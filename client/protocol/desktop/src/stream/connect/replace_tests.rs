use super::*;

fn launch_plan() -> LaunchPlan {
    let xml = r#"<?xml version="1.0" encoding="utf-8"?><root status_code="200"><hostname>PC</hostname><uniqueid>uid</uniqueid><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>1</PairStatus><currentgame>7</currentgame><state>SUNSHINE_SERVER_BUSY</state><RustHostVersion>3.0.0</RustHostVersion></root>"#;
    LaunchPlan {
        host_id: 1,
        record: Default::default(),
        host_name: "PC".into(),
        info: ServerInfo::parse(xml, 47989).expect("the answer parses"),
        native: None,
        listed: true,
        app: nightjar_core::http::App {
            name: "Zelda".into(),
            id: 9,
            hdr_supported: false,
            uuid: String::new(),
            host_index: None,
            art_version: String::new(),
            control: nightjar_core::http::AppControl::None,
            monitor_resume: false,
        },
        resume: false,
        join: false,
        small_stream_input_only: false,
        mic_only: false,
        settings: Settings::default(),
        identity: Arc::new(ClientIdentity::generate().expect("an identity")),
        display: Display { width: 1920, height: 1080, refresh_hz: 60.0, refresh_reported: true },
        hardware: Hardware::None,
        dmabuf: false,
        kms_planes: false,
        gpu_decoder: None,
        packed_modes: 0,
        color_decodable: (0, String::new()),
        base_decodable: color_probe::Base::software(),
        pacing: PacingMode::Immediate,
        vrr: false,
        gamepad_mask: 0,
        prepared_controllers: String::new(),
        microphone_muted: false,
        bitrate_limit_kbps: 0,
        network: Default::default(),
        pace: None,
        host_link_mbps: 0,
        recovery: None,
        video_shown: false,
        confirm_replace: false,
        display_luminance: None,
    }
}

// The review's regression: video showed, the network dropped, and the first reconnect had no
// picture yet when it dropped too. The chain carries its video, so the next attempt follows
// within the budget; a chain that never showed video still stops at once.
#[test]
fn a_reconnect_without_a_picture_yet_keeps_the_chain_going_after_a_real_drop() {
    use nightjar_core::session::error;
    let plan = launch_plan();
    let first = automatic_rejoin(&plan, AppRole::Stream, true, false, 60_000, false, true).expect("a plan");
    assert!(first.video_shown, "the drop came after video");
    // Attempt 1 connects but shows nothing before it drops: no picture of its own.
    let second = automatic_rejoin(&first, AppRole::Stream, true, false, 5_000, false, false).expect("a plan");
    assert!(second.video_shown, "still the chain of a stream that had video");
    assert!(Exit::Terminated(error::NO_VIDEO_TRAFFIC).retryable(first.video_shown));
    // Never any video: nothing to carry.
    let never = automatic_rejoin(&plan, AppRole::Stream, true, false, 5_000, false, false).expect("a plan");
    assert!(!never.video_shown);
    assert!(!Exit::Terminated(error::NO_VIDEO_TRAFFIC).retryable(never.video_shown));
}

// The review's regression: roamed is false, but the same address came back (DHCP gave it
// again) or there is no route yet. The connection resumes in place, so the stream goes on;
// only a return it cannot carry on over reconnects, with the desktop's default settings too.
#[test]
fn only_a_network_back_the_connection_cannot_carry_on_over_reconnects_at_once() {
    let defaults = Settings::default();
    assert_eq!(defaults.network_grace_ms, 3000, "below the connection's own timeout: no tolerance");
    assert!(!reconnects_on_return(NetworkChange::CarriesOn, false, &defaults, true));
    assert!(!reconnects_on_return(NetworkChange::Roamed, false, &defaults, true));
    assert!(reconnects_on_return(NetworkChange::CannotContinue, false, &defaults, true));
    // Not when it would not reconnect on its own (minimized, not a game stream, budget used).
    assert!(!reconnects_on_return(NetworkChange::CannotContinue, false, &defaults, false));
    // Mobile data only with "Continue on mobile data".
    assert!(!reconnects_on_return(NetworkChange::CannotContinue, true, &defaults, true));
    let mobile = Settings { roaming_metered: true, ..Settings::default() };
    assert!(reconnects_on_return(NetworkChange::CannotContinue, true, &mobile, true));
    // A tolerance above the timeout holds the connection and decides; so does the user.
    let tolerant = Settings { network_grace_ms: 20_000, ..Settings::default() };
    assert!(!reconnects_on_return(NetworkChange::CannotContinue, false, &tolerant, true));
    let manual = Settings { auto_reconnect: false, ..Settings::default() };
    assert!(!reconnects_on_return(NetworkChange::CannotContinue, false, &manual, true));
    // The exit it takes reconnects, once the stream had video.
    assert!(Exit::NetworkReturned.retryable(true));
    assert!(!Exit::NetworkReturned.retryable(false));
}

#[test]
fn a_vpn_route_streams_as_remote_with_small_packets() {
    assert_eq!(location_for_route(Location::Auto, true), (Location::Remote, 1024));
    assert_eq!(location_for_route(Location::Auto, false), (Location::Auto, 1392));
    // NIGHTJAR_STREAM_LOCATION decides over the route.
    assert_eq!(location_for_route(Location::Local, true), (Location::Local, 1392));
    assert_eq!(location_for_route(Location::Remote, false), (Location::Remote, 1392));
}

// A blocked media port: the stream never shows a picture and its error shows at once, not after
// five reconnects and a minute (Moonlight V+ PC 6792aeed).
#[test]
fn a_stream_that_never_showed_video_does_not_reconnect() {
    use nightjar_core::session::error;
    for code in [error::NO_VIDEO_TRAFFIC, error::NO_VIDEO_FRAME, -1, 10054] {
        assert!(!Exit::Terminated(code).retryable(false), "{code}");
        assert!(Exit::Terminated(code).retryable(true), "{code}");
    }
    assert!(!Exit::Terminated(error::GRACEFUL).retryable(true));
    assert!(!Exit::Reconnect.retryable(true), "the user's own reconnect is not an automatic one");
}

#[test]
fn a_wired_route_says_wired_with_its_speed_or_that_it_is_unknown() {
    use crate::network::{Fallback, Toward};
    // NUC Flatpak: eno1 at 1000 Mbps was logged as "wired or unknown (from the route)".
    let wired = Ok(Toward { wireless: false, link_mbps: 1000 });
    assert_eq!(route_line(wired, false, 1000), "wired (from the route), link 1000 Mbps");
    assert_eq!(route_line(Ok(Toward { wireless: false, link_mbps: 0 }), false, 0), "wired (from the route), link speed unknown");
    assert_eq!(route_line(Ok(Toward { wireless: true, link_mbps: 0 }), true, 0), "wireless (from the route), link 0 Mbps");
    assert_eq!(route_line(Err(Fallback::NotTelling), false, 0),
        "wired or unknown (from the internet connection, interface does not say), link 0 Mbps");
    assert_eq!(route_line(Err(Fallback::Route), true, 0), "wireless (from the internet connection, route lookup failed), link 0 Mbps");
}

#[test]
fn the_lost_network_notice_promises_a_reconnect_only_when_one_follows() {
    let defaults = Settings::default();
    assert_eq!(lost_notice(&defaults, true), NETWORK_LOST_RECONNECTING);
    assert_eq!(lost_notice(&defaults, false), NETWORK_LOST);
    let tolerant = Settings { network_grace_ms: 20_000, ..Settings::default() };
    assert_eq!(lost_notice(&tolerant, true), NETWORK_LOST);
}

#[test]
fn a_network_return_rejoins_without_the_first_delay_and_only_on_its_own() {
    let mut plan = launch_plan();
    plan.app.control = nightjar_core::http::AppControl::None;
    let now = recovery::now_ms();
    let mut rejoined = automatic_rejoin(&plan, AppRole::Stream, true, false, 0, true, true).expect("a plan");
    assert!(rejoined.resume);
    let budget = rejoined.recovery.as_mut().expect("a budget");
    assert!(budget.allows_attempt(now));
    assert_eq!(budget.next_delay(now), Some(0), "no delay, but it spends an attempt");
    assert_eq!(budget.attempts(), 1);
    // Without retry_now the usual first delay.
    let mut usual = automatic_rejoin(&plan, AppRole::Stream, true, false, 0, false, true).unwrap().recovery.unwrap();
    assert_eq!(usual.next_delay(now), Some(1000));
    // Minimized, off, or a stream that is not a game: the user decides.
    assert!(automatic_rejoin(&plan, AppRole::Stream, true, true, 0, true, true).is_none());
    assert!(automatic_rejoin(&plan, AppRole::Stream, false, false, 0, true, true).is_none());
    assert!(automatic_rejoin(&plan, AppRole::RemoteMonitor, true, false, 0, true, true).is_none());
    // A used-up budget allows no attempt, so the notice promises none.
    let mut spent = recovery::Backoff::default();
    while spent.next_delay(now).is_some() {}
    plan.recovery = Some(spent);
    let rejoined = automatic_rejoin(&plan, AppRole::Stream, true, false, 0, true, true).unwrap();
    assert!(!rejoined.recovery.unwrap().allows_attempt(now));
}

#[test]
fn a_refused_join_says_what_happened_in_our_words() {
    let gone = "no application or remote monitor to resume";
    assert_eq!(resume_refused(true, 503, gone).as_deref(), Some(APP_GONE));
    assert_eq!(resume_refused(false, 503, gone), None, "a resume keeps the PC's words and its recovery");
    assert_eq!(resume_refused(true, 409, "The game running on this PC changed").as_deref(), Some(APP_GONE));
    let browser = resume_refused(true, 409, "Another device is streaming over WebRTC").expect("our words");
    assert!(browser.contains("browser"), "{browser}");
    assert_eq!(resume_refused(true, 503, "Encoder failure"), None);
}

#[test]
fn a_join_goes_to_the_pc_whatever_it_hides_and_a_plain_resume_does_not() {
    use nightjar_core::http::{Error, StreamMove};
    // The PC reports no game to a device that does not own it, and its note still says moved.
    check_resume(true, true, false, true, || panic!("a join asks nothing")).unwrap_or_else(|e| panic!("{}", e.message));
    // Unpaired stays refused.
    let unpaired = check_resume(true, false, false, true, || panic!("not asked")).expect_err("refused");
    assert!(unpaired.message.contains("no longer running"), "{}", unpaired.message);

    let moved: StreamMove = serde_json::from_str(r#"{"moved_to":{"device":"TV","since_unix_ms":1}}"#).unwrap();
    let refused = check_resume(false, true, false, true, || Ok(moved)).expect_err("the move");
    assert_eq!(refused.moved_to.as_deref(), Some("TV"));
    let gone = check_resume(false, true, false, true, || Ok(StreamMove::default())).expect_err("no game");
    assert!(gone.message.contains("no longer running"), "{}", gone.message);
    let unanswered = check_resume(false, true, false, true, || Err(Error::Cancelled)).expect_err("no verdict");
    assert!(unanswered.retryable);
    assert!(check_resume(false, true, true, true, || panic!("not asked")).is_ok());
}

#[test]
fn a_reconnect_of_a_join_never_joins() {
    let mut plan = launch_plan();
    plan.resume = true;
    plan.join = true;
    plan.app.control = nightjar_core::http::AppControl::None;
    assert!(!rejoin(plan.clone(), AppRole::Stream).expect("a plan").join);
    assert!(!automatic_rejoin(&plan, AppRole::Stream, true, false, 0, false, true).expect("a plan").join);
}

#[test]
fn only_the_launch_the_user_answered_confirms_the_replace() {
    use nightjar_core::http::Error;
    let prompt = Error::Status {
        code: 410,
        message: "An app is already running. Launch this app again within 60 seconds to confirm that you want to close it.".into(),
    };
    let mut plan = launch_plan();
    assert!(asks_replace(&plan, AppRole::Stream, &prompt));
    // Another 410, a role that replaces nothing, or a resume is no prompt to answer.
    assert!(!asks_replace(&plan, AppRole::Stream, &Error::Status { code: 410, message: "Gone".into() }));
    assert!(!asks_replace(&plan, AppRole::InputOnly, &prompt));
    // A launch that already confirmed is not asked again: the refusal is shown.
    plan.confirm_replace = true;
    assert!(!asks_replace(&plan, AppRole::Stream, &prompt));
    // A reconnect resumes and never carries the answer.
    let rejoined = rejoin(plan.clone(), AppRole::Stream).expect("a plan");
    assert!(rejoined.resume && !rejoined.confirm_replace);
    assert!(!asks_replace(&rejoined, AppRole::Stream, &prompt));
    let monitor = rejoin(plan, AppRole::RemoteMonitor).expect("a plan");
    assert!(!monitor.resume && !monitor.confirm_replace);
}

#[test]
fn microphone_only_reconnects_minimized_and_is_no_recent_game() {
    let mut plan = launch_plan();
    plan.record.unique_id = "uid".into();
    assert!(recent_game(&plan).is_some());
    plan.mic_only = true;
    assert!(recent_game(&plan).is_none(), "Microphone only is no game to continue");
    plan.mic_only = false;
    plan.listed = false;
    assert!(recent_game(&plan).is_none(), "a resume with no name to list is no game to continue");
    plan.listed = true;
    plan.mic_only = true;
    // It runs minimized or behind other windows by design.
    let rejoined = automatic_rejoin(&plan, AppRole::InputOnly, true, true, 60_000, false, false).expect("a plan");
    assert!(rejoined.mic_only && !rejoined.resume);
    assert!(automatic_rejoin(&plan, AppRole::InputOnly, false, false, 60_000, false, false).is_none(), "the setting is off");
}

#[test]
fn a_dropped_stream_reconnects_without_focus_but_not_minimized() {
    // L4 on the laptop: a system dialog held the focus, so the drop never reconnected.
    assert!(reconnects_automatically(true, false, AppRole::Stream));
    assert!(!reconnects_automatically(true, true, AppRole::Stream), "minimized or hidden waits for the user");
    assert!(!reconnects_automatically(false, false, AppRole::Stream), "the setting is off");
    assert!(!reconnects_automatically(true, false, AppRole::InputOnly));
    assert!(!reconnects_automatically(true, false, AppRole::RemoteMonitor));
}

type ConnectResult = Result<(Session, Facts, LaunchPlan), ConnectError>;

fn connecting(result: Receiver<ConnectResult>, worker: Option<std::thread::JoinHandle<()>>) -> Connecting {
    let plan = launch_plan();
    Connecting {
        shared: Shared::new(Arc::new(|| {}), 2),
        interrupt: Interrupt::default(),
        stage: String::new(),
        app_name: plan.app.name.clone(),
        host_name: plan.host_name.clone(),
        host_id: plan.host_id,
        pacing: plan.pacing,
        settings: plan.settings.clone(),
        plan,
        result,
        worker,
    }
}

// Quitting while connecting destroyed the renderer's GPU device while the connect thread could
// still be making a decoder on it: the launch is cancelled and ends first.
#[test]
fn leaving_cancels_the_launch_and_tears_down_only_after_its_worker_ended() {
    let (sender, result) = channel::<ConnectResult>();
    let interrupt = Interrupt::default();
    let ended = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (worker_interrupt, worker_ended) = (interrupt.clone(), ended.clone());
    let worker = std::thread::spawn(move || {
        while !worker_interrupt.is_interrupted() {
            std::thread::sleep(Duration::from_millis(5));
        }
        // Still busy for a while after the cancel, as a decoder being made on the GPU is.
        std::thread::sleep(Duration::from_millis(100));
        let _ = sender.send(Err("Cancelled.".into()));
        std::thread::sleep(Duration::from_millis(50));
        worker_ended.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let mut connecting = connecting(result, Some(worker));
    connecting.interrupt = interrupt;
    let mut torn_down = false;
    assert!(end_before(Some(connecting), Duration::from_secs(10), || {
        assert!(ended.load(std::sync::atomic::Ordering::SeqCst), "the renderer goes only after the worker ended");
        torn_down = true;
    }));
    assert!(torn_down);
    // Nothing connecting: straight to the teardown.
    let mut torn_down = false;
    assert!(end_before(None, Duration::ZERO, || torn_down = true));
    assert!(torn_down);
}

#[test]
fn a_launch_that_does_not_end_keeps_the_renderer_alive() {
    let (_sender, result) = channel::<ConnectResult>();
    let (release, held) = channel::<()>();
    // Deaf to the cancel until the test lets it go.
    let worker = std::thread::spawn(move || { let _ = held.recv(); });
    let started = Instant::now();
    assert!(!end_before(Some(connecting(result, Some(worker))), Duration::from_millis(100), || panic!("torn down under a running launch")));
    assert!(started.elapsed() < Duration::from_secs(5), "the wait is bounded");
    let _ = release.send(());

    // A worker that sent its result but has not returned yet is still waited for.
    let (sender, result) = channel::<ConnectResult>();
    let (release, held) = channel::<()>();
    let worker = std::thread::spawn(move || {
        let _ = sender.send(Err("refused".into()));
        let _ = held.recv();
    });
    assert!(!end_before(Some(connecting(result, Some(worker))), Duration::from_millis(100), || panic!("torn down before the worker returned")));
    let _ = release.send(());
}

#[test]
fn a_replace_prompt_hands_back_the_plan_to_launch_again() {
    for asks_replace in [true, false] {
        let (sender, result) = channel();
        let mut connecting = connecting(result, None);
        sender.send(Err(ConnectError { message: "refused".into(), retryable: false, asks_replace, moved_to: None, packed_refused: false })).unwrap();
        match connecting.poll() {
            Err(ConnectFailed::ConfirmReplace(plan)) => {
                assert!(asks_replace);
                assert!(plan.app.id == 9 && !plan.resume && !plan.confirm_replace);
            }
            Err(ConnectFailed::Message(message)) => {
                assert!(!asks_replace);
                assert_eq!(message, "refused");
            }
            Err(ConnectFailed::Moved { .. } | ConnectFailed::PackedRefused(_)) | Ok(_) => panic!("the launch failed"),
        }
    }
}

#[test]
fn a_stream_that_moved_stops_its_recovery_and_says_where_it_went() {
    use nightjar_core::http::{Error, StreamMove};
    let moved: StreamMove = serde_json::from_str(r#"{"moved_to":{"device":"TV","since_unix_ms":1}}"#).unwrap();
    let device = |asked: Result<Option<String>, ConnectError>| asked.ok().expect("an answer");
    assert_eq!(device(resume_moved_to(true, false, true, || Ok(moved.clone()))).as_deref(), Some("TV"));
    assert_eq!(device(resume_moved_to(true, false, true, || Ok(StreamMove::default()))), None);
    // An old PC is not even asked.
    assert_eq!(device(resume_moved_to(true, false, false, || panic!("not asked"))), None);
    // One that cannot answer is asked by the next attempt: its game may only have moved, so
    // this is not "the original app is no longer running".
    let unanswered = resume_moved_to(true, false, true, || Err(Error::Cancelled)).expect_err("no verdict");
    assert!(unanswered.retryable && !unanswered.asks_replace && unanswered.moved_to.is_none());
    assert!(!unanswered.message.contains("no longer running"), "{}", unanswered.message);
    // A resume that goes on costs no extra request: the PC reports the game to its owner only.
    assert_eq!(device(resume_moved_to(true, true, true, || panic!("not asked"))), None);
    assert_eq!(device(resume_moved_to(false, false, true, || panic!("not asked"))), None);

    // The refusal is final: not retried, and it wins over the end of the recovery window.
    let refusal = ConnectError { message: String::new(), retryable: false, asks_replace: false, moved_to: Some("TV".into()), packed_refused: false };
    assert!(!refusal.retryable);
    let (sender, result) = channel();
    let mut connecting = connecting(result, None);
    sender.send(Err(refusal)).unwrap();
    match connecting.poll() {
        Err(ConnectFailed::Moved { app, device, .. }) => assert_eq!((app, device.as_str()), (9, "TV")),
        _ => panic!("the recovery must stop on the move"),
    }
}

/// The PC refused full chroma in the handshake (-106), after its launch: the plan comes back
/// to start once more without it, not as a failure. Any other failure stays one.
#[test]
fn a_full_chroma_refusal_after_the_launch_hands_back_the_plan_to_start_again() {
    use nightjar_core::full_chroma::restarts_without;
    use nightjar_core::session::error::PACKED_444_REFUSED;
    for (asked, code) in [(true, PACKED_444_REFUSED), (false, PACKED_444_REFUSED), (true, -100)] {
        let (sender, result) = channel();
        let mut connecting = connecting(result, None);
        let packed_refused = restarts_without(asked, code);
        sender.send(Err(ConnectError { message: "Could not start the stream".into(), retryable: false, asks_replace: false, moved_to: None, packed_refused })).unwrap();
        match connecting.poll() {
            Err(ConnectFailed::PackedRefused(plan)) => {
                assert!(asked && code == PACKED_444_REFUSED);
                assert_eq!(plan.app.id, 9);
            }
            Err(ConnectFailed::Message(message)) => {
                assert!(!(asked && code == PACKED_444_REFUSED), "{message}");
            }
            _ => panic!("the launch failed"),
        }
    }
}
