//! Ordered conventional video output: one sending frame and one pending frame.
use crate::state::{Session, Shared};
use anyhow::{Result, bail};
use butterpollo_core::{
    config::Config,
    packet::VideoPacketizer,
    performance::{Claim, Claims},
};
use butterpollo_windows::{encoder::Encoded, net::Batch, timing::Timer};
use std::{
    net::{SocketAddr, UdpSocket},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

struct Frame {
    encoded: Encoded,
    peer: SocketAddr,
    claimed: Instant,
    polled: Instant,
    latency: Duration,
    /// Frames skipped just before this one; their wire numbers stay used.
    skipped: u32,
    /// A new picture's claim, for the stage timings.
    claim: Option<Claim>,
}
struct State {
    pending: Option<Frame>,
    sending: bool,
    sending_keyframe: bool,
    next_wire_frame: u32,
    skipped: u32,
    error: Option<String>,
}
impl State {
    /// A client waiting for a keyframe throws away the frames before it.
    /// The skipped frame keeps its wire number, so the encoder's frame
    /// numbers (reference frame invalidation) still match the wire.
    fn skip_pending(&mut self) -> bool {
        if !self
            .pending
            .as_ref()
            .is_some_and(|frame| !frame.encoded.idr)
        {
            return false;
        }
        self.pending = None;
        self.skipped = self.skipped.wrapping_add(1);
        true
    }
    fn backlog(&self) -> Result<usize> {
        if let Some(error) = &self.error {
            bail!("video sender stopped: {error}");
        }
        Ok(usize::from(self.sending) + usize::from(self.pending.is_some()))
    }
    fn claim_backlog(&self) -> Result<usize> {
        Ok(self.backlog()?
            + usize::from(
                self.sending_keyframe || self.pending.as_ref().is_some_and(|f| f.encoded.idr),
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
}
pub struct Sender {
    slot: Arc<Slot>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Sender {
    pub fn new(
        socket: Arc<UdpSocket>,
        s: Arc<Session>,
        c: Config,
        h: Shared,
        start: Instant,
        handoff: Handoff,
    ) -> Result<Self> {
        let Handoff {
            mut packetizer,
            mut present_stamper,
            mut last_stamp,
            pacing_due,
            mut send_loss,
        } = handoff;
        let next_wire_frame = packetizer.frame;
        Self::spawn(s.launch.id.clone(), next_wire_frame, move |shared| {
            let _priority = butterpollo_windows::capture::Priority::new();
            let timer = Timer::new()?;
            let mut batch = Batch::default();
            let mut packets = Vec::new();
            let trace_send = tracing::enabled!(target: "pacing", tracing::Level::TRACE);
            batch.waits = trace_send.then(Default::default);
            let mut network_pacer = butterpollo_core::network_pacing::Pacer::new(pacing_due);
            let mut link = None;
            let mut link_due = Instant::now();
            let mut reported_pacing = None;
            let mut fec_reported = None;
            let send_outage = butterpollo_core::stream_policy::SendOutage::from_env();
            let batch_kb = match c.integer("video_max_batch_size_kb", 64) {
                16 => 16,
                32 => 32,
                _ => 64,
            };
            shared.run(|queued| {
                let Frame { encoded: frame, peer, claimed, polled, latency: encode, skipped, claim } = queued;
                // The stream's rate, which the client can change mid-stream (0x5532).
                let period = s.stream_mode().period();
                packetizer.frame = packetizer.frame.wrapping_add(skipped);
                let micros = |d: Duration| d.as_micros().min(u128::from(u64::MAX)) as u64;
                let latency = micros(encode);
                s.stats.latency_us.store(latency, Ordering::Relaxed);
                let captured = frame.presentation.unwrap_or(claimed);
                let age = micros(claimed.saturating_duration_since(captured));
                let dequeued = Instant::now();
                let queue_wait = micros(dequeued.saturating_duration_since(polled));
                // Moonlight's host latency runs from the claim to this
                // dequeue: where punktfunk's runs (`cap_at`, its encode loop's
                // tick, to its packetizer thread). Present to sent covers the
                // whole span.
                let processing = micros(dequeued.saturating_duration_since(claim.map_or(claimed, |c| c.claimed)));
                let stamp = present_stamper.as_mut().map_or(captured, |stamper| stamper.stamp(captured, &s.output.read().unwrap())).max(last_stamp + Duration::from_nanos(11_112));
                last_stamp = stamp;
                // Wrap like the previous host; a saturating cast
                // froze the clock after 13.25 hours.
                let timestamp = (stamp.saturating_duration_since(start).as_secs_f64() * 90000.) as u64 as u32;
                if frame.bytes.is_empty() {
                    return Ok(packetizer.frame);
                }
                packetizer.fec_percent = s.frame_fec_percent(frame.idr || frame.after_invalidation);
                let wire_frame = packetizer.frame;
                let block_layout = packetizer.block_layout(frame.bytes.len());
                let fec_limited = packetizer.fec_limited(frame.bytes.len());
                let packet_bytes = packetizer.packet_size + 16 + if packetizer.key.is_some() { 32 } else { 0 };
                let route = *link.get_or_insert_with(|| butterpollo_windows::net::routed_link(peer));
                let bitrate = s.bitrate.load(Ordering::Relaxed);
                let needed = butterpollo_core::network_pacing::video_wire_rate_bps(&packetizer, bitrate, period, peer.is_ipv6());
                let bps = butterpollo_core::network_pacing::rate_bps(
                    c.integer("pacing_max_bitrate_kbps", 0),
                    bitrate,
                    route.bps,
                    route.wireless,
                    needed,
                );
                let bps = s.video_pacing_bps(bps, needed);
                let safe = butterpollo_core::network_pacing::rate_bps(0, bitrate, route.bps, true, needed);
                let bps = s.keyframe_loss_bps(bps, safe, wire_frame, frame.idr);
                if Instant::now() >= link_due {
                    butterpollo_core::network_pacing::report_rate(&s.launch.warnings, bps, needed, c.integer("pacing_max_bitrate_kbps", 0));
                    if reported_pacing != Some(bps) {
                        tracing::info!(pacing_bps=bps, wire_budget_bps=needed, link_bps=route.bps, configured_kbps=c.integer("pacing_max_bitrate_kbps", 0), "network pacing selected");
                        reported_pacing = Some(bps);
                    }
                }
                // A frame beyond Moonlight's packet limit (very high
                // bitrates) costs that frame and a keyframe, not the
                // session.
                let packetized = Instant::now();
                let prepared_blocks = packetizer.conventional_blocks(&frame.bytes,frame.idr,frame.after_invalidation,timestamp,processing);
                if let Err(error) = &prepared_blocks {
                    s.launch.warnings.event("network_frame", format!("Encoded video frame dropped ({error:#}); requesting a recovery frame. Lower bitrate or resolution to stay within Moonlight's packet limit."), butterpollo_core::session::EVENT_PERIOD);
                    s.request_idr();
                    drop(prepared_blocks);
                    return Ok(packetizer.frame);
                }
                let (packet_count, mut next_block) = prepared_blocks?;
                // A keyframe, as at the start of every stream, may be too
                // large for FEC; only ordinary frames make this worth showing.
                if !frame.idr
                    && fec_limited
                    && fec_reported.is_none_or(|at: Instant| at.elapsed() >= Duration::from_secs(1))
                {
                    fec_reported = Some(Instant::now());
                    s.launch.warnings.event("network_fec", "FEC was reduced or omitted for large video frames because they exceed Moonlight's four-block limit. Packet loss in those frames is harder to recover; lower bitrate or resolution to keep full FEC protection.", butterpollo_core::session::EVENT_PERIOD);
                }
                let frame_bytes = (packet_count * packet_bytes) as u64;
                let dropped = batch.dropped;
                let waits_before = batch.waits;
                let mut pacing_wait = Duration::ZERO;
                let mut send_time = Duration::ZERO;
                let mut batches = 0;
                let mut first_send = None;
                let mut last_send = None;
                let mut first_sent = None;
                let mut lost = butterpollo_core::stream_policy::SendLoss::new(block_layout);
                let mut abandoned = 0;
                let mut sent_packets = 0;
                'blocks: loop {
                    let block_count = match next_block(&mut packets) {
                        Ok(count) => count,
                        Err(error) => {
                            s.launch.warnings.event("network_frame", format!("Encoded video frame dropped ({error:#}); requesting a recovery frame. Lower bitrate or resolution to stay within Moonlight's packet limit."), butterpollo_core::session::EVENT_PERIOD);
                            s.request_idr();
                            drop(next_block);
                            return Ok(packetizer.frame);
                        }
                    };
                    if block_count == 0 { break; }
                    s.stats.video_frame.store(u64::from(wire_frame) + 1, Ordering::Release);
                    let mut remaining = &packets[..block_count];
                    while !remaining.is_empty() {
                        if shared.stop.load(Ordering::Acquire) || s.stopping() || h.stop.load(Ordering::Acquire) {
                            shared.stop();
                            drop(next_block);
                            return Ok(packetizer.frame);
                        }
                        let now = Instant::now();
                        if network_pacer.due() > now {
                            timer.until_precise(network_pacer.due());
                            if trace_send { pacing_wait += now.elapsed(); }
                        }
                        let budget = butterpollo_core::network_pacing::batch_payload_limit(
                            bps, remaining[0].len(), peer.is_ipv6(), batch_kb * 1024,
                        );
                        let count =
                            butterpollo_windows::net::Batch::count(remaining, budget);
                        let send_started = trace_send.then(Instant::now);
                        let refused = batch.dropped;
                        let bytes = match send_outage.filter(|outage| outage.active(start, Instant::now())) {
                            // Lost after the socket: counted as sent, only the client sees it.
                            Some(outage) if outage.in_air() => remaining[..count].iter().map(Vec::len).sum(),
                            Some(_) => {
                                batch.refuse(count);
                                0
                            }
                            None => batch.send(&socket, &remaining[..count], peer)?,
                        };
                        first_sent.get_or_insert_with(Instant::now);
                        if let Some(send_started) = send_started {
                            let finished = Instant::now();
                            first_send.get_or_insert(send_started);
                            last_send = Some(finished);
                            send_time += finished.duration_since(send_started);
                            batches += 1;
                        }
                        remaining = &remaining[count..];
                        sent_packets += count;
                        network_pacer.sent(Instant::now(), bytes, if bytes > 0 { count } else { 0 }, peer.ip().to_canonical().is_ipv6(), bps);
                        s.stats.packets.fetch_add(count as u64, Ordering::Relaxed);
                        s.stats.bytes.fetch_add(bytes as u64, Ordering::Relaxed);
                        // As on the inline path: a frame beyond FEC's repair is
                        // not sent further, and the next frame is a keyframe.
                        if lost.sent(count, (batch.dropped - refused) as usize) {
                            abandoned = packet_count - sent_packets;
                            if send_loss.lost(Instant::now(), period, lost.reached_socket()) {
                                s.request_send_loss_recovery();
                            }
                            break 'blocks;
                        }
                    }
                }
                drop(next_block);
                if lost.unrecoverable() {
                    tracing::debug!(frame = packetizer.frame.wrapping_sub(1), idr = frame.idr, abandoned, "video frame lost on send; the rest of it was not sent");
                } else {
                    send_loss.delivered(frame.idr);
                }
                if batch.dropped != dropped {
                    s.launch.warnings.event("network_send", "Video packets were dropped by the host after transient socket send failures. You may see stutter or recovery frames; lower bitrate and check the network adapter. The log includes the socket error code.", butterpollo_core::session::EVENT_PERIOD);
                }
                s.stats.frames.fetch_add(1, Ordering::Relaxed);
                let sent = Instant::now();
                if let (Some(first), Some(last), Some(before), Some(after)) = (first_send, last_send, waits_before, batch.waits) {
                    tracing::trace!(target: "pacing", frame=packetizer.frame.wrapping_sub(1), packets=packet_count, batches, queue_wait_us=queue_wait,
                        pacing_wait_us=micros(pacing_wait), send_us=micros(send_time),
                        first_send_us=micros(first.saturating_duration_since(start)), last_send_us=micros(last.saturating_duration_since(start)),
                        writable_waits=after.count-before.count, writable_wait_us=micros(after.elapsed-before.elapsed),
                        dropped=batch.dropped-dropped, stream_id=%s.launch.id, "send");
                }
                let mut performance = s.stats.performance.lock().unwrap();
                performance.record_timing(sent,butterpollo_core::performance::Timing{period,encode:latency,host:processing,age,sent:micros(sent.saturating_duration_since(claimed))},frame_bytes);
                // A frame abandoned on send never had its last packet sent.
                let stamps = claim.filter(|_| !lost.unrecoverable()).map(|claim| claim.sent(polled, packetized, first_sent.unwrap_or(sent), sent));
                if let Some(stamps) = &stamps {
                    performance.record_stages(stamps, processing);
                }
                drop(performance);
                if let Some(stamps) = &stamps {
                    s.host_timings.lock().unwrap().record(wire_frame, stamps);
                }
                // The interface lookup takes a moment: refresh the
                // link speed after the frame is out, for the next one.
                if Instant::now() >= link_due {
                    link = Some(butterpollo_windows::net::routed_link(peer));
                    link_due = Instant::now() + Duration::from_secs(2);
                }
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
            .name("video-send".into())
            .spawn(move || {
                let result = run(&shared);
                let mut state = shared.state.lock().unwrap();
                if let Err(error) = result {
                    state.error = Some(format!("{error:#}"));
                }
                shared.stop.store(true, Ordering::Release);
                state.pending = None;
                state.sending = false;
                state.sending_keyframe = false;
                shared.changed.notify_all();
            })?;
        Ok(Self {
            slot,
            worker: Some(worker),
        })
    }

    /// Queues encoded frames, each with its picture's claim from `claims`.
    pub fn submit(
        &self,
        frames: Vec<Encoded>,
        peer: SocketAddr,
        latency: Duration,
        claims: &mut Claims,
    ) -> Result<()> {
        let polled = Instant::now();
        let mut state = self.slot.state.lock().unwrap();
        state.backlog()?;
        for encoded in frames {
            let claim = claims.take(encoded.presentation);
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
    pub fn claim_backlog(&self) -> Result<usize> {
        self.slot.state.lock().unwrap().claim_backlog()
    }
    pub fn wait_for_capacity(&self, limit: usize, timeout: Duration) -> Result<()> {
        let state = self.slot.state.lock().unwrap();
        let (state, _) = self
            .slot
            .changed
            .wait_timeout_while(state, timeout, |state| {
                state.claim_backlog().is_ok_and(|backlog| backlog >= limit)
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
    }
}

#[cfg(test)]
mod tests;
