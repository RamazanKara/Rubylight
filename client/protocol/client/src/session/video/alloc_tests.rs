//! Steady streaming allocates nothing per packet or per frame: a counting allocator over the
//! receive path and the decoder thread, with the decode queue filled to its bound, a block
//! recovered from parity and a key frame in every round.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use super::*;
use crate::session::VideoSetup;
use crate::video::tests::early_chunks::{records, Blocks};
use crate::video::tests::{config, h264_idr, h264_p, Host};

/// Counts the allocations of the threads that opted in, each into its test's counter, so the
/// tests running beside these are neither counted nor slowed by a shared counter.
struct Counting;

thread_local! {
    static COUNTER: Cell<Option<&'static AtomicU64>> = const { Cell::new(None) };
}

fn count() {
    if let Ok(Some(counter)) = COUNTER.try_with(Cell::get) {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

fn count_into(counter: Option<&'static AtomicU64>) {
    COUNTER.with(|c| c.set(counter));
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        System.alloc(layout)
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count();
        System.alloc_zeroed(layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count();
        System.realloc(ptr, layout, new_size)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// How long a test waits for the decoder thread before calling it a hang.
const HANG_GUARD: Duration = Duration::from_secs(60);

/// Frames of a round: the one the decoder holds, then as many as the decode queue takes.
const ROUND: usize = DECODE_UNIT_QUEUE_BOUND + 1;
/// Rounds that fill the pools and grow every buffer to its working size.
const WARM_UP: usize = 4;
/// Rounds that must not allocate.
const MEASURED: usize = 4;
/// The last round only starts: its first frame marks the end of the measurement.
const ROUNDS: usize = WARM_UP + MEASURED + 1;

/// Each frame of each round as its datagrams.
type Rounds = Vec<Vec<Vec<Vec<u8>>>>;

#[derive(Default)]
struct GateState {
    /// Frames the decoder may finish.
    allowed: usize,
    /// Frames the decoder has taken.
    started: usize,
}

/// Holds the decoder in `submit` until the test lets frames through.
#[derive(Default)]
struct Gate {
    state: Mutex<GateState>,
    changed: Condvar,
}

impl Gate {
    fn allow(&self, frames: usize) {
        lock(&self.state).allowed += frames;
        self.changed.notify_all();
    }

    /// Waits until the decoder has taken `frames` frames: then it is held in `submit` with
    /// everything before that frame done.
    fn wait_started(&self, frames: usize) {
        let state = lock(&self.state);
        let (state, wait) = self
            .changed
            .wait_timeout_while(state, HANG_GUARD, |s| s.started < frames)
            .unwrap();
        drop(state);
        assert!(!wait.timed_out(), "the decoder never took frame {frames}");
    }
}

struct Gated(Arc<Gate>);

impl VideoDecoder for Gated {
    fn setup(&mut self, _: &VideoSetup) -> Result<(), i32> {
        Ok(())
    }

    fn wants_early_chunks(&self) -> bool {
        true
    }

    fn submit(&mut self, _: &Frame) -> DecodeStatus {
        let mut state = lock(&self.0.state);
        state.started += 1;
        self.0.changed.notify_all();
        while state.allowed == 0 {
            state = self.0.changed.wait(state).unwrap();
        }
        state.allowed -= 1;
        DecodeStatus::Ok
    }
}

/// Feeds one frame's datagrams the way the receive thread does.
fn feed(shared: &VideoShared, frame: &[Vec<u8>], signals: &mut Vec<VideoSignal>) {
    for datagram in frame {
        let mut sink = Sink {
            shared,
            direct: None,
            signals: &mut *signals,
        };
        lock(&shared.receiver).receive(datagram, 7, &mut sink);
        signals.clear();
    }
}

/// Streams `rounds` through the receiver, the decode queue and the decoder thread, and returns
/// the allocations both threads made in the measured rounds. Each round fills the queue to
/// its bound behind a decoder held on its first frame, then lets the decoder drain it.
fn allocations_in_steady_streaming(
    counter: &'static AtomicU64,
    receiver: VideoReceiver,
    rounds: &Rounds,
) -> u64 {
    let shared = Arc::new(VideoShared::new(receiver));
    let gate = Arc::new(Gate::default());
    let decoder = {
        let (shared, gate) = (shared.clone(), gate.clone());
        std::thread::spawn(move || {
            count_into(Some(counter));
            let decoder = decode_loop(Box::new(Gated(gate)), &shared, |s| s.clear());
            count_into(None);
            drop(decoder);
        })
    };
    count_into(Some(counter));
    let mut signals = Vec::new();
    let mut before = 0;
    for (index, round) in rounds.iter().enumerate() {
        let (first, rest) = round.split_first().expect("a frame");
        feed(&shared, first, &mut signals);
        // The decoder holds this frame, done with every earlier one: both threads are idle.
        gate.wait_started(index * ROUND + 1);
        if index == WARM_UP {
            before = counter.load(Ordering::Relaxed);
        }
        if index == WARM_UP + MEASURED {
            break;
        }
        for frame in rest {
            feed(&shared, frame, &mut signals);
        }
        assert_eq!(shared.queue.len(), DECODE_UNIT_QUEUE_BOUND, "round {index}");
        gate.allow(ROUND);
        // The queue is empty once the decoder has taken the round's last frame.
        gate.wait_started((index + 1) * ROUND);
    }
    let allocations = counter.load(Ordering::Relaxed) - before;
    count_into(None);
    shared.queue.shutdown();
    gate.allow(1);
    decoder.join().unwrap();

    let stats = lock(&shared.receiver).stats();
    assert_eq!(stats.frames_completed, ((ROUNDS - 1) * ROUND + 1) as u64);
    assert_eq!(stats.frame_drops, 0);
    assert_eq!(
        stats.packets_recovered,
        (ROUNDS - 1) as u64,
        "one per round"
    );
    assert_eq!(shared.counters.queue_overflows.load(Ordering::Relaxed), 0);
    assert_eq!(shared.counters.chunks_dropped.load(Ordering::Relaxed), 0);
    allocations
}

#[test]
fn h264_streaming_allocates_nothing_per_packet_or_frame() {
    static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
    let mut host = Host::new();
    // A key frame with prefix SEI, which the depacketizer holds back behind the parameter sets.
    let mut key = vec![0, 0, 0, 1, 0x06, 0x05, 0x04, 0x01, 0x02, 0x80];
    key.extend(h264_idr());
    let p = h264_p(4000);
    let rounds: Rounds = (0..ROUNDS)
        .map(|_| {
            (0..ROUND)
                .map(|n| match n {
                    0 => host.frame(&key, 2),
                    1 => {
                        // A lost data packet, recovered from parity.
                        let mut datagrams = host.frame(&p, 1);
                        datagrams.remove(1);
                        datagrams
                    }
                    _ => host.frame(&p, 1),
                })
                .collect()
        })
        .collect();
    let receiver = VideoReceiver::new(config(0x1));
    let allocations = allocations_in_steady_streaming(&ALLOCATIONS, receiver, &rounds);
    assert_eq!(
        allocations, 0,
        "allocations in {MEASURED} rounds of {ROUND} frames"
    );
}

#[test]
fn pyrowave_early_chunks_allocate_nothing_per_packet_or_frame() {
    static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
    let mut host = Blocks::new();
    let rounds: Rounds = (0..ROUNDS)
        .map(|_| {
            (0..ROUND)
                .map(|n| {
                    let mut blocks = host.frame(&[12, 9], n as u8);
                    if n == 1 {
                        // A lost data packet, recovered from parity.
                        blocks[0].remove(4);
                    }
                    blocks.concat()
                })
                .collect()
        })
        .collect();
    let allocations = allocations_in_steady_streaming(&ALLOCATIONS, records(true), &rounds);
    assert_eq!(
        allocations, 0,
        "allocations in {MEASURED} rounds of {ROUND} frames"
    );
}
