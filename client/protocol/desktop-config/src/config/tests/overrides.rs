//! Unit tests for desktop setting migrations, overrides and platform defaults.

use super::super::*;

#[test]
fn grouped_video_packets_are_on_by_default_and_in_old_files() {
    assert!(Settings::default().receive_coalescing);
    let old: Settings = serde_json::from_str(r#"{"fps":60,"client_roaming":false}"#).unwrap();
    assert!(old.receive_coalescing);
    let off: Settings = serde_json::from_str(r#"{"receive_coalescing":false}"#).unwrap();
    assert!(!off.receive_coalescing);
}

#[test]
fn recover_interrupted_streams_turns_on_once_and_a_later_off_is_kept() {
    assert!(Settings::default().auto_reconnect);
    let dir = std::env::temp_dir().join(format!("nightjar-reconnect-{}", std::process::id()));
    let store = Store::at(dir.clone()).unwrap();
    std::fs::write(dir.join("settings.json"), r#"{"auto_reconnect": false}"#).unwrap();
    let mut settings = store.settings();
    assert!(settings.auto_reconnect, "an old file's off is switched on once");
    settings.auto_reconnect = false;
    store.save_settings(&settings).unwrap();
    assert!(!store.settings().auto_reconnect, "the user's off after the switch is kept");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn tear_free_vrr_is_on_by_default() {
    assert!(Settings::default().vrr_tear_free);
}

#[test]
fn real_vrr_turns_on_once_and_a_later_off_is_kept() {
    assert!(Settings::default().present_at_host_times);
    let dir = std::env::temp_dir().join(format!("nightjar-real-vrr-{}", std::process::id()));
    let store = Store::at(dir.clone()).unwrap();
    std::fs::write(dir.join("settings.json"), r#"{"present_at_host_times": false, "auto_reconnect_default_on": true}"#)
        .unwrap();
    let mut settings = store.settings();
    assert!(settings.present_at_host_times, "an old file's off is switched on once");
    settings.present_at_host_times = false;
    store.save_settings(&settings).unwrap();
    assert!(!store.settings().present_at_host_times, "the user's off after the switch is kept");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_saved_pc_render_resolution_is_kept_in_androids_range() {
    let dir = std::env::temp_dir().join(format!("nightjar-render-scale-{}", std::process::id()));
    let store = Store::at(dir.clone()).unwrap();
    for (saved, read) in [(300, 200), (225, 200), (200, 200), (150, 150), (50, 50), (25, 50)] {
        std::fs::write(dir.join("settings.json"), format!(r#"{{"virtual_display_scale": {saved}}}"#)).unwrap();
        assert_eq!(store.settings().virtual_display_scale, read, "settings.json at {saved} %");
        // A profile's, PC's or game's own value is kept in the range too.
        let own: Overrides = serde_json::from_str(&format!(r#"{{"virtual_display_scale": {saved}}}"#)).unwrap();
        assert_eq!(own.apply(&Settings::default()).virtual_display_scale, read, "an override at {saved} %");
    }
    assert_eq!(Settings::default().virtual_display_scale, 100);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_saved_pc_display_scaling_windows_does_not_offer_keeps_the_pcs_own() {
    let dir = std::env::temp_dir().join(format!("nightjar-display-scale-{}", std::process::id()));
    let store = Store::at(dir.clone()).unwrap();
    for (saved, read) in [(175, 175), (500, 500), (0, 0), (275, 0), (600, 0), (99, 0)] {
        std::fs::write(dir.join("settings.json"), format!(r#"{{"virtual_display_dpi": {saved}}}"#)).unwrap();
        assert_eq!(store.settings().virtual_display_dpi, read, "settings.json at {saved} %");
        let own: Overrides = serde_json::from_str(&format!(r#"{{"virtual_display_dpi": {saved}}}"#)).unwrap();
        assert_eq!(own.apply(&Settings::default()).virtual_display_dpi, read, "an override at {saved} %");
    }
    // Files from before the setting keep the PC's own scale and the render resolution apart.
    std::fs::write(dir.join("settings.json"), r#"{"virtual_display_scale": 150}"#).unwrap();
    assert_eq!((store.settings().virtual_display_dpi, store.settings().virtual_display_scale), (0, 150));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn image_enhancements_follow_the_pcs_app_setting_until_a_pc_chooses() {
    let old = serde_json::from_str::<Settings>("{}").unwrap();
    assert_eq!((old.rtx_hdr, old.rtx_video), (None, None));
    assert_eq!((Settings::default().rtx_hdr, Settings::default().rtx_video), (None, None));
    let own: Overrides = serde_json::from_str(r#"{"rtx_hdr": true, "rtx_video": false}"#).unwrap();
    let applied = own.apply(&Settings::default());
    assert_eq!((applied.rtx_hdr, applied.rtx_video), (Some(true), Some(false)));
    let follow: Overrides = serde_json::from_str(r#"{"rtx_hdr": null}"#).unwrap();
    assert_eq!(follow.apply(&Settings { rtx_hdr: Some(true), ..Settings::default() }).rtx_hdr, None);
}

#[test]
fn clipboard_sync_is_off_by_default_and_in_old_files() {
    assert!(!Settings::default().clipboard_sync);
    assert!(!serde_json::from_str::<Settings>("{}").unwrap().clipboard_sync);
    assert!(serde_json::from_str::<Settings>(r#"{"clipboard_sync":true}"#).unwrap().clipboard_sync);
}

#[test]
fn usb_forwarding_is_off_by_default_and_in_old_files() {
    assert!(!Settings::default().usb_forwarding);
    assert!(!serde_json::from_str::<Settings>("{}").unwrap().usb_forwarding);
    assert!(serde_json::from_str::<Settings>(r#"{"usb_forwarding":true}"#).unwrap().usb_forwarding);
    assert!(Settings::default().usb_auto.is_empty());
    assert!(serde_json::from_str::<Settings>("{}").unwrap().usb_auto.is_empty());
    assert_eq!(serde_json::from_str::<Settings>(r#"{"usb_auto":["28de:1304"]}"#).unwrap().usb_auto, ["28de:1304"]);
}

#[test]
fn telling_the_host_the_link_speed_is_on_by_default_and_in_old_files() {
    assert!(Settings::default().tell_host_link_speed);
    let old: Settings = serde_json::from_str(r#"{"fps":60,"tell_host_wireless":false}"#).unwrap();
    assert!(old.tell_host_link_speed && !old.tell_host_wireless);
    let off: Settings = serde_json::from_str(r#"{"tell_host_link_speed":false}"#).unwrap();
    assert!(!off.tell_host_link_speed);
}

/// The one switch for both hints writes both keys: a file that saved only one of them off loads with
/// both off, in the main settings and in a layer.
#[test]
fn one_network_hint_off_turns_both_off() {
    for key in ["tell_host_wireless", "tell_host_link_speed"] {
        let json = format!(r#"{{"{key}":false}}"#);
        let mut settings: Settings = serde_json::from_str(&json).unwrap();
        settings.migrate();
        assert!(!settings.tell_host_wireless && !settings.tell_host_link_speed, "{key}");
        let layer: Overrides = serde_json::from_str(&json).unwrap();
        assert_eq!(layer.0.get("tell_host_wireless"), Some(&serde_json::json!(false)), "{key}");
        assert_eq!(layer.0.get("tell_host_link_speed"), Some(&serde_json::json!(false)), "{key}");
    }
    let mut on: Settings = serde_json::from_str("{}").unwrap();
    on.migrate();
    assert!(on.tell_host_wireless && on.tell_host_link_speed);
    let layer: Overrides = serde_json::from_str(r#"{"tell_host_wireless":true}"#).unwrap();
    assert!(!layer.0.contains_key("tell_host_link_speed"));
}

/// Lower bitrate's three switches fold into its one value once, in the main settings and in every
/// override layer: a player who had it off keeps it off, and the old keys are never written again.
#[test]
fn the_three_bitrate_switches_fold_into_lower_bitrate_once() {
    use nightjar_policy::choices::LowerBitrate;
    assert_eq!(Settings::default().lower_bitrate, LowerBitrate::Off);
    let key = |name: &str, value: Option<bool>| value.map(|v| format!(r#""{name}":{v},"#)).unwrap_or_default();
    for busy in [None, Some(false), Some(true)] {
        for lost in [None, Some(false), Some(true)] {
            for keep in [None, Some(false), Some(true)] {
                let fields = format!("{}{}{}", key("adaptive_bitrate", busy), key("client_hint_bitrate", lost), key("keep_bitrate", keep));
                let expected = if keep == Some(true) {
                    LowerBitrate::Keep
                } else if busy == Some(true) || lost == Some(true) {
                    LowerBitrate::WhenFramesLost
                } else {
                    LowerBitrate::Off
                };
                let mut s: Settings = serde_json::from_str(&format!("{{{fields}\"fps\":60}}")).unwrap();
                s.migrate();
                assert_eq!(s.lower_bitrate, expected, "{fields}");
                let saved = serde_json::to_string(&s).unwrap();
                assert!(!saved.contains("adaptive_bitrate") && !saved.contains("client_hint_bitrate") && !saved.contains("keep_bitrate"), "{saved}");
                assert!(saved.contains(&format!(r#""lower_bitrate":"{}""#, expected.as_str())), "{saved}");
                // Migrating again changes nothing.
                let mut again: Settings = serde_json::from_str(&saved).unwrap();
                again.migrate();
                assert_eq!(again, s);

                // An override layer holds the choice as its own, over a main Never.
                let layer: Overrides = serde_json::from_str(&format!("{{{fields}\"fps\":60}}")).unwrap();
                let never = Settings { lower_bitrate: LowerBitrate::Keep, ..Settings::default() };
                let applied = layer.apply(&never);
                let layered = if busy.is_none() && lost.is_none() && keep.is_none() { LowerBitrate::Keep } else { expected };
                assert_eq!(applied.lower_bitrate, layered, "layer {fields}");
                let json = serde_json::to_string(&layer).unwrap();
                assert!(!json.contains("adaptive_bitrate") && !json.contains("keep_bitrate"), "{json}");
            }
        }
    }
    // A layer saved under the new key keeps it.
    let both: Overrides = serde_json::from_str(r#"{"keep_bitrate":true,"lower_bitrate":"sooner"}"#).unwrap();
    assert_eq!(both.apply(&Settings::default()).lower_bitrate, LowerBitrate::WhenFramesLost);
}

#[test]
fn the_link_test_before_the_first_bitrate_is_off_by_default_and_in_old_files() {
    assert!(!Settings::default().link_probe);
    let old: Settings = serde_json::from_str(r#"{"fps":60,"bitrate_kbps":0}"#).unwrap();
    assert!(!old.link_probe);
    let on: Settings = serde_json::from_str(r#"{"link_probe":true}"#).unwrap();
    assert!(on.link_probe);
}

#[test]
fn instant_input_and_stream_priority_are_on_by_default_and_round_trip() {
    let defaults = Settings::default();
    assert!(defaults.unbatched_input && defaults.network_priority);
    // A settings file from before the fields existed turns both on, as Android does.
    let old: Settings = serde_json::from_str(r#"{"fps":60,"network_grace_ms":5000}"#).unwrap();
    assert!(old.unbatched_input && old.network_priority);
    let off = Settings { unbatched_input: false, network_priority: false, ..Settings::default() };
    let back: Settings = serde_json::from_str(&serde_json::to_string(&off).unwrap()).unwrap();
    assert_eq!(back, off);
    assert_eq!(serde_json::from_str::<Settings>(&serde_json::to_string(&defaults).unwrap()).unwrap(), defaults);
}

#[test]
fn calibration_follows_the_route_to_the_host_not_the_internet_connection() {
    use crate::network_types::{Fallback, Network, Toward};
    let wired = PaceCalibration { pace_mbps: 900, client_link_mbps: 2500, host_link_mbps: 2500,
        wifi: false, ssid: None, qualified: true };
    // Wi-Fi carries the internet, but the PC is on the local Ethernet segment.
    let wifi = Network { wifi: true, ssid: Some("Home".into()), link_mbps: 866, ..Default::default() };
    let route = wifi.toward(Ok(Toward { wireless: false, link_mbps: 2500 }));
    assert_eq!(wired.for_network(&route, 2500), 900);
    // A Wi-Fi calibration stays found when the route to the PC is the Wi-Fi.
    let on_wifi = PaceCalibration { pace_mbps: 400, client_link_mbps: 866, host_link_mbps: 2500,
        wifi: true, ssid: Some("Home".into()), qualified: true };
    let route = wifi.toward(Ok(Toward { wireless: true, link_mbps: 0 }));
    assert_eq!(route.link_mbps, 866);
    assert_eq!(on_wifi.for_network(&route, 2500), 400);
    // Without a route answer the internet connection decides, as before.
    assert_eq!(wired.for_network(&wifi.toward(Err(Fallback::NotTelling)), 2500), 0);
}

#[test]
fn calibration_is_used_only_on_the_measured_network_and_link() {
    let mut network = crate::network_types::Network { wifi: true, ssid: Some("Home".into()), link_mbps: 1200, ..Default::default() };
    let calibration = PaceCalibration { pace_mbps: 900, client_link_mbps: 1200, host_link_mbps: 2500,
        wifi: true, ssid: Some("Home".into()), qualified: true };
    assert_eq!(calibration.for_network(&network, 2500), 900);
    assert_eq!(calibration.for_network(&network, 1000), 0);
    network.ssid = Some("Hotel".into());
    assert_eq!(calibration.for_network(&network, 2500), 0);
    network.ssid = None;
    assert_eq!(calibration.for_network(&network, 2500), 0);
    let restored: PaceCalibration = serde_json::from_str(&serde_json::to_string(&calibration).unwrap()).unwrap();
    assert_eq!(restored, calibration);
}

#[test]
fn old_pc_snapshots_and_explicit_false_survive_serialization() {
    let settings = Settings { fps: 90, microphone: false, ..Default::default() };
    let old: Overrides = serde_json::from_value(serde_json::to_value(&settings).unwrap()).unwrap();
    assert_eq!(old.apply(&Settings { fps: 144, microphone: true, ..Default::default() }), settings);
    let mut own: Overrides = serde_json::from_str(r#"{"microphone":false}"#).unwrap();
    assert_eq!(serde_json::to_string(&own).unwrap(), r#"{"microphone":false}"#);
    own.clear(&["microphone"]);
    assert!(own.is_empty());
    assert!(serde_json::from_str::<Overrides>(r#"{"fps":"bad"}"#).is_err());
    assert!(serde_json::from_str::<Overrides>(r#"{"codec":"unknown"}"#).is_err());
}

#[test]
fn the_overlay_keeps_androids_values_and_defaults() {
    let defaults = Settings::default();
    assert!(!defaults.overlay);
    assert_eq!((defaults.overlay_mode, defaults.overlay_position), (OverlayMode::Compact, OverlayCorner::TopLeft));
    let old: Settings = serde_json::from_str(r#"{"overlay":true}"#).unwrap();
    assert_eq!((old.overlay, old.overlay_mode, old.overlay_position), (true, OverlayMode::Compact, OverlayCorner::TopLeft));
    let mode = |value: &str| serde_json::from_value::<OverlayMode>(serde_json::json!(value)).unwrap();
    assert_eq!(["compact", "statistics", "debug"].map(mode), OverlayMode::ALL);
    // Android's normalizeMode: the old names, and anything else reads as Compact.
    assert_eq!(["advanced", "expanded", "newer"].map(mode), [OverlayMode::Debug, OverlayMode::Statistics, OverlayMode::Compact]);
    let corner = |value: &str| serde_json::from_value::<OverlayCorner>(serde_json::json!(value)).unwrap();
    assert_eq!(["top_left", "top_right", "bottom_left", "bottom_right"].map(corner), OverlayCorner::ALL);
    assert_eq!(corner("middle"), OverlayCorner::TopLeft);
    let set = Settings { overlay_mode: OverlayMode::Debug, overlay_position: OverlayCorner::BottomRight, ..Default::default() };
    let json = serde_json::to_value(&set).unwrap();
    assert_eq!((&json["overlay_mode"], &json["overlay_position"]), (&serde_json::json!("debug"), &serde_json::json!("bottom_right")));
}

#[test]
fn quick_resume_and_input_only_on_any_pc_are_off_by_default_and_in_old_files() {
    let old: Settings = serde_json::from_str(r#"{"fps":60}"#).unwrap();
    assert!(!old.quick_resume && !old.input_only_any_host);
    assert!(!Settings::default().quick_resume && !Settings::default().input_only_any_host);
    let on: Settings = serde_json::from_str(r#"{"quick_resume":true,"input_only_any_host":true}"#).unwrap();
    assert!(on.quick_resume && on.input_only_any_host);
}

#[test]
fn home_view_keeps_the_rows_per_store_until_chosen_and_reads_back() {
    let old: Settings = serde_json::from_str(r#"{"fps":60}"#).unwrap();
    assert_eq!((old.home_layout, old.home_group, old.home_card_size), (HomeLayout::Rows, HomeGroup::Store, CardSize::Medium));
    assert_eq!(Settings::default().home_layout, HomeLayout::Rows);
    let set = Settings { home_layout: HomeLayout::Grid, home_group: HomeGroup::None, home_card_size: CardSize::Large, ..Default::default() };
    let json = serde_json::to_value(&set).unwrap();
    // The words Android stores for the same choices.
    assert_eq!(
        (&json["home_layout"], &json["home_group"], &json["home_card_size"]),
        (&serde_json::json!("grid"), &serde_json::json!("none"), &serde_json::json!("large"))
    );
    assert_eq!(serde_json::from_value::<Settings>(json).unwrap(), set);
    // By type is Android's "type" too.
    let by_collection: Settings = serde_json::from_str(r#"{"home_group":"collection"}"#).unwrap();
    assert_eq!(by_collection.home_group, HomeGroup::Collection);
    let by_type: Settings = serde_json::from_str(r#"{"home_group":"type"}"#).unwrap();
    assert_eq!(by_type.home_group, HomeGroup::Type);
    assert_eq!(serde_json::to_value(by_type).unwrap()["home_group"], serde_json::json!("type"));
    // A value from a newer Nightjar keeps the default instead of dropping every setting.
    let newer: Settings = serde_json::from_str(r#"{"home_layout":"carousel","home_card_size":"huge","fps":90}"#).unwrap();
    assert_eq!((newer.home_layout, newer.home_card_size, newer.fps), (HomeLayout::Rows, CardSize::Medium, 90));
    assert!(CardSize::Small.scale() < CardSize::Medium.scale() && CardSize::Medium.scale() < CardSize::Large.scale());
}

/// "More settings" and Home's setup card keep their state in settings.json without being settings:
/// an older file reads them folded and unoffered, and they read back as saved.
#[test]
fn ui_state_beside_the_settings_reads_back_and_defaults_closed() {
    let old: Settings = serde_json::from_str(r#"{"fps":60}"#).unwrap();
    assert!(!old.settings_more_open && old.setup_card_host.is_none() && !old.setup_card_seen);
    let set = Settings { settings_more_open: true, setup_card_host: Some(42), setup_card_seen: true, ..Default::default() };
    let json = serde_json::to_value(&set).unwrap();
    assert_eq!((&json["settings_more_open"], &json["setup_card_host"], &json["setup_card_seen"]), (&serde_json::json!(true), &serde_json::json!(42), &serde_json::json!(true)));
    assert_eq!(serde_json::from_value::<Settings>(json).unwrap(), set);
    // A PC's, game's or profile's values never carry them: they apply over the main settings' own.
    let mut own = Overrides::default();
    own.capture(&Settings { fps: 120, ..Settings::default() }, &["fps"]);
    let applied = own.apply(&set);
    assert_eq!((applied.fps, applied.settings_more_open, applied.setup_card_host, applied.setup_card_seen), (120, true, Some(42), true));
}

#[test]
fn quit_on_leave_is_kept_per_game_and_a_known_uuid_decides_alone() {
    let mut record = HostRecord::default();
    assert!(!record.quits_on_leave("game", 5));
    record.set_quit_on_leave("game", 5, true);
    assert_eq!(record.quit_on_leave.iter().map(String::as_str).collect::<Vec<_>>(), ["id:5", "uuid:game"]);
    assert!(record.quits_on_leave("game", 5) && record.quits_on_leave("game", 9));
    assert!(record.quits_on_leave("", 5), "a host without UUIDs matches the ID");
    assert!(!record.quits_on_leave("other", 5), "a reused ID with another UUID does not");
    record.set_quit_on_leave("", 0, true);
    assert_eq!(record.quit_on_leave.len(), 2, "no ID and no UUID keeps nothing");
    assert!(!record.quits_on_leave("", 0));
    let restored: HostRecord = serde_json::from_str(&serde_json::to_string(&record).unwrap()).unwrap();
    assert_eq!(restored, record);
    record.set_quit_on_leave("game", 5, false);
    assert!(record.quit_on_leave.is_empty() && !record.quits_on_leave("", 5));
    let old: HostRecord = serde_json::from_str(r#"{"name":"PC"}"#).unwrap();
    assert!(old.quit_on_leave.is_empty());
}

#[test]
fn files_are_replaced_whole_and_only_the_user_reads_them() {
    let dir = std::env::temp_dir().join(format!("nightjar-private-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    let store = Store::at(dir.clone()).unwrap();
    let identity = Identity { unique_id: "id".into(), certificate_pem: "cert".into(), private_key_pem: "key".into() };
    store.save_identity(&identity).unwrap();
    store.save_identity(&identity).unwrap();
    let names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["identity.json"], "no temporary file is left behind");
    assert!(store.identity().unwrap().is_some());
    // Linux/macOS: POSIX file mode bits check that the private key is 0o600.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join("identity.json")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the private key is the user's alone");
    }
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn an_override_this_version_cannot_read_keeps_the_base_setting() {
    let base = Settings { fps: 60, ..Default::default() };
    let overrides = Overrides(serde_json::Map::from_iter([("fps".to_owned(), serde_json::json!(-1))]));
    assert_eq!(overrides.apply(&base), base, "a bad override costs its entry, not the UI");
}

#[test]
fn an_unreadable_file_keeps_its_bytes_and_refuses_a_save_over_it() {
    let dir = std::env::temp_dir().join(format!("nightjar-unreadable-file-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    let store = Store::at(dir.clone()).unwrap();
    // A directory where the file should be: the read fails for a reason that is not "no file".
    std::fs::create_dir(dir.join("hosts.json")).unwrap();
    assert!(store.hosts().is_empty(), "a default stands in for the contents");
    let error = store.save_hosts(&[]).expect_err("the unreadable file is not replaced");
    assert!(error.to_string().contains("left as it is"), "{error}");
    assert!(dir.join("hosts.json").is_dir(), "it is still there");

    std::fs::create_dir(dir.join("settings.json")).unwrap();
    let _ = store.settings();
    assert!(store.save_settings(&Settings::default()).is_err(), "nor the settings over it");
    assert!(dir.join("settings.json").is_dir());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn an_identity_that_cannot_be_read_is_kept_and_never_replaced() {
    let dir = std::env::temp_dir().join(format!("nightjar-identity-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    let store = Store::at(dir.clone()).unwrap();
    assert_eq!(store.identity().unwrap(), None, "no file: a new key can be made");
    let damaged = br#"{"unique_id":"kept","certificate_pem":"#;
    std::fs::write(dir.join("identity.json"), damaged).unwrap();
    assert!(store.identity().is_err(), "a file this version cannot read is not none");
    assert!(dir.join("identity.json.bad").is_file(), "a copy is kept beside it");
    let identity = Identity { unique_id: "new".into(), certificate_pem: "cert".into(), private_key_pem: "key".into() };
    assert!(store.save_identity(&identity).is_err(), "no new identity is written over it");
    assert_eq!(std::fs::read(dir.join("identity.json")).unwrap(), damaged, "the pairing key stays as it was");
    std::fs::remove_dir_all(dir).ok();
}

/// Settings saved before "Sharper colored text", the shared value lists and the merged
/// Controllers choice read on, in every layer, and a second load changes nothing.
#[test]
fn earlier_settings_read_as_the_merged_choices_in_every_layer() {
    let dir = std::env::temp_dir().join(format!("nightjar-merged-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    let store = Store::at(dir.clone()).unwrap();
    // Main settings: only "Prefer YUV 4:4:4" on, a 12-second wait, desktop's old stepper floors.
    std::fs::write(dir.join("settings.json"), r#"{"prefer_yuv444":true,"network_grace_ms":12000,"quality_min_short_edge":256,"quality_min_fps":75,"controllers":false,"mic_gain":1000}"#).unwrap();
    let settings = store.settings();
    assert!(settings.prefer_yuv444 && settings.experimental_packed_444, "either on reads as Automatic");
    assert_eq!((settings.network_grace_ms, settings.quality_min_short_edge, settings.quality_min_fps), (15_000, 360, 60));
    assert_eq!(settings.mic_gain, 400);
    assert!(!settings.controllers && settings.multi_controller, "Controllers off stays Off; a player each comes back with it");
    store.save_settings(&settings).unwrap();
    assert_eq!(store.settings(), settings, "a second load changes nothing");
    // The defaults stay as they were: 3 s reads as Off and is kept.
    let defaults = Settings::default();
    let mut migrated = defaults.clone();
    migrated.migrate();
    assert_eq!(migrated, defaults);
    assert_eq!((defaults.network_grace_ms, defaults.quality_min_short_edge, defaults.quality_min_fps), (3000, 720, 30));
    assert!(!defaults.prefer_yuv444 && !defaults.experimental_packed_444 && defaults.controllers && defaults.multi_controller && defaults.sops);
    // A PC's, a game's and a profile's own values: either key gets both, neither stays unset.
    let layer = |json: &str| -> Overrides { serde_json::from_str(json).unwrap() };
    let packed = layer(r#"{"experimental_packed_444":true}"#);
    assert!(packed.has(&["prefer_yuv444"]) && packed.apply(&Settings::default()).prefer_yuv444);
    let off = layer(r#"{"prefer_yuv444":false}"#);
    let on = Settings { prefer_yuv444: true, experimental_packed_444: true, ..Settings::default() };
    let applied = off.apply(&on);
    assert!(!applied.prefer_yuv444 && !applied.experimental_packed_444, "a layer's Off wins over the main Automatic");
    let neither = layer(r#"{"fps":120}"#);
    assert!(!neither.has(&SHARP_COLOR_KEYS) && neither.apply(&on).experimental_packed_444, "inherited");
    assert_eq!(layer(r#"{"network_grace_ms":5000}"#).apply(&Settings::default()).network_grace_ms, 5000, "Off as stored");
    assert_eq!(layer(r#"{"quality_min_fps":15}"#).apply(&Settings::default()).quality_min_fps, 30);
    let hosts_file = serde_json::json!([{"name": "Desk", "settings": {"experimental_packed_444": true}, "app_settings": {"id:7": {"prefer_yuv444": true, "experimental_packed_444": false}}}]);
    std::fs::write(dir.join("hosts.json"), hosts_file.to_string()).unwrap();
    let hosts = store.hosts();
    let own = hosts[0].settings.as_ref().unwrap();
    assert_eq!(serde_json::to_value(own).unwrap(), serde_json::json!({"experimental_packed_444": true, "prefer_yuv444": true}));
    let game = hosts[0].app_settings("", 7).unwrap();
    assert_eq!(serde_json::to_value(game).unwrap(), serde_json::json!({"experimental_packed_444": true, "prefer_yuv444": true}));
    std::fs::write(dir.join("profiles.json"), r#"{"entries":[{"id":1,"name":"Travel","settings":{"prefer_yuv444":true,"network_grace_ms":40000}}]}"#).unwrap();
    let profile = &store.profiles().entries[0].settings;
    assert_eq!(serde_json::to_value(profile).unwrap(), serde_json::json!({"prefer_yuv444": true, "experimental_packed_444": true, "network_grace_ms": 30000}));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn fractional_v_sync_is_off_in_settings_saved_before_it_and_keeps_a_choice() {
    let old: Settings = serde_json::from_str(r#"{"vrr": false, "vsync": true}"#).unwrap();
    assert!(!old.fractional_vsync, "off by default (latency rises: client/docs/settings.md)");
    let on: Settings = serde_json::from_str(r#"{"fractional_vsync": true}"#).unwrap();
    assert!(on.fractional_vsync);
    let saved = serde_json::to_value(&on).unwrap();
    assert_eq!(saved["fractional_vsync"], serde_json::Value::Bool(true), "saved under its own name");
}

#[test]
fn native_metalfx_settings_roundtrip_without_requesting_sdl_gpu_shaders() {
    let settings = Settings { upscaling: Upscaling::MetalFx, ..Settings::default() };
    assert_eq!(settings.gpu_need(), None);
    assert!(!settings.upscaling.is_shader(), "MetalFX does not use the sharpness shader setting");
    let stored = serde_json::to_value(&settings).unwrap();
    assert_eq!(stored["upscaling"], "MetalFx");
    let restored: Settings = serde_json::from_value(stored).unwrap();
    assert_eq!(restored.upscaling, Upscaling::MetalFx);
    for (name, value) in [("Bilinear", Upscaling::Bilinear), ("Fsr1", Upscaling::Fsr1), ("Sgsr1", Upscaling::Sgsr1), ("Ai", Upscaling::Ai)] {
        assert_eq!(serde_json::from_value::<Upscaling>(serde_json::json!(name)).unwrap(), value);
    }
}
