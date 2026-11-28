//! Tests for automatic packet pacing, wireless startup seeds, and client bitrate handoff.
use super::*;

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
    let config = butterpollo_config::config::Config::default();
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
        assert_eq!(rate, 4 * 63_000_000);
        // When the client reports a 1 Gbps link speed, known-link behavior stays unchanged.
        let known = Session::new(
            launch("wireless-known-link", Role::Stream),
            Negotiated {
                codec: 1,
                bitrate_kbps: 63_000,
                client_wireless: true,
                pyrowave_client_link_mbps: 1000,
                ..Default::default()
            },
        );
        known.configure_automatic_pacing(config.adaptive_packet_pacing(1, true));
        known.video_pacing_bps(800_000_000, 75_000_000);
        known.tick_automatic_pacing(start, Duration::from_millis(2));
        for step in 1..=40u64 {
            let now = start + Duration::from_millis(500 * step);
            pacing_traffic(&known, 30 * step);
            known.record_control_ping(now, &[4, 0, 0, 0, 0, 0, 0, 0]);
            known.tick_automatic_pacing(now, Duration::from_millis(2));
        }
        assert_eq!(known.video_pacing_bps(800_000_000, 75_000_000), 800_000_000);
    }
    // Without the controller the hint sets the route pacing instead.
    for (setting, codec) in [
        ("wireless_adaptive_pacing=false", 1),
        ("", 3),
        ("pacing_max_bitrate_kbps=80000", 1),
    ] {
        let config = butterpollo_config::config::Config::parse(setting).unwrap();
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
    let mut packetizer = butterpollo_media_policy::packet::VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: s.config.packet_size,
        fec_percent: s.fec_percent(),
        min_fec: s.config.min_fec,
        key: Some([0; 16]),
    };
    let needed = butterpollo_media_policy::network_pacing::video_wire_rate_bps(
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
    let needed = butterpollo_media_policy::network_pacing::video_wire_rate_bps(
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
fn wired_sustained_loss_hands_over_to_adaptive_pacing_and_probes_up() {
    let s = Session::new(
        launch("wired-loss-handoff", Role::Stream),
        Negotiated {
            codec: 1,
            bitrate_kbps: 40_000,
            client_wireless: false,
            moonlight_features: butterpollo_media_policy::rtsp::FEC_STATUS_FEATURE,
            ..Default::default()
        },
    );
    s.configure_automatic_pacing(false);
    s.configure_adaptive_pacing_on_loss(true);
    s.configure_keyframe_loss(true);

    // Clean wired stream stays at 800 Mbps without adaptive pacing.
    let t0 = Instant::now() - Duration::from_secs(1);
    assert_eq!(s.keyframe_loss_bps_at(FAST, 80_000_000, 1, true, t0), FAST);
    for frame in 2..=50 {
        assert_eq!(
            s.keyframe_loss_bps_at(FAST, 80_000_000, frame, false, t0 + FRAME * frame),
            FAST
        );
    }
    pacing_traffic(&s, 50);
    assert!(!s.automatic_pacing_enabled());

    // Three distinct unrecoverable ordinary frames trigger Sustained handoff.
    for frame in 48..=50 {
        s.record_fec_status(&fec_report(frame, 7, 1));
    }
    assert!(s.keyframe_loss_active());
    assert!(s.automatic_pacing_enabled());
    let warning = s.info()["warnings"][0]["message"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(warning.contains("adaptive packet pacing"), "{warning}");
    assert!(warning.contains("twice the bitrate"), "{warning}");
    assert!(
        warning.contains("without reducing the encoder bitrate"),
        "{warning}"
    );
    assert_eq!(s.video_pacing_bps(FAST, 50_000_000), 80_000_000);
    assert_eq!(s.keyframe_loss_rate_bps(80_000_000, 80_000_000), 80_000_000);

    // Delayed pre-handoff reports (<= 50) are fenced and cannot trigger a cut.
    s.record_fec_status(&fec_report(47, 7, 1));
    let start = Instant::now();
    s.tick_automatic_pacing(start, Duration::from_millis(2));
    for step in 1..=20u64 {
        let now = start + Duration::from_millis(500 * step);
        pacing_traffic(&s, 50 + 30 * step);
        s.record_control_ping(now, &[4, 0, 0, 0, 0, 0, 0, 0]);
        s.tick_automatic_pacing(now, Duration::from_millis(2));
    }
    let climbed = s.video_pacing_bps(FAST, 50_000_000);
    assert!(climbed > 80_000_000, "never climbed: {climbed}");
    assert_eq!(s.keyframe_loss_rate_bps(climbed, 80_000_000), climbed);
    assert_eq!(
        s.keyframe_loss_bps(climbed, 80_000_000, 700, false),
        climbed
    );
    assert_eq!(s.bitrate.load(Ordering::Acquire), 40_000);
}

#[test]
fn provisional_startup_and_send_latches_stay_on_keyframe_loss_until_confirmed() {
    let wired_fec = || Negotiated {
        codec: 1,
        bitrate_kbps: 40_000,
        client_wireless: false,
        moonlight_features: butterpollo_media_policy::rtsp::FEC_STATUS_FEATURE,
        ..Default::default()
    };

    // Startup provisional latch stays off adaptive pacing and lifts after 10 s clean.
    let s = Session::new(launch("startup-lifts", Role::Stream), wired_fec());
    s.configure_automatic_pacing(false);
    s.configure_adaptive_pacing_on_loss(true);
    s.configure_keyframe_loss(true);
    let t0 = Instant::now() - Duration::from_millis(100);
    assert_eq!(s.keyframe_loss_bps_at(FAST, 80_000_000, 1, true, t0), FAST);
    s.stats.video_frame.store(2, Ordering::Release);
    s.record_fec_status(&fec_report(1, 7, 1));
    assert!(s.keyframe_loss_active());
    assert!(!s.automatic_pacing_enabled());
    assert_eq!(
        s.keyframe_loss_bps_at(FAST, 80_000_000, 2, false, t0 + Duration::from_millis(120)),
        80_000_000
    );
    let mut frame = 3;
    while s.keyframe_loss_active() {
        s.keyframe_loss_bps_at(FAST, 80_000_000, frame, false, t0 + FRAME * frame);
        frame += 1;
        assert!(frame < 2_000);
    }
    assert!(!s.automatic_pacing_enabled());
    assert_eq!(s.keyframe_loss_rate_bps(FAST, 80_000_000), FAST);

    // Startup latch confirmed by subsequent client loss hands over to adaptive pacing.
    let s = Session::new(launch("startup-confirmed", Role::Stream), wired_fec());
    s.configure_automatic_pacing(false);
    s.configure_adaptive_pacing_on_loss(true);
    s.configure_keyframe_loss(true);
    let t_start = Instant::now() - Duration::from_secs(1);
    s.keyframe_loss.lock().unwrap().sending_at(1, true, t_start);
    s.stats.video_frame.store(2, Ordering::Release);
    s.record_fec_status(&fec_report(1, 7, 1));
    assert!(!s.automatic_pacing_enabled());
    for frame in 2..=6 {
        s.keyframe_loss
            .lock()
            .unwrap()
            .sending_at(frame, false, t_start + FRAME * frame);
    }
    s.stats.video_frame.store(7, Ordering::Release);
    for frame in 4..=6 {
        s.record_fec_status(&fec_report(frame, 7, 1));
    }
    assert!(s.automatic_pacing_enabled());
    assert!(
        s.info()["warnings"][0]["message"]
            .as_str()
            .unwrap()
            .contains("adaptive packet pacing")
    );

    // Send provisional latch stays off adaptive pacing until client-reported loss confirms it.
    let s = Session::new(launch("send-latch", Role::Stream), wired_fec());
    s.configure_automatic_pacing(false);
    s.configure_adaptive_pacing_on_loss(true);
    s.configure_keyframe_loss(true);
    let t_send = Instant::now() - Duration::from_secs(1);
    for frame in 1..=40 {
        s.keyframe_loss_bps_at(FAST, 80_000_000, frame, frame == 1, t_send + FRAME * frame);
    }
    for frame in 41..=43 {
        s.keyframe_loss_bps_at(FAST, 80_000_000, frame, false, t_send + FRAME * frame);
        s.keyframe_abandoned(frame);
    }
    assert!(s.keyframe_loss_active());
    assert!(!s.automatic_pacing_enabled());
    s.stats.video_frame.store(47, Ordering::Release);
    for frame in 44..=46 {
        s.record_fec_status(&fec_report(frame, 7, 1));
    }
    assert!(s.automatic_pacing_enabled());
}

#[test]
fn host_only_pyrowave_legacy_and_disabled_keep_existing_keyframe_loss_behavior() {
    // Host-only socket refusal after a lifted startup latch is Sustained + host_only: no handoff.
    let s = Session::new(
        launch("host-only-after-lift", Role::Stream),
        Negotiated {
            codec: 1,
            bitrate_kbps: 40_000,
            client_wireless: false,
            moonlight_features: butterpollo_media_policy::rtsp::FEC_STATUS_FEATURE,
            ..Default::default()
        },
    );
    s.configure_adaptive_pacing_on_loss(true);
    s.configure_keyframe_loss(true);
    let t0 = Instant::now() - Duration::from_secs(20);
    s.keyframe_loss_bps_at(FAST, 80_000_000, 1, true, t0);
    let latched =
        s.keyframe_loss
            .lock()
            .unwrap()
            .unrecoverable(1, 2, FRAME, t0 + Duration::from_millis(100));
    s.report_keyframe_loss(latched, None);
    let mut frame = 2;
    while s.keyframe_loss_active() {
        s.keyframe_loss_bps_at(FAST, 80_000_000, frame, false, t0 + FRAME * frame);
        frame += 1;
        assert!(frame < 2_000);
    }
    for f in frame..frame + 3 {
        s.keyframe_loss_bps(FAST, 80_000_000, f, false);
        s.keyframe_abandoned(f);
    }
    assert!(s.keyframe_loss_active());
    assert!(!s.automatic_pacing_enabled());
    assert_eq!(s.keyframe_loss_rate_bps(FAST, 80_000_000), 80_000_000);

    // PyroWave (codec 3), legacy client (no FEC_STATUS_FEATURE), and disabled handoff keep KeyframeLoss.
    for (label, codec, features, enable_handoff) in [
        (
            "pyrowave",
            3,
            butterpollo_media_policy::rtsp::FEC_STATUS_FEATURE,
            true,
        ),
        ("legacy", 1, 0, true),
        (
            "disabled",
            1,
            butterpollo_media_policy::rtsp::FEC_STATUS_FEATURE,
            false,
        ),
    ] {
        let s = Session::new(
            launch(label, Role::Stream),
            Negotiated {
                codec,
                bitrate_kbps: 40_000,
                client_wireless: false,
                moonlight_features: features,
                ..Default::default()
            },
        );
        s.configure_adaptive_pacing_on_loss(enable_handoff);
        s.configure_keyframe_loss(true);
        s.stats.video_frame.store(13, Ordering::Release);
        for frame in 10..=12 {
            s.record_fec_status(&fec_report(frame, 7, 1));
        }
        assert!(s.keyframe_loss_active(), "{label}");
        assert!(!s.automatic_pacing_enabled(), "{label}");
        assert_eq!(
            s.keyframe_loss_rate_bps(FAST, 80_000_000),
            80_000_000,
            "{label}"
        );
    }
}

#[test]
fn remembered_route_hands_over_to_adaptive_pacing_and_roam_resets_loss_handoff() {
    let s = Session::new(
        launch("remembered-handoff", Role::Stream),
        Negotiated {
            codec: 2,
            bitrate_kbps: 40_000,
            client_wireless: false,
            moonlight_features: butterpollo_media_policy::rtsp::FEC_STATUS_FEATURE,
            ..Default::default()
        },
    );
    s.configure_automatic_pacing(false);
    s.configure_adaptive_pacing_on_loss(true);
    s.configure_remembered_keyframe_loss(true, true);
    assert!(s.keyframe_loss_active());
    assert!(s.automatic_pacing_enabled());
    assert_eq!(s.video_pacing_bps(FAST, 50_000_000), 80_000_000);
    let warning = s.info()["warnings"][0]["message"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(warning.contains("probes up"), "{warning}");
    assert!(warning.contains("24 hours"), "{warning}");
    assert!(warning.contains("Reset it in Devices"), "{warning}");

    // A roam drops the loss-triggered adaptive pacing so the new path starts fresh.
    s.new_network_path();
    assert!(!s.automatic_pacing_enabled());
    assert!(!s.keyframe_loss_active());
    assert!(s.info()["warnings"].as_array().unwrap().is_empty());
    // Sustained loss on the new path can hand over again.
    s.stats.video_frame.store(103, Ordering::Release);
    for frame in 100..=102 {
        s.record_fec_status(&fec_report(frame, 7, 1));
    }
    assert!(s.automatic_pacing_enabled());
}

#[test]
fn post_loss_caps_by_failed_rate_cuts_on_single_keyframe_and_skips_probes_at_low_fec() {
    // 1) Provisional Startup latch keeps `last_fast_bps` at FAST (800 Mbps), so
    // confirming to Sustained caps post-loss ceiling at 400 Mbps (FAST / 2).
    let s = Session::new(
        launch("failed-bps-cap", Role::Stream),
        Negotiated {
            codec: 1,
            bitrate_kbps: 40_000,
            client_wireless: false,
            moonlight_features: butterpollo_media_policy::rtsp::FEC_STATUS_FEATURE,
            ..Default::default()
        },
    );
    s.configure_adaptive_pacing_on_loss(true);
    s.configure_keyframe_loss(true);
    let t0 = Instant::now() - Duration::from_secs(1);
    assert_eq!(s.keyframe_loss_bps_at(FAST, 80_000_000, 1, true, t0), FAST);
    s.stats.video_frame.store(2, Ordering::Release);
    s.record_fec_status(&fec_report(1, 7, 1));
    assert!(!s.automatic_pacing_enabled());
    for frame in 2..=6 {
        s.keyframe_loss
            .lock()
            .unwrap()
            .sending_at(frame, false, t0 + FRAME * frame);
    }
    s.stats.video_frame.store(7, Ordering::Release);
    for frame in 4..=6 {
        s.record_fec_status(&fec_report(frame, 7, 1));
    }
    assert!(s.automatic_pacing_enabled());
    s.video_pacing_bps(FAST, 50_000_000);
    assert_eq!(
        s.automatic_pacing.lock().unwrap().snapshot()["ceiling_kbps"],
        400_000
    );

    // 2) Single unrecoverable keyframe after handoff cuts pacing on the next tick.
    let start = Instant::now();
    s.tick_automatic_pacing(start, Duration::from_millis(2));
    let mut now = start;
    for step in 1..=6u64 {
        now = start + Duration::from_millis(500 * step);
        pacing_traffic(&s, 6 + 30 * step);
        s.record_control_ping(now, &[4, 0, 0, 0, 0, 0, 0, 0]);
        s.tick_automatic_pacing(now, Duration::from_millis(2));
    }
    let elevated = s.video_pacing_bps(FAST, 50_000_000);
    assert!(elevated > 80_000_000);
    let kf = 6 + 30 * 6 + 1;
    s.keyframe_loss_bps_at(elevated, 80_000_000, kf as u32, true, now);
    s.stats.video_frame.store(kf + 1, Ordering::Release);
    s.record_fec_status(&fec_report(kf as u32, 7, 1));
    let next = now + Duration::from_millis(500);
    pacing_traffic(&s, kf + 30);
    let cut = s
        .tick_automatic_pacing(next, Duration::from_millis(2))
        .expect("single post-handoff keyframe loss triggers cut");
    assert!(cut.1 < cut.0, "{cut:?}");

    // 3) Low FEC (< 10%) holds the safe rate without probing after handoff.
    let low = Session::new(
        launch("low-fec-post-loss", Role::Stream),
        Negotiated {
            codec: 1,
            bitrate_kbps: 40_000,
            client_wireless: false,
            moonlight_features: butterpollo_media_policy::rtsp::FEC_STATUS_FEATURE,
            ..Default::default()
        },
    );
    low.configure_fec(false, 5);
    low.configure_adaptive_pacing_on_loss(true);
    low.configure_remembered_keyframe_loss(true, true);
    assert!(low.automatic_pacing_enabled());
    assert_eq!(low.video_pacing_bps(FAST, 40_000_000), 80_000_000);
    let start = Instant::now();
    low.tick_automatic_pacing(start, Duration::from_millis(2));
    for step in 1..=10u64 {
        let now = start + Duration::from_millis(500 * step);
        pacing_traffic(&low, 30 * step);
        low.record_control_ping(now, &[4, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            low.tick_automatic_pacing(now, Duration::from_millis(2)),
            None
        );
    }
    assert_eq!(low.video_pacing_bps(FAST, 40_000_000), 80_000_000);
}

#[test]
fn post_loss_preserves_non_fast_routes_ignores_stale_roam_and_keeps_adaptive_on_send_refusal() {
    let wired_fec = || Negotiated {
        codec: 1,
        bitrate_kbps: 40_000,
        client_wireless: false,
        moonlight_features: butterpollo_media_policy::rtsp::FEC_STATUS_FEATURE,
        ..Default::default()
    };

    // 1) Non-fast route (host Wi-Fi / 100 Mbps NIC: base == safe == 80 Mbps) does not store
    // `last_fast_bps`, so sustained loss keeps 80 Mbps (2x bitrate) instead of clamping to 62.5 Mbps.
    let s = Session::new(launch("non-fast-route", Role::Stream), wired_fec());
    s.configure_adaptive_pacing_on_loss(true);
    s.configure_keyframe_loss(true);
    let t0 = Instant::now() - Duration::from_secs(1);
    assert_eq!(
        s.keyframe_loss_bps_at(80_000_000, 80_000_000, 1, true, t0),
        80_000_000
    );
    s.stats.video_frame.store(13, Ordering::Release);
    for frame in 10..=12 {
        s.record_fec_status(&fec_report(frame, 7, 1));
    }
    assert!(s.automatic_pacing_enabled());
    // Mid-frame handoff before `video_pacing_bps` runs still clamps pre-handoff FAST to safe.
    assert_eq!(s.keyframe_loss_bps(FAST, 80_000_000, 13, false), 80_000_000);
    assert_eq!(s.video_pacing_bps(80_000_000, 50_000_000), 80_000_000);
    assert_eq!(
        s.automatic_pacing.lock().unwrap().snapshot()["ceiling_kbps"],
        80_000
    );

    // 2) Stale pre-roam latch dropped after `new_network_path()` cannot re-enable post-loss pacing.
    let r = Session::new(launch("roam-race", Role::Stream), wired_fec());
    r.configure_adaptive_pacing_on_loss(true);
    r.configure_keyframe_loss(true);
    let pre_roam = r.path_epoch.load(Ordering::Acquire);
    let stale_latch = Some(butterpollo_media_policy::keyframe_loss::Latched {
        reason: "stale pre-roam loss",
        detail: "frames [1, 2, 3]".into(),
        kind: butterpollo_media_policy::keyframe_loss::LatchKind::Sustained,
        confirmed: false,
        from: None,
        host_only: false,
    });
    r.new_network_path();
    r.report_keyframe_loss_for_epoch(stale_latch, None, pre_roam);
    assert!(!r.automatic_pacing_enabled());
    assert!(r.info()["warnings"].as_array().unwrap().is_empty());

    // 3) Host-only send refusal on an already-adaptive remembered route keeps adaptive wording.
    let m = Session::new(launch("remembered-send-refusal", Role::Stream), wired_fec());
    m.configure_adaptive_pacing_on_loss(true);
    m.configure_remembered_keyframe_loss(true, true);
    assert!(m.automatic_pacing_enabled());
    for frame in 1..=3 {
        m.keyframe_loss_bps(80_000_000, 80_000_000, frame, frame == 1);
        m.keyframe_abandoned(frame);
    }
    assert!(m.automatic_pacing_enabled());
    let warning = m.info()["warnings"][0]["message"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        warning.contains("Adaptive packet pacing remains active"),
        "{warning}"
    );
    assert!(!warning.contains("paced like Wi-Fi"), "{warning}");
}
