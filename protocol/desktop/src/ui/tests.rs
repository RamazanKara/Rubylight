//! The launcher's tests: what each page and dialog draws, its focus and its commands.

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

fn launcher(console_home: bool, home_ready: bool) -> Launcher {
    let settings = Settings { console_home, ..Default::default() };
    let display = Display { width: 1920, height: 1080, refresh_hz: 60.0, refresh_reported: true };
    let mut launcher = Launcher::new(settings, Profiles::default(), crate::shortcuts::Shortcuts::default(), display);
    launcher.home_ready = home_ready;
    launcher
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
    l.dialog = Dialog::ContinueHere { host: 1, app: 7, resume: true, name: "Game".into(), device: "Phone".into() };
    l.ask_after(moved());
    assert!(matches!(l.dialog, Dialog::ContinueHere { .. }));
    l.dialog = Dialog::None;
    assert!(l.next_queued());
    assert!(matches!(l.dialog, Dialog::NowPlaying { .. }));
    assert!(!l.next_queued());
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
    let playing: nightjar_core::http::PlayingOn = serde_json::from_str(
        r#"{"device":"Phone","app":{"install_id":"game-a","app_id":7,"title":"Game"}}"#,
    ).unwrap();
    let tile = nightjar_core::http::App { control: nightjar_core::http::AppControl::Resume, ..game(9) };
    assert_eq!(super::join_target(Some(&tile), 9, Some(&playing)), 7);
    assert_eq!(super::join_target(Some(&tile), 9, None), 9, "nobody plays it: the PC decides");
    assert_eq!(super::join_target(Some(&game(9)), 9, Some(&playing)), 9);
    assert_eq!(super::join_target(None, 9, Some(&playing)), 9);
}

#[test]
fn the_prompt_on_a_tile_starts_the_game_itself() {
    let playing: nightjar_core::http::PlayingOn = serde_json::from_str(
        r#"{"device":"Phone","app":{"install_id":"game-a","app_id":7,"title":"Game"}}"#,
    ).unwrap();
    let tile = nightjar_core::http::App { control: nightjar_core::http::AppControl::RunningGame, ..game(9) };
    assert_eq!(super::asked_start(Some(&tile), 9, false, Some(&playing)), (7, true), "a resume of the tile's id is refused");
    assert_eq!(super::asked_start(Some(&game(7)), 7, true, Some(&playing)), (7, true));
    assert_eq!(super::asked_start(Some(&game(7)), 7, false, Some(&playing)), (7, false));
}

#[test]
fn the_game_another_device_plays_is_matched_by_id_and_install_id() {
    let playing: nightjar_core::http::PlayingOn = serde_json::from_str(
        r#"{"device":"Phone","app":{"install_id":"game-a","app_id":7,"title":"Game"}}"#,
    ).unwrap();
    let app = |id, uuid: &str| nightjar_core::http::App { uuid: uuid.into(), ..game(id) };
    assert!(super::library::plays_this_game(Some(&app(7, "game-a")), 7, &playing));
    assert!(!super::library::plays_this_game(Some(&app(7, "game-b")), 7, &playing), "the id was given to another game");
    assert!(super::library::plays_this_game(Some(&app(9, "game-a")), 9, &playing), "the game has a new id");
    assert!(super::library::plays_this_game(Some(&app(7, "")), 7, &playing), "no install id: the id decides");
    assert!(super::library::plays_this_game(None, 7, &playing));
    assert!(!super::library::plays_this_game(None, 8, &playing));
}

#[test]
fn a_tile_of_the_running_game_is_the_game_another_device_plays() {
    use nightjar_core::http::AppControl;
    let playing: nightjar_core::http::PlayingOn = serde_json::from_str(
        r#"{"device":"Phone","app":{"install_id":"game-a","app_id":7,"title":"Game"}}"#,
    ).unwrap();
    let tile = |control| nightjar_core::http::App { control, uuid: "tile".into(), ..game(9) };
    assert!(super::library::plays_this_game(Some(&tile(AppControl::Resume)), 9, &playing));
    assert!(super::library::plays_this_game(Some(&tile(AppControl::RunningGame)), 9, &playing));
    assert!(!super::library::plays_this_game(Some(&tile(AppControl::Terminate)), 9, &playing));
    let other = nightjar_core::http::App { uuid: "game-b".into(), ..game(9) };
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

use crate::config::HostRecord;
use crate::gamepad::Gamepads;
use std::sync::Arc;

fn record(name: &str, paired: bool, mac: bool) -> HostRecord {
    HostRecord {
        unique_id: name.into(),
        name: name.into(),
        address: "192.0.2.7".into(),
        server_certificate: paired.then(|| "pem".into()),
        mac: mac.then(|| "00:1a:2b:3c:4d:5e".into()),
        manual: true,
        ..Default::default()
    }
}

/// A launcher on the PCs screen with a headless egui context.
struct Frames {
    ctx: egui::Context,
    l: Launcher,
    hosts: Hosts,
    pads: Gamepads,
    time: f64,
    /// The window; tall, a test can see a whole page at once.
    size: egui::Vec2,
    /// The text drawn by the last frame, in paint order.
    drawn: Vec<String>,
    /// The same text with where it landed and the clip it was drawn in.
    placed: Vec<(String, Rect, Rect)>,
    waking: Option<String>,
    /// Every texture change since the first frame, and the last frame's shapes, for screenshots.
    textures: egui::TexturesDelta,
    shapes: Vec<egui::epaint::ClippedShape>,
    /// The display's scale (1 everywhere but in screenshots).
    native_ppp: f32,
    /// The last frame's AccessKit tree, once a test turned AccessKit on.
    access: Option<egui::accesskit::TreeUpdate>,
    /// Keeps every texture for `shoot` (screenshots at any scale, 100 % too).
    shots: bool,
}

impl Drop for Frames {
    fn drop(&mut self) {
        self.textures.clear();
    }
}

impl Frames {
    fn new(record: HostRecord, reach: Reach) -> Self {
        Self::with(record, reach, None, Vec::new())
    }

    /// A PC as the app sees it after the PC answered (`info`) and its classic list loaded.
    fn with(record: HostRecord, reach: Reach, info: Option<nightjar_core::http::ServerInfo>, apps: Vec<nightjar_core::http::App>) -> Self {
        crate::theme::own_palette();
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut l = launcher(false, false);
        l.go(Screen::Hosts);
        let mut hosts = Hosts::without_discovery(vec![record], Arc::new(|| {}));
        hosts.list[0].reach = reach;
        hosts.list[0].info = info;
        if !apps.is_empty() {
            hosts.list[0].apps = Apps::Loaded(apps);
        }
        let mut frames = Self::over(ctx, l, hosts);
        frames.settle();
        frames
    }

    fn frame(&mut self, events: Vec<egui::Event>) -> Vec<Command> {
        self.time += 0.05;
        let mut input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
            time: Some(self.time),
            events,
            focused: true,
            ..Default::default()
        };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(self.native_ppp);
        let mut commands = Vec::new();
        let output = self.ctx.run_ui(input, |ui| {
            commands = self.l.ui(ui, View { hosts: &self.hosts, pads: &self.pads, connecting: None, waking: self.waking.clone() });
        });
        self.access = output.platform_output.accesskit_update.clone();
        // Only screenshots read the textures; everyone else lets them go.
        if self.native_ppp == 1.0 && !self.shots {
            let mut delta = output.textures_delta;
            delta.clear();
        } else {
            self.textures.append(output.textures_delta);
        }
        self.drawn.clear();
        self.placed.clear();
        for clipped in &output.shapes {
            collect_text(&clipped.shape, &mut self.drawn);
            collect_placed(&clipped.shape, clipped.clip_rect, &mut self.placed);
        }
        self.shapes = output.shapes;
        commands
    }

    fn over(ctx: egui::Context, l: Launcher, hosts: Hosts) -> Self {
        Self {
            ctx,
            l,
            hosts,
            pads: Gamepads::default(),
            time: 0.0,
            size: vec2(1280.0, 720.0),
            drawn: Vec::new(),
            placed: Vec::new(),
            waking: None,
            textures: Default::default(),
            shapes: Vec::new(),
            native_ppp: 1.0,
            access: None,
            shots: false,
        }
    }

    /// Paints the last frame into `<dir>/<name>.png`. Needs `native_ppp` set before the first frame.
    fn shoot(&self, dir: &std::path::Path, name: &str) {
        let ppp = self.ctx.pixels_per_point();
        let primitives = self.ctx.tessellate(self.shapes.clone(), ppp);
        let mut canvas = crate::shot::Canvas::new((self.size.x * ppp) as usize, (self.size.y * ppp) as usize);
        canvas.update_textures(&self.textures);
        canvas.paint(&primitives, ppp);
        canvas.save(&dir.join(format!("{name}.png")));
    }

    fn settle(&mut self) -> Vec<Command> {
        let mut commands = Vec::new();
        for _ in 0..4 {
            commands.extend(self.frame(Vec::new()));
        }
        commands
    }

    fn press(&mut self, key: Key) -> Vec<Command> {
        let mut commands = self.frame(crate::gui::tap(key).to_vec());
        commands.extend(self.settle());
        commands
    }

    fn press_with(&mut self, key: Key, modifiers: egui::Modifiers) -> Vec<Command> {
        let mut events = vec![egui::Event::ModifiersChanged(modifiers)];
        events.extend(crate::gui::tap(key));
        let mut commands = self.frame(events);
        commands.extend(self.frame(vec![egui::Event::ModifiersChanged(Default::default())]));
        commands.extend(self.settle());
        commands
    }

    fn click_at(&mut self, pos: Pos2, button: egui::PointerButton) -> Vec<Command> {
        let event = |pressed| egui::Event::PointerButton { pos, button, pressed, modifiers: egui::Modifiers::NONE };
        let mut commands = self.frame(vec![egui::Event::PointerMoved(pos)]);
        commands.extend(self.frame(vec![event(true)]));
        commands.extend(self.frame(vec![event(false)]));
        commands.extend(self.settle());
        commands
    }

    /// Turns the mouse wheel down over the middle of the window, far enough to reach the end
    /// of what scrolls there.
    fn scroll_to_end(&mut self) {
        // Over the window's middle, or over a menu open by its card (where the focus is).
        let menu = self.l.menu_anchor != Anchor::Center && !self.no_dialog();
        let focused = self.ctx.memory(|m| m.focused()).and_then(|id| self.ctx.read_response(id)).map(|r| r.rect.center());
        let centre = focused.filter(|_| menu).unwrap_or((self.size / 2.0).to_pos2());
        self.frame(vec![egui::Event::PointerMoved(centre)]);
        self.frame(vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: vec2(0.0, -1000.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers: egui::Modifiers::NONE,
        }]);
        for _ in 0..10 {
            self.settle();
        }
    }

    /// The card (first stop) and its three dots, as laid out on screen.
    fn card_and_dots(&self) -> (Rect, Rect) {
        let focused = self.ctx.memory(|m| m.focused()).expect("the first PC has focus");
        let card = self.ctx.read_response(focused).expect("the card was laid out").rect;
        let dots = self.ctx.read_response(focused.with("options")).expect("the dots were laid out").rect;
        (card, dots)
    }

    fn options_open(&self) -> bool {
        matches!(self.l.dialog, Dialog::HostOptions { .. })
    }

    fn no_dialog(&self) -> bool {
        matches!(self.l.dialog, Dialog::None)
    }
}

fn collect_text(shape: &egui::Shape, out: &mut Vec<String>) {
    match shape {
        egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
        egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect_text(s, out)),
        _ => {}
    }
}

fn collect_placed(shape: &egui::Shape, clip: Rect, out: &mut Vec<(String, Rect, Rect)>) {
    match shape {
        egui::Shape::Text(text) => out.push((text.galley.text().to_owned(), text.galley.rect.translate(text.pos.to_vec2()), clip)),
        egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect_placed(s, clip, out)),
        _ => {}
    }
}

/// What the PC said last, as the host sends it.
fn answer(paired: bool, running: i32) -> nightjar_core::http::ServerInfo {
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\"><hostname>PC</hostname><uniqueid>uid</uniqueid>             <HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>{}</PairStatus>             <currentgame>{running}</currentgame><state>SUNSHINE_SERVER_{}</state><Permission>1</Permission></root>",
        u8::from(paired),
        if running == 0 { "FREE" } else { "BUSY" },
    );
    nightjar_core::http::ServerInfo::parse(&xml, 47989).expect("the answer parses")
}

fn game(id: i32) -> nightjar_core::http::App {
    nightjar_core::http::App {
        name: "Game".into(),
        id,
        hdr_supported: false,
        uuid: String::new(),
        host_index: None,
        art_version: String::new(),
        control: nightjar_core::http::AppControl::None,
        monitor_resume: false,
    }
}

/// One PC in one state, enough to reach every entry of the options list.
struct State {
    name: &'static str,
    record: HostRecord,
    reach: Reach,
    info: Option<nightjar_core::http::ServerInfo>,
    apps: Vec<nightjar_core::http::App>,
    /// The native read, for the options only it offers (Restart PC, Shut down PC, Update).
    native: Option<(nightjar_core::http::NativeCapabilities, Option<nightjar_core::http::HostUpdate>)>,
}

impl State {
    fn frames(&self) -> Frames {
        let mut frames = Frames::with(self.record.clone(), self.reach, self.info.clone(), self.apps.clone());
        if let Some((capabilities, update)) = &self.native {
            frames.hosts.list[0].native.capabilities = Some(capabilities.clone());
            frames.hosts.list[0].native.update = update.clone();
            frames.settle();
        }
        frames
    }
}

fn every_state() -> Vec<State> {
    let state = |name, record, reach, info, apps| State { name, record, reach, info, apps, native: None };
    vec![
        // Has the one-time PIN and the pairing help.
        state("unpaired", record("Unpaired", false, false), Reach::Online, Some(answer(false, 0)), Vec::new()),
        state("offline, can wake", record("Offline", true, true), Reach::Offline, None, Vec::new()),
        state("not reached yet, can wake", record("Asleep", true, true), Reach::Unknown, None, Vec::new()),
        state("paired", record("Paired", true, false), Reach::Online, Some(answer(true, 0)), Vec::new()),
        // Has Resume and Quit.
        state("running a game", record("Busy", true, false), Reach::Online, Some(answer(true, 7)), vec![game(7)]),
        state("can sleep", record("Sleepy", true, false), Reach::Online, Some(sleep_answer(true)), Vec::new()),
        // Lists Sleep PC, not selectable, with the reason.
        state("sleep not allowed", record("Guarded", true, false), Reach::Online, Some(sleep_answer(false)), Vec::new()),
        // Has Files.
        state("native api", record("Native", true, false), Reach::Online, Some(files::tests::native_answer()), Vec::new()),
        // Has Microphone only.
        state("microphone only", record("Mic", true, false), Reach::Online, Some(mic_only_answer(true)), Vec::new()),
        // Has Restart PC, Shut down PC and Update the PC's host.
        State { native: Some(power_native(true, true)), ..state("power", record("Power", true, false), Reach::Online, Some(answer(true, 0)), Vec::new()) },
        // Lists Restart PC and Shut down PC, not selectable, with the reason; no update without Host control.
        State {
            native: Some(power_native(false, false)),
            ..state("power not allowed", record("Locked", true, false), Reach::Online, Some(answer(true, 0)), Vec::new())
        },
    ]
}

/// A native PC listing Restart, Shut down and Update, with 3.4.0 waiting; the grant and Host
/// control for this device or not.
fn power_native(allowed: bool, host_control: bool) -> (nightjar_core::http::NativeCapabilities, Option<nightjar_core::http::HostUpdate>) {
    let capabilities = serde_json::from_str(&format!(
        "{{\"api_version\":1,\"host\":{{\"name\":\"PC\",\"version\":\"3.3.0\",\"unique_id\":\"uid\"}},\
         \"features\":[\"power.restart\",\"power.shutdown\",\"update\",\"pairing.approve\"],\
         \"permissions\":{{\"list\":true,\"view\":true,\"launch\":true,\"host_control\":{host_control},\"restart_shutdown\":{allowed}}}}}"
    ))
    .expect("the answer parses");
    let update = nightjar_core::http::HostUpdate {
        current_version: "3.3.0".into(),
        latest_version: Some("v3.4.0".into()),
        phase: "idle".into(),
        install_supported: true,
        error: None,
        waiting_for: None,
        install_now: false,
    };
    (capabilities, Some(update))
}

/// A native PC's answer with its native read merged in: `session.mic_only` listed, with or
/// without this device's launch permission.
fn mic_only_answer(launch: bool) -> nightjar_core::http::ServerInfo {
    let mut info = files::tests::native_answer();
    let mut native = files::tests::native("\"session.mic_only\"", false, false);
    native.capabilities.permissions.launch = launch;
    crate::hosts::merge_native(&mut info, Some(&native));
    info
}

/// A paired PC's answer with the power-action fields, sleep allowed for this device or not.
fn sleep_answer(allowed: bool) -> nightjar_core::http::ServerInfo {
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\"><hostname>PC</hostname><uniqueid>uid</uniqueid>             <HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>1</PairStatus><currentgame>0</currentgame>             <state>SUNSHINE_SERVER_FREE</state><PowerActionVersion>1</PowerActionVersion><SleepSupported>1</SleepSupported>             <SleepAllowed>{}</SleepAllowed></root>",
        u8::from(allowed),
    );
    nightjar_core::http::ServerInfo::parse(&xml, 47989).expect("the answer parses")
}

fn removes(commands: &[Command]) -> bool {
    commands.iter().any(|c| matches!(c, Command::Remove(_)))
}

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
    use nightjar_core::http::native_api::library::{Artwork, Game, Library};
    let game = Game {
        install_id: "install-0".into(),
        app_id: 1000,
        title: "Game 00".into(),
        game_id: None,
        store: None,
        installed: Some(true),
        running: false,
        artwork: Artwork { cover: None, version: String::new() },
    };
    f.hosts.list[0].native.library = Some(Library { revision: "r1".into(), games: vec![game] });
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
    f.l.go(Screen::Settings);
    f.settle();
    // The list beside the rows names the sections in order, then the open one heads the rows.
    let titles: Vec<&str> = f.drawn.iter().map(String::as_str).filter(|t| SETTINGS_SECTIONS.contains(t)).collect();
    assert_eq!(titles[..8], SETTINGS_SECTIONS, "the list names the sections in order");
    assert_eq!(titles[8..], ["Video"], "and the first is open");
    // A narrow window lists them as chips above the rows, in the same order.
    f.size = vec2(800.0, 12000.0);
    f.settle();
    let titles: Vec<&str> = f.drawn.iter().map(String::as_str).filter(|t| SETTINGS_SECTIONS.contains(t)).collect();
    assert_eq!(titles[..8], SETTINGS_SECTIONS, "the chips name the sections in order");
}

/// The widgets a screen reader can press or change that have no name to say, with their role and place.
fn unnamed(f: &Frames) -> Vec<String> {
    use egui::accesskit::{Action, Role};
    let tree = f.access.as_ref().expect("AccessKit is on");
    let pressable = |n: &egui::accesskit::Node| {
        n.supports_action(Action::Click) || n.supports_action(Action::Focus)
            || matches!(n.role(), Role::Button | Role::CheckBox | Role::Switch | Role::ComboBox | Role::Slider | Role::TextInput | Role::SpinButton)
    };
    tree.nodes.iter()
        // A label is named by its text, its value.
        .filter(|(_, n)| pressable(n) && n.role() != Role::Label)
        .filter(|(_, n)| n.label().is_none_or(|l| l.trim().is_empty()) && n.labelled_by().is_empty())
        .map(|(_, n)| format!("{:?} at {:?}", n.role(), n.bounds()))
        .collect()
}

/// Every button, switch, list and field on the main screens has a name for a screen reader.
#[test]
fn screen_readers_hear_a_name_for_everything_on_the_main_screens() {
    let mut f = Frames::with(record("Desk", true, true), Reach::Online, Some(answer(true, 7)), vec![game(7), game(9)]);
    f.ctx.enable_accesskit();
    f.size = vec2(1280.0, 12000.0);
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
    let store = crate::pin_lock::PinStore::new(&dir, Arc::new(crate::pin_lock::SystemClock));
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
    let tiers = include_str!("../../../../app/src/main/res/values/settings_tiers.xml");
    let parsed: Vec<(&str, Vec<&str>)> = tiers.split("<string-array name=\"settings_tier_").skip(1).map(|array| {
        let (name, body) = array.split_once('"').expect("a named array");
        let body = body.split("</string-array>").next().unwrap_or_default();
        (name, body.split("<item>").skip(1).filter_map(|i| i.split("</item>").next()).collect())
    }).collect();
    let ours: Vec<(&str, Vec<&str>)> = MAIN_KEYS.iter().map(|(name, keys)| (*name, keys.to_vec())).collect();
    assert_eq!(ours, parsed, "MAIN_KEYS follows settings_tiers.xml");
    let preferences = include_str!("../../../../app/src/main/res/xml/preferences.xml");
    for (desktop, android) in ANDROID_KEYS {
        assert!(preferences.contains(&format!("android:key=\"{android}\"")), "{desktop} matches Android's {android}, which exists");
        assert!(!DESKTOP_MAIN.contains(&desktop), "{desktop}: Android decides its tier");
    }
    assert!(main_row("adaptive_bitrate") && main_row("auto_reconnect") && main_row("window_mode"));
    assert!(!main_row("ui_zoom") && !main_row("usb_forwarding"));
}

#[test]
fn settings_show_a_sections_every_row_with_the_rarer_ones_in_groups_after() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 12000.0);
    f.l.go(Screen::Settings);
    f.settle();
    for text in ["Lower bitrate when the network is busy", "Fine-tuning", "Automatic quality", "PyroWave decoder"] {
        assert!(shows(&f, text), "{text}: nothing is folded away: {:?}", f.drawn);
    }
    assert!(!shows(&f, "Recover interrupted streams") && !shows(&f, "More Video settings (6)"), "one section at a time, no fold");
    // In every section, Android's main rows come before the first group, and no group holds one.
    for section in SETTINGS_SECTIONS {
        f.l.settings_view.section = section;
        f.settle();
        let drawn = f.l.settings_view.drawn.clone();
        assert!(!drawn.is_empty(), "{section} draws rows");
        let first_grouped = drawn.iter().position(|(_, grouped)| *grouped).unwrap_or(drawn.len());
        for (at, (key, grouped)) in drawn.iter().enumerate() {
            if main_row(key) {
                assert!(!grouped && at < first_grouped, "{section}: main row {key} comes first: {drawn:?}");
            }
        }
        // A section without any of Android's main rows (Advanced) lists its rows directly.
        if drawn.iter().any(|(key, _)| main_row(key)) {
            for (key, grouped) in &drawn {
                assert!(main_row(key) || *grouped, "{section}: rarer row {key} sits in a group: {drawn:?}");
            }
        }
    }
    // A PC's own value in a group shows in its section like any other row.
    let id = f.hosts.list[0].id;
    f.l.go(Screen::HostSettings(id));
    f.l.settings_view.section = "Streaming";
    f.l.edit_overrides.capture(&Settings { client_roaming: false, ..Settings::default() }, &["client_roaming"]);
    f.settle();
    assert!(shows(&f, "Keep streaming when the network changes") && shows(&f, "USB forwarding"), "{:?}", f.drawn);
}

#[test]
fn resolution_and_frame_rate_offer_presets_and_custom_with_its_row() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 12000.0);
    f.l.go(Screen::Settings);
    f.settle();
    assert!(!shows(&f, "Custom resolution") && !shows(&f, "Custom frame rate"), "Native and Display need no rows");
    // The open menu, under its button, scrolls when it is taller than the space it gets.
    let wheel_over = |f: &mut Frames, button: &str| {
        let at = f.placed.iter().find(|(t, ..)| t == button).expect("the menu's button").1.center() + vec2(0.0, 150.0);
        f.frame(vec![egui::Event::PointerMoved(at)]);
        f.frame(vec![egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: vec2(0.0, -1000.0),
            phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Unknown, modifiers: egui::Modifiers::NONE }]);
        for _ in 0..10 { f.settle(); }
    };
    click_text(&mut f, "Native (1920x1080)");
    let mut offered: Vec<String> = f.drawn.clone();
    wheel_over(&mut f, "Native (1920x1080)");
    offered.extend(f.drawn.iter().cloned());
    for (w, h) in RESOLUTIONS {
        assert!(offered.contains(&format!("{w}x{h}")), "{w}x{h} is offered: {offered:?}");
    }
    click_text(&mut f, "Custom…");
    assert_eq!((f.l.settings.width, f.l.settings.height), (1920, 1080), "Custom starts from the native size");
    assert!(shows(&f, "Custom (1920x1080)") && shows(&f, "Custom resolution"), "{:?}", f.drawn);
    // A preset hides the row; a saved size off the menu is Custom with its row.
    click_text(&mut f, "Custom (1920x1080)");
    wheel_over(&mut f, "Custom (1920x1080)");
    click_text(&mut f, "5120x1440");
    assert_eq!((f.l.settings.width, f.l.settings.height), (5120, 1440));
    assert!(shows(&f, "5120x1440") && !shows(&f, "Custom resolution"), "{:?}", f.drawn);
    (f.l.settings.width, f.l.settings.height) = (1720, 720);
    f.settle();
    assert!(shows(&f, "Custom (1720x720)") && shows(&f, "Custom resolution"), "{:?}", f.drawn);

    click_text(&mut f, "Display (60 Hz)");
    let mut offered: Vec<String> = f.drawn.clone();
    wheel_over(&mut f, "Display (60 Hz)");
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
    f.l.go(Screen::Settings);
    f.settle();
    assert!(shows(&f, "PC render resolution"), "{:?}", f.drawn);
    assert!(shows(&f, "The size the PC renders at, as a percent of the stream resolution. The video it sends keeps its size. To send fewer pixels and upscale here, lower Resolution instead."));
    // The old name and description went to the Windows scale, a row of its own.
    assert!(!f.drawn.iter().any(|t| t == "Text and interface size on the virtual display."), "{:?}", f.drawn);
    assert!(shows(&f, "PC display scaling") && shows(&f, "Text and interface size on the PC’s virtual display."));
    // The slider holds Android's 50-200 %.
    for (set, kept) in [(300, 200), (25, 50), (135, 135)] {
        f.l.settings.virtual_display_scale = set;
        f.settle();
        assert_eq!(f.l.settings.virtual_display_scale, kept, "{set} %");
    }
}

#[test]
fn a_pcs_own_display_scaling_is_offered_only_by_hosts_that_read_it() {
    use nightjar_core::http::{Availability, Reason};
    assert_eq!(display_scale_gate(Availability::Supported), Some(None));
    assert_eq!(display_scale_gate(Availability::Unknown), Some(None), "the main settings and a PC not reached");
    assert_eq!(display_scale_gate(Availability::Unsupported(None)), None);
    assert_eq!(display_scale_gate(Availability::Unsupported(Some(Reason::UpdateHost))), Some(Some("Update the host software to use this.")));
    let info = |extra: &str| {
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\"><hostname>PC</hostname><uniqueid>uid</uniqueid><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>1</PairStatus><currentgame>0</currentgame><state>SUNSHINE_SERVER_FREE</state><Permission>1</Permission>{extra}</root>"
        );
        nightjar_core::http::ServerInfo::parse(&xml, 47989).expect("the answer parses")
    };
    let rubylight = "<RustHostVersion>3.0.0</RustHostVersion><VirtualDisplayCapable>true</VirtualDisplayCapable><VirtualDisplayDriverReady>true</VirtualDisplayDriverReady>";
    let description = "Text and interface size on the PC’s virtual display.";
    for (host, extra, drawn) in [
        ("Rubylight that reads it", format!("{rubylight}<DisplayScaleVersion>1</DisplayScaleVersion>"), Some(description.to_owned())),
        ("older Rubylight", rubylight.to_owned(), Some(format!("{description} Update the host software to use this."))),
        ("Apollo", String::new(), None),
    ] {
        let mut f = Frames::with(record("PC", true, false), Reach::Online, Some(info(&extra)), Vec::new());
        f.size = vec2(1280.0, 12000.0);
        f.l.settings_view.section = "Display & image";
        let id = f.hosts.list[0].id;
        f.l.go(Screen::HostSettings(id));
        f.settle();
        assert!(shows(&f, "PC render resolution"), "{host}: {:?}", f.drawn);
        assert_eq!(shows(&f, "PC display scaling"), drawn.is_some(), "{host}: {:?}", f.drawn);
        if let Some(drawn) = drawn {
            assert!(shows(&f, &drawn), "{host}: {:?}", f.drawn);
        }
    }
}

#[test]
fn the_pyrowave_gpu_probe_starts_only_with_pyrowave_selected_or_the_codec_menu_open() {
    use crate::config::{CodecPreference, PyroWaveDecoder};
    let started = || crate::stream::pyrowave_mode::settings_probe_started(true);
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

/// A 1280x720 window at 150 % zoom is 853x480 points. Everything drawn must stay inside it
/// sideways (a cut-off button label shows as text outside its clip); a dialog, which does not
/// scroll, must also stay inside it from top to bottom.
fn fit_150(f: &mut Frames) {
    f.l.settings.ui_zoom = 150;
    f.l.settings.language = crate::config::Language::German;
    f.size = vec2(1280.0 / 1.5, 720.0 / 1.5);
    f.settle();
    assert!((f.ctx.zoom_factor() - 1.5).abs() < 1e-6, "the zoom setting reached egui");
}

fn assert_fits(f: &Frames, what: &str, whole_height: bool) {
    let screen = Rect::from_min_size(Pos2::ZERO, f.size);
    assert!(!f.placed.is_empty(), "{what}: nothing was drawn");
    for (text, rect, clip) in &f.placed {
        if text.trim().is_empty() {
            continue;
        }
        assert!(
            rect.left() >= clip.left() - 1.0 && rect.right() <= clip.right() + 1.0 && rect.right() <= screen.right() + 1.0,
            "{what}: {text:?} runs past its edge: text {rect:?}, clip {clip:?}, window {screen:?}"
        );
        if whole_height && must_stay_in_window(*rect, *clip, screen) {
            assert!(rect.top() >= -1.0 && rect.bottom() <= screen.bottom() + 1.0, "{what}: {text:?} leaves the window: {rect:?} in {screen:?}");
        }
    }
    if whole_height {
        let past = areas_past_window(&f.ctx, screen);
        assert!(past.is_empty(), "{what}: its area leaves the window: {past:?} in {screen:?}");
    }
}

/// The label of a tall row scrolled out of its list is laid out but never seen, and its list's
/// clip ends above the window's edge. A clip that reaches the edge scrolls nothing away: text
/// past it is cut off, even when none of it shows.
fn must_stay_in_window(rect: Rect, clip: Rect, screen: Rect) -> bool {
    rect.intersects(clip) || clip.bottom() >= screen.bottom() - 1.0
}

/// A dialog's whole area, whatever part of it reached the screen: egui doesn't paint a button
/// outside the clip, so one wholly below the window never shows up among the drawn text.
fn areas_past_window(ctx: &egui::Context, screen: Rect) -> Vec<Rect> {
    ctx.memory(|m| {
        m.areas()
            .visible_layer_ids()
            .into_iter()
            .filter_map(|layer| m.area_rect(layer.id))
            .filter(|area| area.top() < -1.0 || area.bottom() > screen.bottom() + 1.0)
            .collect()
    })
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
            Dialog::AddHost { text: String::new(), error: Some("Cannot reach the host.".into()) },
            Dialog::Otp { host: id, pin: "1234".into(), passphrase: String::new(), error: None },
            Dialog::PairingHelp,
            Dialog::Rename { host: id, text: String::new() },
            Dialog::ConfirmRemove { host: id, name: "A long name for a gaming PC".into() },
            Dialog::ConfirmUnpair { host: id, name: "A long name for a gaming PC".into() },
            Dialog::ConfirmClose { host: id, name: "A game with a rather long title".into() },
            Dialog::ConfirmQuit { host: id, start: Some((8, "Another long game title".into())), running: "A game with a rather long title".into(), resume: Some(7), named: None },
            Dialog::ConfirmReplace { app: "Another long game title".into(), running: "A game with a rather long title".into() },
            Dialog::ContinueHere { host: id, app: 7, resume: true, name: "A game with a rather long title".into(), device: "A long name for a phone".into() },
            Dialog::NowPlaying { host: id, app: 7, device: "A long name for a television".into() },
            Dialog::GameCameBack { host: id, app: 7 },
            Dialog::MoveFailed { host: id, app: 7 },
            Dialog::PairError { host: id, body: "PIN rejected. Generate a new one-time PIN in your host's web UI and check the passphrase.".into(), one_time: true },
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

/// The rectangles drawn filled with `fill`, nested shapes included.
fn rects_filled(f: &Frames, fill: Color32) -> Vec<Rect> {
    fn walk(shape: &egui::Shape, fill: Color32, out: &mut Vec<Rect>) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, fill, out)),
            egui::Shape::Rect(r) if r.fill == fill => out.push(r.rect),
            _ => {}
        }
    }
    let mut found = Vec::new();
    for clipped in &f.shapes {
        walk(&clipped.shape, fill, &mut found);
    }
    found
}

/// The faded edges of a scroll area as drawn: a mesh shading from clear into the dialog's fill.
fn fades(f: &Frames) -> Vec<Rect> {
    let mut found = Vec::new();
    for clipped in &f.shapes {
        if let egui::Shape::Mesh(mesh) = &clipped.shape {
            let colours: Vec<Color32> = mesh.vertices.iter().map(|v| v.color).collect();
            if colours.contains(&*SURFACE_HIGH) && colours.contains(&Color32::TRANSPARENT) {
                found.push(Rect::from_points(&mesh.vertices.iter().map(|v| v.pos).collect::<Vec<_>>()));
            }
        }
    }
    found
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

#[test]
fn at_150_percent_in_german_every_settings_page_fits_the_width() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    fit_150(&mut f);
    let id = f.hosts.list[0].id;
    for screen in [Screen::Home, Screen::Settings, Screen::HostSettings(id), Screen::Profiles, Screen::Controllers, Screen::Shortcuts] {
        f.l.go(screen);
        f.settle();
        assert_fits(&f, &format!("{screen:?}"), false);
        if matches!(screen, Screen::Settings | Screen::HostSettings(_)) {
            for section in SETTINGS_SECTIONS {
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
fn at_150_percent_in_german_the_roaming_rows_fit_and_mobile_data_shows_only_with_roaming() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    fit_150(&mut f);
    // The 1280-pixel width is what is checked; the height lets the network rows be drawn
    // without scrolling to them.
    f.size.y = 8000.0;
    f.l.settings_view.section = "Streaming";
    let shown = |f: &Frames, text: &str| f.placed.iter().any(|(t, _, _)| t.contains(text));
    let id = f.hosts.list[0].id;
    for (screen, global) in [(Screen::Settings, true), (Screen::HostSettings(id), false)] {
        f.l.settings.client_roaming = true;
        f.l.go(screen);
        f.settle();
        assert!(shown(&f, "Netzwerkwechsel"), "{screen:?}: the roaming row is drawn in German");
        assert!(shown(&f, "Über mobile Daten fortsetzen"), "{screen:?}: mobile data shows while roaming is on");
        assert_fits(&f, &format!("{screen:?} with roaming"), false);
        if global {
            f.l.settings.client_roaming = false;
            f.settle();
            assert!(shown(&f, "Netzwerkwechsel"));
            assert!(!shown(&f, "Über mobile Daten fortsetzen"), "mobile data hides while roaming is off");
        }
    }
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
    // The system setting decides only on Follow system; the tests' machine may say either.
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
    assert_eq!(f.l.settings.ui_zoom, 100);
    f.press_with(Key::Equals, egui::Modifiers::COMMAND);
    assert_eq!(f.l.settings.ui_zoom, 115);
    f.press_with(Key::Equals, egui::Modifiers::COMMAND);
    f.press_with(Key::Equals, egui::Modifiers::COMMAND);
    f.press_with(Key::Equals, egui::Modifiers::COMMAND);
    f.press_with(Key::Equals, egui::Modifiers::COMMAND);
    assert_eq!(f.l.settings.ui_zoom, 150, "the top step is the last one");
    f.press_with(Key::Minus, egui::Modifiers::COMMAND);
    assert_eq!(f.l.settings.ui_zoom, 130);
    f.press_with(Key::Num0, egui::Modifiers::COMMAND);
    assert_eq!(f.l.settings.ui_zoom, 100);
    // The Rename dialog's name field has the keyboard.
    f.press(Key::F2);
    f.press(Key::ArrowDown);
    f.press(Key::ArrowDown);
    f.press(Key::Enter);
    assert!(matches!(f.l.dialog, Dialog::Rename { .. }));
    assert!(f.ctx.text_edit_focused(), "the name field is focused");
    f.press_with(Key::Equals, egui::Modifiers::COMMAND);
    assert_eq!(f.l.settings.ui_zoom, 100, "a text field keeps Ctrl+=");
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
    // Without its library the running game cannot be named, so there is nothing to resume.
    let mut unlisted = by("running a game");
    unlisted.apps = Apps::NotLoaded;
    let options = host_options(&unlisted);
    assert!(!options.contains(&HostOption::Resume) && options.contains(&HostOption::QuitApp));
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

#[test]
fn input_only_through_a_small_stream_needs_the_setting_and_leaves_a_running_game_alone() {
    use nightjar_core::http::{App, AppControl};
    use nightjar_policy::launch::InputOnlyBlocker;
    let states = every_state();
    let by = |name: &str| states.iter().find(|s| s.name == name).unwrap().frames().hosts.list.remove(0);
    let busy = by("running a game");
    assert_eq!(small_stream_offer(&busy, &game(7), true, false), None, "the setting is off");
    assert_eq!(small_stream_offer(&busy, &game(7), true, true), Some(Some(InputOnlyBlocker::ViewPermission)),
        "permission 1 allows neither viewing nor launching");
    assert_eq!(small_stream_offer(&busy, &game(8), false, true), None, "another game runs: never replace it");
    let mut allowed = by("running a game");
    allowed.info.as_mut().unwrap().permission = None;
    assert_eq!(small_stream_offer(&allowed, &game(7), true, true), Some(None), "joins the running game");
    assert_eq!(small_stream_offer(&allowed, &App { control: AppControl::Resume, ..game(7) }, true, true), None, "only ordinary games");
    allowed.apps = Apps::Loaded(vec![game(7), App { control: AppControl::Input, ..game(9) }]);
    assert_eq!(small_stream_offer(&allowed, &game(7), true, true), None, "the host's own Input-only tile is used instead");
    let mut idle = by("paired");
    idle.info.as_mut().unwrap().permission = None;
    assert_eq!(small_stream_offer(&idle, &game(8), false, true), Some(None), "starts the game on an idle PC");
}

#[test]
fn f2_opens_the_options_for_the_focused_pc() {
    for State { name: state, .. } in every_state() {
        let mut f = every_state().into_iter().find(|s| s.name == state).unwrap().frames();
        assert!(!f.options_open(), "{state}");
        f.press(Key::F2);
        assert!(f.options_open(), "{state}: F2, which the controller's West button sends, opens the options");
        f.press(Key::Escape);
        assert!(f.no_dialog(), "{state}: Escape closes them");
        f.press_with(Key::F10, egui::Modifiers::SHIFT);
        assert!(f.options_open(), "{state}: Shift+F10");
    }
}

#[test]
fn right_click_and_the_three_dots_open_the_options() {
    let mut f = Frames::new(record("Paired", true, false), Reach::Online);
    let (card, dots) = f.card_and_dots();
    let body = card.left_bottom() + vec2(20.0, -10.0);
    f.click_at(body, egui::PointerButton::Secondary);
    assert!(f.options_open(), "right-click");
    f.press(Key::Escape);
    f.click_at(dots.center(), egui::PointerButton::Primary);
    assert!(f.options_open(), "the visible dots");
    f.press(Key::Escape);
    let commands = f.click_at(body, egui::PointerButton::Primary);
    assert!(f.no_dialog(), "a plain click still takes the card's own action");
    assert!(commands.iter().any(|c| matches!(c, Command::OpenApps(_))));
}

#[test]
fn every_option_is_reachable_with_the_keyboard() {
    let mut seen: Vec<HostOption> = Vec::new();
    for scenario in every_state() {
        let state = scenario.name;
        let frames = scenario.frames();
        let host = &frames.hosts.list[0];
        let options = host_options(host);
        seen.extend(&options);
        // Sleep PC listed for a device the PC does not allow cannot take focus.
        let reachable = options.iter().filter(|o| match o {
            HostOption::Sleep => host.sleep_option() == Some(Ok(())),
            HostOption::RestartPc => host.pc_power_option(PcPower::Restart) == Some(Ok(())),
            HostOption::ShutDownPc => host.pc_power_option(PcPower::ShutDown) == Some(Ok(())),
            HostOption::PairAnother => host.pair_another_option() == Some(Ok(())),
            _ => true,
        });
        for (n, option) in reachable.enumerate() {
            let mut f = scenario.frames();
            f.press(Key::F2);
            assert!(f.options_open(), "{state}");
            for _ in 0..n {
                f.press(Key::ArrowDown);
            }
            let commands = f.press(Key::Enter);
            let said = |what: fn(&Command) -> bool| commands.iter().any(what);
            let ok = match option {
                HostOption::OpenLibrary => said(|c| matches!(c, Command::OpenApps(_))),
                HostOption::Resume => said(|c| matches!(c, Command::Launch { app: 7, resume: true, .. })),
                HostOption::PairingHelp => matches!(f.l.dialog, Dialog::PairingHelp),
                HostOption::Pair => said(|c| matches!(c, Command::Pair(_))),
                HostOption::Wake => said(|c| matches!(c, Command::Wake(_))),
                HostOption::Sleep => {
                    said(|c| matches!(c, Command::SleepOffer { .. })) && matches!(f.l.dialog, Dialog::ConfirmSleep { offer: None, .. })
                }
                HostOption::RestartPc => {
                    said(|c| matches!(c, Command::PcPowerOffer { action: PcPower::Restart, .. }))
                        && matches!(f.l.dialog, Dialog::ConfirmPcPower { action: PcPower::Restart, offer: None, .. })
                }
                HostOption::ShutDownPc => {
                    said(|c| matches!(c, Command::PcPowerOffer { action: PcPower::ShutDown, .. }))
                        && matches!(f.l.dialog, Dialog::ConfirmPcPower { action: PcPower::ShutDown, offer: None, .. })
                }
                HostOption::UpdateHost => matches!(&f.l.dialog, Dialog::ConfirmUpdate { version, .. } if version == "3.4.0"),
                HostOption::PairAnother => matches!(&f.l.dialog, Dialog::PairRequests { only: Some(_) }),
                HostOption::Files => said(|c| matches!(c, Command::OpenFiles(_))) && f.no_dialog(),
                HostOption::Settings => said(|c| matches!(c, Command::HostSettings(_))),
                HostOption::Details => matches!(f.l.screen, Screen::HostDetails(_)) && f.no_dialog(),
                HostOption::CheckAgain => said(|c| matches!(c, Command::Refresh(_))),
                HostOption::Rename => matches!(f.l.dialog, Dialog::Rename { .. }),
                HostOption::Unpair => matches!(f.l.dialog, Dialog::ConfirmUnpair { .. }),
                HostOption::Remove => matches!(f.l.dialog, Dialog::ConfirmRemove { .. }),
                HostOption::PairOtp => matches!(f.l.dialog, Dialog::Otp { .. }),
                HostOption::QuitApp => matches!(f.l.dialog, Dialog::ConfirmQuit { .. }),
                HostOption::MicOnly => said(|c| matches!(c, Command::MicOnly { .. })) && f.no_dialog(),
            };
            assert!(ok, "{state}: {} by keys (entry {n})", option.label());
        }
    }
    use HostOption::*;
    for option in [
        OpenLibrary, Resume, QuitApp, Pair, PairOtp, Wake, Sleep, RestartPc, ShutDownPc, UpdateHost, PairAnother, Files, Settings, Details, Rename, Unpair,
        PairingHelp, CheckAgain, Remove, MicOnly,
    ] {
        assert!(seen.contains(&option), "no state reaches {}", option.label());
    }
}

#[test]
fn microphone_only_is_offered_only_where_the_pc_lets_this_device_start_it() {
    let offered = |reach, info| {
        let frames = Frames::with(record("Mic", true, false), reach, Some(info), Vec::new());
        host_options(&frames.hosts.list[0]).contains(&HostOption::MicOnly)
    };
    assert!(offered(Reach::Online, mic_only_answer(true)));
    // Without Launch apps, offline, or before the native read (and on stock hosts) it stays hidden.
    assert!(!offered(Reach::Online, mic_only_answer(false)));
    assert!(!offered(Reach::Offline, mic_only_answer(true)));
    assert!(!offered(Reach::Online, files::tests::native_answer()));
    assert!(!offered(Reach::Online, sleep_answer(true)));
}

// ---- A PC's details page. ----

use crate::host_page::tests::{info as host_answer, FakeProbe};
use crate::host_page::CheckState;
use std::sync::atomic::Ordering;

const RUBYLIGHT: &str = "<RustHostVersion>3.0.0</RustHostVersion>";

/// A paired PC's details page; `state` is its serverinfo state, `extra` more of its fields.
fn details(reach: Reach, state: &str, extra: &str) -> Frames {
    let mut f = Frames::with(record("uid", true, false), reach, Some(host_answer(state, extra)), Vec::new());
    let id = f.hosts.list[0].id;
    f.l.open(Screen::HostDetails(id));
    f.settle();
    f
}

fn shows(f: &Frames, text: &str) -> bool {
    f.drawn.iter().any(|t| t == text)
}

/// Frames until the check is no longer running, as the app's waker would bring them.
fn finish_check(f: &mut Frames) {
    for _ in 0..500 {
        f.frame(Vec::new());
        if !f.l.remote.running() {
            f.settle();
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("the check did not finish");
}

fn relay_check() -> Result<nightjar_core::http::RemoteCheck, nightjar_core::http::Error> {
    use nightjar_core::http::{ConnectionTestStats, NetworkPath, RemoteCheck, RemoteOutcome};
    let mut stats = ConnectionTestStats::default();
    [10.0, 14.0, f64::NAN, 12.0, 12.0].into_iter().for_each(|ms| stats.add(ms));
    let server_info = Box::new(host_answer("SUNSHINE_SERVER_FREE", RUBYLIGHT));
    Ok(RemoteCheck { outcome: RemoteOutcome::Verified { server_info, path: Ok(NetworkPath::Relay) }, stats })
}

#[test]
fn the_details_page_shows_what_the_pc_offers_not_its_brand() {
    let f = details(Reach::Online, "SUNSHINE_SERVER_FREE", RUBYLIGHT);
    // The address sits inside the web UI card, under its description.
    let in_card = f.drawn.iter().any(|t| t.starts_with("Manage the host's applications") && t.ends_with("
https://192.0.2.7:47990"));
    assert!(shows(&f, "Rubylight 3.0.0") && shows(&f, "Open in browser") && in_card, "{:?}", f.drawn);
    assert!(shows(&f, "Test this PC"));
    let f = details(Reach::Online, "SUNSHINE_SERVER_FREE", "<Permission>1</Permission>");
    assert!(shows(&f, "Apollo-compatible host") && shows(&f, "Open in browser"), "a known host has a web UI: {:?}", f.drawn);
    // A stock host: the generic details and the check, no web UI.
    let f = details(Reach::Online, "MJOLNIR_STATE_FREE", "");
    assert!(shows(&f, "NVIDIA GameStream") && shows(&f, "Test this PC"), "{:?}", f.drawn);
    assert!(!shows(&f, "Open in browser"));
    let f = details(Reach::Online, "SUNSHINE_SERVER_FREE", "");
    assert!(shows(&f, "Moonlight-compatible host") && !shows(&f, "Open in browser"), "an unknown host's web UI is not guessed");
    let f = details(Reach::Offline, "SUNSHINE_SERVER_FREE", RUBYLIGHT);
    assert!(!shows(&f, "Open in browser"), "an offline PC's web UI cannot be reached");
    // A saved address no browser opens: no button, and the page says why.
    let mut record = record("uid", true, false);
    record.address = "fe80::1%12".into();
    let mut f = Frames::with(record, Reach::Online, Some(host_answer("SUNSHINE_SERVER_FREE", RUBYLIGHT)), Vec::new());
    f.l.open(Screen::HostDetails(f.hosts.list[0].id));
    f.settle();
    assert!(!shows(&f, "Open in browser") && f.drawn.iter().any(|t| t.contains("zone ID (fe80::1%12)")), "{:?}", f.drawn);
    let commands = f.press(Key::Enter);
    assert!(commands.iter().any(|c| matches!(c, Command::RemoteCheck(_))), "the check is the first item then");
}

/// Clicks the first `text` drawn on the last frame.
fn click_text(f: &mut Frames, text: &str) -> Vec<Command> {
    let at = f.placed.iter().find(|(t, ..)| t == text).unwrap_or_else(|| panic!("{text:?} is not drawn: {:?}", f.drawn)).1.center();
    f.click_at(at, egui::PointerButton::Primary)
}

#[test]
fn settings_a_pc_kept_before_the_update_are_listed_once_and_reset_only_on_request() {
    // As 4.1.0 could leave it: Frame rate pinned for the whole PC, and one game's own HDR.
    let mut pinned = record("Gaming PC", true, false);
    pinned.settings = Some(serde_json::from_str(r#"{"fps":120}"#).unwrap());
    pinned.save_app_settings("a", 1, serde_json::from_str(r#"{"hdr":true}"#).unwrap());
    let mut f = Frames::new(pinned, Reach::Online);
    let line = "Gaming PC: PC settings, 1 game";

    assert!(f.l.show_kept_settings_once(&f.hosts.list), "the look is saved as done");
    assert!(matches!(&f.l.dialog, Dialog::KeptSettings { pcs } if pcs == &[KeptPc { name: "Gaming PC".into(), own: true, games: 1 }]));
    f.settle();
    assert!(f.drawn.iter().any(|t| t == line), "{:?}", f.drawn);
    // Keep has focus, so A alone drops nothing.
    let commands = f.press(Key::Enter);
    assert!(!commands.iter().any(|c| matches!(c, Command::ResetKeptSettings)));
    assert!(f.no_dialog());
    assert!(!f.l.show_kept_settings_once(&f.hosts.list), "once");
    assert!(f.no_dialog());

    f.l.settings.kept_settings_checked = false;
    f.l.show_kept_settings_once(&f.hosts.list);
    f.settle();
    assert!(matches!(click_text(&mut f, "Reset all")[..], [Command::ResetKeptSettings]));
    assert!(f.no_dialog());

    // German chosen: the look runs at start, before a frame applies the language, and the line
    // still reads in German once drawn.
    f.l.settings.kept_settings_checked = false;
    f.l.settings.language = crate::config::Language::German;
    f.l.show_kept_settings_once(&f.hosts.list);
    f.settle();
    assert!(f.drawn.iter().any(|t| t == "Gaming PC: PC-Einstellungen, 1 Spiel"), "{:?}", f.drawn);
    f.l.close_dialog();
    f.l.settings.language = crate::config::Language::default();
    f.settle();

    // Another dialog open at start: the look waits for the next start.
    f.l.settings.kept_settings_checked = false;
    f.l.dialog = Dialog::PairingHelp;
    assert!(!f.l.show_kept_settings_once(&f.hosts.list));
    assert!(!f.l.settings.kept_settings_checked);
    // Nothing kept: no dialog, and the look is done all the same.
    f.l.dialog = Dialog::None;
    f.hosts.list[0].record.clear_own_settings();
    assert!(f.l.show_kept_settings_once(&f.hosts.list));
    assert!(f.no_dialog());
}

#[test]
fn the_details_page_shows_shared_folders_and_co_op_once_the_native_read_offers_them() {
    use crate::hosts::tests::{info_lands, native_caps, native_info, native_lands, stock_info, NATIVE_EXTRAS};
    let rows = |f: &Frames| [shows(f, "Shared folders") && shows(f, "Browse"), shows(f, "Co-op") && shows(f, "Show sessions")];
    let mut f = Frames::with(record("uid", true, false), Reach::Online, Some(native_info()), Vec::new());
    f.size = vec2(1280.0, 12000.0);
    let id = f.hosts.list[0].id;
    f.l.open(Screen::HostDetails(id));
    f.settle();
    assert_eq!(rows(&f), [false; 2], "nothing is offered before the native read: {:?}", f.drawn);
    native_lands(&mut f.hosts, id, native_caps(NATIVE_EXTRAS, true));
    f.settle();
    assert_eq!(rows(&f), [true; 2], "{:?}", f.drawn);
    info_lands(&mut f.hosts, id, native_info());
    f.settle();
    assert_eq!(rows(&f), [true; 2], "the next poll keeps them");
    // A host without the native API: no rows, even with a native read from before.
    let mut f = Frames::with(record("uid", true, false), Reach::Online, Some(stock_info()), Vec::new());
    f.size = vec2(1280.0, 12000.0);
    let id = f.hosts.list[0].id;
    f.l.open(Screen::HostDetails(id));
    native_lands(&mut f.hosts, id, native_caps(NATIVE_EXTRAS, true));
    f.settle();
    assert!(shows(&f, "Test this PC"), "the page is drawn: {:?}", f.drawn);
    assert_eq!(rows(&f), [false; 2], "{:?}", f.drawn);
}

#[test]
fn the_details_page_lists_co_op_sessions_only_where_the_pc_offers_them_and_joins_a_free_slot() {
    use crate::coop_page::tests::{api, waker, Fake};
    use nightjar_core::http::native_api::error::ErrorCode;
    let native = "<NativeApiVersion>1</NativeApiVersion>";
    let mut f = details(Reach::Online, "SUNSHINE_SERVER_FREE", native);
    f.size = vec2(1280.0, 12000.0);
    f.settle();
    assert!(!shows(&f, "Co-op"), "a PC that has not said it offers co-op shows none: {:?}", f.drawn);
    let offer = |f: &mut Frames, coop_offered: bool| {
        let info = f.hosts.list[0].info.as_mut().unwrap();
        let evidence = info.capabilities.evidence().unwrap().clone();
        info.capabilities = nightjar_core::http::HostCapabilities::from_evidence(nightjar_core::http::Evidence { native_capabilities: true, coop_offered, ..evidence });
        f.settle();
    };
    offer(&mut f, false);
    assert!(!shows(&f, "Co-op"), "a PC without Nucleus Co-op shows none");
    offer(&mut f, true);
    assert!(shows(&f, "Co-op") && shows(&f, "Show sessions"), "{:?}", f.drawn);
    let id = f.hosts.list[0].id;
    let commands = click_text(&mut f, "Show sessions");
    assert!(commands.iter().any(|c| matches!(c, Command::Coop(h) if *h == id)), "Show sessions opens the list");
    // The list as main opens it: the sessions, the free slot, then a join that launches.
    let fake = Arc::new(Fake { app_id: 1234567, ..Fake::default() });
    f.l.coop.open(id, fake.clone(), waker());
    // Frames until the answers are in, as the waker would bring them; what they asked for.
    let listed = |f: &mut Frames| {
        let mut commands = Vec::new();
        for _ in 0..500 {
            commands.extend(f.frame(Vec::new()));
            if f.l.coop.state_for(id).is_some_and(|open| !open.loading && open.joining.is_none()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        commands.extend(f.settle());
        commands
    };
    let mut commands = listed(&mut f);
    assert!(shows(&f, "Portal 2") && shows(&f, "Free slots: 1") && shows(&f, "Player 1: Fold7") && shows(&f, "Player 2: Free"), "{:?}", f.drawn);
    assert!(shows(&f, "Portal 2 (Co-op)") && shows(&f, "Up to 2 players"));
    commands.extend(click_text(&mut f, "Join"));
    commands.extend(listed(&mut f));
    let launches: Vec<_> = commands.iter().filter_map(|c| match c {
        Command::Launch { host, app, resume } => Some((*host, *app, *resume)),
        _ => None,
    }).collect();
    assert_eq!(launches, [(id, 1234567, false)], "the join launches the session's entry once");
    assert!(shows(&f, "Player 2: This device") && shows(&f, "Return to my slot") && shows(&f, "Joined Portal 2 as player 2. Starting the stream…"), "{:?}", f.drawn);
    // A refusal is drawn with the PC's own words, and launches nothing.
    let fake = Arc::new(Fake { refuse: Some(api(403, ErrorCode::PermissionRequired, "Turn on “Can join co-op games” for this device in the Rubylight console.")), ..Fake::default() });
    f.l.coop.open(id, fake, waker());
    let mut commands = listed(&mut f);
    commands.extend(click_text(&mut f, "Join"));
    commands.extend(listed(&mut f));
    assert!(!commands.iter().any(|c| matches!(c, Command::Launch { .. })), "a refused join launches nothing");
    assert!(shows(&f, "This device may not join co-op sessions on this PC.\nTurn on “Can join co-op games” for this device in the Rubylight console."), "{:?}", f.drawn);
}

/// Whether `text` is drawn on the widget with focus.
fn focus_shows(f: &Frames, text: &str) -> bool {
    let Some(rect) = f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect) else {
        return false;
    };
    f.placed.iter().any(|(t, r, _)| t == text && rect.contains(r.center()))
}

#[test]
fn focus_on_the_web_ui_moves_to_the_check_when_the_pc_goes_away() {
    let mut f = details(Reach::Online, "SUNSHINE_SERVER_FREE", RUBYLIGHT);
    let id = f.hosts.list[0].id;
    assert!(focus_shows(&f, "Open in browser"), "the web UI is the first item");
    f.hosts.list[0].reach = Reach::Offline;
    f.settle();
    assert!(!shows(&f, "Open in browser"));
    assert!(focus_shows(&f, "Test this PC"), "the check has focus, not nothing");
    assert!(f.press(Key::Enter).iter().any(|c| matches!(c, Command::RemoteCheck(h) if *h == id)));

    // Start, then Back: the page comes back without the item it remembers.
    let mut f = details(Reach::Online, "SUNSHINE_SERVER_FREE", RUBYLIGHT);
    f.l.open_settings();
    f.settle();
    assert_eq!(f.l.screen, Screen::Settings);
    f.hosts.list[0].reach = Reach::Unknown;
    f.frame(crate::gui::tap(Key::Escape).to_vec());
    assert_eq!(f.l.screen, Screen::HostDetails(id));
    assert!(focus_shows(&f, "Test this PC"), "the check takes focus on the frame Back lands, not one later");
    f.settle();
    assert!(focus_shows(&f, "Test this PC"));
}

#[test]
fn view_details_opens_the_page_and_back_returns_to_the_card() {
    let mut f = Frames::with(record("uid", true, false), Reach::Online, Some(host_answer("SUNSHINE_SERVER_FREE", RUBYLIGHT)), Vec::new());
    let card = f.ctx.memory(|m| m.focused());
    f.press(Key::F2);
    let at = host_options(&f.hosts.list[0]).iter().position(|o| *o == HostOption::Details).unwrap();
    for _ in 0..at {
        f.press(Key::ArrowDown);
    }
    f.press(Key::Enter);
    assert_eq!(f.l.screen, Screen::HostDetails(f.hosts.list[0].id));
    // The web UI is the first item; Enter opens it.
    let commands = f.press(Key::Enter);
    assert!(commands.iter().any(|c| matches!(c, Command::OpenWebUi(url) if url == "https://192.0.2.7:47990")));
    f.press(Key::Escape);
    assert_eq!(f.l.screen, Screen::Hosts);
    assert_eq!(f.ctx.memory(|m| m.focused()), card, "the card that opened it has focus again");
}

#[test]
fn the_remote_check_runs_shows_its_measurements_and_errors_verbatim() {
    let mut f = details(Reach::Online, "MJOLNIR_STATE_FREE", "");
    let id = f.hosts.list[0].id;
    // Without a web UI the check is the first item.
    let commands = f.press(Key::Enter);
    assert!(commands.iter().any(|c| matches!(c, Command::RemoteCheck(h) if *h == id)), "Test asks the app to start it");
    let probe = FakeProbe::answering(relay_check());
    f.l.remote.start(id, probe.clone(), "uid".into(), Arc::new(|| {}));
    f.settle();
    assert!(shows(&f, "Checking your paired PC…") && shows(&f, "Cancel"), "{:?}", f.drawn);
    probe.release();
    finish_check(&mut f);
    assert!(f.drawn.iter().any(|t| t.starts_with("The paired PC responded on this network.")), "{:?}", f.drawn);
    for text in ["Relay (DERP)", "12.0 ms", "2.0 ms", "20 %", "Test this PC"] {
        assert!(shows(&f, text), "{text}: {:?}", f.drawn);
    }

    // A failure says what the client said, word for word.
    let refused = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "No connection could be made (os error 10061)");
    let probe = FakeProbe::answering(Ok(nightjar_core::http::RemoteCheck {
        outcome: nightjar_core::http::RemoteOutcome::Unreachable(nightjar_core::http::Error::Io(refused)),
        stats: Default::default(),
    }));
    f.l.remote.start(id, probe.clone(), "uid".into(), Arc::new(|| {}));
    probe.release();
    finish_check(&mut f);
    assert!(shows(&f, "No connection could be made (os error 10061)"), "{:?}", f.drawn);
    assert!(f.drawn.iter().any(|t| t.starts_with("The paired PC could not be verified.")));
    assert!(!shows(&f, "Relay (DERP)"), "the last check's route is gone");
}

#[test]
fn cancel_and_leaving_the_page_stop_the_check() {
    let mut f = details(Reach::Online, "MJOLNIR_STATE_FREE", "");
    let id = f.hosts.list[0].id;
    let probe = FakeProbe::answering(relay_check());
    f.l.remote.start(id, probe.clone(), "uid".into(), Arc::new(|| {}));
    f.settle();
    // The button with focus is Cancel now.
    let commands = f.press(Key::Enter);
    assert!(!commands.iter().any(|c| matches!(c, Command::RemoteCheck(_))));
    assert_eq!(probe.cancels.load(Ordering::SeqCst), 1, "the requests in flight are cancelled");
    assert!(shows(&f, "Test cancelled. No host or network settings were changed."), "{:?}", f.drawn);
    assert!(!f.l.remote.running());

    let probe = FakeProbe::answering(relay_check());
    f.l.remote.start(id, probe.clone(), "uid".into(), Arc::new(|| {}));
    f.settle();
    f.press(Key::Escape);
    assert_eq!(f.l.screen, Screen::Hosts);
    assert_eq!(probe.cancels.load(Ordering::SeqCst), 1, "leaving the page cancels it");
    assert!(!f.l.remote.running() && f.l.remote.state.is_none());
    f.l.open(Screen::HostDetails(id));
    f.settle();
    assert!(!shows(&f, "Test cancelled. No host or network settings were changed."), "the page opens fresh");
}

#[test]
fn the_pin_screen_does_not_stop_checks_from_being_polled() {
    let mut f = details(Reach::Online, "MJOLNIR_STATE_FREE", "");
    let id = f.hosts.list[0].id;
    let probe = FakeProbe::answering(relay_check());
    f.l.remote.start(id, probe.clone(), "uid".into(), Arc::new(|| {}));
    f.settle();
    // The app locks with the check in flight, and the page under the lock screen is left.
    let dir = std::env::temp_dir().join(format!("nightjar-ui-pin-poll-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    let store = crate::pin_lock::PinStore::new(&dir, Arc::new(crate::pin_lock::SystemClock));
    store.set_pin(b"1234").unwrap();
    store.set_mode(crate::pin_lock::Mode::App);
    store.flush();
    f.l.pin = Some(crate::pin_ui::PinLock::with_store(store));
    f.l.screen = Screen::Hosts;
    f.l.trail.clear();
    f.settle();
    assert!(f.l.pin.as_ref().unwrap().app_locked());
    assert_eq!(probe.cancels.load(Ordering::SeqCst), 1, "leaving the page cancels its check under the lock too");
    assert!(!f.l.remote.running());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn settings_opened_over_the_page_keep_its_check() {
    let mut f = details(Reach::Online, "MJOLNIR_STATE_FREE", "");
    let id = f.hosts.list[0].id;
    let probe = FakeProbe::answering(relay_check());
    f.l.remote.start(id, probe.clone(), "uid".into(), Arc::new(|| {}));
    probe.release();
    finish_check(&mut f);
    assert!(shows(&f, "Relay (DERP)"), "{:?}", f.drawn);
    // Start, then Back: the result is still there.
    f.l.open_settings();
    f.settle();
    assert_eq!(f.l.screen, Screen::Settings);
    f.press(Key::Escape);
    assert_eq!(f.l.screen, Screen::HostDetails(id));
    assert!(shows(&f, "Relay (DERP)"), "the result survives Settings: {:?}", f.drawn);

    // A running check goes on meanwhile and reports on the page.
    let probe = FakeProbe::answering(relay_check());
    f.l.remote.start(id, probe.clone(), "uid".into(), Arc::new(|| {}));
    f.settle();
    f.l.open_settings();
    f.settle();
    assert!(f.l.remote.running() && probe.cancels.load(Ordering::SeqCst) == 0, "Settings do not cancel it");
    f.press(Key::Escape);
    assert!(shows(&f, "Checking your paired PC…"), "{:?}", f.drawn);
    probe.release();
    finish_check(&mut f);
    assert!(shows(&f, "Relay (DERP)"), "{:?}", f.drawn);

    // Settings that replace the page (PCs from there) leave it: the check is cancelled.
    let probe = FakeProbe::answering(relay_check());
    f.l.remote.start(id, probe.clone(), "uid".into(), Arc::new(|| {}));
    f.l.open_settings();
    f.settle();
    f.l.switch(Screen::Hosts);
    f.settle();
    assert_eq!(probe.cancels.load(Ordering::SeqCst), 1);
    assert!(!f.l.remote.running() && f.l.remote.state.is_none());
}

#[test]
fn at_150_percent_in_german_the_details_page_fits_the_width() {
    let mut f = details(Reach::Online, "SUNSHINE_SERVER_FREE", RUBYLIGHT);
    fit_150(&mut f);
    let id = f.hosts.list[0].id;
    let mut failed = crate::host_page::report(Err(nightjar_core::http::Error::NotPaired));
    failed.error = Some("A rather long error the host gave, word for word, about why it did not answer".into());
    for state in [
        None,
        Some(CheckState::Running),
        Some(CheckState::Cancelled),
        Some(CheckState::Done(crate::host_page::report(relay_check()))),
        Some(CheckState::Done(failed)),
    ] {
        let name = format!("{state:?}");
        f.l.remote.state = state.map(|s| (id, s));
        f.settle();
        assert!(shows(&f, "Im Browser öffnen"), "German is in use: {:?}", f.drawn);
        assert_fits(&f, &format!("details page, {name}"), false);
    }
}

/// When the PC's sleep offer arrives, nothing moves: Cancel keeps its place and the focus, so
/// a click or Enter meant for Cancel never puts the PC to sleep.
#[test]
fn the_sleep_offer_arriving_moves_nothing_under_cancel() {
    let mut f = every_state().into_iter().find(|s| s.name == "can sleep").unwrap().frames();
    let id = f.hosts.list[0].id;
    f.l.dialog = Dialog::ConfirmSleep { host: id, ask: 1, name: "PC".into(), offer: None };
    f.l.focus_dialog = true;
    f.settle();
    let state = |f: &Frames| {
        let cancel = f.placed.iter().find(|(t, ..)| t == "Cancel").map(|p| p.1).expect("Cancel");
        let sleep = f.placed.iter().find(|(t, ..)| t == "Sleep PC").map(|p| p.1).expect("Sleep PC is there from the start");
        let focus = f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect).expect("a focus");
        (cancel, sleep, focus)
    };
    let (cancel, sleep, focus) = state(&f);
    assert!(focus.contains(cancel.center()), "Cancel has the focus: {focus:?} {cancel:?}");
    f.l.sleep_answered(id, 1, "PC", false, Ok(SleepStep::Offer { token: "tok".into(), at: Handed::now() }));
    f.settle();
    assert_eq!(state(&f), (cancel, sleep, focus), "Cancel, Sleep PC and the focus stay put");
    f.frame(vec![egui::Event::PointerMoved(cancel.center())]);
    let commands = f.click_at(cancel.center(), egui::PointerButton::Primary);
    assert!(!commands.iter().any(|c| matches!(c, Command::Sleep { .. })) && f.no_dialog(), "the click cancels");
}

/// Restart PC and Shut down PC confirm as Sleep PC does: Cancel has the focus and keeps its
/// place when the offer arrives; a shutdown warns when the PC may not wake from off; only the
/// confirm button spends the token.
#[test]
fn restart_and_shut_down_wait_for_the_offer_keep_cancel_focused_and_warn_before_an_unwakeable_shutdown() {
    const NO_WAKE: &str = "Once off, this PC may only start with its power button.";
    for (action, label, wake_from_off) in [(PcPower::Restart, "Restart", false), (PcPower::ShutDown, "Shut down", true), (PcPower::ShutDown, "Shut down", false)] {
        let mut f = every_state().into_iter().find(|s| s.name == "power").unwrap().frames();
        let id = f.hosts.list[0].id;
        f.l.dialog = Dialog::ConfirmPcPower { host: id, ask: 1, name: "PC".into(), action, offer: None };
        f.l.focus_dialog = true;
        f.settle();
        let place = |f: &Frames, text: &str| f.placed.iter().find(|(t, ..)| t == text).map(|p| p.1).unwrap_or_else(|| panic!("{text}"));
        let cancel = place(&f, "Cancel");
        let go = place(&f, label);
        let focus = f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect).expect("a focus");
        assert!(focus.contains(cancel.center()), "{label}: Cancel has the focus");
        // Waiting: the button is off and a click on it does nothing.
        let commands = f.click_at(go.center(), egui::PointerButton::Primary);
        assert!(!commands.iter().any(|c| matches!(c, Command::PcPower { .. })), "{label}: nothing to spend yet");

        f.l.pc_power_answered(id, 1, "PC", action, false, Ok(PcPowerStep::Offer { token: "tok".into(), at: Handed::now(), wake_from_off }));
        f.settle();
        assert_eq!((place(&f, "Cancel"), place(&f, label)), (cancel, go), "{label}: nothing moves");
        let warned = f.drawn.iter().any(|t| t == tr(NO_WAKE));
        assert_eq!(warned, action == PcPower::ShutDown && !wake_from_off, "{label}, wake from off {wake_from_off}");
        let commands = f.click_at(go.center(), egui::PointerButton::Primary);
        assert!(
            commands.iter().any(|c| matches!(c, Command::PcPower { action: a, token, .. } if *a == action && token == "tok")) && f.no_dialog(),
            "{label}: confirmed spends the token"
        );
    }
    // A refusal arriving for the dialog shows the host's words.
    let mut f = every_state().into_iter().find(|s| s.name == "power").unwrap().frames();
    let id = f.hosts.list[0].id;
    f.l.dialog = Dialog::ConfirmPcPower { host: id, ask: 1, name: "PC".into(), action: PcPower::Restart, offer: None };
    f.l.pc_power_answered(id, 1, "PC", PcPower::Restart, false, Err("TV is streaming from this PC.".into()));
    assert!(matches!(&f.l.dialog, Dialog::Message { body, .. } if body == "TV is streaming from this PC."));
    // After the 202: Windows' refusal is a failure, a timeout only that the PC may still go.
    let late = |action, result| {
        let mut f = every_state().into_iter().find(|s| s.name == "power").unwrap().frames();
        let id = f.hosts.list[0].id;
        f.l.pc_power_answered(id, 1, "PC", action, true, result);
        match std::mem::replace(&mut f.l.dialog, Dialog::None) {
            Dialog::Message { title, body, .. } => (title, body),
            _ => panic!("a message"),
        }
    };
    let unanswered = "Windows did not confirm the request in time; the PC may still restart: process timed out";
    assert_eq!(late(PcPower::Restart, Ok(PcPowerStep::Unconfirmed(unanswered.into()))), ("PC may still restart".into(), unanswered.into()));
    assert_eq!(late(PcPower::ShutDown, Ok(PcPowerStep::Unconfirmed(unanswered.into()))).0, "PC may still shut down");
    assert_eq!(late(PcPower::Restart, Err("Windows refused to restart the PC: no (exit code 1191)".into())).0, "Could not restart PC");
}

/// A PC whose capabilities list pairing.approve, for a device that holds Host control or not.
fn approving_frames(host_control: bool) -> Frames {
    let mut f = every_state().into_iter().find(|s| s.name == "power").unwrap().frames();
    let capabilities = f.hosts.list[0].native.capabilities.as_mut().unwrap();
    capabilities.capabilities.permissions.host_control = host_control;
    f
}

fn tv_request(id: &str) -> nightjar_core::http::PairingRequest {
    nightjar_core::http::PairingRequest {
        id: id.into(),
        name: "TV".into(),
        address: "192.168.4.116".into(),
        age_seconds: 3,
    }
}

#[test]
fn pair_another_device_is_listed_where_offered_and_says_what_it_needs() {
    let f = every_state().into_iter().find(|s| s.name == "paired").unwrap().frames();
    assert!(!host_options(&f.hosts.list[0]).contains(&HostOption::PairAnother), "an older host");
    let mut f = approving_frames(false);
    assert!(host_options(&f.hosts.list[0]).contains(&HostOption::PairAnother), "no permission");
    assert_eq!(f.hosts.list[0].pair_another_option(), Some(Err(crate::hosts::PAIRING_NOT_ALLOWED)));
    assert!(tr(crate::hosts::PAIRING_NOT_ALLOWED).contains("Approve new devices"));
    assert!(!f.hosts.list[0].may_approve_pairing());
    // The permission alone is enough, as Host control is.
    f.hosts.list[0].native.capabilities.as_mut().unwrap().files.pairing_approve = true;
    assert_eq!(f.hosts.list[0].pair_another_option(), Some(Ok(())));
    let mut f = approving_frames(true);
    assert_eq!(f.hosts.list[0].pair_another_option(), Some(Ok(())));
    // Requests read before Host control went away are not offered.
    f.hosts.list[0].native.pairing_requests = vec![tv_request("a1")];
    assert_eq!(f.hosts.list[0].waiting_to_pair().len(), 1);
    f.hosts.list[0].native.capabilities.as_mut().unwrap().capabilities.permissions.host_control = false;
    assert!(f.hosts.list[0].waiting_to_pair().is_empty());
}

#[test]
fn an_empty_request_list_says_what_to_do_on_the_new_device() {
    let mut f = approving_frames(true);
    let id = f.hosts.list[0].id;
    f.l.dialog = Dialog::PairRequests { only: Some(id) };
    f.l.focus_dialog = true;
    f.settle();
    assert!(f.placed.iter().any(|(t, ..)| t.contains("Nightjar or Moonlight")), "{:?}", f.placed.iter().map(|p| &p.0).collect::<Vec<_>>());
}

#[test]
fn the_pin_pad_is_an_even_grid_under_its_pin_line() {
    let mut f = approving_frames(true);
    f.hosts.list[0].native.pairing_requests = vec![tv_request("a1")];
    f.l.dialog = dialogs::approve_dialog(&f.hosts.list[0], &f.hosts.list[0].native.pairing_requests[0].clone());
    f.settle();
    let at = |text: &str| f.placed.iter().find(|(t, ..)| t == text).map(|p| p.1.center()).unwrap_or_else(|| panic!("{text}"));
    for (a, b) in [("1", "4"), ("4", "7"), ("2", "5"), ("5", "8"), ("8", "0"), ("3", "6"), ("6", "9")] {
        assert!((at(a).x - at(b).x).abs() < 1.0, "{a} above {b}");
    }
    let (left, right) = (at("2").x - at("1").x, at("3").x - at("2").x);
    assert!((left - right).abs() < 1.0, "even columns: {left} and {right}");
    assert!((at("– – – –").x - at("2").x).abs() < 1.0, "the PIN line sits over the middle column");
}

#[test]
fn the_pin_dialog_works_with_a_controller_and_with_the_keyboard() {
    let mut f = approving_frames(true);
    let id = f.hosts.list[0].id;
    f.hosts.list[0].native.pairing_requests = vec![tv_request("a1")];
    f.l.dialog = dialogs::approve_dialog(&f.hosts.list[0], &f.hosts.list[0].native.pairing_requests[0].clone());
    f.l.focus_dialog = true;
    f.settle();
    // The pad opens focused on 1, and Enter (a controller's A) presses it; 2 is one step right.
    f.press(Key::Enter);
    f.press(Key::ArrowRight);
    f.press(Key::Enter);
    let pin = |f: &Frames| match &f.l.dialog {
        Dialog::ApprovePairing { pin, .. } => pin.clone(),
        other => panic!("{:?}", std::mem::discriminant(other)),
    };
    assert_eq!(pin(&f), "12");
    // Approve stays off until the PIN is whole: Backspace keeps it so.
    f.press(Key::Backspace);
    assert_eq!(pin(&f), "1");
    if let Dialog::ApprovePairing { pin, .. } = &mut f.l.dialog {
        "123".clone_into(pin);
    }
    f.settle();
    // The last digit moves the focus to Approve, so Enter sends it.
    let four = f.placed.iter().find(|(t, ..)| t == "4").map(|p| p.1).expect("4");
    f.click_at(four.center(), egui::PointerButton::Primary);
    assert_eq!(pin(&f), "1234");
    f.settle();
    let commands = f.press(Key::Enter);
    assert!(commands.iter().any(|c| matches!(c, Command::ApprovePairing { host, request, pin, .. } if *host == id && request == "a1" && pin == "1234")), "Enter on Approve");
    assert!(matches!(&f.l.dialog, Dialog::ApprovePairing { busy: true, .. }));
    // While the PC answers, nothing more goes out.
    let commands = f.press(Key::Enter);
    assert!(!commands.iter().any(|c| matches!(c, Command::ApprovePairing { .. } | Command::DenyPairing { .. })));
}

#[test]
fn approve_needs_all_four_digits_and_deny_needs_none() {
    let mut f = approving_frames(true);
    let id = f.hosts.list[0].id;
    f.hosts.list[0].native.pairing_requests = vec![tv_request("a1")];
    f.l.dialog = dialogs::approve_dialog(&f.hosts.list[0], &f.hosts.list[0].native.pairing_requests[0].clone());
    f.settle();
    let approve = f.placed.iter().find(|(t, ..)| t == "Approve").map(|p| p.1).expect("Approve");
    let commands = f.click_at(approve.center(), egui::PointerButton::Primary);
    assert!(!commands.iter().any(|c| matches!(c, Command::ApprovePairing { .. })), "no PIN yet");
    let deny = f.placed.iter().find(|(t, ..)| t == "Deny").map(|p| p.1).expect("Deny");
    let commands = f.click_at(deny.center(), egui::PointerButton::Primary);
    assert!(commands.iter().any(|c| matches!(c, Command::DenyPairing { host, request, .. } if *host == id && request == "a1")));
}

#[test]
fn a_refusal_shows_the_hosts_words_and_a_success_a_toast() {
    let mut f = approving_frames(true);
    let id = f.hosts.list[0].id;
    f.hosts.list[0].native.pairing_requests = vec![tv_request("a1")];
    f.l.dialog = dialogs::approve_dialog(&f.hosts.list[0], &f.hosts.list[0].native.pairing_requests[0].clone());
    f.l.pairing_answered(id, "a1", "TV", false, Err("That is not the PIN TV shows.".into()));
    assert!(matches!(&f.l.dialog, Dialog::Message { body, .. } if body == "That is not the PIN TV shows."));
    f.l.dialog = dialogs::approve_dialog(&f.hosts.list[0], &f.hosts.list[0].native.pairing_requests[0].clone());
    f.l.pairing_answered(id, "a1", "TV", true, Ok(()));
    assert!(f.no_dialog(), "a denial says nothing more");
}

#[test]
fn home_shows_who_waits_and_opens_their_pin() {
    let mut f = approving_frames(true);
    f.l.screen = Screen::Home;
    f.hosts.list[0].native.pairing_requests = vec![tv_request("a1")];
    f.settle();
    let banner = f.placed.iter().find(|(t, ..)| t == "TV wants to pair").map(|p| p.1).expect("the banner");
    f.click_at(banner.center(), egui::PointerButton::Primary);
    assert!(matches!(&f.l.dialog, Dialog::ApprovePairing { request, .. } if request == "a1"));
    f.l.dialog = Dialog::None;
    f.hosts.list[0].native.pairing_requests.push(tv_request("a2"));
    f.settle();
    let banner = f.placed.iter().find(|(t, ..)| t == "2 devices want to pair").map(|p| p.1).expect("the count");
    f.click_at(banner.center(), egui::PointerButton::Primary);
    assert!(matches!(&f.l.dialog, Dialog::PairRequests { only: None }));
}

#[test]
fn update_the_pcs_host_is_offered_with_host_control_and_confirmed_with_cancel_focused() {
    let offered = |name| {
        let f = every_state().into_iter().find(|s| s.name == name).unwrap().frames();
        host_options(&f.hosts.list[0]).contains(&HostOption::UpdateHost)
    };
    assert!(offered("power"));
    assert!(!offered("power not allowed"), "not without Host control");
    // Not again while it installs or is queued as an Install now, as after a tap: no second
    // update.started. An automatic update waiting for an idle PC, as while streaming, still
    // takes Install now.
    for phase in ["waiting", "downloading", "ready", "installing"] {
        for install_now in [false, true] {
            let mut f = every_state().into_iter().find(|s| s.name == "power").unwrap().frames();
            let update = f.hosts.list[0].native.update.as_mut().unwrap();
            (update.phase, update.install_now) = (phase.into(), install_now);
            let shown = host_options(&f.hosts.list[0]).contains(&HostOption::UpdateHost);
            assert_eq!(shown, phase != "installing" && !install_now, "{phase} {install_now}");
        }
    }
    assert!(!offered("paired"), "not where the PC does not list it");
    // Not allowed: one row says so for both.
    let locked = every_state().into_iter().find(|s| s.name == "power not allowed").unwrap().frames();
    let options = host_options(&locked.hosts.list[0]);
    assert!(options.contains(&HostOption::RestartPc) && !options.contains(&HostOption::ShutDownPc), "{options:?}");
    let mut f = every_state().into_iter().find(|s| s.name == "power").unwrap().frames();
    let id = f.hosts.list[0].id;
    f.l.dialog = Dialog::ConfirmUpdate { host: id, name: "PC".into(), version: "3.4.0".into() };
    f.l.focus_dialog = true;
    f.settle();
    let commands = f.press(Key::Enter);
    assert!(!commands.iter().any(|c| matches!(c, Command::InstallUpdate { .. })) && f.no_dialog(), "Enter on the focused Cancel");
    f.l.dialog = Dialog::ConfirmUpdate { host: id, name: "PC".into(), version: "3.4.0".into() };
    f.settle();
    let update = f.placed.iter().find(|(t, ..)| t == "Update").map(|p| p.1).expect("Update");
    let commands = f.click_at(update.center(), egui::PointerButton::Primary);
    assert!(commands.iter().any(|c| matches!(c, Command::InstallUpdate { host } if *host == id)));
    f.l.update_answered(id, "PC", Ok(UpdateStep::Back("3.4.0".into())));
    assert!(matches!(&f.l.dialog, Dialog::Message { body, .. } if body.contains("3.4.0")));
}

#[test]
fn sleep_pc_waits_for_the_offer_and_spends_it_only_when_confirmed() {
    let mut f = every_state().into_iter().find(|s| s.name == "can sleep").unwrap().frames();
    let id = f.hosts.list[0].id;
    let at = Handed::now();
    let offer = || Ok(SleepStep::Offer { token: "tok".into(), at });
    let sleeps = |commands: &[Command]| commands.iter().any(|c| matches!(c, Command::Sleep { .. }));
    let waiting = || Dialog::ConfirmSleep { host: id, ask: 1, name: "PC".into(), offer: None };

    // Cancelled while the PC is asked: its offer, or its failure, says nothing afterwards.
    f.l.dialog = waiting();
    f.l.focus_dialog = true;
    f.settle();
    assert!(!sleeps(&f.press(Key::Enter)), "Cancel has focus");
    assert!(f.no_dialog());
    f.l.sleep_answered(id, 1, "Gaming PC", false, offer());
    f.l.sleep_answered(id, 1, "Gaming PC", false, Err("The PC did not answer.".into()));
    assert!(f.no_dialog(), "the user walked away from it");

    // The offer fills the confirmation; one for another PC does not.
    f.l.dialog = waiting();
    f.l.focus_dialog = true;
    f.settle();
    f.l.sleep_answered(id + 1, 1, "Gaming PC", false, offer());
    assert!(matches!(f.l.dialog, Dialog::ConfirmSleep { offer: None, .. }));
    f.l.sleep_answered(id, 1, "Gaming PC", false, offer());
    assert!(matches!(&f.l.dialog, Dialog::ConfirmSleep { offer: Some((token, _)), .. } if token == "tok"));
    f.settle();
    assert!(!sleeps(&f.press(Key::Enter)), "Cancel still has focus once the offer is in");
    f.l.dialog = waiting();
    f.l.sleep_answered(id, 1, "Gaming PC", false, offer());
    f.settle();
    f.press(Key::ArrowRight);
    let commands = f.press(Key::Enter);
    assert!(commands.iter().any(|c| matches!(c, Command::Sleep { host, ask: 1, token, at: when } if *host == id && token == "tok" && *when == at)));
    assert!(f.no_dialog());

    // The answers to the confirmed request are shown, the host's refusal word for word.
    f.l.sleep_answered(id, 1, "Gaming PC", true, Ok(SleepStep::Accepted));
    assert!(matches!(&f.l.dialog, Dialog::Message { title, .. } if title == "Gaming PC is going to sleep"));
    f.l.dialog = Dialog::None;
    let refusal = "End all applications and sessions before requesting sleep";
    f.l.sleep_answered(id, 1, "Gaming PC", true, Err(refusal.into()));
    assert!(matches!(&f.l.dialog, Dialog::Message { title, body } if title == "Could not put Gaming PC to sleep" && body == refusal));
    // A failed offer the user still waits for is shown too.
    f.l.dialog = waiting();
    f.l.sleep_answered(id, 1, "Gaming PC", false, Err(crate::hosts::SLEEP_BUSY.into()));
    assert!(matches!(&f.l.dialog, Dialog::Message { body, .. } if body == crate::hosts::SLEEP_BUSY));
}

#[test]
fn a_sleep_pc_answer_waits_for_the_dialog_the_user_is_in() {
    let mut f = every_state().into_iter().find(|s| s.name == "can sleep").unwrap().frames();
    let id = f.hosts.list[0].id;
    let other = id + 1;
    f.l.dialog = Dialog::Rename { host: other, text: "Living r".into() };
    f.settle();
    f.l.sleep_answered(id, 1, "Gaming PC", true, Ok(SleepStep::Accepted));
    f.l.sleep_answered(id, 2, "Gaming PC", true, Err("The PC did not answer.".into()));
    f.settle();
    assert!(matches!(&f.l.dialog, Dialog::Rename { text, .. } if text == "Living r"), "the rename keeps its text");
    f.l.dialog = Dialog::None;
    f.settle();
    assert!(matches!(&f.l.dialog, Dialog::Message { title, .. } if title == "Gaming PC is going to sleep"), "then it is shown");
    f.l.dialog = Dialog::None;
    f.settle();
    assert!(matches!(&f.l.dialog, Dialog::Message { title, .. } if title == "Could not put Gaming PC to sleep"), "in order");
    f.l.dialog = Dialog::None;
    f.settle();
    assert!(f.no_dialog(), "each once");
}

#[test]
fn a_late_offer_from_a_cancelled_sleep_pc_is_not_taken_for_the_new_one() {
    let mut f = every_state().into_iter().find(|s| s.name == "can sleep").unwrap().frames();
    let id = f.hosts.list[0].id;
    let at = Handed::now();
    let offer = |token: &str| Ok(SleepStep::Offer { token: token.into(), at });
    let ask = |f: &mut Frames| {
        let mut commands = Vec::new();
        f.l.dialog = f.l.choose_host_option(&f.hosts.list[0], HostOption::Sleep, &mut commands).unwrap();
        match commands[..] {
            [Command::SleepOffer { host, ask }] if host == id => ask,
            _ => panic!("Sleep PC asks the PC for a token"),
        }
    };
    let first = ask(&mut f);
    f.l.dialog = Dialog::None;
    let second = ask(&mut f);
    assert_ne!(first, second);

    // The first request's token comes back after the second request replaced it on the PC.
    f.l.sleep_answered(id, first, "Gaming PC", false, offer("old"));
    assert!(matches!(f.l.dialog, Dialog::ConfirmSleep { offer: None, .. }), "the old token is dropped");
    f.l.sleep_answered(id, second, "Gaming PC", false, offer("new"));
    assert!(matches!(&f.l.dialog, Dialog::ConfirmSleep { offer: Some((token, _)), .. } if token == "new"));
    f.l.focus_dialog = true;
    f.settle();
    f.press(Key::ArrowRight);
    let commands = f.press(Key::Enter);
    assert!(commands.iter().any(|c| matches!(c, Command::Sleep { token, ask, .. } if token == "new" && *ask == second)));
    // A failure of the cancelled request is not shown over the new one either.
    f.l.dialog = Dialog::ConfirmSleep { host: id, ask: second, name: "PC".into(), offer: None };
    f.l.sleep_answered(id, first, "Gaming PC", false, Err("The PC did not answer.".into()));
    assert!(matches!(f.l.dialog, Dialog::ConfirmSleep { offer: None, .. }));
}

#[test]
fn sleep_pc_not_allowed_is_listed_with_the_reason_and_does_nothing() {
    let mut f = every_state().into_iter().find(|s| s.name == "sleep not allowed").unwrap().frames();
    f.ctx.enable_accesskit();
    assert!(host_options(&f.hosts.list[0]).contains(&HostOption::Sleep));
    f.press(Key::F2);
    assert!(f.drawn.iter().any(|t| t == crate::hosts::SLEEP_NOT_ALLOWED), "the reason is shown");
    // And heard: the row's name carries the reason, not only the painted line.
    let sleep = HostOption::Sleep.label();
    let said = format!("{}, {}", tr(sleep), tr(crate::hosts::SLEEP_NOT_ALLOWED));
    let labels: Vec<&str> = f.access.as_ref().unwrap().nodes.iter().filter_map(|(_, n)| n.label()).collect();
    assert!(labels.contains(&said.as_str()), "{said} in {labels:?}");
    let mut commands = Vec::new();
    let next = f.l.choose_host_option(&f.hosts.list[0], HostOption::Sleep, &mut commands);
    assert!(next.is_none() && commands.is_empty(), "nothing is asked of the PC");
    // A stock host never lists it.
    let stock = every_state().into_iter().find(|s| s.name == "paired").unwrap().frames();
    assert!(!host_options(&stock.hosts.list[0]).contains(&HostOption::Sleep));
}

#[test]
fn the_replace_prompt_starts_on_resume_else_cancel_never_on_the_button_that_quits() {
    for (resume, name) in [(Some(7), "with something to resume"), (None, "with nothing to resume")] {
        let mut f = Frames::new(record("Paired", true, false), Reach::Online);
        f.l.dialog = Dialog::ConfirmQuit {
            host: f.hosts.list[0].id,
            start: Some((9, "Zelda".into())),
            running: "Portal".into(),
            resume,
            named: Some(7),
        };
        f.l.focus_dialog = true;
        f.settle();
        let commands = f.press(Key::Enter);
        assert!(!commands.iter().any(|c| matches!(c, Command::QuitAndLaunch { .. })), "{name}: Enter must not quit");
        assert_eq!(
            commands.iter().any(|c| matches!(c, Command::Launch { app: 7, resume: true, .. })),
            resume.is_some(),
            "{name}: Enter resumes, or cancels when there is nothing to resume"
        );
        assert!(f.no_dialog(), "{name}");
    }
}

/// What an automatic start goes through (`open_play`): the PC's own report asks, whatever else is known.
#[test]
fn starting_over_a_game_the_pc_reports_asks_first() {
    let xml = r#"<?xml version="1.0" encoding="utf-8"?><root status_code="200"><hostname>PC</hostname><uniqueid>uid</uniqueid><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>1</PairStatus><currentgame>7</currentgame><state>SUNSHINE_SERVER_BUSY</state><Permission>1</Permission></root>"#;
    let info = nightjar_core::http::ServerInfo::parse(xml, 47989).expect("the answer parses");
    let mut f = Frames::with(record("Paired", true, false), Reach::Online, Some(info), vec![game(9)]);
    assert!(f.hosts.list[0].native.session.is_none(), "no session was read");
    let mut commands = Vec::new();
    let asked = f.l.start_app(&f.hosts.list[0], &game(9), &mut commands);
    assert!(matches!(asked, Some(Dialog::ConfirmQuit { .. })), "the running game is not in the list either");
    assert!(commands.is_empty());
}

#[test]
fn a_pc_that_confirms_replacement_starts_without_the_client_prompt() {
    let info = |extension: &str| {
        let xml = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><root status_code="200"><hostname>PC</hostname><uniqueid>uid</uniqueid><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>1</PairStatus><currentgame>7</currentgame><state>SUNSHINE_SERVER_BUSY</state><Permission>1</Permission>{extension}</root>"#
        );
        nightjar_core::http::ServerInfo::parse(&xml, 47989).expect("the answer parses")
    };
    for (extension, confirms) in [("", false), ("<RustHostVersion>3.0.0</RustHostVersion>", true)] {
        let mut f = Frames::with(record("Paired", true, false), Reach::Online, Some(info(extension)), vec![game(7), game(9)]);
        assert_eq!(f.hosts.list[0].confirms_replacement(), confirms);
        let id = f.hosts.list[0].id;
        f.l.go(Screen::Apps(id));
        f.settle();
        // The running game has focus first; the next poster is the other game.
        f.press(Key::ArrowRight);
        let commands = f.press(Key::Enter);
        let launched = commands.iter().any(|c| matches!(c, Command::Launch { app: 9, resume: false, .. }));
        assert_eq!(launched, confirms, "{extension}");
        assert_eq!(matches!(f.l.dialog, Dialog::ConfirmQuit { .. }), !confirms, "{extension}");
    }
}

#[test]
fn removing_a_pc_asks_first() {
    let mut f = Frames::new(record("Paired", true, false), Reach::Online);
    let last = host_options(&f.hosts.list[0]).len() - 1;
    let open_remove = |f: &mut Frames, round: &str| {
        f.press(Key::F2);
        for _ in 0..last {
            f.press(Key::ArrowDown);
        }
        let commands = f.press(Key::Enter);
        assert!(matches!(f.l.dialog, Dialog::ConfirmRemove { .. }), "{round}: the confirmation opens");
        assert!(!removes(&commands), "{round}: choosing Remove does not remove");
    };
    // Cancel has focus: Enter and Escape both leave the PC alone.
    open_remove(&mut f, "enter");
    let commands = f.press(Key::Enter);
    assert!(f.no_dialog());
    assert!(!removes(&commands));
    assert!(f.l.focused_key.is_some(), "Cancel keeps the card's focus");
    assert!(f.ctx.memory(|m| m.focused()).is_some(), "and the card has it again");
    open_remove(&mut f, "escape");
    let commands = f.press(Key::Escape);
    assert!(f.no_dialog());
    assert!(!removes(&commands));
    assert!(f.l.focused_key.is_some());
    // Only the Remove button does it.
    open_remove(&mut f, "confirm");
    f.press(Key::ArrowRight);
    let commands = f.frame(crate::gui::tap(Key::Enter).to_vec());
    assert!(removes(&commands));
    assert!(f.l.focused_key.is_none(), "the card goes with the PC");
}

#[test]
fn rename_keeps_the_name_on_this_device() {
    let mut f = Frames::new(record("Gaming PC", true, false), Reach::Online);
    f.press(Key::F2);
    f.press(Key::ArrowDown);
    f.press(Key::ArrowDown);
    f.press(Key::Enter);
    let Dialog::Rename { text, .. } = &mut f.l.dialog else { panic!("the rename dialog") };
    assert_eq!(text, "Gaming PC", "prefilled with the name shown");
    *text = "  Living room  ".into();
    let commands = f.press(Key::Enter);
    let alias = commands.iter().find_map(|c| match c {
        Command::Rename { alias, .. } => Some(alias.clone()),
        _ => None,
    });
    assert_eq!(alias.as_deref(), Some("  Living room  "));
    let id = f.hosts.list[0].id;
    assert!(f.hosts.rename(id, "  Living room  "));
    assert_eq!(f.hosts.list[0].display_name(), "Living room");
    assert!(f.hosts.rename(id, ""));
    assert_eq!(f.hosts.list[0].display_name(), "Gaming PC", "empty brings the PC's own name back");
    assert!(f.hosts.rename(id, "Gaming PC"));
    assert_eq!(f.hosts.list[0].record.alias, None, "the PC's own name is not stored as an alias");
}

#[test]
fn a_pc_that_never_reported_a_name_starts_the_rename_empty() {
    let mut unnamed = record("x", true, false);
    unnamed.name = String::new();
    let mut f = Frames::new(unnamed, Reach::Online);
    let id = f.hosts.list[0].id;
    assert_eq!(f.hosts.list[0].display_name(), "192.0.2.7", "the card falls back to the address");
    f.press(Key::F2);
    f.press(Key::ArrowDown);
    f.press(Key::ArrowDown);
    f.press(Key::Enter);
    let Dialog::Rename { text, .. } = &f.l.dialog else { panic!("the rename dialog") };
    assert_eq!(text, "", "the address is not offered as a name");
    // Saving the address back, or nothing, leaves no alias.
    assert!(f.hosts.rename(id, "192.0.2.7"));
    assert_eq!(f.hosts.list[0].record.alias, None);
    assert!(f.hosts.rename(id, ""));
    assert_eq!(f.hosts.list[0].record.alias, None);
}

/// The tab stops in the header band, in the order Tab reaches them.
fn header_stops(f: &mut Frames) -> Vec<Rect> {
    let mut stops: Vec<Rect> = Vec::new();
    for _ in 0..14 {
        f.press(Key::Tab);
        let Some(id) = f.ctx.memory(|m| m.focused()) else { continue };
        let Some(rect) = f.ctx.read_response(id).map(|r| r.rect) else { continue };
        if rect.center().y < 80.0 && !stops.contains(&rect) {
            stops.push(rect);
        }
    }
    stops
}

#[test]
fn tab_walks_the_header_left_to_right_and_the_actions_sit_at_the_right_edge() {
    // PCs: Home hides while there is no library; the destinations sit together beside the brand,
    // and Add PC at the right edge, where the search and the PC capsule are elsewhere.
    let mut f = Frames::new(record("Paired", true, false), Reach::Online);
    let stops = header_stops(&mut f);
    assert_eq!(stops.len(), 4, "PCs, Settings, Help and Add PC: {stops:?}");
    assert!(stops.windows(2).all(|w| w[0].right() <= w[1].left()), "in reading order: {stops:?}");
    assert!(stops[1].right() < 640.0, "the destinations sit at the left: {stops:?}");
    assert!(1280.0 - stops[3].right() < 80.0, "Add PC is at the right edge: {:?}", stops[3]);
    assert!(shows(&f, "Add PC") && !shows(&f, "Ctrl K"), "{:?}", f.drawn);
    // Hit targets a finger or a pointer finds: no destination is shorter than 36 points.
    assert!(stops.iter().all(|r| r.height() >= 36.0), "{stops:?}");

    // Home shows all four destinations in order, one after the other.
    let mut f = Frames::new(record("Paired", true, false), Reach::Online);
    f.l.go(Screen::Home);
    f.settle();
    let stops = header_stops(&mut f);
    assert_eq!(stops.len(), 6, "Home, PCs, Settings, Help, the search and the PC: {stops:?}");
    assert!(stops[..4].windows(2).all(|w| w[0].right() <= w[1].left() && w[1].left() - w[0].right() < 12.0), "in reading order, together: {stops:?}");
}

/// The page shown marks its destination in the top bar, below the top level too: a PC's library
/// marks PCs, a Settings page Settings.
#[test]
fn the_top_bar_marks_the_destination_of_the_page_shown() {
    assert_eq!(Screen::Apps(1).destination(), Screen::Hosts);
    assert_eq!(Screen::HostDetails(1).destination(), Screen::Hosts);
    assert_eq!(Screen::AppSettings(1).destination(), Screen::Hosts);
    assert_eq!(Screen::Game { host: 1, app: 2 }.destination(), Screen::Home);
    assert_eq!(Screen::AllGames.destination(), Screen::Home);
    assert_eq!(Screen::Profiles.destination(), Screen::Settings);
    for (_, screen) in DESTINATIONS {
        assert_eq!(screen.destination(), screen, "a destination marks itself");
    }
    // Drawn: the marked tab is filled, the others are not.
    let mut f = Frames::new(record("Desk", true, true), Reach::Online);
    let id = f.hosts.list[0].id;
    f.l.open(Screen::HostDetails(id));
    f.settle();
    let tab = |f: &Frames, name: &str| f.placed.iter().find(|(t, r, _)| t == name && r.center().y < 60.0).map(|p| p.1).expect(name);
    let pcs = tab(&f, "PCs");
    let settings = tab(&f, "Settings");
    let filled = |f: &Frames, at: Rect| meshes_holding(&f.shapes, design_tokens::TINT_SELECTED_FILL).iter().any(|r| r.contains_rect(at));
    assert!(filled(&f, pcs), "PCs is marked on a PC's page");
    assert!(!filled(&f, settings), "Settings is not");
}

/// A paired PC whose library holds `count` installed games with distinct titles and ids.
fn home_with_games(count: u32) -> Frames {
    use nightjar_core::http::native_api::library::{Artwork, Game, Library};
    let games = (0..count)
        .map(|n| Game {
            install_id: format!("install-{n}"),
            app_id: 1000 + n,
            title: format!("Game {n:02}"),
            game_id: Some(format!("steam:{n}")),
            store: None,
            installed: Some(true),
            running: false,
            artwork: Artwork { cover: None, version: String::new() },
        })
        .collect();
    let mut f = Frames::with(record("Desk", true, true), Reach::Online, Some(answer(true, 0)), Vec::new());
    f.hosts.list[0].native.library = Some(Library { revision: "r1".into(), games });
    f.l.go(Screen::Home);
    f.settle();
    f
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
    for g in &mut pc.native.library.as_mut().expect("a library").games {
        g.artwork = nightjar_core::http::native_api::library::Artwork { cover: Some(format!("/cover/{}", g.app_id)), version: "1".into() };
        pc.art.insert(g.app_id as i32, Art::Ready(texture.clone()));
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
    let g = &mut pc.native.library.as_mut().expect("a library").games[0];
    g.artwork = nightjar_core::http::native_api::library::Artwork { cover: Some("/cover/0".into()), version: "1".into() };
    pc.art.insert(g.app_id as i32, Art::Ready(texture.clone()));
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
    f.l.go(Screen::Settings);
    f.settle();
    let top = |f: &Frames| f.placed.iter().find(|(t, ..)| t == "Codec").map(|p| p.1.top()).expect("the Codec row");
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
    f.l.controller_mapping = crate::gamepad::Mapping { name: "Pad".into(), ..Default::default() };
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
    f.hosts.list[0].apps = Apps::Loaded(vec![nightjar_core::http::App { name: "Game 00".into(), uuid: "install-0".into(), ..game(1000) }]);
    f.hosts.list[0].native.library.as_mut().expect("the library").games[0].running = true;
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

/// A game's page: the title over one Play with Favourite on its line, its rows under them, and no
/// sentence with nothing to know. Quit game only for a game that runs.
#[test]
fn a_games_page_is_one_play_beside_favourite_over_its_rows() {
    let mut f = home_with_games(2);
    let pc = f.hosts.list[0].id;
    f.hosts.list[0].apps = Apps::Loaded(vec![nightjar_core::http::App { name: "Game 00".into(), uuid: "install-0".into(), ..game(1000) }]);
    f.l.go(Screen::Game { host: pc, app: 1000 });
    f.settle();
    let at = |f: &Frames, text: &str| f.placed.iter().rfind(|(t, r, _)| t == text && r.top() > chrome::PAGE_TOP).map(|p| p.1).unwrap_or_else(|| panic!("{text} in {:?}", f.drawn));
    let (title, play, star, settings) = (at(&f, "Game 00"), at(&f, "Play"), at(&f, "Favourite"), at(&f, "Stream settings"));
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
    f.hosts.list[0].native.library.as_mut().expect("the library").games[0].running = true;
    f.settle();
    let (resume, quit) = (at(&f, "Resume"), at(&f, "Quit game"));
    assert!((resume.center().y - quit.center().y).abs() < 2.0, "{resume:?} {quit:?}");
    assert_eq!(at(&f, "Stream settings").top(), settings.top(), "no jump for a chip that was not there");
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
    f.hosts.list[0].apps = Apps::Loaded(vec![nightjar_core::http::App { name: "Game 00".into(), uuid: "install-0".into(), ..game(1000) }]);
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
    let mut f = press(Apps::Loaded(vec![game(77)]));
    f.settle();
    assert!(shows(&f, "The stream could not start") && shows(&f, "The PC's app list has no entry for this game."), "{:?}", f.drawn);
    // Gone from the page, nothing starts behind the player's back.
    let mut f = press(Apps::Loading);
    f.l.go(Screen::Home);
    f.frame(Vec::new());
    f.hosts.list[0].apps = Apps::Loaded(vec![nightjar_core::http::App { name: "Game 00".into(), uuid: "install-0".into(), ..game(1000) }]);
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

/// A PC that cannot play yet offers what would let it, in the place and the colour of Play.
#[test]
fn a_games_page_on_an_asleep_or_unpaired_pc_offers_the_way_back() {
    let mut f = home_with_games(1);
    let pc = f.hosts.list[0].id;
    f.l.go(Screen::Game { host: pc, app: 1000 });
    f.settle();
    f.hosts.list[0].reach = Reach::Offline;
    f.settle();
    assert!(shows(&f, "Desk  \u{b7}  Offline"), "{:?}", f.drawn);
    assert!(!shows(&f, "Play"), "{:?}", f.drawn);
    let commands = f.press(Key::Enter);
    assert!(commands.iter().any(|c| matches!(c, Command::Refresh(h) | Command::Wake(h) if *h == pc)));
}

/// Android's game page: a game the PC lists as not installed has Play off and says why, and a
/// copy on another PC that is not installed says so under "Also on".
#[test]
fn a_game_the_pc_has_not_installed_cannot_be_played_from_its_page() {
    let mut f = home_with_games(2);
    f.hosts.list[0].native.library.as_mut().expect("the library").games[0].installed = Some(false);
    // Game 01 runs: Play could not close it, so nothing says it would.
    f.hosts.list[0].native.library.as_mut().unwrap().games[1].running = true;
    // A second PC with the same game, not installed there either.
    let mut hosts = Hosts::without_discovery(vec![record("Desk", true, true), record("Den", true, false)], Arc::new(|| {}));
    for host in &mut hosts.list {
        host.reach = Reach::Online;
        host.native.library = f.hosts.list[0].native.library.clone();
    }
    hosts.list[0].info = f.hosts.list[0].info.clone();
    hosts.list[0].apps = Apps::Loaded(vec![nightjar_core::http::App { name: "Game 00".into(), uuid: "install-0".into(), ..game(1000) }]);
    f.hosts = hosts;
    let pc = f.hosts.list[0].id;
    f.l.go(Screen::Game { host: pc, app: 1000 });
    f.settle();
    assert!(shows(&f, "Desk  \u{b7}  Not installed"), "{:?}", f.drawn);
    assert!(shows(&f, "Den, not installed"), "Also on marks it: {:?}", f.drawn);
    let closes = "Game 01 is running. Playing this game closes it.";
    assert!(!shows(&f, closes), "{:?}", f.drawn);
    assert!(f.ctx.memory(|m| m.focused()).is_some(), "something has focus for the D-pad");
    let play = f.placed.iter().find(|(t, ..)| t == "Play").map(|p| p.1).expect("Play is shown");
    let commands = f.click_at(play.center(), egui::PointerButton::Primary);
    assert!(!commands.iter().any(|c| matches!(c, Command::Launch { .. })), "Play is off");
    // Installed, the same page plays it.
    f.hosts.list[0].native.library.as_mut().unwrap().games[0].installed = Some(true);
    f.settle();
    assert!(!shows(&f, "Not installed on Desk"), "{:?}", f.drawn);
    assert!(shows(&f, closes), "{:?}", f.drawn);
    f.hosts.list[0].native.library.as_mut().unwrap().games[1].running = false;
    f.settle();
    let play = f.placed.iter().find(|(t, ..)| t == "Play").map(|p| p.1).expect("Play is shown");
    let commands = f.click_at(play.center(), egui::PointerButton::Primary);
    assert!(commands.iter().any(|c| matches!(c, Command::Launch { app: 1000, .. })), "Play starts it");
}

#[test]
fn a_games_page_names_its_store_when_the_pc_sends_only_the_key() {
    use nightjar_core::http::native_api::library::Store;
    let mut f = home_with_games(1);
    let store = |source: &str, name: &str| Some(Store { source: source.into(), id: "1".into(), name: name.into() });
    f.hosts.list[0].native.library.as_mut().expect("the library").games[0].store = store("microsoft", "");
    let pc = f.hosts.list[0].id;
    f.l.go(Screen::Game { host: pc, app: 1000 });
    f.settle();
    assert!(shows(&f, "Xbox"), "the key's known name: {:?}", f.drawn);
    // The PC's own name wins.
    f.hosts.list[0].native.library.as_mut().unwrap().games[0].store = store("microsoft", "Microsoft Store");
    f.settle();
    assert!(shows(&f, "Microsoft Store") && !shows(&f, "Xbox"), "{:?}", f.drawn);
}

/// Settings is a top-level page like Home and PCs: no Back in its header. Its sub-pages keep theirs.
#[test]
fn settings_has_no_back_but_its_sub_pages_do() {
    let mut f = Frames::new(record("Desk", true, true), Reach::Online);
    f.l.go(Screen::Settings);
    f.settle();
    let below_bar = |f: &Frames, name: &str| f.placed.iter().any(|(t, r, _)| t == name && r.center().y > 60.0 && r.center().y < 140.0);
    assert!(shows(&f, "Settings") && !below_bar(&f, "Settings") && back_rect(&f).is_none(), "{:?}", f.drawn);
    f.l.open(Screen::Profiles);
    f.settle();
    assert!(back_rect(&f).is_some(), "Back is there on a sub-page");
}

/// Where the round Back shows, on a page that has one.
fn back_rect(f: &Frames) -> Option<Rect> {
    f.ctx.read_response(Id::unique("back")).map(|r| r.rect)
}

/// However far a long page scrolled, Back is one click away: it rises with the page, then holds in
/// the bar where the brand gave way to it. It takes the click over the cover passing beneath.
#[test]
fn back_holds_in_the_bar_when_the_page_scrolled_far() {
    let mut f = home_with_games(70);
    f.l.open(Screen::AllGames);
    f.settle();
    let rest = back_rect(&f).expect("Back on All games");
    assert_eq!(rest.top(), chrome::PAGE_TOP, "at rest it heads the page");
    assert!(shows(&f, "Nightjar"), "the brand is there at rest: {:?}", f.drawn);
    f.scroll_to_end();
    let held = back_rect(&f).expect("Back after scrolling");
    assert!((held.top() - 22.0).abs() < 0.5 && held.left() == rest.left(), "held in the bar: {held:?}");
    assert!(!shows(&f, "Nightjar"), "the brand gave way: {:?}", f.drawn);
    f.click_at(held.center(), egui::PointerButton::Primary);
    assert_eq!(f.l.screen, Screen::Home, "the click left the page and opened no game");
    assert_eq!(chrome::brand_alpha(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0)), None), 1.0, "Home keeps its brand");
}

/// The brand fades over the same stretch of scrolling each time, so Back never rides over its words.
#[test]
fn the_brand_is_gone_before_back_reaches_it() {
    let screen = Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0));
    let alpha_at = |scrolled: f32| chrome::brand_alpha(screen, Some(chrome::PAGE_TOP - scrolled));
    assert_eq!(alpha_at(0.0), 1.0);
    assert_eq!(alpha_at(16.0), 1.0);
    assert!(alpha_at(26.0) > 0.0 && alpha_at(26.0) < 1.0);
    assert_eq!(alpha_at(36.0), 0.0);
    // Back's top passes the mark's foot (59) at 29 scrolled: the brand is mostly gone by then.
    assert!(alpha_at(29.0) < 0.4);
    assert_eq!(chrome::back_top(screen, chrome::PAGE_TOP - 500.0), 22.0);
    assert_eq!(chrome::back_top(screen, chrome::PAGE_TOP), chrome::PAGE_TOP);
}

/// Under a short list of PCs: the search goes on, and How to connect opens the pairing guide.
#[test]
fn a_short_pc_list_says_the_search_goes_on_and_links_how_to_connect() {
    let mut f = Frames::new(record("Desk", true, true), Reach::Online);
    f.l.go(Screen::Hosts);
    f.settle();
    assert!(shows(&f, "Looking for other PCs on this network"), "{:?}", f.drawn);
    let link = f.placed.iter().find(|(t, ..)| t == "How to connect").map(|p| p.1).expect("the link");
    f.click_at(link.center(), egui::PointerButton::Primary);
    let commands = f.settle();
    let opened = |c: &Command| matches!(c, Command::OpenGuide("pairing.md"));
    let mut commands = commands;
    if !commands.iter().any(opened) {
        commands = f.click_at(link.center(), egui::PointerButton::Primary);
    }
    assert!(commands.iter().any(opened), "a click opens the pairing guide");
}

/// The pairing PIN is what the dialog is for: large, bold digits in the amber accent.
#[test]
fn the_pairing_pin_is_large_bold_and_amber() {
    fn digits(shape: &egui::Shape, out: &mut Vec<(String, egui::FontId, Color32)>) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| digits(s, out)),
            egui::Shape::Text(t) => {
                if let Some(section) = t.galley.job.sections.first() {
                    out.push((t.galley.text().to_owned(), section.format.font_id.clone(), t.fallback_color));
                }
            }
            _ => {}
        }
    }
    let mut f = Frames::new(record("Studio", false, false), Reach::Online);
    let id = f.hosts.list[0].id;
    f.l.dialog = Dialog::Pairing { host: id, pin: "4071".into() };
    // Past the dialog's fade-in.
    for _ in 0..4 {
        f.settle();
    }
    let mut texts = Vec::new();
    f.shapes.iter().for_each(|c| digits(&c.shape, &mut texts));
    let pin: Vec<_> = texts.iter().filter(|(t, ..)| t.len() == 1 && "4071".contains(t.as_str())).collect();
    assert_eq!(pin.len(), 4, "{texts:?}");
    // Centred in the dialog's 440-point column, which starts where its title does.
    let left = f.placed.iter().find(|(t, ..)| t == "Pair with Studio").map(|p| p.1.left()).expect("the title");
    let digit = |d: &str| f.placed.iter().find(|(t, ..)| t == d).map(|p| p.1.center().x).expect("the digit");
    let middle = (digit("4") + digit("1")) / 2.0;
    assert!((middle - (left + 220.0)).abs() < 2.0, "the PIN is centred: {middle} in {left}..{}", left + 440.0);
    assert!(pin.iter().all(|(_, font, color)| font.size >= 40.0 && *font == semibold(font.size) && *color == Color32::from(MOON)), "{pin:?}");
}

/// Home's card for a PC to pair says only that, whole: no "Games not loaded yet" cut short after it.
#[test]
fn the_home_card_of_a_pc_to_pair_fits_its_state() {
    use nightjar_core::http::native_api::library::{Artwork, Game, Library};
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    let mut l = launcher(true, false);
    l.identity_ready = true;
    let hosts = Hosts::without_discovery(vec![record("Desk", true, true), HostRecord { address: "192.0.2.12".into(), ..record("Studio", false, false) }], Arc::new(|| {}));
    let mut f = Frames::over(ctx, l, hosts);
    f.hosts.list[0].reach = Reach::Online;
    f.hosts.list[0].info = Some(answer(true, 0));
    let game = Game {
        install_id: "install-0".into(),
        app_id: 1000,
        title: "Game 00".into(),
        game_id: None,
        store: None,
        installed: Some(true),
        running: false,
        artwork: Artwork { cover: None, version: String::new() },
    };
    f.hosts.list[0].native.library = Some(Library { revision: "r1".into(), games: vec![game] });
    f.hosts.list[1].reach = Reach::Online;
    f.hosts.list[1].info = Some(answer(false, 0));
    f.l.go(Screen::Home);
    f.settle();
    assert!(shows(&f, "Ready to pair"), "{:?}", f.drawn);
    assert!(!f.drawn.iter().any(|t| t.contains("Games not") || t.ends_with('…')), "{:?}", f.drawn);
}

/// Before any PC, Home and PCs both link How to connect under their main action, as Android's
/// empty discovery card does, and it opens the pairing guide.
/// Frames over a list with no PCs, on a network being searched.
fn first_run_frames(records: Vec<crate::config::HostRecord>) -> Frames {
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    let mut l = launcher(true, false);
    l.identity_ready = true;
    let mut hosts = Hosts::without_discovery(records, Arc::new(|| {}));
    hosts.discovering = true;
    Frames::over(ctx, l, hosts)
}

/// The first run is the moon, what is happening in one line, and one action; Home sends it there.
#[test]
fn the_first_run_is_one_line_and_add_pc_from_home_too() {
    for screen in [Screen::Home, Screen::Hosts] {
        let mut f = first_run_frames(Vec::new());
        f.l.go(screen);
        f.settle();
        assert_eq!(f.l.screen, Screen::Hosts, "{:?}", f.drawn);
        assert!(shows(&f, "Searching for PCs\u{2026}"), "{:?}", f.drawn);
        let adds = f.placed.iter().filter(|(t, ..)| t == "Add PC").count();
        assert_eq!(adds, 1, "the page's, not the bar's as well: {:?}", f.drawn);
        assert!(!shows(&f, "How to connect") && !f.drawn.iter().any(|t| t.to_lowercase().contains("host")), "{:?}", f.drawn);
        let add = f.placed.iter().find(|(t, ..)| t == "Add PC").map(|p| p.1).expect("Add PC");
        let focused = f.ctx.memory(|m| m.focused()).and_then(|i| f.ctx.read_response(i)).expect("Add PC has focus").rect;
        assert!(focused.contains(add.center()), "{focused:?}");
        f.press(Key::Enter);
        assert!(!matches!(f.l.dialog, Dialog::None), "Enter opens Add PC");
    }
}

/// With no search running it says so, out loud; the dot is off and Add PC is the way on.
#[test]
fn a_first_run_that_cannot_search_says_so() {
    let mut f = first_run_frames(Vec::new());
    f.hosts.discovering = false;
    f.l.go(Screen::Hosts);
    f.settle();
    assert!(shows(&f, "Can't search this network for PCs") && !shows(&f, "Searching for PCs\u{2026}"), "{:?}", f.drawn);
    assert!(shows(&f, "Add PC"), "{:?}", f.drawn);
}

/// A found PC arrives as a card with Pair as the page's one moon action, which takes the focus; Add PC
/// steps down, and more PCs than a first run has make the page a list.
#[test]
fn a_found_pc_rises_in_with_pair_and_add_pc_steps_down() {
    let mut f = first_run_frames(Vec::new());
    f.l.go(Screen::Hosts);
    f.settle();
    let before = f.placed.iter().find(|(t, ..)| t == "Add PC").map(|p| p.1).expect("Add PC");
    let mut found = Hosts::without_discovery(vec![record("Game Room", false, false)], Arc::new(|| {}));
    found.discovering = true;
    found.list[0].reach = Reach::Online;
    f.hosts = found;
    let name = |f: &Frames| f.placed.iter().find(|(t, r, _)| t == "Game Room" && r.top() > chrome::PAGE_TOP).map(|p| p.1).unwrap_or_else(|| panic!("{:?}", f.drawn));
    f.frame(Vec::new());
    f.frame(Vec::new());
    let rising = name(&f);
    f.settle();
    let settled = name(&f);
    assert!(rising.top() > settled.top(), "it rises: {rising:?} to {settled:?}");
    let at = |f: &Frames, text: &str| f.placed.iter().find(|(t, ..)| t == text).map(|p| p.1).unwrap_or_else(|| panic!("{text} in {:?}", f.drawn));
    let (pair, line, add) = (at(&f, "Pair"), at(&f, "Searching for PCs\u{2026}"), at(&f, "Add PC"));
    assert!(settled.bottom() <= pair.top() && pair.bottom() <= line.top() && line.bottom() <= add.top(), "{pair:?} {line:?} {add:?}");
    assert!(add.top() != before.top(), "Add PC stepped down");
    assert_eq!(f.placed.iter().filter(|(t, ..)| t == "PCs").count(), 1, "the tab's alone, no page title: {:?}", f.drawn);
    let focused = f.ctx.memory(|m| m.focused()).and_then(|i| f.ctx.read_response(i)).expect("Pair has focus").rect;
    assert!(focused.contains(pair.center()), "{focused:?}");
    let commands = f.press(Key::Enter);
    let id = f.hosts.list[0].id;
    assert!(commands.iter().any(|c| matches!(c, Command::Pair(host) if *host == id)));
    // Paired, it is the ordinary list.
    let mut paired = Hosts::without_discovery(vec![record("Game Room", true, false)], Arc::new(|| {}));
    paired.discovering = true;
    paired.list[0].reach = Reach::Online;
    f.hosts = paired;
    f.settle();
    assert!(!shows(&f, "Searching for PCs\u{2026}"), "{:?}", f.drawn);
}

/// A saved PC is on Home from the first frame, before it has answered: the first run never flashes on a start.
#[test]
fn a_saved_pc_that_has_not_answered_yet_is_not_the_first_run() {
    let mut f = first_run_frames(vec![record("Desk", true, false)]);
    f.l.go(Screen::Home);
    f.settle();
    assert_eq!(f.l.screen, Screen::Home);
    assert!(!shows(&f, "Searching for PCs\u{2026}"), "{:?}", f.drawn);
}

/// A saved PC that answers as offline with no library saved is still its card on Home, with Wake in its options.
#[test]
fn an_offline_pc_with_nothing_saved_is_a_card_not_the_first_run() {
    let mut f = Frames::new(record("Den", true, true), Reach::Offline);
    f.l.go(Screen::Home);
    f.settle();
    assert_eq!(f.l.screen, Screen::Home);
    assert!(shows(&f, "PCs") && shows(&f, "Den"), "{:?}", f.drawn);
    assert!(host_options(&f.hosts.list[0]).contains(&HostOption::Wake));
}

/// A settings row stays a readable width on a wide window instead of stretching across it.
#[test]
fn rows_keep_a_readable_width_on_a_wide_window() {
    let ctx = egui::Context::default();
    let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1920.0, 1080.0))), ..Default::default() };
    let (label, description) = ("Label", "Description");
    let mut span = 0.0;
    let mut output = ctx.run_ui(input, |ui| {
        let left = ui.max_rect().left();
        span = row(ui, label, description, |ui| ui.button("x")).rect.right() - left;
    });
    output.textures_delta.clear();
    assert!(span <= SETTINGS_W && span > SETTINGS_W - 60.0, "the row's control ends at {span}");
}

/// The game page's quit on leave switch, now a row like every setting, still sends the choice.
#[test]
fn the_quit_on_leave_switch_sends_the_choice() {
    let mut f = home_with_games(1);
    f.ctx.enable_accesskit();
    let pc = f.hosts.list[0].id;
    f.hosts.list[0].apps = Apps::Loaded(vec![nightjar_core::http::App { name: "Game 00".into(), uuid: "install-0".into(), ..game(1000) }]);
    f.l.go(Screen::Game { host: pc, app: 1000 });
    f.settle();
    let bounds = f.access.as_ref().expect("AccessKit is on").nodes.iter()
        .find(|(_, n)| n.label().is_some_and(|l| l.starts_with("Quit game when I leave")))
        .and_then(|(_, n)| n.bounds())
        .expect("the switch has the row's name");
    let at = Pos2::new(((bounds.x0 + bounds.x1) / 2.0) as f32, ((bounds.y0 + bounds.y1) / 2.0) as f32);
    let commands = f.click_at(at, egui::PointerButton::Primary);
    assert!(commands.iter().any(|c| matches!(c, Command::SetQuitOnLeave { enabled: true, .. })), "a click turns it on");
}

#[test]
fn a_pairing_that_was_cancelled_never_shows_an_error() {
    let mut f = Frames::new(record("PC", false, false), Reach::Online);
    let id = f.hosts.list[0].id;
    let waiting = |f: &mut Frames| {
        f.l.dialog = Dialog::Pairing { host: id, pin: "1234".into() };
        f.l.focus_dialog = true;
        f.settle();
    };
    waiting(&mut f);
    let commands = f.press(Key::Escape);
    assert!(commands.iter().any(|c| matches!(c, Command::CancelPair)), "Escape cancels the request");
    assert!(!f.l.pairing_finished(id, Err(crate::hosts::PAIR_NETWORK.into())), "the late failure");
    assert!(f.no_dialog(), "is not shown");
    assert!(!f.l.pairing_finished(id, Ok(())), "and a late success does not open the library");
    assert!(f.no_dialog());
    // A pairing the user still waits for does report both.
    waiting(&mut f);
    assert!(!f.l.pairing_finished(id, Err(crate::hosts::PAIR_PIN_WRONG.into())));
    assert!(matches!(f.l.dialog, Dialog::PairError { one_time: false, .. }));
    waiting(&mut f);
    assert!(f.l.pairing_finished(id, Ok(())));
    assert!(f.no_dialog());
    // Another PC's answer is not this dialog's.
    waiting(&mut f);
    assert!(!f.l.pairing_finished(id + 1, Err(crate::hosts::PAIR_NETWORK.into())));
    assert!(matches!(f.l.dialog, Dialog::Pairing { .. }));
}

#[test]
fn a_failed_pairing_offers_to_try_again() {
    let mut f = Frames::new(record("PC", false, false), Reach::Online);
    let id = f.hosts.list[0].id;
    f.l.pair_failed(id, crate::hosts::PAIR_PIN_WRONG, false);
    f.settle();
    let commands = f.press(Key::Enter);
    assert!(commands.iter().any(|c| matches!(c, Command::Pair(host) if *host == id)), "Try again pairs again");
    f.l.pair_failed(id, crate::hosts::PAIR_OTP_PIN_WRONG, true);
    f.settle();
    f.press(Key::Enter);
    assert!(matches!(f.l.dialog, Dialog::Otp { .. }), "a one-time PIN failure goes back to the PIN form");
}

#[test]
fn add_pc_takes_a_pasted_pairing_link_or_one_from_the_clipboard() {
    let mut f = Frames::new(record("PC", false, false), Reach::Online);
    let link = "rubylight://pair?v=1&host=192.168.4.2&port=47989&otp=4821&fp=00";
    f.l.dialog = Dialog::AddHost { text: link.into(), error: None };
    f.l.focus_dialog = true;
    f.settle();
    let add = f.placed.iter().rev().find(|(t, ..)| t == tr("Add")).map(|(_, rect, _)| rect.center()).expect("Add is drawn");
    let commands = f.click_at(add, egui::PointerButton::Primary);
    assert!(commands.iter().any(|c| matches!(c, Command::AddHost(text) if text == link)), "the link goes to Add PC as typed");
    assert!(f.no_dialog());

    f.l.dialog = Dialog::AddHost { text: String::new(), error: None };
    f.settle();
    let paste = f.placed.iter().rev().find(|(t, ..)| t == tr("Paste link")).map(|(_, rect, _)| rect.center()).expect("Paste link is drawn");
    let commands = f.click_at(paste, egui::PointerButton::Primary);
    assert!(commands.iter().any(|c| matches!(c, Command::AddFromClipboard)), "Paste link reads the clipboard");
    assert!(f.no_dialog());
}

/// Screenshots of the main pages in both palettes, painted on the CPU from made-up PCs (no
/// network): `NIGHTJAR_SHOTS=<dir> cargo test -p nightjar-desktop page_screenshots -- --ignored`.
#[test]
fn a_pc_that_is_up_reads_green_and_amber_never_stands_for_a_state() {
    crate::theme::own_palette();
    let records = vec![record("Up", true, true), record("Playing", true, true), record("Down", true, true), record("New", false, false), record("Unsure", true, true)];
    let mut hosts = Hosts::without_discovery(records, Arc::new(|| {}));
    hosts.list[0].reach = Reach::Online;
    hosts.list[0].info = Some(answer(true, 0));
    hosts.list[1].reach = Reach::Online;
    hosts.list[1].info = Some(answer(true, 7));
    hosts.list[2].reach = Reach::Offline;
    hosts.list[3].reach = Reach::Online;
    hosts.list[3].info = Some(answer(false, 0));
    hosts.list[4].reach = Reach::Unknown;
    let looks: Vec<_> = hosts.list.iter().map(host_status).collect();
    for (host, (words, color, ring)) in hosts.list.iter().zip(&looks) {
        assert!(!words.is_empty(), "{}: a state is always a word", host.record.name);
        assert_ne!(**color, *MOON, "{}: amber is the action, not a state", host.record.name);
        assert_ne!(**ring, *MOON, "{}: amber is the action, not a state", host.record.name);
    }
    assert_eq!([*looks[0].1, *looks[1].1], [*OK, *OK], "up and paired, playing or not, is green");
    assert_eq!(*looks[2].1, *OUTLINE, "offline is quiet");
    assert_eq!(*looks[3].1, *TERTIARY, "not paired is information");
    assert_ne!(*looks[4].2, *OK, "a PC still being checked is not shown as up");
}

/// Home on its own, gathered as the page does.
fn draw_home(l: &mut Launcher, ui: &mut Ui, view: &View<'_>, commands: &mut Vec<Command>) {
    let sources: Vec<Source> = view.hosts.list.iter().map(Source::of).collect();
    let data = home::HomeData::gather(l, &sources);
    l.home(ui, view, &sources, data, commands);
    glass::flush_deferred(ui);
}

#[test]
fn a_running_cover_wears_a_green_dot_and_no_amber() {
    crate::theme::own_palette();
    let mut out = Vec::new();
    running_dot(&mut out, Rect::from_min_size(Pos2::ZERO, vec2(160.0, 240.0)));
    let fills: Vec<Color32> = out.iter().filter_map(|shape| match shape {
        egui::Shape::Circle(c) => Some(c.fill),
        _ => None,
    }).collect();
    assert!(fills.contains(&*OK), "a green dot: {fills:?}");
    assert!(!fills.contains(&*MOON), "and no amber: {fills:?}");
}

#[test]
fn primary_and_quiet_buttons_take_the_control_height() {
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    let mut heights = [0.0; 2];
    for n in 0..2 {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0))), time: Some(f64::from(n)), ..Default::default() };
        let mut output = ctx.run_ui(input, |ui| {
            heights = [primary_button(ui, "Play").rect.height(), quiet_button(ui, "Cancel").rect.height()];
        });
        output.textures_delta.clear();
    }
    assert_eq!(heights, [CONTROL_HEIGHT; 2]);
}

#[test]
fn a_switched_off_primary_button_is_grey_not_faded_amber() {
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    let mut fills = Vec::new();
    for enabled in [true, false] {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0))), ..Default::default() };
        let mut output = ctx.run_ui(input, |ui| {
            ui.add_enabled_ui(enabled, |ui| primary_button(ui, "Play"));
        });
        output.textures_delta.clear();
        // Amber whatever the fade: red well above blue (the surfaces are blue-black).
        let is_amber = |c: Color32| f32::from(c.r()) > 1.6 * f32::from(c.b()) + 10.0;
        let painted: Vec<Color32> = output
            .shapes
            .iter()
            .flat_map(|clipped| match &clipped.shape {
                egui::Shape::Mesh(mesh) => mesh.vertices.iter().map(|v| v.color).filter(|c| c.a() > 0).collect(),
                _ => Vec::new(),
            })
            .collect();
        let amber = painted.iter().any(|&c| is_amber(c));
        fills.push((enabled, amber, painted));
    }
    assert!(fills[0].1, "an enabled Play is amber: {:?}", fills[0].2);
    assert!(!fills[1].1 && !fills[1].2.is_empty(), "a switched-off Play is drawn, but not in amber: {:?}", fills[1].2);
}

/// The outlines of the glass meshes that hold `color`: glass paints meshes, not filled rects.
fn meshes_holding(shapes: &[egui::epaint::ClippedShape], color: Color32) -> Vec<Rect> {
    fn walk(shape: &egui::Shape, color: Color32, out: &mut Vec<Rect>) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, color, out)),
            egui::Shape::Mesh(mesh) if mesh.vertices.iter().any(|v| v.color == color) => out.push(mesh.calc_bounds()),
            _ => {}
        }
    }
    let mut found = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, color, &mut found);
    }
    found
}

/// What each text shape shows on screen (a cut line ends in "…"), with where it landed.
fn collect_visible(shape: &egui::Shape, out: &mut Vec<(String, Rect)>) {
    match shape {
        egui::Shape::Text(text) => {
            let shown: String = text.galley.rows.iter().map(|row| row.text()).collect();
            out.push((shown, text.galley.rect.translate(text.pos.to_vec2())));
        }
        egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect_visible(s, out)),
        _ => {}
    }
}

fn visible(f: &Frames) -> Vec<(String, Rect)> {
    let mut shown = Vec::new();
    for clipped in &f.shapes {
        collect_visible(&clipped.shape, &mut shown);
    }
    shown
}

fn hover_card(f: &mut Frames, card: Rect) {
    f.frame(vec![egui::Event::PointerMoved(card.center())]);
    for _ in 0..30 {
        f.frame(Vec::new());
    }
}

/// The card of the only PC, by its own id.
fn pc_card_rect(f: &Frames) -> Rect {
    let id = f.hosts.list[0].id;
    f.ctx.read_response(Id::unique(("pc-card", id))).expect("the card").rect
}

#[test]
fn a_long_pc_name_ends_in_an_ellipsis_inside_the_card_and_the_tooltip_shows_it_whole() {
    for zoomed in [false, true] {
        let full = "The Very Long Name Of The Living Room Gaming PC Under The Television";
        let mut f = Frames::new(record(full, true, true), Reach::Online);
        if zoomed {
            fit_150(&mut f);
            f.size = vec2(720.0 / 1.5, 480.0 / 1.5);
        }
        f.settle();
        let card = pc_card_rect(&f);
        assert!(card.right() <= f.size.x, "{zoomed}: the card {card:?} fits the window");
        let (line, rect) = visible(&f).into_iter().find(|(t, _)| t.starts_with("The Very")).expect("the name is drawn");
        assert!(line.ends_with('\u{2026}'), "{zoomed}: the name is cut: {line}");
        assert!(rect.right() <= card.right() + 0.5, "{zoomed}: {line} ends at {} in a card ending at {}", rect.right(), card.right());
        // Up to the card's top, away from its action and More.
        let over = pos2(card.left() + 8.0, card.top() + 8.0);
        f.frame(vec![egui::Event::PointerMoved(over)]);
        for _ in 0..30 {
            f.frame(Vec::new());
        }
        assert!(visible(&f).iter().any(|(t, _)| t.starts_with(full)), "{zoomed}: the tooltip shows it whole: {:?}", visible(&f));
    }
}

#[test]
fn a_short_pc_name_is_drawn_whole_with_no_tooltip() {
    let mut f = Frames::new(record("Den", true, true), Reach::Online);
    let card = pc_card_rect(&f);
    hover_card(&mut f, card);
    let shown = visible(&f);
    assert!(shown.iter().any(|(t, _)| t == "Den"), "{shown:?}");
    assert!(!shown.iter().any(|(t, _)| t.starts_with("Den, ")), "no tooltip: {shown:?}");
}

/// A PC's one action says what the screen and the line say, and does it.
#[test]
fn a_pc_offers_the_one_action_its_state_calls_for() {
    let action_of = |f: &Frames| crate::ui::pcs::pc_action(&f.hosts.list[0]);
    let line_of = |f: &Frames| crate::ui::pcs::pc_line_of(&f.hosts.list[0]);
    use crate::ui::pcs::{PcAction, PcLine};
    let click_action = |f: &mut Frames| {
        let at = f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).expect("the action has focus").rect.center();
        f.click_at(at, egui::PointerButton::Primary)
    };
    // Up and paired, idle: Open, a green "Online", and no made-up latency.
    let mut f = Frames::with(record("Den", true, true), Reach::Online, Some(answer(true, 0)), vec![game(7)]);
    assert_eq!(action_of(&f), PcAction::Open);
    assert_eq!(line_of(&f), PcLine::Online);
    assert!(shows(&f, "Open") && shows(&f, "Online") && !f.drawn.iter().any(|t| t.ends_with(" ms")), "{:?}", f.drawn);
    let id = f.hosts.list[0].id;
    assert!(click_action(&mut f).iter().any(|c| matches!(c, Command::OpenApps(h) if *h == id)));

    // Running a game of its known list: Resume starts it again, the line names the game.
    let mut f = Frames::with(record("Den", true, true), Reach::Online, Some(answer(true, 7)), vec![game(7)]);
    f.hosts.list[0].apps = Apps::Loaded(vec![game(7)]);
    f.settle();
    assert_eq!(action_of(&f), PcAction::Resume(7));
    assert!(matches!(line_of(&f), PcLine::Running(_)));
    assert!(shows(&f, "Resume"), "{:?}", f.drawn);
    let commands = click_action(&mut f);
    assert!(commands.iter().any(|c| matches!(c, Command::Launch { app: 7, resume: true, .. })));

    // Running but the list is not known: it asks for it, and offers what still works.
    let mut f = Frames::with(record("Den", true, true), Reach::Online, Some(answer(true, 7)), Vec::new());
    f.hosts.list[0].apps = Apps::NotLoaded;
    let commands = f.settle();
    assert_eq!(action_of(&f), PcAction::Open);
    assert!(commands.iter().any(|c| matches!(c, Command::LoadApps(_))));

    // Asleep with a saved address: Wake. Without one: Try again, which checks again.
    let mut f = Frames::new(HostRecord { mac: Some("00:1a:2b:3c:4d:5e".into()), ..record("Den", true, true) }, Reach::Offline);
    assert_eq!((action_of(&f), line_of(&f)), (PcAction::Wake, PcLine::Offline));
    assert!(shows(&f, "Wake") && shows(&f, "Offline"), "{:?}", f.drawn);
    assert!(click_action(&mut f).iter().any(|c| matches!(c, Command::Wake(_))));
    let mut f = Frames::new(HostRecord { mac: None, ..record("Den", true, true) }, Reach::Offline);
    assert_eq!(action_of(&f), PcAction::TryAgain);
    assert!(click_action(&mut f).iter().any(|c| matches!(c, Command::Refresh(_))));

    // Up and not paired: Pair, and the lock says why.
    let mut f = Frames::new(record("Den", false, false), Reach::Online);
    assert_eq!((action_of(&f), line_of(&f)), (PcAction::Pair, PcLine::NotPaired));
    assert!(shows(&f, "Not paired"), "{:?}", f.drawn);
    assert!(click_action(&mut f).iter().any(|c| matches!(c, Command::Pair(_))));
}

/// The rest of the card still does what selecting a PC always did.
#[test]
fn selecting_a_pcs_card_opens_it_and_a_right_click_offers_its_menu() {
    let mut f = Frames::new(record("Den", true, true), Reach::Online);
    let card = pc_card_rect(&f);
    let commands = f.click_at(card.right_top() + vec2(-20.0, 20.0), egui::PointerButton::Primary);
    assert!(commands.iter().any(|c| matches!(c, Command::OpenApps(_))));
    let mut f = Frames::new(record("Den", true, true), Reach::Online);
    let card = pc_card_rect(&f);
    f.click_at(card.right_top() + vec2(-20.0, 20.0), egui::PointerButton::Secondary);
    assert!(f.options_open(), "its menu");
}

/// Tab meets each card's action, then its More. The card itself takes no focus.
#[test]
fn a_pcs_card_has_two_stops_for_focus() {
    let mut f = Frames::new(record("Den", true, true), Reach::Online);
    let (action, more) = f.card_and_dots();
    let card = pc_card_rect(&f);
    assert!(card.contains_rect(action) && card.contains_rect(more), "{action:?} {more:?} in {card:?}");
    assert!((action.left() - card.left() - 22.0).abs() < 0.5 && (card.right() - more.right() - 22.0).abs() < 0.5, "{action:?} {more:?} in {card:?}");
    assert_eq!((more.width(), more.height()), (48.0, 48.0));
    f.press(Key::Tab);
    let focused = f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).unwrap();
    assert_eq!(focused.rect, more, "More comes after the action");
}

/// The halo of a focused action paints over the card beside it, so no neighbour cuts it.
#[test]
fn a_focused_actions_halo_paints_after_the_next_card() {
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    let mut l = launcher(false, false);
    l.go(Screen::Hosts);
    let mut second = record("Attic", true, true);
    second.address = "192.0.2.77".into();
    second.unique_id = "attic".into();
    let hosts = Hosts::without_discovery(vec![record("Den", true, true), second], Arc::new(|| {}));
    let mut f = Frames::over(ctx, l, hosts);
    f.settle();
    let ring = f.shapes.iter().position(|s| !meshes_holding(std::slice::from_ref(s), design_tokens::FOCUS_RING_COLOR).is_empty()).expect("the ring");
    let neighbour = f.shapes.iter().position(|s| {
        let mut shown = Vec::new();
        collect_visible(&s.shape, &mut shown);
        shown.iter().any(|(t, _)| t == "Attic")
    });
    assert!(neighbour.is_some_and(|at| at < ring), "ring at {ring}, the next card at {neighbour:?}");
}

/// Ctrl+K still opens the search from PCs, where the field gave way to Add PC.
#[test]
fn ctrl_k_searches_from_pcs_and_add_pc_opens_its_dialog() {
    let mut f = Frames::new(record("Den", true, true), Reach::Online);
    let add = f.placed.iter().find(|(t, ..)| t == "Add PC").map(|p| p.1).expect("Add PC");
    assert!(add.top() < 68.0, "in the bar: {add:?}");
    f.click_at(add.center(), egui::PointerButton::Primary);
    assert!(matches!(f.l.dialog, Dialog::AddHost { .. }));
    f.press(Key::Escape);
    f.frame(vec![egui::Event::Key { key: Key::K, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND }]);
    f.settle();
    assert_eq!(f.l.screen, Screen::AllGames);
    assert!(f.l.search_open);
}

/// A PC's page with `count` apps, some running.
fn app_grid(count: i32, running: i32) -> Frames {
    let games: Vec<_> = (1..=count).map(|id| nightjar_core::http::App { name: format!("App {id}"), ..game(id) }).collect();
    let mut f = Frames::with(record("Den", true, true), Reach::Online, Some(answer(true, running)), games);
    let id = f.hosts.list[0].id;
    f.size = vec2(1280.0, 800.0);
    f.l.go(Screen::Apps(id));
    f.settle();
    f
}

/// Nothing of Back is clipped to its own square: that once showed as a dark box around the disc.
#[test]
fn backs_shadow_and_halo_leave_its_square() {
    let mut f = store_mix();
    f.l.go(Screen::AllGames);
    f.settle();
    let back = f.ctx.read_response(Id::unique("back")).expect("Back").rect;
    assert!(f.shapes.iter().all(|s| s.clip_rect != back), "clipped to {back:?}");
}

#[test]
fn an_app_grid_has_seven_fixed_columns_and_no_labels_on_art() {
    let mut f = app_grid(15, 0);
    let id = f.hosts.list[0].id;
    for app in 2..=15 {
        let art = demo_art(&f.ctx, app as usize);
        f.hosts.get_mut(id).unwrap().art.insert(app, art);
    }
    f.hosts.get_mut(id).unwrap().art.insert(1, Art::Missing);
    f.settle();
    let (_, width) = crate::ui::cards::grid(1184.0);
    assert_eq!(width, 155.0, "seven columns at the design's width");
    // Art says its own title; no line is drawn under it, but a tile still holds the name.
    assert!(!shows(&f, "App 2"), "{:?}", f.drawn);
    assert!(f.drawn.iter().any(|t| t.contains("App 1")), "a tile holds it: {:?}", f.drawn);
    // A screen reader still hears them all: the slot is a labelled button.
    let first = f.ctx.memory(|m| m.focused()).and_then(|i| f.ctx.read_response(i)).expect("the first cover has focus").rect;
    assert_eq!(first.size(), vec2(155.0, 232.0), "the slot is the cover");
    assert_eq!(first.left(), 48.0, "at the margin");
    f.press(Key::ArrowRight);
    let second = f.ctx.memory(|m| m.focused()).and_then(|i| f.ctx.read_response(i)).unwrap().rect;
    assert_eq!(second.left() - first.right(), 16.0, "the columns' gap");
    assert_eq!(second.top(), first.top());
    for _ in 0..5 {
        f.press(Key::ArrowRight);
    }
    let last = f.ctx.memory(|m| m.focused()).and_then(|i| f.ctx.read_response(i)).unwrap().rect;
    assert_eq!((last.top(), last.left()), (first.top(), first.left() + 6.0 * (155.0 + 16.0)), "seven to a row");
    f.press(Key::ArrowDown);
    for _ in 0..6 {
        f.press(Key::ArrowLeft);
    }
    let below = f.ctx.memory(|m| m.focused()).and_then(|i| f.ctx.read_response(i)).unwrap().rect;
    assert_eq!(below.left(), first.left(), "the next row starts under it");
    assert_eq!(below.top() - first.bottom(), 18.0, "the rows' gap");
}

#[test]
fn a_cover_without_its_art_yet_holds_its_name() {
    let f = app_grid(3, 0);
    assert!(!f.hosts.list[0].art.contains_key(&2));
    assert!(shows(&f, "App 2"), "the placeholder names it: {:?}", f.drawn);
}

#[test]
fn a_running_app_offers_resume_and_quit_game_beside_its_name() {
    let mut f = app_grid(5, 3);
    let name = "App 3";
    assert_eq!(f.drawn.iter().filter(|t| **t == name).count(), 2, "its chip and its cover: {:?}", f.drawn);
    assert!(shows(&f, "Resume") && shows(&f, "Quit game"), "{:?}", f.drawn);
    for gone in ["Refresh", "Quit running app", "PC settings"] {
        assert!(!shows(&f, gone), "{gone} lives in the PC's menu now: {:?}", f.drawn);
    }
    let at = |f: &Frames, word: &str| f.placed.iter().find(|(t, ..)| t == word).map(|p| p.1).unwrap();
    let (resume, quit) = (at(&f, "Resume"), at(&f, "Quit game"));
    assert!(resume.right() < quit.left(), "{resume:?} {quit:?}");
    assert!(1280.0 - quit.right() < 80.0, "at the right edge: {quit:?}");
    // The row does not grow for them: what runs does not move the grid.
    let top = f.ctx.memory(|m| m.focused()).and_then(|i| f.ctx.read_response(i)).expect("the running cover has focus").rect.top();
    let idle = app_grid(5, 0);
    let idle_top = idle.ctx.memory(|m| m.focused()).and_then(|i| idle.ctx.read_response(i)).unwrap().rect.top();
    assert_eq!(top, idle_top);
    // Resume starts it again; Quit game asks first.
    let commands = f.click_at(resume.center(), egui::PointerButton::Primary);
    assert!(commands.iter().any(|c| matches!(c, Command::Launch { app: 3, resume: true, .. })));
    let quit = at(&f, "Quit game");
    f.click_at(quit.center(), egui::PointerButton::Primary);
    assert!(matches!(f.l.dialog, Dialog::ConfirmQuit { start: None, .. }));
}

#[test]
fn a_narrow_window_lets_the_running_name_go_before_the_title() {
    let mut f = app_grid(5, 3);
    f.size = vec2(560.0, 700.0);
    f.settle();
    f.settle();
    let name = "App 3";
    assert!(shows(&f, "Den"), "the title stays: {:?}", f.drawn);
    assert!(shows(&f, "Resume") && shows(&f, "Quit game"), "{:?}", f.drawn);
    let chips = visible(&f).iter().filter(|(t, r)| *t == name && r.top() < 150.0).count();
    assert_eq!(chips, 0, "no chip in the row: {:?}", visible(&f));
}

#[test]
fn a_running_app_the_list_does_not_know_offers_only_quit_game() {
    let mut f = app_grid(5, 99);
    f.settle();
    assert!(!shows(&f, "Resume") && shows(&f, "Quit game") && shows(&f, "The running app"), "{:?}", f.drawn);
}

#[test]
fn the_pc_capsule_speaks_of_the_pc_whose_page_it_is() {
    let mut other = record("Attic", true, true);
    other.address = "192.0.2.77".into();
    other.unique_id = "attic".into();
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    let mut l = launcher(false, false);
    l.go(Screen::Hosts);
    let mut hosts = Hosts::without_discovery(vec![record("Den", true, true), other], Arc::new(|| {}));
    for host in &mut hosts.list {
        host.reach = Reach::Online;
        host.apps = Apps::Loaded(vec![game(1)]);
    }
    let attic = hosts.list[1].id;
    let mut f = Frames::over(ctx, l, hosts);
    f.l.go(Screen::Apps(attic));
    f.settle();
    let in_bar = |f: &Frames, word: &str| f.placed.iter().any(|(t, r, _)| t == word && r.top() < 68.0);
    assert!(in_bar(&f, "Attic") && !in_bar(&f, "Den"), "{:?}", f.placed);
}

/// A PC with no apps has nothing to start: no Desktop tile that could only fail, one line and Refresh.
#[test]
fn a_pc_with_no_apps_shows_one_line_and_refresh() {
    let mut f = Frames::with(record("Den", true, true), Reach::Online, Some(answer(true, 0)), Vec::new());
    f.hosts.list[0].apps = Apps::Loaded(Vec::new());
    let id = f.hosts.list[0].id;
    f.l.go(Screen::Apps(id));
    f.settle();
    assert!(shows(&f, "This PC lists no apps.") && shows(&f, "Refresh"), "{:?}", f.drawn);
    assert!(!shows(&f, "Desktop"), "no tile to fail: {:?}", f.drawn);
    let focused = f.ctx.memory(|m| m.focused()).and_then(|i| f.ctx.read_response(i)).expect("Refresh has focus").rect;
    let at = |f: &Frames, text: &str| f.placed.iter().find(|(t, ..)| t == text).unwrap().1;
    assert!(focused.contains(at(&f, "Refresh").center()), "{focused:?}");
    assert!(at(&f, "This PC lists no apps.").bottom() < focused.top());
    let commands = f.press(Key::Enter);
    assert!(commands.iter().any(|c| matches!(c, Command::Refresh(host) if *host == id)));
    assert!(!commands.iter().any(|c| matches!(c, Command::Launch { .. })));
    // While the answer is on its way the page holds still: no covers' places for a list that had none.
    let line = at(&f, "This PC lists no apps.");
    f.hosts.list[0].apps = Apps::Loading;
    f.settle();
    assert_eq!(at(&f, "This PC lists no apps."), line);
    assert!(!shows(&f, "Loading the library\u{2026}"), "{:?}", f.drawn);
    // A list with a game in it lands as any.
    f.hosts.list[0].apps = Apps::Loaded(vec![game(7)]);
    f.settle();
    assert!(!shows(&f, "This PC lists no apps.") && !shows(&f, "Refresh"), "{:?}", f.drawn);
    // Failed, it says so out loud with the way to try again.
    f.hosts.list[0].apps = Apps::Failed("The PC did not answer".into());
    f.settle();
    assert!(shows(&f, "The library could not be loaded.") && shows(&f, "The PC did not answer") && shows(&f, "Try again"), "{:?}", f.drawn);
}

/// A PC that lists a Desktop entry has an ordinary tile for it, which starts it.
#[test]
fn a_desktop_entry_the_pc_lists_is_a_normal_tile() {
    let desktop = nightjar_core::http::App { name: "Desktop".into(), ..game(3) };
    let mut f = Frames::with(record("Den", true, true), Reach::Online, Some(answer(true, 0)), vec![desktop]);
    let id = f.hosts.list[0].id;
    f.l.go(Screen::Apps(id));
    f.settle();
    assert!(!shows(&f, "Refresh") && !shows(&f, "This PC lists no apps."), "{:?}", f.drawn);
    let commands = f.press(Key::Enter);
    assert!(commands.iter().any(|c| matches!(c, Command::Launch { host, app: 3, resume: false } if *host == id)));
}

/// A cover's ⋮ takes the bottom right, clear of a logotype at the top or the bottom left, and the bottom
/// left on a running cover, whose dot has the other corner.
#[test]
fn a_covers_dots_sit_clear_of_its_logotype_and_its_running_dot() {
    let cover = Rect::from_min_size(pos2(100.0, 100.0), vec2(160.0, 240.0));
    for badge in [Badge::None, Badge::NotInstalled] {
        let dots = crate::ui::cards::dots_rect(cover, badge);
        assert!(cover.contains_rect(dots), "{dots:?}");
        assert!(dots.top() > cover.center().y && dots.left() > cover.center().x, "{dots:?}");
        assert!(cover.right() - dots.right() < 12.0, "in the corner: {dots:?}");
    }
    let running = crate::ui::cards::dots_rect(cover, Badge::Running);
    let status = cover.right_bottom() - Vec2::splat(18.0);
    assert!(cover.contains_rect(running) && running.right() < status.x - 10.0, "away from the running dot: {running:?}");
    assert!(running.left() - cover.left() < 12.0, "{running:?}");
}

/// Pointing at a cover shows its ⋮ in that corner, and pressing it opens the menu, on Home's shelves too.
#[test]
fn a_covers_dots_open_its_menu_on_homes_shelves() {
    let mut f = home_with_games(9);
    let host = f.hosts.list[0].record.unique_id.clone();
    f.l.favourites = vec![crate::config::Favourite { host, install_id: "install-3".into(), app_id: 1003, title: "Game 03".into() }];
    f.settle();
    // The arrows reach the favourite's cover, first of the page's "Game 03"s; its rect holds its caption.
    let mut cover = None;
    for _ in 0..8 {
        f.press(Key::ArrowDown);
        f.settle();
        let caption = f.placed.iter().find(|(t, ..)| t == "Game 03").map(|p| p.1).unwrap_or_else(|| panic!("{:?}", f.drawn));
        let rect = f.ctx.memory(|m| m.focused()).and_then(|i| f.ctx.read_response(i)).map(|r| r.rect);
        cover = rect.filter(|r| r.contains(caption.center()));
        if cover.is_some() {
            break;
        }
    }
    let cover = cover.expect("the arrows reach the favourite");
    let foot = cover.top() + cover_height(cover.width());
    let at = pos2(cover.right() - 8.0 - 15.0, foot - 8.0 - 15.0);
    f.frame(vec![egui::Event::PointerMoved(at)]);
    f.settle();
    f.click_at(at, egui::PointerButton::Primary);
    assert!(matches!(f.l.dialog, Dialog::GameMenu { .. }), "the menu, not the game's page: {:?}, cover {cover:?}", f.l.screen);
}

#[test]
fn the_list_lands_where_its_places_were_held() {
    let mut f = app_grid(9, 0);
    let id = f.hosts.list[0].id;
    let landed = f.ctx.memory(|m| m.focused()).and_then(|i| f.ctx.read_response(i)).unwrap().rect;
    f.hosts.get_mut(id).unwrap().apps = Apps::Loading;
    f.settle();
    assert!(shows(&f, "Loading the library\u{2026}"), "{:?}", f.drawn);
    let held = meshes_holding(&f.shapes, design_tokens::COVER_PLACEHOLDER);
    assert!(held.iter().any(|r| (r.center() - landed.center()).length() < 0.5 && (r.width() - landed.width()).abs() <= 1.0), "{landed:?} among {held:?}");
    assert!(held.len() >= 14, "two rows of seven: {held:?}");
}

/// The chips stay while the words narrow them, each counting what it would show.
#[test]
fn chip_counts_follow_the_search_and_the_chips_stay() {
    let mut f = store_mix();
    f.l.go(Screen::AllGames);
    f.settle();
    let before: Vec<String> = f.drawn.iter().filter(|t| ["All", "Steam", "Epic Games"].contains(&t.as_str())).cloned().collect();
    let total = f.hosts.list[0].native.library.as_ref().unwrap().games.len();
    assert!(shows(&f, &total.to_string()), "{:?}", f.drawn);
    f.l.search_open = true;
    f.l.library_query = "fort".into();
    f.settle();
    let after: Vec<String> = f.drawn.iter().filter(|t| ["All", "Steam", "Epic Games"].contains(&t.as_str())).cloned().collect();
    assert_eq!(before, after, "the chips stay");
    assert!(!shows(&f, &total.to_string()) && f.drawn.iter().any(|t| t == "1"), "only Fortnite counts: {:?}", f.drawn);
}

#[test]
fn the_open_searchs_words_sit_on_the_closed_ones_line() {
    let mut f = store_mix();
    f.settle();
    let closed = f.placed.iter().find(|(t, r, _)| t == "Search" && r.top() < 68.0).expect("the closed field").1;
    f.l.open_search();
    f.settle();
    let open = f.placed.iter().find(|(t, r, _)| t == "Search" && r.top() < 68.0).expect("the field's hint").1;
    assert!((open.center().y - closed.center().y).abs() <= 0.5, "open {open:?}, closed {closed:?}");
    f.l.library_query = "Search".into();
    f.settle();
    let typed = f.placed.iter().find(|(t, r, _)| t == "Search" && r.top() < 68.0).expect("the typed words").1;
    assert!((typed.center().y - closed.center().y).abs() <= 0.5, "typed {typed:?}, closed {closed:?}");
}

#[test]
#[ignore = "writes PNGs; set NIGHTJAR_SHOTS"]
fn page_screenshots() {
    let Some(dir) = std::env::var_os("NIGHTJAR_SHOTS").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&dir).expect("the folder for the screenshots");
    let prefix = std::env::var("NIGHTJAR_SHOTS_PREFIX").unwrap_or_default();
    let named = |name: &str, id| nightjar_core::http::App { name: name.into(), ..game(id) };
    let games = vec![named("Hollow Knight", 7), named("Forza Horizon 5", 9), named("Desktop", 11), named("Steam Big Picture", 13)];
    for high in [false, true] {
        let look = if high { "high-contrast" } else { "normal" };
        let fresh = || {
            crate::theme::own_palette();
            let ctx = egui::Context::default();
            crate::theme::apply(&ctx);
            let mut l = launcher(false, false);
            l.settings.high_contrast = high;
            l.go(Screen::Hosts);
            let records = vec![
                HostRecord { address: "192.0.2.10".into(), ..record("Living Room PC", true, true) },
                HostRecord { address: "192.0.2.11".into(), ..record("Office PC", true, true) },
                HostRecord { address: "192.0.2.12".into(), ..record("Studio", false, false) },
                HostRecord { address: "192.0.2.13".into(), ..record("Laptop", true, false) },
            ];
            let mut hosts = Hosts::without_discovery(records, Arc::new(|| {}));
            hosts.list[0].reach = Reach::Online;
            hosts.list[0].info = Some(answer(true, 7));
            hosts.list[0].apps = Apps::Loaded(games.clone());
            hosts.list[1].reach = Reach::Offline;
            hosts.list[2].reach = Reach::Online;
            hosts.list[2].info = Some(answer(false, 0));
            hosts.list[3].reach = Reach::Unknown;
            let mut f = Frames::over(ctx, l, hosts);
            f.size = vec2(1280.0, 800.0);
            f.native_ppp = 1.5;
            f.settle();
            f
        };
        let shoot = |f: &mut Frames, page: &str| {
            f.settle();
            f.shoot(&dir, &format!("{prefix}{page}-{look}-desktop"));
        };
        let mut f = fresh();
        shoot(&mut f, "hosts");
        let id = f.hosts.list[0].id;
        f.l.go(Screen::Apps(id));
        shoot(&mut f, "library");
        f.l.go(Screen::Settings);
        shoot(&mut f, "settings");
        let mut f = fresh();
        let last = host_options(&f.hosts.list[0]).len() - 1;
        f.press(Key::F2);
        shoot(&mut f, "host-options");
        for _ in 0..last {
            f.press(Key::ArrowDown);
        }
        f.press(Key::Enter);
        shoot(&mut f, "confirm-remove");
        // The smallest window SDL allows: the list scrolls and fades its edge.
        let mut f = fresh();
        f.size = vec2(720.0, 480.0);
        f.settle();
        f.press(Key::F2);
        shoot(&mut f, "host-options-short-window");
    }
    // A long address and GPU end in "…" and the tooltip shows them whole.
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    let mut l = launcher(false, false);
    l.go(Screen::Hosts);
    let address = "fd12:3456:789a:bcde:f012:3456:789a:bcde";
    let mut hosts = Hosts::without_discovery(vec![HostRecord { address: address.into(), ..record("Living Room PC", true, true) }], Arc::new(|| {}));
    let mut info = answer(true, 0);
    info.gpu_type = Some("AMD Radeon RX 7900 XTX Phantom Gaming OC".into());
    hosts.list[0].reach = Reach::Online;
    hosts.list[0].info = Some(info);
    let mut f = Frames::over(ctx, l, hosts);
    f.size = vec2(720.0, 480.0);
    f.native_ppp = 1.5;
    f.settle();
    let card = f.ctx.read_response(f.ctx.memory(|m| m.focused()).expect("the card has focus")).expect("the card").rect;
    hover_card(&mut f, card);
    f.shoot(&dir, &format!("{prefix}hosts-long-detail-desktop"));
}

/// The demo library's cover art (the same covers the Android demo and README use).
fn demo_art(ctx: &egui::Context, n: usize) -> Art {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../app/src/debug/assets/demo/art/{}.jpg", n + 1));
    let image = image::load_from_memory(&std::fs::read(&path).expect("the demo cover")).expect("a JPEG").to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Art::Ready(ctx.load_texture(format!("demo-art-{n}"), egui::ColorImage::from_rgba_unmultiplied(size, &image), egui::TextureOptions::LINEAR))
}

/// The glass Home on each tier and every other page once, beside the prototype:
/// `NIGHTJAR_SHOTS=<dir> cargo test -p nightjar-desktop --bin nightjar glass_page_screenshots --release -- --ignored`.
#[test]
#[ignore = "writes PNGs; set NIGHTJAR_SHOTS"]
fn glass_page_screenshots() {
    use crate::live_glass::Tier;
    use nightjar_core::http::native_api::library::{Artwork, Game, Library, Store};
    let Some(dir) = std::env::var_os("NIGHTJAR_SHOTS").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&dir).expect("the folder for the screenshots");
    let library = |tier: Tier, ppp: f32, hero: usize, art_less: &[usize]| {
        crate::theme::own_palette();
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        crate::live_glass::set_tier(&ctx, tier);
        let mut l = launcher(true, false);
        l.identity_ready = true;
        let records = vec![HostRecord { address: "192.0.2.10".into(), ..record("Living Room PC", true, true) }];
        let hosts = Hosts::without_discovery(records, Arc::new(|| {}));
        let mut f = Frames::over(ctx, l, hosts);
        f.size = vec2(1280.0, 800.0);
        f.native_ppp = ppp;
        f.shots = true;
        let games: Vec<Game> = POLISH_GAMES
            .iter()
            .enumerate()
            .map(|(n, (title, store))| Game {
                install_id: format!("install-{n}"),
                app_id: 1000 + n as u32,
                title: (*title).into(),
                game_id: (!store.is_empty()).then(|| format!("{store}:{n}")),
                store: (!store.is_empty()).then(|| Store { source: (*store).into(), id: n.to_string(), name: String::new() }),
                installed: Some(true),
                running: n == hero,
                artwork: Artwork { cover: (!art_less.contains(&n)).then(|| format!("/cover/{n}")), version: "1".into() },
            })
            .collect();
        let apps: Vec<nightjar_core::http::App> = games
            .iter()
            .map(|g| nightjar_core::http::App { name: g.title.clone(), uuid: g.install_id.clone(), ..game(g.app_id as i32) })
            .collect();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("the clock").as_secs();
        let pc = &mut f.hosts.list[0];
        pc.reach = Reach::Online;
        pc.info = Some(files::tests::native_answer());
        pc.native.library = Some(Library { revision: "r1".into(), games });
        pc.apps = Apps::Loaded(apps.clone());
        for (n, app) in apps.iter().enumerate() {
            if art_less.contains(&n) {
                pc.art.insert(app.id, Art::Missing);
                continue;
            }
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../app/src/debug/assets/demo/art/{}.jpg", n + 1));
            let image = image::load_from_memory(&std::fs::read(&path).expect("the demo cover")).expect("a JPEG").to_rgba8();
            let image = egui::ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], &image);
            pc.ambient.insert(app.id, f.ctx.load_texture(format!("ambient-{n}"), crate::ambient::bake(&image), egui::TextureOptions::LINEAR));
            pc.art.insert(app.id, Art::Ready(f.ctx.load_texture(format!("cover-{n}"), image, egui::TextureOptions::LINEAR)));
        }
        let uuid = f.hosts.list[0].record.unique_id.clone();
        for (n, days) in [(2u32, 1u64), (8, 3), (3, 8)] {
            f.l.recents.push(crate::config::Recent { host: uuid.clone(), install_id: format!("install-{n}"), app_id: 1000 + n, title: POLISH_GAMES[n as usize].0.into(), at: now - days * 86400 });
        }
        f.l.go(Screen::Home);
        f
    };
    let at_rest = |f: &mut Frames| {
        for _ in 0..16 {
            f.settle();
        }
    };
    for (name, tier, ppp) in [("home-live", Tier::Live, 1.0), ("home-baked", Tier::Baked, 1.0), ("home-solid", Tier::Solid, 1.0), ("home-150", Tier::Live, 1.5)] {
        let mut f = library(tier, ppp, 1, &[]);
        at_rest(&mut f);
        f.shoot(&dir, name);
    }
    // A running game with no art, one on a shelf too: each game's own tile.
    let mut f = library(Tier::Live, 1.0, 18, &[18, 11, 13]);
    at_rest(&mut f);
    f.shoot(&dir, "home-noart");
    // Down to the shelves: the focused cover lifts.
    let mut f = library(Tier::Live, 1.0, 1, &[]);
    at_rest(&mut f);
    for _ in 0..3 {
        f.press(Key::ArrowDown);
    }
    at_rest(&mut f);
    f.shoot(&dir, "home-focus-cover");
    let pc = f.hosts.list[0].id;
    for (name, screen) in [
        ("pcs", Screen::Hosts),
        ("settings", Screen::Settings),
        ("help", Screen::Help),
        ("all-games", Screen::AllGames),
        ("game", Screen::Game { host: pc, app: 1002 }),
        ("apps", Screen::Apps(pc)),
    ] {
        f.l.go(screen);
        at_rest(&mut f);
        f.shoot(&dir, name);
    }
    f.l.go(Screen::Home);
    f.l.open_search();
    f.l.library_query = "ocean".into();
    at_rest(&mut f);
    f.shoot(&dir, "search");
}

/// All 36 games of the Android demo's library have a cover of their own that this app's image
/// decoder reads. Covers carry their titles, so one repeated would show the wrong name.
#[test]
fn demo_covers_decode_and_none_repeats() {
    let art = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/src/debug/assets/demo/art");
    let mut seen = std::collections::HashSet::new();
    for n in 1..=36 {
        let bytes = std::fs::read(art.join(format!("{n}.jpg"))).expect("the demo cover");
        image::load_from_memory(&bytes).expect("a JPEG");
        assert!(seen.insert(bytes), "cover {n} repeats another");
    }
}

/// The showcase library. Store keys as a host sends them (console.rs names them). Ten Steam games
/// overflow a row at both sizes, so Steam's ends in its All tile; one Epic game is not installed.
const POLISH_GAMES: [(&str, &str); 19] = [
    ("Desktop", ""), ("Racing Game", "steam"), ("Open World", "steam"), ("Space Sim", "microsoft"), ("Puzzle Night", "gog"),
    ("Retro Arcade", "steam"), ("Strategy", "battlenet"), ("Music Studio", "steam"), ("Ocean Explorer", "steam"), ("Skybound", "steam"),
    ("Ashfall", "microsoft"), ("Brightwater", "steam"), ("Cinder Road", "steam"), ("Deep Field", "steam"), ("Ember Keep", "steam"),
    ("Frostline", "epic"), ("Glass Harbor", "battlenet"), ("Hollow Peak", "epic"), ("Iron Tide", ""),
];

/// A showcase game shows art/N.jpg of its own place in the demo library, so no game wears another's
/// cover and none stays empty: its title must be the one the demo keeps at that place.
#[test]
fn showcase_games_sit_at_their_own_covers_place() {
    let java = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/src/debug/java/com/limelight/demo/DemoConsole.java")).expect("the demo library");
    let library = java.split("LIBRARY = {").nth(1).expect("the list").split("};").next().expect("its end");
    let demo: Vec<&str> = library.lines().filter_map(|line| line.trim().strip_prefix("{\"")?.split('"').next()).collect();
    for (n, (title, _)) in POLISH_GAMES.iter().enumerate() {
        assert_eq!(Some(title), demo.get(n), "place {n}");
    }
}

/// Every main page and state as a first-time and a returning user meet them, at 1280x800 (150 %)
/// and 1920x1080: `NIGHTJAR_SHOTS=<dir> cargo test -p nightjar-desktop polish_screenshots -- --ignored`.
#[test]
#[ignore = "writes PNGs; set NIGHTJAR_SHOTS"]
fn polish_screenshots() {
    use nightjar_core::http::native_api::library::{Artwork, Game, Library, Store};
    let Some(dir) = std::env::var_os("NIGHTJAR_SHOTS").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&dir).expect("the folder for the screenshots");
    let only = std::env::var("NIGHTJAR_SHOTS_ONLY").unwrap_or_default();
    let titles = POLISH_GAMES;
    for (width, height, ppp) in [(1280.0, 800.0, 1.5), (1920.0, 1080.0, 1.0)] {
        let size = format!("{}x{}", width as u32, height as u32);
        let fresh = |records: Vec<HostRecord>, home: bool| {
            crate::theme::own_palette();
            let ctx = egui::Context::default();
            crate::theme::apply(&ctx);
            let mut l = launcher(home, false);
            l.identity_ready = true;
            let hosts = Hosts::without_discovery(records, Arc::new(|| {}));
            let mut f = Frames::over(ctx, l, hosts);
            f.size = vec2(width, height);
            f.native_ppp = ppp;
            f.shots = true;
            f
        };
        let shoot = |f: &mut Frames, page: &str| {
            if !only.is_empty() && !only.split(',').any(|o| page.starts_with(o)) {
                return;
            }
            f.settle();
            f.settle();
            f.shoot(&dir, &format!("{page}-{size}"));
        };
        // First run: nothing found yet.
        let mut f = fresh(Vec::new(), true);
        f.l.go(Screen::Home);
        shoot(&mut f, "first-run-home");
        f.l.go(Screen::Hosts);
        shoot(&mut f, "first-run-pcs");
        f.l.add_host_dialog();
        shoot(&mut f, "dialog-add-pc");

        // A returning user: a PC with a library, one offline, one to pair.
        let records = vec![
            HostRecord { address: "192.0.2.10".into(), ..record("Living Room PC", true, true) },
            HostRecord { address: "192.0.2.11".into(), ..record("Office PC", true, true) },
            HostRecord { address: "192.0.2.12".into(), ..record("Studio", false, false) },
        ];
        let mut f = fresh(records, true);
        let games: Vec<Game> = titles
            .iter()
            .enumerate()
            .map(|(n, (title, store))| Game {
                install_id: format!("install-{n}"),
                app_id: 1000 + n as u32,
                title: (*title).into(),
                game_id: (!store.is_empty()).then(|| format!("{store}:{n}")),
                store: (!store.is_empty()).then(|| Store { source: (*store).into(), id: n.to_string(), name: String::new() }),
                installed: Some(*title != "Frostline"),
                running: n == 1,
                artwork: Artwork { cover: Some(format!("/cover/{n}")), version: "1".into() },
            })
            .collect();
        let apps: Vec<nightjar_core::http::App> = games
            .iter()
            .map(|g| nightjar_core::http::App { name: g.title.clone(), uuid: g.install_id.clone(), ..game(g.app_id as i32) })
            .collect();
        {
            let pc = &mut f.hosts.list[0];
            pc.reach = Reach::Online;
            pc.info = Some(files::tests::native_answer());
            pc.native.library = Some(Library { revision: "r1".into(), games });
            pc.apps = Apps::Loaded(apps.clone());
            for (n, app) in apps.iter().enumerate() {
                pc.art.insert(app.id, demo_art(&f.ctx, n));
            }
        }
        f.hosts.list[1].reach = Reach::Offline;
        f.hosts.list[2].reach = Reach::Online;
        f.hosts.list[2].info = Some(answer(false, 0));
        let pc = f.hosts.list[0].id;
        f.l.go(Screen::Home);
        shoot(&mut f, "home");
        f.press(Key::ArrowRight);
        shoot(&mut f, "home-focus-moved");
        f.scroll_to_end();
        shoot(&mut f, "home-end");
        // Home's View: its menu, then each layout, grouping and card size.
        f.l.go(Screen::Home);
        f.settle();
        if let Some(view) = f.placed.iter().find(|(t, ..)| t == "View").map(|p| p.1) {
            f.click_at(view.center(), egui::PointerButton::Primary);
            shoot(&mut f, "home-view-menu");
            f.press(Key::Escape);
        }
        for (layout, group, card) in [
            (HomeLayout::Grid, HomeGroup::Store, CardSize::Medium),
            (HomeLayout::Grid, HomeGroup::None, CardSize::Small),
            (HomeLayout::Rows, HomeGroup::None, CardSize::Large),
            (HomeLayout::Rows, HomeGroup::Store, CardSize::Small),
        ] {
            (f.l.settings.home_layout, f.l.settings.home_group, f.l.settings.home_card_size) = (layout, group, card);
            let name = format!("home-view-{}-{}-{}", layout.label(), group.label(), card.label()).to_lowercase();
            shoot(&mut f, &name);
        }
        (f.l.settings.home_layout, f.l.settings.home_group, f.l.settings.home_card_size) = (HomeLayout::Rows, HomeGroup::Store, CardSize::Medium);
        // Help as a returning user meets it, Home in the top bar (the README's shot).
        f.l.go(Screen::Help);
        shoot(&mut f, "help-returning");
        // All games with every game, then with a store's chip clicked.
        f.l.open(Screen::AllGames);
        shoot(&mut f, "all-games");
        f.settle();
        let steam = f.placed.iter().find(|(t, r, _)| t == "Steam" && r.center().y < 200.0).map(|p| p.1).expect("the Steam chip");
        f.click_at(steam.center(), egui::PointerButton::Primary);
        shoot(&mut f, "all-games-store");
        f.l.library_filter = console::LibraryFilter::All;
        f.frame(vec![egui::Event::PointerGone]);
        f.settle();
        // A cover's menu from its ⋮ (the first cover, at the left edge) and from a right-click on
        // the last cover of the first line (at the right edge).
        f.l.focused_key = None;
        f.l.restore_key = None;
        f.l.focus_screen = true;
        f.settle();
        let cover = f.ctx.memory(|m| m.focused()).expect("the first cover has focus");
        let first = f.ctx.read_response(cover).expect("the cover").rect;
        f.frame(vec![egui::Event::PointerMoved(first.center())]);
        f.settle();
        let dots = f.ctx.read_response(cover.with("options")).expect("the cover's ⋮").rect;
        f.click_at(dots.center(), egui::PointerButton::Primary);
        shoot(&mut f, "game-menu-dots");
        f.l.close_dialog();
        // Every cover of the line draws its title inside its own width: the rightmost title is the last cover's.
        let right = f
            .placed
            .iter()
            .filter(|(_, r, _)| r.center().y > first.top() && r.center().y < first.bottom())
            .map(|p| p.1.center().x)
            .max_by(f32::total_cmp)
            .map(|x| pos2(x, first.center().y))
            .expect("the first line's titles");
        f.click_at(right + vec2(first.width() * 0.25, 0.0), egui::PointerButton::Secondary);
        shoot(&mut f, "game-menu-right-click");
        f.l.close_dialog();
        // A cover under the pointer with the focus elsewhere: it rises, no ring.
        // (Closing the menu gave the focus back to the cover it was opened on.)
        f.settle();
        if let Some(held) = f.ctx.memory(|m| m.focused()) {
            f.ctx.memory_mut(|m| m.surrender_focus(held));
        }
        f.frame(vec![egui::Event::PointerMoved(right)]);
        f.settle();
        shoot(&mut f, "all-games-cover-hover");
        // The same in high contrast with the first cover focused: one amber ring.
        f.l.settings.high_contrast = true;
        f.frame(vec![egui::Event::PointerGone]);
        f.ctx.memory_mut(|m| m.request_focus(cover));
        shoot(&mut f, "all-games-focus-high-contrast");
        f.l.go(Screen::Hosts);
        shoot(&mut f, "pcs-focus-high-contrast");
        f.l.settings.high_contrast = false;
        // A PC's card under the pointer with no focus on it: one step lighter, no outline.
        let card = f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).expect("the first PC has focus");
        f.ctx.memory_mut(|m| m.surrender_focus(card.id));
        hover_card(&mut f, card.rect);
        shoot(&mut f, "pcs-card-hover");
        f.frame(vec![egui::Event::PointerGone]);
        f.l.go(Screen::Game { host: pc, app: 1001 });
        shoot(&mut f, "game-running");
        f.l.go(Screen::Game { host: pc, app: 1002 });
        shoot(&mut f, "game");
        // Frostline, which the PC has not installed: no Play.
        f.l.go(Screen::Game { host: pc, app: 1015 });
        shoot(&mut f, "game-not-installed");
        f.l.go(Screen::Hosts);
        shoot(&mut f, "pcs");
        f.l.go(Screen::Apps(pc));
        shoot(&mut f, "library");
        f.l.go(Screen::HostDetails(pc));
        shoot(&mut f, "pc-details");
        f.l.go(Screen::Settings);
        shoot(&mut f, "settings");
        f.scroll_to_end();
        shoot(&mut f, "settings-end");
        // A controller's mapping, as after choosing it: Connect as leads its rows.
        f.l.go(Screen::Controllers);
        f.l.controller_key = "demo-pad".into();
        f.l.controller_mapping = crate::gamepad::Mapping { name: "Xbox Wireless Controller".into(), ..Default::default() };
        shoot(&mut f, "controllers");
        f.l.go(Screen::Hosts);
        f.settle();
        f.press(Key::F2);
        shoot(&mut f, "dialog-pc-options");
        f.l.close_dialog();
        let studio = f.hosts.list[2].id;
        f.l.dialog = Dialog::Pairing { host: studio, pin: "4071".into() };
        f.l.focus_dialog = true;
        shoot(&mut f, "dialog-pairing");
        f.l.pair_failed(studio, crate::hosts::PAIR_PIN_WRONG, false);
        shoot(&mut f, "dialog-pair-failed");
        f.l.close_dialog();
        f.l.dialog = Dialog::ConfirmRemove { host: studio, name: "Studio".into() };
        f.l.focus_dialog = true;
        shoot(&mut f, "dialog-remove");
        f.l.close_dialog();
        f.l.dialog = Dialog::Rename { host: studio, text: String::new() };
        f.l.focus_dialog = true;
        shoot(&mut f, "dialog-rename");
        f.l.close_dialog();
        f.l.dialog = Dialog::ConfirmSleep { host: pc, ask: 1, name: "Living Room PC".into(), offer: None };
        f.l.focus_dialog = true;
        shoot(&mut f, "dialog-sleep-asking");
        f.l.sleep_answered(pc, 1, "Living Room PC", false, Ok(SleepStep::Offer { token: "t".into(), at: Handed::now() }));
        shoot(&mut f, "dialog-sleep-offered");
        f.l.close_dialog();
        f.waking = Some("Office PC".into());
        shoot(&mut f, "waking");
        f.waking = None;
        // Home's rows in one column, drawn alone as `at_one_column_a_long_row_is_its_all_tile`
        // does (the top bar keeps the window wider): a row too long to fit is its All tile.
        if only.is_empty() || only.split(',').any(|o| "home-one-column".starts_with(o)) {
            let column = vec2(200.0, height);
            let mut shapes = Vec::new();
            for _ in 0..6 {
                f.time += 0.05;
                let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, column)), time: Some(f.time), focused: true, ..Default::default() };
                input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(f.native_ppp);
                let mut commands = Vec::new();
                let output = f.ctx.run_ui(input, |ui| {
                    let view = View { hosts: &f.hosts, pads: &f.pads, connecting: None, waking: None };
                    egui::CentralPanel::default().show(ui, |ui| draw_home(&mut f.l, ui, &view, &mut commands));
                });
                f.textures.append(output.textures_delta);
                shapes = output.shapes;
            }
            let (window, page) = (f.size, std::mem::replace(&mut f.shapes, shapes));
            f.size = column;
            f.shoot(&dir, &format!("home-one-column-{size}"));
            (f.size, f.shapes) = (window, page);
        }
        // All games of a PC whose library is empty: no chips, and the page says why. A chip exists
        // only for games it shows, so no filter is ever empty on its own.
        let mut f = fresh(vec![HostRecord { address: "192.0.2.10".into(), ..record("Living Room PC", true, true) }], true);
        f.hosts.list[0].reach = Reach::Online;
        f.hosts.list[0].info = Some(files::tests::native_answer());
        f.hosts.list[0].native.library = Some(Library { revision: "r1".into(), games: Vec::new() });
        f.l.open(Screen::AllGames);
        shoot(&mut f, "all-games-empty");
        // Nothing installed: the Installed chip is offered and shows no game.
        let none = |title: &str, n: u32| Game {
            install_id: format!("install-{n}"),
            app_id: 1000 + n,
            title: title.into(),
            game_id: Some(format!("steam:{n}")),
            store: Some(Store { source: "steam".into(), id: n.to_string(), name: String::new() }),
            installed: Some(false),
            running: false,
            artwork: Artwork { cover: None, version: String::new() },
        };
        f.hosts.list[0].native.library = Some(Library { revision: "r2".into(), games: vec![none("Night Drive", 0), none("Farm Life", 1)] });
        f.l.library_filter = console::LibraryFilter::Installed;
        shoot(&mut f, "all-games-empty-filter");
        // The library still on its way: the spinner Home shows.
        f.hosts.list[0].native.library = None;
        shoot(&mut f, "all-games-loading");
    }
}

/// Issue 55: on a game's (or PC's, or profile's) settings an unchecked row's menu was disabled,
/// so a controller's focus could only land on its Override box, and A there pinned the value it
/// inherited. The menu itself takes focus now; opening it and backing out keeps the row
/// inherited, choosing another value checks Override, and the box still unchecks it.
#[test]
fn an_inherited_row_opens_from_a_controller_and_only_a_new_value_overrides_it() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    f.l.settings.fps = 120;
    let id = f.hosts.list[0].id;
    f.l.go(Screen::Apps(id));
    f.l.open_app_settings(id, &f.hosts.list[0], game(7));
    f.settle();
    assert!(focus_down_to(&mut f, "120 fps"), "Down reaches the inherited Frame rate menu");
    f.press(Key::Enter);
    assert!(shows(&f, "60 fps") && shows(&f, "240 fps"), "A opens it: {:?}", f.drawn);
    // B closes the menu only; the next B leaves the page.
    let commands = f.press(Key::Escape);
    assert!(!shows(&f, "240 fps"), "B closes the menu: {:?}", f.drawn);
    assert!(!commands.iter().any(|c| matches!(c, Command::SaveAppSettings(_))), "and stays on the page");
    assert!(f.l.edit_overrides.is_empty(), "backing out pins nothing: {:?}", f.l.edit_overrides);
    let commands = f.press(Key::Escape);
    assert!(commands.iter().any(|c| matches!(c, Command::SaveAppSettings(_))), "B with no menu open leaves the page");

    f.l.open_app_settings(id, &f.hosts.list[0], game(7));
    f.settle();
    assert!(f.l.edit_overrides.is_empty());
    let at = |f: &Frames, text: &str| f.placed.iter().find(|(t, ..)| t == text).map(|p| p.1).unwrap_or_else(|| panic!("{text} in {:?}", f.drawn));
    click_text(&mut f, "120 fps");
    click_text(&mut f, "60 fps");
    assert!(f.l.edit_overrides.has(&["fps"]), "a new value overrides the row");
    assert_eq!(f.l.edit_overrides.apply(&f.l.inherited_settings).fps, 60);
    // The Frame rate row's own box, the nearest "For this game" under its menu, gives it back.
    let menu = at(&f, "60 fps");
    let boxed = f.placed.iter().filter(|(t, r, _)| t == "For this game" && r.top() >= menu.bottom()).map(|p| p.1)
        .min_by(|a, b| (a.top() - menu.bottom()).total_cmp(&(b.top() - menu.bottom()))).expect("its box");
    f.click_at(boxed.center(), egui::PointerButton::Primary);
    assert!(f.l.edit_overrides.is_empty(), "{:?}", f.l.edit_overrides);
    assert_eq!(f.l.edit_settings.fps, 120);
    assert!(shows(&f, "120 fps"), "{:?}", f.drawn);
}

/// Presses Down until focus lands on the widget drawn with `text` (a menu's value or item, a
/// field's number). False when it never does.
fn focus_down_to(f: &mut Frames, text: &str) -> bool {
    for _ in 0..16 {
        f.press(Key::ArrowDown);
        let focused = f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect);
        let at = f.placed.iter().find(|(t, ..)| t == text).map(|p| p.1.center());
        if focused.zip(at).is_some_and(|(r, at)| r.contains(at)) {
            return true;
        }
    }
    false
}

/// An inherited game setting opened and chosen with the controller alone, its menu item picked
/// by A (keyboard Enter) or by a screen reader's action.
fn pick_inherited_frame_rate(by_reader: bool) {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    f.l.settings.fps = 120;
    let id = f.hosts.list[0].id;
    f.l.go(Screen::Apps(id));
    f.l.open_app_settings(id, &f.hosts.list[0], game(7));
    f.settle();
    assert!(focus_down_to(&mut f, "120 fps"), "Down reaches the inherited Frame rate menu");
    f.press(Key::Enter);
    assert!(focus_down_to(&mut f, "60 fps"), "Down walks into the open menu: {:?}", f.drawn);
    assert!(f.l.edit_overrides.is_empty(), "walking the menu chooses nothing");
    if by_reader {
        let item = f.ctx.memory(|m| m.focused()).unwrap();
        let click = egui::accesskit::ActionRequest {
            action: egui::accesskit::Action::Click,
            target_tree: egui::accesskit::TreeId::ROOT,
            target_node: item.accesskit_id(),
            data: None,
        };
        f.frame(vec![egui::Event::AccessKitActionRequest(click)]);
        f.settle();
    } else {
        f.press(Key::Enter);
    }
    assert!(f.l.edit_overrides.has(&["fps"]), "the chosen item overrides the row");
    assert_eq!(f.l.edit_overrides.apply(&f.l.inherited_settings).fps, 60);
    assert!(!shows(&f, "240 fps"), "choosing closes the menu: {:?}", f.drawn);
    let commands = f.press(Key::Escape);
    assert!(commands.iter().any(|c| matches!(c, Command::SaveAppSettings(_))), "B leaves and saves");
}

#[test]
fn an_inherited_menu_item_chosen_with_a_controller_overrides_the_row() {
    pick_inherited_frame_rate(false);
}

#[test]
fn an_inherited_menu_item_chosen_by_a_screen_reader_overrides_the_row() {
    pick_inherited_frame_rate(true);
}

/// Review of issue 55's fix: a number field steps on Left and Right and consumes them, so an
/// inherited Custom frame rate (a Deck's 40 or 45) stepped with the D-pad was drawn and dropped.
#[test]
fn stepping_an_inherited_number_field_with_a_controller_overrides_it() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    f.l.settings.fps = 45;
    let id = f.hosts.list[0].id;
    f.l.go(Screen::Apps(id));
    f.l.open_app_settings(id, &f.hosts.list[0], game(7));
    f.settle();
    assert!(focus_down_to(&mut f, "45"), "Down reaches the inherited Custom frame rate field: {:?}", f.drawn);
    f.press(Key::ArrowRight);
    assert!(f.l.edit_overrides.has(&["fps"]), "the step overrides the row: {:?}", f.l.edit_overrides);
    assert_eq!(f.l.edit_overrides.apply(&f.l.inherited_settings).fps, 46);
    let commands = f.press(Key::Escape);
    assert!(commands.iter().any(|c| matches!(c, Command::SaveAppSettings(_))), "B leaves and saves");
    assert_eq!(f.l.edit_overrides.apply(&f.l.inherited_settings).fps, 46, "what the save writes");
}

/// Issue 55: a frame rate a PC or profile overrides looked like the main settings' from a game,
/// whose "Reset all to inherited" cannot clear it. Unchecked rows and the game page name it.
#[test]
fn a_value_from_a_pc_or_profile_says_so_on_the_game_settings_and_page() {
    let mut f = home_with_games(1);
    f.size = vec2(1280.0, 4000.0);
    let pc = f.hosts.list[0].id;
    f.l.settings.fps = 60;
    let travel = f.l.profiles.create("Travel", serde_json::from_str(r#"{"bitrate_kbps":20000}"#).unwrap()).unwrap();
    f.l.profiles.active = Some(travel);
    f.hosts.list[0].record.settings = Some(serde_json::from_str(r#"{"fps":120}"#).unwrap());
    f.hosts.list[0].info = Some(answer(true, 1000));
    f.hosts.list[0].apps = Apps::Loaded(vec![nightjar_core::http::App { name: "Game 00".into(), uuid: "install-0".into(), ..game(1000) }]);
    f.l.go(Screen::Game { host: pc, app: 1000 });
    f.settle();
    let line = f.drawn.iter().find(|t| t.contains("Travel")).unwrap_or_else(|| panic!("{:?}", f.drawn)).clone();
    assert!(line.contains("Frame rate set for this PC") && line.contains("Bitrate from profile Travel") && !line.contains("Resolution from"), "{line}");

    f.l.open_app_settings(pc, &f.hosts.list[0], nightjar_core::http::App { name: "Game 00".into(), uuid: "install-0".into(), ..game(1000) });
    f.settle();
    assert!(shows(&f, "Set for this PC") && shows(&f, "From profile Travel"), "{:?}", f.drawn);
    // Clearing the game's own settings keeps showing where the inherited rate comes from.
    f.l.edit_overrides.capture(&Settings { hdr: true, ..Settings::default() }, &["hdr"]);
    f.settle();
    click_text(&mut f, "Clear all for this game");
    click_text(&mut f, "Clear all");
    assert!(f.l.edit_overrides.is_empty(), "{:?}", f.l.edit_overrides);
    assert!(shows(&f, "120 fps") && shows(&f, "Set for this PC"), "{:?}", f.drawn);
    // A PC's own page names the profile only.
    f.l.open_host_settings(pc, &f.hosts.list[0]);
    f.settle();
    assert!(!shows(&f, "Set for this PC") && shows(&f, "From profile Travel"), "{:?}", f.drawn);
    assert_eq!(summary_sources(None, None, None), Vec::<String>::new());
}

/// A paired PC whose library holds `games` (title, store key, installed), on Home.
fn home_with_stores(games: &[(&str, &str, bool)]) -> Frames {
    use nightjar_core::http::native_api::library::{Artwork, Game, Library, Store};
    let games = games
        .iter()
        .enumerate()
        .map(|(n, (title, key, installed))| Game {
            install_id: format!("install-{n}"),
            app_id: 1000 + n as u32,
            title: (*title).into(),
            game_id: Some(format!("{}:{n}", if key.is_empty() { "custom" } else { key })),
            store: (!key.is_empty()).then(|| Store { source: (*key).into(), id: n.to_string(), name: String::new() }),
            installed: Some(*installed),
            running: false,
            artwork: Artwork { cover: None, version: String::new() },
        })
        .collect();
    let mut f = Frames::with(record("Desk", true, true), Reach::Online, Some(answer(true, 0)), Vec::new());
    f.hosts.list[0].native.library = Some(Library { revision: "r1".into(), games });
    f.l.go(Screen::Home);
    f.settle();
    f
}

/// Twelve Steam games, two from Epic, Battle.net and Xbox each, one not installed, and an app of no store.
fn store_mix() -> Frames {
    let mut games: Vec<(String, &str, bool)> = (0..12).map(|n| (format!("Steam {n:02}"), "steam", true)).collect();
    games.extend([
        ("Fortnite".to_owned(), "epic", true),
        ("Alan Wake 2".to_owned(), "epic", false),
        ("Diablo IV".to_owned(), "battlenet", true),
        ("Overwatch 2".to_owned(), "battlenet", true),
        ("Halo Infinite".to_owned(), "microsoft", true),
        ("Forza Horizon 5".to_owned(), "microsoft", true),
        ("Desktop".to_owned(), "", true),
    ]);
    let borrowed: Vec<(&str, &str, bool)> = games.iter().map(|(t, k, i)| (t.as_str(), *k, *i)).collect();
    home_with_stores(&borrowed)
}

fn in_collections(f: &mut Frames, games: &[(&str, &[&str])]) {
    f.hosts.list[0].native.collections = games.iter().map(|(id, names)| ((*id).to_owned(), names.iter().map(|n| (*n).to_owned()).collect())).collect();
    f.settle();
}

/// Collection joins Group by only while the PCs name collections; its rows are the collections by
/// name, a game in several in each, then the games in none.
#[test]
fn home_groups_by_collection_only_when_the_pc_names_any() {
    let mut f = store_mix();
    f.size = vec2(1280.0, 2400.0);
    in_collections(&mut f, &[("install-0", &["VR", "Couch"]), ("install-12", &["Couch"])]);
    let view = f.placed.iter().find(|(t, ..)| t == "View").map(|p| p.1).expect("View in the top row");
    f.click_at(view.center(), egui::PointerButton::Primary);
    let row = |f: &Frames, label: &str| f.placed.iter().find(|(t, ..)| t == label).map(|p| p.1).expect(label);
    for (next, says) in [(HomeGroup::Type, "Type"), (HomeGroup::Collection, "Collection")] {
        let at = row(&f, "Group by");
        f.click_at(at.center(), egui::PointerButton::Primary);
        assert_eq!(f.l.settings.home_group, next);
        assert!(shows(&f, says), "{says}: {:?}", f.drawn);
    }
    let shelf = |f: &Frames, name: &str| f.placed.iter().find(|(t, r, _)| t == name && r.center().y > 80.0).map(|p| p.1.top()).expect(name);
    assert!(shelf(&f, "Couch") < shelf(&f, "VR") && shelf(&f, "VR") < shelf(&f, "No collection"), "{:?}", f.drawn);
    let copies = f.drawn.iter().filter(|t| *t == "Steam 00").count();
    assert!(copies >= 2, "in both of its collections: {:?}", f.drawn);

    // The PC stops naming any: the rows go back to stores, and the choice stays for when it does again.
    in_collections(&mut f, &[]);
    assert_eq!(f.l.settings.home_group, HomeGroup::Collection);
    assert!(shows(&f, "Steam") && !shows(&f, "No collection"), "{:?}", f.drawn);
    assert!(shows(&f, "Store"), "the menu says what shows: {:?}", f.drawn);
    let at = row(&f, "Group by");
    f.click_at(at.center(), egui::PointerButton::Primary);
    assert_eq!(f.l.settings.home_group, HomeGroup::Type);
    let at = row(&f, "Group by");
    f.click_at(at.center(), egui::PointerButton::Primary);
    assert_eq!(f.l.settings.home_group, HomeGroup::None, "no Collection to step through");
}

/// A game in several rows is a card of its own in each: focus comes back to the one it left, and
/// moving between them counts as moving, so the new one scrolls into view.
#[test]
fn a_game_in_two_rows_has_a_focus_key_in_each() {
    let mut f = store_mix();
    f.size = vec2(1280.0, 2400.0);
    in_collections(&mut f, &[("install-0", &["VR", "Couch"])]);
    f.l.settings.home_group = HomeGroup::Collection;
    f.settle();
    let game = "game:steam:0";
    let of_game: Vec<String> = f.l.cards.iter().filter(|(_, g)| g == game).map(|(key, _)| key.clone()).collect();
    assert_eq!(of_game, [format!("row:collection:couch:game:{game}"), format!("row:collection:vr:game:{game}")]);
    f.l.restore_key = Some(of_game[1].clone());
    f.settle();
    assert_eq!(f.l.focused_key.as_ref(), Some(&of_game[1]), "the second card, not the first");
    assert!(f.l.restore_key.is_none());
    f.press(Key::ArrowUp);
    assert_eq!(f.l.focused_key.as_ref(), Some(&of_game[0]), "up to the same game in the row above");
}

/// Hide moves the focus to the next card of another game, as Unstar does, past the game's other
/// cards; the last one's goes to the one before it.
#[test]
fn hiding_a_game_from_its_menu_focuses_the_next_card() {
    let mut f = home_with_stores(&[("Alpha", "steam", true), ("Beta", "steam", true)]);
    f.l.restore_key = Some("row:store:steam:game:game:steam:0".to_owned());
    f.settle();
    f.press(Key::F2);
    assert!(shows(&f, "Hide game"), "{:?}", f.drawn);
    // Down past Game details and the star to Hide game, as a controller does.
    f.press(Key::ArrowDown);
    f.press(Key::ArrowDown);
    let commands = f.frame(crate::gui::tap(Key::Enter).to_vec());
    let Some(Command::SetHidden { games, hidden: true }) = commands.into_iter().find(|c| matches!(c, Command::SetHidden { .. })) else {
        panic!("no SetHidden");
    };
    assert!(crate::config::set_hidden(&mut f.l.hidden_games, games, true));
    f.settle();
    assert!(f.no_dialog());
    assert_eq!(f.l.focused_key.as_deref(), Some("row:store:steam:game:game:steam:1"), "Beta has the focus");
    assert!(f.ctx.memory(|m| m.focused()).is_some(), "something holds the focus");

    let cards = |keys: &[(&str, &str)]| -> Vec<(String, String)> { keys.iter().map(|(k, g)| ((*k).to_owned(), (*g).to_owned())).collect() };
    let shown = cards(&[("couch:a", "a"), ("couch:b", "b"), ("vr:a", "a"), ("vr:c", "c")]);
    assert_eq!(library::card_neighbour(&shown, "couch:a").as_deref(), Some("couch:b"));
    assert_eq!(library::card_neighbour(&shown, "couch:b").as_deref(), Some("vr:a"));
    assert_eq!(library::card_neighbour(&shown, "vr:a").as_deref(), Some("vr:c"));
    assert_eq!(library::card_neighbour(&shown, "vr:c").as_deref(), Some("vr:a"), "the last: the one before");
    assert_eq!(library::card_neighbour(&cards(&[("x:a", "a"), ("y:a", "a")]), "y:a"), None, "nothing else");
    assert_eq!(library::card_neighbour(&shown, "gone"), None);
}

/// A favourite's card is the same game as its library card, which goes with it when it is hidden:
/// the focus moves past that one too, or the next D-pad press finds nothing and starts at the top.
#[test]
fn hiding_a_favourite_focuses_the_next_game_not_its_own_library_card() {
    let mut f = home_with_stores(&[("Alpha", "steam", true), ("Beta", "steam", true)]);
    let id = f.hosts.list[0].id;
    f.l.favourites = vec![crate::config::Favourite { host: "Desk".into(), install_id: "install-0".into(), app_id: 1000, title: "Alpha".into() }];
    f.l.restore_key = Some(format!("favourite:{id}:1000"));
    f.settle();
    f.press(Key::F2);
    assert!(shows(&f, "Hide game"), "{:?}", f.drawn);
    f.press(Key::ArrowDown);
    f.press(Key::ArrowDown);
    let commands = f.frame(crate::gui::tap(Key::Enter).to_vec());
    let Some(Command::SetHidden { games, hidden: true }) = commands.into_iter().find(|c| matches!(c, Command::SetHidden { .. })) else {
        panic!("no SetHidden");
    };
    assert!(crate::config::set_hidden(&mut f.l.hidden_games, games, true));
    f.settle();
    assert_eq!(f.l.focused_key.as_deref(), Some("row:store:steam:game:game:steam:1"), "Beta, not Alpha's own card");
    assert!(f.ctx.memory(|m| m.focused()).is_some(), "something holds the focus");
}

/// A PC's page showing hidden games keeps the card through Hide and Show: the focus stays on it.
/// With them not shown the card goes, and the focus with it.
#[test]
fn hiding_on_a_pcs_page_moves_the_focus_only_when_the_card_goes() {
    let mut f = home_with_games(3);
    let id = f.hosts.list[0].id;
    f.hosts.list[0].apps = Apps::Loaded(vec![game(7), game(9)]);
    f.l.go(Screen::Apps(id));
    for (shows_hidden, focus) in [(true, "app:7"), (false, "app:9")] {
        f.l.show_hidden_apps = shows_hidden;
        f.l.restore_key = Some("app:7".to_owned());
        f.settle();
        assert_eq!(f.l.focused_key.as_deref(), Some("app:7"));
        f.press(Key::F2);
        let hide = f.placed.iter().find(|(t, ..)| t == "Hide game").map(|p| p.1).expect("Hide game in the menu");
        let commands = f.click_at(hide.center(), egui::PointerButton::Primary);
        assert!(commands.iter().any(|c| matches!(c, Command::SetHidden { hidden: true, .. })));
        assert_eq!(f.l.focused_key.as_deref(), Some(focus));
    }
}

/// A running game stays under Continue playing when hidden, off the library's rows, and its card's
/// menu shows it again: hiding must not lose the way back to it.
#[test]
fn a_running_hidden_game_stays_under_continue_playing_and_can_be_shown_again() {
    let mut f = home_with_stores(&[("Alpha", "steam", true), ("Beta", "steam", true)]);
    f.hosts.list[0].native.library.as_mut().unwrap().games[0].running = true;
    let id = f.hosts.list[0].id;
    let keys = {
        let sources: Vec<crate::console::Source> = f.hosts.list.iter().map(crate::console::Source::of).collect();
        crate::console::hide_keys(&sources, id, 1000)
    };
    assert_eq!(keys.len(), 1);
    f.l.hidden_games = keys;
    f.settle();
    // Running, it is Home's hero: hiding must not lose the way back to it.
    assert!(shows(&f, "Alpha") && shows(&f, "Resume"), "{:?}", f.drawn);
    let card = f.ctx.read_response(egui::Id::unique("hero-cover")).expect("the hero's cover").rect;
    f.click_at(card.center(), egui::PointerButton::Secondary);
    let show = f.placed.iter().find(|(t, ..)| t == "Show game").map(|p| p.1).expect("Show game in the menu");
    let commands = f.click_at(show.center(), egui::PointerButton::Primary);
    assert!(commands.iter().any(|c| matches!(c, Command::SetHidden { hidden: false, .. })));
    assert!(f.l.focused_key.as_deref().is_some_and(|key| key.starts_with("hero-")), "the hero keeps the focus: {:?}", f.l.focused_key);
    assert_eq!(id, f.hosts.list[0].id);
    assert!(!shows(&f, "Close game"), "one word, Quit game: {:?}", f.drawn);
}

/// The hero's cover opens the same menu as a shelf's, with F2 from its buttons too, and the focus
/// comes back to what asked.
#[test]
fn the_hero_opens_its_games_menu_and_gives_focus_back() {
    let mut f = home_with_stores(&[("Alpha", "steam", true), ("Beta", "steam", true)]);
    let cover = f.ctx.read_response(egui::Id::unique("hero-cover")).expect("the hero's cover").rect;
    f.click_at(cover.center(), egui::PointerButton::Secondary);
    assert!(shows(&f, "Hide game"), "{:?}", f.drawn);
    f.press(Key::Escape);
    let focused = f.ctx.memory(|m| m.focused()).expect("focus");
    assert_eq!(focused, egui::Id::unique("hero-cover"));
    let star = egui::Id::unique("hero-star");
    f.ctx.memory_mut(|m| m.request_focus(star));
    f.settle();
    f.press(Key::F2);
    assert!(matches!(f.l.dialog, Dialog::GameMenu { .. }), "F2 opens the game's menu");
}

/// A title's word is never cut in two on a tile: the size gives way.
#[test]
fn a_long_word_on_a_tile_stays_whole() {
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    let mut out = Vec::new();
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        cover_shapes(ui, &mut out, Rect::from_min_size(Pos2::ZERO, vec2(155.0, 232.0)), 155.0, 16.0, "Brightwater", None, 1.0);
    });
    output.textures_delta.clear();
    let rows: Vec<usize> = out.iter().filter_map(|shape| match shape {
        egui::Shape::Text(text) => Some(text.galley.rows.len()),
        _ => None,
    }).collect();
    assert_eq!(rows, [1], "one line");
}

/// The demo library in a window of this size, on Home.
fn demo_frames(size: egui::Vec2) -> Frames {
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    let mut l = launcher(true, false);
    let mut hosts = Hosts::without_discovery(super::demo::records(), Arc::new(|| {}));
    super::demo::fill(&mut hosts, &mut l, &ctx);
    let mut f = Frames::over(ctx, l, hosts);
    f.size = size;
    f.settle();
    f
}

fn demo_page(f: &mut Frames, page: super::demo::Page) {
    let waker: crate::hosts::Waker = Arc::new(|| {});
    let command = {
        let Frames { l, hosts, .. } = &mut *f;
        l.demo_page(page, hosts, waker)
    };
    if let Some(Command::ProfileSettings(id)) = command {
        f.l.go(Screen::ProfileSettings(id));
    }
    f.settle();
    f.settle();
}

fn wheel(f: &mut Frames, at: Pos2, points: f32) {
    f.frame(vec![egui::Event::PointerMoved(at)]);
    f.frame(vec![egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, points),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
        modifiers: egui::Modifiers::NONE,
    }]);
    for _ in 0..5 {
        f.settle();
    }
}

/// At 760 wide the Help tab ran under the search button. The tabs give way to what is on their right:
/// first their padding, then their words, and each keeps its name for a screen reader and the pointer.
#[test]
fn the_tabs_never_run_under_the_search_button_in_a_narrow_window() {
    for (width, worded) in [(1280.0, true), (900.0, true), (760.0, true), (506.0, false)] {
        let mut f = demo_frames(vec2(width, 700.0));
        f.settle();
        let in_bar = |rect: &&Rect| rect.top() < 60.0;
        let pc = f.placed.iter().filter(|(text, ..)| text.contains("Living Room PC")).map(|(_, rect, _)| rect).rfind(in_bar).copied().expect("the PC capsule");
        let help = f.placed.iter().filter(|(text, ..)| text == "Help").map(|(_, rect, _)| rect).find(in_bar).copied();
        assert_eq!(help.is_some(), worded, "{width}: the tabs' words");
        if let Some(help) = help {
            // The search button, its gaps and the PC capsule's own padding sit between them.
            assert!(help.right() + 40.0 < pc.left(), "{width}: Help ends at {}, the PC's name starts at {}", help.right(), pc.left());
        }
    }
    assert_eq!(super::chrome::nav_fit(&[50.0, 30.0, 60.0, 35.0], 1000.0), (18.0, true));
    let (pad, worded) = super::chrome::nav_fit(&[50.0, 30.0, 60.0, 35.0], 400.0);
    assert!(worded && (10.0..18.0).contains(&pad), "{pad}");
    assert_eq!(super::chrome::nav_fit(&[50.0, 30.0, 60.0, 35.0], 250.0), (10.0, false));
}

/// Every page is one area the size of the window: what it holds moves up beneath the chrome and is
/// cut only by the window's own edges, never at a line of its own below the chrome or above the
/// bottom. The scrim that keeps the chrome legible comes with it, and it leaves again at the top.
#[test]
fn every_page_scrolls_whole_beneath_the_chrome() {
    use super::demo::Page;
    let size = vec2(1000.0, 300.0);
    let mut f = demo_frames(size);
    for (name, page) in Page::ALL {
        demo_page(&mut f, page);
        let before = f.placed.clone();
        let opened = f.l.page_scrolled;
        wheel(&mut f, Pos2::new(990.0, 280.0), -160.0);
        // Files lists on a thread of its own, late on a busy machine; until then the page is short.
        for _ in 0..200 {
            if f.l.page_scrolled > opened + 50.0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
            wheel(&mut f, Pos2::new(990.0, 280.0), -160.0);
        }
        assert!(f.l.page_scrolled > opened + 50.0, "{name}: the wheel scrolls it from anywhere on the window");
        let moved: Vec<_> = f.placed.iter().filter(|(text, rect, _)| before.iter().any(|(was, at, _)| was == text && at.top() > rect.top() + 50.0)).collect();
        assert!(!moved.is_empty(), "{name}: the page's text moved");
        // A card may clip its own words; the page's are cut by the window alone.
        let window = |clip: &Rect| clip.top() <= 0.5 && clip.bottom() >= size.y - 0.5;
        assert!(moved.iter().any(|(_, _, clip)| window(clip)), "{name}: a clip of its own: {:?}", moved.iter().map(|m| m.2).collect::<Vec<_>>());
        let screen = Rect::from_min_size(Pos2::ZERO, size);
        assert!(!f.l.ambient.veil(&f.ctx, screen, chrome::brand_rect(screen), f.l.page_scrolled).is_empty(), "{name}: the veil comes with scrolling");
        wheel(&mut f, Pos2::new(990.0, 280.0), 4000.0);
        assert!(f.l.page_scrolled < 0.5, "{name} comes back to its top");
    }
}

/// The arrow keys walk a page past the window's edges and back: what they reach rests clear of the
/// chrome, on any page, and coming back up ends at the top.
#[test]
fn the_arrow_keys_keep_the_focus_clear_of_the_chrome_on_every_long_page() {
    use super::demo::Page;
    let size = vec2(1280.0, 480.0);
    for (name, page) in [("all-games", Page::AllGames), ("apps", Page::Apps), ("settings", Page::Settings), ("pc-settings", Page::PcSettings)] {
        let mut f = demo_frames(size);
        demo_page(&mut f, page);
        let focused = |f: &Frames| {
            let id = f.ctx.memory(|m| m.focused()).expect("something is focused");
            (id, f.ctx.read_response(id).expect("the focused widget was drawn").rect)
        };
        // Down the rows of a settings page: the list beside them takes Right first.
        if matches!(page, Page::Settings | Page::PcSettings) {
            f.press(Key::ArrowRight);
        }
        let start = f.l.page_scrolled;
        let mut walked = 0;
        let mut last = focused(&f).0;
        for _ in 0..14 {
            f.press(Key::ArrowDown);
            f.settle();
            f.settle();
            let (id, rect) = focused(&f);
            if id == last {
                continue;
            }
            last = id;
            walked += 1;
            assert!(rect.top() >= chrome::PAGE_TOP - 8.0, "{name}: {rect:?} rests beneath the chrome");
            assert!(rect.bottom() <= size.y + 0.5, "{name}: {rect:?} runs off the window");
        }
        assert!(walked >= 3, "{name}: the focus walked: {walked}");
        assert!(f.l.page_scrolled > start + 50.0, "{name}: the page followed it: {}", f.l.page_scrolled);
        for _ in 0..20 {
            f.press(Key::ArrowUp);
            f.settle();
            f.settle();
            let (_, rect) = focused(&f);
            assert!(rect.bottom() >= 0.0 && rect.top() <= size.y, "{name}: the focus stays on screen going up: {rect:?}");
        }
    }
}

/// On a wide window the sections stay put below the chrome while the rows go by, and a settings page
/// with a header first lets them rise to that place. The list is not cut by the chrome.
#[test]
fn the_side_list_stays_below_the_chrome_while_the_rows_scroll() {
    use super::demo::Page;
    let mut f = demo_frames(vec2(1600.0, 700.0));
    let top_of = |f: &Frames, text: &str| f.placed.iter().find(|(t, ..)| t == text).map(|p| p.1.top()).unwrap_or_else(|| panic!("{text} is drawn"));
    demo_page(&mut f, Page::Settings);
    let (entry, row) = (top_of(&f, "Audio"), top_of(&f, "Codec"));
    wheel(&mut f, Pos2::new(1500.0, 600.0), -200.0);
    assert!(top_of(&f, "Codec") < row - 50.0, "the rows move");
    assert!((top_of(&f, "Audio") - entry).abs() < 0.5, "the sections do not");
    wheel(&mut f, Pos2::new(120.0, 680.0), 200.0);
    assert!((top_of(&f, "Codec") - row).abs() < 0.5, "the wheel over the list scrolls the page too");

    demo_page(&mut f, Page::PcSettings);
    let entry = top_of(&f, "Audio");
    wheel(&mut f, Pos2::new(1500.0, 600.0), -600.0);
    let risen = top_of(&f, "Audio");
    assert!(risen < entry, "under a header the list rises first: {entry} to {risen}");
    let search_top = f.placed.iter().filter(|(t, ..)| t == "Audio").map(|p| p.1.top()).fold(f32::MAX, f32::min);
    assert!(search_top >= chrome::PAGE_TOP, "and stops below the chrome: {search_top}");
    // The window too short for the list: it goes with the page, or its end could never be seen.
    let mut f = demo_frames(vec2(1600.0, 300.0));
    demo_page(&mut f, Page::Settings);
    f.settle();
    let entry = top_of(&f, "Audio");
    wheel(&mut f, Pos2::new(1500.0, 280.0), -100.0);
    assert!(top_of(&f, "Audio") < entry - 50.0, "the short window's list scrolls");
}

/// Help's section chips land the section below the chrome, not beneath it.
#[test]
fn a_help_section_jump_lands_below_the_chrome() {
    use super::demo::Page;
    let mut f = demo_frames(vec2(1280.0, 480.0));
    demo_page(&mut f, Page::Help);
    let chip = f.placed.iter().find(|(t, ..)| t == "Sharing").map(|p| p.1.center()).expect("the Sharing chip");
    f.click_at(chip, egui::PointerButton::Primary);
    f.settle();
    f.settle();
    let tops: Vec<f32> = f.placed.iter().filter(|(t, ..)| t == "Sharing").map(|p| p.1.top()).collect();
    assert!(f.l.page_scrolled > 50.0, "the page moved to it");
    let heading = tops.iter().copied().fold(f32::MIN, f32::max);
    assert!((chrome::PAGE_TOP - 1.0..chrome::PAGE_TOP + 24.0).contains(&heading), "the heading rests just below the chrome: {tops:?}");
}

/// Home at rest draws nothing more: springs and fades ask for frames only while they move.
#[test]
fn home_at_rest_asks_for_no_more_frames() {
    let mut f = store_mix();
    for art in f.hosts.list[0].native.library.as_ref().expect("a library").games.iter().map(|g| g.app_id as i32).collect::<Vec<_>>() {
        f.hosts.list[0].art.insert(art, Art::Missing);
    }
    for _ in 0..12 {
        f.settle();
    }
    assert!(!f.ctx.has_requested_repaint(), "Home at rest asks for no frame");
    f.press(Key::ArrowDown);
    for _ in 0..12 {
        f.settle();
    }
    assert!(!f.ctx.has_requested_repaint(), "nor after the focus has landed");
}

/// The launcher wakes for workers and timers with nothing new to show: the frame comes out the same,
/// so nothing is presented. Whatever does change it, a hover here, is a different frame.
#[test]
fn a_launcher_at_rest_paints_the_same_frame_whenever_it_wakes() {
    use crate::live_glass::{frame_differs, frame_print};
    let mut f = store_mix();
    for art in f.hosts.list[0].native.library.as_ref().expect("a library").games.iter().map(|g| g.app_id as i32).collect::<Vec<_>>() {
        f.hosts.list[0].art.insert(art, Art::Missing);
    }
    for _ in 0..12 {
        f.settle();
    }
    let print = |f: &mut Frames| {
        f.frame(Vec::new());
        let shapes = std::mem::take(&mut f.shapes);
        let primitives = f.ctx.tessellate(shapes, 1.0);
        frame_print(&primitives, 1.0, (1280, 800))
    };
    let shown = print(&mut f);
    for _ in 0..8 {
        let again = print(&mut f);
        assert!(!frame_differs(Some(shown), again, false, false), "an idle wake draws nothing");
    }
    assert!(frame_differs(None, shown, false, false), "the first frame after a stream's is drawn");
    assert!(frame_differs(Some(shown), shown, true, false), "so is one the window asked for");
    assert!(frame_differs(Some(shown), shown, false, true), "and one with a new texture");
    f.frame(vec![egui::Event::PointerMoved(Pos2::new(600.0, 44.0))]);
    for _ in 0..6 {
        f.settle();
    }
    assert!(frame_differs(Some(shown), print(&mut f), false, false), "a hover is a new frame");
}

/// The driver reaches every page of the demo library, on what `fill` puts there and with no dialog
/// in the way: README, store and trailer captures depend on it.
#[test]
fn the_demo_library_reaches_every_page() {
    use super::demo::{self, Page};
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    let mut l = launcher(true, false);
    let mut hosts = Hosts::without_discovery(demo::records(), Arc::new(|| {}));
    demo::fill(&mut hosts, &mut l, &ctx);
    let mut f = Frames::over(ctx, l, hosts);
    f.settle();
    assert_eq!(f.l.screen, Screen::Home);
    assert_eq!(f.hosts.list[0].native.library.as_ref().expect("the library").games.len(), 36);
    assert!(f.hosts.identity().is_none(), "no identity: nothing can ask a PC");
    let pc = f.hosts.list[0].id;
    for (name, page) in Page::ALL {
        let waker: crate::hosts::Waker = Arc::new(|| {});
        let command = { let Frames { l, hosts, .. } = &mut f; l.demo_page(page, hosts, waker) };
        if let Some(Command::ProfileSettings(id)) = command {
            f.l.go(Screen::ProfileSettings(id));
        }
        for _ in 0..4 {
            f.settle();
        }
        let expected = match page {
            Page::Home => Screen::Home,
            Page::AllGames | Page::Search => Screen::AllGames,
            Page::Game => Screen::Game { host: pc, app: 1002 },
            Page::Pcs | Page::FirstRun | Page::FirstRunFound => Screen::Hosts,
            Page::Apps => Screen::Apps(pc),
            Page::AppsEmpty => Screen::Apps(f.hosts.list[1].id),
            Page::Pc => Screen::HostDetails(pc),
            Page::Files => Screen::Files(pc),
            Page::Settings => Screen::Settings,
            Page::PcSettings => Screen::HostSettings(pc),
            Page::GameSettings => Screen::AppSettings(pc),
            Page::Profiles => Screen::Profiles,
            Page::ProfileSettings => Screen::ProfileSettings(1),
            Page::Controllers => Screen::Controllers,
            Page::Shortcuts => Screen::Shortcuts,
            Page::Hotkeys => Screen::StreamHotkeys,
            Page::Help => Screen::Help,
        };
        assert_eq!(f.l.screen, expected, "{name}");
        assert!(f.no_dialog(), "{name} opens with nothing over it");
    }
}

/// The hero is what runs, else what ran last, else the library's first installed game.
#[test]
fn the_hero_is_the_running_game_then_the_last_played_then_the_first_installed() {
    let mut f = home_with_stores(&[("Alpha", "steam", false), ("Beta", "steam", true), ("Gamma", "steam", true)]);
    let hero = |f: &Frames| f.l.hero.as_ref().map(|h| h.title.clone());
    assert_eq!(hero(&f).as_deref(), Some("Beta"), "the first installed");
    f.l.recents.push(crate::config::Recent { host: "Desk".into(), install_id: "install-2".into(), app_id: 1002, title: "Gamma".into(), at: 1000 });
    f.settle();
    assert_eq!(hero(&f).as_deref(), Some("Gamma"), "the last played");
    f.hosts.list[0].native.library.as_mut().expect("a library").games[0].running = true;
    f.settle();
    assert_eq!(hero(&f).as_deref(), Some("Alpha"), "what runs");
}

/// Ctrl K opens the search on All games; the words narrow it; Escape takes both away and gives the
/// focus back to what had it.
#[test]
fn the_search_filters_all_games_and_escape_goes_back() {
    let mut f = store_mix();
    let before = f.ctx.memory(|m| m.focused()).expect("focus on Home");
    f.frame(vec![egui::Event::Key { key: Key::K, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND }]);
    f.settle();
    assert_eq!(f.l.screen, Screen::AllGames);
    assert!(f.ctx.text_edit_focused(), "the field has the keys");
    f.l.library_query = "fort".into();
    f.settle();
    assert!(shows(&f, "Fortnite") && !shows(&f, "Forza Horizon 5") && !shows(&f, "Diablo IV"), "{:?}", f.drawn);
    f.press(Key::Escape);
    assert_eq!(f.l.screen, Screen::Home, "a page opened for the search goes with it");
    assert!(f.l.library_query.is_empty());
    assert_eq!(f.ctx.memory(|m| m.focused()), Some(before), "the focus is back");
}

/// Into a game and back: Back lands on the cover it left.
#[test]
fn back_from_a_game_restores_the_focus_on_home() {
    let mut f = store_mix();
    let shelf = (0..6).find_map(|_| {
        f.press(Key::ArrowDown);
        let id = f.ctx.memory(|m| m.focused())?;
        let rect = f.ctx.read_response(id)?.rect;
        (rect.height() > 200.0 && rect.top() > 300.0).then_some(id)
    });
    let shelf = shelf.expect("a shelf's cover");
    f.press(Key::Enter);
    assert!(matches!(f.l.screen, Screen::Game { .. }), "{:?}", f.l.screen);
    f.press(Key::Escape);
    assert_eq!(f.l.screen, Screen::Home);
    assert_eq!(f.ctx.memory(|m| m.focused()), Some(shelf));
}

/// Covers arriving change nothing's place: a cover's slot is the same with art, without, and while
/// it is on its way.
#[test]
fn covers_arriving_move_nothing() {
    let mut f = home_with_games(4);
    let place = |f: &Frames| f.placed.iter().find(|(t, ..)| t == "PCs").map(|p| p.1.top());
    let heading = |f: &Frames| f.placed.iter().filter(|(t, ..)| t == "Desk").map(|p| p.1.top()).fold(0.0, f32::max);
    let loading = (place(&f), heading(&f));
    let texture = f.ctx.load_texture("art", egui::ColorImage::new([3, 4], vec![Color32::GRAY; 12]), egui::TextureOptions::LINEAR);
    let pc = &mut f.hosts.list[0];
    for g in &mut pc.native.library.as_mut().expect("a library").games {
        pc.art.insert(g.app_id as i32, Art::Ready(texture.clone()));
    }
    f.settle();
    assert_eq!((place(&f), heading(&f)), loading, "art");
    let pc = &mut f.hosts.list[0];
    for g in &mut pc.native.library.as_mut().expect("a library").games {
        pc.art.insert(g.app_id as i32, Art::Missing);
    }
    f.settle();
    assert_eq!((place(&f), heading(&f)), loading, "tiles");
}

/// Hide on a card's menu takes the game off Home on this device; All games has it under Hidden,
/// where the same menu brings it back.
#[test]
fn a_game_hides_from_home_and_shows_again_from_all_games() {
    let mut f = home_with_stores(&[("Alpha", "steam", true), ("Beta", "steam", true)]);
    let card = f.placed.iter().find(|(t, r, _)| t == "Beta" && r.center().y > 80.0).map(|p| p.1).expect("Beta's card");
    f.click_at(card.center(), egui::PointerButton::Secondary);
    let hide = f.placed.iter().find(|(t, ..)| t == "Hide game").map(|p| p.1).expect("Hide game in the menu");
    let commands = f.click_at(hide.center(), egui::PointerButton::Primary);
    let Some(Command::SetHidden { games, hidden: true }) = commands.into_iter().find(|c| matches!(c, Command::SetHidden { .. })) else {
        panic!("no SetHidden");
    };
    assert_eq!(games.len(), 1);
    assert_eq!((games[0].host.as_str(), games[0].install_id.as_str()), ("Desk", "install-1"));
    assert!(crate::config::set_hidden(&mut f.l.hidden_games, games, true));
    f.settle();
    assert!(f.no_dialog());
    assert!(!shows(&f, "Beta") && shows(&f, "Alpha"), "{:?}", f.drawn);

    f.l.go(Screen::AllGames);
    f.settle();
    assert!(shows(&f, "Hidden 1") || shows(&f, "Hidden"), "{:?}", f.drawn);
    assert!(!shows(&f, "Beta"), "only under Hidden: {:?}", f.drawn);
    f.l.library_filter = crate::console::LibraryFilter::Hidden;
    f.settle();
    assert!(shows(&f, "Beta") && !shows(&f, "Alpha"), "{:?}", f.drawn);
    let card = f.placed.iter().find(|(t, r, _)| t == "Beta" && r.center().y > 80.0).map(|p| p.1).expect("Beta's card");
    f.click_at(card.center(), egui::PointerButton::Secondary);
    let show = f.placed.iter().find(|(t, ..)| t == "Show game").map(|p| p.1).expect("Show game in the menu");
    let commands = f.click_at(show.center(), egui::PointerButton::Primary);
    let Some(Command::SetHidden { games, hidden: false }) = commands.into_iter().find(|c| matches!(c, Command::SetHidden { .. })) else {
        panic!("no SetHidden");
    };
    assert!(crate::config::set_hidden(&mut f.l.hidden_games, games, false));
    f.settle();
    assert!(f.l.hidden_games.is_empty());
    assert_eq!(f.l.library_filter, crate::console::LibraryFilter::All, "the empty Hidden chip gives way");
    assert!(shows(&f, "Beta"), "{:?}", f.drawn);
}

/// Home has a row per store, largest first, each as wide as the window and ending in an All tile
/// when more games follow; the apps of no store come last as Other apps.
#[test]
fn home_shows_a_row_per_store_that_ends_in_an_all_tile() {
    let mut f = store_mix();
    f.size = vec2(1280.0, 2400.0);
    f.settle();
    let shelf = |f: &Frames, name: &str| f.placed.iter().find(|(t, r, _)| t == name && r.center().y > 80.0).map(|p| p.1.top()).expect(name);
    // Largest first, ties by key; Epic, Battle.net and Xbox have their rows as Steam does.
    let order = ["Steam", "Battle.net", "Epic Games", "Xbox", "Other apps"].map(|name| shelf(&f, name));
    assert!(order.windows(2).all(|w| w[0] < w[1]), "rows in order: {order:?}");
    f.size = vec2(1280.0, 720.0);
    f.settle();
    // Twelve Steam games do not fit one row at 1280: the row ends in "All 12", which opens All
    // games on Steam.
    assert!(shows(&f, "All 12"), "{:?}", f.drawn);
    assert!(!shows(&f, "Steam 11"), "the rest wait behind the tile");
    let tile = f.placed.iter().find(|(t, ..)| t == "All 12").map(|p| p.1).unwrap();
    f.click_at(tile.center(), egui::PointerButton::Primary);
    assert_eq!(f.l.screen, Screen::AllGames);
    assert_eq!(f.l.library_filter, console::LibraryFilter::Store("steam".into()));
    assert!(shows(&f, "Steam 11") && !shows(&f, "Fortnite"), "only Steam's games: {:?}", f.drawn);
    // Back returns to Home.
    f.press(Key::Escape);
    assert_eq!(f.l.screen, Screen::Home);
}

/// A column so narrow that one cover fills a line still shows each long row's All tile, alone,
/// instead of a header over nothing; the tile still opens the row's games. (The top bar keeps the
/// window's page wider than that, so Home is drawn into a narrow column of its own here.)
#[test]
fn at_one_column_a_long_row_is_its_all_tile() {
    let mut f = store_mix();
    let column = vec2(210.0, 2400.0);
    assert_eq!(grid(column.x - 2.0 * chrome::page_margin(column.x)).0, 1, "one column");
    let mut drawn = Vec::new();
    let mut tile = None;
    let mut clicked_at = None;
    for n in 0..6 {
        // The last passes click where the tile was drawn.
        let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        let events = match (n, tile.or(clicked_at)) {
            (3, Some(at)) => {
                clicked_at = Some(at);
                vec![egui::Event::PointerMoved(at)]
            }
            (4, Some(at)) => vec![button(at, true)],
            (5, Some(at)) => vec![button(at, false)],
            _ => Vec::new(),
        };
        f.time += 0.05;
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, column)), time: Some(f.time), events, focused: true, ..Default::default() };
        let mut commands = Vec::new();
        let mut output = f.ctx.run_ui(input, |ui| {
            let view = View { hosts: &f.hosts, pads: &f.pads, connecting: None, waking: None };
            draw_home(&mut f.l, ui, &view, &mut commands);
        });
        // Debug builds of epaint refuse to drop font textures nobody uploaded.
        output.textures_delta.clear();
        drawn.clear();
        let mut placed = Vec::new();
        for clipped in &output.shapes {
            collect_text(&clipped.shape, &mut drawn);
            collect_placed(&clipped.shape, clipped.clip_rect, &mut placed);
        }
        tile = placed.iter().find(|(t, ..)| t == "All 12").map(|p| p.1.center());
    }
    assert!(drawn.iter().any(|t| t == "All 2"), "every long row ends in its tile: {drawn:?}");
    assert!(!drawn.iter().any(|t| t == "Steam 00"), "no cover fits beside the tile: {drawn:?}");
    assert!(tile.is_some(), "Steam's row is its All tile: {drawn:?}");
    assert_eq!(f.l.screen, Screen::AllGames, "the tile opens All games");
    assert_eq!(f.l.library_filter, console::LibraryFilter::Store("steam".into()));
}

/// A screen reader hears whose games an All tile opens, which destination is the current one,
/// and a subpage's Back apart from the destination of the same name.
#[test]
fn screen_readers_hear_the_all_tiles_store_the_current_destination_and_where_back_leads() {
    fn node<'a>(f: &'a Frames, label: &str) -> Option<&'a egui::accesskit::Node> {
        f.access.as_ref()?.nodes.iter().map(|(_, n)| n).find(|n| n.label() == Some(label))
    }
    use egui::accesskit::Toggled;
    let mut f = store_mix();
    f.ctx.enable_accesskit();
    f.settle();
    assert!(node(&f, "All Steam games, 12").is_some(), "the tile names its store");
    assert_eq!(node(&f, "Home").and_then(|n| n.toggled()), Some(Toggled::True), "Home is the current destination");
    assert_eq!(node(&f, "PCs").and_then(|n| n.toggled()), Some(Toggled::False));
    // A PC's library from the PCs list: Back leads to PCs and says so.
    let id = f.hosts.list[0].id;
    f.l.go(Screen::Hosts);
    f.l.open(Screen::Apps(id));
    f.settle();
    assert!(node(&f, "Back to PCs").is_some(), "Back is not a second \"PCs\"");
    assert_eq!(node(&f, "PCs").and_then(|n| n.toggled()), Some(Toggled::True), "the PCs pill, marked");
}

/// All games: chips for everything, what is installed, each store and the apps of no store; a chip
/// shows only its games, and a not-installed game says so on its cover.
#[test]
fn all_games_filters_by_store_and_by_what_is_installed() {
    let mut f = store_mix();
    f.l.open(Screen::AllGames);
    f.settle();
    let chip = |f: &Frames, name: &str| f.placed.iter().find(|(t, r, _)| t == name && r.center().y < 200.0).map(|p| p.1);
    for name in ["All", "Installed", "Steam", "Epic Games", "Battle.net", "Xbox", "Other apps"] {
        assert!(chip(&f, name).is_some(), "a {name} chip: {:?}", f.drawn);
    }
    assert!(shows(&f, "Alan Wake 2") && shows(&f, "Not installed"), "All shows what is not installed, marked: {:?}", f.drawn);
    for (name, shown, hidden) in [
        ("Xbox", "Halo Infinite", "Fortnite"),
        ("Battle.net", "Diablo IV", "Steam 00"),
        ("Epic Games", "Fortnite", "Halo Infinite"),
        ("Other apps", "Desktop", "Diablo IV"),
        ("Installed", "Fortnite", "Alan Wake 2"),
    ] {
        let at = chip(&f, name).unwrap();
        f.click_at(at.center(), egui::PointerButton::Primary);
        assert!(shows(&f, shown) && !shows(&f, hidden), "{name}: shows {shown}, not {hidden}: {:?}", f.drawn);
    }
    // The chosen chip is the filled one.
    let installed = chip(&f, "Installed").unwrap();
    assert!(meshes_holding(&f.shapes, design_tokens::TINT_SELECTED_FILL).iter().any(|r| r.contains_rect(installed)), "the chosen chip is marked");
    assert_eq!(f.l.library_filter, console::LibraryFilter::Installed);
}

/// Screenshot review: the focused first cover rises into the top of the scroll, and its ring was
/// cut there (in high contrast most). The grid leaves the room, on All games and a PC's library.
#[test]
fn a_risen_covers_ring_is_not_cut_at_the_top_of_the_grid() {
    for high in [false, true] {
        let mut store = store_mix();
        store.l.open(Screen::AllGames);
        let mut classic = Frames::with(record("Desk", true, true), Reach::Online, Some(answer(true, 0)), vec![game(7), game(9)]);
        let id = classic.hosts.list[0].id;
        classic.l.open(Screen::Apps(id));
        for (page, f) in [("All games", &mut store), ("library", &mut classic)] {
            f.l.settings.high_contrast = high;
            for _ in 0..5 {
                f.settle();
            }
            let cover = f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).expect("the first cover has focus").rect;
            let color = if high { design_tokens::HC_FOCUS_RING_COLOR } else { design_tokens::FOCUS_RING_COLOR };
            let mut rings = Vec::new();
            for clipped in &f.shapes {
                if let egui::Shape::Mesh(mesh) = &clipped.shape {
                    if mesh.vertices.iter().any(|v| v.color == color) {
                        rings.push((mesh.calc_bounds(), clipped.clip_rect));
                    }
                }
            }
            let (ring, clip) = *rings.first().unwrap_or_else(|| panic!("{page}, {high}: a ring"));
            assert!(ring.top() < cover.top() - design_tokens::FOCUS_GAP - 1.0, "{page}, {high}: the cover rose: {ring:?} for {cover:?}");
            assert!(clip.contains_rect(ring), "{page}, {high}: the ring {ring:?} is cut by {clip:?}");
        }
    }
}

/// With nothing installed the Installed chip holds no game and says so; an empty library has no
/// chips at all, only the line saying why.
#[test]
fn an_empty_filter_and_an_empty_library_say_so() {
    let mut f = home_with_stores(&[("Halo Infinite", "microsoft", false), ("Diablo IV", "battlenet", false)]);
    f.l.open(Screen::AllGames);
    f.settle();
    let chip = f.placed.iter().find(|(t, r, _)| t == "Installed" && r.center().y < 200.0).map(|p| p.1).expect("an Installed chip");
    f.click_at(chip.center(), egui::PointerButton::Primary);
    assert!(shows(&f, "No games match this filter.") && !shows(&f, "Halo Infinite"), "{:?}", f.drawn);
    let mut f = home_with_stores(&[]);
    f.l.open(Screen::AllGames);
    f.settle();
    assert!(shows(&f, "No games to show yet. They appear here once a PC that shares its game library is online."), "{:?}", f.drawn);
    assert!(!shows(&f, "All") && !shows(&f, "No games match this filter."), "no chips: {:?}", f.drawn);
}

/// All games is reachable over Home's first shelf even when every row fits and no All tile shows.
/// It opens on All, whatever chip was chosen last.
#[test]
fn home_opens_all_games_from_its_top_row_when_every_row_fits() {
    let mut f = home_with_stores(&[("Portal", "steam", true), ("Celeste", "steam", true), ("Fortnite", "epic", true)]);
    f.l.library_filter = console::LibraryFilter::Store("epic".into());
    f.settle();
    assert!(!f.drawn.iter().any(|t| t.strip_prefix("All ").is_some_and(|n| n.parse::<usize>().is_ok())), "every row fits, no All tile: {:?}", f.drawn);
    let action = f.placed.iter().find(|(t, ..)| t == "All games").map(|p| p.1).expect("All games over the first shelf");
    let view = f.placed.iter().find(|(t, ..)| t == "View").map(|p| p.1).expect("View beside it");
    assert!(action.right() <= view.left() && 1280.0 - view.right() < 80.0, "All games, then View, at the right edge: {action:?} {view:?}");
    f.click_at(action.center(), egui::PointerButton::Primary);
    assert_eq!(f.l.screen, Screen::AllGames);
    assert_eq!(f.l.library_filter, console::LibraryFilter::All);
    assert!(shows(&f, "Portal") && shows(&f, "Fortnite"), "{:?}", f.drawn);
    // An empty library offers no way to an empty page, and nothing to view.
    let mut f = home_with_stores(&[]);
    f.settle();
    assert!(!f.placed.iter().any(|(t, ..)| t == "All games" || t == "View"), "{:?}", f.drawn);
}

/// Home's View: a menu under its button whose rows step through the layout, grouping and card
/// size. Home changes behind it at once, the menu stays for the next try, and each choice is
/// saved; Escape closes it and gives focus back to the button.
#[test]
fn home_view_changes_layout_grouping_and_card_size_in_place() {
    let mut f = store_mix();
    let view = f.placed.iter().find(|(t, ..)| t == "View").map(|p| p.1).expect("View in the top row");
    f.click_at(view.center(), egui::PointerButton::Primary);
    assert!(matches!(f.l.dialog, Dialog::HomeView), "the menu opens");
    for row in ["Layout", "Group by", "Card size", "Rows", "Store", "Medium"] {
        assert!(shows(&f, row), "{row}: {:?}", f.drawn);
    }
    // Rows by store: Steam's twelve end in their All tile.
    assert!(shows(&f, "All 12") && !shows(&f, "Steam 11"), "{:?}", f.drawn);
    let row = |f: &Frames, label: &str| f.placed.iter().find(|(t, ..)| t == label).map(|p| p.1).expect(label);
    let at = row(&f, "Layout");
    let commands = f.click_at(at.center(), egui::PointerButton::Primary);
    assert!(commands.iter().any(|c| matches!(c, Command::SaveSettings)), "the choice is saved");
    assert_eq!(f.l.settings.home_layout, HomeLayout::Grid);
    assert!(matches!(f.l.dialog, Dialog::HomeView), "the menu stays open");
    assert!(shows(&f, "Grid"), "the row says the layout now: {:?}", f.drawn);
    // The grid shows every game of each row: no All tile.
    f.size = vec2(1280.0, 2400.0);
    f.settle();
    assert!(shows(&f, "Steam 11") && !shows(&f, "All 12"), "{:?}", f.drawn);
    assert!(shows(&f, "Battle.net"), "still a part per store: {:?}", f.drawn);

    // Group by: type. The games of every store in one part, the apps of no store in their own.
    let shelves = |f: &Frames| f.placed.iter().filter(|(t, r, _)| r.center().y > 80.0 && ["Steam", "Battle.net", "Epic Games", "Xbox", "Other apps", "Library", "Games"].contains(&t.as_str())).map(|p| p.0.clone()).collect::<Vec<_>>();
    let at = row(&f, "Group by");
    f.click_at(at.center(), egui::PointerButton::Primary);
    assert_eq!(f.l.settings.home_group, HomeGroup::Type);
    assert!(shows(&f, "Type"), "the row says the grouping now: {:?}", f.drawn);
    assert_eq!(shelves(&f), ["Games", "Other apps"], "{:?}", f.drawn);

    // Group by: none. One Library part, no store's.
    let at = row(&f, "Group by");
    f.click_at(at.center(), egui::PointerButton::Primary);
    assert_eq!(f.l.settings.home_group, HomeGroup::None);
    assert_eq!(shelves(&f), ["Library"], "{:?}", f.drawn);
    assert!(shows(&f, "Desktop") && shows(&f, "Fortnite"), "every game in it: {:?}", f.drawn);

    // Back to a part per store, where Steam's first line holds Steam's covers alone.
    let at = row(&f, "Group by");
    f.click_at(at.center(), egui::PointerButton::Primary);
    assert_eq!(f.l.settings.home_group, HomeGroup::Store);
    // Card size: Medium, then Large, then Small, fewer covers a line the larger they are.
    let covers_in_line = |f: &Frames| {
        let first = f.placed.iter().find(|(t, ..)| t == "Steam 00").map(|p| p.1).expect("Steam 00");
        f.placed.iter().filter(|(t, r, _)| t.starts_with("Steam ") && (r.top() - first.top()).abs() < 1.0).count()
    };
    let medium = covers_in_line(&f);
    let at = row(&f, "Card size");
    f.click_at(at.center(), egui::PointerButton::Primary);
    assert_eq!(f.l.settings.home_card_size, CardSize::Large);
    let large = covers_in_line(&f);
    let at = row(&f, "Card size");
    f.click_at(at.center(), egui::PointerButton::Primary);
    assert_eq!(f.l.settings.home_card_size, CardSize::Small);
    let small = covers_in_line(&f);
    assert!(large < medium && medium < small, "large {large}, medium {medium}, small {small}");

    // List: every game a line of its own, its store and when it was last played under the title.
    let at = row(&f, "Layout");
    f.click_at(at.center(), egui::PointerButton::Primary);
    assert_eq!(f.l.settings.home_layout, HomeLayout::List);
    assert!(shows(&f, "List"), "{:?}", f.drawn);
    assert!(!shows(&f, "Card size"), "a list's lines have one size: {:?}", f.drawn);
    assert!(shows(&f, "Steam 11") && !shows(&f, "All 12"), "{:?}", f.drawn);
    assert!(!f.drawn.iter().any(|t| t.to_lowercase().contains("played")), "no PC says when: nothing about playing {:?}", f.drawn);
    // What the PC says: Steam 00 played just now, Steam 01 never.
    let install = |f: &Frames, title: &str| {
        let library = f.hosts.list[0].native.library.as_ref().expect("a library");
        library.games.iter().find(|g| g.title == title).map(|g| g.install_id.clone()).expect(title)
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let (played, never) = (install(&f, "Steam 00"), install(&f, "Steam 01"));
    f.hosts.list[0].native.last_played = std::collections::BTreeMap::from([(played, now), (never, 0)]);
    f.settle();
    assert!(shows(&f, "Steam · Played today") && shows(&f, "Steam · Not played yet"), "{:?}", f.drawn);
    // The title beside the cover (the last of the two the line draws), each line under the one before.
    let title = |f: &Frames, name: &str| f.placed.iter().rev().find(|(t, ..)| t == name).map(|p| p.1).expect(name);
    let (first, second) = (title(&f, "Steam 00"), title(&f, "Steam 01"));
    assert!(second.top() > first.bottom() && (second.left() - first.left()).abs() < 1.0, "one under the other: {first:?} {second:?}");
    // And round to Rows.
    let at = row(&f, "Layout");
    f.click_at(at.center(), egui::PointerButton::Primary);
    assert_eq!(f.l.settings.home_layout, HomeLayout::Rows);
    assert!(shows(&f, "Card size"), "back with the covers: {:?}", f.drawn);
    let at = row(&f, "Layout");
    f.click_at(at.center(), egui::PointerButton::Primary);
    assert_eq!(f.l.settings.home_layout, HomeLayout::Grid);

    f.press(Key::Escape);
    assert!(matches!(f.l.dialog, Dialog::None));
    let focused = f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect).expect("focus");
    assert!(focused.contains_rect(view), "back on View: {focused:?}");
}

/// List: the pad's Down walks from line to line, and A on a line opens its game.
#[test]
fn home_list_lines_walk_with_the_pad_and_open_their_game() {
    let mut f = store_mix();
    f.l.settings.home_layout = HomeLayout::List;
    f.settle();
    let focused = |f: &Frames| {
        let rect = f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect)?;
        f.placed.iter().rev().find(|(t, at, _)| t.starts_with("Steam ") && !t.contains('·') && rect.contains(at.center())).map(|p| p.0.clone())
    };
    for _ in 0..8 {
        if focused(&f).as_deref() == Some("Steam 00") { break; }
        pad_press(&mut f, Key::ArrowDown);
    }
    assert_eq!(focused(&f).as_deref(), Some("Steam 00"), "{:?}", f.drawn);
    pad_press(&mut f, Key::ArrowDown);
    assert_eq!(focused(&f).as_deref(), Some("Steam 01"));
    pad_press(&mut f, Key::Enter);
    assert!(matches!(f.l.screen, Screen::Game { .. }), "{:?}", f.l.screen);
}

/// Rows, not by store: one Library line of what fits, its All tile opening All games.
#[test]
fn home_rows_of_one_library_end_in_its_all_tile() {
    let mut f = store_mix();
    f.l.settings.home_group = HomeGroup::None;
    f.settle();
    assert!(shows(&f, "Library") && shows(&f, "All 19"), "{:?}", f.drawn);
    let tile = f.placed.iter().find(|(t, ..)| t == "All 19").map(|p| p.1).unwrap();
    f.click_at(tile.center(), egui::PointerButton::Primary);
    assert_eq!(f.l.screen, Screen::AllGames);
    assert_eq!(f.l.library_filter, console::LibraryFilter::All);
}

/// A focused cover's title that is cut short runs sideways after a pause, and only then.
#[test]
fn a_focused_cover_title_too_long_runs_sideways_after_a_pause() {
    assert_eq!(caption_offset(200.0, 150.0, false, true, 5.0), None, "not focused");
    assert_eq!(caption_offset(200.0, 150.0, true, false, 5.0), None, "reduced motion");
    assert_eq!(caption_offset(120.0, 150.0, true, true, 5.0), None, "it fits");
    let (start, span) = caption_offset(200.0, 150.0, true, true, 0.5).unwrap();
    assert_eq!(start, 0.0, "the start stays a moment");
    assert!(span > 200.0, "the next copy follows after a gap");
    let (later, _) = caption_offset(200.0, 150.0, true, true, 3.0).unwrap();
    assert!(later > 0.0 && later <= span);
}

/// While a PC's library is on its way, All games says so as Home does instead of calling the
/// library empty.
#[test]
fn all_games_shows_the_library_loading_as_home_does() {
    let mut f = Frames::with(record("Desk", true, true), Reach::Online, Some(files::tests::native_answer()), Vec::new());
    f.l.go(Screen::Home);
    f.settle();
    assert!(shows(&f, "Loading the games…"), "Home: {:?}", f.drawn);
    f.l.open(Screen::AllGames);
    f.settle();
    assert!(shows(&f, "Loading the games…"), "All games: {:?}", f.drawn);
    assert!(!shows(&f, "No games to show yet. They appear here once a PC that shares its game library is online."), "{:?}", f.drawn);
}

/// The chips come before the games for Tab and the arrow keys, and the first game has focus on
/// arrival, so A starts browsing straight away.
#[test]
fn all_games_focuses_the_first_game_and_reaches_the_chips_above_it() {
    let mut f = store_mix();
    f.l.open(Screen::AllGames);
    f.settle();
    let focused = |f: &Frames| f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect).expect("focus");
    let first = focused(&f);
    assert!(first.top() > 120.0, "a game has focus, not a chip: {first:?}");
    f.press(Key::ArrowUp);
    let chip = focused(&f);
    assert!(chip.bottom() < first.top() && chip.height() < 40.0, "Up reaches the chips: {chip:?}");
}

/// A chip keeps the focus when the Installed chip before it comes or goes, and a store Nightjar
/// does not know is named with a capital, not by its bare key.
#[test]
fn a_filter_chip_keeps_the_focus_when_the_installed_chip_comes_or_goes() {
    let mut f = home_with_stores(&[("Portal", "steam", true), ("Celeste", "steam", false), ("Fortnite", "epic", true), ("Lone", "itchio", true)]);
    f.l.open(Screen::AllGames);
    f.settle();
    assert!(shows(&f, "Itchio") && !shows(&f, "itchio"), "{:?}", f.drawn);
    let chip = |f: &Frames, name: &str| f.placed.iter().find(|(t, r, _)| t == name && r.center().y < 200.0).map(|p| p.1).expect(name);
    let focused = |f: &Frames| f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect).expect("focus");
    // Up from the first game reaches the chips; Right walks them to Epic Games.
    f.press(Key::ArrowUp);
    for _ in 0..8 {
        if focused(&f).contains_rect(chip(&f, "Epic Games")) {
            break;
        }
        f.press(Key::ArrowRight);
    }
    assert!(focused(&f).contains_rect(chip(&f, "Epic Games")), "the Epic Games chip has focus");
    // Celeste gets installed: the Installed chip goes, the chips after it move left.
    f.hosts.list[0].native.library.as_mut().unwrap().games[1].installed = Some(true);
    f.settle();
    assert!(!f.placed.iter().any(|(t, r, _)| t == "Installed" && r.center().y < 200.0), "{:?}", f.drawn);
    assert!(focused(&f).contains_rect(chip(&f, "Epic Games")), "the focus stayed on Epic Games");
    // And it comes back.
    f.hosts.list[0].native.library.as_mut().unwrap().games[1].installed = Some(false);
    f.settle();
    assert!(focused(&f).contains_rect(chip(&f, "Epic Games")), "the focus stayed on Epic Games");
}

/// A cover's menu opens with F2 (X) beside it, with a right-click at the pointer, and from its ⋮
/// under the dots; it lists the game's actions with Enter on what selecting the cover does, and
/// Escape gives focus back to the cover.
#[test]
fn a_cover_menu_opens_by_the_cover_and_gives_focus_back() {
    let mut f = store_mix();
    let cover = f.ctx.memory(|m| m.focused()).expect("a cover has focus");
    let rect = f.ctx.read_response(cover).unwrap().rect;
    f.press(Key::F2);
    assert!(matches!(f.l.dialog, Dialog::GameMenu { game: GameTarget::Library { .. }, .. }), "F2 opens the game's menu");
    for row in ["Game details", "Add to favourites", "Open this when Nightjar starts", "Open library"] {
        assert!(shows(&f, row), "{row}: {:?}", f.drawn);
    }
    assert!(shows(&f, "Enter"), "the row selecting does says Enter");
    let details = f.placed.iter().find(|(t, ..)| t == "Game details").map(|p| p.1).unwrap();
    assert!(details.left() > rect.right() || details.right() < rect.left(), "beside the cover, not over it: {details:?} by {rect:?}");
    f.press(Key::Escape);
    assert!(f.no_dialog());
    assert_eq!(f.ctx.memory(|m| m.focused()), Some(cover), "focus is back on the cover");
    // A right-click opens it at the pointer.
    let at = rect.center();
    f.click_at(at, egui::PointerButton::Secondary);
    assert!(matches!(f.l.dialog, Dialog::GameMenu { .. }));
    let details = f.placed.iter().find(|(t, ..)| t == "Game details").map(|p| p.1).unwrap();
    assert!((details.left() - at.x).abs() < 80.0 && (details.top() - at.y).abs() < 120.0, "at the pointer: {details:?} for {at:?}");
    // A press outside closes it.
    f.click_at(Pos2::new(1200.0, 700.0), egui::PointerButton::Primary);
    assert!(f.no_dialog(), "a press outside closes the menu");
    // Enter on Game details opens the game's page.
    f.l.focused_key = None;
    f.l.restore_key = None;
    f.ctx.memory_mut(|m| m.request_focus(cover));
    f.settle();
    f.press(Key::F2);
    f.press(Key::Enter);
    assert!(matches!(f.l.screen, Screen::Game { .. }), "Game details opens the page");
}

/// The page under a menu by its card stays as it is: not faded, and the pointer cannot reach it.
#[test]
fn a_menu_by_its_card_leaves_the_page_undimmed_and_out_of_reach() {
    let mut f = Frames::new(record("Desk", true, true), Reach::Online);
    let (card, dots) = f.card_and_dots();
    f.click_at(dots.center(), egui::PointerButton::Primary);
    assert!(f.options_open());
    assert!(rects_filled(&f, Color32::from_black_alpha(150)).is_empty(), "no backdrop over the page");
    let rename = f.placed.iter().find(|(t, ..)| t == "Rename").map(|p| p.1).unwrap();
    assert!(rename.left() < dots.right(), "by the dots: {rename:?} by {dots:?}");
    // A click on the card under the page closes the menu without starting anything.
    let commands = f.click_at(card.left_center() + vec2(20.0, 0.0), egui::PointerButton::Primary);
    assert!(f.no_dialog(), "a press outside closes the menu");
    assert!(!commands.iter().any(|c| matches!(c, Command::OpenApps(_))), "and does not reach the card");
}

/// Remove PC and Unpair come last, under a divider, in the error colour.
#[test]
fn destructive_options_come_last_in_red() {
    let mut f = Frames::new(record("Desk", true, true), Reach::Online);
    let options = host_options(&f.hosts.list[0]);
    assert_eq!(&options[options.len() - 2..], [HostOption::Unpair, HostOption::Remove]);
    assert!(options[..options.len() - 2].iter().all(|o| !o.destructive()));
    f.press(Key::F2);
    let mut colours = Vec::new();
    for clipped in &f.shapes {
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, Color32)>) {
            match shape {
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
                egui::Shape::Text(t) => out.push((t.galley.text().to_owned(), t.fallback_color)),
                _ => {}
            }
        }
        walk(&clipped.shape, &mut colours);
    }
    for (name, red) in [("Remove PC", true), ("Unpair", true), ("Rename", false)] {
        let colour = colours.iter().find(|(t, _)| t == name).map(|c| c.1).expect(name);
        assert_eq!(colour == *ERROR, red, "{name}");
    }
}

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

/// On a PC's library page, F2 (which the controller's X sends, see `main.rs`) or Shift+F10 on a
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
    let commands = f.frame(crate::gui::tap(Key::Enter).to_vec());
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
    let app = nightjar_core::http::App { uuid: "install-9".into(), ..game(9) };
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

/// Where a destination of the top bar is drawn.
fn destination_at(f: &Frames, name: &str) -> Rect {
    f.placed.iter().filter(|(t, ..)| t == name).map(|p| p.1).min_by(|a, b| a.top().total_cmp(&b.top())).expect(name)
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
fn leaving_a_files_page_by_the_top_bar_asks_while_a_transfer_runs() {
    let (downloads, here) = (files::tests::Folder::new("top-bar"), files::tests::Folder::new("top-bar-here"));
    let mut f = Frames::with(record("Desk", true, true), Reach::Online, Some(files::tests::native_answer()), Vec::new());
    let id = f.hosts.list[0].id;
    f.l.files.page = Some(files::tests::sending(id, &downloads, &here));
    f.l.open(Screen::Files(id));
    f.settle();
    assert!(f.l.files.transferring());
    f.click_at(destination_at(&f, "PCs").center(), egui::PointerButton::Primary);
    assert!(matches!(f.l.dialog, Dialog::ConfirmLeave { to: Screen::Hosts }), "PCs asks first");
    assert!(shows(&f, "Cancel the transfers?"), "{:?}", f.drawn);
    assert_eq!(f.l.screen, Screen::Files(id));
    // Stay has the focus, so A does not cancel anything.
    f.press(Key::Enter);
    assert!(f.no_dialog());
    assert_eq!(f.l.screen, Screen::Files(id));
    assert!(f.l.files.transferring(), "Stay keeps the upload");
    // Settings opens over the page: the upload goes on, and Back returns to it.
    f.click_at(destination_at(&f, "Settings").center(), egui::PointerButton::Primary);
    assert!(f.no_dialog() && f.l.screen == Screen::Settings, "Settings does not ask");
    assert!(f.l.files.transferring(), "the upload goes on under Settings");
    f.press(Key::Escape);
    assert_eq!(f.l.screen, Screen::Files(id));
    // Leave goes, and the upload is cancelled with the page.
    f.click_at(destination_at(&f, "PCs").center(), egui::PointerButton::Primary);
    let leave = f.placed.iter().find(|(t, ..)| t == "Leave").map(|p| p.1).expect("Leave");
    f.click_at(leave.center(), egui::PointerButton::Primary);
    assert!(f.no_dialog());
    assert_eq!(f.l.screen, Screen::Hosts);
    assert!(f.l.files.page.is_none(), "the page and its upload are gone");
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

/// Drops egui's focus, as a click on blank space or Escape does.
fn drop_focus(f: &mut Frames) {
    f.ctx.memory_mut(|m| if let Some(id) = m.focused() { m.surrender_focus(id) });
    f.settle();
    assert_eq!(f.ctx.memory(|m| m.focused()), None);
}

/// A controller press as `main.rs` hands it to egui.
fn pad_press(f: &mut Frames, key: Key) {
    let events = f.l.controller_key(&f.ctx, key);
    f.frame(events);
    f.settle();
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
fn b_where_back_ends_asks_to_close_nightjar_with_focus_on_cancel() {
    // The PCs are the root here (no Home): B there asks.
    let mut f = Frames::new(record("Desk", true, false), Reach::Online);
    let focused_text = |f: &Frames| {
        let rect = f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect);
        f.placed.iter().find(|(_, at, _)| rect.is_some_and(|r| r.contains(at.center()))).map(|p| p.0.clone())
    };
    // The keyboard's Escape does not ask: a keyboard has Alt+F4.
    assert!(f.press(Key::Escape).is_empty());
    assert!(f.no_dialog(), "Escape on the root stays harmless");
    pad_press(&mut f, Key::Escape);
    assert!(matches!(f.l.dialog, Dialog::ConfirmExit) && shows(&f, "Close Nightjar?"), "{:?}", f.drawn);
    assert_eq!(focused_text(&f).as_deref(), Some("Cancel"));
    // A right away keeps Nightjar open, and so does B.
    assert!(f.press(Key::Enter).iter().all(|c| !matches!(c, Command::Quit)));
    assert!(f.no_dialog());
    pad_press(&mut f, Key::Escape);
    assert!(f.press(Key::Escape).iter().all(|c| !matches!(c, Command::Quit)));
    assert!(f.no_dialog(), "B closes the question");
    // Over to Close Nightjar and A: it closes.
    pad_press(&mut f, Key::Escape);
    // Past Shut down, Restart and Sleep.
    for _ in 0..6 {
        if focused_text(&f).as_deref() != Some("Close Nightjar") { pad_press(&mut f, Key::ArrowRight); }
    }
    assert_eq!(focused_text(&f).as_deref(), Some("Close Nightjar"));
    assert!(f.press(Key::Enter).iter().any(|c| matches!(c, Command::Quit)));
}

#[test]
fn b_offers_this_pcs_power_each_asked_once_more_with_focus_on_cancel() {
    use crate::power::{windows_offers, PowerAction};
    let mut f = Frames::new(record("Desk", true, false), Reach::Online);
    f.l.power = windows_offers(true);
    let focused_text = |f: &Frames| {
        let rect = f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect);
        f.placed.iter().find(|(_, at, _)| rect.is_some_and(|r| r.contains(at.center()))).map(|p| p.0.clone())
    };
    // To `label` by the pad's right, from Cancel at the left.
    let walk_to = |f: &mut Frames, label: &str| {
        for _ in 0..6 {
            if focused_text(f).as_deref() == Some(label) { break; }
            pad_press(f, Key::ArrowRight);
        }
        assert_eq!(focused_text(f).as_deref(), Some(label), "{:?}", f.drawn);
    };
    pad_press(&mut f, Key::Escape);
    assert!(matches!(f.l.dialog, Dialog::ConfirmExit));
    for label in ["Shut down", "Restart", "Sleep", "Close Nightjar", "Cancel"] {
        assert!(shows(&f, label), "{label}: {:?}", f.drawn);
    }
    for (label, action) in [("Shut down", PowerAction::ShutDown), ("Restart", PowerAction::Restart), ("Sleep", PowerAction::Sleep)] {
        if !matches!(f.l.dialog, Dialog::ConfirmExit) { pad_press(&mut f, Key::Escape); }
        assert_eq!(focused_text(&f).as_deref(), Some("Cancel"));
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
    walk_to(&mut f, "Restart");
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

/// The record dialog takes SDL's keys, before the window's own F11 and F12: F9, Shift with a digit and
/// F11 arrive, where egui reports none of them.
#[test]
fn the_hotkey_dialog_records_the_keys_sdl_reports() {
    use crate::shortcuts::hotkey_bindings;
    use sdl3_sys::everything::*;
    let bound = |id: &str| hotkey_bindings().get(id).cloned();
    let mut l = launcher(false, false);
    l.go(Screen::StreamHotkeys);
    l.dialog = Dialog::StreamHotkey { id: "quit", error: None };
    assert!(l.recording_hotkey());
    assert!(l.hotkey_key_down(SDL_SCANCODE_LCTRL, SDL_KMOD_LCTRL, false).is_none() && l.recording_hotkey(), "Ctrl alone waits");
    assert!(matches!(l.hotkey_key_down(SDL_SCANCODE_F9, SDL_KMOD_LCTRL, false), Some(Command::SaveHotkeys)));
    assert!(!l.recording_hotkey());
    assert_eq!(bound("quit").as_deref(), Some("CTRL+F9"));
    // Shift+1 arrives, and is refused as a typing chord; with Ctrl it is saved.
    l.dialog = Dialog::StreamHotkey { id: "capture", error: None };
    assert!(l.hotkey_key_down(SDL_SCANCODE_1, SDL_KMOD_LSHIFT, false).is_none());
    assert!(matches!(&l.dialog, Dialog::StreamHotkey { error: Some(e), .. } if e == tr("This combination is for typing.")));
    assert!(l.hotkey_key_down(SDL_SCANCODE_1, SDL_Keymod(SDL_KMOD_LCTRL.0 | SDL_KMOD_LSHIFT.0), true).is_none(), "a repeat is no new press");
    assert!(l.hotkey_key_down(SDL_SCANCODE_1, SDL_Keymod(SDL_KMOD_LCTRL.0 | SDL_KMOD_LSHIFT.0), false).is_some());
    assert_eq!(bound("capture").as_deref(), Some("CTRL+SHIFT+1"));
    // F11 alone keeps Nightjar's prefix, and the window stays as it is.
    l.dialog = Dialog::StreamHotkey { id: "overlay", error: None };
    assert!(l.hotkey_key_down(SDL_SCANCODE_F11, SDL_KMOD_NONE, false).is_some());
    assert_eq!(bound("overlay").as_deref(), Some("CTRL+ALT+SHIFT+F11"));
    let main = include_str!("../main.rs");
    let at = |s: &str| main.find(s).unwrap_or_else(|| panic!("{s}"));
    assert!(at("SDL_EVENT_KEY_DOWN if self.launcher.recording_hotkey()") < at("== Some(Action::Fullscreen)"), "recorded before F11 and F12");
    // Escape alone cancels.
    l.dialog = Dialog::StreamHotkey { id: "minimize", error: None };
    assert!(l.hotkey_key_down(SDL_SCANCODE_ESCAPE, SDL_KMOD_NONE, false).is_none());
    assert!(!l.recording_hotkey() && bound("minimize").is_none());
    crate::shortcuts::set_hotkeys(Default::default());
}

/// As on Android, an arrow or Enter alone moves in the record dialog or confirms, and goes on to egui
/// instead of being saved; with a modifier it is recorded. Tab alone is recorded.
#[test]
fn arrows_and_enter_alone_move_in_the_hotkey_dialog() {
    use crate::shortcuts::hotkey_bindings;
    use sdl3_sys::everything::*;
    let bound = |id: &str| hotkey_bindings().get(id).cloned();
    let mut l = launcher(false, false);
    l.go(Screen::StreamHotkeys);
    l.dialog = Dialog::StreamHotkey { id: "capture", error: None };
    for key in [SDL_SCANCODE_UP, SDL_SCANCODE_DOWN, SDL_SCANCODE_LEFT, SDL_SCANCODE_RIGHT, SDL_SCANCODE_RETURN, SDL_SCANCODE_KP_ENTER] {
        assert!(moves_in_hotkey_dialog(key, SDL_KMOD_NONE), "{}", key.0);
        assert!(l.hotkey_key_down(key, SDL_KMOD_NONE, false).is_none(), "{}", key.0);
        assert!(matches!(&l.dialog, Dialog::StreamHotkey { error: None, .. }), "{}", key.0);
        assert!(bound("capture").is_none(), "{}", key.0);
    }
    let main = include_str!("../main.rs");
    assert!(main.contains("SDL_EVENT_KEY_DOWN if self.launcher.recording_hotkey() && !ui::moves_in_hotkey_dialog("), "egui gets them");
    assert!(!moves_in_hotkey_dialog(SDL_SCANCODE_TAB, SDL_KMOD_NONE));
    assert!(!moves_in_hotkey_dialog(SDL_SCANCODE_RETURN, SDL_KMOD_LCTRL));
    assert!(l.hotkey_key_down(SDL_SCANCODE_RETURN, SDL_Keymod(SDL_KMOD_LCTRL.0 | SDL_KMOD_LALT.0), false).is_some());
    assert_eq!(bound("capture").as_deref(), Some("CTRL+ALT+ENTER"));
    l.dialog = Dialog::StreamHotkey { id: "overlay", error: None };
    assert!(l.hotkey_key_down(SDL_SCANCODE_LEFT, SDL_Keymod(SDL_KMOD_LCTRL.0 | SDL_KMOD_LALT.0), false).is_some());
    assert_eq!(bound("overlay").as_deref(), Some("CTRL+ALT+DPAD_LEFT"));
    crate::shortcuts::set_hotkeys(Default::default());
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

fn opened_link(name: &str) -> rubylight_protocol::pairing_uri::PairingUri {
    rubylight_protocol::pairing_uri::PairingUri {
        host: "192.168.4.2".into(),
        port: 47989,
        otp: "4821".into(),
        passphrase: "hunter2".into(),
        unique_id: String::new(),
        cert_sha256: [7; 32],
        name: name.into(),
    }
}

fn pairs(commands: &[Command]) -> bool {
    commands.iter().any(|c| matches!(c, Command::PairLink(_)))
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
    let store = crate::pin_lock::PinStore::new(&dir, Arc::new(crate::pin_lock::SystemClock));
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

#[path = "settings_tests.rs"]
mod settings_tests;
