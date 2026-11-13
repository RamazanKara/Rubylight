//! Separate packetization and paced sending, retaining the newest intra frame.
mod transport;
use crate::state::{Session, Shared};
use anyhow::{Result, bail};
use butterpollo_core::{
    config::Config,
    packet::{BlockPool, PacketBlock, PyrowaveFec, VideoPacketizer},
    performance::{Claim, Claims},
    pyrowave::DetailFec,
    session::Warnings,
};
use butterpollo_windows::{encoder::Encoded, net::ReplySocket, timing::Signal};
use std::{
    collections::VecDeque,
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
    processing_started: Instant,
    latency: Duration,
    /// The encoder's frame came back from its poll.
    polled: Instant,
    /// A new picture's claim, for the stage timings.
    claim: Option<Claim>,
}
struct Prepared {
    blocks: Vec<PacketBlock>,
    peer: SocketAddr,
    claimed: Instant,
    processing: u64,
    latency: u64,
    age: u64,
    wire_frame: u32,
    demand: u64,
    kbps: u32,
    /// For the stage timings and the timing record: a new picture's claim, the poll that
    /// returned its frame, packetization's start and end, and the RTP timestamp.
    claim: Option<Claim>,
    polled: Instant,
    packetized: Instant,
    prepared: Instant,
    timestamp: u32,
    /// Handed over as its blocks were built: `blocks` holds block 0 and the
    /// rest follow in [`Ready::blocks`], and `prepared` is when block 0 was
    /// ready. A whole frame holds all of its blocks.
    streamed: bool,
}
/// The frame for the paced sender, and the blocks of a frame handed over as
/// they are built.
#[derive(Default)]
struct Ready {
    frame: Option<Prepared>,
    /// Blocks after the first of the streamed frame, in order.
    blocks: VecDeque<PacketBlock>,
    /// The packetizer still builds blocks of the streamed frame.
    open: bool,
    /// The paced sender has a frame in hand.
    sending: bool,
    /// A block of the frame in hand failed: the sender sends no more of it
    /// and does not count it as sent.
    abandoned: bool,
}
impl Ready {
    /// Nothing is queued or sending: a frame's first block would leave at once.
    fn idle(&self) -> bool {
        self.frame.is_none() && !self.sending && !self.open && self.blocks.is_empty()
    }
    /// Queue a whole frame, in place of any the sender has not taken: the
    /// newest intra frame wins. A streamed frame not taken yet is replaced
    /// with its later blocks. Returns whether a frame was replaced.
    fn replace(&mut self, frame: Prepared, pool: &BlockPool) -> bool {
        let Some(old) = self.frame.replace(frame) else {
            return false;
        };
        if old.streamed {
            for block in self.blocks.drain(..) {
                pool.give(block);
            }
        }
        for block in old.blocks {
            pool.give(block);
        }
        true
    }
}
#[derive(Default)]
struct DroppedFrames {
    unreported: u64,
    reported: Option<Instant>,
}
impl DroppedFrames {
    fn prepared<T>(
        &mut self,
        result: Result<T>,
        bytes: usize,
        now: Instant,
        warnings: &Warnings,
    ) -> Option<T> {
        match result {
            Ok(packets) => Some(packets),
            Err(error) => {
                warnings.event("pyrowave_frame", format!("PyroWave frame dropped ({error:#}); the client may hold the previous picture. Lower bitrate or resolution to stay within Moonlight's packet limit, or use HEVC/AV1."), butterpollo_core::session::EVENT_PERIOD);
                self.unreported += 1;
                if self
                    .reported
                    .is_none_or(|at| now.duration_since(at) >= Duration::from_secs(5))
                {
                    tracing::warn!(error = %format!("{error:#}"), bytes, dropped = self.unreported, "PyroWave frame dropped");
                    self.unreported = 0;
                    self.reported = Some(now);
                }
                None
            }
        }
    }
}
/// Wire rate to send a frame of `packets` within one frame period.
fn demand_bps(
    packets: usize,
    packet_size: usize,
    encrypted: bool,
    ipv6: bool,
    fps_millihz: u32,
) -> u64 {
    let packet_bytes = packet_size + 16 + if encrypted { 32 } else { 0 };
    let wire_bytes = packets * (packet_bytes + butterpollo_core::network_pacing::overhead(ipv6));
    (wire_bytes as u64).saturating_mul(u64::from(fps_millihz)) / 1000 * 8
}
/// Calibration and live traffic share the same one-millisecond wire batches.
fn batch_limit(bps: u64, packet: usize, ipv6: bool) -> usize {
    butterpollo_core::network_pacing::pyrowave_batch_limit(bps, packet, ipv6)
}
struct Slot {
    pending: Mutex<Option<Frame>>,
    changed: Condvar,
    ready: Mutex<Ready>,
    ready_changed: Condvar,
    /// Sent blocks, for the packetizer to fill again.
    pool: BlockPool,
    error: Mutex<Option<String>>,
    stop: AtomicBool,
    stop_signal: Signal,
}
impl Slot {
    /// Hand an idle sender each block of a frame as it is built; `first`
    /// makes the frame from block 0. A failed block (`None`) abandons the
    /// frame, whose index stays used as for a frame that is never sent: one
    /// the sender has not taken is dropped, one in hand is flagged so the
    /// sender stops there. Returns whether the whole frame was handed over.
    fn hand_over(
        &self,
        blocks: impl Iterator<Item = Option<PacketBlock>>,
        first: impl FnOnce(PacketBlock) -> Prepared,
    ) -> bool {
        let mut first = Some(first);
        let mut complete = true;
        for block in blocks {
            let mut ready = self.ready.lock().unwrap();
            match block {
                None => {
                    ready.open = false;
                    for block in ready.blocks.drain(..) {
                        self.pool.give(block);
                    }
                    // The packetizer only streams to an idle sender: a streamed
                    // frame still queued is this one.
                    match ready.frame.take_if(|frame| frame.streamed) {
                        Some(frame) => {
                            for block in frame.blocks {
                                self.pool.give(block);
                            }
                        }
                        None => ready.abandoned = ready.sending,
                    }
                    complete = false;
                }
                // The frame's details travel with block 0.
                Some(block) => match first.take() {
                    Some(first) => {
                        ready.open = true;
                        ready.frame = Some(first(block));
                    }
                    None => ready.blocks.push_back(block),
                },
            }
            drop(ready);
            self.ready_changed.notify_one();
            if !complete {
                return false;
            }
        }
        self.ready.lock().unwrap().open = false;
        self.ready_changed.notify_one();
        true
    }
}
pub struct Sender {
    slot: Arc<Slot>,
    worker: Option<thread::JoinHandle<()>>,
    transport: Option<thread::JoinHandle<()>>,
    session: Arc<Session>,
}
impl Sender {
    pub fn new(
        socket: Arc<ReplySocket>,
        session: Arc<Session>,
        config: Config,
        host: Shared,
        start: Instant,
        stamper: Option<butterpollo_windows::present_timing::Stamper>,
    ) -> Result<Self> {
        let mut sender = Self::packetizer(
            session.clone(),
            config.clone(),
            host.clone(),
            start,
            stamper,
        )?;
        let shared = sender.slot.clone();
        sender.transport = Some(thread::Builder::new().name("pyrowave-send".into()).spawn(
            move || {
                let result = transport::run(&shared, socket, session, config, host, start);
                if let Err(error) = result {
                    *shared.error.lock().unwrap() = Some(format!("{error:#}"));
                }
                shared.stop.store(true, Ordering::Release);
                let _ = shared.stop_signal.set();
                shared.changed.notify_all();
            },
        )?);
        Ok(sender)
    }

    fn packetizer(
        session: Arc<Session>,
        config: Config,
        host: Shared,
        start: Instant,
        stamper: Option<butterpollo_windows::present_timing::Stamper>,
    ) -> Result<Self> {
        let slot = Arc::new(Slot {
            pending: Mutex::new(None),
            changed: Condvar::new(),
            ready: Mutex::new(Ready::default()),
            ready_changed: Condvar::new(),
            pool: BlockPool::default(),
            error: Mutex::new(None),
            stop: AtomicBool::new(false),
            stop_signal: Signal::new()?,
        });
        let shared = slot.clone();
        let current = session.clone();
        let worker = thread::Builder::new().name("pyrowave-packetize".into()).spawn(move || {
            let _priority =
                crate::video_send::send_priority(config.boolean("video_send_mmcss_only", false));
            let mut packetizer = VideoPacketizer {
                sequence: 0, iv_counter: 0, frame: 1,
                packet_size: current.config.packet_size, fec_percent: 0,
                min_fec: current.config.min_fec,
                key: (current.config.encryption & 2 != 0).then_some(current.launch.key),
            };
            let mut controller = DetailFec::new(current.config.fps_millihz());
            let mut stamper = stamper;
            let mut last_stamp = start;
            let mut fec_reported: Option<Instant> = None;
            let requested_fec = config.integer("pyrowave_critical_fec_percentage", 20);
            let early_blocks = config.boolean("video_send_early_blocks", true);
            if requested_fec != requested_fec.clamp(0, 255) {
                current.launch.warnings.set("pyrowave_fec_config", format!("PyroWave critical FEC {requested_fec}% is outside 0-255; using {}%. Correct the critical FEC setting.", requested_fec.clamp(0, 255)));
            }
            let mut dropped = DroppedFrames::default();
            'frames: while !shared.stop.load(Ordering::Acquire) && !current.stopping() && !host.stop.load(Ordering::Acquire) {
                let frame = {
                    let mut pending = shared.pending.lock().unwrap();
                    if pending.is_none() {
                        pending = shared.changed.wait_timeout(pending, Duration::from_millis(20)).unwrap().0;
                    }
                    pending.take()
                };
                let Some(Frame { encoded: frame, peer, claimed, processing_started, latency, polled, claim }) = frame else { continue };
                let captured = frame.presentation.unwrap_or(claimed);
                let stamp = stamper.as_mut().map_or(captured, |stamper| stamper.stamp(captured, &current.output.read().unwrap()))
                    .max(last_stamp + Duration::from_nanos(11_112));
                last_stamp = stamp;
                let timestamp = (stamp.saturating_duration_since(start).as_secs_f64() * 90000.) as u64 as u32;
                let critical_percentage = current.fec_percent().min(requested_fec.clamp(0, 255) as usize);
                let bitrate = current.bitrate.load(Ordering::Relaxed);
                let kbps = if current.config.configured_bitrate_kbps > 0 {
                    (u64::from(current.config.configured_bitrate_kbps) * u64::from(bitrate)
                        / u64::from(current.config.bitrate_kbps.max(1))).min(2_000_000) as u32
                } else { bitrate };
                let now = Instant::now();
                let Some((detail_percentage, wire_budget)) = (if current.config.pyrowave_records && critical_percentage > 0 {
                    // The codec's own records: framing only orders and pads them.
                    dropped.prepared(controller.observe(&frame.bytes, now, kbps), frame.wire_len(), now, &current.launch.warnings)
                } else { Some((0, 0)) }) else { continue };
                let wire_frame = packetizer.frame;
                // Frame Warp tags the frame by the number the client decodes it as.
                current.frame_warp.tag(wire_frame, claim.as_ref().map(|claim| claim.presented));
                // Same boundary as the conventional packetizer. Neither the
                // prepared frame's queue nor packet pacing can change it.
                let packetized = Instant::now();
                let processing = micros(packetized.saturating_duration_since(processing_started));
                // The framed picture is copied from the codec's output straight into the packets.
                let prepared = match frame.payload() {
                    Ok(payload) => packetizer.pyrowave_blocks_in(payload, timestamp, processing, PyrowaveFec {
                        records: current.config.pyrowave_records, critical_percentage, detail_percentage,
                        wire_budget, ipv6: peer.ip().to_canonical().is_ipv6(),
                    }, Some(&shared.pool)),
                    Err(error) => Err(error),
                };
                let wire_len = frame.wire_len();
                let Some((packets, limited, blocks)) = dropped.prepared(prepared, wire_len, now, &current.launch.warnings) else { continue };
                if limited && fec_reported.is_none_or(|at| at.elapsed() >= Duration::from_secs(1)) {
                    fec_reported = Some(Instant::now());
                    current.launch.warnings.event("pyrowave_fec_frame", "PyroWave error correction was reduced or omitted because the frame and its parity cannot fit the supported wire blocks. Recovery protection is reduced; lower bitrate/resolution or use HEVC/AV1 on a lossy link.", butterpollo_core::session::EVENT_PERIOD);
                }
                let demand = demand_bps(packets, current.config.packet_size, current.config.encryption & 2 != 0, peer.ip().to_canonical().is_ipv6(), current.config.fps_millihz());
                let details = |blocks, streamed| Prepared { blocks, peer, claimed, processing, latency: micros(latency),
                    age: micros(claimed.saturating_duration_since(captured)), wire_frame, demand, kbps,
                    claim, polled, packetized, prepared: Instant::now(), timestamp, streamed };
                // An idle sender takes each block as it is built (6772d4019: the first of a
                // 4K frame leaves after 0.2 ms instead of 1.8). A busy one gets the whole frame,
                // which a newer one can still replace.
                if early_blocks && shared.ready.lock().unwrap().idle() {
                    let blocks = blocks.map(|block| dropped.prepared(block, wire_len, Instant::now(), &current.launch.warnings));
                    shared.hand_over(blocks, |block| details(vec![block], true));
                    continue;
                }
                let mut ready = Vec::new();
                for block in blocks {
                    let Some(block) = dropped.prepared(block, wire_len, Instant::now(), &current.launch.warnings) else {
                        for block in ready { shared.pool.give(block); }
                        continue 'frames
                    };
                    ready.push(block);
                }
                if shared.ready.lock().unwrap().replace(details(ready, false), &shared.pool) {
                    current.stats.frames_replaced.fetch_add(1, Ordering::Relaxed);
                }
                shared.ready_changed.notify_one();
            }
            shared.stop.store(true, Ordering::Release);
            let _ = shared.stop_signal.set();
            shared.changed.notify_all();
            shared.ready_changed.notify_all();
        })?;
        Ok(Self {
            slot,
            worker: Some(worker),
            transport: None,
            session,
        })
    }

    pub fn submit(
        &self,
        frames: Vec<Encoded>,
        peer: SocketAddr,
        latency: Duration,
        processing_started: Instant,
        claims: &mut Claims,
    ) -> Result<()> {
        if let Some(error) = self.slot.error.lock().unwrap().as_ref() {
            bail!("video sender stopped: {error}");
        }
        if frames.is_empty() {
            return Ok(());
        }
        let polled = Instant::now();
        let mut pending = self.slot.pending.lock().unwrap();
        for frame in frames {
            let latency = frame.latency.unwrap_or(latency);
            let claimed = polled.checked_sub(latency).unwrap_or(polled);
            let claim = claims.take(frame.presentation);
            if pending
                .replace(Frame {
                    encoded: frame,
                    peer,
                    claimed,
                    latency,
                    // A frame the encoder finished after a later call (two
                    // frames in flight) was claimed before that call's start.
                    processing_started: processing_started.min(claimed),
                    polled,
                    claim,
                })
                .is_some()
            {
                self.session
                    .stats
                    .frames_replaced
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        self.slot.changed.notify_one();
        Ok(())
    }
}
impl Drop for Sender {
    fn drop(&mut self) {
        self.slot.stop.store(true, Ordering::Release);
        let _ = self.slot.stop_signal.set();
        self.slot.changed.notify_all();
        self.slot.ready_changed.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Some(worker) = self.transport.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests;
