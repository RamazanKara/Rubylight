use super::stream_key_id;
#[test]
fn only_rl_second_screen_1_marks_a_request_of_the_second_screen() {
    for (query, second) in [
        (None, false),
        (Some(""), false),
        (Some("bitrate=1"), false),
        (Some("rlSecondScreen=0"), false),
        (Some("rlSecondScreen=true"), false),
        (Some("xrlSecondScreen=1"), false),
        (Some("rlSecondScreen=1"), true),
        (Some("appid=1&rlSecondScreen=1&rikeyid=2"), true),
        (Some("rlSecondScreen=%31"), true),
    ] {
        assert_eq!(super::second_screen_query(query), second, "{query:?}");
    }
}
fn running_app(f: &crate::state::test_support::Fixture, owner: &str) {
    let app = f.host.apps.read().unwrap()[0].clone();
    let mut running =
        crate::process::RunningApp::with_environment(&app, Default::default()).unwrap();
    running.owner = owner.into();
    *f.host.current_app.lock().unwrap() = Some(running);
}
#[test]
fn a_launch_that_replaces_the_game_logs_the_stop_and_who_launched() {
    use crate::state::test_support::Fixture;
    use butterpollo_core::{remote::Owner, session::Role};
    let f = Fixture::new();
    running_app(&f, "owner");
    let mut launch = f.launch(f.client(u32::MAX), Role::Stream);
    launch.app_id = f.host.current_app.lock().unwrap().as_ref().unwrap().id + 1;
    let path = f.host.directory.join("replace.log");
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .without_time()
        .with_ansi(false)
        .with_target(false)
        .with_writer(std::sync::Mutex::new(std::fs::File::create(&path).unwrap()))
        .finish();
    crate::test_log::capture_during(subscriber, || {
        // The host asks first: nothing stops yet.
        assert!(super::replace_launch_app(&f.host, &launch, false, Owner::None).is_err());
        assert!(f.host.current_app.lock().unwrap().is_some());
        // The same launch again confirms it.
        super::replace_launch_app(&f.host, &launch, false, Owner::None).unwrap();
    });
    assert!(f.host.current_app.lock().unwrap().is_none());
    let log = std::fs::read_to_string(path).unwrap();
    let stops: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("stopping the application"))
        .collect();
    assert_eq!(stops.len(), 1, "{log}");
    for field in ["reason=\"replaced\"", "by=\"parity-device\"", "owner=owner"] {
        assert!(stops[0].contains(field), "{field} missing: {log}");
    }
}
#[test]
fn a_client_that_sends_the_confirmation_replaces_only_on_that_request() {
    use crate::state::test_support::Fixture;
    use butterpollo_core::{remote::Owner, session::Role};
    let f = Fixture::new();
    running_app(&f, "owner");
    let mut launch = f.launch(f.client(u32::MAX), Role::Stream);
    let (id, generation) = f
        .host
        .current_app
        .lock()
        .unwrap()
        .as_ref()
        .map(|app| (app.id, app.generation.clone()))
        .unwrap();
    launch.app_id = id + 1;
    launch.options.insert("rlConfirmReplace".into(), "0".into());
    let asking = || {
        f.host.confirmations.lock().unwrap().active(
            &launch.client.uuid,
            butterpollo_core::remote::Confirmation::Replace,
            &generation,
        )
    };
    // A remembered launch or a retry after a cancelled prompt never
    // redeems the prompt, however often it repeats within 60 seconds.
    for _ in 0..3 {
        let error = super::replace_launch_app(&f.host, &launch, false, Owner::None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("already running"), "{error}");
        assert!(f.host.current_app.lock().unwrap().is_some());
        // Serverinfo still shows the game while the client asks.
        assert!(asking());
    }
    launch.options.insert("rlConfirmReplace".into(), "1".into());
    super::replace_launch_app(&f.host, &launch, false, Owner::None).unwrap();
    assert!(f.host.current_app.lock().unwrap().is_none());
    assert!(!asking());
}
#[test]
fn a_confirmation_closes_only_the_game_its_prompt_named() {
    use crate::state::test_support::Fixture;
    use butterpollo_core::{remote::Owner, session::Role};
    let f = Fixture::new();
    running_app(&f, "owner");
    let mut launch = f.launch(f.client(u32::MAX), Role::Stream);
    launch.app_id = f.host.current_app.lock().unwrap().as_ref().unwrap().id + 1;
    launch.options.insert("rlConfirmReplace".into(), "1".into());
    let refused = |launch: &crate::state::Launch| {
        let error = super::replace_launch_app(&f.host, launch, false, Owner::None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("already running"), "{error}");
        assert!(f.host.current_app.lock().unwrap().is_some());
    };
    // A confirmation the host never asked for closes nothing; it asks.
    refused(&launch);
    // The prompt named a game that ended; another device started the
    // one running now, which nobody agreed to close.
    f.host
        .current_app
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .generation = "started-after-the-prompt".into();
    refused(&launch);
    // The answer to the prompt for the game running now closes it.
    super::replace_launch_app(&f.host, &launch, false, Owner::None).unwrap();
    assert!(f.host.current_app.lock().unwrap().is_none());
}
#[test]
fn stopping_streams_does_not_wait_for_remote_sessions_it_leaves_running() {
    use super::{Duration, Instant, StopStreams, stop_streams};
    use crate::state::test_support::Fixture;
    use butterpollo_core::session::Role;
    let f = Fixture::new();
    let client = f.client(u32::MAX);
    let monitor = f.session(client.clone(), Role::RemoteMonitor);
    let input = f.session(client, Role::InputOnly);
    for terminate in [None, Some("anyone")] {
        let started = Instant::now();
        assert_eq!(stop_streams(&f.host, terminate), StopStreams::Stopped);
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(!monitor.stopping() && !input.stopping());
    }
}
#[test]
fn stopping_streams_ends_a_stream_and_waits_for_it_to_leave() {
    use super::{Duration, StopStreams, stop_streams};
    use crate::state::test_support::Fixture;
    use butterpollo_core::session::Role;
    let f = Fixture::new();
    let stream = f.session(f.client(u32::MAX), Role::Stream);
    let host = f.host.clone();
    let id = stream.launch.id.clone();
    // The worker moves out of `active` once it sees the stop.
    let worker = std::thread::spawn({
        let stream = stream.clone();
        move || {
            while !stream.stopping() {
                std::thread::sleep(Duration::from_millis(2));
            }
            host.sessions.lock().unwrap().active.remove(&id);
        }
    });
    assert_eq!(stop_streams(&f.host, None), StopStreams::Stopped);
    assert!(stream.stopping());
    worker.join().unwrap();
}
#[test]
fn only_the_games_owner_may_terminate_it_while_remote_sessions_are_active() {
    use super::{StopStreams, stop_streams, terminate_refused};
    use crate::state::test_support::Fixture;
    use butterpollo_core::session::Role;
    let f = Fixture::new();
    // No game, nothing to refuse.
    assert!(!terminate_refused(&f.host, "other"));
    running_app(&f, "owner");
    assert!(!terminate_refused(&f.host, "other"));
    let monitor = f.session(f.client(u32::MAX), Role::RemoteMonitor);
    assert!(terminate_refused(&f.host, "other"));
    assert!(!terminate_refused(&f.host, "owner"));
    // A refused stop changes nothing.
    assert_eq!(stop_streams(&f.host, Some("other")), StopStreams::Refused);
    assert!(f.host.current_app.lock().unwrap().is_some());
    assert!(!monitor.stopping());
    // Ending the remote session lifts the guard.
    monitor.stop();
    assert!(!terminate_refused(&f.host, "other"));
    assert_eq!(stop_streams(&f.host, Some("other")), StopStreams::Stopped);
    assert!(f.host.current_app.lock().unwrap().is_none());
}
#[test]
fn a_failed_app_lookup_keeps_its_own_status_and_only_an_unknown_app_is_a_404() {
    use super::{APPLICATION_NOT_FOUND, LaunchFailure, launch_app_error};
    let status = |error: anyhow::Error| {
        error
            .downcast_ref::<LaunchFailure>()
            .map(|failure| failure.0)
    };
    assert_eq!(
        status(launch_app_error(anyhow::anyhow!(APPLICATION_NOT_FOUND))),
        Some(404)
    );
    assert_eq!(
        status(launch_app_error(LaunchFailure(503, "busy").into())),
        Some(503)
    );
    let other = launch_app_error(anyhow::anyhow!("remote input is disabled"));
    assert_eq!(other.to_string(), "remote input is disabled");
    assert!(other.downcast_ref::<LaunchFailure>().is_none());
}
#[test]
fn a_launch_stops_the_watch_before_it_looks_at_a_display() {
    use crate::display_session::tests::WAKE_WATCH_HOLDS;
    use crate::state::test_support::Fixture;
    use butterpollo_core::session::Role;
    let f = Fixture::new();
    let launch = f.launch(f.client(u32::MAX), Role::Stream);
    let args = super::Args::from([("mode".to_owned(), "not a mode".to_owned())]);
    WAKE_WATCH_HOLDS.with(|holds| holds.set(0));
    // Refused before any display: what a launch does first is all this reaches.
    assert!(super::prepare_launch_display(&f.host, &args, &launch).is_err());
    assert_eq!(WAKE_WATCH_HOLDS.with(std::cell::Cell::get), 1);
}
#[test]
fn a_slow_teardown_blocks_the_next_launch_after_the_wait() {
    use super::{Duration, Instant, Mutex, Sessions, wait_for_teardown};
    let sessions = Mutex::new(Sessions::default());
    sessions
        .lock()
        .unwrap()
        .teardown
        .insert("slow".into(), Instant::now() - Duration::from_secs(6));
    assert!(!wait_for_teardown(
        &sessions,
        &["slow".into()],
        Duration::ZERO
    ));
    assert!(sessions.lock().unwrap().owns_capture());
    sessions.lock().unwrap().teardown.remove("slow");
    assert!(wait_for_teardown(
        &sessions,
        &["slow".into()],
        Duration::from_secs(5)
    ));
}
#[test]
fn another_device_cannot_take_over_a_pairing_in_progress() {
    use super::another_device_pairing as blocked;
    // Nothing pending, or this device asking again (after a wrong PIN).
    assert!(!blocked(None, "mine"));
    assert!(!blocked(Some(("mine", true)), "mine"));
    // Another device whose request is still waiting or handshaking.
    assert!(blocked(Some(("theirs", true)), "mine"));
    // Another device that gave up: its stale request is replaced.
    assert!(!blocked(Some(("theirs", false)), "mine"));
}

#[test]
fn android_signed_stream_keys_preserve_the_wire_iv() {
    for (signed, unsigned) in [
        ("0", "0"),
        ("2147483647", "2147483647"),
        ("-2147483648", "2147483648"),
        ("-1", "4294967295"),
    ] {
        assert_eq!(
            stream_key_id(signed).unwrap().to_be_bytes(),
            stream_key_id(unsigned).unwrap().to_be_bytes()
        );
    }
    for invalid in ["", "-2147483649", "4294967296", "1.5", "0x123", "key"] {
        assert!(stream_key_id(invalid).is_err(), "{invalid}");
    }
}

mod properties {
    use super::stream_key_id;
    use proptest::prelude::*;

    proptest! {
        /// `rikeyid` is any 32-bit decimal, signed or unsigned, and nothing else.
        #[test]
        fn stream_key_ids_are_32_bit_decimals(value in r"[+-]?[0-9]{0,11}|\PC{0,12}") {
            let wide = value.parse::<i64>().ok().filter(|n| (i64::from(i32::MIN)..=i64::from(u32::MAX)).contains(n));
            prop_assert_eq!(stream_key_id(&value).ok(), wide.map(|n| n as u32));
        }

        #[test]
        fn signed_and_unsigned_ids_agree(id in any::<u32>()) {
            prop_assert_eq!(stream_key_id(&id.to_string()).unwrap(), id);
            prop_assert_eq!(stream_key_id(&(id as i32).to_string()).unwrap(), id);
        }
    }
}
