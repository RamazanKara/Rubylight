//! Controllers through SDL3: every gamepad is opened at once so it can drive the launcher,
//! and during a stream each one is a host controller with rumble, trigger rumble, motion,
//! touchpad, battery and LED.
//!
//! The mapping (button flags, stick inversion, trigger scaling, motion units, controller
//! types and capabilities, the Start+Select+LB+RB quit combo) follows moonlight-qt's
//! `app/streaming/input/gamepad.cpp` (GPL-3.0, Cameron Gutman and contributors). The other quit
//! combos and the stream menu's controller mouse switch are ported from Moonlight V+ PC
//! (qiin2333/moonlight-qt).

use crate::pads_view::{PadEntry, PadsView};
use crate::strings::{en, trf};
use nightjar_core::input::packets::{motion_type, touch_event};
use nightjar_core::input::InputQueue;
use nightjar_policy::controllers::Controllers;
use sdl3_sys::everything::*;

pub use crate::gamepad_types::*;

mod audio;
mod pad;
mod protocol;

pub use pad::Pad;

use pad::{Guide, GUIDE_TAP};
use protocol::{send_battery, send_state};
#[cfg(test)]
use protocol::{describe, trigger_effect};

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
    rumble_feel: nightjar_policy::rumble::RumbleFeel,
    /// How long Guide is held to open the stream menu; None: Guide always goes to the PC.
    guide_hold: Option<std::time::Duration>,
    /// "Back button as Guide" (never with `back_opens_menu`, [`PadSettings`]).
    back_as_guide: bool,
    /// The settings' dead zones, for a mapping without its own.
    dead_zones: DeadZones,
    /// "Controllers" One player: every controller is sent as gamepad 0 ([`one_player`]).
    one_player: bool,
    /// The gamepad slots this stream's host takes ([`InputQueue::max_controllers`]), set when a
    /// stream starts. None before one, which takes every slot.
    limit: Option<u8>,
    /// The stream's one notice for controllers past [`Self::limit`] has been shown.
    refused_notice_shown: bool,
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

impl Gamepads {
    /// Snapshot for `nightjar-desktop-ui` without exposing `sdl3-sys`.
    pub fn view(&self) -> PadsView {
        let pads = self
            .pads
            .iter()
            .map(|pad| {
                let raw = pad.raw_physical();
                PadEntry {
                    index: pad.index,
                    key: pad.key.clone(),
                    name: pad.name.clone(),
                    brand: pad.brand(),
                    read_raw: Some(std::sync::Arc::new(move || raw)),
                }
            })
            .collect();
        PadsView {
            pads,
            mappings: self.mappings.clone(),
            swap_face_buttons: self.swap_face_buttons,
        }
    }

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
        self.rumble_feel = settings.rumble_feel;
        let pad_settings = PadSettings::from(settings);
        (self.back_as_guide, self.dead_zones) = (pad_settings.back_as_guide, pad_settings.dead_zones);
        self.one_player = one_player(settings);
        self.assign_slots();
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
            refused: false,
            has_touchpad_click,
            clickpad_swallowed: 0,
            back_waiting: false,
            guide: Guide::Up,
            guide_bit: 1 << SDL_GAMEPAD_BUTTON_GUIDE.0,
        });
        self.assign_slots();
        Ok(())
    }

    /// Closes a removed gamepad; returns the gamepad number the PC knew it as, or None when the
    /// host had no slot for it. A controller the host had refused takes the slot it leaves
    /// ([`Self::assign_slots`]).
    pub fn removed(&mut self, id: SDL_JoystickID) -> Option<u8> {
        let position = self.pads.iter().position(|p| p.id == id)?;
        let pad = self.pads.remove(position);
        let slot = pad.slot();
        if let Some(audio) = &pad.audio { audio.reset(pad.handle); }
        drop(pad.audio);
        // SAFETY: a gamepad we opened.
        unsafe { SDL_CloseGamepad(pad.handle) };
        self.assign_slots();
        slot
    }

    /// The gamepad slots this host takes: [`Self::limit`], or every one before a stream.
    pub fn controller_limit(&self) -> u8 {
        self.limit.unwrap_or(MAX_PADS as u8)
    }

    /// Numbers each pad's slot for the PC: its own index, or 0 with One player, and whether the
    /// host has no slot for it. A refused pad moves into the first slot an in-limit controller
    /// leaves, so unplugging one starts a controller the host refused for the rest of the stream.
    fn assign_slots(&mut self) {
        let limit = u16::from(self.controller_limit());
        if self.one_player {
            // Every controller is player 1: none is refused and every index stays.
            for pad in &mut self.pads {
                pad.sent_as = 0;
                pad.refused = false;
            }
            return;
        }
        // The slots in-limit controllers already hold; the rest are free for a refused pad.
        let mut taken = [false; MAX_PADS];
        for pad in &self.pads {
            if u16::from(pad.index) < limit {
                taken[usize::from(pad.index)] = true;
            }
        }
        for pad in &mut self.pads {
            if u16::from(pad.index) >= limit {
                let Some(slot) = (0..usize::from(limit)).find(|&slot| !taken[slot]) else {
                    pad.refused = true;
                    pad.announced = false;
                    continue;
                };
                pad.index = slot as u8;
                taken[slot] = true;
            }
            pad.sent_as = pad.index;
            pad.refused = false;
        }
    }

    /// Sets the controller slots this stream's host takes at its start
    /// ([`InputQueue::max_controllers`]); returns how many pads are past them. Those still drive
    /// Nightjar's own menu, but nothing of theirs is sent, and [`Self::refused_notice`] says why
    /// once.
    pub fn set_limit(&mut self, limit: u8) -> usize {
        self.limit = Some(limit.clamp(1, MAX_PADS as u8));
        self.refused_notice_shown = false;
        self.assign_slots();
        self.pads.iter().filter(|pad| pad.refused).count()
    }

    /// The once-a-stream notice for the controllers this host has no slot for, if one is past the
    /// limit: "This PC takes 4 controllers".
    pub fn refused_notice(&mut self) -> Option<String> {
        if self.refused_notice_shown || !self.pads.iter().any(|pad| pad.refused) {
            return None;
        }
        self.refused_notice_shown = true;
        Some(trf("This PC takes {} controllers", &[&self.controller_limit()]))
    }

    /// The gamepads the PC has during the stream: player 1 alone with "Controllers" One player
    /// (also without a controller, as Android keeps it for games that miss one added later), else
    /// one for each controller.
    pub fn active_mask(&self) -> u16 {
        if self.one_player { 1 } else { self.pads.iter().filter(|p| !p.refused).fold(0, |m, p| m | (1 << p.sent_as)) }
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
                    if pad.refused || pad.mapping.drops_motion_and_touchpad() { return None; }
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
                    if pad.refused || pad.mapping.drops_motion_and_touchpad() { return None; }
                    let [x, y, z] = s.data;
                    match SDL_SensorType(s.sensor) {
                        SDL_SENSOR_ACCEL => {
                            let _ = queue.send_controller_motion(pad.sent_as, motion_type::ACCEL, x, y, z);
                        }
                        SDL_SENSOR_GYRO => {
                            let deg = 57.295_78;
                            let [x, y, z] = pad.mapping.map_gyro([x * deg, y * deg, z * deg]);
                            let _ = queue.send_controller_motion(pad.sent_as, motion_type::GYRO, x, y, z);
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

#[cfg(test)]
mod tests;
#[cfg(test)]
mod chords_tests;
#[cfg(test)]
mod slots_tests;

#[cfg(test)]
#[test]
fn sdl_button_indices_match_sdl3() {
    use crate::gamepad_types::sdl_button_index as idx;
    assert_eq!(idx::SOUTH as i32, SDL_GAMEPAD_BUTTON_SOUTH.0);
    assert_eq!(idx::EAST as i32, SDL_GAMEPAD_BUTTON_EAST.0);
    assert_eq!(idx::WEST as i32, SDL_GAMEPAD_BUTTON_WEST.0);
    assert_eq!(idx::NORTH as i32, SDL_GAMEPAD_BUTTON_NORTH.0);
    assert_eq!(idx::BACK as i32, SDL_GAMEPAD_BUTTON_BACK.0);
    assert_eq!(idx::GUIDE as i32, SDL_GAMEPAD_BUTTON_GUIDE.0);
    assert_eq!(idx::START as i32, SDL_GAMEPAD_BUTTON_START.0);
    assert_eq!(idx::LEFT_SHOULDER as i32, SDL_GAMEPAD_BUTTON_LEFT_SHOULDER.0);
    assert_eq!(idx::RIGHT_SHOULDER as i32, SDL_GAMEPAD_BUTTON_RIGHT_SHOULDER.0);
}
