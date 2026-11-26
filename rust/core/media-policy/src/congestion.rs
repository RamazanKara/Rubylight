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
//!   group (half a frame above 120 fps, [`crate::pacing_high_hz`]); each completed group gives
//!   a send delta and an arrival delta against the one before.
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
#[cfg(test)]
use std::time::{Duration, Instant};

mod control;
mod rate;
mod trendline;

pub use rubylight_protocol::transport_feedback::{
    MAX_ENTRIES, MAX_GAP_REPORTS, TRANSPORT_FEEDBACK_MESSAGE_TYPE, TransportFeedback,
};

pub use control::{ClientClock, Continuity, Control, FLOOR_KBPS, SendLedger};
#[cfg(test)]
use trendline::Deltas;
pub use trendline::{
    BASE_DELAY_WINDOW_US, BURST_US, DelayHistogram, INITIAL_THRESHOLD, OVERUSE_TIME_MS, SMOOTHING,
    THRESHOLD_GAIN, TRENDLINE_WINDOW, Usage,
};
use trendline::{BaseDelay, Group, InterArrival, Trendline};

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
/// Every packet of that window waited at least this long in a queue: the queue stands, and
/// the link is the limit even for a stream that sends far below its rate.
pub const STANDING_QUEUE_US: u64 = 10_000;
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
pub const STALE_US: u64 = 500_000;
/// A group arrives this much slower than it was sent: the bottleneck spread it.
pub const BOTTLENECK_RATIO: f64 = 0.8;
pub const PACE_SAMPLES: usize = 8;
pub const PACE_WINDOW_US: u64 = 2_000_000;
pub const PACE_FRACTION: f64 = 0.95;
pub const PACE_HEADROOM: f64 = 1.25;
/// A group needs this many packets to say anything about the bottleneck.
const PACE_MIN_PACKETS: u32 = 4;
/// A cut is over once the rate is back to this share of the rate before it.
pub const RECOVERED_FRACTION: f64 = 0.9;
/// A recovery not over after this counts as this long.
pub const RECOVERY_CAP_US: u64 = 30_000_000;
/// Recovery times kept for the stream summary.
const RECOVERIES: usize = 64;

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
    /// Overuse signals that cut nothing, because the stream sent far below its rate and no
    /// queue stood ([`Congestion::application_limited`]).
    pub overuse_held: u64,
    pub loss_cuts: u64,
    /// Feedback payload bytes received.
    pub feedback_bytes: u64,
}

/// One report's share of the acked window.
#[derive(Clone, Copy, Debug)]
struct Acked {
    /// When the host processed the report, host µs.
    host_us: u64,
    /// When its last packet arrived, client µs.
    arrival: u64,
    /// The wire bytes the client received.
    received: u64,
    /// When the host sent the last of its packets, host µs.
    last_send: u64,
    /// The wire bytes the host sent for it, lost packets included.
    sent: u64,
    /// The least queueing delay among its packets, µs.
    least_queue: u64,
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
    /// What each report of the last [`ACKED_WINDOW_US`] received and stood for.
    acked: VecDeque<Acked>,
    /// Wire bytes of reports in which nothing arrived, for the next entry of `acked`.
    sent_unacked: u64,
    /// (host µs, packets, lost) per report, with running sums across the window.
    losses: VecDeque<(u64, u64, u64)>,
    acked_received_sum: u128,
    acked_sent_sum: u128,
    losses_packets_sum: u64,
    losses_lost_sum: u64,
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
            sent_unacked: 0,
            losses: VecDeque::new(),
            acked_received_sum: 0,
            acked_sent_sum: 0,
            losses_packets_sum: 0,
            losses_lost_sum: 0,
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

    /// The stream's frame period: above 120 fps the inter-arrival groups shrink to half a frame
    /// ([`crate::pacing_high_hz::group_burst_us`]). A change forgets the open groups.
    pub fn set_frame_period(&mut self, period: std::time::Duration) {
        let burst = crate::pacing_high_hz::group_burst_us(period);
        if burst != self.inter.burst_us() {
            self.inter.set_burst_us(burst);
        }
    }

    /// The inter-arrival group window, µs.
    pub fn group_burst_us(&self) -> u64 {
        self.inter.burst_us()
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
        let mut sent_bytes = 0;
        let mut last_send = 0;
        let mut least_queue = u64::MAX;
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
            sent_bytes += u64::from(p.wire_bytes) * stride;
            last_send = last_send.max(p.sent_us);
            let Some(arrival) = p.arrival_us else {
                lost += stride;
                continue;
            };
            received_bytes += u64::from(p.wire_bytes) * stride;
            last_arrival = Some(last_arrival.map_or(arrival, |a: u64| a.max(arrival)));
            // Queueing delay: one-way delay over the window's lowest.
            let owd = (i128::from(arrival) - i128::from(p.sent_us))
                .clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64;
            let base = self.base_delay.add(now_us, owd);
            let queued = owd.saturating_sub(base).max(0) as u64;
            least_queue = least_queue.min(queued);
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
        self.losses_packets_sum += packets;
        self.losses_lost_sum += lost;
        while self
            .losses
            .front()
            .is_some_and(|(at, ..)| at.saturating_add(LOSS_WINDOW_US) < now_us)
        {
            if let Some((_, p, l)) = self.losses.pop_front() {
                self.losses_packets_sum -= p;
                self.losses_lost_sum -= l;
            }
        }
        // Skipped packets have no record here: they count at the report's mean size.
        let listed = feedback.packets.len() as u64 * stride;
        if let Some(mean) = sent_bytes.checked_div(listed) {
            sent_bytes = sent_bytes.saturating_add(skipped.saturating_mul(mean));
        }
        let Some(arrival) = last_arrival else {
            self.sent_unacked = self.sent_unacked.saturating_add(sent_bytes);
            return;
        };
        if self.last_arrival.is_some_and(|prev| arrival < prev) {
            self.acked.clear();
            self.acked_received_sum = 0;
            self.acked_sent_sum = 0;
        }
        self.last_arrival = Some(arrival);
        let entry = Acked {
            host_us: now_us,
            arrival,
            received: received_bytes,
            last_send,
            sent: sent_bytes.saturating_add(std::mem::take(&mut self.sent_unacked)),
            least_queue,
        };
        self.acked_received_sum = self
            .acked_received_sum
            .saturating_add(u128::from(entry.received));
        self.acked_sent_sum = self.acked_sent_sum.saturating_add(u128::from(entry.sent));
        self.acked.push_back(entry);
        while self.acked.front().is_some_and(|report| {
            report.arrival.saturating_add(ACKED_WINDOW_US) < arrival
                || report.host_us.saturating_add(ACKED_WINDOW_US * 2) < now_us
        }) {
            if let Some(evicted) = self.acked.pop_front() {
                self.acked_received_sum -= u128::from(evicted.received);
                self.acked_sent_sum -= u128::from(evicted.sent);
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
            .is_some_and(|(at, _)| at.saturating_add(PACE_WINDOW_US) < now_us)
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
        let first = self.acked.front()?;
        let span = last.saturating_sub(first.arrival).max(ACKED_WINDOW_US / 5);
        // The first report's bytes arrived before the window's start.
        let bytes = self
            .acked_received_sum
            .saturating_sub(u128::from(first.received));
        (self.acked.len() > 1).then(|| {
            (bytes.saturating_mul(8_000_000) / u128::from(span)).min(u128::from(u64::MAX)) as u64
        })
    }

    /// What the host sent for the same reports, bps: their packets' wire bytes, lost ones
    /// included, over the time it sent them in.
    pub fn sent_bps(&self) -> Option<u64> {
        let first = self.acked.front()?;
        let last = self.acked.back()?.last_send;
        let span = last
            .saturating_sub(first.last_send)
            .max(ACKED_WINDOW_US / 5);
        let bytes = self.acked_sent_sum.saturating_sub(u128::from(first.sent));
        (self.acked.len() > 1).then(|| {
            (bytes.saturating_mul(8_000_000) / u128::from(span)).min(u128::from(u64::MAX)) as u64
        })
    }

    /// The queueing delay no packet of the window got under, µs, once its reports span half
    /// of it. A stall of the link or a burst of jitter leaves earlier or later packets that
    /// waited for nothing, so it stays near zero; a queue the stream keeps filling raises it.
    fn standing_queue_us(&self) -> u64 {
        let (Some(first), Some(last)) = (self.acked.front(), self.acked.back()) else {
            return 0;
        };
        if last.arrival.saturating_sub(first.arrival) < ACKED_WINDOW_US / 2 {
            return 0;
        }
        self.acked
            .iter()
            .map(|report| report.least_queue)
            .min()
            .unwrap_or(0)
    }

    /// Whether an overuse says nothing about the link: the host sent far less than it may (a
    /// still picture, a menu, a light scene), so what the client received is the encoder's
    /// output and not what the link carries, and no queue stood through the window. A cut to
    /// that output would hold the stream at it when the picture gets busy again, and the rate
    /// cannot grow back while so little is sent. The stream's own rate is the measure, not
    /// the received one: a link that delivers little of a full stream is the limit.
    pub fn application_limited(&self) -> bool {
        self.sent_bps().is_some_and(|sent| {
            APP_LIMITED * sent as f64 + 10_000. < self.target_bps() as f64
                && self.standing_queue_us() < STANDING_QUEUE_US
        })
    }

    /// The share of packets lost over [`LOSS_WINDOW_US`], once enough came.
    pub fn loss(&self) -> Option<f64> {
        (self.losses_packets_sum >= LOSS_MIN_PACKETS)
            .then(|| self.losses_lost_sum as f64 / self.losses_packets_sum as f64)
    }

    /// The video wire rate the controller holds now, bps.
    pub fn target_bps(&self) -> u64 {
        (self.delay_bps.min(self.loss_bps) as u64).clamp(self.min_bps, self.max_bps)
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
            "sent_kbps": self.sent_bps().map(|b| b / 1000),
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
            "overuse_held": self.counters.overuse_held,
            "loss_cuts": self.counters.loss_cuts,
            "feedback_bytes": self.counters.feedback_bytes,
            "queue_p50_ms": ms(self.stream_delay.percentile_ms(50.)),
            "queue_p95_ms": ms(self.stream_delay.percentile_ms(95.)),
            "recovery_p50_ms": recovery.map(|r| r.0),
            "recovery_max_ms": recovery.map(|r| r.1),
        })
    }
}

#[cfg(test)]
mod tests;
