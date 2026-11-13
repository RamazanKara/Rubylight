use super::*;
use std::net::UdpSocket;

fn unit(unit: Unit, bytes: &[u8]) -> Encoded {
    Encoded {
        framing: None,
        bytes: bytes.to_vec(),
        unit,
        idr: false,
        after_invalidation: false,
        frame_cap_bytes: Some(1000),
        reencoded: false,
        frame_cap_target: None,
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

const SLICE: usize = 1376;

fn packetizer() -> VideoPacketizer {
    VideoPacketizer {
        sequence: 100,
        iv_counter: 5,
        frame: 7,
        packet_size: 1392,
        fec_percent: 20,
        min_fec: 2,
        key: None,
    }
}
fn streaming(packetizer: &mut VideoPacketizer, spare: Vec<u8>, cap: usize) -> Streaming {
    let now = Instant::now();
    Streaming {
        frame: packetizer.streamed_in(spare, 9000, 500, 4, 4, cap).unwrap(),
        wire_frame: packetizer.frame,
        timestamp: 9000,
        claim: None,
        claimed: now,
        age: 0,
        started: now,
        packetize_started: now,
        first_polled: now,
        last_polled: now,
        payload_bytes: 0,
        processing: 0,
        bps: 0,
        sent: Sent::new(0),
        failed: false,
    }
}
fn push(
    streaming: &mut Streaming,
    packetizer: &mut VideoPacketizer,
    bytes: usize,
    last: bool,
) -> Result<usize> {
    let mut shards = Vec::new();
    let mut built = 0;
    streaming.unit(Instant::now(), bytes);
    streaming.frame.push(
        packetizer,
        &vec![0x55; bytes],
        last,
        &mut shards,
        |packets, _| {
            built += packets.len();
            Ok(())
        },
    )?;
    Ok(built)
}

#[test]
fn an_interrupted_frame_that_built_no_block_consumes_its_index() {
    let mut packetizer = packetizer();
    let mut open = Some(streaming(&mut packetizer, Vec::new(), 100 * SLICE));
    // Too little for a block: nothing left the host.
    assert_eq!(
        push(open.as_mut().unwrap(), &mut packetizer, SLICE / 2, false).unwrap(),
        0
    );
    let mut spare = Vec::new();
    let next = drop_frame(&mut open, &mut packetizer, &mut spare);
    assert!(open.is_none());
    assert_eq!((next, packetizer.frame), (8, 8), "the next frame takes 8");
    assert_eq!((packetizer.sequence, packetizer.iv_counter), (100, 5));
    assert!(spare.capacity() >= SLICE / 2, "the buffer comes back");
    // Nothing open: the number is where it was.
    assert_eq!(drop_frame(&mut open, &mut packetizer, &mut spare), 8);
}

#[test]
fn an_interrupted_frame_with_a_block_out_does_not_consume_a_second_index() {
    let mut packetizer = packetizer();
    let mut open = Some(streaming(&mut packetizer, Vec::new(), 100 * SLICE));
    assert!(push(open.as_mut().unwrap(), &mut packetizer, 30 * SLICE, false).unwrap() > 0);
    assert_eq!(packetizer.frame, 8, "block 0 took index 7");
    let next = drop_frame(&mut open, &mut packetizer, &mut Vec::new());
    assert_eq!((next, packetizer.frame), (8, 8));
}

#[test]
fn a_frame_that_could_not_start_consumes_its_index() {
    let mut packetizer = packetizer();
    // A cap past what four blocks hold cannot be streamed.
    assert!(packetizer.streamed(0, 0, 4, 4, 5000 * SLICE).is_err());
    assert_eq!(packetizer.frame, 7);
    assert_eq!(refuse_frame(&mut packetizer), 8);
    assert_eq!(packetizer.frame, 8);
}

#[test]
fn the_next_frame_after_a_dropped_one_shows_the_client_a_gap() {
    let mut packetizer = packetizer();
    let mut open = Some(streaming(&mut packetizer, Vec::new(), 100 * SLICE));
    push(open.as_mut().unwrap(), &mut packetizer, SLICE / 2, false).unwrap();
    drop_frame(&mut open, &mut packetizer, &mut Vec::new());
    let mut frame = packetizer.streamed(9100, 500, 2, 2, 100 * SLICE).unwrap();
    let mut index = None;
    frame
        .push(
            &mut packetizer,
            &vec![1; 3 * SLICE],
            true,
            &mut Vec::new(),
            |packets, _| {
                index.get_or_insert(u32::from_le_bytes(packets[0][20..24].try_into().unwrap()));
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(index, Some(8), "frame 7 is missing on the wire");
}

#[test]
fn an_overflow_turns_delivery_off_and_the_encoder_is_told_to_join() {
    let stats = Stats::default();
    let delivery = Delivery::new(4);
    assert!(delivery.streams() && !delivery.joins());
    let mut packetizer = packetizer();
    let mut open = streaming(&mut packetizer, Vec::new(), 40 * SLICE);
    push(&mut open, &mut packetizer, 30 * SLICE, false).unwrap();
    let error = push(&mut open, &mut packetizer, 30 * SLICE, false).unwrap_err();
    let overflow = note_failure(&delivery, &stats, &error).unwrap();
    assert_eq!(
        overflow,
        SliceOverflow {
            cap: 40 * SLICE,
            bytes: 60 * SLICE
        }
    );
    assert_eq!(stats.slice_delivery_overflows.load(Ordering::Relaxed), 1);
    assert!(!delivery.streams() && delivery.joins());
    // Other failures are not overflows and leave it on.
    let delivery = Delivery::new(4);
    assert!(note_failure(&delivery, &stats, &anyhow::anyhow!("socket gone")).is_none());
    assert!(delivery.streams() && !delivery.joins());
    assert_eq!(stats.slice_delivery_overflows.load(Ordering::Relaxed), 1);
}

#[test]
fn delivery_is_off_when_not_asked_for_and_follows_the_sender_threads_overflows() {
    let off = Delivery::new(0);
    assert!(!off.streams() && !off.joins());
    // The sender thread counts an overflow; the stream thread turns off.
    let stats = Stats::default();
    let delivery = Delivery::new(2);
    delivery.sync(&stats);
    assert!(delivery.streams() && !delivery.joins());
    let error: anyhow::Error = SliceOverflow { cap: 1, bytes: 2 }.into();
    assert!(count_overflow(&stats, &error).is_some());
    assert!(count_overflow(&stats, &anyhow::anyhow!("socket gone")).is_none());
    delivery.sync(&stats);
    assert!(!delivery.streams() && delivery.joins());
    assert_eq!(stats.slice_delivery_overflows.load(Ordering::Relaxed), 1);
}

#[test]
fn delivery_follows_the_encoder_across_mode_switches_and_keeps_an_overflow() {
    // 4K to 1440p: the rebuilt encoder returns whole frames.
    let delivery = Delivery::new(4);
    assert!(delivery.follow(0));
    assert_eq!(delivery.blocks(), 0);
    assert!(!delivery.streams() && !delivery.joins());
    // And back to 4K: its frames stream in four blocks again.
    assert!(delivery.follow(4));
    assert!(!delivery.follow(4));
    assert!(delivery.streams() && !delivery.joins());
    // 1440p to 4K turns it on for a stream that began without it.
    let delivery = Delivery::new(0);
    assert!(delivery.follow(4));
    assert!(delivery.streams());
    // After an overflow, a rebuilt sliced encoder is told to join.
    delivery.turn_off();
    delivery.follow(0);
    assert!(!delivery.streams() && !delivery.joins());
    delivery.follow(4);
    assert!(!delivery.streams() && delivery.joins());
}

#[test]
fn the_stream_does_not_hand_off_while_a_streamed_frame_is_open() {
    let mut route = Route::default();
    let mut packetizer = packetizer();
    assert!(!open(&route, &None));
    route.route(slice(0, b"a"), |_| true);
    assert!(open(&route, &None), "the route has an open frame");
    route.route(last(1, b"b"), |_| true);
    assert!(!open(&route, &None));
    let held = Some(streaming(&mut packetizer, Vec::new(), 100 * SLICE));
    assert!(open(&route, &held), "so does a streamed frame being sent");
    // A frame being joined sent nothing, so a hand-off may happen.
    route.route(slice(0, b"a"), |_| false);
    assert!(!open(&route, &None));
}

#[test]
fn stamps_of_a_streamed_frame_keep_the_stage_order() {
    let base = Instant::now();
    let at = |us: u64| base + std::time::Duration::from_micros(us);
    let claim = Claim {
        key: at(0),
        presented: at(0),
        acquired: at(100),
        published: at(150),
        claimed: at(200),
        submitted: at(700),
    };
    // The first unit came in at 1.0 ms and the last, much later, at 7.9 ms; the
    // first block left at 1.5 ms and the last packet at 8.0 ms.
    let stamps = stamps(
        &claim,
        (at(1000), at(7900)),
        2,
        at(1100),
        Some(at(1500)),
        at(8000),
    );
    assert_eq!(
        stamps.stages(),
        [100, 50, 50, 500, 7200, 0, 0, 0, 100],
        "encode ends at the last unit; what left earlier is not counted again"
    );
    assert_eq!(stamps.total(), 8000);
    assert_eq!(stamps.overlap(), Some(6400));
    let streamed = stamps.streamed.unwrap();
    assert_eq!((streamed.first_unit, streamed.early_blocks), (at(1000), 2));
}

#[test]
fn a_streaming_frame_counts_the_payload_of_every_unit() {
    let mut packetizer = packetizer();
    let mut open = streaming(&mut packetizer, Vec::new(), 100 * SLICE);
    let later = open.first_polled + std::time::Duration::from_millis(3);
    open.unit(open.first_polled, 100);
    open.unit(later, 250);
    assert_eq!((open.payload_bytes, open.last_polled), (350, later));
}

fn sockets() -> (ReplySocket, UdpSocket) {
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    receiver
        .set_read_timeout(Some(std::time::Duration::from_millis(200)))
        .unwrap();
    (ReplySocket::bind("127.0.0.1:0").unwrap(), receiver)
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
