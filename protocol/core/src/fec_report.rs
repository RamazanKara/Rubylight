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
        self.take_after(now_ms, NOMINAL_INTERVAL_MS)
    }

    /// The report early, once its interval is [`MIN_INTERVAL_MS`] old, if it counts a block
    /// that could not be recovered: the host's FEC hears of the loss without waiting out the
    /// second.
    pub fn take_lost(&mut self, now_ms: u64) -> Option<FecReport> {
        if self.report.unrecovered_blocks == 0 {
            return None;
        }
        self.take_after(now_ms, MIN_INTERVAL_MS)
    }

    fn take_after(&mut self, now_ms: u64, interval_ms: u16) -> Option<FecReport> {
        let elapsed = now_ms.saturating_sub(self.since_ms?);
        if elapsed < u64::from(interval_ms) {
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
mod tests;
