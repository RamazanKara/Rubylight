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
use super::trace::{PacketTrace, Stream};
use super::{error, now_us, DecodeStatus, Shared, VideoDecoder};
use crate::control::driver::ControlHandle;
use crate::control::FrameFecStatus;
use crate::video::{Frame, FrameType, VideoReceiver, VideoSignal, VideoSink};

/// `decodeUnitQueue`'s bound.
pub const DECODE_UNIT_QUEUE_BOUND: usize = 15;
/// No video packet, or no complete key frame after the first packet, for this long ends the
/// stream (`FIRST_FRAME_TIMEOUT_SEC`).
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(10);

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

    /// [`wait_behind`](Self::wait_behind) without the frame count.
    #[cfg(test)]
    pub fn wait(&self, returned: &mut Vec<Vec<u8>>) -> Option<Item> {
        self.wait_behind(returned).map(|(item, _)| item)
    }

    /// The next frame or chunk with the number of frames still queued behind it, or `None`
    /// once shut down. The chunk buffers in `returned` go back to the spares under the lock
    /// this takes anyway: once [`RETURN_BATCH`] have gathered, before sleeping on an empty
    /// queue, and with every frame, which the decoder may take a while over while the receiver
    /// needs buffers for the next frame's chunks.
    pub fn wait_behind(&self, returned: &mut Vec<Vec<u8>>) -> Option<(Item, usize)> {
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
                    Item::Frame(_) => {
                        state.frames -= 1;
                        state.take_spares(returned);
                    }
                    Item::Chunk(_) => state.chunks -= 1,
                }
                return Some((item, state.frames));
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

/// Newer frames queued behind the one the decoder thread just took at which an intra-only
/// decoder is behind for certain. One newer frame can be a pair the network delivered together,
/// which a decoder that keeps up works off within a frame; two mean every frame after it would
/// be shown at least two frame intervals late, and with a decoder slower than the stream the
/// queue would only grow until it overflows.
const SKIP_BACKLOG: usize = 2;
/// For this long after a skip a single newer frame is enough for the next one. A decoder a
/// little slower than the stream then drops one frame at a time as each falls due, instead of
/// two in a row each time the backlog reaches [`SKIP_BACKLOG`] again.
const SKIP_HOLD_US: u64 = 1_000_000;

/// Which frames of a decoder whose frames decode on their own ([`VideoDecoder::intra_only`])
/// give way to a newer one already queued, so that the decoder falling behind costs frames and
/// not latency. Without it every frame waits behind the ones before it, up to
/// [`DECODE_UNIT_QUEUE_BOUND`] decodes, until the queue overflows and is flushed. A decoder that
/// keeps up never has two newer frames queued, and nothing changes for it.
#[derive(Default)]
struct Backlog {
    /// When the last frame was skipped (session clock, us).
    skipped_us: Option<u64>,
}

impl Backlog {
    fn held(&self, now_us: u64) -> bool {
        self.skipped_us
            .is_some_and(|at| now_us.saturating_sub(at) < SKIP_HOLD_US)
    }

    /// Whether the frame just taken, with `behind` newer frames queued, is skipped.
    fn skips(&mut self, behind: usize, now_us: u64) -> bool {
        let skip = behind >= SKIP_BACKLOG || (behind >= 1 && self.held(now_us));
        if skip {
            self.skipped_us = Some(now_us);
        }
        skip
    }

    /// Whether a chunk taken with `behind` frames queued after it is of no use: the first of
    /// those frames is its own (or a later one, when its own was lost), and that one will be
    /// skipped when it is taken with the rest still behind it.
    fn skips_chunk(&self, behind: usize, now_us: u64) -> bool {
        behind > SKIP_BACKLOG || (behind >= 2 && self.held(now_us))
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
    /// Frames an intra-only decoder that had fallen behind passed over for a newer queued one
    /// (see [`Backlog`]), and the early chunks dropped with them (not `chunks_dropped`: their
    /// frames are never decoded).
    pub frames_skipped: AtomicU64,
    pub chunks_skipped: AtomicU64,
    /// The receiver's `VideoStats::decoder_drops`, published after each datagram so it can be
    /// read without the receiver's lock: a direct-submit decoder asks for it from inside its
    /// submit (nightjar#80), on the thread that takes that lock for each batch.
    pub decoder_drops: AtomicU64,
}

/// State the receive thread, the decoder thread and the control thread share.
pub struct VideoShared {
    pub receiver: Mutex<VideoReceiver>,
    /// The receiver's counters, published after each datagram and decode so that reading the
    /// stream's statistics needs no receiver lock.
    pub stats: Mutex<crate::video::VideoStats>,
    pub queue: FrameQueue,
    pub counters: VideoCounters,
    /// `receivedFullFrame`: a key frame was reassembled.
    key_frame: AtomicBool,
}

impl VideoShared {
    pub fn new(receiver: VideoReceiver) -> Self {
        Self {
            stats: Mutex::new(receiver.stats()),
            receiver: Mutex::new(receiver),
            queue: FrameQueue::default(),
            counters: VideoCounters::default(),
            key_frame: AtomicBool::new(false),
        }
    }
}

/// The decoder when it runs on the receive thread (`CAPABILITY_DIRECT_SUBMIT`): the frames the
/// receiver completed, submitted once its lock is released, and the feedback that waits for
/// the next lock.
struct Direct {
    decoder: Box<dyn VideoDecoder>,
    ready: Vec<Frame>,
    decoded_key_frame: bool,
    need_refresh: bool,
    done: Vec<Frame>,
}

impl Direct {
    fn new(decoder: Box<dyn VideoDecoder>) -> Self {
        Self {
            decoder,
            ready: Vec::new(),
            decoded_key_frame: false,
            need_refresh: false,
            done: Vec::new(),
        }
    }

    /// Submits the ready frames; runs without the receiver's lock, since the decoder may wait
    /// for an input buffer.
    fn submit(&mut self, shared: &VideoShared) {
        for frame in self.ready.drain(..) {
            shared.counters.submitted.fetch_add(1, Ordering::Relaxed);
            match self.decoder.submit(&frame) {
                DecodeStatus::Ok => self.decoded_key_frame |= frame.frame_type == FrameType::Idr,
                DecodeStatus::NeedIdr => {
                    shared.counters.need_idr.fetch_add(1, Ordering::Relaxed);
                    self.need_refresh = true;
                }
            }
            self.done.push(frame);
        }
    }

    /// Hands the submissions' feedback and frames back to the receiver.
    fn settle(
        &mut self,
        receiver: &mut VideoReceiver,
        shared: &VideoShared,
        signals: &mut Vec<VideoSignal>,
    ) {
        if self.done.is_empty() {
            return;
        }
        if std::mem::take(&mut self.decoded_key_frame) {
            receiver.frame_decoded(FrameType::Idr);
        }
        if !self.decoder.recovers_with_rfi() {
            receiver.recover_with_idr_only();
        }
        if std::mem::take(&mut self.need_refresh) {
            let mut sink = Sink {
                shared,
                direct: None,
                signals,
            };
            receiver.request_decoder_refresh(&mut sink);
        }
        for frame in self.done.drain(..) {
            receiver.recycle(frame);
        }
    }
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
        if let Some(direct) = self.direct.as_deref_mut() {
            direct.ready.push(frame);
            return Ok(());
        }
        let result = self.shared.queue.offer(frame);
        if result.is_err() {
            self.shared
                .counters
                .queue_overflows
                .fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    fn flush_frames(&mut self) -> usize {
        self.shared.queue.flush().len()
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
            (Some(Direct::new(decoder)), None)
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
        let direct_submit = start.direct_submit;
        if !session.decode_priority {
            session.log("Decode priority is off for this stream (setting or switch)");
        }
        stream.receive = Some(std::thread::Builder::new().name("VideoRecv".into()).spawn(
            move || {
                receive_session.prioritize_receive_thread(direct_submit);
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
    let mut signals = Vec::new();
    let mut waiting = Duration::ZERO;
    let mut first_data: Option<Instant> = None;
    let (encrypted, max_datagram) = {
        let receiver = lock(&shared.receiver);
        let encrypted = receiver.config().encryption_key.is_some();
        (encrypted, receiver.max_datagram_size())
    };
    let mut batch = net::BatchReceiver::new(&socket, max_datagram);
    match batch.take_fallback() {
        Some(e) => session.log(&format!("Video receive takes one datagram per call: {e}")),
        None if batch.batched() => session.log(&format!(
            "Video receive takes up to {} queued datagrams per call",
            net::RECEIVE_BATCH
        )),
        None => {}
    }
    let mut trace = PacketTrace::from_env(Stream::Video { encrypted });
    while !stop.load(Ordering::Acquire) {
        if let Some(trace) = trace.as_mut() {
            trace.waiting();
        }
        let received = batch.receive(&socket);
        // Whatever that receive returned: a refusal followed by a timeout or a failure.
        if let Some(e) = batch.take_fallback() {
            session.log(&format!("Video receive takes one datagram per call from now on: {e}"));
        }
        let count = match received {
            Ok(0) => {
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
            Ok(count) => count,
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
        session.received((0..count).map(|i| batch.length(i)).sum());
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
        receive_batch(
            &shared,
            direct.as_mut(),
            &mut signals,
            count,
            |i| batch.datagram(i),
            &mut trace,
            |signals| route(&control, signals),
        );
    }
    if let Some(trace) = trace {
        let session = session.clone();
        trace.finish(move |line| session.log(line));
    }
    direct.map(|d| d.decoder)
}

/// The batch under one lock of the receiver, each datagram at once. Work a datagram raised
/// that must not run under the lock ends the lock section and runs right after it, ahead of
/// the rest of the batch: a direct decoder's submissions (they may wait for the decoder),
/// then the signals (the control thread takes the receiver's lock too). The submissions'
/// feedback reaches the receiver at the start of the next lock section, before the next
/// datagram, and its signals are routed with the datagram's.
fn receive_batch<'b>(
    shared: &VideoShared,
    mut direct: Option<&mut Direct>,
    signals: &mut Vec<VideoSignal>,
    count: usize,
    datagram_at: impl Fn(usize) -> &'b [u8],
    trace: &mut Option<PacketTrace>,
    mut route_signals: impl FnMut(&mut Vec<VideoSignal>),
) {
    let mut next = 0;
    loop {
        {
            let mut receiver = lock(&shared.receiver);
            if let Some(d) = direct.as_deref_mut() {
                d.settle(&mut receiver, shared, signals);
            }
            while signals.is_empty() && next < count {
                let datagram = datagram_at(next);
                next += 1;
                let now = now_us();
                if let Some(trace) = trace.as_mut() {
                    trace.push(now, datagram);
                }
                let mut sink = Sink {
                    shared,
                    direct: direct.as_deref_mut(),
                    signals: &mut *signals,
                };
                receiver.receive(datagram, now, &mut sink);
                shared
                    .counters
                    .decoder_drops
                    .store(receiver.stats().decoder_drops, Ordering::Relaxed);
                if let Some(d) = direct.as_deref_mut() {
                    if !d.decoder.recovers_with_rfi() {
                        receiver.recover_with_idr_only();
                    }
                    if !d.ready.is_empty() {
                        break;
                    }
                }
            }
            *lock(&shared.stats) = receiver.stats();
        }
        // Outside the lock.
        if let Some(d) = direct.as_deref_mut().filter(|d| !d.ready.is_empty()) {
            d.submit(shared);
            continue;
        }
        route_signals(signals);
        if next >= count {
            break;
        }
    }
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
    let mut backlog = Backlog::default();
    // A key frame was skipped: the next decoded frame stands in for it.
    let mut skipped_idr = false;
    while let Some((item, behind)) = shared.queue.wait_behind(&mut returned) {
        let frame = match item {
            Item::Chunk(chunk) => {
                // Fewer than two frames behind a chunk never make its frame one to skip.
                if behind >= 2 && decoder.intra_only() && backlog.skips_chunk(behind, now_us()) {
                    shared.counters.chunks_skipped.fetch_add(1, Ordering::Relaxed);
                } else {
                    decoder.chunk(chunk.first, chunk.flags, &chunk.data);
                }
                returned.push(chunk.data);
                continue;
            }
            Item::Frame(frame) => frame,
        };
        // With nothing queued behind the frame this costs one comparison.
        if behind >= 1 && decoder.intra_only() && backlog.skips(behind, now_us()) {
            decoder.skip(&frame);
            shared.counters.frames_skipped.fetch_add(1, Ordering::Relaxed);
            skipped_idr |= frame.frame_type == FrameType::Idr;
            lock(&shared.receiver).recycle(frame);
            continue;
        }
        shared.counters.submitted.fetch_add(1, Ordering::Relaxed);
        let status = decoder.submit(&frame);
        {
            let mut receiver = lock(&shared.receiver);
            match status {
                DecodeStatus::NeedIdr => {
                    skipped_idr = false;
                    shared.counters.need_idr.fetch_add(1, Ordering::Relaxed);
                    let mut sink = Sink {
                        shared,
                        direct: None,
                        signals: &mut signals,
                    };
                    receiver.request_decoder_refresh(&mut sink);
                }
                DecodeStatus::Ok => {
                    if std::mem::take(&mut skipped_idr) || frame.frame_type == FrameType::Idr {
                        receiver.frame_decoded(FrameType::Idr);
                    }
                }
            }
            if !decoder.recovers_with_rfi() {
                receiver.recover_with_idr_only();
            }
            receiver.recycle(frame);
            *lock(&shared.stats) = receiver.stats();
        }
        route_signals(&mut signals);
    }
    decoder
}

#[cfg(test)]
mod alloc_tests;

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
    fn a_frame_takes_its_chunk_buffers_back_before_the_decoder_holds_it() {
        // The decoder may sit in submit for a while; the buffers of the chunks it already
        // decoded must be there for the next frame's chunks, or the receiver allocates new
        // ones whenever the decoder happens to hold more than before.
        let queue = FrameQueue::default();
        for i in 0..3 {
            assert!(queue.offer_chunk(i == 0, 0, &[i]));
        }
        queue.offer(Frame::default()).unwrap();
        let mut back = Vec::new();
        for _ in 0..3 {
            let Some(Item::Chunk(chunk)) = queue.wait(&mut back) else {
                panic!("a chunk")
            };
            back.push(chunk.data);
        }
        let spares = || lock(&queue.state).spare.len();
        assert_eq!((back.len(), spares()), (3, 0), "chunks alone wait for a batch");
        assert!(matches!(queue.wait(&mut back), Some(Item::Frame(_))));
        assert_eq!((back.len(), spares()), (0, 3), "the frame hands them back");
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
        /// A frame passed over for a newer one.
        Skipped { number: u32 },
    }

    /// Accumulates the chunks the way the desktop decoder checks them against the frame.
    struct Recorder {
        seen: mpsc::Sender<Seen>,
        envelope: Vec<u8>,
        status: DecodeStatus,
        rfi: bool,
        intra_only: bool,
    }

    impl Recorder {
        fn new(seen: mpsc::Sender<Seen>) -> Self {
            Self {
                seen,
                envelope: Vec::new(),
                status: DecodeStatus::Ok,
                rfi: true,
                intra_only: false,
            }
        }

        fn intra_only(mut self) -> Self {
            self.intra_only = true;
            self
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

        fn intra_only(&self) -> bool {
            self.intra_only
        }

        fn skip(&mut self, frame: &Frame) {
            self.envelope.clear();
            self.seen
                .send(Seen::Skipped {
                    number: frame.number,
                })
                .unwrap();
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

        fn recovers_with_rfi(&self) -> bool {
            self.rfi
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
                Seen::Chunk { .. } | Seen::Skipped { .. } => None,
            })
            .collect()
    }

    fn decoded_numbers(seen: &[Seen]) -> Vec<u32> {
        frames_seen(seen).iter().map(|frame| frame.0).collect()
    }

    fn skipped_numbers(seen: &[Seen]) -> Vec<u32> {
        seen.iter()
            .filter_map(|s| match s {
                Seen::Skipped { number } => Some(*number),
                _ => None,
            })
            .collect()
    }

    fn chunks_seen(seen: &[Seen]) -> usize {
        seen.iter()
            .filter(|s| matches!(s, Seen::Chunk { .. }))
            .count()
    }

    // The data and buffers are private to the video module, so no struct update.
    #[allow(clippy::field_reassign_with_default)]
    fn numbered(number: u32, frame_type: FrameType) -> Frame {
        let mut frame = Frame::default();
        frame.number = number;
        frame.frame_type = frame_type;
        frame
    }

    /// `frames` frames queued as the receiver would, each behind its three chunks, with
    /// nothing taking them yet: the decoder finds them as a backlog when it starts.
    fn backlog(shared: &VideoShared, frames: u32, first_type: FrameType) {
        for number in 1..=frames {
            for i in 0..3 {
                assert!(shared.queue.offer_chunk(i == 0, 0, &[number as u8, i]));
            }
            let frame_type = if number == 1 {
                first_type
            } else {
                FrameType::P
            };
            assert!(shared.queue.offer(numbered(number, frame_type)).is_ok());
        }
    }

    #[test]
    fn the_backlog_rule_needs_two_newer_frames_and_then_holds_at_one() {
        let mut rule = Backlog::default();
        // A decoder that keeps up: nothing behind, or the one frame of a pair that arrived
        // together, however long that goes on.
        for now in [0, 5_000, 2_000_000] {
            assert!(!rule.skips(0, now));
            assert!(!rule.skips(1, now));
            assert!(!rule.skips_chunk(2, now), "its frame is taken with one behind");
        }
        // Two newer frames: behind for certain, and so is a chunk whose frame has two behind.
        assert!(rule.skips_chunk(3, 3_000_000));
        assert!(rule.skips(2, 3_000_000));
        // From here one newer frame is enough, down to the newest, which is decoded.
        assert!(rule.skips_chunk(2, 3_000_100));
        assert!(rule.skips(1, 3_000_100));
        assert!(!rule.skips(0, 3_000_200));
        assert!(!rule.skips_chunk(1, 3_000_200), "the newest frame keeps its chunks");
        // Each skip holds the rule for another second: a decoder a little slower than the
        // stream drops single frames, each as it falls due.
        assert!(rule.skips(1, 3_000_100 + SKIP_HOLD_US - 1));
        assert!(rule.skips(1, 3_000_100 + 2 * SKIP_HOLD_US - 2));
        // A second without a skip, and it takes two newer frames again.
        let later = 3_000_100 + 3 * SKIP_HOLD_US;
        assert!(!rule.skips(1, later));
        assert!(!rule.skips_chunk(2, later));
        assert!(rule.skips(2, later));
    }

    #[test]
    fn a_backlog_decodes_only_the_newest_frame_of_an_intra_only_decoder() {
        let shared = Arc::new(VideoShared::new(records(true)));
        backlog(&shared, 5, FrameType::P);
        let (tx, rx) = mpsc::channel();
        let running = Running::start(&shared, Recorder::new(tx).intra_only(), rx);
        let seen = running.finish();
        assert_eq!(skipped_numbers(&seen), [1, 2, 3, 4], "{seen:?}");
        assert_eq!(decoded_numbers(&seen), [5], "{seen:?}");
        let counters = &shared.counters;
        assert_eq!(counters.frames_skipped.load(Ordering::Relaxed), 4);
        assert_eq!(
            counters.submitted.load(Ordering::Relaxed),
            1,
            "skipped frames are not submitted"
        );
        assert_eq!(counters.need_idr.load(Ordering::Relaxed), 0);
        assert_eq!(counters.queue_overflows.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn the_chunks_of_skipped_frames_are_dropped_and_not_counted_as_dropped() {
        let shared = Arc::new(VideoShared::new(records(true)));
        backlog(&shared, 4, FrameType::P);
        let (tx, rx) = mpsc::channel();
        let running = Running::start(&shared, Recorder::new(tx).intra_only(), rx);
        let seen = running.finish();
        // Frame 1's chunks have four frames behind them and frame 2's three, so they go
        // unseen. Frame 3's have two, which is one behind the frame itself: only the skips
        // before it say that it goes too. Frame 4 keeps its chunks, first one first.
        assert_eq!(skipped_numbers(&seen), [1, 2, 3], "{seen:?}");
        assert_eq!(chunks_seen(&seen), 3, "{seen:?}");
        assert!(
            matches!(seen[seen.len() - 4], Seen::Chunk { first: true }),
            "{seen:?}"
        );
        assert!(
            matches!(seen.last(), Some(Seen::Frame { number: 4, .. })),
            "{seen:?}"
        );
        let counters = &shared.counters;
        assert_eq!(counters.chunks_skipped.load(Ordering::Relaxed), 9);
        assert_eq!(counters.chunks_dropped.load(Ordering::Relaxed), 0);
        assert_eq!(
            lock(&shared.queue.state).chunks,
            0,
            "their buffers left the queue"
        );
    }

    #[test]
    fn a_pair_of_frames_is_no_backlog_and_both_are_decoded_with_their_chunks() {
        let shared = Arc::new(VideoShared::new(records(true)));
        backlog(&shared, 2, FrameType::P);
        let (tx, rx) = mpsc::channel();
        let running = Running::start(&shared, Recorder::new(tx).intra_only(), rx);
        let seen = running.finish();
        assert!(skipped_numbers(&seen).is_empty(), "{seen:?}");
        assert_eq!(decoded_numbers(&seen), [1, 2], "{seen:?}");
        assert_eq!(chunks_seen(&seen), 6, "{seen:?}");
        let counters = &shared.counters;
        assert_eq!(counters.submitted.load(Ordering::Relaxed), 2);
        assert_eq!(counters.frames_skipped.load(Ordering::Relaxed), 0);
        assert_eq!(counters.chunks_skipped.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_decoder_that_predicts_from_earlier_frames_sees_every_frame() {
        let shared = Arc::new(VideoShared::new(records(true)));
        backlog(&shared, 5, FrameType::P);
        let (tx, rx) = mpsc::channel();
        let running = Running::start(&shared, Recorder::new(tx), rx);
        let seen = running.finish();
        assert_eq!(decoded_numbers(&seen), [1, 2, 3, 4, 5], "{seen:?}");
        assert_eq!(chunks_seen(&seen), 15);
        let counters = &shared.counters;
        assert_eq!(counters.submitted.load(Ordering::Relaxed), 5);
        assert_eq!(counters.frames_skipped.load(Ordering::Relaxed), 0);
        assert_eq!(counters.chunks_skipped.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_skipped_key_frame_is_reported_decoded_with_the_next_frame_that_is() {
        // Frame 3 is decoded, but the decoder refuses it: no key frame has been decoded.
        let shared = Arc::new(VideoShared::new(records(true)));
        backlog(&shared, 3, FrameType::Idr);
        assert!(!lock(&shared.receiver).idr_decoded());
        let (tx, rx) = mpsc::channel();
        let mut recorder = Recorder::new(tx).intra_only();
        recorder.status = DecodeStatus::NeedIdr;
        let seen = Running::start(&shared, recorder, rx).finish();
        assert_eq!(skipped_numbers(&seen), [1, 2], "{seen:?}");
        assert!(!lock(&shared.receiver).idr_decoded());
        assert_eq!(shared.counters.need_idr.load(Ordering::Relaxed), 1);

        // Accepted, it stands for the key frame skipped before it.
        let shared = Arc::new(VideoShared::new(records(true)));
        backlog(&shared, 3, FrameType::Idr);
        let (tx, rx) = mpsc::channel();
        let seen = Running::start(&shared, Recorder::new(tx).intra_only(), rx).finish();
        assert_eq!(skipped_numbers(&seen), [1, 2], "{seen:?}");
        assert!(
            lock(&shared.receiver).idr_decoded(),
            "the accepted frame reports the skipped key frame"
        );

        // Without a skipped key frame a decoded inter frame reports none.
        let shared = Arc::new(VideoShared::new(records(true)));
        backlog(&shared, 3, FrameType::P);
        let (tx, rx) = mpsc::channel();
        Running::start(&shared, Recorder::new(tx).intra_only(), rx).finish();
        assert!(!lock(&shared.receiver).idr_decoded());

        // A key frame that is decoded itself is reported as before.
        let shared = Arc::new(VideoShared::new(records(true)));
        backlog(&shared, 1, FrameType::Idr);
        let (tx, rx) = mpsc::channel();
        Running::start(&shared, Recorder::new(tx).intra_only(), rx).finish();
        assert!(lock(&shared.receiver).idr_decoded());
    }

    #[test]
    fn a_decoder_slower_than_the_stream_never_fills_the_queue() {
        // Without the skip the frames pile up until one finds the queue full, which flushes
        // it and asks for a key frame.
        let (shared, full, _, seen) = slower_than_the_stream(false);
        assert!(full.is_some(), "the backlog never reached the bound");
        assert!(skipped_numbers(&seen).is_empty());
        assert_eq!(shared.counters.frames_skipped.load(Ordering::Relaxed), 0);

        let (shared, full, deepest, seen) = slower_than_the_stream(true);
        assert_eq!(full, None, "a frame found the queue full");
        assert!(deepest <= SKIP_BACKLOG + 1, "the queue held {deepest}");
        let (decoded, skipped) = (decoded_numbers(&seen), skipped_numbers(&seen));
        assert_eq!(decoded.len() + skipped.len(), SLOW_FRAMES as usize);
        assert!(decoded.windows(2).all(|pair| pair[0] < pair[1]), "{decoded:?}");
        assert_eq!(decoded.last(), Some(&SLOW_FRAMES), "the newest frame is decoded");
        // One frame in six goes, less the few that built the first backlog.
        assert!(skipped.len() >= 15 && skipped.len() <= 20, "{skipped:?}");
        let counters = &shared.counters;
        assert_eq!(counters.submitted.load(Ordering::Relaxed), decoded.len() as u64);
        assert_eq!(counters.frames_skipped.load(Ordering::Relaxed), skipped.len() as u64);
        assert_eq!(counters.need_idr.load(Ordering::Relaxed), 0);
    }

    /// The frames [`slower_than_the_stream`] sends: six for every five decodes.
    const SLOW_FRAMES: u32 = 121;

    /// Holds each decode until the test lets it go, so the test decides how many frames
    /// arrive during one: no sleeps, and nothing depends on how threads are scheduled.
    struct Gated {
        recorder: Recorder,
        started: mpsc::Sender<()>,
        go: mpsc::Receiver<()>,
    }

    impl VideoDecoder for Gated {
        fn setup(&mut self, setup: &crate::session::VideoSetup) -> Result<(), i32> {
            self.recorder.setup(setup)
        }

        fn intra_only(&self) -> bool {
            self.recorder.intra_only
        }

        fn skip(&mut self, frame: &Frame) {
            self.recorder.skip(frame);
        }

        fn submit(&mut self, frame: &Frame) -> DecodeStatus {
            self.started.send(()).unwrap();
            // A closed gate lets everything through: the test is over.
            let _ = self.go.recv();
            self.recorder.submit(frame)
        }
    }

    /// A stream a fifth faster than its decoder: one frame arrives during each decode and
    /// two during every fifth. Returns the first frame that found the queue full, if one
    /// did, the most frames the queue held, and what the decoder saw.
    fn slower_than_the_stream(
        intra_only: bool,
    ) -> (Arc<VideoShared>, Option<u32>, usize, Vec<Seen>) {
        let shared = Arc::new(VideoShared::new(records(true)));
        let (tx, rx) = mpsc::channel();
        let (started, in_decode) = mpsc::channel();
        let (go, gate) = mpsc::channel();
        let mut recorder = Recorder::new(tx);
        recorder.intra_only = intra_only;
        let decoder = Gated {
            recorder,
            started,
            go: gate,
        };
        let for_thread = shared.clone();
        let thread =
            std::thread::spawn(move || decode_loop(Box::new(decoder), &for_thread, |_| {}));
        let decoding = || {
            in_decode
                .recv_timeout(HANG_GUARD)
                .expect("the decoder never took the next frame")
        };
        assert!(shared.queue.offer(numbered(1, FrameType::P)).is_ok());
        decoding();
        let (mut number, mut full, mut deepest) = (1, None, 0);
        'stream: for decode in 1..=100 {
            let arriving = if decode % 5 == 0 { 2 } else { 1 };
            for _ in 0..arriving {
                number += 1;
                if shared.queue.offer(numbered(number, FrameType::P)).is_err() {
                    full = Some(number);
                    break 'stream;
                }
            }
            deepest = deepest.max(shared.queue.len());
            go.send(()).unwrap();
            // The newest frame is never skipped, so another decode always follows.
            decoding();
        }
        drop(go);
        wait_until("the decoder never took everything queued", || {
            lock(&shared.queue.state).items.is_empty()
        });
        shared.queue.shutdown();
        thread.join().unwrap();
        (shared, full, deepest, rx.try_iter().collect())
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

    #[derive(Debug, PartialEq)]
    enum Step {
        Datagram,
        /// A submission: whether the receiver was free, and the frame's data buffer.
        Submit { unlocked: bool, data: usize },
        RequestIdr,
    }

    /// A direct decoder that logs its submissions and fails the first.
    struct DirectFake {
        shared: Arc<VideoShared>,
        log: Arc<Mutex<Vec<Step>>>,
        status: DecodeStatus,
    }

    impl VideoDecoder for DirectFake {
        fn setup(&mut self, _: &crate::session::VideoSetup) -> Result<(), i32> {
            Ok(())
        }

        fn submit(&mut self, frame: &Frame) -> DecodeStatus {
            let unlocked = self.shared.receiver.try_lock().is_ok();
            lock(&self.log).push(Step::Submit {
                unlocked,
                data: frame.data().as_ptr() as usize,
            });
            std::mem::replace(&mut self.status, DecodeStatus::Ok)
        }
    }

    #[test]
    fn a_direct_decoder_submits_outside_the_receivers_lock_and_its_feedback_comes_first() {
        let shared = Arc::new(VideoShared::new(records(false)));
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut direct = Direct::new(Box::new(DirectFake {
            shared: shared.clone(),
            log: log.clone(),
            status: DecodeStatus::NeedIdr,
        }));
        let mut host = Blocks::new();
        let mut packets = flat(host.frame(&[8, 6], 1));
        packets.extend(flat(host.frame(&[8, 6], 2)));
        packets.extend(flat(host.frame(&[8, 6], 3)));
        let mut signals = Vec::new();
        receive_batch(
            &shared,
            Some(&mut direct),
            &mut signals,
            packets.len(),
            |i| {
                lock(&log).push(Step::Datagram);
                packets[i].as_slice()
            },
            &mut None,
            |signals| {
                for signal in signals.drain(..) {
                    if matches!(signal, VideoSignal::RequestIdr) {
                        lock(&log).push(Step::RequestIdr);
                    }
                }
            },
        );
        let log = std::mem::take(&mut *lock(&log));
        let first = log
            .iter()
            .position(|s| matches!(s, Step::Submit { .. }))
            .expect("a submission");
        let Step::Submit { unlocked, data } = log[first] else {
            unreachable!()
        };
        assert!(unlocked, "{log:?}");
        // The failed submission's key frame request comes before the next datagram.
        assert_eq!(log.get(first + 1), Some(&Step::RequestIdr), "{log:?}");
        assert!(direct.ready.is_empty() && direct.done.is_empty());
        assert_eq!(shared.counters.submitted.load(Ordering::Relaxed), 3, "{log:?}");
        assert_eq!(shared.counters.need_idr.load(Ordering::Relaxed), 1);
        // The first frame's recycled buffer becomes the depacketizer's at the second frame's
        // reassembly, so the third frame carries it.
        let later: Vec<(bool, usize)> = log[first + 1..]
            .iter()
            .filter_map(|s| match s {
                Step::Submit { unlocked, data } => Some((*unlocked, *data)),
                _ => None,
            })
            .collect();
        assert_eq!(later.len(), 2, "{log:?}");
        assert_eq!(later[1], (true, data), "{log:?}");
    }

    /// A decoder replaced mid-stream by one that never asked for reference frame invalidation
    /// has the rest of the stream recover with key frames; one that did keeps RFI.
    #[test]
    fn a_decoder_without_rfi_turns_the_receivers_rfi_recovery_off() {
        for rfi in [true, false] {
            let mut config = crate::video::tests::config(0x01_0000);
            (config.pyrowave_records, config.early_chunks, config.rfi) = (true, true, true);
            let shared = Arc::new(VideoShared::new(VideoReceiver::new(config)));
            assert!(lock(&shared.receiver).rfi_recovery());
            let (tx, rx) = mpsc::channel();
            let mut recorder = Recorder::new(tx);
            recorder.rfi = rfi;
            let running = Running::start(&shared, recorder, rx);
            feed(&shared, &flat(Blocks::new().frame(&[8, 6], 1)));
            let seen = running.finish();
            assert_eq!(frames_seen(&seen).len(), 1, "{seen:?}");
            assert_eq!(lock(&shared.receiver).rfi_recovery(), rfi, "decoder with rfi {rfi}");
        }
    }
}
