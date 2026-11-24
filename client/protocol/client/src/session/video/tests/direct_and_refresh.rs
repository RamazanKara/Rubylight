//! Direct-submit decoder, RFI recovery fallback, key-frame request pacing and arrival
//! feedback tests.

use std::collections::VecDeque;

use super::*;

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
    let mut direct = Direct::new(
        Box::new(DirectFake {
            shared: shared.clone(),
            log: log.clone(),
            status: DecodeStatus::NeedIdr,
        }),
        KeyFrameRequests::new(60),
    );
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

#[test]
fn a_failing_decoder_asks_for_one_key_frame_per_window_and_a_decoded_one_clears_it() {
    // 120 fps: a 100 ms round trip plus four frame intervals.
    let mut requests = KeyFrameRequests::new(120);
    assert_eq!(requests.window_us, 133_333);
    // The first failure asks at once.
    assert!(requests.request(true, || 1_000));
    // A burst of failures within the window asks no more, and frames the decoder took
    // meanwhile do not either.
    for now in (2_000..134_000).step_by(1_000) {
        assert!(!requests.request(true, || now), "at {now} us");
        assert!(!requests.request(false, || now), "at {now} us");
    }
    // The deferred request goes out with the first frame after the window, even one taken.
    assert!(requests.request(false, || 134_400));
    assert!(
        !requests.request(false, || 300_000),
        "nothing waits any more"
    );
    // A decoded key frame answers it: a deferred request is dropped, and the next failure
    // asks at once although it is within the window.
    assert!(!requests.request(true, || 140_000));
    requests.key_frame_decoded();
    assert!(!requests.request(false, || 140_001));
    assert!(requests.request(true, || 140_002));
    // An unknown frame rate counts as 60 fps.
    assert_eq!(KeyFrameRequests::new(0).window_us, 166_666);
}

/// A decoder that fails every frame but `ok`, logging its submissions.
struct FailingDecoder {
    log: Arc<Mutex<Vec<Option<u32>>>>,
    ok: u32,
}

impl VideoDecoder for FailingDecoder {
    fn setup(&mut self, _: &crate::session::VideoSetup) -> Result<(), i32> {
        Ok(())
    }

    fn submit(&mut self, frame: &Frame) -> DecodeStatus {
        lock(&self.log).push(Some(frame.number));
        if frame.number == self.ok {
            DecodeStatus::Ok
        } else {
            DecodeStatus::NeedIdr
        }
    }
}

#[test]
fn a_decoder_failing_every_key_frame_does_not_flood_the_host_with_requests() {
    // Issue 77: the decoder failed every frame, the asked-for key frames too, and each failure
    // asked for another, one key frame every frame or two. A window of seconds (1 fps) keeps
    // the burst within it however slowly the test runs.
    let shared = Arc::new(VideoShared::new(records(false)));
    let log: Arc<Mutex<Vec<Option<u32>>>> = Arc::default();
    let (done, routed) = mpsc::channel();
    let decoder = FailingDecoder {
        log: log.clone(),
        ok: 21,
    };
    let (for_thread, for_route) = (shared.clone(), log.clone());
    let thread = std::thread::spawn(move || {
        decode_loop(
            Box::new(decoder),
            &for_thread,
            KeyFrameRequests::new(1),
            move |signals| {
                for signal in signals.drain(..) {
                    if matches!(signal, VideoSignal::RequestIdr) {
                        lock(&for_route).push(None);
                    }
                }
                done.send(()).unwrap();
            },
        )
    });
    // One frame at a time, once the decoder thread is done with the one before, as the host's
    // answers arrive: each request flushes the queue. Frames 1 to 20 are key frames the decoder fails, 21 is one it decodes, 22 and 23 fail.
    for number in 1..=23 {
        let frame_type = if number <= 21 {
            FrameType::Idr
        } else {
            FrameType::P
        };
        assert!(shared.queue.offer(numbered(number, frame_type)).is_ok());
        routed
            .recv_timeout(HANG_GUARD)
            .expect("the decoder never took the frame");
    }
    shared.queue.shutdown();
    thread.join().unwrap();
    // A request (None) after the first failure and after the first failure that follows the
    // decoded key frame, none for the burst between.
    let mut expected: Vec<Option<u32>> = vec![Some(1), None];
    expected.extend((2..=22).map(Some));
    expected.extend([None, Some(23)]);
    assert_eq!(*lock(&log), expected);
    assert_eq!(shared.counters.need_idr.load(Ordering::Relaxed), 22);
}

/// The window of the tests whose held request must go out without another frame.
const SHORT_WINDOW: Duration = Duration::from_millis(400);

#[test]
fn a_held_request_goes_out_after_its_window_without_another_frame() {
    // A failed key frame and then nothing: a still picture the host does not resend. The
    // request it held must still go out, or the app's switch to the next decoder never comes.
    let shared = Arc::new(VideoShared::new(records(false)));
    let log: Arc<Mutex<Vec<Option<u32>>>> = Arc::default();
    let requests: Arc<Mutex<Vec<Instant>>> = Arc::default();
    let (done, routed) = mpsc::channel();
    let decoder = FailingDecoder {
        log: log.clone(),
        ok: 0,
    };
    let (for_thread, for_route, sent) = (shared.clone(), log.clone(), requests.clone());
    let thread = std::thread::spawn(move || {
        decode_loop(
            Box::new(decoder),
            &for_thread,
            KeyFrameRequests::with_window_us(SHORT_WINDOW.as_micros() as u64),
            move |signals| {
                for signal in signals.drain(..) {
                    if matches!(signal, VideoSignal::RequestIdr) {
                        lock(&for_route).push(None);
                        lock(&sent).push(Instant::now());
                    }
                }
                done.send(()).unwrap();
            },
        )
    });
    for number in 1..=2 {
        assert!(shared.queue.offer(numbered(number, FrameType::Idr)).is_ok());
        routed
            .recv_timeout(HANG_GUARD)
            .expect("the decoder never took the frame");
    }
    assert_eq!(*lock(&log), [Some(1), None, Some(2)], "the second waits");
    // The thread routes the held request when its wait times out.
    routed
        .recv_timeout(HANG_GUARD)
        .expect("the held request never went out");
    shared.queue.shutdown();
    thread.join().unwrap();
    assert_eq!(*lock(&log), [Some(1), None, Some(2), None]);
    let sent = lock(&requests);
    assert!(sent[1] - sent[0] >= SHORT_WINDOW, "{:?}", sent[1] - sent[0]);
}

/// A direct decoder that answers with `statuses` in turn.
struct ScriptedDirect {
    statuses: VecDeque<DecodeStatus>,
}

impl VideoDecoder for ScriptedDirect {
    fn setup(&mut self, _: &crate::session::VideoSetup) -> Result<(), i32> {
        Ok(())
    }

    fn submit(&mut self, _: &Frame) -> DecodeStatus {
        self.statuses.pop_front().expect("a status for each frame")
    }
}

/// Feeds the PyroWave frames (each a key frame) `numbers` to the direct decoder one at a
/// time, and returns the key frame requests each caused.
fn feed_direct(
    shared: &VideoShared,
    direct: &mut Direct,
    host: &mut Blocks,
    numbers: &[u8],
) -> Vec<usize> {
    let mut signals = Vec::new();
    numbers
        .iter()
        .map(|&number| {
            let packets = flat(host.frame(&[8, 6], number));
            let mut requests = 0;
            receive_batch(
                shared,
                Some(&mut *direct),
                &mut signals,
                packets.len(),
                |i| packets[i].as_slice(),
                &mut None,
                &mut StagedChunks::default(),
                |signals| {
                    requests += signals
                        .drain(..)
                        .filter(|s| matches!(s, VideoSignal::RequestIdr))
                        .count();
                },
            );
            requests
        })
        .collect()
}

fn scripted_direct(statuses: &[DecodeStatus], refresh: KeyFrameRequests) -> Direct {
    Direct::new(
        Box::new(ScriptedDirect {
            statuses: statuses.iter().copied().collect(),
        }),
        refresh,
    )
}

#[test]
fn a_direct_decoder_failing_in_a_burst_asks_once_and_a_decoded_key_frame_clears_it() {
    use DecodeStatus::{NeedIdr, Ok as Decoded};
    let shared = Arc::new(VideoShared::new(records(false)));
    // A window of seconds (1 fps): the burst stays within it however slowly the test runs.
    let statuses = [NeedIdr, NeedIdr, NeedIdr, Decoded, NeedIdr, NeedIdr];
    let mut direct = scripted_direct(&statuses, KeyFrameRequests::new(1));
    let requests = feed_direct(
        &shared,
        &mut direct,
        &mut Blocks::new(),
        &[1, 2, 3, 4, 5, 6],
    );
    assert_eq!(requests, [1, 0, 0, 0, 1, 0]);
    assert_eq!(shared.counters.need_idr.load(Ordering::Relaxed), 5);
}

#[test]
fn a_direct_decoders_held_request_goes_out_from_the_receive_poll() {
    use DecodeStatus::NeedIdr;
    let shared = Arc::new(VideoShared::new(records(false)));
    let window = KeyFrameRequests::with_window_us(SHORT_WINDOW.as_micros() as u64);
    let mut direct = scripted_direct(&[NeedIdr, NeedIdr], window);
    let first = Instant::now();
    let requests = feed_direct(&shared, &mut direct, &mut Blocks::new(), &[1, 2]);
    assert_eq!(requests, [1, 0], "the second waits");
    // No more datagrams: the receive thread's poll timeout asks, and sends once it is due.
    let mut signals = Vec::new();
    wait_until("the held request never went out", || {
        direct.send_held(&shared, &mut signals)
    });
    assert!(first.elapsed() >= SHORT_WINDOW);
    assert!(
        matches!(signals[..], [VideoSignal::RequestIdr]),
        "{signals:?}"
    );
    assert!(!direct.send_held(&shared, &mut signals), "sent once");
}

#[test]
fn each_datagram_of_a_batch_reports_its_own_arrival_to_the_feedback() {
    let shared = VideoShared::new(records(false));
    lock(&shared.receiver).enable_transport_feedback();
    // Two RTP-sized datagrams of consecutive sequence numbers, and one 60 ms on that closes the
    // report.
    let datagram = |seq: u16| {
        let mut d = vec![0u8; 16];
        d[2..4].copy_from_slice(&seq.to_be_bytes());
        d
    };
    let packets = [datagram(0x10), datagram(0x11), datagram(0x12)];
    let arrivals = [Some(1_000), Some(1_400), Some(61_000)];
    let mut staged = StagedChunks::default();
    let mut signals = Vec::new();
    let mut reports = Vec::new();
    receive_batch_stamped(
        &shared,
        None,
        &mut signals,
        packets.len(),
        |i| packets[i].as_slice(),
        |i| arrivals[i],
        &mut None,
        &mut staged,
        |signals| {
            for signal in signals.drain(..) {
                if let VideoSignal::TransportFeedback(report) = signal {
                    reports.push(report);
                }
            }
        },
    );
    assert_eq!(reports.len(), 1, "{reports:?}");
    let report = &reports[0];
    assert_eq!(report.base_arrival_us, 1_000);
    // 4 µs ticks: the second datagram arrived 400 µs after the first.
    assert_eq!(report.entries[..2], [0, 100], "{report:?}");
}
