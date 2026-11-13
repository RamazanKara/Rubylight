//! The host's session state machine (plan item A4).
//!
//! `idle -> preparing -> launching -> streaming -> suspended -> stopping -> idle`.
//! Pure and OS-independent: the host gathers [`Facts`] from its launch queue,
//! stream sessions, running app and retained displays, and feeds them in with
//! the time. Everything here is a plain transition, so every row of the table
//! is tested without a GPU.
//!
//! | From | Event | To |
//! |---|---|---|
//! | idle, suspended, streaming | `begin` (a launch or resume is accepted) | preparing |
//! | preparing | `app_starting` | launching |
//! | preparing, launching | `launched`, a stream is up | streaming |
//! | preparing, launching | `launched`, no stream (failed, or the client never connected) | `settle` |
//! | streaming | the last stream of the session ends | suspended, or idle with no game or display |
//! | suspended | the owner's retained display is gone (grace period over), or the game ended | idle |
//! | suspended | a stream starts | streaming |
//! | streaming, suspended, idle | `stopping` (End session, Close game, grace expiry, game ended) | stopping |
//! | stopping | `stopped` | `settle`, which is idle once nothing is retained |
//!
//! `settle` is the resting place after any transition: streaming while a
//! stream runs, suspended while a game runs with a retained display, idle
//! otherwise. A game left running after End session is therefore `idle` with
//! `game_running: true` (native-api.md, design decision 1).
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::native::SessionSnapshot;
use crate::native_api::session::{SessionApp, State};

/// What the host observes right now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// A live `Role::Stream` launch waits for its client to connect.
    pub launch_pending: bool,
    /// The devices with a `Role::Stream` session that is not stopping.
    pub streaming: BTreeSet<String>,
    /// There is a running `current_app`.
    pub game_running: bool,
    /// The running game, and the device that started it.
    pub game: Option<(SessionApp, String)>,
    /// The devices whose game display is retained between streams.
    pub retained: BTreeSet<String>,
}

/// Why an action cannot run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// There is no session (and for Close game, no game either).
    NoSession,
    /// A launch or a teardown is in progress: retry shortly.
    Busy,
    /// The session belongs to another device.
    NotOwner,
}

/// What a permitted action did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The action ran now.
    Done,
    /// There was nothing to do: a repeat, or already stopping.
    Unchanged,
}

/// What the machine decided for an action. `Run` means the host performs the
/// cleanup between `stopping` and `stopped`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    Run,
    Skip,
}

#[derive(Clone, Debug)]
pub struct Machine {
    state: State,
    owner: Option<String>,
    app: Option<SessionApp>,
    since_ms: u64,
    suspended_at_ms: Option<u64>,
    revision: u64,
    /// A launch call is running, and owns the transitions until `launched`.
    launching: bool,
    /// Teardowns that entered `stopping` and have not finished. A launch can
    /// replace the state while one runs, and a later teardown can enter it
    /// again, so the session settles only when the last one ends.
    teardowns: u32,
}
impl Default for Machine {
    fn default() -> Self {
        Self {
            state: State::Idle,
            owner: None,
            app: None,
            since_ms: 0,
            suspended_at_ms: None,
            revision: 1,
            launching: false,
            teardowns: 0,
        }
    }
}

impl Machine {
    pub fn state(&self) -> State {
        self.state
    }
    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }
    pub fn app(&self) -> Option<&SessionApp> {
        self.app.as_ref()
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// When the session was suspended, for the grace deadline.
    pub fn suspended_at_ms(&self) -> Option<u64> {
        self.suspended_at_ms
    }

    fn enter(&mut self, state: State, now: u64) {
        if self.state != state {
            self.state = state;
            self.since_ms = now;
            self.revision += 1;
        }
        self.suspended_at_ms = match state {
            State::Suspended => self.suspended_at_ms.or(Some(now)),
            _ => None,
        };
    }
    fn set_session(&mut self, owner: Option<String>, app: Option<SessionApp>) {
        if self.owner != owner || self.app != app {
            self.owner = owner;
            self.app = app;
            self.revision += 1;
        }
    }

    /// Where a session rests when no transition runs: a stream is up, a game
    /// runs with a retained display (the grace period), or nothing remains.
    fn settle(&mut self, facts: &Facts, now: u64) {
        let owner = self.owner.clone();
        let app = facts.game.as_ref().map(|(app, _)| app.clone());
        if !facts.streaming.is_empty() {
            let owner = owner
                .filter(|owner| facts.streaming.contains(owner))
                .or_else(|| facts.streaming.iter().next().cloned());
            self.set_session(owner, app.or_else(|| self.app.clone()));
            self.enter(State::Streaming, now);
        } else if facts.game_running && !facts.retained.is_empty() {
            // The owner's display, or when that one is gone, another device's.
            let owner = owner
                .filter(|owner| facts.retained.contains(owner))
                .or_else(|| facts.retained.iter().next().cloned());
            self.set_session(owner, app.or_else(|| self.app.clone()));
            self.enter(State::Suspended, now);
        } else {
            self.set_session(None, None);
            self.enter(State::Idle, now);
        }
    }

    /// A launch or resume of a stream is accepted: the host starts preparing
    /// displays and integrations. A device that takes over a running session
    /// becomes its owner.
    ///
    /// A launch accepted while a teardown runs (`stopping`) takes over the
    /// state: it becomes `preparing`, as the launch itself waits for the
    /// streams being torn down to finish before it uses their resources.
    /// The teardown's `stopped` then leaves the launch's state alone. While
    /// the launch call runs, no teardown can enter `stopping` (see
    /// `stopping`).
    pub fn begin(&mut self, owner: &str, app: Option<SessionApp>, now: u64) {
        self.launching = true;
        self.set_session(Some(owner.into()), app);
        self.enter(State::Preparing, now);
    }
    /// The app's own commands start.
    pub fn app_starting(&mut self, now: u64) {
        if self.launching {
            self.enter(State::Launching, now);
        }
    }
    /// The launch call returned. Success leaves the session `launching` until
    /// the client connects; failure falls back to what is still running, or
    /// to `stopping` while a teardown that began before the launch still
    /// runs (its `stopped` settles the session).
    pub fn launched(&mut self, ok: bool, facts: &Facts, now: u64) {
        self.launching = false;
        if ok && facts.streaming.is_empty() {
            self.enter(State::Launching, now);
        } else if !ok && self.teardowns > 0 {
            self.enter(State::Stopping, now);
        } else {
            self.settle(facts, now);
        }
    }
    /// Follow the facts between transitions: a stream starts or ends, the
    /// client never connects, the grace period runs out, the game exits.
    /// Returns whether anything changed.
    pub fn observe(&mut self, facts: &Facts, now: u64) -> bool {
        if self.launching || self.state == State::Stopping {
            return false;
        }
        let waiting = matches!(self.state, State::Preparing | State::Launching)
            && facts.launch_pending
            && facts.streaming.is_empty();
        if waiting {
            return false;
        }
        let before = self.revision;
        self.settle(facts, now);
        self.revision != before
    }

    /// What Disconnect does. It never touches the game: only a streaming
    /// session has a transport to drop, and any other state is a safe repeat.
    pub fn plan_disconnect(&self) -> Result<Plan, Refusal> {
        match self.state {
            State::Streaming => Ok(Plan::Run),
            State::Idle => Err(Refusal::NoSession),
            _ => Ok(Plan::Skip),
        }
    }
    /// What End session does: tear down the stream and the display, keep the
    /// game.
    pub fn plan_end(&self) -> Result<Plan, Refusal> {
        match self.state {
            State::Idle => Err(Refusal::NoSession),
            State::Preparing | State::Launching => Err(Refusal::Busy),
            State::Stopping => Ok(Plan::Skip),
            _ => Ok(Plan::Run),
        }
    }
    /// What Close game does: End session, and the game too. With no session
    /// it closes a game End session left running.
    pub fn plan_close(&self, game_running: bool) -> Result<Plan, Refusal> {
        match self.state {
            State::Idle if !game_running => Err(Refusal::NoSession),
            State::Preparing | State::Launching => Err(Refusal::Busy),
            State::Stopping => Ok(Plan::Skip),
            _ => Ok(Plan::Run),
        }
    }
    /// What a resume by `device` does: only the owner reattaches a suspended
    /// session, or supersedes its own stream.
    pub fn plan_resume(&self, device: &str) -> Result<Plan, Refusal> {
        match self.state {
            State::Idle => Err(Refusal::NoSession),
            State::Preparing | State::Launching | State::Stopping => Err(Refusal::Busy),
            _ if self.owner.as_deref() != Some(device) => Err(Refusal::NotOwner),
            _ => Ok(Plan::Run),
        }
    }

    /// Teardown starts. `owner` names the game's owner when no session does
    /// (Close game on a game End session left running), so a stopping session
    /// always has one. Returns whether this call entered `stopping`: a launch
    /// call that still runs owns the session, and a teardown that already
    /// runs is the one that finishes it.
    #[must_use]
    pub fn stopping(&mut self, owner: Option<&str>, app: Option<SessionApp>, now: u64) -> bool {
        if self.launching || self.state == State::Stopping {
            return false;
        }
        if self.owner.is_none() {
            self.set_session(owner.map(str::to_owned), app);
        }
        self.enter(State::Stopping, now);
        self.teardowns += 1;
        true
    }
    /// A launch owns the state: its call still runs, or it replied and waits
    /// for the client.
    fn launch_owns(&self, facts: &Facts) -> bool {
        self.launching
            || (matches!(self.state, State::Preparing | State::Launching)
                && facts.launch_pending
                && facts.streaming.is_empty())
    }
    /// The transport of the owner's stream was stopped (Disconnect). A launch
    /// that started meanwhile keeps the state.
    pub fn disconnected(&mut self, facts: &Facts, now: u64) {
        if !self.launch_owns(facts) {
            self.settle(facts, now);
        }
    }
    /// Teardown finished. A launch accepted while it ran took over the state
    /// (see `begin`), so the teardown ending then changes nothing: the
    /// launch settles the session when it returns.
    pub fn stopped(&mut self, facts: &Facts, now: u64) {
        self.teardowns = self.teardowns.saturating_sub(1);
        if self.teardowns == 0 && !self.launch_owns(facts) {
            self.settle(facts, now);
        }
    }
    /// A clean host shutdown: nothing survives.
    pub fn reset(&mut self, now: u64) {
        self.launching = false;
        self.teardowns = 0;
        self.set_session(None, None);
        self.enter(State::Idle, now);
    }

    /// The state for `session_view`. `grace_ms` is how long a suspended
    /// session keeps its display, or `None` when it never expires.
    pub fn snapshot(&self, facts: &Facts, grace_ms: Option<u64>) -> SessionSnapshot {
        SessionSnapshot {
            revision: self.revision,
            state: self.state,
            game_running: facts.game_running,
            owner: self.owner.clone(),
            app: self.app.clone(),
            since_unix_ms: (self.state != State::Idle).then_some(self.since_ms),
            suspended_until_unix_ms: (self.state == State::Suspended)
                .then(|| {
                    self.suspended_at_ms
                        .zip(grace_ms)
                        .map(|(at, grace)| at + grace)
                })
                .flatten(),
            launcher: None,
            radeon_profile: None,
            holder: None,
        }
    }

    /// What survives a host crash, or `None` while idle.
    pub fn journal(&self) -> Option<Journal> {
        (self.state != State::Idle).then(|| Journal {
            version: 1,
            state: self.state,
            owner: self.owner.clone(),
            app: self.app.clone(),
            since_unix_ms: self.since_ms,
        })
    }
}

/// The session as the host last wrote it to disk. It is deleted when the
/// session ends cleanly, so one found at startup means the host stopped
/// mid-session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Journal {
    pub version: u32,
    pub state: State,
    pub owner: Option<String>,
    pub app: Option<SessionApp>,
    pub since_unix_ms: u64,
}
impl Journal {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice(bytes).ok()
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests;
