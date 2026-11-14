//! Fractional V-Sync: how many refreshes each picture of a V-Sync stream stays on a display that
//! refreshes at a whole multiple of the stream's rate (60 fps on 120 Hz: 2). Shown for exactly that
//! many refreshes, a picture that comes a little early or late no longer shows the one before it
//! for one refresh more or less. Android's `FramePacingPolicy.refreshesPerFrame` is its Java port.

/// How far a refresh may be from a whole multiple of the stream's rate and still count as one
/// (59.94 against 60), per multiple.
pub const RATE_TOLERANCE_HZ: f64 = 0.5;

/// The most refreshes a picture is held for: the swap chains' sync interval goes up to 4.
pub const MAX_REFRESHES: u32 = 4;

/// The multiple of `fps` that `refresh_hz` is (1 for the same rate, 2 for twice it, up to 4), or
/// None when it is none of them.
pub fn rate_multiple(refresh_hz: f64, fps: f64) -> Option<u32> {
    if fps.is_nan() || refresh_hz.is_nan() || fps <= 0.0 || refresh_hz <= 0.0 {
        return None;
    }
    (1..=MAX_REFRESHES).find(|&k| (refresh_hz - fps * f64::from(k)).abs() <= RATE_TOLERANCE_HZ * f64::from(k))
}

/// The refreshes each picture is shown for: the multiple the display's refresh is of the stream's
/// rate with fractional V-Sync on (`fractional`), else 1. A VRR display refreshes when a picture
/// comes, so it takes 1 too, as does a refresh that is no whole multiple (75 Hz for 60 fps).
pub fn refreshes_per_frame(refresh_hz: f64, fps: f64, vrr: bool, fractional: bool) -> u32 {
    if !fractional || vrr {
        return 1;
    }
    rate_multiple(refresh_hz, fps).unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_multiples_hold_a_picture_that_many_refreshes() {
        assert_eq!(refreshes_per_frame(120.0, 60.0, false, true), 2);
        assert_eq!(refreshes_per_frame(120.0, 40.0, false, true), 3);
        assert_eq!(refreshes_per_frame(120.0, 30.0, false, true), 4);
        assert_eq!(refreshes_per_frame(240.0, 60.0, false, true), 4);
        assert_eq!(refreshes_per_frame(60.0, 30.0, false, true), 2);
        assert_eq!(refreshes_per_frame(120.0, 120.0, false, true), 1, "the display's own rate");
    }

    #[test]
    fn near_rates_count_within_the_tolerance() {
        assert_eq!(refreshes_per_frame(119.88, 60.0, false, true), 2);
        assert_eq!(refreshes_per_frame(119.88, 59.94, false, true), 2);
        assert_eq!(refreshes_per_frame(143.9, 72.0, false, true), 2);
        assert_eq!(refreshes_per_frame(121.5, 60.0, false, true), 1, "1.5 Hz off twice the rate is no multiple");
        assert_eq!(refreshes_per_frame(118.9, 60.0, false, true), 1);
    }

    #[test]
    fn no_multiple_vrr_or_off_shows_each_picture_at_the_next_refresh() {
        assert_eq!(refreshes_per_frame(75.0, 60.0, false, true), 1);
        assert_eq!(refreshes_per_frame(144.0, 60.0, false, true), 1);
        assert_eq!(refreshes_per_frame(240.0, 50.0, false, true), 1, "more than 4 refreshes a picture");
        assert_eq!(refreshes_per_frame(120.0, 60.0, true, true), 1, "VRR");
        assert_eq!(refreshes_per_frame(120.0, 60.0, false, false), 1, "off");
        assert_eq!(refreshes_per_frame(0.0, 60.0, false, true), 1, "unknown refresh");
        assert_eq!(refreshes_per_frame(120.0, 0.0, false, true), 1, "unknown rate");
        assert_eq!(refreshes_per_frame(f64::NAN, 60.0, false, true), 1);
    }

    #[test]
    fn rate_multiple_finds_the_smallest_whole_multiple() {
        assert_eq!(rate_multiple(60.0, 60.0), Some(1));
        assert_eq!(rate_multiple(59.94, 60.0), Some(1));
        assert_eq!(rate_multiple(120.0, 60.0), Some(2));
        assert_eq!(rate_multiple(100.0, 60.0), None);
    }
}
