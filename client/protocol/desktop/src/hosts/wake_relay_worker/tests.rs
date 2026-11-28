//! Unit tests for GUI wake relay worker lifecycle (start, stream pause/resume, toggle-off,
//! lock conflict, re-enroll, and Leave cleanup).

use super::*;
use std::sync::atomic::AtomicUsize;

struct MockPeer {
    enrollments: Mutex<Vec<Vec<nightjar_core::wake_relay::driver::Enrollment>>>,
    stopped: AtomicUsize,
}

struct MockPeerHandle(Arc<MockPeer>);

impl RelayPeerHandle for MockPeerHandle {
    fn set_enrollments(&self, enrollments: Vec<nightjar_core::wake_relay::driver::Enrollment>) {
        self.0.enrollments.lock().unwrap().push(enrollments);
    }
    fn status(&self) -> RelayStatus {
        RelayStatus {
            targets: 1,
            devices: 2,
            last_wake: Some(("Gaming PC".into(), 100, Status::HostAwake)),
            ..RelayStatus::default()
        }
    }
    fn stop(&self) {
        self.0.stopped.fetch_add(1, Ordering::SeqCst);
    }
}

struct FakeBackend {
    lock_blocked: AtomicBool,
    has_lan: AtomicBool,
    starts: AtomicUsize,
    leaves: AtomicUsize,
    asked: AtomicUsize,
    paired: Mutex<Vec<HostRecord>>,
    enroll_outcome: Mutex<Option<Outcome>>,
    peer: Arc<MockPeer>,
}

impl FakeBackend {
    fn new(lock_blocked: bool) -> Arc<Self> {
        Arc::new(Self {
            lock_blocked: AtomicBool::new(lock_blocked),
            has_lan: AtomicBool::new(true),
            starts: AtomicUsize::new(0),
            leaves: AtomicUsize::new(0),
            asked: AtomicUsize::new(0),
            paired: Mutex::new(Vec::new()),
            enroll_outcome: Mutex::new(None),
            peer: Arc::new(MockPeer {
                enrollments: Mutex::new(Vec::new()),
                stopped: AtomicUsize::new(0),
            }),
        })
    }
}

impl WorkerBackend for FakeBackend {
    fn try_lock(&self, dir: &Path) -> io::Result<Option<RelayProcessLock>> {
        if self.lock_blocked.load(Ordering::Acquire) {
            Ok(None)
        } else {
            try_acquire_process_lock(dir)
        }
    }
    fn has_lan(&self) -> bool {
        self.has_lan.load(Ordering::Acquire)
    }
    fn start_peer(&self) -> io::Result<Box<dyn RelayPeerHandle>> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(MockPeerHandle(self.peer.clone())))
    }
    fn load_saved(&self, dir: &Path) -> io::Result<saved::Saved> {
        saved::load(dir)
    }
    fn save_saved(&self, dir: &Path, saved: &saved::Saved) -> io::Result<()> {
        saved::save(dir, saved)
    }
    fn paired_hosts(&self, _dir: &Path, _read_only: bool) -> Vec<HostRecord> {
        self.paired.lock().unwrap().clone()
    }
    fn ask_host(
        &self,
        _identity: &Arc<ClientIdentity>,
        _record: &HostRecord,
        _asking: Ask,
    ) -> Outcome {
        self.asked.fetch_add(1, Ordering::SeqCst);
        self.enroll_outcome
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| Outcome::Unreachable("offline".into()))
    }
    fn leave_hosts(
        &self,
        _saved: &saved::Saved,
        _identity: &Arc<ClientIdentity>,
        dir: &Path,
        _read_only: bool,
    ) -> LeaveReport {
        self.leaves.fetch_add(1, Ordering::SeqCst);
        let _ = saved::remove(dir);
        LeaveReport {
            steps: Vec::new(),
            remove_error: None,
            had_failures: false,
        }
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nightjar-gui-relay-{tag}-{}-{}",
        std::process::id(),
        driver::unix_now()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn wait_until(mut pred: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        if pred() {
            return;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    assert!(pred(), "timed out waiting for wake relay worker transition");
}

#[test]
fn worker_starts_pauses_while_streaming_resumes_and_leaves_cleanly() {
    let dir = temp_dir("lifecycle");
    let wake_count = Arc::new(AtomicUsize::new(0));
    let wc = wake_count.clone();
    let waker: Waker = Arc::new(move || {
        wc.fetch_add(1, Ordering::SeqCst);
    });
    let id = Some(Arc::new(ClientIdentity::generate().expect("a key")));
    let backend = FakeBackend::new(false);
    let mut ctrl = WakeRelayController::with_backend(dir.clone(), false, waker, backend.clone());

    ctrl.sync(true, false, id.clone());
    wait_until(|| ctrl.status().running && backend.starts.load(Ordering::SeqCst) == 1);
    assert!(!ctrl.status().paused_for_stream);
    assert_eq!(ctrl.status().targets, 1);
    assert_eq!(ctrl.status().devices, 2);

    // Streaming pauses the peer cleanly without turning off `running`.
    ctrl.sync(true, true, id.clone());
    wait_until(|| {
        ctrl.status().paused_for_stream && backend.peer.stopped.load(Ordering::SeqCst) == 1
    });

    // Ending the stream resumes it again.
    ctrl.sync(true, false, id.clone());
    wait_until(|| !ctrl.status().paused_for_stream && backend.starts.load(Ordering::SeqCst) == 2);

    // Toggle off stops cleanly.
    ctrl.sync(false, false, id.clone());
    wait_until(|| !ctrl.status().running && backend.peer.stopped.load(Ordering::SeqCst) == 2);

    // Leave cleans saved file and finishes with Ok.
    ctrl.request_leave(id);
    wait_until(|| ctrl.take_leave_finished().is_some());
    assert_eq!(backend.leaves.load(Ordering::SeqCst), 1);

    // Repeated sync frames after Leave completes must never re-run leave_hosts.
    let again_id = Some(Arc::new(ClientIdentity::generate().expect("a key")));
    for _ in 0..5 {
        ctrl.sync(false, false, again_id.clone());
    }
    std::thread::sleep(Duration::from_millis(40));
    assert!(ctrl.take_leave_finished().is_none());
    assert_eq!(backend.leaves.load(Ordering::SeqCst), 1);

    ctrl.stop_and_join();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn reenroll_waits_for_stream_and_retains_request_across_conflict_retry() {
    let dir = temp_dir("reenroll-guard");
    let id = Some(Arc::new(ClientIdentity::generate().expect("a key")));
    let backend = FakeBackend::new(false);
    let record = HostRecord {
        name: "Gaming PC".into(),
        address: "192.168.1.50".into(),
        unique_id: "pc-uid-2".into(),
        ..HostRecord::default()
    };
    *backend.paired.lock().unwrap() = vec![record];
    let mut saved = saved::Saved::default();
    saved.hosts.push(saved::SavedHost {
        host: "pc-uid-2".into(),
        name: "Gaming PC".into(),
        assignment: None,
        refused: Some("removed".into()),
    });
    saved::save(&dir, &saved).unwrap();

    let waker: Waker = Arc::new(|| {});
    let mut ctrl = WakeRelayController::with_backend(dir.clone(), false, waker, backend.clone());
    ctrl.sync(true, true, id.clone());
    // Wait briefly to confirm streaming blocks re-enroll start_active.
    ctrl.reenroll();
    std::thread::sleep(Duration::from_millis(40));
    assert_eq!(backend.starts.load(Ordering::SeqCst), 0);

    // Hold lock briefly when stream stops so first start_active(..., enroll_again=true) returns false.
    backend.lock_blocked.store(true, Ordering::Release);
    ctrl.sync(true, false, id.clone());
    wait_until(|| ctrl.status().conflict);
    assert_eq!(backend.starts.load(Ordering::SeqCst), 0);

    // Clear conflict -> re-enroll must still run with enroll_again=true and ask the refused host!
    backend.lock_blocked.store(false, Ordering::Release);
    wait_until(|| {
        backend.asked.load(Ordering::SeqCst) >= 1 && backend.starts.load(Ordering::SeqCst) == 1
    });

    // Calling request_leave immediately without a prior sync(false) cleans saved file reliably.
    ctrl.request_leave(id);
    wait_until(|| ctrl.take_leave_finished().is_some());
    assert_eq!(backend.leaves.load(Ordering::SeqCst), 1);

    ctrl.stop_and_join();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn initial_empty_saved_enrolls_paired_hosts_and_no_lan_stays_not_running() {
    use nightjar_core::http::native_api::wake_relay::RelayAssignment;
    use nightjar_desktop_config::config::HostRecord;

    let dir = temp_dir("initial-enroll");
    let id = Some(Arc::new(ClientIdentity::generate().expect("a key")));
    let backend = FakeBackend::new(false);
    let record = HostRecord {
        name: "Gaming PC".into(),
        address: "192.168.1.50".into(),
        unique_id: "pc-uid-1".into(),
        ..HostRecord::default()
    };
    *backend.paired.lock().unwrap() = vec![record];
    *backend.enroll_outcome.lock().unwrap() = Some(Outcome::Assigned(Box::new(RelayAssignment {
        target_id: "pc-uid-1".into(),
        target_name: "Gaming PC".into(),
        epoch: 1,
        secret: "a".repeat(64),
        macs: vec!["aa:bb:cc:dd:ee:ff".into()],
        broadcasts: vec!["192.168.1.255".into()],
        address: Some("192.168.1.50".into()),
        http_port: 47989,
        devices: vec!["c".repeat(64)],
        refresh_seconds: 300,
    })));

    let waker: Waker = Arc::new(|| {});
    let mut ctrl = WakeRelayController::with_backend(dir.clone(), false, waker, backend.clone());
    ctrl.sync(true, false, id.clone());
    wait_until(|| {
        backend.asked.load(Ordering::SeqCst) >= 1
            && !backend
                .peer
                .enrollments
                .lock()
                .unwrap()
                .last()
                .is_none_or(Vec::is_empty)
    });
    assert_eq!(backend.asked.load(Ordering::SeqCst), 1);

    // Without active IPv4 LAN on peer start, running stays false alongside the error.
    let dir_no_lan = temp_dir("no-lan");
    let backend_no_lan = FakeBackend::new(false);
    backend_no_lan.has_lan.store(false, Ordering::Release);
    let waker2: Waker = Arc::new(|| {});
    let mut ctrl2 =
        WakeRelayController::with_backend(dir_no_lan.clone(), false, waker2, backend_no_lan);
    ctrl2.sync(true, false, id);
    wait_until(|| ctrl2.status().error.is_some());
    assert!(!ctrl2.status().running);

    ctrl2.stop_and_join();
    ctrl.stop_and_join();
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dir_no_lan);
}

#[test]
fn worker_surfaces_lock_conflict_when_cli_holds_lock() {
    let dir = temp_dir("conflict");
    let _cli_lock = try_acquire_process_lock(&dir)
        .unwrap()
        .expect("acquired lock");
    let waker: Waker = Arc::new(|| {});
    let backend = FakeBackend::new(true);
    let mut ctrl = WakeRelayController::with_backend(dir.clone(), false, waker, backend.clone());
    ctrl.sync(true, false, None);
    wait_until(|| ctrl.status().conflict);
    assert!(!ctrl.status().running);
    assert_eq!(backend.starts.load(Ordering::SeqCst), 0);
    ctrl.stop_and_join();
    let _ = std::fs::remove_dir_all(&dir);
}
