//! Headless launcher frame driver (`Frames`) and core state/layout assertion helpers.

use super::*;

pub(super) fn tap_key(key: Key) -> [egui::Event; 2] {
    let event = |pressed| egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    [event(true), event(false)]
}

pub(super) fn launcher(console_home: bool, home_ready: bool) -> Launcher {
    let settings = Settings { console_home, ..Default::default() };
    let display = Display { width: 1920, height: 1080, refresh_hz: 60.0, refresh_reported: true };
    let mut launcher = Launcher::new(settings, Profiles::default(), crate::shortcuts::Shortcuts::default(), display);
    launcher.home_ready = home_ready;
    launcher.home_root = home_ready;
    launcher
}

pub(super) fn record(name: &str, paired: bool, mac: bool) -> HostRecord {
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
pub(super) struct Frames {
    pub(super) ctx: egui::Context,
    pub(super) l: Launcher,
    pub(super) hosts: Hosts,
    pub(super) pads: PadsView,
    pub(super) time: f64,
    /// The window; tall, a test can see a whole page at once.
    pub(super) size: egui::Vec2,
    /// The text drawn by the last frame, in paint order.
    pub(super) drawn: Vec<String>,
    /// The same text with where it landed and the clip it was drawn in.
    pub(super) placed: Vec<(String, Rect, Rect)>,
    pub(super) waking: Option<WakingView>,
    /// A start under way, as app/launch.rs hands it over: the launch scene covers the launcher.
    pub(super) connecting: Option<ConnectingView>,
    /// Every texture change since the first frame, and the last frame's shapes, for screenshots.
    pub(super) textures: egui::TexturesDelta,
    pub(super) shapes: Vec<egui::epaint::ClippedShape>,
    /// The display's scale (1 everywhere but in screenshots).
    pub(super) native_ppp: f32,
    /// The last frame's AccessKit tree, once a test turned AccessKit on.
    pub(super) access: Option<egui::accesskit::TreeUpdate>,
    /// Keeps every texture for `shoot` (screenshots at any scale, 100 % too).
    pub(super) shots: bool,
}

impl Drop for Frames {
    fn drop(&mut self) {
        self.textures.clear();
    }
}

impl Frames {
    pub(super) fn new(record: HostRecord, reach: Reach) -> Self {
        Self::with(record, reach, None, Vec::new())
    }

    /// A PC as the app sees it after the PC answered (`info`) and its classic list loaded.
    pub(super) fn with(record: HostRecord, reach: Reach, info: Option<nightjar_policy::http::ServerInfo>, apps: Vec<nightjar_policy::http::App>) -> Self {
        crate::theme::own_palette();
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut l = launcher(false, false);
        l.go(Screen::Hosts);
        let mut hosts = Hosts::without_discovery(vec![record], Arc::new(|| {}));
        hosts.list[0].reach = reach;
        hosts.list[0].info = info;
        if !apps.is_empty() {
            hosts.list[0].apps = Apps::Loaded(Arc::new(apps));
        }
        let mut frames = Self::over(ctx, l, hosts);
        frames.settle();
        frames
    }

    pub(super) fn frame(&mut self, events: Vec<egui::Event>) -> Vec<Command> {
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
            commands = self.l.ui(ui, View { hosts: &self.hosts, pads: &self.pads, connecting: self.connecting.clone(), waking: self.waking.clone() });
        });
        self.access = output.platform_output.accesskit_update.clone();
        // Only screenshots read the textures; everyone else lets them go. A run that writes them
        // keeps every frame's from the first, so a glyph added after shots begin patches a font
        // texture the canvas already has.
        if self.native_ppp == 1.0 && !self.shots && std::env::var_os("NIGHTJAR_SHOTS").is_none() {
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

    pub(super) fn over(ctx: egui::Context, l: Launcher, hosts: Hosts) -> Self {
        Self {
            ctx,
            l,
            hosts,
            pads: PadsView::default(),
            time: 0.0,
            size: vec2(1280.0, 720.0),
            drawn: Vec::new(),
            placed: Vec::new(),
            waking: None,
            connecting: None,
            textures: Default::default(),
            shapes: Vec::new(),
            native_ppp: 1.0,
            access: None,
            shots: false,
        }
    }

    /// Paints the last frame into `<dir>/<name>.png`. Needs `native_ppp` set before the first frame.
    pub(super) fn shoot(&self, dir: &std::path::Path, name: &str) {
        let ppp = self.ctx.pixels_per_point();
        let primitives = self.ctx.tessellate(self.shapes.clone(), ppp);
        let mut canvas = crate::shot::Canvas::new((self.size.x * ppp) as usize, (self.size.y * ppp) as usize);
        canvas.update_textures(&self.textures);
        canvas.paint(&primitives, ppp);
        canvas.save(&dir.join(format!("{name}.png")));
    }

    pub(super) fn settle(&mut self) -> Vec<Command> {
        let mut commands = Vec::new();
        for _ in 0..4 {
            commands.extend(self.frame(Vec::new()));
        }
        commands
    }

    pub(super) fn press(&mut self, key: Key) -> Vec<Command> {
        let mut commands = self.frame(tap_key(key).to_vec());
        commands.extend(self.settle());
        commands
    }

    pub(super) fn press_with(&mut self, key: Key, modifiers: egui::Modifiers) -> Vec<Command> {
        let mut events = vec![egui::Event::ModifiersChanged(modifiers)];
        events.extend(tap_key(key));
        let mut commands = self.frame(events);
        commands.extend(self.frame(vec![egui::Event::ModifiersChanged(Default::default())]));
        commands.extend(self.settle());
        commands
    }

    pub(super) fn click_at(&mut self, pos: Pos2, button: egui::PointerButton) -> Vec<Command> {
        let event = |pressed| egui::Event::PointerButton { pos, button, pressed, modifiers: egui::Modifiers::NONE };
        let mut commands = self.frame(vec![egui::Event::PointerMoved(pos)]);
        commands.extend(self.frame(vec![event(true)]));
        commands.extend(self.frame(vec![event(false)]));
        commands.extend(self.settle());
        commands
    }

    /// Turns the mouse wheel down over the middle of the window, far enough to reach the end
    /// of what scrolls there.
    pub(super) fn scroll_to_end(&mut self) {
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
    pub(super) fn card_and_dots(&self) -> (Rect, Rect) {
        let focused = self.ctx.memory(|m| m.focused()).expect("the first PC has focus");
        let card = self.ctx.read_response(focused).expect("the card was laid out").rect;
        let dots = self.ctx.read_response(focused.with("options")).expect("the dots were laid out").rect;
        (card, dots)
    }

    pub(super) fn options_open(&self) -> bool {
        matches!(self.l.dialog, Dialog::HostOptions { .. })
    }

    pub(super) fn no_dialog(&self) -> bool {
        matches!(self.l.dialog, Dialog::None)
    }
}

pub(super) fn collect_text(shape: &egui::Shape, out: &mut Vec<String>) {
    match shape {
        egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
        egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect_text(s, out)),
        _ => {}
    }
}

pub(super) fn collect_placed(shape: &egui::Shape, clip: Rect, out: &mut Vec<(String, Rect, Rect)>) {
    match shape {
        egui::Shape::Text(text) => out.push((text.galley.text().to_owned(), text.galley.rect.translate(text.pos.to_vec2()), clip)),
        egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect_placed(s, clip, out)),
        _ => {}
    }
}

/// What the PC said last, as the host sends it.
pub(super) fn answer(paired: bool, running: i32) -> nightjar_policy::http::ServerInfo {
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\"><hostname>PC</hostname><uniqueid>uid</uniqueid>             <HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>{}</PairStatus>             <currentgame>{running}</currentgame><state>SUNSHINE_SERVER_{}</state><Permission>1</Permission></root>",
        u8::from(paired),
        if running == 0 { "FREE" } else { "BUSY" },
    );
    nightjar_policy::http::ServerInfo::parse(&xml, 47989).expect("the answer parses")
}

pub(super) fn game(id: i32) -> nightjar_policy::http::App {
    nightjar_policy::http::App {
        name: "Game".into(),
        id,
        hdr_supported: false,
        uuid: String::new(),
        host_index: None,
        art_version: String::new(),
        control: nightjar_policy::http::AppControl::None,
        monitor_resume: false,
    }
}

/// One PC in one state, enough to reach every entry of the options list.
pub(super) struct State {
    pub(super) name: &'static str,
    pub(super) record: HostRecord,
    pub(super) reach: Reach,
    pub(super) info: Option<nightjar_policy::http::ServerInfo>,
    pub(super) apps: Vec<nightjar_policy::http::App>,
    /// The native read, for the options only it offers (Restart PC, Shut down PC, Update).
    pub(super) native: Option<(nightjar_policy::http::NativeCapabilities, Option<nightjar_policy::http::HostUpdate>)>,
}

impl State {
    pub(super) fn frames(&self) -> Frames {
        let mut frames = Frames::with(self.record.clone(), self.reach, self.info.clone(), self.apps.clone());
        if let Some((capabilities, update)) = &self.native {
            frames.hosts.list[0].native.capabilities = Some(capabilities.clone());
            frames.hosts.list[0].native.update = update.clone();
            frames.settle();
        }
        frames
    }
}

pub(super) fn every_state() -> Vec<State> {
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
pub(super) fn power_native(allowed: bool, host_control: bool) -> (nightjar_policy::http::NativeCapabilities, Option<nightjar_policy::http::HostUpdate>) {
    let capabilities = serde_json::from_str(&format!(
        "{{\"api_version\":1,\"host\":{{\"name\":\"PC\",\"version\":\"3.3.0\",\"unique_id\":\"uid\"}},\
         \"features\":[\"power.restart\",\"power.shutdown\",\"update\",\"pairing.approve\"],\
         \"permissions\":{{\"list\":true,\"view\":true,\"launch\":true,\"host_control\":{host_control},\"restart_shutdown\":{allowed}}}}}"
    ))
    .expect("the answer parses");
    let update = nightjar_policy::http::HostUpdate {
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
pub(super) fn mic_only_answer(launch: bool) -> nightjar_policy::http::ServerInfo {
    let mut info = files::tests::native_answer();
    let mut native = files::tests::native("\"session.mic_only\"", false, false);
    native.capabilities.permissions.launch = launch;
    crate::hosts::merge_native(&mut info, Some(&native));
    info
}

/// A paired PC's answer with the power-action fields, sleep allowed for this device or not.
pub(super) fn sleep_answer(allowed: bool) -> nightjar_policy::http::ServerInfo {
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\"><hostname>PC</hostname><uniqueid>uid</uniqueid>             <HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>1</PairStatus><currentgame>0</currentgame>             <state>SUNSHINE_SERVER_FREE</state><PowerActionVersion>1</PowerActionVersion><SleepSupported>1</SleepSupported>             <SleepAllowed>{}</SleepAllowed></root>",
        u8::from(allowed),
    );
    nightjar_policy::http::ServerInfo::parse(&xml, 47989).expect("the answer parses")
}

pub(super) fn removes(commands: &[Command]) -> bool {
    commands.iter().any(|c| matches!(c, Command::Remove(_)))
}

/// The main settings on Video with the whole list of sections, as after "More settings" and Video
/// were chosen: a first visit opens Recommended with the sections of detail folded.
pub(super) fn video_settings(f: &mut Frames) {
    f.l.go(Screen::Settings);
    f.l.settings_view.rail_more = true;
    f.l.settings_view.section = "Video";
    f.settle();
}

/// The widgets a screen reader can press or change that have no name to say, with their role and place.
pub(super) fn unnamed(f: &Frames) -> Vec<String> {
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

/// A 1280x720 window at 150 % zoom is 853x480 points. Everything drawn must stay inside it
/// sideways (a cut-off button label shows as text outside its clip); a dialog, which does not
/// scroll, must also stay inside it from top to bottom.
pub(super) fn fit_150(f: &mut Frames) {
    f.l.settings.ui_zoom = 150;
    f.l.settings.language = crate::config::Language::German;
    f.size = vec2(1280.0 / 1.5, 720.0 / 1.5);
    f.settle();
    assert!((f.ctx.zoom_factor() - 1.5).abs() < 1e-6, "the zoom setting reached egui");
}

pub(super) fn assert_fits(f: &Frames, what: &str, whole_height: bool) {
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
pub(super) fn must_stay_in_window(rect: Rect, clip: Rect, screen: Rect) -> bool {
    // Wholly under the window's edge is a page's scrolled-off rest; a dialog's own is caught by
    // `areas_past_window`.
    rect.top() < screen.bottom() && (rect.intersects(clip) || clip.bottom() >= screen.bottom() - 1.0)
}

/// A dialog's whole area, whatever part of it reached the screen: egui doesn't paint a button
/// outside the clip, so one wholly below the window never shows up among the drawn text.
pub(super) fn areas_past_window(ctx: &egui::Context, screen: Rect) -> Vec<Rect> {
    ctx.memory(|m| {
        m.areas()
            .visible_layer_ids()
            .into_iter()
            .filter_map(|layer| m.area_rect(layer.id))
            .filter(|area| area.top() < -1.0 || area.bottom() > screen.bottom() + 1.0)
            .collect()
    })
}

/// The meshes drawn and their clips, nested shapes included.
pub(super) fn meshes_drawn(f: &Frames) -> Vec<(std::sync::Arc<egui::epaint::Mesh>, Rect)> {
    fn walk(shape: &egui::Shape, clip: Rect, out: &mut Vec<(std::sync::Arc<egui::epaint::Mesh>, Rect)>) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, clip, out)),
            egui::Shape::Mesh(mesh) => out.push((mesh.clone(), clip)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    for clipped in &f.shapes {
        walk(&clipped.shape, clipped.clip_rect, &mut found);
    }
    found
}

/// The rectangles drawn filled with `fill`, nested shapes included.
pub(super) fn rects_filled(f: &Frames, fill: Color32) -> Vec<Rect> {
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
pub(super) fn fades(f: &Frames) -> Vec<Rect> {
    let mut found = Vec::new();
    for clipped in &f.shapes {
        if let egui::Shape::Mesh(mesh) = &clipped.shape {
            let colours: Vec<Color32> = mesh.vertices.iter().map(|v| v.color).collect();
            if (colours.contains(&*SURFACE_HIGH) || colours.contains(&dialogs::MENU_FADE)) && colours.contains(&Color32::TRANSPARENT) {
                found.push(Rect::from_points(&mesh.vertices.iter().map(|v| v.pos).collect::<Vec<_>>()));
            }
        }
    }
    found
}

pub(super) const RUBYLIGHT: &str = "<RustHostVersion>3.0.0</RustHostVersion>";

/// A paired PC's details page; `state` is its serverinfo state, `extra` more of its fields.
pub(super) fn details(reach: Reach, state: &str, extra: &str) -> Frames {
    let mut f = Frames::with(record("uid", true, false), reach, Some(host_answer(state, extra)), Vec::new());
    let id = f.hosts.list[0].id;
    f.l.open(Screen::HostDetails(id));
    f.settle();
    f
}

pub(super) fn shows(f: &Frames, text: &str) -> bool {
    f.drawn.iter().any(|t| t == text)
}

/// Frames until the check is no longer running, as the app's waker would bring them.
pub(super) fn finish_check(f: &mut Frames) {
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

pub(super) fn relay_check() -> Result<nightjar_policy::http::RemoteCheck, nightjar_policy::http::Error> {
    use nightjar_policy::http::{ConnectionTestStats, NetworkPath, RemoteCheck, RemoteOutcome};
    let mut stats = ConnectionTestStats::default();
    [10.0, 14.0, f64::NAN, 12.0, 12.0].into_iter().for_each(|ms| stats.add(ms));
    let server_info = Box::new(host_answer("SUNSHINE_SERVER_FREE", RUBYLIGHT));
    Ok(RemoteCheck { outcome: RemoteOutcome::Verified { server_info, path: Ok(NetworkPath::Relay) }, stats })
}

/// Clicks the first `text` drawn on the last frame.
pub(super) fn click_text(f: &mut Frames, text: &str) -> Vec<Command> {
    let at = f.placed.iter().find(|(t, ..)| t == text).unwrap_or_else(|| panic!("{text:?} is not drawn: {:?}", f.drawn)).1.center();
    f.click_at(at, egui::PointerButton::Primary)
}

/// Whether `text` is drawn on the widget with focus.
pub(super) fn focus_shows(f: &Frames, text: &str) -> bool {
    let Some(rect) = f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect) else {
        return false;
    };
    f.placed.iter().any(|(t, r, _)| t == text && rect.contains(r.center()))
}

/// A PC whose capabilities list pairing.approve, for a device that holds Host control or not.
pub(super) fn approving_frames(host_control: bool) -> Frames {
    let mut f = every_state().into_iter().find(|s| s.name == "power").unwrap().frames();
    let capabilities = f.hosts.list[0].native.capabilities.as_mut().unwrap();
    capabilities.capabilities.permissions.host_control = host_control;
    f
}

pub(super) fn tv_request(id: &str) -> nightjar_policy::http::PairingRequest {
    nightjar_policy::http::PairingRequest {
        id: id.into(),
        name: "TV".into(),
        address: "192.168.4.116".into(),
        age_seconds: 3,
    }
}

/// The tab stops in the header band, in the order Tab reaches them.
pub(super) fn header_stops(f: &mut Frames) -> Vec<Rect> {
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
