//! Ordered conventional output with separate packetization and paced sending.
mod early;
pub(crate) mod link_watch;
pub(crate) mod recovery;
mod streamed;
mod transport;
use crate::state::{Session, Shared};
use anyhow::{Result, bail};
use butterpollo_core::{
    config::Config,
    packet::{PacketBlock, VideoPacketizer},
    performance::{Claim, Claims},
};
use butterpollo_windows::{
    encoder::{Encoded, Unit},
    net::ReplySocket,
};
use std::{
    net::SocketAddr,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

fn micros(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

struct Frame {
    encoded: Encoded,
    peer: SocketAddr,
    claimed: Instant,
    polled: Instant,
    latency: Duration,
    /// Capture/encode-loop tick that produced this output (Punktfunk's `cap_at`).
    processing_started: Instant,
    /// Frames skipped just before this one; their wire numbers stay used.
    skipped: u32,
    /// A new picture's claim, for the stage timings.
    claim: Option<Claim>,
    /// The encoder's slice delivery when the frame was submitted.
    slice_blocks: usize,
}
/// Fully prepared packets. Their Moonlight timing is immutable before the send queue.
struct Prepared {
    blocks: Vec<PacketBlock>,
    counts: Vec<usize>,
    layout: Vec<(usize, usize)>,
    packetizer: VideoPacketizer,
    wire_frame: u32,
    idr: bool,
    after_invalidation: bool,
    frame_cap_bytes: Option<u64>,
    /// The cap the host asked the encoder for (AMF only).
    cap_target_bytes: Option<u64>,
    /// AMF encoded this keyframe again in place of an oversize scene cut.
    reencoded: bool,
    /// Too large for full FEC within Moonlight's four blocks.
    fec_limited: bool,
    payload_bytes: usize,
    peer: SocketAddr,
    claimed: Instant,
    /// The encoder's first unit of the frame was in hand. A whole frame has one.
    polled: Instant,
    /// The frame's last unit was in hand: the same as `polled` for a whole frame, and for
    /// a streamed one the part that completes it.
    last_polled: Instant,
    /// Blocks of a streamed frame that were built before its last unit arrived.
    early_blocks: usize,
    packetized: Instant,
    prepared: Instant,
    latency: u64,
    processing: u64,
    age: u64,
    claim: Option<Claim>,
    /// The RTP timestamp on the wire, for the timing record.
    timestamp: u32,
    /// Set when this is one FEC block of a streamed frame, not a whole frame.
    stream: Option<Part>,
    /// The encoder returned the frame whole, even when it goes out block by
    /// block; false for a frame the encoder delivered in units.
    whole: bool,
    /// On the first part of a whole frame sent block by block: the frame's
    /// wire bytes, which the keyframe recovery estimate counts from the start.
    frame_wire_bytes: Option<usize>,
}
impl Prepared {
    /// A later block of a whole frame sent block by block: the frame is
    /// already counted, and its keyframe already timed, from its first.
    fn continues_whole(&self) -> bool {
        self.whole && self.stream.is_some_and(|part| !part.first)
    }
}
/// Where a block sits in a streamed frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Part {
    first: bool,
    last: bool,
}
/// Blocks kept for reuse. A frame's layout takes at most four blocks, and
/// up to four frames hold blocks at once: the one being packetized, the
/// ready one, and the two the paced sender is sending or has in flight. Four
/// times four is 16; blocks past that are freed outside the lock.
const RECYCLED_BLOCKS: usize = 16;

struct State {
    pending: Option<Frame>,
    sending: bool,
    sending_keyframe: bool,
    ready: Option<Prepared>,
    transmitting: bool,
    transmitting_keyframe: bool,
    /// Updated after actual send progress, never a wall-clock-only countdown.
    transmitting_remaining: Option<Duration>,
    /// The stream thread's last recovery lead: send progress wakes it only
    /// when the remaining time crosses it.
    recovery_lead: Option<Duration>,
    /// Blocks the packetizer fills again instead of allocating their shard
    /// buffers. Kept per block, so a whole frame and a block-by-block frame
    /// draw on the same pool.
    recycled: Vec<PacketBlock>,
    /// A streamed frame is open on the packetizer thread: some of its blocks
    /// may be out and the encoder still produces the rest. It counts toward
    /// the claim backlog only, so a new picture is not claimed (and aged)
    /// behind it, while recreation still waits just for what is queued.
    streaming: bool,
    /// A whole frame goes to the paced sender block by block: it counts once,
    /// though it is being packetized and sent at the same time.
    whole_open: bool,
    next_wire_frame: u32,
    skipped: u32,
    error: Option<String>,
}
impl State {
    /// A client waiting for a keyframe throws away the frames before it.
    /// The skipped frame keeps its wire number, so the encoder's frame
    /// numbers (reference frame invalidation) still match the wire.
    /// A streamed frame's units and blocks are not skipped: the frame is
    /// already half on the wire, and dropping a piece of it would leave a
    /// frame the client cannot finish and an index nobody consumed.
    /// Blocks past the pool's cap go to `spill`, which the caller drops after
    /// releasing the state lock.
    fn skip_pending(&mut self, spill: &mut Vec<PacketBlock>) -> bool {
        let raw = self
            .pending
            .as_ref()
            .is_some_and(|frame| !frame.encoded.idr && frame.encoded.unit == Unit::Whole);
        if raw {
            self.pending = None;
            self.skipped = self.skipped.wrapping_add(1);
        }
        let prepared = self
            .ready
            .as_ref()
            .is_some_and(|frame| !frame.idr && frame.stream.is_none());
        if prepared {
            // Packetization has already spent this frame's numbers and IVs.
            let frame = self.ready.take().unwrap();
            self.recycle(frame.blocks, spill);
        }
        raw || prepared
    }
    /// Takes a block from the pool, or an empty one that allocates on use.
    fn take_block(&mut self) -> PacketBlock {
        self.recycled.pop().unwrap_or_default()
    }
    /// Returns blocks to the pool up to [`RECYCLED_BLOCKS`]; the rest go to
    /// `spill`, so that nothing is freed while the state lock is held.
    fn recycle(
        &mut self,
        blocks: impl IntoIterator<Item = PacketBlock>,
        spill: &mut Vec<PacketBlock>,
    ) {
        for block in blocks {
            if self.recycled.len() < RECYCLED_BLOCKS {
                self.recycled.push(block);
            } else {
                spill.push(block);
            }
        }
    }
    fn backlog(&self) -> Result<usize> {
        if let Some(error) = &self.error {
            bail!("video sender stopped: {error}");
        }
        let frame = if self.whole_open {
            1
        } else {
            usize::from(self.sending)
                + usize::from(self.transmitting)
                + usize::from(
                    self.ready
                        .as_ref()
                        .is_some_and(|f| !(self.transmitting && f.continues_whole())),
                )
        };
        Ok(frame + usize::from(self.pending.is_some()))
    }
    fn claim_backlog(&self, recovery_lead: Option<Duration>) -> Result<usize> {
        Ok(self.backlog()?
            + usize::from(
                self.streaming
                    || self.sending_keyframe
                    || (self.transmitting_keyframe
                        && !self
                            .transmitting_remaining
                            .zip(recovery_lead)
                            .is_some_and(|(remaining, lead)| remaining <= lead))
                    || self.pending.as_ref().is_some_and(|f| f.encoded.idr)
                    || self
                        .ready
                        .as_ref()
                        .is_some_and(|f| f.idr && !f.continues_whole()),
            ))
    }
}
struct Slot {
    state: Mutex<State>,
    /// The pending slot and the backlog: the packetizer waits here for a
    /// frame, the stream thread for room. The paced sender never does.
    changed: Condvar,
    /// The prepared slot: the paced sender waits here for a frame, the
    /// packetizer for room.
    ready_changed: Condvar,
    stop: AtomicBool,
    stream_id: String,
}
impl Slot {
    fn finish(&self, result: Result<()>) {
        let mut state = self.state.lock().unwrap();
        if let Err(error) = result {
            state.error.get_or_insert_with(|| format!("{error:#}"));
        }
        self.stop.store(true, Ordering::Release);
        state.pending = None;
        state.ready = None;
        state.sending = false;
        state.sending_keyframe = false;
        state.streaming = false;
        state.whole_open = false;
        state.transmitting = false;
        state.transmitting_keyframe = false;
        state.transmitting_remaining = None;
        self.changed.notify_all();
        self.ready_changed.notify_all();
    }
    fn enqueue(&self, frame: Prepared) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        while state.ready.is_some() && !self.stop.load(Ordering::Acquire) {
            state = self.ready_changed.wait(state).unwrap();
        }
        state.backlog()?;
        if !self.stop.load(Ordering::Acquire) {
            // The backlog does not shrink: the frame was counted as being sent.
            state.ready = Some(frame);
            self.ready_changed.notify_one();
        }
        Ok(())
    }
    fn run(&self, mut send: impl FnMut(Frame) -> Result<u32>) -> Result<()> {
        loop {
            let frame = {
                let mut state = self.state.lock().unwrap();
                while state.pending.is_none() && !self.stop.load(Ordering::Acquire) {
                    state = self.changed.wait(state).unwrap();
                }
                if self.stop.load(Ordering::Acquire) {
                    return Ok(());
                }
                state.sending = true;
                let mut frame = state.pending.take().unwrap();
                state.sending_keyframe = frame.encoded.idr;
                frame.skipped = std::mem::take(&mut state.skipped);
                self.changed.notify_all();
                frame
            };
            let next_wire_frame = send(frame)?;
            let mut state = self.state.lock().unwrap();
            state.next_wire_frame = next_wire_frame;
            state.sending = false;
            state.sending_keyframe = false;
            // With `sending`: the frame's blocks are all queued or out, and it
            // still counts once.
            state.whole_open = false;
            self.changed.notify_all();
        }
    }
    /// The packetizer gave up a whole frame part way through its blocks. A
    /// frame given up while the sender waits for its next block is no longer
    /// a keyframe in flight; one whose block is being sent ends with it.
    fn abandon_whole(&self) {
        let mut state = self.state.lock().unwrap();
        state.whole_open = false;
        if !state.transmitting && state.ready.is_none() {
            state.transmitting_keyframe = false;
            state.transmitting_remaining = None;
        }
        self.changed.notify_all();
    }
    fn stop(&self) {
        let _state = self.state.lock().unwrap();
        self.stop.store(true, Ordering::Release);
        self.changed.notify_all();
        self.ready_changed.notify_all();
    }
    /// A frame was skipped from the pending or the prepared slot: room for
    /// the stream thread, and for a packetizer waiting to hand over.
    fn skipped(&self) {
        self.changed.notify_all();
        self.ready_changed.notify_one();
    }
}
/// Where the stream thread's own sends left off. A sender started after them
/// continues their frame, packet and IV numbering and their pacing.
pub struct Handoff {
    pub packetizer: VideoPacketizer,
    pub present_stamper: Option<butterpollo_windows::present_timing::Stamper>,
    pub last_stamp: Instant,
    pub pacing_due: Instant,
    pub send_loss: butterpollo_core::stream_policy::SendLossRecovery,
    /// FEC blocks per streamed frame; 0 when the stream does not stream.
    /// A rebuilt encoder's count follows through [`Sender::follow_slice_blocks`].
    pub slice_blocks: usize,
    /// Slices per frame the encoder was asked for.
    pub slices: usize,
    /// `BUTTERPOLLO_TEST_SEND_JITTER`'s stalls, for the paced sender.
    pub send_jitter: Option<butterpollo_core::stream_policy::SendJitter>,
    /// The stream's route, shared with the encode loop.
    pub link: Arc<link_watch::LinkWatch>,
}
/// The priority the packetizer and paced sender run at. With
/// `video_send_mmcss_only`, MMCSS's boost alone, as the input thread has it:
/// the thread priority [`Priority::new`] sets after it cancels the boost.
///
/// [`Priority::new`]: butterpollo_windows::capture::Priority::new
pub(crate) fn send_priority(mmcss_only: bool) -> butterpollo_windows::capture::Priority {
    if mmcss_only {
        butterpollo_windows::capture::Priority::input()
    } else {
        butterpollo_windows::capture::Priority::new()
    }
}
pub struct Sender {
    slot: Arc<Slot>,
    worker: Option<thread::JoinHandle<()>>,
    transport: Option<thread::JoinHandle<()>>,
    /// FEC blocks the next streamed frame is planned for.
    slice_blocks: AtomicUsize,
}
impl Sender {
    pub fn new(
        socket: Arc<ReplySocket>,
        s: Arc<Session>,
        c: Config,
        h: Shared,
        start: Instant,
        handoff: Handoff,
    ) -> Result<Self> {
        let mut handoff = handoff;
        let pacing_due = handoff.pacing_due;
        let send_loss = std::mem::take(&mut handoff.send_loss);
        let jitter = handoff.send_jitter.take();
        let link = handoff.link.clone();
        let mmcss_only = c.boolean("video_send_mmcss_only", false);
        let early_blocks = c.boolean("video_send_early_blocks", true);
        let mut sender =
            Self::packetizer_with(s.clone(), start, handoff, mmcss_only, early_blocks)?;
        let shared = sender.slot.clone();
        sender.transport = Some(thread::Builder::new().name("video-send".into()).spawn(
            move || {
                let _priority = send_priority(mmcss_only);
                let result = transport::run(
                    &shared, socket, s, c, h, start, pacing_due, send_loss, jitter, link,
                );
                shared.finish(result);
            },
        )?);
        Ok(sender)
    }

    /// A packetizer that hands over whole frames only, for tests that take
    /// its frames in place of a paced sender.
    #[cfg(test)]
    fn packetizer(
        s: Arc<Session>,
        start: Instant,
        handoff: Handoff,
        mmcss_only: bool,
    ) -> Result<Self> {
        Self::packetizer_with(s, start, handoff, mmcss_only, false)
    }

    /// With `early_blocks`, a whole frame goes to an idle paced sender block
    /// by block, each as it is built (`video_send_early_blocks`).
    fn packetizer_with(
        s: Arc<Session>,
        start: Instant,
        handoff: Handoff,
        mmcss_only: bool,
        early_blocks: bool,
    ) -> Result<Self> {
        let Handoff {
            mut packetizer,
            mut present_stamper,
            mut last_stamp,
            slice_blocks,
            slices,
            ..
        } = handoff;
        let next_wire_frame = packetizer.frame;
        let sender = Self::spawn(s.launch.id.clone(), next_wire_frame, move |shared| {
            let _priority = send_priority(mmcss_only);
            let mut fec_reported = None;
            let mut streams = streamed::Streams::new(slices);
            // Frames skipped while a streamed frame was open, owed to the next one.
            let mut owed = 0u32;
            shared.run(|queued| {
                let Frame {
                    encoded: frame, peer, claimed, polled, latency: encode,
                    processing_started, skipped, claim, slice_blocks,
                } = queued;
                owed = owed.wrapping_add(skipped);
                if frame.unit != Unit::Whole && !streamed::begins(frame.unit) {
                    // A later unit of the streamed frame: its blocks go out as they complete.
                    streams.unit(
                        shared, &s, &mut packetizer, &frame.bytes, streamed::is_last(frame.unit),
                        polled, encode, &mut fec_reported,
                    )?;
                    shared.state.lock().unwrap().streaming = streams.is_open();
                    return Ok(streams.next_wire_frame(&packetizer));
                }
                // A frame that begins while another is open means the other lost its last unit.
                if streams.is_open() {
                    streams.interrupted(&s, &mut packetizer);
                    shared.state.lock().unwrap().streaming = false;
                }
                packetizer.frame = packetizer.frame.wrapping_add(std::mem::take(&mut owed));
                let captured = frame.presentation.unwrap_or(claimed);
                let stamp = present_stamper.as_mut().map_or(captured, |stamper| {
                    stamper.stamp(captured, &s.output.read().unwrap())
                }).max(last_stamp + Duration::from_nanos(11_112));
                last_stamp = stamp;
                let timestamp = (stamp.saturating_duration_since(start).as_secs_f64() * 90000.) as u64 as u32;
                if frame.unit != Unit::Whole {
                    // The first unit of a streamed frame.
                    let packetized = Instant::now();
                    packetizer.fec_percent = s.frame_fec_percent(false);
                    streams.begin(&s, &mut packetizer, streamed::Start {
                        peer, claimed, polled, packetized,
                        processing: micros(packetized.saturating_duration_since(processing_started)),
                        age: micros(claimed.saturating_duration_since(captured)),
                        claim, timestamp, cap: frame.frame_cap_bytes, blocks: slice_blocks,
                    });
                    streams.unit(
                        shared, &s, &mut packetizer, &frame.bytes, streamed::is_last(frame.unit),
                        polled, encode, &mut fec_reported,
                    )?;
                    shared.state.lock().unwrap().streaming = streams.is_open();
                    return Ok(streams.next_wire_frame(&packetizer));
                }
                if frame.bytes.is_empty() {
                    return Ok(packetizer.frame);
                }
                packetizer.fec_percent = s.video_fec_percent(frame.idr || frame.after_invalidation, frame.bytes.len());
                let wire_frame = packetizer.frame;
                let layout = packetizer.block_layout(frame.bytes.len());
                let fec_limited = packetizer.fec_limited(frame.bytes.len());
                let wire = packetizer.clone();
                // Punktfunk's GameStream boundary: this loop's capture/encode
                // tick through entry to packetization, before the paced sender.
                // A backlog at this packetizer remains part of the measurement.
                let packetized = Instant::now();
                let processing = micros(packetized.saturating_duration_since(processing_started));
                let details = |blocks, counts, layout, stream, frame_wire_bytes| Prepared {
                    blocks, counts, layout, packetizer: wire.clone(), wire_frame, idr: frame.idr,
                    after_invalidation: frame.after_invalidation, frame_cap_bytes: frame.frame_cap_bytes,
                    cap_target_bytes: frame.frame_cap_target, reencoded: frame.reencoded, fec_limited,
                    payload_bytes: frame.bytes.len(), peer, claimed, polled, last_polled: polled,
                    early_blocks: 0, packetized,
                    prepared: Instant::now(), latency: micros(encode), processing,
                    age: micros(claimed.saturating_duration_since(captured)), claim, timestamp,
                    stream, whole: true, frame_wire_bytes,
                };
                // An idle paced sender takes each block as it is built, as before the
                // sender had its own thread (6772d4019); a busy one gets the whole frame,
                // which a keyframe can still skip.
                let early = early_blocks && {
                    let mut state = shared.state.lock().unwrap();
                    state.whole_open = state.ready.is_none() && !state.transmitting;
                    state.whole_open
                };
                if early {
                    let dropped = early::send(shared, &mut packetizer, &frame, timestamp, processing, &layout, peer, |blocks, counts, block, part, frame_wire_bytes| {
                        details(blocks, counts, vec![block], Some(part), frame_wire_bytes)
                    });
                    if let Some(error) = dropped? {
                        shared.abandon_whole();
                        s.launch.warnings.event("network_frame", format!("Encoded video frame dropped ({error:#}); requesting a recovery frame. Lower bitrate or resolution to stay within Moonlight's packet limit."), butterpollo_core::session::EVENT_PERIOD);
                        s.request_host_keyframe();
                        return Ok(packetizer.frame);
                    }
                    if !frame.idr && fec_limited
                        && fec_reported.is_none_or(|at: Instant| at.elapsed() >= Duration::from_secs(1))
                    {
                        fec_reported = Some(Instant::now());
                        s.launch.warnings.event("network_fec", "FEC was reduced or omitted for large video frames because they exceed Moonlight's four-block limit. Packet loss in those frames is harder to recover; lower bitrate or resolution to keep full FEC protection.", butterpollo_core::session::EVENT_PERIOD);
                    }
                    tracing::trace!(target: "pacing", stream_id = %s.launch.id,
                        frame = wire_frame, presentation = ?frame.presentation,
                        "packetized picture block by block");
                    return Ok(packetizer.frame);
                }
                let mut blocks: Vec<PacketBlock> = {
                    let mut state = shared.state.lock().unwrap();
                    (0..layout.len()).map(|_| state.take_block()).collect()
                };
                let mut counts = Vec::with_capacity(layout.len());
                let prepared = (|| -> Result<()> {
                    let (_, mut next) = packetizer.conventional_blocks(
                        &frame.bytes, frame.idr, frame.after_invalidation, timestamp, processing,
                    )?;
                    loop {
                        if counts.len() == blocks.len() {
                            blocks.push(Vec::new());
                        }
                        let count = next(&mut blocks[counts.len()])?;
                        if count == 0 { break; }
                        counts.push(count);
                    }
                    Ok(())
                })();
                // The block that only ended the loop (and any the layout over-counted) goes back
                // to the pool: left at the end of the frame it would be the next frame's block 0,
                // empty, every time.
                if prepared.is_ok() && blocks.len() > counts.len() {
                    let unused = blocks.split_off(counts.len());
                    let mut spill = Vec::new();
                    shared.state.lock().unwrap().recycle(unused, &mut spill);
                }
                if let Err(error) = prepared {
                    s.launch.warnings.event("network_frame", format!("Encoded video frame dropped ({error:#}); requesting a recovery frame. Lower bitrate or resolution to stay within Moonlight's packet limit."), butterpollo_core::session::EVENT_PERIOD);
                    s.request_host_keyframe();
                    let mut spill = Vec::new();
                    shared.state.lock().unwrap().recycle(blocks, &mut spill);
                    return Ok(packetizer.frame);
                }
                if !frame.idr && fec_limited
                    && fec_reported.is_none_or(|at: Instant| at.elapsed() >= Duration::from_secs(1))
                {
                    fec_reported = Some(Instant::now());
                    s.launch.warnings.event("network_fec", "FEC was reduced or omitted for large video frames because they exceed Moonlight's four-block limit. Packet loss in those frames is harder to recover; lower bitrate or resolution to keep full FEC protection.", butterpollo_core::session::EVENT_PERIOD);
                }
                tracing::trace!(target: "pacing", stream_id = %s.launch.id,
                    frame = wire_frame, presentation = ?frame.presentation,
                    "packetized picture");
                shared.enqueue(details(blocks, counts, layout, None, None))?;
                Ok(packetizer.frame)
            })
        })?;
        sender.follow_slice_blocks(slice_blocks);
        Ok(sender)
    }

    /// Plan the streamed frames submitted from now on for `blocks` FEC
    /// blocks, as a rebuilt encoder's slice delivery says; frames already
    /// submitted keep the count they were submitted with.
    pub fn follow_slice_blocks(&self, blocks: usize) {
        self.slice_blocks.store(blocks, Ordering::Relaxed);
    }

    fn spawn(
        stream_id: String,
        next_wire_frame: u32,
        run: impl FnOnce(&Slot) -> Result<()> + Send + 'static,
    ) -> Result<Self> {
        let slot = Arc::new(Slot {
            state: Mutex::new(State {
                pending: None,
                sending: false,
                sending_keyframe: false,
                ready: None,
                transmitting: false,
                transmitting_keyframe: false,
                transmitting_remaining: None,
                recovery_lead: None,
                recycled: Vec::new(),
                streaming: false,
                whole_open: false,
                next_wire_frame,
                skipped: 0,
                error: None,
            }),
            changed: Condvar::new(),
            ready_changed: Condvar::new(),
            stop: AtomicBool::new(false),
            stream_id,
        });
        let shared = slot.clone();
        let worker = thread::Builder::new()
            .name("video-packetize".into())
            .spawn(move || {
                let result = run(&shared);
                shared.finish(result);
            })?;
        Ok(Self {
            slot,
            worker: Some(worker),
            transport: None,
            slice_blocks: AtomicUsize::new(0),
        })
    }

    #[cfg(test)]
    pub fn submit(
        &self,
        frames: Vec<Encoded>,
        peer: SocketAddr,
        latency: Duration,
        claims: &mut Claims,
    ) -> Result<()> {
        self.submit_tick(frames, peer, latency, claims, Instant::now() - latency)
    }
    /// Keeps the output tick separate from each picture's original claim.
    /// As in Punktfunk, asynchronous output is timed from the tick that polls
    /// it, while the actual encoder duration and picture age stay separate.
    pub fn submit_tick(
        &self,
        frames: Vec<Encoded>,
        peer: SocketAddr,
        latency: Duration,
        claims: &mut Claims,
        processing_started: Instant,
    ) -> Result<()> {
        let polled = Instant::now();
        // Declared before the guard, so it drops after the lock is released.
        let mut spill = Vec::new();
        let mut state = self.slot.state.lock().unwrap();
        state.backlog()?;
        for encoded in frames {
            // One claim per picture: a streamed frame takes it with its first unit.
            let claim = match encoded.unit {
                Unit::Whole | Unit::Slice { index: 0 } | Unit::Last { index: 0 } => {
                    claims.take(encoded.presentation)
                }
                _ => None,
            };
            let latency = encoded.latency.unwrap_or(latency);
            // Stamp every output from this poll before any capacity wait.
            let claimed = polled.checked_sub(latency).unwrap_or(polled);
            // A keyframe goes out next, not after the frame waiting before it.
            if encoded.idr && state.skip_pending(&mut spill) {
                tracing::debug!(stream_id=%self.slot.stream_id, "video frame queued before a keyframe skipped");
                self.slot.skipped();
            }
            while state.pending.is_some() && !self.slot.stop.load(Ordering::Acquire) {
                state = self.slot.changed.wait(state).unwrap();
                state.backlog()?;
            }
            if self.slot.stop.load(Ordering::Acquire) {
                return Ok(());
            }
            state.pending = Some(Frame {
                encoded,
                peer,
                claimed,
                polled,
                latency,
                processing_started,
                skipped: 0,
                claim,
                slice_blocks: self.slice_blocks.load(Ordering::Relaxed),
            });
            tracing::trace!(target: "pacing", pending=usize::from(state.pending.is_some()),
                sending=usize::from(state.sending), inventory=state.backlog()?,
                stream_id=%self.slot.stream_id, "send queue");
            self.slot.changed.notify_all();
        }
        Ok(())
    }
    pub fn backlog(&self) -> Result<usize> {
        self.slot.state.lock().unwrap().backlog()
    }
    pub fn claim_backlog(&self, recovery_lead: Option<Duration>) -> Result<usize> {
        let mut state = self.slot.state.lock().unwrap();
        state.recovery_lead = recovery_lead;
        state.claim_backlog(recovery_lead)
    }
    pub fn wait_for_capacity(
        &self,
        limit: usize,
        timeout: Duration,
        recovery_lead: Option<Duration>,
    ) -> Result<()> {
        let mut state = self.slot.state.lock().unwrap();
        state.recovery_lead = recovery_lead;
        let (state, _) = self
            .slot
            .changed
            .wait_timeout_while(state, timeout, |state| {
                state
                    .claim_backlog(recovery_lead)
                    .is_ok_and(|backlog| backlog >= limit)
                    && !self.slot.stop.load(Ordering::Acquire)
            })
            .unwrap();
        state.backlog()?;
        Ok(())
    }
    /// For a keyframe request: drops the frame waiting to be sent, which the
    /// client would discard, so the keyframe is claimed without waiting for
    /// it. Returns the backlog left.
    pub fn skip_pending(&self) -> Result<usize> {
        // Declared before the guard, so it drops after the lock is released.
        let mut spill = Vec::new();
        let mut state = self.slot.state.lock().unwrap();
        state.backlog()?;
        if state.skip_pending(&mut spill) {
            tracing::debug!(stream_id=%self.slot.stream_id, "video frame queued before a keyframe request skipped");
            self.slot.skipped();
        }
        state.backlog()
    }
    /// Recreated encoders must start after every accepted output, including
    /// frames whose packetization has not yet advanced the wire index.
    pub fn next_wire_frame(&self) -> Result<u64> {
        let mut state = self.slot.state.lock().unwrap();
        while state.backlog()? != 0 && !self.slot.stop.load(Ordering::Acquire) {
            state = self.slot.changed.wait(state).unwrap();
        }
        Ok(u64::from(state.next_wire_frame.wrapping_add(state.skipped)))
    }
}
impl Drop for Sender {
    fn drop(&mut self) {
        self.slot.stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Some(transport) = self.transport.take() {
            let _ = transport.join();
        }
    }
}

#[cfg(test)]
mod tests;
