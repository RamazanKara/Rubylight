//! Background streaming: a device plays on its own virtual display while
//! someone keeps using the PC. The stream gets an isolated virtual display,
//! controller input and the game's own sound; the device's keyboard and
//! mouse never reach the desktop the person at the PC is typing into, and
//! the host does not take focus for the game.
//!
//! Windows has one input focus per session, so the game never has the real
//! focus. The host tells it that it is active (the activation messages many
//! games mute or slow down without) and, where a device opts in, injects a
//! hook that feeds it the device's keyboard and mouse directly.
use crate::{config::Config, steam::Process};
use std::collections::{BTreeMap, BTreeSet};

/// The effective-config key a device or app turns on.
pub const SETTING: &str = "background_stream";
/// The device setting (a paired client's extra field).
pub const DEVICE_KEY: &str = "background_stream";
/// The app setting (an app's extra field).
pub const APP_KEY: &str = "background-stream";
/// Away from the PC's monitors, so the local mouse cannot reach it, and not
/// primary, so the PC's windows and taskbar stay where they are.
pub const LAYOUT: &str = "extended_isolated";
/// What a background stream fixes, whatever the host, device or app chose:
/// the device's keyboard, mouse, touch and pen would type and click into the
/// PC's focused window, system audio would carry everything the PC plays,
/// and Playnite's launch focus would pull the game in front of the PC user.
pub const FORCED: [(&str, &str); 4] = [
    ("keyboard", "false"),
    ("mouse", "false"),
    ("audio_capture_mode", "game"),
    ("playnite_focus_attempts", "0"),
];
/// SDL (and so many ports and indie games) ignores controllers while its
/// window is unfocused unless this hint is set; it reads hints from the
/// environment.
pub const ENVIRONMENT: [(&str, &str); 1] = [("SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS", "1")];

fn flag(value: Option<&serde_json::Value>) -> bool {
    match value {
        Some(serde_json::Value::Bool(on)) => *on,
        Some(serde_json::Value::String(text)) => {
            matches!(
                text.trim().to_ascii_lowercase().as_str(),
                "true" | "1" | "yes" | "on"
            )
        }
        Some(serde_json::Value::Number(n)) => n.as_i64().is_some_and(|n| n != 0),
        _ => false,
    }
}

/// Whether the device or the app asks for background streaming.
pub fn requested(
    device: &BTreeMap<String, serde_json::Value>,
    app: Option<&BTreeMap<String, serde_json::Value>>,
) -> bool {
    flag(device.get(DEVICE_KEY)) || app.is_some_and(|app| flag(app.get(APP_KEY)))
}

/// Turn background streaming on in a stream's effective configuration.
pub fn apply(config: &mut Config) {
    config.values.insert(SETTING.into(), "true".into());
    for (key, value) in FORCED {
        config.values.insert(key.into(), value.into());
    }
}

pub fn enabled(config: &Config) -> bool {
    config.boolean(SETTING, false)
}

/// The processes whose windows belong to the game: the verified owned
/// processes and everything they started. A child must be younger than its
/// parent, so a reused parent ID does not adopt an unrelated program.
pub fn window_owners(owned: &BTreeMap<u32, u64>, processes: &[Process]) -> BTreeSet<u32> {
    let mut found: BTreeMap<u32, u64> = processes
        .iter()
        .filter(|p| p.pid != 0 && owned.get(&p.pid) == Some(&p.started))
        .map(|p| (p.pid, p.started))
        .collect();
    loop {
        let before = found.len();
        for p in processes {
            if p.pid != 0
                && !found.contains_key(&p.pid)
                && found
                    .get(&p.parent)
                    .is_some_and(|&parent_started| p.started >= parent_started)
            {
                found.insert(p.pid, p.started);
            }
        }
        if found.len() == before {
            return found.into_keys().collect();
        }
    }
}

/// A screen rectangle in desktop coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}
impl Rect {
    fn centre(&self) -> (i64, i64) {
        (
            i64::from(self.x) + i64::from(self.width) / 2,
            i64::from(self.y) + i64::from(self.height) / 2,
        )
    }
    fn contains(&self, (x, y): (i64, i64)) -> bool {
        x >= i64::from(self.x)
            && y >= i64::from(self.y)
            && x < i64::from(self.x) + i64::from(self.width)
            && y < i64::from(self.y) + i64::from(self.height)
    }
}

/// Where to put a game window that is not on the stream's display: a window
/// covering its monitor (borderless fullscreen, maximized) covers the target;
/// any other keeps its size, up to the target's, centred on it. None when the
/// window's centre is already on the target.
pub fn placement(window: Rect, monitor: Rect, target: Rect) -> Option<Rect> {
    if target.width <= 0 || target.height <= 0 || target.contains(window.centre()) {
        return None;
    }
    // Window frames reach a few pixels past a maximized window's monitor.
    let covers = window.width >= monitor.width - 16 && window.height >= monitor.height - 16;
    if covers {
        return Some(target);
    }
    let width = window.width.clamp(1, target.width);
    let height = window.height.clamp(1, target.height);
    Some(Rect {
        x: target.x + (target.width - width) / 2,
        y: target.y + (target.height - height) / 2,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn extra(value: serde_json::Value) -> BTreeMap<String, serde_json::Value> {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn a_device_or_an_app_turns_it_on() {
        let none = extra(json!({}));
        assert!(!requested(&none, None));
        assert!(!requested(&none, Some(&none)));
        assert!(requested(&extra(json!({"background_stream": true})), None));
        assert!(requested(
            &extra(json!({"background_stream": "true"})),
            None
        ));
        assert!(!requested(
            &extra(json!({"background_stream": false})),
            None
        ));
        assert!(requested(
            &none,
            Some(&extra(json!({"background-stream": true})))
        ));
        assert!(!requested(
            &none,
            Some(&extra(json!({"background-stream": "0"})))
        ));
        // An app cannot turn off a device's background streaming.
        assert!(requested(
            &extra(json!({"background_stream": true})),
            Some(&extra(json!({"background-stream": false})))
        ));
    }

    #[test]
    fn applying_it_keeps_the_devices_keyboard_and_mouse_off_the_desktop() {
        let mut config =
            Config::parse("keyboard=true\nmouse=true\naudio_capture_mode=system\n").unwrap();
        assert!(!enabled(&config));
        apply(&mut config);
        assert!(enabled(&config));
        assert!(!config.boolean("keyboard", true));
        assert!(!config.boolean("mouse", true));
        assert!(config.boolean("controller", true));
        assert_eq!(config.get("audio_capture_mode", "system"), "game");
        assert_eq!(config.integer("playnite_focus_attempts", 3), 0);
        let policy = crate::input_policy::Policy::resolve(&config).unwrap();
        assert!(!policy.allows(&crate::input::Input::Text("a".into())));
        assert!(!policy.allows(&crate::input::Input::Relative { x: 1, y: 1 }));
    }

    fn process(pid: u32, parent: u32, started: u64) -> Process {
        Process {
            pid,
            parent,
            started,
            name: String::new(),
        }
    }

    #[test]
    fn window_owners_follow_the_game_tree_but_not_a_reused_parent_id() {
        let processes = [
            process(10, 1, 100),
            process(11, 10, 110),
            process(12, 11, 120),
            // Older than its "parent": the ID 10 belonged to another program.
            process(13, 10, 50),
            process(20, 1, 100),
        ];
        let owned = BTreeMap::from([(10, 100)]);
        assert_eq!(
            window_owners(&owned, &processes),
            BTreeSet::from([10, 11, 12])
        );
        // A reused ID with another creation time is not the game.
        assert!(window_owners(&BTreeMap::from([(10, 99)]), &processes).is_empty());
    }

    const PC: Rect = Rect {
        x: 0,
        y: 0,
        width: 2560,
        height: 1440,
    };
    const STREAM: Rect = Rect {
        x: 64000,
        y: 64000,
        width: 1920,
        height: 1080,
    };

    #[test]
    fn a_borderless_or_maximized_game_fills_the_stream_display() {
        assert_eq!(placement(PC, PC, STREAM), Some(STREAM));
        let maximized = Rect {
            x: -8,
            y: -8,
            width: 2576,
            height: 1416,
        };
        assert_eq!(placement(maximized, PC, STREAM), Some(STREAM));
    }

    #[test]
    fn a_window_keeps_its_size_centred_on_the_stream_display() {
        let window = Rect {
            x: 100,
            y: 100,
            width: 1280,
            height: 720,
        };
        assert_eq!(
            placement(window, PC, STREAM),
            Some(Rect {
                x: 64320,
                y: 64180,
                width: 1280,
                height: 720
            })
        );
        // Larger than the stream display but not covering its own monitor.
        let big = Rect {
            x: 0,
            y: 0,
            width: 2400,
            height: 1300,
        };
        assert_eq!(
            placement(big, PC, STREAM),
            Some(Rect {
                x: 64000,
                y: 64000,
                width: 1920,
                height: 1080
            })
        );
    }

    #[test]
    fn a_window_already_on_the_stream_display_stays() {
        let window = Rect {
            x: 64100,
            y: 64100,
            width: 800,
            height: 600,
        };
        assert_eq!(placement(window, STREAM, STREAM), None);
        assert_eq!(placement(STREAM, STREAM, STREAM), None);
        let empty = Rect { width: 0, ..STREAM };
        assert_eq!(placement(PC, PC, empty), None);
    }
}
