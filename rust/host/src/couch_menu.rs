//! The host-drawn couch menu's host wiring (`docs/couch-menu-design.md`
//! section 3.3): the filter on each control peer, the menu worker that draws
//! pages and runs actions, the video loop's overlay and `POST
//! /session/couch-menu`.
//!
//! The menu is GameStream only: the WebRTC transport has no control peer
//! here, so its sessions never open it.
//!
//! While the menu is closed the video loop does one atomic load per frame and
//! the control loop one branch per batch; nothing else runs.
pub(crate) mod items;

use crate::{
    console::i18n,
    native_api::{self, Failure},
    state::{Session, Shared},
};
use butterpollo_core::{
    config::Config,
    couch_menu::{
        self as menu, Action, Filter, Frame, Layout, Menu, Nav, Snapshot, Strings,
        chord::OpenWith,
        model::{Availability, Setting, Target},
    },
    input::Input,
    native_api::error::ErrorCode,
    session::Role,
    session_state::Refusal,
    state::Client,
};
use butterpollo_windows::{input::Injector, overlay::Overlay};
use std::{
    collections::HashMap,
    sync::{
        Arc, Weak,
        atomic::Ordering,
        mpsc::{self, RecvTimeoutError},
    },
    time::{Duration, Instant},
};

/// How often an open menu redraws its status line.
const REFRESH: Duration = Duration::from_secs(1);

/// Why `role`'s session cannot show the menu, or `None` when it can.
/// `direct` is `wgc_direct_encoder_input`: the read-back encode path uploads
/// the picture itself and composites no overlay.
pub(crate) fn refusal(role: Role, direct: bool, size: [u32; 2]) -> Option<String> {
    if role == Role::InputOnly {
        return Some("couch menu: an input-only session has no picture to draw it on".into());
    }
    if !direct {
        return Some(
            "couch menu: the read-back encode path does not draw it; turn \
             wgc_direct_encoder_input back on"
                .into(),
        );
    }
    Layout::for_stream(size[0], size[1])
        .err()
        .map(|small| small.to_string())
}

/// One control peer's menu filter, made for the peer's session.
#[derive(Default)]
pub(crate) struct PeerMenu {
    /// The session followed, by its menu state.
    shared: Option<Arc<menu::Shared>>,
    /// `None` when the session cannot have the menu or it is off.
    filter: Option<Filter>,
    direct: bool,
    size: [u32; 2],
}

impl PeerMenu {
    /// Follows `s`, reading `config` only when the session is new. Returns
    /// true when it made a filter, so the caller registers its control wake.
    pub(crate) fn follow(
        &mut self,
        s: &Session,
        config: impl FnOnce() -> anyhow::Result<Config>,
    ) -> bool {
        let mode = s.stream_mode();
        let size = [mode.width, mode.height];
        if self
            .shared
            .as_ref()
            .is_some_and(|shared| Arc::ptr_eq(shared, &s.couch_menu))
        {
            if size != self.size
                && let Some(filter) = &mut self.filter
            {
                self.size = size;
                filter.resize(size[0], size[1]);
                s.couch_menu
                    .refuse(refusal(s.launch.role, self.direct, size));
            }
            return false;
        }
        *self = Self {
            shared: Some(s.couch_menu.clone()),
            size,
            ..Self::default()
        };
        // What the stream allows is read here once; `request` and the
        // filter follow the stored refusal, not later edits.
        let reason = if s.launch.role == Role::InputOnly {
            refusal(s.launch.role, true, size)
        } else {
            match config() {
                Err(error) => {
                    tracing::warn!(error = %format!("{error:#}"), "couch menu: configuration unavailable; the menu is off for this stream");
                    Some("couch menu: the configuration did not load".into())
                }
                Ok(c) if !c.couch_menu() => Some(
                    "couch menu: off for this stream (couch_menu); a change applies from the \
                     next stream"
                        .into(),
                ),
                Ok(c) => {
                    self.direct = c.boolean("wgc_direct_encoder_input", true);
                    self.filter = Some(Filter::new(
                        s.couch_menu.clone(),
                        OpenWith::from_config(&c),
                        size[0],
                        size[1],
                    ));
                    refusal(s.launch.role, self.direct, size)
                }
            }
        };
        let refused = reason.is_some();
        s.couch_menu.refuse(reason);
        // A second peer of the session can see another configuration.
        if refused {
            s.couch_menu.close();
        }
        self.filter.is_some()
    }
    /// When the filter wants an empty batch: a hold completing.
    pub(crate) fn next_due(&self) -> Option<Instant> {
        self.filter.as_ref()?.next_due()
    }
}

/// What the game side of a batch needs; the injector, or a fake in tests.
pub(crate) trait Game {
    fn release_all(&mut self);
    fn apply_all(&mut self, events: &[Input]) -> Vec<anyhow::Error>;
    fn pointer(&mut self) -> Option<(f32, f32)>;
}

impl Game for Injector {
    fn release_all(&mut self) {
        Injector::release_all(self);
    }
    fn apply_all(&mut self, events: &[Input]) -> Vec<anyhow::Error> {
        Injector::apply_all(self, events)
    }
    fn pointer(&mut self) -> Option<(f32, f32)> {
        self.pointer_in_stream()
    }
}

/// What one batch left for the control loop.
#[derive(Debug, Default)]
pub(crate) struct Batch {
    /// For the menu worker.
    pub nav: Vec<Nav>,
    /// A chord asked for a menu this session refuses: warn.
    pub refused: Option<String>,
    /// From the injector.
    pub errors: Vec<anyhow::Error>,
}

/// Runs one batch through the peer's filter, then gives the game what it
/// kept. When the menu opens, everything the game holds is released before
/// the kept events (the ones before the chord) are applied. The injector is
/// always called, even with no events, because it flushes there.
pub(crate) fn apply_batch(
    menu: &mut PeerMenu,
    mut game: Option<&mut impl Game>,
    events: &mut Vec<Input>,
    now: Instant,
) -> Batch {
    let mut batch = Batch::default();
    if let Some(filter) = &mut menu.filter {
        let out = filter.apply(now, events, || game.as_deref_mut()?.pointer());
        if out.release_game
            && let Some(game) = game.as_deref_mut()
        {
            game.release_all();
        }
        batch.nav = out.nav;
        batch.refused = out.refused;
    }
    if let Some(game) = game {
        batch.errors = game.apply_all(events);
    }
    batch
}

struct Worker {
    shared: Arc<menu::Shared>,
    nav: mpsc::Sender<Nav>,
}

/// The menu workers of the control loop's sessions, by launch id: one per
/// session, so a second peer of the same session shares it.
#[derive(Default)]
pub(crate) struct Workers(HashMap<String, Worker>);

impl Workers {
    /// Sends `nav` to `s`'s worker, starting one when there is none or it
    /// ended.
    pub(crate) fn send(&mut self, h: &Shared, s: &Arc<Session>, nav: Vec<Nav>) {
        if nav.is_empty() {
            return;
        }
        let id = &s.launch.id;
        let mut nav = nav.into_iter().peekable();
        if let Some(worker) = self.0.get(id)
            && Arc::ptr_eq(&worker.shared, &s.couch_menu)
        {
            while let Some(&next) = nav.peek() {
                if worker.nav.send(next).is_err() {
                    break;
                }
                nav.next();
            }
            if nav.peek().is_none() {
                return;
            }
        }
        self.0.remove(id);
        let Some(worker) = spawn(h, s) else { return };
        for next in nav {
            let _ = worker.nav.send(next);
        }
        self.0.insert(id.clone(), worker);
    }
    /// Drops the workers of sessions `keep` refuses; each ends on its own.
    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&str) -> bool) {
        self.0.retain(|id, _| keep(id));
    }
}

fn spawn(h: &Shared, s: &Arc<Session>) -> Option<Worker> {
    let (sender, receiver) = mpsc::channel();
    let locale = h.config.read().unwrap().get("locale", "en").to_owned();
    let (host, session, shared) = (h.clone(), Arc::downgrade(s), s.couch_menu.clone());
    match std::thread::Builder::new()
        .name("couch-menu".into())
        .spawn(move || run(&host, &session, &shared, &receiver, &locale))
    {
        Ok(_) => Some(Worker {
            shared: s.couch_menu.clone(),
            nav: sender,
        }),
        Err(error) => {
            tracing::error!(%error, "couch menu: the worker did not start; closing the menu");
            s.couch_menu.close();
            None
        }
    }
}

/// The menu's own strings and the labels of the registered rows, in the
/// host's language.
fn strings(locale: &str, registry: &[&dyn items::Item]) -> Result<Strings, String> {
    let strings = Strings::from_lookup(|path| i18n::catalogue(locale, path))?;
    for item in registry {
        let path = menu::model::items::label_path(item.key());
        let label = i18n::catalogue(locale, &path)
            .ok_or_else(|| format!("couch menu: string {path} is missing"))?;
        if let Some(c) = menu::font::uncovered(&label) {
            return Err(format!(
                "couch menu: string {path} has {c:?}, outside Latin-1"
            ));
        }
    }
    Ok(strings)
}

/// The menu worker: waits while the menu is closed, draws pages while it is
/// open and runs the actions, which can block for seconds. It ends when the
/// control loop drops its sender or the session stops.
fn run(
    h: &Shared,
    session: &Weak<Session>,
    shared: &menu::Shared,
    navs: &mpsc::Receiver<Nav>,
    locale: &str,
) {
    let mut menu = match strings(locale, items::ITEMS) {
        Ok(strings) => Menu::new(strings),
        Err(reason) => {
            tracing::error!("{reason}; the couch menu is off for this stream");
            shared.refuse(Some(reason));
            shared.close();
            return;
        }
    };
    let mut shown: Option<Frame> = None;
    let mut refresh_at = Instant::now();
    // A new worker draws an open menu before it waits.
    let mut nav = None;
    loop {
        let Some(s) = session.upgrade() else { return };
        if s.stopping() {
            shared.close();
            return;
        }
        let now = Instant::now();
        match nav {
            Some(Nav::Opened | Nav::Closed) => {
                menu.reset();
                shown = None;
            }
            Some(nav) if menu::Shared::is_open(shared.load()) => {
                if let Some(page) = Page::new(h, &s, shared, locale)
                    && let Some(action) = menu.nav(nav, now, &page.snap, &page.layout)
                {
                    if action == Action::Close {
                        shared.close();
                    } else {
                        // "Working..." while the action runs.
                        page.draw(&menu, shared, &mut shown);
                        let result = act(h, &s, &action);
                        if let Err(failure) = &result {
                            tracing::warn!(?action, code = ?failure.code(), "couch menu: {}", failure.message());
                        }
                        menu.finish(result.map_err(|failure| failure.message().to_owned()));
                    }
                }
            }
            Some(_) => {}
            None => {
                menu.tick(now);
            }
        }
        let open = menu::Shared::is_open(shared.load());
        if open {
            if let Some(page) = Page::new(h, &s, shared, locale) {
                page.draw(&menu, shared, &mut shown);
            }
            refresh_at = Instant::now() + REFRESH;
        }
        drop(s);
        nav = if open {
            let due = menu
                .next_due()
                .map_or(refresh_at, |due| due.min(refresh_at));
            match navs.recv_timeout(due.saturating_duration_since(Instant::now())) {
                Ok(nav) => Some(nav),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match navs.recv() {
                Ok(nav) => Some(nav),
                Err(_) => return,
            }
        };
    }
}

/// What a page is drawn from.
struct Page {
    snap: Snapshot,
    layout: Layout,
}

impl Page {
    /// `None` when the stream became too small: the menu is closed and
    /// refused then.
    fn new(h: &Shared, s: &Session, shared: &menu::Shared, locale: &str) -> Option<Self> {
        let mode = s.stream_mode();
        match Layout::for_stream(mode.width, mode.height) {
            Ok(layout) => Some(Self {
                snap: snapshot(h, s, items::ITEMS, locale),
                layout,
            }),
            Err(small) => {
                tracing::warn!("{small}; closing the couch menu");
                shared.refuse(Some(small.to_string()));
                shared.close();
                None
            }
        }
    }
    fn draw(&self, menu: &Menu, shared: &menu::Shared, shown: &mut Option<Frame>) {
        let frame = menu::render(menu, &self.snap, &self.layout);
        if shown.as_ref() != Some(&frame) && shared.publish(frame.clone()) {
            *shown = Some(frame);
        }
    }
}

/// The session's device as the console has it now.
fn client(h: &Shared, s: &Session) -> Result<Client, Failure> {
    h.paired
        .read()
        .unwrap()
        .clients
        .iter()
        .find(|client| client.enabled && client.uuid == s.launch.client.uuid)
        .cloned()
        .ok_or_else(|| {
            Failure::new(
                ErrorCode::NotPaired,
                "This device is no longer paired with this host, or it is turned off.",
            )
        })
}

/// Runs a row's action with the same functions as the native API, checking
/// the device's permissions right before.
fn act(h: &Shared, s: &Session, action: &Action) -> Result<(), Failure> {
    let session_action = |action| native_api::session_action(h, &client(h, s)?, action);
    match action {
        Action::Close => Ok(()),
        Action::Disconnect => session_action(native_api::Action::Disconnect),
        Action::EndSession => session_action(native_api::Action::End),
        Action::CloseGame => session_action(native_api::Action::CloseGame),
        Action::Change {
            target: Target::Item,
            change,
        } => {
            let client = client(h, s)?;
            let item = items::find(items::ITEMS, &change.id).ok_or_else(|| {
                Failure::new(
                    ErrorCode::InvalidRequest,
                    format!("The menu has no row {}.", change.id),
                )
            })?;
            if item.needs_host_control() {
                native_api::require_host_control(&client)?;
            }
            item.set(h, s, &client, change.value.clone())
        }
        Action::Change { .. } | Action::RestoreRadeon { .. } => Err(Failure::new(
            ErrorCode::TemporarilyUnavailable,
            "Radeon settings are not in the menu yet.",
        )),
    }
}

fn codec(codec: u32) -> &'static str {
    match codec {
        0 => "H.264",
        1 => "HEVC",
        2 => "AV1",
        3 => "PyroWave",
        _ => "video",
    }
}

/// The main page's status line and the registered rows that apply.
fn snapshot(h: &Shared, s: &Session, registry: &[&dyn items::Item], locale: &str) -> Snapshot {
    let host_control =
        client(h, s).is_ok_and(|client| butterpollo_core::native::host_control(&client));
    let items = registry
        .iter()
        .filter_map(|item| {
            let availability = item.available(h, s);
            (availability != Availability::Hidden).then(|| Setting {
                id: item.key().to_owned(),
                label: i18n::catalogue(locale, &menu::model::items::label_path(item.key()))
                    .unwrap_or_else(|| item.key().to_owned()),
                control: item.value(h, s),
                availability,
                applies: item.applies(),
                needs_host_control: item.needs_host_control(),
            })
        })
        .collect();
    Snapshot {
        status: format!(
            "{} {} {} Mb/s",
            s.stream_mode(),
            codec(s.config.codec.into()),
            s.bitrate.load(Ordering::Relaxed) / 1000
        ),
        host_control,
        radeon: None,
        preset: None,
        items,
    }
}

/// The video loop's view of the menu: the overlay it gives the encoder.
#[derive(Default)]
pub(crate) struct VideoOverlay {
    state: u64,
    /// The encode size `overlay` was checked against.
    target: [u32; 2],
    /// The published panel while the menu is open.
    frame: Option<Arc<Frame>>,
    overlay: Option<Overlay>,
    /// The overlay changed: give it to the encoder and encode a new frame,
    /// not a repeat.
    pub dirty: bool,
}

impl VideoOverlay {
    /// One atomic load while nothing changed. The worker draws at the
    /// session's mode, which can differ from the `target` encode size (a mode
    /// the client just asked for, or one the encoder refused): a panel laid
    /// out for another size waits for a redraw. One laid out for this size
    /// that does not fit closes the menu.
    pub(crate) fn poll(&mut self, shared: &menu::Shared, target: [u32; 2]) {
        let state = shared.load();
        if state == self.state && target == self.target {
            return;
        }
        if state != self.state {
            self.state = state;
            self.frame = menu::Shared::is_open(state)
                .then(|| shared.frame())
                .flatten();
        }
        self.target = target;
        let overlay = self.frame.as_deref().and_then(|frame| {
            let overlay = Overlay::new(menu::Shared::revision(state), frame);
            match overlay.validate(target) {
                Ok(()) => Some(overlay),
                Err(error) if frame.target != target => {
                    tracing::debug!(error = %format!("{error:#}"), drawn = ?frame.target, "couch menu: the panel waits for a redraw at the stream's size");
                    None
                }
                Err(error) => {
                    tracing::error!(error = %format!("{error:#}"), "couch menu: the panel does not fit the stream; closing the menu");
                    shared.close();
                    None
                }
            }
        });
        if overlay != self.overlay {
            self.overlay = overlay;
            self.dirty = true;
        }
    }
    /// The overlay for an encoder of `target` size. After a rebuild to a new
    /// size the old panel may not fit; it is dropped until the worker draws
    /// one for the new size, or the stream goes back to the old one.
    pub(crate) fn for_encoder(&mut self, target: [u32; 2]) -> Option<Overlay> {
        if let Some(overlay) = &self.overlay
            && let Err(error) = overlay.validate(target)
        {
            tracing::warn!(error = %format!("{error:#}"), "couch menu: the panel waits for a redraw at the new size");
            self.overlay = None;
            // The next poll checks the panel again, whatever size it sees.
            self.target = [0, 0];
        }
        self.overlay.clone()
    }
    pub(crate) fn has_overlay(&self) -> bool {
        self.overlay.is_some()
    }
}

/// `POST /session/couch-menu`: opens or closes the menu in the caller's
/// stream. Returns whether it is open now.
pub(crate) fn request(h: &Shared, client: &Client, open: bool) -> Result<bool, Failure> {
    let s = own_stream(h, client)?;
    if !open {
        s.couch_menu.close();
        return Ok(false);
    }
    // The control peer stored what the stream allows as it started (off,
    // read-back, input-only, too small); until it connects the menu is
    // refused, so an open always has a filter and a worker behind it.
    s.couch_menu
        .open()
        .map_err(|reason| Failure::new(ErrorCode::TemporarilyUnavailable, reason))?;
    Ok(menu::Shared::is_open(s.couch_menu.load()))
}

/// The caller's live stream, a Stream before a monitor or input session: what the couch menu
/// and the audio settings (`native_api::audio`) act on.
pub(crate) fn own_stream(h: &Shared, client: &Client) -> Result<Arc<Session>, Refusal> {
    let sessions = h.sessions.lock().unwrap();
    let mut live = sessions
        .active
        .values()
        .filter(|s| !s.stopping())
        .peekable();
    if live.peek().is_none() {
        return Err(Refusal::NoSession);
    }
    let mut own: Vec<_> = live
        .filter(|s| s.launch.client.uuid == client.uuid)
        .collect();
    own.sort_by_key(|s| s.launch.role != Role::Stream);
    own.first().map(|s| Arc::clone(s)).ok_or(Refusal::NotOwner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::Fixture;
    use butterpollo_core::couch_menu::chord;
    use butterpollo_core::native::{LAUNCH, VIEW};

    #[test]
    fn couch_menu_refuses_input_only_readback_and_tiny_streams() {
        assert_eq!(refusal(Role::Stream, true, [1920, 1080]), None);
        assert_eq!(refusal(Role::RemoteMonitor, true, [1280, 720]), None);
        assert!(
            refusal(Role::InputOnly, true, [1920, 1080])
                .unwrap()
                .contains("input-only")
        );
        assert!(
            refusal(Role::Stream, false, [1920, 1080])
                .unwrap()
                .contains("wgc_direct_encoder_input")
        );
        assert_eq!(
            refusal(Role::Stream, true, [64, 64]).unwrap(),
            "couch menu: stream too small (64x64)"
        );
    }

    #[derive(Default)]
    struct FakeGame {
        log: Vec<String>,
    }
    impl Game for FakeGame {
        fn release_all(&mut self) {
            self.log.push("release".into());
        }
        fn apply_all(&mut self, events: &[Input]) -> Vec<anyhow::Error> {
            self.log.push(format!("apply {}", events.len()));
            vec![]
        }
        fn pointer(&mut self) -> Option<(f32, f32)> {
            None
        }
    }

    fn pad(buttons: u32) -> Input {
        Input::Controller {
            id: 0,
            active: 1,
            buttons,
            left_trigger: 0,
            right_trigger: 0,
            sticks: [0; 4],
        }
    }

    fn followed(f: &Fixture, role: Role) -> (Arc<Session>, PeerMenu) {
        let s = f.session(f.client(VIEW | LAUNCH), role);
        let mut peer = PeerMenu::default();
        let made = peer.follow(&s, || crate::stream::effective_config(&f.host, &s.launch));
        assert_eq!(made, role != Role::InputOnly);
        (s, peer)
    }

    #[test]
    fn couch_menu_chord_releases_the_game_once_before_the_kept_events() {
        let f = Fixture::new();
        let (s, mut peer) = followed(&f, Role::Stream);
        let mut game = FakeGame::default();
        let start = Instant::now();
        let mut events = vec![pad(chord::LS_CLK), pad(chord::PAD_CHORD)];
        let batch = apply_batch(&mut peer, Some(&mut game), &mut events, start);
        assert!(batch.nav.is_empty());
        assert_eq!(game.log, ["apply 2"]);
        // The hold completes on an empty batch at the due time.
        let due = start + chord::PAD_HOLD;
        assert_eq!(peer.next_due(), Some(due));
        let mut events = vec![];
        let batch = apply_batch(&mut peer, Some(&mut game), &mut events, due);
        assert_eq!(batch.nav, [Nav::Opened]);
        assert_eq!(game.log, ["apply 2", "release", "apply 0"]);
        assert!(menu::Shared::is_open(s.couch_menu.load()));
        // Nothing reaches the game while the menu is open.
        let mut events = vec![pad(0x1000)];
        apply_batch(
            &mut peer,
            Some(&mut game),
            &mut events,
            due + Duration::from_millis(10),
        );
        assert_eq!(game.log.last().unwrap(), "apply 0");
        // Closed from elsewhere, the peer tells the worker.
        s.couch_menu.close();
        let mut events = vec![];
        let batch = apply_batch(
            &mut peer,
            Some(&mut game),
            &mut events,
            due + Duration::from_millis(20),
        );
        assert_eq!(batch.nav, [Nav::Closed]);
    }

    #[test]
    fn couch_menu_off_or_input_only_makes_no_filter_and_forwards_everything() {
        let f = Fixture::new();
        f.host
            .config
            .write()
            .unwrap()
            .values
            .insert("couch_menu".into(), "false".into());
        let s = f.session(f.client(VIEW | LAUNCH), Role::Stream);
        let mut peer = PeerMenu::default();
        assert!(!peer.follow(&s, || crate::stream::effective_config(&f.host, &s.launch)));
        // The same session is followed once.
        assert!(!peer.follow(&s, || panic!("read again")));
        let mut game = FakeGame::default();
        let start = Instant::now();
        let mut events = vec![pad(chord::PAD_CHORD)];
        apply_batch(&mut peer, Some(&mut game), &mut events, start);
        assert_eq!(peer.next_due(), None);
        assert_eq!(game.log, ["apply 1"]);
        let (_, input_only) = followed(&f, Role::InputOnly);
        assert!(input_only.filter.is_none());
    }

    /// A panel laid out for a 1920x1080 stream.
    fn frame(origin: [u32; 2], size: [u32; 2]) -> Frame {
        Frame {
            origin,
            size,
            target: [1920, 1080],
            scale: 1,
            rgba: vec![255; (size[0] * size[1] * 4) as usize].into(),
        }
    }

    #[test]
    fn couch_menu_video_overlay_follows_the_shared_state() {
        let shared = menu::Shared::new();
        let mut video = VideoOverlay::default();
        video.poll(&shared, [1920, 1080]);
        assert!(!video.dirty && !video.has_overlay());
        shared.open().unwrap();
        video.poll(&shared, [1920, 1080]);
        // Open with nothing drawn yet: no panel.
        assert!(!video.dirty && !video.has_overlay());
        assert!(shared.publish(frame([64, 64], [256, 128])));
        video.poll(&shared, [1920, 1080]);
        assert!(video.dirty && video.has_overlay());
        video.dirty = false;
        video.poll(&shared, [1920, 1080]);
        assert!(!video.dirty);
        // After a rebuild to a size the panel misses, it waits for a redraw.
        assert!(video.for_encoder([1920, 1080]).is_some());
        assert!(video.for_encoder([128, 128]).is_none());
        assert!(shared.publish(frame([64, 64], [256, 128])));
        video.poll(&shared, [1920, 1080]);
        assert!(video.dirty && video.has_overlay());
        video.dirty = false;
        shared.close();
        video.poll(&shared, [1920, 1080]);
        assert!(video.dirty && !video.has_overlay());
    }

    #[test]
    fn couch_menu_panel_outside_the_stream_closes_the_menu() {
        let shared = menu::Shared::new();
        let mut video = VideoOverlay::default();
        shared.open().unwrap();
        assert!(shared.publish(frame([1792, 1024], [256, 128])));
        video.poll(&shared, [1920, 1080]);
        assert!(!video.has_overlay());
        assert!(!menu::Shared::is_open(shared.load()));
    }

    #[test]
    fn couch_menu_panel_for_another_mode_waits_instead_of_closing() {
        let shared = menu::Shared::new();
        let mut video = VideoOverlay::default();
        shared.open().unwrap();
        // The client asked for 4K and the worker drew for it, but the
        // encoder refused the mode and the stream stayed at 1080p.
        let uhd = Frame {
            target: [3840, 2160],
            ..frame([1792, 1024], [256, 128])
        };
        assert!(shared.publish(uhd));
        video.poll(&shared, [1920, 1080]);
        assert!(!video.dirty && !video.has_overlay());
        assert!(menu::Shared::is_open(shared.load()), "the menu stays open");
        // A mode the stream does take shows the same panel.
        video.poll(&shared, [3840, 2160]);
        assert!(video.dirty && video.has_overlay());
        video.dirty = false;
        // The worker redraws at the mode the stream kept.
        video.poll(&shared, [1920, 1080]);
        assert!(video.dirty && !video.has_overlay());
        video.dirty = false;
        assert!(shared.publish(frame([832, 464], [256, 128])));
        video.poll(&shared, [1920, 1080]);
        assert!(video.dirty && video.has_overlay());
        video.dirty = false;
        // A rebuild to a mode the encoder then refuses drops the panel; the
        // stream back at the old mode shows it again.
        assert!(video.for_encoder([960, 540]).is_none());
        video.poll(&shared, [1920, 1080]);
        assert!(video.dirty && video.has_overlay());
        assert!(menu::Shared::is_open(shared.load()));
    }

    #[test]
    fn couch_menu_strings_are_complete_in_english_and_german() {
        for locale in ["en", "de-DE", "fr"] {
            strings(locale, items::ITEMS).unwrap();
            for key in menu::model::items::ALL {
                let path = menu::model::items::label_path(key);
                let label = i18n::catalogue(locale, &path).unwrap();
                assert_eq!(menu::font::uncovered(&label), None, "{path}");
            }
        }
        assert_eq!(strings("de", &[]).unwrap().title, "Host-Menü");
        assert_eq!(strings("fr", &[]).unwrap().title, "Host menu");
    }

    #[test]
    fn couch_menu_request_needs_an_own_session_that_can_show_it() {
        let f = Fixture::new();
        let client = f.client(VIEW | LAUNCH);
        let code = |result: Result<bool, Failure>| result.unwrap_err().code();
        assert_eq!(code(request(&f.host, &client, true)), ErrorCode::NoSession);
        let other = Client {
            uuid: "other-device".into(),
            ..client.clone()
        };
        let s = f.session(client.clone(), Role::Stream);
        PeerMenu::default().follow(&s, || crate::stream::effective_config(&f.host, &s.launch));
        assert_eq!(
            code(request(&f.host, &other, true)),
            ErrorCode::NotSessionOwner
        );
        assert!(request(&f.host, &client, true).unwrap());
        assert!(request(&f.host, &client, true).unwrap(), "idempotent");
        assert!(!request(&f.host, &client, false).unwrap());
        assert!(!request(&f.host, &client, false).unwrap(), "idempotent");
    }

    #[test]
    fn couch_menu_off_at_stream_start_stays_off_for_the_stream() {
        for key in ["couch_menu", "wgc_direct_encoder_input"] {
            let f = Fixture::new();
            let client = f.client(VIEW | LAUNCH);
            let set = |value: &str| {
                f.host
                    .config
                    .write()
                    .unwrap()
                    .values
                    .insert(key.into(), value.into());
            };
            set("false");
            let s = f.session(client.clone(), Role::Stream);
            let mut peer = PeerMenu::default();
            peer.follow(&s, || crate::stream::effective_config(&f.host, &s.launch));
            // Turned on mid-stream: no filter, chord or worker would follow
            // an open, or the encoder would not draw it.
            set("true");
            assert_eq!(
                request(&f.host, &client, true).unwrap_err().code(),
                ErrorCode::TemporarilyUnavailable,
                "{key}"
            );
            assert!(!menu::Shared::is_open(s.couch_menu.load()), "{key}");
            // Closing works with the menu off.
            assert!(!request(&f.host, &client, false).unwrap());
        }
    }

    #[test]
    fn couch_menu_request_waits_for_the_control_peer() {
        let f = Fixture::new();
        let client = f.client(VIEW | LAUNCH);
        let s = f.session(client.clone(), Role::Stream);
        assert_eq!(
            request(&f.host, &client, true).unwrap_err().code(),
            ErrorCode::TemporarilyUnavailable
        );
        assert!(!menu::Shared::is_open(s.couch_menu.load()));
        let mut peer = PeerMenu::default();
        assert!(peer.follow(&s, || crate::stream::effective_config(&f.host, &s.launch)));
        assert!(request(&f.host, &client, true).unwrap());
        // On at stream start, a later change waits for the next stream.
        f.host
            .config
            .write()
            .unwrap()
            .values
            .insert("couch_menu".into(), "false".into());
        assert!(!request(&f.host, &client, false).unwrap());
        assert!(request(&f.host, &client, true).unwrap());
    }

    #[test]
    fn couch_menu_request_refuses_an_input_only_session() {
        let f = Fixture::new();
        let client = f.client(VIEW | LAUNCH);
        let s = f.session(client.clone(), Role::InputOnly);
        assert!(!PeerMenu::default().follow(&s, || panic!("an input-only stream reads no config")));
        let failure = request(&f.host, &client, true).unwrap_err();
        assert_eq!(failure.code(), ErrorCode::TemporarilyUnavailable);
        assert!(
            failure.message().contains("input-only"),
            "{}",
            failure.message()
        );
    }

    #[test]
    fn couch_menu_worker_draws_the_open_menu_and_closes_on_back() {
        let f = Fixture::new();
        let (s, _peer) = followed(&f, Role::Stream);
        let mut workers = Workers::default();
        s.couch_menu.open().unwrap();
        workers.send(&f.host, &s, vec![Nav::Opened]);
        let drawn = |want: bool| {
            let until = Instant::now() + Duration::from_secs(5);
            while s.couch_menu.frame().is_some() != want && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(5));
            }
            s.couch_menu.frame().is_some() == want
        };
        assert!(drawn(true), "the worker drew the main page");
        let mode = s.stream_mode();
        let panel = s.couch_menu.frame().unwrap();
        Overlay::new(1, &panel)
            .validate([mode.width, mode.height])
            .unwrap();
        // Back on the main page is Back to game.
        workers.send(&f.host, &s, vec![Nav::Back]);
        let until = Instant::now() + Duration::from_secs(5);
        while menu::Shared::is_open(s.couch_menu.load()) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!menu::Shared::is_open(s.couch_menu.load()));
        assert!(drawn(false), "a closed menu has no panel");
        workers.retain(|_| false);
    }
}
