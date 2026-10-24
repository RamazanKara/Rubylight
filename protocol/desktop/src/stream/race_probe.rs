//! The beam race capability probe on a stream (B10 step 4, docs/beam-racing.md): opens the scan
//! line of the window's monitor, calibrates the scan on a thread for the first second, runs the
//! scan-restart check on the presents that follow (the polling that watches for the new scan runs
//! on a helper thread, never on the render thread) and keeps the verdict the overlay and the timing
//! CSV show. Created only when the beam race is on, so a default stream does none of it.

use std::sync::{Arc, Mutex};

use sdl3_sys::everything::*;

#[cfg(any(windows, test))]
use super::scanline::Follow;
use super::scanline::{decide, Beam, CalibrationError, Evidence, RestartCheck, Sample, Verdict};

/// What the timing CSV records for one present: the beam when the present was issued and the
/// verdict (`race_reason`: "on", the reason, or empty while it is being checked).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanReport {
    pub line: Option<u32>,
    pub vblank: Option<bool>,
    pub reason: &'static str,
}

impl ScanReport {
    pub fn new(sample: Option<Sample>, verdict: &Verdict) -> Self {
        Self { line: sample.map(|s| s.line), vblank: sample.map(|s| s.vblank), reason: verdict.code() }
    }
}

/// What the calibration thread leaves: the generation it measured for, and the beam.
type Calibration = Arc<Mutex<Option<(u32, Result<Beam, CalibrationError>)>>>;

/// The calibration result if it is for `generation`. A result for an earlier one (the window
/// moved to another monitor while it ran) is dropped: it measured the wrong display.
fn take_current(slot: &Calibration, generation: u32) -> Option<Result<Beam, CalibrationError>> {
    let (measured_for, result) = slot.lock().ok()?.take()?;
    (measured_for == generation).then_some(result)
}

/// "median 0.95 ms, longest 5.1 ms" of the delays, or what it was when none began.
fn delays(delays: &[u64]) -> String {
    if delays.is_empty() { return "never".into(); }
    let mut sorted = delays.to_vec();
    sorted.sort_unstable();
    format!("median {:.2} ms, longest {:.2} ms ({} presents that began a scan)", sorted[sorted.len() / 2] as f64 / 1e6, sorted[sorted.len() - 1] as f64 / 1e6, sorted.len())
}

/// How the stream's window sits on its display, read by the presenter.
#[derive(Clone, Copy, Debug)]
pub struct Surroundings {
    /// The swap chain tears and the pacing mode presents with tearing.
    pub tearing: bool,
    /// The window is a fullscreen window.
    pub fullscreen: bool,
    /// The stream's frame rate.
    pub content_hz: f64,
}

pub struct RaceProbe {
    verdict: Verdict,
    reported: String,
    check: RestartCheck,
    beam: Option<Result<Beam, CalibrationError>>,
    calibration: Calibration,
    /// Counts the monitors the probe has measured; a calibration result carries the one it was for.
    generation: u32,
    scanline_ok: bool,
    presents: u32,
    check_said: bool,
    /// Delay from each tested present to the first reading of the picture, for the summary line (ns).
    delays: Vec<u64>,
    /// The longest the display has idled so far (ns), which the check keeps its tests clear of.
    longest_idle_ns: u64,
    #[cfg(windows)]
    scan: Option<super::scanline::Scanline>,
    #[cfg(windows)]
    helper: Option<win::Helper>,
}

impl RaceProbe {
    fn new(scanline_ok: bool) -> Self {
        Self {
            verdict: Verdict::Checking,
            reported: String::new(),
            check: RestartCheck::default(),
            beam: None,
            calibration: Calibration::default(),
            generation: 0,
            scanline_ok,
            presents: 0,
            check_said: false,
            delays: Vec::new(),
            longest_idle_ns: 0,
            #[cfg(windows)]
            scan: None,
            #[cfg(windows)]
            helper: None,
        }
    }

    /// What a verdict looks like for a CSV row.
    pub fn report(&self, sample: Option<Sample>) -> ScanReport { ScanReport::new(sample, &self.verdict) }

    /// The overlay's value for the "Beam race" line.
    pub fn line(&self) -> String { self.verdict.line() }

    fn log_change(&mut self) {
        let line = self.verdict.line();
        if line != self.reported {
            eprintln!("nightjar: beam race: {line}");
            self.reported = line;
        }
    }

    /// What the window moving to another monitor (`follow`) means for the measurements. Returns
    /// whether the new display has to be measured again.
    #[cfg(any(windows, test))]
    fn apply_follow(&mut self, follow: Follow) -> bool {
        match follow {
            Follow::Same => false,
            Follow::Moved | Follow::Lost => {
                // Another display has another scan: whatever was measured is not about this one.
                self.generation += 1;
                self.check = RestartCheck::default();
                self.check_said = false;
                self.delays.clear();
                self.longest_idle_ns = 0;
                self.beam = None;
                if follow == Follow::Lost {
                    eprintln!("nightjar: beam race: the scan line of the monitor the window moved to cannot be read");
                    self.scanline_ok = false;
                    return false;
                }
                eprintln!("nightjar: beam race: the window moved to another monitor, measuring it again");
                true
            }
        }
    }

    /// Folds what the calibration thread found and the check so far into the verdict.
    fn decide(&mut self, around: Surroundings) {
        if self.beam.is_none() {
            self.beam = take_current(&self.calibration, self.generation);
            if let Some(Ok(beam)) = &self.beam {
                self.longest_idle_ns = beam.longest_idle_ns;
                eprintln!(
                    "nightjar: beam race: scan {:.2} ms, {} rows behind {} lines of blanking, {:.1} lines per ms, {:.1} Hz at the top, \
                     {:.0}% of the readings in the blanking, idling counted from line {}, longest idle {:.1} ms, {} scans",
                    beam.scan_ns as f64 / 1e6, beam.rows, beam.blank_rows, beam.lines_per_ms, beam.refresh_hz, beam.vblank_fraction * 100.0,
                    beam.idle_from, beam.longest_idle_ns as f64 / 1e6, beam.scans
                );
            }
        }
        if !self.check_said && !self.check.running() {
            self.check_said = true;
            eprintln!(
                "nightjar: beam race: restart check over {} presents: {} tested, {} did not start a scan within {} ms; scan began after {}",
                self.check.frames, self.check.tested, self.check.missed, super::scanline::RESTART_WINDOW_NS / 1_000_000, delays(&self.delays)
            );
        }
        let display_hz = match &self.beam { Some(Ok(beam)) => beam.refresh_hz, _ => 0.0 };
        self.verdict = decide(&Evidence {
            tearing: around.tearing,
            scanline: self.scanline_ok,
            fullscreen: around.fullscreen,
            beam: self.beam,
            check: self.check,
            content_hz: around.content_hz,
            display_hz,
        });
        self.log_change();
    }
}

#[cfg(windows)]
mod win {
    use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender, TrySendError};
    use std::time::{Duration, Instant};

    use windows::Win32::Foundation::HWND;

    use super::super::scanline::{calibrate, collect, restart, Beam, CalibrationError, Follow, Restart, Sample, Scanline, RESTART_WINDOW_NS};
    use super::{RaceProbe, Surroundings};

    /// How long the calibration reads the scan line, and how often.
    const CALIBRATION: Duration = Duration::from_millis(1200);
    const EVERY: Duration = Duration::from_micros(250);

    /// Presents between looks at whether the window moved to another monitor.
    const FOLLOW_EVERY: u32 = 120;

    /// A helper that gets to a present more than this late (ns) has missed the first readings of
    /// the scan it was to watch for, and says nothing about that present.
    const LATE_START_NS: u64 = 1_000_000;

    /// One present for the helper to watch: the beam from just before the call, when the call
    /// returned (ns on the scan clock) and what the check compares against.
    struct Job {
        before: Sample,
        returned_ns: u64,
        beam: Beam,
        longest_idle_ns: u64,
        generation: u32,
    }

    /// What the helper saw after one present.
    struct Outcome {
        restart: Restart,
        /// From the reading before the present to the first reading of the picture.
        delay_ns: Option<u64>,
        generation: u32,
    }

    /// The thread that polls the scan line for the new scan after a present, so the render thread
    /// does not spin for up to `RESTART_WINDOW_NS` on it.
    pub(super) struct Helper {
        jobs: SyncSender<Job>,
        outcomes: Receiver<Outcome>,
    }

    impl Helper {
        fn spawn(window: HWND, clock: Instant) -> Option<Self> {
            let (jobs, queue) = sync_channel::<Job>(1);
            let (results, outcomes) = channel::<Outcome>();
            // HWND is a plain handle value; the thread opens its own adapter on it.
            let handle = window.0 as usize;
            std::thread::Builder::new().name("beam-restart".into()).spawn(move || watch(handle, clock, queue, results)).ok()?;
            Some(Self { jobs, outcomes })
        }
    }

    fn watch(handle: usize, clock: Instant, queue: Receiver<Job>, results: Sender<Outcome>) {
        // The same clock as the render thread's readings, so the times compare.
        let Some(mut scan) = Scanline::open(HWND(handle as *mut _), clock) else { return };
        for job in queue {
            let mut outcome = Outcome { restart: Restart::NotTested, delay_ns: None, generation: job.generation };
            let now = clock.elapsed().as_nanos() as u64;
            if scan.follow_window() == Follow::Same && now <= job.returned_ns + LATE_START_NS {
                let end = job.returned_ns + RESTART_WINDOW_NS;
                let mut after = Vec::with_capacity(64);
                while (clock.elapsed().as_nanos() as u64) < end {
                    if let Some(sample) = scan.sample() {
                        let active = !sample.vblank;
                        after.push(sample);
                        if active { break; }
                    }
                }
                outcome.delay_ns = after.iter().find(|s| !s.vblank).map(|first| first.at_ns.saturating_sub(job.before.at_ns));
                outcome.restart = restart(&job.before, &after, &job.beam, job.longest_idle_ns);
            }
            if results.send(outcome).is_err() { return; }
        }
    }

    impl RaceProbe {
        /// Opens the scan line of the monitor `window` is on and starts the calibration.
        pub fn start(window: HWND, around: Surroundings) -> Self {
            let scan = Scanline::open(window, Instant::now());
            let mut probe = Self::new(scan.is_some());
            probe.scan = scan;
            match &probe.scan {
                Some(scan) => {
                    eprintln!("nightjar: beam race: reading the scan line of {} ({} rows)", scan.device_name(), scan.rows());
                    probe.helper = Helper::spawn(window, scan.clock());
                    if probe.helper.is_none() {
                        eprintln!("nightjar: beam race: could not start the restart check's thread");
                        probe.scanline_ok = false;
                    }
                    probe.calibrate_in_background(window);
                }
                None => eprintln!("nightjar: beam race: the scan line of the window's monitor cannot be read (D3DKMTGetScanLine)"),
            }
            probe.decide(around);
            probe
        }

        fn calibrate_in_background(&mut self, window: HWND) {
            let result = self.calibration.clone();
            let generation = self.generation;
            // HWND is a plain handle value; the thread opens its own adapter on it.
            let handle = window.0 as usize;
            let spawned = std::thread::Builder::new().name("beam-calibration".into()).spawn(move || {
                let outcome = match Scanline::open(HWND(handle as *mut _), Instant::now()) {
                    Some(scan) => {
                        let samples = collect(&scan, CALIBRATION, EVERY);
                        calibrate(&samples, scan.rows())
                    }
                    None => Err(CalibrationError::NoScan),
                };
                if let Ok(mut slot) = result.lock() { *slot = Some((generation, outcome)); }
            });
            if let Err(error) = spawned {
                eprintln!("nightjar: beam race: could not start the calibration: {error}");
                self.scanline_ok = false;
            }
        }

        /// The beam just before a present is issued.
        pub fn before(&self) -> Option<Sample> {
            self.scan.as_ref().and_then(Scanline::sample)
        }

        /// Takes what the helper found out of its queue into the check.
        fn collect_outcomes(&mut self) {
            let Some(helper) = &self.helper else { return };
            while let Ok(outcome) = helper.outcomes.try_recv() {
                // A result for the monitor the window has left says nothing about this one.
                if outcome.generation != self.generation { continue; }
                if self.check.running() {
                    if let Some(delay) = outcome.delay_ns { self.delays.push(delay); }
                    self.check.push(outcome.restart);
                }
            }
        }

        /// After the present call returned: hands the present to the helper while the check runs,
        /// then folds the verdict. `before` is `before()`'s reading.
        pub fn after(&mut self, window: HWND, before: Option<Sample>, around: Surroundings) {
            self.presents += 1;
            if self.presents % FOLLOW_EVERY == 0 {
                let follow = self.scan.as_mut().map_or(Follow::Same, Scanline::follow_window);
                if follow == Follow::Lost { self.scan = None; }
                if self.apply_follow(follow) { self.calibrate_in_background(window); }
            }
            self.collect_outcomes();
            if let (Some(Ok(beam)), Some(before), Some(scan)) = (self.beam, before, &self.scan) {
                if self.check.running() {
                    let idle = before.vblank && before.line >= beam.idle_from;
                    let (mut sent, mut gone) = (false, false);
                    if let (true, Some(helper)) = (idle, &self.helper) {
                        let job = Job {
                            before,
                            returned_ns: scan.clock().elapsed().as_nanos() as u64,
                            beam,
                            longest_idle_ns: self.longest_idle_ns,
                            generation: self.generation,
                        };
                        match helper.jobs.try_send(job) {
                            Ok(()) => sent = true,
                            // Still watching the last present: this one is not tested.
                            Err(TrySendError::Full(_)) => {}
                            Err(TrySendError::Disconnected(_)) => gone = true,
                        }
                    }
                    if gone {
                        eprintln!("nightjar: beam race: the restart check's thread stopped, its scan line cannot be read");
                        self.helper = None;
                        self.scanline_ok = false;
                    }
                    if idle {
                        // The longest the display idled bounds its own refresh period; this reading counts for the next presents.
                        self.longest_idle_ns = self.longest_idle_ns.max((f64::from(before.line) * 1e6 / beam.lines_per_ms) as u64);
                    }
                    if !sent { self.check.push(Restart::NotTested); }
                }
            }
            self.decide(around);
        }
    }
}

#[cfg(not(windows))]
impl RaceProbe {
    /// The scan line is a Windows call: another platform has none to read.
    fn start_unavailable(around: Surroundings) -> Self {
        let mut probe = Self::new(false);
        probe.decide(around);
        probe
    }

    pub fn before(&self) -> Option<Sample> { None }

    fn after_unavailable(&mut self, around: Surroundings) {
        self.presents += 1;
        self.decide(around);
    }
}

/// The window and what it presents on, as the presenter sees them now.
fn surroundings(renderer: *mut SDL_Renderer, mode_tears: bool, content_hz: f64) -> (Surroundings, *mut SDL_Window) {
    // SAFETY: a live renderer and the window it draws to.
    let window = unsafe { SDL_GetRenderWindow(renderer) };
    // SAFETY: as above.
    let fullscreen = !window.is_null() && unsafe { SDL_GetWindowFlags(window) } & SDL_WINDOW_FULLSCREEN != 0;
    #[cfg(windows)]
    let chain_tears = super::super::media::swapchain_allows_tearing(crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER));
    #[cfg(not(windows))]
    let chain_tears = true;
    (Surroundings { tearing: mode_tears && chain_tears, fullscreen, content_hz }, window)
}

#[cfg(windows)]
fn hwnd_of(window: *mut SDL_Window) -> windows::Win32::Foundation::HWND {
    // SAFETY: a live window; the property is its HWND.
    let hwnd = unsafe { SDL_GetPointerProperty(SDL_GetWindowProperties(window), SDL_PROP_WINDOW_WIN32_HWND_POINTER, std::ptr::null_mut()) };
    windows::Win32::Foundation::HWND(hwnd)
}

impl RaceProbe {
    /// Starts the probe for the stream's renderer. `mode_tears`: the pacing mode presents with
    /// tearing (everything but V-Sync).
    pub fn begin(renderer: *mut SDL_Renderer, mode_tears: bool, content_hz: f64) -> Self {
        let (around, window) = surroundings(renderer, mode_tears, content_hz);
        #[cfg(windows)]
        { Self::start(hwnd_of(window), around) }
        #[cfg(not(windows))]
        { let _ = window; Self::start_unavailable(around) }
    }

    /// After a present call returned. `before` is `before()`'s reading from just ahead of it.
    pub fn after_present(&mut self, renderer: *mut SDL_Renderer, before: Option<Sample>, mode_tears: bool, content_hz: f64) {
        let (around, window) = surroundings(renderer, mode_tears, content_hz);
        #[cfg(windows)]
        self.after(hwnd_of(window), before, around);
        #[cfg(not(windows))]
        { let _ = (window, before); self.after_unavailable(around) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn beam() -> Beam {
        Beam { rows: 1440, blank_rows: 200, total_rows: 1640, scan_ns: 4_100_000, refresh_hz: 243.9, lines_per_ms: 400.0, vblank_fraction: 0.75, scans: 50, idle_from: 1840, longest_idle_ns: 17_000_000 }
    }

    fn around() -> Surroundings { Surroundings { tearing: true, fullscreen: true, content_hz: 60.0 } }

    #[test]
    fn a_calibration_for_another_monitor_is_dropped() {
        let slot = Calibration::default();
        *slot.lock().unwrap() = Some((1, Ok(beam())));
        assert!(take_current(&slot, 2).is_none(), "measured the monitor the window has left");
        assert!(slot.lock().unwrap().is_none(), "and gone, not kept for later");
        *slot.lock().unwrap() = Some((2, Ok(beam())));
        assert!(matches!(take_current(&slot, 2), Some(Ok(_))));
    }

    #[test]
    fn a_probe_ignores_the_stale_calibration_and_takes_the_new_one() {
        let mut probe = RaceProbe::new(true);
        *probe.calibration.lock().unwrap() = Some((0, Ok(beam())));
        assert!(probe.apply_follow(Follow::Moved), "the new display is measured again");
        probe.decide(around());
        assert!(probe.beam.is_none(), "the old display's beam is not the new one's");
        *probe.calibration.lock().unwrap() = Some((1, Ok(Beam { rows: 1080, ..beam() })));
        probe.decide(around());
        assert!(matches!(probe.beam, Some(Ok(b)) if b.rows == 1080));
    }

    #[test]
    fn a_monitor_that_cannot_be_opened_says_no_scanline() {
        let mut probe = RaceProbe::new(true);
        *probe.calibration.lock().unwrap() = Some((0, Ok(beam())));
        probe.decide(around());
        assert!(probe.beam.is_some());
        assert!(!probe.apply_follow(Follow::Lost), "there is nothing to measure");
        probe.decide(around());
        assert_eq!(probe.line(), "off (no scanline)");
        assert!(probe.beam.is_none() && probe.delays.is_empty(), "nothing of the old display is kept");
    }

    #[test]
    fn staying_on_the_monitor_keeps_the_measurements() {
        let mut probe = RaceProbe::new(true);
        *probe.calibration.lock().unwrap() = Some((0, Ok(beam())));
        probe.decide(around());
        assert!(!probe.apply_follow(Follow::Same));
        assert!(probe.beam.is_some());
        assert_eq!(probe.longest_idle_ns, 17_000_000);
    }
}
