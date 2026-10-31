use super::*;
use alloc::vec::Vec;

const PERIOD: i64 = 8_333_333; // 120.00 Hz on the client.

#[test]
fn report_round_trips_and_rejects_nonsense() {
    let report = Report { frames: 60, period_ns: PERIOD as u32, lead_ns: -1_200_000, spread_ns: 900_000 };
    let bytes = report.encode();
    assert_eq!(Report::decode(&bytes), Some(report));

    let mut longer = bytes.to_vec();
    longer.extend_from_slice(&[0xff; 8]);
    assert_eq!(Report::decode(&longer), Some(report), "newer clients may append fields");

    assert_eq!(Report::decode(&bytes[..REPORT_BYTES - 1]), None);
    let mut version = bytes;
    version[0] = 2;
    assert_eq!(Report::decode(&version), None);
    let zero_period = Report { period_ns: 0, ..report }.encode();
    assert_eq!(Report::decode(&zero_period), None);
    let wild_lead = Report { lead_ns: 2 * PERIOD as i32, ..report }.encode();
    assert_eq!(Report::decode(&wild_lead), None);
    let no_frames = Report { frames: 0, ..report }.encode();
    assert_eq!(Report::decode(&no_frames), None);
}

#[test]
fn window_reports_the_early_edge() {
    let mut window = SlackWindow::default();
    for i in 0..100 {
        window.push(3_000_000 + i * 10_000, PERIOD); // 3.00 to 3.99 ms
    }
    let report = window.report(10).unwrap();
    assert_eq!(report.frames, 100);
    assert_eq!(report.period_ns, PERIOD as u32);
    assert!((report.lead_ns - 3_100_000).abs() <= 20_000, "{report:?}");
    assert!((report.spread_ns as i64 - 800_000).abs() <= 20_000, "{report:?}");
    assert!(window.is_empty());
    assert_eq!(window.report(1), None, "the window starts over after a report");
}

#[test]
fn a_cluster_straddling_the_latch_reads_as_slightly_late() {
    // Half the frames make the latch with 0.2 ms to spare, half miss it by 0.2 ms and so
    // have almost a full refresh of slack until the next one.
    let mut window = SlackWindow::default();
    for i in 0..50 {
        window.push(200_000 + i, PERIOD);
        window.push(PERIOD - 200_000 - i, PERIOD);
    }
    let report = window.report(10).unwrap();
    assert!(report.lead_ns < 0 && report.lead_ns > -300_000, "{report:?}");
    assert!(report.spread_ns < 500_000, "{report:?}");
}

#[test]
fn a_refresh_rate_change_restarts_the_window() {
    let mut window = SlackWindow::default();
    window.push(1_000_000, PERIOD);
    window.push(1_000_000, 16_666_667);
    assert_eq!(window.len(), 1);
    assert_eq!(window.report(1).unwrap().period_ns, 16_666_667);
}

#[test]
fn the_lock_waits_for_a_report() {
    let mut lock = PhaseLock::default();
    assert!(!lock.locked());
    assert_eq!(lock.next_interval_ns(8_331_945), 8_331_945);
}

/// Deterministic xorshift for jitter.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// Uniform in [-amplitude, amplitude].
    fn jitter(&mut self, amplitude: i64) -> i64 {
        if amplitude == 0 { 0 } else { (self.next() % (2 * amplitude as u64 + 1)) as i64 - amplitude }
    }
}

struct Outcome {
    /// Slack of every frame after the lock settled, in client nanoseconds.
    settled: Vec<i64>,
}

/// Simulates a host at 120.02 Hz streaming to a 120.00 Hz client for `seconds`.
///
/// `ppm` is how much faster the host's clock runs than the client's. Frames reach the client
/// `latency_ns` (plus uniform jitter) after capture; the client reports every 60 frames and the
/// report reaches the host 5 frames later. Slack is measured against latch deadlines at
/// `latch_phase_ns + n * PERIOD`.
fn simulate(lock_enabled: bool, ppm: i64, jitter_ns: i64, seconds: i64, latch_phase_ns: i64) -> Outcome {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut lock = PhaseLock::default();
    let mut window = SlackWindow::default();
    let host_nominal = 1_000_000_000 / 120; // The host's own idea of 120 Hz.
    let latency_ns = 14_000_000;
    let mut host_time: i64 = 0;
    let mut in_flight: Vec<(usize, Report)> = Vec::new();
    let frames = (seconds * 120) as usize;
    let mut settled = Vec::new();
    for frame in 0..frames {
        // Host time to client time.
        let capture = host_time + host_time * ppm / 1_000_000;
        let ready = capture + latency_ns + rng.jitter(jitter_ns);
        let slack = (latch_phase_ns - ready).rem_euclid(PERIOD);
        if frame >= 120 * 20 {
            settled.push(slack);
        }
        window.push(slack, PERIOD);
        if window.len() == 60 {
            in_flight.push((frame + 5, window.report(30).unwrap()));
        }
        while let Some(&(due, report)) = in_flight.first() {
            if due > frame {
                break;
            }
            in_flight.remove(0);
            if lock_enabled {
                lock.on_report(&report);
            }
        }
        host_time += lock.next_interval_ns(host_nominal);
    }
    Outcome { settled }
}

fn percentile(values: &mut [i64], p: usize) -> i64 {
    values.sort_unstable();
    values[(values.len() - 1) * p / 100]
}

fn mean(values: &[i64]) -> i64 {
    values.iter().sum::<i64>() / values.len() as i64
}

#[test]
fn without_the_lock_the_wait_drifts_across_a_whole_refresh() {
    // 167 ppm: one full refresh of drift every ~50 s.
    let mut outcome = simulate(false, 167, 300_000, 120, 3_000_000);
    let low = percentile(&mut outcome.settled, 1);
    let high = percentile(&mut outcome.settled, 99);
    assert!(high - low > PERIOD * 9 / 10, "drift should sweep the whole refresh: {low}..{high}");
}

#[test]
fn the_lock_holds_frames_just_before_the_latch_despite_drift() {
    for (ppm, jitter, phase) in [(167, 300_000, 3_000_000), (-167, 300_000, 7_000_000), (500, 1_000_000, 0), (0, 0, 4_100_000)] {
        let mut outcome = simulate(true, ppm, jitter, 600, phase);
        let average = mean(&outcome.settled);
        let low = percentile(&mut outcome.settled, 1);
        let high = percentile(&mut outcome.settled, 99);
        // Locked: nearly every frame makes its latch with some room, and the average wait is
        // the margin plus jitter rather than half a refresh (4.2 ms) that swings over time.
        assert!(low > 0 && high < PERIOD / 2, "ppm {ppm} jitter {jitter}: slack {low}..{high}");
        assert!(average < PhaseLock::DEFAULT_MARGIN_NS + 2 * jitter + 500_000,
                "ppm {ppm} jitter {jitter}: average slack {average}");
    }
}

#[test]
fn interval_changes_stay_small() {
    let mut lock = PhaseLock::default();
    lock.on_report(&Report { frames: 60, period_ns: PERIOD as u32, lead_ns: 4_000_000, spread_ns: 0 });
    let nominal = 1_000_000_000 / 120;
    for _ in 0..120 {
        let interval = lock.next_interval_ns(nominal);
        assert!((interval - PERIOD).abs() <= PhaseLock::MAX_STEP_NS + PERIOD / 100, "{interval}");
    }
}


/// A 60 fps stream locked to a 60 Hz screen, with a heavy tail of late frames (decode and
/// Wi-Fi): an exponential delay of `tail_ns` on average on top of a fixed latency. Reports go
/// out every 30 frames with their elapsed time when `paced`, plain otherwise. Returns the share
/// of settled frames that missed their latch, in tenths of a percent, and their average slack.
fn sixty_on_sixty(paced: bool, tail_ns: f64) -> (usize, i64) {
    const SIXTY: i64 = 16_666_667;
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut lock = PhaseLock::default();
    let mut window = SlackWindow::default();
    let mut host_time: i64 = 0;
    let mut in_flight: Vec<(usize, Report)> = Vec::new();
    let (mut missed, mut settled, mut slack_sum) = (0usize, 0usize, 0i64);
    for frame in 0..60 * 300 {
        let capture = host_time + host_time * 50 / 1_000_000;
        let uniform = (rng.next() >> 11) as f64 / (1u64 << 53) as f64;
        let tail = (-(1.0 - uniform).ln() * tail_ns) as i64;
        let ready = capture + 20_000_000 + tail;
        let slack = (5_000_000 - ready).rem_euclid(SIXTY);
        // The latch this capture is aimed at is the one its frame makes without delay.
        let aimed = (5_000_000 - capture - 20_000_000).rem_euclid(SIXTY);
        if frame >= 60 * 30 {
            settled += 1;
            if tail > aimed { missed += 1; }
            slack_sum += slack;
        }
        window.push(slack, SIXTY);
        if window.len() == 30 {
            let report = if paced { window.report_over(10, 30 * SIXTY) } else { window.report(10) };
            in_flight.push((frame + 3, report.unwrap()));
        }
        while let Some(&(due, report)) = in_flight.first() {
            if due > frame { break; }
            in_flight.remove(0);
            lock.on_report(&report);
        }
        host_time += lock.next_interval_ns(SIXTY);
    }
    (missed * 1000 / settled, slack_sum / settled as i64)
}

#[test]
fn at_one_frame_per_refresh_fewer_frames_miss_their_latch() {
    // Retroid Pocket Flip 2 (60 Hz) against a 120 Hz phone: at 60 fps a frame that misses
    // its latch costs a whole refresh and the next frame's slot.
    for (tail, plain_max, paced_max) in [(1_000_000.0, 25, 13), (2_000_000.0, 50, 30), (3_000_000.0, 70, 60)] {
        let (plain, plain_wait) = sixty_on_sixty(false, tail);
        let (paced, paced_wait) = sixty_on_sixty(true, tail);
        assert!(plain <= plain_max && paced <= paced_max,
                "tail {tail}: {plain}\u{2030} missed plain, {paced}\u{2030} paced");
        // The extra room costs well under a millisecond of wait, misses included.
        assert!(paced_wait - plain_wait < 1_000_000, "tail {tail}: {plain_wait} -> {paced_wait} ns");
    }
}

#[test]
fn the_extra_room_needs_a_frame_for_nearly_every_refresh() {
    let fill = |window: &mut SlackWindow, period: i64| {
        for i in 0..30 {
            window.push(2_000_000 + (i % 10) * 400_000, period);
        }
    };
    let mut window = SlackWindow::default();
    fill(&mut window, 16_666_667);
    let plain = window.report(10).unwrap();
    // 30 frames in half a second on a 60 Hz screen: one per refresh.
    fill(&mut window, 16_666_667);
    let paced = window.report_over(10, 500_000_000).unwrap();
    assert_eq!(paced.lead_ns, plain.lead_ns - plain.spread_ns as i32 / 4);
    assert_eq!((paced.frames, paced.spread_ns, paced.period_ns), (plain.frames, plain.spread_ns, plain.period_ns));
    // The same frames on a 120 Hz screen are one per two refreshes: a miss costs no frame.
    fill(&mut window, 8_333_333);
    let half = window.report_over(10, 500_000_000).unwrap();
    fill(&mut window, 8_333_333);
    assert_eq!(half, window.report(10).unwrap());
    // Unknown elapsed time: the plain report.
    fill(&mut window, 16_666_667);
    assert_eq!(window.report_over(10, 0).unwrap(), plain);
}

const SIXTY: i64 = 16_666_667; // 60.00 Hz on the client.

/// A frame the host sent in a [`retroid`] run: when it claimed the frame and the frame's
/// slack at the client, both in client nanoseconds.
struct Sent {
    at_ns: i64,
    slack_ns: i64,
}

/// A 60 fps stream to a 60.000 Hz screen (Retroid Pocket Flip 2) from a host whose clock runs
/// `ppm` fast, for `seconds`. The host claims frames like its arrival pacer: the first frame of
/// its 240 Hz desktop once 7/8 of a frame of credit has built up at the locked interval, at
/// least 3/4 of an interval after the last claim, with at most 1.5 frames banked. In the
/// seconds of `busy` each claim comes a further 5 to 15 ms late, like a host busy with a build.
/// Frames are ready 20 ms plus up to 3 ms after the claim; the client reports every half
/// second with its elapsed time, and a report reaches the host 3 frames later.
fn retroid(lock: &mut PhaseLock, ppm: i64, busy: core::ops::Range<i64>, seconds: i64, seed: u64) -> Vec<Sent> {
    const DESKTOP: i64 = 4_166_667;
    let mut rng = Rng(seed);
    let mut window = SlackWindow::default();
    let mut in_flight: Vec<(usize, Report)> = Vec::new();
    let (mut credit, mut credit_at, mut last_claim) = (1.5f64, 0i64, i64::MIN / 2);
    let mut last_report: Option<i64> = None;
    let mut sent = Vec::new();
    loop {
        let interval = lock.next_interval_ns(SIXTY);
        let funded = credit_at + ((0.875 - credit).max(0.0) * interval as f64) as i64;
        let allowed = funded.max(last_claim + interval * 3 / 4);
        let mut claim = (allowed + DESKTOP - 1) / DESKTOP * DESKTOP;
        if claim >= seconds * 1_000_000_000 {
            return sent;
        }
        if busy.contains(&(claim / 1_000_000_000)) {
            claim += 5_000_000 + (rng.next() % 10_000_001) as i64;
        }
        credit = ((credit + (claim - credit_at) as f64 / interval as f64).min(1.5) - 1.0).max(-1.0);
        credit_at = claim;
        last_claim = claim;
        let at_ns = claim + claim * ppm / 1_000_000;
        let ready = at_ns + 20_000_000 + (rng.next() % 3_000_001) as i64;
        let slack_ns = (5_000_000 - ready).rem_euclid(SIXTY);
        sent.push(Sent { at_ns, slack_ns });
        window.push(slack_ns, SIXTY);
        let since = *last_report.get_or_insert(ready);
        if ready - since >= 500_000_000 {
            last_report = Some(ready);
            if let Some(report) = window.report_over(10, ready - since) {
                in_flight.push((sent.len() + 3, report));
            }
        }
        while let Some(&(due, report)) = in_flight.first() {
            if due > sent.len() {
                break;
            }
            in_flight.remove(0);
            lock.on_report(&report);
        }
    }
}

/// The frames sent in `seconds` of client time: their mean interval and median slack.
fn settled(sent: &[Sent], seconds: core::ops::Range<i64>) -> (i64, i64) {
    let range = seconds.start * 1_000_000_000..seconds.end * 1_000_000_000;
    let frames: Vec<&Sent> = sent.iter().filter(|frame| range.contains(&frame.at_ns)).collect();
    let interval = (frames[frames.len() - 1].at_ns - frames[0].at_ns) / (frames.len() as i64 - 1);
    let mut slack: Vec<i64> = frames.iter().map(|frame| frame.slack_ns).collect();
    (interval, percentile(&mut slack, 50))
}

#[test]
fn the_lock_recovers_after_the_host_was_busy() {
    // 20 s of late claims (a build on the host PC) sweep the frames around the client's refresh.
    // The trim used to run to its limit meanwhile and stay there once the host was free: it
    // sent at 60.5 to 61 fps, and frames waited 8 to 11 ms for their latch instead of about 4.
    for ppm in [100, 4_000, -4_000] {
        for seed in 1..=3 {
            let mut lock = PhaseLock::default();
            let sent = retroid(&mut lock, ppm, 10..30, 45, seed);
            // Five seconds after the host is free again: the screen's rate, frames just in time.
            let (interval, median) = settled(&sent, 35..45);
            assert!((interval - SIXTY).abs() < 20_000, "ppm {ppm} seed {seed}: interval {interval}");
            assert!(median < 6_500_000, "ppm {ppm} seed {seed}: median slack {median}");
        }
    }
}

#[test]
fn the_lock_recovers_from_a_trim_at_its_limit() {
    // The state the field runs stayed in: the trim at a hundredth of a refresh per frame.
    for (ppm, trim) in [(100, -SIXTY / 100), (100, SIXTY / 100), (4_000, -SIXTY / 100), (-4_000, SIXTY / 100)] {
        for seed in 1..=3 {
            let mut lock = PhaseLock::default();
            lock.on_report(&Report { frames: 30, period_ns: SIXTY as u32, lead_ns: 0, spread_ns: 0 });
            (lock.pending_ns, lock.trim_ns) = (0, trim);
            let sent = retroid(&mut lock, ppm, 0..0, 20, seed);
            let (interval, median) = settled(&sent, 3..20);
            assert!((interval - SIXTY).abs() < 20_000, "ppm {ppm} trim {trim} seed {seed}: interval {interval}");
            assert!(median < 6_500_000, "ppm {ppm} trim {trim} seed {seed}: median slack {median}");
        }
    }
}

#[test]
fn only_a_jump_past_half_a_refresh_starts_the_trim_over() {
    let report = |error: i64| Report {
        frames: 30,
        period_ns: SIXTY as u32,
        lead_ns: (PhaseLock::DEFAULT_MARGIN_NS + error) as i32,
        spread_ns: 0,
    };
    let mut lock = PhaseLock::default();
    lock.on_report(&report(0));
    // Errors within half a refresh (8.33 ms) of the last one build the trim as always.
    let mut trim = 0;
    for error in [3_000_000, -5_000_000, 3_300_000, -4_000_000] {
        lock.on_report(&report(error));
        trim += error / 30 / 16;
        assert_eq!(lock.trim_ns, trim, "error {error}");
    }
    // Further means the frames went around the refresh: the trim starts over, while the
    // phase correction still applies.
    lock.on_report(&report(4_400_000));
    assert_eq!((lock.trim_ns, lock.pending_ns), (0, 2_200_000));
    lock.on_report(&report(1_000_000));
    assert_eq!(lock.trim_ns, 1_000_000 / 30 / 16);
}

#[test]
fn the_extra_room_keeps_a_wide_cluster_inside_half_a_refresh() {
    // A spread of 7 ms on a 60 Hz screen leaves no room: the cluster would cross the wrap.
    let mut window = SlackWindow::default();
    for i in 0..30 {
        window.push(1_000_000 + (i % 10) * 900_000, 16_666_667);
    }
    let plain = window.clone().report(10).unwrap();
    let paced = window.report_over(10, 500_000_000).unwrap();
    assert!(plain.spread_ns >= 7_000_000);
    assert_eq!(paced.lead_ns, plain.lead_ns);
}
