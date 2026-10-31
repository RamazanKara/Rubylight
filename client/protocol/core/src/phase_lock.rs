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

    /// [`report`](Self::report) for a window that took `elapsed_ns` of client time since the
    /// last report (zero or less: unknown, the plain report).
    ///
    /// When the stream had a frame for nearly every refresh (90% or more), a frame that misses
    /// its latch waits a whole refresh and lands on the next frame's refresh: one of the two is
    /// never shown. At 60 fps on a 60 Hz screen that is 16.7 ms and a dropped frame per miss,
    /// against 8.3 ms and no drop at 60 fps on 120 Hz. The early edge is then reported lower by
    /// a quarter of the spread, so the host keeps it further from the latch: about the 3rd
    /// percentile at the margin instead of the 10th, still from percentiles that one stray
    /// frame cannot move.
    pub fn report_over(&mut self, min_frames: usize, elapsed_ns: i64) -> Option<Report> {
        let frames = self.count as i64;
        let period = self.period_ns;
        let mut report = self.report(min_frames)?;
        if elapsed_ns > 0 && frames * period * 10 >= elapsed_ns * 9 {
            // The host keeps the edge at its margin or a quarter of the spread; with the extra
            // room the whole cluster must still fit in half a refresh, or its earliest frames
            // would read as late ones across the wrap and the lock would lose the cluster.
            let spread = i64::from(report.spread_ns);
            let target = PhaseLock::DEFAULT_MARGIN_NS.max(spread / 4);
            let room = (period / 2 - spread - target).max(0);
            let lead = i64::from(report.lead_ns) - (spread / 4).min(room);
            report.lead_ns = lead.max(-(period / 2)) as i32;
        }
        Some(report)
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
    /// Phase error of the previous report.
    error_ns: i64,
    /// How far frames move when the host takes a later picture of the source, when it cannot
    /// take an earlier one; 0 when frames move by any amount ([`Self::set_step`]).
    step_ns: i64,
    /// Frames before [`Self::pending_ns`] applies, so a picture move lands when it is due.
    hold_frames: i64,
    /// Frames since the last move of a picture.
    since_move: i64,
    locked: bool,
    /// How much earlier frames reach the client each frame at an interval of exactly the
    /// client's period: the trim that holds them in place. Measured from report to report and
    /// smoothed; `None` until two narrow windows in a row.
    drift_ns: Option<i64>,
    /// The early edge of the last report the drift was measured from, and the frames and the
    /// interval past the period given since.
    drift_lead_ns: Option<i64>,
    drift_frames: i64,
    drift_shift_ns: i64,
}

impl Default for PhaseLock {
    fn default() -> Self {
        Self::new(Self::DEFAULT_MARGIN_NS)
    }
}

impl PhaseLock {
    /// Covers decode-time and network jitter that the 10th percentile does not.
    pub const DEFAULT_MARGIN_NS: i64 = 1_500_000;
    /// Largest change to one frame interval, so motion never visibly stutters. A move of a
    /// picture ([`Self::set_step`]) applies at once: the frame shows a later picture anyway.
    pub const MAX_STEP_NS: i64 = 250_000;
    /// Share of the measured phase error corrected per report.
    const GAIN_NUM: i64 = 1;
    const GAIN_DEN: i64 = 2;
    /// Share of the error per frame added to the clock-rate trim per report.
    const TRIM_DEN: i64 = 16;
    /// Share of the gap to the measured drift the trim closes per report.
    const FOLLOW_DEN: i64 = 4;
    /// Weight of a new sample in the measured drift.
    const DRIFT_DEN: i64 = 4;

    pub fn new(margin_ns: i64) -> Self {
        Self {
            margin_ns,
            period_ns: 0,
            trim_ns: 0,
            pending_ns: 0,
            error_ns: 0,
            step_ns: 0,
            hold_frames: 0,
            since_move: i64::MAX,
            locked: false,
            drift_ns: None,
            drift_lead_ns: None,
            drift_frames: 0,
            drift_shift_ns: 0,
        }
    }

    /// How far frames move when the host takes the source's next picture instead of the one
    /// it would, while it cannot take the one before: every second picture of a 120 fps game at
    /// 60 fps moves a whole picture later or not at all. 0 when frames move by any amount, the
    /// default. The host's `PhaseSync` decides; call once per frame.
    pub fn set_step(&mut self, step_ns: i64) {
        let step_ns = step_ns.max(0);
        // A correction left from the other mode is dropped: a plain lock's microseconds would
        // apply at once in step mode, and a picture move would trickle out 250 us a frame. Step
        // mode has no trim (the source sets the rate), from the first frame on, not only from
        // the next report.
        if (step_ns > 0) != (self.step_ns > 0) {
            self.pending_ns = 0;
            self.hold_frames = 0;
            if step_ns > 0 {
                self.trim_ns = 0;
            }
        }
        self.step_ns = step_ns;
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
        if period <= 0 {
            // No refresh to lock to; a decoded report always has one.
            return;
        }
        let restart = !self.locked || period != self.period_ns;
        if restart {
            // A new client refresh rate: start over.
            *self = Self { step_ns: self.step_ns, ..Self::new(self.margin_ns) };
        }
        self.period_ns = period;
        self.locked = true;
        let lead = i64::from(report.lead_ns);
        // A wide spread needs more room than the fixed margin.
        let target = self.margin_ns.max(i64::from(report.spread_ns) / 4);
        let mut error = lead - target;
        // Never chase more than half a refresh in either direction.
        error = error.clamp(-period / 2, period / 2);
        // Positive error: frames arrive too early, so capture them later. Earlier corrections
        // still in flight are replaced, not added, because the report already reflects them
        // only partially.
        self.pending_ns = error * Self::GAIN_NUM / Self::GAIN_DEN;
        self.hold_frames = 0;
        let previous = core::mem::replace(&mut self.error_ns, error);
        // The frames went round the refresh since the last report, or one window was off.
        let jumped = !restart && (error - previous).abs() > period / 2;
        // A window across a move of a picture (below) shows neither place.
        let window = i64::from(report.frames);
        let spread = i64::from(report.spread_ns);
        let across = self.since_move < window + window / 2 && spread > period / 4;
        if spread > period / 2 {
            // Frames all over the refresh (a busy host): the drift learned before still holds;
            // measure it afresh from the next narrow window.
            self.drift_lead_ns = None;
        } else if !across && !(jumped && self.step_ns == 0) {
            self.measure_drift(lead, period);
        }
        if self.step_ns > 0 {
            // Frames move only later, a picture at a time. Chased from below, the margin asked
            // for earlier frames that never came: the trim ran to its limit while the frames
            // stayed up to a picture late, most of them missing their latch. So frames hold the
            // earliest place with their early edge at the margin or past it, and move a picture
            // later only when that place is gone: frames drifting later move on to the next
            // latch when their early edge would drop under the margin, and frames drifting
            // earlier move once a whole picture of room has opened above it. A quarter of the
            // margin on top keeps two places a picture apart from taking turns. With whole
            // refreshes per picture there is nothing to move. No trim either way: the source
            // sets the rate.
            let step = self.step_ns % period;
            self.trim_ns = 0;
            self.pending_ns = 0;
            if step < period / 16 || step > period - period / 16 || across {
                return;
            }
            // How far the early edge is past the target now, round the refresh (under the
            // target reads as nearly a refresh past it): a report shows where frames were about
            // half its window ago, and they drifted since.
            let drift = self.drift_ns.unwrap_or(0);
            let above = (lead - target + drift * window / 2).rem_euclid(period);
            let room = self.margin_ns / 4;
            let due = if above >= step + room {
                0
            } else if drift > 0 {
                (step + room - above + drift - 1) / drift
            } else if drift < 0 {
                above / -drift + 1
            } else {
                return;
            };
            // A move due within half a window lands at the frame it is due, not up to a report
            // late. A later one waits for the next report, which must show a move before it as
            // across it.
            if due > window / 2 {
                return;
            }
            self.hold_frames = due;
            self.pending_ns = (above + drift * due).rem_euclid(period) / step * step;
            return;
        }
        if jumped {
            // Leave the trim alone: the window says nothing about the clock rate.
            return;
        }
        // The trim integrates the error and follows the measured drift. The error alone could
        // not catch frames that need a large trim: from a trim of 0 at 55 us a frame (a real
        // pair), the phase correction trailed them by more than half a refresh, where their
        // early edge reads late round the refresh, so it pushed them the wrong way and the trim
        // ran off from where they were going; a long busy spell left it there.
        let frames = i64::from(report.frames.max(1));
        let mut trim = self.trim_ns + error / frames / Self::TRIM_DEN;
        if let Some(drift) = self.drift_ns {
            trim += (drift - trim) / Self::FOLLOW_DEN;
        }
        self.trim_ns = trim.clamp(-period / 100, period / 100);
    }

    /// Takes a drift sample from the early edge's move since the last narrow window, less the
    /// interval past the period the host gave meanwhile.
    fn measure_drift(&mut self, lead: i64, period: i64) {
        if let Some(previous) = self.drift_lead_ns.filter(|_| self.drift_frames > 0) {
            let mut moved = (lead - previous + self.drift_shift_ns).rem_euclid(period);
            if moved > period / 2 {
                moved -= period;
            }
            let sample = (moved / self.drift_frames).clamp(-period / 100, period / 100);
            self.drift_ns = Some(self.drift_ns.map_or(sample, |drift| drift + (sample - drift) / Self::DRIFT_DEN));
        }
        self.drift_lead_ns = Some(lead);
        self.drift_frames = 0;
        self.drift_shift_ns = 0;
    }

    /// Interval from this captured frame to the next, given the host's own nominal interval.
    /// Returns the nominal interval until the first report.
    pub fn next_interval_ns(&mut self, nominal_ns: i64) -> i64 {
        if !self.locked {
            return nominal_ns;
        }
        let max = if self.step_ns > 0 { i64::MAX } else { Self::MAX_STEP_NS };
        let step = if self.hold_frames > 0 { 0 } else { self.pending_ns.clamp(-max, max) };
        self.hold_frames = (self.hold_frames - 1).max(0);
        self.pending_ns -= step;
        self.since_move = if self.step_ns > 0 && step != 0 { 0 } else { self.since_move.saturating_add(1) };
        self.drift_frames += 1;
        self.drift_shift_ns += self.trim_ns + step;
        self.period_ns + self.trim_ns + step
    }

    /// Forgets the client, for example after it stops sending reports.
    pub fn reset(&mut self) {
        *self = Self::new(self.margin_ns);
    }
}

#[cfg(test)]
mod tests;
