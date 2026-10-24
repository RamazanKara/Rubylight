//! The raced presenter's scheduling (B10 step 4, docs/beam-racing.md): when the scan starts, which
//! slices go to the screen at which time, and what a slice that comes too late does. Pure
//! arithmetic and a state machine over plain values, so it is tested on synthetic traces; the
//! draws and the presents are in `stream/mod.rs`.
//!
//! The model. A tearing present on an idle variable refresh display starts a new scan. The first
//! picture row is on the screen `delta` after the present was issued (the probe measures it), and
//! row `r` `r * row_ns` after that. A slice presented at `p` is on the screen about `call` later
//! (the present call), so it must be presented before the beam reaches its first row, with a
//! margin `m` for how much that varies. The scan starts as late as the slice that is tightest
//! against the beam needs, and as early as the first slice allows; it is raced only when that
//! start is at least `MIN_GAIN_US` ahead of presenting the whole picture on its final decode.

use std::collections::VecDeque;

use super::pyrowave::BandReady;
use super::scanline::Beam;

/// A frame is raced only when starting the scan on its slices shows its rows at least this much
/// earlier than presenting it whole (us).
pub const MIN_GAIN_US: i64 = 1000;

/// One host batch of wire time (the host paces in batches of at most 1 ms), which a slice can be
/// later than the arrival slope says (us).
pub const HOST_BATCH_US: u64 = 1000;

/// Share of the frame's bytes that have arrived when the first slice is ready, for a stream that
/// has not shown a frame yet (the design's f0).
const FIRST_SLICE_SHARE: f64 = 0.15;

/// Weight of a new frame in the learned slice times.
const LEARN: f64 = 0.125;

/// Presents kept for the latency statistics, and how often they are folded (us).
const RING: usize = 240;
const REFRESH_US: u64 = 5_000_000;

/// The display's scan as the race uses it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scan {
    pub rows: u32,
    /// How long the beam takes per picture row (ns).
    pub row_ns: f64,
    /// From a tearing present on an idle display to the first picture row (us).
    pub delta_us: u64,
}

impl Scan {
    /// The scan for a picture of `picture_rows` rows that the display shows in `height` of its own
    /// rows from row `top` (a picture scaled to fit the screen, letterboxed or not): the beam spends
    /// `height / picture_rows` of its row time on each of the picture's rows, and `top` rows before them.
    pub fn for_picture(beam: &Beam, delta_ns: u64, picture_rows: u32, top: u32, height: u32) -> Self {
        let beam_row_ns = 1e6 / beam.lines_per_ms;
        let rows = picture_rows.max(1);
        Self {
            rows,
            row_ns: beam_row_ns * f64::from(height.max(1)) / f64::from(rows),
            delta_us: ((delta_ns as f64 + f64::from(top) * beam_row_ns) / 1000.0) as u64,
        }
    }

    /// When the beam is on picture row `row`, for a scan whose present was issued at `start_us`.
    pub fn reaches_us(&self, start_us: u64, row: u32) -> u64 {
        start_us + self.delta_us + (f64::from(row) * self.row_ns / 1000.0) as u64
    }

    /// When the beam has left the last row.
    pub fn ends_us(&self, start_us: u64) -> u64 {
        self.reaches_us(start_us, self.rows)
    }
}

/// The rows `first..end` of a picture `height` rows high as a scissor `(top, bottom)`, or None
/// when nothing is left of them.
pub fn scissor(first_row: u32, end_row: u32, height: u32) -> Option<(u32, u32)> {
    let end = end_row.min(height);
    (first_row < end).then_some((first_row, end))
}

/// How old the picture is at the middle row when the beam reaches it (ns): the time the beam
/// gets there for a scan whose present was issued at `present_us`, minus when the slice that
/// holds the row was ready. A slice's decode stands for its band's arrival; step 2 frames (one
/// present on the final decode) and raced frames are measured the same way, from their slices.
pub fn mid_row_age_ns(scan: &Scan, present_us: u64, bands: &[BandReady]) -> Option<i64> {
    let mid = scan.rows / 2;
    let band = bands.iter().find(|band| band.first_row <= mid && mid < band.end_row)?;
    let reached_ns = scan.reaches_us(present_us, mid) as i64 * 1000;
    Some(reached_ns - band.ready_us as i64 * 1000)
}

fn percentile(sorted: &[u64], per_mille: usize) -> u64 {
    sorted[(sorted.len() * per_mille / 1000).min(sorted.len() - 1)]
}

/// The wake of a present that has no slice ready time (a whole picture).
const NO_WAKE: u64 = u64::MAX;

/// What the presents of slices cost, folded every few seconds from the last window.
pub struct Latency {
    samples: VecDeque<(u64, u64)>,
    /// From a slice being ready to its present being issued (us), p50.
    wake_us: u64,
    /// The present call (us), p50.
    call_us: u64,
    /// p99 - p50 of ready-to-returned (us).
    spread_us: u64,
    folded_us: u64,
}

impl Default for Latency {
    fn default() -> Self {
        // What a present costs before any was timed: a plain guess, replaced by the first fold.
        Self { samples: VecDeque::new(), wake_us: 300, call_us: 1200, spread_us: 1000, folded_us: 0 }
    }
}

impl Latency {
    /// Fixed values (tests and the first frames).
    #[cfg(test)]
    pub fn fixed(wake_us: u64, call_us: u64, spread_us: u64) -> Self {
        Self { samples: VecDeque::new(), wake_us, call_us, spread_us, folded_us: u64::MAX }
    }

    /// One present: `ready_us` when its slice was ready, `issued_us` and `returned_us` around the call.
    #[cfg(any(windows, test))]
    pub fn record(&mut self, ready_us: u64, issued_us: u64, returned_us: u64, now_us: u64) {
        self.push(issued_us.saturating_sub(ready_us), returned_us.saturating_sub(issued_us), now_us);
    }

    /// A present of a whole picture, which says what the call costs before any slice was presented.
    pub fn record_call(&mut self, issued_us: u64, returned_us: u64, now_us: u64) {
        self.push(NO_WAKE, returned_us.saturating_sub(issued_us), now_us);
    }

    fn push(&mut self, wake_us: u64, call_us: u64, now_us: u64) {
        if self.samples.len() == RING { self.samples.pop_front(); }
        self.samples.push_back((wake_us, call_us));
        if self.folded_us == 0 || (self.folded_us != u64::MAX && now_us.saturating_sub(self.folded_us) >= REFRESH_US && self.samples.len() >= 20) {
            self.fold(now_us);
        }
    }

    fn fold(&mut self, now_us: u64) {
        if self.samples.len() < 8 { return; }
        self.folded_us = now_us;
        let mut wake: Vec<u64> = self.samples.iter().map(|s| s.0).filter(|&w| w != NO_WAKE).collect();
        let mut call: Vec<u64> = self.samples.iter().map(|s| s.1).collect();
        let mut total: Vec<u64> = self.samples.iter().filter(|s| s.0 != NO_WAKE).map(|s| s.0 + s.1).collect();
        call.sort_unstable();
        self.call_us = percentile(&call, 500);
        if wake.len() >= 8 {
            wake.sort_unstable();
            total.sort_unstable();
            self.wake_us = percentile(&wake, 500);
            self.spread_us = percentile(&total, 990).saturating_sub(percentile(&total, 500));
        } else {
            // Only whole pictures were presented: the call's own spread, the wake as guessed.
            self.spread_us = percentile(&call, 990).saturating_sub(self.call_us);
        }
    }

    pub fn wake_us(&self) -> u64 { self.wake_us }
    pub fn call_us(&self) -> u64 { self.call_us }
    pub fn spread_us(&self) -> u64 { self.spread_us }

    /// The slowest present call in the window (us), for the end-of-stream line.
    pub fn slowest_call_us(&self) -> u64 { self.samples.iter().map(|s| s.1).max().unwrap_or(0) }
    /// The present call's p95 over the window (us), for the end-of-stream line.
    pub fn call_p95_us(&self) -> u64 {
        let mut call: Vec<u64> = self.samples.iter().map(|s| s.1).collect();
        call.sort_unstable();
        if call.is_empty() { 0 } else { percentile(&call, 950) }
    }

    /// The margin ahead of the beam (us): how much a present varies plus one host batch, or the
    /// user's own value when the setting is not 0.
    pub fn margin_us(&self, setting_ms: u32) -> u64 {
        if setting_ms > 0 { u64::from(setting_ms) * 1000 } else { self.spread_us + HOST_BATCH_US }
    }
}

/// When each slice of a frame becomes ready relative to the first, and the row each starts at,
/// learned from the frames shown (or guessed from the pace for the first ones).
pub struct Predictor {
    offsets_us: Vec<f64>,
    first_rows: Vec<u32>,
    learned: bool,
}

impl Predictor {
    /// A guess from the frame's size and the pace the host sends it at: slice `i` is ready when
    /// the share of the frame that holds its rows has arrived, rows split evenly.
    pub fn seeded(slices: u32, rows: u32, frame_bytes: f64, pace_mbps: u32) -> Self {
        let n = slices.max(1);
        let wire_us = if pace_mbps == 0 { 0.0 } else { frame_bytes * 8.0 / f64::from(pace_mbps) };
        let share = |end: u32| FIRST_SLICE_SHARE + (1.0 - FIRST_SLICE_SHARE) * f64::from(end) / f64::from(rows.max(1));
        let end_of = |i: u32| if i + 1 == n { rows } else { (i + 1) * rows / n };
        let first = share(end_of(0));
        Self {
            offsets_us: (0..n).map(|i| wire_us * (share(end_of(i)) - first)).collect(),
            first_rows: (0..n).map(|i| i * rows / n).collect(),
            learned: false,
        }
    }

    /// Folds a frame shown with these slices in. Frames that were not sliced like the learned
    /// ones replace them.
    pub fn learn(&mut self, bands: &[BandReady]) {
        if bands.len() < 2 { return; }
        let base = bands[0].ready_us;
        let offsets: Vec<f64> = bands.iter().map(|band| band.ready_us.saturating_sub(base) as f64).collect();
        if self.learned && self.offsets_us.len() == offsets.len() {
            for (known, seen) in self.offsets_us.iter_mut().zip(&offsets) { *known += (seen - *known) * LEARN; }
        } else {
            self.offsets_us = offsets;
        }
        self.first_rows = bands.iter().map(|band| band.first_row).collect();
        self.learned = true;
    }

    /// `(offset from the first slice, first row)` of every slice.
    pub fn slices(&self) -> impl Iterator<Item = (f64, u32)> + '_ {
        self.offsets_us.iter().copied().zip(self.first_rows.iter().copied())
    }

    pub fn len(&self) -> usize { self.offsets_us.len() }

    /// Whether the offsets come from frames shown and not from the pace.
    #[cfg(test)]
    pub fn learned(&self) -> bool { self.learned }

    /// The predicted offset of the last slice from the first one (us); 0 with none.
    pub fn span_us(&self) -> f64 { self.offsets_us.last().copied().unwrap_or(0.0) }
}

/// Where the scan starts for a frame and what it gains.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    /// When to issue the present that starts the scan (session clock, us).
    pub start_us: u64,
    /// How much earlier than presenting whole on the last slice the picture's rows are shown (us).
    pub gain_us: i64,
}

/// Why a frame is not raced (the overlay's reason and the counters).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skip {
    /// The slices would not be early enough ahead of the beam to gain `MIN_GAIN_US`.
    NoGain,
    /// The frame did not arrive in row order, or had no slices.
    Unsliced,
    /// The first slice seen was not the picture's first.
    Missed,
}

impl Skip {
    pub fn label(self) -> &'static str {
        match self {
            Skip::NoGain => "pace leaves under 1 ms to gain",
            Skip::Unsliced => "frames not in slices",
            Skip::Missed => "first slice missed",
        }
    }

    fn index(self) -> usize {
        match self {
            Skip::NoGain => 0,
            Skip::Unsliced => 1,
            Skip::Missed => 2,
        }
    }
}

/// The scan start for a frame whose first slice was ready at `first_ready_us`, or why not. The
/// scan cannot start before `earliest_us` (the last scan has not ended).
pub fn plan(scan: &Scan, first_ready_us: u64, predictor: &Predictor, latency: &Latency, margin_us: u64, earliest_us: u64) -> Result<Plan, Skip> {
    if predictor.len() < 2 { return Err(Skip::Unsliced); }
    let (wake, call) = (latency.wake_us() as f64, latency.call_us() as f64);
    let (margin, delta) = (margin_us as f64, scan.delta_us as f64);
    // The first slice is presented as soon as it is ready; each later one must be on the screen
    // `margin` before the beam reaches its first row.
    let mut offset = wake;
    for (ready, first_row) in predictor.slices() {
        let beam = f64::from(first_row) * scan.row_ns / 1000.0;
        offset = offset.max(ready + wake + call + margin - delta - beam);
    }
    let start_us = (first_ready_us + offset as u64).max(earliest_us);
    let whole_us = first_ready_us + (predictor.span_us() + wake) as u64;
    let gain_us = whole_us as i64 - start_us as i64;
    if gain_us < MIN_GAIN_US { return Err(Skip::NoGain); }
    Ok(Plan { start_us, gain_us })
}

/// Whether the beam is past the first row of a slice presented at `now_us` for the scan that
/// was issued at `start_us`: its rows are already scanned from the picture before it.
pub fn overtaken(scan: &Scan, start_us: u64, now_us: u64, call_us: u64, first_row: u32) -> bool {
    now_us + call_us >= scan.reaches_us(start_us, first_row)
}

/// What the racer is doing with the frame in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Nothing seen of the frame yet.
    Fresh,
    /// Planned, the scan not started.
    Waiting,
    /// The scan runs; slices are presented as they are ready.
    // Entered by `take`, which only the Windows presenter and the tests call.
    #[cfg_attr(not(any(windows, test)), allow(dead_code))]
    Scanning,
    /// Not raced: presented whole on the final decode.
    Whole,
    /// A slice was late: the rest of the scan runs on what is there; presented whole on the final decode.
    Held,
}

/// How the frame's final decode is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Final {
    /// The whole picture, rows `0..height`.
    Whole,
    /// Only the rows after the slices that were presented.
    Rows(u32, u32),
}

/// What the timing CSV and the counters take from a frame once it is done.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Record {
    /// The frame's presents were issued at these times (session clock, us), the first starting the scan.
    pub presents_us: Vec<u64>,
    pub late: u32,
    pub raced: bool,
    pub skip: Option<Skip>,
}

/// What the racer has done over the stream, for the overlay.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    /// Frames shown with their slices presented ahead of the beam.
    pub raced: u64,
    /// Frames shown whole: not raced (see `skipped`) or raced and cut short by a late slice.
    pub whole: u64,
    /// Slices (the final decode too) that came when the beam was past their rows.
    pub late_slices: u64,
    /// Frames with a late slice.
    pub late_frames: u64,
    /// Raced frames whose picture never came (a newer frame ended them).
    pub abandoned: u64,
    /// The reason of the last frame that was not raced.
    pub last_skip: Option<Skip>,
    /// Frames not raced, by reason (`Skip::NoGain`, `Unsliced`, `Missed`).
    pub skipped: [u64; 3],
}

/// The raced presenter's state for the frame in flight. `T` is what a slice carries to the draw
/// (the plane set it is read from).
pub struct Racer<T> {
    seq: u64,
    state: State,
    queued: Vec<(BandReady, T)>,
    /// Rows `0..covered` are in the video texture from this frame's slices.
    covered: u32,
    plan: Option<Plan>,
    start_us: u64,
    /// When the scan of the frame in flight was due to start (us), 0 before it is planned.
    due_start_us: u64,
    last_scan_end_us: u64,
    ready: Vec<BandReady>,
    record: Record,
    pub totals: Totals,
}

impl<T> Default for Racer<T> {
    fn default() -> Self {
        Self { seq: 0, state: State::Fresh, queued: Vec::new(), covered: 0, plan: None, start_us: 0, due_start_us: 0, last_scan_end_us: 0, ready: Vec::new(), record: Record::default(), totals: Totals::default() }
    }
}

impl<T> Racer<T> {
    #[cfg(test)]
    pub fn seq(&self) -> u64 { self.seq }

    /// The scan of the frame in flight started at this time (us), if it did.
    #[cfg(test)]
    pub fn scan_start_us(&self) -> Option<u64> {
        matches!(self.state, State::Scanning | State::Held).then_some(self.start_us)
    }

    /// Takes a slice ready at `band.ready_us` of frame `seq`. A slice of an older frame is dropped.
    /// `now_us` is when the presenter sees it. Returns what to do with the frame the slice
    /// belongs to when this is its first slice.
    #[allow(clippy::too_many_arguments)] // the frame's view of the scan, the plans and the clock, passed as they are
    pub fn observe(&mut self, seq: u64, band: BandReady, payload: T, now_us: u64, scan: &Scan, predictor: &Predictor, latency: &Latency, margin_us: u64) {
        if seq < self.seq { return; }
        if seq > self.seq {
            self.begin(seq);
            self.state = if band.slice != 0 || band.first_row != 0 {
                self.record.skip = Some(Skip::Missed);
                State::Whole
            } else {
                match plan(scan, band.ready_us, predictor, latency, margin_us, self.last_scan_end_us) {
                    Ok(plan) => { self.plan = Some(plan); self.record.raced = true; State::Waiting }
                    Err(skip) => { self.record.skip = Some(skip); State::Whole }
                }
            };
        }
        self.ready.push(band);
        match self.state {
            State::Waiting => self.queued.push((band, payload)),
            State::Scanning => {
                if overtaken(scan, self.start_us, now_us, latency.call_us(), band.first_row) {
                    self.note_late();
                    self.state = State::Held;
                } else {
                    self.queued.push((band, payload));
                }
            }
            State::Fresh | State::Whole | State::Held => {}
        }
    }

    fn note_late(&mut self) {
        self.record.late += 1;
        self.totals.late_slices += 1;
        if self.record.late == 1 { self.totals.late_frames += 1; }
    }

    fn begin(&mut self, seq: u64) {
        if matches!(self.state, State::Waiting | State::Scanning | State::Held) { self.totals.abandoned += 1; }
        self.plan = None;
        self.seq = seq;
        self.queued.clear();
        self.covered = 0;
        self.start_us = 0;
        self.due_start_us = 0;
        self.ready.clear();
        self.record = Record::default();
    }

    /// When the loop must wake for the frame: the planned scan start, or now for slices waiting
    /// to be presented in a running scan.
    pub fn due_us(&self) -> Option<u64> {
        match self.state {
            State::Waiting => self.plan.map(|plan| plan.start_us),
            State::Scanning if !self.queued.is_empty() => Some(0),
            _ => None,
        }
    }

    /// The slices to draw and present now, if the scan starts or a slice is waiting.
    #[cfg(any(windows, test))]
    pub fn take(&mut self, now_us: u64) -> Vec<(BandReady, T)> {
        match self.state {
            State::Waiting if self.plan.is_some_and(|plan| now_us >= plan.start_us) => {
                self.state = State::Scanning;
                self.start_us = now_us;
                self.due_start_us = self.plan.map_or(now_us, |plan| plan.start_us);
            }
            State::Scanning => {}
            _ => return Vec::new(),
        }
        let batch = std::mem::take(&mut self.queued);
        for (band, _) in &batch { self.covered = self.covered.max(band.end_row); }
        batch
    }

    /// When a slice that was ready at `ready_us` could have been presented: not before the scan was
    /// due to start, which the plan delays on purpose. The wake the latency statistics see starts here.
    #[cfg(any(windows, test))]
    pub fn presentable_us(&self, ready_us: u64) -> u64 {
        ready_us.max(self.due_start_us)
    }

    /// A present of the frame was issued and returned.
    pub fn presented(&mut self, issued_us: u64, scan: &Scan) {
        self.record.presents_us.push(issued_us);
        if self.record.presents_us.len() == 1 { self.last_scan_end_us = scan.ends_us(issued_us); }
    }

    /// The frame's final decode (rows `first_row..end_row`) is to be presented at `now_us`: how to
    /// draw it. It is drawn whole unless the scan runs on the slices presented so far and the
    /// final rows follow them.
    pub fn finish(&mut self, seq: u64, first_row: u32, now_us: u64, scan: &Scan, latency: &Latency) -> Final {
        if seq != self.seq { return Final::Whole; }
        match self.state {
            State::Scanning if self.queued.is_empty() && self.covered == first_row && first_row > 0 => {
                if overtaken(scan, self.start_us, now_us, latency.call_us(), first_row) {
                    self.note_late();
                    self.state = State::Held;
                    Final::Whole
                } else {
                    Final::Rows(first_row, scan.rows)
                }
            }
            State::Scanning => {
                // Slices waiting or missing: the whole picture is the safe draw.
                self.state = State::Held;
                Final::Whole
            }
            State::Held | State::Waiting | State::Whole | State::Fresh => {
                if self.state == State::Waiting { self.record.raced = false; }
                Final::Whole
            }
        }
    }

    /// The frame is shown: its record. The racer is ready for the next frame.
    pub fn done(&mut self) -> Record {
        self.state = State::Fresh;
        self.queued.clear();
        let record = std::mem::take(&mut self.record);
        if record.raced && record.late == 0 { self.totals.raced += 1; } else { self.totals.whole += 1; }
        if let Some(skip) = record.skip {
            self.totals.last_skip = Some(skip);
            self.totals.skipped[skip.index()] += 1;
        }
        record
    }

    /// The slices of the frame as seen so far.
    #[cfg(test)]
    pub fn slices(&self) -> &[BandReady] { &self.ready }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1080 rows, 3.2 us per row (3.46 ms a scan), the first row 800 us after a present.
    fn scan() -> Scan { Scan { rows: 1080, row_ns: 3200.0, delta_us: 800 } }

    fn band(slice: u8, first_row: u32, end_row: u32, ready_us: u64) -> BandReady {
        BandReady { slice, first_row, end_row, fence_value: u64::from(slice) + 1, ready_us }
    }

    /// Four slices of a frame whose first slice is ready at `at` and whose last is ready `span` later.
    fn frame(at: u64, span: u64) -> Vec<BandReady> {
        let rows = [(0, 270), (270, 540), (540, 810), (810, 1080)];
        rows.iter().enumerate().map(|(i, &(a, b))| band(i as u8, a, b, at + span * i as u64 / 3)).collect()
    }

    fn learned(span: u64) -> Predictor {
        let mut predictor = Predictor::seeded(4, 1080, 0.0, 0);
        predictor.learn(&frame(0, span));
        predictor
    }

    fn quiet() -> Latency { Latency::fixed(100, 500, 100) }

    #[test]
    fn a_scissor_is_the_rows_of_the_slice_inside_the_picture() {
        assert_eq!(scissor(0, 270, 1080), Some((0, 270)));
        assert_eq!(scissor(810, 1080, 1080), Some((810, 1080)));
        assert_eq!(scissor(810, 5000, 1080), Some((810, 1080)), "rows past the picture are cut");
        assert_eq!(scissor(1080, 1080, 1080), None);
        assert_eq!(scissor(300, 200, 1080), None, "an empty slice draws nothing");
    }

    #[test]
    fn the_beam_reaches_a_row_after_the_start_delay_and_its_share_of_the_scan() {
        let scan = scan();
        assert_eq!(scan.reaches_us(10_000, 0), 10_800);
        assert_eq!(scan.reaches_us(10_000, 540), 10_800 + 1728);
        assert_eq!(scan.ends_us(10_000), 10_800 + 3456);
    }

    #[test]
    fn a_scaled_or_letterboxed_picture_gets_its_share_of_the_beam() {
        let beam = Beam {
            rows: 1440, blank_rows: 60, total_rows: 1500, scan_ns: 4_166_667, refresh_hz: 240.0, lines_per_ms: 360.0,
            vblank_fraction: 0.04, scans: 20, idle_from: 1000, longest_idle_ns: 9_000_000,
        };
        // 1080 rows shown over 1440 display rows: 4/3 of the beam's 2.78 us per row for each.
        let fit = Scan::for_picture(&beam, 800_000, 1080, 0, 1440);
        assert_eq!(fit.rows, 1080);
        assert!((fit.row_ns - 3703.7).abs() < 0.1, "{}", fit.row_ns);
        assert_eq!(fit.delta_us, 800);
        // Black bars of 90 rows above the picture delay its first row by their beam time.
        let boxed = Scan::for_picture(&beam, 800_000, 1080, 90, 1260);
        assert_eq!(boxed.delta_us, 800 + 250);
        assert!((boxed.row_ns - 3240.7).abs() < 0.1, "{}", boxed.row_ns);
        let whole = Scan::for_picture(&beam, 0, 1440, 0, 1440).ends_us(0);
        assert!(whole.abs_diff(4000) <= 1, "a picture that fills the display takes its rows' time: {whole}");
    }

    #[test]
    fn a_fast_pace_gives_no_race() {
        // The whole frame is in 0.9 ms: nothing to gain over presenting it whole.
        let result = plan(&scan(), 50_000, &learned(900), &quiet(), 1100, 0);
        assert_eq!(result, Err(Skip::NoGain));
    }

    #[test]
    fn a_slow_pace_starts_the_scan_ahead_of_presenting_whole() {
        // 12 ms from the first slice to the last: the scan runs 3.5 ms, the picture is shown ~6 ms earlier.
        let predictor = learned(12_000);
        let plan = plan(&scan(), 50_000, &predictor, &quiet(), 1100, 0).expect("races");
        assert!(plan.start_us >= 50_000 + 100, "not before the first slice is presented");
        assert!(plan.gain_us >= MIN_GAIN_US, "{plan:?}");
        // Every slice is presented before the beam reaches its rows.
        let scan = scan();
        for (offset, first_row) in predictor.slices() {
            let presented = 50_000 + offset as u64 + 100;
            assert!(presented + 500 + 1100 <= scan.reaches_us(plan.start_us, first_row), "slice at row {first_row} would be overtaken: {plan:?}");
        }
        // And the scan is not started earlier than that: one slice is just on the margin.
        let tight = predictor.slices().any(|(offset, first_row)| {
            let presented = 50_000 + offset as u64 + 100;
            presented + 500 + 1100 + 1000 > scan.reaches_us(plan.start_us, first_row)
        });
        assert!(tight, "the start leaves slack that gains nothing: {plan:?}");
    }

    #[test]
    fn a_pace_in_between_gains_less_and_the_gain_is_capped_by_the_scan() {
        let slow = plan(&scan(), 0, &learned(12_000), &quiet(), 1100, 0).unwrap().gain_us;
        let middle = plan(&scan(), 0, &learned(2_000), &quiet(), 1100, 0).unwrap().gain_us;
        let slower = plan(&scan(), 0, &learned(30_000), &quiet(), 1100, 0).unwrap().gain_us;
        assert!(middle < slow, "{middle} {slow}");
        assert_eq!(slow, slower, "past the scan's own length a slower pace gains no more");
    }

    #[test]
    fn a_scan_that_has_not_ended_holds_the_next_start() {
        let free = plan(&scan(), 50_000, &learned(12_000), &quiet(), 1100, 0).unwrap();
        let held = plan(&scan(), 50_000, &learned(12_000), &quiet(), 1100, free.start_us + 1500);
        assert!(held.map_or(true, |p| p.start_us == free.start_us + 1500 && p.gain_us < free.gain_us));
    }

    #[test]
    fn a_bigger_margin_starts_later_and_gains_less() {
        let small = plan(&scan(), 0, &learned(12_000), &quiet(), 600, 0).unwrap();
        let big = plan(&scan(), 0, &learned(12_000), &quiet(), 1500, 0).unwrap();
        assert!(big.start_us > small.start_us && big.gain_us < small.gain_us);
    }

    #[test]
    fn the_margin_is_the_spread_plus_a_host_batch_unless_the_setting_gives_one() {
        let latency = Latency::fixed(100, 500, 400);
        assert_eq!(latency.margin_us(0), 1400);
        assert_eq!(latency.margin_us(3), 3000);
    }

    #[test]
    fn latency_folds_its_window_into_the_median_and_the_spread() {
        let mut latency = Latency::default();
        for i in 0..100u64 {
            // Ready at 0, issued 100 us later, the call takes 400 us; one in twenty takes 2 ms more.
            latency.record(0, 100, 500 + if i % 20 == 0 { 2000 } else { 0 }, 1_000 + i);
        }
        latency.record(0, 100, 500, 6_000_000);
        assert_eq!((latency.wake_us(), latency.call_us()), (100, 400));
        assert_eq!(latency.margin_us(0), 2000 + HOST_BATCH_US, "p99 - p50 of the whole present, plus a host batch");
    }

    #[test]
    fn whole_pictures_teach_the_call_before_any_slice_was_presented() {
        let mut latency = Latency::default();
        assert_eq!((latency.wake_us(), latency.call_us()), (300, 1200), "guessed before anything was timed");
        for i in 0..100u64 {
            latency.record_call(100, 100 + 250 + if i % 20 == 0 { 700 } else { 0 }, 1_000 + i);
        }
        latency.record_call(100, 350, 6_000_000);
        assert_eq!(latency.call_us(), 250);
        assert_eq!(latency.wake_us(), 300, "the wake stays as guessed without slice presents");
        assert_eq!(latency.margin_us(0), 700 + HOST_BATCH_US);
    }

    #[test]
    fn the_predictor_is_seeded_from_the_pace_and_then_learns_the_frames() {
        // 831 KB at 900 Mbps: 7.4 ms on the wire.
        let predictor = Predictor::seeded(4, 1080, 831_000.0, 900);
        let offsets: Vec<f64> = predictor.slices().map(|(offset, _)| offset).collect();
        assert_eq!(offsets[0], 0.0);
        assert!(offsets.windows(2).all(|pair| pair[1] > pair[0]));
        assert!((offsets[3] - 7_388.0 * 0.85 * 0.75).abs() < 800.0, "{offsets:?}");
        assert!(!predictor.learned());
        let mut predictor = predictor;
        for _ in 0..50 { predictor.learn(&frame(1_000, 9_000)); }
        assert!(predictor.learned());
        let last = predictor.slices().last().unwrap();
        assert!((last.0 - 9_000.0).abs() < 50.0 && last.1 == 810, "{last:?}");
    }

    #[test]
    fn a_frame_without_slices_is_not_raced() {
        let predictor = Predictor::seeded(1, 1080, 1.0, 900);
        assert_eq!(plan(&scan(), 0, &predictor, &quiet(), 1100, 0), Err(Skip::Unsliced));
    }

    fn racer_with_frame(span: u64) -> (Racer<u8>, Predictor, Vec<BandReady>) {
        let bands = frame(50_000, span);
        (Racer::default(), learned(span), bands)
    }

    #[test]
    fn slices_are_presented_at_their_times_and_the_final_draws_the_rest() {
        let (mut racer, predictor, bands) = racer_with_frame(12_000);
        let (scan, latency) = (scan(), quiet());
        racer.observe(1, bands[0], 0, 50_100, &scan, &predictor, &latency, 1100);
        let start = racer.due_us().expect("planned");
        assert!(start > 50_100, "the scan waits for the slices behind the beam");
        assert!(racer.take(start - 1).is_empty(), "not before the start");
        let first = racer.take(start);
        assert_eq!(first.len(), 1);
        racer.presented(start, &scan);
        assert_eq!(racer.scan_start_us(), Some(start));
        // The later slices come at their times and are drawn as they are ready.
        for (i, b) in bands.iter().enumerate().skip(1).take(2) {
            let at = b.ready_us + 100;
            racer.observe(1, *b, i as u8, at, &scan, &predictor, &latency, 1100);
            let batch = racer.take(at);
            assert_eq!(batch.len(), 1, "slice {i} is presented when it is ready");
            racer.presented(at, &scan);
        }
        let last = bands[3];
        assert_eq!(racer.finish(1, last.first_row, last.ready_us + 100, &scan, &latency), Final::Rows(810, 1080));
        let record = racer.done();
        assert_eq!((record.late, record.raced, record.presents_us.len()), (0, true, 3));
    }

    #[test]
    fn the_wake_of_a_slice_starts_when_the_scan_was_due_not_when_it_was_ready() {
        let (mut racer, predictor, bands) = racer_with_frame(12_000);
        let (scan, latency) = (scan(), quiet());
        racer.observe(3, bands[0], 0, 50_100, &scan, &predictor, &latency, 1100);
        let start = racer.due_us().unwrap();
        assert!(start > bands[0].ready_us + 500, "the plan waits on purpose");
        racer.take(start + 60);
        assert_eq!(racer.presentable_us(bands[0].ready_us), start, "the wait is not a wake");
        assert_eq!(racer.presentable_us(start + 900), start + 900, "a slice ready after the start is timed from then");
    }

    #[test]
    fn a_late_slice_holds_the_rest_and_the_frame_is_presented_whole() {
        let (mut racer, predictor, bands) = racer_with_frame(12_000);
        let (scan, latency) = (scan(), quiet());
        racer.observe(7, bands[0], 0, 50_100, &scan, &predictor, &latency, 1100);
        let start = racer.due_us().unwrap();
        racer.take(start);
        racer.presented(start, &scan);
        // Slice 2 turns up when the beam is long past its first row (540).
        let late = scan.reaches_us(start, 540) + 300;
        racer.observe(7, bands[1], 1, late, &scan, &predictor, &latency, 1100);
        assert!(racer.take(late).is_empty(), "a late slice is held, not presented");
        racer.observe(7, bands[2], 2, late + 10, &scan, &predictor, &latency, 1100);
        assert!(racer.take(late + 10).is_empty(), "and so are the ones after it");
        assert_eq!(racer.finish(7, 810, late + 5000, &scan, &latency), Final::Whole);
        let record = racer.done();
        assert_eq!(record.late, 1, "counted once");
        assert!(record.raced);
    }

    #[test]
    fn a_final_decode_late_against_the_beam_is_counted_and_drawn_whole() {
        let (mut racer, predictor, bands) = racer_with_frame(12_000);
        let (scan, latency) = (scan(), quiet());
        racer.observe(2, bands[0], 0, 50_100, &scan, &predictor, &latency, 1100);
        let start = racer.due_us().unwrap();
        racer.take(start);
        racer.presented(start, &scan);
        for (i, b) in bands.iter().enumerate().skip(1).take(2) {
            racer.observe(2, *b, i as u8, b.ready_us + 100, &scan, &predictor, &latency, 1100);
            racer.take(b.ready_us + 100);
        }
        let after_scan = scan.ends_us(start) + 100;
        assert_eq!(racer.finish(2, 810, after_scan, &scan, &latency), Final::Whole);
        assert_eq!(racer.done().late, 1);
    }

    #[test]
    fn a_final_before_the_scan_starts_is_drawn_whole_and_not_raced() {
        let (mut racer, predictor, bands) = racer_with_frame(12_000);
        let (scan, latency) = (scan(), quiet());
        racer.observe(3, bands[0], 0, 50_100, &scan, &predictor, &latency, 1100);
        assert_eq!(racer.finish(3, 810, 50_200, &scan, &latency), Final::Whole);
        let record = racer.done();
        assert!(!record.raced && record.late == 0 && record.presents_us.is_empty());
    }

    #[test]
    fn a_fast_frame_is_not_raced_and_says_why() {
        let (scan, latency) = (scan(), quiet());
        let predictor = learned(800);
        let mut racer = Racer::<u8>::default();
        let bands = frame(50_000, 800);
        racer.observe(1, bands[0], 0, 50_100, &scan, &predictor, &latency, 1100);
        assert_eq!(racer.due_us(), None);
        assert_eq!(racer.finish(1, 810, 50_900, &scan, &latency), Final::Whole);
        assert_eq!(racer.done().skip, Some(Skip::NoGain));
        assert_eq!((racer.totals.skipped, racer.totals.whole), ([1, 0, 0], 1), "counted by reason");
    }

    #[test]
    fn a_frame_that_starts_past_its_first_slice_is_presented_whole() {
        let (scan, latency) = (scan(), quiet());
        let mut racer = Racer::<u8>::default();
        racer.observe(1, band(1, 270, 540, 50_000), 0, 50_100, &scan, &learned(12_000), &latency, 1100);
        assert_eq!(racer.due_us(), None);
        assert_eq!(racer.done().skip, Some(Skip::Missed));
    }

    #[test]
    fn the_slices_of_an_older_frame_are_dropped() {
        let (mut racer, predictor, bands) = racer_with_frame(12_000);
        let (scan, latency) = (scan(), quiet());
        racer.observe(5, bands[0], 0, 50_100, &scan, &predictor, &latency, 1100);
        racer.observe(4, bands[1], 1, 50_200, &scan, &predictor, &latency, 1100);
        assert_eq!(racer.seq(), 5);
        assert_eq!(racer.slices().len(), 1, "the older frame's slice is not mixed into this one");
        assert_eq!(racer.finish(4, 810, 60_000, &scan, &latency), Final::Whole, "a final of another frame is drawn whole");
    }

    #[test]
    fn a_newer_frame_ends_one_that_never_finished() {
        let (mut racer, predictor, bands) = racer_with_frame(12_000);
        let (scan, latency) = (scan(), quiet());
        racer.observe(1, bands[0], 0, 50_100, &scan, &predictor, &latency, 1100);
        let start = racer.due_us().unwrap();
        racer.take(start);
        racer.presented(start, &scan);
        let next = frame(120_000, 12_000);
        racer.observe(2, next[0], 0, 120_100, &scan, &predictor, &latency, 1100);
        assert_eq!(racer.seq(), 2);
        assert!(racer.due_us().is_some_and(|due| due > 120_000), "the new frame is planned from its own first slice");
    }

    #[test]
    fn the_next_scan_waits_for_the_last_one_to_end() {
        let (scan, latency) = (scan(), quiet());
        let predictor = learned(12_000);
        let mut racer = Racer::<u8>::default();
        let bands = frame(50_000, 12_000);
        racer.observe(1, bands[0], 0, 50_100, &scan, &predictor, &latency, 1100);
        let start = racer.due_us().unwrap();
        racer.take(start);
        racer.presented(start, &scan);
        racer.finish(1, 810, start + 100, &scan, &latency);
        racer.done();
        // A frame whose first slice is ready in the middle of that scan.
        let early = frame(start + 500, 12_000);
        racer.observe(2, early[0], 0, start + 600, &scan, &predictor, &latency, 1100);
        if let Some(due) = racer.due_us() { assert!(due >= scan.ends_us(start), "the scan has not ended: {due}"); }
    }

    #[test]
    fn mid_row_age_is_measured_the_same_way_for_both_modes() {
        let scan = scan();
        let bands = frame(50_000, 12_000);
        // Step 2: one present on the final decode. The beam is at row 540 at present + 800 + 1728 us.
        let last_ready = bands[3].ready_us;
        let whole = mid_row_age_ns(&scan, last_ready + 100, &bands).unwrap();
        assert_eq!(whole, ((last_ready + 100 + 800 + 1728) as i64 - bands[2].ready_us as i64) * 1000, "the middle row is in slice 2");
        // Raced: the scan starts well before.
        let raced = mid_row_age_ns(&scan, 54_000, &bands).unwrap();
        assert!(raced < whole, "{raced} {whole}");
        assert_eq!(whole - raced, (last_ready + 100 - 54_000) as i64 * 1000);
        // An unsliced frame has one band for the whole picture: its own ready time.
        let one = [band(0, 0, 1080, 60_000)];
        assert_eq!(mid_row_age_ns(&scan, 60_100, &one), Some((60_100 + 800 + 1728 - 60_000) as i64 * 1000));
        assert_eq!(mid_row_age_ns(&scan, 60_100, &[]), None);
    }
}
