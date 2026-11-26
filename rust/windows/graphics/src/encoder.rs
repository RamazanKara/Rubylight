//! The video encoder of a stream.
//!
//! [`Encoder`] is AMF on an AMD GPU and NVENC on an NVIDIA GPU for H.264,
//! HEVC and AV1, and PyroWave's own encoder for PyroWave streams. There is no
//! other encoder to fall back to: when the GPU's encoder cannot start, that
//! is the error the stream reports.

use crate::capture::{GpuImage, Image};
use anyhow::{Context, Result};
use butterpollo_media_policy::{WarningSink, encoder_focus, recorded_warnings, rtsp::Negotiated};

mod types;
#[cfg(test)]
use types::require_encoding_among;
pub use types::{Encoded, SizeTooLarge, Unit, focus_cursor};
use types::{
    amf_failed, attach_warnings, backend, nvenc_failed, require_encoding_gpu, retries_on_graphics,
};

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
    pub fn set_hdr_metadata(&mut self, metadata: butterpollo_media_policy::hdr::Metadata) {
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
    /// Clamp encoder `[white_nits, linear_scale]` allowing Windows' desktop HDR
    /// SDR content brightness range (`80..=1000` nits) alongside RTX HDR.
    fn clamp_luminance(white_nits: f32, linear_scale: f32) -> [f32; 2] {
        let white_nits = if white_nits.is_finite() {
            white_nits.clamp(80., 1000.)
        } else {
            100.
        };
        let linear_scale = if linear_scale.is_finite() {
            linear_scale.clamp(1., 2.)
        } else {
            1.
        };
        [white_nits, linear_scale]
    }
    /// SDR white is absolute luminance; the scRGB scale expands a 1000-nit ceiling.
    pub fn set_luminance(&mut self, white_nits: f32, linear_scale: f32) {
        let luminance = Self::clamp_luminance(white_nits, linear_scale);
        match self {
            Self::Amf(e) => e.luminance = luminance,
            Self::Nvenc(e) => e.set_luminance(luminance),
            Self::Pyrowave(e) => e.luminance = luminance,
        }
    }
    /// The next picture repeats the one last encoded. PyroWave sizes a new
    /// picture from the previous new one and caches static repeat bitstreams;
    /// AMF and NVENC reuse the last converted GPU surface when the source and
    /// conversion inputs match.
    pub fn set_repeat(&mut self, repeat: bool) {
        match self {
            Self::Amf(e) => e.set_repeat(repeat),
            Self::Nvenc(e) => e.set_repeat(repeat),
            Self::Pyrowave(e) => e.repeat = repeat,
        }
    }
    pub fn new_gpu(config: &Negotiated, image: &GpuImage) -> Result<Self> {
        Self::new_gpu_options(
            config,
            image,
            &butterpollo_config::config::Config::default(),
        )
    }
    pub fn new_gpu_options(
        config: &Negotiated,
        image: &GpuImage,
        tuning: &butterpollo_config::config::Config,
    ) -> Result<Self> {
        Self::new_gpu_reported(config, image, tuning, &recorded_warnings())
    }
    pub fn new_gpu_reported(
        config: &Negotiated,
        image: &GpuImage,
        tuning: &butterpollo_config::config::Config,
        warnings: &std::sync::Arc<dyn WarningSink>,
    ) -> Result<Self> {
        warnings.clear("encoder_conversion");
        warnings.clear("encoder_frame_size");
        // A replacement encoder establishes its own refresh policy. Discard
        // the previous AMF fallback before merging fresh startup warnings,
        // including when recovery selects a different codec.
        warnings.clear("encoder_intra_refresh");
        warnings.clear("hdr_dynamic_metadata");
        warnings.clear("encoder_vpe");
        require_encoding_gpu(&image.gpu)?;
        let mut config = config.clone();
        config.sdr_tone_map = tuning.hdr_sdr_tone_map();
        let config = &config;
        let mut created = if config.codec == 3 {
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
            Self::Pyrowave(Box::new(crate::pyrowave::Encoder::new_gpu_reported(
                config,
                image,
                tuning,
                warnings.clone(),
            )?))
        } else if backend(&image.gpu, tuning)? == "nvenc" {
            // As on AMF: convert on the compute queue and encode on its D3D12
            // device, or on the graphics queue when that is unavailable.
            let compute = if crate::nvenc::supports_compute(config)
                && crate::compute::enabled(tuning)
                && crate::compute::shareable(&image.texture)
            {
                match crate::compute::Compute::for_device(&image.gpu.device) {
                    Ok(compute) => Some(compute),
                    Err(error) => {
                        warnings.set("encoder_conversion", &format!("NVENC compute conversion unavailable ({error:#}); converting on the graphics queue. A busy game can delay frames; lower its GPU load or update the NVIDIA driver."));
                        None
                    }
                }
            } else {
                if crate::nvenc::supports_compute(config) && crate::compute::enabled(tuning) {
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
                Err(error) if crate::nvenc::falls_back_to_d3d11(compute.is_some(), &error) => {
                    warnings.set("encoder_conversion", &format!("NVENC on the compute queue's D3D12 device failed ({error:#}); converting on the graphics queue. Lower game GPU load or update the NVIDIA driver (522.25 or newer) if frames are delayed."));
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
            match created {
                Ok(mut encoder) => {
                    if compute.is_some() && !encoder.converts_on_compute() {
                        encoder.mark_compute_fallback();
                    }
                    Self::Nvenc(Box::new(encoder))
                }
                Err(error) => return Err(nvenc_failed(error, &image.gpu.display.adapter)),
            }
        } else {
            // AMF's video converter (`amd_vpe`) runs on the encoder's D3D11
            // context, which a compute-queue encoder does not have.
            let vpe = match crate::amf::vpe_wanted_for(tuning, config) {
                Ok(wanted) => wanted,
                Err(refusal) => {
                    tracing::info!(
                        reason = refusal.code(),
                        "amd_vpe is on, but AMF's video converter cannot write this stream's layout; converting with the shaders"
                    );
                    false
                }
            };
            // Colour conversion on a compute queue keeps running beside a
            // game that fills the GPU's graphics queue.
            // AMF rejects 4:4:4 regardless of which queue converts the input.
            let compute = if vpe {
                tracing::info!(
                    "amd_vpe is on: AMF converts on the encoder's D3D11 context instead of the compute queue"
                );
                None
            } else if !config.yuv444
                && crate::compute::enabled(tuning)
                && crate::compute::shareable(&image.texture)
            {
                match crate::compute::Compute::for_device(&image.gpu.device) {
                    Ok(compute) => Some(compute),
                    Err(error) => {
                        warnings.set("encoder_conversion", &format!("AMF compute conversion unavailable ({error:#}); converting on the graphics queue. A busy game can delay frames; lower its GPU load or update the AMD driver."));
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
                    warnings.set("encoder_conversion", &format!("AMF compute queue initialization failed ({error:#}); retrying conversion on the graphics queue. Lower game GPU load or update the AMD driver if frames are delayed."));
                    crate::amf::Encoder::new_gpu(config, image.gpu.clone(), tuning, None)
                }
                created => created,
            };
            match created {
                Ok(mut encoder) => {
                    attach_warnings(&mut encoder.warnings, warnings);
                    Self::Amf(Box::new(encoder))
                }
                Err(error) => {
                    warnings.clear("encoder_conversion");
                    return Err(amf_failed(error, &image.gpu.display.adapter));
                }
            }
        };
        // An HDR desktop (FP16, or PQ that an SDR stream tone-maps) too.
        if config.hdr || image.pixel != crate::capture::Pixel::Bgra8 {
            created.set_luminance(image.gpu.sdr_white_nits(), 1.0);
        }
        Ok(created)
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
    /// AMF's video converter (`amd_vpe`) gave way: rebuild without it, so conversion can
    /// return to the compute queue.
    pub fn converter_gave_way(&self) -> bool {
        match self {
            Self::Amf(e) => e.vpe_gave_way(),
            Self::Nvenc(_) | Self::Pyrowave(_) => false,
        }
    }
    /// Whether this encoder's frames are converted on a compute queue.
    pub fn converts_on_compute(&self) -> bool {
        match self {
            Self::Amf(e) => e.converts_on_compute(),
            Self::Nvenc(e) => e.converts_on_compute(),
            Self::Pyrowave(e) => e.converts_on_compute(),
        }
    }
    /// Whether NVENC fell back from its D3D12 session to the graphics queue
    /// on this creation.
    pub fn compute_fell_back(&self) -> bool {
        matches!(self, Self::Nvenc(e) if e.compute_fell_back())
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
            Self::Nvenc(_) | Self::Pyrowave(_) => {
                butterpollo_media_policy::encoder_backlog::DEFAULT_LIMIT
            }
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
            &butterpollo_config::config::Config::default(),
        )
    }
    /// An encoder for CPU images, such as the codec probe's.
    pub fn new_options(
        config: &Negotiated,
        display: &str,
        tuning: &butterpollo_config::config::Config,
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

#[cfg(test)]
mod tests;
