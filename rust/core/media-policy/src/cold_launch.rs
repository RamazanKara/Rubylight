//! Waking the console display during a cold launch (issue #117). A launch on a
//! locked PC, or one whose display Windows switched off, gets no first frame:
//! Windows stops composing a physical display that is off, and a Rubylight
//! virtual display's IddCx path stays committed without a swap chain. The host
//! holds a display-required power request for the whole stream, and while it
//! waits for the first frame it wakes the display with the net-zero mouse move
//! a client's first input makes, on the input desktop (the lock screen's when
//! the PC is locked). A launch whose first frame arrives in time never wakes
//! anything.

use std::time::Duration;

/// How long the first-frame wait goes without a frame before each wake: the
/// first after a second, then again while the display powers up, all within
/// the wait's 10 seconds.
pub const WAKES: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(3),
    Duration::from_secs(6),
];

/// Whether the first-frame wait wakes the console display now, after
/// `waited` without a frame and `sent` wakes so far. `swap_chain` is the
/// virtual display's IddCx state: `Some(false)` when the path has no swap
/// chain, as when the display is off; `Some(true)` when Windows composes it,
/// which a wake cannot help; None for a physical display (or one the driver
/// does not report), which a wake can switch back on.
pub fn wake_now(waited: Duration, sent: usize, swap_chain: Option<bool>) -> bool {
    swap_chain != Some(true) && WAKES.get(sent).is_some_and(|after| waited >= *after)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn a_launch_that_gets_its_first_frame_in_time_wakes_nothing() {
        for swap_chain in [None, Some(false), Some(true)] {
            assert!(!wake_now(Duration::ZERO, 0, swap_chain));
            assert!(!wake_now(999 * MS, 0, swap_chain));
        }
    }

    #[test]
    fn a_physical_display_that_stays_dark_is_woken_too() {
        // Issue #117: before, only a virtual display without a swap chain was
        // woken, once; a locked PC's physical display never was.
        assert!(wake_now(Duration::from_secs(1), 0, None));
        assert!(wake_now(Duration::from_secs(1), 0, Some(false)));
        // A virtual display Windows composes is not off.
        assert!(!wake_now(Duration::from_secs(5), 0, Some(true)));
    }

    #[test]
    fn the_wake_repeats_while_the_display_powers_up_then_stops() {
        let display = None;
        assert!(!wake_now(2999 * MS, 1, display));
        assert!(wake_now(Duration::from_secs(3), 1, display));
        assert!(!wake_now(Duration::from_secs(5), 2, display));
        assert!(wake_now(Duration::from_secs(6), 2, display));
        // Bounded: three wakes in all, the last well before the 10 s deadline.
        assert!(!wake_now(Duration::from_secs(9), 3, display));
        assert!(WAKES.iter().all(|after| *after < Duration::from_secs(10)));
        // Each one once: a sent wake waits for the next step.
        let mut sent = 0;
        for tenth in 0..100 {
            if wake_now(Duration::from_millis(tenth * 100), sent, Some(false)) {
                sent += 1;
            }
        }
        assert_eq!(sent, WAKES.len());
    }
}
