//! Tests for wired post-loss adaptive pacing edge cases (block deduplication, seed floor, ceiling lock).

use super::*;

#[test]
fn post_loss_deduplicates_keyframe_blocks_preserves_seed_floor_and_locks_ceiling_on_tick() {
    let now = Instant::now();
    let period = Duration::from_secs_f64(1. / 60.);
    // 1) A non-fast failed rate (or failed_bps/2 below 2x bitrate) never clamps below 2x bitrate.
    let mut slow_route = AdaptivePacing::new_after_loss(Some(0), Some(80_000_000));
    assert_eq!(slow_route.rate(80_000_000, DEMAND, 40_000), SEED);
    assert_eq!(slow_route.snapshot()["ceiling_kbps"], 80_000);
    let mut medium_route = AdaptivePacing::new_after_loss(Some(0), Some(120_000_000));
    assert_eq!(medium_route.rate(120_000_000, DEMAND, 40_000), SEED);
    assert_eq!(medium_route.snapshot()["ceiling_kbps"], 80_000);

    // 2) Multiple failed blocks of one keyframe do not re-trigger single-keyframe cut after an idle tick.
    let mut kf = AdaptivePacing::new_after_loss(Some(0), Some(FAST));
    kf.rate(FAST, DEMAND, 40_000);
    kf.tick(
        now,
        Traffic::default(),
        Some(0),
        period,
        40_000,
        Duration::ZERO,
    );
    kf.on_block_kind(10, 10, period, Block::Unrecoverable, true);
    let t1 = now + Duration::from_millis(500);
    // Idle tick (frames == 0) consumes `keyframe_failed` without cutting or advancing `after_frame`.
    assert_eq!(
        kf.tick(
            t1,
            Traffic::default(),
            Some(10),
            period,
            40_000,
            Duration::from_millis(2)
        ),
        None
    );
    // Second failed block of the same keyframe 10 is deduplicated and cannot trigger a cut.
    kf.on_block_kind(10, 40, period, Block::Unrecoverable, true);
    let t2 = t1 + Duration::from_millis(500);
    let traffic = Traffic {
        frames: 30,
        bytes: 2_500_000,
        packets: 2_000,
        send_losses: 0,
    };
    assert_eq!(
        kf.tick(
            t2,
            traffic,
            Some(40),
            period,
            40_000,
            Duration::from_millis(2)
        ),
        None
    );

    // 3) ProbeLoss updates `snapshot()["ceiling_kbps"]` immediately inside `tick()` before `rate()`.
    let mut lock = AdaptivePacing::new_after_loss(Some(0), Some(FAST));
    lock.rate(FAST, DEMAND, 40_000);
    lock.tick(
        now,
        Traffic::default(),
        Some(0),
        period,
        40_000,
        Duration::ZERO,
    );
    lock.on_ping(t1, &PING);
    assert_eq!(
        lock.tick(
            t1,
            traffic,
            Some(30),
            period,
            40_000,
            Duration::from_millis(2)
        ),
        Some((SEED, 90_000_000))
    );
    lock.on_block_kind(
        31,
        31,
        period,
        Block::Recovered { lost: 1, data: 10 },
        false,
    );
    lock.on_ping(t2, &PING);
    let traffic2 = Traffic {
        frames: 60,
        bytes: 5_000_000,
        packets: 4_000,
        send_losses: 0,
    };
    assert_eq!(
        lock.tick(
            t2,
            traffic2,
            Some(60),
            period,
            40_000,
            Duration::from_millis(2)
        ),
        Some((90_000_000, SEED))
    );
    assert_eq!(lock.snapshot()["ceiling_kbps"], 80_000);
}
