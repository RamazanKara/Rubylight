//! Gamepad release, Launch Curtain chord suppression, Guide-hold timing, and controller mouse helpers.

use nightjar_core::input::InputQueue;
use sdl3_sys::everything::*;

use crate::gamepad_types::*;

use super::pad::{GUIDE_TAP, Guide};
use super::protocol::send_state;
use super::{Action, Gamepads, Pad, quit_mask};

impl Gamepads {
    pub fn release(&mut self, queue: &mut InputQueue) {
        let mask = self.active_mask();
        for pad in &mut self.pads {
            pad.mouse.release(queue);
            pad.touchpad.release(queue);
            cancel_touchpad_contacts(pad, queue);
            pad.raw_buttons = 0;
            pad.c_down = false;
            pad.raw_axes = [0; 6];
            pad.clickpad_swallowed = 0;
            pad.back_waiting = false;
            pad.guide = Guide::Up;
            pad.buttons = 0;
            pad.lt = 0;
            pad.rt = 0;
            pad.ls = (0, 0);
            pad.rs = (0, 0);
            if pad.announced {
                send_state(pad, mask, queue);
            }
        }
    }

    /// Tracks local escape chords (Menu, Quit, Hold-Guide) while the Launch Curtain hides the PC
    /// screen, without sending controller state or short Guide taps to the host.
    pub fn handle_curtain(&mut self, event: &SDL_Event) -> Option<Action> {
        let back_as_guide = self.back_as_guide && !self.back_opens_menu;
        let back_opens_menu = self.back_opens_menu;
        let guide_hold = self.guide_hold;
        let quit = quit_mask(self.quit_combo);
        let menu = crate::shortcuts::stream_controls_mask();
        // SAFETY: union members read by event type.
        unsafe {
            let kind = SDL_EventType(event.r#type);
            if !matches!(
                kind,
                SDL_EVENT_GAMEPAD_BUTTON_DOWN | SDL_EVENT_GAMEPAD_BUTTON_UP
            ) {
                return None;
            }
            let b = event.gbutton;
            let pad = self.find(b.which)?;
            if usize::from(b.button) >= BUTTON_MAP.len() {
                return None;
            }
            let flag = 1 << b.button;
            let guide = i32::from(b.button) == SDL_GAMEPAD_BUTTON_GUIDE.0
                || (back_as_guide && i32::from(b.button) == SDL_GAMEPAD_BUTTON_BACK.0);
            if guide_hold.is_some() && guide {
                match (b.down, pad.guide) {
                    (true, _) if pad.raw_buttons != 0 => pad.guide = Guide::Passed,
                    (true, _) => {
                        pad.guide = Guide::Held(std::time::Instant::now());
                        pad.guide_bit = flag;
                        return None;
                    }
                    (false, Guide::Held(_) | Guide::Opened | Guide::Tapped(_)) => {
                        pad.guide = Guide::Up;
                        return None;
                    }
                    (false, Guide::Up | Guide::Passed) => pad.guide = Guide::Up,
                }
            } else if b.down && matches!(pad.guide, Guide::Held(_)) {
                pad.raw_buttons |= pad.guide_bit;
                pad.guide = Guide::Passed;
            }
            if back_opens_menu && i32::from(b.button) == SDL_GAMEPAD_BUTTON_BACK.0 {
                if !b.down {
                    return std::mem::take(&mut pad.back_waiting).then_some(Action::Menu);
                }
                let held = pad.raw_buttons | flag;
                if held != quit && held != menu {
                    pad.back_waiting = pad.raw_buttons != 0 && held & !quit == 0;
                    return (!pad.back_waiting).then_some(Action::Menu);
                }
                pad.back_waiting = true;
            } else if b.down {
                pad.raw_buttons |= flag;
            } else {
                pad.raw_buttons &= !flag;
            }
            let held = pad.raw_buttons
                | if pad.back_waiting {
                    1 << SDL_GAMEPAD_BUTTON_BACK.0
                } else {
                    0
                };
            if b.down && (held == quit || held == menu) {
                pad.raw_buttons = 0;
                pad.back_waiting = false;
                pad.guide = Guide::Up;
                return Some(if held == quit {
                    Action::Quit
                } else {
                    Action::Menu
                });
            }
            None
        }
    }

    /// Holding Guide, on the stream loop's tick: true when a Guide held for the setting's time opens
    /// the stream menu. A short press's release goes out here once the PC has seen it down (unless
    /// `suppress_host` is set while the Launch Curtain hides the screen).
    pub fn guide_tick(&mut self, now: std::time::Instant, queue: &mut InputQueue) -> bool {
        self.guide_tick_mode(now, false, queue)
    }

    pub fn guide_tick_mode(
        &mut self,
        now: std::time::Instant,
        suppress_host: bool,
        queue: &mut InputQueue,
    ) -> bool {
        let (pad_settings, mask) = (self.pad_settings(), self.active_mask());
        let mut open = false;
        for pad in &mut self.pads {
            match pad.guide {
                Guide::Held(at)
                    if self
                        .guide_hold
                        .is_some_and(|hold| now.duration_since(at) >= hold) =>
                {
                    pad.guide = Guide::Opened;
                    open = true;
                }
                Guide::Tapped(at) if suppress_host || now.duration_since(at) >= GUIDE_TAP => {
                    pad.raw_buttons &= !pad.guide_bit;
                    pad.guide = Guide::Up;
                    if !suppress_host {
                        pad.update(pad_settings);
                        send_state(pad, mask, queue);
                    }
                }
                _ => {}
            }
        }
        open
    }

    /// A Guide press waits on [`Self::guide_tick`]: the stream loop wakes often until it is settled.
    pub fn guide_waiting(&self) -> bool {
        self.pads
            .iter()
            .any(|pad| matches!(pad.guide, Guide::Held(_) | Guide::Tapped(_)))
    }

    pub fn update_mouse(&mut self, queue: &mut InputQueue) -> bool {
        let now = std::time::Instant::now();
        let mut active = false;
        for pad in &mut self.pads {
            active |= pad.mouse.active;
            pad.mouse.tick(
                pad.report(),
                self.analog_scrolling,
                self.mouse_fixed_step,
                now,
                queue,
            );
        }
        active
    }

    /// What the user chose for the controller's buttons in the stream menu.
    pub fn menu_buttons(&self) -> &nightjar_policy::stream_menu::Buttons {
        &self.menu_buttons
    }

    pub fn quit_combo(&self) -> crate::config::GamepadQuitCombo {
        self.quit_combo
    }

    /// Whether Back opens the stream controls, as the settings chose.
    pub fn back_opens_menu(&self) -> bool {
        self.back_opens_menu
    }

    /// Any controller drives the mouse.
    pub fn mouse_active(&self) -> bool {
        self.pads.iter().any(|pad| pad.mouse.active)
    }

    /// The stream menu's controller mouse switch, as in V+ PC: the first controller's mouse turns
    /// on, or every active one turns off. None without a controller.
    pub fn toggle_mouse(&mut self, queue: &mut InputQueue) -> Option<bool> {
        if self.mouse_active() {
            for pad in self.pads.iter_mut().filter(|pad| pad.mouse.active) {
                pad.mouse.toggle(queue);
            }
            return Some(false);
        }
        Some(self.pads.first_mut()?.mouse.toggle(queue))
    }

    pub fn release_mouse(&mut self, id: SDL_JoystickID, queue: &mut InputQueue) {
        if let Some(pad) = self.find(id) {
            pad.mouse.release(queue);
            pad.touchpad.release(queue);
            cancel_touchpad_contacts(pad, queue);
        }
    }
}

pub(super) fn cancel_touchpad_contacts(pad: &mut Pad, queue: &mut InputQueue) {
    pad.cancel_touchpad_contacts(queue);
}
