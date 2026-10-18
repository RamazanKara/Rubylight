use super::*;
use crate::{network_pacing, packet::VideoPacketizer};

const FAST: u64 = 800_000_000;
const DEMAND: u64 = 50_000_000;
const SEED: u64 = 80_000_000;
const PING: [u8; 8] = [4, 0, 0, 0, 0, 0, 0, 0];

struct Stream {
    pacing: AdaptivePacing,
    now: Instant,
    traffic: Traffic,
    last: u32,
    period: Duration,
}

impl Stream {
    fn fresh(enabled: bool) -> Self {
        let now = Instant::now();
        let period = Duration::from_secs_f64(1. / 60.);
        let mut pacing = AdaptivePacing::new(enabled);
        assert_eq!(
            pacing.rate(FAST, DEMAND, 40_000),
            if enabled { SEED } else { FAST }
        );
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

    /// Existing steady-state regressions first exercise a complete healthy
    /// startup. Wrap the synthetic wire sequence back to zero so their small
    /// explicit frame numbers remain newer than the final startup fence.
    fn new(enabled: bool) -> Self {
        let mut stream = Self::fresh(enabled);
        stream.last = 0u32.wrapping_sub(120 * 30);
        for _ in 0..120 {
            stream.healthy();
        }
        assert_eq!(stream.last, 0);
        assert_eq!(stream.pacing.rate(FAST, DEMAND, 40_000), FAST);
        assert!(stream.pacing.probe.is_none());
        stream
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
fn preview_matches_the_next_sender_rate_across_budget_changes() {
    for state in 0..7 {
        for ceiling in [0, 1, 30_000_000, FAST, 950_000_000, u64::MAX] {
            for needed in [0, DEMAND, 90_000_000, u64::MAX] {
                for encoder_kbps in [0, 40_000, 80_000, u32::MAX] {
                    let mut pacing = AdaptivePacing::new(state != 0);
                    if state >= 2 {
                        pacing.rate(FAST, DEMAND, 40_000);
                    }
                    if state == 3 {
                        // Fully validated fast route.
                        pacing.limit = None;
                    } else if state >= 4 {
                        pacing.limit = Some(420_000_000);
                        if state >= 5 {
                            pacing.probe = Some(Probe {
                                stable: 400_000_000,
                                started: Instant::now(),
                                settle: Duration::from_secs(2),
                            });
                            pacing.any_loss = state == 6;
                        }
                    }
                    let preview = pacing.preview_rate(ceiling, needed, encoder_kbps);
                    assert_eq!(
                        preview,
                        pacing.rate(ceiling, needed, encoder_kbps),
                        "state={state}, ceiling={ceiling}, demand={needed}, encoder={encoder_kbps}"
                    );
                }
            }
        }
    }
}

#[test]
fn recovery_budget_previews_preserve_probe_progress_and_loss_feedback() {
    let mut stream = Stream::fresh(true);
    for _ in 0..40 {
        // The encoder asks about the full recovery FEC on every picture;
        // the sender retains the ordinary frame's smaller demand.
        for _ in 0..30 {
            stream.pacing.preview_rate(FAST, 70_000_000, 40_000);
            stream.pacing.rate(FAST, DEMAND, 40_000);
        }
        stream.healthy();
    }
    assert_eq!(stream.pacing.current(), FAST);
    assert!(stream.pacing.probe.is_none());

    stream.first_probe();
    let before = stream.pacing.snapshot();
    let sample_at = stream.pacing.last_sample.unwrap().0;
    let clean_since = stream.pacing.clean_since;
    for _ in 0..100 {
        assert_eq!(
            stream.pacing.preview_rate(FAST, 70_000_000, 40_000),
            400_000_000
        );
        stream.pacing.preview_rate(FAST * 2, DEMAND, 40_000);
    }
    assert_eq!(stream.pacing.snapshot(), before);
    assert_eq!(stream.pacing.last_sample.unwrap().0, sample_at);
    assert_eq!(stream.pacing.clean_since, clean_since);
    assert!(!stream.pacing.awaiting_fence);
    // Feedback still rejects the unconfirmed probe immediately, instead of
    // being discarded as evidence from a changed demand or a previous rate.
    let frame = stream.last.wrapping_add(1);
    stream.report(frame, frame, Block::Unrecoverable);
    assert_eq!(stream.healthy(), Some((420_000_000, 400_000_000)));
}

#[test]
fn fresh_streams_seed_from_encoder_and_wire_demand_before_the_first_packet() {
    for (ceiling, demand, encoder, expected) in [
        (FAST, DEMAND, 40_000, SEED),
        (FAST, 90_000_000, 40_000, 112_500_000),
        (FAST, 10_000_000, 80_000, 160_000_000),
        (30_000_000, DEMAND, 40_000, 30_000_000),
        (FAST, 0, 0, 1_000_000),
        (FAST, u64::MAX, u32::MAX, FAST),
    ] {
        let mut pacing = AdaptivePacing::new(true);
        assert_eq!(pacing.rate(ceiling, demand, encoder), expected);
        // Merely consulting the budget does not advance startup.
        assert_eq!(pacing.rate(ceiling, demand, encoder), expected);
    }
}

#[test]
fn fresh_healthy_streams_probe_gradually_and_finish_at_the_fast_ceiling() {
    let mut stream = Stream::fresh(true);
    let start = stream.now;
    let mut previous = SEED;
    let mut last_change = start;
    let mut increases = 0;
    for _ in 0..40 {
        if let Some((old, next)) = stream.healthy() {
            assert_eq!(old, previous);
            assert!(next > old && next <= old + old / 8);
            assert!(stream.now - last_change >= Duration::from_millis(500));
            previous = next;
            last_change = stream.now;
            increases += 1;
        }
        assert_eq!(stream.pacing.rate(FAST, DEMAND, 40_000), previous);
    }
    assert_eq!(previous, FAST);
    assert!(
        increases >= 20,
        "startup must not jump directly to the ceiling"
    );
    assert!(!stream.pacing.snapshot()["startup"].as_bool().unwrap());
    assert!(!stream.pacing.snapshot()["probing"].as_bool().unwrap());
}

#[test]
fn startup_without_valid_recent_feedback_or_busy_video_never_jumps_to_fast_rate() {
    for condition in 0..6 {
        let mut stream = Stream::fresh(true);
        for _ in 0..120 {
            let change = match condition {
                0 => stream.step(None, 30, 2_500_000, Duration::from_millis(2)),
                1 => stream.step(Some(&[3; 8]), 30, 2_500_000, Duration::from_millis(2)),
                2 => stream.step(Some(&PING), 30, 2_500_000, Duration::ZERO),
                3 => stream.step(Some(&PING), 0, 2_500_000, Duration::from_millis(2)),
                4 => stream.step(Some(&PING), 30, 1, Duration::from_millis(2)),
                _ => {
                    stream.now += Duration::from_secs(3);
                    stream.healthy()
                }
            };
            assert_eq!(change, None, "startup condition {condition}");
            assert_eq!(stream.pacing.rate(FAST, DEMAND, 40_000), SEED);
        }
    }
}

#[test]
fn startup_guard_allows_round_trip_feedback_before_the_next_probe() {
    let mut stream = Stream::fresh(true);
    let start = stream.now;
    let mut previous_change = start;
    let mut changes = 0;
    for _ in 0..12 {
        if stream
            .step(Some(&PING), 30, 2_500_000, Duration::from_millis(400))
            .is_some()
        {
            assert!(stream.now - previous_change >= Duration::from_millis(900));
            previous_change = stream.now;
            changes += 1;
        }
    }
    assert!(
        changes > 0,
        "a stable higher RTT must not permanently block discovery"
    );
}

#[test]
fn a_startup_probe_waits_for_increased_rtt_before_confirming_or_advancing() {
    let mut stream = Stream::fresh(true);
    assert_eq!(
        stream.step(Some(&PING), 30, 2_500_000, Duration::from_millis(400)),
        None
    );
    assert_eq!(
        stream.step(Some(&PING), 30, 2_500_000, Duration::from_millis(400)),
        Some((SEED, 90_000_000))
    );
    let probe_started = stream.now;
    // 600 ms is still within the stable-RTT tolerance above the 400 ms
    // baseline, but extends the feedback guard from 900 ms to 1300 ms.
    for _ in 0..2 {
        assert_eq!(
            stream.step(Some(&PING), 30, 2_500_000, Duration::from_millis(600)),
            None
        );
        let probe = stream.pacing.probe.as_ref().unwrap();
        assert_eq!(probe.started, probe_started);
        assert_eq!(probe.stable, SEED);
        assert_eq!(stream.pacing.rate(FAST, DEMAND, 40_000), 90_000_000);
    }
    assert_eq!(stream.now - probe_started, Duration::from_secs(1));
    assert_eq!(
        stream.step(Some(&PING), 30, 2_500_000, Duration::from_millis(600)),
        Some((90_000_000, 101_250_000))
    );
    assert!(stream.now - probe_started >= Duration::from_millis(1300));
}

#[test]
fn the_first_startup_probe_rolls_back_on_one_new_loss_and_ignores_old_reports() {
    let mut stream = Stream::fresh(true);
    let mut probe = None;
    for _ in 0..4 {
        if let Some(change) = stream.healthy() {
            probe = Some(change);
            break;
        }
    }
    assert_eq!(probe, Some((SEED, 90_000_000)));
    let fence = stream.last;
    stream.report(fence, fence, Block::Unrecoverable);
    assert!(
        !stream.pacing.any_loss,
        "old-rate feedback must not poison the probe"
    );
    stream.report(
        fence + 1,
        fence + 1,
        Block::Recovered { lost: 1, data: 100 },
    );
    stream.now += Duration::from_millis(1);
    stream.last = fence + 1;
    assert_eq!(
        stream.pacing.tick(
            stream.now,
            stream.traffic,
            Some(stream.last),
            stream.period,
            40_000,
            Duration::from_millis(2),
        ),
        Some((90_000_000, SEED)),
        "a valid failed probe need not await another full frame or 500 ms sample"
    );
    assert_eq!(stream.pacing.rate(FAST, DEMAND, 40_000), SEED);
    assert_eq!(stream.pacing.snapshot()["startup"], false);
    assert_eq!(stream.pacing.snapshot()["probing"], false);
    for _ in 0..8 {
        assert_eq!(
            stream.healthy(),
            None,
            "recovery must not immediately repeat startup probes"
        );
    }
}

#[test]
fn a_fec_budget_change_cannot_erase_loss_already_received_during_a_startup_probe() {
    for (needed, expected) in [(60_000_000, SEED), (72_000_000, 90_000_000)] {
        let mut stream = Stream::fresh(true);
        assert_eq!(stream.healthy(), Some((SEED, 90_000_000)));
        let frame = stream.last + 1;
        stream.report(frame, frame, Block::Recovered { lost: 1, data: 100 });
        // Adaptive FEC can update the next frame's wire budget between the
        // validated report and the control worker's pacing sample. Even when
        // the new floor forces the rolled-back rate to equal the probe rate,
        // the failure must end startup and retain the longer recovery hold.
        assert_eq!(stream.pacing.rate(FAST, needed, 40_000), expected);
        assert_eq!(stream.pacing.snapshot()["startup"], false);
        assert_eq!(stream.pacing.snapshot()["probing"], false);
        assert_eq!(stream.pacing.snapshot()["recovery_hold_seconds"], 16);
        assert_eq!(stream.healthy(), None); // Install the new sent-frame fence.
        for _ in 0..20 {
            assert_eq!(stream.healthy(), None);
            assert_eq!(stream.pacing.rate(FAST, needed, 40_000), expected);
        }
    }
}

#[test]
fn startup_refreshes_live_fec_floor_and_honors_a_lower_route_ceiling() {
    let mut stream = Stream::fresh(true);
    assert_eq!(stream.pacing.rate(FAST, 100_000_000, 40_000), 125_000_000);
    assert_eq!(
        stream.pacing.rate(90_000_000, 100_000_000, 40_000),
        90_000_000
    );
    assert_eq!(stream.pacing.rate(FAST, DEMAND, 40_000), 90_000_000);
    // A quieter FEC budget or a faster local route is not proof that the
    // remote path can accept an immediate return to the full ceiling.
    assert_eq!(stream.pacing.rate(FAST, 20_000_000, 40_000), 90_000_000);
}

#[test]
fn a_confirmed_slow_route_cannot_jump_to_a_new_fast_ceiling_without_new_activity() {
    let mut stream = Stream::fresh(true);
    assert_eq!(stream.pacing.rate(SEED, DEMAND, 40_000), SEED);
    for _ in 0..8 {
        assert_eq!(stream.healthy(), None);
    }
    assert_eq!(stream.pacing.snapshot()["startup"], false);
    assert_eq!(stream.pacing.snapshot()["limited"], false);
    assert!(
        stream.pacing.limit.is_none(),
        "the old ceiling is confirmed"
    );

    assert_eq!(stream.pacing.rate(FAST, DEMAND, 40_000), SEED);
    // Pings preceding the route change must not authorize a probe on the
    // replacement route, even while they remain younger than two seconds.
    for _ in 0..24 {
        assert_eq!(
            stream.step(None, 30, 2_500_000, Duration::from_millis(2)),
            None
        );
        assert_eq!(stream.pacing.rate(FAST, DEMAND, 40_000), SEED);
    }
    let mut previous = SEED;
    let mut last_change = stream.now;
    for _ in 0..40 {
        if let Some((old, next)) = stream.healthy() {
            assert_eq!(old, previous);
            assert!(next > old && next <= old + old / 8);
            assert!(stream.now - last_change >= Duration::from_millis(500));
            previous = next;
            last_change = stream.now;
        }
    }
    assert_eq!(
        previous, FAST,
        "new healthy activity should permit guarded discovery"
    );
    // Physical route shrinkage still takes effect before the next packet.
    assert_eq!(stream.pacing.rate(40_000_000, DEMAND, 40_000), 40_000_000);
}

#[test]
fn established_fast_paths_keep_the_full_rate_without_loss_reports() {
    for ping in [None, Some(PING.as_slice())] {
        let mut stream = Stream::new(true);
        for _ in 0..120 {
            assert_eq!(
                stream.step(ping, 30, 2_500_000, Duration::from_millis(2)),
                None
            );
            assert_eq!(stream.pacing.rate(FAST, DEMAND, 40_000), FAST);
        }
    }
}

#[test]
fn disabled_pacing_preserves_the_callers_rate_despite_reports() {
    let mut stream = Stream::new(false);
    for _ in 0..20 {
        assert_eq!(stream.failures(), None);
        assert_eq!(stream.pacing.rate(950_000_000, DEMAND, 40_000), 950_000_000);
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
    assert_eq!(stream.pacing.rate(FAST, DEMAND, 40_000), FAST);
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
    assert_eq!(stream.pacing.rate(FAST, DEMAND, 40_000), 62_500_000);
    // Increased FEC or changed FPS/packet size can increase wire demand while
    // encoder bitrate stays fixed. The next frame must immediately fit it.
    assert_eq!(stream.pacing.rate(FAST, 80_000_000, 40_000), 100_000_000);
    assert_eq!(stream.pacing.rate(FAST, 40_000_000, 40_000), 100_000_000);
    // A changed wire budget fences the old report epoch before accepting
    // fresh losses at the replacement rate.
    assert_eq!(stream.failures(), None);
    assert_eq!(stream.failures(), Some((100_000_000, 50_000_000)));
    assert_eq!(
        stream.pacing.rate(80_000_000, 120_000_000, 40_000),
        80_000_000
    );
    assert_eq!(stream.failures(), None);
    assert_eq!(stream.pacing.rate(80_000_000, u64::MAX, 40_000), 80_000_000);
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
