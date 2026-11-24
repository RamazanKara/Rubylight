//! Unit tests for settings search, presets, stream-menu bindings, and banner alignment.

use super::*;

#[test]
fn the_search_needs_every_word_somewhere_in_the_row() {
    let words = search_words("  Frame   RATE ");
    assert_eq!(words, ["frame", "rate"]);
    assert!(found(&words, &["Frame rate", "Pictures per second.", "Video", ""]));
    assert!(found(&search_words("video pictures"), &["Frame rate", "Pictures per second.", "Video", ""]), "words may come from the section and the description");
    assert!(!found(&search_words("frame audio"), &["Frame rate", "Pictures per second.", "Video", ""]));
}

#[test]
fn reset_all_keeps_the_language_and_the_settings_without_a_row() {
    let changed = Settings {
        fps: 120,
        hdr: true,
        overlay: true,
        overlay_mode: crate::config::OverlayMode::Debug,
        language: Language::German,
        beam_race: true,
        ui_zoom: 150,
        multi_controller: false,
        deadzone: 12,
        console_home: false,
        ..Settings::default()
    };
    let reset = reset_to_defaults(&changed);
    assert_eq!(reset, Settings { language: Language::German, beam_race: true, ..Settings::default() });
}

#[test]
fn every_listed_row_names_a_setting_and_says_its_value() {
    let defaults = serde_json::to_value(Settings::default()).unwrap();
    let display = Display { width: 1920, height: 1080, refresh_hz: 60.0, refresh_reported: true };
    for keys in SECTION_KEYS.iter().flat_map(|keys| keys.iter()) {
        for key in *keys {
            assert!(defaults.get(*key).is_some(), "{key} is a setting");
        }
        assert!(!value_text(keys[0], &Settings::default(), &display, crate::glyphs::Style::Xbox).is_empty(), "{} says its value", keys[0]);
    }
    assert_eq!(value_text("fps", &Settings::default(), &display, crate::glyphs::Style::Xbox), "Display (60 Hz)");
    assert_eq!(value_text("overlay", &Settings { overlay_mode: crate::config::OverlayMode::Debug, ..Settings::default() }, &display, crate::glyphs::Style::Xbox), "Off");
    // The merged choices say the one value both keys make.
    let value = |s: Settings, key: &str| value_text(key, &s, &display, crate::glyphs::Style::Xbox);
    assert_eq!(value(Settings::default(), "controllers"), "A player each");
    assert_eq!(value(Settings { multi_controller: false, ..Settings::default() }, "controllers"), "One player");
    assert_eq!(value(Settings { controllers: false, ..Settings::default() }, "controllers"), "Off");
    assert_eq!(value(Settings::default(), "open_on_start"), "Home");
    assert_eq!(value(Settings { console_home: false, ..Settings::default() }, "open_on_start"), "Show my PCs");
    assert_eq!(value(Settings::default(), "prefer_yuv444"), "Off");
    assert_eq!(value(Settings { experimental_packed_444: true, ..Settings::default() }, "prefer_yuv444"), "On");
    assert_eq!(value(Settings::default(), "network_grace_ms"), "Off", "the desktop's 3 s default is within the connection's own timeout");
    assert_eq!(value(Settings { network_grace_ms: 30_000, ..Settings::default() }, "network_grace_ms"), "30 s");
    assert_eq!(value(Settings::default(), "align_video_top"), "Center");
    // Variable refresh rate says the one choice its three keys make, under any of them.
    let fastest = Settings { present_at_host_times: false, vrr_tear_free: false, ..Settings::default() };
    for key in ["vrr", "present_at_host_times", "vrr_tear_free"] {
        assert_eq!(value(Settings::default(), key), "On");
        assert_eq!(value(fastest.clone(), key), "Fastest");
        assert_eq!(value(Settings { vrr: false, ..Settings::default() }, key), "Off");
    }
    let mixed = Settings { vrr_tear_free: false, ..Settings::default() };
    assert_eq!(value(mixed, "vrr"), if vrr_tear_free_applies() { "Custom" } else { "On" });
}

/// The choice each of the eight saved combinations makes, where `vrr_tear_free` applies
/// (Windows, Linux) and where it does not (macOS).
#[test]
fn variable_refresh_rate_reads_every_saved_combination() {
    use VrrChoice::{Custom, Fastest, Off, On};
    // (vrr, present_at_host_times, vrr_tear_free) -> where it applies, where it does not.
    let table = [
        ((false, false, false), Off, Off),
        ((false, false, true), Off, Off),
        ((false, true, false), Off, Off),
        ((false, true, true), Off, Off),
        ((true, false, false), Fastest, Fastest),
        ((true, false, true), Custom, Fastest),
        ((true, true, false), Custom, On),
        ((true, true, true), On, On),
    ];
    for ((vrr, present_at_host_times, vrr_tear_free), applies, elsewhere) in table {
        let s = Settings { vrr, present_at_host_times, vrr_tear_free, ..Settings::default() };
        assert_eq!(vrr_choice_where(&s, true), applies, "{vrr} {present_at_host_times} {vrr_tear_free} where tear-free applies");
        assert_eq!(vrr_choice_where(&s, false), elsewhere, "{vrr} {present_at_host_times} {vrr_tear_free} where it does not");
        assert_ne!(elsewhere, Custom, "never Custom where tear-free does not apply");
        assert_eq!(vrr_choice(&s), if vrr_tear_free_applies() { applies } else { elsewhere });
    }
    assert_eq!(vrr_choice(&Settings::default()), On, "the defaults are On");
}

/// On and Fastest write all three keys, Off only `vrr`; Custom is shown, never offered, and
/// choosing it writes nothing. Reset all puts the three back together.
#[test]
fn choosing_variable_refresh_rate_writes_its_keys() {
    use VrrChoice::{Custom, Fastest, Off, On};
    assert_eq!(VrrChoice::OFFERED, [Off, On, Fastest]);
    assert!(!VrrChoice::OFFERED.contains(&Custom));
    for bits in 0..8u8 {
        let before = Settings { vrr: bits & 1 != 0, present_at_host_times: bits & 2 != 0, vrr_tear_free: bits & 4 != 0, ..Settings::default() };
        let chose = |choice| {
            let mut s = before.clone();
            set_vrr_choice(&mut s, choice);
            s
        };
        let on = chose(On);
        assert_eq!((on.vrr, on.present_at_host_times, on.vrr_tear_free), (true, true, true), "{bits}");
        assert_eq!(vrr_choice(&on), On);
        let fastest = chose(Fastest);
        assert_eq!((fastest.vrr, fastest.present_at_host_times, fastest.vrr_tear_free), (true, false, false), "{bits}");
        assert_eq!(vrr_choice(&fastest), Fastest);
        let off = chose(Off);
        assert_eq!(off, Settings { vrr: false, ..before.clone() }, "Off changes only vrr ({bits})");
        assert_eq!(vrr_choice(&off), Off);
        assert_eq!(chose(Custom), before, "Custom writes nothing ({bits})");
        let reset = reset_to_defaults(&fastest);
        assert_eq!((reset.vrr, reset.present_at_host_times, reset.vrr_tear_free), (true, true, true));
    }
}

/// "When Nightjar opens" saves both of its keys, and each choice reads back as itself.
#[test]
fn when_nightjar_opens_writes_both_of_its_keys() {
    for screen in StartScreen::ALL {
        for console_home in [true, false] {
            let mut s = Settings { console_home, ..Settings::default() };
            set_start_screen(&mut s, screen);
            assert_eq!(start_screen(&s), screen, "{screen:?} from console_home {console_home}");
        }
    }
    let mut s = Settings::default();
    set_start_screen(&mut s, StartScreen::Pcs);
    assert_eq!((s.open_on_start, s.console_home), (crate::config::OpenOnStart::Pcs, false));
    set_start_screen(&mut s, StartScreen::Last);
    assert_eq!((s.open_on_start, s.console_home), (crate::config::OpenOnStart::Last, false), "Continue keeps the screen behind it");
    assert_eq!(start_screen_name(StartScreen::Chosen, Some("Racing Game")), "Racing Game");
}

/// The tier tables name every row once, and an action row is never a setting.
#[test]
fn the_tier_tables_name_each_row_once() {
    let settings = serde_json::to_value(Settings::default()).unwrap();
    let mut seen = std::collections::HashSet::new();
    for &(desktop, _) in ANDROID_KEYS {
        assert!(seen.insert(desktop), "{desktop} is listed once");
        assert!(settings.get(desktop).is_some() != ACTION_ROWS.contains(&desktop), "{desktop} is a setting or an action, not both");
    }
    for action in ACTION_ROWS {
        assert!(settings.get(action).is_none(), "{action} stores nothing");
    }
    assert!(main_row("window_mode") && main_row("vrr") && main_row("mouse_mode") && main_row("overlay_position") && main_row("connection_test"));
    assert!(!main_row("swap_face_buttons") && !main_row("prefer_yuv444") && !main_row("pyrowave_pace") && !main_row("usb_forwarding"));
}

/// A button given Close game before it became Quit game reads as the one Quit game choice.
#[test]
fn a_saved_close_game_button_reads_quit_game() {
    assert_eq!(stream_menu_button_name(Some("close_game")), "Quit game");
    assert_eq!(stream_menu_button_name(Some("quit_game")), "Quit game");
    assert_eq!(crate::stream_menu::ASSIGNABLE.iter().filter(|(_, name)| *name == "Quit game").count(), 1, "one Quit game in the list");
    // The list highlights it too.
    let mut s = Settings::default();
    s.stream_menu_buttons.insert(Hotkey::Lb, "close_game".to_owned());
    s.stream_menu_buttons.insert(Hotkey::Y, "overlay".to_owned());
    assert_eq!(stream_menu_button_choice(&s, Hotkey::Lb).as_deref(), Some("quit_game"));
    assert_eq!(stream_menu_button_choice(&s, Hotkey::Y).as_deref(), Some("overlay"));
    assert_eq!(stream_menu_button_choice(&s, Hotkey::X), None);
}

#[cfg(test)]
mod banner_tests {
    use super::*;

    #[test]
    fn a_banners_words_and_action_share_a_middle() {
        let ctx = egui::Context::default();
        crate::theme::own_palette();
        crate::theme::apply(&ctx);
        let (mut words, mut action) = (Rect::NOTHING, Rect::NOTHING);
        for _ in 0..3 {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1000.0, 400.0))), ..Default::default() };
            let mut output = ctx.run_ui(input, |ui| {
                action = banner(ui, |ui| words = ui.label("A line").rect, |ui| ui.add(egui::Button::new("Go").min_size(vec2(80.0, CONTROL_HEIGHT)))).rect;
            });
            output.textures_delta.clear();
        }
        assert!((words.center().y - action.center().y).abs() <= 1.0, "words {words:?}, action {action:?}");
    }
}
