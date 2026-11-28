//! Quick Suspend on the host: a device or the console pauses the running
//! game's process tree and resumes it later, also from another device.
//!
//! A suspend picks the processes (`butterpollo_session::quick_suspend::policy`),
//! writes the write-ahead journal and starts the watchdog
//! (`butterpollo_win_io::process::suspend_watch`) before the first process is
//! suspended, and resumes what it suspended if any process refuses. A resume
//! undoes it from the journal; so does the end of the session, the game's
//! stop, the grace period running out, the next stream of the game and a
//! clean shutdown (`resume_for`). The stream stays connected meanwhile and
//! shows the game's last frame; a device that asked learns the state from
//! `0x554B` (`stream::control::quick_suspend`) or the native API.
//!
//! Lock order: the tracker's lock is taken after `launch_transition` and the
//! session mutex and before `current_app`; nothing calls in while holding it.
use crate::state::Host;
use butterpollo_session::{
    quick_suspend::{
        Journal, Record, Recovery, Thaw, journal,
        policy::{self, Refused},
        recover,
    },
    state::Client,
};
use butterpollo_win_io::process::{suspend, suspend_watch};
use rubylight_native_api::quick_suspend::{QuickSuspendStatus, SuspendState};
use rubylight_protocol::quick_suspend::{HostState, Reason, State};
use std::sync::{
    Mutex, MutexGuard, PoisonError,
    atomic::{AtomicU64, Ordering},
};

/// The suspended game, if any, and a count of state changes the control
/// streams follow.
#[derive(Default)]
pub struct Tracker {
    suspended: Mutex<Option<Suspended>>,
    revision: AtomicU64,
}

struct Suspended {
    journal: Journal,
    /// The processes that were suspended.
    count: usize,
    /// The device (or the console) that suspended it.
    by: String,
}

impl Tracker {
    fn lock(&self) -> MutexGuard<'_, Option<Suspended>> {
        self.suspended
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
    /// Moves on every change of the state, for the control streams to push it.
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }
    fn changed(&self) -> u64 {
        self.revision.fetch_add(1, Ordering::AcqRel) + 1
    }
    /// Whether a game is suspended now.
    pub fn suspended(&self) -> bool {
        self.lock().is_some()
    }
    #[cfg(test)]
    pub(crate) fn set_suspended_for_test(&self, title: &str) {
        let mut slot = self.lock();
        *slot = Some(Suspended {
            journal: Journal::new((1, 1), title, 1),
            count: 1,
            by: "test".into(),
        });
        let _ = self.changed();
    }
    #[cfg(test)]
    pub(crate) fn clear_suspended_for_test(&self) {
        let mut slot = self.lock();
        *slot = None;
        let _ = self.changed();
    }
}

/// How long a Quick-Suspended game keeps its retained virtual display while
/// disconnected (`dd_paused_virtual_display_timeout_secs`; 0 = indefinitely).
pub(crate) fn paused_display_timeout(
    config: &butterpollo_config::config::Config,
) -> Option<std::time::Duration> {
    let seconds = config
        .integer("dd_paused_virtual_display_timeout_secs", 7200)
        .max(0);
    (seconds != 0).then(|| std::time::Duration::from_secs(seconds as u64))
}

/// Who asks.
#[derive(Clone, Copy)]
pub enum Who<'a> {
    /// A paired device: the session's owner, or one with Host control.
    Device(&'a Client),
    /// The console's administrator.
    Console,
}

/// The game's state as a device or the console sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    pub reason: Reason,
    /// Plain English for `reason`, None with [`Reason::None`].
    pub message: Option<String>,
    pub app_title: Option<String>,
    pub suspended_count: u16,
    pub suspended_since_unix_ms: u64,
    pub suspended_by: Option<String>,
}

impl Status {
    fn new(state: State, reason: Reason, app_title: Option<String>) -> Self {
        let message = (reason != Reason::None).then(|| {
            policy::message(&Refused {
                reason,
                program: None,
            })
        });
        Self {
            state,
            reason,
            message,
            app_title,
            suspended_count: 0,
            suspended_since_unix_ms: 0,
            suspended_by: None,
        }
    }
    fn refused(refused: &Refused, app_title: Option<String>) -> Self {
        Self {
            message: Some(policy::message(refused)),
            ..Self::new(State::Refused, refused.reason, app_title)
        }
    }
    /// The `0x554B` state report answering `request_id` (0: no request).
    pub fn wire(&self, request_id: u32) -> HostState {
        HostState {
            flags: 0,
            request_id,
            state: self.state,
            reason: self.reason,
            suspended_count: self.suspended_count,
            suspended_since_unix_ms: self.suspended_since_unix_ms,
        }
    }
    /// The native API's answer.
    pub fn native(&self) -> QuickSuspendStatus {
        QuickSuspendStatus {
            state: match self.state {
                State::Unavailable => SuspendState::Unavailable,
                State::Ready => SuspendState::Ready,
                State::Suspended => SuspendState::Suspended,
                State::Resuming => SuspendState::Resuming,
                State::Refused => SuspendState::Refused,
            },
            reason: self.reason.as_str().to_owned(),
            message: self.message.clone(),
            app_title: self.app_title.clone(),
            suspended_count: u32::from(self.suspended_count),
            suspended_since_unix_ms: (self.state == State::Suspended)
                .then_some(self.suspended_since_unix_ms),
            suspended_by: self.suspended_by.clone(),
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

fn enabled(h: &Host) -> bool {
    crate::extensions::quick_suspend(&h.config.read().unwrap_or_else(PoisonError::into_inner))
}

fn suspended_status(suspended: &Suspended) -> Status {
    Status {
        suspended_count: suspended.count.min(usize::from(u16::MAX)) as u16,
        suspended_since_unix_ms: suspended.journal.since_unix_ms,
        suspended_by: Some(suspended.by.clone()),
        ..Status::new(
            State::Suspended,
            Reason::None,
            Some(suspended.journal.app_title.clone()),
        )
    }
}

/// The running game's title, and whether it has a process Quick Suspend could
/// start from.
fn running_game(h: &Host) -> Option<(String, bool)> {
    let app = h.current_app.lock().unwrap_or_else(PoisonError::into_inner);
    let app = app.as_ref()?;
    Some((app.name.clone(), app.has_game_process()))
}

/// The state now. Suspended stays visible while the setting is off, so the
/// game can still be resumed.
pub fn status(h: &Host) -> Status {
    status_with_revision(h).0
}

/// The state now paired with the revision captured under the tracker lock, so
/// control-stream answers never stamp an older snapshot with a newer concurrent
/// transition's revision.
pub(crate) fn status_with_revision(h: &Host) -> (Status, u64) {
    let (suspended, revision) = {
        let slot = h.quick_suspend.lock();
        (
            slot.as_ref().map(suspended_status),
            h.quick_suspend.revision(),
        )
    };
    if let Some(status) = suspended {
        return (status, revision);
    }
    let game = running_game(h);
    let title = game.as_ref().map(|(title, _)| title.clone());
    if !enabled(h) {
        return (
            Status::new(State::Unavailable, Reason::DisabledForApp, title),
            revision,
        );
    }
    let status = match game {
        None => Status::new(State::Unavailable, Reason::NoSession, None),
        Some((_, false)) => Status::new(State::Unavailable, Reason::NoOwnedProcess, title),
        Some((_, true)) => Status::new(State::Ready, Reason::None, title),
    };
    (status, revision)
}

/// Whether `who` may suspend or resume the session's game: the console, a
/// device with Host control, or the device that owns the session and may
/// launch apps.
fn permitted(h: &Host, who: Who<'_>) -> Result<String, Reason> {
    let client = match who {
        Who::Console => return Ok("the console".into()),
        Who::Device(client) => client,
    };
    if butterpollo_session::native::host_control(client) {
        return Ok(client.name.clone());
    }
    let owner = h.with_session(|machine, _, _| machine.owner().map(str::to_owned));
    let owns = owner.is_some_and(|owner| crate::nvhttp::same_device(&owner, &client.uuid));
    if owns && client.allows(butterpollo_session::native::LAUNCH) {
        Ok(client.name.clone())
    } else {
        Err(Reason::NotOwner)
    }
}

/// Guests or a co-op session share the game: pausing it would pause theirs.
fn shared(h: &Host) -> Option<Reason> {
    let sessions = h.sessions.lock().unwrap_or_else(PoisonError::into_inner);
    let mut streams = sessions.active.values().map(|session| &session.launch);
    if streams.clone().any(|launch| launch.nucleus.is_some()) {
        Some(Reason::CoopSession)
    } else if streams.any(|launch| launch.guest().is_some()) {
        Some(Reason::GuestsActive)
    } else {
        None
    }
}

fn refusal(reason: Reason) -> Refused {
    Refused {
        reason,
        program: None,
    }
}

/// Suspend the running game for `who`. A repeat while suspended answers the
/// state; a refusal changes nothing and says why.
pub fn suspend(h: &Host, who: Who<'_>) -> Status {
    suspend_with_revision(h, who).0
}

/// Like [`suspend`], returning the revision corresponding to the reported state.
pub(crate) fn suspend_with_revision(h: &Host, who: Who<'_>) -> (Status, u64) {
    let revision_before = h.quick_suspend.revision();
    let title = running_game(h).map(|(title, _)| title);
    let outcome = (|| -> Result<(Status, u64), Refused> {
        if !enabled(h) {
            return Err(refusal(Reason::DisabledForApp));
        }
        let by = permitted(h, who).map_err(refusal)?;
        // A launch or a session end must not run beside it.
        let _transition = h.transition().map_err(|_| refusal(Reason::SessionBusy))?;
        if let Some(reason) = shared(h) {
            return Err(refusal(reason));
        }
        let mut slot = h.quick_suspend.lock();
        if let Some(suspended) = slot.as_ref() {
            return Ok((suspended_status(suspended), h.quick_suspend.revision()));
        }
        let suspended = freeze(h, by, &mut suspend::Thaw)?;
        let answer = suspended_status(&suspended);
        *slot = Some(suspended);
        let revision = h.quick_suspend.changed();
        drop(slot);
        // A teardown the host starts on its own (the game's stop, the grace
        // period) takes no transition: one that began before the game was
        // suspended resumed nothing, so resume it here. One that begins from
        // now on finds the game suspended and resumes it itself.
        let stopping = h.with_session(|machine, _, _| {
            machine.state() == rubylight_native_api::session::State::Stopping
        });
        if stopping {
            thaw(h, "the session ends");
            return Err(refusal(Reason::SessionBusy));
        }
        Ok((answer, revision))
    })();
    outcome.unwrap_or_else(|refused| {
        tracing::info!(
            reason = refused.reason.as_str(),
            program = refused.program.as_deref().unwrap_or(""),
            "Quick Suspend refused"
        );
        (Status::refused(&refused, title), revision_before)
    })
}

/// Pick, journal and suspend the game's processes. What was suspended is
/// resumed again when a later process refuses.
fn freeze(h: &Host, by: String, thaw: &mut impl Thaw) -> Result<Suspended, Refused> {
    let os = |error: anyhow::Error| {
        tracing::warn!(error = %format!("{error:#}"), "Quick Suspend failed");
        refusal(Reason::IoOrOsError)
    };
    leftovers(h, thaw)?;
    // Listed before `current_app` is locked, as every owner of that lock does.
    let session = crate::process::session_processes().map_err(os)?;
    let all = butterpollo_win_io::process::processes().map_err(os)?;
    let (roots, title) = {
        let app = h.current_app.lock().unwrap_or_else(PoisonError::into_inner);
        let app = app.as_ref().ok_or_else(|| refusal(Reason::NoSession))?;
        let roots = app
            .suspend_roots(&session)
            .map_err(|_| refusal(Reason::NoOwnedProcess))?;
        (roots, app.name.clone())
    };
    let plan = plan_for_roots(&roots, &all, std::process::id())?;
    let mut journal = Journal::new(suspend_watch::me(), &title, now_ms());
    journal.processes = plan
        .targets
        .iter()
        .map(|target| Record {
            pid: target.pid,
            started: target.started,
            name: target.name.clone(),
        })
        .collect();
    // Write-ahead: on disk, and the watchdog running, before the first suspend.
    suspend_watch::lock(&h.directory)
        .and_then(|_lock| journal::save(&h.directory, &journal))
        .map_err(os)?;
    if let Err(error) = suspend_watch::watch(&h.directory) {
        forget(h);
        return Err(os(error));
    }
    let mut done: Vec<&Record> = Vec::new();
    for record in &journal.processes {
        match suspend::suspend(record.pid, record.started) {
            Ok(()) => done.push(record),
            // It exited since the list: nothing to pause.
            Err(suspend::SuspendError::Gone) => {}
            Err(error) => {
                tracing::warn!(pid = record.pid, program = record.name, %error, "Quick Suspend: a process refused; resuming the rest");
                let mut undone = journal.clone();
                undone.processes = done.iter().map(|record| (*record).clone()).collect();
                let recovery = recover(&undone, thaw);
                settle(h, &undone, &recovery, "a suspend failed partway");
                let reason = match error {
                    suspend::SuspendError::Protected => Reason::ProtectedProcess,
                    _ => Reason::IoOrOsError,
                };
                return Err(Refused {
                    reason,
                    program: Some(record.name.clone()),
                });
            }
        }
    }
    if done.is_empty() {
        forget(h);
        return Err(refusal(Reason::NoOwnedProcess));
    }
    let count = done.len();
    tracing::info!(
        game = title,
        processes = count,
        by,
        left_running = plan.left_running.join(", "),
        "Quick Suspend: the game is suspended"
    );
    Ok(Suspended { journal, count, by })
}

/// Run [`policy::plan`] for `roots`, ensuring a live protected root (`started == 0`)
/// that had not yet appeared in `all` still reports [`Reason::ProtectedProcess`]
/// rather than [`Reason::NoOwnedProcess`].
fn plan_for_roots(
    roots: &std::collections::BTreeMap<u32, u64>,
    all: &[butterpollo_session::steam::Process],
    host_pid: u32,
) -> Result<policy::Plan, Refused> {
    match policy::plan(roots, all, host_pid) {
        Ok(plan) => Ok(plan),
        Err(refused)
            if refused.reason == Reason::NoOwnedProcess
                && roots.iter().any(|(&pid, &started)| {
                    started == 0 && !all.iter().any(|p| p.pid == pid && p.started != 0)
                }) =>
        {
            let program = roots
                .iter()
                .filter(|&(_, &started)| started == 0)
                .find_map(|(&pid, _)| all.iter().find(|p| p.pid == pid).map(|p| p.name.clone()));
            Err(Refused {
                reason: Reason::ProtectedProcess,
                program,
            })
        }
        Err(refused) => Err(refused),
    }
}

/// A journal an earlier resume left behind (what it could not resume) is
/// tried again before anything else is suspended; while any of it stays
/// suspended nothing new is, so its records are never written over.
fn leftovers(h: &Host, thaw: &mut impl Thaw) -> Result<(), Refused> {
    let left = suspend_watch::lock(&h.directory).and_then(|_lock| journal::load(&h.directory));
    let left = match left {
        Ok(Some(left)) => left,
        Ok(None) => return Ok(()),
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "Quick Suspend: the journal cannot be read");
            return Err(refusal(Reason::IoOrOsError));
        }
    };
    let recovery = recover(&left, thaw);
    if settle(h, &left, &recovery, "a suspend starts") {
        Ok(())
    } else {
        Err(Refused {
            reason: Reason::IoOrOsError,
            program: recovery
                .failed
                .first()
                .map(|(record, _)| record.name.clone()),
        })
    }
}

/// After a resume of `journal`'s processes: when every one runs again the
/// journal goes, and the watchdog with it; otherwise what failed stays on
/// disk, with the watchdog, for it and the next start, and the console says
/// so. True when every process runs again.
fn settle(h: &Host, journal: &Journal, recovery: &Recovery, why: &str) -> bool {
    if recovery.complete() {
        forget(h);
        return true;
    }
    let mut left = journal.clone();
    left.processes = recovery
        .failed
        .iter()
        .map(|(record, _)| record.clone())
        .collect();
    for (record, error) in &recovery.failed {
        tracing::error!(
            pid = record.pid,
            program = record.name,
            error,
            why,
            "Quick Suspend: a process could not be resumed; the watchdog and the next start try again"
        );
    }
    let saved =
        suspend_watch::lock(&h.directory).and_then(|_lock| journal::save(&h.directory, &left));
    if let Err(error) = saved {
        tracing::error!(error = %format!("{error:#}"), "Quick Suspend: the journal could not be updated");
    }
    h.warnings.event(
        "quick_suspend_resume",
        "A paused game could not be resumed completely; Rubylight tries again when it starts.",
        std::time::Duration::from_secs(600),
    );
    false
}

/// Clear the journal and let the watchdog go: nothing is suspended.
fn forget(h: &Host) {
    let removed = suspend_watch::lock(&h.directory).and_then(|_lock| journal::remove(&h.directory));
    match removed {
        Ok(()) => suspend_watch::release(),
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "Quick Suspend: the journal could not be removed; the next start clears it")
        }
    }
}

/// Resume the suspended game for `who`. Without one it answers the state; a
/// process that stays suspended answers a refusal naming it.
pub fn resume(h: &Host, who: Who<'_>) -> Status {
    resume_with_revision(h, who).0
}

/// Like [`resume`], returning the revision corresponding to the reported state.
pub(crate) fn resume_with_revision(h: &Host, who: Who<'_>) -> (Status, u64) {
    resume_with_rev(h, who, &mut suspend::Thaw)
}

#[cfg(test)]
fn resume_with(h: &Host, who: Who<'_>, thaw: &mut impl Thaw) -> Status {
    resume_with_rev(h, who, thaw).0
}

fn resume_with_rev(h: &Host, who: Who<'_>, thaw: &mut impl Thaw) -> (Status, u64) {
    let revision_before = h.quick_suspend.revision();
    let title = || running_game(h).map(|(title, _)| title);
    if h.quick_suspend.suspended()
        && let Err(reason) = permitted(h, who)
    {
        return (Status::refused(&refusal(reason), title()), revision_before);
    }
    if let Err(failed) = thaw_with(h, "resumed", thaw) {
        let refused = Refused {
            reason: Reason::IoOrOsError,
            program: Some(failed),
        };
        return (Status::refused(&refused, title()), revision_before);
    }
    status_with_revision(h)
}

/// Resume the suspended game because `why` (the session ends, the game stops,
/// a stream of it starts, the host shuts down). Nothing when none is.
pub fn resume_for(h: &Host, why: &str) {
    thaw(h, why);
}

fn thaw(h: &Host, why: &str) {
    let _ = thaw_with(h, why, &mut suspend::Thaw);
}

/// Resume the suspended game, if any. Err names a process that stays
/// suspended, kept in the journal for the watchdog and the next start, and
/// still shown as suspended so a resume or the end of the session tries again.
fn thaw_with(h: &Host, why: &str, thaw: &mut impl Thaw) -> Result<(), String> {
    let mut slot = h.quick_suspend.lock();
    let Some(mut suspended) = slot.take() else {
        return Ok(());
    };
    let recovery = recover(&suspended.journal, thaw);
    let outcome = if settle(h, &suspended.journal, &recovery, why) {
        tracing::info!(
            game = suspended.journal.app_title,
            processes = recovery.resumed.len(),
            why,
            "Quick Suspend: the game runs again"
        );
        Ok(())
    } else {
        let failed = recovery
            .failed
            .first()
            .map(|(record, _)| record.name.clone())
            .unwrap_or_default();
        suspended.journal.processes = recovery
            .failed
            .iter()
            .map(|(record, _)| record.clone())
            .collect();
        suspended.count = suspended.journal.processes.len();
        *slot = Some(suspended);
        Err(failed)
    };
    let _ = h.quick_suspend.changed();
    drop(slot);
    outcome
}

/// Housekeeping: a suspended game whose processes all ended (closed from the
/// PC's task manager, or the PC's session ended) is suspended no more.
pub fn tick(h: &Host) {
    let mut slot = h.quick_suspend.lock();
    let ended = slot.as_ref().is_some_and(|suspended| {
        !suspended
            .journal
            .processes
            .iter()
            .any(|record| suspend::alive(record.pid, record.started))
    });
    if ended {
        slot.take();
        forget(h);
        let _ = h.quick_suspend.changed();
        drop(slot);
        tracing::info!("Quick Suspend: the suspended game ended");
    }
}

#[cfg(test)]
mod tests;
