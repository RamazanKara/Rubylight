//! The host's side of the Radeon controls (plan item A3, brief 4): which session changed
//! settings, and when they go back.
//!
//! `butterpollo_windows::radeon::worker::Worker` owns the driver, the journal and the restore
//! rules (restore only while the setting still holds the written value, failures stay journaled
//! and the worker retries them). The host adds the link to the session: a write is made on
//! behalf of the device that owns the session (the journal's session id is that device's uuid),
//! and the settings go back when that session ends, whichever way it ends.
//!
//! The session state machine is the one source of truth for "ended": `Host::with_session` calls
//! [`Inner::sync`] after every step, so End session, Close game, the grace period running out,
//! a stock `/cancel` and a stream that ended with no grace all restore through the same line.
//! A device's settings go back only when it is gone from the session entirely (no stream, not
//! the owner, no retained display) and the session has settled. While another device launches
//! (Preparing, Launching, Stopping) nothing is restored, because that launch may fail and give
//! the session back to its owner. A Disconnect leaves a suspended session, which keeps its
//! settings. Unpair and disable have
//! their own hook ([`Inner::release_device`]) because they do not end the session by themselves,
//! and a clean shutdown flushes ([`Inner::shutdown`]).
//!
//! Restores run on a thread of their own: they take seconds, and `sync` runs under the session
//! lock.
use butterpollo_core::native_api::radeon::{ControlState, Gpu};
use butterpollo_windows::radeon::{
    BackendError, Change, Entry, ErrorKind, RestoreReport, Setting, WriteError,
    control::RestoreResult,
    worker::{RestoreTicket, Worker},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Deref,
    sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, mpsc},
    time::{Duration, Instant},
};

/// The host's worker, or one a test started.
#[derive(Clone)]
pub enum WorkerRef {
    Host(&'static Worker),
    #[cfg(test)]
    Owned(Arc<Worker>),
}
impl Deref for WorkerRef {
    type Target = Worker;
    fn deref(&self) -> &Worker {
        match self {
            Self::Host(worker) => worker,
            #[cfg(test)]
            Self::Owned(worker) => worker,
        }
    }
}

/// How long a reading may be reused. A client polling the panel causes one driver read a second
/// at most.
pub const CACHE: Duration = Duration::from_secs(1);
/// How long a read waits for the worker (a write or restore ahead of it, or the driver).
pub const READ_WAIT: Duration = if cfg!(test) {
    Duration::from_secs(5)
} else {
    Duration::from_secs(10)
};

enum Message {
    /// Put a device's settings back, unless it wrote again after this generation was handed on.
    Restore(String, u64),
    /// Answer once every restore before this one has finished.
    Flush(mpsc::Sender<()>),
    /// Hold every later message until the sender is used or dropped. For tests of a restore
    /// that waits in the queue while its device writes again.
    #[cfg(test)]
    Pause(mpsc::Receiver<()>),
}

/// What the panel shows, apart from the caller's own flags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    pub state: ControlState,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub gpus: Vec<Gpu>,
}

/// A reading of the panel, when it was taken and the revision it carries.
pub struct Reading {
    pub at: Instant,
    pub view: Arc<View>,
    pub revision: u64,
}

pub fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Default)]
pub struct Inner {
    worker: OnceLock<WorkerRef>,
    /// Devices that may have settings changed in this run. A device leaves when its restore is
    /// handed on.
    written: Mutex<BTreeSet<String>>,
    /// Per device, how many times it noted a write. A restore handed on for an older generation
    /// is stale: the device wrote again since, and that write is the session's to put back.
    generations: Mutex<BTreeMap<String, u64>>,
    /// Held while a restore checks its generation and queues its job, and while a write notes
    /// itself and queues its apply. Either order is safe; what must not happen is a write
    /// slipping between the restore's check and its queueing, which would put the restore behind
    /// the write and undo it.
    queueing: Mutex<()>,
    /// Test hook: the restore thread stops between its check and its queueing.
    #[cfg(test)]
    window: Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
    restorer: OnceLock<mpsc::Sender<Message>>,
    /// The newest reading. The lock is held through a read, so concurrent requests share it.
    pub cache: Mutex<Option<Reading>>,
    /// The revision the panel shows and the view it was counted for.
    pub revision: Mutex<(u64, Option<Arc<View>>)>,
    /// Per device, the settings its last restore could not put back, with why. The panel shows
    /// one only while the journal still holds an entry of that device (see [`Inner::failed`]).
    failures: Mutex<BTreeMap<String, Vec<(String, String)>>>,
}

#[derive(Default)]
pub struct Radeon {
    inner: Arc<Inner>,
}
impl Deref for Radeon {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.inner
    }
}

impl Radeon {
    /// Use `worker` for every Radeon read, write and restore, and start the thread that runs
    /// restores. Returns false when one is attached already.
    pub fn attach(&self, worker: WorkerRef) -> bool {
        if self.inner.worker.set(worker.clone()).is_err() {
            return false;
        }
        let (sender, messages) = mpsc::channel();
        let inner = self.inner.clone();
        let spawned = std::thread::Builder::new()
            .name("radeon-restore".into())
            .spawn(move || restorer(&inner, &worker, &messages));
        match spawned {
            Ok(_) => {
                let _ = self.inner.restorer.set(sender);
            }
            Err(error) => tracing::error!(
                %error,
                "the Radeon restore thread could not start; settings are restored at the next host start"
            ),
        }
        true
    }
}

impl Inner {
    pub fn worker(&self) -> Option<&Worker> {
        self.worker.get().map(Deref::deref)
    }
    #[cfg(test)]
    pub fn status(&self) -> Option<butterpollo_windows::radeon::worker::Status> {
        self.worker().map(Worker::status)
    }
    /// Whether the worker is still putting back what an earlier run left.
    #[cfg(test)]
    pub fn recovering(&self) -> bool {
        self.status().is_some_and(|status| {
            status.phase == butterpollo_windows::radeon::worker::Phase::Recovering
        })
    }

    /// `device` is about to change the settings `ids` (or has just): its session restores them
    /// when it ends.
    pub fn note_written(&self, device: &str, ids: &[String]) {
        *locked(&self.generations)
            .entry(device.to_owned())
            .or_default() += 1;
        locked(&self.written).insert(device.to_owned());
        // A new write to a setting is news of its own: an old failure is not about it. A failure
        // of another setting stays, as does nothing of a restore that failed as a whole ("*"),
        // which cannot be told from one about this write.
        let mut failures = locked(&self.failures);
        if let Some(list) = failures.get_mut(device) {
            list.retain(|(id, _)| id != "*" && !ids.contains(id));
            if list.is_empty() {
                failures.remove(device);
            }
        }
    }
    /// Write `changes` as `device`: note the device and queue the apply under one lock, so a
    /// restore that is about to be queued for the device's earlier session is either ahead of
    /// this write or skipped, never behind it. Then wait for the driver's read-back.
    pub fn apply(
        &self,
        device: &str,
        gpu: i32,
        changes: Vec<Change>,
    ) -> Result<Vec<Setting>, WriteError> {
        let Some(worker) = self.worker() else {
            return Err(WriteError {
                kind: ErrorKind::Unavailable,
                id: None,
                message: "this host has no Radeon controls".into(),
            });
        };
        let ids: Vec<String> = changes.iter().map(|change| change.id.clone()).collect();
        let ticket = {
            let _queueing = locked(&self.queueing);
            self.note_written(device, &ids);
            worker.queue_apply(device, gpu, changes)
        };
        let result = ticket?.wait();
        // The write may have changed more than it reported, or gone on after a timeout.
        self.note_written(device, &ids);
        result
    }
    /// Forget what the panel showed, after a write or a restore.
    pub fn forget(&self) {
        *locked(&self.cache) = None;
    }
    /// The settings a restore could not put back, with why ("*" for a restore that failed as a
    /// whole). `pending` is the journal now: a failure counts only while the journal still holds
    /// an entry of the device it happened to, so a worker retry that succeeded ends it and
    /// another device's later write does not inherit it.
    pub fn failed(&self, pending: &[Entry]) -> BTreeMap<String, String> {
        locked(&self.failures)
            .iter()
            .filter(|(device, _)| pending.iter().any(|entry| entry.session == **device))
            .flat_map(|(device, list)| {
                list.iter().filter(move |(id, _)| {
                    id == "*"
                        || pending
                            .iter()
                            .any(|entry| entry.session == *device && entry.id == *id)
                })
            })
            .cloned()
            .collect()
    }

    fn generation(&self, device: &str) -> u64 {
        locked(&self.generations)
            .get(device)
            .copied()
            .unwrap_or_default()
    }

    fn dispatch(&self, device: String) {
        let Some(restorer) = self.restorer.get() else {
            tracing::error!(
                session = %device,
                "no Radeon restore thread; the settings stay journaled until the next host start"
            );
            return;
        };
        let generation = self.generation(&device);
        if restorer.send(Message::Restore(device, generation)).is_err() {
            tracing::error!(
                "the Radeon restore thread stopped; settings stay journaled until the next host start"
            );
        }
    }

    /// The session machine moved. `present` makes the devices still part of the session (a
    /// stream, the owner, a retained display), or `None` while the session has not settled (a
    /// launch or a teardown runs): nothing is restored then, because a launch that fails hands
    /// the session back to its owner. Every device that wrote settings and is not present gets
    /// them restored: that is an ended session, whichever way it ended. A suspended session
    /// keeps its owner, so Disconnect restores nothing. `present` is not called when no device
    /// wrote anything.
    pub fn sync(&self, present: impl FnOnce() -> Option<BTreeSet<String>>) {
        let ended: Vec<String> = {
            let mut written = locked(&self.written);
            if written.is_empty() {
                return;
            }
            let Some(present) = present() else {
                return;
            };
            let ended: Vec<String> = written
                .iter()
                .filter(|device| !present.contains(*device))
                .cloned()
                .collect();
            for device in &ended {
                written.remove(device);
            }
            ended
        };
        for device in ended {
            self.dispatch(device);
        }
    }
    /// A device was unpaired or turned off: its settings go back even though its session may
    /// linger until the grace period ends.
    pub fn release_device(&self, device: &str) {
        if locked(&self.written).remove(device) {
            self.dispatch(device.to_owned());
        }
    }
    /// Every device's settings go back, whatever its session is doing (unpair all, shutdown).
    pub fn release_all(&self) {
        let devices: Vec<String> = std::mem::take(&mut *locked(&self.written))
            .into_iter()
            .collect();
        for device in devices {
            self.dispatch(device);
        }
    }
    /// A clean shutdown: put back everything this run changed and wait for it, at most `wait`.
    pub fn shutdown(&self, wait: Duration) {
        self.release_all();
        let Some(restorer) = self.restorer.get() else {
            return;
        };
        let (ack, done) = mpsc::channel();
        if restorer.send(Message::Flush(ack)).is_ok() && done.recv_timeout(wait).is_err() {
            tracing::error!(
                wait_seconds = wait.as_secs(),
                "Radeon settings were still being restored at shutdown; what is left stays journaled and the next host start restores it"
            );
        }
    }
    /// Put `device`'s settings back now and wait for the worker. The session goes on: its next
    /// write notes the device again.
    pub fn restore_now(&self, device: &str) -> anyhow::Result<RestoreReport> {
        let worker = self
            .worker()
            .ok_or_else(|| anyhow::anyhow!("this host has no Radeon controls"))?;
        locked(&self.written).remove(device);
        let result = worker.restore_session(device);
        self.record(device, &result);
        // What stayed changed is still the session's to put back when it ends.
        if result.as_ref().map_or(true, |report| !report.is_clean()) {
            locked(&self.written).insert(device.to_owned());
        }
        result
    }
    /// Remember what a restore could not put back, so the panel can say so.
    fn record(&self, device: &str, result: &anyhow::Result<RestoreReport>) {
        let failed: Vec<(String, String)> = match result {
            Ok(report) => report
                .failed()
                .map(|outcome| {
                    let why = match &outcome.result {
                        RestoreResult::Failed(why) => why.clone(),
                        _ => String::new(),
                    };
                    (outcome.id.clone(), why)
                })
                .collect(),
            Err(error) => vec![("*".into(), format!("{error:#}"))],
        };
        let mut failures = locked(&self.failures);
        if failed.is_empty() {
            failures.remove(device);
        } else {
            failures.insert(device.to_owned(), failed);
        }
        drop(failures);
        self.forget();
    }
    /// Whether `device` may have settings changed that its session has not restored.
    pub fn holds(&self, device: &str) -> bool {
        locked(&self.written).contains(device)
    }
    /// Stop the restore thread after its next generation check, before it queues the restore,
    /// until the sender is used or dropped. The receiver says it got there. For tests.
    #[cfg(test)]
    pub fn hold_restore_after_check(&self) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (reached, arrived) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        *locked(&self.window) = Some((reached, wait));
        (arrived, release)
    }
    /// How often `device` noted a write. For tests.
    #[cfg(test)]
    pub fn writes_noted(&self, device: &str) -> u64 {
        self.generation(device)
    }
    /// Stop the restore thread until the returned sender is dropped. For tests.
    #[cfg(test)]
    pub fn pause_restores(&self) -> mpsc::Sender<()> {
        let (resume, wait) = mpsc::channel();
        if let Some(restorer) = self.restorer.get() {
            let _ = restorer.send(Message::Pause(wait));
        }
        resume
    }
    /// Wait until every restore handed on so far has finished, at most `wait`. For tests.
    #[cfg(test)]
    pub fn flush(&self, wait: Duration) -> bool {
        let Some(restorer) = self.restorer.get() else {
            return true;
        };
        let (ack, done) = mpsc::channel();
        restorer.send(Message::Flush(ack)).is_ok() && done.recv_timeout(wait).is_ok()
    }
}

fn restorer(inner: &Inner, worker: &Worker, messages: &mpsc::Receiver<Message>) {
    while let Ok(message) = messages.recv() {
        match message {
            Message::Restore(device, generation) => {
                // No write may note itself between the check and the queueing.
                let queued = {
                    let _queueing = locked(&inner.queueing);
                    if inner.generation(&device) != generation {
                        tracing::info!(
                            session = %device,
                            "a Radeon restore was skipped: the device wrote again after its session ended"
                        );
                        continue;
                    }
                    #[cfg(test)]
                    if let Some((reached, release)) = locked(&inner.window).take() {
                        let _ = reached.send(());
                        let _ = release.recv();
                    }
                    worker.queue_restore(&device)
                };
                // The worker logs a restore that fails, with the setting ids, and retries it.
                let result = queued.and_then(RestoreTicket::wait);
                inner.record(&device, &result);
            }
            Message::Flush(ack) => {
                let _ = ack.send(());
            }
            #[cfg(test)]
            Message::Pause(resume) => {
                let _ = resume.recv();
            }
        }
    }
}

/// Start the host's worker, with the helper-process backend, and attach it.
pub fn start(radeon: &Radeon, directory: &std::path::Path) -> Result<(), BackendError> {
    let worker = butterpollo_windows::radeon::worker::start_host(directory)?;
    radeon.attach(WorkerRef::Host(worker));
    Ok(())
}
