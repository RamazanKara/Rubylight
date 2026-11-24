//! PyroWave records envelope assembly and early-chunk emission.
//!
//! Ported from Nightjar's `android_depacketizer.c` (`pyrowave-renderer/frame.h`).

use super::{
    Depacketizer, PYRO_PACKET_CONTIGUOUS, PYRO_PACKET_FRAME_END, PYRO_PACKET_FRAME_START,
    PYRO_PACKET_RECORD_START,
};
use crate::video::{
    le32, BufferType, FrameBuffer, FrameType, Io, Packet, VideoSink, FLAG_EOF, FLAG_SOF,
    NV_VIDEO_PACKET_SIZE,
};

impl Depacketizer {
    /// Appends one packet to the records envelope: a zero container count once, then each
    /// packet's length, flags and payload (little endian).
    fn append_pyrowave_packet(&mut self, payload: &[u8], flags: u32) -> bool {
        let first = self.buffers.is_empty();
        let added = if first { 4 } else { 0 } + 8 + payload.len();
        if self.data.len() > self.max_frame_bytes - added {
            return false;
        }
        if first {
            self.data.extend_from_slice(&0u32.to_le_bytes());
        }
        self.data
            .extend_from_slice(&(payload.len() as u32).to_le_bytes());
        self.data.extend_from_slice(&flags.to_le_bytes());
        self.data.extend_from_slice(payload);
        let range = 0..self.data.len();
        if first {
            self.buffers.push(FrameBuffer {
                kind: BufferType::PicData,
                range,
            });
        } else {
            self.buffers[0].range = range;
        }
        true
    }

    /// Appends a packet the RTP queue still holds (its block is incomplete): the same as
    /// [`Self::queue_rtp_packet`] without taking the buffer.
    pub(in super::super) fn queue_early_packet<S: VideoSink>(
        &mut self,
        packet: &Packet,
        io: &mut Io<S>,
    ) {
        self.queue_pyrowave_packet(packet, io);
    }

    pub(super) fn queue_pyrowave_packet<S: VideoSink>(
        &mut self,
        packet: &Packet,
        io: &mut Io<S>,
    ) {
        let offset = packet.data_offset();
        if packet.len < offset + NV_VIDEO_PACKET_SIZE {
            return;
        }
        let video = &packet.buf[offset..packet.len];
        let payload = &video[NV_VIDEO_PACKET_SIZE..];
        let spi = (le32(video, 0) >> 8) & 0xFF_FFFF;
        let video_flags = video[8];
        let block = (video[11] >> 4) & 3;
        let last_block = (video[11] >> 6) & 3;
        // Vibepollo marks record starts in extraFlags, Rubylight in multiFecFlags.
        let mut flags = if (video[9] | video[10]) & 0x80 != 0 {
            PYRO_PACKET_RECORD_START
        } else {
            0
        };
        if video_flags & FLAG_SOF != 0 && block == 0 {
            flags |= PYRO_PACKET_FRAME_START;
        }
        if video_flags & FLAG_EOF != 0 && block == last_block {
            flags |= PYRO_PACKET_FRAME_END;
        }
        if payload.is_empty() || payload.len() > self.max_packet_bytes {
            return;
        }
        let first = self.buffers.is_empty();
        if !first
            && ((block == self.pyro_last_block
                && spi == self.last_packet_in_stream.wrapping_add(1) & 0xFF_FFFF)
                || (u16::from(block) == u16::from(self.pyro_last_block) + 1
                    && self.pyro_last_block_ended
                    && video_flags & FLAG_SOF != 0))
        {
            flags |= PYRO_PACKET_CONTIGUOUS;
        }
        if !self.append_pyrowave_packet(payload, flags) {
            return;
        }
        if self.early {
            io.sink.chunk(first, flags, payload);
        }
        if first {
            self.next_frame_number = le32(video, 4);
            self.first_packet_receive_time_us = packet.receive_time_us;
            self.first_packet_presentation_time = packet.presentation_time_us;
            self.first_packet_rtp_timestamp = packet.rtp_timestamp;
            self.frame_host_processing_latency =
                if flags & PYRO_PACKET_FRAME_START != 0 && payload.len() >= 3 {
                    u16::from_le_bytes([payload[1], payload[2]])
                } else {
                    0
                };
        }
        self.last_packet_in_stream = spi;
        self.pyro_last_block = block;
        self.pyro_last_block_ended = video_flags & FLAG_EOF != 0;
    }

    /// Delivers the records envelope collected so far, complete or not.
    pub(in super::super) fn finish_pyrowave_frame<S: VideoSink>(&mut self, io: &mut Io<S>) {
        if self.buffers.is_empty() {
            return;
        }
        self.frame_type = FrameType::Idr;
        self.waiting_for_idr_frame = false;
        self.waiting_for_next_successful_frame = false;
        self.next_frame_number = self.next_frame_number.wrapping_add(1);
        self.reassemble_frame(self.next_frame_number.wrapping_sub(1), false, io);
        self.cleanup_frame_state();
    }
}
