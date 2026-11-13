//! Where the display's beam is (B10 step 4, docs/beam-racing.md): `D3DKMTGetScanLine` for the
//! monitor the window is on, a calibration of the scan from samples taken at the start of a
//! stream, and the checks that say whether racing the beam can work here at all. Nothing runs
//! unless `beam_race` is on or a VRR stream presents with tearing, where the same checks tell a
//! fixed refresh display, which gets the pacing it would have without VRR (`Streaming::sync_vrr`).
//!
//! The windows calls are in `Scanline`; the calibration, the scan-restart check and the reason
//! mapping take plain samples so they are tested on synthetic ones.

#[cfg(windows)]
use std::time::Duration;

/// One `D3DKMTGetScanLine` reading.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sample {
    /// When it was read (ns on a monotonic clock).
    pub at_ns: u64,
    pub line: u32,
    pub vblank: bool,
}

/// The scan as calibrated. `D3DKMTGetScanLine` counts lines from the top of the refresh, the
/// vertical blank first: lines `0..blank_rows` read `vblank`, the next `rows` lines are the
/// picture's rows, and the counter ends at `total_rows`. A scan takes `scan_ns` in all.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Beam {
    /// Rows of the picture (the display's height).
    pub rows: u32,
    /// Lines of blanking before the first row.
    pub blank_rows: u32,
    /// Lines in a whole refresh at the display's top rate: `blank_rows + rows`.
    pub total_rows: u32,
    /// How long a whole scan takes at the display's top rate, blanking included (ns): the time
    /// from a scan's start to the beam leaving the last row.
    pub scan_ns: u64,
    /// The display's top refresh rate (Hz), `1e9 / scan_ns`.
    pub refresh_hz: f64,
    /// Lines the counter walks per ms.
    pub lines_per_ms: f64,
    /// Share of the readings that were in the vertical blank.
    pub vblank_fraction: f64,
    /// Scans the calibration saw from their blanking to their end.
    pub scans: u32,
    /// The counter reading from which a vblank reading is the display idling after a scan (a
    /// variable refresh display waiting for a present) and not blanking: past the last row by
    /// more than the blanking or 1 ms of lines, and past every line a scan that ran straight
    /// through (a fixed refresh, or a stream at the display's rate) counted in a trailing blanking.
    pub idle_from: u32,
    /// The longest the display idled in the calibration (ns): the lower bound of the panel's own
    /// refresh period, which the restart check keeps out of its tests.
    pub longest_idle_ns: u64,
    /// Scans in the calibration after which the display idled for half a scan or more before the
    /// next began (the counter past one and a half refreshes' lines): it waited for a present.
    pub idle_scans: u32,
}

impl Beam {
    /// The picture row the beam is on `elapsed_ns` after a scan began (the counter at line 0):
    /// negative while the blanking lasts, `rows` and over once the picture is scanned.
    #[cfg_attr(not(test), allow(dead_code))] // only the tests read it; the probe and the race do not
    pub fn row_at(&self, elapsed_ns: u64) -> i64 {
        (elapsed_ns as f64 * self.lines_per_ms / 1e6) as i64 - i64::from(self.blank_rows)
    }

    /// The picture row a reading is on, None in the vertical blank (and past the last row).
    pub fn row_of(&self, sample: &Sample) -> Option<u32> {
        if sample.vblank { return None; }
        let row = sample.line.saturating_sub(self.blank_rows);
        (row < self.rows).then_some(row)
    }

    /// How long the beam of a reading takes to leave the picture's last row (ns), at most one scan:
    /// what a present waits for to show its picture whole (`VrrSync::Scan`). The blanking lines a
    /// scan begins with are part of it: the beam is through them in 0.5 ms at 240 Hz and a tearing
    /// flip takes 0.4 ms and more to arrive, so a present issued there tears the top rows. None
    /// once the beam has left the last row, where a variable refresh display waits for a flip.
    pub fn scan_left_ns(&self, sample: &Sample) -> Option<u64> {
        let end = self.blank_rows + self.rows;
        (sample.line < end).then(|| ((f64::from(end - sample.line) * 1e6 / self.lines_per_ms) as u64).min(self.scan_ns))
    }

    /// How long ago the scan a reading belongs to began (ns): the counter keeps walking while a
    /// variable refresh display idles after it, so this is also how long the display has waited.
    pub fn since_scan_ns(&self, sample: &Sample) -> u64 {
        (f64::from(sample.line) * 1e6 / self.lines_per_ms) as u64
    }

    /// The calibration saw the display wait for presents, on two scans or more (`idle_scans`): a
    /// fixed refresh wraps its counter at every scan's end and never does, and one reading past it
    /// (a mode switch while the calibration ran) is not enough. On a resume a 120 Hz VRR monitor
    /// read a 17.0 ms idle on an 8.31 ms scan.
    pub fn idled(&self) -> bool { self.idle_scans >= 2 }

    /// How long after a scan began the beam reaches picture row `row` (ns).
    #[cfg_attr(not(test), allow(dead_code))] // only the tests read it; the probe and the race do not
    pub fn time_to_row(&self, row: u32) -> u64 {
        ((f64::from(self.blank_rows) + f64::from(row)) * 1e6 / self.lines_per_ms) as u64
    }
}

/// Why the samples did not make a scan.
// The variants are made by `calibrate`, which only the Windows probe and the tests call.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalibrationError {
    /// The readings never show the counter walking through a blanking, so no scan began in the
    /// window (the display idled, or nothing presented).
    NoScan,
    /// The readings do not look like a beam (too few per scan, no steady rate).
    Unsteady,
    /// The counter's top differs from the blanking plus the monitor's height by more than 3%: the
    /// desktop is scaled or the display rotated, so the rows are not the picture's.
    RowsDiffer,
}

#[cfg(any(windows, test))]
/// How far the counter's top may be from the blanking plus the monitor's height.
const ROWS_TOLERANCE: f64 = 0.03;

#[cfg(any(windows, test))]
/// Fewest readings a rising run needs to be fitted.
const MIN_RUN: usize = 5;

#[cfg(any(windows, test))]
/// The runs of `samples` (readings in time order) in which the line only rises: one refresh
/// each, from where the counter started to where it wrapped or held.
fn rising_runs(samples: &[Sample]) -> Vec<&[Sample]> {
    let mut runs = Vec::new();
    let mut from = 0;
    for at in 1..samples.len() {
        if samples[at].line <= samples[at - 1].line {
            runs.push(&samples[from..at]);
            from = at;
        }
    }
    if from < samples.len() { runs.push(&samples[from..]); }
    runs
}

#[cfg(any(windows, test))]
/// Slope of the least squares line through (time, line), in lines per ns.
fn slope(run: &[Sample]) -> Option<f64> {
    let origin = run.first()?.at_ns;
    let n = run.len() as f64;
    let (mut st, mut sl, mut stt, mut stl) = (0.0, 0.0, 0.0, 0.0);
    for s in run {
        let t = (s.at_ns - origin) as f64;
        let l = f64::from(s.line);
        st += t;
        sl += l;
        stt += t * t;
        stl += t * l;
    }
    let denominator = n * stt - st * st;
    if denominator.abs() < f64::EPSILON { return None; }
    let rate = (n * stl - st * sl) / denominator;
    (rate > 0.0).then_some(rate)
}

#[cfg(any(windows, test))]
fn median(values: &mut [f64]) -> f64 {
    values.sort_by(|a, b| a.total_cmp(b));
    let mid = values.len() / 2;
    if values.len() % 2 == 1 { values[mid] } else { (values[mid - 1] + values[mid]) / 2.0 }
}

#[cfg(any(windows, test))]
/// Measures the scan from readings taken every 0.25 ms or so while the stream presented. The
/// rate is the median slope of the refreshes the readings cover (rising runs of at least five
/// readings); the blanking is where the counter leaves `vblank` (the middle of the two readings
/// either side, the median over the refreshes). `picture_rows` is the display's height in
/// pixels; 0 takes the highest line the readings reached.
pub fn calibrate(samples: &[Sample], picture_rows: u32) -> Result<Beam, CalibrationError> {
    let vblank = samples.iter().filter(|s| s.vblank).count();
    let vblank_fraction = if samples.is_empty() { 0.0 } else { vblank as f64 / samples.len() as f64 };
    let runs: Vec<&[Sample]> = rising_runs(samples).into_iter().filter(|run| run.len() >= MIN_RUN).collect();
    let (mut rates, mut edges, mut tops) = (Vec::new(), Vec::new(), Vec::new());
    let mut scanned = Vec::new();
    for run in &runs {
        // A refresh counts when its blanking was read (the counter begins in the blanking).
        let Some(edge) = run.windows(2).find(|w| w[0].vblank && !w[1].vblank) else { continue };
        let Some(rate) = slope(run) else { continue };
        rates.push(rate);
        edges.push((f64::from(edge[0].line) + f64::from(edge[1].line)) / 2.0);
        tops.push(f64::from(run.iter().filter(|s| !s.vblank).map(|s| s.line).max().unwrap_or(0)));
        scanned.push(*run);
    }
    if edges.is_empty() {
        return Err(if runs.is_empty() { CalibrationError::NoScan } else { CalibrationError::Unsteady });
    }
    let rate = median(&mut rates);
    let blank_rows = median(&mut edges).round() as u32;
    // The highest line read is short of the last one by half a reading's walk on average.
    let step = rate * median(&mut samples.windows(2).map(|w| (w[1].at_ns - w[0].at_ns) as f64).collect::<Vec<_>>());
    let top = (median(&mut tops) + step / 2.0).round() as u32 + 1;
    let total_rows = if picture_rows > 0 { blank_rows + picture_rows } else { top };
    let rows = total_rows.saturating_sub(blank_rows);
    if rows == 0 { return Err(CalibrationError::Unsteady); }
    if picture_rows > 0 && (f64::from(top) - f64::from(total_rows)).abs() > f64::from(total_rows) * ROWS_TOLERANCE {
        return Err(CalibrationError::RowsDiffer);
    }
    let scan_ns = (f64::from(total_rows) / rate).round() as u64;
    // Vblank lines past the picture on scans that ran straight into the next one are a trailing
    // blanking (a front porch), not idling; a scan that ran on for half as long again as one is idling.
    let straight = scanned.iter()
        .filter(|run| (run[run.len() - 1].at_ns - run[0].at_ns) as f64 <= scan_ns as f64 * 1.5)
        .flat_map(|run| run.iter().filter(|s| s.vblank && s.line >= blank_rows).map(|s| s.line))
        .max().unwrap_or(0);
    let margin = blank_rows.max((rate * 1e6) as u32);
    let idle_from = (total_rows + margin).max(straight + 1);
    let longest_idle_ns = (f64::from(samples.iter().filter(|s| s.vblank).map(|s| s.line).max().unwrap_or(0)) / rate) as u64;
    let idle_line = f64::from(total_rows) * 1.5;
    let idle_scans = rising_runs(samples).iter()
        .filter(|run| run.iter().any(|s| s.vblank && f64::from(s.line) >= idle_line))
        .count() as u32;
    Ok(Beam {
        rows, blank_rows, total_rows, scan_ns,
        refresh_hz: 1e9 / scan_ns as f64,
        lines_per_ms: rate * 1e6,
        vblank_fraction,
        scans: edges.len() as u32,
        idle_from,
        longest_idle_ns,
        idle_scans,
    })
}

#[cfg(any(windows, test))]
/// What reopening the scan line on the window's monitor found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Follow {
    Same,
    /// Reopened on the monitor the window moved to.
    Moved,
    /// The window moved and the new monitor's scan line cannot be opened.
    Lost,
}

#[cfg(any(windows, test))]
/// What the scan-restart check saw after one present.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Restart {
    /// The display was not idling at the present, or the new scan could be the panel's own
    /// refresh: the check says nothing.
    NotTested,
    /// A new scan began within the window after the present.
    Began,
    /// No new scan began in time.
    Missed,
}

/// How long after a present a new scan has to begin (ns). The flip waits for the GPU work queued
/// ahead of it, so the scan begins anywhere from 0.2 ms to the time that work takes. Measured on a
/// 240 Hz VRR display with direct scanout, a PyroWave GPU decode of 2560x1440 at 60 fps and the
/// host encoding on the same GPU: 60 presents spread evenly over 0.2-8.8 ms (median 5 ms); on an
/// idler GPU the median was 0.95 ms and 92% were under 2 ms. 10 ms is the longest seen plus 1 ms.
pub const RESTART_WINDOW_NS: u64 = 10_000_000;

#[cfg(any(windows, test))]
/// How close to the panel's own refresh period a new scan may start (ns) before it could be that
/// refresh and not the present's.
const FORCED_TOLERANCE_NS: i64 = 1_000_000;

#[cfg(any(windows, test))]
/// The check for one present: `before` was read just before the present call, `after` are the
/// readings in the window that followed it (which starts when the call returned, so a slow call
/// does not eat it). A present while the display idles after a scan (the counter at or past
/// `beam.idle_from`, which only a variable refresh display does: a fixed one wraps to line 0)
/// starts a new scan when the present is scanned out directly, so the first reading that is not in
/// the blanking must be within the first 5% of the picture's rows. A present in the blanking of a
/// scan already under way proves nothing and is not tested. Neither is one whose new scan starts a
/// whole `longest_idle_ns` (the longest the display idled so far: its minimum refresh rate, if it
/// ever idled that long) after the last one began, within a millisecond: a VRR panel refreshes by
/// itself at its floor, and that scan is not the present's.
pub fn restart(before: &Sample, after: &[Sample], beam: &Beam, longest_idle_ns: u64) -> Restart {
    if !before.vblank || before.line < beam.idle_from {
        return Restart::NotTested;
    }
    let Some(first) = after.iter().find(|s| !s.vblank) else { return Restart::Missed };
    if u64::from(first.line.saturating_sub(beam.blank_rows)) * 100 >= u64::from(beam.rows) * 5 {
        return Restart::Missed;
    }
    // The scans' starts, counted back from their readings at the counter's rate.
    let started = |s: &Sample| s.at_ns as i64 - (f64::from(s.line) * 1e6 / beam.lines_per_ms) as i64;
    let gap = started(first) - started(before);
    if longest_idle_ns > 0 && (gap - longest_idle_ns as i64).abs() <= FORCED_TOLERANCE_NS {
        return Restart::NotTested;
    }
    Restart::Began
}

/// The presents the restart check tests, and the misses that fail it. A present is tested when
/// the display was idling in its blanking, so a stream that never lets the display idle is
/// given up on after `CHECK_FRAMES` presents.
pub const CHECK_TESTED: u32 = 30;
/// A present whose scan starts late is normal (see the window), so a third of the tested presents
/// have to miss before the check fails; a present that does not restart the scan misses most of them.
pub const CHECK_MISSES: u32 = 9;
pub const CHECK_FRAMES: u32 = 300;
/// Presents after a calibration that looks like a fixed refresh, none of them starting a scan,
/// that tell a fixed refresh without the rest of the check: a second at 60 fps. The VRR panel
/// tested 30 of its first 84 presents, so one that idles starts a scan well within them.
pub const FIXED_FRAMES: u32 = 60;

/// How far the presents a second may fall below the highest the check saw for `FIXED_FRAMES` to
/// tell a fixed refresh: a rate still settling (a resume) says too little, and the whole check
/// decides. A steady rate below the stream's (a 60 fps game, a still desktop) is fine.
pub const STEADY: f64 = 0.8;

/// Whether the stream presents about as often as the display scans (70% of its top rate or more),
/// so a VRR display never idles between its scans either: its presents tell a fixed refresh
/// from a variable one no better than a still screen does.
pub fn busy(content_hz: f64, display_hz: f64) -> bool { content_hz >= display_hz * 0.7 }

/// The restart check over the first presents of a stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RestartCheck {
    pub frames: u32,
    pub tested: u32,
    pub missed: u32,
}

impl RestartCheck {
    #[cfg(any(windows, test))]
    pub fn push(&mut self, result: Restart) {
        if !self.running() { return; }
        self.frames += 1;
        match result {
            Restart::NotTested => {}
            Restart::Began => self.tested += 1,
            Restart::Missed => { self.tested += 1; self.missed += 1; }
        }
    }

    /// Whether the check is still looking at presents.
    pub fn running(&self) -> bool { !self.failed() && !self.passed() && !self.starved() }

    pub fn failed(&self) -> bool { self.missed >= CHECK_MISSES }

    pub fn passed(&self) -> bool { self.tested >= CHECK_TESTED && !self.failed() }

    /// Many presents and the display never idled for one: it scans all the time, a fixed refresh.
    pub fn starved(&self) -> bool { self.frames >= CHECK_FRAMES && self.tested < 3 }
}

/// What the stream's display and swap chain allow, and why not when they do not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RaceCapability {
    pub tearing: bool,
    pub vrr: bool,
    /// Inferred: the scan restarts on our presents, which direct scanout does; the OS was not asked.
    pub flip_inferred: bool,
    pub scanline: bool,
}

impl RaceCapability {
    pub fn on(&self) -> bool { self.tearing && self.vrr && self.flip_inferred && self.scanline }
}

/// Everything the capability is decided from.
#[derive(Clone, Copy, Debug)]
pub struct Evidence {
    /// The swap chain presents with tearing and the pacing mode does (not V-Sync).
    pub tearing: bool,
    /// `D3DKMTGetScanLine` could be opened and read.
    pub scanline: bool,
    /// The window covers its monitor as a fullscreen window.
    pub fullscreen: bool,
    /// The calibration, when it ran.
    pub beam: Option<Result<Beam, CalibrationError>>,
    pub check: RestartCheck,
    /// The stream's frame rate and the display's top rate, for telling a fixed refresh from a
    /// variable one.
    pub content_hz: f64,
    pub display_hz: f64,
    /// The lowest and highest presents a second measured while the check took its presents, None
    /// before a second was measured.
    pub check_rates: Option<(f64, f64)>,
    /// The highest rate the stream presented at while the check took its presents, 0 before it
    /// took any: a check is judged at the rate it ran at, not at a later one.
    pub check_hz: f64,
    /// The rate of the last presents, 0 before there were enough. Too short to judge the display
    /// by; at a busy rate it holds the quick rule back until a whole second has said.
    pub recent_hz: f64,
}

/// The first thing that rules racing out, as the word the overlay shows after "off".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    NoTearing,
    NoScanline,
    NoScan,
    NoVrr,
    /// A present issued while the display idled did not start a scan: what was measured, which
    /// says VRR is not active for this window. The probe has no signal for composition itself.
    NoRestart,
    Windowed,
    /// The stream presents about as often as the display scans: it never idles to be tested.
    NoIdle,
    /// The scan's rows do not match the monitor's height.
    RowsDiffer,
    /// No scan line to race on, but the display runs VRR: Linux reads it from DRM (`drm_vrr`).
    #[cfg_attr(windows, allow(dead_code))]
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    VrrNoScanline,
}

impl Reason {
    pub fn label(self) -> &'static str {
        match self {
            Self::NoTearing => "no tearing",
            Self::NoScanline => "no scanline",
            Self::NoScan => "no scan seen",
            Self::NoVrr => "no VRR",
            Self::NoRestart => "no VRR scan restart",
            Self::Windowed => "not fullscreen",
            Self::NoIdle => "no idle at this frame rate",
            Self::RowsDiffer => "scan rows differ from the display's",
            Self::VrrNoScanline => "no scanline, VRR on",
        }
    }
}

/// What is known so far: still measuring, or the capability with the reason if it is off.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Verdict {
    Checking,
    On(Beam, RaceCapability),
    Off(Reason, RaceCapability),
}

impl Verdict {
    /// The overlay's "Beam race" value.
    pub fn line(&self) -> String {
        match self {
            Self::Checking => "checking the display".into(),
            Self::On(beam, _) => format!("on, scan {:.2} ms over {} rows", beam.scan_ns as f64 / 1e6, beam.rows),
            Self::Off(reason, _) => format!("off ({})", reason.label()),
        }
    }

    /// The display scans at its own rate whatever is presented ("no VRR"): a tearing present tears.
    pub fn fixed_refresh(&self) -> bool { matches!(self, Self::Off(Reason::NoVrr, _)) }

    /// Presents restarted the display's scan (the check passed), or DRM says VRR is on (Linux). The
    /// only verdicts a stream turned to V-Sync by a "no VRR" one returns from; an idle seen in the
    /// calibration alone is not enough.
    pub fn refreshes_on_present(&self) -> bool {
        matches!(self, Self::On(_, capability) | Self::Off(Reason::VrrNoScanline, capability) if capability.vrr)
    }

    /// The timing CSV's `race_reason`: "on" or the reason, empty while it is not known.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Checking => "",
            Self::On(..) => "on",
            Self::Off(reason, _) => reason.label(),
        }
    }
}

/// Decides from the evidence. Order of the reasons is the order of the design's table: tearing,
/// scanline, then the window, then what the restart check says (no restart or not variable).
pub fn decide(e: &Evidence) -> Verdict {
    let mut capability = RaceCapability { tearing: e.tearing, vrr: false, flip_inferred: false, scanline: e.scanline };
    if !e.tearing { return Verdict::Off(Reason::NoTearing, capability); }
    if !e.scanline { return Verdict::Off(Reason::NoScanline, capability); }
    let beam = match e.beam {
        None => return Verdict::Checking,
        Some(Err(CalibrationError::RowsDiffer)) => return Verdict::Off(Reason::RowsDiffer, capability),
        Some(Err(_)) => return Verdict::Off(Reason::NoScan, capability),
        Some(Ok(beam)) => beam,
    };
    if !e.fullscreen {
        // A window that does not cover its display is not raced; the scan is not checked for it.
        return Verdict::Off(Reason::Windowed, capability);
    }
    // A fixed refresh scans all the time whatever is presented: the blanking is a small share of
    // the readings although the stream runs well below the display's top rate (1% on a 120 Hz
    // laptop panel at 60 fps, 65-76% on a 240 Hz VRR panel). One calibration in eight on the VRR
    // panel read 12%, taken while it still scanned at its top rate, so the share alone decides
    // nothing: with `FIXED_FRAMES` presents after it and none of them starting a scan, the check
    // is not waited for. Nor is it after a calibration that saw the display wait for presents
    // (`idled`), or while the rate still settles (`STEADY`): on a resume the quick rule called a
    // 120 Hz VRR monitor "no VRR" from 66 presents a second. Nor while the last presents ran at a
    // busy rate no whole second has measured yet (`recent_hz`): the same monitor's check took its
    // 60 presents at 120 a second, judged by a first second of 52-77. Tested presents that missed
    // still do.
    let check_hz = if e.check_hz > 0.0 { e.check_hz } else { e.content_hz };
    let idled = beam.idled();
    let fixed = check_hz > 0.0 && e.display_hz > 0.0 && !busy(check_hz, e.display_hz) && beam.vblank_fraction < 0.3;
    let steady = e.check_rates.is_none_or(|(low, high)| low >= high * STEADY);
    let rising = busy(e.recent_hz, e.display_hz);
    if fixed && !idled && steady && !rising && e.check.running() && e.check.frames >= FIXED_FRAMES && e.check.tested == e.check.missed {
        return Verdict::Off(Reason::NoVrr, capability);
    }
    if e.check.failed() {
        capability.vrr = !fixed;
        return Verdict::Off(if fixed { Reason::NoVrr } else { Reason::NoRestart }, capability);
    }
    if e.check.starved() {
        // Enough presents went by without the display ever idling after a scan. At a rate near
        // the display's own that is the stream's doing; below it, the display scans all the time.
        // The rate is the one it starved at: a check that starved at a busy rate is measured again
        // when the stream slows well below it (the probe's `apply_surroundings`). A display the
        // calibration saw idle does wait for presents; these just never came while it idled.
        let busy = busy(check_hz, e.display_hz);
        return Verdict::Off(if busy || idled { Reason::NoIdle } else { Reason::NoVrr }, capability);
    }
    capability.vrr = true;
    capability.flip_inferred = true;
    if e.check.passed() { Verdict::On(beam, capability) } else { Verdict::Checking }
}

#[cfg(windows)]
pub use win::Scanline;

#[cfg(windows)]
mod win {
    use super::{Follow, Sample};
    use std::time::Instant;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, HMONITOR, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST};
    use windows::Wdk::Graphics::Direct3D::{
        D3DKMTCloseAdapter, D3DKMTGetScanLine, D3DKMTOpenAdapterFromGdiDisplayName, D3DKMT_CLOSEADAPTER, D3DKMT_GETSCANLINE,
        D3DKMT_OPENADAPTERFROMGDIDISPLAYNAME,
    };

    /// `D3DKMTGetScanLine` on the monitor a window is on. Closes the adapter on drop.
    pub struct Scanline {
        adapter: u32,
        source: u32,
        monitor: HMONITOR,
        window: HWND,
        device: [u16; 32],
        rows: u32,
        clock: Instant,
    }

    impl Scanline {
        /// Opens the monitor `window` is on; None when the monitor, the adapter or the call is not
        /// there (a remote session, a driver without the scanline call).
        pub fn open(window: HWND, clock: Instant) -> Option<Self> {
            let (monitor, device, rows) = monitor_of(window)?;
            let (adapter, source) = open_adapter(&device)?;
            let scan = Self { adapter, source, monitor, window, device, rows, clock };
            // A driver that opens the adapter but refuses the call is as good as none.
            scan.sample()?;
            Some(scan)
        }

        pub fn sample(&self) -> Option<Sample> {
            if self.adapter == 0 { return None; }
            let mut query = D3DKMT_GETSCANLINE { hAdapter: self.adapter, VidPnSourceId: self.source, ..Default::default() };
            // SAFETY: a valid struct for the call; the adapter handle is open.
            let status = unsafe { D3DKMTGetScanLine(&mut query) };
            let at_ns = self.clock.elapsed().as_nanos() as u64;
            status.is_ok().then_some(Sample { at_ns, line: query.ScanLine, vblank: query.InVerticalBlank })
        }

        /// Reopens on the new monitor when the window moved to another one. When the new monitor
        /// cannot be opened the old handle is closed (it reads another display's scan) and every
        /// reading fails from then on.
        pub fn follow_window(&mut self) -> Follow {
            let Some((monitor, device, rows)) = monitor_of(self.window) else { return Follow::Same };
            if monitor == self.monitor { return Follow::Same; }
            self.close();
            let Some((adapter, source)) = open_adapter(&device) else {
                self.monitor = monitor;
                return Follow::Lost;
            };
            self.adapter = adapter;
            self.source = source;
            self.monitor = monitor;
            self.device = device;
            self.rows = rows;
            Follow::Moved
        }

        /// The GDI name of the monitor, for logs.
        pub fn device_name(&self) -> String {
            let end = self.device.iter().position(|c| *c == 0).unwrap_or(self.device.len());
            String::from_utf16_lossy(&self.device[..end])
        }

        /// The monitor's height in pixels: the rows the beam walks.
        pub fn rows(&self) -> u32 { self.rows }

        pub fn clock(&self) -> Instant { self.clock }

        fn close(&mut self) {
            if self.adapter != 0 {
                let close = D3DKMT_CLOSEADAPTER { hAdapter: self.adapter };
                // SAFETY: the handle came from the open call and is closed once.
                let _ = unsafe { D3DKMTCloseAdapter(&close) };
                self.adapter = 0;
            }
        }
    }

    impl Drop for Scanline {
        fn drop(&mut self) { self.close(); }
    }

    fn monitor_of(window: HWND) -> Option<(HMONITOR, [u16; 32], u32)> {
        // SAFETY: plain queries on a window handle of this process.
        let monitor = unsafe { MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST) };
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        // SAFETY: cbSize says which struct this is.
        if !unsafe { GetMonitorInfoW(monitor, (&mut info as *mut MONITORINFOEXW).cast()) }.as_bool() { return None; }
        let rect = info.monitorInfo.rcMonitor;
        Some((monitor, info.szDevice, (rect.bottom - rect.top).max(0) as u32))
    }

    fn open_adapter(device: &[u16; 32]) -> Option<(u32, u32)> {
        let mut open = D3DKMT_OPENADAPTERFROMGDIDISPLAYNAME { DeviceName: *device, ..Default::default() };
        // SAFETY: a valid struct for the call.
        let status = unsafe { D3DKMTOpenAdapterFromGdiDisplayName(&mut open) };
        status.is_ok().then_some((open.hAdapter, open.VidPnSourceId))
    }
}

/// Readings every 0.25 ms for `window`, from `scan` (spinning: a sleep cannot be this short).
#[cfg(windows)]
pub fn collect(scan: &Scanline, window: Duration, every: Duration) -> Vec<Sample> {
    let clock = scan.clock();
    let end = clock.elapsed() + window;
    let mut samples = Vec::with_capacity((window.as_nanos() / every.as_nanos().max(1)) as usize + 1);
    let mut next = clock.elapsed();
    while clock.elapsed() < end {
        if let Some(sample) = scan.sample() { samples.push(sample); }
        next += every;
        while clock.elapsed() < next { std::hint::spin_loop(); }
    }
    samples
}

#[cfg(test)]
mod tests;
