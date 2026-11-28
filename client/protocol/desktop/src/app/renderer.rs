//! The window's SDL renderer: which one the settings and a stream need, rebuilding it, and the decoder
//! devices that hang on it.

use super::*;

/// What Linux's renderer was made for, beyond what SDL says of it.
#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Debug)]
pub(crate) struct RendererState {
    /// The Renderer setting last applied: a change is applied once, with nothing streaming.
    pub(crate) choice: config::Renderer,
    /// Made for an HDR stream with a display's HDR on (`handheld::RendererNow`).
    pub(crate) made_for_hdr: bool,
    /// Made with Vulkan video's extensions and queues, when it is the GPU renderer.
    pub(crate) vulkan_video: bool,
    /// Why the GPU renderer could not be made this run. A stream does not try it again (each try
    /// makes a working renderer again, and one that fails then ends Nightjar); a changed Renderer
    /// setting does.
    pub(crate) gpu_failed: Option<String>,
}

#[cfg(any(target_os = "linux", test))]
impl RendererState {
    pub(crate) fn for_launcher(choice: config::Renderer, vulkan_video: bool, gpu_failed: Option<String>) -> Self {
        // The launcher never asks for an HDR stream's PQ present, even with HDR enabled in settings.
        // Its first HDR stream must still prepare that output (`switch_for_stream`).
        Self { choice, made_for_hdr: false, vulkan_video, gpu_failed }
    }
}

/// Why `rebuild_renderer` did not make the renderer asked for.
#[cfg(unix)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Rebuild {
    /// Something still uses the old renderer's device (a teardown, a decoder dropping). Nothing was
    /// tried, so it is no failure of the renderer: the switch may be asked again later.
    Busy(String),
    /// The renderer asked for could not be made.
    Failed(String),
}

#[cfg(unix)]
impl Rebuild {
    pub(crate) fn text(&self) -> &str {
        match self {
            Self::Busy(why) | Self::Failed(why) => why,
        }
    }
}

#[cfg(any(target_os = "linux", test))]
impl RendererState {
    /// What a stream that asked for the GPU renderer is told when the switch was refused. Only a
    /// renderer that could not be made is remembered (`gpu_failed`); a busy device is not.
    #[cfg(unix)]
    pub(crate) fn rebuild_refused(&mut self, error: &Rebuild) -> String {
        if let Rebuild::Failed(why) = error {
            self.gpu_failed = Some(why.clone());
        }
        error.text().to_owned()
    }
}

#[cfg(any(windows, test))]
pub(crate) use crate::render_device::RemakeGuard;
pub(crate) use crate::render_device::renderer_gone;

/// Whether any layer (the global settings, a PC, a game or a profile) asks for YUV 4:4:4.
#[cfg(windows)]
pub(crate) fn any_yuv444_asked(store: &config::Store, settings: &config::Settings) -> bool {
    let hosts: Vec<config::HostRecord> = nightjar_core::hosts::HostStore::open(store.dir())
        .with_read_only(store.is_read_only())
        .load_records(config::keep_readable_settings_value);
    let profiles = store.profiles();
    settings.prefer_yuv444
        || hosts.iter().flat_map(|host| host.settings.iter().chain(host.app_settings.values()))
            .chain(profiles.entries.iter().map(|profile| &profile.settings))
            .any(|own| own.apply(settings).prefer_yuv444)
}

/// Ends Nightjar, loudly, when it has no renderer to draw with: `why` goes to the log and a
/// message box.
pub(super) fn no_renderer(why: &str) -> ! {
    let why = format!("Nightjar has no renderer any more: {why}");
    eprintln!("nightjar: {why}");
    message_box(SDL_MESSAGEBOX_ERROR, &why);
    logpipe::shutdown();
    std::process::exit(1);
}

/// The renderer's device PyroWave GPU decoding is tied to: the Direct3D 11 device on Windows, the
/// SDL GPU device (the "gpu" renderer, Vulkan) elsewhere.
pub(crate) fn renderer_gpu_device(renderer: *mut SDL_Renderer) -> Option<stream::pipeline::GpuDevice> {
    let (name, property) = if cfg!(windows) {
        ("direct3d11", SDL_PROP_RENDERER_D3D11_DEVICE_POINTER)
    } else {
        ("gpu", SDL_PROP_RENDERER_GPU_DEVICE_POINTER)
    };
    if sdl::renderer_name(renderer) != name { return None; }
    #[cfg(windows)]
    let device = crate::render_device::live_renderer_pointer(renderer, property);
    #[cfg(not(windows))]
    let device = sdl::renderer_pointer(renderer, property);
    (!device.is_null()).then_some(stream::pipeline::GpuDevice(device))
}

/// Whether Linux needs the GPU renderer for Vulkan video, which decodes on its Vulkan device: when
/// VAAPI cannot open a device (NVIDIA's driver without a VAAPI one) and the GPU SDL picks has the
/// video extensions, or `NIGHTJAR_VULKAN_VIDEO=1` (`choice`); not when no stream can decode with it
/// (`vulkan_video_possible`) or `NIGHTJAR_VULKAN_VIDEO=0`. A machine with VAAPI keeps its
/// renderer, unless its VA driver wraps another API (`linux_video::vaapi_plan`). The reason goes
/// to the log.
#[cfg(target_os = "linux")]
pub(crate) fn gpu_for_vulkan_video(possible: bool, choice: vulkan_video::Choice) -> bool {
    use vulkan_video::Choice;
    if !possible || choice == Choice::Off {
        return false;
    }
    let why = match (choice, media::vaapi_device(None)) {
        (Choice::First, _) => format!("{}=1", vulkan_video::ENVIRONMENT),
        (_, Err(error)) => format!("VAAPI is unavailable ({error})"),
        // A wrapper VA driver (nvidia-vaapi-driver) is left out of the order: Vulkan video
        // decodes instead, on the GPU renderer.
        (_, Ok(identity)) => match linux_video::vaapi_plan(Ok(&identity.vendor), stream::env_flag(linux_video::FORCE_VAAPI_ENVIRONMENT) == Some(true)) {
            linux_video::VaapiPlan { skip: true, why: Some(why) } => why,
            _ => return false,
        },
    };
    match stream::pyrowave_sdk::zero_copy::video_devices() {
        Ok(_) => {
            vulkan_video::note(&format!("{why}: the GPU renderer is used for Vulkan video decoding"));
            true
        }
        Err(error) => {
            vulkan_video::note(&format!("{why}, and Vulkan video cannot be used either: {error}"));
            false
        }
    }
}

/// Whether a stream can decode with Vulkan video or map VAAPI's pictures into Vulkan images, so the
/// GPU renderer's device is made with the extensions for them (video ones and decode queues unless
/// `NIGHTJAR_VULKAN_VIDEO` is 0, `choice`; DMA-BUF import ones unless `NIGHTJAR_VAAPI_VULKAN_MAP`
/// is 0, `map`): on Linux, when a stream may decode in hardware (`any_hardware_decoding`).
#[cfg(any(target_os = "linux", test))]
pub(crate) fn vulkan_video_possible(hardware_decoding: bool, choice: vulkan_video::Choice, map: bool) -> bool {
    cfg!(target_os = "linux") && hardware_decoding && (choice != vulkan_video::Choice::Off || map)
}

/// `NIGHTJAR_VAAPI_VULKAN_MAP` from the environment (`vulkan_video::map_switched_on`).
#[cfg(target_os = "linux")]
pub(crate) fn vaapi_map_switched_on() -> bool {
    vulkan_video::map_switched_on(std::env::var_os(vulkan_video::MAP_ENVIRONMENT).as_deref())
}

/// Whether a stream may decode in hardware: the global decoder setting, or a profile's, a host's
/// or an app's override of it, applied over the global settings as a stream applies it. The
/// renderer is chosen once for every stream, so only when all of them say Software is it chosen
/// as for software decoding.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn any_hardware_decoding<'a>(settings: &config::Settings, overrides: impl IntoIterator<Item = &'a config::Overrides>) -> bool {
    settings.decoder != DecoderPreference::Software
        || overrides.into_iter().any(|own| own.apply(settings).decoder != DecoderPreference::Software)
}

/// `settings` or an override of it (`any_hardware_decoding`) asks for HDR.
pub(crate) fn any_hdr_asked<'a>(settings: &config::Settings, overrides: impl IntoIterator<Item = &'a config::Overrides>) -> bool {
    settings.hdr || overrides.into_iter().any(|own| own.apply(settings).hdr)
}

/// Whether any layer (the global settings, a PC, a game or a profile) turns on PyroWave's compute
/// queue: on Linux the renderer's shared device gets the queue only when it is made.
pub(crate) fn any_async_compute_asked<'a>(settings: &config::Settings, overrides: impl IntoIterator<Item = &'a config::Overrides>) -> bool {
    settings.pyrowave_async_compute || overrides.into_iter().any(|own| own.apply(settings).pyrowave_async_compute)
}

/// What a start says when a session that has ended is still using the graphics card: a renderer
/// switch cannot be made while its decoder and audio threads run.
pub(crate) fn teardown_switch_text() -> &'static str {
    tr("A start that has ended is still using the graphics card, so it cannot be switched yet. The change is kept and applied when that start stops; if it does not stop, restart Nightjar.")
}

impl App {
    /// Why the GPU renderer `need` asks for is missing, when a failed switch or the Renderer setting
    /// at OpenGL is why; None otherwise (Windows, or the GPU renderer is there).
    pub(crate) fn gpu_missing(&self, need: &'static str) -> Option<String> {
        if let Some(why) = &self.gpu_failure {
            return Some(trf("The GPU renderer could not be made: {}.", &[why]));
        }
        (cfg!(target_os = "linux") && self.launcher.settings.renderer == config::Renderer::OpenGl && sdl::renderer_name(self.renderer) != "gpu")
            .then(|| trf("{} needs the GPU renderer. Set Renderer to Automatic or GPU (Vulkan).", &[&tr(config::gpu_need_label(need))]))
    }

    /// The gui's textures back on the window's renderer, off a stream's overlay plane renderer
    /// (`kms_scanout`) before that renderer goes.
    pub(crate) fn gui_back_from_overlay(&mut self) {
        if !self.gui_overlay.is_null() {
            self.gui.release_renderer();
            self.gui.renderer_changed();
            self.gui_overlay = std::ptr::null_mut();
        }
    }

    /// Before a stream's checks on Linux: switches to the GPU renderer when the stream needs it
    /// (`handheld::renderer_switch`). A failure keeps a renderer of the old kind and leaves why in
    /// `gpu_failure`, which the check that refuses the stream then says.
    #[cfg(target_os = "linux")]
    pub(crate) fn switch_for_stream(&mut self, settings: &config::Settings) {
        // Before a renderer made for the stream: its HDR description is made for this brightness.
        #[cfg(target_os = "linux")]
        stream::wl_scanout::set_brightness(stream::wl_scanout::Brightness::of_setting(settings.hdr_desktop_brightness));
        self.apply_renderer_choice();
        let gpu = sdl::renderer_name(self.renderer) == "gpu";
        let state = self.renderer_state.clone();
        // A GPU renderer that failed this run: the checks that refuse the stream say why.
        self.gpu_failure.clone_from(&state.gpu_failed);
        let now = handheld::RendererNow {
            gpu,
            hdr_swapchain: sdl::swapchain_takes_hdr(self.renderer),
            made_for_hdr: state.made_for_hdr,
            gpu_failed: state.gpu_failed.is_some(),
            pq_unused: gpu && handheld::pq_unused(handheld::keeps_scrgb_first(), sdl::output_decision(), sdl::hdr10_offered(self.renderer), sdl::pq_failed()),
            hdr_unreachable: gpu && sdl::pq_failed() && !sdl::scrgb_offered(self.renderer),
        };
        let vulkan_choice = vulkan_video::Choice::from_environment();
        let vulkan_video = state.vulkan_video || vulkan_video_possible(settings.decoder != DecoderPreference::Software, vulkan_choice, vaapi_map_switched_on());
        // A stream decoding in hardware where every saved one decoded in software: Vulkan video
        // may need the GPU renderer, as at start.
        // A stream on video planes (KMS) shows HDR and 10-bit pictures on the plane, not the renderer.
        let planes = kms_planned(settings, self.renderer);
        let need = if planes { None } else { settings.gpu_need() }.or_else(|| {
            (!gpu && !state.vulkan_video && vulkan_video && state.choice == config::Renderer::Auto && gpu_for_vulkan_video(vulkan_video, vulkan_choice))
                .then_some("Vulkan video")
        });
        let display_hdr = settings.hdr && handheld::hdr_display(handheld::gamescope(), sdl::any_display_hdr());
        // A stream that still stops on the old device: wait for it before a switch this stream
        // needs, and refuse the start when it does not end in time instead of skipping the switch
        // silently. A switch the stream does not need waits for nothing.
        let teardowns_busy = self.teardowns_busy();
        let wanted = handheld::renderer_switch(state.choice, now, need, settings.hdr && !planes, display_hdr && !planes, false);
        if teardowns_busy && wanted.is_some() && !self.join_teardowns(stream::LEAVE_WAIT) {
            self.gpu_failure = Some(teardown_switch_text().to_owned());
            return;
        }
        // The quit-and-start prompt's plan holds the renderer's device, and a launch's checks may
        // still decode on it.
        let busy = self.streaming.is_some() || self.attempts.is_busy() || self.probe.is_some() || !self.ending_probes.is_empty()
            || self.pending_replace.is_some() || self.teardowns_busy();
        let Some(switch) = handheld::renderer_switch(state.choice, now, need, settings.hdr && !planes, display_hdr && !planes, busy) else { return };
        if let Err(error) = self.rebuild_renderer(true, settings.hdr, display_hdr, vulkan_video, switch.reason()) {
            // A busy device refuses this stream only; the next one asks again.
            self.gpu_failure = Some(self.renderer_state.rebuild_refused(&error));
        }
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn switch_for_stream(&mut self, settings: &config::Settings) {
        if self.streaming.is_some() || self.attempts.is_busy() || self.probe.is_some() || !self.ending_probes.is_empty()
            || self.pending_replace.is_some()
        {
            return;
        }
        let shaders = settings.codec.is_pyrowave() || settings.experimental_packed_444
            || settings.prefer_yuv444 || settings.upscaling.is_shader() || settings.upscaling == config::Upscaling::Ai
            || (settings.decoder == DecoderPreference::Software && (settings.hdr || settings.sdr_10bit))
            || (settings.hdr && stream::hlg_requested());
        let wanted = (sdl::renderer_name(self.renderer) == "gpu") != shaders
            || sdl::swapchain_takes_hdr(self.renderer) != settings.hdr;
        if wanted {
            // As on Linux: wait for the session still stopping, and refuse the start when it does
            // not end in time rather than skip the switch silently.
            if !self.join_teardowns(stream::LEAVE_WAIT) {
                self.gpu_failure = Some(teardown_switch_text().to_owned());
                return;
            }
            self.gpu_failure = self.rebuild_renderer(shaders, settings.hdr, true, false, "the stream format").err().map(|error| error.text().to_owned());
        }
    }

    /// A changed Graphics card setting, applied once with nothing streaming: Nightjar closes and
    /// starts again when it draws on the other GPU (`gpu_adapters::choose_now`).
    #[cfg(windows)]
    pub(crate) fn apply_graphics_card(&mut self) {
        let choice = self.launcher.settings.graphics_card;
        if choice == self.graphics_card || self.streaming.is_some() || self.pending_replace.is_some()
            || self.recovery.is_lost() || self.recovery.pending_reconnect.is_some()
        {
            return;
        }
        // A start still connecting uses the device it would replace; a start that was cancelled or
        // timed out and whose worker has not let go of the graphics card blocks the change, and
        // says so once (it applies when that worker ends).
        if self.attempts.is_connecting() || self.probe.is_some() {
            return;
        }
        if self.attempts.is_busy() || self.teardowns_busy() {
            if self.graphics_card_blocked != Some(choice) {
                self.graphics_card_blocked = Some(choice);
                self.launcher.notify(
                    tr("Graphics card not changed"),
                    tr("A start that has ended is still using the graphics card, so it cannot be switched yet. The change is kept and applied when that start stops; if it does not stop, restart Nightjar."),
                );
            }
            return;
        }
        self.graphics_card_blocked = None;
        // Persist the new card choice first: if saving fails, restart would re-launch on the old
        // card while leaving Nightjar unexpectedly closed.
        if let Err(e) = self.store.save_settings(&self.launcher.settings) {
            eprintln!("nightjar: saving settings failed: {e}");
            self.graphics_card = choice;
            self.save_failed();
            return;
        }
        self.graphics_card = choice;
        if gpu_adapters::choose_now(choice, any_yuv444_asked(&self.store, &self.launcher.settings), self.store.dir()) {
            self.running = false;
        }
    }

    /// A changed Renderer setting, applied once with nothing streaming (`handheld::renderer_for_choice`).
    #[cfg(target_os = "linux")]
    pub(crate) fn apply_renderer_choice(&mut self) {
        let choice = self.launcher.settings.renderer;
        if choice == self.renderer_state.choice || self.streaming.is_some() || self.pending_replace.is_some() {
            return;
        }
        // As `apply_graphics_card`: a start still connecting waits; one that ended (its worker or
        // a decoder check) and has not let go of the device blocks the change, and says so once.
        if self.attempts.is_connecting() || self.probe.is_some() {
            return;
        }
        if self.attempts.is_busy() || !self.ending_probes.is_empty() || self.teardowns_busy() {
            if self.renderer_choice_blocked != Some(choice) {
                self.renderer_choice_blocked = Some(choice);
                self.launcher.notify(
                    tr("Renderer not changed"),
                    tr("A start that has ended is still using the graphics card, so it cannot be switched yet. The change is kept and applied when that start stops; if it does not stop, restart Nightjar."),
                );
            }
            return;
        }
        // A decoder still dropping on the old device defers the change; it is asked again after a pause.
        if self.renderer_choice_retry.is_some_and(|at| Instant::now() < at) {
            return;
        }
        self.renderer_choice_retry = None;
        let Some(gpu) = handheld::renderer_for_choice(choice, sdl::renderer_name(self.renderer) == "gpu") else {
            self.renderer_choice_blocked = None;
            self.renderer_state.choice = choice;
            return;
        };
        match self.rebuild_renderer(gpu, false, false, self.renderer_state.vulkan_video, "the Renderer setting") {
            Ok(()) => {
                self.renderer_choice_blocked = None;
                self.renderer_state.choice = choice;
                if gpu {
                    self.renderer_state.gpu_failed = None;
                }
            }
            Err(Rebuild::Busy(_)) => {
                // The choice stays unapplied (`renderer_state.choice` is the one last applied).
                self.renderer_choice_retry = Some(Instant::now() + Duration::from_secs(2));
                if self.renderer_choice_blocked != Some(choice) {
                    self.renderer_choice_blocked = Some(choice);
                    self.launcher.notify(
                        tr("Renderer not changed"),
                        tr("A start that has ended is still using the graphics card, so it cannot be switched yet. The change is kept and applied when that start stops; if it does not stop, restart Nightjar."),
                    );
                }
            }
            Err(error @ Rebuild::Failed(_)) => {
                self.renderer_choice_blocked = None;
                self.renderer_state.choice = choice;
                if gpu {
                    self.renderer_state.gpu_failed = Some(error.text().to_owned());
                }
                self.launcher.message(tr("Renderer"), trf("The renderer could not be made: {}.", &[&error.text()]));
            }
        }
    }

    /// Makes Linux's renderer again on the same window, as the GPU renderer (`gpu`) or OpenGL, with
    /// nothing streaming or launching on it: what the GUI made on the old one goes first and comes
    /// back on the new one, and what run() took from the renderer is taken again. `hdr`: for an HDR
    /// stream (`display_hdr`: a display has HDR on). When the renderer asked for cannot be made, one
    /// of the old kind is (the GPU renderer once more without Vulkan video's options, never OpenGL in
    /// its place), and the error says why; with none at all Nightjar ends, loudly.
    #[cfg(unix)]
    fn rebuild_renderer(&mut self, gpu: bool, hdr: bool, display_hdr: bool, vulkan_video: bool, reason: &str) -> Result<(), Rebuild> {
        // A session still stopping on this renderer's GPU device keeps it: the caller says the
        // change waits, with the message a start that has ended gets.
        if self.teardowns_busy() {
            return Err(Rebuild::Busy(tr("A start that has ended is still using the graphics card, so it cannot be switched yet. The change is kept and applied when that start stops; if it does not stop, restart Nightjar.").to_owned()));
        }
        // A decoder kept for a reconnect may borrow or hold this device: it goes first, within the
        // leaving deadline; one that does not let go keeps the renderer, as a stuck teardown does.
        if !stream::decoder_cache::clear(Instant::now() + stream::LEAVE_WAIT) {
            return Err(Rebuild::Busy(tr("A start that has ended is still using the graphics card, so it cannot be switched yet. The change is kept and applied when that start stops; if it does not stop, restart Nightjar.").to_owned()));
        }
        let from = sdl::renderer_name(self.renderer);
        let wanted = if gpu { "gpu" } else if cfg!(target_os = "macos") { "metal" } else { "opengl" };
        // Waits for a settings probe still on the old device, and forgets what it found.
        stream::pyrowave_mode::settings_probe_reset();
        // And for the latency benchmark's decoders, which may decode on the old device.
        self.latency_bench.finish();
        self.launcher.latency_bench = self.latency_bench.state();
        self.gui.release_renderer();
        // SAFETY: the renderer run() made, its textures freed, with no stream or launch using it.
        unsafe { crate::sdl::destroy_renderer(self.renderer) };
        if hdr {
            sdl::set_hdr_asked(true);
        }
        // Only an HDR stream's renderer may present as HDR10 PQ; the launcher and SDR streams keep
        // the window target.
        sdl::set_hdr_stream(hdr && display_hdr);
        let mut made = sdl::create_renderer_with(self.window, gpu, vulkan_video);
        #[cfg(target_os = "linux")]
        let mut vulkan_video = vulkan_video;
        #[allow(unused_mut)] // Linux can retry with fewer Vulkan extensions.
        let mut failure = if made.is_null() {
            Some(sdl::error())
        } else if gpu && sdl::renderer_name(made) != "gpu" {
            Some(sdl::gpu_renderer_failure().unwrap_or_else(|| format!("SDL made {:?} instead", sdl::renderer_name(made))))
        } else {
            None
        };
        #[cfg(target_os = "linux")]
        if failure.is_some() && gpu && from == "gpu" {
            // SDL's own fallback is OpenGL: from the GPU renderer, the GPU renderer once more, plainly.
            if !made.is_null() {
                // SAFETY: the renderer just made, nothing on it yet.
                unsafe { crate::sdl::destroy_renderer(made) };
            }
            made = sdl::create_renderer_with(self.window, true, false);
            if !made.is_null() && sdl::renderer_name(made) == "gpu" {
                eprintln!("nightjar: the GPU renderer for {reason} is made without Vulkan video: {}", failure.take().unwrap_or_default());
                vulkan_video = false;
            }
        }
        if made.is_null() {
            #[cfg(target_os = "linux")]
            let previous_vulkan = self.renderer_state.vulkan_video;
            #[cfg(target_os = "macos")]
            let previous_vulkan = false;
            made = sdl::create_renderer_with(self.window, from == "gpu", previous_vulkan);
        }
        if made.is_null() && from == "gpu" {
            made = sdl::create_renderer_with(self.window, false, false);
        }
        sdl::set_hdr_stream(false);
        if made.is_null() {
            no_renderer(&format!("the {wanted:?} renderer for {reason} and the {from:?} one before it could not be made ({})", sdl::error()));
        }
        // SAFETY: the renderer just made.
        unsafe { SDL_SetRenderVSync(made, 1) };
        self.renderer = made;
        self.gui.renderer_changed();
        self.hosts.renderer_changed();
        self.launcher.renderer_changed(&self.gui.ctx);
        #[cfg(feature = "demo-library")]
        {
            let mut hosts_view = self.hosts.ui_view();
            ui::demo::covers(&mut hosts_view, &self.gui.ctx);
            self.hosts.apply_ui_demo(hosts_view);
        }
        self.launcher.gpu_device = renderer_gpu_device(made).map(|d| d.0 as usize);
        self.color_probed = None;
        self.base_probed = None;
        self.base_probe_retried = None;
        self.packed_probed = None;
        self.window_changed = true;
        let to = sdl::renderer_name(made);
        #[cfg(target_os = "linux")]
        {
            self.renderer_state.made_for_hdr = to == "gpu" && hdr && display_hdr;
            if failure.is_none() { self.renderer_state.vulkan_video = vulkan_video; }
        }
        match failure {
            None => {
                eprintln!("nightjar: renderer switched from {from:?} to {to:?} for {reason}");
                Ok(())
            }
            Some(why) => {
                eprintln!("nightjar: renderer {from:?} could not switch to {wanted:?} for {reason} ({why}); drawing with {to:?}");
                Err(Rebuild::Failed(why))
            }
        }
    }

    /// A render device event (`renderer_gone`), from the launcher's loop or a stream's: Windows
    /// makes the renderer again in the launcher's next frame (`recover_renderer`).
    pub(crate) fn note_render_device(&mut self, kind: SDL_EventType) {
        let gone = renderer_gone(kind);
        #[cfg(windows)]
        if gone { self.recovery.lost = true; }
        #[cfg(not(windows))]
        let _ = gone;
    }

    /// Whether the renderer's device is known to be lost or has a pending SDL render-device event.
    /// Always false off Windows, where `recover_renderer` does not apply.
    pub(crate) fn is_renderer_lost(&self) -> bool {
        #[cfg(windows)] { self.recovery.is_lost() }
        #[cfg(not(windows))] { false }
    }

    /// Sets the renderer's V-Sync interval unless the D3D11 device is already lost (SDL3's
    /// `D3D11_ReleaseAll` zeroes `swapChain` without clearing `swapChainFlags`, so calling
    /// `SDL_SetRenderVSync` on a lost D3D11 renderer dereferences a null swap chain).
    pub(crate) fn set_render_vsync(&self, interval: i32) {
        if !self.is_renderer_lost() {
            // SAFETY: renderer call on the main thread while the render device is live.
            unsafe { SDL_SetRenderVSync(self.renderer, interval) };
        }
    }

    /// Records render-device loss from a stream's `exit` and keeps its device reference so
    /// `recover_renderer` can log `GetDeviceRemovedReason()` and flush deferred releases.
    pub(crate) fn note_stream_end_device(&mut self, exit: &Exit, streaming: &mut Streaming) -> bool {
        #[cfg(windows)]
        if matches!(exit, Exit::RenderDeviceLost(_)) || streaming.device_removed() {
            self.recovery.lost = true;
        }
        #[cfg(not(windows))]
        let _ = exit;
        let lost = self.is_renderer_lost();
        #[cfg(windows)]
        if lost {
            self.recovery.lost = true;
            self.recovery.remember_device(streaming.take_device());
        }
        #[cfg(not(windows))]
        let _ = streaming;
        lost
    }

    /// Starts a stream reconnect immediately, or defers it until `recover_renderer` replaces a
    /// lost Windows render device.
    pub(crate) fn start_or_defer_reconnect(&mut self, mut plan: LaunchPlan, teardown: stream::Teardown, renderer_lost: bool) {
        #[cfg(windows)]
        if renderer_lost {
            self.recovery.defer_reconnect(plan);
            self.push_teardown(teardown);
            return;
        }
        #[cfg(not(windows))]
        let _ = renderer_lost;
        self.refresh_plan(&mut plan);
        let mut stopping = self.take_stopping();
        stopping.push(teardown);
        self.attempts.begin(Connecting::start(plan, self.waker.clone(), stopping));
    }

    /// Whether a mid-stream reconnect is waiting for `recover_renderer` to finish.
    pub(crate) fn has_pending_device_reconnect(&self) -> bool {
        #[cfg(windows)] { self.recovery.pending_reconnect.is_some() }
        #[cfg(not(windows))] { false }
    }

    /// Ends the stream with `exit` and updates `--autostream` if active.
    pub(crate) fn finish_stream_exit(&mut self, exit: Exit) {
        let planned = matches!(exit, Exit::User);
        self.end_stream(exit);
        let reconnecting = self.attempts.is_connecting() || self.has_pending_device_reconnect();
        if let Some(auto) = self.autostream.as_mut()
            && !auto.stream_ended(planned, reconnecting) { self.running = false; }
    }

    /// Checks whether a launcher present lost the Windows D3D11 device (via SDL event or
    /// `GetDeviceRemovedReason()` on `DXGI_ERROR_DEVICE_RESET`) and recovers before commands run.
    pub(crate) fn recover_after_launcher_present(&mut self, present_failed: bool) {
        if present_failed {
            eprintln!("nightjar: launcher SDL_RenderPresent failed: {}", sdl::error());
        }
        #[cfg(windows)]
        if self.recovery.is_lost()
            || self.recovery.check_launcher_present_failure(present_failed, self.renderer_device())
        {
            self.recover_renderer();
        }
    }

    /// Makes the window's renderer again once SDL lost its Direct3D 11 device (`renderer_gone`),
    /// before this frame's commands, so that nothing launches on the old device: SDL leaves its
    /// pointer behind, freed, when it cannot make another. SDL makes the device again once by
    /// itself, with every texture gone; when that fails its renderer has no device and never
    /// presents again, and the window kept its last picture with nothing on it answering
    /// (nightjar#115: the connecting screen, frozen, Cancel unanswered). A launch still connecting
    /// ends, as its decoder is on the old device, which the launch holds (`media::DeviceRef`); so
    /// do the decoder checks before one. With no renderer to be had, or a driver that loses each
    /// new one (`RemakeGuard`), Nightjar ends, loudly.
    #[cfg(windows)]
    pub(crate) fn recover_renderer(&mut self) {
        self.recovery.lost = false;
        // Direct3D's reason, from a launch's or ended stream's hold on the old device; with none,
        // SDL let it go.
        let why = self.launch_devices().chain(self.recovery.lost_devices.iter()).find_map(|device| device.removed_reason());
        let why = why.map_or(String::new(), |why| format!(" ({why})"));
        eprintln!("nightjar: the renderer's Direct3D 11 device was lost{why}; the renderer is made again");
        if let Some(connecting) = self.attempts.take() {
            eprintln!("nightjar: the launch connecting on the lost device ends");
            self.attempts.end(connecting);
            self.launcher.message(tr("The stream could not start"), tr("The graphics driver was reset, which ended the stream. Connect again."));
        }
        if let Some(probe) = self.probe.take() {
            let (launch, name) = (probe.launch(), probe.app_name().to_owned());
            eprintln!("nightjar: the decoder checks on the lost device end");
            self.ending_probes.push(probe);
            self.start_failed(launch, Some(name), None, tr("The graphics driver was reset, which ended the stream. Connect again.").to_owned());
        }
        if !self.recovery.remakes.allow(Instant::now()) {
            no_renderer(&format!(
                "the graphics driver lost its device {} times within {} seconds{why}",
                RemakeGuard::MOST,
                RemakeGuard::WITHIN.as_secs()
            ));
        }
        // Wait for any ending stream's teardown (`Session::stop`) so its decoder releases COM
        // references on the old device before SDL destroys the renderer and flushes the swap chain.
        if !self.join_teardowns(stream::LEAVE_WAIT) {
            eprintln!("nightjar: a stream's teardown is still letting go of the lost device after {} ms; continuing renderer recovery", stream::LEAVE_WAIT.as_millis());
        }
        // Waits for a settings probe and the latency benchmark, which may decode on the old device.
        stream::pyrowave_mode::settings_probe_reset();
        self.latency_bench.finish();
        self.launcher.latency_bench = self.latency_bench.state();
        self.gui.release_renderer();
        let old = self.renderer;
        let kind = sdl::renderer_name(old);
        // The colorspace it was made in: scRGB when the start asked for the GPU renderer.
        let linear = sdl::renderer_linear(old);
        // A decoder kept for a reconnect holds a reference to the old device, which the remake waits to see go.
        // Every Windows decoder holds its own: FFmpeg's D3D11VA through `DeviceRef`'s clone, and
        // PyroWave's GPU decoder through the D3D11 textures and fences it made on the device (its
        // Vulkan side is a device of its own). So a drop that is slow costs the remake's wait, not
        // a use after free, and the process goes on.
        if !stream::decoder_cache::clear(Instant::now() + stream::LEAVE_WAIT) {
            eprintln!("nightjar: a decoder kept from the last stream is still letting go of the lost device; the remake waits for its references");
        }
        // SAFETY: the renderer run() made, its gui textures freed. No stream runs in the launcher's
        // frame, and a launch or a decoder still on the old device holds that device itself.
        unsafe { crate::sdl::destroy_renderer(old) };
        // The old swap chain stays on the window, which then takes no new one, while anything
        // else holds its device (a launch's `LaunchPlan::device`, its decoder, or an ended stream):
        // Direct3D destroys it at the device's next flush. Taking `lost_devices` drops our own
        // references as soon as the flush completes, before `remake_renderer`.
        let mut devices: Vec<_> = self.launch_devices().cloned().collect();
        devices.extend(std::mem::take(&mut self.recovery.lost_devices));
        Self::flush_released(devices, Self::FLUSH_WAIT);
        let made = sdl::remake_renderer(self.window, linear, &kind, Duration::from_secs(10));
        if made.is_null() {
            no_renderer(&format!("the {kind:?} renderer could not be made again after its device was lost ({})", sdl::error()));
        }
        // SAFETY: the renderer just made; flush stale render-device events from the old one now.
        unsafe {
            SDL_SetRenderVSync(made, 1);
            SDL_FlushEvents(SDL_EVENT_RENDER_TARGETS_RESET.0, SDL_EVENT_RENDER_DEVICE_LOST.0);
        }
        self.renderer = made;
        self.gui.renderer_changed();
        self.hosts.renderer_changed();
        self.launcher.renderer_changed(&self.gui.ctx);
        #[cfg(feature = "demo-library")]
        {
            let mut hosts_view = self.hosts.ui_view();
            ui::demo::covers(&mut hosts_view, &self.gui.ctx);
            self.hosts.apply_ui_demo(hosts_view);
        }
        self.launcher.gpu_device = renderer_gpu_device(made).map(|d| d.0 as usize);
        self.launcher.tearing_supported =
            media::swapchain_allows_tearing(sdl::renderer_pointer(made, SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER));
        let device = sdl::renderer_pointer(made, SDL_PROP_RENDERER_D3D11_DEVICE_POINTER);
        // What the probes found was found on the old device, or its loss.
        self.color_probed = None;
        self.base_probed = None;
        self.base_probe_retried = None;
        self.packed_probed = None;
        self.window_changed = true;
        let gpu = gpu_adapters::renderer_gpu(device).unwrap_or_else(|| "an unknown GPU".to_owned());
        eprintln!("nightjar: renderer {} made again on {gpu}, tearing {}", sdl::renderer_name(made), self.launcher.tearing_supported);
        // If a running stream was ended by this device loss, reconnect on the fresh renderer now.
        if let Some(mut plan) = self.recovery.pending_reconnect.take()
            && self.running
        {
            eprintln!("nightjar: reconnecting {} after renderer recovery", plan.app.name);
            self.refresh_plan(&mut plan);
            let stopping = self.take_stopping();
            self.attempts.begin(Connecting::start(plan, self.waker.clone(), stopping));
        }
    }

    /// How long a remake waits for the old devices to flush what their users let go of: the flush
    /// takes a decoder's lock, which a start stuck in a driver call may hold.
    #[cfg(windows)]
    pub(super) const FLUSH_WAIT: Duration = Duration::from_secs(2);

    /// Flushes `devices` on a thread of their own, waited for at most `wait`: the remake then goes
    /// on even when a stuck decoder holds the device's lock, instead of freezing the window before
    /// it. What a device still holds when the wait ends stays on the window; the remake's tries
    /// say what they made of it.
    #[cfg(windows)]
    pub(super) fn flush_released(devices: Vec<media::DeviceRef>, wait: Duration) {
        if devices.is_empty() {
            return;
        }
        let (sender, flushed) = channel();
        let spawned = std::thread::Builder::new().name("renderer flush".into()).spawn(move || {
            for device in devices {
                device.flush_released();
            }
            let _ = sender.send(());
        });
        match spawned {
            Ok(_) => match flushed.recv_timeout(wait) {
                Ok(()) => {}
                Err(_) => eprintln!("nightjar: the old device did not flush within {} ms (a decoder still holds it); the renderer is made again without waiting", wait.as_millis()),
            },
            Err(e) => eprintln!("nightjar: the device flush thread cannot start: {e}"),
        }
    }

    /// The holds launches keep on the renderer's device (`LaunchPlan::device`): a start
    /// connecting or ending, a decoder check running or let go of, and one waiting to replace a
    /// game.
    #[cfg(windows)]
    pub(super) fn launch_devices(&self) -> impl Iterator<Item = &media::DeviceRef> {
        let connecting = self.attempts.devices();
        let probes = self.probe.iter().chain(self.ending_probes.iter()).map(LaunchProbe::device);
        connecting.chain(self.pending_replace.iter().map(|plan| &plan.device)).chain(probes)
    }

    /// The renderer's device, for PyroWave decoding on the GPU.
    pub(crate) fn gpu_device(&self) -> Option<stream::pipeline::GpuDevice> {
        #[cfg(windows)]
        if self.recovery.is_lost() {
            return None;
        }
        renderer_gpu_device(self.renderer)
    }

    /// The renderer's device as it is now (null: none): the one a launch holds and decodes on
    /// ([`media::DeviceRef`]). None while it is lost: SDL may have freed it (`recover_renderer`).
    pub(crate) fn renderer_device(&self) -> *mut std::ffi::c_void {
        self.gpu_device().map_or(std::ptr::null_mut(), |device| device.0)
    }

    pub(crate) fn hardware(&self, settings: &config::Settings) -> Hardware {
        if settings.decoder == DecoderPreference::Software {
            return Hardware::None;
        }
        #[cfg(windows)]
        if self.recovery.is_lost() {
            return Hardware::None;
        }
        if cfg!(windows) {
            if sdl::renderer_name(self.renderer) == "direct3d11" {
                let device = sdl::renderer_pointer(self.renderer, SDL_PROP_RENDERER_D3D11_DEVICE_POINTER);
                if !device.is_null() {
                    return Hardware::D3d11(device);
                }
            }
            Hardware::None
        } else {
            let gpu = renderer_gpu_device(self.renderer).map_or(std::ptr::null_mut(), |device| device.0);
            Hardware::Auto(gpu, if gpu.is_null() { self.display_vaapi_node() } else { None })
        }
    }

    /// The render node VAAPI opens on for the OpenGL renderer: the GPU its EGL display runs on,
    /// when that is not the default device (`dmabuf::renderer_vaapi_node`).
    pub(crate) fn display_vaapi_node(&self) -> Option<u32> {
        #[cfg(target_os = "linux")]
        {
            dmabuf::renderer_vaapi_node(self.renderer)
        }
        #[cfg(not(target_os = "linux"))]
        None
    }

    /// Whether VAAPI pictures go to the renderer as DMA-BUFs (`dmabuf`): its OpenGL renderer on
    /// EGL, unless `NIGHTJAR_DMABUF=0`.
    pub(crate) fn dmabuf_import(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            dmabuf::enabled_by_environment(std::env::var_os("NIGHTJAR_DMABUF").as_deref()) && dmabuf::renderer_imports(self.renderer)
        }
        #[cfg(not(target_os = "linux"))]
        false
    }
}
