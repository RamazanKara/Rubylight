//! Windows D3D11 swapchain switching: keeps SDR streams on an 8-bit sRGB (`BGRA8`) swapchain
//! and switches to 16-bit floating-point scRGB (`SDL_COLORSPACE_SRGB_LINEAR`) only for HDR or
//! 10-bit SDR streams, and for AI enhancement on a display in HDR.

#[cfg(any(windows, test))]
use super::*;

/// Whether a Windows D3D11 stream needs a 16-bit floating-point scRGB swapchain
/// (`SDL_COLORSPACE_SRGB_LINEAR`) rather than an 8-bit sRGB (`BGRA8`) swapchain. HDR and 10-bit
/// SDR streams do, and AI enhancement when the window's display is in HDR (`display_hdr`): RTX
/// Video HDR turns 8-bit SDR into HDR, and SDL reports HDR output only for this swapchain. Plain
/// 8-bit SDR, shader upscaling, 4:4:4 and PyroWave SDR render on the lightweight 8-bit swapchain.
#[cfg(any(windows, test))]
pub(crate) fn windows_needs_linear(settings: &config::Settings, display_hdr: bool) -> bool {
    settings.hdr
        || settings.sdr_10bit
        || (settings.upscaling == config::Upscaling::Ai && display_hdr)
}

#[cfg(windows)]
impl App {
    /// Before a stream's checks on Windows: switches the Direct3D 11 swapchain between 8-bit sRGB
    /// (`BGRA8`) and 16-bit float scRGB (`SDL_COLORSPACE_SRGB_LINEAR`) only when the stream format
    /// requires it (`windows_needs_linear`), so SDR streams avoid the bandwidth and DWM scRGB
    /// composition cost of a 64-bit floating-point swapchain.
    pub(crate) fn switch_for_stream(&mut self, settings: &config::Settings) {
        if self.streaming.is_some()
            || self.attempts.is_busy()
            || self.probe.is_some()
            || !self.ending_probes.is_empty()
            || self.pending_replace.is_some()
            || self.recovery.is_lost()
        {
            return;
        }
        let linear = windows_needs_linear(settings, sdl::window_display_hdr(self.window));
        if sdl::renderer_linear(self.renderer) == linear {
            return;
        }
        if !self.join_teardowns(stream::LEAVE_WAIT)
            || !stream::decoder_cache::clear(Instant::now() + stream::LEAVE_WAIT)
        {
            if linear {
                self.gpu_failure = Some(teardown_switch_text().to_owned());
            } else {
                eprintln!(
                    "nightjar: keeping current swapchain for SDR stream: a previous stream is still releasing the device"
                );
            }
            return;
        }
        stream::pyrowave_mode::settings_probe_reset();
        self.latency_bench.finish();
        self.launcher.latency_bench = self.latency_bench.state();
        self.gui.release_renderer();
        let old = self.renderer;
        let kind = sdl::renderer_name(old);
        let held = media::DeviceRef::of_device(sdl::renderer_pointer(
            old,
            SDL_PROP_RENDERER_D3D11_DEVICE_POINTER,
        ));
        // SAFETY: the renderer run() made, its gui textures freed, with no stream or launch using it.
        unsafe { crate::sdl::destroy_renderer(old) };
        let mut devices: Vec<_> = self.launch_devices().cloned().collect();
        devices.push(held);
        Self::flush_released(devices, Self::FLUSH_WAIT);
        let mut made = sdl::remake_renderer(self.window, linear, &kind, Duration::from_secs(2));
        if made.is_null() {
            made = sdl::create_renderer_with(self.window, false, false);
        }
        if made.is_null() {
            no_renderer(&format!(
                "the {kind:?} renderer could not switch colorspace ({})",
                sdl::error()
            ));
        }
        // SAFETY: the renderer just made; flush stale render-device events from the old one now.
        unsafe {
            SDL_SetRenderVSync(made, 1);
            SDL_FlushEvents(
                SDL_EVENT_RENDER_TARGETS_RESET.0,
                SDL_EVENT_RENDER_DEVICE_LOST.0,
            );
        }
        self.renderer = made;
        self.gui.renderer_changed();
        reset_launcher_textures(&mut self.hosts, &mut self.launcher, &self.gui.ctx);
        self.launcher.gpu_device = renderer_gpu_device(made).map(|d| d.0 as usize);
        self.launcher.tearing_supported = media::swapchain_allows_tearing(sdl::renderer_pointer(
            made,
            SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER,
        ));
        self.color_probed = None;
        self.base_probed = None;
        self.base_probe_retried = None;
        self.packed_probed = None;
        self.window_changed = true;
        eprintln!(
            "nightjar: renderer switched to {} swapchain for stream",
            if sdl::renderer_linear(made) {
                "scRGB (HDR/10-bit/AI HDR)"
            } else {
                "sRGB (8-bit)"
            }
        );
    }
}
