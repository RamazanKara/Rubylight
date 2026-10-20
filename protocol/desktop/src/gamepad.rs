//! Controllers through SDL3: every gamepad is opened at once so it can drive the launcher,
//! and during a stream each one is a host controller with rumble, trigger rumble, motion,
//! touchpad, battery and LED.
//!
//! The mapping (button flags, stick inversion, trigger scaling, motion units, controller
//! types and capabilities, the Start+Select+LB+RB quit combo) follows moonlight-qt's
//! `app/streaming/input/gamepad.cpp` (GPL-3.0, Cameron Gutman and contributors).

use nightjar_core::input::packets::{button, capability, motion_type, touch_event};
use nightjar_core::input::InputQueue;
use sdl3_sys::everything::*;

/// `LI_CTYPE_*`.
const CTYPE_UNKNOWN: u8 = 0;
const CTYPE_XBOX: u8 = 1;
const CTYPE_PS: u8 = 2;
const CTYPE_NINTENDO: u8 = 3;

/// `LI_BATTERY_STATE_*`.
const BATTERY_UNKNOWN: u8 = 0x00;
const BATTERY_NOT_PRESENT: u8 = 0x01;
const BATTERY_DISCHARGING: u8 = 0x02;
const BATTERY_CHARGING: u8 = 0x03;
const BATTERY_FULL: u8 = 0x05;
const BATTERY_PERCENT_UNKNOWN: u8 = 0xFF;

const MAX_PADS: usize = 16;

/// SDL gamepad buttons in `SDL_GamepadButton` order, as Moonlight button flags.
const BUTTON_MAP: [u32; 21] = [
    button::A,
    button::B,
    button::X,
    button::Y,
    button::BACK,
    button::SPECIAL,
    button::PLAY,
    button::LS_CLK,
    button::RS_CLK,
    button::LB,
    button::RB,
    button::UP,
    button::DOWN,
    button::LEFT,
    button::RIGHT,
    button::MISC,
    button::PADDLE1,
    button::PADDLE2,
    button::PADDLE3,
    button::PADDLE4,
    button::TOUCHPAD,
];

pub struct Pad {
    pub id: SDL_JoystickID,
    pub handle: *mut SDL_Gamepad,
    /// Host controller number.
    pub index: u8,
    buttons: u32,
    lt: u8,
    rt: u8,
    ls: (i16, i16),
    rs: (i16, i16),
    announced: bool,
}

/// What a controller asked the UI to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Start+Select+LB+RB: leave the stream.
    Quit,
}

#[derive(Default)]
pub struct Gamepads {
    pub pads: Vec<Pad>,
    pub swap_face_buttons: bool,
    pub motion: bool,
}

fn invert(value: i16) -> i16 {
    -(value.max(-32767))
}

impl Gamepads {
    /// Opens a newly attached gamepad.
    pub fn added(&mut self, id: SDL_JoystickID) {
        if self.pads.iter().any(|p| p.id == id) {
            return;
        }
        // SAFETY: opening a gamepad SDL reported.
        let handle = unsafe { SDL_OpenGamepad(id) };
        if handle.is_null() {
            return;
        }
        let used: Vec<u8> = self.pads.iter().map(|p| p.index).collect();
        let index = (0..MAX_PADS as u8).find(|i| !used.contains(i)).unwrap_or(0);
        self.pads.push(Pad {
            id,
            handle,
            index,
            buttons: 0,
            lt: 0,
            rt: 0,
            ls: (0, 0),
            rs: (0, 0),
            announced: false,
        });
    }

    /// Closes a removed gamepad; returns its host number.
    pub fn removed(&mut self, id: SDL_JoystickID) -> Option<u8> {
        let position = self.pads.iter().position(|p| p.id == id)?;
        let pad = self.pads.remove(position);
        // SAFETY: a gamepad we opened.
        unsafe { SDL_CloseGamepad(pad.handle) };
        Some(pad.index)
    }

    fn active_mask(&self) -> u16 {
        self.pads.iter().fold(0, |m, p| m | (1 << p.index))
    }

    pub fn find(&mut self, id: SDL_JoystickID) -> Option<&mut Pad> {
        self.pads.iter_mut().find(|p| p.id == id)
    }

    pub fn by_index(&self, index: u16) -> Option<&Pad> {
        self.pads.iter().find(|p| u16::from(p.index) == index)
    }

    /// Tells the host about every controller (at stream start).
    pub fn announce_all(&mut self, queue: &mut InputQueue) {
        let mask = self.active_mask();
        for pad in &mut self.pads {
            announce(pad, mask, queue);
        }
    }

    pub fn announce_new(&mut self, queue: &mut InputQueue) {
        let mask = self.active_mask();
        for pad in self.pads.iter_mut().filter(|p| !p.announced) {
            announce(pad, mask, queue);
        }
    }

    /// The host forgets a controller: a neutral state with it out of the mask.
    pub fn send_removed(&self, index: u8, queue: &mut InputQueue) {
        let mask = self.active_mask();
        let _ = queue.send_controller(u16::from(index), mask, 0, 0, 0, (0, 0), (0, 0));
    }

    /// Applies an SDL gamepad event to the host's controller state. Returns an action for
    /// the special combos.
    pub fn handle(&mut self, event: &SDL_Event, queue: &mut InputQueue) -> Option<Action> {
        let swap = self.swap_face_buttons;
        let motion = self.motion;
        let mask = self.active_mask();
        // SAFETY: union members read by event type.
        unsafe {
            let kind = SDL_EventType(event.r#type);
            match kind {
                SDL_EVENT_GAMEPAD_BUTTON_DOWN | SDL_EVENT_GAMEPAD_BUTTON_UP => {
                    let b = event.gbutton;
                    let pad = self.find(b.which)?;
                    let mut index = usize::from(b.button);
                    if swap {
                        index = match index {
                            0 => 1,
                            1 => 0,
                            2 => 3,
                            3 => 2,
                            other => other,
                        };
                    }
                    let flag = *BUTTON_MAP.get(index)?;
                    if b.down {
                        pad.buttons |= flag;
                    } else {
                        pad.buttons &= !flag;
                    }
                    let quit = button::PLAY | button::BACK | button::LB | button::RB;
                    if pad.buttons == quit {
                        pad.buttons = 0;
                        send_state(pad, mask, queue);
                        return Some(Action::Quit);
                    }
                    send_state(pad, mask, queue);
                }
                SDL_EVENT_GAMEPAD_AXIS_MOTION => {
                    let a = event.gaxis;
                    let pad = self.find(a.which)?;
                    let trigger = |v: i16| (i32::from(v.max(0)) * 255 / 32767) as u8;
                    match SDL_GamepadAxis(i32::from(a.axis)) {
                        SDL_GAMEPAD_AXIS_LEFTX => pad.ls.0 = a.value,
                        SDL_GAMEPAD_AXIS_LEFTY => pad.ls.1 = invert(a.value),
                        SDL_GAMEPAD_AXIS_RIGHTX => pad.rs.0 = a.value,
                        SDL_GAMEPAD_AXIS_RIGHTY => pad.rs.1 = invert(a.value),
                        SDL_GAMEPAD_AXIS_LEFT_TRIGGER => pad.lt = trigger(a.value),
                        SDL_GAMEPAD_AXIS_RIGHT_TRIGGER => pad.rt = trigger(a.value),
                        _ => return None,
                    }
                    send_state(pad, mask, queue);
                }
                SDL_EVENT_GAMEPAD_TOUCHPAD_DOWN
                | SDL_EVENT_GAMEPAD_TOUCHPAD_MOTION
                | SDL_EVENT_GAMEPAD_TOUCHPAD_UP => {
                    let t = event.gtouchpad;
                    let pad = self.find(t.which)?;
                    let phase = match kind {
                        SDL_EVENT_GAMEPAD_TOUCHPAD_DOWN => touch_event::DOWN,
                        SDL_EVENT_GAMEPAD_TOUCHPAD_UP => touch_event::UP,
                        _ => touch_event::MOVE,
                    };
                    let _ = queue.send_controller_touch(
                        pad.index,
                        phase,
                        t.touchpad as u8,
                        t.finger as u32,
                        t.x,
                        t.y,
                        t.pressure,
                    );
                }
                SDL_EVENT_GAMEPAD_SENSOR_UPDATE if motion => {
                    let s = event.gsensor;
                    let pad = self.find(s.which)?;
                    let [x, y, z] = s.data;
                    match SDL_SensorType(s.sensor) {
                        SDL_SENSOR_ACCEL => {
                            let _ = queue.send_controller_motion(pad.index, motion_type::ACCEL, x, y, z);
                        }
                        SDL_SENSOR_GYRO => {
                            let deg = 57.295_78;
                            let _ = queue.send_controller_motion(
                                pad.index,
                                motion_type::GYRO,
                                x * deg,
                                y * deg,
                                z * deg,
                            );
                        }
                        _ => {}
                    }
                }
                SDL_EVENT_JOYSTICK_BATTERY_UPDATED => {
                    let b = event.jbattery;
                    let pad = self.find(b.which)?;
                    send_battery(pad, b.state, b.percent, queue);
                }
                _ => {}
            }
        }
        None
    }

    /// Rumble from the host; motor speeds 0-65535.
    pub fn rumble(&self, index: u16, low: u16, high: u16) {
        if let Some(pad) = self.by_index(index) {
            // SAFETY: an open gamepad. A long duration; the host sends 0 to stop.
            unsafe { SDL_RumbleGamepad(pad.handle, low, high, 30_000) };
        }
    }

    pub fn rumble_triggers(&self, index: u16, left: u16, right: u16) {
        if let Some(pad) = self.by_index(index) {
            // SAFETY: an open gamepad.
            unsafe { SDL_RumbleGamepadTriggers(pad.handle, left, right, 30_000) };
        }
    }

    pub fn set_led(&self, index: u16, r: u8, g: u8, b: u8) {
        if let Some(pad) = self.by_index(index) {
            // SAFETY: an open gamepad.
            unsafe { SDL_SetGamepadLED(pad.handle, r, g, b) };
        }
    }

    /// The host wants motion data from a controller (rate 0 stops it).
    pub fn set_motion(&self, index: u16, kind: u8, rate_hz: u16) {
        if !self.motion {
            return;
        }
        let Some(pad) = self.by_index(index) else {
            return;
        };
        let sensor = match kind {
            motion_type::ACCEL => SDL_SENSOR_ACCEL,
            motion_type::GYRO => SDL_SENSOR_GYRO,
            _ => return,
        };
        // SAFETY: an open gamepad.
        unsafe { SDL_SetGamepadSensorEnabled(pad.handle, sensor, rate_hz != 0) };
    }

    /// Stops every motor and sensor (stream end).
    pub fn reset(&mut self) {
        for pad in &mut self.pads {
            // SAFETY: open gamepads.
            unsafe {
                SDL_RumbleGamepad(pad.handle, 0, 0, 0);
                SDL_RumbleGamepadTriggers(pad.handle, 0, 0, 0);
                SDL_SetGamepadSensorEnabled(pad.handle, SDL_SENSOR_ACCEL, false);
                SDL_SetGamepadSensorEnabled(pad.handle, SDL_SENSOR_GYRO, false);
            }
            pad.announced = false;
            pad.buttons = 0;
            pad.lt = 0;
            pad.rt = 0;
            pad.ls = (0, 0);
            pad.rs = (0, 0);
        }
    }

    pub fn close_all(&mut self) {
        for pad in self.pads.drain(..) {
            // SAFETY: gamepads we opened.
            unsafe { SDL_CloseGamepad(pad.handle) };
        }
    }
}

fn send_state(pad: &Pad, mask: u16, queue: &mut InputQueue) {
    let _ = queue.send_controller(
        u16::from(pad.index),
        mask,
        pad.buttons,
        pad.lt,
        pad.rt,
        pad.ls,
        pad.rs,
    );
}

fn announce(pad: &mut Pad, mask: u16, queue: &mut InputQueue) {
    // SAFETY: an open gamepad; read-only queries.
    unsafe {
        let kind = match SDL_GetGamepadType(pad.handle) {
            SDL_GAMEPAD_TYPE_XBOX360 | SDL_GAMEPAD_TYPE_XBOXONE => CTYPE_XBOX,
            SDL_GAMEPAD_TYPE_PS3 | SDL_GAMEPAD_TYPE_PS4 | SDL_GAMEPAD_TYPE_PS5 => CTYPE_PS,
            SDL_GAMEPAD_TYPE_NINTENDO_SWITCH_PRO
            | SDL_GAMEPAD_TYPE_NINTENDO_SWITCH_JOYCON_LEFT
            | SDL_GAMEPAD_TYPE_NINTENDO_SWITCH_JOYCON_RIGHT
            | SDL_GAMEPAD_TYPE_NINTENDO_SWITCH_JOYCON_PAIR => CTYPE_NINTENDO,
            _ => CTYPE_UNKNOWN,
        };
        let mut supported = 0u32;
        for (i, flag) in BUTTON_MAP.iter().enumerate() {
            if SDL_GamepadHasButton(pad.handle, SDL_GamepadButton(i as i32)) {
                supported |= flag;
            }
        }
        let props = SDL_GetGamepadProperties(pad.handle);
        let has = |name: *const std::ffi::c_char| SDL_GetBooleanProperty(props, name, false);
        let mut caps = 0u16;
        if SDL_GamepadHasAxis(pad.handle, SDL_GAMEPAD_AXIS_LEFT_TRIGGER) {
            caps |= capability::ANALOG_TRIGGERS;
        }
        if has(SDL_PROP_GAMEPAD_CAP_RUMBLE_BOOLEAN) {
            caps |= capability::RUMBLE;
        }
        if has(SDL_PROP_GAMEPAD_CAP_TRIGGER_RUMBLE_BOOLEAN) {
            caps |= capability::TRIGGER_RUMBLE;
        }
        if has(SDL_PROP_GAMEPAD_CAP_RGB_LED_BOOLEAN) {
            caps |= capability::RGB_LED;
        }
        let touchpads = SDL_GetNumGamepadTouchpads(pad.handle);
        if touchpads > 0 {
            caps |= capability::TOUCHPAD;
        }
        if touchpads > 1 {
            caps |= capability::DUAL_TOUCHPAD;
        }
        if SDL_GamepadHasSensor(pad.handle, SDL_SENSOR_ACCEL) {
            caps |= capability::ACCEL;
        }
        if SDL_GamepadHasSensor(pad.handle, SDL_SENSOR_GYRO) {
            caps |= capability::GYRO;
        }
        let mut percent = -1;
        let state = SDL_GetGamepadPowerInfo(pad.handle, &mut percent);
        if state != SDL_POWERSTATE_UNKNOWN && state != SDL_POWERSTATE_NO_BATTERY && state != SDL_POWERSTATE_ERROR {
            caps |= capability::BATTERY_STATE;
        }
        let _ = queue.send_controller_arrival(pad.index, mask, kind, supported, caps);
        pad.announced = true;
        if caps & capability::BATTERY_STATE != 0 {
            send_battery(pad, state, percent, queue);
        }
    }
}

fn send_battery(pad: &Pad, state: SDL_PowerState, percent: i32, queue: &mut InputQueue) {
    let state_code = match state {
        SDL_POWERSTATE_ON_BATTERY => BATTERY_DISCHARGING,
        SDL_POWERSTATE_CHARGING => BATTERY_CHARGING,
        SDL_POWERSTATE_CHARGED => BATTERY_FULL,
        SDL_POWERSTATE_NO_BATTERY => BATTERY_NOT_PRESENT,
        _ => BATTERY_UNKNOWN,
    };
    let percent = if (0..=100).contains(&percent) {
        percent as u8
    } else {
        BATTERY_PERCENT_UNKNOWN
    };
    let _ = queue.send_controller_battery(pad.index, state_code, percent);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stick_y_inverts_without_overflow() {
        assert_eq!(invert(-32768), 32767);
        assert_eq!(invert(32767), -32767);
        assert_eq!(invert(0), 0);
    }

    #[test]
    fn button_map_follows_sdl_order() {
        assert_eq!(BUTTON_MAP[SDL_GAMEPAD_BUTTON_SOUTH.0 as usize], button::A);
        assert_eq!(BUTTON_MAP[SDL_GAMEPAD_BUTTON_START.0 as usize], button::PLAY);
        assert_eq!(BUTTON_MAP[SDL_GAMEPAD_BUTTON_DPAD_RIGHT.0 as usize], button::RIGHT);
        assert_eq!(BUTTON_MAP[SDL_GAMEPAD_BUTTON_TOUCHPAD.0 as usize], button::TOUCHPAD);
    }
}
