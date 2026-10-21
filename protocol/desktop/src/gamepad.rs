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
    Menu,
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
        let Some(index) = (0..MAX_PADS as u8).find(|i| !used.contains(i)) else {
            unsafe { SDL_CloseGamepad(handle) };
            return;
        };
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

    pub fn active_mask(&self) -> u16 {
        self.pads.iter().fold(0, |m, p| m | (1 << p.index))
    }

    pub fn prepare(&self, motion: bool) -> String {
        self.pads.iter().map(|pad| {
            let (kind, buttons, mut caps) = describe(pad);
            if !motion {
                caps &= !(capability::ACCEL | capability::GYRO);
            }
            let kind = if kind == CTYPE_UNKNOWN { CTYPE_XBOX } else { kind };
            format!("{}:{kind}:{caps}:{buttons}", pad.index)
        }).collect::<Vec<_>>().join(",")
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
            announce(pad, mask, self.motion, queue);
        }
    }

    pub fn announce_new(&mut self, queue: &mut InputQueue) {
        let mask = self.active_mask();
        for pad in self.pads.iter_mut().filter(|p| !p.announced) {
            announce(pad, mask, self.motion, queue);
        }
    }

    /// The host forgets a controller: a neutral state with it out of the mask.
    pub fn send_removed(&self, index: u8, queue: &mut InputQueue) {
        let mask = self.active_mask();
        let _ = queue.send_controller(u16::from(index), mask, 0, 0, 0, (0, 0), (0, 0));
    }

    pub fn release(&mut self, queue: &mut InputQueue) {
        let mask = self.active_mask();
        for pad in &mut self.pads {
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
                    if pad.buttons == button::PLAY | button::BACK {
                        pad.buttons = 0;
                        send_state(pad, mask, queue);
                        return Some(Action::Menu);
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

    pub fn adaptive_triggers(&self, index: u16, flags: u8, type_left: u8, type_right: u8, left: &[u8; 10], right: &[u8; 10]) {
        if let Some(pad) = self.by_index(index) {
            unsafe {
                if SDL_GetGamepadType(pad.handle) == SDL_GAMEPAD_TYPE_PS5 {
                    let effect = trigger_effect(flags, type_left, type_right, left, right);
                    SDL_SendGamepadEffect(pad.handle, effect.as_ptr().cast(), effect.len() as i32);
                }
            }
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
        for pad in &self.pads {
            self.adaptive_triggers(u16::from(pad.index), 0x0C, 0x05, 0x05, &[0; 10], &[0; 10]);
        }
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

// SDL's DS5EffectsState_t excludes the USB/Bluetooth report framing and CRC.
fn trigger_effect(flags: u8, type_left: u8, type_right: u8, left: &[u8; 10], right: &[u8; 10]) -> [u8; 47] {
    let mut effect = [0; 47];
    effect[0] = flags & 0x0C;
    effect[10] = type_right;
    effect[11..21].copy_from_slice(right);
    effect[21] = type_left;
    effect[22..32].copy_from_slice(left);
    effect
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

fn describe(pad: &Pad) -> (u8, u32, u16) {
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
        (kind, supported, caps)
    }
}

fn announce(pad: &mut Pad, mask: u16, motion: bool, queue: &mut InputQueue) {
    let (kind, supported, mut caps) = describe(pad);
    if !motion { caps &= !(capability::ACCEL | capability::GYRO); }
    let _ = queue.send_controller_arrival(pad.index, mask, kind, supported, caps);
    pad.announced = true;
    if caps & capability::BATTERY_STATE != 0 {
        let mut percent = -1;
        let state = unsafe { SDL_GetGamepadPowerInfo(pad.handle, &mut percent) };
        send_battery(pad, state, percent, queue);
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

    static RUMBLE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    unsafe extern "C" fn record_rumble(_: *mut std::ffi::c_void, low: u16, high: u16) -> bool {
        RUMBLE.store((u32::from(low) << 16) | u32::from(high), std::sync::atomic::Ordering::SeqCst);
        true
    }

    /// An SDL virtual gamepad: arrival, a button press and stick motion reach the host's
    /// input queue, and the host's rumble reaches the device.
    #[test]
    fn virtual_gamepad_round_trip() {
        use nightjar_core::control::HostCapabilities;
        // SAFETY: SDL calls on this test's thread; the descriptor outlives the attach.
        unsafe {
            assert!(SDL_Init(SDL_INIT_GAMEPAD), "{}", crate::sdl::error());
            let mut desc: SDL_VirtualJoystickDesc = std::mem::zeroed();
            desc.version = std::mem::size_of::<SDL_VirtualJoystickDesc>() as u32;
            desc.r#type = SDL_JOYSTICK_TYPE_GAMEPAD.0 as u16;
            desc.naxes = SDL_GAMEPAD_AXIS_COUNT.0 as u16;
            desc.nbuttons = SDL_GAMEPAD_BUTTON_COUNT.0 as u16;
            desc.button_mask = (1u32 << SDL_GAMEPAD_BUTTON_COUNT.0) - 1;
            desc.axis_mask = (1u32 << SDL_GAMEPAD_AXIS_COUNT.0) - 1;
            desc.name = c"Nightjar test pad".as_ptr();
            desc.Rumble = Some(record_rumble);
            let id = SDL_AttachVirtualJoystick(&desc);
            assert!(id.0 != 0, "{}", crate::sdl::error());

            let mut pads = Gamepads::default();
            pads.added(id);
            assert_eq!(pads.pads.len(), 1, "{}", crate::sdl::error());
            let mut queue = InputQueue::new(&HostCapabilities {
                app_version: [7, 1, 431, -1],
                feature_flags: 0x03,
                ..Default::default()
            });
            let before = queue.len();
            pads.announce_all(&mut queue);
            assert!(queue.len() > before, "arrival queued");

            let joystick = SDL_GetGamepadJoystick(pads.pads[0].handle);
            SDL_SetJoystickVirtualButton(joystick, SDL_GAMEPAD_BUTTON_SOUTH.0, true);
            SDL_SetJoystickVirtualAxis(joystick, SDL_GAMEPAD_AXIS_LEFTY.0, -32768);
            SDL_UpdateJoysticks();
            let queued = queue.len();
            let mut event: SDL_Event = std::mem::zeroed();
            while SDL_PollEvent(&mut event) {
                pads.handle(&event, &mut queue);
            }
            assert_eq!(pads.pads[0].buttons, button::A);
            assert_eq!(pads.pads[0].ls.1, 32767, "stick up is positive for the host");
            assert!(queue.len() > queued, "controller state queued");

            pads.rumble(0, 0x1234, 0x5678);
            assert_eq!(RUMBLE.load(std::sync::atomic::Ordering::SeqCst), 0x1234_5678);

            pads.release(&mut queue);
            assert_eq!(pads.pads[0].buttons, 0);
            assert_eq!(pads.pads[0].ls, (0, 0));
            SDL_SetJoystickVirtualButton(joystick, SDL_GAMEPAD_BUTTON_START.0, true);
            SDL_SetJoystickVirtualButton(joystick, SDL_GAMEPAD_BUTTON_BACK.0, true);
            SDL_UpdateJoysticks();
            let mut menu = false;
            while SDL_PollEvent(&mut event) {
                menu |= pads.handle(&event, &mut queue) == Some(Action::Menu);
            }
            assert!(menu);
            assert_eq!(pads.pads[0].buttons, 0);

            pads.close_all();
            SDL_DetachVirtualJoystick(id);
            SDL_QuitSubSystem(SDL_INIT_GAMEPAD);
        }
    }

    #[test]
    fn button_map_follows_sdl_order() {
        assert_eq!(BUTTON_MAP[SDL_GAMEPAD_BUTTON_SOUTH.0 as usize], button::A);
        assert_eq!(BUTTON_MAP[SDL_GAMEPAD_BUTTON_START.0 as usize], button::PLAY);
        assert_eq!(BUTTON_MAP[SDL_GAMEPAD_BUTTON_DPAD_RIGHT.0 as usize], button::RIGHT);
        assert_eq!(BUTTON_MAP[SDL_GAMEPAD_BUTTON_TOUCHPAD.0 as usize], button::TOUCHPAD);
    }

    #[test]
    fn dualsense_effect_changes_only_requested_triggers() {
        let effect = trigger_effect(0xFF, 0x21, 0x26, &[3; 10], &[7; 10]);
        assert_eq!(effect[0], 0x0C);
        assert_eq!(&effect[1..10], &[0; 9]);
        assert_eq!(effect[10], 0x26);
        assert_eq!(&effect[11..21], &[7; 10]);
        assert_eq!(effect[21], 0x21);
        assert_eq!(&effect[22..32], &[3; 10]);
        assert_eq!(&effect[32..], &[0; 15]);
    }
}
