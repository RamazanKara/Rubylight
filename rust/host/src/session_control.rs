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

pub struct Control {
    machine: Machine,
    journal: PathBuf,
    written: Option<Journal>,
}
impl Control {
    pub fn new(directory: &Path) -> Self {
        Self {
            machine: Machine::default(),
            journal: directory.join(JOURNAL),
            written: None,
        }
    }
    /// Keep the journal equal to the machine: written while a session
    /// exists, deleted when it ends.
    fn persist(&mut self) {
        let journal = self.machine.journal();
        if journal == self.written {
            return;
        }
        let result = match &journal {
            Some(journal) => {
                butterpollo_core::state::atomic_write(&self.journal, &journal.to_bytes())
            }
            None => match std::fs::remove_file(&self.journal) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
                _ => Ok(()),
            },
        };
        match result {
            Ok(()) => self.written = journal,
            // Retried at the next change.
            Err(error) => {
                tracing::warn!(error = %format!("{error:#}"), "the session journal could not be updated");
            }
        }
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
            let sessions = self.sessions.lock().unwrap();
            (sessions.stream_pending(), sessions.streaming_devices())
        };
        let running = self
            .current_app
            .lock()
            .unwrap()
            .as_ref()
            .map(|app| (app.id, app.name.clone(), app.owner.clone()));
        let game = running.as_ref().map(|(id, name, owner)| {
            (
                session_app(&self.apps.read().unwrap(), *id, name),
                owner.clone(),
            )
        });
        let retained = self.app_display.lock().unwrap().keys().cloned().collect();
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
        let mut control = self.session.lock().unwrap();
        let facts = self.session_facts();
        let now = now_ms();
        control.machine.observe(&facts, now);
        let result = f(&mut control.machine, &facts, now);
        control.persist();
        result
    }
    /// How long the owner's retained display survives without a stream.
    fn grace_ms(&self, owner: Option<&str>) -> Option<u64> {
        let lease = self.app_display.lock().unwrap().get(owner?)?.0.clone();
        let config = self.config.read().unwrap();
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
    pub fn disconnect_session(&self) -> Result<Outcome, Refusal> {
        let owner = self.with_session(|machine, _, _| {
            Ok::<_, Refusal>(match machine.plan_disconnect()? {
                Plan::Run => machine.owner().map(str::to_owned),
                Plan::Skip => None,
            })
        })?;
        let Some(owner) = owner else {
            return Ok(Outcome::Unchanged);
        };
        let canceled = self
            .sessions
            .lock()
            .unwrap()
            .stop_role(Role::Stream, Some(&owner));
        drop(canceled);
        self.with_session(|machine, facts, now| machine.disconnected(facts, now));
        Ok(Outcome::Done)
    }
    /// End session: stop the streams and release the displays and the audio
    /// route; the game keeps running. Busy while a launch runs.
    pub fn end_session(&self) -> Result<Outcome, Refusal> {
        let Ok(_transition) = self.launch_transition.try_lock() else {
            return Err(Refusal::Busy);
        };
        let Some(teardown) = self.begin_teardown(None, |machine, _| machine.plan_end())? else {
            return Ok(Outcome::Unchanged);
        };
        let canceled = self.sessions.lock().unwrap().stop_role(Role::Stream, None);
        drop(canceled);
        self.release_session_resources();
        self.wait_for_streams();
        drop(teardown);
        Ok(Outcome::Done)
    }
    /// Close game: End session, and the game too, as `/cancel` does. With no
    /// session it closes a game End session left running. `caller` owns the
    /// teardown of a game nobody owns.
    pub fn close_game(&self, caller: &str) -> Result<Outcome, Refusal> {
        let Ok(_transition) = self.launch_transition.try_lock() else {
            return Err(Refusal::Busy);
        };
        let Some(teardown) = self.begin_teardown(Some(caller), |machine, facts| {
            machine.plan_close(facts.game_running)
        })?
        else {
            return Ok(Outcome::Unchanged);
        };
        let canceled = self.sessions.lock().unwrap().stop_role(Role::Stream, None);
        drop(canceled);
        // Waits for the app to exit; the nested teardown joins this one.
        self.stop_app();
        self.wait_for_streams();
        drop(teardown);
        Ok(Outcome::Done)
    }
    /// Resume by the owner. A suspended session is reattached by the
    /// client's own `/resume` launch, so there is nothing to do here; one
    /// that still streams is superseded now, as that launch would.
    pub fn resume_session(&self, device: &str) -> Result<Outcome, Refusal> {
        let streaming = self.with_session(|machine, _, _| {
            Ok::<_, Refusal>(match machine.plan_resume(device)? {
                Plan::Run => machine.state() == State::Streaming,
                Plan::Skip => false,
            })
        })?;
        if !streaming {
            return Ok(Outcome::Unchanged);
        }
        let canceled = self
            .sessions
            .lock()
            .unwrap()
            .supersede(device, Role::Stream);
        drop(canceled);
        self.with_session(|machine, facts, now| machine.disconnected(facts, now));
        Ok(Outcome::Done)
    }

    /// Streams, audio route and retained displays: what a session holds apart
    /// from the game. Safe to repeat.
    fn release_session_resources(&self) {
        let audio = self.app_audio.lock().unwrap().take();
        let displays = std::mem::take(&mut *self.app_display.lock().unwrap());
        // Releasing a display restores Windows: never under a lock.
        drop((audio, displays));
    }
    fn wait_for_streams(&self) {
        let ids: Vec<_> = {
            let sessions = self.sessions.lock().unwrap();
            sessions
                .active
                .values()
                .filter(|s| s.launch.role == Role::Stream)
                .map(|s| s.launch.id.clone())
                .chain(sessions.teardown.keys().cloned())
                .collect()
        };
        if !crate::nvhttp::wait_for_teardown(&self.sessions, &ids, TEARDOWN_WAIT) {
            tracing::warn!("streams are still stopping; the session is reported idle anyway");
        }
    }
    /// Plan an action and, when it runs, enter `stopping` in one step.
    /// `None` means there was nothing to do.
    fn begin_teardown(
        &self,
        caller: Option<&str>,
        plan: impl FnOnce(&Machine, &Facts) -> Result<Plan, Refusal>,
    ) -> Result<Option<Teardown<'_>>, Refusal> {
        self.with_session(|machine, facts, now| {
            if plan(machine, facts)? == Plan::Skip {
                return Ok(None);
            }
            let entered = enter_stopping(machine, facts, caller, now);
            Ok(entered.then_some(Teardown {
                host: self,
                entered,
            }))
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
        let mut control = self.session.lock().unwrap();
        let bytes = match std::fs::read(&control.journal) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
            Err(error) => {
                tracing::warn!(%error, "the session journal could not be read");
                return None;
            }
        };
        if let Err(error) = std::fs::remove_file(&control.journal) {
            tracing::warn!(%error, "the session journal could not be deleted");
        }
        control.written = None;
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
