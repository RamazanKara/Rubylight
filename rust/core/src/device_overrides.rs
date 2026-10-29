//! An app's settings for one device (issue #63): `device-overrides` maps a paired device's
//! uuid to the app settings that differ on that device, so one game can stream at 60 to the TV
//! and at 120 to a handheld. A device's block holds the app's own keys: `config-overrides`, the
//! Radeon profile (`radeon-profile`, `radeon-settings`) and the managed `game-files`.
//!
//! Precedence, wherever the host resolves a stream's settings: app × device, then app, then
//! device, then host. The block's `config-overrides` win over the app's key by key (an empty
//! value is none of the block's own, as at every level). Its Radeon profile, when it names one
//! (an empty name for none), replaces the app's together with its settings. Its game files
//! replace the app's entry for the same file, section and key and add the others. The block of
//! a device that is not paired is kept and never used.
use crate::{config, game_files, radeon_profile, state::App, stream_preset};
use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// The app key that holds the blocks, by device uuid.
pub const SETTING: &str = "device-overrides";
/// The app's general overrides, in the app and in a block.
pub const CONFIG: &str = "config-overrides";
/// The app keys a block may hold.
pub const KEYS: [&str; 4] = [
    CONFIG,
    radeon_profile::PROFILE,
    radeon_profile::SETTINGS,
    game_files::SETTING,
];

/// `app` as `device` streams it: its block in `device-overrides` laid over the app's own
/// settings. The result holds no `device-overrides`.
pub fn for_device(app: &App, device: &str) -> App {
    let mut merged = app.clone();
    let blocks = merged.extra.remove(SETTING);
    let Some(block) = blocks
        .as_ref()
        .filter(|_| !device.is_empty())
        .and_then(|blocks| blocks.get(device))
        .and_then(Value::as_object)
    else {
        return merged;
    };
    if let Some(own) = block.get(CONFIG).and_then(Value::as_object) {
        let mut overrides: Map<String, Value> = merged
            .extra
            .get(CONFIG)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        for (key, value) in own {
            // No value of the block's own, as an app's "" or null preset: the app's stays.
            if value.as_str() == Some("") || (key == stream_preset::SETTING && value.is_null()) {
                continue;
            }
            // The app's value under an earlier name of the key gives way too.
            let name = config::current_name(key);
            overrides.retain(|other, _| config::current_name(other) != name);
            overrides.insert(name.to_owned(), value.clone());
        }
        merged.extra.insert(CONFIG.into(), Value::Object(overrides));
    }
    if let Some(profile) = block
        .get(radeon_profile::PROFILE)
        .filter(|profile| !profile.is_null())
    {
        merged
            .extra
            .insert(radeon_profile::PROFILE.into(), profile.clone());
        match block
            .get(radeon_profile::SETTINGS)
            .filter(|settings| !settings.is_null())
        {
            Some(settings) => {
                merged
                    .extra
                    .insert(radeon_profile::SETTINGS.into(), settings.clone());
            }
            None => {
                merged.extra.remove(radeon_profile::SETTINGS);
            }
        }
    }
    if let Some(files) = block.get(game_files::SETTING).and_then(Value::as_array) {
        let mut list = merged
            .extra
            .get(game_files::SETTING)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for entry in files {
            match list
                .iter()
                .position(|listed| game_files::same_key(listed, entry))
            {
                Some(at) => list[at] = entry.clone(),
                None => list.push(entry.clone()),
            }
        }
        merged
            .extra
            .insert(game_files::SETTING.into(), Value::Array(list));
    }
    merged
}

/// Check an app's blocks before it is saved: an object of blocks by device id, each holding
/// only [`KEYS`], with general overrides a single device may have, a known Radeon profile and
/// valid game files.
pub fn check(extra: &BTreeMap<String, Value>) -> Result<()> {
    let blocks = match extra.get(SETTING) {
        None | Some(Value::Null) => return Ok(()),
        Some(Value::Object(blocks)) => blocks,
        Some(_) => bail!("{SETTING} must map a device's id to its settings"),
    };
    for (device, block) in blocks {
        check_block(block).with_context(|| format!("{SETTING} for device {device}"))?;
        if device.trim().is_empty() {
            bail!("{SETTING} names a device without an id");
        }
    }
    Ok(())
}

fn check_block(block: &Value) -> Result<()> {
    let Value::Object(block) = block else {
        bail!("its settings must be an object");
    };
    if let Some(key) = block.keys().find(|key| !KEYS.contains(&key.as_str())) {
        bail!(
            "{key} cannot be set for one device; a device's block holds {}",
            KEYS.join(", ")
        );
    }
    if let Some(overrides) = block.get(CONFIG) {
        let Value::Object(map) = overrides else {
            bail!("{CONFIG} must be an object");
        };
        stream_preset::check_overrides(overrides)?;
        if let Some(key) = map.keys().find(|key| {
            !config::removed(key) && !config::override_allowed(config::current_name(key))
        }) {
            bail!("{key} is a host-wide setting and cannot be set for one device");
        }
    }
    let radeon: BTreeMap<String, Value> = [radeon_profile::PROFILE, radeon_profile::SETTINGS]
        .into_iter()
        .filter_map(|key| Some((key.to_owned(), block.get(key)?.clone())))
        .collect();
    radeon_profile::check(&radeon)?;
    if let Some(list) = block.get(game_files::SETTING) {
        game_files::parse(list)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn app(extra: Value) -> App {
        let mut app: App =
            serde_json::from_value(json!({"name": "RDR2", "cmd": "rdr2.exe"})).unwrap();
        app.extra = serde_json::from_value(extra).unwrap();
        app
    }

    #[test]
    fn a_devices_block_wins_over_the_app_key_by_key() {
        let app = app(json!({
            "config-overrides": {"frame_limiter_fps_limit": 59, "gamepad": "ds4", "stream_preset": "reference"},
            "device-overrides": {
                "handheld": {"config-overrides": {"frame_limiter_fps_limit": 120, "stream_preset": "lowest_latency", "gamepad": ""}},
                "tv": {"config-overrides": {"stream_preset": null}}
            }
        }));
        let handheld = for_device(&app, "handheld");
        assert_eq!(
            handheld.extra[CONFIG],
            json!({"frame_limiter_fps_limit": 120, "gamepad": "ds4", "stream_preset": "lowest_latency"})
        );
        assert!(!handheld.extra.contains_key(SETTING));
        // A null preset and an empty value are none of the block's own.
        assert_eq!(for_device(&app, "tv").extra[CONFIG], app.extra[CONFIG]);
        // Another device, and an app without blocks, get the app as it is.
        let phone = for_device(&app, "phone");
        assert_eq!(phone.extra[CONFIG], app.extra[CONFIG]);
        assert!(!phone.extra.contains_key(SETTING));
        assert_eq!(for_device(&app, "").extra[CONFIG], app.extra[CONFIG]);
        // A block for a device the app has no overrides of its own for still applies.
        let bare = self::app(
            json!({"device-overrides": {"tv": {"config-overrides": {"max_bitrate": 20000}}}}),
        );
        assert_eq!(
            for_device(&bare, "tv").extra[CONFIG],
            json!({"max_bitrate": 20000})
        );
    }

    #[test]
    fn an_earlier_name_of_a_key_gives_way_to_the_blocks() {
        let app = app(json!({
            "config-overrides": {"rtx_hdr_peak_brightness": 1000},
            "device-overrides": {"tv": {"config-overrides": {"hdr_peak_brightness": 600}}}
        }));
        assert_eq!(
            for_device(&app, "tv").extra[CONFIG],
            json!({"hdr_peak_brightness": 600})
        );
    }

    #[test]
    fn a_devices_radeon_profile_replaces_the_apps_with_its_settings() {
        let app = app(json!({
            "radeon-profile": "custom",
            "radeon-settings": {"chill.enabled": true},
            "device-overrides": {
                "tv": {"radeon-profile": "smoothness"},
                "phone": {"radeon-profile": ""},
                "handheld": {"config-overrides": {}}
            }
        }));
        let tv = for_device(&app, "tv");
        assert_eq!(tv.extra[radeon_profile::PROFILE], "smoothness");
        assert!(!tv.extra.contains_key(radeon_profile::SETTINGS));
        let phone = for_device(&app, "phone");
        assert!(
            radeon_profile::selected(&phone.extra, Some(60))
                .unwrap()
                .is_none()
        );
        let handheld = for_device(&app, "handheld");
        assert_eq!(handheld.extra[radeon_profile::PROFILE], "custom");
        assert_eq!(
            handheld.extra[radeon_profile::SETTINGS],
            json!({"chill.enabled": true})
        );
    }

    #[test]
    fn a_devices_game_files_replace_the_same_key_and_add_the_rest() {
        let limit = |value: Value| json!({"path": "OptiScaler.ini", "section": "Framerate", "key": "FramerateLimit", "value": value});
        let app = app(json!({
            "game-files": [limit(json!("{stream_fps-1}")), {"path": "a.ini", "section": "", "key": "k", "value": 1}],
            "device-overrides": {"handheld": {"game-files": [
                {"path": "optiscaler.INI", "section": "framerate", "key": "frameratelimit", "value": "{stream_fps}"},
                {"path": "b.ini", "section": "", "key": "k", "value": 2}
            ]}}
        }));
        let entries = game_files::entries(&for_device(&app, "handheld").extra).unwrap();
        let values: Vec<(&str, Value)> = entries
            .iter()
            .map(|e| (e.path.as_str(), e.value.clone()))
            .collect();
        assert_eq!(
            values,
            [
                ("optiscaler.INI", json!("{stream_fps}")),
                ("a.ini", json!(1)),
                ("b.ini", json!(2))
            ]
        );
        assert_eq!(
            game_files::entries(&for_device(&app, "tv").extra)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn blocks_are_checked_when_the_app_is_saved() {
        let check = |blocks: Value| super::check(&app(json!({"device-overrides": blocks})).extra);
        check(json!({
            "handheld": {
                "config-overrides": {"frame_limiter_fps_limit": 120, "stream_preset": "lowest_latency"},
                "radeon-profile": "smoothness",
                "game-files": [{"path": "OptiScaler.ini", "section": "Framerate", "key": "FramerateLimit", "value": "{stream_fps}"}]
            },
            // A device that is no longer paired keeps its block.
            "gone": {}
        }))
        .unwrap();
        check(json!(null)).unwrap();
        assert!(super::check(&app(json!({})).extra).is_ok());
        for bad in [
            json!([]),
            json!({"tv": []}),
            json!({"tv": {"cmd": "other.exe"}}),
            json!({"tv": {"config-overrides": {"port": 48000}}}),
            json!({"tv": {"config-overrides": {"stream_preset": "fastest"}}}),
            json!({"tv": {"config-overrides": "x"}}),
            json!({"tv": {"radeon-profile": "turbo"}}),
            json!({"tv": {"radeon-settings": {"chill.enabled": true}}}),
            json!({"tv": {"game-files": [{"path": "x.txt", "key": "k", "value": 1}]}}),
            json!({"": {}}),
        ] {
            assert!(check(bad.clone()).is_err(), "{bad}");
        }
        let error = check(json!({"tv": {"config-overrides": {"port": 48000}}})).unwrap_err();
        assert!(format!("{error:#}").contains("device tv"), "{error:#}");
    }
}
