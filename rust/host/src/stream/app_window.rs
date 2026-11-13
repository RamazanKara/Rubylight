//! A stream of the launched app's window in place of its display
//! (`capture_window = window`, butterpollo_core::app_window): this worker
//! finds the window among the app's process tree's windows, lays its capture
//! target over the display's ([`Ready::show_app_window`]) and says on the
//! device what the stream shows while it shows the display instead. The
//! capture reopens on each change of target, and absolute input follows the
//! target (the stream's output) onto the window.
//!
//! It starts before the stream's first capture and waits a moment for the
//! window ([`FIRST_WAIT`]), so the capture and the encoder open on the window
//! instead of opening on the display and being made again for the window a
//! moment later.
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
/// How long the stream's start waits for the app's window, which a launch may
/// still be creating, before it opens on the display (the worker switches to
/// the window once it appears). The device waits for its first frame anyway.
pub(super) const FIRST_WAIT: Duration = Duration::from_millis(1500);
/// How often the window is looked for during [`FIRST_WAIT`], reading the app's
/// process tree each time: its process may start the window's process late.
const FIRST_CHECK: Duration = Duration::from_millis(100);
const NO_PROCESSES: &str = "This app starts no program the host can follow (the desktop, or a detached command), so its window cannot be found and the stream shows the display. Name the program in the app's window settings.";

pub(super) struct AppWindow {
    stop: Arc<AtomicBool>,
    prepared: Arc<Ready>,
    worker: Option<thread::JoinHandle<()>>,
}
impl AppWindow {
    /// Looks for the window for up to `first_wait` before returning, so the
    /// stream's first capture opens on it, then follows it on its own thread.
    pub fn start(
        h: Shared,
        s: Arc<Session>,
        prepared: Arc<Ready>,
        settings: Settings,
        first_wait: Duration,
    ) -> std::io::Result<Self> {
        tracing::info!(
            client = %s.launch.client.name,
            exe = ?settings.exe,
            title = ?settings.title,
            closed = ?settings.closed,
            "streaming the app's window in place of the display"
        );
        let stop = Arc::new(AtomicBool::new(false));
        let mut watch = Watch::new(&settings);
        let started = Instant::now();
        let deadline = started + first_wait;
        let ended = loop {
            let step = watch.check(&h, &s, &prepared, &settings, FIRST_CHECK);
            if !step.waiting || step.ended || Instant::now() >= deadline || stopping(&h, &s) {
                break step.ended;
            }
            thread::sleep(FIRST_CHECK.min(deadline.saturating_duration_since(Instant::now())));
        };
        match &watch.shown {
            Some(target) => tracing::info!(
                client = %s.launch.client.name,
                %target,
                waited_ms = started.elapsed().as_millis() as u64,
                "the stream's first capture opens on the app's window"
            ),
            None => tracing::info!(
                client = %s.launch.client.name,
                waited_ms = started.elapsed().as_millis() as u64,
                "the stream starts on the display; it switches to the app's window when it shows"
            ),
        }
        let worker = if ended {
            None
        } else {
            Some(thread::Builder::new().name("app window".into()).spawn({
                let (stop, prepared) = (stop.clone(), prepared.clone());
                move || run(&h, &s, &prepared, &settings, &stop, watch)
            })?)
        };
        Ok(Self {
            stop,
            prepared,
            worker,
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

fn stopping(h: &Shared, s: &Session) -> bool {
    s.stopping() || h.stop.load(Ordering::Acquire)
}

/// What one look for the window found.
struct Step {
    /// The app has not shown a window yet but may: its processes can be
    /// followed (or its program is named). The stream's start waits on this.
    waiting: bool,
    /// The window closed and the stream ended (`capture_window_closed = end`).
    ended: bool,
}

/// The worker's state between looks for the window.
struct Watch {
    named: bool,
    follow: Follow,
    tree: Option<BTreeSet<u32>>,
    names: BTreeMap<u32, String>,
    tree_read: Option<Instant>,
    /// The window target the stream shows; None for the display.
    shown: Option<String>,
}
impl Watch {
    fn new(settings: &Settings) -> Self {
        Self {
            named: settings.exe.is_some(),
            follow: Follow::default(),
            tree: None,
            names: BTreeMap::new(),
            tree_read: None,
            shown: None,
        }
    }

    /// Looks for the window once, reading the app's process tree when it is
    /// `refresh` old, and lays its target over the display's.
    fn check(
        &mut self,
        h: &Shared,
        s: &Session,
        prepared: &Ready,
        settings: &Settings,
        refresh: Duration,
    ) -> Step {
        let now = Instant::now();
        if self
            .tree_read
            .is_none_or(|at| now.duration_since(at) >= refresh)
        {
            (self.tree, self.names) = app_processes(h, s, self.named);
            self.tree_read = Some(now);
        }
        let mut windows = match butterpollo_windows::capture::app_windows() {
            Ok(windows) => windows,
            Err(error) => {
                tracing::debug!(%error, "the desktop's windows are unavailable");
                Vec::new()
            }
        };
        if self.named {
            for window in &mut windows {
                if let Some(name) = self.names.get(&window.pid) {
                    window.exe.clone_from(name);
                }
            }
        }
        let empty = BTreeSet::new();
        let picked = app_window::pick(
            &windows,
            self.tree.as_ref().unwrap_or(&empty),
            settings,
            self.follow.current(),
        );
        let decision = self.follow.step(picked, settings.closed, now);
        let target = match &decision.target {
            Target::Window { id, pid } => {
                match butterpollo_windows::capture::app_window_target(*id, *pid) {
                    Ok(target) => Some(target),
                    // Closed between the scan and now: the next check sees it.
                    Err(error) => {
                        tracing::debug!(error = %format!("{error:#}"), "the app's window left before its capture target was made");
                        self.shown.clone()
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
        let untraceable = self.tree.is_none() && !self.named;
        self.shown = target;
        report(s, &decision, untraceable);
        let ended = match decision.end {
            Some(reason) => {
                tracing::info!(client = %s.launch.client.name, "the streamed app window closed; ending the stream");
                s.fail(reason);
                true
            }
            None => false,
        };
        Step {
            waiting: first_wait_continues(&decision, untraceable),
            ended,
        }
    }
}

/// Whether the stream's start keeps waiting for the window after `decision`:
/// only while the app has shown none yet and its window can be found. A
/// minimised window, or an app with no processes to follow, opens on the
/// display at once.
fn first_wait_continues(decision: &Decision, untraceable: bool) -> bool {
    decision.target == Target::Display
        && decision.warning == Some(app_window::WAITING)
        && !untraceable
}

fn run(
    h: &Shared,
    s: &Session,
    prepared: &Ready,
    settings: &Settings,
    stop: &AtomicBool,
    mut watch: Watch,
) {
    while !stop.load(Ordering::Acquire) && !stopping(h, s) {
        thread::park_timeout(CHECK);
        if stop.load(Ordering::Acquire) || stopping(h, s) {
            return;
        }
        if watch.check(h, s, prepared, settings, TREE).ended {
            return;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn decision(target: Target, warning: Option<&'static str>) -> Decision {
        Decision {
            target,
            warning,
            end: None,
        }
    }

    #[test]
    fn the_start_waits_only_for_a_window_that_can_still_show() {
        let waiting = decision(Target::Display, Some(app_window::WAITING));
        assert!(first_wait_continues(&waiting, false));
        assert!(
            !first_wait_continues(&waiting, true),
            "an app with no processes to follow opens on the display at once"
        );
        let minimised = decision(Target::Display, Some(app_window::MINIMIZED));
        assert!(!first_wait_continues(&minimised, false));
        let window = decision(Target::Window { id: 7, pid: 9 }, None);
        assert!(!first_wait_continues(&window, false), "found: open on it");
    }

    #[test]
    fn the_first_wait_is_short_against_the_first_frame_deadline() {
        // `start_capture` waits up to 10 s for its first frame; the window wait
        // must leave nearly all of it.
        assert!(FIRST_WAIT <= Duration::from_secs(2));
        assert!(FIRST_CHECK < FIRST_WAIT);
    }
}
