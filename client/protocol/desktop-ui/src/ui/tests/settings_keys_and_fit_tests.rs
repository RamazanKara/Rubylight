//! Settings tier parity, accessibility labels, and 150% viewport fit tests.

use super::*;

#[test]
fn the_pyrowave_gpu_probe_starts_only_with_pyrowave_selected_or_the_codec_menu_open() {
    use crate::config::{CodecPreference, PyroWaveDecoder};
    let started = || crate::platform_hooks::pyrowave_probe_started_for_test();
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 12000.0);
    f.l.settings.hdr = true;
    f.l.settings.pyrowave_decoder = PyroWaveDecoder::Gpu;
    f.l.settings.codec = CodecPreference::Hevc;
    f.l.go(Screen::Settings);
    f.settle();
    assert!(!started(), "an ordinary settings view does not probe");
    f.l.settings.codec = CodecPreference::PyroWave;
    f.settle();
    assert!(started(), "PyroWave selected probes, with the HDR plane format");
}

#[test]
fn a_dialog_whose_button_is_wholly_below_the_window_does_not_fit() {
    let screen = Rect::from_min_size(Pos2::ZERO, vec2(853.0, 480.0));
    for (rows, fits) in [(2, true), (12, false)] {
        let ctx = egui::Context::default();
        let raw = egui::RawInput { screen_rect: Some(screen), ..Default::default() };
        let mut button = Rect::NOTHING;
        for _ in 0..4 {
            let mut output = ctx.run_ui(raw.clone(), |ui| {
                egui::Modal::new(egui::Id::unique("dialog")).show(ui.ctx(), |ui| {
                    for row in 0..rows {
                        ui.label(format!("Row {row}"));
                        ui.add_space(100.0);
                    }
                    button = ui.button("Last").rect;
                });
            });
            output.textures_delta.clear();
        }
        assert_eq!(areas_past_window(&ctx, screen).is_empty(), fits, "{rows} rows, button {button:?}");
    }
}

#[test]
fn at_150_percent_in_german_a_1280x720_window_clips_no_button() {
    for state in every_state() {
        let mut f = state.frames();
        fit_150(&mut f);
        assert_fits(&f, &format!("PCs, {}", state.name), false);
        // Every dialog a PC can open, at the busiest state.
        let id = f.hosts.list[0].id;
        let approving = |name: &str, busy: bool| Dialog::ApprovePairing {
            host: id,
            request: "a1".into(),
            name: name.into(),
            address: "192.168.100.116".into(),
            pc: "A long name for a gaming PC".into(),
            pin: "12".into(),
            busy,
        };
        let dialogs = [
            Dialog::HostOptions { host: id },
            Dialog::AddHost { text: String::new(), error: Some("Cannot reach the PC.".into()) },
            Dialog::Otp { host: id, pin: "1234".into(), passphrase: String::new(), error: None },
            Dialog::PairingHelp,
            Dialog::Rename { host: id, text: String::new() },
            Dialog::ConfirmRemove { host: id, name: "A long name for a gaming PC".into() },
            Dialog::ConfirmUnpair { host: id, name: "A long name for a gaming PC".into() },
            Dialog::ConfirmClose { host: id, name: "A game with a rather long title".into() },
            Dialog::ConfirmQuit { host: id, start: Some((8, "Another long game title".into())), running: "A game with a rather long title".into(), resume: Some(7), named: None },
            Dialog::ConfirmReplace { app: "Another long game title".into(), running: "A game with a rather long title".into() },
            Dialog::ContinueHere { host: id, app: 7, resume: true, name: "A game with a rather long title".into(), device: "A long name for a phone".into(), join: true },
            Dialog::NowPlaying { host: id, app: 7, device: "A long name for a television".into() },
            Dialog::GameCameBack { host: id, app: 7 },
            Dialog::MoveFailed { host: id, app: 7 },
            Dialog::PairError { host: id, body: "PIN rejected. Generate a new one-time PIN in the PC’s web UI and check the passphrase.".into(), one_time: true },
            Dialog::Message { title: "Network test failed".into(), body: "The PC did not answer.".into() },
            Dialog::ConfirmSleep { host: id, ask: 1, name: "A long name for a gaming PC".into(), offer: None },
            Dialog::ConfirmSleep { host: id, ask: 1, name: "A long name for a gaming PC".into(), offer: Some(("t".into(), Handed::now())) },
            Dialog::ConfirmPcPower { host: id, ask: 1, name: "A long name for a gaming PC".into(), action: PcPower::ShutDown, offer: None },
            Dialog::ConfirmPcPower {
                host: id,
                ask: 1,
                name: "A long name for a gaming PC".into(),
                action: PcPower::ShutDown,
                offer: Some(("t".into(), Handed::now(), false)),
            },
            Dialog::ConfirmUpdate { host: id, name: "A long name for a gaming PC".into(), version: "3.4.0".into() },
            Dialog::ConfirmUsbGiveBack { count: 2 },
            Dialog::PairRequests { only: Some(id) },
            approving("A long name for a television set", false),
            approving("A long name for a television set", true),
            // A device picks its own name: words, one long word, or lines.
            approving(&"A long name ".repeat(11)[..128], false),
            approving(&"A long name ".repeat(11)[..128], true),
            approving(&"W".repeat(128), false),
            approving(&"W".repeat(128), true),
            approving(&"TV\n".repeat(40), true),
        ];
        for dialog in dialogs {
            let name = format!("{:?}", std::mem::discriminant(&dialog));
            let options = matches!(dialog, Dialog::HostOptions { .. });
            f.l.dialog = dialog;
            f.settle();
            assert_fits(&f, &format!("dialog {name} on {}", state.name), true);
            // The options a long list hides at first are checked too.
            if options {
                f.scroll_to_end();
                assert!(f.drawn.iter().any(|t| t == tr(HostOption::Remove.label())), "{}: the list ends with Remove: {:?}", state.name, f.drawn);
                assert_fits(&f, &format!("dialog {name} on {}, scrolled to the end", state.name), true);
            }
            f.l.dialog = Dialog::None;
            f.settle();
        }
    }
}

#[test]
fn at_150_percent_the_longest_options_list_scrolls_to_the_option_with_focus() {
    let mut f = every_state().into_iter().find(|s| s.name == "sleep not allowed").unwrap().frames();
    fit_150(&mut f);
    f.press(Key::F2);
    assert!(f.options_open());
    let screen = Rect::from_min_size(Pos2::ZERO, f.size);
    let labels: Vec<&str> = host_options(&f.hosts.list[0]).iter().map(|o| tr(o.label())).collect();
    // Sleep is listed but cannot take focus here.
    for _ in 2..labels.len() {
        f.press(Key::ArrowDown);
        // egui animates the scroll for up to 0.3 s; the frames keep coming until it ends.
        f.settle();
        f.settle();
        let focused = f.ctx.memory(|m| m.focused()).expect("an option has focus");
        let rect = f.ctx.read_response(focused).expect("the option was laid out").rect;
        // The list's visible area is the clip its labels are drawn in.
        let clip = f
            .placed
            .iter()
            .find(|(text, at, _)| labels.contains(&text.as_str()) && rect.contains(at.center()))
            .map(|(_, _, clip)| *clip)
            .unwrap_or_else(|| panic!("the option's label was drawn: {rect:?}"));
        assert!(clip.height() < screen.height() - 40.0 && screen.expand(1.0).contains_rect(clip), "the list scrolls inside the window: {clip:?}");
        assert!(clip.expand(1.0).contains_rect(rect), "the option with focus is in the list's visible area: {rect:?} in {clip:?}");
    }
    f.press(Key::Enter);
    assert!(matches!(f.l.dialog, Dialog::ConfirmRemove { .. }), "the last option is Remove");
}

#[test]
fn the_options_dialog_shows_every_option_whole_or_fades_the_edge_it_scrolls_past() {
    // The 1920x1200 screenshot (at 150 %), the smallest window SDL allows, and 1280x720 at 150 %.
    for window in ["1280x800", "720x480", "1280x720 at 150 %"] {
        let mut f = every_state().into_iter().find(|s| s.name == "sleep not allowed").unwrap().frames();
        match window {
            "1280x800" => f.size = vec2(1280.0, 800.0),
            "720x480" => f.size = vec2(720.0, 480.0),
            _ => fit_150(&mut f),
        }
        f.settle();
        f.press(Key::F2);
        assert!(f.options_open());
        let labels: Vec<&str> = host_options(&f.hosts.list[0]).iter().map(|o| tr(o.label())).collect();
        for end in [false, true] {
            if end {
                f.scroll_to_end();
            }
            let what = format!("{window}{}", if end { ", scrolled to the end" } else { "" });
            let rows: Vec<(Rect, Rect)> =
                f.placed.iter().filter(|(t, ..)| labels.contains(&t.as_str())).map(|(_, rect, clip)| (*rect, *clip)).collect();
            // egui does not paint a row wholly out of view.
            let whole = |(rect, clip): &(Rect, Rect)| rect.top() >= clip.top() - 1.0 && rect.bottom() <= clip.bottom() + 1.0;
            let fades = fades(&f);
            if end {
                let remove = f.placed.iter().find(|(t, ..)| t == tr(HostOption::Remove.label())).map(|(_, rect, clip)| (*rect, *clip));
                assert!(remove.as_ref().is_some_and(whole), "{what}: the list ends on Remove, whole: {remove:?}");
            }
            if rows.len() == labels.len() && rows.iter().all(whole) {
                assert!(fades.is_empty(), "{what}: the whole list shows, so no edge fades: {fades:?}");
                continue;
            }
            assert_ne!(window, "1280x800", "a tall window shows every option whole: {rows:?} of {labels:?}");
            assert_eq!(rows.last().map(|r| r.0.top() > rows[0].0.top()), Some(true), "{what}: the list is drawn");
            assert!(!fades.is_empty(), "{what}: a list cut short fades its edge");
            for row in rows.iter().filter(|r| !whole(r)) {
                let seen = row.0.intersect(row.1);
                if seen.height() > 0.0 {
                    assert!(fades.iter().any(|fade| fade.intersects(seen)), "{what}: the cut row {row:?} sits under a fade {fades:?}");
                }
            }
        }
        // Remove asks first, and that dialog fits the same window.
        let id = f.hosts.list[0].id;
        f.l.dialog = Dialog::ConfirmRemove { host: id, name: "A long name for a gaming PC".into() };
        f.settle();
        assert_fits(&f, &format!("confirm remove at {window}"), true);
        for (text, rect, clip) in &f.placed {
            assert!(rect.top() >= clip.top() - 1.0 && rect.bottom() <= clip.bottom() + 1.0, "confirm remove at {window}: {text:?} is cut: {rect:?} in {clip:?}");
        }
    }
}

#[test]
fn at_150_percent_in_german_a_40_character_pc_name_fits_its_card_options_and_details() {
    let name = "Wohnzimmer-PC mit Radeon RX 7900 XT 20GB";
    assert_eq!(name.chars().count(), 40);
    let mut record = record("uid", true, true);
    record.name = name.into();
    let mut f = Frames::with(record, Reach::Online, Some(answer(true, 7)), Vec::new());
    fit_150(&mut f);
    assert!(shows(&f, name), "{:?}", f.drawn);
    // The card ends it in "…" before its edge.
    assert_fits(&f, "PCs", false);
    let id = f.hosts.list[0].id;
    f.l.dialog = Dialog::HostOptions { host: id };
    f.settle();
    assert!(shows(&f, name), "{:?}", f.drawn);
    assert_fits(&f, "options", true);
    f.scroll_to_end();
    assert!(shows(&f, tr(HostOption::Remove.label())), "the list ends with Remove: {:?}", f.drawn);
    assert_fits(&f, "options, scrolled to the end", true);
    f.l.dialog = Dialog::None;
    f.l.open(Screen::HostDetails(id));
    f.settle();
    assert!(shows(&f, name), "{:?}", f.drawn);
    assert_fits(&f, "details page", false);
}

/// A typical PC name of 24 characters is whole on its card in a 1080 x 720 window: it takes a
/// second line before it is cut, and the status line stays under it, above the card's action.
#[test]
fn a_24_character_pc_name_is_whole_on_its_card_at_1080_by_720() {
    let name = "Gaming Rig upstairs (2)!";
    assert_eq!(name.chars().count(), 24);
    let mut record = record("uid", true, true);
    record.name = name.into();
    let mut f = Frames::with(record, Reach::Online, Some(answer(true, 0)), Vec::new());
    f.size = vec2(1080.0, 720.0);
    f.settle();
    fn galley_of(shape: &egui::Shape, text: &str, out: &mut Option<(bool, Rect)>) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| galley_of(s, text, out)),
            egui::Shape::Text(t) if t.galley.job.text == text => *out = Some((t.galley.elided, shape.visual_bounding_rect())),
            _ => {}
        }
    }
    let mut found = None;
    f.shapes.iter().for_each(|c| galley_of(&c.shape, name, &mut found));
    let (elided, at) = found.unwrap_or_else(|| panic!("{:?}", f.drawn));
    assert!(!elided, "the whole name: {at:?}");
    let online = f.placed.iter().find(|(t, ..)| t == "Online").map(|p| p.1).expect("the status line");
    assert!(online.top() >= at.bottom() - 1.0, "the status under the name: {online:?} {at:?}");
    let (card, _) = f.card_and_dots();
    assert!(online.bottom() < card.bottom(), "{online:?} {card:?}");
}

/// In a short window the PC options open below the top bar, never over it, and keep inside the
/// window.
#[test]
fn a_pcs_options_in_a_short_window_stay_below_the_top_bar() {
    let mut f = Frames::with(record("Den", true, true), Reach::Online, Some(answer(true, 7)), Vec::new());
    f.size = vec2(1080.0, 720.0);
    f.settle();
    f.press(Key::F2);
    f.settle();
    assert!(f.options_open());
    let menu = f.ctx.memory(|m| m.area_rect(Id::unique(("menu", "host-options")))).expect("the menu was laid out");
    assert!(menu.top() >= chrome::BAR_TOP + chrome::BAR_H, "clear of the top bar: {menu:?}");
    assert!(menu.bottom() <= 720.0, "{menu:?}");
}

#[test]
fn at_150_percent_in_german_every_settings_page_fits_the_width() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    fit_150(&mut f);
    // Every row of every section.
    f.l.settings_view.more = [true; 9];
    let id = f.hosts.list[0].id;
    for screen in [Screen::Home, Screen::Settings, Screen::HostSettings(id), Screen::Profiles, Screen::Controllers, Screen::Shortcuts] {
        f.l.go(screen);
        f.settle();
        assert_fits(&f, &format!("{screen:?}"), false);
        if matches!(screen, Screen::Settings | Screen::HostSettings(_)) {
            for section in SETTINGS_SECTIONS {
                if screen != Screen::Settings && (section == "App" || section == "Recommended") { continue; }
                f.l.settings_view.section = section;
                f.settle();
                assert_fits(&f, &format!("{screen:?} {section}"), false);
            }
            f.l.settings_view.query = "bitrate".into();
            f.settle();
            assert_fits(&f, &format!("{screen:?} searching"), false);
            f.l.settings_view.query = "zzz".into();
            f.settle();
            assert_fits(&f, &format!("{screen:?} finding nothing"), false);
            f.l.settings_view.query.clear();
        }
    }
    for dialog in [
        Dialog::ProfileName { id: None, name: String::new(), settings: Default::default(), error: None },
        Dialog::Shortcut { id: None, name: String::new(), chord: String::new(), error: Some("Use between 1 and 8 keys in each chord.".into()) },
        Dialog::DeleteProfile { id: 1, name: "Travel".into() },
        Dialog::DeleteShortcut { id: "a".into(), name: "Task manager".into() },
        Dialog::KeptSettings { pcs: vec![KeptPc { name: "Gaming PC".into(), own: true, games: 12 }] },
    ] {
        let name = format!("{:?}", std::mem::discriminant(&dialog));
        f.l.dialog = dialog;
        f.settle();
        assert_fits(&f, &format!("dialog {name}"), true);
        f.l.dialog = Dialog::None;
        f.settle();
    }
}

#[test]
fn at_150_percent_in_german_the_roaming_row_fits_with_each_choice() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    fit_150(&mut f);
    // The 1280-pixel width is what is checked; the height lets the network rows be drawn
    // without scrolling to them.
    f.size.y = 8000.0;
    f.l.settings_view.section = "Streaming";
    f.l.settings_view.more = [true; 9];
    let shown = |f: &Frames, text: &str| f.placed.iter().any(|(t, _, _)| t.contains(text));
    let id = f.hosts.list[0].id;
    for screen in [Screen::Settings, Screen::HostSettings(id)] {
        // One row with three choices; the longest German name must fit too. A PC's page shows
        // the values it opened with, the default.
        let choices: &[(bool, bool, &str)] = if screen == Screen::Settings {
            &[(true, false, "Außer mobilen Daten"), (true, true, "Jedes Netzwerk")]
        } else {
            &[(true, false, "Außer mobilen Daten")]
        };
        for &(roaming, metered, value) in choices {
            f.l.settings.client_roaming = roaming;
            f.l.settings.roaming_metered = metered;
            f.l.go(screen);
            f.settle();
            assert!(shown(&f, "Netzwerkwechsel"), "{screen:?}: the roaming row is drawn in German");
            assert!(shown(&f, value), "{screen:?}: {value}");
            assert!(!shown(&f, "Über mobile Daten fortsetzen"), "{screen:?}: no second row for mobile data");
            assert_fits(&f, &format!("{screen:?} with {value}"), false);
        }
    }
}

#[test]
fn the_theme_setting_changes_the_look_on_the_next_frame() {
    use crate::config::Theme;
    use crate::theme::{look, Look};
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    assert_eq!(look(), Look::Night, "Dark is the default");
    let night_page = f.ctx.global_style().visuals.panel_fill;
    f.l.settings.theme = Theme::System;
    f.settle();
    assert_eq!(look(), Look::Night, "System on a dark system");
    f.l.system_light = true;
    f.settle();
    assert_eq!(look(), Look::Day, "System on a light system follows it at once");
    assert!(!f.ctx.global_style().visuals.dark_mode, "egui's own widgets are light too");
    assert_ne!(f.ctx.global_style().visuals.panel_fill, night_page);
    f.l.settings.theme = Theme::Oled;
    f.settle();
    assert_eq!(look(), Look::Oled, "OLED black whatever the system says");
    assert_eq!(f.ctx.global_style().visuals.panel_fill, egui::Color32::BLACK);
    f.l.system_light = false;
    f.l.settings.theme = Theme::Light;
    f.settle();
    assert_eq!(look(), Look::Day, "Light stays the day look on a dark system");
    assert!(!f.ctx.global_style().visuals.dark_mode, "egui's own widgets are light too");
    f.l.settings.high_contrast = true;
    f.settle();
    assert_eq!(look(), Look::HighContrast, "high contrast wins over every theme");
    f.l.settings.high_contrast = false;
    f.l.settings.theme = Theme::Dark;
    f.settle();
    assert_eq!(look(), Look::Night);
    assert_eq!(f.ctx.global_style().visuals.panel_fill, night_page);
}

#[test]
fn reduced_motion_turns_off_animated_transitions_and_full_motion_keeps_them() {
    use crate::config::ReduceMotion;
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.l.settings.reduce_motion = ReduceMotion::Reduce;
    f.settle();
    assert_eq!(f.ctx.global_style().animation_time, 0.0, "fades and slides jump");
    assert_eq!(f.ctx.global_style().scroll_animation.points_per_second, f32::INFINITY, "scrolling jumps");
    f.l.settings.reduce_motion = ReduceMotion::Full;
    f.settle();
    assert!(f.ctx.global_style().animation_time > 0.0);
    // The system setting decides only on Follow system; tests answer it with full motion, whatever
    // the machine's own setting (`theme::system_reduces_motion`).
    f.l.settings.reduce_motion = ReduceMotion::System;
    f.settle();
    assert_eq!(f.ctx.global_style().animation_time == 0.0, crate::theme::system_reduces_motion());
}

#[test]
fn the_spinner_stands_still_and_asks_for_no_frames_when_motion_is_reduced() {
    for still in [true, false] {
        crate::theme::own_palette();
        let ctx = egui::Context::default();
        set_look(&ctx, false, still);
        // The first frames ask for repaints of their own (fonts, layout); look at a settled one.
        let mut delay = std::time::Duration::ZERO;
        for n in 0..6 {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(200.0, 200.0))), time: Some(1.0 + f64::from(n) * 0.1), ..Default::default() };
            let mut output = ctx.run_ui(input, |ui| spinner(ui, 10.0));
            output.textures_delta.clear();
            delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        }
        if still {
            assert!(delay > std::time::Duration::from_secs(1), "no repaint is requested: {delay:?}");
        } else {
            assert_eq!(delay, std::time::Duration::ZERO, "the spinning wheel asks for the next frame");
        }
    }
}

#[test]
fn the_high_contrast_setting_reaches_the_style() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.l.settings.high_contrast = true;
    f.settle();
    assert_eq!(f.ctx.global_style().visuals.panel_fill, Color32::BLACK);
    f.l.settings.high_contrast = false;
    f.settle();
    assert_ne!(f.ctx.global_style().visuals.panel_fill, Color32::BLACK);
}

#[test]
fn ctrl_equals_minus_and_zero_step_the_zoom_except_in_a_text_field() {
    let mut f = Frames::new(record("PC", true, true), Reach::Online);
    assert_eq!(f.l.settings.ui_zoom, 0, "new installs start on Auto");
    f.press_with(Key::Equals, egui::Modifiers::COMMAND);
    assert_eq!(f.l.settings.ui_zoom, 115, "zoom in steps from Auto's 100 %");
    f.press_with(Key::Equals, egui::Modifiers::COMMAND);
    f.press_with(Key::Equals, egui::Modifiers::COMMAND);
    f.press_with(Key::Equals, egui::Modifiers::COMMAND);
    f.press_with(Key::Equals, egui::Modifiers::COMMAND);
    f.press_with(Key::Equals, egui::Modifiers::COMMAND);
    assert_eq!(f.l.settings.ui_zoom, 250, "the top step is the last one");
    f.press_with(Key::Minus, egui::Modifiers::COMMAND);
    assert_eq!(f.l.settings.ui_zoom, 200);
    f.press_with(Key::Num0, egui::Modifiers::COMMAND);
    assert_eq!(f.l.settings.ui_zoom, 0, "Ctrl+0 returns to Auto");
    // The Rename dialog's name field has the keyboard.
    f.press(Key::F2);
    f.press(Key::ArrowDown);
    f.press(Key::ArrowDown);
    f.press(Key::Enter);
    assert!(matches!(f.l.dialog, Dialog::Rename { .. }));
    assert!(f.ctx.text_edit_focused(), "the name field is focused");
    f.press_with(Key::Equals, egui::Modifiers::COMMAND);
    assert_eq!(f.l.settings.ui_zoom, 0, "a text field keeps Ctrl+=");
}

#[test]
fn the_options_list_follows_the_pc_state() {
    let mut host = Frames::new(record("PC", false, false), Reach::Online).hosts.list.remove(0);
    assert_eq!(
        host_options(&host),
        [HostOption::Pair, HostOption::Settings, HostOption::Rename, HostOption::Details, HostOption::PairingHelp, HostOption::CheckAgain, HostOption::Remove]
    );
    host.record.server_certificate = Some("pem".into());
    host.reach = Reach::Offline;
    host.record.mac = Some("00:1a:2b:3c:4d:5e".into());
    assert_eq!(
        host_options(&host),
        [
            HostOption::OpenLibrary,
            HostOption::Wake,
            HostOption::Settings,
            HostOption::Rename,
            HostOption::Details,
            HostOption::CheckAgain,
            HostOption::Remove
        ]
    );
    host.record.mac = None;
    assert!(!host_options(&host).contains(&HostOption::Wake), "no MAC, no Wake");
    assert!(!host_options(&host).contains(&HostOption::Unpair), "an offline PC cannot be asked to unpair");
    host.reach = Reach::Online;
    assert!(host_options(&host).contains(&HostOption::Unpair));
    assert!(!host_options(&host).contains(&HostOption::PairingHelp), "the help is for a PC that is not paired");
}

#[test]
fn the_pairing_help_steps_read_in_the_chosen_language() {
    let mut f = Frames::new(record("Den", false, false), Reach::Online);
    f.l.settings.language = Language::German;
    f.l.ask(Dialog::PairingHelp);
    f.settle();
    assert!(shows(&f, "2. Wähle hier den PC im selben Netzwerk und tippe auf Koppeln."), "{:?}", f.drawn);
    assert!(!shows(&f, PAIRING_STEPS[1]), "{:?}", f.drawn);
}

#[test]
fn a_toast_shows_over_the_launcher_and_goes_away() {
    let mut f = Frames::new(record("Den", true, true), Reach::Online);
    f.l.toast(trf("Quitting {} on the PC…", &[&"Hades"]));
    f.settle();
    assert!(f.drawn.iter().any(|t| t == "Quitting Hades on the PC…"), "{:?}", f.drawn);
    assert!(f.press(Key::Escape).is_empty(), "the toast takes no input");
    f.l.settings.language = Language::German;
    f.settle();
    f.l.toast(trf("Quitting {} on the PC…", &[&"Hades"]));
    f.settle();
    assert!(f.drawn.iter().any(|t| t == "Hades wird auf dem PC beendet …"), "{:?}", f.drawn);
    f.l.toast = Some(("gone".into(), std::time::Instant::now() - std::time::Duration::from_secs(5)));
    f.settle();
    assert!(f.l.toast.is_none() && !f.drawn.iter().any(|t| t == "gone"), "{:?}", f.drawn);
}

/// A toast has no blur of its own, so the words of the page under it must not read through: glass
/// nearly as dense as the stream menu's, its shadow past the area, no fill of its own.
#[test]
fn a_toast_stands_on_glass_the_page_does_not_read_through() {
    let mut f = Frames::new(record("Den", true, true), Reach::Online);
    let screen = |f: &Frames| f.ctx.content_rect();
    f.l.toast("Saved".to_owned());
    f.settle();
    let whole = screen(&f);
    let (at, ..) = f.placed.iter().find(|(t, ..)| t == "Saved").map(|p| (p.1, ())).expect("the words");
    // Its area is still fading in, which takes a little off the menu fill's alpha of 240.
    let glass = meshes_drawn(&f).into_iter().find(|(mesh, _)| {
        let bounds = mesh.calc_bounds();
        bounds.contains_rect(at) && bounds.height() < 80.0 && mesh.vertices.iter().any(|v| v.color.a() >= 220)
    });
    let (_, clip) = glass.expect("glass the page does not read through");
    assert_eq!(clip, whole, "the toast's glass clips to the window, not its area");
    let filled = f.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Rect(r) if r.rect.contains_rect(at) && r.rect.height() < 80.0 && r.fill.a() > 0));
    assert!(!filled, "no fill of its own");
}

#[test]
fn waking_shows_the_pc_and_cancel_stops_it() {
    let mut f = Frames::new(record("Den", true, true), Reach::Offline);
    f.waking = Some("Den".into());
    f.settle();
    assert!(f.drawn.iter().any(|t| t == "Waking Den…"), "{:?}", f.drawn);
    assert!(f.press(Key::Escape).is_empty(), "Back does not leave the PC half woken");
    assert!(matches!(f.press(Key::Enter)[..], [Command::CancelWake]), "Cancel has focus");
    f.l.settings.language = Language::German;
    f.settle();
    assert!(f.drawn.iter().any(|t| t == "Den wird aufgeweckt…"), "{:?}", f.drawn);
}

#[test]
fn the_open_play_dialog_keeps_its_cancel_after_the_countdown() {
    let mut f = Frames::new(record("Den", true, true), Reach::Online);
    f.l.ask(Dialog::OpenPlay { game: "Hades".into(), pc: "Den".into(), left: 0 });
    f.settle();
    assert!(f.drawn.iter().any(|t| t == "Starting on Den"), "{:?}", f.drawn);
    assert!(f.press(Key::Enter).is_empty());
    assert!(f.no_dialog(), "Cancel has focus");
}

#[test]
fn the_open_play_countdown_names_the_game_and_the_pc_and_stops_at_cancel_or_back() {
    let mut f = Frames::new(record("Den", true, true), Reach::Online);
    let countdown = || Dialog::OpenPlay { game: "Hades".into(), pc: "Den".into(), left: 3 };
    f.l.ask(countdown());
    f.settle();
    assert!(f.drawn.iter().any(|t| t == "Hades") && f.drawn.iter().any(|t| t == "Starting on Den in 3"), "{:?}", f.drawn);
    assert!(f.press(Key::Enter).is_empty());
    assert!(f.no_dialog(), "Cancel has focus");
    f.l.ask(countdown());
    f.settle();
    f.press(Key::Escape);
    assert!(f.no_dialog(), "Escape, as B and Back are");
    f.l.settings.language = Language::German;
    f.l.ask(countdown());
    f.settle();
    assert!(f.drawn.iter().any(|t| t == "Startet auf Den in 3"), "{:?}", f.drawn);
}

#[test]
fn wake_is_offered_until_the_pc_answers_and_resume_while_it_runs_a_game() {
    let states = every_state();
    let by = |name: &str| states.iter().find(|s| s.name == name).unwrap().frames().hosts.list.remove(0);
    let asleep = host_options(&by("not reached yet, can wake"));
    assert!(asleep.contains(&HostOption::Wake), "a PC not reached yet with a MAC");
    let mut no_mac = by("not reached yet, can wake");
    no_mac.record.mac = None;
    assert!(!host_options(&no_mac).contains(&HostOption::Wake));
    assert!(!host_options(&by("paired")).contains(&HostOption::Wake), "a PC that answers is awake");

    let busy = host_options(&by("running a game"));
    let at = |option| busy.iter().position(|o| *o == option).unwrap();
    assert!(at(HostOption::OpenLibrary) < at(HostOption::Resume) && at(HostOption::Resume) < at(HostOption::QuitApp));
    assert!(!host_options(&by("paired")).contains(&HostOption::Resume));
    // Without its library the running game has no name, and it resumes by its number all the same.
    let mut unlisted = by("running a game");
    unlisted.apps = Apps::NotLoaded;
    let options = host_options(&unlisted);
    assert!(options.contains(&HostOption::Resume) && options.contains(&HostOption::QuitApp));
}

#[test]
fn quick_resume_goes_back_into_the_running_game_and_otherwise_opens_the_library() {
    let states = every_state();
    let by = |name: &str| states.iter().find(|s| s.name == name).unwrap().frames().hosts.list.remove(0);
    let clicked = |host: &Host, quick_resume| {
        let mut commands = Vec::new();
        host_clicked(host, quick_resume, &mut commands);
        commands
    };
    let busy = by("running a game");
    assert!(matches!(clicked(&busy, true)[..], [Command::Launch { app: 7, resume: true, .. }]));
    assert!(matches!(clicked(&busy, false)[..], [Command::OpenApps(_)]), "off: the library, as before");
    assert!(matches!(clicked(&by("paired"), true)[..], [Command::OpenApps(_)]), "nothing running");
    let mut unlisted = by("running a game");
    unlisted.apps = Apps::NotLoaded;
    assert!(matches!(clicked(&unlisted, true)[..], [Command::OpenApps(_)]), "an unnamed game opens the library to load it");
}
