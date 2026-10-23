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

/// Runs `frames` game presents through the clock; returns (host present, client present,
/// late) per frame. `game` gives each frame interval, `jitter` each frame's lateness
/// beyond the fixed 20 ms path, `drift_ppm` how much faster the client's clock runs.
fn run(
    clock: &mut PresentClock,
    frames: usize,
    mut game: impl FnMut(usize) -> i64,
    mut jitter: impl FnMut(usize) -> i64,
    drift_ppm: i64,
) -> Vec<(i64, i64, bool)> {
    let offset = 987_654_321;
    let mut host = 0;
    (0..frames)
        .map(|i| {
            host += game(i);
            let host_ready = host + 20_000 + jitter(i);
            let ready = host_ready + offset + host_ready * drift_ppm / 1_000_000;
            let s = clock.schedule(rtp(host), ready as u64);
            assert!(s.present_us as i64 >= ready, "never before the frame is ready");
            (host, s.present_us as i64, s.late)
        })
        .collect()
}

fn intervals(shown: &[(i64, i64, bool)]) -> impl Iterator<Item = (i64, i64)> + '_ {
    shown
        .windows(2)
        .filter(|w| !w[0].2 && !w[1].2)
        .map(|w| (w[1].0 - w[0].0, w[1].1 - w[0].1))
}

#[test]
fn an_even_game_is_shown_evenly_through_jitter() {
    let mut clock = PresentClock::default();
    let mut noise = Lcg(1);
    let shown = run(&mut clock, 3000, |_| 11_111, |_| noise.next(8_000), 0);
    let settled = &shown[200..];
    let late = settled.iter().filter(|s| s.2).count();
    assert!(late * 100 <= settled.len() * 4, "{late} of {} late", settled.len());
    // Off only by the delay's own steps: at most 25 us falling, and rarely rising.
    let errors: Vec<i64> = intervals(settled).map(|(game, screen)| (screen - game).abs()).collect();
    let close = errors.iter().filter(|e| **e <= 40).count();
    assert!(close * 100 >= errors.len() * 95, "{close} of {}", errors.len());
    assert!(errors.iter().all(|e| *e < 2_000), "{:?}", errors.iter().max());
    // Presenting on arrival would have shown these intervals.
    let mut noise = Lcg(1);
    let arrival: Vec<i64> = (0..3000).map(|i| i * 11_111 + noise.next(8_000)).collect();
    let spread = arrival.windows(2).map(|w| w[1] - w[0]).max().unwrap()
        - arrival.windows(2).map(|w| w[1] - w[0]).min().unwrap();
    assert!(spread > 10_000, "the test's jitter is real: {spread}");
}

#[test]
fn uneven_game_frame_times_are_kept() {
    let mut clock = PresentClock::default();
    let mut noise = Lcg(2);
    let mut pattern = Lcg(3);
    let shown = run(
        &mut clock,
        2000,
        |_| 7_000 + pattern.next(9_000),
        |_| noise.next(5_000),
        0,
    );
    let kept = intervals(&shown[200..])
        .filter(|(game, screen)| (screen - game).abs() <= 25)
        .count();
    assert!(kept * 100 >= 1800 * 90, "{kept} intervals kept");
}

#[test]
fn delay_tracks_the_jitter_percentile_and_falls_slowly() {
    let mut clock = PresentClock::default();
    let mut noise = Lcg(4);
    run(&mut clock, 500, |_| 10_000, |i| if i < 250 { noise.next(12_000) } else { noise.next(2_000) }, 0);
    // Rose to cover ~12 ms of jitter; after it calmed down falls at 25 us a frame.
    let high = clock.delay_us().unwrap();
    run(&mut clock, 100, |_| 10_000, |_| noise.next(2_000), 0);
    let lower = clock.delay_us().unwrap();
    assert!(lower < high);
    assert!(high - lower <= 100 * 25 + 1);
    run(&mut clock, 2000, |_| 10_000, |_| noise.next(2_000), 0);
    let settled = clock.delay_us().unwrap();
    // Steady state: the ~97th percentile of 0..2 ms plus the margin (the clocks' offset
    // and the fixed path are in the delay too).
    let base = 987_654_321 + 20_000;
    assert!((base + 1_800..=base + 2_600).contains(&settled), "{}", settled - base);
}

#[test]
fn a_lone_spike_is_shown_late_but_a_slower_path_raises_the_delay() {
    let mut clock = PresentClock::default();
    let shown = run(&mut clock, 300, |_| 10_000, |i| if i == 200 { 15_000 } else { 1_000 }, 0);
    assert!(shown[200].2, "the spike itself is late");
    assert!(!shown[201].2);
    let before = shown[199].1 - shown[199].0;
    let after = shown[250].1 - shown[250].0;
    assert!((after - before).abs() <= 25, "one spike does not hold every frame");

    let mut clock = PresentClock::default();
    let shown = run(&mut clock, 400, |_| 10_000, |i| if i < 200 { 1_000 } else { 15_000 }, 0);
    let late = shown[200..].iter().filter(|s| s.2).count();
    assert!(late <= 6, "{late} late before the delay covered the slower path");
    assert!(!shown[210..].iter().any(|s| s.2));
}

#[test]
fn a_late_burst_at_the_start_does_not_hold_the_stream() {
    // The first pictures wait for the decoder and the GPU: 90 ms late and falling, then an
    // even 20 ms path with 1 ms of jitter. Measured on a real stream, the delay seeded from the
    // first picture held every frame for the longest hold for about a minute.
    let mut clock = PresentClock::default();
    let mut noise = Lcg(6);
    let shown = run(&mut clock, 700, |_| 16_667, |i| if i < 8 { 90_000 - i as i64 * 11_000 } else { noise.next(1_000) }, 0);
    let hold = |i: usize| shown[i].1 - (shown[i].0 + 987_654_321 + 20_000);
    for i in [30, 100, 300, 699] {
        assert!((0..=3_000).contains(&hold(i)), "frame {i} held {} us", hold(i));
    }
    assert!(clock.delay_us().unwrap() < 987_654_321 + 20_000 + 3_000);
    assert!(shown[..8].iter().all(|s| !s.2), "the burst itself is shown as it comes");
    // A restarted video clock warms up again.
    let late_again = clock.schedule(rtp(1_000), (shown[699].1 + 80_000) as u64);
    assert_eq!(late_again.hold_us, clock.settings.margin_us);
}

#[test]
fn client_clock_drift_is_followed() {
    for drift in [-200, 200] {
        let mut clock = PresentClock::default();
        let mut noise = Lcg(5);
        let shown = run(&mut clock, 20_000, |_| 8_333, |_| noise.next(3_000), drift);
        let late = shown[1000..].iter().filter(|s| s.2).count();
        assert!(late * 100 <= 19_000 * 5, "drift {drift}: {late} late");
        let hold = shown[19_999].1 - shown[19_999].0;
        assert!(hold < 987_654_321 + 20_000 + 3_000 + 40_000, "hold did not run away");
    }
}

#[test]
fn rtp_wrap_and_a_restarted_clock() {
    let mut clock = PresentClock::default();
    // Just below the 32-bit wrap, at 13.25 hours.
    let start = (u32::MAX as i64 - 450_000) * 100 / 9;
    let mut host = start;
    let mut last = None;
    for _ in 0..1000 {
        host += 10_000;
        let s = clock.schedule(rtp(host), (host + 5_000) as u64);
        if let Some(previous) = last {
            assert_eq!(s.present_us as i64 - previous, 10_000, "smooth across the wrap");
        }
        last = Some(s.present_us as i64);
    }
    // The host restarted its video clock at 0: no hour-long hold, back to normal.
    let s = clock.schedule(rtp(10_000), (host + 15_000) as u64);
    assert!(s.hold_us < 1_000);
    let next = clock.schedule(rtp(20_000), (host + 25_000) as u64);
    assert_eq!(next.present_us - s.present_us, 10_000);
}

#[test]
fn presents_respect_the_display_maximum_and_hold_limit() {
    let mut clock = PresentClock::new(Settings {
        min_interval_us: 6_944, // 144 Hz
        max_hold_us: 30_000,
        ..Settings::default()
    });
    let mut host = 0;
    let mut previous = None;
    for i in 0..100 {
        host += if i % 2 == 0 { 2_000 } else { 12_000 };
        let s = clock.schedule(rtp(host), (host + 3_000) as u64);
        if let Some(p) = previous {
            assert!(s.present_us as i64 - p >= 6_944);
        }
        assert!(s.hold_us <= 30_000);
        previous = Some(s.present_us as i64);
    }
}

#[test]
fn clock_offset_from_jittery_exchanges() {
    let mut offset = ClockOffset::default();
    let mut noise = Lcg(6);
    let true_offset = 5_000_000_000i64;
    for i in 0..40 {
        let client_send = 1_000_000 + i * 250_000;
        let up = 300 + noise.next(4_000);
        let down = 300 + noise.next(4_000);
        let host_receive = client_send + up - true_offset;
        let host_send = host_receive + 50;
        let reply = ClockSyncReply {
            sequence: i as u32,
            client_us: client_send as u64,
            host_receive_us: host_receive as u64,
            host_send_us: host_send as u64,
        };
        offset.add(&reply, (host_send + true_offset + down) as u64).unwrap();
    }
    let estimate = offset.offset_at(11_000_000).unwrap();
    assert!((estimate - true_offset).abs() <= offset.rtt_us().unwrap() / 2 + 1);
    assert!((estimate - true_offset).abs() < 1_000, "{}", estimate - true_offset);
    // A reply received before it was sent is nonsense.
    let bad = ClockSyncReply { client_us: 2_000_000, host_receive_us: 0, host_send_us: 0, sequence: 0 };
    assert_eq!(offset.add(&bad, 1_000_000), None);
}

#[test]
fn clock_offset_follows_drift_and_restarts() {
    let mut offset = ClockOffset::default();
    // The client's clock runs 100 ppm fast; the exchanges are clean.
    let at = |client: i64| client - client / 10_000;
    for i in 0..32 {
        let client = 1_000_000 + i * 250_000;
        let host = at(client) as u64;
        let reply = ClockSyncReply { sequence: 0, client_us: client as u64, host_receive_us: host, host_send_us: host };
        offset.add(&reply, client as u64).unwrap();
    }
    let later = 30_000_000;
    let predicted = offset.offset_at(later).unwrap();
    assert!((predicted - (later - at(later))).abs() <= 50, "{predicted}");
    // A new video clock on the host: a second-plus jump drops the old samples.
    let reply = ClockSyncReply { sequence: 0, client_us: 40_000_000, host_receive_us: 1_000, host_send_us: 1_000 };
    offset.add(&reply, 40_000_000).unwrap();
    assert_eq!(offset.offset_at(40_000_000), Some(40_000_000 - 1_000));
}

#[test]
fn latency_is_reported_with_clock_sync() {
    let mut clock = PresentClock::default();
    let reply = ClockSyncReply { sequence: 0, client_us: 1_000, host_receive_us: 500, host_send_us: 500 };
    clock.offset.add(&reply, 1_000).unwrap(); // client = host + 500
    let s = clock.schedule(rtp(100_000), 100_000 + 500 + 7_000);
    assert_eq!(s.latency_us, Some(7_000 + 500));
}
