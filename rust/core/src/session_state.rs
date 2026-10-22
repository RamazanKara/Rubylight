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
//! `game_running: true` (native-api.md, open question 1).
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
mod tests {
    use super::*;

    fn game(id: u32) -> SessionApp {
        SessionApp {
            install_id: "c6f1a9a4".into(),
            app_id: id,
            title: "Dota 2".into(),
        }
    }
    fn set(devices: &[&str]) -> BTreeSet<String> {
        devices.iter().map(|d| (*d).to_owned()).collect()
    }
    /// A game that "me" started, with the given streams and retained displays.
    fn facts(streaming: &[&str], retained: &[&str]) -> Facts {
        Facts {
            launch_pending: false,
            streaming: set(streaming),
            game_running: true,
            game: Some((game(5), "me".into())),
            retained: set(retained),
        }
    }
    fn nothing() -> Facts {
        Facts::default()
    }
    fn streaming_machine() -> Machine {
        let mut m = Machine::default();
        m.begin("me", Some(game(5)), 10);
        m.app_starting(11);
        m.launched(true, &facts(&[], &["me"]), 12);
        assert_eq!(m.state(), State::Launching);
        assert!(m.observe(&facts(&["me"], &["me"]), 13));
        assert_eq!(m.state(), State::Streaming);
        m
    }
    fn suspended_machine() -> Machine {
        let mut m = streaming_machine();
        assert!(m.observe(&facts(&[], &["me"]), 20));
        assert_eq!(m.state(), State::Suspended);
        m
    }

    #[test]
    fn a_new_machine_is_idle_and_has_nothing_to_do() {
        let mut m = Machine::default();
        assert_eq!(m.state(), State::Idle);
        assert_eq!(m.journal(), None);
        assert!(!m.observe(&nothing(), 1));
        assert_eq!(m.plan_disconnect(), Err(Refusal::NoSession));
        assert_eq!(m.plan_end(), Err(Refusal::NoSession));
        assert_eq!(m.plan_close(false), Err(Refusal::NoSession));
        assert_eq!(m.plan_resume("me"), Err(Refusal::NoSession));
    }

    #[test]
    fn a_launch_goes_through_preparing_and_launching_to_streaming() {
        let mut m = Machine::default();
        m.begin("me", Some(game(5)), 10);
        assert_eq!(m.state(), State::Preparing);
        assert_eq!(m.owner(), Some("me"));
        // Facts cannot move a session whose launch call still runs.
        assert!(!m.observe(&nothing(), 11));
        assert_eq!(m.state(), State::Preparing);
        m.app_starting(12);
        assert_eq!(m.state(), State::Launching);
        m.launched(true, &nothing(), 13);
        assert_eq!(m.state(), State::Launching);
        // The launch waits for its client while it is pending.
        let pending = Facts {
            launch_pending: true,
            ..facts(&[], &["me"])
        };
        assert!(!m.observe(&pending, 14));
        assert_eq!(m.state(), State::Launching);
        assert!(m.observe(&facts(&["me"], &["me"]), 15));
        assert_eq!(m.state(), State::Streaming);
        assert_eq!(
            m.snapshot(&facts(&["me"], &[]), None).since_unix_ms,
            Some(15)
        );
    }

    #[test]
    fn a_failed_launch_falls_back_to_what_still_runs() {
        // Nothing before: idle.
        let mut m = Machine::default();
        m.begin("me", Some(game(5)), 1);
        m.launched(false, &nothing(), 2);
        assert_eq!(m.state(), State::Idle);
        assert_eq!(m.owner(), None);
        // A suspended session whose resume failed is suspended again.
        let mut m = suspended_machine();
        m.begin("me", Some(game(5)), 30);
        assert_eq!(m.state(), State::Preparing);
        m.launched(false, &facts(&[], &["me"]), 31);
        assert_eq!(m.state(), State::Suspended);
        // Another device's stream is untouched by a failed launch of mine.
        let mut m = streaming_machine();
        m.begin("other", Some(game(5)), 30);
        m.launched(false, &facts(&["me"], &["me"]), 31);
        assert_eq!(m.state(), State::Streaming);
        assert_eq!(m.owner(), Some("me"));
    }

    #[test]
    fn a_client_that_never_connects_returns_the_session_to_where_it_was() {
        let mut m = suspended_machine();
        m.begin("me", Some(game(5)), 30);
        m.launched(true, &facts(&[], &["me"]), 31);
        assert_eq!(m.state(), State::Launching);
        // The pending launch expired.
        assert!(m.observe(&facts(&[], &["me"]), 62));
        assert_eq!(m.state(), State::Suspended);
        // With no game left it is idle.
        let mut m = Machine::default();
        m.begin("me", Some(game(5)), 1);
        m.launched(true, &nothing(), 2);
        assert!(m.observe(&nothing(), 40));
        assert_eq!(m.state(), State::Idle);
    }

    #[test]
    fn the_last_stream_ending_suspends_a_game_that_keeps_its_display() {
        let mut m = streaming_machine();
        assert!(!m.observe(&facts(&["me"], &["me"]), 14));
        assert!(m.observe(&facts(&[], &["me"]), 20));
        assert_eq!(m.state(), State::Suspended);
        assert_eq!(m.owner(), Some("me"));
        assert_eq!(m.suspended_at_ms(), Some(20));
        // The same facts again change nothing, not even the revision.
        let revision = m.revision();
        assert!(!m.observe(&facts(&[], &["me"]), 25));
        assert_eq!(m.revision(), revision);
        assert_eq!(m.suspended_at_ms(), Some(20));
    }

    #[test]
    fn a_stream_with_no_game_or_no_display_ends_idle_not_suspended() {
        for facts in [
            nothing(),
            Facts {
                game_running: true,
                game: Some((game(5), "me".into())),
                ..Default::default()
            },
            Facts {
                retained: set(&["me"]),
                ..Default::default()
            },
        ] {
            let mut m = streaming_machine();
            assert!(m.observe(&facts, 20));
            assert_eq!(m.state(), State::Idle);
            assert_eq!(m.owner(), None);
        }
    }

    #[test]
    fn the_grace_period_ending_makes_a_suspended_session_idle() {
        let mut m = suspended_machine();
        // The host released the owner's display; the game keeps running.
        let after = Facts {
            retained: set(&[]),
            ..facts(&[], &[])
        };
        assert!(m.observe(&after, 99));
        assert_eq!(m.state(), State::Idle);
        let snapshot = m.snapshot(&after, Some(5));
        assert!(snapshot.game_running);
        assert_eq!(snapshot.owner, None);
        assert_eq!(snapshot.suspended_until_unix_ms, None);
        assert_eq!(m.suspended_at_ms(), None);
    }

    #[test]
    fn another_devices_retained_display_keeps_the_session_suspended() {
        let mut m = suspended_machine();
        assert!(m.observe(&facts(&[], &["other"]), 40));
        assert_eq!(m.state(), State::Suspended);
        assert_eq!(m.owner(), Some("other"));
    }

    #[test]
    fn a_suspended_session_reports_when_its_grace_period_ends() {
        let m = suspended_machine();
        let f = facts(&[], &["me"]);
        assert_eq!(
            m.snapshot(&f, Some(7_200)).suspended_until_unix_ms,
            Some(7_220)
        );
        // A grace period of zero seconds in the settings means it never ends.
        assert_eq!(m.snapshot(&f, None).suspended_until_unix_ms, None);
        // Only a suspended session has a deadline.
        let m = streaming_machine();
        assert_eq!(m.snapshot(&f, Some(1)).suspended_until_unix_ms, None);
    }

    #[test]
    fn reconnecting_resumes_a_suspended_session_and_supersedes_a_streaming_one() {
        for mut m in [suspended_machine(), streaming_machine()] {
            assert_eq!(m.plan_resume("me"), Ok(Plan::Run));
            assert_eq!(m.plan_resume("other"), Err(Refusal::NotOwner));
            m.begin("me", Some(game(5)), 40);
            assert_eq!(m.state(), State::Preparing);
            assert_eq!(m.plan_resume("me"), Err(Refusal::Busy));
            m.app_starting(41);
            m.launched(true, &facts(&[], &["me"]), 42);
            assert!(m.observe(&facts(&["me"], &["me"]), 43));
            assert_eq!(m.state(), State::Streaming);
            assert_eq!(m.owner(), Some("me"));
        }
    }

    #[test]
    fn disconnect_only_acts_on_a_streaming_session_and_repeats_safely() {
        assert_eq!(streaming_machine().plan_disconnect(), Ok(Plan::Run));
        let mut m = streaming_machine();
        m.disconnected(&facts(&[], &["me"]), 30);
        assert_eq!(m.state(), State::Suspended);
        // The game is untouched, and a repeat is a no-op.
        assert_eq!(m.plan_disconnect(), Ok(Plan::Skip));
        let mut busy = Machine::default();
        busy.begin("me", None, 1);
        assert_eq!(busy.plan_disconnect(), Ok(Plan::Skip));
        // A launch that still runs cannot be torn down from underneath.
        assert!(!busy.stopping(None, None, 2));
        assert_eq!(busy.state(), State::Preparing);
    }

    #[test]
    fn end_session_leaves_the_game_running_and_the_session_idle() {
        for mut m in [streaming_machine(), suspended_machine()] {
            assert_eq!(m.plan_end(), Ok(Plan::Run));
            assert!(m.stopping(None, None, 50));
            assert_eq!(m.state(), State::Stopping);
            // A second teardown joins the first.
            assert!(!m.stopping(None, None, 50));
            assert_eq!(m.owner(), Some("me"));
            assert_eq!(m.journal().unwrap().state, State::Stopping);
            // A repeat while stopping does nothing.
            assert_eq!(m.plan_end(), Ok(Plan::Skip));
            assert_eq!(m.plan_close(true), Ok(Plan::Skip));
            // Nothing is observed while the cleanup runs.
            assert!(!m.observe(&facts(&[], &["me"]), 51));
            assert_eq!(m.state(), State::Stopping);
            // The display is gone; the game is not.
            let after = facts(&[], &[]);
            m.stopped(&after, 52);
            assert_eq!(m.state(), State::Idle);
            let snapshot = m.snapshot(&after, None);
            assert!(snapshot.game_running);
            assert_eq!(snapshot.owner, None);
            assert_eq!(m.journal(), None);
            assert_eq!(m.plan_end(), Err(Refusal::NoSession));
        }
    }

    #[test]
    fn close_game_also_closes_a_game_that_end_session_left_running() {
        let mut m = Machine::default();
        assert_eq!(m.plan_close(true), Ok(Plan::Run));
        assert!(m.stopping(Some("me"), Some(game(5)), 60));
        // A stopping session always has an owner.
        assert_eq!(m.owner(), Some("me"));
        m.stopped(&nothing(), 61);
        assert_eq!(m.state(), State::Idle);
        assert_eq!(m.plan_close(false), Err(Refusal::NoSession));
    }

    #[test]
    fn end_and_close_are_busy_while_a_launch_runs() {
        let mut m = Machine::default();
        m.begin("me", Some(game(5)), 1);
        for _ in 0..2 {
            assert_eq!(m.plan_end(), Err(Refusal::Busy));
            assert_eq!(m.plan_close(true), Err(Refusal::Busy));
            assert_eq!(m.plan_resume("me"), Err(Refusal::Busy));
            m.app_starting(2);
            assert_eq!(m.state(), State::Launching);
        }
        m.launched(true, &nothing(), 3);
        assert_eq!(m.plan_end(), Err(Refusal::Busy));
    }

    #[test]
    fn a_clean_shutdown_forgets_the_session() {
        let mut m = streaming_machine();
        assert!(m.journal().is_some());
        m.reset(70);
        assert_eq!(m.state(), State::Idle);
        assert_eq!(m.owner(), None);
        assert_eq!(m.journal(), None);
        // A launch that was running no longer blocks observation.
        let mut m = Machine::default();
        m.begin("me", None, 1);
        m.reset(2);
        assert!(m.observe(&facts(&["me"], &["me"]), 3));
        assert_eq!(m.state(), State::Streaming);
    }

    #[test]
    fn a_stream_the_machine_did_not_begin_is_adopted() {
        let mut m = Machine::default();
        assert!(m.observe(&facts(&["me"], &["me"]), 5));
        assert_eq!(m.state(), State::Streaming);
        assert_eq!(m.owner(), Some("me"));
        assert_eq!(m.app(), Some(&game(5)));
    }

    #[test]
    fn the_revision_moves_only_when_something_a_client_sees_changes() {
        let mut m = Machine::default();
        let start = m.revision();
        m.observe(&nothing(), 1);
        assert_eq!(m.revision(), start);
        m.begin("me", Some(game(5)), 2);
        let preparing = m.revision();
        assert!(preparing > start);
        m.app_starting(3);
        assert!(m.revision() > preparing);
    }

    #[test]
    fn the_journal_round_trips_and_a_damaged_one_is_ignored() {
        let m = suspended_machine();
        let journal = m.journal().unwrap();
        assert_eq!(journal.state, State::Suspended);
        assert_eq!(Journal::parse(&journal.to_bytes()), Some(journal));
        assert_eq!(Journal::parse(b"{"), None);
        assert_eq!(Journal::parse(b""), None);
    }

    #[test]
    fn the_snapshot_feeds_session_view_without_breaking_its_owner_rule() {
        use crate::native::{LAUNCH, LIST, VIEW, session_view};
        use crate::native_api::session::{ActionId, Owner};
        let caller: crate::state::Client = serde_json::from_value(serde_json::json!({
            "name": "Phone", "cert": "", "uuid": "me", "perm": LIST | VIEW | LAUNCH
        }))
        .unwrap();
        for (m, expected) in [
            (Machine::default(), Owner::None),
            (streaming_machine(), Owner::You),
            (suspended_machine(), Owner::You),
        ] {
            let f = facts(&[], &["me"]);
            let view = session_view(&m.snapshot(&f, Some(1000)), &caller);
            assert_eq!(view.owner, expected);
            // The machine's refusals agree with the action rules of the view.
            let allowed = |id: ActionId| view.actions.iter().find(|a| a.id == id).unwrap().allowed;
            assert_eq!(allowed(ActionId::End), m.plan_end().is_ok());
            assert_eq!(allowed(ActionId::Resume), m.plan_resume("me").is_ok());
            assert_eq!(
                allowed(ActionId::Disconnect),
                m.plan_disconnect() == Ok(Plan::Run)
            );
        }
    }

    fn pending(mut f: Facts) -> Facts {
        f.launch_pending = true;
        f
    }

    #[test]
    fn a_launch_that_begins_during_a_teardown_keeps_the_state_when_the_teardown_ends() {
        // Teardown, then begin, then stopped.
        let mut m = suspended_machine();
        assert!(m.stopping(None, None, 30));
        assert_eq!(m.state(), State::Stopping);
        m.begin("you", Some(game(5)), 31);
        assert_eq!(m.state(), State::Preparing);
        assert_eq!(m.owner(), Some("you"));
        // The teardown ends while the launch call runs.
        m.stopped(&facts(&[], &["me"]), 32);
        assert_eq!(m.state(), State::Preparing, "the launch owns the state");
        assert_eq!(m.owner(), Some("you"));
        m.app_starting(33);
        assert_eq!(m.state(), State::Launching);
        m.launched(true, &pending(facts(&[], &[])), 34);
        assert_eq!(m.state(), State::Launching);
        assert!(m.observe(&facts(&["you"], &[]), 35));
        assert_eq!(m.state(), State::Streaming);

        // The teardown ends after the launch replied and waits for its client.
        let mut m = suspended_machine();
        assert!(m.stopping(None, None, 30));
        m.begin("you", Some(game(5)), 31);
        m.app_starting(32);
        m.launched(true, &pending(facts(&[], &[])), 33);
        m.stopped(&pending(facts(&[], &[])), 34);
        assert_eq!(m.state(), State::Launching);
        assert_eq!(m.owner(), Some("you"));
        // Nobody joins: it settles with what runs.
        assert!(m.observe(&facts(&[], &[]), 35));
        assert_eq!(m.state(), State::Idle);

        // The launch fails first: the teardown still runs, so the session
        // stays stopping until it ends.
        let mut m = suspended_machine();
        assert!(m.stopping(None, None, 30));
        m.begin("you", Some(game(5)), 31);
        m.launched(false, &facts(&[], &["me"]), 32);
        assert_eq!(m.state(), State::Stopping);
        assert!(!m.observe(&facts(&[], &["me"]), 32), "observe waits for it");
        m.stopped(&facts(&[], &["me"]), 33);
        assert_eq!(m.state(), State::Suspended);

        // A failed launch with no teardown running still falls back at once.
        let mut m = suspended_machine();
        m.begin("you", Some(game(5)), 31);
        m.launched(false, &facts(&[], &["me"]), 32);
        assert_eq!(m.state(), State::Suspended);
    }

    #[test]
    fn a_second_teardown_keeps_stopping_until_the_last_one_ends() {
        let mut m = suspended_machine();
        assert!(m.stopping(None, None, 30));
        m.begin("me", Some(game(5)), 31);
        m.app_starting(32);
        m.launched(true, &pending(facts(&[], &[])), 33);
        // /cancel while the launch waits for its client.
        assert!(m.stopping(None, None, 34));
        assert_eq!(m.state(), State::Stopping);
        // The first teardown ends: the second is still running.
        m.stopped(&nothing(), 35);
        assert_eq!(m.state(), State::Stopping);
        m.stopped(&nothing(), 36);
        assert_eq!(m.state(), State::Idle);
    }

    #[test]
    fn disconnect_does_not_overwrite_a_launch() {
        let mut m = suspended_machine();
        m.begin("me", Some(game(5)), 30);
        m.disconnected(&facts(&[], &["me"]), 31);
        assert_eq!(m.state(), State::Preparing);
        m.app_starting(32);
        m.launched(true, &pending(facts(&[], &["me"])), 33);
        m.disconnected(&pending(facts(&[], &["me"])), 34);
        assert_eq!(m.state(), State::Launching);
    }
}
