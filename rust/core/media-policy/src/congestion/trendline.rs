//! Inter-arrival packet grouping, trendline overuse detection, and queueing delay histograms.

use std::collections::VecDeque;

use super::PacketResult;

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

/// Queueing delay is measured against the lowest one-way delay of this window.
pub const BASE_DELAY_WINDOW_US: u64 = 10_000_000;
/// Queueing delay histogram: buckets of this width up to the last, which takes the rest.
const DELAY_BUCKET_US: u64 = 250;
const DELAY_BUCKETS: usize = 2_000;

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

#[derive(Clone, Copy, Debug)]
pub(super) struct Group {
    pub(super) first_send: u64,
    pub(super) last_send: u64,
    pub(super) first_arrival: u64,
    pub(super) last_arrival: u64,
    pub(super) bytes: u64,
    pub(super) packets: u32,
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
#[derive(Clone, Debug)]
pub(super) struct InterArrival {
    current: Option<Group>,
    previous: Option<Group>,
    /// Packets sent within this of a group's first belong to it: [`BURST_US`], or less above
    /// 120 fps ([`crate::pacing_high_hz::group_burst_us`]).
    burst_us: u64,
}

impl Default for InterArrival {
    fn default() -> Self {
        Self {
            current: None,
            previous: None,
            burst_us: BURST_US,
        }
    }
}

/// The deltas between two groups, ms, and the later group.
pub(super) struct Deltas {
    pub(super) send_ms: f64,
    pub(super) arrival_ms: f64,
    pub(super) group: Group,
}

impl InterArrival {
    pub(super) fn add(&mut self, p: &PacketResult, arrival: u64) -> Option<Deltas> {
        let Some(current) = self.current.as_mut() else {
            self.current = Some(Group::new(p, arrival));
            return None;
        };
        if p.sent_us < current.first_send {
            // Sent before the open group: a late join, which says nothing about the trend.
            return None;
        }
        if p.sent_us - current.first_send <= self.burst_us {
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

    /// Forgets the groups; the group window stays.
    pub(super) fn reset(&mut self) {
        self.current = None;
        self.previous = None;
    }

    /// Sets the group window, µs; the open groups are forgotten.
    pub(super) fn set_burst_us(&mut self, burst_us: u64) {
        self.reset();
        self.burst_us = burst_us.max(1);
    }

    pub(super) fn burst_us(&self) -> u64 {
        self.burst_us
    }
}

/// The trendline filter and the overuse detector.
#[derive(Clone, Debug)]
pub(super) struct Trendline {
    window: VecDeque<(f64, f64)>,
    accumulated: f64,
    smoothed: f64,
    first_arrival_ms: Option<f64>,
    deltas: u32,
    trend: f64,
    previous_trend: f64,
    pub(super) modified: f64,
    pub(super) threshold: f64,
    last_threshold_ms: Option<f64>,
    time_over: f64,
    overuse_count: u32,
    pub(super) usage: Usage,
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
    pub(super) fn update(&mut self, d: &Deltas) -> Usage {
        let delta = d.arrival_ms - d.send_ms;
        self.deltas = (self.deltas + 1).min(1000);
        self.accumulated += delta;
        self.smoothed = SMOOTHING * self.smoothed + (1. - SMOOTHING) * self.accumulated;
        let arrival_ms = d.group.last_arrival as f64 / 1000.;
        if self
            .first_arrival_ms
            .is_some_and(|first| arrival_ms < first)
        {
            self.first_arrival_ms = Some(arrival_ms);
            self.window.clear();
        }
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
    pub(super) fn add(&mut self, delay_us: u64) {
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

    pub(super) fn clear(&mut self) {
        self.buckets.fill(0);
        self.count = 0;
    }
}

/// The lowest one-way delay over [`BASE_DELAY_WINDOW_US`], in one-second segments.
#[derive(Clone, Debug, Default)]
pub(super) struct BaseDelay {
    segments: VecDeque<(u64, i64)>,
}

impl BaseDelay {
    pub(super) fn add(&mut self, now_us: u64, delay: i64) -> i64 {
        let segment = now_us / 1_000_000;
        match self.segments.back_mut() {
            Some((at, min)) if *at == segment => *min = (*min).min(delay),
            _ => self.segments.push_back((segment, delay)),
        }
        let keep = BASE_DELAY_WINDOW_US / 1_000_000;
        while self
            .segments
            .front()
            .is_some_and(|(at, _)| at.saturating_add(keep) < segment)
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
