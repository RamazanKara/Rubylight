//! Capture publication and idle-wait policy, independent of Windows/GPU APIs.
use std::time::Duration;

/// Preserve short active polls and pending GPU handoffs. After 100 ms without
/// a frame, notifications can interrupt a longer wait; the 10 ms bound still
/// services helper commands, stop and device checks.
pub fn notification_wait_interval(idle: Duration, pending: bool, poll: Duration) -> Duration {
    if pending || idle < Duration::from_millis(100) {
        poll
    } else {
        poll.max(Duration::from_millis(10))
    }
}
#[derive(Default)]
pub struct Intervals {
    last: Option<i64>,
    samples: [i64; 16],
    count: usize,
    next: usize,
}

/// Predict a nearby publication only after a stable source cadence is known.
/// A slow, irregular, restarted or already fresh source keeps the normal slot.
#[derive(Default)]
pub struct Freshness {
    last: Option<u64>,
    intervals: [u64; 16],
    delays: [u64; 16],
    count: usize,
    next: usize,
    median: Option<u64>,
    mean: Option<u64>,
    polling_window: Option<(u64, u64)>,
    /// The latest picture's first capture, and the intervals between the
    /// first captures of recent pictures: see `picture_interval`.
    picture: Option<u64>,
    pictures: [u64; 16],
    picture_count: usize,
    picture_next: usize,
    picture_median: Option<u64>,
    picture_late: Option<u64>,
    /// The latest capture that changed the focus window, the first capture
    /// of its latest picture and the interval before that picture, and
    /// whether the latest capture only extends the focus window's latest
    /// picture: see `observe_focus`.
    focus_at: Option<u64>,
    focus_start: Option<u64>,
    focus_interval: Option<u64>,
    extends: bool,
}
impl Freshness {
    /// A capture this soon after the previous one belongs to the same picture.
    const PICTURE_GAP: u64 = 2_000_000;
    /// The focus window animates while its pictures come at least this often;
    /// other windows' updates then extend its latest picture for twice its
    /// latest interval, and at least this long.
    const FOCUS_ANIMATING: u64 = 50_000_000;
    /// The median interval between pictures, once a few are known: a capture
    /// less than 2 ms after the previous one counts as part of its picture.
    /// WGC on a 240 Hz VRR display delivers each frame of a 120 Hz game with
    /// a second composition 0.1-1 ms later (and in some sessions another 3-4 ms
    /// later), so the capture intervals' median read 7 ms or as little as 3 ms
    /// for a game presenting every 8.3 ms. A later composition counts as a
    /// picture of its own: when many pictures have one, the median falls
    /// short of the game's interval and `late_picture_interval` still covers
    /// it. A 1000 Hz display is one long picture and has no picture interval.
    pub fn picture_interval(&self) -> Option<u64> {
        self.picture_median
    }
    /// The picture interval nearly all recent pictures arrived within: the
    /// second longest of the last 16. A game's frame times vary; a 120 Hz
    /// game's frames under VRR came 7.3 to 9.3 ms apart.
    pub fn late_picture_interval(&self) -> Option<u64> {
        self.picture_late
    }
    /// When the latest picture's first capture was presented.
    pub fn picture_start(&self) -> Option<u64> {
        self.picture
    }
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
    /// Use a short polling window around one predicted publication. Once that
    /// window passes, a source that becomes static returns to the normal wait.
    pub fn poll_wait(&self, age: u64, normal: u64) -> u64 {
        let Some((early, late)) = self.polling_window else {
            return normal;
        };
        if age < early {
            normal.min(early - age)
        } else if age <= late {
            normal.min(100_000)
        } else {
            normal
        }
    }
    /// Whether the latest capture changed only pixels outside the focus window
    /// while that window kept changing (`observe_focus`): it extends the
    /// focus window's latest picture instead of starting one.
    pub fn extends_picture(&self) -> bool {
        self.extends
    }
    pub fn observe(&mut self, captured: u64, delay: u64) {
        self.observe_focus(captured, delay, None);
    }
    /// Observe a capture and whether it changed the focus window (the
    /// foreground window): `None` when the capture cannot tell. A 60 Hz game
    /// window on the 240 Hz VRR desktop next to a window animating at the
    /// display rate (the Claude desktop app presented 245 times a second)
    /// arrived as five captures per game frame, each a picture of its own:
    /// claims at the stream rate drifted through the game's frames and took
    /// some of them twice and skipped as many. A capture that changed only
    /// other windows extends the focus window's picture while that window
    /// animates (`FOCUS_ANIMATING`); a still or slowly changing focus window
    /// (typing, a caret) leaves every update a picture of its own, as without
    /// the distinction.
    pub fn observe_focus(&mut self, captured: u64, delay: u64, focus: Option<bool>) {
        let mut same_picture = false;
        if let Some(previous) = self.last {
            let interval = captured.saturating_sub(previous);
            if captured > previous && interval <= 500_000_000 {
                self.intervals[self.next] = interval;
                self.delays[self.next] = delay;
                self.next = (self.next + 1) % self.intervals.len();
                self.count = (self.count + 1).min(self.intervals.len());
                same_picture = interval < Self::PICTURE_GAP;
            } else {
                self.count = 0;
                self.next = 0;
                self.picture = None;
                self.focus_at = None;
                self.focus_start = None;
                self.focus_interval = None;
            }
        }
        self.extends = false;
        let animating = match (self.focus_at, self.focus_interval) {
            (Some(at), Some(interval)) => {
                interval <= Self::FOCUS_ANIMATING
                    && captured.saturating_sub(at) <= (2 * interval).max(Self::FOCUS_ANIMATING)
            }
            _ => false,
        };
        match focus {
            Some(true) => {
                // A focus change right after another window's update is new;
                // one soon after the focus window's last change is part of it.
                same_picture = self
                    .focus_at
                    .is_some_and(|at| captured.saturating_sub(at) < Self::PICTURE_GAP);
                if !same_picture {
                    self.focus_interval =
                        self.focus_start.map(|start| captured.saturating_sub(start));
                    self.focus_start = Some(captured);
                }
                self.focus_at = Some(captured);
            }
            Some(false) if self.picture.is_some() && animating => {
                same_picture = true;
                self.extends = true;
            }
            _ => {}
        }
        self.last = Some(captured);
        if !same_picture {
            match self.picture {
                Some(start) if captured - start <= 500_000_000 => {
                    self.pictures[self.picture_next] = captured - start;
                    self.picture_next = (self.picture_next + 1) % self.pictures.len();
                    self.picture_count = (self.picture_count + 1).min(self.pictures.len());
                }
                _ => {
                    self.picture_count = 0;
                    self.picture_next = 0;
                }
            }
            self.picture = Some(captured);
            self.picture_median = None;
            self.picture_late = None;
            if self.picture_count >= 4 {
                let mut pictures = self.pictures;
                pictures[..self.picture_count].sort_unstable();
                self.picture_median = Some(pictures[self.picture_count / 2]);
                self.picture_late = Some(pictures[self.picture_count * 7 / 8]);
            }
        }
        // Polling reads the same history many times between frames. Recompute
        // its statistics only when that history changes, outside the wait loop.
        self.median = None;
        self.mean = None;
        self.polling_window = None;
        if self.count >= 4 {
            self.mean = Some(self.intervals[..self.count].iter().sum::<u64>() / self.count as u64);
            let mut intervals = self.intervals;
            intervals[..self.count].sort_unstable();
            let source = intervals[self.count / 2];
            self.median = Some(source);
            if self.count >= 8
                && source >= 3_000_000
                && intervals[0] >= source.saturating_mul(7) / 8
                && intervals[self.count - 1] <= source.saturating_mul(9) / 8
            {
                let mut delays = self.delays;
                delays[..self.count].sort_unstable();
                let expected = source.saturating_add(delays[(self.count - 1) / 10]);
                self.polling_window = Some((
                    expected.saturating_sub(1_000_000),
                    expected.saturating_add(500_000),
                ));
            }
        }
    }
}
impl Intervals {
    pub fn observe(&mut self, now: i64) -> i64 {
        if let Some(previous) = self.last {
            let period = now - previous;
            if (1..=500_000_000).contains(&period) {
                self.samples[self.next] = period;
                self.next = (self.next + 1) % self.samples.len();
                self.count = (self.count + 1).min(self.samples.len());
            }
        }
        self.last = Some(now);
        if self.count < 4 {
            return 0;
        }
        let mut sorted = self.samples;
        sorted[..self.count].sort_unstable();
        sorted[self.count / 2]
    }
}
/// Defer a copy only when a newer composition can precede the same encode slot.
/// The deadline preserves a lone update if the source becomes static.
pub fn publication_deadline(
    period: i64,
    now: i64,
    composition: i64,
    last_publish: i64,
) -> Option<i64> {
    if period <= 0 || composition <= 0 || composition.saturating_mul(5) > period.saturating_mul(3) {
        return None;
    }
    let next = if now < 0 {
        0
    } else {
        (now / period + 1).saturating_mul(period)
    };
    let previous = next - period;
    if now - previous < 6_000_000 && last_publish <= previous - period {
        return None;
    }
    if now.saturating_add(composition).saturating_add(200_000) > next {
        return None;
    }
    let deadline = next - 250_000;
    (deadline > now).then_some(deadline)
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
    fn another_window_extends_the_focus_picture_only_while_the_focus_changes() {
        let ms = 1_000_000;
        let mut cadence = Freshness::default();
        // A focus window changing every 150 ms (typing) does not animate:
        // another window's updates stay pictures of their own.
        for at in [100, 250] {
            cadence.observe_focus(at * ms, ms, Some(true));
            cadence.observe_focus((at + 4) * ms, ms, Some(false));
            assert!(!cadence.extends_picture());
            assert_eq!(cadence.picture_start(), Some((at + 4) * ms));
        }
        // A game at 60 Hz animates.
        cadence.observe_focus(300 * ms, ms, Some(true));
        cadence.observe_focus(316 * ms, ms, Some(true));
        let game = cadence.picture_start();
        assert_eq!(game, Some(316 * ms));
        // The duplicate composition and another window's updates extend it.
        cadence.observe_focus(317 * ms, ms, Some(true));
        assert!(!cadence.extends_picture());
        for at in [321, 325, 329] {
            cadence.observe_focus(at * ms, ms, Some(false));
            assert!(cadence.extends_picture());
            assert_eq!(cadence.picture_start(), game);
        }
        // A focus change soon after another window's update is a new picture.
        cadence.observe_focus(330 * ms, ms, Some(true));
        assert!(!cadence.extends_picture());
        assert_eq!(cadence.picture_start(), Some(330 * ms));
        cadence.observe_focus(370 * ms, ms, Some(false));
        assert!(cadence.extends_picture());
        // Once the focus window is still, every update is a picture again.
        cadence.observe_focus(381 * ms, ms, Some(false));
        assert!(!cadence.extends_picture());
        assert_eq!(cadence.picture_start(), Some(381 * ms));
        // Without dirty regions nothing changes.
        cadence.observe_focus(385 * ms, ms, None);
        assert!(!cadence.extends_picture());
        assert_eq!(cadence.picture_start(), Some(385 * ms));
    }
    fn learned(interval: u64) -> Freshness {
        let mut cadence = Freshness::default();
        for frame in 0..=16 {
            cadence.observe(frame * interval, 500_000);
        }
        cadence
    }
    #[test]
    fn cached_statistics_match_the_original_policy_across_cadence_changes_and_resets() {
        fn reference(cadence: &Freshness, age: u64, normal: u64) -> u64 {
            if cadence.count < 8 {
                return normal;
            }
            let mut intervals = cadence.intervals;
            intervals[..cadence.count].sort_unstable();
            let source = intervals[cadence.count / 2];
            if source < 3_000_000
                || intervals[0] < source.saturating_mul(7) / 8
                || intervals[cadence.count - 1] > source.saturating_mul(9) / 8
            {
                return normal;
            }
            let mut delays = cadence.delays;
            delays[..cadence.count].sort_unstable();
            let expected = source.saturating_add(delays[(cadence.count - 1) / 10]);
            let early = expected.saturating_sub(1_000_000);
            if age < early {
                normal.min(early - age)
            } else if age <= expected.saturating_add(500_000) {
                normal.min(100_000)
            } else {
                normal
            }
        }
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
            cadence.observe(captured, (frame % 17) * 100_000);
            let mut intervals = cadence.intervals;
            intervals[..cadence.count].sort_unstable();
            assert_eq!(
                cadence.median_interval(),
                (cadence.count >= 4).then(|| intervals[cadence.count / 2])
            );
            for age in [0, period - 1_000_000, period, period + 2_000_000, u64::MAX] {
                for normal in [50_000, 1_000_000, 10_000_000] {
                    assert_eq!(
                        cadence.poll_wait(age, normal),
                        reference(&cadence, age, normal)
                    );
                }
            }
        }
    }
    #[test]
    fn a_second_composition_of_a_picture_does_not_shorten_the_picture_interval() {
        // WGC on a 240 Hz VRR display: each frame of a 120 Hz game arrives
        // with another composition 0.1 or 1 ms later; in some sessions every
        // fourth frame has a third 3.5 ms later.
        for third in [false, true] {
            let mut cadence = Freshness::default();
            for frame in 0..200u64 {
                let jitter = (frame.wrapping_mul(2_654_435_761) >> 7) % 600_000;
                let mut start = 1_000_000_000 + frame * 8_333_333 + jitter;
                cadence.observe(start, 100_000);
                let second = start + [100_000, 1_000_000][(frame * 5 % 3).min(1) as usize];
                cadence.observe(second, 100_000);
                if third && frame % 4 == 1 {
                    start += 3_500_000;
                    cadence.observe(start, 100_000);
                }
                if frame >= 20 {
                    let picture = cadence.picture_interval().unwrap();
                    assert!(
                        (7_833_333..=8_833_333).contains(&picture),
                        "third={third} frame {frame}: {picture}"
                    );
                    assert_eq!(cadence.picture_start(), Some(start));
                    assert!(cadence.median_interval().unwrap() < 7_500_000);
                }
            }
        }
        // The 1000 Hz virtual display's own refresh is no picture cadence.
        let mut display = Freshness::default();
        for tick in 0..100 {
            display.observe(tick * 1_000_000, 100_000);
        }
        assert_eq!(display.picture_interval(), None);
        assert_eq!(display.median_interval(), Some(1_000_000));
        // A source without paired compositions has the same median either way.
        let mut cadence = Freshness::default();
        for frame in 0..100u64 {
            cadence.observe(frame * 16_666_667 + frame * 7_919 % 900_000, 100_000);
            assert_eq!(cadence.picture_interval(), cadence.median_interval());
        }
        // A restart starts the picture history over.
        cadence.observe(1, 100_000);
        assert_eq!(cadence.picture_interval(), None);
        assert_eq!(cadence.picture_start(), Some(1));
    }
    #[test]
    fn predictive_polling_is_bounded_and_static_or_irregular_sources_use_normal_waits() {
        let mut cadence = learned(8_333_333);
        assert_eq!(cadence.poll_wait(2_000_000, 1_000_000), 1_000_000);
        assert_eq!(cadence.poll_wait(7_500_000, 1_000_000), 333_333);
        assert_eq!(cadence.poll_wait(8_000_000, 1_000_000), 100_000);
        assert_eq!(cadence.poll_wait(10_000_000, 1_000_000), 1_000_000);
        assert_eq!(cadence.poll_wait(1_000_000_000, 1_000_000), 1_000_000);
        cadence.observe(16 * 8_333_333 + 20_000_000, 500_000);
        assert_eq!(cadence.poll_wait(8_000_000, 1_000_000), 1_000_000);
        assert_eq!(
            Freshness::default().poll_wait(8_000_000, 1_000_000),
            1_000_000
        );
        assert_eq!(
            learned(1_000_000).poll_wait(1_000_000, 1_000_000),
            1_000_000
        );
    }
    #[test]
    fn publication_preserves_static_updates_and_a_waiting_encode_slot() {
        assert_eq!(
            publication_deadline(20_000_000, 3_000_000, 8_333_333, 0),
            Some(19_750_000)
        );
        assert_eq!(
            publication_deadline(20_000_000, 19_000_000, 8_333_333, 0),
            None
        );
        assert_eq!(
            publication_deadline(20_000_000, 21_000_000, 8_333_333, 0),
            None
        );
        assert_eq!(
            publication_deadline(20_000_000, 3_000_000, 12_001_000, 0),
            None
        );
        assert_eq!(publication_deadline(0, 3, 1, 0), None);
        let mut history = Intervals::default();
        for time in [0, 8_000_000, 16_000_000, 24_000_000] {
            assert_eq!(history.observe(time), 0);
        }
        assert_eq!(history.observe(32_000_000), 8_000_000);
        assert_eq!(history.observe(432_000_000), 8_000_000);
    }
}
