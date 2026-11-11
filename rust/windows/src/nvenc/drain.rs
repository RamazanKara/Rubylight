//! The wait behind a synchronous drain: frames in flight finish before
//! reference invalidation, a bitrate reset or teardown.

use anyhow::{Result, bail};
use std::time::Duration;

/// What one wait on the oldest frame in flight did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DrainWait {
    /// Blocked on the frame's completion handle until it signalled or the
    /// time ran out.
    Waited,
    /// The frame is already written: collect it again without blocking.
    Ready,
    /// The frame has no completion handle: pause briefly and poll.
    Unavailable,
}

/// The session side of [`drain_pending`].
pub(super) trait DrainWaiter {
    /// Collects the frames that finished and returns how many remain in flight.
    fn collect_pending(&mut self) -> Result<usize>;
    /// Blocks at most `timeout` on the oldest frame in flight.
    fn wait_oldest(&mut self, timeout: Duration) -> Result<DrainWait>;
    /// The time since the drain began.
    fn elapsed(&self) -> Duration;
    /// Yields briefly when there is no handle to wait on.
    fn pause(&mut self);
}

/// Collects frames until none is in flight, waiting on the oldest frame's
/// completion instead of polling. Fails once `budget` is spent with frames
/// still in flight.
pub(super) fn drain_pending(waiter: &mut impl DrainWaiter, budget: Duration) -> Result<()> {
    // In flight after the last collect that followed a `Ready`: a written frame
    // the lock still refuses (LOCK_BUSY) must not turn the loop into a spin.
    let mut ready_left = None;
    loop {
        let left = waiter.collect_pending()?;
        if left == 0 {
            return Ok(());
        }
        if ready_left == Some(left) {
            waiter.pause();
        }
        let elapsed = waiter.elapsed();
        if elapsed >= budget {
            bail!("NVENC drain timed out");
        }
        ready_left = None;
        match waiter.wait_oldest(budget - elapsed)? {
            DrainWait::Waited => {}
            DrainWait::Ready => ready_left = Some(left),
            DrainWait::Unavailable => waiter.pause(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUDGET: Duration = Duration::from_millis(100);
    const TICK: Duration = Duration::from_millis(1);

    /// A waiter on a simulated clock: a timed-out wait spends its whole
    /// timeout, and every other wait or pause spends one tick.
    struct Fake {
        in_flight: usize,
        has_handle: bool,
        /// Waits that wake early without completing the frame.
        early_wakes: usize,
        /// Every wait times out, as a hung driver does.
        never_completes: bool,
        /// The frame is written but its lock stays busy.
        lock_busy: bool,
        /// The pause that completes the frame, when there is no handle.
        pauses_to_complete: usize,
        clock: Duration,
        timeouts: Vec<Duration>,
        pauses: usize,
    }

    impl Fake {
        fn new(in_flight: usize, has_handle: bool) -> Self {
            Self {
                in_flight,
                has_handle,
                early_wakes: 0,
                never_completes: false,
                lock_busy: false,
                pauses_to_complete: usize::MAX,
                clock: Duration::ZERO,
                timeouts: Vec::new(),
                pauses: 0,
            }
        }
    }

    impl DrainWaiter for Fake {
        fn collect_pending(&mut self) -> Result<usize> {
            Ok(self.in_flight)
        }
        fn wait_oldest(&mut self, timeout: Duration) -> Result<DrainWait> {
            self.timeouts.push(timeout);
            if !self.has_handle {
                return Ok(DrainWait::Unavailable);
            }
            if self.lock_busy {
                // The fence is past the frame but the bitstream lock refuses it:
                // a `Ready` that costs no time.
                return Ok(DrainWait::Ready);
            }
            if self.never_completes {
                self.clock += timeout;
                return Ok(DrainWait::Waited);
            }
            self.clock += TICK;
            if self.early_wakes > 0 {
                self.early_wakes -= 1;
            } else {
                self.in_flight = 0;
            }
            Ok(DrainWait::Waited)
        }
        fn elapsed(&self) -> Duration {
            self.clock
        }
        fn pause(&mut self) {
            self.pauses += 1;
            self.clock += TICK;
            if self.pauses >= self.pauses_to_complete {
                self.in_flight = 0;
            }
        }
    }

    #[test]
    fn drain_completes_immediately_when_nothing_is_in_flight() {
        let mut fake = Fake::new(0, true);
        drain_pending(&mut fake, BUDGET).unwrap();
        assert!(fake.timeouts.is_empty());
        assert_eq!(fake.pauses, 0);
    }

    #[test]
    fn drain_completes_on_the_first_wait() {
        let mut fake = Fake::new(1, true);
        drain_pending(&mut fake, BUDGET).unwrap();
        assert_eq!(fake.timeouts, vec![BUDGET]);
        assert_eq!(fake.pauses, 0);
    }

    #[test]
    fn drain_waits_through_early_wakes_with_the_remaining_budget() {
        let mut fake = Fake::new(1, true);
        fake.early_wakes = 2;
        drain_pending(&mut fake, BUDGET).unwrap();
        assert_eq!(
            fake.timeouts,
            vec![
                Duration::from_millis(100),
                Duration::from_millis(99),
                Duration::from_millis(98),
            ]
        );
        assert_eq!(fake.pauses, 0);
    }

    #[test]
    fn drain_times_out_when_the_frame_never_completes() {
        let mut fake = Fake::new(1, true);
        fake.never_completes = true;
        let error = drain_pending(&mut fake, BUDGET).unwrap_err();
        assert!(error.to_string().contains("NVENC drain timed out"));
        assert_eq!(fake.timeouts, vec![BUDGET]);
        assert_eq!(fake.in_flight, 1);
    }

    #[test]
    fn drain_pauses_when_the_waiter_has_no_handle() {
        let mut fake = Fake::new(1, false);
        fake.pauses_to_complete = 2;
        drain_pending(&mut fake, BUDGET).unwrap();
        assert_eq!(fake.pauses, 2);
    }

    #[test]
    fn drain_times_out_polling_when_the_waiter_has_no_handle() {
        let mut fake = Fake::new(1, false);
        let error = drain_pending(&mut fake, BUDGET).unwrap_err();
        assert!(error.to_string().contains("NVENC drain timed out"));
        assert_eq!(fake.pauses, 100);
    }

    #[test]
    fn a_written_frame_whose_lock_stays_busy_pauses_instead_of_spinning() {
        let mut fake = Fake::new(1, true);
        fake.lock_busy = true;
        let error = drain_pending(&mut fake, BUDGET).unwrap_err();
        assert!(error.to_string().contains("NVENC drain timed out"));
        assert_eq!(fake.pauses, 100);
    }
}
