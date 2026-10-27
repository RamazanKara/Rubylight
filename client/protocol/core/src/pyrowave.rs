//! PyroWave frame framing on the client side: the packet container the client's depacketizer
//! builds, and the record framing that lets a frame with lost packets still decode.
//!
//! A PyroWave bitstream is a sequence header (8 bytes) followed by blocks. Each block starts
//! with a little-endian word: bit 31 marks a header, bits 28-30 carry the frame's sequence
//! number, bits 16-27 the block size in words. Record framing (Rubylight 2.0) splits the blocks
//! into independently decodable records and pads packets with `u32::MAX` words, so a lost
//! packet costs only the records it carried.

use alloc::vec::Vec;

pub mod rows;

/// The bitstream revision both sides must agree on (SDP `a=x-ss-pyrowave.bitstream:`).
pub const BITSTREAM_ID: &str = "186f0393";

/// Packet flags in the client's container, after the core removed the RTP and FEC headers.
pub const PACKET_CONTIGUOUS: u32 = 1;
pub const PACKET_RECORD_START: u32 = 2;
pub const PACKET_FRAME_START: u32 = 4;
pub const PACKET_FRAME_END: u32 = 8;
/// Largest payload of one video packet.
pub const MAX_PACKET_BYTES: usize = 1400;
/// Largest frame the client assembles.
pub const MAX_FRAME_BYTES: usize = 4092 * MAX_PACKET_BYTES;

const HEADER_BIT: u32 = 0x8000_0000;
const PADDING: u32 = u32::MAX;

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn write_le32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn block_sequence(word: u32) -> u32 {
    (word >> 28) & 7
}

fn with_sequence(byte3: u8, sequence: u32) -> u8 {
    (byte3 & 0x8f) | ((sequence as u8) << 4)
}

/// Splits an ordinary (pre-record) container into the decoder's packets.
///
/// Layout: a non-zero packet count, then per packet a byte length and that many bytes of whole
/// blocks. The whole container is validated before the first packet reaches `push`, so a bad
/// frame never changes the decoder's sequence state. Returns false if the container is
/// malformed or `push` refuses a packet.
pub fn push_container(data: &[u8], mut push: impl FnMut(&[u8]) -> bool) -> bool {
    if data.len() < 4 {
        return false;
    }
    let count = le32(data, 0) as usize;
    if count == 0 || count > (data.len() - 4) / 12 {
        return false;
    }
    let mut offset = 4;
    let mut sequence = None;
    for _ in 0..count {
        if data.len() - offset < 4 {
            return false;
        }
        let size = le32(data, offset) as usize;
        offset += 4;
        if size < 8 || size % 4 != 0 || size > data.len() - offset {
            return false;
        }
        // Reject short blocks before the decoder, including duplicated zero-length blocks.
        let mut at = 0;
        while at < size {
            if size - at < 8 {
                return false;
            }
            let word = le32(data, offset + at);
            let block = if word & HEADER_BIT != 0 { 8 } else { ((word >> 16) & 0xfff) as usize * 4 };
            if word == PADDING || block < 8 || block > size - at {
                return false;
            }
            if sequence.is_some_and(|s| s != block_sequence(word)) {
                return false;
            }
            sequence = Some(block_sequence(word));
            at += block;
        }
        offset += size;
    }
    if offset != data.len() {
        return false;
    }

    let mut offset = 4;
    for _ in 0..count {
        let size = le32(data, offset) as usize;
        offset += 4;
        if !push(&data[offset..offset + size]) {
            return false;
        }
        offset += size;
    }
    true
}

/// Reassembles record-framed PyroWave frames, carrying over records a frame lost.
///
/// The container is a zero word, then per received packet: its byte length, its flags
/// (`PACKET_*`) and its payload. The first packet of a frame starts with a 16-byte prefix
/// (version 1, the last packet's payload length, the 8-byte sequence header).
#[derive(Debug, Clone, Default)]
pub struct Records {
    have_previous: bool,
    previous_header: [u8; 8],
    previous: RecordStore,
    incoming: RecordStore,
    /// Share of this frame's records that were lost, 0-100; -1 without the record count,
    /// or 100 after a rejected frame.
    pub loss_percent: f32,
    /// The frame being fed packet by packet.
    scan: Scan,
}

/// How far the frame in progress has been parsed.
#[derive(Debug, Clone, Default)]
struct Scan {
    active: bool,
    packets: usize,
    /// Envelope bytes fed so far, 8 per packet header plus the payloads.
    envelope_bytes: usize,
    /// Where the open run of contiguous packets starts in `incoming.bytes`.
    run_start: usize,
    /// Bytes of `incoming.bytes` parsed into records; the rest is a record in progress.
    scanned: usize,
    /// Records already handed out by `drain_new`.
    reported: usize,
    header: [u8; 8],
    has_header: bool,
    partial: bool,
    ended: bool,
    collecting: bool,
    sequence: u32,
    last_length: usize,
}

/// Byte ranges avoid allocating and copying each individual record. The previous frame and
/// incoming scratch store swap only after a successful push, retaining their allocations.
#[derive(Debug, Clone, Default)]
struct RecordStore {
    bytes: Vec<u8>,
    records: Vec<RecordRange>,
}

#[derive(Debug, Clone, Copy)]
struct RecordRange {
    index: u32,
    offset: usize,
    size: usize,
}

impl Records {
    /// The negotiated geometry lets even the first frame survive a lost sequence-header packet.
    pub fn with_geometry(width: u32, height: u32, chroma444: bool) -> Self {
        let mut previous_header = [0; 8];
        write_le32(&mut previous_header, 0, HEADER_BIT | width.wrapping_sub(1) | (height.wrapping_sub(1) << 14));
        write_le32(&mut previous_header, 4, u32::from(chroma444) << 26);
        Self { have_previous: true, previous_header, ..Self::default() }
    }

    /// Feeds one frame's packets to `push`: the sequence header, received records by index,
    /// then recovered records by index. The recovery cache changes only after all pushes succeed.
    /// Records this frame lost are replayed from earlier frames of the same format, with this
    /// frame's sequence number. Returns false if the frame is unusable or `push` refuses one.
    ///
    /// The whole-frame form of [`begin_frame`](Self::begin_frame), [`feed_envelope`](Self::feed_envelope)
    /// and [`finish_frame`](Self::finish_frame), which a client uses to take records while the
    /// frame is still arriving.
    pub fn push_frame(&mut self, data: &[u8], push: impl FnMut(&[u8]) -> bool) -> bool {
        self.begin_frame();
        if data.len() < 4 || data.len() > MAX_FRAME_BYTES || le32(data, 0) != 0 {
            return false;
        }
        self.feed_envelope(&data[4..]) && self.finish_frame(push)
    }

    /// Starts a frame whose packets arrive one at a time. Drops what an abandoned frame had
    /// collected; the recovery cache is untouched.
    pub fn begin_frame(&mut self) {
        self.loss_percent = 100.0;
        self.incoming.bytes.clear();
        self.incoming.records.clear();
        self.scan = Scan { sequence: u32::MAX, active: true, ..Scan::default() };
    }

    /// Feeds whole envelope packets (length, flags, payload; the container's leading zero word
    /// excluded) of the frame begun with [`begin_frame`](Self::begin_frame). Returns false if
    /// the frame is unusable; the caller then ends it without `finish_frame`.
    pub fn feed_envelope(&mut self, data: &[u8]) -> bool {
        let mut offset = 0;
        while offset < data.len() {
            if data.len() - offset < 8 {
                return false;
            }
            let size = le32(data, offset) as usize;
            let flags = le32(data, offset + 4);
            offset += 8;
            if size > data.len() - offset || !self.feed_packet(flags, &data[offset..offset + size]) {
                return false;
            }
            offset += size;
        }
        true
    }

    /// Feeds one packet (`PACKET_*` flags and payload) of the frame in progress.
    pub fn feed_packet(&mut self, flags: u32, payload: &[u8]) -> bool {
        let scan = &mut self.scan;
        let mut size = payload.len();
        scan.envelope_bytes += 8 + size;
        if !scan.active || scan.ended || 4 + scan.envelope_bytes > MAX_FRAME_BYTES
            || size == 0 || size > MAX_PACKET_BYTES || flags & !15 != 0 {
            return false;
        }
        let mut start = 0;
        if flags & PACKET_CONTIGUOUS == 0 {
            if !self.scan_run(true) {
                return false;
            }
            let scan = &mut self.scan;
            scan.run_start = self.incoming.bytes.len();
            scan.scanned = scan.run_start;
            scan.collecting = flags & (PACKET_RECORD_START | PACKET_FRAME_START) != 0;
            if flags & PACKET_FRAME_START == 0 {
                scan.partial = true;
            }
        }
        let scan = &mut self.scan;
        if flags & PACKET_FRAME_START != 0 {
            if scan.has_header || scan.packets != 0 || size < 16 || payload[0] != 1 {
                return false;
            }
            scan.last_length = usize::from(payload[4]) | (usize::from(payload[5]) << 8);
            if scan.last_length == 0 || scan.last_length > MAX_PACKET_BYTES {
                return false;
            }
            scan.header.copy_from_slice(&payload[8..16]);
            let word = le32(&scan.header, 0);
            if word & HEADER_BIT == 0 || word == PADDING || (le32(&scan.header, 4) >> 24) & 3 != 0 {
                return false;
            }
            scan.sequence = block_sequence(word);
            scan.has_header = true;
        }
        scan.packets += 1;
        if flags & PACKET_RECORD_START != 0 {
            scan.collecting = true;
        }
        if flags & PACKET_FRAME_END != 0 {
            scan.ended = true;
            if scan.has_header {
                if scan.last_length > size {
                    return false;
                }
                size = scan.last_length;
            }
        }
        if flags & PACKET_FRAME_START != 0 {
            if size < 16 {
                return false;
            }
            start = 16;
            size -= 16;
        }
        if scan.collecting {
            self.incoming.bytes.extend_from_slice(&payload[start..start + size]);
        }
        self.scan_run(true)
    }

    /// Hands `f` the records completed since the last call, as (index, bytes): the frame's
    /// records in arrival order, for a decoder that works while the frame is still arriving.
    /// Every record given is one `finish_frame` pushes if it succeeds, so decoding them early
    /// is safe; when it fails the frame is dropped whole. Call before `finish_frame`.
    pub fn drain_new(&mut self, mut f: impl FnMut(u32, &[u8])) {
        for record in &self.incoming.records[self.scan.reported..] {
            f(record.index, &self.incoming.bytes[record.offset..record.offset + record.size]);
        }
        self.scan.reported = self.incoming.records.len();
    }

    /// Parses the records the open run has completed since the last scan. A record cut off at
    /// the end waits for more bytes while `interrupted`, else it makes the frame unusable.
    fn scan_run(&mut self, interrupted: bool) -> bool {
        let scan = &mut self.scan;
        let Some(used) = parse_run(&self.incoming.bytes[scan.scanned..], scan.scanned, interrupted,
            scan.has_header, &mut scan.sequence, &mut self.incoming.records) else { return false };
        scan.scanned += used;
        true
    }

    /// Completes the frame: checks it, pushes the sequence header and every record (received,
    /// then recovered from earlier frames) to `push`, and commits the recovery cache.
    pub fn finish_frame(&mut self, mut push: impl FnMut(&[u8]) -> bool) -> bool {
        if !self.scan.active {
            return false;
        }
        self.scan.active = false;
        let Scan { has_header, mut partial, ended, mut header, .. } = self.scan;
        partial |= !ended || !has_header;
        if !self.scan_run(!ended) || self.scan.sequence == u32::MAX {
            return false;
        }
        let sequence = self.scan.sequence;
        self.incoming.records.sort_unstable_by_key(|record| record.index);
        if self.incoming.records.windows(2).any(|pair| pair[0].index == pair[1].index) {
            return false;
        }
        let received_count = self.incoming.records.len();
        if !has_header {
            if !self.have_previous {
                return false;
            }
            header = self.previous_header;
            header[3] = with_sequence(header[3], sequence);
        }
        let count = (le32(&header, 4) & 0xff_ffff) as usize;
        if has_header && (received_count > count || (!partial && received_count != count)) {
            return false;
        }
        if received_count == 0 && count != 0 {
            return false;
        }
        let loss = if !has_header {
            // The header carries this frame's record count, which may differ from the last.
            -1.0
        } else if count != 0 {
            100.0 * (count - received_count) as f32 / count as f32
        } else {
            0.0
        };

        let same_format = self.have_previous
            && le32(&self.previous_header, 0) & !0x7000_0000 == le32(&header, 0) & !0x7000_0000
            && self.previous_header[7] & 0xfc == header[7] & 0xfc;
        let retain = partial && same_format;
        if retain {
            // Both lists are sorted. Append missing records after the received records to
            // preserve callback order, without changing the cache before all pushes succeed.
            let mut received_at = 0;
            for previous in &self.previous.records {
                while received_at < received_count
                    && self.incoming.records[received_at].index < previous.index {
                    received_at += 1;
                }
                if received_at < received_count
                    && self.incoming.records[received_at].index == previous.index {
                    continue;
                }
                let offset = self.incoming.bytes.len();
                self.incoming.bytes.extend_from_slice(
                    &self.previous.bytes[previous.offset..previous.offset + previous.size]);
                self.incoming.bytes[offset + 3] = with_sequence(self.incoming.bytes[offset + 3], sequence);
                self.incoming.records.push(RecordRange { offset, ..*previous });
            }
        }
        // The SDK zeros absent coefficients; replay cached records with this frame's sequence.
        let original_header = header;
        let format = le32(&header, 4) & 0xff00_0000;
        write_le32(&mut header, 4, format | self.incoming.records.len() as u32);
        if !push(&header) {
            return false;
        }
        for record in &self.incoming.records {
            if !push(&self.incoming.bytes[record.offset..record.offset + record.size]) {
                return false;
            }
        }
        if self.incoming.records.len() != received_count {
            // Keep the committed cache sorted for the next frame's linear recovery merge.
            self.incoming.records.sort_unstable_by_key(|record| record.index);
        }
        core::mem::swap(&mut self.previous, &mut self.incoming);
        self.previous_header = original_header;
        self.have_previous = true;
        self.loss_percent = loss;
        true
    }
}

/// Parses the records a run of contiguous packets completes, and returns the bytes used (a
/// record cut off at the end is left, or an error unless `interrupted`); `None` if malformed.
fn parse_run(
    run: &[u8],
    run_start: usize,
    interrupted: bool,
    has_header: bool,
    sequence: &mut u32,
    received: &mut Vec<RecordRange>,
) -> Option<usize> {
    let mut at = 0;
    while at < run.len() {
        if !has_header && run[at..].iter().all(|&byte| byte == 0) {
            return Some(at);
        }
        if run.len() - at < 8 {
            return interrupted.then_some(at);
        }
        let word = le32(run, at);
        let padding = word == PADDING;
        let size = if padding {
            8 + 4 * u64::from(le32(run, at + 4))
        } else {
            u64::from((word >> 16) & 0xfff) * 4
        };
        if size < 8 || (!padding && word & HEADER_BIT != 0) {
            return None;
        }
        if size > (run.len() - at) as u64 {
            return interrupted.then_some(at);
        }
        let size = size as usize;
        if !padding {
            let block = block_sequence(word);
            if *sequence != u32::MAX && *sequence != block {
                return None;
            }
            *sequence = block;
            let index = le32(run, at + 4) >> 8;
            // Duplicate indices across all runs are rejected together before any callback.
            received.push(RecordRange { index, offset: run_start + at, size });
        }
        at += size;
    }
    Some(at)
}

#[cfg(test)]
mod tests;
