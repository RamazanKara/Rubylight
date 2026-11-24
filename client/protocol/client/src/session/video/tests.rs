//! Unit tests for the video decode-unit queue, early-chunk staging, backlog skipping,
//! direct submission and key-frame request pacing.

mod backlog;
mod direct_and_refresh;
mod staging;

use super::queue::{CHUNK_QUEUE_BOUND, SKIP_BACKLOG, SKIP_HOLD_US};
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
fn a_set_queue_bound_refuses_the_frame_over_it_and_unset_or_larger_keeps_fifteen() {
    assert_eq!(decode_queue_bound(0), DECODE_UNIT_QUEUE_BOUND);
    assert_eq!(decode_queue_bound(40), DECODE_UNIT_QUEUE_BOUND);
    assert_eq!(decode_queue_bound(3), 3);
    assert_eq!(FrameQueue::default().bound(), DECODE_UNIT_QUEUE_BOUND);
    let queue = FrameQueue::with_bound(3);
    for _ in 0..3 {
        assert!(queue.offer(Frame::default()).is_ok());
    }
    assert!(queue.offer(Frame::default()).is_err());
    let mut staged = StagedChunks::default();
    assert!(queue.offer_behind(&mut staged, Frame::default()).1.is_err());
    assert_eq!(queue.flush().len(), 3);
    assert!(queue.offer(Frame::default()).is_ok());
    let shared = VideoShared::with_queue_bound(records(true), 5);
    assert_eq!(shared.queue.bound(), 5);
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
                decode_loop(Box::new(recorder), &for_thread, KeyFrameRequests::new(60), |_| {})
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
