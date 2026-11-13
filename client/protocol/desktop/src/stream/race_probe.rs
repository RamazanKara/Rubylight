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

/// What a measuring thread leaves: the generation it measured for, and what it found.
type Slot<T> = Arc<Mutex<Option<(u32, T)>>>;

/// What the calibration thread leaves: the beam.
type Calibration = Slot<Result<Beam, CalibrationError>>;

/// The result if it is for `generation`. A result for an earlier one (the window moved to another
/// monitor while it ran) is dropped: it measured the wrong display.
fn take_current<T>(slot: &Slot<T>, generation: u32) -> Option<T> {
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
    /// The rate of the last `RECENT` presents, once there were that many.
    recent: Option<f64>,
    recent_since: Option<Instant>,
    recent_count: u32,
}

impl PresentRate {
    /// The presents `recent_hz` is taken over: a quarter of a second at 120 fps, so the check's
    /// quick rule (`scanline::FIXED_FRAMES`, half a second then) has seen them. A resume's first
    /// second read 52-77 presents, and half a second later the check had taken 60 at 120 a second,
    /// still judged at the old rate (nightjar#75).
    const RECENT: u32 = 30;

    fn note(&mut self, at: Instant) {
        let Some(since) = self.since else {
            self.since = Some(at);
            self.recent_since = Some(at);
            return;
        };
        self.count += 1;
        let elapsed = at.duration_since(since).as_secs_f64();
        if elapsed >= 1.0 {
            self.hz = Some(f64::from(self.count) / elapsed);
            (self.since, self.count) = (Some(at), 0);
        }
        self.recent_count += 1;
        if self.recent_count >= Self::RECENT {
            let elapsed = self.recent_since.map_or(0.0, |recent| at.duration_since(recent).as_secs_f64());
            if elapsed > 0.0 { self.recent = Some(f64::from(self.recent_count) / elapsed); }
            (self.recent_since, self.recent_count) = (Some(at), 0);
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

    /// The rate of the last presents, never above the stream's (`stream_hz`, 0 when not known), 0
    /// before there were enough. Too short to judge a display by: a game that swings or the burst
    /// after a stall reads high, so only `scanline::Evidence::recent_hz` reads it.
    fn recent_hz(&self, stream_hz: f64) -> f64 {
        match self.recent {
            Some(hz) if stream_hz > 0.0 => hz.min(stream_hz),
            Some(hz) => hz,
            None => 0.0,
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
    /// The lowest and highest presents a second measured while the check ran: the quick rule
    /// waits for a rate that settled (`scanline::STEADY`).
    check_rates: Option<(f64, f64)>,
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
    /// `recheck` asked for the display to be measured again at the next present.
    recheck_asked: bool,
    #[cfg(windows)]
    scan: Option<super::scanline::Scanline>,
    #[cfg(windows)]
    helper: Option<win::Helper>,
    /// Linux's display check: when to read DRM, the read on its thread, and the last reading.
    #[cfg(target_os = "linux")]
    settle: super::drm_vrr::Settle,
    #[cfg(target_os = "linux")]
    drm_slot: Slot<super::drm_vrr::Reading>,
    #[cfg(target_os = "linux")]
    drm: Option<super::drm_vrr::Reading>,
    /// None without DRM's display check (gamescope).
    #[cfg(target_os = "linux")]
    drm_reader: Option<DrmReader>,
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
            check_rates: None,
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
            recheck_asked: false,
            #[cfg(windows)]
            scan: None,
            #[cfg(windows)]
            helper: None,
            #[cfg(target_os = "linux")]
            settle: Default::default(),
            #[cfg(target_os = "linux")]
            drm_slot: Default::default(),
            #[cfg(target_os = "linux")]
            drm: None,
            #[cfg(target_os = "linux")]
            drm_reader: None,
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

    /// Measures the display again from the next present: a stream that a "no VRR" verdict turned
    /// to V-Sync looks again now and then (`present::VrrPacing::recheck_due`). The calibration and
    /// the scan line readings run off the render thread as at the start.
    pub fn recheck(&mut self) { self.recheck_asked = true; }

    /// What `recheck` asked for: drops what was measured, a "no VRR" verdict too. Returns whether
    /// the display has to be calibrated again.
    fn apply_recheck(&mut self) -> bool {
        if !std::mem::take(&mut self.recheck_asked) { return false; }
        eprintln!("nightjar: {}: measuring the display again", self.topic);
        self.forget();
        true
    }

    /// Drops the calibration, the check and the verdict, to measure the display again.
    fn forget(&mut self) {
        self.generation += 1;
        self.check = RestartCheck::default();
        self.check_hz = 0.0;
        self.check_rates = None;
        self.check_said = false;
        self.delays.clear();
        self.delay_ns = 0;
        self.longest_idle_ns = 0;
        self.beam = None;
        self.verdict = Verdict::Checking;
        #[cfg(target_os = "linux")]
        {
            self.drm = None;
            self.settle.restart();
        }
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
        if self.checking() {
            self.check_hz = self.check_hz.max(content_hz);
            if let Some(hz) = self.rate.hz {
                self.check_rates = Some(self.check_rates.map_or((hz, hz), |(low, high)| (low.min(hz), high.max(hz))));
            }
        }
        false
    }

    /// One present after its call returned: the window's move (`follow`), the window and the rate,
    /// the helper's results, the present itself while the check takes it (`test`), and the
    /// verdict. Returns whether the display has to be calibrated again.
    #[cfg(any(windows, test))]
    fn fold(&mut self, around: Surroundings, follow: Follow, test: impl FnOnce(&mut Self)) -> bool {
        let moved = self.apply_follow(follow);
        let rechecked = self.apply_recheck();
        let again = self.apply_surroundings(around);
        #[cfg(windows)]
        self.collect_outcomes();
        if self.checking() { test(self); }
        self.decide(around);
        moved || rechecked || again
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
        // A display found to scan at its own rate stays so until the window moves to another one or
        // a `recheck`: the V-Sync it gets presents without tearing, which would otherwise become the
        // reason (a recheck is given the tearing the stream had before it).
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
            check_rates: self.check_rates,
            check_hz: self.check_hz,
            recent_hz: self.rate.recent_hz(around.content_hz),
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
            if self.presents.is_multiple_of(FOLLOW_EVERY) {
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

/// The scan line is a Windows call: elsewhere there is none to read, and the verdict is "no
/// scanline", which keeps the pacing. On Linux the display check reads whether the window's
/// display runs VRR from DRM instead (`drm_vrr`): once the window has been fullscreen and focused
/// for a second, again when it becomes so after it was not, on a `recheck` and when it moves to
/// another display. Until a reading comes for a fullscreen, focused window the verdict is
/// "checking"; a windowed one keeps "no scanline", and so does a reading that cannot tell.
#[cfg(not(windows))]
impl RaceProbe {
    fn start_unavailable(around: Surroundings, topic: &'static str) -> Self {
        let mut probe = Self::new(false, topic);
        #[cfg(target_os = "linux")]
        { probe.drm_reader = drm_reader(topic, crate::handheld::gamescope()); }
        probe.apply_surroundings(around);
        probe.decide_unavailable(around);
        probe
    }

    pub fn before(&self) -> Option<Sample> { None }

    fn after_unavailable(&mut self, window: *mut SDL_Window, around: Surroundings) {
        #[cfg(target_os = "linux")]
        self.fold_unavailable(around, display_of(window), Instant::now(), describe_display);
        #[cfg(not(target_os = "linux"))]
        { let _ = window; self.fold_unavailable(around) }
    }

    /// One present: a recheck, the window, and on Linux when to read DRM (`display`: SDL's id of
    /// the window's display; `describe`: its name and mode, for the read), then the verdict.
    fn fold_unavailable(
        &mut self,
        around: Surroundings,
        #[cfg(target_os = "linux")] display: u32,
        #[cfg(target_os = "linux")] now: Instant,
        #[cfg(target_os = "linux")] describe: impl FnOnce(u32) -> super::drm_vrr::Display,
    ) {
        self.presents += 1;
        self.apply_recheck();
        self.apply_surroundings(around);
        #[cfg(target_os = "linux")]
        if let Some(reader) = self.drm_reader {
            match self.settle.present(around.fullscreen && around.focused, display, now) {
                super::drm_vrr::Step::Wait => {}
                super::drm_vrr::Step::Moved => {
                    eprintln!("nightjar: {}: the window moved to another display, reading it again", self.topic);
                    self.forget();
                }
                super::drm_vrr::Step::Read => self.read_drm_in_background(reader, describe(display)),
            }
        }
        self.decide_unavailable(around);
    }

    fn decide_unavailable(&mut self, around: Surroundings) {
        #[cfg(target_os = "linux")]
        if self.decide_drm(around) { return; }
        self.decide(around);
    }
}

/// What reads the window's output from DRM: `drm_vrr`'s, a fake one in tests.
#[cfg(target_os = "linux")]
type DrmReader = fn(&super::drm_vrr::Display) -> super::drm_vrr::Reading;

/// The DRM reader for a probe, none under gamescope. Neither Steam Deck panel has VRR, so gamescope
/// leaves VRR_ENABLED at 0, and "no VRR" would turn the Deck's immediate presents to V-Sync, a
/// latency change nobody has measured there: gamescope keeps the pacing it had.
#[cfg(target_os = "linux")]
fn drm_reader(topic: &str, gamescope: bool) -> Option<DrmReader> {
    if gamescope {
        eprintln!("nightjar: {topic}: gamescope paces the display, the pacing stays");
        return None;
    }
    eprintln!("nightjar: {topic}: no scan line here, reading VRR from DRM once the window is fullscreen and focused");
    let reader: DrmReader = |display| super::drm_vrr::Reading::of(super::drm_vrr::outputs(), display);
    Some(reader)
}

#[cfg(target_os = "linux")]
impl RaceProbe {
    /// Reads the outputs on a thread: the ioctls are quick, but none of it belongs on the render thread.
    fn read_drm_in_background(&mut self, reader: DrmReader, display: super::drm_vrr::Display) {
        let slot = self.drm_slot.clone();
        let generation = self.generation;
        let spawned = std::thread::Builder::new().name("drm-vrr".into()).spawn(move || {
            let reading = reader(&display);
            if let Ok(mut slot) = slot.lock() { *slot = Some((generation, reading)); }
        });
        if let Err(error) = spawned {
            if let Ok(mut slot) = self.drm_slot.lock() {
                *slot = Some((generation, super::drm_vrr::Reading::Unknown(format!("could not start the read: {error}"))));
            }
        }
    }

    /// Takes a reading for this display into the verdict. Returns false to leave the verdict to
    /// the scan line check, which has no scan line here: without DRM (gamescope), for a reading
    /// that cannot tell, and before the first reading while the window is not fullscreen and
    /// focused. That says "no scanline", or keeps a "no VRR" read before.
    fn decide_drm(&mut self, around: Surroundings) -> bool {
        if self.drm_reader.is_none() { return false; }
        if let Some(reading) = take_current(&self.drm_slot, self.generation) {
            eprintln!("nightjar: {}: {}", self.topic, reading.line());
            self.drm = Some(reading);
        }
        match self.drm {
            None if !(around.fullscreen && around.focused) => return false,
            Some(super::drm_vrr::Reading::Unknown(_)) => return false,
            _ => {}
        }
        self.verdict = super::drm_vrr::verdict(around.tearing, self.drm.as_ref());
        self.log_change();
        true
    }
}

/// SDL's id of the window's display, 0 when not known.
#[cfg(target_os = "linux")]
fn display_of(window: *mut SDL_Window) -> u32 {
    if window.is_null() { return 0; }
    // SAFETY: a live window.
    unsafe { SDL_GetDisplayForWindow(window) }.0
}

/// A display's name and current mode in pixels, for finding its output in DRM.
#[cfg(target_os = "linux")]
fn describe_display(id: u32) -> super::drm_vrr::Display {
    let id = SDL_DisplayID(id);
    // SAFETY: SDL owns the name and the mode, which are read here at once.
    unsafe {
        let name = SDL_GetDisplayName(id);
        let name = (!name.is_null()).then(|| std::ffi::CStr::from_ptr(name).to_string_lossy().into_owned()).filter(|name| !name.is_empty());
        match SDL_GetCurrentDisplayMode(id).as_ref() {
            Some(mode) => super::drm_vrr::Display {
                name,
                width: (mode.w as f32 * mode.pixel_density).round() as u32,
                height: (mode.h as f32 * mode.pixel_density).round() as u32,
                refresh_hz: f64::from(mode.refresh_rate),
            },
            None => super::drm_vrr::Display { name, width: 0, height: 0, refresh_hz: 0.0 },
        }
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
        { let _ = before; self.after_unavailable(window, around) }
    }
}

#[cfg(test)]
mod tests;
