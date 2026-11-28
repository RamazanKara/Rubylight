//! Store filters, collections, demo-page layout, and narrow-window tab tests.

use super::*;

#[test]
fn the_pin_screen_has_its_keys_in_the_window_delete_at_the_bottom_right() {
    let dir = std::env::temp_dir().join(format!("nightjar-pin-keys-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.ctx.enable_accesskit();
    for size in [vec2(1280.0, 720.0), vec2(720.0, 480.0)] {
        f.size = size;
        f.l.pin = Some(crate::pin_ui::PinLock::demo(&dir, false, "Desk"));
        f.settle();
        let read = read_out(&f);
        let at = |name: &str| read.iter().find(|(said, _)| said == name).unwrap_or_else(|| panic!("{name} in {read:?}")).1;
        let (zero, delete, confirm, one) = (at("0"), at("Delete"), at("Confirm"), at("1"));
        assert!(delete.x0 > zero.x0 && confirm.x0 < zero.x0, "Confirm, 0, Delete");
        assert!((delete.y0 - zero.y0).abs() < 1.0 && one.y0 < zero.y0, "the bottom row");
        assert!(delete.y1 <= f64::from(size.y) && one.y0 >= 0.0, "all of the pad shows at {size:?}");
        assert!(delete.x1 - delete.x0 > 44.0, "a key to hit at {size:?}");
    }
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn the_pin_page_takes_the_keys_and_escape_leaves_a_list_before_the_page() {
    let dir = std::env::temp_dir().join(format!("nightjar-pin-page-{}", std::process::id()));
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.ctx.enable_accesskit();
    let key = |key| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
    f.l.pin = Some(crate::pin_ui::PinLock::demo_page(&dir, None));
    f.settle();
    let said: Vec<String> = read_out(&f).into_iter().map(|(said, _)| said).collect();
    assert!(said.iter().any(|s| s == "Set PIN") && said.iter().any(|s| s == "Back to Settings"), "{said:?}");
    assert!(!said.iter().any(|s| s == "Lock"), "with no PIN nothing is there to lock: {said:?}");
    assert!(f.ctx.memory(|m| m.focused()).is_some(), "the page opens on its first action");

    f.l.pin = Some(crate::pin_ui::PinLock::demo_page(&dir, Some(crate::pin_lock::Mode::App)));
    f.settle();
    let said: Vec<String> = read_out(&f).into_iter().map(|(said, _)| said).collect();
    for name in ["Change PIN", "Remove PIN", "Lock", "Ask again after"] {
        assert!(said.iter().any(|s| s == name), "{name} in {said:?}");
    }
    // Change, Remove, then the list: by Tab alone.
    f.frame(vec![key(Key::Tab)]);
    f.frame(vec![key(Key::Tab)]);
    f.settle();
    f.frame(vec![key(Key::Enter)]);
    f.settle();
    assert!(egui::Popup::is_any_open(&f.ctx), "Enter opens the list");
    f.frame(vec![key(Key::Escape)]);
    f.settle();
    assert!(!egui::Popup::is_any_open(&f.ctx), "Escape closes the list");
    assert!(f.l.pin.as_ref().unwrap().up(), "and only the list");
    f.frame(vec![key(Key::Escape)]);
    f.settle();
    assert!(!f.l.pin.as_ref().unwrap().up(), "the next one leaves the page");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_panel_blurs_the_whole_screen_from_the_pass_after_it_opens() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    crate::live_glass::set_tier(&f.ctx, crate::live_glass::Tier::Live);
    f.settle();
    assert!(blurred_share(&f) < 0.5, "at rest: the bar");
    assert!(!f.ctx.has_requested_repaint(), "and no frames");
    // A click opens the dialog after this pass's boundary went out. The pass after it, which any
    // input brings, has the whole blur: no panel stays on the bar's until the pointer moves.
    let add = f.placed.iter().find(|(t, ..)| t == "Add PC").map(|p| p.1).expect("Add PC");
    let press = |pressed| egui::Event::PointerButton { pos: add.center(), button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    f.frame(vec![egui::Event::PointerMoved(add.center())]);
    f.settle();
    f.frame(vec![press(true)]);
    f.frame(vec![press(false)]);
    assert!(!matches!(f.l.dialog, Dialog::None), "the click opened Add a PC");
    assert!(f.ctx.has_requested_repaint(), "so the next pass comes");
    f.frame(vec![]);
    assert!(blurred_share(&f) > 0.99);

    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    crate::live_glass::set_tier(&f.ctx, crate::live_glass::Tier::Live);
    f.settle();
    f.waking = Some(WakingView::new("PC"));
    f.frame(vec![]);
    assert!(blurred_share(&f) > 0.99, "the waking card stands anywhere as a dialog does");
    f.waking = None;
    f.frame(vec![]);
    assert!(blurred_share(&f) < 0.5, "and the bar alone once it is gone");
}

/// A dialog over the launcher blurs the page under its dim on the live tier, as Android's
/// `PageBlur` does: the live chain's own output in one quad over the screen, past the chain's
/// boundary (so the chain never blurs it again) and under the bar, which stays itself, both under
/// the dim. The baked tier, which every stream's frames use, keeps the dim alone.
#[test]
fn a_dialog_blurs_the_page_under_the_bar_and_its_dim_on_the_live_tier_only() {
    use crate::live_glass::{Boundary, Tier, LIVE_BACKDROP};
    #[derive(Debug)]
    enum Painted { Boundary, Blur(Rect), Fill(Color32, Rect), Text(String) }
    fn walk(shape: &egui::Shape, out: &mut Vec<Painted>) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
            egui::Shape::Mesh(mesh) if mesh.texture_id == LIVE_BACKDROP => out.push(Painted::Blur(mesh.calc_bounds())),
            egui::Shape::Rect(rect) => out.push(Painted::Fill(rect.fill, rect.rect)),
            egui::Shape::Text(text) => out.push(Painted::Text(text.galley.text().to_owned())),
            egui::Shape::Callback(call) if (call.callback.as_ref() as &dyn std::any::Any).is::<Boundary>() => out.push(Painted::Boundary),
            _ => {}
        }
    }
    for tier in [Tier::Live, Tier::Baked] {
        let mut f = Frames::new(record("PC", true, false), Reach::Online);
        crate::live_glass::set_tier(&f.ctx, tier);
        f.settle();
        let screen = Rect::from_min_size(Pos2::ZERO, f.size).shrink(1.0);
        let painted = |f: &Frames| {
            let mut out = Vec::new();
            f.shapes.iter().for_each(|clipped| walk(&clipped.shape, &mut out));
            out
        };
        let page = |p: &[Painted]| p.iter().position(|p| matches!(p, Painted::Blur(rect) if rect.contains_rect(screen)));
        let dim = |p: &[Painted]| p.iter().position(|p| matches!(p, Painted::Fill(fill, rect) if *fill == super::widgets::DIALOG_SCRIM && rect.contains_rect(screen)));
        assert_eq!(page(&painted(&f)), None, "{tier:?}: no dialog, no blur");
        f.l.add_host_dialog();
        // The dim fades in over a few frames; read it once it has.
        f.settle();
        f.settle();
        let p = painted(&f);
        let dim = dim(&p).unwrap_or_else(|| panic!("{tier:?}: the dim stays: {p:?}"));
        match tier {
            Tier::Live => {
                let page = page(&p).expect("the page is blurred");
                let boundary = p.iter().position(|p| matches!(p, Painted::Boundary)).expect("the chain's boundary");
                assert!(boundary < page && page < dim, "past the boundary, under the dim: {p:?}");
                // The bar's own text is painted over the blur, and dimmed with it.
                assert!(p[page..dim].iter().any(|p| matches!(p, Painted::Text(text) if !text.is_empty())), "the bar over the blur: {p:?}");
            }
            _ => assert_eq!(page(&p), None, "{tier:?}: the dim alone"),
        }
    }
}

#[test]
fn add_and_rename_say_what_a_field_takes_in_its_placeholder() {
    let mut f = Frames::new(record("Desk", true, false), Reach::Online);
    f.l.add_host_dialog();
    f.settle();
    assert!(shows(&f, "IP address or name"), "{:?}", f.drawn);
    assert!(f.drawn.iter().all(|t| !t.trim_end().ends_with('.')), "no sentence: {:?}", f.drawn);
    f.l.dialog = Dialog::Rename { host: f.hosts.list[0].id, text: String::new() };
    f.settle();
    assert!(shows(&f, "Desk"), "the name an empty field leaves: {:?}", f.drawn);
    assert!(f.drawn.iter().all(|t| !t.trim_end().ends_with('.')), "no sentence: {:?}", f.drawn);
}

#[test]
fn a_pin_screen_counts_digits_and_never_says_one() {
    let dir = std::env::temp_dir().join(format!("nightjar-pin-said-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.ctx.enable_accesskit();
    f.l.pin = Some(crate::pin_ui::PinLock::demo(&dir, false, "Desk"));
    f.settle();
    f.frame(vec![egui::Event::Key { key: Key::Num7, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }]);
    f.settle();
    let said: Vec<String> = read_out(&f).into_iter().map(|(said, _)| said).collect();
    assert!(said.iter().any(|s| s == "1 of 4 digits entered"), "{said:?}");
    assert!(f.placed.iter().all(|(text, ..)| !text.contains('*')), "a dot, no character, in a slot");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_dialogs_buttons_sit_right_under_it_in_a_short_window() {
    let mut f = approving_frames(true);
    f.size = vec2(853.0, 480.0);
    f.hosts.list[0].native.pairing_requests = vec![tv_request("a1")];
    f.l.dialog = dialogs::approve_dialog(&f.hosts.list[0], &f.hosts.list[0].native.pairing_requests[0].clone());
    f.settle();
    let at = |text: &str| f.placed.iter().find(|(t, ..)| t == text).map(|p| p.1).unwrap_or_else(|| panic!("{text}"));
    assert!(at("Cancel").top() - at("0").bottom() < 60.0, "no empty row above the buttons: {:?} under {:?}", at("Cancel"), at("0"));
    assert!(at("Cancel").bottom() < 480.0, "in the window");
}

/// F, as on any card, opens a found PC's options from Pair, and the hints call Pair by its name.
#[test]
fn f_on_a_found_card_opens_its_options() {
    let mut found = Hosts::without_discovery(vec![record("Game Room", false, false)], Arc::new(|| {}));
    found.discovering = true;
    found.list[0].reach = Reach::Online;
    let mut f = first_run_frames(Vec::new());
    f.hosts = found;
    f.l.go(Screen::Hosts);
    f.settle();
    f.l.input_source = crate::input_source::InputSource::Keyboard;
    f.settle();
    let count = |f: &Frames, word: &str| f.placed.iter().filter(|(t, ..)| t == word).count();
    assert_eq!(count(&f, "Pair"), 2, "the button and the hint: {:?}", f.placed);
    assert_eq!(count(&f, "Open"), 0, "{:?}", f.placed);
    f.press(Key::F);
    assert!(matches!(f.l.dialog, Dialog::HostOptions { .. }));
}

/// The hint names what Go does: Play, Resume, Add to favourites and Quit game on the hero's buttons,
/// Play or Resume on the game page's, and Open only on what opens.
#[test]
fn the_go_hint_names_what_the_control_does() {
    let tab_until = |f: &mut Frames, word: &str, go: &str| {
        let hint = |f: &Frames| f.placed.iter().filter(|(t, ..)| t == go).count();
        for _ in 0..20 {
            let focused = f.ctx.memory(|m| m.focused()).and_then(|i| f.ctx.read_response(i)).map(|r| r.rect);
            let on = f.placed.iter().any(|(t, r, _)| t == word && focused.is_some_and(|at| at.contains(r.center())));
            if on && f.placed.iter().any(|(t, ..)| t == "Options") {
                let open = f.placed.iter().filter(|(t, ..)| t == "Open").count();
                assert_eq!(open, 0, "{word} is no Open: {:?}", f.placed);
                assert!(hint(f) >= 2, "{word} and the hint {go}: {:?}", f.placed);
                return;
            }
            f.press(Key::Tab);
            f.settle();
        }
        panic!("Tab never reached {word}: {:?}", f.placed);
    };
    let mut f = home_with_games(1);
    f.l.input_source = crate::input_source::InputSource::Keyboard;
    f.settle();
    tab_until(&mut f, "Play", "Play");
    // The hero's star shows no words: its hint says what it does.
    let star = Id::unique("hero-star");
    for _ in 0..20 {
        if f.ctx.memory(|m| m.has_focus(star)) {
            break;
        }
        f.press(Key::Tab);
        f.settle();
    }
    assert!(f.ctx.memory(|m| m.has_focus(star)), "Tab reaches the star");
    assert!(f.placed.iter().any(|(t, ..)| t == "Add to favourites"), "its hint: {:?}", f.placed);
    let pc = f.hosts.list[0].id;
    f.l.go(Screen::Game { host: pc, app: 1000 });
    f.settle();
    tab_until(&mut f, "Play", "Play");
    // A running game: Resume, and its Quit game by its name.
    Arc::make_mut(f.hosts.list[0].native.library.as_mut().unwrap()).games[0].running = true;
    f.settle();
    tab_until(&mut f, "Resume", "Resume");
    f.l.go(Screen::Home);
    f.settle();
    tab_until(&mut f, "Resume", "Resume");
    tab_until(&mut f, "Quit game", "Quit game");
}

/// A key cap and a word for what has focus, for the device in the hand; nothing for a mouse.
#[test]
fn the_button_hints_follow_the_last_input() {
    let mut f = Frames::with(record("Den", true, true), Reach::Online, Some(answer(true, 0)), vec![game(1)]);
    f.l.go(Screen::Hosts);
    f.settle();
    f.press(Key::Tab);
    f.settle();
    let says = |f: &Frames, word: &str| f.placed.iter().any(|(t, ..)| t == word);
    assert!(!says(&f, "Options"), "no hints before the last input is known: {:?}", f.placed);
    f.l.input_source = crate::input_source::InputSource::Keyboard;
    for _ in 0..12 {
        if says(&f, "Options") {
            break;
        }
        f.press(Key::Tab);
        f.settle();
    }
    assert!(says(&f, "Options"), "a card with options takes focus by Tab: {:?}", f.placed);
    assert!(says(&f, "F") && says(&f, "Enter"), "the keyboard's caps: {:?}", f.placed);
    f.l.input_source = crate::input_source::InputSource::Pad(crate::glyphs::Style::PlayStation);
    f.settle();
    assert!(says(&f, "Options") && !says(&f, "Enter"), "a pad's own labels: {:?}", f.placed);
    f.l.input_source = crate::input_source::InputSource::None;
    f.settle();
    assert!(!says(&f, "Options"), "none for a mouse");
}

/// A game the PC runs that is not on its list still resumes and quits, as any other.
#[test]
fn a_running_game_with_no_list_entry_offers_the_same_choices() {
    let mut f = Frames::with(record("Den", true, true), Reach::Online, Some(answer(true, 77)), vec![game(1)]);
    f.settle();
    let host = &f.hosts.list[0];
    assert_eq!(host.running_app(), Some(77));
    let options = dialogs::host_options(host);
    assert!(options.contains(&HostOption::Resume) && options.contains(&HostOption::QuitApp), "{options:?}");
}

/// The Resume a game with no list entry offers: what it sends reaches the stream start as a resume
/// of the running session, whatever the state of the list, and the PC's read still matches it.
#[test]
fn resuming_a_running_game_with_no_list_entry_reaches_the_stream_start() {
    let lists = [Apps::Loaded(Arc::new(vec![game(1)])), Apps::NotLoaded, Apps::Failed("no".into())];
    for list in lists {
        let mut f = Frames::with(record("Den", true, true), Reach::Online, Some(answer(true, 77)), vec![game(1)]);
        f.settle();
        f.hosts.list[0].apps = list;
        let mut commands = Vec::new();
        let asked = f.l.choose_host_option(&f.hosts.list[0], HostOption::Resume, &mut commands);
        assert!(asked.is_none());
        let (app, resume) = match commands[..] {
            [Command::Launch { app, resume, .. }] => (app, resume),
            _ => panic!("one launch"),
        };
        assert!((app, resume) == (77, true));
        let crate::hosts::LaunchApp::Ready(started) = crate::hosts::launch_target(&f.hosts.list[0], app, resume) else {
            panic!("the resume stops short of the stream start");
        };
        assert!(started.id == 77 && started.control == nightjar_policy::http::AppControl::None);
        assert!(started.matches_running_app(f.hosts.list[0].running_app().unwrap(), None), "the start's own check");
    }
}

/// Try again on a failed list reads as a read, not as "no apps": only the empty page's own Refresh
/// holds that page, and it goes when the list has answered, empty or not.
#[test]
fn try_again_on_a_failed_list_does_not_say_no_apps() {
    let mut f = Frames::with(record("Den", true, true), Reach::Online, Some(answer(true, 0)), Vec::new());
    let id = f.hosts.list[0].id;
    f.hosts.list[0].apps = Apps::Failed("The PC did not answer".into());
    f.l.go(Screen::Apps(id));
    f.settle();
    assert!(f.press(Key::Enter).iter().any(|c| matches!(c, Command::Refresh(_))), "Try again has the focus");
    f.hosts.list[0].apps = Apps::Loading;
    f.settle();
    assert!(!shows(&f, "This PC lists no apps."), "{:?}", f.drawn);
    // The empty page's Refresh, answered empty, then read by something else.
    f.hosts.list[0].apps = Apps::Loaded(Arc::new(Vec::new()));
    f.l.refocus();
    f.settle();
    // The launcher reads it before the next frame, as main does with the command.
    assert!(f.frame(tap_key(Key::Enter).to_vec()).iter().any(|c| matches!(c, Command::Refresh(_))));
    f.hosts.list[0].apps = Apps::Loading;
    f.settle();
    assert!(shows(&f, "This PC lists no apps."), "held while it reads: {:?}", f.drawn);
    f.hosts.list[0].apps = Apps::Loaded(Arc::new(Vec::new()));
    f.settle();
    f.hosts.list[0].apps = Apps::Loading;
    f.settle();
    assert!(!shows(&f, "This PC lists no apps."), "the answer ended it: {:?}", f.drawn);
}

/// The smallest window holds the hero's whole row when Try again joins it: Play (or Resume) and
/// Try again as its one quiet action.
#[test]
fn try_again_leaves_homes_row_in_the_smallest_window() {
    for running in [0, 1000] {
        let mut f = home_with_games(1);
        f.hosts.list[0].info = Some(answer(true, running));
        f.size = vec2(720.0, 490.0);
        f.settle();
        f.hosts.list[0].apps = Apps::Failed("The PC did not answer".into());
        f.l.refocus();
        f.settle();
        // The star is a round button with no words: its place is its response's.
        let words = |f: &Frames| -> Vec<Rect> {
            let mut row: Vec<Rect> = ["Resume", "Play", "Try again", "Quit game"].iter().filter_map(|w| f.placed.iter().find(|(t, ..)| t == w).map(|p| p.1)).collect();
            row.extend(f.ctx.read_response(Id::unique("hero-star")).map(|r| r.rect));
            row.sort_by(|a, b| a.left().total_cmp(&b.left()));
            row
        };
        let row = words(&f);
        assert_eq!(row.len(), 2, "running {running}: {:?}", f.drawn);
        assert!(row.iter().all(|r| r.right() <= f.size.x), "running {running}: {row:?} in {:?}", f.size);
        assert!(row.windows(2).all(|w| w[0].right() < w[1].left()), "running {running}: {row:?}");
    }
}

/// Home's hero says what stops Play, as the game's page does, offers Try again and comes back from it.
#[test]
fn homes_play_says_what_stops_it_and_recovers() {
    let entry = nightjar_policy::http::App { name: "Game 00".into(), uuid: "install-0".into(), ..game(1000) };
    type Case = (fn() -> Apps, &'static str);
    let cases: [Case; 2] = [
        (|| Apps::Failed("The PC did not answer".into()), "The app list could not be loaded. The PC did not answer"),
        (|| Apps::Loaded(Arc::new(vec![game(77)])), "The PC’s app list has no entry for this game."),
    ];
    for (list, why) in cases {
        for screen in ["Home", "Game"] {
            let mut f = home_with_games(1);
            let pc = f.hosts.list[0].id;
            if screen == "Game" {
                f.l.go(Screen::Game { host: pc, app: 1000 });
            }
            f.settle();
            f.hosts.list[0].apps = list();
            f.l.refocus();
            f.settle();
            assert!(shows(&f, why), "{screen}: {:?}", f.drawn);
            let at = |f: &Frames, text: &str| f.placed.iter().find(|(t, ..)| t == text).map(|p| p.1);
            let retry = at(&f, "Try again").unwrap_or_else(|| panic!("{screen}: {:?}", f.drawn));
            assert!(at(&f, why).unwrap().bottom() <= at(&f, "Play").unwrap().top() + 1.0, "{screen}: above the buttons");
            assert!(at(&f, "Game 00").is_none_or(|t| t.bottom() <= at(&f, why).unwrap().top()), "{screen}: under the title");
            // The press reads the list again, also one that came without the game.
            let commands = f.click_at(retry.center(), egui::PointerButton::Primary);
            assert!(commands.iter().any(|c| matches!(c, Command::ReadAppsAgain(host) if *host == pc)), "{screen}");
            f.hosts.list[0].apps = Apps::Loading;
            f.settle();
            assert!(!shows(&f, why) && !shows(&f, "Try again"), "{screen}: {:?}", f.drawn);
            f.hosts.list[0].apps = Apps::Loaded(Arc::new(vec![entry.clone()]));
            f.settle();
            let play = at(&f, "Play").expect("Play");
            let commands = f.click_at(play.center(), egui::PointerButton::Primary);
            assert!(commands.iter().any(|c| matches!(c, Command::Launch { app: 1000, .. })), "{screen}: it plays again");
        }
    }
}

/// A dialog over All games holds the search field too: neither a click nor typing reaches it.
#[test]
fn the_search_field_takes_nothing_under_a_dialog() {
    let mut f = home_with_games(3);
    f.l.go(Screen::AllGames);
    f.settle();
    f.l.open_search();
    f.settle();
    let field = f.placed.iter().find(|(t, ..)| t == "Search").map(|p| p.1).expect("the field");
    f.frame(vec![egui::Event::Text("a".into())]);
    assert_eq!(f.l.library_query, "a", "the field has the keys while nothing covers it");
    f.l.add_host_dialog();
    f.settle();
    f.frame(vec![egui::Event::Text("b".into())]);
    assert!(f.l.library_query == "a", "typing under a dialog: {}", f.l.library_query);
    f.click_at(field.center(), egui::PointerButton::Primary);
    f.settle();
    f.ctx.memory_mut(|m| m.request_focus(egui::Id::unique(chrome::SEARCH_ID)));
    f.frame(Vec::new());
    f.frame(vec![egui::Event::Text("c".into())]);
    assert!(f.l.library_query == "a", "a click, then typing, under a dialog: {}", f.l.library_query);
    assert!(matches!(&f.l.dialog, Dialog::AddHost { .. }));
    // A dialog with no field of its own, and the same click and keys.
    f.l.dialog = Dialog::None;
    f.settle();
    f.l.message("Title", "Words".to_owned());
    f.settle();
    f.click_at(field.center(), egui::PointerButton::Primary);
    f.settle();
    f.frame(vec![egui::Event::Text("d".into())]);
    assert!(f.l.library_query == "a", "under a message: {}", f.l.library_query);
}

/// Enter on an empty field closes the search and stays on the page: opened again, Escape ends the
/// search there too, and does not leave for where the first search came from.
#[test]
fn reopening_the_search_and_escaping_stays_on_the_page() {
    let mut f = home_with_games(3);
    f.l.go(Screen::Home);
    f.settle();
    f.l.open_search();
    f.settle();
    assert_eq!(f.l.screen, Screen::AllGames);
    f.press(Key::Enter);
    assert!(!f.l.search_open);
    f.l.open_search();
    f.settle();
    f.press(Key::Escape);
    assert!(!f.l.search_open);
    assert_eq!(f.l.screen, Screen::AllGames, "one Escape ends the search, it does not leave");
}

/// Picking the PC whose page is open adds no screen for Back to walk through.
#[test]
fn opening_the_page_already_open_adds_no_back_press() {
    let mut f = Frames::with(record("Den", true, true), Reach::Online, Some(answer(true, 0)), vec![game(1)]);
    let id = f.hosts.list[0].id;
    f.l.go(Screen::Home);
    f.l.open(Screen::Apps(id));
    f.settle();
    let depth = f.l.trail.len();
    let picked = pcs::pc_action(&f.hosts.list[0]).command(id);
    assert!(matches!(picked, Command::OpenApps(host) if host == id), "what the sheet's row sends");
    f.l.open(Screen::Apps(id));
    assert_eq!(f.l.trail.len(), depth);
}

/// The details page keeps what the PC's card used to say: its address and its GPU, also on a host
/// with no web UI to carry the address.
#[test]
fn the_details_page_shows_the_address_and_the_gpu() {
    let mut f = details(Reach::Online, "MJOLNIR_STATE_FREE", "");
    f.hosts.list[0].record.address = "192.0.2.7".into();
    f.hosts.list[0].info.as_mut().expect("the answer").gpu_type = Some("RX 7900 XT".into());
    f.settle();
    assert!(shows(&f, "192.0.2.7") && shows(&f, "RX 7900 XT"), "{:?}", f.drawn);
    f.hosts.list[0].info.as_mut().expect("the answer").gpu_type = None;
    f.settle();
    assert!(shows(&f, "192.0.2.7") && !shows(&f, "GPU"), "no empty line: {:?}", f.drawn);
}

/// A PC's page with a list has a round Refresh in its head, by keyboard too; it spins and keeps the
/// focus while it reads, and leaves room for what a running game offers.
#[test]
fn a_list_has_a_round_refresh_in_its_head() {
    let square = |f: &Frames| {
        f.ctx.memory(|m| m.focused()).and_then(|i| f.ctx.read_response(i)).map(|r| r.rect).filter(|r| (r.width() - r.height()).abs() < 1.0 && r.height() == 48.0)
    };
    let mut f = Frames::with(record("Den", true, true), Reach::Online, Some(answer(true, 0)), vec![game(7)]);
    let id = f.hosts.list[0].id;
    f.l.go(Screen::Apps(id));
    f.settle();
    let mut tabs = 0;
    while square(&f).is_none() {
        f.press(Key::Tab);
        tabs += 1;
        assert!(tabs < 12, "Tab reaches it");
    }
    let at = square(&f).expect("it");
    assert!(at.right() > f.size.x - 120.0 && at.top() < 200.0, "the head's right: {at:?}");
    assert!(f.press(Key::Enter).iter().any(|c| matches!(c, Command::Refresh(host) if *host == id)));
    f.hosts.list[0].apps = Apps::Loading;
    f.settle();
    assert_eq!(library::head_refresh(&f.hosts.list[0], None), Some(true));
    assert_eq!(square(&f), Some(at), "it keeps the focus while it spins");
    assert!(!f.press(Key::Enter).iter().any(|c| matches!(c, Command::Refresh(_))), "one read at a time");
    f.hosts.list[0].apps = Apps::Loaded(Arc::new(vec![game(7)]));
    f.settle();
    assert_eq!(square(&f), Some(at));
    assert_eq!(library::head_refresh(&f.hosts.list[0], None), Some(false));
    // Beside what a running game offers.
    f.hosts.list[0].info = Some(answer(true, 7));
    f.settle();
    let quit = f.placed.iter().find(|(t, ..)| t == "Quit game").map(|p| p.1).expect("Quit game");
    assert!(quit.right() < at.left(), "{quit:?} {at:?}");
    // No list, no second Refresh: the page's middle has it.
    f.hosts.list[0].apps = Apps::Loaded(Arc::new(Vec::new()));
    assert_eq!(library::head_refresh(&f.hosts.list[0], None), None);
    f.hosts.list[0].apps = Apps::Failed("no".into());
    assert_eq!(library::head_refresh(&f.hosts.list[0], None), None);
    f.hosts.list[0].apps = Apps::Loading;
    assert_eq!(library::head_refresh(&f.hosts.list[0], Some(id)), None, "the empty page's own at work");
}

/// A waiting Play holds while a dialog is open: no stream starts under it and no typed text goes.
#[test]
fn a_waiting_play_holds_while_a_dialog_is_open() {
    let entry = nightjar_policy::http::App { name: "Game 00".into(), uuid: "install-0".into(), ..game(1000) };
    for (landing, launches) in [(Apps::Loaded(Arc::new(vec![entry])), true), (Apps::Failed("The PC did not answer".into()), false)] {
        let mut f = home_with_games(1);
        let pc = f.hosts.list[0].id;
        f.hosts.list[0].apps = Apps::Loading;
        f.l.go(Screen::Game { host: pc, app: 1000 });
        f.settle();
        f.press(Key::Enter);
        assert!(f.l.playing_soon(pc, 1000));
        f.l.dialog = Dialog::AddHost { text: "typed".into(), error: None };
        f.hosts.list[0].apps = landing;
        for _ in 0..3 {
            assert!(!f.frame(Vec::new()).iter().any(|c| matches!(c, Command::Launch { .. })), "not under a dialog");
        }
        assert!(matches!(&f.l.dialog, Dialog::AddHost { text, .. } if text == "typed"), "the words stay");
        assert!(f.l.playing_soon(pc, 1000), "still waiting");
        f.l.dialog = Dialog::None;
        let commands = f.frame(Vec::new());
        assert_eq!(commands.iter().any(|c| matches!(c, Command::Launch { app: 1000, .. })), launches);
        assert!(!f.l.playing_soon(pc, 1000));
        if !launches {
            assert!(matches!(f.l.dialog, Dialog::Message { .. }), "and now it says why");
        }
    }
}

/// A control tile that carries the running id is not the running game: Resume never fires its
/// action, every name for the running game agrees, and the tile is not what Resume names.
#[test]
fn a_control_tile_with_the_running_id_is_never_what_resume_runs() {
    let mut tile = game(77);
    tile.name = "Disconnect".into();
    tile.control = nightjar_policy::http::AppControl::Terminate;
    let mut f = Frames::with(record("Den", true, true), Reach::Online, Some(answer(true, 77)), vec![tile, game(1)]);
    f.settle();
    let host = &f.hosts.list[0];
    assert!(library::running_entry(host).is_none());
    assert_eq!(library::running_app_id(host), None, "one rule for both");
    assert_ne!(library::running_name(host), "Disconnect");
    let mut commands = Vec::new();
    f.l.choose_host_option(host, HostOption::Resume, &mut commands);
    assert!(matches!(commands[..], [Command::Launch { app: 77, resume: true, .. }]));
    let crate::hosts::LaunchApp::Ready(started) = crate::hosts::launch_target(host, 77, true) else {
        panic!("resume goes on");
    };
    assert!(!started.is_control_action() && started.control == nightjar_policy::http::AppControl::None);
    assert!(matches!(crate::hosts::launch_target(host, 77, false), crate::hosts::LaunchApp::Ready(a) if a.is_control_action()), "the tile's own press still is its action");
}

/// Behind a page without art is the stage and nothing else: the canvas, the stage's one layer and
/// the page colour, on every such page, in the default window at 150 %, a large window, the
/// smallest one and 150 % in German. No disc, ring or speck is painted behind or between a page's
/// cards, chips, rows and titles.
#[test]
fn behind_a_page_without_art_is_the_stage_and_nothing_else() {
    let pages = [Screen::Hosts, Screen::Settings, Screen::Help, Screen::Profiles, Screen::Controllers, Screen::Shortcuts, Screen::StreamHotkeys];
    let windows = [(vec2(1280.0, 800.0), 150, false), (vec2(1280.0, 800.0), 100, false), (vec2(1920.0, 1080.0), 100, false), (vec2(720.0, 490.0), 100, false), (vec2(1280.0, 800.0), 150, true)];
    for (window, zoom, german) in windows {
        for page in pages {
            let mut f = four_pcs();
            f.l.settings.ui_zoom = zoom;
            if german {
                f.l.settings.language = crate::config::Language::German;
            }
            f.size = window;
            f.l.go(page);
            f.settle();
            f.settle();
            let what = format!("{page:?} at {window:?}, {zoom} %{}", if german { ", German" } else { "" });
            let screen = f.ctx.content_rect();
            let behind = backdrop_shapes(&f);
            let [egui::Shape::Rect(canvas), egui::Shape::Mesh(stage), egui::Shape::Mesh(ground)] = behind else {
                panic!("{what}: the canvas, the stage and the page colour: {behind:?}");
            };
            assert_eq!(canvas.rect, screen, "{what}");
            assert_eq!(Some(stage.texture_id), glass::backdrop_of(&f.ctx).top.map(|layer| layer.texture), "{what}: what glass samples");
            assert!(stage.calc_bounds().contains_rect(screen), "{what}: the stage covers the window");
            assert_eq!(ground.texture_id, egui::TextureId::default(), "{what}");
            assert!((ground.calc_bounds().bottom() - screen.bottom()).abs() < 0.5 && ground.calc_bounds().width() >= screen.width() - 0.5, "{what}: the page colour to the window's foot");
        }
    }
}
