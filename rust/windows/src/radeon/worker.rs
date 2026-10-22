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
        let (reply, answer) = mpsc::channel();
        let job = Job::Restore {
            session: session.into(),
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
    loop {
        let job = if degraded {
            match queue.recv_timeout(timing.retry_every) {
                Ok(job) => Some(job),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        } else {
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
            Some(Job::Restore { session, reply }) => {
                let result = controller.restore(Scope::Session(&session));
                active.remove(&session);
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
mod tests {
    use super::*;
    use crate::radeon::{
        control::{Entry, Journal},
        model::{Kind, Range, State, Value, Writable},
    };
    use std::collections::BTreeMap;

    fn setting(id: &'static str, kind: Kind, value: Value) -> Setting {
        Setting {
            id,
            group: id.split('.').next().unwrap(),
            label: id,
            interface: "fake",
            kind,
            state: State::Configurable,
            supported: Some(true),
            readable: Some(true),
            writable: Writable::Yes,
            value: Some(value),
            unit: None,
            range: (kind == Kind::Integer).then_some(Range {
                min: 30,
                max: 300,
                step: 10,
            }),
            options: vec![],
            depends: vec![],
            notes: vec![],
            adlx: BTreeMap::new(),
        }
    }

    #[derive(Default)]
    struct Panel {
        settings: Vec<Setting>,
        events: Vec<String>,
        /// Refuse writes of this id.
        refuse: Option<&'static str>,
    }
    /// A driver the worker thread can own while the test watches it.
    #[derive(Clone)]
    struct Driver {
        panel: Arc<Mutex<Panel>>,
        /// The first read waits for a message here.
        hold_first_read: Arc<Mutex<Option<mpsc::Receiver<()>>>>,
    }
    impl RadeonBackend for Driver {
        fn read(&mut self, _gpu: i32) -> Result<Vec<Setting>, BackendError> {
            let gate = self.hold_first_read.lock().unwrap().take();
            if let Some(gate) = gate {
                let _ = gate.recv();
            }
            Ok(self.panel.lock().unwrap().settings.clone())
        }
        fn write(&mut self, _gpu: i32, id: &str, value: &Value) -> Result<(), BackendError> {
            let mut panel = self.panel.lock().unwrap();
            panel.events.push(format!("{id}={}", value.display()));
            if panel.refuse == Some(id) {
                return Err(BackendError("ADLX_FAIL".into()));
            }
            if let Some(s) = panel.settings.iter_mut().find(|s| s.id == id) {
                s.value = Some(value.clone());
            }
            Ok(())
        }
        fn gpus(&mut self) -> Result<Vec<GpuInfo>, BackendError> {
            Ok(vec![
                GpuInfo {
                    id: 1,
                    name: "AMD Radeon RX 7900 XT".into(),
                    driver: "25.9.1".into(),
                },
                GpuInfo {
                    id: 2,
                    name: "AMD Radeon(TM) Graphics".into(),
                    driver: "25.9.1".into(),
                },
            ])
        }
    }
    impl Driver {
        fn new() -> (Self, mpsc::Sender<()>) {
            let (open, gate) = mpsc::channel();
            let driver = Self {
                panel: Arc::new(Mutex::new(Panel {
                    settings: vec![
                        setting("anti_lag.enabled", Kind::Toggle, Value::Bool(false)),
                        setting("pacing.limit", Kind::Integer, Value::Int(140)),
                    ],
                    ..Panel::default()
                })),
                hold_first_read: Arc::new(Mutex::new(Some(gate))),
            };
            (driver, open)
        }
        fn without_gate(self) -> Self {
            self.hold_first_read.lock().unwrap().take();
            self
        }
        fn value(&self, id: &str) -> Value {
            let panel = self.panel.lock().unwrap();
            panel
                .settings
                .iter()
                .find(|s| s.id == id)
                .and_then(|s| s.value.clone())
                .unwrap()
        }
        fn events(&self) -> Vec<String> {
            self.panel.lock().unwrap().events.clone()
        }
    }

    /// A crashed run left Anti-Lag changed from on to off.
    fn crashed_journal(dir: &Path) -> PathBuf {
        let path = dir.join(JOURNAL_FILE);
        Journal {
            version: 1,
            owner: None,
            entries: vec![Entry {
                user: crate::process::user_sid().expect("the test account has a SID"),
                gpu: 1,
                id: "anti_lag.enabled".into(),
                session: "crashed".into(),
                original: Value::Bool(true),
                written: Value::Bool(false),
                side_effect: false,
            }],
        }
        .save(&path)
        .unwrap();
        path
    }
    fn quick() -> Timing {
        Timing {
            recovery_limit: Duration::from_secs(60),
            retry_every: Duration::from_millis(50),
            call_limit: Duration::from_secs(60),
        }
    }
    fn until(what: &str, mut done: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < end, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn change(id: &str, value: Value) -> Change {
        Change {
            id: id.into(),
            value,
        }
    }

    #[test]
    fn startup_returns_at_once_and_a_write_waits_for_the_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let journal = crashed_journal(dir.path());
        let (driver, open) = Driver::new();
        let started = Instant::now();
        let worker = Arc::new(Worker::start(driver.clone(), journal, quick()));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "starting must not wait for the recovery"
        );
        assert_eq!(worker.status().phase, Phase::Recovering);
        let (done, finished) = mpsc::channel();
        let queued = worker.clone();
        std::thread::spawn(move || {
            let result = queued.apply("live", 1, vec![change("pacing.limit", Value::Int(150))]);
            let _ = done.send(result.is_ok());
        });
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            finished.try_recv().is_err(),
            "the write must wait while the recovery is still reading"
        );
        assert!(driver.events().is_empty());
        open.send(()).unwrap();
        assert!(finished.recv_timeout(Duration::from_secs(10)).unwrap());
        assert_eq!(
            driver.events(),
            ["anti_lag.enabled=on", "pacing.limit=150"],
            "the crashed run's setting is put back before the new write"
        );
        assert_eq!(worker.status().phase, Phase::Ready);
    }

    #[test]
    fn a_caller_gives_up_after_the_call_limit_and_its_unstarted_write_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let journal = crashed_journal(dir.path());
        let (driver, open) = Driver::new();
        let timing = Timing {
            call_limit: Duration::from_millis(200),
            ..quick()
        };
        let worker = Worker::start(driver.clone(), journal, timing);
        // The recovery is stuck on its first read, so nothing behind it can start.
        let started = Instant::now();
        let error = worker
            .apply("late", 1, vec![change("pacing.limit", Value::Int(150))])
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(error.kind, crate::radeon::ErrorKind::Unavailable);
        assert!(
            error.message.contains("did not answer"),
            "{}",
            error.message
        );
        let error = worker.restore_session("late").unwrap_err();
        assert!(format!("{error:#}").contains("still runs"), "{error:#}");
        open.send(()).unwrap();
        until("the recovery to finish", || {
            worker.status().phase == Phase::Ready
        });
        // A restore of a session with no entries is a no-op; wait for the queue to drain.
        worker.restore_session("drain").unwrap();
        assert_eq!(
            driver.events(),
            ["anti_lag.enabled=on"],
            "only the crashed run's setting was written; the abandoned request never ran"
        );
        assert_eq!(driver.value("pacing.limit"), Value::Int(140));
    }

    #[test]
    fn a_recovery_that_cannot_restore_is_reported_and_retried() {
        let dir = tempfile::tempdir().unwrap();
        let journal = crashed_journal(dir.path());
        let (driver, _open) = Driver::new();
        let driver = driver.without_gate();
        driver.panel.lock().unwrap().refuse = Some("anti_lag.enabled");
        let worker = Worker::start(driver.clone(), journal.clone(), quick());
        until("the recovery to report", || {
            worker.status().phase != Phase::Recovering
        });
        let status = worker.status();
        assert_eq!(status.phase, Phase::Degraded);
        assert_eq!(status.unrestored, 1);
        assert_eq!(driver.value("anti_lag.enabled"), Value::Bool(false));
        assert_eq!(Journal::load(&journal).unwrap().entries.len(), 1);
        driver.panel.lock().unwrap().refuse = None;
        until("the retry to restore", || {
            worker.status().phase == Phase::Ready
        });
        assert_eq!(driver.value("anti_lag.enabled"), Value::Bool(true));
        assert!(Journal::load(&journal).unwrap().entries.is_empty());
    }

    #[test]
    fn a_recovery_that_cannot_start_is_reported_with_its_reason() {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path().join(JOURNAL_FILE);
        std::fs::write(&journal, "{ not json").unwrap();
        let (driver, _open) = Driver::new();
        let worker = Worker::start(driver.without_gate(), journal, quick());
        until("the recovery to report", || {
            worker.status().phase != Phase::Recovering
        });
        let status = worker.status();
        assert_eq!(status.phase, Phase::Degraded);
        assert!(
            status.error.unwrap().contains("journal"),
            "names the journal"
        );
    }

    #[test]
    fn a_retry_leaves_a_live_session_alone_and_runs_once_it_is_restored() {
        let dir = tempfile::tempdir().unwrap();
        let journal = crashed_journal(dir.path());
        let (driver, _open) = Driver::new();
        let driver = driver.without_gate();
        driver.panel.lock().unwrap().refuse = Some("anti_lag.enabled");
        let worker = Worker::start(driver.clone(), journal, quick());
        until("the recovery to report", || {
            worker.status().phase == Phase::Degraded
        });
        worker
            .apply("live", 1, vec![change("pacing.limit", Value::Int(150))])
            .unwrap();
        driver.panel.lock().unwrap().refuse = None;
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(
            driver.value("anti_lag.enabled"),
            Value::Bool(false),
            "no retry while a session of this run may have settings changed"
        );
        assert_eq!(driver.value("pacing.limit"), Value::Int(150));
        worker.restore_session("live").unwrap();
        assert_eq!(driver.value("pacing.limit"), Value::Int(140));
        until("the retry after the session ended", || {
            worker.status().phase == Phase::Ready
        });
        assert_eq!(driver.value("anti_lag.enabled"), Value::Bool(true));
    }

    #[test]
    fn the_adapter_name_picks_the_gpu_and_the_first_one_is_the_default() {
        let gpu = |id, name: &str| GpuInfo {
            id,
            name: name.into(),
            driver: String::new(),
        };
        let both = || {
            vec![
                gpu(1, "AMD Radeon RX 7900 XT"),
                gpu(2, "AMD Radeon(TM) Graphics"),
            ]
        };
        assert_eq!(pick(both(), Some("graphics")).unwrap().id, 2);
        assert_eq!(pick(both(), Some("rx 7900")).unwrap().id, 1);
        assert_eq!(pick(both(), Some("no such adapter")).unwrap().id, 1);
        assert_eq!(pick(both(), Some("")).unwrap().id, 1);
        assert_eq!(pick(both(), None).unwrap().id, 1);
        assert!(pick(vec![], Some("x")).is_none());
    }

    #[test]
    fn a_panel_reads_the_picked_gpu_and_what_the_journal_would_put_back() {
        let dir = tempfile::tempdir().unwrap();
        let (driver, _open) = Driver::new();
        let driver = driver.without_gate();
        let worker = Worker::start(driver.clone(), dir.path().join(JOURNAL_FILE), quick());
        until("the recovery", || worker.status().phase == Phase::Ready);
        let wait = Duration::from_secs(10);
        assert_eq!(worker.gpu(Some("graphics"), wait).unwrap().unwrap().id, 2);
        let panel = worker.panel(None, wait).unwrap().unwrap();
        assert_eq!(panel.gpu.id, 1);
        assert_eq!(panel.settings.len(), 2);
        assert!(panel.pending.is_empty());

        worker
            .apply("live", 1, vec![change("pacing.limit", Value::Int(150))])
            .unwrap();
        let panel = worker.panel(None, wait).unwrap().unwrap();
        let [entry] = panel.pending.as_slice() else {
            panic!("one pending entry: {:?}", panel.pending);
        };
        assert_eq!(entry.id, "pacing.limit");
        assert_eq!(entry.original, Value::Int(140));
        assert_eq!(entry.written, Value::Int(150));
        // The journal alone gives the same entries, without a driver read.
        assert_eq!(worker.pending(wait).unwrap(), panel.pending);
        worker.restore_session("live").unwrap();
        assert!(worker.pending(wait).unwrap().is_empty());
        assert!(
            worker
                .panel(None, wait)
                .unwrap()
                .unwrap()
                .pending
                .is_empty()
        );
    }

    #[test]
    fn a_panel_read_behind_a_stuck_driver_gives_up_after_its_wait() {
        let dir = tempfile::tempdir().unwrap();
        let (driver, open) = Driver::new();
        let worker = Worker::start(driver, dir.path().join(JOURNAL_FILE), quick());
        // The first read (the panel's) waits for the gate.
        let error = worker
            .panel(None, Duration::from_millis(200))
            .expect_err("the worker is busy");
        assert!(error.0.contains("did not answer"), "{error}");
        open.send(()).unwrap();
    }
}
