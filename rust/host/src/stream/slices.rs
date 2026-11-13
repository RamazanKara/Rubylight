//! Slice delivery (`video_slice_delivery`) on the inline send path: where the
//! units of an encoder's frame go, and the state of the frame being streamed.
//!
//! With slice delivery on, AMF (slice output mode) and NVENC (subframe
//! readback) return a P-frame in units. A unit of a frame
//! this path streams is packetized and sent as it arrives; the units of any
//! other frame (one the plan cannot stream, or any frame once the sender has
//! its own thread) are joined into the whole frame the send paths take.

use crate::state::Session;
use anyhow::Result;
use butterpollo_core::{
    packet::{SliceOverflow, StreamedFrame, VideoPacketizer},
    performance::{Claim, Stamps},
    session::Stats,
    stream_policy::{SendDrop, SendLoss},
};
use butterpollo_windows::{
    encoder::{Encoded, Unit},
    net::{Batch, ReplySocket},
};
use std::{cell::Cell, net::SocketAddr, sync::atomic::Ordering, time::Instant};

/// Whether slice delivery is still on for the stream. A frame past its byte
/// cap turns it off for the rest of the stream, on whichever thread sends
/// it; the encoder is then told to return whole frames.
pub struct Delivery {
    blocks: Cell<usize>,
    off: Cell<bool>,
}

impl Delivery {
    /// `blocks` is the encoder's slice delivery (0 when off).
    pub fn new(blocks: usize) -> Self {
        Self {
            blocks: Cell::new(blocks),
            off: Cell::new(false),
        }
    }
    pub fn blocks(&self) -> usize {
        self.blocks.get()
    }
    /// Take the block count of a rebuilt encoder: `auto` decides from the
    /// picture size, so a mode switch can turn slices on or off. An overflow
    /// keeps delivery off for the rest of the stream. Returns whether the
    /// count changed.
    pub fn follow(&self, blocks: usize) -> bool {
        self.blocks.replace(blocks) != blocks
    }
    pub fn turn_off(&self) {
        self.off.set(true);
    }
    /// Take in an overflow the sender thread counted, which it cannot turn
    /// off from there.
    pub fn sync(&self, stats: &Stats) {
        if stats.slice_delivery_overflows.load(Ordering::Relaxed) != 0 {
            self.turn_off();
        }
    }
    /// A new P-frame may be streamed.
    pub fn streams(&self) -> bool {
        self.blocks.get() > 0 && !self.off.get()
    }
    /// The encoder should return whole frames from now on.
    pub fn joins(&self) -> bool {
        self.blocks.get() > 0 && self.off.get()
    }
}

/// A streamed frame failed to build or to be sent as it should. A frame past
/// the byte cap its blocks were planned for turns slice delivery off and is
/// counted; the overflow is returned for the log.
pub fn note_failure(
    delivery: &Delivery,
    stats: &Stats,
    error: &anyhow::Error,
) -> Option<SliceOverflow> {
    let overflow = count_overflow(stats, error)?;
    delivery.turn_off();
    Some(overflow)
}

/// Count a frame past its byte cap, for either send path; the stream thread
/// turns delivery off when it sees the count ([`Delivery::sync`]).
pub fn count_overflow(stats: &Stats, error: &anyhow::Error) -> Option<SliceOverflow> {
    let overflow = *error.downcast_ref::<SliceOverflow>()?;
    stats
        .slice_delivery_overflows
        .fetch_add(1, Ordering::Relaxed);
    Some(overflow)
}

/// Say, loudly, that a frame passed the cap its blocks were planned for.
pub fn report_overflow(session: &Session, frame: u32, overflow: SliceOverflow) {
    tracing::warn!(
        frame,
        cap_bytes = overflow.cap,
        frame_bytes = overflow.bytes,
        "a streamed video frame passed the byte cap its blocks were planned for; slice delivery is off for this stream"
    );
    session.launch.warnings.event("slice_delivery_overflow", format!("A video frame reached {} bytes, past the {} byte cap its blocks were planned for. Slice delivery is off for the rest of this stream and a recovery frame follows. Report this with the host log.", overflow.bytes, overflow.cap), butterpollo_core::session::EVENT_PERIOD);
}

/// A streamed frame is open (units sent, the last one not seen), so the
/// stream must not move to another sender in the middle of it.
pub fn open(route: &Route, streaming: &Option<Streaming>) -> bool {
    streaming.is_some() || route.streaming()
}

/// Forget the streamed frame the stream will not finish. Its index is
/// consumed if no block left, so the client sees the gap. The payload buffer
/// goes back to `spare`. Returns the next frame number to expect on the wire.
pub fn drop_frame(
    streaming: &mut Option<Streaming>,
    packetizer: &mut VideoPacketizer,
    spare: &mut Vec<u8>,
) -> u32 {
    if let Some(mut open) = streaming.take() {
        open.frame.abandon(packetizer);
        *spare = open.frame.into_buffer();
    }
    packetizer.frame
}

/// A frame that could not even be started is dropped before any packet: its
/// index is consumed, as for any other dropped frame. Returns the next frame
/// number to expect on the wire.
pub fn refuse_frame(packetizer: &mut VideoPacketizer) -> u32 {
    packetizer.skip_frame();
    packetizer.frame
}

/// The points of a streamed frame. `encoded` is when its last unit was in
/// hand, as for any frame, so `encode` is the encoder's whole time. The first
/// blocks left earlier: `first_sent` precedes `encoded` by the overlap, and the
/// stages that would run backwards count as reached with it. The first unit
/// and the blocks sent before the last go into the record.
pub fn stamps(
    claim: &Claim,
    (first_polled, last_polled): (Instant, Instant),
    early_blocks: usize,
    packetize_started: Instant,
    first_sent: Option<Instant>,
    finished: Instant,
) -> Stamps {
    claim
        .sent(
            last_polled,
            packetize_started,
            first_sent.unwrap_or(finished),
            finished,
        )
        .with_streamed(first_polled, early_blocks)
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Mode {
    /// Between frames.
    #[default]
    Idle,
    /// The units of the open frame are streamed.
    Stream,
    /// The units of the open frame are joined.
    Join,
}

/// What [`Route::route`] did with one encoder output.
pub struct Routed {
    /// A frame for the send path: a whole one, or a unit to stream.
    pub frame: Option<Encoded>,
    /// A streamed frame was still open when a new frame began: its last unit
    /// never came and the caller must drop what it kept of it.
    pub interrupted: bool,
}

#[derive(Default)]
pub struct Route {
    mode: Mode,
    joined: Vec<u8>,
    /// The last joined frame's length, to reserve the next buffer.
    last_joined_len: usize,
}

impl Route {
    /// Take the next output of the encoder. `stream` runs on a frame's first
    /// unit and says whether the send path streams it.
    pub fn route(&mut self, frame: Encoded, stream: impl FnOnce(&Encoded) -> bool) -> Routed {
        let (index, last) = match frame.unit {
            Unit::Whole => {
                let interrupted = self.mode == Mode::Stream;
                self.mode = Mode::Idle;
                self.joined.clear();
                return Routed {
                    frame: Some(frame),
                    interrupted,
                };
            }
            Unit::Slice { index } => (index, false),
            Unit::Last { index } => (index, true),
        };
        let mut interrupted = false;
        if index == 0 {
            interrupted = self.mode == Mode::Stream;
            self.joined.clear();
            self.mode = if stream(&frame) {
                Mode::Stream
            } else {
                Mode::Join
            };
            if self.mode == Mode::Join {
                self.joined.reserve(self.last_joined_len);
            }
        }
        let mode = self.mode;
        if last {
            self.mode = Mode::Idle;
        }
        let frame = match mode {
            // A unit of a frame whose start was not seen cannot be sent.
            Mode::Idle => None,
            Mode::Stream => Some(frame),
            Mode::Join => {
                self.joined.extend_from_slice(&frame.bytes);
                last.then(|| {
                    let bytes = std::mem::take(&mut self.joined);
                    self.last_joined_len = bytes.len();
                    Encoded {
                        framing: None,
                        bytes,
                        unit: Unit::Whole,
                        ..frame
                    }
                })
            }
        };
        Routed { frame, interrupted }
    }
    /// A streamed frame is open.
    pub fn streaming(&self) -> bool {
        self.mode == Mode::Stream
    }
}

/// What the stream keeps of a frame while its units arrive.
pub struct Streaming {
    pub frame: StreamedFrame,
    pub wire_frame: u32,
    pub timestamp: u32,
    pub claim: Option<Claim>,
    /// When the encoder had begun the frame, as the sender's timings count it.
    pub claimed: Instant,
    pub age: u64,
    pub started: Instant,
    pub packetize_started: Instant,
    /// When the encoder's first unit and its latest were in hand.
    pub first_polled: Instant,
    pub last_polled: Instant,
    /// The frame's payload so far, over all units, even those not sent.
    pub payload_bytes: usize,
    pub processing: u64,
    pub bps: u64,
    pub sent: Sent,
    /// A unit overflowed the plan, or the frame failed to build: the rest of
    /// its units are dropped.
    pub failed: bool,
}

impl Streaming {
    /// A unit of `bytes` came in at `polled`.
    pub fn unit(&mut self, polled: Instant, bytes: usize) {
        self.last_polled = polled;
        self.payload_bytes += bytes;
    }
}

/// The socket side of a streamed frame.
pub struct Sent {
    pub lost: SendLoss,
    pub first_sent: Option<Instant>,
    pub packets: usize,
    /// The socket refused more than FEC repairs: no more of the frame is sent.
    pub abandoned: bool,
    /// The batch's refused-datagram count when the frame began.
    pub dropped_at_start: u64,
}

impl Sent {
    pub fn new(dropped_at_start: u64) -> Self {
        Self {
            dropped_at_start,
            lost: SendLoss::streamed(),
            first_sent: None,
            packets: 0,
            abandoned: false,
        }
    }
}

/// Send `packets` as `Batch::send` does, except that those `drop` picks are
/// lost (`BUTTERPOLLO_TEST_SEND_LOSS`): refused as by a full socket, or with
/// `air` counted sent and lost after the socket. Returns the bytes counted sent.
pub fn send_lossy(
    batch: &mut Batch,
    socket: &ReplySocket,
    packets: &[Vec<u8>],
    peer: SocketAddr,
    drop: &mut SendDrop,
) -> Result<usize> {
    let mut bytes = 0;
    let mut run = 0;
    for (index, packet) in packets.iter().enumerate() {
        if !drop.drops() {
            continue;
        }
        bytes += batch.send(socket, &packets[run..index], peer)?;
        run = index + 1;
        if drop.in_air() {
            bytes += packet.len();
        } else {
            batch.refuse(1);
        }
    }
    Ok(bytes + batch.send(socket, &packets[run..], peer)?)
}

#[cfg(test)]
mod tests;
