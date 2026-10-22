use super::*;
use crate::{
    display_session::tests::retained_display,
    state::{Session, test_support::Fixture},
};
use butterpollo_core::{config::Config, state::Client};
use std::{sync::Arc, time::Instant};

/// A host with one paired device, which owns whatever the test starts.
struct Setup {
    fixture: Fixture,
    client: Client,
}
impl Setup {
    fn new(settings: &str) -> Self {
        let fixture = Fixture::new();
        let client = fixture.client(u32::MAX);
        *fixture.host.config.write().unwrap() = Config::parse(settings).unwrap();
        Self { fixture, client }
    }
    fn host(&self) -> &Host {
        &self.fixture.host
    }
    fn uuid(&self) -> &str {
        &self.client.uuid
    }
    fn game(&self) -> App {
        self.host().apps.read().unwrap()[0].clone()
    }
    /// The device's game runs.
    fn run_game(&self) {
        let mut running =
            crate::process::RunningApp::with_environment(&self.game(), Default::default()).unwrap();
        running.owner = self.uuid().to_owned();
        *self.host().current_app.lock().unwrap() = Some(running);
    }
    /// The device's game display is kept between its streams.
    fn retain_display(&self) {
        let config = self.host().config.read().unwrap().clone();
        let display = retained_display(&self.fixture.host, &config);
        self.host()
            .app_display
            .lock()
            .unwrap()
            .insert(self.uuid().to_owned(), (display, None));
    }
    fn stream(&self) -> Arc<Session> {
        self.fixture.session(self.client.clone(), Role::Stream)
    }
    /// A game, its display and a stream: `streaming`.
    fn streaming(&self) -> Arc<Session> {
        self.run_game();
        self.retain_display();
        let session = self.stream();
        assert_eq!(self.state(), State::Streaming);
        session
    }
    /// The stream ended by itself and the display is kept: `suspended`.
    fn suspended(&self) -> Arc<Session> {
        let session = self.streaming();
        session.stop();
        assert_eq!(self.state(), State::Suspended);
        session
    }
    fn state(&self) -> State {
        self.host().session_snapshot().state
    }
    fn journal(&self) -> Option<Journal> {
        Journal::parse(&std::fs::read(self.host().directory.join(JOURNAL)).ok()?)
    }
    fn displays(&self) -> usize {
        self.host().app_display.lock().unwrap().len()
    }
    fn game_running(&self) -> bool {
        self.host().current_app.lock().unwrap().is_some()
    }
}

#[test]
fn a_host_with_no_stream_or_game_is_idle_and_every_action_says_so() {
    let setup = Setup::new("");
    let snapshot = setup.host().session_snapshot();
    assert_eq!(snapshot.state, State::Idle);
    assert!(!snapshot.game_running);
    assert_eq!((snapshot.owner, snapshot.app), (None, None));
    assert_eq!(setup.journal(), None);
    let host = setup.host();
    assert_eq!(host.disconnect_session(), Err(Refusal::NoSession));
    assert_eq!(host.end_session(), Err(Refusal::NoSession));
    assert_eq!(host.close_game(setup.uuid()), Err(Refusal::NoSession));
    assert_eq!(host.resume_session(setup.uuid()), Err(Refusal::NoSession));
    assert_eq!(setup.state(), State::Idle);
    assert_eq!(setup.journal(), None);
}

#[test]
fn a_stream_makes_the_session_streaming_names_its_owner_and_is_journaled() {
    let setup = Setup::new("");
    let _session = setup.streaming();
    let snapshot = setup.host().session_snapshot();
    assert_eq!(snapshot.owner.as_deref(), Some(setup.uuid()));
    assert!(snapshot.game_running);
    assert_eq!(snapshot.app.unwrap().title, setup.game().name);
    assert!(snapshot.since_unix_ms.is_some());
    assert_eq!(snapshot.suspended_until_unix_ms, None);
    let journal = setup.journal().unwrap();
    assert_eq!(journal.state, State::Streaming);
    assert_eq!(journal.owner.as_deref(), Some(setup.uuid()));
}

#[test]
fn disconnect_suspends_the_session_and_keeps_the_game_and_the_display() {
    let setup = Setup::new("");
    let session = setup.streaming();
    assert_eq!(setup.host().disconnect_session(), Ok(Outcome::Done));
    assert!(session.stopping());
    let snapshot = setup.host().session_snapshot();
    assert_eq!(snapshot.state, State::Suspended);
    assert_eq!(snapshot.owner.as_deref(), Some(setup.uuid()));
    assert!(snapshot.game_running);
    assert!(setup.game_running());
    assert_eq!(setup.displays(), 1);
    // The deadline is the existing disconnect delay (3 s by default).
    assert_eq!(
        snapshot.suspended_until_unix_ms,
        snapshot.since_unix_ms.map(|since| since + 3000)
    );
    assert_eq!(setup.journal().unwrap().state, State::Suspended);
    // A repeat has no stream to drop.
    assert_eq!(setup.host().disconnect_session(), Ok(Outcome::Unchanged));
    assert_eq!(setup.state(), State::Suspended);
}

#[test]
fn a_stream_that_ends_by_itself_suspends_a_game_that_keeps_its_display() {
    let setup = Setup::new("");
    let session = setup.streaming();
    session.stop();
    assert_eq!(setup.state(), State::Suspended);
    // Without a kept display there is nothing to wait for.
    let setup = Setup::new("");
    setup.run_game();
    let session = setup.stream();
    assert_eq!(setup.state(), State::Streaming);
    session.stop();
    let snapshot = setup.host().session_snapshot();
    assert_eq!(snapshot.state, State::Idle);
    assert!(snapshot.game_running);
    assert_eq!(snapshot.owner, None);
    assert_eq!(setup.journal(), None);
}

#[test]
fn end_session_releases_the_stream_and_display_and_leaves_the_game_running() {
    for suspended in [false, true] {
        let setup = Setup::new("");
        let session = if suspended {
            setup.suspended()
        } else {
            setup.streaming()
        };
        assert_eq!(setup.host().end_session(), Ok(Outcome::Done));
        assert!(session.stopping());
        let snapshot = setup.host().session_snapshot();
        assert_eq!(snapshot.state, State::Idle, "suspended={suspended}");
        assert!(snapshot.game_running);
        assert_eq!(snapshot.owner, None);
        assert_eq!(setup.displays(), 0);
        assert!(setup.game_running());
        assert_eq!(setup.journal(), None);
        // A repeat finds no session.
        assert_eq!(setup.host().end_session(), Err(Refusal::NoSession));
        assert_eq!(setup.state(), State::Idle);
    }
}

#[test]
fn end_session_waits_for_the_stream_worker_to_release_its_resources() {
    let setup = Setup::new("");
    let session = setup.streaming();
    let id = session.launch.id.clone();
    let host = setup.fixture.host.clone();
    let worker = std::thread::spawn(move || {
        while !host
            .sessions
            .lock()
            .unwrap()
            .active
            .get(&id)
            .is_some_and(|session| session.stopping())
        {
            std::thread::sleep(Duration::from_millis(2));
        }
        std::thread::sleep(Duration::from_millis(60));
        host.sessions.lock().unwrap().active.remove(&id);
    });
    let started = Instant::now();
    assert_eq!(setup.host().end_session(), Ok(Outcome::Done));
    assert!(setup.host().sessions.lock().unwrap().active.is_empty());
    assert!(
        started.elapsed() < TEARDOWN_WAIT,
        "it did not wait for nothing"
    );
    worker.join().unwrap();
}

#[test]
fn close_game_ends_the_game_too_in_every_state_that_has_a_session_or_a_game() {
    // Streaming and suspended: the session and the game end.
    for suspended in [false, true] {
        let setup = Setup::new("");
        if suspended {
            setup.suspended();
        } else {
            setup.streaming();
        }
        assert_eq!(setup.host().close_game(setup.uuid()), Ok(Outcome::Done));
        let snapshot = setup.host().session_snapshot();
        assert_eq!(snapshot.state, State::Idle, "suspended={suspended}");
        assert!(!snapshot.game_running);
        assert!(!setup.game_running());
        assert_eq!(setup.displays(), 0);
        assert_eq!(setup.journal(), None);
        assert_eq!(
            setup.host().close_game(setup.uuid()),
            Err(Refusal::NoSession)
        );
    }
    // A game End session left running: idle with the game, and Close game
    // still closes it.
    let setup = Setup::new("");
    setup.suspended();
    assert_eq!(setup.host().end_session(), Ok(Outcome::Done));
    let snapshot = setup.host().session_snapshot();
    assert_eq!(snapshot.state, State::Idle);
    assert!(snapshot.game_running);
    assert_eq!(setup.host().close_game("someone"), Ok(Outcome::Done));
    assert!(!setup.game_running());
    assert!(!setup.host().session_snapshot().game_running);
    assert_eq!(setup.state(), State::Idle);
    assert_eq!(setup.journal(), None);
}

#[test]
fn end_session_and_close_game_are_busy_while_a_launch_prepares_or_starts() {
    let setup = Setup::new("");
    setup.run_game();
    let host = setup.host();
    let launch = host.session_begin(setup.uuid(), Some(&setup.game()));
    for starting in [false, true] {
        if starting {
            launch.app_starting();
        }
        let expected = if starting {
            State::Launching
        } else {
            State::Preparing
        };
        assert_eq!(setup.state(), expected);
        assert_eq!(host.end_session(), Err(Refusal::Busy));
        assert_eq!(host.close_game(setup.uuid()), Err(Refusal::Busy));
        assert_eq!(host.resume_session(setup.uuid()), Err(Refusal::Busy));
        // A disconnect has no stream to drop yet, which is not an error.
        assert_eq!(host.disconnect_session(), Ok(Outcome::Unchanged));
        // Nothing changed, and the game still runs.
        assert_eq!(setup.state(), expected);
        assert!(setup.game_running());
    }
    let snapshot = host.session_snapshot();
    assert_eq!(snapshot.owner.as_deref(), Some(setup.uuid()));
    assert_eq!(snapshot.app.unwrap().title, setup.game().name);
    // A launch call that fails returns the session to what it was.
    drop(launch);
    assert_eq!(setup.state(), State::Idle);
    assert!(host.session_snapshot().game_running);
}

#[test]
fn end_session_and_close_game_are_busy_while_the_launch_lock_is_held() {
    let setup = Setup::new("");
    setup.suspended();
    let transition = setup.host().launch_transition.lock().unwrap();
    assert_eq!(setup.host().end_session(), Err(Refusal::Busy));
    assert_eq!(setup.host().close_game(setup.uuid()), Err(Refusal::Busy));
    assert_eq!(setup.state(), State::Suspended);
    assert!(setup.game_running());
    drop(transition);
    assert_eq!(setup.host().end_session(), Ok(Outcome::Done));
}

#[test]
fn a_launch_moves_through_preparing_and_launching_to_streaming() {
    let setup = Setup::new("");
    setup.run_game();
    setup.retain_display();
    let host = setup.host();
    let mut launch = host.session_begin(setup.uuid(), Some(&setup.game()));
    assert_eq!(setup.state(), State::Preparing);
    launch.app_starting();
    assert_eq!(setup.state(), State::Launching);
    // The launch reply went out and the client has 30 s to connect.
    let pending = setup.fixture.launch(setup.client.clone(), Role::Stream);
    let id = pending.id.clone();
    host.sessions
        .lock()
        .unwrap()
        .pending
        .insert(id.clone(), pending);
    launch.succeed();
    drop(launch);
    assert_eq!(setup.state(), State::Launching);
    host.session_tick();
    assert_eq!(setup.state(), State::Launching);
    // The client connects: the pending launch becomes a stream.
    host.sessions.lock().unwrap().pending.remove(&id);
    let _session = setup.stream();
    let snapshot = host.session_snapshot();
    assert_eq!(snapshot.state, State::Streaming);
    assert_eq!(snapshot.owner.as_deref(), Some(setup.uuid()));
}

#[test]
fn a_launch_nobody_joins_returns_the_session_to_where_it_was() {
    for display in [false, true] {
        let setup = Setup::new("");
        setup.run_game();
        if display {
            setup.retain_display();
        }
        let host = setup.host();
        let mut launch = host.session_begin(setup.uuid(), Some(&setup.game()));
        launch.app_starting();
        let mut pending = setup.fixture.launch(setup.client.clone(), Role::Stream);
        pending.created = Instant::now()
            .checked_sub(Duration::from_secs(31))
            .expect("the clock has run for 31 s");
        host.sessions
            .lock()
            .unwrap()
            .pending
            .insert(pending.id.clone(), pending);
        launch.succeed();
        drop(launch);
        // The 30 s ran out.
        let expected = if display {
            State::Suspended
        } else {
            State::Idle
        };
        assert_eq!(setup.state(), expected, "display={display}");
        assert!(host.session_snapshot().game_running);
    }
}

#[test]
fn a_failed_launch_of_a_suspended_session_leaves_it_suspended() {
    let setup = Setup::new("");
    setup.suspended();
    let launch = setup
        .host()
        .session_begin(setup.uuid(), Some(&setup.game()));
    assert_eq!(setup.state(), State::Preparing);
    drop(launch);
    assert_eq!(setup.state(), State::Suspended);
    assert_eq!(setup.displays(), 1);
}

#[test]
fn resume_by_the_owner_reattaches_a_suspended_session_and_supersedes_a_live_one() {
    let setup = Setup::new("");
    let host = setup.host();
    let _session = setup.suspended();
    // The client's own /resume launch reattaches it: nothing to do now.
    assert_eq!(host.resume_session(setup.uuid()), Ok(Outcome::Unchanged));
    assert_eq!(setup.state(), State::Suspended);
    assert_eq!(host.resume_session("other"), Err(Refusal::NotOwner));
    // The same device resumes while its stream still runs: superseded.
    let setup = Setup::new("");
    let session = setup.streaming();
    assert_eq!(setup.host().resume_session("other"), Err(Refusal::NotOwner));
    assert!(!session.stopping());
    assert_eq!(setup.host().resume_session(setup.uuid()), Ok(Outcome::Done));
    assert!(session.stopping());
    assert_eq!(setup.state(), State::Suspended);
    assert!(setup.game_running());
}

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
    assert_eq!(host.end_session(), Ok(Outcome::Unchanged));
    assert_eq!(host.close_game(setup.uuid()), Ok(Outcome::Unchanged));
    assert_eq!(host.disconnect_session(), Ok(Outcome::Unchanged));
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
    setup.host().stop_app();
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
        let _ = h.end_session();
        let _ = h.close_game(setup.uuid());
        h.stop_app();
        crate::runtime::release_displays(&h);
        h.reap_paused_display();
        h.session_tick();
        let _ = h.disconnect_session();
    }
    let snapshot = h.session_snapshot();
    assert_eq!(snapshot.state, State::Idle);
    assert!(!snapshot.game_running);
    assert_eq!(setup.displays(), 0);
    assert_eq!(setup.journal(), None);
    // The host is still usable afterwards.
    let _session = setup.streaming();
    assert_eq!(h.end_session(), Ok(Outcome::Done));
    assert_eq!(h.session_snapshot().state, State::Idle);
}

#[test]
fn the_actions_the_view_allows_are_the_actions_the_host_accepts() {
    use butterpollo_core::{native::session_view, native_api::session::ActionId};
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
                ActionId::Disconnect => host.disconnect_session(),
                ActionId::End => host.end_session(),
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
    assert_eq!(host.end_session(), Err(Refusal::NoSession));
    assert_eq!(host.session_snapshot().state, State::Idle);
    assert!(!directory.join(JOURNAL).exists());
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
    setup.host().stop_app();
    setup.host().session_shutdown();
    assert_eq!(setup.journal(), None);
    assert_eq!(setup.host().recover_session(), None);
    assert_eq!(setup.state(), State::Idle);
}
