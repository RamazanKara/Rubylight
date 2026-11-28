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
    starts: AtomicUsize,
    leaves: AtomicUsize,
    peer: Arc<MockPeer>,
}

impl FakeBackend {
    fn new(lock_blocked: bool) -> Arc<Self> {
        Arc::new(Self {
            lock_blocked: AtomicBool::new(lock_blocked),
            starts: AtomicUsize::new(0),
            leaves: AtomicUsize::new(0),
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
        true
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
        Vec::new()
    }
    fn ask_host(
        &self,
        _identity: &Arc<ClientIdentity>,
        _record: &HostRecord,
        _asking: Ask,
    ) -> Outcome {
        Outcome::Unreachable("offline".into())
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
    ctrl.sync(false, false, id);
    wait_until(|| !ctrl.status().running && backend.peer.stopped.load(Ordering::SeqCst) == 2);

    // Leave cleans saved file and finishes with Ok.
    ctrl.request_leave();
    wait_until(|| ctrl.take_leave_finished().is_some());
    assert_eq!(backend.leaves.load(Ordering::SeqCst), 1);

    ctrl.stop_and_join();
    let _ = std::fs::remove_dir_all(&dir);
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
