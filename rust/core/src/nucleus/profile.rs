//! The Nucleus Co-op game profile the host writes for a session: one player
//! per slot, each with its own virtual pad by XInput index, in the format
//! Nucleus 2.4's `GameProfile.LoadGameProfile` reads.
//!
//! Nucleus numbers a game's profiles by position (`Profile[1].json`,
//! `Profile[2].json`, ...) and loads the n-th listed as `Profile[n].json`,
//! so the host keeps one profile titled [`TITLE`] in that sequence.
use super::window::Rect;
use serde_json::{Value, json};

pub const TITLE: &str = "Rubylight";

/// A player's ideal processor when none is set, as Nucleus 2.4.2 writes it
/// (`PlayerInfo.IdealProcessor`). `GenericGameHandler.Play` parses any
/// other value with `int.Parse`, so an empty one fails the start with a
/// FormatException after the game's process is up.
const IDEAL_PROCESSOR_ANY: &str = "*";

/// One player of the profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Player {
    /// The XInput user index of the player's virtual pad.
    pub xinput: u8,
    /// Where Nucleus first puts the instance's window.
    pub bounds: Rect,
}

/// Nucleus' name for the XInput pad at `index` when it assigns pads by
/// index (`DevicesFunctions.cs`).
pub fn gamepad_guid(index: u8) -> String {
    format!("00000000-0000-0000-0000-2{:011}", u32::from(index) + 1)
}

fn rect(r: Rect) -> Value {
    json!({ "X": r.x, "Y": r.y, "Width": r.width, "Height": r.height })
}

/// The profile for `players` on a PC with `screens` (Nucleus checks the
/// players' screens against its own list). Only a handler with steps of its
/// own needs one that `plays_itself`: Nucleus 2.4.2 decides that while it
/// paints its setup screen (`GameProfile.FindProfilePlayers`) and clears the
/// flag only after the start, so a paint during the start starts the game a
/// second time. About half of Black Flag's starts did, and each of those
/// looked like the game refusing the hooks or exiting by itself (#59). Without
/// it, Rubylight presses Play, once.
pub fn build(players: &[Player], screens: &[Rect], plays_itself: bool) -> Value {
    let screen_of = |bounds: Rect| {
        screens
            .iter()
            .position(|s| {
                i64::from(bounds.x) >= i64::from(s.x)
                    && i64::from(bounds.x) < s.right()
                    && i64::from(bounds.y) >= i64::from(s.y)
                    && i64::from(bounds.y) < s.bottom()
            })
            .unwrap_or(0)
    };
    let data: Vec<Value> = players
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let screen = screen_of(p.bounds);
            let display = screens.get(screen).copied().unwrap_or(p.bounds);
            json!({
                "PlayerID": i,
                "Nickname": format!("Player{}", i + 1),
                "SteamID": -1,
                "GamepadGuid": gamepad_guid(p.xinput),
                "IsDInput": false,
                "IsXInput": true,
                "IsSDL2": false,
                "Guests": { "GUIDS": "" },
                "Processor": {
                    "IdealProcessor": IDEAL_PROCESSOR_ANY,
                    "ProcessorAffinity": "",
                    "ProcessorPriorityClass": "Normal"
                },
                "IsKeyboardPlayer": false,
                "IsRawMouse": false,
                "HIDDeviceID": ["Not required", "Not required"],
                "ScreenPriority": 0,
                "ScreenIndex": screen,
                "EditBounds": rect(p.bounds),
                "MonitorBounds": rect(p.bounds),
                "Owner": {
                    "Type": 0,
                    "UiBounds": rect(display),
                    "DisplayIndex": screen,
                    "Display": rect(display)
                }
            })
        })
        .collect();
    json!({
        "Title": TITLE,
        "Notes": "Written by Rubylight for streaming; it is rewritten each time.",
        "Player(s)": players.len(),
        "Controller(s)": players.len(),
        "Use XInput Index": true,
        "K&M": 0,
        "AutoPlay": { "Enabled": plays_itself },
        "Data": data,
        "Options": {},
        "UseNicknames": { "Use": false },
        // Both would change the PC user's desktop: a backdrop window over
        // the monitors, and the Windows display scale.
        "AutoDesktopScaling": { "Enabled": false },
        "UseSplitDiv": { "Enabled": false, "HideOnly": false, "Color": "Black" },
        "CustomLayout": { "Ver": 0, "Hor": 0, "Max": players.len() },
        "WindowsSetupTiming": { "Time": "0" },
        "PauseBetweenInstanceLaunch": { "Time": "0" },
        "Network": { "Type": "Automatic" },
        "AudioSettings": { "CustomSettings": false, "DefaultSettings": true },
        "AudioInstances": {},
        "CutscenesModeSettings": {
            "Cutscenes_KeepAspectRatio": false,
            "Cutscenes_MuteAudioOnly": false,
            "Cutscenes_Unfocus": false
        },
        "StopGPUINav": { "Enabled": false },
        "EnableWindowsMerger": { "Enabled": false },
        "MergerResolution": "",
        "EnableLosslessHook": { "Enabled": false },
        "AllScreens": screens.iter().copied().map(rect).collect::<Vec<_>>()
    })
}

/// The profile number for the host's profile among a game's profile files,
/// given as (number, title) in Nucleus' order: the host's own if one is
/// there, else the next number.
pub fn number(existing: &[(u32, Option<String>)]) -> u32 {
    existing
        .iter()
        .find(|(_, title)| title.as_deref() == Some(TITLE))
        .map(|(n, _)| *n)
        .unwrap_or(existing.len() as u32 + 1)
}

/// The number in a profile file's name, `Profile[n].json`.
pub fn file_number(name: &str) -> Option<u32> {
    name.strip_prefix("Profile[")?
        .strip_suffix("].json")?
        .parse()
        .ok()
}

/// Nucleus Co-op 2.4.2's `Settings.ini` as its release ships it (the pinned
/// `NucleusApp.zip`'s, line ends aside). Nucleus stops with a message box
/// when a key it reads is missing (`App_Settings_Loader`), so the host
/// completes a file from this.
pub const DEFAULT_SETTINGS: &str = include_str!("settings-2.4.2.ini");

/// What the host's profile needs in Nucleus' `Settings.ini`: pads assigned
/// by XInput index, which the profile's pad names are, and game profiles
/// on, or Nucleus would not read the profile. Nucleus' check of its own
/// folder and of a Documents folder in OneDrive shows a message box before
/// its window opens, and its reminder of the End key one before each game
/// starts (`GenericGameHandler.Play`); no one answers either in a
/// background stream. Its favourites-only list would hide the host's game
/// (#59).
const FORCED: [(&str, &str, &str); 5] = [
    ("Dev", "UseXinputIndex", "True"),
    ("Misc", "DisableGameProfiles", "False"),
    ("Dev", "DisablePathCheck", "True"),
    ("Dev", "ShowFavoriteOnly", "False"),
    ("Misc", "IgnoreInputLockReminder", "True"),
];

/// The forced keys a session puts back when it ends: how the user set up
/// Nucleus' own window. Pads by index and game profiles on stay set.
const PUT_BACK: [&str; 4] = [
    "DisablePathCheck",
    "ShowFavoriteOnly",
    "IgnoreInputLockReminder",
    "GameSortingOpt",
];

/// Nucleus' list filters in `[Misc] GameSortingOpt` (`SortOptionsPanel.cs`):
/// only games with gamepad hooks, keyboard or several keyboards. Each would
/// hide the host's own handler, which has none of them; the `Sort_*`
/// orders stay.
const LIST_FILTERS: [&[u8]; 3] = [b"Show_Type1", b"Show_Type2", b"Show_Type3"];

/// An INI file as bytes: Nucleus writes it through kernel32's ANSI calls,
/// so a value may be in the PC's code page. Names are matched as ASCII in
/// any case; every other byte, the BOM and the line ends stay.
struct Ini {
    bom: bool,
    /// Each line's text and its end.
    lines: Vec<(Vec<u8>, Vec<u8>)>,
    /// The end a new line gets: the file's first, else CRLF as kernel32 writes.
    end: Vec<u8>,
}

const BOM: &[u8] = b"\xef\xbb\xbf";

impl Ini {
    fn parse(bytes: &[u8]) -> Ini {
        let (bom, mut rest) = match bytes.strip_prefix(BOM) {
            Some(rest) => (true, rest),
            None => (false, bytes),
        };
        let mut lines = Vec::new();
        while !rest.is_empty() {
            let (line, next) = match rest.iter().position(|&b| b == b'\n') {
                Some(at) => rest.split_at(at + 1),
                None => (rest, &[][..]),
            };
            let text_len = line
                .strip_suffix(b"\r\n")
                .or_else(|| line.strip_suffix(b"\n"))
                .map_or(line.len(), <[u8]>::len);
            lines.push((line[..text_len].to_vec(), line[text_len..].to_vec()));
            rest = next;
        }
        let end = lines
            .iter()
            .map(|(_, end)| end)
            .find(|end| !end.is_empty())
            .cloned()
            .unwrap_or_else(|| b"\r\n".to_vec());
        Ini { bom, lines, end }
    }

    fn bytes(&self) -> Vec<u8> {
        let mut bytes = if self.bom { BOM.to_vec() } else { Vec::new() };
        for (text, end) in &self.lines {
            bytes.extend_from_slice(text);
            bytes.extend_from_slice(end);
        }
        bytes
    }

    fn header(text: &[u8]) -> Option<&[u8]> {
        text.trim_ascii().strip_prefix(b"[")?.strip_suffix(b"]")
    }

    fn key(text: &[u8]) -> Option<&[u8]> {
        let at = text.iter().position(|&b| b == b'=')?;
        Some(text[..at].trim_ascii())
    }

    /// The line of `key` in `section` and where the section ends, as
    /// `GetPrivateProfileString` finds them: the first section of that
    /// name. None without the section.
    fn find(&self, section: &str, key: &str) -> Option<(Option<usize>, usize)> {
        let start = self.lines.iter().position(|(text, _)| {
            Self::header(text).is_some_and(|name| name.eq_ignore_ascii_case(section.as_bytes()))
        })?;
        let end = self.lines[start + 1..]
            .iter()
            .position(|(text, _)| text.trim_ascii().starts_with(b"["))
            .map_or(self.lines.len(), |i| start + 1 + i);
        let existing = (start + 1..end).find(|&i| {
            Self::key(&self.lines[i].0).is_some_and(|k| k.eq_ignore_ascii_case(key.as_bytes()))
        });
        Some((existing, end))
    }

    fn get(&self, section: &str, key: &str) -> Option<&[u8]> {
        let text = &self.lines[self.find(section, key)?.0?].0;
        let at = text.iter().position(|&b| b == b'=')?;
        Some(text[at + 1..].trim_ascii())
    }

    fn push(&mut self, at: usize, text: Vec<u8>) {
        // A last line without an end gets one before a line follows it.
        if at > 0 && self.lines[at - 1].1.is_empty() {
            self.lines[at - 1].1 = self.end.clone();
        }
        let end = self.end.clone();
        self.lines.insert(at, (text, end));
    }

    fn set(&mut self, section: &str, key: &str, value: &[u8]) {
        let mut entry = format!("{key}=").into_bytes();
        entry.extend_from_slice(value);
        match self.find(section, key) {
            Some((Some(i), _)) => self.lines[i].0 = entry,
            Some((None, end)) => self.push(end, entry),
            None => {
                let at = self.lines.len();
                self.push(at, format!("[{section}]").into_bytes());
                self.push(at + 1, entry);
            }
        }
    }

    /// Every (section, key, value), in order.
    fn entries(&self) -> Vec<(String, String, Vec<u8>)> {
        let mut section = String::new();
        let mut entries = Vec::new();
        for (text, _) in &self.lines {
            if let Some(name) = Self::header(text) {
                section = String::from_utf8_lossy(name).into_owned();
            } else if let Some(key) = Self::key(text) {
                let at = text.iter().position(|&b| b == b'=').unwrap_or(text.len());
                entries.push((
                    section.clone(),
                    String::from_utf8_lossy(key).into_owned(),
                    text[at + 1..].trim_ascii().to_vec(),
                ));
            }
        }
        entries
    }
}

/// The value of `key` in `section` of an INI file, as Nucleus reads it.
pub fn ini_value(text: &[u8], section: &str, key: &str) -> Option<Vec<u8>> {
    Ini::parse(text).get(section, key).map(<[u8]>::to_vec)
}

/// An INI file with each (section, key, value) of `values` set; other bytes
/// stay as they are.
pub fn ini_set(text: &[u8], values: &[(&str, &str, &str)]) -> Vec<u8> {
    let mut ini = Ini::parse(text);
    for &(section, key, value) in values {
        ini.set(section, key, value.as_bytes());
    }
    ini.bytes()
}

/// A value a session changed in `Settings.ini` and puts back when it ends.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Forced {
    pub section: String,
    pub key: String,
    /// The user's value.
    pub before: Vec<u8>,
    /// The session's.
    pub forced: Vec<u8>,
}

/// [`settings_ini`]'s result.
#[derive(Debug, PartialEq, Eq)]
pub struct Settings {
    pub text: Vec<u8>,
    /// The keys completed from [`DEFAULT_SETTINGS`], as `Section.Key`.
    pub added: Vec<String>,
    /// What the session puts back ([`PUT_BACK`]).
    pub forced: Vec<Forced>,
}

/// Whether a `Settings.ini` is UTF-16 text (its byte order mark): the
/// host edits ANSI text, and would add ASCII keys beside the file's own.
pub fn utf16_ini(text: &[u8]) -> bool {
    text.starts_with(&[0xff, 0xfe]) || text.starts_with(&[0xfe, 0xff])
}

/// Nucleus' `Settings.ini` (empty when there is none) completed from
/// [`DEFAULT_SETTINGS`], with [`FORCED`] set and [`LIST_FILTERS`] off;
/// every other byte stays. A repaired key stays: it is no choice of the
/// user's. Another Nucleus release (`nucleus_path`) ignores 2.4.2's keys it
/// doesn't read.
pub fn settings_ini(text: &[u8]) -> Settings {
    let mut ini = Ini::parse(text);
    let mut added = Vec::new();
    for (section, key, value) in Ini::parse(DEFAULT_SETTINGS.as_bytes()).entries() {
        if ini.get(&section, &key).is_none() {
            ini.set(&section, &key, &value);
            added.push(format!("{section}.{key}"));
        }
    }
    let mut changes: Vec<(&str, &str, Vec<u8>)> = FORCED
        .iter()
        .map(|&(section, key, value)| (section, key, value.as_bytes().to_vec()))
        .collect();
    // Nucleus splits the value at commas and matches each entry exactly.
    // The entries kept are trimmed, as the value is read back: a kept
    // " Sort_2" would never match the session's value at put-back.
    if let Some(sorting) = ini.get("Misc", "GameSortingOpt") {
        let entries: Vec<&[u8]> = sorting
            .split(|&b| b == b',')
            .map(<[u8]>::trim_ascii)
            .collect();
        if entries.iter().any(|entry| LIST_FILTERS.contains(entry)) {
            let kept: Vec<&[u8]> = entries
                .into_iter()
                .filter(|entry| !LIST_FILTERS.contains(entry))
                .collect();
            changes.push(("Misc", "GameSortingOpt", kept.join(b",".as_slice())));
        }
    }
    let mut forced = Vec::new();
    for (section, key, value) in changes {
        let before = ini.get(section, key).unwrap_or_default().to_vec();
        if before == value {
            continue;
        }
        ini.set(section, key, &value);
        if PUT_BACK.contains(&key) {
            forced.push(Forced {
                section: section.into(),
                key: key.into(),
                before,
                forced: value,
            });
        }
    }
    Settings {
        text: ini.bytes(),
        added,
        forced,
    }
}

/// A user's own copy's first `Settings.ini`: [`DEFAULT_SETTINGS`] through
/// [`settings_ini`], never the host's, which holds its user's nicknames,
/// Steam IDs and helper accounts' password.
pub fn default_settings_ini() -> Vec<u8> {
    settings_ini(DEFAULT_SETTINGS.as_bytes()).text
}

/// `Settings.ini` with each of `forced` back to the user's value where it
/// still has the session's: a value Nucleus or the user changed since
/// stays. Returns the file and how many values it put back.
pub fn put_back_settings(text: &[u8], forced: &[Forced]) -> (Vec<u8>, usize) {
    let mut ini = Ini::parse(text);
    let mut changed = 0;
    for value in forced {
        if ini.get(&value.section, &value.key) == Some(&value.forced[..]) {
            ini.set(&value.section, &value.key, &value.before);
            changed += 1;
        }
    }
    (ini.bytes(), changed)
}

/// Nucleus 2.4.2 keeps per-game settings in `metaInfo.json` in the user's
/// profile folder (`GameMetaInfo`), and assigns a game's pads by its
/// `UseApiIndex` there: `GameProfile.LoadGameProfile` puts it over the
/// profile's "Use XInput Index". `Settings.ini`'s `UseXinputIndex` only
/// seeds a game the file has no entry for, and without the file Nucleus
/// saves the first game it loads with every value false. A game saved with
/// `UseApiIndex` false waits for a button press on each player's pad ("Press
/// a button on Player1's gamepad"), which nobody gives during a background
/// stream, and never starts (#56).
///
/// `metaInfo.json` (`text`, None when there is none) as game `guid` needs
/// it, or None when it already is: a missing or empty file becomes an empty
/// list, so each game Nucleus adds takes `Settings.ini`'s value; the game's
/// entry gets pads by index for its players and their guests, as Nucleus'
/// own `UseApiIndex` setter sets both. Every other value stays. A file that
/// is not a JSON object is refused: Nucleus would read no game's values
/// from it.
pub fn meta_info_by_index(text: Option<&[u8]>, guid: &str) -> Result<Option<Vec<u8>>, String> {
    let text = text.map(|text| text.strip_prefix(BOM).unwrap_or(text));
    let Some(text) = text.filter(|text| !text.trim_ascii().is_empty()) else {
        return Ok(Some(b"{}".to_vec()));
    };
    let mut games: Value = serde_json::from_slice(text)
        .map_err(|error| format!("Nucleus Co-op can't read it as JSON ({error})"))?;
    let Some(games) = games.as_object_mut() else {
        return Err("Nucleus Co-op can't read it: it is not a JSON object".into());
    };
    let Some(game) = games.get_mut(guid) else {
        return Ok(None);
    };
    let Some(game) = game.as_object_mut() else {
        return Err(format!(
            "Nucleus Co-op can't read the entry of {guid}: it is not a JSON object"
        ));
    };
    let mut changed = false;
    for key in ["UseApiIndex", "UseApiIndexForGuests"] {
        // Nucleus reads a missing value as Settings.ini's.
        if game
            .get(key)
            .is_some_and(|value| value != &Value::Bool(true))
        {
            game.insert(key.into(), Value::Bool(true));
            changed = true;
        }
    }
    if !changed {
        return Ok(None);
    }
    serde_json::to_vec_pretty(games)
        .map(Some)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests;
