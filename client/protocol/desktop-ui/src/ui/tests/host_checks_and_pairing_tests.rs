//! Remote-access checks, pairing approvals, and incoming PIN dialog tests.

use super::*;

/// Home's setup card: its words while the settings are Auto, nothing with Custom settings (kept
/// for later), the page's first content with the hero and the shelves beneath it, and a close button
/// the arrow keys reach and Enter presses, which clears and saves it.
#[test]
fn the_setup_card_says_what_auto_chose_and_is_dismissed_from_the_keys() {
    let mut f = home_with_games(3);
    let id = f.hosts.list[0].id;
    let before: Vec<(String, Rect)> = f.placed.iter().map(|(t, r, _)| (t.clone(), *r)).collect();
    f.l.settings.setup_card_host = Some(id);
    f.l.settings.setup_card_seen = true;
    f.settle();
    let line = home::setup_line(&f.l.settings, f.l.display);
    assert!(line.ends_with(" · SDR · codec chosen when the stream starts"), "{line}");
    assert!(shows(&f, "Set up for this device and Desk") && shows(&f, &line), "{:?}", f.drawn);
    // It is part of the page: what follows starts beneath it, lower than without it by one amount.
    let title = f.placed.iter().find(|(t, ..)| t == "Set up for this device and Desk").map(|p| p.1).expect("the card is drawn");
    let line_bottom = f.placed.iter().find(|(t, ..)| t == &line).map(|p| p.1.bottom()).expect("its line");
    // Every text drawn once both times either kept its place (the chrome) or moved down by the card's height and gap.
    let once = |placed: &[(String, Rect)], text: &str| {
        let mut all = placed.iter().filter(|(t, _)| t == text);
        all.next().filter(|_| all.next().is_none()).map(|p| p.1)
    };
    let after: Vec<(String, Rect)> = f.placed.iter().map(|(t, r, _)| (t.clone(), *r)).collect();
    let moved: Vec<(String, f32, f32)> = before.iter()
        .filter_map(|(text, _)| Some((text.clone(), once(&before, text)?.top(), once(&after, text)?.top())))
        .filter(|(_, was, now)| (now - was).abs() > 0.5)
        .collect();
    let pushed = moved.first().map(|m| m.2 - m.1).expect("the page's content moved");
    assert!(pushed > title.height(), "down by the card: {moved:?}");
    for (text, was, now) in &moved {
        assert!((now - was - pushed).abs() < 0.5 && *now > line_bottom, "{text} starts beneath the card, as far down as the rest: {was} {now}");
    }

    // Custom settings: no card, and it waits.
    f.l.settings.fps = 30;
    f.settle();
    assert!(!shows(&f, "Set up for this device and Desk") && f.l.settings.setup_card_host == Some(id));
    f.l.settings.fps = Settings::default().fps;
    f.settle();
    assert!(shows(&f, "Set up for this device and Desk"));

    // The arrow keys (a controller's D-pad) reach its close button; Enter dismisses it for good.
    let mut reached = false;
    let mut walked = Vec::new();
    for step in 0..36 {
        f.press(if step < 6 { Key::ArrowUp } else { Key::ArrowDown });
        let focused = f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect);
        walked.push(focused);
        // The close button ends the card, right of its words' start and on their line.
        if focused.is_some_and(|r| r.width() < 50.0 && r.left() > title.left() && (r.center().y - title.center().y).abs() < 30.0) {
            reached = true;
            break;
        }
    }
    assert!(reached, "the arrow keys reach the card's Dismiss: title {title:?}, focus went {walked:?}");
    let commands = f.press(Key::Enter);
    assert_eq!(f.l.settings.setup_card_host, None);
    assert!(f.l.settings.setup_card_seen, "it never returns");
    assert!(commands.iter().any(|c| matches!(c, Command::SaveSettings)), "and that is saved");
    assert!(!shows(&f, "Set up for this device and Desk"));
}

/// Home with the setup card after the first pairing:
/// `NIGHTJAR_SHOTS=<dir> cargo test -p nightjar-desktop setup_card_screenshot -- --ignored`.
#[test]
#[ignore = "writes PNGs; set NIGHTJAR_SHOTS"]
fn setup_card_screenshot() {
    let Some(dir) = std::env::var_os("NIGHTJAR_SHOTS").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&dir).expect("the folder for the screenshots");
    let mut f = home_with_games(8);
    f.l.settings.setup_card_host = Some(f.hosts.list[0].id);
    f.settle();
    f.shoot(&dir, "desktop-home-setup-card");
}

/// The card waits while a device asks to pair: that banner goes first.
#[test]
fn the_setup_card_yields_to_a_device_waiting_to_pair() {
    let mut f = approving_frames(true);
    let id = f.hosts.list[0].id;
    f.l.screen = Screen::Home;
    f.l.settings.setup_card_host = Some(id);
    let name = format!("Set up for this device and {}", f.hosts.list[0].display_name());
    f.settle();
    assert!(shows(&f, &name), "{:?}", f.drawn);
    f.hosts.list[0].native.pairing_requests = vec![tv_request("a1")];
    f.settle();
    assert!(shows(&f, "TV wants to pair") && !shows(&f, &name));
}

/// The card's line: the game page's resolution, rate and bitrate, then HDR or SDR as the start asks,
/// then the codec: negotiated under Auto, named when forced.
#[test]
fn the_setup_cards_line_names_hdr_and_a_forced_codec() {
    let display = Display { width: 1920, height: 1080, refresh_hz: 60.0, refresh_reported: true };
    let (w, h, fps, kbps) = stream_mode(&Settings::default(), display);
    let summary = console::stream_summary(w, h, fps, kbps);
    assert_eq!(home::setup_line(&Settings::default(), display), format!("{summary} · SDR · codec chosen when the stream starts"));
    assert_eq!(home::setup_line(&Settings { hdr: true, ..Settings::default() }, display), format!("{summary} · HDR · codec chosen when the stream starts"));
    assert_eq!(home::setup_line(&Settings { codec: CodecPreference::Hevc, ..Settings::default() }, display), format!("{summary} · SDR · HEVC"));
    crate::strings::with_lang(crate::strings::Lang::De, || {
        assert!(home::setup_line(&Settings::default(), display).ends_with(" · SDR · Codec wird beim Start des Streams gewählt"));
    });
}

/// nightjar#51: the Home grid follows the focus down and back up when the arrow keys (or the
/// D-pad, which sends the same keys) move it, as it already did for Tab.
#[test]
fn home_scrolls_to_the_game_the_arrow_keys_focus() {
    let mut f = home_with_games(36);
    let focused = |f: &Frames| {
        let id = f.ctx.memory(|m| m.focused()).expect("a game is focused");
        (id, f.ctx.read_response(id).expect("the focused widget was drawn").rect)
    };
    let (start, first) = focused(&f);
    assert!(first.top() >= 0.0 && first.bottom() <= 720.0, "the first game starts on screen: {first:?}");
    let mut seen = vec![start];
    // Scrolling animates for up to 0.3 s; the frames run 0.05 s apart.
    let scrolled = |f: &mut Frames| {
        f.settle();
        f.settle();
    };
    for step in 0..8 {
        f.press(Key::ArrowDown);
        scrolled(&mut f);
        let (id, rect) = focused(&f);
        if id == seen[seen.len() - 1] {
            break;
        }
        seen.push(id);
        assert!(rect.top() >= chrome::PAGE_TOP - 8.0 && rect.bottom() <= 720.0, "row {step}: the focused game is clear of the chrome: {rect:?}");
    }
    assert!(seen.len() >= 4, "the focus walked down past the first screen: {} rows", seen.len());
    for step in 0..seen.len() - 1 {
        f.press(Key::ArrowUp);
        scrolled(&mut f);
        let (_, rect) = focused(&f);
        assert!(rect.top() >= 0.0 && rect.bottom() <= 720.0, "back up {step}: on screen: {rect:?}");
    }
    // Up from the top reaches the chrome; the page is back at its start under it.
    for _ in 0..6 {
        let (_, rect) = focused(&f);
        if rect.top() < chrome::PAGE_TOP - 8.0 {
            break;
        }
        f.press(Key::ArrowUp);
        scrolled(&mut f);
    }
    let play = f.ctx.read_response(start).expect("Play is drawn").rect;
    assert!(play.top() >= chrome::PAGE_TOP, "back at the top: {play:?}, focus on {:?}", focused(&f));
}

/// A cover with art names its game under it, focused or not, so a row reads at a glance; a cover
/// without art shows the whole title inside it, once.
#[test]
fn covers_name_the_game_inside_without_art_and_under_art_always() {
    let mut f = home_with_games(3);
    let count = |f: &Frames, title: &str| f.drawn.iter().filter(|t| *t == title).count();
    // Game 00 is the hero: its name stands beside its cover, and its shelf leaves it out.
    assert_eq!((count(&f, "Game 01"), count(&f, "Game 02")), (1, 1), "without art each title shows once: {:?}", f.drawn);
    let texture = f.ctx.load_texture("art", egui::ColorImage::new([3, 4], vec![Color32::GRAY; 12]), egui::TextureOptions::LINEAR);
    let pc = &mut f.hosts.list[0];
    let app_ids: Vec<i32> = Arc::make_mut(pc.native.library.as_mut().expect("a library"))
        .games
        .iter_mut()
        .map(|g| {
            g.artwork = rubylight_native_api::library::Artwork { cover: Some(format!("/cover/{}", g.app_id)), version: "1".into() };
            g.app_id as i32
        })
        .collect();
    for app_id in app_ids {
        Arc::make_mut(&mut pc.art).insert(app_id, Art::Ready(texture.clone()));
    }
    f.settle();
    assert_eq!((count(&f, "Game 01"), count(&f, "Game 02")), (1, 1), "every cover is named under its art: {:?}", f.drawn);
    let under = |f: &Frames, title: &str| f.placed.iter().find(|(t, ..)| t == title).map(|p| p.1).expect(title);
    let before = under(&f, "Game 01");
    f.press(Key::ArrowDown);
    f.press(Key::ArrowRight);
    f.settle();
    assert_eq!((count(&f, "Game 01"), count(&f, "Game 02")), (1, 1), "focus moving names nothing twice: {:?}", f.drawn);
    assert_eq!(under(&f, "Game 01").top(), before.top(), "the title stays put when its cover is focused and lifts");
}

/// Cover art that is not 3:4 fills its box without being squashed: its middle shows, cut at the
/// sides (wide art) or at top and bottom (tall art).
#[test]
fn cover_art_fills_its_box_cropped_never_squashed() {
    let close = |uv: Rect, x: (f32, f32), y: (f32, f32)| {
        (uv.min.x - x.0).abs() < 1e-4 && (uv.max.x - x.1).abs() < 1e-4 && (uv.min.y - y.0).abs() < 1e-4 && (uv.max.y - y.1).abs() < 1e-4
    };
    let poster = vec2(3.0, 4.0);
    assert!(close(cover_uv(vec2(300.0, 400.0), poster), (0.0, 1.0), (0.0, 1.0)), "3:4 art shows whole");
    assert!(close(cover_uv(vec2(600.0, 400.0), poster), (0.25, 0.75), (0.0, 1.0)), "wide art loses its sides");
    assert!(close(cover_uv(vec2(300.0, 800.0), poster), (0.0, 1.0), (0.25, 0.75)), "tall art loses top and bottom");
    // And Home draws it that way: the art's texture is sampled from its middle half only.
    let mut f = home_with_games(1);
    let texture = f.ctx.load_texture("wide", egui::ColorImage::new([6, 4], vec![Color32::GRAY; 24]), egui::TextureOptions::LINEAR);
    let pc = &mut f.hosts.list[0];
    let app_id = {
        let g = &mut Arc::make_mut(pc.native.library.as_mut().expect("a library")).games[0];
        g.artwork = rubylight_native_api::library::Artwork { cover: Some("/cover/0".into()), version: "1".into() };
        g.app_id as i32
    };
    Arc::make_mut(&mut pc.art).insert(app_id, Art::Ready(texture.clone()));
    f.settle();
    let primitives = f.ctx.tessellate(f.shapes.clone(), f.ctx.pixels_per_point());
    let us: Vec<f32> = primitives
        .iter()
        .filter_map(|p| match &p.primitive {
            egui::epaint::Primitive::Mesh(mesh) if mesh.texture_id == texture.id() => Some(mesh.vertices.iter().map(|v| v.uv.x)),
            _ => None,
        })
        .flatten()
        .collect();
    assert!(!us.is_empty(), "the cover was drawn");
    let (lo, hi) = us.iter().fold((f32::MAX, f32::MIN), |(lo, hi), u| (lo.min(*u), hi.max(*u)));
    assert!(lo > 0.2 && hi < 0.8, "the wide art is cropped to its middle: u from {lo} to {hi}");
}

/// Every dialog lays its buttons out the same way: the one primary action at the right edge,
/// Cancel leftmost, all on one line at the foot; Escape (B on a controller) cancels.
#[test]
fn dialogs_put_the_primary_action_right_and_cancel_left_and_escape_cancels() {
    let mut f = Frames::new(record("Desk", true, true), Reach::Online);
    let id = f.hosts.list[0].id;
    let dialogs = [
        ("Save", Dialog::Rename { host: id, text: String::new() }),
        ("Remove PC", Dialog::ConfirmRemove { host: id, name: "Desk".into() }),
        ("Unpair", Dialog::ConfirmUnpair { host: id, name: "Desk".into() }),
        ("Add", Dialog::AddHost { text: String::new(), error: None }),
        ("Quit game", Dialog::ConfirmClose { host: id, name: "Game".into() }),
    ];
    let buttons = ["Save", "Remove PC", "Unpair", "Add", "Quit game", "Paste link", "Cancel"];
    for (primary, dialog) in dialogs {
        f.l.dialog = dialog;
        f.l.focus_dialog = true;
        f.settle();
        let placed: Vec<Rect> = f.placed.iter().filter(|(t, ..)| buttons.contains(&t.as_str())).map(|p| p.1).collect();
        let at = |label: &str| f.placed.iter().find(|(t, ..)| t == label).map(|p| p.1).unwrap_or_else(|| panic!("{primary}: {label} is drawn"));
        let (main, cancel) = (at(primary), at("Cancel"));
        assert!(placed.iter().all(|r| r.right() <= main.right() && r.left() >= cancel.left()), "{primary}: primary right, Cancel left: {placed:?}");
        assert!(placed.iter().all(|r| (r.center().y - main.center().y).abs() < 2.0), "{primary}: one line of buttons: {placed:?}");
        let commands = f.press(Key::Escape);
        assert!(f.no_dialog() && commands.is_empty(), "{primary}: Escape cancels");
    }
}

/// Settings' scroll bar sits next to its rows, and the wheel scrolls them from anywhere beside.
#[test]
fn settings_scroll_beside_their_column_on_a_wide_window() {
    let mut f = Frames::new(record("Desk", true, true), Reach::Online);
    f.size = vec2(1920.0, 640.0);
    f.l.settings_view.more = [true; 9];
    video_settings(&mut f);
    let top = |f: &Frames| f.placed.iter().find(|(t, ..)| t == "Video codec").map(|p| p.1.top()).expect("the Video codec row");
    let before = top(&f);
    let beside = Pos2::new(1500.0, 600.0);
    f.frame(vec![egui::Event::PointerMoved(beside)]);
    f.frame(vec![egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, -200.0),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
        modifiers: egui::Modifiers::NONE,
    }]);
    for _ in 0..5 {
        f.settle();
    }
    assert!(top(&f) < before - 50.0, "the wheel beside the rows scrolls them: {before} to {}", top(&f));
}

/// A controller's mapping scrolls as settings do: the wheel beside it scrolls it.
#[test]
fn controller_mappings_scroll_beside_their_column() {
    let mut f = Frames::new(record("Desk", true, true), Reach::Online);
    f.size = vec2(1920.0, 640.0);
    f.l.go(Screen::Controllers);
    f.l.controller_key = "pad".into();
    f.l.controller_mapping = crate::gamepad_types::Mapping { name: "Pad".into(), ..Default::default() };
    f.settle();
    let top = |f: &Frames| f.placed.iter().find(|(t, ..)| t == "Axes").map(|p| p.1.top()).expect("the axes under Connect as");
    let before = top(&f);
    f.frame(vec![egui::Event::PointerMoved(Pos2::new(1500.0, 600.0))]);
    f.frame(vec![egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, -200.0),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
        modifiers: egui::Modifiers::NONE,
    }]);
    for _ in 0..5 {
        f.settle();
    }
    assert!(top(&f) < before - 50.0, "the wheel beside the rows scrolls them: {before} to {}", top(&f));
}

/// Home's PC card says a PC's state once: an offline PC reads "Offline", never "Offline · Offline".
#[test]
fn the_home_pc_card_says_the_state_once() {
    let mut f = home_with_games(1);
    f.hosts.list[0].reach = Reach::Offline;
    f.settle();
    let lines: Vec<&String> = f.drawn.iter().filter(|t| t.contains("Offline")).collect();
    assert!(!lines.is_empty() && lines.iter().all(|t| t.matches("Offline").count() == 1), "{lines:?} in {:?}", f.drawn);
    assert!(f.placed.iter().any(|(t, r, _)| t == "PCs" && r.center().y > 80.0), "the shelf is Android's \"PCs\": {:?}", f.drawn);
}

/// As Android's game page: the state in a chip over the title and the PC on its own line, not a
/// "Running on …" sentence; a game that isn't running has no chip.
#[test]
fn the_game_page_says_its_state_in_a_chip_and_the_pc_on_its_own_line() {
    let mut f = home_with_games(2);
    let pc = f.hosts.list[0].id;
    f.hosts.list[0].info = Some(answer(true, 1000));
    f.hosts.list[0].apps = Apps::Loaded(Arc::new(vec![nightjar_policy::http::App { name: "Game 00".into(), uuid: "install-0".into(), ..game(1000) }]));
    Arc::make_mut(f.hosts.list[0].native.library.as_mut().expect("the library")).games[0].running = true;
    f.l.go(Screen::Game { host: pc, app: 1000 });
    f.settle();
    // The last of a text: the cover without art shows the title too, before the page does.
    let at = |f: &Frames, text: &str| f.placed.iter().rfind(|(t, r, _)| t == text && r.top() > chrome::PAGE_TOP).map(|p| p.1).unwrap_or_else(|| panic!("{text} in {:?}", f.drawn));
    let (chip, title, line) = (at(&f, "Running"), at(&f, "Game 00"), at(&f, "Desk"));
    assert!(chip.bottom() <= title.top() && title.bottom() <= line.top(), "chip, title, PC: {chip:?} {title:?} {line:?}");
    assert!(!f.drawn.iter().any(|t| t.starts_with("Running on")), "{:?}", f.drawn);
    f.l.go(Screen::Game { host: pc, app: 1001 });
    f.settle();
    assert!(shows(&f, "Desk") && !shows(&f, "Running") && !shows(&f, "Suspended"), "{:?}", f.drawn);
}

/// A game's page: the title over one Play with Add to favourites on its line, its rows under them,
/// and no sentence with nothing to know. Quit game only for a game that runs.
#[test]
fn a_games_page_is_one_play_beside_favourite_over_its_rows() {
    let mut f = home_with_games(2);
    let pc = f.hosts.list[0].id;
    f.hosts.list[0].apps = Apps::Loaded(Arc::new(vec![nightjar_policy::http::App { name: "Game 00".into(), uuid: "install-0".into(), ..game(1000) }]));
    f.l.go(Screen::Game { host: pc, app: 1000 });
    f.settle();
    let at = |f: &Frames, text: &str| f.placed.iter().rfind(|(t, r, _)| t == text && r.top() > chrome::PAGE_TOP).map(|p| p.1).unwrap_or_else(|| panic!("{text} in {:?}", f.drawn));
    let (title, play, star, settings) = (at(&f, "Game 00"), at(&f, "Play"), at(&f, "Add to favourites"), at(&f, "Stream settings"));
    assert!(title.bottom() <= play.top() && play.bottom() <= settings.top(), "{title:?} {play:?} {settings:?}");
    assert!((play.center().y - star.center().y).abs() < 2.0 && play.right() < star.left(), "{play:?} {star:?}");
    assert!(!shows(&f, "Quit game"), "{:?}", f.drawn);
    let focused = f.ctx.memory(|m| m.focused()).and_then(|i| f.ctx.read_response(i)).expect("Play has focus").rect;
    assert!(focused.contains(play.center()), "{focused:?}");
    // The rows do what they say.
    let commands = f.click_at(at(&f, "Hide game").center(), egui::PointerButton::Primary);
    assert!(commands.iter().any(|c| matches!(c, Command::SetHidden { hidden: true, games } if !games.is_empty())));
    f.settle();
    let commands = f.click_at(at(&f, "Quit game when I leave").center(), egui::PointerButton::Primary);
    assert!(commands.iter().any(|c| matches!(c, Command::SetQuitOnLeave { enabled: true, .. })));
    f.settle();
    let commands = f.click_at(at(&f, "Stream settings").center(), egui::PointerButton::Primary);
    assert!(commands.iter().any(|c| matches!(c, Command::AppSettings { .. })));
    // Running, the hero's words change in place and Quit game joins them.
    f.settle();
    f.hosts.list[0].info = Some(answer(true, 1000));
    Arc::make_mut(f.hosts.list[0].native.library.as_mut().expect("the library")).games[0].running = true;
    f.settle();
    let (resume, quit) = (at(&f, "Resume"), at(&f, "Quit game"));
    assert!((resume.center().y - quit.center().y).abs() < 2.0, "{resume:?} {quit:?}");
    assert_eq!(at(&f, "Stream settings").top(), settings.top(), "no jump for a chip that was not there");
}

/// nightjar#61: the game's page switches the profile before the stream starts.
#[test]
fn a_games_page_offers_the_profile_only_with_profiles() {
    let mut f = game_page_with_profiles(&[]);
    assert!(!shows(&f, "Active profile"), "{:?}", f.drawn);
    f.l.profiles.entries.push(crate::config::Profile::new(1, "Quiet", Default::default()));
    f.settle();
    assert!(shows(&f, "Active profile") && shows(&f, "Main settings"), "{:?}", f.drawn);
    f.l.profiles.active = Some(1);
    f.settle();
    assert!(shows(&f, "Active profile") && shows(&f, "Quiet"), "{:?}", f.drawn);
}

#[test]
fn picking_a_profile_on_the_games_page_sets_and_saves_it() {
    let mut f = game_page_with_profiles(&["Quiet", "Sharp"]);
    let at = last_text_at(&f, "Active profile");
    f.click_at(at, egui::PointerButton::Primary);
    f.settle();
    assert!(shows(&f, "Quiet") && shows(&f, "Sharp"), "the list names each profile: {:?}", f.drawn);
    let at = last_text_at(&f, "Sharp");
    let commands = f.click_at(at, egui::PointerButton::Primary);
    assert_eq!(f.l.profiles.active, Some(2));
    assert!(commands.iter().any(|c| matches!(c, Command::SaveProfiles)), "{} commands", commands.len());
    f.settle();
    let at = last_text_at(&f, "Active profile");
    f.click_at(at, egui::PointerButton::Primary);
    f.settle();
    let at = last_text_at(&f, "Main settings");
    f.click_at(at, egui::PointerButton::Primary);
    assert_eq!(f.l.profiles.active, None, "Main settings is a choice too");
}

#[test]
fn a_network_rule_decides_the_profile_and_the_row_opens_the_profiles_page() {
    let mut f = game_page_with_profiles(&["Quiet", "Sharp"]);
    f.l.network.wifi = true;
    f.l.profiles.automatic = true;
    f.l.profiles.active = Some(1);
    f.l.profiles.wifi = Some(2);
    f.settle();
    assert!(shows(&f, "Sharp") && !shows(&f, "Quiet"), "the next stream uses the rule's profile: {:?}", f.drawn);
    let at = last_text_at(&f, "Active profile");
    f.click_at(at, egui::PointerButton::Primary);
    f.settle();
    assert_eq!(f.l.screen, Screen::Profiles);
    assert_eq!(f.l.profiles.active, Some(1), "nothing was changed");
}

/// The game page's profile row and its list (nightjar#61):
/// `NIGHTJAR_SHOTS=<dir> cargo test -p nightjar-desktop game_profile_screenshots -- --ignored`.
#[test]
#[ignore = "writes PNGs; set NIGHTJAR_SHOTS"]
fn game_profile_screenshots() {
    let Some(dir) = std::env::var_os("NIGHTJAR_SHOTS").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&dir).expect("the folder for the screenshots");
    let mut f = game_page_with_profiles(&["Couch", "Hotel Wi-Fi"]);
    f.size = vec2(1280.0, 800.0);
    f.l.profiles.active = Some(1);
    f.settle();
    f.shoot(&dir, "game-profile");
    let at = last_text_at(&f, "Active profile");
    f.click_at(at, egui::PointerButton::Primary);
    f.settle();
    f.shoot(&dir, "game-profile-list");
}

/// Start or the Settings link over a game page keeps its Neural Rendering open, as the PC
/// checks and Files keep theirs: Back finds the panel as it was left.
#[test]
fn settings_opened_over_a_game_page_keeps_its_neural_panel() {
    let mut f = game_page_with_neural(false, rubylight_native_api::radeon::ControlState::Configurable, None);
    let pc = f.hosts.list[0].id;
    f.l.open_settings();
    assert_eq!(f.l.screen, Screen::Settings);
    f.settle();
    assert!(f.l.neural.state_for(pc, 1000).is_some_and(|open| open.state.is_some()), "Settings closed the panel");
    let mut commands = Vec::new();
    f.l.back(&mut commands, true);
    assert_eq!(f.l.screen, Screen::Game { host: pc, app: 1000 });
    f.settle();
    assert!(f.l.neural.state_for(pc, 1000).is_some_and(|open| open.state.is_some()), "Back lost the panel");
    // Leaving the page still closes it.
    f.l.back(&mut commands, true);
    f.settle();
    assert!(f.l.neural.state_for(pc, 1000).is_none());
}

/// With two game pages on the trail (game A, the search, game B), Settings over B keeps B's
/// panel, not A's.
#[test]
fn settings_over_the_newest_game_page_keeps_that_pages_neural_panel() {
    let mut f = game_page_with_neural(false, rubylight_native_api::radeon::ControlState::Configurable, None);
    let pc = f.hosts.list[0].id;
    let neural = f.l.neural.state_for(pc, 1000).and_then(|open| open.state.clone()).unwrap();
    f.hosts.list[0].apps = Apps::Loaded(std::sync::Arc::new(vec![
        nightjar_policy::http::App { name: "Game 00".into(), uuid: "install-0".into(), ..game(1000) },
        nightjar_policy::http::App { name: "Game 01".into(), uuid: "install-1".into(), ..game(1001) },
    ]));
    f.l.open(Screen::AllGames);
    f.l.open(Screen::Game { host: pc, app: 1001 });
    f.l.neural.open(pc, 1001, "install-1".into(), std::sync::Arc::new(FixedNeural(std::sync::Mutex::new(neural))), std::sync::Arc::new(|| {}));
    for _ in 0..400 {
        f.l.neural.poll(Some((pc, 1001)));
        if f.l.neural.state_for(pc, 1001).is_some_and(|open| open.state.is_some()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    f.l.open_settings();
    f.settle();
    assert!(f.l.neural.state_for(pc, 1001).is_some_and(|open| open.state.is_some()), "Settings closed game B's panel");
}

/// A window that changes size on a game page moves its moon: no ghost of the old place fades
/// behind the settings card, at any frame.
#[test]
fn a_game_page_that_changes_size_paints_one_moon() {
    let mut f = home_with_games(1);
    let pc = f.hosts.list[0].id;
    f.hosts.list[0].apps = Apps::Loaded(Arc::new(vec![nightjar_policy::http::App { name: "Game 00".into(), uuid: "install-0".into(), ..game(1000) }]));
    Arc::make_mut(&mut f.hosts.list[0].art).insert(1000, Art::Missing);
    f.l.go(Screen::Game { host: pc, app: 1000 });
    for _ in 0..6 {
        f.settle();
    }
    f.size = vec2(1280.0, 1000.0);
    for n in 0..24 {
        f.frame(Vec::new());
        let moons = backdrop::painted(&f.ctx).moons;
        assert!(moons.len() <= 1, "frame {n}: {moons:?}");
    }
}

/// The frame generation row's line is sentences a player reads whole: it wraps, never cut.
#[test]
fn the_frame_generation_row_shows_its_whole_line() {
    use rubylight_native_api::radeon::ControlState;
    let f = game_page_with_neural(false, ControlState::Configurable, None);
    let about = f.placed.iter().find(|(text, _, _)| text.starts_with("Frame generation for games that offer DLSS frame generation")).unwrap_or_else(|| panic!("{:?}", f.drawn));
    assert!(about.1.height() > 2.0 * super::design_tokens::TYPE_SUB_SIZE, "one line, cut: {:?}", about.1);
    let f = game_page_with_neural(false, ControlState::ReadOnly, Some("Turn on Host control for this device in the Rubylight console to change it."));
    assert!(shows(&f, "Off · Turn on Host control for this device in the Rubylight console to change it."), "{:?}", f.drawn);
}

/// The game page's frame generation row:
/// `NIGHTJAR_SHOTS=<dir> cargo test -p nightjar-desktop frame_generation_screenshots -- --ignored`.
#[test]
#[ignore = "writes PNGs; set NIGHTJAR_SHOTS"]
fn frame_generation_screenshots() {
    use rubylight_native_api::radeon::ControlState;
    let Some(dir) = std::env::var_os("NIGHTJAR_SHOTS").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&dir).expect("the folder for the screenshots");
    game_page_with_neural(false, ControlState::Configurable, None).shoot(&dir, "frame-generation-off");
    game_page_with_neural(true, ControlState::Configurable, None).shoot(&dir, "frame-generation-on");
    game_page_with_neural(false, ControlState::ReadOnly, Some("Turn on Host control for this device in the Rubylight console to change it.")).shoot(&dir, "frame-generation-needs-host-control");
    game_page_with_neural(false, ControlState::TemporarilyUnavailable, Some("Frame generation downloads about 14 MB and copies it into the game's folder: turn it on when no stream is running.")).shoot(&dir, "frame-generation-streaming");
    // The harness's default window: the taller row still leaves the page whole.
    let mut f = game_page_with_neural(false, ControlState::TemporarilyUnavailable, Some("Frame generation downloads about 14 MB and copies it into the game's folder: turn it on when no stream is running."));
    f.size = vec2(1280.0, 720.0);
    f.settle();
    f.shoot(&dir, "frame-generation-streaming-720");
}

/// Play pressed before the PC's app list arrives holds, then starts when it lands; it asks for it too.
#[test]
fn play_before_the_list_arrives_starts_when_it_lands() {
    let mut f = home_with_games(1);
    let pc = f.hosts.list[0].id;
    f.hosts.list[0].apps = Apps::NotLoaded;
    f.l.go(Screen::Game { host: pc, app: 1000 });
    f.settle();
    let commands = f.press(Key::Enter);
    assert!(commands.iter().any(|c| matches!(c, Command::LoadApps(host) if *host == pc)));
    assert!(!commands.iter().any(|c| matches!(c, Command::Launch { .. })));
    assert!(f.l.playing_soon(pc, 1000));
    f.hosts.list[0].apps = Apps::Loading;
    let commands = f.frame(Vec::new());
    assert!(!commands.iter().any(|c| matches!(c, Command::Launch { .. })), "still on its way");
    f.hosts.list[0].apps = Apps::Loaded(Arc::new(vec![nightjar_policy::http::App { name: "Game 00".into(), uuid: "install-0".into(), ..game(1000) }]));
    let commands = f.frame(Vec::new());
    assert!(commands.iter().any(|c| matches!(c, Command::Launch { host, app: 1000, resume: false } if *host == pc)));
    assert!(!f.l.playing_soon(pc, 1000));
    let commands = f.frame(Vec::new());
    assert!(!commands.iter().any(|c| matches!(c, Command::Launch { .. })), "once");
}

/// A Play that waited says why when it cannot start, and leaving the page lets it go.
#[test]
fn play_before_the_list_is_loud_when_it_cannot_start_and_ends_with_the_page() {
    let press = |apps: Apps| {
        let mut f = home_with_games(1);
        let pc = f.hosts.list[0].id;
        f.hosts.list[0].apps = Apps::Loading;
        f.l.go(Screen::Game { host: pc, app: 1000 });
        f.settle();
        f.press(Key::Enter);
        assert!(f.l.playing_soon(pc, 1000));
        f.hosts.list[0].apps = apps;
        f
    };
    let mut f = press(Apps::Failed("The PC did not answer".into()));
    f.settle();
    assert!(shows(&f, "The stream could not start") && shows(&f, "The app list could not be loaded. The PC did not answer"), "{:?}", f.drawn);
    let mut f = press(Apps::Loaded(Arc::new(vec![game(77)])));
    f.settle();
    assert!(shows(&f, "The stream could not start") && shows(&f, "The PC’s app list has no entry for this game."), "{:?}", f.drawn);
    // Gone from the page, nothing starts behind the player's back.
    let mut f = press(Apps::Loading);
    f.l.go(Screen::Home);
    f.frame(Vec::new());
    f.hosts.list[0].apps = Apps::Loaded(Arc::new(vec![nightjar_policy::http::App { name: "Game 00".into(), uuid: "install-0".into(), ..game(1000) }]));
    let commands = f.frame(Vec::new());
    assert!(!commands.iter().any(|c| matches!(c, Command::Launch { .. })));
}

/// Down from the search field goes to the games it filters, as a controller's D-pad does.
#[test]
fn down_from_the_search_field_reaches_the_games() {
    let mut f = home_with_games(4);
    f.l.go(Screen::AllGames);
    f.settle();
    f.l.open_search();
    f.settle();
    assert!(f.ctx.text_edit_focused(), "the field has the keyboard");
    f.press(Key::ArrowDown);
    f.settle();
    assert!(!f.ctx.text_edit_focused(), "Down left the field");
    let focused = f.ctx.memory(|m| m.focused()).and_then(|i| f.ctx.read_response(i)).expect("something has focus").rect;
    assert!(focused.top() > chrome::PAGE_TOP, "a game below the bar: {focused:?}");
}

/// Home's PC card's halo may leave the scroll area, whose edge would cut it: it is deferred, in a clip
/// wider than the area's.
#[test]
fn a_host_cards_halo_is_deferred_past_the_scroll_edge() {
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    let hosts = Hosts::without_discovery(vec![record("Den", true, true)], Arc::new(|| {}));
    let mut deferred = None;
    for pass in 0..2 {
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let area = ui.clip_rect();
            let card = host_card(ui, &hosts.list[0], "Online", 300.0);
            if pass == 0 {
                card.request_focus();
            } else {
                deferred = Some((area, glass::take_deferred(ui)));
            }
        });
        let mut delta = output.textures_delta;
        delta.clear();
    }
    let (area, deferred) = deferred.expect("two passes");
    assert_eq!(deferred.len(), 1, "the halo");
    assert!(deferred[0].0.width() > area.width() && !deferred[0].1.is_empty(), "{:?}", deferred[0].0);
}
