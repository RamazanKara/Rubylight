//! Cross-domain configuration validation, override, and display-request tests.

use anyhow::Result;
use butterpollo_config::config::{Config, HDR_OPTION, HDR_REQUEST, current_name, override_allowed};
use butterpollo_input_policy::audio_haptics::{CURVE, ENABLED, GATE, SETTINGS, STRENGTH};
use butterpollo_input_policy::couch_menu::chord::{KeyChord, OpenWith, PadButtons};
use butterpollo_media_policy::{display_policy, encoder_policy, framegen, nvidia_enhance, rtsp};
use butterpollo_session::config_ext::ConfigExt;
use butterpollo_session::device_overrides;

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
fn audio_haptics_settings_are_checked_on_save_and_override_per_device() {
    let mut config = Config::default();
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
        assert!(override_allowed(key), "{key}");
    }
}

#[test]
fn couch_menu_controls_are_checked_on_save_with_one_message_each() {
    let mut config = Config::default();
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
    assert_eq!(config.get("couch_menu_controller_chord", ""), "guide");
    assert_eq!(config.integer("couch_menu_hold_ms", 0), 3000);
    assert_eq!(config.integer("couch_menu_corner_hold_ms", 0), 3000);
    assert_eq!(
        config.get("couch_menu_keyboard_chord", ""),
        "ctrl+alt+insert"
    );
    let mut object = serde_json::Map::new();
    object.insert("couch_menu_hold_ms".into(), 299.into());
    assert!(config.update(&object).is_err());
    object.insert("couch_menu_hold_ms".into(), 750.into());
    config.update(&object).unwrap();
    let open_with = OpenWith::from_config(&config);
    assert_eq!(open_with.pad_hold, std::time::Duration::from_millis(750));
    assert!(!open_with.keyboard);
}

#[test]
fn a_device_couch_menu_override_beats_the_host_and_a_bad_one_is_refused() {
    for key in [
        "couch_menu_open_with",
        "couch_menu_controller_chord",
        "couch_menu_hold_ms",
        "couch_menu_corner_hold_ms",
        "couch_menu_keyboard_chord",
    ] {
        assert!(override_allowed(key), "{key}");
    }
    let mut config =
        Config::parse("couch_menu_controller_chord = l1+r1+y\ncouch_menu_hold_ms = 2000\n")
            .unwrap();
    let layer = |config: &mut Config, key: &str, value: serde_json::Value| {
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

#[test]
fn display_policies_keep_resolution_and_refresh_independent() {
    let config =
        Config::parse("dd_resolution_option = disabled\ndd_refresh_rate_option = auto\n").unwrap();
    let requested = display_policy::display_request(&config, 1920, 1080, 120).unwrap();
    assert_eq!(requested.resolution, None);
    assert_eq!(requested.refresh, Some(120));
    let config = Config::parse(
        "dd_resolution_option = manual\ndd_manual_resolution = 2560x1440\ndd_refresh_rate_option = disabled\n",
    )
    .unwrap();
    let requested = display_policy::display_request(&config, 1920, 1080, 120).unwrap();
    assert_eq!(requested.resolution, Some((2560, 1440)));
    assert_eq!(requested.refresh, None);
    for value in ["NaN", "inf", "0", "1001"] {
        let config = Config::parse(&format!(
            "dd_refresh_rate_option = manual\ndd_manual_refresh_rate = {value}\n"
        ))
        .unwrap();
        assert!(display_policy::display_request(&config, 1920, 1080, 60).is_err());
    }
}

#[test]
fn rtx_hdr_keeps_the_display_sdr_for_an_hdr_stream_only_where_it_runs() {
    let mut c = Config::parse("rtx_hdr=true\n").unwrap();
    c.values
        .insert(nvidia_enhance::marker("rtx_hdr"), "true".into());
    let hdr = |c: &Config, stream_hdr| {
        display_policy::display_request_rate(
            c,
            1920,
            1080,
            framegen::Rate(60000),
            stream_hdr,
            false,
        )
        .unwrap()
        .hdr
    };
    assert_eq!(hdr(&c, true), Some(true));
    c.values.insert(
        nvidia_enhance::OFFERED_KEY.into(),
        nvidia_enhance::ALL.to_string(),
    );
    assert_eq!(hdr(&c, true), Some(false));
    assert_eq!(hdr(&c, false), Some(false));
}

#[test]
fn exact_display_remapping_hdr_override_and_disabled_policy() {
    let c = Config::parse("dd_mode_remapping={\"mixed\":[{\"requested_resolution\":\"1920x1080\",\"requested_fps\":\"59.94\",\"final_resolution\":\"2560x1440\",\"final_refresh_rate\":\"119.88\"}]}\ndd_hdr_request_override=force_off\n").unwrap();
    let r =
        display_policy::display_request_rate(&c, 1920, 1080, framegen::Rate(59940), true, false)
            .unwrap();
    assert_eq!(r.resolution, Some((2560, 1440)));
    assert_eq!(r.refresh, Some(119880));
    assert_eq!(r.hdr, Some(false));
    let c = Config::parse("dd_configuration_option=disabled\ndd_refresh_rate_option=manual\ndd_manual_refresh_rate=119.88\n").unwrap();
    let r =
        display_policy::display_request_rate(&c, 1920, 1080, framegen::Rate(59940), true, false)
            .unwrap();
    assert_eq!((r.resolution, r.refresh, r.hdr), (None, None, None));
    assert_eq!(
        display_policy::display_request_rate(&c, 1920, 1080, framegen::Rate(59940), true, true)
            .unwrap()
            .refresh,
        Some(119880)
    );
}

#[test]
fn virtual_display_refresh_and_hdr_request_integrate_across_policies() {
    let c = Config::parse(
        "# Vibepollo\nframe_limiter_auto_virtual_framegen = enabled\nexperimental_packed_444 = false\n",
    )
    .unwrap();
    assert_eq!(
        framegen::virtual_refresh(&c),
        framegen::VirtualRefresh::Enabled
    );
    let mut config = Config::default();
    change(
        &mut config,
        "frame_limiter_auto_virtual_framegen",
        "disabled",
    )
    .unwrap();
    change(&mut config, "experimental_packed_444", "false").unwrap();
    assert_eq!(
        framegen::virtual_refresh(&config),
        framegen::VirtualRefresh::Disabled
    );
    assert_eq!(current_name("experimental_packed_444"), "packed_444");
    assert_eq!(
        current_name("frame_limiter_auto_virtual_framegen"),
        "virtual_display_refresh"
    );

    let hdr = |c: &Config, stream: bool| {
        display_policy::display_request_rate(c, 1920, 1080, framegen::Rate(60000), stream, false)
            .unwrap()
            .hdr
    };
    for (text, option, for_hdr, for_sdr) in [
        ("", None, Some(true), Some(false)),
        (
            "dd_hdr_option = auto\n",
            Some("auto"),
            Some(true),
            Some(false),
        ),
        (
            "dd_hdr_request_override = force_on\n",
            Some("force_on"),
            Some(true),
            Some(true),
        ),
        (
            "dd_hdr_option = auto\ndd_hdr_request_override = force_off\n",
            Some("force_off"),
            Some(false),
            Some(false),
        ),
        (
            "dd_hdr_option = disabled\ndd_hdr_request_override = force_on\n",
            Some("disabled"),
            None,
            None,
        ),
        (
            "dd_hdr_request_override = auto\n",
            None,
            Some(true),
            Some(false),
        ),
        (
            "dd_hdr_option = force_on\n",
            Some("force_on"),
            Some(true),
            Some(true),
        ),
        (
            "dd_hdr_option = force_on\ndd_hdr_request_override = force_off\n",
            Some("force_on"),
            Some(true),
            Some(true),
        ),
    ] {
        let c = Config::parse(text).unwrap();
        assert_eq!(
            c.values.get(HDR_OPTION).map(String::as_str),
            option,
            "{text}"
        );
        assert!(!c.values.contains_key(HDR_REQUEST), "{text}");
        assert_eq!(
            (hdr(&c, true), hdr(&c, false)),
            (for_hdr, for_sdr),
            "{text}"
        );
    }
    let extra = std::collections::BTreeMap::from([(
        device_overrides::SETTING.to_owned(),
        serde_json::json!({"tv": {"config-overrides": {"dd_hdr_request_override": "force_on"}}}),
    )]);
    device_overrides::check(&extra).unwrap();

    let config = Config::parse(
        "amd_input_queue_size=1\namd_lowlatency_mode=enabled\ncompute_queue_realtime=true\n",
    )
    .unwrap();
    for codec in 0..=2 {
        let stream = rtsp::Negotiated {
            codec,
            ..Default::default()
        };
        let properties = encoder_policy::amf(&config, &stream).unwrap();
        assert!(
            properties.iter().all(|property| {
                !property.name.ends_with("InputQueueSize") && property.name != "LowLatencyInternal"
            }),
            "codec {codec}"
        );
    }
}

#[test]
fn stream_preset_saves_only_a_preset_name_and_clears_with_null() {
    let mut config = Config::default();
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
    let loaded = Config::parse("stream_preset = fastest\n").unwrap();
    assert_eq!(loaded.get("stream_preset", ""), "fastest");
}

#[test]
fn background_mode_saves_only_nucleus_and_earlier_modes_become_it() {
    let mut config = Config::default();
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
            "background_mode = rdp is not a mode: the co-op helper is the only background mode (nucleus)"
        ),
        "{error}"
    );
    assert_eq!(config.get("background_mode", ""), "");
}

#[test]
fn dvr_directory_refuses_network_and_device_paths_and_allows_local_and_relative_paths() {
    let mut config = Config::default();
    for bad in [
        r"\\server\share",
        "//server/share/clips",
        r"\\?\UNC\server\share",
        r"\\.\pipe\x",
        "NUL",
        r"C:\Clips\COM1",
    ] {
        assert!(change(&mut config, "dvr_directory", bad).is_err(), "{bad}");
        let parsed = Config::parse(&format!("dvr_directory = {bad}\n")).unwrap();
        let key = "dvr_directory".to_owned();
        assert!(parsed.check_saved([&key]).is_err(), "check_saved: {bad}");
    }
    for ok in ["", "clips", r"sub\clips", r"D:\Clips", r"Z:\Clips"] {
        change(&mut config, "dvr_directory", ok).unwrap();
        assert_eq!(config.get("dvr_directory", ""), ok);
    }
}
