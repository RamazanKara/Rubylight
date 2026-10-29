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
//! | 12 | u32 | parity shards of those blocks up to their last arrival (0 on a clean link) |
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

/// Frames skipped between two finished blocks that still count as lost: a longer jump is a
/// stall or a renumbering, which the next estimate shouldn't take as shard loss.
pub const MAX_SKIPPED_FRAMES: u32 = 30;

/// One FEC block the video queue finished, completed or given up, with the queue's counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FecBlock {
    pub frame: u32,
    pub block_index: u8,
    /// Blocks in the frame, 1-4.
    pub block_count: u8,
    pub data_shards: u32,
    pub parity_shards: u32,
    /// Shards of the block that arrived.
    pub arrived_shards: u32,
    /// Holes before the block's last arrival.
    pub missing_shards: u32,
    /// The next block began before this one completed.
    pub unrecovered: bool,
}

/// Blocks between two finished ones that no packet announced: the rest of `last`'s frame and
/// the blocks before `next` (whole frames counted as one block each, their layout unknown).
/// After a given-up block the queue drops the rest of its frame itself, so those don't count;
/// a step back or more than [`MAX_SKIPPED_FRAMES`] counts nothing.
fn blocks_never_seen(last: &FecBlock, next: &FecBlock) -> (u32, u32) {
    let frames = next.frame.wrapping_sub(last.frame);
    if frames == 0 {
        let skipped = next
            .block_index
            .saturating_sub(last.block_index.saturating_add(1));
        return (0, u32::from(skipped));
    }
    if frames > MAX_SKIPPED_FRAMES + 1 {
        return (0, 0);
    }
    let rest = if last.unrecovered {
        0
    } else {
        last.block_count
            .saturating_sub(last.block_index.saturating_add(1))
    };
    (u32::from(rest), frames - 1 + u32::from(next.block_index))
}

/// A client's video FEC outcomes since its last [`FecReport`], on a millisecond clock.
#[derive(Clone, Debug, Default)]
pub struct FecTally {
    report: FecReport,
    /// When the interval began: the first block counted, then each report.
    since_ms: Option<u64>,
    next_seq: u32,
    /// The last block counted, to find blocks that never arrived.
    last: Option<FecBlock>,
}

impl FecTally {
    /// Counts one finished FEC block; the first one starts the interval.
    ///
    /// Only shards whose arrival could be seen count: a completed block up to its last arrival
    /// (parity still in flight is neither lost nor sent), a given-up block whole. Blocks
    /// missing between this one and the last, which no packet announced, count whole and lost.
    pub fn block(&mut self, block: FecBlock, now_ms: u64) {
        self.since_ms.get_or_insert(now_ms);
        let total = block.data_shards.saturating_add(block.parity_shards);
        let (sent, missing) = if block.unrecovered {
            (total, total.saturating_sub(block.arrived_shards))
        } else {
            let seen = block
                .arrived_shards
                .saturating_add(block.missing_shards)
                .min(total);
            (seen, block.missing_shards.min(seen))
        };
        let data = sent.min(block.data_shards);
        self.count(data, sent - data, missing, u16::from(block.unrecovered));
        if let Some(last) = self.last.replace(block) {
            let (after_last, before_this) = blocks_never_seen(&last, &block);
            self.lost(&last, after_last);
            self.lost(&block, before_this);
        }
    }

    /// Counts `blocks` blocks the size of `like` as lost whole.
    fn lost(&mut self, like: &FecBlock, blocks: u32) {
        if blocks > 0 {
            let (data, parity) = (
                like.data_shards.saturating_mul(blocks),
                like.parity_shards.saturating_mul(blocks),
            );
            let blocks = u16::try_from(blocks).unwrap_or(u16::MAX);
            self.count(data, parity, data.saturating_add(parity), blocks);
        }
    }

    fn count(&mut self, data: u32, parity: u32, missing: u32, unrecovered: u16) {
        let report = &mut self.report;
        report.data_shards = report.data_shards.saturating_add(data);
        report.parity_shards = report.parity_shards.saturating_add(parity);
        report.missing_shards = report.missing_shards.saturating_add(missing);
        report.unrecovered_blocks = report.unrecovered_blocks.saturating_add(unrecovered);
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

// Foundation Sunshine's client FEC control, as Moonlight V+ speaks it (qiin2333's
// moonlight-common-c 2aa14ff: RtspConnection.c, SdpGenerator.c, ControlStream.c; host side in
// Foundation's rtsp.cpp and stream.cpp). A host that says `a=x-ss-video[0].fecControlVersion:1`
// in DESCRIBE takes the client's choice as [`FEC_PERCENTAGE_ATTRIBUTE`] in the ANNOUNCE (with
// [`FEC_SUMMARY_VERSION_ATTRIBUTE`]) and a [`FecSummary`] once a second. Rubylight hosts never
// say it; they take [`LAUNCH_PARAMETER`] and [`FecReport`]s instead.

/// DESCRIBE attribute of a host that takes [`FEC_PERCENTAGE_ATTRIBUTE`] and [`FecSummary`]s.
pub const FEC_CONTROL_DESCRIBE_ATTRIBUTE: &str = "x-ss-video[0].fecControlVersion";
/// The [`FEC_CONTROL_DESCRIBE_ATTRIBUTE`] version this client speaks.
pub const FEC_CONTROL_VERSION: u32 = 1;
/// ANNOUNCE attribute: [`FecChoice::summary_percentage`].
pub const FEC_PERCENTAGE_ATTRIBUTE: &str = "x-ml-video.fecPercentage";
/// ANNOUNCE attribute, always `1`: the client sends [`FecSummary`]s.
pub const FEC_SUMMARY_VERSION_ATTRIBUTE: &str = "x-ml-video.fecSummaryVersion";
/// Control-stream message type of [`FecSummary`] (client to host, `SS_FEC_SUMMARY_PTYPE`).
pub const FEC_SUMMARY_MESSAGE_TYPE: u16 = 0x5510;
/// Encoded size of [`FecSummary`].
pub const FEC_SUMMARY_BYTES: usize = 12;
/// The highest fixed percentage [`FEC_PERCENTAGE_ATTRIBUTE`] carries; the host refuses more.
pub const FEC_SUMMARY_MAX_PERCENTAGE: u8 = 100;
/// Data shards one summary counts at most (V+'s bound).
const FEC_SUMMARY_MAX_DATA: u32 = 1_000_000;

impl FecChoice {
    /// The [`FEC_PERCENTAGE_ATTRIBUTE`] value (V+'s `videoFecPercentage`): -2 for the host's
    /// own, -1 for automatic, else the fixed share, at most [`FEC_SUMMARY_MAX_PERCENTAGE`].
    pub fn summary_percentage(&self) -> i32 {
        match self {
            Self::Host => -2,
            Self::Auto => -1,
            Self::Fixed(percentage) => i32::from((*percentage).min(FEC_SUMMARY_MAX_PERCENTAGE)),
        }
    }
}

/// `SS_FEC_SUMMARY`: one second of finished video FEC blocks, three big-endian u32s.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FecSummary {
    /// 1 for the stream's first summary, then +1 every second, also for one not sent.
    pub seq: u32,
    pub data_shards: u32,
    /// Data shards that had not arrived when their block finished.
    pub missing_data_shards: u32,
}

impl FecSummary {
    pub fn encode(&self) -> [u8; FEC_SUMMARY_BYTES] {
        let mut out = [0; FEC_SUMMARY_BYTES];
        out[0..4].copy_from_slice(&self.seq.to_be_bytes());
        out[4..8].copy_from_slice(&self.data_shards.to_be_bytes());
        out[8..12].copy_from_slice(&self.missing_data_shards.to_be_bytes());
        out
    }

    /// Parses exactly [`FEC_SUMMARY_BYTES`] bytes, as the host does.
    pub fn decode(data: &[u8]) -> Option<Self> {
        let data: &[u8; FEC_SUMMARY_BYTES] = data.try_into().ok()?;
        let u32_at = |at: usize| u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
        Some(Self {
            seq: u32_at(0),
            data_shards: u32_at(4),
            missing_data_shards: u32_at(8),
        })
    }
}

/// The data shards of the blocks finished since the last [`FecSummary`]
/// (`connectionRecordFecBlock`): each block counts once, at its first final status.
#[derive(Clone, Debug, Default)]
pub struct FecSummaryTally {
    data_shards: u32,
    missing_data_shards: u32,
    last_block: Option<(u32, u8)>,
    seq: u32,
}

impl FecSummaryTally {
    /// Counts block `block` of frame `frame`: `data` data shards, `received` of them arrived.
    /// A repeat of the last block, an empty block, more received than sent and a block past
    /// the bound of data shards per summary count nothing.
    pub fn block(&mut self, frame: u32, block: u8, data: u16, received: u16) {
        let (data, received) = (u32::from(data), u32::from(received));
        if data == 0 || received > data || self.last_block == Some((frame, block)) {
            return;
        }
        if data <= FEC_SUMMARY_MAX_DATA - self.data_shards {
            self.data_shards += data;
            self.missing_data_shards += data - received;
            self.last_block = Some((frame, block));
        }
    }

    /// Closes the second: the next sequence number is used either way, but a second without
    /// data shards sends nothing.
    pub fn take(&mut self) -> Option<FecSummary> {
        self.seq = self.seq.wrapping_add(1);
        let summary = FecSummary {
            seq: self.seq,
            data_shards: core::mem::take(&mut self.data_shards),
            missing_data_shards: core::mem::take(&mut self.missing_data_shards),
        };
        (summary.data_shards != 0).then_some(summary)
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

    /// A finished block of `data` + `parity` shards at frame `frame`, block 0 of 1.
    fn block(
        frame: u32,
        data: u32,
        parity: u32,
        arrived: u32,
        missing: u32,
        lost: bool,
    ) -> FecBlock {
        FecBlock {
            frame,
            block_index: 0,
            block_count: 1,
            data_shards: data,
            parity_shards: parity,
            arrived_shards: arrived,
            missing_shards: missing,
            unrecovered: lost,
        }
    }

    #[test]
    fn the_tally_reports_once_a_second_and_drops_what_it_cannot_time() {
        let mut tally = FecTally::default();
        assert_eq!(tally.take(5_000), None, "nothing counted yet");
        tally.block(block(1, 10, 2, 11, 1, false), 1_000);
        tally.block(block(2, 10, 2, 0, 0, true), 1_400);
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
        tally.block(block(3, 5, 1, 6, 0, false), 3_200);
        let second = tally.take(4_100).unwrap();
        assert_eq!(
            (second.seq, second.data_shards, second.interval_ms),
            (2, 5, 1_000)
        );
        // A stalled sender: counts older than five seconds are dropped, the sequence goes on.
        tally.block(block(4, 5, 1, 6, 0, false), 4_200);
        assert_eq!(tally.take(9_101), None);
        tally.block(block(5, 5, 1, 6, 0, false), 9_200);
        assert_eq!(tally.take(10_101).unwrap().seq, 3);
        // Counts saturate instead of wrapping.
        let mut full = FecTally::default();
        for frame in 1..4 {
            full.block(block(frame, u32::MAX, u32::MAX, 0, 0, true), 0);
        }
        let report = full.take(1_000).unwrap();
        assert_eq!(report.data_shards, u32::MAX);
        assert_eq!(report.unrecovered_blocks, 3);
    }
    #[test]
    fn the_tally_counts_what_it_could_see_and_blocks_that_never_came() {
        let mut tally = FecTally::default();
        let mut now = 0;
        let mut counted = |b: FecBlock| {
            tally.block(b, now);
            now += 1_000;
            let r = tally.take(now).unwrap();
            assert!(r.missing_shards <= r.data_shards + r.parity_shards);
            (
                r.data_shards,
                r.parity_shards,
                r.missing_shards,
                r.unrecovered_blocks,
            )
        };
        // Complete before its parity arrived: the parity in flight is neither sent nor lost.
        assert_eq!(counted(block(1, 10, 4, 10, 0, false)), (10, 0, 0, 0));
        // One hole before the last arrival, two parity shards still on the way.
        assert_eq!(counted(block(2, 10, 4, 11, 1, false)), (10, 2, 1, 0));
        // Given up: every shard of the block was due.
        assert_eq!(counted(block(3, 10, 4, 7, 2, true)), (10, 4, 7, 1));
        // Frames 4 and 5 never arrived: lost whole, the size of the next one.
        assert_eq!(counted(block(6, 10, 4, 10, 0, false)), (10 + 20, 8, 28, 2));
        // Block 1 of frame 7 never arrived, then the next frame begins after block 2 of 3.
        let first = FecBlock {
            block_count: 3,
            ..block(7, 4, 1, 5, 0, false)
        };
        assert_eq!(counted(first), (4, 1, 0, 0));
        let third = FecBlock {
            block_index: 2,
            ..first
        };
        assert_eq!(counted(third), (8, 2, 5, 1));
        assert_eq!(counted(block(8, 10, 4, 10, 0, false)), (10, 0, 0, 0));
        // The rest of a frame after a completed block never came; after a given-up block the
        // queue drops it, so it doesn't count.
        let two = |frame, lost| FecBlock {
            block_count: 2,
            ..block(frame, 4, 1, if lost { 0 } else { 5 }, 0, lost)
        };
        assert_eq!(counted(two(9, false)), (4, 1, 0, 0));
        assert_eq!(counted(block(10, 10, 4, 10, 0, false)), (14, 1, 5, 1));
        assert_eq!(counted(two(11, true)), (4, 1, 5, 1));
        assert_eq!(counted(block(12, 10, 4, 10, 0, false)), (10, 0, 0, 0));
        // A jump past MAX_SKIPPED_FRAMES or a step back counts nothing extra.
        let far = 12 + MAX_SKIPPED_FRAMES + 2;
        assert_eq!(counted(block(far, 10, 4, 10, 0, false)), (10, 0, 0, 0));
        assert_eq!(counted(block(far - 5, 10, 4, 10, 0, false)), (10, 0, 0, 0));
        // The longest gap that counts.
        let gap = far - 5 + MAX_SKIPPED_FRAMES + 1;
        let (_, _, missing, lost) = counted(block(gap, 10, 4, 10, 0, false));
        assert_eq!(
            (missing, lost),
            (14 * MAX_SKIPPED_FRAMES, MAX_SKIPPED_FRAMES as u16)
        );
        // Frame numbers wrap.
        let mut wrap = FecTally::default();
        wrap.block(block(u32::MAX, 10, 4, 14, 0, false), 0);
        wrap.block(block(1, 10, 4, 14, 0, false), 10);
        assert_eq!(wrap.take(1_000).unwrap().unrecovered_blocks, 1);
    }

    #[test]
    fn fec_summaries_have_the_v_plus_wire_form() {
        // V+'s SS_FEC_SUMMARY: sequence, data packets, missing data packets, all BE32, which
        // Foundation's 0x5510 handler reads with util::endian::big after a 12-byte check.
        let summary = FecSummary {
            seq: 0x0102_0304,
            data_shards: 0x0000_0190,
            missing_data_shards: 0x0000_000c,
        };
        assert_eq!(
            summary.encode(),
            [1, 2, 3, 4, 0, 0, 1, 0x90, 0, 0, 0, 0x0c]
        );
        assert_eq!(FecSummary::decode(&summary.encode()), Some(summary));
        assert_eq!(FecSummary::decode(&summary.encode()[..11]), None);
        assert_eq!(FecSummary::decode(&[0; 13]), None);
        assert_eq!(FEC_SUMMARY_MESSAGE_TYPE, 0x5510);
        assert_eq!(FEC_CONTROL_DESCRIBE_ATTRIBUTE, "x-ss-video[0].fecControlVersion");
        assert_eq!(FEC_PERCENTAGE_ATTRIBUTE, "x-ml-video.fecPercentage");
        assert_eq!(FEC_SUMMARY_VERSION_ATTRIBUTE, "x-ml-video.fecSummaryVersion");
    }

    #[test]
    fn the_summary_percentage_is_v_plus_video_fec_percentage() {
        // STREAM_CONFIGURATION.videoFecPercentage: -2 host, -1 auto, 0-100 fixed; Foundation
        // answers 400 to anything else.
        assert_eq!(FecChoice::Host.summary_percentage(), -2);
        assert_eq!(FecChoice::Auto.summary_percentage(), -1);
        assert_eq!(FecChoice::Fixed(0).summary_percentage(), 0);
        assert_eq!(FecChoice::Fixed(50).summary_percentage(), 50);
        assert_eq!(FecChoice::Fixed(100).summary_percentage(), 100);
        assert_eq!(FecChoice::Fixed(255).summary_percentage(), 100);
    }

    #[test]
    fn the_summary_tally_counts_each_block_once_like_v_plus() {
        let mut tally = FecSummaryTally::default();
        // The first second: nothing finished, so nothing goes out, but the sequence moves on.
        assert_eq!(tally.take(), None);
        // A recovered block reports its final status first; the completion that follows is the
        // same block and doesn't count again.
        tally.block(1, 0, 10, 8);
        tally.block(1, 0, 10, 10);
        tally.block(1, 1, 6, 6);
        // Empty blocks and more received than sent count nothing.
        tally.block(2, 0, 0, 0);
        tally.block(2, 0, 4, 5);
        assert_eq!(
            tally.take(),
            Some(FecSummary {
                seq: 2,
                data_shards: 16,
                missing_data_shards: 2,
            })
        );
        // Counts start over each second; a block repeated after a take still doesn't count.
        tally.block(1, 1, 6, 0);
        tally.block(3, 0, 5, 0);
        assert_eq!(
            tally.take(),
            Some(FecSummary {
                seq: 3,
                data_shards: 5,
                missing_data_shards: 5,
            })
        );
        // A second holds at most a million data shards; the block that would pass it is left out.
        for frame in 0..15 {
            tally.block(10 + frame, 0, u16::MAX, u16::MAX);
        }
        let full = tally.take().unwrap();
        assert_eq!(full.data_shards, 15 * u32::from(u16::MAX));
        tally.block(100, 0, u16::MAX, 0);
        for frame in 0..14 {
            tally.block(200 + frame, 0, u16::MAX, u16::MAX);
        }
        tally.block(300, 0, u16::MAX, u16::MAX);
        let capped = tally.take().unwrap();
        assert!(capped.data_shards <= 1_000_000);
        assert_eq!(capped.data_shards, 15 * u32::from(u16::MAX));
        assert_eq!(capped.missing_data_shards, u32::from(u16::MAX));
    }
}
