//! Phase lock: the host times its frames so they reach the client just before the client's
//! display latch, at the client's exact refresh period.
//!
//! Without it, a 120 Hz host and a 120 Hz phone run on separate clocks: frames reach the
//! phone at a phase that drifts slowly (one full refresh every ~50 s for 120.02 against
//! 120.00 Hz), so each frame waits anywhere from nothing to a whole refresh for the next
//! latch, and the wait changes from minute to minute.
//!
//! The client measures, for every frame it is ready to show, its *slack*: the time from
//! "ready" to the next latch deadline. Every half second it sends a [`Report`] built by
//! [`SlackWindow`]. The host feeds reports to [`PhaseLock`], which nudges the interval
//! between captured frames so the early edge of the slack distribution settles at a small
//! safety margin, and follows the client's refresh period.

/// Control-stream message type of a [`Report`] (client to host).
pub const REPORT_MESSAGE_TYPE: u16 = 0x5530;
/// Encoded size of a [`Report`].
pub const REPORT_BYTES: usize = 16;
const REPORT_VERSION: u8 = 1;

/// Slack statistics of the frames a client showed during one window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Frames in the window.
    pub frames: u16,
    /// The client's display refresh period, in nanoseconds of the client's clock.
    pub period_ns: u32,
    /// Slack of the earliest-to-latch tenth of frames (10th percentile), in nanoseconds.
    /// Negative when those frames reach the client after the latch they were closest to.
    pub lead_ns: i32,
    /// Spread of the slack between its 10th and 90th percentile, in nanoseconds.
    pub spread_ns: u32,
}

impl Report {
    /// Little-endian wire form: version, reserved, frames, period, lead, spread.
    pub fn encode(&self) -> [u8; REPORT_BYTES] {
        let mut out = [0; REPORT_BYTES];
        out[0] = REPORT_VERSION;
        out[2..4].copy_from_slice(&self.frames.to_le_bytes());
        out[4..8].copy_from_slice(&self.period_ns.to_le_bytes());
        out[8..12].copy_from_slice(&self.lead_ns.to_le_bytes());
        out[12..16].copy_from_slice(&self.spread_ns.to_le_bytes());
        out
    }

    /// Parses a report; longer payloads from newer clients are accepted, unknown versions
    /// and implausible values are not.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < REPORT_BYTES || data[0] != REPORT_VERSION {
            return None;
        }
        let u32_at = |at: usize| u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
        let report = Self {
            frames: u16::from_le_bytes([data[2], data[3]]),
            period_ns: u32_at(4),
            lead_ns: u32_at(8) as i32,
            spread_ns: u32_at(12),
        };
        // 24 to 500 Hz, and slack within one refresh.
        let plausible = (2_000_000..=41_700_000).contains(&report.period_ns)
            && report.frames > 0
            && report.lead_ns.unsigned_abs() <= report.period_ns
            && report.spread_ns <= report.period_ns;
        plausible.then_some(report)
    }
}

/// Client side: collects the slack of each frame and summarises a window into a [`Report`].
#[derive(Clone, Debug)]
pub struct SlackWindow {
    samples: [i64; Self::CAPACITY],
    count: usize,
    period_ns: i64,
}

impl Default for SlackWindow {
    fn default() -> Self {
        Self { samples: [0; Self::CAPACITY], count: 0, period_ns: 0 }
    }
}

impl SlackWindow {
    /// Enough for half a second at 500 fps; later frames in a full window are dropped.
    pub const CAPACITY: usize = 256;

    /// Records one frame: `slack_ns` is the time from the frame being ready to the next latch
    /// deadline, `period_ns` the display refresh period at that moment.
    pub fn push(&mut self, slack_ns: i64, period_ns: i64) {
        if period_ns <= 0 {
            return;
        }
        if period_ns != self.period_ns {
            // A refresh-rate change makes earlier samples meaningless.
            self.count = 0;
            self.period_ns = period_ns;
        }
        if self.count < Self::CAPACITY {
            self.samples[self.count] = slack_ns.rem_euclid(period_ns);
            self.count += 1;
        }
    }

    /// Frames recorded since the last report.
    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Summarises and clears the window. Needs at least `min_frames` frames.
    ///
    /// Slack is circular: a frame that just missed a latch has almost a full refresh of
    /// slack until the next one, yet sits right next to frames that just made it. So the
    /// samples are centred on their circular mean before taking percentiles, which keeps
    /// such a cluster together instead of splitting it across both ends of the refresh.
    pub fn report(&mut self, min_frames: usize) -> Option<Report> {
        let count = self.count;
        self.count = 0;
        if count < min_frames.max(1) {
            return None;
        }
        let period = self.period_ns;
        let samples = &mut self.samples[..count];
        let centre = circular_mean(samples, period);
        for sample in samples.iter_mut() {
            // Deviation from the centre, in (-period/2, period/2].
            let mut deviation = (*sample - centre).rem_euclid(period);
            if deviation > period / 2 {
                deviation -= period;
            }
            *sample = deviation;
        }
        samples.sort_unstable();
        let percentile = |p: usize| samples[(count - 1) * p / 100];
        let low = percentile(10);
        let high = percentile(90);
        // The centre itself is ambiguous by whole periods; express the low tail as slack in
        // (-period/2, period/2] so a cluster straddling the latch reads as slightly late.
        let mut lead = (centre + low).rem_euclid(period);
        if lead > period / 2 {
            lead -= period;
        }
        Some(Report {
            frames: count.min(u16::MAX as usize) as u16,
            period_ns: period.clamp(0, u32::MAX as i64) as u32,
            lead_ns: lead as i32,
            spread_ns: (high - low).clamp(0, u32::MAX as i64) as u32,
        })
    }
}

/// Circular mean of values in [0, period), without floating-point trigonometry: the mean of
/// the samples after rotating them so the largest gap between neighbours sits at the wrap.
fn circular_mean(samples: &[i64], period: i64) -> i64 {
    let mut sorted = [0i64; SlackWindow::CAPACITY];
    let sorted = &mut sorted[..samples.len()];
    sorted.copy_from_slice(samples);
    sorted.sort_unstable();
    // The largest gap between neighbours (including the wrap from last to first) marks
    // where the cluster ends; start counting just after it.
    let mut start = 0;
    let mut widest = sorted[0] + period - sorted[sorted.len() - 1];
    for i in 1..sorted.len() {
        let gap = sorted[i] - sorted[i - 1];
        if gap > widest {
            widest = gap;
            start = i;
        }
    }
    let origin = sorted[start];
    let total: i64 = sorted.iter().map(|&value| (value - origin).rem_euclid(period)).sum();
    (origin + total / sorted.len() as i64).rem_euclid(period)
}

/// Host side: turns reports into the interval between the frames the host captures.
#[derive(Clone, Debug)]
pub struct PhaseLock {
    /// Slack the early edge of the distribution should keep before the latch.
    margin_ns: i64,
    /// Client refresh period from the latest report; 0 until the first report.
    period_ns: i64,
    /// Slow per-frame correction that absorbs the clock-rate difference.
    trim_ns: i64,
    /// Phase shift still to apply, spread over the next frames.
    pending_ns: i64,
    locked: bool,
}

impl Default for PhaseLock {
    fn default() -> Self {
        Self::new(Self::DEFAULT_MARGIN_NS)
    }
}

impl PhaseLock {
    /// Covers decode-time and network jitter that the 10th percentile does not.
    pub const DEFAULT_MARGIN_NS: i64 = 1_500_000;
    /// Largest change to one frame interval, so motion never visibly stutters.
    pub const MAX_STEP_NS: i64 = 250_000;
    /// Share of the measured phase error corrected per report.
    const GAIN_NUM: i64 = 1;
    const GAIN_DEN: i64 = 2;
    /// Share of the error per frame added to the clock-rate trim per report.
    const TRIM_DEN: i64 = 16;

    pub fn new(margin_ns: i64) -> Self {
        Self { margin_ns, period_ns: 0, trim_ns: 0, pending_ns: 0, locked: false }
    }

    /// True once a plausible report has arrived.
    pub fn locked(&self) -> bool {
        self.locked
    }

    /// The client's refresh period, once known.
    pub fn period_ns(&self) -> Option<i64> {
        self.locked.then_some(self.period_ns)
    }

    /// Applies one client report.
    pub fn on_report(&mut self, report: &Report) {
        let period = i64::from(report.period_ns);
        if !self.locked || period != self.period_ns {
            // A new client refresh rate: start over.
            self.trim_ns = 0;
            self.pending_ns = 0;
        }
        self.period_ns = period;
        self.locked = true;
        // A wide spread needs more room than the fixed margin.
        let target = self.margin_ns.max(i64::from(report.spread_ns) / 4);
        let mut error = i64::from(report.lead_ns) - target;
        // Never chase more than half a refresh in either direction.
        error = error.clamp(-period / 2, period / 2);
        // Positive error: frames arrive too early, so capture them later. Earlier corrections
        // still in flight are replaced, not added, because the report already reflects them
        // only partially.
        self.pending_ns = error * Self::GAIN_NUM / Self::GAIN_DEN;
        let frames = i64::from(report.frames.max(1));
        self.trim_ns = (self.trim_ns + error / frames / Self::TRIM_DEN).clamp(-period / 100, period / 100);
    }

    /// Interval from this captured frame to the next, given the host's own nominal interval.
    /// Returns the nominal interval until the first report.
    pub fn next_interval_ns(&mut self, nominal_ns: i64) -> i64 {
        if !self.locked {
            return nominal_ns;
        }
        let step = self.pending_ns.clamp(-Self::MAX_STEP_NS, Self::MAX_STEP_NS);
        self.pending_ns -= step;
        self.period_ns + self.trim_ns + step
    }

    /// Forgets the client, for example after it stops sending reports.
    pub fn reset(&mut self) {
        *self = Self::new(self.margin_ns);
    }
}

#[cfg(test)]
mod tests;
