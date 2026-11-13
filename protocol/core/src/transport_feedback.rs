//! Per-packet arrival feedback for video (control message 0x5547, client to host).
//!
//! A host that runs packet-level congestion control lists 0x5547 on its second
//! `a=x-rl-control` line (the `DESCRIBE` answer only; there is no serverinfo element). A
//! client that sees it reports when each video datagram arrived, in batches: a report covers a
//! run of packet ids, every `stride`-th one, and says for each whether and when it arrived. The
//! host joins the ids with its own send times and runs a delay- and loss-based controller on the
//! result (`congestion` in butterpollo-core). The design follows transport-wide congestion
//! control feedback (TWCC, draft-holmer-rmcat-transport-wide-cc-extensions) in a compact form
//! that needs no new field in the video packets.
//!
//! # Packet id
//!
//! Every video datagram already carries a counter that rises by one per packet, data and parity
//! alike, readable without decrypting it ([`packet_id`]):
//!
//! - **encrypted video** (`SS_ENC_VIDEO`): the low 16 bits of the 64-bit IV counter, the
//!   little-endian u16 at offset 0 of the datagram;
//! - **plain video**: the RTP sequence number, the big-endian u16 at offset 2.
//!
//! The client reads it before anything else, so datagrams the depacketizer drops unopened (the
//! parity of a frame already rebuilt) count as arrived.
//!
//! # Report wire form, little-endian
//!
//! | Off | Type | Field |
//! |---|---|---|
//! | 0 | u8 | version, 1 |
//! | 1 | u8 | flags: bit 0 set when this report does not start where the previous one ended |
//! | 2 | u16 | count of entries, 1-[`MAX_ENTRIES`] |
//! | 4 | u16 | base id: the id of entry 0 |
//! | 6 | u16 | stride, 1-[`MAX_STRIDE`]: entry `i` is id `base + i × stride` (mod 2^16) |
//! | 8 | u32 | report sequence: 0 for the stream's first report, then +1 |
//! | 12 | u32 | base arrival: the client's monotonic clock in µs (low 32 bits) that offsets count from |
//! | 16 | u16 | late: packets that arrived after the report covering them was sent |
//! | 18 | u16 | reserved, 0 |
//! | 20 | u16 × count | arrival offset from base arrival in [`TICK_US`] µs units, [`NOT_RECEIVED`] if it did not arrive |
//!
//! A host reads the fields it knows and ignores bytes past `20 + 2 × count`. Consecutive reports
//! continue where the previous one ended: the next base is `base + count × stride`. When more ids
//! went missing than one report holds, the client starts the next report at the packet that
//! arrived and sets flag bit 0. A host counts a skip of fewer than [`MAX_GAP_REPORTS`] reports'
//! worth of ids (`MAX_GAP_REPORTS × MAX_ENTRIES × stride`) as lost and a longer one as a stall or
//! a renumbered stream.
//!
//! # Cost
//!
//! A report goes out every [`INTERVAL_US`] or once [`MAX_ENTRIES`] entries fill, whichever comes
//! first, unreliable and at most 980 bytes, below the ENet fragment size. The stride grows with
//! the packet rate so a report interval holds at most [`MAX_ENTRIES`] entries: about 9 KB/s at
//! 50 Mbps, and at most about 20 KB/s at any rate.

use alloc::vec;
use alloc::vec::Vec;

/// Control-stream message type of [`TransportFeedback`] (client to host).
pub const TRANSPORT_FEEDBACK_MESSAGE_TYPE: u16 = 0x5547;
/// The header before the entries.
pub const HEADER_BYTES: usize = 20;
/// Entries one report carries at most.
pub const MAX_ENTRIES: usize = 480;
/// The most a report may thin the ids out.
pub const MAX_STRIDE: u16 = 64;
/// The unit of an entry's arrival offset, µs.
pub const TICK_US: u64 = 4;
/// An entry for a packet that did not arrive.
pub const NOT_RECEIVED: u16 = 0xFFFF;
/// The latest offset an entry can carry; a later arrival is clamped to it.
pub const MAX_OFFSET: u16 = 0xFFFE;
/// How often a client sends a report while video arrives.
pub const INTERVAL_US: u64 = 50_000;
/// A skipped run (flag bit 0) of fewer than this many full reports of ids counts as lost.
pub const MAX_GAP_REPORTS: usize = 4;

const VERSION: u8 = 1;
/// Flag bit 0: ids were skipped before this report.
pub const FLAG_SKIPPED: u8 = 1;

/// The packet id of a video datagram: the IV counter's low 16 bits when the video is encrypted,
/// the RTP sequence number otherwise. `None` for a datagram too short to hold it.
pub fn packet_id(datagram: &[u8], encrypted: bool) -> Option<u16> {
    if encrypted {
        Some(u16::from_le_bytes([*datagram.first()?, *datagram.get(1)?]))
    } else {
        Some(u16::from_be_bytes([*datagram.get(2)?, *datagram.get(3)?]))
    }
}

/// One report.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TransportFeedback {
    pub flags: u8,
    pub base: u16,
    pub stride: u16,
    pub seq: u32,
    pub base_arrival_us: u32,
    pub late: u16,
    /// Per entry, the arrival offset in [`TICK_US`] units or [`NOT_RECEIVED`].
    pub entries: Vec<u16>,
}

impl TransportFeedback {
    /// The wire form; `None` when the report breaks the rules [`decode`](Self::decode) checks.
    pub fn encode(&self) -> Option<Vec<u8>> {
        if !self.valid() {
            return None;
        }
        let mut out = vec![0; HEADER_BYTES + 2 * self.entries.len()];
        out[0] = VERSION;
        out[1] = self.flags;
        out[2..4].copy_from_slice(&(self.entries.len() as u16).to_le_bytes());
        out[4..6].copy_from_slice(&self.base.to_le_bytes());
        out[6..8].copy_from_slice(&self.stride.to_le_bytes());
        out[8..12].copy_from_slice(&self.seq.to_le_bytes());
        out[12..16].copy_from_slice(&self.base_arrival_us.to_le_bytes());
        out[16..18].copy_from_slice(&self.late.to_le_bytes());
        for (i, entry) in self.entries.iter().enumerate() {
            let at = HEADER_BYTES + 2 * i;
            out[at..at + 2].copy_from_slice(&entry.to_le_bytes());
        }
        Some(out)
    }

    /// Parses a report. Version 0, no entries, more than [`MAX_ENTRIES`], a stride outside
    /// 1-[`MAX_STRIDE`] and a payload shorter than its entries reject it.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < HEADER_BYTES || data[0] == 0 {
            return None;
        }
        let u16_at = |at: usize| u16::from_le_bytes([data[at], data[at + 1]]);
        let u32_at =
            |at: usize| u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
        let count = usize::from(u16_at(2));
        if count == 0 || count > MAX_ENTRIES || data.len() < HEADER_BYTES + 2 * count {
            return None;
        }
        let report = Self {
            flags: data[1],
            base: u16_at(4),
            stride: u16_at(6),
            seq: u32_at(8),
            base_arrival_us: u32_at(12),
            late: u16_at(16),
            entries: (0..count).map(|i| u16_at(HEADER_BYTES + 2 * i)).collect(),
        };
        report.valid().then_some(report)
    }

    fn valid(&self) -> bool {
        (1..=MAX_ENTRIES).contains(&self.entries.len()) && (1..=MAX_STRIDE).contains(&self.stride)
    }

    /// The id of entry `index`.
    pub fn id(&self, index: usize) -> u16 {
        self.base
            .wrapping_add((index as u16).wrapping_mul(self.stride))
    }

    /// Each entry as `(id, arrival offset in µs from base arrival)`, `None` for a packet that
    /// did not arrive.
    pub fn packets(&self) -> impl Iterator<Item = (u16, Option<u64>)> + '_ {
        self.entries.iter().enumerate().map(|(i, &entry)| {
            (
                self.id(i),
                (entry != NOT_RECEIVED).then(|| u64::from(entry) * TICK_US),
            )
        })
    }

    /// The id after the last entry, where the next report starts unless it skips ahead.
    pub fn next_id(&self) -> u16 {
        self.id(self.entries.len())
    }
}

/// The client's side: arrivals in, reports out. One per video stream, fed by the receive
/// thread before the depacketizer sees the datagram. Bounded: at most [`MAX_ENTRIES`] slots.
#[derive(Clone, Debug)]
pub struct FeedbackTally {
    stride: u16,
    /// The id of slot 0 of the open report.
    base: Option<u16>,
    /// Arrival per slot, client µs; `u64::MAX` for none yet.
    slots: Vec<u64>,
    /// Slots up to the highest one filled.
    used: usize,
    /// When the open report's first packet arrived.
    opened_us: u64,
    flags: u8,
    late: u32,
    seq: u32,
    /// Packets since the last report, sampled or not, for the next stride.
    seen: u64,
    /// When the last report was sent.
    last_report_us: Option<u64>,
}

impl Default for FeedbackTally {
    fn default() -> Self {
        Self {
            stride: 1,
            base: None,
            slots: vec![u64::MAX; MAX_ENTRIES],
            used: 0,
            opened_us: 0,
            flags: 0,
            late: 0,
            seq: 0,
            seen: 0,
            last_report_us: None,
        }
    }
}

impl FeedbackTally {
    /// The stride the next report uses.
    pub fn stride(&self) -> u16 {
        self.stride
    }

    /// A datagram with packet id `id` arrived at `now_us` (any monotonic µs clock). Returns a
    /// report when one is due: after [`INTERVAL_US`], or when this packet lies past what the
    /// open report can hold (it then opens the next one).
    pub fn packet(&mut self, id: u16, now_us: u64) -> Option<TransportFeedback> {
        self.seen += 1;
        let Some(base) = self.base else {
            self.open(id, now_us, 0);
            self.place(0, now_us);
            return None;
        };
        let ahead = id.wrapping_sub(base);
        if ahead >= 0x8000 {
            // Before the open report: its report has gone.
            self.late = self.late.saturating_add(1);
            return self.due(now_us);
        }
        let stride = self.stride;
        if ahead % stride != 0 {
            return self.due(now_us);
        }
        let slot = usize::from(ahead / stride);
        if slot < MAX_ENTRIES {
            self.place(slot, now_us);
            return self.due(now_us);
        }
        // Past the open report: send it, then continue at its end, or skip ahead to `id` and
        // say so (the host counts a short skip as loss and a long one as a renumbering).
        let report = self.take(now_us);
        let next = report.as_ref().map_or(base, TransportFeedback::next_id);
        // `take` may have changed the stride; the next report starts on its own grid.
        let stride = usize::from(self.stride);
        let gap = usize::from(id.wrapping_sub(next));
        if gap < 0x8000 && gap / stride < MAX_ENTRIES {
            self.open(next, now_us, 0);
            if gap % stride == 0 {
                self.place(gap / stride, now_us);
            }
        } else {
            self.open(id, now_us, FLAG_SKIPPED);
            self.place(0, now_us);
        }
        report
    }

    /// Sends the open report early (the stream is ending, or the caller flushes on its own
    /// clock); `None` when nothing is open.
    pub fn flush(&mut self, now_us: u64) -> Option<TransportFeedback> {
        let report = self.take(now_us)?;
        let next = report.next_id();
        self.open(next, now_us, 0);
        self.used = 0;
        Some(report)
    }

    fn open(&mut self, base: u16, now_us: u64, flags: u8) {
        self.base = Some(base);
        self.slots.fill(u64::MAX);
        self.used = 0;
        self.opened_us = now_us;
        self.flags |= flags;
    }

    fn place(&mut self, slot: usize, now_us: u64) {
        if self.used == 0 {
            self.opened_us = now_us;
        }
        if self.slots[slot] == u64::MAX {
            self.slots[slot] = now_us;
        }
        self.used = self.used.max(slot + 1);
    }

    fn due(&mut self, now_us: u64) -> Option<TransportFeedback> {
        if self.used == 0 || now_us.saturating_sub(self.opened_us) < INTERVAL_US {
            return None;
        }
        self.flush(now_us)
    }

    /// The open report, and the stride for the next one; `None` with no slot used.
    fn take(&mut self, now_us: u64) -> Option<TransportFeedback> {
        let base = self.base?;
        if self.used == 0 {
            return None;
        }
        let slots = &self.slots[..self.used];
        let first = slots.iter().copied().filter(|&at| at != u64::MAX).min();
        let entries = slots
            .iter()
            .map(|&at| match (at, first) {
                (u64::MAX, _) | (_, None) => NOT_RECEIVED,
                (at, Some(first)) => ((at - first) / TICK_US).min(u64::from(MAX_OFFSET)) as u16,
            })
            .collect();
        let report = TransportFeedback {
            flags: core::mem::take(&mut self.flags),
            base,
            stride: self.stride,
            seq: self.seq,
            base_arrival_us: first.unwrap_or(now_us) as u32,
            late: core::mem::take(&mut self.late).min(u32::from(u16::MAX)) as u16,
            entries,
        };
        self.seq = self.seq.wrapping_add(1);
        self.next_stride(now_us);
        self.slots.fill(u64::MAX);
        self.used = 0;
        Some(report)
    }

    /// Enough stride that one interval's packets fit one report, from the rate since the last.
    fn next_stride(&mut self, now_us: u64) {
        let seen = core::mem::take(&mut self.seen);
        let Some(last) = self.last_report_us.replace(now_us) else {
            return;
        };
        let elapsed = now_us.saturating_sub(last).max(1);
        // Packets per interval at the measured rate, rounded up.
        let per_interval = (seen * INTERVAL_US).div_ceil(elapsed);
        let wanted = per_interval
            .div_ceil(MAX_ENTRIES as u64)
            .clamp(1, u64::from(MAX_STRIDE));
        // A stride only changes at a report boundary, which keeps the next base on the grid.
        self.stride = wanted as u16;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_round_trips_and_extra_bytes_are_ignored() {
        let report = TransportFeedback {
            flags: FLAG_SKIPPED,
            base: 0xFFFE,
            stride: 2,
            seq: 7,
            base_arrival_us: 0xDEAD_BEEF,
            late: 3,
            entries: vec![0, 25, NOT_RECEIVED, MAX_OFFSET],
        };
        let mut bytes = report.encode().unwrap();
        assert_eq!(bytes.len(), HEADER_BYTES + 8);
        assert_eq!(&bytes[..4], &[1, 1, 4, 0]);
        assert_eq!(&bytes[4..8], &[0xFE, 0xFF, 2, 0]);
        bytes.extend_from_slice(&[9, 9, 9]);
        assert_eq!(TransportFeedback::decode(&bytes), Some(report.clone()));
        let ids: Vec<_> = report.packets().collect();
        assert_eq!(
            ids,
            [
                (0xFFFE, Some(0)),
                (0, Some(100)),
                (2, None),
                (4, Some(u64::from(MAX_OFFSET) * TICK_US))
            ]
        );
        assert_eq!(report.next_id(), 6);
    }

    #[test]
    fn broken_reports_are_refused() {
        let good = TransportFeedback {
            stride: 1,
            entries: vec![0; 3],
            ..Default::default()
        };
        let bytes = good.encode().unwrap();
        assert!(TransportFeedback::decode(&bytes[..bytes.len() - 1]).is_none());
        let mut version0 = bytes.clone();
        version0[0] = 0;
        assert!(TransportFeedback::decode(&version0).is_none());
        let mut stride0 = bytes.clone();
        stride0[6] = 0;
        assert!(TransportFeedback::decode(&stride0).is_none());
        let mut wide = bytes.clone();
        wide[6] = (MAX_STRIDE + 1) as u8;
        assert!(TransportFeedback::decode(&wide).is_none());
        let mut empty = bytes.clone();
        empty[2] = 0;
        assert!(TransportFeedback::decode(&empty).is_none());
        let too_many = TransportFeedback {
            stride: 1,
            entries: vec![0; MAX_ENTRIES + 1],
            ..Default::default()
        };
        assert!(too_many.encode().is_none());
        assert!(TransportFeedback::decode(&[1; 19]).is_none());
    }

    #[test]
    fn the_largest_report_stays_below_one_enet_fragment() {
        assert!(HEADER_BYTES + 2 * MAX_ENTRIES <= 1000);
    }

    #[test]
    fn packet_ids_come_from_the_iv_or_the_rtp_sequence() {
        let mut datagram = [0u8; 16];
        datagram[0] = 0x34;
        datagram[1] = 0x12;
        datagram[2] = 0xAB;
        datagram[3] = 0xCD;
        assert_eq!(packet_id(&datagram, true), Some(0x1234));
        assert_eq!(packet_id(&datagram, false), Some(0xABCD));
        assert_eq!(packet_id(&datagram[..3], false), None);
        assert_eq!(packet_id(&datagram[..1], true), None);
    }

    #[test]
    fn a_report_covers_every_packet_of_its_interval() {
        let mut tally = FeedbackTally::default();
        let mut reports = Vec::new();
        // 100 packets 1 ms apart, id 10 lost and 11 before 12 swapped.
        for i in 0..100u16 {
            let id = match i {
                11 => 12,
                12 => 11,
                _ => i,
            };
            if id == 10 {
                continue;
            }
            if let Some(report) = tally.packet(id, 1_000 + u64::from(i) * 1_000) {
                reports.push(report);
            }
        }
        // The first report goes once 50 ms passed since its first packet.
        assert_eq!(reports.len(), 1);
        let first = &reports[0];
        assert_eq!((first.base, first.stride, first.seq), (0, 1, 0));
        assert_eq!(first.base_arrival_us, 1_000);
        assert_eq!(first.entries.len(), 51);
        assert_eq!(first.entries[0], 0);
        assert_eq!(first.entries[10], NOT_RECEIVED);
        assert_eq!(first.entries[11], 12_000 / TICK_US as u16);
        assert_eq!(first.entries[12], 11_000 / TICK_US as u16);
        // The next report continues at the end of the first.
        let second = tally.flush(200_000).unwrap();
        assert_eq!(second.base, first.next_id());
        assert_eq!(second.seq, 1);
        assert_eq!(second.flags, 0);
        assert_eq!(second.entries.len(), 49);
        assert!(second.entries.iter().all(|&e| e != NOT_RECEIVED));
    }

    #[test]
    fn a_packet_after_its_report_counts_late() {
        let mut tally = FeedbackTally::default();
        for id in 0..10u16 {
            if id != 5 {
                tally.packet(id, u64::from(id) * 100);
            }
        }
        let report = tally.flush(2_000).unwrap();
        assert_eq!(report.entries[5], NOT_RECEIVED);
        tally.packet(5, 3_000);
        tally.packet(10, 3_100);
        let next = tally.flush(4_000).unwrap();
        assert_eq!(next.late, 1);
        assert_eq!(next.base, 10);
    }

    #[test]
    fn a_full_report_goes_at_once_and_the_stride_follows_the_rate() {
        let mut tally = FeedbackTally::default();
        let mut reports = Vec::new();
        // 40 000 packets per second (25 µs apart) for a second.
        for i in 0..40_000u32 {
            if let Some(report) = tally.packet(i as u16, u64::from(i) * 25) {
                reports.push(report);
            }
        }
        assert!(reports.iter().all(|r| r.entries.len() <= MAX_ENTRIES));
        // 2 000 packets per 50 ms need a stride of 5 to fit 480 entries.
        let last = reports.last().unwrap();
        assert_eq!(last.stride, 5);
        // Reports chain without gaps and cover every sampled id.
        for pair in reports.windows(2) {
            assert_eq!(pair[1].base, pair[0].next_id(), "{pair:?}");
            assert_eq!(pair[1].flags, 0);
        }
        // Bounded: about 20 a second once the stride settles, and never one per packet.
        assert!(reports.len() < 60, "{}", reports.len());
        for report in &reports {
            assert!(report.entries.iter().all(|&e| e != NOT_RECEIVED));
        }
    }

    #[test]
    fn ids_wrap() {
        let mut tally = FeedbackTally::default();
        for (n, id) in (0xFFF0u16..=0xFFFF).chain(0..16).enumerate() {
            tally.packet(id, n as u64 * 10);
        }
        let report = tally.flush(10_000).unwrap();
        assert_eq!(report.base, 0xFFF0);
        assert_eq!(report.entries.len(), 32);
        assert_eq!(report.next_id(), 16);
    }

    #[test]
    fn a_short_gap_reports_missing_packets_and_a_long_one_skips() {
        let mut tally = FeedbackTally::default();
        for id in 0..10u16 {
            tally.packet(id, u64::from(id));
        }
        // 100 ids lost: inside the open report, each one reported missing.
        assert!(tally.packet(110, 100).is_none());
        let report = tally.flush(200).unwrap();
        assert_eq!(report.entries.len(), 111);
        let missing = report
            .entries
            .iter()
            .filter(|&&e| e == NOT_RECEIVED)
            .count();
        assert_eq!(missing, 100);
        assert_eq!(report.flags, 0);
        // 600 lost: past what a report holds, so the next one skips ahead and says so.
        assert!(tally.packet(111, 250).is_none());
        let report = tally.packet(711, 300).unwrap();
        assert_eq!((report.base, report.entries.len()), (111, 1));
        let next = tally.flush(400).unwrap();
        assert_eq!(next.base, 711);
        assert_eq!(next.flags & FLAG_SKIPPED, FLAG_SKIPPED);
        // A jump of 30 000 ids is the same on the wire; the host tells them apart by size.
        let mut tally = FeedbackTally::default();
        tally.packet(0, 0);
        let report = tally.packet(30_000, 10).unwrap();
        assert_eq!(report.entries.len(), 1);
        let next = tally.flush(20).unwrap();
        assert_eq!(next.base, 30_000);
        assert_eq!(next.flags & FLAG_SKIPPED, FLAG_SKIPPED);
    }
}
