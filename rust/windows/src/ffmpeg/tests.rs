//! The ignored tests load FFmpeg from `BUTTERPOLLO_TEST_FFMPEG_ROOT` (its `bin` directory);
//! build.ps1 sets it and runs them with `--ignored ffmpeg::`.
use super::*;
use std::path::PathBuf;
use std::time::Instant;

fn root() -> PathBuf {
    std::env::var_os("BUTTERPOLLO_TEST_FFMPEG_ROOT")
        .map(PathBuf::from)
        .expect("set BUTTERPOLLO_TEST_FFMPEG_ROOT to the FFmpeg build's bin directory")
}

/// FFmpeg's AC3 or E-AC3 decoder, as a receiver decodes what the host sends.
struct Decoder(Codec);

impl Decoder {
    fn new(directory: &Path, eac3: bool) -> Result<Self> {
        let name = if eac3 { c"eac3" } else { c"ac3" };
        let (codec, found) = Codec::alloc(directory, "AC3 decoder", |ff| {
            // SAFETY: the name is a C string.
            unsafe { (ff.find_decoder)(name.as_ptr()) }
        })?;
        // SAFETY: the context is live and not open yet.
        let code = unsafe { (codec.ff.open)(codec.context, found, std::ptr::null_mut()) };
        codec.ff.check(code, "opening the decoder")?;
        Ok(Self(codec))
    }

    /// One frame's channel count and interleaved samples.
    fn decode(&mut self, frame: &[u8]) -> Result<(usize, Vec<f32>)> {
        let codec = &mut self.0;
        let mut samples = Vec::new();
        let mut channels = 0;
        // SAFETY: av_new_packet makes `frame.len()` writable bytes at the packet's data; the
        // decoded frame's fields are read at FFmpeg 8's offsets, `channels` planes of
        // `count` floats each.
        unsafe {
            let code = (codec.ff.new_packet)(codec.packet, frame.len() as c_int);
            codec.ff.check(code, "allocating a packet")?;
            let data = *codec.packet.cast::<u8>().add(PACKET_DATA).cast::<*mut u8>();
            std::ptr::copy_nonoverlapping(frame.as_ptr(), data, frame.len());
            let code = (codec.ff.send_packet)(codec.context, codec.packet);
            (codec.ff.packet_unref)(codec.packet);
            codec.ff.check(code, "decoder input")?;
            loop {
                let code = (codec.ff.receive_frame)(codec.context, codec.frame);
                if code == AGAIN {
                    break;
                }
                codec.ff.check(code, "decoder output")?;
                let decoded = codec.frame.cast::<u8>();
                assert_eq!(*decoded.add(FRAME_FORMAT).cast::<c_int>(), SAMPLE_FMT_FLTP);
                let count = *decoded.add(FRAME_NB_SAMPLES).cast::<c_int>() as usize;
                let layout = &*decoded.add(FRAME_CH_LAYOUT).cast::<ChannelLayout>();
                channels = layout.nb_channels as usize;
                let planes = decoded.add(FRAME_DATA).cast::<*const f32>();
                for index in 0..count {
                    for channel in 0..channels {
                        samples.push(*(*planes.add(channel)).add(index));
                    }
                }
            }
        }
        Ok((channels, samples))
    }
}

/// `frames` frames of interleaved silence with a 0.5 sine on `active`: 1 kHz, or 50 Hz on
/// 5.1's LFE (channel 3), which AC3 low-passes at 120 Hz.
fn tone(channels: usize, active: usize, frames: usize) -> Vec<Vec<f32>> {
    let hz = if channels == 6 && active == 3 {
        50.0
    } else {
        1000.0
    };
    (0..frames)
        .map(|frame| {
            let mut samples = vec![0.0; FRAME_SAMPLES * channels];
            for index in 0..FRAME_SAMPLES {
                let t = (frame * FRAME_SAMPLES + index) as f32 / 48_000.0;
                samples[index * channels + active] = 0.5 * (std::f32::consts::TAU * hz * t).sin();
            }
            samples
        })
        .collect()
}

fn rms(samples: &[f32], channels: usize, channel: usize) -> f32 {
    let values: Vec<f32> = samples
        .iter()
        .skip(channel)
        .step_by(channels)
        .copied()
        .collect();
    (values.iter().map(|value| value * value).sum::<f32>() / values.len() as f32).sqrt()
}

#[test]
fn channel_masks_are_the_captures_layouts() {
    for (channels, mask) in [(2, 0x3u64), (6, 0x3F), (8, 0x63F)] {
        assert_eq!(channel_mask(channels), Some(mask));
        assert_eq!(mask.count_ones(), u32::from(channels));
    }
    assert_eq!(channel_mask(12), None);
    assert_eq!(channel_mask(1), None);
}

#[test]
fn a_missing_ffmpeg_is_refused_by_name() {
    let empty = std::env::temp_dir().join(format!("butterpollo-no-ffmpeg-{}", std::process::id()));
    std::fs::create_dir_all(&empty).unwrap();
    let error = format!("{:#}", probe(&empty).unwrap_err());
    std::fs::remove_dir_all(&empty).unwrap();
    assert!(error.contains(AVUTIL), "{error}");
}

/// `LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR` takes only absolute paths: a relative folder is made one.
#[test]
fn ffmpeg_is_loaded_by_its_absolute_path() {
    let path = beside(Path::new("no-ffmpeg-here"), AVUTIL).unwrap();
    assert!(path.is_absolute(), "{}", path.display());
    assert!(path.ends_with(Path::new("no-ffmpeg-here").join(AVUTIL)));
    let error = format!("{:#}", probe(Path::new("no-ffmpeg-here")).unwrap_err());
    assert!(error.contains(&path.display().to_string()), "{error}");
}

#[test]
#[ignore = "needs BUTTERPOLLO_TEST_FFMPEG_ROOT"]
fn ffmpeg_loads_from_a_folder_named_without_its_drive() {
    // `\src\...` is relative on Windows (no drive), and loads only once made absolute.
    let root = root();
    let relative: PathBuf = root
        .components()
        .filter(|c| !matches!(c, std::path::Component::Prefix(_)))
        .collect();
    assert!(!relative.is_absolute());
    probe(&relative).unwrap();
}

#[test]
#[ignore = "needs BUTTERPOLLO_TEST_FFMPEG_ROOT"]
fn each_channel_round_trips_at_the_protocols_bitrates() {
    let root = root();
    for (eac3, channels, bitrate) in [
        (false, 2u8, 192_000u32),
        (false, 6, 640_000),
        (true, 2, 256_000),
        (true, 6, 768_000),
    ] {
        let count = usize::from(channels);
        for active in 0..count {
            let mut encoder = Ac3Encoder::new(&root, eac3, channels, bitrate).unwrap();
            let mut decoder = Decoder::new(&root, eac3).unwrap();
            let mut decoded = Vec::new();
            for samples in tone(count, active, 10) {
                let frame = encoder.encode(&samples).unwrap();
                // A constant bitrate: 32 ms of it per frame.
                assert_eq!(
                    frame.len(),
                    bitrate as usize * 32 / 8000,
                    "{eac3} {channels}"
                );
                let (decoded_channels, mut samples) = decoder.decode(&frame).unwrap();
                assert_eq!(decoded_channels, count);
                decoded.append(&mut samples);
            }
            // Past the encoder's delay, every channel decodes where it was sent.
            let settled = &decoded[2 * FRAME_SAMPLES * count..];
            let expected = 0.5 / std::f32::consts::SQRT_2;
            for channel in 0..count {
                let level = rms(settled, count, channel);
                if channel == active {
                    assert!(
                        (level - expected).abs() < 0.1 * expected,
                        "{eac3} {channels} channel {channel}: {level}"
                    );
                } else {
                    assert!(
                        level < 0.05 * expected,
                        "{eac3} {channels} channel {channel} took {level} of channel {active}"
                    );
                }
            }
        }
    }
}

#[test]
#[ignore = "needs BUTTERPOLLO_TEST_FFMPEG_ROOT"]
fn seven_one_is_refused_with_ffmpegs_reason() {
    for (eac3, bitrate) in [(true, 1_024_000), (false, 640_000)] {
        let error = format!(
            "{:#}",
            Ac3Encoder::new(&root(), eac3, 8, bitrate).err().unwrap()
        );
        assert!(error.contains("for 8 channels"), "{error}");
    }
    let error = format!(
        "{:#}",
        Ac3Encoder::new(&root(), false, 12, 640_000).err().unwrap()
    );
    assert!(error.contains("not 12"), "{error}");
}

#[test]
#[ignore = "needs BUTTERPOLLO_TEST_FFMPEG_ROOT"]
fn the_probe_opens_both_encoders_and_frames_must_be_whole() {
    probe(&root()).unwrap();
    let mut encoder = Ac3Encoder::new(&root(), false, 2, 192_000).unwrap();
    assert!(encoder.encode(&[0.0; 2 * 480]).is_err());
    assert_eq!(
        encoder.encode(&[0.0; 2 * FRAME_SAMPLES]).unwrap().len(),
        768
    );
}

/// Where a click at `at` (interleaved frame index, channel 0) comes out after encoding and
/// decoding: the codec delay a receiver adds, in samples.
fn click_delay(directory: &Path, eac3: bool, channels: u8, bitrate: u32, at: usize) -> usize {
    let count = usize::from(channels);
    let mut encoder = Ac3Encoder::new(directory, eac3, channels, bitrate).unwrap();
    let mut decoder = Decoder::new(directory, eac3).unwrap();
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

/// Median, 99th percentile and largest of `times`, in microseconds.
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
/// `cargo test --release -p butterpollo-windows --lib -- --ignored --nocapture passthrough_encode_cost`
/// with BUTTERPOLLO_TEST_FFMPEG_ROOT and BUTTERPOLLO_TEST_OPUS_ROOT (libopus-0.dll's folder).
#[test]
#[ignore = "a benchmark; needs BUTTERPOLLO_TEST_FFMPEG_ROOT and BUTTERPOLLO_TEST_OPUS_ROOT"]
fn passthrough_encode_cost() {
    let root = root();
    let opus_root = PathBuf::from(
        std::env::var_os("BUTTERPOLLO_TEST_OPUS_ROOT")
            .expect("set BUTTERPOLLO_TEST_OPUS_ROOT to libopus-0.dll's folder"),
    );
    // 32 s of audio per encoder: music-like, a tone over noise on every channel.
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
        let mut opus = crate::audio::Opus::new(&opus_root, channels, false).unwrap();
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
