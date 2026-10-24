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
    /// False while the picture is static.
    busy: bool,
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
            busy: true,
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
                self.frame = self.frame.wrapping_add(1);
                // The encoder follows its bitrate, so the frames shrink with it.
                let bytes = (u64::from(WIRE_BYTES) * u64::from(self.kbps) / 40_000) as u32;
                let packets = (u64::from(PACKETS) * u64::from(self.kbps) / 40_000).max(1) as u32;
                self.lr.on_sent(self.frame, packets, bytes, now);
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
                    self.busy,
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
fn a_report_before_the_frame_is_recorded_counts_nothing_and_one_after_does() {
    // The host records a frame once it has sent it (see the module notes).
    let now = Instant::now();
    let mut lr = LinkRate::new(true, &stream(40_000), 0, 20);
    let early = status(7, 5, 0);
    lr.on_status(&early, now);
    assert_eq!(lr.events.missing, 0);
    lr.on_sent(7, PACKETS, WIRE_BYTES, now);
    lr.on_status(&early, now);
    assert!(lr.events.missing > 0);
    assert_eq!(lr.sent(7).unwrap().missing, lr.events.missing as u32);
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
fn recovered_loss_never_cuts_however_many_packets_it_lacked() {
    // A block is reported the moment `data` packets are in, so the parity still
    // in flight reads as missing; FEC rebuilt every one of these frames.
    for lost_parity in [4, 8, 16] {
        let mut rig = Rig::new(40_000);
        rig.run(20_000, |lr, frame, now| {
            repaired(lr, frame, lost_parity, now)
        });
        assert!(rig.changes.is_empty(), "{lost_parity}: {:?}", rig.changes);
        assert_eq!(rig.kbps, 40_000);
    }
}

#[test]
fn an_occasional_failed_frame_is_not_a_cut() {
    // One frame in sixty fails beyond repair: under three in two seconds, and
    // under 5 % of the packets sent.
    let mut rig = Rig::new(40_000);
    let mut n = 0;
    rig.run(20_000, |lr, frame, now| {
        n += 1;
        if n % 60 == 0 {
            unrecoverable(lr, frame, now);
        }
    });
    assert!(rig.changes.is_empty(), "{:?}", rig.changes);
}

/// The 2026-10-24 `wifi` arm (150 Mbit/s, 3 +/- 2 ms delay, 0.5 % loss, HEVC
/// 1080p60 at 14,988 kbps, 20 % FEC) from `host-samples.jsonl`: about 38 packets
/// a frame (2,370 in the first 62 frames, 39 with 20 % parity). The delay jitter reorders packets
/// within a frame, so the client decodes each block when its 32nd packet is in
/// and reports it with the rest still on the way; a few percent of the frames
/// also truly lost a packet. The old controller read 18 % missing and cut to
/// the 2,415 kbps floor within seven seconds (15 changes).
#[test]
fn the_wifi_arm_with_reordering_and_random_loss_never_cuts() {
    let mut rig = Rig::new(14_988);
    let mut n = 0u32;
    rig.run(45_000, |lr, frame, now| {
        n += 1;
        // Data 32 + parity 7. The block is rebuilt when 32 of the 39 packets
        // are in: `late` of them parity-first, i.e. data still on the way.
        let late = 1 + (n * 7 % 7) as u16;
        lr.on_status(
            &Status {
                frame,
                highest_received: 38,
                next_contiguous: 0,
                missing_before_highest: late,
                data: 32,
                parity: 7,
                received_data: 32 - late,
                received_parity: late,
                percentage: 20,
                block: 0,
                blocks: 1,
            },
            now,
        );
    });
    assert!(rig.changes.is_empty(), "{:?}", rig.changes);
    assert_eq!(rig.kbps, 14_988);
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
fn a_second_cut_right_after_the_first_may_go_below_half() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(1_000);
    let first = rig.kbps;
    // The first cut went down by at most half (40 -> 19 Mbit/s); the link
    // carries far less, so the next one inside the window is not held to
    // half of that again.
    assert!(first >= 40_000 * 40 / 100, "{first}");
    rig.lose_three();
    rig.clean(500);
    assert_eq!(rig.cuts(), 2);
    assert!(rig.kbps < first / 2, "{first} -> {}", rig.kbps);
    assert!(rig.kbps >= 1_000, "{}", rig.kbps);
}

#[test]
fn a_cut_after_a_quiet_window_is_held_to_half_again() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(1_000);
    let first = rig.kbps;
    // The previous cut is older than the evidence window: an isolated cut.
    rig.lr.last_cut = rig.lr.last_cut.map(|at| at - Duration::from_secs(3));
    rig.lose_three();
    rig.clean(500);
    assert_eq!(rig.cuts(), 2);
    assert!(rig.kbps >= first * 49 / 100, "{first} -> {}", rig.kbps);
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
    // The floor line says what the link delivered, not 0 (2026-10-24 logs).
    let floor = rig
        .changes
        .iter()
        .find(|(_, c)| c.kind == ChangeKind::Floor);
    let (_, floor) = floor.expect("a floor change");
    assert!(floor.delivered_kbps > 0, "{floor:?}");
    assert!(floor.evidence.loss_share > 0., "{floor:?}");
    let probe = rig
        .changes
        .iter()
        .find(|(_, c)| c.kind == ChangeKind::Probe);
    if let Some((_, probe)) = probe {
        assert!(probe.delivered_kbps > 0, "{probe:?}");
    }
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
    rig.lr.on_latch(rig.now(), true, true);
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

/// The 2026-10-24 `r50-hevc-hi` arm: 75 Mbps requested (58,988 kbps encoder) into
/// 50 Mbit/s. The start-up keyframes are lost, the loss guard latches at 0.8 s
/// from the start, and the client, waiting for a keyframe, reports only the
/// keyframes it asked for, about one every 1.2 to 2 s. Three frames took until
/// 4.9 s and the stream lost its first 5.8 s; once the latch has had its
/// fence, one is enough, and the cut comes with the first report after it.
#[test]
fn a_latch_that_did_not_help_cuts_on_the_first_unrecoverable_frame_after_it() {
    let mut rig = Rig::new(58_988);
    rig.clean(780);
    rig.lr.on_latch(rig.now(), true, true);
    let base = rig.base;
    // Keyframes at 0.9 s (inside the fence), 2.1 s and 4.1 s, each reported
    // about 100 ms after it was sent.
    let mut reports = vec![1_000u64, 2_200, 4_200];
    rig.run(3_000, |lr, frame, now| {
        let at = now.duration_since(base).as_millis() as u64;
        if reports.first().is_some_and(|r| at >= *r) {
            reports.remove(0);
            unrecoverable(lr, frame, now);
        }
    });
    assert_eq!(rig.cuts(), 1, "{:?}", rig.changes);
    let (at, change) = &rig.changes[0];
    assert!(*at < 2_500, "cut at {at} ms");
    assert_eq!(change.reason, "unrecoverable_frames");
}

#[test]
fn a_stray_unrecoverable_frame_long_after_a_latch_is_not_a_cut() {
    let mut rig = Rig::new(40_000);
    rig.clean(1_000);
    rig.lr.on_latch(rig.now(), true, true);
    // The fence and the watch both over: one frame is no longer a cut.
    rig.clean(500 + LATCH_WATCH.as_millis() as u64 + 500);
    let mut left = 1;
    rig.run(PERIOD_MS * 2, |lr, frame, now| {
        if left > 0 {
            left -= 1;
            unrecoverable(lr, frame, now);
        }
    });
    rig.clean(2_000);
    assert!(rig.changes.is_empty(), "{:?}", rig.changes);
}

#[test]
fn a_latch_that_is_not_new_or_was_not_fast_starts_no_watch() {
    let mut rig = Rig::new(40_000);
    rig.clean(1_000);
    rig.lr.on_latch(rig.now(), false, true);
    rig.lr.on_latch(rig.now(), true, false);
    rig.clean(1_000);
    let mut left = 1;
    rig.run(PERIOD_MS * 2, |lr, frame, now| {
        if left > 0 {
            left -= 1;
            unrecoverable(lr, frame, now);
        }
    });
    rig.clean(2_000);
    assert!(rig.changes.is_empty(), "{:?}", rig.changes);
}

#[test]
fn a_lifted_latch_fences_only_when_the_pace_gets_faster() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    // Back at a rate no faster than the Wi-Fi rate: nothing changed.
    rig.lr.on_unlatch(rig.now(), false);
    assert!(rig.lr.fence_at().is_none());
    // Back at the faster rate: one fence, so reports from before it decide
    // nothing.
    rig.lr.on_unlatch(rig.now(), true);
    assert!(rig.lr.fence_at().is_some());
    let late = rig.frame;
    let now = rig.now();
    unrecoverable(&mut rig.lr, late, now);
    rig.clean(2_000);
    assert!(rig.changes.is_empty(), "{:?}", rig.changes);
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

// ---- probing, confirmation and the knee ----

const PERIOD: Duration = Duration::from_micros(16_667);

impl Rig {
    fn probes(&self) -> usize {
        self.changes
            .iter()
            .filter(|(_, c)| c.kind == ChangeKind::Probe)
            .count()
    }
    /// Runs until one more probe was issued; returns it.
    fn until_probe(&mut self) -> Change {
        let seen = self.probes();
        for _ in 0..6_000 {
            self.clean(10);
            if self.probes() > seen {
                return self.last(ChangeKind::Probe).unwrap();
            }
        }
        panic!("no probe in 60 s: {:?}", self.kinds());
    }
    fn last(&self, kind: ChangeKind) -> Option<Change> {
        self.changes
            .iter()
            .rev()
            .find(|(_, c)| c.kind == kind)
            .map(|(_, c)| *c)
    }
}

#[test]
fn a_probe_that_fails_after_a_chain_of_confirmed_steps_goes_back_one_step() {
    // 40 Mbit/s cut once, then four steps confirmed in a row, then the RTT
    // rises and three frames are lost: back to the step before the last one,
    // not to where the climb began.
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(200);
    let cut = rig.last(ChangeKind::StepDown).unwrap().to_kbps;
    let cut_total = rig.lr.good;
    while rig.probes() < 4 {
        rig.until_probe();
    }
    rig.rtt = Duration::from_millis(300);
    // Every outstanding step is confirmed; no new one is issued.
    for _ in 0..400 {
        rig.clean(10);
        if rig.lr.pending.iter().all(Option::is_none) {
            break;
        }
    }
    assert!(rig.lr.pending.iter().all(Option::is_none));
    let c = rig.lr.confirmed.expect("a recent confirmation");
    assert!(c.before > cut_total, "a chain, not one step");
    let expect = rig.lr.kbps_for(c.before, 20, PERIOD);
    assert!(expect > cut, "{expect} against the first cut {cut}");
    rig.lose_three();
    rig.clean(200);
    let failed = rig.last(ChangeKind::ProbeFailed).expect("a failed probe");
    assert_eq!(failed.to_kbps, expect);
    assert_eq!(rig.kbps, expect);
    assert_eq!(rig.cuts(), 1);
}

#[test]
fn a_step_sent_just_before_the_picture_goes_static_is_not_confirmed_by_the_stillness() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    let probe = rig.until_probe();
    let good = rig.lr.good;
    rig.busy = false;
    rig.clean(6_000);
    assert_eq!(rig.lr.good, good, "a static picture confirmed a step");
    assert!(rig.lr.bad.is_some());
    // Motion returns and the link cannot carry the step.
    rig.busy = true;
    rig.lose_three();
    rig.clean(200);
    let failed = rig.last(ChangeKind::ProbeFailed).expect("probe failed");
    assert_eq!(failed.to_kbps, probe.from_kbps);
    assert_eq!(rig.cuts(), 1, "not another cut");
}

#[test]
fn a_quiet_stream_never_probes() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.busy = false;
    rig.clean(40_000);
    assert_eq!(rig.probes(), 0, "{:?}", rig.kinds());
    assert_eq!(rig.lr.state(), State::Limited);
    rig.busy = true;
    rig.clean(5_000);
    assert!(rig.probes() > 0);
}

#[test]
fn less_fec_never_raises_the_bitrate_without_a_probe() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(200);
    let now = rig.now();
    let rtt = rig.rtt;
    let kbps = rig.kbps;
    let mut at = |fec| rig.lr.tick(now, rtt, fec, PERIOD, kbps, true).unwrap();
    let with_20 = at(20);
    let with_10 = at(10);
    assert!(with_10 <= with_20, "{with_10} after {with_20}");
    assert!(at(0) <= with_20);
    // More FEC leaves less for the video, as before.
    assert!(at(30) < with_20);
}

#[test]
fn bad_is_forgotten_after_a_minute_at_it() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(500);
    // Sit at the bad rate: no probes, so only time passes.
    rig.lr.bad = Some(rig.lr.current);
    rig.lr.next_probe = Some(rig.now() + Duration::from_secs(1_000));
    rig.clean(59_000);
    assert!(rig.lr.bad.is_some(), "forgotten too early");
    rig.clean(2_000);
    assert!(rig.lr.bad.is_none(), "still remembered after a minute");
}

#[test]
fn consecutive_failed_probes_double_the_wait_up_to_the_maximum() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    let mut holds = Vec::new();
    for _ in 0..5 {
        rig.until_probe();
        rig.lose_three();
        rig.clean(200);
        holds.push(rig.lr.hold);
    }
    let secs = Duration::from_secs;
    assert_eq!(holds, [secs(4), secs(8), secs(8), secs(8), secs(8)]);
    assert_eq!(rig.lr.hold, MAX_HOLD);
}

#[test]
fn failures_at_the_same_rate_double_the_knee_wait_up_to_its_maximum() {
    // The link carries 27 Mbit/s of the 40 requested: every step over it fails.
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    let limit = 27_000;
    let mut knees = Vec::new();
    while rig.ms < 600_000 {
        rig.clean(100);
        if rig.lr.pending.iter().flatten().any(|_| rig.kbps > limit) {
            rig.lose_three();
            rig.clean(200);
            knees.push(rig.lr.knee);
        }
    }
    assert!(knees.len() >= 6, "{knees:?}");
    assert!(knees.windows(2).all(|w| w[1] >= w[0]), "{knees:?}");
    assert_eq!(knees.last(), Some(&KNEE_MAX_HOLD), "{knees:?}");
    assert!(knees.iter().all(|k| *k <= KNEE_MAX_HOLD));
}

#[test]
fn a_cut_from_within_ten_percent_of_the_bad_rate_counts_as_a_knee_failure() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(500);
    // The bad rate sits 5 % above where the stream is.
    rig.lr.bad = Some(rig.lr.current * 105 / 100);
    rig.lr.hold = HOLD;
    rig.lr.next_probe = Some(rig.now() + Duration::from_secs(1_000));
    rig.clean(1_000);
    rig.lose_three();
    rig.clean(200);
    assert_eq!(rig.cuts(), 2);
    assert_eq!(rig.lr.knee, HOLD * 2);
    assert!(rig.lr.knee_until.is_some());
    assert_eq!(rig.lr.hold, HOLD * 2, "the wait is not reset to the first");
    // A cut from far above the bad rate is a new situation.
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(500);
    rig.lr.bad = Some(rig.lr.current * 2);
    rig.lr.next_probe = Some(rig.now() + Duration::from_secs(1_000));
    rig.clean(1_000);
    rig.lose_three();
    rig.clean(200);
    assert_eq!(rig.lr.knee, HOLD);
    assert!(rig.lr.knee_until.is_none());
    assert_eq!(rig.lr.hold, HOLD);
}

#[test]
fn probes_stay_under_a_known_knee_until_its_wait_is_over() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(500);
    let before = rig.changes.len();
    let bad = rig.lr.current * 100 / 80;
    rig.lr.bad = Some(bad);
    rig.lr.knee_until = Some(rig.now() + Duration::from_secs(20));
    rig.lr.next_probe = Some(rig.now());
    let cap = rig.lr.kbps_for(bad * KNEE_CAP_PERCENT / 100, 20, PERIOD);
    rig.clean(15_000);
    assert!(rig.probes() > 0);
    for (_, c) in &rig.changes[before..] {
        assert!(c.to_kbps <= cap, "{c:?} over the cap {cap}");
    }
    // After the wait the probes go on past it.
    rig.clean(30_000);
    assert!(rig.changes[before..].iter().any(|(_, c)| c.to_kbps > cap));
}

#[test]
fn a_confirmation_below_the_bad_rate_keeps_the_knee_wait() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(500);
    rig.lr.bad = Some(rig.lr.current * 2);
    rig.lr.knee = HOLD * 4;
    rig.lr.knee_until = Some(rig.now() + Duration::from_secs(1));
    let good = rig.lr.good;
    rig.until_probe();
    rig.clean(3_000);
    assert!(rig.lr.good > good, "the step was confirmed");
    assert!(rig.lr.bad.is_some());
    assert_eq!(rig.lr.knee, HOLD * 4);
}

#[test]
fn a_wrap_of_the_frame_counter_changes_nothing() {
    let run = |start: u32| {
        let mut rig = Rig::new(40_000);
        rig.frame = start;
        rig.clean(2_000);
        rig.lose_three();
        rig.clean(6_000);
        rig.lose_three();
        rig.clean(30_000);
        rig.changes
    };
    let plain = run(0);
    assert!(plain.len() > 3);
    for start in [u32::MAX - 100, u32::MAX - 1_500, u32::MAX - 20] {
        assert_eq!(run(start), plain, "start {start}");
    }
}

#[test]
fn a_rebuilt_controller_keeps_the_client_in_charge() {
    let stream = stream(40_000);
    let mut lr = LinkRate::new(true, &stream, 0, 20);
    lr.client_takeover();
    let mut rebuilt = lr.rebuild(true, &stream, 0, 20);
    let now = Instant::now();
    assert_eq!(rebuilt.state(), State::Off);
    assert_eq!(rebuilt.snapshot(now)["reason"], "client_controlled");
    let rtt = Duration::from_millis(20);
    assert_eq!(rebuilt.tick(now, rtt, 20, PERIOD, 40_000, true), None);
    let fresh = LinkRate::new(true, &stream, 0, 20).rebuild(true, &stream, 0, 20);
    assert_eq!(fresh.state(), State::Full);
}

// ---- link simulation ----

fn q(mut link: Link, ms: f64) -> Link {
    link.queue_ms = ms;
    link
}

fn gilbert(mbps: f64, enter: f64, leave: f64, loss: f64) -> Link {
    let mut link = Link::constant(mbps);
    link.burst = Some((enter, leave, loss));
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
            // The host sends a frame in a burst (0.05 of the period); the
            // 5 ms queue only with the packets spread over the whole period.
            for (queue, spread) in [(50., 0.05), (50., 1.), (5., 1.)] {
                let mut sim = Sim::new(q(Link::constant(link), queue), kbps, feedback);
                sim.spread = spread;
                sim.run(75.);
                let what =
                    format!("{link} Mbit/s, {kbps} kbps, fb {feedback}, q {queue}, sp {spread}");
                let wire = sim.mean_wire_bps(55., 75.) / 1e6;
                assert!(wire >= link * 0.65, "{what}: {wire:.1} Mbit/s");
                assert!(wire <= link * 1.08, "{what}: {wire:.1} Mbit/s");
                // Settled, and staying settled: every 5 s window, not the mean.
                for from in (20..75).step_by(5) {
                    let wire = sim.mean_wire_bps(from as f64, from as f64 + 5.) / 1e6;
                    assert!(
                        wire >= link * 0.65,
                        "{what}: {wire:.1} Mbit/s from {from} s"
                    );
                }
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
        for (queue, spread) in [(50., 0.05), (50., 1.), (5., 1.)] {
            let mut sim = Sim::new(q(Link::constant(138.), queue), 179_000, feedback);
            sim.spread = spread;
            sim.run(75.);
            let what = format!("fb {feedback}, q {queue}, sp {spread}");
            let early = sim
                .changes
                .iter()
                .filter(|(t, c)| *t < 10. && c.kind == ChangeKind::StepDown)
                .count();
            assert!(
                early <= 2,
                "{what}: {early} cuts, {:?}",
                sim.changes.first()
            );
            let lowest = sim.kbps_log.iter().skip(2).map(|(_, k)| *k).min().unwrap();
            assert!(lowest >= 55_000, "{what}: fell to {lowest} kbps");
            assert!(sim.mean_kbps(55., 75.) >= 80_000., "{what}");
            for from in (20..75).step_by(5) {
                let kbps = sim.mean_kbps(from as f64, from as f64 + 5.);
                assert!(kbps >= 70_000., "{what}: {kbps:.0} kbps from {from} s");
            }
            assert!(sim.frame_loss(3., 75.) < 0.02, "{what}");
        }
    }
}

#[test]
fn an_encoder_that_cannot_go_below_the_link_is_not_cut_in_a_runaway() {
    // H.264 on AMD sends at least 14 Mbit/s on noisy content however low it
    // is asked. On a 12 Mbit/s link the controller used to cut again at every
    // fence, down to 2 Mbit/s, and say the network could not carry even that.
    for feedback in [2., 20., 80.] {
        let mut sim = Sim::new(q(Link::constant(12.), 50.), 40_000, feedback);
        sim.encoder_min_kbps = 14_000;
        sim.run(60.);
        let what = format!("fb {feedback}");
        let kinds: Vec<_> = sim.changes.iter().map(|(_, c)| c.kind).collect();
        assert!(
            kinds.contains(&ChangeKind::EncoderFloor),
            "{what}: {kinds:?}"
        );
        assert!(!kinds.contains(&ChangeKind::Floor), "{what}: {kinds:?}");
        assert!(sim.cuts() <= 3, "{what}: {} cuts, {kinds:?}", sim.cuts());
        let found = sim
            .changes
            .iter()
            .find(|(_, c)| c.kind == ChangeKind::EncoderFloor)
            .map(|(t, c)| (*t, c.to_kbps))
            .unwrap();
        assert!(found.0 < 15., "{what}: found at {} s", found.0);
        // Nothing asks for less than the encoder sends, from then on.
        let lowest = sim
            .kbps_log
            .iter()
            .filter(|(t, _)| *t > found.0 + 1.)
            .map(|(_, k)| *k)
            .min()
            .unwrap();
        assert!(lowest >= 12_000, "{what}: asked for {lowest} kbps");
        let after = sim.changes.iter().filter(|(t, _)| *t > found.0);
        assert!(
            after
                .clone()
                .all(|(_, c)| c.kind != ChangeKind::StepDown || c.to_kbps >= 12_000),
            "{what}"
        );
        // It keeps trying upward from the floor, not from the minimum.
        assert!(
            after.clone().any(|(_, c)| c.kind == ChangeKind::Probe),
            "{what}: {kinds:?}"
        );
        assert!(
            sim.lr.snapshot(sim.at(60.))["encoder_floor_kbps"]
                .as_u64()
                .unwrap()
                >= 12_000
        );
    }
}

#[test]
fn an_encoder_that_follows_its_bitrate_never_gets_an_encoder_floor() {
    for (link, kbps) in [(150., 225_000), (20., 40_000)] {
        let mut sim = Sim::new(q(Link::constant(link), 50.), kbps, 20.);
        sim.run(75.);
        assert!(
            sim.changes
                .iter()
                .all(|(_, c)| c.kind != ChangeKind::EncoderFloor),
            "{link}: {:?}",
            sim.changes.iter().map(|(_, c)| c.kind).collect::<Vec<_>>()
        );
    }
}

#[test]
fn a_link_that_drops_to_an_eighth_recovers_within_twenty_seconds() {
    // 80 -> 10 -> 80 Mbit/s at a 40 Mbit/s request; the link returns at 40 s.
    for (feedback, queue, spread) in [
        (2., 50., 0.05),
        (20., 50., 0.05),
        (20., 50., 1.),
        (20., 5., 1.),
        (80., 50., 0.05),
        (80., 50., 1.),
    ] {
        let mut sim = Sim::new(q(Link::step(80., 10., 20., 40.), queue), 40_000, feedback);
        sim.spread = spread;
        sim.run(75.);
        let what = format!("fb {feedback}, q {queue}, sp {spread}");
        // An eighth of the rate takes two cuts: the first is held to half,
        // the second, right after it, goes to the delivered rate.
        let (gap, loss) = (1.5, 0.03);
        assert!(
            sim.longest_gap(19., 45.) <= gap,
            "{what}: gap {}",
            sim.longest_gap(19., 45.)
        );
        assert!(
            sim.frame_loss(3., 75.) <= loss,
            "{what}: loss {}",
            sim.frame_loss(3., 75.)
        );
        let back = sim.reaches(40., 36_000).expect("back to 90 %");
        assert!(back <= 60., "{what}: 90 % at {back:.1} s");
        assert!(sim.cuts() <= 2, "{what}: {} cuts", sim.cuts());
    }
}

#[test]
fn a_link_that_drops_to_a_tenth_recovers_within_twenty_seconds() {
    for (feedback, queue, spread) in [
        (2., 50., 0.05),
        (20., 50., 0.05),
        (20., 5., 1.),
        (80., 50., 0.05),
        (80., 50., 1.),
    ] {
        let mut sim = Sim::new(q(Link::step(300., 30., 20., 40.), queue), 40_000, feedback);
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
        assert_eq!(sim.cuts(), 1, "{what}");
    }
}

#[test]
fn a_healthy_link_never_changes_anything() {
    type Make = fn() -> Link;
    let links: [(&str, Make, u32); 11] = [
        ("1 GbE 20", || Link::constant(1000.), 20_000),
        ("1 GbE 40", || Link::constant(1000.), 40_000),
        ("1 GbE 80", || Link::constant(1000.), 80_000),
        ("150 Mbit/s 80", || Link::constant(150.), 80_000),
        ("300 Mbit/s 40", || Link::constant(300.), 40_000),
        // Wi-Fi: 0.5 % and 1 % random loss, repaired by FEC.
        ("wifi 0.5 %", || lossy(Link::constant(1000.), 0.005), 40_000),
        ("wifi-hi 1 %", || lossy(Link::constant(1000.), 0.01), 80_000),
        // Burst loss: a Gilbert channel, mild and hard, repaired by FEC.
        ("gilbert mild", || gilbert(1000., 0.002, 0.2, 0.6), 40_000),
        ("gilbert hard", || gilbert(1000., 0.004, 0.15, 0.7), 80_000),
        ("gilbert 150", || gilbert(150., 0.002, 0.2, 0.6), 80_000),
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
            // A burst can take a frame whole; two in a thousand is the limit.
            let allowed = if name.starts_with("gilbert") {
                0.002
            } else {
                0.
            };
            assert!(sim.frame_loss(0., 75.) <= allowed, "{name} fb {feedback}");
            assert!(sim.kbps_log.iter().all(|(_, k)| *k == kbps), "{name}");
        }
    }
}
