use super::*;
use butterpollo_session::session::Role;
use std::time::Instant;

/// #59: a keyframe's line says whether AMF re-encoded it, the cap the host
/// asked for and whether it lost FEC; keyframes past FEC are counted.
#[test]
fn recovery_keyframes_count_the_ones_past_fec() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = Session::new(
        fixture.launch(fixture.client(0), Role::Stream),
        butterpollo_media_policy::rtsp::Negotiated::default(),
    );
    let config = Config::default();
    let frame = |idr: bool, fec_limited: bool| RecoveryFrame {
        frame: 1001,
        idr,
        after_invalidation: !idr,
        payload_bytes: 1_908_217,
        cap_bytes: Some(1_166_893),
        cap_target_bytes: Some(1_166_893),
        reencoded: true,
        fec_limited,
        wire_bytes: 1_960_000,
        pace_bps: 800_000_000,
        elapsed: Duration::from_millis(20),
        lost: false,
    };
    let past_fec = || {
        session
            .stats
            .keyframes_past_fec
            .load(std::sync::atomic::Ordering::Relaxed)
    };
    report(&session, &config, frame(true, true));
    assert_eq!(past_fec(), 1);
    report(&session, &config, frame(true, false));
    report(&session, &config, frame(false, true));
    assert_eq!(past_fec(), 1);
    assert_eq!(session.info()["keyframes_past_fec"], 1);
}

/// Review 2026-11-07: an IPv4 client on a dual-stack socket arrives
/// IPv4-mapped; its wire budget has the IPv4 headers the pacer charges.
#[test]
fn a_mapped_ipv4_client_has_the_ipv4_wire_budget() {
    let fixture = crate::state::test_support::Fixture::new();
    let packetizer = VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: 20,
        min_fec: 2,
        key: Some([0; 16]),
    };
    let period = Duration::from_secs_f64(1. / 60.);
    let route = Link::default();
    let session = Session::new(
        fixture.launch(fixture.client(0), Role::Stream),
        butterpollo_media_policy::rtsp::Negotiated::default(),
    );
    session
        .bitrate
        .store(40_000, std::sync::atomic::Ordering::Release);
    let config = Config::default();
    let budget = |peer: &str| {
        pace(
            &session,
            &config,
            &packetizer,
            period,
            peer.parse().unwrap(),
            route,
        )
        .needed
    };
    let ipv4 = budget("192.0.2.9:12345");
    assert!(budget("[2001:db8::9]:12345") > ipv4);
    assert_eq!(budget("[::ffff:192.0.2.9]:12345"), ipv4);
}

/// A roam routes the first picture sent to the new address, even when a picture
/// queued before it went to the old one after the roam.
#[test]
fn a_route_follows_the_address_it_sends_to() {
    let (old, new): (SocketAddr, SocketAddr) = (
        "192.0.2.1:9".parse().unwrap(),
        "198.51.100.4:9".parse().unwrap(),
    );
    let mut routed = None;
    assert!(new_route(&mut routed, old), "the first picture");
    assert!(!new_route(&mut routed, old));
    // The session roamed, but this picture was already queued for `old`.
    assert!(!new_route(&mut routed, old));
    assert!(
        new_route(&mut routed, new),
        "the first picture on the new path"
    );
    assert!(!new_route(&mut routed, new));
    assert!(new_route(&mut routed, old), "a roam back");
}

#[test]
fn a_clients_link_hint_caps_the_startup_pace_for_conventional_codecs() {
    let fixture = crate::state::test_support::Fixture::new();
    let packetizer = VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: 20,
        min_fec: 2,
        key: Some([0; 16]),
    };
    let period = Duration::from_secs_f64(1. / 60.);
    let peer = "192.0.2.9:12345".parse().unwrap();
    let route = Link {
        bps: 1_000_000_000,
        ..Default::default()
    };
    // A 1 GbE host: stock Moonlight (no hint) and a 1 GbE or 2.5 GbE
    // client stay at 800 Mbps; a 100 Mbps client is paced at 80.
    for (hint, expected) in [
        (0, 800_000_000),
        (1000, 800_000_000),
        (2500, 800_000_000),
        (100, 80_000_000),
    ] {
        let session = Session::new(
            fixture.launch(fixture.client(0), Role::Stream),
            butterpollo_media_policy::rtsp::Negotiated {
                pyrowave_client_link_mbps: hint,
                ..Default::default()
            },
        );
        session
            .bitrate
            .store(40_000, std::sync::atomic::Ordering::Release);
        session.configure_keyframe_loss(true);
        let config = Config::default();
        assert_eq!(
            pace(&session, &config, &packetizer, period, peer, route).bps,
            expected,
            "hint {hint}"
        );
        assert_eq!(
            preview_pace(&session, &config, &packetizer, period, peer, route).bps,
            expected,
            "hint {hint}"
        );
        // The hint was about the path a roaming client left.
        session.roamed("198.51.100.4".parse().unwrap(), Instant::now());
        assert_eq!(
            pace(&session, &config, &packetizer, period, peer, route).bps,
            800_000_000,
            "hint {hint} after a roam"
        );
    }
}

fn link_stream(fixture: &crate::state::test_support::Fixture) -> std::sync::Arc<Session> {
    let session = Session::new(
        fixture.launch(fixture.client(0), Role::Stream),
        butterpollo_media_policy::rtsp::Negotiated {
            pyrowave_client_link_mbps: 2500,
            ..Default::default()
        },
    );
    session
        .bitrate
        .store(400_000, std::sync::atomic::Ordering::Release);
    session.configure_keyframe_loss(true);
    session
}

/// `pacing_follow_client_link`: a 10 GbE host and a 2.5 GbE client that
/// says so are paced at 2 Gbps instead of 800 Mbps, only when it is on.
#[test]
fn following_the_client_link_lifts_the_wired_ceiling() {
    let fixture = crate::state::test_support::Fixture::new();
    let packetizer = VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: 20,
        min_fec: 2,
        key: Some([0; 16]),
    };
    let period = Duration::from_secs_f64(1. / 60.);
    let peer: SocketAddr = "192.0.2.9:12345".parse().unwrap();
    let route = Link {
        bps: 10_000_000_000,
        ..Default::default()
    };
    let session = link_stream(&fixture);
    let mut config = Config::default();
    assert_eq!(
        pace(&session, &config, &packetizer, period, peer, route).bps,
        800_000_000
    );
    config
        .values
        .insert("pacing_follow_client_link".into(), "true".into());
    assert_eq!(
        pace(&session, &config, &packetizer, period, peer, route).bps,
        2_000_000_000
    );
}

#[test]
fn pyrowave_follows_confirmed_multi_gigabit_link_by_default_and_respects_explicit_false() {
    let fixture = crate::state::test_support::Fixture::new();
    let packetizer = VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: 20,
        min_fec: 2,
        key: Some([0; 16]),
    };
    let period = Duration::from_secs_f64(1. / 60.);
    let peer: SocketAddr = "192.0.2.9:12345".parse().unwrap();
    let route = Link {
        bps: 10_000_000_000,
        ..Default::default()
    };
    // Conventional codecs (H.264/HEVC/AV1) stay at 800 Mbps unless opted in;
    // PyroWave uses `pyrowave_send::transport::pace` (tested there).
    for codec in [0u8, 1, 2] {
        let conventional = Session::new(
            fixture.launch(fixture.client(0), Role::Stream),
            butterpollo_media_policy::rtsp::Negotiated {
                codec,
                pyrowave_client_link_mbps: 2500,
                ..Default::default()
            },
        );
        conventional
            .bitrate
            .store(400_000, std::sync::atomic::Ordering::Release);
        conventional.configure_keyframe_loss(true);
        let mut config = Config::default();
        assert_eq!(
            pace(&conventional, &config, &packetizer, period, peer, route).bps,
            800_000_000,
            "codec {codec}"
        );
        config
            .values
            .insert("pacing_follow_client_link".into(), "true".into());
        assert_eq!(
            pace(&conventional, &config, &packetizer, period, peer, route).bps,
            2_000_000_000,
            "codec {codec} opted in"
        );
    }
}

#[test]
fn a_client_link_hint_below_the_bitrate_floors_the_pace_at_one_point_one_times_it() {
    let fixture = crate::state::test_support::Fixture::new();
    let packetizer = VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: 20,
        min_fec: 2,
        key: Some([0; 16]),
    };
    let period = Duration::from_secs_f64(1. / 60.);
    let peer = "192.0.2.9:12345".parse().unwrap();
    let route = Link {
        bps: 1_000_000_000,
        ..Default::default()
    };
    let session = Session::new(
        fixture.launch(fixture.client(0), Role::Stream),
        butterpollo_media_policy::rtsp::Negotiated {
            pyrowave_client_link_mbps: 100,
            ..Default::default()
        },
    );
    session
        .bitrate
        .store(150_000, std::sync::atomic::Ordering::Release);
    session.configure_keyframe_loss(true);
    let config = Config::default();
    assert_eq!(
        pace(&session, &config, &packetizer, period, peer, route).bps,
        165_000_000
    );
    assert_eq!(
        preview_pace(&session, &config, &packetizer, period, peer, route).bps,
        165_000_000
    );
    // A lighter stream is capped by the link as before.
    session
        .bitrate
        .store(50_000, std::sync::atomic::Ordering::Release);
    assert_eq!(
        pace(&session, &config, &packetizer, period, peer, route).bps,
        80_000_000
    );
}

/// A 40 Mbps stream on a wired 1 GbE host route, as a client that does or
/// does not report a wireless route. `configure` is what the stream start
/// calls; it decides whether the adaptive controller owns the stream.
fn wireless_hint_stream(
    fixture: &crate::state::test_support::Fixture,
    settings: &str,
    codec: u8,
    client_wireless: bool,
) -> (std::sync::Arc<Session>, Config, bool) {
    let session = Session::new(
        fixture.launch(fixture.client(0), Role::Stream),
        butterpollo_media_policy::rtsp::Negotiated {
            codec,
            client_wireless,
            ..Default::default()
        },
    );
    session
        .bitrate
        .store(40_000, std::sync::atomic::Ordering::Release);
    let config = Config::parse(settings).unwrap();
    let keyframe_loss = configure(&session, &config);
    session.configure_keyframe_loss(keyframe_loss);
    (session, config, keyframe_loss)
}

#[test]
fn the_wireless_hint_sets_the_route_pace_only_when_no_controller_owns_the_stream() {
    let fixture = crate::state::test_support::Fixture::new();
    let packetizer = VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: 20,
        min_fec: 2,
        key: Some([0; 16]),
    };
    let period = Duration::from_secs_f64(1. / 60.);
    let peer = "192.0.2.9:12345".parse().unwrap();
    let route = Link {
        bps: 1_000_000_000,
        wireless: false,
        ..Default::default()
    };
    let needed = network_pacing::video_wire_rate_bps(&packetizer, 40_000, period, false);
    let wifi = network_pacing::rate_bps(0, 40_000, route.bps, true, needed);
    assert_eq!(wifi, 80_000_000);
    // No controller: the hint paces like a Wi-Fi host, without it 800 Mbps.
    for (settings, codec) in [
        ("wireless_adaptive_pacing=false", 1),
        ("", 3),
        (
            "pacing_max_bitrate_kbps=0\nwireless_adaptive_pacing=false",
            0,
        ),
    ] {
        for (hint, expected) in [(true, wifi), (false, 800_000_000)] {
            let (session, config, _) = wireless_hint_stream(&fixture, settings, codec, hint);
            assert!(!session.automatic_pacing_enabled(), "{settings}");
            let what = format!("{settings} codec {codec} hint {hint}");
            assert_eq!(
                pace(&session, &config, &packetizer, period, peer, route).bps,
                expected,
                "{what}"
            );
            assert_eq!(
                preview_pace(&session, &config, &packetizer, period, peer, route).bps,
                expected,
                "{what}"
            );
        }
    }
    // A manual limit keeps its own policy with or without the hint.
    let (session, config, _) =
        wireless_hint_stream(&fixture, "pacing_max_bitrate_kbps=300000", 1, true);
    assert_eq!(
        pace(&session, &config, &packetizer, period, peer, route).bps,
        300_000_000
    );
    // The controller owns it: unknown-link wireless clients cap their ceiling at
    // 4x stream bitrate instead of ratcheting to the wired port speed, while
    // wired `host_adaptive_pacing` and known-link streams keep 800 Mbps.
    let (owned, owned_config, owned_keyframe_loss) = wireless_hint_stream(&fixture, "", 1, true);
    let (host_opt_in, host_config, _) =
        wireless_hint_stream(&fixture, "host_adaptive_pacing=true", 1, false);
    assert!(owned.automatic_pacing_enabled() && host_opt_in.automatic_pacing_enabled());
    assert!(!owned.client_wireless_sets_route_pacing());
    assert!(!owned_keyframe_loss);
    let seed = pace(&owned, &owned_config, &packetizer, period, peer, route);
    assert_eq!(seed.bps, 80_000_000);
    assert_eq!(
        seed.bps,
        pace(&host_opt_in, &host_config, &packetizer, period, peer, route).bps
    );
    assert_eq!(
        preview_pace(&owned, &owned_config, &packetizer, period, peer, route).bps,
        seed.bps
    );
    assert_eq!(
        owned.automatic_pacing.lock().unwrap().snapshot()["ceiling_kbps"],
        160_000
    );
    assert_eq!(
        host_opt_in.automatic_pacing.lock().unwrap().snapshot()["ceiling_kbps"],
        800_000
    );
    // The encoder's recovery-frame cap is what `host_adaptive_pacing`
    // streams get, not the smaller one of a Wi-Fi host.
    let cap = |s: &Session, c: &Config| {
        let pace = preview_pace(s, c, &packetizer, period, peer, route);
        network_pacing::recovery_frame_cap(pace.bps, 40_000, period, 20, 1392, false)
    };
    assert_eq!(cap(&owned, &owned_config), cap(&host_opt_in, &host_config));
    // A client that reports a 1 Gbps link speed keeps the known-link ceiling
    // until a roam clears the initial link hint.
    let known = fixture.launch(fixture.client(0), Role::Stream);
    let known = Session::new(
        known,
        butterpollo_media_policy::rtsp::Negotiated {
            codec: 1,
            client_wireless: true,
            pyrowave_client_link_mbps: 1000,
            ..Default::default()
        },
    );
    known
        .bitrate
        .store(40_000, std::sync::atomic::Ordering::Release);
    configure(&known, &owned_config);
    pace(&known, &owned_config, &packetizer, period, peer, route);
    assert_eq!(
        known.automatic_pacing.lock().unwrap().snapshot()["ceiling_kbps"],
        800_000
    );
    known.roamed(peer.ip(), std::time::Instant::now());
    pace(&known, &owned_config, &packetizer, period, peer, route);
    assert_eq!(
        known.automatic_pacing.lock().unwrap().snapshot()["ceiling_kbps"],
        160_000
    );
    // PyroWave is not owned by the controller unless it is opted in.
    let (pyrowave, _, _) = wireless_hint_stream(&fixture, "", 3, true);
    assert!(pyrowave.client_wireless_sets_route_pacing());
    let (pyrowave, _, _) = wireless_hint_stream(&fixture, "host_adaptive_pacing=true", 3, true);
    assert!(!pyrowave.client_wireless_sets_route_pacing());
}

#[test]
fn the_first_picture_and_live_loss_use_the_same_pace_as_sending() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(fixture.client(0), Role::Stream);
    session
        .bitrate
        .store(54_000, std::sync::atomic::Ordering::Release);
    session.configure_keyframe_loss(true);
    let config = Config::default();
    let packetizer = VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: 20,
        min_fec: 2,
        key: Some([0; 16]),
    };
    let period = Duration::from_secs_f64(1. / 119.88);
    let peer = "127.0.0.1:12345".parse().unwrap();
    let route = Link {
        bps: 1_000_000_000,
        ..Default::default()
    };
    let fast = preview_pace(&session, &config, &packetizer, period, peer, route);
    assert_eq!(fast.bps, 800_000_000);
    // The first keyframe and the client's requests around it are the
    // stream's start; a keyframe lost after that is what slows it.
    session.keyframe_loss.lock().unwrap().sending_at(
        1,
        true,
        Instant::now()
            - butterpollo_media_policy::keyframe_loss::STARTUP_WINDOW
            - Duration::from_millis(50),
    );
    session.request_recovery_keyframe();
    // Queries while waiting for capture must not consume the decoder's
    // finite keyframe history or count a picture as sent.
    session.keyframe_loss_bps(fast.bps, 108_000_000, 2, true);
    for _ in 0..100 {
        assert_eq!(
            preview_pace(&session, &config, &packetizer, period, peer, route).bps,
            fast.bps
        );
    }
    session.request_recovery_keyframe();
    let slow = preview_pace(&session, &config, &packetizer, period, peer, route);
    assert_eq!(slow.bps, 108_000_000);
    let cap = |bps| network_pacing::recovery_frame_cap(bps, 54_000, period, 20, 1392, false);
    assert!(cap(slow.bps) < cap(fast.bps));
    session.configure_remembered_keyframe_loss(true, true);
    assert_eq!(
        pace(&session, &config, &packetizer, period, peer, route).bps,
        slow.bps
    );
}

#[test]
fn per_frame_packetizer_fec_swings_do_not_churn_adaptive_pacing_floor() {
    let fixture = crate::state::test_support::Fixture::new();
    let (session, config, _) = wireless_hint_stream(&fixture, "", 1, true);
    assert!(session.automatic_pacing_enabled());
    let period = Duration::from_secs_f64(1. / 60.);
    let peer = "192.0.2.9:12345".parse().unwrap();
    let route = Link {
        bps: 1_000_000_000,
        wireless: false,
        ..Default::default()
    };
    let mut packetizer = VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: session.fec_percent(),
        min_fec: 2,
        key: Some([0; 16]),
    };
    let baseline = pace(&session, &config, &packetizer, period, peer, route);
    // Simulate per-frame statistical / recovery-IDR FEC changes on the packetizer.
    for per_frame_fec in [5, 10, 20, 40, 80] {
        packetizer.fec_percent = per_frame_fec;
        let next = pace(&session, &config, &packetizer, period, peer, route);
        assert_eq!(next.needed, baseline.needed, "fec={per_frame_fec}");
        assert_eq!(next.bps, baseline.bps, "fec={per_frame_fec}");
    }
}

#[test]
fn recovery_estimate_includes_existing_pacing_debt_and_remaining_wire_bytes() {
    let now = Instant::now();
    assert_eq!(
        recovery_remaining(now, now + Duration::from_millis(1), 50_000, 80_000_000),
        Duration::from_millis(6)
    );
    assert_eq!(
        recovery_remaining(now, now - Duration::from_millis(20), 50_000, 80_000_000),
        Duration::from_millis(5)
    );
    assert_eq!(
        recovery_remaining(now, now + Duration::from_millis(1), 0, 80_000_000),
        Duration::from_millis(1)
    );
}

fn loss_fec_report(frame: u32) -> [u8; 21] {
    let mut report = [0u8; 21];
    report[..4].copy_from_slice(&frame.to_be_bytes());
    for (offset, word) in [(4, 11u16), (10, 10), (12, 2), (14, 7), (16, 1)] {
        report[offset..offset + 2].copy_from_slice(&word.to_be_bytes());
    }
    report[18] = 20;
    report[20] = 1;
    report
}

#[test]
fn wired_auto_configures_loss_handoff_and_reports_adaptive_loss_reason() {
    use std::sync::atomic::Ordering;

    let fixture = crate::state::test_support::Fixture::new();
    let packetizer = VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: 20,
        min_fec: 2,
        key: Some([0; 16]),
    };
    let period = Duration::from_secs_f64(1. / 60.);
    let peer: SocketAddr = "192.0.2.9:12345".parse().unwrap();
    let route = Link {
        bps: 1_000_000_000,
        wireless: false,
        ..Default::default()
    };

    let session = Session::new(
        fixture.launch(fixture.client(0), Role::Stream),
        butterpollo_media_policy::rtsp::Negotiated {
            codec: 1,
            bitrate_kbps: 40_000,
            client_wireless: false,
            moonlight_features: butterpollo_media_policy::rtsp::FEC_STATUS_FEATURE,
            ..Default::default()
        },
    );
    let config = Config::default();
    let keyframe_loss = configure(&session, &config);
    assert!(keyframe_loss);
    session.configure_keyframe_loss(keyframe_loss);
    assert!(!session.automatic_pacing_enabled());
    assert_eq!(pacing_reason(&session, &config), "route pacing");

    // Provisional Startup latch stays on KeyframeLoss until confirmed.
    session.keyframe_loss.lock().unwrap().sending_at(
        1,
        true,
        Instant::now() - Duration::from_millis(700),
    );
    session.stats.video_frame.store(2, Ordering::Release);
    session.record_fec_status(&loss_fec_report(1));
    assert!(session.keyframe_loss_active());
    assert!(!session.automatic_pacing_enabled());
    assert_ne!(
        pacing_reason(&session, &config),
        "adaptive packet pacing after video loss"
    );
    assert_eq!(
        pace(&session, &config, &packetizer, period, peer, route).bps,
        80_000_000
    );

    // Sustained client-reported loss activates adaptive pacing and its recovery reason.
    session.stats.video_frame.store(13, Ordering::Release);
    for frame in 10..=12 {
        session.record_fec_status(&loss_fec_report(frame));
    }
    assert!(session.automatic_pacing_enabled());
    assert_eq!(
        pacing_reason(&session, &config),
        "adaptive packet pacing after video loss"
    );
    assert_eq!(
        pace(&session, &config, &packetizer, period, peer, route).bps,
        80_000_000
    );
    let start = Instant::now();
    session.tick_automatic_pacing(start, Duration::from_millis(2));
    for step in 1..=20u64 {
        let now = start + Duration::from_millis(500 * step);
        let frames = 30 * step;
        session.stats.frames.store(frames, Ordering::Relaxed);
        session.stats.packets.store(frames * 70, Ordering::Relaxed);
        session
            .stats
            .bytes
            .store(frames * 83_334, Ordering::Relaxed);
        session
            .stats
            .video_frame
            .store(frames + 1, Ordering::Release);
        session.record_control_ping(now, &[4, 0, 0, 0, 0, 0, 0, 0]);
        session.tick_automatic_pacing(now, Duration::from_millis(2));
    }
    assert!(pace(&session, &config, &packetizer, period, peer, route).bps > 80_000_000);
    report(
        &session,
        &config,
        RecoveryFrame {
            frame: 20,
            idr: true,
            after_invalidation: false,
            payload_bytes: 100_000,
            cap_bytes: None,
            cap_target_bytes: None,
            reencoded: false,
            fec_limited: false,
            wire_bytes: 120_000,
            pace_bps: 80_000_000,
            elapsed: Duration::from_millis(2),
            lost: false,
        },
    );

    // Off, Fixed, PyroWave, and legacy clients without FEC_STATUS_FEATURE do not hand over.
    for (label, text, codec, features) in [
        (
            "off",
            "packet_pacing=off",
            1,
            butterpollo_media_policy::rtsp::FEC_STATUS_FEATURE,
        ),
        (
            "fixed",
            "packet_pacing=80000",
            1,
            butterpollo_media_policy::rtsp::FEC_STATUS_FEATURE,
        ),
        (
            "pyrowave",
            "",
            3,
            butterpollo_media_policy::rtsp::FEC_STATUS_FEATURE,
        ),
        ("legacy", "", 1, 0),
    ] {
        let cfg = Config::parse(text).unwrap();
        let s = Session::new(
            fixture.launch(fixture.client(0), Role::Stream),
            butterpollo_media_policy::rtsp::Negotiated {
                codec,
                bitrate_kbps: 40_000,
                client_wireless: false,
                moonlight_features: features,
                ..Default::default()
            },
        );
        let kf = configure(&s, &cfg);
        s.configure_keyframe_loss(kf);
        s.stats.video_frame.store(13, Ordering::Release);
        for frame in 10..=12 {
            s.record_fec_status(&loss_fec_report(frame));
        }
        assert!(!s.automatic_pacing_enabled(), "{label}");
        assert_ne!(
            pacing_reason(&s, &cfg),
            "adaptive packet pacing after video loss",
            "{label}"
        );
    }
}

mod route_unavailable;
