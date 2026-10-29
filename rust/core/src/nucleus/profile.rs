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
                    "IdealProcessor": "",
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

/// What the host's profile needs in Nucleus' `Settings.ini`: pads assigned
/// by XInput index, which the profile's pad names are, and game profiles
/// on, or Nucleus would not read the profile. Nucleus' check of its own
/// folder and of a Documents folder in OneDrive shows a message box before
/// its window opens, which no one answers in a background stream, and its
/// favourites-only list would hide the host's game (#59).
const FORCED: [(&str, &str, &str); 4] = [
    ("Dev", "UseXinputIndex", "True"),
    ("Misc", "DisableGameProfiles", "False"),
    ("Dev", "DisablePathCheck", "True"),
    ("Dev", "ShowFavoriteOnly", "False"),
];

/// Nucleus' list filters in `[Misc] GameSortingOpt` (`SortOptionsPanel.cs`):
/// only games with gamepad hooks, keyboard or several keyboards. Each would
/// hide the host's own handler, which has none of them; the `Sort_*`
/// orders stay.
const LIST_FILTERS: [&str; 3] = ["Show_Type1", "Show_Type2", "Show_Type3"];

/// The line of `key` in `section` and where the section ends, as
/// `GetPrivateProfileString` finds them: names in any case, the first
/// section of that name. None without the section.
fn find(lines: &[String], section: &str, key: &str) -> Option<(Option<usize>, usize)> {
    let header = format!("[{section}]");
    let start = lines
        .iter()
        .position(|l| l.trim().eq_ignore_ascii_case(&header))?;
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.trim().starts_with('['))
        .map_or(lines.len(), |i| start + 1 + i);
    let existing = (start + 1..end).find(|&i| {
        lines[i]
            .split('=')
            .next()
            .is_some_and(|k| k.trim().eq_ignore_ascii_case(key))
    });
    Some((existing, end))
}

/// The value of `key` in `section` of an INI file, as Nucleus reads it.
pub fn ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let (line, _) = find(&lines, section, key)?;
    let (_, value) = lines[line?].split_once('=')?;
    Some(value.trim().to_string())
}

/// An INI file with each (section, key, value) of `values` set, CRLF line
/// ends; other lines stay as they are.
pub fn ini_set(text: &str, values: &[(&str, &str, &str)]) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    for &(section, key, value) in values {
        let entry = format!("{key}={value}");
        match find(&lines, section, key) {
            Some((Some(i), _)) => lines[i] = entry,
            Some((None, end)) => lines.insert(end, entry),
            None => {
                lines.push(format!("[{section}]"));
                lines.push(entry);
            }
        }
    }
    let mut text = lines.join("\r\n");
    text.push_str("\r\n");
    text
}

/// Nucleus' `Settings.ini` with [`FORCED`] set and [`LIST_FILTERS`] off;
/// other lines stay as they are.
pub fn settings_ini(text: &str) -> String {
    let text = ini_set(text, &FORCED);
    // Nucleus splits the value at commas and matches each entry exactly.
    let Some(sorting) = ini_value(&text, "Misc", "GameSortingOpt") else {
        return text;
    };
    let kept: Vec<&str> = sorting
        .split(',')
        .filter(|entry| !LIST_FILTERS.contains(&entry.trim()))
        .collect();
    if kept.len() == sorting.split(',').count() {
        return text;
    }
    ini_set(&text, &[("Misc", "GameSortingOpt", &kept.join(","))])
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

    /// The `[Dev]` lines every rewrite adds after `UseXinputIndex`.
    const DEV: &str = "DisablePathCheck=True\r\nShowFavoriteOnly=False\r\n";

    #[test]
    fn settings_ini_assigns_pads_by_index_and_keeps_the_rest() {
        let ini = "[Misc]\r\nDisableGameProfiles=False\r\n[Dev]\r\nUseXinputIndex=False\r\nOther=1\r\n[Audio]\r\n";
        assert_eq!(
            settings_ini(ini),
            format!(
                "[Misc]\r\nDisableGameProfiles=False\r\n[Dev]\r\nUseXinputIndex=True\r\nOther=1\r\n{DEV}[Audio]\r\n"
            )
        );
        assert_eq!(
            settings_ini("[Dev]\nOther=1\n[Audio]\n"),
            format!(
                "[Dev]\r\nOther=1\r\nUseXinputIndex=True\r\n{DEV}[Audio]\r\n[Misc]\r\nDisableGameProfiles=False\r\n"
            )
        );
        // Profiles switched off would make Nucleus ignore the host's.
        assert_eq!(
            settings_ini("[Misc]\nDisableGameProfiles=True\n"),
            format!("[Misc]\r\nDisableGameProfiles=False\r\n[Dev]\r\nUseXinputIndex=True\r\n{DEV}")
        );
        assert_eq!(
            settings_ini(""),
            format!("[Dev]\r\nUseXinputIndex=True\r\n{DEV}[Misc]\r\nDisableGameProfiles=False\r\n")
        );
    }

    #[test]
    fn ini_values_are_read_and_set_as_nucleus_reads_them() {
        let ini = "[misc]\r\n GameSortingOpt = Sort_1 \r\n[Dev]\r\nShowFavoriteOnly=True\r\n";
        assert_eq!(
            ini_value(ini, "Misc", "gamesortingopt").as_deref(),
            Some("Sort_1")
        );
        assert_eq!(ini_value(ini, "Dev", "Missing"), None);
        assert_eq!(ini_value(ini, "Audio", "Missing"), None);
        assert_eq!(
            ini_set(ini, &[("Dev", "ShowFavoriteOnly", "False"), ("Audio", "On", "1")]),
            "[misc]\r\n GameSortingOpt = Sort_1 \r\n[Dev]\r\nShowFavoriteOnly=False\r\n[Audio]\r\nOn=1\r\n"
        );
    }

    /// Nucleus Co-op 2.4.2's own Settings.ini as its release ships it.
    const SHIPPED: &str = include_str!("fixtures/nucleus-2.4.2-Settings.ini");

    #[test]
    fn settings_ini_lets_nucleus_start_and_list_the_game_without_a_message_box() {
        // A user's settings that stalled a background stream at Nucleus'
        // start (#59): its folder check on (a message box for a Documents
        // folder in OneDrive or a folder under Program Files), favourites
        // only, and only games for a single keyboard listed.
        // A checkout may have turned its line ends into CRLF.
        let seeded = SHIPPED
            .replace("\r\n", "\n")
            .replace("ShowFavoriteOnly=False", "ShowFavoriteOnly=True")
            .replace("GameSortingOpt=\n", "GameSortingOpt=Sort_2,Show_Type2\n");
        assert!(seeded.contains("DisablePathCheck=False\n"), "{seeded}");
        assert!(seeded.contains("ShowFavoriteOnly=True\n"), "{seeded}");
        assert!(
            seeded.contains("GameSortingOpt=Sort_2,Show_Type2\n"),
            "{seeded}"
        );
        let fixed = settings_ini(&seeded);
        let value = |key: &str| {
            let section = if key == "GameSortingOpt" { "Misc" } else { "Dev" };
            ini_value(&fixed, section, key).unwrap()
        };
        assert_eq!(value("DisablePathCheck"), "True");
        assert_eq!(value("ShowFavoriteOnly"), "False");
        assert_eq!(value("GameSortingOpt"), "Sort_2");
        assert_eq!(value("UseXinputIndex"), "True");
        // Every other line is as shipped, in its place.
        let changed = [
            "DisablePathCheck",
            "ShowFavoriteOnly",
            "GameSortingOpt",
            "UseXinputIndex",
        ];
        let before: Vec<&str> = seeded.lines().collect();
        let after: Vec<&str> = fixed.lines().collect();
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
        assert_eq!(settings_ini(&fixed), fixed);
        // The Sort_* order is kept, the filters go, an empty value stays.
        for (sorting, kept) in [
            ("Show_Type1,Sort_4,Show_Type3", "Sort_4"),
            ("Show_Type1", ""),
            ("Sort_1", "Sort_1"),
            ("", ""),
        ] {
            let text = settings_ini(&format!("[Misc]\r\nGameSortingOpt={sorting}\r\n"));
            assert_eq!(
                ini_value(&text, "Misc", "GameSortingOpt").as_deref(),
                Some(kept),
                "{sorting}"
            );
        }
    }
}
