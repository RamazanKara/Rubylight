//! A running stream: its start once connected, the second screen, its frames and events, and its end.

use super::*;

#[cfg(windows)]
#[link(name = "winmm")]
extern "system" {
    fn timeBeginPeriod(period: u32) -> u32;
    fn timeEndPeriod(period: u32) -> u32;
}

/// 1 ms scheduler resolution while streaming, so pictures go out on time.
pub(crate) struct TimerResolution;

impl TimerResolution {
    pub(crate) fn begin() -> Self {
        #[cfg(windows)]
        // SAFETY: paired with timeEndPeriod in Drop.
        unsafe {
            timeBeginPeriod(1);
        }
        Self
    }
}

impl Drop for TimerResolution {
    fn drop(&mut self) {
        #[cfg(windows)]
        // SAFETY: paired with timeBeginPeriod.
        unsafe {
            timeEndPeriod(1);
        }
    }
}

/// How long the message of a stream the PC failed waits for the PC's reason.
const REASON_WAIT: Duration = Duration::from_secs(5);

/// The end of a stream the PC failed, shown once the poll `seq` of PC `host` answered.
pub(crate) struct PendingEnd {
    pub(crate) host: u64,
    pub(crate) seq: u64,
    ended: Option<String>,
    pub(crate) health: Option<String>,
    pub(crate) until: Instant,
}

impl App {
    pub(crate) fn begin_stream(&mut self, connecting: Connecting, session: nightjar_core::session::Session, facts: stream::Facts) {
        if let Some(auto) = self.autostream.as_mut() { auto.streaming_began(); }
        if let Some(cli) = self.cli.as_mut() { cli.streaming_began(); }
        let settings = connecting.settings.clone();
        self.pads.configure(&settings);
        let display = display_of(self.window);
        let pacing = connecting.pacing;
        eprintln!(
            "nightjar: streaming {}x{}@{} {} at {} kbps, pacing {:?}",
            facts.width,
            facts.height,
            facts.fps,
            stream::codec_name(facts.format),
            facts.bitrate_kbps,
            pacing
        );
        // Without V-Sync the GPU renderer tears where it can (`prefers_immediate`); the property
        // holds for the stream, so the VRR pacing's later interval changes follow it too.
        // A VRR stream without Tear-free VRR accepts tearing (`wl_scanout::vrr_tearing`).
        #[cfg(target_os = "linux")]
        sdl::set_vrr_tearing(stream::wl_scanout::vrr_tearing(settings.vrr_tear_free, stream::wl_scanout::env_on(stream::wl_scanout::VRR_TEARING_ENV)));
        // "HDR brightness follows the desktop": an installed description follows it in place.
        #[cfg(target_os = "linux")]
        stream::wl_scanout::set_brightness(stream::wl_scanout::Brightness::of_setting(settings.hdr_desktop_brightness));
        // The stream draws at 1.0: the launcher's white scale is the launcher's alone.
        #[cfg(target_os = "linux")]
        sdl::launcher_white(self.renderer, false);
        #[cfg(target_os = "linux")]
        sdl::prefer_immediate(self.renderer, stream::present::prefers_immediate(connecting.vrr(), sdl::video_driver() == "wayland", handheld::gamescope()));
        // SAFETY: window and renderer calls on the main thread.
        unsafe {
            SDL_SetRenderVSync(self.renderer, stream::present::sync_interval(pacing, stream::present::VrrSync::Off));
            #[cfg(target_os = "linux")]
            eprintln!("nightjar: present mode {}", sdl::present_mode_name(sdl::present_mode(self.renderer)));
            let wanted = if facts.role == nightjar_core::http::AppRole::InputOnly { WindowMode::Windowed } else { settings.window_mode };
            match stream_window_mode(wanted, handheld::gamescope(), kms::active()) {
                WindowMode::Fullscreen => {
                    let mut mode: SDL_DisplayMode = std::mem::zeroed();
                    let id = SDL_GetDisplayForWindow(self.window);
                    if SDL_GetClosestFullscreenDisplayMode(
                        id,
                        facts.width,
                        facts.height,
                        fullscreen_refresh_hz(connecting.vrr(), &mode_refresh_rates(id), facts.fps),
                        true,
                        &mut mode,
                    ) {
                        SDL_SetWindowFullscreenMode(self.window, &mode);
                    } else {
                        SDL_SetWindowFullscreenMode(self.window, std::ptr::null());
                    }
                    SDL_SetWindowFullscreen(self.window, true);
                }
                WindowMode::Borderless => {
                    SDL_SetWindowFullscreenMode(self.window, std::ptr::null());
                    SDL_SetWindowFullscreen(self.window, true);
                }
                WindowMode::Windowed => {}
            }
            SDL_SyncWindow(self.window);
        }
        self.remember_recent(&connecting);
        // A stream with full chroma: what its PC offered is kept for the next launch there.
        let outcome = session.outcome();
        let (packed, describe_modes) = (outcome.config.experimental_packed_444, outcome.host_packed444_modes);
        if let Some(host) = self.hosts.get_mut(connecting.host_id) {
            if full_chroma::started(&mut host.record, packed, describe_modes) { self.save_hosts(); }
        }
        let mut streaming = Streaming::new(connecting, session, facts, display);
        streaming.watch_microphone_busy(self.waker.clone());
        self.pads.prepare_audio(settings.controllers && settings.controller_audio && streaming.facts.role != nightjar_core::http::AppRole::RemoteMonitor);
        streaming.display_changed(self.renderer);
        if settings.adaptive_bitrate || streaming.quality_mode != config::QualityMode::Off { streaming.enable_adaptive_bitrate(true); }
        if settings.controllers && streaming.facts.role != nightjar_core::http::AppRole::RemoteMonitor {
            streaming.session.input(|q| self.pads.announce_all(q));
        }
        // SAFETY: a live window.
        let focused = unsafe { SDL_GetWindowFlags(self.window) & SDL_WINDOW_INPUT_FOCUS != 0 };
        if focused {
            streaming.set_capture(self.window, true);
        } else {
            streaming.capture_on_focus();
        }
        let (pyrowave, notices_changed) = match self.hosts.get_mut(streaming.host_id) {
            Some(host) => stream::pyrowave_notice(&mut host.record, settings.codec.is_pyrowave(),
                streaming.facts.pyrowave_fallback.as_deref(), streaming.facts.format),
            None => (None, false),
        };
        let (frame_warp, frame_warp_changed) = match self.hosts.get_mut(streaming.host_id) {
            Some(host) => stream::frame_warp_notice(&mut host.record, streaming.facts.frame_warp.is_some(), streaming.facts.frame_warp_refused),
            None => (None, false),
        };
        if notices_changed || frame_warp_changed {
            self.save_hosts();
        }
        let hdr_note = std::mem::take(&mut self.hdr_note).then(|| tr(HDR_OFF_NOTE));
        #[cfg(target_os = "linux")]
        let decoder_notes = self.linux_stream_start(&settings, pacing == stream::present::PacingMode::Vsync);
        #[cfg(not(target_os = "linux"))]
        let decoder_notes: Vec<String> = Vec::new();
        let packed_note = self.packed_note.take();
        let kms_notes = std::mem::take(&mut self.kms_notes);
        let notices: Vec<&str> = hdr_note.into_iter().chain(kms_notes.iter().map(String::as_str)).chain(packed_note.as_deref()).chain(decoder_notes.iter().map(String::as_str)).chain(pyrowave.as_deref())
            .chain(streaming.facts.codec_fallback.as_deref()).chain(streaming.facts.color_notice.as_deref()).chain(frame_warp.as_deref()).collect();
        let notices = notices.join("\n");
        let network = &self.launcher.network;
        let profiles = &self.launcher.profiles;
        let selected = profiles.selected(network.kind(&profiles.home_wifi));
        let warning = if network.error.is_some() && (profiles.automatic || profiles.mobile_limit_kbps > 0) {
            Some(tr("Network details unavailable; automatic profiles and the mobile data limit could not be applied."))
        } else if selected.is_some_and(|id| !profiles.entries.iter().any(|p| p.id == id)) {
            Some(tr("The selected profile is missing; main settings and PC/game overrides apply."))
        } else if profiles.automatic && profiles.home.is_some() && !profiles.home_wifi.is_empty() && network.wifi && network.ssid.is_none() {
            Some(tr("Wi-Fi name unavailable; using the other Wi-Fi profile."))
        } else { None };
        let toast = start_toast(warning, (!notices.is_empty()).then_some(notices.as_str()), handheld::gamescope());
        streaming.show_toast(toast);
        // Straight to the display, the pictures go on a display plane from the first one.
        #[cfg(target_os = "linux")]
        {
            let wishes = stream::kms_scanout::Wishes { tearing: pacing != stream::present::PacingMode::Vsync, tear_free: settings.vrr_tear_free, vrr: settings.vrr, fps: streaming.facts.fps.max(1) as u32 };
            streaming.kms = stream::kms_scanout::Scanout::open(self.window, &sdl::renderer_name(self.renderer), &settings, wishes);
        }
        self.streaming = Some((streaming, TimerResolution::begin()));
        self.start_second_screen(&settings);
    }

    /// The `linux:` line, the present mode for the `video path:` line, and the toasts naming what
    /// hardware decoding is missing (`linux_diagnosis`).
    #[cfg(target_os = "linux")]
    fn linux_stream_start(&self, settings: &config::Settings, vsync: bool) -> Vec<String> {
        let renderer = sdl::renderer_name(self.renderer);
        let device = sdl::renderer_pointer(self.renderer, SDL_PROP_RENDERER_GPU_DEVICE_POINTER);
        let present = if renderer == "gpu" && !device.is_null() {
            // SAFETY: The renderer's live GPU device and its window, on the main thread.
            let supports = |mode| unsafe { SDL_WindowSupportsGPUPresentMode(device.cast(), self.window, mode) };
            format!("present {}", linux_diagnosis::gpu_present_mode(vsync, supports(SDL_GPU_PRESENTMODE_MAILBOX), supports(SDL_GPU_PRESENTMODE_IMMEDIATE)))
        } else {
            format!("{renderer} renderer, V-Sync {}", if vsync { "on" } else { "off" })
        };
        linux_diagnosis::stream_start(self.hardware(settings), &present)
    }

    /// Dual screen mode: a stream of a game, with a second monitor and a PC that keeps a second
    /// stream beside the first, starts the second screen's process (`second_screen`).
    fn start_second_screen(&mut self, settings: &config::Settings) {
        if !settings.dual_screen || second_screen::this_process() || self.second_screen.is_some() {
            return;
        }
        let Some((streaming, _)) = &self.streaming else { return };
        if streaming.mic_only() || streaming.facts.role != nightjar_core::http::AppRole::Stream {
            return;
        }
        let Some(host) = self.hosts.get(streaming.host_id) else { return };
        let target = if !host.info.as_ref().is_some_and(|i| i.capabilities.has(nightjar_core::http::Feature::SecondScreen)) {
            Err("this PC does not keep a second stream beside the first".to_owned())
        } else {
            // SAFETY: a live window.
            let main = unsafe { SDL_GetDisplayForWindow(self.window) };
            let apps = match &host.apps { Apps::Loaded(apps) => apps.as_slice(), _ => &[] };
            // A joined game is another device's, which the PC does not report as running to this one.
            let joined = streaming.joined().then(|| streaming.app().id);
            let running = second_screen::joined_running(host.running_app().map(|id| (id, host.info.as_ref().and_then(|i| i.current_game_uuid.as_deref()))), joined);
            second_screen::pick_display(&window_place::display_bounds(), main)
                .ok_or_else(|| "no second monitor".to_owned())
                .and_then(|point| {
                    let (pc, game) = second_screen::target(&host.record, streaming.app(), apps, running)?;
                    // This stream got past the PIN lock: the second screen's does not ask again.
                    let hand_over = match self.launcher.pin.as_ref().map(pin_ui::PinLock::hand_over) {
                        Some(Ok(Some(token))) => token,
                        Some(Ok(None)) | None => String::new(),
                        Some(Err(e)) => {
                            eprintln!("nightjar: dual screen: the PIN lock cannot be handed to the second screen ({e}); it asks for the PIN there");
                            String::new()
                        }
                    };
                    second_screen::Companion::start(&pc, &game, point, joined, &hand_over)
                        .map(|companion| (companion, pc, game, point))
                        .map_err(|e| format!("could not start: {e}"))
                })
        };
        match target {
            Ok((companion, pc, game, point)) => {
                eprintln!("nightjar: dual screen: the second screen streams {game} from {pc} on the monitor at {},{}", point.0, point.1);
                self.second_screen = Some(companion);
            }
            Err(why) => {
                eprintln!("nightjar: dual screen: no second screen: {why}");
                if let Some((streaming, _)) = self.streaming.as_mut() {
                    streaming.show_toast(tr("The second screen could not be opened."));
                }
            }
        }
    }

    /// The second screen's process has ended or is ending: the PIN lock it was handed opens
    /// nothing any more.
    pub(crate) fn second_screen_gone(&self) {
        if let Some(pin) = &self.launcher.pin {
            pin.end_hand_over();
        }
    }

    /// The second screen ends with the first screen's stream, a reconnect aside, and a second
    /// screen that failed says so.
    pub(crate) fn second_screen_step(&mut self) {
        let Some(companion) = self.second_screen.as_mut() else { return };
        if self.streaming.is_none() && self.connecting.is_none() {
            if let Some(companion) = self.second_screen.take() {
                companion.end();
                self.second_screen_gone();
            }
            return;
        }
        let failed = match companion.exited() {
            None => return,
            Some(Ok(status)) if status.success() => {
                eprintln!("nightjar: dual screen: the second screen closed");
                false
            }
            Some(Ok(status)) => {
                eprintln!("nightjar: dual screen: the second screen failed ({status}); its log is in logs/second-screen");
                true
            }
            Some(Err(e)) => {
                eprintln!("nightjar: dual screen: the second screen cannot be watched: {e}");
                true
            }
        };
        self.second_screen = None;
        self.second_screen_gone();
        if failed {
            if let Some((streaming, _)) = self.streaming.as_mut() {
                streaming.show_toast(tr("The second screen could not be opened."));
            }
        }
    }

    /// Notes the game of a stream that just started, for Home's "Recently played".
    fn remember_recent(&mut self, connecting: &Connecting) {
        let Some(entry) = connecting.recent_game() else { return };
        config::remember(&mut self.launcher.recents, entry);
        if let Err(e) = self.store.save_recents(&self.launcher.recents) {
            eprintln!("nightjar: could not save recent games: {e}");
        }
    }

    /// Shows the message of a stream the PC failed once the poll after it answered, with the
    /// PC's reason, or after `REASON_WAIT` without it.
    pub(crate) fn show_pending_end(&mut self) {
        let Some(pending) = &self.pending_end else { return };
        let answered = self.hosts.failure_after(pending.host, pending.seq);
        if answered.is_none() && Instant::now() < pending.until {
            return;
        }
        let Some(pending) = self.pending_end.take() else { return };
        if let Some((title, body)) = stream::doctor::after_stream(pending.ended, pending.health) {
            // "Now playing on" may have come first; this stream's report waits for it.
            self.launcher.ask_after(Dialog::Message { title: title.into(), body: stream::doctor::with_host_reason(body, answered.flatten().as_deref()) });
        }
    }

    pub(crate) fn end_stream(&mut self, exit: Exit) {
        // Before the overlay plane's renderer goes with the stream.
        self.gui_back_from_overlay();
        let Some((mut streaming, _timer)) = self.streaming.take() else {
            return;
        };
        let host = streaming.host_id;
        // The launcher didn't see the window move while the stream had it.
        self.gui.set_refresh(display_of(self.window).refresh_hz);
        self.open_play.left_by_player = matches!(exit, Exit::User | Exit::CloseWindow);
        // Microphone only has no video; it is going once connected.
        let reconnect = if matches!(exit, Exit::Reconnect) || exit.retryable(streaming.video_started() || streaming.mic_only()) {
            // SAFETY: the window lives as long as the app.
            let away = unsafe { SDL_GetWindowFlags(self.window) & (SDL_WINDOW_MINIMIZED | SDL_WINDOW_HIDDEN) != 0 };
            streaming.reconnect_plan(!matches!(exit, Exit::Reconnect), away)
        } else { None };
        // The stream doctor's report when it found something, unless the stream goes on.
        let health = streaming.health_summary().filter(|_| reconnect.is_none());
        eprintln!(
            "nightjar: stream ended ({})",
            match &exit {
                Exit::User => "left by the user".to_owned(),
                Exit::CloseWindow => "window closed".to_owned(),
                Exit::Reconnect => "reconnecting".to_owned(),
                Exit::NetworkReturned => "the network is back where the connection cannot carry on, reconnecting".to_owned(),
                Exit::Terminated(code) => format!("host or transport error {code}"),
                Exit::Ended(None) => "ended by the PC".to_owned(),
                Exit::Ended(Some(m)) => m.clone(),
            }
        );
        // Quit on leave: only a leave the player asked for quits the game; a stream that is still
        // up when it ends that way was connected. The second screen's leave never does: the
        // game is the first screen's too.
        if matches!(exit, Exit::User) && !second_screen::this_process() {
            let app = streaming.app();
            // Quit game confirmed in the stream menu, or the launch's own choice to quit on leaving.
            // moonlight-qt's --quit-after or --no-quit-after on the command line stands for the game's own choice.
            let given = self.cli.as_ref().filter(|cli| cli.host == host).and_then(|cli| cli.launch.options.quit_after);
            let chosen = streaming.quit_on_leave || given.unwrap_or_else(|| self.hosts.get(host).is_some_and(|h| h.record.quits_on_leave(&app.uuid, app.id)));
            if nightjar_policy::launch::quits_on_leave(chosen, ui::policy_role(streaming.facts.role), true) {
                eprintln!("nightjar: quitting {} on the PC, as chosen for leaving its stream", app.name);
                self.launcher.toast(trf("Quitting {} on the PC…", &[&app.name]));
                self.quitting_on_leave = Some((host, app.name.clone()));
                self.hosts.quit_app(host);
            }
        }
        let streaming_app = streaming.app().id;
        // Settings → Advanced → Export latency CSV saves this file: the measurements of the last
        // stream, written once now.
        let csv = self.store.dir().join("statistics").join(stream::LATENCY_CSV);
        if let Err(e) = streaming.keep_latency_csv(&csv) {
            eprintln!("nightjar: latency CSV {}: {e}", csv.display());
        }
        // Show decode time after streaming, Android's post-stream toast; a reconnect isn't an end.
        let decode_time = streaming.decode_time_line().filter(|_| self.launcher.settings.post_stream_decode_time && reconnect.is_none());
        streaming.finish(self.window, &mut self.pads);
        // The stream's cursor shows the pointer again on leaving.
        self.pointer.shown();
        // SAFETY: window and renderer calls on the main thread.
        unsafe {
            if handheld::start_window(handheld::fills_screen()).windowed_after_stream {
                SDL_SetWindowFullscreen(self.window, false);
            }
            SDL_SetRenderVSync(self.renderer, 1);
        }
        self.hosts.refresh_after_exit(host);
        // A reconnect asks the PC itself before it launches.
        if reconnect.is_none() && self.running {
            self.hosts.read_stream_move(host, streaming_app, matches!(exit, Exit::User | Exit::CloseWindow));
        }
        if let Some(mut plan) = reconnect {
            self.refresh_plan(&mut plan);
            self.cancelled = false;
            self.connecting = Some(Connecting::start(plan, self.waker.clone()));
        } else {
        // A failure the PC ended the stream on: the message waits for the poll just started,
        // which carries the PC's own words for it where it says them.
        let reason_poll = match exit {
            Exit::Terminated(code) if stream::host_ended_stream(code) => self.hosts.polling_seq(host),
            _ => None,
        };
        let ended = match exit {
            Exit::CloseWindow => { self.running = false; None }
            Exit::User | Exit::Reconnect | Exit::Ended(None) => None,
            Exit::Terminated(code) => stream::termination_text(code),
            // Not reconnecting after all (minimized since the network came back): the connection is lost.
            Exit::NetworkReturned => stream::termination_text(nightjar_core::session::error::FAILED),
            Exit::Ended(Some(message)) => Some(message),
        };
        match reason_poll {
            Some(seq) if self.running => {
                self.pending_end = Some(PendingEnd { host, seq, ended, health, until: Instant::now() + REASON_WAIT });
            }
            _ => {
                if let Some((title, body)) = stream::doctor::after_stream(ended, health).filter(|_| self.running) {
                    self.launcher.message(title, body);
                }
            }
        }
        }
        if let Some(line) = decode_time.filter(|_| self.running) {
            eprintln!("nightjar: {line}");
            self.launcher.toast(line);
        }
        // egui forgets held modifiers from before the stream.
        self.gui.push(egui::Event::WindowFocused(true));
        self.launcher.refocus();
    }

    pub(crate) fn stream_frame(&mut self) {
        // A second screen that fails during the stream says so on it; the launcher's frames
        // only run again once the stream has ended.
        self.second_screen_step();
        let window = self.window;
        let renderer = self.renderer;
        let display = display_of(window);
        // The stream menu animates at most once per refresh.
        let menu_frame = Duration::from_secs_f32(1.0 / display.refresh_hz.max(30.0));
        let Some((streaming, _)) = self.streaming.as_mut() else {
            return;
        };
        // SAFETY: the window lives as long as the app.
        let away = unsafe { SDL_GetWindowFlags(window) & (SDL_WINDOW_MINIMIZED | SDL_WINDOW_HIDDEN) != 0 };
        streaming.drain_notices(&mut self.pads, away);
        streaming.open_guide_menu(window, &mut self.pads);
        streaming.follow_text_field(window);
        streaming.follow_fullscreen(window);
        #[cfg(windows)]
        streaming.follow_tab_chord(window);
        streaming.pump();
        streaming.follow_display(display);
        if streaming.export_statistics {
            streaming.save_statistics(&self.store.dir().join("statistics"));
        }
        // Wait for input, a decoded picture or the next present time.
        // SAFETY: event calls on the main thread.
        unsafe {
            let mut event: SDL_Event = std::mem::zeroed();
            match streaming.wait() {
                stream::Wait::Now => {}
                stream::Wait::For(duration) => {
                    // A held direction in the stream menu wakes the loop for its next step.
                    let duration = self.nav_repeat.next(Instant::now()).map_or(duration, |next| duration.min(next + Duration::from_millis(1)));
                    // An animating menu wakes it for egui's next frame, at most once per refresh.
                    let duration = match self.gui.repaint_in(menu_frame) {
                        Some(wait) if streaming.menu_visible => duration.min(wait),
                        _ => duration,
                    };
                    if duration > Duration::from_micros(1500) {
                        let ms = (duration.as_micros() / 1000).saturating_sub(1).max(1) as i32;
                        if SDL_WaitEventTimeout(&mut event, ms) {
                            self.dispatch_stream_event(&event);
                        }
                    } else {
                        SDL_DelayPrecise(duration.as_nanos() as u64);
                    }
                }
            }
            while SDL_PollEvent(&mut event) {
                self.dispatch_stream_event(&event);
            }
        }
        // A held D-pad or stick steps the stream menu again, as in the launcher.
        if self.streaming.as_ref().is_some_and(|(s, _)| s.menu_visible) {
            if self.repeat_navigation() {
                if let Some((s, _)) = self.streaming.as_mut() {
                    s.needs_redraw = true;
                }
            }
        } else {
            self.nav_repeat.clear();
        }
        let Some((streaming, _)) = self.streaming.as_mut() else {
            return;
        };
        // Without a new picture (a still desktop, a paused game) the menu draws when egui asks.
        if streaming.menu_visible && self.gui.repaint_in(menu_frame).is_some_and(|wait| wait.is_zero()) {
            streaming.needs_redraw = true;
        }
        streaming.pump_touchpad(window);
        streaming.pump();
        if let Some(host) = self.hosts.get(streaming.host_id) {
            streaming.hints = host.native.session_hints();
        }
        if streaming.session_page_requested {
            let link = self.launcher.network.description(&self.launcher.profiles.home_wifi);
            streaming.open_session_page(self.waker.clone(), link);
        }
        if streaming.update_overlay() && streaming.overlay_visible {
            streaming.needs_redraw = true;
        }
        if let Some((shown, mode, corner)) = streaming.take_overlay_change() {
            let s = &mut self.launcher.settings;
            s.overlay = shown.unwrap_or(s.overlay);
            s.overlay_mode = mode.unwrap_or(s.overlay_mode);
            s.overlay_position = corner.unwrap_or(s.overlay_position);
            if let Err(e) = self.store.save_settings(s) {
                eprintln!("nightjar: saving settings failed: {e}");
            }
        }
        // A device ticked or unticked for automatic USB forwarding in the stream controls.
        if let Some(auto) = streaming.usb.as_mut().and_then(crate::usb::Panel::take_auto_change) {
            self.launcher.settings.usb_auto = auto;
            if let Err(e) = self.store.save_settings(&self.launcher.settings) {
                eprintln!("nightjar: saving settings failed: {e}");
            }
        }
        if let Some((_, at)) = &streaming.toast {
            // The lost-network notice stays until a network returns.
            if at.elapsed() > stream::TOAST_SHOWN && !streaming.network_lost {
                streaming.toast = None;
                streaming.needs_redraw = true;
            }
        }
        let gui = &mut self.gui;
        // The input filter sends controller input while the present waits (direct_motion.rs).
        let pads = streaming.direct_motion.lend(std::mem::take(&mut self.pads));
        let shortcuts = &mut self.launcher.shortcuts;
        let screenshot = std::mem::take(&mut self.screenshot_requested);
        sdl::read_back_frames(renderer, screenshot);
        let dir = self.screenshot_dir.clone();
        let mut shortcuts_changed = false;
        let main_renderer = self.renderer;
        let gui_overlay = &mut self.gui_overlay;
        streaming.present(renderer, |s, renderer| {
            // The overlay draws into a display plane's own renderer while the planes show the
            // stream (`kms_scanout`): the gui's textures move to the renderer it draws with.
            let target = if renderer == main_renderer { std::ptr::null_mut() } else { renderer };
            if target != *gui_overlay {
                gui.release_renderer();
                gui.renderer_changed();
                *gui_overlay = target;
            }
            // In a video-only frame on a PQ output the overlay's meshes write PQ themselves.
            gui.set_window_state(s.ui_state(renderer));
            shortcuts_changed = draw_overlay(gui, window, renderer, s, &mut pads.pads(), shortcuts);
            gui.set_window_state(std::ptr::null_mut());
            if screenshot {
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis());
                let path = dir.join(format!("nightjar-stream-{stamp}.png"));
                match sdl::screenshot(renderer, &path) {
                    Ok(()) => eprintln!("nightjar: screenshot {}", path.display()),
                    Err(e) => eprintln!("nightjar: screenshot failed: {e}"),
                }
            }
        });
        self.pads = pads.take_back();
        // A shortcut saved or deleted in the stream menu lands in the same file as the settings' list.
        if shortcuts_changed {
            if let Err(e) = self.store.save_shortcuts(&self.launcher.shortcuts) {
                streaming.show_toast(trf("Could not save shortcuts: {}", &[&e]));
            }
        }
        // No picture for 12 s after the start means the stream never came up.
        if streaming.facts.role != nightjar_core::http::AppRole::InputOnly
            && !streaming.has_picture() && streaming.since_start() > Duration::from_secs(12) && streaming.exit.is_none() {
            streaming.exit = Some(Exit::Terminated(nightjar_core::session::error::NO_VIDEO_TRAFFIC));
        }
        if let Some(auto) = &self.autostream {
            if streaming.exit.is_none() && auto.ran_its_time() {
                streaming.exit = Some(Exit::User);
            }
        }
        if let Some(exit) = streaming.exit.take() {
            let planned = matches!(exit, Exit::User);
            self.end_stream(exit);
            // A dropped stream may reconnect (end_stream started it); a failed reconnect ends the
            // run through its message, in autostream_step.
            let reconnecting = self.connecting.is_some();
            if let Some(auto) = self.autostream.as_mut() {
                if !auto.stream_ended(planned, reconnecting) { self.running = false; }
            }
        }
    }

    fn dispatch_stream_event(&mut self, event: &SDL_Event) {
        #[cfg(target_os = "macos")]
        if self.take_pairing_url(event) { return; }
        self.dispatch_stream_event_inner(event);
        // A click or wheel turn has gone out (or the menu took it): later motion may pass it.
        if let Some((s, _)) = self.streaming.as_ref() {
            s.direct_motion.dispatched(event);
        }
    }

    fn dispatch_stream_event_inner(&mut self, event: &SDL_Event) {
        self.track_visibility(event);
        // SAFETY: reading the event type.
        let kind = SDL_EventType(unsafe { event.r#type });
        // A stream keeps the night look; the launcher it returns to has the system's theme.
        if kind == SDL_EVENT_SYSTEM_THEME_CHANGED {
            self.launcher.system_light = sdl::system_light();
        }
        if kind.0 == self.wake_event {
            // The Session page's worker published something new.
            if let Some((s, _)) = self.streaming.as_mut() {
                if s.session_page.is_some() { s.needs_redraw = true; }
            }
            return;
        }
        // SAFETY: a key event's members.
        let key_action = (kind == SDL_EVENT_KEY_DOWN).then(|| unsafe { shortcuts::key_action(Scope::Stream, event.key.scancode, event.key.r#mod) }).flatten();
        // SAFETY: a key event's members.
        let repeat = kind == SDL_EVENT_KEY_DOWN && unsafe { event.key.repeat };
        if key_action == Some(Action::Screenshot) {
            self.screenshot_requested = true;
            if let Some((s, _)) = self.streaming.as_mut() {
                s.needs_redraw = true;
            }
            return;
        }
        log_window_hdr(event);
        if matches!(
            kind,
            SDL_EVENT_WINDOW_RESIZED
                | SDL_EVENT_WINDOW_PIXEL_SIZE_CHANGED
                | SDL_EVENT_WINDOW_EXPOSED
                | SDL_EVENT_WINDOW_HDR_STATE_CHANGED
                | SDL_EVENT_WINDOW_DISPLAY_CHANGED
        ) {
            if let Some((s, _)) = self.streaming.as_mut() {
                s.needs_redraw = true;
            }
        }
        let Some((streaming, _)) = self.streaming.as_mut() else {
            return;
        };
        if matches!(kind, SDL_EVENT_WINDOW_HDR_STATE_CHANGED | SDL_EVENT_WINDOW_DISPLAY_CHANGED | SDL_EVENT_DISPLAY_CURRENT_MODE_CHANGED) {
            streaming.display_changed(self.renderer);
        }
        if streaming.menu_visible {
            let back_button = if self.pads.swap_face_buttons { SDL_GAMEPAD_BUTTON_SOUTH } else { SDL_GAMEPAD_BUTTON_EAST };
            // SAFETY: union members read by event type.
            let back = (kind == SDL_EVENT_KEY_DOWN && unsafe { event.key.scancode } == SDL_SCANCODE_ESCAPE)
                || (kind == SDL_EVENT_GAMEPAD_BUTTON_DOWN && unsafe {
                    event.gbutton.button == back_button.0 as u8 || event.gbutton.button == SDL_GAMEPAD_BUTTON_BACK.0 as u8
                });
            let page = streaming.menu_page != stream_menu::Page::Grid;
            match menu_step(key_action, back, repeat, streaming.shortcut_sheet.is_some(), streaming.session_page.is_some(), page) {
                MenuStep::Pass => {}
                MenuStep::Swallow => return,
                MenuStep::ToggleSheet => return streaming.toggle_shortcut_sheet(self.window, &mut self.pads),
                MenuStep::CloseSessionPage => return streaming.close_session_page(),
                MenuStep::CloseSheet => return streaming.close_shortcut_sheet(self.window, &mut self.pads),
                MenuStep::ClosePage => {
                    streaming.menu_page = stream_menu::Page::Grid;
                    streaming.needs_redraw = true;
                    return;
                }
                MenuStep::CloseMenu => return streaming.toggle_menu(self.window, &mut self.pads),
            }
            // Y, X and the shoulders run their tiles while the grid shows (nightjar_policy::stream_menu).
            if kind == SDL_EVENT_GAMEPAD_BUTTON_DOWN && !page && streaming.session_page.is_none() && streaming.shortcut_sheet.is_none() {
                // SAFETY: union member read by event type.
                let button = SDL_GamepadButton(i32::from(unsafe { event.gbutton.button }));
                if let Some(key) = stream_menu::hotkey(button) {
                    streaming.menu_hotkey = Some(key);
                    streaming.needs_redraw = true;
                    return;
                }
            }
            if matches!(kind, SDL_EVENT_GAMEPAD_BUTTON_DOWN | SDL_EVENT_GAMEPAD_AXIS_MOTION) {
                streaming.needs_redraw = true;
                self.navigate(event);
                return;
            }
            if !matches!(kind, SDL_EVENT_QUIT | SDL_EVENT_WINDOW_CLOSE_REQUESTED
                | SDL_EVENT_WINDOW_FOCUS_LOST | SDL_EVENT_WINDOW_FOCUS_GAINED
                | SDL_EVENT_GAMEPAD_ADDED | SDL_EVENT_GAMEPAD_REMOVED
                | SDL_EVENT_AUDIO_DEVICE_ADDED | SDL_EVENT_AUDIO_DEVICE_REMOVED
                | SDL_EVENT_RENDER_DEVICE_RESET | SDL_EVENT_RENDER_DEVICE_LOST) {
                self.gui.handle(event, Scale::of(self.window));
                streaming.needs_redraw = true;
                return;
            }
        }
        // Microphone only's window has its own buttons; its chords still reach the stream.
        if streaming.mic_only() && !matches!(kind, SDL_EVENT_QUIT | SDL_EVENT_WINDOW_CLOSE_REQUESTED
            | SDL_EVENT_WINDOW_FOCUS_LOST | SDL_EVENT_WINDOW_FOCUS_GAINED
            | SDL_EVENT_AUDIO_DEVICE_ADDED | SDL_EVENT_AUDIO_DEVICE_REMOVED
            | SDL_EVENT_RENDER_DEVICE_RESET | SDL_EVENT_RENDER_DEVICE_LOST) {
            self.gui.handle(event, Scale::of(self.window));
            streaming.needs_redraw = true;
        }
        streaming.handle(event, self.window, self.renderer, &mut self.pads);
    }
}

/// What a key or button does while the stream menu is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuStep {
    /// Not the menu's: egui or the controller navigation gets it.
    Pass,
    /// A held key's repeat of a toggle: holding it doesn't flip the menu open and shut.
    Swallow,
    ToggleSheet,
    CloseSessionPage,
    CloseSheet,
    /// Back from a page a tile opened to the grid.
    ClosePage,
    CloseMenu,
}

/// The step for a stream chord (`action`), Escape or B (`back`), with the menu's pages. Back acts
/// on what is drawn: the Session page first (the sheet never draws over it), then the sheet, then
/// a page a tile opened (`page`).
pub(crate) fn menu_step(action: Option<Action>, back: bool, repeat: bool, sheet: bool, session_page: bool, page: bool) -> MenuStep {
    let controls = action == Some(Action::StreamControls);
    if action != Some(Action::StreamShortcuts) && !back && !controls {
        return MenuStep::Pass;
    }
    if repeat {
        return MenuStep::Swallow;
    }
    if action == Some(Action::StreamShortcuts) {
        MenuStep::ToggleSheet
    } else if session_page {
        MenuStep::CloseSessionPage
    } else if back && sheet {
        MenuStep::CloseSheet
    } else if back && page {
        MenuStep::ClosePage
    } else {
        MenuStep::CloseMenu
    }
}

/// The stream's window mode: gamescope scales the window itself, so no exclusive mode change there.
/// Straight to the display (`kms`) the window keeps the display's mode at any setting: a window
/// size or an exclusive mode would make SDL's KMSDRM set the mode nearest it, and the stream's
/// own mode (its rate at the display's size) is the video planes' (`kms_scanout`).
pub(crate) fn stream_window_mode(mode: WindowMode, gamescope: bool, kms: bool) -> WindowMode {
    match mode {
        _ if kms => WindowMode::Borderless,
        WindowMode::Fullscreen if gamescope => WindowMode::Borderless,
        mode => mode,
    }
}

/// A setting that needs the GPU renderer, off for a stream straight to the display where the
/// renderer could not be made (`kms::without_gpu`).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) const KMS_GPU_OFF: &str = strings::en("{} is off for this stream: the GPU renderer could not be made while drawing straight to the display.");

/// An HDR stream under gamescope without HDR output plays in SDR and says so (handheld.rs).
pub(crate) const HDR_OFF_NOTE: &str = strings::en("HDR is off: gamescope offers no HDR output here, so this stream is in SDR.");

/// The toast a stream starts with: a network warning, the PyroWave fallback and what became of
/// 4:4:4 and 10-bit SDR, else the way to the stream menu (under gamescope, where the
/// controller is the input and Steam keeps the Guide button, Start+Back's).
pub(crate) fn start_toast(warning: Option<&str>, color: Option<&str>, gamescope: bool) -> String {
    let lines: Vec<&str> = warning.into_iter().chain(color).collect();
    if !lines.is_empty() {
        lines.join("\n")
    } else if gamescope {
        tr("Start+Back opens the stream menu").to_owned()
    } else {
        controls_hint()
    }
}

/// The stream hotkeys that open the controls, disconnect and release input, as they are bound.
fn controls_hint() -> String {
    use shortcuts::{chord_for, Chord};
    let keys = |action| chord_for(Scope::Stream, action).map(Chord::text);
    let parts = [
        keys(Action::StreamControls).map(|k| trf("{} opens controls", &[&k])),
        keys(Action::Disconnect).map(|k| trf("{} disconnects", &[&k])),
        keys(Action::Capture).map(|k| trf("{} releases input", &[&k])),
    ];
    parts.into_iter().flatten().collect::<Vec<_>>().join(", ")
}
