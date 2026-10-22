//! The host's session state (plan item A4).
//!
//! `butterpollo_core::session_state::Machine` holds the state:
//! idle, preparing, launching, streaming, suspended, stopping. This module
//! feeds it what the host observes (stream sessions, the running app, the
//! displays kept between streams), moves it at the points where the host
//! launches and tears down, and runs the four actions of the native API.
//!
//! The state only gates the native actions and is read back as a
//! `SessionSnapshot`. Stock Moonlight flows (`/launch`, `/resume`, `/cancel`,
//! RTSP and ENet disconnects, "Quit app") run exactly as before; they pass the
//! same choke points, `session_begin` and `session_teardown`, which only
//! record what they do.
//!
//! Lock order: the session mutex is outermost. Nothing calls into it while
//! holding `sessions`, `current_app` or `app_display`.
use crate::state::Host;
use butterpollo_core::{
    native::{self, SessionSnapshot},
    native_api::session::{SessionApp, State},
    session::Role,
    session_state::{Facts, Journal, Machine, Outcome, Plan, Refusal},
    state::App,
};
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, PoisonError, TryLockError, mpsc},
    time::Duration,
};

/// The session journal in the profile directory. It exists only while a
/// session does, so one found at startup means the host stopped mid-session.
pub const JOURNAL: &str = "session.json";
/// How long End session and Close game wait for streams to release their
/// resources before they report the session idle.
const TEARDOWN_WAIT: Duration = if cfg!(test) {
    Duration::from_millis(300)
} else {
    Duration::from_secs(5)
};

/// A lock that a panic elsewhere poisoned still guards the same data, and the
/// teardown and launch guards run while a panic may unwind: they must not
/// panic again, which would abort the host.
fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How soon the journal writer retries a write or delete that failed.
const RETRY: Duration = if cfg!(test) {
    Duration::from_millis(30)
} else {
    Duration::from_secs(2)
};

enum Message {
    /// The journal the file should hold now, `None` for no file.
    Write(Option<Journal>),
    /// Answer once every message before this one was applied.
    Flush(mpsc::Sender<()>),
}

pub struct Control {
    machine: Machine,
    journal: PathBuf,
    /// What the writer was last told; `None` when it must be told again.
    sent: Option<Option<Journal>>,
    writer: Option<mpsc::Sender<Message>>,
}
impl Control {
    pub fn new(directory: &Path) -> Self {
        let journal = directory.join(JOURNAL);
        let (sender, messages) = mpsc::channel();
        let path = journal.clone();
        let writer = std::thread::Builder::new()
            .name("session-journal".into())
            .spawn(move || write_journals(&path, &messages));
        if let Err(error) = &writer {
            tracing::error!(%error, "the session journal writer could not start; a crash during a session will not be reported");
        }
        Self {
            machine: Machine::default(),
            journal,
            sent: Some(None),
            writer: writer.is_ok().then_some(sender),
        }
    }
    /// Keep the journal equal to the machine: written while a session
    /// exists, deleted when it ends. A thread of its own writes the file, so
    /// launches and requests never wait for the disk.
    fn persist(&mut self) {
        let journal = self.machine.journal();
        if self.sent.as_ref() == Some(&journal) {
            return;
        }
        let Some(writer) = &self.writer else {
            return;
        };
        if writer.send(Message::Write(journal.clone())).is_ok() {
            self.sent = Some(journal);
        } else {
            tracing::error!(
                "the session journal writer stopped; a crash during a session will not be reported"
            );
            self.writer = None;
        }
    }
}

/// The journal writer: applies the newest state it was told, and retries
/// until the file matches. The journal only names a session for the warning
/// after a crash, so it is written without waiting for the disk to confirm.
fn write_journals(path: &Path, messages: &mpsc::Receiver<Message>) {
    // `None`: unknown, what is on disk may differ.
    let mut written: Option<Option<Journal>> = None;
    let mut wanted: Option<Option<Journal>> = None;
    loop {
        let first = if wanted.is_some() && wanted != written {
            messages.recv_timeout(RETRY)
        } else {
            messages
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        };
        let mut flushes = vec![];
        let mut handle = |message| match message {
            Message::Write(journal) => wanted = Some(journal),
            Message::Flush(ack) => flushes.push(ack),
        };
        match first {
            Ok(message) => handle(message),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        // Only the newest state matters.
        while let Ok(message) = messages.try_recv() {
            handle(message);
        }
        if let Some(journal) = &wanted
            && wanted != written
        {
            match apply(path, journal.as_ref()) {
                Ok(()) => written = wanted.clone(),
                Err(error) => {
                    tracing::warn!(%error, "the session journal could not be updated; trying again");
                }
            }
        }
        for ack in flushes {
            let _ = ack.send(());
        }
    }
}
/// No `sync_all`: a write torn by a crash loses at most the crash warning.
fn apply(path: &Path, journal: Option<&Journal>) -> std::io::Result<()> {
    match journal {
        Some(journal) => {
            let temporary = path.with_extension("json.tmp");
            std::fs::write(&temporary, journal.to_bytes())?;
            std::fs::rename(&temporary, path).inspect_err(|_| {
                let _ = std::fs::remove_file(&temporary);
            })
        }
        None => match std::fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        },
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}
fn session_app(apps: &[App], id: u32, name: &str) -> SessionApp {
    SessionApp {
        install_id: apps.iter().find(|app| app.id() == id).map_or_else(
            || {
                uuid::Uuid::new_v5(
                    &uuid::Uuid::NAMESPACE_URL,
                    format!("rubylight:app:{name}").as_bytes(),
                )
                .to_string()
            },
            native::install_id,
        ),
        app_id: id,
        title: name.to_owned(),
    }
}

/// An action by a device other than the session's owner is refused. Checked
/// under the launch lock, after the machine caught up with the host, so a
/// launch that finished a moment ago is judged by its real owner. `None` is a
/// host-side caller, which may act on any session.
fn owned_by(machine: &Machine, caller: Option<&str>) -> Result<(), Refusal> {
    match (machine.owner(), caller) {
        (Some(owner), Some(caller)) if owner != caller => Err(Refusal::NotOwner),
        _ => Ok(()),
    }
}

/// What a teardown ends, so a stock path that releases only some resources
/// does not report the whole session as stopping.
pub enum Scope<'a> {
    /// Streams and displays: the session, not the game.
    Session,
    /// The game too.
    Game,
    /// Only these devices' retained displays (the grace period running out).
    Displays(&'a [String]),
}

impl Host {
    /// What the host observes now.
    pub fn session_facts(&self) -> Facts {
        let (launch_pending, streaming) = {
            let sessions = locked(&self.sessions);
            (sessions.stream_pending(), sessions.streaming_devices())
        };
        let running = locked(&self.current_app)
            .as_ref()
            .map(|app| (app.id, app.name.clone(), app.owner.clone()));
        let game = running.as_ref().map(|(id, name, owner)| {
            let apps = self.apps.read().unwrap_or_else(PoisonError::into_inner);
            (session_app(&apps, *id, name), owner.clone())
        });
        let retained = locked(&self.app_display).keys().cloned().collect();
        Facts {
            launch_pending,
            streaming,
            game_running: running.is_some(),
            game,
            retained,
        }
    }
    /// Run `f` on the machine after bringing it up to date with the facts,
    /// and keep the journal current.
    fn with_session<R>(&self, f: impl FnOnce(&mut Machine, &Facts, u64) -> R) -> R {
        let mut control = locked(&self.session);
        let facts = self.session_facts();
        let now = now_ms();
        control.machine.observe(&facts, now);
        let result = f(&mut control.machine, &facts, now);
        control.persist();
        // Whatever moved the machine (End session, Close game, the grace period running out,
        // /cancel, a reset), a device that no longer owns the session gets its Radeon
        // settings back. Disconnect keeps the owner, so it restores nothing.
        self.radeon.sync(control.machine.owner());
        result
    }
    /// How long the owner's retained display survives without a stream.
    fn grace_ms(&self, owner: Option<&str>) -> Option<u64> {
        let lease = locked(&self.app_display).get(owner?)?.0.clone();
        let config = self.config.read().unwrap_or_else(PoisonError::into_inner);
        config
            .display_disconnect_delay(lease.exclusive_layout())
            .map(|delay| delay.as_millis() as u64)
    }

    /// The session as the native API shows it.
    pub fn session_snapshot(&self) -> SessionSnapshot {
        self.with_session(|machine, facts, _| {
            machine.snapshot(facts, self.grace_ms(machine.owner()))
        })
    }
    /// Follow the host between launches and teardowns: a stream starts or
    /// ends, a launch is never joined, the game exits. Every housekeeping
    /// tick.
    pub fn session_tick(&self) {
        self.with_session(|_, _, _| ());
    }

    /// Disconnect: drop the owner's stream now instead of waiting for a
    /// timeout. The game and the display stay for the grace period, so the
    /// session becomes suspended. A repeat, or any state without a stream,
    /// does nothing.
    pub fn disconnect_session(&self, caller: Option<&str>) -> Result<Outcome, Refusal> {
        let _transition = self.transition()?;
        let owner = self.with_session(|machine, _, _| {
            owned_by(machine, caller)?;
            Ok::<_, Refusal>(match machine.plan_disconnect()? {
                Plan::Run => machine.owner().map(str::to_owned),
                Plan::Skip => None,
            })
        })?;
        let Some(owner) = owner else {
            return Ok(Outcome::Unchanged);
        };
        let canceled = locked(&self.sessions).stop_role(Role::Stream, Some(&owner));
        drop(canceled);
        self.with_session(|machine, facts, now| machine.disconnected(facts, now));
        Ok(Outcome::Done)
    }
    /// End session: stop the owner's streams and release the owner's kept
    /// display and, when no other stream uses it, the audio route. The game
    /// keeps running, and so do the streams of other devices (`/cancel`
    /// stops them all, as stock Moonlight always has). Busy while a launch
    /// runs.
    pub fn end_session(&self, caller: Option<&str>) -> Result<Outcome, Refusal> {
        let _transition = self.transition()?;
        let Some((teardown, owner)) = self.begin_teardown(None, |machine, _| {
            owned_by(machine, caller)?;
            machine.plan_end()
        })?
        else {
            return Ok(Outcome::Unchanged);
        };
        let canceled = locked(&self.sessions).stop_role(Role::Stream, Some(&owner));
        drop(canceled);
        // Releasing a display restores Windows: never under a lock.
        let display = locked(&self.app_display).remove(&owner);
        drop(display);
        self.wait_for_streams(Some(&owner));
        // The route is shared by the streams of the game: a device that
        // still streams keeps it.
        let audio = {
            let mut audio = locked(&self.app_audio);
            if audio
                .as_ref()
                .is_some_and(|route| std::sync::Arc::strong_count(route) == 1)
            {
                audio.take()
            } else {
                None
            }
        };
        drop(audio);
        drop(teardown);
        Ok(Outcome::Done)
    }
    /// Close game: End session, and the game too, as `/cancel` does. With no
    /// session it closes a game End session left running. `caller` owns the
    /// teardown of a game nobody owns.
    pub fn close_game(&self, caller: &str) -> Result<Outcome, Refusal> {
        let _transition = self.transition()?;
        let Some((teardown, _)) = self.begin_teardown(Some(caller), |machine, facts| {
            owned_by(machine, Some(caller))?;
            machine.plan_close(facts.game_running)
        })?
        else {
            return Ok(Outcome::Unchanged);
        };
        let canceled = locked(&self.sessions).stop_role(Role::Stream, None);
        drop(canceled);
        // Waits for the app to exit; the nested teardown joins this one.
        self.stop_app();
        self.wait_for_streams(None);
        drop(teardown);
        Ok(Outcome::Done)
    }
    /// Resume by the owner. A suspended session is reattached by the
    /// client's own `/resume` launch, so there is nothing to do here; one
    /// that still streams is superseded now, as that launch would. Busy
    /// while a launch runs.
    #[allow(
        dead_code,
        reason = "a native client resumes with /launch; this action waits for a later endpoint"
    )]
    pub fn resume_session(&self, device: &str) -> Result<Outcome, Refusal> {
        let _transition = self.transition()?;
        let streaming = self.with_session(|machine, _, _| {
            Ok::<_, Refusal>(match machine.plan_resume(device)? {
                Plan::Run => machine.state() == State::Streaming,
                Plan::Skip => false,
            })
        })?;
        if !streaming {
            return Ok(Outcome::Unchanged);
        }
        let canceled = locked(&self.sessions).supersede(device, Role::Stream);
        drop(canceled);
        self.with_session(|machine, facts, now| machine.disconnected(facts, now));
        Ok(Outcome::Done)
    }

    /// The launch lock, or `Busy` while a launch holds it: an action must not
    /// race a launch of the same device, which a Disconnect or Resume would
    /// otherwise cancel.
    fn transition(&self) -> Result<MutexGuard<'_, ()>, Refusal> {
        match self.launch_transition.try_lock() {
            Ok(guard) => Ok(guard),
            Err(TryLockError::Poisoned(poisoned)) => Ok(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => Err(Refusal::Busy),
        }
    }
    /// Wait for the streams being stopped (the owner's, or all) to release
    /// their resources.
    fn wait_for_streams(&self, owner: Option<&str>) {
        let ids: Vec<_> = {
            let sessions = locked(&self.sessions);
            sessions
                .active
                .values()
                .filter(|s| s.launch.role == Role::Stream)
                .filter(|s| owner.is_none_or(|owner| s.launch.client.uuid == owner))
                .map(|s| s.launch.id.clone())
                .chain(sessions.teardown.keys().cloned())
                .collect()
        };
        if !crate::nvhttp::wait_for_teardown(&self.sessions, &ids, TEARDOWN_WAIT) {
            tracing::warn!("streams are still stopping; the session is reported idle anyway");
        }
    }
    /// Plan an action and, when it runs, enter `stopping` in one step.
    /// `None` means there was nothing to do. Returns the session's owner too.
    fn begin_teardown(
        &self,
        caller: Option<&str>,
        plan: impl FnOnce(&Machine, &Facts) -> Result<Plan, Refusal>,
    ) -> Result<Option<(Teardown<'_>, String)>, Refusal> {
        self.with_session(|machine, facts, now| {
            if plan(machine, facts)? == Plan::Skip {
                return Ok(None);
            }
            let before = machine.owner().map(str::to_owned);
            if !enter_stopping(machine, facts, caller, now) {
                return Ok(None);
            }
            let owner = before
                .or_else(|| machine.owner().map(str::to_owned))
                .unwrap_or_default();
            Ok(Some((
                Teardown {
                    host: self,
                    entered: true,
                },
                owner,
            )))
        })
    }

    /// Mark a teardown the host starts on its own: stock `/cancel`, the
    /// tray, the grace period running out. The state is `stopping` until the
    /// guard drops.
    pub fn session_teardown(&self, scope: Scope<'_>) -> Teardown<'_> {
        let entered = self.with_session(|machine, facts, now| {
            let wanted = match scope {
                Scope::Session => machine.state() != State::Idle,
                Scope::Game => machine.state() != State::Idle || facts.game_running,
                Scope::Displays(owners) => {
                    machine.state() == State::Suspended
                        && machine
                            .owner()
                            .is_some_and(|owner| owners.iter().any(|expired| expired == owner))
                }
            };
            wanted && enter_stopping(machine, facts, None, now)
        });
        Teardown {
            host: self,
            entered,
        }
    }

    /// A launch or resume of a stream starts. The guard moves the session
    /// back to where it was if the launch fails.
    pub fn session_begin(&self, owner: &str, app: Option<&App>) -> LaunchSession<'_> {
        self.with_session(|machine, facts, now| {
            let app = app
                .map(|app| session_app(std::slice::from_ref(app), app.id(), &app.name))
                .or_else(|| facts.game.as_ref().map(|(app, _)| app.clone()));
            machine.begin(owner, app, now);
        });
        LaunchSession {
            host: self,
            ok: false,
        }
    }

    /// At startup, after the previous host's displays and audio were
    /// restored: a journal that survived means that host stopped mid-session.
    /// The machine starts idle, so nothing else has to be undone; the journal
    /// is deleted and reported. Safe to repeat.
    pub fn recover_session(&self) -> Option<Journal> {
        let mut control = locked(&self.session);
        let bytes = match std::fs::read(&control.journal) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
            Err(error) => {
                tracing::warn!(%error, "the session journal could not be read");
                return None;
            }
        };
        if let Err(error) = std::fs::remove_file(&control.journal) {
            // The writer deletes it with its next update, retrying until it works.
            tracing::warn!(%error, "the session journal could not be deleted; it will be tried again");
            control.sent = None;
        }
        let Some(journal) = Journal::parse(&bytes) else {
            tracing::warn!("a damaged session journal was deleted");
            return None;
        };
        let game = journal
            .app
            .as_ref()
            .map_or_else(String::new, |app| format!(" of {}", app.title));
        self.warnings.event(
            "session_interrupted",
            format!(
                "The host stopped during a session{game} ({:?}). The session was cleared; a game it started may still be running.",
                journal.state
            ),
            Duration::from_secs(3600),
        );
        Some(journal)
    }
    /// A clean shutdown: no session survives, and no journal.
    pub fn session_shutdown(&self) {
        self.with_session(|machine, _, now| machine.reset(now));
        self.session_flush();
    }
    /// Wait until the journal writer has applied everything it was told, up
    /// to five seconds. For a clean shutdown and for tests.
    pub fn session_flush(&self) {
        let (ack, done) = mpsc::channel();
        let sent = locked(&self.session)
            .writer
            .as_ref()
            .is_some_and(|writer| writer.send(Message::Flush(ack)).is_ok());
        if sent {
            let _ = done.recv_timeout(Duration::from_secs(5));
        }
    }
}

fn enter_stopping(machine: &mut Machine, facts: &Facts, caller: Option<&str>, now: u64) -> bool {
    // A stopping session always has an owner, also a game nobody owns.
    let owner = facts
        .game
        .as_ref()
        .map(|(_, owner)| owner.as_str())
        .filter(|owner| !owner.is_empty())
        .or(caller)
        .unwrap_or("host");
    machine.stopping(
        Some(owner),
        facts.game.as_ref().map(|(app, _)| app.clone()),
        now,
    )
}

/// A teardown in progress. Dropping it finishes: the session settles to idle,
/// or to what still runs.
pub struct Teardown<'a> {
    host: &'a Host,
    entered: bool,
}
impl Drop for Teardown<'_> {
    fn drop(&mut self) {
        if self.entered {
            self.host
                .with_session(|machine, facts, now| machine.stopped(facts, now));
        }
    }
}

/// A launch in progress. `succeed` before it drops, or the launch counts as
/// failed.
pub struct LaunchSession<'a> {
    host: &'a Host,
    ok: bool,
}
impl LaunchSession<'_> {
    /// The app's own commands start.
    pub fn app_starting(&self) {
        self.host
            .with_session(|machine, _, now| machine.app_starting(now));
    }
    pub fn succeed(&mut self) {
        self.ok = true;
    }
}
impl Drop for LaunchSession<'_> {
    fn drop(&mut self) {
        let ok = self.ok;
        self.host
            .with_session(|machine, facts, now| machine.launched(ok, facts, now));
    }
}

#[cfg(test)]
mod tests;
