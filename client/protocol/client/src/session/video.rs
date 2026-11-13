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

/// Spare chunk buffers a [`StagedChunks`] keeps from the queue's for the next receive call: a
/// call's worth of datagrams twice over, so a call never waits on the queue for a buffer.
const STAGED_SPARES: usize = 2 * net::RECEIVE_BATCH;

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

/// Keeps `data` for a later chunk while `spare` holds fewer than [`STAGED_SPARES`].
fn keep_spare(spare: &mut Vec<Vec<u8>>, data: Vec<u8>) {
    if spare.len() < STAGED_SPARES {
        spare.push(data);
    }
}

/// The early chunks of one receive call on their way to the [`FrameQueue`]. They reach it
/// together, under one lock and with at most one wakeup of the decoder thread: with the frame
/// they end ([`FrameQueue::offer_behind`]), or at the end of the call
/// ([`FrameQueue::offer_staged`]). Queued one at a time, nearly every chunk of a decoder that
/// keeps up found it asleep on an empty queue and woke it, about 125,000 times a second at 4K
/// PyroWave. Only the receive thread stages chunks; the queue applies each chunk's bound when
/// the chunks arrive there, in their order, as [`FrameQueue::offer_chunk`] does.
#[derive(Default)]
pub struct StagedChunks {
    chunks: Vec<Chunk>,
    /// Buffers taken from the queue's spares for the next chunks.
    spare: Vec<Vec<u8>>,
    /// The queue's flush count when the chunks were last checked against it: a flush drops the
    /// chunks staged before it, as it drops the queued ones.
    flushes: u64,
}

impl StagedChunks {
    /// Stages a chunk behind the ones staged so far, after dropping those a flush has dropped
    /// since. The flush count is read without the queue's lock: the receive thread stages under
    /// the receiver's lock, which every flush also runs under, so it has seen every flush before
    /// this one.
    fn stage(&mut self, queue: &FrameQueue, first: bool, flags: u32, payload: &[u8]) {
        let flushes = queue.flushes.load(Ordering::Relaxed);
        if flushes != self.flushes {
            self.discard();
            self.flushes = flushes;
        }
        let mut data = self.spare.pop().unwrap_or_default();
        data.clear();
        data.extend_from_slice(payload);
        self.chunks.push(Chunk { first, flags, data });
    }

    /// Drops the staged chunks, keeping their buffers.
    fn discard(&mut self) {
        for chunk in self.chunks.drain(..) {
            keep_spare(&mut self.spare, chunk.data);
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.chunks.len()
    }
}

/// The bounded decode-unit queue between the receive and decoder threads.
#[derive(Default)]
pub struct FrameQueue {
    state: Mutex<QueueState>,
    ready: Condvar,
    /// Flushes so far, which [`StagedChunks`] reads without the lock. Changed under the lock.
    flushes: AtomicU64,
    /// Wakeups of the decoder thread.
    #[cfg(test)]
    wakes: AtomicU64,
}

impl FrameQueue {
    fn wake(&self) {
        #[cfg(test)]
        self.wakes.fetch_add(1, Ordering::Relaxed);
        self.ready.notify_one();
    }

    /// Queues a frame, or hands it back when the queue holds [`DECODE_UNIT_QUEUE_BOUND`].
    pub fn offer(&self, frame: Frame) -> Result<(), Frame> {
        let mut state = lock(&self.state);
        if state.shutdown || state.frames >= DECODE_UNIT_QUEUE_BOUND {
            return Err(frame);
        }
        state.items.push_back(Item::Frame(frame));
        state.frames += 1;
        self.wake();
        Ok(())
    }

    /// [`offer`](Self::offer) behind the staged chunks, which are queued first as
    /// [`offer_staged`](Self::offer_staged) queues them, under the same lock and with one
    /// wakeup: the chunks dropped over [`CHUNK_QUEUE_BOUND`], and the frame's outcome.
    pub fn offer_behind(
        &self,
        staged: &mut StagedChunks,
        frame: Frame,
    ) -> (usize, Result<(), Frame>) {
        let mut state = lock(&self.state);
        let empty = state.items.is_empty();
        let dropped = self.take_staged(&mut state, staged);
        let result = if state.shutdown || state.frames >= DECODE_UNIT_QUEUE_BOUND {
            Err(frame)
        } else {
            state.items.push_back(Item::Frame(frame));
            state.frames += 1;
            Ok(())
        };
        // A refused frame still leaves the chunks before it queued.
        let wake = result.is_ok() || (empty && !state.items.is_empty());
        drop(state);
        if wake {
            self.wake();
        }
        (dropped, result)
    }

    /// Queues the staged chunks behind everything queued so far, in their order, under one
    /// lock, waking the decoder thread at most once. Each is queued or dropped as
    /// [`offer_chunk`](Self::offer_chunk) would have: returns how many went over
    /// [`CHUNK_QUEUE_BOUND`], which the caller counts.
    pub fn offer_staged(&self, staged: &mut StagedChunks) -> usize {
        if staged.chunks.is_empty() {
            return 0;
        }
        let mut state = lock(&self.state);
        // The decoder thread only sleeps on an empty queue.
        let empty = state.items.is_empty();
        let dropped = self.take_staged(&mut state, staged);
        let wake = empty && !state.items.is_empty();
        drop(state);
        if wake {
            self.wake();
        }
        dropped
    }

    /// Moves the staged chunks into the queue under its lock (see
    /// [`offer_staged`](Self::offer_staged)), and refills the stage's spare buffers from the
    /// queue's. The chunks staged before a flush that has run since are dropped with it,
    /// uncounted as a flush's are.
    fn take_staged(&self, state: &mut QueueState, staged: &mut StagedChunks) -> usize {
        let flushes = self.flushes.load(Ordering::Relaxed);
        if flushes != staged.flushes {
            staged.discard();
            staged.flushes = flushes;
        }
        let mut dropped = 0;
        let StagedChunks { chunks, spare, .. } = staged;
        for chunk in chunks.drain(..) {
            if state.shutdown {
                keep_spare(spare, chunk.data);
            } else if state.chunks >= CHUNK_QUEUE_BOUND {
                dropped += 1;
                keep_spare(spare, chunk.data);
            } else {
                state.items.push_back(Item::Chunk(chunk));
                state.chunks += 1;
            }
        }
        let refill = STAGED_SPARES
            .saturating_sub(spare.len())
            .min(state.spare.len());
        let from = state.spare.len() - refill;
        spare.extend(state.spare.drain(from..));
        dropped
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
            self.wake();
        }
        true
    }

    /// Drops every queued frame and chunk, returning the frames. The queue keeps its
    /// capacity, and the chunk buffers go back to the spares.
    #[cfg(test)]
    pub fn flush(&self) -> Vec<Frame> {
        self.flush_staged(None)
    }

    /// [`flush`](Self::flush) that drops the chunks `staged` holds too: the flush comes after
    /// them in the receiver's order.
    pub fn flush_staged(&self, staged: Option<&mut StagedChunks>) -> Vec<Frame> {
        let mut state = lock(&self.state);
        let flushes = self.flushes.fetch_add(1, Ordering::Relaxed) + 1;
        if let Some(staged) = staged {
            staged.discard();
            staged.flushes = flushes;
        }
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

impl VideoCounters {
    fn count_chunks_dropped(&self, dropped: usize) {
        if dropped > 0 {
            self.chunks_dropped
                .fetch_add(dropped as u64, Ordering::Relaxed);
        }
    }
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
                staged: None,
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
    /// Where the receive thread's early chunks wait for their frame or the end of the receive
    /// call ([`receive_batch`]). Without, each chunk is queued at once.
    staged: Option<&'a mut StagedChunks>,
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
        let result = match self.staged.as_deref_mut() {
            Some(staged) => {
                let (dropped, result) = self.shared.queue.offer_behind(staged, frame);
                self.shared.counters.count_chunks_dropped(dropped);
                result
            }
            None => self.shared.queue.offer(frame),
        };
        if result.is_err() {
            self.shared
                .counters
                .queue_overflows
                .fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    fn flush_frames(&mut self) -> usize {
        self.shared
            .queue
            .flush_staged(self.staged.as_deref_mut())
            .len()
    }

    fn signal(&mut self, signal: VideoSignal) {
        self.signals.push(signal);
    }

    fn chunk(&mut self, first: bool, flags: u32, payload: &[u8]) {
        // PyroWave never submits directly, so a direct decoder has asked for no chunks.
        if self.direct.is_some() {
            return;
        }
        match self.staged.as_deref_mut() {
            Some(staged) => staged.stage(&self.shared.queue, first, flags, payload),
            None => {
                if !self.shared.queue.offer_chunk(first, flags, payload) {
                    self.shared.counters.count_chunks_dropped(1);
                }
            }
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
                | VideoSignal::TransportFeedback(_)
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
                VideoSignal::TransportFeedback(report) => session.transport_feedback(report),
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
    /// The stack may coalesce received video datagrams ([`SessionConfig::receive_coalescing`]).
    ///
    /// [`SessionConfig::receive_coalescing`]: super::SessionConfig::receive_coalescing
    pub receive_coalescing: bool,
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
        let coalescing = start.receive_coalescing;
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
                    coalescing,
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

#[allow(clippy::too_many_arguments)]
fn receive_thread(
    mut socket: UdpSocket,
    next_socket: Arc<NextSocket>,
    shared: Arc<VideoShared>,
    session: Arc<Shared>,
    control: ControlHandle,
    stop: Arc<AtomicBool>,
    mut direct: Option<Direct>,
    coalescing: bool,
) -> Option<Box<dyn VideoDecoder>> {
    let mut signals = Vec::new();
    let mut waiting = Duration::ZERO;
    let mut first_data: Option<Instant> = None;
    let (encrypted, max_datagram) = {
        let receiver = lock(&shared.receiver);
        let encrypted = receiver.config().encryption_key.is_some();
        (encrypted, receiver.max_datagram_size())
    };
    let mut batch = net::BatchReceiver::new(&socket, max_datagram, coalescing);
    if !coalescing {
        session.log("Video receive coalescing is off (setting)");
    }
    match batch.take_fallback() {
        Some(e) => session.log(&format!("Video receive takes one datagram per call: {e}")),
        None if batch.batched() && cfg!(windows) => session.log(&format!(
            "Video receive coalescing is on: up to {} datagrams per call",
            net::RECEIVE_BATCH
        )),
        None if batch.batched() => session.log(&format!(
            "Video receive takes up to {} queued datagrams per call",
            net::RECEIVE_BATCH
        )),
        None => {}
    }
    if let Some(e) = batch.take_gro_refusal() {
        session.log(&format!("Video receive UDP_GRO is off: {e}"));
    } else if batch.gro() {
        session.log("Video receive UDP_GRO is on: coalesced reads are split per datagram");
    }
    let mut staged = StagedChunks::default();
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
            &mut staged,
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
///
/// The batch's early chunks wait in `staged`: each frame takes the ones before it into the
/// queue when it completes, and the rest go at the end of the batch, so the decoder thread
/// wakes once for a batch of chunks rather than for each, and never later than the end of the
/// batch. The datagrams of a lock section share one receive time, read at its first: all of
/// them were queued when the batch was received, and a completed frame (whose signal ends
/// the section) or a submission starts a new section with a new reading.
#[allow(clippy::too_many_arguments)]
fn receive_batch<'b>(
    shared: &VideoShared,
    mut direct: Option<&mut Direct>,
    signals: &mut Vec<VideoSignal>,
    count: usize,
    datagram_at: impl Fn(usize) -> &'b [u8],
    trace: &mut Option<PacketTrace>,
    staged: &mut StagedChunks,
    mut route_signals: impl FnMut(&mut Vec<VideoSignal>),
) {
    let mut next = 0;
    loop {
        {
            let mut receiver = lock(&shared.receiver);
            if let Some(d) = direct.as_deref_mut() {
                d.settle(&mut receiver, shared, signals);
            }
            let mut section_us = None;
            while signals.is_empty() && next < count {
                let datagram = datagram_at(next);
                next += 1;
                let now = *section_us.get_or_insert_with(now_us);
                if let Some(trace) = trace.as_mut() {
                    trace.push(now, datagram);
                }
                let mut sink = Sink {
                    shared,
                    direct: direct.as_deref_mut(),
                    staged: Some(&mut *staged),
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
        if next >= count {
            // The chunks behind the batch's last frame, ahead of the signals so the decoder
            // starts on them first.
            let dropped = shared.queue.offer_staged(staged);
            shared.counters.count_chunks_dropped(dropped);
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
                        staged: None,
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
            // The receiver's statistics change only as datagrams arrive, which publishes them
            // itself, so a decoded frame takes the receiver's lock alone.
            receiver.recycle(frame);
        }
        route_signals(&mut signals);
    }
    decoder
}

#[cfg(test)]
mod alloc_tests;

#[cfg(test)]
mod tests;
