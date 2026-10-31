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
//! A lost first keyframe with nothing else wrong is only provisional: the
//! slow pace lifts after 10 s without loss, and is not remembered for later
//! sessions. A second latch, or loss of a different kind while it holds,
//! makes it sustained. Only sustained loss is remembered.
//!
//! A client whose decoder falls behind asks again too: Moonlight drops its
//! decode queue when it overflows and requests a keyframe. That request comes
//! only after the queue filled up behind the keyframe, so it is told apart by
//! the frames sent since the keyframe; slowing the send would not help it.
//! A client that announces FEC status reports every frame it lost, so for it
//! a repeated request with no report is its decoder asking again, at any
//! distance from the keyframe: its requests are not evidence at all.
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
//! the client's report of the same frame is not counted a second time. Loss
//! only the host saw is provisional: a transmit stall on the host (a game
//! starting, seen in #59) is not the route. It lifts after 10 s clean and is
//! never remembered; only loss the client reports makes a latch sustained.
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
/// The client's answer to a lost first keyframe is a keyframe it asked for,
/// sent this soon after the first: still the start. Any other keyframe is
/// not, whatever its age (an encoder rebuild two seconds in is the host's).
pub const STARTUP_SPAN: Duration = Duration::from_secs(1);
/// A provisional latch returns to the route rate after this long without
/// further evidence, once video has been flowing.
pub const PROVISIONAL_LIFT: Duration = Duration::from_secs(10);
/// "Busy video" for [`PROVISIONAL_LIFT`]: frames sent since the latch.
const LIFT_FRAMES: u32 = 120;

/// How much a latch says about the route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LatchKind {
    /// Only the stream's first keyframe was lost, with no other evidence: it
    /// is a burst at the start, not necessarily a slow route. It lifts after
    /// [`PROVISIONAL_LIFT`] clean and is not remembered for the next session.
    Startup,
    /// Only the host's socket refused video, and no client report backs it.
    /// Provisional like [`LatchKind::Startup`]: it lifts after
    /// [`PROVISIONAL_LIFT`] clean and is not remembered.
    Send,
    /// Repeated losses, a later keyframe lost, a second latch in the session,
    /// or a provisional latch confirmed by more evidence.
    Sustained,
}
impl LatchKind {
    /// Lifts after [`PROVISIONAL_LIFT`] clean and is never remembered.
    pub fn provisional(self) -> bool {
        matches!(self, LatchKind::Startup | LatchKind::Send)
    }
}

/// Why a report slowed the stream, and the frame numbers behind it, so the
/// next report from a user can be read without a debug build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Latched {
    pub reason: &'static str,
    pub detail: String,
    pub kind: LatchKind,
    /// More evidence arrived while a provisional latch held: the same latch,
    /// now sustained, not a new one.
    pub confirmed: bool,
    /// With `confirmed`, the kind of the latch the client's report confirmed.
    pub from: Option<LatchKind>,
    /// Only the host's socket refused video; the client reported nothing.
    pub host_only: bool,
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
    /// Frame, when, and whether the client reported it (not only the host's
    /// socket).
    recent_losses: [Option<(u32, Instant, bool)>; LOSS_FRAMES],
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
    /// The latest keyframe the host's socket refused, until a newer keyframe
    /// goes out: a request [`DECODE_QUEUE`] frames later still asks for it,
    /// after more given-up frames than `abandoned` holds.
    refused_keyframe: Option<u32>,
    /// Evidence set aside because of the host's own doing, for the log.
    ignored: Option<String>,
    latched: Option<&'static str>,
    /// What the current latch says about the route; set with `latched`.
    kind: Option<LatchKind>,
    /// An inherited latch is not new evidence and must not renew its lifetime.
    /// Cleared when a provisional latch lifts.
    observed_at: Option<Instant>,
    /// When client-reported evidence last made the latch sustained (a stream
    /// only remembers its route for this).
    sustained_at: Option<Instant>,
    /// Any latch happened in this stream, lifted or not.
    ever_latched: bool,
    /// A provisional latch lifted: the next latch is sustained.
    lifted: bool,
    /// The keyframe the provisional latch was made for, so that more reports
    /// about the same keyframe do not confirm it.
    latch_keyframe: Option<u32>,
    /// Frames sent since a provisional latch.
    frames_since_latch: u32,
    /// Whether each recent keyframe is part of the stream's start (the
    /// first, or the client's quick request for another), and when the first
    /// was sent.
    keyframe_startup: [bool; 8],
    first_keyframe_at: Option<Instant>,
    /// The last wire frame sent, and the one sent when the latch was made:
    /// only evidence about later frames can confirm a provisional latch.
    last_sent: Option<u32>,
    latch_frame: Option<u32>,
    /// The last loss counted while a provisional latch held; "clean for
    /// 10 s" is counted from it.
    last_loss_at: Option<Instant>,
    lifted_note: Option<String>,
    /// The client sends FEC status for frames it lost; its keyframe requests
    /// are not loss evidence.
    reports_losses: bool,
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
            kind: Some(LatchKind::Sustained),
            ..Default::default()
        }
    }
    /// The client announced FEC status (`ML_FF_FEC_STATUS`): it reports every
    /// frame it could not repair (0x5502), so a lost keyframe shows up as
    /// "arrived incomplete" or three unrecoverable frames. A repeated request
    /// with no such report comes from its decoder (Moonlight's
    /// `requestDecoderRefresh` sends the same 0x0302), not the network.
    pub fn set_reports_losses(&mut self, yes: bool) {
        self.reports_losses = yes;
    }
    pub fn reports_losses(&self) -> bool {
        self.reports_losses
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
            if first {
                self.first_keyframe_at = Some(now);
            }
            self.keyframe_startup[self.next] = first
                || (requested
                    && !host
                    && !loss
                    && self
                        .first_keyframe_at
                        .is_some_and(|at| now.saturating_duration_since(at) <= STARTUP_SPAN));
            self.keyframes[self.next] = Some(frame);
            self.next = (self.next + 1) % self.keyframes.len();
            // Whole unless the socket refuses it too ([`KeyframeLoss::abandoned`]).
            self.refused_keyframe = None;
            self.since_keyframe = 0;
        } else {
            self.since_keyframe = self.since_keyframe.saturating_add(1);
        }
        self.last_sent = Some(frame);
        self.lift_if_clean(now);
    }
    /// A provisional latch that saw no further evidence for
    /// [`PROVISIONAL_LIFT`] of busy video returns to the route rate. Called
    /// with each frame sent, so it needs no timer of its own.
    fn lift_if_clean(&mut self, now: Instant) {
        if !self.provisional() {
            return;
        }
        self.frames_since_latch = self.frames_since_latch.saturating_add(1);
        let quiet = self
            .recent_losses
            .iter()
            .flatten()
            .all(|(_, at, _)| now.saturating_duration_since(*at) >= LOSS_WINDOW);
        // Clean since the latch, or since the last loss counted after it.
        let since = self.observed_at.max(self.last_loss_at);
        let held = since
            .map(|at| now.saturating_duration_since(at))
            .unwrap_or_default();
        if held >= PROVISIONAL_LIFT && self.frames_since_latch >= LIFT_FRAMES && quiet {
            let what = if self.kind == Some(LatchKind::Send) {
                "only the host's socket refused video"
            } else {
                "the first keyframe was lost"
            };
            self.lifted_note = Some(format!("{what}, nothing since for {} s", held.as_secs()));
            self.latched = None;
            self.kind = None;
            self.observed_at = None;
            self.latch_keyframe = None;
            self.latch_frame = None;
            self.last_loss_at = None;
            self.lifted = true;
            self.recent_losses = Default::default();
        }
    }
    /// Set once when a provisional latch lifted, for the log.
    pub fn take_lifted(&mut self) -> Option<String> {
        self.lifted_note.take()
    }
    /// Whether the host sent this wire frame as a keyframe (recent ones only).
    pub fn is_keyframe(&self, frame: u32) -> bool {
        self.keyframes.contains(&Some(frame))
    }
    /// Whether this keyframe is the stream's first or the client's quick
    /// request for another.
    fn startup_keyframe(&self, frame: u32) -> bool {
        self.keyframes
            .iter()
            .position(|k| *k == Some(frame))
            .is_some_and(|i| self.keyframe_startup[i])
    }
    /// Whether this frame was sent after the provisional latch was made.
    fn after_latch(&self, frame: u32) -> bool {
        match self.latch_frame {
            Some(at) => {
                let ahead = frame.wrapping_sub(at);
                ahead != 0 && ahead < 1 << 31
            }
            None => true,
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
        if self.keyframes.contains(&Some(frame)) {
            self.refused_keyframe = Some(frame);
        }
        if !self.enabled {
            return None;
        }
        if self.keyframes.contains(&Some(frame)) {
            let detail = format!("the host could not send keyframe {frame} whole");
            return self.latch(
                "a keyframe was lost on send",
                detail,
                now,
                Some(frame),
                false,
            );
        }
        self.count_loss(
            frame,
            frame,
            period,
            now,
            "three recent video frames were lost on send within two seconds",
            "frames lost on send",
            false,
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
            return self.latch(
                "a keyframe arrived incomplete",
                detail,
                now,
                Some(frame),
                true,
            );
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
            true,
        )
    }
    /// Counts a frame that did not get through; the third distinct one within
    /// two seconds slows the stream. `reported` is the client's report, not
    /// only the host's socket.
    #[allow(clippy::too_many_arguments)]
    fn count_loss(
        &mut self,
        frame: u32,
        last_sent: u32,
        period: Duration,
        now: Instant,
        reason: &'static str,
        what: &str,
        reported: bool,
    ) -> Option<Latched> {
        // A sustained latch the client's reports backed has nothing left to
        // learn; a provisional one, or one only the host saw, still does.
        if (self.observed_at.is_some() && !self.provisional() && self.sustained_at.is_some())
            || period.is_zero()
            || period.saturating_mul(last_sent.wrapping_sub(frame)) >= LOSS_WINDOW
        {
            return None;
        }
        // A provisional latch learns only from frames sent after it.
        if self.provisional() && !self.after_latch(frame) {
            return None;
        }
        // The normal feedback parser accepts up to 1024 previous frames. A
        // delayed train of old reports must not look like current congestion.
        // Keep this additional fence tied to the negotiated frame period.
        for loss in &mut self.recent_losses {
            if loss.is_some_and(|(_, at, _)| now.saturating_duration_since(at) >= LOSS_WINDOW) {
                *loss = None;
            }
        }
        if self
            .recent_losses
            .iter()
            .flatten()
            .any(|(seen, _, _)| *seen == frame)
        {
            return None;
        }
        // Full only while a latch host-only evidence made still learns: the
        // newest loss replaces the oldest, so a client report can confirm it.
        let slot = match self.recent_losses.iter().position(Option::is_none) {
            Some(vacant) => vacant,
            None => self
                .recent_losses
                .iter()
                .enumerate()
                .min_by_key(|(_, loss)| loss.map(|(_, at, _)| at))
                .map(|(i, _)| i)?,
        };
        self.recent_losses[slot] = Some((frame, now, reported));
        if self.provisional() {
            self.last_loss_at = Some(now);
        }
        if self.recent_losses.iter().all(Option::is_some) {
            let frames: Vec<u32> = self
                .recent_losses
                .iter()
                .flatten()
                .map(|(frame, _, _)| *frame)
                .collect();
            self.latch(
                reason,
                format!("{what} {frames:?}; last frame sent {last_sent}"),
                now,
                None,
                reported,
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
        if self.reports_losses {
            self.ignored = Some(format!(
                "repeated keyframe request {} ms after the first from a client that reports its own losses: its decoder asked again",
                gap.unwrap_or_default().as_millis()
            ));
            return None;
        }
        let keyframe =
            self.keyframes[(self.next + self.keyframes.len() - 1) % self.keyframes.len()];
        let detail = format!(
            "second request {} ms after the first; keyframe {keyframe:?} was sent in between, {} frames before this request",
            gap.unwrap_or_default().as_millis(),
            self.since_keyframe
        );
        // A keyframe the host's socket refused never reached the client:
        // asking again for it is the host's loss, counted on send.
        let reported = !keyframe.is_some_and(|frame| {
            self.refused_keyframe == Some(frame) || self.abandoned.contains(&Some(frame))
        });
        self.latch(
            "the client asked again for the keyframe it was just sent",
            detail,
            now,
            keyframe,
            reported,
        )
    }
    /// `keyframe` is the one this evidence is about, if any. A latch about the
    /// stream's start, with nothing else seen, is provisional. `reported`:
    /// the client saw the loss, not only the host's socket; any loss the
    /// client reported within [`LOSS_WINDOW`] counts as well.
    fn latch(
        &mut self,
        reason: &'static str,
        detail: String,
        now: Instant,
        keyframe: Option<u32>,
        reported: bool,
    ) -> Option<Latched> {
        if !self.enabled {
            return None;
        }
        let reported =
            reported
                || self.recent_losses.iter().flatten().any(|(_, at, client)| {
                    *client && now.saturating_duration_since(*at) < LOSS_WINDOW
                });
        if self.observed_at.is_some() {
            let kind = self.kind;
            if !self.provisional() {
                // A latch only the host's socket made, after a lift or on
                // top of an inherited one, holds for the stream; the client's
                // report is what lets the next session remember it.
                if self.sustained_at.is_none() && reported {
                    self.sustained_at = Some(now);
                    return Some(Latched {
                        reason,
                        detail,
                        kind: LatchKind::Sustained,
                        confirmed: true,
                        from: kind,
                        host_only: false,
                    });
                }
                return None;
            }
            // A provisional latch learns, but not from another report about
            // the keyframe it was made for.
            if (keyframe.is_some() && keyframe == self.latch_keyframe)
                || keyframe.is_some_and(|frame| !self.after_latch(frame))
            {
                return None;
            }
            if !reported {
                // Host-only evidence never confirms; it restarts the clean 10 s.
                self.last_loss_at = Some(now);
                return None;
            }
            self.kind = Some(LatchKind::Sustained);
            self.sustained_at = Some(now);
            self.latch_keyframe = None;
            return Some(Latched {
                reason,
                detail,
                kind: LatchKind::Sustained,
                confirmed: true,
                from: kind,
                host_only: false,
            });
        }
        if !reported {
            // Only the host's socket refused video: a transmit stall on the
            // host is not the route. Provisional, unless the stream already
            // lifted one or inherited a latch; never remembered either way.
            let kind = if self.lifted || self.latched.is_some() {
                LatchKind::Sustained
            } else {
                LatchKind::Send
            };
            self.observed_at = Some(now);
            self.latched = Some(reason);
            self.kind = Some(kind);
            self.ever_latched = true;
            self.frames_since_latch = 0;
            self.latch_keyframe = keyframe.filter(|_| kind.provisional());
            self.latch_frame = self.last_sent;
            self.last_loss_at = None;
            return Some(Latched {
                reason,
                detail,
                kind,
                confirmed: false,
                from: None,
                host_only: true,
            });
        }
        let evidence = self
            .recent_losses
            .iter()
            .flatten()
            .any(|(_, at, _)| now.saturating_duration_since(*at) < LOSS_WINDOW);
        // An inherited latch, a second latch and evidence beyond the first
        // keyframe all say the route is slow, not that the start was rough.
        let provisional = self.latched.is_none()
            && !self.lifted
            && !evidence
            && keyframe.is_some_and(|frame| self.startup_keyframe(frame));
        let kind = if provisional {
            LatchKind::Startup
        } else {
            LatchKind::Sustained
        };
        self.observed_at = Some(now);
        self.latched = Some(reason);
        self.kind = Some(kind);
        self.ever_latched = true;
        self.frames_since_latch = 0;
        self.latch_keyframe = keyframe.filter(|_| provisional);
        self.latch_frame = self.last_sent;
        self.last_loss_at = None;
        if !provisional {
            self.sustained_at = Some(now);
        }
        Some(Latched {
            reason,
            detail,
            kind,
            confirmed: false,
            from: None,
            host_only: false,
        })
    }
    fn provisional(&self) -> bool {
        self.kind.is_some_and(LatchKind::provisional)
    }
    pub fn enabled(&self) -> bool {
        self.enabled
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
    /// When client-reported evidence last made the latch sustained: the only
    /// latch the next session inherits. A provisional latch, and one only the
    /// host's socket made, has none.
    pub fn sustained_at(&self) -> Option<Instant> {
        self.sustained_at
    }
    /// A latch happened in this stream, even one that lifted since.
    pub fn ever_latched(&self) -> bool {
        self.ever_latched
    }
    pub fn kind(&self) -> Option<LatchKind> {
        self.kind
    }
    /// The pacing rate: `base` from the route policy, or after confirmed video
    /// losses at most `safe`, the Wi-Fi route's rate for this stream.
    pub fn bps(&self, base: u64, safe: u64) -> u64 {
        if self.latched() { base.min(safe) } else { base }
    }
}

#[cfg(test)]
mod tests;
