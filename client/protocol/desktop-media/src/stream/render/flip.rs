//! AI enhancement, direct draw, PQ video-only clear, and PyroWave unshown helpers.

use super::super::*;
use super::pump::ai_in_effect;
use super::PYROWAVE_UNSHOWN;
#[cfg(windows)]
use super::AI_ENHANCE_UNAVAILABLE;

/// The patched SDL GPU renderer's properties for video-only frames and read-backs
/// (`vendor/sdl3-src/patches/window-target.patch`, `GPU_FrameEligible`).
#[cfg(target_os = "linux")]
const VIDEO_ONLY_PROPERTY: &std::ffi::CStr = c"SDL.renderer.gpu.video_only";
#[cfg(target_os = "linux")]
const VIDEO_ONLY_FRAME_PROPERTY: &std::ffi::CStr = c"SDL.renderer.gpu.video_only_frame";
#[cfg(target_os = "linux")]
const VIDEO_ONLY_DROPPED_PROPERTY: &std::ffi::CStr = c"SDL.renderer.gpu.video_only_dropped_draws";
#[cfg(target_os = "linux")]
const READ_BACK_PROPERTY: &std::ffi::CStr = c"SDL.renderer.gpu.read_back";

impl Streaming {
    /// The current picture through AI enhancement at `output`, or None to draw the picture itself:
    /// a picture it does not read (software-decoded, PyroWave, packed), or the mode failed, which
    /// is logged and shown once and leaves the stream on bilinear scaling.
    #[cfg(windows)]
    pub(in crate::stream) fn enhanced(&mut self, renderer: *mut SDL_Renderer, output: (i32, i32)) -> Option<*mut SDL_Texture> {
        if self.upscale_failed { return None; }
        let Some(source) = self.texture.enhance_source() else { self.enhancer = None; return None };
        if output.0 <= 0 || output.1 <= 0 || output.0 > 16384 || output.1 > 16384 { return None; }
        let key = enhance::Key { source, output, hdr_display: crate::sdl::hdr_available(renderer) };
        if !self.enhancer.as_ref().is_some_and(|enhancer| enhancer.matches(&key)) {
            self.enhancer = None;
            let device = crate::render_device::live_renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_DEVICE_POINTER);
            let made = enhance::stream_probe(device)
                .and_then(|capability| capability.ok_or_else(|| "this GPU has neither video super resolution nor RTX Video HDR".to_owned()))
                .and_then(|capability| enhance::Enhancer::new(renderer, key, capability, self.facts.fps.max(1) as u32));
            match made {
                Ok(enhancer) => {
                    eprintln!("nightjar: {} for {}x{} to {}x{}", enhancer.label, source.picture.0, source.picture.1, output.0, output.1);
                    self.enhancer = Some(enhancer);
                }
                Err(error) => return self.enhance_failed(error),
            }
        }
        let direct = self.enhancer.as_ref().is_some_and(|e| self.texture.enhance_held_frame().is_some_and(|f| e.can_read_held(f)));
        if !direct { self.texture.copy_held(); }
        let held = if direct { self.texture.enhance_held_frame() } else { None };
        match self.enhancer.as_mut()?.draw_frame(renderer, held) {
            Ok(texture) => Some(texture),
            Err(error) => { self.texture.copy_held(); self.enhance_failed(error) }
        }
    }

    #[cfg(not(windows))]
    pub(in crate::stream) fn enhanced(&mut self, _: *mut SDL_Renderer, _: (i32, i32)) -> Option<*mut SDL_Texture> { None }

    /// Whether AI enhancement draws the current picture (`ai_in_effect`); where it does not, its enhancer goes.
    #[cfg(windows)]
    pub(super) fn ai_enhances(&mut self, renderer: *mut SDL_Renderer, output: (i32, i32)) -> bool {
        let source = self.texture.enhance_source();
        let ai = ai_in_effect(self.settings.upscaling, self.upscale_failed, source.is_some())
            && source.is_some_and(|s| enhance::should_enhance(renderer, s, output));
        if !ai { self.enhancer = None; }
        ai
    }

    #[cfg(not(windows))]
    pub(super) fn ai_enhances(&mut self, _: *mut SDL_Renderer, _: (i32, i32)) -> bool { ai_in_effect(self.settings.upscaling, self.upscale_failed, false) }

    #[cfg(windows)]
    fn enhance_failed(&mut self, error: String) -> Option<*mut SDL_Texture> {
        eprintln!("nightjar: AI enhancement failed: {error}; the stream continues with bilinear scaling");
        self.enhancer = None;
        self.upscale_failed = true;
        self.show_toast(tr(AI_ENHANCE_UNAVAILABLE));
        None
    }

    /// The stats line for the upscaling setting.
    pub(in crate::stream) fn upscaling_line(&self) -> String {
        if self.settings.upscaling == config::Upscaling::Ai {
            #[cfg(windows)]
            if let Some(enhancer) = &self.enhancer { return enhancer.label.clone(); }
            return if self.upscale_failed { "Bilinear (AI enhancement unavailable)".into() }
                else if cfg!(windows) { "Bilinear (not a hardware-decoded picture)".into() }
                else { "Bilinear (AI enhancement needs Direct3D 11)".into() };
        }
        #[cfg(target_os = "macos")]
        if self.settings.upscaling == config::Upscaling::MetalFx {
            return match (&self.metalfx, self.metalfx_blocked) {
                (Some(pass), None) => format!("MetalFX ({:.2} ms GPU)", pass.gpu_ms),
                (_, Some(reason)) => format!("Bilinear ({reason})"),
                (None, None) => "Bilinear (no picture yet)".into(),
            };
        }
        if self.upscale_failed { "Bilinear (GPU unavailable)".into() }
        else if self.texture.is_hdr() { "Bilinear (HDR)".into() }
        else if self.upscaler.is_some() { self.settings.upscaling.label().into() }
        else { "Bilinear (no upscale needed)".into() }
    }

    /// Draws the picture itself into the back buffer (`direct` on Windows, the plane pass on
    /// Linux), or returns false for SDL to draw the video texture.
    // Windows and Unix (Linux and macOS): direct draw path; other targets fall back to SDL.
    #[cfg(any(windows, unix))]
    pub(in crate::stream) fn draw_direct(&mut self, renderer: *mut SDL_Renderer, dst: &SDL_FRect, texture: *mut SDL_Texture) -> bool {
        self.texture.draw_direct(renderer, dst, texture)
    }

    // Non-Windows, non-Unix (Linux/macOS) targets: no direct draw path.
    #[cfg(not(any(windows, unix)))]
    pub(in crate::stream) fn draw_direct(&mut self, _renderer: *mut SDL_Renderer, _dst: &SDL_FRect, _texture: *mut SDL_Texture) -> bool {
        false
    }

    /// Clears the window's target for a frame, black. On a PQ output, a frame that will draw only
    /// the picture with the plane pass (`unscaled`) and what of the overlay's writes PQ too asks the patched SDL
    /// for a video-only frame (`present::VideoOnly`): SDL settles it on the frame's first run of
    /// commands on the window's target, which the flush of the clear is (a clear takes no swapchain
    /// image, so V-Sync's wait stays at the frame's first draw). The property is true only around
    /// that flush, so nothing else (the launcher, a later frame) inherits it; the plane pass writes
    /// PQ only when SDL reports the frame video only, and scRGB otherwise.
    ///
    /// # Safety
    /// `renderer` is the live renderer of the presenting thread.
    #[cfg(target_os = "linux")]
    pub(super) unsafe fn clear_window(&mut self, renderer: *mut SDL_Renderer, unscaled: bool) {
        // SAFETY: caller guarantees `renderer` is the live renderer of the presenting thread.
        unsafe {
            let props = SDL_GetRendererProperties(renderer);
            let pq_output = crate::sdl::output_decision() == Some(Ok(crate::handheld::Output::Pq));
            let planes_direct = unscaled && self.texture.draws_planes_direct();
            // What `draw_overlay` in app/overlay.rs draws: nothing, or egui meshes, which write PQ themselves
            // (`ui_pq`) where a frame could be video only but for them.
            let meshes = self.overlay_visible || self.toast.is_some() || self.menu_visible;
            let frame = present::VideoOnlyFrame {
                pq_output,
                planes_direct,
                nothing_else: self.facts.role != nightjar_core::http::AppRole::InputOnly
                    && (!meshes || (pq_output && planes_direct && self.texture.ui_draws_pq(renderer))),
                read_back: SDL_GetBooleanProperty(props, READ_BACK_PROPERTY.as_ptr(), false),
            };
            let (asked, line) = self.texture.video_only().wanted(frame, session::now_us());
            if let Some(line) = line { eprintln!("{line}"); }
            if asked {
                SDL_SetBooleanProperty(props, VIDEO_ONLY_FRAME_PROPERTY.as_ptr(), false);
                SDL_SetBooleanProperty(props, VIDEO_ONLY_PROPERTY.as_ptr(), true);
            }
            SDL_SetRenderDrawColor(renderer, 0, 0, 0, 255);
            SDL_RenderClear(renderer);
            let mut pq_target = false;
            if asked {
                // SDL_FlushRenderer's failure leaves the frame undecided, which the reset below settles
                // as not video only; the settled value says which it was either way.
                SDL_FlushRenderer(renderer);
                SDL_SetBooleanProperty(props, VIDEO_ONLY_PROPERTY.as_ptr(), false);
                pq_target = SDL_GetBooleanProperty(props, VIDEO_ONLY_FRAME_PROPERTY.as_ptr(), false);
                if let Some(line) = self.texture.video_only().settled(pq_target) { eprintln!("{line}"); }
            }
            self.texture.set_pq_target(pq_target);
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) unsafe fn clear_window(&mut self, renderer: *mut SDL_Renderer, _unscaled: bool) {
        // SAFETY: caller guarantees `renderer` is live on the calling thread.
        unsafe {
            SDL_SetRenderDrawColor(renderer, 0, 0, 0, 255);
            SDL_RenderClear(renderer);
        }
    }

    /// The render state `draw_overlay` in app/overlay.rs hands egui's painter for this frame: null unless
    /// the frame is video only on a PQ output, where SDL would drop its own draws (`ui_pq`).
    #[cfg(target_os = "linux")]
    pub fn ui_state(&mut self, renderer: *mut SDL_Renderer) -> *mut SDL_GPURenderState {
        // SAFETY: the live renderer of the presenting thread.
        unsafe { self.texture.ui_pq_state(renderer) }
    }

    #[cfg(not(target_os = "linux"))]
    pub fn ui_state(&mut self, _renderer: *mut SDL_Renderer) -> *mut SDL_GPURenderState {
        std::ptr::null_mut()
    }

    /// After a present: a video-only frame in which SDL dropped draws of its own holds video-only
    /// frames off (`present::VideoOnly::presented`).
    #[cfg(target_os = "linux")]
    pub(super) fn video_only_presented(&mut self, renderer: *mut SDL_Renderer) {
        self.texture.set_pq_target(false);
        // SAFETY: a live renderer and a property name constant.
        let dropped = unsafe { SDL_GetNumberProperty(SDL_GetRendererProperties(renderer), VIDEO_ONLY_DROPPED_PROPERTY.as_ptr(), 0) };
        if let Some(line) = self.texture.video_only().presented(dropped, session::now_us()) { eprintln!("{line}"); }
    }

    /// Ends a stream whose PyroWave pictures cannot be shown; the reason goes to the log.
    pub(super) fn end_pyrowave_unshown(&mut self) {
        eprintln!("nightjar: PyroWave pictures cannot be shown: {}", self.texture.failure.take().as_deref().unwrap_or("GPU reconstruction failed"));
        self.exit = Some(Exit::Ended(Some(tr(PYROWAVE_UNSHOWN).to_owned())));
    }
}
