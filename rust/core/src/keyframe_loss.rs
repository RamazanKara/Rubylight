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
        let reported = !keyframe.is_some_and(|frame| self.abandoned.contains(&Some(frame)));
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
            let payload = (f64::from(kbps) * 125. * period.as_secs_f64()) as usize
                * if keyframe { 5 } else { 1 };
            loss.sending_at(frame, keyframe, now);
            let bps = loss.bps(fast, safe);
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
            let whole = blocks.iter().all(|&(data, parity)| {
                (0..data + parity).filter(|_| lost_packet()).count() <= parity
            });
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
}
