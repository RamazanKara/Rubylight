//! Unit tests for stream pipeline decoder lifecycle, notices, failure escalation and audio setup.

use std::sync::atomic::AtomicU64;

use nightjar_core::session::{AudioSetup, Listener};
use super::ffmpeg::*;
use super::*;

mod feed_and_queue;

fn decoder() -> (Decoder, Arc<Shared>) {
    let shared = Shared::new(Arc::new(|| {}), 8);
    (Decoder::new(shared.clone(), Hardware::None), shared)
}

fn notices(shared: &Shared) -> Vec<Notice> {
    std::mem::take(&mut *lock(&shared.notices))
}

fn audio_setup(codec: u8) -> AudioSetup {
    AudioSetup {
        configuration: nightjar_core::sdp::AudioConfiguration::STEREO,
        opus: nightjar_core::rtsp::OpusConfig::default(),
        codec,
        packet_duration_ms: 5,
        samples_per_frame: 240,
    }
}

/// A resume or reconnect opens its own session: when the stream before ended into it, the
/// decoder is kept and the next session takes it back, parameter for parameter.
#[test]
fn a_reconnect_takes_the_decoder_the_stream_before_kept() {
    let _serial = super::super::decoder_cache::serialize();
    let setup = VideoSetup { format: nightjar_core::sdp::VIDEO_FORMAT_H264, width: 96, height: 64, fps: 60, chroma_packing: 0, logical_width: 96, logical_height: 64, color_space: 1, color_range: 0 };
    let (mut first, shared) = decoder();
    first.setup(&setup).unwrap();
    shared.keep_decoder.store(true, Ordering::Relaxed);
    first.cleanup();
    let before = super::super::decoder_cache::TAKEN.load(Ordering::Relaxed);
    let (mut second, _) = decoder();
    second.setup(&setup).unwrap();
    assert_eq!(super::super::decoder_cache::TAKEN.load(Ordering::Relaxed), before + 1, "the next stream took the kept decoder");
    assert_eq!(second.decoder.as_ref().map(|d| d.hardware), Some(Hardware::None));
    assert!(second.setup.is_some());
}

/// A stream whose codec, format or size changed may not start on the decoder of the one before.
#[test]
fn a_changed_stream_opens_its_own_decoder() {
    let _serial = super::super::decoder_cache::serialize();
    let kept = VideoSetup { format: nightjar_core::sdp::VIDEO_FORMAT_H264, width: 112, height: 64, fps: 60, chroma_packing: 0, logical_width: 112, logical_height: 64, color_space: 1, color_range: 0 };
    let changed = VideoSetup { format: nightjar_core::sdp::VIDEO_FORMAT_H265, ..kept };
    let (mut first, shared) = decoder();
    first.setup(&kept).unwrap();
    shared.keep_decoder.store(true, Ordering::Relaxed);
    first.cleanup();
    let before = super::super::decoder_cache::TAKEN.load(Ordering::Relaxed);
    let (mut second, _) = decoder();
    second.setup(&changed).unwrap();
    assert_eq!(super::super::decoder_cache::TAKEN.load(Ordering::Relaxed), before, "no decoder of the last stream serves this one");
    assert_eq!(second.decoder.as_ref().map(|d| d.hardware), Some(Hardware::None), "and one of its own opened");
}

/// PyroWave's SDK decoder is kept too: its textures and fences are the start's cost.
#[test]
fn a_reconnect_takes_the_pyrowave_decoder_the_stream_before_kept() {
    let _serial = super::super::decoder_cache::serialize();
    let setup = VideoSetup { format: nightjar_core::sdp::VIDEO_FORMAT_PYROWAVE, width: 96, height: 64, fps: 60, chroma_packing: 0, logical_width: 96, logical_height: 64, color_space: 1, color_range: 0 };
    let (mut first, shared) = decoder();
    first.setup(&setup).unwrap();
    shared.keep_decoder.store(true, Ordering::Relaxed);
    first.cleanup();
    let before = super::super::decoder_cache::PYROWAVE_TAKEN.load(Ordering::Relaxed);
    let (mut second, _) = decoder();
    second.setup(&setup).unwrap();
    assert_eq!(super::super::decoder_cache::PYROWAVE_TAKEN.load(Ordering::Relaxed), before + 1, "the next stream took the kept PyroWave decoder");
    assert!(second.pyrowave.is_some());
    assert_eq!(*lock(&second.shared.decoder_name), "PyroWave (Rust, GPU wavelets)");
}

/// A stream that ends for good keeps nothing: the decoder goes as it did before.
#[test]
fn a_stream_that_ends_for_good_drops_its_decoder() {
    let _serial = super::super::decoder_cache::serialize();
    let setup = VideoSetup { format: nightjar_core::sdp::VIDEO_FORMAT_H264, width: 128, height: 96, fps: 60, chroma_packing: 0, logical_width: 128, logical_height: 96, color_space: 1, color_range: 0 };
    let (mut first, _shared) = decoder();
    first.setup(&setup).unwrap();
    first.cleanup();
    let before = super::super::decoder_cache::TAKEN.load(Ordering::Relaxed);
    let (mut second, _) = decoder();
    second.setup(&setup).unwrap();
    assert_eq!(super::super::decoder_cache::TAKEN.load(Ordering::Relaxed), before, "nothing was kept");
}

/// A session that stops after its renderer was torn down (a launch's thread joining the last
/// session's teardown on the way out) parks no decoder that borrows or holds the old device.
#[test]
fn a_session_that_stops_after_its_renderer_went_parks_nothing() {
    let _serial = super::super::decoder_cache::serialize();
    let setup = VideoSetup { format: nightjar_core::sdp::VIDEO_FORMAT_H264, width: 144, height: 96, fps: 60, chroma_packing: 0, logical_width: 144, logical_height: 96, color_space: 1, color_range: 0 };
    let (mut first, shared) = decoder();
    first.setup(&setup).unwrap();
    shared.keep_decoder.store(true, Ordering::Relaxed);
    super::super::decoder_cache::retire();
    first.cleanup();
    let before = super::super::decoder_cache::TAKEN.load(Ordering::Relaxed);
    let (mut second, _) = decoder();
    second.setup(&setup).unwrap();
    assert_eq!(super::super::decoder_cache::TAKEN.load(Ordering::Relaxed), before, "nothing was parked for the new renderer");
}

/// A decoder taken from the last stream whose first frame fails (a frames pool the key could not
/// see fits badly) opens its context again at once, not after the retry's second.
#[test]
fn a_reused_decoders_first_failed_frame_opens_it_again_at_once() {
    let setup = VideoSetup { format: nightjar_core::sdp::VIDEO_FORMAT_H264, width: 192, height: 96, fps: 60, chroma_packing: 0, logical_width: 192, logical_height: 96, color_space: 1, color_range: 0 };
    let (mut decoder, _) = decoder();
    decoder.setup(&setup).unwrap();
    decoder.reused = true;
    decoder.after_frame(false, true, 0);
    assert!(!decoder.reused);
    assert_eq!(decoder.failing, Failing::default(), "the reopen is not counted as a failure run");
    assert!(decoder.decoder.is_some(), "the stream goes on with the same device");
}

#[test]
fn a_reused_decoder_whose_fast_reopen_fails_rebuilds_immediately_without_waiting() {
    let setup = VideoSetup { format: nightjar_core::sdp::VIDEO_FORMAT_H264, width: 192, height: 96, fps: 60, chroma_packing: 0, logical_width: 192, logical_height: 96, color_space: 1, color_range: 0 };
    let (mut decoder, shared) = decoder();
    decoder.setup(&setup).unwrap();
    decoder.decoder = None;
    decoder.reused = true;
    decoder.after_frame(false, true, 0);
    assert!(!decoder.reused);
    assert_eq!(decoder.failing, Failing::default(), "rebuilt immediately instead of entering Escalation::Wait");
    assert!(decoder.decoder.is_some(), "rebuilt a fresh decoder immediately");
    assert!(notices(&shared).is_empty());
}

/// V4L2 M2M decoders keep the driver's capture and output queues across
/// `avcodec_flush_buffers`: a reused one could hand the new stream a picture of the last.
#[test]
fn a_v4l2_m2m_decoder_is_not_kept_for_a_reconnect() {
    let _serial = super::super::decoder_cache::serialize();
    let setup = VideoSetup { format: nightjar_core::sdp::VIDEO_FORMAT_H264, width: 160, height: 96, fps: 60, chroma_packing: 0, logical_width: 160, logical_height: 96, color_space: 1, color_range: 0 };
    let (mut first, shared) = decoder();
    first.setup(&setup).unwrap();
    first.hardware = Hardware::V4l2m2m;
    first.decoder.as_mut().expect("a decoder").hardware = Hardware::V4l2m2m;
    shared.keep_decoder.store(true, Ordering::Relaxed);
    first.cleanup();
    assert!(!super::super::decoder_cache::media_parked(), "the driver's queues outlive a flush");
}

/// A decoder that failed before the stream ended is not handed to the reconnect, even when the
/// failures were still below the retry's threshold.
#[test]
fn a_decoder_that_failed_before_the_stream_ended_is_not_kept() {
    let _serial = super::super::decoder_cache::serialize();
    let setup = VideoSetup { format: nightjar_core::sdp::VIDEO_FORMAT_H264, width: 176, height: 96, fps: 60, chroma_packing: 0, logical_width: 176, logical_height: 96, color_space: 1, color_range: 0 };
    let (mut first, shared) = decoder();
    first.setup(&setup).unwrap();
    shared.keep_decoder.store(true, Ordering::Relaxed);
    first.frame_failed(0);
    assert!(!first.failing.reopened, "one failed frame is below the retry");
    first.cleanup();
    assert!(!super::super::decoder_cache::media_parked(), "a decoder that failed is not reused");
}

#[test]
fn a_muted_stream_opens_no_audio_output() {
    use rubylight_protocol::audio_ext::{CODEC_AC3, CODEC_OPUS, CODEC_PCM};
    let shared = Shared::new(Arc::new(|| {}), 8);
    // This device's output refuses Dolby before it opens anything, so a muted renderer that
    // takes it is not that output.
    assert_eq!(audio_renderer(shared.clone(), 0, false).init(&audio_setup(CODEC_AC3)), Err(-1));
    for codec in [CODEC_OPUS, CODEC_PCM, CODEC_AC3] {
        let mut muted = audio_renderer(shared.clone(), 0, true);
        assert_eq!(muted.init(&audio_setup(codec)), Ok(()), "codec {codec}");
        muted.start();
        muted.play(Some(&[0; 8]));
        muted.play(None);
        muted.stop();
        muted.cleanup();
    }
    assert_eq!(shared.counters.audio_channels.load(Ordering::Relaxed), 0, "no output was opened");
    assert!(notices(&shared).is_empty(), "a muted stream says nothing about its audio");
}

#[test]
fn a_muted_stream_asks_for_no_uncompressed_audio() {
    use rubylight_protocol::audio_ext::CODEC_PCM;
    assert_eq!(passthrough_codecs(true, true), 0);
    assert_eq!(passthrough_codecs(false, true), 0);
    assert_eq!(passthrough_codecs(true, false), CODEC_PCM);
    assert_eq!(passthrough_codecs(false, false), 0);
}

#[test]
fn a_late_termination_of_a_failed_attempt_does_not_end_the_retry() {
    let shared = Shared::new(Arc::new(|| {}), 8);
    let failed = Events::new(shared.clone());
    shared.next_attempt();
    let retry = Events::new(shared.clone());
    // The failed attempt's termination thread runs after the retry began.
    failed.connection_terminated(5);
    assert!(notices(&shared).is_empty(), "a stale termination is dropped");
    retry.connection_terminated(6);
    assert!(matches!(notices(&shared).as_slice(), [Notice::Terminated(6)]));
}

#[test]
fn every_received_frame_with_a_header_value_counts_for_the_host_processing_average() {
    // No decoder is set up, so nothing is decoded or shown: the count must not depend on that.
    let (mut decoder, shared) = decoder();
    for units in [5u16, 0, 6, 4] {
        let mut frame = Frame::default();
        frame.host_processing_latency = units;
        assert!(matches!(decoder.submit(&frame), DecodeStatus::Ok));
    }
    assert_eq!(shared.counters.host_latency_total.load(Ordering::Relaxed), 15);
    assert_eq!(shared.counters.host_latency_frames.load(Ordering::Relaxed), 3);
    assert_eq!(shared.counters.decoded.load(Ordering::Relaxed), 0);
}

#[test]
fn a_decoder_without_the_beam_race_hands_over_no_slices_and_wakes_nobody() {
    let woken = Arc::new(AtomicU64::new(0));
    let counter = woken.clone();
    let shared = Shared::new(Arc::new(move || { counter.fetch_add(1, Ordering::Relaxed); }), 8);
    let decoder = Decoder::new(shared.clone(), Hardware::None);
    assert_eq!(decoder.slices, 0, "the race is off unless a stream asks for slices");
    shared.push_slices(Vec::new());
    assert!(lock(&shared.slices).is_empty());
    assert_eq!(woken.load(Ordering::Relaxed), 0, "no slices, no wake-up");
}

#[test]
fn chunks_through_a_decoder_with_the_beam_race_off_queue_no_slice_and_wake_nobody() {
    let woken = Arc::new(AtomicU64::new(0));
    let counter = woken.clone();
    let shared = Shared::new(Arc::new(move || { counter.fetch_add(1, Ordering::Relaxed); }), 8);
    let mut decoder = Decoder::new(shared.clone(), Hardware::None).with_early_decode(true);
    decoder.pyrowave = Some(super::super::pyrowave::Pyrowave::cpu(256, 128, false).expect("the CPU decoder"));
    let packet = [0u8; 64];
    for n in 0..8 {
        decoder.chunk(n == 0, 0, &packet);
    }
    assert!(lock(&shared.slices).is_empty(), "no slice is queued for the presenter");
    assert_eq!(woken.load(Ordering::Relaxed), 0, "and the presenter is not woken for one");
    assert!(decoder.pyrowave.as_mut().unwrap().take_slices().is_empty());
}

#[test]
fn a_gpu_device_failure_ends_the_stream_with_its_message() {
    let (mut decoder, shared) = decoder();
    let status = decoder.failed(Failure::Device("device removed".into()));
    assert!(matches!(status, DecodeStatus::NeedIdr));
    assert_eq!(shared.counters.decode_errors.load(Ordering::Relaxed), 1);
    let notices = notices(&shared);
    let [Notice::DecoderFailed(text)] = notices.as_slice() else { panic!("expected exactly the DecoderFailed notice") };
    assert_eq!(text, "PyroWave GPU decoding unavailable: device removed");
    // The stream shows it as the reason it ended and does not reconnect on its own.
    assert!(!crate::stream::Exit::Ended(Some(text.clone())).retryable(true));
}

#[test]
fn a_bad_frame_asks_for_an_idr_and_a_full_ring_drops_the_frame_without_ending_the_stream() {
    let (mut decoder, shared) = decoder();
    assert!(matches!(decoder.failed(Failure::Frame("bad".into())), DecodeStatus::NeedIdr));
    assert!(matches!(decoder.failed(Failure::NoPlanes), DecodeStatus::Ok));
    assert!(matches!(decoder.failed(Failure::NoPlanes), DecodeStatus::Ok));
    assert_eq!(shared.counters.decode_errors.load(Ordering::Relaxed), 1);
    assert_eq!(shared.counters.replaced.load(Ordering::Relaxed), 2);
    assert!(notices(&shared).is_empty(), "only a broken GPU decoder ends the stream");
}

#[test]
fn early_and_fallback_frames_are_counted_whatever_the_frame_result() {
    // `submit` counts before it looks at the result, so a frame that failed after its blocks
    // were decoded early (or whole, after a mismatch) still shows in the counters, as in the
    // CPU path before the GPU decoder existed.
    use super::super::pyrowave::Source;
    let (_, shared) = decoder();
    assert_eq!(shared.count_source(Source::Early), 1);
    assert_eq!(shared.count_source(Source::Fallback), 2);
    assert_eq!(shared.count_source(Source::Fallback), 2);
    assert_eq!(shared.count_source(Source::Frame), 0);
    assert_eq!(shared.counters.early_frames.load(Ordering::Relaxed), 1);
    assert_eq!(shared.counters.early_fallbacks.load(Ordering::Relaxed), 2);
}

#[test]
fn evicting_drops_the_oldest_waiting_picture_first_so_the_decoder_is_never_starved() {
    use super::super::present::{tests::decoded, Pending};
    let (_, shared) = decoder();
    let pending = |present_us| Pending { decoded: decoded(present_us), present_us, hold_us: 0, late: false };
    assert!(!shared.evict_oldest(), "nothing queued: the caller reports NoPlanes");
    // The pacer's pictures are older than those the presenter has not taken yet.
    lock(&shared.pacer_backlog).extend([pending(1), pending(2), pending(3)]);
    lock(&shared.frames).push_back(decoded(10));
    assert!(shared.evict_oldest() && shared.evict_oldest());
    let left: Vec<u64> = lock(&shared.pacer_backlog).iter().map(|p| p.present_us).collect();
    assert_eq!(left, [3], "the oldest pacer pictures went first");
    assert_eq!(lock(&shared.frames).len(), 1, "the newer queued picture stays");
    assert!(shared.evict_oldest());
    assert!(lock(&shared.pacer_backlog).is_empty());
    assert_eq!(shared.counters.replaced.load(Ordering::Relaxed), 3);
    assert_eq!(shared.waiting(), 1);
    // Even the last queued picture is older than the frame being decoded, so it may go.
    assert!(shared.evict_oldest() && !shared.evict_oldest());
}

#[test]
fn a_hold_spike_with_host_times_keeps_every_picture_and_the_pool_has_a_surface_for_each() {
    use super::super::present::{tests::decoded, Pending};
    let max_hold_us = rubylight_protocol::present_clock::Settings::default().max_hold_us as u64;
    for fps in [240u32, 144, 60] {
        let (_, shared) = decoder();
        let limit = pool_waiting(true, shared.queue_limit, fps);
        assert!(media::pool_extra(limit) as usize >= limit + 3, "{fps} FPS: spares for the shown, drawn and kept ones too");
        // A Wi-Fi hiccup: the clock holds every picture of the last 50 ms the full hold, and the
        // presenter's queue is full besides.
        let frame_us = 1_000_000 / u64::from(fps);
        let held = max_hold_us.div_ceil(frame_us) + 1;
        lock(&shared.pacer_backlog).extend((0..held).map(|i| {
            let ready_us = 1_000 + i * frame_us;
            Pending { decoded: decoded(ready_us), present_us: ready_us + max_hold_us, hold_us: max_hold_us as i64, late: false }
        }));
        for i in 0..shared.queue_limit - 1 {
            lock(&shared.frames).push_back(decoded(1_000_000 + i as u64));
        }
        shared.queue_pooled(decoded(2_000_000), limit);
        assert_eq!(shared.waiting(), limit, "{fps} FPS: nothing dropped");
        assert_eq!(shared.counters.replaced.load(Ordering::Relaxed), 0, "{fps} FPS");
        assert!(lock(&shared.pacer_backlog).iter().all(|p| p.present_us == p.decoded.ready_us + max_hold_us),
            "{fps} FPS: every picture keeps its time");

        // Past what the pool has room for, the oldest held picture is shown early, not dropped.
        shared.queue_pooled(decoded(2_000_001), limit);
        let pacer = lock(&shared.pacer_backlog);
        assert_eq!(pacer.len() as u64, held, "{fps} FPS");
        assert_eq!((pacer[0].present_us, pacer[0].hold_us), (pacer[0].decoded.ready_us, 0), "{fps} FPS: due now");
        assert_eq!(pacer[1].present_us, pacer[1].decoded.ready_us + max_hold_us, "{fps} FPS: one at a time");
    }
    assert_eq!(pool_waiting(false, 2, 240), media::EXTRA_HW_FRAMES as usize - 3, "other pacing: the same pool as before");
    assert_eq!(media::pool_extra(pool_waiting(false, 2, 240)), media::EXTRA_HW_FRAMES);
}

#[test]
fn at_the_pool_limit_with_nothing_in_the_pacer_the_oldest_queued_picture_goes() {
    use super::super::present::tests::decoded;
    let (_, shared) = decoder();
    for i in 0..4 {
        lock(&shared.frames).push_back(decoded(i));
    }
    shared.queue_pooled(decoded(10), 4);
    let left: Vec<u64> = lock(&shared.frames).iter().map(|d| d.ready_us).collect();
    assert_eq!(left, [1, 2, 3, 10]);
    assert_eq!(shared.counters.replaced.load(Ordering::Relaxed), 1);
}

#[test]
fn at_the_pool_limit_with_every_pacer_picture_due_the_oldest_pacer_picture_goes() {
    use super::super::present::{tests::decoded, Pending};
    let (_, shared) = decoder();
    // Already due, waiting for the presenter thread to take them.
    lock(&shared.pacer_backlog).extend((1..=3u64).map(|ready_us| {
        Pending { decoded: decoded(ready_us), present_us: ready_us, hold_us: 0, late: false }
    }));
    lock(&shared.frames).push_back(decoded(10));
    shared.queue_pooled(decoded(11), 4);
    assert_eq!(shared.waiting(), 4, "bounded to the limit");
    let left: Vec<u64> = lock(&shared.pacer_backlog).iter().map(|p| p.decoded.ready_us).collect();
    assert_eq!(left, [2, 3], "the oldest due picture freed its surface");
    let queued: Vec<u64> = lock(&shared.frames).iter().map(|d| d.ready_us).collect();
    assert_eq!(queued, [10, 11]);
    assert_eq!(shared.counters.replaced.load(Ordering::Relaxed), 1);
}

#[test]
fn frames_failing_for_a_second_open_the_decoder_again_once_then_end_the_stream() {
    let mut failing = Failing::default();
    assert_eq!(failing.failed(0, true, true), Escalation::Wait);
    assert_eq!(failing.failed(400_000, true, true), Escalation::Wait);
    assert_eq!(failing.failed(1_000_000, true, true), Escalation::Reopen);
    assert_eq!(failing.failed(1_100_000, true, true), Escalation::Wait, "the reopened decoder gets its own second");
    assert_eq!(failing.failed(1_600_000, true, true), Escalation::Wait);
    assert_eq!(failing.failed(2_100_000, true, true), Escalation::Rebuild, "a context on the same device made no picture: a new device is the last try");
    assert_eq!(failing.failed(2_200_000, true, true), Escalation::Wait, "the rebuilt decoder gets its own second");
    assert_eq!(failing.failed(2_700_000, true, true), Escalation::Wait);
    assert_eq!(failing.failed(3_200_000, true, true), Escalation::GiveUp);
    assert_eq!(failing.failed(4_000_000, true, true), Escalation::Wait, "said once");
    // A picture between failures starts over, and two failures a pause apart are not enough.
    let mut failing = Failing::default();
    failing.failed(0, true, true);
    failing.failed(500_000, true, true);
    failing.decoded();
    assert_eq!(failing.failed(1_200_000, true, true), Escalation::Wait);
    assert_eq!(failing.failed(6_000_000, true, true), Escalation::Wait);
    // A backend on the renderer's own device (D3D11VA, Vulkan video) or none (software) is opened
    // again once, and a rebuild would take the same device: it ends the stream, as before the
    // rebuild existed.
    let mut failing = Failing::default();
    failing.failed(0, true, false);
    failing.failed(500_000, true, false);
    assert_eq!(failing.failed(1_000_000, true, false), Escalation::Reopen);
    failing.failed(1_100_000, true, false);
    failing.failed(1_600_000, true, false);
    assert_eq!(failing.failed(2_100_000, true, false), Escalation::GiveUp);
    // A decoder that cannot be opened again ends the stream at once.
    let mut failing = Failing::default();
    failing.failed(0, false, false);
    failing.failed(500_000, false, false);
    assert_eq!(failing.failed(1_000_000, false, false), Escalation::GiveUp);
}

#[test]
fn a_decoder_that_keeps_failing_is_opened_again_and_then_ends_the_stream_saying_why() {
    let (mut decoder, shared) = decoder();
    let setup = VideoSetup { format: nightjar_core::sdp::VIDEO_FORMAT_H264, width: 128, height: 128, fps: 60, chroma_packing: 0, logical_width: 128, logical_height: 128, color_space: 1, color_range: 0 };
    decoder.setup(&setup).unwrap();
    for at in [0, 500_000, 1_000_000] {
        decoder.frame_failed(at);
    }
    assert!(decoder.decoder.is_some(), "opened again");
    assert!(decoder.failing.reopened);
    assert!(notices(&shared).is_empty(), "the stream goes on");
    // Software has no device of its own: a rebuild would only open it again, so the reopened
    // decoder's second ends the stream.
    for at in [1_100_000, 1_600_000, 2_100_000] {
        decoder.frame_failed(at);
    }
    assert!(!decoder.failing.rebuilt, "no rebuild for a decoder without a device of its own");
    let ended = notices(&shared);
    let [Notice::DecoderFailed(text)] = ended.as_slice() else { panic!("expected exactly the DecoderFailed notice") };
    assert!(text.contains("stopped working"), "{text}");
    decoder.frame_failed(3_000_000);
    assert!(notices(&shared).is_empty(), "said once");
}

/// Nightjar issue 77: V4L2 M2M on an Odin 2 Portal sometimes never made a picture, opened
/// again or not, and the stream ended. The decoder is still opened again once (that worked in
/// one of the two runs); a hardware decoder that then makes no picture either gives way to the
/// next decoder, software after the last, which gets its own second and reopen.
#[test]
fn a_hardware_decoder_that_never_made_a_picture_gives_way_to_the_next_decoder() {
    let setup = VideoSetup { format: nightjar_core::sdp::VIDEO_FORMAT_H264, width: 128, height: 128, fps: 60, chroma_packing: 0, logical_width: 128, logical_height: 128, color_space: 1, color_range: 0 };
    // Stands in for an opened V4L2 M2M decoder, the last of its order.
    let as_v4l2 = |decoder: &mut Decoder| {
        decoder.hardware = Hardware::V4l2m2m;
        decoder.decoder.as_mut().expect("a decoder").hardware = Hardware::V4l2m2m;
    };
    for pictured_before in [false, true] {
        let (mut decoder, shared) = decoder();
        decoder.setup(&setup).unwrap();
        as_v4l2(&mut decoder);
        decoder.pictured = pictured_before;
        for at in [0, 500_000, 1_000_000] {
            decoder.frame_failed(at);
        }
        assert!(decoder.failing.reopened, "opened again first");
        assert!(decoder.refused.is_empty());
        // The reopened decoder (V4L2 again on the device) makes no picture either.
        as_v4l2(&mut decoder);
        let _ = notices(&shared);
        // And a decoder made anew on a new device is tried before it gives way.
        for at in [1_100_000, 1_600_000, 2_100_000] {
            decoder.frame_failed(at);
        }
        assert!(decoder.failing.rebuilt, "made anew first");
        assert!(decoder.refused.is_empty());
        as_v4l2(&mut decoder);
        let _ = notices(&shared);
        for at in [2_200_000, 2_700_000, 3_200_000] {
            decoder.frame_failed(at);
        }
        let now = decoder.decoder.as_ref().expect("the next decoder");
        assert_eq!(now.hardware, Hardware::None, "software after the last hardware decoder");
        assert_eq!(decoder.refused, [(Hardware::V4l2m2m, setup.format)]);
        assert!(matches!(notices(&shared).as_slice(), [Notice::DecoderFallback(_)]), "the switch is said, the stream goes on");
        assert_eq!(decoder.failing, Failing::default(), "the next decoder gets its own second");
        // A software decoder that fails too is opened again once, then the stream ends.
        for at in [3_300_000, 3_800_000, 4_300_000] {
            decoder.frame_failed(at);
        }
        assert!(decoder.failing.reopened && notices(&shared).is_empty());
        for at in [4_400_000, 4_900_000, 5_400_000] {
            decoder.frame_failed(at);
        }
        assert!(!decoder.failing.rebuilt, "software has no device to rebuild");
        assert!(matches!(notices(&shared).as_slice(), [Notice::DecoderFailed(_)]));
    }
}

#[test]
fn packed_streams_on_linux_name_the_coded_size_their_pictures_are_copied_at() {
    // #80: 3840x2160 packed codes 3840x4320 pictures.
    let packed = VideoSetup { format: nightjar_core::sdp::VIDEO_FORMAT_AV1_MAIN10, width: 3840, height: 4320, fps: 100, chroma_packing: 2, logical_width: 3840, logical_height: 2160, color_space: 2, color_range: 0 };
    assert_eq!(packed_from_memory(&packed, false), cfg!(unix).then_some((3840, 4320)));
    assert_eq!(packed_from_memory(&packed, true), None, "pictures mapped into Vulkan stay on the GPU");
    assert_eq!(packed_from_memory(&VideoSetup { chroma_packing: 0, height: 2160, ..packed }, false), None, "unpacked pictures are not rebuilt");
    // A failed import after install drops the map: from then the doctor hears of memory.
    assert_eq!(packed_map_dropped(&packed, true, false), cfg!(unix).then_some((3840, 4320)));
    assert_eq!(packed_map_dropped(&packed, true, true), None, "still mapped");
    assert_eq!(packed_map_dropped(&packed, false, false), None, "already said at install");
}

#[test]
fn only_8_bit_4_2_0_unpacked_streams_go_to_an_importing_presenter_as_dmabufs() {
    // HEVC Main10 is 10-bit 4:2:0: VIDEO_FORMAT_MASK_10BIT also holds the 10-bit 4:4:4 bits.
    use nightjar_core::sdp::{VIDEO_FORMAT_H264, VIDEO_FORMAT_H265_MAIN10};
    let setup = VideoSetup { format: VIDEO_FORMAT_H264, width: 1920, height: 1080, fps: 60, chroma_packing: 0, logical_width: 1920, logical_height: 1080, color_space: 1, color_range: 0 };
    assert!(exports_dmabuf(true, false, &setup));
    assert!(!exports_dmabuf(false, false, &setup), "a presenter that does not import gets mappings");
    assert!(!exports_dmabuf(true, false, &VideoSetup { format: VIDEO_FORMAT_H265_MAIN10, ..setup }), "SDL's GL renderers have no P010 texture");
    assert!(!exports_dmabuf(true, false, &VideoSetup { format: VIDEO_FORMAT_H264 | VIDEO_FORMAT_MASK_YUV444, ..setup }));
    assert!(!exports_dmabuf(true, false, &VideoSetup { chroma_packing: 1, ..setup }), "packed pictures are rebuilt from memory");
    // A display plane takes the pictures as they are, P010 too, but not 4:4:4 or packed ones.
    assert!(exports_dmabuf(false, true, &setup));
    assert!(exports_dmabuf(false, true, &VideoSetup { format: VIDEO_FORMAT_H265_MAIN10, ..setup }));
    assert!(!exports_dmabuf(true, true, &VideoSetup { format: VIDEO_FORMAT_H264 | VIDEO_FORMAT_MASK_YUV444, ..setup }));
    assert!(!exports_dmabuf(false, true, &VideoSetup { chroma_packing: 1, ..setup }));
    // A software decoder asked for DMA-BUFs decodes as before.
    let (decoder, _shared) = decoder();
    let mut decoder = decoder.with_dmabuf(true);
    decoder.setup(&VideoSetup { width: 128, height: 128, logical_width: 128, logical_height: 128, ..setup }).unwrap();
    assert_eq!(decoder.decoder.as_ref().map(|d| d.hardware), Some(Hardware::None));
}

#[test]
fn a_pyrowave_decoder_that_keeps_failing_ends_the_stream() {
    let (mut decoder, shared) = decoder();
    let setup = VideoSetup { format: nightjar_core::sdp::VIDEO_FORMAT_PYROWAVE, width: 128, height: 128, fps: 60, chroma_packing: 0, logical_width: 128, logical_height: 128, color_space: 1, color_range: 0 };
    decoder.setup(&setup).unwrap();
    for at in [0, 500_000, 1_000_000] {
        decoder.frame_failed(at);
    }
    assert!(matches!(notices(&shared).as_slice(), [Notice::DecoderFailed(_)]));
}

#[test]
fn pyrowave_frames_that_lost_their_data_never_end_the_stream() {
    let (mut decoder, shared) = decoder();
    let setup = VideoSetup { format: nightjar_core::sdp::VIDEO_FORMAT_PYROWAVE, width: 128, height: 128, fps: 60, chroma_packing: 0, logical_width: 128, logical_height: 128, color_space: 1, color_range: 0 };
    decoder.setup(&setup).unwrap();
    // A loss burst of seconds on a bad link: an IDR is asked for each time, nothing more.
    for at in [0, 500_000, 1_000_000, 2_000_000, 5_000_000] {
        decoder.lost = false;
        assert!(matches!(decoder.failed(Failure::Loss("lost".into())), DecodeStatus::NeedIdr));
        decoder.after_frame(false, true, at);
    }
    assert!(notices(&shared).is_empty(), "the stream goes on");
    assert_eq!(decoder.failing, Failing::default());
    // Frames the decoder itself rejects still end it.
    for at in [6_000_000, 6_500_000, 7_000_000] {
        decoder.lost = false;
        decoder.failed(Failure::Frame("bad".into()));
        decoder.after_frame(false, true, at);
    }
    assert!(matches!(notices(&shared).as_slice(), [Notice::DecoderFailed(_)]));
}

#[test]
fn gpu_mode_without_a_device_never_builds_a_cpu_decoder() {
    // No renderer device (Windows) or no SDK library next to the test executable (Linux, where
    // the planes cross through the CPU and the device only proves the renderer): the GPU
    // decoder cannot be built, and setup reports that instead of decoding on the CPU.
    let (mut decoder, shared) = decoder();
    decoder = decoder.with_gpu_decoder(Some(GpuDevice(std::ptr::null_mut())));
    let setup = VideoSetup { format: nightjar_core::sdp::VIDEO_FORMAT_PYROWAVE, width: 128, height: 128, fps: 60, chroma_packing: 0, logical_width: 128, logical_height: 128, color_space: 1, color_range: 0 };
    if cfg!(target_os = "macos") {
        // A Mac decodes on the processor instead and says so (no bundled driver beside the tests).
        assert!(decoder.setup(&setup).is_ok());
        assert!(decoder.pyrowave.is_some());
        assert_eq!(*lock(&shared.decoder_name), MAC_CPU_INSTEAD);
        assert!(matches!(notices(&shared).as_slice(), [Notice::DecoderFallback(_)]));
        return;
    }
    assert!(decoder.setup(&setup).is_err());
    assert!(decoder.pyrowave.is_none());
    let notices = notices(&shared);
    assert!(matches!(notices.as_slice(), [Notice::DecoderFailed(text)] if text.contains("PyroWave GPU decoding unavailable: ")));
    #[cfg(target_os = "macos")]
    assert!(matches!(notices.as_slice(), [Notice::DecoderFailed(text)] if text.contains("no macOS backend")));
    #[cfg(target_os = "linux")]
    assert!(matches!(notices.as_slice(), [Notice::DecoderFailed(text)] if text.contains("libpyrowave-shared.so not found")), "the reason is the missing library");
}
