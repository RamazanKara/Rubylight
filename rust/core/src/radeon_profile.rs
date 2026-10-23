//! Radeon profiles per game: Competitive, Smoothness, Quality and Custom, named like
//! Nightjar's per-game presets. A library app carries the choice in its own entry:
//! `radeon-profile` names the profile and `radeon-settings` lists the settings of a Custom
//! one. The host applies the settings through the journaled Radeon controls when the game is
//! launched and puts them back when the session ends (`rust/host/src/radeon_profile.rs`).
//!
//! The table is `radeon_profiles.json`, which the web console reads too. The settings are
//! ids the Radeon controls already write (`docs/hardware-matrix.md`). The profiles are not a
//! measured claim: they set what their description says and nothing more.
use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, sync::OnceLock};

/// The app key that names the profile.
pub const PROFILE: &str = "radeon-profile";
/// The app key that lists a Custom profile's settings: an object of setting id to value.
pub const SETTINGS: &str = "radeon-settings";
/// The most settings a Custom profile lists.
pub const MAX_SETTINGS: usize = 24;

#[derive(Debug, Deserialize)]
pub struct Table {
    /// The token that stands for the stream's frame rate in a value.
    pub stream_fps: String,
    pub profiles: Vec<Profile>,
}
#[derive(Debug, Deserialize, PartialEq)]
pub struct Profile {
    pub id: String,
    pub label: String,
    pub description: String,
    pub settings: Vec<Item>,
}
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Item {
    pub id: String,
    pub value: Value,
}

pub fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| {
        serde_json::from_str(include_str!("radeon_profiles.json"))
            .expect("bundled radeon_profiles.json")
    })
}

/// The profile `id` names, ignoring case and surrounding space.
pub fn find(id: &str) -> Option<&'static Profile> {
    let id = id.trim().to_ascii_lowercase();
    table().profiles.iter().find(|profile| profile.id == id)
}

/// What an app asks for, with the settings to write in order.
#[derive(Debug, PartialEq)]
pub struct Selected {
    pub profile: &'static Profile,
    pub changes: Vec<Item>,
}

/// A setting id: lower-case words joined by `.` (`frame_rate_target.fps`).
fn valid_id(id: &str) -> bool {
    id.len() <= 64
        && id.contains('.')
        && id.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        })
}

/// The settings a Custom profile lists, validated, with each feature's `.enabled` toggle
/// first: the driver refuses a feature's other settings while it is off, so the toggle has
/// to come earlier in the same request.
fn custom_items(value: &Value) -> Result<Vec<Item>> {
    let Some(settings) = value.as_object() else {
        bail!("{SETTINGS} must be an object of Radeon setting ids and values");
    };
    if settings.len() > MAX_SETTINGS {
        bail!("{SETTINGS} lists more than {MAX_SETTINGS} settings");
    }
    let mut items = Vec::with_capacity(settings.len());
    for (id, value) in settings {
        if !valid_id(id) {
            bail!("{id:?} is not a Radeon setting id");
        }
        let fine = match value {
            Value::Bool(_) => true,
            Value::Number(number) => number.is_i64(),
            Value::String(text) => !text.is_empty() && text.len() <= 64,
            _ => false,
        };
        if !fine {
            bail!("{id}: a value is a boolean, a whole number or a short text");
        }
        items.push(Item {
            id: id.clone(),
            value: value.clone(),
        });
    }
    items.sort_by_key(|item| !item.id.ends_with(".enabled"));
    Ok(items)
}

/// Whether `extra` (an app's own entry) names a Radeon profile that makes sense. Run when an
/// app is saved, so a typo is an error at the editor and not a game that starts without it.
pub fn check(extra: &BTreeMap<String, Value>) -> Result<()> {
    selected(extra, None).map(drop)
}

/// The profile an app asks for, or `None` when it asks for none. `fps` is the stream's frame
/// rate, which a value of `$stream_fps` stands for; with `None` such a value stays as it is
/// (a check at save time has no stream).
pub fn selected(extra: &BTreeMap<String, Value>, fps: Option<u32>) -> Result<Option<Selected>> {
    let name = match extra.get(PROFILE) {
        None | Some(Value::Null) => "",
        Some(Value::String(name)) => name.trim(),
        Some(_) => bail!("{PROFILE} must be competitive, smoothness, quality or custom"),
    };
    let listed = match extra.get(SETTINGS) {
        None | Some(Value::Null) => None,
        Some(Value::Object(map)) if map.is_empty() => None,
        Some(other) => Some(other),
    };
    if name.is_empty() {
        if listed.is_some() {
            bail!("{SETTINGS} needs {PROFILE} set to custom");
        }
        return Ok(None);
    }
    let Some(profile) = find(name) else {
        bail!("{PROFILE} must be competitive, smoothness, quality or custom");
    };
    let items = if profile.id == "custom" {
        let Some(listed) = listed else {
            bail!("the custom Radeon profile needs at least one setting in {SETTINGS}");
        };
        custom_items(listed)?
    } else {
        if listed.is_some() {
            bail!("{SETTINGS} goes only with the custom Radeon profile");
        }
        profile.settings.clone()
    };
    let token = &table().stream_fps;
    let mut changes = items;
    for item in &mut changes {
        if let (Some(rate), true) = (fps, item.value.as_str() == Some(token)) {
            item.value = Value::from(rate.clamp(15, 1000));
        }
    }
    Ok(Some(Selected { profile, changes }))
}

impl Selected {
    /// The first setting whose value still stands for the stream's frame rate, which a launch
    /// without one cannot supply. Such a profile is refused before anything is written.
    pub fn without_stream_fps(&self) -> Option<&str> {
        let token = &table().stream_fps;
        self.changes
            .iter()
            .find(|item| item.value.as_str() == Some(token))
            .map(|item| item.id.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Map, json};

    fn extra(value: Value) -> BTreeMap<String, Value> {
        serde_json::from_value(value).unwrap()
    }
    fn ids(selected: &Selected) -> Vec<&str> {
        selected
            .changes
            .iter()
            .map(|item| item.id.as_str())
            .collect()
    }

    #[test]
    fn the_table_has_the_four_profiles_with_valid_ids() {
        let ids: Vec<_> = table().profiles.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["competitive", "smoothness", "quality", "custom"]);
        for profile in &table().profiles {
            for item in &profile.settings {
                assert!(valid_id(&item.id), "{}", item.id);
            }
        }
        assert!(find("custom").unwrap().settings.is_empty());
    }

    #[test]
    fn an_app_without_a_profile_asks_for_none() {
        assert_eq!(selected(&extra(json!({})), Some(60)).unwrap(), None);
        for off in [
            json!({"radeon-profile": ""}),
            json!({"radeon-profile": null}),
        ] {
            assert_eq!(selected(&extra(off), Some(60)).unwrap(), None);
        }
    }

    #[test]
    fn a_named_profile_gives_its_settings_with_the_stream_rate_filled_in() {
        let smooth = selected(&extra(json!({"radeon-profile": " Smoothness "})), Some(120))
            .unwrap()
            .unwrap();
        assert_eq!(smooth.profile.id, "smoothness");
        assert_eq!(
            smooth.changes,
            [
                Item {
                    id: "frame_rate_target.enabled".into(),
                    value: json!(true)
                },
                Item {
                    id: "frame_rate_target.fps".into(),
                    value: json!(120)
                },
            ]
        );
        let competitive = selected(&extra(json!({"radeon-profile": "competitive"})), None)
            .unwrap()
            .unwrap();
        assert_eq!(ids(&competitive), ["anti_lag.enabled"]);
        // The rate is kept inside what the driver takes.
        let slow = selected(&extra(json!({"radeon-profile": "smoothness"})), Some(5))
            .unwrap()
            .unwrap();
        assert_eq!(slow.changes[1].value, json!(15));
    }

    #[test]
    fn custom_lists_its_own_settings_with_toggles_first() {
        let custom = selected(
            &extra(json!({
                "radeon-profile": "custom",
                "radeon-settings": {
                    "chill.max_fps": 90, "chill.enabled": true, "vsync.mode": "always_off"
                }
            })),
            Some(60),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            ids(&custom),
            ["chill.enabled", "chill.max_fps", "vsync.mode"]
        );
    }

    #[test]
    fn what_makes_no_sense_is_refused_naming_the_problem() {
        let refused = |value: Value| selected(&extra(value), Some(60)).unwrap_err().to_string();
        assert!(refused(json!({"radeon-profile": "fastest"})).contains("competitive"));
        assert!(refused(json!({"radeon-profile": 3})).contains("radeon-profile"));
        assert!(refused(json!({"radeon-profile": "custom"})).contains("at least one"));
        assert!(
            refused(json!({"radeon-profile": "custom", "radeon-settings": {}}))
                .contains("at least one")
        );
        assert!(refused(json!({"radeon-settings": {"anti_lag.enabled": true}})).contains("custom"));
        assert!(
            refused(json!({"radeon-profile": "quality", "radeon-settings": {"a.b": 1}}))
                .contains("only with the custom")
        );
        for bad in ["nodot", "Anti_Lag.enabled", "a..b", "a.b c", ""] {
            let error =
                refused(json!({"radeon-profile": "custom", "radeon-settings": {bad: true}}));
            assert!(error.contains("not a Radeon setting id"), "{bad}: {error}");
        }
        for bad in [
            json!(1.5),
            json!(null),
            json!([1]),
            json!(""),
            json!({"a": 1}),
        ] {
            let error =
                refused(json!({"radeon-profile": "custom", "radeon-settings": {"a.b": bad}}));
            assert!(error.contains("a.b"), "{error}");
        }
        assert!(
            refused(json!({"radeon-profile": "custom", "radeon-settings": [1]})).contains("object")
        );
    }

    #[test]
    fn too_many_custom_settings_are_refused() {
        let many: Map<String, Value> = (0..=MAX_SETTINGS)
            .map(|n| (format!("a.s{n}"), json!(true)))
            .collect();
        let error = check(&extra(
            json!({"radeon-profile": "custom", "radeon-settings": many}),
        ))
        .unwrap_err()
        .to_string();
        assert!(error.contains("more than"), "{error}");
    }

    #[test]
    fn the_console_copy_of_the_table_is_the_host_table() {
        let copy = include_str!("../../web/src/lib/radeon_profiles.json");
        assert_eq!(copy, include_str!("radeon_profiles.json"));
    }

    #[test]
    fn a_launch_without_a_frame_rate_leaves_the_token_for_the_host_to_refuse() {
        let smooth = selected(&extra(json!({"radeon-profile": "smoothness"})), None)
            .unwrap()
            .unwrap();
        assert_eq!(smooth.without_stream_fps(), Some("frame_rate_target.fps"));
        let known = selected(&extra(json!({"radeon-profile": "smoothness"})), Some(90))
            .unwrap()
            .unwrap();
        assert_eq!(known.without_stream_fps(), None);
        let competitive = selected(&extra(json!({"radeon-profile": "competitive"})), None)
            .unwrap()
            .unwrap();
        assert_eq!(competitive.without_stream_fps(), None);
    }
}
