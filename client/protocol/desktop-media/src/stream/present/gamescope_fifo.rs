//! Leaving gamescope's FIFO queue once it holds a V-Sync stream's pictures back (Nightjar #181).
//!
//! Gamescope's Vulkan layer gives the driver MAILBOX and runs FIFO itself, in its own queue of
//! commits, each FIFO commit held behind the one before it, with the swapchain's three images. On
//! the issue's Odin 2 Portal (120 Hz, gamescope 3.17), after a change of the game's rate a 60 fps
//! stream showed 34-42 fps for the rest of the stream: its presents waited 27.9 ms at p95, over
//! three refreshes, and the pictures that came meanwhile were replaced, while every one arrived
//! and decoded in 1 ms. Nothing on Nightjar's side held that state: the pacer takes the newest due
//! picture, the stream's 120 fps held each picture one refresh (no fractional V-Sync), and the
//! PC's phase lock stood aside throughout.
//!
//! The stream keeps FIFO as before. Only when its own presents keep waiting longer than a refresh
//! and a half while pictures are replaced, second after second, does it ask for MAILBOX (`FifoWatch`),
//! which does not tear either: gamescope shows the newest finished picture at its refresh and
//! keeps no queue of them. That is said in the log and to the player. A gamescope frame limit
//! makes gamescope force FIFO whatever is asked; the switch then changes nothing.

use super::PacingMode;
#[cfg(test)]
use super::{in_order_pacing, FramePacing};

/// Whether a stream is one whose presents the watch reads: V-Sync under gamescope on the GPU
/// (Vulkan) renderer, whose swapchain goes through gamescope's layer, with a frame pacing that
/// takes the newest due picture. The in-order pacings keep FIFO's one picture a refresh.
pub fn gamescope_fifo_watched(mode: PacingMode, in_order: bool, gamescope: bool, vulkan: bool) -> bool {
    mode == PacingMode::Vsync && !in_order && gamescope && vulkan
}

/// How long one look at the presents lasts.
const WINDOW_US: u64 = 1_000_000;
/// Looks in a row that must find the queue holding pictures back.
const STUCK_WINDOWS: u32 = 3;
/// Presents a look needs to say anything: a stream that shows fewer pictures is not judged.
const MIN_PRESENTS: u32 = 10;

/// Watches a V-Sync stream's presents for gamescope's FIFO queue holding pictures back: in each
/// second, at least half of the presents waited over a refresh and a half, and pictures were
/// replaced for at least one in ten of them. A queue that drains waits at most a refresh: a stream
/// at the display's rate is let through one picture a refresh, a slower one at once.
#[derive(Debug, Default)]
pub struct FifoWatch {
    window_start_us: Option<u64>,
    presents: u32,
    slow: u32,
    replaced: u32,
    stuck_windows: u32,
    /// The stream asked for MAILBOX.
    pub left: bool,
}

impl FifoWatch {
    /// A picture's present returned at `now_us` after waiting `wait_us`, on a display refreshing
    /// every `refresh_us`, with `replaced` pictures dropped for it. True once, when the stream
    /// should leave FIFO.
    pub fn presented(&mut self, now_us: u64, wait_us: u64, refresh_us: u64, replaced: u32) -> bool {
        if self.left || refresh_us == 0 { return false; }
        let start = *self.window_start_us.get_or_insert(now_us);
        self.presents += 1;
        if wait_us.saturating_mul(2) > refresh_us.saturating_mul(3) { self.slow += 1; }
        self.replaced = self.replaced.saturating_add(replaced);
        if now_us.saturating_sub(start) < WINDOW_US { return false; }
        let stuck = self.presents >= MIN_PRESENTS && self.slow * 2 >= self.presents && self.replaced.saturating_mul(10) >= self.presents;
        self.stuck_windows = if stuck { self.stuck_windows + 1 } else { 0 };
        (self.window_start_us, self.presents, self.slow, self.replaced) = (Some(now_us), 0, 0, 0);
        self.left = self.stuck_windows >= STUCK_WINDOWS;
        self.left
    }

    /// The stream starts afresh (a new display or swapchain): what was seen says nothing now.
    pub fn restart(&mut self) {
        *self = Self { left: self.left, ..Self::default() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFRESH: u64 = 8_333;

    /// Feeds `seconds` of presents at `shown` a second, each waiting `wait_us`, with `replaced`
    /// pictures dropped a second; returns the second it asked to leave in, if it did.
    fn run(watch: &mut FifoWatch, from_us: u64, seconds: u64, shown: u64, wait_us: u64, replaced: u64) -> Option<u64> {
        let mut left = None;
        for second in 0..seconds {
            for n in 0..shown {
                let now = from_us + second * 1_000_000 + n * 1_000_000 / shown;
                if watch.presented(now, wait_us, REFRESH, u32::from(n < replaced)) && left.is_none() { left = Some(second); }
            }
        }
        left
    }

    #[test]
    fn the_issues_stuck_queue_leaves_fifo_after_three_seconds() {
        // #181: 60 fps received, 37 shown with 27 ms present waits and 23 replaced a second.
        let mut watch = FifoWatch::default();
        assert_eq!(run(&mut watch, 0, 10, 37, 27_000, 23), Some(3));
        assert!(watch.left);
        // Once: nothing more to ask for.
        assert_eq!(run(&mut watch, 10_000_000, 10, 37, 27_000, 23), None);
        // At a 30 fps source the stuck queue showed 23 of 30 with 41 ms waits.
        let mut slow_source = FifoWatch::default();
        assert!(run(&mut slow_source, 0, 10, 23, 41_000, 7).is_some());
    }

    #[test]
    fn a_draining_queue_keeps_fifo() {
        // 120 fps on 120 Hz: each present waits up to a refresh, now and then a picture replaced.
        let mut watch = FifoWatch::default();
        assert_eq!(run(&mut watch, 0, 30, 118, REFRESH, 2), None);
        // 60 fps on 120 Hz: presents return at once.
        assert_eq!(run(&mut watch, 30_000_000, 30, 60, 1_000, 0), None);
        // Long waits without replaced pictures (the PC sends slower than the display) are no queue.
        assert_eq!(run(&mut watch, 60_000_000, 30, 30, 30_000, 0), None);
        // Replaced pictures with short waits (a burst from the network) are no queue either.
        assert_eq!(run(&mut watch, 90_000_000, 30, 100, 4_000, 20), None);
        assert!(!watch.left);
    }

    #[test]
    fn a_second_that_drains_starts_the_count_again() {
        let mut watch = FifoWatch::default();
        // Two stuck seconds, one that drains, two stuck again: not three in a row.
        assert_eq!(run(&mut watch, 0, 2, 37, 27_000, 23), None);
        assert_eq!(run(&mut watch, 2_000_000, 1, 60, 1_000, 0), None);
        assert_eq!(run(&mut watch, 3_000_000, 2, 37, 27_000, 23), None);
        assert!(!watch.left);
        // Too few presents to judge.
        let mut few = FifoWatch::default();
        assert_eq!(run(&mut few, 0, 10, 8, 100_000, 8), None);
        // A restart forgets what was seen, but not that the stream left.
        let mut restarted = FifoWatch::default();
        assert_eq!(run(&mut restarted, 0, 2, 37, 27_000, 23), None);
        restarted.restart();
        assert_eq!(run(&mut restarted, 5_000_000, 2, 37, 27_000, 23), None);
        assert!(run(&mut restarted, 7_000_000, 3, 37, 27_000, 23).is_some());
        restarted.restart();
        assert!(restarted.left);
    }

    #[test]
    fn only_gamescope_v_sync_streams_taking_the_newest_picture_on_vulkan_are_watched() {
        // The issue's settings: Lowest latency with V-Sync, and the PC's frame times (6.3.2).
        assert!(gamescope_fifo_watched(PacingMode::Vsync, in_order_pacing(FramePacing::Latency, true, false), true, true));
        assert!(gamescope_fifo_watched(PacingMode::Vsync, in_order_pacing(FramePacing::Latency, true, true), true, true));
        assert!(gamescope_fifo_watched(PacingMode::Vsync, in_order_pacing(FramePacing::Balanced, true, false), true, true));
        for (pacing, drop_late) in [(FramePacing::CapFps, true), (FramePacing::Smoothness, true), (FramePacing::Balanced, false)] {
            assert!(!gamescope_fifo_watched(PacingMode::Vsync, in_order_pacing(pacing, drop_late, false), true, true), "{pacing:?} keeps FIFO");
        }
        assert!(!gamescope_fifo_watched(PacingMode::Vsync, false, false, true), "a desktop compositor is not watched");
        assert!(!gamescope_fifo_watched(PacingMode::Vsync, false, true, false), "OpenGL does not use gamescope's Vulkan layer");
        for mode in [PacingMode::Immediate, PacingMode::HostTimes, PacingMode::BeamRace] {
            assert!(!gamescope_fifo_watched(mode, false, true, true), "{mode:?} has its own present mode");
        }
    }
}
