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
use nightjar_policy::controllers::Controllers;
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
    /// Percent of trigger travel ignored; 0 (the default) sends triggers raw.
    pub trigger_deadzone: u8,
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
        Self { name: String::new(), buttons: [None; 21], axes: [1, 2, 3, 4, 5, 6], deadzone: 0, trigger_deadzone: 0, connect_as: CTYPE_UNKNOWN }
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

/// Stick and trigger dead zones, in percent of their travel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeadZones {
    pub stick: u8,
    pub trigger: u8,
}

/// What every controller's state goes through on its way to the PC besides its own mapping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PadSettings {
    /// Nintendo layout: A/B and X/Y swapped.
    pub swap: bool,
    /// "Back button as Guide": the physical Back (Select, View) reaches the PC as Guide.
    pub back_as_guide: bool,
    /// "Stick dead zone" and "Trigger dead zone", for a mapping that sets none of its own.
    pub dead_zones: DeadZones,
}

impl From<&crate::config::Settings> for PadSettings {
    fn from(settings: &crate::config::Settings) -> Self {
        Self {
            swap: settings.swap_face_buttons,
            // "Back opens the stream menu" keeps Back for Nightjar, which never sends it, as on Android.
            back_as_guide: settings.back_as_guide && !settings.back_opens_menu,
            dead_zones: DeadZones {
                stick: settings.deadzone.min(u32::from(MAX_STICK_DEADZONE)) as u8,
                trigger: settings.trigger_deadzone.min(u32::from(MAX_TRIGGER_DEADZONE)) as u8,
            },
        }
    }
}

/// The face button swap alone: a mapping as it is, without the settings' dead zones or Back as Guide.
impl From<bool> for PadSettings {
    fn from(swap: bool) -> Self {
        Self { swap, ..Self::default() }
    }
}

/// "Controllers" One player ([`Controllers`]): every controller is player 1, and the PC keeps
/// player 1 while none is connected (Android's `persistGamepadsAfterDisconnect`).
pub fn one_player(settings: &crate::config::Settings) -> bool {
    Controllers::from_saved(settings.controllers, settings.multi_controller) == Controllers::OnePlayer
}

/// The gamepad number a controller in host slot `index` reaches the PC as: its own, or 0 (player 1)
/// for every controller with One player.
fn sent_index(one_player: bool, index: u8) -> u8 {
    let players = if one_player { Controllers::OnePlayer } else { Controllers::PlayerEach };
    players.sent_index(index).unwrap_or(index)
}

/// "Back button as Guide": the physical Back as Guide, before the mapping, which then sends what it
/// sends for Guide (Android remaps Back before its button map too).
fn back_as_guide(raw: u32, on: bool) -> u32 {
    const BACK: u32 = 1 << SDL_GAMEPAD_BUTTON_BACK.0;
    const GUIDE: u32 = 1 << SDL_GAMEPAD_BUTTON_GUIDE.0;
    if on && raw & BACK != 0 { (raw & !BACK) | GUIDE } else { raw }
}

impl Mapping {
    pub fn valid(&self) -> bool {
        self.deadzone <= MAX_STICK_DEADZONE
            && self.trigger_deadzone <= MAX_TRIGGER_DEADZONE
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

    /// The dead zones this mapping streams with: its own, and `defaults` (the settings') where it
    /// sets none (0).
    pub fn dead_zones(&self, defaults: DeadZones) -> DeadZones {
        DeadZones {
            stick: if self.deadzone != 0 { self.deadzone } else { defaults.stick },
            trigger: if self.trigger_deadzone != 0 { self.trigger_deadzone } else { defaults.trigger },
        }
    }

    /// The physical `buttons` (bits of `1 << SDL_GamepadButton`) and `axes` as the PC gets them.
    pub fn apply(&self, buttons: u32, axes: [i16; 6], settings: PadSettings) -> Report {
        let mut mapped = [0; 6];
        for (value, source) in mapped.iter_mut().zip(self.axes) {
            if source != 0 {
                *value = axes[source.unsigned_abs() as usize - 1];
                if source < 0 { *value = invert(*value); }
            }
        }
        let zones = self.dead_zones(settings.dead_zones);
        let trigger = |v: i16| trigger(v, zones.trigger);
        Report {
            buttons: self.map_buttons(back_as_guide(buttons, settings.back_as_guide), settings.swap),
            lt: trigger(mapped[4]), rt: trigger(mapped[5]),
            ls: deadzone((mapped[0], invert(mapped[1])), zones.stick),
            rs: deadzone((mapped[2], invert(mapped[3])), zones.stick),
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

pub const MAX_TRIGGER_DEADZONE: u8 = 30;
/// The largest stick dead zone in percent, a mapping's or the settings'.
const MAX_STICK_DEADZONE: u8 = 50;

/// A trigger as 0-255: travel inside the dead zone reads as released, the rest scales smoothly to
/// full, like the stick dead zone.
fn trigger(value: i16, percent: u8) -> u8 {
    let value = i32::from(value.max(0));
    let edge = i32::from(percent) * 32767 / 100;
    if value <= edge { return 0; }
    ((value - edge) * 255 / (32767 - edge)) as u8
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
    /// Host controller slot: which controller this is, in the stream menu and the buttons page.
    pub index: u8,
    /// The gamepad number the PC knows it as: `index`, or 0 for every controller with
    /// "Controllers" One player ([`sent_index`]).
    sent_as: u8,
    pub key: String,
    pub name: String,
    pub mapping: Mapping,
    raw_buttons: u32,
    raw_axes: [i16; 6],
    /// USB vendor and product SDL reports (0 when it has none), sent as the controller's
    /// identity before its arrival.
    usb_id: (u16, u16),
    /// The Switch 2 controllers' C button (SDL's Misc 2) is down. It sits outside the button
    /// mapping and goes as `button::C`, which the queue clears for a host without it.
    c_down: bool,
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
    /// With Back opening the stream controls: Back is held while other buttons of a quit combo
    /// with Select are, so it counts toward that combo and opens the controls on release instead.
    back_waiting: bool,
    /// Holding Guide opens the stream menu: where this pad's Guide press is.
    guide: Guide,
    /// The physical button (as `1 << SDL_GamepadButton`) `guide` follows: Guide, or Back with
    /// "Back button as Guide".
    guide_bit: u32,
}

/// A Guide press while holding it opens the stream menu. The press is kept from the PC until it is
/// known to be short (then the PC gets a whole tap), part of a chord, or long (then it is the menu's).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Guide {
    #[default]
    Up,
    /// Down since then, not sent yet.
    Held(std::time::Instant),
    /// A short press, sent to the PC as down at this time; it goes up after [`GUIDE_TAP`].
    Tapped(std::time::Instant),
    /// Down with another button: the PC has it like any button.
    Passed,
    /// Held long enough: it opened the menu, and its release is the menu's too.
    Opened,
}

/// How long the PC sees a short Guide press held: long enough for games and the system to notice it
/// (moonlight-android's minimum is 25 ms).
const GUIDE_TAP: std::time::Duration = std::time::Duration::from_millis(50);

impl Pad {
    /// The physical state through `mapping` and `settings` (the stream's [`PadSettings`], from the
    /// settings, or the face button swap alone), for the buttons page's test view.
    pub fn preview(&self, mapping: &Mapping, settings: impl Into<PadSettings>) -> Report {
        let settings = settings.into();
        // Read physical state so the test view works without sending input to a PC.
        unsafe {
            let buttons = (0..21).filter(|i| SDL_GetGamepadButton(self.handle, SDL_GamepadButton(*i)))
                .fold(0, |bits, i| bits | (1 << i));
            let axes = std::array::from_fn(|i| SDL_GetGamepadAxis(self.handle, SDL_GamepadAxis(i as i32)));
            mapping.apply(buttons, axes, settings)
        }
    }

    /// The gamepad number the PC knows this controller as (`index`, or 0 with One player).
    pub fn sent_as(&self) -> u8 {
        self.sent_as
    }

    fn update(&mut self, settings: PadSettings) {
        let state = self.mapping.apply(self.raw_buttons, self.raw_axes, settings);
        self.buttons = if self.mapping.emulates_clickpad(self.has_touchpad_click) {
            clickpad(state.buttons, &mut self.clickpad_swallowed)
        } else {
            state.buttons
        } | if self.c_down { button::C } else { 0 };
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
    menu_buttons: nightjar_policy::stream_menu::Buttons,
    mouse_emulation: bool,
    analog_scrolling: crate::config::AnalogScrolling,
    mouse_fixed_step: bool,
    touchpad_as_mouse: bool,
    disable_rumble: bool,
    /// How long Guide is held to open the stream menu; None: Guide always goes to the PC.
    guide_hold: Option<std::time::Duration>,
    /// "Back button as Guide" (never with `back_opens_menu`, [`PadSettings`]).
    back_as_guide: bool,
    /// The settings' dead zones, for a mapping without its own.
    dead_zones: DeadZones,
    /// "Controllers" One player: every controller is sent as gamepad 0 ([`one_player`]).
    one_player: bool,
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
        self.menu_buttons.clone_from(&settings.stream_menu_buttons);
        self.mouse_emulation = settings.controller_mouse;
        self.analog_scrolling = settings.analog_scrolling;
        self.mouse_fixed_step = settings.mouse_fixed_step;
        self.touchpad_as_mouse = settings.gamepad_touchpad_as_mouse;
        self.disable_rumble = settings.disable_rumble;
        let pad_settings = PadSettings::from(settings);
        (self.back_as_guide, self.dead_zones) = (pad_settings.back_as_guide, pad_settings.dead_zones);
        self.one_player = one_player(settings);
        for pad in &mut self.pads {
            pad.sent_as = sent_index(self.one_player, pad.index);
        }
        self.guide_hold = (settings.guide_hold_ms > 0).then(|| std::time::Duration::from_millis(u64::from(settings.guide_hold_ms)));
        if self.guide_hold.is_none() {
            // Turned off mid-press: the held Guide is forgotten (the PC never had it), a tap still goes up on the tick.
            for pad in &mut self.pads {
                if matches!(pad.guide, Guide::Held(_) | Guide::Opened) { pad.guide = Guide::Up; }
            }
        }
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
        let sdl_name = unsafe {
            let name = SDL_GetGamepadName(handle);
            (!name.is_null()).then(|| std::ffi::CStr::from_ptr(name).to_string_lossy().into_owned())
        };
        let (vendor, product) = unsafe { (SDL_GetGamepadVendor(handle), SDL_GetGamepadProduct(handle)) };
        // A nameless pad's mapping is keyed in English, so it does not follow the language.
        const UNNAMED: &str = en("Controller");
        let key = if vendor != 0 || product != 0 {
            format!("usb:{vendor:04x}:{product:04x}")
        } else {
            format!("name:{}", sdl_name.as_deref().unwrap_or(UNNAMED))
        };
        let name = sdl_name.unwrap_or_else(|| crate::strings::tr(UNNAMED).to_owned());
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
            sent_as: sent_index(self.one_player, index),
            key,
            name,
            mapping,
            raw_buttons: 0,
            raw_axes: [0; 6],
            usb_id: (vendor, product),
            c_down: false,
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
            back_waiting: false,
            guide: Guide::Up,
            guide_bit: 1 << SDL_GAMEPAD_BUTTON_GUIDE.0,
        });
        Ok(())
    }

    /// Closes a removed gamepad; returns the gamepad number the PC knew it as.
    pub fn removed(&mut self, id: SDL_JoystickID) -> Option<u8> {
        let position = self.pads.iter().position(|p| p.id == id)?;
        let pad = self.pads.remove(position);
        if let Some(audio) = &pad.audio { audio.reset(pad.handle); }
        drop(pad.audio);
        // SAFETY: a gamepad we opened.
        unsafe { SDL_CloseGamepad(pad.handle) };
        Some(pad.sent_as)
    }

    /// The gamepads the PC has during the stream: player 1 alone with "Controllers" One player
    /// (also without a controller, as Android keeps it for games that miss one added later), else
    /// one for each controller.
    pub fn active_mask(&self) -> u16 {
        if self.one_player { 1 } else { self.pads.iter().fold(0, |m, p| m | (1 << p.sent_as)) }
    }

    /// The gamepad mask a launch with `settings` tells the PC: none with "Controllers" Off, player 1
    /// with One player, else one for each controller connected now.
    pub fn launch_mask(&self, settings: &crate::config::Settings) -> u32 {
        match Controllers::from_saved(settings.controllers, settings.multi_controller) {
            Controllers::Off => 0,
            Controllers::OnePlayer => 1,
            Controllers::PlayerEach => self.pads.iter().fold(0, |m, p| m | (1 << p.index)),
        }
    }

    /// Rubylight's prepared controller list for a launch with `settings`: empty with "Controllers"
    /// Off, with One player the first controller as player 1 alone.
    pub fn launch_controllers(&self, settings: &crate::config::Settings) -> String {
        let players = Controllers::from_saved(settings.controllers, settings.multi_controller);
        let mut listed = 0u16;
        self.pads.iter().filter_map(|pad| {
            let sent = players.sent_index(pad.index)?;
            if listed & (1 << sent) != 0 {
                return None;
            }
            listed |= 1 << sent;
            let (kind, buttons, mut caps) = describe(pad);
            if !settings.controller_motion {
                caps &= !(capability::ACCEL | capability::GYRO);
            }
            let kind = if kind == CTYPE_UNKNOWN { CTYPE_XBOX } else { kind };
            Some(format!("{sent}:{kind}:{caps}:{buttons}"))
        }).collect::<Vec<_>>().join(",")
    }

    /// The controllers the PC knows as gamepad `index`: one, or every one with One player.
    fn sent_as(&self, index: u16) -> impl Iterator<Item = &Pad> {
        self.pads.iter().filter(move |pad| u16::from(pad.sent_as) == index)
    }

    /// What each controller's state goes through on its way to the PC.
    fn pad_settings(&self) -> PadSettings {
        PadSettings {
            swap: self.swap_face_buttons,
            back_as_guide: self.back_as_guide && !self.back_opens_menu,
            dead_zones: self.dead_zones,
        }
    }

    /// The type of this gamepad if it is a real pad, with a stick: SDL also calls some other devices
    /// gamepads.
    pub fn real_pad_type(&self, id: SDL_JoystickID) -> Option<SDL_GamepadType> {
        let pad = self.pads.iter().find(|p| p.id == id)?;
        // SAFETY: the handle lives as long as its Pad.
        unsafe {
            let stick = SDL_GamepadHasAxis(pad.handle, SDL_GAMEPAD_AXIS_LEFTX) && SDL_GamepadHasAxis(pad.handle, SDL_GAMEPAD_AXIS_LEFTY);
            stick.then(|| SDL_GetGamepadType(pad.handle))
        }
    }

    pub fn find(&mut self, id: SDL_JoystickID) -> Option<&mut Pad> {
        self.pads.iter_mut().find(|p| p.id == id)
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

    /// The host forgets gamepad `index` ([`Self::removed`]): a neutral state with it out of the
    /// mask. With One player another controller still sent as it sends its own state instead, and
    /// player 1 stays in the mask.
    pub fn send_removed(&self, index: u8, queue: &mut InputQueue) {
        let mask = self.active_mask();
        if let Some(pad) = self.sent_as(u16::from(index)).find(|pad| pad.announced) {
            send_state(pad, mask, queue);
            return;
        }
        let _ = queue.send_controller(u16::from(index), mask, 0, 0, 0, (0, 0), (0, 0));
    }

    pub fn release(&mut self, queue: &mut InputQueue) {
        let mask = self.active_mask();
        for pad in &mut self.pads {
            pad.mouse.release(queue);
            pad.touchpad.release(queue);
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

    /// Applies an SDL gamepad event to the host's controller state. Returns an action for
    /// the special combos.
    pub fn handle(&mut self, event: &SDL_Event, queue: &mut InputQueue) -> Option<Action> {
        let pad_settings = self.pad_settings();
        let motion = self.motion;
        let mask = self.active_mask();
        let back_opens_menu = self.back_opens_menu;
        let guide_hold = self.guide_hold;
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
                    if i32::from(b.button) == SDL_GAMEPAD_BUTTON_MISC2.0 {
                        // The Switch 2 controllers' C button: not mapped, sent as itself.
                        if pad.c_down != b.down {
                            pad.c_down = b.down;
                            pad.update(pad_settings);
                            send_state(pad, mask, queue);
                        }
                        return None;
                    }
                    if usize::from(b.button) >= BUTTON_MAP.len() { return None; }
                    if touchpad_as_mouse && i32::from(b.button) == SDL_GAMEPAD_BUTTON_TOUCHPAD.0 {
                        pad.touchpad.button(b.down, queue);
                        return None;
                    }
                    let flag = 1 << b.button;
                    // With "Back button as Guide" Back is the PC's Guide, held for the menu like
                    // Guide (as on Android); its bit stays Back's for the local chords below.
                    let guide = i32::from(b.button) == SDL_GAMEPAD_BUTTON_GUIDE.0
                        || (pad_settings.back_as_guide && i32::from(b.button) == SDL_GAMEPAD_BUTTON_BACK.0);
                    if guide_hold.is_some() && guide {
                        let now = std::time::Instant::now();
                        match (b.down, pad.guide) {
                            (true, held) => {
                                // A tap still going out ends first.
                                if matches!(held, Guide::Tapped(_)) {
                                    pad.raw_buttons &= !pad.guide_bit;
                                    pad.update(pad_settings);
                                    send_state(pad, mask, queue);
                                }
                                if pad.raw_buttons != 0 {
                                    // Guide joining held buttons is a chord (a quit combo with Guide): it goes through.
                                    pad.guide = Guide::Passed;
                                } else {
                                    pad.guide = Guide::Held(now);
                                    pad.guide_bit = flag;
                                    return None;
                                }
                            }
                            (false, Guide::Held(_)) => {
                                // Let go before the menu: the PC gets the whole press, and `guide_tick` its release.
                                pad.raw_buttons |= flag;
                                pad.guide_bit = flag;
                                pad.guide = Guide::Tapped(now);
                                pad.update(pad_settings);
                                send_state(pad, mask, queue);
                                return None;
                            }
                            (false, Guide::Opened | Guide::Tapped(_)) => {
                                if pad.guide == Guide::Opened { pad.guide = Guide::Up; }
                                return None;
                            }
                            // Passed with a chord: released below like any button.
                            (false, Guide::Up | Guide::Passed) => pad.guide = Guide::Up,
                        }
                    } else if b.down && matches!(pad.guide, Guide::Held(_)) {
                        // Another button while Guide is held: a chord for the PC (Steam's Guide chords), Guide first.
                        pad.raw_buttons |= pad.guide_bit;
                        pad.guide = Guide::Passed;
                    }
                    // Local escape chords use physical buttons even when remapped or disabled.
                    let menu = crate::shortcuts::stream_controls_mask();
                    if back_opens_menu && i32::from(b.button) == SDL_GAMEPAD_BUTTON_BACK.0 {
                        // Back is Nightjar's and never reaches the PC. Pressed while only buttons of
                        // a quit combo with Select are held, it waits for the combo's last button
                        // and opens the controls on release if the combo doesn't complete.
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
                    let held = pad.raw_buttons | if pad.back_waiting { 1 << SDL_GAMEPAD_BUTTON_BACK.0 } else { 0 };
                    if b.down && (held == quit || held == menu) {
                        let action = if held == quit { Action::Quit } else { Action::Menu };
                        pad.raw_buttons = 0;
                        pad.c_down = false;
                        pad.back_waiting = false;
                        pad.raw_axes = [0; 6];
                        pad.mouse.release(queue);
                        pad.touchpad.release(queue);
                        pad.update(pad_settings);
                        send_state(pad, mask, queue);
                        return Some(action);
                    }
                    pad.update(pad_settings);
                    let action = pad.mouse.update(pad.report(), mouse_emulation, precision, std::time::Instant::now(), queue);
                    send_state(pad, mask, queue);
                    return action.map(Action::Mouse);
                }
                SDL_EVENT_GAMEPAD_AXIS_MOTION => {
                    let a = event.gaxis;
                    let pad = self.find(a.which)?;
                    *pad.raw_axes.get_mut(usize::from(a.axis))? = a.value;
                    pad.update(pad_settings);
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
                        pad.sent_as,
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
                            let _ = queue.send_controller_motion(pad.sent_as, motion_type::ACCEL, x, y, z);
                        }
                        SDL_SENSOR_GYRO => {
                            let deg = 57.295_78;
                            let _ = queue.send_controller_motion(
                                pad.sent_as,
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

    /// Rumble from the host for gamepad `index`, on every controller sent as it; motor speeds 0-65535.
    pub fn rumble(&self, index: u16, low: u16, high: u16) {
        if self.disable_rumble { return; }
        for pad in self.sent_as(index) {
            if pad.audio.as_ref().is_some_and(|audio| audio.active()) { continue; }
            // SAFETY: an open gamepad. A long duration; the host sends 0 to stop.
            unsafe { SDL_RumbleGamepad(pad.handle, low, high, 30_000) };
        }
    }

    pub fn rumble_triggers(&self, index: u16, left: u16, right: u16) {
        if self.disable_rumble { return; }
        for pad in self.sent_as(index) {
            // SAFETY: an open gamepad.
            unsafe { SDL_RumbleGamepadTriggers(pad.handle, left, right, 30_000) };
        }
    }

    pub fn set_led(&self, index: u16, r: u8, g: u8, b: u8) {
        for pad in self.sent_as(index) {
            // SAFETY: an open gamepad.
            unsafe { SDL_SetGamepadLED(pad.handle, r, g, b) };
            if let Some(audio) = &pad.audio { audio.send(pad.handle); }
        }
    }

    pub fn adaptive_triggers(&self, index: u16, flags: u8, type_left: u8, type_right: u8, left: &[u8; 10], right: &[u8; 10]) {
        for pad in self.sent_as(index) {
            send_trigger_effect(pad, flags, type_left, type_right, left, right);
        }
    }

    /// The host wants motion data from gamepad `index` (rate 0 stops it), from every controller
    /// sent as it.
    pub fn set_motion(&self, index: u16, kind: u8, rate_hz: u16) {
        if !self.motion {
            return;
        }
        let sensor = match kind {
            motion_type::ACCEL => SDL_SENSOR_ACCEL,
            motion_type::GYRO => SDL_SENSOR_GYRO,
            _ => return,
        };
        for pad in self.sent_as(index) {
            // A pad pinned as Xbox told the host it has no motion sensors.
            let on = rate_hz != 0 && !pad.mapping.drops_motion_and_touchpad();
            // SAFETY: an open gamepad.
            unsafe { SDL_SetGamepadSensorEnabled(pad.handle, sensor, on) };
        }
    }

    /// Stops every motor and sensor (stream end).
    pub fn reset(&mut self) {
        for pad in &self.pads {
            send_trigger_effect(pad, 0x0C, 0x05, 0x05, &[0; 10], &[0; 10]);
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

    /// Haptics or speaker audio for gamepad `pcm.controller`: the controller sent as it that plays it.
    pub fn pcm(&mut self, pcm: nightjar_core::control::haptics::Pcm, speaker: bool) {
        if !speaker && self.disable_rumble { return; }
        if let Some(pad) = self.pads.iter_mut().find(|p| u16::from(p.sent_as) == pcm.controller && p.audio.is_some()) {
            if let Some(audio) = &mut pad.audio { audio.offer(pcm, speaker); audio.update(pad.handle); }
        }
    }

    pub fn audio_control(&mut self, payload: &[u8]) {
        if !crate::controller_audio::valid_control(payload) { return; }
        let index = u16::from_le_bytes([payload[2], payload[3]]);
        if let Some(pad) = self.pads.iter_mut().find(|p| u16::from(p.sent_as) == index && p.audio.is_some()) {
            if let Some(audio) = &mut pad.audio { audio.control(payload, pad.handle); }
        }
    }

    /// Holding Guide, on the stream loop's tick: true when a Guide held for the setting's time opens
    /// the stream menu. A short press's release goes out here once the PC has seen it down.
    pub fn guide_tick(&mut self, now: std::time::Instant, queue: &mut InputQueue) -> bool {
        let (pad_settings, mask) = (self.pad_settings(), self.active_mask());
        let mut open = false;
        for pad in &mut self.pads {
            match pad.guide {
                Guide::Held(at) if self.guide_hold.is_some_and(|hold| now.duration_since(at) >= hold) => {
                    pad.guide = Guide::Opened;
                    open = true;
                }
                Guide::Tapped(at) if now.duration_since(at) >= GUIDE_TAP => {
                    pad.raw_buttons &= !pad.guide_bit;
                    pad.guide = Guide::Up;
                    pad.update(pad_settings);
                    send_state(pad, mask, queue);
                }
                _ => {}
            }
        }
        open
    }

    /// A Guide press waits on [`Self::guide_tick`]: the stream loop wakes often until it is settled.
    pub fn guide_waiting(&self) -> bool {
        self.pads.iter().any(|pad| matches!(pad.guide, Guide::Held(_) | Guide::Tapped(_)))
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

    /// The buttons that disconnect, as the settings chose them.
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

/// The host's adaptive trigger effect, on a DualSense.
fn send_trigger_effect(pad: &Pad, flags: u8, type_left: u8, type_right: u8, left: &[u8; 10], right: &[u8; 10]) {
    // SAFETY: an open gamepad.
    unsafe {
        if SDL_GetGamepadType(pad.handle) == SDL_GAMEPAD_TYPE_PS5 {
            let effect = trigger_effect(flags, type_left, type_right, left, right);
            SDL_SendGamepadEffect(pad.handle, effect.as_ptr().cast(), effect.len() as i32);
        }
    }
}

fn send_state(pad: &Pad, mask: u16, queue: &mut InputQueue) {
    let report = if pad.mouse.active { Report::default() } else { pad.report() };
    let _ = queue.send_controller(
        u16::from(pad.sent_as),
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
    // Which controller it is goes first, so the host can choose its virtual pad on arrival
    // (a Switch 2 Pro Controller its own); the queue sends it only to a host that takes it.
    if pad.usb_id != (0, 0) {
        let _ = queue.send_controller_identity(pad.sent_as, pad.usb_id.0, pad.usb_id.1);
    }
    let _ = queue.send_controller_arrival(pad.sent_as, mask, kind, supported, caps);
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
    let _ = queue.send_controller_battery(pad.sent_as, state_code, percent);
}

#[cfg(test)]
mod tests;
