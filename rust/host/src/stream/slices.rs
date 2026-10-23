//! Slice delivery (`video_slice_delivery`) on the inline send path: where the
//! units of an AMF frame go, and the state of the frame being streamed.
//!
//! With slice delivery on, AMF returns a P-frame in units. A unit of a frame
//! this path streams is packetized and sent as it arrives; the units of any
//! other frame (one the plan cannot stream, or any frame once the sender has
//! its own thread) are joined into the whole frame the send paths take.

use anyhow::Result;
use butterpollo_core::{
    packet::StreamedFrame,
    performance::Claim,
    stream_policy::{SendDrop, SendLoss},
};
use butterpollo_windows::{
    encoder::{Encoded, Unit},
    net::Batch,
};
use std::{
    net::{SocketAddr, UdpSocket},
    time::Instant,
};

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
                last.then(|| Encoded {
                    bytes: std::mem::take(&mut self.joined),
                    unit: Unit::Whole,
                    ..frame
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
    pub processing: u64,
    pub bps: u64,
    pub sent: Sent,
    /// A unit overflowed the plan, or the frame failed to build: the rest of
    /// its units are dropped.
    pub failed: bool,
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
    socket: &UdpSocket,
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
mod tests {
    use super::*;

    fn unit(unit: Unit, bytes: &[u8]) -> Encoded {
        Encoded {
            bytes: bytes.to_vec(),
            unit,
            idr: false,
            after_invalidation: false,
            frame_cap_bytes: Some(1000),
            latency: None,
            presentation: None,
        }
    }
    fn slice(index: u32, bytes: &[u8]) -> Encoded {
        unit(Unit::Slice { index }, bytes)
    }
    fn last(index: u32, bytes: &[u8]) -> Encoded {
        unit(Unit::Last { index }, bytes)
    }

    #[test]
    fn whole_frames_pass_untouched() {
        let mut route = Route::default();
        let routed = route.route(unit(Unit::Whole, b"frame"), |_| panic!("not a unit"));
        assert_eq!(routed.frame.unwrap().bytes, b"frame");
        assert!(!routed.interrupted && !route.streaming());
    }

    #[test]
    fn a_streamed_frame_passes_every_unit_through() {
        let mut route = Route::default();
        let mut asked = 0;
        for frame in [slice(0, b"a"), slice(1, b"b"), last(2, b"c")] {
            let routed = route.route(frame, |_| {
                asked += 1;
                true
            });
            assert!(routed.frame.is_some() && !routed.interrupted);
        }
        assert_eq!(asked, 1, "the first unit decides");
        assert!(!route.streaming());
    }

    #[test]
    fn a_joined_frame_is_returned_whole_at_its_last_unit() {
        let mut route = Route::default();
        assert!(route.route(slice(0, b"ab"), |_| false).frame.is_none());
        assert!(route.route(slice(1, b"cd"), |_| panic!()).frame.is_none());
        let frame = route.route(last(2, b"e"), |_| panic!()).frame.unwrap();
        assert_eq!((frame.unit, &frame.bytes[..]), (Unit::Whole, &b"abcde"[..]));
        assert_eq!(frame.frame_cap_bytes, Some(1000));
        // The next frame starts empty.
        let frame = route.route(last(0, b"x"), |_| false).frame.unwrap();
        assert_eq!(frame.bytes, b"x");
    }

    #[test]
    fn a_new_frame_interrupts_a_streamed_one_that_never_finished() {
        let mut route = Route::default();
        route.route(slice(0, b"a"), |_| true);
        assert!(route.streaming());
        let routed = route.route(slice(0, b"b"), |_| true);
        assert!(routed.interrupted && routed.frame.is_some());
        let routed = route.route(unit(Unit::Whole, b"w"), |_| true);
        assert!(routed.interrupted, "a whole frame ends it too");
        assert!(!route.streaming());
    }

    #[test]
    fn units_of_a_frame_that_was_not_seen_begin_are_dropped() {
        let mut route = Route::default();
        assert!(route.route(slice(2, b"a"), |_| true).frame.is_none());
        assert!(route.route(last(3, b"b"), |_| true).frame.is_none());
        assert!(route.route(last(0, b"c"), |_| true).frame.is_some());
    }

    fn sockets() -> (UdpSocket, UdpSocket) {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(std::time::Duration::from_millis(200)))
            .unwrap();
        (UdpSocket::bind("127.0.0.1:0").unwrap(), receiver)
    }
    fn received(socket: &UdpSocket) -> usize {
        let mut buffer = [0u8; 64];
        std::iter::from_fn(|| socket.recv(&mut buffer).ok()).count()
    }

    #[test]
    fn send_lossy_loses_what_the_generator_picks_and_counts_them_refused() {
        let (sender, receiver) = sockets();
        let peer = receiver.local_addr().unwrap();
        let packets: Vec<Vec<u8>> = (0..40).map(|_| vec![7u8; 32]).collect();
        let mut drop = SendDrop::parse("50:9").unwrap();
        let mut expected = drop.clone();
        let lost = packets.iter().filter(|_| expected.drops()).count();
        assert!(lost > 5 && lost < 35, "a fair share is lost: {lost}");
        let mut batch = Batch::default();
        let bytes = send_lossy(&mut batch, &sender, &packets, peer, &mut drop).unwrap();
        assert_eq!(batch.dropped as usize, lost);
        assert_eq!(bytes, (40 - lost) * 32);
        assert_eq!(received(&receiver), 40 - lost);
    }

    #[test]
    fn send_lossy_in_the_air_counts_every_datagram_sent() {
        let (sender, receiver) = sockets();
        let peer = receiver.local_addr().unwrap();
        let packets: Vec<Vec<u8>> = (0..10).map(|_| vec![1u8; 32]).collect();
        let mut drop = SendDrop::parse("100:1:air").unwrap();
        let mut batch = Batch::default();
        let bytes = send_lossy(&mut batch, &sender, &packets, peer, &mut drop).unwrap();
        assert_eq!((bytes, batch.dropped), (320, 0));
        assert_eq!(received(&receiver), 0);
    }

    #[test]
    fn send_lossy_with_no_loss_sends_everything() {
        let (sender, receiver) = sockets();
        let peer = receiver.local_addr().unwrap();
        let packets: Vec<Vec<u8>> = (0..10).map(|_| vec![1u8; 32]).collect();
        let mut drop = SendDrop::parse("0.0001:3").unwrap();
        let mut batch = Batch::default();
        let bytes = send_lossy(&mut batch, &sender, &packets, peer, &mut drop).unwrap();
        assert_eq!((bytes, batch.dropped), (320, 0));
        assert_eq!(received(&receiver), 10);
    }
}
