//! The refresh range of the window's display on macOS. `NSScreen` reports, since macOS 12, the
//! shortest and the longest interval between two refreshes; on a fixed-refresh screen the two are
//! equal, on ProMotion and Adaptive-Sync displays they differ. SDL's display mode can report 0 Hz
//! for ProMotion, so it is no measure of this.
//!
//! Ported from Nonary's moonlight-qt (GPL-3.0, commit 19758e5e6), app/streaming/
//! macdisplaytiming.{h,mm}: what is read from the screen and the tests for a valid and for a
//! variable range.

/// What `NSScreen` says about a display's refreshes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DisplayTiming {
    pub maximum_frames_per_second: i64,
    /// The shortest interval between two refreshes (s).
    pub minimum_refresh_interval: f64,
    /// The longest interval between two refreshes (s).
    pub maximum_refresh_interval: f64,
    pub display_update_granularity: f64,
}

impl DisplayTiming {
    /// Upstream `hasValidTiming`.
    pub fn valid(&self) -> bool {
        self.maximum_frames_per_second > 0
            && self.minimum_refresh_interval.is_finite() && self.minimum_refresh_interval > 0.0
            && self.maximum_refresh_interval.is_finite() && self.maximum_refresh_interval >= self.minimum_refresh_interval
            && self.display_update_granularity.is_finite() && self.display_update_granularity >= 0.0
    }

    /// Upstream `supportsVariableRefresh`: continuous Adaptive-Sync and discrete ProMotion ranges
    /// both qualify; a fixed-refresh screen reports equal intervals.
    pub fn variable_refresh(&self) -> bool {
        self.valid() && self.maximum_refresh_interval - self.minimum_refresh_interval > 0.000_001
    }

    /// The slowest and the fastest refresh rate (Hz), of a valid timing.
    pub fn range_hz(&self) -> (f64, f64) {
        (1.0 / self.maximum_refresh_interval, 1.0 / self.minimum_refresh_interval)
    }
}

/// The timing of the screen SDL's Cocoa window is on. `None` before macOS 12, for a window with no
/// screen and for a timing that is not valid.
///
/// # Safety
/// `cocoa_window` is null or SDL's live `NSWindow`, and this is the main thread, where the window
/// and its screen live.
#[cfg(target_os = "macos")]
pub unsafe fn of_window(cocoa_window: *mut std::ffi::c_void) -> Option<DisplayTiming> {
    use objc2::runtime::AnyObject;
    use objc2::{msg_send, sel};
    let window = cocoa_window.cast::<AnyObject>();
    if window.is_null() { return None; }
    let screen: *mut AnyObject = msg_send![window, screen];
    if screen.is_null() { return None; }
    // The four properties came with macOS 12.
    let known: bool = msg_send![screen, respondsToSelector: sel!(minimumRefreshInterval)];
    if !known { return None; }
    let maximum_frames_per_second: isize = msg_send![screen, maximumFramesPerSecond];
    let timing = DisplayTiming {
        maximum_frames_per_second: maximum_frames_per_second as i64,
        minimum_refresh_interval: msg_send![screen, minimumRefreshInterval],
        maximum_refresh_interval: msg_send![screen, maximumRefreshInterval],
        display_update_granularity: msg_send![screen, displayUpdateGranularity],
    };
    timing.valid().then_some(timing)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timing(fps: i64, shortest: f64, longest: f64, granularity: f64) -> DisplayTiming {
        DisplayTiming { maximum_frames_per_second: fps, minimum_refresh_interval: shortest, maximum_refresh_interval: longest, display_update_granularity: granularity }
    }

    #[test]
    fn a_range_is_variable_when_its_intervals_differ() {
        let promotion = timing(120, 1.0 / 120.0, 1.0 / 24.0, 1.0 / 240.0);
        assert!(promotion.valid() && promotion.variable_refresh());
        let (slowest, fastest) = promotion.range_hz();
        assert!((slowest - 24.0).abs() < 1e-6 && (fastest - 120.0).abs() < 1e-6);
        let adaptive = timing(144, 1.0 / 144.0, 1.0 / 48.0, 0.0);
        assert!(adaptive.variable_refresh(), "a granularity of zero is a continuous range");
        let fixed = timing(60, 1.0 / 60.0, 1.0 / 60.0, 1.0 / 60.0);
        assert!(fixed.valid() && !fixed.variable_refresh(), "equal intervals are a fixed refresh");
    }

    #[test]
    fn a_timing_that_is_not_valid_is_no_range() {
        assert!(!DisplayTiming::default().valid(), "macOS before 12 reports nothing");
        assert!(!timing(0, 1.0 / 120.0, 1.0 / 24.0, 0.0).variable_refresh(), "no frame rate");
        assert!(!timing(120, 0.0, 1.0 / 24.0, 0.0).variable_refresh(), "no shortest interval");
        assert!(!timing(120, 1.0 / 24.0, 1.0 / 120.0, 0.0).variable_refresh(), "the longest is shorter than the shortest");
        assert!(!timing(120, 1.0 / 120.0, f64::INFINITY, 0.0).variable_refresh());
        assert!(!timing(120, 1.0 / 120.0, 1.0 / 24.0, -1.0).variable_refresh());
        assert!(!timing(120, f64::NAN, 1.0 / 24.0, 0.0).variable_refresh());
    }
}
