//! Slower packet pacing for a client whose link drops video frames.
//!
//! Wired and unknown routes are paced at up to 800 Mbps
//! ([`crate::network_pacing::rate_bps`]). A keyframe is several ordinary
//! frames long, so behind a slower port with a shallow queue (an Onn 4K Pro's
//! 100 Mbps Ethernet behind a switch) it is the frame that gets lost. The
//! client asks for another keyframe, which is lost the same way, and the
//! picture stays frozen: 0 of 585 frames arrived whole in 20 s through a
//! 100 Mbps, 64 KiB loopback bottleneck. Once a keyframe is lost the stream
//! is paced like a Wi-Fi route for the rest of the session: twice the
//! encoder bitrate, at least 1.25 times the packetized wire demand. Repeated
//! unrecoverable ordinary frames need the same protection: successful
//! recovery keyframes must not leave a lossy route sending at the fast rate.
//! Isolated ordinary-frame losses keep the fast route policy.
//!
//! A client whose decoder falls behind asks again too: Moonlight drops its
//! decode queue when it overflows and requests a keyframe. That request comes
//! only after the queue filled up behind the keyframe, so it is told apart by
//! the frames sent since the keyframe; slowing the send would not help it.
use std::time::{Duration, Instant};

/// A second keyframe request this soon after the first, with a keyframe sent
/// in between: that keyframe did not arrive.
pub const REPEAT: Duration = Duration::from_millis(500);
/// Moonlight's decode unit queue (`decodeUnitQueue`) holds 15 frames. A
/// request after this many frames followed the keyframe can be its overflow
/// (a decoder that falls behind); a lost keyframe is asked for again as soon
/// as the next frame arrives.
pub const DECODE_QUEUE: u32 = 15;
/// Require repeated, recent receiver-confirmed losses before slowing ordinary
/// frames. One frame can produce several failed FEC block reports.
pub const LOSS_WINDOW: Duration = Duration::from_secs(2);
const LOSS_FRAMES: usize = 3;

#[derive(Debug, Default)]
pub struct KeyframeLoss {
    enabled: bool,
    /// Recent keyframes' wire frame numbers, to match the client's reports.
    keyframes: [Option<u32>; 8],
    next: usize,
    last_request: Option<Instant>,
    keyframe_since_request: bool,
    /// Frames sent after the latest keyframe.
    since_keyframe: u32,
    recent_losses: [Option<(u32, Instant)>; LOSS_FRAMES],
    latched: Option<&'static str>,
    /// An inherited latch is not new evidence and must not renew its lifetime.
    observed_at: Option<Instant>,
}
impl KeyframeLoss {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            ..Default::default()
        }
    }
    pub fn remembered(reason: &'static str) -> Self {
        Self {
            enabled: true,
            latched: Some(reason),
            ..Default::default()
        }
    }
    /// A frame about to be sent with this wire number.
    pub fn sending(&mut self, frame: u32, keyframe: bool) {
        if keyframe {
            self.keyframes[self.next] = Some(frame);
            self.next = (self.next + 1) % self.keyframes.len();
            self.keyframe_since_request = true;
            self.since_keyframe = 0;
        } else {
            self.since_keyframe = self.since_keyframe.saturating_add(1);
        }
    }
    /// The client could not repair a block of this frame. Returns the reason
    /// when this report slows the stream.
    pub fn unrecoverable(
        &mut self,
        frame: u32,
        last_sent: u32,
        period: Duration,
        now: Instant,
    ) -> Option<&'static str> {
        if self.keyframes.contains(&Some(frame)) {
            return self.latch("a keyframe arrived incomplete", now);
        }
        if !self.enabled
            || self.observed_at.is_some()
            || period.is_zero()
            || period.saturating_mul(last_sent.wrapping_sub(frame)) >= LOSS_WINDOW
        {
            return None;
        }
        // The normal feedback parser accepts up to 1024 previous frames. A
        // delayed train of old reports must not look like current congestion.
        // Keep this additional fence tied to the negotiated frame period.
        for loss in &mut self.recent_losses {
            if loss.is_some_and(|(_, at)| now.saturating_duration_since(at) >= LOSS_WINDOW) {
                *loss = None;
            }
        }
        if self
            .recent_losses
            .iter()
            .flatten()
            .any(|(seen, _)| *seen == frame)
        {
            return None;
        }
        let vacant = self.recent_losses.iter_mut().find(|loss| loss.is_none())?;
        *vacant = Some((frame, now));
        if self.recent_losses.iter().all(Option::is_some) {
            self.latch(
                "three recent video frames arrived incomplete within two seconds",
                now,
            )
        } else {
            None
        }
    }
    /// The client asked for a keyframe after losing frames. Returns the reason
    /// when this request slows the stream.
    pub fn requested(&mut self, now: Instant) -> Option<&'static str> {
        let repeated = self.keyframe_since_request
            && self.since_keyframe < DECODE_QUEUE
            && self
                .last_request
                .is_some_and(|at| now.saturating_duration_since(at) < REPEAT);
        self.last_request = Some(now);
        self.keyframe_since_request = false;
        if repeated {
            self.latch(
                "the client asked again for the keyframe it was just sent",
                now,
            )
        } else {
            None
        }
    }
    fn latch(&mut self, reason: &'static str, now: Instant) -> Option<&'static str> {
        if !self.enabled || self.observed_at.is_some() {
            return None;
        }
        self.observed_at = Some(now);
        self.latched = Some(reason);
        self.latched
    }
    pub fn latched(&self) -> bool {
        self.latched.is_some()
    }
    pub fn reason(&self) -> Option<&'static str> {
        self.latched
    }
    pub fn observed_at(&self) -> Option<Instant> {
        self.observed_at
    }
    /// The pacing rate: `base` from the route policy, or after confirmed video
    /// losses at most `safe`, the Wi-Fi route's rate for this stream.
    pub fn bps(&self, base: u64, safe: u64) -> u64 {
        if self.latched() { base.min(safe) } else { base }
    }
}

#[cfg(test)]
mod tests {
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
        // The stream's first keyframe is lost: the client asks for one.
        loss.sending(1, true);
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
            let payload = (f64::from(kbps) * 125. * period.as_secs_f64()) as usize
                * if keyframe { 5 } else { 1 };
            loss.sending(frame, keyframe);
            let bps = loss.bps(800_000_000, safe);
            let batch =
                (batch_payload_limit(bps, datagram, false, 64 * 1024) / datagram).clamp(1, 64);
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
                    shown_frames(false, fec_reports, 100e6, queue),
                    (0, false),
                    "{queue} B queue"
                );
                // Slowed after the first lost keyframe, or the repeated
                // request without reports, the third keyframe arrives.
                let (shown, latched) = shown_frames(true, fec_reports, 100e6, queue);
                assert!(
                    latched && shown >= 117,
                    "{queue} B queue, reports {fec_reports}: {shown}"
                );
            }
        }
    }

    #[test]
    fn a_fast_path_keeps_the_fast_rate() {
        for queue in [16 * 1024, 64 * 1024] {
            assert_eq!(shown_frames(true, true, 1e9, queue), (120, false));
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
        loss.sending(1, true);
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
}
