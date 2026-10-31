use super::*;

#[test]
#[ignore = "needs the default playback device; proves the fix only while nothing plays"]
fn an_endpoint_capture_keeps_delivering_through_silence() {
    // SAFETY: no pointers; the thread keeps COM for the test.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap() };
    // The host's 1 ms timer resolution, so 5 ms waits are not 15.6 ms ones.
    // SAFETY: no pointers.
    unsafe { windows::Win32::Media::timeBeginPeriod(1) };
    let mut capture = Loopback::new(2).unwrap();
    let start = std::time::Instant::now();
    let (mut first, mut last, mut worst, mut packets) = (None, start, std::time::Duration::ZERO, 0);
    while start.elapsed() < std::time::Duration::from_secs(2) {
        capture.wait(std::time::Duration::from_millis(5));
        while capture.read(240).unwrap().is_some() {
            let now = std::time::Instant::now();
            // The endpoint takes a moment to start delivering.
            if first.is_some_and(|at| now - at > std::time::Duration::from_millis(200)) {
                worst = worst.max(now - last);
            }
            first.get_or_insert(now);
            last = now;
            packets += 1;
        }
    }
    // An idle endpoint without the keep-alive delivers nothing at all. With it, the audio
    // keeps time, and no wait comes near the 50 ms after which the host sends its own
    // silence (deliveries bunch up to about 40 ms on a busy PC, as they do with sound).
    eprintln!("{packets} packets, worst gap {worst:?}");
    assert!(packets >= 360, "{packets} packets of 5 ms in 2 s");
    assert!(
        worst < std::time::Duration::from_millis(50),
        "a {worst:?} gap between deliveries"
    );
}

fn capture_samples(rate: u32) -> CaptureSamples {
    CaptureSamples {
        channels: 2,
        bits: 32,
        float: true,
        resampler: butterpollo_core::audio::Resampler::new(rate, 2).unwrap(),
        matrix: butterpollo_core::audio::mix_matrix(2, 3, 2).unwrap(),
        input: vec![0.; 2],
        output: vec![0.; 2],
        trimmed: 0,
    }
}

fn captured(samples: &mut CaptureSamples, values: &[f32], flags: u32) {
    assert_eq!(
        (samples.channels, samples.bits, samples.float),
        (2, 32, true)
    );
    assert!(values.len().is_multiple_of(2));
    // SAFETY: values contains complete float stereo frames and outlives the call.
    unsafe { samples.push_packet(values.as_ptr().cast(), values.len() / 2, flags) };
}

#[test]
fn capture_discontinuity_preserves_valid_packets_from_the_same_drain() {
    let discontinuity = AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32;
    for packet_ms in [5, 10, 20] {
        let mut samples = capture_samples(48_000);
        // Windows delivers two 10 ms packets in one drain. The flag on
        // the later packet must not erase any of the earlier packet.
        let first = vec![0.25; 480 * 2];
        let second = vec![-0.25; 480 * 2];
        captured(&mut samples, &first, 0);
        captured(&mut samples, &second, discontinuity);
        let mut output = Vec::new();
        while let Some(packet) = samples.read(packet_ms * 48) {
            output.extend(packet);
        }
        assert_eq!(output, [first, second].concat(), "{packet_ms} ms packets");
    }
}

#[test]
fn capture_discontinuity_preserves_a_partial_packet_across_reads() {
    let mut samples = capture_samples(48_000);
    let first = vec![0.25; 480 * 2];
    captured(&mut samples, &first, 0);
    assert!(samples.read(960).is_none());
    // A silent discontinuity also carries valid frames, without a data
    // pointer; the preceding half packet must still be delivered.
    // SAFETY: Silent packets do not read the null data pointer.
    unsafe {
        samples.push_packet(
            std::ptr::null(),
            480,
            (AUDCLNT_BUFFERFLAGS_SILENT.0 | AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0) as u32,
        );
    }
    assert_eq!(
        samples.read(960).unwrap(),
        [first, vec![0.; 480 * 2]].concat()
    );
    assert!(samples.read(960).is_none());
}

#[test]
fn capture_discontinuity_keeps_fractional_resampling_position() {
    let mut samples = capture_samples(44_100);
    let ramp = |start| {
        (start..start + 222)
            .flat_map(|frame| [frame as f32 / 1000., -(frame as f32) / 1000.])
            .collect::<Vec<_>>()
    };
    captured(&mut samples, &ramp(0), 0);
    let mut output = samples.read(240).unwrap();
    captured(
        &mut samples,
        &ramp(222),
        AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32,
    );
    output.extend(samples.read(240).unwrap());
    for (frame, stereo) in output.as_chunks::<2>().0.iter().enumerate() {
        let expected = frame as f32 * 44_100. / 48_000. / 1000.;
        assert!((stereo[0] - expected).abs() < 1e-6, "frame {frame}");
        assert!((stereo[1] + expected).abs() < 1e-6, "frame {frame}");
    }
}

/// What a late audio thread leaves: `queued_ms` of audio waiting when it wakes, all read in
/// one pass. Returns the packets read and the samples discarded, both at 48 kHz.
fn late_wake(rate: u32, queued_ms: usize, packet_ms: usize) -> (usize, u64) {
    let mut samples = capture_samples(rate);
    let frames = rate as usize * queued_ms / 1000;
    let input = vec![0.1; frames * 2];
    // Windows hands over its 10 ms chunks as they come; the thread has not read any yet.
    for chunk in input.chunks(rate as usize / 100 * 2) {
        captured(&mut samples, chunk, 0);
    }
    let mut packets = 0;
    while samples.read(packet_ms * 48).is_some() {
        packets += 1;
    }
    (packets, samples.take_trimmed(rate))
}

#[test]
fn a_late_wake_reports_the_backlog_it_trims_so_the_stream_keeps_its_time() {
    for rate in [48_000, 44_100] {
        for (queued_ms, packet_ms) in [(100, 5), (60, 5), (45, 10), (100, 20)] {
            let (packets, trimmed) = late_wake(rate, queued_ms, packet_ms);
            // Every millisecond woken up late is either a packet or counted as trimmed.
            // At 48 kHz that is exact. A fractional rate may leave up to two packets queued
            // unread (still in the stream, not lost).
            let accounted = packets * packet_ms * 48 + trimmed as usize;
            let queued = queued_ms * 48;
            let unread = if rate == 48_000 {
                0
            } else {
                2 * packet_ms * 48
            };
            assert!(
                accounted <= queued && queued - accounted <= unread,
                "{rate} Hz, {queued_ms} ms, {packet_ms} ms packets: {packets} packets, {trimmed} trimmed"
            );
            assert!(trimmed > 0, "{rate} Hz, {queued_ms} ms");
        }
    }
}

#[test]
fn a_wake_within_the_limit_trims_nothing() {
    for (queued_ms, packet_ms) in [(25, 5), (30, 5), (30, 10), (60, 20)] {
        let (packets, trimmed) = late_wake(48_000, queued_ms, packet_ms);
        assert_eq!(trimmed, 0, "{queued_ms} ms, {packet_ms} ms packets");
        assert_eq!(packets, queued_ms / packet_ms);
    }
}

#[test]
fn the_hundred_millisecond_cap_is_reported_as_trimmed_too() {
    let mut samples = capture_samples(48_000);
    // 150 ms pushed before any read: the cap keeps the newest 100 ms.
    for _ in 0..15 {
        captured(&mut samples, &vec![0.1; 480 * 2], 0);
    }
    assert_eq!(samples.take_trimmed(48_000), 50 * 48);
    // Taken once: a second call has nothing new.
    assert_eq!(samples.take_trimmed(48_000), 0);
}

#[test]
fn capture_discontinuities_keep_the_combined_backlog_bounded() {
    for packet_ms in [5, 10, 20] {
        let limit_ms = (packet_ms * 3).max(30);
        for captured_ms in [limit_ms, limit_ms + 10] {
            let mut samples = capture_samples(48_000);
            let input = (0..captured_ms * 48)
                .flat_map(|frame| [frame as f32 / 10_000.; 2])
                .collect::<Vec<_>>();
            for packet in input.as_chunks::<960>().0 {
                captured(
                    &mut samples,
                    packet,
                    AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32,
                );
            }
            let mut output = Vec::new();
            while let Some(packet) = samples.read(packet_ms * 48) {
                output.extend(packet);
            }
            let kept_ms = if captured_ms > limit_ms {
                packet_ms * 2
            } else {
                captured_ms
            };
            assert_eq!(
                output,
                input[input.len() - kept_ms * 48 * 2..],
                "{packet_ms} ms packets, {captured_ms} ms captured"
            );
        }
    }
}

#[test]
#[ignore = "requires the packaged libopus DLL"]
fn native_opus_round_trip_preserves_every_surround_channel_and_quality_layout() {
    type DecoderCreate =
        unsafe extern "C" fn(i32, i32, i32, i32, *const u8, *mut i32) -> *mut c_void;
    type Decode = unsafe extern "C" fn(*mut c_void, *const u8, i32, *mut f32, i32, i32) -> i32;
    struct Decoder {
        state: *mut c_void,
        destroy: Destroy,
        _library: libloading::Library,
    }
    impl Drop for Decoder {
        fn drop(&mut self) {
            // SAFETY: `self.state` came from opus_multistream_decoder_create and is destroyed
            // once, here.
            unsafe {
                (self.destroy)(self.state);
            }
        }
    }
    let root = std::path::PathBuf::from(
        std::env::var_os("BUTTERPOLLO_TEST_OPUS_ROOT").expect("set BUTTERPOLLO_TEST_OPUS_ROOT"),
    );
    // LFE is deliberately band-limited by Opus surround layouts. Exercise
    // that channel with bass, while the other channels use distinct tones.
    fn frequency(channel: usize, channels: usize) -> f64 {
        if channels > 2 && channel == 3 {
            80.0
        } else {
            320.0 + 160.0 * channel as f64
        }
    }
    let mut reports = vec![];
    for (channels, quality, custom) in [
        (2, false, None),
        (2, true, None),
        (6, false, None),
        (6, true, None),
        (8, false, None),
        (8, true, None),
        (12, false, None),
        (12, true, None),
        (6, true, Some("642543210")),
    ] {
        for packet_ms in [5, 10, 20] {
            let layout =
                butterpollo_core::audio::OpusLayout::select(channels, quality, custom).unwrap();
            let mut encoder = Opus::new_layout_duration(&root, &layout, packet_ms as u8).unwrap();
            // SAFETY: the symbol types match libopus, `layout.mapping` has `channels` entries,
            // and the library is kept in Decoder with its pointers.
            let (decoder, decode) = unsafe {
                let library = libloading::Library::new(root.join("libopus-0.dll")).unwrap();
                let create = *library
                    .get::<DecoderCreate>(b"opus_multistream_decoder_create\0")
                    .unwrap();
                let destroy = *library
                    .get::<Destroy>(b"opus_multistream_decoder_destroy\0")
                    .unwrap();
                let decode = *library
                    .get::<Decode>(b"opus_multistream_decode_float\0")
                    .unwrap();
                let mut error = 0;
                let state = create(
                    48000,
                    channels as i32,
                    layout.streams,
                    layout.coupled,
                    layout.mapping.as_ptr(),
                    &mut error,
                );
                assert_eq!(error, 0);
                assert!(!state.is_null());
                (
                    Decoder {
                        state,
                        destroy,
                        _library: library,
                    },
                    decode,
                )
            };
            let frames = packet_ms * 48;
            let mut tones = vec![vec![]; channels];
            let mut bytes = 0;
            for packet in 0..80 {
                let samples: Vec<_> = (0..frames)
                    .flat_map(|frame| {
                        (0..channels).map(move |channel| {
                            let frequency = frequency(channel, channels) as f32;
                            ((packet * frames + frame) as f32 * std::f32::consts::TAU * frequency
                                / 48000.0)
                                .sin()
                                * 0.2
                        })
                    })
                    .collect();
                let encoded = encoder.encode(&samples).unwrap();
                assert!(encoded.len() <= 1360);
                bytes += encoded.len();
                let mut decoded = vec![0.0; frames * channels];
                // SAFETY: `decoder.state` is a live decoder, `encoded` is read for its length,
                // and `decoded` holds `frames` frames of `channels` samples.
                let result = unsafe {
                    decode(
                        decoder.state,
                        encoded.as_ptr(),
                        encoded.len() as i32,
                        decoded.as_mut_ptr(),
                        frames as i32,
                        0,
                    )
                };
                assert_eq!(result, frames as i32);
                if packet >= 8 {
                    for frame in decoded.chunks_exact(channels) {
                        for (channel, sample) in frame.iter().enumerate() {
                            assert!(sample.is_finite());
                            tones[channel].push(*sample);
                        }
                    }
                }
            }
            for (channel, signal) in tones.iter().enumerate() {
                let energy = (0..channels)
                    .map(|tone| {
                        let frequency = frequency(tone, channels);
                        let (real, imaginary) = signal.iter().enumerate().fold(
                            (0.0, 0.0),
                            |(real, imaginary), (at, sample)| {
                                let phase = at as f64 * std::f64::consts::TAU * frequency / 48000.0;
                                (
                                    real + f64::from(*sample) * phase.cos(),
                                    imaginary + f64::from(*sample) * phase.sin(),
                                )
                            },
                        );
                        real * real + imaginary * imaginary
                    })
                    .collect::<Vec<_>>();
                let strongest = energy
                    .iter()
                    .enumerate()
                    .max_by(|a, b| a.1.total_cmp(b.1))
                    .unwrap()
                    .0;
                assert_eq!(
                    strongest, channel,
                    "{channels} channels, quality={quality}, packet={packet_ms}ms"
                );
                assert!(energy[channel] > 100.0);
            }
            reports.push(serde_json::json!({"channels":channels,"quality":quality,"custom":custom,"packet_ms":packet_ms,"encoded_kbps":bytes as f64 * 8.0 / (80.0 * packet_ms as f64),"channel_errors":0}));
        }
    }
    if let Some(path) = std::env::var_os("BUTTERPOLLO_TEST_AUDIO_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&reports).unwrap()).unwrap();
    }
    eprintln!(
        "Validated {} native Opus encode/decode layouts and packet durations",
        reports.len()
    );
}
