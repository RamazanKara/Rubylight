//! Page, store, demo, neural, link, backdrop, and game-facts fixtures for launcher UI tests.

use super::*;

/// A paired PC whose library holds `count` installed games with distinct titles and ids.
pub(super) fn home_with_games(count: u32) -> Frames {
    use rubylight_native_api::library::{Artwork, Game, Library};
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
            last_played: None,
            playtime_minutes: None,
            collections: Vec::new(),
        })
        .collect();
    let mut f = Frames::with(record("Desk", true, true), Reach::Online, Some(answer(true, 0)), Vec::new());
    f.hosts.list[0].native.library = Some(Arc::new(Library { revision: "r1".into(), games }));
    f.l.go(Screen::Home);
    f.settle();
    f
}

/// A game's page on a PC, with profiles named `names` (ids from 1).
pub(super) fn game_page_with_profiles(names: &[&str]) -> Frames {
    let mut f = home_with_games(1);
    let pc = f.hosts.list[0].id;
    f.hosts.list[0].apps = Apps::Loaded(Arc::new(vec![nightjar_policy::http::App { name: "Game 00".into(), uuid: "install-0".into(), ..game(1000) }]));
    f.l.profiles.entries = names.iter().zip(1u64..).map(|(name, id)| crate::config::Profile::new(id, *name, Default::default())).collect();
    f.l.go(Screen::Game { host: pc, app: 1000 });
    f.settle();
    f
}

/// The last `text` drawn below the chrome: a popup's rows come after the page's.
pub(super) fn last_text_at(f: &Frames, text: &str) -> egui::Pos2 {
    f.placed.iter().rfind(|(t, r, _)| t == text && r.top() > chrome::PAGE_TOP).map(|p| p.1.center()).unwrap_or_else(|| panic!("{text} in {:?}", f.drawn))
}

/// A PC whose game's frame generation answers `state` to every read and write.
pub(super) struct FixedNeural(pub(super) std::sync::Mutex<nightjar_policy::http::Neural>);

impl crate::neural_page::NeuralSource for FixedNeural {
    fn neural(&self, _: &str) -> Result<nightjar_policy::http::Neural, nightjar_policy::http::Error> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn write(&self, _: &nightjar_policy::http::NeuralWrite) -> Result<nightjar_policy::http::Neural, nightjar_policy::http::Error> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn cancel(&self) {}
}

/// The game's page on a PC that lists frame generation, its panel open with `on` and `reason`.
pub(super) fn game_page_with_neural(on: bool, state: rubylight_native_api::radeon::ControlState, reason: Option<&str>) -> Frames {
    let mut f = home_with_games(1);
    let pc = f.hosts.list[0].id;
    f.hosts.list[0].apps = Apps::Loaded(Arc::new(vec![nightjar_policy::http::App { name: "Game 00".into(), uuid: "install-0".into(), ..game(1000) }]));
    f.hosts.list[0].info = Some(answer(true, 0));
    let info = f.hosts.list[0].info.as_mut().unwrap();
    let evidence = info.capabilities.evidence().unwrap().clone();
    info.capabilities = nightjar_policy::http::HostCapabilities::from_evidence(nightjar_policy::http::Evidence { paired: true, view_permission: true, native_api_version: 1, native_capabilities: true, neural_offered: true, ..evidence });
    let neural = nightjar_policy::http::Neural {
        app: "install-0".into(),
        accepted: true,
        gpu: Some("AMD Radeon 780M Graphics".into()),
        gpu_note: None,
        switches: vec![nightjar_policy::http::NeuralSwitch { id: nightjar_policy::http::FRAME_GENERATION.into(), state, on, reason: reason.map(str::to_owned), why: None }],
        replaces_earlier_upscaling: false,
        job: None,
        mode: None,
        release: None,
    };
    f.l.go(Screen::Game { host: pc, app: 1000 });
    f.l.neural.open(pc, 1000, "install-0".into(), std::sync::Arc::new(FixedNeural(std::sync::Mutex::new(neural))), std::sync::Arc::new(|| {}));
    for _ in 0..400 {
        f.l.neural.poll(Some((pc, 1000)));
        if f.l.neural.state_for(pc, 1000).is_some_and(|open| open.state.is_some()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    f.size = vec2(1280.0, 1000.0);
    f.settle();
    f.settle();
    f
}

/// Where the round Back shows, on a page that has one.
pub(super) fn back_rect(f: &Frames) -> Option<Rect> {
    f.ctx.read_response(Id::unique("back")).map(|r| r.rect)
}

/// Before any PC, Home and PCs both link How to connect under their main action, as Android's
/// empty discovery card does, and it opens the pairing guide.
/// Frames over a list with no PCs, on a network being searched.
pub(super) fn first_run_frames(records: Vec<crate::config::HostRecord>) -> Frames {
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    let mut l = launcher(true, false);
    l.identity_ready = true;
    let mut hosts = Hosts::without_discovery(records, Arc::new(|| {}));
    hosts.discovering = true;
    Frames::over(ctx, l, hosts)
}

/// The colours a galley of `text` is drawn in.
pub(super) fn galley_inks(shape: &egui::Shape, text: &str, out: &mut Vec<Color32>) {
    match shape {
        egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| galley_inks(s, text, out)),
        egui::Shape::Text(t) if t.galley.text() == text => out.push(t.fallback_color),
        _ => {}
    }
}

/// Home on its own, gathered as the page does.
pub(super) fn draw_home(l: &mut Launcher, ui: &mut Ui, view: &View<'_>, commands: &mut Vec<Command>) {
    let sources: Vec<Source> = view.hosts.list.iter().map(Source::of).collect();
    let data = home::HomeData::gather(l, &sources);
    l.home(ui, view, &sources, data, commands);
    glass::flush_deferred(ui);
}

/// The outlines of the glass meshes that hold `color`: glass paints meshes, not filled rects.
pub(super) fn meshes_holding(shapes: &[egui::epaint::ClippedShape], color: Color32) -> Vec<Rect> {
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
pub(super) fn collect_visible(shape: &egui::Shape, out: &mut Vec<(String, Rect)>) {
    match shape {
        egui::Shape::Text(text) => {
            let shown: String = text.galley.rows.iter().map(|row| row.text()).collect();
            out.push((shown, text.galley.rect.translate(text.pos.to_vec2())));
        }
        egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect_visible(s, out)),
        _ => {}
    }
}

pub(super) fn visible(f: &Frames) -> Vec<(String, Rect)> {
    let mut shown = Vec::new();
    for clipped in &f.shapes {
        collect_visible(&clipped.shape, &mut shown);
    }
    shown
}

pub(super) fn hover_card(f: &mut Frames, card: Rect) {
    f.frame(vec![egui::Event::PointerMoved(card.center())]);
    for _ in 0..30 {
        f.frame(Vec::new());
    }
}

/// The card of the only PC, by its own id.
pub(super) fn pc_card_rect(f: &Frames) -> Rect {
    let id = f.hosts.list[0].id;
    f.ctx.read_response(Id::unique(("pc-card", id))).expect("the card").rect
}

/// A PC's page with `count` apps, some running.
pub(super) fn app_grid(count: i32, running: i32) -> Frames {
    let games: Vec<_> = (1..=count).map(|id| nightjar_policy::http::App { name: format!("App {id}"), ..game(id) }).collect();
    let mut f = Frames::with(record("Den", true, true), Reach::Online, Some(answer(true, running)), games);
    let id = f.hosts.list[0].id;
    f.size = vec2(1280.0, 800.0);
    f.l.go(Screen::Apps(id));
    f.settle();
    f
}

/// The top bar over Den's page, with Attic not paired and Cellar asleep. Ids in that order.
pub(super) fn three_pcs() -> (Frames, [u64; 3]) {
    let mut attic = record("Attic", false, false);
    attic.address = "192.0.2.77".into();
    let mut cellar = record("Cellar", true, true);
    cellar.address = "192.0.2.78".into();
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    let mut l = launcher(false, false);
    l.go(Screen::Hosts);
    let mut hosts = Hosts::without_discovery(vec![record("Den", true, true), attic, cellar], Arc::new(|| {}));
    for (host, reach) in hosts.list.iter_mut().zip([Reach::Online, Reach::Online, Reach::Offline]) {
        host.reach = reach;
    }
    hosts.list[0].apps = Apps::Loaded(Arc::new(vec![game(1)]));
    let ids = [hosts.list[0].id, hosts.list[1].id, hosts.list[2].id];
    let mut f = Frames::over(ctx, l, hosts);
    f.l.go(Screen::Apps(ids[0]));
    f.settle();
    (f, ids)
}

/// Where the bar's capsule is, by the name in it.
pub(super) fn capsule_at(f: &Frames, name: &str) -> Pos2 {
    f.placed.iter().find(|(t, r, _)| t == name && r.top() < 68.0).expect("the capsule names its PC").1.center()
}

pub(super) fn focus_capsule(f: &mut Frames) {
    f.ctx.memory_mut(|m| m.request_focus(Id::unique("top-pc")));
    f.settle();
}

/// The name in the row that has the focus, if a row of the sheet has it.
pub(super) fn focused_row(f: &Frames) -> Option<String> {
    let rect = f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id))?.rect;
    f.placed.iter().find(|(t, r, _)| rect.height() == 60.0 && rect.contains_rect(*r) && ["Den", "Attic", "Cellar"].contains(&t.as_str())).map(|(t, ..)| t.clone())
}

/// The demo library's cover art (the same covers the Android demo and README use).
pub(super) fn demo_art(ctx: &egui::Context, n: usize) -> Art {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../app/src/debug/assets/demo/art/{}.jpg", n + 1));
    let image = image::load_from_memory(&std::fs::read(&path).expect("the demo cover")).expect("a JPEG").to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Art::Ready(ctx.load_texture(format!("demo-art-{n}"), egui::ColorImage::from_rgba_unmultiplied(size, &image), egui::TextureOptions::LINEAR))
}

/// The showcase library. Store keys as a host sends them (console.rs names them). Ten Steam games
/// overflow a row at both sizes, so Steam's ends in its All tile; one Epic game is not installed.
pub(super) const POLISH_GAMES: [(&str, &str); 19] = [
    ("Desktop", ""), ("Racing Game", "steam"), ("Open World", "steam"), ("Space Sim", "microsoft"), ("Puzzle Night", "gog"),
    ("Retro Arcade", "steam"), ("Strategy", "battlenet"), ("Music Studio", "steam"), ("Ocean Explorer", "steam"), ("Skybound", "steam"),
    ("Ashfall", "microsoft"), ("Brightwater", "steam"), ("Cinder Road", "steam"), ("Deep Field", "steam"), ("Ember Keep", "steam"),
    ("Frostline", "epic"), ("Glass Harbor", "battlenet"), ("Hollow Peak", "epic"), ("Iron Tide", ""),
];

/// Presses Down until focus lands on the widget drawn with `text` (a menu's value or item, a
/// field's number). False when it never does.
pub(super) fn focus_down_to(f: &mut Frames, text: &str) -> bool {
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
pub(super) fn pick_inherited_frame_rate(by_reader: bool) {
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

/// A paired PC whose library holds `games` (title, store key, installed), on Home.
pub(super) fn home_with_stores(games: &[(&str, &str, bool)]) -> Frames {
    use rubylight_native_api::library::{Artwork, Game, Library, Store};
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
            last_played: None,
            playtime_minutes: None,
            collections: Vec::new(),
        })
        .collect();
    let mut f = Frames::with(record("Desk", true, true), Reach::Online, Some(answer(true, 0)), Vec::new());
    f.hosts.list[0].native.library = Some(Arc::new(Library { revision: "r1".into(), games }));
    f.l.go(Screen::Home);
    f.settle();
    f
}

/// Twelve Steam games, two from Epic, Battle.net and Xbox each, one not installed, and an app of no store.
pub(super) fn store_mix() -> Frames {
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

pub(super) fn in_collections(f: &mut Frames, games: &[(&str, &[&str])]) {
    f.hosts.list[0].native.collections = Arc::new(games.iter().map(|(id, names)| ((*id).to_owned(), names.iter().map(|n| (*n).to_owned()).collect())).collect());
    f.settle();
}

/// The demo library in a window of this size, on Home.
pub(super) fn demo_frames(size: egui::Vec2) -> Frames {
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

pub(super) fn demo_page(f: &mut Frames, page: super::demo::Page) {
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

pub(super) fn wheel(f: &mut Frames, at: Pos2, points: f32) {
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

/// Where a destination of the top bar is drawn.
pub(super) fn destination_at(f: &Frames, name: &str) -> Rect {
    f.placed.iter().filter(|(t, ..)| t == name).map(|p| p.1).min_by(|a, b| a.top().total_cmp(&b.top())).expect(name)
}

/// Drops egui's focus, as a click on blank space or Escape does.
pub(super) fn drop_focus(f: &mut Frames) {
    f.ctx.memory_mut(|m| if let Some(id) = m.focused() { m.surrender_focus(id) });
    f.settle();
    assert_eq!(f.ctx.memory(|m| m.focused()), None);
}

/// A controller press as app/launcher.rs hands it to egui.
pub(super) fn pad_press(f: &mut Frames, key: Key) {
    let events = f.l.controller_key(&f.ctx, key);
    f.frame(events);
    f.settle();
}

/// A launcher on Settings → Latency benchmark, with this device's display and audio read.
pub(super) fn latency_benchmark_frames(high: bool) -> Frames {
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    let mut l = launcher(false, false);
    l.settings.high_contrast = high;
    l.go(Screen::LatencyBenchmark);
    l.latency_bench.device = Some(crate::latency_bench::Device { panel_hz: Some(143.998), audio: Some((48_000, 480)), gpu: None });
    let mut f = Frames::over(ctx, l, Hosts::without_discovery(Vec::new(), Arc::new(|| {})));
    f.size = vec2(1280.0, 800.0);
    f
}

pub(super) fn opened_link(name: &str) -> rubylight_protocol::pairing_uri::PairingUri {
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

pub(super) fn pairs(commands: &[Command]) -> bool {
    commands.iter().any(|c| matches!(c, Command::PairLink(_)))
}

pub(super) fn invite(name: &str, expires: u64) -> rubylight_protocol::invite_uri::InviteUri {
    rubylight_protocol::invite_uri::InviteUri {
        secret: [3; 32],
        unique_id: "HOST-1".into(),
        cert_sha256: [7; 32],
        name: name.into(),
        expires,
        permission: "controller".into(),
        ports: rubylight_protocol::invite_uri::InvitePorts {
            http: 47989,
            https: 47984,
            ..Default::default()
        },
        session: None,
        endpoints: Vec::new(),
        nat: String::new(),
        ipv6: Vec::new(),
        external_ipv4: None,
        mapped_ports: false,
    }
}

pub(super) fn redeems(commands: &[Command]) -> bool {
    commands
        .iter()
        .any(|c| matches!(c, Command::RedeemInvite(_)))
}

/// What a screen reader is told is on screen, with where.
pub(super) fn read_out(f: &Frames) -> Vec<(String, egui::accesskit::Rect)> {
    f.access.as_ref().unwrap().nodes.iter().filter_map(|(_, n)| Some((n.label().or(n.value())?.to_owned(), n.bounds()?))).collect()
}

/// The screen's share of the live chain's first region in the last frame.
pub(super) fn blurred_share(f: &Frames) -> f32 {
    let boundary = f.shapes.iter().find_map(|c| match &c.shape {
        egui::Shape::Callback(call) => (call.callback.as_ref() as &dyn std::any::Any).downcast_ref::<crate::live_glass::Boundary>(),
        _ => None,
    });
    let regions = &boundary.expect("the live tier paints a boundary").regions;
    regions[0].area() / (f.size.x * f.size.y)
}

/// Steps frames until the sky's fade ends, checking each against the one before: a moon that comes
/// starts faint, one that goes leaves faint, one that stays changes by a frame's step at most, and
/// the fade only runs forward.
pub(super) fn follow_the_fade(f: &mut Frames, what: &str, moves: bool) -> Vec<backdrop::Painted> {
    let mut last = backdrop::painted(&f.ctx);
    let mut frames = Vec::new();
    for n in 0..12 {
        f.frame(Vec::new());
        let now = backdrop::painted(&f.ctx);
        fade_step(&last, &now, what, n, moves);
        frames.push(now.clone());
        last = now;
    }
    assert!(!last.fading, "{what}: the fade ends");
    frames
}

/// One frame of [`follow_the_fade`]. A moon still fading that moves on its page (its first frame
/// laid out a little differently) goes on fading where it stands: one that goes and one that comes
/// alike are one moon. With `moves` a moon at full may move too: the window changed size.
pub(super) fn fade_step(last: &backdrop::Painted, now: &backdrop::Painted, what: &str, n: usize, moves: bool) {
    let step = 0.35;
    let mut gone: Vec<_> = last.moons.iter().filter(|(d, _)| !now.moons.iter().any(|(e, _)| e == d)).collect();
    for (disc, alpha) in &now.moons {
        match last.moons.iter().find(|(d, _)| d == disc) {
            Some((_, was)) => assert!((alpha - was).abs() <= step, "{what}, frame {n}: the moon at {disc:?} jumps from {was} to {alpha}: {last:?} then {now:?}"),
            None if *alpha <= step => {}
            None => {
                let moved = gone.iter().position(|(_, was)| (alpha - was).abs() <= step && (moves || *alpha < 1.0));
                let moved = moved.unwrap_or_else(|| panic!("{what}, frame {n}: a moon at {disc:?} comes in at {alpha}: {last:?} then {now:?}"));
                gone.remove(moved);
            }
        }
    }
    for (disc, was) in gone {
        assert!(*was <= step, "{what}, frame {n}: the moon at {disc:?} goes at {was}");
    }
    if last.fading && now.fading {
        assert!(now.along >= last.along, "{what}, frame {n}: the fade starts over: {last:?} then {now:?}");
    }
}

/// Asserts the moon this frame is whole in the window and clear of everything the page drew, and
/// says which kinds of shape it checked.
pub(super) fn assert_moon_clear(f: &Frames, what: &str) -> Vec<&'static str> {
    let Some((center, r)) = backdrop::disc_of(&f.ctx) else {
        return Vec::new();
    };
    let screen = f.ctx.content_rect();
    assert!(screen.contains_rect(Rect::from_center_size(center, Vec2::splat(r * 2.0))), "{what}: the moon at {center:?}, r {r}, is whole in the window");
    let mut kinds = Vec::new();
    for (kind, rect) in page_shapes(f) {
        if rect.is_positive() && rect.intersects(screen) {
            assert!(rect.distance_to_pos(center) >= r, "{what}: the moon at {center:?}, r {r}, meets a {kind} at {rect:?}");
            kinds.push(kind);
        }
    }
    kinds
}

/// Every shape the last frame drew but the backdrop's own, as laid out within its clip: the sky,
/// the page colour and the veil run the window's width, the chrome's soft shade falls from the
/// window's top, and blurred shadows and halos are no cards.
pub(super) fn page_shapes(f: &Frames) -> Vec<(&'static str, Rect)> {
    fn walk(shape: &egui::Shape, clip: Rect, soft: &glass::SoftTextures, out: &mut Vec<(&'static str, Rect)>) {
        let (kind, rect) = match shape {
            egui::Shape::Vec(shapes) => return shapes.iter().for_each(|s| walk(s, clip, soft, out)),
            egui::Shape::Mesh(mesh) if soft.contains(&mesh.texture_id) => return,
            egui::Shape::Mesh(mesh) => ("mesh", mesh.calc_bounds()),
            egui::Shape::Rect(rect) => ("rect", rect.rect),
            egui::Shape::Circle(circle) => ("circle", Rect::from_center_size(circle.center, Vec2::splat(circle.radius * 2.0))),
            egui::Shape::Path(path) => ("path", Rect::from_points(&path.points)),
            egui::Shape::Text(text) => ("text", text.visual_bounding_rect()),
            egui::Shape::Noop => return,
            other => ("shape", other.visual_bounding_rect()),
        };
        out.push((kind, rect.intersect(clip)));
    }
    let screen = f.ctx.content_rect();
    let soft = glass::soft_textures(&f.ctx);
    let backdrop = |shape: &egui::Shape| matches!(shape, egui::Shape::Vec(s) if matches!(s.first(), Some(egui::Shape::Rect(r)) if r.rect.min == Pos2::ZERO && r.fill == design_tokens::COLOR_CANVAS));
    let mut out = Vec::new();
    for clipped in f.shapes.iter().filter(|c| !backdrop(&c.shape)) {
        walk(&clipped.shape, clipped.clip_rect, &soft, &mut out);
    }
    out.retain(|(_, rect)| rect.width() < screen.width() - 1.0 && rect.top() > screen.top());
    out
}

/// The demo's PCs page: one running a game, one online, one asleep and one not paired.
pub(super) fn four_pcs() -> Frames {
    pcs_more(0)
}

/// [`four_pcs`] and `more` paired PCs after them.
pub(super) fn pcs_more(more: usize) -> Frames {
    let mut records = vec![record("Living Room PC", true, true), record("Studio PC", true, true), record("Office PC", true, true), record("Game Room PC", false, false)];
    records.extend((0..more).map(|n| record(&format!("PC {n}"), true, false)));
    for (n, record) in records.iter_mut().enumerate() {
        record.address = format!("192.0.2.{}", 80 + n);
    }
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    let mut l = launcher(false, false);
    l.go(Screen::Hosts);
    let mut hosts = Hosts::without_discovery(records, Arc::new(|| {}));
    for (host, reach) in hosts.list.iter_mut().zip([Reach::Online, Reach::Online, Reach::Offline, Reach::Online]) {
        host.reach = reach;
    }
    hosts.list[0].apps = Apps::Loaded(Arc::new(vec![game(1)]));
    Frames::over(ctx, l, hosts)
}

/// A small cover texture of this test's own.
pub(super) fn cover_texture(f: &Frames, name: &str) -> egui::TextureHandle {
    f.ctx.load_texture(name, egui::ColorImage::new([3, 4], vec![Color32::GRAY; 12]), egui::TextureOptions::LINEAR)
}

/// A PC with one game in its list, and the launch of that game as app/launch.rs hands it to the scene.
pub(super) fn launching() -> (Frames, ConnectingView) {
    let mut f = Frames::with(record("Living Room PC", true, true), Reach::Online, Some(answer(true, 0)), vec![nightjar_policy::http::App { name: "Racing Game".into(), ..game(7) }]);
    let launch = ConnectingView { app: "Racing Game".into(), host: "Living Room PC".into(), stage: "Checking network profile".into(), host_id: f.hosts.list[0].id, app_id: 7 };
    f.connecting = Some(launch.clone());
    f.settle();
    (f, launch)
}

/// Clicks the last `text` drawn: a dialog's, over the page.
pub(super) fn click_in_dialog(f: &mut Frames, text: &str) -> Vec<Command> {
    let at = f.placed.iter().rev().find(|(t, ..)| t == text).unwrap_or_else(|| panic!("{text:?} is not drawn: {:?}", f.drawn)).1.center();
    f.click_at(at, egui::PointerButton::Primary)
}

pub(super) fn game_facts(genres: &[&str], themes: &[&str], normally_hours: u64) -> nightjar_policy::game_info::cache::Facts {
    nightjar_policy::game_info::cache::Facts {
        name: String::new(),
        genres: genres.iter().map(|g| (*g).to_owned()).collect(),
        themes: themes.iter().map(|t| (*t).to_owned()).collect(),
        hastily: Some(normally_hours * 3600 * 3 / 4),
        normally: Some(normally_hours * 3600),
        completely: Some(normally_hours * 3600 * 2),
    }
}

/// The key the first PC's game `title` keeps its facts under, as the pages look it up.
pub(super) fn facts_key(f: &Frames, title: &str) -> String {
    let game = f.hosts.list[0].library().expect("a library").games.iter().find(|g| g.title == title).expect(title);
    let looked_up = crate::hosts::game_of(&game.title, game.game_id.as_deref(), None);
    nightjar_policy::game_info::names::key(&looked_up.name, looked_up.steam)
}
