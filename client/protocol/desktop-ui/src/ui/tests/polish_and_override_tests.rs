//! Polish screenshots, store grouping, and inherited setting override tests.

use super::*;

#[test]
fn a_menu_stays_inside_the_window_flipping_where_it_would_not_fit() {
    let screen = Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 720.0));
    let size = vec2(280.0, 300.0);
    // Under its dots, right edges lined up.
    let dots = Rect::from_min_size(Pos2::new(600.0, 100.0), vec2(30.0, 30.0));
    let at = place(Anchor::Below(dots), size, screen);
    assert_eq!((at.x + size.x, at.y), (dots.right(), dots.bottom() + 6.0));
    // Above them near the bottom.
    let low = dots.translate(vec2(0.0, 560.0));
    assert_eq!(place(Anchor::Below(low), size, screen).y, low.top() - 6.0 - size.y);
    // Beside a card, on its left near the right edge.
    let card = Rect::from_min_size(Pos2::new(1100.0, 100.0), vec2(170.0, 250.0));
    assert_eq!(place(Anchor::Beside(card), size, screen).x, card.left() - 6.0 - size.x);
    // At the pointer, flipped up and left in the corner, never off screen.
    let at = place(Anchor::Point(Pos2::new(1270.0, 710.0)), size, screen);
    assert!(screen.contains_rect(Rect::from_min_size(at, size)), "{at:?}");
    // Taller than the window: from its top margin.
    assert_eq!(place(Anchor::Point(Pos2::new(10.0, 10.0)), vec2(280.0, 900.0), screen).y, 8.0);
}

/// On a PC's library page, F2 (which the controller's X sends, see `nav_key` in app/launcher.rs) or Shift+F10 on a
/// game opens its menu with Play (Resume for the running game) and Game settings in it; it no
/// longer jumps straight to the settings.
#[test]
fn the_library_page_opens_a_games_menu_with_play_and_its_settings() {
    let mut f = Frames::with(record("Desk", true, true), Reach::Online, Some(answer(true, 7)), vec![game(7), game(9)]);
    let id = f.hosts.list[0].id;
    f.l.open(Screen::Apps(id));
    f.settle();
    let first = f.ctx.memory(|m| m.focused()).expect("the running game has focus on arrival");
    let shift = egui::Modifiers { shift: true, ..Default::default() };
    for (keys, chord) in [("F2", None), ("Shift+F10", Some(shift))] {
        let commands = match chord {
            None => f.press(Key::F2),
            Some(modifiers) => f.press_with(Key::F10, modifiers),
        };
        assert!(!commands.iter().any(|c| matches!(c, Command::AppSettings { .. })), "{keys} does not jump to the settings");
        assert!(matches!(f.l.dialog, Dialog::GameMenu { game: GameTarget::Classic { app: 7 }, host } if host == id), "{keys} opens the focused game's menu");
        assert!(shows(&f, "Resume") && !shows(&f, "Play"), "{keys}: the running game resumes: {:?}", f.drawn);
        assert!(shows(&f, "Game settings"), "{keys}: {:?}", f.drawn);
        f.press(Key::Escape);
        assert!(f.no_dialog());
        assert_eq!(f.ctx.memory(|m| m.focused()), Some(first), "{keys}: focus is back on the game");
    }
    // The game that does not run offers Play.
    f.press(Key::ArrowRight);
    assert_ne!(f.ctx.memory(|m| m.focused()), Some(first), "Right moves to the other game");
    f.press(Key::F2);
    assert!(matches!(f.l.dialog, Dialog::GameMenu { game: GameTarget::Classic { app: 9 }, .. }), "the other game's menu");
    assert!(shows(&f, "Play") && shows(&f, "Game settings") && shows(&f, "Open this when Nightjar starts"), "{:?}", f.drawn);
}

/// Remove from favourites in a Favourites card's menu moves the focus to the next favourite, as
/// Delete on the card does; the card it was on is gone, so nothing would hold the focus.
#[test]
fn unstarring_a_favourite_from_its_menu_focuses_the_next_one() {
    let mut f = home_with_games(3);
    let id = f.hosts.list[0].id;
    let star = |n: u32| crate::config::Favourite { host: "Desk".into(), install_id: format!("install-{n}"), app_id: 1000 + n, title: format!("Game {n:02}") };
    f.l.favourites = vec![star(0), star(1)];
    f.l.restore_key = Some(format!("favourite:{id}:1000"));
    f.settle();
    assert_eq!(f.l.focused_key, Some(format!("favourite:{id}:1000")), "the first favourite has focus");
    f.press(Key::F2);
    assert!(matches!(f.l.dialog, Dialog::GameMenu { game: GameTarget::Library { app: 1000, favourite: true }, .. }));
    assert!(shows(&f, "Remove from favourites"), "{:?}", f.drawn);
    // Down to the second row, Remove from favourites, as a controller does.
    f.press(Key::ArrowDown);
    // The app applies the command before the next frame, as here.
    let commands = f.frame(tap_key(Key::Enter).to_vec());
    let unstarred: Vec<_> = commands.iter().filter_map(|c| if let Command::ToggleFavourite(star) = c { Some(star.clone()) } else { None }).collect();
    assert_eq!(unstarred, vec![star(0)]);
    crate::config::toggle_favourite(&mut f.l.favourites, star(0));
    f.settle();
    assert!(f.no_dialog());
    assert_eq!(f.l.focused_key, Some(format!("favourite:{id}:1001")), "the next favourite has focus");
    assert!(f.ctx.memory(|m| m.focused()).is_some(), "something holds the focus");
}

/// A favourite whose PC is away with no library saved still has its menu, from the stored star:
/// it can be unstarred, and its settings open once the PC's app list is known.
#[test]
fn a_favourite_whose_library_is_not_known_still_has_a_menu() {
    let app = nightjar_policy::http::App { uuid: "install-9".into(), ..game(9) };
    let mut f = Frames::with(record("Desk", true, true), Reach::Offline, None, vec![app]);
    let id = f.hosts.list[0].id;
    let star = crate::config::Favourite { host: "Desk".into(), install_id: "install-9".into(), app_id: 9, title: "Saved".into() };
    f.l.favourites = vec![star.clone()];
    f.l.go(Screen::Home);
    f.l.restore_key = Some(format!("favourite:{id}:9"));
    f.settle();
    assert!(shows(&f, "Saved"), "the card shows the stored title: {:?}", f.drawn);
    assert_eq!(f.l.focused_key, Some(format!("favourite:{id}:9")));
    f.press(Key::F2);
    assert!(matches!(f.l.dialog, Dialog::GameMenu { game: GameTarget::Library { app: 9, favourite: true }, .. }), "F2 opens its menu");
    for row in ["Game details", "Remove from favourites", "Game settings"] {
        assert!(shows(&f, row), "{row}: {:?}", f.drawn);
    }
    let row = f.placed.iter().find(|(t, ..)| t == "Remove from favourites").map(|p| p.1).unwrap();
    let commands = f.click_at(row.center(), egui::PointerButton::Primary);
    assert!(commands.iter().any(|c| matches!(c, Command::ToggleFavourite(s) if *s == star)), "the stored star comes off");
}

/// Settings in the top bar opens over a PC's page as Start does: Back returns to the page, and
/// the trail to it is kept.
#[test]
fn the_settings_destination_opens_over_a_page_and_back_returns_to_it() {
    let mut f = Frames::with(record("Desk", true, true), Reach::Online, Some(answer(true, 0)), vec![game(7)]);
    let id = f.hosts.list[0].id;
    f.l.open(Screen::Apps(id));
    f.settle();
    f.click_at(destination_at(&f, "Settings").center(), egui::PointerButton::Primary);
    assert_eq!(f.l.screen, Screen::Settings);
    f.press(Key::Escape);
    assert_eq!(f.l.screen, Screen::Apps(id), "Back returns to the PC's games");
    f.press(Key::Escape);
    assert_eq!(f.l.screen, Screen::Hosts, "and from there to PCs");
}

/// PCs in the top bar would leave a Files page whose upload runs, which cancels it: it asks
/// first. Stay keeps the page and the upload; Leave goes. Settings opens over the page and keeps
/// the upload without asking.
#[test]
fn leaving_a_files_page_keeps_its_transfers_and_a_button_says_so() {
    let (downloads, here) = (files::tests::Folder::new("top-bar"), files::tests::Folder::new("top-bar-here"));
    let mut f = Frames::with(record("Desk", true, true), Reach::Online, Some(files::tests::native_answer()), Vec::new());
    let id = f.hosts.list[0].id;
    f.l.files.page = Some(files::tests::sending(id, &downloads, &here));
    f.l.open(Screen::Files(id));
    f.settle();
    assert!(f.l.files.transferring());
    assert!(!shows(&f, "File transfers running: 1"), "the page itself shows them");
    // PCs leaves without asking: the upload goes on, and a button over the launcher says so.
    f.click_at(destination_at(&f, "PCs").center(), egui::PointerButton::Primary);
    assert!(f.no_dialog(), "nothing to cancel");
    assert_eq!(f.l.screen, Screen::Hosts);
    f.settle();
    assert!(f.l.files.transferring(), "the upload goes on");
    assert_eq!(f.l.files.running_away(None), Some((id, files::Away::Running(1))));
    assert!(shows(&f, "File transfers running: 1"), "{:?}", f.drawn);
}

/// Hover lightens a card one step and draws no outline; focus adds a single amber ring outside it
/// and the card keeps its size and place.
#[test]
fn hover_lightens_a_card_and_focus_adds_one_ring_without_resizing() {
    let mut f = Frames::new(record("Desk", true, true), Reach::Online);
    let (action, _) = f.card_and_dots();
    let card = pc_card_rect(&f);
    let card_id = Id::unique(("pc-card", f.hosts.list[0].id));
    let rings = |f: &Frames| -> Vec<Rect> { meshes_holding(&f.shapes, design_tokens::FOCUS_RING_COLOR).into_iter().filter(|r| r.intersects(card)).collect() };
    // Focused (the first action has focus on arrival): one ring around it, with a gap.
    let around = rings(&f);
    assert_eq!(around.len(), 1, "one ring: {around:?}");
    assert!(around[0].contains_rect(action) && around[0] != action, "outside the action: {around:?} around {action:?}");
    // Moved off and hovered: lighter, no outline of any colour, same rect.
    let rest = meshes_holding(&f.shapes, design_tokens::HOVER_FILL_LIFT).len();
    f.ctx.memory_mut(|m| m.surrender_focus(m.focused().unwrap()));
    hover_card(&mut f, card);
    assert!(rings(&f).is_empty(), "no outline on hover: {:?}", rings(&f));
    assert!(meshes_holding(&f.shapes, design_tokens::HOVER_FILL_LIFT).len() > rest, "the hovered card is one step lighter");
    assert_eq!(f.ctx.read_response(card_id).unwrap().rect, card, "the card did not move or grow");
}

#[test]
fn a_controller_press_with_nothing_focused_focuses_the_first_item() {
    let mut f = Frames::new(record("Desk", true, false), Reach::Online);
    let first = f.ctx.memory(|m| m.focused()).expect("the first card has focus on arrival");
    for key in [Key::ArrowDown, Key::ArrowUp, Key::ArrowRight, Key::Enter] {
        drop_focus(&mut f);
        // egui alone moves nowhere from no focus: the press was lost.
        f.press(Key::ArrowDown);
        assert_eq!(f.ctx.memory(|m| m.focused()), None);
        pad_press(&mut f, key);
        assert_eq!(f.ctx.memory(|m| m.focused()), Some(first), "{key:?} focuses the first card");
        assert!(f.no_dialog(), "{key:?}: A only focuses, it does not open the card");
    }
    // With focus, the D-pad moves it as before.
    pad_press(&mut f, Key::ArrowRight);
    assert_ne!(f.ctx.memory(|m| m.focused()), Some(first), "Right moves on to the card's options");

    // Settings: the first row.
    let mut s = Frames::new(record("Desk", true, false), Reach::Online);
    s.l.go(Screen::Settings);
    s.settle();
    let row = s.ctx.memory(|m| m.focused()).expect("the first row has focus on arrival");
    drop_focus(&mut s);
    pad_press(&mut s, Key::ArrowDown);
    assert_eq!(s.ctx.memory(|m| m.focused()), Some(row));

    // A dialog: its first control (Cancel on a confirmation).
    let id = f.hosts.list[0].id;
    f.l.ask(Dialog::ConfirmRemove { host: id, name: "Desk".into() });
    f.settle();
    let cancel = f.ctx.memory(|m| m.focused()).expect("Cancel has focus");
    drop_focus(&mut f);
    pad_press(&mut f, Key::ArrowUp);
    assert_eq!(f.ctx.memory(|m| m.focused()), Some(cancel));
    assert!(matches!(f.l.dialog, Dialog::ConfirmRemove { .. }));
}

#[test]
fn b_where_back_ends_asks_to_close_nightjar_with_focus_on_close_nightjar() {
    // The PCs are the root here (no Home): B there asks.
    let mut f = Frames::new(record("Desk", true, false), Reach::Online);
    let focused_text = |f: &Frames| {
        let rect = f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect);
        f.placed.iter().find(|(_, at, _)| rect.is_some_and(|r| r.contains(at.center()))).map(|p| p.0.clone())
    };
    // Both keyboard Escape and controller B ask to close Nightjar on the root screen,
    // and pressing Escape again dismisses the dialog without quitting.
    assert!(f.press(Key::Escape).is_empty());
    assert!(matches!(f.l.dialog, Dialog::ConfirmExit) && shows(&f, "Close Nightjar?"), "{:?}", f.drawn);
    assert!(f.press(Key::Escape).iter().all(|c| !matches!(c, Command::Quit)));
    assert!(f.no_dialog(), "a second Escape closes the question");
    pad_press(&mut f, Key::Escape);
    assert!(matches!(f.l.dialog, Dialog::ConfirmExit) && shows(&f, "Close Nightjar?"), "{:?}", f.drawn);
    assert_eq!(focused_text(&f).as_deref(), Some("Close Nightjar"));
    // A right away closes Nightjar.
    assert!(f.press(Key::Enter).iter().any(|c| matches!(c, Command::Quit)));
    assert!(f.no_dialog());
    // B keeps it open.
    pad_press(&mut f, Key::Escape);
    assert!(f.press(Key::Escape).iter().all(|c| !matches!(c, Command::Quit)));
    assert!(f.no_dialog(), "B closes the question");
    // Left to Cancel and A: it stays open.
    pad_press(&mut f, Key::Escape);
    pad_press(&mut f, Key::ArrowLeft);
    assert_eq!(focused_text(&f).as_deref(), Some("Cancel"));
    assert!(f.press(Key::Enter).iter().all(|c| !matches!(c, Command::Quit)));
    assert!(f.no_dialog());
}

#[test]
fn b_offers_this_pcs_power_each_asked_once_more_with_focus_on_close_nightjar() {
    use crate::power::{windows_offers, PowerAction};
    let mut f = Frames::new(record("Desk", true, false), Reach::Online);
    f.l.power = windows_offers(true);
    let focused_text = |f: &Frames| {
        let rect = f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect);
        f.placed.iter().find(|(_, at, _)| rect.is_some_and(|r| r.contains(at.center()))).map(|p| p.0.clone())
    };
    // To `label` by the pad's left from Close Nightjar at the right, or right from Cancel at the
    // left in the power question that follows.
    let walk = |f: &mut Frames, label: &str, key: Key| {
        for _ in 0..6 {
            if focused_text(f).as_deref() == Some(label) { break; }
            pad_press(f, key);
        }
        assert_eq!(focused_text(f).as_deref(), Some(label), "{:?}", f.drawn);
    };
    let walk_to = |f: &mut Frames, label: &str| walk(f, label, Key::ArrowLeft);
    let walk_right = |f: &mut Frames, label: &str| walk(f, label, Key::ArrowRight);
    pad_press(&mut f, Key::Escape);
    assert!(matches!(f.l.dialog, Dialog::ConfirmExit));
    for label in ["Shut down", "Restart", "Sleep", "Close Nightjar", "Cancel"] {
        assert!(shows(&f, label), "{label}: {:?}", f.drawn);
    }
    for (label, action) in [("Shut down", PowerAction::ShutDown), ("Restart", PowerAction::Restart), ("Sleep", PowerAction::Sleep)] {
        if !matches!(f.l.dialog, Dialog::ConfirmExit) { pad_press(&mut f, Key::Escape); }
        assert_eq!(focused_text(&f).as_deref(), Some("Close Nightjar"));
        walk_to(&mut f, label);
        // Choosing it only asks.
        assert!(f.press(Key::Enter).iter().all(|c| !matches!(c, Command::Power(_))), "{label}");
        assert!(matches!(f.l.dialog, Dialog::ConfirmPower(o) if o.action == action), "{label}");
        assert!(shows(&f, action.question()), "{:?}", f.drawn);
        assert_eq!(focused_text(&f).as_deref(), Some("Cancel"), "{label}: focus starts on Cancel");
        // A right away, or B, does nothing.
        assert!(f.press(Key::Enter).iter().all(|c| !matches!(c, Command::Power(_))));
        assert!(f.no_dialog());
    }
    // Over to the action itself and A: now it is asked for.
    pad_press(&mut f, Key::Escape);
    walk_to(&mut f, "Restart");
    f.press(Key::Enter);
    assert!(matches!(f.l.dialog, Dialog::ConfirmPower(o) if o.action == PowerAction::Restart));
    pad_press(&mut f, Key::Escape);
    assert!(f.no_dialog(), "B cancels the second question");
    pad_press(&mut f, Key::Escape);
    walk_to(&mut f, "Restart");
    f.press(Key::Enter);
    walk_right(&mut f, "Restart");
    let commands = f.press(Key::Enter);
    assert!(commands.iter().any(|c| matches!(c, Command::Power(o) if o.action == PowerAction::Restart)));
    assert!(commands.iter().all(|c| !matches!(c, Command::Quit)), "Nightjar is left to the system to close");
    assert!(f.no_dialog());
}

#[test]
fn the_close_question_offers_only_what_this_pcs_power_allows() {
    let mut f = Frames::new(record("Desk", true, false), Reach::Online);
    // Not known yet (or nothing allowed): the question is as before.
    pad_press(&mut f, Key::Escape);
    assert!(matches!(f.l.dialog, Dialog::ConfirmExit));
    assert!(["Shut down", "Restart", "Sleep"].iter().all(|l| !shows(&f, l)), "{:?}", f.drawn);
    f.press(Key::Escape);
    // A Modern Standby PC: no Sleep.
    f.l.power = crate::power::windows_offers(false);
    pad_press(&mut f, Key::Escape);
    assert!(shows(&f, "Shut down") && shows(&f, "Restart") && !shows(&f, "Sleep"), "{:?}", f.drawn);
}

/// Settings → Stream hotkeys with one hotkey changed and one off, and the dialog that records a new chord
/// refusing one another hotkey has: `NIGHTJAR_SHOTS=<dir> cargo test -p nightjar-desktop stream_hotkeys_screenshots -- --ignored`.
#[test]
#[ignore = "writes PNGs; set NIGHTJAR_SHOTS"]
fn stream_hotkeys_screenshots() {
    let Some(dir) = std::env::var_os("NIGHTJAR_SHOTS").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&dir).expect("the folder for the screenshots");
    crate::shortcuts::record_hotkey("overlay", &["CTRL".into(), "F2".into()]).expect("a free chord");
    crate::shortcuts::turn_off_hotkey("minimize").expect("not the last way out");
    for high in [false, true] {
        let look = if high { "high-contrast" } else { "normal" };
        crate::theme::own_palette();
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut l = launcher(false, false);
        l.settings.high_contrast = high;
        l.go(Screen::StreamHotkeys);
        let mut f = Frames::over(ctx, l, Hosts::without_discovery(Vec::new(), Arc::new(|| {})));
        f.size = vec2(1280.0, 800.0);
        f.native_ppp = 1.5;
        f.settle();
        f.shoot(&dir, &format!("stream-hotkeys-{look}-desktop"));
        let taken = crate::shortcuts::record_hotkey("capture", &["CTRL".into(), "ALT".into(), "SHIFT".into(), "Q".into()])
            .expect_err("Disconnect has it");
        f.l.dialog = Dialog::StreamHotkey { id: "capture", error: Some(taken) };
        f.settle();
        f.shoot(&dir, &format!("stream-hotkey-taken-{look}-desktop"));
    }
}

#[test]
fn the_latency_benchmark_page_starts_stops_and_shows_its_results() {
    let mut f = latency_benchmark_frames(false);
    f.settle();
    for text in ["Start 10-second test", "Copy results", "Reported panel refresh: 144.00 Hz", "Reported audio: 48000 Hz · 480 frames per buffer", "Frame callback interval", "Timer wakeup delay", "Decoder benchmark", "H.264", "AV1"] {
        assert!(f.drawn.iter().any(|t| t == text), "{text}: {:?}", f.drawn);
    }
    assert_eq!(f.drawn.iter().filter(|t| *t == "no data yet").count(), 7, "four metrics and three codecs before a test");
    let at = |f: &Frames, text: &str| f.placed.iter().find(|(t, ..)| t == text).unwrap_or_else(|| panic!("{text}")).1.center();
    let commands = f.click_at(at(&f, "Start 10-second test"), egui::PointerButton::Primary);
    assert!(commands.iter().any(|c| matches!(c, Command::StartLatencyBenchmark)));

    f.l.latency_bench.start_stub();
    f.settle();
    assert!(f.drawn.iter().any(|t| t == "Stop test"));
    assert_eq!(f.drawn.iter().filter(|t| *t == "Testing…").count(), 3, "the codecs wait for their turn");
    let commands = f.click_at(at(&f, "Stop test"), egui::PointerButton::Primary);
    assert!(commands.iter().any(|c| matches!(c, Command::StopLatencyBenchmark)));
    f.l.latency_bench.finish();

    f.l.latency_bench = crate::latency_bench::Bench::fixture();
    f.settle();
    for text in ["6.94 / 7.12 / 7.61 (n=1439)", "0.52 / 0.98 / 1.21 (n=99)", "1.18 ms · D3D11VA", "1.04 ms · D3D11VA", "Not supported"] {
        assert!(f.drawn.iter().any(|t| t == text), "{text}: {:?}", f.drawn);
    }
    f.click_at(at(&f, "Copy results"), egui::PointerButton::Primary);
    assert!(f.drawn.iter().any(|t| t == "Results copied"), "a toast says so");
}

/// Settings → Latency benchmark before a test and with results:
/// `NIGHTJAR_SHOTS=<dir> cargo test -p nightjar-desktop --bin nightjar latency_benchmark_screenshots -- --ignored`.
#[test]
#[ignore = "writes PNGs; set NIGHTJAR_SHOTS"]
fn latency_benchmark_screenshots() {
    let Some(dir) = std::env::var_os("NIGHTJAR_SHOTS").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&dir).expect("the folder for the screenshots");
    for high in [false, true] {
        let look = if high { "high-contrast" } else { "normal" };
        let mut f = latency_benchmark_frames(high);
        f.native_ppp = 1.5;
        f.settle();
        f.shoot(&dir, &format!("latency-benchmark-{look}-desktop"));
        f.l.latency_bench = crate::latency_bench::Bench::fixture();
        f.settle();
        f.shoot(&dir, &format!("latency-benchmark-results-{look}-desktop"));
    }
}

#[test]
fn a_link_from_outside_pairs_only_when_pair_is_chosen() {
    let mut f = Frames::new(record("Desk", true, false), Reach::Online);
    let link = opened_link("Wohnzimmer");
    let uri = link.to_uri();
    f.l.open_link(&uri);
    let commands = f.settle();
    assert!(!pairs(&commands), "opening it asks, nothing more");
    assert_eq!(f.hosts.list.len(), 1);
    assert!(f.drawn.iter().any(|t| t.contains("Wohnzimmer")), "names the PC");
    assert!(f.drawn.iter().any(|t| t == "Address: 192.168.4.2:47989"), "and its address, on a line that says so");
    assert!(!f.drawn.iter().any(|t| t.contains("4821") || t.contains("hunter2")), "never the secrets");
    assert!(!f.l.can_open_link(), "the next link waits behind it");
    // Cancel has focus: Enter, and the controller's A and B, leave the PC out.
    assert!(!pairs(&f.press(Key::Enter)));
    assert!(f.no_dialog());
    f.l.open_link(&uri);
    f.settle();
    assert!(!pairs(&f.press(Key::Escape)));
    assert!(f.no_dialog());
    assert!(f.l.can_open_link());
    // The controller's way to Pair: right, then A.
    f.l.open_link(&uri);
    f.settle();
    assert!(f.ctx.memory(|m| m.focused()).is_some(), "a control has focus");
    f.time += 1.0;
    f.settle();
    pad_press(&mut f, Key::ArrowRight);
    let events = f.l.controller_key(&f.ctx, Key::Enter);
    let commands = f.frame(events);
    let chosen = commands.iter().find_map(|c| if let Command::PairLink(chosen) = c { Some(chosen) } else { None });
    assert_eq!(chosen.map(|chosen| **chosen == link), Some(true), "the link as it was opened");
    f.settle();
    assert!(f.no_dialog());
}

#[test]
fn an_invite_from_outside_is_opened_only_when_join_is_chosen() {
    let mut f = Frames::new(record("Desk", true, false), Reach::Online);
    let link = invite("Den PC", 4_000_000_000);
    f.l.open_link(&link.to_uri());
    let commands = f.settle();
    assert!(!redeems(&commands), "opening it asks, nothing more");
    assert!(matches!(f.l.dialog, Dialog::ConfirmInvite { .. }));
    assert!(
        f.placed.iter().any(|(t, ..)| t.contains("Den PC")),
        "the PC is named"
    );
    let join = f
        .placed
        .iter()
        .find(|(t, ..)| t == tr("Join"))
        .expect("Join is drawn")
        .1
        .center();
    assert!(
        !redeems(&f.click_at(join, egui::PointerButton::Primary)),
        "not in its first second"
    );
    f.time += 1.0;
    f.settle();
    let commands = f.click_at(join, egui::PointerButton::Primary);
    let chosen = commands.iter().find_map(|c| {
        if let Command::RedeemInvite(chosen) = c {
            Some(chosen)
        } else {
            None
        }
    });
    assert_eq!(chosen.map(|chosen| **chosen == link), Some(true));
    assert!(f.no_dialog());
}

#[test]
fn an_expired_invite_says_so_and_offers_no_join() {
    let mut f = Frames::new(record("Desk", true, false), Reach::Online);
    f.l.open_link(&invite("Den PC", 1).to_uri());
    f.settle();
    f.time += 2.0;
    f.settle();
    assert!(f
        .placed
        .iter()
        .any(|(t, ..)| t == tr("This invite has expired. Ask for a new one.")));
    assert!(!f.placed.iter().any(|(t, ..)| t == tr("Join")));
    f.l.dialog = Dialog::None;
    f.l.open_link("rubylight://invite#v=2");
    assert!(
        matches!(f.l.dialog, Dialog::Message { .. }),
        "a broken invite is refused, never asked about"
    );
}

#[test]
fn pair_only_works_once_the_question_has_been_there_a_second() {
    let mut f = Frames::new(record("Desk", true, false), Reach::Online);
    f.l.open_link(&opened_link("Wohnzimmer").to_uri());
    f.settle();
    let tr_pair = tr("Pair");
    let pair = f.placed.iter().find(|(t, ..)| t == tr_pair).expect("Pair is drawn").1.center();
    // The controller cannot reach it yet: focus stays on Cancel.
    let cancel_focus = f.ctx.memory(|m| m.focused());
    assert!(cancel_focus.is_some());
    pad_press(&mut f, Key::ArrowRight);
    assert_eq!(f.ctx.memory(|m| m.focused()), cancel_focus);
    // A click meant for what was there before lands on it, and the question stays.
    assert!(!pairs(&f.click_at(pair, egui::PointerButton::Primary)));
    assert!(matches!(f.l.dialog, Dialog::ConfirmLink { .. }));
    f.time += 1.0;
    f.settle();
    assert!(pairs(&f.click_at(pair, egui::PointerButton::Primary)));
    assert!(f.no_dialog());
    // The next question starts its own wait.
    f.l.open_link(&opened_link("Studio").to_uri());
    f.settle();
    assert!(!pairs(&f.click_at(pair, egui::PointerButton::Primary)));
}

#[test]
fn a_link_that_hides_its_name_is_shown_plainly() {
    let mut f = Frames::new(record("Desk", true, false), Reach::Online);
    f.l.open_link(&opened_link("Desk\u{202e}cod\n").to_uri());
    f.settle();
    assert!(f.drawn.iter().any(|t| t.contains("Deskcod")), "{:?}", f.drawn);
    assert!(!f.drawn.iter().any(|t| t.contains('\u{202e}')));
    // No name left: the PC stands in for it, and the address still says which.
    f.press(Key::Escape);
    f.l.open_link(&opened_link("\u{202e}").to_uri());
    f.settle();
    assert!(f.drawn.iter().any(|t| t.contains(tr("the PC"))), "{:?}", f.drawn);
    assert!(f.drawn.iter().any(|t| t == "Address: 192.168.4.2:47989"));
}

#[test]
fn a_link_waits_behind_a_dialog_and_the_pin_screen() {
    let mut f = Frames::new(record("Desk", true, false), Reach::Online);
    assert!(f.l.can_open_link());
    let id = f.hosts.list[0].id;
    f.l.ask(Dialog::ConfirmRemove { host: id, name: "Desk".into() });
    assert!(!f.l.can_open_link(), "not over a question already open");
    f.l.dialog = Dialog::None;
    let dir = std::env::temp_dir().join(format!("nightjar-link-pin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = crate::pin_ui::open_store(&dir);
    store.set_pin(b"1234").unwrap();
    store.set_mode(crate::pin_lock::Mode::App);
    store.flush();
    f.l.pin = Some(crate::pin_ui::PinLock::with_store(store));
    f.settle();
    assert!(!f.l.can_open_link(), "not past the PIN screen");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bad_link_from_outside_is_refused_without_repeating_it() {
    let good = opened_link("Desk").to_uri();
    let wrong_version = good.replacen("v=1", "v=2", 1);
    let hostile = format!("{good}&x={}", "a".repeat(32 * 1024 * 1024));
    for bad in ["", "rubylight:", "RUBYLIGHT://PAIR?v=1", "rubylight://other?otp=4821", wrong_version.as_str(), hostile.as_str()] {
        let mut f = Frames::new(record("Desk", true, false), Reach::Online);
        f.l.open_link(bad);
        let commands = f.settle();
        assert!(!pairs(&commands));
        assert!(matches!(&f.l.dialog, Dialog::Message { body, .. } if body == tr(crate::hosts::LINK_INVALID)), "{}", &bad[..bad.len().min(40)]);
        assert!(!f.drawn.iter().any(|t| t.contains("4821")));
    }
}

#[test]
fn only_what_cant_be_taken_back_is_red() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    let host = f.hosts.list[0].id;
    // A dialog is still fading in on the last frame: red by its hue, not its exact value.
    let red = |f: &Frames| {
        fn walk(shape: &egui::Shape, out: &mut usize) {
            match shape {
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
                egui::Shape::Mesh(mesh) if mesh.vertices.iter().any(|v| v.color.r() > 0x60 && v.color.g() < v.color.r() / 3 && v.color.b() < v.color.r() / 3 && v.color.a() > 0x80) => *out += 1,
                _ => {}
            }
        }
        let mut count = 0;
        f.shapes.iter().for_each(|c| walk(&c.shape, &mut count));
        count
    };
    for dialog in [
        Dialog::ConfirmRemove { host, name: "PC".into() },
        Dialog::ConfirmUnpair { host, name: "PC".into() },
        Dialog::ConfirmClose { host, name: "Racing Game".into() },
    ] {
        f.l.dialog = dialog;
        f.settle();
        assert_eq!(red(&f), 1, "the one red control");
    }
    f.l.dialog = Dialog::Rename { host, text: "PC".into() };
    f.settle();
    assert_eq!(red(&f), 0, "renaming costs nothing");
}

#[test]
fn a_dialog_stands_on_the_panels_glass_and_not_a_flat_card() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    let host = f.hosts.list[0].id;
    f.l.dialog = Dialog::Rename { host, text: "PC".into() };
    f.settle();
    let flat = rects_filled(&f, *SURFACE_HIGH);
    assert!(flat.iter().all(|r| r.width() < 300.0), "no opaque card behind it: {flat:?}");
    let label = f.placed.iter().find(|(text, ..)| text == "Rename PC").expect("the dialog's words").1;
    fn walk(shape: &egui::Shape, out: &mut Vec<Rect>) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
            egui::Shape::Mesh(mesh) => out.push(mesh.calc_bounds()),
            _ => {}
        }
    }
    let mut meshes = Vec::new();
    f.shapes.iter().for_each(|c| walk(&c.shape, &mut meshes));
    assert!(meshes.iter().any(|r| r.contains_rect(label) && r.width() < f.size.x), "a mesh of the panel's size is behind its words, whole: {meshes:?}");
}

#[test]
fn the_pc_menu_says_what_the_pc_page_says() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    let host = f.hosts.list[0].id;
    f.l.dialog = Dialog::HostOptions { host };
    f.settle();
    assert!(shows(&f, "Online"), "its status under its name");
    assert!(!shows(&f, "Paired"), "being paired is not news");
}
