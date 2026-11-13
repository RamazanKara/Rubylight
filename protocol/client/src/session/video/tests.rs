use super::*;
use crate::video::tests::early_chunks::{records, Blocks};
use std::sync::mpsc;

/// How long a test waits for something it caused before calling it a hang. Only a hang
/// runs into it: no test passes or fails on how fast a thread gets scheduled.
const HANG_GUARD: Duration = Duration::from_secs(60);

/// Waits for `done`, polling: for state another thread changes without signalling the test.
fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + HANG_GUARD;
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn next_item(queue: &FrameQueue) -> Item {
    queue.wait(&mut Vec::new()).expect("an item")
}

#[test]
fn the_queue_is_bounded_and_shuts_down() {
    let queue = FrameQueue::default();
    for _ in 0..DECODE_UNIT_QUEUE_BOUND {
        assert!(queue.offer(Frame::default()).is_ok());
    }
    assert!(queue.offer(Frame::default()).is_err());
    assert_eq!(queue.flush().len(), DECODE_UNIT_QUEUE_BOUND);
    queue.offer(Frame::default()).unwrap();
    assert!(queue.wait(&mut Vec::new()).is_some());
    queue.shutdown();
    assert!(queue.wait(&mut Vec::new()).is_none());
    assert!(queue.offer(Frame::default()).is_err());
}

#[test]
fn chunks_have_their_own_bound_and_a_drop_is_reported() {
    let queue = FrameQueue::default();
    for i in 0..CHUNK_QUEUE_BOUND {
        assert!(queue.offer_chunk(i == 0, 2, &[1, 2, 3]));
    }
    assert!(!queue.offer_chunk(false, 2, &[9]), "over the bound is a drop");
    assert!(queue.offer(Frame::default()).is_ok(), "frames are bounded apart from chunks");
    assert_eq!(queue.len(), 1, "len counts frames");
    assert!(matches!(next_item(&queue), Item::Chunk(c) if c.first && c.data == [1, 2, 3]));
    assert!(queue.offer_chunk(false, 0, &[4]), "room again once one is taken");
    assert!(!queue.offer_chunk(false, 0, &[5]));
    queue.shutdown();
    assert!(queue.offer_chunk(false, 0, &[6]), "after shutdown nothing counts as dropped");
}

#[test]
fn returned_buffers_come_back_in_batches_and_are_reused() {
    let queue = FrameQueue::default();
    for _ in 0..4 {
        assert!(queue.offer_chunk(false, 0, &[1]));
    }
    let spares = || lock(&queue.state).spare.len();
    let mut back: Vec<Vec<u8>> = (0..RETURN_BATCH - 1)
        .map(|_| Vec::with_capacity(2048))
        .collect();
    queue.wait(&mut back);
    assert_eq!((back.len(), spares()), (RETURN_BATCH - 1, 0), "fewer than a batch stay");
    back.push(Vec::with_capacity(2048));
    queue.wait(&mut back);
    assert_eq!((back.len(), spares()), (0, RETURN_BATCH), "a batch goes back in one lock");
    // Two of the four are left; the next chunk takes a returned buffer.
    queue.offer_chunk(false, 0, &[7]);
    assert_eq!(spares(), RETURN_BATCH - 1);
    let mut last = None;
    for _ in 0..3 {
        last = queue.wait(&mut Vec::new());
    }
    let Some(Item::Chunk(chunk)) = last else {
        panic!("a chunk")
    };
    assert_eq!(chunk.data, [7]);
    assert!(chunk.data.capacity() >= 2048, "an offered chunk reuses a returned buffer");
    // The spares are bounded.
    let mut many: Vec<Vec<u8>> = (0..CHUNK_QUEUE_BOUND + 10).map(|_| Vec::new()).collect();
    queue.offer_chunk(false, 0, &[8]);
    queue.wait(&mut many);
    assert!(many.is_empty());
    assert_eq!(spares(), CHUNK_QUEUE_BOUND);
}

#[test]
fn a_frame_takes_its_chunk_buffers_back_before_the_decoder_holds_it() {
    // The decoder may sit in submit for a while; the buffers of the chunks it already
    // decoded must be there for the next frame's chunks, or the receiver allocates new
    // ones whenever the decoder happens to hold more than before.
    let queue = FrameQueue::default();
    for i in 0..3 {
        assert!(queue.offer_chunk(i == 0, 0, &[i]));
    }
    queue.offer(Frame::default()).unwrap();
    let mut back = Vec::new();
    for _ in 0..3 {
        let Some(Item::Chunk(chunk)) = queue.wait(&mut back) else {
            panic!("a chunk")
        };
        back.push(chunk.data);
    }
    let spares = || lock(&queue.state).spare.len();
    assert_eq!((back.len(), spares()), (3, 0), "chunks alone wait for a batch");
    assert!(matches!(queue.wait(&mut back), Some(Item::Frame(_))));
    assert_eq!((back.len(), spares()), (0, 3), "the frame hands them back");
}

#[test]
fn a_sleeping_waiter_hands_back_what_it_holds_and_wakes_for_a_chunk() {
    let queue = Arc::new(FrameQueue::default());
    let (tx, rx) = mpsc::channel();
    let waiter = {
        let queue = queue.clone();
        std::thread::spawn(move || {
            let mut back = vec![Vec::new(), Vec::new(), Vec::new()];
            let item = queue.wait(&mut back);
            tx.send((item.is_some(), back.len())).unwrap();
        })
    };
    // It sleeps on the empty queue after giving its three buffers back.
    wait_until("the waiter never handed its buffers back", || {
        lock(&queue.state).spare.len() == 3
    });
    assert!(queue.offer_chunk(true, 0, &[1]));
    assert_eq!(rx.recv_timeout(HANG_GUARD).unwrap(), (true, 0));
    waiter.join().unwrap();
}

#[test]
fn a_flush_drops_queued_chunks_with_the_frames_and_keeps_the_capacity() {
    let queue = FrameQueue::default();
    for round in 0..3 {
        queue.offer_chunk(round == 0, 0, &[1, 2]);
    }
    queue.offer(Frame::default()).unwrap();
    queue.offer_chunk(true, 0, &[3]);
    queue.offer_chunk(false, 0, &[4]);
    let capacity = lock(&queue.state).items.capacity();
    let frames = queue.flush();
    assert_eq!(frames.len(), 1, "only the frame comes back");
    {
        let state = lock(&queue.state);
        assert_eq!((state.frames, state.chunks, state.items.len()), (0, 0, 0));
        assert_eq!(state.items.capacity(), capacity, "the off path keeps its allocation");
        assert_eq!(state.spare.len(), 5, "the chunk buffers are kept");
    }
    assert_eq!(queue.len(), 0);
    // The chunk count was reset with it.
    for i in 0..CHUNK_QUEUE_BOUND {
        assert!(queue.offer_chunk(i == 0, 0, &[1]), "chunk {i}");
    }
    assert!(!queue.offer_chunk(false, 0, &[1]));
    assert!(queue.flush().is_empty());
    assert!(queue.offer_chunk(true, 0, &[1]));
}

#[test]
fn a_flush_on_the_off_path_allocates_no_frame_list() {
    let queue = FrameQueue::default();
    assert_eq!(queue.flush().capacity(), 0);
    queue.offer(Frame::default()).unwrap();
    assert_eq!(queue.flush().len(), 1);
}

/// What the mock decoder saw, in order.
#[derive(Debug, PartialEq)]
enum Seen {
    Chunk { first: bool },
    /// A frame, and whether the chunks since the last first add up to its envelope.
    Frame { number: u32, whole: bool },
    /// A frame passed over for a newer one.
    Skipped { number: u32 },
}

/// Accumulates the chunks the way the desktop decoder checks them against the frame.
struct Recorder {
    seen: mpsc::Sender<Seen>,
    envelope: Vec<u8>,
    status: DecodeStatus,
    rfi: bool,
    intra_only: bool,
}

impl Recorder {
    fn new(seen: mpsc::Sender<Seen>) -> Self {
        Self {
            seen,
            envelope: Vec::new(),
            status: DecodeStatus::Ok,
            rfi: true,
            intra_only: false,
        }
    }

    fn intra_only(mut self) -> Self {
        self.intra_only = true;
        self
    }
}

impl VideoDecoder for Recorder {
    fn setup(&mut self, _: &crate::session::VideoSetup) -> Result<(), i32> {
        Ok(())
    }

    fn wants_early_chunks(&self) -> bool {
        true
    }

    fn chunk(&mut self, first: bool, flags: u32, payload: &[u8]) {
        if first {
            self.envelope = vec![0; 4];
        }
        self.envelope
            .extend_from_slice(&(payload.len() as u32).to_le_bytes());
        self.envelope.extend_from_slice(&flags.to_le_bytes());
        self.envelope.extend_from_slice(payload);
        self.seen.send(Seen::Chunk { first }).unwrap();
    }

    fn intra_only(&self) -> bool {
        self.intra_only
    }

    fn skip(&mut self, frame: &Frame) {
        self.envelope.clear();
        self.seen
            .send(Seen::Skipped {
                number: frame.number,
            })
            .unwrap();
    }

    fn submit(&mut self, frame: &Frame) -> DecodeStatus {
        let whole = self.envelope == frame.data();
        self.envelope.clear();
        self.seen
            .send(Seen::Frame {
                number: frame.number,
                whole,
            })
            .unwrap();
        std::mem::replace(&mut self.status, DecodeStatus::Ok)
    }

    fn recovers_with_rfi(&self) -> bool {
        self.rfi
    }
}

/// The decoder thread's loop on a thread of its own.
struct Running {
    shared: Arc<VideoShared>,
    seen: mpsc::Receiver<Seen>,
    thread: JoinHandle<Box<dyn VideoDecoder>>,
}

impl Running {
    fn start(shared: &Arc<VideoShared>, recorder: Recorder, seen: mpsc::Receiver<Seen>) -> Self {
        let for_thread = shared.clone();
        Self {
            shared: shared.clone(),
            seen,
            thread: std::thread::spawn(move || {
                decode_loop(Box::new(recorder), &for_thread, |_| {})
            }),
        }
    }

    /// Stops the decoder once it has taken everything queued, and returns all it saw. The
    /// tests queue everything before this, and the one item the decoder may still be
    /// handling when the queue runs empty is finished before it sees the shutdown.
    fn finish(self) -> Vec<Seen> {
        wait_until("the decoder never took everything queued", || {
            lock(&self.shared.queue.state).items.is_empty()
        });
        self.shared.queue.shutdown();
        self.thread.join().unwrap();
        self.seen.try_iter().collect()
    }
}

/// The datagrams one at a time, each chunk queued as the receiver gives it.
fn feed(shared: &VideoShared, packets: &[Vec<u8>]) {
    let mut signals = Vec::new();
    for packet in packets {
        let mut receiver = lock(&shared.receiver);
        let mut sink = Sink {
            shared,
            direct: None,
            staged: None,
            signals: &mut signals,
        };
        receiver.receive(packet, 7, &mut sink);
    }
}

/// The datagrams as the receive thread takes them, in receive calls of `call` datagrams.
fn feed_calls(
    shared: &VideoShared,
    staged: &mut StagedChunks,
    packets: &[Vec<u8>],
    call: usize,
) {
    let mut signals = Vec::new();
    for batch in packets.chunks(call) {
        receive_batch(
            shared,
            None,
            &mut signals,
            batch.len(),
            |i| batch[i].as_slice(),
            &mut None,
            staged,
            |signals| signals.clear(),
        );
    }
}

fn refresh(shared: &VideoShared) {
    let mut signals = Vec::new();
    let mut sink = Sink {
        shared,
        direct: None,
        staged: None,
        signals: &mut signals,
    };
    lock(&shared.receiver).request_decoder_refresh(&mut sink);
}

fn flat(blocks: Vec<Vec<Vec<u8>>>) -> Vec<Vec<u8>> {
    blocks.into_iter().flatten().collect()
}

fn frames_seen(seen: &[Seen]) -> Vec<(u32, bool)> {
    seen.iter()
        .filter_map(|s| match s {
            Seen::Frame { number, whole } => Some((*number, *whole)),
            Seen::Chunk { .. } | Seen::Skipped { .. } => None,
        })
        .collect()
}

fn decoded_numbers(seen: &[Seen]) -> Vec<u32> {
    frames_seen(seen).iter().map(|frame| frame.0).collect()
}

fn skipped_numbers(seen: &[Seen]) -> Vec<u32> {
    seen.iter()
        .filter_map(|s| match s {
            Seen::Skipped { number } => Some(*number),
            _ => None,
        })
        .collect()
}

fn chunks_seen(seen: &[Seen]) -> usize {
    seen.iter()
        .filter(|s| matches!(s, Seen::Chunk { .. }))
        .count()
}

// The data and buffers are private to the video module, so no struct update.
#[allow(clippy::field_reassign_with_default)]
fn numbered(number: u32, frame_type: FrameType) -> Frame {
    let mut frame = Frame::default();
    frame.number = number;
    frame.frame_type = frame_type;
    frame
}

/// `frames` frames queued as the receiver would, each behind its three chunks, with
/// nothing taking them yet: the decoder finds them as a backlog when it starts.
fn backlog(shared: &VideoShared, frames: u32, first_type: FrameType) {
    for number in 1..=frames {
        for i in 0..3 {
            assert!(shared.queue.offer_chunk(i == 0, 0, &[number as u8, i]));
        }
        let frame_type = if number == 1 {
            first_type
        } else {
            FrameType::P
        };
        assert!(shared.queue.offer(numbered(number, frame_type)).is_ok());
    }
}

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
    let thread =
        std::thread::spawn(move || decode_loop(Box::new(decoder), &for_thread, |_| {}));
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
    // The frame woke the decoder; the chunks behind it found the queue busy.
    assert_eq!(wakes(&shared.queue), 2);
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

#[derive(Debug, PartialEq)]
enum Step {
    Datagram,
    /// A submission: whether the receiver was free, and the frame's data buffer.
    Submit { unlocked: bool, data: usize },
    RequestIdr,
}

/// A direct decoder that logs its submissions and fails the first.
struct DirectFake {
    shared: Arc<VideoShared>,
    log: Arc<Mutex<Vec<Step>>>,
    status: DecodeStatus,
}

impl VideoDecoder for DirectFake {
    fn setup(&mut self, _: &crate::session::VideoSetup) -> Result<(), i32> {
        Ok(())
    }

    fn submit(&mut self, frame: &Frame) -> DecodeStatus {
        let unlocked = self.shared.receiver.try_lock().is_ok();
        lock(&self.log).push(Step::Submit {
            unlocked,
            data: frame.data().as_ptr() as usize,
        });
        std::mem::replace(&mut self.status, DecodeStatus::Ok)
    }
}

#[test]
fn a_direct_decoder_submits_outside_the_receivers_lock_and_its_feedback_comes_first() {
    let shared = Arc::new(VideoShared::new(records(false)));
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut direct = Direct::new(Box::new(DirectFake {
        shared: shared.clone(),
        log: log.clone(),
        status: DecodeStatus::NeedIdr,
    }));
    let mut host = Blocks::new();
    let mut packets = flat(host.frame(&[8, 6], 1));
    packets.extend(flat(host.frame(&[8, 6], 2)));
    packets.extend(flat(host.frame(&[8, 6], 3)));
    let mut signals = Vec::new();
    receive_batch(
        &shared,
        Some(&mut direct),
        &mut signals,
        packets.len(),
        |i| {
            lock(&log).push(Step::Datagram);
            packets[i].as_slice()
        },
        &mut None,
        &mut StagedChunks::default(),
        |signals| {
            for signal in signals.drain(..) {
                if matches!(signal, VideoSignal::RequestIdr) {
                    lock(&log).push(Step::RequestIdr);
                }
            }
        },
    );
    let log = std::mem::take(&mut *lock(&log));
    let first = log
        .iter()
        .position(|s| matches!(s, Step::Submit { .. }))
        .expect("a submission");
    let Step::Submit { unlocked, data } = log[first] else {
        unreachable!()
    };
    assert!(unlocked, "{log:?}");
    // The failed submission's key frame request comes before the next datagram.
    assert_eq!(log.get(first + 1), Some(&Step::RequestIdr), "{log:?}");
    assert!(direct.ready.is_empty() && direct.done.is_empty());
    assert_eq!(shared.counters.submitted.load(Ordering::Relaxed), 3, "{log:?}");
    assert_eq!(shared.counters.need_idr.load(Ordering::Relaxed), 1);
    // The first frame's recycled buffer becomes the depacketizer's at the second frame's
    // reassembly, so the third frame carries it.
    let later: Vec<(bool, usize)> = log[first + 1..]
        .iter()
        .filter_map(|s| match s {
            Step::Submit { unlocked, data } => Some((*unlocked, *data)),
            _ => None,
        })
        .collect();
    assert_eq!(later.len(), 2, "{log:?}");
    assert_eq!(later[1], (true, data), "{log:?}");
}

/// A decoder replaced mid-stream by one that never asked for reference frame invalidation
/// has the rest of the stream recover with key frames; one that did keeps RFI.
#[test]
fn a_decoder_without_rfi_turns_the_receivers_rfi_recovery_off() {
    for rfi in [true, false] {
        let mut config = crate::video::tests::config(0x01_0000);
        (config.pyrowave_records, config.early_chunks, config.rfi) = (true, true, true);
        let shared = Arc::new(VideoShared::new(VideoReceiver::new(config)));
        assert!(lock(&shared.receiver).rfi_recovery());
        let (tx, rx) = mpsc::channel();
        let mut recorder = Recorder::new(tx);
        recorder.rfi = rfi;
        let running = Running::start(&shared, recorder, rx);
        feed(&shared, &flat(Blocks::new().frame(&[8, 6], 1)));
        let seen = running.finish();
        assert_eq!(frames_seen(&seen).len(), 1, "{seen:?}");
        assert_eq!(lock(&shared.receiver).rfi_recovery(), rfi, "decoder with rfi {rfi}");
    }
}
