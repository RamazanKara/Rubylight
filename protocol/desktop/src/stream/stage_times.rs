//! Per-frame stage timers for measurement: each stage adds up over a frame, `commit` keeps the
//! frame's totals, and the summary at the end of a session gives p50 and p95 per stage.
//! Percentiles use the same nearest-rank rule as `scripts/timing_summary.py`. The totals go
//! into a fixed-size histogram, so a stream of any length keeps a constant amount of memory.

use std::time::Instant;

/// Histogram resolution in microseconds, and its range (longer totals count in the last bucket).
const BUCKET_US: u32 = 10;
const BUCKETS: usize = 10_000;

pub struct Stages<const N: usize> {
    names: [&'static str; N],
    frame: [u32; N],
    counts: [Vec<u32>; N],
    frames: usize,
}

impl<const N: usize> Stages<N> {
    pub fn new(names: [&'static str; N]) -> Self {
        Self { names, frame: [0; N], counts: std::array::from_fn(|_| vec![0; BUCKETS]), frames: 0 }
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
        for (counts, total) in self.counts.iter_mut().zip(self.frame) {
            counts[((total / BUCKET_US) as usize).min(BUCKETS - 1)] += 1;
        }
        self.frames += 1;
        self.frame = [0; N];
    }

    pub fn frames(&self) -> usize {
        self.frames
    }

    /// `name p50/p95` in milliseconds per stage, or None before the first frame.
    pub fn summary(&self) -> Option<String> {
        self.summary_of(N)
    }

    /// The summary of the first `shown` stages, for a session where the later ones never ran.
    pub fn summary_of(&self, shown: usize) -> Option<String> {
        if self.frames() == 0 { return None; }
        let parts: Vec<String> = self.names.iter().zip(&self.counts).take(shown).map(|(name, counts)| {
            let at = |p| percentile(counts, self.frames, p) as f64 / 1000.0;
            format!("{name} {:.2}/{:.2}", at(50), at(95))
        }).collect();
        Some(format!("(ms p50/p95 over {} frames): {}", self.frames(), parts.join(", ")))
    }
}

/// Nearest rank over `n` samples in a histogram: element `min(n - 1, round(p / 100 * (n - 1)))`
/// of the sorted samples, as the start of its bucket in microseconds.
pub fn percentile(counts: &[u32], n: usize, p: usize) -> u32 {
    if n == 0 { return 0; }
    let rank = (n - 1).min((p as f64 / 100.0 * (n - 1) as f64).round() as usize);
    let mut seen = 0;
    for (bucket, count) in counts.iter().enumerate() {
        seen += *count as usize;
        if seen > rank { return bucket as u32 * BUCKET_US; }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_use_the_nearest_rank_rule_of_the_summary_script() {
        // 101 samples of 10..=1010 us, one per bucket.
        let mut counts = vec![0; BUCKETS];
        counts[1..=101].fill(1);
        let at = |p| percentile(&counts, 101, p);
        assert_eq!((at(50), at(95), at(99)), (510, 960, 1000));
        assert_eq!(percentile(&counts, 0, 50), 0);
    }

    #[test]
    fn a_summary_can_leave_out_the_last_stages() {
        let mut stages = Stages::new(["a", "b", "c"]);
        stages.frame = [1500, 0, 0];
        stages.commit();
        assert_eq!(stages.summary_of(2).unwrap(), "(ms p50/p95 over 1 frames): a 1.50/1.50, b 0.00/0.00");
        assert!(stages.summary().unwrap().ends_with("c 0.00/0.00"));
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
        // A frame longer than the range counts in the last bucket; memory does not grow.
        stages.frame = [u32::MAX, 0];
        stages.commit();
        assert_eq!(stages.counts[0].len(), BUCKETS);
        assert_eq!(stages.frames(), 3);
        stages.add(0, Instant::now());
        stages.reset();
        assert_eq!(stages.frame, [0, 0]);
    }
}
