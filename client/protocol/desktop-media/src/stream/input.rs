//! Keyboard, mouse, touch and controller events while streaming.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct KeyInput {
    pub(super) code: u16,
    pub(super) modifiers: u8,
    pub(super) flags: u8,
}

/// What a key sends to the PC, or None for a key without a Windows code.
pub(super) fn key_input(scancode: SDL_Scancode, held: SDL_Keymod, system_keys: bool, right_alt_as_meta: bool) -> Option<KeyInput> {
    let has = |flag: SDL_Keymod| held.0 & flag.0 != 0;
    let mapped = if right_alt_as_meta && scancode == SDL_SCANCODE_RALT { SDL_SCANCODE_RGUI } else { scancode };
    let (code, flags) = keymap::virtual_key(mapped, system_keys || (right_alt_as_meta && scancode == SDL_SCANCODE_RALT))?;
    let mut modifiers = 0u8;
    if has(SDL_KMOD_CTRL) {
        modifiers |= modifier::CTRL;
    }
    if has(SDL_KMOD_LALT) || (!right_alt_as_meta && has(SDL_KMOD_RALT)) {
        modifiers |= modifier::ALT;
    }
    if has(SDL_KMOD_SHIFT) {
        modifiers |= modifier::SHIFT;
    }
    if (has(SDL_KMOD_GUI) && system_keys) || (right_alt_as_meta && has(SDL_KMOD_RALT)) {
        modifiers |= modifier::META;
    }
    Some(KeyInput { code, modifiers, flags })
}

/// The PC's button for an SDL mouse button, or None for one it doesn't get.
pub(super) fn mouse_button_code(button: u8, nav_buttons: bool) -> Option<u8> {
    match i32::from(button) {
        SDL_BUTTON_LEFT => Some(mouse_button::LEFT),
        SDL_BUTTON_MIDDLE => Some(mouse_button::MIDDLE),
        SDL_BUTTON_RIGHT => Some(mouse_button::RIGHT),
        SDL_BUTTON_X1 if nav_buttons => Some(mouse_button::X1),
        SDL_BUTTON_X2 if nav_buttons => Some(mouse_button::X2),
        _ => None,
    }
}

/// Whether pointer motion and the wheel go to the PC: captured and in front. A stream that opened
/// behind another window is captured before it is focused (`capture_on_focus`), and a pointer
/// passing over it then must not move or scroll the PC.
/// The events a microphone-only session acts on: closing the window ends it, and the window's
/// focus and the audio devices (a headset's microphone) still count. Keys, pointers and
/// controllers stay on this device.
pub(super) fn mic_only_handles(kind: SDL_EventType) -> bool {
    matches!(
        kind,
        SDL_EVENT_QUIT
            | SDL_EVENT_WINDOW_CLOSE_REQUESTED
            | SDL_EVENT_RENDER_DEVICE_RESET
            | SDL_EVENT_RENDER_DEVICE_LOST
            | SDL_EVENT_WINDOW_FOCUS_LOST
            | SDL_EVENT_WINDOW_FOCUS_GAINED
            | SDL_EVENT_AUDIO_DEVICE_ADDED
            | SDL_EVENT_AUDIO_DEVICE_REMOVED
            // For Nightjar's own chords only (mic_only_chord); no key goes to the PC.
            | SDL_EVENT_KEY_DOWN
            | SDL_EVENT_KEY_UP
    )
}

/// The chords Microphone only keeps: disconnect, full screen and minimize. The rest are about
/// input, the picture or the clipboard, which it does not have.
pub(super) fn mic_only_chord(action: Option<crate::shortcuts::Action>) -> bool {
    use crate::shortcuts::Action;
    matches!(action, Some(Action::Disconnect | Action::Fullscreen | Action::Minimize))
}

pub(super) fn pointer_goes_out(captured: bool, focused: bool) -> bool {
    captured && focused
}

/// Whether a mouse button event goes to the PC: presses require capture and focus, while
/// releases also pass if the button is currently held in `mouse_down` so none stays stuck.
pub(super) fn mouse_button_goes_out(captured: bool, focused: bool, down: bool, was_down: bool) -> bool {
    if down { pointer_goes_out(captured, focused) } else { pointer_goes_out(captured, focused) || was_down }
}

/// Reconciles a key or mouse button event that `DirectMotion` already sent to the host.
/// Returns `true` when the press was recorded as currently held (i.e. not already released
/// by `raise_all_keys()` or a subsequent `SENT` release).
fn reconcile_sent_press<T: Eq + std::hash::Hash>(
    held: &mut HashSet<T>,
    item: T,
    down: bool,
    still_down_in_filter: bool,
) -> bool {
    if down {
        if still_down_in_filter {
            held.insert(item);
            return true;
        }
    } else {
        held.remove(&item);
    }
    false
}

/// Whether a key event goes to the PC: presses require capture and focus, while releases also
/// pass if the key is currently held in `keys_down` so none stays stuck.
pub(super) fn key_goes_out(captured: bool, focused: bool, down: bool, was_down: bool) -> bool {
    if down { captured && focused } else { (captured && focused) || was_down }
}

/// A wheel turn as the PC's high-resolution scroll. SDL gives a trackpad's turn in fractions of a
/// notch; the queue carries the fraction to the next turn.
pub(super) fn send_wheel(w: &SDL_MouseWheelEvent, q: &mut nightjar_core::input::InputQueue) {
    let (x, y) = crate::sdl::wheel_turn(w);
    let _ = q.send_wheel_turn(f64::from(x), f64::from(y));
}

impl Streaming {
    /// What the input filter may send while the loop waits in the present: what this loop would
    /// send itself, with nothing changing it meanwhile (`direct_motion::Rules`).
    pub(super) fn direct_rules(&self, window: *mut SDL_Window) -> direct_motion::Rules {
        // SAFETY: a live window.
        let focused = unsafe { SDL_GetWindowFlags(window) & SDL_WINDOW_INPUT_FOCUS != 0 };
        // A microphone-only session sends the PC no input at all.
        let ours = self.captured && self.focused && !self.menu_visible && !self.plan.mic_only;
        direct_motion::Rules {
            keys: ours.then_some(direct_motion::KeyRules {
                system_keys: self.system_keys(),
                right_alt_as_meta: self.settings.right_alt_as_meta,
            }),
            clicks: (ours && !self.absolute).then_some(direct_motion::ClickRules { nav_buttons: self.settings.mouse_nav_buttons }),
            // The launch curtain's A and B are the main loop's (`curtain_press`).
            pads: !self.menu_visible && !self.curtain_down() && self.facts.role != AppRole::RemoteMonitor && self.settings.controllers && !self.plan.mic_only
                && (focused || self.settings.background_controllers),
        }
    }

    /// Syncs direct Steam Deck IMU emission with stream input rules and Quick Suspend state.
    pub fn sync_deck_imu(&self, window: *mut SDL_Window, pads: &Gamepads) {
        pads.set_deck_imu_sending(self.direct_rules(window).pads && !self.quick_suspend.paused());
    }

    /// Handles one SDL event while streaming.
    pub fn handle(
        &mut self,
        event: &SDL_Event,
        window: *mut SDL_Window,
        renderer: *mut SDL_Renderer,
        pads: &mut Gamepads,
    ) {
        // SAFETY: union members read by event type.
        unsafe {
            let kind = SDL_EventType(event.r#type);
            if self.touchpad_swallows(event) {
                return;
            }
            if self.plan.mic_only && !mic_only_handles(kind) {
                return;
            }
            match kind {
                // Closing Microphone only's window goes back to the PCs; quitting still quits.
                SDL_EVENT_WINDOW_CLOSE_REQUESTED if self.plan.mic_only => {
                    self.exit = Some(Exit::User);
                }
                SDL_EVENT_QUIT if self.plan.mic_only => {
                    self.exit.get_or_insert(Exit::CloseWindow);
                }
                SDL_EVENT_QUIT | SDL_EVENT_WINDOW_CLOSE_REQUESTED => {
                    self.exit = Some(Exit::CloseWindow);
                }
                SDL_EVENT_RENDER_DEVICE_RESET | SDL_EVENT_RENDER_DEVICE_LOST => {
                    let reason = render_device_ended(kind).expect("a render device event");
                    eprintln!("nightjar: {reason}");
                    self.note_render_device_lost(reason.into());
                }
                SDL_EVENT_KEY_DOWN | SDL_EVENT_KEY_UP => self.key(&event.key, direct_motion::sent(event), window, pads),
                SDL_EVENT_TEXT_INPUT if !event.text.text.is_null() => {
                    let text = std::ffi::CStr::from_ptr(event.text.text).to_string_lossy();
                    self.text(&text);
                }
                SDL_EVENT_KEYMAP_CHANGED => self.report_keyboard_layout(),
                // What this app put there came from the PC or its own controls.
                SDL_EVENT_CLIPBOARD_UPDATE if !event.clipboard.owner => {
                    if let Some(step) = self.clipboard_sync.as_mut().map(|sync| sync.changed(Instant::now())) {
                        self.clipboard_sync_step(step);
                    }
                }
                SDL_EVENT_WINDOW_FOCUS_LOST => {
                    self.focused = false;
                    self.raise_all_keys();
                    if !self.settings.background_controllers {
                        self.session.input(|q| pads.release(q));
                    }
                    if self.captured {
                        self.set_capture(window, false);
                        self.captured = true; // recapture on focus
                    }
                }
                SDL_EVENT_WINDOW_FOCUS_GAINED => {
                    self.focused = true;
                    if self.captured && !self.menu_visible {
                        self.set_capture(window, true);
                    }
                }
                SDL_EVENT_MOUSE_MOTION => {
                    let m = event.motion;
                    if self.facts.touch && self.captured && matches!(m.which, SDL_TOUCH_MOUSEID | SDL_PEN_MOUSEID) { return; }
                    if !pointer_goes_out(self.captured, self.focused) {
                        return;
                    }
                    if self.absolute {
                        self.absolute_position(m.x, m.y, window, renderer);
                    } else {
                        self.direct_motion.send(m.xrel, m.yrel, self.settings.mouse_speed);
                    }
                }
                SDL_EVENT_MOUSE_BUTTON_DOWN | SDL_EVENT_MOUSE_BUTTON_UP => {
                    let b = event.button;
                    if direct_motion::sent(event) {
                        if let Some(button) = mouse_button_code(b.button, self.settings.mouse_nav_buttons) {
                            reconcile_sent_press(
                                &mut self.mouse_down,
                                button,
                                b.down,
                                self.direct_motion.is_sent_button_down(button),
                            );
                        }
                        return;
                    }
                    if self.facts.touch && self.captured && matches!(b.which, SDL_TOUCH_MOUSEID | SDL_PEN_MOUSEID) { return; }
                    if !self.captured {
                        // The first click only takes the pointer; the launch curtain's buttons
                        // have it while the curtain is down.
                        if b.down && !self.curtain_down() {
                            self.set_capture(window, true);
                            if self.absolute {
                                self.absolute_position(b.x, b.y, window, renderer);
                            }
                        }
                        return;
                    }
                    let Some(button) = mouse_button_code(b.button, self.settings.mouse_nav_buttons) else { return };
                    if !mouse_button_goes_out(self.captured, self.focused, b.down, self.mouse_down.contains(&button)) {
                        return;
                    }
                    if b.down {
                        self.mouse_down.insert(button);
                    } else {
                        self.mouse_down.remove(&button);
                        self.direct_motion.clear_sent_button(button);
                    }
                    self.session.input(|q| {
                        let _ = q.send_mouse_button(button, b.down);
                    });
                }
                SDL_EVENT_MOUSE_WHEEL => {
                    if !pointer_goes_out(self.captured, self.focused) || direct_motion::sent(event) {
                        return;
                    }
                    self.session.input(|q| send_wheel(&event.wheel, q));
                }
                SDL_EVENT_FINGER_DOWN | SDL_EVENT_FINGER_MOTION | SDL_EVENT_FINGER_UP | SDL_EVENT_FINGER_CANCELED
                | SDL_EVENT_PEN_MOTION | SDL_EVENT_PEN_DOWN | SDL_EVENT_PEN_UP | SDL_EVENT_PEN_AXIS
                | SDL_EVENT_PEN_BUTTON_DOWN | SDL_EVENT_PEN_BUTTON_UP | SDL_EVENT_PEN_PROXIMITY_OUT => {
                    if !self.facts.touch || !self.captured || !self.focused { return; }
                    let mut area = (0, 0);
                    SDL_GetCurrentRenderOutputSize(renderer, &mut area.0, &mut area.1);
                    let picture = self.picture_size(area);
                    let density = SDL_GetWindowPixelDensity(window).max(0.01);
                    let rect = viewport(picture, area, self.settings.video_scaling, self.settings.align_video_top);
                    self.session.input(|q| self.touch.handle(event, rect, area, density, q));
                }
                SDL_EVENT_GAMEPAD_ADDED => {
                    if let Err(why) = pads.added(event.gdevice.which) {
                        self.show_toast(why.message());
                    }
                    if self.controller_audio_available() { pads.auto_audio(); }
                    if self.sends_controllers() {
                        self.session.input(|q| pads.announce_new(q));
                        if let Some(refused) = pads.refused_notice() {
                            self.show_toast(refused);
                        }
                    } else if let Some(off) = self.controllers_off_notice(pads.pads.len()) {
                        self.add_toast(off);
                    }
                }
                SDL_EVENT_GAMEPAD_REMOVED => {
                    // Release simulated mouse/touchpad state under the input lock, then close the
                    // gamepad outside it so joining the Deck IMU worker cannot deadlock on `input`.
                    self.session.input(|q| pads.release_mouse(event.gdevice.which, q));
                    let removed = pads.removed(event.gdevice.which);
                    if let Some(index) = removed
                        && self.sends_controllers() {
                            // A controller the host had refused may have taken the freed slot:
                            // its neutral state and arrival follow the removed one's.
                            self.session.input(|q| {
                                pads.send_removed(index, q);
                                pads.announce_new(q);
                            });
                        }
                }
                SDL_EVENT_AUDIO_DEVICE_ADDED => {
                    if self.controller_audio_available() {
                        pads.auto_audio();
                        self.session.input(|q| pads.announce_new(q));
                    }
                }
                SDL_EVENT_AUDIO_DEVICE_REMOVED => {
                    pads.removed_audio_device(event.adevice.which);
                    if self.sends_controllers() {
                        self.session.input(|q| pads.announce_new(q));
                    }
                }
                _ => {
                    // A controller event the input filter applied while the loop waited in the
                    // present: only its action is left.
                    let acted = direct_motion::acted(event);
                    let focused = SDL_GetWindowFlags(window) & SDL_WINDOW_INPUT_FOCUS != 0;
                    let allowed = self.facts.role != AppRole::RemoteMonitor
                        && self.settings.controllers
                        && (focused || self.settings.background_controllers);
                    if acted.is_none() && !allowed {
                        return;
                    }
                    if launch_curtain::pad_input(kind) && self.curtain_down() && !self.menu_visible {
                        if kind == SDL_EVENT_GAMEPAD_BUTTON_DOWN
                            && pads.curtain_button_clean(event.gbutton.which)
                            && self.curtain_press(
                                launch_curtain::pad_press(
                                    event.gbutton.button,
                                    pads.swap_face_buttons,
                                ),
                                pads,
                            )
                        {
                            return;
                        }
                        match pads.handle_curtain(event) {
                            Some(Action::Quit) => self.exit = Some(Exit::User),
                            Some(Action::Menu) => self.toggle_menu(window, pads),
                            _ => {}
                        }
                        return;
                    }
                    let mut action = acted;
                    if acted.is_none() {
                        self.session.input(|q| action = pads.handle(event, q));
                    }
                    match action {
                        Some(Action::Quit) => self.exit = Some(Exit::User),
                        Some(Action::Menu) => self.toggle_menu(window, pads),
                        Some(Action::Mouse(active)) => self.show_toast(menu_extras::mouse_toast(pads, active, true)),
                        None => {}
                    }
                }
            }
        }
    }

    fn picture_size(&self, area: (i32, i32)) -> (i32, i32) {
        if self.facts.role == AppRole::InputOnly {
            return area;
        }
        let tex = self.texture.size();
        if tex.0 > 0 && tex.1 > 0 {
            tex
        } else {
            (self.facts.width.max(0), self.facts.height.max(0))
        }
    }

    fn absolute_position(&self, x: f32, y: f32, window: *mut SDL_Window, renderer: *mut SDL_Renderer) {
        // SAFETY: live window and renderer.
        let (density, (w, h)) = unsafe {
            let mut size = (0, 0);
            SDL_GetCurrentRenderOutputSize(renderer, &mut size.0, &mut size.1);
            (SDL_GetWindowPixelDensity(window).max(0.01), size)
        };
        let picture = self.picture_size((w, h));
        let rect = viewport(picture, (w, h), self.settings.video_scaling, self.settings.align_video_top);
        let (px, py) = (x * density - rect.x, y * density - rect.y);
        let (rw, rh) = (rect.w.max(1.0), rect.h.max(1.0));
        let cx = px.clamp(0.0, rw - 1.0);
        let cy = py.clamp(0.0, rh - 1.0);
        let (sw, sh) = (rw.min(32767.0), rh.min(32767.0));
        self.session.input(|q| {
            let _ = q.send_mouse_position(
                (cx * sw / rw) as i16,
                (cy * sh / rh) as i16,
                sw as i16,
                sh as i16,
            );
        });
    }

    /// `sent`: the input filter sent it while the loop waited in the present.
    pub(super) fn key(&mut self, k: &SDL_KeyboardEvent, sent: bool, window: *mut SDL_Window, pads: &mut Gamepads) {
        if k.repeat {
            // The PC repeats a held key itself; SDL's repeated text is that key's too.
            let held = key_input(k.scancode, k.r#mod, self.system_keys(), self.settings.right_alt_as_meta)
                .is_some_and(|key| self.keys_down.contains(&key.code));
            if k.down && held {
                self.text_field.key_down(text_field::types(k.key, k.r#mod));
            }
            return;
        }
        if sent {
            if let Some(KeyInput { code, .. }) =
                key_input(k.scancode, k.r#mod, self.system_keys(), self.settings.right_alt_as_meta)
                && reconcile_sent_press(&mut self.keys_down, code, k.down, self.direct_motion.is_sent_key_down(code))
                && self.captured
                && self.focused
            {
                self.text_field.key_down(text_field::types(k.key, k.r#mod));
            }
            return;
        }
        if self.plan.mic_only
            && !(k.down && mic_only_chord(crate::shortcuts_runtime::key_action(crate::shortcuts::Scope::Stream, k.scancode, k.r#mod)))
        {
            return;
        }
        // Nightjar's own chords (shortcuts.rs, the list the help shows); every other key goes to the PC.
        if k.down {
            use crate::shortcuts::{Action, Scope};
            use crate::shortcuts_runtime::key_action;
            match key_action(Scope::Stream, k.scancode, k.r#mod) {
                Some(Action::StreamControls) => {
                    self.toggle_menu(window, pads);
                    return;
                }
                Some(Action::StreamShortcuts) => {
                    self.toggle_shortcut_sheet(window, pads);
                    return;
                }
                Some(Action::FetchClipboard) => {
                    self.clipboard(false);
                    return;
                }
                Some(Action::SendClipboard) => {
                    self.clipboard(true);
                    return;
                }
                Some(Action::Disconnect) => {
                    self.exit = Some(Exit::User);
                    return;
                }
                Some(Action::Capture) => {
                    let on = !self.captured;
                    self.raise_all_keys();
                    self.set_capture(window, on);
                    let release = crate::shortcuts::chord_for(Scope::Stream, Action::Capture).map(crate::shortcuts::Chord::text);
                    self.show_toast(match (on, release) {
                        (true, Some(keys)) => trf("Input captured. {} releases it.", &[&keys]),
                        (true, None) => tr("Input captured.").to_owned(),
                        (false, _) => tr("Input released. Click the stream to capture it.").to_owned(),
                    });
                    return;
                }
                Some(Action::Overlay) => {
                    self.overlay_visible = !self.overlay_visible;
                    self.needs_redraw = true;
                    return;
                }
                Some(Action::MouseMode) => {
                    self.absolute = !self.absolute;
                    let on = self.captured;
                    self.set_capture(window, false);
                    self.set_capture(window, on);
                    self.show_toast(if self.absolute {
                        tr("Mouse: desktop pointer")
                    } else {
                        tr("Mouse: captured for games")
                    });
                    return;
                }
                Some(Action::Fullscreen) => {
                    // SAFETY: a live window.
                    unsafe {
                        let fullscreen = SDL_GetWindowFlags(window) & SDL_WINDOW_FULLSCREEN != 0;
                        SDL_SetWindowFullscreen(window, !fullscreen);
                    }
                    self.raise_all_keys();
                    return;
                }
                Some(Action::Minimize) => {
                    self.raise_all_keys();
                    // SAFETY: a live window.
                    unsafe { SDL_MinimizeWindow(window) };
                    return;
                }
                // F12 is the main loop's (a screenshot); the rest are the launcher's.
                _ => {}
            }
            // Enter and Escape on the launch curtain, while it is down.
            if self.curtain_press(launch_curtain::key_press(k.scancode), pads) {
                return;
            }
        }
        let Some(KeyInput { code, modifiers, flags }) =
            key_input(k.scancode, k.r#mod, self.system_keys(), self.settings.right_alt_as_meta)
        else {
            return;
        };
        if !key_goes_out(self.captured, self.focused, k.down, self.keys_down.contains(&code)) {
            return;
        }
        if k.down {
            self.keys_down.insert(code);
            self.text_field.key_down(text_field::types(k.key, k.r#mod));
        } else {
            self.keys_down.remove(&code);
            self.direct_motion.clear_sent_key(code);
        }
        self.session.input(|q| {
            let _ = q.send_keyboard(0x8000 | code, k.down, modifiers, flags);
        });
    }

    /// Text from SDL text input: only what a touch keyboard typed goes to the PC, as UTF-8.
    pub(super) fn text(&mut self, text: &str) {
        let Some(text) = self.text_field.text(text) else { return; };
        if !self.captured || !self.focused {
            return;
        }
        self.session.input(|q| {
            if let Err(error) = q.send_text(text) {
                eprintln!("nightjar: text field: could not send text: {error}");
            }
        });
    }
}

#[cfg(test)]
mod mic_only_tests {
    use super::*;

    #[test]
    fn microphone_only_keeps_only_the_chords_that_are_not_about_input() {
        use crate::shortcuts::Action;
        for kept in [Action::Disconnect, Action::Fullscreen, Action::Minimize] {
            assert!(mic_only_chord(Some(kept)), "{kept:?}");
        }
        for dropped in [Action::Capture, Action::MouseMode, Action::SendClipboard, Action::StreamControls] {
            assert!(!mic_only_chord(Some(dropped)), "{dropped:?}");
        }
        assert!(!mic_only_chord(None), "an ordinary key never goes");
        assert!(mic_only_handles(SDL_EVENT_KEY_DOWN) && !mic_only_handles(SDL_EVENT_MOUSE_MOTION));
    }

    #[test]
    fn unfocused_stream_drops_presses_but_still_releases_held_keys_and_buttons() {
        assert!(key_goes_out(true, true, true, false));
        assert!(!key_goes_out(true, false, true, false), "unfocused press must not reach host");
        assert!(!key_goes_out(false, true, true, false), "uncaptured press must not reach host");
        assert!(key_goes_out(true, false, false, true), "held key release still goes out");
        assert!(!key_goes_out(true, false, false, false), "unheld key release while unfocused is dropped");

        assert!(mouse_button_goes_out(true, true, true, false));
        assert!(!mouse_button_goes_out(true, false, true, false), "unfocused click must not reach host");
        assert!(mouse_button_goes_out(true, false, false, true), "held mouse button release still goes out");
        assert!(!mouse_button_goes_out(true, false, false, false), "unheld mouse button release while unfocused is dropped");
    }

    #[test]
    fn direct_motion_sent_presses_reconcile_before_early_returns_and_after_raise_all_keys() {
        let mut held = HashSet::new();
        // Press sent by DirectMotion and still down in filter: recorded even if capture later dropped.
        assert!(reconcile_sent_press(&mut held, mouse_button::LEFT, true, true));
        assert!(held.contains(&mouse_button::LEFT));
        // Matching release removes it.
        assert!(!reconcile_sent_press(&mut held, mouse_button::LEFT, false, false));
        assert!(held.is_empty());
        // Press already released by raise_all_keys() before SDL_PollEvent drained it: not re-added.
        assert!(!reconcile_sent_press(&mut held, mouse_button::LEFT, true, false));
        assert!(held.is_empty());
    }
}
