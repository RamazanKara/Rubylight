//! The video encoder of a stream.
//!
//! [`Encoder`] is AMF for H.264, HEVC and AV1 and PyroWave's own encoder for
//! PyroWave streams. There is no other encoder to fall back to: when AMF
//! cannot start, that is the error the stream reports.

use crate::capture::{GpuImage, Image};
use anyhow::{Context, Result};
use butterpollo_core::{amd_gpu, encoder_focus, rtsp::Negotiated, session::Warnings};

/// How `Encoded::bytes` relates to a picture. Only AMF in slice output mode
/// (`video_slice_delivery`) returns anything but `Whole`: a P-frame then comes
/// as `Slice` units in order, ending with `Last`, all of one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Whole,
    Slice { index: u32 },
    Last { index: u32 },
}

pub struct Encoded {
    pub bytes: Vec<u8>,
    pub unit: Unit,
    pub idr: bool,
    pub after_invalidation: bool,
    /// Driver-read AMF frame limit at submission. None means unavailable or
    /// unsupported; Some(0) means the driver's unrestricted/default setting.
    pub frame_cap_bytes: Option<u64>,
    /// AMF encoded this keyframe again in place of an oversize scene cut.
    pub reencoded: bool,
    /// The pace-based budget the host asked AMF to hold this frame to (the
    /// last `set_frame_cap`), against which a keyframe's size is judged.
    pub frame_cap_target: Option<u64>,
    /// Submission through completed codec output, including asynchronous work.
    pub latency: Option<std::time::Duration>,
    pub presentation: Option<std::time::Instant>,
}

/// What a stream reports when AMF does not start on `adapter`.
fn amf_failed(error: anyhow::Error, adapter: &str) -> anyhow::Error {
    error.context(format!(
        "AMF could not start on {adapter}. If Windows is at the sign-in screen or an RDP session was disconnected, sign in to and unlock the console session, then retry. Otherwise update the AMD graphics driver or select another codec on the device"
    ))
}

/// Rubylight encodes only on an AMD GPU. A device on another vendor's GPU is
/// refused in words, naming the AMD GPU to use when the PC has one, and never
/// handed to AMF or PyroWave to fail obscurely.
fn require_amd(device: &crate::capture::Device) -> Result<()> {
    if amd_gpu::is_amd(device.vendor) {
        return Ok(());
    }
    require_amd_among(
        &device.display.adapter,
        device.vendor,
        &crate::capture::gpus().unwrap_or_default(),
    )
}
fn require_amd_among(name: &str, vendor: u32, gpus: &[crate::capture::Gpu]) -> Result<()> {
    let all = crate::capture::amd_adapters(gpus);
    let adapter = amd_gpu::Adapter {
        name: name.into(),
        vendor,
    };
    Ok(amd_gpu::check_adapter(&adapter, &all)?)
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
    Pyrowave(Box<crate::pyrowave::Encoder>),
}
impl Encoder {
    /// Best-effort pace-based frame cap. Unsupported backends safely ignore
    /// it; AMF preserves an explicit user setting, including driver default.
    pub fn set_frame_cap(&mut self, bytes: u64) -> Option<u64> {
        match self {
            Self::Amf(encoder) => encoder.set_frame_cap(bytes),
            _ => None,
        }
    }
    pub fn frame_cap(&self) -> Option<u64> {
        match self {
            Self::Amf(encoder) => encoder.frame_cap(),
            _ => None,
        }
    }
    /// The largest inter frame sent as encoded; AMF encodes a larger one again
    /// as a keyframe. PyroWave holds every picture to its budget.
    pub fn set_cut_limit(&mut self, bytes: u64) {
        if let Self::Amf(encoder) = self {
            encoder.set_cut_limit(bytes);
        }
    }
    /// Inter frames encoded again as keyframes since the encoder opened.
    pub fn reencoded_cuts(&self) -> u64 {
        match self {
            Self::Amf(encoder) => encoder.reencoded_cuts(),
            _ => 0,
        }
    }
    pub fn keyframes_over_cap(&self) -> u64 {
        match self {
            Self::Amf(encoder) => encoder.keyframes_over_cap(),
            _ => 0,
        }
    }
    pub fn set_hdr_metadata(&mut self, metadata: butterpollo_core::hdr::Metadata) {
        if let Self::Amf(encoder) = self {
            encoder.set_hdr_metadata(metadata);
        }
    }
    /// The receiver's display peak, for dynamic metadata that names a target
    /// display. PyroWave carries none.
    pub fn set_hdr_target(&mut self, nits: u16) {
        if let Self::Amf(encoder) = self {
            encoder.set_hdr_target(nits);
        }
    }
    /// SDR white is absolute luminance; the scRGB scale expands a 1000-nit ceiling.
    pub fn set_luminance(&mut self, white_nits: f32, linear_scale: f32) {
        let luminance = [white_nits.clamp(100., 200.), linear_scale.clamp(1., 2.)];
        match self {
            Self::Amf(e) => e.luminance = luminance,
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
        require_amd(&image.gpu)?;
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
            Err(error) if compute.is_some() => {
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
            Self::Pyrowave(e) => e.encode_gpu(image, idr, bitrate),
        }
    }
    pub fn accepts_gpu_device(&self, image: &GpuImage) -> bool {
        match self {
            Self::Amf(e) => e.accepts_gpu_device(image),
            Self::Pyrowave(e) => e.accepts_gpu_device(image),
        }
    }
    pub fn pending(&self) -> bool {
        match self {
            Self::Amf(e) => e.pending(),
            Self::Pyrowave(_) => false,
        }
    }
    /// When the encoder accepted the input of the last encode call, after
    /// its colour conversion. Only AMF reports it.
    pub fn last_submitted(&self) -> Option<std::time::Instant> {
        match self {
            Self::Amf(e) => e.last_submitted(),
            Self::Pyrowave(_) => None,
        }
    }
    /// Frames the GPU encoder has accepted and not yet finished.
    pub fn backlog(&self) -> usize {
        match self {
            Self::Amf(e) => e.backlog(),
            Self::Pyrowave(_) => 0,
        }
    }
    pub fn backlog_limit(&self) -> usize {
        match self {
            Self::Amf(e) => e.backlog_limit(),
            Self::Pyrowave(_) => butterpollo_core::encoder_backlog::DEFAULT_LIMIT,
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
        if let Self::Amf(encoder) = self {
            encoder.log_stall();
        }
    }
    pub fn device_removed(&self) -> Option<crate::device_loss::DeviceLost> {
        match self {
            Self::Amf(encoder) => encoder.device_removed(),
            Self::Pyrowave(encoder) => encoder.device_removed(),
        }
    }
    /// The GPU an AMF encoder opened on, for wording what it cannot do.
    pub fn adapter(&self) -> Option<&str> {
        match self {
            Self::Amf(encoder) => Some(encoder.adapter()),
            Self::Pyrowave(_) => None,
        }
    }
    /// An integer capability of the AMF encoder, such as its throughput limit.
    pub fn capability(&self, name: &str) -> Option<i64> {
        match self {
            Self::Amf(encoder) => encoder.capability(name),
            Self::Pyrowave(_) => None,
        }
    }
    /// The encoder family, as the encoder setting names it.
    pub fn backend(&self) -> &'static str {
        match self {
            Self::Amf(_) => "amf",
            Self::Pyrowave(_) => "pyrowave",
        }
    }
    pub fn supports_invalidation(&self) -> bool {
        match self {
            Self::Amf(e) => e.supports_invalidation(),
            Self::Pyrowave(_) => false,
        }
    }
    pub fn invalidate_ref_frames(&mut self, first: u64, last: u64) -> bool {
        match self {
            Self::Amf(encoder) => encoder.invalidate_ref_frames(first, last),
            Self::Pyrowave(_) => false,
        }
    }
    pub fn needs_recovery_confirmation(&self) -> bool {
        matches!(self, Self::Amf(encoder) if encoder.invalidation_pending())
    }
    pub fn poll(&mut self) -> Result<Vec<Encoded>> {
        match self {
            Self::Amf(e) => e.poll(),
            Self::Pyrowave(_) => Ok(vec![]),
        }
    }
    /// Blocks a P-frame is sent in as its slices leave the encoder; 0 when
    /// `video_slice_delivery` is off, for AV1 and PyroWave, and after
    /// [`Self::join_slices`].
    pub fn slice_delivery(&self) -> u8 {
        match self {
            Self::Amf(e) => e.slice_delivery(),
            Self::Pyrowave(_) => 0,
        }
    }
    /// Slice delivery is off for the rest of the session: later frames come
    /// back whole.
    pub fn join_slices(&mut self) {
        if let Self::Amf(e) = self {
            e.join_slices();
        }
    }
    /// A recreated encoder must keep the wire frame numbers used by RFI.
    pub fn set_next_frame(&mut self, frame: u64) {
        if let Self::Amf(e) = self {
            e.set_next_frame(frame);
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
        require_amd(&device)?;
        if config.codec == 3 {
            return Ok(Self::Pyrowave(Box::new(
                crate::pyrowave::Encoder::new_device(config, device, tuning)?,
            )));
        }
        let adapter = device.display.adapter.clone();
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
            Self::Pyrowave(e) => e.overlay = overlay,
        }
    }
    /// Whether a panel is set.
    pub fn has_overlay(&self) -> bool {
        match self {
            Self::Amf(e) => e.overlay.is_some(),
            Self::Pyrowave(e) => e.overlay.is_some(),
        }
    }
}
// Encoder focus (docs/encoder-focus-design.md): AMF's ROI map. PyroWave has
// none: it refuses a map and reports `Reason::Pyrowave`.
impl Encoder {
    /// The map's settings for the next frames, or `None` for no map.
    pub fn set_focus(&mut self, settings: Option<crate::compute::FocusSettings>) -> Result<()> {
        match self {
            Self::Amf(e) => e.set_focus(settings),
            Self::Pyrowave(_) if settings.is_some() => Err(anyhow::anyhow!(
                "encoder focus: {}",
                encoder_focus::Reason::Pyrowave.message()
            )),
            Self::Pyrowave(_) => Ok(()),
        }
    }
    pub fn set_focus_verify(&mut self, every: Option<u32>) {
        if let Self::Amf(e) = self {
            e.set_focus_verify(every);
        }
    }
    pub fn focus_stats(&self) -> std::result::Result<encoder_focus::Stats, encoder_focus::Reason> {
        match self {
            Self::Amf(e) => Ok(e.focus_stats()),
            Self::Pyrowave(_) => Err(encoder_focus::Reason::Pyrowave),
        }
    }
    /// Why a map failed on a frame since the last call. The encoder has
    /// turned focus off; the stream keeps it off and says so.
    pub fn take_focus_failure(&mut self) -> Option<anyhow::Error> {
        match self {
            Self::Amf(e) => e.take_focus_failure(),
            Self::Pyrowave(_) => None,
        }
    }
    /// The startup check (design section 7). An error is the encoder's.
    pub fn probe_focus(&mut self) -> Result<encoder_focus::Support> {
        match self {
            Self::Amf(e) => Ok(e.probe_focus()?.support),
            Self::Pyrowave(_) => Ok(encoder_focus::Support::Unsupported {
                reason: encoder_focus::Reason::Pyrowave,
                message: encoder_focus::Reason::Pyrowave.message().into(),
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
    fn a_stream_on_a_non_amd_gpu_is_refused_in_words() {
        let all = [gpu("NVIDIA GeForce RTX 4070", 0x10de)];
        let error = require_amd_among(&all[0].name, all[0].vendor, &all).unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("this PC has only NVIDIA GeForce RTX 4070"),
            "{message}"
        );
        assert!(message.contains("stay on Rubylight 2.6.x"), "{message}");
        assert!(message.contains("/releases/tag/v2.6.0"), "{message}");
        // It never reaches AMF, whose failure text would blame the driver.
        assert!(!message.contains("AMF could not start"), "{message}");
    }
    #[test]
    fn an_amd_igpu_beside_an_nvidia_dgpu_is_named_when_the_display_is_on_the_nvidia() {
        let all = [
            gpu("NVIDIA GeForce RTX 4070", 0x10de),
            gpu("AMD Radeon(TM) Graphics", 0x1002),
        ];
        let message = format!(
            "{:#}",
            require_amd_among(&all[0].name, 0x10de, &all).unwrap_err()
        );
        assert!(
            message.starts_with("NVIDIA GeForce RTX 4070 is not an AMD graphics card"),
            "{message}"
        );
        assert!(
            message.contains("also has AMD Radeon(TM) Graphics"),
            "{message}"
        );
        assert!(require_amd_among(&all[1].name, 0x1002, &all).is_ok());
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
