//! How evenly the host sends video, against the stream's frame period, for the 5 s timing log
//! (#144). The microphone's and the pointer's arrival gaps ([`crate::arrival_gaps`]) show what
//! reaches the host; these show what leaves it. At 240 or 480 Hz a stutter the client sees is
//! either frames the host sent unevenly (bunched back to back, or late) or the network's and
//! the client's doing, and this line tells the two apart. A gap is the time between the last
//! packets of consecutive frames handed to the socket.

use std::time::{Duration, Instant};

/// A gap this long is a pause with nothing to send, such as a still picture, not a late frame
/// (as for arrivals, [`crate::arrival_gaps::PAUSE`]).
pub const PAUSE: Duration = crate::arrival_gaps::PAUSE;

/// One log line's counts. Each gap that is not a pause falls in one of the five classes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    /// Frames sent.
    pub frames: u32,
    /// Gaps under a quarter period: two frames sent back to back, as one burst.
    pub bunched: u32,
    /// From a quarter to three quarters of a period.
    pub short: u32,
    /// Within a quarter period of the period.
    pub on_time: u32,
    /// From one and a quarter to two periods: a frame late.
    pub late: u32,
    /// Two periods and more: a period passed without a frame.
    pub skipped: u32,
    /// Gaps of [`PAUSE`] and more.
    pub pauses: u32,
    /// The mean distance of a gap from the period, µs.
    pub jitter_us: u32,
    /// The longest gap that was not a pause, µs.
    pub max_gap_us: u32,
    /// The frame period of the last frame, µs.
    pub period_us: u32,
}

/// The gaps since the last [`take`](SendGaps::take).
#[derive(Debug, Default)]
pub struct SendGaps {
    last: Option<Instant>,
    window: Summary,
    deviation_us: u64,
    gaps: u32,
}

fn micros(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

fn clamp32(value: u64) -> u32 {
    value.min(u64::from(u32::MAX)) as u32
}

impl SendGaps {
    /// Counts a frame whose last packet left at `sent`, in a stream at `period`. A zero period
    /// (a sender that does not know it) counts nothing.
    pub fn note(&mut self, sent: Instant, period: Duration) {
        if period.is_zero() {
            return;
        }
        let w = &mut self.window;
        w.frames = w.frames.saturating_add(1);
        let period_us = micros(period).max(1);
        w.period_us = clamp32(period_us);
        let Some(last) = self.last.replace(sent) else {
            return;
        };
        let gap = sent.saturating_duration_since(last);
        if gap >= PAUSE {
            w.pauses = w.pauses.saturating_add(1);
            return;
        }
        let gap_us = micros(gap);
        // In quarter periods: under 1 bunched, 1-3 short, 3-5 on time, 5-8 late, 8+ skipped.
        let quarters = gap_us.saturating_mul(4) / period_us;
        let class = match quarters {
            0 => &mut w.bunched,
            1..=2 => &mut w.short,
            3..=4 => &mut w.on_time,
            5..=7 => &mut w.late,
            _ => &mut w.skipped,
        };
        *class = class.saturating_add(1);
        w.max_gap_us = w.max_gap_us.max(clamp32(gap_us));
        self.deviation_us = self.deviation_us.saturating_add(gap_us.abs_diff(period_us));
        self.gaps = self.gaps.saturating_add(1);
    }

    /// What the frames since the last call showed; the next window starts from the last frame.
    pub fn take(&mut self) -> Summary {
        let mut summary = std::mem::take(&mut self.window);
        if self.gaps > 0 {
            summary.jitter_us = clamp32(self.deviation_us / u64::from(self.gaps));
        }
        self.deviation_us = 0;
        self.gaps = 0;
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(period: Duration, gaps_us: &[u64]) -> Summary {
        let mut gaps = SendGaps::default();
        let mut at = Instant::now();
        gaps.note(at, period);
        for gap in gaps_us {
            at += Duration::from_micros(*gap);
            gaps.note(at, period);
        }
        gaps.take()
    }

    #[test]
    fn an_even_stream_is_on_time_with_no_jitter() {
        let period = Duration::from_micros(4_166);
        let summary = run(period, &[4_166; 99]);
        assert_eq!(summary.frames, 100);
        assert_eq!(summary.on_time, 99);
        assert_eq!(
            (
                summary.bunched,
                summary.short,
                summary.late,
                summary.skipped
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(summary.jitter_us, 0);
        assert_eq!(summary.max_gap_us, 4_166);
        assert_eq!(summary.period_us, 4_166);
    }

    #[test]
    fn frames_sent_in_pairs_show_as_bunched_and_late() {
        // 240 Hz frames leaving two at a time: 0.2 ms apart, then 8.1 ms to the next pair.
        let period = Duration::from_micros(4_166);
        let summary = run(period, &[200, 8_132, 200, 8_132, 200]);
        assert_eq!(summary.bunched, 3);
        assert_eq!(summary.late, 2);
        assert_eq!((summary.on_time, summary.skipped), (0, 0));
        assert_eq!(summary.max_gap_us, 8_132);
        // |200 - 4166| = 3966 three times, |8132 - 4166| = 3966 twice.
        assert_eq!(summary.jitter_us, 3_966);
    }

    #[test]
    fn each_class_has_its_bounds_in_quarter_periods() {
        let period = Duration::from_micros(4_000);
        let summary = run(
            period,
            &[
                999, 1_000, 2_999, 3_000, 4_999, 5_000, 7_999, 8_000, 249_999,
            ],
        );
        assert_eq!(summary.bunched, 1);
        assert_eq!(summary.short, 2);
        assert_eq!(summary.on_time, 2);
        assert_eq!(summary.late, 2);
        assert_eq!(summary.skipped, 2);
        assert_eq!(summary.pauses, 0);
        assert_eq!(summary.max_gap_us, 249_999);
    }

    #[test]
    fn a_pause_is_not_a_late_frame() {
        let period = Duration::from_micros(4_166);
        let summary = run(period, &[4_166, 300_000, 4_166]);
        assert_eq!(summary.pauses, 1);
        assert_eq!(summary.on_time, 2);
        assert_eq!(summary.skipped, 0);
        assert_eq!(summary.max_gap_us, 4_166);
    }

    #[test]
    fn take_starts_the_next_window_from_the_last_frame() {
        let period = Duration::from_micros(8_333);
        let mut gaps = SendGaps::default();
        let start = Instant::now();
        gaps.note(start, period);
        gaps.note(start + period, period);
        let first = gaps.take();
        assert_eq!((first.frames, first.on_time), (2, 1));
        gaps.note(start + period * 3, period);
        let second = gaps.take();
        // The gap across the boundary counts in the second window.
        assert_eq!((second.frames, second.skipped, second.on_time), (1, 1, 0));
        assert_eq!(gaps.take(), Summary::default());
    }

    #[test]
    fn a_sender_without_a_period_counts_nothing() {
        let summary = run(Duration::ZERO, &[1_000, 2_000]);
        assert_eq!(summary, Summary::default());
    }
}
