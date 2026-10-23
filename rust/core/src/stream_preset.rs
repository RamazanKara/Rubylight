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
        assert!(encoder_policy::amf_frame_cap_automatic(&nothing));
        assert_eq!(nothing.get("amd_quality", "speed"), "speed");
        assert!(!nothing.boolean("amd_preanalysis", false));
        for covered in &table().covered {
            let default = match covered.key.as_str() {
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
            assert!(
                !filled(text).values.contains_key("amd_max_frame_size"),
                "{text:?}"
            );
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
        let reference = filled("stream_preset = reference");
        assert_eq!(reference.get("amd_max_frame_size", ""), "0");
        assert!(!reference.values.contains_key("amd_quality"));
        // No preset sets the rate control: the measurements give no reason to change it.
        for preset in &table().presets {
            assert!(!preset.settings.contains_key("amd_rc"), "{}", preset.id);
        }
        assert!(
            !table()
                .covered
                .iter()
                .any(|covered| covered.key == "amd_rc")
        );
        assert!(!reference.values.contains_key("amd_rc"));
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
    fn overrides_name_a_preset_or_nothing() {
        use serde_json::json;
        for ok in [
            json!({}),
            json!({"max_bitrate": 5000}),
            json!({"stream_preset": "reference"}),
            json!({"stream_preset": "Lowest-Latency"}),
            json!({"stream_preset": ""}),
            json!({"stream_preset": null}),
        ] {
            check_overrides(&ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
        for bad in [
            json!({"stream_preset": "fastest"}),
            json!({"stream_preset": 1}),
            json!({"stream_preset": true}),
            json!({"stream_preset": ["reference"]}),
        ] {
            assert!(check_overrides(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_web_consoles_copy_of_the_table_is_the_same_file() {
        // build.ps1 builds a copy of rust/web, so the console cannot reach
        // ../core; it keeps its own copy of this table.
        assert_eq!(
            include_bytes!("stream_presets.json"),
            include_bytes!("../../web/src/lib/stream_presets.json"),
            "copy rust/core/src/stream_presets.json to rust/web/src/lib/stream_presets.json"
        );
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
            // Reference quality leaves the frame size to the driver and the rate control alone.
            let (reference_rate, reference) = amf("stream_preset = reference");
            assert_eq!(property(&reference_rate, size), None, "codec {codec}");
            assert_eq!(property(&reference, rc), default_rc, "codec {codec}");
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

    /// Every property the encoder policy resolves, by name, value and whether
    /// the encoder must accept it.
    fn resolved(config: &Config, stream: &Negotiated) -> Vec<(String, i64, bool)> {
        let mut all = encoder_policy::amf_rate_control(config, stream).unwrap();
        all.extend(encoder_policy::amf(config, stream).unwrap());
        all.iter()
            .map(|p| {
                let value = match p.value {
                    encoder_policy::Value::Integer(v) => v,
                    encoder_policy::Value::Boolean(b) => i64::from(b),
                };
                (p.name.clone(), value, p.required)
            })
            .collect()
    }

    #[test]
    fn balanced_resolves_to_exactly_the_policy_main_has_without_presets() {
        // encoder_policy.rs is unchanged from origin/main, so a configuration
        // with no preset resolves as main does. Balanced must match it value
        // for value, whatever the codec, bitrate, frame rate or dynamic range.
        for codec in 0..=2u8 {
            for (bitrate_kbps, fps, hdr) in [
                (40_000, 60, false),
                (8_000, 30, false),
                (120_000, 120, true),
                (300_000, 240, true),
            ] {
                let stream = Negotiated {
                    codec,
                    bitrate_kbps,
                    fps,
                    hdr,
                    ..Negotiated::default()
                };
                let main = resolved(&Config::default(), &stream);
                assert!(!main.is_empty());
                for text in [
                    "stream_preset = balanced",
                    "stream_preset = \"Balanced\"",
                    "stream_preset = fastest",
                ] {
                    assert_eq!(
                        resolved(&filled(text), &stream),
                        main,
                        "codec {codec} {bitrate_kbps} {fps} {text}"
                    );
                }
            }
        }
    }
}
