//! Tests for adaptive burst pacing startup seeding, probing, and budget previews.

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

    fn tick_step(
        &mut self,
        ping: Option<&[u8]>,
        frames: u64,
        bytes: u64,
        rtt: Duration,
    ) -> Option<PacingStep> {
        self.now += Duration::from_millis(500);
        self.traffic.frames += frames;
        self.traffic.bytes += bytes;
        self.traffic.packets += 2_000;
        self.last = self.last.wrapping_add(frames as u32);
        if let Some(payload) = ping {
            self.pacing.on_ping(self.now, payload);
        }
        self.pacing.tick_step(
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
fn floor_changes_below_the_current_rate_do_not_drop_loss_cuts_or_reset_samples() {
    let mut stream = Stream::new(true);
    // Prime a baseline sample and then vary `needed` well below FAST (800M) without changing
    // the effective rate. Loss reports arriving meanwhile must still trigger a cut.
    assert_eq!(stream.healthy(), None);
    let end = stream.last + 30;
    stream.report(end - 1, end, Block::Unrecoverable);
    assert_eq!(stream.pacing.rate(FAST, DEMAND + 5_000_000, 40_000), FAST);
    stream.report(end, end, Block::Unrecoverable);
    assert_eq!(stream.pacing.rate(FAST, DEMAND - 5_000_000, 40_000), FAST);
    assert!(!stream.pacing.awaiting_fence);
    assert_eq!(stream.healthy(), Some((FAST, FAST / 2)));
}

#[test]
fn new_after_loss_seeds_at_double_bitrate_fences_pre_handoff_frames_and_probes_up() {
    let now = Instant::now();
    let period = Duration::from_secs_f64(1. / 60.);
    let mut pacing = AdaptivePacing::new_after_loss(Some(100), None);
    assert!(pacing.enabled());
    assert_eq!(pacing.preview_rate(FAST, DEMAND, 40_000), SEED);
    assert_eq!(pacing.rate(FAST, DEMAND, 40_000), SEED);
    assert_eq!(
        pacing.tick(
            now,
            Traffic::default(),
            Some(100),
            period,
            40_000,
            Duration::ZERO,
        ),
        None
    );
    // Reports for frames sent at the prior fast rate (<= 100) stay fenced out.
    for frame in [99, 100] {
        pacing.on_block(frame, 100, period, Block::Unrecoverable);
    }
    assert!(!pacing.any_loss);
    let next = now + Duration::from_millis(500);
    pacing.on_ping(next, &PING);
    let traffic = Traffic {
        frames: 30,
        bytes: 2_500_000,
        packets: 2_000,
        send_losses: 0,
    };
    assert_eq!(
        pacing.tick(
            next,
            traffic,
            Some(130),
            period,
            40_000,
            Duration::from_millis(2)
        ),
        Some((SEED, 90_000_000))
    );
    // Reports after the probe fence (> 130) trigger probe rollback.
    pacing.on_block(131, 160, period, Block::Unrecoverable);
    let probe_end = next + Duration::from_millis(500);
    pacing.on_ping(probe_end, &PING);
    assert_eq!(
        pacing.tick(
            probe_end,
            Traffic {
                frames: 60,
                bytes: 5_000_000,
                packets: 4_000,
                send_losses: 0,
            },
            Some(160),
            period,
            40_000,
            Duration::from_millis(2),
        ),
        Some((90_000_000, SEED))
    );

    // Frame 101 immediately crosses the `Some(100)` fence while 100 does not.
    let mut boundary = AdaptivePacing::new_after_loss(Some(100), None);
    boundary.rate(FAST, DEMAND, 40_000);
    boundary.on_block(100, 101, period, Block::Unrecoverable);
    assert!(!boundary.any_loss);
    boundary.on_block(101, 101, period, Block::Unrecoverable);
    assert!(boundary.any_loss);

    // Without a prior frame (`None`, e.g. remembered loss), frame 1 is accepted.
    let mut remembered = AdaptivePacing::new_after_loss(None, None);
    remembered.rate(FAST, DEMAND, 40_000);
    remembered.on_block(1, 1, period, Block::Unrecoverable);
    assert!(remembered.any_loss);
}

#[test]
fn post_loss_refinements_stay_scoped_from_normal_adaptive_pacing() {
    let period = Duration::from_secs_f64(1. / 60.);

    // 1) `failed_bps / 2` caps ceiling on mid-stream handoff; `None` keeps full route ceiling.
    let mut capped = AdaptivePacing::new_after_loss(Some(10), Some(FAST));
    assert_eq!(capped.preview_rate(FAST, DEMAND, 40_000), SEED);
    capped.rate(FAST, DEMAND, 40_000);
    assert_eq!(capped.snapshot()["ceiling_kbps"], 400_000);

    // 2) ProbeLoss locks `learned_ceiling` in post-loss mode, stopping 60 s sawtooths,
    // while `AdaptivePacing::new(true)` (Wi-Fi / Always) keeps probing normally.
    let now = Instant::now();
    let mut post = AdaptivePacing::new_after_loss(Some(0), Some(FAST));
    post.rate(FAST, DEMAND, 40_000);
    post.tick(
        now,
        Traffic::default(),
        Some(0),
        period,
        40_000,
        Duration::ZERO,
    );
    let t1 = now + Duration::from_millis(500);
    post.on_ping(t1, &PING);
    let traffic1 = Traffic {
        frames: 30,
        bytes: 2_500_000,
        packets: 2_000,
        send_losses: 0,
    };
    assert_eq!(
        post.tick(
            t1,
            traffic1,
            Some(30),
            period,
            40_000,
            Duration::from_millis(2)
        ),
        Some((SEED, 90_000_000))
    );
    post.on_block_kind(
        31,
        31,
        period,
        Block::Recovered { lost: 1, data: 10 },
        false,
    );
    let t2 = t1 + Duration::from_millis(500);
    post.on_ping(t2, &PING);
    assert_eq!(
        post.tick(
            t2,
            Traffic {
                frames: 60,
                bytes: 5_000_000,
                packets: 4_000,
                send_losses: 0,
            },
            Some(60),
            period,
            40_000,
            Duration::from_millis(2),
        ),
        Some((90_000_000, SEED))
    );
    assert_eq!(post.rate(FAST, DEMAND, 40_000), SEED);
    assert_eq!(post.snapshot()["ceiling_kbps"], 80_000);

    // 3) Single unrecoverable keyframe cuts post-loss pacing immediately, while
    // normal AdaptivePacing::new(true) still requires two failed frames.
    let mut normal = Stream::new(true);
    normal
        .pacing
        .on_block_kind(30, 30, period, Block::Unrecoverable, true);
    assert_eq!(normal.healthy(), None);
    let mut post_kf = AdaptivePacing::new_after_loss(Some(0), Some(FAST));
    post_kf.rate(FAST, DEMAND, 40_000);
    post_kf.tick(
        now,
        Traffic::default(),
        Some(0),
        period,
        40_000,
        Duration::ZERO,
    );
    post_kf.on_block_kind(10, 10, period, Block::Unrecoverable, true);
    assert_eq!(
        post_kf.tick(
            t1,
            traffic1,
            Some(30),
            period,
            40_000,
            Duration::from_millis(2)
        ),
        Some((SEED, 62_500_000))
    );

    // 4) Low FEC (< 10%) skips upward probes in post-loss mode without collapsing `limit`,
    // whereas normal AdaptivePacing::new(true) is unaffected.
    let mut low_fec = AdaptivePacing::new_after_loss(Some(0), Some(FAST));
    low_fec.set_fec_percent(5);
    low_fec.rate(FAST, 40_000_000, 40_000);
    low_fec.tick(
        now,
        Traffic::default(),
        Some(0),
        period,
        40_000,
        Duration::ZERO,
    );
    low_fec.on_ping(t1, &PING);
    assert_eq!(
        low_fec.tick(
            t1,
            traffic1,
            Some(30),
            period,
            40_000,
            Duration::from_millis(2)
        ),
        None
    );
    assert_eq!(low_fec.rate(FAST, 40_000_000, 40_000), SEED);

    let mut normal_low = Stream::fresh(true);
    normal_low.pacing.set_fec_percent(5);
    assert_eq!(normal_low.healthy(), Some((SEED, 90_000_000)));
}

mod loss_and_recovery;
mod path_sim;
mod post_loss;
mod wireless;
