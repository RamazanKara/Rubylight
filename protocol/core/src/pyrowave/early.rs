//! Early record push for a decoder that keeps the records itself (the PyroWave SDK): each record
//! of a records frame goes to it as soon as its packets have arrived, so when the frame completes
//! only the sequence header and the last records are left to push before the decode.
//!
//! The packets come as the session's early chunks (`VideoDecoder::chunk`), the frame itself
//! afterwards. The frame decides: the early packets are used only if they add up to its envelope
//! in length, begin with its first packet byte for byte, and every one of them was taken.
//! Otherwise the caller clears its decoder and pushes the frame whole, as without early push.
//! Records decode in any order, so the push order does not change the picture.

use alloc::vec::Vec;

use super::{le32, Records, HEADER_BIT};

/// What [`EarlyPush::finish`] did with a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finish {
    /// None of the frame's packets came early: push the frame whole.
    Whole,
    /// Its packets came early but are not the frame (one missed, refused or unusable): clear the
    /// decoder and push the frame whole.
    Fallback,
    /// The header and the records not pushed yet went to the decoder: the frame is complete.
    Done,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Off,
    Active,
    Failed,
}

/// Frames finished each way since the state was created.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EarlyStats {
    /// Frames whose records went to the decoder while they arrived.
    pub done: u64,
    /// Frames whose early packets were not the frame, pushed whole instead.
    pub fallback: u64,
}

/// The frame in progress of [`EarlyPush::chunk`].
#[derive(Debug, Default)]
pub struct EarlyPush {
    state: State,
    /// Packets came for a frame whose first packet did not.
    missed_start: bool,
    /// Envelope bytes of the packets taken, the container's leading zero word included.
    bytes: usize,
    /// The frame's first packet as the envelope has it (length, flags, payload): an earlier frame
    /// of the same length whose finish never came is not the frame.
    first_packet: Vec<u8>,
    /// The records already pushed, one bit per block index.
    pushed: Vec<u64>,
    pub stats: EarlyStats,
}

impl EarlyPush {
    /// One packet of the next frame's envelope: its `PACKET_*` flags and payload. `first` starts
    /// a frame, and the caller clears its decoder before. Each record the packet completes goes
    /// to `push`; a refused one, or a packet that does not parse, leaves the frame to the whole
    /// push.
    pub fn chunk(
        &mut self,
        records: &mut Records,
        first: bool,
        flags: u32,
        payload: &[u8],
        mut push: impl FnMut(&[u8]) -> bool,
    ) {
        if first {
            records.begin_frame();
            self.pushed.clear();
            self.state = State::Active;
            self.bytes = 4;
            self.missed_start = false;
            self.first_packet.clear();
            self.first_packet
                .extend_from_slice(&(payload.len() as u32).to_le_bytes());
            self.first_packet.extend_from_slice(&flags.to_le_bytes());
            self.first_packet.extend_from_slice(payload);
        } else if self.state == State::Off {
            self.missed_start = true;
            return;
        }
        if self.state != State::Active {
            return;
        }
        self.bytes += 8 + payload.len();
        if !records.feed_packet(flags, payload) {
            self.state = State::Failed;
            return;
        }
        let mut failed = false;
        let pushed = &mut self.pushed;
        records.drain_new(|index, record| {
            if failed {
                return;
            }
            if !push(record) {
                failed = true;
                return;
            }
            let word = index as usize / 64;
            if word >= pushed.len() {
                pushed.resize(word + 1, 0);
            }
            pushed[word] |= 1 << (index % 64);
        });
        if failed {
            self.state = State::Failed;
        }
    }

    /// Packets in the envelope layout (per packet its byte length, its flags and its payload), as
    /// [`chunk`](Self::chunk) takes them one at a time; `first` is the first packet's. A batch
    /// that does not parse fails the frame in progress.
    pub fn chunks(
        &mut self,
        records: &mut Records,
        data: &[u8],
        first: bool,
        mut push: impl FnMut(&[u8]) -> bool,
    ) {
        let (mut offset, mut first) = (0, first);
        while offset < data.len() {
            if data.len() - offset < 8 || le32(data, offset) as usize > data.len() - offset - 8 {
                // Packets of the frame were lost here, its first one perhaps among them.
                if self.state == State::Active {
                    self.state = State::Failed;
                }
                self.missed_start = true;
                return;
            }
            let size = le32(data, offset) as usize;
            let flags = le32(data, offset + 4);
            offset += 8;
            self.chunk(
                records,
                first,
                flags,
                &data[offset..offset + size],
                &mut push,
            );
            first = false;
            offset += size;
        }
    }

    /// Ends the frame whose envelope (the container) is `data`. With [`Finish::Done`] the header
    /// and the records not pushed early (the last ones, and those `records` replays for lost ones)
    /// went to `push`, and `records` holds the frame's loss.
    pub fn finish(
        &mut self,
        records: &mut Records,
        data: &[u8],
        mut push: impl FnMut(&[u8]) -> bool,
    ) -> Finish {
        let state = core::mem::take(&mut self.state);
        let missed_start = core::mem::take(&mut self.missed_start);
        let first_packet = data.get(4..4 + self.first_packet.len());
        let done = state == State::Active
            && self.bytes == data.len()
            && first_packet == Some(&self.first_packet[..])
            && {
                let pushed = &self.pushed;
                records.finish_frame(|packet| {
                    let index = le32(packet, 4) >> 8;
                    let early = le32(packet, 0) & HEADER_BIT == 0
                        && pushed
                            .get(index as usize / 64)
                            .is_some_and(|word| word & (1 << (index % 64)) != 0);
                    early || push(packet)
                })
            };
        if done {
            self.stats.done += 1;
            Finish::Done
        } else if state == State::Off && !missed_start {
            Finish::Whole
        } else {
            self.stats.fallback += 1;
            Finish::Fallback
        }
    }
}
