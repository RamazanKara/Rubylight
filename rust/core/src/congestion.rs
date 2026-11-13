//! Packet-level congestion control from the client's per-packet arrival feedback (0x5547).
//!
//! A pure state machine: the session hands it each report joined with the host's send times
//! ([`PacketResult`]), a periodic [`Congestion::tick`] returns the video wire rate to hold and
//! the pace to send at, and every instant is a microsecond count passed in, so tests are
//! deterministic. Off unless `congestion_control` is set and the client sends feedback.
//!
//! It follows the Google Congestion Control design (draft-ietf-rmcat-gcc-02, the delay-based
//! half as WebRTC's `goog_cc` runs it), written for this host:
//!
//! - **Inter-arrival groups**: packets sent within [`BURST_US`] of a group's first form one
//!   group; each completed group gives a send delta and an arrival delta against the one before.
//! - **Trendline filter**: the accumulated one-way delay variation, smoothed, and its slope over
//!   the last [`TRENDLINE_WINDOW`] groups by least squares.
//! - **Overuse detector**: the slope, scaled, against an adaptive threshold that tracks it
//!   (fast up, slow down), so a link whose delay always wobbles does not read as congested.
//!   Overuse needs [`OVERUSE_TIME_MS`] of it on two samples in a row with a slope that is not
//!   falling.
//! - **AIMD rate**: overuse cuts the rate to [`DECREASE_FACTOR`] of what the client received,
//!   at most once a round trip; normal use raises it, multiplicatively far below the link's
//!   last known capacity, additively near it; underuse holds it.
//! - **Loss**: the share of packets the client never received, before error correction. Loss
//!   the stream's FEC absorbs is ignored; above [`LOSS_HIGH`] or three quarters of the parity
//!   share it cuts the rate by half the loss, at most once per [`LOSS_CUT_GAP_US`] and round
//!   trip. Loss after FEC stays the link-rate controller's (`link_rate`), which keeps running:
//!   the session applies the lower of the two.
//!
//! The rate starts at the requested one, so a link that carries the stream never sees it change.
//!
//! **Pacing.** A group the sender put out faster than it arrived ([`BOTTLENECK_RATIO`]) shows
//! the bottleneck's rate. Once [`PACE_SAMPLES`] such groups came within [`PACE_WINDOW_US`], the
//! controller paces the video at [`PACE_FRACTION`] of their smoothed rate, never below
//! [`PACE_HEADROOM`] times the target, so bursts queue on the host rather than in the access
//! point. A group sent at about that pace that arrives as fast keeps the estimate; one sent well
//! above it that arrives as fast raises it. Without such evidence it leaves pacing alone.
//!
//! **Measurement.** It keeps the queueing delay each received packet saw (its one-way delay
//! over the lowest of the last [`BASE_DELAY_WINDOW_US`]; the clocks need not agree, only tick
//! at the same rate) as percentiles per window and per stream, the recovery time of each cut
//! (back to [`RECOVERED_FRACTION`] of the rate before it), and the feedback it took.
use std::collections::VecDeque;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub use rubylight_protocol::transport_feedback::{
    MAX_ENTRIES, MAX_GAP_REPORTS, TRANSPORT_FEEDBACK_MESSAGE_TYPE, TransportFeedback,
};

/// Packets sent within this of a group's first belong to it (WebRTC's `kBurstDeltaThresholdMs`).
pub const BURST_US: u64 = 5_000;
/// Groups in the trendline's regression (WebRTC's `kDefaultTrendlineWindowSize`).
pub const TRENDLINE_WINDOW: usize = 20;
/// Smoothing of the accumulated delay (WebRTC's `kDefaultTrendlineSmoothingCoeff`).
pub const SMOOTHING: f64 = 0.9;
/// Gain on the slope before it meets the threshold (`kDefaultTrendlineThresholdGain`).
pub const THRESHOLD_GAIN: f64 = 4.0;
/// The slope's sample count is capped at this when scaled (`kMinNumDeltas`).
const MAX_DELTAS: u32 = 60;
/// The adaptive threshold: start, bounds and gains (`kOverUsingTimeThreshold` and the
/// detector's `k_up_`, `k_down_`).
pub const INITIAL_THRESHOLD: f64 = 12.5;
const MIN_THRESHOLD: f64 = 6.;
const MAX_THRESHOLD: f64 = 600.;
const K_UP: f64 = 0.0087;
const K_DOWN: f64 = 0.039;
/// A slope this far past the threshold is an outlier the threshold does not follow.
const MAX_ADAPT_OFFSET: f64 = 15.;
/// Time over the threshold before it is overuse.
pub const OVERUSE_TIME_MS: f64 = 10.;
/// Overuse cuts to this share of what the client received.
pub const DECREASE_FACTOR: f64 = 0.85;
/// One cut takes at most half the rate.
pub const MAX_CUT: f64 = 0.5;
/// Far below the link's last capacity the rate grows this much a second.
pub const MULTIPLICATIVE_INCREASE: f64 = 0.08;
/// Near it, this share of the rate a second.
pub const ADDITIVE_INCREASE: f64 = 0.02;
/// The least an additive step adds a second.
const MIN_ADDITIVE_BPS: f64 = 100_000.;
/// The client received this much more than the last capacity without overuse: forget it.
const FORGET_CAPACITY: f64 = 1.15;
/// "Near" the capacity: within this share of it.
const NEAR_CAPACITY: f64 = 0.9;
/// The rate grows only while the client receives at least this share of it, plus 10 kbps:
/// a static picture sends little, and a rate far above what is sent proves nothing.
const APP_LIMITED: f64 = 1.5;
/// The window the received rate is measured over.
pub const ACKED_WINDOW_US: u64 = 500_000;
/// Loss above this share cuts the rate, unless FEC carries more (see [`FEC_ABSORBS`]).
pub const LOSS_HIGH: f64 = 0.10;
/// Loss below this lets the loss limit grow again.
pub const LOSS_LOW: f64 = 0.02;
/// Loss up to this share of the parity share is FEC's to repair.
pub const FEC_ABSORBS: f64 = 0.75;
/// Loss is judged over this window, once it holds [`LOSS_MIN_PACKETS`].
pub const LOSS_WINDOW_US: u64 = 1_000_000;
pub const LOSS_MIN_PACKETS: u64 = 100;
/// The least time between two loss cuts (plus a round trip).
pub const LOSS_CUT_GAP_US: u64 = 300_000;
/// Without feedback for this long the controller lets go: the rate returns to the request.
pub const STALE_US: u64 = 2_000_000;
/// A group arrives this much slower than it was sent: the bottleneck spread it.
pub const BOTTLENECK_RATIO: f64 = 0.8;
pub const PACE_SAMPLES: usize = 8;
pub const PACE_WINDOW_US: u64 = 2_000_000;
pub const PACE_FRACTION: f64 = 0.95;
pub const PACE_HEADROOM: f64 = 1.25;
/// A group needs this many packets to say anything about the bottleneck.
const PACE_MIN_PACKETS: u32 = 4;
/// Queueing delay is measured against the lowest one-way delay of this window.
pub const BASE_DELAY_WINDOW_US: u64 = 10_000_000;
/// A cut is over once the rate is back to this share of the rate before it.
pub const RECOVERED_FRACTION: f64 = 0.9;
/// A recovery not over after this counts as this long.
pub const RECOVERY_CAP_US: u64 = 30_000_000;
/// Queueing delay histogram: buckets of this width up to the last, which takes the rest.
const DELAY_BUCKET_US: u64 = 250;
const DELAY_BUCKETS: usize = 2_000;
/// Recovery times kept for the stream summary.
const RECOVERIES: usize = 64;

/// Whether the delay is rising, flat or falling.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Usage {
    #[default]
    Normal,
    Overusing,
    Underusing,
}

impl Usage {
    pub fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Overusing => "overusing",
            Self::Underusing => "underusing",
        }
    }
}

/// What the rate does next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RateState {
    #[default]
    Hold,
    Increase,
    Decrease,
}

impl RateState {
    pub fn name(self) -> &'static str {
        match self {
            Self::Hold => "hold",
            Self::Increase => "increase",
            Self::Decrease => "decrease",
        }
    }
}

/// One packet of a report, joined with the host's record of it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PacketResult {
    /// When the host handed it to the socket, host µs.
    pub sent_us: u64,
    /// When it arrived, client µs; `None` when it did not.
    pub arrival_us: Option<u64>,
    /// Its bytes on the wire.
    pub wire_bytes: u32,
}

/// One report, joined.
#[derive(Clone, Copy, Debug, Default)]
pub struct Feedback<'a> {
    /// In id order, which is send order.
    pub packets: &'a [PacketResult],
    /// Every `stride`-th packet is reported; counts scale by it.
    pub stride: u16,
    /// Packets that arrived after their report.
    pub late: u16,
    /// Packets the client skipped reporting before this report, counted as lost.
    pub skipped: u32,
}

/// A change worth a log line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Change {
    pub kind: ChangeKind,
    pub from_bps: u64,
    pub to_bps: u64,
    /// The received rate the cut aimed at, or the loss share for a loss cut.
    pub acked_bps: u64,
    pub loss: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    /// Delay rose: overuse.
    Overuse,
    /// Loss past what FEC repairs.
    Loss,
    /// Back at the requested rate.
    Restored,
    /// Feedback stopped; the requested rate again.
    Stale,
}

impl ChangeKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Overuse => "overuse",
            Self::Loss => "loss",
            Self::Restored => "restored",
            Self::Stale => "stale",
        }
    }
}

/// What a tick decided.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Decision {
    /// The video wire rate to hold, bps; `None` for no limit (the requested rate).
    pub video_bps: Option<u64>,
    /// The pace to send video at, bps; `None` leaves pacing to the host's policy.
    pub pace_bps: Option<u64>,
}

#[derive(Clone, Copy, Debug)]
struct Group {
    first_send: u64,
    last_send: u64,
    first_arrival: u64,
    last_arrival: u64,
    bytes: u64,
    packets: u32,
}

impl Group {
    fn new(p: &PacketResult, arrival: u64) -> Self {
        Self {
            first_send: p.sent_us,
            last_send: p.sent_us,
            first_arrival: arrival,
            last_arrival: arrival,
            bytes: u64::from(p.wire_bytes),
            packets: 1,
        }
    }
}

/// Groups packets by send time and yields the deltas between consecutive groups.
#[derive(Clone, Debug, Default)]
struct InterArrival {
    current: Option<Group>,
    previous: Option<Group>,
}

/// The deltas between two groups, ms, and the later group.
struct Deltas {
    send_ms: f64,
    arrival_ms: f64,
    group: Group,
}

impl InterArrival {
    fn add(&mut self, p: &PacketResult, arrival: u64) -> Option<Deltas> {
        let Some(current) = self.current.as_mut() else {
            self.current = Some(Group::new(p, arrival));
            return None;
        };
        if p.sent_us < current.first_send {
            // Sent before the open group: a late join, which says nothing about the trend.
            return None;
        }
        if p.sent_us - current.first_send <= BURST_US {
            current.last_send = current.last_send.max(p.sent_us);
            current.first_arrival = current.first_arrival.min(arrival);
            current.last_arrival = current.last_arrival.max(arrival);
            current.bytes += u64::from(p.wire_bytes);
            current.packets += 1;
            return None;
        }
        let done = *current;
        self.current = Some(Group::new(p, arrival));
        let previous = self.previous.replace(done)?;
        let send = done.last_send.saturating_sub(previous.last_send) as f64 / 1000.;
        let arrival = done.last_arrival as f64 / 1000. - previous.last_arrival as f64 / 1000.;
        if arrival < 0. {
            // Reordered groups: skip the pair.
            return None;
        }
        Some(Deltas {
            send_ms: send,
            arrival_ms: arrival,
            group: done,
        })
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

/// The trendline filter and the overuse detector.
#[derive(Clone, Debug)]
struct Trendline {
    window: VecDeque<(f64, f64)>,
    accumulated: f64,
    smoothed: f64,
    first_arrival_ms: Option<f64>,
    deltas: u32,
    trend: f64,
    previous_trend: f64,
    modified: f64,
    threshold: f64,
    last_threshold_ms: Option<f64>,
    time_over: f64,
    overuse_count: u32,
    usage: Usage,
}

impl Default for Trendline {
    fn default() -> Self {
        Self {
            window: VecDeque::with_capacity(TRENDLINE_WINDOW + 1),
            accumulated: 0.,
            smoothed: 0.,
            first_arrival_ms: None,
            deltas: 0,
            trend: 0.,
            previous_trend: 0.,
            modified: 0.,
            threshold: INITIAL_THRESHOLD,
            last_threshold_ms: None,
            time_over: -1.,
            overuse_count: 0,
            usage: Usage::Normal,
        }
    }
}

/// The least-squares slope of `points`; `None` when x does not vary.
fn slope(points: &VecDeque<(f64, f64)>) -> Option<f64> {
    let n = points.len() as f64;
    let (sx, sy) = points
        .iter()
        .fold((0., 0.), |(sx, sy), &(x, y)| (sx + x, sy + y));
    let (mx, my) = (sx / n, sy / n);
    let (num, den) = points.iter().fold((0., 0.), |(num, den), &(x, y)| {
        (num + (x - mx) * (y - my), den + (x - mx) * (x - mx))
    });
    (den != 0.).then(|| num / den)
}

impl Trendline {
    fn update(&mut self, d: &Deltas) -> Usage {
        let delta = d.arrival_ms - d.send_ms;
        self.deltas = (self.deltas + 1).min(1000);
        self.accumulated += delta;
        self.smoothed = SMOOTHING * self.smoothed + (1. - SMOOTHING) * self.accumulated;
        let arrival_ms = d.group.last_arrival as f64 / 1000.;
        let first = *self.first_arrival_ms.get_or_insert(arrival_ms);
        self.window.push_back((arrival_ms - first, self.smoothed));
        if self.window.len() > TRENDLINE_WINDOW {
            self.window.pop_front();
        }
        if self.window.len() == TRENDLINE_WINDOW
            && let Some(trend) = slope(&self.window)
        {
            self.trend = trend;
        }
        self.detect(d.send_ms, arrival_ms);
        self.usage
    }

    fn detect(&mut self, send_ms: f64, now_ms: f64) {
        if self.deltas < 2 {
            self.usage = Usage::Normal;
            return;
        }
        self.modified = f64::from(self.deltas.min(MAX_DELTAS)) * self.trend * THRESHOLD_GAIN;
        if self.modified > self.threshold {
            if self.time_over < 0. {
                self.time_over = send_ms / 2.;
            } else {
                self.time_over += send_ms;
            }
            self.overuse_count += 1;
            if self.time_over > OVERUSE_TIME_MS
                && self.overuse_count > 1
                && self.trend >= self.previous_trend
            {
                self.time_over = 0.;
                self.overuse_count = 0;
                self.usage = Usage::Overusing;
            }
        } else if self.modified < -self.threshold {
            self.time_over = -1.;
            self.overuse_count = 0;
            self.usage = Usage::Underusing;
        } else {
            self.time_over = -1.;
            self.overuse_count = 0;
            self.usage = Usage::Normal;
        }
        self.previous_trend = self.trend;
        self.adapt_threshold(now_ms);
    }

    fn adapt_threshold(&mut self, now_ms: f64) {
        let last = *self.last_threshold_ms.get_or_insert(now_ms);
        let magnitude = self.modified.abs();
        if magnitude > self.threshold + MAX_ADAPT_OFFSET {
            self.last_threshold_ms = Some(now_ms);
            return;
        }
        let k = if magnitude < self.threshold {
            K_DOWN
        } else {
            K_UP
        };
        let dt = (now_ms - last).clamp(0., 100.);
        self.threshold += k * (magnitude - self.threshold) * dt;
        self.threshold = self.threshold.clamp(MIN_THRESHOLD, MAX_THRESHOLD);
        self.last_threshold_ms = Some(now_ms);
    }
}

/// A histogram of queueing delays, µs.
#[derive(Clone, Debug)]
pub struct DelayHistogram {
    buckets: Vec<u32>,
    count: u64,
}

impl Default for DelayHistogram {
    fn default() -> Self {
        Self {
            buckets: vec![0; DELAY_BUCKETS],
            count: 0,
        }
    }
}

impl DelayHistogram {
    fn add(&mut self, delay_us: u64) {
        let bucket = ((delay_us / DELAY_BUCKET_US) as usize).min(DELAY_BUCKETS - 1);
        self.buckets[bucket] = self.buckets[bucket].saturating_add(1);
        self.count += 1;
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    /// The `p`th percentile (0-100) in ms, the middle of its bucket; `None` while empty.
    pub fn percentile_ms(&self, p: f64) -> Option<f64> {
        if self.count == 0 {
            return None;
        }
        let rank = ((p / 100. * self.count as f64).ceil() as u64).clamp(1, self.count);
        let mut seen = 0;
        for (i, &n) in self.buckets.iter().enumerate() {
            seen += u64::from(n);
            if seen >= rank {
                return Some((i as f64 + 0.5) * DELAY_BUCKET_US as f64 / 1000.);
            }
        }
        None
    }

    fn clear(&mut self) {
        self.buckets.fill(0);
        self.count = 0;
    }
}

/// The lowest one-way delay over [`BASE_DELAY_WINDOW_US`], in one-second segments.
#[derive(Clone, Debug, Default)]
struct BaseDelay {
    segments: VecDeque<(u64, i64)>,
}

impl BaseDelay {
    fn add(&mut self, now_us: u64, delay: i64) -> i64 {
        let segment = now_us / 1_000_000;
        match self.segments.back_mut() {
            Some((at, min)) if *at == segment => *min = (*min).min(delay),
            _ => self.segments.push_back((segment, delay)),
        }
        let keep = BASE_DELAY_WINDOW_US / 1_000_000;
        while self
            .segments
            .front()
            .is_some_and(|(at, _)| at + keep < segment)
        {
            self.segments.pop_front();
        }
        self.segments
            .iter()
            .map(|(_, min)| *min)
            .min()
            .unwrap_or(delay)
    }
}

/// Counters for logs and the stream summary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    pub reports: u64,
    /// Packets the reports stood for (sampled packets times the stride).
    pub packets: u64,
    pub lost: u64,
    pub late: u64,
    /// Reported packets the host had no send record of.
    pub unmatched: u64,
    pub overuse_cuts: u64,
    pub loss_cuts: u64,
    /// Feedback payload bytes received.
    pub feedback_bytes: u64,
}

/// The controller of one stream.
#[derive(Clone, Debug)]
pub struct Congestion {
    min_bps: u64,
    max_bps: u64,
    /// The delay-based (AIMD) rate.
    delay_bps: f64,
    /// The loss-based limit.
    loss_bps: f64,
    state: RateState,
    inter: InterArrival,
    trend: Trendline,
    /// Received (client µs of the report's last arrival, wire bytes) per report.
    acked: VecDeque<(u64, u64)>,
    /// (host µs, packets, lost) per report.
    losses: VecDeque<(u64, u64, u64)>,
    /// The client clock's µs at the last report, for the acked window.
    last_arrival: Option<u64>,
    last_feedback: Option<u64>,
    last_tick: Option<u64>,
    last_decrease: Option<u64>,
    last_loss_cut: Option<u64>,
    /// Smoothed received rate at overuse: the link's last capacity.
    capacity_bps: Option<f64>,
    /// Bottleneck rate samples: (host µs, bps), and their smoothed value.
    bottleneck: VecDeque<(u64, f64)>,
    bottleneck_bps: Option<f64>,
    base_delay: BaseDelay,
    window_delay: DelayHistogram,
    stream_delay: DelayHistogram,
    /// The rate before the cut that started an open recovery, and when.
    recovering: Option<(u64, f64)>,
    recoveries: VecDeque<u64>,
    fec_percent: u32,
    counters: Counters,
    change: Option<Change>,
    limited: bool,
}

impl Congestion {
    /// A controller between `min_bps` and `max_bps` of video wire rate, starting at the top.
    pub fn new(min_bps: u64, max_bps: u64) -> Self {
        let max_bps = max_bps.max(1);
        let min_bps = min_bps.min(max_bps);
        Self {
            min_bps,
            max_bps,
            delay_bps: max_bps as f64,
            loss_bps: max_bps as f64,
            state: RateState::Hold,
            inter: InterArrival::default(),
            trend: Trendline::default(),
            acked: VecDeque::new(),
            losses: VecDeque::new(),
            last_arrival: None,
            last_feedback: None,
            last_tick: None,
            last_decrease: None,
            last_loss_cut: None,
            capacity_bps: None,
            bottleneck: VecDeque::new(),
            bottleneck_bps: None,
            base_delay: BaseDelay::default(),
            window_delay: DelayHistogram::default(),
            stream_delay: DelayHistogram::default(),
            recovering: None,
            recoveries: VecDeque::new(),
            fec_percent: 0,
            counters: Counters::default(),
            change: None,
            limited: false,
        }
    }

    /// New bounds, as when the stream's bitrate or mode changed. The rate stays where it is,
    /// within them.
    pub fn set_bounds(&mut self, min_bps: u64, max_bps: u64) {
        let was_max = self.max_bps as f64;
        self.max_bps = max_bps.max(1);
        self.min_bps = min_bps.min(self.max_bps);
        let clamp = |v: f64| {
            if v >= was_max {
                self.max_bps as f64
            } else {
                v.clamp(self.min_bps as f64, self.max_bps as f64)
            }
        };
        self.delay_bps = clamp(self.delay_bps);
        self.loss_bps = clamp(self.loss_bps);
    }

    /// One report, at host time `now_us`. `bytes` is its payload size.
    pub fn on_feedback(&mut self, now_us: u64, feedback: Feedback, bytes: usize) {
        let stride = u64::from(feedback.stride.max(1));
        self.counters.reports += 1;
        self.counters.feedback_bytes += bytes as u64;
        self.counters.late += u64::from(feedback.late);
        let skipped = u64::from(feedback.skipped);
        let mut packets = skipped;
        let mut lost = skipped;
        let mut received_bytes = 0;
        let mut last_arrival = None;
        if self
            .last_feedback
            .is_some_and(|at| now_us.saturating_sub(at) >= STALE_US)
        {
            self.inter.reset();
        }
        self.last_feedback = Some(now_us);
        for p in feedback.packets {
            packets += stride;
            let Some(arrival) = p.arrival_us else {
                lost += stride;
                continue;
            };
            received_bytes += u64::from(p.wire_bytes) * stride;
            last_arrival = Some(last_arrival.map_or(arrival, |a: u64| a.max(arrival)));
            // Queueing delay: one-way delay over the window's lowest.
            let owd = arrival as i64 - p.sent_us as i64;
            let base = self.base_delay.add(now_us, owd);
            let queued = (owd - base).max(0) as u64;
            self.window_delay.add(queued);
            self.stream_delay.add(queued);
            if let Some(deltas) = self.inter.add(p, arrival) {
                self.bottleneck_sample(now_us, &deltas.group, stride);
                let usage = self.trend.update(&deltas);
                self.on_usage(usage);
            }
        }
        self.counters.packets += packets;
        self.counters.lost += lost;
        self.losses.push_back((now_us, packets, lost));
        while self
            .losses
            .front()
            .is_some_and(|(at, ..)| at + LOSS_WINDOW_US < now_us)
        {
            self.losses.pop_front();
        }
        if let Some(arrival) = last_arrival {
            self.last_arrival = Some(arrival);
            self.acked.push_back((arrival, received_bytes));
            while self
                .acked
                .front()
                .is_some_and(|(at, _)| at + ACKED_WINDOW_US < arrival)
            {
                self.acked.pop_front();
            }
        }
    }

    /// A packet the host recorded none of: counted, not judged.
    pub fn on_unmatched(&mut self, packets: u64) {
        self.counters.unmatched += packets;
    }

    fn on_usage(&mut self, usage: Usage) {
        self.state = match (usage, self.state) {
            (Usage::Overusing, _) => RateState::Decrease,
            (Usage::Underusing, _) => RateState::Hold,
            (Usage::Normal, RateState::Hold) => RateState::Increase,
            (Usage::Normal, state) => state,
        };
    }

    /// A group sent faster than it arrived shows the bottleneck's rate.
    fn bottleneck_sample(&mut self, now_us: u64, g: &Group, stride: u64) {
        if g.packets < PACE_MIN_PACKETS {
            return;
        }
        let arrival_span = g.last_arrival.saturating_sub(g.first_arrival);
        let send_span = g.last_send.saturating_sub(g.first_send);
        if arrival_span < 300 {
            return;
        }
        // The first packet's bytes arrive at the span's start.
        let bits = (g.bytes * stride) as f64 * 8. * f64::from(g.packets - 1) / f64::from(g.packets);
        let received = bits * 1e6 / arrival_span as f64;
        let sent = if send_span == 0 {
            f64::INFINITY
        } else {
            bits * 1e6 / send_span as f64
        };
        let sample = if received < BOTTLENECK_RATIO * sent {
            received
        } else if let Some(estimate) = self.bottleneck_bps {
            // The link kept up. Sent at about the estimate (paced at it): the estimate holds.
            // Sent well above it: the bottleneck is at least what arrived.
            if sent <= 1.1 * estimate {
                estimate
            } else {
                estimate.max(received)
            }
        } else {
            return;
        };
        let received = sample;
        self.bottleneck.push_back((now_us, received));
        while self
            .bottleneck
            .front()
            .is_some_and(|(at, _)| at + PACE_WINDOW_US < now_us)
        {
            self.bottleneck.pop_front();
        }
        self.bottleneck_bps = Some(match self.bottleneck_bps {
            Some(old) => 0.8 * old + 0.2 * received,
            None => received,
        });
    }

    /// What the client received over the last [`ACKED_WINDOW_US`], bps.
    pub fn acked_bps(&self) -> Option<u64> {
        let last = self.last_arrival?;
        let first = self.acked.front()?.0;
        let span = last.saturating_sub(first).max(ACKED_WINDOW_US / 5);
        // The first report's bytes arrived before the window's start.
        let bytes: u64 = self.acked.iter().skip(1).map(|(_, b)| b).sum();
        (self.acked.len() > 1).then(|| bytes * 8 * 1_000_000 / span)
    }

    /// The share of packets lost over [`LOSS_WINDOW_US`], once enough came.
    pub fn loss(&self) -> Option<f64> {
        let (packets, lost) = self
            .losses
            .iter()
            .fold((0, 0), |(p, l), (_, packets, lost)| (p + packets, l + lost));
        (packets >= LOSS_MIN_PACKETS).then(|| lost as f64 / packets as f64)
    }

    /// The video wire rate the controller holds now, bps.
    pub fn target_bps(&self) -> u64 {
        (self.delay_bps.min(self.loss_bps) as u64).clamp(self.min_bps, self.max_bps)
    }

    /// Run the rate control: `fec_percent` is the parity share in use (parity per 100 data
    /// packets).
    pub fn tick(&mut self, now_us: u64, rtt: Duration, fec_percent: u32) -> Decision {
        self.fec_percent = fec_percent;
        let dt = self
            .last_tick
            .replace(now_us)
            .map_or(0., |last| now_us.saturating_sub(last) as f64 / 1e6)
            .min(1.);
        let stale = self
            .last_feedback
            .is_none_or(|at| now_us.saturating_sub(at) >= STALE_US);
        if stale {
            if self.limited {
                self.note(ChangeKind::Stale, self.target_bps(), self.max_bps, 0, 0.);
                self.limited = false;
            }
            self.delay_bps = self.max_bps as f64;
            self.loss_bps = self.max_bps as f64;
            self.state = RateState::Hold;
            self.recovering = None;
            return Decision::default();
        }
        let before = self.target_bps();
        let acked = self.acked_bps();
        let rtt_us = rtt.as_micros().min(u128::from(u64::MAX)) as u64;
        self.delay_rate(now_us, dt, acked, rtt_us);
        self.loss_rate(now_us, dt, before, rtt_us);
        let target = self.target_bps();
        self.track_recovery(now_us, before, target);
        if target >= self.max_bps && self.limited {
            self.limited = false;
            self.note(ChangeKind::Restored, before, target, acked.unwrap_or(0), 0.);
        } else if target < self.max_bps {
            self.limited = true;
        }
        Decision {
            video_bps: (target < self.max_bps).then_some(target),
            pace_bps: self.pace_bps(now_us),
        }
    }

    fn delay_rate(&mut self, now_us: u64, dt: f64, acked: Option<u64>, rtt_us: u64) {
        match self.state {
            RateState::Hold => {}
            RateState::Increase => {
                let limit = acked.map(|a| APP_LIMITED * a as f64 + 10_000.);
                if limit.is_some_and(|limit| self.delay_bps >= limit) {
                    return;
                }
                let near = self
                    .capacity_bps
                    .is_some_and(|cap| self.delay_bps >= NEAR_CAPACITY * cap);
                let grown = if near {
                    self.delay_bps + (ADDITIVE_INCREASE * self.delay_bps).max(MIN_ADDITIVE_BPS) * dt
                } else {
                    self.delay_bps * (1. + MULTIPLICATIVE_INCREASE).powf(dt)
                };
                let grown = limit.map_or(grown, |limit| grown.min(limit.max(self.delay_bps)));
                self.delay_bps = grown.min(self.max_bps as f64);
                // The link carries more than it did: forget the old capacity.
                if let (Some(cap), Some(a)) = (self.capacity_bps, acked)
                    && a as f64 > FORGET_CAPACITY * cap
                {
                    self.capacity_bps = None;
                }
            }
            RateState::Decrease => {
                let due = self
                    .last_decrease
                    .is_none_or(|at| now_us.saturating_sub(at) >= rtt_us);
                if let (true, Some(acked)) = (due, acked) {
                    let from = self.target_bps();
                    let aimed = DECREASE_FACTOR * acked as f64;
                    if aimed < self.delay_bps {
                        self.delay_bps = aimed.max(self.delay_bps * (1. - MAX_CUT));
                        self.delay_bps = self.delay_bps.max(self.min_bps as f64);
                        self.counters.overuse_cuts += 1;
                        self.last_decrease = Some(now_us);
                        self.note(ChangeKind::Overuse, from, self.target_bps(), acked, 0.);
                    }
                    self.capacity_bps = Some(match self.capacity_bps {
                        Some(cap) => 0.95 * cap + 0.05 * acked as f64,
                        None => acked as f64,
                    });
                }
                self.state = RateState::Hold;
            }
        }
    }

    /// The share of packets lost that FEC no longer carries.
    fn loss_threshold(&self) -> f64 {
        let parity = f64::from(self.fec_percent) / (100. + f64::from(self.fec_percent));
        LOSS_HIGH.max(FEC_ABSORBS * parity)
    }

    fn loss_rate(&mut self, now_us: u64, dt: f64, before: u64, rtt_us: u64) {
        let Some(loss) = self.loss() else {
            return;
        };
        if loss > self.loss_threshold() {
            let due = self
                .last_loss_cut
                .is_none_or(|at| now_us.saturating_sub(at) >= LOSS_CUT_GAP_US + rtt_us);
            if due {
                let cut = (before as f64 * (1. - 0.5 * loss)).max(self.min_bps as f64);
                self.loss_bps = cut;
                // The delay-based rate starts its climb from here too.
                self.delay_bps = self.delay_bps.min(cut);
                self.last_loss_cut = Some(now_us);
                self.counters.loss_cuts += 1;
                self.note(ChangeKind::Loss, before, self.target_bps(), 0, loss);
            }
        } else if loss < LOSS_LOW {
            self.loss_bps =
                (self.loss_bps * (1. + MULTIPLICATIVE_INCREASE).powf(dt)).min(self.max_bps as f64);
        }
    }

    fn track_recovery(&mut self, now_us: u64, before: u64, target: u64) {
        if target < before && self.recovering.is_none() {
            self.recovering = Some((now_us, before as f64));
        }
        if let Some((since, from)) = self.recovering {
            let elapsed = now_us.saturating_sub(since);
            if target as f64 >= RECOVERED_FRACTION * from || elapsed >= RECOVERY_CAP_US {
                self.recovering = None;
                if self.recoveries.len() == RECOVERIES {
                    self.recoveries.pop_front();
                }
                self.recoveries.push_back(elapsed.min(RECOVERY_CAP_US));
            }
        }
    }

    fn pace_bps(&self, now_us: u64) -> Option<u64> {
        let recent = self
            .bottleneck
            .iter()
            .filter(|(at, _)| at + PACE_WINDOW_US >= now_us)
            .count();
        if recent < PACE_SAMPLES {
            return None;
        }
        let pace =
            (self.bottleneck_bps? * PACE_FRACTION).max(self.target_bps() as f64 * PACE_HEADROOM);
        Some(pace as u64)
    }

    fn note(&mut self, kind: ChangeKind, from_bps: u64, to_bps: u64, acked_bps: u64, loss: f64) {
        self.change = Some(Change {
            kind,
            from_bps,
            to_bps,
            acked_bps,
            loss,
        });
    }

    /// The last change since the previous call, for the log.
    pub fn take_change(&mut self) -> Option<Change> {
        self.change.take()
    }

    pub fn counters(&self) -> Counters {
        self.counters
    }

    pub fn usage(&self) -> Usage {
        self.trend.usage
    }

    pub fn rate_state(&self) -> RateState {
        self.state
    }

    /// The scaled trend and the threshold it meets.
    pub fn trend(&self) -> (f64, f64) {
        (self.trend.modified, self.trend.threshold)
    }

    /// Queueing delay p50 and p95 since the last call, ms, and the samples; clears the window.
    pub fn take_window_delay(&mut self) -> (Option<f64>, Option<f64>, u64) {
        let out = (
            self.window_delay.percentile_ms(50.),
            self.window_delay.percentile_ms(95.),
            self.window_delay.count(),
        );
        self.window_delay.clear();
        out
    }

    /// Queueing delay over the whole stream.
    pub fn stream_delay(&self) -> &DelayHistogram {
        &self.stream_delay
    }

    /// Recovery times of the cuts so far, µs, oldest first.
    pub fn recoveries(&self) -> impl Iterator<Item = u64> + '_ {
        self.recoveries.iter().copied()
    }

    /// The median and longest recovery, ms.
    pub fn recovery_ms(&self) -> Option<(u64, u64)> {
        let mut all: Vec<u64> = self.recoveries.iter().copied().collect();
        all.sort_unstable();
        let median = *all.get(all.len() / 2)?;
        Some((median / 1000, all.last().copied().unwrap_or(0) / 1000))
    }

    pub fn max_bps(&self) -> u64 {
        self.max_bps
    }

    pub fn bottleneck_bps(&self) -> Option<u64> {
        self.bottleneck_bps.map(|b| b as u64)
    }

    /// Whether feedback came within [`STALE_US`].
    pub fn fresh(&self, now_us: u64) -> bool {
        self.last_feedback
            .is_some_and(|at| now_us.saturating_sub(at) < STALE_US)
    }

    /// State for the session API, the `stream timings` line and the end-of-stream summary.
    pub fn snapshot(&self, now_us: u64) -> serde_json::Value {
        let ms = |v: Option<f64>| v.map(|v| (v * 100.).round() / 100.);
        let recovery = self.recovery_ms();
        serde_json::json!({
            "fresh": self.fresh(now_us),
            "state": self.state.name(),
            "usage": self.trend.usage.name(),
            "target_kbps": self.target_bps() / 1000,
            "max_kbps": self.max_bps / 1000,
            "acked_kbps": self.acked_bps().map(|b| b / 1000),
            "loss_permille": self.loss().map(|l| (l * 1000.).round() as u64),
            "trend": (self.trend.modified * 100.).round() / 100.,
            "threshold": (self.trend.threshold * 100.).round() / 100.,
            "capacity_kbps": self.capacity_bps.map(|b| b as u64 / 1000),
            "bottleneck_kbps": self.bottleneck_bps().map(|b| b / 1000),
            "pace_kbps": self.pace_bps(now_us).map(|b| b / 1000),
            "reports": self.counters.reports,
            "packets": self.counters.packets,
            "lost": self.counters.lost,
            "late": self.counters.late,
            "unmatched": self.counters.unmatched,
            "overuse_cuts": self.counters.overuse_cuts,
            "loss_cuts": self.counters.loss_cuts,
            "feedback_bytes": self.counters.feedback_bytes,
            "queue_p50_ms": ms(self.stream_delay.percentile_ms(50.)),
            "queue_p95_ms": ms(self.stream_delay.percentile_ms(95.)),
            "recovery_p50_ms": recovery.map(|r| r.0),
            "recovery_max_ms": recovery.map(|r| r.1),
        })
    }
}

/// The host's record of each video packet it sent, by packet id: one store per packet, no
/// lock, written by the sender and read by the control thread. Off until [`Self::enable`],
/// and then 64 Ki slots (512 KiB) per stream.
pub struct SendLedger {
    started: Instant,
    live: AtomicBool,
    encrypted: AtomicBool,
    /// Per id: `(sent µs since started + 1, wrapping) << 32 | wire bytes`; 0 for none.
    slots: OnceLock<Box<[AtomicU64]>>,
}

impl SendLedger {
    pub fn new(started: Instant) -> Self {
        Self {
            started,
            live: AtomicBool::new(false),
            encrypted: AtomicBool::new(false),
            slots: OnceLock::new(),
        }
    }

    /// Starts recording; `encrypted` says where the packet id is (see
    /// [`rubylight_protocol::transport_feedback::packet_id`]).
    pub fn enable(&self, encrypted: bool) {
        self.slots
            .get_or_init(|| (0..=u16::MAX).map(|_| AtomicU64::new(0)).collect());
        self.encrypted.store(encrypted, Ordering::Relaxed);
        self.live.store(true, Ordering::Release);
    }

    pub fn disable(&self) {
        self.live.store(false, Ordering::Release);
    }

    pub fn live(&self) -> bool {
        self.live.load(Ordering::Relaxed)
    }

    /// µs since the ledger started.
    pub fn micros(&self, at: Instant) -> u64 {
        at.saturating_duration_since(self.started).as_micros() as u64
    }

    /// Packets the sender handed to the socket at `at`, each with `overhead` bytes of IP and
    /// UDP framing on top. Call from one thread at a time.
    pub fn record<T: AsRef<[u8]>>(&self, packets: &[T], overhead: usize, at: Instant) {
        if !self.live.load(Ordering::Acquire) {
            return;
        }
        let Some(slots) = self.slots.get() else {
            return;
        };
        let encrypted = self.encrypted.load(Ordering::Relaxed);
        let stamp = (self.micros(at) as u32).wrapping_add(1).max(1);
        for packet in packets {
            let packet = packet.as_ref();
            let Some(id) = rubylight_protocol::transport_feedback::packet_id(packet, encrypted)
            else {
                continue;
            };
            let bytes = (packet.len() + overhead).min(u32::MAX as usize) as u64;
            slots[usize::from(id)].store(u64::from(stamp) << 32 | bytes, Ordering::Relaxed);
        }
    }

    /// The send time (µs since started) and wire bytes of packet `id`, read at `now_us`; `None`
    /// when none was recorded or the record is older than [`STALE_US`] (an id from a wrap ago).
    pub fn lookup(&self, id: u16, now_us: u64) -> Option<(u64, u32)> {
        let word = self.slots.get()?[usize::from(id)].load(Ordering::Relaxed);
        if word == 0 {
            return None;
        }
        let stamp = (word >> 32) as u32;
        let age = (now_us as u32).wrapping_add(1).wrapping_sub(stamp);
        if u64::from(age) > STALE_US {
            return None;
        }
        Some((now_us.saturating_sub(u64::from(age)), word as u32))
    }
}

/// The client's arrival clock, unwrapped from the reports' 32-bit base.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClientClock {
    last: Option<(u32, u64)>,
}

impl ClientClock {
    /// The report's base arrival as a 64-bit µs count, continuing the previous ones.
    pub fn unwrap(&mut self, base: u32) -> u64 {
        let value = match self.last {
            None => u64::from(base),
            Some((raw, value)) => {
                let delta = base.wrapping_sub(raw) as i32;
                value.wrapping_add_signed(i64::from(delta))
            }
        };
        self.last = Some((base, value));
        value
    }
}

/// The expected next id after a report, for counting a skip as loss.
#[derive(Clone, Copy, Debug, Default)]
pub struct Continuity {
    next: Option<u16>,
}

impl Continuity {
    /// Packets skipped before `report` that count as lost: none for a continuous report or a
    /// long jump (a stall or renumbering).
    pub fn skipped(&mut self, report: &TransportFeedback) -> u32 {
        let skipped = match self.next {
            Some(next)
                if report.flags & rubylight_protocol::transport_feedback::FLAG_SKIPPED != 0 =>
            {
                let gap = u32::from(report.base.wrapping_sub(next));
                let stride = u32::from(report.stride.max(1));
                let limit = (MAX_GAP_REPORTS * MAX_ENTRIES) as u32 * stride;
                if gap < limit { gap } else { 0 }
            }
            _ => 0,
        };
        self.next = Some(report.next_id());
        skipped
    }
}

/// Encoder kbps the controller never goes below on H.264, HEVC and AV1; PyroWave stops at its
/// severe-detail-loss rate, as the link-rate controller does.
pub const FLOOR_KBPS: u32 = 2_000;

/// The controller with the session's bookkeeping around it, behind one mutex: the joins of a
/// report with the [`SendLedger`], the conversion between wire and encoder rates, and the
/// combination with the link-rate controller's limit.
///
/// While it is on, the session writes the encoder bitrate only through [`Control::combine`]:
/// the lower of the two controllers' limits, and the bitrate from before either when both let
/// go. In shadow mode (`BUTTERPOLLO_CONGESTION_SHADOW=1`, for the A/B) it measures and logs
/// everything, but its own limit and pace are never applied.
#[derive(Default)]
pub struct Control {
    cc: Option<Congestion>,
    shadow: bool,
    clock: ClientClock,
    continuity: Continuity,
    buffer: Vec<PacketResult>,
    budget: Option<crate::adaptive_bitrate::WireBudget>,
    ceiling_kbps: u32,
    floor_kbps: u32,
    /// What the link-rate controller wants, encoder kbps; `None` for no limit.
    pub link_wanted: Option<u32>,
    /// What this controller wants, encoder kbps.
    wanted: Option<u32>,
    /// The bitrate from before either limit.
    restore: Option<u32>,
    /// The client set the bitrate: off for the rest of the stream, also when it is rebuilt.
    taken: bool,
}

impl Control {
    /// A controller for `stream`; `maximum_kbps` is the user's `max_bitrate` (0: none). Off when
    /// `enabled` is false.
    pub fn new(
        stream: &crate::rtsp::Negotiated,
        enabled: bool,
        maximum_kbps: u32,
        shadow: bool,
    ) -> Self {
        if !enabled {
            return Self::default();
        }
        let ceiling = stream
            .bitrate_kbps
            .min(if maximum_kbps > 0 {
                maximum_kbps
            } else {
                u32::MAX
            })
            .max(1);
        let floor = if stream.codec == 3 {
            crate::link_rate::pyrowave_floor_kbps(stream)
        } else {
            FLOOR_KBPS
        }
        .min(ceiling);
        let budget = crate::adaptive_bitrate::WireBudget::of(stream);
        let period = crate::reconfigure::Mode::of(stream).period();
        let (min, max) = Self::bounds(&budget, floor, ceiling, 20, period);
        Self {
            cc: Some(Congestion::new(min, max)),
            shadow,
            budget: Some(budget),
            ceiling_kbps: ceiling,
            floor_kbps: floor,
            ..Self::default()
        }
    }

    /// Video wire bps of the floor and the ceiling, with the headroom
    /// [`WireBudget::encoder_kbps`](crate::adaptive_bitrate::WireBudget::encoder_kbps) takes
    /// back, so the ceiling converts back to the requested bitrate.
    fn bounds(
        budget: &crate::adaptive_bitrate::WireBudget,
        floor: u32,
        ceiling: u32,
        fec: usize,
        period: Duration,
    ) -> (u64, u64) {
        let video = |kbps| {
            budget
                .total_bps(kbps, fec, period)
                .saturating_sub(budget.audio_bps)
        };
        (video(floor), video(ceiling))
    }

    /// Whether the controller runs for this stream (also in shadow mode).
    pub fn active(&self) -> bool {
        self.cc.is_some()
    }

    pub fn shadow(&self) -> bool {
        self.shadow
    }

    /// A controller for the stream after a change of mode or bitrate; a client takeover stays.
    pub fn rebuild(
        &self,
        stream: &crate::rtsp::Negotiated,
        enabled: bool,
        maximum_kbps: u32,
        shadow: bool,
    ) -> Self {
        if self.taken {
            return Self {
                taken: true,
                ..Self::default()
            };
        }
        Self::new(stream, enabled, maximum_kbps, shadow)
    }

    /// The client set the bitrate itself: it owns it from now on.
    pub fn takeover(&mut self) {
        self.taken = true;
        self.cc = None;
        self.wanted = None;
        self.link_wanted = None;
        self.restore = None;
    }

    /// The bitrate from before either limit, taken for a rebuilt stream.
    pub fn take_restore(&mut self) -> Option<u32> {
        self.restore.take()
    }

    /// One 0x5547 payload at host time `now_us`, joined with `ledger`. Returns whether it was
    /// a valid report.
    pub fn on_report(&mut self, ledger: &SendLedger, now_us: u64, payload: &[u8]) -> bool {
        let Some(cc) = self.cc.as_mut() else {
            return false;
        };
        let Some(report) = TransportFeedback::decode(payload) else {
            return false;
        };
        let base = self.clock.unwrap(report.base_arrival_us);
        let skipped = self.continuity.skipped(&report);
        self.buffer.clear();
        let mut unmatched = 0;
        for (id, offset) in report.packets() {
            match ledger.lookup(id, now_us) {
                Some((sent_us, wire_bytes)) => self.buffer.push(PacketResult {
                    sent_us,
                    arrival_us: offset.map(|offset| base + offset),
                    wire_bytes,
                }),
                None => unmatched += 1,
            }
        }
        cc.on_unmatched(unmatched * u64::from(report.stride));
        cc.on_feedback(
            now_us,
            Feedback {
                packets: &self.buffer,
                stride: report.stride,
                late: report.late,
                skipped,
            },
            payload.len(),
        );
        true
    }

    /// Runs the rate control. Returns the pace to apply, bps (`None`: the host's own).
    pub fn tick(
        &mut self,
        now_us: u64,
        rtt: Duration,
        fec: usize,
        period: Duration,
    ) -> Option<u64> {
        let (Some(cc), Some(budget)) = (self.cc.as_mut(), self.budget.as_ref()) else {
            return None;
        };
        let (min, max) = Self::bounds(budget, self.floor_kbps, self.ceiling_kbps, fec, period);
        cc.set_bounds(min, max);
        let decision = cc.tick(now_us, rtt, fec.min(u32::MAX as usize) as u32);
        let (ceiling, floor) = (self.ceiling_kbps, self.floor_kbps);
        self.wanted = decision.video_bps.map(|bps| {
            budget
                .encoder_kbps(bps + budget.audio_bps, ceiling, fec, period)
                .clamp(floor, ceiling)
        });
        if self.shadow { None } else { decision.pace_bps }
    }

    /// What this controller wants, encoder kbps, applied or (in shadow mode) not.
    pub fn wanted(&self) -> Option<u32> {
        self.wanted
    }

    /// The encoder bitrate to use now that the bitrate in effect is `current`: the lowest of
    /// the bitrate from before any limit and both controllers' limits.
    pub fn combine(&mut self, current: u32) -> u32 {
        let own = if self.shadow { None } else { self.wanted };
        let limit = match (self.link_wanted, own) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        match limit {
            Some(kbps) => kbps.min(*self.restore.get_or_insert(current)),
            None => self.restore.take().unwrap_or(current),
        }
    }

    /// The last change worth a log line.
    pub fn take_change(&mut self) -> Option<Change> {
        self.cc.as_mut()?.take_change()
    }

    /// Queueing delay p50/p95 since the last call, for the `stream timings` line.
    pub fn take_window_delay(&mut self) -> (Option<f64>, Option<f64>, u64) {
        self.cc
            .as_mut()
            .map_or((None, None, 0), Congestion::take_window_delay)
    }

    pub fn snapshot(&self, now_us: u64) -> serde_json::Value {
        match &self.cc {
            Some(cc) => {
                let mut value = cc.snapshot(now_us);
                value["shadow"] = self.shadow.into();
                value["wanted_kbps"] = self.wanted.into();
                value["link_wanted_kbps"] = self.link_wanted.into();
                value
            }
            None => serde_json::json!({ "state": "off" }),
        }
    }
}

#[cfg(test)]
mod tests;
