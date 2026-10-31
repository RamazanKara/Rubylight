use super::*;
#[test]
fn events_expire_and_clearing_absent_codes_skips_the_lock() {
    let warnings = Warnings::default();
    warnings.clear("encoder_recovery");
    assert_eq!(warnings.present.load(Ordering::Relaxed), 0);
    warnings.set("display_virtual", "Physical display in use");
    warnings.event("network_send", "Packets dropped", Duration::ZERO);
    // An expired event disappears; a lasting condition stays.
    assert_eq!(
        warnings
            .snapshot()
            .iter()
            .map(|w| w.code.as_str())
            .collect::<Vec<_>>(),
        ["display_virtual"]
    );
    assert_eq!(
        warnings.present.load(Ordering::Relaxed),
        warning_bit("display_virtual")
    );
    warnings.event("network_send", "Packets dropped", Duration::from_secs(60));
    assert_eq!(warnings.snapshot().len(), 2);
    warnings.clear("display_virtual");
    warnings.clear("network_send");
    assert!(warnings.snapshot().is_empty());
    assert_eq!(warnings.present.load(Ordering::Relaxed), 0);
}
#[test]
fn warnings_survive_preparation_and_clear_after_recovery() {
    let launch = launch("warnings", Role::Stream);
    launch.warnings.set("display", "Physical display in use");
    let session = Session::new(launch.clone(), Negotiated::default());
    *session.encoder.write().unwrap() = "amf".into();
    let capture = Arc::new(Warnings::default());
    *session.capture_warnings.write().unwrap() = capture.clone();
    capture.set("capture", "Desktop Duplication in use");
    launch.warnings.set("display", "Physical display in use");
    assert_eq!(session.info()["encoder"], "amf");
    assert_eq!(
        session.info()["warnings"],
        serde_json::json!([
            {"code":"display", "message":"Physical display in use"},
            {"code":"capture", "message":"Desktop Duplication in use"}
        ])
    );
    capture.set("capture", "Capture recovering");
    assert_eq!(
        session.info()["warnings"][1]["message"],
        "Capture recovering"
    );
    capture.clear("capture");
    launch.warnings.clear("display");
    assert_eq!(session.info()["warnings"], serde_json::json!([]));
    let other = Session::new(
        super::tests::launch("other", Role::Stream),
        Negotiated::default(),
    );
    launch.warnings.set("audio", "Audio interrupted");
    assert_eq!(other.info()["warnings"], serde_json::json!([]));
}
#[test]
fn a_roam_moves_the_client_and_restarts_the_media_ping_wait() {
    let mut launch = launch("roam", Role::Stream);
    launch.peer = "::ffff:192.168.1.20".parse().unwrap();
    let session = Session::new(launch, Negotiated::default());
    let start = Instant::now();
    assert_eq!(
        session.client_ip(),
        "192.168.1.20".parse::<IpAddr>().unwrap()
    );
    assert_eq!(session.media_wait_start(start), start);
    assert!(!session.take_new_path());
    let at = start + Duration::from_secs(60);
    session.roamed("::ffff:10.0.0.7".parse().unwrap(), at);
    assert_eq!(session.client_ip(), "10.0.0.7".parse::<IpAddr>().unwrap());
    assert_eq!(session.roams(), 1);
    assert_eq!(session.media_wait_start(start), at);
    assert!(session.take_new_path(), "the first ping from the new path");
    assert!(!session.take_new_path(), "once");
}
#[test]
fn a_new_path_forgets_the_old_paths_pacing_estimate_and_keyframe_loss() {
    let session = Session::new(launch("new-path", Role::Stream), Negotiated::default());
    session.configure_automatic_pacing(true);
    session.configure_keyframe_loss(true);
    let lose = |session: &Session| {
        for frame in 10..=12 {
            session.keyframe_loss.lock().unwrap().unrecoverable(
                frame,
                frame,
                Duration::from_millis(16),
                Instant::now(),
            );
        }
    };
    lose(&session);
    assert!(session.keyframe_loss_active());
    session.video_pacing_bps(100_000_000, 10_000_000);
    let ceiling = |session: &Session| {
        session.automatic_pacing.lock().unwrap().snapshot()["ceiling_kbps"].clone()
    };
    assert_ne!(ceiling(&session), 0);
    session.configure_adaptive_bitrate(true, 0);
    {
        let mut bitrate = session.adaptive_bitrate.lock().unwrap();
        bitrate.on_block(crate::adaptive_fec::Block::Clean);
        let start = Instant::now();
        for i in 0..2 {
            let now = start + Duration::from_secs(i);
            bitrate.on_ping(now, &[4, 0, 0, 0, 0, 0, 0, 0]);
            bitrate.tick(
                now,
                Default::default(),
                Duration::from_millis(2),
                20,
                Duration::from_millis(16),
                20_000,
            );
        }
        assert_eq!(bitrate.minimum_rtt(), Some(Duration::from_millis(2)));
    }
    session.new_network_path();
    assert!(!session.keyframe_loss_active());
    assert_eq!(ceiling(&session), 0);
    assert_eq!(session.adaptive_bitrate.lock().unwrap().minimum_rtt(), None);
    assert!(session.automatic_pacing_enabled());
    // The guard still watches the new path.
    lose(&session);
    assert!(session.keyframe_loss_active());
}
#[test]
fn fec_status_clients_do_not_latch_on_requests_even_after_a_roam() {
    const FAST: u64 = 800_000_000;
    const SAFE: u64 = 62_000_000;
    for features in [1, 0] {
        let s = Session::new(
            launch("fec-status", Role::Stream),
            Negotiated {
                moonlight_features: features,
                ..Default::default()
            },
        );
        s.configure_keyframe_loss(true);
        // Asked, sent keyframe 41, asked again right after it.
        let ask_twice = |s: &Session, first: u32| {
            for frame in first..first + 40 {
                s.keyframe_loss_bps(FAST, SAFE, frame, frame == first);
            }
            s.keyframe_loss.lock().unwrap().settle();
            s.request_recovery_keyframe();
            s.keyframe_loss_bps(FAST, SAFE, first + 40, true);
            s.keyframe_loss_bps(FAST, SAFE, first + 41, false);
            s.request_recovery_keyframe();
        };
        ask_twice(&s, 1);
        assert_eq!(s.keyframe_loss_active(), features == 0, "{features}");
        s.new_network_path();
        assert!(!s.keyframe_loss_active());
        ask_twice(&s, 100);
        assert_eq!(
            s.keyframe_loss_active(),
            features == 0,
            "after a roam, {features}"
        );
    }
}
#[test]
fn a_roam_drops_the_clients_hints_about_the_old_path() {
    let session = Session::new(
        launch("hints", Role::Stream),
        Negotiated {
            codec: 3,
            bitrate_kbps: 200_000,
            configured_bitrate_kbps: 250_000,
            pyrowave_client_link_mbps: 100,
            pyrowave_client_pace_mbps: 150,
            ..Default::default()
        },
    );
    let ceiling = |session: &Session| {
        session.configure_adaptive_bitrate(true, 0);
        session.adaptive_bitrate.lock().unwrap().snapshot()["wire_ceiling_kbps"].clone()
    };
    assert_eq!(
        (session.client_link_mbps(), session.client_pace_mbps()),
        (100, 150)
    );
    assert_eq!(ceiling(&session), 95_000);
    session.roamed("10.0.0.7".parse().unwrap(), Instant::now());
    assert_eq!(
        (session.client_link_mbps(), session.client_pace_mbps()),
        (0, 0)
    );
    assert_eq!(ceiling(&session), 250_000);
}
#[test]
fn the_console_gets_pipeline_rates_once_a_window_is_complete() {
    let session = Session::new(launch("rates", Role::Stream), Negotiated::default());
    assert!(session.info()["pipeline_rates"].is_null());
    session.stats.pipeline.captured(1, 2);
    session.stats.pipeline.submitted(true);
    session.stats.frames.fetch_add(1, Ordering::Relaxed);
    assert_eq!(
        session.pipeline_counts(),
        crate::pipeline_rates::Counts {
            source: 1,
            capture: 2,
            submit: 1,
            output: 1,
            repeat: 1,
        }
    );
    std::thread::sleep(crate::pipeline_rates::CONSOLE_WINDOW);
    let rates = &session.info()["pipeline_rates"];
    assert!(rates["window_ms"].as_u64().unwrap() >= 1000, "{rates}");
    assert!(rates["capture_mhz"].as_u64().unwrap() > 0, "{rates}");
    assert!(rates["repeat_mhz"].as_u64().unwrap() > 0, "{rates}");
}
#[test]
fn client_fec_is_exposed_as_telemetry_without_changing_stream_decisions() {
    let session = Session::new(launch("fec", Role::Stream), Negotiated::default());
    session.idr.store(false, Ordering::Relaxed);
    session.stats.video_frame.store(8, Ordering::Release);
    let bitrate = session.bitrate.load(Ordering::Relaxed);
    let report = [
        0, 0, 0, 7, 0, 11, 0, 0, 0, 1, 0, 10, 0, 2, 0, 9, 0, 1, 20, 0, 1,
    ];
    session.record_fec_status(&report);
    session.record_fec_status(&report[..20]);
    let info = session.info();
    assert_eq!(info["performance"]["fec_reports"], 2);
    assert_eq!(info["performance"]["fec_recovered_frames"], 1);
    assert_eq!(info["performance"]["fec_invalid_reports"], 1);
    assert_eq!(info["performance"]["fec_loss_reports"], 0);
    assert_eq!(session.bitrate.load(Ordering::Relaxed), bitrate);
    assert!(!session.idr.load(Ordering::Relaxed));
    assert!(session.invalidation.lock().unwrap().is_none());
    assert_eq!(info["idr_requests"], 0);
    assert_eq!(info["reference_invalidations"], 0);
    assert!(!session.stopping());
}
#[test]
fn statistical_fec_reports_show_in_the_stream_timings() {
    use crate::extensions::FecChoice;
    use crate::statistical_fec::FecReport;
    let report = |seq| {
        FecReport {
            seq,
            data_shards: 600,
            interval_ms: 1000,
            ..Default::default()
        }
        .encode()
    };
    let start = Instant::now();
    // The Retroid run: B3 reports moved statistical FEC 20 → 0 while
    // `fec_reports`, which counts only 0x5502, stayed 0.
    for choice in [FecChoice::Auto, FecChoice::Host] {
        let session = Session::new(launch("fec", Role::Stream), Negotiated::default());
        session.configure_fec_choice(choice, true, 20, 50);
        let used = choice == FecChoice::Auto;
        assert_eq!(
            session.record_fec_report_at(&report(0), start).unwrap().1,
            used
        );
        session
            .record_fec_report_at(&report(1), start + Duration::from_secs(1))
            .unwrap();
        assert!(session.record_fec_report_at(&[1; 3], start).is_err());
        let performance = &session.info()["performance"];
        assert_eq!(performance["fec_loss_reports"], 2, "{choice:?}");
        assert_eq!(performance["fec_loss_refused_reports"], 1, "{choice:?}");
        assert_eq!(performance["fec_reports"], 0, "{choice:?}");
    }
}
fn fec_report(frame: u32, received_data: u16, received_parity: u16) -> [u8; 21] {
    // Ten data and two parity packets at 20 %, one block.
    let mut report = [0; 21];
    report[..4].copy_from_slice(&frame.to_be_bytes());
    for (offset, word) in [(4, 11u16), (10, 10), (12, 2)] {
        report[offset..offset + 2].copy_from_slice(&word.to_be_bytes());
    }
    report[14..16].copy_from_slice(&received_data.to_be_bytes());
    report[16..18].copy_from_slice(&received_parity.to_be_bytes());
    report[18] = 20;
    report[20] = 1;
    report
}
#[test]
fn host_bitrate_requires_valid_video_feedback_and_yields_to_the_client() {
    let s = Session::new(
        launch("host-abr", Role::Stream),
        Negotiated {
            bitrate_kbps: 20_000,
            configured_bitrate_kbps: 30_000,
            ..Default::default()
        },
    );
    s.configure_adaptive_bitrate(true, 0);
    let start = Instant::now();
    s.tick_adaptive_bitrate(start, Duration::from_millis(8));
    s.record_fec_status(&fec_report(1, 7, 1)); // no frame has been sent
    assert!(!s.adaptive_bitrate.lock().unwrap().active());
    s.stats.video_frame.store(10, Ordering::Release);
    s.record_fec_status(&fec_report(1, 7, 1));
    s.tick_adaptive_bitrate(start + Duration::from_secs(1), Duration::from_millis(8));
    assert!(s.bitrate.load(Ordering::Acquire) < 20_000);
    assert!(s.adaptive_pacing_bps(u64::MAX) < 30_000_000);
    let after_loss = s.info()["adaptive_bitrate"]["wire_budget_kbps"].clone();
    s.record_fec_status(&fec_report(1, 7, 1)); // same block is ignored
    s.tick_adaptive_bitrate(start + Duration::from_secs(2), Duration::from_millis(8));
    assert_eq!(s.info()["adaptive_bitrate"]["wire_budget_kbps"], after_loss);
    s.set_client_bitrate(17_000);
    s.record_fec_status(&fec_report(2, 7, 1));
    s.tick_adaptive_bitrate(start + Duration::from_secs(4), Duration::from_millis(200));
    assert_eq!(s.bitrate.load(Ordering::Acquire), 17_000);
    assert_eq!(s.adaptive_pacing_bps(800_000_000), 800_000_000);
    assert_eq!(s.info()["adaptive_bitrate"]["owner"], "client");
}
/// A session with the link controller on, `frames` frames of 100 packets
/// sent 17 ms apart ending now, and the wire frame marker past them.
fn link_session(frames: u32, enabled: bool) -> (Arc<Session>, Instant) {
    let s = Session::new(
        launch("link-rate", Role::Stream),
        Negotiated {
            bitrate_kbps: 20_000,
            configured_bitrate_kbps: 20_000,
            ..Default::default()
        },
    );
    s.configure_link_rate(enabled, 0);
    let end = Instant::now();
    for frame in 1..=frames {
        let at = end - Duration::from_millis(17 * u64::from(frames - frame));
        s.record_link_sent_at(frame, 100, 112_600, at);
    }
    s.stats
        .video_frame
        .store(u64::from(frames) + 10, Ordering::Release);
    (s, end)
}
fn lose_frames(s: &Session, frames: std::ops::RangeInclusive<u32>) {
    for frame in frames {
        s.record_fec_status(&fec_report(frame, 7, 1));
    }
}
#[test]
fn a_burst_of_lost_frames_lowers_the_bitrate_and_says_so() {
    let (s, end) = link_session(60, true);
    assert_eq!(s.info()["link_rate"]["state"], "full");
    lose_frames(&s, 50..=54);
    let (from, to) = s
        .tick_link_rate(end + Duration::from_millis(200), Duration::from_millis(10))
        .expect("the controller lowers the bitrate");
    assert_eq!(from, 20_000);
    assert!((20_000 * 40 / 100..20_000 * 85 / 100).contains(&to), "{to}");
    assert_eq!(s.bitrate.load(Ordering::Acquire), to);
    let info = s.info();
    assert_eq!(info["link_rate"]["state"], "limited");
    let warning = info["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["code"] == LINK_WARNING)
        .expect("the console gets a warning")
        .clone();
    assert!(
        warning["message"].as_str().unwrap().contains("Mbps"),
        "{warning}"
    );
    // Later ticks do not change the warning or the bitrate while it holds.
    assert_eq!(
        s.tick_link_rate(end + Duration::from_millis(300), Duration::from_millis(10)),
        None
    );
}
/// 2026-10-29 sweep (r-def): eleven invalidation requests for one lost
/// frame cut the bitrate for 26 s; three separate losses still do.
#[test]
fn a_clients_repeated_invalidations_for_one_loss_count_once() {
    let (s, end) = link_session(60, true);
    for last in 45..=55 {
        s.request_invalidation(45, last);
    }
    assert_eq!(
        s.tick_link_rate(end + Duration::from_millis(200), Duration::from_millis(10)),
        None
    );
    assert_eq!(s.info()["link_rate"]["state"], "full");
    let (s, end) = link_session(60, true);
    for first in [20, 35, 50] {
        s.request_invalidation(first, first + 3);
        s.request_invalidation(first, first + 4);
    }
    let (_, to) = s
        .tick_link_rate(end + Duration::from_millis(200), Duration::from_millis(10))
        .expect("three losses lower the bitrate");
    assert!(to < 20_000, "{to}");
}
/// 2026-10-29: a PyroWave stream to a phone on Wi-Fi lost about a quarter
/// of its packets for the whole stream and nothing lowered its bitrate.
/// Review 2026-10-29: only a lost coarse block (block 0) loses the picture;
/// detail blocks carry no parity while it moves, and one lost packet there
/// only blurs part of it.
#[test]
fn lost_pyrowave_frames_lower_the_bitrate_too() {
    let s = Session::new(
        launch("link-rate-pyrowave", Role::Stream),
        Negotiated {
            codec: 3,
            pyrowave_records: true,
            bitrate_kbps: 397_464,
            configured_bitrate_kbps: 400_000,
            ..Default::default()
        },
    );
    s.configure_link_rate(true, 0);
    assert_eq!(s.info()["link_rate"]["state"], "full");
    let end = Instant::now();
    for frame in 1..=60u32 {
        let at = end - Duration::from_millis(17 * u64::from(60 - frame));
        s.record_link_sent_at(frame, 600, 840_000, at);
    }
    s.stats.video_frame.store(70, Ordering::Release);
    let block = |frame, block| {
        let mut report = fec_report(frame, 7, 1);
        report[19] = block;
        report[20] = 3;
        report
    };
    for frame in 40..=49 {
        s.record_fec_status(&block(frame, 1));
        s.record_fec_status(&block(frame, 2));
    }
    assert_eq!(
        s.tick_link_rate(end + Duration::from_millis(100), Duration::from_millis(10)),
        None,
        "lost detail blocks are not lost pictures"
    );
    for frame in 50..=54 {
        s.record_fec_status(&block(frame, 0));
    }
    let (from, to) = s
        .tick_link_rate(end + Duration::from_millis(200), Duration::from_millis(10))
        .expect("the controller lowers the PyroWave bitrate");
    assert_eq!(from, 397_464);
    assert!(
        (397_464 * 40 / 100..397_464 * 85 / 100).contains(&to),
        "{to}"
    );
    assert_eq!(s.bitrate.load(Ordering::Acquire), to);
    assert_eq!(s.info()["link_rate"]["state"], "limited");
}
/// Review 2026-10-29: 1080p120 at 200 Mbps is under PyroWave's 373 Mbps
/// floor. Its warning must not call the user's bitrate the lowest at which
/// PyroWave keeps a usable picture: this link loses PyroWave at that rate,
/// and HEVC or AV1 will hold up better. Its WARN line comes once.
#[test]
fn a_pyrowave_stream_with_no_room_under_it_is_told_to_use_hevc_or_av1() {
    let s = Session::new(
        launch("link-rate-pyrowave-no-room", Role::Stream),
        Negotiated {
            codec: 3,
            pyrowave_records: true,
            width: 1920,
            height: 1080,
            fps: 120,
            bitrate_kbps: 200_000,
            configured_bitrate_kbps: 200_000,
            ..Default::default()
        },
    );
    s.configure_link_rate(true, 0);
    let end = Instant::now();
    for frame in 1..=60u32 {
        let at = end - Duration::from_millis(8 * u64::from(60 - frame));
        s.record_link_sent_at(frame, 300, 420_000, at);
    }
    s.stats.video_frame.store(70, Ordering::Release);
    lose_frames(&s, 50..=54);
    assert_eq!(
        s.tick_link_rate(end + Duration::from_millis(200), Duration::from_millis(10)),
        None,
        "nothing to lower"
    );
    assert_eq!(s.info()["link_rate"]["state"], "floor");
    let message = link_warning(&s).expect("the link warning");
    assert!(
        message.contains("loses PyroWave video at 200 Mbps"),
        "{message}"
    );
    assert!(message.contains("HEVC or AV1"), "{message}");
    assert!(!message.contains("lowest bitrate"), "{message}");
    // The floor again after a quiet spell is INFO, not a second WARN.
    let floor = crate::link_rate::Change {
        kind: crate::link_rate::ChangeKind::Floor,
        reason: "unrecoverable_frames",
        from_kbps: 200_000,
        to_kbps: 200_000,
        delivered_kbps: 0,
        evidence: crate::link_rate::Evidence::default(),
    };
    let mut link = s.link_rate.lock().unwrap();
    assert!(!note_link_change(&mut link, end, floor).warned);
    // A PyroWave stream with room under it warns at every floor, as before.
    let mut room = crate::link_control::LinkControl::new(
        crate::link_rate::LinkRate::new(
            true,
            &Negotiated {
                codec: 3,
                width: 1920,
                height: 1080,
                fps: 60,
                bitrate_kbps: 400_000,
                ..Default::default()
            },
            0,
            20,
        ),
        0,
    );
    for _ in 0..2 {
        let report = note_link_change(&mut room, end, floor);
        assert!(report.warned && !report.no_room);
    }
}
/// The floor warning of a PyroWave stream with room under it names the
/// floor as the lowest rate at which PyroWave keeps a usable picture.
#[test]
fn a_pyrowave_floor_warning_names_its_floor_and_hevc_or_av1() {
    let s = Session::new(
        launch("link-rate-pyrowave-floor", Role::Stream),
        Negotiated {
            codec: 3,
            bitrate_kbps: 400_000,
            ..Default::default()
        },
    );
    s.announce_link_change(LinkReport {
        change: crate::link_rate::Change {
            kind: crate::link_rate::ChangeKind::Floor,
            reason: "unrecoverable_frames",
            from_kbps: 186_624,
            to_kbps: 186_624,
            delivered_kbps: 0,
            evidence: crate::link_rate::Evidence::default(),
        },
        was_limited: true,
        warned: true,
        no_room: false,
        ceiling_kbps: 400_000,
        loss_share: 0.,
        lowest_kbps: 186_624,
        log_info: false,
    });
    let message = link_warning(&s).expect("the link warning");
    assert!(
        message.contains("the lowest bitrate at which PyroWave keeps a usable picture"),
        "{message}"
    );
    assert!(message.contains("Use HEVC or AV1"), "{message}");
}
/// Review 2026-10-29: a PyroWave client's keyframe requests are no
/// evidence of loss, even from a client that sends no FEC status; only
/// lost coarse blocks are.
#[test]
fn pyrowave_keyframe_requests_lower_nothing() {
    let s = Session::new(
        launch("link-rate-pyrowave-requests", Role::Stream),
        Negotiated {
            codec: 3,
            pyrowave_records: true,
            bitrate_kbps: 400_000,
            configured_bitrate_kbps: 400_000,
            ..Default::default()
        },
    );
    s.configure_link_rate(true, 0);
    let end = Instant::now();
    for frame in 1..=60u32 {
        let at = end - Duration::from_millis(17 * u64::from(60 - frame));
        s.record_link_sent_at(frame, 600, 840_000, at);
    }
    s.stats.video_frame.store(70, Ordering::Release);
    for _ in 0..10 {
        s.request_client_keyframe();
    }
    assert_eq!(
        s.tick_link_rate(end + Duration::from_millis(200), Duration::from_millis(10)),
        None
    );
    assert_eq!(s.info()["link_rate"]["state"], "full");
}
fn ask_for_notices(s: &Session) {
    use crate::link_notice::LinkNoticeRequest;
    assert_eq!(
        s.link_notices
            .lock()
            .unwrap()
            .on_request(&LinkNoticeRequest { enabled: true }.encode()),
        Some(true)
    );
}
fn notice_at(s: &Session, at: Instant) -> Option<crate::link_notice::LinkNotice> {
    let bytes = s.link_notices.lock().unwrap().take(at)?;
    Some(crate::link_notice::LinkNotice::decode(&bytes).expect("the host sends valid notices"))
}
#[test]
fn a_client_that_asked_is_told_when_the_bitrate_is_lowered_and_when_it_is_back() {
    use crate::link_notice::{LinkReason, LinkState};
    let (s, end) = link_session(60, true);
    ask_for_notices(&s);
    let at = end + Duration::from_millis(200);
    assert_eq!(notice_at(&s, at), None, "nothing to say at the full rate");
    lose_frames(&s, 50..=54);
    let (_, to) = s
        .tick_link_rate(at, Duration::from_millis(10))
        .expect("the controller lowers the bitrate");
    let notice = notice_at(&s, at).expect("the step down is announced");
    assert_eq!(notice.state, LinkState::Limited);
    assert_eq!(notice.reason, LinkReason::Network);
    assert_eq!((notice.ceiling_kbps, notice.current_kbps), (20_000, to));
    assert_eq!(notice_at(&s, at + Duration::from_millis(8)), None, "once");
    // A reconfigure puts the requested bitrate back and the client hears it.
    s.rebuild_link_rate(&s.config, true, 0);
    assert_eq!(s.bitrate.load(Ordering::Acquire), 20_000);
    let notice = notice_at(&s, at + Duration::from_secs(1)).expect("the return is announced");
    assert_eq!(notice.state, LinkState::Full);
    assert_eq!(notice.reason, LinkReason::None);
    assert_eq!((notice.ceiling_kbps, notice.current_kbps), (20_000, 20_000));
}
#[test]
fn a_client_that_did_not_ask_is_told_nothing() {
    let (s, end) = link_session(60, true);
    lose_frames(&s, 50..=54);
    let at = end + Duration::from_millis(200);
    s.tick_link_rate(at, Duration::from_millis(10))
        .expect("the controller lowers the bitrate");
    assert_eq!(notice_at(&s, at), None);
    assert_eq!(notice_at(&s, at + Duration::from_secs(5)), None);
}
#[test]
fn a_client_that_takes_the_bitrate_over_is_told_it_is_no_longer_lowered() {
    use crate::link_notice::LinkState;
    let (s, end) = link_session(60, true);
    ask_for_notices(&s);
    lose_frames(&s, 50..=54);
    let at = end + Duration::from_millis(200);
    s.tick_link_rate(at, Duration::from_millis(10)).unwrap();
    assert_eq!(notice_at(&s, at).unwrap().state, LinkState::Limited);
    s.set_client_bitrate(12_000);
    let notice = notice_at(&s, at + Duration::from_secs(1)).expect("cleared");
    assert_eq!(notice.state, LinkState::Full);
    assert_eq!((notice.ceiling_kbps, notice.current_kbps), (12_000, 12_000));
}
#[test]
fn every_controller_change_has_its_notice() {
    use crate::link_notice::{LinkReason, LinkState};
    use crate::link_rate::{Change, ChangeKind, Evidence};
    for (kind, state, reason) in [
        (
            ChangeKind::StepDown,
            LinkState::Limited,
            LinkReason::Network,
        ),
        (ChangeKind::Probe, LinkState::Limited, LinkReason::Network),
        (
            ChangeKind::ProbeFailed,
            LinkState::Limited,
            LinkReason::Network,
        ),
        (ChangeKind::Floor, LinkState::Floor, LinkReason::Network),
        (
            ChangeKind::EncoderFloor,
            LinkState::Floor,
            LinkReason::Network,
        ),
        (ChangeKind::Full, LinkState::Full, LinkReason::None),
    ] {
        let to_kbps = if kind == ChangeKind::Full {
            20_000
        } else {
            5_000
        };
        let notice = link_notice(&LinkReport {
            change: Change {
                kind,
                reason: "test",
                from_kbps: 10_000,
                to_kbps,
                delivered_kbps: 0,
                evidence: Evidence::default(),
            },
            was_limited: true,
            warned: false,
            no_room: false,
            ceiling_kbps: 20_000,
            loss_share: 0.,
            lowest_kbps: 5_000,
            log_info: false,
        });
        assert_eq!(
            (
                notice.state,
                notice.reason,
                notice.ceiling_kbps,
                notice.current_kbps
            ),
            (state, reason, 20_000, to_kbps),
            "{kind:?}"
        );
    }
}
#[test]
fn a_healthy_session_keeps_its_bitrate_and_has_no_warning() {
    let (s, end) = link_session(120, true);
    // Frames that FEC repaired are not loss the controller acts on.
    for frame in (10..120).step_by(7) {
        s.record_fec_status(&fec_report(frame, 9, 1));
    }
    for ms in (0..3000).step_by(8) {
        assert_eq!(
            s.tick_link_rate(end + Duration::from_millis(ms), Duration::from_millis(10)),
            None
        );
    }
    assert_eq!(s.bitrate.load(Ordering::Acquire), 20_000);
    let info = s.info();
    assert_eq!(info["link_rate"]["state"], "full");
    assert!(
        info["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|w| w["code"] != LINK_WARNING)
    );
}
#[test]
fn the_controller_is_off_when_disabled() {
    let (s, end) = link_session(60, false);
    lose_frames(&s, 50..=54);
    assert_eq!(
        s.tick_link_rate(end + Duration::from_millis(200), Duration::ZERO),
        None
    );
    assert_eq!(s.bitrate.load(Ordering::Acquire), 20_000);
    assert_eq!(s.info()["link_rate"]["state"], "off");
}
#[test]
fn a_client_that_sets_the_bitrate_clears_the_warning_and_restores_nothing() {
    let (s, end) = link_session(60, true);
    lose_frames(&s, 50..=54);
    s.tick_link_rate(end + Duration::from_millis(200), Duration::from_millis(10))
        .unwrap();
    assert!(
        s.launch
            .warnings
            .snapshot()
            .iter()
            .any(|w| w.code == LINK_WARNING)
    );
    s.set_client_bitrate(12_000);
    assert_eq!(s.bitrate.load(Ordering::Acquire), 12_000);
    assert!(
        s.launch
            .warnings
            .snapshot()
            .iter()
            .all(|w| w.code != LINK_WARNING)
    );
    assert_eq!(s.info()["link_rate"]["reason"], "client_controlled");
    lose_frames(&s, 55..=59);
    assert_eq!(
        s.tick_link_rate(end + Duration::from_secs(1), Duration::from_millis(10)),
        None
    );
    assert_eq!(s.bitrate.load(Ordering::Acquire), 12_000);
    // A reconfigure keeps the client in charge and does not bring back the
    // bitrate the host had lowered.
    s.rebuild_link_rate(&s.config, true, 0);
    assert_eq!(s.bitrate.load(Ordering::Acquire), 12_000);
    assert_eq!(s.info()["link_rate"]["reason"], "client_controlled");
}
/// One block of a client FEC report: `data` and `parity` packets, of which
/// `received` arrived (data first).
fn fec_block(frame: u32, block: u8, blocks: u8, data: u16, received: u16) -> [u8; 21] {
    let parity = (u32::from(data) * 20).div_ceil(100) as u16;
    let mut report = [0; 21];
    report[..4].copy_from_slice(&frame.to_be_bytes());
    for (offset, word) in [(10, data), (12, parity), (14, received.min(data))] {
        report[offset..offset + 2].copy_from_slice(&word.to_be_bytes());
    }
    report[16..18].copy_from_slice(&received.saturating_sub(data).to_be_bytes());
    report[18] = 20;
    report[19] = block;
    report[20] = blocks;
    report
}
#[test]
fn a_keyframe_the_host_abandoned_is_not_counted_again_as_network_loss() {
    // Two 300-packet keyframes in two loss windows: every packet of both
    // is missing at the client, 7 % of what was sent in each window.
    for abandoned in [true, false] {
        let s = Session::new(
            launch("link-abandoned", Role::Stream),
            Negotiated {
                bitrate_kbps: 20_000,
                configured_bitrate_kbps: 20_000,
                ..Default::default()
            },
        );
        s.configure_link_rate(true, 0);
        s.configure_keyframe_loss(true);
        s.stats.video_frame.store(500, Ordering::Release);
        let base = Instant::now();
        let rtt = Duration::from_millis(10);
        assert_eq!(s.tick_link_rate(base, rtt), None);
        let mut cut = None;
        for window in 0..2u32 {
            let first = 1 + window * 41;
            for frame in first..first + 40 {
                let at = base
                    + Duration::from_millis(
                        u64::from(window) * 600 + u64::from(frame - first) * 10,
                    );
                s.record_link_sent_at(frame, 100, 112_600, at);
            }
            let keyframe = first + 40;
            s.record_link_sent_at(
                keyframe,
                300,
                337_800,
                base + Duration::from_millis(u64::from(window) * 600 + 400),
            );
            if abandoned {
                s.keyframe_loss.lock().unwrap().sending(keyframe, true);
                s.keyframe_abandoned(keyframe);
            }
            s.record_fec_status(&fec_block(keyframe, 0, 2, 125, 0));
            s.record_fec_status(&fec_block(keyframe, 1, 2, 125, 0));
            cut = s.tick_link_rate(
                base + Duration::from_millis(u64::from(window + 1) * 600),
                rtt,
            );
        }
        assert_eq!(
            cut.is_some(),
            !abandoned,
            "abandoned {abandoned}: {}",
            s.link_rate_snapshot()
        );
        if abandoned {
            assert_eq!(s.link_rate_snapshot()["state"], "full");
        }
    }
}
#[test]
fn pacing_counts_as_fast_only_when_the_applied_rate_is_above_the_wifi_rate() {
    let s = Session::new(launch("link-pace", Role::Stream), Negotiated::default());
    s.configure_keyframe_loss(true);
    assert_eq!(
        s.keyframe_loss_bps(800_000_000, 62_000_000, 1, false),
        800_000_000
    );
    assert!(s.pace_was_fast.load(Ordering::Relaxed));
    // Once slowed to the Wi-Fi rate the route's own rate is not what ran.
    s.configure_remembered_keyframe_loss(true, true);
    assert_eq!(
        s.keyframe_loss_bps(800_000_000, 62_000_000, 2, false),
        62_000_000
    );
    assert!(!s.pace_was_fast.load(Ordering::Relaxed));
}
const FAST: u64 = 800_000_000;
const SAFE: u64 = 62_000_000;
const FRAME: Duration = Duration::from_micros(16_667);
fn fence_at(s: &Session) -> Option<Instant> {
    s.link_rate.lock().unwrap().rate.fence_at()
}
/// A link session whose first keyframe (wire frame 1) was lost at `t0`,
/// with the pace `base` requested for every frame.
fn first_keyframe_lost(base: u64, t0: Instant) -> Arc<Session> {
    let (s, _) = link_session(60, true);
    s.configure_keyframe_loss(true);
    s.keyframe_loss_bps_at(base, SAFE, 1, true, t0);
    s.keyframe_loss_bps_at(base, SAFE, 2, false, t0 + FRAME);
    let latched = s
        .keyframe_loss
        .lock()
        .unwrap()
        .unrecoverable(1, 2, FRAME, t0 + Duration::from_millis(100))
        .unwrap();
    assert_eq!(latched.kind, crate::keyframe_loss::LatchKind::Startup);
    s.report_keyframe_loss(Some(latched), None);
    s
}
#[test]
fn a_latch_confirmed_by_more_evidence_does_not_start_a_second_fence() {
    let t0 = Instant::now();
    let s = first_keyframe_lost(FAST, t0);
    let first = fence_at(&s).expect("the first latch is a reaction to fast pacing");
    // Three more distinct frames confirm the same latch; it is not new.
    let at = t0 + Duration::from_secs(1);
    let confirmed = {
        let mut loss = s.keyframe_loss.lock().unwrap();
        // More reports about the same keyframe are not new evidence.
        assert!(loss.unrecoverable(1, 2, FRAME, at).is_none());
        assert!(loss.abandoned(1, FRAME, at).is_none());
        for frame in 3..=4 {
            loss.sending_at(frame, false, at);
            assert!(loss.unrecoverable(frame, 4, FRAME, at).is_none());
        }
        loss.sending_at(5, false, at);
        loss.unrecoverable(5, 5, FRAME, at)
    }
    .expect("more evidence confirms the latch");
    assert!(confirmed.confirmed);
    std::thread::sleep(Duration::from_millis(5));
    s.report_keyframe_loss(Some(confirmed), None);
    assert_eq!(fence_at(&s), Some(first));
}
#[test]
fn a_latch_is_a_reaction_to_fast_pacing_only_if_fast_pacing_was_applied() {
    // The route allows 800 Mbps, but the frame went out at 50 Mbps (the
    // client's own link): the latch cannot be the whole fix.
    let s = first_keyframe_lost(50_000_000, Instant::now());
    assert!(!s.pace_was_fast.load(Ordering::Relaxed));
    assert_eq!(fence_at(&s), None);
    // The same loss after frames that really went out fast starts a fence.
    let s = first_keyframe_lost(FAST, Instant::now());
    assert!(fence_at(&s).is_some());
    // A frame paced at the Wi-Fi rate because of the latch is not fast,
    // whatever the route would have allowed.
    assert_eq!(
        s.keyframe_loss_bps_at(FAST, SAFE, 3, false, Instant::now()),
        SAFE
    );
    assert!(!s.pace_was_fast.load(Ordering::Relaxed));
}
#[test]
fn a_lifted_start_up_latch_gives_link_rate_one_fence_for_the_faster_pace() {
    for (base, fenced) in [(FAST, true), (50_000_000, false)] {
        let t0 = Instant::now();
        let s = first_keyframe_lost(base, t0);
        let held = fence_at(&s);
        assert_eq!(held.is_some(), fenced);
        let mut frame = 3;
        while s.keyframe_loss_active() {
            let at = t0 + FRAME * frame;
            s.keyframe_loss_bps_at(base, SAFE, frame, false, at);
            frame += 1;
            assert!(frame < 2_000, "the latch never lifted");
        }
        assert_eq!(s.pace_was_fast.load(Ordering::Relaxed), base > SAFE);
        if fenced {
            let lifted = fence_at(&s).unwrap();
            assert!(Some(lifted) > held, "the lift starts a fence");
        } else {
            assert_eq!(fence_at(&s), None, "the pace did not change");
        }
        assert!(s.launch.warnings.snapshot().is_empty());
    }
}
fn link_warning(s: &Session) -> Option<String> {
    s.launch
        .warnings
        .snapshot()
        .into_iter()
        .find(|w| w.code == LINK_WARNING)
        .map(|w| w.message)
}
#[test]
fn the_warning_and_the_console_agree_on_the_bitrate_and_nothing_is_said_under_the_lock() {
    let (s, end) = link_session(60, true);
    lose_frames(&s, 50..=54);
    // The change is worked out under the lock; the warning is not set
    // until the lock is released.
    let now = end + Duration::from_millis(200);
    let mut link = s.link_rate.lock().unwrap();
    link.drain(&s.link_sent);
    link.rate
        .tick(
            now,
            Duration::from_millis(10),
            s.fec_percent(),
            s.stream_mode().period(),
            20_000,
            true,
        )
        .expect("the controller lowers the bitrate");
    let change = link.rate.take_change().unwrap();
    let report = note_link_change(&mut link, now, change);
    drop(link);
    assert_eq!(link_warning(&s), None);
    s.announce_link_change(report);
    let message = link_warning(&s).expect("the console gets a warning");
    let lowest = s.link_rate.lock().unwrap().lowest_kbps;
    // "As low as", never a bare "lowered the bitrate to X", which the
    // badge's current rate would contradict once the host probes upward.
    assert!(
        message.contains(&format!("as low as {} so far", mbps(lowest))),
        "{message}"
    );
    assert!(!message.contains("bitrate to"), "{message}");
    assert!(message.contains("20 Mbps"), "{message}");
}
#[test]
fn an_encoder_floor_says_so_without_claiming_the_lowest_bitrate_is_too_high() {
    let s = Session::new(launch("link-floor", Role::Stream), Negotiated::default());
    let change = crate::link_rate::Change {
        kind: crate::link_rate::ChangeKind::EncoderFloor,
        reason: "encoder_floor",
        from_kbps: 9_000,
        to_kbps: 14_000,
        delivered_kbps: 17_000,
        evidence: Default::default(),
    };
    let mut link = s.link_rate.lock().unwrap();
    let report = note_link_change(&mut link, Instant::now(), change);
    drop(link);
    s.announce_link_change(report);
    let message = link_warning(&s).unwrap();
    assert!(message.contains("14 Mbps"), "{message}");
    assert!(message.contains("does not go lower"), "{message}");
    assert!(!message.contains("cannot carry even"), "{message}");
}
#[test]
fn a_queue_cut_says_the_network_held_video_back_not_that_packets_were_lost() {
    let s = Session::new(launch("link-queue", Role::Stream), Negotiated::default());
    let change = crate::link_rate::Change {
        kind: crate::link_rate::ChangeKind::StepDown,
        reason: "queue",
        from_kbps: 36_000,
        to_kbps: 31_700,
        delivered_kbps: 0,
        evidence: Default::default(),
    };
    let mut link = s.link_rate.lock().unwrap();
    let report = note_link_change(&mut link, Instant::now(), change);
    drop(link);
    s.announce_link_change(report);
    let message = link_warning(&s).expect("the console gets a warning");
    assert!(
        message.contains("holding video back in a growing queue"),
        "{message}"
    );
    assert!(!message.contains("lost"), "{message}");
    assert!(message.contains("as low as"), "{message}");
}
#[test]
fn keyframe_requests_count_only_for_a_client_that_sends_no_fec_status() {
    for reports_fec in [false, true] {
        let (s, end) = link_session(60, true);
        if reports_fec {
            s.record_fec_status(&fec_report(40, 10, 2));
        }
        for _ in 0..4 {
            s.request_client_keyframe();
        }
        let cut = s.tick_link_rate(end + Duration::from_millis(200), Duration::from_millis(10));
        assert_eq!(cut.is_some(), !reports_fec, "reports_fec {reports_fec}");
    }
}
fn pacing_traffic(s: &Session, frames: u64) {
    s.stats.frames.store(frames, Ordering::Relaxed);
    s.stats.packets.store(frames * 70, Ordering::Relaxed);
    s.stats.bytes.store(frames * 83_334, Ordering::Relaxed);
    // The wire frame marker stores last_sent + 1, not last_sent.
    s.stats.video_frame.store(frames + 1, Ordering::Release);
}
#[test]
fn pyrowave_loss_controls_adapt_without_forcing_extra_frames() {
    for records in [false, true] {
        let s = Session::new(
            launch("pyrowave-loss", Role::Stream),
            Negotiated {
                codec: 3,
                pyrowave_records: records,
                bitrate_kbps: 200_000,
                configured_bitrate_kbps: 250_000,
                pyrowave_client_pace_mbps: 150,
                ..Default::default()
            },
        );
        s.idr.store(false, Ordering::Release);
        s.configure_keyframe_loss(true);
        s.configure_adaptive_bitrate(true, 0);
        let start = Instant::now();
        s.tick_adaptive_bitrate(start, Duration::from_millis(5));
        s.keyframe_loss_bps(1_000_000_000, 250_000_000, 1, true);
        s.stats.video_frame.store(2, Ordering::Release);
        s.record_fec_status(&fec_report(1, 7, 1));
        assert_eq!(
            s.keyframe_loss_bps(1_000_000_000, 250_000_000, 2, true),
            250_000_000
        );
        let (_, target) = s
            .tick_adaptive_bitrate(start + Duration::from_secs(1), Duration::from_millis(5))
            .unwrap();
        assert!(target < 150_000);
        assert!(s.adaptive_pacing_bps(u64::MAX) <= 150_000_000);
        assert!(!s.idr.load(Ordering::Acquire));
        assert!(s.invalidation.lock().unwrap().is_none());
        s.set_client_bitrate(180_000);
        s.record_fec_status(&fec_report(2, 7, 1));
        s.tick_adaptive_bitrate(start + Duration::from_secs(3), Duration::from_millis(5));
        assert_eq!(s.bitrate.load(Ordering::Acquire), 180_000);
    }
}

#[test]
fn pyrowave_packet_adaptation_responds_to_distinct_lost_frames() {
    let s = Session::new(
        launch("pyrowave-pacing", Role::Stream),
        Negotiated {
            codec: 3,
            bitrate_kbps: 40_000,
            ..Default::default()
        },
    );
    s.configure_automatic_pacing(true);
    let start = Instant::now();
    assert_eq!(s.video_pacing_bps(150_000_000, 50_000_000), 80_000_000);
    s.tick_automatic_pacing(start, Duration::from_millis(2));
    pacing_traffic(&s, 60);
    s.record_fec_status(&fec_report(59, 7, 1));
    s.record_fec_status(&fec_report(60, 7, 1));
    assert_eq!(
        s.tick_automatic_pacing(start + Duration::from_secs(1), Duration::from_millis(2)),
        Some((80_000_000, 62_500_000))
    );
    assert_eq!(s.bitrate.load(Ordering::Acquire), 40_000);
    // A measured path below demand remains the hard ceiling.
    assert_eq!(s.video_pacing_bps(30_000_000, 50_000_000), 30_000_000);
}

#[test]
fn a_wireless_client_stream_starts_at_the_controllers_seed_and_climbs_toward_the_route_ceiling() {
    let config = crate::config::Config::default();
    for (wireless, expected) in [(true, 126_000_000), (false, 800_000_000)] {
        let negotiated = Negotiated {
            codec: 1,
            bitrate_kbps: 63_000,
            client_wireless: wireless,
            ..Default::default()
        };
        let s = Session::new(launch("wireless-start", Role::Stream), negotiated);
        s.configure_automatic_pacing(
            config.adaptive_packet_pacing(s.config.codec, s.config.client_wireless),
        );
        // The sender passes the host route's base (800 Mbps on a wired 1 GbE
        // host) to the controller: its seed gives the conservative start.
        assert_eq!(s.automatic_pacing_enabled(), wireless);
        assert!(!s.client_wireless_sets_route_pacing());
        assert_eq!(s.video_pacing_bps(800_000_000, 75_000_000), expected);
        if !wireless {
            continue;
        }
        // Clean feedback lets the rate climb above twice the bitrate, in
        // steps of at most 12.5%, and never past the route ceiling.
        let start = Instant::now();
        s.tick_automatic_pacing(start, Duration::from_millis(2));
        let mut rate = expected;
        for step in 1..=40u64 {
            let now = start + Duration::from_millis(500 * step);
            pacing_traffic(&s, 30 * step);
            s.record_control_ping(now, &[4, 0, 0, 0, 0, 0, 0, 0]);
            s.tick_automatic_pacing(now, Duration::from_millis(2));
            let next = s.video_pacing_bps(800_000_000, 75_000_000);
            assert!(next >= rate && next <= rate + rate / 8, "{rate} -> {next}");
            assert!(next <= 800_000_000);
            rate = next;
        }
        assert!(rate > 2 * 63_000_000, "never climbed: {rate}");
    }
    // Without the controller the hint sets the route pacing instead.
    for (setting, codec) in [
        ("wireless_adaptive_pacing=false", 1),
        ("", 3),
        ("pacing_max_bitrate_kbps=80000", 1),
        ("host_adaptive_bitrate=true", 1),
    ] {
        let config = crate::config::Config::parse(setting).unwrap();
        let s = Session::new(
            launch("wireless-hint", Role::Stream),
            Negotiated {
                codec,
                client_wireless: true,
                ..Default::default()
            },
        );
        s.configure_automatic_pacing(config.adaptive_packet_pacing(codec, true));
        assert!(
            s.client_wireless_sets_route_pacing(),
            "{setting} codec {codec}"
        );
    }
    let wired = Session::new(launch("wired", Role::Stream), Negotiated::default());
    assert!(!wired.client_wireless_sets_route_pacing());
}

#[test]
fn pyrowave_fec_adapts_for_records_and_legacy_clients() {
    for records in [false, true] {
        let s = Session::new(
            launch("pyrowave-fec", Role::Stream),
            Negotiated {
                codec: 3,
                pyrowave_records: records,
                ..Default::default()
            },
        );
        assert_eq!(s.configure_fec(true, 200), 200);
        s.stats.video_frame.store(3, Ordering::Release);
        s.record_fec_status(&fec_report(1, 9, 1));
        s.tick_fec(
            Instant::now() + crate::adaptive_fec::AdaptiveFec::CLEAN_HOLD + Duration::from_secs(1),
        );
        assert_eq!(s.fec_percent(), 199);
        s.record_fec_status(&fec_report(2, 7, 1));
        assert_eq!(s.fec_percent(), 200);
    }
}
#[test]
fn automatic_pacing_uses_validated_new_losses_without_changing_stream_quality() {
    let s = Session::new(
        launch("automatic-pacing", Role::Stream),
        Negotiated {
            bitrate_kbps: 40_000,
            ..Default::default()
        },
    );
    s.configure_automatic_pacing(true);
    s.idr.store(false, Ordering::Relaxed);
    let mode = s.stream_mode();
    let start = Instant::now();
    assert_eq!(s.video_pacing_bps(800_000_000, 50_000_000), 80_000_000);
    s.tick_automatic_pacing(start, Duration::from_millis(2));
    s.record_fec_status(&fec_report(1, 7, 1)); // No sent frames yet.
    s.record_fec_status(&fec_report(2, 7, 1));
    pacing_traffic(&s, 30);
    s.record_fec_status(&fec_report(29, 7, 1)[..20]); // Bad length.
    s.record_fec_status(&fec_report(30, 11, 1)); // Impossible count.
    s.record_fec_status(&fec_report(31, 7, 1)); // Future frames.
    s.record_fec_status(&fec_report(32, 7, 1));
    s.record_fec_status(&fec_report(29, 7, 1));
    s.record_fec_status(&fec_report(29, 7, 1)); // Duplicate block.
    assert_eq!(
        s.tick_automatic_pacing(start + Duration::from_millis(500), Duration::from_millis(2)),
        None
    );
    // The invalid reports did not reserve these frame/block identities.
    pacing_traffic(&s, 60);
    s.record_fec_status(&fec_report(30, 7, 1));
    s.record_fec_status(&fec_report(31, 7, 1));
    assert_eq!(
        s.tick_automatic_pacing(start + Duration::from_secs(1), Duration::from_millis(2)),
        Some((80_000_000, 62_500_000))
    );
    pacing_traffic(&s, 90);
    // These unseen blocks are valid telemetry, but describe the old rate.
    s.record_fec_status(&fec_report(58, 7, 1));
    s.record_fec_status(&fec_report(60, 7, 1));
    assert_eq!(
        s.tick_automatic_pacing(
            start + Duration::from_millis(1500),
            Duration::from_millis(2)
        ),
        None
    );
    pacing_traffic(&s, 120);
    s.record_fec_status(&fec_report(119, 7, 1));
    s.record_fec_status(&fec_report(120, 7, 1));
    assert_eq!(
        s.tick_automatic_pacing(start + Duration::from_secs(2), Duration::from_millis(2)),
        None // Already at the live wire-demand floor.
    );
    assert_eq!(s.video_pacing_bps(800_000_000, 50_000_000), 62_500_000);
    assert_eq!(s.bitrate.load(Ordering::Acquire), 40_000);
    assert_eq!(s.stream_mode(), mode);
    assert!(!s.idr.load(Ordering::Relaxed));
    assert!(s.invalidation.lock().unwrap().is_none());
    assert_eq!(s.stats.idr_requests.load(Ordering::Relaxed), 0);
    assert_eq!(s.stats.send_loss_recoveries.load(Ordering::Relaxed), 0);
    assert!(!s.stopping());
    let performance = &s.info()["performance"];
    assert_eq!(performance["fec_invalid_reports"], 6);
    assert_eq!(performance["fec_duplicate_reports"], 1);
}
#[test]
fn automatic_pacing_respects_client_bitrate_changes_and_updated_fec_wire_demand() {
    let s = Session::new(
        launch("pacing-client-bitrate", Role::Stream),
        Negotiated {
            bitrate_kbps: 40_000,
            configured_bitrate_kbps: 60_000,
            packet_size: 1392,
            min_fec: 2,
            ..Default::default()
        },
    );
    s.configure_automatic_pacing(true);
    s.configure_adaptive_bitrate(true, 0);
    s.configure_fec(false, 20);
    s.set_client_bitrate(40_000);
    s.idr.store(false, Ordering::Relaxed);
    let mode = s.stream_mode();
    let start = Instant::now();
    let mut packetizer = crate::packet::VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: s.config.packet_size,
        fec_percent: s.fec_percent(),
        min_fec: s.config.min_fec,
        key: Some([0; 16]),
    };
    let needed = crate::network_pacing::video_wire_rate_bps(
        &packetizer,
        s.bitrate.load(Ordering::Acquire),
        mode.period(),
        false,
    );
    s.video_pacing_bps(800_000_000, needed);
    s.tick_automatic_pacing(start, Duration::from_millis(2));
    for interval in 1..=4 {
        let frame = interval * 30;
        pacing_traffic(&s, frame);
        s.record_fec_status(&fec_report(frame as u32 - 1, 7, 1));
        s.record_fec_status(&fec_report(frame as u32, 7, 1));
        let now = start + Duration::from_millis(interval * 500);
        let change = s.tick_automatic_pacing(now, Duration::from_millis(2));
        // Startup is already close to wire demand, so the first cut
        // reaches the floor; repeated loss must not push below it.
        assert_eq!(change.is_some(), interval == 1);
        assert_eq!(s.video_pacing_bps(800_000_000, needed), needed + needed / 4);
        assert_eq!(s.tick_adaptive_bitrate(now, Duration::from_millis(2)), None);
        assert_eq!(s.bitrate.load(Ordering::Acquire), 40_000);
    }
    let previous = s.video_pacing_bps(800_000_000, needed);
    assert_eq!(previous, needed + needed / 4);
    // The real /bitrate writer retains ownership. The sender supplies a
    // refreshed wire budget when bitrate or FEC changes between frames.
    s.set_client_bitrate(80_000);
    packetizer.fec_percent = s.configure_fec(false, 50);
    let needed = crate::network_pacing::video_wire_rate_bps(
        &packetizer,
        s.bitrate.load(Ordering::Acquire),
        mode.period(),
        false,
    );
    let raised = s.video_pacing_bps(800_000_000, needed);
    assert!(raised > previous);
    assert_eq!(raised, needed + needed / 4);
    pacing_traffic(&s, 150);
    s.record_fec_status(&fec_report(149, 7, 1));
    s.record_fec_status(&fec_report(150, 7, 1));
    let now = start + Duration::from_millis(2500);
    assert_eq!(
        s.tick_automatic_pacing(now, Duration::from_millis(200)),
        None
    );
    assert_eq!(
        s.tick_adaptive_bitrate(now, Duration::from_millis(200)),
        None
    );
    assert_eq!(s.bitrate.load(Ordering::Acquire), 80_000);
    assert_eq!(s.info()["adaptive_bitrate"]["owner"], "client");
    assert_eq!(s.stream_mode(), mode);
    assert!(!s.idr.load(Ordering::Relaxed));
}
#[test]
fn disabled_automatic_pacing_preserves_the_base_rate() {
    for (enabled, codec) in [(false, 0), (false, 1), (false, 3)] {
        let s = Session::new(
            launch("pacing-bypass", Role::Stream),
            Negotiated {
                codec,
                ..Default::default()
            },
        );
        s.configure_automatic_pacing(enabled);
        assert_eq!(s.video_pacing_bps(950_000_000, 50_000_000), 950_000_000);
        let start = Instant::now();
        s.tick_automatic_pacing(start, Duration::from_millis(2));
        pacing_traffic(&s, 30);
        s.record_fec_status(&fec_report(29, 7, 1));
        s.record_fec_status(&fec_report(30, 7, 1));
        assert_eq!(
            s.tick_automatic_pacing(start + Duration::from_secs(1), Duration::from_millis(2)),
            None
        );
        assert_eq!(s.video_pacing_bps(950_000_000, 50_000_000), 950_000_000);
        let info = s.info();
        assert_eq!(info["packet_pacing"]["automatic"], false);
        assert_eq!(info["performance"]["fec_unrecoverable_frames"], 2);
    }
}
#[test]
fn automatic_pacing_guards_first_frame_and_each_reconnect() {
    let start = Instant::now();
    let launch = launch("pacing-reconnect", Role::Stream);
    let negotiated = Negotiated {
        bitrate_kbps: 40_000,
        ..Default::default()
    };
    let first = Session::new(launch.clone(), negotiated.clone());
    first.configure_automatic_pacing(true);
    assert_eq!(first.stats.frames.load(Ordering::Relaxed), 0);
    assert_eq!(first.video_pacing_bps(800_000_000, 50_000_000), 80_000_000);
    first.tick_automatic_pacing(start, Duration::from_millis(2));
    let ping = [4, 0, 0, 0, 0, 0, 0, 0];
    for interval in 1..=30 {
        let now = start + Duration::from_millis(interval * 500);
        pacing_traffic(&first, interval * 30);
        first.record_control_ping(now, &ping);
        first.tick_automatic_pacing(now, Duration::from_millis(2));
        first.video_pacing_bps(800_000_000, 50_000_000);
    }
    assert_eq!(first.video_pacing_bps(800_000_000, 50_000_000), 800_000_000);
    assert_eq!(first.bitrate.load(Ordering::Acquire), 40_000);
    // A client can change its downstream connection without changing
    // its identity or host route. Never reuse an old fast rate at startup.
    let reconnect = Session::new(launch, negotiated);
    reconnect.configure_automatic_pacing(true);
    assert_eq!(reconnect.stats.frames.load(Ordering::Relaxed), 0);
    assert_eq!(
        reconnect.video_pacing_bps(800_000_000, 50_000_000),
        80_000_000
    );
    assert_eq!(first.video_pacing_bps(800_000_000, 50_000_000), 800_000_000);
    assert_eq!(reconnect.bitrate.load(Ordering::Acquire), 40_000);
}
#[test]
fn a_client_fec_share_splits_the_encoder_budget_at_a_fence_or_a_report() {
    use crate::extensions::FecChoice;
    use crate::statistical_fec::FecReport;
    let report = |seq, missing| {
        FecReport {
            seq,
            data_shards: 900,
            parity_shards: 180,
            missing_shards: missing,
            unrecovered_blocks: 0,
            interval_ms: 1000,
        }
        .encode()
    };
    // The host's own FEC: the encoder gets the whole bitrate, as before.
    let host = Session::new(launch("host", Role::Stream), Negotiated::default());
    host.configure_fec_choice(FecChoice::Host, true, 20, 50);
    host.bitrate.store(12_000, Ordering::Release);
    assert_eq!(host.encoder_bitrate(true), 12_000);

    // 12 Mbps at 60 fps: about 25 data shards a frame.
    let stream = Negotiated {
        packet_size: 1024,
        fps: 60,
        ..Default::default()
    };
    let auto = Session::new(launch("auto", Role::Stream), stream);
    assert_eq!(auto.configure_fec_choice(FecChoice::Auto, true, 20, 40), 20);
    let start = Instant::now();
    auto.bitrate.store(12_000, Ordering::Release);
    assert_eq!(auto.encoder_bitrate(false), 10_000, "12 Mbps at 20 % FEC");
    auto.record_fec_report_at(&report(0, 0), start).unwrap();
    assert_eq!(auto.fec_percent(), 0);
    assert_eq!(
        auto.encoder_bitrate(false),
        12_000,
        "the first report replaces the fallback share without an IDR"
    );
    assert_eq!(
        auto.encoder_bitrate(true),
        12_000,
        "a clean link: all video"
    );
    auto.record_fec_report_at(&report(1, 540), start + Duration::from_secs(1))
        .unwrap();
    assert_eq!(auto.fec_percent(), 40);
    assert_eq!(
        auto.encoder_bitrate(false),
        8_571,
        "rising loss moves the split at once: 12 Mbps × 100 / 140"
    );
    let within = |s: &Session| {
        u64::from(s.encoder_bitrate(false)) * (100 + s.fec_percent() as u64) / 100 <= 12_000
    };
    assert!(within(&auto), "video and parity fit the budget");
    assert_eq!(auto.encoder_bitrate(true), 8_571);
    // A small fall keeps the split until a fence, a larger one moves it.
    auto.fec_percent.store(36, Ordering::Release);
    auto.follow_fec_report(false);
    assert_eq!(auto.encoder_bitrate(false), 8_571, "within the hysteresis");
    auto.fec_percent.store(30, Ordering::Release);
    auto.follow_fec_report(false);
    assert_eq!(auto.encoder_bitrate(false), 9_230, "12 Mbps × 100 / 130");
    auto.fec_percent.store(31, Ordering::Release);
    auto.follow_fec_report(false);
    assert_eq!(auto.encoder_bitrate(false), 9_160, "any rise moves it");
    assert!(within(&auto));
    // A new budget (a client /bitrate) applies at once, at the latched share.
    auto.bitrate.store(7_000, Ordering::Release);
    assert_eq!(auto.encoder_bitrate(false), 5_343);
    // A host-FEC session never follows reports.
    host.follow_fec_report(true);
    assert_eq!(host.encoder_bitrate(false), 12_000);

    let fixed = Session::new(launch("fixed", Role::Stream), Negotiated::default());
    assert_eq!(
        fixed.configure_fec_choice(FecChoice::Fixed(90), true, 20, 50),
        50
    );
    fixed.bitrate.store(12_000, Ordering::Release);
    assert_eq!(fixed.encoder_bitrate(true), 8_000);
    // Back to the host's FEC on a new configuration: no split.
    fixed.configure_fec_choice(FecChoice::Host, true, 20, 50);
    assert_eq!(fixed.encoder_bitrate(true), 12_000);
    assert_eq!(fec_split_kbps(0, 50), 0);
    assert_eq!(fec_split_kbps(1, 255), 1);
}
#[test]
fn auto_fec_keeps_the_configured_share_for_recovery_frames() {
    use crate::extensions::FecChoice;
    use crate::statistical_fec::FecReport;
    let report = |seq, missing| {
        FecReport {
            seq,
            data_shards: 900,
            parity_shards: 180,
            missing_shards: missing,
            unrecovered_blocks: 0,
            interval_ms: 1000,
        }
        .encode()
    };
    let stream = Negotiated {
        packet_size: 1024,
        fps: 60,
        ..Default::default()
    };
    let auto = Session::new(launch("auto-recovery", Role::Stream), stream);
    auto.configure_fec_choice(FecChoice::Auto, true, 20, 40);
    auto.bitrate.store(12_000, Ordering::Release);
    let start = Instant::now();
    auto.record_fec_report_at(&report(0, 0), start).unwrap();
    // A clean link: ordinary frames go without parity, but a keyframe or
    // the frame after an invalidation keeps the configured 20 %, and the
    // recovery budget is planned with the same share.
    assert_eq!(auto.video_fec_percent(false, 30_000), 0);
    assert_eq!(auto.video_fec_percent(true, 30_000), 20);
    assert_eq!(auto.video_fec_percent(true, 3_000), 20);
    assert_eq!(auto.recovery_fec_percent(), 20);
    // Above the configured share the statistical choice wins.
    auto.record_fec_report_at(&report(1, 540), start + Duration::from_secs(1))
        .unwrap();
    assert_eq!(auto.video_fec_percent(false, 30_000), 40);
    assert_eq!(auto.video_fec_percent(true, 30_000), 40);
    assert_eq!(auto.recovery_fec_percent(), 40);

    // Without Auto the plan is the host's recovery share, as before.
    let host = Session::new(launch("host-recovery", Role::Stream), Negotiated::default());
    host.configure_fec_choice(FecChoice::Host, true, 20, 50);
    assert_eq!(host.recovery_fec_percent(), host.frame_fec_percent(true));
    let fixed = Session::new(
        launch("fixed-recovery", Role::Stream),
        Negotiated::default(),
    );
    fixed.configure_fec_choice(FecChoice::Fixed(10), true, 20, 50);
    assert_eq!(fixed.video_fec_percent(true, 30_000), 10);
    assert_eq!(fixed.recovery_fec_percent(), 10);
}
#[test]
fn auto_fec_finds_the_largest_recovery_frame_its_own_percentage_covers_in_full() {
    use crate::extensions::FecChoice;
    use crate::statistical_fec::FecReport;
    let report = |seq, missing| {
        FecReport {
            seq,
            data_shards: 900,
            parity_shards: 180,
            missing_shards: missing,
            unrecovered_blocks: 0,
            interval_ms: 1000,
        }
        .encode()
    };
    let stream = Negotiated {
        packet_size: 1392,
        fps: 60,
        ..Default::default()
    };
    let packetizer = |percent| crate::packet::VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: percent,
        min_fec: stream.min_fec,
        key: Some([0; 16]),
    };
    let auto = Session::new(launch("auto-whole", Role::Stream), stream.clone());
    auto.configure_fec_choice(FecChoice::Auto, true, 20, 40);
    auto.bitrate.store(200_000, Ordering::Release);
    let start = Instant::now();
    // Only a fixed choice: the packetizer's own limit applies.
    let fixed = Session::new(launch("fixed-whole", Role::Stream), stream.clone());
    fixed.configure_fec_choice(FecChoice::Fixed(20), true, 20, 40);
    assert_eq!(fixed.statistical_fec_whole_payload(start), None);
    // A clean link: recovery frames keep the configured 20 %, so the limit
    // is the fixed one.
    auto.record_fec_report_at(&report(0, 0), start).unwrap();
    let clean = auto.statistical_fec_whole_payload(start).unwrap();
    assert_eq!(clean, packetizer(20).fec_whole_payload_limit());
    // A lossy link raises an average frame's share to 40 %, but a large
    // frame gets the most parity it can carry in full, and a recovery
    // frame at least the configured 20 %: the limit is the 20 % one, not
    // the smaller 40 % limit the average frame's share would give.
    let later = start + Duration::from_secs(1);
    auto.record_fec_report_at(&report(1, 540), later).unwrap();
    assert_eq!(auto.recovery_fec_percent(), 40);
    let lossy = auto.statistical_fec_whole_payload(later).unwrap();
    assert_eq!(lossy, clean);
    assert!(lossy > packetizer(40).fec_whole_payload_limit());
    let percent = |payload| auto.video_fec_percent(true, payload);
    assert!(!packetizer(percent(lossy)).fec_limited(lossy));
    // The next shard is a frame its own percentage no longer covers.
    let next = lossy + 1376;
    assert!(packetizer(percent(next)).fec_limited(next));
}
#[test]
fn adaptive_fec_follows_reports_and_stays_fixed_without_them_or_when_off() {
    let start = Instant::now();
    // A client that never reports: the configured percentage, for minutes.
    let quiet = Session::new(launch("quiet", Role::Stream), Negotiated::default());
    assert_eq!(quiet.configure_fec(true, 30), 30);
    for second in 0..300 {
        assert_eq!(quiet.tick_fec(start + Duration::from_secs(second)), None);
    }
    assert_eq!(quiet.fec_percent(), 30);
    assert_eq!(quiet.info()["fec_percent"], 30);
    // Off: reports are still counted, the percentage never moves.
    let off = Session::new(launch("off", Role::Stream), Negotiated::default());
    assert_eq!(off.configure_fec(false, 20), 20);
    off.stats.video_frame.store(100, Ordering::Release);
    off.record_fec_status(&fec_report(10, 10, 0));
    off.record_fec_status(&fec_report(11, 7, 1));
    for second in 0..120 {
        assert_eq!(off.tick_fec(start + Duration::from_secs(second)), None);
    }
    assert_eq!(off.fec_percent(), 20);
    assert_eq!(off.info()["performance"]["fec_unrecoverable_frames"], 1);
    // On: a clean link decays, a rebuilt frame raises, a lost one restores.
    let on = Session::new(launch("on", Role::Stream), Negotiated::default());
    assert_eq!(on.configure_fec(true, 20), 20);
    on.stats.video_frame.store(100, Ordering::Release);
    on.record_fec_status(&fec_report(10, 10, 0));
    let now = Instant::now();
    assert_eq!(on.tick_fec(now), None);
    let later = now + Duration::from_secs(60);
    on.tick_fec(later);
    assert!(on.fec_percent() < 20);
    let decayed = on.fec_percent();
    on.record_fec_status(&fec_report(20, 9, 1));
    assert!(on.fec_percent() > decayed);
    on.record_fec_status(&fec_report(21, 7, 1));
    assert_eq!(on.fec_percent(), 20);
    // Recovery frames keep the configured percentage on a decayed link.
    on.tick_fec(later + Duration::from_secs(120));
    let decayed = on.fec_percent();
    assert!(decayed < 20);
    assert_eq!(on.frame_fec_percent(false), decayed);
    assert_eq!(on.frame_fec_percent(true), 20);
    assert_eq!(off.frame_fec_percent(true), 20);
    assert_eq!(quiet.frame_fec_percent(false), 30);
    // PyroWave reports its own configured and adapted protection.
    let pyrowave = Session::new(
        launch("pyrowave-fec", Role::Stream),
        Negotiated {
            codec: 3,
            ..Default::default()
        },
    );
    pyrowave.configure_fec(true, 200);
    assert_eq!(pyrowave.info()["fec_percent"], 200);
}
#[test]
fn client_luminance_changes_never_relabel_hdr_content() {
    use rubylight_protocol::control::DisplayCaps;
    let display = crate::hdr::Metadata::display(1000., 0.01, 600.);
    for hdr in [false, true] {
        let s = Session::new(
            launch("hdr", Role::Stream),
            Negotiated {
                hdr,
                ..Default::default()
            },
        );
        let mut caps = DisplayCaps {
            hdr: true,
            max_centinits: 260_000,
            max_average_centinits: 120_000,
            min_decimillinits: 5,
        };
        // Early reports must not start HDR with the receiver's luminance.
        assert!(s.record_display_caps(&caps.encode()).unwrap().changed);
        assert!(s.hdr_metadata.read().unwrap().is_none());
        assert_eq!(s.set_display_hdr_metadata(display), display);
        // Brighter, dimmer, unknown and SDR receiver reports all preserve
        // the source metadata, preventing HDR control updates/codec restarts.
        for peak in [40_000, 100_000, 260_000, 0] {
            caps.max_centinits = peak;
            caps.max_average_centinits = 0;
            caps.hdr = peak != 0;
            s.record_display_caps(&caps.encode()).unwrap();
            assert_eq!(*s.hdr_metadata.read().unwrap(), Some(display));
            assert_eq!(s.set_display_hdr_metadata(display), display);
        }
        assert!(s.record_display_caps(&caps.encode()[..8]).is_none());
        assert_eq!(*s.hdr_metadata.read().unwrap(), Some(display));
        // A real source change must still reach the encoder and client.
        let new_source = crate::hdr::Metadata::display(1500., 0.001, 800.);
        assert_eq!(s.set_display_hdr_metadata(new_source), new_source);
        assert_eq!(*s.hdr_metadata.read().unwrap(), Some(new_source));
    }
}
#[test]
fn a_launch_still_being_prepared_does_not_expire() {
    let mut sessions = Sessions::default();
    let mut slow = launch("slow", Role::Stream);
    slow.created = Instant::now() - Duration::from_secs(90);
    slow.preparing.store(true, Ordering::Release);
    sessions.pending.insert(slow.id.clone(), slow.clone());
    drop(sessions.expire());
    assert!(sessions.pending.contains_key("slow"));
    // Prepared: the client's 30 s count from then.
    slow.preparing.store(false, Ordering::Release);
    drop(sessions.expire());
    assert!(!sessions.pending.contains_key("slow"));
}
#[test]
fn starting_rechecks_launch_expiry_after_rtsp_authorization() {
    let mut sessions = Sessions::default();
    let fresh = launch("expired-before-play", Role::Stream);
    sessions.pending.insert(fresh.id.clone(), fresh.clone());
    let authorized = sessions.pending_for_peer(fresh.peer).unwrap();
    // The RTSP connection can wait for input after taking its authorized
    // snapshot. PLAY must check the current launch, even before housekeeping.
    sessions.pending.get_mut(&fresh.id).unwrap().created = Instant::now() - Duration::from_secs(31);
    assert!(sessions.start(authorized, Negotiated::default()).is_err());
    assert!(sessions.active.is_empty());
    // Keep preparation ownership for expire() to return to its caller;
    // restoring display/audio resources must happen outside the session lock.
    assert!(sessions.pending.contains_key(&fresh.id));
    assert_eq!(sessions.expire().len(), 1);

    let fresh = launch("valid-play", Role::Stream);
    sessions.pending.insert(fresh.id.clone(), fresh.clone());
    assert!(sessions.start(fresh.clone(), Negotiated::default()).is_ok());
    assert!(sessions.start(fresh, Negotiated::default()).is_err());
}
#[test]
fn a_launch_takes_rtsp_only_once_prepared_and_starts_one_session() {
    let mut sessions = Sessions::default();
    let early = launch("early", Role::Stream);
    early.preparing.store(true, Ordering::Release);
    sessions.pending.insert(early.id.clone(), early.clone());
    // Its client connects only after the reply: nothing may claim or
    // start it while the display and app are still being prepared.
    assert!(sessions.rtsp_for_peer(early.peer).is_empty());
    assert!(sessions.pending_for_peer(early.peer).is_err());
    let refused = sessions.claim("early").unwrap_err().to_string();
    assert!(refused.contains("still being prepared"), "{refused}");
    assert!(
        sessions
            .start(early.clone(), Negotiated::default())
            .is_err()
    );
    assert!(sessions.active.is_empty() && sessions.pending.contains_key("early"));
    drop(sessions.expire());
    assert!(sessions.pending.contains_key("early"));

    early.preparing.store(false, Ordering::Release);
    assert_eq!(sessions.rtsp_for_peer(early.peer).len(), 1);
    sessions.claim("early").unwrap();
    let session = sessions
        .start(early.clone(), Negotiated::default())
        .unwrap();
    // A second PLAY, or a claim of the consumed launch, starts nothing.
    assert!(sessions.start(early, Negotiated::default()).is_err());
    assert_eq!(sessions.active.len(), 1);
    // The running stream's own requests (TEARDOWN) still pass.
    sessions.claim("early").unwrap();
    sessions.begin_teardown(&session);
    assert!(sessions.claim("early").is_err());
}
#[test]
fn a_claimed_handshake_outlives_the_30_s_by_its_lease_up_to_the_limit() {
    let mut sessions = Sessions::default();
    let claimed = launch("claimed", Role::Stream);
    let unclaimed = launch("unclaimed", Role::Stream);
    sessions.pending.insert(claimed.id.clone(), claimed.clone());
    sessions
        .pending
        .insert(unclaimed.id.clone(), unclaimed.clone());
    sessions.claim("claimed").unwrap();
    let until = sessions.pending["claimed"].claimed_until.unwrap();
    assert!(until > Instant::now() + RTSP_LEASE - Duration::from_secs(1));
    // 31 s after the reply: the claimed handshake goes on, the other expired.
    for id in ["claimed", "unclaimed"] {
        sessions.pending.get_mut(id).unwrap().created = Instant::now() - Duration::from_secs(31);
    }
    let expired = sessions.expire();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].id, "unclaimed");
    assert!(sessions.claim("unclaimed").is_err());
    sessions.claim("claimed").unwrap();
    // A lease never reaches past the limit, and an ended lease expires.
    let at = sessions.pending["claimed"].created;
    assert_eq!(
        sessions.pending["claimed"].lease(at + CLAIM_LIMIT - Duration::from_secs(1)),
        at + CLAIM_LIMIT
    );
    sessions.pending.get_mut("claimed").unwrap().claimed_until =
        Some(Instant::now() - Duration::from_millis(1));
    let ended = sessions.claim("claimed").unwrap_err().to_string();
    assert!(ended.contains("expired"), "{ended}");
    sessions.pending.get_mut("claimed").unwrap().claimed_until = Some(Instant::now() + RTSP_LEASE);
    sessions.pending.get_mut("claimed").unwrap().created = Instant::now() - CLAIM_LIMIT;
    assert!(sessions.start(claimed, Negotiated::default()).is_err());
    assert_eq!(sessions.expire().len(), 1);
    assert!(sessions.pending.is_empty() && sessions.active.is_empty());
}
#[test]
fn a_reconnect_replaces_a_claimed_launch_which_then_starts_nothing() {
    let mut sessions = Sessions::default();
    let first = launch("first", Role::Stream);
    drop(sessions.queue(first.clone()).unwrap());
    sessions.claim("first").unwrap();
    // The same device launches again while the first handshake is under way.
    let replaced = sessions.queue(launch("second", Role::Stream)).unwrap();
    assert_eq!(replaced.len(), 1);
    assert_eq!(replaced[0].id, "first");
    assert!(sessions.claim("first").is_err());
    assert!(sessions.start(first, Negotiated::default()).is_err());
    sessions.claim("second").unwrap();
    let second = sessions.pending["second"].clone();
    sessions.start(second, Negotiated::default()).unwrap();
    assert_eq!(sessions.active.keys().collect::<Vec<_>>(), ["second"]);
}
#[test]
fn rtsp_authorization_does_not_keep_canceled_preparations_alive() {
    use std::sync::{Mutex, Weak};
    struct Restore {
        sessions: Weak<Mutex<Sessions>>,
        restored: Arc<AtomicU32>,
    }
    impl Preparation<()> for Restore {
        fn prepared(&self) -> &() {
            &()
        }
        fn into_prepared(self: Box<Self>) {}
    }
    impl AudioPreparation<()> for Restore {
        fn route(&self) -> Arc<()> {
            Arc::new(())
        }
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            assert!(self.sessions.upgrade().unwrap().try_lock().is_ok());
            self.restored.fetch_add(1, Ordering::Release);
        }
    }
    for expires in [false, true] {
        let sessions = Arc::new(Mutex::new(Sessions::default()));
        let restored = Arc::new(AtomicU32::new(0));
        let pending = launch("pending", Role::Stream);
        *pending.preparation.lock().unwrap() = Some(Box::new(Restore {
            sessions: Arc::downgrade(&sessions),
            restored: restored.clone(),
        }));
        *pending.audio_preparation.lock().unwrap() = Some(Box::new(Restore {
            sessions: Arc::downgrade(&sessions),
            restored: restored.clone(),
        }));
        let (authorized, canceled) = {
            let mut locked = sessions.lock().unwrap();
            let peer = pending.peer;
            locked.queue(pending).unwrap();
            let authorized = locked.rtsp_for_peer(peer);
            let canceled = if expires {
                locked.pending.get_mut("pending").unwrap().created =
                    Instant::now() - Duration::from_secs(31);
                locked.expire()
            } else {
                locked.request_stop(Some("pending"))
            };
            (authorized, canceled)
        };
        drop(canceled);
        assert_eq!(
            restored.load(Ordering::Acquire),
            2,
            "an RTSP socket must not retain canceled display and audio preparation"
        );
        assert_eq!(authorized[0].id, "pending");
    }
}
#[test]
fn play_consumes_the_current_pending_launch_with_its_preparation() {
    struct Prepared;
    impl Preparation<()> for Prepared {
        fn prepared(&self) -> &() {
            &()
        }
        fn into_prepared(self: Box<Self>) {}
    }
    let mut sessions = Sessions::default();
    let pending = launch("prepared", Role::Stream);
    let peer = pending.peer;
    *pending.preparation.lock().unwrap() = Some(Box::new(Prepared));
    sessions.queue(pending).unwrap();
    let authorized = sessions.rtsp_for_peer(peer).pop().unwrap();
    sessions
        .pending
        .get_mut("prepared")
        .unwrap()
        .options
        .insert("server_state".into(), "current".into());
    let session = sessions.start(authorized, Negotiated::default()).unwrap();
    assert_eq!(
        session
            .launch
            .options
            .get("server_state")
            .map(String::as_str),
        Some("current")
    );
    assert!(session.launch.preparation.lock().unwrap().is_some());
    assert!(sessions.pending.is_empty());
}
#[test]
fn expired_launches_restore_resources_after_unlocking_sessions() {
    use std::sync::{Mutex, Weak};
    struct Restore {
        sessions: Weak<Mutex<Sessions>>,
        restored: Arc<AtomicBool>,
    }
    impl Preparation<()> for Restore {
        fn prepared(&self) -> &() {
            &()
        }
        fn into_prepared(self: Box<Self>) {}
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            assert!(self.sessions.upgrade().unwrap().try_lock().is_ok());
            self.restored.store(true, Ordering::Release);
        }
    }
    let sessions = Arc::new(Mutex::new(Sessions::default()));
    let restored = Arc::new(AtomicBool::new(false));
    let mut expired = launch("expired", Role::Stream);
    expired.created = Instant::now() - Duration::from_secs(31);
    *expired.preparation.lock().unwrap() = Some(Box::new(Restore {
        sessions: Arc::downgrade(&sessions),
        restored: restored.clone(),
    }));
    let expired = {
        let mut locked = sessions.lock().unwrap();
        locked.pending.insert(expired.id.clone(), expired);
        let live = launch("live", Role::InputOnly);
        locked.queue(live.clone()).unwrap();
        assert_eq!(locked.pending_for_peer(live.peer).unwrap().id, "live");
        assert_eq!(locked.rtsp_for_peer(live.peer).len(), 1);
        let expired = locked.expire();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].id, "expired");
        assert!(locked.pending.contains_key("live"));
        assert!(!restored.load(Ordering::Acquire));
        expired
    };
    assert!(!restored.load(Ordering::Acquire));
    drop(expired);
    assert!(restored.load(Ordering::Acquire));
}
#[test]
fn canceled_launches_restore_resources_after_unlocking_sessions() {
    use std::sync::{Mutex, Weak};
    struct Restore {
        sessions: Weak<Mutex<Sessions>>,
        restored: Arc<AtomicBool>,
    }
    impl Preparation<()> for Restore {
        fn prepared(&self) -> &() {
            &()
        }
        fn into_prepared(self: Box<Self>) {}
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            assert!(self.sessions.upgrade().unwrap().try_lock().is_ok());
            self.restored.store(true, Ordering::Release);
        }
    }
    let cancellations: [fn(&mut Sessions) -> Vec<Launch>; 7] = [
        |s| s.supersede("client", Role::Stream),
        |s| s.request_stop(None),
        |s| s.request_stop(Some("pending")),
        |s| s.request_stop(Some("client")),
        |s| s.stop_role(Role::Stream, None),
        |s| s.stop_role(Role::Stream, Some("client")),
        |s| s.queue(launch("replacement", Role::Stream)).unwrap(),
    ];
    for cancel in cancellations {
        let sessions = Arc::new(Mutex::new(Sessions::default()));
        let restored = Arc::new(AtomicBool::new(false));
        let pending = launch("pending", Role::Stream);
        *pending.preparation.lock().unwrap() = Some(Box::new(Restore {
            sessions: Arc::downgrade(&sessions),
            restored: restored.clone(),
        }));
        let canceled = {
            let mut locked = sessions.lock().unwrap();
            locked.pending.insert(pending.id.clone(), pending);
            let canceled = cancel(&mut locked);
            assert_eq!(canceled.len(), 1);
            assert!(!locked.pending.contains_key("pending"));
            assert!(!restored.load(Ordering::Acquire));
            canceled
        };
        assert!(!restored.load(Ordering::Acquire));
        drop(canceled);
        assert!(restored.load(Ordering::Acquire));
    }
}
#[test]
fn pending_launches_and_streams_keep_sessions_busy_until_they_expire_or_end() {
    let mut sessions = Sessions::default();
    assert!(sessions.idle());
    let mut pending = launch("pending", Role::Stream);
    sessions.pending.insert(pending.id.clone(), pending.clone());
    assert!(!sessions.idle());
    pending.created = Instant::now() - Duration::from_secs(31);
    sessions.pending.insert(pending.id.clone(), pending.clone());
    assert!(sessions.idle());
    assert!(sessions.owns_capture());
    pending.preparing.store(true, Ordering::Release);
    assert!(!sessions.idle());
    pending.preparing.store(false, Ordering::Release);
    let fresh = launch("fresh", Role::Stream);
    sessions.pending.insert(fresh.id.clone(), fresh.clone());
    sessions.start(fresh, Negotiated::default()).unwrap();
    assert!(!sessions.idle());
    sessions.active.clear();
    sessions.teardown.insert("ended".into(), Instant::now());
    assert!(!sessions.idle());
    sessions.teardown.clear();
    assert!(sessions.idle());
}
fn launch(id: &str, role: Role) -> Launch {
    Launch {
        id: id.into(),
        client: Client {
            name: "fixture".into(),
            cert: String::new(),
            uuid: "client".into(),
            perm: u32::MAX,
            enabled: true,
            extra: Default::default(),
        },
        peer: "127.0.0.1".parse().unwrap(),
        app_id: 1,
        background: None,
        key: [0; 16],
        key_id: 1,
        ping: "ping".into(),
        connect_data: 1,
        role,
        created: Instant::now(),
        rtsp_encrypted: true,
        rtsp_counter: Arc::new(AtomicU32::new(1)),
        rtsp_received: Default::default(),
        preparation: Default::default(),
        vrr_requested: false,
        host_audio: false,
        requested_rate: 0,
        options: Default::default(),
        audio_preparation: Default::default(),
        preparing: Default::default(),
        transport: Default::default(),
        roaming_offered: Default::default(),
        claimed_until: None,
        nucleus: None,
        held: None,
        warnings: Default::default(),
    }
}
#[test]
fn what_a_launch_holds_goes_with_its_last_copy_and_never_with_an_rtsp_snapshot() {
    let held: Arc<dyn std::any::Any + Send + Sync> = Arc::new(());
    let given_back = Arc::downgrade(&held);
    let mut first = launch("held", Role::Stream);
    first.held = Some(held);
    let copy = first.clone();
    assert!(first.rtsp_snapshot().held.is_none());
    drop(first);
    assert!(given_back.upgrade().is_some());
    drop(copy);
    assert!(given_back.upgrade().is_none());
}
#[test]
fn a_ten_bit_format_is_sdr_only_when_the_query_says_hdr_mode_zero() {
    // Launch and resume carry the same query, so one table covers both.
    for (hdr_mode, sdr) in [(None, false), (Some("0"), true), (Some("1"), false)] {
        let mut launch = launch("color", Role::Stream);
        if let Some(mode) = hdr_mode {
            launch.options.insert("hdrMode".into(), mode.into());
        }
        assert_eq!(launch.ten_bit_sdr_requested(), sdr, "hdrMode={hdr_mode:?}");
    }
}
#[test]
fn a_failed_stream_is_not_reported_as_a_normal_close() {
    let closed = Session::new(launch("closed", Role::Stream), Negotiated::default());
    closed.stop();
    assert_eq!(closed.termination_reason(), 0x8003_0023);
    let failed = Session::new(launch("failed", Role::Stream), Negotiated::default());
    failed.fail();
    assert!(failed.stopping());
    assert_eq!(failed.termination_reason(), 0x8000_4005);
}
#[test]
fn a_worker_that_cannot_start_fails_without_leaving_an_active_session() {
    let mut sessions = Sessions::default();
    let failed = launch("failed", Role::Stream);
    sessions.queue(failed.clone()).unwrap();
    let failed = sessions.start(failed, Negotiated::default()).unwrap();
    let other = launch("other", Role::InputOnly);
    sessions.queue(other.clone()).unwrap();
    let other = sessions.start(other, Negotiated::default()).unwrap();
    sessions.fail_start(&failed);
    assert!(failed.failed());
    assert!(failed.stopping());
    assert_eq!(failed.termination_reason(), 0x8000_4005);
    assert!(!sessions.active.contains_key("failed"));
    assert!(sessions.active.contains_key("other"));
    assert!(!other.stopping());
    assert!(sessions.teardown.is_empty());
    let retry = launch("retry", Role::Stream);
    sessions.queue(retry.clone()).unwrap();
    sessions.start(retry, Negotiated::default()).unwrap();
}
#[test]
fn teardown_leaves_active_while_resources_remain_owned() {
    use std::sync::{Mutex, Weak};
    struct Resource {
        sessions: Weak<Mutex<Sessions>>,
        released: Arc<AtomicBool>,
    }
    impl Preparation<()> for Resource {
        fn prepared(&self) -> &() {
            &()
        }
        fn into_prepared(self: Box<Self>) {}
    }
    impl Drop for Resource {
        fn drop(&mut self) {
            let sessions = self.sessions.upgrade().unwrap();
            let sessions = sessions.try_lock().unwrap();
            assert!(!sessions.active.contains_key("old"));
            assert!(sessions.teardown.contains_key("old"));
            self.released.store(true, Ordering::Release);
        }
    }
    let sessions = Arc::new(Mutex::new(Sessions::default()));
    let released = Arc::new(AtomicBool::new(false));
    let old = launch("old", Role::Stream);
    *old.preparation.lock().unwrap() = Some(Box::new(Resource {
        sessions: Arc::downgrade(&sessions),
        released: released.clone(),
    }));
    let old = {
        let mut sessions = sessions.lock().unwrap();
        sessions.queue(old.clone()).unwrap();
        sessions.start(old, Negotiated::default()).unwrap()
    };
    old.stop();
    {
        let mut sessions = sessions.lock().unwrap();
        sessions.begin_teardown(&old);
        assert!(sessions.active.is_empty());
        assert!(sessions.owns_capture());
        assert!(!sessions.idle());
        let mut next = launch("next", Role::Stream);
        next.app_id = 2;
        sessions.queue(next.clone()).unwrap();
        sessions.start(next, Negotiated::default()).unwrap();
        assert!(!released.load(Ordering::Acquire));
    }
    drop(old);
    assert!(released.load(Ordering::Acquire));
    let mut sessions = sessions.lock().unwrap();
    sessions.teardown.remove("old");
    sessions.active.clear();
    assert!(sessions.idle());
    assert!(!sessions.owns_capture());
}
#[test]
fn only_pyrowave_sessions_report_quality_bitrates() {
    for codec in 0..=3 {
        let config = Negotiated {
            codec,
            rate_millihz: 59_940,
            ..Default::default()
        };
        let session = Session::new(launch("quality", Role::Stream), config.clone());
        let info = session.info();
        if codec == 3 {
            assert_eq!(
                info["pyrowave_minimum_kbps"],
                crate::pyrowave::minimum_kbps(config.width, config.height, 59_940)
            );
            assert_eq!(
                info["pyrowave_recommended_kbps"],
                crate::pyrowave::recommended_kbps(config.width, config.height, 59_940)
            );
        } else {
            assert!(info["pyrowave_minimum_kbps"].is_null());
            assert!(info["pyrowave_recommended_kbps"].is_null());
        }
        session.bitrate.store(100_000, Ordering::Relaxed);
        assert_eq!(session.info()["encoder_bitrate_kbps"], 100_000);
    }
}
#[test]
fn host_only_loss_on_a_remembered_route_names_the_host_s_adapter() {
    const FAST: u64 = 800_000_000;
    const SAFE: u64 = 62_000_000;
    let s = Session::new(launch("onn", Role::Stream), Negotiated::default());
    s.configure_remembered_keyframe_loss(true, true);
    for frame in 1..=20 {
        assert_eq!(s.keyframe_loss_bps(FAST, SAFE, frame, frame == 20), SAFE);
    }
    // The socket refuses keyframe 20: held for the stream, never saved,
    // and the warning points at the host, not the network path.
    s.keyframe_abandoned(20);
    assert!(s.keyframe_loss_active());
    assert!(s.keyframe_loss_sustained_at().is_none());
    let message = s.info()["warnings"][0]["message"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(message.contains("host's network adapter"), "{message}");
    assert!(message.contains("rest of the stream"), "{message}");
    assert!(!message.contains("network path"), "{message}");
}
#[test]
fn a_lost_keyframe_paces_the_rest_of_the_stream_like_wifi() {
    const FAST: u64 = 800_000_000;
    const SAFE: u64 = 62_000_000;
    let s = Session::new(launch("onn", Role::Stream), Negotiated::default());
    s.configure_keyframe_loss(true);
    assert_eq!(s.keyframe_loss_bps(FAST, SAFE, 1, true), FAST);
    for frame in 2..=8 {
        assert_eq!(s.keyframe_loss_bps(FAST, SAFE, frame, false), FAST);
    }
    s.stats.video_frame.store(9, Ordering::Release);
    // A lost ordinary frame is left to recovery frames.
    s.record_fec_status(&fec_report(5, 7, 1));
    assert_eq!(s.keyframe_loss_bps(FAST, SAFE, 9, false), FAST);
    assert!(s.info()["warnings"].as_array().unwrap().is_empty());
    // The keyframe arrived short of what FEC repairs.
    s.record_fec_status(&fec_report(1, 7, 1));
    assert_eq!(s.keyframe_loss_bps(FAST, SAFE, 10, false), SAFE);
    assert_eq!(s.keyframe_loss_bps(FAST, SAFE, 11, true), SAFE);
    assert_eq!(s.info()["warnings"][0]["code"], "network_keyframe_loss");
    // The stream's first keyframe alone: held for now, not remembered.
    assert!(s.keyframe_loss_ever_latched());
    assert!(s.keyframe_loss_observed_at().is_some());
    assert!(s.keyframe_loss_sustained_at().is_none());

    // Without reports: the client asks again for the keyframe it was sent.
    let s = Session::new(launch("old", Role::Stream), Negotiated::default());
    s.configure_keyframe_loss(true);
    s.keyframe_loss_bps(FAST, SAFE, 1, true);
    s.keyframe_loss.lock().unwrap().settle();
    s.request_recovery_keyframe();
    assert_eq!(s.keyframe_loss_bps(FAST, SAFE, 2, true), FAST);
    s.request_recovery_keyframe();
    assert_eq!(s.keyframe_loss_bps(FAST, SAFE, 3, true), SAFE);
    assert_eq!(s.stats.idr_requests.load(Ordering::Relaxed), 2);

    // Explicitly disabled policies preserve the base rate for every codec.
    for (enabled, codec) in [(false, 1), (false, 3)] {
        let s = Session::new(
            launch("off", Role::Stream),
            Negotiated {
                codec,
                ..Default::default()
            },
        );
        s.configure_keyframe_loss(enabled);
        s.keyframe_loss_bps(FAST, SAFE, 1, true);
        s.stats.video_frame.store(2, Ordering::Release);
        s.record_fec_status(&fec_report(1, 0, 0));
        s.request_recovery_keyframe();
        s.keyframe_loss_bps(FAST, SAFE, 2, true);
        s.request_recovery_keyframe();
        assert_eq!(s.keyframe_loss_bps(FAST, SAFE, 3, false), FAST);
    }
}
#[test]
fn repeated_distinct_unrecoverable_frames_reduce_pacing_without_changing_bitrate() {
    const FAST: u64 = 800_000_000;
    const SAFE: u64 = 62_000_000;
    let s = Session::new(launch("loss", Role::Stream), Negotiated::default());
    s.configure_keyframe_loss(true);
    s.stats.video_frame.store(7, Ordering::Release);
    let bitrate = s.bitrate.load(Ordering::Relaxed);
    s.record_fec_status(&fec_report(1, 10, 0)); // Clean.
    s.record_fec_status(&fec_report(2, 9, 1)); // Recovered.
    // Interleaved failed blocks from two frames are still only two losses.
    for (frame, block) in [(4, 0), (5, 0), (4, 1), (5, 1), (4, 2), (5, 2)] {
        let mut report = fec_report(frame, 7, 1);
        report[19] = block;
        report[20] = 4;
        s.record_fec_status(&report);
        s.record_fec_status(&report); // Duplicate block.
        assert!(!s.keyframe_loss_active());
        assert_eq!(s.keyframe_loss_rate_bps(FAST, SAFE), FAST);
    }
    s.record_fec_status(&fec_report(6, 7, 1));
    assert!(s.keyframe_loss_active());
    assert_eq!(s.keyframe_loss_rate_bps(FAST, SAFE), SAFE);
    assert_eq!(s.bitrate.load(Ordering::Relaxed), bitrate);
    assert_eq!(s.stats.idr_requests.load(Ordering::Relaxed), 0);
    assert!(
        s.info()["warnings"][0]["message"]
            .as_str()
            .unwrap()
            .contains("three recent")
    );
}
#[test]
fn repeated_loss_guard_preserves_disabled_manual_and_adaptive_owners() {
    for text in [
        "keyframe_loss_pacing=false",
        "pacing_max_bitrate_kbps=80000",
        "host_adaptive_pacing=true",
        "host_adaptive_bitrate=true",
    ] {
        let config = crate::config::Config::parse(text).unwrap();
        let s = Session::new(launch("owner", Role::Stream), Negotiated::default());
        s.configure_keyframe_loss(config.keyframe_loss_pacing(0, false));
        s.stats.video_frame.store(4, Ordering::Release);
        for frame in 1..=3 {
            s.record_fec_status(&fec_report(frame, 7, 1));
        }
        assert!(!s.keyframe_loss_active(), "{text}");
        assert!(
            s.info()["warnings"].as_array().unwrap().is_empty(),
            "{text}"
        );
    }
}
#[test]
fn a_fresh_probe_or_disabled_policy_clears_the_previous_launch_warning() {
    let s = Session::new(launch("remembered", Role::Stream), Negotiated::default());
    for enabled in [true, false] {
        s.configure_remembered_keyframe_loss(true, true);
        assert_eq!(
            s.keyframe_loss_reason(),
            Some("remembered from the previous session")
        );
        assert!(!s.info()["warnings"].as_array().unwrap().is_empty());
        s.configure_remembered_keyframe_loss(enabled, false);
        assert!(!s.keyframe_loss_active());
        assert!(s.info()["warnings"].as_array().unwrap().is_empty());
    }
    s.configure_remembered_keyframe_loss(false, true);
    assert!(!s.keyframe_loss_active());
}
#[test]
fn sender_pacing_queries_do_not_add_sent_frames_to_keyframe_history() {
    const FAST: u64 = 800_000_000;
    const SAFE: u64 = 62_000_000;
    let s = Session::new(launch("query", Role::Stream), Negotiated::default());
    s.configure_keyframe_loss(true);
    s.keyframe_loss_bps(FAST, SAFE, 1, true);
    s.keyframe_loss.lock().unwrap().settle();
    s.request_recovery_keyframe();
    s.keyframe_loss_bps(FAST, SAFE, 2, true);
    for _ in 0..crate::keyframe_loss::DECODE_QUEUE + 1 {
        assert!(!s.keyframe_loss_active());
        assert_eq!(s.keyframe_loss_rate_bps(FAST, SAFE), FAST);
    }
    // No ordinary frame followed that keyframe: repeated queries cannot
    // turn a lost-keyframe request into an apparent decode queue overflow.
    s.request_recovery_keyframe();
    assert!(s.keyframe_loss_active());
    assert_eq!(s.keyframe_loss_rate_bps(FAST, SAFE), SAFE);
}
/// A stream sending frames 1..=40 on a good link; `FAST`/`SAFE` as above.
fn steady_stream(name: &str) -> std::sync::Arc<Session> {
    let s = Session::new(launch(name, Role::Stream), Negotiated::default());
    s.configure_keyframe_loss(true);
    s.keyframe_loss_bps(800_000_000, 62_000_000, 1, true);
    for frame in 2..=40 {
        s.keyframe_loss_bps(800_000_000, 62_000_000, frame, false);
    }
    s.stats.video_frame.store(41, Ordering::Release);
    s
}
#[test]
fn the_hosts_own_keyframe_and_the_clients_reaction_to_it_do_not_slow_the_stream() {
    // Alt+Tab, a bitrate change or an encoder restart: the host sends a
    // keyframe nobody asked for, the client resets and asks for one,
    // twice. No packet was lost (the issue behind Wi-Fi pacing for 24 h).
    const FAST: u64 = 800_000_000;
    const SAFE: u64 = 62_000_000;
    let s = steady_stream("alt-tab");
    s.keyframe_loss_bps(FAST, SAFE, 41, true);
    s.request_recovery_keyframe();
    s.keyframe_loss_bps(FAST, SAFE, 42, true);
    s.request_recovery_keyframe();
    for frame in 43..=45 {
        s.keyframe_loss_bps(FAST, SAFE, frame, false);
    }
    s.stats.video_frame.store(46, Ordering::Release);
    for frame in 43..=45 {
        s.record_fec_status(&fec_report(frame, 7, 1));
    }
    assert!(!s.keyframe_loss_active());
    assert!(s.keyframe_loss_observed_at().is_none());
    assert_eq!(s.keyframe_loss_rate_bps(FAST, SAFE), FAST);
    assert!(s.info()["warnings"].as_array().unwrap().is_empty());
    // The keyframe itself arriving incomplete is still network loss.
    s.record_fec_status(&fec_report(41, 7, 1));
    assert!(s.keyframe_loss_active());
    assert!(s.keyframe_loss_observed_at().is_some());
}
#[test]
fn the_start_of_a_stream_does_not_slow_it_but_a_lost_first_keyframe_does() {
    // Logged at the start of real streams: the client asks for a
    // keyframe, gets frame 1, and asks again ~100 ms later before it has
    // shown anything. Nothing was lost.
    const FAST: u64 = 800_000_000;
    const SAFE: u64 = 62_000_000;
    let start = |name: &str| {
        let s = Session::new(launch(name, Role::Stream), Negotiated::default());
        s.configure_keyframe_loss(true);
        s.request_recovery_keyframe();
        s.keyframe_loss_bps(FAST, SAFE, 1, true);
        s.request_recovery_keyframe();
        s.keyframe_loss_bps(FAST, SAFE, 2, true);
        s.request_recovery_keyframe();
        for frame in 3..=10 {
            s.keyframe_loss_bps(FAST, SAFE, frame, false);
        }
        s.stats.video_frame.store(11, Ordering::Release);
        s
    };
    let s = start("startup");
    assert!(!s.keyframe_loss_active());
    assert!(s.keyframe_loss_observed_at().is_none());
    assert_eq!(s.keyframe_loss_rate_bps(FAST, SAFE), FAST);
    assert!(s.info()["warnings"].as_array().unwrap().is_empty());
    // Frames reported after the second keyframe, inside the window, are
    // the same start; the first keyframe reported incomplete is a loss.
    s.record_fec_status(&fec_report(5, 7, 1));
    assert!(!s.keyframe_loss_active());
    s.record_fec_status(&fec_report(2, 7, 1));
    assert!(s.keyframe_loss_active());
}
#[test]
fn frames_the_host_could_not_send_slow_the_stream_and_the_clients_report_adds_nothing() {
    // Send-buffer pressure at 800 Mbps (error 10055) is loss: three frames
    // within two seconds slow the stream, named as lost on send.
    const FAST: u64 = 800_000_000;
    const SAFE: u64 = 62_000_000;
    let s = steady_stream("lost-on-send");
    for frame in 41..=43 {
        s.keyframe_loss_bps(FAST, SAFE, frame, false);
        s.keyframe_abandoned(frame);
    }
    assert!(s.keyframe_loss_active());
    assert_eq!(
        s.keyframe_loss_reason(),
        Some("three recent video frames were lost on send within two seconds")
    );
    assert_eq!(s.keyframe_loss_bps(FAST, SAFE, 44, false), SAFE);
    // Only the host saw it: for now, and nothing for the next session.
    assert!(s.keyframe_loss_sustained_at().is_none());
    assert_eq!(s.info()["frames_lost_on_send"], 3);
    assert!(
        s.info()["warnings"][0]["message"]
            .as_str()
            .unwrap()
            .contains("The host's network adapter refused video packets")
    );
    // Two such frames are not enough, and the client reporting them is the
    // same loss, not more of it.
    let s = steady_stream("lost-on-send-twice");
    for frame in 41..=42 {
        s.keyframe_loss_bps(FAST, SAFE, frame, false);
        s.keyframe_abandoned(frame);
    }
    s.stats.video_frame.store(44, Ordering::Release);
    for frame in 41..=42 {
        s.record_fec_status(&fec_report(frame, 7, 1));
    }
    assert!(!s.keyframe_loss_active());
}

#[test]
fn a_keyframe_the_host_could_not_send_whole_slows_the_stream() {
    const FAST: u64 = 800_000_000;
    const SAFE: u64 = 62_000_000;
    let s = steady_stream("keyframe-lost-on-send");
    s.keyframe_loss_bps(FAST, SAFE, 41, true);
    s.keyframe_abandoned(41);
    assert_eq!(
        s.keyframe_loss_reason(),
        Some("a keyframe was lost on send")
    );
}

#[test]
fn the_keyframe_after_a_loss_on_send_does_not_hide_the_losses_around_it() {
    // Not a change the host made on its own, so no window follows it and
    // the client's reports of later frames still count.
    const FAST: u64 = 800_000_000;
    const SAFE: u64 = 62_000_000;
    let s = steady_stream("send-loss-recovery");
    s.keyframe_loss.lock().unwrap().settle();
    s.request_send_loss_recovery();
    s.keyframe_loss_bps(FAST, SAFE, 41, true);
    s.stats.video_frame.store(60, Ordering::Release);
    for frame in [45, 50, 55] {
        s.record_fec_status(&fec_report(frame, 7, 1));
    }
    assert!(s.keyframe_loss_active());
}

#[test]
fn a_request_just_before_the_hosts_keyframe_is_not_the_one_asked_for_again() {
    const FAST: u64 = 800_000_000;
    const SAFE: u64 = 62_000_000;
    let s = steady_stream("request-then-host-keyframe");
    s.request_recovery_keyframe();
    s.request_host_keyframe();
    s.keyframe_loss_bps(FAST, SAFE, 41, true);
    s.keyframe_loss_bps(FAST, SAFE, 42, false);
    s.request_recovery_keyframe();
    assert!(!s.keyframe_loss_active());
}

#[test]
fn the_console_shows_the_encoded_rate_beside_the_requested_one() {
    // Artemis's Warp launches at 60 fps and asks for 120 in the stream
    // request; with the launch frame rate on, 60 is encoded. The console
    // showed 120 as the stream's rate and target.
    let request = Negotiated {
        fps: 120,
        rate_millihz: 120_000,
        ..Default::default()
    };
    let mut config = request.clone();
    let limited = crate::config::Config::parse("limit_framerate=true\n").unwrap();
    crate::stream_policy::apply(&mut config, 60_000, &limited);
    let session = Session::new(launch("warp", Role::Stream), config.clone());
    let info = session.info();
    assert_eq!(
        (info["fps"].clone(), info["requested_fps"].clone()),
        (60.into(), 120.into())
    );
    // By default the stream runs at the request's rate.
    let mut following = request.clone();
    crate::stream_policy::apply(&mut following, 60_000, &crate::config::Config::default());
    let info = Session::new(launch("warp", Role::Stream), following).info();
    assert_eq!(
        (info["fps"].clone(), info["requested_fps"].clone()),
        (120.into(), 120.into())
    );
    // A client's own mode switch is what it asked for.
    let switch = crate::reconfigure::Mode {
        width: config.width,
        height: config.height,
        fps_millihz: 90_000,
    };
    let mut reconfigure = session.reconfigure.lock().unwrap();
    let now = Instant::now();
    reconfigure.on_request(now, switch);
    assert_eq!(
        reconfigure.take_due(now + Duration::from_secs(1)),
        Some(switch)
    );
    drop(reconfigure);
    let info = session.info();
    assert_eq!(
        (info["fps"].clone(), info["requested_fps"].clone()),
        (90.into(), 90.into())
    );
    let unlimited = Negotiated {
        fps: 120,
        rate_millihz: 119_880,
        ..Default::default()
    };
    let info = Session::new(launch("plain", Role::Stream), unlimited).info();
    assert_eq!(
        (info["fps"].clone(), info["requested_fps"].clone()),
        (120.into(), 120.into())
    );
}
#[test]
fn a_sent_picture_feeds_the_client_report_and_the_record_sink_alike() {
    use crate::{host_timing::HostTimingRequest, performance::Stamps, timing_record};
    use std::time::Duration;
    let session = Session::new(launch("record", Role::Stream), Negotiated::default());
    let epoch = Instant::now();
    let us = |n: u64| epoch + Duration::from_micros(n);
    let stamps = Stamps {
        presented: us(100),
        acquired: us(200),
        published: us(250),
        claimed: us(300),
        submitted: us(500),
        encoded: us(2_500),
        packetized: us(2_600),
        prepared: us(2_700),
        first_sent: us(2_700),
        last_sent: us(2_900),
        streamed: None,
    };
    // Neither a client report nor a sink: nothing is kept.
    session.record_sent(1, 9_000, &stamps, epoch);
    assert_eq!(session.host_timings.lock().unwrap().take_report(), None);

    let (sink, rows) = timing_record::channel(8);
    *session.timing_record.write().unwrap() = Some(Arc::new(sink));
    session
        .host_timings
        .lock()
        .unwrap()
        .on_request(&HostTimingRequest { enabled: true }.encode());
    session.record_sent(2, 9_000, &stamps, epoch);
    let report = session.host_timings.lock().unwrap().take_report().unwrap();
    let (record, extra) = crate::host_timing::HostTiming::decode_report_with(&report)
        .unwrap()
        .remove(0);
    assert_eq!((record.frame, record.host_us), (2, 2_800));
    assert_eq!(extra.unwrap().presented_at_us, 100);
    let row = rows.try_recv().unwrap();
    assert_eq!(row.wire_frame, 2);
    assert_eq!(row.rtp_us, 100_000);
    assert_eq!(row.points[0], 100);
    assert_eq!(row.points[9], 2_900);
    assert!(rows.try_recv().is_err(), "the first picture had no sink");
}
#[test]
fn pyrowave_feedback_cannot_force_frames_or_turn_invalidation_into_idr() {
    let session = Session::new(
        launch("pyrowave", Role::Stream),
        Negotiated {
            codec: 3,
            ..Default::default()
        },
    );
    session.idr.store(false, Ordering::Release);
    *session.recovery_wake.lock().unwrap() = Some(Box::new(|| {
        panic!("intra-only feedback must not wake the session");
    }));
    for (first, last) in [(1, 2), (0, 0), (4, 3)] {
        session.request_invalidation(first, last);
    }
    assert_eq!(
        session
            .stats
            .reference_invalidations
            .load(Ordering::Relaxed),
        3
    );
    assert_eq!(session.stats.idr_requests.load(Ordering::Relaxed), 0);
    assert!(session.invalidation.lock().unwrap().is_none());
    session.request_idr();
    assert_eq!(session.stats.idr_requests.load(Ordering::Relaxed), 1);
    assert!(!session.idr.load(Ordering::Acquire));
}
#[test]
fn slice_delivery_overflows_are_counted_in_the_session_info() {
    let session = Session::new(launch("overflow", Role::Stream), Negotiated::default());
    assert_eq!(session.info()["slice_delivery_overflows"], 0);
    session
        .stats
        .slice_delivery_overflows
        .fetch_add(1, Ordering::Relaxed);
    assert_eq!(session.info()["slice_delivery_overflows"], 1);
}
#[test]
fn send_loss_recovery_forces_a_keyframe_without_counting_a_client_request() {
    for codec in 0..=3 {
        let session = Session::new(
            launch("send-loss", Role::Stream),
            Negotiated {
                codec,
                ..Default::default()
            },
        );
        session.idr.store(false, Ordering::Release);
        session.request_send_loss_recovery();
        // PyroWave frames are all intra; nothing to force.
        assert_eq!(session.idr.load(Ordering::Acquire), codec != 3);
        assert_eq!(session.stats.idr_requests.load(Ordering::Relaxed), 0);
        assert_eq!(session.info()["send_loss_recoveries"], 1);
    }
}
#[test]
fn inter_frame_recovery_keeps_invalidation_ranges_and_idr_fallback() {
    for codec in 0..=2 {
        let session = Session::new(
            launch("inter-frame", Role::Stream),
            Negotiated {
                codec,
                ..Default::default()
            },
        );
        session.idr.store(false, Ordering::Release);
        session.request_invalidation(4, 6);
        session.request_invalidation(2, 3);
        assert_eq!(*session.invalidation.lock().unwrap(), Some((2, 6)));
        assert!(!session.idr.load(Ordering::Acquire));
        session.request_invalidation(0, 0);
        assert!(session.idr.load(Ordering::Acquire));
        assert_eq!(session.stats.idr_requests.load(Ordering::Relaxed), 1);
        assert_eq!(
            session
                .stats
                .reference_invalidations
                .load(Ordering::Relaxed),
            3
        );
    }
}
#[test]
fn independent_roles_and_targeted_teardown_preserve_other_sessions() {
    let mut sessions = Sessions::default();
    sessions.queue(launch("game", Role::Stream)).unwrap();
    sessions
        .queue(launch("monitor", Role::RemoteMonitor))
        .unwrap();
    sessions.queue(launch("input", Role::InputOnly)).unwrap();
    // A client launching again replaces its own earlier launch in that role.
    sessions
        .queue(launch("duplicate", Role::RemoteMonitor))
        .unwrap();
    assert!(!sessions.pending.contains_key("monitor"));
    assert!(sessions.pending.contains_key("duplicate"));
    let game = sessions
        .start(launch("game", Role::Stream), Negotiated::default())
        .unwrap();
    let input = sessions
        .start(launch("input", Role::InputOnly), Negotiated::default())
        .unwrap();
    drop(sessions.stop_role(Role::RemoteMonitor, Some("client")));
    assert!(sessions.pending.is_empty());
    assert!(!game.stopping());
    assert!(!input.stopping());
    drop(sessions.stop_role(Role::Stream, None));
    assert!(game.stopping());
    assert!(!input.stopping());
    drop(sessions.request_stop(Some("client")));
    assert!(input.stopping());
}
#[test]
fn a_new_launch_replaces_an_unconnected_launch_and_stops_an_abandoned_stream() {
    let mut sessions = Sessions::default();
    // A launch whose RTSP never arrived must not block the next attempt.
    sessions.queue(launch("first", Role::Stream)).unwrap();
    sessions.queue(launch("retry", Role::Stream)).unwrap();
    assert_eq!(sessions.pending.keys().collect::<Vec<_>>(), ["retry"]);
    // A stream the client abandoned is stopped when it launches again.
    let old = sessions
        .start(launch("retry", Role::Stream), Negotiated::default())
        .unwrap();
    sessions.queue(launch("resume", Role::Stream)).unwrap();
    assert!(old.stopping());
    assert!(sessions.pending.contains_key("resume"));
    // Another client's stream is untouched.
    let mut other = launch("other", Role::Stream);
    other.client.uuid = "other-client".into();
    sessions.queue(other.clone()).unwrap();
    let other = sessions.start(other, Negotiated::default()).unwrap();
    sessions.queue(launch("again", Role::Stream)).unwrap();
    assert!(!other.stopping());
}
fn webrtc_launch(id: &str, client: &str) -> Launch {
    let mut launch = launch(id, Role::Stream);
    launch.transport = Transport::WebRtc;
    launch.client.uuid = client.into();
    launch
}
#[test]
fn a_session_of_either_transport_ends_the_other_one() {
    // A WebRTC session replaces a Moonlight stream and a pending Moonlight launch.
    let mut sessions = Sessions::default();
    sessions.queue(launch("moonlight", Role::Stream)).unwrap();
    let moonlight = sessions
        .start(launch("moonlight", Role::Stream), Negotiated::default())
        .unwrap();
    sessions.queue(launch("pending", Role::Stream)).unwrap();
    let canceled = sessions.supersede_other_transport(Role::Stream, Transport::WebRtc);
    assert_eq!(canceled.len(), 1);
    assert!(moonlight.stopping());
    assert!(sessions.pending.is_empty());
    // And a Moonlight launch replaces a WebRTC stream, from any device.
    sessions.queue(webrtc_launch("web", "tv")).unwrap();
    let web = sessions
        .start(webrtc_launch("web", "tv"), Negotiated::default())
        .unwrap();
    let canceled = sessions.supersede_other_transport(Role::Stream, Transport::GameStream);
    assert!(canceled.is_empty());
    assert!(web.stopping());
    // Sessions of the transport asking are left alone.
    sessions.queue(webrtc_launch("web2", "tv")).unwrap();
    let web2 = sessions
        .start(webrtc_launch("web2", "tv"), Negotiated::default())
        .unwrap();
    let canceled = sessions.supersede_other_transport(Role::Stream, Transport::WebRtc);
    assert!(canceled.is_empty());
    assert!(!web2.stopping());
    // Another role is untouched too.
    sessions
        .queue(launch("monitor", Role::RemoteMonitor))
        .unwrap();
    let canceled = sessions.supersede_other_transport(Role::Stream, Transport::GameStream);
    assert!(canceled.is_empty());
    assert!(sessions.pending.contains_key("monitor"));
}
#[test]
fn a_nucleus_co_op_stream_is_never_ended_by_the_other_transport() {
    let seat = Seat {
        session: 1,
        slot: 1,
    };
    let mut sessions = Sessions::default();
    let mut player = webrtc_launch("player", "tv");
    player.nucleus = Some(seat);
    sessions.queue(player.clone()).unwrap();
    let player = sessions.start(player, Negotiated::default()).unwrap();
    let mut pending = launch("pending", Role::Stream);
    pending.nucleus = Some(seat);
    sessions.queue(pending).unwrap();
    let canceled = sessions.supersede_other_transport(Role::Stream, Transport::GameStream);
    assert!(canceled.is_empty());
    assert!(!player.stopping());
    assert!(sessions.pending.contains_key("pending"));
}
#[test]
fn a_webrtc_stream_is_held_by_another_device_only_while_it_lives() {
    let mut sessions = Sessions::default();
    assert!(!sessions.held_by_other(Role::Stream, Transport::WebRtc, "tv"));
    sessions.queue(webrtc_launch("web", "tv")).unwrap();
    assert!(!sessions.held_by_other(Role::Stream, Transport::WebRtc, "tv"));
    assert!(sessions.held_by_other(Role::Stream, Transport::WebRtc, "phone"));
    // A Moonlight launch is not a WebRTC one.
    assert!(!sessions.held_by_other(Role::Stream, Transport::GameStream, "phone"));
    let web = sessions
        .start(webrtc_launch("web", "tv"), Negotiated::default())
        .unwrap();
    assert!(sessions.held_by_other(Role::Stream, Transport::WebRtc, "phone"));
    web.stop();
    assert!(!sessions.held_by_other(Role::Stream, Transport::WebRtc, "phone"));
}
