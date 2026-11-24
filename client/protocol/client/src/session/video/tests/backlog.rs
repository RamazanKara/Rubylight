//! Backlog skipping and slow-decoder pacing tests for the video decoder thread.

use super::*;

#[test]
fn the_backlog_rule_needs_two_newer_frames_and_then_holds_at_one() {
    let mut rule = Backlog::default();
    // A decoder that keeps up: nothing behind, or the one frame of a pair that arrived
    // together, however long that goes on.
    for now in [0, 5_000, 2_000_000] {
        assert!(!rule.skips(0, now));
        assert!(!rule.skips(1, now));
        assert!(!rule.skips_chunk(2, now), "its frame is taken with one behind");
    }
    // Two newer frames: behind for certain, and so is a chunk whose frame has two behind.
    assert!(rule.skips_chunk(3, 3_000_000));
    assert!(rule.skips(2, 3_000_000));
    // From here one newer frame is enough, down to the newest, which is decoded.
    assert!(rule.skips_chunk(2, 3_000_100));
    assert!(rule.skips(1, 3_000_100));
    assert!(!rule.skips(0, 3_000_200));
    assert!(!rule.skips_chunk(1, 3_000_200), "the newest frame keeps its chunks");
    // Each skip holds the rule for another second: a decoder a little slower than the
    // stream drops single frames, each as it falls due.
    assert!(rule.skips(1, 3_000_100 + SKIP_HOLD_US - 1));
    assert!(rule.skips(1, 3_000_100 + 2 * SKIP_HOLD_US - 2));
    // A second without a skip, and it takes two newer frames again.
    let later = 3_000_100 + 3 * SKIP_HOLD_US;
    assert!(!rule.skips(1, later));
    assert!(!rule.skips_chunk(2, later));
    assert!(rule.skips(2, later));
}

#[test]
fn a_backlog_decodes_only_the_newest_frame_of_an_intra_only_decoder() {
    let shared = Arc::new(VideoShared::new(records(true)));
    backlog(&shared, 5, FrameType::P);
    let (tx, rx) = mpsc::channel();
    let running = Running::start(&shared, Recorder::new(tx).intra_only(), rx);
    let seen = running.finish();
    assert_eq!(skipped_numbers(&seen), [1, 2, 3, 4], "{seen:?}");
    assert_eq!(decoded_numbers(&seen), [5], "{seen:?}");
    let counters = &shared.counters;
    assert_eq!(counters.frames_skipped.load(Ordering::Relaxed), 4);
    assert_eq!(
        counters.submitted.load(Ordering::Relaxed),
        1,
        "skipped frames are not submitted"
    );
    assert_eq!(counters.need_idr.load(Ordering::Relaxed), 0);
    assert_eq!(counters.queue_overflows.load(Ordering::Relaxed), 0);
}

#[test]
fn the_chunks_of_skipped_frames_are_dropped_and_not_counted_as_dropped() {
    let shared = Arc::new(VideoShared::new(records(true)));
    backlog(&shared, 4, FrameType::P);
    let (tx, rx) = mpsc::channel();
    let running = Running::start(&shared, Recorder::new(tx).intra_only(), rx);
    let seen = running.finish();
    // Frame 1's chunks have four frames behind them and frame 2's three, so they go
    // unseen. Frame 3's have two, which is one behind the frame itself: only the skips
    // before it say that it goes too. Frame 4 keeps its chunks, first one first.
    assert_eq!(skipped_numbers(&seen), [1, 2, 3], "{seen:?}");
    assert_eq!(chunks_seen(&seen), 3, "{seen:?}");
    assert!(
        matches!(seen[seen.len() - 4], Seen::Chunk { first: true }),
        "{seen:?}"
    );
    assert!(
        matches!(seen.last(), Some(Seen::Frame { number: 4, .. })),
        "{seen:?}"
    );
    let counters = &shared.counters;
    assert_eq!(counters.chunks_skipped.load(Ordering::Relaxed), 9);
    assert_eq!(counters.chunks_dropped.load(Ordering::Relaxed), 0);
    assert_eq!(
        lock(&shared.queue.state).chunks,
        0,
        "their buffers left the queue"
    );
}

#[test]
fn a_pair_of_frames_is_no_backlog_and_both_are_decoded_with_their_chunks() {
    let shared = Arc::new(VideoShared::new(records(true)));
    backlog(&shared, 2, FrameType::P);
    let (tx, rx) = mpsc::channel();
    let running = Running::start(&shared, Recorder::new(tx).intra_only(), rx);
    let seen = running.finish();
    assert!(skipped_numbers(&seen).is_empty(), "{seen:?}");
    assert_eq!(decoded_numbers(&seen), [1, 2], "{seen:?}");
    assert_eq!(chunks_seen(&seen), 6, "{seen:?}");
    let counters = &shared.counters;
    assert_eq!(counters.submitted.load(Ordering::Relaxed), 2);
    assert_eq!(counters.frames_skipped.load(Ordering::Relaxed), 0);
    assert_eq!(counters.chunks_skipped.load(Ordering::Relaxed), 0);
}

#[test]
fn a_decoder_that_predicts_from_earlier_frames_sees_every_frame() {
    let shared = Arc::new(VideoShared::new(records(true)));
    backlog(&shared, 5, FrameType::P);
    let (tx, rx) = mpsc::channel();
    let running = Running::start(&shared, Recorder::new(tx), rx);
    let seen = running.finish();
    assert_eq!(decoded_numbers(&seen), [1, 2, 3, 4, 5], "{seen:?}");
    assert_eq!(chunks_seen(&seen), 15);
    let counters = &shared.counters;
    assert_eq!(counters.submitted.load(Ordering::Relaxed), 5);
    assert_eq!(counters.frames_skipped.load(Ordering::Relaxed), 0);
    assert_eq!(counters.chunks_skipped.load(Ordering::Relaxed), 0);
}

/// The receive thread holding the receiver's lock (FEC recovery, frame assembly) does not hold
/// up the decoder: decoded frames go back through their own list, which the next lock section
/// takes into the receiver's pool.
#[test]
fn the_decoder_hands_frames_back_without_waiting_for_the_receivers_lock() {
    let shared = Arc::new(VideoShared::new(records(true)));
    backlog(&shared, 5, FrameType::P);
    let (tx, rx) = mpsc::channel();
    let held = lock(&shared.receiver);
    let running = Running::start(&shared, Recorder::new(tx), rx);
    wait_until("the decoder waited for the receiver's lock", || {
        lock(&shared.returns.frames).len() == 5
    });
    drop(held);
    let seen = running.finish();
    assert_eq!(decoded_numbers(&seen), [1, 2, 3, 4, 5], "{seen:?}");
    let mut signals = Vec::new();
    let mut staged = StagedChunks::default();
    receive_batch(
        &shared,
        None,
        &mut signals,
        0,
        |_| -> &'static [u8] { &[] },
        &mut None,
        &mut staged,
        |_| {},
    );
    assert!(lock(&shared.returns.frames).is_empty());
    assert!(!shared.returns.pending.load(Ordering::Relaxed));
}

#[test]
fn a_skipped_key_frame_is_reported_decoded_with_the_next_frame_that_is() {
    // Frame 3 is decoded, but the decoder refuses it: no key frame has been decoded.
    let shared = Arc::new(VideoShared::new(records(true)));
    backlog(&shared, 3, FrameType::Idr);
    assert!(!lock(&shared.receiver).idr_decoded());
    let (tx, rx) = mpsc::channel();
    let mut recorder = Recorder::new(tx).intra_only();
    recorder.status = DecodeStatus::NeedIdr;
    let seen = Running::start(&shared, recorder, rx).finish();
    assert_eq!(skipped_numbers(&seen), [1, 2], "{seen:?}");
    assert!(!lock(&shared.receiver).idr_decoded());
    assert_eq!(shared.counters.need_idr.load(Ordering::Relaxed), 1);

    // Accepted, it stands for the key frame skipped before it.
    let shared = Arc::new(VideoShared::new(records(true)));
    backlog(&shared, 3, FrameType::Idr);
    let (tx, rx) = mpsc::channel();
    let seen = Running::start(&shared, Recorder::new(tx).intra_only(), rx).finish();
    assert_eq!(skipped_numbers(&seen), [1, 2], "{seen:?}");
    assert!(
        lock(&shared.receiver).idr_decoded(),
        "the accepted frame reports the skipped key frame"
    );

    // Without a skipped key frame a decoded inter frame reports none.
    let shared = Arc::new(VideoShared::new(records(true)));
    backlog(&shared, 3, FrameType::P);
    let (tx, rx) = mpsc::channel();
    Running::start(&shared, Recorder::new(tx).intra_only(), rx).finish();
    assert!(!lock(&shared.receiver).idr_decoded());

    // A key frame that is decoded itself is reported as before.
    let shared = Arc::new(VideoShared::new(records(true)));
    backlog(&shared, 1, FrameType::Idr);
    let (tx, rx) = mpsc::channel();
    Running::start(&shared, Recorder::new(tx).intra_only(), rx).finish();
    assert!(lock(&shared.receiver).idr_decoded());
}

#[test]
fn a_decoder_slower_than_the_stream_never_fills_the_queue() {
    // Without the skip the frames pile up until one finds the queue full, which flushes
    // it and asks for a key frame.
    let (shared, full, _, seen) = slower_than_the_stream(false, false);
    assert!(full.is_some(), "the backlog never reached the bound");
    assert!(skipped_numbers(&seen).is_empty());
    assert_eq!(shared.counters.frames_skipped.load(Ordering::Relaxed), 0);

    let (shared, full, deepest, seen) = slower_than_the_stream(true, false);
    assert_eq!(full, None, "a frame found the queue full");
    assert!(deepest <= SKIP_BACKLOG + 1, "the queue held {deepest}");
    let (decoded, skipped) = (decoded_numbers(&seen), skipped_numbers(&seen));
    assert_eq!(decoded.len() + skipped.len(), SLOW_FRAMES as usize);
    assert!(decoded.windows(2).all(|pair| pair[0] < pair[1]), "{decoded:?}");
    assert_eq!(decoded.last(), Some(&SLOW_FRAMES), "the newest frame is decoded");
    // One frame in six goes, less the few that built the first backlog.
    assert!(skipped.len() >= 15 && skipped.len() <= 20, "{skipped:?}");
    let counters = &shared.counters;
    assert_eq!(counters.submitted.load(Ordering::Relaxed), decoded.len() as u64);
    assert_eq!(counters.frames_skipped.load(Ordering::Relaxed), skipped.len() as u64);
    assert_eq!(counters.need_idr.load(Ordering::Relaxed), 0);
}

#[test]
fn a_decoder_slower_than_the_stream_skips_alike_with_chunks_staged_per_receive_call() {
    // The same stream as PyroWave datagrams in receive calls: the chunks reach the queue
    // with their frame or at the end of the call, and the skips stay as they were.
    let (shared, full, deepest, seen) = slower_than_the_stream(true, true);
    assert_eq!(full, None, "a frame found the queue full");
    assert!(deepest <= SKIP_BACKLOG + 1, "the queue held {deepest}");
    let (decoded, skipped) = (decoded_numbers(&seen), skipped_numbers(&seen));
    assert_eq!(decoded.len() + skipped.len(), SLOW_FRAMES as usize);
    assert!(decoded.windows(2).all(|pair| pair[0] < pair[1]), "{decoded:?}");
    assert_eq!(decoded.last(), Some(&SLOW_FRAMES), "the newest frame is decoded");
    assert!(skipped.len() >= 15 && skipped.len() <= 20, "{skipped:?}");
    // A decoded frame had every chunk of its own, first one first.
    assert!(frames_seen(&seen).iter().all(|(_, whole)| *whole), "{seen:?}");
    assert!(chunks_seen(&seen) > 0);
    let counters = &shared.counters;
    assert_eq!(counters.submitted.load(Ordering::Relaxed), decoded.len() as u64);
    assert_eq!(counters.frames_skipped.load(Ordering::Relaxed), skipped.len() as u64);
    assert!(counters.chunks_skipped.load(Ordering::Relaxed) > 0);
    assert_eq!(counters.chunks_dropped.load(Ordering::Relaxed), 0);
    assert_eq!(counters.queue_overflows.load(Ordering::Relaxed), 0);
}

/// The frames [`slower_than_the_stream`] sends: six for every five decodes.
const SLOW_FRAMES: u32 = 121;

/// Holds each decode until the test lets it go, so the test decides how many frames
/// arrive during one: no sleeps, and nothing depends on how threads are scheduled.
struct Gated {
    recorder: Recorder,
    started: mpsc::Sender<()>,
    go: mpsc::Receiver<()>,
}

impl VideoDecoder for Gated {
    fn setup(&mut self, setup: &crate::session::VideoSetup) -> Result<(), i32> {
        self.recorder.setup(setup)
    }

    fn intra_only(&self) -> bool {
        self.recorder.intra_only
    }

    fn chunk(&mut self, first: bool, flags: u32, payload: &[u8]) {
        self.recorder.chunk(first, flags, payload);
    }

    fn skip(&mut self, frame: &Frame) {
        self.recorder.skip(frame);
    }

    fn submit(&mut self, frame: &Frame) -> DecodeStatus {
        self.started.send(()).unwrap();
        // A closed gate lets everything through: the test is over.
        let _ = self.go.recv();
        self.recorder.submit(frame)
    }
}

/// A stream a fifth faster than its decoder: one frame arrives during each decode and
/// two during every fifth, queued whole or, with `datagrams`, as PyroWave datagrams in
/// receive calls of [`net::RECEIVE_BATCH`]. Returns the first frame that found the queue
/// full, if one did, the most frames the queue held, and what the decoder saw.
fn slower_than_the_stream(
    intra_only: bool,
    datagrams: bool,
) -> (Arc<VideoShared>, Option<u32>, usize, Vec<Seen>) {
    let shared = Arc::new(VideoShared::new(records(true)));
    let (tx, rx) = mpsc::channel();
    let (started, in_decode) = mpsc::channel();
    let (go, gate) = mpsc::channel();
    let mut recorder = Recorder::new(tx);
    recorder.intra_only = intra_only;
    let decoder = Gated {
        recorder,
        started,
        go: gate,
    };
    let for_thread = shared.clone();
    let thread = std::thread::spawn(move || {
        decode_loop(
            Box::new(decoder),
            &for_thread,
            KeyFrameRequests::new(60),
            |_| {},
        )
    });
    let decoding = || {
        in_decode
            .recv_timeout(HANG_GUARD)
            .expect("the decoder never took the next frame")
    };
    let mut host = Blocks::new();
    let mut staged = StagedChunks::default();
    // Whether frame `number` found room in the queue.
    let mut arrive = |number: u32| {
        if !datagrams {
            return shared.queue.offer(numbered(number, FrameType::P)).is_ok();
        }
        // The test host numbers its frames from 1 as this does.
        let packets = flat(host.frame(&[8, 6], number as u8));
        feed_calls(&shared, &mut staged, &packets, net::RECEIVE_BATCH);
        shared.counters.queue_overflows.load(Ordering::Relaxed) == 0
    };
    assert!(arrive(1));
    decoding();
    let (mut number, mut full, mut deepest) = (1, None, 0);
    'stream: for decode in 1..=100 {
        let arriving = if decode % 5 == 0 { 2 } else { 1 };
        for _ in 0..arriving {
            number += 1;
            if !arrive(number) {
                full = Some(number);
                break 'stream;
            }
        }
        deepest = deepest.max(shared.queue.len());
        go.send(()).unwrap();
        // The newest frame is never skipped, so another decode always follows.
        decoding();
    }
    drop(go);
    wait_until("the decoder never took everything queued", || {
        lock(&shared.queue.state).items.is_empty()
    });
    shared.queue.shutdown();
    thread.join().unwrap();
    (shared, full, deepest, rx.try_iter().collect())
}
