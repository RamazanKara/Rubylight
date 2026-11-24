//! Unit tests for desktop settings defaults, deserialization and compatibility.

use super::super::*;

#[test]
fn home_is_on_unless_the_file_says_otherwise() {
    assert!(Settings::default().console_home);
    let old: Settings = serde_json::from_str("{\"fps\": 90}").unwrap();
    assert!(old.console_home, "a settings file from before Home keeps it on");
    let off: Settings = serde_json::from_str("{\"console_home\": false}").unwrap();
    assert!(!off.console_home);
}

/// The simple layer's choices are stored beside the settings they write; a file from before them
/// reads as the recommendation, and reading or saving it changes none of the values above.
#[test]
fn the_simple_choices_default_to_the_recommendation_and_read_back() {
    use nightjar_policy::simple_profile::{Hdr, Picture, Smoothness};
    let s = Settings::default();
    assert_eq!((s.simple_smoothness, s.simple_picture, s.simple_hdr), (Smoothness::Auto, Picture::Auto, Hdr::Auto));
    let old: Settings = serde_json::from_str("{\"frame_pacing\":\"Latency\",\"bitrate_kbps\":40000}").unwrap();
    assert_eq!((old.simple_smoothness, old.simple_picture, old.simple_hdr), (Smoothness::Auto, Picture::Auto, Hdr::Auto));
    assert_eq!((old.frame_pacing, old.bitrate_kbps), (crate::config::FramePacing::Latency, 40_000), "nothing above changed");
    let saved = Settings {
        simple_smoothness: Smoothness::Smoothest,
        simple_picture: Picture::SaveData,
        simple_hdr: Hdr::Off,
        ..Settings::default()
    };
    let json = serde_json::to_string(&saved).unwrap();
    let read: Settings = serde_json::from_str(&json).unwrap();
    assert_eq!((read.simple_smoothness, read.simple_picture, read.simple_hdr), (Smoothness::Smoothest, Picture::SaveData, Hdr::Off));
    assert!(json.contains("\"simple_smoothness\":\"smoothest\""), "{json}");
}

#[test]
fn the_theme_choice_keeps_the_stored_values_of_the_older_choices() {
    // Names, not positions: the file keeps reading after Light was added last.
    for (theme, value) in [(Theme::System, "System"), (Theme::Dark, "Dark"), (Theme::Oled, "Oled"), (Theme::Light, "Light")] {
        assert_eq!(serde_json::to_value(theme).unwrap(), value, "{theme:?}");
        assert_eq!(serde_json::from_value::<Theme>(serde_json::Value::String(value.into())).unwrap(), theme);
    }
    assert_eq!(Theme::ALL.len(), 4);
    assert_eq!(Theme::default(), Theme::Dark);
}

#[test]
fn choosing_nightjars_wait_forgets_the_saved_untrusted_displays() {
    let dir = std::env::temp_dir().join(format!("nightjar-scan-untrusted-clear-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = Store::at(dir.clone()).unwrap();
    let first = r"\\?\DISPLAY#MON1#5&1&0&UID1#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
    let second = r"\\?\DISPLAY#MON2#5&1&0&UID2#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
    store.save_scan_untrusted(&ScanUntrusted { displays: vec![first.into(), second.into()] }).unwrap();
    // Nightjar's wait forgets the display the user chose it for, and keeps the others.
    store.clear_scan_untrusted(Some(first)).unwrap();
    assert_eq!(Store::at(dir.clone()).unwrap().scan_untrusted().displays, vec![second.to_owned()]);
    // The choice in the main settings forgets them all.
    store.clear_scan_untrusted(None).unwrap();
    assert!(Store::at(dir.clone()).unwrap().scan_untrusted().displays.is_empty());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_settings_file_from_before_accessibility_loads_with_the_defaults() {
    let old: Settings = serde_json::from_str("{\"fps\": 90}").unwrap();
    assert_eq!(old.ui_zoom, 0, "Auto is the default");
    assert!(!old.high_contrast);
    assert_eq!(old.reduce_motion, ReduceMotion::System);
    assert_eq!(old.language, Language::System);
    let set: Settings =
        serde_json::from_str("{\"ui_zoom\":150,\"high_contrast\":true,\"reduce_motion\":\"Reduce\",\"language\":\"German\"}").unwrap();
    assert_eq!((set.ui_zoom, set.high_contrast, set.reduce_motion, set.language), (150, true, ReduceMotion::Reduce, Language::German));
}

#[test]
fn a_value_from_a_newer_version_does_not_reset_the_other_settings() {
    let set: Settings = serde_json::from_str("{\"fps\":90,\"reduce_motion\":\"Sometimes\",\"language\":\"Klingon\",\"ui_zoom\":130}").unwrap();
    assert_eq!((set.fps, set.ui_zoom), (90, 130));
    assert_eq!((set.reduce_motion, set.language), (ReduceMotion::System, Language::System));
}

#[test]
fn zoom_steps_stay_inside_the_range_and_odd_values_snap() {
    assert_eq!(step_zoom(100, 1), 115);
    assert_eq!(step_zoom(100, -1), 85);
    assert_eq!(step_zoom(150, 1), 200, "150 % is no longer the top");
    assert_eq!(step_zoom(200, 1), 250);
    assert_eq!(step_zoom(250, 1), 250, "the top step stays");
    assert_eq!(step_zoom(250, -1), 200);
    assert_eq!(nearest_zoom(230), 250);
    assert_eq!(nearest_zoom(170), 150);
    assert_eq!(step_zoom(85, -1), 85, "the bottom step stays");
    assert_eq!(nearest_zoom(0), 0, "Auto stays Auto");
    assert_eq!(nearest_zoom(9999), 250);
    assert_eq!(nearest_zoom(120), 115);
    assert_eq!(step_zoom(120, 1), 130);
    let set: Settings = serde_json::from_str("{\"ui_zoom\":250}").unwrap();
    assert_eq!(nearest_zoom(set.ui_zoom), 250, "a stored 250 % loads as it is");
}

#[test]
fn auto_zoom_follows_the_screens_height_in_points() {
    assert_eq!(auto_zoom(Some(800.0), 1.0), 100, "a Deck's 1280x800 panel stays at 100 %");
    assert_eq!(auto_zoom(Some(1080.0), 1.0), 100, "1080p stays at 100 %");
    assert_eq!(auto_zoom(Some(1440.0), 1.0), 100, "1440p unscaled keeps today's size");
    assert_eq!(auto_zoom(Some(2160.0), 1.0), 200, "4K unscaled, a docked Deck on a TV");
    assert_eq!(auto_zoom(Some(1440.0), 1.5), 100, "4K at OS scale 1.5: the OS already scales");
    assert_eq!(auto_zoom(Some(1080.0), 2.0), 100, "4K at OS scale 2");
    assert_eq!(auto_zoom(Some(4320.0), 1.0), 250, "clamped at 250 %");
    assert_eq!(auto_zoom(None, 1.0), 100, "an unknown screen size stays at 100 %");
}

#[test]
fn the_old_default_zoom_moves_to_auto_once() {
    let mut old: Settings = serde_json::from_str("{\"ui_zoom\":100}").unwrap();
    assert!(!old.ui_zoom_auto_migrated, "a file from before Auto has no marker");
    old.migrate();
    assert_eq!(old.ui_zoom, 0, "the old default 100 % becomes Auto");
    assert!(old.ui_zoom_auto_migrated);
    old.ui_zoom = 100;
    old.migrate();
    assert_eq!(old.ui_zoom, 100, "a 100 % chosen after the migration is kept");
    let mut other: Settings = serde_json::from_str("{\"ui_zoom\":150}").unwrap();
    other.migrate();
    assert_eq!(other.ui_zoom, 150, "other stored sizes stay");
    assert_eq!(Settings::default().ui_zoom, 0, "new installs start on Auto");
    assert!(Settings::default().ui_zoom_auto_migrated, "and do not migrate their choice later");
}

#[test]
fn early_decode_is_off_unless_the_file_says_otherwise() {
    assert!(!Settings::default().early_decode);
    let old: Settings = serde_json::from_str("{\"fps\": 90}").unwrap();
    assert!(!old.early_decode, "a settings file from before early decode keeps it off");
    let on: Settings = serde_json::from_str("{\"early_decode\": true}").unwrap();
    assert!(on.early_decode);
}

#[test]
fn pyrowave_async_compute_is_off_unless_the_file_says_otherwise() {
    assert!(!Settings::default().pyrowave_async_compute);
    let old: Settings = serde_json::from_str("{}").unwrap();
    assert!(!old.pyrowave_async_compute, "a settings file from before the setting keeps it off");
    let on: Settings = serde_json::from_str("{\"pyrowave_async_compute\": true}").unwrap();
    assert!(on.pyrowave_async_compute);
    let saved = serde_json::to_value(&on).unwrap();
    assert_eq!(saved["pyrowave_async_compute"], serde_json::Value::Bool(true), "saved under its key");
    let back: Settings = serde_json::from_value(saved).unwrap();
    assert!(back.pyrowave_async_compute);
}

#[test]
fn beam_race_is_off_unless_the_file_says_otherwise() {
    assert!(!Settings::default().beam_race);
    let old: Settings = serde_json::from_str("{\"early_decode\": true}").unwrap();
    assert!(!old.beam_race, "a settings file from before the beam race keeps it off");
    let on: Settings = serde_json::from_str("{\"beam_race\": true}").unwrap();
    assert!(on.beam_race);
    assert_eq!(old.beam_race_margin_ms, 0, "the margin is automatic unless the file sets one");
    let margin: Settings = serde_json::from_str("{\"beam_race_margin_ms\": 2}").unwrap();
    assert_eq!(margin.beam_race_margin_ms, 2);
}

#[test]
fn pyrowave_decoder_uses_the_platform_default_and_reads_from_the_file() {
    let default = if cfg!(target_os = "macos") { PyroWaveDecoder::Cpu } else { PyroWaveDecoder::Gpu };
    assert_eq!(Settings::default().pyrowave_decoder, default);
    let old: Settings = serde_json::from_str("{\"fps\": 90}").unwrap();
    assert_eq!(old.pyrowave_decoder, default, "an older file gets the default");
    let cpu: Settings = serde_json::from_str("{\"pyrowave_decoder\": \"Cpu\"}").unwrap();
    assert_eq!(cpu.pyrowave_decoder, PyroWaveDecoder::Cpu, "a saved CPU choice stays");
    let gpu: Settings = serde_json::from_str("{\"pyrowave_decoder\": \"Gpu\"}").unwrap();
    assert_eq!(gpu.pyrowave_decoder, PyroWaveDecoder::Gpu);
    let mut overrides = Overrides::default();
    overrides.0.insert("pyrowave_decoder".into(), serde_json::json!("Gpu"));
    assert_eq!(overrides.apply(&Settings::default()).pyrowave_decoder, PyroWaveDecoder::Gpu);
}

#[test]
fn yuv444_and_sdr10_default_off_and_ask_for_the_gpu_renderer() {
    let old: Settings = serde_json::from_str("{\"fps\": 90}").unwrap();
    assert!(!old.prefer_yuv444 && !old.sdr_10bit, "an older file gets them off");
    assert!(!Settings::default().needs_gpu());
    let on: Settings = serde_json::from_str("{\"prefer_yuv444\": true}").unwrap();
    assert!(on.needs_gpu(), "Linux draws 4:4:4 pictures with the plane pass");
    let on: Settings = serde_json::from_str("{\"sdr_10bit\": true}").unwrap();
    assert!(on.needs_gpu(), "and software 10-bit pictures too");
    // A game or PC that turns one on starts Nightjar with the GPU renderer as well.
    let mut overrides = Overrides::default();
    assert!(!overrides.needs_gpu());
    overrides.0.insert("sdr_10bit".into(), serde_json::json!(true));
    assert!(overrides.needs_gpu());
}

#[test]
fn hdr_brightness_is_the_pcs_by_default_and_a_pc_can_follow_the_desktop() {
    assert!(!Settings::default().hdr_desktop_brightness, "absolute: what HDR10 means and direct scanout needs");
    let old: Settings = serde_json::from_str("{\"hdr\": true}").unwrap();
    assert!(!old.hdr_desktop_brightness, "an older file gets the default");
    let on: Settings = serde_json::from_str("{\"hdr_desktop_brightness\": true}").unwrap();
    assert!(on.hdr_desktop_brightness);
    let saved = serde_json::to_value(&on).unwrap();
    assert_eq!(saved["hdr_desktop_brightness"], serde_json::json!(true), "saved under its own key");
    let mut overrides = Overrides::default();
    overrides.0.insert("hdr_desktop_brightness".into(), serde_json::json!(true));
    assert!(overrides.apply(&Settings::default()).hdr_desktop_brightness, "a PC's own setting");
    assert!(!overrides.needs_gpu(), "the renderer HDR takes is enough");
}

#[test]
fn frame_warp_is_off_by_default_and_saved_under_its_own_key() {
    assert!(!Settings::default().frame_warp, "off until the player turns it on");
    let old: Settings = serde_json::from_str("{\"upscale_sharpness\": 70}").unwrap();
    assert!(!old.frame_warp, "an older file gets the default");
    let on: Settings = serde_json::from_str("{\"frame_warp\": true}").unwrap();
    assert!(on.frame_warp);
    let saved = serde_json::to_value(&on).unwrap();
    assert_eq!(saved["frame_warp"], serde_json::json!(true), "saved under its own key");
    let again: Settings = serde_json::from_value(saved).unwrap();
    assert_eq!(again, on, "it round-trips");
    let mut overrides = Overrides::default();
    overrides.0.insert("frame_warp".into(), serde_json::json!(true));
    assert!(overrides.apply(&Settings::default()).frame_warp, "a PC's or game's own setting");
    assert!(!overrides.needs_gpu(), "every renderer draws it");
}

#[test]
fn the_renderer_choice_defaults_to_auto_and_reads_from_the_file() {
    assert_eq!(Settings::default().renderer, Renderer::Auto);
    let old: Settings = serde_json::from_str("{\"fps\": 90}").unwrap();
    assert_eq!(old.renderer, Renderer::Auto, "an older file gets the default");
    for choice in Renderer::ALL {
        let saved = serde_json::to_string(&Settings { renderer: choice, ..Settings::default() }).unwrap();
        assert_eq!(serde_json::from_str::<Settings>(&saved).unwrap().renderer, choice);
    }
    let gpu: Settings = serde_json::from_str("{\"renderer\": \"Gpu\"}").unwrap();
    assert_eq!(gpu.renderer, Renderer::Gpu);
    let opengl: Settings = serde_json::from_str("{\"renderer\": \"OpenGl\"}").unwrap();
    assert_eq!(opengl.renderer, Renderer::OpenGl);
}

#[test]
fn the_gpu_need_names_hdr_only_when_nothing_else_needs_the_renderer() {
    assert_eq!(Settings::default().gpu_need(), None);
    let mut settings = Settings { hdr: true, ..Settings::default() };
    assert_eq!(settings.gpu_need(), Some("HDR"));
    settings.upscaling = Upscaling::Fsr1;
    assert_eq!(settings.gpu_need(), Some("upscaling"), "a stream that needs the renderer even in SDR");
    settings.codec = CodecPreference::PyroWave;
    assert_eq!(settings.gpu_need(), Some("PyroWave"));
}

#[test]
fn a_stored_temporal_upscaling_reads_as_sgsr1_everywhere() {
    // The Temporal (SGSR 2) choice is gone: the settings file and every PC, game and profile layer read it as SGSR 1.
    let old: Settings = serde_json::from_str(r#"{"upscaling": "Sgsr2"}"#).unwrap();
    assert_eq!(old.upscaling, Upscaling::Sgsr1);
    assert!(old.upscaling.is_shader(), "the sharpness row and the shader path");
    for kept in ["Bilinear", "Fsr1", "Sgsr1", "Ai", "MetalFx"] {
        let s: Settings = serde_json::from_str(&format!(r#"{{"upscaling": "{kept}"}}"#)).unwrap();
        assert_eq!(serde_json::to_string(&s.upscaling).unwrap(), format!("\"{kept}\""));
    }
    let layer: Overrides = serde_json::from_str(r#"{"upscaling": "Sgsr2"}"#).unwrap();
    assert_eq!(layer.apply(&Settings::default()).upscaling, Upscaling::Sgsr1);
    let saved = serde_json::to_string(&old).unwrap();
    assert!(saved.contains("\"upscaling\":\"Sgsr1\""), "{saved}");
}

#[test]
fn files_round_trip() {
    let dir = std::env::temp_dir().join(format!("nightjar-config-{}", std::process::id()));
    let store = Store::at(dir.clone()).unwrap();
    assert_eq!(store.settings(), Settings::default());
    let settings = Settings {
        fps: 120,
        codec: CodecPreference::Av1,
        ..Default::default()
    };
    store.save_settings(&settings).unwrap();
    assert_eq!(store.settings(), settings);
    let mut profiles = Profiles::default();
    let id = profiles.create("Travel", serde_json::from_str(r#"{"fps":60,"hdr":false}"#).unwrap()).unwrap();
    profiles.active = Some(id);
    store.save_profiles(&profiles).unwrap();
    assert_eq!(store.profiles(), profiles);
    let mut shortcuts = crate::shortcuts::Shortcuts::default();
    crate::shortcuts::save(&mut shortcuts, None, "Task manager", "CTRL + SHIFT + ESC").unwrap();
    store.save_shortcuts(&shortcuts).unwrap();
    assert_eq!(store.shortcuts(), shortcuts);
    // Stream hotkeys hold across a restart.
    assert!(store.hotkeys().is_empty());
    let hotkeys = BTreeMap::from([("quit".to_string(), "CTRL+F9".to_string()), ("overlay".into(), "off".into())]);
    store.save_hotkeys(&hotkeys).unwrap();
    assert_eq!(store.hotkeys(), hotkeys);
    let mapping = crate::gamepad_types::Mapping { name: "Test pad".into(), deadzone: 12, ..Default::default() };
    let mut mappings = BTreeMap::from([("usb:1234:0001".into(), mapping)]);
    store.save_controller_mappings(&mappings).unwrap();
    assert_eq!(store.controller_mappings(), mappings);
    mappings.get_mut("usb:1234:0001").unwrap().axes[0] = -128;
    store.save_controller_mappings(&mappings).unwrap();
    assert!(store.controller_mappings().is_empty());
    let mut hosts = vec![HostRecord {
        unique_id: "ABC".into(),
        name: "Desk".into(),
        address: "10.0.0.2".into(),
        http_port: 47989,
        server_certificate: Some("PEM".into()),
        ..Default::default()
    }];
    hosts[0].save_app_settings("game-a", 1, serde_json::from_str(r#"{"mouse_mode":"Absolute"}"#).unwrap());
    store.save_hosts(&hosts).unwrap();
    assert_eq!(store.hosts(), hosts);
    // A settings file from an older version keeps the new fields' defaults.
    std::fs::write(dir.join("settings.json"), r#"{"fps": 90}"#).unwrap();
    assert_eq!(store.settings().fps, 90);
    assert!(store.settings().present_at_host_times, "present_at_host_times (Variable refresh rate On) is switched on once");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn host_settings_inherit_until_overridden() {
    let mut host: HostRecord = serde_json::from_str(r#"{"address":"pc.local"}"#).unwrap();
    let mut global = Settings { fps: 120, ..Default::default() };
    assert_eq!(host.settings(&global).fps, 120);
    host.settings = Some(serde_json::from_str(r#"{"fps":60,"microphone":true}"#).unwrap());
    global.fps = 144;
    let saved: HostRecord = serde_json::from_str(&serde_json::to_string(&host).unwrap()).unwrap();
    assert_eq!(saved.settings(&global).fps, 60);
    assert!(saved.settings(&global).microphone);
    host.settings = None;
    assert_eq!(host.settings(&global).fps, 144);
}

#[test]
fn a_pcs_own_settings_count_each_game_once_and_clear_to_the_layers_below() {
    // As 4.1.0 wrote it: Frame rate pinned for the PC, one game with a UUID (saved under both
    // keys) and one without.
    let mut host: HostRecord = serde_json::from_value(serde_json::json!({
        "address": "pc.local",
        "settings": { "fps": 120 },
        "app_settings": { "uuid:a": { "hdr": true }, "id:1": { "hdr": true }, "id:2": { "fps": 30 } },
    })).unwrap();
    assert_eq!(host.own_settings(), (true, 2));
    host.clear_own_settings();
    assert_eq!(host.own_settings(), (false, 0));
    let main = Settings { fps: 60, ..Default::default() };
    assert_eq!(host.settings(&main).fps, 60);
    assert!(host.app_settings("a", 1).is_none());

    let untouched: HostRecord = serde_json::from_str(r#"{"address":"pc.local","settings":{}}"#).unwrap();
    assert_eq!(untouched.own_settings(), (false, 0));
    // Settings saved before the look at kept settings have it still to run; after, it stays done.
    let old: Settings = serde_json::from_str(r#"{"fps":60}"#).unwrap();
    assert!(!old.kept_settings_checked);
    let done = Settings { kept_settings_checked: true, ..old };
    assert!(serde_json::from_str::<Settings>(&serde_json::to_string(&done).unwrap()).unwrap().kept_settings_checked);
}

#[test]
fn a_setting_names_the_layer_it_comes_from() {
    let profile = Profile { id: 1, name: "Travel".into(), settings: serde_json::from_str(r#"{"bitrate_kbps":5000}"#).unwrap() };
    let pc: Overrides = serde_json::from_str(r#"{"fps":120,"bitrate_kbps":9000}"#).unwrap();
    let game: Overrides = serde_json::from_str(r#"{"hdr":true}"#).unwrap();
    assert_eq!(source(&["fps"], Some(&profile), Some(&pc), Some(&game)), Source::Pc);
    assert_eq!(source(&["bitrate_kbps"], Some(&profile), None, Some(&game)), Source::Profile("Travel".into()));
    assert_eq!(source(&["hdr"], Some(&profile), Some(&pc), Some(&game)), Source::Game);
    assert_eq!(source(&["width", "height"], Some(&profile), Some(&pc), Some(&game)), Source::Main);
    assert_eq!(source(&["fps"], None, None, None), Source::Main);
}

#[test]
fn a_pc_alias_round_trips_and_an_old_file_has_none() {
    let old: HostRecord = serde_json::from_str(r#"{"address":"pc.local","name":"DESKTOP-1"}"#).unwrap();
    assert_eq!(old.alias, None);
    let mut host = old;
    host.set_alias("  Living room  ");
    assert_eq!(host.alias.as_deref(), Some("Living room"));
    let saved: HostRecord = serde_json::from_str(&serde_json::to_string(&host).unwrap()).unwrap();
    assert_eq!(saved.alias.as_deref(), Some("Living room"));
    assert_eq!(saved.name, "DESKTOP-1", "the PC's own name is untouched");
    host.set_alias(&"x".repeat(60));
    assert_eq!(host.alias.as_ref().map(|a| a.chars().count()), Some(ALIAS_MAX_CHARS));
    host.set_alias("   ");
    assert_eq!(host.alias, None, "an empty name restores the PC's own");
}

#[test]
fn partial_profiles_and_game_overrides_follow_their_layers() {
    let mut global = Settings { fps: 120, microphone: true, ..Default::default() };
    let mut profiles = Profiles::default();
    let id = profiles.create(" Home ", serde_json::from_str(r#"{"codec":"Av1","fps":90}"#).unwrap()).unwrap();
    profiles.active = Some(id);
    assert!(profiles.create("HOME", Overrides::default()).is_err());
    let mut host = HostRecord { settings: Some(serde_json::from_str(r#"{"fps":60}"#).unwrap()), ..Default::default() };
    host.save_app_settings("game-a", 1, serde_json::from_str(r#"{"hdr":true,"microphone":false}"#).unwrap());
    global.upscale_sharpness = 80;
    let base = host.settings(&profiles.settings(&global, crate::network_types::Kind::Other));
    let game = host.app_settings("game-a", 1).unwrap().apply(&base);
    assert_eq!(game.fps, 60);
    assert_eq!(game.codec, CodecPreference::Av1);
    assert_eq!(game.upscale_sharpness, 80);
    assert!(game.hdr);
    assert!(!game.microphone);
    assert!(host.app_settings("game-b", 1).is_none());
    assert!(host.app_settings("game-a", 7).is_some());
    assert!(host.app_settings("", 1).is_some());
    host.save_app_settings("game-a", 1, Overrides::default());
    assert!(host.app_settings.is_empty());
    profiles.delete(id);
    assert_eq!(profiles.active, None);
    assert_eq!(profiles.settings(&global, crate::network_types::Kind::Other), global);
}

#[test]
fn network_choices_fall_back_and_deleted_rules_do_not_select_reused_ids() {
    use crate::network_types::{Kind, Network};
    let mut profiles = Profiles::default();
    let active = profiles.create("Default", serde_json::from_str(r#"{"fps":60}"#).unwrap()).unwrap();
    let travel = profiles.create("Travel", serde_json::from_str(r#"{"fps":30}"#).unwrap()).unwrap();
    profiles.active = Some(active);
    profiles.mobile = Some(travel);
    profiles.wifi = Some(travel);
    profiles.home = Some(active);
    profiles.home_wifi.push("Home".into());
    profiles.mobile_limit_kbps = 5000;
    let mut network = Network { wifi: true, ssid: Some("Home".into()), ..Default::default() };
    assert_eq!(network.kind(&profiles.home_wifi), Kind::HomeWifi);
    assert_eq!(profiles.selected(Kind::Mobile), Some(active));
    profiles.automatic = true;
    assert_eq!(profiles.settings(&Settings::default(), Kind::Mobile).fps, 30);
    assert_eq!(profiles.selected(Kind::Other), Some(active));
    network.metered = true;
    assert_eq!(network.kind(&profiles.home_wifi), Kind::Mobile);
    assert_eq!(profiles.bitrate_limit(Kind::Mobile), 5000);
    assert_eq!(profiles.bitrate_limit(Kind::HomeWifi), 0);
    network.metered = false;
    network.ssid = None;
    assert_eq!(network.kind(&profiles.home_wifi), Kind::Wifi);
    profiles.delete(travel);
    assert_eq!((profiles.mobile, profiles.wifi), (None, None));
    assert_eq!(profiles.selected(Kind::Mobile), Some(active));
    profiles.mobile = Some(999);
    assert_eq!(profiles.settings(&Settings::default(), Kind::Mobile), Settings::default());
    let restored: Profiles = serde_json::from_str(&serde_json::to_string(&profiles).unwrap()).unwrap();
    assert_eq!(restored, profiles);
}

#[test]
fn telling_the_host_about_wireless_is_on_by_default_and_in_old_files() {
    assert!(Settings::default().tell_host_wireless);
    // A settings file from before the field existed keeps it on.
    let old: Settings = serde_json::from_str(r#"{"fps":60,"auto_reconnect":true}"#).unwrap();
    assert!(old.tell_host_wireless && old.auto_reconnect);
    let off: Settings = serde_json::from_str(r#"{"tell_host_wireless":false}"#).unwrap();
    assert!(!off.tell_host_wireless);
}

#[test]
fn client_roaming_is_on_and_mobile_data_off_by_default_and_in_old_files() {
    let defaults = Settings::default();
    assert!(defaults.client_roaming && !defaults.roaming_metered);
    let old: Settings = serde_json::from_str(r#"{"fps":60,"tell_host_wireless":false}"#).unwrap();
    assert!(old.client_roaming && !old.roaming_metered);
    let set: Settings = serde_json::from_str(r#"{"client_roaming":false,"roaming_metered":true}"#).unwrap();
    assert!(!set.client_roaming && set.roaming_metered);
}

#[test]
fn play_over_the_internet_is_on_by_default_and_in_old_files() {
    assert!(Settings::default().internet_play);
    let old: Settings = serde_json::from_str(r#"{"fps":60}"#).unwrap();
    assert!(old.internet_play);
    let off: Settings = serde_json::from_str(r#"{"internet_play":false}"#).unwrap();
    assert!(!off.internet_play);
    // The host's reply is kept per PC and survives a save.
    let record = HostRecord { internet_play: Some("{\"version\":1}".into()), ..Default::default() };
    let saved: HostRecord = serde_json::from_str(&serde_json::to_string(&record).unwrap()).unwrap();
    assert_eq!(saved.internet_play.as_deref(), Some("{\"version\":1}"));
    let old: HostRecord = serde_json::from_str(r#"{"address":"192.0.2.1"}"#).unwrap();
    assert!(old.internet_play.is_none());
}

#[test]
fn frames_waiting_for_the_decoder_are_fifteen_by_default_and_in_old_files() {
    assert_eq!(Settings::default().decode_queue_frames, 15);
    let old: Settings = serde_json::from_str(r#"{"fps":60,"sops":false}"#).unwrap();
    assert_eq!(old.decode_queue_frames, 15);
    let set: Settings = serde_json::from_str(r#"{"decode_queue_frames":5}"#).unwrap();
    assert_eq!(set.decode_queue_frames, 5);
}
