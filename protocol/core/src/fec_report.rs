//! Client-chosen video FEC: the launch parameter [`LAUNCH_PARAMETER`] and the client's
//! once-a-second loss report ([`FecReport`], control message 0x5539, client to host).
//!
//! A host that takes both says so with the serverinfo element [`SERVERINFO_FIELD`] and lists
//! 0x5539 in DESCRIBE's `a=x-rl-control`. It acts on the reports only under
//! `rlFecPercentage=auto` and logs them otherwise. This is video RTP FEC only; PyroWave keeps
//! its own.
//!
//! Report wire form, little-endian:
//!
//! | Off | Type | Field |
//! |---|---|---|
//! | 0 | u8 | version, 1 |
//! | 1 | u8 | flags, 0 |
//! | 2 | u16 | reserved, 0 |
//! | 4 | u32 | seq: 0 for the stream's first report, then +1 |
//! | 8 | u32 | data shards of the FEC blocks finished in the interval |
//! | 12 | u32 | parity shards of those blocks |
//! | 16 | u32 | shards of those blocks that never arrived |
//! | 20 | u16 | blocks with more missing shards than parity |
//! | 22 | u16 | measured interval in ms, 250-5000 |

use alloc::string::{String, ToString};

/// Control-stream message type of [`FecReport`] (client to host).
pub const FEC_REPORT_MESSAGE_TYPE: u16 = 0x5539;
/// Encoded size of [`FecReport`].
pub const FEC_REPORT_BYTES: usize = 24;
/// Serverinfo element: the host takes [`LAUNCH_PARAMETER`] and [`FecReport`]s.
pub const SERVERINFO_FIELD: &str = "FecControlVersion";
/// `/launch` and `/resume` query parameter carrying a [`FecChoice`].
pub const LAUNCH_PARAMETER: &str = "rlFecPercentage";
/// Shortest and longest interval a report may cover.
pub const MIN_INTERVAL_MS: u16 = 250;
pub const MAX_INTERVAL_MS: u16 = 5000;
/// How often a client sends a report.
pub const NOMINAL_INTERVAL_MS: u16 = 1000;

const VERSION: u8 = 1;

/// What the client asked for with [`LAUNCH_PARAMETER`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FecChoice {
    /// The host's own `fec_percentage` and `adaptive_fec` (also when the parameter is absent).
    Host,
    /// Statistical FEC from the client's [`FecReport`]s.
    Auto,
    /// A fixed percentage. The host still caps it at its `fec_max_percentage`.
    Fixed(u8),
}

impl FecChoice {
    /// `host`, `auto` or a decimal 0-255 without a sign; anything else is `None`.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "host" => Some(Self::Host),
            "auto" => Some(Self::Auto),
            _ if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) => {
                text.parse().ok().map(Self::Fixed)
            }
            _ => None,
        }
    }

    /// The query value [`parse`](Self::parse) reads back.
    pub fn as_param(&self) -> String {
        match self {
            Self::Host => "host".into(),
            Self::Auto => "auto".into(),
            Self::Fixed(percentage) => percentage.to_string(),
        }
    }
}

/// One interval of the client's video FEC statistics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FecReport {
    pub seq: u32,
    pub data_shards: u32,
    pub parity_shards: u32,
    pub missing_shards: u32,
    pub unrecovered_blocks: u16,
    pub interval_ms: u16,
}

impl FecReport {
    pub fn encode(&self) -> [u8; FEC_REPORT_BYTES] {
        let mut out = [0; FEC_REPORT_BYTES];
        out[0] = VERSION;
        out[4..8].copy_from_slice(&self.seq.to_le_bytes());
        out[8..12].copy_from_slice(&self.data_shards.to_le_bytes());
        out[12..16].copy_from_slice(&self.parity_shards.to_le_bytes());
        out[16..20].copy_from_slice(&self.missing_shards.to_le_bytes());
        out[20..22].copy_from_slice(&self.unrecovered_blocks.to_le_bytes());
        out[22..24].copy_from_slice(&self.interval_ms.to_le_bytes());
        out
    }

    /// Parses a report; version 0, more missing shards than were sent and an interval
    /// outside 250-5000 ms reject it. Bytes past the known fields are ignored.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < FEC_REPORT_BYTES || data[0] == 0 {
            return None;
        }
        let u32_at =
            |at: usize| u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
        let report = Self {
            seq: u32_at(4),
            data_shards: u32_at(8),
            parity_shards: u32_at(12),
            missing_shards: u32_at(16),
            unrecovered_blocks: u16::from_le_bytes([data[20], data[21]]),
            interval_ms: u16::from_le_bytes([data[22], data[23]]),
        };
        let sent = u64::from(report.data_shards) + u64::from(report.parity_shards);
        let plausible = u64::from(report.missing_shards) <= sent
            && (MIN_INTERVAL_MS..=MAX_INTERVAL_MS).contains(&report.interval_ms);
        plausible.then_some(report)
    }
}

/// A client's video FEC outcomes since its last [`FecReport`], on a millisecond clock.
#[derive(Clone, Debug, Default)]
pub struct FecTally {
    report: FecReport,
    /// When the interval began: the first block counted, then each report.
    since_ms: Option<u64>,
    next_seq: u32,
}

impl FecTally {
    /// Counts one finished FEC block, recovered or not; the first one starts the interval.
    pub fn block(
        &mut self,
        data_shards: u32,
        parity_shards: u32,
        missing_shards: u32,
        unrecovered: bool,
        now_ms: u64,
    ) {
        self.since_ms.get_or_insert(now_ms);
        let report = &mut self.report;
        report.data_shards = report.data_shards.saturating_add(data_shards);
        report.parity_shards = report.parity_shards.saturating_add(parity_shards);
        report.missing_shards = report.missing_shards.saturating_add(missing_shards);
        report.unrecovered_blocks = report
            .unrecovered_blocks
            .saturating_add(u16::from(unrecovered));
    }

    /// The report once its interval is [`NOMINAL_INTERVAL_MS`] old, which starts the next
    /// interval. An interval without video, once video has begun, gives a report without
    /// shards, which keeps the host's reports fresh and their sequence continuous; one past
    /// [`MAX_INTERVAL_MS`] (a stalled sender) is dropped: its counts can't be timed.
    pub fn take(&mut self, now_ms: u64) -> Option<FecReport> {
        let elapsed = now_ms.saturating_sub(self.since_ms?);
        if elapsed < u64::from(NOMINAL_INTERVAL_MS) {
            return None;
        }
        let mut report = core::mem::take(&mut self.report);
        self.since_ms = Some(now_ms);
        if elapsed > u64::from(MAX_INTERVAL_MS) {
            return None;
        }
        report.seq = self.next_seq;
        report.interval_ms = elapsed as u16;
        self.next_seq = self.next_seq.wrapping_add(1);
        Some(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fec_choices_parse_and_print() {
        for (text, choice) in [
            ("host", FecChoice::Host),
            ("auto", FecChoice::Auto),
            ("0", FecChoice::Fixed(0)),
            ("20", FecChoice::Fixed(20)),
            ("255", FecChoice::Fixed(255)),
        ] {
            assert_eq!(FecChoice::parse(text), Some(choice), "{text}");
            assert_eq!(FecChoice::parse(&choice.as_param()), Some(choice));
        }
        for text in [
            "256", "-1", "+5", "05x", "", " 5", "Host", "AUTO", "1e2", "999999",
        ] {
            assert_eq!(FecChoice::parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn fec_reports_round_trip_and_reject_nonsense() {
        let report = FecReport {
            seq: 7,
            data_shards: 400,
            parity_shards: 80,
            missing_shards: 12,
            unrecovered_blocks: 1,
            interval_ms: 1000,
        };
        let wire = report.encode();
        assert_eq!(wire.len(), FEC_REPORT_BYTES);
        assert_eq!(wire[..4], [1, 0, 0, 0]);
        assert_eq!(wire[22..24], 1000u16.to_le_bytes());
        assert_eq!(FecReport::decode(&wire), Some(report));
        // A newer version's tail is ignored.
        let mut longer = wire.to_vec();
        longer.extend_from_slice(&[9; 8]);
        longer[0] = 2;
        assert_eq!(FecReport::decode(&longer), Some(report));
        assert_eq!(FecReport::decode(&wire[..FEC_REPORT_BYTES - 1]), None);
        let mut zero = wire;
        zero[0] = 0;
        assert_eq!(FecReport::decode(&zero), None);
        let all_missing = FecReport {
            missing_shards: 480,
            ..report
        };
        assert_eq!(FecReport::decode(&all_missing.encode()), Some(all_missing));
        let too_many = FecReport {
            missing_shards: 481,
            ..report
        };
        assert_eq!(FecReport::decode(&too_many.encode()), None);
        // No overflow at the extremes.
        let huge = FecReport {
            data_shards: u32::MAX,
            parity_shards: u32::MAX,
            missing_shards: u32::MAX,
            ..report
        };
        assert_eq!(FecReport::decode(&huge.encode()), Some(huge));
        for interval_ms in [0, 249, 5001, u16::MAX] {
            let report = FecReport {
                interval_ms,
                ..report
            };
            assert_eq!(FecReport::decode(&report.encode()), None, "{interval_ms}");
        }
        for interval_ms in [250, 5000] {
            let report = FecReport {
                interval_ms,
                ..report
            };
            assert_eq!(FecReport::decode(&report.encode()), Some(report));
        }
    }

    #[test]
    fn the_tally_reports_once_a_second_and_drops_what_it_cannot_time() {
        let mut tally = FecTally::default();
        assert_eq!(tally.take(5_000), None, "nothing counted yet");
        tally.block(10, 2, 1, false, 1_000);
        tally.block(10, 2, 12, true, 1_400);
        assert_eq!(tally.take(1_999), None, "under a second");
        let first = tally.take(2_050).unwrap();
        assert_eq!(
            first,
            FecReport {
                seq: 0,
                data_shards: 20,
                parity_shards: 4,
                missing_shards: 13,
                unrecovered_blocks: 1,
                interval_ms: 1_050,
            }
        );
        assert_eq!(FecReport::decode(&first.encode()), Some(first));
        // A second without video sends a report without shards, which the host takes: its
        // reports stay fresh while the picture stands still.
        let idle = tally.take(3_100).unwrap();
        assert_eq!(
            idle,
            FecReport {
                seq: 1,
                interval_ms: 1_050,
                ..FecReport::default()
            }
        );
        assert_eq!(FecReport::decode(&idle.encode()), Some(idle));
        tally.block(5, 1, 0, false, 3_200);
        let second = tally.take(4_100).unwrap();
        assert_eq!(
            (second.seq, second.data_shards, second.interval_ms),
            (2, 5, 1_000)
        );
        // A stalled sender: counts older than five seconds are dropped, the sequence goes on.
        tally.block(5, 1, 0, false, 4_200);
        assert_eq!(tally.take(9_101), None);
        tally.block(5, 1, 0, false, 9_200);
        assert_eq!(tally.take(10_101).unwrap().seq, 3);
        // Counts saturate instead of wrapping.
        let mut full = FecTally::default();
        for _ in 0..3 {
            full.block(u32::MAX, u32::MAX, u32::MAX, true, 0);
        }
        let report = full.take(1_000).unwrap();
        assert_eq!(report.data_shards, u32::MAX);
        assert_eq!(report.unrecovered_blocks, 3);
    }
}
