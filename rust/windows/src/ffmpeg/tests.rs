//! The ignored tests load FFmpeg from `BUTTERPOLLO_TEST_FFMPEG_ROOT` (its `bin` directory);
//! build.ps1 sets it and runs them with `--ignored ffmpeg::`.
use super::*;
use std::path::PathBuf;

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
