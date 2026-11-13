//! Named streaming presets over existing host settings: Lowest latency,
//! Balanced and Reference quality. The table is `stream_presets.json`, which
//! the web console reads too, so the console and the host cannot disagree.
//!
//! A preset only fills in the settings it covers that nobody set. A value
//! set in `sunshine.conf`, in a device's overrides or in an app's overrides
//! always wins, so the preset is a set of defaults, not a lock. A device or
//! an app can name its own preset (`stream_preset` in its overrides); the
//! most specific name (app, then device, then host) is the one that fills
//! the covered settings left unset by all three layers.
//!
//! Every value is one an existing setting already takes and that
//! `rust/PERFORMANCE.md` or `rust/PERFORMANCE_WORK.md` measured; the table's
//! `evidence` field names the section. Balanced sets nothing: it is today's
//! defaults. No preset adds an encoder knob. Pre-analysis, quality presets,
//! quality rate-control modes and the high-motion boost stay out of Reference
//! quality because the measurements show no consistent picture gain for their
//! encode time. Packed 4:4:4 is no Reference default either: on text and UI
//! fixtures it adds 1.2 to 2.0 ms of encode time on the RX 7900 XT
//! (`rust/PERFORMANCE.md`, "November 1: packed 4:4:4 as the Reference
//! default").
use crate::config::Config;
use serde::Deserialize;
use std::{collections::BTreeMap, sync::OnceLock};

/// The setting that names the preset.
pub const SETTING: &str = "stream_preset";

#[derive(Debug, Deserialize)]
pub struct Table {
    pub setting: String,
    /// The preset an unset or unreadable `stream_preset` means.
    pub default: String,
    /// Every setting any preset sets, with the value the host uses when it is
    /// unset. A preset leaves a covered setting it does not list at that value.
    pub covered: Vec<Covered>,
    pub presets: Vec<Preset>,
}
#[derive(Debug, Deserialize)]
pub struct Covered {
    pub key: String,
    pub default: String,
}
#[derive(Debug, Deserialize)]
pub struct Preset {
    pub id: String,
    pub label: String,
    pub description: String,
    pub settings: BTreeMap<String, String>,
    pub evidence: BTreeMap<String, String>,
}

pub fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| {
        serde_json::from_str(include_str!("stream_presets.json"))
            .expect("bundled stream_presets.json")
    })
}

/// The preset `id` names, ignoring case, quotes and `-` or space for `_`.
pub fn find(id: &str) -> Option<&'static Preset> {
    let id = id
        .trim()
        .trim_matches('"')
        .trim()
        .to_ascii_lowercase()
        .replace(['-', ' '], "_");
    table().presets.iter().find(|preset| preset.id == id)
}

/// The preset a configuration selects; Balanced when unset. An unreadable
/// name is reported once and also means Balanced.
pub fn selected(config: &Config) -> &'static Preset {
    let name = config.get(SETTING, "");
    if name.is_empty() {
        return find(&table().default).expect("default stream preset");
    }
    find(name).unwrap_or_else(|| {
        crate::config::invalid(SETTING, name);
        find(&table().default).expect("default stream preset")
    })
}

/// Fill the covered settings that `config` leaves unset with the values of
/// the preset it selects.
pub fn fill(config: &mut Config) {
    let preset = selected(config);
    for (key, value) in &preset.settings {
        if config.get(key, "").is_empty() {
            config.values.insert(key.clone(), value.clone());
        }
    }
}

/// Check the `stream_preset` of a device's or an app's overrides before they
/// are saved, as the host's own setting is: it must be a preset name, empty
/// (no preset of its own) or null (clears it). Anything else, including a
/// number or a boolean, is an error.
pub fn check_overrides(overrides: &serde_json::Value) -> anyhow::Result<()> {
    let Some(value) = overrides.get(SETTING) else {
        return Ok(());
    };
    match value {
        serde_json::Value::Null => Ok(()),
        serde_json::Value::String(name) if name.is_empty() || valid(name) => Ok(()),
        _ => anyhow::bail!("stream_preset must be lowest_latency, balanced or reference"),
    }
}

/// Whether `value` is a preset name the settings API accepts.
pub fn valid(value: &str) -> bool {
    find(value).is_some()
}

#[cfg(test)]
mod tests;
