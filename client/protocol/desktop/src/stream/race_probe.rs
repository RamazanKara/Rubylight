//! The beam race capability probe on a stream (B10 step 4, docs/beam-racing.md): opens the scan
//! line of the window's monitor, calibrates the scan on a thread for the first second, runs the
//! scan-restart check on the presents that follow (the polling that watches for the new scan runs
//! on a helper thread, never on the render thread) and keeps the verdict the overlay and the timing
//! CSV show. Created when the beam race is on, on a VRR stream that presents with tearing, where a
//! "no VRR" verdict switches the stream to the pacing it would have without VRR
//! (`Streaming::sync_vrr`), and for `NIGHTJAR_VRR_SYNC=scan`. Other streams do none of it.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use sdl3_sys::everything::*;

#[cfg(any(windows, test))]
use super::scanline::Follow;
use super::scanline::{busy, decide, Beam, CalibrationError, Evidence, RestartCheck, Sample, Verdict};

/// The share of the display's rate a stream that ran the check at a busy one (`busy`, 70%) has to
/// slow below before the display is measured again: a rate that hovers about 70% does not restart
/// the check on every second.
const SLOWER: f64 = 0.6;

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

/// Presents per second over the last second or so: what the display is asked to show, which can be
/// far below the stream's rate (a still desktop, a game held at 30 fps).
#[derive(Debug, Default)]
struct PresentRate {
    since: Option<Instant>,
    count: u32,
    hz: Option<f64>,
}

impl PresentRate {
    fn note(&mut self, at: Instant) {
        let Some(since) = self.since else {
            self.since = Some(at);
            return;
        };
        self.count += 1;
        let elapsed = at.duration_since(since).as_secs_f64();
        if elapsed >= 1.0 {
            self.hz = Some(f64::from(self.count) / elapsed);
            (self.since, self.count) = (Some(at), 0);
        }
    }

    /// The rate the display is shown pictures at: the measured one once there is one, never above
    /// the stream's (`stream_hz`, 0 when not known).
    fn content_hz(&self, stream_hz: f64) -> f64 {
        match self.hz {
            Some(hz) if stream_hz > 0.0 => hz.min(stream_hz),
            Some(hz) => hz,
            None => stream_hz,
        }
    }
}

/// How the stream's window sits on its display, read by the presenter.
#[derive(Clone, Copy, Debug)]
pub struct Surroundings {
    /// The swap chain tears and the pacing mode presents with tearing.
    pub tearing: bool,
    /// The window is a fullscreen window.
    pub fullscreen: bool,
    /// The window has the keyboard focus.
    pub focused: bool,
    /// The stream's frame rate.
    pub content_hz: f64,
}

pub struct RaceProbe {
    /// What the log lines start with: "beam race", or "display check" when only VRR asked for it.
    topic: &'static str,
    verdict: Verdict,
    reported: String,
    rate: PresentRate,
    check: RestartCheck,
    /// The highest rate the stream presented at while the check ran: a check that saw a busy rate
    /// is measured again when the rate falls (`apply_surroundings`).
    check_hz: f64,
    /// Whether the last present went straight to a fullscreen window nothing covers, None before
    /// the first. Only those feed the check: a composed present never restarts a scan.
    direct: Option<bool>,
    /// The window had the focus once. A fullscreen window that lost it may lie under another
    /// (Alt-Tab in borderless fullscreen), composed, until it has it again (`covered`).
    had_focus: bool,
    covered: bool,
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
    /// The median of `delays` (ns), 0 with none.
    delay_ns: u64,
    #[cfg(windows)]
    scan: Option<super::scanline::Scanline>,
    #[cfg(windows)]
    helper: Option<win::Helper>,
}

impl RaceProbe {
    fn new(scanline_ok: bool, topic: &'static str) -> Self {
        Self {
            topic,
            verdict: Verdict::Checking,
            reported: String::new(),
            rate: PresentRate::default(),
            check: RestartCheck::default(),
            check_hz: 0.0,
            direct: None,
            had_focus: false,
            covered: false,
            beam: None,
            calibration: Calibration::default(),
            generation: 0,
            scanline_ok,
            presents: 0,
            check_said: false,
            delays: Vec::new(),
            longest_idle_ns: 0,
            delay_ns: 0,
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

    /// What the display check says so far.
    pub fn verdict(&self) -> &Verdict { &self.verdict }

    /// The display scans at its own rate whatever is presented ("no VRR"), so a tearing present
    /// tears: the stream is better off with V-Sync.
    pub fn fixed_refresh(&self) -> bool { self.verdict.fixed_refresh() }

    /// The scan of the window's display, once calibrated.
    pub fn beam(&self) -> Option<Beam> {
        match self.beam {
            Some(Ok(beam)) => Some(beam),
            _ => None,
        }
    }

    /// The beam and how long a tearing present on an idle display takes to start the scan (ns),
    /// when racing is possible: the verdict is on and a present that began a scan was timed.
    pub fn racing(&self) -> Option<(Beam, u64)> {
        match self.verdict {
            Verdict::On(beam, capability) if capability.on() && self.delay_ns > 0 => Some((beam, self.delay_ns)),
            _ => None,
        }
    }

    fn log_change(&mut self) {
        let line = self.verdict.line();
        if line != self.reported {
            eprintln!("nightjar: {}: {line}", self.topic);
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
                self.forget();
                if follow == Follow::Lost {
                    eprintln!("nightjar: {}: the scan line of the monitor the window moved to cannot be read", self.topic);
                    self.scanline_ok = false;
                    return false;
                }
                eprintln!("nightjar: {}: the window moved to another monitor, measuring it again", self.topic);
                true
            }
        }
    }

    /// Drops the calibration, the check and the verdict, to measure the display again.
    fn forget(&mut self) {
        self.generation += 1;
        self.check = RestartCheck::default();
        self.check_hz = 0.0;
        self.check_said = false;
        self.delays.clear();
        self.delay_ns = 0;
        self.longest_idle_ns = 0;
        self.beam = None;
        self.verdict = Verdict::Checking;
    }

    /// Whether this present feeds the restart check: it is still running and the present goes
    /// straight to a fullscreen window.
    fn checking(&self) -> bool { self.check.running() && self.direct == Some(true) }

    /// What the window and the stream's rate mean for what was measured so far, ahead of the
    /// present's check. Returns whether the display has to be measured again. A display found to
    /// scan at its own rate stays so: the window and the rate do not change the display.
    fn apply_surroundings(&mut self, around: Surroundings) -> bool {
        if around.focused { self.had_focus = true; }
        self.covered = around.fullscreen && self.had_focus && !around.focused;
        let direct = around.fullscreen && !self.covered;
        let was = self.direct.replace(direct);
        if self.fixed_refresh() { return false; }
        if was == Some(false) && direct {
            // A composed window shows its presents at the compositor's refresh: a calibration and
            // presents from then say nothing about how the display follows this one's.
            eprintln!("nightjar: {}: the window went fullscreen, measuring the display again", self.topic);
            self.forget();
            return true;
        }
        // A check that passed saw the display follow presents: no rate changes that.
        let display_hz = self.beam().map_or(0.0, |beam| beam.refresh_hz);
        if display_hz <= 0.0 || self.check.passed() { return false; }
        let content_hz = self.rate.content_hz(around.content_hz);
        if busy(self.check_hz, display_hz) && content_hz < display_hz * SLOWER {
            // Presents at a busy rate never let a display idle, VRR or not, and the ones it tested
            // can miss behind a deep queue: what the check saw then, starved or failed, would read
            // as a fixed refresh at the slower rate.
            eprintln!("nightjar: {}: the stream slowed to {content_hz:.0} presents a second, measuring the display again", self.topic);
            self.forget();
            return true;
        }
        if self.checking() { self.check_hz = self.check_hz.max(content_hz); }
        false
    }

    /// One present after its call returned: the window's move (`follow`), the window and the rate,
    /// the helper's results, the present itself while the check takes it (`test`), and the
    /// verdict. Returns whether the display has to be calibrated again.
    #[cfg(any(windows, test))]
    fn fold(&mut self, around: Surroundings, follow: Follow, test: impl FnOnce(&mut Self)) -> bool {
        let moved = self.apply_follow(follow);
        let again = self.apply_surroundings(around);
        #[cfg(windows)]
        self.collect_outcomes();
        if self.checking() { test(self); }
        self.decide(around);
        moved || again
    }

    /// Folds what the calibration thread found and the check so far into the verdict.
    fn decide(&mut self, around: Surroundings) {
        if self.beam.is_none() {
            self.beam = take_current(&self.calibration, self.generation);
            if let Some(Ok(beam)) = &self.beam {
                self.longest_idle_ns = beam.longest_idle_ns;
                eprintln!(
                    "nightjar: {}: scan {:.2} ms, {} rows behind {} lines of blanking, {:.1} lines per ms, {:.1} Hz at the top, \
                     {:.0}% of the readings in the blanking, idling counted from line {}, longest idle {:.1} ms, {} scans, {:.0} presents a second",
                    self.topic, beam.scan_ns as f64 / 1e6, beam.rows, beam.blank_rows, beam.lines_per_ms, beam.refresh_hz, beam.vblank_fraction * 100.0,
                    beam.idle_from, beam.longest_idle_ns as f64 / 1e6, beam.scans, self.rate.content_hz(around.content_hz)
                );
            }
        }
        if !self.check_said && !self.check.running() {
            self.check_said = true;
            eprintln!(
                "nightjar: {}: restart check over {} presents: {} tested, {} did not start a scan within {} ms; scan began after {}",
                self.topic, self.check.frames, self.check.tested, self.check.missed, super::scanline::RESTART_WINDOW_NS / 1_000_000, delays(&self.delays)
            );
        }
        // A display found to scan at its own rate stays so until the window moves to another one:
        // the V-Sync it gets then presents without tearing, which would otherwise become the reason.
        if self.fixed_refresh() { return; }
        let display_hz = match &self.beam { Some(Ok(beam)) => beam.refresh_hz, _ => 0.0 };
        self.verdict = decide(&Evidence {
            tearing: around.tearing,
            scanline: self.scanline_ok,
            fullscreen: around.fullscreen && !self.covered,
            beam: self.beam,
            check: self.check,
            content_hz: self.rate.content_hz(around.content_hz),
            display_hz,
            check_hz: self.check_hz,
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
        pub fn start(window: HWND, around: Surroundings, topic: &'static str) -> Self {
            let scan = Scanline::open(window, Instant::now());
            let mut probe = Self::new(scan.is_some(), topic);
            probe.apply_surroundings(around);
            probe.scan = scan;
            match &probe.scan {
                Some(scan) => {
                    eprintln!("nightjar: {topic}: reading the scan line of {} ({} rows)", scan.device_name(), scan.rows());
                    probe.helper = Helper::spawn(window, scan.clock());
                    if probe.helper.is_none() {
                        eprintln!("nightjar: {topic}: could not start the restart check's thread");
                        probe.scanline_ok = false;
                    }
                    probe.calibrate_in_background(window);
                }
                None => eprintln!("nightjar: {topic}: the scan line of the window's monitor cannot be read (D3DKMTGetScanLine)"),
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
                eprintln!("nightjar: {}: could not start the calibration: {error}", self.topic);
                self.scanline_ok = false;
            }
        }

        /// The beam just before a present is issued.
        pub fn before(&self) -> Option<Sample> {
            self.scan.as_ref().and_then(Scanline::sample)
        }

        /// Takes what the helper found out of its queue into the check.
        pub(super) fn collect_outcomes(&mut self) {
            let Some(helper) = &self.helper else { return };
            while let Ok(outcome) = helper.outcomes.try_recv() {
                // A result for the monitor the window has left says nothing about this one.
                if outcome.generation != self.generation { continue; }
                if self.check.running() {
                    if let Some(delay) = outcome.delay_ns {
                        self.delays.push(delay);
                        let mut sorted = self.delays.clone();
                        sorted.sort_unstable();
                        self.delay_ns = sorted[sorted.len() / 2];
                    }
                    self.check.push(outcome.restart);
                }
            }
        }

        /// After the present call returned: hands the present to the helper while the check runs,
        /// then folds the verdict. `before` is `before()`'s reading.
        pub fn after(&mut self, window: HWND, before: Option<Sample>, around: Surroundings) {
            self.presents += 1;
            let mut follow = Follow::Same;
            if self.presents % FOLLOW_EVERY == 0 {
                follow = self.scan.as_mut().map_or(Follow::Same, Scanline::follow_window);
                if follow == Follow::Lost { self.scan = None; }
            }
            if self.fold(around, follow, |probe| probe.test_present(before)) { self.calibrate_in_background(window); }
        }

        /// Hands a present the check takes to the helper when the display idled, or counts it
        /// untested.
        fn test_present(&mut self, before: Option<Sample>) {
            let (Some(Ok(beam)), Some(before), Some(scan)) = (self.beam, before, &self.scan) else { return };
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
                eprintln!("nightjar: {}: the restart check's thread stopped, its scan line cannot be read", self.topic);
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
}

#[cfg(not(windows))]
impl RaceProbe {
    /// The scan line is a Windows call: another platform has none to read.
    fn start_unavailable(around: Surroundings, topic: &'static str) -> Self {
        let mut probe = Self::new(false, topic);
        probe.apply_surroundings(around);
        probe.decide(around);
        probe
    }

    pub fn before(&self) -> Option<Sample> { None }

    fn after_unavailable(&mut self, around: Surroundings) {
        self.presents += 1;
        self.apply_surroundings(around);
        self.decide(around);
    }
}

/// The window and what it presents on, as the presenter sees them now.
fn surroundings(renderer: *mut SDL_Renderer, mode_tears: bool, content_hz: f64) -> (Surroundings, *mut SDL_Window) {
    // SAFETY: a live renderer and the window it draws to.
    let window = unsafe { SDL_GetRenderWindow(renderer) };
    // SAFETY: as above.
    let flags = (!window.is_null()).then(|| unsafe { SDL_GetWindowFlags(window) });
    let has = |flag| flags.is_some_and(|flags| flags & flag != 0);
    #[cfg(windows)]
    let chain_tears = super::super::media::swapchain_allows_tearing(crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER));
    #[cfg(not(windows))]
    let chain_tears = true;
    let surroundings = Surroundings {
        tearing: mode_tears && chain_tears,
        fullscreen: has(SDL_WINDOW_FULLSCREEN),
        focused: has(SDL_WINDOW_INPUT_FOCUS),
        content_hz,
    };
    (surroundings, window)
}

#[cfg(windows)]
fn hwnd_of(window: *mut SDL_Window) -> windows::Win32::Foundation::HWND {
    // SAFETY: a live window; the property is its HWND.
    let hwnd = unsafe { SDL_GetPointerProperty(SDL_GetWindowProperties(window), SDL_PROP_WINDOW_WIN32_HWND_POINTER, std::ptr::null_mut()) };
    windows::Win32::Foundation::HWND(hwnd)
}

impl RaceProbe {
    /// Starts the probe for the stream's renderer. `mode_tears`: the pacing mode presents with
    /// tearing (everything but V-Sync). `topic` starts its log lines.
    pub fn begin(renderer: *mut SDL_Renderer, mode_tears: bool, content_hz: f64, topic: &'static str) -> Self {
        let (around, window) = surroundings(renderer, mode_tears, content_hz);
        #[cfg(windows)]
        { Self::start(hwnd_of(window), around, topic) }
        #[cfg(not(windows))]
        { let _ = window; Self::start_unavailable(around, topic) }
    }

    /// After a present call returned. `before` is `before()`'s reading from just ahead of it.
    pub fn after_present(&mut self, renderer: *mut SDL_Renderer, before: Option<Sample>, mode_tears: bool, content_hz: f64) {
        self.rate.note(Instant::now());
        let (around, window) = surroundings(renderer, mode_tears, content_hz);
        #[cfg(windows)]
        self.after(hwnd_of(window), before, around);
        #[cfg(not(windows))]
        { let _ = (window, before); self.after_unavailable(around) }
    }
}

#[cfg(test)]
mod tests {
    use super::super::scanline::{Restart, CHECK_FRAMES, CHECK_MISSES, FIXED_FRAMES};
    use super::*;

    fn beam() -> Beam {
        Beam { rows: 1440, blank_rows: 200, total_rows: 1640, scan_ns: 4_100_000, refresh_hz: 243.9, lines_per_ms: 400.0, vblank_fraction: 0.75, scans: 50, idle_from: 1840, longest_idle_ns: 17_000_000 }
    }

    fn around() -> Surroundings { Surroundings { tearing: true, fullscreen: true, focused: true, content_hz: 60.0 } }

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
        let mut probe = RaceProbe::new(true, "beam race");
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
        let mut probe = RaceProbe::new(true, "beam race");
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
        let mut probe = RaceProbe::new(true, "beam race");
        *probe.calibration.lock().unwrap() = Some((0, Ok(beam())));
        probe.decide(around());
        assert!(!probe.apply_follow(Follow::Same));
        assert!(probe.beam.is_some());
        assert_eq!(probe.longest_idle_ns, 17_000_000);
    }

    #[test]
    fn the_rate_is_the_presents_measured_never_above_the_streams() {
        let start = Instant::now();
        let mut rate = PresentRate::default();
        let at = |ms: u64| start + std::time::Duration::from_millis(ms);
        for i in 0..=20 { rate.note(at(i * 50)); }
        assert_eq!(rate.content_hz(120.0), 20.0, "a still desktop the host repeats at 20 fps, on a 120 fps stream");
        assert_eq!(rate.content_hz(0.0), 20.0, "with the stream's rate unknown");
        let mut fast = PresentRate::default();
        for i in 0..=250 { fast.note(at(i * 4)); }
        assert_eq!(fast.content_hz(120.0), 120.0, "never above the stream's");
        assert_eq!(PresentRate::default().content_hz(60.0), 60.0, "the stream's until a second is measured");
    }

    #[test]
    fn a_fixed_refresh_stays_fixed_under_the_v_sync_it_gets_until_the_window_moves() {
        let mut probe = RaceProbe::new(true, "display check");
        *probe.calibration.lock().unwrap() = Some((0, Ok(Beam { vblank_fraction: 0.01, refresh_hz: 119.5, ..beam() })));
        probe.decide(around());
        assert!(!probe.fixed_refresh(), "the calibration alone decides nothing");
        for _ in 0..super::super::scanline::FIXED_FRAMES { probe.check.push(super::super::scanline::Restart::NotTested); }
        probe.decide(around());
        assert!(probe.fixed_refresh(), "{}", probe.line());
        assert!(probe.beam().is_some());
        // V-Sync presents do not tear: that is not the reason the display was found fixed.
        probe.decide(Surroundings { tearing: false, ..around() });
        assert!(probe.fixed_refresh(), "{}", probe.line());
        // Nor do the window or the rate change the display.
        assert!(!present(&mut probe, Surroundings { fullscreen: false, ..around() }, 1));
        assert!(!present(&mut probe, Surroundings { focused: false, ..around() }, 1));
        assert!(!present(&mut probe, Surroundings { content_hz: 20.0, ..around() }, 1));
        assert!(!present(&mut probe, around(), 1));
        assert!(probe.fixed_refresh(), "{}", probe.line());
        assert!(probe.apply_follow(Follow::Moved));
        assert!(!probe.fixed_refresh(), "another display is checked again");
        assert!(probe.beam().is_none());
    }

    /// Presents `n` pictures through `fold`, as `after` does on Windows, each with `result` when
    /// the check takes it (a calibrated display, as `test_present`). Returns whether the display
    /// was to be calibrated again.
    fn present_with(probe: &mut RaceProbe, around: Surroundings, n: u32, result: Restart) -> bool {
        let mut again = false;
        for _ in 0..n {
            again |= probe.fold(around, Follow::Same, |probe| if probe.beam().is_some() { probe.check.push(result) });
        }
        again
    }

    /// Presents that start no scan.
    fn present(probe: &mut RaceProbe, around: Surroundings, n: u32) -> bool { present_with(probe, around, n, Restart::NotTested) }

    #[test]
    fn what_a_composed_window_saw_does_not_decide_the_display_once_it_is_fullscreen() {
        // This PC's 240 Hz VRR panel behind a window, composed: little blanking, and no present
        // starts a scan.
        let windowed = Surroundings { fullscreen: false, ..around() };
        let mut probe = RaceProbe::new(true, "display check");
        assert!(!probe.apply_surroundings(windowed), "where the stream starts is not a change");
        *probe.calibration.lock().unwrap() = Some((0, Ok(Beam { vblank_fraction: 0.05, ..beam() })));
        assert!(!present(&mut probe, windowed, CHECK_FRAMES));
        assert_eq!(probe.check, RestartCheck::default(), "composed presents feed no check");
        assert_eq!(probe.line(), "off (not fullscreen)");
        // What the check had from those presents before: starved under a 60 fps stream.
        for _ in 0..CHECK_FRAMES { probe.check.push(Restart::NotTested); }
        let mut presented = false;
        assert!(probe.fold(around(), Follow::Same, |_| presented = true), "F11: the display is measured again");
        assert!(presented, "this present is the check's");
        assert_eq!(*probe.verdict(), Verdict::Checking, "not no VRR");
        assert_eq!(probe.check, RestartCheck::default());
        assert!(probe.beam().is_none(), "the composed calibration is dropped");
    }

    #[test]
    fn a_fullscreen_window_under_another_feeds_no_check_and_is_measured_again_with_the_focus() {
        let mut probe = RaceProbe::new(true, "display check");
        probe.apply_surroundings(around());
        *probe.calibration.lock().unwrap() = Some((0, Ok(Beam { vblank_fraction: 0.05, ..beam() })));
        assert!(!present(&mut probe, around(), 10));
        let frames = probe.check.frames;
        assert!(frames > 0);
        // Alt-Tab in borderless fullscreen: the window stays fullscreen under another, composed.
        let behind = Surroundings { focused: false, ..around() };
        assert!(!present(&mut probe, behind, CHECK_FRAMES));
        assert_eq!(probe.check.frames, frames, "composed presents feed no check");
        assert!(!probe.fixed_refresh(), "{}", probe.line());
        assert!(present(&mut probe, around(), 1), "the focus back: measured again");
        assert_eq!(probe.check, RestartCheck::default());
        // A fullscreen window that never had the focus (started from a script) is checked.
        let mut unfocused = RaceProbe::new(true, "display check");
        unfocused.apply_surroundings(behind);
        assert!(unfocused.checking());
    }

    #[test]
    fn a_check_at_a_busy_rate_is_measured_again_when_the_rate_falls_not_called_no_vrr() {
        // This PC's 240 Hz VRR panel calibrated while it scanned at its top rate.
        let vrr_busy = Beam { vblank_fraction: 0.12, ..beam() };
        let fast = Surroundings { content_hz: 200.0, ..around() };
        let still = Surroundings { content_hz: 20.0, ..around() };
        // The check starved at 200 fps: no idle at this rate.
        let mut probe = RaceProbe::new(true, "display check");
        probe.apply_surroundings(fast);
        *probe.calibration.lock().unwrap() = Some((0, Ok(vrr_busy)));
        assert!(!present(&mut probe, fast, CHECK_FRAMES + 1));
        assert_eq!(probe.line(), "off (no idle at this frame rate)");
        // A still screen the host repeats at 20 fps.
        assert!(present(&mut probe, still, 1), "measured again at the slower rate");
        assert!(!probe.fixed_refresh(), "{}", probe.line());
        assert_eq!(*probe.verdict(), Verdict::Checking);
        // Still running, past the presents the quick rule waits for, at 200 fps; then a menu at 60.
        let mut probe = RaceProbe::new(true, "display check");
        probe.apply_surroundings(fast);
        *probe.calibration.lock().unwrap() = Some((0, Ok(vrr_busy)));
        assert!(!present(&mut probe, fast, FIXED_FRAMES * 2));
        assert_eq!(*probe.verdict(), Verdict::Checking);
        assert!(present(&mut probe, around(), 1));
        assert!(!probe.fixed_refresh(), "{}", probe.line());
        // A display that is fixed after all is found at the slower rate.
        *probe.calibration.lock().unwrap() = Some((1, Ok(Beam { vblank_fraction: 0.01, ..beam() })));
        assert!(!present(&mut probe, around(), FIXED_FRAMES + 1));
        assert!(probe.fixed_refresh(), "{}", probe.line());
    }

    #[test]
    fn a_check_that_failed_at_a_busy_rate_is_measured_again_when_the_rate_falls() {
        // At 200 fps behind a deep queue the tested presents missed: the check failed, and at that
        // rate it says only that no restart was seen.
        let fast = Surroundings { content_hz: 200.0, ..around() };
        let mut probe = RaceProbe::new(true, "display check");
        probe.apply_surroundings(fast);
        *probe.calibration.lock().unwrap() = Some((0, Ok(Beam { vblank_fraction: 0.12, ..beam() })));
        assert!(!present_with(&mut probe, fast, CHECK_MISSES + 1, Restart::Missed));
        assert!(probe.check.failed());
        assert_eq!(probe.line(), "off (no VRR scan restart)");
        // A menu the host repeats at 20 fps: measured again, not no VRR.
        assert!(present(&mut probe, Surroundings { content_hz: 20.0, ..around() }, 1));
        assert!(!probe.fixed_refresh(), "{}", probe.line());
        assert_eq!(*probe.verdict(), Verdict::Checking);
        // A check that failed at a slow rate stays failed.
        let mut slow = RaceProbe::new(true, "display check");
        slow.apply_surroundings(around());
        *slow.calibration.lock().unwrap() = Some((0, Ok(beam())));
        assert!(!present_with(&mut slow, around(), CHECK_MISSES + 1, Restart::Missed));
        assert!(!present(&mut slow, Surroundings { content_hz: 20.0, ..around() }, 1));
        assert!(slow.check.failed());
    }

    #[test]
    fn a_rate_that_hovers_about_seventy_percent_does_not_restart_the_check() {
        let fast = Surroundings { content_hz: 200.0, ..around() };
        let mut probe = RaceProbe::new(true, "display check");
        probe.apply_surroundings(fast);
        *probe.calibration.lock().unwrap() = Some((0, Ok(Beam { vblank_fraction: 0.12, ..beam() })));
        assert!(!present(&mut probe, fast, CHECK_FRAMES + 1));
        assert_eq!(probe.line(), "off (no idle at this frame rate)");
        // 243.9 Hz: 70% is 170.7, 60% is 146.3.
        assert!(!present(&mut probe, Surroundings { content_hz: 160.0, ..around() }, 10), "just under 70%");
        assert_eq!(probe.line(), "off (no idle at this frame rate)");
        assert!(present(&mut probe, Surroundings { content_hz: 140.0, ..around() }, 1), "under 60%");
    }
}
