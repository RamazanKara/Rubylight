//! The raced presenter's scheduling (B10 step 4, docs/beam-racing.md): when the scan starts, which
//! slices go to the screen at which time, and what a slice that comes too late does. Pure
//! arithmetic and a state machine over plain values, so it is tested on synthetic traces; the
//! draws and the presents are in `stream/render.rs`.
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

/// Slices drawn in one pass: the rows `first_row..end_row` of consecutive slices of one plane
/// set, ready once the newest of their fences is; `last` is the index of the final slice.
#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub first_row: u32,
    pub end_row: u32,
    pub fence_value: u64,
    pub last: usize,
}

/// Joins the slices that follow one another in the picture (and sit in the same plane set) into
/// runs. Each draw costs the presenting thread 0.2 ms, and a frame's final decode waits for it.
#[cfg(any(windows, test))]
pub fn runs(bands: impl Iterator<Item = (BandReady, usize)>) -> Vec<Run> {
    let mut runs: Vec<(Run, usize)> = Vec::new();
    for (index, (band, set)) in bands.enumerate() {
        match runs.last_mut() {
            Some((run, run_set)) if run.end_row == band.first_row && *run_set == set => {
                run.end_row = band.end_row;
                run.fence_value = run.fence_value.max(band.fence_value);
                run.last = index;
            }
            _ => runs.push((Run { first_row: band.first_row, end_row: band.end_row, fence_value: band.fence_value, last: index }, set)),
        }
    }
    runs.into_iter().map(|(run, _)| run).collect()
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

/// The next frame sequence number. It counts across decoders: a rebuilt `GpuStream` (or a new
/// slice setting) must not restart at 1, because the racer drops slices and pictures that are
/// older than the newest frame it has seen, and would freeze the video on the restart.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn next_seq() -> u64 {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
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
    /// The newest frame with rows on the screen from a present of the racer (0 before the first).
    shown_seq: u64,
    pub totals: Totals,
}

impl<T> Default for Racer<T> {
    fn default() -> Self {
        Self { seq: 0, state: State::Fresh, queued: Vec::new(), covered: 0, plan: None, start_us: 0, due_start_us: 0, last_scan_end_us: 0, ready: Vec::new(), record: Record::default(), shown_seq: 0, totals: Totals::default() }
    }
}

impl<T> Racer<T> {
    /// The frame the racer follows (`Decoded::race_seq`), 0 before the first.
    #[cfg(any(windows, test))]
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

    /// Whether rows of a frame newer than `seq` are on the screen: a picture of `seq` (0 for a frame
    /// that was not sliced, which says nothing of its age) shown now would step the image back.
    pub fn newer_on_screen(&self, seq: u64) -> bool {
        seq != 0 && seq < self.shown_seq
    }

    /// Lets go of the frame in flight (the slices it holds) and waits for the next. The frame's
    /// older slices stay dropped; the totals stay.
    pub fn reset(&mut self) {
        if matches!(self.state, State::Waiting | State::Scanning | State::Held) { self.totals.abandoned += 1; }
        self.state = State::Fresh;
        self.plan = None;
        self.queued.clear();
        self.covered = 0;
        self.start_us = 0;
        self.due_start_us = 0;
        self.ready.clear();
        self.record = Record::default();
    }

    /// The picture of frame `seq` is on the screen, presented at `issued_us`: closes the frame the
    /// racer follows and returns its record. A picture of any other frame closes nothing.
    pub fn close(&mut self, seq: u64, issued_us: u64, scan: &Scan) -> Option<Record> {
        if seq != self.seq { return None; }
        self.presented(issued_us, scan);
        Some(self.done())
    }

    /// A present of the frame was issued and returned.
    pub fn presented(&mut self, issued_us: u64, scan: &Scan) {
        self.shown_seq = self.shown_seq.max(self.seq);
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
mod tests;
