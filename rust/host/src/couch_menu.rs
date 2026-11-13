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

/// The session's device as the console has it now, as the screen the session streams to: a
/// second screen's menu ends or disconnects its own stream, not the first screen's.
fn client(h: &Shared, s: &Session) -> Result<Client, Failure> {
    h.paired
        .read()
        .unwrap()
        .clients
        .iter()
        .find(|client| {
            client.enabled
                && client.uuid == butterpollo_core::state::device_uuid(&s.launch.client.uuid)
        })
        .cloned()
        .map(|client| {
            if client.uuid == s.launch.client.uuid {
                client
            } else {
                client.second_screen()
            }
        })
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
        Action::RunCommand { index, name } => {
            // The PC commands permission, as for the device's own stream menu.
            if !client(h, s)?.allows(1 << 20) {
                return Err(Failure::new(
                    ErrorCode::PermissionRequired,
                    "This device may not run PC commands.",
                ));
            }
            let run =
                crate::process::pc_command(h, *index, Some(name.as_str())).ok_or_else(|| {
                    Failure::new(
                        ErrorCode::InvalidRequest,
                        "The command changed or this game turned PC commands off.",
                    )
                })?;
            run.start().map_err(|error| {
                tracing::warn!(%error, command = %name, "couch menu PC command failed");
                Failure::new(ErrorCode::Internal, "The command could not be started.")
            })
        }
        Action::Change { .. } | Action::RestoreRadeon { .. } => Err(Failure::new(
            ErrorCode::TemporarilyUnavailable,
            "Graphics card settings are not in this menu.",
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
    let device = client(h, s).ok();
    let host_control = device
        .as_ref()
        .is_some_and(butterpollo_core::native::host_control);
    // The same list, in the same order, the device's own stream menu shows.
    let commands = if device.as_ref().is_some_and(|client| client.allows(1 << 20)) {
        let config = h.config.read().unwrap().clone();
        crate::process::pc_command_names(h, &config)
    } else {
        Vec::new()
    };
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
        commands,
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
/// acts on.
fn own_stream(h: &Shared, client: &Client) -> Result<Arc<Session>, Refusal> {
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
mod tests;
