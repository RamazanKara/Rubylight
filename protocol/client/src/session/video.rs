//! The video stream's threads: UDP receive with pings and the startup timeouts, the 15-frame
//! decode-unit queue and its decoder thread (or direct submission on the receive thread),
//! and the routing of [`VideoSignal`]s to the control stream.
//!
//! Ported from moonlight-common-c's `VideoStream.c` and the queue handling of
//! `VideoDepacketizer.c` (GPL-3.0, Cameron Gutman and contributors).

use std::collections::VecDeque;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::net::{self, Pinger, UDP_RECV_POLL_TIMEOUT};
use super::{error, now_us, DecodeStatus, Shared, VideoDecoder};
use crate::control::driver::ControlHandle;
use crate::control::FrameFecStatus;
use crate::video::{Frame, FrameType, VideoReceiver, VideoSignal, VideoSink};

/// `decodeUnitQueue`'s bound.
pub const DECODE_UNIT_QUEUE_BOUND: usize = 15;
/// No video packet, or no complete key frame after the first packet, for this long ends the
/// stream (`FIRST_FRAME_TIMEOUT_SEC`).
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(10);
/// Large enough that no datagram is truncated (Windows fails a truncating receive); the
/// receiver cuts datagrams to its own maximum.
const RECEIVE_BUFFER: usize = 65536;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

#[derive(Default)]
struct QueueState {
    frames: VecDeque<Frame>,
    shutdown: bool,
}

/// The bounded decode-unit queue between the receive and decoder threads.
#[derive(Default)]
pub struct FrameQueue {
    state: Mutex<QueueState>,
    ready: Condvar,
}

impl FrameQueue {
    /// Queues a frame, or hands it back when the queue holds [`DECODE_UNIT_QUEUE_BOUND`].
    pub fn offer(&self, frame: Frame) -> Result<(), Frame> {
        let mut state = lock(&self.state);
        if state.shutdown || state.frames.len() >= DECODE_UNIT_QUEUE_BOUND {
            return Err(frame);
        }
        state.frames.push_back(frame);
        self.ready.notify_one();
        Ok(())
    }

    pub fn flush(&self) -> Vec<Frame> {
        lock(&self.state).frames.drain(..).collect()
    }

    /// The next frame, or `None` once shut down.
    pub fn wait(&self) -> Option<Frame> {
        let mut state = lock(&self.state);
        loop {
            if state.shutdown {
                return None;
            }
            if let Some(frame) = state.frames.pop_front() {
                return Some(frame);
            }
            state = self.ready.wait(state).unwrap_or_else(|p| p.into_inner());
        }
    }

    pub fn len(&self) -> usize {
        lock(&self.state).frames.len()
    }

    pub fn shutdown(&self) {
        lock(&self.state).shutdown = true;
        self.ready.notify_all();
    }
}

/// Counters the session reports.
#[derive(Default)]
pub struct VideoCounters {
    pub submitted: AtomicU64,
    pub need_idr: AtomicU64,
    pub queue_overflows: AtomicU64,
}

/// State the receive thread, the decoder thread and the control thread share.
pub struct VideoShared {
    pub receiver: Mutex<VideoReceiver>,
    pub queue: FrameQueue,
    pub counters: VideoCounters,
    /// `receivedFullFrame`: a key frame was reassembled.
    key_frame: AtomicBool,
}

impl VideoShared {
    pub fn new(receiver: VideoReceiver) -> Self {
        Self {
            receiver: Mutex::new(receiver),
            queue: FrameQueue::default(),
            counters: VideoCounters::default(),
            key_frame: AtomicBool::new(false),
        }
    }
}

/// The decoder when it runs on the receive thread (`CAPABILITY_DIRECT_SUBMIT`), and the
/// feedback that waits until the receiver is free again.
struct Direct {
    decoder: Box<dyn VideoDecoder>,
    decoded_key_frame: bool,
    need_refresh: bool,
    done: Vec<Frame>,
}

struct Sink<'a> {
    shared: &'a VideoShared,
    direct: Option<&'a mut Direct>,
    signals: &'a mut Vec<VideoSignal>,
}

impl VideoSink for Sink<'_> {
    fn frame(&mut self, frame: Frame) -> Result<(), Frame> {
        let key_frame = frame.frame_type == FrameType::Idr;
        if key_frame {
            self.shared.key_frame.store(true, Ordering::Relaxed);
        }
        let Some(direct) = self.direct.as_deref_mut() else {
            let result = self.shared.queue.offer(frame);
            if result.is_err() {
                self.shared
                    .counters
                    .queue_overflows
                    .fetch_add(1, Ordering::Relaxed);
            }
            return result;
        };
        self.shared
            .counters
            .submitted
            .fetch_add(1, Ordering::Relaxed);
        match direct.decoder.submit(&frame) {
            DecodeStatus::Ok => direct.decoded_key_frame |= key_frame,
            DecodeStatus::NeedIdr => {
                self.shared
                    .counters
                    .need_idr
                    .fetch_add(1, Ordering::Relaxed);
                direct.need_refresh = true;
            }
        }
        direct.done.push(frame);
        Ok(())
    }

    fn flush_frames(&mut self) {
        drop(self.shared.queue.flush());
    }

    fn signal(&mut self, signal: VideoSignal) {
        self.signals.push(signal);
    }
}

fn fec_status(status: crate::video::FecStatus) -> FrameFecStatus {
    FrameFecStatus {
        frame_index: status.frame_index,
        highest_received_sequence: status.highest_received_sequence,
        next_contiguous_sequence: status.next_contiguous_sequence,
        missing_before_highest_received: status.missing_before_highest,
        total_data_packets: status.total_data_packets,
        total_parity_packets: status.total_parity_packets,
        received_data_packets: status.received_data_packets,
        received_parity_packets: status.received_parity_packets,
        fec_percentage: status.fec_percentage,
        multi_fec_block_index: status.block_index,
        multi_fec_block_count: status.block_count,
    }
}

/// Hands the receiver's signals to the control stream: key frame requests, invalidations
/// (RFI, or a key frame request without it), FEC reports (queued for Sunshine hosts only)
/// and the per-frame quality and LTR feedback. Only signals that send something wake the
/// control thread.
pub fn route(control: &ControlHandle, signals: &mut Vec<VideoSignal>) {
    if signals.is_empty() {
        return;
    }
    let urgent = signals.iter().any(|s| {
        matches!(
            s,
            VideoSignal::RequestIdr
                | VideoSignal::InvalidateFrames { .. }
                | VideoSignal::FrameReceived { ltr: true, .. }
        )
    });
    let now = Instant::now();
    let apply = |session: &mut crate::control::ControlSession| {
        for signal in signals.drain(..) {
            match signal {
                VideoSignal::RequestIdr => session.request_idr_frame(),
                VideoSignal::InvalidateFrames { first, last } => session.frame_loss(first, last),
                VideoSignal::FecStatus(status) => session.frame_fec_status(fec_status(status)),
                VideoSignal::FrameReceived { frame, ltr } => session.frame_complete(frame, ltr),
                VideoSignal::FrameSeen { frame } => session.frame_seen(frame, now),
            }
        }
    };
    if urgent {
        control.with_session(apply);
    } else {
        control.with_session_quiet(apply);
    }
}

/// What the video stream needs from the session to run.
pub struct VideoStart {
    pub socket: UdpSocket,
    pub target: std::net::SocketAddr,
    pub ping_payload: Option<[u8; 16]>,
    pub direct_submit: bool,
}

/// The running video stream.
pub struct VideoStream {
    shared: Arc<VideoShared>,
    stop: Arc<AtomicBool>,
    pinger: Option<Pinger>,
    receive: Option<JoinHandle<Option<Box<dyn VideoDecoder>>>>,
    decode: Option<JoinHandle<Box<dyn VideoDecoder>>>,
}

impl VideoStream {
    /// Starts the receive thread (and the decoder thread without direct submission), then
    /// the pings. The decoder is set up and started.
    pub fn start(
        start: VideoStart,
        decoder: Box<dyn VideoDecoder>,
        shared: Arc<VideoShared>,
        session: Arc<Shared>,
        control: ControlHandle,
    ) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let (direct, queued) = if start.direct_submit {
            (
                Some(Direct {
                    decoder,
                    decoded_key_frame: false,
                    need_refresh: false,
                    done: Vec::new(),
                }),
                None,
            )
        } else {
            (None, Some(decoder))
        };
        let mut stream = Self {
            shared: shared.clone(),
            stop: stop.clone(),
            pinger: None,
            receive: None,
            decode: None,
        };
        let socket = start.socket.try_clone()?;
        let receive_control = control.clone();
        let receive_shared = shared.clone();
        let receive_stop = stop.clone();
        let receive_session = session.clone();
        stream.receive = Some(std::thread::Builder::new().name("VideoRecv".into()).spawn(
            move || {
                receive_thread(
                    socket,
                    receive_shared,
                    receive_session,
                    receive_control,
                    receive_stop,
                    direct,
                )
            },
        )?);
        if let Some(decoder) = queued {
            stream.decode = Some(
                std::thread::Builder::new()
                    .name("VideoDec".into())
                    .spawn(move || decoder_thread(decoder, shared, control))?,
            );
        }
        stream.pinger = Some(Pinger::start(
            "VideoPing",
            &start.socket,
            start.target,
            start.ping_payload,
        )?);
        Ok(stream)
    }

    /// Stops and joins the threads and returns the decoder for `stop` and `cleanup`.
    pub fn stop(mut self) -> Option<Box<dyn VideoDecoder>> {
        self.stop.store(true, Ordering::Release);
        self.shared.queue.shutdown();
        if let Some(mut pinger) = self.pinger.take() {
            pinger.stop();
        }
        let mut decoder = self.receive.take().and_then(|t| t.join().ok()).flatten();
        if let Some(thread) = self.decode.take() {
            decoder = thread.join().ok().or(decoder);
        }
        decoder
    }
}

fn receive_thread(
    socket: UdpSocket,
    shared: Arc<VideoShared>,
    session: Arc<Shared>,
    control: ControlHandle,
    stop: Arc<AtomicBool>,
    mut direct: Option<Direct>,
) -> Option<Box<dyn VideoDecoder>> {
    let mut buffer = vec![0u8; RECEIVE_BUFFER];
    let mut signals = Vec::new();
    let mut waiting = Duration::ZERO;
    let mut first_data: Option<Instant> = None;
    while !stop.load(Ordering::Acquire) {
        let length = match net::receive(&socket, &mut buffer) {
            Ok(Some(length)) => length,
            Ok(None) => {
                if first_data.is_none() {
                    waiting += UDP_RECV_POLL_TIMEOUT;
                    if waiting >= FIRST_FRAME_TIMEOUT {
                        session.log("Terminating connection due to lack of video traffic");
                        session.terminate(error::NO_VIDEO_TRAFFIC);
                        break;
                    }
                }
                continue;
            }
            Err(e) => {
                session.log(&format!("Video receive failed: {e}"));
                session.terminate(e.raw_os_error().unwrap_or(-1));
                break;
            }
        };
        let first = *first_data.get_or_insert_with(|| {
            session.log(&format!(
                "Received first video packet after {} ms",
                waiting.as_millis()
            ));
            Instant::now()
        });
        if !shared.key_frame.load(Ordering::Relaxed) && first.elapsed() >= FIRST_FRAME_TIMEOUT {
            session.log("Terminating connection due to lack of a successful video frame");
            session.terminate(error::NO_VIDEO_FRAME);
            break;
        }
        {
            let mut receiver = lock(&shared.receiver);
            let mut sink = Sink {
                shared: &shared,
                direct: direct.as_mut(),
                signals: &mut signals,
            };
            receiver.receive(&buffer[..length], now_us(), &mut sink);
            if let Some(d) = direct.as_mut() {
                if std::mem::take(&mut d.decoded_key_frame) {
                    receiver.frame_decoded(FrameType::Idr);
                }
                if std::mem::take(&mut d.need_refresh) {
                    let mut sink = Sink {
                        shared: &shared,
                        direct: None,
                        signals: &mut signals,
                    };
                    receiver.request_decoder_refresh(&mut sink);
                }
                for frame in d.done.drain(..) {
                    receiver.recycle(frame);
                }
            }
        }
        route(&control, &mut signals);
    }
    direct.map(|d| d.decoder)
}

fn decoder_thread(
    mut decoder: Box<dyn VideoDecoder>,
    shared: Arc<VideoShared>,
    control: ControlHandle,
) -> Box<dyn VideoDecoder> {
    let mut signals = Vec::new();
    while let Some(frame) = shared.queue.wait() {
        shared.counters.submitted.fetch_add(1, Ordering::Relaxed);
        let status = decoder.submit(&frame);
        {
            let mut receiver = lock(&shared.receiver);
            match status {
                DecodeStatus::NeedIdr => {
                    shared.counters.need_idr.fetch_add(1, Ordering::Relaxed);
                    let mut sink = Sink {
                        shared: &shared,
                        direct: None,
                        signals: &mut signals,
                    };
                    receiver.request_decoder_refresh(&mut sink);
                }
                DecodeStatus::Ok => {
                    if frame.frame_type == FrameType::Idr {
                        receiver.frame_decoded(FrameType::Idr);
                    }
                }
            }
            receiver.recycle(frame);
        }
        route(&control, &mut signals);
    }
    decoder
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_queue_is_bounded_and_shuts_down() {
        let queue = FrameQueue::default();
        for _ in 0..DECODE_UNIT_QUEUE_BOUND {
            assert!(queue.offer(Frame::default()).is_ok());
        }
        assert!(queue.offer(Frame::default()).is_err());
        assert_eq!(queue.flush().len(), DECODE_UNIT_QUEUE_BOUND);
        queue.offer(Frame::default()).unwrap();
        assert!(queue.wait().is_some());
        queue.shutdown();
        assert!(queue.wait().is_none());
        assert!(queue.offer(Frame::default()).is_err());
    }
}
