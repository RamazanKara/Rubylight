//! Streamed P-frames on the packetizer thread (`video_slice_delivery`).
//!
//! The units of an AMF frame reach this thread one by one, through the same
//! pending slot as whole frames. Each FEC block the frame completes becomes a
//! [`Prepared`] part for the paced sender, so its first blocks are on the wire
//! while the encoder still produces the rest.
use super::*;
use crate::stream::slices;
use butterpollo_core::packet::StreamedFrame;
use butterpollo_windows::encoder::Unit;

/// The first unit of a frame, which a streamed frame is built from.
pub(super) fn begins(unit: Unit) -> bool {
    matches!(unit, Unit::Slice { index: 0 } | Unit::Last { index: 0 })
}

pub(super) fn is_last(unit: Unit) -> bool {
    matches!(unit, Unit::Last { .. })
}

/// What the first unit of a frame decides for all of its parts.
pub(super) struct Start {
    pub peer: SocketAddr,
    pub claimed: Instant,
    pub polled: Instant,
    pub packetized: Instant,
    pub processing: u64,
    pub age: u64,
    pub claim: Option<Claim>,
    pub timestamp: u32,
    pub cap: Option<u64>,
}

/// The streamed frame being built, with what its parts share.
struct Open {
    frame: StreamedFrame,
    start: Start,
    /// The packetizer as it was before block 0, which the pace is chosen from.
    wire: VideoPacketizer,
    wire_frame: u32,
    payload_bytes: usize,
    first_polled: Instant,
    last_polled: Instant,
    /// A unit overflowed the plan or the frame failed to build: the rest of
    /// its units are dropped.
    failed: bool,
}

pub(super) struct Streams {
    blocks: usize,
    slices: usize,
    open: Option<Open>,
    /// The payload buffer, kept from frame to frame.
    spare: Vec<u8>,
    shards: PacketBlock,
}

impl Streams {
    pub fn new(blocks: usize, slices: usize) -> Self {
        Self {
            blocks,
            slices: blocks.max(slices),
            open: None,
            spare: Vec::new(),
            shards: PacketBlock::new(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// The frame number to expect next on the wire: a frame takes its number
    /// when its first block is built, so an open one that has built none
    /// still owes it.
    pub fn next_wire_frame(&self, packetizer: &VideoPacketizer) -> u32 {
        match &self.open {
            Some(open) if packetizer.frame == open.wire_frame => open.wire_frame.wrapping_add(1),
            _ => packetizer.frame,
        }
    }

    /// A new frame began while one was open: its last unit never came. The
    /// index it owes is consumed, so the client sees the gap, and a recovery
    /// frame is asked for.
    pub fn interrupted(&mut self, s: &Session, packetizer: &mut VideoPacketizer) {
        let Some(mut open) = self.open.take() else {
            return;
        };
        open.frame.abandon(packetizer);
        self.spare = open.frame.into_buffer();
        // A frame that failed has asked for its recovery frame already.
        if !open.failed {
            s.launch.warnings.event(
                "network_frame",
                "A streamed video frame lost its last slice; requesting a recovery frame.",
                butterpollo_core::session::EVENT_PERIOD,
            );
            s.request_host_keyframe();
        }
    }

    /// Begin a streamed frame on its first unit. When it cannot be planned its
    /// index is consumed and its units are dropped.
    pub fn begin(&mut self, s: &Session, packetizer: &mut VideoPacketizer, start: Start) {
        let wire = packetizer.clone();
        let cap = start.cap.unwrap_or(0) as usize;
        match packetizer.streamed_in(
            std::mem::take(&mut self.spare),
            start.timestamp,
            start.processing,
            self.blocks,
            self.slices,
            cap,
        ) {
            Ok(frame) => {
                self.open = Some(Open {
                    frame,
                    wire_frame: wire.frame,
                    wire,
                    payload_bytes: 0,
                    first_polled: start.polled,
                    last_polled: start.polled,
                    failed: false,
                    start,
                });
            }
            Err(error) => {
                // As an overflow: the plan the stream thread made does not hold.
                s.stats
                    .slice_delivery_overflows
                    .fetch_add(1, Ordering::Relaxed);
                s.launch.warnings.event(
                    "network_frame",
                    format!(
                        "Encoded video frame dropped ({error:#}); requesting a recovery frame."
                    ),
                    butterpollo_core::session::EVENT_PERIOD,
                );
                s.request_host_keyframe();
                packetizer.skip_frame();
            }
        }
    }

    /// Take the next unit of the open frame; every block it completes goes to
    /// the paced sender. Units of a frame that is not open are dropped.
    #[allow(clippy::too_many_arguments)]
    pub fn unit(
        &mut self,
        shared: &Slot,
        s: &Session,
        packetizer: &mut VideoPacketizer,
        bytes: &[u8],
        last: bool,
        polled: Instant,
        latency: Duration,
        fec_reported: &mut Option<Instant>,
    ) -> Result<()> {
        let Some(open) = self.open.as_mut() else {
            return Ok(());
        };
        open.payload_bytes += bytes.len();
        open.last_polled = polled;
        if !open.failed {
            let payload_bytes = open.payload_bytes;
            let (start, wire, wire_frame) = (&open.start, &open.wire, open.wire_frame);
            let mut fatal = None;
            let pushed = open.frame.push(
                packetizer,
                bytes,
                last,
                &mut self.shards,
                |packets, block| {
                    // The shard buffers change hands; the ones that come back are
                    // refilled by the next block.
                    let mut parts = shared
                        .state
                        .lock()
                        .unwrap()
                        .recycled
                        .pop()
                        .unwrap_or_default();
                    parts.truncate(1);
                    if parts.is_empty() {
                        parts.push(Vec::new());
                    }
                    let out = &mut parts[0];
                    out.resize_with(packets.len(), Vec::new);
                    for (to, from) in out.iter_mut().zip(packets.iter_mut()) {
                        std::mem::swap(to, from);
                    }
                    let prepared = Prepared {
                        blocks: parts,
                        counts: vec![packets.len()],
                        layout: vec![(block.data, block.parity)],
                        packetizer: wire.clone(),
                        wire_frame,
                        idr: false,
                        after_invalidation: false,
                        frame_cap_bytes: start.cap,
                        payload_bytes,
                        peer: start.peer,
                        claimed: start.claimed,
                        polled: start.polled,
                        packetized: start.packetized,
                        prepared: Instant::now(),
                        latency: micros(latency),
                        processing: start.processing,
                        age: start.age,
                        claim: start.claim,
                        timestamp: start.timestamp,
                        stream: Some(Part {
                            first: block.index == 0,
                            last: block.is_last(),
                        }),
                    };
                    if let Err(error) = shared.enqueue(prepared) {
                        fatal = Some(error);
                    }
                    Ok(())
                },
            );
            if let Some(error) = fatal {
                return Err(error);
            }
            if let Err(error) = pushed {
                open.failed = true;
                if let Some(overflow) = slices::count_overflow(&s.stats, &error) {
                    slices::report_overflow(s, wire_frame, overflow);
                } else {
                    s.launch.warnings.event("network_frame", format!("Encoded video frame dropped ({error:#}); requesting a recovery frame. Lower bitrate or resolution to stay within Moonlight's packet limit."), butterpollo_core::session::EVENT_PERIOD);
                }
                s.request_host_keyframe();
            }
        }
        if !last {
            return Ok(());
        }
        let Some(open) = self.open.take() else {
            return Ok(());
        };
        if !open.failed {
            // As the whole-frame path judges it, from the frame's size.
            if (open.frame.fec_limited() || packetizer.fec_limited(open.payload_bytes))
                && fec_reported.is_none_or(|at| at.elapsed() >= Duration::from_secs(1))
            {
                *fec_reported = Some(Instant::now());
                s.launch.warnings.event("network_fec", "FEC was reduced or omitted for large video frames because they exceed Moonlight's four-block limit. Packet loss in those frames is harder to recover; lower bitrate or resolution to keep full FEC protection.", butterpollo_core::session::EVENT_PERIOD);
            }
            tracing::trace!(target: "pacing", stream_id = %s.launch.id, frame = open.wire_frame,
                units = open.frame.units(), early_blocks = open.frame.early_blocks(),
                encode_span_us = micros(open.last_polled.saturating_duration_since(open.first_polled)),
                "streamed picture");
        }
        self.spare = open.frame.into_buffer();
        Ok(())
    }
}
