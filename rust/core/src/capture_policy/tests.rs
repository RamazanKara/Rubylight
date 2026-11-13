use super::*;
#[test]
fn compositions_tell_a_focus_change_only_by_dirty_rectangles_inside_it() {
    // A game window at (100, 50)-(1380, 770) on the second display of a
    // desktop whose first display is 2560 wide.
    let display = [2560, 0, 2560 + 1920, 1080];
    let game = focus_on_display([2660, 50, 3940, 770], display);
    assert_eq!(game, Some([100, 50, 1380, 770]));
    // A window half off the display is clipped to it; one on another
    // display is no focus window here.
    assert_eq!(
        focus_on_display([4000, 900, 5000, 1300], display),
        Some([1440, 900, 1920, 1080])
    );
    assert_eq!(focus_on_display([0, 0, 2560, 1440], display), None);
    // Another window animating beside the game.
    assert_eq!(focus_changed(&[[1500, 0, 1900, 400]], game), Some(false));
    // A rectangle touching the game's edge does not overlap it.
    assert_eq!(focus_changed(&[[1380, 0, 1900, 400]], game), Some(false));
    // Any rectangle inside the game changes it.
    assert_eq!(
        focus_changed(&[[1500, 0, 1900, 400], [600, 300, 700, 400]], game),
        Some(true)
    );
    // No dirty rectangles (a forced repaint, or Windows before 11 24H2)
    // or no focus window on the display cannot tell.
    assert_eq!(focus_changed(&[], game), None);
    assert_eq!(focus_changed(&[[1500, 0, 1900, 400]], None), None);
    // A dropped composition's focus change survives into the published one.
    assert_eq!(merge_focus(Some(true), Some(false)), Some(true));
    assert_eq!(merge_focus(Some(false), Some(true)), Some(true));
    assert_eq!(merge_focus(None, Some(false)), Some(false));
    assert_eq!(merge_focus(Some(false), None), Some(false));
    assert_eq!(merge_focus(None, None), None);
}
#[test]
fn static_capture_sleeps_longer_only_without_a_pending_gpu_handoff() {
    let poll = Duration::from_micros(500);
    assert_eq!(
        notification_wait_interval(Duration::ZERO, false, poll),
        poll
    );
    assert_eq!(
        notification_wait_interval(Duration::from_millis(99), false, poll),
        poll
    );
    assert_eq!(
        notification_wait_interval(Duration::from_millis(100), false, poll),
        Duration::from_millis(10)
    );
    assert_eq!(
        notification_wait_interval(Duration::from_secs(1), true, poll),
        poll
    );
}
#[test]
fn another_window_extends_the_focus_picture_only_while_the_focus_changes() {
    let ms = 1_000_000;
    let mut cadence = Freshness::default();
    // A focus window changing every 150 ms (typing) does not animate:
    // another window's updates stay pictures of their own.
    for at in [100, 250] {
        cadence.observe_focus(at * ms, ms, Some(true));
        cadence.observe_focus((at + 4) * ms, ms, Some(false));
        assert!(!cadence.extends_picture());
        assert_eq!(cadence.picture_start(), Some((at + 4) * ms));
    }
    // A game at 60 Hz animates.
    cadence.observe_focus(300 * ms, ms, Some(true));
    cadence.observe_focus(316 * ms, ms, Some(true));
    let game = cadence.picture_start();
    assert_eq!(game, Some(316 * ms));
    // The duplicate composition and another window's updates extend it.
    cadence.observe_focus(317 * ms, ms, Some(true));
    assert!(!cadence.extends_picture());
    for at in [321, 325, 329] {
        cadence.observe_focus(at * ms, ms, Some(false));
        assert!(cadence.extends_picture());
        assert_eq!(cadence.picture_start(), game);
    }
    // A focus change soon after another window's update is a new picture.
    cadence.observe_focus(330 * ms, ms, Some(true));
    assert!(!cadence.extends_picture());
    assert_eq!(cadence.picture_start(), Some(330 * ms));
    cadence.observe_focus(370 * ms, ms, Some(false));
    assert!(cadence.extends_picture());
    // Once the focus window is still, every update is a picture again.
    cadence.observe_focus(381 * ms, ms, Some(false));
    assert!(!cadence.extends_picture());
    assert_eq!(cadence.picture_start(), Some(381 * ms));
    // Without dirty regions nothing changes.
    cadence.observe_focus(385 * ms, ms, None);
    assert!(!cadence.extends_picture());
    assert_eq!(cadence.picture_start(), Some(385 * ms));
}
fn learned(interval: u64) -> Freshness {
    let mut cadence = Freshness::default();
    for frame in 0..=16 {
        cadence.observe(frame * interval, 500_000);
    }
    cadence
}
#[test]
fn cached_statistics_match_the_original_policy_across_cadence_changes_and_resets() {
    fn reference(cadence: &Freshness, age: u64, normal: u64) -> u64 {
        if cadence.count < 8 {
            return normal;
        }
        let mut intervals = cadence.intervals;
        intervals[..cadence.count].sort_unstable();
        let source = intervals[cadence.count / 2];
        if source < 3_000_000
            || intervals[0] < source.saturating_mul(7) / 8
            || intervals[cadence.count - 1] > source.saturating_mul(9) / 8
        {
            return normal;
        }
        let mut delays = cadence.delays;
        delays[..cadence.count].sort_unstable();
        let expected = source.saturating_add(delays[(cadence.count - 1) / 10]);
        let early = expected.saturating_sub(1_000_000);
        if age < early {
            normal.min(early - age)
        } else if age <= expected.saturating_add(500_000) {
            normal.min(100_000)
        } else {
            normal
        }
    }
    let mut cadence = Freshness::default();
    let mut captured = 1_000_000_000u64;
    for frame in 0..2000u64 {
        let period = [16_666_667, 8_333_333, 2_000_000, 20_000_000][(frame / 40 % 4) as usize];
        captured = match frame % 97 {
            0 => captured.saturating_sub(50_000_000),
            1 => captured,
            2 => captured + 600_000_000,
            _ => captured + period + (frame % 5) * 10_000,
        };
        cadence.observe(captured, (frame % 17) * 100_000);
        let mut intervals = cadence.intervals;
        intervals[..cadence.count].sort_unstable();
        assert_eq!(
            cadence.median_interval(),
            (cadence.count >= 4).then(|| intervals[cadence.count / 2])
        );
        for age in [0, period - 1_000_000, period, period + 2_000_000, u64::MAX] {
            for normal in [50_000, 1_000_000, 10_000_000] {
                assert_eq!(
                    cadence.poll_wait(age, normal),
                    reference(&cadence, age, normal)
                );
            }
        }
    }
}
#[test]
fn a_second_composition_of_a_picture_does_not_shorten_the_picture_interval() {
    // WGC on a 240 Hz VRR display: each frame of a 120 Hz game arrives
    // with another composition 0.1 or 1 ms later; in some sessions every
    // fourth frame has a third 3.5 ms later.
    for third in [false, true] {
        let mut cadence = Freshness::default();
        for frame in 0..200u64 {
            let jitter = (frame.wrapping_mul(2_654_435_761) >> 7) % 600_000;
            let mut start = 1_000_000_000 + frame * 8_333_333 + jitter;
            cadence.observe(start, 100_000);
            let second = start + [100_000, 1_000_000][(frame * 5 % 3).min(1) as usize];
            cadence.observe(second, 100_000);
            if third && frame % 4 == 1 {
                start += 3_500_000;
                cadence.observe(start, 100_000);
            }
            if frame >= 20 {
                let picture = cadence.picture_interval().unwrap();
                assert!(
                    (7_833_333..=8_833_333).contains(&picture),
                    "third={third} frame {frame}: {picture}"
                );
                assert_eq!(cadence.picture_start(), Some(start));
                assert!(cadence.median_interval().unwrap() < 7_500_000);
                // The mean of all captures halves; the pictures' own mean, with a
                // picture split by a third composition counted once, still agrees
                // with their median, so step mode can tell the cadence.
                let mean = cadence.picture_mean_interval().unwrap();
                assert!(
                    mean.abs_diff(picture) <= picture / 16,
                    "third={third} frame {frame}: {mean} {picture}"
                );
                assert!(cadence.mean_interval().unwrap() < 5_000_000);
            }
        }
    }
    // 160 fps on the 240 Hz display: pictures one and two refreshes apart
    // in turn. Their mean stays apart from the median, so step mode stays off.
    let mut alternating = Freshness::default();
    let mut at = 1_000_000_000u64;
    for picture in 0..64u64 {
        at += [4_166_667, 8_333_333][(picture % 2) as usize];
        alternating.observe(at, 100_000);
        alternating.observe(at + 100_000, 100_000);
    }
    let median = alternating.picture_interval().unwrap();
    let mean = alternating.picture_mean_interval().unwrap();
    assert!(mean.abs_diff(median) > median / 8, "{median} {mean}");
    // The 1000 Hz virtual display's own refresh is no picture cadence.
    let mut display = Freshness::default();
    for tick in 0..100 {
        display.observe(tick * 1_000_000, 100_000);
    }
    assert_eq!(display.picture_interval(), None);
    assert_eq!(display.median_interval(), Some(1_000_000));
    // A source without paired compositions has the same median either way.
    let mut cadence = Freshness::default();
    for frame in 0..100u64 {
        cadence.observe(frame * 16_666_667 + frame * 7_919 % 900_000, 100_000);
        assert_eq!(cadence.picture_interval(), cadence.median_interval());
    }
    // A restart starts the picture history over.
    cadence.observe(1, 100_000);
    assert_eq!(cadence.picture_interval(), None);
    assert_eq!(cadence.picture_start(), Some(1));
}
#[test]
fn predictive_polling_is_bounded_and_static_or_irregular_sources_use_normal_waits() {
    let mut cadence = learned(8_333_333);
    assert_eq!(cadence.poll_wait(2_000_000, 1_000_000), 1_000_000);
    assert_eq!(cadence.poll_wait(7_500_000, 1_000_000), 333_333);
    assert_eq!(cadence.poll_wait(8_000_000, 1_000_000), 100_000);
    assert_eq!(cadence.poll_wait(10_000_000, 1_000_000), 1_000_000);
    assert_eq!(cadence.poll_wait(1_000_000_000, 1_000_000), 1_000_000);
    cadence.observe(16 * 8_333_333 + 20_000_000, 500_000);
    assert_eq!(cadence.poll_wait(8_000_000, 1_000_000), 1_000_000);
    assert_eq!(
        Freshness::default().poll_wait(8_000_000, 1_000_000),
        1_000_000
    );
    assert_eq!(
        learned(1_000_000).poll_wait(1_000_000, 1_000_000),
        1_000_000
    );
}
#[test]
fn publication_preserves_static_updates_and_a_waiting_encode_slot() {
    assert_eq!(
        publication_deadline(20_000_000, 3_000_000, 8_333_333, 0),
        Some(19_750_000)
    );
    assert_eq!(
        publication_deadline(20_000_000, 19_000_000, 8_333_333, 0),
        None
    );
    assert_eq!(
        publication_deadline(20_000_000, 21_000_000, 8_333_333, 0),
        None
    );
    assert_eq!(
        publication_deadline(20_000_000, 3_000_000, 12_001_000, 0),
        None
    );
    assert_eq!(publication_deadline(0, 3, 1, 0), None);
    let mut history = Intervals::default();
    for time in [0, 8_000_000, 16_000_000, 24_000_000] {
        assert_eq!(history.observe(time), 0);
    }
    assert_eq!(history.observe(32_000_000), 8_000_000);
    assert_eq!(history.observe(432_000_000), 8_000_000);
}
