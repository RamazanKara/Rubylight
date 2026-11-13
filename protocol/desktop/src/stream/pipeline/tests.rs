use super::*;

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
    let status = decoder.failed(Failure::Device("PyroWave GPU decode failed: device removed".into()));
    assert!(matches!(status, DecodeStatus::NeedIdr));
    assert_eq!(shared.counters.decode_errors.load(Ordering::Relaxed), 1);
    let notices = notices(&shared);
    let [Notice::DecoderFailed(text)] = notices.as_slice() else { panic!("expected exactly the DecoderFailed notice") };
    assert_eq!(text, "PyroWave GPU decode failed: device removed");
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
    assert_eq!(failing.failed(0, true), Escalation::Wait);
    assert_eq!(failing.failed(400_000, true), Escalation::Wait);
    assert_eq!(failing.failed(1_000_000, true), Escalation::Reopen);
    assert_eq!(failing.failed(1_100_000, true), Escalation::Wait, "the reopened decoder gets its own second");
    assert_eq!(failing.failed(1_600_000, true), Escalation::Wait);
    assert_eq!(failing.failed(2_100_000, true), Escalation::GiveUp);
    assert_eq!(failing.failed(3_000_000, true), Escalation::Wait, "said once");
    // A picture between failures starts over, and two failures a pause apart are not enough.
    let mut failing = Failing::default();
    failing.failed(0, true);
    failing.failed(500_000, true);
    failing.decoded();
    assert_eq!(failing.failed(1_200_000, true), Escalation::Wait);
    assert_eq!(failing.failed(6_000_000, true), Escalation::Wait);
    // A decoder that cannot be opened again ends the stream at once.
    let mut failing = Failing::default();
    failing.failed(0, false);
    failing.failed(500_000, false);
    assert_eq!(failing.failed(1_000_000, false), Escalation::GiveUp);
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
    for at in [1_100_000, 1_600_000, 2_200_000] {
        decoder.frame_failed(at);
    }
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
        for at in [1_100_000, 1_600_000, 2_100_000] {
            decoder.frame_failed(at);
        }
        let now = decoder.decoder.as_ref().expect("the next decoder");
        assert_eq!(now.hardware, Hardware::None, "software after the last hardware decoder");
        assert_eq!(decoder.refused, [(Hardware::V4l2m2m, setup.format)]);
        assert!(matches!(notices(&shared).as_slice(), [Notice::DecoderFallback(_)]), "the switch is said, the stream goes on");
        assert_eq!(decoder.failing, Failing::default(), "the next decoder gets its own second");
        // A software decoder that fails too is opened again once, then the stream ends.
        for at in [2_200_000, 2_700_000, 3_200_000] {
            decoder.frame_failed(at);
        }
        assert!(decoder.failing.reopened && notices(&shared).is_empty());
        for at in [3_300_000, 3_800_000, 4_300_000] {
            decoder.frame_failed(at);
        }
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
    assert!(decoder.setup(&setup).is_err());
    assert!(decoder.pyrowave.is_none());
    let notices = notices(&shared);
    assert!(matches!(notices.as_slice(), [Notice::DecoderFailed(text)] if text.contains("PyroWave GPU decoding is unavailable")));
    #[cfg(target_os = "macos")]
    assert!(matches!(notices.as_slice(), [Notice::DecoderFailed(text)] if text.contains("no macOS backend")));
    #[cfg(target_os = "linux")]
    assert!(matches!(notices.as_slice(), [Notice::DecoderFailed(text)] if text.contains("libpyrowave-shared.so not found")), "the reason is the missing library");
}

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

/// NVDEC pictures that go through memory after all, because CUDA's copy into the renderer's
/// Vulkan images failed, are said in a toast once, on the frame that shows it.
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
