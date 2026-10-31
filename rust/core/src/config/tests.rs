#[test]
fn folder_ids_are_short_plain_names() {
    for id in ["games", "saves-2", "a_b", &"x".repeat(64)] {
        assert!(super::valid_folder_id(id), "{id}");
    }
    for id in [
        "",
        "../games",
        "a/b",
        "a b",
        "ü",
        &"x".repeat(65),
        "__proto__",
        "constructor",
        "prototype",
    ] {
        assert!(!super::valid_folder_id(id), "{id}");
    }
}

#[test]
fn a_shared_folder_is_a_folder_on_a_drive() {
    for root in [
        r"D:\Games",
        r"d:/Games/Saves",
        r"E:\",
        r"C:\Users\Player\My Games",
    ] {
        assert!(super::valid_folder_root(root), "{root}");
    }
    for root in [
        "",
        "Games",
        r"\Games",
        "D:",
        "D:Games",
        r"\\server\share",
        r"\\?\D:\Games",
        r"\\.\D:\Games",
        r"\\?\UNC\server\share",
        r"D:\Games\..\Windows",
        r"D:\Games\.",
        r"D:\Games:secret",
        r"D:\..",
    ] {
        assert!(!super::valid_folder_root(root), "{root}");
    }
    let config = |path: &str| {
        let list = serde_json::json!([{"id": "games", "name": "Games", "path": path}]);
        super::Config::parse(&format!("shared_folders = {list}"))
            .unwrap()
            .shared_folders()
    };
    assert_eq!(config(r"D:\Games").len(), 1);
    assert!(config(r"\\server\games").is_empty());
    assert!(config("games").is_empty());
}

#[test]
fn webrtc_video_is_paced_unless_the_measurement_switch_turns_it_off() {
    let mut config = super::Config::default();
    assert!(config.webrtc_pacing());
    let saved = serde_json::json!({"webrtc_pacing": "false"});
    config.update(saved.as_object().unwrap()).unwrap();
    assert!(!config.webrtc_pacing());
}

#[test]
fn file_transfer_is_off_with_4_gib_uploads_paced_at_50_mbps_while_streaming() {
    let mut config = super::Config::default();
    assert!(!config.file_transfer());
    assert_eq!(config.file_transfer_dir(), None);
    assert_eq!(config.file_transfer_max_bytes(), 4096 << 20);
    assert_eq!(config.file_transfer_stream_rate(), Some(6_250_000));
    let saved = serde_json::json!({
        "file_transfer": true,
        "file_transfer_dir": "\"D:\\Inbox\"",
        "file_transfer_max_mb": "2",
        "file_transfer_stream_mbps": 0,
    });
    config.update(saved.as_object().unwrap()).unwrap();
    assert!(config.file_transfer());
    assert_eq!(
        config.file_transfer_dir(),
        Some(std::path::PathBuf::from("D:\\Inbox"))
    );
    assert_eq!(config.file_transfer_max_bytes(), 2 << 20);
    assert_eq!(config.file_transfer_stream_rate(), None);
    for bad in ["0", "-1", "9223372036854775807"] {
        let saved = serde_json::json!({ "file_transfer_max_mb": bad, "file_transfer_dir": " " });
        config.update(saved.as_object().unwrap()).unwrap();
        assert_eq!(config.file_transfer_max_bytes(), 4096 << 20, "{bad}");
        assert_eq!(config.file_transfer_dir(), None);
    }
}

#[test]
fn usb_forwarding_is_off_with_4_devices_at_once() {
    let mut config = super::Config::default();
    assert!(!config.usb_forwarding());
    assert_eq!(config.usb_max_devices(), 4);
    let saved = serde_json::json!({"usb_forwarding": "enabled", "usb_max_devices": "16"});
    config.update(saved.as_object().unwrap()).unwrap();
    assert!(config.usb_forwarding());
    assert_eq!(config.usb_max_devices(), 16);
    for bad in ["0", "17", "-1", "lots"] {
        let saved = serde_json::json!({ "usb_max_devices": bad });
        config.update(saved.as_object().unwrap()).unwrap();
        assert_eq!(config.usb_max_devices(), 4, "{bad}");
    }
    let saved = serde_json::json!({"usb_max_devices": 1});
    config.update(saved.as_object().unwrap()).unwrap();
    assert_eq!(config.usb_max_devices(), 1);
}

#[test]
fn webrtc_rtx_is_on_and_datagram_loss_is_off_until_set() {
    let mut config = super::Config::default();
    assert!(config.webrtc_rtx());
    assert_eq!(config.webrtc_loss_permille(), 0);
    let saved = serde_json::json!({"webrtc_rtx": "false", "webrtc_loss_permille": "1500"});
    config.update(saved.as_object().unwrap()).unwrap();
    assert!(!config.webrtc_rtx());
    assert_eq!(config.webrtc_loss_permille(), 1000);
}

#[test]
fn audio_passthrough_is_on_unless_turned_off() {
    assert!(super::Config::default().audio_passthrough());
    for (text, on) in [
        ("audio_passthrough = true\n", true),
        ("audio_passthrough = disabled\n", false),
        ("audio_passthrough = 0\n", false),
    ] {
        assert_eq!(
            super::Config::parse(text).unwrap().audio_passthrough(),
            on,
            "{text}"
        );
    }
}

#[test]
fn client_roaming_is_on_unless_turned_off() {
    assert!(super::Config::default().client_roaming());
    for (text, on) in [
        ("client_roaming = true\n", true),
        ("client_roaming = off\n", false),
        ("client_roaming = 0\n", false),
    ] {
        assert_eq!(
            super::Config::parse(text).unwrap().client_roaming(),
            on,
            "{text}"
        );
    }
}

#[test]
fn hdr_dynamic_metadata_is_on_unless_turned_off() {
    assert!(super::Config::default().hdr_dynamic_metadata());
    for (text, on) in [
        ("hdr_dynamic_metadata = true\n", true),
        ("hdr_dynamic_metadata = disabled\n", false),
        ("hdr_dynamic_metadata = 0\n", false),
    ] {
        assert_eq!(
            super::Config::parse(text).unwrap().hdr_dynamic_metadata(),
            on,
            "{text}"
        );
    }
}

#[test]
fn the_fec_cap_is_0_to_255_percent_and_50_unless_set() {
    assert_eq!(super::Config::default().fec_max_percentage(), 50);
    for (text, cap) in [
        ("fec_max_percentage = 0\n", 0),
        ("fec_max_percentage = 255\n", 255),
        ("fec_max_percentage = 0x1e\n", 30),
        ("fec_max_percentage = -1\n", 50),
        ("fec_max_percentage = 256\n", 50),
        ("fec_max_percentage = lots\n", 50),
    ] {
        assert_eq!(
            super::Config::parse(text).unwrap().fec_max_percentage(),
            cap,
            "{text}"
        );
    }
}

#[test]
fn the_keyboard_follows_text_fields_without_their_text_by_default() {
    let c = super::Config::default();
    assert!(c.keyboard_follows_focus() && !c.text_input_surrounding_text());
    let c = super::Config::parse(
        "keyboard_follows_focus = disabled\ntext_input_surrounding_text = enabled\n",
    )
    .unwrap();
    assert!(!c.keyboard_follows_focus() && c.text_input_surrounding_text());
    for key in [
        "keyboard_follows_focus",
        "text_input_surrounding_text",
        "keyboard_layout",
    ] {
        assert!(super::override_allowed(key), "{key}");
    }
}

#[test]
fn the_bandwidth_probe_is_on_unless_turned_off() {
    assert!(super::Config::default().bandwidth_probe());
    for (text, on) in [
        ("bandwidth_probe = true\n", true),
        ("bandwidth_probe = disabled\n", false),
        ("bandwidth_probe = 0\n", false),
    ] {
        assert_eq!(
            super::Config::parse(text).unwrap().bandwidth_probe(),
            on,
            "{text}"
        );
    }
}

#[test]
fn keyboard_layout_takes_client_us_or_a_klid() {
    use super::KeyboardLayoutSetting::{Client, Klid, Us};
    let layout = |text: &str| super::Config::parse(text).unwrap().keyboard_layout();
    assert_eq!(super::Config::default().keyboard_layout(), Client);
    for (text, expected) in [
        ("keyboard_layout = client", Client),
        ("keyboard_layout = us", Us),
        ("keyboard_layout = 00000407", Klid(0x407)),
        ("keyboard_layout = \"0001041f\"", Klid(0x1041F)),
        // What the host can't use is reported and the device's layout applies.
        ("keyboard_layout = de", Client),
        ("keyboard_layout = 0000040", Client),
        ("keyboard_layout = 0000040G", Client),
        ("keyboard_layout = 000004070", Client),
    ] {
        assert_eq!(layout(text), expected, "{text}");
    }
}

#[test]
fn the_webrtc_server_is_off_on_8000_and_8001_and_refuses_bad_ports() {
    let config = super::Config::default();
    assert!(!config.webrtc_server());
    assert_eq!(
        (config.webrtc_port(), config.webrtc_media_port()),
        (8000, 8001)
    );
    let mut config = super::Config::default();
    let saved = serde_json::json!({"webrtc_server": true, "webrtc_port": 9000, "webrtc_media_port": "9001"});
    config.update(saved.as_object().unwrap()).unwrap();
    assert!(config.webrtc_server());
    assert_eq!(
        (config.webrtc_port(), config.webrtc_media_port()),
        (9000, 9001)
    );
    for bad in [
        serde_json::json!(80),
        serde_json::json!(70000),
        serde_json::json!("x"),
    ] {
        for key in ["webrtc_port", "webrtc_media_port"] {
            let saved = serde_json::json!({ key: bad });
            assert!(
                config.update(saved.as_object().unwrap()).is_err(),
                "{key} {bad}"
            );
        }
    }
    // A hand-edited file with a bad port falls back loudly to the default.
    let config = super::Config::parse(
        "webrtc_port = 80
webrtc_media_port = 99999
",
    )
    .unwrap();
    assert_eq!(
        (config.webrtc_port(), config.webrtc_media_port()),
        (8000, 8001)
    );
}
#[test]
fn direct_driver_push_defaults_to_the_fence_ring_unless_turned_off() {
    let fence = |text: &str| super::Config::parse(text).unwrap().idd_push_fence_ring();
    assert!(fence(""));
    assert!(fence("port = 47989"));
    assert!(fence("idd_push_fence_ring = true"));
    assert!(!fence("idd_push_fence_ring = false"));
    assert!(!fence("idd_push_fence_ring = off"));
    assert!(super::Config::default().idd_push_fence_ring());
}
#[test]
fn removed_rtx_hdr_settings_go_quietly_and_its_peak_brightness_is_renamed() {
    // Profiles, apps and devices saved before RTX HDR was removed.
    let mut config = super::Config::parse(
            "rtx_hdr = true\nrtx_hdr_contrast = 14\nrtx_hdr_sdr_brightness = 20\nrtx_hdr_peak_brightness = 1500\nport = 47989\n",
        )
        .unwrap();
    for key in super::REMOVED_KEYS {
        assert!(!config.values.contains_key(*key), "{key}");
    }
    assert_eq!(config.integer("hdr_peak_brightness", 0), 1500);
    assert_eq!(config.integer("port", 0), 47989);
    // A console or API save still sending them.
    let saved = serde_json::json!({"rtx_hdr_saturation": 5, "rtx_hdr_peak_brightness": 900});
    config.update(saved.as_object().unwrap()).unwrap();
    assert!(!config.values.contains_key("rtx_hdr_saturation"));
    assert_eq!(config.integer("hdr_peak_brightness", 0), 900);
    // Devices and apps may still choose the peak, under its new name.
    assert!(super::override_allowed("hdr_peak_brightness"));
    assert!(!super::override_allowed("rtx_hdr"));
    assert_eq!(
        super::current_name("rtx_hdr_peak_brightness"),
        "hdr_peak_brightness"
    );
}
#[test]
fn packet_pacing_requires_opt_in_and_yields_to_manual_bitrate_and_codec_policies() {
    let cases = [
        ("", [false, false, false, false]),
        ("host_adaptive_pacing=false", [false, false, false, false]),
        ("host_adaptive_pacing=invalid", [false, false, false, false]),
        ("host_adaptive_pacing=true", [true, true, true, true]),
        (
            "host_adaptive_pacing=true\npacing_max_bitrate_kbps=0",
            [true, true, true, true],
        ),
        (
            "host_adaptive_pacing=true\npacing_max_bitrate_kbps=-1",
            [true, true, true, true],
        ),
        (
            "host_adaptive_pacing=true\npacing_max_bitrate_kbps=1",
            [false, false, false, false],
        ),
        (
            "host_adaptive_pacing=true\npacing_max_bitrate_kbps=80000",
            [false, false, false, false],
        ),
        (
            "host_adaptive_pacing=true\nhost_adaptive_bitrate=true",
            [false, false, false, false],
        ),
        (
            "host_adaptive_pacing=true\nhost_adaptive_bitrate=false",
            [true, true, true, true],
        ),
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
            "wireless_adaptive_pacing=true",
            on,
            [false, false, false, true],
            off,
            [true; 4],
        ),
        (
            "wireless_adaptive_pacing=invalid",
            on,
            [false, false, false, true],
            off,
            [true; 4],
        ),
        (
            "wireless_adaptive_pacing=false",
            off,
            [true; 4],
            off,
            [true; 4],
        ),
        // The host option still turns it on for everything, wired or wireless.
        (
            "host_adaptive_pacing=true",
            all,
            [false; 4],
            all,
            [false; 4],
        ),
        (
            "host_adaptive_pacing=true\nwireless_adaptive_pacing=false",
            all,
            [false; 4],
            all,
            [false; 4],
        ),
        // A manual limit or host bitrate adaptation owns pacing.
        (
            "pacing_max_bitrate_kbps=80000",
            off,
            [false; 4],
            off,
            [false; 4],
        ),
        (
            "pacing_max_bitrate_kbps=0",
            on,
            [false, false, false, true],
            off,
            [true; 4],
        ),
        (
            "host_adaptive_bitrate=true",
            off,
            [false; 4],
            off,
            [false; 4],
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
    assert!(super::override_allowed("wireless_adaptive_pacing"));
}

#[test]
fn disconnected_displays_restore_by_default_and_exclusive_layouts_cannot_be_retained() {
    use std::time::Duration;
    for (text, ordinary, exclusive) in [
        ("", Some(3000), Some(3000)),
        (
            "dd_config_revert_on_disconnect=false",
            Some(7_200_000),
            Some(3000),
        ),
        (
            "dd_config_revert_on_disconnect=false\ndd_paused_virtual_display_timeout_secs=0",
            None,
            Some(3000),
        ),
        (
            "dd_config_revert_on_disconnect=false\ndd_paused_virtual_display_timeout_secs=20\ndd_config_revert_delay=500",
            Some(20_000),
            Some(500),
        ),
        ("dd_config_revert_delay=0", Some(0), Some(0)),
        ("dd_config_revert_delay=-1", Some(0), Some(0)),
        (
            "dd_config_revert_on_disconnect=invalid",
            Some(3000),
            Some(3000),
        ),
    ] {
        let config = super::Config::parse(text).unwrap();
        assert_eq!(
            config.display_disconnect_delay(false),
            ordinary.map(Duration::from_millis),
            "{text}"
        );
        assert_eq!(
            config.display_disconnect_delay(true),
            exclusive.map(Duration::from_millis),
            "{text}"
        );
    }
}

#[test]
fn keyframe_loss_pacing_is_on_unless_a_manual_limit_or_adaptation_owns_pacing() {
    for (text, enabled) in [
        ("", true),
        ("keyframe_loss_pacing=true", true),
        ("keyframe_loss_pacing=invalid", true),
        ("keyframe_loss_pacing=false", false),
        ("pacing_max_bitrate_kbps=0", true),
        ("pacing_max_bitrate_kbps=80000", false),
        ("host_adaptive_pacing=true", false),
        ("host_adaptive_bitrate=true", false),
    ] {
        let config = super::Config::parse(text).unwrap();
        assert_eq!(config.keyframe_loss_pacing(0, false), enabled, "{text}");
    }
    assert!(super::override_allowed("keyframe_loss_pacing"));
}

#[test]
fn link_rate_adaptation_is_on_unless_disabled_or_the_opt_in_owns_the_bitrate() {
    for (text, enabled) in [
        ("", true),
        ("link_rate_adaptation=true", true),
        ("link_rate_adaptation=invalid", true),
        ("link_rate_adaptation=false", false),
        ("host_adaptive_bitrate=true", false),
        (
            "host_adaptive_bitrate=true\nlink_rate_adaptation=true",
            false,
        ),
        ("host_adaptive_bitrate=false", true),
        ("host_adaptive_pacing=true", true),
    ] {
        let config = super::Config::parse(text).unwrap();
        assert_eq!(config.link_rate_adaptation(), enabled, "{text}");
    }
    assert!(super::override_allowed("link_rate_adaptation"));
}

#[test]
fn resetting_packet_pacing_opt_in_restores_the_disabled_default() {
    let mut config = super::Config::default();
    assert!(!config.adaptive_packet_pacing(0, false));
    for reset in [serde_json::Value::Null, serde_json::json!("")] {
        config
            .update(
                serde_json::json!({"host_adaptive_pacing": true})
                    .as_object()
                    .unwrap(),
            )
            .unwrap();
        assert!(config.adaptive_packet_pacing(0, false));
        config
            .update(
                serde_json::json!({"host_adaptive_pacing": reset})
                    .as_object()
                    .unwrap(),
            )
            .unwrap();
        assert!(!config.adaptive_packet_pacing(0, false));
        assert!(!config.values.contains_key("host_adaptive_pacing"));
    }
}

#[test]
fn hardware_metrics_defaults_and_file_interval_bounds() {
    let defaults = super::Config::default();
    assert!(!defaults.boolean("hardware_metrics", false));
    assert_eq!(defaults.hardware_metrics_interval(), 1000);
    assert!(defaults.list("hardware_metrics_selection").is_empty());
    for (value, expected) in [
        ("invalid", 1000),
        ("0", 500),
        ("750", 750),
        ("20000", 10000),
    ] {
        let config = super::Config::parse(&format!("hardware_metrics_interval={value}\n")).unwrap();
        assert_eq!(config.hardware_metrics_interval(), expected);
    }
}
#[test]
fn stream_preset_saves_only_a_preset_name_and_clears_with_null() {
    let mut config = super::Config::default();
    for value in ["lowest_latency", "balanced", "reference", "Reference"] {
        change(&mut config, "stream_preset", value).unwrap();
        assert_eq!(config.get("stream_preset", ""), value);
    }
    for value in ["custom", "fastest", "lowest", "5"] {
        assert!(
            change(&mut config, "stream_preset", value).is_err(),
            "{value}"
        );
        assert_eq!(config.get("stream_preset", ""), "Reference");
    }
    let mut object = serde_json::Map::new();
    object.insert("stream_preset".into(), serde_json::Value::Null);
    config.update(&object).unwrap();
    assert!(!config.values.contains_key("stream_preset"));
    // A file the host did not write may hold anything; it still loads.
    let loaded = super::Config::parse(
        "stream_preset = fastest
",
    )
    .unwrap();
    assert_eq!(loaded.get("stream_preset", ""), "fastest");
}
#[test]
fn background_mode_saves_only_nucleus_and_earlier_modes_become_it() {
    let mut config = super::Config::default();
    // A client or an old console sending an earlier mode saves nucleus.
    for value in ["auto", "child_session", "virtual_display", "nucleus", ""] {
        change(&mut config, "background_mode", value).unwrap();
        let saved = if value.is_empty() { "" } else { "nucleus" };
        assert_eq!(config.get("background_mode", ""), saved, "{value}");
    }
    let error = format!(
        "{:#}",
        change(&mut config, "background_mode", "rdp").unwrap_err()
    );
    assert!(
            error.contains(
                "background_mode = rdp is not a mode: Nucleus Co-op is the only background mode (nucleus)"
            ),
            "{error}"
        );
    assert_eq!(config.get("background_mode", ""), "");
}
#[test]
fn a_saved_child_session_mode_becomes_nucleus_with_one_report() {
    for old in [
        "auto",
        "child_session",
        "virtual_display",
        " Child_Session ",
    ] {
        let text = format!("background_mode = {old}\nport = 48123\n");
        let (config, retired) = Config::parse_retiring(&text).unwrap();
        assert_eq!(config.get("background_mode", ""), "nucleus", "{old}");
        assert_eq!(retired.len(), 1, "{old}");
        let warning = retired_warning(&retired).unwrap();
        assert!(warning.contains("is now nucleus"), "{warning}");
        assert!(
            warning.contains("Windows child sessions were removed"),
            "{warning}"
        );
        // The saved text holds nucleus, and reads without a report.
        assert!(config.text().contains("background_mode = nucleus"));
        let (_, again) = Config::parse_retiring(&config.text()).unwrap();
        assert!(again.is_empty(), "{old}");
    }
    for kept in ["nucleus", ""] {
        let (config, retired) =
            Config::parse_retiring(&format!("background_mode = {kept}\n")).unwrap();
        assert!(retired.is_empty(), "{kept}");
        assert_eq!(config.get("background_mode", ""), kept);
    }
    // Apps and devices.
    let mut settings = serde_json::json!({"background_mode": "child_session", "port": "1"});
    assert!(super::replace_retired(
        settings.as_object_mut().unwrap().iter_mut()
    ));
    assert_eq!(settings["background_mode"], "nucleus");
    let mut nucleus = serde_json::json!({"background_mode": "nucleus"});
    assert!(!super::replace_retired(
        nucleus.as_object_mut().unwrap().iter_mut()
    ));
}
#[test]
fn the_child_session_settings_go_without_a_report() {
    let (config, retired) = Config::parse_retiring(
            "child_session_resolution = 2560x1440\nchild_session_scale = 150\nchild_session_port = 48289\nchild_session_host = true\nport = 48123\n",
        )
        .unwrap();
    assert!(retired.is_empty());
    for key in [
        "child_session_resolution",
        "child_session_scale",
        "child_session_port",
        "child_session_host",
    ] {
        assert!(!config.values.contains_key(key), "{key}");
        assert!(super::removed(key), "{key}");
    }
    assert_eq!(config.port().unwrap(), 48123);
}
#[test]
fn hardware_metrics_saves_validate_interval_and_selection() {
    let mut config = super::Config::default();
    for value in ["500", "1000", "10000"] {
        change(&mut config, "hardware_metrics_interval", value).unwrap();
    }
    for value in ["499", "10001", "invalid", "1.5"] {
        assert!(change(&mut config, "hardware_metrics_interval", value).is_err());
    }
    for value in [
        serde_json::Value::Null,
        serde_json::Value::String(String::new()),
    ] {
        change(&mut config, "hardware_metrics_interval", "750").unwrap();
        let mut object = serde_json::Map::new();
        object.insert("hardware_metrics_interval".into(), value);
        config.update(&object).unwrap();
        assert!(!config.values.contains_key("hardware_metrics_interval"));
        assert_eq!(config.hardware_metrics_interval(), 1000);
    }
    change(
        &mut config,
        "hardware_metrics_selection",
        "[cpu_usage_percent, gpu_temperature_c]",
    )
    .unwrap();
    assert_eq!(
        config.list("hardware_metrics_selection"),
        ["cpu_usage_percent", "gpu_temperature_c"]
    );
    assert!(
        change(
            &mut config,
            "hardware_metrics_selection",
            "[cpu_usage_percent, cpu_temperature_c]"
        )
        .is_err()
    );
    assert_eq!(
        config.list("hardware_metrics_selection"),
        ["cpu_usage_percent", "gpu_temperature_c"]
    );
    change(&mut config, "hardware_metrics_selection", "[]").unwrap();
    assert!(config.list("hardware_metrics_selection").is_empty());
    let mut config =
        super::Config::parse("hardware_metrics_interval=0\nhardware_metrics_selection=[unknown]\n")
            .unwrap();
    change(&mut config, "sunshine_name", "Gaming PC").unwrap();
    assert_eq!(config.list("hardware_metrics_selection"), ["unknown"]);
}
#[test]
fn a_saved_ltr_zero_from_before_2_0_0_gives_way_to_the_new_default_once() {
    let mut config = super::Config::parse("amd_ltr_frames = 0\nport = 47989\n").unwrap();
    assert!(config.upgrade_ltr_default(false));
    assert!(!config.values.contains_key("amd_ltr_frames"));
    assert_eq!(config.integer("port", 0), 47989);
    // Chosen after the upgrade, 0 stays off.
    let mut config = super::Config::parse("amd_ltr_frames = 0\n").unwrap();
    assert!(!config.upgrade_ltr_default(true));
    assert_eq!(config.integer("amd_ltr_frames", 4), 0);
    // Other values are the user's own.
    for text in ["", "amd_ltr_frames = 1\n", "amd_ltr_frames = 4\n"] {
        let mut config = super::Config::parse(text).unwrap();
        let before = config.values.clone();
        assert!(!config.upgrade_ltr_default(false), "{text}");
        assert_eq!(config.values, before);
    }
}
#[test]
fn overrides_allow_stream_input_display_and_encoder_keys_but_not_host_settings() {
    for key in [
        "max_bitrate",
        "stream_audio",
        "keyboard",
        "gamepad",
        "dd_hdr_option",
        "dd_mode_remapping",
        "encoder",
        "amd_quality",
        "encoder_focus",
        "hdr_dynamic_metadata",
        "hdr_dolby_vision",
        "hdr_hlg",
        "frame_limiter_fps_limit",
        "adaptive_fec",
        "host_adaptive_pacing",
    ] {
        assert!(super::override_allowed(key), "{key}");
    }
    for key in [
        "port",
        "bind_address",
        "file_apps",
        "cert",
        "pkey",
        "enable_pairing",
        "server_cmd",
        "global_prep_cmd",
        "log_path",
        "unknown",
    ] {
        assert!(!super::override_allowed(key), "{key}");
    }
}
#[test]
fn audio_processing_overrides_per_device_and_app_but_idle_sleep_stays_host_wide() {
    assert!(super::override_allowed("audio_night_mode"));
    assert!(super::override_allowed("audio_headphone_surround"));
    assert!(!super::override_allowed("sleep_after_idle_minutes"));
    assert!(!super::override_allowed("sleep_while_suspended"));
}
#[test]
fn idle_sleep_minutes_are_off_or_between_5_and_1440() {
    let mut config = super::Config::default();
    for value in ["0", "5", "30", "1440", "\"60\""] {
        change(&mut config, "sleep_after_idle_minutes", value).unwrap();
    }
    assert_eq!(config.integer("sleep_after_idle_minutes", -1), 60);
    for value in ["3", "4", "1441", "-5", "off", "1.5", "true"] {
        let error = change(&mut config, "sleep_after_idle_minutes", value).unwrap_err();
        assert!(
            error.to_string().contains("sleep_after_idle_minutes"),
            "{value}: {error}"
        );
    }
    assert_eq!(config.integer("sleep_after_idle_minutes", -1), 60);
    // A JSON number from the console is checked the same way.
    let mut object = serde_json::Map::new();
    object.insert("sleep_after_idle_minutes".into(), 3.into());
    assert!(config.update(&object).is_err());
    object.insert("sleep_after_idle_minutes".into(), 15.into());
    config.update(&object).unwrap();
    assert_eq!(config.integer("sleep_after_idle_minutes", -1), 15);
    // Null resets it to off.
    object.insert("sleep_after_idle_minutes".into(), serde_json::Value::Null);
    config.update(&object).unwrap();
    assert!(!config.values.contains_key("sleep_after_idle_minutes"));
}
#[test]
fn audio_processing_and_suspended_sleep_settings_are_booleans() {
    for key in [
        "audio_night_mode",
        "audio_headphone_surround",
        "sleep_while_suspended",
    ] {
        let mut config = super::Config::default();
        for value in ["true", "on", "enabled", "1"] {
            change(&mut config, key, value).unwrap();
            assert!(config.boolean(key, false), "{key}={value}");
        }
        for value in ["false", "off", "0"] {
            change(&mut config, key, value).unwrap();
            assert!(!config.boolean(key, true), "{key}={value}");
        }
        for value in ["maybe", "2", "night"] {
            let error = change(&mut config, key, value).unwrap_err();
            assert!(error.to_string().contains(key), "{key}={value}: {error}");
        }
        let mut object = serde_json::Map::new();
        object.insert(key.into(), true.into());
        config.update(&object).unwrap();
        assert!(config.boolean(key, false), "{key} as a JSON boolean");
    }
}
#[test]
fn audio_haptics_settings_are_checked_on_save_and_override_per_device() {
    use crate::audio_haptics::{CURVE, ENABLED, GATE, SETTINGS, STRENGTH};
    let mut config = super::Config::default();
    for (key, value) in [
        (ENABLED, "true"),
        (STRENGTH, "0"),
        (STRENGTH, "200"),
        (CURVE, "soft"),
        (GATE, "-80"),
        (GATE, "-10"),
    ] {
        change(&mut config, key, value).unwrap();
    }
    for (key, value) in [
        (ENABLED, "sometimes"),
        (STRENGTH, "250"),
        (CURVE, "wobbly"),
        (GATE, "0"),
    ] {
        let error = change(&mut config, key, value).unwrap_err();
        assert!(error.to_string().contains(key), "{key}={value}: {error}");
    }
    let mut object = serde_json::Map::new();
    object.insert(STRENGTH.into(), 300.into());
    assert!(config.update(&object).is_err());
    object.insert(STRENGTH.into(), serde_json::Value::Null);
    config.update(&object).unwrap();
    assert!(!config.values.contains_key(STRENGTH));
    for key in SETTINGS {
        assert!(super::override_allowed(key), "{key}");
    }
}
#[test]
fn a_device_or_app_audio_override_beats_the_host_and_a_bad_one_is_refused() {
    // The host applies device, then app overrides onto its own file
    // through `update`, one key at a time.
    let mut config = super::Config::parse("audio_night_mode = off\n").unwrap();
    let layer = |config: &mut super::Config, key: &str, value: serde_json::Value| {
        config.update(&serde_json::Map::from_iter([(key.to_owned(), value)]))
    };
    layer(&mut config, "audio_night_mode", true.into()).unwrap();
    assert!(config.boolean("audio_night_mode", false));
    layer(&mut config, "audio_night_mode", "off".into()).unwrap();
    assert!(!config.boolean("audio_night_mode", true));
    assert!(layer(&mut config, "audio_headphone_surround", "loud".into()).is_err());
    assert!(!config.values.contains_key("audio_headphone_surround"));
}
#[test]
fn couch_menu_controls_are_checked_on_save_with_one_message_each() {
    let mut config = super::Config::default();
    for (key, value) in [
        ("couch_menu_controller_chord", "l1+r1+y"),
        ("couch_menu_controller_chord", "guide"),
        ("couch_menu_hold_ms", "300"),
        ("couch_menu_hold_ms", "3000"),
        ("couch_menu_corner_hold_ms", "500"),
        ("couch_menu_corner_hold_ms", "3000"),
        ("couch_menu_keyboard_chord", "ctrl+alt+insert"),
        ("couch_menu_open_with", "controller,touch"),
    ] {
        change(&mut config, key, value).unwrap();
        assert_eq!(config.get(key, ""), value);
    }
    for (key, value, message) in [
        (
            "couch_menu_controller_chord",
            "start+select+l1+r1",
            "quits the stream",
        ),
        ("couch_menu_controller_chord", "start", "mouse mode"),
        ("couch_menu_controller_chord", "start+select", "into Guide"),
        (
            "couch_menu_controller_chord",
            "select+l1+r1+x",
            "statistics",
        ),
        ("couch_menu_controller_chord", "a+start", "mouse mode"),
        (
            "couch_menu_controller_chord",
            "l1+r1+a",
            "A presses the focused row",
        ),
        ("couch_menu_controller_chord", "b", "B goes back"),
        ("couch_menu_controller_chord", "x+up", "Up moves the focus"),
        ("couch_menu_controller_chord", "l3+r2", "unknown button"),
        ("couch_menu_hold_ms", "299", "between 300 and 3000"),
        ("couch_menu_hold_ms", "3001", "between 300 and 3000"),
        ("couch_menu_corner_hold_ms", "499", "between 500 and 3000"),
        ("couch_menu_corner_hold_ms", "3001", "between 500 and 3000"),
        (
            "couch_menu_keyboard_chord",
            "ctrl+alt+shift+home",
            "Ctrl+Alt+Shift",
        ),
        ("couch_menu_keyboard_chord", "ctrl+alt+del", "unknown chord"),
        ("couch_menu_open_with", "pad", "unknown entry"),
    ] {
        let error = change(&mut config, key, value).unwrap_err().to_string();
        assert!(error.starts_with(key), "{key}={value}: {error}");
        assert!(error.contains(message), "{key}={value}: {error}");
    }
    // Nothing refused was kept.
    assert_eq!(config.get("couch_menu_controller_chord", ""), "guide");
    assert_eq!(config.integer("couch_menu_hold_ms", 0), 3000);
    assert_eq!(config.integer("couch_menu_corner_hold_ms", 0), 3000);
    assert_eq!(
        config.get("couch_menu_keyboard_chord", ""),
        "ctrl+alt+insert"
    );
    // A JSON number from the console is checked the same way.
    let mut object = serde_json::Map::new();
    object.insert("couch_menu_hold_ms".into(), 299.into());
    assert!(config.update(&object).is_err());
    object.insert("couch_menu_hold_ms".into(), 750.into());
    config.update(&object).unwrap();
    let open_with = crate::couch_menu::chord::OpenWith::from_config(&config);
    assert_eq!(open_with.pad_hold, std::time::Duration::from_millis(750));
    assert!(!open_with.keyboard);
}
#[test]
fn a_device_couch_menu_override_beats_the_host_and_a_bad_one_is_refused() {
    use crate::couch_menu::chord::{KeyChord, OpenWith, PadButtons};
    for key in [
        "couch_menu_open_with",
        "couch_menu_controller_chord",
        "couch_menu_hold_ms",
        "couch_menu_corner_hold_ms",
        "couch_menu_keyboard_chord",
    ] {
        assert!(super::override_allowed(key), "{key}");
    }
    let mut config =
        super::Config::parse("couch_menu_controller_chord = l1+r1+y\ncouch_menu_hold_ms = 2000\n")
            .unwrap();
    let layer = |config: &mut super::Config, key: &str, value: serde_json::Value| {
        config.update(&serde_json::Map::from_iter([(key.to_owned(), value)]))
    };
    layer(&mut config, "couch_menu_controller_chord", "select".into()).unwrap();
    layer(&mut config, "couch_menu_hold_ms", 400.into()).unwrap();
    layer(
        &mut config,
        "couch_menu_keyboard_chord",
        "ctrl+alt+end".into(),
    )
    .unwrap();
    layer(&mut config, "couch_menu_open_with", "controller".into()).unwrap();
    let open_with = OpenWith::from_config(&config);
    assert!(open_with.controller && !open_with.keyboard && !open_with.touch);
    assert_eq!(open_with.pad_chord, PadButtons::parse("select").unwrap());
    assert_eq!(open_with.pad_hold, std::time::Duration::from_millis(400));
    assert_eq!(
        open_with.key_chord,
        KeyChord::parse("ctrl+alt+end").unwrap()
    );
    assert!(layer(&mut config, "couch_menu_controller_chord", "start".into()).is_err());
    assert!(layer(&mut config, "couch_menu_corner_hold_ms", 100.into()).is_err());
    assert_eq!(config.get("couch_menu_controller_chord", ""), "select");
    assert!(!config.values.contains_key("couch_menu_corner_hold_ms"));
}
use super::*;
/// A change saved to sunshine.conf, as the settings API does it.
fn change(config: &mut Config, key: &str, value: &str) -> Result<()> {
    let mut object = serde_json::Map::new();
    object.insert(key.into(), value.into());
    let mut next = config.clone();
    next.update(&object)?;
    next.check_saved(object.keys())?;
    *config = next;
    Ok(())
}
#[test]
fn overrides_that_are_never_saved_may_hold_a_comment_sign() {
    let mut config = Config::default();
    let mut object = serde_json::Map::new();
    object.insert("sunshine_name".into(), "Gaming PC #2".into());
    config.update(&object).unwrap();
    assert_eq!(config.get("sunshine_name", ""), "Gaming PC #2");
}
#[test]
fn a_bad_value_already_in_the_file_does_not_block_other_saves() {
    let mut config = Config::parse("port = 70000\nframe_limiter_fps_limit = -1\n").unwrap();
    change(&mut config, "encoder", "amf").unwrap();
    assert_eq!(config.get("encoder", ""), "amf");
    assert!(change(&mut config, "port", "70000").is_err());
    change(&mut config, "port", "48000").unwrap();
}
#[test]
fn values_the_file_cannot_give_back_are_refused() {
    let mut config = Config::default();
    // A comment would cut it short.
    assert!(change(&mut config, "sunshine_name", "Gaming PC #2").is_err());
    change(&mut config, "sunshine_name", "\"Gaming PC #2\"").unwrap();
    // An unclosed bracket would swallow the settings after it.
    assert!(change(&mut config, "adapter_name", "[Radeon").is_err());
    assert!(!config.values.contains_key("adapter_name"));
    // Spaces around a value and indented lists come back as the file reads them.
    change(&mut config, "sunshine_name", "  Living room  ").unwrap();
    change(&mut config, "global_prep_cmd", "[\n  {\"do\": \"a\"}\n]").unwrap();
    let reread = Config::parse(&config.text()).unwrap();
    assert_eq!(reread.get("sunshine_name", ""), "Living room");
    assert_eq!(reread.get("global_prep_cmd", ""), "[\n{\"do\": \"a\"}\n]");
}
#[test]
fn bind_address_and_log_path_that_would_stop_the_host_are_refused() {
    let mut config = Config::default();
    assert!(change(&mut config, "bind_address", "Ethernet").is_err());
    change(&mut config, "bind_address", "192.168.1.5").unwrap();
    change(&mut config, "bind_address", "").unwrap();
    assert!(!config.values.contains_key("bind_address"));
    assert!(change(&mut config, "log_path", "C:\\Logs\\").is_err());
    let folder = std::env::temp_dir();
    assert!(change(&mut config, "log_path", folder.to_str().unwrap()).is_err());
    change(&mut config, "log_path", "D:\\Logs\\host.log").unwrap();
}
#[test]
fn display_policies_keep_resolution_and_refresh_independent() {
    let config =
        Config::parse("dd_resolution_option = disabled\ndd_refresh_rate_option = auto\n").unwrap();
    let requested = config.display_request(1920, 1080, 120).unwrap();
    assert_eq!(requested.resolution, None);
    assert_eq!(requested.refresh, Some(120));
    let config = Config::parse("dd_resolution_option = manual\ndd_manual_resolution = 2560x1440\ndd_refresh_rate_option = disabled\n").unwrap();
    let requested = config.display_request(1920, 1080, 120).unwrap();
    assert_eq!(requested.resolution, Some((2560, 1440)));
    assert_eq!(requested.refresh, None);
    for value in ["NaN", "inf", "0", "1001"] {
        let config = Config::parse(&format!(
            "dd_refresh_rate_option = manual\ndd_manual_refresh_rate = {value}\n"
        ))
        .unwrap();
        assert!(config.display_request(1920, 1080, 60).is_err());
    }
}
/// A profile from a host that streamed with NVIDIA, Intel or software
/// encoding, or limited frames in the NVIDIA driver.
const REMOVED_FEATURES: &str = "encoder = nvenc\nframe_limiter_provider = nvidia-control-panel\nrtss_frame_limit_type = nvidia-reflex\nnvenc_preset = 4\nqsv_preset = slow\nsw_preset = fast\nmin_threads = 4\nframe_limiter_disable_vsync = true\nrtss_disable_vsync_ullm = true\nnvenc_force_split_encode = forced\ndd_wa_dummy_plug_hdr10 = true\nport = 48123\n";
#[test]
fn removed_encoders_and_nvidia_limiters_migrate_with_one_report() {
    let (config, retired) = Config::parse_retiring(REMOVED_FEATURES).unwrap();
    assert_eq!(config.get("encoder", ""), "amf");
    assert_eq!(config.get("frame_limiter_provider", ""), "auto");
    assert_eq!(config.get("rtss_frame_limit_type", ""), "async");
    assert_eq!(config.port().unwrap(), 48123);
    // The removed encoders' and the NVIDIA driver's own settings go quietly.
    assert_eq!(config.values.len(), 4, "{:?}", config.values);
    assert_eq!(
        retired
            .iter()
            .map(|r| (r.key.as_str(), r.value.as_str(), r.current))
            .collect::<Vec<_>>(),
        [
            ("encoder", "nvenc", "amf"),
            ("frame_limiter_provider", "nvidia-control-panel", "auto"),
            ("rtss_frame_limit_type", "nvidia-reflex", "async"),
        ]
    );
    let warning = retired_warning(&retired).unwrap();
    assert!(warning.contains("encoder nvenc is now amf"), "{warning}");
    assert!(warning.contains("frame_limiter_provider nvidia-control-panel is now auto"));
    // What the host writes back reads without a report.
    let (again, retired) = Config::parse_retiring(&config.text()).unwrap();
    assert_eq!(again.values, config.values);
    assert!(retired.is_empty());
    assert!(retired_warning(&retired).is_none());
}
#[test]
fn every_encoder_the_host_no_longer_has_becomes_amf() {
    for name in [
        "nvenc",
        "nvenc_experimental",
        "nvenc_legacy",
        "quicksync",
        "qsv",
        "software",
        "mediafoundation",
        "\"nvenc\"",
        "something_else",
    ] {
        let (config, retired) = Config::parse_retiring(&format!("encoder = {name}\n")).unwrap();
        assert_eq!(config.get("encoder", ""), "amf", "{name}");
        assert_eq!(retired.len(), 1, "{name}");
    }
    for kept in ["auto", "amf", "pyrowave", "amdvce", "amdvce_legacy"] {
        let (config, retired) = Config::parse_retiring(&format!("encoder = {kept}\n")).unwrap();
        assert_eq!(config.get("encoder", ""), kept);
        assert!(retired.is_empty(), "{kept}");
    }
    for (key, old, current) in [
        ("frame_limiter_provider", "nvcp", "auto"),
        ("frame_limiter_provider", "NVIDIA", "auto"),
        ("rtss_frame_limit_type", "reflex", "async"),
        ("rtss_frame_limit_type", "nvidia reflex", "async"),
    ] {
        let config = Config::parse(&format!("{key} = {old}\n")).unwrap();
        assert_eq!(config.get(key, ""), current, "{key} {old}");
    }
    for (key, kept) in [
        ("frame_limiter_provider", "rtss"),
        ("frame_limiter_provider", "none"),
        ("rtss_frame_limit_type", "back-edge-sync"),
    ] {
        let config = Config::parse(&format!("{key} = {kept}\n")).unwrap();
        assert_eq!(config.get(key, ""), kept);
    }
    // A save sending a removed encoder stores AMF.
    let mut config = Config::default();
    change(&mut config, "encoder", "nvenc").unwrap();
    assert_eq!(config.get("encoder", ""), "amf");
    // An app's or a device's own overrides.
    let mut app = serde_json::json!({"name": "Game", "frame-generation-mode": "nvidia-smooth-motion",
            "config-overrides": {"encoder": "software", "frame_limiter_provider": "nvidia-control-panel"}});
    assert!(replace_retired_in_app(app.as_object_mut().unwrap()));
    assert_eq!(
        app,
        serde_json::json!({"name": "Game", "frame-generation-mode": "none",
                "config-overrides": {"encoder": "amf", "frame_limiter_provider": "auto"}})
    );
    assert!(!replace_retired_in_app(app.as_object_mut().unwrap()));
}
#[test]
fn retired_vigem_controllers_become_their_vhf_pads() {
    let c = Config::parse("gamepad = x360\n").unwrap();
    assert_eq!(c.get("gamepad", ""), "vhf_xbox_one");
    assert_eq!(c.text().trim(), "gamepad = vhf_xbox_one");
    let c = Config::parse("gamepad=ds4\n").unwrap();
    assert_eq!(c.get("gamepad", ""), "vhf_ds4");
    for kept in ["auto", "vhf_ds5", "ds5", "vhf_xbox"] {
        let c = Config::parse(&format!("gamepad={kept}\n")).unwrap();
        assert_eq!(c.get("gamepad", ""), kept);
    }
    // Only the gamepad setting has these retired values.
    assert_eq!(
        Config::parse("unknown_key=x360\n")
            .unwrap()
            .get("unknown_key", ""),
        "x360"
    );
    assert_eq!(replacement("gamepad", "x360"), Some("vhf_xbox_one"));
    assert_eq!(replacement("gamepad", "vhf_ds4"), None);
    // An app's or a device's own settings.
    let mut app = serde_json::json!({"name": "x360", "gamepad": "ds4", "output": "x360"});
    assert!(replace_retired(app.as_object_mut().unwrap()));
    assert_eq!(
        app,
        serde_json::json!({"name": "x360", "gamepad": "vhf_ds4", "output": "x360"})
    );
    assert!(!replace_retired(app.as_object_mut().unwrap()));
    let mut app = serde_json::json!({"name": "Game", "gamepad": "x360",
            "config-overrides": {"gamepad": "ds4", "fec_percentage": "30"}});
    assert!(replace_retired_in_app(app.as_object_mut().unwrap()));
    assert_eq!(
        app,
        serde_json::json!({"name": "Game", "gamepad": "vhf_xbox_one",
                "config-overrides": {"gamepad": "vhf_ds4", "fec_percentage": "30"}})
    );
    assert!(!replace_retired_in_app(app.as_object_mut().unwrap()));
    // A device's block in the app too.
    let mut app = serde_json::json!({"name": "Game",
            "device-overrides": {"tv": {"config-overrides": {"gamepad": "x360"}}}});
    assert!(replace_retired_in_app(app.as_object_mut().unwrap()));
    assert_eq!(
        app["device-overrides"]["tv"]["config-overrides"]["gamepad"],
        "vhf_xbox_one"
    );
    assert!(!replace_retired_in_app(app.as_object_mut().unwrap()));
}
/// A Vibepollo or Rubylight 2.0 file with every setting under its
/// earlier name, a setting this host does not know, and a comment.
const EARLIER_NAMES: &str = "# Vibepollo\nrtss_path = D:\\Tools\\RTSS\ndd_vdd_static_monitor_count = \"2\"\nrtx_hdr_peak_brightness = 1400\nport = 48123\nfuture_setting = kept\n";
#[test]
fn settings_under_an_earlier_name_are_read_under_the_current_one() {
    let c = Config::parse(EARLIER_NAMES).unwrap();
    // What the host used before through its fallbacks, now under the
    // names the console shows.
    assert_eq!(c.get("rtss_install_path", ""), "D:\\Tools\\RTSS");
    assert_eq!(c.integer("dd_virtual_display_permanent_count", -1), 2);
    assert_eq!(c.integer("hdr_peak_brightness", 0), 1400);
    for (old, _) in RENAMED_KEYS {
        assert!(!c.values.contains_key(*old), "{old}");
    }
    assert_eq!(c.get("future_setting", ""), "kept");
    assert_eq!(c.port().unwrap(), 48123);
    // With both names set the host read the current one; it stays.
    let both = Config::parse(
            "rtss_install_path = C:\\RTSS\nrtss_path = D:\\RTSS\ndd_virtual_display_permanent_count = 0\ndd_vdd_static_monitor_count = 3\n",
        )
        .unwrap();
    assert_eq!(both.get("rtss_install_path", ""), "C:\\RTSS");
    assert_eq!(both.integer("dd_virtual_display_permanent_count", -1), 0);
    assert_eq!(both.values.len(), 2);
    assert_eq!(current_name("rtss_path"), "rtss_install_path");
    assert_eq!(current_name("rtss_install_path"), "rtss_install_path");
    assert_eq!(current_name("port"), "port");
}
#[test]
fn a_saved_configuration_holds_settings_under_their_current_names() {
    // The console saves every setting as held after parsing, so the
    // first save after an upgrade retires the earlier names.
    let before = Config::parse(EARLIER_NAMES).unwrap();
    let text = before.text();
    for (old, new) in RENAMED_KEYS {
        assert!(!text.contains(&format!("{old} =")), "{text}");
        assert!(text.contains(&format!("{new} =")), "{text}");
    }
    assert!(text.contains("future_setting = kept"), "{text}");
    assert_eq!(Config::parse(&text).unwrap().values, before.values);
}
#[test]
fn a_setting_sent_under_an_earlier_name_is_saved_under_the_current_one() {
    let mut config = Config::parse("dd_virtual_display_permanent_count = 1\n").unwrap();
    change(&mut config, "dd_vdd_static_monitor_count", "3").unwrap();
    assert_eq!(config.integer("dd_virtual_display_permanent_count", 0), 3);
    assert!(!config.values.contains_key("dd_vdd_static_monitor_count"));
    // Sent together, the current name wins, as in the file.
    let mut object = serde_json::Map::new();
    object.insert("rtss_path".into(), "D:\\Old".into());
    object.insert("rtss_install_path".into(), "C:\\New".into());
    config.update(&object).unwrap();
    assert_eq!(config.get("rtss_install_path", ""), "C:\\New");
    assert!(!config.values.contains_key("rtss_path"));
    // Resetting the earlier name resets the setting.
    let mut object = serde_json::Map::new();
    object.insert("rtss_path".into(), serde_json::Value::Null);
    config.update(&object).unwrap();
    assert!(!config.values.contains_key("rtss_install_path"));
    // The value is checked under the name it is saved under.
    assert!(change(&mut config, "rtss_path", "C:\\RTSS #2").is_err());
    assert!(!config.values.contains_key("rtss_install_path"));
}
#[test]
fn existing_config_round_trips() {
    let c = Config::parse("# Apollo\nport=48123\nunknown_key = custom\nprep_cmd = [\n {\"do\":\"echo #kept\",\"undo\":\"\"}\n]\n").unwrap();
    assert_eq!(c.ports().unwrap().rtsp, 48144);
    assert_eq!(Config::parse(&c.text()).unwrap().values, c.values);
    assert_eq!(c.get("unknown_key", ""), "custom");
    for (legacy, expected) in [
        ("debug", "debug"),
        ("1", "debug"),
        ("warning", "warn"),
        ("verbose", "trace"),
        ("fatal", "error"),
        ("none", "off"),
        ("invalid", "info"),
    ] {
        assert_eq!(
            Config::parse(&format!("min_log_level={legacy}\n"))
                .unwrap()
                .log_level(),
            expected
        );
    }
}
#[test]
fn exact_display_remapping_hdr_override_and_disabled_policy() {
    let c = Config::parse("dd_mode_remapping={\"mixed\":[{\"requested_resolution\":\"1920x1080\",\"requested_fps\":\"59.94\",\"final_resolution\":\"2560x1440\",\"final_refresh_rate\":\"119.88\"}]}\ndd_hdr_request_override=force_off\n").unwrap();
    let r = c
        .display_request_rate(1920, 1080, crate::framegen::Rate(59940), true, false)
        .unwrap();
    assert_eq!(r.resolution, Some((2560, 1440)));
    assert_eq!(r.refresh, Some(119880));
    assert_eq!(r.hdr, Some(false));
    let c = Config::parse("dd_configuration_option=disabled\ndd_refresh_rate_option=manual\ndd_manual_refresh_rate=119.88\n").unwrap();
    let r = c
        .display_request_rate(1920, 1080, crate::framegen::Rate(59940), true, false)
        .unwrap();
    assert_eq!((r.resolution, r.refresh, r.hdr), (None, None, None));
    assert_eq!(
        c.display_request_rate(1920, 1080, crate::framegen::Rate(59940), true, true)
            .unwrap()
            .refresh,
        Some(119880)
    );
}
#[test]
fn vibepollo_files_load_like_vibepollo() {
    let c = Config::parse(concat!(
        "\u{feff}sunshine_name = 27\" PC # living room\n",
        "not a setting\n",
        "keybindings = [0x10, 0xA0, \"0x11\", 0xA2]\n",
        "csrf_allowed_origins = [https://a.example, https://b.example]\n",
        "encoder =\n",
        "port = \"48123\"\n",
        "port = 50000\n",
        "audio_sink = Speakers\n",
        "bad key = 1\n",
        "global_prep_cmd = [\n",
        "  {\"do\":\"echo #1\",\"undo\":\"\"}\n",
        "]\n",
        "dd_snapshot_exclude_devices = DISPLAY1, \"DISPLAY2\"\n",
        "fec_percentage = 0x1e\n",
        "enable_pairing = enable\n",
    ))
    .unwrap();
    assert_eq!(c.get("sunshine_name", ""), "27\" PC");
    assert_eq!(c.list("keybindings"), ["0x10", "0xA0", "0x11", "0xA2"]);
    assert_eq!(
        c.list("csrf_allowed_origins"),
        ["https://a.example", "https://b.example"]
    );
    assert_eq!(c.get("encoder", "auto"), "auto");
    assert!(!c.values.contains_key("encoder"));
    assert_eq!(c.port().unwrap(), 48123);
    assert_eq!(c.get("audio_sink", ""), "Speakers");
    assert!(!c.values.contains_key("bad key") && !c.values.contains_key("key"));
    assert_eq!(
        c.get("global_prep_cmd", ""),
        "[\n{\"do\":\"echo #1\",\"undo\":\"\"}\n]"
    );
    assert_eq!(
        c.list("dd_snapshot_exclude_devices"),
        ["DISPLAY1", "DISPLAY2"]
    );
    assert_eq!(c.integer("fec_percentage", 20), 30);
    assert!(c.boolean("enable_pairing", false));
    assert_eq!(
        Config::parse("port = 70000\n").unwrap().port().unwrap(),
        47989
    );
    // An unclosed list is dropped; the rest of the file still loads.
    let c = Config::parse("keybindings = [0x10,\nport = 48123\n").unwrap();
    assert_eq!(c.port().unwrap(), 48123);
    assert!(!c.values.contains_key("keybindings"));
    let c = Config::parse("port = 48123\nkeybindings = [0x10,\n").unwrap();
    assert_eq!(c.port().unwrap(), 48123);
    assert!(!c.values.contains_key("keybindings"));
}
#[test]
fn files_saved_as_utf16_or_in_the_ansi_code_page_load() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("sunshine.conf");
    let text = "sunshine_name = Café € PC\r\nport = 48123 # base\r\n";
    let mut utf16 = vec![0xff, 0xfe];
    utf16.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    let mut big_endian = vec![0xfe, 0xff];
    big_endian.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
    let mut bom = b"\xef\xbb\xbf".to_vec();
    bom.extend_from_slice(text.as_bytes());
    let ansi = b"sunshine_name = Caf\xe9 \x80 PC\r\nport = 48123 # base\r\n".to_vec();
    for bytes in [utf16, big_endian, bom, ansi, text.as_bytes().to_vec()] {
        std::fs::write(&path, bytes).unwrap();
        let c = Config::load(&path).unwrap();
        assert_eq!(c.get("sunshine_name", ""), "Café € PC");
        assert_eq!(c.port().unwrap(), 48123);
        assert_eq!(c.values.len(), 2);
    }
}
#[test]
fn update_is_transactional_and_rejects_injection() {
    let mut c = Config::default();
    assert!(
        c.update(
            serde_json::json!({"port":65535,"encoder":"amf"})
                .as_object()
                .unwrap()
        )
        .is_err()
    );
    assert!(c.values.is_empty());
    assert!(
        c.update(
            serde_json::json!({"encoder":"amf\nport=1"})
                .as_object()
                .unwrap()
        )
        .is_err()
    );
}
