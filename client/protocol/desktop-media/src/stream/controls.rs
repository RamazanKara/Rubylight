//! Stream network recovery, microphone routing, input capture, clipboard, menu, and accessors.

use super::*;

impl Streaming {
    /// The route toward the PC went away: a frozen picture is never silent. The notice stays
    /// until a network returns; `away`: the window is minimized or hidden.
    pub(super) fn network_lost(&mut self, away: bool) {
        eprintln!("nightjar: the stream's network was lost");
        self.network_lost = true;
        self.show_toast(tr(lost_notice(&self.settings, self.reconnects_on_its_own(away))));
    }

    /// Notes a network change that moved the stream's route, whether or not the network was lost
    /// first: [`carried_packet_size`] reads it.
    pub(super) fn network_changed(&mut self, change: NetworkChange) {
        if change != NetworkChange::CarriesOn {
            self.network_moved = true;
            #[cfg(windows)]
            if self.facts.wlan_streaming {
                self._wlan = wlan::StreamingMode::begin();
            }
        }
    }

    /// A network is back after the route was gone; `change`: what it means for the session
    /// ([`reconnects_on_return`]).
    pub(super) fn network_returned(&mut self, change: NetworkChange, metered: bool, away: bool) {
        if !std::mem::take(&mut self.network_lost) { return; }
        #[cfg(windows)]
        if self.facts.wlan_streaming {
            self._wlan = wlan::StreamingMode::begin();
        }
        if self.toast.as_ref().is_some_and(|(text, _)| *text == tr(NETWORK_LOST) || *text == tr(NETWORK_LOST_RECONNECTING)) {
            self.toast = None;
            self.needs_redraw = true;
        }
        if change != NetworkChange::CannotContinue || self.exit.is_some() { return; }
        if !reconnects_on_return(change, metered, &self.settings, self.reconnects_on_its_own(away)) {
            eprintln!("nightjar: the network is back on another address the stream did not roam to; \
                its connection times out");
            return;
        }
        eprintln!("nightjar: the network is back on another address the stream did not roam to: reconnecting now");
        self.retry_now = true;
        self.exit = Some(Exit::NetworkReturned);
    }

    /// Whether a dropped stream would reconnect on its own now: its video started ([`Exit::retryable`])
    /// and [`automatic_rejoin`] has a plan with an attempt left in its budget. Spends nothing.
    fn reconnects_on_its_own(&self, away: bool) -> bool {
        self.video_started() && automatic_rejoin(&self.plan, self.facts.role, self.settings.auto_reconnect, away, self.connected_ms(), false, true)
            .and_then(|plan| plan.recovery)
            .is_some_and(|budget| budget.allows_attempt(recovery::now_ms()))
    }

    /// Whether video started: this connection showed a picture, or an earlier one of its automatic
    /// reconnect chain did ([`LaunchPlan::video_shown`]). A reconnect after a real drop that has no
    /// picture yet when it drops again keeps reconnecting until its budget is spent; a stream that
    /// never showed video does not reconnect (Moonlight V+ PC 6792aeed).
    pub fn video_started(&self) -> bool {
        nightjar_policy::recovery::video_started(self.first_frame, self.plan.video_shown)
    }

    fn connected_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// The plan that starts the stream again; `automatic` after a drop, `away` while the window is
    /// minimized or hidden.
    pub fn reconnect_plan(&self, automatic: bool, away: bool) -> Option<LaunchPlan> {
        let mut plan = if automatic {
            automatic_rejoin(&self.plan, self.facts.role, self.settings.auto_reconnect, away, self.connected_ms(), self.retry_now, self.video_started())?
        } else {
            let mut plan = rejoin(self.plan.clone(), self.facts.role)?;
            plan.recovery = None;
            plan.video_shown = false;
            plan
        };
        plan.microphone_muted = self.microphone_muted;
        // The packets the running session negotiated: a resume of it asks for the same size and
        // probes nothing while the old one may still be live.
        plan.negotiated_packet_size = carried_packet_size(self.facts.packet_size, self.network_moved);
        // Automatic bitrate and the client-hint controller never run together; whichever owned the
        // rate leaves its cut. A Foundation PC's own controller cannot be told where to start
        // (`AbrRequest` has no starting rate), so its cut is not carried.
        let owned = self.bitrate_policy.is_some() || self.client_hint_active();
        plan.resume_bitrate_kbps = reconnect_bitrate_start(automatic, owned, self.plan.bitrate_before_manual.is_some(), self.bitrate_kbps);
        plan.resume_quality = if automatic { self.quality_policy.as_ref().and_then(quality::Quality::current) } else { None };
        Some(plan)
    }

    /// A bitrate the player set in the stream menu: every reconnect of this stream starts at it,
    /// with automatic bitrate off as it is now (Android's `GameRecovery.putSessionBitrate`): a
    /// chosen bitrate keeps this device's own lowering off at the start (`own_lowering_at_start`).
    pub(super) fn keep_manual_bitrate(&mut self, kbps: u32) {
        self.plan.bitrate_before_manual.get_or_insert(self.settings.bitrate_kbps);
        self.settings.bitrate_kbps = kbps;
        self.plan.settings = self.settings.clone();
    }

    /// Automatic bitrate is on again: a reconnect starts from the settings' own bitrate with it.
    pub(super) fn forget_manual_bitrate(&mut self) {
        if let Some(kbps) = self.plan.bitrate_before_manual.take() {
            self.settings.bitrate_kbps = kbps;
            if !self.settings.lower_bitrate.keeps() {
                self.settings.lower_bitrate = nightjar_policy::choices::LowerBitrate::WhenFramesLost;
            }
            self.plan.settings = self.settings.clone();
        }
    }

    pub fn microphone_available(&self) -> bool {
        self.settings.microphone && (self.session.microphone_available() || self.controller_audio_available()) && self.facts.role != AppRole::RemoteMonitor
    }

    pub fn video_scaling(&self) -> (config::VideoScaling, bool) { (self.settings.video_scaling, self.settings.align_video_top) }

    pub fn set_video_scaling(&mut self, mode: config::VideoScaling, top: bool) {
        self.settings.video_scaling = mode;
        self.settings.align_video_top = top;
        self.session.input(|q| self.touch.release(q));
        self.needs_redraw = true;
    }

    pub fn controller_audio_available(&self) -> bool {
        self.sends_controllers() && self.settings.controller_audio
    }

    /// Whether this session tells the host about controllers: not with Controllers off, and never
    /// from a view-only remote monitor.
    pub fn sends_controllers(&self) -> bool {
        sends_controllers(&self.settings, self.facts.role)
    }

    /// The notice that Controllers is Off while `connected` controllers are on this device: once a
    /// stream, at its start or for the first controller connected during it.
    pub fn controllers_off_notice(&mut self, connected: usize) -> Option<String> {
        self.controllers_off.take(&self.settings, &self.plan.controllers_from, connected)
    }

    pub fn mute_microphone(&mut self, muted: bool, pads: &mut Gamepads) {
        self.microphone_muted = muted;
        self.microphone = None;
        if !muted {
            for audio in pads.pads.iter_mut().filter_map(|pad| pad.audio.as_mut()) { audio.host_muted = false; }
        }
        self.route_microphones(pads);
    }

    pub(super) fn route_microphones(&mut self, pads: &mut Gamepads) {
        let permitted = self.settings.microphone && self.facts.role != AppRole::RemoteMonitor;
        let mut controller_route = false;
        for pad in &mut pads.pads {
            let Some(audio) = &mut pad.audio else { continue; };
            if audio.host_muted { self.microphone_muted = true; }
            match audio.capture(permitted, self.microphone_muted, &self.shared) {
                Ok(active) => controller_route |= active,
                Err(error) => {
                    self.microphone_muted = true;
                    self.show_toast(error);
                }
            }
        }
        if self.microphone_muted {
            for audio in pads.pads.iter_mut().filter_map(|pad| pad.audio.as_mut()) {
                let _ = audio.capture(permitted, true, &self.shared);
            }
        }
        if !permitted || self.microphone_muted || controller_route {
            self.microphone = None;
        } else if self.microphone.is_none() && self.session.microphone_available() {
            match microphone::Microphone::open(self.shared.clone(), microphone::echo_for(self.plan.mic_only, self.settings.mic_echo_cancel), self.settings.mic_gain, mic_dsp::Choice::of(&self.settings)) {
                Ok(microphone) => {
                    self.microphone = Some(microphone);
                    self.microphone_unavailable = false;
                }
                Err(error) => {
                    self.microphone_muted = true;
                    self.microphone_unavailable = self.plan.mic_only;
                    self.show_toast(error);
                }
            }
        }
        for pad in &mut pads.pads {
            // The gamepad number the PC knows the controller as (player 1 for each with One
            // player), and none for a controller this host has no slot for.
            let Some(sent_as) = pad.slot() else { continue; };
            let sent_as = u16::from(sent_as);
            let Some(audio) = &mut pad.audio else { continue; };
            while let Some(packet) = audio.microphone_packet(sent_as) {
                if let Err(error) = self.session.send(ClientMessage::ControllerMicrophone(packet)) {
                    self.shared.notify(Notice::MicrophoneFailed(format!("Controller microphone forwarding stopped: {error}")));
                    break;
                }
            }
        }
        pads.update_audio();
        if pads.reap_bt_audio() {
            if self.sends_controllers() { self.session.input(|q| pads.announce_new(q)); }
            self.show_toast(trf("Controller audio unavailable: {}", &[&tr("The device stopped answering.")]));
        }
    }

    pub fn save_statistics(&mut self, directory: &std::path::Path) {
        self.export_statistics = false;
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
        let path = directory.join(format!("nightjar-{stamp}.csv"));
        let csv = statistics_csv(&self.statistics);
        match std::fs::create_dir_all(directory).and_then(|_| std::fs::write(&path, csv)) {
            Ok(()) => self.show_toast(trf(MSG_STATS_SAVED, &[&path.display()])),
            Err(error) => self.show_toast(trf(MSG_STATS_SAVE_FAILED, &[&error])),
        }
    }

    /// Keeps this stream's once-a-second measurements at `path` for Settings → Advanced → Export
    /// latency CSV, as Android keeps its last stream's latency CSV. Written once as the stream
    /// ends, from rows the overlay's window collects anyway: no work during the stream. A stream
    /// without a second of measurements leaves the last one's file. False when nothing was written.
    pub fn keep_latency_csv(&self, path: &std::path::Path) -> std::io::Result<bool> {
        if self.statistics.is_empty() {
            return Ok(false);
        }
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory)?;
        }
        std::fs::write(path, statistics_csv(&self.statistics)).map(|()| true)
    }

    /// Settings → Advanced → Show decode time after streaming: Android's post-stream toast, the
    /// average decode time of the stream's frames and its codec, from the counters the overlay
    /// reads. None before a frame was decoded.
    pub fn decode_time_line(&self) -> Option<String> {
        let c = &self.shared.counters;
        decode_time_text(c.decode_us_total.load(Ordering::Relaxed), c.decoded.load(Ordering::Relaxed), negotiate::codec_name(self.facts.format))
    }

    /// A stream that opened behind another window captures input (and with it the keyboard
    /// grab) once it comes to the front, as after a focus loss, not only at the first click.
    pub fn capture_on_focus(&mut self) {
        self.captured = self.takes_input();
    }

    /// Once per pass: the hook keeps the Tab of a stream hotkey Windows would take, while that
    /// hotkey can act and the grab does not have the keyboard.
    #[cfg(windows)]
    pub fn follow_tab_chord(&mut self, window: *mut SDL_Window) {
        // SAFETY: a live window.
        let focused = unsafe { SDL_GetWindowFlags(window) & SDL_WINDOW_INPUT_FOCUS != 0 };
        let grab = system_keys::keyboard_grab(self.captured, self.system_keys());
        let on = system_keys::tab_guard(system_keys::tab_chord_action(), focused, grab, self.menu_visible, self.plan.mic_only);
        self.direct_motion.tab_guard(window, on);
    }

    /// "Send system keys" as it holds now: with "System keys only in full screen", only while
    /// the window is full screen, which the Fullscreen and the Borderless window mode both are.
    pub(super) fn system_keys(&self) -> bool {
        system_keys::sent(self.settings.capture_system_keys, self.settings.system_keys_fullscreen_only, self.fullscreen)
    }

    /// Once per pass: with "System keys only in full screen", the window going full screen or
    /// back to a window takes the system keys with it.
    pub fn follow_fullscreen(&mut self, window: *mut SDL_Window) {
        // SAFETY: a live window.
        let fullscreen = unsafe { SDL_GetWindowFlags(window) & SDL_WINDOW_FULLSCREEN != 0 };
        let before = self.system_keys();
        self.fullscreen = fullscreen;
        if self.system_keys() != before {
            // A key held across the change would stay down on the PC.
            self.raise_all_keys();
            self.set_capture(window, self.captured);
        }
    }

    /// Whether this stream captures the mouse and keyboard: not a view-only remote monitor, and
    /// not Microphone only, which sends nothing but the microphone.
    fn takes_input(&self) -> bool {
        self.facts.role != AppRole::RemoteMonitor && !self.plan.mic_only
    }

    /// Mouse capture: relative motion with a hidden, confined pointer.
    pub fn set_capture(&mut self, window: *mut SDL_Window, on: bool) {
        self.focused = unsafe { SDL_GetWindowFlags(window) & SDL_WINDOW_INPUT_FOCUS != 0 };
        let on = on && self.takes_input();
        self.captured = on;
        let cursor_shapes = on && self.absolute && self.settings.cursor_shapes && self.facts.cursor_shapes;
        if cursor_shapes != self.cursor.enabled
            && self.session.send(ClientMessage::CursorShapeRequest { enabled: cursor_shapes }).is_ok() {
            self.cursor.clear();
            self.cursor.enabled = cursor_shapes;
        }
        // SAFETY: a live window.
        unsafe {
            if self.absolute {
                SDL_SetWindowRelativeMouseMode(window, false);
                if on && !self.cursor.enabled {
                    SDL_HideCursor();
                } else {
                    SDL_ShowCursor();
                }
            } else {
                SDL_SetWindowRelativeMouseMode(window, on);
            }
            let grab = system_keys::keyboard_grab(on, self.system_keys());
            let sdl_grab = self.direct_motion.keyboard_grab(window, grab);
            SDL_SetWindowKeyboardGrab(window, sdl_grab);
        }
        self.direct_motion.set_active(on && !self.absolute && self.focused, self.settings.mouse_speed);
    }

    pub fn raise_all_keys(&mut self) {
        self.release_shortcut();
        let (direct_keys, direct_buttons) = self.direct_motion.take_sent_down();
        self.session.input(|q| {
            release_held_inputs(&mut self.keys_down, &mut self.mouse_down, direct_keys, direct_buttons, &mut self.touch, q);
        });
    }

    pub fn clipboard(&mut self, send: bool) {
        if !self.facts.clipboard || self.clipboard_busy { return; }
        let client = lock(&self.shared.host_client).clone().expect("connected HTTP client");
        let text = if send {
            // SAFETY: SDL clipboard queries run on the main thread; SDL_GetClipboardText returns
            // either null or a newly allocated NUL-terminated buffer freed with SDL_free.
            let read = unsafe {
                if !SDL_HasClipboardText() {
                    Ok(String::new())
                } else {
                    let text = SDL_GetClipboardText();
                    if text.is_null() {
                        Err(())
                    } else {
                        let value = std::ffi::CStr::from_ptr(text).to_string_lossy().into_owned();
                        SDL_free(text.cast());
                        Ok(value)
                    }
                }
            };
            match non_empty_clipboard_text(read) {
                Ok(value) => Some(value),
                Err(message) => {
                    self.show_toast(tr(message));
                    return;
                }
            }
        } else { None };
        let shared = self.shared.clone();
        self.clipboard_busy = true;
        std::thread::spawn(move || {
            let result = client.server_info_https().map_err(|e| e.to_string()).and_then(|info| {
                let permission = if send { nightjar_core::http::PERMISSION_CLIPBOARD_SET } else { nightjar_core::http::PERMISSION_CLIPBOARD_READ };
                if !info.capabilities.has(Feature::Clipboard) || info.permission.is_some_and(|p| p & permission == 0) {
                    return Err("The PC denied clipboard access.".into());
                }
                match text {
                    Some(text) => client.send_clipboard(&text).map(|_| None),
                    None => client.clipboard().map(Some),
                }.map_err(|e| e.to_string())
            });
            shared.notify(Notice::Clipboard(result));
        });
    }

    /// Sends the image on this device's clipboard to the PC, or (`send` false) puts the PC's
    /// image on this device's clipboard. Every failure says what failed.
    pub fn clipboard_image(&mut self, send: bool) {
        if !self.facts.image_clipboard || self.clipboard_busy { return; }
        let client = lock(&self.shared.host_client).clone().expect("connected HTTP client");
        let local = if send {
            match crate::clipboard::read() {
                Ok(Some(local)) => Some(local),
                Ok(None) => {
                    self.show_toast(tr(MSG_LOCAL_CLIPBOARD_NO_IMAGE));
                    return;
                }
                Err(error) => {
                    self.show_toast(trf(MSG_LOCAL_CLIPBOARD_IMAGE_READ_FAILED, &[&error]));
                    return;
                }
            }
        } else { None };
        let shared = self.shared.clone();
        self.clipboard_busy = true;
        std::thread::spawn(move || {
            let result = match local {
                Some(local) => crate::clipboard::to_png(local)
                    .map_err(|e| trf(MSG_LOCAL_CLIPBOARD_PNG_FAILED, &[&e]))
                    .and_then(|png| client.send_clipboard_image(&png).map_err(|e| image_error(e, true)))
                    .map(|()| None),
                None => match client.clipboard_image() {
                    Ok(Some(png)) => crate::clipboard::from_png(&png)
                        .map(Some)
                        .map_err(|e| trf(MSG_HOST_IMAGE_READ_FAILED, &[&e])),
                    Ok(None) => Err(tr(MSG_HOST_CLIPBOARD_NO_IMAGE).to_owned()),
                    Err(e) => Err(image_error(e, false)),
                },
            };
            shared.notify(Notice::ClipboardImage(result));
        });
    }

    /// Opens the stream menu when a held Guide asked for it on the last tick, and returns a new
    /// join-request guest name if one arrived (`None` otherwise) so the UI shell can switch to the
    /// Guests page or show a toast.
    pub fn open_guide_menu(&mut self, window: *mut SDL_Window, pads: &mut Gamepads) -> Option<String> {
        if std::mem::take(&mut self.guide_menu) && !self.menu_visible {
            self.toggle_menu(window, pads);
        }
        self.guests.take_prompt()
    }

    pub fn toggle_menu(&mut self, window: *mut SDL_Window, pads: &mut Gamepads) {
        // While End or Close game runs, the Session page stays so its answer (or failure) shows.
        if self.menu_visible && self.session_page_busy {
            return;
        }
        self.menu_visible = !self.menu_visible;
        if self.menu_visible {
            self.refresh_host_audio();
        } else {
            self.session_page_open = false;
            self.session_page_busy = false;
            self.close_session_page_requested = false;
            self.shortcut_sheet = None;
            // The menu's own text field takes no more text once it closes.
            // SAFETY: a live window on the main thread.
            unsafe {
                if SDL_TextInputActive(window) && !SDL_StopTextInput(window) {
                    eprintln!("nightjar: could not stop the menu's text input");
                }
            }
        }
        self.text_field.apply(window, self.menu_visible);
        self.raise_all_keys();
        self.session.input(|q| pads.release(q));
        // The launch curtain's buttons keep the pointer until it lifts (`follow_curtain`).
        self.set_capture(window, !self.menu_visible && !self.curtain_down());
        self.needs_redraw = true;
    }

    /// Whether the PC speaks the Rubylight native API, so the Session page has something to show.
    pub fn session_page_available(&self) -> bool {
        self.facts.role == AppRole::Stream && self.plan.info.capabilities.has(Feature::NativeApi)
    }

    /// Returns a TLS-pinned host HTTP client from the running stream's plan, if paired.
    pub fn plan_host_client(&self) -> Result<HostClient, String> {
        host_client_for(&self.plan.identity, &self.plan.record, &self.plan.record.address)
    }

    /// Returns the active shared host HTTP client, if connected.
    pub fn host_client(&self) -> Option<Arc<HostClient>> {
        lock(&self.shared.host_client).clone()
    }

    /// Marks the stream to quit the remote app on leave and exit cleanly.
    pub fn quit_app_and_leave(&mut self) {
        self.quit_on_leave = true;
        self.exit = Some(Exit::User);
    }

    /// The stream menu's header on its first page: the game's name over the stream's mode
    /// (`1440p · 144 FPS · HEVC HDR`, nightjar_policy::stream_menu::mode_line), the PC's name while
    /// the mode is not known. A remote monitor or input session keeps the menu's title.
    pub fn menu_header(&self) -> (String, String) {
        let place = format!("{} · {}", self.facts.app_name, self.facts.host_name);
        if self.facts.role != AppRole::Stream || self.facts.app_name.trim().is_empty() {
            return (tr(MSG_STREAM_MENU).to_owned(), place);
        }
        let size = |v: i32| u32::try_from(v).unwrap_or(0);
        let hdr = present::hdr_negotiated(self.settings.hdr, self.facts.format);
        let mode = nightjar_policy::stream_menu::mode_line(size(self.facts.width), size(self.facts.height), size(self.facts.fps), self.facts.format, hdr);
        (self.facts.app_name.clone(), mode.unwrap_or_else(|| self.facts.host_name.clone()))
    }

    /// Ctrl+Alt+Shift+H, or Shortcuts & help in the stream controls: the shortcut sheet opens over
    /// the stream (from the controls when they or the Session page are open), and the chord again
    /// closes it and the controls. It draws only while shown, like the controls.
    pub fn toggle_shortcut_sheet(&mut self, window: *mut SDL_Window, pads: &mut Gamepads) {
        let session_page = self.session_page_open.then_some(self.session_page_busy);
        match sheet_chord(self.menu_visible, self.shortcut_sheet, session_page) {
            SheetChord::Stay => {}
            SheetChord::Close => self.toggle_menu(window, pads),
            SheetChord::Open(from) => {
                if self.menu_visible {
                    if self.session_page_open && !self.session_page_busy {
                        self.session_page_open = false;
                        self.close_session_page_requested = true;
                    }
                } else {
                    self.toggle_menu(window, pads);
                }
                if self.menu_visible && !self.session_page_open {
                    self.shortcut_sheet = Some(from);
                    self.needs_redraw = true;
                }
            }
        }
    }

    /// Escape or B on the shortcut sheet: back to the controls it was opened from, or to the game
    /// when its chord opened it.
    pub fn close_shortcut_sheet(&mut self, window: *mut SDL_Window, pads: &mut Gamepads) {
        match self.shortcut_sheet.take() {
            Some(Sheet::FromChord) => self.toggle_menu(window, pads),
            Some(Sheet::FromMenu) | None => self.needs_redraw = true,
        }
    }

    pub fn send_shortcut(&mut self, shortcut: &crate::shortcuts::Shortcut) {
        let ready = self.focused && self.captured && self.facts.role != AppRole::RemoteMonitor;
        if let Some(why) = shortcut_refusal(ready, self.settings.capture_system_keys, &shortcut.keys) {
            self.show_toast(tr(why));
            return;
        }
        // A shortcut still held goes up first, so this one is pressed in full.
        self.release_shortcut();
        if self.shortcut_held.is_some() {
            self.show_toast(trf(MSG_SHORTCUT_NOT_SENT, &[&nightjar_core::input::InputError::QueueFull]));
            return;
        }
        match self.session.input(|q| crate::shortcuts_runtime::press(&shortcut.keys, &self.keys_down, q)) {
            Ok(chord) => {
                self.shortcut_held = Some((chord, Instant::now() + Duration::from_millis(crate::shortcuts::HOLD_MS)));
            }
            Err(error) => self.show_toast(trf(MSG_SHORTCUT_NOT_SENT, &[&error])),
        }
    }

    /// Lets go of the held shortcut's keys once its hold is over; `pump` calls this.
    pub(super) fn release_shortcut_when_due(&mut self, now: Instant) {
        if self.shortcut_held.as_ref().is_some_and(|(_, due)| now >= *due) { self.release_shortcut(); }
    }

    /// When the held shortcut's keys go up, for the loop's wait.
    pub(super) fn shortcut_release_in(&self, now: Instant) -> Option<Duration> {
        self.shortcut_held.as_ref().map(|(_, due)| due.saturating_duration_since(now))
    }

    fn release_shortcut(&mut self) {
        let Some((mut chord, due)) = self.shortcut_held.take() else { return };
        match self.session.input(|q| crate::shortcuts_runtime::release(&mut chord, q)) {
            Ok(()) => {}
            // Keys left down would stay down on the PC: the queue drains, so the next pump tries again.
            Err(nightjar_core::input::InputError::QueueFull) => self.shortcut_held = Some((chord, due)),
            Err(error) => eprintln!("nightjar: shortcut keys not released: {error}"),
        }
    }

    /// What the user changed of the overlay since the settings last kept it: the stream menu and
    /// the shortcut change the global setting too, as on Android. Only the changed values, since
    /// the others may be this PC's, game's or profile's own and are not the global ones.
    pub fn take_overlay_change(&mut self) -> Option<OverlayChange> {
        let now = (self.overlay_visible, self.overlay_mode, self.overlay_corner);
        let change = overlay_change(self.overlay_kept, now)?;
        self.overlay_kept = now;
        Some(change)
    }

    /// Whether the stream has shown its first picture.
    pub fn has_picture(&self) -> bool {
        self.first_frame
    }

    /// The app this stream started or joined.
    pub fn app(&self) -> &App {
        &self.plan.app
    }

    /// Whether this stream joined the game the PC runs, another device's too (`LaunchPlan::join`).
    pub fn joined(&self) -> bool {
        self.plan.join
    }

    /// Input-only through a small ordinary stream: video and audio arrive and are discarded.
    pub fn small_stream_input_only(&self) -> bool {
        self.plan.small_stream_input_only
    }

    /// Only this device's microphone goes to the PC.
    pub fn mic_only(&self) -> bool {
        self.plan.mic_only
    }

    /// Microphone only: starts asking the PC every few seconds whether another device's
    /// microphone plays (`GET /session`); `wake` redraws when that changes.
    pub fn watch_microphone_busy(&mut self, wake: Waker) {
        if !self.plan.mic_only || !self.plan.info.capabilities.has(Feature::MicOnly) {
            return;
        }
        let identity = self.plan.identity.clone();
        let record = self.plan.record.clone();
        self.microphone_busy = MicrophoneBusyWatch::start(
            move || host_client_for(&identity, &record, &record.address).map(|c| move || c.microphone_busy().map_err(|e| e.to_string())),
            wake,
            MICROPHONE_BUSY_EVERY,
        );
    }

    /// Whether the PC said, last time it was asked, that another device's microphone plays.
    pub fn microphone_busy(&self) -> bool {
        self.microphone_busy.as_ref().is_some_and(MicrophoneBusyWatch::busy)
    }

    pub fn since_start(&self) -> Duration {
        self.started.elapsed()
    }

    /// The stream ends into a reconnect, on its own or by the player's choice: the decoder's
    /// device and codec context are kept for the session that follows it when it is the same
    /// stream (`stream::pipeline::Decoder::cleanup`).
    pub fn keep_decoder_for_reconnect(&self, keep: bool) {
        self.shared.keep_decoder.store(keep, Ordering::Relaxed);
    }

    #[cfg(windows)] pub fn take_device(&mut self) -> crate::media::DeviceRef { std::mem::take(&mut self.plan.device) }
    pub fn device_removed(&self) -> bool { self.plan.device.is_removed() }
    pub fn note_render_device_lost(&mut self, reason: String) { self.retry_now = true; self.exit.get_or_insert(Exit::RenderDeviceLost(reason)); }
    pub fn check_render_device_lost(&mut self) -> bool { let lost = crate::render_device::event_pending() || self.device_removed(); if lost { self.note_render_device_lost(crate::render_device::lost_reason().into()); } lost }
}

/// Validates a local clipboard read before sending it to the PC: empty text refuses the send so
/// an empty clipboard never wipes the PC's clipboard.
fn non_empty_clipboard_text(read: Result<String, ()>) -> Result<String, &'static str> {
    match read {
        Err(()) => Err(MSG_LOCAL_CLIPBOARD_READ_FAILED),
        Ok(value) if value.is_empty() => Err(MSG_LOCAL_CLIPBOARD_NO_TEXT),
        Ok(value) => Ok(value),
    }
}

/// Releases every key, mouse button and touch point currently held by either the main loop or
/// `DirectMotion` (before SDL's queue has been drained).
fn release_held_inputs(
    keys_down: &mut HashSet<u16>,
    mouse_down: &mut HashSet<u8>,
    direct_keys: HashSet<u16>,
    direct_buttons: HashSet<u8>,
    touch: &mut touch::Touch,
    q: &mut nightjar_core::input::InputQueue,
) {
    keys_down.extend(direct_keys);
    mouse_down.extend(direct_buttons);
    touch.release(q);
    for key in keys_down.drain() {
        let _ = q.send_keyboard(0x8000 | key, false, 0, 0);
    }
    for button in mouse_down.drain() {
        let _ = q.send_mouse_button(button, false);
    }
}

#[cfg(test)]
mod direct_motion_raise_keys_tests {
    use super::*;
    use nightjar_core::control::HostCapabilities;
    use nightjar_core::input::packets::magic;
    use nightjar_core::input::InputQueue;

    #[test]
    fn raise_all_keys_releases_direct_motion_sent_key_and_mouse_before_sdl_poll() {
        let mut queue = InputQueue::new(&HostCapabilities::default());
        // Simulate DirectMotion sending W (0x57) and Left Mouse Button (1) DOWN during present,
        // followed immediately by open_guide_menu()/follow_fullscreen() -> raise_all_keys()
        // before wait_stream_events() drains the SDL queue (`keys_down` and `mouse_down` are empty).
        let mut keys_down = HashSet::new();
        let mut mouse_down = HashSet::new();
        let direct_keys = HashSet::from([0x57u16]);
        let direct_buttons = HashSet::from([mouse_button::LEFT]);
        let mut touch = touch::Touch::default();

        release_held_inputs(&mut keys_down, &mut mouse_down, direct_keys, direct_buttons, &mut touch, &mut queue);

        assert!(keys_down.is_empty());
        assert!(mouse_down.is_empty());
        let mut packets = Vec::new();
        queue.drain(Instant::now(), false, &mut packets);
        let magics: Vec<u32> = packets
            .iter()
            .filter(|p| p.data.len() >= 8)
            .map(|p| u32::from_le_bytes(p.data[4..8].try_into().unwrap()))
            .collect();
        assert!(magics.contains(&magic::KEY_UP), "expected KEY_UP in {magics:#x?}");
        assert!(magics.contains(&magic::MOUSE_BUTTON_UP), "expected MOUSE_BUTTON_UP in {magics:#x?}");

        // Also verify deduplication when a key/button is in both sets.
        let mut keys_down = HashSet::from([0x57u16]);
        let mut mouse_down = HashSet::from([mouse_button::LEFT]);
        release_held_inputs(
            &mut keys_down,
            &mut mouse_down,
            HashSet::from([0x57u16]),
            HashSet::from([mouse_button::LEFT]),
            &mut touch,
            &mut queue,
        );
        let mut dedup_packets = Vec::new();
        queue.drain(Instant::now(), false, &mut dedup_packets);
        assert_eq!(dedup_packets.len(), 2, "each held key/button is released once");
    }

    #[test]
    fn empty_local_clipboard_is_refused_before_sending() {
        assert_eq!(non_empty_clipboard_text(Ok(String::new())), Err(MSG_LOCAL_CLIPBOARD_NO_TEXT));
        assert_eq!(non_empty_clipboard_text(Err(())), Err(MSG_LOCAL_CLIPBOARD_READ_FAILED));
        assert_eq!(non_empty_clipboard_text(Ok("   ".into())), Ok("   ".into()));
        assert_eq!(non_empty_clipboard_text(Ok("copied".into())), Ok("copied".into()));
    }
}
