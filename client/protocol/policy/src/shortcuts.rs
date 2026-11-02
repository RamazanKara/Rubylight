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
    match code {
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
            _ => unreachable!("validated shortcut key"),
        }
        .into(),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn android_document_round_trips_and_invalid_chords_do_not_replace_entries() {
        let text = r#"{"version":1,"data":[{"id":"desktop","name":"Task view","keys":["WIN","TAB"]}]}"#;
        let mut shortcuts = Shortcuts::parse(text).unwrap();
        assert_eq!(Shortcuts::parse(&shortcuts.encode()).unwrap(), shortcuts);
        let before = shortcuts.clone();
        assert_eq!(shortcuts.save_chord(Some("desktop"), "Bad", "CTRL + CTRL_LEFT", "x"), Err(ShortcutError::RepeatedKey("CTRL_LEFT".into())));
        assert_eq!(shortcuts, before);
        assert_eq!(Shortcuts::parse(&text.replace("\"TAB\"", "\"UNKNOWN\"")), Err(ShortcutError::UnsupportedKey("UNKNOWN".into())));
        assert_eq!(Shortcuts::parse(&text.replace("\"version\":1", "\"version\":2")), Err(ShortcutError::Version));
        let entry = serde_json::to_string(&shortcuts.data[0]).unwrap();
        assert_eq!(Shortcuts::parse(&format!(r#"{{"data":[{entry},{entry}]}}"#)), Err(ShortcutError::BadId));
        assert_eq!(key_code("KEYCODE_F24"), Some(0x87));
        assert_eq!(key_code("VK_MENU"), Some(0xA4));
        assert_eq!(key_code("NUMPAD_9"), Some(0x69));
        assert_eq!(key_code("PRINT"), Some(0x2A));
        assert_eq!(key_code("SCREENSHOT"), Some(0x2C));
        shortcuts.save_chord(None, "Portable aliases", "VK_LCONTROL + PRINTSCREEN + F01", "desktop").unwrap();
        assert_eq!(shortcuts.data[1].keys, ["CTRL", "SYSRQ", "F1"]);
        assert_eq!(shortcuts.data[1].id, "desktop-1");
        for code in (0x30..=0x39).chain(0x41..=0x5A).chain(0x60..=0x87) {
            assert_eq!(key_code(&key_name(code)), Some(code));
        }
    }

    #[test]
    fn a_shortcut_added_in_the_stream_gets_a_free_id_and_the_limit_holds() {
        let mut shortcuts = Shortcuts::default();
        let keys = |k: &[&str]| k.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(shortcuts.save(None, " Task view ", &keys(&["WIN", "TAB"]), "android").unwrap(), "android-1");
        assert_eq!(shortcuts.data[0].name, "Task view");
        assert_eq!(shortcuts.save(None, "Snip", &keys(&["WIN", "SHIFT", "S"]), "android").unwrap(), "android-2");
        shortcuts.remove("android-1").unwrap();
        assert_eq!(shortcuts.save(None, "Again", &keys(&["F5"]), "android").unwrap(), "android-1");
        assert_eq!(shortcuts.remove("android-9"), Err(ShortcutError::Gone));
        assert_eq!(shortcuts.save(Some("gone"), "x", &keys(&["A"]), "android"), Err(ShortcutError::Gone));
        assert_eq!(shortcuts.save(None, "", &keys(&["A"]), "android"), Err(ShortcutError::BadName));
        assert_eq!(shortcuts.save(None, "x", &[], "android"), Err(ShortcutError::ChordLength));
        while shortcuts.data.len() < MAX_SHORTCUTS {
            let n = shortcuts.data.len();
            shortcuts.save(None, &format!("s{n}"), &keys(&["F1"]), "android").unwrap();
        }
        let full = shortcuts.clone();
        assert_eq!(shortcuts.save(None, "one more", &keys(&["F2"]), "android"), Err(ShortcutError::TooMany));
        assert_eq!(shortcuts, full);
        assert_eq!(Shortcuts::parse(&shortcuts.encode()).unwrap(), shortcuts);
    }

    #[test]
    fn a_saved_chord_replaces_the_built_in_with_the_same_keys() {
        assert_eq!(built_ins(&[]).len(), BUILT_INS.len());
        let saved = [Shortcut { id: "a".into(), name: "Taskmgr".into(), keys: vec!["ESC".into(), "SHIFT".into(), "CTRL".into()] }];
        let ids: Vec<_> = built_ins(&saved).iter().map(|b| b.id).collect();
        assert_eq!(ids, ["switch_window", "close_window", "start_menu", "show_desktop"]);
        for built_in in BUILT_INS {
            let keys: Vec<String> = built_in.keys.iter().map(|k| k.to_string()).collect();
            assert_eq!(normalize_keys(&keys).unwrap(), keys, "{}", built_in.id);
        }
    }

    /// Android's editor hands over keys in the order they were tapped: Esc, Ctrl, Shift sent Esc alone,
    /// then Ctrl + Shift. Both clients send what this returns.
    #[test]
    fn modifiers_go_first_whatever_order_the_keys_were_picked_in() {
        let keys = |k: &[&str]| k.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(normalize_keys(&keys(&["ESC", "CTRL", "SHIFT"])).unwrap(), ["CTRL", "SHIFT", "ESC"]);
        assert_eq!(normalize_keys(&keys(&["F4", "ALT"])).unwrap(), ["ALT", "F4"]);
        // Stable: modifiers keep their own order, and so do the other keys.
        assert_eq!(normalize_keys(&keys(&["S", "SHIFT", "WIN"])).unwrap(), ["SHIFT", "WIN", "S"]);
        assert_eq!(normalize_keys(&keys(&["A", "CTRL", "B"])).unwrap(), ["CTRL", "A", "B"]);
        let mut shortcuts = Shortcuts::default();
        shortcuts.save(None, "Task Manager", &keys(&["ESC", "CTRL", "SHIFT"]), "android").unwrap();
        assert_eq!(shortcuts.data[0].keys, ["CTRL", "SHIFT", "ESC"]);
        let file = r#"{"version":1,"data":[{"id":"a","name":"Old","keys":["ESC","SHIFT","CTRL"]}]}"#;
        assert_eq!(Shortcuts::parse(file).unwrap().data[0].keys, ["SHIFT", "CTRL", "ESC"]);
    }

    /// An old Android shortcut can hold a key without a portable name (a media key); it is no built-in.
    #[test]
    fn a_saved_chord_with_an_unknown_key_does_not_hide_a_built_in() {
        let saved = [Shortcut { id: "a".into(), name: "Old".into(), keys: vec!["CTRL".into(), "SHIFT".into(), "ESC".into(), "MEDIA_PLAY_PAUSE".into()] }];
        assert_eq!(built_ins(&saved).len(), BUILT_INS.len());
        let saved = [Shortcut { id: "a".into(), name: "Old".into(), keys: vec!["ANDROID_KEYCODE_300".into()] }];
        assert_eq!(built_ins(&saved).len(), BUILT_INS.len());
        assert_eq!(normalize_keys(&["CTRL".into(), "ANDROID_KEYCODE_300".into()]), Err(ShortcutError::UnsupportedKey("ANDROID_KEYCODE_300".into())));
    }

    #[test]
    fn every_picker_key_is_a_saved_name_with_a_label() {
        let mut seen = HashSet::new();
        for key in PICKER {
            assert!(seen.insert(*key), "{key} twice");
            let code = key_code(key).unwrap_or_else(|| panic!("{key} unsupported"));
            assert_eq!(key_name(code), *key);
            assert!(!key_label(key).is_empty());
        }
        assert!(PICKER[..4].iter().all(|k| is_modifier(k)));
        assert!(!PICKER[4..].iter().any(|k| is_modifier(k)));
        assert_eq!(chord_label(&["CTRL".into(), "SHIFT".into(), "ESC".into()]), "Ctrl + Shift + Esc");
        assert_eq!(chord_label(&["WIN".into(), "DPAD_LEFT".into()]), "Win + ←");
        assert_eq!(chord_label(&["CAPS_LOCK".into(), "SHIFT".into()]), "Shift + Caps Lock");
        assert_eq!(chord_label(&["ESC".into(), "SHIFT".into(), "CTRL".into()]), "Shift + Ctrl + Esc");
        assert_eq!(key_label("NUMPAD_4"), "Num 4");
    }
}
