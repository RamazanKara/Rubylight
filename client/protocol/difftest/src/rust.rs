//! The Rust side: `nightjar-core` writing the same event log as the C shim.

use std::collections::VecDeque;

use nightjar_core::audio::{AudioConfig, AudioOutput, AudioReceiver};
use nightjar_core::video::{Frame, VideoConfig, VideoReceiver, VideoSignal, VideoSink};

struct LogSink {
    log: Vec<u8>,
    queue: VecDeque<Frame>,
    bound: usize,
}

impl LogSink {
    fn frame_log(&mut self, f: &Frame) {
        let log = &mut self.log;
        log.push(b'F');
        log.extend_from_slice(&f.number.to_le_bytes());
        log.push(f.frame_type as u8);
        log.extend_from_slice(&f.host_processing_latency.to_le_bytes());
        log.extend_from_slice(&f.receive_time_us.to_le_bytes());
        log.extend_from_slice(&f.enqueue_time_us.to_le_bytes());
        log.extend_from_slice(&f.presentation_time_us.to_le_bytes());
        log.extend_from_slice(&f.rtp_timestamp.to_le_bytes());
        log.push(f.hdr_active.into());
        log.push(f.colorspace);
        log.extend_from_slice(&(f.buffers().len() as u32).to_le_bytes());
        for (kind, bytes) in f.buffer_slices() {
            log.push(kind as u8);
            log.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            log.extend_from_slice(bytes);
        }
    }
}

impl VideoSink for LogSink {
    fn frame(&mut self, frame: Frame) -> Result<(), Frame> {
        if self.queue.len() == self.bound {
            return Err(frame);
        }
        self.frame_log(&frame);
        self.queue.push_back(frame);
        Ok(())
    }

    fn flush_frames(&mut self) {
        self.log.push(b'X');
        self.queue.clear();
    }

    fn signal(&mut self, signal: VideoSignal) {
        let log = &mut self.log;
        match signal {
            VideoSignal::RequestIdr => log.push(b'I'),
            VideoSignal::InvalidateFrames { first, last } => {
                log.push(b'R');
                log.extend_from_slice(&first.to_le_bytes());
                log.extend_from_slice(&last.to_le_bytes());
            }
            VideoSignal::FecStatus(status) => {
                log.push(b'S');
                log.extend_from_slice(&status.to_bytes());
            }
            VideoSignal::FrameReceived { frame, ltr } => {
                log.push(b'C');
                log.extend_from_slice(&frame.to_le_bytes());
                log.push(ltr.into());
            }
            VideoSignal::FrameSeen { frame } => {
                log.push(b'W');
                log.extend_from_slice(&frame.to_le_bytes());
            }
            // Nightjar's own FEC report; the C core has no counterpart.
            VideoSignal::FecBlock(_) => {}
        }
    }
}

pub struct RustVideo {
    receiver: VideoReceiver,
    sink: LogSink,
}

impl RustVideo {
    pub fn new(config: VideoConfig, queue_bound: usize) -> Self {
        Self {
            receiver: VideoReceiver::new(config),
            sink: LogSink {
                log: Vec::new(),
                queue: VecDeque::new(),
                bound: if queue_bound == 0 { 15 } else { queue_bound },
            },
        }
    }

    pub fn receive(&mut self, datagram: &[u8], now_us: u64) {
        self.receiver.receive(datagram, now_us, &mut self.sink);
    }

    /// Completes queued frames like the decoder thread: decoded, or needing a key frame.
    pub fn complete_all(&mut self, ok: bool) {
        while let Some(frame) = self.sink.queue.pop_front() {
            if ok {
                self.receiver.frame_decoded(frame.frame_type);
            } else {
                self.receiver.request_decoder_refresh(&mut self.sink);
            }
            self.receiver.recycle(frame);
        }
    }

    pub fn set_hdr(&mut self, active: bool) {
        self.receiver.set_hdr_active(active);
    }

    pub fn current_frame(&self) -> u32 {
        self.receiver.current_frame_number()
    }

    pub fn stats(&self) -> (u32, u32) {
        let stats = self.receiver.stats();
        (stats.packet_count_video, stats.packet_count_fec)
    }

    pub fn take_log(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.sink.log)
    }
}

pub struct RustAudio {
    receiver: AudioReceiver,
    log: Vec<u8>,
}

impl RustAudio {
    pub fn new(config: AudioConfig) -> Self {
        Self {
            receiver: AudioReceiver::new(config),
            log: Vec::new(),
        }
    }

    pub fn receive(&mut self, datagram: &[u8], now_us: u64) {
        let log = &mut self.log;
        self.receiver.receive(datagram, now_us, &mut |out| {
            log.push(b'A');
            match out {
                AudioOutput::Packet(opus) => {
                    log.extend_from_slice(&(opus.len() as u32).to_le_bytes());
                    log.extend_from_slice(opus);
                }
                AudioOutput::Lost => log.extend_from_slice(&0u32.to_le_bytes()),
            }
        });
    }

    pub fn timeout(&mut self) {
        self.receiver.receive_timeout();
    }

    pub fn stats(&self) -> [u32; 7] {
        let s = self.receiver.stats();
        [
            s.packet_count_audio,
            s.packet_count_fec,
            s.packet_count_fec_recovered,
            s.packet_count_fec_failed,
            s.packet_count_oos,
            s.packet_count_invalid,
            s.packet_count_fec_invalid,
        ]
    }

    pub fn take_log(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.log)
    }
}
