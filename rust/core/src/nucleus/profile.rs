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
/// players' screens against its own list).
pub fn build(players: &[Player], screens: &[Rect]) -> Value {
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
        "AutoPlay": { "Enabled": true },
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
mod tests {
    use super::*;

    #[test]
    fn pads_are_named_by_xinput_index_like_nucleus() {
        assert_eq!(gamepad_guid(0), "00000000-0000-0000-0000-200000000001");
        assert_eq!(gamepad_guid(3), "00000000-0000-0000-0000-200000000004");
        assert_eq!(gamepad_guid(9), "00000000-0000-0000-0000-200000000010");
    }

    const SCREEN: Rect = Rect {
        x: 0,
        y: 0,
        width: 2560,
        height: 1440,
    };

    #[test]
    fn the_profile_has_one_xinput_player_per_slot_and_starts_by_itself() {
        let players = [
            Player {
                xinput: 1,
                bounds: Rect {
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 1080,
                },
            },
            Player {
                xinput: 2,
                bounds: Rect {
                    x: 100,
                    y: 100,
                    width: 1280,
                    height: 720,
                },
            },
        ];
        let p = build(&players, &[SCREEN]);
        assert_eq!(p["Title"], TITLE);
        assert_eq!(p["AutoPlay"]["Enabled"], true);
        assert_eq!(p["Use XInput Index"], true);
        assert_eq!(p["Player(s)"], 2);
        assert_eq!(p["UseSplitDiv"]["Enabled"], false);
        assert_eq!(p["AutoDesktopScaling"]["Enabled"], false);
        let data = p["Data"].as_array().unwrap();
        assert_eq!(data.len(), 2);
        assert_eq!(data[1]["PlayerID"], 1);
        assert_eq!(
            data[1]["GamepadGuid"],
            "00000000-0000-0000-0000-200000000003"
        );
        assert_eq!(data[1]["IsXInput"], true);
        assert_eq!(data[1]["IsKeyboardPlayer"], false);
        assert_eq!(data[1]["MonitorBounds"]["Width"], 1280);
        assert_eq!(data[1]["ScreenIndex"], 0);
        assert_eq!(data[1]["Owner"]["Display"]["Width"], 2560);
        assert_eq!(data[1]["Guests"]["GUIDS"], "");
        assert_eq!(p["AllScreens"][0]["Height"], 1440);
        // Every key Nucleus 2.4 reads without a null check is there.
        for key in [
            "Options",
            "WindowsSetupTiming",
            "PauseBetweenInstanceLaunch",
            "UseNicknames",
            "CutscenesModeSettings",
            "Network",
            "CustomLayout",
            "Notes",
            "AudioSettings",
            "AudioInstances",
        ] {
            assert!(!p[key].is_null(), "{key}");
        }
    }

    /// #59: Nucleus 2.4.2's `GenericGameHandler.Play` runs `int.Parse` on a
    /// player's ideal processor unless it is "*", and an empty one failed
    /// every start with a FormatException once the game's process was up.
    #[test]
    fn each_player_s_processor_settings_are_ones_nucleus_starts_with() {
        let player = Player {
            xinput: 0,
            bounds: SCREEN,
        };
        let p = build(&[player, player], &[SCREEN]);
        for data in p["Data"].as_array().unwrap() {
            let processor = &data["Processor"];
            let ideal = processor["IdealProcessor"].as_str().unwrap();
            assert!(
                ideal == "*" || ideal.parse::<i32>().is_ok(),
                "Nucleus can't parse IdealProcessor {ideal:?}"
            );
            assert_eq!(ideal, "*");
            // No affinity and Normal priority: Nucleus keeps its own.
            assert_eq!(processor["ProcessorAffinity"], "");
            assert_eq!(processor["ProcessorPriorityClass"], "Normal");
        }
    }

    #[test]
    fn the_host_keeps_one_profile_in_nucleus_numbering() {
        assert_eq!(number(&[]), 1);
        let others = [(1, None), (2, Some("Couch".to_string()))];
        assert_eq!(number(&others), 3);
        let ours = [(1, None), (2, Some(TITLE.to_string())), (3, None)];
        assert_eq!(number(&ours), 2);
        assert_eq!(file_number("Profile[12].json"), Some(12));
        assert_eq!(file_number("Profile[x].json"), None);
        assert_eq!(file_number("notes.txt"), None);
    }

    /// The value of `key` in `section` as text.
    fn value(text: &[u8], section: &str, key: &str) -> Option<String> {
        ini_value(text, section, key).map(|v| String::from_utf8(v).unwrap())
    }

    /// Nucleus 2.4.2's own file; a checkout may have turned its line ends
    /// into CRLF.
    fn shipped() -> String {
        DEFAULT_SETTINGS.replace("\r\n", "\n")
    }

    #[test]
    fn settings_ini_forces_the_hosts_keys_and_keeps_every_other_byte() {
        let ini =
            "[Misc]\r\nDisableGameProfiles=True\r\n[Dev]\r\nUseXinputIndex=False\r\nOther=1\r\n";
        let settings = settings_ini(ini.as_bytes());
        let text = &settings.text;
        assert_eq!(value(text, "Dev", "UseXinputIndex").unwrap(), "True");
        // Profiles switched off would make Nucleus ignore the host's.
        assert_eq!(value(text, "Misc", "DisableGameProfiles").unwrap(), "False");
        assert_eq!(value(text, "Dev", "DisablePathCheck").unwrap(), "True");
        assert_eq!(value(text, "Dev", "ShowFavoriteOnly").unwrap(), "False");
        assert_eq!(
            value(text, "Misc", "IgnoreInputLockReminder").unwrap(),
            "True"
        );
        assert!(
            text.starts_with(b"[Misc]\r\nDisableGameProfiles=False\r\n"),
            "{}",
            String::from_utf8_lossy(text)
        );
        assert!(String::from_utf8_lossy(text).contains("\r\nOther=1\r\n"));
        // Pads by index and profiles on stay set; the rest goes back.
        let keys: Vec<&str> = settings.forced.iter().map(|f| f.key.as_str()).collect();
        // A key completed from Nucleus' defaults that already has the
        // session's value is no change.
        assert_eq!(keys, ["DisablePathCheck"]);
        assert_eq!(settings.forced[0].before, b"False");
        assert_eq!(settings.forced[0].forced, b"True");
    }

    #[test]
    fn a_partial_empty_or_missing_settings_file_is_completed_so_every_key_parses() {
        // The host wrote this two-key file where none was: Nucleus then
        // stopped on bool.Parse("") with a message box (#59).
        let old = b"[Dev]\r\nUseXinputIndex=True\r\n[Misc]\r\nDisableGameProfiles=False\r\n";
        let defaults = Ini::parse(DEFAULT_SETTINGS.as_bytes()).entries();
        assert!(defaults.len() > 100, "{}", defaults.len());
        for text in [&old[..], b"", b"\n"] {
            let settings = settings_ini(text);
            for (section, key, default) in &defaults {
                let found = ini_value(&settings.text, section, key)
                    .unwrap_or_else(|| panic!("{section}.{key} missing"));
                let parses =
                    |v: &[u8]| v.eq_ignore_ascii_case(b"true") || v.eq_ignore_ascii_case(b"false");
                if parses(default.as_slice()) {
                    assert!(parses(found.as_slice()), "{section}.{key}={found:?}");
                }
            }
            assert!(settings.added.contains(&"Dev.Blur".to_string()));
            assert!(settings.added.contains(&"Hotkeys.Close".to_string()));
            // Completed, it stays as it is.
            let again = settings_ini(&settings.text);
            assert_eq!(again.text, settings.text);
            assert!(again.added.is_empty() && again.forced.is_empty());
        }
        let settings = settings_ini(old);
        assert!(!settings.added.contains(&"Dev.UseXinputIndex".to_string()));
        assert!(
            settings
                .text
                .starts_with(b"[Dev]\r\nUseXinputIndex=True\r\n")
        );
    }

    #[test]
    fn a_value_in_the_pcs_code_page_its_bom_and_line_ends_are_kept() {
        // Nucleus writes through kernel32's ANSI calls: ü is 0xFC in Latin-1.
        let mut ini = b"\xef\xbb\xbf[Misc]\nSettingsLocation=C:\\M\xfcnchen\n".to_vec();
        ini.extend_from_slice(shipped().as_bytes());
        let settings = settings_ini(&ini);
        assert!(
            settings
                .text
                .starts_with(b"\xef\xbb\xbf[Misc]\nSettingsLocation=C:\\M\xfcnchen\n")
        );
        // The file's LF stays LF, also for what it changes.
        assert!(!settings.text.contains(&b'\r'));
        assert_eq!(
            ini_value(&settings.text, "Misc", "SettingsLocation").unwrap(),
            b"C:\\M\xfcnchen"
        );
        assert_eq!(
            value(&settings.text, "Dev", "DisablePathCheck").unwrap(),
            "True"
        );
        // A last line without an end gets one before a new line follows.
        let text = ini_set(b"[Dev]\r\nA=1", &[("Dev", "B", "2")]);
        assert_eq!(text, b"[Dev]\r\nA=1\r\nB=2\r\n");
    }

    #[test]
    fn ini_values_are_read_and_set_as_nucleus_reads_them() {
        let ini = b"[misc]\r\n GameSortingOpt = Sort_1 \r\n[Dev]\r\nShowFavoriteOnly=True\r\n";
        assert_eq!(value(ini, "Misc", "gamesortingopt").unwrap(), "Sort_1");
        assert_eq!(ini_value(ini, "Dev", "Missing"), None);
        assert_eq!(ini_value(ini, "Audio", "Missing"), None);
        assert_eq!(
            ini_set(
                ini,
                &[("Dev", "ShowFavoriteOnly", "False"), ("Audio", "On", "1")]
            ),
            b"[misc]\r\n GameSortingOpt = Sort_1 \r\n[Dev]\r\nShowFavoriteOnly=False\r\n[Audio]\r\nOn=1\r\n"
        );
    }

    /// The shipped file with a user's settings that stalled a background
    /// stream at Nucleus' start (#59): its folder check on (a message box
    /// for a Documents folder in OneDrive or a folder under Program
    /// Files), favourites only, and only games for a single keyboard listed.
    fn seeded() -> String {
        let seeded = shipped()
            .replace("ShowFavoriteOnly=False", "ShowFavoriteOnly=True")
            .replace("GameSortingOpt=\n", "GameSortingOpt=Sort_2,Show_Type2\n");
        assert!(seeded.contains("DisablePathCheck=False\n"), "{seeded}");
        assert!(seeded.contains("ShowFavoriteOnly=True\n"), "{seeded}");
        assert!(
            seeded.contains("GameSortingOpt=Sort_2,Show_Type2\n"),
            "{seeded}"
        );
        seeded
    }

    #[test]
    fn settings_ini_lets_nucleus_start_and_list_the_game_without_a_message_box() {
        let seeded = seeded();
        let settings = settings_ini(seeded.as_bytes());
        assert!(settings.added.is_empty(), "{:?}", settings.added);
        let fixed = String::from_utf8(settings.text.clone()).unwrap();
        let get = |section: &str, key: &str| value(fixed.as_bytes(), section, key).unwrap();
        assert_eq!(get("Dev", "DisablePathCheck"), "True");
        assert_eq!(get("Dev", "ShowFavoriteOnly"), "False");
        assert_eq!(get("Misc", "GameSortingOpt"), "Sort_2");
        assert_eq!(get("Dev", "UseXinputIndex"), "True");
        // Every other line is as shipped, in its place.
        let changed = [
            "DisablePathCheck",
            "ShowFavoriteOnly",
            "GameSortingOpt",
            "UseXinputIndex",
        ];
        let before: Vec<&str> = seeded.split('\n').collect();
        let after: Vec<&str> = fixed.split('\n').collect();
        assert_eq!(before.len(), after.len());
        for (old, new) in before.iter().zip(&after) {
            let key = old.split('=').next().unwrap();
            if changed.contains(&key) {
                assert!(new.starts_with(&format!("{key}=")), "{old} became {new}");
            } else {
                assert_eq!(old, new);
            }
        }
        // A second pass changes nothing.
        let again = settings_ini(&settings.text);
        assert_eq!(again.text, settings.text);
        assert!(again.forced.is_empty());
        // The Sort_* order is kept, the filters go, an empty value stays.
        for (sorting, kept) in [
            ("Show_Type1,Sort_4,Show_Type3", "Sort_4"),
            ("Show_Type1", ""),
            ("Sort_1", "Sort_1"),
            ("", ""),
        ] {
            let text = settings_ini(format!("[Misc]\r\nGameSortingOpt={sorting}\r\n").as_bytes());
            assert_eq!(
                value(&text.text, "Misc", "GameSortingOpt").unwrap(),
                kept,
                "{sorting}"
            );
        }
    }

    #[test]
    fn the_users_values_come_back_where_the_session_s_still_are() {
        let seeded = seeded();
        let settings = settings_ini(seeded.as_bytes());
        let keys: Vec<&str> = settings.forced.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(
            keys,
            ["DisablePathCheck", "ShowFavoriteOnly", "GameSortingOpt"]
        );
        // Put back whole; pads by index stay.
        let (back, changed) = put_back_settings(&settings.text, &settings.forced);
        assert_eq!(changed, 3);
        assert_eq!(
            String::from_utf8(back).unwrap(),
            seeded.replace("UseXinputIndex=False", "UseXinputIndex=True")
        );
        // Nucleus or the user changed the list's order meanwhile: it stays.
        let changed_since = ini_set(&settings.text, &[("Misc", "GameSortingOpt", "Sort_3")]);
        let (back, changed) = put_back_settings(&changed_since, &settings.forced);
        assert_eq!(changed, 2);
        assert_eq!(value(&back, "Misc", "GameSortingOpt").unwrap(), "Sort_3");
        assert_eq!(value(&back, "Dev", "ShowFavoriteOnly").unwrap(), "True");
        // Again, nothing is left to put back.
        assert_eq!(put_back_settings(&back, &settings.forced).1, 0);
    }

    #[test]
    fn a_list_filter_beside_spaced_entries_comes_back() {
        // Nucleus writes no spaces, but a hand-edited file may have them:
        // the session's value is trimmed as it is read back.
        let users = b"[Misc]\r\nGameSortingOpt=Show_Type1, Sort_2 ,Sort_4\r\n";
        let settings = settings_ini(users);
        assert_eq!(
            value(&settings.text, "Misc", "GameSortingOpt").unwrap(),
            "Sort_2,Sort_4"
        );
        let (back, changed) = put_back_settings(&settings.text, &settings.forced);
        assert_eq!(changed, settings.forced.len());
        assert_eq!(
            value(&back, "Misc", "GameSortingOpt").unwrap(),
            "Show_Type1, Sort_2 ,Sort_4"
        );
        // Spaced entries without a filter are no change of the session's.
        let unfiltered = settings_ini(b"[Misc]\r\nGameSortingOpt=Sort_2, Sort_4\r\n");
        assert!(
            unfiltered.forced.iter().all(|f| f.key != "GameSortingOpt"),
            "{:?}",
            unfiltered.forced
        );
        assert_eq!(
            value(&unfiltered.text, "Misc", "GameSortingOpt").unwrap(),
            "Sort_2, Sort_4"
        );
    }

    #[test]
    fn a_new_copys_settings_are_nucleus_own_with_the_sessions_values() {
        let text = default_settings_ini();
        // Every key of the shipped file, nothing to complete or force again.
        for (section, key, _) in Ini::parse(DEFAULT_SETTINGS.as_bytes()).entries() {
            assert!(
                ini_value(&text, &section, &key).is_some(),
                "{section}.{key}"
            );
        }
        let again = settings_ini(&text);
        assert!(again.added.is_empty() && again.forced.is_empty());
        for (section, key, forced) in FORCED {
            assert_eq!(value(&text, section, key).unwrap(), forced);
        }
        // As the release ships it: nobody's own values.
        assert_eq!(
            value(&text, "Misc", "NucleusAccountPassword").unwrap(),
            "12345"
        );
    }

    /// An entry as Nucleus 2.4.2 saves the first game it loads without a
    /// metaInfo.json (`GameMetaInfo.SaveGameMetaInfo`): every value false.
    const SAVED_FIRST: &str = r#"{
  "Rubylight-sandfall_win64_shipping-049e4c28": {
    "TotalPlayTime": null,
    "LastPlayedAt": null,
    "IconPath": null,
    "SaveProfile": false,
    "DisableProfiles": false,
    "KeepSymLink": false,
    "Favorite": false,
    "FirstLaunch": false,
    "CheckUpdate": false,
    "UseApiIndex": false,
    "UseApiIndexForGuests": false,
    "ProfileAssignGamepadByButonPress": false,
    "SteamLanguage": "App Setting"
  },
  "Other": {
    "UseApiIndex": false,
    "Favorite": true
  }
}"#;

    fn entry(text: &[u8], guid: &str) -> serde_json::Map<String, Value> {
        let games: Value = serde_json::from_slice(text).unwrap();
        games[guid].as_object().unwrap().clone()
    }

    #[test]
    fn a_game_saved_with_pads_by_button_press_gets_them_by_index() {
        let guid = "Rubylight-sandfall_win64_shipping-049e4c28";
        let text = meta_info_by_index(Some(SAVED_FIRST.as_bytes()), guid)
            .unwrap()
            .unwrap();
        let game = entry(&text, guid);
        assert_eq!(game["UseApiIndex"], true);
        assert_eq!(game["UseApiIndexForGuests"], true);
        // Every other value stays, the other game's too.
        let before = entry(SAVED_FIRST.as_bytes(), guid);
        assert_eq!(game.len(), before.len());
        for (key, value) in &before {
            if !key.starts_with("UseApiIndex") {
                assert_eq!(&game[key], value, "{key}");
            }
        }
        assert_eq!(
            entry(&text, "Other"),
            entry(SAVED_FIRST.as_bytes(), "Other")
        );
        // Written once: the file it wrote needs nothing more.
        assert_eq!(meta_info_by_index(Some(&text), guid), Ok(None));
    }

    #[test]
    fn without_metainfo_nucleus_gets_an_empty_list_to_seed_from_settings() {
        // Not Nucleus' own first save, which turns pads by index off.
        assert_eq!(meta_info_by_index(None, "Game"), Ok(Some(b"{}".to_vec())));
        assert_eq!(
            meta_info_by_index(Some(b" \r\n"), "Game"),
            Ok(Some(b"{}".to_vec()))
        );
        assert_eq!(
            meta_info_by_index(Some(b"\xef\xbb\xbf"), "Game"),
            Ok(Some(b"{}".to_vec()))
        );
    }

    #[test]
    fn a_game_with_pads_by_index_or_no_entry_leaves_the_file_alone() {
        let by_index = br#"{"Game": {"UseApiIndex": true, "UseApiIndexForGuests": true}}"#;
        assert_eq!(meta_info_by_index(Some(by_index), "Game"), Ok(None));
        // Missing values are Settings.ini's, which the session sets.
        assert_eq!(
            meta_info_by_index(Some(br#"{"Game": {"Favorite": true}}"#), "Game"),
            Ok(None)
        );
        // A game Nucleus has not saved yet takes Settings.ini's value.
        assert_eq!(
            meta_info_by_index(Some(SAVED_FIRST.as_bytes()), "Game"),
            Ok(None)
        );
        // Its byte order mark is read past.
        let mut marked = b"\xef\xbb\xbf".to_vec();
        marked.extend_from_slice(by_index);
        assert_eq!(meta_info_by_index(Some(&marked), "Game"), Ok(None));
    }

    #[test]
    fn a_metainfo_nucleus_can_t_read_is_refused() {
        for text in [&b"{"[..], b"[]", b"null", br#"{"Game": 1}"#] {
            let error = meta_info_by_index(Some(text), "Game").unwrap_err();
            assert!(error.starts_with("Nucleus Co-op can't read"), "{error}");
        }
    }
}
