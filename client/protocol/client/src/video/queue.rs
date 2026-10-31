//! The RTP video queue: collects each FEC block of a frame, recovers missing data shards from
//! parity, and passes completed frames to the depacketizer in sequence order. Port of
//! `RtpVideoQueue.c` and Nightjar's `android_rtp_video.c` (PyroWave records keep the data
//! of incomplete blocks).

use super::depacketizer::Depacketizer;
use super::{
    is_before16, is_before32, le32, FecBlock, FecStatus, Io, Packet, VideoConfig, VideoSignal,
    VideoSink, FLAG_CONTAINS_PIC_DATA, FLAG_EOF, FLAG_SOF, MAX_RTP_HEADER_SIZE,
    NV_VIDEO_PACKET_SIZE,
};
use crate::fec::ReedSolomon;

/// Control message type of [`FecStatus`] reports.
pub const FEC_STATUS_TYPE: u16 = 0x5502;

/// No speculative RFI for 5 minutes after out-of-order data or a wrong loss prediction.
const SPECULATIVE_RFI_COOLDOWN_PERIOD_US: u64 = 300_000_000;

pub(super) struct RtpQueue {
    pending: Vec<Packet>,
    completed: Vec<Packet>,

    buffer_first_recv_time_us: u64,
    buffer_lowest_seq: u16,
    buffer_highest_seq: u16,
    buffer_first_parity_seq: u16,
    buffer_data_packets: u32,
    buffer_parity_packets: u32,
    received_data_packets: u32,
    received_parity_packets: u32,
    received_highest_seq: u16,
    fec_percentage: u32,
    next_contiguous_seq: u16,
    missing_packets: u32,
    use_fast_queue_path: bool,
    reported_lost_frame: bool,

    current_frame: u32,

    multi_fec_capable: bool,
    current_block: u8,
    last_block: u8,

    last_oos_frame_pts: u64,
    received_oos_data: bool,

    packet_size: usize,
    rfi: bool,
    fec_supported: bool,
    records: bool,
    /// Records go to the depacketizer as their packets arrive instead of when the frame
    /// completes ([`VideoConfig::early_chunks`]).
    early: bool,
    /// Data packets of the pending block already given to the depacketizer: the run from the
    /// block's first packet with no hole. They stay pending for FEC recovery.
    forwarded: u32,

    /// FEC recovery's code and scratch, kept so that recovering a block allocates nothing.
    fec: Option<ReedSolomon>,
    fec_missing: Vec<bool>,
    fec_shards: Vec<Box<[u8]>>,
}

impl RtpQueue {
    pub(super) fn new(config: &VideoConfig) -> Self {
        Self {
            pending: Vec::new(),
            completed: Vec::new(),
            buffer_first_recv_time_us: 0,
            buffer_lowest_seq: 0,
            buffer_highest_seq: 0,
            buffer_first_parity_seq: 0,
            buffer_data_packets: 0,
            buffer_parity_packets: 0,
            received_data_packets: 0,
            received_parity_packets: 0,
            received_highest_seq: 0,
            fec_percentage: 0,
            next_contiguous_seq: 0,
            missing_packets: 0,
            use_fast_queue_path: false,
            reported_lost_frame: false,
            current_frame: 1,
            multi_fec_capable: config.at_least(7, 1, 431),
            current_block: 0,
            last_block: 0,
            last_oos_frame_pts: 0,
            received_oos_data: false,
            packet_size: config.packet_size,
            rfi: config.rfi,
            fec_supported: config.app_version[0] >= 5,
            records: config.pyrowave_records,
            early: config.pyrowave_records && config.early_chunks,
            forwarded: 0,
            fec: None,
            fec_missing: Vec::new(),
            fec_shards: Vec::new(),
        }
    }

    pub(super) fn current_frame_number(&self) -> u32 {
        self.current_frame
    }

    fn purge_pending<S: VideoSink>(&mut self, io: &mut Io<S>) {
        for packet in self.pending.drain(..) {
            io.pools.recycle_packet(packet.buf);
        }
    }

    fn purge_completed<S: VideoSink>(&mut self, io: &mut Io<S>) {
        for packet in self.completed.drain(..) {
            io.pools.recycle_packet(packet.buf);
        }
    }

    /// `notifyOrdinaryFrameLost`: with PyroWave records a lost frame still decodes partially.
    fn notify_frame_lost<S: VideoSink>(
        &self,
        depacketizer: &mut Depacketizer,
        io: &mut Io<S>,
        frame: u32,
        speculative: bool,
    ) {
        if !self.records {
            depacketizer.notify_frame_lost(frame, speculative, io);
        }
    }

    fn report_fec_status<S: VideoSink>(&self, io: &mut Io<S>) {
        io.sink.signal(VideoSignal::FecStatus(FecStatus {
            frame_index: self.current_frame,
            highest_received_sequence: self.received_highest_seq,
            next_contiguous_sequence: self.next_contiguous_seq,
            missing_before_highest: self.missing_packets as u16,
            total_data_packets: self.buffer_data_packets as u16,
            total_parity_packets: self.buffer_parity_packets as u16,
            received_data_packets: self.received_data_packets as u16,
            received_parity_packets: self.received_parity_packets as u16,
            fec_percentage: self.fec_percentage as u8,
            block_index: self.current_block,
            block_count: self.last_block + 1,
        }));
    }

    /// Hands the pending block's counters to the FEC report as it finishes: `unrecovered` when
    /// the next block begins before this one completed (`FecTally::block` counts them).
    fn finish_fec_block<S: VideoSink>(&self, io: &mut Io<S>, unrecovered: bool) {
        io.sink.signal(VideoSignal::FecBlock(FecBlock {
            frame: self.current_frame,
            block_index: self.current_block,
            block_count: self.last_block + 1,
            data_shards: self.buffer_data_packets,
            parity_shards: self.buffer_parity_packets,
            arrived_shards: self.received_data_packets + self.received_parity_packets,
            missing_shards: self.missing_packets,
            unrecovered,
        }));
    }

    /// Adds a packet to the pending block; hands it back if it duplicates a queued one.
    fn queue_packet(
        &mut self,
        mut packet: Packet,
        is_parity: bool,
        is_fec_recovery: bool,
    ) -> Result<(), Packet> {
        let mut out_of_sequence = false;
        if self.use_fast_queue_path && packet.seq == self.next_contiguous_seq {
            self.next_contiguous_seq = packet.seq.wrapping_add(1);
        } else {
            for entry in &self.pending {
                if packet.seq == entry.seq {
                    return Err(packet);
                } else if is_before16(packet.seq.into(), entry.seq.into()) {
                    out_of_sequence = true;
                }
            }
            // An out-of-order packet leaves next_contiguous_seq behind, so stay on the slow
            // path for the rest of this block to catch duplicates.
            self.use_fast_queue_path = false;
        }

        packet.is_parity = is_parity;
        packet.presentation_time_us = u64::from(packet.timestamp) * 1000 / 90;
        packet.rtp_timestamp = packet.timestamp;

        // Recovered packets are ours, so they say nothing about reordering on the network.
        if !is_fec_recovery {
            if out_of_sequence {
                self.last_oos_frame_pts = packet.presentation_time_us;
                self.received_oos_data = true;
            } else if self.received_oos_data
                && packet.presentation_time_us
                    > self.last_oos_frame_pts + SPECULATIVE_RFI_COOLDOWN_PERIOD_US
            {
                self.received_oos_data = false;
            }
        }

        self.pending.push(packet);
        Ok(())
    }

    /// Returns 0 once the block is complete, recovering missing data shards when enough
    /// packets arrived.
    fn reconstruct_frame<S: VideoSink>(
        &mut self,
        depacketizer: &mut Depacketizer,
        io: &mut Io<S>,
    ) -> i32 {
        let total = self.buffer_data_packets + self.buffer_parity_packets;
        let needed = self.buffer_data_packets;

        if (self.pending.len() as u32) < needed {
            // Without recent reordering from this host, a block missing more packets than its
            // parity can only be lost, so report it before the next frame arrives.
            if self.rfi
                && !self.reported_lost_frame
                && !self.received_oos_data
                && self.missing_packets > total.wrapping_sub(needed)
            {
                self.notify_frame_lost(depacketizer, io, self.current_frame, true);
                self.reported_lost_frame = true;
            }
            return -1;
        }

        if self.reported_lost_frame && !self.received_oos_data {
            // The prediction was wrong; stop predicting for a while.
            self.received_oos_data = true;
            self.last_oos_frame_pts = self.pending[0].presentation_time_us;
        }

        if self.received_data_packets == self.buffer_data_packets {
            return 0;
        }

        if !self.fec_supported {
            return -1;
        }

        let (data, parity) = (
            self.buffer_data_packets as usize,
            self.buffer_parity_packets as usize,
        );
        let renewed = match &mut self.fec {
            Some(rs) => rs.renew(data, parity),
            fec @ None => {
                *fec = ReedSolomon::new(data, parity);
                fec.is_some()
            }
        };
        let Some(rs) = self.fec.as_ref().filter(|_| renewed) else {
            return -3;
        };

        let total = total as usize;
        let receive_size = self.packet_size + MAX_RTP_HEADER_SIZE;
        self.fec_missing.clear();
        self.fec_missing.resize(total, true);
        // Empty boxes hold no memory.
        self.fec_shards.clear();
        self.fec_shards.resize_with(total, Box::default);
        for packet in &mut self.pending {
            let index = packet.seq.wrapping_sub(self.buffer_lowest_seq) as usize;
            self.fec_missing[index] = false;
            packet.buf[packet.len..receive_size].fill(0);
            self.fec_shards[index] = std::mem::take(&mut packet.buf);
        }
        for (shard, &missing) in self.fec_shards.iter_mut().zip(&self.fec_missing) {
            if missing {
                *shard = io.pools.packet();
            }
        }

        let decoded = rs.decode(&mut self.fec_shards, &self.fec_missing, receive_size);
        let mut ret = if decoded.is_ok() { 0 } else { -1 };

        // Report the final FEC status of a block that needed recovery.
        self.report_fec_status(io);

        for packet in &mut self.pending {
            let index = packet.seq.wrapping_sub(self.buffer_lowest_seq) as usize;
            packet.buf = std::mem::take(&mut self.fec_shards[index]);
        }

        let head_header = self.pending[0].buf[0];
        let head_timestamp = self.pending[0].timestamp;
        let data_packets = self.buffer_data_packets as usize;
        for index in 0..total {
            if !self.fec_missing[index] {
                continue;
            }
            let buf = std::mem::take(&mut self.fec_shards[index]);
            if ret != 0 || index >= data_packets {
                io.pools.recycle_packet(buf);
                continue;
            }
            let mut recovered = Packet {
                buf,
                len: 0,
                seq: (index as u16).wrapping_add(self.buffer_lowest_seq),
                timestamp: head_timestamp,
                receive_time_us: 0,
                presentation_time_us: 0,
                rtp_timestamp: 0,
                is_parity: false,
            };
            recovered.buf[0] = head_header;
            let offset = recovered.data_offset();
            recovered.buf[offset + 4..offset + 8]
                .copy_from_slice(&self.current_frame.to_le_bytes());
            recovered.buf[offset + 11] = ((self.last_block << 2) | self.current_block) << 4;
            let flags = recovered.buf[offset + 8];
            // Reject recovered packets that cannot be part of this block.
            let corrupt = (index == 0 && flags & FLAG_SOF == 0)
                || (index == data_packets - 1 && flags & FLAG_EOF == 0)
                || (index > 0 && index < data_packets - 1 && flags & FLAG_CONTAINS_PIC_DATA == 0)
                || flags & !(FLAG_SOF | FLAG_EOF | FLAG_CONTAINS_PIC_DATA) != 0;
            if corrupt {
                ret = -1;
                io.pools.recycle_packet(recovered.buf);
                continue;
            }
            recovered.len = self.packet_size + offset;
            io.stats.packets_recovered += 1;
            if let Err(packet) = self.queue_packet(recovered, false, true) {
                io.pools.recycle_packet(packet.buf);
            }
        }
        ret
    }

    /// Gives the depacketizer the pending block's data packets that arrived in order and are
    /// not in a hole, so it can pass them on while the rest of the block is still in flight.
    /// `arrived` is the data packet just queued: only it can extend the run, so a packet behind
    /// a hole costs nothing, and the pending list is searched only while packets that arrived
    /// out of order are waiting for the hole to fill.
    fn forward_ready<S: VideoSink>(
        &mut self,
        arrived: u16,
        depacketizer: &mut Depacketizer,
        io: &mut Io<S>,
    ) {
        if arrived != self.buffer_lowest_seq.wrapping_add(self.forwarded as u16) {
            return;
        }
        // In-order arrival is the last one queued.
        let mut index = self.pending.len() - 1;
        loop {
            let packet = &mut self.pending[index];
            packet.receive_time_us = self.buffer_first_recv_time_us;
            depacketizer.queue_early_packet(packet, io);
            self.forwarded += 1;
            let waiting = self.pending.len() - self.received_parity_packets as usize;
            if waiting <= self.forwarded as usize {
                return;
            }
            let seq = self.buffer_lowest_seq.wrapping_add(self.forwarded as u16);
            let Some(next) = self.pending.iter().rposition(|p| !p.is_parity && p.seq == seq) else {
                return;
            };
            index = next;
        }
    }

    /// Moves the block's data packets to the completed list in sequence order and drops the
    /// parity packets. With early records the packets not yet forwarded go straight on to the
    /// depacketizer, so the envelope stays in packet order across blocks.
    fn stage_complete_fec_block<S: VideoSink>(
        &mut self,
        depacketizer: &mut Depacketizer,
        io: &mut Io<S>,
    ) {
        let lowest = self.buffer_lowest_seq;
        let start = self.completed.len();
        for mut packet in self.pending.drain(..) {
            if packet.is_parity {
                io.pools.recycle_packet(packet.buf);
                continue;
            }
            // One receive time per block: the depacketizer only needs the first.
            packet.receive_time_us = self.buffer_first_recv_time_us;
            self.completed.push(packet);
        }
        self.completed[start..].sort_by_key(|p| p.seq.wrapping_sub(lowest));
        if self.early {
            let forwarded = std::mem::take(&mut self.forwarded) as usize;
            for (index, packet) in self.completed.drain(start..).enumerate() {
                if index < forwarded {
                    io.pools.recycle_packet(packet.buf);
                } else {
                    depacketizer.queue_rtp_packet(packet, io);
                }
            }
        }
    }

    fn submit_completed_frame<S: VideoSink>(
        &mut self,
        depacketizer: &mut Depacketizer,
        io: &mut Io<S>,
    ) {
        // Room for the whole frame up front (records add 8 bytes per packet).
        depacketizer.reserve(self.completed.len() * (self.packet_size + 8) + 4);
        for packet in self.completed.drain(..) {
            depacketizer.queue_rtp_packet(packet, io);
        }
    }

    /// `RtpvAddPacket` with Nightjar's PyroWave records wrapper. Hands the packet back when it
    /// is rejected.
    pub(super) fn add_packet<S: VideoSink>(
        &mut self,
        packet: Packet,
        depacketizer: &mut Depacketizer,
        io: &mut Io<S>,
    ) -> Result<(), Packet> {
        if !self.records {
            return self.add_ordinary(packet, depacketizer, io);
        }
        let offset = packet.data_offset();
        if packet.len < offset + NV_VIDEO_PACKET_SIZE {
            return Err(packet);
        }
        let frame = le32(&packet.buf, offset + 4);
        let block = (packet.buf[offset + 11] >> 4) & 3;
        if is_before32(frame, self.current_frame)
            || (frame == self.current_frame && block < self.current_block)
        {
            return Err(packet);
        }
        if frame != self.current_frame || block != self.current_block {
            if !self.pending.is_empty() {
                self.report_fec_status(io);
                self.finish_fec_block(io, true);
                // Keep every data packet of the incomplete block; the records survive holes.
                self.stage_complete_fec_block(depacketizer, io);
            }
            if frame != self.current_frame {
                self.submit_completed_frame(depacketizer, io);
                depacketizer.finish_pyrowave_frame(io);
            }
            self.current_frame = frame;
            self.current_block = block;
        }
        let result = self.add_ordinary(packet, depacketizer, io);
        if self.current_frame != frame {
            depacketizer.finish_pyrowave_frame(io);
        }
        result
    }

    fn add_ordinary<S: VideoSink>(
        &mut self,
        mut packet: Packet,
        depacketizer: &mut Depacketizer,
        io: &mut Io<S>,
    ) -> Result<(), Packet> {
        if is_before16(packet.seq.into(), self.next_contiguous_seq.into()) {
            return Err(packet);
        }
        let offset = packet.data_offset();
        if packet.len < offset + NV_VIDEO_PACKET_SIZE {
            return Err(packet);
        }
        // Legacy hosts look like single-block multi-FEC hosts.
        if !self.multi_fec_capable {
            packet.buf[offset + 10] = 0x10;
            packet.buf[offset + 11] = 0;
        }
        let frame = le32(&packet.buf, offset + 4);
        let fec_info = le32(&packet.buf, offset + 12);
        let blocks = packet.buf[offset + 11];

        if is_before16(frame, self.current_frame) {
            return Err(packet);
        }

        let fec_index = (fec_info & 0x3F_F000) >> 12;
        let block = (blocks >> 4) & 0x3;

        if frame == self.current_frame && block < self.current_block {
            return Err(packet);
        }

        // Start a new block when the queue is empty after a delivery or when the next frame or
        // block begins before this one completed.
        if self.pending.is_empty() || self.current_frame != frame || self.current_block != block {
            if !self.pending.is_empty() {
                self.report_fec_status(io);
                self.finish_fec_block(io, true);
                // Losing one block loses the rest of its frame.
                if self.last_block != 0 && self.current_frame == frame {
                    self.purge_pending(io);
                    self.purge_completed(io);
                    if !self.reported_lost_frame {
                        self.notify_frame_lost(depacketizer, io, self.current_frame, false);
                    }
                    self.current_frame = self.current_frame.wrapping_add(1);
                    self.current_block = 0;
                    self.reported_lost_frame = false;
                    return Err(packet);
                }
            }

            // A block must continue the current frame or start the next frame at block 0.
            let expected = if self.current_frame == frame {
                self.current_block
            } else {
                0
            };
            if block != expected {
                self.report_fec_status(io);
                self.purge_pending(io);
                self.purge_completed(io);
                if !self.reported_lost_frame {
                    self.notify_frame_lost(depacketizer, io, frame, false);
                }
                self.current_frame = frame.wrapping_add(1);
                self.current_block = 0;
                self.reported_lost_frame = false;
                return Err(packet);
            }

            self.purge_pending(io);
            if self.current_frame != frame {
                self.purge_completed(io);
                // A whole frame went missing. The frame before this one may already have been
                // reported by the speculative RFI logic.
                if self.current_frame.wrapping_add(1) != frame || !self.reported_lost_frame {
                    self.notify_frame_lost(depacketizer, io, frame.wrapping_sub(1), false);
                }
            }

            self.current_frame = frame;
            io.sink.signal(VideoSignal::FrameSeen { frame });

            self.buffer_first_recv_time_us = io.now_us;
            self.buffer_lowest_seq = packet.seq.wrapping_sub(fec_index as u16);
            self.next_contiguous_seq = self.buffer_lowest_seq;
            self.received_data_packets = 0;
            self.received_parity_packets = 0;
            self.forwarded = 0;
            self.received_highest_seq = 0;
            self.missing_packets = 0;
            self.use_fast_queue_path = true;
            self.reported_lost_frame = false;
            self.buffer_data_packets = (fec_info & 0xFFC0_0000) >> 22;
            self.fec_percentage = (fec_info & 0xFF0) >> 4;
            self.buffer_parity_packets =
                (self.buffer_data_packets * self.fec_percentage).div_ceil(100);
            self.buffer_first_parity_seq = self
                .buffer_lowest_seq
                .wrapping_add(self.buffer_data_packets as u16);
            self.buffer_highest_seq = self
                .buffer_first_parity_seq
                .wrapping_add(self.buffer_parity_packets as u16)
                .wrapping_sub(1);
            self.current_block = block;
            self.last_block = (blocks >> 6) & 0x3;

            io.stats.packet_count_video = io
                .stats
                .packet_count_video
                .wrapping_add(self.buffer_data_packets);
            io.stats.packet_count_fec = io
                .stats
                .packet_count_fec
                .wrapping_add(self.buffer_parity_packets);
        }

        if is_before16(self.buffer_highest_seq.into(), packet.seq.into()) {
            return Err(packet);
        }

        let seq = packet.seq;
        let is_parity = !is_before16(seq.into(), self.buffer_first_parity_seq.into());
        self.queue_packet(packet, is_parity, false)?;

        if self.pending.len() == 1 {
            self.missing_packets += u32::from(seq.wrapping_sub(self.buffer_lowest_seq));
            self.received_highest_seq = seq;
        } else if is_before16(self.received_highest_seq.into(), seq.into()) {
            self.missing_packets +=
                u32::from(seq.wrapping_sub(self.received_highest_seq).wrapping_sub(1));
            self.received_highest_seq = seq;
        } else {
            // An accepted packet behind the highest one fills a hole.
            self.missing_packets = self.missing_packets.wrapping_sub(1);
        }

        if is_parity {
            self.received_parity_packets += 1;
        } else {
            self.received_data_packets += 1;
        }

        if self.early && !is_parity {
            self.forward_ready(seq, depacketizer, io);
        }

        if self.reconstruct_frame(depacketizer, io) == 0 {
            self.finish_fec_block(io, false);
            self.stage_complete_fec_block(depacketizer, io);
            if self.current_block < self.last_block {
                self.current_block += 1;
            } else {
                self.submit_completed_frame(depacketizer, io);
                self.current_frame = self.current_frame.wrapping_add(1);
                self.current_block = 0;
            }
        }
        Ok(())
    }
}
