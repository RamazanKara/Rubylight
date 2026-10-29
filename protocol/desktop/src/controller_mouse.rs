use std::time::{Duration, Instant};

use nightjar_core::input::{packets::{button, mouse_button}, InputQueue};

use crate::config::AnalogScrolling;
use crate::gamepad::Report;

#[derive(Default)]
pub struct Mouse {
    pub active: bool,
    /// Switched on from the stream menu, which works when holding Start is off in the settings.
    from_menu: bool,
    start: Option<Instant>,
    last_tick: Option<Instant>,
    buttons: u32,
    step: u8,
}

fn movement(stick: (i16, i16)) -> (i16, i16) {
    let (x, y) = (f32::from(stick.0) * 4.0 / 32766.0, f32::from(stick.1) * 4.0 / 32766.0);
    let speed = x * x + y * y;
    ((x * speed) as i16, (y * speed) as i16)
}

impl Mouse {
    pub fn release(&mut self, queue: &mut InputQueue) {
        for (flag, button) in [(button::A, mouse_button::LEFT), (button::B, mouse_button::RIGHT)] {
            if self.buttons & flag != 0 { let _ = queue.send_mouse_button(button, false); }
        }
        self.buttons = 0;
        self.start = None;
        self.last_tick = None;
    }

    /// The stream menu's switch: flips the mouse whether or not holding Start does.
    pub fn toggle(&mut self, queue: &mut InputQueue) -> bool {
        self.release(queue);
        self.active = !self.active;
        self.from_menu = self.active;
        self.active
    }

    pub fn update(&mut self, report: Report, enabled: bool, precision: bool, now: Instant, queue: &mut InputQueue) -> Option<bool> {
        let mut changed = None;
        if enabled {
            if report.buttons & button::PLAY != 0 {
                self.start.get_or_insert(now);
            } else if self.start.take().is_some_and(|at| now.duration_since(at) >= Duration::from_millis(750)) {
                self.release(queue);
                self.active = !self.active;
                self.from_menu = false;
                changed = Some(self.active);
            }
        } else if self.active && !self.from_menu {
            self.release(queue);
            self.active = false;
            changed = Some(false);
        }
        if !self.active { return changed; }
        let pressed = report.buttons & !self.buttons;
        let released = self.buttons & !report.buttons;
        for (flag, button) in [(button::A, mouse_button::LEFT), (button::B, mouse_button::RIGHT)] {
            if (pressed | released) & flag != 0 { let _ = queue.send_mouse_button(button, pressed & flag != 0); }
        }
        if precision && pressed & button::Y != 0 { self.step = if self.step >= 128 { 1 } else { self.step.max(1) * 2 }; }
        if pressed & button::UP != 0 { let _ = queue.send_scroll(1); }
        if pressed & button::DOWN != 0 { let _ = queue.send_scroll(-1); }
        if pressed & button::LEFT != 0 { let _ = queue.send_hscroll(-1); }
        if pressed & button::RIGHT != 0 { let _ = queue.send_hscroll(1); }
        self.buttons = report.buttons;
        changed
    }

    pub fn tick(&mut self, report: Report, scrolling: AnalogScrolling, precision: bool, now: Instant, queue: &mut InputQueue) {
        if !self.active || self.last_tick.is_some_and(|at| now.duration_since(at) < Duration::from_millis(50)) { return; }
        self.last_tick = Some(now);
        let (pointer, scroll) = match scrolling {
            AnalogScrolling::Right => (report.ls, Some(report.rs)),
            AnalogScrolling::Left => (report.rs, Some(report.ls)),
            AnalogScrolling::Off => (report.ls, None),
        };
        let step = if precision && self.buttons & button::X != 0 { i16::from(self.step.max(1)) } else { 0 };
        for stick in [Some(pointer), (scrolling == AnalogScrolling::Off).then_some(report.rs)].into_iter().flatten() {
            let (mut x, mut y) = movement(stick);
            if step != 0 { x = x.signum() * step; y = y.signum() * step; }
            if x != 0 || y != 0 { let _ = queue.send_mouse_move(x, -y); }
        }
        if let Some(stick) = scroll {
            let (x, y) = movement(stick);
            if x != 0 { let _ = queue.send_high_res_hscroll(x); }
            if y != 0 { let _ = queue.send_high_res_scroll(y); }
        }
    }
}

#[derive(Default)]
pub struct Touchpad {
    contacts: std::collections::BTreeMap<(i32, i32), (f32, f32)>,
    remainder: (f32, f32),
    button: Option<u8>,
}

impl Touchpad {
    pub fn release(&mut self, queue: &mut InputQueue) {
        if let Some(button) = self.button.take() { let _ = queue.send_mouse_button(button, false); }
        self.contacts.clear();
        self.remainder = (0.0, 0.0);
    }

    pub fn button(&mut self, down: bool, queue: &mut InputQueue) {
        if down && self.button.is_none() {
            let button = if self.contacts.len() > 1 { mouse_button::RIGHT } else { mouse_button::LEFT };
            if queue.send_mouse_button(button, true).is_ok() { self.button = Some(button); }
        } else if !down {
            if let Some(button) = self.button.take() { let _ = queue.send_mouse_button(button, false); }
        }
    }

    pub fn touch(&mut self, event: &sdl3_sys::everything::SDL_GamepadTouchpadEvent, queue: &mut InputQueue) {
        use sdl3_sys::everything::*;
        let key = (event.touchpad, event.finger);
        if event.r#type == SDL_EVENT_GAMEPAD_TOUCHPAD_UP {
            self.contacts.remove(&key);
            self.remainder = (0.0, 0.0);
        } else if event.r#type == SDL_EVENT_GAMEPAD_TOUCHPAD_DOWN {
            self.contacts.insert(key, (event.x, event.y));
            self.remainder = (0.0, 0.0);
        } else if let Some(previous) = self.contacts.get_mut(&key) {
            let delta = (event.x - previous.0, event.y - previous.1);
            *previous = (event.x, event.y);
            if self.contacts.first_key_value().is_some_and(|(first, _)| *first == key) {
                let scrolling = self.contacts.len() > 1;
                let (x, y) = if scrolling { (delta.0 * 720.0, -delta.1 * 720.0) } else { (delta.0 * 1920.0, delta.1 * 1080.0) };
                let (x, y) = (x + self.remainder.0, y + self.remainder.1);
                self.remainder = (x.fract(), y.fract());
                if scrolling {
                    if x.abs() >= 1.0 { let _ = queue.send_high_res_hscroll(x as i16); }
                    if y.abs() >= 1.0 { let _ = queue.send_high_res_scroll(y as i16); }
                } else if x.abs() >= 1.0 || y.abs() >= 1.0 { let _ = queue.send_mouse_move(x as i16, y as i16); }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touchpad_scrolls_with_two_fingers_and_releases_the_original_click() {
        use sdl3_sys::everything::*;
        use nightjar_core::input::packets::magic;
        let mut touchpad = Touchpad::default();
        let mut queue = InputQueue::new(&nightjar_core::control::HostCapabilities { feature_flags: 1, app_version: [7, 1, 431, -1], ..Default::default() });
        let mut event = SDL_GamepadTouchpadEvent { r#type: SDL_EVENT_GAMEPAD_TOUCHPAD_DOWN, x: 0.25, y: 0.25, ..Default::default() };
        touchpad.touch(&event, &mut queue);
        event.r#type = SDL_EVENT_GAMEPAD_TOUCHPAD_MOTION;
        event.x = 0.5;
        touchpad.touch(&event, &mut queue);
        let mut packets = Vec::new();
        queue.drain(Instant::now(), false, &mut packets);
        assert!(packets.iter().any(|p| p.data == nightjar_core::input::packets::InputEvent::MouseMove { dx: 480, dy: 0 }.encode(true)));
        event.r#type = SDL_EVENT_GAMEPAD_TOUCHPAD_DOWN;
        event.finger = 1;
        touchpad.touch(&event, &mut queue);
        touchpad.button(true, &mut queue);
        event.finger = 0;
        event.r#type = SDL_EVENT_GAMEPAD_TOUCHPAD_MOTION;
        event.y = 0.5;
        touchpad.touch(&event, &mut queue);
        event.finger = 1;
        event.r#type = SDL_EVENT_GAMEPAD_TOUCHPAD_UP;
        touchpad.touch(&event, &mut queue);
        touchpad.release(&mut queue);
        packets.clear();
        queue.drain(Instant::now(), false, &mut packets);
        assert!(packets.iter().any(|p| u32::from_le_bytes(p.data[4..8].try_into().unwrap()) == magic::SCROLL));
        assert_eq!(packets.last().unwrap().data, nightjar_core::input::packets::InputEvent::MouseButton { button: mouse_button::RIGHT, down: false }.encode(true));
        assert!(touchpad.contacts.is_empty());
        assert!(touchpad.button.is_none());
    }

    #[test]
    fn hold_start_toggles_and_releasing_input_clears_mouse_buttons() {
        let mut mouse = Mouse::default();
        let mut queue = InputQueue::new(&nightjar_core::control::HostCapabilities::default());
        let now = Instant::now();
        let held = Report { buttons: button::PLAY, ..Default::default() };
        mouse.update(held, true, true, now, &mut queue);
        assert_eq!(mouse.update(Report::default(), true, true, now + Duration::from_millis(749), &mut queue), None);
        mouse.update(held, true, true, now, &mut queue);
        assert_eq!(mouse.update(Report::default(), true, true, now + Duration::from_millis(750), &mut queue), Some(true));
        mouse.update(Report { buttons: button::A | button::B | button::X | button::Y, ls: (32767, 0), ..Default::default() }, true, true, now, &mut queue);
        assert_eq!(mouse.step, 2);
        let before = queue.len();
        mouse.release(&mut queue);
        assert_eq!(queue.len(), before + 2);
        assert_eq!(mouse.buttons, 0);
        assert!(mouse.start.is_none());
        assert!(mouse.active);
        assert_eq!(mouse.update(Report::default(), false, true, now, &mut queue), Some(false));
    }

    /// The stream menu's switch holds when holding Start is off in the settings (as in V+ PC);
    /// switching the setting off still ends a mouse that holding Start started.
    #[test]
    fn the_menu_switch_works_without_the_start_hold_setting() {
        let mut mouse = Mouse::default();
        let mut queue = InputQueue::new(&nightjar_core::control::HostCapabilities::default());
        let now = Instant::now();
        assert!(mouse.toggle(&mut queue));
        assert_eq!(mouse.update(Report { buttons: button::A, ..Default::default() }, false, false, now, &mut queue), None);
        assert!(mouse.active, "a button press with the setting off keeps the menu's mouse");
        assert_eq!(mouse.buttons, button::A, "and A clicks");
        assert!(!mouse.toggle(&mut queue));
        assert_eq!(mouse.buttons, 0, "switching off lets go of the buttons");

        let held = Report { buttons: button::PLAY, ..Default::default() };
        mouse.update(held, true, false, now, &mut queue);
        assert_eq!(mouse.update(Report::default(), true, false, now + Duration::from_millis(750), &mut queue), Some(true));
        assert_eq!(mouse.update(Report::default(), false, false, now, &mut queue), Some(false));
    }

    #[test]
    fn stick_response_and_precision_steps_match_android() {
        assert_eq!(movement((0, 0)), (0, 0));
        assert_eq!(movement((16383, 0)), (8, 0));
        assert_eq!(movement((32766, -32766)), (128, -128));
        let mut mouse = Mouse { active: true, ..Default::default() };
        let mut queue = InputQueue::new(&nightjar_core::control::HostCapabilities::default());
        let now = Instant::now();
        for expected in [2, 4, 8, 16, 32, 64, 128, 1, 2] {
            mouse.update(Report { buttons: button::Y, ..Default::default() }, true, true, now, &mut queue);
            assert_eq!(mouse.step, expected);
            mouse.update(Report::default(), true, true, now, &mut queue);
        }
        mouse.tick(Report { ls: (32766, 0), ..Default::default() }, AnalogScrolling::Right, false, now, &mut queue);
        let queued = queue.len();
        mouse.tick(Report { ls: (32766, 0), ..Default::default() }, AnalogScrolling::Right, false, now + Duration::from_millis(49), &mut queue);
        assert_eq!(queue.len(), queued);
    }
}
