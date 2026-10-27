//! Frame reassembly from the RTP queue's ordered packets: frame headers, Annex B parsing for
//! H.264 and HEVC, the IDR and RFI wait states, and loss reporting. Port of
//! `VideoDepacketizer.c` and Nightjar's `android_depacketizer.c` (PyroWave record envelopes).

use std::ops::Range;

use super::{
    is_before24, is_before32, le32, BufferType, Codec, Frame, FrameBuffer, FrameType, Io, Packet,
    VideoConfig, VideoSignal, VideoSink, COLORSPACE_REC_2020, FLAG_CONTAINS_PIC_DATA, FLAG_EOF,
    FLAG_SOF, NV_VIDEO_PACKET_SIZE,
};

const CONSECUTIVE_DROP_LIMIT: u32 = 120;

const H264_NAL_TYPE_SEI: u8 = 6;
const H264_NAL_TYPE_SPS: u8 = 7;
const H264_NAL_TYPE_PPS: u8 = 8;
const H264_NAL_TYPE_AUD: u8 = 9;
const HEVC_NAL_TYPE_VPS: u8 = 32;
const HEVC_NAL_TYPE_SPS: u8 = 33;
const HEVC_NAL_TYPE_PPS: u8 = 34;
const HEVC_NAL_TYPE_AUD: u8 = 35;
const HEVC_NAL_TYPE_SEI: u8 = 39;

const NV_VIDEO_PACKET_EXTRA_FLAG_LTR_FRAME: u8 = 0x1;

/// Flags of each packet in a PyroWave records envelope (`pyrowave-renderer/frame.h`).
pub const PYRO_PACKET_CONTIGUOUS: u32 = 1;
pub const PYRO_PACKET_RECORD_START: u32 = 2;
pub const PYRO_PACKET_FRAME_START: u32 = 4;
pub const PYRO_PACKET_FRAME_END: u32 = 8;
pub const PYRO_MAX_FRAME_BYTES: usize = 4092 * 1400;

/// A window into a packet payload (`BUFFER_DESC`).
#[derive(Clone, Copy)]
struct Desc {
    offset: usize,
    length: usize,
}

pub(super) struct Depacketizer {
    codec: Codec,
    sunshine: bool,
    frame_type_header: bool,
    header_size: fn(u8) -> u32,
    records: bool,
    /// Give each envelope packet to the sink as it is appended ([`VideoSink::chunk`]).
    early: bool,
    colorspace: u8,
    pub(super) hdr_active: bool,

    /// The frame being assembled: its bytes and buffer list (`nalChainHead`).
    data: Vec<u8>,
    buffers: Vec<FrameBuffer>,

    next_frame_number: u32,
    start_frame_number: u32,
    waiting_for_next_successful_frame: bool,
    waiting_for_idr_frame: bool,
    waiting_for_ref_inval_frame: bool,
    last_packet_in_stream: u32,
    decoding_frame: bool,
    frame_type: FrameType,
    last_packet_payload_length: u16,
    strict_idr_frame_wait: bool,
    synthetic_pts_base_us: u64,
    frame_host_processing_latency: u16,
    first_packet_receive_time_us: u64,
    first_packet_presentation_time: u64,
    first_packet_rtp_timestamp: u32,
    drop_state_pending: bool,
    idr_frame_processed: bool,
    consecutive_frame_drops: u32,

    pyro_last_block: u8,
    pyro_last_block_ended: bool,
}

/// Frame header length by host version and the header's first byte.
fn header_size_for(config: &VideoConfig) -> fn(u8) -> u32 {
    if config.at_least(7, 1, 450) {
        |first| if first == 0x01 { 8 } else { 44 }
    } else if config.at_least(7, 1, 446) {
        |first| if first == 0x01 { 8 } else { 41 }
    } else if config.at_least(7, 1, 415) {
        |first| if first == 0x01 { 8 } else { 24 }
    } else if config.at_least(7, 1, 350) {
        |_| 8
    } else if config.at_least(7, 1, 320) {
        |_| 12
    } else if config.at_least(5, 0, 0) {
        |_| 8
    } else {
        |_| 0
    }
}

impl Depacketizer {
    pub(super) fn new(config: &VideoConfig) -> Self {
        Self {
            codec: config.codec(),
            sunshine: config.sunshine(),
            frame_type_header: config.at_least(7, 1, 350),
            header_size: header_size_for(config),
            records: config.pyrowave_records,
            early: config.pyrowave_records && config.early_chunks,
            colorspace: config.colorspace,
            hdr_active: false,
            data: Vec::new(),
            buffers: Vec::new(),
            next_frame_number: 1,
            start_frame_number: 0,
            waiting_for_next_successful_frame: false,
            waiting_for_idr_frame: true,
            waiting_for_ref_inval_frame: false,
            last_packet_in_stream: u32::MAX,
            decoding_frame: false,
            frame_type: FrameType::P,
            last_packet_payload_length: 0,
            strict_idr_frame_wait: !config.rfi,
            synthetic_pts_base_us: 0,
            frame_host_processing_latency: 0,
            first_packet_receive_time_us: 0,
            first_packet_presentation_time: 0,
            first_packet_rtp_timestamp: 0,
            drop_state_pending: false,
            idr_frame_processed: false,
            consecutive_frame_drops: 0,
            pyro_last_block: 0,
            pyro_last_block_ended: false,
        }
    }

    pub(super) fn reserve(&mut self, bytes: usize) {
        self.data.reserve(bytes);
    }

    pub(super) fn idr_frame_processed(&mut self) {
        self.idr_frame_processed = true;
    }

    fn cleanup_frame_state(&mut self) {
        self.data.clear();
        self.buffers.clear();
    }

    /// Drops the frame in progress and decides whether recovery needs a key frame or RFI.
    fn drop_frame_state<S: VideoSink>(&mut self, io: &mut Io<S>) {
        self.drop_state_pending = false;
        io.stats.frame_drops += 1;

        if self.strict_idr_frame_wait
            || !self.idr_frame_processed
            || self.waiting_for_idr_frame
            || (!self.buffers.is_empty() && self.frame_type == FrameType::Idr)
        {
            self.waiting_for_idr_frame = true;
        } else {
            self.waiting_for_ref_inval_frame = true;
        }

        self.consecutive_frame_drops += 1;
        if self.consecutive_frame_drops == CONSECUTIVE_DROP_LIMIT {
            self.consecutive_frame_drops = 0;
            self.waiting_for_idr_frame = true;
            io.sink.signal(VideoSignal::RequestIdr);
        }

        self.cleanup_frame_state();
    }

    pub(super) fn request_decoder_refresh<S: VideoSink>(&mut self, sink: &mut S) {
        self.waiting_for_idr_frame = true;
        sink.flush_frames();
        // Dropped at the next frame boundary, where it cannot corrupt a frame in progress.
        self.drop_state_pending = true;
        sink.signal(VideoSignal::RequestIdr);
    }

    // Annex B parsing. `data` is the payload after the video packet header.

    /// The length of the start code at `d` (3 or 4), only if a NAL type byte follows it.
    fn start_sequence(data: &[u8], d: Desc) -> Option<usize> {
        if d.length <= 3 {
            return None;
        }
        let at = d.offset;
        if data[at] == 0 && data[at + 1] == 0 {
            if data[at + 2] == 0 {
                if d.length > 4 && data[at + 3] == 1 {
                    return Some(4);
                }
            } else if data[at + 2] == 1 {
                return Some(3);
            }
        }
        None
    }

    fn nal_type(&self, data: &[u8], d: Desc) -> Option<u8> {
        let seq = Self::start_sequence(data, d)?;
        let byte = data[d.offset + seq];
        Some(match self.codec {
            Codec::H264 => byte & 0x1F,
            _ => (byte & 0x7E) >> 1,
        })
    }

    fn nal_is(&self, data: &[u8], d: Desc, h264: u8, hevc: u8) -> bool {
        match self.codec {
            Codec::H264 => self.nal_type(data, d) == Some(h264),
            Codec::Hevc => self.nal_type(data, d) == Some(hevc),
            Codec::Opaque => false,
        }
    }

    fn is_seq_reference_frame_start(&self, data: &[u8], d: Desc) -> bool {
        match (self.codec, self.nal_type(data, d)) {
            (Codec::H264, Some(t)) => t == 5,
            (Codec::Hevc, Some(t)) => (16..=21).contains(&t),
            _ => false,
        }
    }

    fn is_access_unit_delimiter(&self, data: &[u8], d: Desc) -> bool {
        self.nal_is(data, d, H264_NAL_TYPE_AUD, HEVC_NAL_TYPE_AUD)
    }

    fn is_sei_nal(&self, data: &[u8], d: Desc) -> bool {
        self.nal_is(data, d, H264_NAL_TYPE_SEI, HEVC_NAL_TYPE_SEI)
    }

    fn is_picture_parameter_set_nal(&self, data: &[u8], d: Desc) -> bool {
        self.nal_is(data, d, H264_NAL_TYPE_PPS, HEVC_NAL_TYPE_PPS)
    }

    fn is_idr_frame_start(&self, data: &[u8], d: Desc) -> bool {
        self.nal_is(data, d, H264_NAL_TYPE_SPS, HEVC_NAL_TYPE_VPS)
    }

    /// Advances to the next start code, or to the end.
    fn skip_to_next_nal_or_end(data: &[u8], d: &mut Desc) {
        if let Some(seq) = Self::start_sequence(data, *d) {
            d.offset += seq;
            d.length -= seq;
        }
        while Self::start_sequence(data, *d).is_none() {
            if d.length == 0 {
                return;
            }
            d.offset += 1;
            d.length -= 1;
        }
    }

    fn buffer_type(&self, fragment: &[u8]) -> BufferType {
        let d = Desc {
            offset: 0,
            length: fragment.len(),
        };
        match (self.codec, self.nal_type(fragment, d)) {
            (Codec::H264, Some(H264_NAL_TYPE_SPS)) => BufferType::Sps,
            (Codec::H264, Some(H264_NAL_TYPE_PPS)) => BufferType::Pps,
            (Codec::Hevc, Some(HEVC_NAL_TYPE_SPS)) => BufferType::Sps,
            (Codec::Hevc, Some(HEVC_NAL_TYPE_PPS)) => BufferType::Pps,
            (Codec::Hevc, Some(HEVC_NAL_TYPE_VPS)) => BufferType::Vps,
            _ => BufferType::PicData,
        }
    }

    fn queue_fragment(&mut self, fragment: &[u8]) {
        let kind = self.buffer_type(fragment);
        let start = self.data.len();
        self.data.extend_from_slice(fragment);
        self.buffers.push(FrameBuffer {
            kind,
            range: start..self.data.len(),
        });
    }

    /// The first packet of an H.264 or HEVC key frame: parameter sets padded apart, then
    /// the picture data to the end of the packet. Prefix SEI (`sei`, the run before the
    /// parameter sets, and any between them) goes with the picture data, after every
    /// parameter set: renderers hand parameter sets to the decoder on their own, so picture
    /// data before one would be overwritten.
    fn process_avc_hevc_payload_slow(&mut self, data: &[u8], mut cur: Desc, sei: Range<usize>) {
        let mut held: Vec<Range<usize>> = Vec::new();
        if !sei.is_empty() {
            held.push(sei);
        }
        while cur.length != 0 {
            if Self::start_sequence(data, cur).is_none() {
                Self::skip_to_next_nal_or_end(data, &mut cur);
            }
            while self.is_access_unit_delimiter(data, cur) {
                Self::skip_to_next_nal_or_end(data, &mut cur);
            }
            if self.is_sei_nal(data, cur) {
                let start = cur.offset;
                Self::skip_to_next_nal_or_end(data, &mut cur);
                held.push(start..cur.offset);
                continue;
            }

            let start = cur.offset;
            let mut contains_pic_data = false;
            if self.is_seq_reference_frame_start(data, cur) {
                self.waiting_for_idr_frame = false;
                self.waiting_for_ref_inval_frame = false;
                self.waiting_for_next_successful_frame = false;
                contains_pic_data = true;
                self.frame_type = FrameType::Idr;
            }

            Self::skip_to_next_nal_or_end(data, &mut cur);
            if contains_pic_data {
                while cur.length != 0 {
                    Self::skip_to_next_nal_or_end(data, &mut cur);
                }
            }
            let fragment = &data[start..cur.offset];
            if self.buffer_type(fragment) == BufferType::PicData {
                for range in held.drain(..) {
                    self.queue_fragment(&data[range]);
                }
            }
            self.queue_fragment(fragment);
        }
        // SEI at the packet's end: the picture data follows in the next packet.
        for range in held {
            self.queue_fragment(&data[range]);
        }
    }

    /// Hands the assembled frame to the sink.
    fn reassemble_frame<S: VideoSink>(
        &mut self,
        frame_number: u32,
        frame_is_ltr: bool,
        io: &mut Io<S>,
    ) {
        if self.buffers.is_empty() {
            return;
        }
        let (data, buffers) = io.pools.frame_storage();
        let data = std::mem::replace(&mut self.data, data);
        let buffers = std::mem::replace(&mut self.buffers, buffers);
        let frame_type =
            if buffers[0].kind != BufferType::PicData || self.frame_type == FrameType::Idr {
                FrameType::Idr
            } else {
                FrameType::P
            };
        let frame = Frame {
            number: frame_number,
            frame_type,
            host_processing_latency: self.frame_host_processing_latency,
            receive_time_us: self.first_packet_receive_time_us,
            enqueue_time_us: io.now_us,
            presentation_time_us: self.first_packet_presentation_time,
            rtp_timestamp: self.first_packet_rtp_timestamp,
            hdr_active: self.hdr_active,
            colorspace: if self.hdr_active {
                COLORSPACE_REC_2020
            } else {
                self.colorspace
            },
            data,
            buffers,
        };
        if let Err(frame) = io.sink.frame(frame) {
            // The decode queue overflowed: recover with a key frame.
            self.waiting_for_idr_frame = true;
            io.pools.recycle_frame(frame.data, frame.buffers);
            self.drop_frame_state(io);
            io.sink.flush_frames();
            io.sink.signal(VideoSignal::RequestIdr);
            return;
        }
        io.stats.frames_completed += 1;
        io.sink.signal(VideoSignal::FrameReceived {
            frame: frame_number,
            ltr: frame_is_ltr,
        });
        self.consecutive_frame_drops = 0;
        // The next RFI range starts after this frame.
        self.start_frame_number = self.next_frame_number;
    }

    fn detected_frame_loss<S: VideoSink>(&self, io: &mut Io<S>, last: u32) {
        io.sink.signal(VideoSignal::InvalidateFrames {
            first: self.start_frame_number,
            last,
        });
    }

    /// Drops the current frame after it turned out unusable and asks for recovery.
    fn abandon_frame<S: VideoSink>(&mut self, frame_index: u32, io: &mut Io<S>) {
        self.decoding_frame = false;
        self.next_frame_number = frame_index.wrapping_add(1);
        self.drop_frame_state(io);
        if self.waiting_for_idr_frame {
            io.sink.signal(VideoSignal::RequestIdr);
        } else {
            self.detected_frame_loss(io, frame_index);
        }
    }

    fn process_rtp_payload<S: VideoSink>(
        &mut self,
        video: &[u8],
        receive_time_us: u64,
        presentation_time_us: u64,
        rtp_timestamp: u32,
        io: &mut Io<S>,
    ) {
        let stream_packet_index = (le32(video, 0) >> 8) & 0xFF_FFFF;
        let frame_index = le32(video, 4);
        let flags = video[8];
        let extra_flags = video[9];
        let block = (video[11] >> 4) & 0x3;
        let last_block = (video[11] >> 6) & 0x3;
        let data = &video[NV_VIDEO_PACKET_SIZE..];
        let mut cur = Desc {
            offset: 0,
            length: data.len(),
        };

        let marker = flags & !FLAG_CONTAINS_PIC_DATA;
        let first_packet = (marker == (FLAG_SOF | FLAG_EOF) || marker == FLAG_SOF) && block == 0;
        let last_packet = flags & FLAG_EOF != 0 && block == last_block;

        // Packets of a frame already dropped.
        if is_before32(frame_index, self.next_frame_number) {
            return;
        }

        // Catches corrupt FEC recoveries that slipped past the RTP queue.
        let expected_index = self.last_packet_in_stream.wrapping_add(1) & 0xFF_FFFF;
        if is_before24(stream_packet_index, expected_index)
            || (flags & FLAG_SOF == 0 && stream_packet_index != expected_index)
        {
            self.abandon_frame(frame_index, io);
            return;
        }

        if first_packet {
            if is_before32(self.next_frame_number, frame_index) {
                // The network dropped whole frames; wait for the next complete one.
                self.next_frame_number = frame_index;
                self.waiting_for_next_successful_frame = true;
                self.drop_frame_state(io);
            }

            self.decoding_frame = true;
            self.frame_type = FrameType::P;
            self.first_packet_receive_time_us = receive_time_us;

            // Some Sunshine versions send no PTS; synthesize one from the receive time.
            if self.synthetic_pts_base_us == 0 {
                self.synthetic_pts_base_us = receive_time_us;
            }
            self.first_packet_presentation_time = if presentation_time_us == 0 && frame_index > 0 {
                receive_time_us.wrapping_sub(self.synthetic_pts_base_us)
            } else {
                presentation_time_us
            };
            self.first_packet_rtp_timestamp = rtp_timestamp;
        }

        self.last_packet_in_stream = stream_packet_index;

        let mut frame_header_size = 0;
        // The first packet's prefix SEI after the AUD, which the decoder gets (HDR10+ among it).
        let mut sei = cur.offset..cur.offset;
        if first_packet && cur.length > 0 {
            if self.frame_type_header && cur.length >= 4 {
                match data[cur.offset + 3] {
                    2 | 4 | 5 => {
                        // Codecs without a parsed bitstream trust the header's IDR marker.
                        if data[cur.offset + 3] == 2 && self.codec == Codec::Opaque {
                            self.waiting_for_idr_frame = false;
                            self.waiting_for_next_successful_frame = false;
                            self.frame_type = FrameType::Idr;
                        }
                        if self.waiting_for_ref_inval_frame {
                            self.waiting_for_ref_inval_frame = false;
                            self.waiting_for_next_successful_frame = false;
                        }
                    }
                    _ => {}
                }
            } else if self.waiting_for_ref_inval_frame {
                // Older hosts: hope for the best.
                self.detected_frame_loss(io, frame_index.wrapping_sub(1));
                self.waiting_for_ref_inval_frame = false;
                self.waiting_for_next_successful_frame = false;
            }

            if self.sunshine && cur.length >= 3 {
                self.frame_host_processing_latency =
                    u16::from_le_bytes([data[cur.offset + 1], data[cur.offset + 2]]);
            }

            // Opaque codecs need the exact length of the last packet's payload.
            if self.codec == Codec::Opaque && cur.length >= 6 {
                self.last_packet_payload_length =
                    u16::from_le_bytes([data[cur.offset + 4], data[cur.offset + 5]]);
            }

            frame_header_size = (self.header_size)(data[0]);
            if cur.length >= frame_header_size as usize {
                cur.offset += frame_header_size as usize;
                cur.length -= frame_header_size as usize;
            }

            if self.codec != Codec::Opaque {
                if Self::start_sequence(data, cur).is_none() {
                    Self::skip_to_next_nal_or_end(data, &mut cur);
                }
                if self.is_access_unit_delimiter(data, cur) {
                    Self::skip_to_next_nal_or_end(data, &mut cur);
                }
                // Skipped only to find what follows: the SEI stays in the frame.
                sei = cur.offset..cur.offset;
                while self.is_sei_nal(data, cur) {
                    Self::skip_to_next_nal_or_end(data, &mut cur);
                }
                sei.end = cur.offset;
            }
        }

        if self.codec != Codec::Opaque {
            if first_packet && self.is_idr_frame_start(data, cur) {
                self.process_avc_hevc_payload_slow(data, cur, sei);
            } else {
                // Intel's H.264 Media Foundation encoder prepends a PPS to each P-frame.
                if first_packet && self.is_picture_parameter_set_nal(data, cur) {
                    if !sei.is_empty() {
                        self.queue_fragment(&data[sei]);
                    }
                    Self::skip_to_next_nal_or_end(data, &mut cur);
                } else {
                    cur.length += cur.offset - sei.start;
                    cur.offset = sei.start;
                }
                self.queue_fragment(&data[cur.offset..cur.offset + cur.length]);
            }
        } else {
            if last_packet {
                let length = u32::from(self.last_packet_payload_length);
                if length > frame_header_size && length - frame_header_size <= cur.length as u32 {
                    cur.length = (length - frame_header_size) as usize;
                } else {
                    self.abandon_frame(frame_index, io);
                    return;
                }
            }
            self.queue_fragment(&data[cur.offset..cur.offset + cur.length]);
        }

        if last_packet {
            self.decoding_frame = false;
            self.next_frame_number = frame_index.wrapping_add(1);

            if self.waiting_for_idr_frame || self.waiting_for_ref_inval_frame {
                if self.waiting_for_idr_frame {
                    // Request the key frame only once a frame arrived whole, when the network
                    // has likely recovered.
                    if self.waiting_for_next_successful_frame {
                        io.sink.signal(VideoSignal::RequestIdr);
                    }
                } else {
                    self.detected_frame_loss(io, frame_index);
                }
                self.waiting_for_next_successful_frame = false;
                self.drop_frame_state(io);
                return;
            }

            if self.drop_state_pending {
                if !self.buffers.is_empty() && self.frame_type == FrameType::Idr {
                    // Keep a key frame: it is the recovery the drop is waiting for.
                    self.drop_state_pending = false;
                } else {
                    self.drop_frame_state(io);
                    return;
                }
            }

            self.reassemble_frame(
                frame_index,
                extra_flags & NV_VIDEO_PACKET_EXTRA_FLAG_LTR_FRAME != 0,
                io,
            );
        }
    }

    /// The RTP queue found a frame unrecoverable (`notifyFrameLost`).
    pub(super) fn notify_frame_lost<S: VideoSink>(
        &mut self,
        frame_number: u32,
        _speculative: bool,
        io: &mut Io<S>,
    ) {
        self.drop_frame_state(io);
        if !self.waiting_for_idr_frame {
            self.next_frame_number = frame_number.wrapping_add(1);
            self.detected_frame_loss(io, frame_number);
        }
    }

    /// `queueRtpPacket`: consumes a data packet in frame order.
    pub(super) fn queue_rtp_packet<S: VideoSink>(&mut self, packet: Packet, io: &mut Io<S>) {
        if self.records {
            self.queue_pyrowave_packet(&packet, io);
        } else {
            let offset = packet.data_offset();
            self.process_rtp_payload(
                &packet.buf[offset..packet.len],
                packet.receive_time_us,
                packet.presentation_time_us,
                packet.rtp_timestamp,
                io,
            );
        }
        io.pools.recycle_packet(packet.buf);
    }

    /// Appends one packet to the records envelope: a zero container count once, then each
    /// packet's length, flags and payload (little endian).
    fn append_pyrowave_packet(&mut self, payload: &[u8], flags: u32) -> bool {
        let first = self.buffers.is_empty();
        let added = if first { 4 } else { 0 } + 8 + payload.len();
        if self.data.len() > PYRO_MAX_FRAME_BYTES - added {
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
    pub(super) fn queue_early_packet<S: VideoSink>(&mut self, packet: &Packet, io: &mut Io<S>) {
        self.queue_pyrowave_packet(packet, io);
    }

    fn queue_pyrowave_packet<S: VideoSink>(&mut self, packet: &Packet, io: &mut Io<S>) {
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
        if payload.is_empty() || payload.len() > 1400 {
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
    pub(super) fn finish_pyrowave_frame<S: VideoSink>(&mut self, io: &mut Io<S>) {
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
