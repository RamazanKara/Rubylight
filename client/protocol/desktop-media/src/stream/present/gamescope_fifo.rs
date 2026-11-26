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
//! and a half while pictures are replaced, second after second, does it ask for MAILBOX
//! (`FifoWatch`), which does not tear either: gamescope shows the newest finished picture at its
//! refresh and keeps no queue of them. A gamescope frame limit forces FIFO whatever is asked, and
//! holds presents back the same way, so the next second tells: presents that stopped waiting keep
//! MAILBOX and the player is told; presents that still wait go back to FIFO, said in the log only.

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

/// What a look at the presents found, for the stream to act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FifoStep {
    /// The queue held pictures back for `STUCK_WINDOWS` looks: ask for MAILBOX and call
    /// [`FifoWatch::asked`], or leave it and the watch counts afresh.
    Leave,
    /// On MAILBOX the presents stopped waiting: it stays, and the player is told.
    Left,
    /// On MAILBOX the presents still wait (gamescope's frame limit keeps its FIFO): back to FIFO.
    Held,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Watching,
    /// MAILBOX was asked for; the next look tells whether it took.
    Trying,
    Left,
    Held,
}

/// Watches a V-Sync stream's presents for gamescope's FIFO queue holding pictures back: in each
/// second, at least half of the presents waited over a refresh and a half, and pictures were
/// replaced for at least one in ten of them. A queue that drains waits at most a refresh: a stream
/// at the display's rate is let through one picture a refresh, a slower one at once.
#[derive(Debug, Default)]
pub struct FifoWatch {
    state: State,
    window_start_us: Option<u64>,
    presents: u32,
    slow: u32,
    replaced: u32,
    stuck_windows: u32,
}

impl FifoWatch {
    /// A picture's present returned at `now_us` after waiting `wait_us`, on a display refreshing
    /// every `refresh_us`, with `replaced` pictures dropped for it. Says what a finished look found.
    pub fn presented(&mut self, now_us: u64, wait_us: u64, refresh_us: u64, replaced: u32) -> Option<FifoStep> {
        if self.settled() || refresh_us == 0 { return None; }
        let start = *self.window_start_us.get_or_insert(now_us);
        self.presents += 1;
        if wait_us.saturating_mul(2) > refresh_us.saturating_mul(3) { self.slow += 1; }
        self.replaced = self.replaced.saturating_add(replaced);
        if now_us.saturating_sub(start) < WINDOW_US { return None; }
        let (presents, slow, replaced) = (self.presents, self.slow, self.replaced);
        (self.window_start_us, self.presents, self.slow, self.replaced) = (Some(now_us), 0, 0, 0);
        if presents < MIN_PRESENTS {
            if self.state == State::Watching { self.stuck_windows = 0; }
            return None;
        }
        let waiting = slow * 2 >= presents;
        match self.state {
            State::Watching => {
                let stuck = waiting && replaced.saturating_mul(10) >= presents;
                self.stuck_windows = if stuck { self.stuck_windows + 1 } else { 0 };
                if self.stuck_windows < STUCK_WINDOWS { return None; }
                // Counts afresh unless the stream asks (`asked`).
                self.stuck_windows = 0;
                Some(FifoStep::Leave)
            }
            State::Trying if waiting => { self.state = State::Held; Some(FifoStep::Held) }
            State::Trying => { self.state = State::Left; Some(FifoStep::Left) }
            State::Left | State::Held => None,
        }
    }

    /// The stream asked for MAILBOX after [`FifoStep::Leave`]: the look starting now says whether
    /// it took.
    pub fn asked(&mut self) {
        *self = Self { state: State::Trying, ..Self::default() };
    }

    /// The stream keeps FIFO for good: MAILBOX could not be asked for.
    pub fn stay(&mut self) {
        self.state = State::Held;
    }

    /// Whether the stream asks for MAILBOX: while it is tried, and once it took.
    pub fn mailbox(&self) -> bool {
        matches!(self.state, State::Trying | State::Left)
    }

    /// Whether the watch is done: MAILBOX took, or gamescope kept its FIFO.
    pub fn settled(&self) -> bool {
        matches!(self.state, State::Left | State::Held)
    }

    /// The stream starts afresh (a new display or swapchain): what was seen says nothing now, but
    /// what was asked for and decided stays.
    pub fn restart(&mut self) {
        *self = Self { state: self.state, ..Self::default() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFRESH: u64 = 8_333;

    /// Feeds `seconds` of presents at `shown` a second, each waiting `wait_us`, with `replaced`
    /// pictures dropped a second; returns the first step and the second it came in, if one did.
    fn run(watch: &mut FifoWatch, from_us: u64, seconds: u64, shown: u64, wait_us: u64, replaced: u64) -> Option<(FifoStep, u64)> {
        let mut step = None;
        for second in 0..seconds {
            for n in 0..shown {
                let now = from_us + second * 1_000_000 + n * 1_000_000 / shown;
                if let Some(found) = watch.presented(now, wait_us, REFRESH, u32::from(n < replaced)) && step.is_none() {
                    step = Some((found, second));
                }
            }
        }
        step
    }

    #[test]
    fn the_issues_stuck_queue_leaves_fifo_and_tells_once_presents_stop_waiting() {
        // #181: 60 fps received, 37 shown with 27 ms present waits and 23 replaced a second.
        let mut watch = FifoWatch::default();
        assert_eq!(run(&mut watch, 0, 4, 37, 27_000, 23), Some((FifoStep::Leave, 3)));
        assert!(!watch.mailbox(), "nothing is asked for until the stream asks");
        watch.asked();
        assert!(watch.mailbox());
        // On MAILBOX every picture is shown and presents return at once.
        assert_eq!(run(&mut watch, 4_000_000, 2, 60, 500, 0), Some((FifoStep::Left, 1)));
        assert!(watch.mailbox() && watch.settled());
        // Once: nothing more to say.
        assert_eq!(run(&mut watch, 6_000_000, 10, 37, 27_000, 23), None);
        // At a 30 fps source the stuck queue showed 23 of 30 with 41 ms waits.
        let mut slow_source = FifoWatch::default();
        assert!(matches!(run(&mut slow_source, 0, 10, 23, 41_000, 7), Some((FifoStep::Leave, _))));
    }

    #[test]
    fn a_gamescope_frame_limit_keeps_fifo_without_telling_the_player() {
        // A 60 fps limit on a 120 Hz panel with a 120 fps stream: every present waits a limited
        // frame (16.7 ms) and half the pictures are replaced, which looks like the stuck queue.
        let mut watch = FifoWatch::default();
        assert_eq!(run(&mut watch, 0, 4, 60, 16_700, 60), Some((FifoStep::Leave, 3)));
        watch.asked();
        // The limit forces FIFO whatever is asked: the presents still wait, so back to FIFO.
        assert_eq!(run(&mut watch, 4_000_000, 2, 60, 16_700, 60), Some((FifoStep::Held, 1)));
        assert!(!watch.mailbox() && watch.settled());
        assert_eq!(run(&mut watch, 6_000_000, 10, 60, 16_700, 60), None);
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
        assert!(!watch.mailbox() && !watch.settled());
    }

    #[test]
    fn a_second_that_drains_or_a_stream_that_does_not_ask_starts_the_count_again() {
        let mut watch = FifoWatch::default();
        // Two stuck seconds, one that drains, two stuck again: not three in a row.
        assert_eq!(run(&mut watch, 0, 2, 37, 27_000, 23), None);
        assert_eq!(run(&mut watch, 2_000_000, 1, 60, 1_000, 0), None);
        assert_eq!(run(&mut watch, 3_000_000, 2, 37, 27_000, 23), None);
        // Too few presents to judge.
        let mut few = FifoWatch::default();
        assert_eq!(run(&mut few, 0, 10, 8, 100_000, 8), None);
        // A stream that could not ask (the device going away) hears again after three more.
        let mut unasked = FifoWatch::default();
        assert_eq!(run(&mut unasked, 0, 4, 37, 27_000, 23), Some((FifoStep::Leave, 3)));
        assert_eq!(run(&mut unasked, 4_000_000, 4, 37, 27_000, 23), Some((FifoStep::Leave, 2)));
        assert!(!unasked.mailbox());
    }

    #[test]
    fn a_restart_forgets_what_was_seen_but_not_what_was_decided() {
        let mut watch = FifoWatch::default();
        assert_eq!(run(&mut watch, 0, 2, 37, 27_000, 23), None);
        watch.restart();
        assert_eq!(run(&mut watch, 5_000_000, 2, 37, 27_000, 23), None);
        assert!(run(&mut watch, 7_000_000, 3, 37, 27_000, 23).is_some());
        watch.asked();
        watch.restart();
        assert!(watch.mailbox(), "a MAILBOX being tried stays asked for");
        assert_eq!(run(&mut watch, 10_000_000, 2, 60, 500, 0), Some((FifoStep::Left, 1)));
        watch.restart();
        assert!(watch.mailbox() && watch.settled());
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
