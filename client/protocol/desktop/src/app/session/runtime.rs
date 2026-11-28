//! Stream runtime helpers: Windows timer resolution, pending failure dialogs, Linux start
//! diagnostics, in-stream settings persistence and stream-menu input routing.

use super::super::*;

#[cfg(windows)]
#[link(name = "winmm")]
unsafe extern "system" {
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
pub(super) const REASON_WAIT: Duration = Duration::from_secs(5);

/// The end of a stream the PC failed, shown once the poll `seq` of PC `host` answered.
pub(crate) struct PendingEnd {
    pub(crate) host: u64,
    pub(crate) seq: u64,
    pub(super) ended: Option<String>,
    pub(crate) health: Option<String>,
    pub(crate) until: Instant,
}

impl App {
    /// The `linux:` line, the present mode for the `video path:` line, and the toasts naming what
    /// hardware decoding is missing (`linux_diagnosis`).
    #[cfg(target_os = "linux")]
    pub(super) fn linux_stream_start(&self, settings: &config::Settings, vsync: bool) -> Vec<String> {
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

    /// Notes the game of a stream that just started, for Home's "Recently played".
    pub(super) fn remember_recent(&mut self, connecting: &Connecting) {
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

    /// Routes an event to the stream menu while it is open; returns whether the menu consumed it.
    pub(super) fn dispatch_stream_menu(
        &mut self,
        event: &SDL_Event,
        kind: SDL_EventType,
        key_action: Option<Action>,
        repeat: bool,
    ) -> bool {
        let pad_allowed = self.stream_controllers_allowed();
        if !pad_allowed
            && matches!(
                kind,
                SDL_EVENT_GAMEPAD_BUTTON_DOWN
                    | SDL_EVENT_GAMEPAD_BUTTON_UP
                    | SDL_EVENT_GAMEPAD_AXIS_MOTION
                    | SDL_EVENT_GAMEPAD_TOUCHPAD_DOWN
                    | SDL_EVENT_GAMEPAD_TOUCHPAD_MOTION
                    | SDL_EVENT_GAMEPAD_TOUCHPAD_UP
            )
        {
            return true;
        }
        let Some((streaming, _)) = self.streaming.as_mut() else { return false };
            // What is in the hand names the Control Center's hints.
            if let Some(source) = input_source::of(event, |id| self.pads.real_pad_type(id))
                && source != self.overlay.input
            {
                self.overlay.input = source;
                streaming.needs_redraw = true;
            }
            let back_button = if self.pads.swap_face_buttons { SDL_GAMEPAD_BUTTON_SOUTH } else { SDL_GAMEPAD_BUTTON_EAST };
            // SAFETY: union members read by event type.
            let back = (kind == SDL_EVENT_KEY_DOWN && unsafe { event.key.scancode } == SDL_SCANCODE_ESCAPE)
                || (kind == SDL_EVENT_GAMEPAD_BUTTON_DOWN && unsafe {
                    event.gbutton.button == back_button.0 as u8 || event.gbutton.button == SDL_GAMEPAD_BUTTON_BACK.0 as u8
                });
            let page = self.overlay.menu_page != stream_menu::Page::Grid;
            match menu_step(key_action, back, repeat, streaming.shortcut_sheet.is_some(), self.overlay.session_page.is_some(), page) {
                MenuStep::Pass => {}
                MenuStep::Swallow => return true,
                MenuStep::ToggleSheet => {
                    streaming.toggle_shortcut_sheet(self.window, &mut self.pads);
                    if std::mem::take(&mut streaming.close_session_page_requested) {
                        self.overlay.close_session_page(streaming);
                    }
                    self.overlay.sync_to_stream(streaming);
                    return true;
                }
                MenuStep::CloseSessionPage => { self.overlay.close_session_page(streaming); return true; }
                MenuStep::CloseSheet => { streaming.close_shortcut_sheet(self.window, &mut self.pads); return true; }
                MenuStep::ClosePage => {
                    self.overlay.menu_page = stream_menu::Page::Grid;
                    // Back on the module's page, the focus returns to the card that opened the page left.
                    self.overlay.center.restore_focus(&self.gui.ctx);
                    streaming.needs_redraw = true;
                    return true;
                }
                MenuStep::CloseMenu => {
                    streaming.toggle_menu(self.window, &mut self.pads);
                    self.overlay.sync_to_stream(streaming);
                    return true;
                }
            }
            // Y, X and the shoulders run their tiles while the grid shows (nightjar_policy::stream_menu);
            // one whose tile is not offered goes on to the menu's navigation as any other button.
            if kind == SDL_EVENT_GAMEPAD_BUTTON_DOWN && !page && self.overlay.session_page.is_none() && streaming.shortcut_sheet.is_none() {
                // SAFETY: union member read by event type.
                let button = SDL_GamepadButton(i32::from(unsafe { event.gbutton.button }));
                if let Some(key) = stream_menu::offered_hotkey(hotkey_sdl::menu_nav_button(button), &self.overlay.menu_hotkeys) {
                    self.overlay.menu_hotkey = Some(key);
                    streaming.needs_redraw = true;
                    return true;
                }
            }
            // LT and RT step along the Control Center's rail, one module for each pull; on a page a
            // card opened, on the Session page and on the sheet they do nothing.
            if kind == SDL_EVENT_GAMEPAD_AXIS_MOTION {
                // SAFETY: union member read by event type.
                let axis = unsafe { event.gaxis };
                let right = match SDL_GamepadAxis(i32::from(axis.axis)) {
                    SDL_GAMEPAD_AXIS_LEFT_TRIGGER => Some(false),
                    SDL_GAMEPAD_AXIS_RIGHT_TRIGGER => Some(true),
                    _ => None,
                };
                if let Some(right) = right {
                    if let Some(forward) = self.overlay.triggers.pull(right, axis.value)
                        && !page
                        && self.overlay.session_page.is_none()
                        && streaming.shortcut_sheet.is_none()
                    {
                        self.overlay.center.request_step(forward);
                        streaming.needs_redraw = true;
                    }
                    return true;
                }
            }
            if matches!(
                kind,
                SDL_EVENT_GAMEPAD_BUTTON_DOWN
                    | SDL_EVENT_GAMEPAD_BUTTON_UP
                    | SDL_EVENT_GAMEPAD_AXIS_MOTION
            ) {
                let changed = self.navigate(event);
                if (kind != SDL_EVENT_GAMEPAD_AXIS_MOTION || changed)
                    && let Some((s, _)) = self.streaming.as_mut()
                {
                    s.needs_redraw = true;
                }
                return true;
            }
            if matches!(
                kind,
                SDL_EVENT_GAMEPAD_SENSOR_UPDATE
                    | SDL_EVENT_SENSOR_UPDATE
                    | SDL_EVENT_GAMEPAD_UPDATE_COMPLETE
                    | SDL_EVENT_GAMEPAD_REMAPPED
                    | SDL_EVENT_GAMEPAD_STEAM_HANDLE_UPDATED
                    | SDL_EVENT_JOYSTICK_AXIS_MOTION
                    | SDL_EVENT_JOYSTICK_BALL_MOTION
                    | SDL_EVENT_JOYSTICK_HAT_MOTION
                    | SDL_EVENT_JOYSTICK_BUTTON_DOWN
                    | SDL_EVENT_JOYSTICK_BUTTON_UP
                    | SDL_EVENT_JOYSTICK_ADDED
                    | SDL_EVENT_JOYSTICK_REMOVED
                    | SDL_EVENT_JOYSTICK_BATTERY_UPDATED
                    | SDL_EVENT_JOYSTICK_UPDATE_COMPLETE
            ) {
                return true;
            }
            if !matches!(kind, SDL_EVENT_QUIT | SDL_EVENT_WINDOW_CLOSE_REQUESTED
                | SDL_EVENT_WINDOW_FOCUS_LOST | SDL_EVENT_WINDOW_FOCUS_GAINED
                | SDL_EVENT_GAMEPAD_ADDED | SDL_EVENT_GAMEPAD_REMOVED
                | SDL_EVENT_AUDIO_DEVICE_ADDED | SDL_EVENT_AUDIO_DEVICE_REMOVED
                | SDL_EVENT_RENDER_DEVICE_RESET | SDL_EVENT_RENDER_DEVICE_LOST) {
                self.gui.handle(event, Scale::of(self.window));
                streaming.needs_redraw = true;
                return true;
            }
        false
    }
}

/// Persists overlay, bitrate-keep and automatic-USB toggles changed inside the stream menu.
pub(super) fn save_stream_settings(
    settings: &mut config::Settings,
    store: &Store,
    save_failed_told: &mut bool,
    streaming: &mut Streaming,
) {
        if let Some((shown, mode, corner)) = streaming.take_overlay_change() {
            let s = &mut *settings;
            s.overlay = shown.unwrap_or(s.overlay);
            s.overlay_mode = mode.unwrap_or(s.overlay_mode);
            s.overlay_position = corner.unwrap_or(s.overlay_position);
            if let Err(e) = store.save_settings(s) {
                eprintln!("nightjar: saving settings failed: {e}");
                if first_save_failure(save_failed_told) {
                    streaming.show_toast(save_failed_text(store.dir()));
                }
            }
        }
        // Keep my bitrate turned on or off in the stream menu: Lower bitrate's choice in the main
        // settings, stored as Settings stores Never or The PC decides.
        if let Some(keep) = streaming.take_keep_change() {
            let s = &mut *settings;
            stream::store_keep(s, keep);
            if let Err(e) = store.save_settings(s) {
                eprintln!("nightjar: saving settings failed: {e}");
                if first_save_failure(save_failed_told) {
                    streaming.show_toast(save_failed_text(store.dir()));
                }
            }
        }
        // A device ticked or unticked for automatic USB forwarding in the stream controls.
        if let Some(auto) = streaming.usb.as_mut().and_then(crate::usb::Panel::take_auto_change) {
            settings.usb_auto = auto;
            if let Err(e) = store.save_settings(settings) {
                eprintln!("nightjar: saving settings failed: {e}");
                if first_save_failure(save_failed_told) {
                    streaming.show_toast(save_failed_text(store.dir()));
                }
            }
        }
}
