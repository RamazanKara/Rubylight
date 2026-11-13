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
    let nvenc = crate::nvenc::Tuning::new(&nothing, &stream(1)).unwrap();
    assert_eq!((nvenc.vbv_increase, nvenc.preset), (0, 1));
    assert!(!nvenc.temporal_aq);
    for covered in &table().covered {
        let default = match covered.key.as_str() {
            "amd_max_frame_size" => "auto",
            "amd_quality" => "speed",
            "amd_preanalysis" => "false",
            "nvenc_vbv_increase" => "0",
            "nvenc_preset" => "1",
            "nvenc_temporal_aq" => "false",
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
    let latency =
        filled("stream_preset = lowest_latency\namd_max_frame_size = auto\namd_preanalysis = true");
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
        include_bytes!("../stream_presets.json"),
        include_bytes!("../../../web/src/lib/stream_presets.json"),
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

#[test]
fn every_amf_setting_a_preset_fills_has_its_nvenc_counterpart() {
    // AMF setting and value -> NVENC setting and value, as docs/configuration.md lists them.
    let counterparts = [
        ("amd_max_frame_size", "1", "nvenc_vbv_increase", "0"),
        ("amd_max_frame_size", "0", "nvenc_vbv_increase", "400"),
        ("amd_quality", "speed", "nvenc_preset", "1"),
        ("amd_preanalysis", "false", "nvenc_temporal_aq", "false"),
    ];
    for preset in &table().presets {
        let amd = preset.settings.keys().filter(|k| k.starts_with("amd_"));
        let nvenc = preset.settings.keys().filter(|k| k.starts_with("nvenc_"));
        assert_eq!(amd.count(), nvenc.count(), "{}", preset.id);
        for (key, value) in &preset.settings {
            if !key.starts_with("amd_") {
                continue;
            }
            let (_, _, nv_key, nv_value) = counterparts
                .iter()
                .find(|(k, v, ..)| k == key && v == value)
                .unwrap_or_else(|| panic!("{} {key}={value} has no NVENC counterpart", preset.id));
            assert_eq!(
                preset.settings.get(*nv_key).map(String::as_str),
                Some(*nv_value),
                "{}",
                preset.id
            );
        }
    }
}

#[test]
fn the_presets_reach_the_nvenc_tuning() {
    for codec in 0..=2u8 {
        let stream = stream(codec);
        let tuning = |text: &str| crate::nvenc::Tuning::new(&filled(text), &stream).unwrap();
        // One frame at 40 Mbps and 60 fps is 666,667 bits.
        let one_frame = tuning("").vbv(stream.bitrate_kbps, 60_000);
        assert_eq!(one_frame, 666_666, "codec {codec}");
        // Lowest latency is P1, no temporal AQ and a one-frame VBV, as the defaults.
        let latency = tuning("stream_preset = lowest_latency");
        assert_eq!(latency.preset, 1);
        assert!(!latency.temporal_aq);
        assert_eq!(latency.vbv(stream.bitrate_kbps, 60_000), one_frame);
        // Reference quality loosens the VBV to five frames and leaves the rest alone.
        let reference = tuning("stream_preset = reference");
        assert_eq!(
            reference.vbv(stream.bitrate_kbps, 60_000),
            one_frame * 5 + 3
        );
        assert_eq!(reference.preset, 1);
        assert_eq!(reference.rate_control, tuning("").rate_control);
        // An explicit value still wins.
        let explicit =
            tuning("stream_preset = reference\nnvenc_vbv_increase = 100\nnvenc_preset = 4");
        assert_eq!(explicit.vbv(stream.bitrate_kbps, 60_000), 1_333_333);
        assert_eq!(explicit.preset, 4);
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
