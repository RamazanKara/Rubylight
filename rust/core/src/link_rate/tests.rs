use std::time::{Duration, Instant};

use super::sim::*;
use super::*;

const PERIOD_MS: u64 = 17;
const PACKETS: u32 = 100;
const WIRE_BYTES: u32 = 112_600;

fn stream(kbps: u32) -> Negotiated {
    Negotiated {
        bitrate_kbps: kbps,
        ..Default::default()
    }
}

fn status(frame: u32, received: u16, parity_lost: u16) -> Status {
    Status {
        frame,
        highest_received: 0,
        next_contiguous: 0,
        missing_before_highest: 0,
        data: 80,
        parity: 16,
        received_data: received,
        received_parity: 16 - parity_lost,
        percentage: 20,
        block: 0,
        blocks: 1,
    }
}

/// A frame the client could not rebuild.
fn unrecoverable(lr: &mut LinkRate, frame: u32, now: Instant) {
    lr.on_status(&status(frame, 60, 0), now);
    lr.on_unrecoverable(frame, now);
}

/// A frame that lost `missing` packets, all of them repaired by FEC.
fn repaired(lr: &mut LinkRate, frame: u32, missing: u16, now: Instant) {
    lr.on_status(&status(frame, 80, missing), now);
}

/// A 60 fps host: one frame every 17 ms, a tick every 8 ms and a ping every
/// 100 ms, one millisecond at a time.
struct Rig {
    lr: LinkRate,
    base: Instant,
    ms: u64,
    frame: u32,
    kbps: u32,
    rtt: Duration,
    changes: Vec<(u64, Change)>,
}

impl Rig {
    fn new(kbps: u32) -> Self {
        Self::with(LinkRate::new(true, &stream(kbps), 0, 20), kbps)
    }
    fn with(lr: LinkRate, kbps: u32) -> Self {
        Self {
            lr,
            base: Instant::now(),
            ms: 0,
            frame: 0,
            kbps,
            rtt: Duration::from_millis(20),
            changes: Vec::new(),
        }
    }
    fn now(&self) -> Instant {
        self.base + Duration::from_millis(self.ms)
    }
    /// Runs `ms` milliseconds; `each` sees every frame right after it is sent.
    fn run(&mut self, ms: u64, mut each: impl FnMut(&mut LinkRate, u32, Instant)) {
        for _ in 0..ms {
            self.ms += 1;
            let now = self.now();
            if self.ms.is_multiple_of(PERIOD_MS) {
                self.frame += 1;
                self.lr.on_sent(self.frame, PACKETS, WIRE_BYTES, now);
                each(&mut self.lr, self.frame, now);
            }
            if self.ms.is_multiple_of(100) {
                self.lr.on_ping(now, &[4, 0, 0, 0, 0, 0, 0, 0]);
            }
            if self.ms.is_multiple_of(8)
                && let Some(kbps) = self.lr.tick(
                    now,
                    self.rtt,
                    20,
                    Duration::from_micros(16_667),
                    self.kbps,
                    true,
                )
            {
                self.kbps = kbps;
            }
            if let Some(change) = self.lr.take_change() {
                self.changes.push((self.ms, change));
            }
        }
    }
    fn clean(&mut self, ms: u64) {
        self.run(ms, |_, _, _| {});
    }
    /// Three unrecoverable frames in a row.
    fn lose_three(&mut self) {
        let mut left = 3;
        self.run(PERIOD_MS * 3 + 1, |lr, frame, now| {
            if left > 0 {
                left -= 1;
                unrecoverable(lr, frame, now);
            }
        });
    }
    fn kinds(&self) -> Vec<ChangeKind> {
        self.changes.iter().map(|(_, c)| c.kind).collect()
    }
    fn cuts(&self) -> usize {
        self.kinds()
            .iter()
            .filter(|k| **k == ChangeKind::StepDown)
            .count()
    }
}

// ---- unit tests ----

#[test]
fn a_healthy_link_gets_no_target_and_no_change_in_a_minute() {
    let mut rig = Rig::new(40_000);
    // Recovered loss well under the threshold, including a packet or two per
    // frame, is what a good Wi-Fi link looks like.
    rig.run(60_000, |lr, frame, now| {
        if frame % 7 == 0 {
            repaired(lr, frame, 2, now);
        }
    });
    assert!(rig.changes.is_empty());
    assert_eq!(rig.kbps, 40_000);
    assert_eq!(rig.lr.state(), State::Full);
    assert_eq!(
        rig.lr.tick(
            rig.now(),
            rig.rtt,
            20,
            Duration::from_micros(16_667),
            40_000,
            true
        ),
        None
    );
}

#[test]
fn three_unrecoverable_frames_cut_once_to_between_half_and_85_percent() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(500);
    assert_eq!(rig.cuts(), 1);
    let (_, cut) = rig.changes[0];
    assert_eq!(cut.reason, "unrecoverable_frames");
    assert_eq!(cut.from_kbps, 40_000);
    assert!(cut.to_kbps < 40_000 * 85 / 100, "{cut:?}");
    assert!(cut.to_kbps >= 40_000 * 40 / 100, "{cut:?}");
    assert_eq!(rig.kbps, cut.to_kbps);
    assert_eq!(rig.lr.state(), State::Limited);
    assert_eq!(cut.evidence.unrecoverable_frames, 3);
}

#[test]
fn two_unrecoverable_frames_or_three_spread_over_time_do_not_cut() {
    let mut rig = Rig::new(40_000);
    let mut left = 2;
    rig.run(1_000, |lr, frame, now| {
        if left > 0 {
            left -= 1;
            unrecoverable(lr, frame, now);
        }
    });
    rig.clean(5_000);
    // Three, but one every 2.5 s.
    for _ in 0..3 {
        let mut once = true;
        rig.run(2_500, |lr, frame, now| {
            if std::mem::take(&mut once) {
                unrecoverable(lr, frame, now);
            }
        });
    }
    assert!(rig.changes.is_empty(), "{:?}", rig.changes);
}

#[test]
fn the_same_frame_reported_in_several_blocks_counts_once() {
    let mut rig = Rig::new(40_000);
    rig.run(500, |lr, frame, now| {
        for _ in 0..5 {
            unrecoverable(lr, frame.min(1), now);
        }
    });
    assert!(rig.changes.is_empty());
}

#[test]
fn recovered_loss_under_five_percent_never_cuts_but_five_in_two_windows_does() {
    let mut rig = Rig::new(40_000);
    rig.run(20_000, |lr, frame, now| repaired(lr, frame, 4, now));
    assert!(rig.changes.is_empty(), "4 % is repaired by FEC");

    let mut rig = Rig::new(40_000);
    // 6 % for one second: two full windows.
    rig.run(1_400, |lr, frame, now| repaired(lr, frame, 6, now));
    rig.clean(500);
    assert_eq!(rig.cuts(), 1, "{:?}", rig.changes);
    assert_eq!(rig.changes[0].1.reason, "fec_loss_share");
    assert!(rig.changes[0].1.evidence.loss_share >= 0.05);
}

#[test]
fn a_five_percent_burst_in_one_window_does_not_cut() {
    let mut rig = Rig::new(40_000);
    rig.clean(1_000);
    // One window of loss, then clean traffic: the second window resets it.
    rig.run(450, |lr, frame, now| repaired(lr, frame, 8, now));
    rig.clean(5_000);
    assert!(rig.changes.is_empty(), "{:?}", rig.changes);
}

#[test]
fn a_quiet_stream_cannot_cut_on_a_single_lost_packet() {
    let mut rig = Rig::new(40_000);
    // Under LOSS_MIN_PACKETS sent per window.
    for _ in 0..4 {
        let now = rig.now();
        rig.lr.on_sent(rig.frame + 1, 10, 11_000, now);
        rig.frame += 1;
        repaired(&mut rig.lr, rig.frame, 6, now);
        rig.clean(500);
    }
    assert!(rig.changes.is_empty());
}

#[test]
fn three_frames_lost_on_send_cut() {
    let mut rig = Rig::new(40_000);
    rig.clean(1_000);
    let mut left = 3;
    rig.run(200, |lr, frame, now| {
        if left > 0 {
            left -= 1;
            lr.on_send_loss(frame, now);
        }
    });
    rig.clean(300);
    assert_eq!(rig.cuts(), 1);
    assert_eq!(rig.changes[0].1.reason, "send_losses");
}

#[test]
fn network_keyframe_requests_cut_only_without_fec_status() {
    // A client that sends no FEC status: three network requests cut.
    let mut rig = Rig::new(40_000);
    rig.clean(1_000);
    for _ in 0..3 {
        rig.lr.on_keyframe_request(true, rig.now());
        rig.clean(100);
    }
    rig.clean(400);
    assert_eq!(rig.cuts(), 1);
    let (_, cut) = rig.changes[0];
    assert_eq!(cut.reason, "keyframe_requests");
    // No share is known: 70 % of the wire rate, less the audio reserve.
    assert!(
        cut.to_kbps < 40_000 * 75 / 100 && cut.to_kbps > 40_000 / 3,
        "{cut:?}"
    );

    // The decoder asked, not the network.
    let mut rig = Rig::new(40_000);
    rig.clean(1_000);
    for _ in 0..6 {
        rig.lr.on_keyframe_request(false, rig.now());
        rig.clean(100);
    }
    assert!(rig.changes.is_empty());

    // A client that reports FEC status is judged by it, not by its requests.
    let mut rig = Rig::new(40_000);
    rig.run(300, |lr, frame, now| repaired(lr, frame, 1, now));
    for _ in 0..6 {
        rig.lr.on_keyframe_request(true, rig.now());
        rig.clean(100);
    }
    rig.clean(1_000);
    assert!(rig.changes.is_empty());
}

#[test]
fn a_rise_in_rtt_alone_is_no_evidence() {
    let mut rig = Rig::new(40_000);
    rig.clean(3_000);
    rig.rtt = Duration::from_millis(400);
    rig.clean(20_000);
    assert!(rig.changes.is_empty());
    assert_eq!(rig.kbps, 40_000);
}

#[test]
fn reports_for_frames_sent_before_a_cut_took_effect_are_ignored() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(150);
    assert_eq!(rig.cuts(), 1);
    // Late reports for frames sent at the old rate (the queue is still
    // draining): plenty of them, and none counts.
    let last = rig.frame;
    let now = rig.now();
    for old in last - 40..last - 10 {
        unrecoverable(&mut rig.lr, old, now);
    }
    rig.clean(1_500);
    assert_eq!(rig.cuts(), 1, "{:?}", rig.changes);
    // Frames sent after the cut do count: a second cut follows.
    rig.lose_three();
    rig.clean(500);
    assert_eq!(rig.cuts(), 2, "{:?}", rig.changes);
}

#[test]
fn a_second_cut_is_never_below_half_the_first() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(1_000);
    let first = rig.kbps;
    rig.lose_three();
    rig.clean(500);
    assert_eq!(rig.cuts(), 2);
    assert!(rig.kbps >= first * 40 / 100, "{first} -> {}", rig.kbps);
}

#[test]
fn it_probes_back_up_in_small_steps_and_returns_to_full() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(60_000);
    let probes: Vec<_> = rig
        .changes
        .iter()
        .filter(|(_, c)| c.kind == ChangeKind::Probe)
        .collect();
    assert!(!probes.is_empty());
    let cut_at = rig.changes[0].0;
    assert!(probes[0].0 >= cut_at + 2_000, "first probe waits the hold");
    for (_, p) in &probes {
        // One step is about 12.5 % (5 % near the failing rate), never 25 %.
        assert!(p.to_kbps > p.from_kbps, "{p:?}");
        assert!(p.to_kbps * 100 <= p.from_kbps * 125, "{p:?}");
    }
    assert!(rig.kinds().contains(&ChangeKind::Full), "{:?}", rig.kinds());
    assert_eq!(rig.lr.state(), State::Full);
    assert_eq!(rig.kbps, 40_000, "Full goes back to the requested rate");
}

#[test]
fn a_failed_probe_returns_to_the_last_good_rate_and_waits_twice_as_long() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    let mut probed = None;
    for _ in 0..200 {
        rig.clean(100);
        if let Some((at, c)) = rig
            .changes
            .iter()
            .find(|(_, c)| c.kind == ChangeKind::Probe)
        {
            probed = Some((*at, *c));
            break;
        }
    }
    let (probe_at, probe) = probed.expect("a probe");
    rig.lose_three();
    rig.clean(400);
    let failed = rig
        .changes
        .iter()
        .find(|(_, c)| c.kind == ChangeKind::ProbeFailed)
        .copied()
        .expect("probe failed");
    assert_eq!(failed.1.to_kbps, probe.from_kbps, "back to the good rate");
    assert_eq!(rig.kbps, probe.from_kbps);
    assert_eq!(rig.cuts(), 1, "a failed probe is not another cut");
    // The wait doubled from 2 s to 4 s.
    rig.clean(30_000);
    let next = rig
        .changes
        .iter()
        .find(|(at, c)| *at > failed.0 && c.kind == ChangeKind::Probe)
        .expect("probes again");
    assert!(next.0 >= failed.0 + 4_000, "{} after {}", next.0, failed.0);
    assert!(probe_at < failed.0);
}

#[test]
fn it_does_not_probe_while_the_rtt_is_elevated() {
    let mut rig = Rig::new(40_000);
    rig.clean(3_000);
    rig.lose_three();
    rig.clean(500);
    rig.rtt = Duration::from_millis(300);
    rig.clean(15_000);
    assert!(
        rig.kinds().iter().all(|k| *k == ChangeKind::StepDown),
        "{:?}",
        rig.kinds()
    );
    rig.rtt = Duration::from_millis(20);
    rig.clean(5_000);
    assert!(rig.kinds().contains(&ChangeKind::Probe));
}

#[test]
fn it_never_goes_below_the_floor_and_says_so() {
    let mut rig = Rig::new(4_000);
    rig.clean(1_000);
    let mut lowest = rig.kbps;
    for _ in 0..12 {
        rig.lose_three();
        rig.clean(1_200);
        assert!(rig.kbps >= 2_000, "{}", rig.kbps);
        lowest = lowest.min(rig.kbps);
    }
    // The wire allowance for 2 Mbit/s is whole packets per frame.
    assert!((2_000..=2_500).contains(&lowest), "{lowest}");
    assert!(
        rig.kinds().contains(&ChangeKind::Floor),
        "{:?}",
        rig.kinds()
    );
}

#[test]
fn it_is_off_when_disabled_for_pyrowave_and_after_the_client_sets_the_bitrate() {
    let disabled = LinkRate::new(false, &stream(40_000), 0, 20);
    assert_eq!(disabled.state(), State::Off);
    let pyrowave = LinkRate::new(
        true,
        &Negotiated {
            codec: 3,
            ..stream(40_000)
        },
        0,
        20,
    );
    assert_eq!(pyrowave.state(), State::Off);
    assert_eq!(pyrowave.snapshot(Instant::now())["reason"], "pyrowave");
    for lr in [disabled, pyrowave] {
        let mut rig = Rig::with(lr, 40_000);
        rig.clean(1_000);
        rig.lose_three();
        rig.clean(2_000);
        assert!(rig.changes.is_empty());
        assert_eq!(rig.kbps, 40_000);
    }

    let mut rig = Rig::new(40_000);
    rig.clean(1_000);
    rig.lr.client_takeover();
    rig.lose_three();
    rig.clean(2_000);
    assert!(rig.changes.is_empty());
    assert_eq!(rig.lr.state(), State::Off);
    assert_eq!(rig.lr.snapshot(rig.now())["reason"], "client_controlled");
}

#[test]
fn max_bitrate_lowers_the_ceiling() {
    let lr = LinkRate::new(true, &stream(80_000), 30_000, 20);
    assert_eq!(lr.ceiling_kbps(), 30_000);
    let mut rig = Rig::with(lr, 30_000);
    rig.clean(1_000);
    rig.lose_three();
    rig.clean(500);
    assert!(rig.kbps < 30_000);
}

#[test]
fn a_latch_that_fixes_the_loss_gets_one_fence_before_a_cut() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    // Two unrecoverable frames, then the loss guard latches and slows the
    // pacing: the reports in flight from before it must not cut.
    let mut left = 2;
    rig.run(PERIOD_MS * 2 + 1, |lr, frame, now| {
        if left > 0 {
            left -= 1;
            unrecoverable(lr, frame, now);
        }
    });
    rig.lr.on_latch(rig.now(), true);
    let late = rig.frame;
    let now = rig.now();
    unrecoverable(&mut rig.lr, late, now);
    rig.clean(2_000);
    assert!(rig.changes.is_empty(), "{:?}", rig.changes);
    // Loss that continues after the fence is the network and cuts.
    rig.lose_three();
    rig.clean(500);
    assert_eq!(rig.cuts(), 1);
}

#[test]
fn the_snapshot_reports_state_and_counters() {
    let mut rig = Rig::new(40_000);
    let before = rig.lr.snapshot(rig.now());
    assert_eq!(before["state"], "full");
    assert_eq!(before["ceiling_kbps"], 40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(500);
    let after = rig.lr.snapshot(rig.now());
    assert_eq!(after["state"], "limited");
    assert_eq!(after["reason"], "unrecoverable_frames");
    assert_eq!(after["steps_down"], 1);
    assert_eq!(after["current_kbps"], rig.kbps);
}

// ---- link simulation ----

fn q(mut link: Link, ms: f64) -> Link {
    link.queue_ms = ms;
    link
}

fn lossy(mut link: Link, share: f64) -> Link {
    link.random_loss = share;
    link
}

#[test]
fn a_stream_above_its_link_settles_near_the_link_rate_with_little_loss() {
    // (link Mbit/s, requested kbps)
    for (link, kbps) in [
        (150., 225_000),
        (80., 120_000),
        (20., 40_000),
        (20., 20_000),
    ] {
        for feedback in [2., 20., 80.] {
            for (queue, spread) in [(50., 1.), (5., 1.), (50., 0.3)] {
                let mut sim = Sim::new(q(Link::constant(link), queue), kbps, feedback);
                sim.spread = spread;
                sim.run(75.);
                let what =
                    format!("{link} Mbit/s, {kbps} kbps, fb {feedback}, q {queue}, sp {spread}");
                let wire = sim.mean_wire_bps(55., 75.) / 1e6;
                assert!(wire >= link * 0.65, "{what}: {wire:.1} Mbit/s");
                assert!(wire <= link * 1.05, "{what}: {wire:.1} Mbit/s");
                assert!(
                    sim.frame_loss(3., 75.) < 0.02,
                    "{what}: {}",
                    sim.frame_loss(3., 75.)
                );
                assert!(
                    sim.longest_gap(3., 75.) < 0.5,
                    "{what}: {}",
                    sim.longest_gap(3., 75.)
                );
            }
        }
    }
}

#[test]
fn a_big_overshoot_is_one_cut_not_a_cascade() {
    // The 2026-10-23 link-speed run: 179 Mbit/s requested on a link that
    // carries about 115, which the opt-in controller cut to 24.7 in nine seconds.
    for feedback in [2., 20., 80.] {
        let mut sim = Sim::new(Link::constant(138.), 179_000, feedback);
        sim.run(75.);
        let early = sim
            .changes
            .iter()
            .filter(|(t, c)| *t < 10. && c.kind == ChangeKind::StepDown)
            .count();
        assert_eq!(early, 1, "fb {feedback}: {:?}", sim.changes.first());
        let lowest = sim.kbps_log.iter().skip(2).map(|(_, k)| *k).min().unwrap();
        assert!(lowest >= 60_000, "fb {feedback}: fell to {lowest} kbps");
        assert!(sim.mean_kbps(55., 75.) >= 80_000., "fb {feedback}");
        assert!(sim.frame_loss(3., 75.) < 0.02, "fb {feedback}");
    }
}

#[test]
fn a_link_that_drops_to_an_eighth_recovers_within_twenty_seconds() {
    // 80 -> 10 -> 80 Mbit/s at a 40 Mbit/s request; the link returns at 40 s.
    for (feedback, queue, spread) in [
        (2., 50., 1.),
        (20., 50., 1.),
        (20., 5., 1.),
        (20., 50., 0.3),
        (80., 50., 1.),
    ] {
        let mut sim = Sim::new(q(Link::step(80., 10., 20., 40.), queue), 40_000, feedback);
        sim.spread = spread;
        sim.run(75.);
        let what = format!("fb {feedback}, q {queue}, sp {spread}");
        assert!(
            sim.longest_gap(19., 45.) <= 1.5,
            "{what}: gap {}",
            sim.longest_gap(19., 45.)
        );
        assert!(
            sim.frame_loss(3., 75.) <= 0.03,
            "{what}: loss {}",
            sim.frame_loss(3., 75.)
        );
        let back = sim.reaches(40., 36_000).expect("back to 90 %");
        assert!(back <= 60., "{what}: 90 % at {back:.1} s");
        assert!(sim.cuts() <= 3, "{what}: {} cuts", sim.cuts());
    }
}

#[test]
fn a_link_that_drops_to_a_tenth_recovers_within_twenty_seconds() {
    for feedback in [2., 20., 80.] {
        let mut sim = Sim::new(Link::step(300., 30., 20., 40.), 40_000, feedback);
        sim.run(75.);
        let what = format!("fb {feedback}");
        assert!(
            sim.longest_gap(19., 45.) <= 1.5,
            "{what}: gap {}",
            sim.longest_gap(19., 45.)
        );
        assert!(
            sim.frame_loss(3., 75.) <= 0.03,
            "{what}: loss {}",
            sim.frame_loss(3., 75.)
        );
        let back = sim.reaches(40., 36_000).expect("back to 90 %");
        assert!(back <= 60., "{what}: 90 % at {back:.1} s");
        assert_eq!(sim.cuts(), 1, "{what}");
    }
}

#[test]
fn a_healthy_link_never_changes_anything() {
    type Make = fn() -> Link;
    let links: [(&str, Make, u32); 8] = [
        ("1 GbE 20", || Link::constant(1000.), 20_000),
        ("1 GbE 40", || Link::constant(1000.), 40_000),
        ("1 GbE 80", || Link::constant(1000.), 80_000),
        ("150 Mbit/s 80", || Link::constant(150.), 80_000),
        ("300 Mbit/s 40", || Link::constant(300.), 40_000),
        // Wi-Fi: 0.5 % and 1 % random loss, repaired by FEC.
        ("wifi 0.5 %", || lossy(Link::constant(1000.), 0.005), 40_000),
        ("wifi-hi 1 %", || lossy(Link::constant(1000.), 0.01), 80_000),
        // A dip the stream never feels.
        (
            "dip at 20 Mbit/s",
            || Link::step(300., 30., 20., 40.),
            20_000,
        ),
    ];
    for (name, make, kbps) in links {
        for feedback in [2., 20., 80.] {
            let mut sim = Sim::new(make(), kbps, feedback);
            sim.run(75.);
            assert!(
                sim.changes.is_empty(),
                "{name} fb {feedback}: {:?}",
                sim.changes.first()
            );
            assert_eq!(sim.frame_loss(0., 75.), 0., "{name} fb {feedback}");
            assert!(sim.kbps_log.iter().all(|(_, k)| *k == kbps), "{name}");
        }
    }
}
