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
//! A Disconnect leaves a suspended session, which keeps its settings. Unpair and disable have
//! their own hook ([`Inner::release_device`]) because they do not end the session by themselves,
//! and a clean shutdown flushes ([`Inner::shutdown`]).
//!
//! Restores run on a thread of their own: they take seconds, and `sync` runs under the session
//! lock.
use butterpollo_core::native_api::radeon::{ControlState, Gpu};
use butterpollo_windows::radeon::{
    BackendError, RestoreReport, control::RestoreResult, worker::Worker,
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
    Restore(String),
    /// Answer once every restore before this one has finished.
    Flush(mpsc::Sender<()>),
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
    restorer: OnceLock<mpsc::Sender<Message>>,
    /// The newest reading. The lock is held through a read, so concurrent requests share it.
    pub cache: Mutex<Option<Reading>>,
    /// The revision the panel shows and the view it was counted for.
    pub revision: Mutex<(u64, Option<Arc<View>>)>,
    /// Per device, the settings its last restore could not put back, with why.
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

    /// `device` is about to change settings: its session restores them when it ends.
    pub fn note_written(&self, device: &str) {
        locked(&self.written).insert(device.to_owned());
    }
    /// Forget what the panel showed, after a write or a restore.
    pub fn forget(&self) {
        *locked(&self.cache) = None;
    }
    /// The settings the last restore of any device could not put back, with why.
    pub fn failed(&self) -> BTreeMap<String, String> {
        locked(&self.failures).values().flatten().cloned().collect()
    }

    fn dispatch(&self, device: String) {
        let Some(restorer) = self.restorer.get() else {
            tracing::error!(
                session = %device,
                "no Radeon restore thread; the settings stay journaled until the next host start"
            );
            return;
        };
        if restorer.send(Message::Restore(device)).is_err() {
            tracing::error!(
                "the Radeon restore thread stopped; settings stay journaled until the next host start"
            );
        }
    }

    /// The session machine moved. `owner` is the device that owns the session now, `None` when
    /// it is idle. Every device that wrote settings and no longer owns the session gets them
    /// restored: that is an ended session, whichever way it ended. A suspended session keeps
    /// its owner, so Disconnect restores nothing.
    pub fn sync(&self, owner: Option<&str>) {
        let ended: Vec<String> = {
            let mut written = locked(&self.written);
            if written.is_empty() {
                return;
            }
            let ended: Vec<String> = written
                .iter()
                .filter(|device| Some(device.as_str()) != owner)
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
    /// A clean shutdown: put back everything this run changed and wait for it, at most `wait`.
    pub fn shutdown(&self, wait: Duration) {
        self.sync(None);
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
        locked(&self.written).remove(device);
        let worker = self
            .worker()
            .ok_or_else(|| anyhow::anyhow!("this host has no Radeon controls"))?;
        let result = worker.restore_session(device);
        self.record(device, &result);
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
            Message::Restore(device) => {
                // The worker logs a restore that fails, with the setting ids, and retries it.
                let result = worker.restore_session(&device);
                inner.record(&device, &result);
            }
            Message::Flush(ack) => {
                let _ = ack.send(());
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
