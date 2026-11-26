//! Unit tests for `CompositorWatch` (Trigger 1 dead-callback probe/lock/give-up, Trigger 2
//! two-stage pulse and keep-on recovery, upstream jitter suppression, and window resets).

use super::*;

const FRAME_60_NS: u64 = 16_666_667;
const FRAME_120_NS: u64 = 8_333_333;
const SEC_NS: u64 = 1_000_000_000;

fn feed_second(
    watch: &mut CompositorWatch,
    start_ns: u64,
    fps: u32,
    rel_gap_fn: impl Fn(u32) -> u64,
    ren_gap_fn: impl Fn(u32) -> Option<u64>,
) {
    let mut rel_t = start_ns;
    let mut ren_t = start_ns;
    for i in 0..fps {
        rel_t += rel_gap_fn(i);
        watch.on_released(rel_t, true);
        if let Some(rg) = ren_gap_fn(i) {
            ren_t += rg;
            watch.on_rendered(rel_t, ren_t, rel_t);
        }
    }
}

#[test]
fn healthy_60_and_120_fps_stay_dormant() {
    for fps in [60u32, 120u32] {
        let gap = if fps == 60 { FRAME_60_NS } else { FRAME_120_NS };
        let mut w = CompositorWatch::new();
        let mut now = 10 * SEC_NS;
        for _ in 0..20 {
            feed_second(&mut w, now, fps, |_| gap, |_| Some(gap));
            now += SEC_NS;
            assert_eq!(w.poll(now, false, false), CompositorAction::None);
        }
        assert!(!w.is_locked_on());
        assert!(w.take_event().is_none());
    }
}

#[test]
fn trigger1_dead_callbacks_probe_and_lock_when_recovered() {
    let mut w = CompositorWatch::new();
    let mut now = 100 * SEC_NS;
    assert_eq!(w.poll(now, false, false), CompositorAction::None);

    // First 2 poll ticks after start (< 3 s warmup) stay dormant even with zero callbacks.
    for _ in 0..2 {
        feed_second(&mut w, now, 30, |_| 33_333_333, |_| None);
        now += SEC_NS;
        assert_eq!(w.poll(now, false, false), CompositorAction::None);
    }

    // Next 3 s with dead callbacks trigger KeepOn probe.
    for i in 0..3 {
        feed_second(&mut w, now, 30, |_| 33_333_333, |_| None);
        now += SEC_NS;
        let expected = if i == 2 {
            CompositorAction::KeepOn
        } else {
            CompositorAction::None
        };
        assert_eq!(w.poll(now, false, false), expected);
    }
    assert!(w.take_event().is_some());

    // Second later, keepalive restores callbacks (>90%) -> LockedOn!
    feed_second(&mut w, now, 60, |_| FRAME_60_NS, |_| Some(FRAME_60_NS));
    now += SEC_NS;
    assert_eq!(w.poll(now, false, false), CompositorAction::None);
    assert!(w.is_locked_on());
    assert!(w.take_event().is_some());
}

#[test]
fn trigger1_gives_up_if_hal_never_delivers_callbacks() {
    let mut w = CompositorWatch::new();
    let mut now = SEC_NS;
    w.poll(now, false, false);
    for _ in 0..2 {
        feed_second(&mut w, now, 60, |_| FRAME_60_NS, |_| None);
        now += SEC_NS;
        w.poll(now, false, false);
    }
    // Trigger probe
    for i in 0..3 {
        feed_second(&mut w, now, 60, |_| FRAME_60_NS, |_| None);
        now += SEC_NS;
        let act = w.poll(now, false, false);
        if i == 2 {
            assert_eq!(act, CompositorAction::KeepOn);
        }
    }
    // 5 s probe expires -> Stop, never retries Trigger 1
    let mut stopped = false;
    for _ in 0..5 {
        feed_second(&mut w, now, 60, |_| FRAME_60_NS, |_| None);
        now += SEC_NS;
        if w.poll(now, false, false) == CompositorAction::Stop {
            stopped = true;
        }
    }
    assert!(stopped);
    assert!(!w.is_locked_on());
    for _ in 0..10 {
        feed_second(&mut w, now, 60, |_| FRAME_60_NS, |_| None);
        now += SEC_NS;
        assert_eq!(w.poll(now, false, false), CompositorAction::None);
    }
}

#[test]
fn trigger2_stage1_pulse_clears_and_resets_cooldown() {
    let mut w = CompositorWatch::new();
    let mut now = SEC_NS;
    w.poll(now, false, false);
    for _ in 0..3 {
        feed_second(&mut w, now, 60, |_| FRAME_60_NS, |_| Some(FRAME_60_NS));
        now += SEC_NS;
        w.poll(now, false, false);
    }
    // 4 s of clean upstream + bunched post-handoff render gaps (0ms / 33ms) -> Pulse
    for i in 0..4 {
        feed_second(
            &mut w,
            now,
            60,
            |_| FRAME_60_NS,
            |f| Some(if f % 2 == 0 { 100_000 } else { FRAME_60_NS * 2 }),
        );
        now += SEC_NS;
        let expected = if i == 3 {
            CompositorAction::Pulse
        } else {
            CompositorAction::None
        };
        assert_eq!(w.poll(now, false, false), expected);
    }
    // Settle 2 s + 3 s smooth -> returns to Watching without KeepOn lock
    for _ in 0..6 {
        feed_second(&mut w, now, 60, |_| FRAME_60_NS, |_| Some(FRAME_60_NS));
        now += SEC_NS;
        assert_eq!(w.poll(now, false, false), CompositorAction::None);
    }
    assert!(!w.is_locked_on());
}

#[test]
fn trigger2_persistent_judder_escalates_to_stage2_and_locks() {
    let mut w = CompositorWatch::new();
    let mut now = SEC_NS;
    w.poll(now, false, false);
    for _ in 0..3 {
        feed_second(&mut w, now, 60, |_| FRAME_60_NS, |_| Some(FRAME_60_NS));
        now += SEC_NS;
        w.poll(now, false, false);
    }
    // Judder -> Stage 1 Pulse
    for i in 0..4 {
        feed_second(
            &mut w,
            now,
            60,
            |_| FRAME_60_NS,
            |f| Some(if f % 2 == 0 { 100_000 } else { FRAME_60_NS * 2 }),
        );
        now += SEC_NS;
        let act = w.poll(now, false, false);
        if i == 3 {
            assert_eq!(act, CompositorAction::Pulse);
        }
    }
    // 2 s settle + 3 s still juddering -> KeepOn
    let mut escalated = false;
    for _ in 0..6 {
        feed_second(
            &mut w,
            now,
            60,
            |_| FRAME_60_NS,
            |f| Some(if f % 2 == 0 { 100_000 } else { FRAME_60_NS * 2 }),
        );
        now += SEC_NS;
        if w.poll(now, false, false) == CompositorAction::KeepOn {
            escalated = true;
        }
    }
    assert!(escalated);

    // Keepalive stabilizes render gaps for 3 s -> LockedOn
    for _ in 0..3 {
        feed_second(&mut w, now, 60, |_| FRAME_60_NS, |_| Some(FRAME_60_NS));
        now += SEC_NS;
        assert_eq!(w.poll(now, false, false), CompositorAction::None);
    }
    assert!(w.is_locked_on());
}

#[test]
fn trigger2_repeated_relapses_skip_stage1_pulse() {
    let mut w = CompositorWatch::new();
    let mut now = SEC_NS;
    w.poll(now, false, false);
    for _ in 0..3 {
        feed_second(&mut w, now, 60, |_| FRAME_60_NS, |_| Some(FRAME_60_NS));
        now += SEC_NS;
        w.poll(now, false, false);
    }

    // Two pulses within 15 min that briefly recover
    for _ in 0..2 {
        for i in 0..4 {
            feed_second(
                &mut w,
                now,
                60,
                |_| FRAME_60_NS,
                |f| Some(if f % 2 == 0 { 100_000 } else { FRAME_60_NS * 2 }),
            );
            now += SEC_NS;
            let act = w.poll(now, false, false);
            if i == 3 {
                assert_eq!(act, CompositorAction::Pulse);
            }
        }
        for _ in 0..6 {
            feed_second(&mut w, now, 60, |_| FRAME_60_NS, |_| Some(FRAME_60_NS));
            now += SEC_NS;
            w.poll(now, false, false);
        }
        now += 31 * SEC_NS;
        w.poll(now, false, false);
    }

    // Third relapse within 15 min goes directly to KeepOn!
    let mut direct_keep_on = false;
    for _ in 0..4 {
        feed_second(
            &mut w,
            now,
            60,
            |_| FRAME_60_NS,
            |f| Some(if f % 2 == 0 { 100_000 } else { FRAME_60_NS * 2 }),
        );
        now += SEC_NS;
        if w.poll(now, false, false) == CompositorAction::KeepOn {
            direct_keep_on = true;
        }
    }
    assert!(direct_keep_on);
}

#[test]
fn upstream_jitter_and_overlay_suppress_triggers() {
    let mut w = CompositorWatch::new();
    let mut now = SEC_NS;
    w.poll(now, false, false);
    for _ in 0..3 {
        feed_second(&mut w, now, 60, |_| FRAME_60_NS, |_| Some(FRAME_60_NS));
        now += SEC_NS;
        w.poll(now, false, false);
    }

    // Unsteady upstream (50% 8ms/25ms release gaps AND matching render gaps) never triggers Trigger 2.
    for _ in 0..10 {
        feed_second(
            &mut w,
            now,
            60,
            |f| if f % 2 == 0 { FRAME_120_NS } else { 25_000_000 },
            |f| Some(if f % 2 == 0 { FRAME_120_NS } else { 25_000_000 }),
        );
        now += SEC_NS;
        assert_eq!(w.poll(now, false, false), CompositorAction::None);
    }

    // Dead callbacks while overlay visible stay ignored.
    for _ in 0..6 {
        feed_second(&mut w, now, 30, |_| 33_333_333, |_| None);
        now += SEC_NS;
        assert_eq!(w.poll(now, true, false), CompositorAction::None);
    }
}

#[test]
fn reset_window_preserves_locked_on() {
    let mut w = CompositorWatch::new();
    let mut now = SEC_NS;
    w.poll(now, false, false);
    for _ in 0..3 {
        feed_second(&mut w, now, 30, |_| 33_333_333, |_| None);
        now += SEC_NS;
        w.poll(now, false, false);
    }
    for _ in 0..3 {
        feed_second(&mut w, now, 30, |_| 33_333_333, |_| None);
        now += SEC_NS;
        w.poll(now, false, false);
    }
    feed_second(&mut w, now, 60, |_| FRAME_60_NS, |_| Some(FRAME_60_NS));
    now += SEC_NS;
    w.poll(now, false, false);
    assert!(w.is_locked_on());

    w.reset_window();
    assert!(w.is_locked_on());
    assert_eq!(w.poll(now + SEC_NS, true, false), CompositorAction::None);
    assert!(w.is_locked_on());
}

#[test]
fn stage1_watch_requires_three_dead_buckets_before_trigger1() {
    let mut w = CompositorWatch::new();
    let mut now = SEC_NS;
    w.poll(now, false, false);
    for _ in 0..3 {
        feed_second(&mut w, now, 60, |_| FRAME_60_NS, |_| Some(FRAME_60_NS));
        now += SEC_NS;
        w.poll(now, false, false);
    }
    for i in 0..4 {
        feed_second(
            &mut w,
            now,
            60,
            |_| FRAME_60_NS,
            |f| Some(if f % 2 == 0 { 100_000 } else { FRAME_60_NS * 2 }),
        );
        now += SEC_NS;
        let act = w.poll(now, false, false);
        if i == 3 {
            assert_eq!(act, CompositorAction::Pulse);
        }
    }
    // Settle 2 s
    for _ in 0..2 {
        feed_second(&mut w, now, 60, |_| FRAME_60_NS, |_| Some(FRAME_60_NS));
        now += SEC_NS;
        assert_eq!(w.poll(now, false, false), CompositorAction::None);
    }
    // Single dead-callback batching stall bucket during Stage1Watching must NOT trigger ProbingTrigger1.
    feed_second(&mut w, now, 60, |_| FRAME_60_NS, |_| None);
    now += SEC_NS;
    assert_eq!(w.poll(now, false, false), CompositorAction::None);
    feed_second(&mut w, now, 60, |_| FRAME_60_NS, |_| Some(FRAME_60_NS));
    now += SEC_NS;
    assert_eq!(w.poll(now, false, false), CompositorAction::None);
    assert!(!w.is_locked_on());
}

#[test]
fn bogus_render_timestamp_does_not_poison_watch() {
    let mut w = CompositorWatch::new();
    let mut now = SEC_NS;
    w.poll(now, false, false);
    for _ in 0..3 {
        feed_second(&mut w, now, 60, |_| FRAME_60_NS, |_| Some(FRAME_60_NS));
        now += SEC_NS;
        w.poll(now, false, false);
    }
    // Inject sentinel INT64_MAX and wild far-future timestamp
    w.on_rendered(now, i64::MAX as u64, now);
    w.on_rendered(now, now + 60 * SEC_NS, now);

    // Trigger 2 still detects clean judder afterward without prev_render_ns getting wedged.
    let mut pulsed = false;
    for _ in 0..4 {
        feed_second(
            &mut w,
            now,
            60,
            |_| FRAME_60_NS,
            |f| Some(if f % 2 == 0 { 100_000 } else { FRAME_60_NS * 2 }),
        );
        now += SEC_NS;
        if w.poll(now, false, false) == CompositorAction::Pulse {
            pulsed = true;
        }
    }
    assert!(pulsed);
}

#[test]
fn reset_window_during_probe_emits_stop_on_next_poll() {
    let mut w = CompositorWatch::new();
    let mut now = SEC_NS;
    w.poll(now, false, false);
    for _ in 0..3 {
        feed_second(&mut w, now, 60, |_| FRAME_60_NS, |_| Some(FRAME_60_NS));
        now += SEC_NS;
        assert_eq!(w.poll(now, false, false), CompositorAction::None);
    }
    for i in 0..3 {
        feed_second(&mut w, now, 60, |_| FRAME_60_NS, |_| None);
        now += SEC_NS;
        let act = w.poll(now, false, false);
        assert_eq!(
            act,
            if i == 2 {
                CompositorAction::KeepOn
            } else {
                CompositorAction::None
            }
        );
    }
    w.reset_window();
    now += SEC_NS;
    assert_eq!(w.poll(now, false, false), CompositorAction::Stop);
    now += SEC_NS;
    assert_eq!(w.poll(now, false, false), CompositorAction::None);
}

#[test]
fn sentinel_render_timestamps_do_not_count_as_delivered_callbacks() {
    let mut w = CompositorWatch::new();
    let mut now = SEC_NS;
    w.poll(now, false, false);
    for _ in 0..3 {
        feed_second(&mut w, now, 60, |_| FRAME_60_NS, |_| Some(FRAME_60_NS));
        now += SEC_NS;
        assert_eq!(w.poll(now, false, false), CompositorAction::None);
    }
    for i in 0..3 {
        for f in 0..60 {
            let ts = now + f * FRAME_60_NS;
            w.on_released(ts, true);
            w.on_rendered(ts, i64::MAX as u64, ts);
        }
        now += SEC_NS;
        assert_eq!(
            w.poll(now, false, false),
            if i == 2 {
                CompositorAction::KeepOn
            } else {
                CompositorAction::None
            }
        );
    }
    for f in 0..60 {
        let ts = now + f * FRAME_60_NS;
        w.on_released(ts, true);
        w.on_rendered(ts, i64::MAX as u64, ts);
    }
    assert_eq!(w.poll(now + SEC_NS, false, false), CompositorAction::None);
    assert!(!w.is_locked_on());
}
