//! When each picture goes to the screen: the pacing mode a stream gets, what VRR does with the display
//! check's verdict, presenting without waiting for a refresh, and the `Pacer` that holds decoded pictures
//! for their turn.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacingMode {
    HostTimes,
    Immediate,
    /// Immediate, with the slices of a frame presented ahead of the beam (B10 step 4). Never
    /// chosen by `pacing`: the stream switches to it when the display's probe allows.
    BeamRace,
    Vsync,
}

impl PacingMode {
    pub fn label(self) -> &'static str {
        match self {
            PacingMode::HostTimes => "VRR, PC frame times",
            PacingMode::Immediate => "Immediate",
            PacingMode::BeamRace => "Beam race",
            PacingMode::Vsync => "V-Sync",
        }
    }
}

pub fn pacing(settings: &crate::config::Settings, tearing_supported: bool) -> (PacingMode, bool) {
    let vrr = settings.vrr && tearing_supported;
    let mode = if vrr {
        // `experimental_packed_444` is the stream's pick here: the launch's settings hold "Sharper
        // colored text" as `full_chroma::decide` resolved it, packed full chroma or not.
        if settings.present_at_host_times && !(cfg!(windows) && settings.experimental_packed_444) {
            PacingMode::HostTimes
        } else {
            PacingMode::Immediate
        }
    } else {
        without_vrr(settings)
    };
    (mode, vrr)
}

/// The pacing without VRR: V-Sync, or immediate when the settings turn V-Sync off and the frame
/// pacing is the lowest latency (every other frame pacing waits for the refresh).
pub fn without_vrr(settings: &crate::config::Settings) -> PacingMode {
    if settings.vsync || settings.frame_pacing.forces_vsync() { PacingMode::Vsync } else { PacingMode::Immediate }
}

/// Whether a stream asks the GPU renderer for IMMEDIATE before MAILBOX when it presents without
/// V-Sync (`sdl::prefer_immediate`): MAILBOX never tears, so it would hold each picture for the
/// next refresh. moonlight-qt's Vulkan renderer (plvk.cpp) and chiaki-ng take IMMEDIATE with
/// V-Sync off. A VRR stream on a Wayland compositor keeps MAILBOX and gamescope takes IMMEDIATE,
/// as Nonary's moonlight-qt fork does (plvkpresentation.h).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn prefers_immediate(vrr: bool, wayland: bool, gamescope: bool) -> bool {
    !(vrr && wayland && !gamescope)
}

/// The longest a picture waits in the in-order frame pacings before a newer due one replaces it:
/// one frame of the stream, at most 250 ms (Android's `FramePacingPolicy.shouldDrainOutput`).
pub fn frame_us(fps: u32) -> u64 {
    (1_000_000 / u64::from(fps.max(1))).min(250_000)
}

/// The pacing once the display check (`race_probe`) has a verdict: a VRR stream on a display that
/// scans at its own rate whatever is presented tears on nearly every present (99% of them on a
/// 120 Hz laptop panel), so it takes the pacing it would have without VRR (`fallback`, from
/// `without_vrr`): V-Sync, or immediate when the settings turn V-Sync off. Anything else keeps its
/// mode: still checking, VRR seen, or a reason that says nothing about the refresh (a window, no
/// scan line, no restart seen under composition, a stream at the display's own rate).
pub fn after_display_check(mode: PacingMode, verdict: &super::super::scanline::Verdict, fallback: PacingMode) -> PacingMode {
    match mode {
        PacingMode::Immediate | PacingMode::HostTimes if verdict.fixed_refresh() => fallback,
        mode => mode,
    }
}

/// What a VRR stream's pacing does with the display check's verdict and `NIGHTJAR_VRR_SYNC`, a
/// step on each present (`Streaming::sync_vrr`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VrrPacing {
    /// The display check's verdict turns a fixed refresh display to `fallback`.
    pub check: bool,
    pub fallback: PacingMode,
    /// The check changed the pacing.
    pub fell_back: bool,
    pub sync: VrrSync,
    /// The sync interval the renderer was last given.
    pub interval: i32,
    /// The pacing the check turned from, which a recheck that sees the display refresh on present
    /// returns to.
    pub(super) restore: Option<PacingMode>,
    /// When the display is measured again while fallen back (µs on the session clock): None while
    /// a check runs, and for good once a stream that returned falls back again, so the schedule
    /// never flaps. A verdict the probe reaches by itself still returns the stream: on Linux a DRM
    /// reading after the window became fullscreen and focused again, one switch per such change.
    recheck_us: Option<u64>,
    recheck_every_us: u64,
    rechecks: bool,
    /// The stream returned from a fall back once.
    pub recovered: bool,
    /// The display check runs for the present mode alone (`stats::checks_display_for_present_mode`):
    /// its verdict changes no pacing.
    pub present_check: bool,
    /// The last verdict found the display at a fixed refresh, and none since found it refreshing
    /// on present.
    fixed_display: bool,
    /// The stream repeats the held picture for a VRR panel itself (`stream::lfc`), which reads the
    /// scan line too.
    pub lfc: bool,
}

/// The first recheck after a "no VRR" fall back, and the longest between two: each one doubles
/// the wait. A recheck costs the render thread a scan line reading per present for a second or two.
pub const RECHECK_FIRST_US: u64 = 10_000_000;
pub const RECHECK_MAX_US: u64 = 160_000_000;

impl VrrPacing {
    /// `mode`: the pacing the stream starts with, whose sync interval the renderer has.
    pub fn new(check: bool, fallback: PacingMode, sync: VrrSync, mode: PacingMode) -> Self {
        Self {
            check, fallback, fell_back: false, sync, interval: sync_interval(mode, VrrSync::Off),
            restore: None, recheck_us: None, recheck_every_us: RECHECK_FIRST_US, rechecks: true, recovered: false,
            present_check: false, fixed_display: false, lfc: false,
        }
    }

    /// Has the stream repeat pictures for the panel (`stream::lfc`).
    pub fn with_lfc(self, lfc: bool) -> Self {
        Self { lfc, ..self }
    }

    /// Runs the display check for the present mode where it changes no pacing.
    pub fn with_present_check(self, present_check: bool) -> Self {
        Self { present_check, ..self }
    }

    /// Whether the display, as far as the check knows, refreshes on present: false once it fell
    /// back or a verdict found a fixed refresh. The present mode follows it (`prefers_immediate`).
    pub fn display_vrr(&self) -> bool {
        !(self.fell_back || self.fixed_display)
    }

    /// Folds the display check's verdict (`verdict`, None before the probe runs) into `mode`, and
    /// returns the sync interval to give the renderer when it is not the last one given. It is
    /// given once: a renderer that refuses it refuses it every frame. A stream that fell back
    /// returns to its pacing once a recheck finds the display refreshing on present, and plans the
    /// next recheck when one ends without (`now_us`, `recheck_due`).
    pub fn step(&mut self, mode: &mut PacingMode, verdict: Option<&super::super::scanline::Verdict>, now_us: u64) -> Option<i32> {
        if let (true, Some(verdict)) = (self.check || self.present_check, verdict) {
            if verdict.fixed_refresh() {
                self.fixed_display = true;
            } else if verdict.refreshes_on_present() {
                self.fixed_display = false;
            }
        }
        if let (true, Some(verdict)) = (self.check, verdict) {
            let after = after_display_check(*mode, verdict, self.fallback);
            if after != *mode {
                self.restore = Some(*mode);
                *mode = after;
                self.fell_back = true;
                // Back once and no VRR again: the display stays on the fall back.
                self.rechecks &= !self.recovered;
                self.recheck_us = None;
            } else if let (true, Some(restore)) = (self.fell_back, self.restore) {
                if verdict.refreshes_on_present() {
                    *mode = restore;
                    (self.fell_back, self.recovered, self.recheck_us) = (false, true, None);
                } else if *verdict == super::super::scanline::Verdict::Checking {
                    // A recheck under way: the next is planned from its verdict.
                    self.recheck_us = None;
                } else if self.rechecks && self.recheck_us.is_none() {
                    self.recheck_us = Some(now_us + self.recheck_every_us);
                }
            }
        }
        let interval = sync_interval(*mode, self.sync);
        (interval != self.interval).then(|| {
            self.interval = interval;
            interval
        })
    }

    /// Whether the display check takes the presents as tearing ones: the pacing tears, or the
    /// check itself turned it to V-Sync and a recheck has to see what the stream had before.
    pub fn probe_tears(&self, mode: PacingMode) -> bool { mode != PacingMode::Vsync || self.fell_back }

    /// Whether the display check is to measure the display again now (`RaceProbe::recheck`): a
    /// stream on its fall back whose recheck time has come. The next one waits twice as long.
    pub fn recheck_due(&mut self, now_us: u64) -> bool {
        if !self.recheck_us.is_some_and(|at| now_us >= at) { return false; }
        self.recheck_us = None;
        self.recheck_every_us = (self.recheck_every_us * 2).min(RECHECK_MAX_US);
        true
    }
}

/// How a VRR stream presents while the display may still be scanning the last picture
/// (`NIGHTJAR_VRR_SYNC`, a measurement switch). A tearing present then shows the new picture from
/// the beam down: 6-20% of the presents of a 60 fps stream on a 240 Hz VRR panel did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VrrSync {
    /// Present on arrival, with tearing.
    #[default]
    Off,
    /// Sync interval 1: the flip is immediate on an idle display and waits out a scan under way.
    Present1,
    /// Wait out a scan under way on this thread (the beam from the scan line, at most one scan),
    /// then present with tearing.
    Scan,
    /// Tears only when the display is in its vertical blank and the present before it has left the
    /// queue, as read just before the present; any other present goes out with sync interval 1 and
    /// is shown whole after the scan under way.
    Blank,
}

impl VrrSync {
    /// `NIGHTJAR_VRR_SYNC`'s value; anything else is said on stderr and leaves it off.
    pub fn parse(text: Option<&str>) -> Self {
        match text.map(str::trim) {
            None | Some("" | "off" | "0") => Self::Off,
            Some("present1") => Self::Present1,
            Some("scan") => Self::Scan,
            Some("blank") => Self::Blank,
            Some(other) => {
                eprintln!("nightjar: NIGHTJAR_VRR_SYNC={other:?} is not present1, scan, blank or off, so it is off");
                Self::Off
            }
        }
    }

    /// A VRR stream's sync: what `NIGHTJAR_VRR_SYNC` says when it is set (`off` shows every frame
    /// at once), else Tear-free VRR (`vrr_tear_free`), which waits out a scan under way where the
    /// scan line can be read, on Windows.
    pub fn choose(env: Option<&str>, tear_free: bool, windows: bool) -> Self {
        match env.map(str::trim) {
            None | Some("") => if tear_free && windows { Self::Scan } else { Self::Off },
            text => Self::parse(text),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Present1 => "present1",
            Self::Scan => "scan",
            Self::Blank => "blank",
        }
    }
}

/// What the swap chain and the scan line said about the last present, read just before the next
/// one. Mirrors upstream `D3D11PresentPolicy::FlipObservation`.
#[derive(Clone, Copy, Debug, Default)]
pub struct FlipObservation {
    pub predecessor_known: bool,
    pub statistics_valid: bool,
    pub pending: bool,
    pub raster_valid: bool,
    pub in_vertical_blank: bool,
    pub raster_query_start_us: u64,
    pub raster_query_end_us: u64,
}

/// Whether a present is latched (sync interval 1). Literal port of upstream
/// `D3D11PresentPolicy::latch` from Nonary's moonlight-qt (GPL-3.0, commit 19758e5e6),
/// app/streaming/video/ffmpeg-renderers/d3d11presentpolicy.h, without its `synchronizeAll`
/// argument. A retired predecessor can still be scanning out. Unknown or active scanout must not
/// authorize a tearing flip. A fresh blank is a risk filter, not a reservation of the phase when a
/// later asynchronous flip reaches the panel.
pub fn latch(planned_latched: bool, protection_window_us: u64, observation: impl FnOnce() -> FlipObservation, clock: impl FnOnce() -> u64) -> bool {
    if planned_latched { return true; }
    if protection_window_us == 0 { return false; } // Explicitly disabled protection.
    let observation = observation();
    if !observation.predecessor_known || !observation.statistics_valid || observation.pending
        || !observation.raster_valid || !observation.in_vertical_blank { return true; }
    let now_us = clock();
    // Bound the entire observation age, including any preemption after the query. This
    // conservative budget does not claim a remaining blank time.
    let precision_us = (protection_window_us / 16).min(250);
    precision_us == 0 || observation.raster_query_start_us == 0
        || observation.raster_query_end_us < observation.raster_query_start_us
        || now_us < observation.raster_query_end_us
        || now_us - observation.raster_query_start_us > precision_us
}

/// Upstream `vrrFloorLatchGapUs` (vrrtimingcontroller.cpp, commit 19758e5e6).
pub const FLOOR_LATCH_GAP_US: u64 = 20_000;
/// Upstream `latchedPresentationHeadroomUs` default (vrrtimingcontroller.h, commit 19758e5e6).
pub const LATCH_HEADROOM_US: u64 = 225;

/// Whether the planned flip latches (sync interval 1), from the time since the last flip. Literal
/// port of upstream `vrrtimingcontroller.cpp` (commit 19758e5e6), where the latch is decided before
/// the present. A frame closer to the last flip than one display period plus headroom is latched:
/// the scan of the picture before cannot be over. After a gap of `FLOOR_LATCH_GAP_US` or more,
/// past the VRR range, the driver may be repeating the last picture, so the flip queue places this
/// one too.
pub fn planned_latch(since_last_flip_us: Option<u64>, display_period_us: u64) -> bool {
    match since_last_flip_us {
        None => false,
        Some(since) => since < display_period_us.saturating_add(LATCH_HEADROOM_US) || since >= FLOOR_LATCH_GAP_US,
    }
}

/// The flip history of a stream's presents, for `VrrSync::Blank`.
#[derive(Default)]
pub struct FlipGuard {
    /// When the last present was issued (us), whatever its kind.
    pub last_flip_us: Option<u64>,
    /// Presents that went out with tearing.
    pub tearing: u64,
    /// Presents latched by `planned_latch`.
    pub planned: u64,
    /// Presents latched by the observation.
    pub protected: u64,
}

/// The swap chain's sync interval for a pacing mode: 1 waits for the refresh, 0 presents at once
/// with tearing. Racing slices needs tearing whatever `sync` says. On macOS Real VRR presents
/// synchronised: a Metal layer shows a synchronised present on a display with a refresh range
/// when it is issued, and Nonary's moonlight-qt keeps `displaySyncEnabled` on under VRR for it
/// (GPL-3.0, commit 19758e5e6, vt_metal.mm).
pub fn sync_interval(mode: PacingMode, sync: VrrSync) -> i32 {
    match (mode, sync) {
        (PacingMode::Vsync, _) => 1,
        (PacingMode::HostTimes, _) if cfg!(target_os = "macos") => 1,
        (PacingMode::Immediate | PacingMode::HostTimes, VrrSync::Present1) => 1,
        _ => 0,
    }
}

/// How long a picture a V-Sync present refused tries again before it gives way to the next one.
/// A picture later than the desktop's deadline misses the refresh whatever happens, so it may as
/// well wait for its turn; skipping it would show the last picture twice. Earlier than that, the next
/// picture comes in time for the same refresh, so nothing is lost. A source faster than the stream
/// moves a phase-locked stream a picture at a time, 4.2 ms from a 240 Hz monitor, which at 120 Hz
/// lands up to 5 ms past a refresh: 3 ms repeated up to 1.9% there, 5 ms up to 0.65%.
pub const REFUSED_HOLD: std::time::Duration = std::time::Duration::from_millis(5);

/// Refused presents in a row before one waits after all. The display takes a picture every refresh,
/// so this many means the GPU is behind, and only a waiting present holds its work back.
pub(super) const REFUSED_IN_A_ROW: u32 = 4;

/// Refused presents among the last 64 that say the hold is too short here. A picture every refresh
/// is refused rarely, where one is late (0.5 to 1%). Too short a hold refuses far more: a stream
/// arriving later than the hold but before the desktop's deadline loses every other picture.
pub(super) const REFUSED_OF_64_WIDENS: u32 = 8;
/// How much longer the hold gets each time.
pub(super) const HOLD_STEP_US: u64 = 500;
/// Pictures between looks at a hold that grew. A bad spell on the network passes, and half a minute
/// apart the cost on a display that needs the longer hold is small.
pub(super) const PICTURES_SHORTEN: u32 = 4096;
/// Refused presents among those that let it get a step shorter: late pictures now and then (0.5 to
/// 1.6%) must not keep it long for good, where too short a hold refuses far more.
const REFUSED_SHORTEN: u32 = 64;

/// Refreshes that showed a picture again, among `MISSED_OF` pictures, before each picture waits its
/// turn (`SkippedPicture::missed`): 1.5%, two windows in a row. Once settled, a window at 120 Hz
/// missed at most 5 of 1024, and at 60 Hz 8, where borderless at 120 Hz missed 15 to 37. A spell on
/// Wi-Fi reached 17 and passed. The first `MISSED_OF` pictures don't count: everywhere they missed 11
/// to 23, and most of a run's misses are there.
pub(super) const MISSED_WAIT: u32 = 15;
pub(super) const MISSED_OF: u32 = 1024;

/// How long before a refresh the phase lock aims a stream with no refreshes to spare, ahead of its
/// own margin. A composed window's picture must be ready before the desktop's deadline, not the
/// refresh: 4.5 ms before it at 120 Hz and 5.2 ms at 60 Hz on the 780M laptop, and softer at 60 Hz, so
/// 5 ms at 120 Hz still repeated 0.25% and at 60 Hz 2.7%, where 7 ms repeated 0.2%. At most 3/5 of a
/// refresh: any later and a picture of the lock's whole range comes round behind the one before it.
pub fn deadline_before_refresh(refresh_hz: f32) -> i64 {
    if refresh_hz <= 0.0 { return 0; }
    let refresh_us = 1_000_000.0 / f64::from(refresh_hz);
    (3_000.0 + refresh_us / 4.0).min(refresh_us * 3.0 / 5.0) as i64
}

/// How far ahead of the refresh the phase lock aims: `set_us` (a measurement run's) or that deadline,
/// for a stream whose presents skip. A waiting present and exclusive fullscreen, which nothing
/// composes, keep the refresh itself.
pub fn phase_lock_aim_us(skipping: bool, exclusive: bool, set_us: Option<i64>, refresh_hz: f32) -> i64 {
    if !skipping || exclusive { return 0; }
    set_us.unwrap_or_else(|| deadline_before_refresh(refresh_hz))
}

/// Whether a stream of `frame_us` a frame on a display of `refresh_hz` skips a picture instead of
/// waiting behind the one before it, at V-Sync with a frame pacing that shows the newest picture.
///
/// A present waits while the last picture has not reached the display. With a picture every
/// refresh that wait never drains: each picture waits behind the last for good, and the wait holds
/// the D3D11 lock, so the decoder's next picture waits as well. 120 fps on a 120 Hz panel took 22 ms
/// from arrival to the display that way, and 7 ms when pictures that would wait are skipped. A
/// slower stream has refreshes to spare, so it never builds this up and keeps every picture. The
/// in-order frame pacings keep theirs too.
pub fn skips_instead_of_waiting(mode: PacingMode, frame_pacing: FramePacing, frame_us: u64, refresh_hz: f32) -> bool {
    mode == PacingMode::Vsync
        && matches!(frame_pacing, FramePacing::Latency | FramePacing::Balanced)
        && refresh_hz > 0.0
        && frame_us as f64 <= 1_020_000.0 / f64::from(refresh_hz)
}

/// What a present that never waits behind the last picture did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))] // Only Windows has this present.
pub enum NoWait {
    Shown,
    /// The last picture has not reached the display: this one was not presented, and the back
    /// buffer still holds it.
    Refused,
    /// Something else went wrong. SDL's own present recovers from what can be recovered from.
    Failed,
}

/// Presents at the next refresh unless the last picture still waits for one, trying again for
/// `hold`.
///
/// # Safety
/// `renderer` is the live D3D11 renderer of this thread.
#[cfg(windows)]
pub unsafe fn present_without_waiting(renderer: *mut SDL_Renderer, hold: std::time::Duration) -> NoWait {
    use windows::core::Interface;
    use windows::Win32::Graphics::Dxgi::{IDXGISwapChain, DXGI_ERROR_WAS_STILL_DRAWING, DXGI_PRESENT_DO_NOT_WAIT};
    let chain = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER);
    let Some(chain) = IDXGISwapChain::from_raw_borrowed(&chain) else { return NoWait::Failed };
    if !SDL_FlushRenderer(renderer) { return NoWait::Failed; }
    let begun = std::time::Instant::now();
    let result = loop {
        let result = chain.Present(1, DXGI_PRESENT_DO_NOT_WAIT);
        if result != DXGI_ERROR_WAS_STILL_DRAWING || begun.elapsed() >= hold { break result; }
        // It waits for a refresh anyway, and has until the next deadline: a sleep costs nothing.
        SDL_DelayNS(500_000);
    };
    // A present unbinds the back buffer's view: this makes SDL's next draw bind it again.
    SDL_FlushRenderer(renderer);
    if result == DXGI_ERROR_WAS_STILL_DRAWING {
        NoWait::Refused
    } else if result.is_ok() {
        NoWait::Shown
    } else {
        NoWait::Failed
    }
}

/// Only Windows has this present: SDL's own takes over elsewhere.
///
/// # Safety
/// Nothing to uphold here; the signature is the Windows one's.
#[cfg(not(windows))]
pub unsafe fn present_without_waiting(_renderer: *mut SDL_Renderer, _hold: std::time::Duration) -> NoWait { NoWait::Failed }

/// Presents with sync interval 1 when `latched` (the flip waits out a scan under way), else with
/// tearing. Returns whether the present succeeded.
///
/// # Safety
/// `renderer` is the live D3D11 renderer of this thread.
#[cfg(windows)]
pub unsafe fn present_adaptive(renderer: *mut SDL_Renderer, latched: bool) -> bool {
    use windows::core::Interface;
    use windows::Win32::Graphics::Dxgi::{IDXGISwapChain, DXGI_PRESENT, DXGI_PRESENT_ALLOW_TEARING};
    let chain = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER);
    let Some(chain) = IDXGISwapChain::from_raw_borrowed(&chain) else { return false };
    if !SDL_FlushRenderer(renderer) { return false; }
    let result = if latched { chain.Present(1, DXGI_PRESENT(0)) } else { chain.Present(0, DXGI_PRESENT_ALLOW_TEARING) };
    // A present unbinds the back buffer's view: this makes SDL's next draw bind it again.
    SDL_FlushRenderer(renderer);
    result.is_ok()
}

/// Only Windows has this present.
///
/// # Safety
/// Nothing to uphold here; the signature is the Windows one's.
#[cfg(not(windows))]
pub unsafe fn present_adaptive(_renderer: *mut SDL_Renderer, _latched: bool) -> bool { false }

/// The swap chain's flip history: `(predecessor_known, statistics_valid, pending)`. The predecessor
/// is the last present issued; statistics are valid when the last present to reach the display is
/// not after it, and pending when that present is before it.
///
/// # Safety
/// `renderer` is the live D3D11 renderer of this thread.
#[cfg(windows)]
pub unsafe fn flip_statistics(renderer: *mut SDL_Renderer) -> (bool, bool, bool) {
    use windows::core::Interface;
    use windows::Win32::Graphics::Dxgi::{IDXGISwapChain, DXGI_FRAME_STATISTICS};
    let chain = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER);
    let Some(chain) = IDXGISwapChain::from_raw_borrowed(&chain) else { return (false, false, false) };
    let last = chain.GetLastPresentCount();
    let mut stats = DXGI_FRAME_STATISTICS::default();
    let frames = chain.GetFrameStatistics(&mut stats);
    match (last, frames) {
        (Ok(last), Ok(())) => (true, stats.PresentCount <= last, stats.PresentCount < last),
        (Ok(_), Err(_)) => (true, false, false),
        (Err(_), _) => (false, false, false),
    }
}

/// Only Windows has this flip history.
///
/// # Safety
/// Nothing to uphold here; the signature is the Windows one's.
#[cfg(not(windows))]
pub unsafe fn flip_statistics(_renderer: *mut SDL_Renderer) -> (bool, bool, bool) { (false, false, false) }

/// A stream faster than the display is refused by design: to wait there is the queue again.
fn a_picture_a_refresh(frame_us: u64, refresh_us: u64) -> bool {
    frame_us * 10 >= refresh_us * 9
}

/// A picture skipped for one that was still waiting. The next picture takes its place, but a still
/// desktop may send none: then this one is drawn after all, a little later than the next would
/// have come, or the screen would stay a picture behind.
#[derive(Debug)]
pub struct SkippedPicture {
    pub(super) due_us: Option<u64>,
    in_a_row: u32,
    /// How long a refused present tries again.
    pub(super) hold_us: u64,
    /// What the hold starts at, and comes back to.
    base_us: u64,
    /// One bit for each of the last pictures, set where it was refused.
    pub(super) recent: u64,
    /// Pictures since the last look at the hold, and how many of them were refused.
    pub(super) pictures: u32,
    pub(super) refused: u32,
    /// Pictures toward `MISSED_OF`, and the misses among them.
    missed_of: u32,
    pub(super) missed: u32,
    /// Whether the last window reached `MISSED_WAIT`.
    missed_often: bool,
    /// Pictures until misses count, on a new stream or display.
    pub(super) settling: u32,
}

impl SkippedPicture {
    pub fn new(hold: std::time::Duration) -> Self {
        let hold_us = hold.as_micros() as u64;
        Self { due_us: None, in_a_row: 0, hold_us, base_us: hold_us, recent: 0, pictures: 0, refused: 0, missed_of: 0, missed: 0, missed_often: false, settling: MISSED_OF }
    }

    /// Another display or stream rate: what the hold grew to says nothing about its deadline.
    pub fn restart(&mut self) {
        self.hold_us = self.base_us;
        self.recent = 0;
        self.pictures = 0;
        self.refused = 0;
        self.missed_of = 0;
        self.missed = 0;
        self.missed_often = false;
        self.settling = MISSED_OF;
    }

    /// A refresh showed the last picture again although this one was on its way: it came too late
    /// for the desktop's deadline. Now and then that is the price of no queue in front of the
    /// display. Often, on a display whose deadline the stream keeps crossing, each picture waits its
    /// turn instead, a hold of a whole refresh: a picture behind, which never repeats one and still
    /// leaves the decoder alone. It comes back a step at a time, like any hold that grew. Returns
    /// the new hold when it grew.
    pub fn missed(&mut self, frame_us: u64, refresh_us: u64) -> Option<std::time::Duration> {
        if self.settling > 0 || !a_picture_a_refresh(frame_us, refresh_us) { return None; }
        self.missed += 1;
        if self.missed < MISSED_WAIT || !self.missed_often || self.hold_us > refresh_us { return None; }
        self.missed = 0;
        self.hold_us = refresh_us + HOLD_STEP_US;
        Some(self.hold())
    }

    /// One more picture toward the next look at the hold.
    pub(super) fn count(&mut self, refused: bool) {
        self.settling = self.settling.saturating_sub(1);
        self.missed_of += 1;
        if self.missed_of >= MISSED_OF {
            self.missed_of = 0;
            self.missed_often = self.missed >= MISSED_WAIT;
            self.missed = 0;
        }
        self.pictures += 1;
        self.refused += u32::from(refused);
        if self.pictures >= PICTURES_SHORTEN {
            if self.refused <= REFUSED_SHORTEN {
                self.hold_us = self.hold_us.saturating_sub(HOLD_STEP_US).max(self.base_us);
            }
            self.pictures = 0;
            self.refused = 0;
        }
    }

    pub fn hold(&self) -> std::time::Duration {
        std::time::Duration::from_micros(self.hold_us)
    }

    /// A picture was refused. Refused often on a stream with a picture a refresh, the hold grows
    /// until they stop: at a whole refresh it always gets its turn, like the present it replaces.
    /// A stream faster than the display is refused by design. Returns the new hold when it grew.
    pub fn skipped(&mut self, now_us: u64, frame_us: u64, refresh_us: u64) -> Option<std::time::Duration> {
        self.due_us = Some(now_us + frame_us + frame_us / 2);
        self.in_a_row += 1;
        self.count(true);
        self.recent = self.recent << 1 | 1;
        if self.recent.count_ones() < REFUSED_OF_64_WIDENS || !a_picture_a_refresh(frame_us, refresh_us) || self.hold_us > refresh_us { return None; }
        self.recent = 0;
        self.hold_us += HOLD_STEP_US;
        Some(self.hold())
    }

    /// A picture was presented: nothing is left to draw.
    pub fn shown(&mut self) {
        self.due_us = None;
        self.in_a_row = 0;
        self.recent <<= 1;
        self.count(false);
    }

    /// Whether the next present waits for its turn.
    pub fn must_wait(&self) -> bool {
        self.in_a_row >= REFUSED_IN_A_ROW
    }

    pub fn due_us(&self) -> Option<u64> {
        self.due_us
    }

    /// Whether the skipped picture is to be drawn now, once. That present waits for its turn: no
    /// later picture would take its place.
    pub fn take_due(&mut self, now_us: u64) -> bool {
        let due = self.due_us.is_some_and(|due| now_us >= due);
        if due {
            self.due_us = None;
            self.in_a_row = REFUSED_IN_A_ROW;
        }
        due
    }
}

/// Finds the refreshes that showed a picture again although the next one was already presented, from
/// the swap chain's statistics. A gap in the stream repeats a picture too, but then the next one came
/// late and reaches the display within a refresh.
#[derive(Debug, Default)]
pub struct MissWatch {
    /// The last presents: their count and when each was issued.
    issued: [(u32, u64); 4],
    pub(super) next: usize,
    /// The picture last seen on the display: its present count and refresh.
    pub(super) shown: Option<(u32, u32)>,
}

impl MissWatch {
    pub fn presented(&mut self, count: u32, issued_us: u64) {
        self.issued[self.next % self.issued.len()] = (count, issued_us);
        self.next += 1;
    }

    /// New statistics: whether they show a picture that missed its refresh.
    pub fn seen(&mut self, flip: crate::timing_csv::Flip, refresh_us: u64) -> bool {
        let (count, refresh) = (flip.stat_present_count, flip.stat_present_refresh);
        if count == 0 || refresh == 0 || flip.stat_sync_us == 0 { return false; }
        let Some((last_count, last_refresh)) = self.shown.replace((count, refresh)) else { return false };
        let pictures = count.wrapping_sub(last_count);
        if pictures == 0 || refresh.wrapping_sub(last_refresh) <= pictures { return false; }
        let Some(&(_, issued_us)) = self.issued.iter().find(|(issued, _)| *issued == count) else { return false };
        let behind = u64::from(flip.stat_sync_refresh.wrapping_sub(refresh)) * refresh_us;
        flip.stat_sync_us.saturating_sub(behind).saturating_sub(issued_us) > refresh_us
    }
}

/// The picture to draw now, and how many it replaced. A newer one that is due takes the place of the
/// one kept after a refused present; with none, the kept one is drawn once it is overdue.
pub fn take_over_kept<T>(due: Option<T>, kept: &mut Option<T>, overdue: bool) -> (Option<T>, u32) {
    match due {
        Some(newer) => (Some(newer), u32::from(kept.take().is_some())),
        None if overdue => (kept.take(), 0),
        None => (None, 0),
    }
}

/// A picture with its present time.
pub struct Pending {
    pub decoded: Decoded,
    pub present_us: u64,
    pub hold_us: i64,
    pub late: bool,
}

pub struct Pacer {
    pub mode: PacingMode,
    pub clock: PresentClock,
    /// Shared with the decoder thread (`Shared::pacer_backlog`): when every GPU plane set is
    /// held by a picture waiting here, the decoder drops the oldest of them instead of the
    /// newest frame.
    pub(super) pending: std::sync::Arc<std::sync::Mutex<VecDeque<Pending>>>,
    /// The frame pacing setting, which applies while the mode is V-Sync.
    pub(super) frame_pacing: FramePacing,
    /// Keep only the newest frame (`Settings::drop_late_frames`, Android's
    /// `checkbox_drop_late_frames`): off, Balanced shows every picture in turn as the in-order
    /// pacings do instead of the newest due one.
    pub(super) drop_late_frames: bool,
    /// One frame of the stream (`frame_us`).
    frame_us: u64,
    /// The FPS limit's last present slot.
    last_slot_us: Option<u64>,
    /// Frames waiting in the decode queue as the next picture is accepted.
    decode_queue: usize,
}

impl Pacer {
    /// `min_interval_us`: the display's shortest refresh interval.
    #[cfg(test)]
    pub fn new(mode: PacingMode, min_interval_us: i64) -> Self {
        Self::with_backlog(mode, min_interval_us, Default::default())
    }

    pub fn with_backlog(mode: PacingMode, min_interval_us: i64, pending: std::sync::Arc<std::sync::Mutex<VecDeque<Pending>>>) -> Self {
        Self {
            mode,
            // The clock only schedules in HostTimes, which needs a VRR display.
            clock: PresentClock::new(ClockSettings::adaptive(min_interval_us)),
            pending,
            frame_pacing: FramePacing::Latency,
            drop_late_frames: true,
            frame_us: frame_us(60),
            last_slot_us: None,
            decode_queue: 0,
        }
    }

    /// How many frames wait in the decode queue now. Once it stays non-empty frame after frame,
    /// the decoder is behind: PC's frame times show each picture as soon as it is ready and do
    /// not learn its lateness, which is the backlog's. A network burst is paced as usual
    /// (`PresentClock::schedule_queued`).
    pub fn set_decode_queue(&mut self, queued: usize) {
        self.decode_queue = queued;
    }

    /// The frame pacing setting and the stream's frame rate, for V-Sync.
    pub fn with_frame_pacing(mut self, frame_pacing: FramePacing, fps: u32) -> Self {
        self.frame_pacing = frame_pacing;
        self.frame_us = frame_us(fps);
        self
    }

    /// Keep only the newest frame, for Balanced frame pacing.
    pub fn with_drop_late_frames(mut self, drop_late_frames: bool) -> Self {
        self.drop_late_frames = drop_late_frames;
        self
    }

    /// Whether this stream skips a picture that would wait behind the last one, on this display.
    /// Balanced that keeps every picture keeps it as the in-order pacings do.
    pub fn skips_instead_of_waiting(&self, refresh_hz: f32) -> bool {
        !self.keeps_every_picture() && skips_instead_of_waiting(self.mode, self.frame_pacing, self.frame_us, refresh_hz)
    }

    /// Whether pictures are shown in turn, one a refresh, instead of the newest due one: the FPS
    /// limit, the smoothest pacing, and Balanced without Keep only the newest frame.
    fn keeps_every_picture(&self) -> bool {
        self.paced(FramePacing::CapFps) || self.paced(FramePacing::Smoothness) || (self.paced(FramePacing::Balanced) && !self.drop_late_frames)
    }

    /// One frame of the stream.
    pub fn frame_us(&self) -> u64 { self.frame_us }

    /// The stream's frame rate changed.
    pub fn set_fps(&mut self, fps: u32) { self.frame_us = frame_us(fps); }

    /// Whether the frame pacing setting shapes this stream's presents now.
    pub(super) fn paced(&self, pacing: FramePacing) -> bool {
        self.mode == PacingMode::Vsync && self.frame_pacing == pacing
    }

    /// The FPS limit's slot for a picture ready at `ready_us`: a frame after the last slot, but
    /// never before the picture is ready nor more than a frame after (a late picture starts the
    /// slots again), so it holds a picture at most one frame.
    fn cap_slot(&mut self, ready_us: u64) -> u64 {
        let slot = self.last_slot_us.map_or(ready_us, |last| (last + self.frame_us).clamp(ready_us, ready_us + self.frame_us));
        self.last_slot_us = Some(slot);
        slot
    }

    /// Takes newly decoded pictures, scheduling each once, in order.
    pub fn accept(&mut self, decoded: Decoded) {
        let (present_us, hold_us, late) = match self.mode {
            PacingMode::HostTimes => {
                let s = self.clock.schedule_queued(decoded.rtp, decoded.ready_us, self.decode_queue);
                (s.present_us, s.hold_us, s.late)
            }
            _ if self.paced(FramePacing::CapFps) => {
                let slot = self.cap_slot(decoded.ready_us);
                (slot, (slot - decoded.ready_us) as i64, false)
            }
            _ => (decoded.ready_us, 0, false),
        };
        super::super::pipeline::lock(&self.pending).push_back(Pending {
            decoded,
            present_us,
            hold_us,
            late,
        });
    }

    /// The raced frame number of the next picture and when it is due, if one is waiting.
    #[cfg(any(windows, test))]
    pub fn front_race(&self) -> Option<(u64, u64)> {
        super::super::pipeline::lock(&self.pending).front().map(|p| (p.decoded.race_seq, p.present_us))
    }

    /// When the next picture is due, if one is waiting.
    pub fn next_due(&self) -> Option<u64> {
        super::super::pipeline::lock(&self.pending).front().map(|p| p.present_us)
    }

    /// The newest picture due at `now_us`; older due ones are dropped (counted in the
    /// return's second value). With the FPS limit, the smoothest frame pacing or Balanced without
    /// Keep only the newest frame it is the oldest due picture instead, one a refresh: an older one
    /// is dropped only once it is more than a frame old and a newer one is due.
    pub fn take_due(&mut self, now_us: u64) -> (Option<Pending>, u32) {
        let mut chosen = None;
        let mut dropped = 0;
        let keeps_every_picture = self.keeps_every_picture();
        let mut pending = super::super::pipeline::lock(&self.pending);
        if keeps_every_picture {
            while pending.get(1).is_some_and(|next| next.present_us <= now_us)
                && pending.front().is_some_and(|p| now_us.saturating_sub(p.decoded.ready_us) > self.frame_us)
            {
                pending.pop_front();
                dropped += 1;
            }
            if pending.front().is_some_and(|p| p.present_us <= now_us) {
                chosen = pending.pop_front();
            }
            return (chosen, dropped);
        }
        while pending.front().is_some_and(|p| p.present_us <= now_us) {
            if chosen.is_some() {
                dropped += 1;
            }
            chosen = pending.pop_front();
        }
        (chosen, dropped)
    }
}
