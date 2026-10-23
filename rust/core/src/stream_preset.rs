//! Named streaming presets over existing host settings: Lowest latency,
//! Balanced and Reference quality. The table is `stream_presets.json`, which
//! the web console reads too, so the console and the host cannot disagree.
//!
//! A preset only fills in the settings it covers. A value set in
//! `sunshine.conf` always wins, so the preset is a set of defaults, not a
//! lock. A device or an app can name its own preset (`stream_preset` in its
//! overrides); that replaces the host's values for the covered settings, and
//! the device's or app's own overrides still win over it.
//!
//! Every value is one an existing setting already takes and that
//! `rust/PERFORMANCE.md` or `rust/PERFORMANCE_WORK.md` measured; the table's
//! `evidence` field names the section. Balanced sets nothing: it is today's
//! defaults. No preset adds an encoder knob. Pre-analysis, quality presets,
//! quality rate-control modes and the high-motion boost stay out of Reference
//! quality because the measurements show no consistent picture gain for their
//! encode time. Packed 4:4:4 as a Reference default waits for measurements on
//! text fixtures.
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

/// Make `id` the preset for the covered settings: set what it lists and
/// drop the rest so they fall back to their defaults. For a device or an app
/// that picks a preset over the host's. Returns false for an unknown `id`.
pub fn select(config: &mut Config, id: &str) -> bool {
    let Some(preset) = find(id) else {
        crate::config::invalid(SETTING, id);
        return false;
    };
    for covered in &table().covered {
        match preset.settings.get(&covered.key) {
            Some(value) => {
                config.values.insert(covered.key.clone(), value.clone());
            }
            None => {
                config.values.remove(&covered.key);
            }
        }
    }
    true
}

/// Whether `value` is a preset name the settings API accepts.
pub fn valid(value: &str) -> bool {
    find(value).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoder_policy;
    use crate::rtsp::Negotiated;

    fn config(text: &str) -> Config {
        Config::parse(text).unwrap()
    }
    fn filled(text: &str) -> Config {
        let mut config = config(text);
        fill(&mut config);
        config
    }

    #[test]
    fn the_table_covers_only_overridable_settings_and_lists_only_covered_ones() {
        let table = table();
        assert_eq!(table.setting, SETTING);
        assert!(crate::config::override_allowed(SETTING));
        let ids: Vec<_> = table.presets.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["lowest_latency", "balanced", "reference"]);
        for covered in &table.covered {
            assert!(
                crate::config::override_allowed(&covered.key),
                "{}",
                covered.key
            );
        }
        for preset in &table.presets {
            for key in preset.settings.keys() {
                assert!(
                    table.covered.iter().any(|covered| &covered.key == key),
                    "{} sets {key}, which is not covered",
                    preset.id
                );
                // Each value is backed by a measurement that the table cites.
                let evidence = preset.evidence.get(key).map_or("", String::as_str);
                assert!(evidence.contains("PERFORMANCE"), "{} {key}", preset.id);
            }
        }
    }

    #[test]
    fn balanced_sets_nothing_so_it_is_todays_defaults() {
        let balanced = find("balanced").unwrap();
        assert!(balanced.settings.is_empty());
        assert_eq!(table().default, "balanced");
        for text in [
            "",
            "stream_preset = balanced",
            "sunshine_name = Host\nmax_bitrate = 5000",
        ] {
            let before = config(text);
            assert_eq!(filled(text).values, before.values, "{text:?}");
        }
        // Today's defaults are what the table says an unset covered setting is.
        let nothing = Config::default();
        assert_eq!(nothing.get("amd_rc", "vbr_latency"), "vbr_latency");
        assert!(encoder_policy::amf_frame_cap_automatic(&nothing));
        assert_eq!(nothing.get("amd_quality", "speed"), "speed");
        assert!(!nothing.boolean("amd_preanalysis", false));
        for covered in &table().covered {
            let default = match covered.key.as_str() {
                "amd_rc" => "vbr_latency",
                "amd_max_frame_size" => "auto",
                "amd_quality" => "speed",
                "amd_preanalysis" => "false",
                other => panic!("{other} is covered but this test does not know its default"),
            };
            assert_eq!(covered.default, default);
        }
    }

    #[test]
    fn unset_or_unreadable_names_mean_balanced() {
        for text in [
            "",
            "stream_preset = custom",
            "stream_preset = \"\"",
            "stream_preset = fastest",
        ] {
            assert_eq!(selected(&config(text)).id, "balanced", "{text:?}");
            assert!(!filled(text).values.contains_key("amd_rc"), "{text:?}");
        }
        for (text, id) in [
            ("stream_preset = reference", "reference"),
            ("stream_preset = Lowest-Latency", "lowest_latency"),
            ("stream_preset = \"Reference\"", "reference"),
            ("stream_preset = lowest latency", "lowest_latency"),
        ] {
            assert_eq!(selected(&config(text)).id, id, "{text:?}");
        }
    }

    #[test]
    fn each_preset_fills_the_settings_nobody_set() {
        let latency = filled("stream_preset = lowest_latency");
        assert_eq!(latency.get("amd_max_frame_size", ""), "1");
        assert_eq!(latency.get("amd_quality", ""), "speed");
        assert!(latency.values.contains_key("amd_preanalysis"));
        assert!(!latency.boolean("amd_preanalysis", true));
        assert!(!latency.values.contains_key("amd_rc"));
        let reference = filled("stream_preset = reference");
        assert_eq!(reference.get("amd_rc", ""), "cbr");
        assert_eq!(reference.get("amd_max_frame_size", ""), "0");
        assert!(!reference.values.contains_key("amd_quality"));
        // The preset name itself stays in the configuration.
        assert_eq!(reference.get(SETTING, ""), "reference");
    }

    #[test]
    fn a_value_set_in_the_configuration_beats_the_preset() {
        let reference = filled("stream_preset = reference\namd_rc = hqvbr\namd_max_frame_size = 2");
        assert_eq!(reference.get("amd_rc", ""), "hqvbr");
        assert_eq!(reference.get("amd_max_frame_size", ""), "2");
        let latency = filled(
            "stream_preset = lowest_latency\namd_max_frame_size = auto\namd_preanalysis = true",
        );
        assert_eq!(latency.get("amd_max_frame_size", ""), "auto");
        assert!(latency.boolean("amd_preanalysis", false));
        assert_eq!(latency.get("amd_quality", ""), "speed");
    }

    #[test]
    fn selecting_a_preset_replaces_the_covered_settings_only() {
        let mut config = config(
            "amd_rc = hqvbr\namd_quality = quality\namd_max_frame_size = 4\nmax_bitrate = 9000\namd_vbaq = disabled",
        );
        assert!(select(&mut config, "reference"));
        assert_eq!(config.get("amd_rc", ""), "cbr");
        assert_eq!(config.get("amd_max_frame_size", ""), "0");
        assert!(!config.values.contains_key("amd_quality"));
        assert_eq!(config.get("max_bitrate", ""), "9000");
        assert_eq!(config.get("amd_vbaq", ""), "disabled");
        assert!(select(&mut config, "balanced"));
        for covered in &table().covered {
            assert!(!config.values.contains_key(&covered.key), "{}", covered.key);
        }
        let before = config.values.clone();
        assert!(!select(&mut config, "fastest"));
        assert_eq!(config.values, before);
    }

    fn stream(codec: u8) -> Negotiated {
        Negotiated {
            codec,
            bitrate_kbps: 40_000,
            fps: 60,
            ..Negotiated::default()
        }
    }
    fn property(properties: &[encoder_policy::Property], name: &str) -> Option<i64> {
        properties
            .iter()
            .find(|p| p.name == name)
            .map(|p| match p.value {
                encoder_policy::Value::Integer(v) => v,
                encoder_policy::Value::Boolean(b) => i64::from(b),
            })
    }

    #[test]
    fn the_presets_reach_the_amf_properties() {
        // One frame at 40 Mbps and 60 fps is 666,667 bits.
        let frames = |n: f64| (n * 40_000_000.0 / 60.0).round() as i64;
        let (one_frame, two_frames) = (frames(1.), frames(2.));
        for codec in 0..=2u8 {
            let size = if codec == 2 {
                "Av1MaxCompressedFrameSize"
            } else {
                if codec == 1 {
                    "HevcMaxAUSize"
                } else {
                    "MaxAUSize"
                }
            };
            let rc = [
                "RateControlMethod",
                "HevcRateControlMethod",
                "Av1RateControlMethod",
            ][codec as usize];
            let stream = stream(codec);
            let amf = |text: &str| {
                let config = filled(text);
                (
                    encoder_policy::amf_rate_control(&config, &stream).unwrap(),
                    encoder_policy::amf(&config, &stream).unwrap(),
                )
            };
            // Balanced starts automatic caps at two frames and leaves rate control to the default.
            let (balanced_rate, balanced) = amf("");
            assert_eq!(
                property(&balanced_rate, size),
                Some(two_frames),
                "codec {codec}"
            );
            assert_eq!(
                amf("stream_preset = balanced")
                    .0
                    .iter()
                    .map(|p| (&p.name, p.required))
                    .collect::<Vec<_>>(),
                balanced_rate
                    .iter()
                    .map(|p| (&p.name, p.required))
                    .collect::<Vec<_>>(),
            );
            let default_rc = property(&balanced, rc);
            assert_eq!(
                default_rc,
                Some(if codec == 0 { 3 } else { 1 }),
                "codec {codec}"
            );
            // Lowest latency caps every frame at one frame's share.
            let (latency_rate, latency) = amf("stream_preset = lowest_latency");
            assert_eq!(
                property(&latency_rate, size),
                Some(one_frame),
                "codec {codec}"
            );
            assert_eq!(property(&latency, rc), default_rc, "codec {codec}");
            assert_eq!(
                property(
                    &latency,
                    [
                        "EnablePreAnalysis",
                        "HevcEnablePreAnalysis",
                        "Av1EnablePreAnalysis"
                    ][codec as usize]
                ),
                Some(0)
            );
            // Reference quality asks for constant bitrate and leaves the frame size to the driver.
            let (reference_rate, reference) = amf("stream_preset = reference");
            assert_eq!(property(&reference_rate, size), None, "codec {codec}");
            assert_eq!(
                property(&reference, rc),
                Some(if codec == 0 { 1 } else { 3 }),
                "codec {codec}"
            );
            // An explicit value still wins.
            let (explicit_rate, explicit) =
                amf("stream_preset = reference\namd_rc = vbr_peak\namd_max_frame_size = 4");
            assert_eq!(
                property(&explicit_rate, size),
                Some(frames(4.)),
                "codec {codec}"
            );
            assert_eq!(property(&explicit, rc), Some(2), "codec {codec}");
        }
    }
}
