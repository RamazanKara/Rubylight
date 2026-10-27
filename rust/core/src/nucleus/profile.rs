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
/// on, or Nucleus would not read the profile.
const FORCED: [(&str, &str, &str); 2] = [
    ("Dev", "UseXinputIndex", "True"),
    ("Misc", "DisableGameProfiles", "False"),
];

/// Nucleus' `Settings.ini` with [`FORCED`] set; other lines stay as they are.
pub fn settings_ini(text: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    for (section, key, value) in FORCED {
        let header = format!("[{section}]");
        let entry = format!("{key}={value}");
        let start = lines
            .iter()
            .position(|l| l.trim().eq_ignore_ascii_case(&header));
        let Some(start) = start else {
            lines.push(header);
            lines.push(entry);
            continue;
        };
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
        match existing {
            Some(i) => lines[i] = entry,
            None => lines.insert(end, entry),
        }
    }
    let mut text = lines.join("\r\n");
    text.push_str("\r\n");
    text
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

    #[test]
    fn settings_ini_assigns_pads_by_index_and_keeps_the_rest() {
        let ini = "[Misc]\r\nDisableGameProfiles=False\r\n[Dev]\r\nUseXinputIndex=False\r\nOther=1\r\n[Audio]\r\n";
        assert_eq!(
            settings_ini(ini),
            "[Misc]\r\nDisableGameProfiles=False\r\n[Dev]\r\nUseXinputIndex=True\r\nOther=1\r\n[Audio]\r\n"
        );
        assert_eq!(
            settings_ini("[Dev]\nOther=1\n[Audio]\n"),
            "[Dev]\r\nOther=1\r\nUseXinputIndex=True\r\n[Audio]\r\n[Misc]\r\nDisableGameProfiles=False\r\n"
        );
        // Profiles switched off would make Nucleus ignore the host's.
        assert_eq!(
            settings_ini("[Misc]\nDisableGameProfiles=True\n"),
            "[Misc]\r\nDisableGameProfiles=False\r\n[Dev]\r\nUseXinputIndex=True\r\n"
        );
        assert_eq!(
            settings_ini(""),
            "[Dev]\r\nUseXinputIndex=True\r\n[Misc]\r\nDisableGameProfiles=False\r\n"
        );
    }
}
