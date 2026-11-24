//! Settings tests for section keys, debug rows, VRR gating, overrides, controllers, and bitrate choices.

use super::*;

/// The profile notice's line sits beside its button's middle, not at the button's top.
#[test]
fn the_profile_notice_lines_up_with_its_button() {
    let mut f = settings_with(Settings::default());
    let travel = f.l.profiles.create("Travel", serde_json::from_str(r#"{"bitrate_kbps":20000}"#).unwrap()).unwrap();
    f.l.profiles.active = Some(travel);
    f.settle();
    let at = |text: &str| f.placed.iter().find(|(t, ..)| t.starts_with(text)).unwrap_or_else(|| panic!("{text}: {:?}", f.drawn)).1.center().y;
    let (line, button) = (at("Streams on this network use the profile Travel."), at("Manage profiles"));
    assert!((line - button).abs() < 2.0, "the notice at {line}, its button at {button}");
}

/// SECTION_KEYS lists every setting row in the order drawn with the folds open, so a PC's count
/// and Reset all miss none.
#[test]
fn the_section_key_table_is_what_the_page_draws() {
    // Every setting another one shows on.
    let changed = Settings {
        bitrate_kbps: 30_000,
        adaptive_quality: crate::config::QualityMode::Motion,
        upscaling: crate::config::Upscaling::Fsr1,
        virtual_display: true,
        controller_mouse: true,
        microphone: true,
        overlay: true,
        client_roaming: true,
        dual_screen: true,
        pyrowave_decoder: crate::config::PyroWaveDecoder::Gpu,
        decoder: DecoderPreference::Software,
        ..Settings::default()
    };
    let only_here = |key: &str, vrr: bool| match key {
        "check_updates" => (crate::platform_hooks::hooks().self_update_checks)(),
        "network_priority" | "receive_coalescing" => cfg!(any(target_os = "linux", windows)),
        // It needs HDR, which hides Less color banding.
        "hdr_desktop_brightness" => false,
        // A PC's own settings, for a host that reports them (NvidiaEnhancements).
        "rtx_hdr" | "rtx_video" => false,
        // Only where a saved PC offers more than stock packets; the fixture PC has no serverinfo.
        "jumbo_frames" => false,
        "frame_pacing" => !vrr,
        // One row with Fractional V-Sync, saying why the even cadence holds nothing (VRR, macOS, X11).
        "vsync" => true,
        // Everywhere, saying why where it holds nothing (VRR, macOS, X11).
        "fractional_vsync" => true,
        "renderer" | "display_output" | "video_planes" | "high_performance_gpu_power" | "vulkan_video_first" | "vulkan_rfi" => cfg!(target_os = "linux"),
        "videotoolbox_rfi" => cfg!(target_os = "macos"),
        // A Mac's GPU decoder uses the graphics queue of a device of its own.
        "pyrowave_async_compute" => !cfg!(target_os = "macos"),
        "graphics_card" => graphics_card_here(),
        // Windows' wait for the scan line, with Variable refresh rate On.
        "vrr_wait" => cfg!(windows) && vrr,
        _ => true,
    };
    for vrr in [false, true] {
        let mut f = settings_with(Settings { vrr, ..changed.clone() });
        f.l.tearing_supported = true;
        open_folds(&mut f);
        for (at, section) in SETTINGS_SECTIONS.iter().enumerate() {
            f.l.settings_view.section = section;
            f.settle();
            let mut drawn = setting_keys(&f);
            drawn.dedup();
            let listed: Vec<&str> = SECTION_KEYS[at].iter().map(|keys| keys[0]).filter(|k| only_here(k, vrr)).collect();
            assert_eq!(drawn, listed, "{section}, VRR {vrr}");
        }
    }
}

/// The settings for developers and diagnosis sit in Debug, the last section, and in no other, on
/// the main settings and a PC's own; the search still finds them there. Advanced holds the
/// renderer, the game hint and the link and pace tests; Video position stays in Display & image.
#[test]
fn the_developer_rows_and_diagnostics_are_in_debug() {
    // Android's main rows in its order, then PyroWave's, the decoder's, the cursor and Diagnostics.
    const DEBUG: [&str; 11] = ["error_correction", "tell_host_wireless",
        "network_priority", "unbatched_input", "pyrowave_decoder", "pyrowave_async_compute", "decoder",
        "vulkan_video_first", "vulkan_rfi", "videotoolbox_rfi", "cursor_shapes"];
    assert_eq!(SETTINGS_SECTIONS.last(), Some(&"Debug"));
    // Prioritize stream traffic does something on Linux and Windows, so only they draw it; Vulkan
    // video is Linux's; the compute queue is the GPU decoder's, macOS's default is the processor.
    let wanted = DEBUG.into_iter()
        .filter(|k| *k != "network_priority" || cfg!(any(target_os = "linux", windows)))
        .filter(|k| !matches!(*k, "vulkan_video_first" | "vulkan_rfi") || cfg!(target_os = "linux"))
        .filter(|k| *k != "videotoolbox_rfi" || cfg!(target_os = "macos"))
        .filter(|k| *k != "pyrowave_async_compute" || (Settings::default().pyrowave_decoder == crate::config::PyroWaveDecoder::Gpu && !cfg!(target_os = "macos")))
        .collect::<Vec<_>>();
    let mut f = settings_with(Settings::default());
    open_folds(&mut f);
    let id = f.hosts.list[0].id;
    for screen in [Screen::Settings, Screen::HostSettings(id)] {
        f.l.go(screen);
        for section in SETTINGS_SECTIONS {
            if screen != Screen::Settings && section == "App" { continue; }
            f.l.settings_view.section = section;
            f.settle();
            let drawn = setting_keys(&f);
            match section {
                "Debug" => {
                    // Diagnostics, last; decode time after streaming belongs to the whole app.
                    let mut wanted = wanted.clone();
                    wanted.push("link_probe");
                    if screen == Screen::Settings { wanted.push("post_stream_decode_time"); }
                    assert_eq!(drawn, wanted, "{screen:?}: Debug holds exactly these")
                }
                _ => assert!(!drawn.iter().any(|k| DEBUG.contains(k)), "{screen:?} {section}: {drawn:?}"),
            }
            assert_eq!(drawn.contains(&"align_video_top"), section == "Display & image", "{screen:?} {section}: {drawn:?}");
            // Full color range is a display-matching choice: Display & image, not Debug.
            assert_eq!(drawn.contains(&"full_range"), section == "Display & image", "{screen:?} {section}: {drawn:?}");
            if section == "Advanced" {
                // The renderer is Linux's choice for the whole app, so a PC's settings do not have it.
                assert_eq!(drawn.contains(&"renderer"), cfg!(target_os = "linux") && screen == Screen::Settings, "{drawn:?}");
                // So is where it shows itself, and whether a stream goes on a display plane there.
                for key in ["display_output", "video_planes", "high_performance_gpu_power"] {
                    assert_eq!(drawn.contains(&key), cfg!(target_os = "linux") && screen == Screen::Settings, "{key}: {drawn:?}");
                }
                assert!(drawn.contains(&"sops") && !drawn.contains(&"link_probe"), "{screen:?}: {drawn:?}");
            }
        }
    }
    f.l.go(Screen::Settings);
    for (query, row) in [("error correction", "Error correction"), ("local cursor", "Local cursor")] {
        f.l.settings_view.query = query.into();
        f.settle();
        // A search lists "Debug" with the switch off; the section's count says the row was found there.
        assert!(shows(&f, row) && f.l.settings_view.found[SETTINGS_SECTIONS.len() - 1] > 0,
            "{query} finds {row} in Debug: {:?} {:?}", f.l.settings_view.found, f.drawn);
    }
}

/// Frame pacing is drawn with Variable refresh rate off, and VRR's way of showing frames is its own
/// row's choice, with no rows of its own: a gate over some rows never carries over to the rows after
/// them. V-Sync is one row with its even cadence, so it stays and says why it holds nothing under
/// VRR; where the screen can't tear, VRR has no row and works as off.
#[test]
fn the_vrr_rows_follow_their_own_gates() {
    let (vrr_row, pacing, vsync) = ("Variable refresh rate", "Frame pacing", "V-Sync");
    let display = |s: Settings, tearing: bool| {
        let mut f = settings_with(s);
        f.l.tearing_supported = tearing;
        f.l.settings_view.section = "Display & image";
        open_folds(&mut f);
        f
    };
    for vrr in [false, true] {
        let f = display(Settings { vrr, frame_pacing: crate::config::FramePacing::Latency, ..Settings::default() }, true);
        assert!(shows(&f, vrr_row), "{:?}", f.drawn);
        // V-Sync is one row with its even cadence, which says why it holds nothing under VRR.
        assert_eq!((shows(&f, pacing), shows(&f, vsync)), (!vrr, true), "VRR {vrr}: {:?}", f.drawn);
        assert_eq!(shows(&f, "Not used with Variable refresh rate"), vrr, "VRR {vrr}: {:?}", f.drawn);
        assert!(!shows(&f, "Real VRR") && !shows(&f, "Tear-free VRR"), "VRR {vrr}: one row: {:?}", f.drawn);
        assert!(shows(&f, "Upscaling"), "VRR {vrr}: the next group is its own");
    }
    let f = display(Settings { vrr: false, frame_pacing: crate::config::FramePacing::Balanced, ..Settings::default() }, true);
    assert!(shows(&f, pacing) && shows(&f, vsync), "a pacing that waits keeps V-Sync on, its cadence still a choice: {:?}", f.drawn);
    if cfg!(windows) {
        let f = display(Settings { vrr: true, frame_pacing: crate::config::FramePacing::Latency, ..Settings::default() }, false);
        assert!(!shows(&f, vrr_row) && shows(&f, pacing) && shows(&f, vsync), "{:?}", f.drawn);
    }
}

/// Variable refresh rate is one choice for its three keys: On and Fastest write all three, Off
/// only `vrr`. A mix none of them writes reads Custom, which the list never offers.
#[test]
fn variable_refresh_rate_is_one_choice_for_its_three_keys() {
    let row = "Variable refresh rate";
    let mut f = settings_with(Settings::default());
    f.l.tearing_supported = true;
    f.l.settings_view.section = "Display & image";
    f.settle();
    assert_eq!(row_words(&f, row), ["On"], "the defaults are On");
    click_value(&mut f, row, "On");
    assert!(shows(&f, "Fastest") && shows(&f, "Off"), "the list is open: {:?}", f.drawn);
    click_last(&mut f, "Fastest");
    let triple = |f: &Frames| (f.l.settings.vrr, f.l.settings.present_at_host_times, f.l.settings.vrr_tear_free);
    assert_eq!(triple(&f), (true, false, false), "Fastest writes all three");
    click_value(&mut f, row, "Fastest");
    click_last(&mut f, "Off");
    assert_eq!(triple(&f), (false, false, false), "Off writes only vrr");
    click_value(&mut f, row, "Off");
    click_last(&mut f, "On");
    assert_eq!(triple(&f), (true, true, true), "On writes all three");

    // A mix from the old switches or a hand-edited file: shown honestly, not offered.
    let mixed = Settings { present_at_host_times: false, vrr_tear_free: true, ..Settings::default() };
    let mut f = settings_with(mixed.clone());
    f.l.tearing_supported = true;
    f.l.settings_view.section = "Display & image";
    f.settle();
    let shown = if vrr_tear_free_applies() { "Custom" } else { "Fastest" };
    let words = row_words(&f, row);
    assert!(words.contains(&shown) && words.contains(&"Reset"), "{words:?}");
    assert_eq!(f.l.settings, mixed, "drawing it changes nothing");
    click_value(&mut f, row, "Reset");
    assert_eq!(triple(&f), (true, true, true), "Reset puts all three back");
    f.l.settings = mixed.clone();
    f.settle();
    let customs = |f: &Frames| f.placed.iter().filter(|(t, ..)| t == "Custom").count();
    let before = customs(&f);
    click_value(&mut f, row, shown);
    assert!(shows(&f, "Off") && shows(&f, "On"), "the list is open: {:?}", f.drawn);
    assert_eq!(customs(&f), before, "Custom is not among the choices: {:?}", f.drawn);
    click_last(&mut f, "On");
    assert_eq!(triple(&f), (true, true, true));
}

/// A PC's own Variable refresh rate keeps all three keys together, and its box gives all three
/// back.
#[test]
fn a_pcs_variable_refresh_rate_keeps_its_three_keys_together() {
    use egui::accesskit::{Role, Toggled};
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    f.ctx.enable_accesskit();
    f.l.tearing_supported = true;
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.l.settings_view.section = "Display & image";
    f.settle();
    let row = "Variable refresh rate";
    click_value(&mut f, row, "On");
    click_last(&mut f, "Fastest");
    for key in ["vrr", "present_at_host_times", "vrr_tear_free"] {
        assert!(f.l.edit_overrides.has(&[key]), "the PC keeps {key}: {:?}", f.l.edit_overrides);
    }
    let edit = &f.l.edit_settings;
    assert_eq!((edit.vrr, edit.present_at_host_times, edit.vrr_tear_free), (true, false, false));
    let row_box = |f: &Frames| {
        f.access.as_ref().expect("AccessKit is on").nodes.iter()
            .find(|(_, n)| n.role() == Role::CheckBox && n.label() == Some("For this PC") && n.description() == Some(row))
            .map(|(_, n)| (n.toggled(), n.bounds().expect("laid out")))
            .expect("the row's box")
    };
    let (toggled, bounds) = row_box(&f);
    assert_eq!(toggled, Some(Toggled::True));
    f.click_at(Pos2::new(((bounds.x0 + bounds.x1) / 2.0) as f32, ((bounds.y0 + bounds.y1) / 2.0) as f32), egui::PointerButton::Primary);
    assert!(f.l.edit_overrides.is_empty(), "the box gives all three back: {:?}", f.l.edit_overrides);
    let edit = &f.l.edit_settings;
    assert_eq!((edit.vrr, edit.present_at_host_times, edit.vrr_tear_free), (true, true, true), "the main settings' On again");
}

/// Sharper colored text is one choice for its two keys, Off or On; under PyroWave it has
/// no row, nor has Less color banding or Video decoding (in Debug).
#[test]
fn sharper_colored_text_is_one_choice_hidden_under_pyrowave() {
    let mut f = settings_with(Settings { experimental_packed_444: true, ..Settings::default() });
    open_folds(&mut f);
    assert!(row_words(&f, "Sharper colored text").contains(&"On") && shows(&f, "Less color banding"), "{:?}", f.drawn);
    click_value(&mut f, "Sharper colored text", "On");
    let commands = click_last(&mut f, "Off");
    assert!(!f.l.settings.prefer_yuv444 && !f.l.settings.experimental_packed_444, "Off turns both off");
    assert!(commands.iter().any(|c| matches!(c, Command::AskFullChromaAgain)), "{} commands", commands.len());
    click_value(&mut f, "Sharper colored text", "Off");
    click_last(&mut f, "On");
    assert!(f.l.settings.prefer_yuv444 && f.l.settings.experimental_packed_444, "On writes both");
    f.l.settings_view.section = "Debug";
    f.settle();
    assert!(shows(&f, "Video decoding"), "{:?}", f.drawn);

    let mut f = settings_with(Settings { codec: CodecPreference::PyroWave, experimental_packed_444: true, ..Settings::default() });
    open_folds(&mut f);
    assert!(!shows(&f, "Sharper colored text") && !shows(&f, "Less color banding"), "{:?}", f.drawn);
    f.l.settings_view.section = "Debug";
    f.settle();
    assert!(!shows(&f, "Video decoding") && shows(&f, "PyroWave decoder"), "{:?}", f.drawn);
}

/// A PC's own value on a row its codec hides (Sharper colored text under PyroWave) still counts;
/// with the codec given back, the row shows with its box ticked, and the box gives it back.
#[test]
fn a_pcs_own_value_on_a_hidden_row_comes_back_with_its_row() {
    use egui::accesskit::{Role, Toggled};
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    f.ctx.enable_accesskit();
    f.l.settings_view.more = [true; 9];
    f.hosts.list[0].record.settings = Some(serde_json::from_str(r#"{"codec":"PyroWave","prefer_yuv444":true}"#).unwrap());
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    let row_box = |f: &Frames, row: &str| {
        f.access.as_ref().expect("AccessKit is on").nodes.iter()
            .find(|(_, n)| n.role() == Role::CheckBox && n.label() == Some("For this PC") && n.description() == Some(row))
            .map(|(_, n)| (n.toggled(), n.bounds().expect("laid out")))
    };
    assert!(row_box(&f, "Sharper colored text").is_none() && shows(&f, "2 settings are set here."), "{:?}", f.drawn);
    let click = |f: &mut Frames, bounds: egui::accesskit::Rect| {
        f.click_at(Pos2::new(((bounds.x0 + bounds.x1) / 2.0) as f32, ((bounds.y0 + bounds.y1) / 2.0) as f32), egui::PointerButton::Primary);
    };
    let (_, codec) = row_box(&f, "Video codec").expect("the codec's box");
    click(&mut f, codec);
    let (toggled, bounds) = row_box(&f, "Sharper colored text").expect("the row is back");
    assert_eq!(toggled, Some(Toggled::True), "with the PC's own value");
    click(&mut f, bounds);
    assert!(f.l.edit_overrides.is_empty(), "{:?}", f.l.edit_overrides);
}

/// Left from any row stays in that row (its Reset, a slider) or goes to the open section's
/// entry, never to a Reset or control of another row, with changed rows above and below.
#[test]
fn left_never_lands_on_another_rows_reset() {
    let focused = |f: &Frames| f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect);
    let changed = [Settings { fps: 120, ..Settings::default() }, Settings { fps: 60, bitrate_kbps: 40_000, ..Settings::default() }];
    for width in [1280.0, 900.0] {
        for (case, settings) in changed.iter().enumerate() {
            for step in 0..8 {
                let mut f = settings_with(settings.clone());
                f.size = vec2(width, 4000.0);
                f.l.go(Screen::Settings);
                f.l.settings_view.section = "Video";
                f.settle();
                for _ in 0..step {
                    f.press(Key::ArrowDown);
                }
                let from = focused(&f).expect("a row's control has focus");
                let entry = f.placed.iter().filter(|(t, ..)| t == "Video").map(|p| p.1)
                    .min_by(|a, b| a.height().total_cmp(&b.height())).expect("the entry");
                if from.contains(entry.center()) {
                    continue;
                }
                f.press(Key::ArrowLeft);
                let to = focused(&f).expect("Left keeps a focus");
                let same_row = to.top() < from.bottom() && from.top() < to.bottom();
                assert!(to.contains(entry.center()) || same_row, "{width} case {case} step {step}: Left from {from:?} lands on {to:?}, not the entry {entry:?} or its row");
            }
        }
    }
    // A changed row's own Reset is still left of its control.
    let mut f = settings_with(Settings { width: 1280, height: 720, ..Settings::default() });
    let from = focused(&f).expect("Resolution has focus");
    f.press(Key::ArrowLeft);
    let reset = f.placed.iter().find(|(t, ..)| t == "Reset").expect("Resolution's Reset").1;
    assert!(focused(&f).is_some_and(|r| r.contains(reset.center())), "Left from {from:?} reaches the row's Reset {reset:?}, not {:?}", focused(&f));
}

/// Escape (B) in the search field leaves it for the first row found, so the D-pad carries on.
#[test]
fn escape_in_the_search_moves_focus_to_the_rows() {
    let mut f = settings_with(Settings::default());
    f.press_with(Key::F, egui::Modifiers::COMMAND);
    f.frame(vec![egui::Event::Text("hdr".into())]);
    f.settle();
    f.press(Key::Escape);
    let focused = |f: &Frames| f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect);
    let results = f.placed.iter().find(|(t, ..)| t == "Results for “hdr”").expect("the results").1;
    assert_eq!(f.l.settings_view.query, "hdr", "the first Escape keeps the search");
    assert!(focused(&f).is_some_and(|r| r.left() > results.left() && r.top() > results.bottom()), "a row found has focus: {:?}", focused(&f));
    // With nothing found, the open section's entry takes it.
    f.press_with(Key::F, egui::Modifiers::COMMAND);
    f.frame(vec![egui::Event::Text("zzzz".into())]);
    f.settle();
    f.press(Key::Escape);
    let entry = f.placed.iter().filter(|(t, ..)| t == "Video").map(|p| p.1).min_by(|a, b| a.height().total_cmp(&b.height())).expect("the entry");
    assert!(focused(&f).is_some_and(|r| r.contains(entry.center())), "the entry has focus: {:?}", focused(&f));
}

/// Ctrl+F is the page's: with a dialog open it leaves the focus where it is.
#[test]
fn ctrl_f_leaves_an_open_dialog_alone() {
    let mut f = settings_with(Settings::default());
    f.l.ask(Dialog::Message { title: "Network test failed".into(), body: "The PC did not answer.".into() });
    f.settle();
    let before = f.ctx.memory(|m| m.focused());
    assert!(before.is_some(), "the dialog's button has focus");
    f.press_with(Key::F, egui::Modifiers::COMMAND);
    assert_eq!(f.ctx.memory(|m| m.focused()), before, "the dialog keeps the focus");
}

/// A PC's own value without a row on the page (beam racing) still counts, and Clear all clears it.
#[test]
fn a_pcs_value_without_a_row_counts_and_clears() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    f.hosts.list[0].record.settings = Some(serde_json::from_str(r#"{"beam_race":true}"#).unwrap());
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    assert!(shows(&f, "1 setting is set here.") && shows(&f, "1 of them has no row on this page; Clear all clears it too."), "{:?}", f.drawn);
    click_text(&mut f, "Clear all for this PC");
    click_text(&mut f, "Clear all");
    assert!(f.l.edit_overrides.is_empty() && shows(&f, "Nothing is set here yet."), "{:?}", f.drawn);
}

/// Bitrate on mobile data is the profiles' cap, Android's list: on the main settings once this
/// device was on mobile data or a cap is set, and not a setting of a PC.
#[test]
fn bitrate_on_mobile_data_is_the_profiles_cap() {
    let arrays = include_str!("../../../../../app/src/main/res/values/arrays.xml");
    let values = arrays.split("<string-array name=\"mobile_bitrate_values\"").nth(1).and_then(|a| a.split("</string-array>").next()).expect("Android's list");
    let android: Vec<u32> = values.split("<item>").skip(1).filter_map(|i| i.split("</item>").next()?.trim().parse().ok()).collect();
    assert_eq!(android, MOBILE_BITRATES_KBPS, "the same choices as Android");
    let mut f = settings_with(Settings::default());
    assert!(!shows(&f, "Bitrate on mobile data"), "not on mobile data, no cap: {:?}", f.drawn);
    f.l.profiles.mobile_limit_kbps = 5_000;
    f.settle();
    assert_eq!(row_words(&f, "Bitrate on mobile data"), ["5 Mbps"], "a main row with its value");
    assert!(f.l.settings_view.drawn.contains(&("mobile_bitrate", false)));
    click_value(&mut f, "Bitrate on mobile data", "5 Mbps");
    let commands = click_last(&mut f, "10 Mbps");
    assert_eq!(f.l.profiles.mobile_limit_kbps, 10_000);
    assert!(commands.iter().any(|c| matches!(c, Command::SaveProfiles)), "{} commands", commands.len());
    assert_eq!(f.l.settings, Settings::default(), "a cap, not a setting");
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    assert!(!shows(&f, "Bitrate on mobile data"), "{:?}", f.drawn);
}

/// Controls opens the Controller buttons page from two rows, the test from its own; Controllers
/// is one choice, and with it off the controller rows have nothing to set.
#[test]
fn the_controller_rows_open_their_page_and_controllers_is_one_choice() {
    let mut f = settings_with(Settings::default());
    f.l.settings_view.section = "Controls";
    f.settle();
    for (button, test) in [("Controller buttons", false), ("Test", true)] {
        f.l.controller_test = !test;
        // The button, right of the row's label.
        let at = f.placed.iter().filter(|(t, ..)| t == button).map(|p| p.1).max_by(|a, b| a.left().total_cmp(&b.left())).expect("the button");
        let commands = f.click_at(at.center(), egui::PointerButton::Primary);
        assert!(commands.iter().any(|c| matches!(c, Command::ControllerSettings)), "{button}: {} commands", commands.len());
        assert_eq!(f.l.controller_test, test, "{button}");
        f.l.go(Screen::Settings);
        f.settle();
    }
    open_folds(&mut f);
    assert!(shows(&f, "Nintendo button layout") && shows(&f, "Stick dead zone"), "{:?}", f.drawn);
    click_value(&mut f, "Controllers", "A player each");
    click_last(&mut f, "One player");
    assert_eq!((f.l.settings.controllers, f.l.settings.multi_controller), (true, false));
    click_value(&mut f, "Controllers", "One player");
    click_last(&mut f, "Off");
    assert!(!f.l.settings.controllers, "Off sends none");
    assert!(!shows(&f, "Nintendo button layout") && !shows(&f, "Stick dead zone") && shows(&f, "Send system keys"), "{:?}", f.drawn);
}

/// When Nightjar opens is one choice for Home and the start screen; A chosen game is listed once a
/// game was chosen in its menu.
#[test]
fn when_nightjar_opens_is_one_choice() {
    let mut f = settings_with(Settings::default());
    f.l.settings_view.section = "App";
    f.settle();
    assert_eq!(row_words(&f, "When Nightjar opens"), ["Home"]);
    click_value(&mut f, "When Nightjar opens", "Home");
    assert!(shows(&f, "Show my PCs") && shows(&f, "Continue my last game") && !shows(&f, "A chosen game"), "{:?}", f.drawn);
    click_last(&mut f, "Show my PCs");
    assert_eq!((f.l.settings.open_on_start, f.l.settings.console_home), (crate::config::OpenOnStart::Pcs, false));
    f.l.open_game = Some(crate::config::OpenGame { host: "uid".into(), install_id: "a".into(), app_id: 7, title: "Racing Game".into() });
    f.settle();
    click_value(&mut f, "When Nightjar opens", "Show my PCs");
    click_last(&mut f, "Racing Game");
    assert_eq!(f.l.settings.open_on_start, crate::config::OpenOnStart::Chosen);
    assert!(row_words(&f, "When Nightjar opens").contains(&"Racing Game"), "{:?}", f.drawn);
}

/// The Controller buttons page: one switch for the test, in the rows' style, and no sentence
/// repeated under every axis and button.
#[test]
fn the_controller_buttons_page_has_one_test_switch_and_no_repeated_hints() {
    use egui::accesskit::Role;
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    f.ctx.enable_accesskit();
    f.l.go(Screen::Controllers);
    f.l.controller_key = "pad".into();
    f.l.controller_mapping = crate::gamepad_types::Mapping { name: "Pad".into(), ..Default::default() };
    f.settle();
    assert!(shows(&f, "Controller buttons") && shows(&f, "Controller test"), "{:?}", f.drawn);
    assert!(!f.drawn.iter().any(|t| t.starts_with("Choose the physical axis") || t.starts_with("Default follows the Nintendo") || t.starts_with("Test input")), "{:?}", f.drawn);
    let switch = f.access.as_ref().expect("AccessKit is on").nodes.iter()
        .find(|(_, n)| n.role() != Role::Label && n.label() == Some("Controller test"))
        .and_then(|(_, n)| n.bounds())
        .expect("the test's switch");
    f.click_at(Pos2::new(((switch.x0 + switch.x1) / 2.0) as f32, ((switch.y0 + switch.y1) / 2.0) as f32), egui::PointerButton::Primary);
    assert!(f.l.controller_test);
}

/// Never survives every Recommended choice but Save data, which turns Nightjar's own lowering on;
/// the page does not show the row.
#[test]
fn never_survives_every_recommended_choice_but_save_data() {
    let display = Display { width: 2560, height: 1440, refresh_hz: 120.0, refresh_reported: true };
    let caps = simple_caps(&display, false, true);
    let recommended = simple::recommended(&caps);
    let mut s = Settings { lower_bitrate: LowerBitrate::Keep, ..Settings::default() };
    for choices in [
        simple::Choices { smoothness: simple::Smoothness::Smoothest, ..recommended },
        simple::Choices { hdr: simple::Hdr::Off, ..recommended },
        simple::Choices { picture: simple::Picture::Auto, ..recommended },
        simple::Choices { picture: simple::Picture::Best, ..recommended },
    ].into_iter().enumerate() {
        simple_apply(&mut s, &display, &caps, choices.1);
        assert_eq!(s.lower_bitrate, LowerBitrate::Keep, "choice {}", choices.0);
    }
    simple_apply(&mut s, &display, &caps, simple::Choices { picture: simple::Picture::SaveData, ..recommended });
    assert_eq!(s.lower_bitrate, LowerBitrate::WhenFramesLost);
    // Leaving Save data takes back the lowering it turned on.
    simple_apply(&mut s, &display, &caps, simple::Choices { picture: simple::Picture::Auto, ..recommended });
    assert_eq!(s.lower_bitrate, LowerBitrate::Off);
}

/// A PC's, game's or profile's Lower bitrate saved as the old switches (an imported profile too)
/// reads as its own choice over a main Never.
#[test]
fn an_old_switch_override_over_never_reads_as_its_own_choice() {
    let never = Settings { lower_bitrate: LowerBitrate::Keep, ..Settings::default() };
    for (json, choice) in [(r#"{"adaptive_bitrate":true}"#, LowerBitrate::WhenFramesLost), (r#"{"adaptive_bitrate":false,"client_hint_bitrate":false}"#, LowerBitrate::Off),
        (r#"{"client_hint_bitrate":true}"#, LowerBitrate::WhenFramesLost)] {
        let layer: Overrides = serde_json::from_str(json).unwrap();
        assert_eq!(layer.apply(&never).lower_bitrate, choice, "{json}");
    }
    // A layer without the row leaves the main Never; one saved with Never keeps it.
    let other: Overrides = serde_json::from_str(r#"{"fps":60}"#).unwrap();
    assert_eq!(other.apply(&never).lower_bitrate, LowerBitrate::Keep);
    let kept: Overrides = serde_json::from_str(r#"{"adaptive_bitrate":false,"client_hint_bitrate":false,"keep_bitrate":true}"#).unwrap();
    assert_eq!(kept.apply(&Settings::default()).lower_bitrate, LowerBitrate::Keep);
}

/// Lower bitrate's list open on the Video section, at 1920x1080:
/// `NIGHTJAR_SHOTS=<dir> cargo test -p nightjar-desktop lower_bitrate_open_screenshot -- --ignored`.
#[test]
#[ignore = "writes PNGs; set NIGHTJAR_SHOTS"]
fn lower_bitrate_open_screenshot() {
    let Some(dir) = std::env::var_os("NIGHTJAR_SHOTS").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&dir).expect("the folder for the screenshots");
    let mut f = home_with_games(1);
    f.size = vec2(1920.0, 1080.0);
    f.shots = true;
    f.l.logo = None;
    f.l.go(Screen::Settings);
    f.l.settings_view.rail_more = true;
    f.l.settings_view.section = "Video";
    f.l.settings_view.to_top = true;
    f.settle();
    let at = f.placed.iter().find(|(t, ..)| t == "The PC decides").expect("the row's value").1.center();
    f.click_at(at, egui::PointerButton::Primary);
    f.settle();
    assert!(f.placed.iter().any(|(t, ..)| t == "Never: keep my bitrate"), "the list is open");
    assert!(!f.placed.iter().any(|(t, ..)| t == "When the network is busy"), "no longer a choice");
    f.shoot(&dir, "settings-lower-bitrate-open-1920x1080");
}
