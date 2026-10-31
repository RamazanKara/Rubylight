use super::*;

fn controller() -> AdaptiveBitrate {
    let mut c = AdaptiveBitrate::default();
    let stream = Negotiated {
        bitrate_kbps: 20_000,
        configured_bitrate_kbps: 30_000,
        ..Default::default()
    };
    c.configure(true, &stream, 0, 20);
    c
}
fn run(
    c: &mut AdaptiveBitrate,
    start: Instant,
    rounds: u64,
    busy: bool,
    ping: bool,
    fec: usize,
) -> u32 {
    let mut kbps = 20_000;
    let mut bytes = 0;
    for i in 0..rounds {
        let now = start + Duration::from_secs(i);
        bytes += if busy { u64::from(kbps) * 125 } else { 0 };
        if ping {
            c.on_ping(now, &[4, 0, 0, 0, 0, 0, 0, 0]);
        }
        if let Some(target) = c.tick(
            now,
            Traffic {
                frames: i * if busy { 60 } else { 1 },
                bytes,
                send_losses: 0,
            },
            Duration::from_millis(8),
            fec,
            Duration::from_secs_f64(1. / 60.),
            kbps,
        ) {
            kbps = target;
        }
    }
    kbps
}
#[test]
fn no_feedback_and_client_ownership_leave_fixed_bitrate_untouched() {
    let mut c = controller();
    assert_eq!(run(&mut c, Instant::now(), 60, true, true, 20), 20_000);
    assert_eq!(c.video_pacing_bps(), 0);
    c.on_block(Block::Unrecoverable);
    c.client_takeover();
    assert_eq!(run(&mut c, Instant::now(), 60, true, true, 20), 20_000);
    assert_eq!(c.video_pacing_bps(), 0);
}
#[test]
fn losses_reduce_budget_then_busy_clean_streams_recover_slowly() {
    let start = Instant::now();
    let mut c = controller();
    c.on_block(Block::Unrecoverable);
    let low = run(&mut c, start, 2, true, true, 20);
    assert!(low < 20_000);
    let low_budget = c.current_bps;
    let restored = run(&mut c, start + Duration::from_secs(3), 90, true, true, 20);
    assert!(restored > low);
    assert!(c.current_bps > low_budget);
    assert!(c.current_bps <= c.ceiling_bps);
}
#[test]
fn idle_video_and_stale_control_never_count_as_a_clean_link() {
    for (busy, ping) in [(false, true), (true, false), (false, false)] {
        let mut c = controller();
        c.on_block(Block::Unrecoverable);
        run(&mut c, Instant::now(), 2, true, true, 20);
        let budget = c.current_bps;
        run(
            &mut c,
            Instant::now() + Duration::from_secs(10),
            120,
            busy,
            ping,
            5,
        );
        assert_eq!(c.current_bps, budget);
    }
}
#[test]
fn fec_rounding_audio_encryption_and_headers_fit_the_same_wire_ceiling() {
    for size in [256, 1024, 1400] {
        for fec in [0, 5, 20, 50, 100] {
            let stream = Negotiated {
                packet_size: size,
                min_fec: 2,
                encryption: 2,
                audio_channels: 8,
                audio_quality: true,
                ..Default::default()
            };
            let budget = WireBudget::of(&stream);
            let period = Duration::from_secs_f64(1. / 120.);
            let rate = budget.encoder_kbps(30_000_000, 100_000, fec, period);
            assert!(budget.video_bps(rate, fec, period) + budget.audio_bps <= 30_000_000);
        }
    }
}
/// Busy, pinged rounds at `rtt` from round `from`.
fn steady(c: &mut AdaptiveBitrate, start: Instant, from: u64, rounds: u64, rtt: u64) {
    let mut rate = 20_000;
    for i in from..from + rounds {
        let now = start + Duration::from_secs(i);
        c.on_ping(now, &[4, 0, 0, 0, 0, 0, 0, 0]);
        rate = c
            .tick(
                now,
                Traffic {
                    frames: i * 60,
                    bytes: i * 2_500_000,
                    send_losses: 0,
                },
                Duration::from_millis(rtt),
                20,
                Duration::from_secs_f64(1. / 60.),
                rate,
            )
            .unwrap_or(rate);
    }
}
#[test]
fn a_new_path_learns_its_own_minimum_rtt() {
    let start = Instant::now();
    for roam in [false, true] {
        let mut c = controller();
        c.on_block(Block::Clean);
        steady(&mut c, start, 0, 5, 2);
        assert_eq!(c.current_bps, c.ceiling_bps);
        if roam {
            c.new_path();
        }
        steady(&mut c, start, 5, 10, 40);
        if roam {
            assert_eq!(
                c.current_bps, c.ceiling_bps,
                "a steady 40 ms path is not congested"
            );
        } else {
            assert!(
                c.current_bps < c.ceiling_bps,
                "40 ms over a 2 ms baseline is queueing"
            );
            assert_eq!(c.reason, "rising_rtt");
        }
    }
}
#[test]
fn rtt_requires_sustained_congestion_and_unknown_rtt_does_not_poison_baseline() {
    let start = Instant::now();
    let mut c = controller();
    c.on_block(Block::Clean);
    let mut rate = 20_000;
    for (i, rtt) in [0, 8, 50, 8, 50, 50].into_iter().enumerate() {
        let now = start + Duration::from_secs(i as u64);
        c.on_ping(now, &[4, 0, 0, 0, 0, 0, 0, 0]);
        rate = c
            .tick(
                now,
                Traffic {
                    frames: i as u64 * 60,
                    bytes: i as u64 * 2_500_000,
                    send_losses: 0,
                },
                Duration::from_millis(rtt),
                20,
                Duration::from_secs_f64(1. / 60.),
                rate,
            )
            .unwrap_or(rate);
        if i < 5 {
            assert_eq!(c.current_bps, c.ceiling_bps);
        }
    }
    assert!(c.current_bps < c.ceiling_bps);
    assert_eq!(c.reason, "rising_rtt");
}
#[test]
fn configured_wire_cap_applies_with_parity_and_no_recovery_from_just_lower_fec() {
    let mut c = controller();
    let stream = Negotiated {
        bitrate_kbps: 20_000,
        configured_bitrate_kbps: 40_000,
        ..Default::default()
    };
    c.configure(true, &stream, 25_000, 20);
    c.on_block(Block::Unrecoverable);
    let start = Instant::now();
    let low = run(&mut c, start, 2, true, true, 20);
    assert!(c.current_bps < 25_000_000);
    let held = c
        .tick(
            start + Duration::from_secs(3),
            Traffic::default(),
            Duration::ZERO,
            0,
            Duration::from_secs_f64(1. / 60.),
            low,
        )
        .unwrap();
    assert_eq!(held, low);
    assert!(c.video_pacing_bps() + c.budget.unwrap().audio_bps <= 25_000_000);
}
#[test]
fn disabled_streams_never_activate() {
    for (enabled, codec) in [(false, 0), (false, 3)] {
        let mut c = AdaptiveBitrate::default();
        c.configure(
            enabled,
            &Negotiated {
                codec,
                ..Default::default()
            },
            0,
            20,
        );
        c.on_block(Block::Unrecoverable);
        assert!(!c.active());
        assert_eq!(c.video_pacing_bps(), 0);
    }
}
#[test]
fn a_budget_smaller_than_audio_and_minimum_packets_never_stalls_the_pacer() {
    let mut c = controller();
    let stream = Negotiated {
        bitrate_kbps: 500,
        configured_bitrate_kbps: 1_000,
        audio_channels: 8,
        audio_quality: true,
        ..Default::default()
    };
    c.configure(true, &stream, 0, 20);
    c.on_block(Block::Unrecoverable);
    assert!(c.budget_unusable());
    let start = Instant::now();
    for i in 0..4 {
        assert_eq!(
            c.tick(
                start + Duration::from_secs(i),
                Traffic::default(),
                Duration::from_millis(8),
                20,
                Duration::from_secs_f64(1. / 60.),
                500
            ),
            None
        );
    }
    assert_eq!(c.video_pacing_bps(), 0);
    assert_eq!(c.snapshot()["reason"], "wire_budget_too_small");
}
#[test]
fn higher_fps_revalidates_the_minimum_wire_budget() {
    let mut c = controller();
    let stream = Negotiated {
        bitrate_kbps: 2_000,
        configured_bitrate_kbps: 6_000,
        audio_channels: 8,
        audio_quality: true,
        packet_size: 1024,
        min_fec: 2,
        ..Default::default()
    };
    c.configure(true, &stream, 0, 20);
    c.on_block(Block::Clean);
    assert!(c.active());
    let start = Instant::now();
    for i in 0..2 {
        c.tick(
            start + Duration::from_secs(i),
            Traffic::default(),
            Duration::from_millis(8),
            20,
            Duration::from_secs_f64(1. / 240.),
            2_000,
        );
    }
    assert!(!c.active());
    assert_eq!(c.video_pacing_bps(), 0);
}

#[test]
fn pyrowave_loss_floor_keeps_frames_encodable_with_configured_and_minimum_parity() {
    for records in [false, true] {
        for fps in [60, 120] {
            for fec in [0, 20, 255] {
                for min_fec in [0, 2, 8] {
                    let stream = Negotiated {
                        codec: 3,
                        pyrowave_records: records,
                        packet_size: 1392,
                        min_fec,
                        fps,
                        bitrate_kbps: 500_000,
                        configured_bitrate_kbps: 800_000,
                        ..Default::default()
                    };
                    let mut c = AdaptiveBitrate::default();
                    c.configure(true, &stream, 0, fec);
                    let period = crate::reconfigure::Mode::of(&stream).period();
                    let start = Instant::now();
                    let mut rate = stream.bitrate_kbps;
                    for i in 0..64 {
                        c.on_block(Block::Unrecoverable);
                        rate = c
                            .tick(
                                start + Duration::from_secs(i),
                                Traffic::default(),
                                Duration::from_millis(8),
                                // A lower sender FEC percentage does not
                                // change the encoder's configured reserve.
                                0,
                                period,
                                rate,
                            )
                            .unwrap_or(rate);
                        assert!(rate <= stream.bitrate_kbps);
                        assert!(c.current_bps <= c.ceiling_bps);
                        assert!(
                            crate::pyrowave::budget_with_minimum(
                                rate,
                                period,
                                stream.packet_size,
                                records,
                                fec as u32,
                                min_fec,
                            ) >= 16,
                            "records={records}, fps={fps}, fec={fec}, minimum={min_fec}, rate={rate}"
                        );
                    }
                    assert!(c.active());
                    assert_eq!(c.current_bps, c.floor_bps);
                }
            }
        }
    }
}

#[test]
fn pyrowave_minimum_frame_never_raises_an_impossible_user_ceiling() {
    for fps in [60, 120] {
        for (bitrate_kbps, configured_bitrate_kbps, maximum_kbps) in [
            (2_000, 30_000, 0),
            (20_000, 2_000, 0),
            (20_000, 30_000, 2_000),
        ] {
            let stream = Negotiated {
                codec: 3,
                pyrowave_records: true,
                packet_size: 1392,
                min_fec: 2,
                fps,
                bitrate_kbps,
                configured_bitrate_kbps,
                ..Default::default()
            };
            let mut c = AdaptiveBitrate::default();
            c.configure(true, &stream, maximum_kbps, 20);
            c.on_block(Block::Unrecoverable);
            assert!(c.budget_unusable());
            let ceiling = c.ceiling_bps;
            let start = Instant::now();
            for i in 0..4 {
                assert_eq!(
                    c.tick(
                        start + Duration::from_secs(i),
                        Traffic::default(),
                        Duration::from_millis(8),
                        0,
                        crate::reconfigure::Mode::of(&stream).period(),
                        bitrate_kbps,
                    ),
                    None
                );
            }
            assert_eq!(c.current_bps, ceiling);
            assert_eq!(c.video_pacing_bps(), 0);
            assert_eq!(c.snapshot()["reason"], "wire_budget_too_small");
        }
    }
}

#[test]
fn higher_pyrowave_fps_can_raise_the_frame_minimum_without_clean_traffic() {
    let stream = Negotiated {
        codec: 3,
        pyrowave_records: true,
        packet_size: 1392,
        min_fec: 2,
        fps: 60,
        bitrate_kbps: 20_000,
        configured_bitrate_kbps: 30_000,
        ..Default::default()
    };
    let mut c = AdaptiveBitrate::default();
    c.configure(true, &stream, 0, 20);
    let start = Instant::now();
    let period = crate::reconfigure::Mode::of(&stream).period();
    let mut rate = stream.bitrate_kbps;
    for i in 0..32 {
        c.on_block(Block::Unrecoverable);
        rate = c
            .tick(
                start + Duration::from_secs(i),
                Traffic::default(),
                Duration::ZERO,
                0,
                period,
                rate,
            )
            .unwrap_or(rate);
    }
    let faster = Duration::from_secs_f64(1. / 120.);
    assert!(crate::pyrowave::budget(rate, faster, 1392, true, 20) < 16);
    let next = c
        .tick(
            start + Duration::from_secs(33),
            Traffic::default(),
            Duration::ZERO,
            0,
            faster,
            rate,
        )
        .unwrap();
    assert!(next > rate);
    assert!(next <= stream.bitrate_kbps);
    assert!(crate::pyrowave::budget(next, faster, 1392, true, 20) >= 16);
    assert!(c.current_bps <= c.ceiling_bps);
}

#[test]
fn missing_rtt_never_counts_as_stable_latency_for_recovery() {
    let mut c = controller();
    c.on_block(Block::Unrecoverable);
    let start = Instant::now();
    let rate = run(&mut c, start, 2, true, true, 20);
    let budget = c.current_bps;
    for i in 2..60 {
        let now = start + Duration::from_secs(i);
        c.on_ping(now, &[4, 0, 0, 0, 0, 0, 0, 0]);
        c.tick(
            now,
            Traffic {
                frames: i * 60,
                bytes: i * u64::from(rate) * 125,
                send_losses: 0,
            },
            Duration::ZERO,
            20,
            Duration::from_secs_f64(1. / 60.),
            rate,
        );
    }
    assert_eq!(c.current_bps, budget);
    assert_eq!(c.reason, "rtt_unavailable");
}
#[test]
fn controller_pcm_shares_the_budget_and_released_reserves_recover_slowly() {
    let mut c = controller();
    c.on_block(Block::Clean);
    let start = Instant::now();
    let original = run(&mut c, start, 2, true, true, 20);
    let pacing = c.video_pacing_bps();
    let period = Duration::from_secs_f64(1. / 60.);
    c.reserve_controller_pcm(3, 20, period);
    assert_eq!(pacing - c.video_pacing_bps(), 6_000_000);
    let reduced = c
        .tick(
            start + Duration::from_secs(2),
            Traffic::default(),
            Duration::from_millis(8),
            20,
            period,
            original,
        )
        .unwrap();
    assert!(reduced < original);
    assert!(
        c.budget.unwrap().video_bps(reduced, 20, period) + c.budget.unwrap().audio_bps
            <= c.ceiling_bps
    );
    c.reserve_controller_pcm(0, 20, period);
    let held = c
        .tick(
            start + Duration::from_secs(3),
            Traffic::default(),
            Duration::from_millis(8),
            20,
            period,
            reduced,
        )
        .unwrap();
    assert_eq!(held, reduced);
    c.reserve_controller_pcm(usize::MAX, 20, period);
    assert_eq!(c.controller_pcm_bps, 96_000_000);
    assert_eq!(c.video_pacing_bps(), 0);
    c.reserve_controller_pcm(0, 20, period);
    assert!(c.active());
    c.client_takeover();
    c.reserve_controller_pcm(48, 20, period);
    assert_eq!(c.video_pacing_bps(), 0);
    assert_eq!(c.reason, "client_controlled");
    assert!(!c.budget_unusable());
}
