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
//!   window before protection drops to [`MINIMUM_PERCENT`].
//! - Unlike Foundation's, protection never drops below [`MINIMUM_PERCENT`]
//!   (or the cap): a clean second says nothing about the next burst.
//!
//! The binomial tails are computed when a report changes the estimate, on the
//! control thread and outside the lock frames take ([`StatisticalFec::observe`],
//! then [`PendingEstimate::build`] and [`StatisticalFec::install`]); reports
//! closer than [`MIN_ESTIMATE_INTERVAL`] are counted without a new estimate. A
//! frame only walks the integer layout and compares its blocks' parity with
//! that table.
use std::time::{Duration, Instant};

pub use rubylight_protocol::fec_report::{FEC_REPORT_MESSAGE_TYPE, FecReport};

use crate::packet::conventional_fec;
use crate::pyrowave::fec_parameters;

/// The modelled chance that some block of a frame cannot be rebuilt.
pub const FRAME_FAILURE_TARGET: f64 = 0.001;
/// The least protection once reports show little or no loss. On real Wi-Fi
/// (Retroid, rust/PERFORMANCE.md, November 1) zero parity after a clean
/// second turned every lost packet into a lost frame and a keyframe, and a
/// single reordered packet into a lost-frame report; the PC's 20 % recovered
/// what that link lost. Half of it still keeps a few shards per block (5 at
/// 40 Mbps, 18 at 150 Mbps, 1080p60).
pub const MINIMUM_PERCENT: u8 = 10;
/// History the estimate covers, in one-second buckets.
pub const WINDOW_SECONDS: usize = 10;
pub const WINDOW: Duration = Duration::from_secs(WINDOW_SECONDS as u64);
/// Reports older than this are stale: the host's percentage applies again.
pub const FEEDBACK_TIMEOUT: Duration = Duration::from_secs(3);
/// Reports closer than this to the last estimate are counted but don't
/// re-estimate: a client sends one a second, and each estimate builds tables.
pub const MIN_ESTIMATE_INTERVAL: Duration = Duration::from_millis(500);
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

/// A new loss estimate whose parity tables are still to be built.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PendingEstimate {
    loss: f64,
}

impl PendingEstimate {
    /// Builds the tables from the binomial tails, the costly part of an estimate.
    pub fn build(self) -> Estimate {
        Estimate {
            loss: self.loss,
            required: required_parity(self.loss),
        }
    }
}

/// A loss estimate with its parity tables, for [`StatisticalFec::install`].
#[derive(Debug)]
pub struct Estimate {
    loss: f64,
    required: Vec<[u8; MAX_BLOCK_SHARDS + 1]>,
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
    /// When the window was last estimated.
    estimated: Option<Instant>,
    loss: Option<f64>,
    /// Parity a block needs, by block count and data shards; [`UNREACHABLE`]
    /// when it cannot fit. Empty until the estimate shows loss.
    required: Vec<[u8; MAX_BLOCK_SHARDS + 1]>,
    /// The loss of the last estimate handed out for its tables.
    pending: Option<f64>,
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
            estimated: None,
            loss: None,
            required: Vec::new(),
            pending: None,
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

    /// [`MINIMUM_PERCENT`], within the cap.
    pub fn floor(&self) -> usize {
        usize::from(MINIMUM_PERCENT.min(self.maximum))
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

    /// Takes one report and builds the tables of a new estimate at once; see
    /// [`Self::observe`] for a caller that holds a lock frames also take.
    pub fn report(&mut self, now: Instant, report: &FecReport) -> Result<(), Rejected> {
        if let Some(pending) = self.observe(now, report)? {
            self.install(pending.build());
        }
        Ok(())
    }

    /// Counts one report. A report without shards (a second without video)
    /// keeps the reports fresh and the sequence continuous. A new estimate
    /// with loss comes back to be built (without this lock) and installed;
    /// until then frames keep the previous one. A report less than
    /// [`MIN_ESTIMATE_INTERVAL`] after the last estimate only counts.
    pub fn observe(
        &mut self,
        now: Instant,
        report: &FecReport,
    ) -> Result<Option<PendingEstimate>, Rejected> {
        let sent = u64::from(report.data_shards) + u64::from(report.parity_shards);
        if sent > MAX_REPORTED_SHARDS {
            return Err(Rejected::Implausible);
        }
        let mut gap = false;
        if let Some((seq, at)) = self.last {
            let delta = report.seq.wrapping_sub(seq);
            if delta == 0 || delta >= 1 << 31 {
                return Err(Rejected::Stale);
            }
            if delta != 1 || now.saturating_duration_since(at) > FEEDBACK_TIMEOUT {
                // Don't join observations across an unknown gap.
                self.buckets = [Bucket::default(); WINDOW_SECONDS];
                self.resumed = Some(now);
                gap = true;
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
        if !gap
            && self
                .estimated
                .is_some_and(|at| now.saturating_duration_since(at) < MIN_ESTIMATE_INTERVAL)
        {
            return Ok(None);
        }
        self.estimated = Some(now);
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
        if loss == self.loss {
            self.pending = None;
            return Ok(None);
        }
        match loss {
            Some(loss) if loss > 0. => {
                self.pending = Some(loss);
                Ok(Some(PendingEstimate { loss }))
            }
            _ => {
                // No loss needs no tables.
                self.pending = None;
                self.loss = loss;
                self.required = Vec::new();
                Ok(None)
            }
        }
    }

    /// Installs the estimate [`Self::observe`] handed out last; an older one,
    /// or one for another stream, is dropped.
    pub fn install(&mut self, estimate: Estimate) {
        if self.pending == Some(estimate.loss) {
            self.pending = None;
            self.loss = Some(estimate.loss);
            self.required = estimate.required;
        }
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
                if settling {
                    self.fallback()
                } else {
                    self.floor()
                }
            }
            Some(_) => self.select(data_shards),
        }
    }

    fn select(&self, data_shards: usize) -> usize {
        if data_shards == 0 {
            return 0;
        }
        let mut best = None;
        for ratio in self.floor().max(1)..=self.maximum() {
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
mod tests;
