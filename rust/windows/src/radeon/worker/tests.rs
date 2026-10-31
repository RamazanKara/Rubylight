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
fn a_retry_runs_on_time_while_the_panel_keeps_asking() {
    let dir = tempfile::tempdir().unwrap();
    let journal = crashed_journal(dir.path());
    let (driver, _open) = Driver::new();
    let driver = driver.without_gate();
    driver.panel.lock().unwrap().refuse = Some("anti_lag.enabled");
    let timing = Timing {
        retry_every: Duration::from_millis(200),
        ..quick()
    };
    let worker = Worker::start(driver.clone(), journal, timing);
    until("the recovery to report", || {
        worker.status().phase == Phase::Degraded
    });
    driver.panel.lock().unwrap().refuse = None;
    // Each read is a job of its own, more often than the retry is due.
    until("the retry while the panel reads", || {
        worker.pending(Duration::from_secs(5)).unwrap();
        worker.status().phase == Phase::Ready
    });
    assert_eq!(driver.value("anti_lag.enabled"), Value::Bool(true));
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
