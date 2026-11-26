//! When each picture goes to the screen: the pacing mode a stream gets, what VRR does with the display
//! check's verdict, presenting without waiting for a refresh, and the `Pacer` that holds decoded pictures
//! for their turn.

use super::*;

#[path = "skipped_picture.rs"]
mod skipped_picture;
pub use skipped_picture::*;

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
/// pacing is the lowest latency (every other frame pacing waits for the refresh, and so do the
/// PC's frame times, whose pictures go out for a refresh).
pub fn without_vrr(settings: &crate::config::Settings) -> PacingMode {
    if settings.vsync || settings.frame_pacing.forces_vsync() || settings.host_frame_times { PacingMode::Vsync } else { PacingMode::Immediate }
}

/// The display's refresh grid for the PC's frame times without VRR, from a refresh time on the
/// session clock and the refresh period: a picture goes out a quarter of a refresh before the
/// desktop's deadline for its refresh (`Grid::lead_us`), and a time within half a refresh after
/// a refresh takes that one (`Grid::slack_us`), as Android's grid does.
pub fn refresh_grid(refresh_us: i64, period_ns: i64) -> Grid {
    let interval_us = period_ns / 1000;
    Grid { anchor_us: refresh_us, interval_us, lead_us: interval_us / 4, slack_us: interval_us / 2 }
}

/// What the phase lock adds to its aim while the PC's frame times place each picture on the grid:
/// with the pictures right at the deadline, ordinary jitter moves them between two refreshes, so
/// the PC keeps them a little earlier (Android's `SCHEDULED_LATCH_ROOM_NS`).
pub use nightjar_policy::vsync::HOST_GRID_LATCH_ROOM_US;

/// Whether a stream asks the GPU renderer for IMMEDIATE before MAILBOX when it presents without
/// V-Sync (`sdl::prefer_immediate`): MAILBOX never tears, so it would hold each picture for the
/// next refresh. moonlight-qt's Vulkan renderer (plvk.cpp) and chiaki-ng take IMMEDIATE with
/// V-Sync off. A VRR stream on a Wayland compositor keeps MAILBOX and gamescope takes IMMEDIATE,
/// as Nonary's moonlight-qt fork does (plvkpresentation.h).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn prefers_immediate(vrr: bool, wayland: bool, gamescope: bool) -> bool {
    !(vrr && wayland && !gamescope)
}

/// Whether the frame pacing shows every picture in turn, one a refresh, instead of the newest due
/// one, while the mode is V-Sync (`Pacer::keeps_every_picture`): the FPS limit, the smoothest
/// pacing, and Balanced without Keep only the newest frame. The PC's frame times take the lowest
/// latency's newest picture.
pub fn in_order_pacing(frame_pacing: FramePacing, drop_late_frames: bool, host_frame_times: bool) -> bool {
    !host_frame_times
        && (matches!(frame_pacing, FramePacing::CapFps | FramePacing::Smoothness) || (frame_pacing == FramePacing::Balanced && !drop_late_frames))
}

/// Whether a V-Sync stream on the GPU (Vulkan) renderer under gamescope asks for MAILBOX instead
/// of FIFO (Nightjar #181). Gamescope's Vulkan layer gives the driver MAILBOX and runs FIFO
/// itself, in its own queue of commits, each FIFO commit held behind the one before it; with the
/// swapchain's three images a present then waits for gamescope to let go of one. Once that queue
/// held pictures back it did not drain: after a change of the game's rate, a 60 fps stream on a
/// 120 Hz Odin 2 Portal showed 34-42 fps with p95 present waits of 27.9 ms for the rest of the
/// stream, while every picture arrived and decoded in 1 ms. Nothing on Nightjar's side kept that
/// state: the pacer takes the newest due picture, and the PC's phase lock stood aside throughout.
/// MAILBOX does not tear either: gamescope shows the newest finished picture at its refresh, and
/// keeps no queue of them. Only the pacings that take the
/// newest due picture ask for it; the in-order ones keep FIFO's one picture a refresh, and other
/// renderers do not go through gamescope's layer.
pub fn gamescope_mailbox(mode: PacingMode, in_order: bool, gamescope: bool, vulkan: bool) -> bool {
    mode == PacingMode::Vsync && !in_order && gamescope && vulkan
}

/// The longest a picture waits in the in-order frame pacings before a newer due one replaces it:
/// one frame of the stream, at most 250 ms (Android's `FramePacingPolicy.shouldDrainOutput`).
pub fn frame_us(fps: u32) -> u64 {
    (1_000_000 / u64::from(fps.max(1))).min(nightjar_policy::vsync::MAX_QUEUED_FRAME_US)
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

#[path = "vrr.rs"]
mod vrr;
pub use vrr::*;

/// How long a picture a V-Sync present refused tries again before it gives way to the next one.
/// A picture later than the desktop's deadline misses the refresh whatever happens, so it may as
/// well wait for its turn; skipping it would show the last picture twice. Earlier than that, the next
/// picture comes in time for the same refresh, so nothing is lost. A source faster than the stream
/// moves a phase-locked stream a picture at a time, 4.2 ms from a 240 Hz monitor, which at 120 Hz
/// lands up to 5 ms past a refresh: 3 ms repeated up to 1.9% there, 5 ms up to 0.65%.
pub const REFUSED_HOLD: std::time::Duration = std::time::Duration::from_millis(5);

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

/// Whether fractional V-Sync stands aside for VRR: the user chose VRR where this display offers it
/// (the launch plan's `vrr`, which the settings screen hides the row on), even where the display
/// check turned the stream to V-Sync pacing, or the pacing shows pictures as they come. The PC's
/// frame times place each picture on a refresh of their own (`Pacer::host_grid`), so it stands
/// aside for them too. A row the screen hides never holds a picture.
pub fn fractional_vsync_stands_aside(vrr_chosen: bool, mode: PacingMode, host_frame_times: bool) -> bool {
    vrr_chosen || mode != PacingMode::Vsync || host_frame_times
}

/// The draws a held picture's margin follows (`DrawTime`): 32, half a second at 60 fps.
pub const DRAW_TIMES: usize = 32;

/// How long the draws that come after fractional V-Sync's hold took: with Frame Warp the hold runs
/// before the shift, the upscaler or AI enhancement and the overlay, so their time comes out of
/// the margin unless it is added to it (`held_margin_us`).
#[derive(Clone, Debug, Default)]
pub struct DrawTime {
    times: VecDeque<u64>,
}

impl DrawTime {
    pub fn record(&mut self, us: u64) {
        if self.times.len() == DRAW_TIMES { self.times.pop_front(); }
        self.times.push_back(us);
    }

    /// The 90th percentile of the last `DRAW_TIMES` draws, None before the first.
    pub fn p90_us(&self) -> Option<u64> {
        if self.times.is_empty() { return None; }
        let mut sorted: Vec<u64> = self.times.iter().copied().collect();
        sorted.sort_unstable();
        Some(sorted[(sorted.len() * 9).div_ceil(10) - 1])
    }

    pub fn reset(&mut self) {
        self.times.clear();
    }
}

/// The base of a held picture's margin (`held_margin_us`): `set_us`, a measurement run's
/// `NIGHTJAR_HELD_MARGIN_US`, else 2 ms where nothing composes (Windows' exclusive fullscreen, a KMS plane)
/// and the desktop's deadline elsewhere (`deadline_before_refresh`).
pub fn held_base_margin_us(set_us: Option<u64>, uncomposed: bool, refresh_hz: f32) -> u64 {
    set_us.unwrap_or_else(|| if uncomposed { 2_000 } else { deadline_before_refresh(refresh_hz).max(0) as u64 })
}

/// Whether a window in SDL's fullscreen with a display mode (`exclusive`) is shown without a
/// compositor: on Windows (`windows`) only. SDL emulates that fullscreen on Wayland, where the
/// compositor still schedules each picture and its deadline holds.
pub fn uncomposed_fullscreen(exclusive: bool, windows: bool) -> bool {
    exclusive && windows
}

/// The desktop's deadline a held picture's turn must make: `base_us` (the desktop's deadline, or
/// 2 ms where nothing composes), plus the draws after the hold (`draw_us`, with Frame Warp), since a
/// picture that comes late for its turn may have to draw after its hold. The sum stays under 4/5 of a
/// refresh, and never under `base_us`. The present itself goes out just after the refresh that opens
/// the turn (`phase::held_guard_us`).
pub fn held_margin_us(base_us: u64, draw_us: Option<u64>, refresh_us: u64) -> u64 {
    let Some(draw_us) = draw_us else { return base_us };
    (base_us + draw_us).min((refresh_us * 4 / 5).max(base_us))
}

/// How far ahead of the refresh the phase lock aims: `set_us` (a measurement run's) or that deadline,
/// for a stream whose presents skip. A waiting present and exclusive fullscreen, which nothing
/// composes, keep the refresh itself. A picture fractional V-Sync holds for its turn is aimed at the
/// hold's own target instead (`phase::held_present_lead_us`, the refresh it is presented in plus the
/// guard `phase::held_guard_us`): aimed at the deadline, a host that steers delivered the picture
/// after the refresh that opens the turn, so the hold never waited.
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

#[path = "no_wait.rs"]
pub(super) mod no_wait;
pub use no_wait::{present_after_egl_swap, present_without_waiting, NoWait};

/// Presents with sync interval 1 when `latched` (the scan wait would cost the next picture its
/// turn, `scan_sync::ScanWait::Latch`), else with tearing. Returns whether the present succeeded.
///
/// # Safety
/// `renderer` is the live D3D11 renderer of this thread.
#[cfg(windows)]
pub unsafe fn present_adaptive(renderer: *mut SDL_Renderer, latched: bool) -> bool {
    use windows::core::Interface;
    use windows::Win32::Graphics::Dxgi::{IDXGISwapChain, DXGI_PRESENT, DXGI_PRESENT_ALLOW_TEARING};
    // SAFETY: caller guarantees `renderer` is the live D3D11 renderer of this thread.
    unsafe {
        let chain = crate::render_device::live_renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER);
        let Some(chain) = IDXGISwapChain::from_raw_borrowed(&chain) else { return false };
        if !SDL_FlushRenderer(renderer) { return false; }
        let result = if latched { chain.Present(1, DXGI_PRESENT(0)) } else { chain.Present(0, DXGI_PRESENT_ALLOW_TEARING) };
        // A present unbinds the back buffer's view: this makes SDL's next draw bind it again.
        SDL_FlushRenderer(renderer);
        result.is_ok()
    }
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
    // SAFETY: caller guarantees `renderer` is the live D3D11 renderer of this thread.
    unsafe {
        let chain = crate::render_device::live_renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER);
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
}

/// Only Windows has this flip history.
///
/// # Safety
/// Nothing to uphold here; the signature is the Windows one's.
#[cfg(not(windows))]
pub unsafe fn flip_statistics(_renderer: *mut SDL_Renderer) -> (bool, bool, bool) { (false, false, false) }

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
    /// True once `upload_take` / `scan_out` already consumed this picture into `VideoTexture`
    /// (so retrying a refused `skipped_picture` presents directly without re-uploading a
    /// consumed `GpuPlanes` `Lease` or `Picture::Taken`).
    pub uploaded: bool,
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
    /// The PC's frame times without VRR (`Settings::host_frame_times`): while the mode is V-Sync,
    /// the clock places each picture on the display's refresh grid and the frame pacing setting
    /// is not used (the lowest latency's newest picture is taken).
    pub(super) host_frame_times: bool,
    /// The display's refresh grid and the desktop's deadline before a refresh (µs), for the PC's
    /// frame times; None until the render thread reads the display's timing (`set_grid`).
    grid: Option<Grid>,
    deadline_us: i64,
    /// The display's shortest refresh interval, for a clock made again.
    min_interval_us: i64,
    /// Whether the clock last learnt its delay on the refresh grid: a VRR stream that the display
    /// check turns to V-Sync with the PC's frame times, or back, starts its delay again.
    clock_on_grid: bool,
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
            host_frame_times: false,
            grid: None,
            deadline_us: 0,
            min_interval_us,
            clock_on_grid: false,
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

    /// The PC's frame times for V-Sync. A stream that starts at V-Sync learns its delay for a
    /// fixed refresh (the grid's share of frames, `PresentClock::schedule_on_grid`), not a VRR
    /// panel's link; a VRR stream keeps its clock when the display check turns it to V-Sync.
    pub fn with_host_frame_times(mut self, host_frame_times: bool) -> Self {
        self.host_frame_times = host_frame_times;
        if host_frame_times && self.mode == PacingMode::Vsync {
            self.clock = PresentClock::new(ClockSettings { min_interval_us: self.min_interval_us, ..ClockSettings::default() });
        }
        self.clock_on_grid = self.host_grid();
        self
    }

    /// Whether the PC's frame times place this stream's pictures on the refresh grid now.
    pub fn host_grid(&self) -> bool {
        self.mode == PacingMode::Vsync && self.host_frame_times
    }

    /// The display's refresh grid (None while its timing can't be read: pictures then go out at
    /// their time and V-Sync takes the refresh after) and the desktop's deadline before a refresh,
    /// for the pictures accepted next.
    pub fn set_grid(&mut self, grid: Option<Grid>, deadline_us: i64) {
        self.grid = grid;
        self.deadline_us = deadline_us.max(0);
    }

    /// The frame pacing that shapes this stream's presents: the PC's frame times take the
    /// lowest latency's newest picture.
    fn effective_pacing(&self) -> FramePacing {
        if self.host_frame_times { FramePacing::Latency } else { self.frame_pacing }
    }

    /// Whether this stream skips a picture that would wait behind the last one, on this display.
    /// Balanced that keeps every picture keeps it as the in-order pacings do.
    pub fn skips_instead_of_waiting(&self, refresh_hz: f32) -> bool {
        !self.keeps_every_picture() && skips_instead_of_waiting(self.mode, self.effective_pacing(), self.frame_us, refresh_hz)
    }

    /// Whether pictures are shown in turn, one a refresh, instead of the newest due one: the FPS
    /// limit, the smoothest pacing, and Balanced without Keep only the newest frame.
    pub fn keeps_every_picture(&self) -> bool {
        self.mode == PacingMode::Vsync && self.in_order()
    }

    /// Whether this stream's frame pacing shows every picture in turn while the mode is V-Sync.
    pub fn in_order(&self) -> bool {
        in_order_pacing(self.frame_pacing, self.drop_late_frames, self.host_frame_times)
    }

    /// Updates the display's shortest refresh interval (µs) after a display change.
    #[allow(dead_code)]
    pub fn set_min_interval_us(&mut self, min_interval_us: i64) {
        self.min_interval_us = min_interval_us;
    }

    /// Maximum age of a queued in-order picture before a newer due one replaces it (`take_due`).
    /// When the display refresh period is near or slower than one stream frame (e.g. 89.89 Hz
    /// Steam Deck OLED streaming 90 fps, or 59.94 Hz streaming 60 fps), allow one full refresh
    /// plus 2 ms of scheduling jitter so steady V-Sync pacing never falsely drops frames (#175).
    pub(super) fn stale_frame_us(&self) -> u64 {
        let refresh_us = self.min_interval_us.max(0) as u64;
        if refresh_us > 0 && refresh_us.saturating_mul(100) >= self.frame_us.saturating_mul(98) {
            refresh_us.max(self.frame_us).saturating_add(2_000)
        } else {
            self.frame_us
        }
    }

    /// One frame of the stream.
    pub fn frame_us(&self) -> u64 { self.frame_us }

    /// The stream's frame rate changed.
    pub fn set_fps(&mut self, fps: u32) { self.frame_us = frame_us(fps); }

    /// Whether the frame pacing setting shapes this stream's presents now.
    pub(super) fn paced(&self, pacing: FramePacing) -> bool {
        self.mode == PacingMode::Vsync && self.effective_pacing() == pacing
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
        if self.host_frame_times && self.host_grid() != self.clock_on_grid {
            // A grid's lateness under the desktop's deadline is no VRR delay, nor the other way
            // round: the clock forgets the frames it saw (its clock sync stays).
            self.clock.reset();
            self.clock_on_grid = self.host_grid();
        }
        let (present_us, hold_us, late) = match self.mode {
            PacingMode::HostTimes => {
                let s = self.clock.schedule_queued(decoded.rtp, decoded.ready_us, self.decode_queue);
                (s.present_us, s.hold_us, s.late)
            }
            _ if self.host_grid() => self.schedule_on_grid(&decoded),
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
            uploaded: false,
        });
    }

    /// When a picture of the PC's frame times goes out at V-Sync: its time on the PC's clock plus the
    /// learnt delay, moved onto the refresh grid. The clock sees each picture ready the desktop's
    /// deadline later than it is, so the refresh it picks is one the picture can still make; it is
    /// presented that much before the clock's release (`lead_us` ahead of the deadline), never
    /// before it is ready. Without a grid V-Sync takes the refresh after its time.
    fn schedule_on_grid(&mut self, decoded: &Decoded) -> (u64, i64, bool) {
        let deadline = self.deadline_us as u64;
        let ready = decoded.ready_us + deadline;
        let (release_us, hold_us, late) = match self.grid {
            Some(grid) => {
                let s = self.clock.schedule_on_grid_queued(decoded.rtp, ready, grid, self.decode_queue);
                (s.release_us, s.hold_us, s.late)
            }
            None => {
                let s = self.clock.schedule_queued(decoded.rtp, ready, self.decode_queue);
                (s.present_us, s.hold_us, s.late)
            }
        };
        (release_us.saturating_sub(deadline).max(decoded.ready_us), hold_us, late)
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
            let stale_us = self.stale_frame_us();
            while pending.get(1).is_some_and(|next| next.present_us <= now_us)
                && pending.front().is_some_and(|p| now_us.saturating_sub(p.decoded.ready_us) > stale_us)
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
