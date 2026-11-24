//! Unit tests for FFmpeg frame feeding, in-flight timing, backend fallback and presenter queue wakeups.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::Mutex;

use crate::media::Received;
use super::super::ffmpeg::*;
use super::super::*;
use super::{decoder, notices};

/// A fake FFmpeg decoder. Synchronous, each frame's picture comes out at the first receive
/// after its send; asynchronous, as FFmpeg's V4L2 M2M decoder: a frame's picture is ready
/// only after `ready_after` more receives (the driver's decode time), "no picture yet" until
/// then, and with `queue` frames in the driver a send says EAGAIN, the pts rescaled as FFmpeg
/// rescales it.
struct FakeDecoder {
    asynchronous: bool,
    ready_after: u32,
    queue: usize,
    /// Sent frames: pts and receives left before the picture is ready.
    pending: VecDeque<(i64, u32)>,
    /// Frames that make no picture, and frames whose picture is damaged.
    missing: Vec<u32>,
    corrupt: Vec<u32>,
}

#[derive(Debug, PartialEq)]
struct FakePicture(i64, bool);

impl DecodedPicture for FakePicture {
    fn pts(&self) -> i64 { self.0 }
    fn corrupt(&self) -> bool { self.1 }
}

impl FakeDecoder {
    fn synchronous() -> Self {
        Self { asynchronous: false, ready_after: 0, queue: usize::MAX, pending: VecDeque::new(), missing: Vec::new(), corrupt: Vec::new() }
    }
    fn v4l2(ready_after: u32, queue: usize) -> Self {
        Self { asynchronous: true, ready_after, queue, ..Self::synchronous() }
    }
}

impl PictureDecoder for FakeDecoder {
    type Picture = FakePicture;
    fn send(&mut self, _: &[u8], pts: i64) -> i32 {
        if self.pending.len() >= self.queue {
            return media::AGAIN;
        }
        if !self.missing.contains(&((pts >> 32) as u32)) {
            // V4L2 M2M carries the pts through the driver in microseconds (`media::packet_timebase`).
            let pts = if self.asynchronous { media::v4l2_pts_round_trip(pts, media::packet_timebase(Hardware::V4l2m2m)) } else { pts };
            self.pending.push_back((pts, self.ready_after));
        }
        0
    }
    fn receive(&mut self) -> Received<FakePicture> {
        let Some((pts, left)) = self.pending.front_mut() else { return Received::None };
        if *left > 0 {
            *left -= 1;
            return Received::None;
        }
        let pts = *pts;
        self.pending.pop_front();
        Received::Frame(FakePicture(pts, self.corrupt.contains(&((pts >> 32) as u32))))
    }
    fn asynchronous(&self) -> bool { self.asynchronous }
}

fn pts(number: u32) -> i64 {
    (i64::from(number) << 32) | i64::from(number * 1500)
}

/// Feeds frame `number` with `wait_us`, returning what it did, the pictures that came out and
/// how often it waited. Each wait takes [`ASYNC_POLL_US`], everything else no time.
fn feed_frame(decoder: &mut FakeDecoder, number: u32, wait_us: u64) -> (FedFrame, Vec<FakePicture>, u32) {
    let (mut pictures, polls) = (Vec::new(), std::cell::Cell::new(0u32));
    let fed = feed(decoder, &[0; 4], pts(number), wait_us, || u64::from(polls.get()) * ASYNC_POLL_US,
        || polls.set(polls.get() + 1), |picture| pictures.push(picture));
    (fed, pictures, polls.get())
}

#[test]
fn an_asynchronous_decoder_hands_over_each_frames_picture_with_that_frame_not_the_next() {
    // Without waiting, V4L2 M2M's picture of frame N came out only at frame N+1's send.
    let mut late = FakeDecoder::v4l2(3, 4);
    let (fed, pictures, _) = feed_frame(&mut late, 1, 0);
    assert!(pictures.is_empty() && !fed.own);
    let (_, pictures, _) = feed_frame(&mut late, 2, 0);
    assert!(pictures.is_empty(), "nothing is ready at the next send either without asking again");
    // Asked again while the frame is in flight, the picture comes out with its own frame.
    let mut decoder = FakeDecoder::v4l2(3, 4);
    for number in 1..=5 {
        let (fed, pictures, polls) = feed_frame(&mut decoder, number, async_wait_us(60_000));
        assert_eq!(pictures, [FakePicture(pts(number), false)], "frame {number}");
        assert!(fed.own && polls == 3, "frame {number}: {fed:?}, {polls} polls");
        assert_eq!(frame_status(&fed, true, true), DecodeStatus::Ok);
    }
}

#[test]
fn an_asynchronous_decoder_is_waited_for_no_longer_than_its_bound_and_the_picture_comes_later() {
    let mut decoder = FakeDecoder::v4l2(1_000, 4);
    let wait_us = async_wait_us(60_000);
    let (fed, pictures, polls) = feed_frame(&mut decoder, 1, wait_us);
    assert!(pictures.is_empty() && u64::from(polls) == wait_us / ASYNC_POLL_US);
    // In flight is not a failure for an asynchronous decoder.
    assert_eq!(frame_status(&fed, true, true), DecodeStatus::Ok);
    // Half a frame, never more than 8 ms.
    assert_eq!(async_wait_us(60_000), 8_000);
    assert_eq!(async_wait_us(120_000), 4_166);
    assert_eq!(async_wait_us(240_000), 2_083);
}

/// Reading after EAGAIN and waiting for the frame's own picture each had the whole wait, so
/// a frame could hold the decoder thread for twice its bound.
#[test]
fn reading_after_eagain_and_waiting_for_the_picture_share_one_deadline() {
    // The driver takes one frame and needs 20 polls (5 ms) for each.
    let mut decoder = FakeDecoder::v4l2(20, 1);
    let wait_us = async_wait_us(60_000);
    feed_frame(&mut decoder, 1, 0);
    let (fed, pictures, polls) = feed_frame(&mut decoder, 2, wait_us);
    assert_eq!(pictures, [FakePicture(pts(1), false)], "frame 1's picture after EAGAIN");
    assert!(!fed.own && !fed.refused && !fed.failed, "{fed:?}");
    assert_eq!(u64::from(polls), wait_us / ASYNC_POLL_US, "8 ms in all, not 5 ms and then 8 more");
    assert_eq!(frame_status(&fed, true, true), DecodeStatus::Ok);
}

#[test]
fn eagain_on_send_reads_the_ready_pictures_and_sends_the_frame_again_without_asking_for_a_key_frame() {
    // The driver takes one frame: the next send says EAGAIN until its picture is read.
    let mut decoder = FakeDecoder::v4l2(2, 1);
    let (_, pictures, _) = feed_frame(&mut decoder, 1, 0);
    assert!(pictures.is_empty());
    let (fed, pictures, _) = feed_frame(&mut decoder, 2, async_wait_us(60_000));
    assert!(!fed.refused && !fed.failed, "{fed:?}");
    assert_eq!(pictures, [FakePicture(pts(1), false), FakePicture(pts(2), false)]);
    assert_eq!(frame_status(&fed, true, true), DecodeStatus::Ok);
    // A synchronous decoder that says EAGAIN gets the frame again after its pictures are read.
    let mut decoder = FakeDecoder { queue: 1, ..FakeDecoder::synchronous() };
    decoder.pending.push_back((pts(1), 0));
    let (fed, pictures, polls) = feed_frame(&mut decoder, 2, 8_000);
    assert_eq!(pictures, [FakePicture(pts(1), false), FakePicture(pts(2), false)]);
    assert_eq!((fed.refused, polls), (false, 0), "a synchronous decoder is never waited for");
}

#[test]
fn a_synchronous_frame_without_a_picture_or_with_a_damaged_one_asks_for_recovery() {
    let mut decoder = FakeDecoder::synchronous();
    decoder.missing = vec![1, 3];
    decoder.corrupt = vec![4];
    // Before the decoder's first picture a frame without one is not a failure.
    let (fed, _, _) = feed_frame(&mut decoder, 1, 0);
    assert_eq!(frame_status(&fed, false, false), DecodeStatus::Ok);
    let (fed, pictures, _) = feed_frame(&mut decoder, 2, 0);
    assert_eq!((pictures.len(), frame_status(&fed, false, false)), (1, DecodeStatus::Ok));
    let (fed, pictures, _) = feed_frame(&mut decoder, 3, 0);
    assert!(pictures.is_empty());
    assert_eq!(frame_status(&fed, false, true), DecodeStatus::NeedIdr, "steady, a frame made no picture");
    let (fed, pictures, _) = feed_frame(&mut decoder, 4, 0);
    assert_eq!(pictures, [FakePicture(pts(4), true)], "the damaged picture is still shown");
    assert_eq!(frame_status(&fed, false, true), DecodeStatus::NeedIdr);
    let (fed, _, _) = feed_frame(&mut decoder, 5, 0);
    assert_eq!(frame_status(&fed, false, true), DecodeStatus::Ok);
    // The asynchronous decoder's frames are not judged that way.
    assert_eq!(frame_status(&FedFrame { corrupt: 1, pictures: 1, ..FedFrame::default() }, true, true), DecodeStatus::Ok);
    // Failures stay failures for both.
    for asynchronous in [false, true] {
        assert_eq!(frame_status(&FedFrame { refused: true, ..FedFrame::default() }, asynchronous, false), DecodeStatus::NeedIdr);
        assert_eq!(frame_status(&FedFrame { failed: true, pictures: 1, ..FedFrame::default() }, asynchronous, false), DecodeStatus::NeedIdr);
    }
}

#[test]
fn a_picture_takes_the_times_of_its_own_frame_and_drops_those_of_frames_without_one() {
    let times = |number: u32| InFlight { number, receive_us: u64::from(number) * 10, enqueue_us: 0, input_us: 0, host_latency: 0 };
    let mut in_flight: VecDeque<_> = [7, 8, 9].map(times).into();
    assert_eq!(take_in_flight(&mut in_flight, 8), Some(times(8)));
    assert_eq!(in_flight, [times(9)], "frame 7 made no picture and its times are dropped");
    assert_eq!(take_in_flight(&mut in_flight, 6), None, "an earlier frame's picture keeps the later times");
    assert_eq!(in_flight.len(), 1);
    // Across the frame number's wrap.
    let mut in_flight: VecDeque<_> = [u32::MAX - 1, u32::MAX, 0, 1].map(times).into();
    assert_eq!(take_in_flight(&mut in_flight, 0), Some(times(0)));
    assert_eq!(in_flight, [times(1)]);
}

#[test]
fn a_refused_backend_goes_on_with_the_ones_after_it() {
    let gpu = std::ptr::dangling_mut::<c_void>();
    let order = vec![Hardware::Vulkan(gpu), Hardware::Vaapi, Hardware::V4l2m2m, Hardware::Cuda];
    assert_eq!(backends_after(order.clone(), Hardware::Vulkan(gpu)), [Hardware::Vaapi, Hardware::V4l2m2m, Hardware::Cuda]);
    assert!(backends_after(order.clone(), Hardware::Cuda).is_empty());
    assert!(backends_after(vec![Hardware::D3d11(gpu)], Hardware::D3d11(gpu)).is_empty(), "D3D11VA goes to software");
    assert!(backends_after(order, Hardware::None).is_empty());
}

/// A VAAPI stream on the GPU renderer whose pictures cannot be mapped into its Vulkan images
/// goes to Vulkan video on its device, not through memory picture by picture.
#[test]
fn unmapped_vaapi_pictures_go_to_vulkan_video_where_the_order_has_it_next() {
    let gpu = std::ptr::dangling_mut::<c_void>();
    let order = || vec![Hardware::Vaapi, Hardware::Vulkan(gpu), Hardware::V4l2m2m, Hardware::Cuda];
    let unmapped = Unmapped { vaapi: true, zero_copy: false, packed: false, map_switched_on: true, switch: None };
    let never = |_| false;
    assert_eq!(vulkan_for_unmapped(unmapped, order(), never), Some(Hardware::Vulkan(gpu)));
    assert_eq!(vulkan_for_unmapped(Unmapped { zero_copy: true, ..unmapped }, order(), never), None, "mapped, or DMA-BUFs: nothing to save");
    assert_eq!(vulkan_for_unmapped(Unmapped { vaapi: false, ..unmapped }, order(), never), None, "another decoder's pictures");
    assert_eq!(vulkan_for_unmapped(Unmapped { packed: true, ..unmapped }, order(), never), None, "Vulkan video leaves packed full chroma");
    // FFmpeg took Vulkan video to software for this stream before: VAAPI keeps it.
    assert_eq!(vulkan_for_unmapped(unmapped, order(), |backend| backend == Hardware::Vulkan(gpu)), None);
    // Vulkan video first: it did not open, or was refused, before VAAPI was tried.
    assert_eq!(vulkan_for_unmapped(unmapped, vec![Hardware::Vulkan(gpu), Hardware::Vaapi, Hardware::V4l2m2m, Hardware::Cuda], never), None);
    // Another renderer has no device for it, and NIGHTJAR_VULKAN_VIDEO=0 leaves it out.
    let null = std::ptr::null_mut();
    assert_eq!(vulkan_for_unmapped(unmapped, vec![Hardware::Vaapi, Hardware::Vulkan(null), Hardware::V4l2m2m, Hardware::Cuda], never), None);
    assert_eq!(vulkan_for_unmapped(unmapped, vec![Hardware::Vaapi, Hardware::V4l2m2m, Hardware::Cuda], never), None);
    // The switches: 0 keeps memory; the map's own 0 asks for memory unless this one says 1.
    assert_eq!(vulkan_for_unmapped(Unmapped { switch: Some(false), ..unmapped }, order(), never), None);
    let map_off = Unmapped { map_switched_on: false, ..unmapped };
    assert_eq!(vulkan_for_unmapped(map_off, order(), never), None, "NIGHTJAR_VAAPI_VULKAN_MAP=0 alone is the through-memory A/B");
    assert_eq!(vulkan_for_unmapped(Unmapped { switch: Some(true), ..map_off }, order(), never), Some(Hardware::Vulkan(gpu)));
}

/// Vulkan video that stood in for unmappable VAAPI and that FFmpeg then took to software
/// hands the stream back to VAAPI, for the format it stood in for only.
#[test]
fn vulkan_video_refused_after_standing_in_hands_the_stream_back_to_vaapi() {
    let gpu = std::ptr::dangling_mut::<c_void>();
    assert!(stood_in_for_vaapi(Hardware::Vulkan(gpu), Some(7), 7));
    assert!(!stood_in_for_vaapi(Hardware::Vulkan(gpu), None, 7), "Vulkan video in its own turn of the order");
    assert!(!stood_in_for_vaapi(Hardware::Vulkan(gpu), Some(3), 7), "another format's stand-in");
    assert!(!stood_in_for_vaapi(Hardware::Cuda, Some(7), 7));
}

/// A frame after which FFmpeg turns out to have taken the decoder to software renames it, says
/// so, opens the next decoder and asks for a key frame.
#[test]
fn a_refused_decoder_is_replaced_and_said_on_the_frame_that_shows_it() {
    let (mut decoder, shared) = decoder();
    let setup = VideoSetup { format: nightjar_core::sdp::VIDEO_FORMAT_H264, width: 128, height: 128, fps: 60, chroma_packing: 0, logical_width: 128, logical_height: 128, color_space: 1, color_range: 0 };
    decoder.setup(&setup).unwrap();
    assert!(notices(&shared).is_empty());
    decoder.decoder.as_mut().expect("the software decoder").refuse();
    assert_eq!(decoder.decode_frame(&Frame::default()), DecodeStatus::NeedIdr);
    assert!(matches!(notices(&shared).as_slice(), [Notice::DecoderFallback(_)]), "the fallback is said");
    assert_eq!(decoder.refused, [(Hardware::None, setup.format)]);
    let replaced = decoder.decoder.as_ref().expect("a decoder after the refusal");
    assert!(!replaced.refused(), "a new decoder, not the refused one");
    // The overlay names the decoder that now decodes (the refused one was named on the
    // frame that showed it, before the replacement opened).
    assert_eq!(*lock(&shared.decoder_name), replaced.name);
}

/// A packed 4:4:4 stream cannot be unpacked by the software decoder: when FFmpeg takes its
/// hardware decoder to software, the stream ends with the hardware-needed reason instead of
/// decoding garbled; an unpacked stream still goes on in software.
#[test]
fn a_refused_hardware_decoder_never_hands_a_packed_stream_to_software() {
    for (chroma_packing, falls_back) in [(2, false), (0, true)] {
        let (mut decoder, shared) = decoder();
        let setup = VideoSetup { format: nightjar_core::sdp::VIDEO_FORMAT_H264, width: 128, height: 128, fps: 60, chroma_packing, logical_width: 128, logical_height: 128, color_space: 1, color_range: 0 };
        decoder.setup = Some(setup);
        decoder.hardware = Hardware::V4l2m2m;
        decoder.replace_refused(Hardware::V4l2m2m);
        let said = notices(&shared);
        if falls_back {
            assert!(matches!(said.as_slice(), [Notice::DecoderFallback(_)]), "the switch is said, the stream goes on");
            assert_eq!(decoder.decoder.as_ref().expect("a software decoder").hardware, Hardware::None);
        } else {
            assert!(matches!(said.as_slice(), [Notice::DecoderFailed(text)] if *text == crate::strings::tr(PACKED_NEEDS_HARDWARE)), "the hardware-needed failure");
            assert!(decoder.decoder.is_none(), "no software decoder under a packed stream");
        }
    }
}

/// Pictures the presenter could not show from the software decoder end the stream with a reason
/// in the user's words; pictures from before the decoder was installed say nothing about it.
#[test]
fn pictures_the_presenter_cannot_show_from_software_end_the_stream_saying_why() {
    let (mut decoder, shared) = decoder();
    let setup = VideoSetup { format: nightjar_core::sdp::VIDEO_FORMAT_H264, width: 128, height: 128, fps: 60, chroma_packing: 0, logical_width: 128, logical_height: 128, color_space: 1, color_range: 0 };
    decoder.setup(&setup).unwrap();
    assert!(!decoder.presenter_refused(), "nothing refused");
    shared.present_refused.store(decoder.installed_us.saturating_sub(1).max(1), Ordering::Relaxed);
    if decoder.installed_us > 1 {
        assert!(!decoder.presenter_refused(), "an older decoder's picture");
        assert!(notices(&shared).is_empty());
    }
    shared.present_refused.store(session::now_us().max(1), Ordering::Relaxed);
    assert!(decoder.presenter_refused());
    let ended = notices(&shared);
    let [Notice::DecoderFailed(text)] = ended.as_slice() else { panic!("expected exactly the DecoderFailed notice") };
    assert_eq!(text, crate::strings::tr(PICTURES_UNSHOWN));
    assert_eq!(shared.present_refused.load(Ordering::Relaxed), 0, "taken");
}

/// NVDEC pictures that go through memory after all, because CUDA's copy into the renderer's
/// Vulkan images failed, are said in a toast once, on the frame that shows it.
// Linux/macOS: fail_cuda_copy is only built on Unix targets.
#[cfg(unix)]
#[test]
fn a_failed_cuda_copy_is_said_once_in_a_toast() {
    let (mut decoder, shared) = decoder();
    let setup = VideoSetup { format: nightjar_core::sdp::VIDEO_FORMAT_H264, width: 128, height: 128, fps: 60, chroma_packing: 0, logical_width: 128, logical_height: 128, color_space: 1, color_range: 0 };
    decoder.setup(&setup).unwrap();
    assert!(notices(&shared).is_empty());
    decoder.decoder.as_mut().expect("the software decoder").fail_cuda_copy("CUDA error 999");
    decoder.decode_frame(&Frame::default());
    let said = notices(&shared);
    assert!(matches!(said.as_slice(), [Notice::DecoderFallback(text)] if text == crate::strings::tr(CUDA_FALLBACK_TOAST)), "{}", said.len());
    decoder.decode_frame(&Frame::default());
    assert!(notices(&shared).is_empty(), "said once");
}

/// A refused decoder replaced mid-stream by a backend that never asked for reference frame
/// invalidation makes the session recover with key frames for the rest of the stream.
#[test]
fn a_fallback_to_a_backend_without_rfi_turns_rfi_recovery_off() {
    let gpu = std::ptr::dangling_mut::<c_void>();
    for backend in [Hardware::D3d11(gpu), Hardware::Vaapi, Hardware::None] {
        assert!(recovers_with_rfi(Some(backend)), "{}", backend.label());
    }
    for backend in [Hardware::Vulkan(gpu), Hardware::V4l2m2m, Hardware::Cuda] {
        assert!(!recovers_with_rfi(Some(backend)), "{}", backend.label());
    }
    assert!(recovers_with_rfi(None), "PyroWave, or no decoder yet");
    let (decoder, _) = decoder();
    assert!(VideoDecoder::recovers_with_rfi(&decoder));
}

/// The presenter's waker counts: what `Shared::queue` wakes it for.
fn counting_shared(limit: usize) -> (Arc<Shared>, Arc<std::sync::atomic::AtomicUsize>) {
    let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = wakes.clone();
    (Shared::new(Arc::new(move || { counted.fetch_add(1, Ordering::SeqCst); }), limit), wakes)
}

#[test]
fn the_presenter_is_woken_when_a_picture_finds_the_queue_empty_not_for_each_picture() {
    use super::super::super::present::tests::decoded;
    let (shared, wakes) = counting_shared(2);
    shared.queue(decoded(1));
    assert_eq!(wakes.load(Ordering::SeqCst), 1, "the first picture wakes it");
    // It has not taken them yet: its wake is on its way, and it takes all of them.
    shared.queue(decoded(2));
    shared.queue(decoded(3));
    assert_eq!(wakes.load(Ordering::SeqCst), 1, "pictures joining the queue add no wake");
    assert_eq!(lock(&shared.frames).iter().map(|d| d.ready_us).collect::<Vec<_>>(), [2, 3], "the queue is still bounded");
    // The presenter takes the queue (`Streaming::pump`); the next picture wakes it again.
    lock(&shared.frames).clear();
    shared.queue(decoded(4));
    assert_eq!(wakes.load(Ordering::SeqCst), 2);
    // A queue the decoder emptied itself (`evict_oldest` on the last picture) is empty too.
    assert!(shared.evict_oldest());
    shared.queue(decoded(5));
    assert_eq!(wakes.load(Ordering::SeqCst), 3);
}

/// A presenter that sleeps until woken and then takes everything queued, as `Streaming::pump` and
/// its loop do, never leaves a picture behind while it sleeps: no wake is lost to the pictures
/// that did not send one.
#[test]
fn a_presenter_that_sleeps_between_wakes_misses_no_picture() {
    use super::super::super::present::tests::decoded;
    use std::sync::{Condvar, atomic::AtomicBool};
    const PICTURES: u64 = 2_000;
    let signal = Arc::new((Mutex::new(false), Condvar::new()));
    let woken = signal.clone();
    let shared = Shared::new(Arc::new(move || {
        *lock(&woken.0) = true;
        woken.1.notify_one();
    }), usize::MAX);
    let done = Arc::new(AtomicBool::new(false));
    let decoder = {
        let (shared, done) = (shared.clone(), done.clone());
        std::thread::spawn(move || {
            for i in 0..PICTURES {
                shared.queue(decoded(i));
                if i % 64 == 0 { std::thread::yield_now(); }
            }
            done.store(true, Ordering::SeqCst);
        })
    };
    let (mut taken, mut sleeps) = (0u64, 0u64);
    let mut mine = VecDeque::new();
    while taken < PICTURES {
        // The loop's wait: asleep until a wake; the timeout is the failure, not a poll.
        {
            let (flag, condvar) = &*signal;
            let mut woke = lock(flag);
            while !*woke {
                let (next, timeout) = condvar.wait_timeout(woke, std::time::Duration::from_secs(10)).unwrap();
                woke = next;
                assert!(!timeout.timed_out(), "asleep with {} of {PICTURES} pictures taken, queue {}, decoder done {}",
                    taken, lock(&shared.frames).len(), done.load(Ordering::SeqCst));
            }
            *woke = false;
            sleeps += 1;
        }
        std::mem::swap(&mut *lock(&shared.frames), &mut mine);
        taken += mine.len() as u64;
        mine.clear();
    }
    decoder.join().unwrap();
    assert_eq!(taken, PICTURES);
    assert!(sleeps <= PICTURES);
}
