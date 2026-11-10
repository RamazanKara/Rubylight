//! The video encoder of a stream.
//!
//! [`Encoder`] is AMF on an AMD GPU and NVENC on an NVIDIA GPU for H.264,
//! HEVC and AV1, and PyroWave's own encoder for PyroWave streams. There is no
//! other encoder to fall back to: when the GPU's encoder cannot start, that
//! is the error the stream reports.

use crate::capture::{GpuImage, Image};
use anyhow::{Context, Result};
use butterpollo_core::{encoder_focus, encoder_gpu, rtsp::Negotiated, session::Warnings};

/// How `Encoded::bytes` relates to a picture. Only slice delivery
/// (`video_slice_delivery`: AMF's slice output mode, NVENC's subframe
/// readback) returns anything but `Whole`: a P-frame then comes as `Slice`
/// units in order, ending with `Last`, all of one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Whole,
    Slice { index: u32 },
    Last { index: u32 },
}

pub struct Encoded {
    pub bytes: Vec<u8>,
    /// How `bytes` go on the wire. `None` sends them as they are. PyroWave
    /// sets its record framing or older clients' container here, as pieces of
    /// `bytes`, which the packetizer copies straight into packets
    /// ([`Self::payload`]): `bytes` then holds the codec's own output.
    pub framing: Option<butterpollo_core::pyrowave::Framing>,
    pub unit: Unit,
    pub idr: bool,
    pub after_invalidation: bool,
    /// Driver-read AMF frame limit, or NVENC's VBV bound, at submission. None
    /// means unavailable or unsupported; Some(0) means AMF's
    /// unrestricted/default setting.
    pub frame_cap_bytes: Option<u64>,
    /// The encoder made this keyframe again in place of an oversize scene cut.
    pub reencoded: bool,
    /// The pace-based budget the host asked the encoder to hold this frame to
    /// (the last `set_frame_cap`), against which a keyframe's size is judged.
    pub frame_cap_target: Option<u64>,
    /// Submission through completed codec output, including asynchronous work.
    pub latency: Option<std::time::Duration>,
    pub presentation: Option<std::time::Instant>,
}
impl Encoded {
    /// The picture as the packetizer reads it: `bytes`, or `bytes` framed.
    pub fn payload(&self) -> Result<butterpollo_core::pyrowave::Payload<'_>> {
        Ok(match &self.framing {
            None => butterpollo_core::pyrowave::Payload::Bytes(&self.bytes),
            Some(framing) => framing.view(&self.bytes)?.into(),
        })
    }
    /// Bytes the picture takes on the wire, before packet headers.
    pub fn wire_len(&self) -> usize {
        self.framing
            .as_ref()
            .map_or(self.bytes.len(), butterpollo_core::pyrowave::Framing::len)
    }
}
/// A large frame's buffer goes back to the encoders' pool
/// ([`crate::frame_pool`]) when the sender is done with it.
impl Drop for Encoded {
    fn drop(&mut self) {
        crate::frame_pool::give(std::mem::take(&mut self.bytes));
    }
}

/// A stream size the encoder cannot take: no driver update or sign in helps,
/// a lower resolution on the device does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizeTooLarge {
    pub encoder: &'static str,
    pub width: u32,
    pub height: u32,
    /// The largest width and height, where the driver says them.
    pub max: Option<(u32, u32)>,
}
impl std::fmt::Display for SizeTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}x{} is larger than {} on this GPU can encode",
            self.width, self.height, self.encoder
        )?;
        if let Some((width, height)) = self.max {
            write!(f, " (at most {width}x{height})")?;
        }
        f.write_str("; choose a lower resolution on the device")
    }
}
impl std::error::Error for SizeTooLarge {}

/// The size error in `error`, said once with the adapter, in place of the
/// driver and sign-in advice that does not apply to it.
fn size_failed(error: &anyhow::Error, encoder: &str, adapter: &str) -> Option<anyhow::Error> {
    let size = error.downcast_ref::<SizeTooLarge>()?;
    Some(anyhow::anyhow!(
        "{encoder} could not start on {adapter}: {size}"
    ))
}

/// Whether an AMF encoder that failed with a compute queue is tried again on
/// the graphics queue: not for a size the encoder cannot take on any queue.
fn retries_on_graphics(error: &anyhow::Error) -> bool {
    error.downcast_ref::<SizeTooLarge>().is_none()
}

/// What a stream reports when AMF does not start on `adapter`.
fn amf_failed(error: anyhow::Error, adapter: &str) -> anyhow::Error {
    if let Some(size) = size_failed(&error, "AMF", adapter) {
        return size;
    }
    error.context(format!(
        "AMF could not start on {adapter}. If Windows is at the sign-in screen or an RDP session was disconnected, sign in to and unlock the console session, then retry. Otherwise update the AMD graphics driver or select another codec on the device"
    ))
}

/// What a stream reports when NVENC does not start on `adapter`.
fn nvenc_failed(error: anyhow::Error, adapter: &str) -> anyhow::Error {
    if let Some(size) = size_failed(&error, "NVENC", adapter) {
        return size;
    }
    // The cause names the apps holding the sessions; a driver update or
    // signing in frees none.
    if crate::nvenc::session_limit(&error) {
        return error.context(format!(
            "NVENC could not start on {adapter}. Close the app recording or streaming with it, then retry"
        ));
    }
    error.context(format!(
        "NVENC could not start on {adapter}. If Windows is at the sign-in screen or an RDP session was disconnected, sign in to and unlock the console session, then retry. Otherwise update the NVIDIA graphics driver or select another codec on the device"
    ))
}

/// Rubylight encodes only on an AMD or an NVIDIA GPU. A device on another
/// vendor's GPU is refused in words, naming the GPU to use when the PC has
/// one, and never handed to an encoder to fail obscurely.
fn require_encoding_gpu(device: &crate::capture::Device) -> Result<()> {
    if encoder_gpu::encodes(device.vendor) {
        return Ok(());
    }
    require_encoding_among(
        &device.display.adapter,
        device.vendor,
        &crate::capture::gpus().unwrap_or_default(),
    )
}
fn require_encoding_among(name: &str, vendor: u32, gpus: &[crate::capture::Gpu]) -> Result<()> {
    let all = crate::capture::gpu_adapters(gpus);
    let adapter = encoder_gpu::Adapter {
        name: name.into(),
        vendor,
    };
    Ok(encoder_gpu::check_adapter(&adapter, &all)?)
}
/// AMF or NVENC for this device and the `encoder` setting
/// ([`butterpollo_core::encoder_policy::backend`]).
fn backend(
    device: &crate::capture::Device,
    tuning: &butterpollo_core::config::Config,
) -> Result<&'static str> {
    butterpollo_core::encoder_policy::backend(
        tuning.get("encoder", "auto"),
        device.vendor,
        &device.display.adapter,
    )
}

/// Preserve startup warnings when the selected encoder adopts the session's
/// shared reporter; future encoder warnings must use that same reporter too.
fn attach_warnings(current: &mut std::sync::Arc<Warnings>, reported: &std::sync::Arc<Warnings>) {
    for warning in current.snapshot() {
        reported.set(&warning.code, &warning.message);
    }
    *current = reported.clone();
}

pub enum Encoder {
    Amf(Box<crate::amf::Encoder>),
    Nvenc(Box<crate::nvenc::Encoder>),
    Pyrowave(Box<crate::pyrowave::Encoder>),
}
impl Encoder {
    /// Best-effort pace-based frame cap. PyroWave ignores it; AMF preserves an
    /// explicit user setting, including driver default. `floor` (two nominal
    /// frames) bounds how far AMF's keyframe cap scale lowers it. NVENC holds
    /// frames to its one-frame VBV and lowers that to a smaller cap.
    pub fn set_frame_cap(&mut self, bytes: u64, floor: u64) -> Option<u64> {
        match self {
            Self::Amf(encoder) => encoder.set_frame_cap(bytes, floor),
            Self::Nvenc(encoder) => encoder.set_frame_cap(bytes),
            Self::Pyrowave(_) => None,
        }
    }
    pub fn frame_cap(&self) -> Option<u64> {
        match self {
            Self::Amf(encoder) => encoder.frame_cap(),
            Self::Nvenc(encoder) => encoder.frame_cap(),
            Self::Pyrowave(_) => None,
        }
    }
    /// The largest inter frame sent as encoded; AMF and NVENC encode a larger
    /// one again as a keyframe. PyroWave holds every picture to its budget.
    pub fn set_cut_limit(&mut self, bytes: u64) {
        match self {
            Self::Amf(encoder) => encoder.set_cut_limit(bytes),
            Self::Nvenc(encoder) => encoder.set_cut_limit(bytes),
            Self::Pyrowave(_) => {}
        }
    }
    /// Inter frames encoded again as keyframes since the encoder opened.
    pub fn reencoded_cuts(&self) -> u64 {
        match self {
            Self::Amf(encoder) => encoder.reencoded_cuts(),
            Self::Nvenc(encoder) => encoder.reencoded_cuts(),
            Self::Pyrowave(_) => 0,
        }
    }
    pub fn keyframes_over_cap(&self) -> u64 {
        match self {
            Self::Amf(encoder) => encoder.keyframes_over_cap(),
            Self::Nvenc(encoder) => encoder.keyframes_over_cap(),
            Self::Pyrowave(_) => 0,
        }
    }
    /// How far under the pace-based target AMF's cap is written; 1 elsewhere:
    /// NVENC's VBV holds keyframes to the cap without one.
    pub fn keyframe_cap_scale(&self) -> f64 {
        match self {
            Self::Amf(encoder) => encoder.keyframe_cap_scale(),
            _ => 1.,
        }
    }
    pub fn set_hdr_metadata(&mut self, metadata: butterpollo_core::hdr::Metadata) {
        match self {
            Self::Amf(encoder) => encoder.set_hdr_metadata(metadata),
            Self::Nvenc(encoder) => encoder.set_hdr_metadata(metadata),
            Self::Pyrowave(_) => {}
        }
    }
    /// The receiver's display peak, for dynamic metadata that names a target
    /// display. PyroWave carries none.
    pub fn set_hdr_target(&mut self, nits: u16) {
        match self {
            Self::Amf(encoder) => encoder.set_hdr_target(nits),
            Self::Nvenc(encoder) => encoder.set_hdr_target(nits),
            Self::Pyrowave(_) => {}
        }
    }
    /// SDR white is absolute luminance; the scRGB scale expands a 1000-nit ceiling.
    pub fn set_luminance(&mut self, white_nits: f32, linear_scale: f32) {
        let luminance = [white_nits.clamp(100., 200.), linear_scale.clamp(1., 2.)];
        match self {
            Self::Amf(e) => e.luminance = luminance,
            Self::Nvenc(e) => e.set_luminance(luminance),
            Self::Pyrowave(e) => e.luminance = luminance,
        }
    }
    /// The next picture repeats the one last encoded. PyroWave sizes a new
    /// picture from the previous new one, so a repeat just before it does not
    /// shrink it to the few milliseconds since that repeat.
    pub fn set_repeat(&mut self, repeat: bool) {
        if let Self::Pyrowave(e) = self {
            e.repeat = repeat;
        }
    }
    pub fn new_gpu(config: &Negotiated, image: &GpuImage) -> Result<Self> {
        Self::new_gpu_options(config, image, &butterpollo_core::config::Config::default())
    }
    pub fn new_gpu_options(
        config: &Negotiated,
        image: &GpuImage,
        tuning: &butterpollo_core::config::Config,
    ) -> Result<Self> {
        Self::new_gpu_reported(config, image, tuning, &Default::default())
    }
    pub fn new_gpu_reported(
        config: &Negotiated,
        image: &GpuImage,
        tuning: &butterpollo_core::config::Config,
        warnings: &std::sync::Arc<Warnings>,
    ) -> Result<Self> {
        warnings.clear("encoder_conversion");
        warnings.clear("encoder_frame_size");
        // A replacement encoder establishes its own refresh policy. Discard
        // the previous AMF fallback before merging fresh startup warnings,
        // including when recovery selects a different codec.
        warnings.clear("encoder_intra_refresh");
        warnings.clear("hdr_dynamic_metadata");
        require_encoding_gpu(&image.gpu)?;
        if config.codec == 3 {
            if config.hdr {
                // The colour conversion (`gpu_color::constants`) encodes this curve.
                tracing::info!(
                    transfer = if config.hlg() { "HLG" } else { "PQ" },
                    "PyroWave HDR stream"
                );
            }
            if config.hdr && tuning.hdr_dynamic_metadata() {
                tracing::info!(
                    "PyroWave stream: no HDR dynamic metadata (HDR10+, HDR Vivid) is sent"
                );
            }
            return Ok(Self::Pyrowave(Box::new(
                crate::pyrowave::Encoder::new_device_reported(
                    config,
                    image.gpu.clone(),
                    tuning,
                    warnings.clone(),
                )?,
            )));
        }
        if backend(&image.gpu, tuning)? == "nvenc" {
            // As on AMF: convert on the compute queue and encode on its D3D12
            // device, or on the graphics queue when that is unavailable.
            let compute = if !config.yuv444
                && crate::compute::enabled(tuning)
                && crate::compute::shareable(&image.texture)
            {
                match crate::compute::Compute::for_device(&image.gpu.device) {
                    Ok(compute) => Some(compute),
                    Err(error) => {
                        warnings.set("encoder_conversion", format!("NVENC compute conversion unavailable ({error:#}); converting on the graphics queue. A busy game can delay frames; lower its GPU load or update the NVIDIA driver."));
                        None
                    }
                }
            } else {
                if !config.yuv444 && crate::compute::enabled(tuning) {
                    warnings.set("encoder_conversion", "NVENC compute conversion unavailable: the capture texture cannot be shared. Converting on the graphics queue; lower game GPU load or update the NVIDIA driver if frames are delayed.");
                }
                None
            };
            let created = match crate::nvenc::Encoder::new_gpu(
                config,
                image.gpu.clone(),
                tuning,
                warnings,
                compute.clone(),
            ) {
                // Past the session limit the graphics queue's device fails
                // the same way, so it is not tried.
                Err(error) if compute.is_some() && !crate::nvenc::session_limit(&error) => {
                    warnings.set("encoder_conversion", format!("NVENC on the compute queue's D3D12 device failed ({error:#}); converting on the graphics queue. Lower game GPU load or update the NVIDIA driver (522.25 or newer) if frames are delayed."));
                    crate::nvenc::Encoder::new_gpu(
                        config,
                        image.gpu.clone(),
                        tuning,
                        warnings,
                        None,
                    )
                }
                created => created,
            };
            return match created {
                Ok(encoder) => Ok(Self::Nvenc(Box::new(encoder))),
                Err(error) => Err(nvenc_failed(error, &image.gpu.display.adapter)),
            };
        }
        // Colour conversion on a compute queue keeps running beside a
        // game that fills the GPU's graphics queue.
        // AMF rejects 4:4:4 regardless of which queue converts the input.
        let compute = if !config.yuv444
            && crate::compute::enabled(tuning)
            && crate::compute::shareable(&image.texture)
        {
            match crate::compute::Compute::for_device(&image.gpu.device) {
                Ok(compute) => Some(compute),
                Err(error) => {
                    warnings.set("encoder_conversion", format!("AMF compute conversion unavailable ({error:#}); converting on the graphics queue. A busy game can delay frames; lower its GPU load or update the AMD driver."));
                    None
                }
            }
        } else {
            if !config.yuv444 && crate::compute::enabled(tuning) {
                warnings.set("encoder_conversion", "AMF compute conversion unavailable: the capture texture cannot be shared. Converting on the graphics queue; lower game GPU load or update the AMD driver if frames are delayed.");
            }
            None
        };
        let created = match crate::amf::Encoder::new_gpu(
            config,
            image.gpu.clone(),
            tuning,
            compute.clone(),
        ) {
            Err(error) if compute.is_some() && retries_on_graphics(&error) => {
                warnings.set("encoder_conversion", format!("AMF compute queue initialization failed ({error:#}); retrying conversion on the graphics queue. Lower game GPU load or update the AMD driver if frames are delayed."));
                crate::amf::Encoder::new_gpu(config, image.gpu.clone(), tuning, None)
            }
            created => created,
        };
        match created {
            Ok(mut encoder) => {
                attach_warnings(&mut encoder.warnings, warnings);
                Ok(Self::Amf(Box::new(encoder)))
            }
            Err(error) => {
                warnings.clear("encoder_conversion");
                Err(amf_failed(error, &image.gpu.display.adapter))
            }
        }
    }
    pub fn encode_gpu(
        &mut self,
        image: &GpuImage,
        idr: bool,
        bitrate: u32,
    ) -> Result<Vec<Encoded>> {
        match self {
            Self::Amf(e) => e.encode_gpu(image, idr, bitrate),
            Self::Nvenc(e) => e.encode_gpu(image, idr, bitrate),
            Self::Pyrowave(e) => e.encode_gpu(image, idr, bitrate),
        }
    }
    pub fn accepts_gpu_device(&self, image: &GpuImage) -> bool {
        match self {
            Self::Amf(e) => e.accepts_gpu_device(image),
            Self::Nvenc(e) => e.accepts_gpu_device(image),
            Self::Pyrowave(e) => e.accepts_gpu_device(image),
        }
    }
    pub fn pending(&self) -> bool {
        match self {
            Self::Amf(e) => e.pending(),
            Self::Nvenc(e) => e.pending(),
            Self::Pyrowave(e) => e.pending(),
        }
    }
    /// An event that ends a wait for output as soon as the oldest frame is
    /// done (`nvenc_output_event`, `amd_output_thread`), armed by this call. `None` keeps the
    /// caller's output poll; with an event that poll stays the wait's bound.
    pub fn output_event(&self) -> Option<&crate::timing::Signal> {
        match self {
            Self::Amf(e) => e.output_event(),
            Self::Nvenc(e) => e.output_event(),
            Self::Pyrowave(e) => e.output_event(),
        }
    }
    /// A poll can wait in the driver: AMF's QueryOutput blocks for its
    /// timeout unless `amd_output_thread` queries; the others return at once.
    pub fn poll_blocks(&self) -> bool {
        match self {
            Self::Amf(e) => e.poll_blocks(),
            Self::Nvenc(_) | Self::Pyrowave(_) => false,
        }
    }
    /// The earliest a poll can find output (`amd_output_prediction`); the
    /// caller's next output poll waits for it. `None` polls as before.
    pub fn output_due(&self) -> Option<std::time::Instant> {
        match self {
            Self::Amf(e) => e.output_due(),
            Self::Nvenc(_) | Self::Pyrowave(_) => None,
        }
    }
    /// When the encoder accepted the input of the last encode call, after
    /// its colour conversion.
    pub fn last_submitted(&self) -> Option<std::time::Instant> {
        match self {
            Self::Amf(e) => e.last_submitted(),
            Self::Nvenc(e) => e.last_submitted(),
            Self::Pyrowave(e) => e.last_submitted(),
        }
    }
    /// Frames the GPU encoder has accepted and not yet finished.
    pub fn backlog(&self) -> usize {
        match self {
            Self::Amf(e) => e.backlog(),
            Self::Nvenc(e) => e.backlog(),
            Self::Pyrowave(e) => e.backlog(),
        }
    }
    pub fn backlog_limit(&self) -> usize {
        match self {
            Self::Amf(e) => e.backlog_limit(),
            // PyroWave holds at most two frames in flight, the default limit:
            // with `pyrowave_encode_overlap` off its backlog stays empty.
            Self::Nvenc(_) | Self::Pyrowave(_) => butterpollo_core::encoder_backlog::DEFAULT_LIMIT,
        }
    }
    pub fn recovery_backlog_limit(&self) -> usize {
        match self {
            Self::Amf(e) => e.recovery_backlog_limit(),
            _ => self.backlog_limit(),
        }
    }
    pub fn recovery_lead(&self) -> Option<std::time::Duration> {
        match self {
            Self::Amf(e) => e.recovery_lead(),
            _ => None,
        }
    }
    pub fn log_stall(&self) {
        match self {
            Self::Amf(encoder) => encoder.log_stall(),
            Self::Nvenc(encoder) => encoder.log_stall(),
            Self::Pyrowave(_) => {}
        }
    }
    pub fn device_removed(&self) -> Option<crate::device_loss::DeviceLost> {
        match self {
            Self::Amf(encoder) => encoder.device_removed(),
            Self::Nvenc(encoder) => encoder.device_removed(),
            Self::Pyrowave(encoder) => encoder.device_removed(),
        }
    }
    /// The GPU an AMF encoder opened on, for wording what it cannot do.
    pub fn adapter(&self) -> Option<&str> {
        match self {
            Self::Amf(encoder) => Some(encoder.adapter()),
            Self::Nvenc(encoder) => Some(encoder.adapter()),
            Self::Pyrowave(_) => None,
        }
    }
    /// An integer capability of the AMF encoder, such as its throughput limit.
    pub fn capability(&self, name: &str) -> Option<i64> {
        match self {
            Self::Amf(encoder) => encoder.capability(name),
            Self::Nvenc(encoder) => encoder.capability(name),
            Self::Pyrowave(_) => None,
        }
    }
    /// The encoder family, as the encoder setting names it.
    pub fn backend(&self) -> &'static str {
        match self {
            Self::Amf(_) => "amf",
            Self::Nvenc(_) => "nvenc",
            Self::Pyrowave(_) => "pyrowave",
        }
    }
    pub fn supports_invalidation(&self) -> bool {
        match self {
            Self::Amf(e) => e.supports_invalidation(),
            Self::Nvenc(e) => e.supports_invalidation(),
            Self::Pyrowave(_) => false,
        }
    }
    pub fn invalidate_ref_frames(&mut self, first: u64, last: u64) -> bool {
        match self {
            Self::Amf(encoder) => encoder.invalidate_ref_frames(first, last),
            Self::Nvenc(encoder) => encoder.invalidate_ref_frames(first, last),
            Self::Pyrowave(_) => false,
        }
    }
    pub fn needs_recovery_confirmation(&self) -> bool {
        matches!(self, Self::Amf(encoder) if encoder.invalidation_pending())
    }
    pub fn poll(&mut self) -> Result<Vec<Encoded>> {
        match self {
            Self::Amf(e) => e.poll(),
            Self::Nvenc(e) => e.poll(),
            Self::Pyrowave(e) => e.poll(),
        }
    }
    /// Waits for every frame still in flight and returns them, for a caller
    /// that wants each picture before it goes on, such as a fixture or a
    /// benchmark. PyroWave only: AMF and NVENC keep their output for
    /// [`Self::poll`].
    pub fn finish(&mut self) -> Result<Vec<Encoded>> {
        match self {
            Self::Amf(_) | Self::Nvenc(_) => Ok(vec![]),
            Self::Pyrowave(e) => e.finish(),
        }
    }
    /// Blocks a P-frame is sent in as its slices leave the encoder; 0 when
    /// `video_slice_delivery` is off, for AV1 and PyroWave, on an NVENC
    /// driver without subframe readback, and after [`Self::join_slices`].
    pub fn slice_delivery(&self) -> u8 {
        match self {
            Self::Amf(e) => e.slice_delivery(),
            Self::Nvenc(e) => e.slice_delivery(),
            Self::Pyrowave(_) => 0,
        }
    }
    /// Slice delivery is off for the rest of the session: later frames come
    /// back whole.
    pub fn join_slices(&mut self) {
        match self {
            Self::Amf(e) => e.join_slices(),
            Self::Nvenc(e) => e.join_slices(),
            Self::Pyrowave(_) => {}
        }
    }
    /// A recreated encoder must keep the wire frame numbers used by RFI.
    pub fn set_next_frame(&mut self, frame: u64) {
        match self {
            Self::Amf(e) => e.set_next_frame(frame),
            Self::Nvenc(e) => e.set_next_frame(frame),
            Self::Pyrowave(_) => {}
        }
    }
    pub fn new(config: &Negotiated, display: &str) -> Result<Self> {
        Self::new_options(
            config,
            display,
            &butterpollo_core::config::Config::default(),
        )
    }
    /// An encoder for CPU images, such as the codec probe's.
    pub fn new_options(
        config: &Negotiated,
        display: &str,
        tuning: &butterpollo_core::config::Config,
    ) -> Result<Self> {
        // The GPU the encoding adapter setting names, as capture opens it;
        // the display's own GPU only when nothing is set. A Radeon beside
        // another card that drives the monitor must probe on the Radeon.
        let device = crate::capture::Device::new_adapter(
            display,
            tuning.get("adapter_name", ""),
            tuning.get("adapter_pnp_id", ""),
        )
        .context("the encoder could not open the configured GPU. If Windows is at the sign-in screen or an RDP session was disconnected, sign in to and unlock the console session, then retry. Otherwise check the adapter setting")?;
        require_encoding_gpu(&device)?;
        if config.codec == 3 {
            return Ok(Self::Pyrowave(Box::new(
                crate::pyrowave::Encoder::new_device(config, device, tuning)?,
            )));
        }
        let adapter = device.display.adapter.clone();
        if backend(&device, tuning)? == "nvenc" {
            return crate::nvenc::Encoder::new_device_options(config, device, tuning)
                .map(|encoder| Self::Nvenc(Box::new(encoder)))
                .map_err(|error| nvenc_failed(error, &adapter));
        }
        crate::amf::Encoder::new_device_options(config, device, tuning)
            .map(|mut encoder| {
                encoder.probe = true;
                Self::Amf(Box::new(encoder))
            })
            .map_err(|error| amf_failed(error, &adapter))
    }
    pub fn encode(&mut self, image: &Image, idr: bool, bitrate: u32) -> Result<Vec<Encoded>> {
        match self {
            Self::Amf(e) => e.encode(image, idr, bitrate),
            Self::Nvenc(e) => e.encode(image, idr, bitrate),
            Self::Pyrowave(e) => e.encode(image, idr, bitrate),
        }
    }
}
// The couch menu's hook, apart from the rest so the encoder's other features
// can change beside it.
impl Encoder {
    /// Draw the couch menu's panel into the next pictures, or stop with
    /// `None` (docs/couch-menu-design.md 6.1). Only GPU frames get it: a CPU
    /// [`Encoder::encode`] fails while it is set.
    pub fn set_overlay(&mut self, overlay: Option<crate::overlay::Overlay>) {
        match self {
            Self::Amf(e) => e.overlay = overlay,
            Self::Nvenc(e) => e.overlay = overlay,
            Self::Pyrowave(e) => e.overlay = overlay,
        }
    }
    /// Whether a panel is set.
    pub fn has_overlay(&self) -> bool {
        match self {
            Self::Amf(e) => e.overlay.is_some(),
            Self::Nvenc(e) => e.overlay.is_some(),
            Self::Pyrowave(e) => e.overlay.is_some(),
        }
    }
}
// Encoder focus (docs/encoder-focus-design.md): AMF's ROI map and NVENC's
// QP delta map. PyroWave has none: it refuses a map and reports
// `Reason::Pyrowave`.
impl Encoder {
    /// Why PyroWave has no map.
    fn no_focus(&self) -> encoder_focus::Reason {
        encoder_focus::Reason::Pyrowave
    }
    /// The map's settings for the next frames, or `None` for no map.
    pub fn set_focus(&mut self, settings: Option<crate::compute::FocusSettings>) -> Result<()> {
        match self {
            Self::Amf(e) => e.set_focus(settings),
            Self::Nvenc(e) => e.set_focus(settings),
            other if settings.is_some() => Err(anyhow::anyhow!(
                "encoder focus: {}",
                other.no_focus().message()
            )),
            _ => Ok(()),
        }
    }
    pub fn set_focus_verify(&mut self, every: Option<u32>) {
        match self {
            Self::Amf(e) => e.set_focus_verify(every),
            Self::Nvenc(e) => e.set_focus_verify(every),
            Self::Pyrowave(_) => {}
        }
    }
    pub fn focus_stats(&self) -> std::result::Result<encoder_focus::Stats, encoder_focus::Reason> {
        match self {
            Self::Amf(e) => Ok(e.focus_stats()),
            Self::Nvenc(e) => Ok(e.focus_stats()),
            other => Err(other.no_focus()),
        }
    }
    /// Why a map failed on a frame since the last call. The encoder has
    /// turned focus off; the stream keeps it off and says so.
    pub fn take_focus_failure(&mut self) -> Option<anyhow::Error> {
        match self {
            Self::Amf(e) => e.take_focus_failure(),
            Self::Nvenc(e) => e.take_focus_failure(),
            Self::Pyrowave(_) => None,
        }
    }
    /// The startup check (design section 7). An error is the encoder's.
    pub fn probe_focus(&mut self) -> Result<encoder_focus::Support> {
        match self {
            Self::Amf(e) => Ok(e.probe_focus()?.support),
            Self::Nvenc(e) => Ok(e.probe_focus()?.support),
            other => Ok(encoder_focus::Support::Unsupported {
                reason: other.no_focus(),
                message: other.no_focus().message().into(),
            }),
        }
    }
}
/// The visible cursor's centre in `target` pixels, for `centre` focus.
pub fn focus_cursor(image: &GpuImage, (width, height): (u32, u32)) -> Option<(i32, i32)> {
    let cursor = image.cursor.as_ref()?;
    let scale = |position: i32, size: u32, source: u32, target: u32| {
        ((i64::from(position) + i64::from(size / 2)) * i64::from(target) / i64::from(source.max(1)))
            as i32
    };
    Some((
        scale(cursor.position[0], cursor.width, image.width, width),
        scale(cursor.position[1], cursor.height, image.height, height),
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn amf_failure_names_the_adapter_and_console_recovery() {
        let message = format!(
            "{:#}",
            amf_failed(
                anyhow::anyhow!("AMD AMF runtime unavailable"),
                "Microsoft Remote Display Adapter",
            )
        );
        assert!(
            message.starts_with("AMF could not start on Microsoft Remote Display Adapter"),
            "{message}"
        );
        assert!(message.contains("sign-in screen"));
        assert!(message.contains("RDP session was disconnected"));
        assert!(message.contains("unlock the console session"));
        assert!(message.contains("update the AMD graphics driver"));
        assert!(
            message.ends_with("AMD AMF runtime unavailable"),
            "{message}"
        );
    }
    #[test]
    fn a_size_the_encoder_cannot_take_says_so_and_not_to_update_the_driver() {
        let size = SizeTooLarge {
            encoder: "AMF",
            width: 8192,
            height: 8192,
            max: Some((4096, 2176)),
        };
        let message = format!(
            "{:#}",
            amf_failed(
                anyhow::Error::new(size.clone()).context("AMF encoder Init"),
                "AMD Radeon RX 7900 XT"
            )
        );
        assert_eq!(
            message,
            "AMF could not start on AMD Radeon RX 7900 XT: 8192x8192 is larger than AMF on this GPU can encode (at most 4096x2176); choose a lower resolution on the device"
        );
        let message = format!(
            "{:#}",
            nvenc_failed(
                anyhow::Error::new(SizeTooLarge {
                    encoder: "NVENC",
                    max: None,
                    ..size
                }),
                "NVIDIA GeForce RTX 4070"
            )
        );
        assert_eq!(
            message,
            "NVENC could not start on NVIDIA GeForce RTX 4070: 8192x8192 is larger than NVENC on this GPU can encode; choose a lower resolution on the device"
        );
        for advice in ["driver", "sign-in", "unlock"] {
            assert!(!message.contains(advice), "{message}");
        }
    }
    #[test]
    fn a_size_the_encoder_cannot_take_is_not_retried_on_the_graphics_queue() {
        let size = SizeTooLarge {
            encoder: "AMF",
            width: 8192,
            height: 8192,
            max: Some((8192, 4352)),
        };
        assert!(!retries_on_graphics(
            &anyhow::Error::new(size).context("AMF encoder Init")
        ));
        assert!(retries_on_graphics(&anyhow::anyhow!(
            "AMF error AMF_FAIL (1)"
        )));
    }
    fn gpu(name: &str, vendor: u32) -> crate::capture::Gpu {
        crate::capture::Gpu {
            name: name.into(),
            vendor,
            dedicated_memory: 0,
            luid: (0, 0),
            pnp_id: None,
        }
    }
    #[test]
    fn nvenc_failure_names_the_adapter_and_the_nvidia_driver() {
        let message = format!(
            "{:#}",
            nvenc_failed(
                anyhow::anyhow!("NVENC session failed"),
                "NVIDIA GeForce RTX 4070",
            )
        );
        assert!(
            message.starts_with("NVENC could not start on NVIDIA GeForce RTX 4070"),
            "{message}"
        );
        assert!(message.contains("unlock the console session"));
        assert!(message.contains("update the NVIDIA graphics driver"));
        assert!(!message.contains("AMD"), "{message}");
        assert!(message.ends_with("NVENC session failed"), "{message}");
    }
    #[test]
    fn an_nvenc_session_limit_names_the_apps_not_the_driver() {
        let message = format!(
            "{:#}",
            nvenc_failed(
                anyhow::anyhow!("NVENC open session failed (10)")
                    .context(crate::nvenc::SessionLimit { memory: false }),
                "NVIDIA GeForce RTX 4070",
            )
        );
        assert!(
            message.starts_with("NVENC could not start on NVIDIA GeForce RTX 4070"),
            "{message}"
        );
        assert!(message.contains("NVENC session limit reached"), "{message}");
        assert!(message.contains("OBS, ShadowPlay or Discord"), "{message}");
        assert!(!message.contains("driver"), "{message}");
        assert!(!message.contains("sign in"), "{message}");
    }
    #[test]
    fn a_stream_on_an_intel_gpu_is_refused_in_words() {
        let all = [gpu("Intel(R) Arc(TM) A770", 0x8086)];
        let error = require_encoding_among(&all[0].name, all[0].vendor, &all).unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("this PC has only Intel(R) Arc(TM) A770"),
            "{message}"
        );
        assert!(message.contains("stay on Rubylight 2.6.x"), "{message}");
        assert!(message.contains("/releases/tag/v2.6.0"), "{message}");
        // It never reaches an encoder, whose failure text would blame the driver.
        assert!(!message.contains("could not start"), "{message}");
    }
    #[test]
    fn amd_and_nvidia_gpus_encode_and_an_intel_display_names_the_one_that_can() {
        let all = [
            gpu("Intel(R) UHD Graphics 770", 0x8086),
            gpu("NVIDIA GeForce RTX 4070", 0x10de),
            gpu("AMD Radeon(TM) Graphics", 0x1002),
        ];
        assert!(require_encoding_among(&all[1].name, 0x10de, &all).is_ok());
        assert!(require_encoding_among(&all[2].name, 0x1002, &all).is_ok());
        let message = format!(
            "{:#}",
            require_encoding_among(&all[0].name, 0x8086, &all).unwrap_err()
        );
        assert!(
            message.starts_with(
                "Intel(R) UHD Graphics 770 is neither an AMD nor an NVIDIA graphics card"
            ),
            "{message}"
        );
        assert!(
            message.contains("also has NVIDIA GeForce RTX 4070"),
            "{message}"
        );
    }
    #[test]
    fn startup_warnings_survive_attaching_the_session_reporter() {
        let mut encoder = std::sync::Arc::new(Warnings::default());
        encoder.set(
            "encoder_frame_size",
            "driver cannot apply the requested cap",
        );
        let session = std::sync::Arc::new(Warnings::default());
        session.set("encoder_conversion", "retained conversion warning");
        attach_warnings(&mut encoder, &session);
        assert!(std::sync::Arc::ptr_eq(&encoder, &session));
        assert!(session.snapshot().iter().any(|warning| {
            warning.code == "encoder_frame_size"
                && warning.message == "driver cannot apply the requested cap"
        }));
        assert!(
            session
                .snapshot()
                .iter()
                .any(|warning| warning.code == "encoder_conversion")
        );
        encoder.set("encoder_buffer", "later rate-control warning");
        assert!(
            session
                .snapshot()
                .iter()
                .any(|warning| warning.code == "encoder_buffer")
        );
    }
    /// Enumerates native adapters before rejecting a nonexistent GPU.
    #[test]
    #[ignore = "requires an interactive desktop display and a hardware GPU"]
    fn encoders_open_on_the_configured_gpu() {
        let stream = Negotiated {
            width: 640,
            height: 480,
            fps: 30,
            bitrate_kbps: 2000,
            ..Default::default()
        };
        // A GPU that is not there: the encoder must not quietly use the
        // display's GPU instead, as the codec probe did with two GPUs.
        let tuning =
            butterpollo_core::config::Config::parse("adapter_name = No Such GPU\n").unwrap();
        let error = Encoder::new_options(&stream, "", &tuning)
            .err()
            .expect("an encoder on a missing GPU");
        assert!(format!("{error:#}").contains("No Such GPU"), "{error:#}");
    }
}
