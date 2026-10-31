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
    let display = Display { width: 1920, height: 1080, refresh_hz: 60.0 };
    let mut launcher = Launcher::new(settings, Profiles::default(), crate::shortcuts::Shortcuts::default(), display);
    launcher.home_ready = home_ready;
    launcher
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
    l.back(&mut commands);
    assert_eq!(l.screen, Screen::Home);
    assert_eq!(l.restore_key.as_deref(), Some("game:steam:570"));
    assert!(l.focus_screen, "the first item takes focus if the remembered one is gone");
    // Home is the end of the line.
    l.back(&mut commands);
    assert_eq!(l.screen, Screen::Home);
}

#[test]
fn back_ends_on_home_when_it_is_on_and_on_the_pcs_when_it_is_not() {
    let mut on = launcher(true, true);
    on.go(Screen::Hosts);
    on.open(Screen::Apps(1));
    let mut commands = Vec::new();
    on.back(&mut commands);
    assert_eq!(on.screen, Screen::Hosts);
    on.back(&mut commands);
    assert_eq!(on.screen, Screen::Home, "the PCs list goes back to Home");

    let mut off = launcher(false, true);
    off.go(Screen::Hosts);
    off.open(Screen::Apps(1));
    off.back(&mut commands);
    off.back(&mut commands);
    assert_eq!(off.screen, Screen::Hosts, "Open on Home is off: the PCs are the root");

    let mut empty = launcher(true, false);
    empty.go(Screen::Hosts);
    empty.back(&mut commands);
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
    l.back(&mut commands);
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
        l.back(&mut commands);
        assert_eq!(l.screen, page, "B returns to where Start was pressed");
        assert_eq!(l.restore_key.as_deref(), Some("play"));
        l.back(&mut commands);
        assert_eq!(l.screen, Screen::Home, "and the page still goes back to the root");
    }
    // From a destination, Settings replaces it as before.
    let mut l = launcher(false, false);
    l.go(Screen::Hosts);
    l.open_settings();
    let mut commands = Vec::new();
    l.back(&mut commands);
    assert_eq!(l.screen, Screen::Hosts);
}

#[test]
fn a_game_page_for_another_copy_replaces_the_page() {
    let mut l = launcher(true, true);
    l.go(Screen::Home);
    l.open(Screen::Game { host: 1, app: 5 });
    l.go(Screen::Game { host: 2, app: 9 });
    let mut commands = Vec::new();
    l.back(&mut commands);
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
        let centre = (self.size / 2.0).to_pos2();
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
}

impl State {
    fn frames(&self) -> Frames {
        Frames::with(self.record.clone(), self.reach, self.info.clone(), self.apps.clone())
    }
}

fn every_state() -> Vec<State> {
    let state = |name, record, reach, info, apps| State { name, record, reach, info, apps };
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
    ]
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
fn the_destinations_read_home_pcs_settings() {
    let names: Vec<&str> = DESTINATIONS.iter().map(|(name, _)| *name).collect();
    assert_eq!(names, ["Home", "PCs", "Settings"]);
    let screens: Vec<Screen> = DESTINATIONS.iter().map(|(_, screen)| *screen).collect();
    assert_eq!(screens, [Screen::Home, Screen::Hosts, Screen::Settings]);
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
    l.back(&mut commands);
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
    l.back(&mut commands);
    assert_eq!(l.screen, Screen::Settings);
    assert_eq!(l.restore_key.as_deref(), Some("profiles-link"), "the row that opened it takes focus");
    assert_eq!(l.trail.len(), 1, "Settings still goes back to Home in one step");
    l.open(Screen::Shortcuts);
    l.back(&mut commands);
    l.back(&mut commands);
    assert_eq!(l.screen, Screen::Home);
}

#[test]
fn settings_sections_follow_androids_order() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 12000.0);
    f.l.go(Screen::Settings);
    f.settle();
    // The chips come first, then the page's headings in capitals.
    let chips: Vec<&str> = f.drawn.iter().map(String::as_str).filter(|t| SETTINGS_SECTIONS.contains(t)).collect();
    assert_eq!(chips, SETTINGS_SECTIONS, "the chips list the sections in order");
    let upper: Vec<String> = SETTINGS_SECTIONS.iter().map(|s| s.to_uppercase()).collect();
    let headings: Vec<&String> = f.drawn.iter().filter(|t| upper.contains(t)).collect();
    assert_eq!(headings, upper.iter().collect::<Vec<_>>(), "and the page has them in the same order");
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
        for section in SETTINGS_SECTIONS {
            f.l.unfolded.insert(section);
        }
        f.settle();
        assert!(f.access.as_ref().is_some_and(|t| t.nodes.len() > 5), "{name}: a tree came out");
        missing.extend(unnamed(&f).into_iter().map(|w| format!("{name}: {w}")));
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
    assert!(!main_row("tell_host_wireless") && !main_row("client_hint_bitrate") && !main_row("ui_zoom"));
}

#[test]
fn settings_fold_rows_behind_more_and_show_them_when_opened() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 12000.0);
    f.l.go(Screen::Settings);
    f.settle();
    assert!(shows(&f, "Device controls bitrate") && shows(&f, "Recover interrupted streams"), "main rows show: {:?}", f.drawn);
    assert!(!shows(&f, "Tell the PC when on Wi-Fi") && !shows(&f, "Loss-based bitrate (Rust engine)") && !shows(&f, "Diagnostics"), "folded rows wait");
    let more = f.placed.iter().find(|(t, _, _)| t == "More Advanced settings (5)").map(|p| p.1.center()).expect("Advanced folds five rows");
    f.click_at(more, egui::PointerButton::Primary);
    for text in ["Loss-based bitrate (Rust engine)", "Full chroma", "Diagnostics", "Tell the PC when on Wi-Fi", "Link test before the first bitrate", "Fewer Advanced settings"] {
        assert!(shows(&f, text), "{text} shows once Advanced opens");
    }
    assert!(!shows(&f, "Client roaming"), "Streaming stays folded");
    // A PC's override of a folded row keeps it in view while the section is folded.
    f.l.unfolded.clear();
    let id = f.hosts.list[0].id;
    f.l.go(Screen::HostSettings(id));
    f.l.edit_overrides.capture(&Settings { client_roaming: false, ..Settings::default() }, &["client_roaming"]);
    f.settle();
    assert!(shows(&f, "Client roaming") && !shows(&f, "Instant mouse and pen input"), "{:?}", f.drawn);
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
    f.l.unfolded.insert("Display & image");
    f.l.go(Screen::Settings);
    f.settle();
    assert!(shows(&f, "PC render resolution"), "{:?}", f.drawn);
    assert!(shows(&f, "Percent of the stream resolution that the PC renders at. The size of the transmitted video does not change. To send fewer pixels and upscale on this device, lower Resolution."));
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
        f.l.unfolded.insert("Display & image");
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
        if whole_height {
            assert!(rect.top() >= -1.0 && rect.bottom() <= screen.bottom() + 1.0, "{what}: {text:?} leaves the window: {rect:?} in {screen:?}");
        }
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
            Dialog::PairError { host: id, body: "PIN rejected. Generate a new one-time PIN in your host's web UI and check the passphrase.".into(), one_time: true },
            Dialog::Message { title: "Network test failed".into(), body: "The PC did not answer.".into() },
            Dialog::ConfirmSleep { host: id, ask: 1, name: "A long name for a gaming PC".into(), offer: None },
            Dialog::ConfirmSleep { host: id, ask: 1, name: "A long name for a gaming PC".into(), offer: Some(("t".into(), Handed::now())) },
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
        assert!(clip.height() < screen.height() - 100.0 && screen.expand(1.0).contains_rect(clip), "the list scrolls inside the window: {clip:?}");
        assert!(clip.expand(1.0).contains_rect(rect), "the option with focus is in the list's visible area: {rect:?} in {clip:?}");
    }
    f.press(Key::Enter);
    assert!(matches!(f.l.dialog, Dialog::ConfirmRemove { .. }), "the last option is Remove");
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
    }
    for dialog in [
        Dialog::ProfileName { id: None, name: String::new(), settings: Default::default(), error: None },
        Dialog::Shortcut { id: None, name: String::new(), chord: String::new(), error: Some("Use between 1 and 8 keys in each chord.".into()) },
        Dialog::DeleteProfile { id: 1, name: "Travel".into() },
        Dialog::DeleteShortcut { id: "a".into(), name: "Task manager".into() },
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
    f.l.unfolded.extend(SETTINGS_SECTIONS);
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
        let reachable = options.iter().filter(|o| **o != HostOption::Sleep || host.sleep_option() == Some(Ok(())));
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
                HostOption::Files => said(|c| matches!(c, Command::OpenFiles(_))) && f.no_dialog(),
                HostOption::Settings => said(|c| matches!(c, Command::HostSettings(_))),
                HostOption::Details => matches!(f.l.screen, Screen::HostDetails(_)) && f.no_dialog(),
                HostOption::CheckAgain => said(|c| matches!(c, Command::Refresh(_))),
                HostOption::Rename => matches!(f.l.dialog, Dialog::Rename { .. }),
                HostOption::Unpair => matches!(f.l.dialog, Dialog::ConfirmUnpair { .. }),
                HostOption::Remove => matches!(f.l.dialog, Dialog::ConfirmRemove { .. }),
                HostOption::PairOtp => matches!(f.l.dialog, Dialog::Otp { .. }),
                HostOption::QuitApp => matches!(f.l.dialog, Dialog::ConfirmQuit { .. }),
            };
            assert!(ok, "{state}: {} by keys (entry {n})", option.label());
        }
    }
    use HostOption::*;
    for option in [OpenLibrary, Resume, QuitApp, Pair, PairOtp, Wake, Sleep, Files, Settings, Details, Rename, Unpair, PairingHelp, CheckAgain, Remove] {
        assert!(seen.contains(&option), "no state reaches {}", option.label());
    }
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
fn the_details_page_shows_shared_folders_and_co_op_once_the_native_read_offers_them() {
    use crate::hosts::tests::{info_lands, native_caps, native_info, native_lands, stock_info, NATIVE_EXTRAS};
    let rows = |f: &Frames| [shows(f, "SHARED FOLDERS") && shows(f, "Browse"), shows(f, "CO-OP") && shows(f, "Show sessions")];
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
    assert!(!shows(&f, "CO-OP"), "a PC that has not said it offers co-op shows none: {:?}", f.drawn);
    let offer = |f: &mut Frames, coop_offered: bool| {
        let info = f.hosts.list[0].info.as_mut().unwrap();
        let evidence = info.capabilities.evidence().unwrap().clone();
        info.capabilities = nightjar_core::http::HostCapabilities::from_evidence(nightjar_core::http::Evidence { native_capabilities: true, coop_offered, ..evidence });
        f.settle();
    };
    offer(&mut f, false);
    assert!(!shows(&f, "CO-OP"), "a PC without Nucleus Co-op shows none");
    offer(&mut f, true);
    assert!(shows(&f, "CO-OP") && shows(&f, "Show sessions"), "{:?}", f.drawn);
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
    assert!(host_options(&f.hosts.list[0]).contains(&HostOption::Sleep));
    f.press(Key::F2);
    assert!(f.drawn.iter().any(|t| t == crate::hosts::SLEEP_NOT_ALLOWED), "the reason is shown");
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
    // PCs: Home hides while there is no library, Add PC follows Settings.
    let mut f = Frames::new(record("Paired", true, false), Reach::Online);
    let stops = header_stops(&mut f);
    assert_eq!(stops.len(), 3, "PCs, Settings and Add PC: {stops:?}");
    assert!(stops.windows(2).all(|w| w[0].right() < w[1].left()), "in reading order: {stops:?}");
    assert!(1280.0 - stops[2].right() < 80.0, "Add PC is at the right edge: {:?}", stops[2]);

    // Home shows all three destinations in order.
    let mut f = Frames::new(record("Paired", true, false), Reach::Online);
    f.l.go(Screen::Home);
    f.settle();
    let stops = header_stops(&mut f);
    assert_eq!(stops.len(), 3, "Home, PCs and Settings: {stops:?}");
    assert!(stops.windows(2).all(|w| w[0].right() < w[1].left()), "in reading order: {stops:?}");
    assert!(1280.0 - stops[2].right() < 80.0, "Settings is at the right edge");
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
        assert!(rect.top() >= 0.0 && rect.bottom() <= 720.0, "row {step}: the focused game is on screen: {rect:?}");
    }
    assert!(seen.len() >= 4, "the focus walked down past the first screen: {} rows", seen.len());
    for step in 0..seen.len() {
        f.press(Key::ArrowUp);
        scrolled(&mut f);
        let (_, rect) = focused(&f);
        assert!(rect.top() >= 0.0 && rect.bottom() <= 720.0, "back up {step}: on screen: {rect:?}");
    }
    assert_eq!(focused(&f).0, start, "back at the first game");
}

/// A cover with art names its game under it only while it has the focus (the art carries the
/// title already); a cover without art shows the whole title inside it, once.
#[test]
fn covers_name_the_game_inside_without_art_and_under_art_only_when_focused() {
    let mut f = home_with_games(3);
    let count = |f: &Frames, title: &str| f.drawn.iter().filter(|t| *t == title).count();
    assert_eq!((count(&f, "Game 00"), count(&f, "Game 01")), (1, 1), "without art each title shows once: {:?}", f.drawn);
    let texture = f.ctx.load_texture("art", egui::ColorImage::new([3, 4], vec![Color32::GRAY; 12]), egui::TextureOptions::LINEAR);
    let pc = &mut f.hosts.list[0];
    for g in &mut pc.native.library.as_mut().expect("a library").games {
        g.artwork = nightjar_core::http::native_api::library::Artwork { cover: Some(format!("/cover/{}", g.app_id)), version: "1".into() };
        pc.art.insert(g.app_id as i32, Art::Ready(texture.clone()));
    }
    f.settle();
    assert_eq!((count(&f, "Game 00"), count(&f, "Game 01")), (1, 0), "only the focused cover is named: {:?}", f.drawn);
    f.press(Key::ArrowRight);
    f.settle();
    assert_eq!((count(&f, "Game 00"), count(&f, "Game 01")), (0, 1), "the name follows the focus: {:?}", f.drawn);
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
        ("Close game", Dialog::ConfirmClose { host: id, name: "Game".into() }),
    ];
    let buttons = ["Save", "Remove PC", "Unpair", "Add", "Close game", "Paste link", "Cancel"];
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
    f.size = vec2(1920.0, 1080.0);
    f.l.go(Screen::Settings);
    f.settle();
    let top = |f: &Frames| f.placed.iter().find(|(t, ..)| t.starts_with("More Video settings")).map(|p| p.1.top()).expect("the fold row");
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

/// A controller's mapping scrolls in the same column as settings: the wheel beside it scrolls it.
#[test]
fn controller_mappings_scroll_beside_their_column() {
    let mut f = Frames::new(record("Desk", true, true), Reach::Online);
    f.size = vec2(1920.0, 1080.0);
    f.l.go(Screen::Controllers);
    f.l.controller_key = "pad".into();
    f.l.controller_mapping = crate::gamepad::Mapping { name: "Pad".into(), ..Default::default() };
    f.settle();
    let top = |f: &Frames| f.placed.iter().find(|(t, ..)| t == "AXES").map(|p| p.1.top()).expect("the axes under Connect as");
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

/// "More … settings" is a row like the settings it folds, opened from anywhere on it.
#[test]
fn the_fold_is_a_row_that_opens_anywhere_on_it() {
    let mut f = Frames::new(record("Desk", true, true), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    f.l.go(Screen::Settings);
    f.settle();
    let label = f.placed.iter().find(|(t, ..)| t.starts_with("More Video settings")).map(|p| p.1).expect("Video folds rows");
    let resolution = f.placed.iter().find(|(t, ..)| t == "Resolution").map(|p| p.1).expect("the first row");
    // Its right end, where the chevron is, far from the label's text.
    let at = Pos2::new(resolution.left() + SETTINGS_W - 40.0, label.center().y);
    f.click_at(at, egui::PointerButton::Primary);
    assert!(shows(&f, "Fewer Video settings"), "{:?}", f.drawn);
}

/// Home's PC card says a PC's state once: an offline PC reads "Offline", never "Offline · Offline".
#[test]
fn the_home_pc_card_says_the_state_once() {
    let mut f = home_with_games(1);
    f.hosts.list[0].reach = Reach::Offline;
    f.settle();
    let lines: Vec<&String> = f.drawn.iter().filter(|t| t.contains("Offline")).collect();
    assert!(!lines.is_empty() && lines.iter().all(|t| t.matches("Offline").count() == 1), "{lines:?} in {:?}", f.drawn);
    assert!(shows(&f, "PCS"), "the section is Android's \"PCs\": {:?}", f.drawn);
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
    let at = |f: &Frames, text: &str| f.placed.iter().rfind(|(t, ..)| t == text).map(|p| p.1).unwrap_or_else(|| panic!("{text} in {:?}", f.drawn));
    let (chip, title, line) = (at(&f, "Running"), at(&f, "Game 00"), at(&f, "Desk"));
    assert!(chip.bottom() <= title.top() && title.bottom() <= line.top(), "chip, title, PC: {chip:?} {title:?} {line:?}");
    assert!(!f.drawn.iter().any(|t| t.starts_with("Running on")), "{:?}", f.drawn);
    f.l.go(Screen::Game { host: pc, app: 1001 });
    f.settle();
    assert!(shows(&f, "Desk") && !shows(&f, "Running") && !shows(&f, "Suspended"), "{:?}", f.drawn);
}

/// Settings is a top-level page like Home and PCs: no Back in its header. Its sub-pages keep theirs.
#[test]
fn settings_has_no_back_but_its_sub_pages_do() {
    let mut f = Frames::new(record("Desk", true, true), Reach::Online);
    f.l.go(Screen::Settings);
    f.settle();
    assert!(shows(&f, "Settings") && !f.drawn.iter().any(|t| t.starts_with('‹')), "{:?}", f.drawn);
    f.l.open(Screen::Profiles);
    f.settle();
    assert!(shows(&f, "‹  Settings"), "{:?}", f.drawn);
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
#[test]
fn no_pcs_yet_links_how_to_connect_on_home_and_pcs() {
    for screen in [Screen::Home, Screen::Hosts] {
        crate::theme::own_palette();
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut l = launcher(true, false);
        l.identity_ready = true;
        let mut f = Frames::over(ctx, l, Hosts::without_discovery(Vec::new(), Arc::new(|| {})));
        f.l.go(screen);
        f.settle();
        let at = |f: &Frames, text: &str| f.placed.iter().find(|(t, ..)| t == text).map(|p| p.1).unwrap_or_else(|| panic!("{screen:?}: {text} in {:?}", f.drawn));
        let main = at(&f, if screen == Screen::Home { "Open PCs" } else { "Add a PC by address" });
        let link = at(&f, "How to connect");
        assert!(link.top() >= main.bottom(), "{screen:?}: the link under the main action: {link:?} {main:?}");
        f.frame(vec![egui::Event::PointerMoved(link.center())]);
        let commands = f.click_at(link.center(), egui::PointerButton::Primary);
        assert!(commands.iter().any(|c| matches!(c, Command::OpenGuide("pairing.md"))), "{screen:?}: a click opens the guide");
    }
}

/// Home before any PC speaks of PCs, as Android's Home does, and never of hosts.
#[test]
fn home_without_pcs_speaks_of_pcs() {
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    let mut l = launcher(true, false);
    l.identity_ready = true;
    let mut f = Frames::over(ctx, l, Hosts::without_discovery(Vec::new(), Arc::new(|| {})));
    f.l.go(Screen::Home);
    f.settle();
    assert!(shows(&f, "No PCs yet") && shows(&f, "Open PCs"), "{:?}", f.drawn);
    assert!(!f.drawn.iter().any(|t| t.to_lowercase().contains("host")), "{:?}", f.drawn);
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
        .find(|(_, n)| n.label().is_some_and(|l| l.starts_with("Quit the game when I leave")))
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

#[test]
fn the_running_badge_is_a_green_dot_and_white_words_on_the_scrim_that_fit() {
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    let mut badge = Rect::NOTHING;
    let mut words = 0.0;
    let mut output = egui::FullOutput::default();
    for n in 0..2 {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0))), time: Some(f64::from(n)), ..Default::default() };
        output = ctx.run_ui(input, |ui| {
            badge = running_badge(ui.painter(), pos2(20.0, 20.0));
            words = ui.painter().layout_no_wrap(tr("Running").to_owned(), semibold(text::CAPTION), Color32::WHITE).size().x;
        });
        output.textures_delta.clear();
    }
    assert!(badge.width() >= words + space::S * 3.0, "the badge is {} wide for words {words} wide", badge.width());
    assert_eq!(badge.height(), space::XL);
    let mut fills = Vec::new();
    for clipped in &output.shapes {
        match &clipped.shape {
            egui::Shape::Rect(r) => fills.push(r.fill),
            egui::Shape::Circle(c) => fills.push(c.fill),
            _ => {}
        }
    }
    assert!(fills.contains(&SCRIM), "the badge sits on the scrim: {fills:?}");
    assert!(fills.contains(&*OK), "with a green dot: {fills:?}");
    assert!(!fills.contains(&*MOON), "and no amber fill: {fills:?}");
    assert!(collect_all_text(&output.shapes).iter().any(|t| t == tr("Running")));
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

/// Every text drawn in `shapes`.
fn collect_all_text(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
    let mut drawn = Vec::new();
    for clipped in shapes {
        collect_text(&clipped.shape, &mut drawn);
    }
    drawn
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

/// A PC with a long IPv6 address and GPU name; `zoomed` is the smallest window SDL allows at 150 %, in German.
fn long_detail_card(zoomed: bool) -> (Frames, String) {
    let address = "fd12:3456:789a:bcde:f012:3456:789a:bcde";
    let mut info = answer(true, 0);
    info.gpu_type = Some("AMD Radeon RX 7900 XTX Phantom Gaming OC".into());
    let mut f = Frames::with(HostRecord { address: address.into(), ..record("Living Room PC", true, true) }, Reach::Online, Some(info), Vec::new());
    if zoomed {
        fit_150(&mut f);
        f.size = vec2(720.0 / 1.5, 480.0 / 1.5);
    }
    f.settle();
    (f, format!("{address}  ·  AMD Radeon RX 7900 XTX Phantom Gaming OC"))
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

#[test]
fn a_long_address_and_gpu_end_in_an_ellipsis_inside_the_card_and_the_tooltip_shows_them_whole() {
    for zoomed in [false, true] {
        let (mut f, full) = long_detail_card(zoomed);
        let (card, _) = f.card_and_dots();
        assert!(card.right() <= f.size.x, "{zoomed}: the card {card:?} fits the window");
        let (line, rect) = visible(&f).into_iter().find(|(t, _)| t.starts_with("fd12:")).expect("the address line is drawn");
        assert!(line.ends_with('…') && line.chars().count() < full.chars().count(), "{zoomed}: the line is cut: {line}");
        assert!(rect.right() <= card.right() - CARD_PAD + 0.5, "{zoomed}: {line} ends at {} in a card ending at {}", rect.right(), card.right());
        assert!(rect.left() >= card.left(), "{zoomed}: the line starts inside the card");
        // The state line above it still fits whole.
        let status = host_status(&f.hosts.list[0]).0;
        assert!(visible(&f).iter().any(|(t, _)| *t == status), "{zoomed}: {status} is drawn whole");
        hover_card(&mut f, card);
        assert!(visible(&f).iter().any(|(t, _)| *t == full), "{zoomed}: the tooltip shows the whole line: {:?}", visible(&f));
    }
}

#[test]
fn a_short_detail_line_is_drawn_whole_with_no_tooltip() {
    let (mut f, _) = long_detail_card(false);
    f.hosts.list[0].record.address = "192.0.2.7".into();
    f.hosts.list[0].info.as_mut().expect("the answer").gpu_type = Some("RX 7900 XT".into());
    f.settle();
    let (card, _) = f.card_and_dots();
    hover_card(&mut f, card);
    let shown = visible(&f);
    let lines: Vec<_> = shown.iter().filter(|(t, _)| t == "192.0.2.7  ·  RX 7900 XT").collect();
    assert_eq!(lines.len(), 1, "drawn once, whole, and no tooltip: {shown:?}");
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

/// Made-up cover art without any words on it: a night gradient in one hue with the brand's orbits.
/// The demo library's cover art (the same genre covers the Android demo and README use).
fn demo_art(ctx: &egui::Context, n: usize) -> Art {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../app/src/debug/assets/demo/art/{}.png", n + 1));
    let image = image::load_from_memory(&std::fs::read(&path).expect("the demo cover")).expect("a PNG").to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Art::Ready(ctx.load_texture(format!("demo-art-{n}"), egui::ColorImage::from_rgba_unmultiplied(size, &image), egui::TextureOptions::LINEAR))
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
    let titles = [
        ("Desktop", ""), ("Racing Game", "Steam"), ("Open World", "Xbox"), ("Space Sim", "Epic Games"), ("Puzzle Night", "GOG"),
        ("Retro Arcade", "Steam"), ("Strategy", "Battle.net"), ("Music Studio", "Steam"), ("Ocean Explorer", "Xbox"), ("Skybound", ""),
    ];
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
                game_id: (!store.is_empty()).then(|| format!("steam:{n}")),
                store: (!store.is_empty()).then(|| Store { source: "steam".into(), id: n.to_string(), name: (*store).into() }),
                installed: Some(true),
                running: n == 1,
                artwork: Artwork { cover: (n < 9).then(|| format!("/cover/{n}")), version: "1".into() },
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
            for (n, app) in apps.iter().enumerate().take(9) {
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
        f.l.go(Screen::Game { host: pc, app: 1001 });
        shoot(&mut f, "game-running");
        f.l.go(Screen::Game { host: pc, app: 1002 });
        shoot(&mut f, "game");
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
    }
}
