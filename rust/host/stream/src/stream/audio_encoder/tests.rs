use super::{AudioEncoder, AudioSink, check};
use anyhow::Result;
use butterpollo_media_policy::audio_passthrough::{Codec, pcm_s16le};
use rubylight_protocol::audio_ext::{
    CHUNK_PAYLOAD_BYTES, CODEC_AC3, CODEC_EAC3, CODEC_OPUS, CODEC_PCM, ChunkHeader,
};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn ffmpeg() -> PathBuf {
    std::env::var_os("BUTTERPOLLO_TEST_FFMPEG_ROOT")
        .expect("BUTTERPOLLO_TEST_FFMPEG_ROOT names FFmpeg's bin folder")
        .into()
}

/// A GameStream-like transport that keeps the packets and frames it is given.
#[derive(Default)]
struct Recorder {
    packets: Vec<Vec<u8>>,
    parts: Vec<Vec<Vec<u8>>>,
}

impl AudioSink for Recorder {
    fn ready(&mut self) -> bool {
        true
    }
    fn send(&mut self, packet: &[u8], _: Instant, _: u64) -> Result<usize> {
        self.packets.push(packet.to_vec());
        Ok(0)
    }
    fn send_frame(&mut self, parts: &[Vec<u8>], _: Instant, _: u64) -> Result<usize> {
        self.parts.push(parts.to_vec());
        Ok(0)
    }
}

/// A WebRTC-like transport, with the trait's own `send_frame`.
struct OpusOnly;

impl AudioSink for OpusOnly {
    fn ready(&mut self) -> bool {
        true
    }
    fn send(&mut self, _: &[u8], _: Instant, _: u64) -> Result<usize> {
        Ok(0)
    }
}

#[test]
fn pcm_goes_out_as_s16le_packets_and_opus_is_not_a_passthrough() {
    let nowhere = Path::new("C:\\no-ffmpeg-here");
    let mut pcm = AudioEncoder::passthrough(nowhere, Codec::Pcm, 2).unwrap();
    let mut sink = Recorder::default();
    let samples = [0.5, -0.5, 0.0, 1.0];
    assert_eq!(
        pcm.send(&mut sink, &samples, Instant::now(), 0, &Default::default())
            .unwrap(),
        0
    );
    assert_eq!(sink.packets, [pcm_s16le(&samples)]);
    assert!(sink.parts.is_empty());
    let error = AudioEncoder::passthrough(nowhere, Codec::Opus, 2)
        .err()
        .unwrap();
    assert_eq!(error.to_string(), "Opus is not a passthrough codec");
}

#[test]
fn opus_and_pcm_need_no_ffmpeg_and_dolby_without_it_is_refused_by_name() {
    let nowhere = Path::new("C:\\no-ffmpeg-here");
    for channels in [2, 6, 8, 12] {
        check(nowhere, 0, channels).unwrap();
        check(nowhere, CODEC_OPUS, channels).unwrap();
        check(nowhere, CODEC_PCM, channels).unwrap();
    }
    let error = format!("{:#}", check(nowhere, CODEC_AC3, 2).unwrap_err());
    assert!(
        error.starts_with("this host cannot encode Dolby Digital (AC3) for 2 audio channels")
            && error.contains("avutil-60.dll"),
        "{error}"
    );
    // More channels than the codec carries are refused before FFmpeg is looked for.
    let error = format!("{:#}", check(nowhere, CODEC_AC3, 8).unwrap_err());
    assert!(error.contains("cannot carry 8"), "{error}");
    assert!(check(nowhere, CODEC_AC3 | CODEC_PCM, 2).is_err());
}

#[test]
fn ffmpeg_probes_once_per_codec_and_channels_and_retries_failures_later() {
    use super::{PROBE_RETRY, PROBE_RETRY_MAX, Probes};
    use std::{cell::RefCell, time::Duration};
    let probes = Probes::default();
    let directory = Path::new("C:\\ffmpeg");
    let start = Instant::now();
    let calls = RefCell::new(Vec::new());
    let get = |codec, channels, after: Duration, works: bool| {
        probes
            .get(directory, codec, channels, start + after, |retry| {
                calls.borrow_mut().push((codec, channels, retry));
                if works {
                    Ok(())
                } else {
                    anyhow::bail!("no FFmpeg")
                }
            })
            .map_err(|error| error.to_string())
    };
    // A success is kept: each ANNOUNCE after the first is a lookup.
    let zero = Duration::ZERO;
    assert!(get(Codec::Ac3, 2, zero, true).is_ok());
    assert!(get(Codec::Ac3, 2, Duration::from_secs(3600), false).is_ok());
    // Each codec and channel count is probed on its own.
    assert!(get(Codec::Ac3, 6, zero, true).is_ok());
    assert_eq!(get(Codec::Eac3, 2, zero, false).unwrap_err(), "no FFmpeg");
    assert_eq!(
        *calls.borrow(),
        [
            (Codec::Ac3, 2, false),
            (Codec::Ac3, 6, false),
            (Codec::Eac3, 2, false)
        ]
    );
    // A failure stands until its retry, which waits twice as long each time, up to the cap.
    calls.borrow_mut().clear();
    let mut at = zero;
    for wait in [10, 20, 40, 80, 160, 320, 600, 600] {
        let wait = Duration::from_secs(wait);
        assert_eq!(
            get(Codec::Eac3, 2, at + wait - Duration::from_millis(1), true).unwrap_err(),
            "no FFmpeg"
        );
        at += wait;
        assert!(get(Codec::Eac3, 2, at, false).is_err());
    }
    assert_eq!(calls.borrow().len(), 8);
    assert!(calls.borrow().iter().all(|&(_, _, retry)| retry));
    assert_eq!(
        (PROBE_RETRY, PROBE_RETRY_MAX),
        (Duration::from_secs(10), Duration::from_secs(600))
    );
    // Once FFmpeg opens, the result is kept.
    at += PROBE_RETRY_MAX;
    assert!(get(Codec::Eac3, 2, at, true).is_ok());
    assert!(get(Codec::Eac3, 2, at, false).is_ok());
    assert_eq!(calls.borrow().len(), 9);
}

#[test]
fn a_launch_is_offered_only_the_dolby_codecs_that_open_for_its_channels() {
    use super::{PROBE_RETRY, Probes, dolby_bits, launch_channels};
    use std::{collections::BTreeMap, time::Duration};
    let bit = |codec| match codec {
        Codec::Ac3 => CODEC_AC3,
        Codec::Eac3 => CODEC_EAC3,
        other => panic!("{other:?} is not probed"),
    };
    // Each codec is probed only for channels it can carry: AC3 up to 5.1, E-AC3 up to 7.1.
    let both = CODEC_AC3 | CODEC_EAC3;
    for (channels, fit) in [(2, both), (6, both), (8, CODEC_EAC3), (12, 0)] {
        let mut probed = 0;
        let bits = dolby_bits(channels, |codec| {
            probed |= bit(codec);
            Some(true)
        });
        assert_eq!((bits, probed), (Some(fit), fit), "{channels} channels");
    }
    // FFmpeg has no E-AC3 7.1: a 7.1 launch is offered neither, a 5.1 one AC3.
    let ac3_only = |codec| Some(codec == Codec::Ac3);
    assert_eq!(dolby_bits(8, ac3_only), Some(0));
    assert_eq!(dolby_bits(6, ac3_only), Some(CODEC_AC3));
    // A probe that is due leaves the kept result unknown.
    assert_eq!(
        dolby_bits(2, |codec| (codec == Codec::Ac3).then_some(true)),
        None
    );

    // What serverinfo reads without loading FFmpeg: a success for good, a failure until
    // its retry is due.
    let probes = Probes::default();
    let directory = Path::new("C:\\ffmpeg");
    let start = Instant::now();
    assert_eq!(probes.peek(directory, Codec::Ac3, 2, start), None);
    probes
        .get(directory, Codec::Ac3, 2, start, |_| Ok(()))
        .unwrap();
    let failed = probes.get(directory, Codec::Eac3, 2, start, |_| {
        anyhow::bail!("no FFmpeg")
    });
    assert!(failed.is_err());
    let hour = Duration::from_secs(3600);
    assert_eq!(
        probes.peek(directory, Codec::Ac3, 2, start + hour),
        Some(true)
    );
    assert_eq!(probes.peek(directory, Codec::Ac3, 6, start), None);
    assert_eq!(probes.peek(directory, Codec::Eac3, 2, start), Some(false));
    assert_eq!(
        probes.peek(directory, Codec::Eac3, 2, start + PROBE_RETRY),
        None
    );

    // The launch's channels, as `/launch` reads `surroundAudioInfo` (mask << 16 | count).
    let launch = |info: Option<String>| {
        launch_channels(&BTreeMap::from_iter(
            info.map(|info| ("surroundAudioInfo".to_owned(), info)),
        ))
    };
    for (info, channels) in [
        (None, 2),
        (Some((3 << 16 | 2).to_string()), 2),
        (Some((63 << 16 | 6).to_string()), 6),
        (Some((1599 << 16 | 8).to_string()), 8),
        (Some("12".to_owned()), 12),
        // 7.1.4 from a Java int, signed and unsigned.
        (Some("-163643380".to_owned()), 12),
        (Some((0xF63F_u32 << 16 | 12).to_string()), 12),
        (Some("4".to_owned()), 2),
        (Some("4294967308".to_owned()), 2),
        (Some("-2147483649".to_owned()), 2),
        (Some("stereo".to_owned()), 2),
    ] {
        assert_eq!(launch(info.clone()), channels, "{info:?}");
    }
}

#[test]
#[ignore = "needs BUTTERPOLLO_TEST_FFMPEG_ROOT"]
fn ffmpeg_dolby_plus_7_1_is_refused_and_the_rest_opens() {
    let root = ffmpeg();
    for (bits, channels) in [
        (CODEC_AC3, 2),
        (CODEC_AC3, 6),
        (CODEC_EAC3, 2),
        (CODEC_EAC3, 6),
    ] {
        check(&root, bits, channels).unwrap();
    }
    let error = format!("{:#}", check(&root, CODEC_EAC3, 8).unwrap_err());
    assert!(
        error
            .starts_with("this host cannot encode Dolby Digital Plus (E-AC3) for 8 audio channels"),
        "{error}"
    );
    // So a 7.1 launch is offered neither, and stereo and 5.1 ones both.
    let both = CODEC_AC3 | CODEC_EAC3;
    for (channels, offered) in [(2, both), (6, both), (8, 0), (12, 0)] {
        assert_eq!(super::dolby_codecs(&root, channels), offered, "{channels}");
    }
}

#[test]
#[ignore = "needs BUTTERPOLLO_TEST_FFMPEG_ROOT"]
fn ffmpeg_dolby_frames_go_out_in_numbered_chunks_and_never_as_opus_packets() {
    let samples = vec![0.25; 1536 * 6];
    // 640 kbit/s AC3 5.1 is 2560 bytes a frame, three chunks.
    let mut ac3 = AudioEncoder::passthrough(&ffmpeg(), Codec::Ac3, 6).unwrap();
    let mut sink = Recorder::default();
    for _ in 0..3 {
        assert_eq!(
            ac3.send(&mut sink, &samples, Instant::now(), 0, &Default::default())
                .unwrap(),
            0
        );
    }
    assert!(sink.packets.is_empty());
    for (index, parts) in sink.parts.iter().enumerate() {
        let headers: Vec<_> = parts
            .iter()
            .map(|part| {
                assert_eq!(part.len(), CHUNK_PAYLOAD_BYTES);
                ChunkHeader::decode(part).unwrap()
            })
            .collect();
        let frame_index = index as u16;
        assert_eq!(
            headers,
            [
                ChunkHeader {
                    first: true,
                    last: false,
                    frame_index
                },
                ChunkHeader {
                    first: false,
                    last: false,
                    frame_index
                },
                ChunkHeader {
                    first: false,
                    last: true,
                    frame_index
                },
            ]
        );
    }
    // A transport without passthrough refuses the frame instead of dropping it.
    let error = ac3
        .send(
            &mut OpusOnly,
            &samples,
            Instant::now(),
            0,
            &Default::default(),
        )
        .unwrap_err();
    assert_eq!(error.to_string(), "this transport carries only Opus audio");
}

fn click_delay(directory: &Path, eac3: bool, channels: u8, bitrate: u32, at: usize) -> usize {
    use butterpollo_win_graphics::ffmpeg::{Ac3Decoder, Ac3Encoder, FRAME_SAMPLES};
    let count = usize::from(channels);
    let mut encoder = Ac3Encoder::new(directory, eac3, channels, bitrate).unwrap();
    let mut decoder = Ac3Decoder::new(directory, eac3).unwrap();
    let mut decoded = Vec::new();
    for frame in 0..4 {
        let mut samples = vec![0.0; FRAME_SAMPLES * count];
        if frame == at / FRAME_SAMPLES {
            samples[(at % FRAME_SAMPLES) * count] = 0.9;
        }
        let (_, mut out) = decoder.decode(&encoder.encode(&samples).unwrap()).unwrap();
        decoded.append(&mut out);
    }
    let peak = (0..decoded.len() / count)
        .max_by(|a, b| {
            decoded[a * count]
                .abs()
                .total_cmp(&decoded[b * count].abs())
        })
        .unwrap();
    peak - at
}

fn spread(times: &mut [std::time::Duration]) -> (f64, f64, f64) {
    times.sort();
    let us = |d: std::time::Duration| d.as_secs_f64() * 1e6;
    (
        us(times[times.len() / 2]),
        us(times[times.len() * 99 / 100]),
        us(times[times.len() - 1]),
    )
}

/// The numbers for rust/PERFORMANCE.md, not a pass/fail check: the host's encode time per
/// packet for AC3 and E-AC3 against Opus, and the AC3 codecs' delay. Run in release:
/// `cargo test --release -p butterpollo --lib -- --ignored --nocapture passthrough_encode_cost`
/// with BUTTERPOLLO_TEST_FFMPEG_ROOT and BUTTERPOLLO_TEST_OPUS_ROOT (libopus-0.dll's folder).
#[test]
#[ignore = "a benchmark; needs BUTTERPOLLO_TEST_FFMPEG_ROOT and BUTTERPOLLO_TEST_OPUS_ROOT"]
fn passthrough_encode_cost() {
    use butterpollo_win_graphics::ffmpeg::{Ac3Encoder, FRAME_SAMPLES};
    use butterpollo_win_io::audio::Opus;
    let root = ffmpeg();
    let opus_root = PathBuf::from(
        std::env::var_os("BUTTERPOLLO_TEST_OPUS_ROOT")
            .expect("set BUTTERPOLLO_TEST_FFMPEG_ROOT and BUTTERPOLLO_TEST_OPUS_ROOT"),
    );
    let seconds = 32;
    let signal = |channels: usize, samples: usize| -> Vec<f32> {
        let mut state = 0x1234_5678u32;
        (0..samples * channels)
            .map(|index| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (state >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
                let t = (index / channels) as f32 / 48_000.0;
                0.3 * (std::f32::consts::TAU * 440.0 * t).sin() + 0.1 * noise
            })
            .collect()
    };
    println!("codec, channels, packet ms, encode median/p99/max us, codec delay samples");
    for (name, eac3, channels, bitrate) in [
        ("AC3", false, 2u8, 192_000u32),
        ("AC3", false, 6, 640_000),
        ("E-AC3", true, 2, 256_000),
        ("E-AC3", true, 6, 768_000),
    ] {
        let count = usize::from(channels);
        let frames = seconds * 48_000 / FRAME_SAMPLES;
        let audio = signal(count, frames * FRAME_SAMPLES);
        let mut encoder = Ac3Encoder::new(&root, eac3, channels, bitrate).unwrap();
        let mut times: Vec<_> = audio
            .chunks_exact(FRAME_SAMPLES * count)
            .map(|samples| {
                let start = Instant::now();
                std::hint::black_box(encoder.encode(samples).unwrap());
                start.elapsed()
            })
            .collect();
        let (median, p99, max) = spread(&mut times);
        let delay = click_delay(&root, eac3, channels, bitrate, FRAME_SAMPLES + 100);
        println!("{name}, {channels}, 32, {median:.1}/{p99:.1}/{max:.1}, {delay}");
    }
    for channels in [2usize, 6] {
        let packet = 240;
        let audio = signal(channels, seconds * 48_000);
        let mut opus = Opus::new(&opus_root, channels, false).unwrap();
        let mut times: Vec<_> = audio
            .chunks_exact(packet * channels)
            .map(|samples| {
                let start = Instant::now();
                std::hint::black_box(opus.encode(samples).unwrap());
                start.elapsed()
            })
            .collect();
        let (median, p99, max) = spread(&mut times);
        println!("Opus, {channels}, 5, {median:.1}/{p99:.1}/{max:.1}, -");
    }
}
