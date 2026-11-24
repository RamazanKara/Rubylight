//! Bounded decode-unit queue, early-chunk staging, backlog skipping rule and lock-free frame
//! return queue for the video pipeline.
//!
//! Ported from moonlight-common-c's `VideoDepacketizer.c` queue handling (GPL-3.0, Cameron
//! Gutman and contributors).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Instant;

use super::lock;
use super::super::net;
use crate::video::{Frame, VideoReceiver};

/// `decodeUnitQueue`'s bound, the most frames [`FrameQueue`] holds and its default.
pub const DECODE_UNIT_QUEUE_BOUND: usize = 15;

/// The decode-unit queue's bound for [`SessionConfig::decode_queue_bound`]: 0 (unset) and
/// anything above [`DECODE_UNIT_QUEUE_BOUND`] keep that bound, which the frame pool is sized for.
///
/// [`SessionConfig::decode_queue_bound`]: super::super::SessionConfig::decode_queue_bound
pub fn decode_queue_bound(setting: usize) -> usize {
    if setting == 0 {
        DECODE_UNIT_QUEUE_BOUND
    } else {
        setting.min(DECODE_UNIT_QUEUE_BOUND)
    }
}

/// Early chunks queued at most: a frame is at most 4092 packets, so this holds a few frames
/// when the decoder falls behind. A chunk over the bound is dropped, which the decoder finds
/// from the frame (see [`super::super::VideoDecoder::chunk`]).
pub(super) const CHUNK_QUEUE_BOUND: usize = 16384;

/// Chunk buffers the decoder thread collects before it hands them back to the queue.
pub(super) const RETURN_BATCH: usize = 32;

/// Spare chunk buffers a [`StagedChunks`] keeps from the queue's for the next receive call: a
/// call's worth of datagrams twice over, so a call never waits on the queue for a buffer.
const STAGED_SPARES: usize = 2 * net::RECEIVE_BATCH;

/// One packet of a records frame in flight, see [`crate::video::VideoSink::chunk`].
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

/// What [`FrameQueue::wait_behind`] came back with.
pub enum Waited {
    /// The next item and the number of frames queued behind it.
    Item(Item, usize),
    /// Nothing came by the deadline.
    TimedOut,
    Shutdown,
}

#[derive(Default)]
pub(super) struct QueueState {
    pub(super) items: VecDeque<Item>,
    /// Frames among `items`, which the bound counts.
    pub(super) frames: usize,
    pub(super) chunks: usize,
    /// Chunk payload buffers handed back by the decoder thread.
    pub(super) spare: Vec<Vec<u8>>,
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
    pub(super) chunks: Vec<Chunk>,
    /// Buffers taken from the queue's spares for the next chunks.
    pub(super) spare: Vec<Vec<u8>>,
    /// The queue's flush count when the chunks were last checked against it: a flush drops the
    /// chunks staged before it, as it drops the queued ones.
    flushes: u64,
}

impl StagedChunks {
    /// Stages a chunk behind the ones staged so far, after dropping those a flush has dropped
    /// since. The flush count is read without the queue's lock: the receive thread stages under
    /// the receiver's lock, which every flush also runs under, so it has seen every flush before
    /// this one.
    pub(super) fn stage(&mut self, queue: &FrameQueue, first: bool, flags: u32, payload: &[u8]) {
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
    pub(super) fn len(&self) -> usize {
        self.chunks.len()
    }
}

/// The bounded decode-unit queue between the receive and decoder threads.
pub struct FrameQueue {
    pub(super) state: Mutex<QueueState>,
    ready: Condvar,
    /// Mirror of `state.frames`, updated under `state` so [`Self::len`] is lock-free.
    len: AtomicUsize,
    /// Flushes so far, which [`StagedChunks`] reads without the lock. Changed under the lock.
    flushes: AtomicU64,
    /// Frames queued at most: a frame over it is refused, and the receiver flushes the queue
    /// and asks for a key frame.
    bound: usize,
    /// Wakeups of the decoder thread.
    #[cfg(test)]
    pub(super) wakes: AtomicU64,
}

impl Default for FrameQueue {
    fn default() -> Self {
        Self::with_bound(DECODE_UNIT_QUEUE_BOUND)
    }
}

impl FrameQueue {
    /// A queue of at most [`decode_queue_bound`]`(bound)` frames.
    pub fn with_bound(bound: usize) -> Self {
        Self {
            state: Mutex::default(),
            ready: Condvar::new(),
            len: AtomicUsize::new(0),
            flushes: AtomicU64::new(0),
            bound: decode_queue_bound(bound),
            #[cfg(test)]
            wakes: AtomicU64::new(0),
        }
    }

    /// The most frames the queue holds.
    pub fn bound(&self) -> usize {
        self.bound
    }

    fn wake(&self) {
        #[cfg(test)]
        self.wakes.fetch_add(1, Ordering::Relaxed);
        self.ready.notify_one();
    }

    /// Queues a frame, or hands it back when the queue holds its [`bound`](Self::bound).
    pub fn offer(&self, frame: Frame) -> Result<(), Frame> {
        let mut state = lock(&self.state);
        if state.shutdown || state.frames >= self.bound {
            return Err(frame);
        }
        // The decoder thread only sleeps on an empty queue.
        let empty = state.items.is_empty();
        state.items.push_back(Item::Frame(frame));
        state.frames += 1;
        self.len.store(state.frames, Ordering::Relaxed);
        let wake = empty && !state.items.is_empty();
        drop(state);
        if wake {
            self.wake();
        }
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
        let result = if state.shutdown || state.frames >= self.bound {
            Err(frame)
        } else {
            state.items.push_back(Item::Frame(frame));
            state.frames += 1;
            self.len.store(state.frames, Ordering::Relaxed);
            Ok(())
        };
        // The decoder thread only sleeps on an empty queue.
        let wake = empty && !state.items.is_empty();
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
    #[cfg(test)]
    pub fn flush_staged(&self, staged: Option<&mut StagedChunks>) -> Vec<Frame> {
        let mut frames = Vec::new();
        self.flush_staged_into(staged, |frame| frames.push(frame));
        frames
    }

    /// Drops every queued frame and chunk (plus any chunks `staged` holds), handing each flushed
    /// frame to `on_frame` and returning how many frames were flushed, without allocating a `Vec`.
    pub fn flush_staged_into(
        &self,
        staged: Option<&mut StagedChunks>,
        mut on_frame: impl FnMut(Frame),
    ) -> usize {
        let mut state = lock(&self.state);
        let flushes = self.flushes.fetch_add(1, Ordering::Relaxed) + 1;
        if let Some(staged) = staged {
            staged.discard();
            staged.flushes = flushes;
        }
        state.frames = 0;
        self.len.store(0, Ordering::Relaxed);
        state.chunks = 0;
        let QueueState { items, spare, .. } = &mut *state;
        let mut count = 0;
        for item in items.drain(..) {
            match item {
                Item::Frame(frame) => {
                    count += 1;
                    on_frame(frame);
                }
                Item::Chunk(chunk) => {
                    if spare.len() < CHUNK_QUEUE_BOUND {
                        spare.push(chunk.data);
                    }
                }
            }
        }
        count
    }

    /// [`wait_behind`](Self::wait_behind) without the frame count.
    #[cfg(test)]
    pub fn wait(&self, returned: &mut Vec<Vec<u8>>) -> Option<Item> {
        match self.wait_behind(returned, None) {
            Waited::Item(item, _) => Some(item),
            _ => None,
        }
    }

    /// The next frame or chunk with the number of frames still queued behind it, or
    /// [`Waited::Shutdown`] once shut down, or [`Waited::TimedOut`] when nothing came by the
    /// `deadline`. The chunk buffers in `returned` go back to the spares under the lock this
    /// takes anyway: once [`RETURN_BATCH`] have gathered, before sleeping on an empty queue, and
    /// with every frame, which the decoder may take a while over while the receiver needs
    /// buffers for the next frame's chunks.
    pub fn wait_behind(&self, returned: &mut Vec<Vec<u8>>, deadline: Option<Instant>) -> Waited {
        let mut state = lock(&self.state);
        loop {
            if state.shutdown {
                return Waited::Shutdown;
            }
            if returned.len() >= RETURN_BATCH {
                state.take_spares(returned);
            }
            if let Some(item) = state.items.pop_front() {
                match item {
                    Item::Frame(_) => {
                        state.frames -= 1;
                        self.len.store(state.frames, Ordering::Relaxed);
                        state.take_spares(returned);
                    }
                    Item::Chunk(_) => state.chunks -= 1,
                }
                return Waited::Item(item, state.frames);
            }
            state.take_spares(returned);
            state = match deadline {
                None => self.ready.wait(state).unwrap_or_else(|p| p.into_inner()),
                Some(deadline) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Waited::TimedOut;
                    }
                    self.ready
                        .wait_timeout(state, left)
                        .unwrap_or_else(|p| p.into_inner())
                        .0
                }
            };
        }
    }

    pub fn len(&self) -> usize {
        self.len.load(Ordering::Relaxed)
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
pub(super) const SKIP_BACKLOG: usize = 2;
/// For this long after a skip a single newer frame is enough for the next one. A decoder a
/// little slower than the stream then drops one frame at a time as each falls due, instead of
/// two in a row each time the backlog reaches [`SKIP_BACKLOG`] again.
pub(super) const SKIP_HOLD_US: u64 = 1_000_000;

/// Which frames of a decoder whose frames decode on their own ([`super::super::VideoDecoder::intra_only`])
/// give way to a newer one already queued, so that the decoder falling behind costs frames and
/// not latency. Without it every frame waits behind the ones before it, up to
/// [`DECODE_UNIT_QUEUE_BOUND`] decodes, until the queue overflows and is flushed. A decoder that
/// keeps up never has two newer frames queued, and nothing changes for it.
#[derive(Default)]
pub(super) struct Backlog {
    /// When the last frame was skipped (session clock, us).
    skipped_us: Option<u64>,
}

impl Backlog {
    fn held(&self, now_us: u64) -> bool {
        self.skipped_us
            .is_some_and(|at| now_us.saturating_sub(at) < SKIP_HOLD_US)
    }

    /// Whether the frame just taken, with `behind` newer frames queued, is skipped.
    pub(super) fn skips(&mut self, behind: usize, now_us: u64) -> bool {
        let skip = behind >= SKIP_BACKLOG || (behind >= 1 && self.held(now_us));
        if skip {
            self.skipped_us = Some(now_us);
        }
        skip
    }

    /// Whether a chunk taken with `behind` frames queued after it is of no use: the first of
    /// those frames is its own (or a later one, when its own was lost), and that one will be
    /// skipped when it is taken with the rest still behind it.
    pub(super) fn skips_chunk(&self, behind: usize, now_us: u64) -> bool {
        behind > SKIP_BACKLOG || (behind >= 2 && self.held(now_us))
    }
}

/// Frames the decoder thread returns at most before the receive thread takes them back: every
/// frame the decode queue holds and the one being decoded, as the receiver's frame pool keeps.
const RETURNED_FRAMES: usize = DECODE_UNIT_QUEUE_BOUND + 1;

/// The frames the decoder thread is done with, on their way back to the receiver's pool. The
/// decoder hands each over under this list's own lock, so it never waits for the receiver's
/// lock (FEC recovery, frame assembly); the receive thread recycles them at the start of its
/// next lock section, before any frame of that section needs storage.
pub(super) struct FrameReturns {
    /// Allocated once, at [`RETURNED_FRAMES`]; a frame past it is freed instead.
    pub(super) frames: Mutex<Vec<Frame>>,
    /// Set after a frame is handed over, so a lock section with nothing to take skips the lock.
    pub(super) pending: AtomicBool,
}

impl FrameReturns {
    pub(super) fn new() -> Self {
        Self {
            frames: Mutex::new(Vec::with_capacity(RETURNED_FRAMES)),
            pending: AtomicBool::new(false),
        }
    }

    /// Hands a frame back from the decoder thread.
    pub(super) fn give(&self, frame: Frame) {
        let mut frames = lock(&self.frames);
        // Only frames in flight come back, so the list never reaches its bound in practice; a
        // frame past it would be freed by the full pool too.
        let spare = if frames.len() < RETURNED_FRAMES {
            frames.push(frame);
            None
        } else {
            Some(frame)
        };
        drop(frames);
        self.pending.store(true, Ordering::Release);
        drop(spare);
    }

    /// Recycles the returned frames into the receiver, under its lock.
    pub(super) fn take_into(&self, receiver: &mut VideoReceiver) {
        if !self.pending.load(Ordering::Acquire) || !self.pending.swap(false, Ordering::Acquire) {
            return;
        }
        for frame in lock(&self.frames).drain(..) {
            receiver.recycle(frame);
        }
    }
}
