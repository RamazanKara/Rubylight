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
//!
//! Only losses on the network count. The host also changes the stream itself:
//! a recreated encoder, a bitrate change, an HDR metadata update or a capture
//! change makes it send a keyframe nobody asked for, and the client reacts to
//! the break in the picture with requests of its own. Alt+Tab out of a
//! fullscreen game does this. Such a keyframe opens a short window in which
//! requests and ordinary-frame reports are not evidence, and it is never the
//! keyframe a later request "asks again" for. Keyframes are told apart by why
//! they were made, not by the order of events: the host marks the ones it
//! makes on its own ([`KeyframeLoss::host_keyframe`]), so a client request
//! that happens to come first does not turn them into answers. A keyframe
//! that answers loss (frames lost on send, a reference invalidation the
//! decoder may not have followed) opens no window: the losses around it are
//! what this guard counts. The stream's first keyframe opens a shorter one: a
//! client that has not decoded a picture yet asks again within about 100 ms
//! (seen in the host log at the start of several streams). A keyframe the
//! host sent whole and the client reports incomplete counts in or out of the
//! window.
//!
//! A frame the host could not send whole (a socket refused packets, such as
//! error 10055 at 800 Mbps) is loss on send. It counts like loss the client
//! reports: three within two seconds, or one keyframe, slow the stream, and
//! the client's report of the same frame is not counted a second time.
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
/// After a keyframe the host sent on its own, requests and reports about
/// ordinary frames are the client reacting to the host's change.
pub const HOST_CHANGE_WINDOW: Duration = Duration::from_secs(1);
/// The same for the stream's first keyframe, which the client can only ask
/// for again until it has decoded one. Long enough to cover the pair of
/// requests [`REPEAT`] could match, short enough that a first keyframe lost
/// on a slow port is still caught by the second one.
pub const STARTUP_WINDOW: Duration = REPEAT;

/// Why a report slowed the stream, and the frame numbers behind it, so the
/// next report from a user can be read without a debug build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Latched {
    pub reason: &'static str,
    pub detail: String,
}

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
    /// A client request no keyframe has answered yet.
    request_pending: bool,
    /// The next keyframe was made on the host's own initiative, whatever the
    /// client asked meanwhile (see [`KeyframeLoss::host_keyframe`]).
    host_reason: bool,
    /// The next keyframe answers loss the host saw (see
    /// [`KeyframeLoss::loss_keyframe`]).
    loss_reason: bool,
    /// Wire frame, end and length of the window of the latest host change.
    host_change: Option<(u32, Instant, Duration)>,
    /// Frames the host did not finish sending.
    abandoned: [Option<u32>; 8],
    next_abandoned: usize,
    /// Evidence set aside because of the host's own doing, for the log.
    ignored: Option<String>,
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
        self.sending_at(frame, keyframe, Instant::now());
    }
    pub fn sending_at(&mut self, frame: u32, keyframe: bool, now: Instant) {
        if keyframe {
            let first = self.keyframes.iter().all(Option::is_none);
            let host = std::mem::take(&mut self.host_reason);
            let loss = std::mem::take(&mut self.loss_reason);
            let requested = std::mem::take(&mut self.request_pending);
            // A keyframe the host made on its own breaks the picture whatever
            // the client asked at the same time. The stream's first starts
            // it: the client has decoded nothing yet and asks again until it
            // has. A keyframe nobody asked for and no loss explains is the
            // encoder's own (HDR metadata, a bitrate change).
            let window = if host {
                Some(HOST_CHANGE_WINDOW)
            } else if first {
                Some(STARTUP_WINDOW)
            } else if requested || loss {
                None
            } else {
                Some(HOST_CHANGE_WINDOW)
            };
            match window {
                Some(window) => self.host_change = Some((frame, now + window, window)),
                None if requested => self.keyframe_since_request = true,
                None => {}
            }
            self.keyframes[self.next] = Some(frame);
            self.next = (self.next + 1) % self.keyframes.len();
            self.since_keyframe = 0;
        } else {
            self.since_keyframe = self.since_keyframe.saturating_add(1);
        }
    }
    /// The next keyframe is one the host decided to make by itself: a
    /// recreated encoder, a frame beyond the packet limit. The client's
    /// reaction to the break in the picture is not network loss, even when it
    /// asked for a keyframe just before.
    pub fn host_keyframe(&mut self) {
        self.host_reason = true;
    }
    /// The next keyframe answers loss the host saw itself (frames its socket
    /// refused, a reference invalidation the decoder may not have followed).
    /// It opens no window: the losses around it are what this guard counts.
    pub fn loss_keyframe(&mut self) {
        self.loss_reason = true;
    }
    /// The last request cannot be answered with a keyframe (intra-only
    /// codec), so the next keyframe is not its answer.
    pub fn unanswerable(&mut self) {
        self.request_pending = false;
    }
    /// The host did not send all of this frame (a socket refused packets).
    /// That is loss on send: three such frames within two seconds, or one
    /// keyframe, slow the stream like loss the client reports, and the
    /// client's own report of the frame is not counted a second time.
    pub fn abandoned(&mut self, frame: u32, period: Duration, now: Instant) -> Option<Latched> {
        // Remembered even when the guard is off: [`KeyframeLoss::network_loss`]
        // tells the bitrate controller which frames the host already counted.
        self.abandoned[self.next_abandoned] = Some(frame);
        self.next_abandoned = (self.next_abandoned + 1) % self.abandoned.len();
        if !self.enabled {
            return None;
        }
        if self.keyframes.contains(&Some(frame)) {
            let detail = format!("the host could not send keyframe {frame} whole");
            return self.latch("a keyframe was lost on send", detail, now);
        }
        self.count_loss(
            frame,
            frame,
            period,
            now,
            "three recent video frames were lost on send within two seconds",
            "frames lost on send",
        )
    }
    /// Tests that start a stream and then lose a keyframe skip the wait.
    #[cfg(test)]
    pub(crate) fn settle(&mut self) {
        self.host_change = None;
    }
    /// Evidence the last report or request set aside, once.
    pub fn take_ignored(&mut self) -> Option<String> {
        self.ignored.take()
    }
    fn host_changed(&self, now: Instant) -> Option<(u32, Duration)> {
        self.host_change
            .filter(|(_, until, _)| now < *until)
            .map(|(frame, _, window)| (frame, window))
    }
    /// Whether the host stopped sending this frame part way. The packets the
    /// client never got are the host's loss, counted once on send.
    pub fn is_abandoned(&self, frame: u32) -> bool {
        self.abandoned.contains(&Some(frame))
    }
    /// Whether a frame the client reports incomplete is evidence about the
    /// network, by the same rules [`KeyframeLoss::unrecoverable`] applies:
    /// not a frame the host abandoned on send (counted there), and not an
    /// ordinary frame in the window after the host's own keyframe. A lost
    /// keyframe counts. Pure, and independent of whether the guard is on, so
    /// the bitrate controller shares the classification.
    pub fn network_loss(&self, frame: u32, now: Instant) -> bool {
        if self.abandoned.contains(&Some(frame)) {
            return false;
        }
        if self.keyframes.contains(&Some(frame)) {
            return true;
        }
        !self
            .host_changed(now)
            .is_some_and(|(from, _)| frame.wrapping_sub(from) < 1 << 31)
    }
    /// Whether a keyframe or reference invalidation request is evidence about
    /// the network: not the client reacting to the host's own keyframe, and
    /// not its decode queue overflowing behind the keyframe it just got
    /// (a request [`DECODE_QUEUE`] or more frames after a keyframe that
    /// answered the previous one). Call before [`KeyframeLoss::requested`].
    pub fn network_request(&self, now: Instant) -> bool {
        if self.host_changed(now).is_some() {
            return false;
        }
        let gap = self
            .last_request
            .map(|at| now.saturating_duration_since(at));
        let overflow = self.keyframe_since_request
            && self.since_keyframe >= DECODE_QUEUE
            && gap.is_some_and(|gap| gap < REPEAT);
        !overflow
    }
    /// The client could not repair a block of this frame. Returns the reason
    /// when this report slows the stream.
    pub fn unrecoverable(
        &mut self,
        frame: u32,
        last_sent: u32,
        period: Duration,
        now: Instant,
    ) -> Option<Latched> {
        if !self.enabled {
            return None;
        }
        if self.abandoned.contains(&Some(frame)) {
            self.ignored = Some(format!(
                "frame {frame} was not sent whole by the host (counted as lost on send)"
            ));
            return None;
        }
        if self.keyframes.contains(&Some(frame)) {
            let detail = format!(
                "the client could not rebuild keyframe {frame}; last frame sent {last_sent}; keyframes sent as {:?}",
                self.keyframes.iter().flatten().collect::<Vec<_>>()
            );
            return self.latch("a keyframe arrived incomplete", detail, now);
        }
        // Ordinary frames sent after the host changed the stream: bigger
        // pictures and a new decoder state, not yet a verdict on the route.
        if let Some((from, window)) = self.host_changed(now)
            && frame.wrapping_sub(from) < 1 << 31
        {
            self.ignored = Some(format!(
                "frame {frame} reported incomplete within {} ms of the host's own keyframe {from}",
                window.as_millis()
            ));
            return None;
        }
        self.count_loss(
            frame,
            last_sent,
            period,
            now,
            "three recent video frames arrived incomplete within two seconds",
            "unrecoverable frames",
        )
    }
    /// Counts a frame that did not get through; the third distinct one within
    /// two seconds slows the stream.
    fn count_loss(
        &mut self,
        frame: u32,
        last_sent: u32,
        period: Duration,
        now: Instant,
        reason: &'static str,
        what: &str,
    ) -> Option<Latched> {
        if self.observed_at.is_some()
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
            let frames: Vec<u32> = self
                .recent_losses
                .iter()
                .flatten()
                .map(|(frame, _)| *frame)
                .collect();
            self.latch(
                reason,
                format!("{what} {frames:?}; last frame sent {last_sent}"),
                now,
            )
        } else {
            None
        }
    }
    /// The client asked for a keyframe after losing frames. Returns the reason
    /// when this request slows the stream.
    pub fn requested(&mut self, now: Instant) -> Option<Latched> {
        if !self.enabled {
            return None;
        }
        let gap = self
            .last_request
            .map(|at| now.saturating_duration_since(at));
        let repeated = self.keyframe_since_request
            && self.since_keyframe < DECODE_QUEUE
            && gap.is_some_and(|gap| gap < REPEAT);
        self.keyframe_since_request = false;
        self.request_pending = true;
        if let Some((from, window)) = self.host_changed(now) {
            // The request reacts to the host's change; the next pair of
            // requests, after the window, is the one that can show a loss.
            self.last_request = None;
            self.ignored = Some(format!(
                "keyframe request within {} ms of the host's own keyframe {from}",
                window.as_millis()
            ));
            return None;
        }
        self.last_request = Some(now);
        if !repeated {
            return None;
        }
        let keyframe =
            self.keyframes[(self.next + self.keyframes.len() - 1) % self.keyframes.len()];
        let detail = format!(
            "second request {} ms after the first; keyframe {keyframe:?} was sent in between, {} frames before this request",
            gap.unwrap_or_default().as_millis(),
            self.since_keyframe
        );
        self.latch(
            "the client asked again for the keyframe it was just sent",
            detail,
            now,
        )
    }
    fn latch(&mut self, reason: &'static str, detail: String, now: Instant) -> Option<Latched> {
        if !self.enabled || self.observed_at.is_some() {
            return None;
        }
        self.observed_at = Some(now);
        self.latched = Some(reason);
        Some(Latched { reason, detail })
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
            loss.sending_at(frame, keyframe, now);
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
                // request without reports, the third keyframe arrives. The
                // requests of the first half second are the client's start
                // and do not count, so without reports it takes that long.
                let (shown, latched) = shown_frames(true, fec_reports, 100e6, queue);
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
        assert_eq!(loss.bps(FAST, SAFE), SAFE);
        // Nothing lost on send is counted once the guard is off.
        let mut off = KeyframeLoss::new(false);
        off.sending_at(1, true, now);
        assert_eq!(off.abandoned(1, PERIOD, now), None);
        assert!(!off.latched());
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
}
