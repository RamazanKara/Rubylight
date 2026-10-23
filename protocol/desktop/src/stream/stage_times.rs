//! Per-frame stage timers for measurement: each stage adds up over a frame, `commit` keeps the
//! frame's totals, and the summary at the end of a session gives p50 and p95 per stage.
//! Percentiles use the same nearest-rank rule as `scripts/timing_summary.py`.

use std::time::Instant;

pub struct Stages<const N: usize> {
    names: [&'static str; N],
    frame: [u32; N],
    samples: [Vec<u32>; N],
}

impl<const N: usize> Stages<N> {
    pub fn new(names: [&'static str; N]) -> Self {
        Self { names, frame: [0; N], samples: std::array::from_fn(|_| Vec::new()) }
    }

    /// Starts a new frame's totals.
    pub fn reset(&mut self) {
        self.frame = [0; N];
    }

    /// Adds the time since `since` to `stage` of the frame in progress.
    pub fn add(&mut self, stage: usize, since: Instant) {
        self.frame[stage] = self.frame[stage].saturating_add(since.elapsed().as_micros().min(u128::from(u32::MAX)) as u32);
    }

    /// Keeps the frame's totals.
    pub fn commit(&mut self) {
        for (samples, total) in self.samples.iter_mut().zip(self.frame) { samples.push(total); }
        self.frame = [0; N];
    }

    pub fn frames(&self) -> usize {
        self.samples[0].len()
    }

    /// `name p50/p95` in milliseconds per stage, or None before the first frame.
    pub fn summary(&self) -> Option<String> {
        if self.frames() == 0 { return None; }
        let parts: Vec<String> = self.names.iter().zip(&self.samples).map(|(name, samples)| {
            let mut sorted = samples.clone();
            sorted.sort_unstable();
            format!("{name} {:.2}/{:.2}", percentile(&sorted, 50) as f64 / 1000.0, percentile(&sorted, 95) as f64 / 1000.0)
        }).collect();
        Some(format!("(ms p50/p95 over {} frames): {}", self.frames(), parts.join(", ")))
    }
}

/// Nearest rank on a sorted slice: `sorted[min(n - 1, round(p / 100 * (n - 1)))]`.
pub fn percentile(sorted: &[u32], p: usize) -> u32 {
    if sorted.is_empty() { return 0; }
    let n = sorted.len();
    sorted[(n - 1).min((p as f64 / 100.0 * (n - 1) as f64).round() as usize)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_use_the_nearest_rank_rule_of_the_summary_script() {
        let sorted: Vec<u32> = (1..=101).collect();
        assert_eq!((percentile(&sorted, 50), percentile(&sorted, 95), percentile(&sorted, 99)), (51, 96, 100));
        assert_eq!(percentile(&[7], 95), 7);
        assert_eq!(percentile(&[], 50), 0);
    }

    #[test]
    fn frames_add_up_and_the_summary_reports_each_stage() {
        let mut stages = Stages::new(["a", "b"]);
        assert!(stages.summary().is_none());
        stages.frame = [1500, 0];
        stages.commit();
        stages.frame = [2500, 40];
        stages.commit();
        assert_eq!(stages.frames(), 2);
        assert_eq!(stages.summary().unwrap(), "(ms p50/p95 over 2 frames): a 2.50/2.50, b 0.04/0.04");
        stages.add(0, Instant::now());
        stages.reset();
        assert_eq!(stages.frame, [0, 0]);
    }
}
