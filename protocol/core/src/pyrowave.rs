//! PyroWave frame framing on the client side: the packet container the client's depacketizer
//! builds, and the record framing that lets a frame with lost packets still decode.
//!
//! A PyroWave bitstream is a sequence header (8 bytes) followed by blocks. Each block starts
//! with a little-endian word: bit 31 marks a header, bits 28-30 carry the frame's sequence
//! number, bits 16-27 the block size in words. Record framing (Rubylight 2.0) splits the blocks
//! into independently decodable records and pads packets with `u32::MAX` words, so a lost
//! packet costs only the records it carried.

use alloc::vec::Vec;

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
    pub fn push_frame(&mut self, data: &[u8], mut push: impl FnMut(&[u8]) -> bool) -> bool {
        self.loss_percent = 100.0;
        if data.len() < 4 || data.len() > MAX_FRAME_BYTES || le32(data, 0) != 0 {
            return false;
        }
        self.incoming.bytes.clear();
        self.incoming.records.clear();
        let mut run_start = 0;
        let mut header = [0u8; 8];
        let (mut has_header, mut partial, mut ended, mut collecting) = (false, false, false, false);
        let mut sequence = u32::MAX;
        let mut last_length = 0usize;

        let mut offset = 4;
        while offset < data.len() {
            if ended || data.len() - offset < 8 {
                return false;
            }
            let mut size = le32(data, offset) as usize;
            let flags = le32(data, offset + 4);
            offset += 8;
            if size == 0 || size > MAX_PACKET_BYTES || size > data.len() - offset || flags & !15 != 0 {
                return false;
            }
            let mut packet = offset;
            offset += size;
            if flags & PACKET_CONTIGUOUS == 0 {
                if !parse_run(&self.incoming.bytes[run_start..], run_start, true,
                    has_header, &mut sequence, &mut self.incoming.records) {
                    return false;
                }
                run_start = self.incoming.bytes.len();
                collecting = flags & (PACKET_RECORD_START | PACKET_FRAME_START) != 0;
                if flags & PACKET_FRAME_START == 0 {
                    partial = true;
                }
            }
            if flags & PACKET_FRAME_START != 0 {
                if has_header || offset != 12 + size || size < 16 || data[packet] != 1 {
                    return false;
                }
                last_length = usize::from(data[packet + 4]) | (usize::from(data[packet + 5]) << 8);
                if last_length == 0 || last_length > MAX_PACKET_BYTES {
                    return false;
                }
                header.copy_from_slice(&data[packet + 8..packet + 16]);
                let word = le32(&header, 0);
                if word & HEADER_BIT == 0 || word == PADDING || (le32(&header, 4) >> 24) & 3 != 0 {
                    return false;
                }
                sequence = block_sequence(word);
                has_header = true;
            }
            if flags & PACKET_RECORD_START != 0 {
                collecting = true;
            }
            if flags & PACKET_FRAME_END != 0 {
                ended = true;
                if has_header {
                    if last_length > size {
                        return false;
                    }
                    size = last_length;
                }
            }
            if flags & PACKET_FRAME_START != 0 {
                if size < 16 {
                    return false;
                }
                packet += 16;
                size -= 16;
            }
            if collecting {
                self.incoming.bytes.extend_from_slice(&data[packet..packet + size]);
            }
        }
        partial |= !ended || !has_header;
        if !parse_run(&self.incoming.bytes[run_start..], run_start, !ended,
            has_header, &mut sequence, &mut self.incoming.records) || sequence == u32::MAX {
            return false;
        }
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

/// Parses a run of contiguous packets into records. `interrupted` runs may end mid-record.
fn parse_run(
    run: &[u8],
    run_start: usize,
    interrupted: bool,
    has_header: bool,
    sequence: &mut u32,
    received: &mut Vec<RecordRange>,
) -> bool {
    let mut at = 0;
    while at < run.len() {
        if !has_header && run[at..].iter().all(|&byte| byte == 0) {
            return true;
        }
        if run.len() - at < 8 {
            return interrupted;
        }
        let word = le32(run, at);
        let padding = word == PADDING;
        let size = if padding {
            8 + 4 * u64::from(le32(run, at + 4))
        } else {
            u64::from((word >> 16) & 0xfff) * 4
        };
        if size < 8 || (!padding && word & HEADER_BIT != 0) {
            return false;
        }
        if size > (run.len() - at) as u64 {
            return interrupted;
        }
        let size = size as usize;
        if !padding {
            let block = block_sequence(word);
            if *sequence != u32::MAX && *sequence != block {
                return false;
            }
            *sequence = block;
            let index = le32(run, at + 4) >> 8;
            // Duplicate indices across all runs are rejected together before any callback.
            received.push(RecordRange { index, offset: run_start + at, size });
        }
        at += size;
    }
    true
}

#[cfg(test)]
mod tests;
