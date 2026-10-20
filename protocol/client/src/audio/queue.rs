//! The RTP audio queue: reorders 4-packet FEC blocks, recovers up to two lost packets per
//! block from its parity packets, and releases packets in sequence, with gaps once a block is
//! given up. Port of `RtpAudioQueue.c` with Nightjar's patch (the out-of-sequence wait is in
//! microseconds).

use super::AudioStats;
use crate::fec::{ReedSolomon, AUDIO_PARITY_MATRIX};

/// How long to wait for out-of-sequence shards after a block should have arrived.
const RTPQ_OOS_WAIT_TIME_MS: u64 = 10;
const DATA_SHARDS: usize = 4;
const FEC_SHARDS: usize = 2;
const TOTAL_SHARDS: usize = DATA_SHARDS + FEC_SHARDS;
const CACHED_FEC_BLOCK_LIMIT: usize = 4;
const RTP_HEADER: usize = 12;
const FEC_HEADER: usize = 12;

pub(super) const RTP_PAYLOAD_TYPE_AUDIO: u8 = 97;
const RTP_PAYLOAD_TYPE_FEC: u8 = 127;

/// What the receiver should do after [`AudioQueue::add_packet`].
#[derive(Debug, PartialEq, Eq)]
pub(super) enum AddResult {
    /// Dropped, duplicated or waiting for earlier packets.
    Queued,
    /// The packet is the next in sequence: play it now.
    HandleNow,
    /// Packets are ready in [`AudioQueue::next_packet`].
    Ready,
}

/// A packet released by the queue.
pub(super) enum Queued {
    /// The next packet, with this sequence number.
    Packet { seq: u16 },
    /// The next packet was lost and could not be recovered.
    Lost,
}

/// The RTP header fields the queue uses, in host order.
pub(super) struct RtpHeader {
    pub packet_type: u8,
    pub seq: u16,
    pub timestamp: u32,
    pub ssrc: u32,
}

impl RtpHeader {
    pub(super) fn parse(packet: &[u8]) -> Self {
        Self {
            packet_type: packet[1],
            seq: u16::from_be_bytes([packet[2], packet[3]]),
            timestamp: u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]),
            ssrc: u32::from_be_bytes([packet[8], packet[9], packet[10], packet[11]]),
        }
    }
}

struct FecBlock {
    /// Four data packets (header and payload) then two parity shards.
    storage: Box<[u8]>,
    marks: [bool; TOTAL_SHARDS],
    payload_type: u8,
    base_seq: u16,
    base_timestamp: u32,
    ssrc: u32,
    queue_time_us: u64,
    data_received: u8,
    fec_received: u8,
    fully_reassembled: bool,
    next_data_index: u8,
    allow_discontinuity: bool,
    block_size: u16,
}

impl FecBlock {
    fn data_packet_size(&self) -> usize {
        RTP_HEADER + self.block_size as usize
    }

    fn data_packet(&self, index: usize) -> &[u8] {
        let size = self.data_packet_size();
        &self.storage[index * size..(index + 1) * size]
    }

    fn data_packet_mut(&mut self, index: usize) -> &mut [u8] {
        let size = self.data_packet_size();
        &mut self.storage[index * size..(index + 1) * size]
    }

    fn fec_shard_mut(&mut self, index: usize) -> &mut [u8] {
        let start = DATA_SHARDS * self.data_packet_size() + index * self.block_size as usize;
        &mut self.storage[start..start + self.block_size as usize]
    }

    /// The six RS shards: data payloads after their RTP headers, then the parity shards.
    fn shards(&mut self) -> Vec<&mut [u8]> {
        let size = self.data_packet_size();
        let (data, fec) = self.storage.split_at_mut(DATA_SHARDS * size);
        let mut shards: Vec<&mut [u8]> = data
            .chunks_exact_mut(size)
            .map(|p| &mut p[RTP_HEADER..])
            .collect();
        shards.extend(
            fec.chunks_exact_mut(self.block_size.max(1) as usize)
                .take(FEC_SHARDS),
        );
        shards
    }
}

fn is_before16(x: u16, y: u16) -> bool {
    x.wrapping_sub(y) > 0x7FFF
}

pub(super) struct AudioQueue {
    /// Blocks in sequence order.
    blocks: Vec<FecBlock>,
    free_blocks: Vec<FecBlock>,
    rs: ReedSolomon,
    next_rtp_seq: u16,
    oldest_rtp_base_seq: u16,
    last_oos_seq: u16,
    received_oos_data: bool,
    synchronizing: bool,
    incompatible_server: bool,
    packet_duration: u32,
    pub(super) stats: AudioStats,
}

impl AudioQueue {
    pub(super) fn new(packet_duration_ms: u32, app_version: [i32; 4]) -> Self {
        let v = app_version;
        // GFE before 3.19 breaks invariants the FEC logic needs; pass its audio straight through.
        let modern = v[0] > 7 || (v[0] == 7 && (v[1] > 1 || (v[1] == 1 && v[2] >= 415)));
        Self {
            blocks: Vec::new(),
            free_blocks: Vec::new(),
            rs: ReedSolomon::with_parity_matrix(DATA_SHARDS, FEC_SHARDS, &AUDIO_PARITY_MATRIX)
                .expect("4+2 code"),
            next_rtp_seq: 0,
            oldest_rtp_base_seq: 0,
            last_oos_seq: 0,
            received_oos_data: false,
            synchronizing: true,
            incompatible_server: !modern,
            packet_duration: packet_duration_ms,
            stats: AudioStats::default(),
        }
    }

    fn allocate_block(&mut self, block_size: u16) -> FecBlock {
        if let Some(block) = self.free_blocks.pop() {
            if block.block_size == block_size {
                return block;
            }
        }
        let size =
            DATA_SHARDS * (RTP_HEADER + block_size as usize) + FEC_SHARDS * block_size as usize;
        FecBlock {
            storage: vec![0; size].into_boxed_slice(),
            marks: [true; TOTAL_SHARDS],
            payload_type: 0,
            base_seq: 0,
            base_timestamp: 0,
            ssrc: 0,
            queue_time_us: 0,
            data_received: 0,
            fec_received: 0,
            fully_reassembled: false,
            next_data_index: 0,
            allow_discontinuity: false,
            block_size,
        }
    }

    fn free_block_head(&mut self) {
        let head = self.blocks.remove(0);
        self.oldest_rtp_base_seq = head.base_seq.wrapping_add(DATA_SHARDS as u16);
        // Completing a block, recovered or not, means we are synchronized with the host.
        self.synchronizing = false;
        if self.free_blocks.len() < CACHED_FEC_BLOCK_LIMIT {
            self.free_blocks.push(head);
        }
    }

    /// Finds or creates the block a packet belongs to; `None` rejects the packet.
    fn block_for_packet(
        &mut self,
        header: &RtpHeader,
        packet: &[u8],
        now_us: u64,
    ) -> Option<usize> {
        let length = packet.len();
        let (payload_type, base_seq, base_timestamp, ssrc, block_size);
        if header.packet_type == RTP_PAYLOAD_TYPE_AUDIO {
            if length < RTP_HEADER {
                self.stats.packet_count_invalid += 1;
                return None;
            }
            self.stats.packet_count_audio += 1;

            // Recent out-of-sequence packets make us give up on blocks less eagerly.
            if !self.synchronizing && is_before16(header.seq, self.oldest_rtp_base_seq) {
                self.last_oos_seq = header.seq;
                self.stats.packet_count_oos += 1;
                self.received_oos_data = true;
            } else if self.received_oos_data
                && is_before16(self.oldest_rtp_base_seq, self.last_oos_seq)
            {
                // More than 32767 packets without reordering.
                self.received_oos_data = false;
            }

            payload_type = header.packet_type;
            base_seq = (header.seq / DATA_SHARDS as u16) * DATA_SHARDS as u16;
            base_timestamp = header
                .timestamp
                .wrapping_sub(u32::from(header.seq - base_seq).wrapping_mul(self.packet_duration));
            ssrc = header.ssrc;
            block_size = (length - RTP_HEADER) as u16;
        } else if header.packet_type == RTP_PAYLOAD_TYPE_FEC {
            if length < RTP_HEADER + FEC_HEADER {
                self.stats.packet_count_fec_invalid += 1;
                return None;
            }
            self.stats.packet_count_fec += 1;

            let fec = &packet[RTP_HEADER..];
            payload_type = fec[1];
            base_seq = u16::from_be_bytes([fec[2], fec[3]]);
            base_timestamp = u32::from_be_bytes([fec[4], fec[5], fec[6], fec[7]]);
            ssrc = u32::from_be_bytes([fec[8], fec[9], fec[10], fec[11]]);

            if fec[0] as usize >= FEC_SHARDS {
                self.stats.packet_count_fec_invalid += 1;
                return None;
            }
            if base_seq % DATA_SHARDS as u16 != 0 {
                // Blocks must start on a 4-packet boundary (not so on GFE 3.13).
                self.stats.packet_count_fec_invalid += 1;
                self.incompatible_server = true;
                return None;
            }
            block_size = (length - RTP_HEADER - FEC_HEADER) as u16;
        } else {
            return None;
        }

        // Start at the next block boundary, so a stream joined mid-block reports no loss.
        if self.synchronizing && self.oldest_rtp_base_seq == 0 {
            self.oldest_rtp_base_seq = base_seq.wrapping_add(DATA_SHARDS as u16);
            self.next_rtp_seq = self.oldest_rtp_base_seq;
            return None;
        }

        if is_before16(base_seq, self.oldest_rtp_base_seq) {
            return None;
        }

        let mut position = self.blocks.len();
        for (index, block) in self.blocks.iter().enumerate() {
            if block.base_seq == base_seq {
                if block.block_size != block_size {
                    // Hosts with variable packet sizes cannot use this FEC scheme.
                    self.stats.packet_count_fec_invalid += 1;
                    self.incompatible_server = true;
                    return None;
                }
                return if block.fully_reassembled {
                    None
                } else {
                    Some(index)
                };
            } else if is_before16(base_seq, block.base_seq) {
                position = index;
                break;
            }
        }

        let mut block = self.allocate_block(block_size);
        block.marks = [true; TOTAL_SHARDS];
        block.payload_type = payload_type;
        block.base_seq = base_seq;
        block.base_timestamp = base_timestamp;
        block.ssrc = ssrc;
        block.queue_time_us = now_us;
        block.data_received = 0;
        block.fec_received = 0;
        block.fully_reassembled = false;
        block.next_data_index = 0;
        block.allow_discontinuity = false;
        self.blocks.insert(position, block);
        Some(position)
    }

    fn complete_fec_block(&mut self, index: usize) -> bool {
        let block = &mut self.blocks[index];
        if usize::from(block.data_received + block.fec_received) < DATA_SHARDS {
            return false;
        }
        if usize::from(block.data_received) == DATA_SHARDS {
            return true;
        }
        let marks = block.marks;
        let len = block.block_size as usize;
        {
            let mut shards = block.shards();
            if self.rs.decode(&mut shards, &marks, len).is_err() {
                return false;
            }
        }
        let duration = self.packet_duration;
        for i in 0..DATA_SHARDS {
            if block.marks[i] {
                let (payload_type, seq, ts, ssrc) = (
                    block.payload_type,
                    block.base_seq.wrapping_add(i as u16),
                    block
                        .base_timestamp
                        .wrapping_add((i as u32).wrapping_mul(duration)),
                    block.ssrc,
                );
                let packet = block.data_packet_mut(i);
                packet[0] = 0x80;
                packet[1] = payload_type;
                packet[2..4].copy_from_slice(&seq.to_be_bytes());
                packet[4..8].copy_from_slice(&ts.to_be_bytes());
                packet[8..12].copy_from_slice(&ssrc.to_be_bytes());
                block.marks[i] = false;
            }
        }
        let recovered = DATA_SHARDS as u32 - u32::from(block.data_received);
        self.stats.packet_count_fec_recovered += recovered;
        true
    }

    fn has_packet_ready(&self) -> bool {
        self.blocks.first().is_some_and(|head| {
            let index = head.next_data_index as usize;
            (!head.marks[index] && head.base_seq.wrapping_add(index as u16) == self.next_rtp_seq)
                || head.allow_discontinuity
        })
    }

    fn handle_missing_packets(&mut self, now_us: u64) {
        let Some(head) = self.blocks.first() else {
            return;
        };
        // A whole block before the oldest one we have was lost: resynchronize on it.
        if is_before16(self.next_rtp_seq, head.base_seq) {
            self.next_rtp_seq = head.base_seq;
            self.oldest_rtp_base_seq = head.base_seq;
            return;
        }
        // Wait for a second block before giving up on the first.
        if self.blocks.len() == 1 {
            return;
        }
        let wait_us = u64::from(self.packet_duration) * DATA_SHARDS as u64 * 1000
            + RTPQ_OOS_WAIT_TIME_MS * 1000;
        if !self.received_oos_data || now_us.wrapping_sub(head.queue_time_us) > wait_us {
            self.stats.packet_count_fec_failed += 1;
            self.blocks[0].allow_discontinuity = true;
        }
    }

    /// `RtpaAddPacket`: `packet` is the whole datagram (at most 1400 bytes).
    pub(super) fn add_packet(
        &mut self,
        header: &RtpHeader,
        packet: &[u8],
        now_us: u64,
    ) -> AddResult {
        if self.incompatible_server {
            return if header.packet_type == RTP_PAYLOAD_TYPE_AUDIO {
                AddResult::HandleNow
            } else {
                AddResult::Queued
            };
        }

        let Some(index) = self.block_for_packet(header, packet, now_us) else {
            return AddResult::Queued;
        };

        if header.packet_type == RTP_PAYLOAD_TYPE_AUDIO {
            let block = &mut self.blocks[index];
            let pos = header.seq.wrapping_sub(block.base_seq) as usize;
            if !block.marks[pos] {
                return AddResult::Queued;
            }
            block.data_packet_mut(pos).copy_from_slice(packet);
            block.marks[pos] = false;
            block.data_received += 1;

            // The common case: the next packet in order is played right away.
            if header.seq == self.next_rtp_seq {
                self.next_rtp_seq = header.seq.wrapping_add(1);
                block.next_data_index += 1;
                if self.next_rtp_seq == block.base_seq.wrapping_add(DATA_SHARDS as u16) {
                    self.free_block_head();
                }
                return AddResult::HandleNow;
            }
        } else {
            let shard = packet[RTP_HEADER] as usize;
            let block = &mut self.blocks[index];
            if !block.marks[DATA_SHARDS + shard] {
                return AddResult::Queued;
            }
            block
                .fec_shard_mut(shard)
                .copy_from_slice(&packet[RTP_HEADER + FEC_HEADER..]);
            block.marks[DATA_SHARDS + shard] = false;
            block.fec_received += 1;
        }

        if self.complete_fec_block(index) {
            self.blocks[index].fully_reassembled = true;
        }

        if !self.has_packet_ready() {
            self.handle_missing_packets(now_us);
        }

        if self.has_packet_ready() {
            AddResult::Ready
        } else {
            AddResult::Queued
        }
    }

    /// `RtpaGetQueuedPacket`: the next packet to play, copied to `out` (RTP header and
    /// payload), or `None` when waiting.
    pub(super) fn next_packet(&mut self, out: &mut Vec<u8>) -> Option<Queued> {
        if self
            .blocks
            .first()
            .is_some_and(|head| head.allow_discontinuity)
        {
            let head = &mut self.blocks[0];
            let lost = head.marks[head.next_data_index as usize];
            if lost {
                // Conceal this packet.
                head.next_data_index += 1;
                self.next_rtp_seq = self.next_rtp_seq.wrapping_add(1);
            }
            if usize::from(self.blocks[0].next_data_index) == DATA_SHARDS {
                self.free_block_head();
            }
            if lost {
                return Some(Queued::Lost);
            }
        }

        if self.has_packet_ready() {
            let head = &mut self.blocks[0];
            let index = head.next_data_index as usize;
            out.clear();
            out.extend_from_slice(head.data_packet(index));
            head.next_data_index += 1;
            let seq = head.base_seq.wrapping_add(index as u16);
            let done = usize::from(head.next_data_index) == DATA_SHARDS;
            self.next_rtp_seq = self.next_rtp_seq.wrapping_add(1);
            if done {
                self.free_block_head();
            }
            return Some(Queued::Packet { seq });
        }
        None
    }
}
