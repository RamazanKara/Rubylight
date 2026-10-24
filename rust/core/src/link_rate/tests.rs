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
    lr.on_unrecoverable(frame, false, now);
}

/// A keyframe the client could not rebuild, sent just now.
fn lost_keyframe(lr: &mut LinkRate, frame: u32, now: Instant) {
    lr.on_keyframe_sent(frame, now);
    lr.on_status(&status(frame, 60, 0), now);
    lr.on_unrecoverable(frame, true, now);
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
    /// Up to this many milliseconds are added to `rtt` at every tick.
    rtt_jitter_ms: u64,
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
            rtt_jitter_ms: 0,
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
                    self.rtt
                        + Duration::from_millis(
                            self.ms.wrapping_mul(2_654_435_761) % (self.rtt_jitter_ms + 1),
                        ),
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

/// A frame that lost `missing` data packets that FEC rebuilt.
fn rebuilt_data(lr: &mut LinkRate, frame: u32, missing: u16, now: Instant) {
    lr.on_status(&status(frame, 80 - missing, 0), now);
}

#[test]
fn rebuilt_data_loss_cuts_only_while_the_rtt_is_elevated() {
    // (rtt ms, data packets missing per 100 sent, cut)
    for (rtt, missing, cut) in [
        (20, 10, false),
        (100, 10, true),
        // 4 of 80 data packets is 5 % of the block's 96, not 4 % of the 100 sent.
        (100, 4, true),
        (100, 1, false),
        (100, 0, false),
    ] {
        let mut rig = Rig::new(40_000);
        rig.clean(3_000);
        rig.rtt = Duration::from_millis(rtt);
        rig.run(5_000, |lr, frame, now| {
            rebuilt_data(lr, frame, missing, now)
        });
        assert_eq!(
            rig.cuts() > 0,
            cut,
            "rtt {rtt}, {missing}: {:?}",
            rig.changes
        );
        if cut {
            assert_eq!(rig.changes[0].1.reason, "fec_loss_share");
        }
    }
}

#[test]
fn a_queue_filling_on_a_link_the_stream_exceeds_cuts_before_fec_gives_out() {
    // 50 Mbit/s with a 50 ms drop-tail queue and about 7 % more on the wire
    // than the link carries, packets spread over the frame period: FEC rebuilds
    // every frame (under 1 % are lost), the RTT climbs by the whole queue, and
    // the stream sits there. Only the rebuilt blocks, with the RTT, show it.
    let mut sim = Sim::new(q(Link::constant(50.), 50.), 38_800, 20.);
    sim.spread = 1.;
    sim.run(30.);
    let first = sim.changes.first().expect("a cut");
    assert_eq!(first.1.kind, ChangeKind::StepDown);
    assert_eq!(first.1.reason, "fec_loss_share");
    assert!(first.0 < 5., "cut at {} s", first.0);
    let wire = sim.mean_wire_bps(10., 30.) / 1e6;
    assert!(
        (50. * 0.65..=50. * 1.02).contains(&wire),
        "{wire:.1} Mbit/s"
    );
    assert!(
        sim.frame_loss(3., 30.) < 0.02,
        "{}",
        sim.frame_loss(3., 30.)
    );
}

#[test]
fn an_encoder_at_one_and_a_fifth_of_the_link_cuts_within_two_seconds_and_settles_under_it() {
    let mut sim = Sim::new(q(Link::constant(50.), 50.), 60_000, 20.);
    sim.run(30.);
    let first = sim.changes.first().expect("a cut");
    assert_eq!(first.1.kind, ChangeKind::StepDown);
    assert!(first.0 < 2., "cut at {} s", first.0);
    let wire = sim.mean_wire_bps(10., 30.) / 1e6;
    assert!(
        (50. * 0.65..=50. * 1.08).contains(&wire),
        "{wire:.1} Mbit/s"
    );
    assert!(
        sim.frame_loss(3., 30.) < 0.02,
        "{}",
        sim.frame_loss(3., 30.)
    );
}

/// The 2026-10-24 `wifi` arm (150 Mbit/s, 3 +/- 2 ms delay, 0.5 % loss, HEVC
/// 1080p60 at 14,988 kbps, 20 % FEC) from `host-samples.jsonl`: about 38 packets
/// a frame. A frame leaves the host in under a millisecond and the +/- 2 ms
/// jitter then puts its packets in an almost random order, so the client
/// rebuilds a block when its 32nd packet is in and reports it with parity in
/// and data still on the way: 17.7 % "missing" in the log, so nearly every
/// frame was reported. The old controller cut to the 2,415 kbps floor within
/// seven seconds (15 changes).
///
/// Modelled here as it is: a random arrival order per block, 0.5 % of packets
/// lost, a report at the 32nd arrival unless all 32 data packets came first.
/// One block of the `wifi` arm as the client reports it (see the test below):
/// data 32 + parity 7 in a random arrival order, `loss_permille` of the packets
/// lost, a report at the 32nd arrival unless all 32 data packets came first.
/// `queued` more data packets than that were still behind a queue when parity
/// made up for them. Returns whether the block was reported.
fn wifi_block(
    seed: &mut u64,
    lr: &mut LinkRate,
    frame: u32,
    loss_permille: u64,
    queued: u16,
    now: Instant,
) -> bool {
    let mut random = || {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    };
    let mut order: Vec<u16> = (0..39).collect();
    for i in (1..order.len()).rev() {
        order.swap(i, (random() % (i as u64 + 1)) as usize);
    }
    let (mut data, mut parity) = (0u16, 0u16);
    let mut report = None;
    for index in order {
        if random() % 1000 < loss_permille {
            continue;
        }
        if index < 32 {
            data += 1;
        } else {
            parity += 1;
        }
        if data + parity == 32 {
            report = Some((data, parity));
            break;
        }
    }
    let (data, parity) = report.unwrap_or((data, parity));
    // A block whose data all came first needs no report.
    if data == 32 {
        return false;
    }
    let (data, parity) = (data.saturating_sub(queued), (parity + queued).min(7));
    lr.on_status(
        &Status {
            frame,
            highest_received: 38,
            next_contiguous: 0,
            missing_before_highest: 39 - data - parity,
            data: 32,
            parity: 7,
            received_data: data,
            received_parity: parity,
            percentage: 20,
            block: 0,
            blocks: 1,
        },
        now,
    );
    true
}

#[test]
fn the_wifi_arm_with_reordering_and_random_loss_never_cuts() {
    let mut rig = Rig::new(14_988);
    // Far more than the 3 +/- 2 ms each way of the arm.
    rig.rtt_jitter_ms = 8;
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let (mut blocks, mut rebuilt) = (0u32, 0u32);
    rig.run(45_000, |lr, frame, now| {
        blocks += 1;
        rebuilt += u32::from(wifi_block(&mut seed, lr, frame, 5, 0, now));
    });
    assert!(rig.changes.is_empty(), "{:?}", rig.changes);
    assert_eq!(rig.kbps, 14_988);
    // The model is the log: nearly every block is reported, and the data
    // packets the client had not yet were far over the 5 % that cut. Counting
    // them as loss (or the whole tail, 18 %) would cut this stream.
    assert!(rebuilt * 100 > blocks * 95, "{rebuilt} of {blocks}");
    let share = rig.lr.snapshot(rig.now())["rebuilt_data_missing_percent"]
        .as_f64()
        .unwrap();
    assert!(share > 5., "{share}");
}

/// The review of 2026-10-24: the same arm with the RTT up from 20 to 60 ms for
/// 1.5 s at 20 s (a Wi-Fi roam, a competing flow) gave a cut 14,988 -> 8,705
/// and a rate still 14,027 kbps 21 s later: while the RTT is up the reordering
/// share counted as loss. Only what the flat RTT did not already show is a
/// queue; a real one, with loss on top, still cuts.
#[test]
fn a_short_rtt_excursion_on_the_wifi_arm_is_no_cut_but_a_real_queue_is() {
    // Data packets behind a queue on top of the arm's, per block, while the
    // RTT is up.
    for extra in [0u16, 3] {
        let mut rig = Rig::new(14_988);
        rig.rtt_jitter_ms = 8;
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        rig.run(20_000, |lr, frame, now| {
            wifi_block(&mut seed, lr, frame, 5, 0, now);
        });
        assert!(rig.changes.is_empty(), "{:?}", rig.changes);
        rig.rtt = Duration::from_millis(60);
        rig.run(1_500, |lr, frame, now| {
            wifi_block(&mut seed, lr, frame, 5, extra, now);
        });
        rig.rtt = Duration::from_millis(20);
        rig.run(20_000, |lr, frame, now| {
            wifi_block(&mut seed, lr, frame, 5, 0, now);
        });
        assert_eq!(
            rig.cuts() > 0,
            extra > 0,
            "extra {extra}: {:?}",
            rig.changes
        );
        if extra == 0 {
            assert_eq!(rig.kbps, 14_988);
        }
    }
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
    // And the probe after it, once the losses stop.
    rig.clean(60_000);
    let probe = rig
        .changes
        .iter()
        .find(|(_, c)| c.kind == ChangeKind::Probe);
    let (_, probe) = probe.expect("a probe change");
    assert!(probe.delivered_kbps > 0, "{probe:?}");
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
            lost_keyframe(lr, frame, now);
        }
    });
    assert_eq!(rig.cuts(), 1, "{:?}", rig.changes);
    let (at, change) = &rig.changes[0];
    assert!(*at < 2_500, "cut at {at} ms");
    assert_eq!(change.reason, "unrecoverable_frames");
}

/// The same run, 2026-10-24: after the latch's fence the only sign of loss for
/// 1.5 s was the client's keyframe request, from a client that reports FEC
/// status (so requests are otherwise not evidence). No keyframe had got through
/// since the latch, so the latch had failed; the cut had no measured share and
/// the 30 % default left the stream above the link.
#[test]
fn a_keyframe_request_with_no_keyframe_through_since_the_latch_cuts_deep() {
    let mut rig = Rig::new(58_988);
    rig.run(300, |lr, frame, now| repaired(lr, frame, 1, now));
    rig.lr.on_latch(rig.now(), true, true);
    // A request inside the fence is nothing.
    rig.clean(300);
    rig.lr.on_keyframe_request(true, rig.now());
    rig.clean(1_000);
    assert!(rig.changes.is_empty(), "{:?}", rig.changes);
    rig.lr.on_keyframe_request(true, rig.now());
    rig.clean(300);
    assert_eq!(rig.cuts(), 1, "{:?}", rig.changes);
    let (_, cut) = rig.changes[0];
    assert_eq!(cut.reason, "keyframe_requests");
    assert!(cut.to_kbps <= 58_988 / 2, "{cut:?}");

    // The same request, a keyframe that was lost since the latch: cut.
    let mut rig = Rig::new(58_988);
    rig.run(300, |lr, frame, now| repaired(lr, frame, 1, now));
    rig.lr.on_latch(rig.now(), true, true);
    rig.clean(600);
    let (frame, now) = (rig.frame, rig.now());
    rig.lr.on_keyframe_sent(frame, now);
    rig.clean(700);
    rig.lr.on_status(&status(frame, 60, 0), rig.now());
    rig.lr.on_unrecoverable(frame, true, rig.now());
    rig.clean(300);
    assert_eq!(rig.cuts(), 1, "{:?}", rig.changes);
}

/// A start-up latch (the first keyframe was lost, pacing slowed) is followed by
/// a keyframe that arrives whole. A request after that is the decoder, a
/// resize or one burst, not proof that the latch failed: the stream must not be
/// halved on it, whether or not the client reports FEC status.
#[test]
fn a_keyframe_request_after_a_whole_keyframe_since_the_latch_is_not_a_cut() {
    for fec in [true, false] {
        let mut rig = Rig::new(58_988);
        if fec {
            rig.run(300, |lr, frame, now| repaired(lr, frame, 1, now));
        } else {
            rig.clean(300);
        }
        rig.lr.on_latch(rig.now(), true, true);
        rig.clean(600);
        // A keyframe after the fence, nothing reported lost for it.
        let (frame, now) = (rig.frame, rig.now());
        rig.lr.on_keyframe_sent(frame, now);
        rig.clean(1_000);
        rig.lr.on_keyframe_request(true, rig.now());
        rig.clean(1_000);
        assert!(rig.changes.is_empty(), "fec {fec}: {:?}", rig.changes);
    }
    // One ordinary frame lost after a whole keyframe is no cut either, but
    // with no keyframe through since the latch it is.
    for keyframe in [true, false] {
        let mut rig = Rig::new(58_988);
        rig.clean(300);
        rig.lr.on_latch(rig.now(), true, true);
        rig.clean(600);
        let (frame, now) = (rig.frame, rig.now());
        if keyframe {
            rig.lr.on_keyframe_sent(frame, now);
        }
        rig.clean(1_000);
        let (frame, now) = (rig.frame, rig.now());
        unrecoverable(&mut rig.lr, frame, now);
        rig.clean(1_000);
        assert_eq!(rig.changes.is_empty(), keyframe, "{:?}", rig.changes);
    }
}

/// 2026-10-24 review: the keyframe that answers a latch is sent about 20 ms
/// after it, inside the fence, and arrives whole. One lost frame or one request
/// 1.5 s later must not halve the rate (main needed three events).
#[test]
fn a_keyframe_sent_inside_the_fence_that_arrived_whole_makes_one_event_no_cut() {
    for fec in [true, false] {
        for request in [false, true] {
            let mut rig = Rig::new(58_988);
            if fec {
                rig.run(300, |lr, frame, now| repaired(lr, frame, 1, now));
            } else {
                rig.clean(300);
            }
            rig.lr.on_latch(rig.now(), true, true);
            rig.clean(20);
            let (frame, now) = (rig.frame, rig.now());
            rig.lr.on_keyframe_sent(frame, now);
            rig.clean(1_500);
            let (frame, now) = (rig.frame, rig.now());
            if request {
                rig.lr.on_keyframe_request(true, now);
            } else {
                unrecoverable(&mut rig.lr, frame, now);
            }
            rig.clean(1_000);
            assert!(
                rig.changes.is_empty(),
                "fec {fec} request {request}: {:?}",
                rig.changes
            );
        }
    }
    // The same keyframe reported lost inside the fence did not arrive: the
    // next event is a cut.
    let mut rig = Rig::new(58_988);
    rig.clean(300);
    rig.lr.on_latch(rig.now(), true, true);
    rig.clean(20);
    let (frame, now) = (rig.frame, rig.now());
    rig.lr.on_keyframe_sent(frame, now);
    rig.clean(100);
    let now = rig.now();
    lost_keyframe(&mut rig.lr, frame, now);
    rig.clean(1_400);
    let (frame, now) = (rig.frame, rig.now());
    unrecoverable(&mut rig.lr, frame, now);
    rig.clean(1_000);
    assert_eq!(rig.cuts(), 1, "{:?}", rig.changes);
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
fn a_link_that_drops_to_an_eighth_recovers_within_twenty_five_seconds() {
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
        // The last probe that failed on the dip keeps probes under its rate
        // for the quiet hold (KNEE_QUIET), and a step at a time past it.
        assert!(back <= 65., "{what}: 90 % at {back:.1} s");
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

/// The 2026-10-24 `r50-hevc-hi` arm (50 Mbit/s, 50 ms queue, 59 Mbps asked): a
/// probe that failed at the link's edge was followed by climbs back to the
/// same edge, a freeze at each, 6 to 9 of them in a run.
#[test]
fn probes_stop_under_the_rate_that_failed_for_the_quiet_hold() {
    for (feedback, spread) in [(20., 0.05), (20., 1.), (80., 0.05), (80., 1.)] {
        let mut sim = Sim::new(q(Link::constant(50.), 50.), 60_000, feedback);
        sim.spread = spread;
        sim.run(60.);
        let what = format!("fb {feedback}, sp {spread}");
        let (at, failed) = sim
            .changes
            .iter()
            .find(|(_, c)| c.kind == ChangeKind::ProbeFailed)
            .map(|(t, c)| (*t, c.from_kbps))
            .unwrap_or_else(|| panic!("{what}: no probe failed: {:?}", sim.changes));
        // For the quiet hold nothing goes back to the rate that failed.
        let top = sim
            .kbps_log
            .iter()
            .filter(|(t, _)| *t > at + 1. && *t < at + 19.)
            .map(|(_, k)| *k)
            .max()
            .unwrap();
        assert!(top < failed, "{what}: {top} kbps again after {failed}");
        let failures: Vec<_> = sim
            .changes
            .iter()
            .filter(|(t, c)| {
                *t > 2. && matches!(c.kind, ChangeKind::ProbeFailed | ChangeKind::StepDown)
            })
            .map(|(t, c)| (*t, c.kind))
            .collect();
        assert!(failures.len() <= 3, "{what}: {failures:?}");
        assert!(
            sim.frame_loss(3., 60.) < 0.02,
            "{what}: {}",
            sim.frame_loss(3., 60.)
        );
    }
}

#[test]
fn a_link_that_improves_clears_the_knee_and_the_rate_climbs_back() {
    // 50 Mbit/s for 15 s, then 150: the probe that failed at 50 keeps the
    // stream under it for the quiet hold, and one small step past it, then the
    // usual climb, finds the room. Without the knee cleared on the step that
    // passes it, the near-knee steps (a twentieth every two seconds) go on to
    // the ceiling and the stream gets there 15 s later.
    for feedback in [20., 80.] {
        let link = Link::new(|t| if t < 15. { 50e6 } else { 150e6 });
        let mut sim = Sim::new(q(link, 50.), 60_000, feedback);
        sim.run(75.);
        let what = format!("fb {feedback}");
        let back = sim.reaches(15., 54_000).unwrap_or(f64::MAX);
        assert!(back <= 40., "{what}: 90 % at {back:.1} s");
        assert!(
            sim.frame_loss(40., 75.) < 0.005,
            "{what}: {}",
            sim.frame_loss(40., 75.)
        );
    }
}

/// The 2026-10-24 `r50-hevc-hi@4` run: a probe to 41.6 Mbps on a 50 Mbit/s link
/// started a storm of recovery keyframes (38 in 3.5 s, a freeze of 3.7 s). The
/// client sent FEC status, so its keyframe requests were not evidence (E4), and
/// a keyframe lost whole is not reported as an unrecoverable frame: the probe
/// was judged failed only when ordinary frames were reported 4 s in.
#[test]
fn a_storm_of_keyframe_requests_fails_the_probe_that_is_pending() {
    let mut rig = Rig::new(59_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(500);
    rig.until_probe();
    rig.until_probe();
    let start = rig.ms;
    let before = rig.changes.len();
    let mut n = 0u32;
    // A request every 7 frames (115 ms), as in the log, and no FEC report.
    rig.run(1_500, |lr, _, now| {
        n += 1;
        if n.is_multiple_of(7) {
            lr.on_keyframe_request(true, now);
        }
    });
    let (at, change) = rig.changes[before..]
        .iter()
        .find(|(_, c)| c.kind == ChangeKind::ProbeFailed)
        .unwrap_or_else(|| panic!("no failed probe: {:?}", rig.kinds()));
    assert!(*at - start < 700, "failed after {} ms", at - start);
    assert_eq!(change.reason, "keyframe_requests");
}

#[test]
fn keyframe_requests_without_a_pending_probe_or_with_fewer_than_three_are_not_a_cut() {
    // A healthy link: a client that reports FEC status asks for a few keyframes.
    let mut rig = Rig::new(59_000);
    rig.clean(2_000);
    rig.run(10, |lr, frame, now| repaired(lr, frame, 2, now));
    let mut n = 0u32;
    rig.run(10_000, |lr, _, now| {
        n += 1;
        if n.is_multiple_of(7) {
            lr.on_keyframe_request(true, now);
        }
    });
    assert!(rig.changes.is_empty(), "{:?}", rig.kinds());
    // A probe pending, and only two requests inside the window.
    let mut rig = Rig::new(59_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(500);
    rig.until_probe();
    let before = rig.changes.len();
    let mut n = 0u32;
    rig.run(1_500, |lr, _, now| {
        n += 1;
        if n == 10 || n == 17 {
            lr.on_keyframe_request(true, now);
        }
    });
    assert!(
        rig.changes[before..]
            .iter()
            .all(|(_, c)| c.kind != ChangeKind::ProbeFailed && c.kind != ChangeKind::StepDown),
        "{:?}",
        rig.kinds()
    );
}
