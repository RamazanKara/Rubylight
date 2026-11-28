//! Simulated path tests comparing adaptive pacing against fixed-rate policies.

use super::*;

#[derive(Clone, Copy, Debug)]
enum PathPolicy {
    Fixed800,
    Fixed80,
    Adaptive,
}

#[derive(Debug, Default)]
struct FrameDelivery {
    complete_frames: usize,
    total: Duration,
    maximum: Duration,
}

impl FrameDelivery {
    fn record(&mut self, duration: Duration) {
        self.complete_frames += 1;
        self.total += duration;
        self.maximum = self.maximum.max(duration);
    }

    fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "complete_frames": self.complete_frames,
            "mean_ms": (self.complete_frames > 0)
                .then(|| self.total.as_secs_f64() * 1000. / self.complete_frames as f64),
            "maximum_ms": (self.complete_frames > 0)
                .then_some(self.maximum.as_secs_f64() * 1000.),
        })
    }
}

#[derive(Debug)]
struct PathResult {
    lost: usize,
    conservative_startup_lost: usize,
    first_idr_lost: usize,
    first_idr_complete: bool,
    first_idr_finished: Duration,
    first_probe: Option<Duration>,
    unrecoverable_frames: usize,
    lost_after_settling: usize,
    final_rate: u64,
    largest_fast_frame: Duration,
    encoded_bytes: u64,
    all_delivery: FrameDelivery,
    first_two_seconds_delivery: FrameDelivery,
    steady_after_ten_seconds_delivery: FrameDelivery,
    first_two_seconds_lost: usize,
    first_two_seconds_unrecoverable: usize,
    steady_after_ten_seconds_lost: usize,
    steady_after_ten_seconds_unrecoverable: usize,
    recovered_frames: usize,
    recovered_blocks: usize,
    unrecoverable_blocks: usize,
    rate_increases: usize,
    rate_decreases: usize,
}

fn deliver_feedback(
    pending: &mut std::collections::VecDeque<(Instant, u32, Block)>,
    pacing: &mut AdaptivePacing,
    now: Instant,
    last_sent: u32,
    period: Duration,
) {
    while pending.front().is_some_and(|(due, _, _)| *due <= now) {
        let (_, frame, block) = pending.pop_front().unwrap();
        pacing.on_block(frame, last_sent, period, block);
    }
}

/// A deterministic queue model, not an estimate of any customer's NIC. The
/// encoder always produces the same 40 Mbps stream; only packet spacing can
/// change. Reports travel through an independently delayed feedback queue.
/// No sleeps, sockets, desktop settings or live host are involved.
fn simulate_path(
    egress_bps: u64,
    ipv6: bool,
    feedback_delay: Duration,
    feedback_enabled: bool,
) -> PathResult {
    simulate_policy(
        PathPolicy::Adaptive,
        egress_bps,
        ipv6,
        feedback_delay,
        feedback_enabled,
    )
}

fn simulate_policy(
    policy: PathPolicy,
    egress_bps: u64,
    ipv6: bool,
    feedback_delay: Duration,
    feedback_enabled: bool,
) -> PathResult {
    let period = Duration::from_secs_f64(1. / 60.);
    let packetizer = VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: 20,
        min_fec: 2,
        key: Some([0; 16]),
    };
    let needed = network_pacing::video_wire_rate_bps(&packetizer, 40_000, period, ipv6);
    let datagram = packetizer.packet_size + 16 + 32;
    let wire_packet = datagram + network_pacing::overhead(ipv6);
    let host_time = Duration::from_secs_f64(wire_packet as f64 * 8. / 1_000_000_000.);
    let egress_time = Duration::from_secs_f64(wire_packet as f64 * 8. / egress_bps as f64);
    let capacity = Duration::from_secs_f64(16. * 1024. * 8. / egress_bps as f64);
    let start = Instant::now();
    let mut now = start;
    let mut drained_at = start;
    let mut pacer = network_pacing::Pacer::new(start);
    let base_rate = match policy {
        PathPolicy::Fixed80 => SEED,
        PathPolicy::Fixed800 | PathPolicy::Adaptive => FAST,
    };
    let mut pacing = AdaptivePacing::new(matches!(policy, PathPolicy::Adaptive));
    let mut traffic = Traffic::default();
    let mut pending = std::collections::VecDeque::new();
    let initial_rate = pacing.rate(base_rate, needed, 40_000);
    pacing.tick(start, traffic, None, period, 40_000, feedback_delay);
    let mut result = PathResult {
        lost: 0,
        conservative_startup_lost: 0,
        first_idr_lost: 0,
        first_idr_complete: true,
        first_idr_finished: Duration::ZERO,
        first_probe: None,
        unrecoverable_frames: 0,
        lost_after_settling: 0,
        final_rate: initial_rate,
        largest_fast_frame: Duration::ZERO,
        encoded_bytes: 0,
        all_delivery: FrameDelivery::default(),
        first_two_seconds_delivery: FrameDelivery::default(),
        steady_after_ten_seconds_delivery: FrameDelivery::default(),
        first_two_seconds_lost: 0,
        first_two_seconds_unrecoverable: 0,
        steady_after_ten_seconds_lost: 0,
        steady_after_ten_seconds_unrecoverable: 0,
        recovered_frames: 0,
        recovered_blocks: 0,
        unrecoverable_blocks: 0,
        rate_increases: 0,
        rate_decreases: 0,
    };
    for frame in 1..=960 {
        let available = start + period * (frame - 1);
        let opening = available - start < Duration::from_secs(2);
        let steady = available - start >= Duration::from_secs(10);
        now = now.max(available);
        deliver_feedback(&mut pending, &mut pacing, now, frame - 1, period);
        let frame_start = now;
        let rate = pacing.rate(base_rate, needed, 40_000);
        let per_batch = (network_pacing::batch_payload_limit(rate, datagram, ipv6, 64 * 1024)
            .min(65507)
            / datagram)
            .clamp(1, 64);
        let keyframe = frame == 1 || frame == 300;
        let mut failed_frame = false;
        let mut frame_lost_data = false;
        let payload =
            (40_000. * 125. * period.as_secs_f64()).ceil() as usize * if keyframe { 8 } else { 1 };
        for (data, fec) in packetizer.block_layout(payload) {
            let mut lost_data = 0;
            let mut parity_received = 0;
            let mut packet = 0;
            while packet < data + fec {
                now = now.max(pacer.due());
                let count = (data + fec - packet).min(per_batch);
                for index in packet..packet + count {
                    now += host_time;
                    let done = drained_at.max(now) + egress_time;
                    if done.saturating_duration_since(now) <= capacity {
                        drained_at = done;
                        parity_received += usize::from(index >= data);
                    } else {
                        result.lost += 1;
                        result.conservative_startup_lost +=
                            usize::from(result.first_probe.is_none());
                        result.first_idr_lost += usize::from(frame == 1);
                        result.lost_after_settling += usize::from(frame > 180);
                        result.first_two_seconds_lost += usize::from(opening);
                        result.steady_after_ten_seconds_lost += usize::from(steady);
                        lost_data += usize::from(index < data);
                    }
                }
                traffic.packets += count as u64;
                pacer.sent(now, count * datagram, count, ipv6, rate);
                packet += count;
            }
            let failed = lost_data > parity_received;
            failed_frame |= failed;
            frame_lost_data |= lost_data > 0;
            result.recovered_blocks += usize::from(lost_data > 0 && !failed);
            result.unrecoverable_blocks += usize::from(failed);
            if frame == 1 {
                result.first_idr_complete &= !failed;
            }
            // Like stock clients, report damaged blocks only. The host's
            // normal sent-frame/block validation precedes this controller.
            if feedback_enabled && lost_data > 0 {
                pending.push_back((
                    now + feedback_delay,
                    frame,
                    if failed {
                        Block::Unrecoverable
                    } else {
                        Block::Recovered {
                            lost: lost_data as u16,
                            data: data as u16,
                        }
                    },
                ));
            }
        }
        result.unrecoverable_frames += usize::from(failed_frame);
        result.first_two_seconds_unrecoverable += usize::from(opening && failed_frame);
        result.steady_after_ten_seconds_unrecoverable += usize::from(steady && failed_frame);
        result.recovered_frames += usize::from(frame_lost_data && !failed_frame);
        if !failed_frame {
            // Delivery is measured from the synthetic encoder's scheduled
            // availability to the last accepted datagram leaving the modeled
            // egress. This includes sender backlog and queue delay. It is a
            // conservative completion bound, not decoder/presentation time.
            let delivery = drained_at - available;
            result.all_delivery.record(delivery);
            if opening {
                result.first_two_seconds_delivery.record(delivery);
            }
            if steady {
                result.steady_after_ten_seconds_delivery.record(delivery);
            }
        }
        if frame == 1 {
            result.first_idr_finished = drained_at - start;
        }
        if !keyframe && rate == FAST {
            result.largest_fast_frame = result.largest_fast_frame.max(now - frame_start);
        }
        traffic.frames += 1;
        traffic.bytes += payload as u64;
        result.encoded_bytes += payload as u64;
        deliver_feedback(&mut pending, &mut pacing, now, frame, period);
        if feedback_enabled {
            pacing.on_ping(now, &PING);
        }
        if let Some((previous, next)) =
            pacing.tick(now, traffic, Some(frame), period, 40_000, feedback_delay)
        {
            result.rate_increases += usize::from(next > previous);
            result.rate_decreases += usize::from(next < previous);
            if next > initial_rate && result.first_probe.is_none() {
                result.first_probe = Some(now - start);
            }
        }
        result.final_rate = pacing.rate(base_rate, needed, 40_000);
    }
    result
}

#[test]
fn conservative_startup_preserves_the_first_idr_before_testing_unknown_capacity() {
    for ipv6 in [false, true] {
        for delay_ms in [2, 20, 80] {
            let delay = Duration::from_millis(delay_ms);
            let fast = simulate_path(1_000_000_000, ipv6, delay, true);
            eprintln!("queue model: ipv6={ipv6} feedback_ms={delay_ms} egress_mbps=1000 {fast:?}");
            assert_eq!(fast.lost, 0, "{fast:?}");
            assert_eq!(fast.unrecoverable_frames, 0);
            assert_eq!(
                fast.final_rate, FAST,
                "healthy discovery must finish: {fast:?}"
            );
            assert!(fast.largest_fast_frame > Duration::ZERO);
            assert!(fast.largest_fast_frame < Duration::from_millis(2));
            let slow = simulate_path(100_000_000, ipv6, delay, true);
            eprintln!("queue model: ipv6={ipv6} feedback_ms={delay_ms} egress_mbps=100 {slow:?}");
            assert_eq!(slow.first_idr_lost, 0, "{slow:?}");
            assert!(slow.first_idr_complete, "{slow:?}");
            assert!(
                slow.first_idr_finished < Duration::from_millis(150),
                "{slow:?}"
            );
            assert_eq!(slow.conservative_startup_lost, 0, "{slow:?}");
            assert!(slow.first_probe.unwrap() >= Duration::from_millis(500));
            // Probe loss remains visible, but every frame throughout all
            // sixteen seconds must survive within this fixture's 20% FEC.
            // No startup or probe interval is excluded from that gate. This
            // is not a guarantee for arbitrary queues or other FEC budgets.
            assert_eq!(slow.unrecoverable_frames, 0, "{slow:?}");
            assert!(
                slow.lost > 0,
                "fixture must exercise a failed probe: {slow:?}"
            );
            assert_eq!(slow.lost_after_settling, 0, "{slow:?}");
            assert!(slow.final_rate <= 100_000_000, "{slow:?}");
            assert_eq!(slow.encoded_bytes, fast.encoded_bytes);
        }
    }
}

#[test]
fn matched_queue_model_compares_fast_fixed_suitable_and_adaptive_pacing() {
    for ipv6 in [false, true] {
        for delay_ms in [2, 20, 80] {
            for egress_bps in [100_000_000, 1_000_000_000] {
                let delay = Duration::from_millis(delay_ms);
                let baseline = simulate_policy(PathPolicy::Fixed800, egress_bps, ipv6, delay, true);
                let fixed = simulate_policy(PathPolicy::Fixed80, egress_bps, ipv6, delay, true);
                let adaptive = simulate_policy(PathPolicy::Adaptive, egress_bps, ipv6, delay, true);
                for (policy, result) in [
                    (PathPolicy::Fixed800, &baseline),
                    (PathPolicy::Fixed80, &fixed),
                    (PathPolicy::Adaptive, &adaptive),
                ] {
                    eprintln!(
                        "pacing_model_comparison {}",
                        serde_json::json!({
                            "policy": format!("{policy:?}"),
                            "ipv6": ipv6,
                            "feedback_ms": delay_ms,
                            "egress_bps": egress_bps,
                            "queue_bytes": 16 * 1024,
                            "seconds": 16,
                            "frames": 960,
                            "encoded_bytes": result.encoded_bytes,
                            "first_idr_complete": result.first_idr_complete,
                            "first_idr_delivery_ms": result.first_idr_complete
                                .then_some(result.first_idr_finished.as_secs_f64() * 1000.),
                            "first_idr_last_packet_ms": result.first_idr_finished.as_secs_f64() * 1000.,
                            "first_idr_lost_packets": result.first_idr_lost,
                            "all_complete_frame_delivery": result.all_delivery.summary(),
                            "first_two_seconds_delivery": result.first_two_seconds_delivery.summary(),
                            "steady_after_ten_seconds_delivery": result.steady_after_ten_seconds_delivery.summary(),
                            "lost_packets": result.lost,
                            "unrecoverable_frames": result.unrecoverable_frames,
                            "first_two_seconds_lost_packets": result.first_two_seconds_lost,
                            "first_two_seconds_unrecoverable_frames": result.first_two_seconds_unrecoverable,
                            "steady_after_ten_seconds_lost_packets": result.steady_after_ten_seconds_lost,
                            "steady_after_ten_seconds_unrecoverable_frames": result.steady_after_ten_seconds_unrecoverable,
                            "fec_recovered_frames": result.recovered_frames,
                            "fec_recovered_blocks": result.recovered_blocks,
                            "unrecoverable_blocks": result.unrecoverable_blocks,
                            "rate_increases": result.rate_increases,
                            "rate_decreases": result.rate_decreases,
                            "final_bps": result.final_rate,
                        })
                    );
                }
                assert_eq!(baseline.encoded_bytes, fixed.encoded_bytes);
                assert_eq!(adaptive.encoded_bytes, fixed.encoded_bytes);
                assert_eq!(
                    fixed.lost, 0,
                    "a suitable fixed rate must also pass: {fixed:?}"
                );
                assert_eq!(fixed.unrecoverable_frames, 0);
                assert_eq!(fixed.all_delivery.complete_frames, 960);
                assert_eq!(fixed.first_two_seconds_delivery.complete_frames, 120);
                assert_eq!(fixed.steady_after_ten_seconds_delivery.complete_frames, 360);
                assert_eq!(fixed.rate_increases, 0);
                assert_eq!(fixed.rate_decreases, 0);
                assert_eq!(fixed.final_rate, SEED);
                assert_eq!(adaptive.unrecoverable_frames, 0, "{adaptive:?}");
                assert_eq!(adaptive.all_delivery.complete_frames, 960);
                assert_eq!(adaptive.first_idr_finished, fixed.first_idr_finished);
                if egress_bps == 1_000_000_000 {
                    assert_eq!(baseline.lost, 0);
                    assert_eq!(baseline.all_delivery.complete_frames, 960);
                    assert_eq!(adaptive.lost, 0);
                    assert_eq!(adaptive.final_rate, FAST);
                    assert!(baseline.first_idr_finished < adaptive.first_idr_finished);
                } else {
                    // The fast fixed policy is a negative control for this
                    // finite queue, not a competing result we silently omit.
                    assert!(baseline.lost > 0);
                    assert!(baseline.unrecoverable_frames > 0);
                    assert!(!baseline.first_idr_complete);
                    assert!(adaptive.recovered_frames > 0);
                    assert!(adaptive.rate_decreases > 0);
                }
            }
        }
    }
}

#[test]
fn a_client_without_feedback_keeps_the_conservative_start_and_complete_frames() {
    for ipv6 in [false, true] {
        for egress in [100_000_000, 1_000_000_000] {
            let result = simulate_path(egress, ipv6, Duration::from_millis(20), false);
            assert_eq!(result.final_rate, SEED);
            assert!(result.first_probe.is_none());
            assert_eq!(result.lost, 0, "{result:?}");
            assert_eq!(result.unrecoverable_frames, 0);
            assert!(result.first_idr_complete);
        }
    }
}

#[test]
fn reason_logs_once_per_change_and_switches_when_trigger_changes() {
    let mut stream = Stream::fresh(true);
    let mut steps = Vec::new();
    for _ in 0..60 {
        if let Some(step) = stream.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(2)) {
            steps.push(step);
        }
    }
    assert!(steps.len() >= 3, "{steps:?}");
    assert!(stream.pacing.probe.is_none());
    assert_eq!(steps[0].reason, PacingReason::StartupHeadroom);
    assert!(steps[0].reason_changed);
    for step in &steps[1..] {
        assert_eq!(step.reason, PacingReason::StartupHeadroom);
        assert!(
            !step.reason_changed,
            "same reason must not re-flag info log"
        );
    }

    // Substantial loss switches the reason cleanly.
    let end = stream.last.wrapping_add(30);
    stream.report(end.wrapping_sub(1), end, Block::Unrecoverable);
    stream.report(end, end, Block::Unrecoverable);
    let cut = stream
        .tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(2))
        .expect("loss cut");
    assert_eq!(cut.reason, PacingReason::PacketLoss);
    assert!(cut.reason_changed);

    // Next recovery probe reports RecoveryProbe once.
    let mut recovery = None;
    for _ in 0..20 {
        if let Some(step) = stream.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(2)) {
            recovery = Some(step);
            break;
        }
    }
    let recovery = recovery.expect("recovery probe");
    assert_eq!(recovery.reason, PacingReason::RecoveryProbe);
    assert!(recovery.reason_changed);

    // A loss during that probe restores the stable rate under ProbeLoss.
    let end = stream.last.wrapping_add(1);
    stream.report(end, end, Block::Recovered { lost: 1, data: 10 });
    let reverted = stream
        .tick_step(Some(&PING), 1, 100_000, Duration::from_millis(2))
        .expect("probe revert");
    assert_eq!(reverted.reason, PacingReason::ProbeLoss);
    assert!(reverted.reason_changed);
}

mod wired_handoff;
