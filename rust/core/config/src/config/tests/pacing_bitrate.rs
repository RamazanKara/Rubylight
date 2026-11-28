//! Tests for packet pacing, keyframe-loss pacing, and adaptive bitrate folding.

#[test]
fn packet_pacing_reads_automatic_always_off_and_a_fixed_rate() {
    use super::PacketPacing as P;
    for (text, expected) in [
        ("", P::Auto),
        ("packet_pacing=auto", P::Auto),
        ("packet_pacing=AUTO", P::Auto),
        ("packet_pacing=always", P::Always),
        ("packet_pacing=off", P::Off),
        ("packet_pacing=1", P::Fixed(1)),
        ("packet_pacing=80000", P::Fixed(80000)),
        ("packet_pacing=0x1388", P::Fixed(5000)),
        // The earlier pacing limit read 0 and negative rates as no rate.
        ("packet_pacing=0", P::Auto),
        ("packet_pacing=-1", P::Auto),
        ("packet_pacing=invalid", P::Auto),
    ] {
        let config = super::Config::parse(text).unwrap();
        assert_eq!(config.packet_pacing(), expected, "{text}");
    }
    assert_eq!(super::Config::default().manual_pacing_kbps(), None);
    assert_eq!(
        super::Config::parse("packet_pacing=80000")
            .unwrap()
            .manual_pacing_kbps(),
        Some(80000)
    );
    assert!(super::override_allowed("packet_pacing"));
    assert!(!super::override_allowed("host_adaptive_pacing"));
    assert!(!super::override_allowed("wireless_adaptive_pacing"));
    assert!(!super::override_allowed("pacing_max_bitrate_kbps"));
}

#[test]
fn packet_pacing_picks_the_adaptive_controller_like_the_retired_switches_did() {
    let cases = [
        ("", [false, false, false, false]),
        ("packet_pacing=auto", [false, false, false, false]),
        ("packet_pacing=always", [true, true, true, true]),
        ("packet_pacing=off", [false, false, false, false]),
        ("packet_pacing=80000", [false, false, false, false]),
    ];
    for (text, expected) in cases {
        let config = super::Config::parse(text).unwrap();
        for (codec, enabled) in expected.into_iter().enumerate() {
            assert_eq!(
                config.adaptive_packet_pacing(codec as u8, false),
                enabled,
                "codec {codec}: {text}"
            );
        }
        assert!(!config.adaptive_packet_pacing(u8::MAX, false));
        assert!(!config.adaptive_packet_pacing(u8::MAX, true));
    }
}

#[test]
fn a_wireless_client_gets_adaptive_pacing_on_h26x_and_av1_unless_something_owns_pacing() {
    // (config, [H.264, HEVC, AV1, PyroWave] adaptive, keyframe-loss pacing, same for wired)
    let on = [true, true, true, false];
    let off = [false; 4];
    let all = [true; 4];
    let cases = [
        ("", on, [false, false, false, true], off, [true; 4]),
        (
            "packet_pacing=auto",
            on,
            [false, false, false, true],
            off,
            [true; 4],
        ),
        (
            "packet_pacing=invalid",
            on,
            [false, false, false, true],
            off,
            [true; 4],
        ),
        ("packet_pacing=off", off, [true; 4], off, [true; 4]),
        // Always still turns it on for everything, wired or wireless.
        ("packet_pacing=always", all, [false; 4], all, [false; 4]),
        // A fixed rate owns pacing.
        ("packet_pacing=80000", off, [false; 4], off, [false; 4]),
        (
            "packet_pacing=0",
            on,
            [false, false, false, true],
            off,
            [true; 4],
        ),
        // Keyframe-loss pacing can still be switched off.
        (
            "keyframe_loss_pacing=false",
            on,
            [false; 4],
            off,
            [false; 4],
        ),
    ];
    for (text, wireless, wireless_keyframe, wired, wired_keyframe) in cases {
        let config = super::Config::parse(text).unwrap();
        for codec in 0..4u8 {
            let i = usize::from(codec);
            assert_eq!(
                config.adaptive_packet_pacing(codec, true),
                wireless[i],
                "wireless codec {codec}: {text}"
            );
            assert_eq!(
                config.keyframe_loss_pacing(codec, true),
                wireless_keyframe[i],
                "wireless keyframe codec {codec}: {text}"
            );
            assert_eq!(
                config.adaptive_packet_pacing(codec, false),
                wired[i],
                "wired codec {codec}: {text}"
            );
            assert_eq!(
                config.keyframe_loss_pacing(codec, false),
                wired_keyframe[i],
                "wired keyframe codec {codec}: {text}"
            );
        }
    }
    // PyroWave plus a wireless client stays on its calibrated pace.
    let default = super::Config::default();
    assert!(!default.adaptive_packet_pacing(3, true));
    assert!(default.keyframe_loss_pacing(3, true));
}

#[test]
fn keyframe_loss_pacing_is_on_unless_a_fixed_rate_or_adaptation_owns_pacing() {
    for (text, enabled) in [
        ("", true),
        ("keyframe_loss_pacing=true", true),
        ("keyframe_loss_pacing=invalid", true),
        ("keyframe_loss_pacing=false", false),
        ("packet_pacing=0", true),
        ("packet_pacing=auto", true),
        ("packet_pacing=off", true),
        ("packet_pacing=80000", false),
        ("packet_pacing=always", false),
        // The retired host bitrate controller no longer turns it off.
        ("host_adaptive_bitrate=true", true),
    ] {
        let config = super::Config::parse(text).unwrap();
        assert_eq!(config.keyframe_loss_pacing(0, false), enabled, "{text}");
    }
    assert!(super::override_allowed("keyframe_loss_pacing"));
}

#[test]
fn link_rate_adaptation_is_on_unless_disabled() {
    for (text, enabled) in [
        ("", true),
        ("bitrate_adaptation=on", true),
        ("bitrate_adaptation=delay", true),
        ("bitrate_adaptation=off", false),
        ("bitrate_adaptation=\"off\"", false),
        ("bitrate_adaptation=invalid", true),
        ("link_rate_adaptation=true", true),
        ("link_rate_adaptation=invalid", true),
        ("link_rate_adaptation=false", false),
        // The retired host bitrate controller no longer turns it off.
        ("host_adaptive_bitrate=true", true),
        (
            "host_adaptive_bitrate=true\nlink_rate_adaptation=false",
            false,
        ),
        ("host_adaptive_bitrate=false", true),
        ("host_adaptive_pacing=true", true),
    ] {
        let config = super::Config::parse(text).unwrap();
        assert_eq!(config.link_rate_adaptation(), enabled, "{text}");
    }
    assert!(super::override_allowed("bitrate_adaptation"));
    assert!(!super::override_allowed("link_rate_adaptation"));
}

#[test]
fn congestion_control_is_off_unless_set() {
    for (text, enabled) in [
        ("", false),
        ("congestion_control=true", true),
        ("congestion_control=invalid", false),
        ("congestion_control=false", false),
        ("congestion_control=true\nhost_adaptive_bitrate=true", true),
        ("congestion_control=true\nlink_rate_adaptation=false", true),
        ("bitrate_adaptation=delay", true),
        ("bitrate_adaptation=on", false),
        ("bitrate_adaptation=off", false),
        // A value saved under the current name wins over the switches.
        ("bitrate_adaptation=on\ncongestion_control=true", false),
    ] {
        let config = super::Config::parse(text).unwrap();
        assert_eq!(config.congestion_control(), enabled, "{text}");
    }
    assert!(!super::override_allowed("congestion_control"));
}

/// The pacing rate and the two adaptive pacing switches fold into Packet
/// pacing: a positive rate is the rate, adaptive pacing for everything is
/// `always`, wireless pacing off alone is `off`, the rest the default.
#[test]
fn the_pacing_settings_fold_into_packet_pacing() {
    use super::{Config, PacketPacing as P, folded_packet_pacing as fold, migrate_overrides};
    assert_eq!(fold(None, None, None), None);
    for (rate, host, wireless, folded) in [
        (Some("80000"), None, None, Some("80000")),
        (Some("80000"), Some("true"), Some("false"), Some("80000")),
        (Some("0"), None, None, None),
        (Some("-1"), Some("true"), None, Some("always")),
        (None, Some("true"), None, Some("always")),
        (None, Some("true"), Some("false"), Some("always")),
        (None, Some("false"), Some("false"), Some("off")),
        (None, None, Some("false"), Some("off")),
        (None, None, Some("true"), None),
        (None, Some("false"), None, None),
        (None, Some("invalid"), Some("invalid"), None),
    ] {
        assert_eq!(
            fold(rate, host, wireless).as_deref(),
            folded,
            "{rate:?} {host:?} {wireless:?}"
        );
    }
    for (text, expected) in [
        ("", P::Auto),
        ("pacing_max_bitrate_kbps=120000", P::Fixed(120000)),
        ("host_adaptive_pacing=true", P::Always),
        ("wireless_adaptive_pacing=false", P::Off),
        (
            "host_adaptive_pacing=true
pacing_max_bitrate_kbps=80000",
            P::Fixed(80000),
        ),
        (
            "packet_pacing=off
host_adaptive_pacing=true",
            P::Off,
        ),
    ] {
        let config = Config::parse(text).unwrap();
        assert_eq!(config.packet_pacing(), expected, "{text}");
        for key in [
            "pacing_max_bitrate_kbps",
            "host_adaptive_pacing",
            "wireless_adaptive_pacing",
        ] {
            assert!(!config.text().contains(key), "{text}");
        }
    }
    // An app's or device's overrides fold at migration where their own
    // settings decide the result.
    for (layer, expected) in [
        (
            serde_json::json!({"pacing_max_bitrate_kbps": 80000}),
            Some("80000"),
        ),
        (
            serde_json::json!({"host_adaptive_pacing": true}),
            Some("always"),
        ),
        (
            serde_json::json!({"host_adaptive_pacing": false, "wireless_adaptive_pacing": "false"}),
            Some("off"),
        ),
        (
            serde_json::json!({"host_adaptive_pacing": false, "wireless_adaptive_pacing": true}),
            None,
        ),
        (
            serde_json::json!({"host_adaptive_pacing": true, "packet_pacing": "off"}),
            Some("off"),
        ),
        (serde_json::json!({"fec_percentage": 20}), None),
    ] {
        let mut layer = layer.as_object().unwrap().clone();
        migrate_overrides(&mut layer);
        assert_eq!(
            layer.get("packet_pacing").and_then(|v| v.as_str()),
            expected,
            "{layer:?}"
        );
        assert!(!layer.contains_key("host_adaptive_pacing"));
        assert!(!layer.contains_key("wireless_adaptive_pacing"));
        assert!(!layer.contains_key("pacing_max_bitrate_kbps"));
    }
}

/// A layer holding only some of the pacing switches keeps the old combined
/// behaviour: it stays as saved at migration and folds over the layers below
/// when it is applied.
#[test]
fn a_layer_with_some_pacing_switches_folds_over_the_layers_below() {
    use super::{Config, migrate_overrides};
    // (what the host holds, the layer, Packet pacing the layer applies)
    for (host, layer, expected) in [
        // Adaptive pacing on for everyone: a device's wireless switch off
        // does not turn it off, as before.
        (
            "host_adaptive_pacing=true",
            serde_json::json!({"wireless_adaptive_pacing": false}),
            "always",
        ),
        (
            "",
            serde_json::json!({"wireless_adaptive_pacing": false}),
            "off",
        ),
        (
            "wireless_adaptive_pacing=false",
            serde_json::json!({"wireless_adaptive_pacing": true}),
            "auto",
        ),
        (
            "wireless_adaptive_pacing=false",
            serde_json::json!({"host_adaptive_pacing": true}),
            "always",
        ),
        (
            "host_adaptive_pacing=true",
            serde_json::json!({"host_adaptive_pacing": false}),
            "auto",
        ),
        (
            "pacing_max_bitrate_kbps=90000",
            serde_json::json!({"wireless_adaptive_pacing": false}),
            "90000",
        ),
        (
            "pacing_max_bitrate_kbps=90000",
            serde_json::json!({"pacing_max_bitrate_kbps": 0}),
            "auto",
        ),
        (
            "host_adaptive_pacing=true",
            serde_json::json!({"pacing_max_bitrate_kbps": 60000}),
            "60000",
        ),
    ] {
        let layer = layer.as_object().unwrap().clone();
        let mut migrated = layer.clone();
        // Only a rate, or adaptive pacing on, decides alone at migration.
        migrate_overrides(&mut migrated);
        let decided = layer
            .get("pacing_max_bitrate_kbps")
            .is_some_and(|rate| rate.as_i64() > Some(0))
            || layer.get("host_adaptive_pacing") == Some(&serde_json::json!(true));
        assert_eq!(migrated == layer, !decided, "{layer:?}");
        let config = Config::parse(host).unwrap();
        let mut folded = migrated.clone();
        config.fold_layer(&mut folded);
        assert_eq!(
            folded.get("packet_pacing").and_then(|v| v.as_str()),
            Some(expected),
            "{host:?} {layer:?}"
        );
        assert_eq!(folded.len(), 1, "{folded:?}");
        // Applied as the host does: the layer's setting over the host's.
        let mut applied = config.clone();
        applied.update(&migrated).unwrap();
        assert_eq!(
            applied.get("packet_pacing", "auto"),
            expected,
            "{host:?} {layer:?}"
        );
    }
}

/// The two bitrate switches fold into Adaptive bitrate once, from what the
/// configuration file, a console save or an app's or device's overrides
/// held: congestion control on is `delay`, link-rate adaptation off alone is
/// `off`, the rest `on`; the switches are then gone.
#[test]
fn the_bitrate_switches_fold_into_adaptive_bitrate() {
    use super::{BitrateAdaptation as A, Config, folded_bitrate_adaptation as fold};
    assert_eq!(fold(None, None), None);
    for (link, congestion, folded) in [
        (Some("true"), None, "on"),
        (Some("false"), None, "off"),
        (Some("invalid"), None, "on"),
        (None, Some("false"), "on"),
        (None, Some("true"), "delay"),
        (Some("true"), Some("false"), "on"),
        (Some("true"), Some("true"), "delay"),
        (Some("false"), Some("false"), "off"),
        (Some("false"), Some("true"), "delay"),
        (Some("\"false\""), Some("\"no\""), "off"),
    ] {
        assert_eq!(
            fold(link, congestion),
            Some(folded),
            "{link:?} {congestion:?}"
        );
    }
    // The configuration file.
    for (text, expected) in [
        ("", A::On),
        ("link_rate_adaptation=false", A::Off),
        (
            "link_rate_adaptation=false\ncongestion_control=false",
            A::Off,
        ),
        ("congestion_control=true", A::Delay),
        (
            "link_rate_adaptation=false\ncongestion_control=true",
            A::Delay,
        ),
        ("link_rate_adaptation=true\ncongestion_control=false", A::On),
        ("bitrate_adaptation=off\nlink_rate_adaptation=true", A::Off),
    ] {
        let config = Config::parse(text).unwrap();
        assert_eq!(config.bitrate_adaptation(), expected, "{text}");
        assert!(!config.text().contains("link_rate_adaptation"), "{text}");
        assert!(!config.text().contains("congestion_control"), "{text}");
    }
    // A save sent with the earlier names.
    let mut config = Config::default();
    let sent = serde_json::json!({"link_rate_adaptation": false});
    config.update(sent.as_object().unwrap()).unwrap();
    assert_eq!(config.bitrate_adaptation(), A::Off);
    assert!(!config.text().contains("link_rate_adaptation"));
    let sent = serde_json::json!({"congestion_control": "true", "link_rate_adaptation": true});
    config.update(sent.as_object().unwrap()).unwrap();
    assert_eq!(config.bitrate_adaptation(), A::Delay);
    let sent = serde_json::json!({"congestion_control": true, "bitrate_adaptation": "off"});
    config.update(sent.as_object().unwrap()).unwrap();
    assert_eq!(config.bitrate_adaptation(), A::Off);
    // An app's or device's overrides fold at migration where their own
    // switches decide the result.
    for (layer, expected) in [
        (
            serde_json::json!({"link_rate_adaptation": "false", "congestion_control": "false"}),
            Some("off"),
        ),
        (
            serde_json::json!({"congestion_control": true}),
            Some("delay"),
        ),
        (
            serde_json::json!({"link_rate_adaptation": true, "congestion_control": false}),
            Some("on"),
        ),
        (
            serde_json::json!({"congestion_control": true, "bitrate_adaptation": "on"}),
            Some("on"),
        ),
        (serde_json::json!({"fec_percentage": 20}), None),
    ] {
        let mut layer = layer.as_object().unwrap().clone();
        super::migrate_overrides(&mut layer);
        assert_eq!(
            layer
                .get("bitrate_adaptation")
                .and_then(serde_json::Value::as_str),
            expected,
            "{layer:?}"
        );
        assert!(!layer.contains_key("link_rate_adaptation"));
        assert!(!layer.contains_key("congestion_control"));
    }
    assert!(super::retired_name("link_rate_adaptation"));
    assert!(super::retired_name("congestion_control"));
}

/// A layer holding only one of the bitrate switches keeps the old combined
/// behaviour: it stays as saved at migration and folds over the layers below
/// when it is applied.
#[test]
fn a_layer_with_one_bitrate_switch_folds_over_the_layers_below() {
    use super::{Config, migrate_overrides};
    // (what the host holds, the layer, Adaptive bitrate the layer applies)
    for (host, layer, expected) in [
        // Congestion control on at the host: a device turning link-rate
        // adaptation on gets the delay controller, as before.
        (
            "congestion_control=true",
            serde_json::json!({"link_rate_adaptation": true}),
            "delay",
        ),
        (
            "congestion_control=true",
            serde_json::json!({"link_rate_adaptation": false}),
            "delay",
        ),
        (
            "",
            serde_json::json!({"link_rate_adaptation": false}),
            "off",
        ),
        ("", serde_json::json!({"link_rate_adaptation": true}), "on"),
        (
            "link_rate_adaptation=false",
            serde_json::json!({"link_rate_adaptation": true}),
            "on",
        ),
        (
            "link_rate_adaptation=false",
            serde_json::json!({"congestion_control": false}),
            "off",
        ),
        (
            "congestion_control=true",
            serde_json::json!({"congestion_control": false}),
            "on",
        ),
    ] {
        let layer = layer.as_object().unwrap().clone();
        let mut migrated = layer.clone();
        assert!(!migrate_overrides(&mut migrated), "{layer:?}");
        assert_eq!(migrated, layer, "stays as saved: {layer:?}");
        let config = Config::parse(host).unwrap();
        let mut folded = migrated.clone();
        config.fold_layer(&mut folded);
        assert_eq!(
            folded.get("bitrate_adaptation").and_then(|v| v.as_str()),
            Some(expected),
            "{host:?} {layer:?}"
        );
        assert_eq!(folded.len(), 1, "{folded:?}");
        let mut applied = config.clone();
        applied.update(&migrated).unwrap();
        assert_eq!(
            applied.get("bitrate_adaptation", "on"),
            expected,
            "{host:?} {layer:?}"
        );
    }
}

#[test]
fn resetting_packet_pacing_restores_the_default() {
    let mut config = super::Config::default();
    assert!(!config.adaptive_packet_pacing(0, false));
    for reset in [serde_json::Value::Null, serde_json::json!("")] {
        config
            .update(
                serde_json::json!({"packet_pacing": "always"})
                    .as_object()
                    .unwrap(),
            )
            .unwrap();
        assert!(config.adaptive_packet_pacing(0, false));
        config
            .update(
                serde_json::json!({"packet_pacing": reset})
                    .as_object()
                    .unwrap(),
            )
            .unwrap();
        assert!(!config.adaptive_packet_pacing(0, false));
        assert!(!config.values.contains_key("packet_pacing"));
    }
    // The earlier names fold into Packet pacing when an older console or a
    // script sends them.
    config
        .update(
            serde_json::json!({"host_adaptive_pacing": true})
                .as_object()
                .unwrap(),
        )
        .unwrap();
    assert!(config.adaptive_packet_pacing(0, false));
    assert_eq!(config.get("packet_pacing", ""), "always");
    assert!(!config.values.contains_key("host_adaptive_pacing"));
}

#[test]
fn adaptive_pacing_after_loss_only_applies_to_wired_auto_conventional_streams() {
    let wired = [true, true, true, false];
    let off = [false; 4];
    for (text, expected_wired) in [
        ("", wired),
        ("packet_pacing=auto", wired),
        ("packet_pacing=0", wired),
        ("packet_pacing=off", off),
        ("packet_pacing=always", off),
        ("packet_pacing=80000", off),
        ("keyframe_loss_pacing=false", off),
    ] {
        let config = super::Config::parse(text).unwrap();
        for codec in 0..4u8 {
            assert_eq!(
                config.adaptive_pacing_after_loss(codec, false),
                expected_wired[usize::from(codec)],
                "wired codec {codec}: {text}"
            );
            assert!(
                !config.adaptive_pacing_after_loss(codec, true),
                "wireless codec {codec}: {text}"
            );
        }
    }
}
