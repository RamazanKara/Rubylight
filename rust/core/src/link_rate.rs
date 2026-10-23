//! Lowers the encoder bitrate when the network cannot carry the stream.
//!
//! A pure state machine: the session feeds it what the host sent and what the
//! client reports back, a periodic `tick` returns the encoder bitrate to use,
//! and every instant is passed in, so tests are deterministic.
//!
//! It acts only on loss that FEC cannot absorb (see the `*_FRAMES`, `LOSS_*`
//! and `KEYFRAME_REQUESTS` constants). On a healthy link it never returns a
//! target, never touches pacing and uses no RTT as a trigger, so the stream
//! takes exactly the path it takes without it. After a cut it ignores reports
//! for frames sent before the cut took effect (the [`Fence`]); the opt-in
//! `adaptive_bitrate` controller lacks this, and on the 2026-10-23 link-speed
//! runs it cut once a second (179 to 24.7 Mbps in 9 s on a link that carries
//! about 115) because late reports kept counting against the lower bitrate.
//!
//! Inactive (`State::Off`) when disabled in the configuration, for PyroWave,
//! and once the client sets the bitrate itself. The caller also passes
//! `enabled = false` while `host_adaptive_bitrate` is on
//! ([`crate::config::Config::link_rate_adaptation`] folds that in).
//!
//! # Calling it (brief 3)
//!
//! - One `Mutex<LinkRate>` per session, shared by the video send path, the
//!   control thread that reads client reports and the stream tick. Every
//!   method takes `&mut self` and does bounded work with no allocation after
//!   `new`.
//! - [`LinkRate::on_sent`] comes before the frame's first packet is sent, with
//!   the wire frame number the client will report, the packets (data and
//!   parity) and the wire bytes. A report that arrives before `on_sent` for its
//!   frame has no bytes to count against.
//! - Feed [`LinkRate::on_status`] only deduplicated FEC blocks (the same block
//!   repeated in several reports once); a frame with several blocks is
//!   counted once for E1, and its packets once for E2.
//! - From one client report call `on_status` first, then `on_unrecoverable`
//!   for the same frame, then `on_latch` (a fast-rate latch is a reaction to
//!   the same loss and starts the fence).
//! - Call `tick` from the stream tick with the current RTT, the FEC percentage
//!   in use, the frame period, the encoder bitrate now in effect and whether
//!   the picture is moving (`busy`: a static desktop sends almost nothing, so a
//!   probe sent then proves nothing). A returned kbps is applied through the
//!   encoder bitrate path; `None` means the requested rate. Log the result of
//!   `take_change` after each call.
//! - The wire frame counter must not restart under one controller. Use
//!   [`LinkRate::rebuild`] when the stream is reconfigured (new bitrate, FEC,
//!   mode or a restarted frame counter): it keeps a client takeover and
//!   forgets the rest.
//!
//! Units: the controller works in total wire bits per second (video with FEC
//! and framing plus the audio reserve), the unit of
//! [`WireBudget::total_bps`], and converts to encoder kbps at the current FEC
//! percentage with [`WireBudget::encoder_kbps`].
use std::time::{Duration, Instant};

use crate::{adaptive_bitrate::WireBudget, fec_status::Status, rtsp::Negotiated};

#[cfg(test)]
mod sim;
#[cfg(test)]
mod tests;

/// Every evidence test runs over this window.
pub const EVIDENCE_WINDOW: Duration = Duration::from_secs(2);
/// Evidence is evaluated at most this often.
pub const EVALUATE_EVERY: Duration = Duration::from_millis(100);
/// Distinct unrecoverable frames in the window that cut the bitrate (E1).
/// Matches the loss guard's "3 unrecoverable frames in 2 s" (Oct 17).
pub const UNRECOVERABLE_FRAMES: usize = 3;
/// Share of the packets the host sent that the FEC reports show missing, and
/// that cuts (E2). FEC repairs less than this (0.5 % random loss on the Oct 23
/// Wi-Fi arms gave about 2,400 recovered frames and no cut). The share is over
/// all packets sent, not only those of reported frames: a stream at the edge
/// of its link loses a few packets in one frame in ten, which is 5 % of that
/// frame and 0.5 % of the stream, and FEC rebuilds it.
pub const LOSS_SHARE_PERCENT: u64 = 5;
/// E2 needs the share in this many consecutive windows of [`LOSS_WINDOW`].
pub const LOSS_WINDOWS: u8 = 2;
pub const LOSS_WINDOW: Duration = Duration::from_millis(500);
/// A window in which the host sent fewer packets than this says nothing: a
/// quiet stream loses 5 % to a single packet.
pub const LOSS_MIN_PACKETS: u64 = 100;
/// Frames lost on send (WSAENOBUFS and the like) that cut (E3).
pub const SEND_LOSS_FRAMES: usize = 3;
/// Network-classified keyframe requests that cut, only from a client that
/// sends no FEC status (E4).
pub const KEYFRAME_REQUESTS: usize = 3;
/// A cut aims at this share of the delivered rate, FEC included.
pub const DELIVERED_PERCENT: u64 = 85;
/// One cut lowers the target by at least 15 % ...
pub const MAX_CUT_PERCENT: u64 = 85;
/// ... and at most by half, so one late or inflated estimate cannot collapse
/// the stream.
pub const MIN_CUT_PERCENT: u64 = 50;
/// Target after a cut when the loss share is unknown (E4 alone).
pub const UNKNOWN_SHARE_PERCENT: u64 = 70;
/// After a change, evidence from frames sent within this long is ignored so
/// the bottleneck queue can drain; the longer of this and
/// `2 x RTT + FENCE_MARGIN` applies.
pub const MIN_FENCE: Duration = Duration::from_millis(500);
pub const FENCE_MARGIN: Duration = Duration::from_millis(250);
/// Frames in the encoder when the bitrate changes (Oct 7: "two frames in the
/// encoder"); the fence frame is this far past the next frame sent.
pub const PIPELINE_FRAMES: u32 = 2;
/// Wait after a step down before the first probe.
pub const HOLD: Duration = Duration::from_secs(2);
/// A failed probe doubles the wait up to this. The link may have recovered
/// since, and only a probe can tell, so the wait stays short; a repeat probe is
/// cheap because it goes only [`NEAR_STEP_DIVISOR`] past the bad rate.
pub const MAX_HOLD: Duration = Duration::from_secs(8);
/// A probe that fails within [`KNEE_PERCENT`] of the previous bad rate means
/// the link has a fixed knee there. Each such failure doubles a wait, up to
/// [`KNEE_MAX_HOLD`], during which probes stay under [`KNEE_CAP_PERCENT`] of
/// the bad rate. Without it a fixed link pays a failed probe every 9 s forever:
/// on 45 fixed-link runs of 5 minutes (225 on 150, 120 on 80, 40 on 20, 20 on
/// 20 and 179 on 138 Mbit/s, three feedback delays, three burst/queue models)
/// 4.7 failed probes a minute and 1.00 % of frames lost fell to 1.4 a minute
/// and 0.77 % (0.76 % is the periodic 3x frame). 16 s left 2.5 a minute, 60 s
/// 0.9, but a 60 s wait keeps a recovered link at 90 % of its old rate that
/// long; 30 s is the middle.
pub const KNEE_MAX_HOLD: Duration = Duration::from_secs(30);
pub const KNEE_PERCENT: u64 = 5;
/// A step down from within this share of the bad rate is a knee failure too.
pub const KNEE_CUT_PERCENT: u64 = 90;
/// 95 % left the stream at the link's edge, where each periodic big frame
/// cost a cut (a window at 62 % of the link); 90 % never fell below 76 %.
pub const KNEE_CAP_PERCENT: u64 = 90;
/// Minimum time between probes (also at least the fence).
pub const PROBE_EVERY: Duration = Duration::from_millis(500);
/// Probe step as a fraction of the current rate: 1/8. A 25 % step loses more
/// than 20 % FEC can repair (2.2.19).
pub const PROBE_STEP_DIVISOR: u64 = 8;
/// Step within [`NEAR_BAD_PERCENT`] of the last bad rate, one step at a time
/// and confirmed before the next.
pub const NEAR_STEP_DIVISOR: u64 = 20;
pub const NEAR_BAD_PERCENT: u64 = 90;
/// A probe step counts as good after this long without evidence.
pub const CONFIRM_AFTER: Duration = EVIDENCE_WINDOW;
/// `bad` is forgotten after this long clean at 95 % of it or more.
pub const FORGET_BAD_AFTER: Duration = Duration::from_secs(60);
/// `Full` again after this long clean at the ceiling.
pub const FULL_AFTER: Duration = Duration::from_secs(10);
/// A control ping this recent counts as fresh feedback.
const PING_FRESH: Duration = Duration::from_secs(2);
const RING: usize = 1024;
const EVENT_CAP: usize = 32;
const PENDING_CAP: usize = 4;

/// A confirmed probe step, kept for one evidence window: loss that FEC
/// absorbs for two seconds shows up just after the step was confirmed.
#[derive(Clone, Copy)]
struct Confirmed {
    /// The good rate before the step.
    before: u64,
    /// The lowest rate confirmed.
    rate: u64,
    at: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Disabled, PyroWave, or the client owns the bitrate.
    Off,
    /// At the requested bitrate. `tick` returns `None`.
    Full,
    /// Below the requested bitrate because of the network, probing back up.
    Limited,
    /// At the minimum bitrate and still losing.
    Floor,
}
impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Off => "off",
            State::Full => "full",
            State::Limited => "limited",
            State::Floor => "floor",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    StepDown,
    Probe,
    ProbeFailed,
    Floor,
    Full,
}

/// The evidence behind a step down, for the log and the session warning.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Evidence {
    pub unrecoverable_frames: usize,
    pub send_losses: usize,
    pub keyframe_requests: usize,
    /// Consecutive 500 ms windows at or over [`LOSS_SHARE_PERCENT`].
    pub loss_windows: u8,
    /// Missing packets over packets sent, 0..1, since the last change.
    pub loss_share: f64,
}

/// One change of state or bitrate, taken with [`LinkRate::take_change`] and
/// logged by the host.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Change {
    pub kind: ChangeKind,
    /// `unrecoverable_frames`, `fec_loss_share`, `send_losses`,
    /// `keyframe_requests`, `probe`, `recovered`.
    pub reason: &'static str,
    pub from_kbps: u32,
    pub to_kbps: u32,
    /// Video wire rate the link delivered, kbps (0 when unknown).
    pub delivered_kbps: u32,
    pub evidence: Evidence,
}

#[derive(Clone, Copy, Default)]
struct Sent {
    frame: u32,
    packets: u32,
    bytes: u32,
    /// Packets the client reported missing (FEC status).
    missing: u32,
    at: Option<Instant>,
}

/// Evidence counts only for frames sent at or after `frame` and at or after
/// `at`.
#[derive(Clone, Copy)]
struct Fence {
    frame: Option<u32>,
    at: Instant,
}
impl Fence {
    fn admits(&self, frame: Option<u32>, sent: Instant) -> bool {
        sent >= self.at
            && match (self.frame, frame) {
                (Some(fence), Some(frame)) => frame.wrapping_sub(fence) < 1 << 31,
                _ => true,
            }
    }
}

#[derive(Default)]
struct Events {
    unrecoverable: Vec<(Instant, u32)>,
    send_losses: Vec<(Instant, u32)>,
    requests: Vec<Instant>,
    window: Option<Instant>,
    /// Packets reported missing and packets sent in the current E2 window.
    missing: u64,
    sent: u64,
    hot: u8,
    /// The oldest frame with any loss evidence; the delivered rate is
    /// measured from there, not diluted by the clean frames before the drop.
    first_loss: Option<u32>,
}
impl Events {
    fn clear(&mut self) {
        self.unrecoverable.clear();
        self.send_losses.clear();
        self.requests.clear();
        self.window = None;
        self.missing = 0;
        self.sent = 0;
        self.hot = 0;
        self.first_loss = None;
    }
    fn lost(&mut self, frame: u32) {
        if self
            .first_loss
            .is_none_or(|first| first.wrapping_sub(frame) < 1 << 31)
        {
            self.first_loss = Some(frame);
        }
    }
    fn distinct(events: &mut Vec<(Instant, u32)>, now: Instant, frame: u32) {
        if events.iter().any(|(_, seen)| *seen == frame) {
            return;
        }
        if events.len() >= EVENT_CAP {
            events.remove(0);
        }
        events.push((now, frame));
    }
    fn prune(&mut self, now: Instant) {
        let recent = |at: &Instant| now.saturating_duration_since(*at) < EVIDENCE_WINDOW;
        self.unrecoverable.retain(|(at, _)| recent(at));
        self.send_losses.retain(|(at, _)| recent(at));
        self.requests.retain(recent);
    }
    /// Closes the finished 500 ms windows of reported loss.
    fn roll(&mut self, now: Instant) {
        let Some(start) = self.window else {
            self.window = Some(now);
            self.missing = 0;
            self.sent = 0;
            return;
        };
        if now.saturating_duration_since(start) < LOSS_WINDOW {
            return;
        }
        let hot =
            self.sent >= LOSS_MIN_PACKETS && self.missing * 100 >= self.sent * LOSS_SHARE_PERCENT;
        self.hot = if hot { self.hot.saturating_add(1) } else { 0 };
        self.missing = 0;
        self.sent = 0;
        self.window = Some(now);
    }
}

#[derive(Clone, Copy)]
struct Step {
    rate: u64,
    at: Instant,
}

pub struct LinkRate {
    budget: Option<WireBudget>,
    reason: &'static str,
    client_owned: bool,
    state: State,
    ceiling_kbps: u32,
    floor_kbps: u32,
    /// Total wire bits per second the encoder is held to.
    current: u64,
    /// The last rate that stayed clean.
    good: u64,
    /// The last rate that did not.
    bad: Option<u64>,
    /// Probe steps above `good` not yet confirmed clean, oldest first.
    pending: [Option<Step>; PENDING_CAP],
    confirmed: Option<Confirmed>,
    hold: Duration,
    /// Knee wait, doubled by failures at the bad rate, reset past it.
    knee: Duration,
    /// Probes stay under [`KNEE_CAP_PERCENT`] of `bad` until then.
    knee_until: Option<Instant>,
    next_probe: Option<Instant>,
    at_ceiling: Option<Instant>,
    near_bad: Option<Instant>,
    fence: Option<Fence>,
    last_sent: Option<u32>,
    ring: Box<[Sent]>,
    events: Events,
    fec_status_seen: bool,
    last_ping: Option<Instant>,
    minimum_rtt: Option<Duration>,
    rtt: Duration,
    next_eval: Option<Instant>,
    cache: Option<((u64, usize, Duration), u32)>,
    delivered_bps: u64,
    last_change: Option<Instant>,
    change: Option<Change>,
    steps_down: u32,
    probes: u32,
    failed_probes: u32,
}

impl Default for LinkRate {
    fn default() -> Self {
        Self::off("disabled")
    }
}

impl LinkRate {
    fn off(reason: &'static str) -> Self {
        Self {
            budget: None,
            reason,
            client_owned: false,
            state: State::Off,
            ceiling_kbps: 0,
            floor_kbps: 0,
            current: 0,
            good: 0,
            bad: None,
            pending: [None; PENDING_CAP],
            confirmed: None,
            hold: HOLD,
            knee: HOLD,
            knee_until: None,
            next_probe: None,
            at_ceiling: None,
            near_bad: None,
            fence: None,
            last_sent: None,
            ring: Box::default(),
            events: Events::default(),
            fec_status_seen: false,
            last_ping: None,
            minimum_rtt: None,
            rtt: Duration::ZERO,
            next_eval: None,
            cache: None,
            delivered_bps: 0,
            last_change: None,
            change: None,
            steps_down: 0,
            probes: 0,
            failed_probes: 0,
        }
    }

    /// `enabled` is the configuration (`link_rate_adaptation` and no
    /// `host_adaptive_bitrate`). `max_kbps` is the user's `max_bitrate` (0 for
    /// none), `fec` the FEC percentage in use.
    pub fn new(enabled: bool, stream: &Negotiated, max_kbps: u32, fec: usize) -> Self {
        let mut this = Self::off("disabled");
        if !enabled {
            return this;
        }
        if stream.codec == 3 {
            this.reason = "pyrowave";
            return this;
        }
        let ceiling =
            stream
                .bitrate_kbps
                .min(500_000)
                .min(if max_kbps > 0 { max_kbps } else { u32::MAX });
        let budget = WireBudget::of(stream);
        let period = crate::reconfigure::Mode::of(stream).period();
        let Some(minimum) = budget.minimum_encoder_kbps(ceiling, fec, period) else {
            this.reason = "wire_budget_too_small";
            return this;
        };
        this.floor_kbps = ceiling.min(2_000).max(minimum).min(ceiling);
        this.ceiling_kbps = ceiling;
        this.budget = Some(budget);
        this.ring = vec![Sent::default(); RING].into_boxed_slice();
        for events in [&mut this.events.unrecoverable, &mut this.events.send_losses] {
            events.reserve_exact(EVENT_CAP);
        }
        this.events.requests.reserve_exact(EVENT_CAP);
        this.state = State::Full;
        this.reason = "healthy";
        this
    }

    /// A new controller for a reconfigured stream (new bitrate, FEC or
    /// codec). It keeps what the stream already decided: once the client set
    /// the bitrate, it stays in charge.
    pub fn rebuild(&self, enabled: bool, stream: &Negotiated, max_kbps: u32, fec: usize) -> Self {
        let mut this = Self::new(enabled, stream, max_kbps, fec);
        if self.client_owned {
            this.client_takeover();
        }
        this
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn ceiling_kbps(&self) -> u32 {
        self.ceiling_kbps
    }

    /// The client set the bitrate itself (`/bitrate`); this controller stays
    /// off until the stream is reconfigured.
    pub fn client_takeover(&mut self) {
        self.client_owned = true;
        self.state = State::Off;
        self.reason = "client_controlled";
        self.events.clear();
        self.pending = [None; PENDING_CAP];
        self.confirmed = None;
    }

    /// Record the video frame the host just sent: its wire frame number,
    /// packets (data and parity) and wire bytes. One store, no allocation.
    pub fn on_sent(&mut self, frame: u32, packets: u32, wire_bytes: u32, now: Instant) {
        if self.state == State::Off {
            return;
        }
        self.last_sent = Some(match self.last_sent {
            Some(last) if frame.wrapping_sub(last) >= 1 << 31 => last,
            _ => frame,
        });
        self.ring[frame as usize % RING] = Sent {
            frame,
            packets,
            bytes: wire_bytes,
            missing: 0,
            at: Some(now),
        };
        self.events.sent += u64::from(packets);
    }

    fn sent(&self, frame: u32) -> Option<Sent> {
        if self.ring.is_empty() {
            return None;
        }
        Some(self.ring[frame as usize % RING]).filter(|s| s.at.is_some() && s.frame == frame)
    }

    fn admits(&self, frame: Option<u32>, now: Instant) -> bool {
        let Some(fence) = self.fence else {
            return true;
        };
        let sent = frame.and_then(|f| self.sent(f)).and_then(|s| s.at);
        fence.admits(frame, sent.unwrap_or(now))
    }

    /// One validated, deduplicated block of a client FEC status (0x5502).
    /// Feeds the packet-loss share (E2) and records that this client reports
    /// FEC, which turns the keyframe-request trigger (E4) off.
    pub fn on_status(&mut self, status: &Status, now: Instant) {
        self.fec_status_seen = true;
        if !self.is_active() || !self.admits(Some(status.frame), now) {
            return;
        }
        if self.sent(status.frame).is_none() {
            return;
        }
        let total = u32::from(status.data) + u32::from(status.parity);
        let received = u32::from(status.received_data) + u32::from(status.received_parity);
        let missing = total.saturating_sub(received);
        if missing > 0 {
            self.events.missing += u64::from(missing);
            self.events.lost(status.frame);
            self.ring[status.frame as usize % RING].missing += missing;
        }
    }

    /// A distinct frame the client could not rebuild, already classified by
    /// `KeyframeLoss` (not inside a host-change window, not counted as lost on
    /// send). Feeds E1.
    pub fn on_unrecoverable(&mut self, frame: u32, now: Instant) {
        if self.is_active() && self.admits(Some(frame), now) {
            Events::distinct(&mut self.events.unrecoverable, now, frame);
            self.events.lost(frame);
        }
    }

    /// A frame the host could not send whole. Feeds E3.
    pub fn on_send_loss(&mut self, frame: u32, now: Instant) {
        if self.is_active() && self.admits(Some(frame), now) {
            Events::distinct(&mut self.events.send_losses, now, frame);
            self.events.lost(frame);
        }
    }

    /// A keyframe or RFI request. `network` is `KeyframeLoss`'s verdict that
    /// the network, not the host or the decoder queue, caused it. Feeds E4,
    /// and only from a client that has sent no FEC status.
    pub fn on_keyframe_request(&mut self, network: bool, now: Instant) {
        if !network || self.fec_status_seen || !self.is_active() || !self.admits(None, now) {
            return;
        }
        if self.events.requests.len() >= EVENT_CAP {
            self.events.requests.remove(0);
        }
        self.events.requests.push(now);
    }

    /// `KeyframeLoss` latched (paced like Wi-Fi). If the pacing before the
    /// latch was faster than the Wi-Fi rate the latch may be the whole fix
    /// (the Onn burst-loss port), so give it one fence before cutting.
    pub fn on_latch(&mut self, now: Instant, was_fast: bool) {
        if was_fast && self.is_active() {
            self.set_fence(now);
        }
    }

    /// A client control ping (`type 4`, 8 bytes), the proof that feedback is
    /// fresh.
    pub fn on_ping(&mut self, now: Instant, payload: &[u8]) {
        if payload.len() == 8 && payload[..4] == 4u32.to_le_bytes() {
            self.last_ping = Some(now);
        }
    }

    fn is_active(&self) -> bool {
        self.budget.is_some() && self.state != State::Off
    }

    fn set_fence(&mut self, now: Instant) {
        let wait = MIN_FENCE.max(self.rtt * 2 + FENCE_MARGIN);
        self.fence = Some(Fence {
            frame: self.last_sent.map(|f| f.wrapping_add(1 + PIPELINE_FRAMES)),
            at: now + wait,
        });
        self.events.clear();
    }

    fn fence_wait(&self) -> Duration {
        MIN_FENCE.max(self.rtt * 2 + FENCE_MARGIN)
    }

    /// Call every feedback tick (8 ms is fine; it evaluates every 100 ms). Returns
    /// the encoder bitrate to use while limited, `None` while `Full` or off.
    /// `fec` is the FEC percentage now in use, `current_kbps` the bitrate the
    /// encoder runs at, `busy` whether video traffic is busy (at least 60 % of
    /// the frames and bytes expected), and `rtt` the control RTT (zero when
    /// unknown).
    pub fn tick(
        &mut self,
        now: Instant,
        rtt: Duration,
        fec: usize,
        period: Duration,
        current_kbps: u32,
        busy: bool,
    ) -> Option<u32> {
        if !self.is_active() {
            return None;
        }
        self.rtt = rtt;
        if self.next_eval.is_none_or(|at| now >= at) {
            self.next_eval = Some(now + EVALUATE_EVERY);
            self.observe_rtt(now, rtt);
            self.events.roll(now);
            self.events.prune(now);
            if let Some(why) = self.triggered() {
                self.on_evidence(now, why, fec, period, current_kbps);
            } else if self.state != State::Full {
                self.advance(now, busy, fec, period);
            }
        }
        (self.state != State::Full).then(|| self.encoder_kbps(fec, period))
    }

    fn observe_rtt(&mut self, now: Instant, rtt: Duration) {
        if !rtt.is_zero() && self.ping_fresh(now) {
            self.minimum_rtt = Some(self.minimum_rtt.map_or(rtt, |m| m.min(rtt)));
        }
    }

    fn ping_fresh(&self, now: Instant) -> bool {
        self.last_ping
            .is_some_and(|at| now.saturating_duration_since(at) < PING_FRESH)
    }

    fn rtt_elevated(&self) -> bool {
        !self.rtt.is_zero()
            && self
                .minimum_rtt
                .is_some_and(|min| self.rtt > min + (min / 2).max(Duration::from_millis(8)))
    }

    fn encoder_kbps(&mut self, fec: usize, period: Duration) -> u32 {
        let key = (self.current, fec, period);
        if let Some((cached, kbps)) = self.cache
            && cached == key
        {
            return kbps;
        }
        let mut kbps = self
            .budget()
            .encoder_kbps(self.current, self.ceiling_kbps, fec, period)
            .clamp(self.floor_kbps, self.ceiling_kbps);
        // Less FEC frees wire bits for video, but only a probe may spend them.
        if let Some(((current, was_fec, was_period), cached)) = self.cache
            && current == self.current
            && period == was_period
            && fec < was_fec
        {
            kbps = kbps.min(cached);
        }
        self.cache = Some((key, kbps));
        kbps
    }

    fn kbps_for(&self, total: u64, fec: usize, period: Duration) -> u32 {
        self.budget()
            .encoder_kbps(total, self.ceiling_kbps, fec, period)
            .clamp(self.floor_kbps, self.ceiling_kbps)
    }

    fn budget(&self) -> WireBudget {
        self.budget.expect("active controller has a budget")
    }

    fn triggered(&self) -> Option<Trigger> {
        let e = &self.events;
        let evidence = Evidence {
            unrecoverable_frames: e.unrecoverable.len(),
            send_losses: e.send_losses.len(),
            keyframe_requests: e.requests.len(),
            loss_windows: e.hot,
            loss_share: 0.,
        };
        let (reason, share_known) = if evidence.unrecoverable_frames >= UNRECOVERABLE_FRAMES {
            ("unrecoverable_frames", true)
        } else if evidence.loss_windows >= LOSS_WINDOWS {
            ("fec_loss_share", true)
        } else if evidence.send_losses >= SEND_LOSS_FRAMES {
            ("send_losses", true)
        } else if !self.fec_status_seen && evidence.keyframe_requests >= KEYFRAME_REQUESTS {
            ("keyframe_requests", false)
        } else {
            return None;
        };
        Some(Trigger {
            reason,
            share_known,
            evidence,
        })
    }

    /// Frames sent since the fence and since the first frame with loss
    /// evidence, within the evidence window: the video wire rate the host
    /// offered, and the share of its packets the client reports missing.
    fn delivered(&self, now: Instant) -> Option<(u64, f64)> {
        let mut frames = 0usize;
        let (mut first, mut last): (Option<Instant>, Option<Instant>) = (None, None);
        let (mut bytes, mut first_bytes, mut packets, mut missing) = (0u64, 0u64, 0u64, 0u64);
        for s in self.ring.iter() {
            let Some(at) = s.at else { continue };
            if now.saturating_duration_since(at) >= EVIDENCE_WINDOW
                || !self.fence.is_none_or(|f| f.admits(Some(s.frame), at))
                || self
                    .events
                    .first_loss
                    .is_some_and(|first| s.frame.wrapping_sub(first) >= 1 << 31)
            {
                continue;
            }
            frames += 1;
            bytes += u64::from(s.bytes);
            packets += u64::from(s.packets);
            missing += u64::from(s.missing);
            if first.is_none_or(|f| at < f) {
                first = Some(at);
                first_bytes = u64::from(s.bytes);
            }
            if last.is_none_or(|l| at > l) {
                last = Some(at);
            }
        }
        let span = last?.saturating_duration_since(first?).as_secs_f64();
        if frames < 2 || span <= 0. || packets == 0 {
            return None;
        }
        let e = &self.events;
        let packet_share = (missing as f64 / packets as f64).min(1.);
        let frame_share =
            ((e.unrecoverable.len() + e.send_losses.len()) as f64 / frames as f64).min(1.);
        Some((
            ((bytes - first_bytes) as f64 * 8. / span) as u64,
            packet_share.max(frame_share),
        ))
    }

    fn floor_total(&self, fec: usize, period: Duration) -> u64 {
        self.budget().total_bps(self.floor_kbps, fec, period)
    }

    fn ceiling_total(&self, fec: usize, period: Duration) -> u64 {
        self.budget().total_bps(self.ceiling_kbps, fec, period)
    }

    fn record(&mut self, now: Instant, change: Change) {
        self.last_change = Some(now);
        self.change = Some(change);
    }

    /// Take the change made by the last `tick`, once, to log it.
    pub fn take_change(&mut self) -> Option<Change> {
        self.change.take()
    }

    fn on_evidence(
        &mut self,
        now: Instant,
        why: Trigger,
        fec: usize,
        period: Duration,
        current_kbps: u32,
    ) {
        let from_kbps = if self.state == State::Full {
            current_kbps.min(self.ceiling_kbps)
        } else {
            self.encoder_kbps(fec, period)
        };
        let budget = self.budget();
        if self.state == State::Full {
            self.current = budget
                .total_bps(from_kbps.max(1), fec, period)
                .min(self.ceiling_total(fec, period));
            self.good = self.current;
        }
        let mut evidence = why.evidence;
        // A probe is failing: go back to the last good rate, not below it.
        let recent = self
            .confirmed
            .filter(|c| now.saturating_duration_since(c.at) < EVIDENCE_WINDOW);
        let failing = match self.pending.iter().flatten().next() {
            Some(oldest) => Some((oldest.rate, self.good)),
            None => recent.map(|c| (c.rate, c.before)),
        };
        if let Some((failed, back_to)) = failing {
            self.pending = [None; PENDING_CAP];
            self.confirmed = None;
            let knee = self.bad.is_some_and(|bad| {
                failed * 100 >= bad * (100 - KNEE_PERCENT)
                    && failed * 100 <= bad * (100 + KNEE_PERCENT)
            });
            self.bad = Some(match self.bad {
                Some(bad) if knee => bad.min(failed),
                _ => failed,
            });
            self.good = back_to;
            self.current = back_to;
            self.hold = (self.hold * 2).min(MAX_HOLD);
            self.next_probe = Some(now + self.hold);
            self.knee_failed(now, knee);
            self.failed_probes += 1;
            self.at_ceiling = None;
            self.near_bad = None;
            self.set_fence(now);
            let to_kbps = self.encoder_kbps(fec, period);
            self.record(
                now,
                Change {
                    kind: ChangeKind::ProbeFailed,
                    reason: why.reason,
                    from_kbps,
                    to_kbps,
                    delivered_kbps: (self.delivered_bps / 1000) as u32,
                    evidence,
                },
            );
            return;
        }
        let cur = self.current;
        let floor = self.floor_total(fec, period).min(cur);
        if cur <= floor {
            // Nothing left to cut.
            if self.state != State::Floor {
                self.state = State::Floor;
                self.reason = "floor";
                self.record(
                    now,
                    Change {
                        kind: ChangeKind::Floor,
                        reason: why.reason,
                        from_kbps,
                        to_kbps: from_kbps,
                        delivered_kbps: 0,
                        evidence,
                    },
                );
            }
            self.set_fence(now);
            return;
        }
        let estimate = self.delivered(now).filter(|_| why.share_known);
        let target = match estimate {
            Some((sent, share)) => {
                evidence.loss_share = share;
                let delivered = (sent as f64 * (1. - share)) as u64;
                self.delivered_bps = delivered;
                (delivered + budget.audio_bps) * DELIVERED_PERCENT / 100
            }
            None => {
                self.delivered_bps = 0;
                cur * UNKNOWN_SHARE_PERCENT / 100
            }
        }
        .clamp(cur * MIN_CUT_PERCENT / 100, cur * MAX_CUT_PERCENT / 100)
        .max(floor);
        let knee = self
            .bad
            .is_some_and(|bad| cur * 100 >= bad * KNEE_CUT_PERCENT);
        self.bad = Some(self.bad.map_or(cur, |bad| bad.min(cur)));
        self.confirmed = None;
        self.current = target;
        self.good = target;
        // A cut from the knee is a failed probe that confirmed first.
        self.hold = if knee {
            (self.hold * 2).min(MAX_HOLD)
        } else {
            HOLD
        };
        self.next_probe = Some(now + self.hold.max(self.fence_wait()));
        self.knee_failed(now, knee);
        self.at_ceiling = None;
        self.near_bad = None;
        self.steps_down += 1;
        self.state = State::Limited;
        self.reason = why.reason;
        self.set_fence(now);
        let to_kbps = self.encoder_kbps(fec, period);
        self.record(
            now,
            Change {
                kind: ChangeKind::StepDown,
                reason: why.reason,
                from_kbps,
                to_kbps,
                delivered_kbps: (self.delivered_bps / 1000) as u32,
                evidence,
            },
        );
    }

    /// A failure at the bad rate (`knee`) doubles the wait that keeps probes
    /// under it; any other failure leaves the bad rate somewhere new.
    fn knee_failed(&mut self, now: Instant, knee: bool) {
        if knee {
            self.knee = (self.knee * 2).min(KNEE_MAX_HOLD);
            self.knee_until = Some(now + self.knee);
        } else {
            self.knee = HOLD;
            self.knee_until = None;
        }
    }

    /// The bad rate is gone (passed or forgotten): probe freely again.
    fn forget_knee(&mut self) {
        self.knee = HOLD;
        self.knee_until = None;
    }

    /// No evidence this round: confirm earlier probe steps, then probe or
    /// return to `Full`.
    fn advance(&mut self, now: Instant, busy: bool, fec: usize, period: Duration) {
        if !busy {
            // A static picture proves nothing: a step sent just before it
            // needs its two seconds of motion.
            for step in self.pending.iter_mut().flatten() {
                step.at = now;
            }
        }
        for slot in &mut self.pending {
            if let Some(step) = slot
                && now.saturating_duration_since(step.at) >= CONFIRM_AFTER
            {
                if step.rate > self.good {
                    // A failure now goes back to the rate before this step,
                    // not to where the climb began.
                    self.confirmed = Some(Confirmed {
                        before: self.good,
                        rate: step.rate,
                        at: now,
                    });
                }
                self.good = self.good.max(step.rate);
                self.hold = HOLD;
                if self.bad.is_none_or(|bad| step.rate > bad) {
                    self.bad = None;
                    self.knee = HOLD;
                    self.knee_until = None;
                }
                *slot = None;
            }
        }
        self.pending.sort_by_key(|s| s.is_none());
        // Quiet video, stale pings or a queue in the path prove nothing.
        if !(busy && self.ping_fresh(now) && !self.rtt.is_zero() && !self.rtt_elevated()) {
            return;
        }
        let ceiling = self.ceiling_total(fec, period);
        let unconfirmed = self.pending.iter().any(Option::is_some);
        if self.current >= ceiling {
            let since = *self.at_ceiling.get_or_insert(now);
            if !unconfirmed && now.saturating_duration_since(since) >= FULL_AFTER {
                self.finish(now, fec, period);
            }
            return;
        }
        match (self.bad, self.near_bad) {
            (Some(bad), None) if self.current * 100 >= bad * 95 => self.near_bad = Some(now),
            (Some(bad), Some(since)) => {
                if self.current * 100 < bad * 95 {
                    self.near_bad = None;
                } else if now.saturating_duration_since(since) >= FORGET_BAD_AFTER && !unconfirmed {
                    self.bad = None;
                    self.near_bad = None;
                    self.forget_knee();
                }
            }
            _ => {}
        }
        if self.next_probe.is_some_and(|at| now < at) || self.pending[PENDING_CAP - 1].is_some() {
            return;
        }
        let mut divisor = PROBE_STEP_DIVISOR;
        let wait = PROBE_EVERY.max(self.fence_wait());
        if let Some(bad) = self.bad
            && (self.current + self.current / PROBE_STEP_DIVISOR) * 100 >= bad * NEAR_BAD_PERCENT
        {
            if unconfirmed {
                return;
            }
            divisor = NEAR_STEP_DIVISOR;
        }
        // The packet count per frame quantizes the encoder bitrate; a step that
        // changes nothing is no probe.
        let step = (self.current / divisor).max(1);
        let level = self.kbps_for(self.current, fec, period);
        // On a known knee, probes stop just under it until the wait is over.
        let limit = match (self.bad, self.knee_until) {
            (Some(bad), Some(until)) if now < until => {
                ceiling.min((bad * KNEE_CAP_PERCENT).div_ceil(100))
            }
            _ => ceiling,
        };
        let mut next = self.current;
        while next < limit && self.kbps_for(next, fec, period) <= level {
            next += step;
        }
        let next = next.min(limit);
        if next <= self.current || (limit < ceiling && self.kbps_for(next, fec, period) <= level) {
            return;
        }
        let from_kbps = self.encoder_kbps(fec, period);
        self.current = next;
        self.next_probe = Some(now + wait);
        if let Some(slot) = self.pending.iter_mut().find(|s| s.is_none()) {
            *slot = Some(Step {
                rate: next,
                at: now,
            });
        }
        self.probes += 1;
        if self.state == State::Floor {
            self.state = State::Limited;
        }
        let to_kbps = self.encoder_kbps(fec, period);
        self.record(
            now,
            Change {
                kind: ChangeKind::Probe,
                reason: "probe",
                from_kbps,
                to_kbps,
                delivered_kbps: 0,
                evidence: Evidence::default(),
            },
        );
    }

    fn finish(&mut self, now: Instant, fec: usize, period: Duration) {
        let kbps = self.encoder_kbps(fec, period);
        self.state = State::Full;
        self.reason = "healthy";
        self.bad = None;
        self.hold = HOLD;
        self.forget_knee();
        self.at_ceiling = None;
        self.near_bad = None;
        self.pending = [None; PENDING_CAP];
        self.confirmed = None;
        self.record(
            now,
            Change {
                kind: ChangeKind::Full,
                reason: "recovered",
                from_kbps: kbps,
                to_kbps: kbps,
                delivered_kbps: 0,
                evidence: Evidence::default(),
            },
        );
    }

    /// State for the session API and the `stream timings` log line.
    pub fn snapshot(&self, now: Instant) -> serde_json::Value {
        let current_kbps = match self.cache {
            Some((_, kbps)) if matches!(self.state, State::Limited | State::Floor) => kbps,
            _ => self.ceiling_kbps,
        };
        serde_json::json!({
            "state": self.state.name(),
            "reason": self.reason,
            "ceiling_kbps": self.ceiling_kbps,
            "current_kbps": current_kbps,
            "delivered_kbps": self.delivered_bps / 1000,
            "bad_kbps": self.bad.map_or(0, |bad| self.kbps_of(bad)),
            "steps_down": self.steps_down,
            "probes": self.probes,
            "failed_probes": self.failed_probes,
            "last_change_ms_ago": self
                .last_change
                .map(|at| now.saturating_duration_since(at).as_millis() as u64),
        })
    }

    /// Rough encoder kbps for a wire rate at the last FEC seen; only for the
    /// snapshot.
    fn kbps_of(&self, wire: u64) -> u32 {
        match (self.cache, self.budget) {
            (Some(((_, fec, period), _)), Some(budget)) => {
                budget.encoder_kbps(wire, self.ceiling_kbps, fec, period)
            }
            _ => 0,
        }
    }
}

struct Trigger {
    reason: &'static str,
    share_known: bool,
    evidence: Evidence,
}
