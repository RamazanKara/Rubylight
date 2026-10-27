//! The once-a-second stats tick every policy reads.

/// One stats tick, passed by reference to every controller and the stream doctor. A measurement
/// is `None` when unknown, so [`Stats::default`] is a tick at time 0 with nothing measured: no
/// controller reads it as healthy.
///
/// Each policy also reads a negative or NaN measurement as unknown, as its Java original does,
/// but callers pass `None` instead. The trace adapters alone pass Java's -1 and NaN through.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    /// Monotonic time of the tick.
    pub now_ms: u64,
    /// Frames arrived in the last two seconds, so the counters below are current.
    pub recent_video: bool,
    /// The native connection-quality warning.
    pub poor_connection: bool,
    /// The stream's frame rate, 0 when unknown. Only the stream doctor reads it; the controllers
    /// keep the rate they were given.
    pub fps: u32,
    /// Frames lost after FEC, in percent.
    pub loss_percent: Option<f32>,
    pub decode_ms: Option<f32>,
    pub jitter_ms: Option<f32>,
    /// Network queueing delay above its recent minimum.
    pub queue_ms: Option<f32>,
    pub host_p95_ms: Option<f32>,
    pub display_p95_ms: Option<f32>,
    pub received_fps: Option<f32>,
    pub released_fps: Option<f32>,
    pub shown_fps: Option<f32>,
    /// True when `host_p95_ms` runs to the last packet sent, as the host's timing reports measure it.
    pub host_includes_send: bool,
    /// The thermal monitor's level: 0 normal (or no thermal reading), 1 warm, 2 and up hot.
    pub thermal_level: u32,
}

/// A measured value: at least 0, which also drops NaN.
pub(crate) fn known(value: Option<f32>) -> Option<f32> {
    value.filter(|v| *v >= 0.0)
}

/// The 95th percentile of a stage over the last two stats ticks, for [`Stats::host_p95_ms`] and
/// [`Stats::display_p95_ms`]. Android's `FrameLatencyStats` summarizes the last two seconds the
/// same way: the sample at rank ceil(0.95 n) of the sorted values. Values go in as microseconds.
#[derive(Clone, Debug, Default)]
pub struct RecentP95 {
    previous: Vec<u32>,
    current: Vec<u32>,
}

impl RecentP95 {
    /// Samples kept per tick: a second of a 1000 fps stream, plus slack.
    pub const MAX_SAMPLES: usize = 1024;

    /// Records one sample of the current tick; samples past [`Self::MAX_SAMPLES`] are dropped.
    pub fn push(&mut self, us: u64) {
        if self.current.len() < Self::MAX_SAMPLES {
            self.current.push(u32::try_from(us).unwrap_or(u32::MAX));
        }
    }

    /// Whether this tick or the previous one has a sample.
    pub fn has_samples(&self) -> bool {
        !self.previous.is_empty() || !self.current.is_empty()
    }

    /// The p95 of this tick's and the previous tick's samples in ms, `None` without any, then
    /// starts the next tick.
    pub fn tick(&mut self) -> Option<f32> {
        let mut all: Vec<u32> = self.previous.iter().chain(&self.current).copied().collect();
        std::mem::swap(&mut self.previous, &mut self.current);
        self.current.clear();
        if all.is_empty() {
            return None;
        }
        all.sort_unstable();
        let rank = (all.len() * 95).div_ceil(100);
        Some(all[rank - 1] as f32 / 1000.0)
    }

    /// Forgets every sample, as when the stream changes mode.
    pub fn clear(&mut self) {
        self.previous.clear();
        self.current.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p95_is_the_nearest_rank_over_two_ticks() {
        let mut p95 = RecentP95::default();
        assert_eq!(p95.tick(), None);
        assert!(!p95.has_samples());
        // 1..=10 ms in the first tick, 11..=20 ms in the second: rank ceil(19) of 20 is 19 ms.
        for ms in 1..=10 {
            p95.push(ms * 1000);
        }
        assert_eq!(p95.tick(), Some(10.0));
        for ms in (11..=20).rev() {
            p95.push(ms * 1000);
        }
        assert_eq!(p95.tick(), Some(19.0));
        // The first tick has aged out; the second still counts once.
        assert!(p95.has_samples());
        assert_eq!(p95.tick(), Some(20.0));
        assert!(!p95.has_samples());
        assert_eq!(p95.tick(), None);
    }

    #[test]
    fn one_sample_is_its_own_p95_and_a_tick_is_bounded() {
        let mut p95 = RecentP95::default();
        p95.push(2_500);
        assert_eq!(p95.tick(), Some(2.5));
        p95.clear();
        assert!(!p95.has_samples());
        for _ in 0..RecentP95::MAX_SAMPLES + 10 {
            p95.push(1_000);
        }
        p95.push(u64::MAX);
        assert_eq!(p95.current.len(), RecentP95::MAX_SAMPLES);
        assert_eq!(p95.tick(), Some(1.0));
    }
}
