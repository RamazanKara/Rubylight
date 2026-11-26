//! Dialog queueing, navigation trails, and host-option matrix tests.

use super::*;

#[test]
fn the_stand_in_names_fit_their_sentences() {
    assert_eq!(at_sentence_start(tr("the running app")), "the running app".replacen('t', "T", 1));
    assert_eq!(at_sentence_start("osu!"), "osu!");
    crate::strings::set(crate::strings::Lang::De);
    assert_eq!(at_sentence_start(tr("the running app")), "Die laufende App");
    assert_eq!(trf("Pairing with {} failed", &[&tr("the PC")]), "Koppeln mit dem PC fehlgeschlagen");
    assert_eq!(trf("Pair with {}", &[&tr("the PC")]), "Mit dem PC koppeln");
    assert_eq!(trf("Quit {}?", &[&tr("the running app")]), "die laufende App beenden?");
}

#[test]
fn an_answer_that_arrives_by_itself_waits_for_the_open_dialog() {
    let mut l = launcher(true, true);
    let moved = || Dialog::NowPlaying { host: 1, app: 7, device: "TV".into() };
    l.ask_after(moved());
    assert!(matches!(l.dialog, Dialog::NowPlaying { .. }), "nothing open: at once");
    // The failure report of the same stream arrives while it is up, and neither is lost.
    l.ask_after(Dialog::Message { title: "Stream".into(), body: "failed".into() });
    assert!(matches!(l.dialog, Dialog::NowPlaying { .. }));
    l.dialog = Dialog::None;
    assert!(l.next_queued());
    assert!(matches!(&l.dialog, Dialog::Message { body, .. } if body == "failed"));

    // And the other way round: a question the player is in stays, its answer comes after it.
    l.dialog = Dialog::ContinueHere { host: 1, app: 7, resume: true, name: "Game".into(), device: "Phone".into(), join: false };
    l.ask_after(moved());
    assert!(matches!(l.dialog, Dialog::ContinueHere { .. }));
    l.dialog = Dialog::None;
    assert!(l.next_queued());
    assert!(matches!(l.dialog, Dialog::NowPlaying { .. }));
    assert!(!l.next_queued());
}

#[test]
fn a_worker_s_message_waits_for_the_pin_and_the_replace_prompt_but_not_for_a_running_test() {
    let mut l = launcher(true, true);
    // A wake for another PC fails while the PIN is up: the PIN stays, the message comes after it.
    l.ask(Dialog::Pairing { host: 1, pin: "1234".into() });
    l.notify(tr("Could not wake the PC"), "no route");
    assert!(matches!(&l.dialog, Dialog::Pairing { pin, .. } if pin == "1234"));
    l.ask(Dialog::ConfirmReplace { app: "Hades".into(), running: "Celeste".into() });
    l.notify(tr("Could not quit the game"), "busy");
    assert!(matches!(l.dialog, Dialog::ConfirmReplace { .. }), "the launch waits on this answer");
    l.dialog = Dialog::None;
    assert!(l.next_queued());
    assert!(matches!(&l.dialog, Dialog::Message { body, .. } if body == "no route"));
    l.dialog = Dialog::None;
    assert!(l.next_queued());
    assert!(matches!(&l.dialog, Dialog::Message { body, .. } if body == "busy"));
    assert!(!l.next_queued());

    // Over nothing or a running test's progress: at once.
    l.dialog = Dialog::NetworkTest { stage: "Measuring".into(), calibrate: false };
    l.notify(tr("Network test failed"), "timed out");
    assert!(matches!(&l.dialog, Dialog::Message { body, .. } if body == "timed out"));
    assert!(l.focus_dialog);
    // A click's own answer still shows at once.
    l.ask(Dialog::Rename { host: 1, text: "Den".into() });
    l.message(tr("Could not unpair"), "now");
    assert!(matches!(&l.dialog, Dialog::Message { body, .. } if body == "now"));
}

#[test]
fn a_prompt_that_said_the_game_moved_says_it_came_back() {
    let mut l = launcher(true, true);
    assert_eq!(l.moved_prompt(), None);
    l.ask_after(Dialog::NowPlaying { host: 1, app: 7, device: "TV".into() });
    assert_eq!(l.moved_prompt(), Some((1, 7)));
    // Another PC's or game's answer changes nothing.
    l.game_came_back(2, 7);
    l.game_came_back(1, 8);
    assert!(matches!(l.dialog, Dialog::NowPlaying { .. }));
    l.focus_dialog = false;
    l.game_came_back(1, 7);
    assert!(matches!(l.dialog, Dialog::GameCameBack { host: 1, app: 7 }));
    assert!(l.focus_dialog, "the new question gets the focus");
    assert_eq!(l.moved_prompt(), None, "nothing left to watch");

    // A device that only streamed beside the game: another prompt, for that game only.
    let mut beside = launcher(true, true);
    beside.ask_after(Dialog::NowPlaying { host: 1, app: 7, device: "TV".into() });
    beside.move_failed(1, 8);
    assert!(matches!(beside.dialog, Dialog::NowPlaying { .. }));
    beside.move_failed(1, 7);
    assert!(matches!(beside.dialog, Dialog::MoveFailed { host: 1, app: 7 }));
    assert_eq!(beside.moved_prompt(), None);

    // One that still waits behind another dialog changes in its place, and takes nothing away.
    l.dialog = Dialog::Message { title: "Stream".into(), body: "failed".into() };
    l.ask_after(Dialog::NowPlaying { host: 1, app: 7, device: "TV".into() });
    assert_eq!(l.moved_prompt(), Some((1, 7)));
    l.game_came_back(1, 7);
    assert!(matches!(l.dialog, Dialog::Message { .. }));
    l.dialog = Dialog::None;
    assert!(l.next_queued());
    assert!(matches!(l.dialog, Dialog::GameCameBack { .. }));
}

#[test]
fn a_join_from_a_tile_starts_the_game_the_pc_runs() {
    let playing: nightjar_policy::http::PlayingOn = serde_json::from_str(
        r#"{"device":"Phone","app":{"install_id":"game-a","app_id":7,"title":"Game"}}"#,
    ).unwrap();
    let tile = nightjar_policy::http::App { control: nightjar_policy::http::AppControl::Resume, ..game(9) };
    assert_eq!(super::join_target(Some(&tile), 9, Some(&playing)), 7);
    assert_eq!(super::join_target(Some(&tile), 9, None), 9, "nobody plays it: the PC decides");
    assert_eq!(super::join_target(Some(&game(9)), 9, Some(&playing)), 9);
    assert_eq!(super::join_target(None, 9, Some(&playing)), 9);
}

#[test]
fn the_prompt_on_a_tile_starts_the_game_itself() {
    let playing: nightjar_policy::http::PlayingOn = serde_json::from_str(
        r#"{"device":"Phone","app":{"install_id":"game-a","app_id":7,"title":"Game"}}"#,
    ).unwrap();
    let tile = nightjar_policy::http::App { control: nightjar_policy::http::AppControl::RunningGame, ..game(9) };
    assert_eq!(super::asked_start(Some(&tile), 9, false, Some(&playing)), (7, true), "a resume of the tile's id is refused");
    assert_eq!(super::asked_start(Some(&game(7)), 7, true, Some(&playing)), (7, true));
    assert_eq!(super::asked_start(Some(&game(7)), 7, false, Some(&playing)), (7, false));
}

#[test]
fn the_game_another_device_plays_is_matched_by_id_and_install_id() {
    let playing: nightjar_policy::http::PlayingOn = serde_json::from_str(
        r#"{"device":"Phone","app":{"install_id":"game-a","app_id":7,"title":"Game"}}"#,
    ).unwrap();
    let app = |id, uuid: &str| nightjar_policy::http::App { uuid: uuid.into(), ..game(id) };
    assert!(super::library::plays_this_game(Some(&app(7, "game-a")), 7, &playing));
    assert!(!super::library::plays_this_game(Some(&app(7, "game-b")), 7, &playing), "the id was given to another game");
    assert!(super::library::plays_this_game(Some(&app(9, "game-a")), 9, &playing), "the game has a new id");
    assert!(super::library::plays_this_game(Some(&app(7, "")), 7, &playing), "no install id: the id decides");
    assert!(super::library::plays_this_game(None, 7, &playing));
    assert!(!super::library::plays_this_game(None, 8, &playing));
}

#[test]
fn a_tile_of_the_running_game_is_the_game_another_device_plays() {
    use nightjar_policy::http::AppControl;
    let playing: nightjar_policy::http::PlayingOn = serde_json::from_str(
        r#"{"device":"Phone","app":{"install_id":"game-a","app_id":7,"title":"Game"}}"#,
    ).unwrap();
    let tile = |control| nightjar_policy::http::App { control, uuid: "tile".into(), ..game(9) };
    assert!(super::library::plays_this_game(Some(&tile(AppControl::Resume)), 9, &playing));
    assert!(super::library::plays_this_game(Some(&tile(AppControl::RunningGame)), 9, &playing));
    assert!(!super::library::plays_this_game(Some(&tile(AppControl::Terminate)), 9, &playing));
    let other = nightjar_policy::http::App { uuid: "game-b".into(), ..game(9) };
    assert!(!super::library::plays_this_game(Some(&other), 9, &playing), "a game of its own is not this one");
}

#[test]
fn a_move_to_a_device_it_may_not_name_says_another_device() {
    assert_eq!(super::dialogs::now_playing_title("TV"), "Now playing on TV");
    assert_eq!(super::dialogs::now_playing_title(""), "Now playing on another device");
}

#[test]
fn back_returns_to_the_screen_and_the_item_it_left() {
    let mut l = launcher(true, true);
    l.go(Screen::Home);
    l.focused_key = Some("game:steam:570".into());
    l.open(Screen::Game { host: 1, app: 5 });
    assert_eq!(l.screen, Screen::Game { host: 1, app: 5 });
    l.focused_key = Some("play".into());
    let mut commands = Vec::new();
    l.back(&mut commands, true);
    assert_eq!(l.screen, Screen::Home);
    assert_eq!(l.restore_key.as_deref(), Some("game:steam:570"));
    assert!(l.focus_screen, "the first item takes focus if the remembered one is gone");
    // Home is the end of the line: the keyboard's Escape does nothing there, a controller's B
    // asks to close Nightjar.
    l.back(&mut commands, false);
    assert!(matches!(l.dialog, Dialog::None) && l.screen == Screen::Home, "Escape on Home stays harmless");
    l.back(&mut commands, true);
    assert_eq!(l.screen, Screen::Home);
    assert!(matches!(l.dialog, Dialog::ConfirmExit));
    l.back(&mut commands, true);
    assert!(matches!(l.dialog, Dialog::None) && commands.is_empty(), "a second Back keeps Nightjar open");
}

#[test]
fn back_ends_on_home_when_it_is_on_and_on_the_pcs_when_it_is_not() {
    let mut on = launcher(true, true);
    on.go(Screen::Hosts);
    on.open(Screen::Apps(1));
    let mut commands = Vec::new();
    on.back(&mut commands, true);
    assert_eq!(on.screen, Screen::Hosts);
    on.back(&mut commands, true);
    assert_eq!(on.screen, Screen::Home, "the PCs list goes back to Home");

    let mut off = launcher(false, true);
    off.go(Screen::Hosts);
    off.open(Screen::Apps(1));
    off.back(&mut commands, true);
    off.back(&mut commands, true);
    assert_eq!(off.screen, Screen::Hosts, "Open on Home is off: the PCs are the root");

    let mut empty = launcher(true, false);
    empty.go(Screen::Hosts);
    empty.back(&mut commands, true);
    assert_eq!(empty.screen, Screen::Hosts, "no library, no Home to go back to");
}

#[test]
fn settings_open_once_and_back_leaves_them_where_they_were_opened() {
    let mut l = launcher(true, true);
    l.go(Screen::Home);
    l.open_settings();
    l.open_settings();
    l.go(Screen::Profiles); // a settings subpage, reached the usual way
    l.open_settings();
    assert_eq!(l.screen, Screen::Settings);
    assert_eq!(l.trail.len(), 1, "Start inside Settings does not stack screens");
    let mut commands = Vec::new();
    l.back(&mut commands, true);
    assert_eq!(l.screen, Screen::Home);
    assert!(commands.iter().any(|c| matches!(c, Command::SaveSettings)), "leaving Settings saves them");
}

#[test]
fn start_from_a_library_or_game_page_returns_to_it() {
    for page in [Screen::Apps(1), Screen::Game { host: 1, app: 5 }] {
        let mut l = launcher(true, true);
        l.go(Screen::Home);
        l.open(page);
        l.focused_key = Some("play".into());
        l.open_settings();
        assert_eq!(l.screen, Screen::Settings);
        let mut commands = Vec::new();
        l.back(&mut commands, true);
        assert_eq!(l.screen, page, "B returns to where Start was pressed");
        assert_eq!(l.restore_key.as_deref(), Some("play"));
        l.back(&mut commands, true);
        assert_eq!(l.screen, Screen::Home, "and the page still goes back to the root");
    }
    // From a destination, Settings replaces it as before.
    let mut l = launcher(false, false);
    l.go(Screen::Hosts);
    l.open_settings();
    let mut commands = Vec::new();
    l.back(&mut commands, true);
    assert_eq!(l.screen, Screen::Hosts);
}

#[test]
fn a_game_page_for_another_copy_replaces_the_page() {
    let mut l = launcher(true, true);
    l.go(Screen::Home);
    l.open(Screen::Game { host: 1, app: 5 });
    l.go(Screen::Game { host: 2, app: 9 });
    let mut commands = Vec::new();
    l.back(&mut commands, true);
    assert_eq!(l.screen, Screen::Home, "one Back, not one per copy");
}

// ---- Headless frames: the PCs list driven by keys and the pointer. ----

#[test]
fn the_destinations_read_home_pcs_settings_help() {
    let names: Vec<&str> = DESTINATIONS.iter().map(|(name, _)| *name).collect();
    assert_eq!(names, ["Home", "PCs", "Settings", "Help"]);
    let screens: Vec<Screen> = DESTINATIONS.iter().map(|(_, screen)| *screen).collect();
    assert_eq!(screens, [Screen::Home, Screen::Hosts, Screen::Settings, Screen::Help]);
}

/// L1 and R1 step through the destinations, round at the ends, and leave a dialog alone.
#[test]
fn the_shoulders_step_through_the_destinations() {
    let mut f = Frames::new(record("Desk", true, false), Reach::Online);
    // Home is offered once a library is there to show.
    use rubylight_native_api::library::{Artwork, Game, Library};
    let game = Game {
        install_id: "install-0".into(),
        app_id: 1000,
        title: "Game 00".into(),
        game_id: None,
        store: None,
        installed: Some(true),
        running: false,
        artwork: Artwork { cover: None, version: String::new() },
        last_played: None,
        playtime_minutes: None,
        collections: Vec::new(),
    };
    f.hosts.list[0].native.library = Some(Arc::new(Library { revision: "r1".into(), games: vec![game] }));
    f.l.go(Screen::Home);
    f.settle();
    let step = |f: &mut Frames, forward: bool| {
        let mut commands = Vec::new();
        f.l.step_destination(&f.ctx, forward, &mut commands);
        f.settle();
        commands
    };
    step(&mut f, true);
    assert_eq!(f.l.screen, Screen::Hosts, "R1 from Home");
    step(&mut f, false);
    assert_eq!(f.l.screen, Screen::Home, "L1 back");
    step(&mut f, false);
    assert_eq!(f.l.screen, Screen::Help, "L1 from Home goes round to Help");
    step(&mut f, true);
    assert_eq!(f.l.screen, Screen::Home, "R1 from Help goes round to Home");
    // Settings is left saved, as its button leaves it.
    step(&mut f, true);
    step(&mut f, true);
    assert_eq!(f.l.screen, Screen::Settings);
    assert!(step(&mut f, true).iter().any(|c| matches!(c, Command::SaveSettings)));
    assert_eq!(f.l.screen, Screen::Help);
    // Over a dialog nothing moves.
    f.l.go(Screen::Home);
    pad_press(&mut f, Key::Escape);
    assert!(matches!(f.l.dialog, Dialog::ConfirmExit));
    assert!(step(&mut f, true).is_empty());
    assert_eq!(f.l.screen, Screen::Home);
    assert!(matches!(f.l.dialog, Dialog::ConfirmExit));
}

#[test]
fn destinations_replace_each_other_and_back_ends_at_the_root() {
    let mut l = launcher(true, true);
    l.go(Screen::Home);
    for screen in [Screen::Hosts, Screen::Settings, Screen::Hosts, Screen::Settings] {
        l.switch(screen);
        assert_eq!(l.screen, screen);
        assert_eq!(l.trail.len(), 1, "one step back to Home, however many destinations were visited");
    }
    l.switch(Screen::Home);
    assert_eq!((l.screen, l.trail.len()), (Screen::Home, 0), "Home is the root again");
    l.switch(Screen::Hosts);
    let mut commands = Vec::new();
    l.back(&mut commands, true);
    assert_eq!(l.screen, Screen::Home);
}

#[test]
fn profiles_and_shortcuts_pop_back_to_settings() {
    let mut l = launcher(true, true);
    l.go(Screen::Home);
    l.open_settings();
    l.focused_key = Some("profiles-link".into());
    l.open(Screen::Profiles);
    let mut commands = Vec::new();
    l.back(&mut commands, true);
    assert_eq!(l.screen, Screen::Settings);
    assert_eq!(l.restore_key.as_deref(), Some("profiles-link"), "the row that opened it takes focus");
    assert_eq!(l.trail.len(), 1, "Settings still goes back to Home in one step");
    l.open(Screen::Shortcuts);
    l.back(&mut commands, true);
    l.back(&mut commands, true);
    assert_eq!(l.screen, Screen::Home);
}

#[test]
fn settings_sections_follow_androids_order() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 12000.0);
    video_settings(&mut f);
    // Debug is listed on request; with it, every section shows.
    f.l.settings.show_debug_settings = true;
    f.settle();
    // The list beside the rows names the sections as Android's does, the simple ones first and the
    // rest in Android's order, then the open one heads the rows.
    let listed: [&str; 9] = ["Recommended", "Controls", "Audio", "App", "Video", "Display & image", "Streaming", "Advanced", "Debug"];
    let titles: Vec<&str> = f.drawn.iter().map(String::as_str).filter(|t| SETTINGS_SECTIONS.contains(t)).collect();
    assert_eq!(titles[..9], listed, "the list names the sections in order");
    assert_eq!(titles[9..], ["Video"], "and the chosen one is open");
    // A narrow window lists them as chips above the rows, in the same order.
    f.size = vec2(800.0, 12000.0);
    f.settle();
    let titles: Vec<&str> = f.drawn.iter().map(String::as_str).filter(|t| SETTINGS_SECTIONS.contains(t)).collect();
    assert_eq!(titles[..9], listed, "the chips name the sections in order");
}

/// Every button, switch, list and field on the main screens has a name for a screen reader.
#[test]
fn screen_readers_hear_a_name_for_everything_on_the_main_screens() {
    let mut f = Frames::with(record("Desk", true, true), Reach::Online, Some(answer(true, 7)), vec![game(7), game(9)]);
    f.ctx.enable_accesskit();
    f.size = vec2(1280.0, 12000.0);
    // Every row of the settings, each section's fold open.
    f.l.settings_view.more = [true; 9];
    let id = f.hosts.list[0].id;
    let mut screens = vec![("PCs", Screen::Hosts), ("Home", Screen::Home), ("library", Screen::Apps(id)), ("details", Screen::HostDetails(id)), ("PC settings", Screen::HostSettings(id))];
    screens.push(("settings", Screen::Settings));
    let mut missing = Vec::new();
    for (name, screen) in screens {
        f.l.go(screen);
        // Every section of the settings pages, and the search with its Clear.
        let sections: &[&'static str] = if matches!(screen, Screen::Settings | Screen::HostSettings(_)) { &SETTINGS_SECTIONS } else { &[""] };
        for section in sections {
            f.l.settings_view.section = section;
            f.settle();
            assert!(f.access.as_ref().is_some_and(|t| t.nodes.len() > 5), "{name}: a tree came out");
            missing.extend(unnamed(&f).into_iter().map(|w| format!("{name} {section}: {w}")));
        }
        if sections.len() > 1 {
            f.l.settings_view.query = "bitrate".into();
            f.settle();
            missing.extend(unnamed(&f).into_iter().map(|w| format!("{name} searching: {w}")));
            f.l.settings_view.query.clear();
        }
    }
    f.l.go(Screen::Hosts);
    f.l.dialog = Dialog::HostOptions { host: id };
    f.settle();
    missing.extend(unnamed(&f).into_iter().map(|w| format!("options: {w}")));
    assert_eq!(missing, Vec::<String>::new(), "unnamed widgets");
    // The PIN screen: every key, Delete, Confirm and the way out.
    f.l.dialog = Dialog::None;
    let dir = std::env::temp_dir().join(format!("nightjar-ui-pin-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    let store = crate::pin_ui::open_store(&dir);
    store.set_pin(b"1234").unwrap();
    store.set_mode(crate::pin_lock::Mode::App);
    store.flush();
    f.l.pin = Some(crate::pin_ui::PinLock::with_store(store));
    f.settle();
    assert!(f.l.pin.as_ref().unwrap().app_locked());
    assert_eq!(unnamed(&f), Vec::<String>::new(), "the PIN screen");
    // A label's name is its value.
    let labels: Vec<String> = f.access.as_ref().unwrap().nodes.iter().filter_map(|(_, n)| n.label().or(n.value()).map(str::to_owned)).collect();
    for wanted in ["1", "0", "Delete", "Confirm", "Close Nightjar", "0 of 4 digits entered"] {
        assert!(labels.iter().any(|l| l == wanted), "{wanted} in {labels:?}");
    }
    assert!(!labels.iter().any(|l| l.contains("Desk")), "the lock screen hides the PCs from the reader too");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn settings_main_rows_are_androids_tiers() {
    let tiers = include_str!("../../../../../app/src/main/res/values/settings_tiers.xml");
    let parsed: Vec<(&str, Vec<&str>)> = tiers.split("<string-array name=\"settings_tier_").skip(1).map(|array| {
        let (name, body) = array.split_once('"').expect("a named array");
        let body = body.split("</string-array>").next().unwrap_or_default();
        (name, body.split("<item>").skip(1).filter_map(|i| i.split("</item>").next()).collect())
    }).collect();
    let ours: Vec<(&str, Vec<&str>)> = MAIN_KEYS.iter().map(|(name, keys)| (*name, keys.to_vec())).collect();
    assert_eq!(ours, parsed, "MAIN_KEYS follows settings_tiers.xml");
    let preferences = include_str!("../../../../../app/src/main/res/xml/preferences.xml");
    for &(desktop, android) in ANDROID_KEYS {
        // Real VRR has no row of its own on Android either: Variable refresh rate's control writes it (VrrPreference).
        let control = if android == "checkbox_real_vrr" { "checkbox_vrr" } else { android };
        assert!(preferences.contains(&format!("android:key=\"{control}\"")), "{desktop} matches Android's {android}, which exists");
        assert!(!DESKTOP_MAIN.contains(&desktop), "{desktop}: Android decides its tier");
    }
    assert!(main_row("lower_bitrate") && main_row("auto_reconnect") && main_row("window_mode") && main_row("connection_test"));
    assert!(main_row("vrr") && main_row("mouse_mode") && main_row("controllers") && main_row("controller_test") && main_row("open_on_start"));
    assert!(!main_row("ui_zoom") && !main_row("usb_forwarding") && !main_row("swap_face_buttons") && !main_row("pyrowave_pace"));
}

/// Every section draws Android's main rows first, then one row that says how many more it holds;
/// opened, it shows exactly that many, after the main rows, in their groups.
#[test]
fn settings_show_a_sections_main_rows_then_a_counted_fold() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 12000.0);
    video_settings(&mut f);
    assert!(shows(&f, "Lower bitrate") && !shows(&f, "Automatic quality"), "{:?}", f.drawn);
    assert!(!shows(&f, "Recover interrupted streams") && !shows(&f, "Fine-tuning"), "one section at a time, no catch-all group");
    let fold = |f: &Frames| f.drawn.iter().find(|t| (t.starts_with("More ") || t.starts_with("Fewer ")) && *t != "More settings").cloned();
    let id = f.hosts.list[0].id;
    for screen in [Screen::Settings, Screen::HostSettings(id)] {
        f.l.go(screen);
        for section in SETTINGS_SECTIONS {
            if screen != Screen::Settings && (section == "App" || section == "Recommended") { continue; }
            f.l.settings_view.section = section;
            f.l.settings_view.more = [false; 9];
            f.settle();
            let closed = f.l.settings_view.drawn.clone();
            assert!(!closed.is_empty(), "{section} draws rows");
            let any_main = closed.iter().any(|(key, _)| main_row(key));
            assert!(!any_main || closed.iter().all(|(key, folded)| main_row(key) && !folded),
                "{screen:?} {section}: only main rows while closed: {closed:?}");
            let more = fold(&f);
            f.l.settings_view.more = [true; 9];
            f.settle();
            let open = f.l.settings_view.drawn.clone();
            let folded = open.iter().filter(|(_, folded)| *folded).count();
            // A section without main rows here (Advanced without a second GPU) lists its rows directly.
            if any_main && folded > 0 {
                assert_eq!(more, Some(format!("More {section} settings ({folded})")), "{screen:?}: {open:?}");
                assert_eq!(fold(&f), Some(format!("Fewer {section} settings")));
                let first = open.iter().position(|(_, folded)| *folded).unwrap();
                assert!(open[..first].iter().all(|(key, _)| main_row(key)) && open[first..].iter().all(|(key, _)| !main_row(key)), "{screen:?} {section}: {open:?}");
            } else {
                assert_eq!((more, fold(&f)), (None, None), "{screen:?} {section}: no fold over nothing");
            }
        }
    }
    // A PC's own value behind the fold shows in its section like any other row.
    f.l.go(Screen::HostSettings(id));
    f.l.settings_view.section = "Streaming";
    f.l.edit_overrides.capture(&Settings { client_roaming: false, ..Settings::default() }, &["client_roaming"]);
    f.settle();
    assert!(shows(&f, "Keep streaming when the network changes") && shows(&f, "Dual screen mode"), "{:?}", f.drawn);
}

#[test]
fn resolution_and_frame_rate_offer_presets_and_custom_with_its_row() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 12000.0);
    video_settings(&mut f);
    assert!(!shows(&f, "Custom resolution") && !shows(&f, "Custom frame rate"), "Native and Display need no rows");
    // The open menu, under its button, scrolls when it is taller than the space it gets.
    let wheel_over = |f: &mut Frames, button: &str, lines: f32| {
        let at = f.placed.iter().find(|(t, ..)| t == button).expect("the menu's button").1.center() + vec2(0.0, 150.0);
        f.frame(vec![egui::Event::PointerMoved(at)]);
        f.frame(vec![egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: vec2(0.0, lines),
            phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Unknown, modifiers: egui::Modifiers::NONE }]);
        for _ in 0..10 { f.settle(); }
    };
    click_text(&mut f, "Native (1920x1080)");
    let mut offered: Vec<String> = f.drawn.clone();
    wheel_over(&mut f, "Native (1920x1080)", -6.0);
    offered.extend(f.drawn.iter().cloned());
    wheel_over(&mut f, "Native (1920x1080)", -1000.0);
    offered.extend(f.drawn.iter().cloned());
    assert!(offered.contains(&"Native 75% (1440x810)".to_owned()), "75% native is offered: {offered:?}");
    assert!(offered.contains(&"Native 50% (960x540)".to_owned()), "50% native is offered: {offered:?}");
    for (w, h) in RESOLUTIONS {
        assert!(offered.contains(&format!("{w}x{h}")), "{w}x{h} is offered: {offered:?}");
    }
    click_text(&mut f, "Custom…");
    assert_eq!((f.l.settings.width, f.l.settings.height), (1920, 1080), "Custom starts from the native size");
    assert!(shows(&f, "Custom (1920x1080)") && shows(&f, "Custom resolution"), "{:?}", f.drawn);
    // A preset hides the row; a saved size off the menu is Custom with its row.
    click_text(&mut f, "Custom (1920x1080)");
    wheel_over(&mut f, "Custom (1920x1080)", -1000.0);
    click_text(&mut f, "5120x1440");
    assert_eq!((f.l.settings.width, f.l.settings.height), (5120, 1440));
    assert!(shows(&f, "5120x1440") && !shows(&f, "Custom resolution"), "{:?}", f.drawn);
    (f.l.settings.width, f.l.settings.height) = (1720, 720);
    f.settle();
    assert!(shows(&f, "Custom (1720x720)") && shows(&f, "Custom resolution"), "{:?}", f.drawn);

    click_text(&mut f, "Display (60 Hz)");
    let mut offered: Vec<String> = f.drawn.clone();
    wheel_over(&mut f, "Display (60 Hz)", -1000.0);
    offered.extend(f.drawn.iter().cloned());
    for fps in FRAME_RATES {
        assert!(offered.contains(&format!("{fps} fps")), "{fps} fps is offered: {offered:?}");
    }
    click_text(&mut f, "Custom…");
    assert_eq!(f.l.settings.fps, 60, "Custom starts from the display's rate");
    assert!(shows(&f, "Custom (60 fps)") && shows(&f, "Custom frame rate"), "{:?}", f.drawn);
    click_text(&mut f, "Custom (60 fps)");
    click_text(&mut f, "120 fps");
    assert!(!shows(&f, "Custom frame rate"), "{:?}", f.drawn);
    f.l.settings.fps = 75;
    f.settle();
    assert!(shows(&f, "Custom (75 fps)") && shows(&f, "Custom frame rate"), "{:?}", f.drawn);
}

#[test]
fn the_pc_render_resolution_is_named_and_ranged_as_on_android() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 12000.0);
    f.l.settings_view.section = "Display & image";
    f.l.settings_view.more = [true; 9];
    f.l.go(Screen::Settings);
    f.settle();
    assert!(!shows(&f, "PC render resolution"), "with the virtual display off it has nothing to set: {:?}", f.drawn);
    f.l.settings.virtual_display = true;
    f.settle();
    let row = f.placed.iter().find(|(t, ..)| t == "PC render resolution").unwrap_or_else(|| panic!("{:?}", f.drawn)).1;
    f.frame(vec![egui::Event::PointerMoved(row.center())]);
    f.settle();
    assert!(shows(&f, "The size the PC renders at, as a percent of the stream resolution; the video sent keeps its size. Above 100 the PC draws more pixels than it sends, below fewer."), "{:?}", f.drawn);
    // The old name and description went to the Windows scale, a row of its own.
    assert!(!f.drawn.iter().any(|t| t == "Text and interface size on the virtual display."), "{:?}", f.drawn);
    assert!(shows(&f, "PC display scaling"));
    // The slider holds Android's 50-200 %.
    for (set, kept) in [(300, 200), (25, 50), (135, 135)] {
        f.l.settings.virtual_display_scale = set;
        f.settle();
        assert_eq!(f.l.settings.virtual_display_scale, kept, "{set} %");
    }
}

#[test]
fn a_pcs_own_display_scaling_is_offered_only_by_hosts_that_read_it() {
    use nightjar_policy::http::{Availability, Reason};
    assert!(display_scale_gate(Availability::Supported));
    assert!(display_scale_gate(Availability::Unknown), "the main settings and a PC not reached");
    assert!(!display_scale_gate(Availability::Unsupported(None)));
    assert!(!display_scale_gate(Availability::Unsupported(Some(Reason::UpdateHost))), "shown only where it works");
    let info = |extra: &str| {
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\"><hostname>PC</hostname><uniqueid>uid</uniqueid><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>1</PairStatus><currentgame>0</currentgame><state>SUNSHINE_SERVER_FREE</state><Permission>1</Permission>{extra}</root>"
        );
        nightjar_policy::http::ServerInfo::parse(&xml, 47989).expect("the answer parses")
    };
    let rubylight = "<RustHostVersion>3.0.0</RustHostVersion><VirtualDisplayCapable>true</VirtualDisplayCapable><VirtualDisplayDriverReady>true</VirtualDisplayDriverReady>";
    for (host, extra, drawn) in [
        ("Rubylight that reads it", format!("{rubylight}<DisplayScaleVersion>1</DisplayScaleVersion>"), true),
        ("older Rubylight", rubylight.to_owned(), false),
        ("Apollo", String::new(), false),
    ] {
        let mut f = Frames::with(record("PC", true, false), Reach::Online, Some(info(&extra)), Vec::new());
        f.size = vec2(1280.0, 12000.0);
        f.l.settings_view.section = "Display & image";
        f.l.settings_view.more = [true; 9];
        let id = f.hosts.list[0].id;
        f.l.go(Screen::HostSettings(id));
        f.settle();
        assert!(shows(&f, "Display arrangement"), "{host}: {:?}", f.drawn);
        assert_eq!(shows(&f, "PC display scaling"), drawn, "{host}: {:?}", f.drawn);
    }
}

#[test]
fn opening_files_or_host_settings_from_settings_saves_edits_first() {
    let mut f = Frames::new(record("Desk", true, false), Reach::Online);
    f.hosts.list[0].reach = Reach::Online;
    let id = f.hosts.list[0].id;
    f.l.go(Screen::Settings);
    f.l.settings.fps = 120;
    f.settle();

    let mut files_commands = Vec::new();
    let dialog = f.l.choose_host_option(&f.hosts.list[0], HostOption::Files, &mut files_commands);
    assert!(dialog.is_none());
    assert_eq!(files_commands.len(), 2);
    assert!(matches!(files_commands[0], Command::SaveSettings));
    assert!(matches!(files_commands[1], Command::OpenFiles(h) if h == id));

    let mut settings_commands = Vec::new();
    let dialog = f.l.choose_host_option(&f.hosts.list[0], HostOption::Settings, &mut settings_commands);
    assert!(dialog.is_none());
    assert_eq!(settings_commands.len(), 2);
    assert!(matches!(settings_commands[0], Command::SaveSettings));
    assert!(matches!(settings_commands[1], Command::HostSettings(h) if h == id));
}

#[test]
fn the_usbipd_notice_gives_devices_back_only_when_asked() {
    let mut f = every_state().into_iter().find(|s| s.name == "paired").unwrap().frames();
    f.l.ask_after(Dialog::ConfirmUsbGiveBack { count: 2 });
    f.settle();
    assert!(f.drawn.iter().any(|t| t.contains("usbipd-win (2)")), "{:?}", f.drawn);
    let not_now = f.placed.iter().find(|(t, ..)| t == "Not now").map(|p| p.1).expect("Not now");
    let commands = f.click_at(not_now.center(), egui::PointerButton::Primary);
    assert!(!commands.iter().any(|c| matches!(c, Command::GiveUsbDevicesBack)) && f.no_dialog(), "Not now leaves them shared");
    f.l.ask(Dialog::ConfirmUsbGiveBack { count: 1 });
    f.settle();
    let give = f.placed.iter().find(|(t, ..)| t == "Give back").map(|p| p.1).expect("Give back");
    let commands = f.click_at(give.center(), egui::PointerButton::Primary);
    assert!(commands.iter().any(|c| matches!(c, Command::GiveUsbDevicesBack)) && f.no_dialog());
}
