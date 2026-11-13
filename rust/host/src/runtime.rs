//! Host housekeeping and user actions, independent of listener setup.
use crate::{
    maintenance,
    state::{AppStop, Shared},
};
use butterpollo_core::{native_api::session::State, session::Role};
use butterpollo_windows::{process::StopSignal, tray::Action};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Receiver,
    },
    time::{Duration, Instant},
};
use tokio::sync::Notify;

/// Whether a client streams the running app.
fn streaming(h: &Shared) -> bool {
    h.sessions
        .lock()
        .unwrap()
        .active
        .values()
        .any(|session| session.launch.role == Role::Stream && !session.stopping())
}
fn application_finished(h: &Shared) -> bool {
    application_finished_with(
        h,
        butterpollo_windows::process::processes,
        butterpollo_windows::process::image_path,
    )
}
/// The process list a Steam game is looked for in, and the program paths of
/// its new processes, are read with `current_app` released (the stream and
/// input threads look at the app every second) and applied only to the app
/// they were read for.
fn application_finished_with(
    h: &Shared,
    processes: impl FnOnce() -> anyhow::Result<Vec<butterpollo_core::steam::Process>>,
    image_path: impl Fn(u32) -> Option<String>,
) -> bool {
    // Between a press of Continue here and its stream a device is on its way to the game, and
    // after a failed press the device it was taken from may still come back.
    let streaming = streaming(h);
    // Asked even with a stream: that one closes the window of the device that came back.
    let awaited = h.game_awaited(streaming);
    let connected = streaming || awaited;
    let scan_for = {
        let mut current = h.current_app.lock().unwrap();
        let Some(app) = current.as_mut() else {
            return false;
        };
        if app.connection_state(connected) {
            return true;
        }
        app.wants_processes().then(|| app.generation.clone())
    };
    let scan = match scan_for
        .as_deref()
        .map(|generation| (generation, processes()))
    {
        Some((generation, Err(error))) => {
            tracing::warn!(%error, "application exit check failed");
            // A failed read waits its second like a look, not retried every tick.
            if let Some(app) = h
                .current_app
                .lock()
                .unwrap()
                .as_mut()
                .filter(|app| app.generation == generation)
            {
                app.scan_failed();
            }
            return false;
        }
        Some((generation, Ok(processes))) => {
            let unlooked = h
                .current_app
                .lock()
                .unwrap()
                .as_ref()
                .filter(|app| app.generation == generation)
                .map(|app| app.unlooked(&processes))
                .unwrap_or_default();
            let paths = unlooked
                .into_iter()
                .map(|pid| (pid, image_path(pid)))
                .collect();
            Some(crate::process::ProcessScan { processes, paths })
        }
        None => None,
    };
    h.current_app.lock().unwrap().as_mut().is_some_and(|app| {
        // An app that replaced the one scanned for is looked at with its own list next time.
        let scan = scan
            .as_ref()
            .filter(|_| scan_for.as_deref() == Some(app.generation.as_str()));
        match app.exited(scan) {
            Ok(finished) => finished,
            Err(error) => {
                tracing::warn!(%error, "application exit check failed");
                false
            }
        }
    })
}
/// Start an app that was launched before anyone signed in, keeping its
/// session identity. It replaces the placeholder only if that still runs.
fn start_deferred(h: &Shared) {
    let pending = h.current_app.lock().unwrap().as_mut().and_then(|app| {
        app.take_ready_deferred()
            .map(|(app_config, args)| (app_config, args, app.generation.clone(), app.owner.clone()))
    });
    let Some((app, args, generation, owner)) = pending else {
        return;
    };
    let h = h.clone();
    tokio::task::spawn_blocking(move || {
        let _transition = h.launch_transition.lock().unwrap();
        if !h
            .current_app
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|app| app.generation == generation)
        {
            return;
        }
        tracing::info!(app = %app.name, "user signed in; starting the deferred application");
        match crate::process::launch(&h, &app, &args) {
            Ok(mut started) => {
                let mut current = h.current_app.lock().unwrap();
                if let Some(placeholder) = current
                    .as_mut()
                    .filter(|placeholder| placeholder.generation == generation)
                {
                    started.take_place_of(placeholder, owner);
                    let event = started.started_event();
                    *current = Some(started);
                    drop(current);
                    h.app_started_webhook(event);
                }
            }
            Err(error) => {
                tracing::warn!(%error, app = %app.name, "deferred application failed to start");
                let canceled = h.sessions.lock().unwrap().stop_role(Role::Stream, None);
                drop(canceled);
                h.stop_app(AppStop::LaunchFailed);
            }
        }
    });
}
fn user_action(h: &Shared, action: Action, web_port: u16) {
    match action {
        Action::Open => {
            if let Err(error) = butterpollo_windows::tray::open_web(web_port) {
                tracing::warn!(%error, "could not open the administration console");
            }
        }
        Action::EnterPin | Action::PairDevice | Action::RestoreSettings => {
            let page = match action {
                Action::EnterPin => butterpollo_windows::tray::PAGE_ENTER_PIN,
                Action::RestoreSettings => butterpollo_windows::tray::PAGE_BACKUP,
                _ => butterpollo_windows::tray::PAGE_PAIR_QR,
            };
            if let Err(error) = butterpollo_windows::tray::open_web_page(web_port, page) {
                tracing::warn!(%error, page, "could not open the administration console");
            }
        }
        Action::StopSessions => {
            let h = h.clone();
            tokio::task::spawn_blocking(move || {
                let _transition = h.launch_transition.lock().unwrap();
                release_displays(&h);
            });
        }
        Action::QuitApp => {
            tracing::info!("quitting the running app from the tray");
            let h = h.clone();
            // Stopping the app waits for it to exit.
            tokio::task::spawn_blocking(move || {
                let _transition = h.launch_transition.lock().unwrap();
                let canceled = h.sessions.lock().unwrap().stop_role(Role::Stream, None);
                drop(canceled);
                h.stop_app(AppStop::Tray);
            });
        }
        Action::CheckUpdates => maintenance::trigger_update(h, true),
        Action::BackupSettings => {
            let h = h.clone();
            tokio::task::spawn_blocking(move || {
                match crate::settings_backup::export_from_tray(&h) {
                    Ok(path) => {
                        tracing::info!(path = %path.display(), "settings backed up from the tray");
                        butterpollo_windows::tray::notify(
                            "Settings backed up",
                            &format!(
                                "Saved to {}. Pairing identity and sign-in are not in it.",
                                path.display()
                            ),
                        );
                    }
                    Err(error) => {
                        tracing::warn!(error = %format!("{error:#}"), "the tray settings backup failed");
                        butterpollo_windows::tray::notify(
                            "Settings not backed up",
                            &format!("{error:#}"),
                        );
                    }
                }
            });
        }
        Action::Restart => {
            h.restart.store(true, Ordering::Release);
            h.stop.store(true, Ordering::Release);
        }
        Action::Quit => h.stop.store(true, Ordering::Release),
    }
}
/// Keep routine app status in the tray instead of covering the captured
/// desktop with launch, pause and resume notifications.
fn show_app(h: &Shared, tray: &mut butterpollo_core::tray::Tracker) {
    let app = h
        .current_app
        .lock()
        .unwrap()
        .as_ref()
        .map(|app| app.name.clone());
    let current = butterpollo_core::tray::Status {
        app,
        streaming: streaming(h),
    };
    let Some(_) = tray.update(current) else {
        return;
    };
    butterpollo_windows::tray::show_app(tray.icon(), tray.app(), tray.summary().as_deref());
}
/// End every stream and remote monitor and release the displays they held,
/// which removes the virtual displays and restores the layout.
pub fn release_displays(h: &Shared) {
    let teardown = h.session_teardown(crate::session_control::Scope::Session);
    let canceled = h.sessions.lock().unwrap().request_stop(None);
    drop(canceled);
    crate::remote_display::disconnect(h, None);
    let audio = h.app_audio.lock().unwrap().take();
    let displays = std::mem::take(&mut *h.app_display.lock().unwrap());
    drop((audio, displays));
    drop(teardown);
}
/// The registered restore hotkey, following the setting.
#[derive(Default)]
struct RestoreHotkey {
    wanted: Option<(u32, u32)>,
    registered: Option<butterpollo_windows::hotkey::Hotkey>,
    presses: Option<std::sync::mpsc::Receiver<()>>,
    checked: Option<Instant>,
}
impl RestoreHotkey {
    fn poll(&mut self, h: &Shared) {
        if self
            .presses
            .as_ref()
            .is_some_and(|presses| presses.try_iter().count() > 0)
        {
            tracing::info!("restore hotkey pressed; ending streams and restoring the displays");
            let h = h.clone();
            // Restoring the layout waits on Windows.
            tokio::task::spawn_blocking(move || release_displays(&h));
        }
        if self
            .checked
            .is_some_and(|at| at.elapsed() < Duration::from_secs(2))
        {
            return;
        }
        self.checked = Some(Instant::now());
        let wanted = butterpollo_core::hotkey::restore_hotkey(&h.config.read().unwrap());
        if wanted == self.wanted {
            return;
        }
        self.wanted = wanted;
        self.registered = None;
        self.presses = None;
        let Some((key, modifiers)) = wanted else {
            return;
        };
        let (pressed, presses) = std::sync::mpsc::channel();
        // A press wakes an idle loop.
        let wake = h.maintain_wake().clone();
        match butterpollo_windows::hotkey::Hotkey::register(key, modifiers, move || {
            let _ = pressed.send(());
            wake.notify_one();
        }) {
            Ok(hotkey) => {
                tracing::debug!(key, modifiers, "registered the display restore hotkey");
                self.registered = Some(hotkey);
                self.presses = Some(presses);
            }
            Err(error) => {
                tracing::warn!(error = %format!("{error:#}"), key, modifiers, "the display restore hotkey could not be registered");
            }
        }
    }
}
/// Follow the session between launches and teardowns, off the async thread:
/// it takes the session lock, which an action holds while it waits for the
/// host's other locks. A tick still in flight is not joined by another, so a
/// slow disk or lock cannot pile up blocking tasks.
fn spawn_session_tick(h: &Shared, ticking: &Arc<AtomicBool>) -> bool {
    let h = h.clone();
    spawn_exclusive(ticking, move || h.session_tick())
}
/// The Steam, Playnite and launcher sync checks, one at a time: an older discovery reconciling last
/// would remove an app a newer one just added, and a hung drive must not park a thread per tick.
fn spawn_library_tick(h: &Shared, running: &Arc<AtomicBool>) -> bool {
    let h = h.clone();
    spawn_exclusive(running, move || {
        // Where Steam is takes registry reads and the signed-in user's environment: both checks
        // ask, so the answer is looked up once per tick, and only when one of them needs it.
        let steam = crate::steam::Roots::new(butterpollo_windows::steam::roots);
        crate::steam::watch(&h, &steam);
        crate::playnite::watch(&h);
        crate::library::watch(&h, &steam);
    })
}
/// How long a library tick may run before it is told: while one hangs the Steam, Playnite and
/// launcher checks all wait for it.
const LIBRARY_TICK_TOO_LONG: Duration = Duration::from_secs(5 * 60);
/// The loop's tick while the host has work: it follows sessions, the running app, kept displays
/// and a queued update this often.
const BUSY_TICK: Duration = Duration::from_millis(100);
/// The longest an idle host sleeps: the update check, due by the wall clock, looks every second.
const IDLE_TICK: Duration = Duration::from_secs(1);
/// Whether nothing the loop follows every [`BUSY_TICK`] goes on: no launch, stream, teardown or
/// remote monitor, no app, app audio or kept display, a session machine that rests idle, and no
/// update queued. Then only the loop's timers and `Host::wake_maintenance` have work for it. A
/// session machine still settling, or locked by an action, is followed every tick as before.
fn idle(h: &Shared) -> bool {
    !h.sessions.lock().unwrap().owns_capture()
        && h.current_app.lock().unwrap().is_none()
        && h.app_audio.lock().unwrap().is_none()
        && h.app_display.lock().unwrap().is_empty()
        && h.monitors.lock().unwrap().is_empty()
        && !matches!(
            h.updates.lock().unwrap()["phase"].as_str(),
            Some("waiting" | "downloading" | "ready" | "installing")
        )
        && h.try_session_state() == Some(State::Idle)
}
/// When an idle loop runs next, unless woken: at its first timer, no later than [`IDLE_TICK`] and
/// no sooner than [`BUSY_TICK`] from `now`.
fn idle_until(now: Instant, timers: impl IntoIterator<Item = Instant>) -> Instant {
    timers
        .into_iter()
        .fold(now + IDLE_TICK, Instant::min)
        .max(now + BUSY_TICK)
}
/// Hands each tray action on, and wakes the idle loop for it, from a thread that ends with the
/// tray.
fn forward_actions(actions: Receiver<Action>, wake: Arc<Notify>) -> Receiver<Action> {
    let (forward, forwarded) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for action in actions {
            if forward.send(action).is_err() {
                return;
            }
            wake.notify_one();
        }
    });
    forwarded
}
/// Wakes the idle loop when the service asks the host to stop. The thread waits as long as the
/// host runs.
fn wake_on_stop(stop_signal: &Arc<StopSignal>, wake: Arc<Notify>) {
    if !stop_signal.supervised() {
        return;
    }
    let stop_signal = stop_signal.clone();
    std::thread::spawn(move || {
        stop_signal.wait();
        wake.notify_one();
    });
}
/// Since when a library tick has been in flight, for one warning about a hung one.
#[derive(Default)]
struct LibraryTickWatch {
    since: Option<Instant>,
    told: bool,
}
impl LibraryTickWatch {
    /// Notes whether a tick just started; how long it has run, once, when that is too long.
    fn note(&mut self, started: bool, now: Instant) -> Option<Duration> {
        if started {
            self.since = Some(now);
            self.told = false;
            return None;
        }
        let running = now.duration_since(self.since?);
        if running < LIBRARY_TICK_TOO_LONG || self.told {
            return None;
        }
        self.told = true;
        Some(running)
    }
}
/// Run `work` on the blocking pool unless the previous run behind `running`
/// has not finished. Returns whether it started.
fn spawn_exclusive(running: &Arc<AtomicBool>, work: impl FnOnce() + Send + 'static) -> bool {
    if running.swap(true, Ordering::AcqRel) {
        return false;
    }
    /// Clears the flag also when `work` panics.
    struct Done(Arc<AtomicBool>);
    impl Drop for Done {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
    let done = Done(running.clone());
    tokio::task::spawn_blocking(move || {
        let _done = done;
        work();
    });
    true
}
pub async fn maintain(
    h: Shared,
    stop_signal: StopSignal,
    actions: Option<Receiver<Action>>,
    web_port: u16,
) {
    let mut update_at = Instant::now();
    let mut steam_at = Instant::now() + Duration::from_secs(5);
    let mut launcher_at = Instant::now();
    let launcher_reading = Arc::new(AtomicBool::new(false));
    let mut wake_at = Instant::now();
    let waking = Arc::new(AtomicBool::new(false));
    let library_ticking = Arc::new(AtomicBool::new(false));
    let mut library_tick_watch = LibraryTickWatch::default();
    let mut hotkey = RestoreHotkey::default();
    let mut tray = butterpollo_core::tray::Tracker::default();
    let mut auto_lock = crate::auto_lock::Watch::default();
    let mut housekeeping: Option<tokio::task::JoinHandle<()>> = None;
    let ticking = Arc::new(AtomicBool::new(false));
    // An idle loop sleeps until its next timer. A launch or stream (`Sessions::with_waker`), an
    // app started from the console, a queued update, a quit or restart, a tray action, the
    // restore hotkey and the service's stop wake it sooner.
    let wake = h.maintain_wake().clone();
    let stop_signal = Arc::new(stop_signal);
    wake_on_stop(&stop_signal, wake.clone());
    let actions = actions.map(|actions| forward_actions(actions, wake.clone()));
    while !h.stop.load(Ordering::Acquire) {
        if stop_signal.requested() {
            tracing::info!("service requested host shutdown");
            h.stop.store(true, Ordering::Release);
            break;
        }
        let expired = h.sessions.lock().unwrap().expire();
        if !expired.is_empty() {
            tokio::task::spawn_blocking(move || drop(expired));
        }
        if housekeeping.as_ref().is_none_or(|task| task.is_finished())
            && (h.current_app.lock().unwrap().is_some()
                || h.app_audio.lock().unwrap().is_some()
                || !h.app_display.lock().unwrap().is_empty())
        {
            let h = h.clone();
            housekeeping = Some(tokio::task::spawn_blocking(move || {
                let Ok(_transition) = h.launch_transition.try_lock() else {
                    return;
                };
                h.reap_paused_display();
                if application_finished(&h) {
                    let canceled = h.sessions.lock().unwrap().stop_role(Role::Stream, None);
                    drop(canceled);
                    h.stop_app(AppStop::Exited);
                }
            }));
        }
        spawn_session_tick(&h, &ticking);
        hotkey.poll(&h);
        if Instant::now() >= launcher_at {
            launcher_at = Instant::now() + crate::launcher_watch::EVERY;
            let h = h.clone();
            spawn_exclusive(&launcher_reading, move || crate::launcher_watch::watch(&h));
        }
        if Instant::now() >= wake_at {
            wake_at = Instant::now() + butterpollo_core::wake::idle::TICK;
            let h = h.clone();
            spawn_exclusive(&waking, move || crate::wake::tick(&h));
        }
        if Instant::now() >= steam_at {
            steam_at = Instant::now() + Duration::from_secs(30);
            let started = spawn_library_tick(&h, &library_ticking);
            if let Some(running) = library_tick_watch.note(started, Instant::now()) {
                tracing::warn!(
                    seconds = running.as_secs(),
                    "a library check is still running: Steam, Playnite and launcher changes wait for it"
                );
            }
        }
        if Instant::now() >= update_at {
            update_at = Instant::now() + Duration::from_secs(1);
            // Due by the wall clock since the last check, so a PC that slept
            // checks as soon as it wakes and a new interval applies at once.
            if crate::updater::check_due(&h) {
                maintenance::trigger_update(&h, false);
            }
            if crate::neural::removal_due(&h) {
                let h = h.clone();
                tokio::task::spawn_blocking(move || crate::neural::remove_pending(&h));
            }
        }
        crate::updater::poll(&h);
        auto_lock.poll(&h);
        start_deferred(&h);
        if actions.is_some() {
            show_app(&h, &mut tray);
            butterpollo_windows::tray::release_overdue_notices();
        }
        if let Some(actions) = &actions {
            while let Ok(action) = actions.try_recv() {
                user_action(&h, action, web_port);
            }
        }
        if h.stop.load(Ordering::Acquire) {
            break;
        }
        if housekeeping
            .as_ref()
            .is_some_and(|task| !task.is_finished())
            || !idle(&h)
        {
            tokio::time::sleep(BUSY_TICK).await;
            continue;
        }
        let notices = actions
            .is_some()
            .then(butterpollo_windows::tray::notices_due)
            .flatten();
        let timers = [update_at, launcher_at, wake_at, steam_at];
        let at = idle_until(
            Instant::now(),
            timers.into_iter().chain(notices).chain(auto_lock.due()),
        );
        tokio::select! {
            () = tokio::time::sleep_until(at.into()) => {}
            () = wake.notified() => {}
        }
    }
}

#[cfg(test)]
mod tests;
