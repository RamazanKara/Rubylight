use super::*;
use crate::{network_pacing, packet::VideoPacketizer};

const FAST: u64 = 800_000_000;
const DEMAND: u64 = 50_000_000;
const PING: [u8; 8] = [4, 0, 0, 0, 0, 0, 0, 0];

struct Stream {
    pacing: AdaptivePacing,
    now: Instant,
    traffic: Traffic,
    last: u32,
    period: Duration,
}

impl Stream {
    fn new(enabled: bool) -> Self {
        let now = Instant::now();
        let period = Duration::from_secs_f64(1. / 60.);
        let mut pacing = AdaptivePacing::new(enabled);
        assert_eq!(pacing.rate(FAST, DEMAND), FAST);
        pacing.tick(
            now,
            Traffic::default(),
            Some(0),
            period,
            40_000,
            Duration::ZERO,
        );
        Self {
            pacing,
            now,
            traffic: Traffic::default(),
            last: 0,
            period,
        }
    }

    fn step(
        &mut self,
        ping: Option<&[u8]>,
        frames: u64,
        bytes: u64,
        rtt: Duration,
    ) -> Option<(u64, u64)> {
        self.now += Duration::from_millis(500);
        self.traffic.frames += frames;
        self.traffic.bytes += bytes;
        self.traffic.packets += 2_000;
        self.last = self.last.wrapping_add(frames as u32);
        if let Some(payload) = ping {
            self.pacing.on_ping(self.now, payload);
        }
        self.pacing.tick(
            self.now,
            self.traffic,
            Some(self.last),
            self.period,
            40_000,
            rtt,
        )
    }

    fn healthy(&mut self) -> Option<(u64, u64)> {
        self.step(Some(&PING), 30, 2_500_000, Duration::from_millis(2))
    }

    fn report(&mut self, frame: u32, last_sent: u32, block: Block) {
        self.pacing.on_block(frame, last_sent, self.period, block);
    }

    fn failures(&mut self) -> Option<(u64, u64)> {
        let end = self.last.wrapping_add(30);
        self.report(end.wrapping_sub(1), end, Block::Unrecoverable);
        self.report(end, end, Block::Unrecoverable);
        self.healthy()
    }

    fn first_probe(&mut self) {
        assert_eq!(self.failures(), Some((FAST, FAST / 2)));
        for _ in 0..16 {
            assert_eq!(self.healthy(), None);
        }
        assert_eq!(self.healthy(), Some((400_000_000, 420_000_000)));
    }
}

#[test]
fn healthy_fast_paths_and_clients_without_loss_feedback_keep_the_full_rate() {
    for ping in [None, Some(PING.as_slice())] {
        let mut stream = Stream::new(true);
        for _ in 0..120 {
            assert_eq!(
                stream.step(ping, 30, 2_500_000, Duration::from_millis(2)),
                None
            );
            assert_eq!(stream.pacing.rate(FAST, DEMAND), FAST);
        }
    }
}

#[test]
fn disabled_pacing_preserves_the_callers_rate_despite_reports() {
    let mut stream = Stream::new(false);
    for _ in 0..20 {
        assert_eq!(stream.failures(), None);
        assert_eq!(stream.pacing.rate(950_000_000, DEMAND), 950_000_000);
    }
}

#[test]
fn one_failed_frame_is_not_counted_as_four_when_it_has_four_blocks() {
    let mut stream = Stream::new(true);
    for _ in 0..4 {
        stream.report(30, 30, Block::Unrecoverable);
    }
    assert_eq!(stream.healthy(), None);
    // A delayed block for that same frame in a later control interval cannot
    // make one actual failed frame look like a new failure.
    stream.report(30, 60, Block::Unrecoverable);
    stream.report(60, 60, Block::Unrecoverable);
    assert_eq!(stream.healthy(), None);
    assert_eq!(stream.failures(), Some((FAST, FAST / 2)));
}

#[test]
fn recovered_blocks_do_not_hide_failed_blocks_of_the_same_frames() {
    let mut stream = Stream::new(true);
    for frame in [29, 30] {
        stream.report(frame, 30, Block::Recovered { lost: 1, data: 100 });
        stream.report(frame, 30, Block::Unrecoverable);
    }
    assert_eq!(stream.healthy(), Some((FAST, FAST / 2)));
}

#[test]
fn later_failed_blocks_still_count_after_the_recovered_blocks_were_sampled() {
    let mut stream = Stream::new(true);
    for frame in [29, 30] {
        stream.report(frame, 30, Block::Recovered { lost: 1, data: 100 });
    }
    assert_eq!(stream.healthy(), None);
    // Distinct later blocks establish that both frames actually failed, even
    // though their initial recovered-block losses belonged to the last sample.
    for frame in [29, 30] {
        stream.report(frame, 60, Block::Unrecoverable);
    }
    assert_eq!(stream.healthy(), Some((FAST, FAST / 2)));
}

#[test]
fn distinct_out_of_order_failures_count_once_each() {
    let mut stream = Stream::new(true);
    stream.report(30, 30, Block::Unrecoverable);
    stream.report(29, 30, Block::Unrecoverable);
    stream.report(30, 30, Block::Unrecoverable);
    assert_eq!(stream.healthy(), Some((FAST, FAST / 2)));
}

#[test]
fn future_stale_and_pre_cut_reports_cannot_reduce_the_replacement_rate() {
    let mut stream = Stream::new(true);
    stream.last = 300;
    for frame in [301, 302, 1, 2] {
        stream.report(frame, 300, Block::Unrecoverable);
    }
    assert_eq!(stream.healthy(), None);
    assert_eq!(stream.failures(), Some((FAST, FAST / 2)));
    let fenced = stream.last;
    for frame in [fenced.wrapping_sub(1), fenced] {
        stream.report(frame, fenced, Block::Unrecoverable);
    }
    assert_eq!(stream.healthy(), None);
    assert_eq!(stream.failures(), Some((400_000_000, 200_000_000)));
}

#[test]
fn frame_fences_and_distinct_failures_work_across_u32_wrap() {
    let mut stream = Stream::new(true);
    stream.last = u32::MAX - 30;
    assert_eq!(stream.failures(), Some((FAST, FAST / 2)));
    assert_eq!(stream.last, u32::MAX);
    for frame in [u32::MAX - 1, u32::MAX] {
        stream.report(frame, 29, Block::Unrecoverable);
    }
    stream.report(0, 29, Block::Unrecoverable);
    stream.report(1, 29, Block::Unrecoverable);
    assert_eq!(stream.healthy(), Some((400_000_000, 200_000_000)));
}

#[test]
fn sparse_repaired_packets_do_not_ratchet_the_rate_down() {
    let mut stream = Stream::new(true);
    for _ in 0..60 {
        let end = stream.last + 30;
        for frame in [end - 1, end] {
            stream.report(frame, end, Block::Recovered { lost: 1, data: 100 });
        }
        assert_eq!(stream.healthy(), None);
    }
    assert_eq!(stream.pacing.rate(FAST, DEMAND), FAST);
    let end = stream.last + 30;
    for frame in [end - 1, end] {
        stream.report(
            frame,
            end,
            Block::Recovered {
                lost: 20,
                data: 100,
            },
        );
    }
    assert_eq!(stream.healthy(), Some((FAST, FAST / 2)));
}

#[test]
fn host_send_failures_alone_do_not_guess_a_slower_downstream_link() {
    let mut stream = Stream::new(true);
    for _ in 0..20 {
        stream.traffic.send_losses += 100;
        assert_eq!(stream.healthy(), None);
    }
}

#[test]
fn reductions_follow_live_wire_demand_and_never_override_a_lower_ceiling() {
    let mut stream = Stream::new(true);
    for _ in 0..10 {
        stream.failures();
    }
    assert_eq!(stream.pacing.rate(FAST, DEMAND), 62_500_000);
    // Increased FEC or changed FPS/packet size can increase wire demand while
    // encoder bitrate stays fixed. The next frame must immediately fit it.
    assert_eq!(stream.pacing.rate(FAST, 80_000_000), 100_000_000);
    assert_eq!(stream.pacing.rate(FAST, 40_000_000), 100_000_000);
    assert_eq!(stream.failures(), Some((100_000_000, 50_000_000)));
    assert_eq!(stream.pacing.rate(80_000_000, 120_000_000), 80_000_000);
    assert_eq!(stream.failures(), None);
    assert_eq!(stream.pacing.rate(80_000_000, u64::MAX), 80_000_000);
}

#[test]
fn recovery_requires_fresh_valid_pings_busy_traffic_and_stable_rtt() {
    for condition in 0..6 {
        let mut stream = Stream::new(true);
        assert_eq!(stream.failures(), Some((FAST, FAST / 2)));
        for _ in 0..24 {
            let result = match condition {
                0 => stream.step(None, 30, 2_500_000, Duration::from_millis(2)),
                1 => stream.step(Some(&[3; 8]), 30, 2_500_000, Duration::from_millis(2)),
                2 => stream.step(Some(&PING), 1, 2_500_000, Duration::from_millis(2)),
                3 => stream.step(Some(&PING), 30, 1, Duration::from_millis(2)),
                4 => stream.step(Some(&PING), 30, 2_500_000, Duration::ZERO),
                _ => stream.step(Some(&PING), 30, 2_500_000, Duration::from_millis(50)),
            };
            assert_eq!(result, None, "unhealthy condition {condition}");
        }
        for _ in 0..16 {
            assert_eq!(stream.healthy(), None);
        }
        assert_eq!(stream.healthy(), Some((400_000_000, 420_000_000)));
    }
}

#[test]
fn a_late_control_sample_does_not_count_as_continuous_healthy_streaming() {
    let mut stream = Stream::new(true);
    assert_eq!(stream.failures(), Some((FAST, FAST / 2)));
    for _ in 0..14 {
        assert_eq!(stream.healthy(), None);
    }
    stream.now += Duration::from_secs(3);
    assert_eq!(stream.healthy(), None);
    for _ in 0..16 {
        assert_eq!(stream.healthy(), None);
    }
    assert_eq!(stream.healthy(), Some((400_000_000, 420_000_000)));
}

#[test]
fn a_probe_needs_two_continuous_healthy_seconds_before_another_increase() {
    let mut stream = Stream::new(true);
    stream.first_probe();
    assert_eq!(stream.healthy(), None);
    assert_eq!(stream.healthy(), None);
    // An idle sample breaks confirmation, even though wall-clock time since
    // starting the probe eventually exceeds two seconds.
    assert_eq!(
        stream.step(Some(&PING), 0, 0, Duration::from_millis(2)),
        None
    );
    for _ in 0..3 {
        assert_eq!(stream.healthy(), None);
    }
    assert!(stream.pacing.probe.is_some());
    for _ in 0..2 {
        assert_eq!(stream.healthy(), None);
    }
    assert!(stream.pacing.probe.is_none());
    for _ in 0..11 {
        assert_eq!(stream.healthy(), None);
    }
    assert_eq!(stream.healthy(), Some((420_000_000, 441_000_000)));
}

#[test]
fn even_sparse_new_loss_rolls_back_an_unconfirmed_probe() {
    let mut stream = Stream::new(true);
    stream.first_probe();
    let before_probe = stream.last;
    // Reports from the old pacing rate cannot invalidate the new probe.
    for frame in [before_probe - 1, before_probe] {
        stream.report(frame, before_probe, Block::Unrecoverable);
    }
    assert_eq!(stream.healthy(), None);
    let end = stream.last + 30;
    stream.report(end, end, Block::Recovered { lost: 1, data: 100 });
    assert_eq!(stream.healthy(), Some((420_000_000, 400_000_000)));
    assert_eq!(stream.pacing.snapshot()["recovery_hold_seconds"], 16);
}

#[test]
fn failed_probes_restore_the_working_rate_and_back_off_to_a_one_minute_hold() {
    let mut stream = Stream::new(true);
    stream.first_probe();
    for hold_seconds in [16, 32, 60, 60] {
        assert_eq!(stream.failures(), Some((420_000_000, 400_000_000)));
        assert_eq!(
            stream.pacing.snapshot()["recovery_hold_seconds"],
            hold_seconds
        );
        for _ in 0..hold_seconds * 2 {
            assert_eq!(stream.healthy(), None);
        }
        assert_eq!(stream.healthy(), Some((400_000_000, 420_000_000)));
    }
}

struct PathResult {
    lost: usize,
    lost_after_settling: usize,
    final_rate: u64,
    largest_normal_frame: Duration,
    encoded_bytes: u64,
}

/// A deterministic queue model, not an estimate of any customer's NIC. The
/// encoder always produces the same 40 Mbps stream; only packet spacing can
/// change. No sleeps, sockets, desktop settings or live host are involved.
fn simulate_path(egress_bps: u64, ipv6: bool) -> PathResult {
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
    let mut pacing = AdaptivePacing::new(true);
    let mut traffic = Traffic::default();
    pacing.rate(FAST, needed);
    pacing.tick(
        start,
        traffic,
        None,
        period,
        40_000,
        Duration::from_millis(2),
    );
    let mut result = PathResult {
        lost: 0,
        lost_after_settling: 0,
        final_rate: FAST,
        largest_normal_frame: Duration::ZERO,
        encoded_bytes: 0,
    };
    for frame in 1..=360 {
        now = now.max(start + period * (frame - 1));
        let frame_start = now;
        let rate = pacing.rate(FAST, needed);
        let per_batch = (network_pacing::batch_payload_limit(rate, datagram, ipv6, 64 * 1024)
            .min(65507)
            / datagram)
            .clamp(1, 64);
        let keyframe = frame == 1 || frame == 240;
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
                        result.lost_after_settling += usize::from(frame > 180);
                        lost_data += usize::from(index < data);
                    }
                }
                traffic.packets += count as u64;
                pacer.sent(now, count * datagram, count, ipv6, rate);
                packet += count;
            }
            // Like stock clients, report damaged blocks only. The host's
            // normal sent-frame/block validation precedes this controller.
            if lost_data > 0 {
                pacing.on_block(
                    frame,
                    frame,
                    period,
                    if lost_data > parity_received {
                        Block::Unrecoverable
                    } else {
                        Block::Recovered {
                            lost: lost_data as u16,
                            data: data as u16,
                        }
                    },
                );
            }
        }
        if !keyframe {
            result.largest_normal_frame = result.largest_normal_frame.max(now - frame_start);
        }
        traffic.frames += 1;
        traffic.bytes += payload as u64;
        result.encoded_bytes += payload as u64;
        pacing.on_ping(now, &PING);
        pacing.tick(
            now,
            traffic,
            Some(frame),
            period,
            40_000,
            Duration::from_millis(2),
        );
        result.final_rate = pacing.rate(FAST, needed);
    }
    result
}

#[test]
fn only_a_lossy_slower_egress_reduces_bursts_while_encoder_quality_stays_fixed() {
    for ipv6 in [false, true] {
        let fast = simulate_path(1_000_000_000, ipv6);
        assert_eq!(fast.lost, 0);
        assert_eq!(fast.final_rate, FAST);
        assert!(fast.largest_normal_frame < Duration::from_millis(2));
        let slow = simulate_path(100_000_000, ipv6);
        assert!(slow.lost > 0, "reactive pacing cannot prevent startup loss");
        assert_eq!(slow.lost_after_settling, 0);
        assert!(slow.final_rate <= 100_000_000);
        assert_eq!(slow.encoded_bytes, fast.encoded_bytes);
    }
}
