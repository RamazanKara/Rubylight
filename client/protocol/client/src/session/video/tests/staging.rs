//! Early-chunk staging, batch receive ordering, loss recovery and mid-frame refresh tests.

use super::*;

#[test]
fn the_decoder_thread_gets_every_chunk_ahead_of_its_frame() {
    let shared = Arc::new(VideoShared::new(records(true)));
    let (tx, rx) = mpsc::channel();
    let running = Running::start(&shared, Recorder::new(tx), rx);
    let mut host = Blocks::new();
    for n in 0..6u8 {
        feed(&shared, &flat(host.frame(&[12, 9, 5], n)));
    }
    let seen = running.finish();
    let frames = frames_seen(&seen);
    assert_eq!(frames.len(), 6, "{seen:?}");
    assert!(frames.iter().all(|(_, whole)| *whole), "{seen:?}");
    // Parity packets are no chunks: 26 data packets a frame.
    let chunks = seen
        .iter()
        .filter(|s| matches!(s, Seen::Chunk { .. }))
        .count();
    assert_eq!(chunks, 6 * 26);
    assert_eq!(shared.counters.chunks_dropped.load(Ordering::Relaxed), 0);
}

#[test]
fn the_decoder_thread_gets_every_chunk_ahead_of_its_frame_from_receive_calls() {
    let shared = Arc::new(VideoShared::new(records(true)));
    let (tx, rx) = mpsc::channel();
    let running = Running::start(&shared, Recorder::new(tx), rx);
    let mut host = Blocks::new();
    let packets: Vec<Vec<u8>> = (0..6u8)
        .flat_map(|n| flat(host.frame(&[12, 9, 5], n)))
        .collect();
    let mut staged = StagedChunks::default();
    // Calls of 7 end inside frames and among their parity.
    feed_calls(&shared, &mut staged, &packets, 7);
    let seen = running.finish();
    let frames = frames_seen(&seen);
    assert_eq!(frames.len(), 6, "{seen:?}");
    assert!(frames.iter().all(|(_, whole)| *whole), "{seen:?}");
    assert_eq!(chunks_seen(&seen), 6 * 26);
    assert_eq!(shared.counters.chunks_dropped.load(Ordering::Relaxed), 0);
}

/// A queued item as a value.
#[derive(Debug, PartialEq)]
enum Queued {
    Chunk { first: bool, flags: u32, data: Vec<u8> },
    Frame { number: u32 },
}

/// Takes everything queued, in order.
fn drain(queue: &FrameQueue) -> Vec<Queued> {
    let mut out = Vec::new();
    while !lock(&queue.state).items.is_empty() {
        out.push(match next_item(queue) {
            Item::Chunk(Chunk { first, flags, data }) => Queued::Chunk { first, flags, data },
            Item::Frame(frame) => Queued::Frame {
                number: frame.number,
            },
        });
    }
    out
}

fn wakes(queue: &FrameQueue) -> u64 {
    queue.wakes.load(Ordering::Relaxed)
}

#[test]
fn a_receive_call_wakes_a_decoder_that_keeps_up_once_for_its_chunks() {
    // A block of 40 data packets: a call of 32 is all chunks of the frame in progress.
    let packets = flat(Blocks::new().frame(&[40], 1));
    let call = &packets[..net::RECEIVE_BATCH];
    // A decoder that keeps up has taken everything queued before each datagram arrives.
    let take_all = |queue: &FrameQueue| {
        while !lock(&queue.state).items.is_empty() {
            next_item(queue);
        }
    };

    // Queued one at a time, each chunk finds the queue empty and wakes the decoder.
    let shared = VideoShared::new(records(true));
    for packet in call {
        take_all(&shared.queue);
        feed(&shared, std::slice::from_ref(packet));
    }
    assert_eq!(wakes(&shared.queue), net::RECEIVE_BATCH as u64);

    // Staged for the call, they wake it once, all queued by the end of the call.
    let shared = VideoShared::new(records(true));
    let mut staged = StagedChunks::default();
    let mut signals = Vec::new();
    receive_batch(
        &shared,
        None,
        &mut signals,
        call.len(),
        |i| {
            take_all(&shared.queue);
            call[i].as_slice()
        },
        &mut None,
        &mut staged,
        |signals| signals.clear(),
    );
    assert_eq!(wakes(&shared.queue), 1);
    assert_eq!(staged.len(), 0, "nothing waits past the call");
    let queued = drain(&shared.queue);
    assert_eq!(queued.len(), net::RECEIVE_BATCH, "{queued:?}");
    assert!(matches!(queued[0], Queued::Chunk { first: true, .. }));
    assert!(queued[1..]
        .iter()
        .all(|q| matches!(q, Queued::Chunk { first: false, .. })));
}

#[test]
fn a_frame_completed_mid_call_is_queued_and_wakes_the_decoder_with_its_datagram() {
    let shared = VideoShared::new(records(true));
    let mut host = Blocks::new();
    // Blocks of 8 and 6 data packets with 2 parity each.
    let one = flat(host.frame(&[8, 6], 1));
    let two = flat(host.frame(&[8, 6], 2));
    let mut staged = StagedChunks::default();
    // The first call ends inside frame one; the second completes it with its sixth
    // datagram (the last data packet), then carries the parity and frame two's first block.
    feed_calls(&shared, &mut staged, &one[..10], 10);
    assert_eq!(wakes(&shared.queue), 1);
    let second: Vec<Vec<u8>> = one[10..].iter().chain(&two[..10]).cloned().collect();
    let mut signals = Vec::new();
    let mut completed = None;
    receive_batch(
        &shared,
        None,
        &mut signals,
        second.len(),
        |i| second[i].as_slice(),
        &mut None,
        &mut staged,
        |signals| {
            for signal in signals.drain(..) {
                if let VideoSignal::FrameReceived { frame, .. } = signal {
                    // Routed after the completing datagram, before the rest of the call.
                    let state = lock(&shared.queue.state);
                    completed = Some((frame, state.frames, state.chunks));
                }
            }
        },
    );
    assert_eq!(completed, Some((1, 1, 14)), "the frame and its 14 chunks were queued");
    // The queue already held frame one's first block, so empty-queue wakeups fire only once.
    assert_eq!(wakes(&shared.queue), 1);
    let queued = drain(&shared.queue);
    assert_eq!(queued.len(), 14 + 1 + 8, "{queued:?}");
    assert!(matches!(queued[0], Queued::Chunk { first: true, .. }));
    assert_eq!(queued[14], Queued::Frame { number: 1 });
    assert!(matches!(queued[15], Queued::Chunk { first: true, .. }));
}

#[test]
fn receive_calls_queue_the_same_chunks_and_frames_in_the_same_order() {
    let mut host = Blocks::new();
    let mut packets = Vec::new();
    for n in 0..6u8 {
        let mut frame = flat(host.frame(&[12, 9, 5], n));
        if n == 3 {
            // A lost data packet, recovered from parity later in the block.
            frame.remove(4);
        }
        packets.extend(frame);
    }
    let one_at_a_time = VideoShared::new(records(true));
    feed(&one_at_a_time, &packets);
    let want = drain(&one_at_a_time.queue);
    let frames: Vec<&Queued> = want
        .iter()
        .filter(|q| matches!(q, Queued::Frame { .. }))
        .collect();
    assert_eq!(frames.len(), 6, "{want:?}");
    // Calls that end anywhere: inside a frame, on its last data packet, among its parity,
    // and one call for the whole stream.
    for call in [1, 2, 7, 16, net::RECEIVE_BATCH, packets.len()] {
        let shared = VideoShared::new(records(true));
        let mut staged = StagedChunks::default();
        feed_calls(&shared, &mut staged, &packets, call);
        assert_eq!(drain(&shared.queue), want, "calls of {call}");
        assert_eq!(staged.len(), 0);
        assert_eq!(shared.counters.chunks_dropped.load(Ordering::Relaxed), 0);
    }
}

#[test]
fn a_flush_mid_call_drops_the_chunks_staged_before_it_as_queued_ones() {
    let packets = flat(Blocks::new().frame(&[12, 9], 1));
    let flush_at = 5;
    // One at a time, the flush (a decode error's refresh) finds five chunks queued.
    let one_at_a_time = VideoShared::new(records(true));
    feed(&one_at_a_time, &packets[..flush_at]);
    one_at_a_time.queue.flush();
    feed(&one_at_a_time, &packets[flush_at..]);
    let want = drain(&one_at_a_time.queue);
    assert!(
        matches!(want[0], Queued::Chunk { first: false, .. }),
        "{want:?}"
    );
    // In one call, the same flush between the same two datagrams.
    let shared = VideoShared::new(records(true));
    let mut staged = StagedChunks::default();
    let mut signals = Vec::new();
    receive_batch(
        &shared,
        None,
        &mut signals,
        packets.len(),
        |i| {
            if i == flush_at {
                shared.queue.flush();
            }
            packets[i].as_slice()
        },
        &mut None,
        &mut staged,
        |signals| signals.clear(),
    );
    assert_eq!(drain(&shared.queue), want);
    assert_eq!(shared.counters.chunks_dropped.load(Ordering::Relaxed), 0);
}

#[test]
fn staged_chunks_keep_the_bound_shutdown_and_flushes_of_queued_ones() {
    let queue = FrameQueue::default();
    let mut staged = StagedChunks::default();
    // A flush after staging, before the chunks reach the queue, drops them uncounted.
    staged.stage(&queue, true, 0, &[1]);
    staged.stage(&queue, false, 0, &[2]);
    queue.flush();
    assert_eq!(queue.offer_staged(&mut staged), 0);
    assert_eq!(staged.len(), 0);
    assert!(lock(&queue.state).items.is_empty());
    assert_eq!(wakes(&queue), 0, "nothing to wake for");
    // Staged after it, they are queued.
    staged.stage(&queue, true, 0, &[3]);
    assert_eq!(queue.offer_staged(&mut staged), 0);
    assert_eq!(drain(&queue).len(), 1);
    // The bound: the chunks over it are dropped and counted, in their order.
    for i in 0..CHUNK_QUEUE_BOUND - 1 {
        assert!(queue.offer_chunk(i == 0, 0, &[4]));
    }
    for i in 0..3u8 {
        staged.stage(&queue, false, 0, &[5 + i]);
    }
    assert_eq!(queue.offer_staged(&mut staged), 2);
    let state = lock(&queue.state);
    assert_eq!(state.chunks, CHUNK_QUEUE_BOUND);
    assert!(matches!(state.items.back(), Some(Item::Chunk(c)) if c.data == [5]));
    drop(state);
    // A frame behind staged chunks over the bound is still queued.
    staged.stage(&queue, false, 0, &[9]);
    let (dropped, result) = queue.offer_behind(&mut staged, Frame::default());
    assert_eq!((dropped, result.is_ok()), (1, true));
    // After shutdown nothing counts as dropped.
    queue.shutdown();
    staged.stage(&queue, false, 0, &[10]);
    assert_eq!(queue.offer_staged(&mut staged), 0);
}

#[test]
fn staged_chunks_reuse_the_queues_returned_buffers() {
    let queue = FrameQueue::default();
    let mut staged = StagedChunks::default();
    staged.stage(&queue, true, 0, &[1]);
    queue.offer_staged(&mut staged);
    let mut back: Vec<Vec<u8>> = (0..RETURN_BATCH)
        .map(|_| Vec::with_capacity(2048))
        .collect();
    queue.wait(&mut back);
    assert_eq!(lock(&queue.state).spare.len(), RETURN_BATCH);
    // The next chunks to reach the queue bring the returned buffers to the stage.
    staged.stage(&queue, true, 0, &[2]);
    queue.offer_staged(&mut staged);
    assert_eq!(staged.spare.len(), RETURN_BATCH);
    assert!(lock(&queue.state).spare.is_empty());
    staged.stage(&queue, false, 0, &[3]);
    assert!(staged.chunks[0].data.capacity() >= 2048, "a staged chunk reuses a buffer");
}

#[test]
fn losses_in_the_stream_still_end_in_whole_frames() {
    let shared = Arc::new(VideoShared::new(records(true)));
    let (tx, rx) = mpsc::channel();
    let running = Running::start(&shared, Recorder::new(tx), rx);
    let mut host = Blocks::new();
    for n in 0..8u8 {
        let packets = flat(host.frame(&[16, 11], n));
        // One data packet of each block lost; its parity recovers it. The first block
        // has 20 datagrams, 16 of them data.
        let lost = if n % 2 == 0 { [3usize, 24] } else { [7, 29] };
        let arrived: Vec<Vec<u8>> = packets
            .iter()
            .enumerate()
            .filter(|(i, _)| !lost.contains(i))
            .map(|(_, p)| p.clone())
            .collect();
        feed(&shared, &arrived);
    }
    let seen = running.finish();
    let frames = frames_seen(&seen);
    assert_eq!(frames.len(), 8, "{seen:?}");
    assert!(frames.iter().all(|(_, whole)| *whole), "{seen:?}");
}

#[test]
fn a_refresh_in_the_middle_of_a_frame_leaves_it_to_be_caught_from_its_bytes() {
    let shared = Arc::new(VideoShared::new(records(true)));
    let mut host = Blocks::new();
    feed(&shared, &flat(host.frame(&[8, 6], 1)));
    // The next frame's first block arrives, then the decoder is refreshed (a decode error,
    // say): the queue drops everything it holds, the first chunk of this frame included.
    let two = host.frame(&[8, 6], 2);
    feed(&shared, &two[0]);
    refresh(&shared);
    feed(&shared, &two[1]);
    feed(&shared, &flat(host.frame(&[8, 6], 3)));
    // The decoder only starts now, so it sees the queue as the refresh left it.
    let (tx, rx) = mpsc::channel();
    let running = Running::start(&shared, Recorder::new(tx), rx);
    let seen = running.finish();
    let verdicts = frames_seen(&seen);
    let (last, whole) = *verdicts.last().expect("a frame after the refresh");
    assert!(whole, "the frame after the refresh is whole: {seen:?}");
    for (number, whole) in &verdicts {
        if *number < last {
            assert!(!whole, "frame {number} lost its first chunk in the flush: {seen:?}");
        }
    }
    // Nothing flushed reached the decoder: the one first chunk is the last frame's.
    let firsts = seen
        .iter()
        .filter(|s| matches!(s, Seen::Chunk { first: true }))
        .count();
    assert_eq!(firsts, 1, "{seen:?}");
}

#[test]
fn a_decode_error_is_counted_after_the_chunks_it_had() {
    let shared = Arc::new(VideoShared::new(records(true)));
    let (tx, rx) = mpsc::channel();
    let mut recorder = Recorder::new(tx);
    recorder.status = DecodeStatus::NeedIdr;
    let running = Running::start(&shared, recorder, rx);
    let mut host = Blocks::new();
    feed(&shared, &flat(host.frame(&[8, 6], 1)));
    let seen = running.finish();
    assert!(matches!(seen.last(), Some(Seen::Frame { whole: true, .. })), "{seen:?}");
    assert_eq!(shared.counters.need_idr.load(Ordering::Relaxed), 1);
}
