//! In-process GUI wake-relay worker (`settings.wake_relay`) in `hosts/wake_relay_worker.rs`.
//! Coordinates access to `wake-relay.json` via `wake-relay.lock`, pauses while streaming
//! (`self.streaming.is_some()`), and stops cleanly on toggle-off, Leave, or app exit.

mod shared;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use nightjar_core::http::ClientIdentity;
use nightjar_core::wake_relay::driver::{
    self, Ask, Driver, Nat, Outcome, RelayStatus, Status, UdpLan, WakeRelayPeer,
};
use nightjar_core::wake_relay::saved;
use nightjar_desktop_config::config::HostRecord;
use nightjar_desktop_config::hosts_types::Waker;
use nightjar_desktop_config::strings::{tr, trf};
use nightjar_desktop_ui::ui::WakeRelayView;

pub(crate) use shared::{
    LeaveReport, LeaveStep, RelayProcessLock, ask, display_name, interfaces, leave_saved, paired,
    paired_in, try_acquire_process_lock,
};
#[cfg(test)]
pub(crate) use shared::{interface_of, left_already, on_this_network};

const POLL_TICK: Duration = Duration::from_millis(500);
const CONFLICT_RETRY: Duration = Duration::from_secs(2);

fn has_saved_assignments(dir: &Path) -> bool {
    saved::load(dir).is_ok_and(|s| {
        s.hosts
            .iter()
            .any(|h| h.assignment.is_some() || h.refused.is_some())
    })
}

fn wake_summary(status: Status) -> &'static str {
    match status {
        Status::PacketSent => tr("magic packets sent"),
        Status::WaitingForBoot => tr("waiting for the PC"),
        Status::HostAwake => tr("awake"),
        Status::TimedOut => tr("no answer in time"),
        Status::RateLimited => tr("rate limited"),
        Status::RejectedAuth => tr("rejected authorization"),
        Status::RejectedReplay => tr("rejected clock or replay"),
        Status::TargetNotOnLan => tr("not on this network"),
    }
}

trait RelayPeerHandle: Send {
    fn set_enrollments(&self, enrollments: Vec<nightjar_core::wake_relay::driver::Enrollment>);
    fn status(&self) -> RelayStatus;
    fn stop(&self);
}

impl RelayPeerHandle for WakeRelayPeer {
    fn set_enrollments(&self, enrollments: Vec<nightjar_core::wake_relay::driver::Enrollment>) {
        WakeRelayPeer::set_enrollments(self, enrollments);
    }
    fn status(&self) -> RelayStatus {
        WakeRelayPeer::status(self)
    }
    fn stop(&self) {
        WakeRelayPeer::stop(self);
    }
}

trait WorkerBackend: Send + Sync + 'static {
    fn try_lock(&self, dir: &Path) -> io::Result<Option<RelayProcessLock>>;
    fn has_lan(&self) -> bool;
    fn start_peer(&self) -> io::Result<Box<dyn RelayPeerHandle>>;
    fn load_saved(&self, dir: &Path) -> io::Result<saved::Saved>;
    fn save_saved(&self, dir: &Path, saved: &saved::Saved) -> io::Result<()>;
    fn paired_hosts(&self, dir: &Path, read_only: bool) -> Vec<HostRecord>;
    fn ask_host(&self, identity: &Arc<ClientIdentity>, record: &HostRecord, asking: Ask)
    -> Outcome;
    fn leave_hosts(
        &self,
        saved: &saved::Saved,
        identity: &Arc<ClientIdentity>,
        dir: &Path,
        read_only: bool,
    ) -> LeaveReport;
}

struct LiveBackend;

impl WorkerBackend for LiveBackend {
    fn try_lock(&self, dir: &Path) -> io::Result<Option<RelayProcessLock>> {
        try_acquire_process_lock(dir)
    }
    fn has_lan(&self) -> bool {
        !interfaces().is_empty()
    }
    fn start_peer(&self) -> io::Result<Box<dyn RelayPeerHandle>> {
        WakeRelayPeer::start(driver::relay_config(), Arc::new(UdpLan::new(interfaces)))
            .map(|peer| Box::new(peer) as Box<dyn RelayPeerHandle>)
    }
    fn load_saved(&self, dir: &Path) -> io::Result<saved::Saved> {
        saved::load(dir)
    }
    fn save_saved(&self, dir: &Path, saved: &saved::Saved) -> io::Result<()> {
        saved::save(dir, saved)
    }
    fn paired_hosts(&self, dir: &Path, read_only: bool) -> Vec<HostRecord> {
        paired_in(dir, read_only)
    }
    fn ask_host(
        &self,
        identity: &Arc<ClientIdentity>,
        record: &HostRecord,
        asking: Ask,
    ) -> Outcome {
        ask(identity, record, asking, None)
    }
    fn leave_hosts(
        &self,
        saved: &saved::Saved,
        identity: &Arc<ClientIdentity>,
        dir: &Path,
        read_only: bool,
    ) -> LeaveReport {
        leave_saved(saved, identity, dir, read_only)
    }
}

#[derive(Default)]
struct SharedState {
    enabled: bool,
    streaming: bool,
    shutdown: bool,
    identity: Option<Arc<ClientIdentity>>,
    reenroll_seq: u64,
    leave_seq: u64,
    status: WakeRelayView,
    leave_done: Option<Result<(), String>>,
}

/// Manages the background GUI wake relay thread when `settings.wake_relay` is on.
pub struct WakeRelayController {
    store_dir: PathBuf,
    read_only: bool,
    waker: Waker,
    shared: Arc<Mutex<SharedState>>,
    wake: Arc<(Mutex<()>, Condvar)>,
    stop_flag: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    backend: Arc<dyn WorkerBackend>,
    seen_reenroll: u64,
    seen_leave: u64,
}

impl WakeRelayController {
    pub fn new(store_dir: PathBuf, read_only: bool, waker: Waker) -> Self {
        Self::with_backend(store_dir, read_only, waker, Arc::new(LiveBackend))
    }

    fn with_backend(
        store_dir: PathBuf,
        read_only: bool,
        waker: Waker,
        backend: Arc<dyn WorkerBackend>,
    ) -> Self {
        let has_saved = has_saved_assignments(&store_dir);
        Self {
            store_dir,
            read_only,
            waker,
            shared: Arc::new(Mutex::new(SharedState {
                status: WakeRelayView {
                    has_saved,
                    ..WakeRelayView::default()
                },
                ..SharedState::default()
            })),
            wake: Arc::new((Mutex::new(()), Condvar::new())),
            stop_flag: Arc::new(AtomicBool::new(false)),
            worker: None,
            backend,
            seen_reenroll: 0,
            seen_leave: 0,
        }
    }

    /// Synchronizes desired enabled/streaming/identity state and manages worker lifecycle.
    pub fn sync(&mut self, enabled: bool, streaming: bool, identity: Option<Arc<ClientIdentity>>) {
        let should_run;
        {
            let mut st = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            let changed = st.enabled != enabled
                || st.streaming != streaming
                || st.identity.is_some() != identity.is_some();
            st.enabled = enabled;
            st.streaming = streaming;
            st.identity = identity;
            should_run =
                !st.shutdown && (st.enabled || st.leave_seq > self.seen_leave || st.status.busy);
            if !enabled && st.leave_seq == self.seen_leave && !st.status.busy {
                st.status.running = false;
                st.status.paused_for_stream = false;
                st.status.conflict = false;
                st.status.error = None;
                st.status.has_saved = has_saved_assignments(&self.store_dir);
            }
            if changed {
                self.notify_worker();
            }
        }
        if should_run && self.worker.as_ref().is_none_or(JoinHandle::is_finished) {
            if let Some(old) = self.worker.take() {
                let _ = old.join();
            }
            self.spawn_worker();
        }
    }

    pub fn reenroll(&mut self) {
        {
            let mut st = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            st.reenroll_seq += 1;
        }
        self.notify_worker();
    }

    pub fn request_leave(&mut self) {
        {
            let mut st = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            st.leave_seq += 1;
            st.status.busy = true;
            st.enabled = false;
        }
        self.notify_worker();
        if self.worker.as_ref().is_none_or(JoinHandle::is_finished) {
            if let Some(old) = self.worker.take() {
                let _ = old.join();
            }
            self.spawn_worker();
        }
    }

    pub fn take_leave_finished(&mut self) -> Option<Result<(), String>> {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .leave_done
            .take()
    }

    pub fn status(&self) -> WakeRelayView {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .status
            .clone()
    }

    pub fn stop_and_join(&mut self) {
        {
            let mut st = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            st.shutdown = true;
            st.enabled = false;
        }
        self.stop_flag.store(true, Ordering::Release);
        self.notify_worker();
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }

    fn notify_worker(&self) {
        let (lock, cvar) = &*self.wake;
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        cvar.notify_all();
    }

    fn spawn_worker(&mut self) {
        self.stop_flag.store(false, Ordering::Release);
        let shared = self.shared.clone();
        let wake = self.wake.clone();
        let dir = self.store_dir.clone();
        let read_only = self.read_only;
        let waker = self.waker.clone();
        let backend = self.backend.clone();
        let stop_flag = self.stop_flag.clone();
        let mut seen_reenroll = self.seen_reenroll;
        let mut seen_leave = self.seen_leave;
        match std::thread::Builder::new()
            .name("wake-relay-gui".into())
            .spawn(move || {
                run_worker_loop(
                    &dir,
                    read_only,
                    &shared,
                    &wake,
                    &waker,
                    &*backend,
                    &stop_flag,
                    &mut seen_reenroll,
                    &mut seen_leave,
                );
            }) {
            Ok(handle) => self.worker = Some(handle),
            Err(e) => {
                let mut st = self.shared.lock().unwrap_or_else(|e| e.into_inner());
                st.status.error = Some(e.to_string());
                st.status.busy = false;
                (self.waker)();
            }
        }
    }
}

impl Drop for WakeRelayController {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

struct ActiveRelay {
    driver: Driver,
    peer: Option<Box<dyn RelayPeerHandle>>,
    _lock: RelayProcessLock,
}

fn update_view(shared: &Mutex<SharedState>, waker: &Waker, next: WakeRelayView) {
    let changed = {
        let mut st = shared.lock().unwrap_or_else(|e| e.into_inner());
        let differs = st.status != next;
        st.status = next;
        differs
    };
    if changed {
        (waker)();
    }
}

fn wait_for(wake: &(Mutex<()>, Condvar), duration: Duration) {
    let (lock, cvar) = wake;
    let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    let _ = cvar.wait_timeout(guard, duration);
}

fn build_active_view(
    driver: &Driver,
    peer_status: &RelayStatus,
    paused_for_stream: bool,
    dir: &Path,
) -> WakeRelayView {
    let views = driver.hosts();
    let refused = views.iter().filter(|h| h.refused.is_some()).count();
    let error = peer_status
        .stun_error
        .as_ref()
        .or(peer_status.dht_error.as_ref())
        .cloned();
    let last_wake = peer_status
        .last_wake
        .as_ref()
        .map(|(pc, _, status)| format!("{pc}: {}", wake_summary(*status)));
    WakeRelayView {
        running: true,
        paused_for_stream,
        conflict: false,
        targets: peer_status.targets,
        devices: peer_status.devices,
        refused,
        last_wake,
        symmetric_nat: peer_status.nat == Nat::Symmetric,
        error,
        has_saved: has_saved_assignments(dir),
        busy: false,
    }
}

#[allow(clippy::too_many_arguments)]
fn run_worker_loop(
    dir: &Path,
    read_only: bool,
    shared: &Mutex<SharedState>,
    wake: &(Mutex<()>, Condvar),
    waker: &Waker,
    backend: &dyn WorkerBackend,
    stop_flag: &AtomicBool,
    seen_reenroll: &mut u64,
    seen_leave: &mut u64,
) {
    let mut active: Option<ActiveRelay> = None;
    let mut last_peer_status = RelayStatus::default();
    loop {
        let (enabled, streaming, shutdown, identity, reenroll_seq, leave_seq) = {
            let st = shared.lock().unwrap_or_else(|e| e.into_inner());
            (
                st.enabled,
                st.streaming,
                st.shutdown,
                st.identity.clone(),
                st.reenroll_seq,
                st.leave_seq,
            )
        };
        if shutdown || stop_flag.load(Ordering::Acquire) {
            break;
        }
        if leave_seq > *seen_leave {
            *seen_leave = leave_seq;
            if let Some(mut running) = active.take() {
                if let Some(peer) = running.peer.take() {
                    peer.stop();
                }
                if running.driver.take_changed() {
                    let _ = backend.save_saved(dir, &running.driver.saved());
                }
            }
            let outcome = perform_leave(dir, read_only, identity.as_ref(), backend);
            {
                let mut st = shared.lock().unwrap_or_else(|e| e.into_inner());
                st.leave_done = Some(outcome.clone());
                st.status = WakeRelayView {
                    has_saved: has_saved_assignments(dir),
                    error: outcome.err(),
                    ..WakeRelayView::default()
                };
            }
            (waker)();
            break;
        }
        if !enabled {
            break;
        }
        if reenroll_seq > *seen_reenroll {
            *seen_reenroll = reenroll_seq;
            if let Some(mut old) = active.take() {
                if let Some(peer) = old.peer.take() {
                    peer.stop();
                }
                if old.driver.take_changed() {
                    let _ = backend.save_saved(dir, &old.driver.saved());
                }
            }
            let _ = start_active(dir, backend, true, &mut active, shared, waker);
        }
        if active.is_none() && !streaming {
            if !start_active(dir, backend, false, &mut active, shared, waker) {
                wait_for(wake, CONFLICT_RETRY);
                continue;
            }
        }
        if let Some(relay) = active.as_mut() {
            if streaming {
                if let Some(peer) = relay.peer.take() {
                    last_peer_status = peer.status();
                    peer.stop();
                }
                let next = build_active_view(&relay.driver, &last_peer_status, true, dir);
                update_view(shared, waker, next);
                wait_for(wake, POLL_TICK);
                continue;
            }
            if relay.peer.is_none() {
                if !backend.has_lan() {
                    let next = WakeRelayView {
                        running: true,
                        error: Some(
                            tr("This computer has no active IPv4 network for magic packets.")
                                .to_owned(),
                        ),
                        has_saved: has_saved_assignments(dir),
                        ..WakeRelayView::default()
                    };
                    update_view(shared, waker, next);
                    wait_for(wake, CONFLICT_RETRY);
                    continue;
                }
                match backend.start_peer() {
                    Ok(peer) => {
                        peer.set_enrollments(relay.driver.enrollments());
                        relay.peer = Some(peer);
                    }
                    Err(e) => {
                        let next = WakeRelayView {
                            running: true,
                            error: Some(trf("Could not start wake relay socket: {}", &[&e])),
                            has_saved: has_saved_assignments(dir),
                            ..WakeRelayView::default()
                        };
                        update_view(shared, waker, next);
                        wait_for(wake, CONFLICT_RETRY);
                        continue;
                    }
                }
            }
            if let Some(identity) = identity.as_ref()
                && !relay.driver.due(driver::unix_now()).is_empty()
            {
                let records = backend.paired_hosts(dir, read_only);
                let listed: Vec<(String, String)> = records
                    .iter()
                    .map(|r| (r.unique_id.clone(), display_name(r)))
                    .collect();
                let _ = relay.driver.set_hosts(&listed);
                for (id, asking) in relay.driver.due(driver::unix_now()) {
                    let st = shared.lock().unwrap_or_else(|e| e.into_inner());
                    if !st.enabled || st.streaming || st.shutdown || st.leave_seq > *seen_leave {
                        break;
                    }
                    drop(st);
                    let Some(record) = records.iter().find(|r| r.unique_id == id) else {
                        continue;
                    };
                    let outcome = backend.ask_host(identity, record, asking);
                    let _ = relay.driver.on_result(&id, outcome, driver::unix_now());
                }
            }
            if relay.driver.take_changed() {
                if let Some(peer) = relay.peer.as_ref() {
                    peer.set_enrollments(relay.driver.enrollments());
                }
                if let Err(e) = backend.save_saved(dir, &relay.driver.saved()) {
                    eprintln!(
                        "nightjar: wake-relay: {} cannot be saved: {e}",
                        saved::FILE_NAME
                    );
                }
            }
            if let Some(peer) = relay.peer.as_ref() {
                last_peer_status = peer.status();
            }
            let next = build_active_view(&relay.driver, &last_peer_status, false, dir);
            update_view(shared, waker, next);
        }
        wait_for(wake, POLL_TICK);
    }
    if let Some(mut relay) = active.take() {
        if let Some(peer) = relay.peer.take() {
            peer.stop();
        }
        if relay.driver.take_changed() {
            let _ = backend.save_saved(dir, &relay.driver.saved());
        }
    }
    let final_view = WakeRelayView {
        has_saved: has_saved_assignments(dir),
        ..WakeRelayView::default()
    };
    update_view(shared, waker, final_view);
}

fn start_active(
    dir: &Path,
    backend: &dyn WorkerBackend,
    enroll_again: bool,
    active: &mut Option<ActiveRelay>,
    shared: &Mutex<SharedState>,
    waker: &Waker,
) -> bool {
    let lock = match backend.try_lock(dir) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            update_view(
                shared,
                waker,
                WakeRelayView {
                    conflict: true,
                    has_saved: has_saved_assignments(dir),
                    ..WakeRelayView::default()
                },
            );
            return false;
        }
        Err(e) => {
            update_view(
                shared,
                waker,
                WakeRelayView {
                    error: Some(e.to_string()),
                    has_saved: has_saved_assignments(dir),
                    ..WakeRelayView::default()
                },
            );
            return false;
        }
    };
    let saved = match backend.load_saved(dir) {
        Ok(saved) => saved,
        Err(e) => {
            update_view(
                shared,
                waker,
                WakeRelayView {
                    error: Some(e.to_string()),
                    has_saved: true,
                    ..WakeRelayView::default()
                },
            );
            return false;
        }
    };
    let driver = Driver::new(saved, enroll_again);
    *active = Some(ActiveRelay {
        driver,
        peer: None,
        _lock: lock,
    });
    true
}

fn perform_leave(
    dir: &Path,
    read_only: bool,
    identity: Option<&Arc<ClientIdentity>>,
    backend: &dyn WorkerBackend,
) -> Result<(), String> {
    let _lock = match backend.try_lock(dir) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            return Err(tr(
                "Another Nightjar wake relay is running. Stop nightjar wake-relay first.",
            )
            .to_owned());
        }
        Err(e) => return Err(e.to_string()),
    };
    let saved = backend.load_saved(dir).map_err(|e| e.to_string())?;
    let Some(identity) = identity else {
        let _ = saved::remove(dir);
        return Ok(());
    };
    let report = backend.leave_hosts(&saved, identity, dir, read_only);
    if let Some(err) = report.remove_error {
        return Err(err);
    }
    if report.had_failures {
        let details: Vec<String> = report
            .steps
            .into_iter()
            .filter_map(|step| match step {
                LeaveStep::NotPaired { name } => Some(trf(
                    "{}: not paired anymore; remove it in the PC’s console",
                    &[&name],
                )),
                LeaveStep::Failed { name, why } => Some(format!("{name}: {why}")),
                _ => None,
            })
            .collect();
        if !details.is_empty() {
            return Err(details.join("\n"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
