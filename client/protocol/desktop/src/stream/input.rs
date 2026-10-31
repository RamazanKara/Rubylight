//! Keyboard, mouse, touch and controller events while streaming.

use super::*;

/// Ctrl+Alt+Shift is Nightjar's own chord; every other one, a host's own menu chord such as
/// Ctrl+Alt+Backspace included, goes to the PC like any key.
pub(super) fn local_chord(modifiers: SDL_Keymod) -> bool {
    let has = |flag: SDL_Keymod| modifiers.0 & flag.0 != 0;
    has(SDL_KMOD_CTRL) && has(SDL_KMOD_ALT) && has(SDL_KMOD_SHIFT)
}

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

impl Streaming {
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
            #[cfg(windows)]
            if self.touchpad_swallows(event) {
                return;
            }
            match kind {
                SDL_EVENT_QUIT | SDL_EVENT_WINDOW_CLOSE_REQUESTED => {
                    self.exit = Some(Exit::CloseWindow);
                }
                SDL_EVENT_RENDER_DEVICE_RESET | SDL_EVENT_RENDER_DEVICE_LOST => {
                    let reason = render_device_ended(kind).expect("a render device event");
                    eprintln!("nightjar: {reason}");
                    self.exit.get_or_insert(Exit::Ended(Some(reason.into())));
                }
                SDL_EVENT_KEY_DOWN | SDL_EVENT_KEY_UP => self.key(&event.key, window, pads),
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
                    if !self.captured {
                        return;
                    }
                    if self.absolute {
                        self.absolute_position(m.x, m.y, window, renderer);
                    } else {
                        let (rx, ry) = self.relative_remainder;
                        let speed = self.settings.mouse_speed.clamp(25, 400) as f32 / 100.0;
                        let (dx, dy) = (m.xrel * speed + rx, m.yrel * speed + ry);
                        let (ix, iy) = (dx.trunc(), dy.trunc());
                        self.relative_remainder = (dx - ix, dy - iy);
                        if ix != 0.0 || iy != 0.0 {
                            self.session.input(|q| {
                                let _ = q.send_mouse_move(ix as i16, iy as i16);
                            });
                        }
                    }
                }
                SDL_EVENT_MOUSE_BUTTON_DOWN | SDL_EVENT_MOUSE_BUTTON_UP => {
                    let b = event.button;
                    if self.facts.touch && self.captured && matches!(b.which, SDL_TOUCH_MOUSEID | SDL_PEN_MOUSEID) { return; }
                    if !self.captured {
                        // The first click only takes the pointer.
                        if b.down {
                            self.set_capture(window, true);
                            if self.absolute {
                                self.absolute_position(b.x, b.y, window, renderer);
                            }
                        }
                        return;
                    }
                    let button = match i32::from(b.button) {
                        SDL_BUTTON_LEFT => mouse_button::LEFT,
                        SDL_BUTTON_MIDDLE => mouse_button::MIDDLE,
                        SDL_BUTTON_RIGHT => mouse_button::RIGHT,
                        SDL_BUTTON_X1 if self.settings.mouse_nav_buttons => mouse_button::X1,
                        SDL_BUTTON_X2 if self.settings.mouse_nav_buttons => mouse_button::X2,
                        _ => return,
                    };
                    if b.down {
                        self.mouse_down.insert(button);
                    } else {
                        self.mouse_down.remove(&button);
                    }
                    self.session.input(|q| {
                        let _ = q.send_mouse_button(button, b.down);
                    });
                }
                SDL_EVENT_MOUSE_WHEEL => {
                    if !self.captured {
                        return;
                    }
                    let w = event.wheel;
                    let flip = if w.direction == SDL_MOUSEWHEEL_FLIPPED { -1.0 } else { 1.0 };
                    let (x, y) = (w.x * flip, w.y * flip);
                    self.session.input(|q| {
                        if y != 0.0 {
                            let _ = q.send_high_res_scroll((y * 120.0).clamp(-32000.0, 32000.0) as i16);
                        }
                        if x != 0.0 {
                            let _ = q.send_high_res_hscroll((x * 120.0).clamp(-32000.0, 32000.0) as i16);
                        }
                    });
                }
                SDL_EVENT_FINGER_DOWN | SDL_EVENT_FINGER_MOTION | SDL_EVENT_FINGER_UP | SDL_EVENT_FINGER_CANCELED
                | SDL_EVENT_PEN_MOTION | SDL_EVENT_PEN_DOWN | SDL_EVENT_PEN_UP | SDL_EVENT_PEN_AXIS
                | SDL_EVENT_PEN_BUTTON_DOWN | SDL_EVENT_PEN_BUTTON_UP | SDL_EVENT_PEN_PROXIMITY_OUT => {
                    if !self.facts.touch || !self.captured || !self.focused { return; }
                    let mut area = (0, 0);
                    SDL_GetCurrentRenderOutputSize(renderer, &mut area.0, &mut area.1);
                    let picture = if self.facts.role == AppRole::InputOnly { area } else { self.texture.size() };
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
                    }
                }
                SDL_EVENT_GAMEPAD_REMOVED => {
                    self.session.input(|q| pads.release_mouse(event.gdevice.which, q));
                    if let Some(index) = pads.removed(event.gdevice.which) {
                        if self.sends_controllers() {
                            self.session.input(|q| pads.send_removed(index, q));
                        }
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
                    let focused = SDL_GetWindowFlags(window) & SDL_WINDOW_INPUT_FOCUS != 0;
                    if self.facts.role == AppRole::RemoteMonitor || !self.settings.controllers || (!focused && !self.settings.background_controllers) {
                        return;
                    }
                    let mut action = None;
                    self.session.input(|q| action = pads.handle(event, q));
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

    fn absolute_position(&self, x: f32, y: f32, window: *mut SDL_Window, renderer: *mut SDL_Renderer) {
        // SAFETY: live window and renderer.
        let (density, (w, h)) = unsafe {
            let mut size = (0, 0);
            SDL_GetCurrentRenderOutputSize(renderer, &mut size.0, &mut size.1);
            (SDL_GetWindowPixelDensity(window).max(0.01), size)
        };
        let picture = if self.facts.role == AppRole::InputOnly { (w, h) } else { self.texture.size() };
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

    pub(super) fn key(&mut self, k: &SDL_KeyboardEvent, window: *mut SDL_Window, pads: &mut Gamepads) {
        if k.repeat {
            // The PC repeats a held key itself; SDL's repeated text is that key's too.
            let held = key_input(k.scancode, k.r#mod, self.settings.capture_system_keys, self.settings.right_alt_as_meta)
                .is_some_and(|key| self.keys_down.contains(&key.code));
            if k.down && held {
                self.text_field.key_down(text_field::types(k.key, k.r#mod));
            }
            return;
        }
        if k.down && local_chord(k.r#mod) {
            match k.scancode {
                SDL_SCANCODE_TAB => {
                    self.toggle_menu(window, pads);
                    return;
                }
                SDL_SCANCODE_C => {
                    self.clipboard(false);
                    return;
                }
                SDL_SCANCODE_V => {
                    self.clipboard(true);
                    return;
                }
                SDL_SCANCODE_Q => {
                    self.exit = Some(Exit::User);
                    return;
                }
                SDL_SCANCODE_Z => {
                    let on = !self.captured;
                    self.raise_all_keys();
                    self.set_capture(window, on);
                    self.show_toast(if on {
                        tr("Input captured. Ctrl+Alt+Shift+Z releases it.")
                    } else {
                        tr("Input released. Click the stream to capture it.")
                    });
                    return;
                }
                SDL_SCANCODE_S => {
                    self.overlay_visible = !self.overlay_visible;
                    self.needs_redraw = true;
                    return;
                }
                SDL_SCANCODE_M => {
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
                SDL_SCANCODE_X => {
                    // SAFETY: a live window.
                    unsafe {
                        let fullscreen = SDL_GetWindowFlags(window) & SDL_WINDOW_FULLSCREEN != 0;
                        SDL_SetWindowFullscreen(window, !fullscreen);
                    }
                    self.raise_all_keys();
                    return;
                }
                SDL_SCANCODE_D => {
                    self.raise_all_keys();
                    // SAFETY: a live window.
                    unsafe { SDL_MinimizeWindow(window) };
                    return;
                }
                _ => {}
            }
        }
        if !self.captured {
            return;
        }
        let Some(KeyInput { code, modifiers, flags }) =
            key_input(k.scancode, k.r#mod, self.settings.capture_system_keys, self.settings.right_alt_as_meta)
        else {
            return;
        };
        if k.down {
            self.keys_down.insert(code);
            self.text_field.key_down(text_field::types(k.key, k.r#mod));
        } else {
            self.keys_down.remove(&code);
        }
        self.session.input(|q| {
            let _ = q.send_keyboard(0x8000 | code, k.down, modifiers, flags);
        });
    }

    /// Text from SDL text input: only what a touch keyboard typed goes to the PC, as UTF-8.
    pub(super) fn text(&mut self, text: &str) {
        let Some(text) = self.text_field.text(text) else { return; };
        if !self.captured {
            return;
        }
        self.session.input(|q| {
            if let Err(error) = q.send_text(text) {
                eprintln!("nightjar: text field: could not send text: {error}");
            }
        });
    }
}
