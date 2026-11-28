//! Unit tests for WebRTC session bridge, monotonic timestamps and audio layout selection.

use super::live::audio_layout;
use super::*;
use butterpollo_media_policy::{audio::OpusLayout, rtsp::webrtc_audio_kbps};
use butterpollo_session::session::Role;
use butterpollo_stream::state::test_support::Fixture;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

#[test]
fn the_audio_layout_is_the_answered_one_at_the_bitrate_the_video_budget_paid_for() {
    let answered = |channels| {
        let (streams, coupled, mapping): (i32, i32, &[u8]) = match channels {
            6 => (4, 2, &[0, 4, 1, 2, 3, 5]),
            _ => (5, 3, &[0, 6, 1, 2, 3, 4, 5, 7]),
        };
        OpusLayout {
            channels: usize::from(channels),
            streams,
            coupled,
            mapping: mapping.to_vec(),
            bitrate: 1,
        }
    };
    // Stereo ignores the answer; the surround layouts keep theirs.
    let stereo = audio_layout(2, None).unwrap();
    assert_eq!((stereo.channels, stereo.bitrate), (2, 128_000));
    for channels in [6u8, 8] {
        let layout = audio_layout(channels, Some(answered(channels))).unwrap();
        let want = answered(channels);
        assert_eq!(
            (layout.streams, layout.coupled, &layout.mapping),
            (want.streams, want.coupled, &want.mapping)
        );
        // GameStream's surround bitrates, and what `Negotiated::webrtc` subtracts.
        assert_eq!(
            layout.bitrate,
            [256_000, 450_000][usize::from(channels / 8)]
        );
        assert_eq!(layout.bitrate, webrtc_audio_kbps(channels) as i32 * 1000);
        assert_eq!(
            layout.bitrate,
            OpusLayout::select(usize::from(channels), false, None)
                .unwrap()
                .bitrate
        );
        // No answered layout, or one for another channel count, is refused.
        assert!(audio_layout(channels, None).is_err());
        assert!(audio_layout(channels, Some(answered(if channels == 6 { 8 } else { 6 }))).is_err());
    }
    assert!(audio_layout(4, None).is_err());
}

fn bridge() -> (Arc<Bridge>, UnboundedReceiver<Event>) {
    let (events, received) = unbounded_channel();
    (Bridge::new(events), received)
}

fn session(f: &Fixture) -> Arc<Session> {
    let session = f.session(f.client(u32::MAX), Role::Stream);
    // A new session starts with its first keyframe pending.
    assert!(session.idr.swap(false, Ordering::AcqRel));
    session
}

fn test_media(f: &Fixture) -> Arc<butterpollo_stream::stream::Media> {
    (25000u16..55000)
        .step_by(20)
        .find_map(|base| {
            *f.host.config.write().unwrap() = butterpollo_config::config::Config::parse(&format!(
                "port = {base}\nstream_mic = false\n"
            ))
            .unwrap();
            butterpollo_stream::stream::Media::new(f.host.clone(), "127.0.0.1".parse().unwrap())
                .ok()
        })
        .expect("available ephemeral base port for Media::new")
}

fn requested(session: &Session) -> (bool, u64) {
    (
        session.idr.load(Ordering::Acquire),
        session.stats.idr_requests.load(Ordering::Acquire),
    )
}

#[test]
fn session_ids_are_positive_and_never_repeat() {
    let links = butterpollo_stream::webrtc_bridge::WebRtcLinks::default();
    let ids: Vec<u64> = (0..4).map(|_| links.next_session_id()).collect();
    assert_eq!(ids, [1, 2, 3, 4]);
    // A launch nobody put a transport for has nothing to take or discard.
    assert!(links.take("nothing").is_none());
    links.discard("nothing");
    assert_eq!(links.len(), 0);
}

#[test]
fn capture_instants_never_repeat_or_step_back() {
    let last = Cell::new(None);
    let base = Instant::now();
    let first = monotonic(&last, base);
    assert_eq!(first, base);
    // The same instant, and an earlier one, each move on by at least a tick.
    let repeated = monotonic(&last, base);
    assert!(repeated >= first + MIN_STEP);
    let earlier = monotonic(&last, base - Duration::from_millis(5));
    assert!(earlier >= repeated + MIN_STEP);
    // A later one is kept as it is.
    let later = base + Duration::from_secs(1);
    assert_eq!(monotonic(&last, later), later);
}

#[test]
fn a_pli_or_fir_is_the_clients_keyframe_request_and_the_rest_are_the_hosts() {
    let f = Fixture::new();
    let session = session(&f);
    let (bridge, _events) = bridge();
    bridge.bind(&session);
    assert_eq!(requested(&session), (false, 0));
    for (n, reason) in [IdrReason::Pli, IdrReason::Fir].into_iter().enumerate() {
        session.idr.store(false, Ordering::Release);
        bridge.idr(reason);
        assert_eq!(requested(&session), (true, n as u64 + 1), "{reason:?}");
    }
    let before = requested(&session).1;
    for reason in [
        IdrReason::Connected,
        IdrReason::Reconnected,
        IdrReason::QueueFull,
        IdrReason::PacerBacklog,
        IdrReason::WaitingForKeyframe,
        IdrReason::WriteFailed,
    ] {
        session.idr.store(false, Ordering::Release);
        bridge.idr(reason);
        assert!(session.idr.load(Ordering::Acquire), "{reason:?}");
    }
    assert_eq!(requested(&session).1, before + 6);
}

#[test]
fn a_request_before_the_session_exists_is_ignored() {
    let f = Fixture::new();
    let session = session(&f);
    let (bridge, _events) = bridge();
    bridge.idr(IdrReason::Pli);
    bridge.event(TransportEvent::Control {
        data: br#"{"type":"request-keyframe"}"#.to_vec(),
        binary: false,
    });
    assert_eq!(requested(&session), (false, 0));
}

#[test]
fn the_control_channels_request_keyframe_asks_for_one_and_other_text_does_not() {
    let f = Fixture::new();
    let session = session(&f);
    let (bridge, _events) = bridge();
    bridge.bind(&session);
    for text in [&br#"{"type":"idr"}"#[..], b"not json", b"{}"] {
        bridge.event(TransportEvent::Control {
            data: text.to_vec(),
            binary: false,
        });
    }
    // Binary frames on the control channel are not the JSON protocol.
    bridge.event(TransportEvent::Control {
        data: br#"{"type":"request-keyframe"}"#.to_vec(),
        binary: true,
    });
    assert_eq!(requested(&session), (false, 0));
    bridge.event(TransportEvent::Control {
        data: br#"{"type":"request-keyframe"}"#.to_vec(),
        binary: false,
    });
    assert_eq!(requested(&session), (true, 1));
}

#[test]
fn pad_announcements_on_the_control_channel_reach_the_gamepad_worker_and_state_does_not() {
    let f = Fixture::new();
    let session = session(&f);
    let (bridge, _events) = bridge();
    bridge.bind(&session);
    let (input, mut received) = InputHandle::probe();
    bridge.attach_input(input);
    let connected = br#"{"v":2,"type":"gamepad-connected","controllerId":0}"#;
    let gone = br#"{"v":2,"type":"gamepad-disconnected","controllerId":0}"#;
    let state = br#"{"v":2,"type":"gamepad-state","controllerId":0}"#;
    for (data, binary) in [
        (&connected[..], false),
        (&state[..], false),
        (&gone[..], false),
        (&connected[..], true),
    ] {
        bridge.event(TransportEvent::Control {
            data: data.to_vec(),
            binary,
        });
    }
    let texts = received();
    assert_eq!(
        texts,
        [
            String::from_utf8_lossy(connected).into_owned(),
            String::from_utf8_lossy(gone).into_owned()
        ]
    );
    assert_eq!(requested(&session), (false, 0));
}

#[test]
fn a_transport_that_closes_ends_the_session_even_before_it_is_bound() {
    let f = Fixture::new();
    let (bound, _events) = bridge();
    let session = session(&f);
    bound.bind(&session);
    bound.event(TransportEvent::Closed);
    assert!(session.stopping());

    // Closed first: the session is stopped the moment it is bound.
    let (early, _events) = bridge();
    let late = Fixture::new();
    let session = self::session(&late);
    early.event(TransportEvent::Closed);
    assert!(!session.stopping());
    early.bind(&session);
    assert!(session.stopping());
}

#[test]
fn a_transport_that_connected_before_the_session_gets_its_keyframe_when_bound() {
    let f = Fixture::new();
    let session = session(&f);
    let (bridge, mut events) = bridge();
    bridge.event(TransportEvent::Connected);
    assert!(matches!(events.try_recv(), Ok(Event::Connected)));
    assert_eq!(requested(&session), (false, 0));
    bridge.bind(&session);
    assert_eq!(requested(&session), (true, 1));
    // A path that drops and returns changes nothing by itself.
    bridge.event(TransportEvent::Disconnected);
    bridge.event(TransportEvent::Reconnected);
    assert!(!session.stopping());
}

#[test]
fn commands_inbox_is_bounded_without_blocking() {
    let (commands, mut rx) = Commands::test_channel(COMMAND_CAP);
    for _ in 0..COMMAND_CAP {
        assert!(commands.send(Command::Stop));
    }
    assert!(!commands.send(Command::Stop));
    assert!(matches!(rx.try_recv(), Ok(Command::Stop)));
    assert!(commands.send(Command::Stop));
}

#[tokio::test]
async fn switch_session_is_refused_when_another_device_owns_remote_session() {
    use butterpollo_session::webrtc::protocol::{
        AudioSettings, ErrorCode, ServerMessage, VideoSettings,
    };
    let f = Fixture::new();
    let app0 = f.host.apps.read().unwrap()[0].clone();
    let other_id = app0.id() + 1;
    let mut app1 = app0.clone();
    app1.name = "Second App".into();
    app1.computed_id = Some(other_id);
    f.host.apps.write().unwrap().push(app1);

    let mut running =
        butterpollo_stream::process::RunningApp::with_environment(&app0, Default::default())
            .unwrap();
    running.owner = "owner".into();
    *f.host.current_app.lock().unwrap() = Some(running);

    let monitor = f.session(f.client(u32::MAX), Role::RemoteMonitor);
    let intruder = crate::WebRtcRuntime::for_host(&f.host)
        .clients
        .console_device(&f.host, "intruder")
        .unwrap();
    let client_id = intruder.webrtc().unwrap().0.to_owned();

    let media = test_media(&f);
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(8);
    let commands = spawn(
        f.host.clone(),
        media,
        client_id,
        "127.0.0.1:5000".parse().unwrap(),
        "127.0.0.1".parse().unwrap(),
        false,
        out_tx,
    );
    assert!(commands.send(Command::Switch(Request {
        app_id: other_id.to_string(),
        video: VideoSettings {
            width: 1920,
            height: 1080,
            fps: 60,
            codec: "hevc".into(),
            bitrate_kbps: 20_000,
            hdr: false,
        },
        audio: AudioSettings {
            channels: 2,
            sample_rate: None,
        },
    })));
    let reply = tokio::time::timeout(Duration::from_secs(2), out_rx.recv())
        .await
        .expect("timed out waiting for switch-session reply")
        .expect("actor exited without replying");
    assert!(
        matches!(
            reply,
            ServerMessage::Error {
                ref request_type,
                code: ErrorCode::SessionActive,
                ..
            } if request_type == "switch-session"
        ),
        "expected switch-session refusal, got {reply:?}"
    );
    assert!(f.host.current_app.lock().unwrap().is_some());
    assert!(!monitor.stopping());
    drop(commands);
    f.host.stop.store(true, Ordering::Release);
}

#[tokio::test]
async fn same_app_switch_allows_resume_alongside_remote_monitor_and_enforces_webrtc_exclusivity() {
    use butterpollo_session::{
        session::Transport,
        webrtc::protocol::{
            AudioSettings, ErrorCode, HostSessionState, ServerMessage, VideoSettings,
        },
    };
    let f = Fixture::new();
    let app0 = f.host.apps.read().unwrap()[0].clone();
    let app_id = app0.id();
    let mut running =
        butterpollo_stream::process::RunningApp::with_environment(&app0, Default::default())
            .unwrap();
    running.owner = "owner".into();
    *f.host.current_app.lock().unwrap() = Some(running);

    let mut owner_client = f.client(u32::MAX);
    owner_client.uuid = "owner".into();
    let monitor = f.session(owner_client.clone(), Role::RemoteMonitor);
    let mut webrtc_launch = f.launch(owner_client, Role::Stream);
    webrtc_launch.transport = Transport::WebRtc;
    let webrtc_stream = butterpollo_stream::state::Session::new(webrtc_launch, Default::default());
    f.host
        .sessions
        .lock()
        .unwrap()
        .active
        .insert(webrtc_stream.launch.id.clone(), webrtc_stream.clone());

    let intruder = crate::WebRtcRuntime::for_host(&f.host)
        .clients
        .console_device(&f.host, "intruder")
        .unwrap();
    let client_id = intruder.webrtc().unwrap().0.to_owned();
    let media = test_media(&f);
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let commands = spawn(
        f.host.clone(),
        media,
        client_id,
        "127.0.0.1:5000".parse().unwrap(),
        "127.0.0.1".parse().unwrap(),
        false,
        out_tx,
    );
    let req = move || Request {
        app_id: app_id.to_string(),
        video: VideoSettings {
            width: 1920,
            height: 1080,
            fps: 60,
            codec: "hevc".into(),
            bitrate_kbps: 20_000,
            hdr: false,
        },
        audio: AudioSettings {
            channels: 2,
            sample_rate: None,
        },
    };

    // 1. Active WebRTC stream owned by another device still blocks same-app takeover.
    assert!(commands.send(Command::Switch(req())));
    let reply = tokio::time::timeout(Duration::from_secs(2), out_rx.recv())
        .await
        .expect("timed out waiting for webrtc exclusivity refusal")
        .expect("actor exited");
    assert!(
        matches!(
            reply,
            ServerMessage::Error {
                ref request_type,
                code: ErrorCode::SessionActive,
                ..
            } if request_type == "switch-session"
        ),
        "expected SessionActive on WebRTC held_by_other, got {reply:?}"
    );
    assert!(!webrtc_stream.stopping());

    // 2. Remove the WebRTC stream while keeping `RemoteMonitor` alive; non-terminating resume succeeds.
    f.host
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&webrtc_stream.launch.id);
    assert!(commands.send(Command::Switch(req())));
    let first = tokio::time::timeout(Duration::from_secs(2), out_rx.recv())
        .await
        .expect("timed out waiting for resuming state")
        .expect("actor exited");
    assert!(
        matches!(
            first,
            ServerMessage::HostSessionStatus {
                state: HostSessionState::Resuming,
                ..
            }
        ),
        "expected Resuming status, got {first:?}"
    );
    let second = tokio::time::timeout(Duration::from_secs(2), out_rx.recv())
        .await
        .expect("timed out waiting for starting state after stop_streams_for")
        .expect("actor exited");
    assert!(
        matches!(
            second,
            ServerMessage::HostSessionStatus {
                state: HostSessionState::Starting,
                ..
            }
        ),
        "same-app switch must proceed past stop_streams_for instead of returning NotOwner: {second:?}"
    );
    assert!(!monitor.stopping());
    assert!(f.host.current_app.lock().unwrap().is_some());
    assert!(commands.send(Command::Stop));
    drop(commands);
    f.host.stop.store(true, Ordering::Release);
}

#[tokio::test]
async fn late_stop_streams_refusal_emits_failed_host_session_status() {
    use butterpollo_session::{
        session::Transport,
        webrtc::protocol::{AudioSettings, HostSessionState, ServerMessage, VideoSettings},
    };
    let f = Fixture::new();
    let app0 = f.host.apps.read().unwrap()[0].clone();
    let app_id = app0.id();
    let mut other_device = f.client(u32::MAX);
    other_device.uuid = "other-device".into();
    // Start a running app owned by the caller so the early terminate_refused check passes.
    let caller = crate::WebRtcRuntime::for_host(&f.host)
        .clients
        .console_device(&f.host, "caller")
        .unwrap();
    let client_id = caller.webrtc().unwrap().0.to_owned();
    let mut running =
        butterpollo_stream::process::RunningApp::with_environment(&app0, Default::default())
            .unwrap();
    running.owner = caller.uuid.clone();
    *f.host.current_app.lock().unwrap() = Some(running);
    let media = test_media(&f);
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let commands = spawn(
        f.host.clone(),
        media,
        client_id,
        "127.0.0.1:5000".parse().unwrap(),
        "127.0.0.1".parse().unwrap(),
        false,
        out_tx,
    );
    let req = Request {
        app_id: app_id.to_string(),
        video: VideoSettings {
            width: 1920,
            height: 1080,
            fps: 60,
            codec: "hevc".into(),
            bitrate_kbps: 20_000,
            hdr: false,
        },
        audio: AudioSettings {
            channels: 2,
            sample_rate: None,
        },
    };

    // 1. Late refusal during `switch` (after Resuming is already announced) must emit Failed.
    let guard = f.host.launch_transition.lock().unwrap();
    assert!(commands.send(Command::Switch(req)));
    let announced = tokio::time::timeout(Duration::from_secs(2), out_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        announced,
        ServerMessage::HostSessionStatus {
            state: HostSessionState::Resuming,
            ..
        }
    ));
    let mut webrtc_launch = f.launch(other_device.clone(), Role::Stream);
    webrtc_launch.transport = Transport::WebRtc;
    let other_stream = butterpollo_stream::state::Session::new(webrtc_launch, Default::default());
    f.host
        .sessions
        .lock()
        .unwrap()
        .active
        .insert(other_stream.launch.id.clone(), other_stream.clone());
    drop(guard);
    let failed = tokio::time::timeout(Duration::from_secs(2), out_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            failed,
            ServerMessage::HostSessionStatus {
                state: HostSessionState::Failed,
                message: Some(ref msg),
                ..
            } if msg.contains("Another device")
        ),
        "late switch refusal must emit HostSessionState::Failed, got {failed:?}"
    );
    f.host
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&other_stream.launch.id);

    // 2. Late refusal during `stop-host-session` (after Stopping is announced) must emit Failed.
    let guard = f.host.launch_transition.lock().unwrap();
    assert!(commands.send(Command::StopHost));
    let announced = tokio::time::timeout(Duration::from_secs(2), out_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        announced,
        ServerMessage::HostSessionStatus {
            state: HostSessionState::Stopping,
            ..
        }
    ));
    f.host.current_app.lock().unwrap().as_mut().unwrap().owner = other_device.uuid.clone();
    drop(guard);
    let failed = tokio::time::timeout(Duration::from_secs(2), out_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            failed,
            ServerMessage::HostSessionStatus {
                state: HostSessionState::Failed,
                message: Some(ref msg),
                ..
            } if msg.contains("Another device")
        ),
        "late stop-host refusal must emit HostSessionState::Failed, got {failed:?}"
    );
    assert!(commands.send(Command::Stop));
    drop(commands);
    f.host.stop.store(true, Ordering::Release);
}
