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

/// SDL's events after which the renderer's device is gone: made again by SDL with every texture
/// lost (`SDL_EVENT_RENDER_DEVICE_RESET`), or not at all (`SDL_EVENT_RENDER_DEVICE_LOST`), when
/// SDL's Direct3D 11 renderer draws and presents nothing ever again.
pub(crate) fn renderer_gone(kind: SDL_EventType) -> bool {
    matches!(kind, SDL_EVENT_RENDER_DEVICE_RESET | SDL_EVENT_RENDER_DEVICE_LOST)
}

/// How often Windows' renderer may be made again (`App::recover_renderer`): a driver that loses
/// every new device would otherwise have Nightjar make it again forever, showing nothing.
#[cfg(any(windows, test))]
#[derive(Default)]
pub(crate) struct RemakeGuard(std::collections::VecDeque<Instant>);

#[cfg(any(windows, test))]
impl RemakeGuard {
    pub(crate) const MOST: usize = 5;
    pub(crate) const WITHIN: Duration = Duration::from_secs(60);

    /// Whether the renderer may be made again at `now`, counted when it may: at most `MOST` times
    /// within `WITHIN`.
    pub(crate) fn allow(&mut self, now: Instant) -> bool {
        while self.0.front().is_some_and(|&at| now.saturating_duration_since(at) >= Self::WITHIN) {
            self.0.pop_front();
        }
        if self.0.len() >= Self::MOST {
            return false;
        }
        self.0.push_back(now);
        true
    }
}

/// Whether any layer (the global settings, a PC, a game or a profile) asks for YUV 4:4:4.
#[cfg(windows)]
pub(crate) fn any_yuv444_asked(store: &config::Store, settings: &config::Settings) -> bool {
    let (hosts, profiles) = (store.hosts(), store.profiles());
    settings.prefer_yuv444
        || hosts.iter().flat_map(|host| host.settings.iter().chain(host.app_settings.values()))
            .chain(profiles.entries.iter().map(|profile| &profile.settings))
            .any(|own| own.apply(settings).prefer_yuv444)
}

/// Ends Nightjar, loudly, when it has no renderer to draw with: `why` goes to the log and a
/// message box.
fn no_renderer(why: &str) -> ! {
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
        // The quit-and-start prompt's plan holds the renderer's device, and a launch's checks may
        // still decode on it.
        let busy = self.streaming.is_some() || self.attempts.is_busy() || self.probe.is_some() || !self.ending_probes.is_empty() || self.pending_replace.is_some();
        let Some(switch) = handheld::renderer_switch(state.choice, now, need, settings.hdr && !planes, display_hdr && !planes, busy) else { return };
        if let Err(why) = self.rebuild_renderer(true, settings.hdr, display_hdr, vulkan_video, switch.reason()) {
            self.renderer_state.gpu_failed = Some(why.clone());
            self.gpu_failure = Some(why);
        }
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn switch_for_stream(&mut self, settings: &config::Settings) {
        if self.streaming.is_some() || self.attempts.is_busy() || self.probe.is_some() || !self.ending_probes.is_empty() || self.pending_replace.is_some() {
            return;
        }
        let shaders = settings.codec.is_pyrowave() || settings.experimental_packed_444
            || settings.prefer_yuv444 || settings.upscaling != config::Upscaling::Bilinear
            || (settings.decoder == DecoderPreference::Software && (settings.hdr || settings.sdr_10bit));
        if (sdl::renderer_name(self.renderer) == "gpu") != shaders
            || sdl::swapchain_takes_hdr(self.renderer) != settings.hdr
        {
            self.gpu_failure = self.rebuild_renderer(shaders, settings.hdr, true, false, "the stream format").err();
        }
    }

    /// A changed Graphics card setting, applied once with nothing streaming: Nightjar closes and
    /// starts again when it draws on the other GPU (`gpu_adapters::choose_now`).
    #[cfg(windows)]
    pub(crate) fn apply_graphics_card(&mut self) {
        let choice = self.launcher.settings.graphics_card;
        if choice == self.graphics_card || self.streaming.is_some() || self.pending_replace.is_some() {
            return;
        }
        // A start still connecting uses the device it would replace; a start that was cancelled or
        // timed out and whose worker has not let go of the graphics card blocks the change, and
        // says so once (it applies when that worker ends).
        if self.attempts.is_connecting() || self.probe.is_some() {
            return;
        }
        if self.attempts.is_busy() {
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
        self.graphics_card = choice;
        if gpu_adapters::choose_now(choice, any_yuv444_asked(&self.store, &self.launcher.settings), self.store.dir()) {
            // The new start reads the choice back, whatever order this frame's commands run in.
            if let Err(e) = self.store.save_settings(&self.launcher.settings) {
                eprintln!("nightjar: saving settings failed: {e}");
            }
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
        if self.attempts.is_busy() || !self.ending_probes.is_empty() {
            if self.renderer_choice_blocked != Some(choice) {
                self.renderer_choice_blocked = Some(choice);
                self.launcher.notify(
                    tr("Renderer not changed"),
                    tr("A start that has ended is still using the graphics card, so it cannot be switched yet. The change is kept and applied when that start stops; if it does not stop, restart Nightjar."),
                );
            }
            return;
        }
        self.renderer_choice_blocked = None;
        self.renderer_state.choice = choice;
        let Some(gpu) = handheld::renderer_for_choice(choice, sdl::renderer_name(self.renderer) == "gpu") else { return };
        match self.rebuild_renderer(gpu, false, false, self.renderer_state.vulkan_video, "the Renderer setting") {
            Ok(()) if gpu => self.renderer_state.gpu_failed = None,
            Ok(()) => {}
            Err(why) => {
                if gpu {
                    self.renderer_state.gpu_failed = Some(why.clone());
                }
                self.launcher.message(tr("Renderer"), trf("The renderer could not be made: {}.", &[&why]));
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
    fn rebuild_renderer(&mut self, gpu: bool, hdr: bool, display_hdr: bool, vulkan_video: bool, reason: &str) -> Result<(), String> {
        let from = sdl::renderer_name(self.renderer);
        let wanted = if gpu { "gpu" } else if cfg!(target_os = "macos") { "metal" } else { "opengl" };
        // Waits for a settings probe still on the old device, and forgets what it found.
        stream::pyrowave_mode::settings_probe_reset();
        // And for the latency benchmark's decoders, which may decode on the old device.
        self.launcher.latency_bench.finish();
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
        #[cfg(feature = "demo-library")]
        ui::demo::covers(&mut self.hosts, &self.gui.ctx);
        self.launcher.renderer_changed(&self.gui.ctx);
        self.launcher.gpu_device = renderer_gpu_device(made);
        self.color_probed = None;
        self.base_probed = None;
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
                Err(why)
            }
        }
    }

    /// A render device event (`renderer_gone`), from the launcher's loop or a stream's: Windows
    /// makes the renderer again in the launcher's next frame (`recover_renderer`).
    pub(crate) fn note_render_device(&mut self, kind: SDL_EventType) {
        let gone = renderer_gone(kind);
        #[cfg(windows)]
        if gone {
            self.renderer_lost = true;
        }
        #[cfg(not(windows))]
        let _ = gone;
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
        // Direct3D's reason, from a launch's hold on the old device; with none, SDL let it go.
        let why = self.launch_devices().find_map(|device| device.removed_reason());
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
        if !self.remakes.allow(Instant::now()) {
            no_renderer(&format!(
                "the graphics driver lost its device {} times within {} seconds{why}",
                RemakeGuard::MOST,
                RemakeGuard::WITHIN.as_secs()
            ));
        }
        // Waits for a settings probe and the latency benchmark, which may decode on the old device.
        stream::pyrowave_mode::settings_probe_reset();
        self.launcher.latency_bench.finish();
        self.gui.release_renderer();
        let old = self.renderer;
        let kind = sdl::renderer_name(old);
        // The colorspace it was made in: scRGB when the start asked for the GPU renderer.
        let linear = sdl::renderer_linear(old);
        // SAFETY: the renderer run() made, its gui textures freed. No stream runs in the launcher's
        // frame, and a launch or a decoder still on the old device holds that device itself.
        unsafe { crate::sdl::destroy_renderer(old) };
        // The old swap chain stays on the window, which then takes no new one, while anything
        // else holds its device (a launch's `LaunchPlan::device`, its decoder): Direct3D destroys
        // it at the device's next flush. A stream's decoder lets go of it as the stream ends; the
        // remake's tries wait for one that is slow to.
        let devices: Vec<_> = self.launch_devices().cloned().collect();
        Self::flush_released(devices, Self::FLUSH_WAIT);
        let made = sdl::remake_renderer(self.window, linear, &kind, Duration::from_secs(10));
        if made.is_null() {
            no_renderer(&format!("the {kind:?} renderer could not be made again after its device was lost ({})", sdl::error()));
        }
        // SAFETY: the renderer just made.
        unsafe { SDL_SetRenderVSync(made, 1) };
        self.renderer = made;
        self.gui.renderer_changed();
        self.hosts.renderer_changed();
        #[cfg(feature = "demo-library")]
        ui::demo::covers(&mut self.hosts, &self.gui.ctx);
        self.launcher.renderer_changed(&self.gui.ctx);
        self.launcher.gpu_device = renderer_gpu_device(made);
        self.launcher.tearing_supported =
            media::swapchain_allows_tearing(sdl::renderer_pointer(made, SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER));
        let device = sdl::renderer_pointer(made, SDL_PROP_RENDERER_D3D11_DEVICE_POINTER);
        // What the probes found was found on the old device, or its loss.
        self.color_probed = None;
        self.base_probed = None;
        self.packed_probed = None;
        self.window_changed = true;
        // SAFETY: on the main thread. What the old renderer queued is about it.
        unsafe { SDL_FlushEvents(SDL_EVENT_RENDER_TARGETS_RESET.0, SDL_EVENT_RENDER_DEVICE_LOST.0) };
        let gpu = gpu_adapters::renderer_gpu(device).unwrap_or_else(|| "an unknown GPU".to_owned());
        eprintln!("nightjar: renderer {} made again on {gpu}, tearing {}", sdl::renderer_name(made), self.launcher.tearing_supported);
    }

    /// How long a remake waits for the old devices to flush what their users let go of: the flush
    /// takes a decoder's lock, which a start stuck in a driver call may hold.
    #[cfg(windows)]
    const FLUSH_WAIT: Duration = Duration::from_secs(2);

    /// Flushes `devices` on a thread of their own, waited for at most `wait`: the remake then goes
    /// on even when a stuck decoder holds the device's lock, instead of freezing the window before
    /// it. What a device still holds when the wait ends stays on the window; the remake's tries
    /// say what they made of it.
    #[cfg(windows)]
    fn flush_released(devices: Vec<media::DeviceRef>, wait: Duration) {
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
    fn launch_devices(&self) -> impl Iterator<Item = &media::DeviceRef> {
        let connecting = self.attempts.devices();
        let probes = self.probe.iter().chain(self.ending_probes.iter()).map(LaunchProbe::device);
        connecting.chain(self.pending_replace.iter().map(|plan| &plan.device)).chain(probes)
    }

    /// The renderer's device, for PyroWave decoding on the GPU.
    pub(crate) fn gpu_device(&self) -> Option<stream::pipeline::GpuDevice> {
        #[cfg(windows)]
        if self.renderer_lost {
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
        if self.renderer_lost {
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
