use rubylight_protocol::phase_lock::{Report, SlackWindow};
use sdl3_sys::everything::*;

/// Windows in a row a new place of the frames takes to reach the host of a stream with no refreshes
/// to spare (`SteadyReports`): 2 s.
pub const STEADY_WINDOWS: u32 = 4;
/// Windows of a new lock that go out as they are: it has to find its place first.
const STEADY_AFTER: u32 = 6;

/// Keeps a passing change of place from the host, on a stream with no refreshes to spare.
///
/// A source faster than the stream takes frames a picture off their place now and then, for a second
/// or two, and they come back by themselves. The host answers a report from there by taking a later
/// picture, and it learns a false drift that moves them later again: they land past the desktop's
/// deadline, where a picture a refresh has none to spare and each miss repeats one and loses one. So a
/// report from a new place repeats the last place until `windows` in a row said the same. Frames that
/// came early only wait a little longer, and a refresh that missed goes through at once.
#[derive(Debug, Default)]
pub struct SteadyReports {
    kept: Option<(i32, u32)>,
    seen: u32,
    away: u32,
    missed: bool,
}

impl SteadyReports {
    /// A refresh repeated a picture that was late for it.
    pub fn missed(&mut self) { self.missed = true; }

    /// What to send for this window's `report`.
    pub fn report(&mut self, report: Report, windows: u32) -> Report {
        let missed = std::mem::take(&mut self.missed);
        self.seen = self.seen.saturating_add(1);
        let period = i64::from(report.period_ns);
        let moved = |kept: i32| (i64::from(report.lead_ns) - i64::from(kept)).rem_euclid(period);
        let held = self.kept.filter(|&(lead, _)| {
            let moved = moved(lead);
            let away = moved.min(period - moved) > period / 5 || i64::from(report.spread_ns) > period / 4;
            away && !missed && self.seen > STEADY_AFTER && self.away + 1 < windows
        });
        if let Some((lead_ns, spread_ns)) = held {
            self.away += 1;
            return Report { lead_ns, spread_ns, ..report };
        }
        self.away = 0;
        self.kept = Some((report.lead_ns, report.spread_ns));
        report
    }
}

#[derive(Default)]
pub struct Phase {
    window: SlackWindow,
    last_report: u64,
    last_frame: u64,
    pub lead_us: Option<i32>,
    steady: SteadyReports,
}

impl Phase {
    /// A refresh repeated a picture that was late for it.
    pub fn missed(&mut self) { self.steady.missed(); }

    /// `steady_windows`: how long a stream with no refreshes to spare keeps a new place from the
    /// host (`SteadyReports`), 0 for any other.
    pub fn sample(&mut self, ready_us: u64, now_us: u64, timing: Option<(i64, i64)>, steady_windows: u32) -> Option<Report> {
        let Some((latch_us, period_ns)) = timing.filter(|(_, period)| (2_000_000..=41_700_000).contains(period)) else {
            self.reset(); return None;
        };
        if now_us.saturating_sub(ready_us) > 250_000 { self.reset(); return None; }
        // A place kept before the stream stopped skipping is stale once it skips again.
        if steady_windows == 0 { self.steady = SteadyReports::default(); }
        if self.last_frame == 0 || now_us.saturating_sub(self.last_frame) > 250_000 { self.reset(); self.last_report = now_us; }
        self.last_frame = now_us;
        self.window.push((latch_us - ready_us as i64) * 1000, period_ns);
        let elapsed_us = now_us.saturating_sub(self.last_report);
        if elapsed_us < 500_000 { return None; }
        self.last_report = now_us;
        // The window's length tells the report whether a frame came every refresh.
        let report = self.window.report_over(10, elapsed_us.min(u32::MAX as u64) as i64 * 1000)?;
        self.lead_us = Some(report.lead_ns / 1000);
        Some(if steady_windows > 0 { self.steady.report(report, steady_windows) } else { report })
    }

    pub fn reset(&mut self) { *self = Self::default(); }
}

/// The latch moved `aim_us` ahead of the refresh: slack is reported to that point.
pub fn aimed(timing: Option<(i64, i64)>, aim_us: i64) -> Option<(i64, i64)> {
    timing.map(|(latch_us, period_ns)| (latch_us - aim_us, period_ns))
}

/// How long pictures in a row have to come `EARLY_SPARE_US` and a refresh ahead of their turn's
/// deadline before the turns move a refresh earlier (`HeldAnchor::hold`). Longer than the host's
/// phase lock takes to move them later itself (a report or two), so the two never take turns.
pub const EARLY_FOR_US: u64 = 2_000_000;
/// See `EARLY_FOR_US`.
pub const EARLY_SPARE_US: u64 = 3_000;

/// Which refresh the last picture of a held stream was shown at (fractional V-Sync), and where the
/// next one's turn is. The swap chain's statistics name the last present shown, which may be a
/// redraw without a picture (the menu, the overlay, Frame Warp's shift): its turn would put the next
/// picture a hold behind it. So the picture's own present (`presented`) is watched until the
/// statistics show it, and until then its turn counts from the refresh its hold aimed it at.
#[derive(Debug, Default)]
pub struct HeldAnchor {
    picture: u32,
    shown_at: Option<u32>,
    /// The refresh the last picture's hold aimed it at, and the next picture's, until its present.
    aimed: Option<u32>,
    pending: Option<u32>,
    /// Since when pictures came a refresh early for their turn, in the clock's ticks.
    early_since: Option<u64>,
}

impl HeldAnchor {
    /// A picture went out as the swap chain's present `count`.
    pub fn presented(&mut self, count: u32) {
        (self.picture, self.shown_at, self.aimed) = (count, None, self.pending.take());
    }

    /// The refresh the last picture shows at, from the last present shown (`present_count` at
    /// `present_refresh`) and the latest refresh: the one it was shown at, else the one its hold aimed
    /// it at, while it waits or once a redraw passed it unseen, else the next one while it waits, and
    /// None when it is not known (no picture yet, or an unaimed one a redraw passed unseen).
    #[cfg_attr(not(windows), allow(dead_code))] // Only Windows counts refreshes.
    pub fn last_refresh(&mut self, present_count: u32, present_refresh: u32, sync_refresh: u32) -> Option<u32> {
        if self.picture == 0 || present_refresh == 0 { return None; }
        if present_count == self.picture { self.shown_at = Some(present_refresh); }
        if self.shown_at.is_some() { return self.shown_at; }
        let waiting = (self.picture.wrapping_sub(present_count) as i32) > 0;
        if !waiting { return self.aimed; }
        // Still unseen once its aimed refresh has come: it missed that, and shows at the next at
        // the earliest. Counted from the aimed one, the next picture came a refresh early (holds
        // of 3 and 1 in turn at 60 fps on 120 Hz).
        let next = sync_refresh.wrapping_add(1);
        Some(self.aimed.filter(|&aimed| (aimed.wrapping_sub(next) as i32) > 0).unwrap_or(next))
    }

    /// Fractional V-Sync: how long a picture waits before its present, which then goes out at the
    /// next refresh, so it is shown at its turn: `held` refreshes after the refresh the last picture
    /// shows at (`last`, `last_refresh`), `margin` ahead of it for the desktop's deadline. Nothing for
    /// a picture later than that, which the next ones then count from, or with no hold or no known last
    /// picture. A swap chain's own sync interval holds the picture too, but a picture early for its
    /// turn waits in the swap chain's queue, where each one after it waits as well and the phase
    /// report, which counts within the hold, never sees it: 60 fps on 120 Hz went from 9 to 36 ms that
    /// way. At most `held` refreshes and one more, behind a last picture still waiting.
    ///
    /// Pictures that come `early_spare` and a refresh ahead of their turn's deadline for `early_for`
    /// in a row move the turns a refresh earlier, once: a source whose clock runs ahead of the display
    /// brings them earlier and earlier, each waiting longer, up to the whole hold, until a host's phase
    /// lock moves them later, and a host without one never does (0 and 1 show the move only).
    ///
    /// Ticks of the clock of `now` and `sync_ticks`, the latest refresh `sync_refresh`'s time, and of
    /// `period`, `margin`, `early_spare` and `early_for`. The refresh it aims the picture at goes to
    /// the next `presented`.
    #[allow(clippy::too_many_arguments)]
    pub fn hold(&mut self, now: u64, sync_ticks: u64, sync_refresh: u32, last: Option<u32>, held: u32, period: u64, margin: u64,
        early_spare: u64, early_for: u64) -> u64 {
        let Some(last) = last.filter(|_| held > 1 && period > 0) else {
            (self.pending, self.early_since) = (None, None);
            return 0;
        };
        let (period, margin) = (period.min(i64::MAX as u64) as i64, margin.min(i64::MAX as u64) as i64);
        let mut ahead = i64::from(last.wrapping_add(held).wrapping_sub(sync_refresh) as i32);
        let wait = |ahead: i64| sync_ticks as i64 + ahead * period - margin - now as i64;
        // A refresh earlier is still after the last picture's: `held` is 2 or more.
        if wait(ahead - 1) >= early_spare.min(i64::MAX as u64) as i64 {
            let since = *self.early_since.get_or_insert(now);
            if now.saturating_sub(since) >= early_for {
                ahead -= 1;
                self.early_since = None;
            }
        } else {
            self.early_since = None;
        }
        // The refresh it shows at: its turn, or the first whose deadline it still makes.
        let first = (now as i64 + margin - sync_ticks as i64 + period - 1).div_euclid(period);
        self.pending = Some(sync_refresh.wrapping_add(ahead.max(first) as u32));
        wait(ahead).clamp(0, i64::from(held + 1) * period) as u64
    }
}

/// The refresh a picture held for `held` refreshes is due at, and that many refreshes as the
/// period (fractional V-Sync): its turns count from the refresh the last picture was shown at
/// (`last`, `HeldAnchor::last_refresh`), so the latch is the latest refresh of that grid,
/// `sync_refresh` being the latest refresh at `sync_ticks`. The refresh itself, and its period, for
/// `held` 1 or no known refresh of the last picture. Ticks and periods in the clock's own units.
#[cfg_attr(not(windows), allow(dead_code))] // Only Windows counts refreshes.
pub fn held_latch(sync_ticks: u64, sync_refresh: u32, last: Option<u32>, held: u32, period: u64) -> (u64, u64) {
    let Some(last) = last.filter(|_| held > 1) else { return (sync_ticks, period) };
    // Signed: the last picture may still wait for a refresh after the latest one (`last_refresh`),
    // and a remainder of the unsigned difference is right only where `held` divides 2^32.
    let since = i64::from(sync_refresh.wrapping_sub(last) as i32).rem_euclid(i64::from(held)) as u64;
    (sync_ticks.saturating_sub(since * period), period * u64::from(held))
}

/// Fractional V-Sync where the display tells when a picture reached it as a time rather than a
/// refresh count: Wayland's presentation feedback, or a KMS commit, which returns at the refresh
/// that shows it. The turns are `HeldAnchor::hold`'s, counted in refreshes of the display's period
/// from the last picture's time (`shown`), or from the time its hold aimed it at until that comes.
/// Times on the session clock, µs.
#[derive(Debug, Default)]
pub struct HeldClock {
    picture: Option<u32>,
    /// The picture before `picture`, whose feedback may still come after `picture` went out.
    before: Option<u32>,
    shown_us: Option<u64>,
    aimed_us: Option<u64>,
    pending_us: Option<u64>,
    /// The last hold's refreshes and refresh period, µs.
    held: u32,
    period_us: u64,
    turns: HeldAnchor,
}

impl HeldClock {
    /// The picture of stream frame `frame` went out.
    pub fn presented(&mut self, frame: u32) {
        self.before = self.picture;
        (self.picture, self.shown_us, self.aimed_us) = (Some(frame), None, self.pending_us.take());
    }

    /// Frame `frame` reached the display at `at_us`. The last picture's time is where the next turn
    /// counts from. The feedback of the picture before it may come only after the last one went out
    /// (the compositor answers at the refresh, the poll runs after a present): with nothing else
    /// known of the last picture, it shows a hold after that one.
    pub fn shown(&mut self, frame: u32, at_us: u64) {
        if self.picture == Some(frame) {
            self.shown_us = Some(at_us);
        } else if self.before == Some(frame) && self.shown_us.is_none() && self.aimed_us.is_none() && self.held > 1 {
            self.aimed_us = Some(at_us + u64::from(self.held) * self.period_us);
        }
    }

    fn last_us(&self) -> Option<u64> { self.shown_us.or(self.aimed_us) }

    /// `HeldAnchor::hold` for the next picture, `period_us` a refresh: how long it waits before its
    /// present.
    pub fn hold(&mut self, now_us: u64, held: u32, period_us: u64, margin_us: u64) -> u64 {
        (self.held, self.period_us) = (held, period_us);
        let Some(last) = self.last_us().filter(|_| held > 1 && period_us > 0) else {
            self.pending_us = None;
            return self.turns.hold(now_us, 0, 0, None, held, period_us, margin_us, EARLY_SPARE_US, EARLY_FOR_US);
        };
        let wait = self.turns.hold(now_us, last, 0, Some(0), held, period_us, margin_us, EARLY_SPARE_US, EARLY_FOR_US);
        self.pending_us = self.turns.pending.take().map(|refresh| last + u64::from(refresh) * period_us);
        wait
    }

    /// The phase report's latch and period for a held stream (`timing`'s): the latest refresh of the
    /// grid the turns count on, at or before `now_us`, and `held` refreshes. None without a hold or a
    /// last picture, or with one long gone.
    pub fn timing(&self, now_us: u64, held: u32, period_us: u64) -> Option<(i64, i64)> {
        let last = self.last_us().filter(|_| held > 1 && period_us > 0)?;
        let grid = u64::from(held) * period_us;
        let latch = if now_us >= last { last + (now_us - last) / grid * grid } else { last.checked_sub((last - now_us).div_ceil(grid) * grid)? };
        (now_us - latch <= 100_000 + grid).then_some((latch as i64, (grid * 1000) as i64))
    }
}

/// Whether the display tells when it showed a picture, which fractional V-Sync's turns count from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeldTimes {
    /// It does: pictures are held for their turns.
    Told,
    /// Not known yet (Wayland's feedback before the first answer was due): nothing held, nothing said.
    Unknown,
    /// It does not (X11, a compositor without timed feedback, swap chain statistics failing): each
    /// picture shows at the next refresh, and the log says why once.
    Untold,
}

/// How long a stream's first feedback request may wait for a timed answer before the display
/// counts as giving none (`glass_times`), µs.
pub const FEEDBACK_DUE_US: u64 = 1_000_000;

/// `HeldTimes` of Wayland's presentation feedback: `offered` None before the first request,
/// Some(false) where it is not offered or carries no times, else timed answers since
/// `last_timed_us`, the first request at `first_request_us`. Times on the session clock, µs.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // Only Linux has the feedback.
pub fn glass_times(offered: Option<bool>, first_request_us: Option<u64>, last_timed_us: Option<u64>, now_us: u64) -> HeldTimes {
    match offered {
        None => HeldTimes::Unknown,
        Some(false) => HeldTimes::Untold,
        Some(true) if last_timed_us.is_some_and(|at| now_us.saturating_sub(at) < FEEDBACK_DUE_US) => HeldTimes::Told,
        // Answers that stopped are as overdue as a first one that never came.
        Some(true) if now_us.saturating_sub(last_timed_us.or(first_request_us).unwrap_or(now_us)) < FEEDBACK_DUE_US => HeldTimes::Unknown,
        Some(true) => HeldTimes::Untold,
    }
}

/// How long a swap chain whose statistics failed holds nothing before it is tried again
/// (`StatsWatch`), µs.
pub const STATS_RETRY_US: u64 = 2_000_000;

/// Whether the swap chain's statistics, which fractional V-Sync's turns count from, can be read. A
/// single failure passes (the first read after a mode change may fail); two in a row hold nothing
/// for `STATS_RETRY_US`, then the next held picture tries again.
#[derive(Debug, Default)]
pub struct StatsWatch {
    fails: u32,
    since_us: Option<u64>,
}

impl StatsWatch {
    /// A held picture read the statistics (`ok`) or could not.
    pub fn read(&mut self, ok: bool, now_us: u64) {
        if ok { *self = Self::default(); return; }
        self.fails = self.fails.saturating_add(1);
        // A failed try after the wait starts a new one.
        if self.since_us.is_none_or(|since| now_us.saturating_sub(since) >= STATS_RETRY_US) { self.since_us = Some(now_us); }
    }

    #[cfg_attr(not(windows), allow(dead_code))] // Only Windows reads the statistics.
    pub fn times(&self, now_us: u64) -> HeldTimes {
        match self.since_us {
            Some(since) if self.fails >= 2 && now_us.saturating_sub(since) < STATS_RETRY_US => HeldTimes::Untold,
            _ => HeldTimes::Told,
        }
    }
}

/// `HeldAnchor::hold` for the renderer's swap chain, in microseconds; None when its statistics
/// cannot be read (`StatsWatch`).
#[cfg(windows)]
pub fn held_wait_us(renderer: *mut SDL_Renderer, refresh_hz: f32, held: u32, margin_us: u64, anchor: &mut HeldAnchor) -> Option<u64> {
    use windows::core::Interface;
    use windows::Win32::Graphics::Dxgi::{IDXGISwapChain, DXGI_FRAME_STATISTICS};
    if held <= 1 || refresh_hz <= 0.0 { return Some(0); }
    // SAFETY: the chain pointer comes from the live renderer and is only borrowed for these calls.
    unsafe {
        let chain = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER);
        let chain = IDXGISwapChain::from_raw_borrowed(&chain)?;
        let mut stats = DXGI_FRAME_STATISTICS::default();
        if chain.GetFrameStatistics(&mut stats).is_err() || stats.SyncQPCTime <= 0 { return None; }
        let last = anchor.last_refresh(stats.PresentCount, stats.PresentRefreshCount, stats.SyncRefreshCount);
        let freq = SDL_GetPerformanceFrequency() as f64;
        let ticks = |us: f64| (us * freq / 1e6) as u64;
        let wait = anchor.hold(SDL_GetPerformanceCounter(), stats.SyncQPCTime as u64, stats.SyncRefreshCount, last,
            held, ticks(1e6 / f64::from(refresh_hz)), ticks(margin_us as f64), ticks(EARLY_SPARE_US as f64), ticks(EARLY_FOR_US as f64));
        Some((wait as f64 * 1e6 / freq) as u64)
    }
}

/// The swap chain's count of the last present, for `HeldAnchor::presented`; 0 without one.
#[cfg(windows)]
pub fn last_present_count(renderer: *mut SDL_Renderer) -> u32 {
    use windows::core::Interface;
    use windows::Win32::Graphics::Dxgi::IDXGISwapChain;
    // SAFETY: the chain pointer comes from the live renderer and is only borrowed for this call.
    unsafe {
        let chain = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER);
        IDXGISwapChain::from_raw_borrowed(&chain).and_then(|chain| chain.GetLastPresentCount().ok()).unwrap_or(0)
    }
}

/// Only Windows has the count.
#[cfg(not(windows))]
pub fn last_present_count(_: *mut SDL_Renderer) -> u32 { 0 }

/// Only Windows holds pictures for their turn.
#[cfg(not(windows))]
pub fn held_wait_us(_: *mut SDL_Renderer, _: f32, _: u32, _: u64, _: &mut HeldAnchor) -> Option<u64> { Some(0) }

#[cfg(windows)]
pub fn timing(renderer: *mut SDL_Renderer, refresh_hz: f32, held: u32, anchor: &mut HeldAnchor) -> Option<(i64, i64)> {
    use windows::core::Interface;
    use windows::Win32::Graphics::Dxgi::{IDXGISwapChain, DXGI_FRAME_STATISTICS};
    use windows::Win32::Graphics::Dwm::{DwmGetCompositionTimingInfo, DWM_TIMING_INFO};
    unsafe {
        let period_ns = (1_000_000_000.0 / f64::from(refresh_hz)) as i64;
        let chain = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER);
        let stats = IDXGISwapChain::from_raw_borrowed(&chain).and_then(|chain| {
            let mut stats = DXGI_FRAME_STATISTICS::default();
            chain.GetFrameStatistics(&mut stats).ok().map(|_| stats)
        });
        let held_stats = stats.filter(|s| s.SyncQPCTime > 0).map(|s| {
            let ticks_per_refresh = (period_ns as f64 * SDL_GetPerformanceFrequency() as f64 / 1e9) as u64;
            let last = if held > 1 { anchor.last_refresh(s.PresentCount, s.PresentRefreshCount, s.SyncRefreshCount) } else { None };
            let (latch, _) = held_latch(s.SyncQPCTime as u64, s.SyncRefreshCount, last, held, ticks_per_refresh);
            let (_, period) = held_latch(0, s.SyncRefreshCount, last, held, period_ns as u64);
            (latch, period as i64)
        });
        // DWM's clock knows no refresh of the last picture, so it keeps the refresh itself.
        let (latch, period_ns) = if let Some(held) = held_stats { held } else {
            let window = SDL_GetRenderWindow(renderer);
            // DWM exposes the primary compositor's clock, not the phase of another monitor.
            if SDL_GetDisplayForWindow(window) != SDL_GetPrimaryDisplay() { return None; }
            let mut info = DWM_TIMING_INFO { cbSize: std::mem::size_of::<DWM_TIMING_INFO>() as u32, ..Default::default() };
            DwmGetCompositionTimingInfo(windows::Win32::Foundation::HWND::default(), &mut info).ok()?;
            let measured = (info.qpcRefreshPeriod as f64 * 1_000_000_000.0 / SDL_GetPerformanceFrequency() as f64) as i64;
            if (measured - period_ns).abs() > period_ns / 100 { return None; }
            (info.qpcVBlank, measured)
        };
        let counter = SDL_GetPerformanceCounter();
        let age_us = counter.checked_sub(latch)? as f64 * 1_000_000.0 / SDL_GetPerformanceFrequency() as f64;
        if age_us > 100_000.0 { return None; }
        Some((nightjar_core::session::now_us() as i64 - age_us as i64, period_ns))
    }
}

/// The swap chain's counters and last vblank time right after a present call, for the timing CSV
/// (`timing_csv::Flip`). Empty when the renderer has no DXGI swap chain.
#[cfg(windows)]
pub fn present_stats(renderer: *mut SDL_Renderer) -> crate::timing_csv::Flip {
    use windows::core::Interface;
    use windows::Win32::Graphics::Dxgi::{IDXGISwapChain, DXGI_FRAME_STATISTICS};
    // SAFETY: the chain pointer comes from the live renderer and is only borrowed for these calls.
    unsafe {
        let chain = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER);
        let Some(chain) = IDXGISwapChain::from_raw_borrowed(&chain) else { return Default::default() };
        let mut flip = crate::timing_csv::Flip { present_count: chain.GetLastPresentCount().unwrap_or(0), ..Default::default() };
        let mut stats = DXGI_FRAME_STATISTICS::default();
        if chain.GetFrameStatistics(&mut stats).is_ok() {
            flip.stat_present_count = stats.PresentCount;
            flip.stat_present_refresh = stats.PresentRefreshCount;
            flip.stat_sync_refresh = stats.SyncRefreshCount;
            let age = SDL_GetPerformanceCounter().checked_sub(stats.SyncQPCTime as u64).filter(|_| stats.SyncQPCTime > 0);
            if let Some(age) = age {
                let age_us = (age as f64 * 1_000_000.0 / SDL_GetPerformanceFrequency() as f64) as u64;
                flip.stat_sync_us = nightjar_core::session::now_us().saturating_sub(age_us);
            }
        }
        flip
    }
}

#[cfg(not(windows))]
pub fn present_stats(_: *mut SDL_Renderer) -> crate::timing_csv::Flip { Default::default() }

#[cfg(not(windows))]
pub fn timing(renderer: *mut SDL_Renderer, _: f32, _held: u32, _anchor: &mut HeldAnchor) -> Option<(i64, i64)> {
    type Sync = unsafe extern "C" fn(*mut std::ffi::c_void, libc::c_ulong, *mut i64, *mut i64, *mut i64) -> i32;
    type Rate = unsafe extern "C" fn(*mut std::ffi::c_void, libc::c_ulong, *mut i32, *mut i32) -> i32;
    // Looked up once, when a context has them; a missing one is looked up again next time.
    static PROCS: std::sync::OnceLock<(Sync, Rate)> = std::sync::OnceLock::new();
    unsafe {
        if SDL_GL_GetCurrentContext().is_null() { return None; }
        let props = SDL_GetWindowProperties(SDL_GetRenderWindow(renderer));
        let display = SDL_GetPointerProperty(props, SDL_PROP_WINDOW_X11_DISPLAY_POINTER, std::ptr::null_mut());
        let drawable = SDL_GetNumberProperty(props, SDL_PROP_WINDOW_X11_WINDOW_NUMBER, 0) as libc::c_ulong;
        if display.is_null() || drawable == 0 { return None; }
        let (sync, rate) = match PROCS.get() {
            Some(&procs) => procs,
            None => {
                let sync: Sync = std::mem::transmute(SDL_GL_GetProcAddress(c"glXGetSyncValuesOML".as_ptr())?);
                let rate: Rate = std::mem::transmute(SDL_GL_GetProcAddress(c"glXGetMscRateOML".as_ptr())?);
                *PROCS.get_or_init(|| (sync, rate))
            }
        };
        let (mut ust, mut msc, mut sbc, mut numerator, mut denominator) = (0, 0, 0, 0, 0);
        if sync(display, drawable, &mut ust, &mut msc, &mut sbc) == 0
            || rate(display, drawable, &mut numerator, &mut denominator) == 0 || numerator <= 0 || denominator <= 0 { return None; }
        // Mesa's UST uses CLOCK_MONOTONIC. Reject other epochs instead of inventing a phase.
        let mut now: libc::timespec = std::mem::zeroed();
        if libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) != 0 { return None; }
        let age_us = now.tv_sec * 1_000_000 + now.tv_nsec / 1000 - ust;
        if !(0..100_000).contains(&age_us) { return None; }
        Some((nightjar_core::session::now_us() as i64 - age_us, 1_000_000_000 * i64::from(denominator) / i64::from(numerator)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measured_latch_reports_are_bounded_and_reset_after_missing_timing() {
        let mut phase = Phase::default();
        let mut report = None;
        for n in 1..=61 {
            let now = n * 10_000;
            report = phase.sample(now - 2000, now, Some((now as i64, 10_000_000)), 0).or(report);
        }
        let report = report.unwrap();
        assert_eq!(report.lead_ns, 2_000_000);
        assert!(Report::decode(&report.encode()).is_some());
        assert_eq!(phase.sample(700_000, 700_000, None, 0), None);
        assert_eq!(phase.lead_us, None);
        assert_eq!(phase.sample(800_000, 800_000, Some((800_000, 10_000_000)), 0), None);
        assert_eq!(phase.sample(800_000, 1_100_000, Some((1_100_000, 10_000_000)), 0), None);
    }

    fn at(lead_ns: i32) -> Report {
        Report { frames: 60, period_ns: 8_333_333, lead_ns, spread_ns: 1_300_000 }
    }

    fn settled() -> SteadyReports {
        let mut steady = SteadyReports::default();
        for _ in 0..=STEADY_AFTER { steady.report(at(2_000_000), STEADY_WINDOWS); }
        steady
    }

    /// Reports of real runs: frames a picture early for two windows, then back.
    #[test]
    fn a_passing_change_of_place_never_reaches_the_host() {
        let mut steady = settled();
        for lead in [-2_640_000, -1_840_000] {
            assert_eq!(steady.report(at(lead), STEADY_WINDOWS).lead_ns, 2_000_000);
        }
        assert_eq!(steady.report(at(2_490_000), STEADY_WINDOWS).lead_ns, 2_490_000, "near the last place: as it is");
    }

    #[test]
    fn a_window_across_a_change_of_place_keeps_the_last_spread_too() {
        let mut steady = settled();
        let across = steady.report(Report { spread_ns: 4_900_000, ..at(2_100_000) }, STEADY_WINDOWS);
        assert_eq!((across.lead_ns, across.spread_ns), (2_000_000, 1_300_000));
    }

    #[test]
    fn a_new_place_that_lasts_reaches_the_host() {
        let mut steady = settled();
        for _ in 1..STEADY_WINDOWS {
            assert_eq!(steady.report(at(-2_500_000), STEADY_WINDOWS).lead_ns, 2_000_000);
        }
        assert_eq!(steady.report(at(-2_500_000), STEADY_WINDOWS).lead_ns, -2_500_000);
        assert_eq!(steady.report(at(-2_400_000), STEADY_WINDOWS).lead_ns, -2_400_000, "the new place is the place now");
    }

    #[test]
    fn a_missed_refresh_goes_through_at_once() {
        let mut steady = settled();
        steady.missed();
        assert_eq!(steady.report(at(-1_000_000), STEADY_WINDOWS).lead_ns, -1_000_000);
        assert_eq!(steady.report(at(2_000_000), STEADY_WINDOWS).lead_ns, -1_000_000, "the miss counts once");
    }

    #[test]
    fn a_new_lock_finds_its_place_unheld() {
        let mut steady = SteadyReports::default();
        for lead in [-1_530_000, 3_020_000, -210_000, -3_850_000, 2_130_000, -2_000_000] {
            assert_eq!(steady.report(at(lead), STEADY_WINDOWS).lead_ns, lead);
        }
    }

    #[test]
    fn across_the_wrap_of_a_refresh_is_no_change_of_place() {
        let mut steady = settled();
        assert_eq!(steady.report(at(3_600_000), STEADY_WINDOWS).lead_ns, 3_600_000);
        assert_eq!(steady.report(at(4_100_000), STEADY_WINDOWS).lead_ns, 4_100_000);
        assert_eq!(steady.report(at(-4_166_000), STEADY_WINDOWS).lead_ns, -4_166_000);
    }

    #[test]
    fn a_stream_with_refreshes_to_spare_sends_every_window_as_it_is() {
        let mut phase = Phase::default();
        let mut leads = Vec::new();
        for n in 1..=400u64 {
            let now = n * 10_000;
            // Frames move a quarter of a refresh every second.
            let ready = now - if n < 200 { 2000 } else { 4500 };
            if let Some(report) = phase.sample(ready, now, Some((now as i64, 10_000_000)), 0) { leads.push(report.lead_ns); }
        }
        assert_eq!(leads.last(), Some(&4_500_000));
    }

    #[test]
    fn a_stream_that_skips_again_finds_its_place_unheld() {
        let mut phase = Phase::default();
        let mut leads = Vec::new();
        for n in 1..=700u64 {
            let now = n * 10_000;
            // Settled while skipping, then 2 s with refreshes to spare, then skipping a quarter of a
            // refresh later.
            let (ready, windows) = match n {
                ..=400 => (now - 2000, STEADY_WINDOWS),
                401..=600 => (now - 2000, 0),
                _ => (now - 4500, STEADY_WINDOWS),
            };
            if let Some(report) = phase.sample(ready, now, Some((now as i64, 10_000_000)), windows) { leads.push(report.lead_ns); }
        }
        assert_eq!(leads.last(), Some(&4_500_000));
    }

    #[test]
    fn slack_is_reported_to_the_point_the_lock_aims_at() {
        let mut phase = Phase::default();
        let mut report = None;
        for n in 1..=61 {
            let now = n * 10_000;
            report = phase.sample(now - 7000, now, aimed(Some((now as i64, 10_000_000)), 5_000), 0).or(report);
        }
        assert_eq!(report.unwrap().lead_ns, 2_000_000, "ready 7 ms before the refresh is 2 ms before a deadline 5 ms ahead of it");
    }

    #[test]
    fn a_held_picture_latches_on_the_refresh_its_hold_ends_on() {
        // Shown at refresh 100, held for 2: the grid is 100, 102, 104...
        assert_eq!(held_latch(10_000, 105, Some(100), 2, 1000), (9_000, 2000), "latest refresh 105: the grid's 104");
        assert_eq!(held_latch(10_000, 104, Some(100), 2, 1000), (10_000, 2000));
        assert_eq!(held_latch(10_000, 107, Some(100), 3, 1000), (9_000, 3000), "held 3: 106");
        assert_eq!(held_latch(10_000, 100, Some(100), 4, 1000), (10_000, 4000), "the refresh it was shown at");
        assert_eq!(held_latch(10_000, 1, Some(u32::MAX), 2, 1000), (10_000, 2000), "across the counter's wrap");
        assert_eq!(held_latch(10_000, 2, Some(u32::MAX), 3, 1000), (10_000, 3000), "held 3 across the counter's wrap");
    }

    #[test]
    fn a_last_picture_still_waiting_latches_on_its_grids_refresh_before() {
        // The last picture waits for 101, the latest refresh is 100: held 3, its grid is 95, 98, 101.
        assert_eq!(held_latch(10_000, 100, Some(101), 3, 1000), (8_000, 3000), "98, not 100");
        assert_eq!(held_latch(10_000, 100, Some(101), 2, 1000), (9_000, 2000), "held 2: 99");
        assert_eq!(held_latch(10_000, 100, Some(102), 4, 1000), (8_000, 4000), "held 4: 98");
    }

    #[test]
    fn without_a_hold_or_a_known_refresh_the_latch_is_the_refresh() {
        assert_eq!(held_latch(10_000, 105, Some(100), 1, 1000), (10_000, 1000));
        assert_eq!(held_latch(10_000, 105, None, 2, 1000), (10_000, 1000), "no picture shown yet");
    }

    #[test]
    fn a_held_stream_reports_its_slack_over_the_held_period() {
        let mut phase = Phase::default();
        let mut report = None;
        for n in 1..=61 {
            let now = n * 16_667;
            // Latched every 16.7 ms (60 fps held 2 refreshes on 120 Hz), ready 3 ms before.
            report = phase.sample(now - 3000, now, Some((now as i64, 16_666_666)), 0).or(report);
        }
        let report = report.unwrap();
        assert_eq!((report.period_ns, report.lead_ns), (16_666_666, 3_000_000));
        assert!(Report::decode(&report.encode()).is_some(), "the host takes a period of a stream frame");
    }

    /// `HeldAnchor::hold` of a fresh anchor, which never moves the turns earlier.
    fn held_wait(now: u64, sync_ticks: u64, sync_refresh: u32, last: Option<u32>, held: u32, period: u64, margin: u64) -> u64 {
        HeldAnchor::default().hold(now, sync_ticks, sync_refresh, last, held, period, margin, u64::MAX, 0)
    }

    #[test]
    fn a_held_picture_waits_for_its_turn_and_a_late_one_goes_at_once() {
        // Refresh 100 at tick 10_000, 1000 ticks a refresh, margin 300; last picture shown at 100.
        assert_eq!(held_wait(10_100, 10_000, 100, Some(100), 2, 1000, 300), 1_600, "turn 102 at 12_000");
        assert_eq!(held_wait(10_100, 10_000, 101, Some(100), 2, 1000, 300), 600, "latest 101 at 10_000: turn at 11_000");
        assert_eq!(held_wait(10_100, 10_000, 100, Some(100), 3, 1000, 300), 2_600, "held 3");
        assert_eq!(held_wait(10_800, 10_000, 102, Some(100), 2, 1000, 300), 0, "past its turn's deadline: at once");
        assert_eq!(held_wait(10_100, 10_000, 100, Some(101), 2, 1000, 300), 2_600, "the last one still waits for 101");
        assert_eq!(held_wait(10_100, 10_000, 0, Some(u32::MAX), 2, 1000, 300), 600, "across the counter's wrap: turn 1");
    }

    #[test]
    fn without_a_hold_or_a_known_last_picture_nothing_waits() {
        assert_eq!(held_wait(10_100, 10_000, 100, Some(100), 1, 1000, 300), 0);
        assert_eq!(held_wait(10_100, 10_000, 100, None, 2, 1000, 300), 0);
        assert_eq!(held_wait(0, 50_000, 100, Some(100), 2, 1000, 0), 3_000, "never more than the hold and one refresh");
    }

    #[test]
    fn the_anchor_is_the_last_pictures_refresh_not_a_redraws() {
        let mut anchor = HeldAnchor::default();
        assert_eq!(anchor.last_refresh(5, 100, 100), None, "no picture yet");
        anchor.presented(7);
        assert_eq!(anchor.last_refresh(6, 100, 101), Some(102), "still waiting: the next refresh");
        assert_eq!(anchor.last_refresh(7, 102, 102), Some(102), "shown");
        // A redraw (present 8) shown at 103 leaves the turns counting from 102.
        assert_eq!(anchor.last_refresh(8, 103, 103), Some(102));
        anchor.presented(10);
        assert_eq!(anchor.last_refresh(9, 104, 104), Some(105), "the next picture waits");
        assert_eq!(anchor.last_refresh(11, 106, 106), None, "passed by a redraw unseen: not known");
        anchor.presented(1);
        assert_eq!(anchor.last_refresh(u32::MAX, 106, 106), Some(107), "waiting across the counter's wrap");
    }

    #[test]
    fn a_held_picture_counts_from_the_refresh_its_hold_aimed_it_at_until_it_is_seen() {
        let mut anchor = HeldAnchor::default();
        // Last picture shown at 100 (present 7); the next one's turn is 102.
        anchor.presented(7);
        assert_eq!(anchor.last_refresh(7, 100, 100), Some(100));
        assert_eq!(anchor.hold(10_100, 10_000, 100, Some(100), 2, 1000, 300, u64::MAX, 0), 1_600);
        anchor.presented(8);
        // The statistics still name present 7 at the latest refresh 101: its turn, not 102.
        assert_eq!(anchor.last_refresh(7, 100, 101), Some(102), "aimed at 102");
        assert_eq!(anchor.last_refresh(7, 100, 100), Some(102), "even read a refresh behind");
        assert_eq!(anchor.last_refresh(7, 100, 102), Some(103), "102 came and it is still unseen: missed, 103 at the earliest");
        // A redraw (present 9) passed it unseen: still its aimed refresh, not unknown.
        assert_eq!(anchor.last_refresh(9, 103, 103), Some(102));
        // Seen a refresh late: the refresh it was shown at.
        anchor.presented(10);
        assert_eq!(anchor.last_refresh(10, 105, 105), Some(105), "the statistics win");
    }

    #[test]
    fn a_late_picture_is_aimed_at_the_first_refresh_whose_deadline_it_makes() {
        let mut anchor = HeldAnchor::default();
        // Turn 102 at 12_000 passed (now 12_900, margin 300), and 103's deadline (12_700) too: 104.
        assert_eq!(anchor.hold(12_900, 12_000, 102, Some(100), 2, 1000, 300, u64::MAX, 0), 0);
        anchor.presented(5);
        assert_eq!(anchor.last_refresh(4, 101, 102), Some(104));
    }

    #[test]
    fn a_clocked_hold_counts_its_turns_from_the_last_pictures_time() {
        let mut clock = HeldClock::default();
        assert_eq!(clock.hold(10_000, 2, 8_333, 5_000), 0, "no picture yet");
        assert_eq!(clock.timing(10_000, 2, 8_333), None);
        // Shown at 100_000; the next one's turn 116_666, its deadline 111_666.
        clock.presented(1);
        clock.shown(1, 100_000);
        clock.shown(9, 105_000);
        assert_eq!(clock.hold(103_000, 2, 8_333, 5_000), 8_666, "a refresh early is not long enough");
        assert_eq!(clock.timing(103_000, 2, 8_333), Some((100_000, 16_666_000)));
        assert_eq!(clock.timing(130_000, 2, 8_333), Some((116_666, 16_666_000)));
        // Until its feedback comes, the next counts from the refresh the hold aimed this one at.
        clock.presented(2);
        assert_eq!(clock.hold(118_000, 2, 8_333, 5_000), 10_332, "from 116_666: turn 133_332");
        assert_eq!(clock.timing(118_000, 2, 8_333), Some((116_666, 16_666_000)));
        // The feedback says it was a refresh late: from there.
        clock.shown(2, 124_999);
        assert_eq!(clock.hold(126_000, 2, 8_333, 5_000), 10_665);
        // A late picture aims at the first refresh whose deadline it makes, and counts on from it.
        let mut clock = HeldClock::default();
        clock.presented(1);
        clock.shown(1, 100_000);
        assert_eq!(clock.hold(113_000, 2, 8_333, 5_000), 0);
        clock.presented(2);
        assert_eq!(clock.timing(130_000, 2, 8_333), Some((124_999, 16_666_000)), "aimed at 124_999");
    }

    /// Wayland in the order the renderer runs: the hold, the present, and the poll of the answers,
    /// which comes before the next picture's hold.
    #[test]
    fn a_clocked_hold_engages_in_the_renderers_order() {
        const P: u64 = 8_333;
        let mut clock = HeldClock::default();
        // Picture 1 at 100_000 (nothing known: at once), shown at the refresh 108_333.
        assert_eq!(clock.hold(100_000, 2, P, 5_000), 0);
        clock.presented(1);
        // Picture 2, 16.7 ms on: the poll before its hold brings 1's answer.
        clock.shown(1, 108_333);
        assert_eq!(clock.hold(116_000, 2, P, 5_000), 124_999 - 5_000 - 116_000, "turn 108_333 + 2 refreshes");
        assert_eq!(clock.timing(116_000, 2, P), Some((108_333, 16_666_000)), "the report has a grid");
        clock.presented(2);
        // Picture 3 before 2's answer came: from the refresh 2 was aimed at.
        assert_eq!(clock.hold(132_000, 2, P, 5_000), 141_665 - 5_000 - 132_000);
        clock.presented(3);
        assert!(clock.timing(140_000, 2, P).is_some());
    }

    /// Answers that come only after the next picture went out (the old order, or a compositor that
    /// answers late) still start the turns: the last picture shows a hold after the one answered.
    #[test]
    fn a_late_answer_for_the_picture_before_starts_the_turns() {
        const P: u64 = 8_333;
        let mut clock = HeldClock::default();
        assert_eq!(clock.hold(100_000, 2, P, 5_000), 0);
        clock.presented(1);
        assert_eq!(clock.hold(116_000, 2, P, 5_000), 0, "nothing known yet");
        clock.presented(2);
        clock.shown(1, 108_333);
        // 2 counts as shown at 108_333 + 2 refreshes = 124_999: 3's turn 141_665.
        assert_eq!(clock.hold(132_000, 2, P, 5_000), 141_665 - 5_000 - 132_000);
        assert_eq!(clock.timing(132_000, 2, P), Some((124_999, 16_666_000)));
        // An answer for an older picture, or one the last picture already has, changes nothing.
        clock.presented(3);
        clock.shown(1, 200_000);
        clock.shown(2, 124_999);
        assert_eq!(clock.timing(145_000, 2, P), Some((141_665, 16_666_000)));
    }

    #[test]
    fn the_feedback_is_unknown_until_an_answer_was_due() {
        assert_eq!(glass_times(None, None, None, 5_000_000), HeldTimes::Unknown, "no picture yet");
        assert_eq!(glass_times(Some(true), Some(1_000_000), None, 1_500_000), HeldTimes::Unknown, "the first answer not due yet");
        assert_eq!(glass_times(Some(true), Some(1_000_000), None, 2_000_000), HeldTimes::Untold, "due and absent");
        assert_eq!(glass_times(Some(true), Some(1_000_000), Some(1_020_000), 1_500_000), HeldTimes::Told);
        assert_eq!(glass_times(Some(true), Some(1_000_000), Some(1_020_000), 2_100_000), HeldTimes::Untold, "answers stopped");
        assert_eq!(glass_times(Some(false), Some(1_000_000), None, 1_000_000), HeldTimes::Untold, "not offered, or untimed");
    }

    #[test]
    fn statistics_that_keep_failing_hold_nothing_for_a_while() {
        let mut stats = StatsWatch::default();
        assert_eq!(stats.times(0), HeldTimes::Told);
        stats.read(false, 1_000);
        assert_eq!(stats.times(2_000), HeldTimes::Told, "a single failure passes");
        stats.read(true, 3_000);
        stats.read(false, 4_000);
        assert_eq!(stats.times(5_000), HeldTimes::Told, "failures not in a row");
        stats.read(false, 6_000);
        assert_eq!(stats.times(7_000), HeldTimes::Untold);
        assert_eq!(stats.times(4_000 + STATS_RETRY_US), HeldTimes::Told, "tried again");
        stats.read(false, 4_000 + STATS_RETRY_US);
        assert_eq!(stats.times(5_000 + STATS_RETRY_US), HeldTimes::Untold, "still failing: another wait");
        stats.read(true, 6_000 + STATS_RETRY_US);
        stats.read(false, 7_000 + STATS_RETRY_US);
        assert_eq!(stats.times(8_000 + STATS_RETRY_US), HeldTimes::Told);
    }

    /// The span of `source` from `from` to the next `fn ` at the same indent.
    fn function<'a>(source: &'a str, from: &str) -> &'a str {
        let start = source.find(from).unwrap_or_else(|| panic!("{from}"));
        let rest = &source[start + from.len()..];
        &rest[..rest.find("\n    pub fn ").or_else(|| rest.find("\n    fn ")).unwrap_or(rest.len())]
    }

    #[test]
    fn the_answers_are_polled_before_the_hold_and_the_present() {
        let present = function(include_str!("render.rs"), "pub fn present(");
        let poll = present.find("self.glass.poll(").expect("present polls the answers");
        assert!(poll < present.find("self.hold_for_turn(").unwrap(), "before the hold with Frame Warp");
        assert!(poll < present.find("self.kms_flip(").unwrap(), "before the present and its hold");
        assert!(present.find("self.held_clock.presented(").unwrap() > present.find("self.kms_flip(").unwrap());
    }

    #[test]
    fn with_frame_warp_the_hold_comes_before_its_shift() {
        let render = include_str!("render.rs");
        let present = function(render, "pub fn present(");
        assert!(present.find("self.hold_for_turn(").unwrap() < present.find("warp.draw(None)").unwrap());
        // Held once: the presents themselves hold only without Frame Warp.
        for flip in ["fn render_flip(", "fn kms_flip(&mut self, draw_overlay"] {
            let body = function(render, flip);
            for (at, _) in body.match_indices("self.hold_for_turn(") {
                assert!(body[..at].rsplit('\n').next().unwrap().contains("self.frame_warp.is_none()"), "{flip}");
            }
        }
    }

    #[test]
    fn a_display_change_starts_the_held_turns_over() {
        let changed = function(include_str!("mod.rs"), "pub fn display_changed(");
        assert!(changed.contains("(self.held_anchor, self.held_clock, self.held_stats) = Default::default();"));
    }

    #[test]
    fn pictures_a_refresh_early_for_long_enough_move_the_turns_a_refresh_earlier() {
        let mut anchor = HeldAnchor::default();
        // 40 fps held 3 refreshes of 1000 ticks, margin 300: last picture at 100 (tick 10_000), turn
        // 103 at 13_000. A picture ready at 10_500 has 102's deadline (11_700) 1200 ahead.
        let hold = |anchor: &mut HeldAnchor, now| anchor.hold(now, 10_000, 100, Some(100), 3, 1000, 300, 1_000, 5_000);
        assert_eq!(hold(&mut anchor, 10_500), 2_200, "the turn, 103");
        assert_eq!(hold(&mut anchor, 14_000), 0, "a late one: the run starts over");
        assert_eq!(hold(&mut anchor, 10_500), 2_200, "early since 10_500");
        assert_eq!(hold(&mut anchor, 15_400), 0, "4900 later and late");
        assert_eq!(hold(&mut anchor, 10_500), 2_200);
        // Early all the way from 10_500 to 15_500: one move, to 102.
        let mut anchor = HeldAnchor::default();
        assert_eq!(anchor.hold(10_500, 10_000, 100, Some(100), 3, 1000, 300, 1_000, 5_000), 2_200);
        assert_eq!(anchor.hold(15_500, 15_000, 105, Some(105), 3, 1000, 300, 1_000, 5_000), 1_200, "moved: 107 for 108");
        anchor.presented(9);
        assert_eq!(anchor.last_refresh(8, 105, 105), Some(107));
        assert_eq!(anchor.hold(15_500, 15_000, 105, Some(105), 3, 1000, 300, 1_000, 5_000), 2_200, "the run starts over");
        // Too little to spare (900 of 1000) never moves them.
        let mut anchor = HeldAnchor::default();
        for now in [10_600, 13_000, 20_000] {
            assert_eq!(anchor.hold(now, now - 800, 100, Some(100), 3, 1000, 300, 1_000, 5_000), 1_900);
        }
    }
}
