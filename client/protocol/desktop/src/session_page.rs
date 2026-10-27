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
            HostExit::CloseGame => en("Close game"),
            HostExit::QuitApp => en("Quit game"),
        }
    }

    pub fn confirm(self) -> &'static str {
        match self {
            HostExit::End => en("End the stream session on the PC?\nThe game keeps running."),
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
        Some(ErrorCode::SessionBusy) | Some(ErrorCode::TemporarilyUnavailable) => en("The host is busy. Try again in a moment."),
        Some(ErrorCode::NotSessionOwner) => en("This session belongs to another device."),
        Some(ErrorCode::NoSession) => en("There is no session to end."),
        Some(ErrorCode::HostControlRequired) => en("Turn on Host control for this device on the host."),
        _ if session.state == SessionState::Stopping => en("The session is already ending."),
        _ => en("Not available right now."),
    })
}

/// Why an exit action failed, as the page shows it.
pub fn exit_failure_text(failure: &Failure) -> String {
    match failure.kind {
        FailureKind::HostControl => tr("Turn on Host control for this device on the host to close games.").into(),
        FailureKind::NoSession => tr("This device does not own the session on the PC.").into(),
        FailureKind::Busy => tr("The host is busy. Try again in a moment.").into(),
        FailureKind::Unreachable => tr("Cannot reach the host.").into(),
        _ => match &failure.host_message {
            Some(message) if !message.is_empty() => message.clone(),
            _ => tr("The host did not do it.").into(),
        },
    }
}

/// What opening the host's menu came to. The stream has closed its own menu before the request,
/// so the host's menu gets the input; a refusal is the host's message (its code decides the
/// text on the host), and an answer that the menu is still closed is a failure too.
pub fn host_menu_result(answer: Result<CouchMenu, Error>) -> Result<(), String> {
    match answer {
        Ok(CouchMenu { open: true }) => Ok(()),
        Ok(CouchMenu { open: false }) => Err(tr("The host did not open its menu.").into()),
        Err(error) => Err(trf("The host menu did not open: {}", &[&error])),
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
        (en("Host"), host.to_owned()),
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
            Err(error) => worker.update(|s| s.setup_error = Some(error)),
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
        self.requests.send(Request::Exit(exit)).is_ok()
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
mod tests {
    use super::*;
    use nightjar_core::http::native_api::error::ApiError;
    use nightjar_core::http::native_api::session::{Action, Owner};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn session(state: SessionState, actions: &[(ActionId, bool, Option<ErrorCode>)]) -> Session {
        Session {
            revision: 1,
            state,
            owner: Owner::You,
            app: None,
            game_running: true,
            since_unix_ms: None,
            suspended_until_unix_ms: None,
            launcher: None,
            radeon_profile: None,
            actions: actions.iter().map(|&(id, allowed, reason)| Action { id, allowed, reason }).collect(),
        }
    }

    fn exits(entries: &[Entry]) -> Vec<HostExit> {
        entries.iter().map(|e| e.exit).collect()
    }

    #[test]
    fn close_game_needs_host_control_and_quit_game_is_the_fallback() {
        let with = actions(true, true, None);
        assert_eq!(exits(&with), [HostExit::End, HostExit::CloseGame]);
        let without = actions(true, false, None);
        assert_eq!(exits(&without), [HostExit::End, HostExit::QuitApp]);
        assert!(!exits(&without).contains(&HostExit::CloseGame), "never offered without host control");
        // No Launch permission: neither of them is offered.
        assert!(actions(false, true, None).is_empty());
        assert!(actions(false, false, None).is_empty());
        assert!(with.iter().chain(&without).all(|e| e.blocked.is_none()), "nothing is blocked before the host speaks");
    }

    #[test]
    fn nothing_is_offered_until_the_permissions_are_read() {
        let unknown = Hints { launch: false, host_control: false };
        let mut state = State::new();
        assert!(state.actions(unknown).is_empty(), "no Launch permission known: no exits");
        state.launch = Some(true);
        assert_eq!(exits(&state.actions(unknown)), [HostExit::End, HostExit::QuitApp]);
        state.host_control = Some(true);
        assert_eq!(exits(&state.actions(unknown)), [HostExit::End, HostExit::CloseGame]);
        state.launch = Some(false);
        assert!(state.actions(unknown).is_empty());
    }

    #[test]
    fn the_host_can_block_an_action_with_a_reason() {
        let busy = session(
            SessionState::Launching,
            &[(ActionId::End, false, Some(ErrorCode::SessionBusy)), (ActionId::CloseGame, false, Some(ErrorCode::SessionBusy))],
        );
        let entries = actions(true, true, Some(&busy));
        assert!(entries.iter().all(|e| e.blocked == Some("The host is busy. Try again in a moment.")));
        let other = session(SessionState::Streaming, &[(ActionId::End, false, Some(ErrorCode::NotSessionOwner))]);
        assert_eq!(actions(true, false, Some(&other))[0].blocked, Some("This session belongs to another device."));
        // The classic quit is not a native action: the session's list never blocks it.
        assert_eq!(actions(true, false, Some(&other))[1].blocked, None);
        let ready = session(SessionState::Streaming, &[(ActionId::End, true, None), (ActionId::CloseGame, true, None)]);
        assert!(actions(true, true, Some(&ready)).iter().all(|e| e.blocked.is_none()));
    }

    #[test]
    fn what_each_exit_keeps_is_said_before_it_runs() {
        assert!(HostExit::End.confirm().contains("The game keeps running"));
        assert!(HostExit::CloseGame.confirm().contains("Unsaved progress will be lost"));
        assert!(HostExit::QuitApp.confirm().contains("Quit the game"));
        assert_eq!(HostExit::End.label(), "End session");
    }

    #[test]
    fn connection_facts_read_like_android() {
        let lines = connection_lines("GAMING-PC", "HEVC", (1920, 1080, 60), 20_000, "Home Wi-Fi", Some("4.0 ms RTT (±1.0), 0.0% frames lost"));
        assert_eq!(lines[0], ("Host", "GAMING-PC".to_owned()));
        assert_eq!(lines[2].1, "1920 × 1080 at 60 fps");
        assert_eq!(lines[3].1, "20 Mbps");
        assert_eq!(lines[4], ("Link", "Home Wi-Fi".to_owned()));
        assert_eq!(lines[5].0, "Connection");
        let lines = connection_lines("PC", "AV1", (2560, 1440, 120), 12_500, "", None);
        assert_eq!(lines[3].1, "12.5 Mbps");
        assert_eq!(lines.len(), 4);
    }

    #[test]
    fn polling_is_gated_by_the_intervals_and_stops_for_a_host_without_radeon() {
        let start = Instant::now();
        let mut gate = Gate::new(RADEON_INTERVAL, SESSION_INTERVAL);
        assert_eq!(gate.due(start), Due { session: true, radeon: true }, "the first reads are due at once");
        gate.session_read(start);
        gate.radeon_read(start);
        let at = |ms| start + Duration::from_millis(ms);
        assert_eq!(gate.due(at(500)), Due { session: false, radeon: false });
        assert_eq!(gate.wait(at(500)), Duration::from_millis(500));
        // Radeon: once a second at most. Session: every two seconds.
        assert_eq!(gate.due(at(999)), Due { session: false, radeon: false });
        assert_eq!(gate.due(at(1000)), Due { session: false, radeon: true });
        gate.radeon_read(at(1000));
        assert_eq!(gate.due(at(1999)), Due { session: false, radeon: false });
        assert_eq!(gate.due(at(2000)), Due { session: true, radeon: true });
        // A read that took a while counts from when it finished.
        gate.radeon_read(at(2300));
        assert!(!gate.due(at(3200)).radeon);
        assert!(gate.due(at(3300)).radeon);
        // A failed write brings the next read forward.
        gate.radeon_now();
        assert!(gate.due(at(2301)).radeon);
        // A host without Radeon endpoints is not asked again; the session still is.
        gate.radeon_missing();
        gate.session_read(at(4000));
        assert_eq!(gate.due(at(9000)), Due { session: true, radeon: false });
        assert_eq!(gate.wait(at(4500)), Duration::from_millis(1500));
    }

    // A host that counts what it is asked, with the doc's Radeon answer.
    struct Fake {
        reads: Arc<AtomicUsize>,
        read_times: Arc<Mutex<Vec<Instant>>>,
        sessions: Arc<AtomicUsize>,
        writes: Arc<Mutex<Vec<SettingsWrite>>>,
        actions: Arc<Mutex<Vec<ActionId>>>,
        quits: Arc<AtomicUsize>,
        radeon: Arc<Mutex<Option<Radeon>>>,
        write_error: Option<ErrorCode>,
        write_missing: bool,
        quit: Result<bool, u16>,
        /// The capabilities' features.
        features: Vec<String>,
    }

    struct Counters {
        reads: Arc<AtomicUsize>,
        read_times: Arc<Mutex<Vec<Instant>>>,
        sessions: Arc<AtomicUsize>,
        writes: Arc<Mutex<Vec<SettingsWrite>>>,
        actions: Arc<Mutex<Vec<ActionId>>>,
        quits: Arc<AtomicUsize>,
    }

    fn radeon_json(writable: bool) -> Radeon {
        serde_json::from_str(&format!(
            r#"{{ "revision": 1, "state": "configurable", "host_control": true, "writable_now": {writable},
              "gpus": [{{ "id": 5, "name": "AMD Radeon", "driver": "1", "settings": [
                {{ "id": "anti_lag.enabled", "group": "anti_lag", "kind": "toggle", "state": "configurable", "value": true }},
                {{ "id": "chill.max_fps", "group": "chill", "kind": "integer", "state": "configurable", "value": 100,
                   "range": {{ "min": 30, "max": 300, "step": 10 }} }}] }}] }}"#
        ))
        .unwrap()
    }

    fn fake(writable: bool) -> (Fake, Counters) {
        let counters = Counters {
            reads: Arc::default(),
            read_times: Arc::default(),
            sessions: Arc::default(),
            writes: Arc::default(),
            actions: Arc::default(),
            quits: Arc::default(),
        };
        let fake = Fake {
            reads: counters.reads.clone(),
            read_times: counters.read_times.clone(),
            sessions: counters.sessions.clone(),
            writes: counters.writes.clone(),
            actions: counters.actions.clone(),
            quits: counters.quits.clone(),
            radeon: Arc::new(Mutex::new(Some(radeon_json(writable)))),
            write_error: None,
            write_missing: false,
            quit: Ok(true),
            features: Vec::new(),
        };
        (fake, counters)
    }

    fn api_error(status: u16, code: ErrorCode, setting: Option<&str>) -> Error {
        Error::Api { status, error: ApiError { code, message: "host words".into(), setting: setting.map(str::to_owned), retry_after_ms: None } }
    }

    impl Api for Fake {
        fn capabilities(&self) -> Result<Capabilities, Error> {
            let mut caps: Capabilities = serde_json::from_str(
                r#"{ "api_version": 1, "host": { "name": "PC", "version": "3", "unique_id": "x" }, "features": [],
                     "permissions": { "list": true, "view": true, "launch": true, "host_control": true } }"#,
            )
            .unwrap();
            caps.features = self.features.clone();
            Ok(caps)
        }
        fn session(&self) -> Result<Session, Error> {
            self.sessions.fetch_add(1, Ordering::SeqCst);
            Ok(session(SessionState::Streaming, &[(ActionId::End, true, None), (ActionId::CloseGame, true, None)]))
        }
        fn session_action(&self, action: ActionId) -> Result<Session, Error> {
            self.actions.lock().unwrap().push(action);
            match self.write_error {
                Some(code) => Err(api_error(403, code, None)),
                None => Ok(session(SessionState::Stopping, &[])),
            }
        }
        fn quit_app(&self) -> Result<bool, Error> {
            self.quits.fetch_add(1, Ordering::SeqCst);
            self.quit.map_err(|code| Error::Status { code: i32::from(code), message: String::new() })
        }
        fn radeon(&self) -> Result<Radeon, Error> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.read_times.lock().unwrap().push(Instant::now());
            self.radeon.lock().unwrap().clone().ok_or_else(|| Error::NotFound("radeon".into()))
        }
        fn write_radeon(&self, write: &SettingsWrite) -> Result<Radeon, Error> {
            self.writes.lock().unwrap().push(write.clone());
            if self.write_missing {
                return Err(Error::NotFound("radeon".into()));
            }
            match self.write_error {
                Some(code) => Err(api_error(409, code, Some("chill.max_fps"))),
                None => Ok(radeon_json(true)),
            }
        }
    }

    fn open(fake: Fake, session_ms: u64, radeon_ms: u64) -> Page {
        Page::open_with(move || Ok(fake), Arc::new(|| {}), Duration::from_millis(session_ms), Duration::from_millis(radeon_ms))
    }

    fn until(what: &str, mut done: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(Instant::now() < end, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn editable_row(page: &Page, id: &str) -> Row {
        page.state().radeon.panel.unwrap().find(id).unwrap().clone()
    }

    #[test]
    fn the_host_menu_follows_the_capability_and_a_refusal_keeps_the_hosts_words() {
        assert!(!State::new().host_menu(), "nothing is offered before the capabilities answer");
        let (mut fake, _) = fake(false);
        fake.features = vec!["native_api".into(), feature::COUCH_MENU.into()];
        let page = open(fake, 1000, 1000);
        until("capabilities", || page.state().couch_menu.is_some());
        assert!(page.state().host_menu());

        assert_eq!(host_menu_result(Ok(CouchMenu { open: true })), Ok(()));
        assert!(host_menu_result(Ok(CouchMenu { open: false })).is_err(), "a menu still closed is no success");
        for code in [ErrorCode::NoSession, ErrorCode::NotSessionOwner, ErrorCode::PermissionRequired, ErrorCode::TemporarilyUnavailable] {
            let error = host_menu_result(Err(api_error(409, code, None))).unwrap_err();
            assert!(error.contains("host words"), "{code:?}: {error}");
        }
    }

    #[test]
    fn it_polls_only_while_the_page_is_open() {
        let (fake, counters) = fake(true);
        let page = open(fake, 20, 20);
        until("a few reads", || counters.reads.load(Ordering::SeqCst) >= 3 && counters.sessions.load(Ordering::SeqCst) >= 2);
        let state = page.state();
        assert_eq!(state.launch, Some(true));
        assert_eq!(state.host_control, Some(true));
        assert!(state.session.is_some() && state.radeon.panel.is_some());
        assert!(!state.host_menu(), "no host menu without session.couch_menu");
        drop(page);
        // The worker ends after the call in flight: give it a moment, then nothing more arrives.
        std::thread::sleep(Duration::from_millis(100));
        let (reads, sessions) = (counters.reads.load(Ordering::SeqCst), counters.sessions.load(Ordering::SeqCst));
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(counters.reads.load(Ordering::SeqCst), reads, "no Radeon reads after the page closed");
        assert_eq!(counters.sessions.load(Ordering::SeqCst), sessions, "no session reads after the page closed");
    }

    #[test]
    fn radeon_is_read_at_most_once_per_interval() {
        let (fake, counters) = fake(true);
        let page = open(fake, 10_000, 100);
        // Wait for reads rather than for time: the check is on the gaps the host saw, which
        // can only be longer than the interval on a slow machine, never shorter.
        until("four reads", || counters.reads.load(Ordering::SeqCst) >= 4);
        drop(page);
        let times = counters.read_times.lock().unwrap().clone();
        for pair in times.windows(2) {
            let gap = pair[1].duration_since(pair[0]);
            assert!(gap >= Duration::from_millis(100), "two Radeon reads {gap:?} apart at a 100 ms interval");
        }
    }

    #[test]
    fn a_write_goes_out_only_when_the_panel_allows_it() {
        // Host control but no owned session: writable_now is false, so nothing is sent.
        let (fake, counters) = fake(false);
        let page = open(fake, 1000, 1000);
        until("the panel", || page.state().radeon.panel.is_some());
        let row = editable_row(&page, "anti_lag.enabled");
        assert_eq!(row.mode, Mode::Value);
        assert!(!page.write(&row, Value::Bool(false)));
        std::thread::sleep(Duration::from_millis(50));
        assert!(counters.writes.lock().unwrap().is_empty(), "no write without writable_now");
    }

    #[test]
    fn a_write_sends_the_value_and_the_answer_replaces_the_panel() {
        let (fake, counters) = fake(true);
        let page = open(fake, 1000, 1000);
        until("the panel", || page.state().radeon.panel.is_some());
        let row = editable_row(&page, "chill.max_fps");
        assert_eq!(row.mode, Mode::Edit);
        let value = row.step(1).unwrap();
        assert_eq!(value, Value::Integer(110), "100 plus the host's step of 10 (20 presses cross 30..300 is 10)");
        assert!(page.write(&row, value));
        // While it is in flight, a second write is refused.
        until("the write", || !counters.writes.lock().unwrap().is_empty());
        let sent = counters.writes.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].gpu, 5);
        assert_eq!(sent[0].changes, [Change { id: "chill.max_fps".into(), value: Value::Integer(110) }]);
        until("the answer", || page.state().radeon.pending.is_none());
        assert!(page.state().radeon.write_failure.is_none());
    }

    #[test]
    fn a_second_write_is_refused_while_one_is_in_flight() {
        let (fake, _counters) = fake(true);
        let page = open(fake, 1000, 1000);
        until("the panel", || page.state().radeon.panel.is_some());
        let row = editable_row(&page, "chill.max_fps");
        // The state a first write leaves behind until the worker answers.
        lock(&page.shared).radeon.pending = Some("anti_lag.enabled".into());
        assert!(!page.write(&row, Value::Integer(110)));
        lock(&page.shared).radeon.pending = None;
        assert!(page.write(&row, Value::Integer(110)));
    }

    #[test]
    fn a_rejected_write_shows_inline_and_reads_the_value_back() {
        let (mut fake, counters) = fake(true);
        fake.write_error = Some(ErrorCode::DriverRejected);
        let page = open(fake, 1000, 10_000);
        until("the panel", || page.state().radeon.panel.is_some());
        let before = counters.reads.load(Ordering::SeqCst);
        let row = editable_row(&page, "chill.max_fps");
        assert!(page.write(&row, Value::Integer(110)));
        until("the failure", || page.state().radeon.write_failure.is_some());
        let failure = page.state().radeon.write_failure.unwrap();
        assert_eq!(failure.setting.as_deref(), Some("chill.max_fps"));
        assert_eq!(failure.text(), "The AMD driver rejected this change.");
        assert!(page.state().radeon.pending.is_none());
        // The 10 s interval would not read again by itself: the failure asks for it.
        until("a read back", || counters.reads.load(Ordering::SeqCst) > before);
    }

    #[test]
    fn a_host_without_radeon_hides_the_panel_and_stops_asking() {
        let (fake, counters) = fake(true);
        *fake.radeon.lock().unwrap() = None;
        let page = open(fake, 20, 20);
        until("hidden", || page.state().radeon.hidden);
        std::thread::sleep(Duration::from_millis(120));
        assert_eq!(counters.reads.load(Ordering::SeqCst), 1, "a 404 is not asked again");
        assert!(counters.sessions.load(Ordering::SeqCst) >= 2, "the session is still read");
    }

    #[test]
    fn end_and_close_use_the_native_actions_and_quit_uses_cancel() {
        let (fake, counters) = fake(true);
        let page = open(fake, 1000, 1000);
        assert!(page.exit(HostExit::End));
        until("done", || matches!(page.state().exit, ExitState::Done(HostExit::End)));
        assert!(page.exit(HostExit::CloseGame));
        until("done", || matches!(page.state().exit, ExitState::Done(HostExit::CloseGame)));
        assert_eq!(*counters.actions.lock().unwrap(), [ActionId::End, ActionId::CloseGame]);
        assert_eq!(counters.quits.load(Ordering::SeqCst), 0);
        assert!(page.exit(HostExit::QuitApp));
        until("done", || matches!(page.state().exit, ExitState::Done(HostExit::QuitApp)));
        assert_eq!(counters.quits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_failed_exit_keeps_the_stream_and_says_why() {
        let (mut fake, _counters) = fake(true);
        fake.write_error = Some(ErrorCode::HostControlRequired);
        let page = open(fake, 1000, 1000);
        assert!(page.exit(HostExit::CloseGame));
        until("failed", || matches!(page.state().exit, ExitState::Failed(..)));
        let ExitState::Failed(exit, failure) = page.state().exit else { unreachable!() };
        assert_eq!(exit, HostExit::CloseGame);
        assert_eq!(exit_failure_text(&failure), "Turn on Host control for this device on the host to close games.");

        let (mut fake, _counters) = self::fake(true);
        fake.quit = Err(599);
        let page = open(fake, 1000, 1000);
        assert!(page.exit(HostExit::QuitApp));
        until("failed", || matches!(page.state().exit, ExitState::Failed(..)));
        let ExitState::Failed(_, failure) = page.state().exit else { unreachable!() };
        assert_eq!(exit_failure_text(&failure), "The running game belongs to another device.");
    }

    #[test]
    fn the_page_is_busy_until_an_exit_has_been_answered() {
        let (fake, _counters) = fake(true);
        let page = open(fake, 1000, 1000);
        assert!(!page.busy());
        // The state an exit leaves behind until the host answers.
        lock(&page.shared).exit = ExitState::Running(HostExit::End);
        assert!(page.busy(), "closing the page now would hide the host's answer");
        assert!(!page.exit(HostExit::CloseGame), "a second exit is refused too");
        lock(&page.shared).exit = ExitState::Failed(HostExit::End, Failure::of(&Error::NotFound("x".into()), None));
        assert!(!page.busy(), "a failed exit leaves the page free to close");
    }

    #[test]
    fn a_write_that_gets_404_hides_the_panel_and_stops_the_reads() {
        let (mut fake, counters) = fake(true);
        fake.write_missing = true;
        let page = open(fake, 1000, 40);
        until("the panel", || page.state().radeon.panel.is_some());
        let row = editable_row(&page, "chill.max_fps");
        assert!(page.write(&row, Value::Integer(110)));
        until("hidden", || page.state().radeon.hidden);
        std::thread::sleep(Duration::from_millis(30));
        let reads = counters.reads.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(counters.reads.load(Ordering::SeqCst), reads, "no more reads once a write got 404");
        assert!(page.state().radeon.hidden, "and the panel does not come back");
    }

    /// Manual: the page's worker against a real paired host, without a stream.
    /// `NIGHTJAR_LIVE_CONFIG=<config dir> NIGHTJAR_LIVE_ADDRESS=127.0.0.1 cargo test -p nightjar-desktop
    /// live_host -- --ignored --nocapture`
    #[test]
    #[ignore = "needs a paired Rubylight host; see the doc comment"]
    fn live_host_answers_the_page() {
        use nightjar_core::http::ClientIdentity;
        let Some(dir) = crate::sdl::test_input("NIGHTJAR_LIVE_CONFIG", "the paired host's config directory") else { return };
        let Some(address) = crate::sdl::test_input("NIGHTJAR_LIVE_ADDRESS", "the host's address") else { return };
        let (dir, address) = (dir.to_string_lossy().into_owned(), address.to_string_lossy().into_owned());
        let store = crate::config::Store::at(dir.into()).unwrap();
        let saved = store.identity().expect("a saved identity");
        let identity = Arc::new(ClientIdentity::from_pem(saved.unique_id, &saved.certificate_pem, &saved.private_key_pem).unwrap());
        let record = store.hosts().into_iter().find(|h| h.address == address && h.server_certificate.is_some()).expect("a paired host at that address");
        let page = Page::open(move || crate::hosts::client_for(&identity, &record, &address), Arc::new(|| {}));
        until("the first answers", || {
            let s = page.state();
            s.session.is_some() && (s.radeon.panel.is_some() || s.radeon.hidden || s.radeon.load_failure.is_some())
        });
        let state = page.state();
        println!("{state:#?}");
        println!("actions: {:?}", state.actions(Hints { launch: true, host_control: false }));
    }

    #[test]
    fn a_host_that_cannot_be_reached_is_said_not_hidden() {
        let page = Page::open_with(
            || -> Result<Fake, String> { Err("No client for this PC.".into()) },
            Arc::new(|| {}),
            SESSION_INTERVAL,
            RADEON_INTERVAL,
        );
        until("the error", || page.state().setup_error.is_some());
        assert_eq!(page.state().setup_error.as_deref(), Some("No client for this PC."));
    }

    /// Benchmark, not a check: `cargo test --release -p nightjar-desktop bench_menu_frame -- --ignored --nocapture`.
    /// One headless egui frame of the open "Session and graphics" window, laid out and tessellated.
    #[test]
    #[ignore = "benchmark"]
    fn bench_menu_frame() {
        let (fake, _) = fake(true);
        let mut page = open(fake, 60_000, 60_000);
        until("the page", || page.state().radeon.panel.is_some() && page.state().session.is_some());
        let lines: Vec<(&'static str, String)> = connection_lines("GAMING-PC", "HEVC", (2560, 1440, 120), 40_000, "Home Wi-Fi", Some("4.0 ms RTT (±1.0), 0.0% frames lost"));
        let hints = Hints { launch: true, host_control: true };
        for lang in [crate::strings::Lang::En, crate::strings::Lang::De] {
            crate::strings::set(lang);
            let ctx = egui::Context::default();
            crate::theme::apply(&ctx);
            let mut time = 0.0;
            let mut micros = Vec::new();
            for frame in 0..1200 {
                time += 1.0 / 60.0;
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1920.0, 1080.0))),
                    time: Some(time),
                    focused: true,
                    ..Default::default()
                };
                let start = Instant::now();
                let mut output = ctx.run_ui(input, |ui| {
                    egui::Window::new("Session and graphics")
                        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                        .collapsible(false)
                        .resizable(false)
                        .vscroll(true)
                        .default_width(440.0)
                        .show(ui.ctx(), |ui| {
                            crate::session_ui::show(ui, &mut page, "GAMING-PC", &lines, hints);
                        });
                });
                let meshes = ctx.tessellate(std::mem::take(&mut output.shapes), 1.0);
                output.textures_delta.clear();
                std::hint::black_box(&meshes);
                if frame >= 200 {
                    micros.push(start.elapsed().as_secs_f64() * 1e6);
                }
            }
            micros.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let at = |q: f64| micros[((micros.len() - 1) as f64 * q) as usize];
            println!(
                "BENCH {lang:?}: {} frames, p50 {:.0} us, p90 {:.0} us, p99 {:.0} us, max {:.0} us, mean {:.0} us",
                micros.len(),
                at(0.5),
                at(0.9),
                at(0.99),
                micros[micros.len() - 1],
                micros.iter().sum::<f64>() / micros.len() as f64
            );
        }
    }
}
