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
}

impl Beam {
    /// The picture row the beam is on `elapsed_ns` after a scan began (the counter at line 0):
    /// negative while the blanking lasts, `rows` and over once the picture is scanned.
    #[allow(dead_code)] // the race itself (B10 step 4, brief 3) reads the beam; the probe and the tests do for now
    pub fn row_at(&self, elapsed_ns: u64) -> i64 {
        (elapsed_ns as f64 * self.lines_per_ms / 1e6) as i64 - i64::from(self.blank_rows)
    }

    /// The picture row a reading is on, None in the vertical blank (and past the last row).
    #[allow(dead_code)] // the race itself (B10 step 4, brief 3) reads the beam; the probe and the tests do for now
    pub fn row_of(&self, sample: &Sample) -> Option<u32> {
        if sample.vblank { return None; }
        let row = sample.line.saturating_sub(self.blank_rows);
        (row < self.rows).then_some(row)
    }

    /// How long the beam of a reading takes to leave the picture's last row (ns), at most one scan:
    /// what a present waits for to show its picture whole (`VrrSync::Scan`). None in the blanking,
    /// where a flip comes before the first row.
    pub fn scan_left_ns(&self, sample: &Sample) -> Option<u64> {
        let row = self.row_of(sample)?;
        Some((((f64::from(self.rows - row)) * 1e6 / self.lines_per_ms) as u64).min(self.scan_ns))
    }

    /// How long after a scan began the beam reaches picture row `row` (ns).
    #[allow(dead_code)] // the race itself (B10 step 4, brief 3) reads the beam; the probe and the tests do for now
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
    Ok(Beam {
        rows, blank_rows, total_rows, scan_ns,
        refresh_hz: 1e9 / scan_ns as f64,
        lines_per_ms: rate * 1e6,
        vblank_fraction,
        scans: edges.len() as u32,
        idle_from,
        longest_idle_ns,
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
    /// The highest rate the stream presented at while the check took its presents, 0 before it
    /// took any: a check is judged at the rate it ran at, not at a later one.
    pub check_hz: f64,
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
    // is not waited for.
    let check_hz = if e.check_hz > 0.0 { e.check_hz } else { e.content_hz };
    let fixed = check_hz > 0.0 && e.display_hz > 0.0 && !busy(check_hz, e.display_hz) && beam.vblank_fraction < 0.3;
    if fixed && e.check.running() && e.check.frames >= FIXED_FRAMES && e.check.tested == e.check.missed {
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
        // when the stream slows well below it (the probe's `apply_surroundings`).
        let busy = busy(check_hz, e.display_hz);
        return Verdict::Off(if busy { Reason::NoIdle } else { Reason::NoVrr }, capability);
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
mod tests {
    use super::*;

    /// A synthetic display like the one measured on the RX 7900 XT: the counter runs through
    /// `blank` lines of blanking (`vblank` set) and then `rows` picture lines at `lines_per_ms`,
    /// from each start in `starts`; between scans it idles at 0 in the blanking (a variable
    /// refresh display waiting for a present). Read every `every_ns` until `until`.
    fn display(starts: &[u64], blank: u32, rows: u32, lines_per_ms: f64, every_ns: u64, until: u64) -> Vec<Sample> {
        let total = f64::from(blank + rows);
        let scan_ns = (total * 1e6 / lines_per_ms) as u64;
        let mut samples = Vec::new();
        let mut t = 0;
        while t < until {
            let scan = starts.iter().rev().find(|s| **s <= t).filter(|s| t - **s < scan_ns);
            samples.push(match scan {
                Some(s) => {
                    let line = ((t - s) as f64 * lines_per_ms / 1e6) as u32;
                    Sample { at_ns: t, line, vblank: line < blank }
                }
                None => Sample { at_ns: t, line: 0, vblank: true },
            });
            t += every_ns;
        }
        samples
    }

    /// The measured display: 1440 rows behind 202 lines of blanking, 394 lines per ms (4.17 ms).
    const BLANK: u32 = 202;
    const RATE: f64 = 394.0;

    #[test]
    fn a_240_hz_display_with_a_60_hz_stream_calibrates_to_its_scan() {
        // A scan every 16.667 ms, as a 60 fps stream on a variable refresh display makes.
        let starts: Vec<u64> = (0..60).map(|i| 3_000_000 + i * 16_667_000).collect();
        let samples = display(&starts, BLANK, 1440, RATE, 250_000, 1_000_000_000);
        let beam = calibrate(&samples, 1440).unwrap();
        assert_eq!(beam.rows, 1440);
        assert!((i64::from(beam.blank_rows) - i64::from(BLANK)).abs() <= 45, "{}", beam.blank_rows);
        assert!((beam.scan_ns as f64 - 4_170_000.0).abs() < 60_000.0, "S is the whole scan, blanking included: {}", beam.scan_ns);
        assert!((beam.refresh_hz - 240.0).abs() < 4.0, "{}", beam.refresh_hz);
        assert!((beam.lines_per_ms - RATE).abs() < 3.0, "{}", beam.lines_per_ms);
        assert!(beam.scans >= 50);
        assert!(beam.vblank_fraction > 0.7, "the display idles between scans: {}", beam.vblank_fraction);
    }

    #[test]
    fn a_fixed_refresh_display_scanning_all_the_time_calibrates_too() {
        let starts: Vec<u64> = (0..120).map(|i| 1_000_000 + i * 4_167_000).collect();
        let samples = display(&starts, BLANK, 1440, RATE, 250_000, 450_000_000);
        let beam = calibrate(&samples, 1440).unwrap();
        assert!((beam.scan_ns as f64 - 4_170_000.0).abs() < 60_000.0, "{}", beam.scan_ns);
        assert!(beam.vblank_fraction < 0.3, "{}", beam.vblank_fraction);
    }

    #[test]
    fn the_rows_come_from_the_readings_when_the_height_is_not_given() {
        let starts: Vec<u64> = (0..40).map(|i| 1_000_000 + i * 16_667_000).collect();
        let samples = display(&starts, 150, 1080, 394.0, 100_000, 600_000_000);
        let beam = calibrate(&samples, 0).unwrap();
        assert!((1070..=1090).contains(&beam.rows), "{}", beam.rows);
        assert_eq!(beam.total_rows, beam.blank_rows + beam.rows);
    }

    #[test]
    fn a_display_that_never_scans_in_the_window_says_so() {
        let samples = display(&[], BLANK, 1440, RATE, 250_000, 1_000_000_000);
        assert_eq!(calibrate(&samples, 1440), Err(CalibrationError::NoScan));
        assert_eq!(calibrate(&[], 1440), Err(CalibrationError::NoScan));
    }

    #[test]
    fn a_scan_cut_by_the_window_or_too_short_to_fit_is_not_used() {
        // Readings 1.5 ms apart see 3 per scan: too few to fit.
        let coarse = display(&[1_000_000, 20_000_000], BLANK, 1440, RATE, 1_500_000, 40_000_000);
        assert!(calibrate(&coarse, 1440).is_err());
        // A rising run whose blanking was not read is not a refresh to measure.
        let mid: Vec<Sample> = (0..20).map(|i| Sample { at_ns: i * 250_000, line: 300 + i as u32 * 60, vblank: false }).collect();
        assert!(calibrate(&mid, 1440).is_err());
    }

    fn measured() -> Beam {
        Beam { rows: 1440, blank_rows: 200, total_rows: 1640, scan_ns: 4_100_000, refresh_hz: 243.9, lines_per_ms: 400.0, vblank_fraction: 0.75, scans: 50, idle_from: 2040, longest_idle_ns: 0 }
    }

    #[test]
    fn the_beam_row_follows_the_scan() {
        let beam = measured();
        assert_eq!(beam.row_at(0), -200, "a scan begins in the blanking");
        assert_eq!(beam.row_at(500_000), 0, "400 lines per ms: 200 lines of blanking take 0.5 ms");
        assert_eq!(beam.row_at(2_300_000), 720);
        assert_eq!(beam.row_at(4_100_000), 1440, "the last row");
        assert_eq!(beam.time_to_row(0), 500_000);
        assert_eq!(beam.time_to_row(720), 2_300_000);
        assert_eq!(beam.row_of(&Sample { at_ns: 0, line: 900, vblank: false }), Some(700));
        assert_eq!(beam.row_of(&Sample { at_ns: 0, line: 100, vblank: true }), None);
        assert_eq!(beam.row_of(&Sample { at_ns: 0, line: 5000, vblank: false }), None, "past the last row");
    }

    #[test]
    fn a_present_mid_scan_waits_for_the_beam_to_leave_the_last_row() {
        let beam = measured();
        // 400 lines per ms: row 700 has 740 rows to go, 1.85 ms.
        assert_eq!(beam.scan_left_ns(&Sample { at_ns: 0, line: 900, vblank: false }), Some(1_850_000));
        assert_eq!(beam.scan_left_ns(&Sample { at_ns: 0, line: 200, vblank: false }), Some(3_600_000), "the first row: the whole picture to go");
        assert_eq!(beam.scan_left_ns(&Sample { at_ns: 0, line: 100, vblank: true }), None, "in the blanking a flip comes before the first row");
        assert_eq!(beam.scan_left_ns(&Sample { at_ns: 0, line: 5000, vblank: false }), None, "past the last row");
        let short = Beam { scan_ns: 2_000_000, ..beam };
        assert_eq!(short.scan_left_ns(&Sample { at_ns: 0, line: 200, vblank: false }), Some(2_000_000), "never more than a scan");
    }

    fn at(ns: u64, line: u32, vblank: bool) -> Sample { Sample { at_ns: ns, line, vblank } }

    #[test]
    fn a_present_in_the_blanking_that_restarts_the_scan_passes() {
        let before = at(0, 2400, true);
        // The first reading out of the blanking is on row 3 (line 203).
        let after = [at(10_000, 0, true), at(20_000, 130, true), at(30_000, 203, false), at(40_000, 215, false)];
        assert_eq!(restart(&before, &after, &measured(), 0), Restart::Began);
    }

    #[test]
    fn a_present_whose_scan_starts_late_or_far_down_misses() {
        let before = at(0, 2400, true);
        let beam = measured();
        // Nothing within the window.
        assert_eq!(restart(&before, &[at(100_000, 0, true), at(400_000, 0, true)], &beam, 0), Restart::Missed);
        assert_eq!(restart(&before, &[], &beam, 0), Restart::Missed);
        // A scan already well down when first seen: it began before the present, so it is not the present's.
        assert_eq!(restart(&before, &[at(1_000, 600, false)], &beam, 0), Restart::Missed);
    }

    #[test]
    fn the_window_covers_the_slowest_flip_measured_with_a_millisecond_to_spare() {
        // 60 presents on the 240 Hz VRR display with the host encoding on the same GPU began their
        // scans 0.2-8.8 ms after the present call (p99 8.79 ms).
        const { assert!(RESTART_WINDOW_NS >= 8_790_000 + 1_000_000) };
        let beam = measured();
        let late = [at(8_700_000, 2400, true), at(8_790_000, 203, false)];
        assert_eq!(restart(&at(0, 2400, true), &late, &beam, 0), Restart::Began);
    }

    #[test]
    fn a_present_in_the_middle_of_a_scan_or_in_its_blanking_says_nothing() {
        // The blanking of a scan under way (a fixed refresh has nothing else) is not the display idling.
        assert_eq!(restart(&at(0, 50, true), &[at(1_000, 203, false)], &measured(), 0), Restart::NotTested);
        assert_eq!(restart(&at(0, 900, false), &[at(1_000, 910, false)], &measured(), 0), Restart::NotTested);
    }

    #[test]
    fn a_third_of_the_tested_presents_missing_fails_the_check() {
        let mut check = RestartCheck::default();
        for _ in 0..10 { check.push(Restart::Began); }
        for _ in 0..CHECK_MISSES - 1 { check.push(Restart::Missed); }
        assert!(!check.failed() && check.running());
        check.push(Restart::Missed);
        assert!(check.failed() && !check.running() && !check.passed());
        // Later results do not change a settled check.
        let mut done = RestartCheck::default();
        for _ in 0..CHECK_TESTED { done.push(Restart::Began); }
        assert!(done.passed() && !done.running());
        for _ in 0..5 { done.push(Restart::Missed); }
        assert!(done.passed() && !done.failed(), "presents after the check do not count");
    }

    #[test]
    fn presents_in_a_scan_are_not_tests_and_cannot_pass_alone() {
        let mut check = RestartCheck::default();
        for _ in 0..CHECK_FRAMES - 1 { check.push(Restart::NotTested); }
        assert!(check.running() && !check.passed(), "no present was tested, so nothing is proven yet");
        check.push(Restart::NotTested);
        assert!(check.starved() && !check.running(), "a display that never idles is a fixed refresh");
    }

    fn beam(vblank_fraction: f64) -> Beam {
        Beam { vblank_fraction, ..measured() }
    }

    fn evidence() -> Evidence {
        let mut check = RestartCheck::default();
        for _ in 0..CHECK_TESTED { check.push(Restart::Began); }
        Evidence { tearing: true, scanline: true, fullscreen: true, beam: Some(Ok(beam(0.75))), check, content_hz: 60.0, display_hz: 240.0, check_hz: 0.0 }
    }

    fn missed() -> RestartCheck {
        let mut check = RestartCheck::default();
        for _ in 0..CHECK_MISSES { check.push(Restart::Missed); }
        check
    }

    #[test]
    fn everything_in_order_is_on() {
        let verdict = decide(&evidence());
        assert!(matches!(verdict, Verdict::On(_, c) if c.on()), "{verdict:?}");
        assert_eq!(verdict.code(), "on");
        assert!(verdict.line().starts_with("on, scan 4.10 ms over 1440 rows"), "{}", verdict.line());
    }

    #[test]
    fn each_reason_names_what_is_missing() {
        let line = |e: Evidence| decide(&e).line();
        assert_eq!(line(Evidence { tearing: false, ..evidence() }), "off (no tearing)", "V-Sync presents never tear");
        assert_eq!(line(Evidence { scanline: false, ..evidence() }), "off (no scanline)");
        assert_eq!(line(Evidence { beam: Some(Err(CalibrationError::NoScan)), ..evidence() }), "off (no scan seen)");
        assert_eq!(line(Evidence { check: missed(), ..evidence() }), "off (no VRR scan restart)", "a borderless window whose presents do not restart the scan");
        assert_eq!(line(Evidence { check: missed(), fullscreen: false, ..evidence() }), "off (not fullscreen)", "windowed");
        // A window is not raced whatever the check saw, and before the check has finished.
        assert_eq!(line(Evidence { fullscreen: false, ..evidence() }), "off (not fullscreen)");
        assert_eq!(line(Evidence { fullscreen: false, check: RestartCheck::default(), ..evidence() }), "off (not fullscreen)");
        assert_eq!(line(Evidence { fullscreen: false, beam: Some(Ok(beam(0.13))), ..evidence() }), "off (not fullscreen)", "a windowed fixed refresh");
        // A fixed refresh scans all the time: little blanking at a stream rate far below the top rate.
        assert_eq!(line(Evidence { check: missed(), beam: Some(Ok(beam(0.05))), ..evidence() }), "off (no VRR)");
        // At the display's own rate the two cannot be told apart, so a miss is only a missing restart.
        assert_eq!(line(Evidence { check: missed(), beam: Some(Ok(beam(0.05))), content_hz: 240.0, ..evidence() }), "off (no VRR scan restart)");
        let mut starved = RestartCheck::default();
        for _ in 0..CHECK_FRAMES { starved.push(Restart::NotTested); }
        assert_eq!(line(Evidence { check: starved, ..evidence() }), "off (no VRR)", "the display never idled");
    }

    #[test]
    fn tearing_and_scanline_come_before_the_measurements() {
        // The reason does not wait for a calibration that has not finished.
        assert_eq!(decide(&Evidence { tearing: false, beam: None, ..evidence() }).line(), "off (no tearing)");
        assert_eq!(decide(&Evidence { scanline: false, beam: None, ..evidence() }).line(), "off (no scanline)");
        assert_eq!(decide(&Evidence { beam: None, ..evidence() }).line(), "checking the display");
        assert_eq!(decide(&Evidence { beam: None, ..evidence() }).code(), "");
    }

    #[test]
    fn the_check_unfinished_is_checking_not_on() {
        let mut check = RestartCheck::default();
        for _ in 0..5 { check.push(Restart::Began); }
        assert_eq!(decide(&Evidence { check, ..evidence() }), Verdict::Checking);
    }

    /// A display whose counter never wraps by itself: it counts from each start in `starts` and
    /// goes on, the blanking (`vblank`) before the first row and after the last, until the next
    /// start. A long gap is a variable refresh display idling; a short one, a trailing blanking
    /// (front porch) on scans that run straight into each other.
    fn counting(starts: &[u64], blank: u32, rows: u32, lines_per_ms: f64, every_ns: u64, until: u64) -> Vec<Sample> {
        let mut samples = Vec::new();
        let mut t = 0;
        while t < until {
            samples.push(match starts.iter().rev().find(|s| **s <= t) {
                Some(s) => {
                    let line = ((t - s) as f64 * lines_per_ms / 1e6) as u32;
                    Sample { at_ns: t, line, vblank: line < blank || line >= blank + rows }
                }
                None => Sample { at_ns: t, line: 0, vblank: true },
            });
            t += every_ns;
        }
        samples
    }

    #[test]
    fn idling_is_counted_from_past_the_blanking_and_the_longest_idle_is_kept() {
        let starts: Vec<u64> = (0..60).map(|i| 3_000_000 + i * 16_667_000).collect();
        let beam = calibrate(&counting(&starts, BLANK, 1440, RATE, 250_000, 1_000_000_000), 1440).unwrap();
        assert!(beam.idle_from >= beam.total_rows + beam.blank_rows, "{beam:?}");
        assert!(beam.idle_from >= beam.total_rows + RATE as u32 - 5, "at least 1 ms of lines past the last row: {beam:?}");
        assert!((15_500_000..=17_000_000).contains(&beam.longest_idle_ns), "a scan every 16.7 ms: {}", beam.longest_idle_ns);
    }

    #[test]
    fn a_trailing_blanking_longer_than_the_margin_is_not_idling() {
        // Scans 5.7 ms apart: 600 lines (1.5 ms) of blanking after the last row, more than the 1 ms
        // or the blanking the margin allows. A reading in it must not count as the display idling.
        let starts: Vec<u64> = (0..150).map(|i| 1_000_000 + i * 5_700_000).collect();
        let beam = calibrate(&counting(&starts, BLANK, 1440, RATE, 250_000, 800_000_000), 1440).unwrap();
        assert!(beam.idle_from > beam.total_rows + 540, "past the porch's lines: {beam:?}");
        let in_porch = Sample { at_ns: 0, line: beam.total_rows + 500, vblank: true };
        let late = [Sample { at_ns: 1_000_000, line: BLANK + 3, vblank: false }];
        assert_eq!(restart(&in_porch, &late, &beam, 0), Restart::NotTested, "the porch of a scan that ran straight through");
        let idling = Sample { at_ns: 0, line: beam.idle_from + 5, vblank: true };
        assert_eq!(restart(&idling, &late, &beam, 0), Restart::Began);
    }

    #[test]
    fn a_present_while_the_display_idles_short_of_the_margin_is_not_tested() {
        let beam = measured();
        let late = [at(1_000_000, 203, false)];
        assert_eq!(restart(&at(0, beam.total_rows + 100, true), &late, &beam, 0), Restart::NotTested);
        assert_eq!(restart(&at(0, beam.idle_from, true), &late, &beam, 0), Restart::Began);
    }

    #[test]
    fn a_scan_at_the_panels_own_refresh_period_is_not_the_presents() {
        // 400 lines per ms. A 48 Hz floor refreshes after 20.8 ms; the stream is 60 fps, so it
        // presents at 16.7 ms of idling. The scan that starts 20.8 ms after the last one began is
        // the panel's, whenever the present was, and cannot show the present restarted anything.
        let beam = measured();
        let before = at(1_000_000, 6_680, true); // 16.7 ms after the last scan began
        let forced = [at(1_000_000 + 4_600_000, 200, false)]; // its picture starts 20.8 ms after the last start
        assert_eq!(restart(&before, &forced, &beam, 20_800_000), Restart::NotTested);
        // The same scan with nothing known about the floor is taken at its word.
        assert_eq!(restart(&before, &forced, &beam, 0), Restart::Began);
        // A flip 1.5 ms after the present is 3.8 ms short of the floor: the present's.
        let flipped = [at(1_000_000 + 2_000_000, 200, false)];
        assert_eq!(restart(&before, &flipped, &beam, 20_800_000), Restart::Began);
    }

    #[test]
    fn a_counter_whose_top_differs_from_the_monitors_height_is_refused() {
        // A scaled desktop or a rotated display: the monitor says 1440 rows, the counter walks 1080.
        let starts: Vec<u64> = (0..60).map(|i| 3_000_000 + i * 16_667_000).collect();
        let samples = counting(&starts, BLANK, 1080, RATE, 250_000, 1_000_000_000);
        assert_eq!(calibrate(&samples, 1440), Err(CalibrationError::RowsDiffer));
        assert!(calibrate(&samples, 1080).is_ok(), "the same readings fit the height they walk");
        assert!(calibrate(&samples, 1100).is_ok(), "within 3%");
        assert_eq!(decide(&Evidence { beam: Some(Err(CalibrationError::RowsDiffer)), ..evidence() }).line(), "off (scan rows differ from the display's)");
    }

    #[test]
    fn a_stream_at_the_displays_rate_has_no_idle_to_test() {
        let mut starved = RestartCheck::default();
        for _ in 0..CHECK_FRAMES { starved.push(Restart::NotTested); }
        let line = |hz: f64| decide(&Evidence { check: starved, content_hz: hz, ..evidence() }).line();
        assert_eq!(line(240.0), "off (no idle at this frame rate)");
        assert_eq!(line(200.0), "off (no idle at this frame rate)");
        assert_eq!(line(60.0), "off (no VRR)", "a display that scans all the time under a slow stream");
    }

    #[test]
    fn a_check_is_judged_at_the_rate_it_ran_at_not_a_later_one() {
        let mut starved = RestartCheck::default();
        for _ in 0..CHECK_FRAMES { starved.push(Restart::NotTested); }
        // Starved and failed at 200 fps on a 240 Hz VRR panel calibrated near its top rate; the
        // stream has slowed to 60 since.
        let later = Evidence { beam: Some(Ok(beam(0.12))), content_hz: 60.0, check_hz: 200.0, ..evidence() };
        assert_eq!(decide(&Evidence { check: starved, ..later }).line(), "off (no idle at this frame rate)");
        assert_eq!(decide(&Evidence { check: missed(), ..later }).line(), "off (no VRR scan restart)");
        // Run at 60 fps, they say what they did.
        let slow = Evidence { check_hz: 60.0, content_hz: 200.0, ..later };
        assert_eq!(decide(&Evidence { check: starved, ..slow }).line(), "off (no VRR)");
        assert_eq!(decide(&Evidence { check: missed(), ..slow }).line(), "off (no VRR)");
    }

    #[test]
    fn a_fixed_refresh_is_told_a_second_after_its_calibration_without_waiting_for_the_check() {
        let presents = |results: &[Restart]| {
            let mut check = RestartCheck::default();
            for &result in results { check.push(result); }
            check
        };
        let quiet = |n: u32| presents(&vec![Restart::NotTested; n as usize]);
        // The laptop's 120 Hz panel under a 60 fps stream: 1% of the readings in the blanking, and
        // a second of presents none of which started a scan.
        let laptop = Evidence { check: quiet(FIXED_FRAMES), beam: Some(Ok(beam(0.01))), content_hz: 60.0, display_hz: 119.5, ..evidence() };
        assert_eq!(decide(&laptop).line(), "off (no VRR)");
        let mut missing = vec![Restart::NotTested; FIXED_FRAMES as usize];
        missing[10] = Restart::Missed;
        assert_eq!(decide(&Evidence { check: presents(&missing), ..laptop }).line(), "off (no VRR)", "a present in the blanking that did not restart it");
        // Not yet: the calibration alone decides nothing.
        assert_eq!(decide(&Evidence { check: quiet(FIXED_FRAMES - 1), ..laptop }), Verdict::Checking);
        // The VRR panel calibrated while it still scanned at its top rate (12% in the blanking):
        // a present that started a scan leaves it to the check.
        let mut began = vec![Restart::NotTested; FIXED_FRAMES as usize];
        began[5] = Restart::Began;
        let busy_vrr = Evidence { check: presents(&began), beam: Some(Ok(beam(0.12))), ..evidence() };
        assert_eq!(decide(&busy_vrr), Verdict::Checking);
        // This PC's 240 Hz VRR panel at 60 fps idles between scans: the check decides.
        let vrr = Evidence { check: quiet(FIXED_FRAMES), beam: Some(Ok(beam(0.67))), ..evidence() };
        assert_eq!(decide(&vrr), Verdict::Checking);
        // At the display's own rate a fixed refresh and a busy VRR display look alike.
        assert_eq!(decide(&Evidence { content_hz: 119.5, ..laptop }), Verdict::Checking);
        // A check that passed is not overruled by the blanking share.
        assert!(matches!(decide(&Evidence { beam: Some(Ok(beam(0.12))), ..evidence() }), Verdict::On(..)));
    }

    /// Manual: `cargo test -p nightjar-desktop scanline_dump -- --ignored --nocapture` while a
    /// stream (or anything that presents) runs on the primary monitor. Prints raw readings.
    #[cfg(windows)]
    #[test]
    #[ignore = "reads the primary monitor's real scan line"]
    fn scanline_dump() {
        use std::time::Duration;
        use windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow;
        // SAFETY: the desktop window handle is always valid.
        let scan = Scanline::open(unsafe { GetDesktopWindow() }, std::time::Instant::now()).expect("scanline on the primary monitor");
        println!("monitor {}", scan.device_name());
        let samples = collect(&scan, Duration::from_millis(1000), Duration::from_micros(250));
        for s in samples.iter().take(400) { println!("{} {} {}", s.at_ns, s.line, u8::from(s.vblank)); }
        println!("{:?}", calibrate(&samples, 0));
    }
}
