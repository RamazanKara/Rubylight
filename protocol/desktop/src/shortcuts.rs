use std::collections::HashSet;
use crate::strings::{tr, trf};
use std::io::Read;
use std::path::Path;

use nightjar_core::input::{InputError, InputQueue};
use nightjar_core::input::packets::modifier;
use serde::{Deserialize, Serialize};

const MAX_BYTES: usize = 64 * 1024;

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

fn version() -> u8 { 1 }

impl Default for Shortcuts {
    fn default() -> Self { Self { version: 1, data: Vec::new() } }
}

impl Shortcuts {
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > MAX_BYTES { return Err(tr("Shortcut file is larger than 64 KiB.").into()); }
        let mut entries: Self = serde_json::from_str(text).map_err(|e| trf("Invalid shortcut file: {}", &[&e]))?;
        if entries.version != 1 { return Err(tr("Unsupported shortcut file version.").into()); }
        if entries.data.len() > 32 { return Err(tr("You can save up to 32 shortcuts.").into()); }
        let mut ids = HashSet::new();
        for entry in &mut entries.data {
            if entry.id.is_empty() || entry.id.len() > 64 || !entry.id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                || !ids.insert(entry.id.clone()) {
                return Err(tr("Invalid or repeated shortcut ID.").into());
            }
            validate_name(&entry.name)?;
            entry.keys = normalize_keys(&entry.keys)?;
            entry.name = entry.name.trim().into();
        }
        Ok(entries)
    }

    pub fn read(path: &Path) -> Result<Self, String> {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let mut text = String::new();
        file.take((MAX_BYTES + 1) as u64).read_to_string(&mut text).map_err(|e| e.to_string())?;
        Self::parse(&text)
    }

    pub fn encode(&self) -> String { serde_json::to_string_pretty(self).expect("shortcuts serialize") }

    pub fn save(&mut self, id: Option<&str>, name: &str, chord: &str) -> Result<(), String> {
        validate_name(name)?;
        if chord.len() > 256 { return Err(tr("Use a chord of at most 256 characters.").into()); }
        let keys: Vec<_> = chord.split('+').map(|key| key.trim().to_uppercase()).collect();
        let keys = normalize_keys(&keys)?;
        if let Some(id) = id {
            let entry = self.data.iter_mut().find(|e| e.id == id).ok_or(tr("Shortcut no longer exists."))?;
            entry.name = name.trim().into();
            entry.keys = keys;
        } else {
            if self.data.len() >= 32 { return Err(tr("You can save up to 32 shortcuts.").into()); }
            let id = (1..).map(|i| format!("desktop-{i}")).find(|id| !self.data.iter().any(|e| &e.id == id)).unwrap();
            self.data.push(Shortcut { id, name: name.trim().into(), keys });
        }
        Ok(())
    }
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        Err(tr("Use a name between 1 and 80 characters without control characters.").into())
    } else { Ok(()) }
}

fn normalize_keys(keys: &[String]) -> Result<Vec<String>, String> {
    if keys.is_empty() || keys.len() > 8 { return Err(tr("Use between 1 and 8 keys in each chord.").into()); }
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for name in keys {
        let code = key_code(name).ok_or_else(|| trf("Unsupported key: {}", &[&name]))?;
        if !seen.insert(code) { return Err(trf("Repeated key: {}", &[&name])); }
        normalized.push(key_name(code));
    }
    Ok(normalized)
}

fn key_name(code: u16) -> String {
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
        }.into(),
    }
}

// Android exports these portable names instead of platform key codes.
pub fn key_code(name: &str) -> Option<u16> {
    let name = name.trim().to_uppercase();
    let name = name.strip_prefix("KEYCODE_").unwrap_or(&name);
    let name = if name == "VK_MENU" { "ALT" } else { name.strip_prefix("VK_").unwrap_or(name) };
    if name.len() == 1 {
        let c = name.as_bytes()[0];
        if c.is_ascii_uppercase() || c.is_ascii_digit() { return Some(u16::from(c)); }
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

fn modifier(code: u16) -> u8 {
    match code {
        0xA0 | 0xA1 => modifier::SHIFT, 0xA2 | 0xA3 => modifier::CTRL,
        0xA4 | 0xA5 => modifier::ALT, 0x5B | 0x5C => modifier::META,
        _ => 0,
    }
}

pub fn send(keys: &[String], held: &HashSet<u16>, queue: &mut InputQueue) -> Result<(), InputError> {
    let mut keys: Vec<_> = keys.iter().map(|name| key_code(name).ok_or(InputError::Invalid)).collect::<Result<_, _>>()?;
    keys.retain(|key| !held.contains(key));
    keys.sort_by_key(|code| modifier(*code) == 0);
    if queue.is_holding() { return Err(InputError::Stopped); }
    // Reserve releases too: the session holds the queue lock throughout the chord.
    if queue.len() + keys.len() * 2 > nightjar_core::input::queue::MAX_QUEUED { return Err(InputError::QueueFull); }
    let held_modifiers = held.iter().fold(0, |mask, code| mask | modifier(*code));
    let mut pressed = Vec::new();
    for key in keys {
        pressed.push(key);
        let modifiers = pressed.iter().fold(held_modifiers, |mask, code| mask | modifier(*code));
        queue.send_keyboard(0x8000 | key, true, modifiers, 0)?;
    }
    while let Some(key) = pressed.pop() {
        let modifiers = pressed.iter().fold(held_modifiers, |mask, code| mask | modifier(*code));
        queue.send_keyboard(0x8000 | key, false, modifiers, 0)?;
    }
    Ok(())
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
        assert!(shortcuts.save(Some("desktop"), "Bad", "CTRL + CTRL_LEFT").is_err());
        assert_eq!(shortcuts, before);
        assert!(Shortcuts::parse(&text.replace("\"TAB\"", "\"UNKNOWN\"")).is_err());
        assert!(Shortcuts::parse(&text.replace("\"version\":1", "\"version\":2")).is_err());
        assert!(Shortcuts::parse(&format!(r#"{{"data":[{},{}]}}"#, serde_json::to_string(&shortcuts.data[0]).unwrap(), serde_json::to_string(&shortcuts.data[0]).unwrap())).is_err());
        assert_eq!(key_code("KEYCODE_F24"), Some(0x87));
        assert_eq!(key_code("VK_MENU"), Some(0xA4));
        assert_eq!(key_code("NUMPAD_9"), Some(0x69));
        assert_eq!(key_code("PRINT"), Some(0x2A));
        assert_eq!(key_code("SCREENSHOT"), Some(0x2C));
        shortcuts.save(None, "Portable aliases", "VK_LCONTROL + PRINTSCREEN + F01").unwrap();
        assert_eq!(shortcuts.data[1].keys, ["CTRL", "SYSRQ", "F1"]);
        for code in (0x30..=0x39).chain(0x41..=0x5A).chain(0x60..=0x87) {
            assert_eq!(key_code(&key_name(code)), Some(code));
        }
    }

    #[test]
    fn chords_keep_held_keys_and_reject_a_full_queue_before_pressing_anything() {
        use nightjar_core::input::InputEvent;
        let caps = nightjar_core::control::HostCapabilities { app_version: [7, 1, 0, -1], ..Default::default() };
        let mut queue = InputQueue::new(&caps);
        let mut packets = Vec::new();
        queue.drain(std::time::Instant::now(), false, &mut packets);
        packets.clear();
        let held = HashSet::from([0xA2]);
        send(&["C".into(), "CTRL".into()], &held, &mut queue).unwrap();
        queue.drain(std::time::Instant::now(), false, &mut packets);
        let keys: Vec<_> = packets.iter().map(|p| p.data.clone()).collect();
        assert_eq!(keys, vec![
            InputEvent::Keyboard { key_code: 0x8043, down: true, modifiers: modifier::CTRL, flags: 0 }.encode(true),
            InputEvent::Keyboard { key_code: 0x8043, down: false, modifiers: modifier::CTRL, flags: 0 }.encode(true),
        ]);
        packets.clear();
        send(&["TAB".into(), "ALT".into(), "CTRL".into()], &HashSet::new(), &mut queue).unwrap();
        queue.drain(std::time::Instant::now(), false, &mut packets);
        let expected = [(0xA4, true, 4), (0xA2, true, 6), (0x09, true, 6), (0x09, false, 6), (0xA2, false, 4), (0xA4, false, 0)];
        assert_eq!(packets.iter().map(|p| p.data.clone()).collect::<Vec<_>>(), expected.iter().map(|&(key, down, modifiers)| {
            InputEvent::Keyboard { key_code: 0x8000 | key, down, modifiers, flags: 0 }.encode(true)
        }).collect::<Vec<_>>());
        while queue.len() < nightjar_core::input::queue::MAX_QUEUED - 1 {
            queue.send_keyboard(0x8041, true, 0, 0).unwrap();
        }
        let before = queue.len();
        assert_eq!(send(&["ALT".into(), "TAB".into()], &HashSet::new(), &mut queue), Err(InputError::QueueFull));
        assert_eq!(queue.len(), before);
    }
}
