//! Native AMD AMF encoder for H.264, HEVC and AV1.
//!
//! Calls AMF's C ABI directly, takes D3D11 or D3D12
//! textures from capture and Radeon compute without a copy, and applies the
//! per-codec settings from `butterpollo_core::encoder_policy`.
mod create;
mod dynamic;
pub mod focus;
pub(crate) mod gpu;
mod intra_refresh;
mod metadata;
mod output;
mod properties;
mod rate_control;
mod submit;
mod units;

use crate::text::to_wide;
use crate::{
    amf_abi::*,
    capture::{Device, GpuImage, Image},
    encoder::{Encoded, Unit},
};
use anyhow::{Context, Result, bail};
use butterpollo_core::encoder_backlog::{AMF_SURFACE_LIMIT, AmfBacklog, QUERY_TIMEOUT};
use butterpollo_core::encoder_policy::KeyframeCapScale;
#[cfg(test)]
use create::{OutputRange, guard_smart_access_video, transfer_characteristic};
#[cfg(test)]
use output::{Unreturned, output_is_idr, unreturned_notice};
use std::{
    ptr,
    time::{Duration, Instant},
};
#[cfg(test)]
use submit::{acknowledge_keyframe, frame_cap_target};
use windows::core::Interface;
/// A call this slow held the stream thread: a stalled VCN or compute queue
/// blocks inside the driver, where nothing can time it out.
const SLOW_CALL: Duration = Duration::from_millis(50);
fn timed<T>(call: &'static str, run: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let result = run();
    let elapsed = started.elapsed();
    if matches!(call, "AMF Init" | "AMF Terminate" | "AMF context Terminate") {
        tracing::debug!(
            call,
            elapsed_us = elapsed.as_micros() as u64,
            "encoder lifecycle call"
        );
    }
    if elapsed >= SLOW_CALL {
        tracing::warn!(
            call,
            elapsed_ms = elapsed.as_millis() as u64,
            "slow encoder call"
        );
    }
    result
}
/// The SDK name of an AMF result, empty for a code this host does not name.
pub(crate) fn result_name(n: AMF_RESULT) -> &'static str {
    match n {
        AMF_RESULT_AMF_OK => "AMF_OK",
        AMF_RESULT_AMF_FAIL => "AMF_FAIL",
        AMF_RESULT_AMF_UNEXPECTED => "AMF_UNEXPECTED",
        AMF_RESULT_AMF_ACCESS_DENIED => "AMF_ACCESS_DENIED",
        AMF_RESULT_AMF_INVALID_ARG => "AMF_INVALID_ARG",
        AMF_RESULT_AMF_OUT_OF_RANGE => "AMF_OUT_OF_RANGE",
        AMF_RESULT_AMF_OUT_OF_MEMORY => "AMF_OUT_OF_MEMORY",
        AMF_RESULT_AMF_INVALID_POINTER => "AMF_INVALID_POINTER",
        AMF_RESULT_AMF_NO_INTERFACE => "AMF_NO_INTERFACE",
        AMF_RESULT_AMF_NOT_IMPLEMENTED => "AMF_NOT_IMPLEMENTED",
        AMF_RESULT_AMF_NOT_SUPPORTED => "AMF_NOT_SUPPORTED",
        AMF_RESULT_AMF_NOT_FOUND => "AMF_NOT_FOUND",
        AMF_RESULT_AMF_ALREADY_INITIALIZED => "AMF_ALREADY_INITIALIZED",
        AMF_RESULT_AMF_NOT_INITIALIZED => "AMF_NOT_INITIALIZED",
        AMF_RESULT_AMF_INVALID_FORMAT => "AMF_INVALID_FORMAT",
        AMF_RESULT_AMF_WRONG_STATE => "AMF_WRONG_STATE",
        AMF_RESULT_AMF_NO_DEVICE => "AMF_NO_DEVICE",
        AMF_RESULT_AMF_DIRECTX_FAILED => "AMF_DIRECTX_FAILED",
        AMF_RESULT_AMF_INVALID_DATA_TYPE => "AMF_INVALID_DATA_TYPE",
        AMF_RESULT_AMF_INVALID_RESOLUTION => "AMF_INVALID_RESOLUTION",
        AMF_RESULT_AMF_CODEC_NOT_SUPPORTED => "AMF_CODEC_NOT_SUPPORTED",
        AMF_RESULT_AMF_SURFACE_FORMAT_NOT_SUPPORTED => "AMF_SURFACE_FORMAT_NOT_SUPPORTED",
        AMF_RESULT_AMF_ENCODER_NOT_PRESENT => "AMF_ENCODER_NOT_PRESENT",
        _ => "",
    }
}
/// What a result that says "this hardware cannot" means to the user.
fn result_hint(n: AMF_RESULT) -> &'static str {
    match n {
        AMF_RESULT_AMF_NOT_SUPPORTED
        | AMF_RESULT_AMF_CODEC_NOT_SUPPORTED
        | AMF_RESULT_AMF_ENCODER_NOT_PRESENT => {
            " (this GPU or driver has no encoder for the request)"
        }
        AMF_RESULT_AMF_NO_DEVICE | AMF_RESULT_AMF_DIRECTX_FAILED => {
            " (the driver could not use this graphics device)"
        }
        AMF_RESULT_AMF_INVALID_RESOLUTION => " (size or frame rate beyond the encoder's limit)",
        AMF_RESULT_AMF_SURFACE_FORMAT_NOT_SUPPORTED => " (input format not supported)",
        _ => "",
    }
}
pub(crate) fn amf_error_text(n: AMF_RESULT) -> String {
    match result_name(n) {
        "" => format!("AMF error {n}"),
        name => format!("AMF error {name} ({n}){}", result_hint(n)),
    }
}
pub(crate) fn check(n: AMF_RESULT) -> Result<()> {
    if n != AMF_RESULT_AMF_OK {
        bail!("{}", amf_error_text(n));
    }
    Ok(())
}
pub(crate) fn int(n: i64) -> AMFVariantStruct {
    AMFVariantStruct {
        type_: AMF_VARIANT_TYPE_AMF_VARIANT_INT64,
        __bindgen_anon_1: AMFVariantStruct__bindgen_ty_1 { int64Value: n },
    }
}
/// What a returned output needs of its submission, copied so a frame can stay
/// in flight while its units come.
#[derive(Clone, Copy)]
struct Submitted {
    started: Instant,
    accepted: Instant,
    presentation: Instant,
    idr: bool,
    after_invalidation: bool,
    frame_cap_bytes: Option<u64>,
    frame_cap_target: Option<u64>,
    reencoded: bool,
    /// Frames since the IDR the client never decoded (`ltr::Plan::skipped`).
    skipped: u64,
}
struct Submission {
    pts: i64,
    started: Instant,
    accepted: Instant,
    presentation: Instant,
    idr: bool,
    after_invalidation: bool,
    frame_cap_bytes: Option<u64>,
    /// The last [`Encoder::set_frame_cap`] request when it was submitted.
    frame_cap_target: Option<u64>,
    skipped: u64,
    /// The largest inter frame sent as AMF made it ([`reencodes_cut`]).
    cut_limit: Option<u64>,
    /// This is the keyframe re-encode of an inter frame past `cut_limit`.
    reencoded: bool,
    _capture: Option<GpuImage>,
    _converted: Option<std::sync::Arc<windows::Win32::Graphics::Direct3D11::ID3D11Texture2D>>,
    /// The compute conversion's output, kept like `_converted` so the frame
    /// can be encoded again.
    _converted12: Option<std::sync::Arc<windows::Win32::Graphics::Direct3D12::ID3D12Resource>>,
}
/// How long AMF may hold an accepted frame before it counts as lost.
const UNRETURNED_AFTER: Duration = Duration::from_secs(2);
impl Submission {
    /// Whether AMF has held this frame too long, counted from SubmitInput
    /// accepting it. Time spent uploading or converting before that is not AMF
    /// losing the frame: a packed probe's first frame on an RX 7900 XT once
    /// took 4.7 s before submission, and counting from the start dropped it
    /// as soon as AMF had it.
    fn unreturned(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.accepted) > UNRETURNED_AFTER
    }
}
/// Whether AMF's output goes back to it as a keyframe instead of to the
/// client. AMF picks a frame's QP from the frames before and never encodes
/// again, so after steady frames a scene cut comes out as an inter frame of
/// about 2.4 MB (HEVC 1440p60 at 250 Mbps) that leaves without parity and is
/// lost, while a keyframe of the same picture lands at the frame cap. Only an
/// inter frame past the stream's full-FEC bound, once per frame (`keyframe`
/// covers a requested or re-encoded one), and only while nothing submitted
/// after it can reference it (`alone`).
fn reencodes_cut(keyframe: bool, bytes: u64, limit: Option<u64>, alone: bool) -> bool {
    !keyframe && alone && limit.is_some_and(|limit| bytes > limit)
}
/// A keyframe AMF made more than 1.1 times the cap the host asked for.
fn over_cap_target(bytes: u64, target: Option<u64>) -> bool {
    target.is_some_and(|target| target > 0 && bytes * 10 > target * 11)
}
pub struct Encoder {
    pub(crate) warnings: std::sync::Arc<butterpollo_core::session::Warnings>,
    component: *mut AMFComponent,
    context: *mut AMFContext,
    config: butterpollo_core::rtsp::Negotiated,
    codec: u8,
    _device: Device,
    _library: libloading::Library,
    index: i64,
    gpu_convert: Option<gpu::Converter>,
    in_flight: std::collections::VecDeque<Submission>,
    backlog: AmfBacklog,
    /// A frame AMF never returned may have been a reference: the next frame
    /// is a keyframe, or the client decodes damaged pictures until one comes.
    recover: bool,
    /// Opened to test what the driver can do (the codec capability probe),
    /// not to stream: nobody sees its pauses.
    pub(crate) probe: bool,
    rate_control: rate_control::State,
    frame_cap: rate_control::FrameCap,
    /// The full-FEC bound the stream sets with an automatic frame cap: AMF
    /// does not hold a scene cut's inter frame to the cap, so one past this
    /// is encoded again as a keyframe.
    cut_limit: Option<u64>,
    reencoded_cuts: u64,
    /// The last [`Encoder::set_frame_cap`] request, the target submissions carry.
    frame_cap_target: Option<u64>,
    keyframes_over_cap: u64,
    /// How far under the target the automatic cap is written.
    keyframe_cap: KeyframeCapScale,
    reencode_failures: u64,
    /// An input is prepared and not yet accepted: no re-encode may change the
    /// references its plan names.
    preparing: bool,
    references: butterpollo_core::ltr::References,
    /// AMF's H.264 and HEVC headers as a client that dropped frames until a
    /// long-term reference repair needs them: only on a stream with LTR.
    repair_headers: Option<butterpollo_core::bitstream::RepairHeaders>,
    /// A header rewrite failed: outputs are dropped until the keyframe it
    /// asked for, as their headers no longer match what the client has.
    repair_dropping: bool,
    /// AMF's bytes before any rewrite, kept when a test asks: a decoder that
    /// got every frame needs AMF's own numbering.
    #[cfg(test)]
    unrewritten: Option<Vec<u8>>,
    /// The bytes of a frame whose slices are still coming.
    frame_bytes: usize,
    ownership: Box<gpu::Ownership>,
    pub(crate) luminance: [f32; 2],
    /// The couch menu's panel, drawn by the conversion while set.
    pub(crate) overlay: Option<crate::overlay::Overlay>,
    /// Successfully applied metadata and any pending retry/keyframe.
    hdr_metadata: metadata::State,
    /// Conversion on a D3D12 compute queue, with AMF on D3D12.
    compute: Option<Box<ComputeInput>>,
    /// HDR10+ and HDR Vivid from the conversion's statistics.
    dynamic: Option<Box<dynamic::Dynamic>>,
    /// The statistics pass behind the D3D11 conversion, for a stream that
    /// wants dynamic metadata and converts there.
    graphics_stats: Option<Box<crate::gpu_color::HdrStats>>,
    last_query: Option<AMF_RESULT>,
    last_submit: Option<AMF_RESULT>,
    /// When AMF last accepted an input, for the stream's stage timings.
    last_accepted: Option<Instant>,
    input_full: u64,
    /// Every output unit, with when QueryOutput returned it. Set by the
    /// slice/tile delivery probe only.
    unit_log: Option<Vec<OutputUnit>>,
    /// Blocks a P-frame is sent in when `video_slice_delivery` is on, else 0:
    /// AMF then outputs slice by slice.
    slice_blocks: u8,
    /// Set when the host turns slice delivery off for the session: slice output
    /// stays, but every frame is joined and returned whole.
    slices_joined: bool,
    units: units::Sequencer,
    /// The last poll took a unit of a joined frame and returned nothing.
    unit_held: bool,
    /// A later slice named a different picture type than its frame's first
    /// slice, and it was logged.
    unit_idr_reported: bool,
    /// Encoder focus: the ROI map and its counters.
    focus: focus::State,
}
/// One output buffer from QueryOutput: a frame, or with slice/tile output one
/// slice or tile of it.
pub struct OutputUnit {
    pub pts: i64,
    /// The driver's buffer type: 0 frame, 1 slice or tile, 2 last slice or tile.
    pub kind: Option<i64>,
    pub bytes: usize,
    /// QueryOutput returned the unit.
    pub at: Instant,
    /// The call that submitted the frame began, and AMF accepted the input.
    pub started: Option<Instant>,
    pub accepted: Option<Instant>,
}
struct ComputeInput {
    converter: crate::compute::Converter,
    context: gpu::D3d12Context,
}
impl Encoder {
    /// Start recording every output unit (see [`OutputUnit`]). With slice or
    /// tile output, `poll` then keeps a frame in flight until its last unit.
    pub fn record_units(&mut self) {
        self.unit_log = Some(Vec::new());
    }
    pub fn take_units(&mut self) -> Vec<OutputUnit> {
        self.unit_log
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }
    /// The GPU this encoder opened on.
    pub fn adapter(&self) -> &str {
        &self._device.display.adapter
    }
    pub fn device_removed(&self) -> Option<crate::device_loss::DeviceLost> {
        crate::device_loss::DeviceLost::d3d11(&self._device.device).or_else(|| {
            self.compute.as_ref().and_then(|input| {
                crate::device_loss::DeviceLost::d3d12(&input.converter.compute().device)
            })
        })
    }
    /// Snapshot only at failure/recreation, before dropping the driver state.
    pub fn log_stall(&self) {
        let oldest = self.in_flight.front();
        // SAFETY: the D3D11 device is owned by this encoder and outlives the
        // call; GetDeviceRemovedReason only reads its state.
        let removed = unsafe { self._device.device.GetDeviceRemovedReason() };
        let result_name = |code| match code {
            Some(AMF_RESULT_AMF_OK) => "AMF_OK",
            Some(AMF_RESULT_AMF_REPEAT) => "AMF_REPEAT",
            Some(AMF_RESULT_AMF_EOF) => "AMF_EOF",
            Some(AMF_RESULT_AMF_NEED_MORE_INPUT) => "AMF_NEED_MORE_INPUT",
            Some(AMF_RESULT_AMF_INPUT_FULL) => "AMF_INPUT_FULL",
            Some(_) => "other",
            None => "not called",
        };
        tracing::warn!(
            last_submit = ?self.last_submit,
            last_query = ?self.last_query,
            submit_result = result_name(self.last_submit),
            query_result = result_name(self.last_query),
            input_full = self.input_full,
            in_flight = self.in_flight.len(),
            retained = self.ownership.retained(),
            oldest_pts = ?oldest.map(|s| s.pts),
            oldest_ms = ?oldest.map(|s| s.started.elapsed().as_millis()),
            next_pts = self.index,
            d3d11_removed = ?removed,
            "AMF stall snapshot"
        );
        if let Some(input) = &self.compute {
            input.converter.log_stall();
        }
    }
    pub fn pending(&self) -> bool {
        !self.in_flight.is_empty()
    }
    /// When AMF accepted the last submitted input.
    pub fn last_submitted(&self) -> Option<Instant> {
        self.last_accepted
    }
    /// Frames submitted and not yet returned.
    pub fn backlog(&self) -> usize {
        self.in_flight.len()
    }
    /// Application depth, independent of AMF's driver queue and surface pool.
    pub fn backlog_limit(&self) -> usize {
        self.backlog.admission_limit(Instant::now())
    }
    /// A dedicated paced sender can choose a fresh input near an IDR's end.
    pub fn recovery_backlog_limit(&self) -> usize {
        self.backlog.recovery_admission_limit(
            Instant::now(),
            self.in_flight
                .iter()
                .find(|frame| frame.idr)
                .map(|frame| frame.accepted),
        )
    }
    pub fn recovery_lead(&self) -> Option<Duration> {
        self.backlog.recovery_lead()
    }
    pub fn set_next_frame(&mut self, frame: u64) {
        debug_assert!(self.in_flight.is_empty());
        self.index = frame.saturating_sub(1).min(i64::MAX as u64) as i64;
    }
    pub fn supports_invalidation(&self) -> bool {
        self.references.enabled()
    }
    pub fn invalidation_pending(&self) -> bool {
        self.references.invalidation_pending()
    }
    /// False asks for a keyframe: no long-term reference repairs the range,
    /// or the client's decoder could not order the recovery frame
    /// ([`butterpollo_core::ltr::Counter`]).
    pub fn invalidate_ref_frames(&mut self, first: u64, last: u64) -> bool {
        self.references.invalidate(first, last)
    }
}
/// The statistics pass behind the conversion a frame takes: the compute
/// queue's, or the D3D11 conversion's.
fn stats<'a>(
    compute: Option<&'a mut ComputeInput>,
    graphics: Option<&'a mut crate::gpu_color::HdrStats>,
) -> Option<&'a mut dyn dynamic::Stats> {
    match compute {
        Some(input) => Some(&mut input.converter),
        None => graphics.map(|stats| stats as &mut dyn dynamic::Stats),
    }
}
fn scale_rate_control(value: i64, applied_kbps: u32, requested_kbps: u32) -> i64 {
    let scaled = i128::from(value) * i128::from(requested_kbps) / i128::from(applied_kbps.max(1));
    scaled.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}
impl Drop for Encoder {
    fn drop(&mut self) {
        // Destroy the converter before its context and dynamically loaded runtime.
        self.gpu_convert.take();
        self.graphics_stats.take();
        // SAFETY: Self owns each non-null interface reference. Terminating the component
        // releases its inputs before compute/context cleanup; the device, runtime and
        // boxed surface observer remain alive through termination and each final Release.
        unsafe {
            if !self.component.is_null() {
                timed("AMF Terminate", || {
                    ((*(*self.component).pVtbl).Terminate.unwrap())(self.component)
                });
                ((*(*self.component).pVtbl).Release.unwrap())(self.component);
            }
            // After the encoder has let go of its D3D12 inputs.
            self.compute.take();
            if !self.context.is_null() {
                timed("AMF context Terminate", || {
                    ((*(*self.context).pVtbl).Terminate.unwrap())(self.context)
                });
                ((*(*self.context).pVtbl).Release.unwrap())(self.context);
            }
        }
    }
}

#[cfg(test)]
mod focus_native_tests;
#[cfg(test)]
mod intra_refresh_native_tests;
#[cfg(test)]
mod ltr_native_tests;
#[cfg(test)]
mod tests;
