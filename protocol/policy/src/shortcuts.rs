//! The user's keyboard shortcuts: the file both clients save and import (the Android app's
//! format, `{"version":1,"data":[{"id","name","keys":["CTRL","SHIFT","ESC"]}]}`), its limits,
//! the portable key names, the built-in chords the stream menu offers, and the keys its key
//! picker shows.
//!
//! Android's `KeyboardShortcuts` reads and writes the same file; the stream menu on both clients
//! adds, edits and removes shortcuts through [`Shortcuts`].

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

pub const MAX_SHORTCUTS: usize = 32;
pub const MAX_KEYS: usize = 8;
pub const MAX_BYTES: usize = 64 * 1024;
pub const MAX_NAME: usize = 80;
pub const MAX_CHORD: usize = 256;

/// Why a shortcut file or a shortcut was refused. Each client words it in its own language.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShortcutError {
    TooLarge,
    /// Not the file's JSON shape; the parser's own words.
    Invalid(String),
    Version,
    TooMany,
    BadId,
    BadName,
    ChordTooLong,
    ChordLength,
    UnsupportedKey(String),
    RepeatedKey(String),
    /// Edited or removed while it was already gone.
    Gone,
}

impl ShortcutError {
    /// A stable name for the error, for clients that look up their own wording.
    pub fn code(&self) -> &'static str {
        match self {
            Self::TooLarge => "too_large",
            Self::Invalid(_) => "invalid",
            Self::Version => "version",
            Self::TooMany => "too_many",
            Self::BadId => "bad_id",
            Self::BadName => "bad_name",
            Self::ChordTooLong => "chord_too_long",
            Self::ChordLength => "chord_length",
            Self::UnsupportedKey(_) => "unsupported_key",
            Self::RepeatedKey(_) => "repeated_key",
            Self::Gone => "gone",
        }
    }

    /// The key or parser message the error names, if any.
    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::Invalid(detail) | Self::UnsupportedKey(detail) | Self::RepeatedKey(detail) => Some(detail),
            _ => None,
        }
    }
}

impl std::fmt::Display for ShortcutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.detail() {
            Some(detail) => write!(f, "{}: {detail}", self.code()),
            None => f.write_str(self.code()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shortcut {
    pub id: String,
    pub name: String,
    pub keys: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shortcuts {
    #[serde(default = "version")]
    version: u8,
    pub data: Vec<Shortcut>,
}

fn version() -> u8 {
    1
}

impl Default for Shortcuts {
    fn default() -> Self {
        Self { version: 1, data: Vec::new() }
    }
}

impl Shortcuts {
    pub fn parse(text: &str) -> Result<Self, ShortcutError> {
        if text.len() > MAX_BYTES {
            return Err(ShortcutError::TooLarge);
        }
        let mut entries: Self = serde_json::from_str(text).map_err(|e| ShortcutError::Invalid(e.to_string()))?;
        if entries.version != 1 {
            return Err(ShortcutError::Version);
        }
        if entries.data.len() > MAX_SHORTCUTS {
            return Err(ShortcutError::TooMany);
        }
        let mut ids = HashSet::new();
        for entry in &mut entries.data {
            if !valid_id(&entry.id) || !ids.insert(entry.id.clone()) {
                return Err(ShortcutError::BadId);
            }
            validate_name(&entry.name)?;
            entry.keys = normalize_keys(&entry.keys)?;
            entry.name = entry.name.trim().into();
        }
        Ok(entries)
    }

    pub fn encode(&self) -> String {
        serde_json::to_string_pretty(self).expect("shortcuts serialize")
    }

    /// Saves a chord typed as text (`CTRL + SHIFT + ESC`): over the shortcut `id`, or as a new one
    /// whose ID starts with `prefix`.
    pub fn save_chord(&mut self, id: Option<&str>, name: &str, chord: &str, prefix: &str) -> Result<String, ShortcutError> {
        if chord.len() > MAX_CHORD {
            return Err(ShortcutError::ChordTooLong);
        }
        let keys: Vec<_> = chord.split('+').map(|key| key.trim().to_uppercase()).collect();
        self.save(id, name, &keys, prefix)
    }

    /// Saves `keys` under `name`: over the shortcut `id`, or as a new one at the end whose ID
    /// starts with `prefix`. Returns the shortcut's ID; nothing changes when it is refused.
    pub fn save(&mut self, id: Option<&str>, name: &str, keys: &[String], prefix: &str) -> Result<String, ShortcutError> {
        validate_name(name)?;
        let keys = normalize_keys(keys)?;
        if let Some(id) = id {
            let entry = self.data.iter_mut().find(|e| e.id == id).ok_or(ShortcutError::Gone)?;
            entry.name = name.trim().into();
            entry.keys = keys;
            return Ok(id.into());
        }
        if self.data.len() >= MAX_SHORTCUTS {
            return Err(ShortcutError::TooMany);
        }
        let id = (1..).map(|i| format!("{prefix}-{i}")).find(|id| !self.data.iter().any(|e| &e.id == id)).unwrap();
        if !valid_id(&id) {
            return Err(ShortcutError::BadId);
        }
        self.data.push(Shortcut { id: id.clone(), name: name.trim().into(), keys });
        Ok(id)
    }

    pub fn remove(&mut self, id: &str) -> Result<(), ShortcutError> {
        let before = self.data.len();
        self.data.retain(|e| e.id != id);
        if self.data.len() == before {
            Err(ShortcutError::Gone)
        } else {
            Ok(())
        }
    }

    /// Puts the shortcut `id` at `index` of the list, or last when `index` is past the end. The
    /// list's order is the order the stream menu shows them in.
    pub fn move_to(&mut self, id: &str, index: usize) -> Result<(), ShortcutError> {
        let from = self.data.iter().position(|e| e.id == id).ok_or(ShortcutError::Gone)?;
        let entry = self.data.remove(from);
        self.data.insert(index.min(self.data.len()), entry);
        Ok(())
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn validate_name(name: &str) -> Result<(), ShortcutError> {
    if name.trim().is_empty() || name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
        Err(ShortcutError::BadName)
    } else {
        Ok(())
    }
}

/// The keys in their portable names, modifiers first and otherwise in the order given, refused
/// when empty, longer than [`MAX_KEYS`], unknown or repeated. A chord is pressed in this order and
/// released in reverse, so Esc picked before Ctrl and Shift still sends Ctrl + Shift + Esc.
pub fn normalize_keys(keys: &[String]) -> Result<Vec<String>, ShortcutError> {
    if keys.is_empty() || keys.len() > MAX_KEYS {
        return Err(ShortcutError::ChordLength);
    }
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for name in keys {
        let code = key_code(name).ok_or_else(|| ShortcutError::UnsupportedKey(name.clone()))?;
        if !seen.insert(code) {
            return Err(ShortcutError::RepeatedKey(name.clone()));
        }
        normalized.push(key_name(code));
    }
    // Stable: modifiers keep their order among themselves, and so do the other keys.
    normalized.sort_by_key(|name| !is_modifier(name));
    Ok(normalized)
}

/// The portable name the file uses for a Windows virtual-key code that [`key_code`] accepts.
pub fn key_name(code: u16) -> String {
    try_key_name(code).expect("validated shortcut key")
}

/// The portable name for a Windows virtual-key code, if [`key_code`] has one for it.
pub fn try_key_name(code: u16) -> Option<String> {
    Some(match code {
        0x30..=0x39 | 0x41..=0x5A => char::from(code as u8).to_string(),
        0x70..=0x87 => format!("F{}", code - 0x6F),
        0x60..=0x69 => format!("NUMPAD_{}", code - 0x60),
        _ => match code {
            0xA2 => "CTRL", 0xA3 => "CTRL_RIGHT", 0xA0 => "SHIFT", 0xA1 => "SHIFT_RIGHT",
            0xA4 => "ALT", 0xA5 => "ALT_RIGHT", 0x5B => "WIN", 0x5C => "META_RIGHT",
            0x1B => "ESC", 0x08 => "BACKSPACE", 0x2E => "DELETE", 0x0D => "ENTER",
            0x09 => "TAB", 0x20 => "SPACE", 0x0C => "CLEAR", 0x24 => "HOME", 0x23 => "END",
            0x14 => "CAPS_LOCK", 0x90 => "NUM_LOCK", 0x91 => "SCROLL_LOCK", 0x2C => "SYSRQ",
            0x2A => "PRINT", 0x13 => "BREAK", 0x21 => "PAGE_UP", 0x22 => "PAGE_DOWN",
            0x25 => "DPAD_LEFT", 0x27 => "DPAD_RIGHT", 0x26 => "DPAD_UP", 0x28 => "DPAD_DOWN",
            0x2D => "INSERT", 0x5D => "MENU", 0x6A => "NUMPAD_MULTIPLY", 0x6B => "NUMPAD_ADD",
            0x6C => "NUMPAD_COMMA", 0x6D => "NUMPAD_SUBTRACT", 0x6E => "NUMPAD_DOT", 0x6F => "NUMPAD_DIVIDE",
            0xBA => "SEMICOLON", 0xBB => "EQUALS", 0xBC => "COMMA", 0xBD => "MINUS",
            0xBE => "PERIOD", 0xBF => "SLASH", 0xC0 => "GRAVE", 0xDB => "LEFT_BRACKET",
            0xDC => "BACKSLASH", 0xDD => "RIGHT_BRACKET", 0xDE => "APOSTROPHE",
            _ => return None,
        }
        .into(),
    })
}

/// The Windows virtual-key code for a key name. Android exports portable names; the Win32 `VK_`
/// and Android `KEYCODE_` spellings are accepted too.
pub fn key_code(name: &str) -> Option<u16> {
    let name = name.trim().to_uppercase();
    let name = name.strip_prefix("KEYCODE_").unwrap_or(&name);
    let name = if name == "VK_MENU" { "ALT" } else { name.strip_prefix("VK_").unwrap_or(name) };
    if name.len() == 1 {
        let c = name.as_bytes()[0];
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            return Some(u16::from(c));
        }
    }
    if let Some(n) = name.strip_prefix('F').and_then(|n| n.parse::<u16>().ok()).filter(|n| (1..=24).contains(n)) {
        return Some(0x6F + n);
    }
    if let Some(n) = name.strip_prefix("NUMPAD_").and_then(|n| n.parse::<u16>().ok()).filter(|n| *n < 10) {
        return Some(0x60 + n);
    }
    Some(match name {
        "CTRL" | "CONTROL" | "LCONTROL" | "CTRL_LEFT" => 0xA2,
        "RCONTROL" | "CTRL_RIGHT" => 0xA3,
        "SHIFT" | "LSHIFT" | "SHIFT_LEFT" => 0xA0,
        "RSHIFT" | "SHIFT_RIGHT" => 0xA1,
        "ALT" | "LMENU" | "ALT_LEFT" => 0xA4,
        "RMENU" | "ALT_RIGHT" => 0xA5,
        "WIN" | "LWIN" | "META_LEFT" => 0x5B,
        "RWIN" | "META_RIGHT" => 0x5C,
        "ESC" | "ESCAPE" => 0x1B,
        "BACKSPACE" | "BACK" | "DEL" => 0x08,
        "DELETE" | "FORWARD_DEL" => 0x2E,
        "ENTER" | "RETURN" | "NUMPAD_ENTER" => 0x0D,
        "TAB" => 0x09, "SPACE" => 0x20, "CLEAR" => 0x0C,
        "HOME" | "MOVE_HOME" => 0x24, "END" | "MOVE_END" => 0x23,
        "CAPITAL" | "CAPS_LOCK" => 0x14,
        "NUMLOCK" | "NUM_LOCK" => 0x90, "SCROLL" | "SCROLL_LOCK" => 0x91,
        "SNAPSHOT" | "SYSRQ" | "PRINTSCREEN" | "SCREENSHOT" => 0x2C,
        "PRINT" => 0x2A,
        "PAUSE" | "BREAK" => 0x13,
        "PRIOR" | "PAGE_UP" => 0x21, "NEXT" | "PAGE_DOWN" => 0x22,
        "LEFT" | "DPAD_LEFT" => 0x25, "RIGHT" | "DPAD_RIGHT" => 0x27,
        "UP" | "DPAD_UP" => 0x26, "DOWN" | "DPAD_DOWN" => 0x28,
        "INSERT" => 0x2D, "MENU" => 0x5D,
        "NUMPAD_MULTIPLY" => 0x6A, "NUMPAD_ADD" => 0x6B, "NUMPAD_COMMA" => 0x6C,
        "NUMPAD_SUBTRACT" => 0x6D, "NUMPAD_DOT" => 0x6E, "NUMPAD_DIVIDE" => 0x6F,
        "SEMICOLON" => 0xBA, "EQUALS" | "PLUS" | "NUMPAD_EQUALS" => 0xBB, "COMMA" => 0xBC, "MINUS" => 0xBD,
        "PERIOD" => 0xBE, "SLASH" => 0xBF, "GRAVE" => 0xC0,
        "LEFT_BRACKET" => 0xDB, "BACKSLASH" => 0xDC, "RIGHT_BRACKET" => 0xDD, "APOSTROPHE" => 0xDE,
        _ => return None,
    })
}

/// How a key shows on a key cap and in a chord's text: `Ctrl`, `Esc`, `←`, `F4`, `A`.
pub fn key_label(name: &str) -> String {
    let Some(code) = key_code(name) else { return name.to_string() };
    let label = match code {
        0xA2 | 0xA3 => "Ctrl", 0xA0 | 0xA1 => "Shift", 0xA4 | 0xA5 => "Alt", 0x5B | 0x5C => "Win",
        0x1B => "Esc", 0x09 => "Tab", 0x0D => "Enter", 0x20 => "Space", 0x08 => "Backspace",
        0x2E => "Del", 0x2D => "Ins", 0x24 => "Home", 0x23 => "End", 0x21 => "PgUp", 0x22 => "PgDn",
        0x25 => "←", 0x27 => "→", 0x26 => "↑", 0x28 => "↓", 0x2C => "PrtSc", 0x14 => "Caps Lock",
        0x5D => "Menu", 0x13 => "Pause",
        _ => return key_name(code).replace("NUMPAD_", "Num "),
    };
    label.to_string()
}

/// A chord as people write it and as it is pressed, modifiers first: `Ctrl + Shift + Esc`. A
/// draft picked Caps Lock then Shift was named `Caps Lock + Shift` though it is saved and sent
/// as Shift + Caps Lock.
pub fn chord_label(keys: &[String]) -> String {
    let (modifiers, others): (Vec<&String>, Vec<&String>) = keys.iter().partition(|key| is_modifier(key));
    modifiers.into_iter().chain(others).map(|key| key_label(key)).collect::<Vec<_>>().join(" + ")
}

/// A chord the stream menu offers without the user saving it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuiltIn {
    /// Stable, for each client's own label.
    pub id: &'static str,
    pub keys: &'static [&'static str],
}

/// What a Windows PC needs most often from a controller: Task Manager first.
pub const BUILT_INS: &[BuiltIn] = &[
    BuiltIn { id: "task_manager", keys: &["CTRL", "SHIFT", "ESC"] },
    BuiltIn { id: "switch_window", keys: &["ALT", "TAB"] },
    BuiltIn { id: "close_window", keys: &["ALT", "F4"] },
    BuiltIn { id: "start_menu", keys: &["WIN"] },
    BuiltIn { id: "show_desktop", keys: &["WIN", "D"] },
    BuiltIn { id: "game_bar", keys: &["WIN", "G"] },
    // Windows' own Win + Alt + G: Win first, as Windows names it.
    BuiltIn { id: "record_that", keys: &["WIN", "ALT", "G"] },
];

/// The built-ins the user has not saved under a name of their own: a saved chord with the same
/// keys, in any order, replaces the built-in instead of showing twice. A key without a portable
/// name (an old Android shortcut's media key) still counts, so that chord matches no built-in.
pub fn built_ins(saved: &[Shortcut]) -> Vec<BuiltIn> {
    let chord = |keys: &mut dyn Iterator<Item = &str>| {
        let mut codes: Vec<Result<u16, String>> = keys.map(|key| key_code(key).ok_or_else(|| key.trim().to_uppercase())).collect();
        codes.sort_unstable();
        codes
    };
    let saved: Vec<_> = saved.iter().map(|s| chord(&mut s.keys.iter().map(String::as_str))).collect();
    BUILT_INS.iter().copied().filter(|b| !saved.contains(&chord(&mut b.keys.iter().copied()))).collect()
}

/// How long a sent chord stays held before its keys go up, in milliseconds. Pressed and released in
/// the same instant, a chord reaches the PC, but what reads the keys once a frame (the Steam
/// overlay's Shift + Tab, many games) never sees it held. Android holds its chords as long
/// (`GameKeyboardInput.SHORTCUT_HOLD_MS`).
pub const HOLD_MS: u64 = 100;

/// The keys the stream menu's key picker offers, in its order: modifiers first, then the keys
/// chords use most, arrows, letters, digits and function keys. A controller reaches every one
/// with the D-pad; nothing needs typing.
pub const PICKER: &[&str] = &[
    "CTRL", "SHIFT", "ALT", "WIN",
    "ESC", "TAB", "ENTER", "SPACE", "BACKSPACE", "DELETE", "SYSRQ", "MENU",
    "DPAD_UP", "DPAD_DOWN", "DPAD_LEFT", "DPAD_RIGHT", "HOME", "END", "PAGE_UP", "PAGE_DOWN", "INSERT", "CAPS_LOCK",
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M",
    "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z",
    "1", "2", "3", "4", "5", "6", "7", "8", "9", "0",
    "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
];

/// The modifier keys, which a chord holds while the others are pressed.
pub fn is_modifier(name: &str) -> bool {
    matches!(key_code(name), Some(0xA0..=0xA5 | 0x5B | 0x5C))
}

// Nightjar's own stream hotkeys: one table for both clients. Desktop Nightjar reads it directly
// (desktop/src/shortcuts.rs), the Android app through `StreamMenuModel` ([`crate::stream_menu::json`]).
// Each action has a default chord, may be changed to another of 2 to 4 keys or turned off, and a
// chord another action has is refused with that action named, on a change and on a reset alike. A
// bound chord never reaches the PC, so chords that type or edit text, and the ones Windows' shell
// keeps, are refused.

/// A Nightjar client.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Client {
    Desktop,
    Android,
}

/// Which clients have an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum On {
    Both,
    /// What only one client can do (a desktop window's full screen, Android's on-screen PC keyboard).
    Only(Client),
}

impl On {
    pub fn covers(self, client: Client) -> bool {
        self == On::Both || self == On::Only(client)
    }
}

/// One of Nightjar's own stream actions and its default chord.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hotkey {
    /// Stable: the saved bindings' and Android's preference key's name, and each client's label.
    pub id: &'static str,
    pub on: On,
    pub keys: &'static [&'static str],
    /// A client's earlier chord for the action, which still does it there while the action keeps
    /// its default. Kept for one release, with a notice the first time it is used.
    pub old: &'static [(Client, &'static [&'static str])],
}

const fn hotkey(id: &'static str, on: On, keys: &'static [&'static str]) -> Hotkey {
    Hotkey { id, on, keys, old: &[] }
}

const NIGHTJAR: [&str; 3] = ["CTRL", "ALT", "SHIFT"];

/// Every stream hotkey in the order the clients list them. Moonlight's meanings for Z, Q, M, S, X
/// and D; Tab opens the stream menu on both clients. Android's M (menu) and C (cursor) of earlier
/// releases stay there as old chords for one release.
pub const HOTKEYS: &[Hotkey] = &[
    Hotkey { old: &[(Client::Android, &["CTRL", "ALT", "SHIFT", "M"])], ..hotkey("stream_menu", On::Both, &["CTRL", "ALT", "SHIFT", "TAB"]) },
    hotkey("help", On::Only(Client::Desktop), &["CTRL", "ALT", "SHIFT", "H"]),
    hotkey("quit", On::Both, &["CTRL", "ALT", "SHIFT", "Q"]),
    hotkey("capture", On::Both, &["CTRL", "ALT", "SHIFT", "Z"]),
    hotkey("mouse_mode", On::Only(Client::Desktop), &["CTRL", "ALT", "SHIFT", "M"]),
    hotkey("overlay", On::Both, &["CTRL", "ALT", "SHIFT", "S"]),
    hotkey("send_clipboard", On::Only(Client::Desktop), &["CTRL", "ALT", "SHIFT", "V"]),
    hotkey("fetch_clipboard", On::Only(Client::Desktop), &["CTRL", "ALT", "SHIFT", "C"]),
    hotkey("fullscreen", On::Only(Client::Desktop), &["CTRL", "ALT", "SHIFT", "X"]),
    hotkey("minimize", On::Only(Client::Desktop), &["CTRL", "ALT", "SHIFT", "D"]),
    hotkey("pc_keyboard", On::Only(Client::Android), &["CTRL", "ALT", "SHIFT", "K"]),
    Hotkey { old: &[(Client::Android, &["CTRL", "ALT", "SHIFT", "C"])], ..hotkey("cursor", On::Only(Client::Android), &["CTRL", "ALT", "SHIFT", "P"]) },
];

/// The saved text of an action turned off.
pub const OFF: &str = "off";

/// Shortest and longest chord a hotkey may be changed to.
pub const HOTKEY_KEYS: std::ops::RangeInclusive<usize> = 2..=4;

/// The user's changes, by action ID: a chord's keys joined by `+` ([`binding_text`]) or [`OFF`].
/// An action without an entry has its default.
pub type HotkeyBindings = std::collections::BTreeMap<String, String>;

/// Why a hotkey's new chord was refused. Each client words it in its own language.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HotkeyError {
    UnknownAction(String),
    /// Not one of this client's actions.
    NotHere,
    /// Fewer than 2 or more than 4 keys.
    Length,
    /// Without Ctrl, Alt or Shift, or with the Windows key, which Windows keeps for itself.
    Modifier,
    /// Not exactly one key besides the modifiers.
    OneKey,
    UnsupportedKey(String),
    RepeatedKey(String),
    /// Ctrl+Alt+Del, which never reaches an app, and the chords Windows' shell keeps: Alt+Tab,
    /// Alt+Esc, Ctrl+Esc, Ctrl+Shift+Esc and their Shift and Ctrl forms.
    Reserved,
    /// A chord that types or edits text: Shift with a printable or editing key, Alt, Alt+Shift or
    /// Ctrl+Alt with a printable key (AltGr reaches Windows as Ctrl+Alt, Android as Alt), and
    /// Ctrl+C, Ctrl+V and the other editing chords.
    Typing,
    /// The last keyboard way out of a captured desktop stream: the stream menu's or Disconnect's.
    LastWayOut,
    /// Another action's chord: its ID.
    Taken(&'static str),
}

impl HotkeyError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownAction(_) => "unknown_action",
            Self::NotHere => "not_here",
            Self::Length => "hotkey_length",
            Self::Modifier => "hotkey_modifier",
            Self::OneKey => "hotkey_one_key",
            Self::UnsupportedKey(_) => "unsupported_key",
            Self::RepeatedKey(_) => "repeated_key",
            Self::Reserved => "reserved",
            Self::Typing => "hotkey_typing",
            Self::LastWayOut => "last_way_out",
            Self::Taken(_) => "taken",
        }
    }

    /// The key or action ID the error names, if any.
    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::UnknownAction(detail) | Self::UnsupportedKey(detail) | Self::RepeatedKey(detail) => Some(detail),
            Self::Taken(id) => Some(id),
            _ => None,
        }
    }
}

/// The hotkeys a client has, in list order.
pub fn hotkeys(client: Client) -> impl Iterator<Item = &'static Hotkey> {
    HOTKEYS.iter().filter(move |h| h.on.covers(client))
}

pub fn find_hotkey(id: &str) -> Option<&'static Hotkey> {
    HOTKEYS.iter().find(|h| h.id == id)
}

fn owned(keys: &[&str]) -> Vec<String> {
    keys.iter().map(|k| k.to_string()).collect()
}

/// Whether a key types a character: letters, digits, Space, punctuation and the numpad's.
fn printable(code: u16) -> bool {
    matches!(code, 0x20 | 0x30..=0x39 | 0x41..=0x5A | 0x60..=0x6F | 0xBA..=0xC0 | 0xDB..=0xDE)
}

/// Whether a key moves or changes text in a field: Tab, Enter, Backspace, Insert, Delete, the
/// arrows, Home, End and the page keys.
fn editing(code: u16) -> bool {
    matches!(code, 0x08 | 0x09 | 0x0D | 0x21..=0x28 | 0x2D | 0x2E)
}

/// Why a chord of `ctrl`, `alt`, `shift` and `other` may not be a hotkey, if it may not.
fn refused_chord(ctrl: bool, alt: bool, shift: bool, other: u16) -> Option<HotkeyError> {
    const TAB: u16 = 0x09;
    const ESC: u16 = 0x1B;
    let refused = match (ctrl, alt, shift) {
        (true, true, _) if other == 0x2E => HotkeyError::Reserved,
        // Alt+Tab, Alt+Shift+Tab, Ctrl+Alt+Tab, Alt+Esc, Alt+Shift+Esc; Ctrl+Esc and Ctrl+Shift+Esc.
        (_, true, false) | (false, true, true) if other == TAB => HotkeyError::Reserved,
        (false, true, _) if other == ESC => HotkeyError::Reserved,
        (true, false, _) if other == ESC => HotkeyError::Reserved,
        (false, false, true) if printable(other) || editing(other) => HotkeyError::Typing,
        // AltGr and Alt codes: Windows reports AltGr as Ctrl+Alt, Android as (right) Alt, so
        // AltGr+Shift (the capitals of AltGr's letters) reaches Android as Alt+Shift.
        (_, true, false) | (false, true, true) if printable(other) => HotkeyError::Typing,
        // Select all, copy, paste, cut, undo, redo, and moving or deleting by words.
        (true, false, false) if matches!(other, 0x41 | 0x43 | 0x56 | 0x58 | 0x59 | 0x5A) || (editing(other) && other != TAB) =>HotkeyError::Typing,
        (true, false, true) if matches!(other, 0x5A | 0x21..=0x28) => HotkeyError::Typing,
        _ => return None,
    };
    Some(refused)
}

/// A hotkey chord in its saved form: Ctrl, Alt and Shift first in that order (a right-hand one
/// counts as the left), then the one other key. A key alone keeps Nightjar's prefix: it becomes
/// Ctrl+Alt+Shift and that key. Refused unless it has 2 to 4 keys, at least one of Ctrl, Alt and
/// Shift, no Windows key and exactly one other key, and refused when it types or edits text or is
/// Windows' own ([`HotkeyError::Typing`], [`HotkeyError::Reserved`]).
pub fn hotkey_keys(keys: &[String]) -> Result<Vec<String>, HotkeyError> {
    let prefixed: Vec<String>;
    let keys = match keys {
        [key] if !is_modifier(key) => {
            prefixed = NIGHTJAR.iter().map(|k| k.to_string()).chain([key.clone()]).collect();
            &prefixed[..]
        }
        keys => keys,
    };
    if !HOTKEY_KEYS.contains(&keys.len()) {
        return Err(HotkeyError::Length);
    }
    let mut modifiers = [false; 3];
    let mut other: Option<u16> = None;
    for name in keys {
        let code = key_code(name).ok_or_else(|| HotkeyError::UnsupportedKey(name.clone()))?;
        let slot = match code {
            0xA2 | 0xA3 => Some(0),
            0xA4 | 0xA5 => Some(1),
            0xA0 | 0xA1 => Some(2),
            0x5B | 0x5C => return Err(HotkeyError::Modifier),
            _ => None,
        };
        match slot {
            Some(slot) if modifiers[slot] => return Err(HotkeyError::RepeatedKey(name.clone())),
            Some(slot) => modifiers[slot] = true,
            None if other == Some(code) => return Err(HotkeyError::RepeatedKey(name.clone())),
            None if other.is_some() => return Err(HotkeyError::OneKey),
            None => other = Some(code),
        }
    }
    let other = other.ok_or(HotkeyError::OneKey)?;
    if !modifiers.contains(&true) {
        return Err(HotkeyError::Modifier);
    }
    if let Some(refused) = refused_chord(modifiers[0], modifiers[1], modifiers[2], other) {
        return Err(refused);
    }
    let mut saved: Vec<String> = NIGHTJAR.iter().zip(modifiers).filter(|(_, held)| *held).map(|(name, _)| name.to_string()).collect();
    saved.push(key_name(other));
    Ok(saved)
}

/// How a binding is saved: the keys joined by `+`, or [`OFF`] for none.
pub fn binding_text(keys: Option<&[String]>) -> String {
    keys.map_or_else(|| OFF.to_string(), |keys| keys.join("+"))
}

/// What one action does on a client now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Bound {
    pub id: &'static str,
    /// Its chord in saved form; none while it is off.
    pub keys: Option<Vec<String>>,
    /// The user changed it: another chord, or off.
    pub changed: bool,
    /// Earlier chords that still do it ([`Hotkey::old`]).
    pub old: Vec<Vec<String>>,
    /// The action earlier in the list that has this action's chord too (a hand-edited file): this
    /// one is left without a chord until one of them changes.
    pub taken: Option<&'static str>,
}

/// Each of `client`'s hotkeys with the user's `bindings`, in list order. A saved binding that no
/// longer reads (a hand-edited file) leaves its action at the default, and a chord an action
/// earlier in the list has already leaves the later one without ([`Bound::taken`]). An old chord
/// stays only while its action has its default and no action's chord is the same. A desktop file
/// that leaves neither the stream menu nor Disconnect a chord (hand-edited) gives the stream menu
/// its default back, taken from whichever action has it, so a captured stream keeps a way out.
pub fn bound(client: Client, bindings: &HotkeyBindings) -> Vec<Bound> {
    let mut list = bound_as_saved(client, bindings);
    if client == Client::Desktop && !has_way_out(&list) {
        let default = owned(find_hotkey(WAYS_OUT[0]).expect("listed").keys);
        for b in list.iter_mut().filter(|b| b.keys.as_ref() == Some(&default)) {
            b.keys = None;
            b.taken = Some(WAYS_OUT[0]);
        }
        let menu = list.iter_mut().find(|b| b.id == WAYS_OUT[0]).expect("the desktop has a stream menu");
        (menu.keys, menu.changed, menu.taken) = (Some(default), false, None);
    }
    let taken: Vec<Vec<String>> = list.iter().filter_map(|b| b.keys.clone()).collect();
    for b in list.iter_mut().filter(|b| !b.changed) {
        let hotkey = find_hotkey(b.id).expect("listed");
        b.old = hotkey.old.iter().filter(|(on, _)| *on == client).map(|(_, keys)| owned(keys)).filter(|keys| !taken.contains(keys)).collect();
    }
    list
}

/// [`bound`] as the file has it: no old chords, and no way out given back.
fn bound_as_saved(client: Client, bindings: &HotkeyBindings) -> Vec<Bound> {
    let default = |h: &Hotkey| Some(owned(h.keys));
    let mut list: Vec<Bound> = hotkeys(client)
        .map(|h| {
            let saved = bindings.get(h.id).and_then(|text| match text.trim() {
                OFF => Some(None),
                text => hotkey_keys(&text.split('+').map(|k| k.trim().to_string()).collect::<Vec<_>>()).ok().map(Some),
            });
            let changed = saved.as_ref().is_some_and(|keys| *keys != default(h));
            Bound { id: h.id, keys: saved.unwrap_or_else(|| default(h)), changed, old: Vec::new(), taken: None }
        })
        .collect();
    for n in 1..list.len() {
        let (earlier, rest) = list.split_at_mut(n);
        let b = &mut rest[0];
        if let Some(first) = earlier.iter().find(|e| e.keys.is_some() && e.keys == b.keys) {
            b.taken = Some(first.id);
            b.keys = None;
        }
    }
    list
}

fn has_way_out(list: &[Bound]) -> bool {
    list.iter().any(|b| WAYS_OUT.contains(&b.id) && b.keys.is_some())
}

/// Checks `keys` as `id`'s new chord on `client`: its saved form, or why it is refused. A chord
/// another action has now is refused with that action named; an old chord ([`Hotkey::old`]) gives
/// way instead.
pub fn check_hotkey(client: Client, id: &str, keys: &[String], bindings: &HotkeyBindings) -> Result<Vec<String>, HotkeyError> {
    let hotkey = find_hotkey(id).ok_or_else(|| HotkeyError::UnknownAction(id.to_string()))?;
    if !hotkey.on.covers(client) {
        return Err(HotkeyError::NotHere);
    }
    let keys = hotkey_keys(keys)?;
    match bound(client, bindings).into_iter().find(|b| b.id != id && b.keys.as_ref() == Some(&keys)) {
        Some(other) => Err(HotkeyError::Taken(other.id)),
        None => Ok(keys),
    }
}

/// Checks that `id` may go back to its default chord on `client`: refused with the action named
/// while another action has that chord now, as a change to it would be.
pub fn check_reset(client: Client, id: &str, bindings: &HotkeyBindings) -> Result<(), HotkeyError> {
    let hotkey = find_hotkey(id).ok_or_else(|| HotkeyError::UnknownAction(id.to_string()))?;
    check_hotkey(client, id, &owned(hotkey.keys), bindings).map(drop)
}

/// The actions that leave a captured desktop stream from the keyboard. Android always has Back.
const WAYS_OUT: [&str; 2] = ["stream_menu", "quit"];

/// Checks that `id` may be turned off on `client`: the desktop keeps a chord for the stream menu
/// or Disconnect, so a captured stream always has a keyboard way out.
pub fn check_off(client: Client, id: &str, bindings: &HotkeyBindings) -> Result<(), HotkeyError> {
    let hotkey = find_hotkey(id).ok_or_else(|| HotkeyError::UnknownAction(id.to_string()))?;
    if !hotkey.on.covers(client) {
        return Err(HotkeyError::NotHere);
    }
    if client == Client::Desktop && WAYS_OUT.contains(&id) {
        let mut after = bindings.clone();
        after.insert(id.to_string(), OFF.to_string());
        if !has_way_out(&bound_as_saved(client, &after)) {
            return Err(HotkeyError::LastWayOut);
        }
    }
    Ok(())
}

/// The modifiers of a chord in saved form, in the input packets' bits (Shift 1, Ctrl 2, Alt 4),
/// and its other key's Windows virtual-key code.
pub fn hotkey_code(keys: &[String]) -> (u8, u16) {
    keys.iter().fold((0, 0), |(modifiers, vk), key| match key_code(key) {
        Some(0xA0 | 0xA1) => (modifiers | 0x01, vk),
        Some(0xA2 | 0xA3) => (modifiers | 0x02, vk),
        Some(0xA4 | 0xA5) => (modifiers | 0x04, vk),
        Some(code) => (modifiers, code),
        None => (modifiers, vk),
    })
}

/// The action a key press does, and whether it was an old chord: `modifiers` held in the input
/// packets' bits (Shift 1, Ctrl 2, Alt 4, Windows 8) and exactly those, `vk` the pressed key's
/// Windows virtual-key code.
pub fn hotkey_at(bound: &[Bound], modifiers: u8, vk: u16) -> Option<(&'static str, bool)> {
    let hit = |keys: &Vec<String>| hotkey_code(keys) == (modifiers & 0x0F, vk);
    bound.iter().find(|b| b.keys.as_ref().is_some_and(hit)).map(|b| (b.id, false))
        .or_else(|| bound.iter().find(|b| b.old.iter().any(hit)).map(|b| (b.id, true)))
}

#[cfg(test)]
mod tests;
