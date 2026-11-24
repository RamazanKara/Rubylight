//! The video stream's threads: UDP receive with pings and the startup timeouts, the bounded
//! decode-unit queue (15 frames unless the session sets fewer) and its decoder thread (or
//! direct submission on the receive thread), and the routing of [`VideoSignal`]s to the control stream.
//!
//! Ported from moonlight-common-c's `VideoStream.c` and the queue handling of
//! `VideoDepacketizer.c` (GPL-3.0, Cameron Gutman and contributors).

mod queue;
mod refresh;
mod sink;

use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(test)]
use std::sync::Condvar;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub use queue::{FrameQueue, Item, StagedChunks, Waited, DECODE_UNIT_QUEUE_BOUND};
#[cfg(test)]
pub use queue::{decode_queue_bound, Chunk};
use queue::{Backlog, FrameReturns, RETURN_BATCH};
use refresh::KeyFrameRequests;
use sink::{fec_status, Direct, Sink};

use super::net::{self, NextSocket, PingHandle, Pinger, REBIND_WAIT, UDP_RECV_POLL_TIMEOUT};
use super::trace::{PacketTrace, Stream};
use super::{error, now_us, DecodeStatus, Shared, VideoDecoder};
use crate::control::driver::ControlHandle;
#[cfg(test)]
use crate::video::Frame;
use crate::video::{FrameType, VideoReceiver, VideoSignal};

/// No video packet, or no complete key frame after the first packet, for this long ends the
/// stream (`FIRST_FRAME_TIMEOUT_SEC`).
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(10);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
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
    /// Frames the decoder thread is done with ([`FrameReturns`]).
    returns: FrameReturns,
}

impl VideoShared {
    #[cfg(test)]
    pub fn new(receiver: VideoReceiver) -> Self {
        Self::with_queue_bound(receiver, DECODE_UNIT_QUEUE_BOUND)
    }

    /// With a decode-unit queue of at most [`decode_queue_bound`]`(bound)` frames.
    pub fn with_queue_bound(receiver: VideoReceiver, bound: usize) -> Self {
        Self {
            stats: Mutex::new(receiver.stats()),
            receiver: Mutex::new(receiver),
            queue: FrameQueue::with_bound(bound),
            counters: VideoCounters::default(),
            key_frame: AtomicBool::new(false),
            returns: FrameReturns::new(),
        }
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
    /// The stream's frame rate, which paces a failing decoder's key frame requests.
    pub fps: i32,
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
                Some(Direct::new(decoder, KeyFrameRequests::new(start.fps))),
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
            let refresh = KeyFrameRequests::new(start.fps);
            let decode_session = session.clone();
            stream.decode = Some(std::thread::Builder::new().name("VideoDec".into()).spawn(
                move || {
                    decode_session.prioritize_decode_thread();
                    decoder_thread(decoder, shared, control, refresh)
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
                // A direct decoder's held key frame request, due with no frame to carry it.
                if let Some(direct) = direct.as_mut()
                    && direct.send_held(&shared, &mut signals)
                {
                    route(&control, &mut signals);
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
        receive_batch_stamped(
            &shared,
            direct.as_mut(),
            &mut signals,
            count,
            |i| batch.datagram(i),
            |i| batch.arrival_us(i),
            &mut trace,
            &mut staged,
            |signals| route(&control, signals),
        );
        // Datagrams that completed no frame (a held request needs a frame or this to go out).
        if let Some(direct) = direct.as_mut()
            && direct.send_held(&shared, &mut signals)
        {
            route(&control, &mut signals);
        }
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
/// the section) or a submission starts a new section with a new reading. A datagram the
/// receive stamped itself (`arrival_at`: the kernel's receive time) takes its own.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn receive_batch<'b>(
    shared: &VideoShared,
    direct: Option<&mut Direct>,
    signals: &mut Vec<VideoSignal>,
    count: usize,
    datagram_at: impl Fn(usize) -> &'b [u8],
    trace: &mut Option<PacketTrace>,
    staged: &mut StagedChunks,
    route_signals: impl FnMut(&mut Vec<VideoSignal>),
) {
    receive_batch_stamped(
        shared,
        direct,
        signals,
        count,
        datagram_at,
        |_| None,
        trace,
        staged,
        route_signals,
    );
}

/// Whether `signal` ends the current receiver lock section so it routes ahead of later
/// datagrams. Routine per-block and start signals (`FrameSeen`, `FecBlock`, `FecStatus`) stay
/// batched until a frame completes, an urgent recovery signal fires, or the call ends.
fn ends_section(signal: &VideoSignal) -> bool {
    !matches!(
        signal,
        VideoSignal::FrameSeen { .. } | VideoSignal::FecBlock(_) | VideoSignal::FecStatus(_)
    )
}

/// [`receive_batch`] with each datagram's own arrival where the receive has one.
#[allow(clippy::too_many_arguments)]
fn receive_batch_stamped<'b>(
    shared: &VideoShared,
    mut direct: Option<&mut Direct>,
    signals: &mut Vec<VideoSignal>,
    count: usize,
    datagram_at: impl Fn(usize) -> &'b [u8],
    arrival_at: impl Fn(usize) -> Option<u64>,
    trace: &mut Option<PacketTrace>,
    staged: &mut StagedChunks,
    mut route_signals: impl FnMut(&mut Vec<VideoSignal>),
) {
    let mut next = 0;
    loop {
        {
            let mut receiver = lock(&shared.receiver);
            shared.returns.take_into(&mut receiver);
            if let Some(d) = direct.as_deref_mut() {
                d.settle(&mut receiver, shared, signals);
            }
            let mut section_us = None;
            let mut section_ended = signals.iter().any(ends_section);
            while !section_ended && next < count {
                let datagram = datagram_at(next);
                let arrival = arrival_at(next);
                next += 1;
                let now = arrival.unwrap_or_else(|| *section_us.get_or_insert_with(now_us));
                if let Some(trace) = trace.as_mut() {
                    trace.push(now, datagram);
                }
                let before = signals.len();
                let mut sink = Sink {
                    shared,
                    direct: direct.as_deref_mut(),
                    staged: Some(&mut *staged),
                    signals: &mut *signals,
                };
                receiver.receive(datagram, now, &mut sink);
                section_ended = signals[before..].iter().any(ends_section);
                if let Some(d) = direct.as_deref_mut() {
                    if !d.decoder.recovers_with_rfi() {
                        receiver.recover_with_idr_only();
                    }
                    if !d.ready.is_empty() {
                        break;
                    }
                }
            }
            let stats = receiver.stats();
            shared
                .counters
                .decoder_drops
                .store(stats.decoder_drops, Ordering::Relaxed);
            *lock(&shared.stats) = stats;
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
    refresh: KeyFrameRequests,
) -> Box<dyn VideoDecoder> {
    decode_loop(decoder, &shared, refresh, |signals| {
        route(&control, signals)
    })
}

/// The decoder thread's loop, with the routing of the receiver's signals left to the caller.
fn decode_loop(
    mut decoder: Box<dyn VideoDecoder>,
    shared: &VideoShared,
    mut refresh: KeyFrameRequests,
    mut route_signals: impl FnMut(&mut Vec<VideoSignal>),
) -> Box<dyn VideoDecoder> {
    let mut signals = Vec::new();
    // Chunk buffers go back to the queue with the next wait, in one lock.
    let mut returned: Vec<Vec<u8>> = Vec::with_capacity(RETURN_BATCH);
    let mut backlog = Backlog::default();
    // A key frame was skipped: the next decoded frame stands in for it.
    let mut skipped_idr = false;
    // The receiver recovers with key frames only (`recover_with_idr_only`, which stays so).
    let mut idr_only = false;
    loop {
        // A held key frame request wakes the thread when it falls due, frames or not.
        let deadline = refresh.held_until_us().map(|due| {
            Instant::now() + Duration::from_micros(due.saturating_sub(now_us()))
        });
        let (item, behind) = match shared.queue.wait_behind(&mut returned, deadline) {
            Waited::Item(item, behind) => (item, behind),
            Waited::Shutdown => break,
            Waited::TimedOut => {
                if refresh.request(false, now_us) {
                    let mut sink = Sink {
                        shared,
                        direct: None,
                        staged: None,
                        signals: &mut signals,
                    };
                    lock(&shared.receiver).request_decoder_refresh(&mut sink);
                    route_signals(&mut signals);
                }
                continue;
            }
        };
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
            shared.returns.give(frame);
            continue;
        }
        shared.counters.submitted.fetch_add(1, Ordering::Relaxed);
        let status = decoder.submit(&frame);
        let need_idr = status == DecodeStatus::NeedIdr;
        let decoded_idr = status == DecodeStatus::Ok
            && (std::mem::take(&mut skipped_idr) || frame.frame_type == FrameType::Idr);
        if need_idr {
            skipped_idr = false;
            shared.counters.need_idr.fetch_add(1, Ordering::Relaxed);
        }
        if decoded_idr {
            refresh.key_frame_decoded();
        }
        let request_idr = refresh.request(need_idr, now_us);
        // Asked after each frame: a decoder replaced mid-stream may not recover with RFI.
        let lost_rfi = !idr_only && !decoder.recovers_with_rfi();
        // Most frames tell the receiver nothing and leave its lock to the receive thread.
        if request_idr || decoded_idr || lost_rfi {
            let mut receiver = lock(&shared.receiver);
            if request_idr {
                let mut sink = Sink {
                    shared,
                    direct: None,
                    staged: None,
                    signals: &mut signals,
                };
                receiver.request_decoder_refresh(&mut sink);
            }
            if decoded_idr {
                receiver.frame_decoded(FrameType::Idr);
            }
            if lost_rfi {
                receiver.recover_with_idr_only();
                idr_only = true;
            }
        }
        // The receiver's statistics change only as datagrams arrive, which publishes them
        // itself, and the frame goes back without the receiver's lock.
        shared.returns.give(frame);
        route_signals(&mut signals);
    }
    decoder
}

#[cfg(test)]
mod alloc_tests;

#[cfg(test)]
mod tests;
