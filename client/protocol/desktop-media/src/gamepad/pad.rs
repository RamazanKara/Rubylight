//! Individual SDL3 gamepad state, physical input reads, and Guide-hold tracking.

use crate::gamepad_types::{clickpad, Mapping, PadSettings, Report};
use crate::pads_view::PadBrand;
use nightjar_core::input::packets::button;
use sdl3_sys::everything::*;

pub struct Pad {
    pub audio: Option<crate::controller_audio::Audio>,
    pub id: SDL_JoystickID,
    pub handle: *mut SDL_Gamepad,
    /// Host controller slot: which controller this is, in the stream menu and the buttons page.
    /// A refused controller takes a free slot when another is unplugged ([`super::Gamepads::assign_slots`]).
    pub index: u8,
    /// The gamepad number the PC knows it as: `index`, or 0 for every controller with
    /// "Controllers" One player ([`crate::gamepad_types::sent_index`]).
    pub(super) sent_as: u8,
    pub key: String,
    pub name: String,
    pub mapping: Mapping,
    pub(super) raw_buttons: u32,
    pub(super) raw_axes: [i16; 6],
    /// USB vendor and product SDL reports (0 when it has none), sent as the controller's
    /// identity before its arrival.
    pub(super) usb_id: (u16, u16),
    /// The Switch 2 controllers' C button (SDL's Misc 2) is down. It sits outside the button
    /// mapping and goes as `button::C`, which only a host that takes controller identities
    /// gets (its flag is a Steam Controller's left grip touch, nightjar#125).
    pub(super) c_down: bool,
    pub(super) mouse: crate::controller_mouse::Mouse,
    pub(super) touchpad: crate::controller_mouse::Touchpad,
    pub(super) buttons: u32,
    pub(super) lt: u8,
    pub(super) rt: u8,
    pub(super) ls: (i16, i16),
    pub(super) rs: (i16, i16),
    pub(super) announced: bool,
    /// This host has no gamepad slot for it ([`super::Gamepads::set_limit`]): it still drives the
    /// launcher and Nightjar's own menu, but its state, touch, motion and battery are not sent.
    pub(super) refused: bool,
    /// SDL reports a PlayStation pad or a touchpad click button: no Select+LB stand-in.
    pub(super) has_touchpad_click: bool,
    pub(super) clickpad_swallowed: u32,
    /// With Back opening the stream controls: Back is held while other buttons of a quit combo
    /// with Select are, so it counts toward that combo and opens the controls on release instead.
    pub(super) back_waiting: bool,
    /// Holding Guide opens the stream menu: where this pad's Guide press is.
    pub(super) guide: Guide,
    /// The physical button (as `1 << SDL_GamepadButton`) `guide` follows: Guide, or Back with
    /// "Back button as Guide".
    pub(super) guide_bit: u32,
}

/// A Guide press while holding it opens the stream menu. The press is kept from the PC until it is
/// known to be short (then the PC gets a whole tap), part of a chord, or long (then it is the menu's).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Guide {
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
pub(super) const GUIDE_TAP: std::time::Duration = std::time::Duration::from_millis(50);

impl Pad {
    /// Physical `(buttons, axes)` as SDL reports them right now (must run on the UI thread).
    pub fn raw_physical(&self) -> (u32, [i16; 6]) {
        // Read physical state so the test view works without sending input to a PC.
        unsafe {
            let buttons = (0..21)
                .filter(|i| SDL_GetGamepadButton(self.handle, SDL_GamepadButton(*i)))
                .fold(0, |bits, i| bits | (1 << i));
            let axes = std::array::from_fn(|i| SDL_GetGamepadAxis(self.handle, SDL_GamepadAxis(i as i32)));
            (buttons, axes)
        }
    }

    pub fn brand(&self) -> PadBrand {
        // SAFETY: an open gamepad handle on the UI thread.
        match unsafe { SDL_GetGamepadType(self.handle) } {
            SDL_GAMEPAD_TYPE_PS3 | SDL_GAMEPAD_TYPE_PS4 | SDL_GAMEPAD_TYPE_PS5 => PadBrand::PlayStation,
            SDL_GAMEPAD_TYPE_NINTENDO_SWITCH_PRO
            | SDL_GAMEPAD_TYPE_NINTENDO_SWITCH_JOYCON_LEFT
            | SDL_GAMEPAD_TYPE_NINTENDO_SWITCH_JOYCON_RIGHT
            | SDL_GAMEPAD_TYPE_NINTENDO_SWITCH_JOYCON_PAIR => PadBrand::Nintendo,
            _ => PadBrand::Xbox,
        }
    }

    /// The physical state through `mapping` and `settings` (the stream's [`PadSettings`], from the
    /// settings, or the face button swap alone), for the buttons page's test view.
    pub fn preview(&self, mapping: &Mapping, settings: impl Into<PadSettings>) -> Report {
        let (buttons, axes) = self.raw_physical();
        mapping.apply(buttons, axes, settings.into())
    }

    /// The gamepad number the PC knows this controller as, or None when the host has no slot
    /// for it ([`super::Gamepads::set_limit`]).
    pub fn slot(&self) -> Option<u8> {
        (!self.refused).then_some(self.sent_as)
    }

    pub(super) fn update(&mut self, settings: PadSettings) {
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
        unsafe {
            SDL_GetGamepadType(self.handle) == SDL_GAMEPAD_TYPE_PS5
                && SDL_GetGamepadConnectionState(self.handle) == SDL_JOYSTICK_CONNECTION_WIRED
        }
    }

    pub(super) fn report(&self) -> Report {
        Report { buttons: self.buttons, lt: self.lt, rt: self.rt, ls: self.ls, rs: self.rs }
    }
}
