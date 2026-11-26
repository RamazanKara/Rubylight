use super::*;

const FRAME_US: u64 = 16_667;
const PACKET: u32 = 1_400;
/// The client's clock is this far from the host's; only differences matter.
const CLIENT_OFFSET: u64 = 7_000_000_000;

/// One bottleneck: a rate, a drop-tail queue and a propagation delay.
struct Link {
    bps: f64,
    free_at: u64,
    propagation: u64,
    queue_limit: u64,
    /// Every `drop_every`-th packet is lost on the air (0: none).
    drop_every: u64,
    sent: u64,
    /// (from, until, bps): the rate for packets sent in that time instead of `bps`.
    slow: Vec<(u64, u64, f64)>,
}

impl Link {
    fn new(bps: f64) -> Self {
        Self {
            bps,
            free_at: 0,
            propagation: 2_000,
            queue_limit: 200_000,
            drop_every: 0,
            sent: 0,
            slow: Vec::new(),
        }
    }

    /// The client's arrival time, or `None` for a lost packet.
    fn carry(&mut self, sent: u64, bytes: u32) -> Option<u64> {
        self.sent += 1;
        if self.drop_every != 0 && self.sent.is_multiple_of(self.drop_every) {
            return None;
        }
        let start = sent.max(self.free_at);
        if start - sent > self.queue_limit {
            return None;
        }
        let bps = self
            .slow
            .iter()
            .find(|(from, until, _)| (*from..*until).contains(&sent))
            .map_or(self.bps, |slow| slow.2);
        self.free_at = start + (f64::from(bytes) * 8. * 1e6 / bps) as u64;
        Some(self.free_at + self.propagation + CLIENT_OFFSET)
    }
}

/// What the run saw.
struct Run {
    targets: Vec<(u64, u64)>,
    last: Decision,
    queue_p95_ms: Option<f64>,
}

/// A 60 fps stream through `link` for `seconds`: frames of the target's size, paced at the
/// decision's pace or three times the target, a report every 50 ms, a tick every 20 ms.
fn run(cc: &mut Congestion, link: &mut Link, from_us: u64, seconds: u64, max_bps: u64) -> Run {
    run_content(cc, link, from_us, seconds * 1_000_000, max_bps, u64::MAX)
}

/// The same for `micros`, with a picture that needs only `content_bps`: the encoder makes the
/// lower of that and the target.
fn run_content(
    cc: &mut Congestion,
    link: &mut Link,
    from_us: u64,
    micros: u64,
    max_bps: u64,
    content_bps: u64,
) -> Run {
    let mut pending: VecDeque<PacketResult> = VecDeque::new();
    let mut decision = Decision::default();
    let mut targets = Vec::new();
    let end = from_us + micros;
    let mut next_frame = from_us;
    let mut next_report = from_us + 50_000;
    let mut next_tick = from_us + 20_000;
    let mut now = from_us;
    while now < end {
        if now >= next_frame {
            let target = decision.video_bps.unwrap_or(max_bps);
            let pace = decision.pace_bps.unwrap_or(target * 3) as f64;
            let bytes = target.min(content_bps) / 60 / 8;
            let packets = bytes.div_ceil(u64::from(PACKET)).max(1);
            for i in 0..packets {
                let sent = now + (i as f64 * f64::from(PACKET) * 8. * 1e6 / pace) as u64;
                let arrival = link.carry(sent, PACKET);
                pending.push_back(PacketResult {
                    sent_us: sent,
                    arrival_us: arrival,
                    wire_bytes: PACKET,
                });
            }
            next_frame += FRAME_US;
        }
        if now >= next_report {
            // Reported in send order up to the first packet still in flight.
            let mut ready = Vec::new();
            while let Some(p) = pending.front() {
                let done = match p.arrival_us {
                    Some(at) => at - CLIENT_OFFSET <= now,
                    None => p.sent_us + 20_000 <= now,
                };
                if !done {
                    break;
                }
                ready.push(pending.pop_front().unwrap());
            }
            if !ready.is_empty() {
                cc.on_feedback(
                    now,
                    Feedback {
                        packets: &ready,
                        stride: 1,
                        late: 0,
                        skipped: 0,
                    },
                    20 + 2 * ready.len(),
                );
            }
            next_report += 50_000;
        }
        if now >= next_tick {
            decision = cc.tick(now, Duration::from_millis(10), 20);
            targets.push((now, decision.video_bps.unwrap_or(max_bps)));
            next_tick += 20_000;
        }
        now += 1_000;
    }
    let (_, p95, _) = cc.take_window_delay();
    Run {
        targets,
        last: decision,
        queue_p95_ms: p95,
    }
}

fn mean_after(targets: &[(u64, u64)], from: u64) -> f64 {
    let tail: Vec<u64> = targets
        .iter()
        .filter(|(t, _)| *t >= from)
        .map(|(_, b)| *b)
        .collect();
    tail.iter().sum::<u64>() as f64 / tail.len() as f64
}

#[test]
fn a_link_that_carries_the_stream_never_changes_it() {
    let max = 50_000_000;
    let mut cc = Congestion::new(2_000_000, max);
    let mut link = Link::new(300e6);
    let run = run(&mut cc, &mut link, 0, 10, max);
    assert!(run.targets.iter().all(|(_, b)| *b == max));
    assert_eq!(run.last, Decision::default());
    assert_eq!(cc.counters().overuse_cuts, 0);
    assert_eq!(cc.counters().loss_cuts, 0);
    assert_eq!(cc.counters().lost, 0);
    assert!(cc.take_change().is_none());
    assert!(run.queue_p95_ms.unwrap() < 2., "{:?}", run.queue_p95_ms);
    let snapshot = cc.snapshot(10_000_000);
    assert_eq!(snapshot["fresh"], true);
    assert_eq!(snapshot["target_kbps"], 50_000);
}

#[test]
fn a_narrow_link_brings_the_rate_down_to_it_and_keeps_the_queue_short() {
    let max = 50_000_000;
    let mut cc = Congestion::new(2_000_000, max);
    let mut link = Link::new(30e6);
    // The first seconds fill the buffer before the first cut; judge what follows.
    run(&mut cc, &mut link, 0, 10, max);
    let run = run(&mut cc, &mut link, 10_000_000, 20, max);
    assert!(cc.counters().overuse_cuts >= 1, "{:?}", cc.counters());
    let mean = mean_after(&run.targets, 10_000_000);
    assert!((15e6..33e6).contains(&mean), "mean target {mean}");
    // Without the controller the 200 ms buffer would stay full.
    assert!(run.queue_p95_ms.unwrap() < 100., "{:?}", run.queue_p95_ms);
    assert!(cc.stream_delay().count() > 1000);
    // The narrow link spread the bursts: pacing follows it, above the target.
    let pace = run.last.pace_bps.expect("bottleneck pacing");
    assert!((20e6..60e6).contains(&(pace as f64)), "{pace}");
    let target = run.last.video_bps.unwrap_or(max);
    assert!(
        pace as f64 >= PACE_HEADROOM * target as f64 - 1.,
        "{pace} {target}"
    );
}

#[test]
fn the_rate_recovers_when_the_link_widens_again() {
    let max = 40_000_000;
    let mut cc = Congestion::new(2_000_000, max);
    let mut link = Link::new(20e6);
    run(&mut cc, &mut link, 0, 10, max);
    assert!(cc.target_bps() < max);
    link.bps = 400e6;
    let run = run(&mut cc, &mut link, 10_000_000, 30, max);
    assert_eq!(run.targets.last().unwrap().1, max);
    let (median, longest) = cc.recovery_ms().expect("a recovery");
    assert!(
        median > 0 && longest <= RECOVERY_CAP_US / 1000,
        "{median} {longest}"
    );
    let mut restored = false;
    while let Some(change) = cc.take_change() {
        restored |= change.kind == ChangeKind::Restored;
    }
    assert!(restored);
}

#[test]
fn loss_that_fec_repairs_is_ignored_and_heavier_loss_cuts() {
    let max = 30_000_000;
    // 5 % loss with 20 % FEC: below the threshold of max(10 %, 0.75 × 1/6).
    let mut cc = Congestion::new(2_000_000, max);
    let mut link = Link::new(500e6);
    link.drop_every = 20;
    let run_light = run(&mut cc, &mut link, 0, 5, max);
    assert_eq!(cc.counters().loss_cuts, 0);
    assert!(run_light.targets.iter().all(|(_, b)| *b == max));
    let loss = cc.loss().unwrap();
    assert!((0.04..0.06).contains(&loss), "{loss}");
    // 20 %: past it.
    let mut cc = Congestion::new(2_000_000, max);
    let mut link = Link::new(500e6);
    link.drop_every = 5;
    run(&mut cc, &mut link, 0, 3, max);
    assert!(cc.counters().loss_cuts >= 1);
    assert!(cc.target_bps() < max * 95 / 100);
    let change = std::iter::from_fn(|| cc.take_change()).last().unwrap();
    assert_eq!(change.kind, ChangeKind::Loss);
}

#[test]
fn feedback_that_stops_hands_the_rate_back() {
    let max = 50_000_000;
    let mut cc = Congestion::new(2_000_000, max);
    let mut link = Link::new(20e6);
    run(&mut cc, &mut link, 0, 5, max);
    assert!(cc.target_bps() < max);
    let _ = cc.take_change();
    // No reports for longer than STALE_US.
    let later = 5_000_000 + STALE_US + 1;
    assert_eq!(
        cc.tick(later, Duration::from_millis(10), 20),
        Decision::default()
    );
    assert_eq!(cc.take_change().unwrap().kind, ChangeKind::Stale);
    assert_eq!(cc.target_bps(), max);
    assert!(!cc.fresh(later));
}

/// The field case: a stream allowed 33 Mbps whose picture needs about 2, on a link that
/// carries all of it but slows for a moment every few seconds, as Wi-Fi does. Each slowing
/// reads as overuse; a cut to 0.85 of the 2 Mbps received would hold the stream there, and
/// the rate cannot grow back while so little is sent.
#[test]
fn an_overuse_while_the_picture_needs_little_cuts_nothing() {
    let max = 33_000_000;
    let mut cc = Congestion::new(2_000_000, max);
    let mut link = Link::new(300e6);
    link.slow = (1..=6)
        .map(|i| (i * 3_000_000, i * 3_000_000 + 120_000, 1e6))
        .collect();
    let run = run_content(&mut cc, &mut link, 0, 21_000_000, max, 1_400_000);
    // The slowings were seen, so the test is not passing for want of a signal.
    assert!(cc.counters().overuse_held >= 3, "{:?}", cc.counters());
    assert_eq!(cc.counters().overuse_cuts, 0, "{:?}", cc.counters());
    assert_eq!(cc.counters().loss_cuts, 0);
    assert!(run.targets.iter().all(|(_, b)| *b == max));
    assert_eq!(run.last.video_bps, None);
    assert!(cc.take_change().is_none());
    let snapshot = cc.snapshot(21_000_000);
    assert_eq!(snapshot["overuse_held"], cc.counters().overuse_held);
    assert!(
        snapshot["sent_kbps"].as_u64().unwrap() < 3_000,
        "{snapshot}"
    );
}

/// A stream sending half its rate into a link that carries less than that: the queue stands,
/// so the overuse is the link's and the rate comes down below the link.
#[test]
fn a_link_narrower_than_a_light_picture_still_brings_the_rate_down() {
    let max = 50_000_000;
    let mut cc = Congestion::new(2_000_000, max);
    let mut link = Link::new(20e6);
    run_content(&mut cc, &mut link, 0, 10_000_000, max, 25_000_000);
    let run = run_content(&mut cc, &mut link, 10_000_000, 20_000_000, max, 25_000_000);
    assert!(cc.counters().overuse_cuts >= 1, "{:?}", cc.counters());
    let mean = mean_after(&run.targets, 10_000_000);
    assert!((10e6..22e6).contains(&mean), "mean target {mean}");
    assert!(run.queue_p95_ms.unwrap() < 100., "{:?}", run.queue_p95_ms);
}

#[test]
fn the_sent_rate_counts_lost_and_skipped_packets() {
    let mut cc = Congestion::new(1_000_000, 50_000_000);
    let packet = |sent_us: u64, arrived: bool| PacketResult {
        sent_us,
        arrival_us: arrived.then_some(CLIENT_OFFSET + sent_us + 2_000),
        wire_bytes: 1_000,
    };
    let report = |cc: &mut Congestion, at: u64, packets: &[PacketResult], skipped: u32| {
        cc.on_feedback(
            at,
            Feedback {
                packets,
                stride: 1,
                late: 0,
                skipped,
            },
            20,
        );
    };
    report(&mut cc, 10_000, &[packet(0, true)], 0);
    assert_eq!(cc.sent_bps(), None);
    // A report in which nothing arrived adds its bytes to the next one that has an arrival.
    report(&mut cc, 60_000, &[packet(50_000, false)], 0);
    assert_eq!(cc.sent_bps(), None);
    // Two listed, one of them lost, and two skipped at the listed mean: five packets since
    // the first report, over 200 ms of sending.
    report(
        &mut cc,
        210_000,
        &[packet(150_000, false), packet(200_000, true)],
        2,
    );
    assert_eq!(cc.sent_bps(), Some(5 * 1_000 * 8 * 1_000_000 / 200_000));
    assert_eq!(cc.acked_bps(), Some(1_000 * 8 * 1_000_000 / 200_000));
}

#[test]
fn before_any_feedback_nothing_is_limited() {
    let mut cc = Congestion::new(1_000_000, 10_000_000);
    assert_eq!(cc.tick(0, Duration::ZERO, 20), Decision::default());
    assert_eq!(cc.tick(1_000_000, Duration::ZERO, 20), Decision::default());
    assert!(cc.take_change().is_none());
}

#[test]
fn bounds_follow_a_new_request() {
    let mut cc = Congestion::new(1_000_000, 10_000_000);
    cc.set_bounds(1_000_000, 20_000_000);
    assert_eq!(cc.target_bps(), 20_000_000);
    cc.set_bounds(1_000_000, 5_000_000);
    assert_eq!(cc.target_bps(), 5_000_000);
}

#[test]
fn the_trendline_reads_a_growing_queue_as_overuse_and_a_steady_one_as_normal() {
    let group = |at: u64| Group {
        first_send: 0,
        last_send: 0,
        first_arrival: at,
        last_arrival: at,
        bytes: 0,
        packets: 1,
    };
    let mut steady = Trendline::default();
    let mut growing = Trendline::default();
    let mut overused = false;
    for i in 0..60u64 {
        let usage = steady.update(&Deltas {
            send_ms: 16.,
            arrival_ms: 16.,
            group: group(i * 16_000),
        });
        assert_eq!(usage, Usage::Normal);
        overused |= growing.update(&Deltas {
            send_ms: 16.,
            arrival_ms: 20.,
            group: group(i * 20_000),
        }) == Usage::Overusing;
    }
    assert!(overused);
    // A queue that drains reads as underuse.
    let mut draining = Trendline::default();
    let mut under = false;
    for i in 0..60u64 {
        under |= draining.update(&Deltas {
            send_ms: 16.,
            arrival_ms: 12.,
            group: group(i * 12_000),
        }) == Usage::Underusing;
    }
    assert!(under);
}

#[test]
fn histogram_percentiles() {
    let mut h = DelayHistogram::default();
    assert_eq!(h.percentile_ms(50.), None);
    for us in [0, 100, 300, 1_000, 2_000, 600_000] {
        h.add(us);
    }
    assert_eq!(h.count(), 6);
    assert_eq!(h.percentile_ms(50.), Some(0.375));
    // The last bucket takes everything past 500 ms.
    assert_eq!(h.percentile_ms(100.), Some(499.875));
}

#[test]
fn the_ledger_finds_each_packet_by_its_id() {
    let started = Instant::now();
    let ledger = SendLedger::new(started);
    let mut plain = vec![0u8; 100];
    plain[2..4].copy_from_slice(&0x1234u16.to_be_bytes());
    // Off: nothing recorded.
    ledger.record(&[plain.clone()], 28, started + Duration::from_millis(5));
    assert_eq!(ledger.lookup(0x1234, 5_000), None);
    ledger.enable(false);
    ledger.record(&[plain.clone()], 28, started + Duration::from_millis(5));
    assert_eq!(ledger.lookup(0x1234, 6_000), Some((5_000, 128)));
    assert_eq!(ledger.lookup(0x1235, 6_000), None);
    // A slight clock-read race (stamp within 50 ms after now_us) clamps age to 0.
    assert_eq!(ledger.lookup(0x1234, 4_000), Some((4_000, 128)));
    // Too old: an id from a wrap ago.
    assert_eq!(ledger.lookup(0x1234, 5_000 + STALE_US + 1), None);
    // Encrypted: the IV counter's low bits.
    let mut sealed = vec![0u8; 50];
    sealed[..8].copy_from_slice(&0x0001_0000_0000_beefu64.to_le_bytes());
    ledger.enable(true);
    ledger.record(&[sealed], 0, started + Duration::from_millis(7));
    assert_eq!(ledger.lookup(0xbeef, 8_000), Some((7_000, 50)));
    ledger.disable();
    assert!(!ledger.live());
}

#[test]
fn the_client_clock_unwraps() {
    let mut clock = ClientClock::default();
    assert_eq!(clock.unwrap(u32::MAX - 10), u64::from(u32::MAX - 10));
    assert_eq!(clock.unwrap(5), u64::from(u32::MAX) + 6);
    // A report whose base is a little earlier (reordered) goes back.
    assert_eq!(clock.unwrap(1), u64::from(u32::MAX) + 2);
}

#[test]
fn a_short_skip_counts_as_loss_and_a_long_one_does_not() {
    let report = |base: u16, flags: u8| TransportFeedback {
        flags,
        base,
        stride: 1,
        entries: vec![0; 10],
        ..Default::default()
    };
    let mut continuity = Continuity::default();
    assert_eq!(continuity.skipped(&report(100, 0)), 0);
    assert_eq!(continuity.skipped(&report(110, 0)), 0);
    assert_eq!(continuity.skipped(&report(720, 1)), 600);
    assert_eq!(continuity.skipped(&report(40_000, 1)), 0);
}

#[test]
fn the_session_side_takes_the_lower_limit_and_restores_the_bitrate_before_both() {
    let stream = crate::rtsp::Negotiated::default();
    assert!(!Control::new(&stream, false, 0, false).active());
    let mut c = Control::new(&stream, true, 0, false);
    assert!(c.active());
    assert_eq!(c.combine(20_000), 20_000);
    c.link_wanted = Some(15_000);
    assert_eq!(c.combine(20_000), 15_000);
    c.wanted = Some(10_000);
    assert_eq!(c.combine(15_000), 10_000);
    c.link_wanted = None;
    assert_eq!(c.combine(10_000), 10_000);
    c.wanted = None;
    assert_eq!(c.combine(10_000), 20_000);
    // Shadow mode measures but never applies its own limit; the link's still holds.
    let mut shadow = Control::new(&stream, true, 0, true);
    shadow.wanted = Some(5_000);
    assert_eq!(shadow.combine(20_000), 20_000);
    shadow.link_wanted = Some(12_000);
    assert_eq!(shadow.combine(20_000), 12_000);
    // A client that set the bitrate keeps it, also across a rebuild.
    c.takeover();
    assert!(!c.active());
    assert!(!c.rebuild(&stream, true, 0, false).active());
    assert!(
        Control::new(&stream, true, 0, false)
            .rebuild(&stream, true, 0, false)
            .active()
    );
}

#[test]
fn the_ceiling_converts_back_to_the_requested_bitrate() {
    let stream = crate::rtsp::Negotiated::default();
    let mut c = Control::new(&stream, true, 0, false);
    let period = crate::reconfigure::Mode::of(&stream).period();
    // No feedback: no limit and no pace.
    assert_eq!(c.tick(0, Duration::ZERO, 20, period), None);
    assert_eq!(c.wanted(), None);
    let budget = crate::adaptive_bitrate::WireBudget::of(&stream);
    let (_, max) = Control::bounds(&budget, FLOOR_KBPS, 20_000, 20, period);
    assert_eq!(
        budget.encoder_kbps(max + budget.audio_bps, 20_000, 20, period),
        20_000
    );
    // A user cap lowers the ceiling.
    let capped = Control::new(&stream, true, 8_000, false);
    assert_eq!(capped.ceiling_kbps, 8_000);
}

#[test]
fn a_report_is_joined_with_the_ledger() {
    let stream = crate::rtsp::Negotiated::default();
    let mut c = Control::new(&stream, true, 0, false);
    let started = Instant::now();
    let ledger = SendLedger::new(started);
    ledger.enable(false);
    let packets: Vec<Vec<u8>> = (0..10u16)
        .map(|i| {
            let mut p = vec![0u8; 1000];
            p[2..4].copy_from_slice(&i.to_be_bytes());
            p
        })
        .collect();
    ledger.record(&packets[..8], 28, started + Duration::from_millis(1));
    let report = TransportFeedback {
        base: 0,
        stride: 1,
        base_arrival_us: 5_000,
        entries: (0..10u16)
            .map(|i| {
                if i == 3 {
                    rubylight_protocol::transport_feedback::NOT_RECEIVED
                } else {
                    i * 10
                }
            })
            .collect(),
        ..Default::default()
    };
    let bytes = report.encode().unwrap();
    assert!(c.on_report(&ledger, 20_000, &bytes));
    let snapshot = c.snapshot(20_000);
    assert_eq!(snapshot["reports"], 1);
    // Ids 8 and 9 were never recorded.
    assert_eq!(snapshot["packets"], 8);
    assert_eq!(snapshot["lost"], 1);
    assert_eq!(snapshot["unmatched"], 2);
    assert_eq!(snapshot["shadow"], false);
    assert!(!c.on_report(&ledger, 20_000, &[1, 2, 3]));
    let mut off = Control::default();
    assert!(!off.on_report(&ledger, 20_000, &bytes));
    assert_eq!(off.snapshot(0)["state"], "off");
}

#[test]
fn pyrowave_below_floor_lowers_congestion_floor_to_half_ceiling() {
    let stream = crate::rtsp::Negotiated {
        codec: 3,
        width: 1920,
        height: 1080,
        fps: 60,
        bitrate_kbps: 100_000,
        ..Default::default()
    };
    assert!(crate::link_rate::pyrowave_floor_kbps(&stream) >= 100_000);
    let c = Control::new(&stream, true, 0, false);
    assert_eq!(c.ceiling_kbps, 100_000);
    assert_eq!(c.floor_kbps, 50_000);
}

#[test]
fn duplicate_and_reordered_reports_are_ignored() {
    let stream = crate::rtsp::Negotiated::default();
    let mut c = Control::new(&stream, true, 0, false);
    let started = Instant::now();
    let ledger = SendLedger::new(started);
    ledger.enable(false);
    let packets: Vec<Vec<u8>> = (0..6u16)
        .map(|i| {
            let mut p = vec![0u8; 1000];
            p[2..4].copy_from_slice(&i.to_be_bytes());
            p
        })
        .collect();
    ledger.record(&packets, 28, started + Duration::from_millis(1));
    let report = |seq: u32, base: u16, base_arrival_us: u32| {
        TransportFeedback {
            base,
            stride: 1,
            seq,
            base_arrival_us,
            entries: vec![0, 10],
            ..Default::default()
        }
        .encode()
        .unwrap()
    };
    assert!(c.on_report(&ledger, 20_000, &report(u32::MAX - 1, 0, 5_000)));
    // Duplicate and older reports do not mutate controller state.
    assert!(!c.on_report(&ledger, 25_000, &report(u32::MAX - 1, 2, 6_000)));
    assert!(!c.on_report(&ledger, 26_000, &report(u32::MAX - 2, 2, 7_000)));
    assert!(!c.on_report(&ledger, 27_000, &report(u32::MAX - 100, 2, 8_000)));
    assert_eq!(c.snapshot(27_000)["reports"], 1);
    assert_eq!(c.snapshot(27_000)["packets"], 2);
    // Newer report is accepted, including across 32-bit sequence wrap.
    assert!(c.on_report(&ledger, 30_000, &report(u32::MAX, 2, 10_000)));
    assert!(!c.on_report(&ledger, 31_000, &report(u32::MAX - 1, 2, 11_000)));
    assert!(c.on_report(&ledger, 35_000, &report(0, 4, 15_000)));
    assert!(!c.on_report(&ledger, 36_000, &report(u32::MAX, 4, 16_000)));
    assert_eq!(c.snapshot(36_000)["reports"], 3);
    assert_eq!(c.snapshot(36_000)["packets"], 6);
}

#[test]
fn client_clock_underflow_and_backward_jumps_do_not_overflow_or_stall_eviction() {
    let mut clock = ClientClock::default();
    assert_eq!(clock.unwrap(0), 0);
    // Stepping backward past 0 must clamp to 0 instead of wrapping near u64::MAX.
    assert_eq!(clock.unwrap(0xFFF0_0000), 0);
    assert_eq!(clock.unwrap(1_000), 0x0010_0000 + 1_000);

    // Feed Control::on_report with backward base_arrival_us and extreme timestamps:
    // must not panic on overflow, and acked window must stay bounded.
    let stream = crate::rtsp::Negotiated::default();
    let mut c = Control::new(&stream, true, 0, false);
    let started = Instant::now();
    let ledger = SendLedger::new(started);
    ledger.enable(false);
    let packets: Vec<Vec<u8>> = (0..20u16)
        .map(|i| {
            let mut p = vec![0u8; 1000];
            p[2..4].copy_from_slice(&i.to_be_bytes());
            p
        })
        .collect();
    ledger.record(&packets, 28, started + Duration::from_millis(1));
    let send = |seq: u32, base_arrival_us: u32| {
        TransportFeedback {
            base: 0,
            stride: 1,
            seq,
            base_arrival_us,
            entries: vec![0, 50],
            ..Default::default()
        }
        .encode()
        .unwrap()
    };
    assert!(c.on_report(&ledger, 10_000, &send(1, 0)));
    assert!(c.on_report(&ledger, 20_000, &send(2, 0xFFF0_0000)));
    assert!(c.on_report(&ledger, 30_000, &send(3, 1_000)));
    // Backward arrival jump clears stale acked entries and host_us bounds window growth.
    let mut cc = Congestion::new(1_000_000, 100_000_000);
    for i in 0..20u64 {
        let now_us = (i + 1) * 200_000;
        let arrival = if i == 10 { 500 } else { 10_000_000 + i * 100 };
        let pkt = [PacketResult {
            sent_us: now_us.saturating_sub(1_000),
            arrival_us: Some(arrival),
            wire_bytes: 1_200,
        }];
        cc.on_feedback(
            now_us,
            Feedback {
                packets: &pkt,
                stride: 1,
                late: 0,
                skipped: 0,
            },
            16,
        );
    }
    assert!(
        cc.acked.len() <= 8,
        "acked window stayed bounded: {}",
        cc.acked.len()
    );
    assert!(cc.acked_bps().is_some());
    assert!(cc.sent_bps().is_some());
}

#[test]
fn congestion_running_sums_match_full_window_scan() {
    let mut cc = Congestion::new(1_000_000, 100_000_000);
    for i in 0..40u64 {
        let now_us = (i + 1) * 50_000;
        let pkt = [
            PacketResult {
                sent_us: now_us.saturating_sub(2_000),
                arrival_us: Some(now_us),
                wire_bytes: 1_200,
            },
            PacketResult {
                sent_us: now_us.saturating_sub(1_000),
                arrival_us: if i % 5 == 0 { None } else { Some(now_us + 500) },
                wire_bytes: 900,
            },
        ];
        cc.on_feedback(
            now_us,
            Feedback {
                packets: &pkt,
                stride: 1,
                late: 1,
                skipped: 0,
            },
            16,
        );
        let expected_received: u64 = cc.acked.iter().map(|s| s.received).sum();
        let expected_sent: u64 = cc.acked.iter().map(|s| s.sent).sum();
        let expected_packets: u64 = cc.losses.iter().map(|s| s.1).sum();
        let expected_lost: u64 = cc.losses.iter().map(|s| s.2).sum();
        assert_eq!(cc.acked_received_sum, u128::from(expected_received));
        assert_eq!(cc.acked_sent_sum, u128::from(expected_sent));
        assert_eq!(cc.losses_packets_sum, expected_packets);
        assert_eq!(cc.losses_lost_sum, expected_lost);
    }
}

/// Deltas between groups for ten frames at `period`, three packets 100 µs apart each.
fn frame_deltas(inter: &mut InterArrival, period: Duration) -> usize {
    let frame_us = period.as_micros() as u64;
    let mut deltas = 0;
    for frame in 0..10u64 {
        for i in 0..3u64 {
            let sent_us = frame * frame_us + i * 100;
            let p = PacketResult {
                sent_us,
                arrival_us: Some(sent_us + 2_000),
                wire_bytes: PACKET,
            };
            deltas += usize::from(inter.add(&p, sent_us + 2_000).is_some());
        }
    }
    deltas
}

#[test]
fn above_120_fps_each_frame_is_its_own_group() {
    let at = |millihz| crate::framegen::Rate(millihz).period();
    // 240 Hz: 5 ms groups take two frames each, five groups and three deltas; half-frame
    // groups take one, ten groups and eight deltas.
    let mut fixed = InterArrival::default();
    assert_eq!(frame_deltas(&mut fixed, at(240_000)), 3);
    let mut scaled = InterArrival::default();
    scaled.set_burst_us(crate::pacing_high_hz::group_burst_us(at(240_000)));
    assert_eq!(frame_deltas(&mut scaled, at(240_000)), 8);
    // 120 Hz keeps the 5 ms window, which already holds one frame.
    let mut at_120 = InterArrival::default();
    at_120.set_burst_us(crate::pacing_high_hz::group_burst_us(at(120_000)));
    assert_eq!(at_120.burst_us(), BURST_US);
    assert_eq!(frame_deltas(&mut at_120, at(120_000)), 8);
}

#[test]
fn the_controller_follows_the_frame_period_only_above_120_fps() {
    let at = |millihz| crate::framegen::Rate(millihz).period();
    let mut cc = Congestion::new(2_000_000, 100_000_000);
    assert_eq!(cc.group_burst_us(), BURST_US);
    for millihz in [30_000, 60_000, 119_880, 120_000] {
        cc.set_frame_period(at(millihz));
        assert_eq!(cc.group_burst_us(), BURST_US, "{millihz}");
    }
    cc.set_frame_period(Duration::ZERO);
    assert_eq!(cc.group_burst_us(), BURST_US);
    cc.set_frame_period(at(240_000));
    assert_eq!(cc.group_burst_us(), 2_083);
    cc.set_frame_period(at(480_000));
    assert_eq!(cc.group_burst_us(), 1_041);
    cc.set_frame_period(at(120_000));
    assert_eq!(cc.group_burst_us(), BURST_US);
}
