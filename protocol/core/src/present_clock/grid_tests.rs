//! Tests of scheduling onto a vsync grid and of the content rate.
use super::*;
use alloc::vec::Vec;

/// Deterministic noise in `0..range`.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, range: i64) -> i64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) % range as u64) as i64
    }
}

fn rtp(host_us: i64) -> u32 {
    (host_us * 9 / 100) as u64 as u32
}

fn grid(interval_us: i64, anchor_us: i64) -> Grid {
    Grid { anchor_us, interval_us, lead_us: interval_us / 4, slack_us: interval_us / 2 }
}

/// The grid as a display reports it: the latest vsync before the frame is ready, with a few
/// microseconds of measurement noise.
fn nearest_anchor(interval_us: i64, phase_us: i64, ready_us: i64, noise: i64) -> i64 {
    (ready_us - phase_us).div_euclid(interval_us) * interval_us + phase_us + noise
}

/// Runs `frames` presents at `fps` through the clock on a panel of `hz`; returns the frame's
/// ready time and its grid schedule.
fn run_grid(
    clock: &mut PresentClock,
    fps: i64,
    hz: i64,
    frames: usize,
    slack_us: i64,
    mut jitter: impl FnMut(usize) -> i64,
) -> Vec<(i64, GridSchedule)> {
    let interval = 1_000_000 / hz;
    let period = 1_000_000 / fps;
    let offset = 5_000_000;
    let mut noise = Lcg(7);
    (0..frames)
        .map(|i| {
            let host = (i as i64 + 1) * period;
            let ready = host + 20_000 + jitter(i) + offset;
            let anchor = nearest_anchor(interval, 3_333, ready, noise.next(21) - 10);
            let g = Grid { slack_us, ..grid(interval, anchor) };
            let s = clock.schedule_on_grid(rtp(host), ready as u64, g);
            assert!(s.release_us as i64 >= ready, "never released before it is ready");
            assert!(s.vsync_us as i64 >= ready, "never planned for a vsync it cannot make");
            (ready, s)
        })
        .collect()
}

/// The vsync's place on the panel's own grid; `vsync_index` counts from a moving anchor.
fn vsync_steps(shown: &[(i64, GridSchedule)], interval: i64) -> Vec<i64> {
    shown
        .windows(2)
        .map(|w| (w[1].1.vsync_us as i64 - w[0].1.vsync_us as i64 + interval / 2).div_euclid(interval))
        .collect()
}

#[test]
fn snap_takes_the_first_vsync_at_or_after() {
    assert_eq!(snap(1_000, 0, 16_667), (16_667, 1));
    assert_eq!(snap(16_667, 0, 16_667), (16_667, 1), "on a vsync stays on it");
    assert_eq!(snap(16_668, 0, 16_667), (33_334, 2));
    assert_eq!(snap(0, 0, 8_333), (0, 0));
    // An anchor in the future: indices are negative.
    let future = 1_000_000 + 5 * 11_111;
    assert_eq!(snap(1_000_000, future, 11_111), (1_000_000, -5));
    assert_eq!(snap(1_000_001, future, 11_111), (1_011_111, -4));
    // No grid: every instant.
    assert_eq!(snap(1234, 0, 0), (1234, 0));
    assert_eq!(snap(1234, 99, -5), (1234, 0));
}

#[test]
fn without_a_grid_the_schedule_is_unchanged() {
    let mut a = PresentClock::default();
    let mut b = PresentClock::default();
    let mut noise = Lcg(3);
    let mut host = 0;
    for i in 0..2000 {
        host += 6_944 + noise.next(300);
        let ready = (host + 15_000 + noise.next(9_000) + 40_000_000) as u64;
        let plain = a.schedule(rtp(host), ready);
        let gridless = b.schedule_on_grid(rtp(host), ready, grid(0, 123));
        assert_eq!(gridless.release_us, plain.present_us, "frame {i}");
        assert_eq!(gridless.late, plain.late);
        assert_eq!(gridless.hold_us, plain.hold_us);
        assert_eq!(gridless.latency_us, plain.latency_us);
        assert_eq!(a.delay_us(), b.delay_us(), "the delay is learned the same way");
    }
    assert_eq!((a.frames, a.late_frames), (b.frames, b.late_frames));
}

#[test]
fn a_grid_learns_the_delay_of_a_lower_percentile() {
    let mut plain = PresentClock::default();
    let mut lower = PresentClock::new(Settings { percentile: GRID_PERCENTILE, ..Settings::default() });
    let mut on_grid = PresentClock::default();
    let mut noise = Lcg(11);
    for i in 0..1500 {
        let host = i * 11_111;
        let ready = (host + 20_000 + noise.next(8_000) + 7_000_000) as u64;
        plain.schedule(rtp(host), ready);
        lower.schedule(rtp(host), ready);
        on_grid.schedule_on_grid(rtp(host), ready, grid(16_667, 1_000));
    }
    assert_eq!(on_grid.delay_us(), lower.delay_us(), "the same rule with the grid's percentile");
    // Lateness is uniform over 8 ms (on the clocks' offset of 7 s and 20 ms of path): the 75th
    // percentile is near 6 ms above the path, the 97th near 7.8 ms (a window of 128 frames is noisy).
    let (grid_delay, plain_delay) = (on_grid.delay_us().unwrap(), plain.delay_us().unwrap());
    assert!(plain_delay - grid_delay > 1_000, "{plain_delay} against {grid_delay}");
    assert!((grid_delay - 7_000_000 - 20_000 - 6_000 - 500).abs() < 700, "{grid_delay}");
}

#[test]
fn a_grid_with_no_interval_keeps_the_plain_percentile() {
    let mut plain = PresentClock::default();
    let mut gridless = PresentClock::default();
    let mut noise = Lcg(12);
    for i in 0..1500 {
        let host = i * 11_111;
        let ready = (host + 20_000 + noise.next(8_000) + 7_000_000) as u64;
        plain.schedule(rtp(host), ready);
        gridless.schedule_on_grid(rtp(host), ready, Grid { interval_us: 0, ..grid(0, 0) });
    }
    assert_eq!(plain.delay_us(), gridless.delay_us());
}

#[test]
fn observing_frames_does_not_change_the_schedule() {
    let mut a = PresentClock::default();
    let mut b = PresentClock::default();
    let mut noise = Lcg(21);
    for i in 0..1000 {
        let host = i * 11_111;
        let ready = (host + 20_000 + noise.next(7_000) + 3_000_000) as u64;
        b.observe(rtp(host));
        assert_eq!(a.schedule(rtp(host), ready), b.schedule(rtp(host), ready));
    }
}

#[test]
fn an_even_game_lands_one_vsync_apart_on_a_matching_panel() {
    let mut clock = PresentClock::default();
    let mut noise = Lcg(5);
    let shown = run_grid(&mut clock, 60, 60, 1200, 8_333, |_| noise.next(6_000));
    let settled = &shown[120..];
    let steps = vsync_steps(settled, 16_667);
    // Frames are ready anywhere in a 6 ms window; the vsync they get is the panel's own.
    let exact = steps.iter().filter(|s| **s == 1).count();
    assert!(exact * 100 >= steps.len() * 96, "{exact} of {} one vsync apart", steps.len());
    assert!(settled.iter().all(|(_, s)| !s.shares_vsync || s.late), "no frame shares at equal rates");
}

#[test]
fn a_frame_is_never_held_past_the_vsync_it_would_have_had_by_more_than_one() {
    // Released at once, a frame is shown at the first vsync after it is ready. The scheduler
    // may use a later one only to keep the cadence, and at most one later.
    for (fps, hz) in [(60, 60), (90, 90), (90, 120), (72, 120), (120, 120), (45, 90), (30, 60)] {
        let mut clock = PresentClock::default();
        let mut noise = Lcg(fps as u64 * 31 + hz as u64);
        let interval = 1_000_000 / hz;
        let shown = run_grid(&mut clock, fps, hz, 1500, interval / 2, |_| noise.next(8_000));
        let mut worst = 0;
        for (ready, s) in &shown[150..] {
            let anchor = s.vsync_us as i64 - s.vsync_index * interval;
            let first = snap(*ready, anchor, interval).1;
            worst = worst.max(s.vsync_index - first);
            assert!(s.vsync_index >= first, "{fps} on {hz}");
        }
        assert!(worst <= 1, "{fps} fps on {hz} Hz: {worst} vsyncs late");
    }
}

#[test]
fn ninety_on_a_120_panel_follows_the_four_three_pattern() {
    let mut clock = PresentClock::default();
    let shown = run_grid(&mut clock, 90, 120, 900, 0, |_| 0);
    let steps = vsync_steps(&shown[200..], 8_333);
    assert!(steps.iter().all(|s| (1..=2).contains(s)), "{steps:?}");
    // 120 / 90 = 4/3 vsyncs a frame: one in three intervals is two vsyncs long.
    let twos = steps.iter().filter(|s| **s == 2).count();
    let expected = steps.len() / 3;
    assert!(twos.abs_diff(expected) <= 2, "{twos} long intervals of {}", steps.len());
    assert!(shown[200..].iter().all(|(_, s)| !s.shares_vsync), "no frame is lost");
}

#[test]
fn content_faster_than_the_panel_shares_vsyncs_and_says_so() {
    let mut clock = PresentClock::default();
    let shown = run_grid(&mut clock, 90, 60, 900, 0, |_| 0);
    let settled = &shown[200..];
    let shared = settled.iter().filter(|(_, s)| s.shares_vsync).count();
    // 30 of every 90 frames cannot be shown.
    assert!(shared.abs_diff(settled.len() / 3) <= settled.len() / 20, "{shared} of {}", settled.len());
    // The cadence does not run behind for good: a frame waits for at most two vsyncs.
    let longest = settled.iter().map(|(ready, s)| s.vsync_us as i64 - ready).max().unwrap();
    assert!(longest <= 2 * 16_667, "a frame waited {longest} us for its vsync");
}

#[test]
fn a_frame_ready_inside_the_lead_is_released_at_once() {
    let mut clock = PresentClock::default();
    let g = Grid { anchor_us: 1_000_000, interval_us: 16_667, lead_us: 4_000, slack_us: 0 };
    // Ready 1 ms before the vsync at 1_016_667: inside the 4 ms lead, so no wait.
    let s = clock.schedule_on_grid(rtp(0), 1_015_667, g);
    assert_eq!(s.vsync_us, 1_016_667);
    assert_eq!(s.release_us, 1_015_667);
    assert_eq!(s.hold_us, 0);
    // Ready 10 ms before the next one: released 4 ms before that vsync.
    let s = clock.schedule_on_grid(rtp(1_000), 1_023_000, Grid { anchor_us: 1_016_667, ..g });
    assert_eq!(s.vsync_us, 1_033_334);
    assert_eq!(s.release_us, 1_033_334 - 4_000);
    assert_eq!(s.hold_us, 1_033_334 - 4_000 - 1_023_000);
}

#[test]
fn a_late_frame_reports_how_late() {
    let mut clock = PresentClock::default();
    let mut host = 0;
    let g = grid(8_333, 0);
    // Settle on 20 ms of path delay.
    for _ in 0..400 {
        host += 11_111;
        clock.schedule_on_grid(rtp(host), (host + 20_000 + 9_000_000) as u64, g);
    }
    host += 11_111;
    let on_time = clock.schedule_on_grid(rtp(host), (host + 20_000 + 9_000_000) as u64, g);
    assert!(!on_time.late);
    assert_eq!(on_time.late_by_us, 0);
    host += 11_111;
    let s = clock.schedule_on_grid(rtp(host), (host + 45_000 + 9_000_000) as u64, g);
    assert!(s.late, "25 ms behind the delay");
    assert!(s.late_by_us > 20_000, "{}", s.late_by_us);
}

#[test]
fn a_moving_anchor_does_not_make_false_collisions() {
    // The same grid reported from different vsyncs and with noise gives one frame per vsync.
    let mut clock = PresentClock::default();
    let shown = run_grid(&mut clock, 60, 60, 600, 8_333, |i| (i as i64 * 37) % 3_000);
    assert!(shown[100..].iter().all(|(_, s)| !s.shares_vsync));
}

#[test]
fn reset_forgets_the_previous_vsync() {
    let mut clock = PresentClock::default();
    let g = grid(16_667, 0);
    clock.schedule_on_grid(rtp(0), 5_000_000, g);
    clock.reset();
    let s = clock.schedule_on_grid(rtp(0), 5_000_000, g);
    assert!(!s.shares_vsync);
}

#[test]
fn latency_to_the_vsync_needs_clock_sync() {
    let mut clock = PresentClock::default();
    let s = clock.schedule_on_grid(rtp(100_000), 3_000_000, grid(8_333, 0));
    assert_eq!(s.latency_us, None);
}

// The content rate.

fn feed(clock: &mut PresentClock, from_us: i64, fps_milli: i64, seconds: i64) -> i64 {
    let mut host = from_us;
    let end = from_us + seconds * 1_000_000;
    while host < end {
        clock.observe(rtp(host));
        host += 1_000_000_000 / fps_milli;
    }
    host
}

#[test]
fn a_steady_rate_is_reported_after_two_seconds() {
    let mut clock = PresentClock::default();
    let end = feed(&mut clock, 1_000_000, 90_000, 1);
    assert_eq!(clock.content_rate_mhz(), None, "one second is not enough");
    feed(&mut clock, end, 90_000, 2);
    let rate = clock.content_rate_mhz().expect("rate after three seconds");
    assert!(rate.abs_diff(90_000) <= 50, "{rate}");
}

#[test]
fn common_rates_are_recovered() {
    for fps in [24_000u32, 30_000, 60_000, 72_000, 90_000, 120_000, 144_000] {
        let mut clock = PresentClock::default();
        feed(&mut clock, 0, i64::from(fps), 4);
        let rate = clock.content_rate_mhz().expect("rate");
        assert!(rate.abs_diff(fps) * 1000 <= fps * 5, "{fps} gave {rate}");
    }
}

#[test]
fn jittered_frames_still_give_the_rate() {
    let mut clock = PresentClock::default();
    let mut noise = Lcg(9);
    for i in 1..=90 * 4 {
        // Presents of an even 90 fps game, each a few ms either way.
        clock.observe(rtp(i * 11_111 + noise.next(3_000)));
    }
    let rate = clock.content_rate_mhz().expect("rate");
    assert!(rate.abs_diff(90_000) <= 2_000, "{rate}");
}

#[test]
fn a_hitch_does_not_move_the_rate() {
    let mut clock = PresentClock::default();
    let mut host = feed(&mut clock, 0, 60_000, 3);
    for _ in 0..3 {
        host += 120_000; // a 120 ms hitch
        clock.observe(rtp(host));
        host = feed(&mut clock, host, 60_000, 1);
    }
    let rate = clock.content_rate_mhz().expect("rate");
    assert!(rate.abs_diff(60_000) <= 100, "{rate}");
}

#[test]
fn a_new_rate_is_reported_only_after_it_holds() {
    let mut clock = PresentClock::default();
    let mut host = feed(&mut clock, 0, 90_000, 4);
    assert!(clock.content_rate_mhz().unwrap().abs_diff(90_000) <= 50);
    // The game drops to 30 fps: at first the old rate stays reported.
    host = feed(&mut clock, host, 30_000, 1);
    assert!(clock.content_rate_mhz().unwrap().abs_diff(90_000) <= 50, "not yet");
    feed(&mut clock, host, 30_000, 3);
    let rate = clock.content_rate_mhz().unwrap();
    assert!(rate.abs_diff(30_000) <= 50, "{rate}");
}

#[test]
fn a_drift_inside_three_percent_does_not_change_the_report() {
    let mut clock = PresentClock::default();
    let mut host = feed(&mut clock, 0, 60_000, 3);
    let reported = clock.content_rate_mhz().unwrap();
    for fps in [60_600, 61_200, 59_400, 60_300, 59_800] {
        host = feed(&mut clock, host, fps, 2);
        assert_eq!(clock.content_rate_mhz(), Some(reported), "at {fps}");
    }
}

#[test]
fn a_pause_keeps_the_rate_until_the_new_one_holds() {
    let mut clock = PresentClock::default();
    let host = feed(&mut clock, 0, 60_000, 3);
    // A 5 s pause, then 120 fps.
    let end = feed(&mut clock, host + 5_000_000, 120_000, 1);
    assert!(clock.content_rate_mhz().unwrap().abs_diff(60_000) <= 100, "kept until the new rate holds");
    feed(&mut clock, end, 120_000, 3);
    assert!(clock.content_rate_mhz().unwrap().abs_diff(120_000) <= 200);
    clock.reset();
    assert_eq!(clock.content_rate_mhz(), None);
}

#[test]
fn a_rate_the_game_leaves_without_settling_is_withdrawn() {
    let mut clock = PresentClock::default();
    let mut host = feed(&mut clock, 0, 60_000, 4);
    assert!(clock.content_rate_mhz().unwrap().abs_diff(60_000) <= 100);
    // The game wanders between 90 and 120 fps, never holding either for two seconds.
    for _ in 0..4 {
        host = feed(&mut clock, host, 90_000, 1);
        host = feed(&mut clock, host, 120_000, 1);
    }
    assert_eq!(clock.content_rate_mhz(), None, "unsteady content has no rate");
    // It settles: the new rate is reported again after it holds.
    feed(&mut clock, host, 90_000, 4);
    assert!(clock.content_rate_mhz().unwrap().abs_diff(90_000) <= 100);
}

#[test]
fn a_short_excursion_keeps_the_rate() {
    let mut clock = PresentClock::default();
    let mut host = feed(&mut clock, 0, 60_000, 4);
    host = feed(&mut clock, host, 90_000, 1);
    feed(&mut clock, host, 60_000, 3);
    assert!(clock.content_rate_mhz().unwrap().abs_diff(60_000) <= 100);
}

#[test]
fn the_content_rate_survives_rtp_wrap_and_counts_a_repeated_stamp_once() {
    let mut clock = PresentClock::default();
    let start = (u32::MAX as i64 - 90_000) * 100 / 9; // one second before the wrap
    let mut host = start;
    for _ in 0..60 * 5 {
        clock.observe(rtp(host));
        clock.observe(rtp(host));
        host += 16_667;
    }
    let rate = clock.content_rate_mhz().expect("rate across the wrap");
    assert!(rate.abs_diff(60_000) <= 100, "{rate}");
}
