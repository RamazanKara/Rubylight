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
    use butterpollo_session::{remote::Owner, session::Role};
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
    use butterpollo_session::{remote::Owner, session::Role};
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
            butterpollo_session::remote::Confirmation::Replace,
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
    use butterpollo_session::{remote::Owner, session::Role};
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
    use butterpollo_session::session::Role;
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
    use butterpollo_session::session::Role;
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
    use butterpollo_session::session::Role;
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
fn non_terminating_switch_enforces_owner_webrtc_exclusivity_and_preserves_nucleus_streams() {
    use super::{StopStreams, stop_streams_for};
    use crate::state::test_support::Fixture;
    use butterpollo_session::session::{Role, Seat, Transport};
    let f = Fixture::new();
    running_app(&f, "owner");

    // Terminating switch (`stop_app = true`) is refused when `RemoteMonitor` is active and owned by another device.
    let mut owner_client = f.client(u32::MAX);
    owner_client.uuid = "owner".into();
    let owner_stream = f.session(owner_client.clone(), Role::Stream);
    let monitor = f.session(owner_client.clone(), Role::RemoteMonitor);
    assert_eq!(
        stop_streams_for(&f.host, "other", true),
        StopStreams::Refused
    );
    assert!(!owner_stream.stopping());
    assert!(!monitor.stopping());

    // Non-terminating resume/takeover (`stop_app = false`) is allowed despite `RemoteMonitor`
    // when no active WebRTC `Role::Stream` holds exclusivity, leaving the app and `RemoteMonitor` running.
    let host = f.host.clone();
    let owner_id = owner_stream.launch.id.clone();
    let owner_worker = std::thread::spawn({
        let s = owner_stream.clone();
        move || {
            while !s.stopping() {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            host.sessions.lock().unwrap().active.remove(&owner_id);
        }
    });
    assert_eq!(
        stop_streams_for(&f.host, "other", false),
        StopStreams::Stopped
    );
    owner_worker.join().unwrap();
    assert!(owner_stream.stopping());
    assert!(!monitor.stopping());
    assert!(f.host.current_app.lock().unwrap().is_some());
    f.host
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&monitor.launch.id);

    // Active WebRTC stream owned by `owner` blocks another device's non-terminating switch.
    let mut webrtc_launch = f.launch(owner_client.clone(), Role::Stream);
    webrtc_launch.transport = Transport::WebRtc;
    let webrtc_stream = crate::state::Session::new(webrtc_launch, Default::default());
    f.host
        .sessions
        .lock()
        .unwrap()
        .active
        .insert(webrtc_stream.launch.id.clone(), webrtc_stream.clone());
    assert_eq!(
        stop_streams_for(&f.host, "other", false),
        StopStreams::Refused
    );
    assert!(!webrtc_stream.stopping());

    // A concurrent Nucleus stream is preserved when `owner` switches or stops host streams.
    let mut coop_client = f.client(u32::MAX);
    coop_client.uuid = "coop-player".into();
    let mut coop_launch = f.launch(coop_client, Role::Stream);
    coop_launch.nucleus = Some(Seat {
        session: 1,
        slot: 0,
    });
    let coop_stream = crate::state::Session::new(coop_launch, Default::default());
    f.host
        .sessions
        .lock()
        .unwrap()
        .active
        .insert(coop_stream.launch.id.clone(), coop_stream.clone());
    let host = f.host.clone();
    let id = webrtc_stream.launch.id.clone();
    let worker = std::thread::spawn({
        let s = webrtc_stream.clone();
        move || {
            while !s.stopping() {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            host.sessions.lock().unwrap().active.remove(&id);
        }
    });
    assert_eq!(
        stop_streams_for(&f.host, "owner", false),
        StopStreams::Stopped
    );
    worker.join().unwrap();
    assert!(webrtc_stream.stopping());
    assert!(!coop_stream.stopping());
    assert!(f.host.current_app.lock().unwrap().is_some());
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
    use butterpollo_session::session::Role;
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

#[test]
fn refused_switch_does_not_cancel_callers_launch_and_end_device_stops_launch() {
    use crate::state::test_support::Fixture;
    use butterpollo_session::session::{Role, Seat, Transport};
    let f = Fixture::new();
    running_app(&f, "owner");

    // Another device holds an active WebRTC stream.
    let mut owner_client = f.client(u32::MAX);
    owner_client.uuid = "owner".into();
    let mut webrtc_launch = f.launch(owner_client.clone(), Role::Stream);
    webrtc_launch.transport = Transport::WebRtc;
    let webrtc_stream = crate::state::Session::new(webrtc_launch, Default::default());
    f.host
        .sessions
        .lock()
        .unwrap()
        .active
        .insert(webrtc_stream.launch.id.clone(), webrtc_stream.clone());

    // Caller `other` currently has an in-progress launch; refusing its non-terminating
    // switch must not cancel its own launch cancellation handle.
    let transition = f.host.launch_transition.lock().unwrap();
    let _started = f.host.launch_started("other");
    assert_eq!(
        super::stop_streams_for(&f.host, "other", false),
        super::StopStreams::Refused
    );
    assert!(!f.host.launch_stopped());

    // end_device stops an in-progress launch of either screen of the device.
    drop(_started);
    let _second = f
        .host
        .launch_started(&butterpollo_session::state::second_screen_uuid("other"));
    let host = f.host.clone();
    let ended = std::thread::spawn(move || {
        crate::nvhttp::end_device(&host, "other");
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !f.host.launch_stopped() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(f.host.launch_stopped());
    drop(_second);
    drop(transition);
    ended.join().unwrap();

    // Concurrent Nucleus stream is untouched when host streams stop.
    let mut coop_client = f.client(u32::MAX);
    coop_client.uuid = "coop-1".into();
    let mut coop_launch = f.launch(coop_client, Role::Stream);
    coop_launch.nucleus = Some(Seat {
        session: 7,
        slot: 0,
    });
    let coop_stream = crate::state::Session::new(coop_launch, Default::default());
    f.host
        .sessions
        .lock()
        .unwrap()
        .active
        .insert(coop_stream.launch.id.clone(), coop_stream.clone());
    let canceled = f.host.sessions.lock().unwrap().stop_host_streams();
    drop(canceled);
    assert!(webrtc_stream.stopping());
    assert!(!coop_stream.stopping());
}

#[test]
fn terminate_confirmation_and_vacated_background_share_device_identity_across_screens() {
    use crate::state::test_support::Fixture;
    use butterpollo_session::session::Role;
    let f = Fixture::new();
    running_app(&f, "owner");
    let _monitor = f.session(f.client(u32::MAX), Role::RemoteMonitor);
    let owner_second = butterpollo_session::state::second_screen_uuid("owner");
    assert!(super::same_device("owner", &owner_second));
    assert!(!super::terminate_refused(&f.host, &owner_second));
    assert!(super::quit_vacated_background(&f.host, &owner_second).is_none());
}
#[tokio::test]
async fn a_successful_launch_reports_its_setup_fallback_and_a_failed_one_never_does() {
    use crate::state::test_support::Fixture;
    use butterpollo_session::{remote::Control, session::Role};
    async fn body(response: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }
    let f = Fixture::new();
    let launch = f.launch(f.client(u32::MAX), Role::Stream);
    assert_eq!(super::launch_steps::launch_warning(&launch, None), None);
    launch.warnings.set("audio_route", "not a setup fallback");
    launch
        .warnings
        .set("display_scale", "Scale <b>lowered</b>.");
    assert_eq!(super::launch_steps::launch_warning(&launch, None), None);
    launch.warnings.set(
        "display_virtual",
        "Using a physical display & \u{202E}more.",
    );
    let warning = super::launch_steps::launch_warning(&launch, None);
    assert_eq!(
        warning,
        Some((
            "virtual_display_unavailable".into(),
            "Using a physical display & more.".into()
        ))
    );
    assert_eq!(
        super::launch_steps::launch_warning(&launch, Some(Control::Resume)),
        None
    );
    let ok = body(super::launch_response(
        Ok(("gamesession".into(), "rtsp://h:1".into(), warning.clone())),
        1,
        "gamesession",
        false,
        true,
    ))
    .await;
    assert!(ok.contains("<gamesession>1</gamesession>"), "{ok}");
    assert!(
        ok.contains("<clientWarning>virtual_display_unavailable</clientWarning><clientWarningMessage>Using a physical display &amp; more.</clientWarningMessage>"),
        "{ok}"
    );
    let failed = body(super::launch_response(
        Err(anyhow::anyhow!("no")),
        1,
        "gamesession",
        false,
        false,
    ))
    .await;
    assert!(!failed.contains("clientWarning"), "{failed}");
}
