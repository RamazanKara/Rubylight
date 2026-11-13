//! The in-stream "Session and graphics" page for a host with the Rubylight native API (the
//! desktop counterpart of Nightjar Android's stream menu page): End session, Close game or Quit
//! game, the connection facts, the host's own menu when it offers one, and the Radeon panel.
//!
//! Everything that touches the network runs on one worker thread that exists only while the
//! page is open: it reads `/session` about every 2 s and `/radeon` at most once a second (the
//! next read starts a second after the previous one finished), and sends the writes and the
//! exit actions. One thread means a write never overlaps a read and answers never arrive out of
//! order. The render, decode and input threads never wait for it: the UI thread only copies the
//! last published [`State`], and the worker wakes the UI loop when it changes. Dropping the
//! [`Page`] ends the thread (after the call in flight), so nothing polls while the page is closed.
//!
//! The API behind the worker is the [`Api`] trait, so the whole behaviour is tested with a fake.

use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nightjar_core::http::native_api::capabilities::Capabilities;
use nightjar_core::http::native_api::error::ErrorCode;
use nightjar_core::http::native_api::radeon::{Change, Radeon, SettingsWrite, Value};
use nightjar_core::http::native_api::feature;
use nightjar_core::http::native_api::session::{ActionId, CouchMenu, Session, State as SessionState};
use nightjar_core::http::{Error, HostClient};

use crate::hosts::Waker;
use crate::strings::{en, tr, trf};
use crate::radeon::{is_missing, Failure, FailureKind, Mode, Panel, Row};

/// How often `/session` is read while the page is open.
pub const SESSION_INTERVAL: Duration = Duration::from_secs(2);
/// The pause between the end of one `/radeon` read and the start of the next.
pub const RADEON_INTERVAL: Duration = Duration::from_secs(1);

/// The calls of the native API the page makes.
pub trait Api: Send + 'static {
    fn capabilities(&self) -> Result<Capabilities, Error>;
    fn session(&self) -> Result<Session, Error>;
    fn session_action(&self, action: ActionId) -> Result<Session, Error>;
    /// The classic `/cancel`: Ok(false) when the host declined.
    fn quit_app(&self) -> Result<bool, Error>;
    fn radeon(&self) -> Result<Radeon, Error>;
    fn write_radeon(&self, write: &SettingsWrite) -> Result<Radeon, Error>;
}

impl Api for HostClient {
    fn capabilities(&self) -> Result<Capabilities, Error> {
        self.native_capabilities()
    }
    fn session(&self) -> Result<Session, Error> {
        HostClient::session(self)
    }
    fn session_action(&self, action: ActionId) -> Result<Session, Error> {
        HostClient::session_action(self, action)
    }
    fn quit_app(&self) -> Result<bool, Error> {
        self.cancel()
    }
    fn radeon(&self) -> Result<Radeon, Error> {
        HostClient::radeon(self)
    }
    fn write_radeon(&self, write: &SettingsWrite) -> Result<Radeon, Error> {
        HostClient::write_radeon(self, write)
    }
}

/// What ends the stream from the page. Disconnect is not one of these: it leaves the game and
/// the session as they are (the host keeps them suspended) and is the stream's own exit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostExit {
    /// The native End: the stream and this device's display end, the game keeps running.
    End,
    /// The native Close game (needs Host control): End, and the game ends too.
    CloseGame,
    /// The classic `/cancel`, for a device without Host control: it quits the game.
    QuitApp,
}

impl HostExit {
    pub fn label(self) -> &'static str {
        match self {
            HostExit::End => en("End session"),
            HostExit::CloseGame => en("Quit game"),
            HostExit::QuitApp => en("Quit game"),
        }
    }

    pub fn confirm(self) -> &'static str {
        match self {
            HostExit::End => en("End the session on the PC?\nThe PC gets its screen and sound back. The game keeps running there."),
            HostExit::CloseGame => en("Close the game on the PC?\nUnsaved progress will be lost."),
            HostExit::QuitApp => en("Quit the game on the PC?\nUnsaved progress will be lost."),
        }
    }
}

/// One session action row: what it does, and why it cannot be used right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub exit: HostExit,
    pub blocked: Option<&'static str>,
}

/// The session actions this device may offer. Resume and Disconnect are always there and are not
/// listed. End session and the game exit need the Launch permission; the game exit is Close game
/// with Host control and the classic Quit game without it. A row the host says is not allowed now
/// stays, with the reason; a session not read yet blocks nothing (the host decides).
pub fn actions(launch: bool, host_control: bool, session: Option<&Session>) -> Vec<Entry> {
    if !launch {
        return Vec::new();
    }
    let game = if host_control { HostExit::CloseGame } else { HostExit::QuitApp };
    [HostExit::End, game]
        .into_iter()
        .map(|exit| Entry { exit, blocked: session.and_then(|s| blocked(exit, s)) })
        .collect()
}

fn blocked(exit: HostExit, session: &Session) -> Option<&'static str> {
    let id = match exit {
        HostExit::End => ActionId::End,
        HostExit::CloseGame => ActionId::CloseGame,
        // The classic quit is not a native action: the host's /cancel answers for itself.
        HostExit::QuitApp => return None,
    };
    let action = session.actions.iter().find(|a| a.id == id)?;
    if action.allowed {
        return None;
    }
    Some(match action.reason {
        Some(ErrorCode::SessionBusy) | Some(ErrorCode::TemporarilyUnavailable) => en("The PC is busy. Try again in a moment."),
        Some(ErrorCode::NotSessionOwner) => en("This session belongs to another device."),
        Some(ErrorCode::NoSession) => en("There is no session to end."),
        Some(ErrorCode::HostControlRequired) => en("Turn on Host control for this device in the PC’s web UI."),
        _ if session.state == SessionState::Stopping => en("The session is already ending."),
        _ => en("Not available right now."),
    })
}

/// Why an exit action failed, as the page shows it.
pub fn exit_failure_text(failure: &Failure) -> String {
    match failure.kind {
        FailureKind::HostControl => tr("Turn on Host control for this device in the PC’s web UI to close games.").into(),
        FailureKind::NoSession => tr("This device does not own the session on the PC.").into(),
        FailureKind::Busy => tr("The PC is busy. Try again in a moment.").into(),
        FailureKind::Unreachable => tr("Cannot reach the PC.").into(),
        _ => match &failure.host_message {
            Some(message) if !message.is_empty() => message.clone(),
            _ => tr("The PC did not do it.").into(),
        },
    }
}

/// What opening the host's menu came to. The stream has closed its own menu before the request,
/// so the host's menu gets the input; a refusal is the host's message (its code decides the
/// text on the host), and an answer that the menu is still closed is a failure too.
pub fn host_menu_result(answer: Result<CouchMenu, Error>) -> Result<(), String> {
    match answer {
        Ok(CouchMenu { open: true }) => Ok(()),
        Ok(CouchMenu { open: false }) => Err(tr("The PC did not open its menu.").into()),
        Err(error) => Err(trf("The PC’s menu did not open: {}", &[&error])),
    }
}

/// The facts shown under the actions, from what the stream already holds.
pub fn connection_lines(
    host: &str,
    codec: &str,
    size: (i32, i32, i32),
    bitrate_kbps: i32,
    link: &str,
    network: Option<&str>,
) -> Vec<(&'static str, String)> {
    let mbps = f64::from(bitrate_kbps) / 1000.0;
    let mut lines = vec![
        (en("PC"), host.to_owned()),
        (en("Codec"), codec.to_owned()),
        (en("Resolution"), trf("{} × {} at {} fps", &[&size.0, &size.1, &size.2])),
        (en("Bitrate"), if bitrate_kbps % 1000 == 0 { format!("{mbps:.0} Mbps") } else { format!("{mbps:.1} Mbps") }),
    ];
    if !link.is_empty() {
        lines.push((en("Link"), link.to_owned()));
    }
    if let Some(network) = network {
        lines.push((en("Connection"), network.to_owned()));
    }
    lines
}

/// When the next reads are due. The Radeon read starts a second after the previous one
/// finished, the session read two; a host without Radeon endpoints is not asked again.
pub struct Gate {
    radeon: Duration,
    session: Duration,
    radeon_done: Option<Instant>,
    session_done: Option<Instant>,
    radeon_off: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Due {
    pub session: bool,
    pub radeon: bool,
}

impl Gate {
    pub fn new(radeon: Duration, session: Duration) -> Self {
        Self { radeon, session, radeon_done: None, session_done: None, radeon_off: false }
    }

    pub fn due(&self, now: Instant) -> Due {
        Due {
            session: self.session_done.is_none_or(|t| now.duration_since(t) >= self.session),
            radeon: !self.radeon_off && self.radeon_done.is_none_or(|t| now.duration_since(t) >= self.radeon),
        }
    }

    /// How long until something is due (zero when something is).
    pub fn wait(&self, now: Instant) -> Duration {
        let left = |done: Option<Instant>, every: Duration| match done {
            None => Duration::ZERO,
            Some(t) => every.saturating_sub(now.duration_since(t)),
        };
        let session = left(self.session_done, self.session);
        if self.radeon_off { session } else { session.min(left(self.radeon_done, self.radeon)) }
    }

    pub fn session_read(&mut self, now: Instant) {
        self.session_done = Some(now);
    }

    pub fn radeon_read(&mut self, now: Instant) {
        self.radeon_done = Some(now);
    }

    /// The Radeon panel should be read again at once (after a failed write).
    pub fn radeon_now(&mut self) {
        self.radeon_done = None;
    }

    pub fn radeon_missing(&mut self) {
        self.radeon_off = true;
    }
}

/// What the Radeon panel shows right now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RadeonState {
    /// The last host answer, or None before the first one.
    pub panel: Option<Panel>,
    /// The host has no Radeon endpoints: draw no panel.
    pub hidden: bool,
    /// Why there is no panel yet (the host could not be reached).
    pub load_failure: Option<Failure>,
    /// The last write that failed, shown at its setting.
    pub write_failure: Option<Failure>,
    /// The setting a write is in flight for.
    pub pending: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExitState {
    Idle,
    Running(HostExit),
    Failed(HostExit, Failure),
    /// The host did it: the stream should end.
    Done(HostExit),
}

#[derive(Clone, Debug)]
pub struct State {
    /// What this device may do, from `/capabilities`; None until read.
    pub launch: Option<bool>,
    pub host_control: Option<bool>,
    /// The host draws its own menu into the stream (`session.couch_menu`); None until read.
    pub couch_menu: Option<bool>,
    pub session: Option<Session>,
    pub session_error: Option<String>,
    pub radeon: RadeonState,
    pub exit: ExitState,
    /// The worker could not start: no client for this host.
    pub setup_error: Option<String>,
}

impl State {
    fn new() -> Self {
        Self {
            launch: None,
            host_control: None,
            couch_menu: None,
            session: None,
            session_error: None,
            radeon: RadeonState::default(),
            exit: ExitState::Idle,
            setup_error: None,
        }
    }

    /// Host control as far as known: the Radeon read's flag is the newest, then `/capabilities`.
    pub fn host_control(&self, hint: bool) -> bool {
        self.radeon.panel.as_ref().map(|p| p.host_control).or(self.host_control).unwrap_or(hint)
    }

    /// Whether to offer the host's menu: only when the capabilities list it.
    pub fn host_menu(&self) -> bool {
        self.couch_menu == Some(true)
    }

    /// The session actions to offer now.
    pub fn actions(&self, hints: Hints) -> Vec<Entry> {
        actions(self.launch.unwrap_or(hints.launch), self.host_control(hints.host_control), self.session.as_ref())
    }

    /// Whether a write of `row` may be sent now: the panel allows writes, the row is editable
    /// and no other write is in flight.
    pub fn can_write(&self, row: &Row) -> bool {
        self.radeon.pending.is_none()
            && !matches!(self.exit, ExitState::Running(_))
            && row.mode == Mode::Edit
            && self.radeon.panel.as_ref().is_some_and(Panel::can_write)
    }
}

/// What is known about this device before the page has read anything.
#[derive(Clone, Copy, Debug)]
pub struct Hints {
    pub launch: bool,
    pub host_control: bool,
}

enum Request {
    Write { gpu: i32, id: String, value: Value },
    Exit(HostExit),
}

/// The open page. Dropping it stops the worker.
pub struct Page {
    shared: Arc<Mutex<State>>,
    requests: Sender<Request>,
    /// The exit the user has been asked to confirm.
    pub confirming: Option<HostExit>,
}

impl Page {
    /// Starts the worker. `make` builds the API client on the worker thread.
    pub fn open<A: Api>(make: impl FnOnce() -> Result<A, String> + Send + 'static, wake: Waker) -> Page {
        Self::open_with(make, wake, SESSION_INTERVAL, RADEON_INTERVAL)
    }

    pub fn open_with<A: Api>(
        make: impl FnOnce() -> Result<A, String> + Send + 'static,
        wake: Waker,
        session_every: Duration,
        radeon_every: Duration,
    ) -> Page {
        let shared = Arc::new(Mutex::new(State::new()));
        let (requests, inbox) = channel();
        let worker = Worker { shared: shared.clone(), wake, gate: Gate::new(radeon_every, session_every) };
        let spawned = std::thread::Builder::new().name("session-page".into()).spawn(move || match make() {
            Ok(api) => worker.run(api, inbox),
            Err(error) => worker.update(|s| {
                // An exit asked for before the worker gave up would show as running for ever.
                if let ExitState::Running(exit) = s.exit {
                    s.exit = ExitState::Failed(exit, Failure::of(&Error::InvalidArgument(error.clone()), None));
                }
                s.setup_error = Some(error);
            }),
        });
        if spawned.is_err() {
            lock(&shared).setup_error = Some(tr("The page could not start its worker.").into());
        }
        Page { shared, requests, confirming: None }
    }

    pub fn state(&self) -> State {
        lock(&self.shared).clone()
    }

    /// Asks for one Radeon change. Returns false (and sends nothing) when the panel does not
    /// allow it now: not writable, the row is not editable, or another write is in flight.
    pub fn write(&self, row: &Row, value: Value) -> bool {
        let mut state = lock(&self.shared);
        if !state.can_write(row) {
            return false;
        }
        state.radeon.pending = Some(row.id.clone());
        state.radeon.write_failure = None;
        drop(state);
        self.requests.send(Request::Write { gpu: row.gpu, id: row.id.clone(), value }).is_ok()
    }

    /// An exit is running: the page must stay until the host has answered, so a failure shows.
    pub fn busy(&self) -> bool {
        matches!(lock(&self.shared).exit, ExitState::Running(_))
    }

    /// Runs End session, Close game or Quit game. Returns false when one is already running.
    pub fn exit(&self, exit: HostExit) -> bool {
        let mut state = lock(&self.shared);
        if matches!(state.exit, ExitState::Running(_)) {
            return false;
        }
        state.exit = ExitState::Running(exit);
        drop(state);
        if self.requests.send(Request::Exit(exit)).is_err() {
            // The worker is gone (it could not start): the exit fails with why, it never runs.
            let mut state = lock(&self.shared);
            let why = state.setup_error.clone().unwrap_or_else(|| tr("The page could not start its worker.").into());
            state.exit = ExitState::Failed(exit, Failure::of(&Error::InvalidArgument(why), None));
            return false;
        }
        true
    }
}

fn lock(shared: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

struct Worker {
    shared: Arc<Mutex<State>>,
    wake: Waker,
    gate: Gate,
}

impl Worker {
    fn update(&self, change: impl FnOnce(&mut State)) {
        change(&mut lock(&self.shared));
        (self.wake)();
    }

    fn run<A: Api>(mut self, api: A, inbox: std::sync::mpsc::Receiver<Request>) {
        // A failed read is not fatal: the reads below say more, and the page works from its hints.
        if let Ok(caps) = api.capabilities() {
            self.update(|s| {
                s.launch = Some(caps.permissions.launch);
                s.host_control = Some(caps.permissions.host_control);
                s.couch_menu = Some(caps.features.iter().any(|f| f == feature::COUCH_MENU));
            });
        }
        loop {
            let due = self.gate.due(Instant::now());
            if due.session || due.radeon {
                if due.session {
                    self.read_session(&api);
                }
                if due.radeon {
                    self.read_radeon(&api);
                }
                continue;
            }
            match inbox.recv_timeout(self.gate.wait(Instant::now())) {
                Ok(Request::Write { gpu, id, value }) => self.write(&api, gpu, id, value),
                Ok(Request::Exit(exit)) => self.exit(&api, exit),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    fn read_session<A: Api>(&mut self, api: &A) {
        let result = api.session();
        self.gate.session_read(Instant::now());
        self.update(|s| match result {
            Ok(session) => {
                s.session = Some(session);
                s.session_error = None;
            }
            // A failed read keeps what is on screen; the next one tries again.
            Err(error) => s.session_error = Some(error.to_string()),
        });
    }

    fn read_radeon<A: Api>(&mut self, api: &A) {
        let result = api.radeon();
        self.gate.radeon_read(Instant::now());
        if result.as_ref().is_err_and(is_missing) {
            self.gate.radeon_missing();
        }
        self.update(|s| match result {
            Ok(radeon) => {
                let panel = Panel::of(&radeon);
                s.radeon.hidden = false;
                s.radeon.load_failure = None;
                s.radeon.panel = Some(panel);
            }
            Err(error) if is_missing(&error) => s.radeon = RadeonState { hidden: true, ..RadeonState::default() },
            Err(error) => {
                // With a panel on screen, a failed read keeps it; the next read tries again.
                if s.radeon.panel.is_none() {
                    s.radeon.load_failure = Some(Failure::of(&error, None));
                }
            }
        });
    }

    fn write<A: Api>(&mut self, api: &A, gpu: i32, id: String, value: Value) {
        let write = SettingsWrite { gpu, changes: vec![Change { id: id.clone(), value }] };
        let result = api.write_radeon(&write);
        let failed = result.is_err();
        if result.as_ref().is_err_and(is_missing) {
            self.gate.radeon_missing();
        }
        if failed {
            // The value on the host may differ from what the panel shows: read it back now.
            self.gate.radeon_now();
        } else {
            self.gate.radeon_read(Instant::now());
        }
        self.update(|s| {
            s.radeon.pending = None;
            match result {
                Ok(radeon) => {
                    s.radeon.panel = Some(Panel::of(&radeon));
                    s.radeon.write_failure = None;
                }
                Err(error) if is_missing(&error) => s.radeon = RadeonState { hidden: true, ..RadeonState::default() },
                Err(error) => s.radeon.write_failure = Some(Failure::of(&error, Some(&id))),
            }
        });
    }

    fn exit<A: Api>(&mut self, api: &A, exit: HostExit) {
        let result = match exit {
            HostExit::End => api.session_action(ActionId::End).map(|_| ()),
            HostExit::CloseGame => api.session_action(ActionId::CloseGame).map(|_| ()),
            HostExit::QuitApp => match api.quit_app() {
                Ok(true) => Ok(()),
                Ok(false) => Err(Error::InvalidArgument(tr("The PC declined to quit the game.").into())),
                Err(Error::Status { code: 599, .. }) => {
                    Err(Error::InvalidArgument(tr("The running game belongs to another device.").into()))
                }
                Err(error) => Err(error),
            },
        };
        self.update(|s| {
            s.exit = match result {
                Ok(()) => ExitState::Done(exit),
                Err(error) => ExitState::Failed(exit, Failure::of(&error, None)),
            }
        });
    }
}

#[cfg(test)]
mod tests;
