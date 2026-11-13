//! A stream of the launched app's window in place of its display
//! (`capture_window = window`, butterpollo_core::app_window): this worker
//! finds the window among the app's process tree's windows, lays its capture
//! target over the display's ([`Ready::show_app_window`]) and says on the
//! device what the stream shows while it shows the display instead. The
//! capture reopens on each change of target, and absolute input follows the
//! target (the stream's output) onto the window.
use crate::display_session::Ready;
use crate::state::{Session, Shared};
use butterpollo_core::app_window::{self, Decision, Follow, Settings, Target};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const WARNING: &str = "capture_app_window";
/// How often the window is looked for: minimising, restoring and closing
/// show within this.
const CHECK: Duration = Duration::from_millis(250);
/// How often the app's process tree is read again: it takes a process list.
const TREE: Duration = Duration::from_secs(1);
const NO_PROCESSES: &str = "This app starts no program the host can follow (the desktop, or a detached command), so its window cannot be found and the stream shows the display. Name the program in the app's window settings.";

pub(super) struct AppWindow {
    stop: Arc<AtomicBool>,
    prepared: Arc<Ready>,
    worker: Option<thread::JoinHandle<()>>,
}
impl AppWindow {
    pub fn start(
        h: Shared,
        s: Arc<Session>,
        prepared: Arc<Ready>,
        settings: Settings,
    ) -> std::io::Result<Self> {
        tracing::info!(
            client = %s.launch.client.name,
            exe = ?settings.exe,
            title = ?settings.title,
            closed = ?settings.closed,
            "streaming the app's window in place of the display"
        );
        let stop = Arc::new(AtomicBool::new(false));
        let worker = thread::Builder::new().name("app window".into()).spawn({
            let (stop, prepared) = (stop.clone(), prepared.clone());
            move || run(&h, &s, &prepared, &settings, &stop)
        })?;
        Ok(Self {
            stop,
            prepared,
            worker: Some(worker),
        })
    }
}
impl Drop for AppWindow {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            super::finish_worker(worker, "app window");
        }
        // A display kept for the device's reconnect shows the display; the
        // next stream finds the window again.
        self.prepared.show_app_window(None);
    }
}

/// The app's processes and, when a program is named, every process's
/// program name; None when the app has no processes to follow.
fn app_processes(
    h: &Shared,
    s: &Session,
    named: bool,
) -> (Option<BTreeSet<u32>>, BTreeMap<u32, String>) {
    let tree = h
        .current_app
        .lock()
        .unwrap()
        .as_ref()
        .filter(|app| app.id == s.launch.app_id)
        .map(crate::process::RunningApp::window_processes);
    let tree = match tree {
        Some(Ok(tree)) => Some(tree),
        Some(Err(error)) => {
            tracing::debug!(error = %format!("{error:#}"), "the app's processes are unavailable for its window");
            None
        }
        None => None,
    };
    let names = if named {
        butterpollo_windows::process::processes()
            .map(|all| all.into_iter().map(|p| (p.pid, p.name)).collect())
            .unwrap_or_default()
    } else {
        BTreeMap::new()
    };
    (tree, names)
}

fn run(h: &Shared, s: &Session, prepared: &Ready, settings: &Settings, stop: &AtomicBool) {
    let named = settings.exe.is_some();
    let mut follow = Follow::default();
    let (mut tree, mut names) = (None, BTreeMap::new());
    let mut tree_read: Option<Instant> = None;
    let mut shown: Option<String> = None;
    while !stop.load(Ordering::Acquire) && !s.stopping() && !h.stop.load(Ordering::Acquire) {
        let now = Instant::now();
        if tree_read.is_none_or(|at| now.duration_since(at) >= TREE) {
            (tree, names) = app_processes(h, s, named);
            tree_read = Some(now);
        }
        let mut windows = match butterpollo_windows::capture::app_windows() {
            Ok(windows) => windows,
            Err(error) => {
                tracing::debug!(%error, "the desktop's windows are unavailable");
                Vec::new()
            }
        };
        if named {
            for window in &mut windows {
                if let Some(name) = names.get(&window.pid) {
                    window.exe.clone_from(name);
                }
            }
        }
        let empty = BTreeSet::new();
        let picked = app_window::pick(
            &windows,
            tree.as_ref().unwrap_or(&empty),
            settings,
            follow.current(),
        );
        let decision = follow.step(picked, settings.closed, now);
        let target = match &decision.target {
            Target::Window { id, pid } => {
                match butterpollo_windows::capture::app_window_target(*id, *pid) {
                    Ok(target) => Some(target),
                    // Closed between the scan and now: the next check sees it.
                    Err(error) => {
                        tracing::debug!(error = %format!("{error:#}"), "the app's window left before its capture target was made");
                        shown.clone()
                    }
                }
            }
            Target::Display => None,
        };
        if prepared.show_app_window(target.clone()) {
            match (&target, picked) {
                (Some(target), Some(window)) => {
                    tracing::info!(%target, title = %window.title, "the stream shows the app's window")
                }
                _ => tracing::info!(
                    reason = decision.warning.unwrap_or("the window closed"),
                    "the stream shows the display in place of the app's window"
                ),
            }
        }
        shown = target;
        report(s, &decision, tree.is_none() && !named);
        if let Some(reason) = decision.end {
            tracing::info!(client = %s.launch.client.name, "the streamed app window closed; ending the stream");
            s.fail(reason);
            return;
        }
        thread::park_timeout(CHECK);
    }
}

/// What the device is told: the decision's warning, or why no window can be
/// found for an app with no processes to follow.
fn report(s: &Session, decision: &Decision, untraceable: bool) {
    match decision.warning {
        Some(_) if untraceable && decision.target == Target::Display => {
            s.launch.warnings.set(WARNING, NO_PROCESSES)
        }
        Some(warning) => s.launch.warnings.set(WARNING, warning),
        None => s.launch.warnings.clear(WARNING),
    }
}
