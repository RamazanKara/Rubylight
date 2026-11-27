//! Unit tests for session grace periods, teardown, journal recovery, and stop logging.

use super::super::*;
use super::Setup;
use crate::{display_session::tests::retained_display, state::test_support::Fixture};
use butterpollo_config::config::Config;
use std::{sync::Arc, time::Instant};

#[test]
fn the_grace_period_follows_the_existing_disconnect_delay() {
    for (settings, until) in [
        ("", Some(3000)),
        ("dd_config_revert_delay=500", Some(500)),
        ("dd_config_revert_on_disconnect=false", Some(7_200_000)),
        (
            "dd_config_revert_on_disconnect=false\ndd_paused_virtual_display_timeout_secs=60",
            Some(60_000),
        ),
        (
            "dd_config_revert_on_disconnect=false\ndd_paused_virtual_display_timeout_secs=0",
            None,
        ),
    ] {
        let setup = Setup::new(settings);
        setup.suspended();
        let snapshot = setup.host().session_snapshot();
        assert_eq!(
            snapshot.suspended_until_unix_ms,
            snapshot
                .since_unix_ms
                .zip(until)
                .map(|(since, ms)| since + ms),
            "{settings}"
        );
    }
}

#[test]
fn quick_suspended_session_grace_follows_paused_virtual_display_timeout() {
    for (settings, expected_ms) in [
        ("", Some(7_200_000)),
        ("dd_config_revert_delay=500", Some(7_200_000)),
        ("dd_paused_virtual_display_timeout_secs=60", Some(60_000)),
        ("dd_paused_virtual_display_timeout_secs=0", None),
    ] {
        let setup = Setup::new(settings);
        setup.suspended();
        setup.host().quick_suspend.set_suspended_for_test("Desktop");
        let snapshot = setup.host().session_snapshot();
        assert_eq!(
            snapshot.suspended_until_unix_ms,
            snapshot
                .since_unix_ms
                .zip(expected_ms)
                .map(|(since, ms)| since + ms),
            "{settings}"
        );
    }
}

#[test]
fn when_the_grace_period_runs_out_the_session_ends_as_end_session_does() {
    let setup = Setup::new("dd_config_revert_delay=0");
    let _session = setup.suspended();
    assert_eq!(setup.displays(), 1);
    setup.host().reap_paused_display();
    let snapshot = setup.host().session_snapshot();
    assert_eq!(snapshot.state, State::Idle);
    assert!(snapshot.game_running, "the game keeps running");
    assert_eq!(snapshot.owner, None);
    assert_eq!(setup.displays(), 0);
    assert!(setup.game_running());
    assert_eq!(setup.journal(), None);
    // Reaping again finds nothing.
    setup.host().reap_paused_display();
    assert_eq!(setup.state(), State::Idle);

    // A timeout of 0 seconds never reaps: the session stays suspended.
    let setup = Setup::new(
        "dd_config_revert_on_disconnect=false\ndd_paused_virtual_display_timeout_secs=0",
    );
    let _session = setup.suspended();
    setup.host().reap_paused_display();
    assert_eq!(setup.state(), State::Suspended);
    assert_eq!(setup.displays(), 1);

    // A display a stream holds (a second owner of the lease) is never reaped.
    let setup = Setup::new("dd_config_revert_delay=0");
    let config = setup.host().config.read().unwrap().clone();
    let display = retained_display(&setup.fixture.host, &config);
    setup.run_game();
    setup
        .host()
        .app_display
        .lock()
        .unwrap()
        .insert(setup.uuid().to_owned(), (display.clone(), None));
    let _session = setup.stream();
    setup.host().reap_paused_display();
    assert_eq!(setup.state(), State::Streaming);
    assert_eq!(setup.displays(), 1);
}

/// A game whose undo command runs for seconds must not hold its audio route:
/// the route goes once its processes have stopped, before the undo command.
#[test]
fn shutdown_releases_the_audio_route_before_the_games_undo_commands() {
    let setup = Setup::new("");
    let host = setup.host();
    let mut app = setup.game();
    app.cmd.clear();
    app.prep = vec![butterpollo_session::state::PrepCommand {
        r#do: String::new(),
        undo: "ping -n 3 127.0.0.1 >nul".into(),
        elevated: false,
        extra: Default::default(),
    }];
    let mut running = crate::process::RunningApp::with_environment(&app, Default::default())
        .expect("the test game starts");
    running.owner = setup.uuid().to_owned();
    *host.current_app.lock().unwrap() = Some(running);
    setup.retain_display();
    *host.app_audio.lock().unwrap() = Some(Arc::new(
        butterpollo_win_io::audio_route::Route::detached(&host.directory),
    ));
    let stopping = setup.fixture.host.clone();
    let began = Instant::now();
    let stopped = std::thread::spawn(move || {
        stopping.stop_app(crate::state::AppStop::Shutdown {
            deadline: Instant::now() + std::time::Duration::from_secs(5),
        })
    });
    let released = (0..100).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(10));
        host.app_audio.lock().unwrap().is_none() && host.app_display.lock().unwrap().is_empty()
    });
    assert!(
        released,
        "the display and audio route were still held while the game's processes stopped"
    );
    assert!(
        !stopped.is_finished(),
        "the audio route was not released until the undo command had finished"
    );
    stopped.join().unwrap();
    assert!(
        began.elapsed() >= std::time::Duration::from_secs(1),
        "the test's undo command finished too quickly to prove the order"
    );
    assert!(!setup.game_running());
    assert!(host.app_audio.lock().unwrap().is_none());
    assert_eq!(setup.displays(), 0);
}

#[test]
fn hold_sink_while_paused_keeps_the_audio_route_while_the_game_runs() {
    use butterpollo_win_io::audio_route::Route;
    // The game's audio route, as a stream that ended left it: no stream
    // shares it any more.
    let route = |setup: &Setup| {
        let route = Route::detached(&setup.host().directory);
        *setup.host().app_audio.lock().unwrap() = Some(Arc::new(route));
    };
    let held = |setup: &Setup| setup.host().app_audio.lock().unwrap().is_some();
    const ON: &str = "hold_sink_while_paused=true";

    // Off: the next reap drops it and Windows gets its device back.
    let setup = Setup::new("");
    let _session = setup.suspended();
    route(&setup);
    setup.host().reap_paused_display();
    assert!(!held(&setup));
    assert!(setup.game_running());

    // On: it stays while the game runs, also after the grace period
    // released the display.
    let setup = Setup::new(&format!("{ON}\ndd_config_revert_delay=0"));
    let _session = setup.suspended();
    route(&setup);
    setup.host().reap_paused_display();
    setup.host().reap_paused_display();
    assert!(held(&setup));
    assert_eq!(setup.state(), State::Idle);
    assert_eq!(setup.displays(), 0);
    // Turning the setting off releases it at the next reap.
    *setup.host().config.write().unwrap() = Config::parse("dd_config_revert_delay=0").unwrap();
    setup.host().reap_paused_display();
    assert!(!held(&setup));

    // A stream sharing the route keeps it either way.
    for settings in ["", ON] {
        let setup = Setup::new(settings);
        let _session = setup.streaming();
        route(&setup);
        let stream = setup.host().app_audio.lock().unwrap().clone();
        setup.host().reap_paused_display();
        assert!(held(&setup), "{settings}");
        drop(stream);
    }

    // The game stopping releases it, whether it quit or exited by itself
    // (housekeeping calls stop_app then).
    let setup = Setup::new(ON);
    let _session = setup.suspended();
    route(&setup);
    setup.host().reap_paused_display();
    assert!(held(&setup));
    setup.host().stop_app(crate::state::AppStop::Test);
    assert!(!held(&setup));

    // Without a game nothing holds it.
    let setup = Setup::new(ON);
    route(&setup);
    setup.host().reap_paused_display();
    assert!(!held(&setup));

    // End session releases it with the display.
    let setup = Setup::new(ON);
    let _session = setup.suspended();
    route(&setup);
    setup.host().reap_paused_display();
    assert!(held(&setup));
    assert_eq!(setup.host().end_session(None), Ok(Outcome::Done));
    assert!(!held(&setup));
    assert!(setup.game_running());
}

#[test]
fn a_teardown_shows_stopping_and_settles_when_it_ends() {
    let setup = Setup::new("");
    let session = setup.streaming();
    session.stop();
    let teardown = setup.host().session_teardown(Scope::Game);
    let snapshot = setup.host().session_snapshot();
    assert_eq!(snapshot.state, State::Stopping);
    assert_eq!(snapshot.owner.as_deref(), Some(setup.uuid()));
    assert_eq!(setup.journal().unwrap().state, State::Stopping);
    // Repeating an action while it runs is safe and does nothing.
    let host = setup.host();
    assert_eq!(host.end_session(None), Ok(Outcome::Unchanged));
    assert_eq!(host.close_game(setup.uuid()), Ok(Outcome::Unchanged));
    assert_eq!(host.disconnect_session(None), Ok(Outcome::Unchanged));
    assert_eq!(host.resume_session(setup.uuid()), Err(Refusal::Busy));
    // A second teardown joins the first and does not end it early.
    drop(host.session_teardown(Scope::Game));
    assert_eq!(setup.state(), State::Stopping);
    drop(teardown);
    // The game and its display remain, so the session rests suspended.
    assert_eq!(setup.state(), State::Suspended);
}

#[test]
fn a_teardown_of_nothing_does_not_enter_stopping() {
    let setup = Setup::new("");
    let teardown = setup.host().session_teardown(Scope::Game);
    assert_eq!(setup.state(), State::Idle);
    drop(teardown);
    let teardown = setup.host().session_teardown(Scope::Session);
    assert_eq!(setup.state(), State::Idle);
    drop(teardown);
    // Only the displays of the devices that expired count.
    let setup = Setup::new("");
    let _session = setup.suspended();
    let owners = ["someone-else".to_owned()];
    let teardown = setup.host().session_teardown(Scope::Displays(&owners));
    assert_eq!(setup.state(), State::Suspended);
    drop(teardown);
    let owners = [setup.uuid().to_owned()];
    let teardown = setup.host().session_teardown(Scope::Displays(&owners));
    assert_eq!(setup.state(), State::Stopping);
    drop(teardown);
}

#[test]
fn the_stock_stop_paths_end_the_game_and_the_session() {
    // /cancel, the tray and "Quit app" all end in stop_app, after the
    // streams were told to stop.
    let setup = Setup::new("");
    let session = setup.streaming();
    session.stop();
    setup.host().stop_app(crate::state::AppStop::Test);
    let snapshot = setup.host().session_snapshot();
    assert_eq!(snapshot.state, State::Idle);
    assert!(!snapshot.game_running);
    assert_eq!(setup.displays(), 0);
    assert_eq!(setup.journal(), None);
    // The tray's Stop sessions releases the streams and displays only.
    let setup = Setup::new("");
    let session = setup.streaming();
    crate::runtime::release_displays(&setup.fixture.host);
    assert!(session.stopping());
    let snapshot = setup.host().session_snapshot();
    assert_eq!(snapshot.state, State::Idle);
    assert!(snapshot.game_running);
    assert_eq!(setup.displays(), 0);
    assert_eq!(setup.journal(), None);
}

#[test]
fn cleanup_repeats_safely_in_every_order() {
    let setup = Setup::new("");
    let h = setup.fixture.host.clone();
    setup.suspended();
    for _ in 0..3 {
        let _ = h.end_session(None);
        let _ = h.close_game(setup.uuid());
        h.stop_app(crate::state::AppStop::Test);
        crate::runtime::release_displays(&h);
        h.reap_paused_display();
        h.session_tick();
        let _ = h.disconnect_session(None);
    }
    let snapshot = h.session_snapshot();
    assert_eq!(snapshot.state, State::Idle);
    assert!(!snapshot.game_running);
    assert_eq!(setup.displays(), 0);
    assert_eq!(setup.journal(), None);
    // The host is still usable afterwards.
    let _session = setup.streaming();
    assert_eq!(h.end_session(None), Ok(Outcome::Done));
    assert_eq!(h.session_snapshot().state, State::Idle);
}

#[test]
fn the_actions_the_view_allows_are_the_actions_the_host_accepts() {
    use butterpollo_session::native::session_view;
    use rubylight_native_api::session::ActionId;
    for scenario in ["idle", "game", "streaming", "suspended"] {
        for action in [
            ActionId::Resume,
            ActionId::Disconnect,
            ActionId::End,
            ActionId::CloseGame,
        ] {
            let setup = Setup::new("");
            match scenario {
                "game" => setup.run_game(),
                "streaming" => {
                    setup.streaming();
                }
                "suspended" => {
                    setup.suspended();
                }
                _ => {}
            }
            let host = setup.host();
            let mut caller = setup.client.clone();
            caller.extra.insert("host_control".into(), true.into());
            let view = session_view(&host.session_snapshot(), &caller);
            let allowed = view
                .actions
                .iter()
                .find(|a| a.id == action)
                .unwrap()
                .allowed;
            let result = match action {
                ActionId::Resume => host.resume_session(setup.uuid()),
                ActionId::Disconnect => host.disconnect_session(None),
                ActionId::End => host.end_session(None),
                _ => host.close_game(setup.uuid()),
            };
            // Resume on a suspended session is allowed but has nothing to do
            // here: the client's own /resume launch reattaches it.
            let ran = result == Ok(Outcome::Done)
                || (action == ActionId::Resume && result == Ok(Outcome::Unchanged));
            assert_eq!(allowed, ran, "{scenario} {action:?}: {result:?}");
        }
    }
}

#[test]
fn a_journal_that_survived_a_crash_is_cleared_once_at_the_next_start() {
    // The host stopped while streaming: its journal is what is left.
    let crashed = Setup::new("");
    let _session = crashed.streaming();
    crashed.host().session_flush();
    let bytes = std::fs::read(crashed.host().directory.join(JOURNAL)).unwrap();
    let directory = Fixture::profile();
    std::fs::write(directory.join(JOURNAL), &bytes).unwrap();
    let restarted = Fixture::load(directory.clone());
    let host = &restarted.host;
    let journal = host.recover_session().unwrap();
    assert_eq!(journal.state, State::Streaming);
    assert_eq!(journal.owner.as_deref(), Some(crashed.uuid()));
    assert!(!directory.join(JOURNAL).exists());
    assert!(
        host.warnings
            .snapshot()
            .iter()
            .any(|warning| warning.code == "session_interrupted")
    );
    // The new host is idle and usable; recovering again does nothing.
    assert_eq!(host.session_snapshot().state, State::Idle);
    assert_eq!(host.recover_session(), None);
    assert_eq!(host.recover_session(), None);
    assert_eq!(host.end_session(None), Err(Refusal::NoSession));
    assert_eq!(host.session_snapshot().state, State::Idle);
    assert!(!directory.join(JOURNAL).exists());
}

#[test]
fn a_journal_recovery_could_not_delete_is_deleted_by_the_writer_later() {
    use std::os::windows::fs::OpenOptionsExt;
    let crashed = Setup::new("");
    let _session = crashed.streaming();
    crashed.host().session_flush();
    let bytes = std::fs::read(crashed.host().directory.join(JOURNAL)).unwrap();
    let directory = Fixture::profile();
    let path = directory.join(JOURNAL);
    std::fs::write(&path, &bytes).unwrap();
    // Another process holds the file open without sharing deletes (a virus
    // scanner, a backup): reading works, deleting does not.
    let holder = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&path)
        .unwrap();
    let restarted = Fixture::load(directory.clone());
    let host = &restarted.host;
    let journal = host.recover_session().unwrap();
    assert_eq!(journal.state, State::Streaming);
    assert!(path.exists(), "the delete failed");
    // The next housekeeping pass tells the writer to delete it, and the
    // writer keeps trying while the file is held.
    assert_eq!(host.session_snapshot().state, State::Idle);
    host.session_flush();
    assert!(path.exists());
    drop(holder);
    let deadline = Instant::now() + Duration::from_secs(5);
    while path.exists() && Instant::now() < deadline {
        host.session_tick();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!path.exists(), "the writer deleted it once it could");
    // Recovering again finds nothing.
    assert_eq!(host.recover_session(), None);
}

#[test]
fn a_damaged_journal_is_deleted_and_the_host_starts_idle() {
    let directory = Fixture::profile();
    std::fs::write(directory.join(JOURNAL), b"{ not json").unwrap();
    let restarted = Fixture::load(directory.clone());
    assert_eq!(restarted.host.recover_session(), None);
    assert!(!directory.join(JOURNAL).exists());
    assert_eq!(restarted.host.session_snapshot().state, State::Idle);
    assert_eq!(restarted.host.recover_session(), None);
}

#[test]
fn a_clean_shutdown_leaves_no_journal_to_recover() {
    let setup = Setup::new("");
    let session = setup.streaming();
    assert!(setup.journal().is_some());
    session.stop();
    setup.host().stop_app(crate::state::AppStop::Test);
    setup.host().session_shutdown();
    assert_eq!(setup.journal(), None);
    assert_eq!(setup.host().recover_session(), None);
    assert_eq!(setup.state(), State::Idle);
}

#[test]
fn a_journal_that_could_not_be_deleted_is_deleted_by_a_later_try() {
    use std::os::windows::fs::OpenOptionsExt;
    // A stale journal another program holds open without sharing it cannot be
    // deleted. (A directory in its place, swapped for a file, left a moment
    // with nothing there: a try in that moment found nothing to delete and
    // was done, and the stale file stayed.)
    let directory = Fixture::profile();
    let path = directory.join(JOURNAL);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(&path, b"stale").unwrap();
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&path)
        .unwrap();
    let (sender, messages) = mpsc::channel();
    let writer = std::thread::spawn({
        let path = path.clone();
        move || write_journals(&path, &messages)
    });
    sender.send(Message::Write(None)).unwrap();
    let (ack, done) = mpsc::channel();
    sender.send(Message::Flush(ack)).unwrap();
    done.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(path.exists(), "the delete failed");
    // The other program lets go: the writer must not have assumed the delete
    // had worked.
    drop(held);
    let deadline = Instant::now() + Duration::from_secs(5);
    while path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!path.exists(), "the writer retried the delete");
    drop(sender);
    writer.join().unwrap();
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_stop_logs_its_reason_the_client_that_asked_and_the_app() {
    let setup = Setup::new("");
    setup.suspended();
    let game = setup.game();
    let path = setup.host().directory.join("app-stop.log");
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .without_time()
        .with_ansi(false)
        .with_target(false)
        .with_writer(std::sync::Mutex::new(std::fs::File::create(&path).unwrap()))
        .finish();
    crate::test_log::capture_during(subscriber, || {
        assert_eq!(setup.host().close_game(setup.uuid()), Ok(Outcome::Done));
        // Nothing runs any more: a second stop has nothing to log.
        setup.host().stop_app(crate::state::AppStop::Shutdown {
            deadline: Instant::now(),
        });
    });
    let log = std::fs::read_to_string(path).unwrap();
    let stops: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("stopping the application"))
        .collect();
    assert_eq!(stops.len(), 1, "{log}");
    for field in [
        "reason=\"close game\"".to_owned(),
        format!("by=\"{}\"", setup.uuid()),
        format!("app={}", game.name),
        format!("owner={}", setup.uuid()),
        "app_id=".to_owned(),
    ] {
        assert!(stops[0].contains(&field), "{field} missing: {log}");
    }
}

#[test]
fn each_stop_names_its_reason_and_only_client_stops_name_a_client() {
    use crate::state::AppStop;
    let cases = [
        (AppStop::Cancel("a"), "cancel", Some("a")),
        (AppStop::Terminate("b"), "terminate", Some("b")),
        (AppStop::WebRtc("c"), "webrtc", Some("c")),
        (AppStop::CloseGame("d"), "close game", Some("d")),
        (AppStop::Replaced("e"), "replaced", Some("e")),
        (AppStop::WebUi, "web ui", None),
        (AppStop::Tray, "tray", None),
        (AppStop::Exited, "exited", None),
        (AppStop::LaunchFailed, "launch failed", None),
        (
            AppStop::Shutdown {
                deadline: Instant::now(),
            },
            "shutdown",
            None,
        ),
    ];
    for (stop, reason, client) in cases {
        assert_eq!((stop.reason(), stop.client()), (reason, client), "{stop:?}");
    }
}
