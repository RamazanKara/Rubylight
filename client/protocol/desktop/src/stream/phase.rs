use rubylight_protocol::phase_lock::{Report, SlackWindow};
use sdl3_sys::everything::*;

#[derive(Default)]
pub struct Phase {
    window: SlackWindow,
    last_report: u64,
    last_frame: u64,
    pub lead_us: Option<i32>,
}

impl Phase {
    pub fn sample(&mut self, ready_us: u64, now_us: u64, timing: Option<(i64, i64)>) -> Option<Report> {
        let Some((latch_us, period_ns)) = timing.filter(|(_, period)| (2_000_000..=41_700_000).contains(period)) else {
            self.reset(); return None;
        };
        if now_us.saturating_sub(ready_us) > 250_000 { self.reset(); return None; }
        if self.last_frame == 0 || now_us.saturating_sub(self.last_frame) > 250_000 { self.reset(); self.last_report = now_us; }
        self.last_frame = now_us;
        self.window.push((latch_us - ready_us as i64) * 1000, period_ns);
        let elapsed_us = now_us.saturating_sub(self.last_report);
        if elapsed_us < 500_000 { return None; }
        self.last_report = now_us;
        // The window's length tells the report whether a frame came every refresh.
        let report = self.window.report_over(10, elapsed_us.min(u32::MAX as u64) as i64 * 1000)?;
        self.lead_us = Some(report.lead_ns / 1000);
        Some(report)
    }

    pub fn reset(&mut self) { *self = Self::default(); }
}

#[cfg(windows)]
pub fn timing(renderer: *mut SDL_Renderer, refresh_hz: f32) -> Option<(i64, i64)> {
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
        let latch = stats.filter(|s| s.SyncQPCTime > 0).map(|s| s.SyncQPCTime as u64);
        let (latch, period_ns) = if let Some(latch) = latch { (latch, period_ns) } else {
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
pub fn timing(renderer: *mut SDL_Renderer, _: f32) -> Option<(i64, i64)> {
    type Sync = unsafe extern "C" fn(*mut std::ffi::c_void, libc::c_ulong, *mut i64, *mut i64, *mut i64) -> i32;
    type Rate = unsafe extern "C" fn(*mut std::ffi::c_void, libc::c_ulong, *mut i32, *mut i32) -> i32;
    unsafe {
        if SDL_GL_GetCurrentContext().is_null() { return None; }
        let props = SDL_GetWindowProperties(SDL_GetRenderWindow(renderer));
        let display = SDL_GetPointerProperty(props, SDL_PROP_WINDOW_X11_DISPLAY_POINTER, std::ptr::null_mut());
        let drawable = SDL_GetNumberProperty(props, SDL_PROP_WINDOW_X11_WINDOW_NUMBER, 0) as libc::c_ulong;
        if display.is_null() || drawable == 0 { return None; }
        let sync: Sync = std::mem::transmute(SDL_GL_GetProcAddress(c"glXGetSyncValuesOML".as_ptr())?);
        let rate: Rate = std::mem::transmute(SDL_GL_GetProcAddress(c"glXGetMscRateOML".as_ptr())?);
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
            report = phase.sample(now - 2000, now, Some((now as i64, 10_000_000))).or(report);
        }
        let report = report.unwrap();
        assert_eq!(report.lead_ns, 2_000_000);
        assert!(Report::decode(&report.encode()).is_some());
        assert_eq!(phase.sample(700_000, 700_000, None), None);
        assert_eq!(phase.lead_us, None);
        assert_eq!(phase.sample(800_000, 800_000, Some((800_000, 10_000_000))), None);
        assert_eq!(phase.sample(800_000, 1_100_000, Some((1_100_000, 10_000_000))), None);
    }
}
