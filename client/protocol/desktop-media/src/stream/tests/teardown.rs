//! Unit tests for RTP clock unwrap, overlay formatting, terminations and session teardown.

use super::negotiate::display;
use super::super::*;

#[test]
fn rtp_clock_unwraps() {
    let mut c = RtpClock::default();
    let a = c.host_us(u32::MAX - 899);
    let b = c.host_us(600);
    // 1500 ticks of 90 kHz across the wrap, give or take the rounding.
    assert!((b - a - 16_667).abs() <= 1, "{}", b - a);
}

#[test]
fn the_pipeline_line_says_each_stage_in_fps() {
    let report = PipelineRatesReport {
        short_window: false,
        window_ms: 1000,
        source_mhz: 119_880,
        capture_mhz: 240_000,
        submit_mhz: 120_000,
        output_mhz: 120_000,
        repeat_mhz: 0,
    };
    assert_eq!(pipeline_rates_line(&report), "119.9 source, 240.0 captured, 120.0 encoded, 120.0 sent, 0.0 repeated fps");
    let unknown = PipelineRatesReport { source_mhz: UNKNOWN, ..report };
    assert!(pipeline_rates_line(&unknown).starts_with("- source, 240.0 captured"));
    crate::strings::set(crate::strings::Lang::De);
    assert_eq!(pipeline_rates_line(&report), "119.9 Quelle, 240.0 erfasst, 120.0 kodiert, 120.0 gesendet, 0.0 wiederholt fps");
}

#[test]
fn a_host_without_hardware_readings_is_not_asked_again_but_a_failed_request_is() {
    use nightjar_core::http::{Availability, Error};
    let sample = HardwareMetrics::parse(r#"{"version":1,"sampled_at_unix_ms":5,"interval_ms":1000,"cpu":{},"gpus":[]}"#).unwrap();
    assert_eq!(hardware_answer(&Ok(sample)), HardwareAnswer::Sample { sampled_at_unix_ms: 5, interval_ms: 1000 });
    let unavailable = [
        HostApiError::Unavailable { feature: Feature::HardwareMetrics, availability: Availability::Unknown },
        HostApiError::UnsupportedVersion,
        HostApiError::Host(Error::NotFound("api/hardware-metrics".into())),
        HostApiError::Host(Error::Status { code: 403, message: String::new() }),
        HostApiError::Host(Error::Status { code: 409, message: String::new() }),
    ];
    for error in unavailable {
        assert_eq!(hardware_answer(&Err(error)), HardwareAnswer::Unavailable);
    }
    for error in [Error::Status { code: 500, message: String::new() }, Error::Malformed("{".into()), Error::Cancelled] {
        assert_eq!(hardware_answer(&Err(HostApiError::Host(error))), HardwareAnswer::Failed, "tried again later");
    }
}

#[test]
fn overlay_modes_show_their_own_lines() {
    let overlay = Overlay {
        lines: vec![("Decoder".into(), "d3d11va".into())],
        statistics: vec![("Frames".into(), "60".into())],
        ..Default::default()
    };
    assert_eq!(overlay.lines(OverlayMode::Debug)[0].0, "Decoder");
    assert_eq!(overlay.lines(OverlayMode::Statistics)[0].0, "Frames");
    assert!(overlay.lines(OverlayMode::Compact).is_empty(), "Compact draws its one line instead");
}

#[test]
fn the_link_notice_line_says_the_lowered_bitrate() {
    use rubylight_protocol::control::LinkReason;
    let notice = |current_kbps| LinkNotice {
        state: LinkState::Limited,
        reason: LinkReason::Network,
        ceiling_kbps: 150_000,
        current_kbps,
    };
    assert_eq!(link_notice_line(&notice(37_500)), "PC lowered the bitrate to 38 Mbps: network");
    assert_eq!(link_notice_line(&notice(9_900)), "PC lowered the bitrate to 9.9 Mbps: network");
    crate::strings::set(crate::strings::Lang::De);
    assert_eq!(link_notice_line(&notice(4_500)), "PC hat die Bitrate auf 4.5 Mbit/s gesenkt: Netzwerk");
}

#[test]
fn terminations_read_well() {
    assert!(termination_text(0).is_none());
    assert!(termination_text(-100).unwrap().contains("UDP"));
    // A stop before the first picture is usually the game closing, not protected content.
    let early = termination_text(nightjar_core::session::error::UNEXPECTED_EARLY).unwrap();
    assert!(early.contains("The game may have closed"), "{early}");
}

#[test]
fn a_start_the_pc_refused_in_the_handshake_shows_its_reason() {
    let refused = connect::start_failure_text("Could not start the stream: RTSP handshake failed".into(), Stage::RtspHandshake, || {
        Some("No encoder on this PC can stream HEVC".into())
    });
    assert!(refused.starts_with("Could not start the stream: RTSP handshake failed"));
    assert!(refused.ends_with("The PC reported: No encoder on this PC can stream HEVC"), "{refused}");
    // A PC that keeps no reason, as other hosts never do, leaves today's text.
    let plain = connect::start_failure_text("Could not start the stream: x".into(), Stage::RtspHandshake, || None);
    assert_eq!(plain, "Could not start the stream: x");
    assert_eq!(connect::start_failure_text("e".into(), Stage::RtspHandshake, || Some(" ".into())), "e");
    // Another stage does not ask the PC at all.
    let other = connect::start_failure_text("e".into(), Stage::VideoStreamStart, || panic!("asked the PC"));
    assert_eq!(other, "e");
}

#[test]
fn only_terminations_the_pc_sent_ask_it_for_the_reason() {
    use nightjar_core::session::error;
    for code in [
        0x8000_4005_u32 as i32,
        0xc000_0005_u32 as i32,
        error::UNEXPECTED_EARLY,
        error::PROTECTED_CONTENT,
        error::FRAME_CONVERSION,
    ] {
        assert!(host_ended_stream(code), "{code:#x}");
    }
    for code in [error::GRACEFUL, error::DISCONNECTED, error::NO_VIDEO_TRAFFIC, error::NO_VIDEO_FRAME, 110, 10054, 503] {
        assert!(!host_ended_stream(code), "{code}");
    }
    let body = doctor::with_host_reason(
        "The PC could not convert the picture for streaming.".into(),
        Some("HDR is on, but the display is not in HDR"),
    );
    assert_eq!(
        body,
        "The PC could not convert the picture for streaming.\n\nThe PC reported: HDR is on, but the display is not in HDR"
    );
    assert_eq!(doctor::with_host_reason("Ended.".into(), None), "Ended.");
}

#[test]
fn an_unreported_or_unusable_refresh_rate_leaves_pacing_unfloored() {
    assert_eq!(display().refresh_interval_us(), Some(6_949), "1 s / 143.9 Hz, in whole microseconds");
    let unknown = [
        Display { refresh_hz: 60.0, refresh_reported: false, ..display() },
        Display { refresh_hz: f32::NAN, refresh_reported: true, ..display() },
        Display { refresh_hz: f32::INFINITY, refresh_reported: true, ..display() },
        Display { refresh_hz: 0.0, refresh_reported: true, ..display() },
    ];
    for display in unknown {
        assert_eq!(display.refresh_interval_us(), None, "{display:?} sets no floor");
    }
}

#[test]
fn a_reconfigured_rate_is_confirmed_by_the_hosts_frame_timestamps() {
    assert!(shows_rate(1500, 60_000));
    assert!(shows_rate(1499, 60_000), "the host rounds its 90 kHz stamps");
    assert!(shows_rate(750, 120_000));
    assert!(shows_rate(1502, 59_940), "59.94 Hz at 90 kHz");
    assert!(!shows_rate(1500, 120_000), "a rate the host did not apply");
    assert!(!shows_rate(750, 60_000));
    assert!(!shows_rate(0, 60_000));
    assert!(!shows_rate(1500, 0), "no rate to confirm");
}

#[test]
fn teardown_runs_off_the_calling_thread() {
    let caller = std::thread::current().id();
    let (sender, receiver) = std::sync::mpsc::channel();
    let mut teardown = crate::stream::spawn_background(move || {
        let _ = sender.send(std::thread::current().id());
    });
    let ran_on = receiver.recv_timeout(std::time::Duration::from_secs(10)).expect("teardown runs");
    assert_ne!(ran_on, caller, "a session's stop joins its threads, never on the UI thread");
    assert!(teardown.join_until(Instant::now() + Duration::from_secs(10)));
}

// A request the host does not apply puts every field it changed back together: the facts' rate,
// the shared millihertz, the settings (rate and size), the request in flight and, through it, the
// bitrate policy's mode, the menu's rate and the pacer's frame time.
#[test]
fn a_timed_out_reconfigure_restores_the_whole_mode() {
    let before = ModeBefore {
        fps_millihz: 59_940,
        fps: 60,
        settings_fps: 60,
        width: 2560,
        height: 1440,
        requested: Reconfigure { width: 2560, height: 1440, fps_millihz: 59_940 },
    };
    let mut settings = Settings { fps: 120, width: 1920, height: 1080, ..Default::default() };
    let shared = std::sync::atomic::AtomicU32::new(120_000);
    let mut requested = Reconfigure { width: 1920, height: 1080, fps_millihz: 120_000 };
    let mut fps = 120;
    before.restore(&mut fps, &shared, &mut settings, &mut requested);
    assert_eq!((fps, shared.load(Ordering::Relaxed), settings.fps), (60, 59_940, 60));
    assert_eq!((settings.width, settings.height), (2560, 1440));
    assert_eq!(requested, before.requested, "the mode menu and the plan read is the old one");
    assert_eq!(before.fps_millihz.div_ceil(1000), 60, "the pacer gets the old rate");
}

// A reconnect keeps the running session's packet size, as the PC refuses a test while it holds the
// session. Jumbo packets were tested on the path the stream started on: once the network moved
// (docked Ethernet to Wi-Fi), the reconnect asks for normal packets instead of streaming nothing.
#[test]
fn a_reconnect_after_the_network_moved_does_not_carry_jumbo_packets() {
    assert_eq!(carried_packet_size(8160, false), 8160);
    assert_eq!(carried_packet_size(8160, true), 0);
    // A normal size was never tested and is carried as before.
    assert_eq!(carried_packet_size(1392, true), 1392);
    assert_eq!(carried_packet_size(1392, false), 1392);
    assert_eq!(carried_packet_size(-1, true), 0);
    // What the reconnect then asks for: the route's size, or the run's own normal one.
    assert_eq!(connect::resume_packet_size(carried_packet_size(8160, true), 1392, 0), 1392);
    assert_eq!(connect::resume_packet_size(carried_packet_size(8160, true), 1392, 8192), 1392);
    assert_eq!(connect::resume_packet_size(carried_packet_size(8160, true), 1392, 1024), 1024);
    assert_eq!(connect::resume_packet_size(carried_packet_size(8160, false), 1392, 0), 8160);
}

// The snapshot is the stream as it runs, not the mode the menu's size editor (or the request
// itself) already wrote into the requested mode: a 1440p60 stream asked for 4K120 that the PC does
// not apply goes back to 1440p60 in the menu and in the bitrate policy, which reads this mode.
#[test]
fn the_mode_to_go_back_to_is_the_running_one_not_the_asked_one() {
    let settings = Settings { fps: 60, width: 2560, height: 1440, ..Default::default() };
    let before = ModeBefore::of((2560, 1440, 60), 59_940, &settings);
    assert_eq!(before.requested, Reconfigure { width: 2560, height: 1440, fps_millihz: 59_940 });
    assert_eq!((before.fps, before.fps_millihz, before.settings_fps), (60, 59_940, 60));
    assert_eq!((before.width, before.height), (2560, 1440));

    // Restoring over the asked mode, as the timeout does.
    let mut asked_settings = Settings { fps: 120, width: 3840, height: 2160, ..Default::default() };
    let shared = std::sync::atomic::AtomicU32::new(120_000);
    let mut requested = Reconfigure { width: 3840, height: 2160, fps_millihz: 120_000 };
    let mut fps = 120;
    before.restore(&mut fps, &shared, &mut asked_settings, &mut requested);
    assert_eq!(requested, Reconfigure { width: 2560, height: 1440, fps_millihz: 59_940 });
    assert_eq!((asked_settings.width, asked_settings.height, asked_settings.fps), (2560, 1440, 60));
}

/// The teardown handle is what lets the app hold the renderer until a session's decoder and audio
/// threads have ended, and reports honestly when the deadline passed first.
#[test]
fn a_teardown_is_joined_where_its_owner_must_wait() {
    let (sender, receiver) = std::sync::mpsc::channel();
    let mut teardown = spawn_background(move || {
        let _ = sender.send(());
    });
    assert!(teardown.join_until(Instant::now() + Duration::from_secs(10)));
    assert!(receiver.try_recv().is_ok(), "the stop ran before the join returned");

    let (started, running) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let mut teardown = spawn_background(move || {
        let _ = started.send(());
        let _ = released.recv();
    });
    running.recv().expect("the teardown started");
    assert!(!teardown.join_until(Instant::now() + Duration::from_millis(10)), "a deadline that passed first says the session did not join");
    release.send(()).expect("release the teardown");
}

/// A reconnect joins the old session's teardown before the connect worker's first request, so the
/// PC never sees this device's old control connection and its new one at once.
#[test]
fn a_reconnect_joins_the_old_session_before_its_first_request() {
    let ended = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = ended.clone();
    let previous = spawn_background(move || {
        std::thread::sleep(Duration::from_millis(20));
        flag.store(true, Ordering::Release);
    });
    let shared = pipeline::Shared::new(std::sync::Arc::new(|| {}), 2);
    super::super::connect::wait_for_previous(vec![previous], &shared, &Interrupt::default());
    assert!(ended.load(Ordering::Acquire), "the old session ended before the launch");
    assert!(lock(&shared.teardowns).is_empty(), "a teardown that ended is not handed on");
}

/// A launch right after a stream waits for every session still stopping, as a reconnect does for
/// the one it replaces. Leaving the start ends the wait: what still runs goes to the app to join,
/// never dropped.
#[test]
fn a_launch_joins_every_stopping_session_and_hands_on_what_it_leaves() {
    let ended = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let stopping: Vec<Teardown> = (0..2)
        .map(|_| {
            let count = ended.clone();
            spawn_background(move || {
                std::thread::sleep(Duration::from_millis(20));
                count.fetch_add(1, Ordering::AcqRel);
            })
        })
        .collect();
    let shared = pipeline::Shared::new(std::sync::Arc::new(|| {}), 2);
    super::super::connect::wait_for_previous(stopping, &shared, &Interrupt::default());
    assert_eq!(ended.load(Ordering::Acquire), 2, "both sessions ended before the launch");
    assert!(lock(&shared.teardowns).is_empty());

    // A session that does not stop, and a start the player left: no five-second wait, and the
    // handle is kept for the app.
    let (release, released) = std::sync::mpsc::channel::<()>();
    let stuck = spawn_background(move || {
        let _ = released.recv();
    });
    let left = Interrupt::default();
    left.interrupt();
    let began = Instant::now();
    super::super::connect::wait_for_previous(vec![stuck], &shared, &left);
    assert!(began.elapsed() < LEAVE_WAIT, "leaving the start ends the wait");
    let mut kept = std::mem::take(&mut *lock(&shared.teardowns));
    assert_eq!(kept.len(), 1, "the session still stopping is handed to the app");
    release.send(()).expect("release the teardown");
    assert!(kept[0].join_until(Instant::now() + Duration::from_secs(10)));
}

/// A renderer switch and the exit prune the teardowns that ended and are busy only while one is
/// still stopping on the device.
#[test]
fn a_teardown_blocks_the_renderer_only_until_it_ends() {
    let mut teardowns = Vec::new();
    teardowns.push(spawn_background(|| {}));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !teardowns[0].finished() {
        assert!(Instant::now() < deadline, "the empty teardown ends");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(!prune_teardowns(&mut teardowns), "a finished teardown does not block");
    assert!(teardowns.is_empty(), "and is dropped");

    let (release, released) = std::sync::mpsc::channel::<()>();
    teardowns.push(spawn_background(move || {
        let _ = released.recv();
    }));
    assert!(prune_teardowns(&mut teardowns), "a session still stopping blocks");
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while prune_teardowns(&mut teardowns) {
        assert!(Instant::now() < deadline, "the teardown ends");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// A connect attempt that set a session aside to stop hands its teardown to the app, which joins
/// it with the others.
#[test]
fn a_connect_attempt_hands_its_teardown_to_the_app() {
    let shared = pipeline::Shared::new(std::sync::Arc::new(|| {}), 2);
    let (release, released) = std::sync::mpsc::channel::<()>();
    lock(&shared.teardowns).push(spawn_background(move || {
        let _ = released.recv();
    }));
    let mut kept = std::mem::take(&mut *lock(&shared.teardowns));
    assert_eq!(kept.len(), 1);
    assert!(prune_teardowns(&mut kept), "still stopping");
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while prune_teardowns(&mut kept) {
        assert!(Instant::now() < deadline, "the teardown ends");
        std::thread::sleep(Duration::from_millis(2));
    }
}
