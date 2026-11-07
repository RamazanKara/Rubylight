//! The device in the hand, from the launcher's last SDL event: what the button hints name. A stick
//! at rest drifts, so it counts only past the point where it moves the focus; a device that SDL calls
//! a gamepad but has no stick is no pad.

use sdl3_sys::everything::*;

use crate::glyphs::{self, Piece, Style};
use crate::shortcuts::{self, Action, Button, Chord, Key};

/// How far a stick goes before it counts as the device in the hand: where it starts to move the focus.
pub const STICK_HELD: i32 = 24000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputSource {
    /// A mouse or a finger: no hints.
    #[default]
    None,
    Keyboard,
    Pad(Style),
}

impl InputSource {
    /// The key or button that does what has focus.
    pub fn go(self, swap_face_buttons: bool) -> Vec<Piece> {
        self.pieces(Action::Select, Key::Enter, swap_face_buttons)
    }

    /// The key or button that opens what has focus's options.
    pub fn more(self, swap_face_buttons: bool) -> Vec<Piece> {
        self.pieces(Action::Options, Key::F, swap_face_buttons)
    }

    fn pieces(self, action: Action, key: Key, swap: bool) -> Vec<Piece> {
        match self {
            InputSource::None => Vec::new(),
            InputSource::Keyboard => glyphs::chord(Chord::key(key), false),
            InputSource::Pad(style) => [Button::South, Button::East, Button::West, Button::North]
                .into_iter()
                .find(|button| shortcuts::pad_action(*button, swap) == Some(action))
                .map(|button| vec![style.piece(button)])
                .unwrap_or_default(),
        }
    }
}

/// What this event says of the device in the hand; `None` when it says nothing. `real_pad` gives the
/// type of a gamepad that is a real pad, with a stick.
pub fn of(event: &SDL_Event, real_pad: impl Fn(SDL_JoystickID) -> Option<SDL_GamepadType>) -> Option<InputSource> {
    let pad = |id| real_pad(id).map(|kind| InputSource::Pad(Style::of_types([kind])));
    // SAFETY: union members read by event type.
    unsafe {
        match SDL_EventType(event.r#type) {
            SDL_EVENT_KEY_DOWN => Some(InputSource::Keyboard),
            SDL_EVENT_MOUSE_BUTTON_DOWN | SDL_EVENT_MOUSE_WHEEL | SDL_EVENT_FINGER_DOWN => Some(InputSource::None),
            SDL_EVENT_MOUSE_MOTION if event.motion.xrel != 0.0 || event.motion.yrel != 0.0 => Some(InputSource::None),
            SDL_EVENT_GAMEPAD_BUTTON_DOWN => pad(event.gbutton.which),
            SDL_EVENT_GAMEPAD_AXIS_MOTION => {
                let stick = matches!(SDL_GamepadAxis(i32::from(event.gaxis.axis)), SDL_GAMEPAD_AXIS_LEFTX | SDL_GAMEPAD_AXIS_LEFTY);
                (stick && i32::from(event.gaxis.value).abs() > STICK_HELD).then(|| pad(event.gaxis.which)).flatten()
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: SDL_EventType) -> SDL_Event {
        // SAFETY: SDL_Event is plain data, and all zeroes is a valid one.
        let mut event: SDL_Event = unsafe { std::mem::zeroed() };
        event.r#type = kind.0;
        event
    }

    fn sony(_: SDL_JoystickID) -> Option<SDL_GamepadType> {
        Some(SDL_GAMEPAD_TYPE_PS5)
    }

    fn no_stick(_: SDL_JoystickID) -> Option<SDL_GamepadType> {
        None
    }

    #[test]
    fn each_device_says_what_it_is() {
        assert_eq!(of(&event(SDL_EVENT_KEY_DOWN), sony), Some(InputSource::Keyboard));
        assert_eq!(of(&event(SDL_EVENT_MOUSE_BUTTON_DOWN), sony), Some(InputSource::None));
        assert_eq!(of(&event(SDL_EVENT_MOUSE_WHEEL), sony), Some(InputSource::None));
        assert_eq!(of(&event(SDL_EVENT_GAMEPAD_BUTTON_DOWN), sony), Some(InputSource::Pad(Style::PlayStation)));
        assert_eq!(of(&event(SDL_EVENT_WINDOW_RESIZED), sony), None, "a window event says nothing");
    }

    #[test]
    fn a_mouse_that_did_not_move_says_nothing() {
        let mut still = event(SDL_EVENT_MOUSE_MOTION);
        assert_eq!(of(&still, sony), None);
        still.motion.xrel = 3.0;
        assert_eq!(of(&still, sony), Some(InputSource::None));
    }

    #[test]
    fn a_device_with_no_stick_is_no_pad() {
        assert_eq!(of(&event(SDL_EVENT_GAMEPAD_BUTTON_DOWN), no_stick), None);
    }

    #[test]
    fn a_drifting_stick_says_nothing() {
        let mut stick = event(SDL_EVENT_GAMEPAD_AXIS_MOTION);
        stick.gaxis.axis = SDL_GAMEPAD_AXIS_LEFTX.0 as u8;
        stick.gaxis.value = 9000;
        assert_eq!(of(&stick, sony), None, "at rest it drifts");
        stick.gaxis.value = -30000;
        assert_eq!(of(&stick, sony), Some(InputSource::Pad(Style::PlayStation)));
        stick.gaxis.axis = SDL_GAMEPAD_AXIS_LEFT_TRIGGER.0 as u8;
        assert_eq!(of(&stick, sony), None, "a trigger moves no focus");
    }

    #[test]
    fn the_hints_name_what_the_buttons_really_do() {
        let word = |pieces: Vec<Piece>| match pieces.as_slice() {
            [Piece::Face(name)] => (*name).to_owned(),
            other => panic!("one face button: {other:?}"),
        };
        assert_eq!(word(InputSource::Pad(Style::Xbox).go(false)), "A");
        assert_eq!(word(InputSource::Pad(Style::Xbox).more(false)), "X");
        assert_eq!(word(InputSource::Pad(Style::Xbox).go(true)), "B", "the Nintendo layout setting trades them");
        assert_eq!(glyphs::spoken(&InputSource::Keyboard.go(false)), "Enter");
        assert_eq!(glyphs::spoken(&InputSource::Keyboard.more(false)), "F");
        assert!(InputSource::None.go(false).is_empty() && InputSource::None.more(false).is_empty());
    }
}
