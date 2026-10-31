use super::*;

const FAST: u64 = 800_000_000;
const SAFE: u64 = 62_000_000;
const PERIOD: Duration = Duration::from_millis(16);

#[test]
fn remembered_loss_slows_the_first_frame_but_only_new_evidence_renews_it() {
    let now = Instant::now();
    let mut loss = KeyframeLoss::remembered("remembered from the previous session");
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
    assert_eq!(loss.observed_at(), None);
    for frame in 1..=2 {
        assert_eq!(loss.unrecoverable(frame, 3, PERIOD, now), None);
    }
    assert!(loss.unrecoverable(3, 3, PERIOD, now).is_some());
    assert_eq!(loss.observed_at(), Some(now));
    assert_ne!(loss.reason(), Some("remembered from the previous session"));
    assert_eq!(loss.unrecoverable(4, 4, PERIOD, now + PERIOD), None);
    assert_eq!(loss.observed_at(), Some(now));
}

#[test]
fn a_keyframe_the_client_could_not_repair_slows_every_later_frame() {
    let now = Instant::now();
    let mut loss = KeyframeLoss::new(true);
    loss.sending(1, true);
    for frame in 2..=40 {
        loss.sending(frame, false);
    }
    // An isolated ordinary-frame loss is left to recovery frames.
    assert_eq!(loss.unrecoverable(7, 40, PERIOD, now), None);
    assert_eq!(loss.bps(FAST, SAFE), FAST);
    assert!(loss.unrecoverable(1, 40, PERIOD, now).is_some());
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
    // It holds for the session and reports once.
    assert_eq!(loss.unrecoverable(1, 40, PERIOD, now), None);
    assert!(loss.latched());
    // Slower than the route only: a manual or Wi-Fi rate below it stays.
    assert_eq!(loss.bps(40_000_000, SAFE), 40_000_000);
}

#[test]
fn a_repeated_request_after_its_keyframe_was_sent_slows_the_stream() {
    let start = Instant::now();
    let mut loss = KeyframeLoss::new(true);
    // A keyframe is lost long after the start: the client asks for one.
    loss.sending_at(1, true, started_long_ago());
    assert_eq!(loss.requested(start), None);
    // A second request before any keyframe answered the first is the
    // same loss, not a lost keyframe.
    assert_eq!(loss.requested(start + Duration::from_millis(20)), None);
    loss.sending(3, true);
    loss.sending(4, false);
    assert!(loss.requested(start + Duration::from_millis(60)).is_some());
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
}

#[test]
fn a_decoder_that_falls_behind_keeps_the_fast_rate() {
    // A 240 fps stream to a client whose decode queue overflows: every
    // keyframe arrives, 15 or more frames queue up behind it and the
    // client asks again, about 70 ms apart (e2e-av1-240-ddx-load: 24
    // requests in 10 s, the stream paced at twice the bitrate).
    let start = Instant::now();
    let mut loss = KeyframeLoss::new(true);
    let mut frame = 1;
    for request in 0..24u32 {
        loss.sending(frame, true);
        frame += 1;
        for _ in 0..DECODE_QUEUE + 1 {
            loss.sending(frame, false);
            frame += 1;
        }
        assert_eq!(
            loss.requested(start + Duration::from_millis(u64::from(request) * 70)),
            None,
            "request {request}"
        );
    }
    assert_eq!(loss.bps(FAST, SAFE), FAST);
    // A keyframe lost on the way is still asked for at the next frame.
    loss.sending(frame, true);
    loss.sending(frame + 1, false);
    assert!(
        loss.requested(start + Duration::from_millis(24 * 70))
            .is_some()
    );
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
}

#[test]
fn spaced_recoveries_on_a_healthy_link_keep_the_fast_rate() {
    let start = Instant::now();
    let mut loss = KeyframeLoss::new(true);
    let mut frame = 1;
    // A keyframe a second for a minute, each one received: requests are
    // REPEAT or more apart.
    for second in 0..60 {
        loss.sending(frame, true);
        frame += 1;
        assert_eq!(
            loss.requested(start + Duration::from_millis(second * 1000 + 500)),
            None
        );
    }
    // Isolated reports of lost ordinary frames do not slow it either.
    for lost in 2..frame {
        if !loss.keyframes.contains(&Some(lost)) {
            assert_eq!(
                loss.unrecoverable(lost, lost, PERIOD, start + LOSS_WINDOW * lost),
                None
            );
        }
    }
    assert_eq!(loss.bps(FAST, SAFE), FAST);
}

#[test]
fn disabled_it_never_slows_the_stream() {
    let start = Instant::now();
    let mut loss = KeyframeLoss::new(false);
    loss.sending(1, true);
    assert_eq!(loss.unrecoverable(1, 1, PERIOD, start), None);
    loss.requested(start);
    loss.sending(2, true);
    assert_eq!(loss.requested(start + Duration::from_millis(10)), None);
    assert_eq!(loss.bps(FAST, SAFE), FAST);
}

/// Two seconds of a 31 Mbps (a 40 Mbps client's) 1440p60 stream through
/// a 100 Mbps egress with a drop-tail queue of `queue` bytes, behind the
/// host's 1 Gbps link, as in `network_pacing`'s model. A Moonlight client
/// without reference invalidation shows nothing until a keyframe arrives
/// whole, and asks for one after each loss once a later frame arrives.
/// Returns the frames it showed and whether pacing slowed.
fn shown_frames(
    enabled: bool,
    fec_reports: bool,
    egress_bps: f64,
    queue: usize,
    fast: u64,
) -> (u32, bool) {
    use crate::network_pacing::{
        Pacer, batch_payload_limit, overhead, rate_bps, video_wire_rate_bps,
    };
    let packetizer = crate::packet::VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: 20,
        min_fec: 2,
        key: Some([0; 16]),
    };
    let (kbps, period) = (31_000, Duration::from_secs_f64(1. / 60.));
    let needed = video_wire_rate_bps(&packetizer, kbps, period, false);
    let safe = rate_bps(0, kbps, 0, true, needed);
    let datagram = packetizer.packet_size + 16 + 32;
    let wire = (datagram + overhead(false)) as f64 * 8.;
    let (wire_time, host_time) = (
        Duration::from_secs_f64(wire / egress_bps),
        Duration::from_secs_f64(wire / 1e9),
    );
    let capacity = Duration::from_secs_f64(queue as f64 * 8. / egress_bps);
    let start = Instant::now();
    let mut loss = KeyframeLoss::new(enabled);
    let mut pacer = Pacer::new(start);
    let (mut now, mut drained) = (start, start);
    let (mut waiting, mut shown) = (true, 0);
    for frame in 1..=120u32 {
        now = now.max(start + period * (frame - 1));
        // A keyframe is five ordinary frames here; AMF's are larger at low motion.
        let keyframe = waiting;
        let payload =
            (f64::from(kbps) * 125. * period.as_secs_f64()) as usize * if keyframe { 5 } else { 1 };
        loss.sending_at(frame, keyframe, now);
        let bps = loss.bps(fast, safe);
        let batch = (batch_payload_limit(bps, datagram, false, 64 * 1024) / datagram).clamp(1, 64);
        let mut whole = true;
        for (data, fec) in packetizer.block_layout(payload) {
            let (mut remaining, mut lost) = (data + fec, 0);
            while remaining > 0 {
                now = now.max(pacer.due());
                let count = remaining.min(batch);
                for _ in 0..count {
                    now += host_time;
                    let done = drained.max(now) + wire_time;
                    if done.saturating_duration_since(now) <= capacity {
                        drained = done;
                    } else {
                        lost += 1;
                    }
                }
                pacer.sent(now, count * datagram, count, false, bps);
                remaining -= count;
            }
            whole &= lost <= fec;
        }
        if !whole {
            if fec_reports {
                loss.unrecoverable(frame, frame, period, now);
            }
            // The request follows the next frame, a period later.
            loss.requested(now + period);
            waiting = true;
        } else if keyframe || !waiting {
            waiting = false;
            shown += 1;
        }
    }
    (shown, loss.latched())
}

#[test]
fn a_slow_port_that_drops_every_fast_keyframe_gets_a_picture() {
    for queue in [16 * 1024, 64 * 1024] {
        for fec_reports in [true, false] {
            // The released policy: every keyframe is lost, nothing is shown.
            assert_eq!(
                shown_frames(false, fec_reports, 100e6, queue, FAST),
                (0, false),
                "{queue} B queue"
            );
            // Slowed after the first lost keyframe, or the repeated
            // request without reports, the third keyframe arrives. The
            // requests of the first half second are the client's start
            // and do not count, so without reports it takes that long.
            let (shown, latched) = shown_frames(true, fec_reports, 100e6, queue, FAST);
            let least = if fec_reports { 117 } else { 80 };
            assert!(
                latched && shown >= least,
                "{queue} B queue, reports {fec_reports}: {shown}"
            );
        }
    }
}

#[test]
fn a_fast_path_keeps_the_fast_rate() {
    for queue in [16 * 1024, 64 * 1024] {
        assert_eq!(shown_frames(true, true, 1e9, queue, FAST), (120, false));
    }
}

#[test]
fn a_100_mbps_client_that_says_so_gets_its_first_keyframe_whole_without_a_latch() {
    // The same slow port as above, but the client reports its link
    // (Nightjar, wired): the host paces at 4/5 of it from the first
    // packet, so the keyframe fits the port's queue and nothing is lost.
    use crate::network_pacing::rate_bps_for_client;
    let hinted = rate_bps_for_client(0, 31_000, 1_000_000_000, false, 0, 100_000_000);
    assert_eq!(hinted, 80_000_000);
    for queue in [16 * 1024, 64 * 1024] {
        for fec_reports in [true, false] {
            assert_eq!(
                shown_frames(true, fec_reports, 100e6, queue, hinted),
                (120, false),
                "{queue} B queue, reports {fec_reports}"
            );
            // Without the hint, the same stream loses its keyframe.
            let (shown, latched) = shown_frames(true, fec_reports, 100e6, queue, FAST);
            assert!(latched && shown < 120, "{queue} B queue: {shown}");
        }
    }
}

#[test]
fn only_recent_keyframes_match_reports() {
    let now = Instant::now();
    let mut loss = KeyframeLoss::new(true);
    for frame in 1..=9 {
        loss.sending(frame * 100, true);
    }
    // The first keyframe left the eight-entry history.
    assert_eq!(loss.unrecoverable(100, 900, PERIOD, now), None);
    assert!(loss.unrecoverable(200, 900, PERIOD, now).is_some());
}

#[test]
fn repeated_ordinary_losses_slow_before_any_keyframe_is_lost() {
    let now = Instant::now();
    let mut loss = KeyframeLoss::new(true);
    loss.sending_at(1, true, started_long_ago());
    for frame in 2..=20 {
        loss.sending(frame, false);
    }
    assert_eq!(loss.unrecoverable(5, 20, PERIOD, now), None);
    assert_eq!(
        loss.unrecoverable(12, 20, PERIOD, now + Duration::from_millis(500)),
        None
    );
    assert_eq!(loss.bps(FAST, SAFE), FAST);
    // Reports may arrive out of frame order.
    assert!(
        loss.unrecoverable(8, 20, PERIOD, now + Duration::from_secs(1))
            .is_some()
    );
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
    assert_eq!(
        loss.unrecoverable(20, 20, PERIOD, now + Duration::from_secs(1)),
        None
    );
}

#[test]
fn interleaved_block_reports_count_each_frame_once() {
    let now = Instant::now();
    let mut loss = KeyframeLoss::new(true);
    for frame in [1, 2, 1, 2, 1, 2, 1, 2] {
        assert_eq!(loss.unrecoverable(frame, 3, PERIOD, now), None);
    }
    assert_eq!(loss.bps(FAST, SAFE), FAST);
    assert!(loss.unrecoverable(3, 3, PERIOD, now).is_some());
}

#[test]
fn loss_window_expires_individual_entries() {
    let start = Instant::now();
    let mut loss = KeyframeLoss::new(true);
    assert_eq!(loss.unrecoverable(1, 1, PERIOD, start), None);
    assert_eq!(
        loss.unrecoverable(2, 2, PERIOD, start + Duration::from_secs(1)),
        None
    );
    // Exactly two seconds expires frame 1, but retains frame 2.
    assert_eq!(loss.unrecoverable(3, 3, PERIOD, start + LOSS_WINDOW), None);
    assert_eq!(loss.bps(FAST, SAFE), FAST);
    assert!(
        loss.unrecoverable(4, 4, PERIOD, start + LOSS_WINDOW + PERIOD)
            .is_some()
    );
}

#[test]
fn ordinary_loss_fence_rejects_stale_and_future_frames_and_handles_wrap() {
    let now = Instant::now();
    for fps in [30, 60, 120, 240] {
        let period = Duration::from_secs_f64(1. / f64::from(fps));
        let mut loss = KeyframeLoss::new(true);
        for frame in [1, 2, 3, 1001] {
            assert_eq!(loss.unrecoverable(frame, 1000, period, now), None);
        }
        assert_eq!(loss.unrecoverable(998, 1000, period, now), None);
        assert_eq!(loss.unrecoverable(999, 1000, period, now), None);
        assert!(loss.unrecoverable(1000, 1000, period, now).is_some());
    }
    let mut loss = KeyframeLoss::new(true);
    assert_eq!(loss.unrecoverable(u32::MAX, 1, PERIOD, now), None);
    assert_eq!(loss.unrecoverable(0, 1, PERIOD, now), None);
    assert!(loss.unrecoverable(1, 1, PERIOD, now).is_some());
}

#[test]
fn disabled_guard_and_unknown_period_ignore_repeated_ordinary_losses() {
    let now = Instant::now();
    for (enabled, period) in [(false, PERIOD), (true, Duration::ZERO)] {
        let mut loss = KeyframeLoss::new(enabled);
        for frame in 1..20 {
            assert_eq!(loss.unrecoverable(frame, frame, period, now), None);
        }
        assert_eq!(loss.bps(FAST, SAFE), FAST);
    }
}

/// A stream that has been running a while: frame 1 was the first keyframe
/// and ordinary frames follow it up to `last`.
/// Seconds into a stream, well past its first keyframe's startup window.
fn started_long_ago() -> Instant {
    Instant::now() - Duration::from_secs(10)
}

fn running(last: u32) -> KeyframeLoss {
    let mut loss = KeyframeLoss::new(true);
    loss.sending_at(1, true, started_long_ago());
    for frame in 2..=last {
        loss.sending(frame, false);
    }
    loss
}

#[test]
fn a_keyframe_the_host_sent_on_its_own_is_not_the_one_a_request_asks_again_for() {
    // Bitrate change, encoder or capture restart, HDR metadata: the
    // encoder emits a keyframe nobody asked for. The client resets its
    // decoder and asks, twice in quick succession, as Moonlight does when
    // a request races the keyframe already on its way. Nothing was lost.
    let start = Instant::now();
    let mut loss = running(300);
    loss.sending_at(301, true, start);
    assert_eq!(loss.requested(start + Duration::from_millis(40)), None);
    loss.sending_at(302, true, start + Duration::from_millis(45));
    loss.sending_at(303, false, start + Duration::from_millis(50));
    assert_eq!(loss.requested(start + Duration::from_millis(120)), None);
    assert!(!loss.latched());
    assert_eq!(loss.bps(FAST, SAFE), FAST);
    assert!(loss.take_ignored().is_some());
}

#[test]
fn requests_the_host_provoked_do_not_count_even_without_its_keyframe_being_asked_for() {
    // A request is answered, then the host changes the stream again with
    // a keyframe of its own (Alt+Tab recreates the capture and encoder)
    // and the client asks once more. The old rule armed on every keyframe.
    let start = Instant::now();
    let mut loss = running(50);
    assert_eq!(loss.requested(start), None);
    loss.sending_at(51, true, start + Duration::from_millis(5));
    loss.sending_at(60, true, start + Duration::from_millis(200));
    assert_eq!(loss.requested(start + Duration::from_millis(300)), None);
    assert_eq!(loss.bps(FAST, SAFE), FAST);
}

#[test]
fn ordinary_frames_after_a_host_keyframe_are_not_counted_for_a_second() {
    let start = Instant::now();
    let mut loss = running(100);
    loss.sending_at(101, true, start);
    for frame in 102..=140 {
        loss.sending_at(frame, false, start);
    }
    for (frame, ms) in [(110, 100), (120, 200), (130, 300), (131, 400)] {
        assert_eq!(
            loss.unrecoverable(frame, 140, PERIOD, start + Duration::from_millis(ms)),
            None
        );
    }
    assert!(!loss.latched());
    // Losses that persist after the window are counted.
    let after = start + HOST_CHANGE_WINDOW;
    for frame in [132, 133] {
        assert_eq!(loss.unrecoverable(frame, 140, PERIOD, after), None);
    }
    assert!(loss.unrecoverable(134, 140, PERIOD, after).is_some());
    // Frames sent before the host's change were not affected by it.
    let mut loss = running(100);
    loss.sending_at(101, true, start);
    for frame in [97, 98] {
        assert_eq!(loss.unrecoverable(frame, 101, PERIOD, start), None);
    }
    assert!(loss.unrecoverable(99, 101, PERIOD, start).is_some());
}

#[test]
fn a_host_keyframe_the_client_could_not_rebuild_is_still_a_lost_keyframe() {
    let start = Instant::now();
    let mut loss = running(100);
    loss.sending_at(101, true, start);
    let latched = loss
        .unrecoverable(101, 103, PERIOD, start + Duration::from_millis(30))
        .unwrap();
    assert_eq!(latched.reason, "a keyframe arrived incomplete");
    assert!(latched.detail.contains("101"), "{latched:?}");
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
}

#[test]
fn a_keyframe_asked_again_for_after_the_window_still_slows_the_stream() {
    let start = Instant::now();
    let mut loss = running(100);
    loss.sending_at(101, true, start);
    let late = start + HOST_CHANGE_WINDOW + Duration::from_millis(300);
    assert_eq!(loss.requested(late), None);
    loss.sending_at(150, true, late + Duration::from_millis(20));
    loss.sending_at(151, false, late + Duration::from_millis(30));
    let latched = loss.requested(late + Duration::from_millis(100)).unwrap();
    assert_eq!(
        latched.reason,
        "the client asked again for the keyframe it was just sent"
    );
    assert!(latched.detail.contains("150"), "{latched:?}");
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
}

/// The rc.4 run-1 chain (#59): the client asks, keyframe 2509 goes out,
/// 11 frames follow, and it asks again 128 ms after the first request.
/// It reported no loss at all.
fn asked_again(loss: &mut KeyframeLoss, t: Instant) -> Option<Latched> {
    assert_eq!(loss.requested(t), None);
    loss.sending_at(2509, true, t + Duration::from_millis(10));
    for frame in 2510..=2520 {
        loss.sending_at(frame, false, t + Duration::from_millis(10) + PERIOD);
    }
    loss.requested(t + Duration::from_millis(128))
}

#[test]
fn a_client_that_reports_its_losses_does_not_latch_on_a_repeated_request() {
    let t = Instant::now();
    let mut loss = running(2500);
    loss.set_reports_losses(true);
    assert_eq!(asked_again(&mut loss, t), None);
    let note = loss.take_ignored().unwrap();
    assert!(note.contains("128 ms"), "{note}");
    assert!(note.contains("reports its own losses"), "{note}");
    assert!(!loss.latched());
    assert_eq!(loss.bps(FAST, SAFE), FAST);
    // A client without FEC status latches on the same chain, as before.
    let mut loss = running(2500);
    let latched = asked_again(&mut loss, t).unwrap();
    assert_eq!(
        latched.reason,
        "the client asked again for the keyframe it was just sent"
    );
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
}

#[test]
fn a_client_that_reports_its_losses_still_latches_on_an_incomplete_keyframe() {
    let t = Instant::now();
    let mut loss = running(2500);
    loss.set_reports_losses(true);
    assert_eq!(asked_again(&mut loss, t), None);
    let latched = loss
        .unrecoverable(2509, 2520, PERIOD, t + Duration::from_millis(130))
        .unwrap();
    assert_eq!(latched.reason, "a keyframe arrived incomplete");
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
    // So do three unrecoverable frames.
    let mut loss = running(2500);
    loss.set_reports_losses(true);
    for frame in [2400, 2450] {
        assert_eq!(loss.unrecoverable(frame, 2500, PERIOD, t), None);
    }
    assert!(loss.unrecoverable(2490, 2500, PERIOD, t).is_some());
}

#[test]
fn frames_lost_on_send_slow_the_stream_and_are_not_counted_twice() {
    // Send-buffer pressure (error 10055 at 800 Mbps on a Hyper-V route):
    // the socket refuses the rest of ordinary frames. Three within two
    // seconds slow the stream, as three the client reports would.
    let now = Instant::now();
    let mut loss = running(100);
    assert_eq!(loss.abandoned(40, PERIOD, now), None);
    assert_eq!(loss.abandoned(41, PERIOD, now), None);
    let latched = loss.abandoned(42, PERIOD, now).unwrap();
    assert_eq!(
        latched.reason,
        "three recent video frames were lost on send within two seconds"
    );
    // Only the host saw it: provisional, and still slow.
    assert_eq!(latched.kind, LatchKind::Send);
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
    // The client reporting the same frames adds nothing to count.
    let mut loss = running(100);
    for frame in [40, 41] {
        assert_eq!(loss.abandoned(frame, PERIOD, now), None);
    }
    for frame in [40, 41] {
        assert_eq!(loss.unrecoverable(frame, 100, PERIOD, now), None);
    }
    assert!(loss.take_ignored().is_some());
    assert!(!loss.latched());
    // Frames the host sent whole and the client lost count with them.
    assert!(loss.unrecoverable(60, 100, PERIOD, now).is_some());
}

#[test]
fn a_keyframe_lost_on_send_slows_the_stream_at_once() {
    let now = Instant::now();
    let mut loss = running(100);
    loss.sending_at(101, true, now);
    let latched = loss.abandoned(101, PERIOD, now).unwrap();
    assert_eq!(latched.reason, "a keyframe was lost on send");
    assert!(latched.detail.contains("101"), "{latched:?}");
    assert_eq!(latched.kind, LatchKind::Send);
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
    assert!(loss.sustained_at().is_none());
    // Nothing lost on send is counted once the guard is off.
    let mut off = KeyframeLoss::new(false);
    off.sending_at(1, true, now);
    assert_eq!(off.abandoned(1, PERIOD, now), None);
    assert!(!off.latched());
}

/// A stream long past its start whose socket refused frames 101-103 at
/// `t0` (#59: `UDP send failed code=10035` while a game started).
fn send_latched(t0: Instant) -> KeyframeLoss {
    let mut loss = running(100);
    for frame in 101..=103 {
        loss.sending_at(frame, false, t0);
    }
    assert_eq!(loss.abandoned(101, PERIOD, t0), None);
    assert_eq!(loss.abandoned(102, PERIOD, t0), None);
    let latched = loss.abandoned(103, PERIOD, t0).unwrap();
    assert_eq!(latched.kind, LatchKind::Send);
    assert!(!latched.confirmed);
    loss
}

#[test]
fn loss_only_the_host_saw_is_provisional_and_lifts() {
    let t0 = Instant::now();
    let mut loss = send_latched(t0);
    assert_eq!(loss.kind(), Some(LatchKind::Send));
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
    assert!(loss.sustained_at().is_none());
    assert!(loss.ever_latched());
    let frame = flow(&mut loss, 104, t0, t0 + Duration::from_millis(9_900));
    assert!(loss.latched());
    assert!(loss.take_lifted().is_none());
    flow(
        &mut loss,
        frame,
        t0 + Duration::from_millis(9_900),
        t0 + Duration::from_millis(10_300),
    );
    assert!(!loss.latched());
    assert_eq!(loss.bps(FAST, SAFE), FAST);
    assert!(loss.sustained_at().is_none());
    let note = loss.take_lifted().unwrap();
    assert!(note.contains("host's socket"), "{note}");
}

#[test]
fn more_send_loss_extends_a_send_latch_and_never_confirms_it() {
    let t0 = Instant::now();
    let mut loss = send_latched(t0);
    let mut frame = flow(&mut loss, 104, t0, t0 + Duration::from_secs(5));
    // Three more frames, and a keyframe, refused five seconds in.
    let again = t0 + Duration::from_secs(5);
    for _ in 0..3 {
        loss.sending_at(frame, false, again);
        assert_eq!(loss.abandoned(frame, PERIOD, again), None);
        frame += 1;
    }
    loss.sending_at(frame, true, again);
    assert_eq!(loss.abandoned(frame, PERIOD, again), None);
    assert_eq!(loss.kind(), Some(LatchKind::Send));
    assert!(loss.sustained_at().is_none());
    // Clean ten seconds counts from the last of them.
    frame = flow(
        &mut loss,
        frame + 1,
        again,
        t0 + Duration::from_millis(14_900),
    );
    assert!(loss.latched());
    flow(
        &mut loss,
        frame,
        t0 + Duration::from_millis(14_900),
        t0 + Duration::from_millis(15_300),
    );
    assert!(!loss.latched());
    assert!(loss.sustained_at().is_none());
}

#[test]
fn a_client_report_confirms_a_send_latch_for_memory() {
    let t0 = Instant::now();
    // An ordinary frame the client lost right after: it takes the oldest
    // of the three slots the host's losses filled, and confirms.
    let mut loss = send_latched(t0);
    loss.sending_at(104, false, t0);
    let at = t0 + Duration::from_millis(100);
    let confirmed = loss.unrecoverable(104, 104, PERIOD, at).unwrap();
    assert!(confirmed.confirmed);
    assert_eq!(confirmed.from, Some(LatchKind::Send));
    assert_eq!(confirmed.kind, LatchKind::Sustained);
    assert_eq!(loss.sustained_at(), Some(at));
    // Or a keyframe it could not rebuild.
    let mut loss = send_latched(t0);
    loss.sending_at(104, true, t0);
    let confirmed = loss.unrecoverable(104, 104, PERIOD, at).unwrap();
    assert!(confirmed.confirmed);
    assert_eq!(confirmed.from, Some(LatchKind::Send));
    assert_eq!(loss.sustained_at(), Some(at));
    flow(&mut loss, 105, at, at + Duration::from_secs(30));
    assert!(loss.latched());
    // A report of a frame the host refused is not the client's loss.
    let mut loss = send_latched(t0);
    assert_eq!(loss.unrecoverable(103, 103, PERIOD, at), None);
    assert!(loss.sustained_at().is_none());
}

#[test]
fn a_send_latch_after_a_lift_holds_for_the_stream_but_is_not_remembered() {
    let t0 = Instant::now();
    let mut loss = send_latched(t0);
    let mut frame = flow(&mut loss, 104, t0, t0 + Duration::from_millis(10_300));
    assert!(!loss.latched());
    let again = t0 + Duration::from_secs(20);
    for _ in 0..2 {
        loss.sending_at(frame, false, again);
        assert_eq!(loss.abandoned(frame, PERIOD, again), None);
        frame += 1;
    }
    loss.sending_at(frame, false, again);
    let latched = loss.abandoned(frame, PERIOD, again).unwrap();
    assert_eq!(latched.kind, LatchKind::Sustained);
    assert!(!latched.confirmed);
    assert!(loss.sustained_at().is_none());
    frame = flow(&mut loss, frame + 1, again, again + Duration::from_secs(30));
    assert!(loss.latched());
    assert!(loss.sustained_at().is_none());
    // The client's report is what makes it worth remembering.
    let late = again + Duration::from_secs(30);
    loss.sending_at(frame, true, late);
    let confirmed = loss.unrecoverable(frame, frame, PERIOD, late).unwrap();
    assert!(confirmed.confirmed);
    assert_eq!(confirmed.from, Some(LatchKind::Sustained));
    assert_eq!(loss.sustained_at(), Some(late));
    assert_eq!(loss.unrecoverable(frame, frame, PERIOD, late), None);
}

/// After the host's socket refused frames, the client asks for a
/// keyframe, the socket refuses recovery keyframe 104 too, and the client
/// asks again 55 ms later. It never got 104: the host's loss.
fn asked_again_for_a_refused_keyframe(loss: &mut KeyframeLoss, t: Instant) -> Option<Latched> {
    assert_eq!(loss.requested(t), None);
    let sent = t + Duration::from_millis(5);
    loss.sending_at(104, true, sent);
    loss.abandoned(104, PERIOD, sent);
    for frame in 105..=107 {
        loss.sending_at(frame, false, sent + PERIOD);
    }
    loss.requested(t + Duration::from_millis(55))
}

#[test]
fn asking_again_for_a_keyframe_the_host_refused_does_not_confirm_a_send_latch() {
    let t0 = Instant::now();
    let mut loss = send_latched(t0);
    let t = t0 + Duration::from_millis(20);
    assert_eq!(asked_again_for_a_refused_keyframe(&mut loss, t), None);
    assert_eq!(loss.kind(), Some(LatchKind::Send));
    assert!(loss.sustained_at().is_none());
    // It lifts 10 s after the last of it, like any send latch.
    let frame = flow(&mut loss, 108, t, t + Duration::from_millis(10_300));
    assert!(!loss.latched(), "{frame}");
    assert!(loss.sustained_at().is_none());
}

#[test]
fn a_refused_keyframe_stays_the_host_s_loss_past_the_abandoned_ring() {
    // A 240 fps stream on a flapping adapter: after refused recovery
    // keyframe 104, nine more frames are given up, more than the eight
    // the abandoned ring holds, and the client asks again within
    // DECODE_QUEUE frames of 104.
    let t0 = Instant::now();
    let mut loss = send_latched(t0);
    let t = t0 + Duration::from_millis(20);
    assert_eq!(loss.requested(t), None);
    let sent = t + Duration::from_millis(5);
    loss.sending_at(104, true, sent);
    loss.abandoned(104, PERIOD, sent);
    for frame in 105..=113 {
        loss.sending_at(frame, false, sent);
        loss.abandoned(frame, PERIOD, sent);
    }
    assert!(!loss.is_abandoned(104));
    assert_eq!(loss.requested(t + Duration::from_millis(60)), None);
    assert_eq!(loss.kind(), Some(LatchKind::Send));
    assert!(loss.sustained_at().is_none());
    // A newer keyframe that goes out whole is the one asked for again.
    let mut loss = send_latched(t0);
    assert_eq!(loss.requested(t), None);
    loss.sending_at(104, true, sent);
    loss.abandoned(104, PERIOD, sent);
    assert_eq!(loss.requested(t + Duration::from_millis(60)), None);
    loss.sending_at(105, true, t + Duration::from_millis(65));
    let confirmed = loss.requested(t + Duration::from_millis(110)).unwrap();
    assert!(confirmed.confirmed);
    assert!(loss.sustained_at().is_some());
}

#[test]
fn asking_again_for_a_keyframe_the_host_refused_does_not_renew_a_remembered_route() {
    let t = Instant::now();
    let mut loss = KeyframeLoss::remembered("remembered from the previous session");
    loss.sending_at(1, true, started_long_ago());
    for frame in 2..=103 {
        loss.sending(frame, false);
    }
    assert_eq!(asked_again_for_a_refused_keyframe(&mut loss, t), None);
    assert!(loss.latched());
    assert!(loss.sustained_at().is_none());
    // A keyframe that did reach the client still renews it.
    let mut loss = KeyframeLoss::remembered("remembered from the previous session");
    loss.sending_at(1, true, started_long_ago());
    for frame in 2..=103 {
        loss.sending(frame, false);
    }
    assert_eq!(loss.requested(t), None);
    loss.sending_at(104, true, t + Duration::from_millis(5));
    let renewed = loss.requested(t + Duration::from_millis(55)).unwrap();
    assert_eq!(renewed.kind, LatchKind::Sustained);
    assert!(loss.sustained_at().is_some());
}

#[test]
fn frames_abandoned_on_send_are_remembered_with_the_guard_on_or_off() {
    let now = Instant::now();
    for enabled in [true, false] {
        let mut loss = KeyframeLoss::new(enabled);
        assert!(!loss.is_abandoned(9));
        loss.abandoned(9, PERIOD, now);
        assert!(loss.is_abandoned(9), "guard {enabled}");
        assert!(!loss.is_abandoned(10), "guard {enabled}");
    }
}

#[test]
fn the_recovery_keyframe_after_a_loss_on_send_opens_no_window() {
    // The host answers frames its socket refused with a keyframe. That
    // keyframe is recovery, not a change of the stream: the losses around
    // it still count.
    let start = Instant::now();
    let mut loss = running(100);
    loss.loss_keyframe();
    loss.sending_at(101, true, start);
    for frame in 102..=140 {
        loss.sending_at(frame, false, start);
    }
    for frame in [110, 120] {
        assert_eq!(loss.unrecoverable(frame, 140, PERIOD, start), None);
    }
    assert!(loss.unrecoverable(130, 140, PERIOD, start).is_some());
    assert!(loss.take_ignored().is_none());
}

#[test]
fn a_request_just_before_the_hosts_keyframe_does_not_make_it_the_one_asked_for() {
    // The client asks a moment before the host recreates the encoder; the
    // keyframe that follows is the host's, whatever was pending.
    let start = Instant::now();
    let mut loss = running(100);
    assert_eq!(loss.requested(start), None);
    loss.host_keyframe();
    loss.sending_at(101, true, start + Duration::from_millis(10));
    loss.sending_at(102, false, start + Duration::from_millis(20));
    assert_eq!(loss.requested(start + Duration::from_millis(60)), None);
    assert!(!loss.latched());
    assert!(loss.take_ignored().is_some());
    // The same request answered by an ordinary keyframe is still the one
    // a repeat asks again for.
    let mut loss = running(100);
    assert_eq!(loss.requested(start), None);
    loss.sending_at(101, true, start + Duration::from_millis(10));
    loss.sending_at(102, false, start + Duration::from_millis(20));
    assert!(loss.requested(start + Duration::from_millis(60)).is_some());
}

#[test]
fn a_request_a_codec_cannot_answer_leaves_nothing_pending() {
    // An intra-only codec makes no keyframe for a request; the next
    // keyframe (the host's own) must not be taken as its answer.
    let start = Instant::now();
    let mut loss = running(100);
    assert_eq!(loss.requested(start), None);
    loss.unanswerable();
    loss.sending_at(101, true, start + Duration::from_millis(10));
    // Taken as an answer it would arm the repeat rule; as the encoder's
    // own it opens the window, so the repeat is not evidence.
    loss.sending_at(102, false, start + Duration::from_millis(20));
    assert_eq!(loss.requested(start + Duration::from_millis(60)), None);
    assert!(!loss.latched());
}

#[test]
fn about_one_loss_a_second_with_recovery_keyframes_still_slows_the_stream() {
    // A route that loses a frame about every 900 ms: each loss is answered with a
    // reference-invalidation confirm keyframe. If those opened windows
    // the next loss would always land inside one and nothing would count.
    let start = Instant::now();
    let mut loss = running(100);
    let mut latched = None;
    for (i, frame) in [110_u32, 190, 270].into_iter().enumerate() {
        let at = start + Duration::from_millis(900 * i as u64);
        loss.loss_keyframe();
        loss.sending_at(frame + 5, true, at);
        latched = loss.unrecoverable(frame, frame + 6, PERIOD, at);
    }
    assert!(latched.is_some());
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
}

#[test]
fn nothing_is_ignored_or_logged_while_the_guard_is_off() {
    let start = Instant::now();
    let mut loss = KeyframeLoss::new(false);
    loss.sending_at(1, true, start);
    loss.sending_at(2, false, start);
    assert_eq!(loss.requested(start), None);
    assert_eq!(loss.unrecoverable(2, 3, PERIOD, start), None);
    assert!(loss.take_ignored().is_none());
}

#[test]
fn the_ignored_note_names_the_window_that_applied() {
    let start = Instant::now();
    let mut loss = KeyframeLoss::new(true);
    loss.sending_at(1, true, start);
    assert_eq!(loss.requested(start + Duration::from_millis(100)), None);
    let note = loss.take_ignored().unwrap();
    assert!(note.contains("500 ms"), "{note}");
    let mut loss = running(100);
    loss.host_keyframe();
    loss.sending_at(101, true, start);
    assert_eq!(loss.requested(start + Duration::from_millis(100)), None);
    let note = loss.take_ignored().unwrap();
    assert!(note.contains("1000 ms"), "{note}");
}

/// The start of a stream as the host log shows it (a 120 fps stream with
/// a remembered latch, twice): the client's first request comes before
/// the first keyframe, wire frame 1, is sent; it asks again 112 ms later,
/// the host answers with frame 11, and the client has shown nothing yet.
fn startup(loss: &mut KeyframeLoss, t0: Instant) {
    assert_eq!(loss.requested(t0 - Duration::from_millis(5)), None);
    loss.sending_at(1, true, t0);
    for frame in 2..=10 {
        loss.sending_at(frame, false, t0 + PERIOD * (frame - 1));
    }
    assert_eq!(loss.requested(t0 + Duration::from_millis(112)), None);
    loss.sending_at(11, true, t0 + Duration::from_millis(116));
    for frame in 12..=20 {
        loss.sending_at(
            frame,
            false,
            t0 + Duration::from_millis(116) + PERIOD * (frame - 11),
        );
    }
}

#[test]
fn the_clients_requests_while_the_first_keyframe_arrives_do_not_slow_the_stream() {
    let t0 = Instant::now();
    let mut loss = KeyframeLoss::new(true);
    startup(&mut loss, t0);
    // A third request and an ordinary-frame report in the same moment
    // are the same start, not evidence.
    assert_eq!(loss.requested(t0 + Duration::from_millis(300)), None);
    assert_eq!(
        loss.unrecoverable(12, 20, PERIOD, t0 + Duration::from_millis(300)),
        None
    );
    assert!(!loss.latched());
    assert_eq!(loss.bps(FAST, SAFE), FAST);
    // Past the window the first request of a new pair is not a pair.
    assert_eq!(loss.requested(t0 + Duration::from_millis(700)), None);
    assert!(!loss.latched());
}

#[test]
fn a_keyframe_lost_right_after_the_start_still_slows_the_stream() {
    let t0 = Instant::now();
    let mut loss = KeyframeLoss::new(true);
    startup(&mut loss, t0);
    // The client reports the keyframe it was sent incomplete: that
    // counts inside the startup window as well.
    assert!(
        loss.unrecoverable(11, 20, PERIOD, t0 + Duration::from_millis(200))
            .is_some()
    );
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
    // Or, past the window, it asks, gets a keyframe and asks again.
    let mut loss = KeyframeLoss::new(true);
    startup(&mut loss, t0);
    let t = t0 + Duration::from_millis(600);
    assert_eq!(loss.requested(t), None);
    loss.sending_at(30, true, t + PERIOD);
    loss.sending_at(31, false, t + PERIOD * 2);
    assert!(loss.requested(t + Duration::from_millis(60)).is_some());
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
}

/// Ordinary frames numbered from `from`, one per period starting at
/// `start`, until `end`. Returns the next frame number.
fn flow(loss: &mut KeyframeLoss, from: u32, start: Instant, end: Instant) -> u32 {
    let mut frame = from;
    while start + PERIOD * (frame - from) < end {
        loss.sending_at(frame, false, start + PERIOD * (frame - from));
        frame += 1;
    }
    frame
}

/// The stream's first keyframe (wire frame 1) lost at the very start.
fn lost_first_keyframe(t0: Instant) -> (KeyframeLoss, Latched) {
    let mut loss = KeyframeLoss::new(true);
    loss.sending_at(1, true, t0);
    loss.sending_at(2, false, t0 + PERIOD);
    let latched = loss
        .unrecoverable(1, 2, PERIOD, t0 + Duration::from_millis(100))
        .unwrap();
    (loss, latched)
}

#[test]
fn a_lost_first_keyframe_is_provisional_and_lifts_after_ten_clean_seconds() {
    let t0 = Instant::now();
    let (mut loss, latched) = lost_first_keyframe(t0);
    assert_eq!(latched.kind, LatchKind::Startup);
    assert!(!latched.confirmed);
    assert_eq!(loss.kind(), Some(LatchKind::Startup));
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
    // Nothing to remember: the next session starts at the route rate.
    assert!(loss.ever_latched());
    assert!(loss.sustained_at().is_none());
    // Still held just short of ten seconds.
    let (a, b) = (
        t0 + Duration::from_millis(9_900),
        t0 + Duration::from_millis(10_300),
    );
    let frame = flow(&mut loss, 3, t0 + PERIOD * 2, a);
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
    assert!(loss.take_lifted().is_none());
    // Lifted ten seconds after the latch, once, with a note for the log.
    flow(&mut loss, frame, a, b);
    assert_eq!(loss.bps(FAST, SAFE), FAST);
    assert!(!loss.latched());
    assert!(loss.observed_at().is_none());
    assert!(loss.sustained_at().is_none());
    assert!(loss.ever_latched());
    let note = loss.take_lifted().unwrap();
    assert!(note.contains("first keyframe"), "{note}");
    assert!(loss.take_lifted().is_none());
}

#[test]
fn a_provisional_latch_waits_for_busy_video_and_quiet_reports() {
    let t0 = Instant::now();
    let (mut loss, _) = lost_first_keyframe(t0);
    // Ten seconds, but only a trickle of frames: not busy video.
    loss.sending_at(3, false, t0 + Duration::from_secs(11));
    assert!(loss.latched());
    // An isolated ordinary-frame loss within the window keeps it held.
    let late = t0 + Duration::from_secs(12);
    assert_eq!(loss.unrecoverable(500, 600, PERIOD, late), None);
    for frame in 4..=200 {
        loss.sending_at(frame, false, late + Duration::from_millis(u64::from(frame)));
    }
    assert!(loss.latched());
    // The loss restarts the count: ten seconds after it, not after the
    // latch, the latch lifts.
    loss.sending_at(201, false, late + Duration::from_millis(9_900));
    assert!(loss.latched());
    loss.sending_at(202, false, late + Duration::from_millis(10_100));
    assert!(!loss.latched());
}

#[test]
fn a_provisional_latch_is_clean_for_ten_seconds_after_the_last_counted_loss() {
    let t0 = Instant::now();
    let (mut loss, _) = lost_first_keyframe(t0);
    let mut frame = flow(&mut loss, 3, t0 + PERIOD * 2, t0 + Duration::from_secs(6));
    // A loss after the latch (frames sent after it) at 6 s.
    let lost_at = t0 + Duration::from_secs(6);
    loss.sending_at(frame, false, lost_at);
    assert_eq!(loss.unrecoverable(frame, frame, PERIOD, lost_at), None);
    frame = flow(
        &mut loss,
        frame + 1,
        lost_at,
        t0 + Duration::from_millis(10_500),
    );
    // Ten seconds after the latch, but only four and a half after the loss.
    assert!(loss.latched());
    frame = flow(
        &mut loss,
        frame,
        t0 + Duration::from_millis(10_500),
        t0 + Duration::from_millis(15_900),
    );
    assert!(loss.latched());
    flow(
        &mut loss,
        frame,
        t0 + Duration::from_millis(15_900),
        t0 + Duration::from_millis(16_300),
    );
    assert!(!loss.latched());
    assert!(loss.take_lifted().is_some());
}

#[test]
fn loss_from_before_the_latch_does_not_confirm_it() {
    let t0 = Instant::now();
    let late = t0 + Duration::from_secs(1);
    // K1 and its answer K2 are both out when K1 is reported lost.
    let mut loss = KeyframeLoss::new(true);
    loss.sending_at(1, true, t0);
    assert_eq!(loss.requested(t0 + Duration::from_millis(100)), None);
    loss.sending_at(2, false, t0 + PERIOD);
    loss.sending_at(3, true, t0 + Duration::from_millis(120));
    loss.sending_at(4, false, t0 + Duration::from_millis(140));
    let latched = loss.unrecoverable(1, 4, PERIOD, late).unwrap();
    assert_eq!(latched.kind, LatchKind::Startup);
    // K2, sent before the latch, reported lost: the same bad start.
    assert_eq!(loss.unrecoverable(3, 4, PERIOD, late), None);
    assert_eq!(loss.abandoned(3, PERIOD, late), None);
    for frame in [2, 4] {
        assert_eq!(loss.unrecoverable(frame, 4, PERIOD, late), None);
    }
    assert_eq!(loss.kind(), Some(LatchKind::Startup));
    assert!(loss.sustained_at().is_none());
    // A keyframe sent after the latch and lost is more.
    loss.sending_at(5, true, late);
    assert_eq!(
        loss.unrecoverable(5, 5, PERIOD, late + Duration::from_millis(50))
            .unwrap()
            .kind,
        LatchKind::Sustained
    );
    // A repeated-request latch followed by the old first keyframe's report.
    let mut loss = KeyframeLoss::new(true);
    startup(&mut loss, t0);
    let t = t0 + Duration::from_millis(600);
    assert_eq!(loss.requested(t), None);
    loss.sending_at(30, true, t + PERIOD);
    loss.sending_at(31, false, t + PERIOD * 2);
    let latched = loss.requested(t + Duration::from_millis(60)).unwrap();
    assert_eq!(latched.kind, LatchKind::Startup);
    let after = t + Duration::from_millis(100);
    assert_eq!(loss.unrecoverable(1, 31, PERIOD, after), None);
    assert_eq!(loss.unrecoverable(11, 31, PERIOD, after), None);
    assert_eq!(loss.kind(), Some(LatchKind::Startup));
    // Ordinary frames sent before the latch do not count either.
    for frame in [12, 13, 14] {
        assert_eq!(loss.unrecoverable(frame, 31, PERIOD, after), None);
    }
    assert_eq!(loss.kind(), Some(LatchKind::Startup));
}

#[test]
fn only_the_first_keyframe_and_the_clients_quick_request_are_the_start() {
    let t0 = Instant::now();
    // An encoder rebuild two and a half seconds in.
    let mut loss = KeyframeLoss::new(true);
    loss.sending_at(1, true, t0);
    loss.host_keyframe();
    loss.sending_at(150, true, t0 + Duration::from_millis(2_500));
    let latched = loss
        .unrecoverable(150, 151, PERIOD, t0 + Duration::from_millis(2_600))
        .unwrap();
    assert_eq!(latched.kind, LatchKind::Sustained);
    // A keyframe nobody asked for, even soon after the first.
    let mut loss = KeyframeLoss::new(true);
    loss.sending_at(1, true, t0);
    loss.sending_at(40, true, t0 + Duration::from_millis(700));
    let latched = loss
        .unrecoverable(40, 41, PERIOD, t0 + Duration::from_millis(800))
        .unwrap();
    assert_eq!(latched.kind, LatchKind::Sustained);
    // A keyframe the client asked for, too late to be the first's answer.
    let mut loss = KeyframeLoss::new(true);
    loss.sending_at(1, true, t0);
    assert_eq!(loss.requested(t0 + Duration::from_millis(1_800)), None);
    loss.sending_at(100, true, t0 + Duration::from_millis(1_900));
    let latched = loss
        .unrecoverable(100, 101, PERIOD, t0 + Duration::from_millis(2_000))
        .unwrap();
    assert_eq!(latched.kind, LatchKind::Sustained);
    // The quick one is.
    let mut loss = KeyframeLoss::new(true);
    loss.sending_at(1, true, t0);
    assert_eq!(loss.requested(t0 + Duration::from_millis(300)), None);
    loss.sending_at(20, true, t0 + Duration::from_millis(400));
    let latched = loss
        .unrecoverable(20, 21, PERIOD, t0 + Duration::from_millis(500))
        .unwrap();
    assert_eq!(latched.kind, LatchKind::Startup);
}

#[test]
fn more_evidence_confirms_a_provisional_latch_and_it_stays() {
    let t0 = Instant::now();
    let (mut loss, _) = lost_first_keyframe(t0);
    // More reports about the same keyframe are not new evidence.
    // (Past the host-change window, so ordinary reports count.)
    let at = t0 + Duration::from_secs(1);
    assert_eq!(loss.unrecoverable(1, 2, PERIOD, at), None);
    assert_eq!(loss.abandoned(1, PERIOD, at), None);
    assert_eq!(loss.kind(), Some(LatchKind::Startup));
    // Three distinct unrecoverable frames are.
    for frame in 3..=4 {
        loss.sending_at(frame, false, at);
        assert_eq!(loss.unrecoverable(frame, 4, PERIOD, at), None);
    }
    loss.sending_at(5, false, at);
    let confirmed = loss.unrecoverable(5, 5, PERIOD, at).unwrap();
    assert!(confirmed.confirmed);
    assert_eq!(confirmed.kind, LatchKind::Sustained);
    assert_eq!(loss.kind(), Some(LatchKind::Sustained));
    assert_eq!(loss.sustained_at(), Some(at));
    // It no longer lifts.
    flow(&mut loss, 5, at, at + Duration::from_secs(30));
    assert!(loss.latched());
    assert!(loss.take_lifted().is_none());
}

#[test]
fn a_second_latch_in_the_session_is_sustained() {
    let t0 = Instant::now();
    let (mut loss, _) = lost_first_keyframe(t0);
    let frame = flow(&mut loss, 3, t0 + PERIOD * 2, t0 + Duration::from_secs(11));
    assert!(!loss.latched());
    // Whatever it is lost on, the second latch is sustained.
    let again = t0 + Duration::from_millis(11_000);
    loss.sending_at(frame, true, again);
    let latched = loss
        .unrecoverable(frame, frame, PERIOD, again + Duration::from_millis(100))
        .unwrap();
    assert_eq!(latched.kind, LatchKind::Sustained);
    assert!(!latched.confirmed);
    assert!(loss.sustained_at().is_some());
    let later = again + Duration::from_secs(1);
    flow(&mut loss, frame + 1, later, later + Duration::from_secs(30));
    assert!(loss.latched());
}

#[test]
fn only_the_start_of_the_stream_is_provisional() {
    let t0 = Instant::now();
    // A keyframe lost long after the first.
    let mut loss = KeyframeLoss::new(true);
    loss.sending_at(1, true, t0);
    loss.sending_at(500, true, t0 + STARTUP_SPAN + Duration::from_secs(1));
    let latched = loss
        .unrecoverable(500, 501, PERIOD, t0 + STARTUP_SPAN + Duration::from_secs(2))
        .unwrap();
    assert_eq!(latched.kind, LatchKind::Sustained);
    assert!(loss.sustained_at().is_some());
    // Evidence about ordinary frames came first.
    let mut loss = KeyframeLoss::new(true);
    loss.sending_at(1, true, t0);
    loss.sending_at(2, false, t0 + PERIOD);
    loss.sending_at(30, false, t0 + Duration::from_secs(1));
    let seen = t0 + Duration::from_millis(1_100);
    assert_eq!(loss.unrecoverable(20, 30, PERIOD, seen), None);
    let latched = loss.unrecoverable(1, 30, PERIOD, seen).unwrap();
    assert_eq!(latched.kind, LatchKind::Sustained);
    // A keyframe lost on send at the start is provisional too, as the
    // host's own loss.
    let mut loss = KeyframeLoss::new(true);
    loss.sending_at(1, true, t0);
    let latched = loss.abandoned(1, PERIOD, t0).unwrap();
    assert_eq!(latched.reason, "a keyframe was lost on send");
    assert_eq!(latched.kind, LatchKind::Send);
    // Three frames lost on send are never a start-up burst, but they are
    // still only the host's.
    let mut loss = running(100);
    loss.abandoned(40, PERIOD, t0);
    loss.abandoned(41, PERIOD, t0);
    assert_eq!(
        loss.abandoned(42, PERIOD, t0).unwrap().kind,
        LatchKind::Send
    );
}

#[test]
fn a_repeated_request_at_the_start_is_provisional() {
    let t0 = Instant::now();
    let mut loss = KeyframeLoss::new(true);
    startup(&mut loss, t0);
    // Past the window the client asks, is sent a keyframe, asks again.
    let t = t0 + Duration::from_millis(600);
    assert_eq!(loss.requested(t), None);
    loss.sending_at(30, true, t + PERIOD);
    loss.sending_at(31, false, t + PERIOD * 2);
    let latched = loss.requested(t + Duration::from_millis(60)).unwrap();
    assert_eq!(latched.kind, LatchKind::Startup);
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
    assert!(loss.sustained_at().is_none());
    // The same chain deep into the stream is not.
    let mut loss = running(100);
    let t = Instant::now();
    assert_eq!(loss.requested(t), None);
    loss.sending_at(101, true, t + PERIOD);
    loss.sending_at(102, false, t + PERIOD * 2);
    let latched = loss.requested(t + Duration::from_millis(60)).unwrap();
    assert_eq!(latched.kind, LatchKind::Sustained);
}

#[test]
fn an_inherited_latch_stays_sustained_when_the_first_keyframe_is_lost_again() {
    let t0 = Instant::now();
    let mut loss = KeyframeLoss::remembered("remembered from the previous session");
    assert_eq!(loss.kind(), Some(LatchKind::Sustained));
    loss.sending_at(1, true, t0);
    loss.sending_at(2, false, t0 + PERIOD);
    let latched = loss
        .unrecoverable(1, 2, PERIOD, t0 + Duration::from_millis(100))
        .unwrap();
    assert_eq!(latched.kind, LatchKind::Sustained);
    assert!(loss.sustained_at().is_some());
    flow(&mut loss, 3, t0 + PERIOD * 2, t0 + Duration::from_secs(30));
    assert!(loss.latched());
}

#[test]
fn the_latch_names_the_frames_behind_it() {
    let now = Instant::now();
    let mut loss = running(30);
    for frame in [20, 22] {
        assert_eq!(loss.unrecoverable(frame, 30, PERIOD, now), None);
    }
    let latched = loss.unrecoverable(25, 30, PERIOD, now).unwrap();
    assert_eq!(
        latched.detail,
        "unrecoverable frames [20, 22, 25]; last frame sent 30"
    );
}

#[test]
fn the_bitrate_controller_shares_the_classification_even_with_the_guard_off() {
    let t0 = Instant::now();
    for enabled in [true, false] {
        let mut loss = KeyframeLoss::new(enabled);
        loss.sending_at(1, true, t0);
        for frame in 2..=10 {
            loss.sending_at(frame, false, t0);
        }
        // The first keyframe's startup window: ordinary frames after it
        // and requests are the client reacting, not the network.
        assert!(!loss.network_loss(5, t0 + Duration::from_millis(10)));
        assert!(!loss.network_request(t0 + Duration::from_millis(10)));
        // A lost keyframe always counts.
        assert!(loss.network_loss(1, t0 + Duration::from_millis(10)));
        // Past the window an ordinary frame does.
        let later = t0 + STARTUP_WINDOW + Duration::from_millis(1);
        assert!(loss.network_loss(5, later));
        assert!(loss.network_request(later));
        // A frame the host abandoned on send is counted there.
        loss.abandoned(7, PERIOD, later);
        assert!(!loss.network_loss(7, later), "guard {enabled}");
    }
}

#[test]
fn a_request_behind_a_full_decode_queue_is_not_network_evidence() {
    let t0 = Instant::now();
    let mut loss = running(1);
    let first = t0 + Duration::from_secs(5);
    assert_eq!(loss.requested(first), None);
    loss.sending_at(40, true, first);
    for frame in 41..41 + DECODE_QUEUE {
        loss.sending_at(frame, false, first);
    }
    let soon = first + Duration::from_millis(250);
    assert!(!loss.network_request(soon), "overflow behind the keyframe");
    // A second later it is a fresh loss again.
    assert!(loss.network_request(first + REPEAT + Duration::from_millis(1)));
    // Right behind the keyframe it is the keyframe that was lost.
    let mut loss = running(1);
    assert_eq!(loss.requested(first), None);
    loss.sending_at(40, true, first);
    loss.sending_at(41, false, first);
    assert!(loss.network_request(soon));
}

/// Forty seconds of the October 28 scene-cut run: 200 Mbps HEVC at 60 fps
/// on an 800 Mbps route with 0.3 % random loss and no bottleneck, a scene
/// cut every two seconds. AMF does not hold the cut, an ordinary frame, to
/// the frame cap, so it leaves without parity and is lost; the recovery
/// keyframe answering it is as large as the cap allows. A frame is lost
/// when one of its blocks loses more packets than it has parity. Returns
/// the recovery keyframes lost and the guard.
fn lost_cut_recoveries(fec_whole: bool) -> (usize, KeyframeLoss) {
    use crate::network_pacing::{fec_whole_frame_cap, recovery_frame_cap};
    let packetizer = crate::packet::VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: 20,
        min_fec: 2,
        key: Some([0; 16]),
    };
    let (kbps, period) = (200_000, Duration::from_secs_f64(1. / 60.));
    let mut cap = recovery_frame_cap(FAST, kbps, period, 20, 1392, false);
    if fec_whole {
        cap = fec_whole_frame_cap(cap, packetizer.fec_whole_payload_limit(), kbps, period);
    }
    let blocks = packetizer.block_layout(cap as usize);
    let mut seed = 0x2545_f491_4f6c_dd1d_u64;
    let mut lost_packet = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % 1000 < 3
    };
    let start = started_long_ago();
    let mut loss = KeyframeLoss::new(true);
    loss.sending_at(1, true, start);
    let mut lost = 0;
    for frame in 2..=2400u32 {
        let at = start + period * frame;
        if frame % 120 != 0 {
            loss.sending_at(frame, false, at);
            continue;
        }
        // The cut, reported two frames later; then the recovery keyframe.
        loss.sending_at(frame, false, at);
        loss.unrecoverable(frame, frame + 2, period, at + period * 2);
        loss.loss_keyframe();
        loss.sending_at(frame + 3, true, at + period * 3);
        let whole = blocks
            .iter()
            .all(|&(data, parity)| (0..data + parity).filter(|_| lost_packet()).count() <= parity);
        if !whole {
            lost += 1;
            loss.unrecoverable(frame + 3, frame + 5, period, at + period * 5);
        }
    }
    (lost, loss)
}

#[test]
fn recovery_keyframes_within_full_fec_survive_random_loss_without_a_latch() {
    // The automatic cap, about 2.6 MB, leaves no parity in four blocks:
    // the first lost recovery keyframe latches the slow pace.
    let (lost, loss) = lost_cut_recoveries(false);
    assert!(lost >= 15, "{lost} of 20 recovery keyframes lost");
    assert!(loss.latched());
    // Held to full parity (about 1.17 MB) every one arrives, and the cut
    // losses alone, one every two seconds, slow nothing.
    let (lost, loss) = lost_cut_recoveries(true);
    assert_eq!(lost, 0);
    assert!(!loss.latched());
    assert_eq!(loss.bps(FAST, SAFE), FAST);
}
