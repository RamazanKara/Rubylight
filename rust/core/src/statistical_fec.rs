//! Statistical video FEC for a client that asks for it (`rlFecPercentage=auto`):
//! the Reed-Solomon share of each frame follows the client's once-a-second
//! loss reports (0x5539, `rubylight_protocol::fec_report`).
//!
//! Ported from Foundation Sunshine's `src/streaming/fec.cpp` and
//! `docs/dynamic-fec.zh-CN.md` (AlkaidLab/foundation-sunshine 880f1ef, GPL-3):
//!
//! - Ten one-second buckets of reported shards; the estimate is the worst
//!   second's share of missing shards. No report is not the same as no loss.
//! - Each frame gets the smallest percentage whose blocks all keep the
//!   binomial probability of losing more shards than they have parity under
//!   [`FRAME_FAILURE_TARGET`] (split evenly across the frame's blocks), up to
//!   the host's `fec_max_percentage`. When none reaches it, the largest
//!   percentage the frame can still carry within the cap.
//! - A report gap (a skipped sequence number or [`FEEDBACK_TIMEOUT`] without
//!   one) forgets the history. Without fresh reports the host's own
//!   percentage applies, and zero loss after a gap keeps it for a whole
//!   window before protection drops to zero.
//!
//! The binomial tails are computed when a report changes the estimate (on the
//! control thread, once a second at most); a frame only walks the integer
//! layout and compares its blocks' parity with that table.
use std::time::{Duration, Instant};

pub use rubylight_protocol::fec_report::{FEC_REPORT_MESSAGE_TYPE, FecReport};

use crate::packet::conventional_fec;
use crate::pyrowave::fec_parameters;

/// The modelled chance that some block of a frame cannot be rebuilt.
pub const FRAME_FAILURE_TARGET: f64 = 0.001;
/// History the estimate covers, in one-second buckets.
pub const WINDOW_SECONDS: usize = 10;
pub const WINDOW: Duration = Duration::from_secs(WINDOW_SECONDS as u64);
/// Reports older than this are stale: the host's percentage applies again.
pub const FEEDBACK_TIMEOUT: Duration = Duration::from_secs(3);
/// More shards than any real second carries is a broken report.
pub const MAX_REPORTED_SHARDS: u64 = 1_000_000;
/// Reed-Solomon over GF(256): data and parity of one block.
const MAX_BLOCK_SHARDS: usize = 255;
/// Moonlight takes at most four blocks a frame.
const MAX_BLOCKS: usize = 4;
const UNREACHABLE: u8 = u8::MAX;

#[derive(Clone, Copy, Debug, Default)]
struct Bucket {
    second: Option<u64>,
    sent: u64,
    missing: u64,
}

/// Why a report was not used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejected {
    /// The sequence number repeats or goes back.
    Stale,
    /// More shards than [`MAX_REPORTED_SHARDS`].
    Implausible,
}

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Stale => "repeated or older sequence number",
            Self::Implausible => "implausible shard count",
        })
    }
}

#[derive(Debug)]
pub struct StatisticalFec {
    maximum: u8,
    fallback: u8,
    minimum_parity: usize,
    origin: Instant,
    buckets: [Bucket; WINDOW_SECONDS],
    /// Sequence number and arrival of the last accepted report.
    last: Option<(u32, Instant)>,
    /// Set by a gap in the reports: zero loss counts only a full window later.
    resumed: Option<Instant>,
    loss: Option<f64>,
    /// Parity a block needs, by block count and data shards; [`UNREACHABLE`]
    /// when it cannot fit. Empty until the estimate shows loss.
    required: Vec<[u8; MAX_BLOCK_SHARDS + 1]>,
    logged: Option<usize>,
    logged_at: Option<Instant>,
}

impl StatisticalFec {
    /// `maximum` is `fec_max_percentage`, `fallback` the host's own
    /// percentage and `minimum_parity` the client's `minRequiredFecPackets`.
    pub fn new(maximum: u8, fallback: u8, minimum_parity: usize, now: Instant) -> Self {
        Self {
            maximum,
            fallback,
            minimum_parity,
            origin: now,
            buckets: [Bucket::default(); WINDOW_SECONDS],
            last: None,
            resumed: None,
            loss: None,
            required: Vec::new(),
            logged: None,
            logged_at: None,
        }
    }

    /// `(old, new)` when `percent` (an average frame's) changed since it was
    /// last logged, at most once a second.
    pub fn log_due(&mut self, now: Instant, percent: usize) -> Option<(usize, usize)> {
        let old = *self.logged.get_or_insert(usize::from(self.fallback));
        if old == percent
            || self
                .logged_at
                .is_some_and(|at| now.saturating_duration_since(at) < Duration::from_secs(1))
        {
            return None;
        }
        self.logged = Some(percent);
        self.logged_at = Some(now);
        Some((old, percent))
    }

    pub fn maximum(&self) -> usize {
        usize::from(self.maximum)
    }

    pub fn fallback(&self) -> usize {
        usize::from(self.fallback)
    }

    /// Whether a report was accepted since the stream started.
    pub fn reported(&self) -> bool {
        self.last.is_some()
    }

    /// The worst second's share of missing shards in the window, if any
    /// second had shards.
    pub fn loss(&self) -> Option<f64> {
        self.loss
    }

    /// Takes one report. A report without shards (a second without video)
    /// keeps the reports fresh and the sequence continuous.
    pub fn report(&mut self, now: Instant, report: &FecReport) -> Result<(), Rejected> {
        let sent = u64::from(report.data_shards) + u64::from(report.parity_shards);
        if sent > MAX_REPORTED_SHARDS {
            return Err(Rejected::Implausible);
        }
        if let Some((seq, at)) = self.last {
            let delta = report.seq.wrapping_sub(seq);
            if delta == 0 || delta >= 1 << 31 {
                return Err(Rejected::Stale);
            }
            if delta != 1 || now.saturating_duration_since(at) > FEEDBACK_TIMEOUT {
                // Don't join observations across an unknown gap.
                self.buckets = [Bucket::default(); WINDOW_SECONDS];
                self.resumed = Some(now);
            }
        }
        self.last = Some((report.seq, now));
        let second = now.saturating_duration_since(self.origin).as_secs();
        let bucket = &mut self.buckets[second as usize % WINDOW_SECONDS];
        if bucket.second != Some(second) {
            *bucket = Bucket {
                second: Some(second),
                ..Bucket::default()
            };
        }
        bucket.sent += sent;
        bucket.missing += u64::from(report.missing_shards).min(sent);
        let loss = self
            .buckets
            .iter()
            .filter(|b| {
                b.sent > 0
                    && b.second
                        .is_some_and(|s| s <= second && second - s < WINDOW_SECONDS as u64)
            })
            .map(|b| b.missing as f64 / b.sent as f64)
            .reduce(f64::max);
        if loss != self.loss {
            self.loss = loss;
            self.required = match loss {
                Some(loss) if loss > 0. => required_parity(loss),
                _ => Vec::new(),
            };
        }
        Ok(())
    }

    /// The percentage for a frame of `data_shards`.
    pub fn percent(&self, now: Instant, data_shards: usize) -> usize {
        let fresh = self
            .last
            .is_some_and(|(_, at)| now.saturating_duration_since(at) <= FEEDBACK_TIMEOUT);
        match self.loss {
            _ if !fresh => self.fallback(),
            None => self.fallback(),
            Some(loss) if loss <= 0. => {
                let settling = self
                    .resumed
                    .is_some_and(|at| now.saturating_duration_since(at) < WINDOW);
                if settling { self.fallback() } else { 0 }
            }
            Some(_) => self.select(data_shards),
        }
    }

    fn select(&self, data_shards: usize) -> usize {
        if data_shards == 0 {
            return 0;
        }
        let mut best = None;
        for ratio in 1..=self.maximum() {
            let (blocks, applied) = conventional_fec(data_shards, ratio, self.minimum_parity);
            if applied < ratio || blocks == 0 || blocks > MAX_BLOCKS {
                // The frame can't carry this percentage.
                continue;
            }
            best = Some(ratio);
            let required = &self.required[blocks - 1];
            let aligned = data_shards.div_ceil(blocks);
            let sufficient = (0..blocks).all(|block| {
                let data = data_shards.saturating_sub(block * aligned).min(aligned);
                let need = required[data.min(MAX_BLOCK_SHARDS)];
                data == 0
                    || (need != UNREACHABLE
                        && fec_parameters(data, ratio, self.minimum_parity).0 >= usize::from(need))
            });
            if sufficient {
                return ratio;
            }
        }
        // A cap is not a guarantee: keep the most protection the frame can carry.
        best.unwrap_or(self.maximum())
    }
}

/// For each block count, the parity a block of `data` shards needs so that
/// more losses than parity stay under the frame's share of
/// [`FRAME_FAILURE_TARGET`], under independent loss at `loss`.
fn required_parity(loss: f64) -> Vec<[u8; MAX_BLOCK_SHARDS + 1]> {
    (1..=MAX_BLOCKS)
        .map(|blocks| {
            let target = FRAME_FAILURE_TARGET / blocks as f64;
            let mut table = [UNREACHABLE; MAX_BLOCK_SHARDS + 1];
            table[0] = 0;
            // More data never needs less parity: each search starts at the last answer.
            let mut parity = 0;
            for (data, slot) in table.iter_mut().enumerate().skip(1) {
                while data + parity <= MAX_BLOCK_SHARDS
                    && tail(data + parity, parity, loss) > target
                {
                    parity += 1;
                }
                if data + parity > MAX_BLOCK_SHARDS {
                    break;
                }
                *slot = parity as u8;
            }
            table
        })
        .collect()
}

/// P(X > k) for X ~ Binomial(n, p).
fn tail(n: usize, k: usize, p: f64) -> f64 {
    if k >= n {
        return 0.;
    }
    if p >= 1. {
        return 1.;
    }
    let odds = p / (1. - p);
    let mut term = (1. - p).powi(n as i32);
    let mut cdf = term;
    for i in 0..k {
        term *= (n - i) as f64 / (i + 1) as f64 * odds;
        cdf += term;
    }
    (1. - cdf).max(0.)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(seq: u32, sent: u32, missing: u32) -> FecReport {
        FecReport {
            seq,
            data_shards: sent - sent / 6,
            parity_shards: sent / 6,
            missing_shards: missing,
            unrecovered_blocks: 0,
            interval_ms: 1000,
        }
    }

    /// P(X > k) by summing the upper terms with exact binomial coefficients.
    fn tail_reference(n: usize, k: usize, p: f64) -> f64 {
        let mut sum = 0.;
        for i in k + 1..=n {
            let mut c = 1f64;
            for j in 0..i {
                c = c * (n - j) as f64 / (j + 1) as f64;
            }
            sum += c * p.powi(i as i32) * (1. - p).powi((n - i) as i32);
        }
        sum
    }

    #[test]
    fn the_binomial_tail_matches_a_direct_sum() {
        for (n, k, p) in [(10, 2, 0.01), (60, 5, 0.03), (255, 20, 0.05), (40, 0, 0.1)] {
            let (fast, exact) = (tail(n, k, p), tail_reference(n, k, p));
            assert!((fast - exact).abs() < 1e-9, "{n} {k} {p}: {fast} {exact}");
        }
        assert_eq!(tail(10, 10, 0.5), 0.);
        assert_eq!(tail(10, 3, 1.), 1.);
        assert_eq!(tail(10, 3, 0.), 0.);
    }

    #[test]
    fn the_chosen_share_is_the_smallest_under_the_target() {
        let start = Instant::now();
        let mut fec = StatisticalFec::new(50, 20, 0, start);
        fec.report(start, &report(0, 1000, 10)).unwrap();
        assert_eq!(fec.loss(), Some(0.01));
        for shards in [1, 5, 30, 120, 255, 600, 900] {
            let percent = fec.percent(start, shards);
            assert!((1..=50).contains(&percent), "{shards}: {percent}");
            let check = |percent: usize| {
                let (blocks, applied) = conventional_fec(shards, percent, 0);
                assert_eq!(applied, percent);
                let aligned = shards.div_ceil(blocks);
                (0..blocks).all(|b| {
                    let data = shards.saturating_sub(b * aligned).min(aligned);
                    let parity = fec_parameters(data, percent, 0).0;
                    tail(data + parity, parity, 0.01) <= FRAME_FAILURE_TARGET / blocks as f64
                })
            };
            assert!(check(percent), "{shards}: {percent} misses the target");
            assert!(
                !check(percent - 1),
                "{shards}: {percent} is not the smallest"
            );
        }
        // Small frames need a larger share than large ones.
        assert!(fec.percent(start, 5) > fec.percent(start, 255));
    }

    #[test]
    fn heavy_loss_is_capped_by_the_maximum() {
        let start = Instant::now();
        let mut fec = StatisticalFec::new(30, 20, 0, start);
        fec.report(start, &report(0, 600, 300)).unwrap();
        assert_eq!(fec.percent(start, 100), 30);
        // Everything lost: still the cap, not nothing.
        fec.report(start + Duration::from_secs(1), &report(1, 600, 600))
            .unwrap();
        assert_eq!(fec.percent(start + Duration::from_secs(1), 100), 30);
        let none = StatisticalFec::new(0, 20, 0, start);
        assert_eq!(none.select(100), 0);
    }

    #[test]
    fn without_fresh_reports_the_host_percentage_applies() {
        let start = Instant::now();
        let mut fec = StatisticalFec::new(50, 20, 2, start);
        assert_eq!(fec.percent(start, 100), 20, "before the first report");
        fec.report(start, &report(0, 600, 0)).unwrap();
        // The first clean report may drop protection at once.
        assert_eq!(fec.percent(start, 100), 0);
        assert_eq!(fec.percent(start + FEEDBACK_TIMEOUT, 100), 0);
        let stale = start + FEEDBACK_TIMEOUT + Duration::from_millis(1);
        assert_eq!(fec.percent(stale, 100), 20, "stale");
    }

    #[test]
    fn a_gap_forgets_the_history_and_zero_loss_waits_a_window() {
        let start = Instant::now();
        let mut fec = StatisticalFec::new(50, 20, 0, start);
        fec.report(start, &report(0, 600, 30)).unwrap();
        let lossy = fec.percent(start, 100);
        assert!(lossy > 0);
        // Seq 1 is missing: the loss before the gap no longer counts, and zero
        // loss is trusted only after a full window of continuous reports.
        let mut now = start + Duration::from_secs(1);
        fec.report(now, &report(2, 600, 0)).unwrap();
        assert_eq!(fec.loss(), Some(0.));
        assert_eq!(fec.percent(now, 100), 20);
        for seq in 3..13 {
            now += Duration::from_secs(1);
            fec.report(now, &report(seq, 600, 0)).unwrap();
        }
        assert_eq!(fec.percent(now, 100), 0);
        // Loss after a gap counts at once.
        now += FEEDBACK_TIMEOUT + Duration::from_secs(1);
        fec.report(now, &report(13, 600, 30)).unwrap();
        assert_eq!(fec.percent(now, 100), lossy);
    }

    #[test]
    fn old_loss_leaves_the_window() {
        let start = Instant::now();
        let mut fec = StatisticalFec::new(50, 20, 0, start);
        fec.report(start, &report(0, 600, 60)).unwrap();
        let mut now = start;
        for seq in 1..WINDOW_SECONDS as u32 {
            now += Duration::from_secs(1);
            fec.report(now, &report(seq, 600, 6)).unwrap();
            assert_eq!(fec.loss(), Some(0.1), "{seq}");
        }
        now += Duration::from_secs(1);
        fec.report(now, &report(WINDOW_SECONDS as u32, 600, 6))
            .unwrap();
        assert_eq!(fec.loss(), Some(0.01));
    }

    #[test]
    fn reports_in_one_second_are_weighted_by_shards() {
        let start = Instant::now();
        let mut fec = StatisticalFec::new(50, 20, 0, start);
        fec.report(start, &report(0, 100, 10)).unwrap();
        fec.report(start + Duration::from_millis(500), &report(1, 900, 0))
            .unwrap();
        assert_eq!(fec.loss(), Some(0.01));
    }

    #[test]
    fn repeated_old_and_broken_reports_are_refused() {
        let start = Instant::now();
        let mut fec = StatisticalFec::new(50, 20, 0, start);
        fec.report(start, &report(u32::MAX, 600, 6)).unwrap();
        assert_eq!(
            fec.report(start, &report(u32::MAX, 600, 6)),
            Err(Rejected::Stale)
        );
        assert_eq!(
            fec.report(start, &report(u32::MAX - 5, 600, 6)),
            Err(Rejected::Stale)
        );
        // The sequence wraps.
        fec.report(start + Duration::from_secs(1), &report(0, 600, 6))
            .unwrap();
        let huge = FecReport {
            data_shards: 999_999,
            parity_shards: 2,
            ..report(1, 600, 0)
        };
        assert_eq!(fec.report(start, &huge), Err(Rejected::Implausible));
        // A second without video keeps the sequence and the estimate.
        let idle = start + Duration::from_secs(2);
        fec.report(
            idle,
            &FecReport {
                data_shards: 0,
                parity_shards: 0,
                missing_shards: 0,
                ..report(1, 600, 0)
            },
        )
        .unwrap();
        assert_eq!(fec.loss(), Some(0.01));
        assert!(fec.percent(idle, 100) > 0);
    }

    #[test]
    fn the_client_minimum_parity_counts() {
        let start = Instant::now();
        let mut fec = StatisticalFec::new(50, 20, 4, start);
        fec.report(start, &report(0, 100_000, 1)).unwrap();
        // At 0.001 % loss one parity shard is plenty, but every block carries
        // the client's four: the smallest percentage already reaches it.
        assert_eq!(fec.percent(start, 200), 1);
    }

    #[test]
    fn changes_are_logged_once_a_second() {
        let start = Instant::now();
        let mut fec = StatisticalFec::new(50, 20, 0, start);
        assert_eq!(fec.log_due(start, 20), None);
        assert_eq!(fec.log_due(start, 9), Some((20, 9)));
        assert_eq!(fec.log_due(start + Duration::from_millis(500), 12), None);
        assert_eq!(
            fec.log_due(start + Duration::from_secs(1), 12),
            Some((9, 12))
        );
    }

    #[test]
    fn the_tables_are_cheap_enough_for_the_control_thread() {
        // One table per changed estimate, at most once a second.
        let started = Instant::now();
        for loss in [0.001, 0.01, 0.05, 0.2, 0.5] {
            let table = required_parity(loss);
            assert_eq!(table.len(), MAX_BLOCKS);
            assert!(table.iter().all(|t| t[0] == 0));
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }
}
