//! Settings tests for screenshots, network tests, jumbo packets, keyboard/system keys, and row layout.

use super::*;

/// The settings page in every state worth a look, at 1280x800 (150 %) and 1920x1080: every
/// section with its main rows (`settings-section-N`) and with its fold open (`-more`, `-more-end`),
/// and Variable refresh rate where the display offers it (`settings-vrr`, `-list`, `-custom`).
/// `NIGHTJAR_SHOTS=<dir> cargo test -p nightjar-desktop settings_screenshots -- --ignored`.
#[test]
#[ignore = "writes PNGs; set NIGHTJAR_SHOTS"]
fn settings_screenshots() {
    let Some(dir) = std::env::var_os("NIGHTJAR_SHOTS").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&dir).expect("the folder for the screenshots");
    for (width, height, ppp) in [(1280.0, 800.0, 1.5), (1920.0, 1080.0, 1.0)] {
        for high in [false, true] {
            let size = format!("{}x{}{}", width as u32, height as u32, if high { "-high-contrast" } else { "" });
            let fresh = || {
                let mut f = home_with_games(1);
                f.size = vec2(width, height);
                f.native_ppp = ppp;
                f.shots = true;
                // The logo went up in frames before shots were kept; load it again so they see it.
                f.l.logo = None;
                f.l.settings.high_contrast = high;
                f.l.settings.fps = 60;
                f.l.settings.bitrate_kbps = 40_000;
                f
            };
            let shoot = |f: &mut Frames, page: &str| {
                f.settle();
                f.settle();
                f.shoot(&dir, &format!("{page}-{size}"));
            };
            let mut f = fresh();
            f.l.go(Screen::Settings);
            shoot(&mut f, "settings");
            // More settings chosen: Profiles and the sections of detail under the simple ones.
            f.l.settings_view.rail_more = true;
            shoot(&mut f, "settings-more");
            f.l.settings_view.section = "Video";
            f.l.settings_view.to_top = true;
            f.settle();
            // A row's hint, under the pointer.
            let rate = f.placed.iter().find(|(t, ..)| t == "Frame rate").expect("the row").1.center();
            f.frame(vec![egui::Event::PointerMoved(rate)]);
            shoot(&mut f, "settings-hint");
            f.frame(vec![egui::Event::PointerGone]);
            f.l.settings_view.section = "Controls";
            f.l.settings_view.to_top = true;
            shoot(&mut f, "settings-controls");
            if high {
                continue;
            }
            // Every section: its main rows, then its fold open, top and end.
            for (at, section) in SETTINGS_SECTIONS.iter().enumerate() {
                f.l.settings_view.section = section;
                f.l.settings_view.more = [false; 9];
                f.l.settings_view.to_top = true;
                shoot(&mut f, &format!("settings-section-{at}"));
                f.l.settings_view.more[at] = true;
                f.l.settings_view.to_top = true;
                shoot(&mut f, &format!("settings-section-{at}-more"));
                f.scroll_to_end();
                shoot(&mut f, &format!("settings-section-{at}-more-end"));
            }
            f.l.settings_view.more = [true; 9];
            // Dual screen mode on: Second screen resolution can be changed.
            f.l.settings.dual_screen = true;
            f.l.settings_view.section = "Streaming";
            f.l.settings_view.to_top = true;
            f.settle();
            f.scroll_to_end();
            shoot(&mut f, "settings-dual-screen");
            f.l.settings.dual_screen = false;
            f.l.settings_view.query = "bitrate".into();
            shoot(&mut f, "settings-search");
            f.l.settings_view.query = "teleport".into();
            shoot(&mut f, "settings-search-nothing");
            f.l.settings_view.query.clear();
            f.l.settings_view.section = "App";
            f.l.settings_view.confirm_reset = true;
            f.l.settings_view.to_top = true;
            f.settle();
            f.scroll_to_end();
            shoot(&mut f, "settings-reset-all");
            f.l.settings_view.confirm_reset = false;
            f.l.settings_view.section = "Video";
            // Variable refresh rate on a display that offers it: the row, its list, and a mix of
            // its keys the list does not offer.
            let mut f = fresh();
            f.l.tearing_supported = true;
            f.l.go(Screen::Settings);
            f.l.settings_view.section = "Display & image";
            f.l.settings_view.to_top = true;
            shoot(&mut f, "settings-vrr");
            click_value(&mut f, "Variable refresh rate", "On");
            shoot(&mut f, "settings-vrr-list");
            click_last(&mut f, "Fastest");
            f.l.settings.vrr_tear_free = true;
            shoot(&mut f, "settings-vrr-custom");
            // Presets at the defaults (Native marked), and the question a chip asks.
            let mut f = fresh();
            (f.l.settings.fps, f.l.settings.bitrate_kbps) = (0, 0);
            f.l.go(Screen::Settings);
            shoot(&mut f, "settings-presets-native");
            f.l.ask(Dialog::ConfirmPreset(Preset::BatterySaver));
            shoot(&mut f, "dialog-preset");
            f.l.dialog = Dialog::None;
            // The microphone's rows, and Frame pacing's PC's frame times (V-Sync's row gives way).
            f.l.settings.microphone = true;
            f.l.settings.host_frame_times = true;
            f.l.settings_view.more = [true; 9];
            f.l.settings_view.section = "Audio";
            f.l.settings_view.to_top = true;
            shoot(&mut f, "settings-microphone");
            f.l.settings_view.section = "Display & image";
            f.l.settings_view.to_top = true;
            f.settle();
            f.scroll_to_end();
            shoot(&mut f, "settings-frame-pacing-host");
            // Frame pacing's list on a screen without VRR, the PC's frame times among its choices.
            let mut f = fresh();
            f.l.go(Screen::Settings);
            f.l.settings_view.section = "Display & image";
            f.l.settings_view.to_top = true;
            f.settle();
            click_value(&mut f, "Frame pacing", "Lowest latency");
            shoot(&mut f, "settings-frame-pacing-list");
            // About: the version, the source link and the credits, top and end.
            let mut f = fresh();
            f.l.go(Screen::Settings);
            f.l.open(Screen::About);
            shoot(&mut f, "about");
            f.scroll_to_end();
            shoot(&mut f, "about-end");
            // The tests in the main settings: no PC that runs them, one, and two with the PC's list
            // and a saved pace.
            for count in 0..3 {
                let mut f = fresh();
                f.hosts = network_test_pcs(&[("Desk", true, Reach::Online), ("Den", true, Reach::Offline)][..count]);
                if count == 2 {
                    f.hosts.list[0].record.pace = Some(crate::config::PaceCalibration { pace_mbps: 180, client_link_mbps: 1000, host_link_mbps: 2500, wifi: false, ssid: None, qualified: true });
                }
                f.l.go(Screen::Settings);
                f.l.settings_view.more = [true; 9];
                f.l.settings_view.section = "Streaming";
                f.l.settings_view.to_top = true;
                shoot(&mut f, &format!("settings-connection-test-{count}"));
                f.l.settings_view.section = "Advanced";
                f.l.settings_view.to_top = true;
                shoot(&mut f, &format!("settings-frame-pace-{count}"));
            }
            // Bitrate on mobile data, with a cap set on the Profiles page.
            let mut f = fresh();
            f.l.profiles.mobile_limit_kbps = 10_000;
            f.l.go(Screen::Settings);
            shoot(&mut f, "settings-mobile-bitrate");
            // A PC's settings with two of its own values, over a profile.
            let mut f = fresh();
            let pc = f.hosts.list[0].id;
            let travel = f.l.profiles.create("Travel", serde_json::from_str(r#"{"bitrate_kbps":20000}"#).unwrap()).unwrap();
            f.l.profiles.active = Some(travel);
            f.hosts.list[0].record.settings = Some(serde_json::from_str(r#"{"fps":120,"hdr":true}"#).unwrap());
            f.l.open_host_settings(pc, &f.hosts.list[0]);
            shoot(&mut f, "pc-settings");
            f.l.settings_view.section = "Debug";
            f.l.settings_view.to_top = true;
            shoot(&mut f, "pc-settings-debug");
            f.l.settings_view.section = "Video";
            // A game's settings: one of its own, the rest from the PC, the profile and the main settings.
            let app = nightjar_policy::http::App { name: "Racing Game".into(), uuid: "install-0".into(), ..game(1000) };
            f.l.open_app_settings(pc, &f.hosts.list[0], app);
            f.l.edit_overrides.capture(&Settings { codec: CodecPreference::Hevc, ..Settings::default() }, &["codec"]);
            f.l.edit_settings = f.l.edit_overrides.apply(&f.l.inherited_settings);
            shoot(&mut f, "game-settings");
            // The main settings while a profile is in use.
            f.l.go(Screen::Settings);
            shoot(&mut f, "settings-profile-in-use");
        }
    }
}

/// Jumbo video packets appear only when a saved PC offers more than stock 1400-byte packets;
/// the row's own switch is the `jumbo_frames` setting, off by default.
#[test]
fn jumbo_video_packets_are_offered_only_by_a_pc_that_offers_them() {
    use egui::accesskit::Role;
    let mut plain = settings_read_with(Settings::default());
    in_section(&mut plain, "Streaming");
    assert!(!shows(&plain, "Jumbo video packets"), "{:?}", plain.drawn);

    let mut f = settings_with_pcs(&[("Desk", true, Reach::Online)]);
    in_section(&mut f, "Streaming");
    assert!(shows(&f, "Jumbo video packets"), "{:?}", f.drawn);
    assert!(!f.l.settings.jumbo_frames, "off by default");
    let bounds = f.access.as_ref().expect("AccessKit is on").nodes.iter()
        .find(|(_, n)| n.role() == Role::Switch && n.label() == Some("Jumbo video packets"))
        .map(|(_, n)| n.bounds().expect("laid out"))
        .expect("the row's switch");
    let at = Pos2::new(((bounds.x0 + bounds.x1) / 2.0) as f32, ((bounds.y0 + bounds.y1) / 2.0) as f32);
    f.click_at(at, egui::PointerButton::Primary);
    assert!(f.l.settings.jumbo_frames, "the row turns the setting on");

    // A PC's own page edits no global network rows; the setting is in the main settings.
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    in_section(&mut f, "Streaming");
    assert!(!shows(&f, "Jumbo video packets"), "{:?}", f.drawn);
}

/// A customer looked for the PyroWave calibration in Settings: Connection test is one of
/// Streaming's main rows, the frame pace is under Debug › Diagnostics, as on a PC's own
/// settings, and the search finds both.
#[test]
fn the_connection_test_and_frame_pace_are_in_the_main_settings_and_a_pcs_own() {
    let mut f = settings_with_pcs(&[("Desk", true, Reach::Online)]);
    let id = f.hosts.list[0].id;
    for page in [Screen::Settings, Screen::HostSettings(id)] {
        if page != Screen::Settings {
            f.l.open_host_settings(id, &f.hosts.list[0]);
        }
        in_section(&mut f, "Streaming");
        assert!(f.l.settings_view.drawn.contains(&("connection_test", false)), "{page:?}: a main row: {:?}", f.l.settings_view.drawn);
        // One PC is the PC: nothing to choose. An action: no box of a PC's own, no Reset.
        assert_eq!(row_words(&f, "Connection test"), ["Test"], "{page:?}");
        let commands = click_text(&mut f, "Test");
        assert!(matches!(commands[..], [Command::NetworkTest { host, calibrate: false }] if host == id), "{page:?}: {} commands", commands.len());
        in_section(&mut f, "Debug");
        assert!(shows(&f, "Diagnostics"), "{page:?}: {:?}", f.drawn);
        assert_eq!(row_words(&f, "PyroWave frame pace"), ["Calibrate"], "{page:?}: nothing to clear before a calibration");
        let commands = click_text(&mut f, "Calibrate");
        assert!(matches!(commands[..], [Command::NetworkTest { host, calibrate: true }] if host == id), "{page:?}: {} commands", commands.len());
    }
    // A saved pace says so and can be cleared, from the main settings too.
    f.hosts.list[0].record.pace = Some(crate::config::PaceCalibration { pace_mbps: 180, client_link_mbps: 1000, host_link_mbps: 2500, wifi: false, ssid: None, qualified: true });
    f.l.go(Screen::Settings);
    in_section(&mut f, "Debug");
    assert!(shows(&f, "180 Mbps · confirmed"), "{:?}", f.drawn);
    let commands = click_text(&mut f, "Clear");
    assert!(matches!(commands[..], [Command::ClearPace(host)] if host == id), "{} commands", commands.len());

    let at = |name: &str| SETTINGS_SECTIONS.iter().position(|s| *s == name).unwrap();
    for (query, row, section) in [("connection test", "Connection test", "Streaming"), ("download speed", "Connection test", "Streaming"),
        ("pyrowave frame pace", "PyroWave frame pace", "Debug"), ("diagnostics", "PyroWave frame pace", "Debug")] {
        f.l.settings_view.query = query.into();
        f.settle();
        assert!(shows(&f, row) && f.l.settings_view.found[at(section)] > 0, "{query}: {:?} {:?}", f.l.settings_view.found, f.drawn);
    }
    f.l.settings_view.query = "pyrowave frame pace".into();
    f.settle();
    assert!(!shows(&f, "Connection test"), "{:?}", f.drawn);
}

/// With several paired PCs a list beside each test chooses the one tested: the first that is
/// online until the user picks another, never one that is not paired. One off the network says so.
#[test]
fn the_main_settings_tests_choose_among_several_paired_pcs() {
    let mut f = settings_with_pcs(&[("Loft", false, Reach::Online), ("Desk", true, Reach::Offline), ("Den", true, Reach::Online)]);
    let (loft, desk, den) = (f.hosts.list[0].id, f.hosts.list[1].id, f.hosts.list[2].id);
    in_section(&mut f, "Streaming");
    let words = row_words(&f, "Connection test");
    assert!(words.contains(&"Test") && words.contains(&"Den"), "the paired PC that is online: {words:?}");
    assert_eq!(f.l.settings_view.test_pc, None, "nothing is chosen until the user does");
    let commands = click_text(&mut f, "Test");
    assert!(matches!(commands[..], [Command::NetworkTest { host, calibrate: false }] if host == den), "{} commands", commands.len());

    // The list has the paired PCs only; choosing one moves both tests to it.
    click_text(&mut f, "Den");
    assert!(shows(&f, "Desk") && !shows(&f, "Loft"), "{:?}", f.drawn);
    click_last(&mut f, "Desk");
    assert_eq!(f.l.settings_view.test_pc, Some(desk));
    // It is off the network: both tests wait for it.
    let words = row_words(&f, "Connection test");
    assert!(words.contains(&"Desk") && words.contains(&"Offline") && !words.contains(&"Test"), "{words:?}");
    in_section(&mut f, "Debug");
    let words = row_words(&f, "PyroWave frame pace");
    assert!(words.contains(&"Desk") && words.contains(&"Offline") && !words.contains(&"Calibrate"), "{words:?}");
    f.hosts.list[1].reach = Reach::Online;
    f.settle();
    let commands = click_text(&mut f, "Calibrate");
    assert!(matches!(commands[..], [Command::NetworkTest { host, calibrate: true }] if host == desk), "{} commands", commands.len());

    // The choice is the launcher's, not a setting, and holds only while that PC is listed.
    assert_eq!(f.l.settings, Settings::default());
    let pc = |f: &Frames, chosen| network_test_pc(&test_pcs(Screen::Settings, &f.hosts, Host::bandwidth_test), chosen).map(|h| h.id);
    assert_eq!(pc(&f, Some(den)), Some(den));
    assert_eq!(pc(&f, Some(loft)), Some(desk), "not paired: the first paired PC that is online");
    assert_eq!(pc(&f, Some(u64::MAX)), Some(desk), "forgotten");
    f.hosts.list[1].reach = Reach::Offline;
    f.hosts.list[2].reach = Reach::Unknown;
    assert_eq!(pc(&f, None), Some(desk), "none online: the first paired one");

    // One paired PC beside one that is not: nothing to choose.
    let mut f = settings_with_pcs(&[("Loft", false, Reach::Online), ("Desk", true, Reach::Online)]);
    in_section(&mut f, "Streaming");
    assert_eq!(row_words(&f, "Connection test"), ["Test"], "{:?}", f.drawn);
}

/// The tests are drawn only with a paired PC that runs them: none without one; the rows around
/// them stay. Connection test measures ping, jitter and loss on any paired PC, so a PC whose host
/// has no speed test has its row (and the delay alone), as on Android.
#[test]
fn the_tests_have_no_row_without_a_pc_that_runs_them() {
    for pcs in [&[][..], &[("Loft", false, Reach::Online)][..]] {
        let mut f = settings_with_pcs(pcs);
        in_section(&mut f, "Streaming");
        assert!(!shows(&f, "Connection test") && shows(&f, "Recover interrupted streams") && shows(&f, "Keep streaming when the network changes"), "{:?}", f.drawn);
        in_section(&mut f, "Debug");
        assert!(!shows(&f, "PyroWave frame pace") && shows(&f, "Test the link before starting"), "{:?}", f.drawn);
        assert!(test_pcs(Screen::Settings, &f.hosts, Host::bandwidth_test).is_empty());
    }
    // A paired PC that never said it has a speed test (stock hosts): the delay test is there.
    let mut f = settings_with(Settings::default());
    open_folds(&mut f);
    in_section(&mut f, "Streaming");
    assert_eq!(row_words(&f, "Connection test"), ["Test"], "{:?}", f.drawn);
    let id = f.hosts.list[0].id;
    let commands = click_text(&mut f, "Test");
    assert!(matches!(commands[..], [Command::NetworkTest { host, calibrate: false }] if host == id), "{} commands", commands.len());
    in_section(&mut f, "Debug");
    assert!(!shows(&f, "PyroWave frame pace"), "no calibration without PyroWave: {:?}", f.drawn);
}

/// Nightjar #87: system keys can go to the PC only while the stream is full screen. One row with
/// Send system keys' choices, saved in its two switches as before.
#[test]
fn system_keys_only_in_full_screen_is_a_choice_of_send_system_keys() {
    let defaults = Settings::default();
    assert!(defaults.capture_system_keys && !defaults.system_keys_fullscreen_only, "as before: in a window too");
    let old: Settings = serde_json::from_str(r#"{"capture_system_keys":false}"#).unwrap();
    assert!(!old.capture_system_keys && !old.system_keys_fullscreen_only, "a settings file from before");
    let saved = serde_json::to_value(Settings { system_keys_fullscreen_only: true, ..Settings::default() }).unwrap();
    assert_eq!((saved["capture_system_keys"].as_bool(), saved["system_keys_fullscreen_only"].as_bool()), (Some(true), Some(true)));
    let mut f = settings_with(Settings::default());
    f.l.settings_view.section = "Controls";
    open_folds(&mut f);
    assert!(shows(&f, "Send system keys") && !shows(&f, "System keys only in full screen"), "{:?}", f.drawn);
    let rows = setting_keys(&f);
    let at = rows.iter().position(|key| *key == "capture_system_keys").unwrap();
    assert_eq!(rows[at + 1], "right_alt_as_meta", "one row");
}

/// "Keyboard follows text fields", as on Android: on by default, also for a settings file from
/// before it, a main row of Controls beside Clipboard sync, and a PC's own page has it too.
#[test]
fn keyboard_follows_text_fields_is_a_controls_setting_on_by_default() {
    assert!(Settings::default().keyboard_follows_text_fields);
    let old: Settings = serde_json::from_str("{}").unwrap();
    assert!(old.keyboard_follows_text_fields, "a settings file from before");
    let off: Settings = serde_json::from_str(r#"{"keyboard_follows_text_fields":false}"#).unwrap();
    assert!(!off.keyboard_follows_text_fields);
    let mut f = settings_with(Settings::default());
    let id = f.hosts.list[0].id;
    for screen in [Screen::Settings, Screen::HostSettings(id)] {
        f.l.go(screen);
        f.l.settings_view.section = "Controls";
        f.settle();
        let rows = setting_keys(&f);
        let at = rows.iter().position(|key| *key == "keyboard_follows_text_fields").unwrap_or_else(|| panic!("{screen:?}: {rows:?}"));
        assert_eq!(rows[at + 1], "clipboard_sync", "{screen:?}: a main row, as on Android");
        assert!(shows(&f, "Keyboard follows text fields"), "{:?}", f.drawn);
    }
    assert!(ANDROID_KEYS.contains(&("keyboard_follows_text_fields", "checkbox_keyboard_follows_text_fields")));
}

/// A row that does nothing with another setting's value, or on this computer, says why under its
/// label, as Android's tags do; it says nothing where it works.
#[test]
fn rows_that_do_nothing_here_say_why() {
    use crate::config::MouseMode;
    let mut f = settings_with(Settings { mouse_mode: MouseMode::Absolute, ..Settings::default() });
    open_folds(&mut f);
    in_section(&mut f, "Controls");
    assert!(shows(&f, "Not used with Desktop pointer"), "Mouse speed: {:?}", f.drawn);
    assert!(shows(&f, "Not used: this computer has no on-screen keyboard"), "{:?}", f.drawn);
    f.l.screen_keyboard = true;
    f.l.settings.mouse_mode = MouseMode::Capture;
    f.settle();
    assert!(!shows(&f, "Not used: this computer has no on-screen keyboard") && !shows(&f, "Not used with"), "{:?}", f.drawn);
    // Local cursor draws in Desktop pointer mode only.
    in_section(&mut f, "Debug");
    assert!(shows(&f, "Not used with Captured"), "Local cursor: {:?}", f.drawn);
    f.l.settings.mouse_mode = MouseMode::Absolute;
    f.settle();
    assert!(!shows(&f, "Not used with Captured"), "{:?}", f.drawn);
    if cfg!(target_os = "linux") {
        f.l.settings.decoder = DecoderPreference::Software;
        f.settle();
        assert!(shows(&f, "Not used with Software"), "Vulkan video rows: {:?}", f.drawn);
    }
}

/// The rows that were two switches each are one choice, and a value saved by an earlier version
/// reads as the stream used it: "Back opens the stream menu" won over "Back button as Guide",
/// full screen only and mobile data counted only with their first switch on, and frame loss took
/// the place of the busy network wherever it ran.
#[test]
fn paired_switches_read_as_one_choice() {
    let display = crate::stream_types::Display { width: 1920, height: 1080, refresh_hz: 60.0, refresh_reported: true };
    let value = |json: &str, key: &str| {
        let s: Settings = serde_json::from_str(json).unwrap();
        value_text(key, &s, &display, crate::glyphs::Style::Xbox)
    };
    assert_eq!(value("{}", "back_opens_menu"), "Back");
    assert_eq!(value(r#"{"back_as_guide":true}"#, "back_opens_menu"), "Guide");
    assert_eq!(value(r#"{"back_opens_menu":true,"back_as_guide":true}"#, "back_as_guide"), "Stream menu");
    assert_eq!(value("{}", "capture_system_keys"), "Always");
    assert_eq!(value(r#"{"system_keys_fullscreen_only":true}"#, "capture_system_keys"), "In full screen");
    assert_eq!(value(r#"{"capture_system_keys":false,"system_keys_fullscreen_only":true}"#, "capture_system_keys"), "Off");
    assert_eq!(value("{}", "client_roaming"), "Except mobile data");
    assert_eq!(value(r#"{"roaming_metered":true}"#, "roaming_metered"), "Any network");
    assert_eq!(value(r#"{"client_roaming":false,"roaming_metered":true}"#, "client_roaming"), "Off");
    assert_eq!(value("{}", "lower_bitrate"), "The PC decides");
    assert_eq!(value(r#"{"lower_bitrate":"sooner"}"#, "lower_bitrate"), "Lower it sooner");
    assert_eq!(value(r#"{"lower_bitrate":"keep"}"#, "lower_bitrate"), "Never: keep my bitrate");
    // A PC's own value carries the key.
    let mut own_rate = Overrides::default();
    own_rate.capture(&Settings { lower_bitrate: LowerBitrate::Keep, ..Settings::default() }, &["lower_bitrate"]);
    assert_eq!(own_rate.apply(&Settings::default()).lower_bitrate, LowerBitrate::Keep, "Never for this PC over the main setting");
    let json = serde_json::to_string(&own_rate).unwrap();
    assert_eq!(serde_json::from_str::<Overrides>(&json).unwrap().apply(&Settings::default()).lower_bitrate, LowerBitrate::Keep);

    // A PC's own value keeps both switches of its choice, so the layer reads whole.
    let mut own = Overrides::default();
    let chosen = Settings { back_opens_menu: false, back_as_guide: true, ..Settings::default() };
    own.capture(&chosen, &["back_opens_menu", "back_as_guide"]);
    let main = Settings { back_opens_menu: true, ..Settings::default() };
    let applied = own.apply(&main);
    assert!(!applied.back_opens_menu && applied.back_as_guide, "Guide for this PC over the main Stream menu");
    // The old rows are gone from every section.
    for gone in ["back_as_guide", "system_keys_fullscreen_only", "roaming_metered", "adaptive_bitrate", "client_hint_bitrate", "keep_bitrate"] {
        assert!(SECTION_KEYS.iter().all(|rows| rows.iter().all(|keys| keys[0] != gone)), "{gone} names no row");
    }
}

/// Runs `draw` in a bare frame `width` wide.
fn bare_frame<R>(width: f32, draw: impl FnOnce(&mut Ui) -> R) -> R {
    let ctx = egui::Context::default();
    let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 800.0))), ..Default::default() };
    let (mut draw, mut out) = (Some(draw), None);
    let mut output = ctx.run_ui(input, |ui| {
        if let Some(draw) = draw.take() {
            out = Some(draw(ui));
        }
    });
    output.textures_delta.clear();
    out.expect("the frame ran")
}

/// Rows placed one after another are one group across the whole column; a gap starts another.
#[test]
fn rows_that_follow_each_other_are_one_group_across_the_column() {
    let (a, b, c, column_width) = bare_frame(1000.0, |ui| {
        let column_width = ui.available_width();
        let a = group_row(ui, |ui| { ui.label("a"); });
        let b = group_row(ui, |ui| { ui.label("b"); });
        ui.add_space(30.0);
        let c = group_row(ui, |ui| { ui.label("c"); });
        (a, b, c, column_width)
    });
    assert!((a.bottom() - b.top()).abs() < 0.5, "{a:?} then {b:?}");
    assert!((a.width() - column_width).abs() < 0.5, "{} of {column_width}", a.width());
    assert!(c.top() - b.bottom() >= 30.0, "{b:?} then {c:?}");
}

/// A column as wide as a page may ever want still ends with the page: its controls stay on screen.
#[test]
fn a_column_ends_with_the_page_and_stops_at_its_widest_on_a_wide_window() {
    for (window, most) in [(800.0, 800.0), (3000.0, SETTINGS_W)] {
        let right = bare_frame(window, |ui| {
            let mut right = 0.0;
            column(ui, SETTINGS_W, |ui| right = group_row(ui, |ui| { ui.label("a"); }).right());
            right
        });
        assert!(right <= most + 8.0 && right > most - 60.0, "at {window}: {right}");
    }
}

/// A group's heading is the section type as written, not a shouted label, and no group is a
/// catch-all "Fine-tuning".
#[test]
fn a_group_heading_is_written_as_it_reads() {
    // The Refresh rate group's rows show with Variable refresh rate off.
    let mut f = settings_with(Settings { vrr: false, ..Settings::default() });
    f.l.settings_view.section = "Display & image";
    open_folds(&mut f);
    assert!(shows(&f, "Refresh rate") && shows(&f, "PC display"), "{:?}", f.drawn);
    assert!(!shows(&f, "REFRESH RATE") && !shows(&f, "Fine-tuning"), "{:?}", f.drawn);
}

/// The switch and the list choice take the sizes the design gives every control.
#[test]
fn a_switch_and_a_list_choice_have_the_designed_size() {
    let (switch, choice) = bare_frame(1000.0, |ui| {
        let switch = toggle(ui, &mut true).rect;
        let choice = Combo::from_id_salt("choice").selected_text("Automatic").show_ui(ui, |_| {}).response.rect;
        (switch, choice)
    });
    assert_eq!(switch.size(), vec2(design_tokens::SIZE_SWITCH_W, design_tokens::SIZE_SWITCH_H));
    assert_eq!(choice.height(), 40.0);
}
