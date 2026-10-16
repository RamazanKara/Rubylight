//! The capture source's cadence in nanoseconds and its idle-wait policy, independent of
//! Windows/GPU APIs.
use std::time::Duration;

/// Preserve short active polls and pending GPU handoffs. After 100 ms without
/// a frame, notifications can interrupt a longer wait; the 10 ms bound still
/// services stop and device checks.
pub fn notification_wait_interval(idle: Duration, pending: bool, poll: Duration) -> Duration {
    if pending || idle < Duration::from_millis(100) {
        poll
    } else {
        poll.max(Duration::from_millis(10))
    }
}

/// The recent intervals between source frames.
#[derive(Default)]
pub struct Freshness {
    last: Option<u64>,
    intervals: [u64; 16],
    count: usize,
    next: usize,
    median: Option<u64>,
    mean: Option<u64>,
}
impl Freshness {
    /// The median interval between recent source frames, once a few are known.
    /// The capture worker observes every frame, so this is the source cadence
    /// even when a consumer only looks at some of them.
    pub fn median_interval(&self) -> Option<u64> {
        self.median
    }
    /// The mean of the same intervals: the source's rate. A compositor that
    /// alternates between one and two refreshes per picture (4 and 8 ms on a
    /// 240 Hz display) has a median of either, but delivers about 160 frames a
    /// second either way.
    pub fn mean_interval(&self) -> Option<u64> {
        self.mean
    }
    pub fn observe(&mut self, captured: u64) {
        if let Some(previous) = self.last {
            let interval = captured.saturating_sub(previous);
            if captured > previous && interval <= 500_000_000 {
                self.intervals[self.next] = interval;
                self.next = (self.next + 1) % self.intervals.len();
                self.count = (self.count + 1).min(self.intervals.len());
            } else {
                self.count = 0;
                self.next = 0;
            }
        }
        self.last = Some(captured);
        // Consumers read the same history many times between frames. Recompute
        // its statistics only when that history changes.
        self.median = None;
        self.mean = None;
        if self.count >= 4 {
            self.mean = Some(self.intervals[..self.count].iter().sum::<u64>() / self.count as u64);
            let mut intervals = self.intervals;
            intervals[..self.count].sort_unstable();
            self.median = Some(intervals[self.count / 2]);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn static_capture_sleeps_longer_only_without_a_pending_gpu_handoff() {
        let poll = Duration::from_micros(500);
        assert_eq!(
            notification_wait_interval(Duration::ZERO, false, poll),
            poll
        );
        assert_eq!(
            notification_wait_interval(Duration::from_millis(99), false, poll),
            poll
        );
        assert_eq!(
            notification_wait_interval(Duration::from_millis(100), false, poll),
            Duration::from_millis(10)
        );
        assert_eq!(
            notification_wait_interval(Duration::from_secs(1), true, poll),
            poll
        );
    }
    #[test]
    fn cached_statistics_follow_cadence_changes_and_resets() {
        let mut cadence = Freshness::default();
        let mut captured = 1_000_000_000u64;
        for frame in 0..2000u64 {
            let period = [16_666_667, 8_333_333, 2_000_000, 20_000_000][(frame / 40 % 4) as usize];
            captured = match frame % 97 {
                0 => captured.saturating_sub(50_000_000),
                1 => captured,
                2 => captured + 600_000_000,
                _ => captured + period + (frame % 5) * 10_000,
            };
            cadence.observe(captured);
            let mut intervals = cadence.intervals;
            intervals[..cadence.count].sort_unstable();
            assert_eq!(
                cadence.median_interval(),
                (cadence.count >= 4).then(|| intervals[cadence.count / 2])
            );
            assert_eq!(
                cadence.mean_interval(),
                (cadence.count >= 4)
                    .then(|| intervals[..cadence.count].iter().sum::<u64>() / cadence.count as u64)
            );
        }
    }
}
