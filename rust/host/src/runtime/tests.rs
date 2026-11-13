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

#[test]
fn a_hung_library_tick_is_told_once_and_the_next_one_starts_over() {
    let start = Instant::now();
    let mut watch = LibraryTickWatch::default();
    assert_eq!(watch.note(false, start), None, "none started yet");
    assert_eq!(watch.note(true, start), None);
    assert_eq!(watch.note(false, start + Duration::from_secs(30)), None);
    let late = start + LIBRARY_TICK_TOO_LONG;
    assert_eq!(watch.note(false, late), Some(LIBRARY_TICK_TOO_LONG));
    assert_eq!(
        watch.note(false, late + Duration::from_secs(30)),
        None,
        "once"
    );
    assert_eq!(watch.note(true, late), None);
    let again = late + LIBRARY_TICK_TOO_LONG;
    assert_eq!(watch.note(false, again), Some(LIBRARY_TICK_TOO_LONG));
}

#[tokio::test]
async fn a_library_tick_in_flight_is_not_joined_by_another() {
    let f = Fixture::new();
    let running = Arc::new(AtomicBool::new(true));
    assert!(!spawn_library_tick(&f.host, &running));
    assert!(
        running.load(Ordering::Acquire),
        "the one in flight still holds it"
    );
}

#[test]
fn an_idle_loop_sleeps_until_its_first_timer_within_its_bounds() {
    let now = Instant::now();
    assert_eq!(idle_until(now, std::iter::empty()), now + IDLE_TICK);
    let soon = now + Duration::from_millis(400);
    let later = now + Duration::from_secs(30);
    assert_eq!(idle_until(now, [later, soon]), soon);
    assert_eq!(idle_until(now, [later]), now + IDLE_TICK);
    // A timer already due runs no more often than a busy host ticks.
    assert_eq!(idle_until(now, [now, soon]), now + BUSY_TICK);
}

#[test]
fn the_host_is_idle_only_with_nothing_the_tick_follows() {
    let f = Fixture::new();
    assert!(idle(&f.host), "a host that has just started");
    // A launch waiting for its client.
    let launch = f.launch(f.client(u32::MAX), Role::Stream);
    let canceled = f.host.sessions.lock().unwrap().queue(launch).unwrap();
    drop(canceled);
    assert!(!idle(&f.host));
    let canceled = f.host.sessions.lock().unwrap().request_stop(None);
    drop(canceled);
    assert!(idle(&f.host));
    // A running app.
    *f.host.current_app.lock().unwrap() = Some(steam_app());
    assert!(!idle(&f.host));
    let app = f.host.current_app.lock().unwrap().take();
    drop(app);
    assert!(idle(&f.host));
    // An update waiting to install.
    f.host.updates.lock().unwrap()["phase"] = serde_json::json!("waiting");
    assert!(!idle(&f.host));
    f.host.updates.lock().unwrap()["phase"] = serde_json::json!("idle");
    assert!(idle(&f.host));
    // The session machine, held by an action.
    let held = f.host.session.lock().unwrap();
    assert!(!idle(&f.host));
    drop(held);
    assert!(idle(&f.host));
}

/// Whether `wake` was told, or is told within `wait`.
async fn woken(wake: &Notify, wait: Duration) -> bool {
    tokio::time::timeout(wait, wake.notified()).await.is_ok()
}

#[tokio::test]
async fn a_launch_a_wake_and_a_tray_action_wake_the_idle_loop() {
    let f = Fixture::new();
    let wake = f.host.maintain_wake().clone();
    let none = Duration::from_millis(50);
    assert!(
        !woken(&wake, none).await,
        "nothing woke a host that has just started"
    );
    let launch = f.launch(f.client(u32::MAX), Role::Stream);
    let canceled = f.host.sessions.lock().unwrap().queue(launch).unwrap();
    drop(canceled);
    assert!(woken(&wake, none).await, "a launch wakes it");
    let canceled = f.host.sessions.lock().unwrap().request_stop(None);
    drop(canceled);
    assert!(!woken(&wake, none).await, "a launch that went does not");
    f.host.wake_maintenance();
    assert!(woken(&wake, none).await);
    // A tray click is handed on, then wakes it.
    let (tray, clicks) = std::sync::mpsc::channel();
    let actions = forward_actions(clicks, wake.clone());
    tray.send(Action::Open).unwrap();
    assert!(woken(&wake, Duration::from_secs(5)).await);
    assert!(matches!(actions.try_recv(), Ok(Action::Open)));
    // The thread ends with the tray.
    drop(tray);
    assert!(matches!(
        actions.recv_timeout(Duration::from_secs(5)),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
    ));
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
fn close_on_disconnect_waits_for_the_device_continuing_the_game() {
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
    let phone = fixture.client(u32::MAX);
    let stream = fixture.session(phone.clone(), Role::Stream);
    assert!(!application_finished(&fixture.host));
    let mut tv = phone;
    tv.uuid = "tv".into();
    tv.name = "TV".into();
    fixture.host.continue_here(&tv).unwrap();
    assert!(stream.stopping());
    fixture
        .host
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&stream.launch.id);
    assert!(
        !application_finished(&fixture.host),
        "the game outlives the gap between the two streams"
    );
    // The TV never came, and the phone never left on purpose: it gets to come back.
    fixture.host.moves.expire_claim();
    fixture.host.reap_paused_display();
    assert!(
        !application_finished(&fixture.host),
        "a failed move does not close the game"
    );
    // It did not: now the last device has left.
    fixture.host.moves.expire_back();
    assert!(application_finished(&fixture.host));
}

#[test]
fn ending_the_session_after_a_failed_move_gives_up_the_window() {
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
    let phone = fixture.client(u32::MAX);
    // Its kept display is what makes this a session to end.
    let config = fixture.host.config.read().unwrap().clone();
    let display = crate::display_session::tests::retained_display(&fixture.host, &config);
    fixture
        .host
        .app_display
        .lock()
        .unwrap()
        .insert(phone.uuid.clone(), (display, None));
    let stream = fixture.session(phone.clone(), Role::Stream);
    assert!(!application_finished(&fixture.host));
    let mut tv = phone.clone();
    tv.uuid = "tv".into();
    fixture.host.continue_here(&tv).unwrap();
    let sessions = &fixture.host.sessions;
    sessions.lock().unwrap().active.remove(&stream.launch.id);
    fixture.host.moves.expire_claim();
    fixture.host.reap_paused_display();
    assert!(!application_finished(&fixture.host));
    fixture.host.end_session(Some(&phone.uuid)).unwrap();
    assert!(
        application_finished(&fixture.host),
        "the phone said it is done, so it is not coming back"
    );
}

#[test]
fn the_device_that_came_back_after_a_failed_move_closes_the_game_by_leaving() {
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
    let phone = fixture.client(u32::MAX);
    let stream = fixture.session(phone.clone(), Role::Stream);
    assert!(!application_finished(&fixture.host));
    let mut tv = phone.clone();
    tv.uuid = "tv".into();
    fixture.host.continue_here(&tv).unwrap();
    let sessions = &fixture.host.sessions;
    sessions.lock().unwrap().active.remove(&stream.launch.id);
    fixture.host.moves.expire_claim();
    fixture.host.reap_paused_display();
    assert!(!application_finished(&fixture.host));
    let back = fixture.session(phone, Role::Stream);
    assert!(!application_finished(&fixture.host));
    sessions.lock().unwrap().active.remove(&back.launch.id);
    // Its window closed when it came back: leaving now is leaving.
    assert!(application_finished(&fixture.host));
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
