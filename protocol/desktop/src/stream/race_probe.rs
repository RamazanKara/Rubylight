//! The beam race capability probe on a stream (B10 step 4, docs/beam-racing.md): opens the
//! scan line of the window's monitor, calibrates the scan on a thread for the first second, runs
//! the scan-restart check on the presents that follow and keeps the verdict the overlay and the
//! timing CSV show. Created only when the beam race is on, so a default stream does none of it.

use std::sync::{Arc, Mutex};

use sdl3_sys::everything::*;

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

type Calibration = Arc<Mutex<Option<Result<Beam, CalibrationError>>>>;

/// "median 0.95 ms, longest 5.1 ms" of the delays, or what it was when none began.
fn delays(delays: &[u64]) -> String {
    if delays.is_empty() { return "never".into(); }
    let mut sorted = delays.to_vec();
    sorted.sort_unstable();
    format!("median {:.2} ms, longest {:.2} ms ({} of the tested presents)", sorted[sorted.len() / 2] as f64 / 1e6, sorted[sorted.len() - 1] as f64 / 1e6, sorted.len())
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
    scanline_ok: bool,
    presents: u32,
    check_said: bool,
    /// Delay from each tested present to the first reading of the picture, for the summary line (ns).
    delays: Vec<u64>,
    #[cfg(windows)]
    scan: Option<super::scanline::Scanline>,
}

impl RaceProbe {
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

    /// Folds what the calibration thread found and the check so far into the verdict.
    fn decide(&mut self, around: Surroundings) {
        if self.beam.is_none() {
            self.beam = self.calibration.lock().ok().and_then(|mut done| done.take());
            if let Some(Ok(beam)) = &self.beam {
                eprintln!(
                    "nightjar: beam race: scan {:.2} ms, {} rows behind {} lines of blanking, {:.1} lines per ms, {:.1} Hz at the top, \
                     {:.0}% of the readings in the blanking, {} scans",
                    beam.scan_ns as f64 / 1e6, beam.rows, beam.blank_rows, beam.lines_per_ms, beam.refresh_hz, beam.vblank_fraction * 100.0, beam.scans
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
    use std::time::{Duration, Instant};

    use windows::Win32::Foundation::HWND;

    use super::super::scanline::{calibrate, collect, restart, CalibrationError, Restart, RestartCheck, Sample, Scanline, Verdict, RESTART_WINDOW_NS};
    use super::{Calibration, RaceProbe, Surroundings};

    /// How long the calibration reads the scan line, and how often.
    const CALIBRATION: Duration = Duration::from_millis(1200);
    const EVERY: Duration = Duration::from_micros(250);

    /// Presents between looks at whether the window moved to another monitor.
    const FOLLOW_EVERY: u32 = 120;

    impl RaceProbe {
        /// Opens the scan line of the monitor `window` is on and starts the calibration.
        pub fn start(window: HWND, around: Surroundings) -> Self {
            let scan = Scanline::open(window, Instant::now());
            let mut probe = Self {
                verdict: Verdict::Checking,
                reported: String::new(),
                check: RestartCheck::default(),
                beam: None,
                calibration: Calibration::default(),
                scanline_ok: scan.is_some(),
                presents: 0,
                check_said: false,
                delays: Vec::new(),
                scan,
            };
            match &probe.scan {
                Some(scan) => {
                    eprintln!("nightjar: beam race: reading the scan line of {} ({} rows)", scan.device_name(), scan.rows());
                    probe.calibrate_in_background(window);
                }
                None => eprintln!("nightjar: beam race: the scan line of the window's monitor cannot be read (D3DKMTGetScanLine)"),
            }
            probe.decide(around);
            probe
        }

        fn calibrate_in_background(&mut self, window: HWND) {
            let result = self.calibration.clone();
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
                if let Ok(mut slot) = result.lock() { *slot = Some(outcome); }
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

        /// After the present call returned: the restart check on this present while it runs, then
        /// the verdict. `before` is `before()`'s reading.
        pub fn after(&mut self, window: HWND, before: Option<Sample>, around: Surroundings) {
            self.presents += 1;
            if self.presents % FOLLOW_EVERY == 0 {
                let moved = self.scan.as_mut().is_some_and(Scanline::follow_window);
                if moved {
                    // Another display has another scan: measure it again.
                    eprintln!("nightjar: beam race: the window moved to another monitor, measuring it again");
                    self.check = RestartCheck::default();
                    self.check_said = false;
                    self.delays.clear();
                    self.beam = None;
                    self.calibrate_in_background(window);
                }
            }
            if let (Some(Ok(beam)), Some(before), Some(scan)) = (&self.beam, before, &self.scan) {
                if self.check.running() && before.vblank {
                    // Poll for the new scan for as long as the check allows: it begins at once or not at all.
                    let end = scan.clock().elapsed().as_nanos() as u64 + RESTART_WINDOW_NS;
                    let mut after = Vec::with_capacity(32);
                    while (scan.clock().elapsed().as_nanos() as u64) < end {
                        if let Some(sample) = scan.sample() {
                            let active = !sample.vblank;
                            after.push(sample);
                            if active { break; }
                        }
                    }
                    if let Some(first) = after.iter().find(|s| !s.vblank) { self.delays.push(first.at_ns.saturating_sub(before.at_ns)); }
                    self.check.push(restart(&before, &after, beam));
                } else if self.check.running() {
                    self.check.push(Restart::NotTested);
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
        let mut probe = Self {
            verdict: Verdict::Checking,
            reported: String::new(),
            check: RestartCheck::default(),
            beam: None,
            calibration: Calibration::default(),
            scanline_ok: false,
            presents: 0,
            check_said: false,
            delays: Vec::new(),
        };
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
