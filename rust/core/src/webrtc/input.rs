//! The Rubylight `input` DataChannel extension: keyboard and mouse messages
//! from a WebRTC client, mapped to the [`Input`] events the Moonlight path
//! produces, and the tracker that releases whatever a client still holds when
//! its channel closes.
//!
//! Messages are JSON with `"v":1`. Keys are DOM `KeyboardEvent.code` values,
//! mapped to the Windows virtual key `Input::Keyboard` carries, with
//! `flags` 0 (a normalized key) and no modifier bits: modifiers are ordinary
//! keys. Mouse buttons are Moonlight's 1 to 5, wheel units are 120 per notch
//! and Moonlight's scroll sign is up positive.
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use serde::Deserialize;
use serde_json::Value;

use crate::input::Input;

/// The only `v` the `input` channel accepts.
pub const VERSION: i64 = 1;
/// Most characters one `text` message may carry.
pub const MAX_TEXT_CHARS: usize = 32;
/// Wheel units per notch, in both DOM and Moonlight terms.
pub const WHEEL_NOTCH: i16 = 120;
/// Moonlight's `MODIFIER_EXTENDED`: the key is the extended twin of its VK,
/// as for keypad Enter, which shares VK_RETURN with the main Enter key.
const MODIFIER_EXTENDED: u8 = 0x10;
/// Marks a table entry whose key needs the extended bit on top of its VK.
const EXT: u16 = 0x100;

/// Why a message was refused. Never a panic: a hostile or newer client can
/// send anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageError {
    /// Not JSON, or a field missing or of the wrong type.
    Malformed(String),
    /// `v` missing, not an integer, or not one this host speaks.
    Version(Option<i64>),
    /// A `type` this channel does not define.
    UnknownType(String),
    /// A well-formed message with a value outside what it allows.
    Invalid(&'static str),
}

impl fmt::Display for MessageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(why) => write!(f, "malformed message: {why}"),
            Self::Version(Some(v)) => write!(f, "unsupported message version {v}"),
            Self::Version(None) => f.write_str("message has no version"),
            Self::UnknownType(kind) => write!(f, "unknown message type {kind:?}"),
            Self::Invalid(why) => write!(f, "invalid message: {why}"),
        }
    }
}

impl std::error::Error for MessageError {}

/// One parsed `input` channel message.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum InputMessage {
    Key {
        code: String,
        down: bool,
    },
    Text {
        text: String,
    },
    MouseMove {
        dx: f64,
        dy: f64,
    },
    MousePosition {
        x: f64,
        y: f64,
    },
    MouseButton {
        button: i64,
        down: bool,
    },
    Wheel {
        #[serde(default)]
        dx: f64,
        #[serde(default)]
        dy: f64,
    },
}

const TYPES: [&str; 6] = [
    "key",
    "text",
    "mouse-move",
    "mouse-position",
    "mouse-button",
    "wheel",
];

/// Parse one text message from the `input` channel.
pub fn parse(text: &str) -> Result<InputMessage, MessageError> {
    let value: Value =
        serde_json::from_str(text).map_err(|e| MessageError::Malformed(e.to_string()))?;
    check_envelope(&value, &TYPES, &[VERSION])?;
    let message: InputMessage =
        serde_json::from_value(value).map_err(|e| MessageError::Malformed(e.to_string()))?;
    match &message {
        InputMessage::Text { text } if text.chars().count() > MAX_TEXT_CHARS => {
            Err(MessageError::Invalid("text is longer than 32 characters"))
        }
        InputMessage::MouseButton { button, .. } if !(0..=4).contains(button) => {
            Err(MessageError::Invalid("mouse button is not 0 to 4"))
        }
        _ => Ok(message),
    }
}

/// Check that a parsed message is an object whose `v` is one of `versions`
/// and whose `type` is one of `types`.
pub(crate) fn check_envelope(
    value: &Value,
    types: &[&str],
    versions: &[i64],
) -> Result<(), MessageError> {
    let Some(object) = value.as_object() else {
        return Err(MessageError::Malformed("not a JSON object".into()));
    };
    let version = object.get("v").and_then(Value::as_i64);
    if !version.is_some_and(|v| versions.contains(&v)) {
        return Err(MessageError::Version(version));
    }
    match object.get("type").and_then(Value::as_str) {
        Some(kind) if types.contains(&kind) => Ok(()),
        Some(kind) => Err(MessageError::UnknownType(kind.to_owned())),
        None => Err(MessageError::Malformed("message has no type".into())),
    }
}

/// A number of any size from a client, as the `i16` Moonlight carries.
fn to_i16(value: f64) -> i16 {
    // `as` saturates and turns NaN into 0.
    value.round() as i16
}

/// Add `delta` to the carried remainder and take out the whole pixels, as
/// the `i16` Moonlight carries; the fraction stays for the next message.
/// Motion beyond `i16` saturates and the excess is dropped.
fn take_whole(remainder: &mut f64, delta: f64) -> i16 {
    let total = *remainder + delta;
    // A NaN cannot come from JSON; 0 is the safe answer.
    let whole = if total.is_nan() { 0. } else { total.trunc() };
    *remainder = total - whole;
    if remainder.is_nan() || remainder.abs() >= 1. {
        *remainder = 0.;
    }
    whole as i16
}

/// The Windows virtual key of a `KeyboardEvent.code`, and whether the key is
/// the extended twin of that VK.
pub fn virtual_key(code: &str) -> Option<(u16, bool)> {
    KEYS.iter()
        .find(|(name, _)| *name == code)
        .map(|(_, vk)| (vk & 0xff, vk & EXT != 0))
}

/// Every `KeyboardEvent.code` the host maps, with its Windows virtual key.
/// Punctuation follows the US layout's physical positions, which is what
/// `code` names. Codes with no virtual key (Fn, Power, Hyper) are absent.
pub const KEYS: &[(&str, u16)] = &[
    // Letters
    ("KeyA", 0x41),
    ("KeyB", 0x42),
    ("KeyC", 0x43),
    ("KeyD", 0x44),
    ("KeyE", 0x45),
    ("KeyF", 0x46),
    ("KeyG", 0x47),
    ("KeyH", 0x48),
    ("KeyI", 0x49),
    ("KeyJ", 0x4a),
    ("KeyK", 0x4b),
    ("KeyL", 0x4c),
    ("KeyM", 0x4d),
    ("KeyN", 0x4e),
    ("KeyO", 0x4f),
    ("KeyP", 0x50),
    ("KeyQ", 0x51),
    ("KeyR", 0x52),
    ("KeyS", 0x53),
    ("KeyT", 0x54),
    ("KeyU", 0x55),
    ("KeyV", 0x56),
    ("KeyW", 0x57),
    ("KeyX", 0x58),
    ("KeyY", 0x59),
    ("KeyZ", 0x5a),
    // Digit row
    ("Digit0", 0x30),
    ("Digit1", 0x31),
    ("Digit2", 0x32),
    ("Digit3", 0x33),
    ("Digit4", 0x34),
    ("Digit5", 0x35),
    ("Digit6", 0x36),
    ("Digit7", 0x37),
    ("Digit8", 0x38),
    ("Digit9", 0x39),
    // Function keys
    ("F1", 0x70),
    ("F2", 0x71),
    ("F3", 0x72),
    ("F4", 0x73),
    ("F5", 0x74),
    ("F6", 0x75),
    ("F7", 0x76),
    ("F8", 0x77),
    ("F9", 0x78),
    ("F10", 0x79),
    ("F11", 0x7a),
    ("F12", 0x7b),
    ("F13", 0x7c),
    ("F14", 0x7d),
    ("F15", 0x7e),
    ("F16", 0x7f),
    ("F17", 0x80),
    ("F18", 0x81),
    ("F19", 0x82),
    ("F20", 0x83),
    ("F21", 0x84),
    ("F22", 0x85),
    ("F23", 0x86),
    ("F24", 0x87),
    // Numpad
    ("Numpad0", 0x60),
    ("Numpad1", 0x61),
    ("Numpad2", 0x62),
    ("Numpad3", 0x63),
    ("Numpad4", 0x64),
    ("Numpad5", 0x65),
    ("Numpad6", 0x66),
    ("Numpad7", 0x67),
    ("Numpad8", 0x68),
    ("Numpad9", 0x69),
    ("NumpadMultiply", 0x6a),
    ("NumpadAdd", 0x6b),
    ("NumpadSubtract", 0x6d),
    ("NumpadDecimal", 0x6e),
    ("NumpadDivide", 0x6f),
    // Keypad Enter is VK_RETURN, told apart by the extended bit.
    ("NumpadEnter", 0x0d | EXT),
    // VK_ABNT_C2, the Brazilian keypad comma.
    ("NumpadComma", 0xc2),
    // VK_OEM_NEC_EQUAL, the keypad equals sign.
    ("NumpadEqual", 0x92),
    // Numpad 5 with Num Lock off.
    ("NumpadClear", 0x0c),
    ("NumLock", 0x90),
    // Arrows and navigation
    ("ArrowLeft", 0x25),
    ("ArrowUp", 0x26),
    ("ArrowRight", 0x27),
    ("ArrowDown", 0x28),
    ("Home", 0x24),
    ("End", 0x23),
    ("PageUp", 0x21),
    ("PageDown", 0x22),
    ("Insert", 0x2d),
    ("Delete", 0x2e),
    // Editing and whitespace
    ("Escape", 0x1b),
    ("Tab", 0x09),
    ("Backspace", 0x08),
    ("Enter", 0x0d),
    ("Space", 0x20),
    ("ContextMenu", 0x5d),
    // Locks and system keys
    ("CapsLock", 0x14),
    ("ScrollLock", 0x91),
    ("PrintScreen", 0x2c),
    ("Pause", 0x13),
    ("Sleep", 0x5f),
    // Modifiers, left and right
    ("ShiftLeft", 0xa0),
    ("ShiftRight", 0xa1),
    ("ControlLeft", 0xa2),
    ("ControlRight", 0xa3),
    ("AltLeft", 0xa4),
    ("AltRight", 0xa5),
    ("MetaLeft", 0x5b),
    ("MetaRight", 0x5c),
    // Firefox before 118 names the Windows keys OS.
    ("OSLeft", 0x5b),
    ("OSRight", 0x5c),
    // Punctuation, by US-layout physical position
    ("Backquote", 0xc0),
    ("Minus", 0xbd),
    ("Equal", 0xbb),
    ("BracketLeft", 0xdb),
    ("BracketRight", 0xdd),
    ("Backslash", 0xdc),
    ("Semicolon", 0xba),
    ("Quote", 0xde),
    ("Comma", 0xbc),
    ("Period", 0xbe),
    ("Slash", 0xbf),
    // The extra key of ISO layouts: VK_OEM_102.
    ("IntlBackslash", 0xe2),
    // The Brazilian slash key: VK_ABNT_C1.
    ("IntlRo", 0xc1),
    // The Japanese yen key is VK_OEM_5, the same VK as Backslash.
    ("IntlYen", 0xdc),
    // Language keys: VK_HANGUL/VK_KANA, VK_HANJA/VK_KANJI, convert, non-convert.
    ("Lang1", 0x15),
    ("Lang2", 0x19),
    ("KanaMode", 0x15),
    ("Convert", 0x1c),
    ("NonConvert", 0x1d),
    // Media and browser keys that have a VK
    ("BrowserBack", 0xa6),
    ("BrowserForward", 0xa7),
    ("BrowserRefresh", 0xa8),
    ("BrowserStop", 0xa9),
    ("BrowserSearch", 0xaa),
    ("BrowserFavorites", 0xab),
    ("BrowserHome", 0xac),
    ("AudioVolumeMute", 0xad),
    ("AudioVolumeDown", 0xae),
    ("AudioVolumeUp", 0xaf),
    ("MediaTrackNext", 0xb0),
    ("MediaTrackPrevious", 0xb1),
    ("MediaStop", 0xb2),
    ("MediaPlayPause", 0xb3),
    ("LaunchMail", 0xb4),
    ("MediaSelect", 0xb5),
    ("LaunchApp1", 0xb6),
    ("LaunchApp2", 0xb7),
];

/// The keys and mouse buttons one client holds down. Feed it every event the
/// client causes, from the `input` channel and from gamepad mouse mode, and
/// it yields the releases when the channel closes, the stream stops or the
/// session switches.
#[derive(Debug, Default, Clone)]
pub struct HeldInput {
    /// (key, extended) to the modifiers and flags of the press, which the
    /// release repeats so the host sees the same key.
    keys: BTreeMap<(u16, bool), (u8, u8)>,
    buttons: BTreeSet<u8>,
}

impl HeldInput {
    pub fn observe(&mut self, input: &Input) {
        match input {
            Input::Keyboard {
                key,
                modifiers,
                flags,
                down,
            } => {
                let id = (*key, modifiers & MODIFIER_EXTENDED != 0);
                if *down {
                    self.keys.insert(id, (*modifiers, *flags));
                } else {
                    self.keys.remove(&id);
                }
            }
            Input::MouseButton { button, down } => {
                if *down {
                    self.buttons.insert(*button);
                } else {
                    self.buttons.remove(button);
                }
            }
            _ => {}
        }
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.buttons.is_empty()
    }

    /// The release of everything held, mouse buttons first. Forgets it all.
    pub fn release_all(&mut self) -> Vec<Input> {
        let buttons =
            std::mem::take(&mut self.buttons)
                .into_iter()
                .map(|button| Input::MouseButton {
                    button,
                    down: false,
                });
        let keys =
            std::mem::take(&mut self.keys)
                .into_iter()
                .map(|((key, _), (modifiers, flags))| Input::Keyboard {
                    key,
                    modifiers,
                    flags,
                    down: false,
                });
        buttons.chain(keys).collect()
    }
}

/// One client's `input` channel: turns messages into [`Input`] events and
/// remembers what is held.
#[derive(Debug, Clone)]
pub struct InputChannel {
    width: i16,
    height: i16,
    held: HeldInput,
    unknown_codes: u64,
    /// Sub-pixel motion not yet sent, per axis, so that fractional
    /// `movementX/Y` from a high-resolution mouse adds up instead of rounding to nothing.
    remainder: (f64, f64),
}

impl InputChannel {
    /// `width` and `height` are the stream's size, which absolute mouse
    /// positions are scaled against.
    pub fn new(width: u32, height: u32) -> Self {
        let clamp = |n: u32| n.clamp(1, i16::MAX as u32) as i16;
        Self {
            width: clamp(width),
            height: clamp(height),
            held: HeldInput::default(),
            unknown_codes: 0,
            remainder: (0., 0.),
        }
    }

    /// Key codes dropped so far because the table has no virtual key for
    /// them, for the stream summary.
    pub fn unknown_codes(&self) -> u64 {
        self.unknown_codes
    }

    pub fn held(&self) -> &HeldInput {
        &self.held
    }

    /// Record an event that did not come from this channel but presses a key
    /// or button for the same client, such as gamepad mouse mode.
    pub fn observe(&mut self, input: &Input) {
        self.held.observe(input);
    }

    /// The release of everything this client holds.
    pub fn release_all(&mut self) -> Vec<Input> {
        self.held.release_all()
    }

    /// Parse a message and return the events it causes, possibly none.
    pub fn handle(&mut self, text: &str) -> Result<Vec<Input>, MessageError> {
        let events = self.translate(parse(text)?);
        for event in &events {
            self.held.observe(event);
        }
        Ok(events)
    }

    /// The events of a parsed message, without recording them as held.
    pub fn translate(&mut self, message: InputMessage) -> Vec<Input> {
        match message {
            InputMessage::Key { code, down } => match virtual_key(&code) {
                Some((key, extended)) => vec![Input::Keyboard {
                    key,
                    modifiers: if extended { MODIFIER_EXTENDED } else { 0 },
                    flags: 0,
                    down,
                }],
                None => {
                    self.unknown_codes += 1;
                    Vec::new()
                }
            },
            InputMessage::Text { text } if text.is_empty() => Vec::new(),
            InputMessage::Text { text } => vec![Input::Text(text)],
            InputMessage::MouseMove { dx, dy } => {
                let (x, y) = (
                    take_whole(&mut self.remainder.0, dx),
                    take_whole(&mut self.remainder.1, dy),
                );
                if x == 0 && y == 0 {
                    Vec::new()
                } else {
                    vec![Input::Relative { x, y }]
                }
            }
            InputMessage::MousePosition { x, y } => {
                let scale = |fraction: f64, size: i16| {
                    // A NaN fraction cannot come from JSON, but 0 is the safe answer.
                    let fraction = if fraction.is_nan() { 0. } else { fraction };
                    (fraction.clamp(0., 1.) * f64::from(size)).round() as i16
                };
                vec![Input::Absolute {
                    x: scale(x, self.width),
                    y: scale(y, self.height),
                    width: self.width,
                    height: self.height,
                }]
            }
            // The parser already limited the button to 0 to 4. DOM order is
            // left, middle, right, back, forward; Moonlight's is the same
            // numbered from 1.
            InputMessage::MouseButton { button, down } => vec![Input::MouseButton {
                button: (button.clamp(0, 4) + 1) as u8,
                down,
            }],
            InputMessage::Wheel { dx, dy } => {
                let mut events = Vec::new();
                // DOM deltaY is positive down; Moonlight's scroll is positive up.
                // Clamp first so negating i16::MIN cannot overflow.
                let vertical = -to_i16(dy).max(-i16::MAX);
                if vertical != 0 {
                    events.push(Input::Scroll {
                        amount: vertical,
                        horizontal: false,
                    });
                }
                // DOM deltaX and Moonlight's horizontal scroll are both positive right.
                let horizontal = to_i16(dx).max(-i16::MAX);
                if horizontal != 0 {
                    events.push(Input::Scroll {
                        amount: horizontal,
                        horizontal: true,
                    });
                }
                events
            }
        }
    }
}

#[cfg(test)]
mod tests;
