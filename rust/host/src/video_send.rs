//! Ordered conventional output with separate packetization and paced sending.
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
        atomic::{AtomicBool, Ordering},
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
}
/// Where a block sits in a streamed frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Part {
    first: bool,
    last: bool,
}
struct State {
    pending: Option<Frame>,
    sending: bool,
    sending_keyframe: bool,
    ready: Option<Prepared>,
    transmitting: bool,
    transmitting_keyframe: bool,
    /// Updated after actual send progress, never a wall-clock-only countdown.
    transmitting_remaining: Option<Duration>,
    recycled: Vec<Vec<PacketBlock>>,
    /// A streamed frame is open on the packetizer thread: some of its blocks
    /// may be out and the encoder still produces the rest. It counts toward
    /// the claim backlog only, so a new picture is not claimed (and aged)
    /// behind it, while recreation still waits just for what is queued.
    streaming: bool,
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
    fn skip_pending(&mut self) -> bool {
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
            self.recycle(frame.blocks);
        }
        raw || prepared
    }
    fn recycle(&mut self, blocks: Vec<PacketBlock>) {
        if self.recycled.len() < 2 {
            self.recycled.push(blocks);
        }
    }
    fn backlog(&self) -> Result<usize> {
        if let Some(error) = &self.error {
            bail!("video sender stopped: {error}");
        }
        Ok(usize::from(self.sending)
            + usize::from(self.pending.is_some())
            + usize::from(self.transmitting)
            + usize::from(self.ready.is_some()))
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
                    || self.ready.as_ref().is_some_and(|f| f.idr),
            ))
    }
}
struct Slot {
    state: Mutex<State>,
    changed: Condvar,
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
        state.transmitting = false;
        state.transmitting_keyframe = false;
        state.transmitting_remaining = None;
        self.changed.notify_all();
    }
    fn enqueue(&self, frame: Prepared) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        while state.ready.is_some() && !self.stop.load(Ordering::Acquire) {
            state = self.changed.wait(state).unwrap();
        }
        state.backlog()?;
        if !self.stop.load(Ordering::Acquire) {
            state.ready = Some(frame);
            self.changed.notify_all();
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
            self.changed.notify_all();
        }
    }
    fn stop(&self) {
        let _state = self.state.lock().unwrap();
        self.stop.store(true, Ordering::Release);
        self.changed.notify_all();
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
    pub slice_blocks: usize,
    /// Slices per frame the encoder was asked for.
    pub slices: usize,
}
pub struct Sender {
    slot: Arc<Slot>,
    worker: Option<thread::JoinHandle<()>>,
    transport: Option<thread::JoinHandle<()>>,
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
        let mut sender = Self::packetizer(s.clone(), start, handoff)?;
        let shared = sender.slot.clone();
        sender.transport = Some(thread::Builder::new().name("video-send".into()).spawn(
            move || {
                let result = transport::run(&shared, socket, s, c, h, start, pacing_due, send_loss);
                shared.finish(result);
            },
        )?);
        Ok(sender)
    }

    fn packetizer(s: Arc<Session>, start: Instant, handoff: Handoff) -> Result<Self> {
        let Handoff {
            mut packetizer,
            mut present_stamper,
            mut last_stamp,
            slice_blocks,
            slices,
            ..
        } = handoff;
        let next_wire_frame = packetizer.frame;
        Self::spawn(s.launch.id.clone(), next_wire_frame, move |shared| {
            let _priority = butterpollo_windows::capture::Priority::new();
            let mut fec_reported = None;
            let mut streams = streamed::Streams::new(slice_blocks, slices);
            // Frames skipped while a streamed frame was open, owed to the next one.
            let mut owed = 0u32;
            shared.run(|queued| {
                let Frame {
                    encoded: frame, peer, claimed, polled, latency: encode,
                    processing_started, skipped, claim,
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
                        claim, timestamp, cap: frame.frame_cap_bytes,
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
                let mut blocks = shared.state.lock().unwrap().recycled.pop().unwrap_or_default();
                let mut counts = Vec::with_capacity(layout.len());
                // Punktfunk's GameStream boundary: this loop's capture/encode
                // tick through entry to packetization, before the paced sender.
                // A backlog at this packetizer remains part of the measurement.
                let packetized = Instant::now();
                let processing = micros(packetized.saturating_duration_since(processing_started));
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
                if let Err(error) = prepared {
                    s.launch.warnings.event("network_frame", format!("Encoded video frame dropped ({error:#}); requesting a recovery frame. Lower bitrate or resolution to stay within Moonlight's packet limit."), butterpollo_core::session::EVENT_PERIOD);
                    s.request_host_keyframe();
                    shared.state.lock().unwrap().recycle(blocks);
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
                shared.enqueue(Prepared {
                    blocks, counts, layout, packetizer: wire, wire_frame, idr: frame.idr,
                    after_invalidation: frame.after_invalidation, frame_cap_bytes: frame.frame_cap_bytes,
                    cap_target_bytes: frame.frame_cap_target, reencoded: frame.reencoded, fec_limited,
                    payload_bytes: frame.bytes.len(), peer, claimed, polled, last_polled: polled,
                    early_blocks: 0, packetized,
                    prepared: Instant::now(), latency: micros(encode), processing,
                    age: micros(claimed.saturating_duration_since(captured)), claim, timestamp,
                    stream: None,
                })?;
                Ok(packetizer.frame)
            })
        })
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
                recycled: Vec::new(),
                streaming: false,
                next_wire_frame,
                skipped: 0,
                error: None,
            }),
            changed: Condvar::new(),
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
            if encoded.idr && state.skip_pending() {
                tracing::debug!(stream_id=%self.slot.stream_id, "video frame queued before a keyframe skipped");
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
        self.slot.state.lock().unwrap().claim_backlog(recovery_lead)
    }
    pub fn wait_for_capacity(
        &self,
        limit: usize,
        timeout: Duration,
        recovery_lead: Option<Duration>,
    ) -> Result<()> {
        let state = self.slot.state.lock().unwrap();
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
        let mut state = self.slot.state.lock().unwrap();
        state.backlog()?;
        if state.skip_pending() {
            tracing::debug!(stream_id=%self.slot.stream_id, "video frame queued before a keyframe request skipped");
            self.slot.changed.notify_all();
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
