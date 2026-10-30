//! Host housekeeping and user actions, independent of listener setup.
use crate::{
    maintenance,
    state::{AppStop, Shared},
};
use butterpollo_core::session::Role;
use butterpollo_windows::{process::StopSignal, tray::Action};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Receiver,
    },
    time::{Duration, Instant},
};

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
    let connected = streaming(h);
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
                if current
                    .as_ref()
                    .is_some_and(|placeholder| placeholder.generation == generation)
                {
                    started.generation = generation;
                    started.owner = owner;
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
        match butterpollo_windows::hotkey::Hotkey::register(key, modifiers, move || {
            let _ = pressed.send(());
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
    let mut hotkey = RestoreHotkey::default();
    let mut tray = butterpollo_core::tray::Tracker::default();
    let mut housekeeping: Option<tokio::task::JoinHandle<()>> = None;
    let ticking = Arc::new(AtomicBool::new(false));
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
            let h = h.clone();
            tokio::task::spawn_blocking(move || {
                crate::steam::watch(&h);
                crate::playnite::watch(&h);
                crate::library::watch(&h);
            });
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
        start_deferred(&h);
        if actions.is_some() {
            show_app(&h, &mut tray);
        }
        if let Some(actions) = &actions {
            while let Ok(action) = actions.try_recv() {
                user_action(&h, action, web_port);
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::Fixture;

    #[tokio::test]
    async fn a_session_tick_in_flight_is_not_joined_by_another() {
        let running = Arc::new(AtomicBool::new(false));
        let (release, hold) = std::sync::mpsc::channel::<()>();
        assert!(spawn_exclusive(&running, move || {
            let _ = hold.recv();
        }));
        // The first one is stuck: the next ticks are skipped, not queued.
        let ran = Arc::new(AtomicBool::new(false));
        for _ in 0..3 {
            let ran = ran.clone();
            assert!(!spawn_exclusive(&running, move || {
                ran.store(true, Ordering::Release);
            }));
        }
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while running.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "the first run finished");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(!ran.load(Ordering::Acquire));
        // Once it finished, the next tick runs, also after a panic.
        assert!(spawn_exclusive(&running, || panic!("a failing tick")));
        while running.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "the flag clears after a panic");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(spawn_exclusive(&running, || ()));
    }

    fn steam_app() -> crate::process::RunningApp {
        let mut app = crate::process::RunningApp::with_environment(
            &butterpollo_core::state::App::desktop(),
            Default::default(),
        )
        .unwrap();
        app.track_steam(r"C:\not-a-game");
        app
    }

    #[test]
    fn a_steam_games_process_scan_leaves_the_running_app_to_the_stream() {
        let fixture = Fixture::new();
        *fixture.host.current_app.lock().unwrap() = Some(steam_app());
        let mut scanned = false;
        let paths = std::cell::RefCell::new(vec![]);
        assert!(!application_finished_with(
            &fixture.host,
            || {
                // The stream and input threads look at the app while the list is read.
                assert!(
                    fixture.host.try_current_app().is_some(),
                    "the process scan held the running app"
                );
                scanned = true;
                Ok(vec![butterpollo_core::steam::Process {
                    pid: 4242,
                    parent: 1,
                    started: 10,
                    name: "game.exe".into(),
                }])
            },
            |pid| {
                // And while the new process's program path is.
                assert!(
                    fixture.host.try_current_app().is_some(),
                    "the path read held the running app"
                );
                paths.borrow_mut().push(pid);
                Some(r"C:\not-a-game\game.exe".into())
            }
        ));
        assert!(scanned);
        assert_eq!(*paths.borrow(), [4242]);
        // The list was applied: the game runs, and the next look is a second away.
        let current = fixture.host.current_app.lock().unwrap();
        let app = current.as_ref().unwrap();
        assert!(!app.wants_processes());
        assert_eq!(app.steam_tracked(), [4242]);
    }

    #[test]
    fn a_failed_process_scan_is_tried_again_a_second_later_not_every_tick() {
        let fixture = Fixture::new();
        *fixture.host.current_app.lock().unwrap() = Some(steam_app());
        let scans = std::cell::Cell::new(0);
        for _ in 0..3 {
            assert!(!application_finished_with(
                &fixture.host,
                || {
                    scans.set(scans.get() + 1);
                    anyhow::bail!("no process list")
                },
                |_| None
            ));
        }
        assert_eq!(scans.get(), 1);
        let current = fixture.host.current_app.lock().unwrap();
        assert!(!current.as_ref().unwrap().wants_processes());
    }

    #[test]
    fn a_process_list_read_for_a_replaced_app_is_not_applied_to_its_successor() {
        let fixture = Fixture::new();
        *fixture.host.current_app.lock().unwrap() = Some(steam_app());
        assert!(!application_finished_with(
            &fixture.host,
            || {
                *fixture.host.current_app.lock().unwrap() = Some(steam_app());
                Ok(Vec::new())
            },
            |_| None
        ));
        // The new app still gets its own first look.
        let current = fixture.host.current_app.lock().unwrap();
        assert!(current.as_ref().unwrap().wants_processes());
    }

    #[test]
    fn global_close_on_disconnect_waits_for_the_last_streaming_device() {
        let fixture = Fixture::new();
        fixture
            .host
            .config
            .write()
            .unwrap()
            .values
            .insert("terminate_on_pause".into(), "true".into());
        let app = serde_json::from_value(serde_json::json!({"name":"Desktop"})).unwrap();
        *fixture.host.current_app.lock().unwrap() =
            Some(crate::process::launch(&fixture.host, &app, &Default::default()).unwrap());
        assert!(!application_finished(&fixture.host));
        let client = fixture.client(u32::MAX);
        let first = fixture.session(client.clone(), Role::Stream);
        let second = fixture.session(client.clone(), Role::Stream);
        let _input = fixture.session(client, Role::InputOnly);
        assert!(!application_finished(&fixture.host));
        fixture
            .host
            .sessions
            .lock()
            .unwrap()
            .active
            .remove(&first.launch.id);
        assert!(!application_finished(&fixture.host));
        fixture
            .host
            .sessions
            .lock()
            .unwrap()
            .active
            .remove(&second.launch.id);
        assert!(application_finished(&fixture.host));
    }
}
