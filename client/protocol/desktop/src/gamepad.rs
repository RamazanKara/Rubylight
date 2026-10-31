//! Controllers through SDL3: every gamepad is opened at once so it can drive the launcher,
//! and during a stream each one is a host controller with rumble, trigger rumble, motion,
//! touchpad, battery and LED.
//!
//! The mapping (button flags, stick inversion, trigger scaling, motion units, controller
//! types and capabilities, the Start+Select+LB+RB quit combo) follows moonlight-qt's
//! `app/streaming/input/gamepad.cpp` (GPL-3.0, Cameron Gutman and contributors). The other quit
//! combos and the stream menu's controller mouse switch are ported from Moonlight V+ PC
//! (qiin2333/moonlight-qt).

use crate::strings::{en, trf};
use nightjar_core::input::packets::{button, capability, motion_type, touch_event};
use nightjar_core::input::InputQueue;
use serde::{Deserialize, Serialize};
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

/// Why [`Gamepads::added`] left a controller out.
#[derive(Debug, PartialEq)]
pub enum NotOpened {
    /// SDL could not open it; SDL's error.
    Failed(String),
    /// All [`MAX_PADS`] controllers the host takes are connected.
    Limit,
}

impl NotOpened {
    /// What the user is told.
    pub fn message(&self) -> String {
        match self {
            NotOpened::Failed(error) => trf("Controller could not be opened: {}", &[error]),
            NotOpened::Limit => trf("Controller limit reached: {} are connected", &[&MAX_PADS]),
        }
    }
}

/// SDL gamepad buttons in `SDL_GamepadButton` order, as Moonlight button flags.
pub const BUTTON_MAP: [u32; 21] = [
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

pub const BUTTON_NAMES: [&str; 21] = [
    en("A / South"), en("B / East"), en("X / West"), en("Y / North"), en("Back / Select"), en("Guide"), en("Start"),
    en("Left stick click"), en("Right stick click"), en("Left shoulder"), en("Right shoulder"),
    en("D-pad up"), en("D-pad down"), en("D-pad left"), en("D-pad right"), en("Share / Misc"),
    en("Paddle 1"), en("Paddle 2"), en("Paddle 3"), en("Paddle 4"), en("Touchpad click"),
];
pub const AXIS_NAMES: [&str; 6] = [en("Left stick X"), en("Left stick Y"), en("Right stick X"), en("Right stick Y"), en("Left trigger"), en("Right trigger")];

/// The "Connect as" choices: Automatic (zero) keeps the type SDL detects, the others are the
/// `LI_CTYPE_*` the host is told instead.
pub const CONNECT_AS: [(u8, &str); 4] = [
    (CTYPE_UNKNOWN, en("Automatic")), (CTYPE_XBOX, en("Xbox")), (CTYPE_PS, en("PlayStation")), (CTYPE_NINTENDO, en("Nintendo")),
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mapping {
    pub name: String,
    pub buttons: [Option<u32>; 21],
    /// Zero disables an axis; otherwise the signed source index plus one.
    pub axes: [i8; 6],
    pub deadzone: u8,
    /// One of [`CONNECT_AS`]; mappings saved before it existed load as Automatic.
    #[serde(deserialize_with = "lenient_connect_as")]
    pub connect_as: u8,
}

/// Anything but a known choice reads as Automatic, so one bad value (256, -1, "x", or a type a
/// newer Nightjar adds) never fails the whole file and with it every controller's mapping.
fn lenient_connect_as<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u8, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value
        .as_u64()
        .and_then(|v| u8::try_from(v).ok())
        .filter(|v| CONNECT_AS.iter().any(|(kind, _)| kind == v))
        .unwrap_or(CTYPE_UNKNOWN))
}

impl Default for Mapping {
    fn default() -> Self {
        Self { name: String::new(), buttons: [None; 21], axes: [1, 2, 3, 4, 5, 6], deadzone: 0, connect_as: CTYPE_UNKNOWN }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub buttons: u32,
    pub lt: u8,
    pub rt: u8,
    pub ls: (i16, i16),
    pub rs: (i16, i16),
}

impl Mapping {
    pub fn valid(&self) -> bool {
        self.deadzone <= 50
            && self.buttons.iter().flatten().all(|b| *b == 0 || BUTTON_MAP.contains(b))
            && self.axes.iter().enumerate().all(|(i, source)| {
                *source == 0 || if i < 4 { (1..=4).contains(&source.unsigned_abs()) } else { (5..=6).contains(source) }
            })
    }

    /// The type the pad arrives as: the pinned one, else `detected`. A value this version does
    /// not know counts as Automatic, so a newer Nightjar's choice never drops the mapping.
    pub fn arrival_type(&self, detected: u8) -> u8 {
        if self.connect_as != CTYPE_UNKNOWN && CONNECT_AS.iter().any(|(kind, _)| *kind == self.connect_as) {
            self.connect_as
        } else {
            detected
        }
    }

    /// Whether the pad is pinned as Xbox, which leaves out motion and the touchpad: an Xbox pad has
    /// neither, and hosts such as Rubylight turn any pad that reports them into a DualSense.
    pub fn drops_motion_and_touchpad(&self) -> bool {
        self.connect_as == CTYPE_XBOX
    }

    /// The capabilities the pad arrives with.
    pub fn arrival_capabilities(&self, caps: u16) -> u16 {
        if !self.drops_motion_and_touchpad() { return caps; }
        caps & !(capability::ACCEL | capability::GYRO | capability::TOUCHPAD | capability::DUAL_TOUCHPAD)
    }

    /// Whether Select+LB clicks the touchpad: pinned as PlayStation on a pad without a touchpad
    /// click, unless a button is already mapped to it.
    pub fn emulates_clickpad(&self, has_touchpad_click: bool) -> bool {
        self.connect_as == CTYPE_PS && !has_touchpad_click && !self.buttons.contains(&Some(button::TOUCHPAD))
    }

    pub fn map_buttons(&self, raw: u32, swap: bool) -> u32 {
        self.buttons.iter().enumerate().filter(|(i, _)| raw & (1 << i) != 0).fold(0, |buttons, (i, target)| {
            buttons | target.unwrap_or(BUTTON_MAP[if swap && i < 4 { i ^ 1 } else { i }])
        })
    }

    pub fn apply(&self, buttons: u32, axes: [i16; 6], swap: bool) -> Report {
        let mut mapped = [0; 6];
        for (value, source) in mapped.iter_mut().zip(self.axes) {
            if source != 0 {
                *value = axes[source.unsigned_abs() as usize - 1];
                if source < 0 { *value = invert(*value); }
            }
        }
        let trigger = |v: i16| (i32::from(v.max(0)) * 255 / 32767) as u8;
        Report {
            buttons: self.map_buttons(buttons, swap),
            lt: trigger(mapped[4]), rt: trigger(mapped[5]),
            ls: deadzone((mapped[0], invert(mapped[1])), self.deadzone),
            rs: deadzone((mapped[2], invert(mapped[3])), self.deadzone),
        }
    }
}

/// Select+LB pressed together, and nothing else, sends the touchpad click instead while both stay
/// held; `swallowed` holds those of the two not yet let go, which stay silent until released.
/// The Android client does the same for a pad pinned as PlayStation.
fn clickpad(buttons: u32, swallowed: &mut u32) -> u32 {
    let combo = button::BACK | button::LB;
    if *swallowed == 0 && buttons == combo { *swallowed = combo; }
    *swallowed &= buttons;
    (buttons & !*swallowed) | if *swallowed == combo { button::TOUCHPAD } else { 0 }
}

fn deadzone(value: (i16, i16), percent: u8) -> (i16, i16) {
    if percent == 0 { return value; }
    let (x, y) = (f32::from(value.0), f32::from(value.1));
    let length = x.hypot(y);
    let edge = f32::from(percent) * 32767.0 / 100.0;
    if length <= edge { return (0, 0); }
    let scale = ((length - edge) / (32767.0 - edge)).min(1.0) * 32767.0 / length;
    ((x * scale).round() as i16, (y * scale).round() as i16)
}

pub struct Pad {
    pub audio: Option<crate::controller_audio::Audio>,
    pub id: SDL_JoystickID,
    pub handle: *mut SDL_Gamepad,
    /// Host controller number.
    pub index: u8,
    pub key: String,
    pub name: String,
    pub mapping: Mapping,
    raw_buttons: u32,
    raw_axes: [i16; 6],
    mouse: crate::controller_mouse::Mouse,
    touchpad: crate::controller_mouse::Touchpad,
    buttons: u32,
    lt: u8,
    rt: u8,
    ls: (i16, i16),
    rs: (i16, i16),
    announced: bool,
    /// SDL reports a PlayStation pad or a touchpad click button: no Select+LB stand-in.
    has_touchpad_click: bool,
    clickpad_swallowed: u32,
}

impl Pad {
    pub fn preview(&self, mapping: &Mapping, swap: bool) -> Report {
        // Read physical state so the test view works without sending input to a PC.
        unsafe {
            let buttons = (0..21).filter(|i| SDL_GetGamepadButton(self.handle, SDL_GamepadButton(*i)))
                .fold(0, |bits, i| bits | (1 << i));
            let axes = std::array::from_fn(|i| SDL_GetGamepadAxis(self.handle, SDL_GamepadAxis(i as i32)));
            mapping.apply(buttons, axes, swap)
        }
    }

    fn update(&mut self, swap: bool) {
        let state = self.mapping.apply(self.raw_buttons, self.raw_axes, swap);
        self.buttons = if self.mapping.emulates_clickpad(self.has_touchpad_click) {
            clickpad(state.buttons, &mut self.clickpad_swallowed)
        } else {
            state.buttons
        };
        self.lt = state.lt;
        self.rt = state.rt;
        self.ls = state.ls;
        self.rs = state.rs;
    }

    pub fn supports_audio(&self) -> bool {
        unsafe { SDL_GetGamepadType(self.handle) == SDL_GAMEPAD_TYPE_PS5
            && SDL_GetGamepadConnectionState(self.handle) == SDL_JOYSTICK_CONNECTION_WIRED }
    }

    fn report(&self) -> Report { Report { buttons: self.buttons, lt: self.lt, rt: self.rt, ls: self.ls, rs: self.rs } }
}

/// What a controller asked the UI to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// The quit combo (Start+Select+LB+RB unless the settings choose another): leave the stream.
    Quit,
    Menu,
    Mouse(bool),
}

#[derive(Default)]
pub struct Gamepads {
    pub pads: Vec<Pad>,
    pub swap_face_buttons: bool,
    pub motion: bool,
    pub mappings: std::collections::BTreeMap<String, Mapping>,
    back_opens_menu: bool,
    quit_combo: crate::config::GamepadQuitCombo,
    mouse_emulation: bool,
    analog_scrolling: crate::config::AnalogScrolling,
    mouse_fixed_step: bool,
    touchpad_as_mouse: bool,
    disable_rumble: bool,
}

/// The physical buttons of a quit combo, as bits of `1 << SDL_GamepadButton`. Face buttons are
/// positions (A is south) like the rest of the local chords, so the Nintendo layout swap leaves
/// them where they are.
pub fn quit_mask(combo: crate::config::GamepadQuitCombo) -> u32 {
    use crate::config::GamepadQuitCombo as Combo;
    let bits = |buttons: &[SDL_GamepadButton]| buttons.iter().fold(0u32, |mask, b| mask | 1 << b.0);
    let shoulders = [SDL_GAMEPAD_BUTTON_LEFT_SHOULDER, SDL_GAMEPAD_BUTTON_RIGHT_SHOULDER];
    bits(&shoulders) | match combo {
        Combo::StartSelectLbRb => bits(&[SDL_GAMEPAD_BUTTON_START, SDL_GAMEPAD_BUTTON_BACK]),
        Combo::SelectLbRbY => bits(&[SDL_GAMEPAD_BUTTON_BACK, SDL_GAMEPAD_BUTTON_NORTH]),
        Combo::StartLbRbA => bits(&[SDL_GAMEPAD_BUTTON_START, SDL_GAMEPAD_BUTTON_SOUTH]),
        Combo::StartLbRbB => bits(&[SDL_GAMEPAD_BUTTON_START, SDL_GAMEPAD_BUTTON_EAST]),
        Combo::LbRbXY => bits(&[SDL_GAMEPAD_BUTTON_WEST, SDL_GAMEPAD_BUTTON_NORTH]),
        Combo::LbRbAB => bits(&[SDL_GAMEPAD_BUTTON_SOUTH, SDL_GAMEPAD_BUTTON_EAST]),
    }
}

fn invert(value: i16) -> i16 {
    -(value.max(-32767))
}

impl Gamepads {
    pub fn configure(&mut self, settings: &crate::config::Settings) {
        self.swap_face_buttons = settings.swap_face_buttons;
        self.motion = settings.controller_motion;
        self.back_opens_menu = settings.back_opens_menu;
        self.quit_combo = settings.gamepad_quit_combo;
        self.mouse_emulation = settings.controller_mouse;
        self.analog_scrolling = settings.analog_scrolling;
        self.mouse_fixed_step = settings.mouse_fixed_step;
        self.touchpad_as_mouse = settings.gamepad_touchpad_as_mouse;
        self.disable_rumble = settings.disable_rumble;
    }

    /// Opens a newly attached gamepad. One that SDL cannot open, or one past the [`MAX_PADS`]
    /// the host takes, is logged and left out, and the caller tells the user.
    pub fn added(&mut self, id: SDL_JoystickID) -> Result<(), NotOpened> {
        if self.pads.iter().any(|p| p.id == id) {
            return Ok(());
        }
        let used: Vec<u8> = self.pads.iter().map(|p| p.index).collect();
        let Some(index) = (0..MAX_PADS as u8).find(|i| !used.contains(i)) else {
            eprintln!("nightjar: controller {}: left out, {MAX_PADS} are already connected", id.0);
            return Err(NotOpened::Limit);
        };
        // SAFETY: opening a gamepad SDL reported.
        let handle = unsafe { SDL_OpenGamepad(id) };
        if handle.is_null() {
            let error = crate::sdl::error();
            eprintln!("nightjar: controller {}: could not be opened: {error}", id.0);
            return Err(NotOpened::Failed(error));
        }
        let name = unsafe {
            let name = SDL_GetGamepadName(handle);
            if name.is_null() { "Controller".into() } else { std::ffi::CStr::from_ptr(name).to_string_lossy().into_owned() }
        };
        let (vendor, product) = unsafe { (SDL_GetGamepadVendor(handle), SDL_GetGamepadProduct(handle)) };
        let key = if vendor != 0 || product != 0 { format!("usb:{vendor:04x}:{product:04x}") } else { format!("name:{name}") };
        let mapping = self.mappings.get(&key).cloned().unwrap_or_default();
        // SAFETY: the gamepad opened above; read-only queries.
        let has_touchpad_click = unsafe {
            matches!(SDL_GetGamepadType(handle), SDL_GAMEPAD_TYPE_PS3 | SDL_GAMEPAD_TYPE_PS4 | SDL_GAMEPAD_TYPE_PS5)
                || SDL_GamepadHasButton(handle, SDL_GAMEPAD_BUTTON_TOUCHPAD)
        };
        self.pads.push(Pad {
            audio: None,
            id,
            handle,
            index,
            key,
            name,
            mapping,
            raw_buttons: 0,
            raw_axes: [0; 6],
            mouse: crate::controller_mouse::Mouse::default(),
            touchpad: crate::controller_mouse::Touchpad::default(),
            buttons: 0,
            lt: 0,
            rt: 0,
            ls: (0, 0),
            rs: (0, 0),
            announced: false,
            has_touchpad_click,
            clickpad_swallowed: 0,
        });
        Ok(())
    }

    /// Closes a removed gamepad; returns its host number.
    pub fn removed(&mut self, id: SDL_JoystickID) -> Option<u8> {
        let position = self.pads.iter().position(|p| p.id == id)?;
        let pad = self.pads.remove(position);
        if let Some(audio) = &pad.audio { audio.reset(pad.handle); }
        drop(pad.audio);
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
            pad.mouse.release(queue);
            pad.touchpad.release(queue);
            pad.raw_buttons = 0;
            pad.raw_axes = [0; 6];
            pad.clickpad_swallowed = 0;
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
        let back_opens_menu = self.back_opens_menu;
        let quit = quit_mask(self.quit_combo);
        let mouse_emulation = self.mouse_emulation;
        let precision = self.mouse_fixed_step;
        let touchpad_as_mouse = self.touchpad_as_mouse;
        // SAFETY: union members read by event type.
        unsafe {
            let kind = SDL_EventType(event.r#type);
            match kind {
                SDL_EVENT_GAMEPAD_BUTTON_DOWN | SDL_EVENT_GAMEPAD_BUTTON_UP => {
                    let b = event.gbutton;
                    let pad = self.find(b.which)?;
                    if usize::from(b.button) >= BUTTON_MAP.len() { return None; }
                    if touchpad_as_mouse && i32::from(b.button) == SDL_GAMEPAD_BUTTON_TOUCHPAD.0 {
                        pad.touchpad.button(b.down, queue);
                        return None;
                    }
                    if back_opens_menu && i32::from(b.button) == SDL_GAMEPAD_BUTTON_BACK.0 {
                        return b.down.then_some(Action::Menu);
                    }
                    let flag = 1 << b.button;
                    if b.down {
                        pad.raw_buttons |= flag;
                    } else {
                        pad.raw_buttons &= !flag;
                    }
                    // Local escape chords use physical buttons even when remapped or disabled.
                    let menu = (1 << SDL_GAMEPAD_BUTTON_START.0) | (1 << SDL_GAMEPAD_BUTTON_BACK.0);
                    if b.down && (pad.raw_buttons == quit || pad.raw_buttons == menu) {
                        let action = if pad.raw_buttons == quit { Action::Quit } else { Action::Menu };
                        pad.raw_buttons = 0;
                        pad.raw_axes = [0; 6];
                        pad.mouse.release(queue);
                        pad.touchpad.release(queue);
                        pad.update(swap);
                        send_state(pad, mask, queue);
                        return Some(action);
                    }
                    pad.update(swap);
                    let action = pad.mouse.update(pad.report(), mouse_emulation, precision, std::time::Instant::now(), queue);
                    send_state(pad, mask, queue);
                    return action.map(Action::Mouse);
                }
                SDL_EVENT_GAMEPAD_AXIS_MOTION => {
                    let a = event.gaxis;
                    let pad = self.find(a.which)?;
                    *pad.raw_axes.get_mut(usize::from(a.axis))? = a.value;
                    pad.update(swap);
                    send_state(pad, mask, queue);
                }
                SDL_EVENT_GAMEPAD_TOUCHPAD_DOWN
                | SDL_EVENT_GAMEPAD_TOUCHPAD_MOTION
                | SDL_EVENT_GAMEPAD_TOUCHPAD_UP => {
                    let t = event.gtouchpad;
                    let pad = self.find(t.which)?;
                    if touchpad_as_mouse { pad.touchpad.touch(&t, queue); return None; }
                    // A pad pinned as Xbox told the host it has no touchpad.
                    if pad.mapping.drops_motion_and_touchpad() { return None; }
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
                    if pad.mapping.drops_motion_and_touchpad() { return None; }
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
        if self.disable_rumble { return; }
        if let Some(pad) = self.by_index(index) {
            if pad.audio.as_ref().is_some_and(|audio| audio.active()) { return; }
            // SAFETY: an open gamepad. A long duration; the host sends 0 to stop.
            unsafe { SDL_RumbleGamepad(pad.handle, low, high, 30_000) };
        }
    }

    pub fn rumble_triggers(&self, index: u16, left: u16, right: u16) {
        if self.disable_rumble { return; }
        if let Some(pad) = self.by_index(index) {
            // SAFETY: an open gamepad.
            unsafe { SDL_RumbleGamepadTriggers(pad.handle, left, right, 30_000) };
        }
    }

    pub fn set_led(&self, index: u16, r: u8, g: u8, b: u8) {
        if let Some(pad) = self.by_index(index) {
            // SAFETY: an open gamepad.
            unsafe { SDL_SetGamepadLED(pad.handle, r, g, b) };
            if let Some(audio) = &pad.audio { audio.send(pad.handle); }
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
        // A pad pinned as Xbox told the host it has no motion sensors.
        let on = rate_hz != 0 && !pad.mapping.drops_motion_and_touchpad();
        // SAFETY: an open gamepad.
        unsafe { SDL_SetGamepadSensorEnabled(pad.handle, sensor, on) };
    }

    /// Stops every motor and sensor (stream end).
    pub fn reset(&mut self) {
        for pad in &self.pads {
            self.adaptive_triggers(u16::from(pad.index), 0x0C, 0x05, 0x05, &[0; 10], &[0; 10]);
        }
        for pad in &mut self.pads {
            if let Some(audio) = pad.audio.take() { audio.reset(pad.handle); }
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
            pad.mouse = crate::controller_mouse::Mouse::default();
            pad.touchpad = crate::controller_mouse::Touchpad::default();
        }
    }

    pub fn close_all(&mut self) {
        for pad in self.pads.drain(..) {
            if let Some(audio) = &pad.audio { audio.reset(pad.handle); }
            drop(pad.audio);
            // SAFETY: gamepads we opened.
            unsafe { SDL_CloseGamepad(pad.handle) };
        }
    }

    pub fn prepare_audio(&mut self, enabled: bool) {
        for pad in &mut self.pads {
            if let Some(audio) = pad.audio.take() { audio.reset(pad.handle); }
        }
        if !enabled { return; }
        self.auto_audio();
    }

    pub fn auto_audio(&mut self) {
        let outputs = crate::controller_audio::outputs();
        let pads: Vec<_> = self.pads.iter().filter(|p| p.supports_audio()).map(|p| p.index).collect();
        if let ([index], [(device, _)]) = (pads.as_slice(), outputs.as_slice()) {
            if self.pads.iter().any(|p| p.index == *index && p.audio.is_none()) {
                if let Err(error) = self.set_audio(*index, Some(*device)) { eprintln!("nightjar: controller audio: {error}"); }
            }
            let inputs = crate::controller_audio::inputs();
            if let [(input, _)] = inputs.as_slice() {
                if let Some(audio) = self.pads.iter_mut().find(|p| p.index == *index).and_then(|p| p.audio.as_mut()) {
                    if audio.input.is_none() { audio.set_input(Some(*input)); }
                }
            }
        }
    }

    pub fn set_audio(&mut self, index: u8, device: Option<SDL_AudioDeviceID>) -> Result<(), String> {
        if device.is_some_and(|device| self.pads.iter().any(|p| p.index != index && p.audio.as_ref().is_some_and(|a| a.device == device))) {
            return Err("That output already belongs to another controller.".into());
        }
        let Some(pad) = self.pads.iter_mut().find(|p| p.index == index && p.supports_audio()) else { return Err("Connect a DualSense controller over USB.".into()); };
        if device.is_some_and(|id| !crate::controller_audio::outputs().iter().any(|(candidate, _)| *candidate == id)) {
            return Err("The controller's four-channel output is unavailable.".into());
        }
        if let Some(audio) = pad.audio.take() { audio.reset(pad.handle); }
        pad.announced = false;
        if let Some(device) = device {
            let audio = crate::controller_audio::Audio::open(device, pad.handle)?;
            audio.send(pad.handle);
            pad.audio = Some(audio);
        }
        Ok(())
    }

    pub fn set_audio_input(&mut self, index: u8, device: Option<SDL_AudioDeviceID>) -> Result<(), String> {
        if device.is_some_and(|device| self.pads.iter().any(|p| p.index != index && p.audio.as_ref().is_some_and(|a| a.input == Some(device)))) {
            return Err("That microphone already belongs to another controller.".into());
        }
        if device.is_some_and(|id| !crate::controller_audio::inputs().iter().any(|(candidate, _)| *candidate == id)) {
            return Err("The controller microphone is unavailable.".into());
        }
        if let Some(audio) = self.pads.iter_mut().find(|p| p.index == index).and_then(|p| p.audio.as_mut()) { audio.set_input(device); }
        Ok(())
    }

    pub fn removed_audio_device(&mut self, device: SDL_AudioDeviceID) {
        for pad in &mut self.pads {
            if pad.audio.as_mut().is_some_and(|audio| audio.removed(device)) {
                if let Some(audio) = pad.audio.take() { audio.reset(pad.handle); }
                pad.announced = false;
            }
        }
    }

    pub fn update_audio(&mut self) {
        for pad in &mut self.pads {
            if let Some(audio) = &mut pad.audio { audio.update(pad.handle); }
        }
    }

    pub fn pcm(&mut self, pcm: nightjar_core::control::haptics::Pcm, speaker: bool) {
        if !speaker && self.disable_rumble { return; }
        if let Some(pad) = self.pads.iter_mut().find(|p| u16::from(p.index) == pcm.controller) {
            if let Some(audio) = &mut pad.audio { audio.offer(pcm, speaker); audio.update(pad.handle); }
        }
    }

    pub fn audio_control(&mut self, payload: &[u8]) {
        if !crate::controller_audio::valid_control(payload) { return; }
        let index = u16::from_le_bytes([payload[2], payload[3]]);
        if let Some(pad) = self.pads.iter_mut().find(|p| u16::from(p.index) == index) {
            if let Some(audio) = &mut pad.audio { audio.control(payload, pad.handle); }
        }
    }

    pub fn update_mouse(&mut self, queue: &mut InputQueue) -> bool {
        let now = std::time::Instant::now();
        let mut active = false;
        for pad in &mut self.pads {
            active |= pad.mouse.active;
            pad.mouse.tick(pad.report(), self.analog_scrolling, self.mouse_fixed_step, now, queue);
        }
        active
    }

    /// Any controller drives the mouse.
    pub fn mouse_active(&self) -> bool {
        self.pads.iter().any(|pad| pad.mouse.active)
    }

    /// The stream menu's controller mouse switch, as in V+ PC: the first controller's mouse turns
    /// on, or every active one turns off. None without a controller.
    pub fn toggle_mouse(&mut self, queue: &mut InputQueue) -> Option<bool> {
        if self.mouse_active() {
            for pad in self.pads.iter_mut().filter(|pad| pad.mouse.active) { pad.mouse.toggle(queue); }
            return Some(false);
        }
        Some(self.pads.first_mut()?.mouse.toggle(queue))
    }

    pub fn release_mouse(&mut self, id: SDL_JoystickID, queue: &mut InputQueue) {
        if let Some(pad) = self.find(id) { pad.mouse.release(queue); pad.touchpad.release(queue); }
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
    let report = if pad.mouse.active { Report::default() } else { pad.report() };
    let _ = queue.send_controller(
        u16::from(pad.index),
        mask,
        report.buttons,
        report.lt,
        report.rt,
        report.ls,
        report.rs,
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
        let kind = pad.mapping.arrival_type(kind);
        let mut supported = 0u32;
        for (i, flag) in BUTTON_MAP.iter().enumerate() {
            if SDL_GamepadHasButton(pad.handle, SDL_GamepadButton(i as i32)) {
                supported |= pad.mapping.buttons[i].unwrap_or(*flag);
            }
        }
        let props = SDL_GetGamepadProperties(pad.handle);
        let has = |name: *const std::ffi::c_char| SDL_GetBooleanProperty(props, name, false);
        let mut caps = 0u16;
        if pad.audio.is_some() { caps |= nightjar_core::control::haptics::CAPABILITY; }
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
        (kind, supported, pad.mapping.arrival_capabilities(caps))
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
        let _sdl = crate::sdl::TEST_LOCK.lock().unwrap();
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
            pads.added(id).unwrap();
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

            pads.pads[0].mapping.buttons[0] = Some(button::X);
            pads.pads[0].mapping.buttons[1] = Some(button::X);
            SDL_SetJoystickVirtualButton(joystick, SDL_GAMEPAD_BUTTON_EAST.0, true);
            SDL_UpdateJoysticks();
            while SDL_PollEvent(&mut event) { pads.handle(&event, &mut queue); }
            assert_eq!(pads.pads[0].buttons, button::X);
            SDL_SetJoystickVirtualButton(joystick, SDL_GAMEPAD_BUTTON_SOUTH.0, false);
            SDL_UpdateJoysticks();
            while SDL_PollEvent(&mut event) { pads.handle(&event, &mut queue); }
            assert_eq!(pads.pads[0].buttons, button::X, "a second physical button still holds the target");
            SDL_SetJoystickVirtualButton(joystick, SDL_GAMEPAD_BUTTON_EAST.0, false);
            SDL_UpdateJoysticks();
            while SDL_PollEvent(&mut event) { pads.handle(&event, &mut queue); }
            assert_eq!(pads.pads[0].buttons, 0);

            pads.rumble(0, 0x1234, 0x5678);
            assert_eq!(RUMBLE.load(std::sync::atomic::Ordering::SeqCst), 0x1234_5678);
            pads.disable_rumble = true;
            pads.rumble(0, 0x4321, 0x8765);
            assert_eq!(RUMBLE.load(std::sync::atomic::Ordering::SeqCst), 0x1234_5678);

            pads.release(&mut queue);
            assert_eq!(pads.pads[0].buttons, 0);
            assert_eq!(pads.pads[0].ls, (0, 0));
            pads.pads[0].mapping.buttons[SDL_GAMEPAD_BUTTON_START.0 as usize] = Some(0);
            pads.pads[0].mapping.buttons[SDL_GAMEPAD_BUTTON_BACK.0 as usize] = Some(0);
            SDL_SetJoystickVirtualButton(joystick, SDL_GAMEPAD_BUTTON_START.0, true);
            SDL_SetJoystickVirtualButton(joystick, SDL_GAMEPAD_BUTTON_BACK.0, true);
            SDL_UpdateJoysticks();
            let mut menu = false;
            while SDL_PollEvent(&mut event) {
                menu |= pads.handle(&event, &mut queue) == Some(Action::Menu);
            }
            assert!(menu);
            assert_eq!(pads.pads[0].buttons, 0);
            pads.back_opens_menu = true;
            let event = SDL_Event { gbutton: SDL_GamepadButtonEvent { r#type: SDL_EVENT_GAMEPAD_BUTTON_DOWN,
                which: id, button: SDL_GAMEPAD_BUTTON_BACK.0 as u8, down: true, ..Default::default() } };
            assert_eq!(pads.handle(&event, &mut queue), Some(Action::Menu));
            assert_eq!(pads.pads[0].buttons, 0);

            pads.close_all();
            SDL_DetachVirtualJoystick(id);
            SDL_QuitSubSystem(SDL_INIT_GAMEPAD);
        }
    }

    /// A chord the host may claim (its menu opens on L3+R3 held by default, and the chord can
    /// change) is ordinary input here: both stick clicks reach the host for as long as they are
    /// held, and no Nightjar action comes of it.
    #[test]
    fn stick_clicks_held_together_reach_the_host_and_are_no_action() {
        let _sdl = crate::sdl::TEST_LOCK.lock().unwrap();
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
            let id = SDL_AttachVirtualJoystick(&desc);
            assert!(id.0 != 0, "{}", crate::sdl::error());
            let mut pads = Gamepads::default();
            pads.added(id).unwrap();
            let mut queue = InputQueue::new(&HostCapabilities {
                app_version: [7, 1, 431, -1],
                feature_flags: 0x03,
                ..Default::default()
            });
            pads.announce_all(&mut queue);
            let joystick = SDL_GetGamepadJoystick(pads.pads[0].handle);
            let mut event: SDL_Event = std::mem::zeroed();
            for stick in [SDL_GAMEPAD_BUTTON_LEFT_STICK, SDL_GAMEPAD_BUTTON_RIGHT_STICK] {
                SDL_SetJoystickVirtualButton(joystick, stick.0, true);
                SDL_UpdateJoysticks();
                let queued = queue.len();
                while SDL_PollEvent(&mut event) {
                    assert_eq!(pads.handle(&event, &mut queue), None);
                }
                assert!(queue.len() > queued, "each click is sent");
            }
            assert_eq!(pads.pads[0].buttons, button::LS_CLK | button::RS_CLK);
            // Held for two seconds: nothing local fires, and both stay down for the host.
            std::thread::sleep(std::time::Duration::from_secs(2));
            assert!(!pads.update_mouse(&mut queue));
            assert_eq!(pads.pads[0].buttons, button::LS_CLK | button::RS_CLK);
            for stick in [SDL_GAMEPAD_BUTTON_LEFT_STICK, SDL_GAMEPAD_BUTTON_RIGHT_STICK] {
                SDL_SetJoystickVirtualButton(joystick, stick.0, false);
            }
            SDL_UpdateJoysticks();
            while SDL_PollEvent(&mut event) {
                assert_eq!(pads.handle(&event, &mut queue), None);
            }
            assert_eq!(pads.pads[0].buttons, 0);

            pads.close_all();
            SDL_DetachVirtualJoystick(id);
            SDL_QuitSubSystem(SDL_INIT_GAMEPAD);
        }
    }

    /// Every quit combo in the settings leaves the stream when its last button goes down, and only
    /// the chosen one does; the menu chord and the stream menu's mouse switch keep working.
    #[test]
    fn the_chosen_quit_combo_leaves_the_stream() {
        use crate::config::GamepadQuitCombo;
        let _sdl = crate::sdl::TEST_LOCK.lock().unwrap();
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
            let id = SDL_AttachVirtualJoystick(&desc);
            assert!(id.0 != 0, "{}", crate::sdl::error());
            let mut pads = Gamepads::default();
            pads.added(id).unwrap();
            let mut queue = InputQueue::new(&HostCapabilities { app_version: [7, 1, 431, -1], feature_flags: 0x03, ..Default::default() });
            pads.announce_all(&mut queue);
            let press = |pads: &mut Gamepads, queue: &mut InputQueue, button: i32, down: bool| {
                let event = SDL_Event { gbutton: SDL_GamepadButtonEvent { r#type: if down { SDL_EVENT_GAMEPAD_BUTTON_DOWN } else { SDL_EVENT_GAMEPAD_BUTTON_UP },
                    which: id, button: button as u8, down, ..Default::default() } };
                pads.handle(&event, queue)
            };
            // Shoulders first so Start+Select alone (the menu chord) is never all that is held.
            let order = |mask: u32| {
                let mut buttons: Vec<i32> = (0..32).filter(|b| mask & 1 << b != 0).collect();
                buttons.sort_by_key(|b| ![SDL_GAMEPAD_BUTTON_LEFT_SHOULDER.0, SDL_GAMEPAD_BUTTON_RIGHT_SHOULDER.0].contains(b));
                buttons
            };
            for chosen in GamepadQuitCombo::ALL {
                pads.quit_combo = chosen;
                for combo in GamepadQuitCombo::ALL {
                    let buttons = order(quit_mask(combo));
                    let mut actions = Vec::new();
                    for button in &buttons { actions.push(press(&mut pads, &mut queue, *button, true)); }
                    for button in &buttons { press(&mut pads, &mut queue, *button, false); }
                    let quit = actions.last() == Some(&Some(Action::Quit));
                    assert_eq!(quit, combo == chosen, "{combo:?} held with {chosen:?} chosen");
                    assert!(actions[..actions.len() - 1].iter().all(Option::is_none), "{combo:?}: {actions:?}");
                    assert_eq!(pads.pads[0].raw_buttons, 0);
                }
                assert_eq!(press(&mut pads, &mut queue, SDL_GAMEPAD_BUTTON_START.0, true), None);
                assert_eq!(press(&mut pads, &mut queue, SDL_GAMEPAD_BUTTON_BACK.0, true), Some(Action::Menu), "{chosen:?}");
                press(&mut pads, &mut queue, SDL_GAMEPAD_BUTTON_START.0, false);
                press(&mut pads, &mut queue, SDL_GAMEPAD_BUTTON_BACK.0, false);
            }
            assert_eq!(quit_mask(GamepadQuitCombo::StartSelectLbRb), 1 << SDL_GAMEPAD_BUTTON_START.0 | 1 << SDL_GAMEPAD_BUTTON_BACK.0
                | 1 << SDL_GAMEPAD_BUTTON_LEFT_SHOULDER.0 | 1 << SDL_GAMEPAD_BUTTON_RIGHT_SHOULDER.0, "the default stays moonlight-qt's");

            assert!(!pads.mouse_active());
            assert_eq!(pads.toggle_mouse(&mut queue), Some(true));
            assert!(pads.mouse_active());
            assert_eq!(pads.toggle_mouse(&mut queue), Some(false));
            assert!(!pads.mouse_active());

            pads.close_all();
            SDL_DetachVirtualJoystick(id);
            SDL_QuitSubSystem(SDL_INIT_GAMEPAD);
            assert_eq!(Gamepads::default().toggle_mouse(&mut queue), None, "no controller, no switch");
        }
    }

    /// A controller left out says why: SDL's error when it cannot be opened, the limit past
    /// the host's 16.
    #[test]
    fn a_controller_that_is_left_out_says_why() {
        let _sdl = crate::sdl::TEST_LOCK.lock().unwrap();
        // SAFETY: SDL calls on this test's thread; the descriptor outlives the attaches.
        unsafe {
            assert!(SDL_Init(SDL_INIT_GAMEPAD), "{}", crate::sdl::error());
            let mut pads = Gamepads::default();
            match pads.added(SDL_JoystickID(0x7fff_fff0)) {
                Err(NotOpened::Failed(error)) => assert!(!error.is_empty()),
                other => panic!("{other:?}"),
            }
            assert!(pads.pads.is_empty());

            let mut desc: SDL_VirtualJoystickDesc = std::mem::zeroed();
            desc.version = std::mem::size_of::<SDL_VirtualJoystickDesc>() as u32;
            desc.r#type = SDL_JOYSTICK_TYPE_GAMEPAD.0 as u16;
            desc.naxes = SDL_GAMEPAD_AXIS_COUNT.0 as u16;
            desc.nbuttons = SDL_GAMEPAD_BUTTON_COUNT.0 as u16;
            desc.button_mask = (1u32 << SDL_GAMEPAD_BUTTON_COUNT.0) - 1;
            desc.axis_mask = (1u32 << SDL_GAMEPAD_AXIS_COUNT.0) - 1;
            desc.name = c"Nightjar test pad".as_ptr();
            let ids: Vec<_> = (0..=MAX_PADS).map(|_| SDL_AttachVirtualJoystick(&desc)).collect();
            assert!(ids.iter().all(|id| id.0 != 0), "{}", crate::sdl::error());
            for id in &ids[..MAX_PADS] {
                pads.added(*id).unwrap();
            }
            assert_eq!(pads.added(ids[MAX_PADS]), Err(NotOpened::Limit));
            assert_eq!(pads.pads.len(), MAX_PADS);
            assert!(NotOpened::Limit.message().contains("16"));

            pads.close_all();
            for id in ids {
                SDL_DetachVirtualJoystick(id);
            }
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
    fn mappings_apply_once_preserve_explicit_bindings_and_scale_dead_zones() {
        let mut mapping = Mapping::default();
        assert_eq!(mapping.map_buttons(1, true), button::B);
        mapping.buttons[0] = Some(button::X);
        mapping.buttons[2] = Some(button::Y);
        assert_eq!(mapping.map_buttons(1, true), button::X);
        assert_eq!(mapping.map_buttons(1 | 4, false), button::X | button::Y);
        mapping.axes = [-4, 3, 0, -1, 6, 0];
        let report = mapping.apply(0, [-32768, 1000, -2000, 32767, 0, 32767], false);
        assert_eq!(report.ls, (-32767, 2000));
        assert_eq!(report.rs, (0, -32767));
        assert_eq!((report.lt, report.rt), (255, 0));
        assert_eq!(deadzone((3000, 3000), 20), (0, 0));
        assert_eq!(deadzone((32767, 0), 20), (32767, 0));
        let middle = deadzone((20000, 0), 20);
        assert!((16000..18000).contains(&middle.0));
        assert_eq!(middle.1, 0);
        assert!(mapping.valid());
        mapping.axes[4] = -6;
        assert!(!mapping.valid());
        mapping.axes[4] = 6;
        mapping.buttons[0] = Some(button::A | button::B);
        assert!(!mapping.valid());
    }

    #[test]
    fn a_pinned_type_replaces_the_detected_one_and_automatic_keeps_it() {
        let mut mapping = Mapping::default();
        assert_eq!(mapping.connect_as, CTYPE_UNKNOWN);
        for detected in [CTYPE_UNKNOWN, CTYPE_XBOX, CTYPE_PS, CTYPE_NINTENDO] {
            assert_eq!(mapping.arrival_type(detected), detected);
        }
        // #53: a pad SDL reports as PlayStation can be told to the host as Xbox.
        mapping.connect_as = CTYPE_XBOX;
        assert_eq!(mapping.arrival_type(CTYPE_PS), CTYPE_XBOX);
        assert_eq!(mapping.arrival_type(CTYPE_UNKNOWN), CTYPE_XBOX);
        mapping.connect_as = CTYPE_PS;
        assert_eq!(mapping.arrival_type(CTYPE_XBOX), CTYPE_PS);
        mapping.connect_as = CTYPE_NINTENDO;
        assert_eq!(mapping.arrival_type(CTYPE_XBOX), CTYPE_NINTENDO);
        assert!(mapping.valid());
        // A value this version does not know is Automatic and keeps the mapping valid.
        mapping.connect_as = 9;
        assert_eq!(mapping.arrival_type(CTYPE_PS), CTYPE_PS);
        assert!(mapping.valid());
        assert_eq!(CONNECT_AS.map(|(kind, _)| kind), [0, 1, 2, 3]);
    }

    #[test]
    fn an_xbox_pin_arrives_without_motion_or_touchpad() {
        let all = capability::ANALOG_TRIGGERS | capability::RUMBLE | capability::TOUCHPAD
            | capability::DUAL_TOUCHPAD | capability::ACCEL | capability::GYRO | capability::BATTERY_STATE;
        let mut mapping = Mapping::default();
        // Automatic and the other types keep what the pad has.
        assert_eq!(mapping.arrival_capabilities(all), all);
        for kind in [CTYPE_PS, CTYPE_NINTENDO] {
            mapping.connect_as = kind;
            assert_eq!(mapping.arrival_capabilities(all), all);
            assert!(!mapping.drops_motion_and_touchpad());
        }
        // Rubylight turns an Xbox pad with motion or a touchpad into a DualSense.
        mapping.connect_as = CTYPE_XBOX;
        assert!(mapping.drops_motion_and_touchpad());
        assert_eq!(mapping.arrival_capabilities(all),
            capability::ANALOG_TRIGGERS | capability::RUMBLE | capability::BATTERY_STATE);
    }

    #[test]
    fn select_lb_sends_the_first_button_then_clicks_the_touchpad_of_a_pad_pinned_as_playstation() {
        let mut mapping = Mapping { connect_as: CTYPE_PS, ..Default::default() };
        assert!(mapping.emulates_clickpad(false));
        assert!(!mapping.emulates_clickpad(true), "a pad with a touchpad click keeps it");
        mapping.buttons[0] = Some(button::TOUCHPAD);
        assert!(!mapping.emulates_clickpad(false), "a button mapped to the touchpad replaces the combo");
        assert!(!Mapping::default().emulates_clickpad(false));
        assert!(!Mapping { connect_as: CTYPE_XBOX, ..Default::default() }.emulates_clickpad(false));

        let (back, lb) = (button::BACK, button::LB);
        let mut swallowed = 0;
        // The first button of the combo goes through: holding it back would add latency.
        assert_eq!(clickpad(back, &mut swallowed), back);
        assert_eq!(clickpad(back | lb, &mut swallowed), button::TOUCHPAD);
        assert_eq!(clickpad(back | lb | button::A, &mut swallowed), button::TOUCHPAD | button::A);
        // Letting go of LB lifts the click; Back stays silent until it is let go too.
        assert_eq!(clickpad(back, &mut swallowed), 0);
        assert_eq!(clickpad(back | lb, &mut swallowed), lb);
        assert_eq!(clickpad(0, &mut swallowed), 0);
        assert_eq!(swallowed, 0);
        // Select+LB as part of a larger chord is left alone.
        assert_eq!(clickpad(back | lb | button::PLAY, &mut swallowed), back | lb | button::PLAY);
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
