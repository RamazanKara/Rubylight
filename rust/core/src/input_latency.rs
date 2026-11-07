//! How long keyboard, mouse, touch and pen input waits in the host: from the
//! control worker reading a pass's first datagram to its input being injected,
//! for the 5 s `stream timings` line.

use std::time::Duration;

/// Samples one window keeps for its percentiles; a 1000 Hz mouse fills 5000 in
/// 5 s. Later passes still count and still set the maximum.
const SAMPLES: usize = 8192;

#[derive(Debug, Default)]
pub struct InputLatency {
    /// Microseconds, one per pass that injected input.
    samples: Vec<u32>,
    passes: u64,
    max_us: u32,
}

/// One window: p50/p99/max in milliseconds, and the passes it saw.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Summary {
    pub passes: u64,
    pub p50_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
}

impl std::fmt::Display for Summary {
    /// As the stage fields of the same line: `p50/p99/max`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:.2}/{:.2}/{:.2}",
            self.p50_ms, self.p99_ms, self.max_ms
        )
    }
}

impl InputLatency {
    /// A pass that injected input `waited` after its first datagram was read.
    pub fn record(&mut self, waited: Duration) {
        let us = u32::try_from(waited.as_micros()).unwrap_or(u32::MAX);
        self.passes += 1;
        self.max_us = self.max_us.max(us);
        if self.samples.len() < SAMPLES {
            self.samples.push(us);
        }
    }

    /// The window since the last call; the next starts empty.
    pub fn take(&mut self) -> Summary {
        let mut samples = std::mem::take(&mut self.samples);
        samples.sort_unstable();
        let at = |percent: usize| {
            samples
                .get(samples.len().saturating_sub(1) * percent / 100)
                .map_or(0., |us| f64::from(*us) / 1000.)
        };
        let summary = Summary {
            passes: self.passes,
            p50_ms: at(50),
            p99_ms: at(99),
            max_ms: f64::from(self.max_us) / 1000.,
        };
        samples.clear();
        self.samples = samples;
        self.passes = 0;
        self.max_us = 0;
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_reports_percentiles_and_the_maximum_then_starts_empty() {
        let mut latency = InputLatency::default();
        assert_eq!(latency.take(), Summary::default());
        assert_eq!(latency.take().to_string(), "0.00/0.00/0.00");
        for us in (1..=100).rev() {
            latency.record(Duration::from_micros(us * 10));
        }
        let summary = latency.take();
        assert_eq!(summary.passes, 100);
        assert_eq!(summary.p50_ms, 0.5);
        assert_eq!(summary.p99_ms, 0.99);
        assert_eq!(summary.max_ms, 1.0);
        assert_eq!(summary.to_string(), "0.50/0.99/1.00");
        assert_eq!(latency.take(), Summary::default());
    }

    #[test]
    fn passes_past_the_sample_cap_still_count_and_set_the_maximum() {
        let mut latency = InputLatency::default();
        for _ in 0..SAMPLES {
            latency.record(Duration::from_micros(100));
        }
        latency.record(Duration::from_millis(21));
        let summary = latency.take();
        assert_eq!(summary.passes, SAMPLES as u64 + 1);
        assert_eq!(summary.p99_ms, 0.1);
        assert_eq!(summary.max_ms, 21.0);
    }
}
