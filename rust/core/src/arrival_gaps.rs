//! How evenly something arrives, for the log. A drag that reaches the host in
//! 40 ms steps and a Windows that draws in 40 ms steps look the same in the
//! stream's frame rates; the arrivals of its pointer moves and of its
//! microphone, which leaves the client on another thread, tell them apart.

use std::time::{Duration, Instant};

/// One log line's span.
pub const PERIOD: Duration = Duration::from_secs(5);
/// A gap this long is a pause, such as a lifted finger, and not a late arrival.
pub const PAUSE: Duration = Duration::from_millis(250);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub arrivals: u32,
    pub gaps_25_50: u32,
    pub gaps_50_plus: u32,
    pub pauses: u32,
    /// The longest gap that was not a pause.
    pub max_gap_ms: u32,
}

#[derive(Debug, Default)]
pub struct ArrivalGaps {
    started: Option<Instant>,
    last: Option<Instant>,
    window: Summary,
}

impl ArrivalGaps {
    /// Counts an arrival. Once a [`PERIOD`] has passed it returns what the
    /// period saw, so nothing is logged while nothing arrives.
    pub fn note(&mut self, now: Instant) -> Option<Summary> {
        if let Some(last) = self.last.replace(now) {
            let gap = now.saturating_duration_since(last);
            if gap >= PAUSE {
                self.window.pauses += 1;
            } else {
                let ms = gap.as_millis() as u32;
                match ms {
                    25..=49 => self.window.gaps_25_50 += 1,
                    50.. => self.window.gaps_50_plus += 1,
                    _ => {}
                }
                self.window.max_gap_ms = self.window.max_gap_ms.max(ms);
            }
        }
        self.window.arrivals += 1;
        let started = *self.started.get_or_insert(now);
        (now.saturating_duration_since(started) >= PERIOD).then(|| {
            self.started = Some(now);
            std::mem::take(&mut self.window)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(gaps_ms: &[u64]) -> (ArrivalGaps, Instant, Option<Summary>) {
        let mut gaps = ArrivalGaps::default();
        let mut now = Instant::now();
        let mut last = gaps.note(now);
        for gap in gaps_ms {
            now += Duration::from_millis(*gap);
            last = gaps.note(now);
        }
        (gaps, now, last)
    }

    #[test]
    fn an_even_drag_has_no_long_gaps() {
        let (mut gaps, now, _) = run(&[8; 100]);
        let summary = gaps.note(now + PERIOD).unwrap();
        // The closing gap is a pause; the drag's own never counted.
        assert_eq!(summary.arrivals, 102);
        assert_eq!(
            (summary.gaps_25_50, summary.gaps_50_plus, summary.pauses),
            (0, 0, 1)
        );
        assert_eq!(summary.max_gap_ms, 8);
    }

    #[test]
    fn steps_of_40_ms_and_a_stall_are_counted_apart() {
        let (mut gaps, now, _) = run(&[40, 40, 40, 60, 8]);
        let summary = gaps.note(now + PERIOD).unwrap();
        assert_eq!((summary.gaps_25_50, summary.gaps_50_plus), (3, 1));
        assert_eq!(summary.max_gap_ms, 60);
    }

    #[test]
    fn a_lifted_finger_is_a_pause_and_not_the_longest_gap() {
        let (mut gaps, now, _) = run(&[8, 249, 250, 8]);
        let summary = gaps.note(now + PERIOD).unwrap();
        assert_eq!((summary.gaps_50_plus, summary.pauses), (1, 2));
        assert_eq!(summary.max_gap_ms, 249);
    }

    #[test]
    fn each_period_reports_once_and_starts_empty() {
        let mut gaps = ArrivalGaps::default();
        let start = Instant::now();
        assert_eq!(gaps.note(start), None);
        assert_eq!(gaps.note(start + PERIOD - Duration::from_millis(1)), None);
        assert_eq!(gaps.note(start + PERIOD).unwrap().arrivals, 3);
        assert_eq!(gaps.note(start + PERIOD + Duration::from_millis(8)), None);
        let next = gaps.note(start + PERIOD * 2).unwrap();
        assert_eq!(next.arrivals, 2);
    }
}
