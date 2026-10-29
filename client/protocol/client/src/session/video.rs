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

use super::net::{self, NextSocket, PingHandle, Pinger, REBIND_WAIT, UDP_RECV_POLL_TIMEOUT};
use super::trace::PacketTrace;
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

/// Early chunks queued at most: a frame is at most 4092 packets, so this holds a few frames
/// when the decoder falls behind. A chunk over the bound is dropped, which the decoder finds
/// from the frame (see [`VideoDecoder::chunk`]).
const CHUNK_QUEUE_BOUND: usize = 16384;

/// Chunk buffers the decoder thread collects before it hands them back to the queue.
const RETURN_BATCH: usize = 32;

/// One packet of a records frame in flight, see [`VideoSink::chunk`].
pub struct Chunk {
    pub first: bool,
    pub flags: u32,
    pub data: Vec<u8>,
}

/// What the decoder thread takes from the queue, in the order the receiver produced it.
pub enum Item {
    Frame(Frame),
    Chunk(Chunk),
}

#[derive(Default)]
struct QueueState {
    items: VecDeque<Item>,
    /// Frames among `items`, which the bound counts.
    frames: usize,
    chunks: usize,
    /// Chunk payload buffers handed back by the decoder thread.
    spare: Vec<Vec<u8>>,
    shutdown: bool,
}

impl QueueState {
    /// Keeps the buffers the decoder is done with for the next chunks, up to the bound.
    fn take_spares(&mut self, returned: &mut Vec<Vec<u8>>) {
        let room = CHUNK_QUEUE_BOUND.saturating_sub(self.spare.len());
        returned.truncate(room);
        self.spare.append(returned);
    }
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
        if state.shutdown || state.frames >= DECODE_UNIT_QUEUE_BOUND {
            return Err(frame);
        }
        state.items.push_back(Item::Frame(frame));
        state.frames += 1;
        self.ready.notify_one();
        Ok(())
    }

    /// Queues an early chunk behind everything queued so far. Returns false when it was
    /// dropped over [`CHUNK_QUEUE_BOUND`], which the caller counts.
    pub fn offer_chunk(&self, first: bool, flags: u32, payload: &[u8]) -> bool {
        let mut state = lock(&self.state);
        if state.shutdown {
            return true;
        }
        if state.chunks >= CHUNK_QUEUE_BOUND {
            return false;
        }
        let mut data = state.spare.pop().unwrap_or_default();
        data.clear();
        data.extend_from_slice(payload);
        // The decoder thread only sleeps on an empty queue.
        let wake = state.items.is_empty();
        state.items.push_back(Item::Chunk(Chunk { first, flags, data }));
        state.chunks += 1;
        drop(state);
        if wake {
            self.ready.notify_one();
        }
        true
    }

    /// Drops every queued frame and chunk, returning the frames. The queue keeps its
    /// capacity, and the chunk buffers go back to the spares.
    pub fn flush(&self) -> Vec<Frame> {
        let mut state = lock(&self.state);
        state.frames = 0;
        state.chunks = 0;
        let QueueState { items, spare, .. } = &mut *state;
        let mut frames = Vec::new();
        for item in items.drain(..) {
            match item {
                Item::Frame(frame) => frames.push(frame),
                Item::Chunk(chunk) => {
                    if spare.len() < CHUNK_QUEUE_BOUND {
                        spare.push(chunk.data);
                    }
                }
            }
        }
        frames
    }

    /// The next frame or chunk, or `None` once shut down. The chunk buffers in `returned` go
    /// back to the spares under the lock this takes anyway: once [`RETURN_BATCH`] have
    /// gathered, and before sleeping on an empty queue.
    pub fn wait(&self, returned: &mut Vec<Vec<u8>>) -> Option<Item> {
        let mut state = lock(&self.state);
        loop {
            if state.shutdown {
                return None;
            }
            if returned.len() >= RETURN_BATCH {
                state.take_spares(returned);
            }
            if let Some(item) = state.items.pop_front() {
                match item {
                    Item::Frame(_) => state.frames -= 1,
                    Item::Chunk(_) => state.chunks -= 1,
                }
                return Some(item);
            }
            state.take_spares(returned);
            state = self.ready.wait(state).unwrap_or_else(|p| p.into_inner());
        }
    }

    pub fn len(&self) -> usize {
        lock(&self.state).frames
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
    /// Early chunks dropped because the decoder fell behind (the decoder then decodes the
    /// frame whole).
    pub chunks_dropped: AtomicU64,
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

    fn chunk(&mut self, first: bool, flags: u32, payload: &[u8]) {
        // PyroWave never submits directly, so a direct decoder has asked for no chunks.
        if self.direct.is_none() && !self.shared.queue.offer_chunk(first, flags, payload) {
            self.shared.counters.chunks_dropped.fetch_add(1, Ordering::Relaxed);
        }
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
                VideoSignal::FecBlock(block) => session.fec_block(block, now),
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
    /// Where a roam's rebind leaves the receive thread's next socket.
    pub next_socket: Arc<NextSocket>,
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
        let next_socket = start.next_socket.clone();
        let receive_control = control.clone();
        let receive_shared = shared.clone();
        let receive_stop = stop.clone();
        let receive_session = session.clone();
        stream.receive = Some(std::thread::Builder::new().name("VideoRecv".into()).spawn(
            move || {
                receive_session.prioritize_thread();
                receive_thread(
                    socket,
                    next_socket,
                    receive_shared,
                    receive_session,
                    receive_control,
                    receive_stop,
                    direct,
                )
            },
        )?);
        if let Some(decoder) = queued {
            let decode_session = session.clone();
            stream.decode = Some(std::thread::Builder::new().name("VideoDec".into()).spawn(
                move || {
                    decode_session.prioritize_decode_thread();
                    decoder_thread(decoder, shared, control)
                },
            )?);
        }
        stream.pinger = Some(Pinger::start(
            "VideoPing",
            &start.socket,
            start.target,
            start.ping_payload,
        )?);
        Ok(stream)
    }

    /// Moves the pings to another socket, for a roam.
    pub fn ping_handle(&self) -> Option<PingHandle> {
        self.pinger.as_ref().map(Pinger::handle)
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
    mut socket: UdpSocket,
    next_socket: Arc<NextSocket>,
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
    let mut trace = PacketTrace::from_env("video", true);
    while !stop.load(Ordering::Acquire) {
        let before = if trace.is_some() { now_us() } else { 0 };
        let length = match net::receive(&socket, &mut buffer) {
            Ok(Some(length)) => length,
            Ok(None) => {
                if let Some(next) = next_socket.take() {
                    session.log("Video moved to the roam's new socket");
                    socket = next;
                }
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
                // The old socket's address went away with the network, maybe ahead of the
                // rebind: wait a while for the roam's new socket, then fail.
                if let Some(next) = next_socket.wait(REBIND_WAIT, &stop) {
                    session.log(&format!(
                        "Video receive failed on the old socket, moved to the roam's new one: {e}"
                    ));
                    socket = next;
                    continue;
                }
                if stop.load(Ordering::Acquire) {
                    break;
                }
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
        let now = now_us();
        if let Some(trace) = trace.as_mut() {
            trace.push(before, now, &buffer[..length]);
        }
        {
            let mut receiver = lock(&shared.receiver);
            let mut sink = Sink {
                shared: &shared,
                direct: direct.as_mut(),
                signals: &mut signals,
            };
            receiver.receive(&buffer[..length], now, &mut sink);
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
    if let Some(trace) = trace {
        trace.finish(|line| session.log(line));
    }
    direct.map(|d| d.decoder)
}

fn decoder_thread(
    decoder: Box<dyn VideoDecoder>,
    shared: Arc<VideoShared>,
    control: ControlHandle,
) -> Box<dyn VideoDecoder> {
    decode_loop(decoder, &shared, |signals| route(&control, signals))
}

/// The decoder thread's loop, with the routing of the receiver's signals left to the caller.
fn decode_loop(
    mut decoder: Box<dyn VideoDecoder>,
    shared: &VideoShared,
    mut route_signals: impl FnMut(&mut Vec<VideoSignal>),
) -> Box<dyn VideoDecoder> {
    let mut signals = Vec::new();
    // Chunk buffers go back to the queue with the next wait, in one lock.
    let mut returned: Vec<Vec<u8>> = Vec::with_capacity(RETURN_BATCH);
    while let Some(item) = shared.queue.wait(&mut returned) {
        let frame = match item {
            Item::Chunk(chunk) => {
                decoder.chunk(chunk.first, chunk.flags, &chunk.data);
                returned.push(chunk.data);
                continue;
            }
            Item::Frame(frame) => frame,
        };
        shared.counters.submitted.fetch_add(1, Ordering::Relaxed);
        let status = decoder.submit(&frame);
        {
            let mut receiver = lock(&shared.receiver);
            match status {
                DecodeStatus::NeedIdr => {
                    shared.counters.need_idr.fetch_add(1, Ordering::Relaxed);
                    let mut sink = Sink {
                        shared,
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
        route_signals(&mut signals);
    }
    decoder
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::tests::early_chunks::{records, Blocks};
    use std::sync::mpsc;

    /// How long a test waits for something it caused before calling it a hang. Only a hang
    /// runs into it: no test passes or fails on how fast a thread gets scheduled.
    const HANG_GUARD: Duration = Duration::from_secs(60);

    /// Waits for `done`, polling: for state another thread changes without signalling the test.
    fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + HANG_GUARD;
        while !done() {
            assert!(Instant::now() < deadline, "{what}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn next_item(queue: &FrameQueue) -> Item {
        queue.wait(&mut Vec::new()).expect("an item")
    }

    #[test]
    fn the_queue_is_bounded_and_shuts_down() {
        let queue = FrameQueue::default();
        for _ in 0..DECODE_UNIT_QUEUE_BOUND {
            assert!(queue.offer(Frame::default()).is_ok());
        }
        assert!(queue.offer(Frame::default()).is_err());
        assert_eq!(queue.flush().len(), DECODE_UNIT_QUEUE_BOUND);
        queue.offer(Frame::default()).unwrap();
        assert!(queue.wait(&mut Vec::new()).is_some());
        queue.shutdown();
        assert!(queue.wait(&mut Vec::new()).is_none());
        assert!(queue.offer(Frame::default()).is_err());
    }

    #[test]
    fn chunks_have_their_own_bound_and_a_drop_is_reported() {
        let queue = FrameQueue::default();
        for i in 0..CHUNK_QUEUE_BOUND {
            assert!(queue.offer_chunk(i == 0, 2, &[1, 2, 3]));
        }
        assert!(!queue.offer_chunk(false, 2, &[9]), "over the bound is a drop");
        assert!(queue.offer(Frame::default()).is_ok(), "frames are bounded apart from chunks");
        assert_eq!(queue.len(), 1, "len counts frames");
        assert!(matches!(next_item(&queue), Item::Chunk(c) if c.first && c.data == [1, 2, 3]));
        assert!(queue.offer_chunk(false, 0, &[4]), "room again once one is taken");
        assert!(!queue.offer_chunk(false, 0, &[5]));
        queue.shutdown();
        assert!(queue.offer_chunk(false, 0, &[6]), "after shutdown nothing counts as dropped");
    }

    #[test]
    fn returned_buffers_come_back_in_batches_and_are_reused() {
        let queue = FrameQueue::default();
        for _ in 0..4 {
            assert!(queue.offer_chunk(false, 0, &[1]));
        }
        let spares = || lock(&queue.state).spare.len();
        let mut back: Vec<Vec<u8>> = (0..RETURN_BATCH - 1)
            .map(|_| Vec::with_capacity(2048))
            .collect();
        queue.wait(&mut back);
        assert_eq!((back.len(), spares()), (RETURN_BATCH - 1, 0), "fewer than a batch stay");
        back.push(Vec::with_capacity(2048));
        queue.wait(&mut back);
        assert_eq!((back.len(), spares()), (0, RETURN_BATCH), "a batch goes back in one lock");
        // Two of the four are left; the next chunk takes a returned buffer.
        queue.offer_chunk(false, 0, &[7]);
        assert_eq!(spares(), RETURN_BATCH - 1);
        let mut last = None;
        for _ in 0..3 {
            last = queue.wait(&mut Vec::new());
        }
        let Some(Item::Chunk(chunk)) = last else {
            panic!("a chunk")
        };
        assert_eq!(chunk.data, [7]);
        assert!(chunk.data.capacity() >= 2048, "an offered chunk reuses a returned buffer");
        // The spares are bounded.
        let mut many: Vec<Vec<u8>> = (0..CHUNK_QUEUE_BOUND + 10).map(|_| Vec::new()).collect();
        queue.offer_chunk(false, 0, &[8]);
        queue.wait(&mut many);
        assert!(many.is_empty());
        assert_eq!(spares(), CHUNK_QUEUE_BOUND);
    }

    #[test]
    fn a_sleeping_waiter_hands_back_what_it_holds_and_wakes_for_a_chunk() {
        let queue = Arc::new(FrameQueue::default());
        let (tx, rx) = mpsc::channel();
        let waiter = {
            let queue = queue.clone();
            std::thread::spawn(move || {
                let mut back = vec![Vec::new(), Vec::new(), Vec::new()];
                let item = queue.wait(&mut back);
                tx.send((item.is_some(), back.len())).unwrap();
            })
        };
        // It sleeps on the empty queue after giving its three buffers back.
        wait_until("the waiter never handed its buffers back", || {
            lock(&queue.state).spare.len() == 3
        });
        assert!(queue.offer_chunk(true, 0, &[1]));
        assert_eq!(rx.recv_timeout(HANG_GUARD).unwrap(), (true, 0));
        waiter.join().unwrap();
    }

    #[test]
    fn a_flush_drops_queued_chunks_with_the_frames_and_keeps_the_capacity() {
        let queue = FrameQueue::default();
        for round in 0..3 {
            queue.offer_chunk(round == 0, 0, &[1, 2]);
        }
        queue.offer(Frame::default()).unwrap();
        queue.offer_chunk(true, 0, &[3]);
        queue.offer_chunk(false, 0, &[4]);
        let capacity = lock(&queue.state).items.capacity();
        let frames = queue.flush();
        assert_eq!(frames.len(), 1, "only the frame comes back");
        {
            let state = lock(&queue.state);
            assert_eq!((state.frames, state.chunks, state.items.len()), (0, 0, 0));
            assert_eq!(state.items.capacity(), capacity, "the off path keeps its allocation");
            assert_eq!(state.spare.len(), 5, "the chunk buffers are kept");
        }
        assert_eq!(queue.len(), 0);
        // The chunk count was reset with it.
        for i in 0..CHUNK_QUEUE_BOUND {
            assert!(queue.offer_chunk(i == 0, 0, &[1]), "chunk {i}");
        }
        assert!(!queue.offer_chunk(false, 0, &[1]));
        assert!(queue.flush().is_empty());
        assert!(queue.offer_chunk(true, 0, &[1]));
    }

    #[test]
    fn a_flush_on_the_off_path_allocates_no_frame_list() {
        let queue = FrameQueue::default();
        assert_eq!(queue.flush().capacity(), 0);
        queue.offer(Frame::default()).unwrap();
        assert_eq!(queue.flush().len(), 1);
    }

    /// What the mock decoder saw, in order.
    #[derive(Debug, PartialEq)]
    enum Seen {
        Chunk { first: bool },
        /// A frame, and whether the chunks since the last first add up to its envelope.
        Frame { number: u32, whole: bool },
    }

    /// Accumulates the chunks the way the desktop decoder checks them against the frame.
    struct Recorder {
        seen: mpsc::Sender<Seen>,
        envelope: Vec<u8>,
        status: DecodeStatus,
    }

    impl Recorder {
        fn new(seen: mpsc::Sender<Seen>) -> Self {
            Self {
                seen,
                envelope: Vec::new(),
                status: DecodeStatus::Ok,
            }
        }
    }

    impl VideoDecoder for Recorder {
        fn setup(&mut self, _: &crate::session::VideoSetup) -> Result<(), i32> {
            Ok(())
        }

        fn wants_early_chunks(&self) -> bool {
            true
        }

        fn chunk(&mut self, first: bool, flags: u32, payload: &[u8]) {
            if first {
                self.envelope = vec![0; 4];
            }
            self.envelope
                .extend_from_slice(&(payload.len() as u32).to_le_bytes());
            self.envelope.extend_from_slice(&flags.to_le_bytes());
            self.envelope.extend_from_slice(payload);
            self.seen.send(Seen::Chunk { first }).unwrap();
        }

        fn submit(&mut self, frame: &Frame) -> DecodeStatus {
            let whole = self.envelope == frame.data();
            self.envelope.clear();
            self.seen
                .send(Seen::Frame {
                    number: frame.number,
                    whole,
                })
                .unwrap();
            std::mem::replace(&mut self.status, DecodeStatus::Ok)
        }
    }

    /// The decoder thread's loop on a thread of its own.
    struct Running {
        shared: Arc<VideoShared>,
        seen: mpsc::Receiver<Seen>,
        thread: JoinHandle<Box<dyn VideoDecoder>>,
    }

    impl Running {
        fn start(shared: &Arc<VideoShared>, recorder: Recorder, seen: mpsc::Receiver<Seen>) -> Self {
            let for_thread = shared.clone();
            Self {
                shared: shared.clone(),
                seen,
                thread: std::thread::spawn(move || {
                    decode_loop(Box::new(recorder), &for_thread, |_| {})
                }),
            }
        }

        /// Stops the decoder once it has taken everything queued, and returns all it saw. The
        /// tests queue everything before this, and the one item the decoder may still be
        /// handling when the queue runs empty is finished before it sees the shutdown.
        fn finish(self) -> Vec<Seen> {
            wait_until("the decoder never took everything queued", || {
                lock(&self.shared.queue.state).items.is_empty()
            });
            self.shared.queue.shutdown();
            self.thread.join().unwrap();
            self.seen.try_iter().collect()
        }
    }

    fn feed(shared: &VideoShared, packets: &[Vec<u8>]) {
        let mut signals = Vec::new();
        for packet in packets {
            let mut receiver = lock(&shared.receiver);
            let mut sink = Sink {
                shared,
                direct: None,
                signals: &mut signals,
            };
            receiver.receive(packet, 7, &mut sink);
        }
    }

    fn refresh(shared: &VideoShared) {
        let mut signals = Vec::new();
        let mut sink = Sink {
            shared,
            direct: None,
            signals: &mut signals,
        };
        lock(&shared.receiver).request_decoder_refresh(&mut sink);
    }

    fn flat(blocks: Vec<Vec<Vec<u8>>>) -> Vec<Vec<u8>> {
        blocks.into_iter().flatten().collect()
    }

    fn frames_seen(seen: &[Seen]) -> Vec<(u32, bool)> {
        seen.iter()
            .filter_map(|s| match s {
                Seen::Frame { number, whole } => Some((*number, *whole)),
                Seen::Chunk { .. } => None,
            })
            .collect()
    }

    #[test]
    fn the_decoder_thread_gets_every_chunk_ahead_of_its_frame() {
        let shared = Arc::new(VideoShared::new(records(true)));
        let (tx, rx) = mpsc::channel();
        let running = Running::start(&shared, Recorder::new(tx), rx);
        let mut host = Blocks::new();
        for n in 0..6u8 {
            feed(&shared, &flat(host.frame(&[12, 9, 5], n)));
        }
        let seen = running.finish();
        let frames = frames_seen(&seen);
        assert_eq!(frames.len(), 6, "{seen:?}");
        assert!(frames.iter().all(|(_, whole)| *whole), "{seen:?}");
        // Parity packets are no chunks: 26 data packets a frame.
        let chunks = seen
            .iter()
            .filter(|s| matches!(s, Seen::Chunk { .. }))
            .count();
        assert_eq!(chunks, 6 * 26);
        assert_eq!(shared.counters.chunks_dropped.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn losses_in_the_stream_still_end_in_whole_frames() {
        let shared = Arc::new(VideoShared::new(records(true)));
        let (tx, rx) = mpsc::channel();
        let running = Running::start(&shared, Recorder::new(tx), rx);
        let mut host = Blocks::new();
        for n in 0..8u8 {
            let packets = flat(host.frame(&[16, 11], n));
            // One data packet of each block lost; its parity recovers it. The first block
            // has 20 datagrams, 16 of them data.
            let lost = if n % 2 == 0 { [3usize, 24] } else { [7, 29] };
            let arrived: Vec<Vec<u8>> = packets
                .iter()
                .enumerate()
                .filter(|(i, _)| !lost.contains(i))
                .map(|(_, p)| p.clone())
                .collect();
            feed(&shared, &arrived);
        }
        let seen = running.finish();
        let frames = frames_seen(&seen);
        assert_eq!(frames.len(), 8, "{seen:?}");
        assert!(frames.iter().all(|(_, whole)| *whole), "{seen:?}");
    }

    #[test]
    fn a_refresh_in_the_middle_of_a_frame_leaves_it_to_be_caught_from_its_bytes() {
        let shared = Arc::new(VideoShared::new(records(true)));
        let mut host = Blocks::new();
        feed(&shared, &flat(host.frame(&[8, 6], 1)));
        // The next frame's first block arrives, then the decoder is refreshed (a decode error,
        // say): the queue drops everything it holds, the first chunk of this frame included.
        let two = host.frame(&[8, 6], 2);
        feed(&shared, &two[0]);
        refresh(&shared);
        feed(&shared, &two[1]);
        feed(&shared, &flat(host.frame(&[8, 6], 3)));
        // The decoder only starts now, so it sees the queue as the refresh left it.
        let (tx, rx) = mpsc::channel();
        let running = Running::start(&shared, Recorder::new(tx), rx);
        let seen = running.finish();
        let verdicts = frames_seen(&seen);
        let (last, whole) = *verdicts.last().expect("a frame after the refresh");
        assert!(whole, "the frame after the refresh is whole: {seen:?}");
        for (number, whole) in &verdicts {
            if *number < last {
                assert!(!whole, "frame {number} lost its first chunk in the flush: {seen:?}");
            }
        }
        // Nothing flushed reached the decoder: the one first chunk is the last frame's.
        let firsts = seen
            .iter()
            .filter(|s| matches!(s, Seen::Chunk { first: true }))
            .count();
        assert_eq!(firsts, 1, "{seen:?}");
    }

    #[test]
    fn a_decode_error_is_counted_after_the_chunks_it_had() {
        let shared = Arc::new(VideoShared::new(records(true)));
        let (tx, rx) = mpsc::channel();
        let mut recorder = Recorder::new(tx);
        recorder.status = DecodeStatus::NeedIdr;
        let running = Running::start(&shared, recorder, rx);
        let mut host = Blocks::new();
        feed(&shared, &flat(host.frame(&[8, 6], 1)));
        let seen = running.finish();
        assert!(matches!(seen.last(), Some(Seen::Frame { whole: true, .. })), "{seen:?}");
        assert_eq!(shared.counters.need_idr.load(Ordering::Relaxed), 1);
    }
}
