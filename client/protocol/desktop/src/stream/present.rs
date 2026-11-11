//! Showing decoded pictures: the video texture, and when each picture goes to the screen.
//!
//! Four pacing modes:
//! - host times (VRR): each picture is shown at the PC's frame time plus a steady delay
//!   ([`PresentClock`]), so the display refreshes at the game's cadence instead of the
//!   network's. Presents are immediate (tearing allowed) at the scheduled time.
//! - immediate: shown as soon as decoded (lowest latency; tears without VRR).
//! - vsync: the newest picture at each refresh, or with the FPS limit and smoothest frame pacings
//!   (`config::FramePacing`) each picture in turn.
//! - beam race: immediate, and a picture still being received is shown slice by slice just ahead
//!   of the display's beam (`stream::race`); chosen once the display is known to allow it.
//!
//! The D3D11 path copies the decoder's surface into the renderer's NV12 texture on the GPU
//! (as moonlight-qt's d3d11va renderer does when it cannot bind decoder surfaces directly);
//! other paths upload planes.

use std::collections::VecDeque;
use std::ffi::c_void;

use rubylight_protocol::present_clock::{PresentClock, Settings as ClockSettings};
use sdl3_sys::everything::*;

use super::pipeline::Decoded;
use crate::config::FramePacing;
use crate::media::{self, FrameInfo};

/// How long the presenter waits for a GPU-decoded picture's fence before the stream ends. The
/// decoder thread has already waited for the decode, so this is only a bound on a broken device.
#[cfg(windows)]
const PLANES_DECODE_DEADLINE: std::time::Duration = std::time::Duration::from_millis(500);

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
pub fn after_display_check(mode: PacingMode, verdict: &super::scanline::Verdict, fallback: PacingMode) -> PacingMode {
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
    restore: Option<PacingMode>,
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
}

/// The first recheck after a "no VRR" fall back, and the longest between two: each one doubles
/// the wait. A recheck costs the render thread a scan line reading per present for a second or two.
pub const RECHECK_FIRST_US: u64 = 10_000_000;
pub const RECHECK_MAX_US: u64 = 160_000_000;

impl VrrPacing {
    /// `mode`: the pacing the stream starts with, whose sync interval the renderer has.
    pub fn new(check: bool, fallback: PacingMode, sync: VrrSync, mode: PacingMode) -> Self {
        Self {
            check, fallback, fell_back: false, sync, interval: i32::from(mode == PacingMode::Vsync),
            restore: None, recheck_us: None, recheck_every_us: RECHECK_FIRST_US, rechecks: true, recovered: false,
            present_check: false, fixed_display: false,
        }
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
    pub fn step(&mut self, mode: &mut PacingMode, verdict: Option<&super::scanline::Verdict>, now_us: u64) -> Option<i32> {
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
                } else if *verdict == super::scanline::Verdict::Checking {
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
}

impl VrrSync {
    /// `NIGHTJAR_VRR_SYNC`'s value; anything else is said on stderr and leaves it off.
    pub fn parse(text: Option<&str>) -> Self {
        match text.map(str::trim) {
            None | Some("" | "off" | "0") => Self::Off,
            Some("present1") => Self::Present1,
            Some("scan") => Self::Scan,
            Some(other) => {
                eprintln!("nightjar: NIGHTJAR_VRR_SYNC={other:?} is not present1, scan or off, so it is off");
                Self::Off
            }
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Present1 => "present1",
            Self::Scan => "scan",
        }
    }
}

/// The swap chain's sync interval for a pacing mode: 1 waits for the refresh, 0 presents at once
/// with tearing. Racing slices needs tearing whatever `sync` says.
pub fn sync_interval(mode: PacingMode, sync: VrrSync) -> i32 {
    match (mode, sync) {
        (PacingMode::Vsync, _) => 1,
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
const REFUSED_IN_A_ROW: u32 = 4;

/// Refused presents among the last 64 that say the hold is too short here. A picture every refresh
/// is refused rarely, where one is late (0.5 to 1%). Too short a hold refuses far more: a stream
/// arriving later than the hold but before the desktop's deadline loses every other picture.
const REFUSED_OF_64_WIDENS: u32 = 8;
/// How much longer the hold gets each time.
const HOLD_STEP_US: u64 = 500;
/// Pictures between looks at a hold that grew. A bad spell on the network passes, and half a minute
/// apart the cost on a display that needs the longer hold is small.
const PICTURES_SHORTEN: u32 = 4096;
/// Refused presents among those that let it get a step shorter: late pictures now and then (0.5 to
/// 1.6%) must not keep it long for good, where too short a hold refuses far more.
const REFUSED_SHORTEN: u32 = 64;

/// Refreshes that showed a picture again, among `MISSED_OF` pictures, before each picture waits its
/// turn (`SkippedPicture::missed`): 1.5%, two windows in a row. Once settled, a window at 120 Hz
/// missed at most 5 of 1024, and at 60 Hz 8, where borderless at 120 Hz missed 15 to 37. A spell on
/// Wi-Fi reached 17 and passed. The first `MISSED_OF` pictures don't count: everywhere they missed 11
/// to 23, and most of a run's misses are there.
const MISSED_WAIT: u32 = 15;
const MISSED_OF: u32 = 1024;

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

/// A stream faster than the display is refused by design: to wait there is the queue again.
fn a_picture_a_refresh(frame_us: u64, refresh_us: u64) -> bool {
    frame_us * 10 >= refresh_us * 9
}

/// A picture skipped for one that was still waiting. The next picture takes its place, but a still
/// desktop may send none: then this one is drawn after all, a little later than the next would
/// have come, or the screen would stay a picture behind.
#[derive(Debug)]
pub struct SkippedPicture {
    due_us: Option<u64>,
    in_a_row: u32,
    /// How long a refused present tries again.
    hold_us: u64,
    /// What the hold starts at, and comes back to.
    base_us: u64,
    /// One bit for each of the last pictures, set where it was refused.
    recent: u64,
    /// Pictures since the last look at the hold, and how many of them were refused.
    pictures: u32,
    refused: u32,
    /// Pictures toward `MISSED_OF`, and the misses among them.
    missed_of: u32,
    missed: u32,
    /// Whether the last window reached `MISSED_WAIT`.
    missed_often: bool,
    /// Pictures until misses count, on a new stream or display.
    settling: u32,
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
    fn count(&mut self, refused: bool) {
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
    next: usize,
    /// The picture last seen on the display: its present count and refresh.
    shown: Option<(u32, u32)>,
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
    pending: std::sync::Arc<std::sync::Mutex<VecDeque<Pending>>>,
    /// The frame pacing setting, which applies while the mode is V-Sync.
    frame_pacing: FramePacing,
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
            clock: PresentClock::new(ClockSettings {
                min_interval_us,
                ..Default::default()
            }),
            pending,
            frame_pacing: FramePacing::Latency,
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

    /// Whether this stream skips a picture that would wait behind the last one, on this display.
    pub fn skips_instead_of_waiting(&self, refresh_hz: f32) -> bool {
        skips_instead_of_waiting(self.mode, self.frame_pacing, self.frame_us, refresh_hz)
    }

    /// One frame of the stream.
    pub fn frame_us(&self) -> u64 { self.frame_us }

    /// The stream's frame rate changed.
    pub fn set_fps(&mut self, fps: u32) { self.frame_us = frame_us(fps); }

    /// Whether the frame pacing setting shapes this stream's presents now.
    fn paced(&self, pacing: FramePacing) -> bool {
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
        super::pipeline::lock(&self.pending).push_back(Pending {
            decoded,
            present_us,
            hold_us,
            late,
        });
    }

    /// The raced frame number of the next picture and when it is due, if one is waiting.
    #[cfg(any(windows, test))]
    pub fn front_race(&self) -> Option<(u64, u64)> {
        super::pipeline::lock(&self.pending).front().map(|p| (p.decoded.race_seq, p.present_us))
    }

    /// When the next picture is due, if one is waiting.
    pub fn next_due(&self) -> Option<u64> {
        super::pipeline::lock(&self.pending).front().map(|p| p.present_us)
    }

    /// The newest picture due at `now_us`; older due ones are dropped (counted in the
    /// return's second value). With the FPS limit or the smoothest frame pacing it is the oldest
    /// due picture instead, one a refresh: an older one is dropped only once it is more than a
    /// frame old and a newer one is due.
    pub fn take_due(&mut self, now_us: u64) -> (Option<Pending>, u32) {
        let mut chosen = None;
        let mut dropped = 0;
        let mut pending = super::pipeline::lock(&self.pending);
        if self.paced(FramePacing::CapFps) || self.paced(FramePacing::Smoothness) {
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

/// What holds the planes of the picture that waits for the back buffer.
#[cfg(windows)]
enum PlanesHold {
    /// A GPU decoder's plane set, once the decode fence reaches `fence_value`.
    Lease { lease: super::gpu_planes::Lease, fence_value: u64 },
    /// The planes the pass uploaded into its own textures (`PlanesPass::upload_frame`).
    Frame,
    /// A 4:4:4 decoder surface bound in the pass (`PlanesPass::bind_surface`): `held` is the
    /// decoder's frame while it is read in place, None for the pass's copy.
    Surface { held: Option<media::Frame> },
}

/// The current picture's planes, waiting for the back buffer or the video texture.
#[cfg(windows)]
struct DirectPlanes {
    source: PlanesHold,
    parameters: [u32; 8],
    /// The video texture holds the picture too (something read it).
    in_texture: bool,
    /// A PyroWave picture's draw was added to the stage times.
    timed: bool,
}

/// The renderer's texture holding the current picture.
pub struct VideoTexture {
    #[cfg(windows)]
    direct_packed: bool,
    pub texture: *mut SDL_Texture,
    width: i32,
    height: i32,
    /// The part of the current decoded picture that is the stream, without an encoder's alignment
    /// padding (`crop::visible_size`); None when the whole texture is the picture.
    visible: Option<(i32, i32)>,
    format: SDL_PixelFormat,
    colorspace: SDL_Colorspace,
    d3d11: *mut c_void,
    packed: Option<super::packed444::Reconstruction>,
    pyrowave: Option<super::pyrowave_gpu::Reconstruction>,
    pyrowave_format: Option<[u32; 8]>,
    /// PyroWave decoded on the GPU: converts the decoder's planes into the texture.
    #[cfg(windows)]
    planes: Option<super::d3d11::PlanesPass>,
    #[cfg(unix)]
    planes: Option<super::planes_gpu::PlanesPass>,
    /// Vulkan video pictures' planes, copied on the GPU, which `planes` draws (dropped after it).
    #[cfg(unix)]
    vulkan: Option<crate::vulkan_video::Planes>,
    /// The current picture's planes, copied for `planes` and not yet drawn, with the parameters to
    /// draw them with: `draw_direct` draws them into the back buffer, `sdl_texture` into the
    /// texture when something reads it (an upscaler, or SDL after a failed direct draw).
    #[cfg(unix)]
    planes_pending: Option<[u32; 8]>,
    /// A PyroWave picture's stage times wait for its deferred conversion (`passes_drawn`).
    #[cfg(unix)]
    pyrowave_stages_open: bool,
    /// Drawing the planes into the back buffer failed: they go through the texture for the rest
    /// of the stream.
    #[cfg(unix)]
    direct_planes_failed: bool,
    /// This frame is video only (`VideoOnly`): the plane pass writes PQ into the window's target.
    #[cfg(target_os = "linux")]
    pq_target: bool,
    #[cfg(target_os = "linux")]
    video_only: VideoOnly,
    /// Why the last `upload` failed, when it knows more than the caller can say.
    pub failure: Option<String>,
    /// The last failure written to the log, so a failing picture stream logs each reason once.
    logged_failure: Option<String>,
    /// The last picture of a GPU-decoded or planar stream carried the PQ transfer.
    hdr_frame: bool,
    /// The last picture carried the HLG transfer (ARIB STD-B67).
    hlg_frame: bool,
    /// Measurement: CPU time per PyroWave picture in `pyrowave_gpu`, kept across `destroy`.
    pub pyrowave_stages: super::stage_times::Stages<2>,
    packed_format: Option<[u32; 12]>,
    white_level: f32,
    /// The HDR headroom the texture was made with on its own (`ensure_with`), None for SDL's.
    texture_headroom: Option<f32>,
    hdr_mode: Option<bool>,
    hdr_metadata: Option<nightjar_core::control::HdrMetadata>,
    /// The session negotiated HDR (asked for it and got a 10-bit format), for pictures before or
    /// without the host's HDR mode; a 10-bit SDR stream is not HDR.
    hdr_stream: bool,
    /// The texture's planes are imported DMA-BUFs (`crate::dmabuf`): its NV12 shader reads the
    /// chroma plane as red-green, where uploaded chroma is luminance-alpha.
    #[cfg(unix)]
    imported: bool,
    #[cfg(unix)]
    importer: Option<crate::dmabuf::Importer>,
    /// The picture whose surface the imported texture samples, kept from the decoder until the
    /// next picture replaces it.
    #[cfg(unix)]
    held: Option<media::Frame>,
    /// A picture could not be imported: the stream's pictures are mapped to memory from then on.
    #[cfg(unix)]
    dmabuf_declined: bool,
    /// The pass that draws D3D11VA pictures into the back buffer, for the device it was made on.
    #[cfg(windows)]
    direct: Option<(usize, super::direct::Direct)>,
    /// The pass failed: SDL draws the pictures for the rest of the stream.
    #[cfg(windows)]
    direct_failed: bool,
    /// The decoder's picture, held instead of copied while the pass reads it in place.
    #[cfg(windows)]
    held: Option<media::Frame>,
    /// The video texture holds the current picture.
    #[cfg(windows)]
    copied: bool,
    /// The current picture's planes, not drawn into the video texture: `draw_direct` draws them into
    /// the back buffer, `materialize_planes` into the texture when something reads it.
    #[cfg(windows)]
    direct_planes: Option<DirectPlanes>,
    /// Drawing the planes into the back buffer failed: they go through the texture for the rest
    /// of the stream.
    #[cfg(windows)]
    direct_planes_failed: bool,
    /// Which way the stream's planes are drawn was logged.
    #[cfg(windows)]
    direct_planes_said: bool,
    /// The current picture came from D3D11VA, and its HDR10+ peak in nits.
    #[cfg(windows)]
    d3d11_picture: bool,
    #[cfg(windows)]
    hdr10_plus: Option<f32>,
    /// The current picture's Dolby Vision peak in nits (`media::Frame::dolby_vision`).
    #[cfg(windows)]
    dolby_vision: Option<f32>,
    /// A profile 5 (IPT) RPU was logged: its pictures are drawn as HDR10, once said.
    #[cfg(windows)]
    dolby_vision_ipt_said: bool,
    /// The current picture converted to RGB by the pass, for SDL (`sdl_texture`): 10-bit SDR as
    /// gamma-coded RGB, HLG as linear light (`converted_linear`). The view the pass writes it
    /// through, and whether it holds the current picture.
    #[cfg(windows)]
    converted: *mut SDL_Texture,
    #[cfg(windows)]
    converted_linear: bool,
    #[cfg(windows)]
    converted_view: Option<windows::Win32::Graphics::Direct3D11::ID3D11RenderTargetView>,
    #[cfg(windows)]
    converted_current: bool,
}

impl Default for VideoTexture {
    fn default() -> Self {
        Self {
            #[cfg(windows)]
            direct_packed: true,
            texture: std::ptr::null_mut(),
            width: 0,
            height: 0,
            visible: None,
            format: SDL_PIXELFORMAT_UNKNOWN,
            colorspace: SDL_COLORSPACE_UNKNOWN,
            d3d11: std::ptr::null_mut(),
            packed: None,
            pyrowave: None,
            pyrowave_format: None,
            #[cfg(any(windows, unix))]
            planes: None,
            #[cfg(unix)]
            vulkan: None,
            #[cfg(unix)]
            planes_pending: None,
            #[cfg(unix)]
            pyrowave_stages_open: false,
            #[cfg(unix)]
            direct_planes_failed: false,
            #[cfg(target_os = "linux")]
            pq_target: false,
            #[cfg(target_os = "linux")]
            video_only: VideoOnly::from_environment(),
            failure: None,
            logged_failure: None,
            hdr_frame: false,
            hlg_frame: false,
            pyrowave_stages: super::stage_times::Stages::new(super::pyrowave_gpu::UPLOAD_AND_PASSES),
            packed_format: None,
            white_level: 1.0,
            texture_headroom: None,
            hdr_mode: None,
            hdr_metadata: None,
            hdr_stream: false,
            #[cfg(unix)]
            imported: false,
            #[cfg(unix)]
            importer: None,
            #[cfg(unix)]
            held: None,
            #[cfg(unix)]
            dmabuf_declined: false,
            #[cfg(windows)]
            direct: None,
            #[cfg(windows)]
            direct_failed: false,
            #[cfg(windows)]
            held: None,
            #[cfg(windows)]
            copied: false,
            #[cfg(windows)]
            direct_planes: None,
            #[cfg(windows)]
            direct_planes_failed: false,
            #[cfg(windows)]
            direct_planes_said: false,
            #[cfg(windows)]
            d3d11_picture: false,
            #[cfg(windows)]
            hdr10_plus: None,
            #[cfg(windows)]
            dolby_vision: None,
            #[cfg(windows)]
            dolby_vision_ipt_said: false,
            #[cfg(windows)]
            converted: std::ptr::null_mut(),
            #[cfg(windows)]
            converted_linear: false,
            #[cfg(windows)]
            converted_view: None,
            #[cfg(windows)]
            converted_current: false,
        }
    }
}

/// Whether the session negotiated HDR: the client asked for it and the host chose a 10-bit format.
/// With "10-bit color (SDR)" a 10-bit format alone is SDR.
pub fn hdr_negotiated(asked_hdr: bool, negotiated_format: u32) -> bool {
    asked_hdr && negotiated_format & nightjar_core::sdp::VIDEO_FORMAT_MASK_10BIT != 0
}

/// FFmpeg's AVCOL_TRC_SMPTE2084 (PQ) and AVCOL_TRC_ARIB_STD_B67 (HLG).
const TRC_PQ: i32 = 16;
const TRC_HLG: i32 = 18;

/// Whether a picture is shown as HDR: as the host's HDR mode says once it has, else when it
/// carries the PQ or HLG transfer or the session negotiated HDR. A 10-bit format alone is not HDR.
fn shows_hdr(hdr_mode: Option<bool>, hdr_stream: bool, color_trc: i32) -> bool {
    hdr_mode.unwrap_or(hdr_stream || color_trc == TRC_PQ || color_trc == TRC_HLG)
}

/// The plane shader's constants for a software-decoded planar picture: its colour description is
/// FFmpeg's, read as `colorspace` reads it (the shader's matrix 0 is BT.601, 2 BT.2020, else BT.709).
/// An HDR picture with the HLG transfer gets the shader's HLG flag (128), never PQ, with the
/// display it is drawn for (`planar_hlg`) in place of the wavelet pass's level and operation,
/// and that display's white as the SDR white.
pub(super) fn planar_parameters(info: &FrameInfo, layout: media::PlanarLayout, hdr: bool, white_level: f32, headroom: f32) -> [u32; 8] {
    let matrix = match info.color_space { 5 | 6 => 0, 9 | 10 => 2, _ => 1 };
    let hlg = planar_hlg(info, hdr, white_level, headroom);
    let flags = u32::from(info.color_range == 2) | (u32::from(hdr) << 1) | (matrix << 2) | (u32::from(layout.ten_bit) << 4)
        | (u32::from(layout.chroma444) << 5) | (u32::from(layout.ten_bit) << 6) | (u32::from(hlg.is_some()) << 7);
    match hlg {
        Some(display) => [layout.width, layout.height, layout.width, layout.height, display.peak.to_bits(), display.gamma.to_bits(), flags, (display.white / 80.0).to_bits()],
        None => [layout.width, layout.height, layout.width, layout.height, 0, 3, flags, white_level.to_bits()],
    }
}

/// Whether a packed picture takes the other reconstruction than the one made: from planes for a
/// Vulkan picture (mapped VAAPI or Vulkan video, `vulkan_picture`) where the made one reads memory
/// (no `planes`), or the reverse, as when a new decoder's map fails.
#[cfg(any(unix, test))]
fn packed_path_changed(reconstruction: bool, vulkan_picture: bool, planes: bool) -> bool {
    reconstruction && vulkan_picture != planes
}

/// `planar_parameters` for Vulkan video's copied planes when `vulkan`: flag 256 (the chroma
/// interleaved, 10-bit samples in the high bits) in place of 64 (in the low bits).
pub(super) fn vulkan_flags(mut parameters: [u32; 8], vulkan: bool) -> [u32; 8] {
    if vulkan {
        parameters[6] = parameters[6] & !64 | 256;
    }
    parameters
}

/// `vulkan_flags` for the one copied image of a mapped 4:4:4 VAAPI picture when `interleaved`
/// (`media::FRAME_VULKAN_INTERLEAVED444`): flag 2048 in place of 256, Y, Cb and Cr read from one
/// texel, its 10-bit samples whole.
pub(super) fn interleaved_flags(mut parameters: [u32; 8], interleaved: bool) -> [u32; 8] {
    if interleaved {
        parameters[6] = parameters[6] & !256 | 2048;
    }
    parameters
}

/// The display a plane shader picture shown as HDR with the HLG transfer is drawn for, as the
/// Windows draw pass draws it (`hlg::hlg_display`): an HDR output's own peak (`headroom` over the
/// SDR white of `white_level` x 80 nits), else HLG's 1000-nit reference display with its
/// reference white at SDR white, which SDL's tone map brings into SDR. None for other pictures.
pub(super) fn planar_hlg(info: &FrameInfo, hdr: bool, white_level: f32, headroom: f32) -> Option<super::hlg::HlgDisplay> {
    (hdr && info.color_trc == TRC_HLG).then(|| super::hlg::hlg_display(headroom, 80.0 * white_level))
}

/// Whether a picture SDL would show (NV12 or P010 from memory, or an imported VAAPI surface) goes
/// to the plane shader instead: SDL reads P010 as PQ and knows no HLG, so an HLG one is drawn as a
/// 4:2:0 planar picture with the shader's HLG flag. SDR and PQ pictures keep SDL's upload or
/// import as they are.
#[cfg(unix)]
pub(super) fn hlg_through_planes(info: &FrameInfo) -> bool {
    info.color_trc == TRC_HLG && matches!(info.kind, media::FRAME_NV12 | media::FRAME_P010 | media::FRAME_DMABUF)
}

/// FFmpeg's colour description as an SDL colorspace.
fn colorspace(info: &FrameInfo) -> SDL_Colorspace {
    // AVCOL_RANGE_JPEG = 2; AVCOL_SPC_BT709 = 1, BT470BG = 5, SMPTE170M = 6, BT2020_NCL = 9;
    // AVCOL_TRC_SMPTE2084 = 16.
    let full = info.color_range == 2;
    // A PQ picture is YCbCr BT.2020 (SDL's BT2020_* spaces carry the PQ transfer). SDL_COLORSPACE_HDR10
    // is an RGB space with the identity matrix: SDL's D3D11 renderer cannot make a P010 texture in it
    // ("Unsupported YUV colorspace"), which showed as "Cannot show d3d11 pictures".
    if info.color_trc == 16 {
        return if full { SDL_COLORSPACE_BT2020_FULL } else { SDL_COLORSPACE_BT2020_LIMITED };
    }
    match (info.color_space, full) {
        (9 | 10, false) => SDL_COLORSPACE_BT2020_LIMITED,
        (9 | 10, true) => SDL_COLORSPACE_BT2020_FULL,
        (5 | 6, false) => SDL_COLORSPACE_BT601_LIMITED,
        (5 | 6, true) => SDL_COLORSPACE_BT601_FULL,
        (_, true) => SDL_COLORSPACE_BT709_FULL,
        _ => SDL_COLORSPACE_BT709_LIMITED,
    }
}

/// The YCbCr matrix (0 BT.601, 1 BT.709, 2 BT.2020) and full range of an SDL colorspace from
/// `colorspace`, for `stream::direct`.
#[cfg(windows)]
fn direct_matrix(space: SDL_Colorspace) -> (u32, bool) {
    match space {
        SDL_COLORSPACE_BT601_LIMITED => (0, false),
        SDL_COLORSPACE_BT601_FULL => (0, true),
        SDL_COLORSPACE_BT2020_LIMITED => (2, false),
        SDL_COLORSPACE_BT2020_FULL => (2, true),
        SDL_COLORSPACE_BT709_FULL => (1, true),
        _ => (1, false),
    }
}

/// A decoded picture's transfer, one decision for every path that shows it. When the stream
/// shows HDR (`shows_hdr`: the host's HDR mode, else the PQ or HLG transfer or the negotiated
/// HDR), a picture with the HLG transfer is HLG and any other P010 picture PQ, even when the
/// bitstream does not signal PQ. Everything else is SDR: an NV12 picture is never PQ.
#[cfg(windows)]
fn picture_transfer(format: SDL_PixelFormat, hdr_mode: Option<bool>, hdr_stream: bool, hdr_frame: bool, hlg_frame: bool) -> super::direct::Transfer {
    use super::direct::Transfer;
    let trc = if hlg_frame { TRC_HLG } else if hdr_frame { TRC_PQ } else { 0 };
    if !matches!(format, SDL_PIXELFORMAT_P010 | SDL_PIXELFORMAT_NV12) || !shows_hdr(hdr_mode, hdr_stream, trc) {
        Transfer::Sdr
    } else if hlg_frame {
        Transfer::Hlg
    } else if format == SDL_PIXELFORMAT_P010 {
        Transfer::Pq
    } else {
        Transfer::Sdr
    }
}

/// SDL's D3D11 shaders decode every P010 texture as PQ and NV12 as gamma, and know no HLG: a
/// picture whose `picture_transfer` differs reaches SDL converted to RGB by the pass.
#[cfg(windows)]
fn sdl_needs_rgb(format: SDL_PixelFormat, transfer: super::direct::Transfer) -> bool {
    use super::direct::Transfer;
    let sdl_reads = if format == SDL_PIXELFORMAT_P010 { Transfer::Pq } else { Transfer::Sdr };
    sdl_reads != transfer
}

/// The pass for the renderer's device, made on first use or after the device changed.
#[cfg(windows)]
fn direct_pass(direct: &mut Option<(usize, super::direct::Direct)>, renderer: *mut SDL_Renderer) -> Result<&mut super::direct::Direct, String> {
    let device = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_DEVICE_POINTER);
    if !direct.as_ref().is_some_and(|(made_on, _)| *made_on == device as usize) {
        *direct = None;
        // SAFETY: The renderer's live device, used on the presenting thread.
        let made = unsafe {
            use windows::core::Interface;
            let device = windows::Win32::Graphics::Direct3D11::ID3D11Device::from_raw_borrowed(&device)
                .ok_or("the renderer has no Direct3D 11 device")?;
            super::direct::Direct::new(device)?
        };
        *direct = Some((device as usize, made));
    }
    Ok(&mut direct.as_mut().unwrap().1)
}

/// Runs a submission that reads `texture` under the decoder's lock when it is the held
/// decoder picture's.
#[cfg(windows)]
fn lock_for(held: &Option<media::Frame>, texture: *mut c_void, submit: &mut dyn FnMut()) {
    match held {
        Some(frame) if frame.d3d11_surface().is_some_and(|surface| surface.texture == texture) => frame.with_decoder_lock(submit),
        _ => submit(),
    }
}

/// The HDR properties of a texture request.
#[derive(Clone, Copy, Debug, PartialEq)]
struct HdrProperties {
    /// `SDL_PROP_TEXTURE_CREATE_SDR_WHITE_POINT_FLOAT`, in nits (HDR10 textures only).
    white_point: Option<f32>,
    /// `SDL_PROP_TEXTURE_CREATE_HDR_HEADROOM_FLOAT`, relative to the SDR white.
    headroom: Option<f32>,
}

/// The white point and headroom a texture is created with. `white_level` is the renderer's SDR
/// white as a multiple of 80 nits; a value that is not a positive finite number is not passed on.
fn hdr_properties(hdr10: bool, white_level: f32, metadata: Option<nightjar_core::control::HdrMetadata>) -> HdrProperties {
    let white_nits = 80.0 * white_level;
    if !white_nits.is_finite() || white_nits <= 0.0 {
        return HdrProperties { white_point: None, headroom: None };
    }
    let headroom = metadata.and_then(|metadata| {
        let peak = if metadata.max_content_light_level != 0 {
            metadata.max_content_light_level
        } else {
            metadata.max_display_luminance
        };
        (peak > 0).then(|| f32::from(peak) / white_nits)
    });
    HdrProperties { white_point: hdr10.then_some(white_nits), headroom }
}

fn pixel_format_name(format: SDL_PixelFormat) -> String {
    let name = SDL_GetPixelFormatName(format);
    if name.is_null() {
        return format!("format {:#x}", format.0);
    }
    // SAFETY: checked non-null above.
    unsafe { std::ffi::CStr::from_ptr(name) }.to_string_lossy().into_owned()
}

impl VideoTexture {
    /// `hdr_stream`: the session negotiated HDR.
    pub fn new(_direct_packed: bool, hdr_stream: bool) -> Self {
        Self { #[cfg(windows)] direct_packed: _direct_packed, hdr_stream, ..Default::default() }
    }

    pub fn set_hdr_mode(
        &mut self,
        enabled: bool,
        metadata: Option<nightjar_core::control::HdrMetadata>,
    ) {
        if self.hdr_mode != Some(enabled) || self.hdr_metadata != metadata {
            self.destroy();
            self.hdr_mode = Some(enabled);
            self.hdr_metadata = metadata;
        }
    }

    /// Records why a picture cannot be shown: `short` for the caller's message, `detail` once in
    /// the log (a failing stream retries every picture).
    fn fail(&mut self, detail: String, short: String) {
        if self.logged_failure.as_deref() != Some(detail.as_str()) {
            eprintln!("nightjar: {detail}");
            self.logged_failure = Some(detail);
        }
        self.failure = Some(short);
    }

    pub fn is_hdr(&self) -> bool {
        self.hdr_mode.unwrap_or(
            self.hdr_frame
                || self.hlg_frame
                || self
                    .packed_format
                    .is_some_and(|parameters| parameters[7] != 0),
        )
    }

    /// The picture can only be shown on an HDR output. An HLG picture can be shown on an SDR one
    /// too: the draw pass and the plane shader draw it for HLG's reference display, tone mapped.
    pub fn needs_hdr_output(&self) -> bool {
        self.is_hdr() && !self.hlg_frame
    }

    /// The host's HDR mastering metadata while its HDR mode is on.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn hdr_metadata(&self) -> Option<nightjar_core::control::HdrMetadata> {
        // HLG carries no mastering metadata and keeps today's path.
        self.hdr_metadata.filter(|_| self.hdr_mode == Some(true) && !self.hlg_frame)
    }

    /// The picture's size: the texture's, less the alignment padding of a decoded picture.
    pub fn size(&self) -> (i32, i32) {
        self.visible.unwrap_or((self.width, self.height))
    }

    /// The part of `texture` that holds the picture when it is the video texture with alignment
    /// padding, for SDL to draw; None for the whole texture. It ends half a texel inside the
    /// picture, so bilinear sampling at the edge never reads the first padding texel.
    pub fn source_rect(&self, texture: *mut SDL_Texture) -> Option<SDL_FRect> {
        let (w, h) = self.size();
        (!texture.is_null() && texture == self.texture && (w, h) != (self.width, self.height))
            .then_some(SDL_FRect { x: 0.0, y: 0.0, w: w as f32 - 0.5, h: h as f32 - 0.5 })
    }

    /// The picture AI enhancement can read: a hardware-decoded NV12 or P010 Direct3D 11 texture.
    #[cfg(windows)]
    pub fn enhance_source(&self) -> Option<super::enhance::Source> {
        let ten_bit = match self.format {
            SDL_PIXELFORMAT_NV12 => false,
            SDL_PIXELFORMAT_P010 => true,
            _ => return None,
        };
        (!self.texture.is_null() && !self.d3d11.is_null()).then_some(super::enhance::Source {
            texture: self.texture, d3d11: self.d3d11, picture: self.size(), ten_bit, colorspace: self.colorspace, transfer: self.transfer(),
        })
    }

    fn ensure(
        &mut self,
        renderer: *mut SDL_Renderer,
        format: SDL_PixelFormat,
        width: i32,
        height: i32,
        space: SDL_Colorspace,
        white_level: f32,
    ) -> bool {
        self.ensure_with(renderer, format, width, height, space, white_level, None, std::ptr::null_mut())
    }

    /// `ensure` for a texture with an HDR headroom of its own (an HLG picture's), which SDL tone
    /// maps down to an output with less.
    #[allow(clippy::too_many_arguments)]
    fn ensure_with(
        &mut self,
        renderer: *mut SDL_Renderer,
        format: SDL_PixelFormat,
        width: i32,
        height: i32,
        space: SDL_Colorspace,
        white_level: f32,
        texture_headroom: Option<f32>,
        pixel_buffer: *mut c_void,
    ) -> bool {
        // Each VideoToolbox picture has its own backing. A later CPU upload must also
        // replace an imported texture before writing to it.
        #[cfg(target_os = "macos")]
        let reusable = pixel_buffer.is_null() && self.held.is_none();
        #[cfg(not(target_os = "macos"))]
        let reusable = pixel_buffer.is_null();
        if reusable && !self.texture.is_null()
            && self.format == format
            && self.width == width
            && self.height == height
            && self.colorspace == space
            && self.white_level == white_level
            && self.texture_headroom == texture_headroom
        {
            return true;
        }
        self.destroy();
        // SAFETY: SDL property and texture calls on a live renderer.
        unsafe {
            let props = SDL_CreateProperties();
            SDL_SetNumberProperty(
                props,
                SDL_PROP_TEXTURE_CREATE_FORMAT_NUMBER,
                i64::from(format.0),
            );
            SDL_SetNumberProperty(
                props,
                SDL_PROP_TEXTURE_CREATE_ACCESS_NUMBER,
                i64::from(if format == SDL_PIXELFORMAT_RGBA64_FLOAT {
                    SDL_TEXTUREACCESS_TARGET.0
                } else {
                    SDL_TEXTUREACCESS_STATIC.0
                }),
            );
            SDL_SetNumberProperty(
                props,
                SDL_PROP_TEXTURE_CREATE_WIDTH_NUMBER,
                i64::from(width),
            );
            SDL_SetNumberProperty(
                props,
                SDL_PROP_TEXTURE_CREATE_HEIGHT_NUMBER,
                i64::from(height),
            );
            SDL_SetNumberProperty(
                props,
                SDL_PROP_TEXTURE_CREATE_COLORSPACE_NUMBER,
                i64::from(space.0),
            );
            let metadata = self.hdr_metadata.filter(|_| self.hdr_mode == Some(true));
            // SRGB_LINEAR is the float target of the PyroWave and packed paths, never a PQ picture.
            let hdr = hdr_properties(self.hdr_frame && space != SDL_COLORSPACE_SRGB_LINEAR, white_level, metadata);
            if let Some(white_point) = hdr.white_point {
                SDL_SetFloatProperty(props, SDL_PROP_TEXTURE_CREATE_SDR_WHITE_POINT_FLOAT, white_point);
            }
            if let Some(headroom) = texture_headroom.or(hdr.headroom) {
                SDL_SetFloatProperty(props, SDL_PROP_TEXTURE_CREATE_HDR_HEADROOM_FLOAT, headroom);
            }
            if !pixel_buffer.is_null() {
                SDL_SetPointerProperty(props, SDL_PROP_TEXTURE_CREATE_METAL_PIXELBUFFER_POINTER, pixel_buffer);
            }
            self.texture = SDL_CreateTextureWithProperties(renderer, props);
            SDL_DestroyProperties(props);
            if self.texture.is_null() {
                let error = crate::sdl::error();
                let detail = format!(
                    "SDL could not create the {} video texture ({width}x{height}, colorspace {:#x}, SDR white point {:?}, HDR headroom {:?}, renderer {}): {error}",
                    pixel_format_name(format),
                    space.0,
                    hdr.white_point,
                    texture_headroom.or(hdr.headroom),
                    crate::sdl::renderer_name(renderer),
                );
                self.fail(detail, error);
                return false;
            }
            SDL_SetTextureScaleMode(self.texture, SDL_SCALEMODE_LINEAR);
            self.d3d11 = SDL_GetPointerProperty(
                SDL_GetTextureProperties(self.texture),
                SDL_PROP_TEXTURE_D3D11_TEXTURE_POINTER,
                std::ptr::null_mut(),
            );
        }
        self.format = format;
        self.width = width;
        self.height = height;
        self.colorspace = space;
        self.white_level = white_level;
        self.texture_headroom = texture_headroom;
        true
    }

    /// Puts a decoded picture into the texture; `rows` are the only rows of a GPU-decoded picture
    /// to draw when the rows above them are there already (the raced presenter). A decoded H.264,
    /// HEVC or AV1 picture is shown without its alignment padding (`Decoded::visible`): the texture keeps
    /// the whole picture and what draws it samples only the stream's part, so nothing is copied.
    pub fn upload(&mut self, renderer: *mut SDL_Renderer, decoded: &Decoded, rows: Option<(u32, u32)>) -> bool {
        // Planes copied for the last picture are not this one's, unless this one fails: then the
        // last picture stays on screen, from its planes while the pass and texture that hold them
        // do (the texture was never drawn into).
        #[cfg(unix)]
        let last_planes = self.planes_pending.take();
        let uploaded = self.upload_picture(renderer, decoded, rows);
        #[cfg(unix)]
        { self.planes_pending = planes_after_upload(uploaded, self.planes_pending, last_planes, self.planes.is_some() && !self.texture.is_null(), self.pyrowave_format); }
        self.visible = decoded.visible.filter(|_| uploaded && plain(decoded)).and_then(|visible| visible_part(visible, (self.width, self.height)));
        uploaded
    }

    fn upload_picture(&mut self, renderer: *mut SDL_Renderer, decoded: &Decoded, rows: Option<(u32, u32)>) -> bool {
        // A picture whose conversion never ran is not counted.
        #[cfg(unix)]
        if std::mem::take(&mut self.pyrowave_stages_open) { self.pyrowave_stages.reset(); }
        #[cfg(windows)]
        if let media::Picture::GpuPlanes { lease, fence_value } = &decoded.picture {
            return self.upload_planes(renderer, decoded, lease, *fence_value, rows);
        }
        #[cfg(not(windows))]
        let _ = rows;
        #[cfg(unix)]
        if let media::Picture::GpuPlanes { lease } = &decoded.picture {
            return self.upload_planes(renderer, decoded, lease);
        }
        let Some(frame) = decoded.picture.frame() else { return false };
        let info = frame.info();
        // Only a D3D11VA picture (below) is drawn by `stream::direct`.
        #[cfg(windows)]
        {
            self.d3d11_picture = false;
            self.held = None;
            self.converted_current = false;
        }
        // SDL's HDR headroom and tone mapping are relative to the display's SDR white.
        let white_level = if shows_hdr(self.hdr_mode, self.hdr_stream, info.color_trc) {
            // SAFETY: The live renderer belongs to the presenting thread.
            unsafe {
                SDL_GetFloatProperty(
                    SDL_GetRendererProperties(renderer),
                    SDL_PROP_RENDERER_SDR_WHITE_POINT_FLOAT,
                    1.0,
                )
                .max(0.001)
            }
        } else {
            1.0
        };
        if info.kind == media::FRAME_WAVELET {
            let setup = decoded.setup;
            let ten = setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_10BIT != 0;
            let hdr = self.hdr_mode.unwrap_or(ten);
            if hdr && !crate::sdl::hdr_available(renderer) { self.destroy(); return false; }
            let flags = u32::from(setup.color_range != 0) | (u32::from(hdr) << 1) | ((setup.color_space as u32) << 2)
                | (u32::from(ten) << 4) | (u32::from(setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_YUV444 != 0) << 5);
            let parameters = [setup.width as u32, setup.height as u32, info.width as u32, info.height as u32, 0, 0, flags, white_level.to_bits()];
            if self.pyrowave_format.is_some_and(|previous| previous != parameters) { self.destroy(); }
            if !self.ensure(renderer, SDL_PIXELFORMAT_RGBA64_FLOAT, setup.width, setup.height, SDL_COLORSPACE_SRGB_LINEAR, white_level) { return false; }
            if self.pyrowave.is_none() {
                self.pyrowave = super::pyrowave_gpu::Reconstruction::new(renderer, self.texture, parameters);
                self.pyrowave_format = Some(parameters);
            }
            let drawn = self.pyrowave.as_ref().is_some_and(|gpu| gpu.draw_timed(renderer, self.texture, frame, &mut self.pyrowave_stages));
            if drawn { self.pyrowave_stages.commit(); } else { self.pyrowave_stages.reset(); }
            return drawn;
        }
        if decoded.setup.chroma_packing != 0 {
            let setup = decoded.setup;
            let Some(layout) = rubylight_protocol::packed444::Geometry::new(
                setup.logical_width as u32,
                setup.logical_height as u32,
            ) else {
                return false;
            };
            if !rubylight_protocol::packed444::supported(setup.chroma_packing)
                || layout.coded_size() != (info.width as u32, info.height as u32)
                || (info.ten_bit != 0)
                    != (setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_10BIT != 0)
            {
                self.destroy();
                return false;
            }
            let hdr = self
                .hdr_mode
                .unwrap_or(setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_10BIT != 0);
            if hdr && !crate::sdl::hdr_available(renderer) {
                self.destroy();
                return false;
            }
            let parameters = [
                layout.width,
                layout.height,
                layout.padded_width,
                layout.padded_height,
                info.ten_bit as u32,
                setup.color_range as u32,
                setup.color_space as u32,
                u32::from(hdr),
                white_level.to_bits(),
                u32::from(setup.chroma_packing),
                0,
                0,
            ];
            // A picture mapped into Vulkan (or Vulkan video's) is rebuilt from its planes; one in
            // memory from a buffer. A new decoder may change which, so the reconstruction goes too.
            #[cfg(unix)]
            let coded = media::PlanarLayout::of_vulkan(&info);
            #[cfg(unix)]
            let other_path = packed_path_changed(self.packed.is_some(), coded.is_some(), self.vulkan.is_some());
            #[cfg(not(unix))]
            let other_path = false;
            if other_path
                || self
                    .packed_format
                    .is_some_and(|previous| previous != parameters)
            {
                self.destroy();
            }
            #[cfg(windows)]
            if self.direct_packed && (!hdr || unsafe {
                let properties = SDL_GetRendererProperties(renderer);
                let peak = self.hdr_metadata.map_or(0, |m| {
                    if m.max_content_light_level != 0 { m.max_content_light_level } else { m.max_display_luminance }
                });
                // Keep SDL's texture path when metadata calls for tone mapping.
                SDL_GetNumberProperty(properties, SDL_PROP_RENDERER_OUTPUT_COLORSPACE_NUMBER, 0)
                    == i64::from(SDL_COLORSPACE_SRGB_LINEAR.0)
                    && f32::from(peak) / (80.0 * white_level)
                        <= SDL_GetFloatProperty(properties, SDL_PROP_RENDERER_HDR_HEADROOM_FLOAT, 1.0)
            })
                && !crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_DEVICE_POINTER)
                    .is_null()
            {
                if self.packed.is_none() || !self.texture.is_null() {
                    self.destroy();
                    self.packed = super::packed444::Reconstruction::new(
                        renderer,
                        std::ptr::null_mut(),
                        &parameters,
                    );
                }
                let success = self
                    .packed
                    .as_mut()
                    .is_some_and(|packed| packed.upload(frame));
                if success {
                    self.width = setup.logical_width;
                    self.height = setup.logical_height;
                    self.packed_format = Some(parameters);
                } else {
                    self.destroy();
                }
                return success;
            }
            if !self.ensure(
                renderer,
                SDL_PIXELFORMAT_RGBA64_FLOAT,
                setup.logical_width,
                setup.logical_height,
                SDL_COLORSPACE_SRGB_LINEAR,
                white_level,
            ) {
                return false;
            }
            #[cfg(unix)]
            if let Some(coded) = coded {
                return self.draw_packed_planes(renderer, frame, coded, &parameters);
            }
            if self.packed.is_none() {
                self.packed =
                    super::packed444::Reconstruction::new(renderer, self.texture, &parameters);
                self.packed_format = Some(parameters);
            }
            let success = self
                .packed
                .as_mut()
                .is_some_and(|packed| packed.draw(renderer, self.texture, frame, &parameters));
            if !success {
                self.destroy();
            }
            return success;
        }
        self.hdr_frame = info.color_trc == TRC_PQ;
        self.hlg_frame = info.color_trc == TRC_HLG;
        if let Some(layout) = media::PlanarLayout::of(&info) {
            return self.upload_planar(renderer, &info, layout, white_level);
        }
        #[cfg(unix)]
        if let Some(layout) = media::PlanarLayout::of_vulkan(&info) {
            return self.upload_vulkan(renderer, frame, &info, layout, white_level);
        }
        #[cfg(unix)]
        if hlg_through_planes(&info) {
            return self.upload_hlg(renderer, frame, &info, white_level);
        }
        let space = colorspace(&info);
        let (w, h) = (info.width, info.height);
        // SAFETY: plane pointers and pitches come from the live AVFrame.
        unsafe {
            match info.kind {
                // AYUV, Y410 or Y416: no SDL texture format, so the plane pass draws its copy.
                #[cfg(windows)]
                media::FRAME_D3D11_444 => self.upload_d3d11_444(renderer, frame, &info, white_level),
                #[cfg(windows)]
                media::FRAME_D3D11 => {
                    let format = if info.ten_bit != 0 {
                        SDL_PIXELFORMAT_P010
                    } else {
                        SDL_PIXELFORMAT_NV12
                    };
                    if !self.ensure(renderer, format, w, h, space, white_level) {
                        return false;
                    }
                    self.d3d11_picture = true;
                    self.hdr10_plus = frame.hdr10_plus_peak();
                    self.dolby_vision = match frame.dolby_vision() {
                        Some(rpu) if rpu.ipt => {
                            if !self.dolby_vision_ipt_said {
                                eprintln!("nightjar: the stream's Dolby Vision is profile 5 (IPT), which has no HDR10 base layer; its pictures are drawn as HDR10");
                                self.dolby_vision_ipt_said = true;
                            }
                            None
                        }
                        rpu => rpu.and_then(|rpu| rpu.peak),
                    };
                    self.held = None;
                    self.copied = false;
                    // The pass reads a readable decoder texture in place: no copy.
                    if !self.direct_failed && super::direct::enabled()
                        && frame.d3d11_surface().is_some_and(|surface| surface.readable)
                    {
                        self.held = frame.reference();
                        if self.held.is_some() {
                            return true;
                        }
                    }
                    if self.d3d11.is_null() {
                        self.fail(
                            format!(
                                "the {} video texture has no Direct3D 11 texture to copy the decoder's pictures into (renderer {})",
                                pixel_format_name(format),
                                crate::sdl::renderer_name(renderer),
                            ),
                            "the video texture is not a Direct3D 11 texture".into(),
                        );
                        return false;
                    }
                    let copied = frame.copy_to_d3d11(self.d3d11, w, h);
                    self.copied = copied;
                    if !copied {
                        self.fail(
                            format!("copying a {w}x{h} decoder picture into the {} video texture failed", pixel_format_name(format)),
                            "the GPU copy failed".into(),
                        );
                    }
                    copied
                }
                #[cfg(target_os = "macos")]
                media::FRAME_VIDEOTOOLBOX => {
                    if crate::sdl::renderer_is(renderer, "metal") {
                        let format = if info.ten_bit == 0 { SDL_PIXELFORMAT_NV12 } else { SDL_PIXELFORMAT_P010 };
                        // FFmpeg keeps the CVPixelBufferRef in AVFrame.data[3]. Keep the
                        // frame (and thus its pool slot) alive for the displayed texture.
                        let buffer: *mut c_void = frame.raw.as_ref().data[3].cast();
                        if !buffer.is_null() {
                            if let Some(held) = frame.reference() {
                                if self.ensure_with(renderer, format, w, h, space, white_level, None, buffer) {
                                    self.held = Some(held);
                                    return true;
                                }
                            }
                        }
                    }
                    // An SDL renderer without IOSurface import can still display the frame.
                    frame.map_to_memory().is_some_and(|mapped| self.upload_nv(renderer, &mapped.info(), space, white_level))
                }
                media::FRAME_NV12 | media::FRAME_P010 => self.upload_nv(renderer, &info, space, white_level),
                #[cfg(unix)]
                media::FRAME_DMABUF => self.upload_dmabuf(renderer, frame, &info, space, white_level),
                media::FRAME_YUV420P => {
                    self.ensure(renderer, SDL_PIXELFORMAT_IYUV, w, h, space, white_level)
                        && SDL_UpdateYUVTexture(
                            self.texture,
                            std::ptr::null(),
                            info.data[0],
                            info.linesize[0],
                            info.data[1],
                            info.linesize[1],
                            info.data[2],
                            info.linesize[2],
                        )
                }
                _ => false,
            }
        }
    }

    /// Uploads an NV12 or P010 picture from memory.
    fn upload_nv(&mut self, renderer: *mut SDL_Renderer, info: &FrameInfo, space: SDL_Colorspace, white_level: f32) -> bool {
        // A texture made for imports reads its chroma as red-green, uploads are luminance-alpha.
        #[cfg(unix)]
        if self.imported {
            self.destroy();
        }
        let format = if info.kind == media::FRAME_P010 { SDL_PIXELFORMAT_P010 } else { SDL_PIXELFORMAT_NV12 };
        self.ensure(renderer, format, info.width, info.height, space, white_level)
            // SAFETY: plane pointers and pitches come from the live AVFrame.
            && unsafe {
                SDL_UpdateNVTexture(self.texture, std::ptr::null(), info.data[0], info.linesize[0], info.data[1], info.linesize[1])
            }
    }

    /// Imports a VAAPI picture's DMA-BUFs into the NV12 texture. When that cannot be done it says
    /// why once and maps the picture to memory instead, and the decoder maps the stream's next
    /// ones itself.
    #[cfg(unix)]
    fn upload_dmabuf(&mut self, renderer: *mut SDL_Renderer, frame: &media::Frame, info: &FrameInfo, space: SDL_Colorspace, white_level: f32) -> bool {
        let Some(export) = frame.dmabuf() else { return false };
        if !self.dmabuf_declined {
            match self.import(renderer, export, info, space, white_level) {
                Ok(()) => {
                    // The texture samples the decoder's surface: it stays out of the decoder's
                    // hands until the next picture replaces it.
                    self.held = frame.reference();
                    return true;
                }
                Err(error) => {
                    eprintln!("nightjar: VAAPI pictures cannot be imported as DMA-BUFs ({error}); mapping them to memory instead");
                    self.dmabuf_declined = true;
                    self.destroy();
                }
            }
        }
        export.decline();
        let Some(mapped) = frame.map_to_memory() else {
            self.fail(format!("mapping a {}x{} VAAPI picture to memory failed", info.width, info.height), "the picture could not be read".into());
            return false;
        };
        self.upload_nv(renderer, &mapped.info(), space, white_level)
    }

    /// Draws an HLG NV12 or P010 picture with the plane shader (`hlg_through_planes`). A VAAPI
    /// surface is mapped to memory for it, this picture alone: the decoder keeps exporting, so a
    /// PQ or SDR picture after it is imported again.
    #[cfg(unix)]
    fn upload_hlg(&mut self, renderer: *mut SDL_Renderer, frame: &media::Frame, info: &FrameInfo, white_level: f32) -> bool {
        let mapped = if info.kind == media::FRAME_DMABUF {
            let Some(mapped) = frame.map_to_memory() else {
                self.fail(format!("mapping a {}x{} VAAPI picture to memory failed", info.width, info.height), "the picture could not be read".into());
                return false;
            };
            Some(mapped)
        } else {
            None
        };
        let info = mapped.as_ref().map_or(*info, media::Frame::info);
        let Some(layout) = media::PlanarLayout::of_semi_planar(&info) else {
            self.fail(format!("a {}x{} HLG picture of frame kind {} has no planes to draw", info.width, info.height, info.kind), "the HLG picture cannot be drawn".into());
            return false;
        };
        self.upload_planar(renderer, &info, layout, white_level)
    }

    #[cfg(unix)]
    fn import(&mut self, renderer: *mut SDL_Renderer, export: &crate::dmabuf::Export, info: &FrameInfo, space: SDL_Colorspace, white_level: f32) -> Result<(), String> {
        if !self.imported && !self.texture.is_null() {
            self.destroy();
        }
        // The hint is read when SDL makes the texture: only textures for imports get it.
        let hint = c"SDL_RENDER_OPENGL_NV12_RG_SHADER";
        crate::sdl::set_hint(hint.as_ptr(), c"1");
        let made = self.ensure(renderer, SDL_PIXELFORMAT_NV12, info.width, info.height, space, white_level);
        // SAFETY: a hint name.
        unsafe { SDL_ResetHint(hint.as_ptr()) };
        if !made {
            return Err(self.failure.take().unwrap_or_else(|| "SDL made no NV12 texture".into()));
        }
        self.imported = true;
        if self.importer.is_none() {
            self.importer = Some(crate::dmabuf::Importer::new()?);
        }
        let importer = self.importer.as_mut().expect("made above");
        // SAFETY: on the renderer's thread, with its live texture.
        unsafe { importer.import(renderer, self.texture, export, info.width as u32, info.height as u32) }
    }

    /// The current picture's transfer: the one decision of the pass, AI enhancement and what
    /// SDL draws (`picture_transfer`).
    #[cfg(windows)]
    fn transfer(&self) -> super::direct::Transfer {
        picture_transfer(self.format, self.hdr_mode, self.hdr_stream, self.hdr_frame, self.hlg_frame)
    }

    /// Puts a held decoder picture into the video texture, for what reads it (AI enhancement,
    /// `sdl_texture`). Nothing to do when the picture was copied.
    #[cfg(windows)]
    pub fn copy_held(&mut self) {
        self.materialize_planes();
        let Some(frame) = &self.held else { return };
        if self.copied || self.d3d11.is_null() { return; }
        self.copied = frame.copy_to_d3d11(self.d3d11, self.width, self.height);
        if !self.copied {
            self.fail(
                format!("copying a {}x{} decoder picture into the {} video texture failed", self.width, self.height, pixel_format_name(self.format)),
                "the GPU copy failed".into(),
            );
        }
    }

    #[cfg(not(windows))]
    pub fn copy_held(&mut self) {}

    /// The texture SDL draws or the upscalers read for the current picture: the video texture
    /// (with a held picture copied in, or on Linux the pending planes drawn into it), or for a
    /// 10-bit SDR or an HLG picture, which SDL's shaders would decode as PQ or gamma, its RGB
    /// conversion by the pass.
    #[cfg(windows)]
    pub fn sdl_texture(&mut self, renderer: *mut SDL_Renderer) -> *mut SDL_Texture {
        self.materialize_planes();
        if self.texture.is_null() || !self.d3d11_picture || !sdl_needs_rgb(self.format, self.transfer()) {
            self.copy_held();
            return self.texture;
        }
        if !self.converted_current {
            if let Err(error) = self.convert(renderer) {
                let what = if self.transfer() == super::direct::Transfer::Hlg { "an HLG" } else { "a 10-bit SDR" };
                self.fail(format!("converting {what} picture for SDL failed: {error}"), "the GPU conversion failed".into());
                self.copy_held();
                return self.texture;
            }
            self.converted_current = true;
        }
        self.converted
    }

    #[cfg(unix)]
    pub fn sdl_texture(&mut self, renderer: *mut SDL_Renderer) -> *mut SDL_Texture {
        if let Err(error) = self.draw_pending_planes(renderer) {
            self.fail(format!("drawing the picture's planes into the video texture failed: {error}"), error);
        }
        self.texture
    }

    #[cfg(not(any(windows, unix)))]
    pub fn sdl_texture(&mut self, _renderer: *mut SDL_Renderer) -> *mut SDL_Texture {
        self.texture
    }

    /// Where the plane pass draws a picture: nowhere yet while the planes can go straight into the
    /// back buffer (`draw_direct`), else the texture.
    #[cfg(unix)]
    fn planes_target(&self) -> *mut SDL_Texture {
        if self.direct_planes_failed { self.texture } else { std::ptr::null_mut() }
    }

    /// Notes a picture the plane pass drew into `target` with `parameters`.
    #[cfg(unix)]
    fn planes_drawn(&mut self, target: *mut SDL_Texture, parameters: [u32; 8]) {
        self.planes_pending = target.is_null().then_some(parameters);
    }

    /// Draws the copied planes into the texture, once per picture.
    #[cfg(unix)]
    fn draw_pending_planes(&mut self, renderer: *mut SDL_Renderer) -> Result<(), String> {
        let Some(parameters) = self.planes_pending.take() else { return Ok(()) };
        let step = std::time::Instant::now();
        let texture = self.texture;
        let pass = self.planes.as_mut().ok_or("the plane pass is gone")?;
        // SAFETY: The pass copied the current picture's planes on this renderer, for `parameters`.
        unsafe { pass.draw_bound(renderer, texture, &parameters) }?;
        self.passes_drawn(step);
        Ok(())
    }

    /// The deferred draw of a PyroWave picture's planes goes to its "passes" stage, once, and
    /// completes the picture's stage times (`upload_planes` leaves them open).
    #[cfg(unix)]
    fn passes_drawn(&mut self, step: std::time::Instant) {
        if std::mem::take(&mut self.pyrowave_stages_open) {
            self.pyrowave_stages.add(super::pyrowave_gpu::PASSES, step);
            self.pyrowave_stages.commit();
        }
    }

    /// Whether `draw_direct` will draw the current picture with the plane pass (unless the pass
    /// fails then): its planes are pending, the pass is there and has not failed.
    #[cfg(target_os = "linux")]
    pub fn draws_planes_direct(&self) -> bool {
        self.planes_pending.is_some() && self.planes.is_some() && !self.texture.is_null() && !self.direct_planes_failed
    }

    /// Whether SDL settled this frame as video only: `draw_direct` then writes PQ (`VideoOnly`).
    #[cfg(target_os = "linux")]
    pub fn set_pq_target(&mut self, pq_target: bool) {
        self.pq_target = pq_target;
    }

    #[cfg(target_os = "linux")]
    pub fn video_only(&mut self) -> &mut VideoOnly {
        &mut self.video_only
    }

    /// Draws the current picture's copied planes straight into `destination` of the back buffer,
    /// converted and scaled in one pass, and returns true; false for SDL to draw `texture` (an
    /// upscaler's output, a picture that is not planes, or after a failure, which is logged once).
    /// The pass does what SDL's draw of the RGBA64F texture would: its tone map and colour scale.
    #[cfg(unix)]
    pub fn draw_direct(&mut self, renderer: *mut SDL_Renderer, destination: &SDL_FRect, texture: *mut SDL_Texture) -> bool {
        let Some(parameters) = self.planes_pending.filter(|_| texture == self.texture && !texture.is_null()) else { return false };
        let step = std::time::Instant::now();
        // SAFETY: The live renderer and texture of the presenting thread.
        let look = unsafe { super::planes_gpu::SdlLook::of(renderer, texture) };
        #[cfg(target_os = "linux")]
        let look = super::planes_gpu::SdlLook { pq_target: self.pq_target, ..look };
        let visible = self.size();
        let Some(pass) = self.planes.as_mut() else { return false };
        // SAFETY: The pass copied the current picture's planes on this renderer, for `parameters`.
        match unsafe { pass.draw_direct(renderer, &parameters, destination, visible, look) } {
            Ok(()) => {
                self.passes_drawn(step);
                true
            }
            Err(error) => {
                eprintln!("nightjar: drawing the picture's planes into the back buffer failed: {error}; they go through the video texture from now on");
                self.direct_planes_failed = true;
                false
            }
        }
    }

    /// Whether the picture being uploaded waits for the back buffer (`direct_planes::goes_direct`).
    #[cfg(windows)]
    fn goes_direct(&self, renderer: *mut SDL_Renderer, partial: bool) -> bool {
        super::direct_planes::goes_direct(super::direct_planes::Conditions {
            switch: super::direct_planes::enabled(),
            failed: self.direct_planes_failed,
            swap_chain: !self.d3d11.is_null() && !crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER).is_null(),
            partial,
        })
    }

    /// Says once which way the stream's `what` pictures are drawn.
    #[cfg(windows)]
    fn say_planes_path(&mut self, direct: bool, what: &str) {
        if std::mem::replace(&mut self.direct_planes_said, true) { return; }
        if direct {
            eprintln!("nightjar: {what} pictures are converted and scaled straight into the back buffer in one pass ({}=0 draws them through the video texture)", super::direct_planes::SWITCH);
        } else if !super::direct_planes::enabled() {
            eprintln!("nightjar: {what} pictures are drawn through the video texture ({}=0)", super::direct_planes::SWITCH);
        } else {
            eprintln!("nightjar: {what} pictures are drawn through the video texture");
        }
    }

    /// Makes `picture` the current one, waiting for the back buffer; the one it replaces goes back.
    #[cfg(windows)]
    fn hold_direct(&mut self, picture: DirectPlanes, what: &str) {
        self.finish_direct();
        self.say_planes_path(true, what);
        self.direct_planes = Some(picture);
    }

    /// Lets go of the current picture's planes: a GPU decoder's plane set goes back to the decoder
    /// behind the last draw of it.
    #[cfg(windows)]
    fn finish_direct(&mut self) {
        let Some(DirectPlanes { source: PlanesHold::Lease { lease, .. }, .. }) = self.direct_planes.take() else { return };
        if let Some(pass) = &self.planes {
            if let Err(error) = pass.give_back(&lease) {
                eprintln!("nightjar: {error}");
            }
        }
    }

    /// Draws the current picture's waiting planes into the video texture, once, for what reads
    /// the texture (an upscaler, AI enhancement, or SDL after a failed direct draw).
    #[cfg(windows)]
    fn materialize_planes(&mut self) {
        let Some(mut picture) = self.direct_planes.take() else { return };
        if !picture.in_texture {
            let parameters = picture.parameters;
            let drawn = match (&picture.source, self.planes.as_mut()) {
                (_, None) => Err("the plane pass is gone".to_string()),
                (PlanesHold::Lease { lease, fence_value }, Some(pass)) => pass.draw_held(lease, *fence_value, &parameters, PLANES_DECODE_DEADLINE),
                (PlanesHold::Frame, Some(pass)) => pass.draw_uploaded_frame(&parameters),
                (PlanesHold::Surface { held, .. }, Some(pass)) => pass.draw_bound_surface(&mut |submit| match held {
                    Some(frame) => frame.with_decoder_lock(submit),
                    None => submit(),
                }),
            };
            match drawn {
                Ok(()) => picture.in_texture = true,
                Err(error) => self.fail(format!("drawing the picture's planes into the video texture failed: {error}"), error),
            }
        }
        self.direct_planes = Some(picture);
    }

    /// Draws the current picture's planes into `destination` of the back buffer, converted and
    /// scaled in one pass, and returns true; false for SDL to draw `texture` (an upscaler's output,
    /// or after a failure, which is logged once and keeps the texture for the stream).
    #[cfg(windows)]
    fn draw_planes_direct(&mut self, renderer: *mut SDL_Renderer, destination: &SDL_FRect, texture: *mut SDL_Texture) -> bool {
        if texture.is_null() || texture != self.texture { return false; }
        let step = std::time::Instant::now();
        let swap_chain = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER);
        if swap_chain.is_null() { return false; }
        let mut output = (0, 0);
        // SAFETY: The live renderer and texture of the presenting thread.
        let (look, flushed) = unsafe {
            SDL_GetCurrentRenderOutputSize(renderer, &mut output.0, &mut output.1);
            let look = super::direct_planes::Look::of(renderer, texture);
            // A new output size is resized by SDL's next run of its queue, this flush: let go first.
            if let Some(pass) = &mut self.planes { pass.keep_back_buffer_if(swap_chain as usize, output); }
            (look, SDL_FlushRenderer(renderer))
        };
        let visible = self.size();
        let constants = super::direct_planes::constants(destination, visible, look);
        let viewport = {
            let x = destination.x.max(0.0);
            let y = destination.y.max(0.0);
            [x, y, (destination.x + destination.w).min(output.0 as f32) - x, (destination.y + destination.h).min(output.1 as f32) - y]
        };
        let (Some(pass), Some(picture)) = (self.planes.as_mut(), self.direct_planes.as_ref()) else { return false };
        let drawn = if !flushed {
            Err(format!("SDL_FlushRenderer: {}", crate::sdl::error()))
        } else {
            // SAFETY: The renderer's swap chain, on the device the pass was made on.
            unsafe { pass.back_buffer(swap_chain, output) }.and_then(|target| match &picture.source {
                PlanesHold::Lease { lease, fence_value } => {
                    let source = super::d3d11::DirectSource::Lease { lease, fence_value: *fence_value, deadline: PLANES_DECODE_DEADLINE };
                    pass.draw_direct(&target, viewport, &source, &picture.parameters, &constants, &mut |submit| submit())
                }
                PlanesHold::Frame => pass.draw_direct(&target, viewport, &super::d3d11::DirectSource::Frame, &picture.parameters, &constants, &mut |submit| submit()),
                PlanesHold::Surface { held, .. } => pass.draw_direct(&target, viewport, &super::d3d11::DirectSource::Surface, &picture.parameters, &constants, &mut |submit| match held {
                    Some(frame) => frame.with_decoder_lock(submit),
                    None => submit(),
                }),
            })
        };
        match drawn {
            Ok(()) => {
                if let Some(picture) = self.direct_planes.as_mut() {
                    if matches!(picture.source, PlanesHold::Lease { .. }) && !std::mem::replace(&mut picture.timed, true) {
                        self.pyrowave_stages.add(super::pyrowave_gpu::PASSES, step);
                        self.pyrowave_stages.commit();
                    }
                }
                true
            }
            Err(error) => {
                eprintln!("nightjar: drawing the picture's planes into the back buffer failed: {error}; they go through the video texture from now on");
                self.direct_planes_failed = true;
                false
            }
        }
    }

    #[cfg(windows)]
    fn convert(&mut self, renderer: *mut SDL_Renderer) -> Result<(), String> {
        use windows::core::Interface;
        let picture = self.picture().ok_or("no picture to convert")?;
        // HLG becomes linear light relative to SDR white in a float texture, as the PyroWave and
        // packed pictures SDL draws; 10-bit SDR gamma-coded RGB.
        let linear = picture.transfer == super::direct::Transfer::Hlg;
        // SAFETY: A live texture of this renderer.
        let made_for = (!self.converted.is_null()).then(|| unsafe { ((*self.converted).w, (*self.converted).h) });
        if !self.converted.is_null() && (self.converted_linear != linear || made_for != Some(self.size())) {
            self.converted_view = None;
            // SAFETY: a texture we created.
            unsafe { SDL_DestroyTexture(self.converted) };
            self.converted = std::ptr::null_mut();
        }
        if self.converted.is_null() {
            // SAFETY: SDL calls on the presenting thread's live renderer.
            unsafe {
                let (format, space) = if linear { (SDL_PIXELFORMAT_RGBA64_FLOAT, SDL_COLORSPACE_SRGB_LINEAR) } else { (SDL_PIXELFORMAT_ABGR2101010, SDL_COLORSPACE_SRGB) };
                let props = SDL_CreateProperties();
                SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_FORMAT_NUMBER, i64::from(format.0));
                SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_ACCESS_NUMBER, i64::from(SDL_TEXTUREACCESS_TARGET.0));
                SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_WIDTH_NUMBER, i64::from(self.size().0));
                SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_HEIGHT_NUMBER, i64::from(self.size().1));
                SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_COLORSPACE_NUMBER, i64::from(space.0));
                self.converted = SDL_CreateTextureWithProperties(renderer, props);
                self.converted_linear = linear;
                SDL_DestroyProperties(props);
                if self.converted.is_null() { return Err(format!("SDL could not make the RGB texture: {}", crate::sdl::error())); }
                SDL_SetTextureScaleMode(self.converted, SDL_SCALEMODE_LINEAR);
                let raw = SDL_GetPointerProperty(SDL_GetTextureProperties(self.converted), SDL_PROP_TEXTURE_D3D11_TEXTURE_POINTER, std::ptr::null_mut());
                let texture = windows::Win32::Graphics::Direct3D11::ID3D11Texture2D::from_raw_borrowed(&raw).ok_or("the RGB texture has no Direct3D 11 texture")?;
                let mut view = None;
                texture.GetDevice().and_then(|device| device.CreateRenderTargetView(texture, None, Some(&mut view)))
                    .map_err(|e| format!("the RGB texture's view: {e}"))?;
                self.converted_view = view;
            }
        }
        let view = self.converted_view.clone().ok_or("the RGB texture has no view")?;
        let size = (self.size().0 as u32, self.size().1 as u32);
        // SAFETY: The live renderer belongs to the presenting thread.
        let headroom = unsafe { SDL_GetFloatProperty(SDL_GetRendererProperties(renderer), SDL_PROP_RENDERER_HDR_HEADROOM_FLOAT, 1.0) };
        let white_nits = 80.0 * self.white_level;
        let pass = direct_pass(&mut self.direct, renderer)?;
        let held = &self.held;
        // SAFETY: The picture's texture and the view belong to the renderer's device.
        unsafe { pass.convert(&view, size, &picture, white_nits, headroom, |submit| lock_for(held, picture.texture, submit)) }
    }

    /// The current D3D11VA picture for the pass: the held decoder slice or the copied texture.
    #[cfg(windows)]
    fn picture(&self) -> Option<super::direct::Picture> {
        let ten_bit = match self.format {
            SDL_PIXELFORMAT_NV12 => false,
            SDL_PIXELFORMAT_P010 => true,
            _ => return None,
        };
        if !self.d3d11_picture || self.texture.is_null() {
            return None;
        }
        let (texture, slice) = match &self.held {
            Some(frame) => frame.d3d11_surface().map(|surface| (surface.texture, surface.slice))?,
            None if self.copied && !self.d3d11.is_null() => (self.d3d11, 0),
            None => return None,
        };
        let (matrix, full_range) = direct_matrix(self.colorspace);
        Some(super::direct::Picture {
            texture, slice, size: (self.size().0 as u32, self.size().1 as u32), ten_bit, full_range, matrix, transfer: self.transfer(),
        })
    }

    /// Draws `texture` into the back buffer with `stream::direct` and returns true, or returns
    /// false for SDL to draw it. `texture` is the video texture or AI enhancement's output; the
    /// pass draws the D3D11VA picture of either, and nothing else. A failing pass is logged
    /// once and SDL draws from then on.
    /// Lets go of the direct passes' back-buffer views, for a frame whose upscaler or AI
    /// enhancement runs SDL's queue before `draw_direct`: SDL resizes there, and a held view
    /// would make its `ResizeBuffers` fail.
    #[cfg(windows)]
    pub fn release_back_buffers(&mut self) {
        if let Some(pass) = &mut self.planes { pass.release_back_buffer(); }
        if let Some((_, pass)) = &mut self.direct { pass.release_back_buffer(); }
    }

    #[cfg(windows)]
    pub fn draw_direct(&mut self, renderer: *mut SDL_Renderer, destination: &SDL_FRect, texture: *mut SDL_Texture) -> bool {
        if self.direct_planes.is_some() {
            let drawn = self.draw_planes_direct(renderer, destination, texture);
            if !drawn {
                // SDL may resize the back buffer before the pass draws again.
                if let Some(pass) = &mut self.planes { pass.release_back_buffer(); }
            }
            return drawn;
        }
        if let Some(pass) = &mut self.planes { pass.release_back_buffer(); }
        let drawn = self.try_draw_direct(renderer, destination, texture);
        if !drawn {
            // SDL may resize the back buffer before the pass draws again.
            if let Some((_, pass)) = &mut self.direct { pass.release_back_buffer(); }
        }
        drawn
    }

    #[cfg(windows)]
    fn try_draw_direct(&mut self, renderer: *mut SDL_Renderer, destination: &SDL_FRect, texture: *mut SDL_Texture) -> bool {
        if self.direct_failed || texture.is_null() || !super::direct::enabled() {
            return false;
        }
        let Some(mut picture) = self.picture() else { return false };
        if texture != self.texture {
            // AMF's HQ Scaler output: the same picture as NV12 or P010 at the output size.
            // SAFETY: A live texture of this renderer.
            let (format, w, h) = unsafe { ((*texture).format, (*texture).w, (*texture).h) };
            if format != self.format {
                return false;
            }
            // SAFETY: A live texture of this renderer.
            picture.texture = unsafe { SDL_GetPointerProperty(SDL_GetTextureProperties(texture), SDL_PROP_TEXTURE_D3D11_TEXTURE_POINTER, std::ptr::null_mut()) };
            if picture.texture.is_null() {
                return false;
            }
            picture.slice = 0;
            picture.size = (w as u32, h as u32);
        }
        let metadata = self.hdr_metadata.filter(|_| self.hdr_mode == Some(true));
        let white_nits = 80.0 * self.white_level;
        let tone = super::direct::Tone {
            white_nits,
            dolby_vision: self.dolby_vision,
            hdr10_plus: self.hdr10_plus,
            static_peak: hdr_properties(true, self.white_level, metadata).headroom.map(|headroom| headroom * white_nits),
        };
        let drawn = direct_pass(&mut self.direct, renderer).and_then(|pass| {
            let held = &self.held;
            // SAFETY: The picture's texture and the renderer belong to the pass's device.
            unsafe { pass.present(renderer, destination, &picture, tone, |submit| lock_for(held, picture.texture, submit)) }
        });
        match drawn {
            Ok(()) => true,
            Err(error) => {
                eprintln!("nightjar: drawing the decoder's pictures directly failed: {error}; SDL draws them from now on");
                self.direct_failed = true;
                self.direct = None;
                false
            }
        }
    }

    pub fn draw(&mut self, renderer: *mut SDL_Renderer, destination: &SDL_FRect) -> bool {
        #[cfg(windows)]
        if self.texture.is_null() {
            if let Some(packed) = &mut self.packed {
                return packed.present(
                    renderer,
                    self.packed_format.as_ref().unwrap(),
                    destination,
                );
            }
        }
        if !self.texture.is_null() {
            let source = self.source_rect(self.texture);
            // SAFETY: The renderer and its video texture are live on this thread.
            unsafe { SDL_RenderTexture(renderer, self.texture, source.as_ref().map_or(std::ptr::null(), |rect| rect as *const _), destination) };
        }
        true
    }

    /// Makes the texture and the planes pass ready for pictures of `setup` and returns the pass
    /// parameters, or `None` (with `failure` set when it is a loud one).
    #[cfg(windows)]
    fn prepare_planes(&mut self, renderer: *mut SDL_Renderer, setup: nightjar_core::session::VideoSetup) -> Option<[u32; 8]> {
        let ten = setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_10BIT != 0;
        let hdr = self.hdr_mode.unwrap_or(ten);
        if hdr && !crate::sdl::hdr_available(renderer) { self.destroy(); return None; }
        // SAFETY: The live renderer belongs to the presenting thread.
        let white_level = if hdr { unsafe { SDL_GetFloatProperty(SDL_GetRendererProperties(renderer), SDL_PROP_RENDERER_SDR_WHITE_POINT_FLOAT, 1.0) }.max(0.001) } else { 1.0 };
        let flags = u32::from(setup.color_range != 0) | (u32::from(hdr) << 1) | ((setup.color_space as u32) << 2)
            | (u32::from(ten) << 4) | (u32::from(setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_YUV444 != 0) << 5);
        let parameters = [setup.width as u32, setup.height as u32, setup.width as u32, setup.height as u32, 0, 3, flags, white_level.to_bits()];
        if self.pyrowave_format.is_some_and(|previous| previous != parameters) { self.destroy(); }
        if !self.ensure(renderer, SDL_PIXELFORMAT_RGBA64_FLOAT, setup.width, setup.height, SDL_COLORSPACE_SRGB_LINEAR, white_level) { return None; }
        if self.planes.is_none() {
            if self.d3d11.is_null() {
                self.failure = Some("PyroWave GPU decoding needs the Direct3D 11 renderer".into());
                return None;
            }
            // SAFETY: `d3d11` is the live texture of the renderer's device.
            match unsafe { super::d3d11::PlanesPass::new(self.d3d11) } {
                Ok(pass) => { self.planes = Some(pass); self.pyrowave_format = Some(parameters); }
                Err(error) => { self.failure = Some(format!("PyroWave GPU decoding cannot draw its planes: {error}")); return None; }
            }
        }
        Some(parameters)
    }

    /// Draws a picture the GPU decoder left in D3D11 planes: waits for its decode on the GPU,
    /// converts the planes (only `rows` of them when given) into the texture and gives the plane
    /// set back.
    #[cfg(windows)]
    fn upload_planes(&mut self, renderer: *mut SDL_Renderer, decoded: &Decoded, lease: &super::gpu_planes::Lease, fence_value: u64, rows: Option<(u32, u32)>) -> bool {
        let Some(parameters) = self.prepare_planes(renderer, decoded.setup) else { return false };
        // The picture's set stays with the presenter while the planes wait for the back buffer.
        if self.goes_direct(renderer, rows.is_some()) {
            if let Some(held) = lease.hand_over() {
                self.pyrowave_stages.reset();
                self.hold_direct(DirectPlanes { source: PlanesHold::Lease { lease: held, fence_value }, parameters, in_texture: false, timed: false }, "PyroWave");
                return true;
            }
        }
        self.finish_direct();
        self.say_planes_path(false, "PyroWave");
        let step = std::time::Instant::now();
        let drawn = self.planes.as_mut().is_some_and(|pass| match pass.draw(lease, fence_value, &parameters, PLANES_DECODE_DEADLINE, rows) {
            Ok(()) => true,
            Err(error) => { self.failure = Some(format!("PyroWave GPU decoding failed: {error}")); false }
        });
        if drawn {
            self.pyrowave_stages.add(super::pyrowave_gpu::PASSES, step);
            self.pyrowave_stages.commit();
        } else { self.pyrowave_stages.reset(); }
        drawn
    }

    /// Draws `rows` of a slice of the frame being received into the texture (the frame's plane
    /// set stays with it). Returns false when the slice cannot be drawn; `failure` says why when
    /// the stream must end.
    #[cfg(windows)]
    pub fn upload_slice(&mut self, renderer: *mut SDL_Renderer, setup: nightjar_core::session::VideoSetup, pin: &super::gpu_planes::Pin, fence_value: u64, rows: (u32, u32)) -> bool {
        let Some(parameters) = self.prepare_planes(renderer, setup) else { return false };
        // The rows of a frame being received go through the texture, which the frame's picture then is.
        self.finish_direct();
        let step = std::time::Instant::now();
        let drawn = self.planes.as_mut().is_some_and(|pass| match pass.draw_slice(pin, fence_value, &parameters, PLANES_DECODE_DEADLINE, rows) {
            Ok(()) => true,
            Err(error) => { self.failure = Some(format!("PyroWave GPU decoding failed: {error}")); false }
        });
        if drawn { self.pyrowave_stages.add(super::pyrowave_gpu::PASSES, step); } else { self.pyrowave_stages.reset(); }
        drawn
    }

    /// Draws a GPU-decoded picture: from the decoder's images on the renderer's device, sampled in
    /// place (the pass holds the set until the next picture replaces it) or copied, or from CPU
    /// plane buffers, uploaded; converted into the texture, or at present time into the back
    /// buffer (`draw_direct`).
    #[cfg(unix)]
    fn upload_planes(&mut self, renderer: *mut SDL_Renderer, decoded: &Decoded, lease: &super::gpu_planes::Lease) -> bool {
        let setup = decoded.setup;
        let ten = setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_10BIT != 0;
        let hdr = self.hdr_mode.unwrap_or(ten);
        if hdr && !crate::sdl::hdr_available(renderer) { self.destroy(); return false; }
        // SAFETY: The live renderer belongs to the presenting thread.
        let white_level = if hdr { unsafe { SDL_GetFloatProperty(SDL_GetRendererProperties(renderer), SDL_PROP_RENDERER_SDR_WHITE_POINT_FLOAT, 1.0) }.max(0.001) } else { 1.0 };
        let flags = u32::from(setup.color_range != 0) | (u32::from(hdr) << 1) | ((setup.color_space as u32) << 2)
            | (u32::from(ten) << 4) | (u32::from(setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_YUV444 != 0) << 5);
        let parameters = [setup.width as u32, setup.height as u32, setup.width as u32, setup.height as u32, 0, 3, flags, white_level.to_bits()];
        if self.pyrowave_format.is_some_and(|previous| previous != parameters) { self.destroy(); }
        if !self.ensure(renderer, SDL_PIXELFORMAT_RGBA64_FLOAT, setup.width, setup.height, SDL_COLORSPACE_SRGB_LINEAR, white_level) { return false; }
        // A pass made for the other way of reading the planes (another decoder's) goes; its held
        // set goes back to that decoder's pool.
        if self.planes.as_ref().is_some_and(|pass| !pass.suits(lease.planes())) { self.planes = None; }
        if self.planes.is_none() {
            // SAFETY: The live renderer belongs to the presenting thread.
            match unsafe { super::planes_gpu::PlanesPass::for_planes(renderer, lease.planes()) } {
                Ok(pass) => { self.planes = Some(pass); self.pyrowave_format = Some(parameters); }
                Err(error) => { self.failure = Some(format!("PyroWave GPU decoding cannot draw its planes: {error}")); return false; }
            }
        }
        let texture = self.planes_target();
        let stages = &mut self.pyrowave_stages;
        let failure = &mut self.failure;
        let drawn = self.planes.as_mut().is_some_and(|pass| match unsafe { pass.draw(renderer, texture, lease, &parameters, stages) } {
            Ok(()) => true,
            Err(error) => { *failure = Some(format!("PyroWave GPU decoding failed: {error}")); false }
        });
        if !drawn {
            self.pyrowave_stages.reset();
        } else if texture.is_null() {
            // The conversion runs at the present (`passes_drawn`).
            self.pyrowave_stages_open = true;
        } else {
            self.pyrowave_stages.commit();
        }
        if drawn { self.planes_drawn(texture, parameters); }
        drawn
    }

    /// Draws a D3D11VA 4:4:4 picture (`media::FRAME_D3D11_444`) into the RGBA64F texture as a
    /// software-decoded 4:4:4 one is drawn (same constants: range, matrix, 10-bit SDR, HDR10, HLG):
    /// the plane pass copies the decoder's surface and its surface entry converts it.
    #[cfg(windows)]
    fn upload_d3d11_444(&mut self, renderer: *mut SDL_Renderer, frame: &media::Frame, info: &FrameInfo, white_level: f32) -> bool {
        let Some(layout) = media::PlanarLayout::of_d3d11(info) else {
            self.fail(format!("a {}x{} 4:4:4 decoder picture has no size", info.width, info.height), "the 4:4:4 picture cannot be drawn".into());
            return false;
        };
        let Some(parameters) = self.planar_target(renderer, info, layout, white_level, false) else { return false };
        if self.planes.is_none() {
            if self.d3d11.is_null() {
                self.fail(format!("the RGBA64F video texture has no Direct3D 11 texture (renderer {})", crate::sdl::renderer_name(renderer)), "it needs the Direct3D 11 renderer".into());
                return false;
            }
            // SAFETY: `d3d11` is the live texture of the renderer's device.
            match unsafe { super::d3d11::PlanesPass::new(self.d3d11) } {
                Ok(pass) => { self.planes = Some(pass); self.pyrowave_format = Some(parameters); }
                Err(error) => { self.fail(format!("the plane shader pass cannot be made: {error}"), format!("the plane shader pass cannot be made: {error}")); return false; }
            }
        }
        if self.goes_direct(renderer, false) {
            // A readable decoder texture is read in place, its frame held until the next picture.
            let bound = self.planes.as_mut().unwrap().bind_surface(frame, info, &parameters, true);
            match bound {
                Ok(in_place) => {
                    let held = if in_place { frame.reference() } else { None };
                    let held = if in_place && held.is_none() {
                        // No second reference to the frame: the slice is copied instead.
                        match self.planes.as_mut().unwrap().bind_surface(frame, info, &parameters, false) {
                            Ok(_) => None,
                            Err(error) => { self.fail(format!("binding a {}x{} 4:4:4 decoder picture failed: {error}", layout.width, layout.height), error); return false; }
                        }
                    } else { held };
                    self.hold_direct(DirectPlanes { source: PlanesHold::Surface { held }, parameters, in_texture: false, timed: false }, "4:4:4");
                    return true;
                }
                Err(error) => { self.fail(format!("binding a {}x{} 4:4:4 decoder picture failed: {error}", layout.width, layout.height), error); return false; }
            }
        }
        self.finish_direct();
        self.say_planes_path(false, "4:4:4");
        match self.planes.as_mut().unwrap().draw_surface(frame, info, &parameters) {
            Ok(()) => true,
            Err(error) => { self.fail(format!("drawing a {}x{} 4:4:4 decoder picture failed: {error}", layout.width, layout.height), error); false }
        }
    }

    /// Draws a software-decoded planar picture (4:4:4, or 10-bit) with the plane shader into an
    /// RGBA64F texture, as the GPU decoder's planes are drawn: SDL has no texture formats for them.
    fn upload_planar(&mut self, renderer: *mut SDL_Renderer, info: &FrameInfo, layout: media::PlanarLayout, white_level: f32) -> bool {
        let Some(parameters) = self.planar_target(renderer, info, layout, white_level, false) else { return false };
        #[cfg(windows)]
        let drawn = {
            if self.planes.is_none() {
                if self.d3d11.is_null() {
                    self.fail(format!("the RGBA64F video texture has no Direct3D 11 texture (renderer {})", crate::sdl::renderer_name(renderer)), "it needs the Direct3D 11 renderer".into());
                    return false;
                }
                // SAFETY: `d3d11` is the live texture of the renderer's device.
                match unsafe { super::d3d11::PlanesPass::new(self.d3d11) } {
                    Ok(pass) => { self.planes = Some(pass); self.pyrowave_format = Some(parameters); }
                    Err(error) => { self.fail(format!("the plane shader pass cannot be made: {error}"), format!("the plane shader pass cannot be made: {error}")); return false; }
                }
            }
            if self.goes_direct(renderer, false) {
                match self.planes.as_mut().unwrap().upload_frame(info) {
                    Ok(()) => {
                        self.hold_direct(DirectPlanes { source: PlanesHold::Frame, parameters, in_texture: false, timed: false }, "planar");
                        return true;
                    }
                    Err(error) => Err(error),
                }
            } else {
                self.finish_direct();
                self.say_planes_path(false, "planar");
                self.planes.as_mut().unwrap().draw_frame(info, &parameters)
            }
        };
        #[cfg(unix)]
        let drawn = {
            if self.planes.is_none() {
                // SAFETY: The live renderer belongs to the presenting thread.
                match unsafe { super::planes_gpu::PlanesPass::for_frames(renderer, layout) } {
                    Ok(pass) => { self.planes = Some(pass); self.pyrowave_format = Some(parameters); }
                    Err(error) => { self.fail(format!("the plane shader pass cannot be made: {error}"), format!("the plane shader pass cannot be made: {error}")); return false; }
                }
            }
            let texture = self.planes_target();
            // SAFETY: The pass was made on this renderer for pictures shaped like `info` (a change
            // of shape changes the parameters, which destroys it), and `info` is the live frame's.
            let drawn = unsafe { self.planes.as_mut().unwrap().draw_frame(renderer, texture, info, &parameters) };
            if drawn.is_ok() { self.planes_drawn(texture, parameters); }
            drawn
        };
        #[cfg(not(any(windows, unix)))]
        let drawn: Result<(), String> = Err("this platform has no plane shader pass".into());
        match drawn {
            Ok(()) => true,
            Err(error) => { self.fail(format!("drawing a {}x{} planar picture failed: {error}", layout.width, layout.height), error); false }
        }
    }

    /// The plane shader's constants for `info` drawn as `layout`, with the RGBA64F texture made for
    /// them (and a pass made for other constants gone); None, the reason said, when the picture
    /// cannot be drawn. `vulkan`: from Vulkan video's copied planes (flag 256 in place of 64).
    fn planar_target(&mut self, renderer: *mut SDL_Renderer, info: &FrameInfo, layout: media::PlanarLayout, white_level: f32, vulkan: bool) -> Option<[u32; 8]> {
        let hdr = shows_hdr(self.hdr_mode, self.hdr_stream, info.color_trc);
        // An HLG picture needs no HDR output: on an SDR one it is drawn for HLG's reference
        // display and SDL's tone map brings it into SDR (`planar_hlg`).
        let hdr_output = crate::sdl::hdr_available(renderer);
        if hdr && info.color_trc != TRC_HLG && !hdr_output {
            self.destroy();
            self.fail(format!("an HDR {}x{} picture needs HDR output, which the renderer {} does not offer", layout.width, layout.height, crate::sdl::renderer_name(renderer)), "the display has no HDR output".into());
            return None;
        }
        // SAFETY: The live renderer belongs to the presenting thread.
        let headroom = if hdr_output { unsafe { SDL_GetFloatProperty(SDL_GetRendererProperties(renderer), SDL_PROP_RENDERER_HDR_HEADROOM_FLOAT, 1.0) } } else { 1.0 };
        let hlg = planar_hlg(info, hdr, white_level, headroom);
        let parameters = interleaved_flags(vulkan_flags(planar_parameters(info, layout, hdr, white_level, headroom), vulkan), vulkan && media::interleaved_444(info));
        if self.pyrowave_format.is_some_and(|previous| previous != parameters) { self.destroy(); }
        // The HLG picture's own peak over its white, which SDL tone maps down to an output with less.
        let texture_headroom = hlg.map(|display| display.peak / display.white);
        self.ensure_with(renderer, SDL_PIXELFORMAT_RGBA64_FLOAT, info.width, info.height, SDL_COLORSPACE_SRGB_LINEAR, white_level, texture_headroom, std::ptr::null_mut()).then_some(parameters)
    }

    /// Draws a Vulkan video picture (`crate::vulkan_video`) as a planar picture of its depth and
    /// chroma: its two planes are copied on the GPU into the presenter's own images (which hands
    /// the picture back to the decoder), and the plane shader draws them with the colour handling
    /// of software-decoded pictures (range, matrix, 10-bit SDR, HDR10, HLG). Nothing goes through
    /// memory.
    #[cfg(unix)]
    fn upload_vulkan(&mut self, renderer: *mut SDL_Renderer, frame: &media::Frame, info: &FrameInfo, layout: media::PlanarLayout, white_level: f32) -> bool {
        let Some(parameters) = self.planar_target(renderer, info, layout, white_level, true) else { return false };
        let what = || format!("{}x{} {}-bit Vulkan video picture", layout.width, layout.height, if layout.ten_bit { 10 } else { 8 });
        if self.vulkan.is_none() {
            let device = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_GPU_DEVICE_POINTER);
            // SAFETY: The renderer's live GPU device, which outlives the texture.
            let made = if media::interleaved_444(info) {
                unsafe { crate::vulkan_video::Planes::interleaved(device.cast(), layout.width, layout.height, layout.ten_bit) }
            } else {
                unsafe { crate::vulkan_video::Planes::new(device.cast(), layout.width, layout.height, layout.ten_bit, layout.chroma444) }
            };
            match made {
                Ok(planes) => self.vulkan = Some(planes),
                Err(error) => { self.fail(format!("the planes of a {} cannot be made: {error}", what()), error); return false; }
            }
        }
        if self.planes.is_none() {
            let textures = self.vulkan.as_ref().map_or([std::ptr::null_mut(); 2], |planes| planes.textures());
            // SAFETY: The live renderer of the presenting thread; the textures live in `vulkan`,
            // which is dropped after the pass.
            match unsafe { super::planes_gpu::PlanesPass::for_vulkan(renderer, textures) } {
                Ok(pass) => { self.planes = Some(pass); self.pyrowave_format = Some(parameters); }
                Err(error) => { self.fail(format!("the plane shader pass cannot be made: {error}"), format!("the plane shader pass cannot be made: {error}")); return false; }
            }
        }
        let texture = self.planes_target();
        let (Some(planes), Some(pass)) = (self.vulkan.as_mut(), self.planes.as_mut()) else { return false };
        // SAFETY: A live Vulkan frame of the decoder on this renderer's device (`device_context`);
        // planes and pass made on this renderer for pictures shaped like it (a change of shape
        // changes the parameters, which destroys them).
        let drawn = unsafe {
            // A VA-API picture is sampled where VA-API wrote it when it can be (the route is logged
            // once); any other, or a surface the driver does not sample, is copied.
            #[cfg(target_os = "linux")]
            let sampled = planes.sample(renderer, frame.raw.as_ptr());
            #[cfg(not(target_os = "linux"))]
            let sampled: Result<Option<[*mut SDL_GPUTexture; 2]>, String> = Ok(None);
            match sampled {
                Ok(Some(textures)) => pass.draw_sampled_textures(renderer, texture, textures, &parameters),
                Ok(None) => {
                    pass.use_copy();
                    planes.copy(renderer, frame.raw.as_ptr()).and_then(|()| pass.draw_bound(renderer, texture, &parameters))
                }
                Err(error) => Err(error),
            }
        };
        match drawn {
            Ok(()) => { self.planes_drawn(texture, parameters); true }
            Err(error) => { self.fail(format!("drawing a {} failed: {error}", what()), error); false }
        }
    }

    /// The packed 4:4:4 counterpart of `upload_vulkan`: a packed picture mapped into Vulkan (or
    /// Vulkan video's) is copied into planes of its coded size and rebuilt from them into the
    /// target; nothing goes through memory (#80).
    #[cfg(unix)]
    fn draw_packed_planes(&mut self, renderer: *mut SDL_Renderer, frame: &media::Frame, coded: media::PlanarLayout, parameters: &[u32; 12]) -> bool {
        let what = || format!("{}x{} {}-bit packed full chroma Vulkan picture", coded.width, coded.height, if coded.ten_bit { 10 } else { 8 });
        if self.vulkan.is_none() {
            let device = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_GPU_DEVICE_POINTER);
            // SAFETY: The renderer's live GPU device, which outlives the planes.
            match unsafe { crate::vulkan_video::Planes::new(device.cast(), coded.width, coded.height, coded.ten_bit, coded.chroma444) } {
                Ok(planes) => self.vulkan = Some(planes),
                Err(error) => { self.fail(format!("the planes of a {} cannot be made: {error}", what()), error); return false; }
            }
        }
        if self.packed.is_none() {
            let textures = self.vulkan.as_ref().map_or([std::ptr::null_mut(); 2], |planes| planes.textures());
            // SAFETY: The live renderer of the presenting thread; the textures live in `vulkan`,
            // which `destroy` drops with the reconstruction.
            match unsafe { super::packed444::Reconstruction::for_planes(renderer, textures) } {
                Ok(packed) => { self.packed = Some(packed); self.packed_format = Some(*parameters); }
                Err(error) => { self.fail(format!("the packed plane pass cannot be made: {error}"), error); return false; }
            }
        }
        let texture = self.texture;
        let (Some(planes), Some(packed)) = (self.vulkan.as_mut(), self.packed.as_ref()) else { return false };
        // SAFETY: A live Vulkan frame of the decoder on this renderer's device; planes made on this
        // renderer for pictures of its size (another size changes the parameters, which destroys them).
        let drawn = unsafe { planes.copy(renderer, frame.raw.as_ptr()) }.and_then(|()| {
            if packed.draw_planes(renderer, texture, parameters) { Ok(()) } else { Err(crate::sdl::error()) }
        });
        match drawn {
            Ok(()) => true,
            Err(error) => { self.fail(format!("drawing a {} failed: {error}", what()), error); false }
        }
    }

    /// A packed picture was rebuilt from Vulkan planes (`draw_packed_planes`), not from memory.
    #[cfg(all(test, unix))]
    pub(super) fn packed_from_planes(&self) -> bool {
        self.packed.is_some() && self.vulkan.is_some()
    }

    pub fn destroy(&mut self) {
        // While the pass that reads it is still there: the set goes back behind its last draw.
        #[cfg(windows)]
        self.finish_direct();
        #[cfg(any(windows, unix))]
        { self.planes = None; }
        self.pyrowave = None;
        self.pyrowave_format = None;
        self.packed = None;
        self.packed_format = None;
        if !self.texture.is_null() {
            // SAFETY: a texture we created.
            unsafe { SDL_DestroyTexture(self.texture) };
        }
        #[cfg(windows)]
        if !self.converted.is_null() {
            self.converted_view = None;
            // SAFETY: a texture we created.
            unsafe { SDL_DestroyTexture(self.converted) };
        }
        // The stage timers belong to the whole stream, not to one texture.
        let stages = std::mem::replace(&mut self.pyrowave_stages, super::stage_times::Stages::new(super::pyrowave_gpu::UPLOAD_AND_PASSES));
        *self = Self {
            #[cfg(windows)]
            direct_packed: self.direct_packed,
            failure: self.failure.take(),
            hdr_mode: self.hdr_mode,
            hdr_metadata: self.hdr_metadata,
            hdr_stream: self.hdr_stream,
            // Facts about the stream, not the texture: `ensure` calls `destroy` before it reads them.
            hdr_frame: self.hdr_frame,
            hlg_frame: self.hlg_frame,
            #[cfg(windows)]
            dolby_vision_ipt_said: self.dolby_vision_ipt_said,
            logged_failure: self.logged_failure.take(),
            pyrowave_stages: stages,
            #[cfg(unix)]
            dmabuf_declined: self.dmabuf_declined,
            #[cfg(unix)]
            direct_planes_failed: self.direct_planes_failed,
            #[cfg(target_os = "linux")]
            video_only: std::mem::replace(&mut self.video_only, VideoOnly::of(None)),
            // The pass outlives the texture; the next pictures are a new scene to it.
            #[cfg(windows)]
            direct: self.direct.take().map(|(device, mut pass)| { pass.reset(); (device, pass) }),
            #[cfg(windows)]
            direct_failed: self.direct_failed,
            #[cfg(windows)]
            direct_planes_failed: self.direct_planes_failed,
            #[cfg(windows)]
            direct_planes_said: self.direct_planes_said,
            ..Self::default()
        };
    }
}

/// The planes to draw after an upload: the new picture's, or when it failed the last one's while
/// the pass and texture that hold them (`held`) are still there and the pass was made for them
/// (`pass_format`, its parameters: a change of shape makes a new pass, whose images never held
/// them), so the last good picture stays on screen and SDL never draws the texture the planes
/// were never drawn into.
#[cfg_attr(not(unix), allow(dead_code))]
fn planes_after_upload(uploaded: bool, new: Option<[u32; 8]>, last: Option<[u32; 8]>, held: bool, pass_format: Option<[u32; 8]>) -> Option<[u32; 8]> {
    if uploaded || new.is_some() || !held || pass_format != last { new } else { last }
}

/// `0` keeps every HDR frame on the PQ pass through the back buffer, as before video-only frames.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub const VIDEO_ONLY_ENVIRONMENT: &str = "NIGHTJAR_PQ_VIDEO_ONLY";

/// How long the first frame that lost draws holds video-only frames off; each further one doubles it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const VIDEO_ONLY_HOLD_US: u64 = 2_000_000;

/// After this many frames that lost draws, a stream draws no more video-only frames.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const VIDEO_ONLY_MISSES: u32 = 4;

/// What a frame on the window's target will draw, for whether it can be video only.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VideoOnlyFrame {
    /// The renderer presents through the patched SDL's PQ pass (`handheld::Output::Pq`).
    pub pq_output: bool,
    /// The plane pass draws the picture straight into the window's target (`draws_planes_direct`),
    /// with no upscaler or AI enhancement drawing it instead.
    pub planes_direct: bool,
    /// Nothing else draws there: no overlay, toast, stream menu or Remote Input screen.
    pub nothing_else: bool,
    /// A read-back of the frame is asked for (a screenshot), which reads the back buffer as scRGB.
    pub read_back: bool,
}

/// HDR frames that draw only the picture, through the patched SDL's `SDL.renderer.gpu.video_only`
/// (`vendor/sdl3-src/README.md`): the plane pass writes PQ into the HDR10 swapchain image itself,
/// and SDL skips the RGBA16F back buffer and its full-screen PQ pass. SDL drops its own draws in
/// such a frame and counts them; a frame that lost any (something drew that `VideoOnlyFrame` did
/// not foresee) holds video-only frames off for a while, longer each time, and after
/// `VIDEO_ONLY_MISSES` for the rest of the stream.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug)]
pub struct VideoOnly {
    switched_on: bool,
    /// The path was logged.
    said: bool,
    /// SDL's count of the draws it dropped, as last read.
    dropped_seen: i64,
    misses: u32,
    held_until_us: u64,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl VideoOnly {
    pub fn of(value: Option<&std::ffi::OsStr>) -> Self {
        Self { switched_on: !value.is_some_and(|value| value == "0"), said: false, dropped_seen: 0, misses: 0, held_until_us: 0 }
    }

    pub fn from_environment() -> Self {
        Self::of(std::env::var_os(VIDEO_ONLY_ENVIRONMENT).as_deref())
    }

    /// Whether to ask SDL for a video-only frame, and a line to log the first time one could be
    /// asked for while the environment switches them off.
    pub fn wanted(&mut self, frame: VideoOnlyFrame, now_us: u64) -> (bool, Option<&'static str>) {
        let could = frame.pq_output && frame.planes_direct && frame.nothing_else && !frame.read_back;
        let line = (could && !self.switched_on && !std::mem::replace(&mut self.said, true))
            .then_some("nightjar: HDR frames go through the back buffer and its PQ pass (NIGHTJAR_PQ_VIDEO_ONLY=0)");
        (could && self.switched_on && self.misses < VIDEO_ONLY_MISSES && now_us >= self.held_until_us, line)
    }

    /// What SDL settled for a frame asked to be video only; a line to log the first time.
    pub fn settled(&mut self, video_only: bool) -> Option<&'static str> {
        (!std::mem::replace(&mut self.said, true)).then_some(if video_only {
            "nightjar: HDR frames with only the picture take PQ straight from the plane pass, without the back buffer and its PQ pass"
        } else {
            "nightjar: SDL did not take a video-only frame; HDR frames go through the back buffer and its PQ pass"
        })
    }

    /// SDL's count of the draws it dropped in video-only frames, after a present; a line to log
    /// when this frame lost any.
    pub fn presented(&mut self, dropped: i64, now_us: u64) -> Option<String> {
        // A renderer made again counts from 0.
        let lost = if dropped < self.dropped_seen { 0 } else { dropped - self.dropped_seen };
        self.dropped_seen = dropped;
        if lost == 0 {
            return None;
        }
        self.misses += 1;
        if self.misses >= VIDEO_ONLY_MISSES {
            return Some(format!("nightjar: SDL dropped {lost} draws of a video-only HDR frame; HDR frames go through the PQ pass for the rest of the stream"));
        }
        let hold_us = VIDEO_ONLY_HOLD_US << (self.misses - 1);
        self.held_until_us = now_us + hold_us;
        Some(format!("nightjar: SDL dropped {lost} draws of a video-only HDR frame; HDR frames go through the PQ pass for {} s", hold_us / 1_000_000))
    }
}

/// A decoded H.264, HEVC or AV1 picture, which may carry an encoder's alignment padding; not a
/// PyroWave or packed 4:4:4 one, whose size the setup gives.
fn plain(decoded: &Decoded) -> bool {
    decoded.setup.chroma_packing == 0 && decoded.setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_PYROWAVE == 0
        && decoded.picture.frame().is_some()
}

/// The `visible` part of a `texture`-sized texture to show, when it is less than the texture;
/// None when the whole texture is, or the part would reach past it.
fn visible_part(visible: (i32, i32), texture: (i32, i32)) -> Option<(i32, i32)> {
    (visible != texture && visible.0 <= texture.0 && visible.1 <= texture.1).then_some(visible)
}

/// The largest rectangle of the picture's aspect ratio inside `area`, centred.
#[cfg(test)]
pub fn letterbox(picture: (i32, i32), area: (i32, i32)) -> SDL_FRect {
    viewport(picture, area, crate::config::VideoScaling::Fit, false)
}

pub fn viewport(picture: (i32, i32), area: (i32, i32), scaling: crate::config::VideoScaling, top: bool) -> SDL_FRect {
    use crate::config::VideoScaling;
    let (aw, ah) = (area.0 as f32, area.1 as f32);
    if picture.0 <= 0 || picture.1 <= 0 || area.0 <= 0 || area.1 <= 0 {
        return SDL_FRect { x: 0.0, y: 0.0, w: aw.max(1.0), h: ah.max(1.0) };
    }
    let (pw, ph) = (picture.0 as f32, picture.1 as f32);
    let (w, h) = match scaling {
        VideoScaling::Stretch => (aw, ah),
        VideoScaling::Native => (pw, ph),
        VideoScaling::Fit | VideoScaling::Fill => {
            let scale = if scaling == VideoScaling::Fit { (aw / pw).min(ah / ph) } else { (aw / pw).max(ah / ph) };
            ((pw * scale).round(), (ph * scale).round())
        }
    };
    SDL_FRect {
        x: ((aw - w) / 2.0).trunc(),
        y: if top && matches!(scaling, VideoScaling::Fit | VideoScaling::Native) { 0.0 } else { ((ah - h) / 2.0).trunc() },
        w,
        h,
    }
}

#[cfg(all(target_os = "macos", feature = "demo-library"))]
#[path = "macos_video_check.rs"]
pub(super) mod macos_video_check;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn a_packed_picture_on_the_other_path_than_the_made_reconstruction_rebuilds_it() {
        assert!(!packed_path_changed(false, true, false), "nothing made yet");
        assert!(!packed_path_changed(true, true, true), "planes for a Vulkan picture");
        assert!(!packed_path_changed(true, false, false), "memory for a picture in memory");
        assert!(packed_path_changed(true, false, true), "the map failed: memory now");
        assert!(packed_path_changed(true, true, false), "a new decoder maps: planes now");
    }

    const VIDEO_ALONE: VideoOnlyFrame = VideoOnlyFrame { pq_output: true, planes_direct: true, nothing_else: true, read_back: false };

    #[test]
    fn a_frame_is_video_only_when_it_draws_only_the_planes_on_a_pq_output_unless_the_environment_says_0() {
        let mut video_only = VideoOnly::of(None);
        assert_eq!(video_only.wanted(VIDEO_ALONE, 0), (true, None));
        for frame in [
            VideoOnlyFrame { pq_output: false, ..VIDEO_ALONE },
            VideoOnlyFrame { planes_direct: false, ..VIDEO_ALONE },
            VideoOnlyFrame { nothing_else: false, ..VIDEO_ALONE },
            VideoOnlyFrame { read_back: true, ..VIDEO_ALONE },
        ] {
            assert_eq!(video_only.wanted(frame, 0), (false, None), "{frame:?}");
        }
        assert!(VideoOnly::of(Some(std::ffi::OsStr::new("1"))).wanted(VIDEO_ALONE, 0).0);
        let mut off = VideoOnly::of(Some(std::ffi::OsStr::new("0")));
        assert_eq!(off.wanted(VideoOnlyFrame { pq_output: false, ..VIDEO_ALONE }, 0), (false, None), "nothing to say off PQ");
        let (wanted, line) = off.wanted(VIDEO_ALONE, 0);
        assert!(!wanted && line.is_some_and(|line| line.contains(VIDEO_ONLY_ENVIRONMENT)), "said once");
        assert_eq!(off.wanted(VIDEO_ALONE, 0), (false, None));
    }

    #[test]
    fn what_sdl_settled_is_said_once() {
        let mut video_only = VideoOnly::of(None);
        assert!(video_only.settled(true).is_some_and(|line| line.contains("straight")));
        assert_eq!(video_only.settled(true), None);
        assert_eq!(video_only.settled(false), None);
        assert!(VideoOnly::of(None).settled(false).is_some_and(|line| line.contains("did not")));
    }

    #[test]
    fn dropped_draws_hold_video_only_frames_off_longer_each_time_then_for_the_stream() {
        let mut video_only = VideoOnly::of(None);
        assert_eq!(video_only.presented(0, 0), None);
        assert!(video_only.presented(3, 1_000).is_some_and(|line| line.contains("dropped 3") && line.contains("for 2 s")));
        assert!(!video_only.wanted(VIDEO_ALONE, 2_000_999).0, "held for 2 s");
        assert!(video_only.wanted(VIDEO_ALONE, 2_001_000).0);
        assert_eq!(video_only.presented(3, 2_001_000), None, "the count did not grow");
        assert!(video_only.presented(4, 3_000_000).is_some_and(|line| line.contains("dropped 1") && line.contains("for 4 s")));
        assert!(!video_only.wanted(VIDEO_ALONE, 6_999_999).0);
        assert!(video_only.wanted(VIDEO_ALONE, 7_000_000).0);
        // A renderer made again starts its count at 0: nothing was lost.
        assert_eq!(video_only.presented(0, 7_000_000), None);
        assert!(video_only.presented(1, 8_000_000).is_some_and(|line| line.contains("for 8 s")));
        assert!(video_only.presented(2, 20_000_000).is_some_and(|line| line.contains("rest of the stream")));
        assert!(!video_only.wanted(VIDEO_ALONE, u64::MAX).0, "off for the stream");
    }

    fn frame(trc: i32, space: i32, range: i32) -> FrameInfo {
        FrameInfo {
            width: 1920,
            height: 1080,
            kind: media::FRAME_D3D11,
            ten_bit: 1,
            pts: 0,
            data: [std::ptr::null_mut(); 3],
            linesize: [0; 3],
            color_space: space,
            color_range: range,
            color_trc: trc,
        }
    }

    #[test]
    fn without_v_sync_tears_except_a_vrr_stream_on_wayland() {
        // (vrr, wayland, gamescope)
        assert!(prefers_immediate(false, false, false), "X11 without V-Sync tears");
        assert!(prefers_immediate(false, true, false), "Wayland without VRR tears");
        assert!(prefers_immediate(true, false, false), "X11 VRR takes IMMEDIATE");
        assert!(!prefers_immediate(true, true, false), "a VRR stream on a Wayland compositor keeps MAILBOX");
        assert!(prefers_immediate(true, true, true), "gamescope's VRR takes IMMEDIATE");
        assert_eq!(crate::sdl::present_mode_name(Some(1)), "Immediate (tears)");
        assert_eq!(crate::sdl::present_mode_name(Some(2)), "Mailbox (no tearing)");
        assert_eq!(crate::sdl::present_mode_name(Some(0)), "V-Sync (FIFO)");
        assert_eq!(crate::sdl::present_mode_name(None), "set by the renderer");
    }

    #[test]
    fn a_pq_picture_is_a_ycbcr_colorspace_so_a_p010_texture_can_be_made() {
        // The YUV conversion matrix of an RGB colorspace (SDL_COLORSPACE_HDR10) does not exist.
        for (range, expected) in [(1, SDL_COLORSPACE_BT2020_LIMITED), (2, SDL_COLORSPACE_BT2020_FULL), (0, SDL_COLORSPACE_BT2020_LIMITED)] {
            let space = colorspace(&frame(16, 9, range));
            assert_eq!(space.0, expected.0);
            assert_ne!(space.0, SDL_COLORSPACE_HDR10.0);
            assert_eq!((space.0 >> 28) & 0xf, SDL_COLOR_TYPE_YCBCR.0, "a YUV texture needs a YCbCr colorspace");
            assert_eq!((space.0 >> 5) & 0x1f, SDL_TRANSFER_CHARACTERISTICS_PQ.0, "PQ must stay the transfer");
        }
        // A PQ stream whose VUI says nothing about the matrix is still BT.2020.
        assert_eq!(colorspace(&frame(16, 2, 0)).0, SDL_COLORSPACE_BT2020_LIMITED.0);
        assert_eq!(colorspace(&frame(1, 1, 0)).0, SDL_COLORSPACE_BT709_LIMITED.0);
    }

    #[test]
    fn the_texture_gets_the_display_white_and_the_peak_as_headroom() {
        let metadata = nightjar_core::control::HdrMetadata { max_display_luminance: 1000, max_content_light_level: 0, ..Default::default() };
        // White level 2.5 is 200 nits; a 1000-nit peak is 5x that.
        assert_eq!(hdr_properties(true, 2.5, Some(metadata)), HdrProperties { white_point: Some(200.0), headroom: Some(5.0) });
        // SDR streams pass no white point.
        assert_eq!(hdr_properties(false, 1.0, None), HdrProperties { white_point: None, headroom: None });
        // A broken display value (zero, NaN, infinity) is never passed on.
        for white in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(hdr_properties(true, white, Some(metadata)), HdrProperties { white_point: None, headroom: None });
        }
        // No metadata, or an empty one, leaves the headroom to SDL.
        assert_eq!(hdr_properties(true, 1.0, None).headroom, None);
        let empty = nightjar_core::control::HdrMetadata::default();
        assert_eq!(hdr_properties(true, 1.0, Some(empty)).headroom, None);
    }

    #[test]
    fn destroying_the_texture_keeps_what_is_known_about_the_stream() {
        // `ensure` destroys first and reads the PQ flag after; a failing stream logs once, not per picture.
        let mut texture = VideoTexture { hdr_frame: true, ..Default::default() };
        texture.fail("detail".into(), "short".into());
        assert_eq!(texture.failure.as_deref(), Some("short"));
        texture.destroy();
        assert!(texture.hdr_frame, "the PQ flag must survive destroy");
        assert!(texture.is_hdr());
        assert_eq!(texture.logged_failure.as_deref(), Some("detail"));
        assert_eq!(texture.failure.as_deref(), Some("short"));
        assert!(hdr_properties(texture.hdr_frame, 2.5, None).white_point == Some(200.0));
        let mut sdr10 = VideoTexture::new(true, false);
        let mut hdr = VideoTexture::new(true, true);
        sdr10.destroy();
        hdr.destroy();
        assert!(!sdr10.hdr_stream && hdr.hdr_stream, "what the session negotiated must survive destroy");
    }

    /// A failed upload keeps the last picture's planes for the present, where SDL would have
    /// drawn the texture they never went into; a picture that is not planes clears them.
    #[test]
    fn a_failed_upload_keeps_the_last_planes_on_screen() {
        let last = Some([1; 8]);
        assert_eq!(planes_after_upload(false, None, last, true, last), last);
        assert_eq!(planes_after_upload(true, None, last, true, last), None, "a decoded picture in the texture replaces them");
        assert_eq!(planes_after_upload(true, Some([2; 8]), last, true, Some([2; 8])), Some([2; 8]));
        assert_eq!(planes_after_upload(false, None, last, false, last), None, "the pass that held them is gone");
        assert_eq!(planes_after_upload(false, None, last, true, Some([2; 8])), None, "a new pass for a new shape never held them");
    }

    /// Plane pictures wait for the back buffer (`draw_direct`) instead of going through the
    /// texture, until a direct draw fails, which holds for the rest of the stream.
    #[cfg(unix)]
    #[test]
    fn plane_pictures_go_straight_to_the_back_buffer_until_that_fails() {
        let mut texture = VideoTexture::default();
        assert!(texture.planes_target().is_null(), "the plane pass must only copy the planes");
        texture.planes_drawn(texture.planes_target(), [7; 8]);
        assert_eq!(texture.planes_pending, Some([7; 8]));
        // No picture texture yet: SDL has nothing to draw, the planes stay for the next present.
        assert!(!texture.draw_direct(std::ptr::null_mut(), &SDL_FRect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 }, std::ptr::null_mut()));
        assert_eq!(texture.planes_pending, Some([7; 8]));
        texture.direct_planes_failed = true;
        texture.destroy();
        assert!(texture.direct_planes_failed, "a failed direct draw must survive destroy");
        assert_eq!(texture.planes_pending, None, "a new texture has no planes of the old one");
        let fake = std::ptr::NonNull::<SDL_Texture>::dangling().as_ptr();
        texture.texture = fake;
        assert_eq!(texture.planes_target(), fake);
        texture.planes_drawn(fake, [7; 8]);
        assert_eq!(texture.planes_pending, None, "a picture drawn into the texture is not pending");
        texture.texture = std::ptr::null_mut();
    }

    #[test]
    fn a_10_bit_sdr_stream_keeps_the_sdr_white_until_the_host_says_hdr() {
        // The bug: every 10-bit picture without the host's HDR mode took the display's SDR white
        // (and HDR conversion), so 10-bit SDR came out too dark or too bright.
        use nightjar_core::sdp::{VIDEO_FORMAT_AV1_MAIN10, VIDEO_FORMAT_H265, VIDEO_FORMAT_H265_MAIN10};
        assert!(!hdr_negotiated(false, VIDEO_FORMAT_H265_MAIN10), "10-bit color (SDR) is not HDR");
        assert!(!hdr_negotiated(false, VIDEO_FORMAT_AV1_MAIN10));
        assert!(hdr_negotiated(true, VIDEO_FORMAT_H265_MAIN10));
        assert!(!hdr_negotiated(true, VIDEO_FORMAT_H265), "HDR asked for, 8-bit chosen");
        let bt709 = 1;
        assert!(!shows_hdr(None, hdr_negotiated(false, VIDEO_FORMAT_H265_MAIN10), bt709));
        assert!(shows_hdr(None, hdr_negotiated(true, VIDEO_FORMAT_H265_MAIN10), bt709));
        // The PQ transfer, and the host's HDR mode once it is known, still decide.
        assert!(shows_hdr(None, false, 16));
        assert!(!shows_hdr(Some(false), true, 16));
        assert!(shows_hdr(Some(true), false, bt709));
    }

    #[test]
    fn planar_pictures_get_the_plane_shader_flags_of_their_colour_description() {
        let info = |kind, width, height, space, range| FrameInfo { kind, width, height, ten_bit: 0, ..frame(1, space, range) };
        let layout = |info: &FrameInfo| media::PlanarLayout::of(info).unwrap();
        // 8-bit 4:4:4, BT.709 limited: matrix 1, 4:4:4 (32).
        let yuv444 = info(media::FRAME_YUV444P, 64, 32, 1, 1);
        assert_eq!(planar_parameters(&yuv444, layout(&yuv444), false, 1.0, 1.0), [64, 32, 64, 32, 0, 3, 4 | 32, 1.0f32.to_bits()]);
        // 10-bit 4:2:0, BT.601 full: full (1), matrix 0, ten (16), FFmpeg's low-bit samples (64).
        let main10 = info(media::FRAME_YUV420P10, 7, 3, 6, 2);
        assert_eq!(planar_parameters(&main10, layout(&main10), false, 1.0, 1.0)[6], 1 | 16 | 64);
        // 10-bit 4:4:4 HDR, BT.2020, white 2.5.
        let hdr = info(media::FRAME_YUV444P10, 16, 16, 9, 1);
        assert_eq!(planar_parameters(&hdr, layout(&hdr), true, 2.5, 4.0)[4..], [0, 3, 2 | 8 | 16 | 32 | 64, 2.5f32.to_bits()]);
        // An HLG one gets the HLG flag (128) beside HDR, so the shader does not read it as PQ;
        // with the host's HDR mode off it is drawn as SDR, without it.
        let hlg = FrameInfo { color_trc: TRC_HLG, ..hdr };
        // Its display comes in place of the wavelet pass's level and operation: a 4 x 200-nit
        // output is an 800-nit HLG display with BT.2100's gamma for it, white staying 2.5.
        let display = super::super::hlg::hlg_display(4.0, 200.0);
        assert_eq!(planar_parameters(&hlg, layout(&hlg), true, 2.5, 4.0)[4..],
            [display.peak.to_bits(), display.gamma.to_bits(), 2 | 8 | 16 | 32 | 64 | 128, 2.5f32.to_bits()]);
        assert_eq!(planar_parameters(&hlg, layout(&hlg), false, 1.0, 1.0)[4..], [0, 3, 8 | 16 | 32 | 64, 1.0f32.to_bits()]);
        // On an SDR output: the 1000-nit reference display with its 203-nit white as SDR white,
        // and the texture says the 1000 / 203 headroom SDL tone maps from.
        assert_eq!(planar_parameters(&hlg, layout(&hlg), true, 1.0, 1.0)[4..],
            [1000.0f32.to_bits(), 1.2f32.to_bits(), 2 | 8 | 16 | 32 | 64 | 128, (203.0f32 / 80.0).to_bits()]);
        let sdr = planar_hlg(&hlg, true, 1.0, 1.0).unwrap();
        assert_eq!(sdr.peak / sdr.white, 1000.0 / 203.0);
        assert!(planar_hlg(&hdr, true, 2.5, 4.0).is_none() && planar_hlg(&hlg, false, 1.0, 1.0).is_none());
        // So an HLG picture keeps the stream going when the output loses HDR; a PQ one ends it.
        let texture = VideoTexture { hlg_frame: true, hdr_mode: Some(true), ..Default::default() };
        assert!(texture.is_hdr() && !texture.needs_hdr_output());
        let texture = VideoTexture { hdr_frame: true, ..Default::default() };
        assert!(texture.needs_hdr_output());
        // Odd 4:2:0 pictures round their chroma up, as FFmpeg does.
        assert_eq!([0, 1, 2].map(|p| layout(&main10).plane_size(p)), [(7, 3), (4, 2), (4, 2)]);
        assert_eq!([0, 1, 2].map(|p| layout(&yuv444).plane_size(p)), [(64, 32); 3]);
        // Only the three planar kinds take this path.
        for kind in [media::FRAME_NV12, media::FRAME_P010, media::FRAME_YUV420P, media::FRAME_D3D11, media::FRAME_WAVELET] {
            assert!(media::PlanarLayout::of(&info(kind, 64, 32, 1, 1)).is_none());
        }
        assert!(media::PlanarLayout::of(&info(media::FRAME_YUV444P, 0, 32, 1, 1)).is_none());
    }

    /// A Vulkan video picture goes to the plane shader as the planar picture of its depth and
    /// chroma, with the colour flags a software-decoded one gets, and flag 256 in place of 64: its
    /// chroma is interleaved and its 10-bit samples sit in the high bits. Never through SDL's P010,
    /// which SDL reads as PQ.
    #[cfg(unix)]
    #[test]
    fn a_vulkan_picture_is_drawn_as_the_planar_picture_of_its_depth_and_chroma() {
        // BT.2020 (matrix 2, flag 8), limited range, SDR.
        let info = |kind, ten_bit, color_trc| FrameInfo { kind, width: 7, height: 5, ten_bit, color_trc, ..frame(1, 9, 1) };
        let layout = |kind, ten_bit| media::PlanarLayout::of_vulkan(&info(kind, ten_bit, 1)).unwrap();
        let shape = |layout: media::PlanarLayout| (layout.chroma444, layout.ten_bit);
        assert_eq!(shape(layout(media::FRAME_VULKAN, 0)), (false, false));
        assert_eq!(shape(layout(media::FRAME_VULKAN, 1)), (false, true));
        assert_eq!(shape(layout(media::FRAME_VULKAN444, 1)), (true, true));
        assert_eq!([0, 1].map(|p| layout(media::FRAME_VULKAN, 0).plane_size(p)), [(7, 5), (4, 3)]);
        for kind in [media::FRAME_NV12, media::FRAME_P010, media::FRAME_DMABUF, media::FRAME_YUV420P10, 4] {
            assert!(media::PlanarLayout::of_vulkan(&info(kind, 1, 1)).is_none(), "kind {kind}");
        }
        assert!(media::PlanarLayout::of(&info(media::FRAME_VULKAN, 1, 1)).is_none());
        // 10-bit SDR: no HDR flag, the 10-bit flag (16) and 256, never 64.
        let sdr10 = info(media::FRAME_VULKAN, 1, 1);
        let flags = vulkan_flags(planar_parameters(&sdr10, layout(media::FRAME_VULKAN, 1), false, 1.0, 1.0), true)[6];
        assert_eq!(flags, 8 | 16 | 256);
        // HDR10 and HLG keep their flags.
        let pq = FrameInfo { color_trc: TRC_PQ, ..sdr10 };
        assert_eq!(vulkan_flags(planar_parameters(&pq, layout(media::FRAME_VULKAN, 1), true, 2.5, 4.0), true)[6], 2 | 8 | 16 | 256);
        let hlg = FrameInfo { color_trc: TRC_HLG, ..pq };
        assert_eq!(vulkan_flags(planar_parameters(&hlg, layout(media::FRAME_VULKAN, 1), true, 2.5, 4.0), true)[6], 2 | 8 | 16 | 128 | 256);
        // 8-bit 4:4:4, full range.
        let full = FrameInfo { color_range: 2, ..info(media::FRAME_VULKAN444, 0, 1) };
        assert_eq!(vulkan_flags(planar_parameters(&full, layout(media::FRAME_VULKAN444, 0), false, 1.0, 1.0), true)[6], 1 | 8 | 32 | 256);
        assert_eq!(vulkan_flags([0, 0, 0, 0, 0, 0, 16 | 64, 0], false)[6], 16 | 64, "other pictures keep 64");
        // A mapped 4:4:4 VAAPI picture: full chroma, 2048 in place of 256, so its whole 10-bit
        // samples are not taken for high bits.
        let interleaved = info(media::FRAME_VULKAN_INTERLEAVED444, 1, 1);
        assert!(media::interleaved_444(&interleaved) && !media::interleaved_444(&sdr10));
        assert_eq!(shape(layout(media::FRAME_VULKAN_INTERLEAVED444, 1)), (true, true));
        let flags = |info: &FrameInfo, interleaved| interleaved_flags(vulkan_flags(planar_parameters(info, layout(info.kind, info.ten_bit), false, 1.0, 1.0), true), interleaved)[6];
        assert_eq!(flags(&interleaved, true), 8 | 16 | 32 | 2048);
        assert_eq!(flags(&info(media::FRAME_VULKAN_INTERLEAVED444, 0, 1), true), 8 | 32 | 2048);
        assert_eq!(flags(&sdr10, false), 8 | 16 | 256, "two-plane pictures keep 256");
    }

    /// Linux hardware decoders (VAAPI, NVDEC, V4L2) hand SDL NV12 and P010 pictures, and SDL read
    /// an HLG P010 picture as PQ. An HLG one goes to the plane shader with the HLG flag; SDR and
    /// PQ ones keep SDL's path (no copy, no added latency).
    #[cfg(unix)]
    #[test]
    fn an_hlg_hardware_picture_is_drawn_as_hlg_on_linux_never_as_pq() {
        let info = |kind, color_trc| FrameInfo { kind, width: 64, height: 32, ten_bit: i32::from(kind == media::FRAME_P010), color_trc, ..frame(1, 9, 1) };
        for kind in [media::FRAME_NV12, media::FRAME_P010, media::FRAME_DMABUF] {
            assert!(hlg_through_planes(&info(kind, TRC_HLG)), "kind {kind}");
            assert!(!hlg_through_planes(&info(kind, TRC_PQ)), "PQ kind {kind} stays SDL's");
            assert!(!hlg_through_planes(&info(kind, 1)), "SDR kind {kind} stays SDL's");
        }
        // Planar pictures already take the plane shader; the others have no NV12 planes.
        for kind in [media::FRAME_YUV420P, media::FRAME_YUV420P10, media::FRAME_WAVELET] {
            assert!(!hlg_through_planes(&info(kind, TRC_HLG)), "kind {kind}");
        }
        // A P010 picture reads as 10-bit 4:2:0 with its samples moved to the low bits (64), and
        // shown as HDR it gets the HLG flag (128), never PQ.
        let p010 = info(media::FRAME_P010, TRC_HLG);
        let layout = media::PlanarLayout::of_semi_planar(&p010).unwrap();
        assert_eq!(layout.plane_size(1), (32, 16));
        assert_eq!(planar_parameters(&p010, layout, true, 2.5, 4.0)[6], 2 | 8 | 16 | 64 | 128);
        let nv12 = info(media::FRAME_NV12, TRC_HLG);
        assert_eq!(planar_parameters(&nv12, media::PlanarLayout::of_semi_planar(&nv12).unwrap(), true, 2.5, 4.0)[6], 2 | 8 | 128);
        assert!(media::PlanarLayout::of_semi_planar(&info(media::FRAME_DMABUF, TRC_HLG)).is_none(), "a surface is mapped first");
    }

    /// The report: an HDR stream showed "Cannot show d3d11 pictures". Needs a Direct3D 11 display.
    #[cfg(windows)]
    #[test]
    #[ignore = "requires a Direct3D 11 display"]
    fn sdl_makes_a_p010_texture_in_the_pq_colorspace_we_choose() {
        let _guard = crate::sdl::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: This test owns SDL and all its resources on one thread.
        unsafe {
            assert!(SDL_Init(SDL_INIT_VIDEO), "{}", crate::sdl::error());
            SDL_SetHint(SDL_HINT_RENDER_DRIVER, c"direct3d11".as_ptr());
            let window = SDL_CreateWindow(c"HDR texture test".as_ptr(), 128, 64, SDL_WINDOW_HIDDEN);
            let renderer = crate::sdl::create_renderer(window, true);
            assert!(!renderer.is_null(), "{}", crate::sdl::error());
            let make = |space: SDL_Colorspace| {
                let metadata = nightjar_core::control::HdrMetadata { max_display_luminance: 1000, ..Default::default() };
                let mut texture = VideoTexture { hdr_frame: true, hdr_mode: Some(true), hdr_metadata: Some(metadata), ..Default::default() };
                // A white level of 2.5 is 200 nits, so a 1000-nit peak is 5x headroom.
                let made = texture.ensure(renderer, SDL_PIXELFORMAT_P010, 1920, 1080, space, 2.5);
                let failure = texture.failure.take();
                let properties = if made { SDL_GetTextureProperties(texture.texture) } else { SDL_PropertiesID(0) };
                let white = SDL_GetFloatProperty(properties, SDL_PROP_TEXTURE_SDR_WHITE_POINT_FLOAT, 0.0);
                let headroom = SDL_GetFloatProperty(properties, SDL_PROP_TEXTURE_HDR_HEADROOM_FLOAT, 0.0);
                texture.destroy();
                (made, failure, white, headroom)
            };
            let (made, why, white, headroom) = make(colorspace(&frame(16, 9, 1)));
            assert!(made, "{why:?}");
            assert_eq!((white, headroom), (200.0, 5.0), "the texture must keep the display's white and the peak");
            let (made, why, ..) = make(SDL_COLORSPACE_HDR10);
            assert!(!made, "SDL accepts an RGB colorspace for P010 now; the HDR10 workaround is unnecessary");
            assert!(why.is_some_and(|why| why.contains("YUV colorspace")));
            SDL_DestroyRenderer(renderer);
            SDL_DestroyWindow(window);
            SDL_Quit();
        }
    }

    #[test]
    fn the_pyrowave_stage_timers_survive_destroying_the_texture() {
        let mut texture = VideoTexture::default();
        texture.pyrowave_stages.add(0, std::time::Instant::now());
        texture.pyrowave_stages.commit();
        texture.destroy();
        assert_eq!(texture.pyrowave_stages.frames(), 1);
        assert!(texture.pyrowave_stages.summary().is_some());
    }

    #[test]
    fn scaling_matches_android_fit_fill_stretch_native_and_top_alignment() {
        use crate::config::VideoScaling::*;
        for (mode, top, expected) in [
            (Fit, false, [0.0, 152.0, 1280.0, 720.0]),
            (Fit, true, [0.0, 0.0, 1280.0, 720.0]),
            (Fill, false, [-270.0, 0.0, 1820.0, 1024.0]),
            (Fill, true, [-270.0, 0.0, 1820.0, 1024.0]),
            (Stretch, true, [0.0, 0.0, 1280.0, 1024.0]),
            (Native, false, [-320.0, -28.0, 1920.0, 1080.0]),
            (Native, true, [-320.0, 0.0, 1920.0, 1080.0]),
        ] {
            let r = viewport((1920, 1080), (1280, 1024), mode, top);
            assert_eq!([r.x, r.y, r.w, r.h], expected);
        }
        let r = viewport((0, 0), (1280, 1024), Native, true);
        assert_eq!([r.x, r.y, r.w, r.h], [0.0, 0.0, 1280.0, 1024.0]);
    }

    #[test]
    fn letterbox_keeps_the_aspect_ratio() {
        let r = letterbox((1920, 1080), (2560, 1600));
        assert_eq!((r.x, r.y, r.w, r.h), (0.0, 80.0, 2560.0, 1440.0));
        let r = letterbox((1920, 1080), (1920, 1080));
        assert_eq!((r.x, r.y, r.w, r.h), (0.0, 0.0, 1920.0, 1080.0));
        let r = letterbox((1280, 1024), (1920, 1080));
        assert_eq!((r.w, r.h), (1350.0, 1080.0));
    }

    #[test]
    fn the_default_config_shows_frames_immediately_with_vrr_and_waits_for_vsync_without() {
        // Measured on a 240 Hz tearing display: HostTimes held frames about 7 ms and had the
        // less even present intervals, so it is opt-in (docs/pyrowave-decode.md).
        let settings = crate::config::Settings::default();
        assert!(!settings.present_at_host_times);
        assert_eq!(pacing(&settings, true), (PacingMode::Immediate, true));
        // No tearing support: VRR is not available and the display's refresh is awaited, as before.
        assert_eq!(pacing(&settings, false), (PacingMode::Vsync, false));
        let no_vrr = crate::config::Settings { vrr: false, ..settings.clone() };
        assert_eq!(pacing(&no_vrr, true), (PacingMode::Vsync, false));
        let no_vsync = crate::config::Settings { vrr: false, vsync: false, ..settings.clone() };
        assert_eq!(pacing(&no_vsync, true), (PacingMode::Immediate, false));
    }

    #[test]
    fn pacing_preserves_vrr_without_host_time_delay() {
        let mut settings = crate::config::Settings { present_at_host_times: true, ..Default::default() };
        assert_eq!(pacing(&settings, true), (PacingMode::HostTimes, true));
        settings.present_at_host_times = false;
        assert_eq!(pacing(&settings, true), (PacingMode::Immediate, true));
        settings.present_at_host_times = true;
        settings.experimental_packed_444 = true;
        let expected = if cfg!(windows) {
            PacingMode::Immediate
        } else {
            PacingMode::HostTimes
        };
        assert_eq!(pacing(&settings, true), (expected, true));
        assert!(settings.present_at_host_times);
    }

    #[test]
    fn only_a_display_that_scans_at_its_own_rate_turns_a_vrr_stream_to_v_sync() {
        use super::super::scanline::{Beam, RaceCapability, Reason, Verdict};
        let capability = RaceCapability { tearing: true, vrr: false, flip_inferred: false, scanline: true };
        let off = |reason| Verdict::Off(reason, capability);
        let fixed = off(Reason::NoVrr);
        let vsync = PacingMode::Vsync;
        assert_eq!(after_display_check(PacingMode::Immediate, &fixed, vsync), PacingMode::Vsync, "the laptop's 120 Hz panel");
        assert_eq!(after_display_check(PacingMode::HostTimes, &fixed, vsync), PacingMode::Vsync);
        assert_eq!(after_display_check(PacingMode::Vsync, &fixed, vsync), PacingMode::Vsync);
        // V-Sync turned off in the settings: what the stream would have without VRR.
        let immediate = PacingMode::Immediate;
        assert_eq!(after_display_check(PacingMode::Immediate, &fixed, immediate), PacingMode::Immediate);
        assert_eq!(after_display_check(PacingMode::HostTimes, &fixed, immediate), PacingMode::Immediate);
        // Still checking, VRR seen, and every reason that says nothing about the refresh keep the mode.
        let beam = Beam { rows: 1440, blank_rows: 200, total_rows: 1640, scan_ns: 4_100_000, refresh_hz: 243.9, lines_per_ms: 400.0,
            vblank_fraction: 0.7, scans: 50, idle_from: 2040, longest_idle_ns: 0, idle_scans: 0 };
        let mut kept = vec![Verdict::Checking, Verdict::On(beam, RaceCapability { vrr: true, flip_inferred: true, ..capability })];
        kept.extend([Reason::NoTearing, Reason::NoScanline, Reason::NoScan, Reason::NoRestart, Reason::Windowed, Reason::NoIdle, Reason::RowsDiffer, Reason::VrrNoScanline].map(off));
        for verdict in &kept {
            for mode in [PacingMode::Immediate, PacingMode::HostTimes, PacingMode::BeamRace, PacingMode::Vsync] {
                assert_eq!(after_display_check(mode, verdict, PacingMode::Vsync), mode, "{verdict:?}");
            }
        }
    }

    #[test]
    fn the_vrr_sync_switch_sets_the_sync_interval_of_tearing_modes_only() {
        assert_eq!(VrrSync::parse(None), VrrSync::Off);
        assert_eq!(VrrSync::parse(Some("off")), VrrSync::Off);
        assert_eq!(VrrSync::parse(Some(" present1 ")), VrrSync::Present1);
        assert_eq!(VrrSync::parse(Some("scan")), VrrSync::Scan);
        assert_eq!(VrrSync::parse(Some("vsync")), VrrSync::Off, "an unknown value is said and left off");
        assert_eq!(VrrSync::default(), VrrSync::Off, "presents on arrival unless measured otherwise");
        for sync in [VrrSync::Off, VrrSync::Present1, VrrSync::Scan] {
            assert_eq!(sync_interval(PacingMode::Vsync, sync), 1);
            assert_eq!(sync_interval(PacingMode::BeamRace, sync), 0, "slices need tearing");
        }
        assert_eq!(sync_interval(PacingMode::Immediate, VrrSync::Off), 0);
        assert_eq!(sync_interval(PacingMode::Immediate, VrrSync::Present1), 1);
        assert_eq!(sync_interval(PacingMode::HostTimes, VrrSync::Present1), 1);
        assert_eq!(sync_interval(PacingMode::Immediate, VrrSync::Scan), 0, "waits on this thread, then tears");
    }

    #[test]
    fn a_vrr_streams_pacing_follows_the_display_check_once_and_sets_the_interval_once() {
        use super::super::scanline::{RaceCapability, Reason, Verdict};
        let capability = RaceCapability { tearing: true, vrr: false, flip_inferred: false, scanline: true };
        let fixed = Verdict::Off(Reason::NoVrr, capability);
        // V-Sync off: no pacing to change, but the present mode follows the display.
        let mut vrr = VrrPacing::new(false, PacingMode::Immediate, VrrSync::Off, PacingMode::Immediate).with_present_check(true);
        let mut mode = PacingMode::Immediate;
        assert!(vrr.display_vrr(), "VRR until the check says otherwise");
        assert_eq!(vrr.step(&mut mode, Some(&Verdict::Checking), 0), None);
        assert!(vrr.display_vrr());
        assert_eq!(vrr.step(&mut mode, Some(&fixed), 0), None, "no new sync interval");
        assert_eq!((mode, vrr.fell_back, vrr.display_vrr()), (PacingMode::Immediate, false, false));
        assert_eq!(vrr.step(&mut mode, Some(&Verdict::Off(Reason::VrrNoScanline, RaceCapability { vrr: true, ..capability })), 0), None);
        assert!(vrr.display_vrr(), "the display refreshes on present again");
        // Without either check, a verdict means nothing to it.
        let mut unchecked = VrrPacing::new(false, PacingMode::Immediate, VrrSync::Off, PacingMode::Immediate);
        unchecked.step(&mut mode, Some(&fixed), 0);
        assert!(unchecked.display_vrr());
        let settings = crate::config::Settings::default();
        let (start, _) = pacing(&settings, true);
        assert_eq!(start, PacingMode::Immediate);
        let mut vrr = VrrPacing::new(true, without_vrr(&settings), VrrSync::Off, start);
        let mut mode = start;
        assert_eq!(vrr.step(&mut mode, None, 0), None, "no probe yet: the renderer keeps interval 0");
        assert_eq!(vrr.step(&mut mode, Some(&Verdict::Checking), 0), None);
        assert_eq!(vrr.step(&mut mode, Some(&fixed), 0), Some(1), "V-Sync waits for the refresh");
        assert_eq!((mode, vrr.fell_back), (PacingMode::Vsync, true));
        assert_eq!(vrr.step(&mut mode, Some(&fixed), 0), None, "given once");
        // V-Sync off in the settings: immediate stays, with nothing for the renderer.
        let no_vsync = crate::config::Settings { vsync: false, present_at_host_times: true, ..Default::default() };
        let (start, _) = pacing(&no_vsync, true);
        assert_eq!(start, PacingMode::HostTimes);
        let mut vrr = VrrPacing::new(true, without_vrr(&no_vsync), VrrSync::Off, start);
        let mut mode = start;
        assert_eq!(vrr.step(&mut mode, Some(&fixed), 0), None);
        assert_eq!((mode, vrr.fell_back), (PacingMode::Immediate, true));
        // NIGHTJAR_VRR_CHECK=0: the verdict is not applied; present1 still sets interval 1.
        let mut vrr = VrrPacing::new(false, PacingMode::Vsync, VrrSync::Present1, PacingMode::Immediate);
        let mut mode = PacingMode::Immediate;
        assert_eq!(vrr.step(&mut mode, Some(&fixed), 0), Some(1));
        assert_eq!((mode, vrr.fell_back), (PacingMode::Immediate, false));
        // A stream that starts on V-Sync has interval 1 already.
        let mut vrr = VrrPacing::new(false, PacingMode::Vsync, VrrSync::Off, PacingMode::Vsync);
        let mut mode = PacingMode::Vsync;
        assert_eq!(vrr.step(&mut mode, None, 0), None);
    }

    #[test]
    fn a_v_sync_fall_back_is_rechecked_and_returns_once_the_display_refreshes_on_present() {
        use super::super::scanline::{Beam, RaceCapability, Reason, Verdict};
        let capability = RaceCapability { tearing: true, vrr: false, flip_inferred: false, scanline: true };
        let fixed = Verdict::Off(Reason::NoVrr, capability);
        let beam = Beam { rows: 2160, blank_rows: 90, total_rows: 2250, scan_ns: 8_310_000, refresh_hz: 120.3, lines_per_ms: 270.8,
            vblank_fraction: 0.45, scans: 80, idle_from: 2521, longest_idle_ns: 17_000_000, idle_scans: 30 };
        let on = Verdict::On(beam, RaceCapability { vrr: true, flip_inferred: true, ..capability });
        let idles = Verdict::Off(Reason::NoIdle, RaceCapability { vrr: true, ..capability });
        let busy = Verdict::Off(Reason::NoIdle, capability);
        let second = 1_000_000;
        let mut vrr = VrrPacing::new(true, PacingMode::Vsync, VrrSync::Off, PacingMode::Immediate);
        let mut mode = PacingMode::Immediate;
        // nightjar#72's resume: no VRR, V-Sync.
        assert!(vrr.probe_tears(mode));
        assert_eq!(vrr.step(&mut mode, Some(&fixed), 0), Some(1));
        assert!(vrr.probe_tears(mode), "rechecked with the tearing it had");
        assert!(!VrrPacing::new(true, PacingMode::Vsync, VrrSync::Off, PacingMode::Vsync).probe_tears(PacingMode::Vsync), "V-Sync from the settings");
        assert!(!vrr.recheck_due(0));
        assert_eq!(vrr.step(&mut mode, Some(&fixed), 0), None);
        assert!(!vrr.recheck_due(RECHECK_FIRST_US - 1), "nothing until the first recheck");
        assert!(vrr.recheck_due(RECHECK_FIRST_US));
        assert!(!vrr.recheck_due(RECHECK_FIRST_US), "asked once");
        // The probe measures again; the old verdict until its next present plans nothing.
        assert_eq!(vrr.step(&mut mode, Some(&fixed), RECHECK_FIRST_US), None);
        assert_eq!(vrr.step(&mut mode, Some(&Verdict::Checking), RECHECK_FIRST_US + second), None);
        assert!(!vrr.recheck_due(100 * RECHECK_MAX_US), "none while one runs");
        // At 120 fps on the 120 Hz display the presents never let it idle, and the calibration
        // saw no idle either: nothing says it refreshes on present, V-Sync stays and waits twice as long.
        let ended = RECHECK_FIRST_US + 2 * second;
        assert_eq!(vrr.step(&mut mode, Some(&busy), ended), None);
        assert_eq!(mode, PacingMode::Vsync);
        assert!(!vrr.recheck_due(ended + 2 * RECHECK_FIRST_US - 1));
        assert!(vrr.recheck_due(ended + 2 * RECHECK_FIRST_US));
        // Now the presents restart the scan: Immediate again, interval 0.
        assert_eq!(vrr.step(&mut mode, Some(&Verdict::Checking), 0), None);
        assert_eq!(vrr.step(&mut mode, Some(&on), 0), Some(0));
        assert_eq!((mode, vrr.fell_back, vrr.recovered), (PacingMode::Immediate, false, true));
        assert!(vrr.probe_tears(mode));
        assert_eq!(vrr.step(&mut mode, Some(&on), 0), None);
        assert!(!vrr.recheck_due(u64::MAX), "nothing to recheck on Immediate");
        // No VRR once more after returning: V-Sync for good, no flapping.
        assert_eq!(vrr.step(&mut mode, Some(&fixed), 0), Some(1));
        for at in [0, RECHECK_MAX_US, u64::MAX] {
            assert_eq!(vrr.step(&mut mode, Some(&fixed), at), None);
            assert!(!vrr.recheck_due(at));
        }
        assert_eq!(vrr.step(&mut mode, Some(&on), 0), Some(0), "the probe's own later verdict still counts");
        // A recheck with no idle to test, on a display the calibration saw idle, is no restart seen:
        // V-Sync stays; the waits never grow past the longest, and a passed check returns.
        let mut vrr = VrrPacing::new(true, PacingMode::Vsync, VrrSync::Off, PacingMode::HostTimes);
        let mut mode = PacingMode::HostTimes;
        vrr.step(&mut mode, Some(&fixed), 0);
        let mut at = 0;
        for _ in 0..10 {
            vrr.step(&mut mode, Some(&fixed), at);
            at += RECHECK_MAX_US;
            assert!(vrr.recheck_due(at));
        }
        assert_eq!(vrr.step(&mut mode, Some(&idles), at), None);
        assert_eq!(mode, PacingMode::Vsync);
        assert_eq!(vrr.step(&mut mode, Some(&on), at), Some(0));
        assert_eq!(mode, PacingMode::HostTimes, "the pacing it had before");
    }

    #[test]
    fn a_linux_vrr_stream_on_a_display_without_vrr_waits_for_the_refresh() {
        use super::super::drm_vrr::{verdict, Reading};
        // nightjar#80 and the Fedora 8600G report: Linux has no scan line, so the check said "no
        // scanline" and the stream stayed immediate on a 144 Hz display without VRR. SDL's GPU
        // renderer takes vsync 0 as MAILBOX first, which at the panel's own rate replaces and
        // repeats pictures ("replaced before they were shown" at the stream's end), and HostTimes
        // was worse still (display wait p95 31-65 ms against 19 ms with V-Sync). DRM's
        // VRR_ENABLED 0 now turns both to V-Sync.
        let off = Reading::Vrr { enabled: false, output: "DP-1".into(), capable: Some(1) };
        let on = Reading::Vrr { enabled: true, output: "DP-1".into(), capable: Some(1) };
        let settings = crate::config::Settings::default();
        for start in [PacingMode::Immediate, PacingMode::HostTimes] {
            let mut vrr = VrrPacing::new(true, without_vrr(&settings), VrrSync::Off, start);
            let mut mode = start;
            assert_eq!(vrr.step(&mut mode, Some(&verdict(true, None)), 0), None, "still reading: the pacing stays");
            assert_eq!(vrr.step(&mut mode, Some(&verdict(true, Some(&off))), 0), Some(1), "V-Sync waits for the refresh");
            assert_eq!((mode, vrr.fell_back), (PacingMode::Vsync, true));
            // A recheck that reads VRR on (the user turned it on) returns to the pacing it had.
            let tears = vrr.probe_tears(mode);
            assert!(tears, "rechecked with the pacing it had");
            assert_eq!(vrr.step(&mut mode, Some(&verdict(tears, Some(&on))), 0), Some(0));
            assert_eq!(mode, start);
        }
        // Where VRR runs (KDE adaptive sync, GNOME VRR, gamescope) nothing changes.
        let mut vrr = VrrPacing::new(true, without_vrr(&settings), VrrSync::Off, PacingMode::Immediate);
        let mut mode = PacingMode::Immediate;
        assert_eq!(vrr.step(&mut mode, Some(&verdict(true, Some(&on))), 0), None);
        assert_eq!((mode, vrr.fell_back), (PacingMode::Immediate, false));
        // Nor where the output cannot be told: today's behaviour.
        assert_eq!(vrr.step(&mut mode, Some(&verdict(true, Some(&Reading::Unknown("two outputs".into())))), 0), None);
        assert_eq!(mode, PacingMode::Immediate);
        // V-Sync off in the settings: immediate, as without VRR.
        let no_vsync = crate::config::Settings { vsync: false, ..Default::default() };
        let mut vrr = VrrPacing::new(true, without_vrr(&no_vsync), VrrSync::Off, PacingMode::HostTimes);
        let mut mode = PacingMode::HostTimes;
        vrr.step(&mut mode, Some(&verdict(true, Some(&off))), 0);
        assert_eq!(mode, PacingMode::Immediate);
    }

    #[test]
    fn pacing_keeps_the_non_vrr_fallback() {
        let mut settings = crate::config::Settings {
            experimental_packed_444: true,
            ..Default::default()
        };
        assert_eq!(pacing(&settings, false), (PacingMode::Vsync, false));
        settings.vrr = false;
        assert_eq!(pacing(&settings, true), (PacingMode::Vsync, false));
        settings.vsync = false;
        assert_eq!(pacing(&settings, true), (PacingMode::Immediate, false));
    }

    /// A video texture of `texture` size holding a decoded picture of that size from a `stream`
    /// of that size, cropped as `pump` and `upload` crop it. The texture pointer is never
    /// dereferenced.
    fn holding(texture: (i32, i32), stream: (i32, i32)) -> VideoTexture {
        let mut video = VideoTexture::new(true, false);
        video.texture = std::ptr::NonNull::dangling().as_ptr();
        (video.width, video.height) = texture;
        video.format = SDL_PIXELFORMAT_NV12;
        video.visible = visible_part(nightjar_core::video::crop::visible_size(texture, &[stream]), texture);
        video
    }

    #[test]
    fn a_padded_av1_picture_is_drawn_without_its_padding() {
        // AMD's AV1 encoder codes a 2184x1968 stream as 2240x1968 (rust/PERFORMANCE_WORK.md).
        let video = holding((2240, 1968), (2184, 1968));
        assert_eq!(video.size(), (2184, 1968));
        let source = video.source_rect(video.texture).expect("SDL draws part of the texture");
        assert_eq!((source.x, source.y, source.w, source.h), (0.0, 0.0, 2183.5, 1967.5), "the 56 padding columns stay out, half a texel in");
        assert!(video.source_rect(std::ptr::NonNull::<SDL_Texture>::dangling().as_ptr().wrapping_add(1)).is_none(), "an upscaled or enhanced texture is all picture");
        // Every scaling mode places the stream's size: Fit fills a window of its aspect ratio.
        let fit = viewport(video.size(), (1092, 984), crate::config::VideoScaling::Fit, false);
        assert_eq!((fit.x, fit.y, fit.w, fit.h), (0.0, 0.0, 1092.0, 984.0));
        let fill = viewport(video.size(), (1092, 984), crate::config::VideoScaling::Fill, false);
        assert_eq!((fill.w, fill.h), (1092.0, 984.0));
        // 1920x1080 is coded as 1920x1082.
        let video = holding((1920, 1082), (1920, 1080));
        assert_eq!(video.size(), (1920, 1080));
        assert_eq!(video.source_rect(video.texture).map(|rect| (rect.w, rect.h)), Some((1919.5, 1079.5)));
    }

    #[test]
    fn a_picture_of_the_stream_size_or_smaller_is_drawn_whole() {
        for (texture, stream) in [((1920, 1080), (1920, 1080)), ((2184, 1968), (2184, 1968)), ((1280, 720), (1920, 1080)), ((2560, 1440), (1920, 1080))] {
            let video = holding(texture, stream);
            assert_eq!(video.size(), texture, "{texture:?} for a {stream:?} stream");
            assert!(video.source_rect(video.texture).is_none(), "SDL draws the whole texture, as before");
        }
        // No stream size known: no crop.
        assert_eq!(holding((2240, 1968), (0, 0)).size(), (2240, 1968));
        // A visible part never reaches past the texture.
        assert_eq!(visible_part((2184, 1968), (2184, 1960)), None);
    }

    #[test]
    fn a_new_texture_drops_the_crop() {
        let mut video = holding((2240, 1968), (2184, 1968));
        video.texture = std::ptr::null_mut();
        video.destroy();
        assert_eq!(video.visible, None);
    }

    #[test]
    fn an_uploaded_padded_picture_is_shown_without_its_padding() {
        // A real decoded 128x128 picture through `upload` on a software renderer (no window), as a
        // 120x120 stream's picture with its padding.
        // SAFETY: SDL calls on a surface and renderer made and used on this thread.
        unsafe {
            let surface = SDL_CreateSurface(256, 256, SDL_PIXELFORMAT_RGBA32);
            assert!(!surface.is_null(), "{}", crate::sdl::error());
            let renderer = SDL_CreateSoftwareRenderer(surface);
            assert!(!renderer.is_null(), "{}", crate::sdl::error());
            let mut video = VideoTexture::new(true, false);
            let mut picture = decoded(0);
            picture.setup.chroma_packing = 0;
            let coded = picture.picture.frame().map(|frame| (frame.info().width, frame.info().height)).unwrap();
            assert_eq!(coded, (128, 128));
            picture.visible = Some(nightjar_core::video::crop::visible_size(coded, &[(120, 120)]));
            assert!(video.upload(renderer, &picture, None), "{:?}", video.failure);
            assert_eq!(video.size(), (120, 120));
            let rect = video.source_rect(video.texture).expect("SDL draws part of the texture");
            assert_eq!((rect.x, rect.y, rect.w, rect.h), (0.0, 0.0, 119.5, 119.5));
            // A picture of the stream's size is drawn whole.
            picture.visible = Some(coded);
            assert!(video.upload(renderer, &picture, None), "{:?}", video.failure);
            assert_eq!(video.size(), coded);
            assert!(video.source_rect(video.texture).is_none());
            // A PyroWave or packed picture is never cropped by it.
            picture.setup.chroma_packing = 1;
            picture.visible = Some((120, 120));
            video.upload(renderer, &picture, None);
            assert_ne!(video.size(), (120, 120));
            video.destroy();
            SDL_DestroyRenderer(renderer);
            SDL_DestroySurface(surface);
        }
    }

    #[cfg(windows)]
    #[test]
    fn the_direct_pass_reads_only_the_stream_part_of_a_padded_picture() {
        let mut video = holding((2240, 1968), (2184, 1968));
        video.d3d11_picture = true;
        video.copied = true;
        video.d3d11 = std::ptr::NonNull::<c_void>::dangling().as_ptr();
        // The pass's shader samples `picture / textureSize` of the 2240-wide texture.
        assert_eq!(video.picture().map(|picture| picture.size), Some((2184, 1968)));
        assert_eq!(video.enhance_source().map(|source| source.picture), Some((2184, 1968)));
    }

    pub(crate) fn decoded(ready_us: u64) -> Decoded {
        let mut decoder =
            media::VideoDecoder::open(media::Codec::Hevc, media::Hardware::None, 1, false).unwrap();
        assert_eq!(
            decoder.send(
                include_bytes!("../../../../app/src/main/assets/packed444/hevc-8.bin"),
                0
            ),
            0
        );
        let media::Received::Frame(frame) = decoder.receive() else {
            panic!("fixture did not decode");
        };
        Decoded {
            picture: media::Picture::Frame(frame),
            setup: nightjar_core::session::VideoSetup {
                format: nightjar_core::sdp::VIDEO_FORMAT_H265,
                width: 128,
                height: 128,
                fps: 60,
                chroma_packing: 1,
                logical_width: 128,
                logical_height: 64,
                color_space: 1,
                color_range: 1,
            },
            rtp: ready_us as u32,
            number: 0,
            receive_us: 0,
            enqueue_us: 0,
            input_us: 0,
            ready_us,
            host_latency: 0,
            early: 0,
            first_block_us: 0,
            race: Default::default(),
            bands: Vec::new(),
            race_seq: 0,
            visible: None,
        }
    }

    #[test]
    fn immediate_pacing_takes_the_newest_ready_picture_without_holding_it() {
        let mut pacer = Pacer::new(PacingMode::Immediate, 16_667);
        assert!(pacer.next_due().is_none());
        for ready in [10, 20, 30] {
            pacer.accept(decoded(ready));
        }
        let (picture, dropped) = pacer.take_due(25);
        let picture = picture.unwrap();
        assert_eq!(picture.decoded.ready_us, 20);
        assert_eq!(picture.present_us, 20);
        assert_eq!(picture.hold_us, 0);
        assert_eq!(dropped, 1);
        assert_eq!(pacer.next_due(), Some(30));
        let (picture, dropped) = pacer.take_due(30);
        assert_eq!(picture.unwrap().decoded.ready_us, 30);
        assert_eq!(dropped, 0);
        assert!(pacer.next_due().is_none());
    }

    #[test]
    fn the_front_picture_says_which_raced_frame_it_is_and_when_it_is_due() {
        let mut pacer = Pacer::new(PacingMode::BeamRace, 16_667);
        assert_eq!(pacer.front_race(), None);
        let mut picture = decoded(40);
        picture.race_seq = 7;
        pacer.accept(picture);
        pacer.accept(decoded(50));
        assert_eq!(pacer.front_race(), Some((7, 40)));
    }

    #[cfg(windows)]
    #[test]
    fn an_hdr_stream_is_pq_without_a_pq_transfer_in_the_bitstream() {
        use super::super::direct::Transfer::{Pq, Sdr};
        // The host's HDR mode decides, not the VUI: a host that leaves the transfer out still streams PQ.
        assert_eq!(picture_transfer(SDL_PIXELFORMAT_P010, Some(true), false, false, false), Pq);
        // Before the HDR mode arrives, the negotiated HDR or the PQ transfer decides.
        assert_eq!(picture_transfer(SDL_PIXELFORMAT_P010, None, true, false, false), Pq);
        assert_eq!(picture_transfer(SDL_PIXELFORMAT_P010, None, false, true, false), Pq);
        // 10-bit SDR, and HDR mode off, are not PQ.
        assert_eq!(picture_transfer(SDL_PIXELFORMAT_P010, None, false, false, false), Sdr);
        assert_eq!(picture_transfer(SDL_PIXELFORMAT_P010, Some(false), true, true, false), Sdr);
        // An 8-bit picture is never PQ.
        assert_eq!(picture_transfer(SDL_PIXELFORMAT_NV12, Some(true), true, true, false), Sdr);
    }

    #[cfg(windows)]
    #[test]
    fn an_hlg_picture_is_hlg_on_every_path_never_pq() {
        use super::super::direct::Transfer::{Hlg, Pq, Sdr};
        // The HLG transfer in the VUI makes the picture HLG, with or without the host's HDR mode
        // or negotiated HDR, 10-bit or 8-bit.
        for (hdr_mode, hdr_stream) in [(Some(true), true), (None, true), (None, false), (Some(true), false)] {
            assert_eq!(picture_transfer(SDL_PIXELFORMAT_P010, hdr_mode, hdr_stream, false, true), Hlg, "{hdr_mode:?} {hdr_stream}");
            assert_eq!(picture_transfer(SDL_PIXELFORMAT_NV12, hdr_mode, hdr_stream, false, true), Hlg, "{hdr_mode:?} {hdr_stream}");
        }
        assert!(shows_hdr(None, false, TRC_HLG), "HLG is HDR");
        // The host's HDR mode off still means SDR, as it does for PQ.
        assert_eq!(picture_transfer(SDL_PIXELFORMAT_P010, Some(false), true, false, true), Sdr);
        // SDL's shaders know no HLG: whatever it is in, SDL gets the pass's conversion.
        assert!(sdl_needs_rgb(SDL_PIXELFORMAT_P010, Hlg) && sdl_needs_rgb(SDL_PIXELFORMAT_NV12, Hlg));
        assert!(!sdl_needs_rgb(SDL_PIXELFORMAT_P010, Pq));
        // And the picture counts as HDR (upscaler gate, HDR output check).
        let texture = VideoTexture { hlg_frame: true, ..Default::default() };
        assert!(texture.is_hdr());
        assert_eq!(texture.transfer(), Sdr, "no picture format yet");
        let texture = VideoTexture { hlg_frame: true, format: SDL_PIXELFORMAT_P010, ..Default::default() };
        assert_eq!(texture.transfer(), Hlg);
    }

    #[cfg(windows)]
    #[test]
    fn sdl_gets_rgb_exactly_when_its_shaders_would_decode_the_picture_wrongly() {
        use super::super::direct::Transfer::{Pq, Sdr};
        // SDL decodes P010 as PQ and NV12 as gamma: every case of the one decision.
        for (format, hdr_mode, hdr_stream, hdr_frame) in [
            (SDL_PIXELFORMAT_P010, Some(true), false, false),
            (SDL_PIXELFORMAT_P010, None, false, false),
            (SDL_PIXELFORMAT_P010, Some(false), true, true),
            (SDL_PIXELFORMAT_NV12, Some(true), true, true),
            (SDL_PIXELFORMAT_NV12, None, false, false),
        ] {
            let transfer = picture_transfer(format, hdr_mode, hdr_stream, hdr_frame, false);
            let sdl_decodes_pq = format == SDL_PIXELFORMAT_P010;
            assert_eq!(sdl_needs_rgb(format, transfer), sdl_decodes_pq != (transfer == Pq), "{} {hdr_mode:?} {hdr_stream} {hdr_frame}", format.0);
        }
        // 10-bit SDR is the case: converted, not shown as PQ.
        assert!(sdl_needs_rgb(SDL_PIXELFORMAT_P010, Sdr));
        assert!(!sdl_needs_rgb(SDL_PIXELFORMAT_P010, Pq));
        assert!(!sdl_needs_rgb(SDL_PIXELFORMAT_NV12, Sdr));
    }

    #[test]
    fn frame_pacing_other_than_lowest_latency_waits_for_the_refresh_without_vrr_only() {
        let mut settings = crate::config::Settings { vrr: false, vsync: false, ..Default::default() };
        assert_eq!(settings.frame_pacing, FramePacing::Latency, "the default keeps today's pacing");
        assert_eq!(pacing(&settings, true).0, PacingMode::Immediate, "lowest latency follows the V-Sync setting");
        for mode in [FramePacing::Balanced, FramePacing::CapFps, FramePacing::Smoothness] {
            settings.frame_pacing = mode;
            assert_eq!(pacing(&settings, true).0, PacingMode::Vsync, "{mode:?}");
        }
        let vrr = crate::config::Settings { frame_pacing: FramePacing::Smoothness, ..Default::default() };
        assert_eq!(pacing(&vrr, true), (PacingMode::Immediate, true), "VRR ignores frame pacing, as on Android");
        assert_eq!(frame_us(60), 16_666);
        assert_eq!(frame_us(0), 250_000, "an unknown rate holds at most 250 ms");
    }

    /// The report: 120 fps on a 120 Hz panel took 22 ms from arrival to the display, 7 ms when it
    /// skips the pictures that would wait, and a 240 Hz panel never waited.
    #[test]
    fn a_stream_with_no_refreshes_to_spare_skips_instead_of_waiting() {
        let skips = |pacing, fps, hz| skips_instead_of_waiting(PacingMode::Vsync, pacing, frame_us(fps), hz);
        for pacing in [FramePacing::Latency, FramePacing::Balanced] {
            for (fps, hz) in [(120, 120.0), (60, 60.0), (60, 59.94), (60, 60.5), (240, 240.0), (120, 60.0), (144, 120.0)] {
                assert!(skips(pacing, fps, hz), "{pacing:?} {fps} fps at {hz} Hz has none to spare");
            }
            for (fps, hz) in [(120, 240.0), (60, 120.0), (90, 120.0), (120, 144.0), (30, 60.0), (115, 120.0)] {
                assert!(!skips(pacing, fps, hz), "{pacing:?} {fps} fps at {hz} Hz drains by itself, and keeps every picture");
            }
            assert!(!skips(pacing, 120, 0.0), "no refresh rate known");
        }
        for pacing in [FramePacing::CapFps, FramePacing::Smoothness] {
            assert!(!skips(pacing, 120, 120.0), "{pacing:?} keeps its pictures in order");
        }
        for mode in [PacingMode::Immediate, PacingMode::HostTimes, PacingMode::BeamRace] {
            assert!(!skips_instead_of_waiting(mode, FramePacing::Latency, frame_us(120), 120.0), "{mode:?} does not wait");
        }
        let pacer = Pacer::new(PacingMode::Vsync, 8_333).with_frame_pacing(FramePacing::Latency, 120);
        assert!(pacer.skips_instead_of_waiting(120.0));
        assert!(!pacer.skips_instead_of_waiting(240.0));
    }

    /// A stream dragged to a faster display, or changed by automatic quality, follows its new rate.
    #[test]
    fn a_new_frame_rate_decides_whether_a_stream_skips() {
        let mut pacer = Pacer::new(PacingMode::Vsync, 16_666).with_frame_pacing(FramePacing::Latency, 60);
        assert!(!pacer.skips_instead_of_waiting(120.0), "60 fps has refreshes to spare at 120 Hz");
        pacer.set_fps(120);
        assert!(pacer.skips_instead_of_waiting(120.0), "120 fps has none");
        assert_eq!(pacer.frame_us(), frame_us(120));
        pacer.set_fps(60);
        assert!(!pacer.skips_instead_of_waiting(120.0));
    }

    /// The next picture replaces a refused one, which is shown after all only if none comes.
    #[test]
    fn a_kept_picture_gives_way_to_a_newer_one_and_is_drawn_once_when_none_comes() {
        let mut kept = Some(1);
        assert_eq!(take_over_kept(None, &mut kept, false), (None, 0), "not yet");
        assert_eq!(kept, Some(1));
        assert_eq!(take_over_kept(None, &mut kept, true), (Some(1), 0), "drawn, so not replaced");
        assert_eq!(kept, None);
        assert_eq!(take_over_kept(None, &mut kept, true), (None, 0), "once");

        let mut kept = Some(1);
        assert_eq!(take_over_kept(Some(2), &mut kept, false), (Some(2), 1), "replaced");
        assert_eq!(kept, None);
        let mut kept = Some(1);
        assert_eq!(take_over_kept(Some(2), &mut kept, true), (Some(2), 1), "the newer one wins when both are due");
        let mut kept = None;
        assert_eq!(take_over_kept(Some(2), &mut kept, false), (Some(2), 0), "nothing to replace");
    }

    /// A still desktop sends no next picture, and the screen must not stay one behind.
    #[test]
    fn a_skipped_picture_is_drawn_after_all_unless_a_newer_one_reaches_the_display() {
        let mut skipped = SkippedPicture::new(REFUSED_HOLD);
        assert!(!skipped.take_due(u64::MAX));
        skipped.skipped(1_000, 8_333, 8_333);
        assert_eq!(skipped.due_us(), Some(13_499), "after the next picture would have come");
        assert!(!skipped.take_due(13_498));
        assert!(!skipped.must_wait());
        assert!(skipped.take_due(13_499));
        assert!(skipped.must_wait(), "nothing later would take its place, so it gets its turn");
        assert!(!skipped.take_due(u64::MAX), "once");
        skipped.shown();
        assert!(!skipped.must_wait());
        skipped.skipped(1_000, 8_333, 8_333);
        skipped.shown();
        assert_eq!(skipped.due_us(), None);
        assert!(!skipped.take_due(u64::MAX), "a newer one showed");
    }

    /// A GPU that is behind gets its work held back: pictures nobody sees would pile up on it.
    #[test]
    fn presents_refused_again_and_again_wait_once() {
        let mut skipped = SkippedPicture::new(REFUSED_HOLD);
        for _ in 0..REFUSED_IN_A_ROW {
            assert!(!skipped.must_wait());
            skipped.skipped(1_000, 8_333, 8_333);
        }
        assert!(skipped.must_wait());
        skipped.shown();
        assert!(!skipped.must_wait());
    }

    /// At 60 Hz the stream arrived 1.6 ms before the refresh: no hold showed 37 pictures a second,
    /// a 1 ms hold 50, 3 ms 60. A display whose deadline is earlier must not stay at half the rate.
    #[test]
    fn too_short_a_hold_grows_until_every_other_picture_stops_being_refused() {
        let (frame, refresh) = (16_666, 16_666);
        let mut skipped = SkippedPicture::new(std::time::Duration::ZERO);
        let mut grew = 0;
        for _ in 0..4_000 {
            if skipped.hold_us >= 3_000 { break; }
            if skipped.skipped(0, frame, refresh).is_some() { grew += 1; }
            skipped.shown();
        }
        assert_eq!((skipped.hold_us, grew), (3_000, 6), "half the pictures refused: it grows step by step");
    }

    #[test]
    fn a_hold_that_grew_comes_back_once_the_pictures_stop_being_refused() {
        let mut skipped = SkippedPicture::new(REFUSED_HOLD);
        while skipped.skipped(0, 8_333, 8_333).is_none() {}
        assert_eq!(skipped.hold_us, 5_500);
        let left = PICTURES_SHORTEN - skipped.pictures;
        for _ in 0..left - 1 { skipped.shown(); }
        assert_eq!(skipped.hold_us, 5_500);
        skipped.shown();
        assert_eq!(skipped.hold(), REFUSED_HOLD, "a step shorter");
        for _ in 0..PICTURES_SHORTEN { skipped.shown(); }
        assert_eq!(skipped.hold(), REFUSED_HOLD, "never below where it started");
    }

    /// A late picture is refused now and then for good, or the hold would only ever grow.
    #[test]
    fn late_pictures_now_and_then_do_not_keep_a_hold_long() {
        let mut skipped = SkippedPicture::new(REFUSED_HOLD);
        while skipped.skipped(0, 8_333, 8_333).is_none() {}
        assert_eq!(skipped.hold_us, 5_500);
        for n in 0..2 * PICTURES_SHORTEN {
            if n % 100 == 0 { skipped.skipped(0, 8_333, 8_333); } else { skipped.shown(); }
        }
        assert_eq!(skipped.hold(), REFUSED_HOLD, "1 in 100 refused");
        // More than that, and it stays: this display needs it.
        while skipped.skipped(0, 8_333, 8_333).is_none() {}
        assert_eq!(skipped.hold_us, 5_500);
        for n in 0..2 * PICTURES_SHORTEN {
            if n % 16 == 0 { skipped.skipped(0, 8_333, 8_333); } else { skipped.shown(); }
        }
        assert_eq!(skipped.hold_us, 5_500, "1 in 16 refused");
    }

    /// Frame statistics for a picture on `refresh`, read on the same refresh.
    fn on_display(count: u32, refresh: u32, at_us: u64) -> crate::timing_csv::Flip {
        crate::timing_csv::Flip { present_count: count.wrapping_add(1), stat_present_count: count, stat_present_refresh: refresh, stat_sync_refresh: refresh, stat_sync_us: at_us }
    }

    /// The report: a picture presented 4 ms into a refresh reached the display two refreshes later.
    #[test]
    fn a_picture_a_refresh_late_counts_as_missed_and_a_gap_in_the_stream_does_not() {
        const REFRESH: u64 = 8_333;
        let mut watch = MissWatch::default();
        watch.presented(10, 1_000);
        assert!(!watch.seen(on_display(10, 100, 8_333), REFRESH), "the first statistics have nothing before them");
        watch.presented(11, 9_400);
        assert!(!watch.seen(on_display(11, 101, 16_666), REFRESH), "the next refresh");
        assert!(!watch.seen(on_display(11, 101, 16_666), REFRESH), "the same picture again");
        watch.presented(12, 20_700);
        assert!(watch.seen(on_display(12, 103, 33_332), REFRESH), "presented 12.6 ms before it was shown, and a refresh repeated");
        // The stream paused: the next picture came just before its refresh.
        watch.presented(13, 78_000);
        assert!(!watch.seen(on_display(13, 109, 83_330), REFRESH), "a gap, not a miss");
        // Read a refresh after the flip.
        watch.presented(14, 92_000);
        let read_later = crate::timing_csv::Flip { stat_sync_refresh: 112, stat_sync_us: 108_329, ..on_display(14, 111, 0) };
        assert!(!watch.seen(read_later, REFRESH), "under a refresh from the present to the flip, whenever it was read");
        // Statistics that skipped a picture still add up.
        let mut watch = MissWatch::default();
        assert!(!watch.seen(on_display(20, 200, 8_333), REFRESH));
        watch.presented(22, 0);
        assert!(!watch.seen(on_display(22, 202, 25_000), REFRESH), "two pictures on two refreshes");
        // The counters wrap.
        let mut watch = MissWatch::default();
        assert!(!watch.seen(on_display(u32::MAX, u32::MAX, 8_333), REFRESH));
        watch.presented(1, 0);
        assert!(watch.seen(on_display(1, 2, 33_000), REFRESH), "two pictures on three refreshes, across the wrap");
        // A renderer without statistics.
        assert!(!MissWatch::default().seen(Default::default(), REFRESH));
    }

    #[test]
    fn many_missed_refreshes_make_each_picture_wait_its_turn() {
        const REFRESH: u64 = 8_333;
        let mut skipped = SkippedPicture::new(REFUSED_HOLD);
        for _ in 0..MISSED_OF { skipped.shown(); }
        for _ in 0..2 * MISSED_WAIT { assert_eq!(skipped.missed(REFRESH, REFRESH), None, "one bad spell passes"); }
        for _ in 0..MISSED_OF { skipped.shown(); }
        for _ in 0..MISSED_WAIT - 1 { assert_eq!(skipped.missed(REFRESH, REFRESH), None); }
        assert_eq!(skipped.hold(), REFUSED_HOLD, "now and then is the price of no queue");
        assert_eq!(skipped.missed(REFRESH, REFRESH), Some(std::time::Duration::from_micros(REFRESH + HOLD_STEP_US)));
        assert!(skipped.hold() > std::time::Duration::from_micros(REFRESH), "a whole refresh always gets its turn");
        for _ in 0..2 * MISSED_WAIT { assert_eq!(skipped.missed(REFRESH, REFRESH), None, "it holds no longer than that"); }
        // It comes back, and tries again.
        let mut looks = 0;
        while skipped.hold() > REFUSED_HOLD {
            for _ in 0..PICTURES_SHORTEN { skipped.shown(); }
            looks += 1;
        }
        assert_eq!(looks, 8, "half a minute a step at 120 Hz");
    }

    #[test]
    fn misses_spread_over_many_pictures_leave_the_hold_as_it_is() {
        let mut skipped = SkippedPicture::new(REFUSED_HOLD);
        for _ in 0..MISSED_OF { skipped.shown(); }
        for _ in 0..20 {
            for _ in 0..MISSED_WAIT - 1 { assert_eq!(skipped.missed(8_333, 8_333), None); }
            for _ in 0..MISSED_OF { skipped.shown(); }
        }
        assert_eq!(skipped.hold(), REFUSED_HOLD);
    }

    /// A spell on Wi-Fi: a window of many misses among clean ones.
    #[test]
    fn a_bad_spell_now_and_then_leaves_the_hold_as_it_is() {
        let mut skipped = SkippedPicture::new(REFUSED_HOLD);
        for _ in 0..MISSED_OF { skipped.shown(); }
        for _ in 0..10 {
            for _ in 0..2 * MISSED_WAIT { assert_eq!(skipped.missed(8_333, 8_333), None); }
            for _ in 0..2 * MISSED_OF { skipped.shown(); }
        }
        assert_eq!(skipped.hold(), REFUSED_HOLD);
    }

    /// Every run missed most in its first seconds, while the window was still being placed.
    #[test]
    fn misses_while_a_stream_settles_do_not_count() {
        let mut skipped = SkippedPicture::new(REFUSED_HOLD);
        for _ in 0..MISSED_OF - 1 {
            assert_eq!(skipped.missed(8_333, 8_333), None);
            skipped.shown();
        }
        assert_eq!(skipped.hold(), REFUSED_HOLD);
        skipped.shown();
        for _ in 0..MISSED_WAIT - 1 { assert_eq!(skipped.missed(8_333, 8_333), None, "the count starts here"); }
        assert_eq!(skipped.missed(8_333, 8_333), None, "the first window of many");
        for _ in 0..MISSED_OF { skipped.shown(); }
        for _ in 0..MISSED_WAIT - 1 { assert_eq!(skipped.missed(8_333, 8_333), None); }
        assert!(skipped.missed(8_333, 8_333).is_some(), "the second in a row");
    }

    #[test]
    fn the_desktop_deadline_stays_under_most_of_a_refresh() {
        assert_eq!(deadline_before_refresh(60.0), 7_166);
        assert_eq!(deadline_before_refresh(120.0), 5_000);
        assert_eq!(deadline_before_refresh(240.0), 2_500);
        assert_eq!(deadline_before_refresh(0.0), 0, "no display timing");
    }

    #[test]
    fn the_phase_lock_aims_at_the_deadline_only_where_a_picture_has_to_make_it() {
        assert_eq!(phase_lock_aim_us(true, false, None, 120.0), 5_000);
        assert_eq!(phase_lock_aim_us(true, false, Some(1_000), 120.0), 1_000, "a measurement run's");
        assert_eq!(phase_lock_aim_us(false, false, None, 120.0), 0, "a present that waits, Linux's among them");
        assert_eq!(phase_lock_aim_us(true, true, None, 120.0), 0, "exclusive fullscreen");
    }

    /// 120 fps on 60 Hz waiting its turn was the 51 ms queue.
    #[test]
    fn a_stream_faster_than_the_display_never_waits_for_its_misses() {
        let mut skipped = SkippedPicture::new(REFUSED_HOLD);
        for _ in 0..MISSED_OF { skipped.shown(); }
        for _ in 0..4 {
            for _ in 0..2 * MISSED_WAIT { assert_eq!(skipped.missed(8_333, 16_666), None); }
            for _ in 0..MISSED_OF { skipped.shown(); }
        }
        assert_eq!(skipped.hold(), REFUSED_HOLD);
    }

    /// 17 ms grown at 60 Hz would be a queue at 120 Hz.
    #[test]
    fn another_display_starts_from_the_first_hold() {
        let mut skipped = SkippedPicture::new(REFUSED_HOLD);
        while skipped.skipped(0, 16_666, 16_666).is_none() {}
        assert!(skipped.hold() > REFUSED_HOLD);
        skipped.restart();
        assert_eq!(skipped.hold(), REFUSED_HOLD);
        for _ in 0..REFUSED_OF_64_WIDENS - 1 { assert_eq!(skipped.skipped(0, 8_333, 8_333), None, "what it counted went too"); }
        // The new display settles as a new stream does.
        let mut skipped = SkippedPicture::new(REFUSED_HOLD);
        for _ in 0..MISSED_OF { skipped.shown(); }
        skipped.restart();
        for _ in 0..2 * MISSED_WAIT { assert_eq!(skipped.missed(8_333, 8_333), None); }
        for _ in 0..MISSED_OF { skipped.shown(); }
        for _ in 0..MISSED_WAIT { assert_eq!(skipped.missed(8_333, 8_333), None, "those did not count"); }
    }

    #[test]
    fn a_late_picture_now_and_then_leaves_the_hold_as_it_is() {
        let (frame, refresh) = (8_333, 8_333);
        let mut skipped = SkippedPicture::new(REFUSED_HOLD);
        for n in 0..100_000 {
            if n % 16 == 0 {
                assert_eq!(skipped.skipped(0, frame, refresh), None, "1 in 16, more than the 0.5 to 1% of the runs");
            }
            skipped.shown();
        }
        assert_eq!(skipped.hold(), REFUSED_HOLD);
    }

    #[test]
    fn the_hold_stops_at_a_refresh_and_never_grows_on_a_stream_faster_than_the_display() {
        let mut skipped = SkippedPicture::new(REFUSED_HOLD);
        for _ in 0..100_000 {
            skipped.skipped(0, 8_333, 8_333);
        }
        assert_eq!(skipped.hold_us, 8_500, "a hold longer than a refresh always gets its turn");
        let mut faster = SkippedPicture::new(REFUSED_HOLD);
        for _ in 0..100_000 {
            assert_eq!(faster.skipped(0, 8_333, 16_666), None, "120 fps at 60 Hz is refused by design");
        }
    }

    /// The real swap chain: presents back to back never wait for a refresh, the ones the display
    /// has no room for say so, and SDL's next draw still reaches the back buffer. Needs a display
    /// that shows the window, which a hidden one is not.
    #[cfg(windows)]
    #[test]
    #[ignore = "requires a Direct3D 11 display, and shows a small window"]
    fn a_present_without_waiting_is_refused_while_the_last_picture_waits() {
        let _guard = crate::sdl::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: This test owns SDL and all its resources on one thread.
        unsafe {
            assert!(SDL_Init(SDL_INIT_VIDEO), "{}", crate::sdl::error());
            SDL_SetHint(SDL_HINT_RENDER_DRIVER, c"direct3d11".as_ptr());
            let window = SDL_CreateWindow(c"Present test".as_ptr(), 128, 64, SDL_WindowFlags(0));
            let renderer = crate::sdl::create_renderer(window, true);
            assert!(!renderer.is_null(), "{}", crate::sdl::error());
            assert!(SDL_SetRenderVSync(renderer, 1));
            let clear = |red: u8| {
                assert!(SDL_SetRenderDrawColor(renderer, red, 0, 0, 255));
                assert!(SDL_RenderClear(renderer));
            };
            // Reading back takes longer than a refresh, so not during the burst.
            let drawn = |red: u8| {
                clear(red);
                let surface = SDL_RenderReadPixels(renderer, std::ptr::null());
                assert!(!surface.is_null(), "{}", crate::sdl::error());
                let (mut r, mut g, mut b, mut a) = (0, 0, 0, 0);
                assert!(SDL_ReadSurfacePixel(surface, 0, 0, &mut r, &mut g, &mut b, &mut a));
                SDL_DestroySurface(surface);
                assert_eq!(r, red, "the draw after a present must reach the back buffer");
            };
            let settle = |renderer| for _ in 0..5 { SDL_PumpEvents(); SDL_RenderPresent(renderer); };
            settle(renderer);
            let (mut shown, mut refused, mut longest) = (0, 0, std::time::Duration::ZERO);
            for n in 0..60u8 {
                clear(n + 1);
                let begun = std::time::Instant::now();
                match present_without_waiting(renderer, std::time::Duration::ZERO) {
                    NoWait::Shown => shown += 1,
                    NoWait::Refused => refused += 1,
                    NoWait::Failed => panic!("the present failed"),
                }
                longest = longest.max(begun.elapsed());
                // A burst over several refreshes: some find the last picture gone, most do not.
                SDL_DelayPrecise(1_000_000);
            }
            assert!(shown >= 1, "shown {shown}, refused {refused}");
            assert!(refused >= 1, "a picture a millisecond, the display has no room for all: shown {shown}");
            assert!(longest < std::time::Duration::from_millis(4), "a present waited {longest:?}, shown {shown}, refused {refused}");
            drawn(99);
            // With a hold as long as a refresh it gets its turn.
            let before = shown;
            for _ in 0..3 {
                drawn(200);
                if present_without_waiting(renderer, std::time::Duration::from_millis(100)) == NoWait::Shown { shown += 1; }
            }
            assert_eq!(shown, before + 3, "a hold longer than a refresh always gets its turn");
            drawn(201);
            SDL_DestroyRenderer(renderer);
            SDL_DestroyWindow(window);
            SDL_Quit();
        }
    }

    // Regression (nightjar#80): PC's frame times held pictures by the lateness a decoder behind
    // the stream made, up to the longest hold.
    #[test]
    fn host_times_show_a_picture_at_once_while_the_decoder_is_behind() {
        let mut pacer = Pacer::new(PacingMode::HostTimes, 0);
        let mut reference = pacer.clock.clone();
        let mut ready = 0;
        let mut accept = |pacer: &mut Pacer, queued: usize| {
            ready += 10_000;
            let rtp = (ready * 9 / 100) as u32;
            let mut picture = decoded(ready);
            picture.rtp = rtp;
            pacer.set_decode_queue(queued);
            pacer.accept(picture);
            let last = crate::stream::pipeline::lock(&pacer.pending).back().map(|p| (p.present_us, p.hold_us));
            (rtp, ready, last.unwrap())
        };
        // Review of nightjar#80: a network burst of three frames leaves the queue non-empty for
        // two of them; they are paced as usual.
        for queued in [2, 1, 0] {
            let (rtp, ready, got) = accept(&mut pacer, queued);
            let s = reference.schedule(rtp, ready);
            assert_eq!(got, (s.present_us, s.hold_us), "burst frame with {queued} queued");
        }
        // A decoder that cannot keep up keeps the queue non-empty: after a few frames each
        // picture is shown as soon as it is ready and its lateness is not learnt.
        for i in 1..=12 {
            let (rtp, ready, got) = accept(&mut pacer, 3);
            let s = if i < rubylight_protocol::present_clock::BEHIND_AFTER_FRAMES {
                reference.schedule(rtp, ready)
            } else {
                reference.schedule_behind(rtp, ready)
            };
            assert_eq!(got, (s.present_us, s.hold_us), "backlog frame {i}");
        }
        // Caught up: scheduled as before.
        let (rtp, ready, got) = accept(&mut pacer, 0);
        let s = reference.schedule(rtp, ready);
        assert_eq!(got, (s.present_us, s.hold_us));
    }

    #[test]
    fn lowest_and_balanced_pacing_show_the_newest_due_picture() {
        for mode in [FramePacing::Latency, FramePacing::Balanced] {
            let mut pacer = Pacer::new(PacingMode::Vsync, 8_333).with_frame_pacing(mode, 60);
            for ready in [1_000, 9_000, 17_000] {
                pacer.accept(decoded(ready));
            }
            let (picture, dropped) = pacer.take_due(17_000);
            assert_eq!((picture.unwrap().decoded.ready_us, dropped), (17_000, 2), "{mode:?}");
        }
    }

    #[test]
    fn smoothest_pacing_shows_each_picture_in_turn_and_skips_only_stale_ones() {
        let mut pacer = Pacer::new(PacingMode::Vsync, 8_333).with_frame_pacing(FramePacing::Smoothness, 60);
        for ready in [1_000, 9_000, 17_000] {
            pacer.accept(decoded(ready));
        }
        let (picture, dropped) = pacer.take_due(17_000);
        assert_eq!((picture.unwrap().decoded.ready_us, dropped), (1_000, 0), "16 ms old: still within a frame");
        let (picture, dropped) = pacer.take_due(26_000);
        assert_eq!((picture.unwrap().decoded.ready_us, dropped), (17_000, 1), "17 ms old with a newer one due: skipped");
        assert!(pacer.next_due().is_none());
        pacer.accept(decoded(30_000));
        let (picture, dropped) = pacer.take_due(60_000);
        assert_eq!((picture.unwrap().decoded.ready_us, dropped), (30_000, 0), "a lone late picture still shows");
        let mut vrr = Pacer::new(PacingMode::Immediate, 8_333).with_frame_pacing(FramePacing::Smoothness, 60);
        vrr.accept(decoded(1_000));
        vrr.accept(decoded(2_000));
        assert_eq!(vrr.take_due(2_000).1, 1, "without V-Sync the setting does not apply");
    }

    #[test]
    fn the_fps_limit_gives_each_picture_its_own_slot_holding_it_at_most_a_frame() {
        let mut pacer = Pacer::new(PacingMode::Vsync, 8_333).with_frame_pacing(FramePacing::CapFps, 60);
        // Two pictures that came 2 ms apart, then one late.
        for ready in [10_000, 12_000, 60_000] {
            pacer.accept(decoded(ready));
        }
        let (picture, dropped) = pacer.take_due(12_000);
        let picture = picture.unwrap();
        assert_eq!((picture.decoded.ready_us, picture.present_us, picture.hold_us, dropped), (10_000, 10_000, 0, 0));
        assert_eq!(pacer.next_due(), Some(26_666), "the early one waits for the next slot");
        assert!(pacer.take_due(20_000).0.is_none());
        let (picture, dropped) = pacer.take_due(26_666);
        let picture = picture.unwrap();
        assert_eq!((picture.decoded.ready_us, picture.hold_us, dropped), (12_000, 14_666, 0));
        assert_eq!(pacer.next_due(), Some(60_000), "a late picture starts the slots again");
        pacer.accept(decoded(60_500));
        pacer.accept(decoded(61_000));
        assert_eq!(pacer.take_due(100_000).1, 2, "pictures more than a frame old give way to the newest due one");
    }
}
