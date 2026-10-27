//! Video receive: RTP reordering, FEC block recovery and frame reassembly for H.264, HEVC, AV1
//! and PyroWave, plus the loss and RFI decisions the control stream acts on.
//!
//! Ported from moonlight-common-c (GPL-3.0, Cameron Gutman and contributors):
//! `VideoStream.c` (receive and decryption), `RtpVideoQueue.c` and `VideoDepacketizer.c`, with
//! Nightjar's patches and its `android_rtp_video.c` and `android_depacketizer.c` wrappers
//! (PyroWave records). The behaviour matches that C exactly; `protocol/difftest` checks it.
//!
//! [`VideoReceiver`] has no sockets or threads. Feed it each datagram with its receive time;
//! it hands completed frames and control-stream signals to a [`VideoSink`].

mod decrypt;
mod depacketizer;
mod queue;

use std::ops::Range;

use depacketizer::Depacketizer;
use queue::RtpQueue;

pub use queue::FEC_STATUS_TYPE;

pub const VIDEO_FORMAT_MASK_H264: u32 = 0x000F;
pub const VIDEO_FORMAT_MASK_H265: u32 = 0x0F00;
pub const VIDEO_FORMAT_MASK_AV1: u32 = 0xF000;
pub const VIDEO_FORMAT_MASK_PYROWAVE: u32 = 0x0F_0000;

pub const COLORSPACE_REC_601: u8 = 0;
pub const COLORSPACE_REC_709: u8 = 1;
pub const COLORSPACE_REC_2020: u8 = 2;

/// RTP header plus the extension the host always sends.
pub const MAX_RTP_HEADER_SIZE: usize = 16;
/// `ENC_VIDEO_HEADER`: IV, frame number and GCM tag before each encrypted packet.
pub const ENC_VIDEO_HEADER_SIZE: usize = 32;

/// The video socket's receive buffer size: 2048 packets, or about 60 ms at the stream's
/// bitrate when that is more, capped at 64 MiB (Nightjar's `videoRecvBufferSize`). The OS
/// may grant less.
pub fn recv_buffer_size(bitrate_kbps: u32, packet_size: usize) -> usize {
    let packets = 2048 * (packet_size + MAX_RTP_HEADER_SIZE) as u64;
    let burst = u64::from(bitrate_kbps) * 1000 / 8 * 60 / 1000;
    packets.max(burst).min(64 * 1024 * 1024) as usize
}

/// Session parameters the receiver needs, as negotiated over RTSP.
#[derive(Clone, Debug)]
pub struct VideoConfig {
    /// The negotiated `VIDEO_FORMAT_*` value (PyroWave formats included).
    pub format: u32,
    /// `StreamConfig.packetSize`: video payload bytes per packet, excluding the RTP header.
    pub packet_size: usize,
    /// The host's version quad from serverinfo; Sunshine-derived hosts have a negative fourth part.
    pub app_version: [i32; 4],
    /// Reference frame invalidation is supported by both the host and the decoder.
    pub rfi: bool,
    /// The host sends PyroWave record framing (PyroWave negotiated and the host advertises it).
    pub pyrowave_records: bool,
    /// `StreamConfig.colorSpace`, reported on SDR frames.
    pub colorspace: u8,
    /// The `remoteInputAesKey` when the host encrypts video (`SS_ENC_VIDEO`).
    pub encryption_key: Option<[u8; 16]>,
    /// The decoder takes the packets of a PyroWave records envelope as they arrive
    /// ([`VideoSink::chunk`]), ahead of the completed [`Frame`]. Ignored without
    /// `pyrowave_records`; the frames themselves are the same either way.
    pub early_chunks: bool,
}

impl VideoConfig {
    fn at_least(&self, major: i32, minor: i32, patch: i32) -> bool {
        let v = &self.app_version;
        v[0] > major || (v[0] == major && (v[1] > minor || (v[1] == minor && v[2] >= patch)))
    }

    fn sunshine(&self) -> bool {
        self.app_version[3] < 0
    }

    fn codec(&self) -> Codec {
        if self.format & VIDEO_FORMAT_MASK_H264 != 0 {
            Codec::H264
        } else if self.format & VIDEO_FORMAT_MASK_H265 != 0 {
            Codec::Hevc
        } else {
            Codec::Opaque
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Codec {
    H264,
    Hevc,
    /// AV1 and PyroWave: passed through without parsing.
    Opaque,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FrameType {
    /// References earlier frames.
    #[default]
    P = 0,
    /// A key frame. For H.264 and HEVC it starts with the parameter set buffers.
    Idr = 1,
}

/// `BUFFER_TYPE_*`: what a buffer of an H.264 or HEVC frame holds. Other codecs only use
/// `PicData`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferType {
    PicData = 0,
    Sps = 1,
    Pps = 2,
    Vps = 3,
}

/// One buffer of a frame, a range of [`Frame::data`]. Decoders that take parameter sets
/// separately submit `Sps`, `Pps` and `Vps` buffers on their own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameBuffer {
    pub kind: BufferType,
    pub range: Range<usize>,
}

/// A completed frame, the `DECODE_UNIT` equivalent.
#[derive(Debug, Default)]
pub struct Frame {
    pub number: u32,
    pub frame_type: FrameType,
    /// Host processing latency in 1/10 ms; zero when the host does not report it.
    pub host_processing_latency: u16,
    /// Receive time of the frame's first FEC block, in the caller's microsecond clock.
    pub receive_time_us: u64,
    /// When the frame was completed.
    pub enqueue_time_us: u64,
    /// Presentation time from the RTP timestamp, in microseconds.
    pub presentation_time_us: u64,
    /// The RTP timestamp in 90 kHz units.
    pub rtp_timestamp: u32,
    pub hdr_active: bool,
    pub colorspace: u8,
    data: Vec<u8>,
    buffers: Vec<FrameBuffer>,
}

impl Frame {
    /// All buffers back to back.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn buffers(&self) -> &[FrameBuffer] {
        &self.buffers
    }

    /// Each buffer's type and bytes, in order.
    pub fn buffer_slices(&self) -> impl Iterator<Item = (BufferType, &[u8])> {
        self.buffers
            .iter()
            .map(|b| (b.kind, &self.data[b.range.clone()]))
    }
}

/// `SS_FRAME_FEC_STATUS`, the FEC outcome of a frame block. Sunshine-derived hosts take it as
/// control message [`FEC_STATUS_TYPE`] (0x5502) to adapt their FEC; other hosts ignore it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FecStatus {
    pub frame_index: u32,
    pub highest_received_sequence: u16,
    pub next_contiguous_sequence: u16,
    pub missing_before_highest: u16,
    pub total_data_packets: u16,
    pub total_parity_packets: u16,
    pub received_data_packets: u16,
    pub received_parity_packets: u16,
    pub fec_percentage: u8,
    pub block_index: u8,
    pub block_count: u8,
}

impl FecStatus {
    /// The 21-byte big-endian control payload.
    pub fn to_bytes(&self) -> [u8; 21] {
        let mut out = [0; 21];
        out[0..4].copy_from_slice(&self.frame_index.to_be_bytes());
        let words = [
            self.highest_received_sequence,
            self.next_contiguous_sequence,
            self.missing_before_highest,
            self.total_data_packets,
            self.total_parity_packets,
            self.received_data_packets,
            self.received_parity_packets,
        ];
        for (i, word) in words.iter().enumerate() {
            out[4 + 2 * i..6 + 2 * i].copy_from_slice(&word.to_be_bytes());
        }
        out[18] = self.fec_percentage;
        out[19] = self.block_index;
        out[20] = self.block_count;
        out
    }
}

/// What the receiver tells the control stream, in the order the C core would act on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VideoSignal {
    /// `LiRequestIdrFrame`: drop pending invalidations and request a key frame.
    RequestIdr,
    /// `connectionDetectedFrameLoss`: invalidate these reference frames (RFI), or request a key
    /// frame when RFI is unavailable or the control stream's invalidation queue is full.
    InvalidateFrames { first: u32, last: u32 },
    /// `connectionSendFrameFecStatus`: queue a 0x5502 report (best effort, Sunshine hosts only).
    FecStatus(FecStatus),
    /// `connectionReceivedCompleteFrame`: a frame completed; acknowledge it if it is an LTR
    /// frame and the host is Sunshine-derived with RFI enabled.
    FrameReceived { frame: u32, ltr: bool },
    /// `connectionSawFrame`: the first packet of a frame block arrived (connection quality).
    FrameSeen { frame: u32 },
}

/// Receives the receiver's output. Calls happen inside [`VideoReceiver::receive`] and the
/// decoder feedback methods.
pub trait VideoSink {
    /// Takes a completed frame, or hands it back when the decode queue is full. A full queue
    /// makes the receiver call [`VideoSink::flush_frames`], drop its state and request a key
    /// frame, as the C core does when its 15-frame queue overflows.
    fn frame(&mut self, frame: Frame) -> Result<(), Frame>;
    /// Discard every queued frame that has not reached the decoder.
    fn flush_frames(&mut self) {}
    fn signal(&mut self, signal: VideoSignal);
    /// One packet of the records envelope of the frame being assembled, in order, as soon as
    /// it is in the envelope (only with [`VideoConfig::early_chunks`]). `first` starts a new
    /// envelope, so everything given since the previous `first` is dropped. The `Frame` that
    /// ends the envelope carries `[0u32]` and then `[payload length][flags][payload]` for
    /// every chunk, so a sink that sees all chunks since `first` has seen the whole frame.
    fn chunk(&mut self, _first: bool, _flags: u32, _payload: &[u8]) {}
}

/// Receiver output as values, for callers that collect it.
#[derive(Debug)]
pub enum VideoEvent {
    Frame(Frame),
    Signal(VideoSignal),
    /// [`VideoSink::chunk`].
    Chunk {
        first: bool,
        flags: u32,
        payload: Vec<u8>,
    },
}

/// Collects everything and never reports a full queue.
impl VideoSink for Vec<VideoEvent> {
    fn frame(&mut self, frame: Frame) -> Result<(), Frame> {
        self.push(VideoEvent::Frame(frame));
        Ok(())
    }

    fn flush_frames(&mut self) {
        self.retain(|event| !matches!(event, VideoEvent::Frame(_)));
    }

    fn signal(&mut self, signal: VideoSignal) {
        self.push(VideoEvent::Signal(signal));
    }

    fn chunk(&mut self, first: bool, flags: u32, payload: &[u8]) {
        self.push(VideoEvent::Chunk {
            first,
            flags,
            payload: payload.to_vec(),
        });
    }
}

/// Counters for the life of the receiver.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VideoStats {
    /// Data packets in the FEC blocks started (`RTP_VIDEO_STATS.packetCountVideo`).
    pub packet_count_video: u32,
    /// Parity packets in the FEC blocks started (`RTP_VIDEO_STATS.packetCountFec`).
    pub packet_count_fec: u32,
    /// Datagrams received, including rejected ones.
    pub packets_received: u64,
    /// Data packets rebuilt from parity.
    pub packets_recovered: u64,
    /// Encrypted datagrams that failed authentication.
    pub decrypt_failures: u64,
    /// Frames handed to the sink.
    pub frames_completed: u64,
    /// Times the depacketizer dropped its frame state (each lost or skipped frame).
    pub frame_drops: u64,
}

/// A packet waiting in the RTP queue. The buffer holds `packet_size + 16` bytes.
struct Packet {
    buf: Box<[u8]>,
    len: usize,
    seq: u16,
    timestamp: u32,
    receive_time_us: u64,
    presentation_time_us: u64,
    rtp_timestamp: u32,
    is_parity: bool,
}

impl Packet {
    fn data_offset(&self) -> usize {
        rtp_data_offset(self.buf[0])
    }
}

fn rtp_data_offset(header: u8) -> usize {
    if header & FLAG_EXTENSION != 0 {
        16
    } else {
        12
    }
}

const FLAG_EXTENSION: u8 = 0x10;
const FLAG_CONTAINS_PIC_DATA: u8 = 0x1;
const FLAG_EOF: u8 = 0x2;
const FLAG_SOF: u8 = 0x4;
const NV_VIDEO_PACKET_SIZE: usize = 16;

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn is_before16(x: u32, y: u32) -> bool {
    (x.wrapping_sub(y) & 0xFFFF) > 0x7FFF
}

fn is_before24(x: u32, y: u32) -> bool {
    (x.wrapping_sub(y) & 0xFF_FFFF) > 0x7F_FFFF
}

fn is_before32(x: u32, y: u32) -> bool {
    x.wrapping_sub(y) > 0x7FFF_FFFF
}

/// Recycled packet and frame buffers, so steady streaming does not allocate.
struct Pools {
    packet_size: usize,
    packets: Vec<Box<[u8]>>,
    frames: Vec<(Vec<u8>, Vec<FrameBuffer>)>,
}

const PACKET_POOL_LIMIT: usize = 4096;
const FRAME_POOL_LIMIT: usize = 4;

impl Pools {
    fn packet(&mut self) -> Box<[u8]> {
        self.packets
            .pop()
            .unwrap_or_else(|| vec![0; self.packet_size + MAX_RTP_HEADER_SIZE].into_boxed_slice())
    }

    fn recycle_packet(&mut self, buf: Box<[u8]>) {
        if self.packets.len() < PACKET_POOL_LIMIT {
            self.packets.push(buf);
        }
    }

    fn frame_storage(&mut self) -> (Vec<u8>, Vec<FrameBuffer>) {
        self.frames.pop().unwrap_or_default()
    }

    fn recycle_frame(&mut self, mut data: Vec<u8>, mut buffers: Vec<FrameBuffer>) {
        if self.frames.len() < FRAME_POOL_LIMIT {
            data.clear();
            buffers.clear();
            self.frames.push((data, buffers));
        }
    }
}

/// Mutable state shared by the queue and the depacketizer during one call.
struct Io<'a, S: VideoSink> {
    sink: &'a mut S,
    pools: &'a mut Pools,
    stats: &'a mut VideoStats,
    now_us: u64,
}

/// The video receive path of one stream.
pub struct VideoReceiver {
    config: VideoConfig,
    queue: RtpQueue,
    depacketizer: Depacketizer,
    decryptor: Option<decrypt::Decryptor>,
    pools: Pools,
    stats: VideoStats,
}

impl VideoReceiver {
    pub fn new(config: VideoConfig) -> Self {
        Self {
            queue: RtpQueue::new(&config),
            depacketizer: Depacketizer::new(&config),
            decryptor: config.encryption_key.as_ref().map(decrypt::Decryptor::new),
            pools: Pools {
                packet_size: config.packet_size,
                packets: Vec::new(),
                frames: Vec::new(),
            },
            stats: VideoStats::default(),
            config,
        }
    }

    pub fn config(&self) -> &VideoConfig {
        &self.config
    }

    /// Largest datagram the receiver reads; longer ones are truncated like a short `recv`.
    pub fn max_datagram_size(&self) -> usize {
        self.config.packet_size
            + MAX_RTP_HEADER_SIZE
            + if self.decryptor.is_some() {
                ENC_VIDEO_HEADER_SIZE
            } else {
                0
            }
    }

    /// Processes one datagram from the video socket received at `now_us` (any monotonic
    /// microsecond clock; it also stamps completed frames).
    pub fn receive(&mut self, datagram: &[u8], now_us: u64, sink: &mut impl VideoSink) {
        self.stats.packets_received += 1;
        let datagram = &datagram[..datagram.len().min(self.max_datagram_size())];
        let header = if self.decryptor.is_some() {
            ENC_VIDEO_HEADER_SIZE
        } else {
            0
        };
        if datagram.len() < 12 + header {
            return;
        }
        let mut buf = self.pools.packet();
        let len = if let Some(decryptor) = &self.decryptor {
            // Drop shards of frames already reassembled before spending time on decryption.
            let frame = le32(datagram, 12);
            if frame != 0 && frame < self.queue.current_frame_number() {
                self.pools.recycle_packet(buf);
                return;
            }
            match decryptor.open(datagram, &mut buf) {
                Some(len) => len,
                None => {
                    self.stats.decrypt_failures += 1;
                    self.pools.recycle_packet(buf);
                    return;
                }
            }
        } else {
            buf[..datagram.len()].copy_from_slice(datagram);
            datagram.len()
        };
        let packet = Packet {
            seq: u16::from_be_bytes([buf[2], buf[3]]),
            timestamp: u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]),
            buf,
            len,
            receive_time_us: 0,
            presentation_time_us: 0,
            rtp_timestamp: 0,
            is_parity: false,
        };
        let mut io = Io {
            sink,
            pools: &mut self.pools,
            stats: &mut self.stats,
            now_us,
        };
        if let Err(packet) = self
            .queue
            .add_packet(packet, &mut self.depacketizer, &mut io)
        {
            io.pools.recycle_packet(packet.buf);
        }
    }

    /// The decoder accepted a frame (`DR_OK`). A decoded key frame enables RFI recovery.
    pub fn frame_decoded(&mut self, frame_type: FrameType) {
        if frame_type == FrameType::Idr {
            self.depacketizer.idr_frame_processed();
        }
    }

    /// The decoder needs a key frame (`DR_NEED_IDR`, `requestDecoderRefresh`): flushes the
    /// sink's queued frames, drops the frame in progress at its end and requests a key frame.
    pub fn request_decoder_refresh(&mut self, sink: &mut impl VideoSink) {
        self.depacketizer.request_decoder_refresh(sink);
    }

    /// The host display's HDR mode from the control stream; frames report it and use BT.2020
    /// while it is on.
    pub fn set_hdr_active(&mut self, active: bool) {
        self.depacketizer.hdr_active = active;
    }

    /// The frame the RTP queue is assembling.
    pub fn current_frame_number(&self) -> u32 {
        self.queue.current_frame_number()
    }

    pub fn stats(&self) -> VideoStats {
        self.stats
    }

    /// Returns a frame's buffers once the caller is done with it, to be reused.
    pub fn recycle(&mut self, frame: Frame) {
        self.pools.recycle_frame(frame.data, frame.buffers);
    }
}

#[cfg(test)]
pub(crate) mod tests;
