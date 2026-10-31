//! The one thread that owns the Radeon controls of a running host.
//!
//! Startup recovery of a crashed run's settings must neither block the host from starting nor race
//! a later write, so both go through this worker. It owns the only `Controller`, takes jobs in
//! order, and runs the recovery first: a write or a session restore queued behind it waits until
//! the recovery has finished (or hit its time limit). Settings that could not be put back stay in
//! the journal; the worker logs that loudly and retries while no session of this run is live.
use super::{
    BackendError, Change, Controller, Entry, GpuInfo, RadeonBackend, RestoreReport, Scope, Setting,
    WriteError, control::JOURNAL_FILE, helper::HelperBackend,
};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    time::{Duration, Instant},
};

/// After this the startup recovery stops starting new work; what is left stays journaled and is
/// retried.
pub const RECOVERY_LIMIT: Duration = Duration::from_secs(90);
/// How often a recovery that left settings changed is tried again.
pub const RETRY_EVERY: Duration = Duration::from_secs(60);

/// How long `apply` and `restore_session` wait for the worker before giving up. It covers the
/// queue (including a startup recovery at its limit) plus the call itself.
pub const CALL_LIMIT: Duration = Duration::from_secs(150);

#[derive(Clone, Copy, Debug)]
pub struct Timing {
    pub recovery_limit: Duration,
    pub retry_every: Duration,
    pub call_limit: Duration,
}
impl Default for Timing {
    fn default() -> Self {
        Self {
            recovery_limit: RECOVERY_LIMIT,
            retry_every: RETRY_EVERY,
            call_limit: CALL_LIMIT,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// The startup recovery has not finished; queued jobs wait.
    Recovering,
    Ready,
    /// Settings a crashed run changed are still journaled and the worker is retrying.
    Degraded,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub phase: Phase,
    /// How many settings the last recovery could not put back.
    pub unrestored: usize,
    /// Why the last recovery could not even start (another live host owns the journal, the
    /// journal is unreadable).
    pub error: Option<String>,
}

enum Job {
    Apply {
        session: String,
        gpu: i32,
        changes: Vec<Change>,
        reply: mpsc::Sender<Result<Vec<Setting>, WriteError>>,
        /// Set by a caller that stopped waiting: a job that has not started is then dropped, so
        /// a late write cannot land after the caller was told it failed.
        abandoned: Arc<AtomicBool>,
    },
    Restore {
        session: String,
        /// Only these settings (and what the driver switched off because of them), or all of
        /// the session's.
        only: Option<Vec<String>>,
        reply: mpsc::Sender<anyhow::Result<RestoreReport>>,
    },
    Gpu {
        adapter: Option<String>,
        reply: mpsc::Sender<Result<Option<GpuInfo>, BackendError>>,
    },
    Panel {
        adapter: Option<String>,
        reply: mpsc::Sender<Result<Option<Panel>, BackendError>>,
    },
    Pending {
        reply: mpsc::Sender<Result<Vec<Entry>, BackendError>>,
    },
}

/// The settings of the GPU the host encodes on, as the driver reads them now, with what the
/// journal would still put back.
#[derive(Clone, Debug)]
pub struct Panel {
    pub gpu: GpuInfo,
    pub settings: Vec<Setting>,
    pub pending: Vec<Entry>,
}

/// The GPU the host works with: the one whose name contains `adapter` (the configured adapter
/// name), else the first AMD GPU ADLX lists.
fn pick(gpus: Vec<GpuInfo>, adapter: Option<&str>) -> Option<GpuInfo> {
    let wanted = adapter.map(str::to_lowercase).filter(|a| !a.is_empty());
    let named = wanted.and_then(|wanted| {
        gpus.iter()
            .position(|gpu| gpu.name.to_lowercase().contains(&wanted))
    });
    gpus.into_iter().nth(named.unwrap_or(0))
}

pub struct Worker {
    jobs: mpsc::Sender<Job>,
    status: Arc<Mutex<Status>>,
    call_limit: Duration,
}

impl Worker {
    /// Start the worker thread and queue the startup recovery. Returns at once.
    pub fn start<B: RadeonBackend + Send + 'static>(
        backend: B,
        journal: PathBuf,
        timing: Timing,
    ) -> Self {
        let (jobs, queue) = mpsc::channel();
        let status = Arc::new(Mutex::new(Status {
            phase: Phase::Recovering,
            unrestored: 0,
            error: None,
        }));
        let shared = status.clone();
        std::thread::Builder::new()
            .name("radeon".into())
            .spawn(move || run(Controller::new(backend, journal), &queue, &shared, timing))
            .expect("spawn the Radeon worker thread");
        Self {
            jobs,
            status,
            call_limit: timing.call_limit,
        }
    }

    pub fn status(&self) -> Status {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Queue behind the recovery and any earlier job, and wait for the driver's read-back, at
    /// most `call_limit`. Each call runs one helper process per read and write (about 600 ms for
    /// a small request), so this must stay off the stream-start path: call it from a task that
    /// the stream does not wait for.
    ///
    /// On a timeout the request is dropped if it has not started. One that was already running
    /// may still finish, so the caller must still `restore_session` afterwards.
    pub fn apply(
        &self,
        session: &str,
        gpu: i32,
        changes: Vec<Change>,
    ) -> Result<Vec<Setting>, WriteError> {
        self.queue_apply(session, gpu, changes)?.wait()
    }

    /// Queue a write and return at once: the job is behind everything queued before this call.
    /// A caller that must order the write against other queued work does that between this and
    /// the [`ApplyTicket::wait`].
    pub fn queue_apply(
        &self,
        session: &str,
        gpu: i32,
        changes: Vec<Change>,
    ) -> Result<ApplyTicket, WriteError> {
        let (reply, answer) = mpsc::channel();
        let abandoned = Arc::new(AtomicBool::new(false));
        let job = Job::Apply {
            session: session.into(),
            gpu,
            changes,
            reply,
            abandoned: abandoned.clone(),
        };
        self.jobs.send(job).map_err(|_| stopped())?;
        Ok(ApplyTicket {
            answer,
            abandoned,
            call_limit: self.call_limit,
        })
    }

    /// Queue behind the recovery and any earlier job, and put one session's settings back, waiting
    /// at most `call_limit`. A restore that times out stays queued and still runs.
    pub fn restore_session(&self, session: &str) -> anyhow::Result<RestoreReport> {
        self.queue_restore(session)?.wait()
    }

    /// Queue a restore and return at once, like [`Worker::queue_apply`].
    pub fn queue_restore(&self, session: &str) -> anyhow::Result<RestoreTicket> {
        self.queue_restore_of(session, None)
    }

    /// Put back only `ids` of a session that goes on (and the switches the driver made because of
    /// them), leaving its other journaled settings, waiting at most `call_limit`.
    pub fn restore_settings(&self, session: &str, ids: &[String]) -> anyhow::Result<RestoreReport> {
        self.queue_restore_of(session, Some(ids.to_vec()))?.wait()
    }

    fn queue_restore_of(
        &self,
        session: &str,
        only: Option<Vec<String>>,
    ) -> anyhow::Result<RestoreTicket> {
        let (reply, answer) = mpsc::channel();
        let job = Job::Restore {
            session: session.into(),
            only,
            reply,
        };
        self.jobs
            .send(job)
            .map_err(|_| anyhow::anyhow!("the Radeon worker stopped"))?;
        Ok(RestoreTicket {
            answer,
            call_limit: self.call_limit,
        })
    }
}

/// A queued write, waiting for the driver's read-back.
pub struct ApplyTicket {
    answer: mpsc::Receiver<Result<Vec<Setting>, WriteError>>,
    abandoned: Arc<AtomicBool>,
    call_limit: Duration,
}
impl ApplyTicket {
    /// Wait at most `call_limit`; see [`Worker::apply`] for what a timeout leaves behind.
    pub fn wait(self) -> Result<Vec<Setting>, WriteError> {
        match self.answer.recv_timeout(self.call_limit) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                self.abandoned.store(true, Ordering::SeqCst);
                Err(WriteError {
                    kind: super::ErrorKind::Unavailable,
                    id: None,
                    message: format!(
                        "the Radeon worker did not answer within {} s; the request was dropped if it had not started, and may still finish if it had",
                        self.call_limit.as_secs()
                    ),
                })
            }
            Err(RecvTimeoutError::Disconnected) => Err(stopped()),
        }
    }
}

/// A queued restore.
pub struct RestoreTicket {
    answer: mpsc::Receiver<anyhow::Result<RestoreReport>>,
    call_limit: Duration,
}
impl RestoreTicket {
    /// Wait at most `call_limit`. A restore that times out stays queued and still runs.
    pub fn wait(self) -> anyhow::Result<RestoreReport> {
        match self.answer.recv_timeout(self.call_limit) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => Err(anyhow::anyhow!(
                "the Radeon worker did not answer within {} s; the restore stays queued and still runs",
                self.call_limit.as_secs()
            )),
            Err(RecvTimeoutError::Disconnected) => {
                Err(anyhow::anyhow!("the Radeon worker stopped"))
            }
        }
    }
}

impl Worker {
    /// Ask the worker something and wait at most `wait`. Queued behind the recovery and any write
    /// or restore ahead of it, so a caller that cannot wait long gets a timeout error and the
    /// answer is dropped.
    fn ask<T>(
        &self,
        wait: Duration,
        job: impl FnOnce(mpsc::Sender<Result<T, BackendError>>) -> Job,
    ) -> Result<T, BackendError> {
        let (reply, answer) = mpsc::channel();
        self.jobs
            .send(job(reply))
            .map_err(|_| BackendError("the Radeon worker stopped".into()))?;
        match answer.recv_timeout(wait) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => Err(BackendError(format!(
                "the Radeon worker did not answer within {} s",
                wait.as_secs()
            ))),
            Err(RecvTimeoutError::Disconnected) => {
                Err(BackendError("the Radeon worker stopped".into()))
            }
        }
    }

    /// The GPU the host works with (see `pick`), or `None` when ADLX lists no AMD GPU.
    pub fn gpu(
        &self,
        adapter: Option<&str>,
        wait: Duration,
    ) -> Result<Option<GpuInfo>, BackendError> {
        self.ask(wait, |reply| Job::Gpu {
            adapter: adapter.map(String::from),
            reply,
        })
    }

    /// Read the GPU's settings from the driver, fresh, and the journal's pending entries. It
    /// runs one helper process, so the caller keeps it off the stream path and caches the answer.
    pub fn panel(
        &self,
        adapter: Option<&str>,
        wait: Duration,
    ) -> Result<Option<Panel>, BackendError> {
        self.ask(wait, |reply| Job::Panel {
            adapter: adapter.map(String::from),
            reply,
        })
    }

    /// What the journal would still put back for the signed-in user. It reads the journal only,
    /// never the driver, so a write that has just read its settings back can show the entries
    /// without a second helper run.
    pub fn pending(&self, wait: Duration) -> Result<Vec<Entry>, BackendError> {
        self.ask(wait, |reply| Job::Pending { reply })
    }
}

fn stopped() -> WriteError {
    WriteError {
        kind: super::ErrorKind::Unavailable,
        id: None,
        message: "the Radeon worker stopped".into(),
    }
}

fn set_status(status: &Mutex<Status>, next: Status) {
    *status.lock().unwrap_or_else(|e| e.into_inner()) = next;
}

/// One recovery pass. Returns whether anything is still unrestored (or could not be tried).
fn recover<B: RadeonBackend>(
    controller: &mut Controller<B>,
    status: &Mutex<Status>,
    timing: Timing,
    retry: bool,
) -> bool {
    controller.set_deadline(Some(Instant::now() + timing.recovery_limit));
    let result = controller.recover_on_startup();
    controller.set_deadline(None);
    let next = match &result {
        Ok(report) => Status {
            phase: if report.is_clean() {
                Phase::Ready
            } else {
                Phase::Degraded
            },
            unrestored: report.failed().count(),
            error: None,
        },
        Err(error) => Status {
            phase: Phase::Degraded,
            unrestored: 0,
            error: Some(format!("{error:#}")),
        },
    };
    let degraded = next.phase == Phase::Degraded;
    if degraded {
        let retry_in = timing.retry_every.as_secs();
        match &result {
            Ok(report) => {
                let ids: Vec<String> = report.failed().map(|o| o.id.clone()).collect();
                if retry {
                    tracing::warn!(
                        settings = %ids.join(", "),
                        retry_in_seconds = retry_in,
                        "Radeon settings from an earlier run are still not restored"
                    );
                } else {
                    tracing::error!(
                        settings = %ids.join(", "),
                        retry_in_seconds = retry_in,
                        "Radeon settings changed by an earlier run could not be restored; they stay journaled and the host retries"
                    );
                }
            }
            Err(error) => tracing::error!(
                error = %format!("{error:#}"),
                retry_in_seconds = retry_in,
                "Radeon recovery could not run; the host retries"
            ),
        }
    } else if retry {
        tracing::info!("Radeon settings from an earlier run are restored");
    }
    set_status(status, next);
    degraded
}

fn run<B: RadeonBackend>(
    mut controller: Controller<B>,
    queue: &mpsc::Receiver<Job>,
    status: &Mutex<Status>,
    timing: Timing,
) {
    // Sessions of this run that may have settings changed; a retry of the older journal entries
    // must not touch them.
    let mut active: HashSet<String> = HashSet::new();
    let mut degraded = recover(&mut controller, status, timing, false);
    // The next retry is due `retry_every` after the worker degraded or last retried. Jobs that
    // come meanwhile do not put it off: a panel that asks more often would put it off for good.
    let mut retry_at = None;
    loop {
        let job = if degraded {
            let due = *retry_at.get_or_insert_with(|| Instant::now() + timing.retry_every);
            let job = match due.checked_duration_since(Instant::now()) {
                Some(left) if !left.is_zero() => match queue.recv_timeout(left) {
                    Ok(job) => Some(job),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                },
                _ => None,
            };
            if job.is_none() {
                retry_at = None;
            }
            job
        } else {
            retry_at = None;
            match queue.recv() {
                Ok(job) => Some(job),
                Err(_) => return,
            }
        };
        match job {
            None if active.is_empty() => degraded = recover(&mut controller, status, timing, true),
            None => tracing::debug!("Radeon recovery retry waits for the live sessions"),
            Some(Job::Apply {
                session,
                gpu,
                changes,
                reply,
                abandoned,
            }) => {
                if abandoned.load(Ordering::SeqCst) {
                    tracing::warn!(session = %session, "dropped a Radeon request its caller gave up on");
                    continue;
                }
                // Even a refused request may leave journaled entries behind.
                active.insert(session.clone());
                let _ = reply.send(controller.apply(&session, gpu, &changes));
            }
            Some(Job::Restore {
                session,
                only,
                reply,
            }) => {
                let result = match &only {
                    Some(ids) => controller.restore(Scope::Settings(&session, ids)),
                    None => controller.restore(Scope::Session(&session)),
                };
                // A restore of some settings leaves the session going.
                if only.is_none() {
                    active.remove(&session);
                }
                match &result {
                    Ok(report) if !report.is_clean() => {
                        tracing::error!(
                            session = %session,
                            settings = %report.failed().map(|o| o.id.as_str()).collect::<Vec<_>>().join(", "),
                            "Radeon settings of a finished session could not be restored; they stay journaled and the host retries"
                        );
                        degraded = true;
                        set_status(
                            status,
                            Status {
                                phase: Phase::Degraded,
                                unrestored: report.failed().count(),
                                error: None,
                            },
                        );
                    }
                    Err(error) => {
                        tracing::error!(
                            session = %session,
                            error = %format!("{error:#}"),
                            "Radeon settings of a finished session could not be restored"
                        );
                        degraded = true;
                        set_status(
                            status,
                            Status {
                                phase: Phase::Degraded,
                                unrestored: 0,
                                error: Some(format!("{error:#}")),
                            },
                        );
                    }
                    Ok(_) => {}
                }
                let _ = reply.send(result);
            }
            Some(Job::Gpu { adapter, reply }) => {
                let _ = reply.send(
                    controller
                        .backend()
                        .gpus()
                        .map(|gpus| pick(gpus, adapter.as_deref())),
                );
            }
            Some(Job::Panel { adapter, reply }) => {
                let _ = reply.send(panel(&mut controller, adapter.as_deref()));
            }
            Some(Job::Pending { reply }) => {
                let _ = reply.send(
                    controller
                        .pending()
                        .map_err(|e| BackendError(format!("the Radeon journal: {e:#}"))),
                );
            }
        }
    }
}

fn panel<B: RadeonBackend>(
    controller: &mut Controller<B>,
    adapter: Option<&str>,
) -> Result<Option<Panel>, BackendError> {
    let Some(gpu) = pick(controller.backend().gpus()?, adapter) else {
        return Ok(None);
    };
    let settings = controller.backend().read(gpu.id)?;
    let pending = controller
        .pending()
        .map_err(|e| BackendError(format!("the Radeon journal: {e:#}")))?;
    Ok(Some(Panel {
        gpu,
        settings,
        pending,
    }))
}

static HOST: OnceLock<Worker> = OnceLock::new();

/// Start the host's worker once, behind the helper-process backend, and begin the startup
/// recovery of `directory`'s journal. Returns at once; the recovery runs on the worker.
pub fn start_host(directory: &Path) -> Result<&'static Worker, BackendError> {
    if let Some(worker) = HOST.get() {
        return Ok(worker);
    }
    let backend = HelperBackend::host()?;
    let worker = Worker::start(backend, directory.join(JOURNAL_FILE), Timing::default());
    Ok(HOST.get_or_init(|| worker))
}

/// The host's worker, once `start_host` has run.
pub fn host() -> Option<&'static Worker> {
    HOST.get()
}

#[cfg(test)]
mod tests;
