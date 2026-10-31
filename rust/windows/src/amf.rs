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
mod properties;
mod rate_control;
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
use std::{
    ptr,
    time::{Duration, Instant},
};
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
fn acknowledge_keyframe(
    idr: bool,
    result: AMF_RESULT,
    recover: &mut bool,
    metadata: &mut metadata::State,
) {
    // All reasons are satisfied by the same accepted keyframe. A non-IDR
    // must retain a recovery request raised by poll() during backpressure.
    if idr && result == AMF_RESULT_AMF_OK {
        *recover = false;
        metadata.submitted(result);
    }
}
fn output_is_idr(picture_type: Option<i64>, requested_idr: bool) -> bool {
    // A driver may not expose the output-type property. Use the request attached
    // to this output's PTS; the global input counter can already be several
    // frames ahead, or continue an earlier encoder's frame numbering.
    picture_type.map_or(requested_idr, |kind| kind == 0)
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
/// The pace-based cap target keyframes are measured against: none for an
/// explicit `amd_max_frame_size` (including driver default), which is the
/// user's limit, not the host's.
fn frame_cap_target(automatic: bool, bytes: u64) -> Option<u64> {
    (automatic && bytes > 0).then_some(bytes)
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
    fn prepare_surface(
        &mut self,
        surface: *mut AMFSurface,
        idr: bool,
    ) -> Result<(butterpollo_core::ltr::Plan, bool)> {
        let (plan, idr, ltr_rejected) =
            self.prepare_surface_at(surface, self.index as u64 + 1, idr)?;
        if ltr_rejected {
            self.references.disable();
        }
        Ok((plan, idr))
    }
    /// [`Self::prepare_surface`] for wire frame `frame`: a re-encode prepares
    /// the frame already submitted again. It changes only the surface: when
    /// the surface rejects the LTR properties, the plan is the keyframe of
    /// disabled references and the caller disables them (the third value)
    /// once that is certain.
    fn prepare_surface_at(
        &mut self,
        surface: *mut AMFSurface,
        frame: u64,
        mut idr: bool,
    ) -> Result<(butterpollo_core::ltr::Plan, bool, bool)> {
        let mut plan = self.references.plan(frame, idr);
        let [_, _, mark, reference] = self.ltr_properties();
        let apply = |name: &str, value: AMFVariantStruct| -> Result<()> {
            // SAFETY: Both callers own surface throughout preparation. These local
            // variants have matching scalar tags, and AMF copies them and the terminated
            // property name synchronously while the runtime remains loaded.
            unsafe {
                check(((*(*surface).pVtbl).SetProperty.unwrap())(
                    surface,
                    to_wide(name).as_ptr(),
                    value,
                ))
            }
        };
        let ltr = (|| -> Result<()> {
            if let Some(slot) = plan.mark {
                apply(mark, int(slot as i64))?;
            }
            if let Some(slot) = plan.reference {
                apply(reference, int(1 << slot))?;
            }
            Ok(())
        })();
        let ltr_rejected = ltr.is_err();
        if let Err(error) = ltr {
            tracing::warn!(%error,"AMF LTR surface rejected; requesting a larger IDR recovery frame. Disable amd_ltr_frames if this repeats");
            idr = true;
            plan = butterpollo_core::ltr::References::disabled_keyframe_plan();
        }
        if idr {
            apply(
                match self.codec {
                    0 => "ForcePictureType",
                    1 => "HevcForcePictureType",
                    _ => "Av1ForceFrameType",
                },
                int(if self.codec == 2 { 1 } else { 2 }),
            )?;
            // A requested keyframe must start a fresh decoder: a client that
            // reset its decoder otherwise stays frozen. AV1 repeats its
            // sequence header only when asked, as the original backend did.
            let headers: &[&str] = match self.codec {
                0 => &["InsertSPS", "InsertPPS"],
                1 => &["HevcInsertHeader"],
                _ => &["Av1ForceInsertSequenceHeader"],
            };
            for header in headers {
                apply(header, gpu::boolean(true))?;
            }
        }
        // LTR rejection can force a keyframe here even when the caller did not
        // request one. Retain that effective type for acknowledgements and output.
        Ok((plan, idr, ltr_rejected))
    }
    fn expire_unreturned(&mut self) {
        // A submission the driver never answers would otherwise keep every
        // later query waiting for its timeout.
        let now = Instant::now();
        while self.in_flight.front().is_some_and(|s| s.unreturned(now)) {
            let stale = self.in_flight.pop_front();
            if let Some(stale) = &stale {
                self.units.forget(stale.pts);
            }
            match unreturned_notice(self.probe, stale.map(|s| s.pts)) {
                Unreturned::Warning(message) => self.warnings.event(
                    "encoder_dropped",
                    message,
                    butterpollo_core::session::EVENT_PERIOD,
                ),
                Unreturned::Probe(message) => tracing::info!("{message}"),
            }
            self.recover = true;
        }
    }
    pub fn poll(&mut self) -> Result<Vec<Encoded>> {
        // A unit of a joined frame leaves nothing to hand on: ask again at
        // once for the frame's next unit instead of after the caller's wait.
        loop {
            self.unit_held = false;
            let output = self.poll_once()?;
            if !(self.unit_held && output.is_empty()) {
                return Ok(output);
            }
        }
    }
    fn poll_once(&mut self) -> Result<Vec<Encoded>> {
        if let Some(input) = &mut self.compute {
            input.converter.release_completed();
        }
        // SAFETY: Self retains the component/runtime; encoder QueryOutput returns an
        // owned AMFBuffer through its AMFData base. Null and size checks bound its host
        // bytes, copied before Release; the picture union is read only with an INT64 tag.
        unsafe {
            let mut output = vec![];
            // With a query timeout, asking when nothing is in flight would only
            // wait for it. One finished frame is handed to the sender at once:
            // asking for the next one still in flight would hold it for the
            // timeout. Callers poll again while frames are pending.
            if self.in_flight.is_empty() {
                return Ok(output);
            }
            let mut data = ptr::null_mut();
            let code = timed("AMF QueryOutput", || {
                ((*(*self.component).pVtbl).QueryOutput.unwrap())(self.component, &mut data)
            });
            self.last_query = Some(code);
            if code == AMF_RESULT_AMF_REPEAT
                || code == AMF_RESULT_AMF_EOF
                || code == AMF_RESULT_AMF_NEED_MORE_INPUT
            {
                // A late caller does not mean the GPU stopped producing.
                // Query first, preserving ready pictures and their timestamps;
                // only an empty query can retire an unanswered submission.
                self.expire_unreturned();
                return Ok(output);
            }
            check(code)?;
            if data.is_null() {
                self.expire_unreturned();
                return Ok(output);
            }
            let buffer = data as *mut AMFBuffer;
            let pts = ((*(*data).pVtbl).GetPts.unwrap())(data);
            if self.unit_log.is_some() && self.log_unit(buffer, pts) {
                return Ok(output);
            }
            // In slice output mode a frame stays in flight until its last unit.
            let kind = if self.slice_blocks > 0 {
                Self::buffer_kind(self.codec, buffer)
            } else {
                None
            };
            let last = kind != Some(1);
            let position = self.in_flight.iter().position(|s| s.pts == pts);
            let submission = position.map(|position| {
                let s = &self.in_flight[position];
                Submitted {
                    started: s.started,
                    accepted: s.accepted,
                    presentation: s.presentation,
                    idr: s.idr,
                    after_invalidation: s.after_invalidation,
                    frame_cap_bytes: s.frame_cap_bytes,
                    frame_cap_target: s.frame_cap_target,
                    reencoded: s.reencoded,
                    skipped: s.skipped,
                }
            });
            let finished = position
                .filter(|_| last)
                .and_then(|position| self.in_flight.remove(position));
            // Nothing submitted after it references the frame yet.
            let newest = finished.is_some() && position == Some(self.in_flight.len());
            let latency = submission.as_ref().map(|s| s.started.elapsed());
            let presentation = submission.as_ref().map(|s| s.presentation);
            let timing = submission
                .as_ref()
                .map(|s| (s.accepted, s.accepted.duration_since(s.started)));
            let requested_idr = submission.as_ref().is_some_and(|s| s.idr);
            let frame_cap_bytes = submission.as_ref().and_then(|s| s.frame_cap_bytes);
            let frame_cap_target = submission.as_ref().and_then(|s| s.frame_cap_target);
            let reencoded = submission.as_ref().is_some_and(|s| s.reencoded);
            let after_invalidation = submission.is_some_and(|s| s.after_invalidation);
            let skipped = submission.map_or(0, |s| s.skipped);
            let v = &*(*buffer).pVtbl;
            let size = (v.GetSize.unwrap())(buffer);
            let raw = (v.GetNative.unwrap())(buffer) as *const u8;
            let mut picture: AMFVariantStruct = std::mem::zeroed();
            let prop = match self.codec {
                0 => "OutputDataType",
                1 => "HevcOutputDataType",
                _ => "Av1OutputFrameType",
            };
            let _ = (v.GetProperty.unwrap())(buffer, to_wide(prop).as_ptr(), &mut picture);
            self.focus_feedback(buffer, pts, last);
            if size > 64 * 1024 * 1024 || raw.is_null() {
                (v.Release.unwrap())(buffer);
                bail!("invalid AMF output buffer");
            }
            let picture_type = if picture.type_ == AMF_VARIANT_TYPE_AMF_VARIANT_INT64 {
                Some(picture.__bindgen_anon_1.int64Value)
            } else {
                None
            };
            let idr = output_is_idr(picture_type, requested_idr);
            if idr && !requested_idr {
                // A keyframe the encoder chose (a pre-analysis scene change
                // or the GOP) settles the rate control as a requested one.
                self.focus.keyframe_out();
            }
            if let Some(cut) = finished.as_ref().filter(|cut| {
                reencodes_cut(
                    idr || cut.idr || cut.reencoded,
                    size as u64,
                    cut.cut_limit,
                    newest && !self.preparing && self.slice_blocks == 0,
                )
            }) {
                match self.reencode_as_idr(cut) {
                    Ok(()) => {
                        (v.Release.unwrap())(buffer);
                        // poll() asks again at once, for the keyframe.
                        self.unit_held = true;
                        return Ok(output);
                    }
                    Err(error) => {
                        self.reencode_failures += 1;
                        tracing::error!(error = %format!("{error:#}"), pts, bytes = size, failures = self.reencode_failures, "AMF could not encode an oversize scene cut again as a keyframe; sending the inter frame, which loses its FEC. Report this with the host log.");
                        self.warnings.event("encoder_reencode", format!("AMF could not encode an oversize scene cut again as a keyframe ({error:#}); the frame goes without full FEC and may freeze the picture briefly. Report this with the host log."), butterpollo_core::session::EVENT_PERIOD);
                    }
                }
            }
            let Some(sent) = self.repaired(
                std::slice::from_raw_parts(raw, size as usize),
                skipped,
                idr && requested_idr,
            ) else {
                (v.Release.unwrap())(buffer);
                if self.slice_blocks > 0 {
                    self.units.forget(pts);
                }
                if last {
                    self.frame_completed(timing, false);
                }
                return Ok(output);
            };
            self.frame_bytes += sent.len();
            if last {
                let frame_bytes = std::mem::take(&mut self.frame_bytes);
                if idr && over_cap_target(frame_bytes as u64, frame_cap_target) {
                    self.keyframes_over_cap += 1;
                }
                // The written cap is what the driver held at submission.
                if idr
                    && self.frame_cap.automatic()
                    && self.slice_blocks == 0
                    && let (Some(target), Some(written)) = (frame_cap_target, frame_cap_bytes)
                    && let Some((lowered, now)) =
                        self.keyframe_cap
                            .observe(frame_bytes as u64, target, written)
                {
                    tracing::warn!(
                        codec = self.codec,
                        lowered_overshoot = (lowered * 100.).round() / 100.,
                        overshoot = (now * 100.).round() / 100.,
                        keyframe_bytes = frame_bytes,
                        cap_bytes = written,
                        target_bytes = target,
                        "AMF keyframes did not shrink under a lower frame cap; the driver ignores it for keyframes, so the cap returns to its target for this encoder"
                    );
                }
                self.references.encoded(idr, frame_bytes);
            }
            let bytes: &[u8] = &sent;
            if self.slice_blocks > 0 {
                let streamable = !self.slices_joined
                    && !idr
                    && !after_invalidation
                    && frame_cap_bytes.is_some_and(|cap| cap > 0);
                let pushed = self.units.push(pts, kind, bytes, || units::Meta {
                    streamed: streamable,
                    idr,
                });
                (v.Release.unwrap())(buffer);
                // Whether a frame is a keyframe is read from its first slice.
                // If a later one says otherwise, that assumption is wrong for
                // this driver: say so, once, rather than stream a keyframe.
                if idr != pushed.meta.idr && !self.unit_idr_reported {
                    self.unit_idr_reported = true;
                    tracing::error!(
                        pts,
                        first_slice_idr = pushed.meta.idr,
                        this_slice_idr = idr,
                        "AMF reported a different picture type on a later slice of one frame; slice delivery decides a frame's type from its first slice. Report this with the host log."
                    );
                }
                if let Some(lost) = pushed.abandoned {
                    self.lost_frame(lost);
                }
                let (unit, bytes) = match pushed.step {
                    units::Step::Hold => {
                        self.unit_held = true;
                        return Ok(output);
                    }
                    units::Step::Unit(unit, bytes) => (unit, bytes),
                    units::Step::Whole(bytes) => (Unit::Whole, bytes),
                };
                // Dynamic metadata goes before the first slice or tile, the
                // Dolby Vision RPU after the last (AV1: before the first).
                let (spliced, frame_cap_bytes) = dynamic::output(
                    self.dynamic.as_deref_mut(),
                    unit,
                    &bytes,
                    frame_cap_bytes,
                    pushed.meta.idr,
                );
                let bytes = spliced.unwrap_or(bytes);
                let whole = unit == Unit::Whole;
                // A joined or frame-mode keyframe is recovered as a whole.
                let bytes =
                    if whole && self.codec == 0 && pushed.meta.idr && self.references.enabled() {
                        butterpollo_core::bitstream::h264_reference_recovery(&bytes)
                    } else {
                        bytes
                    };
                if last {
                    self.frame_completed(timing, !bytes.is_empty());
                }
                output.push(Encoded {
                    bytes,
                    unit,
                    idr: whole && pushed.meta.idr,
                    after_invalidation: whole && after_invalidation,
                    frame_cap_bytes,
                    reencoded: whole && reencoded,
                    frame_cap_target,
                    latency,
                    presentation,
                });
                return Ok(output);
            }
            // The sender outlives this AMF buffer reference. Own the compressed
            // bytes before Release makes its native storage available for reuse.
            // H.264 carries no dynamic metadata (`dynamic::wanted`).
            let (bytes, frame_cap_bytes) = if self.codec == 0 && idr && self.references.enabled() {
                (
                    butterpollo_core::bitstream::h264_reference_recovery(bytes),
                    frame_cap_bytes,
                )
            } else {
                let (spliced, cap) = dynamic::output(
                    self.dynamic.as_deref_mut(),
                    Unit::Whole,
                    bytes,
                    frame_cap_bytes,
                    idr,
                );
                (spliced.unwrap_or_else(|| sent.into_owned()), cap)
            };
            (v.Release.unwrap())(buffer);
            self.frame_completed(timing, !bytes.is_empty());
            output.push(Encoded {
                bytes,
                unit: Unit::Whole,
                idr,
                after_invalidation,
                frame_cap_bytes,
                reencoded,
                frame_cap_target,
                latency,
                presentation,
            });
            Ok(output)
        }
    }
    /// [`dynamic::sample`]: a failed statistics pass costs the stream its
    /// dynamic metadata, never a frame.
    fn sample_dynamic(&mut self) {
        dynamic::sample(
            &mut self.dynamic,
            stats(
                self.compute.as_deref_mut(),
                self.graphics_stats.as_deref_mut(),
            ),
            &self.warnings,
        );
    }
    /// The `OutputBufferType` of a buffer QueryOutput returned: 0 frame, 1 slice
    /// or tile, 2 last slice or tile. None when the driver does not say.
    ///
    /// # Safety
    /// `buffer` is a live AMF buffer.
    unsafe fn buffer_kind(codec: u8, buffer: *mut AMFBuffer) -> Option<i64> {
        // SAFETY: The caller passes the live buffer; the property storage is
        // initialized and only an INT64 tag is read.
        unsafe {
            let v = &*(*buffer).pVtbl;
            let name = match codec {
                0 => "OutputBufferType",
                1 => "HevcOutputBufferType",
                _ => "AV1OutputBufferType",
            };
            let mut value: AMFVariantStruct = std::mem::zeroed();
            ((v.GetProperty.unwrap())(buffer, to_wide(name).as_ptr(), &mut value)
                == AMF_RESULT_AMF_OK
                && value.type_ == AMF_VARIANT_TYPE_AMF_VARIANT_INT64)
                .then_some(value.__bindgen_anon_1.int64Value)
        }
    }
    /// The backlog learns from every frame AMF finishes.
    fn frame_completed(&mut self, timing: Option<(Instant, Duration)>, produced: bool) {
        if let Some((accepted, preparation)) = timing.filter(|_| produced) {
            let previous = self.backlog.limit();
            let now = Instant::now();
            self.backlog
                .completed(now, now.duration_since(accepted), preparation);
            if self.backlog.limit() != previous {
                tracing::debug!(
                    previous,
                    limit = self.backlog.limit(),
                    "AMF application backlog learned"
                );
            }
        }
    }
    /// AMF's bytes as the client must get them ([`RepairHeaders`]), or
    /// `None` to drop them: after a rewrite fails, every output until the
    /// requested keyframe, which brings AMF's own headers back.
    ///
    /// [`RepairHeaders`]: butterpollo_core::bitstream::RepairHeaders
    fn repaired<'a>(
        &mut self,
        bytes: &'a [u8],
        skipped: u64,
        requested_keyframe: bool,
    ) -> Option<std::borrow::Cow<'a, [u8]>> {
        #[cfg(test)]
        if let Some(unrewritten) = self.unrewritten.as_mut() {
            unrewritten.extend_from_slice(bytes);
        }
        if self.repair_dropping {
            if !requested_keyframe {
                return None;
            }
            self.repair_dropping = false;
        }
        match butterpollo_core::bitstream::repaired(self.repair_headers.as_mut(), bytes, skipped) {
            Ok(bytes) => Some(bytes),
            Err(error) => {
                tracing::error!(%error, codec = self.codec, "AMF output has headers the long-term reference repair cannot rewrite; dropping frames until a keyframe, then recovering with keyframes for the rest of the session. Report this with the host log.");
                self.repair_headers = None;
                self.references.disable();
                self.recover = true;
                self.repair_dropping = true;
                None
            }
        }
    }
    /// Slice output of a frame that never got its last unit: the sender may
    /// have started it, so the next frame is a keyframe.
    fn lost_frame(&mut self, pts: i64) {
        self.warnings.event("encoder_dropped", format!("AMF returned frame {pts} in slices and never finished it; requesting a keyframe. You may see a pause; lower game GPU load or update the AMD driver if this repeats."), butterpollo_core::session::EVENT_PERIOD);
        self.recover = true;
    }
    /// Blocks a P-frame is sent in, as its slices leave the encoder; 0 when
    /// `video_slice_delivery` is off.
    pub fn slice_delivery(&self) -> u8 {
        if self.slices_joined {
            0
        } else {
            self.slice_blocks
        }
    }
    /// Return every later frame whole, even P-frames, from the next one on.
    /// The encoder stays in slice output mode. A frame whose first unit has
    /// left is finished as it began.
    pub fn join_slices(&mut self) {
        self.slices_joined = true;
    }
    /// Record one output unit for the probe. True for a slice or tile that is
    /// not the last of its frame: the frame stays in flight.
    ///
    /// # Safety
    /// `buffer` is the AMF buffer QueryOutput just returned; this releases it
    /// when it returns true.
    unsafe fn log_unit(&mut self, buffer: *mut AMFBuffer, pts: i64) -> bool {
        let at = Instant::now();
        // SAFETY: The caller passes the live buffer; the property storage is
        // initialized and only an INT64 tag is read.
        unsafe {
            let v = &*(*buffer).pVtbl;
            let kind = Self::buffer_kind(self.codec, buffer);
            let submission = self.in_flight.iter().find(|s| s.pts == pts);
            let unit = OutputUnit {
                pts,
                kind,
                bytes: (v.GetSize.unwrap())(buffer),
                at,
                started: submission.map(|s| s.started),
                accepted: submission.map(|s| s.accepted),
            };
            if let Some(log) = self.unit_log.as_mut() {
                log.push(unit);
            }
            let partial = kind == Some(1);
            if partial {
                (v.Release.unwrap())(buffer);
            }
            partial
        }
    }
    /// A CPU image, such as the codec probe's, goes through the same GPU
    /// conversion as a capture.
    pub fn encode(&mut self, image: &Image, idr: bool, bitrate: u32) -> Result<Vec<Encoded>> {
        anyhow::ensure!(
            self.overlay.is_none(),
            "couch menu: needs wgc_direct_encoder_input"
        );
        let image = GpuImage::upload(&self._device, image).context("AMF image upload")?;
        self.encode_gpu(&image, idr, bitrate)
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
/// How a frame AMF did not return within two seconds is reported.
#[derive(Debug, PartialEq)]
enum Unreturned {
    /// A stream's: shown to the client and in the console.
    Warning(String),
    /// The capability probe's: only logged. The first frame of an encoder the
    /// driver has just started (the service probes at boot or right after an
    /// install, and an iGPU shares memory with the desktop) can be slow; the
    /// probe still judges the codec by what it returns afterwards, and nobody
    /// watches it.
    Probe(String),
}
fn unreturned_notice(probe: bool, pts: Option<i64>) -> Unreturned {
    if probe {
        Unreturned::Probe(format!(
            "AMF returned no output for the probe's frame {pts:?} within two seconds; the driver was probably still starting up. The probe goes on with a keyframe and counts the codec by what it returns."
        ))
    } else {
        Unreturned::Warning(format!(
            "AMF returned no output for frame {pts:?} within two seconds; dropping it and requesting a keyframe. You may see a pause; lower game GPU load or update the AMD driver if this repeats."
        ))
    }
}
impl Encoder {
    /// Apply a pace-based encoded-byte budget. An explicit user setting,
    /// including driver default (zero), is preserved. Optional property failures
    /// warn without interrupting the stream; return only the observed limit.
    /// An automatic cap in frame mode is written under `bytes` while AMF's
    /// keyframes overshoot it ([`KeyframeCapScale`]), never under `floor`;
    /// submissions keep `bytes` as their target.
    pub fn set_frame_cap(&mut self, bytes: u64, floor: u64) -> Option<u64> {
        let target = bytes;
        self.frame_cap_target = frame_cap_target(self.frame_cap.automatic(), target);
        let bytes = butterpollo_core::encoder_policy::keyframe_cap_bytes(
            self.frame_cap.automatic(),
            self.slice_blocks,
            &self.keyframe_cap,
            target,
            floor,
        );
        let component = self.component;
        self.frame_cap
            .update_toward(self.codec, bytes, target, &self.warnings, |name, bits| {
                // SAFETY: Self owns the live component/runtime for this entire
                // update. SetProperty copies the terminated name and integer.
                unsafe {
                    let result = check(((*(*component).pVtbl).SetProperty.unwrap())(
                        component,
                        to_wide(name).as_ptr(),
                        int(bits),
                    ));
                    (result, Self::read_component(component, name))
                }
            })
    }
    pub fn frame_cap(&self) -> Option<u64> {
        self.frame_cap.effective()
    }
    /// The largest inter frame sent as encoded ([`reencodes_cut`]), with an
    /// automatic frame cap only: an explicit `amd_max_frame_size` is kept.
    pub fn set_cut_limit(&mut self, bytes: u64) {
        self.cut_limit = (self.frame_cap.automatic() && bytes > 0).then_some(bytes);
    }
    pub fn reencoded_cuts(&self) -> u64 {
        self.reencoded_cuts
    }
    /// Keyframes AMF made more than 1.1 times the cap the host asked for.
    pub fn keyframes_over_cap(&self) -> u64 {
        self.keyframes_over_cap
    }
    pub fn keyframe_cap_scale(&self) -> f64 {
        self.keyframe_cap.scale()
    }
    fn set_bitrate(&mut self, bitrate: u32) -> Result<()> {
        let changed = bitrate != self.rate_control.requested();
        let component = self.component;
        self.rate_control.update(
            self.codec,
            bitrate,
            &self.warnings,
            |name| {
                // SAFETY: Self retains the component/runtime and its borrowed
                // property info. Null checks and INT64 tags precede all reads;
                // only copied integers escape this call.
                unsafe {
                    let mut current = int(0);
                    let mut info = ptr::null();
                    let table = &*(*component).pVtbl;
                    if (table.GetProperty.unwrap())(component, to_wide(name).as_ptr(), &mut current)
                        != AMF_RESULT_AMF_OK
                        || current.type_ != AMF_VARIANT_TYPE_AMF_VARIANT_INT64
                        || (table.GetPropertyInfo.unwrap())(
                            component,
                            to_wide(name).as_ptr(),
                            &mut info,
                        ) != AMF_RESULT_AMF_OK
                        || info.is_null()
                    {
                        return None;
                    }
                    Some(rate_control::Limit {
                        value: current.__bindgen_anon_1.int64Value,
                        minimum: ((*info).minValue.type_ == AMF_VARIANT_TYPE_AMF_VARIANT_INT64)
                            .then(|| (*info).minValue.__bindgen_anon_1.int64Value),
                        maximum: ((*info).maxValue.type_ == AMF_VARIANT_TYPE_AMF_VARIANT_INT64)
                            .then(|| (*info).maxValue.__bindgen_anon_1.int64Value),
                    })
                }
            },
            |name, value| {
                // SAFETY: The live component copies the terminated name and
                // the correctly tagged integer during SetProperty.
                unsafe {
                    check(((*(*component).pVtbl).SetProperty.unwrap())(
                        component,
                        to_wide(name).as_ptr(),
                        int(value),
                    ))
                    .with_context(|| format!("AMF property {name}"))
                }
            },
        );
        if changed {
            self.frame_cap
                .observe(self.read(rate_control::frame_cap_name(self.codec)));
        }
        Ok(())
    }
    pub fn accepts_gpu_device(&self, image: &GpuImage) -> bool {
        self._device.device.as_raw() == image.gpu.device.as_raw()
            && (self.compute.is_none() || crate::compute::shareable(&image.texture))
    }
    fn wait_capacity(&mut self) -> Result<Vec<Encoded>> {
        // Only a full queue waits: a poll blocks for the query timeout while a
        // frame is in flight, which delayed submitting the next frame.
        let mut output = Vec::new();
        let deadline = Instant::now() + Duration::from_millis(100);
        while self.in_flight.len() >= AMF_SURFACE_LIMIT
            || self.ownership.retained() >= AMF_SURFACE_LIMIT
        {
            let queried = Instant::now();
            output.extend(self.poll()?);
            // A completion at the deadline still frees capacity successfully.
            let retained = self.ownership.retained();
            if self.in_flight.len() < AMF_SURFACE_LIMIT && retained < AMF_SURFACE_LIMIT {
                break;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!("AMF GPU queue failed to drain");
            }
            if retained >= AMF_SURFACE_LIMIT {
                // Output can be complete while AMF still owns every input.
                // QueryOutput then returns immediately; wait on the native
                // release notification instead of spinning on that empty poll.
                self.ownership
                    .wait_below(AMF_SURFACE_LIMIT, remaining.min(QUERY_TIMEOUT));
            } else {
                // Some drivers ignore QueryTimeout or return REPEAT early.
                // Bound those retries too, without extending a real driver wait.
                let pause = QUERY_TIMEOUT
                    .saturating_sub(queried.elapsed())
                    .min(remaining);
                if !pause.is_zero() {
                    std::thread::sleep(pause);
                }
            }
        }
        Ok(output)
    }
    /// Submit `cut`'s converted input again as a keyframe, in place of the
    /// inter frame AMF made of it ([`reencodes_cut`]). The keyframe takes the
    /// frame's index and resets the long-term references, so a later repair
    /// names only the keyframe. SubmitInput is the last step that can fail:
    /// before it, nothing has changed.
    fn reencode_as_idr(&mut self, cut: &Submission) -> Result<()> {
        let surface = match (&cut._converted, &cut._converted12, self.compute.as_ref()) {
            (Some(texture), _, _) => gpu::wrap(self.context, texture, &mut self.ownership)?.0,
            (None, Some(texture), Some(input)) => {
                input.context.wrap(texture, &mut self.ownership)?
            }
            _ => bail!("the frame's converted input was not kept"),
        };
        let frame = cut.pts as u64 + 1;
        // SAFETY: As in `encode_gpu`: the fresh surface wraps the converted
        // texture the submission kept, which no later conversion has reused.
        unsafe {
            let v = &*(*surface.0).pVtbl;
            check((v.SetCrop.unwrap())(
                surface.0,
                0,
                0,
                self.config.coded_size().0 as i32,
                self.config.coded_size().1 as i32,
            ))
            .context("AMF native surface crop")?;
            (v.SetPts.unwrap())(surface.0, cut.pts);
            (v.SetDuration.unwrap())(
                surface.0,
                10_000_000_000 / i64::from(self.config.fps_millihz()),
            );
            let (plan, _, ltr_rejected) = self.prepare_surface_at(surface.0, frame, true)?;
            let result = timed("AMF SubmitInput", || {
                ((*(*self.component).pVtbl).SubmitInput.unwrap())(self.component, surface.0.cast())
            });
            self.last_submit = Some(result);
            check(result).context("AMF SubmitInput of the keyframe")?;
            // Only now: had SubmitInput failed, the references stay as they were.
            if ltr_rejected {
                self.references.disable();
            }
            acknowledge_keyframe(true, result, &mut self.recover, &mut self.hdr_metadata);
            let accepted = Instant::now();
            self.last_accepted = Some(accepted);
            self.in_flight.push_back(Submission {
                pts: cut.pts,
                // From the first submission: the frame's encode time counts both.
                started: cut.started,
                accepted,
                presentation: cut.presentation,
                idr: true,
                after_invalidation: false,
                frame_cap_bytes: cut.frame_cap_bytes,
                frame_cap_target: cut.frame_cap_target,
                skipped: plan.skipped,
                cut_limit: cut.cut_limit,
                reencoded: true,
                _capture: cut._capture.clone(),
                _converted: cut._converted.clone(),
                _converted12: None,
            });
            self.references.accepted(frame, &plan);
            self.focus.keyframe_out();
            self.reencoded_cuts += 1;
            Ok(())
        }
    }
    pub fn encode_gpu(
        &mut self,
        image: &GpuImage,
        idr: bool,
        bitrate: u32,
    ) -> Result<Vec<Encoded>> {
        let started = Instant::now();
        let mut output = self.wait_capacity()?;
        let idr = idr || self.hdr_metadata.needs_idr() || self.recover;
        if self._device.device.as_raw() != image.gpu.device.as_raw() {
            bail!("GPU frame and encoder must use the same D3D11 device");
        }
        let source = (image.width, image.height, image.pixel);
        let (surface, converted, conversion) = if let Some(input) = self.compute.as_mut() {
            let compute = input.converter.compute().clone();
            let texture = timed("compute open", || compute.open(&image.texture))?;
            let pointer = image
                .cursor
                .as_ref()
                .map(|cursor| input.converter.pointer(cursor))
                .transpose()?;
            input.converter.values = crate::gpu_color::constants(
                &self.config,
                source,
                self.luminance,
                image.cursor.as_ref(),
            );
            input.converter.set_overlay(self.overlay.as_ref());
            let converted = timed("compute conversion", || {
                input.converter.convert(
                    &texture,
                    crate::compute::format(image.pixel),
                    pointer.as_ref(),
                    image,
                )
            })?;
            timed("conversion fence wait", || {
                gpu::synchronize(&converted.texture, &converted.fence, converted.value)
            })?;
            let surface = timed("AMF surface wrap", || {
                input.context.wrap(&converted.texture, &mut self.ownership)
            })?;
            // Kept while the frame may need encoding again (`reencodes_cut`).
            let kept = self.cut_limit.map(|_| converted.texture.clone());
            (surface, (None, kept), Some(converted.conversion))
        } else {
            if self
                .gpu_convert
                .as_ref()
                .is_none_or(|converter| converter.source != source)
            {
                self.gpu_convert = Some(gpu::Converter::new(&self._device, &self.config, source)?);
            }
            let color = &mut self.gpu_convert.as_mut().unwrap().color;
            color.set_luminance(self.luminance);
            color.set_overlay(self.overlay.as_ref());
            let (surface, texture) = timed("graphics conversion", || {
                self.gpu_convert
                    .as_mut()
                    .unwrap()
                    .convert(self.context, image, &mut self.ownership)
            })?;
            // Submitted with `sample_dynamic`, once the frame went to the encoder.
            if let Some(stats) = self.graphics_stats.as_deref_mut() {
                stats.queue(image, &self.gpu_convert.as_ref().unwrap().color);
            }
            (surface, (Some(texture), None), None)
        };
        let (converted, converted12) = converted;
        self.set_bitrate(bitrate)?;
        // SAFETY: surface owns a live AMF interface backed by this encoder's device;
        // conversion has synchronized the GPU writes and the boxed ownership observer
        // retains textures until AMF releases them. SubmitInput retains its own surface
        // reference, while self keeps the component, context and runtime alive.
        unsafe {
            let v = &*(*surface.0).pVtbl;
            check((v.SetCrop.unwrap())(
                surface.0,
                0,
                0,
                self.config.coded_size().0 as i32,
                self.config.coded_size().1 as i32,
            ))
            .context("AMF native surface crop")?;
            (v.SetPts.unwrap())(surface.0, self.index);
            (v.SetDuration.unwrap())(
                surface.0,
                10_000_000_000 / i64::from(self.config.fps_millihz()),
            );
            let (plan, idr) = self.prepare_surface(surface.0, idr)?;
            self.attach_focus(surface.0, idr, plan.after_invalidation, conversion)?;
            let deadline = Instant::now() + Duration::from_millis(100);
            loop {
                let result = timed("AMF SubmitInput", || {
                    ((*(*self.component).pVtbl).SubmitInput.unwrap())(
                        self.component,
                        surface.0.cast(),
                    )
                });
                self.last_submit = Some(result);
                acknowledge_keyframe(idr, result, &mut self.recover, &mut self.hdr_metadata);
                if result != AMF_RESULT_AMF_INPUT_FULL {
                    check(result)?;
                    break;
                }
                self.input_full += 1;
                // This input's references are planned: a re-encode now would
                // drop the ones it may name.
                self.preparing = true;
                let polled = self.poll();
                self.preparing = false;
                output.extend(polled?);
                if Instant::now() >= deadline {
                    bail!("AMF GPU input queue failed to drain");
                }
                std::thread::yield_now();
            }
            // Compute keeps the capture until conversion completes; graphics
            // still retains it until the encoder returns its output.
            let accepted = Instant::now();
            self.last_accepted = Some(accepted);
            self.in_flight.push_back(Submission {
                pts: self.index,
                started,
                accepted,
                presentation: image.captured,
                idr,
                after_invalidation: plan.after_invalidation,
                frame_cap_bytes: self.frame_cap.effective(),
                frame_cap_target: self.frame_cap_target,
                skipped: plan.skipped,
                cut_limit: self.cut_limit,
                reencoded: false,
                _capture: self.compute.is_none().then(|| image.clone()),
                _converted: converted,
                _converted12: converted12,
            });
            self.backlog.submitted();
            self.references.accepted(self.index as u64 + 1, &plan);
            self.index += 1;
            // Before the poll, so the frame it returns carries the newest
            // finished sample; the frames already collected stay either way.
            self.sample_dynamic();
            output.extend(self.poll()?);
            Ok(output)
        }
    }
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
