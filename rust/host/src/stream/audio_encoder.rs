//! The audio thread's encoder: Opus for every stock client, or the PCM, AC3 or E-AC3
//! passthrough a client picked in its ANNOUNCE (`x-rl-audio.codec`).

use super::audio_sink::AudioSink;
use anyhow::{Context, Result, bail};
use butterpollo_core::{
    audio_passthrough::{self, Codec},
    rtsp::Negotiated,
};
use butterpollo_windows::{audio::Opus, ffmpeg::Ac3Encoder};
use std::{path::Path, time::Instant};

pub(super) enum AudioEncoder {
    Opus(Opus),
    Pcm,
    Dolby {
        encoder: Ac3Encoder,
        /// The next frame's [`butterpollo_core::audio_ext::ChunkHeader`] index.
        frame: u16,
    },
}

impl AudioEncoder {
    /// The passthrough encoder for `codec`; Opus is opened by `open_audio_encoder`.
    pub(super) fn passthrough(directory: &Path, codec: Codec, channels: u8) -> Result<Self> {
        Ok(match codec {
            Codec::Opus => bail!("Opus is not a passthrough codec"),
            Codec::Pcm => Self::Pcm,
            Codec::Ac3 | Codec::Eac3 => Self::Dolby {
                encoder: dolby(directory, codec, channels)?,
                frame: 0,
            },
        })
    }

    /// Encodes one packet's samples (one AC3 frame for AC3 and E-AC3) and sends it, returning
    /// how many RTP packets did not go out.
    pub(super) fn send(
        &mut self,
        output: &mut dyn AudioSink,
        samples: &[f32],
        captured: Instant,
        position: u64,
    ) -> Result<usize> {
        match self {
            Self::Opus(opus) => output.send(&opus.encode(samples)?, captured, position),
            Self::Pcm => output.send(&audio_passthrough::pcm_s16le(samples), captured, position),
            Self::Dolby { encoder, frame } => {
                let parts = audio_passthrough::chunks(&encoder.encode(samples)?, *frame);
                *frame = frame.wrapping_add(1);
                output.send_frame(&parts, captured, position)
            }
        }
    }
}

/// FFmpeg's AC3 or E-AC3 encoder at the protocol's bitrate for `channels`.
fn dolby(directory: &Path, codec: Codec, channels: u8) -> Result<Ac3Encoder> {
    let Some(bitrate) = codec.bitrate(channels) else {
        bail!("{} cannot carry {channels} audio channels", codec.name());
    };
    Ac3Encoder::new(directory, codec == Codec::Eac3, channels, bitrate)
}

/// Refuses an ANNOUNCE whose passthrough codec this host can't encode for its channels (FFmpeg
/// has no E-AC3 7.1) by opening the encoder the stream would use, so the client hears why before
/// it sets up its audio, and no stream starts with other audio than it picked.
pub(crate) fn check_announce(directory: &Path, n: &Negotiated) -> Result<()> {
    check(directory, n.audio_codec, n.audio_channels)
}

fn check(directory: &Path, bits: u8, channels: u8) -> Result<()> {
    let codec = Codec::from_announce(bits, channels)?;
    match codec {
        Codec::Opus | Codec::Pcm => Ok(()),
        Codec::Ac3 | Codec::Eac3 => {
            dolby(directory, codec, channels)
                .map(drop)
                .with_context(|| {
                    format!(
                        "this host cannot encode {} for {channels} audio channels",
                        codec.name()
                    )
                })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AudioEncoder, AudioSink, check};
    use anyhow::Result;
    use butterpollo_core::audio_ext::{
        CHUNK_PAYLOAD_BYTES, CODEC_AC3, CODEC_EAC3, CODEC_OPUS, CODEC_PCM, ChunkHeader,
    };
    use butterpollo_core::audio_passthrough::{Codec, pcm_s16le};
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
        assert_eq!(pcm.send(&mut sink, &samples, Instant::now(), 0).unwrap(), 0);
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
            error.starts_with(
                "this host cannot encode Dolby Digital Plus (E-AC3) for 8 audio channels"
            ),
            "{error}"
        );
    }

    #[test]
    #[ignore = "needs BUTTERPOLLO_TEST_FFMPEG_ROOT"]
    fn ffmpeg_dolby_frames_go_out_in_numbered_chunks_and_never_as_opus_packets() {
        let samples = vec![0.25; 1536 * 6];
        // 640 kbit/s AC3 5.1 is 2560 bytes a frame, three chunks.
        let mut ac3 = AudioEncoder::passthrough(&ffmpeg(), Codec::Ac3, 6).unwrap();
        let mut sink = Recorder::default();
        for _ in 0..3 {
            assert_eq!(ac3.send(&mut sink, &samples, Instant::now(), 0).unwrap(), 0);
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
            .send(&mut OpusOnly, &samples, Instant::now(), 0)
            .unwrap_err();
        assert_eq!(error.to_string(), "this transport carries only Opus audio");
    }

    /// The host's chunks and RTP packets (every chunk of a frame on its timestamp) through
    /// Nightjar's receiver: whole frames in order, a lost chunk recovered by FEC, and the frames
    /// of a block past recovery reported lost, once each.
    #[test]
    fn host_dolby_packets_come_out_of_nightjar_as_whole_frames() {
        use butterpollo_core::{audio_passthrough::chunks, packet::AudioPacketizer};
        use nightjar_core::audio::{AudioConfig, AudioOutput, AudioReceiver};
        // 640 kbit/s AC3 5.1 frames, 2560 bytes and three chunks each.
        let frames: Vec<Vec<u8>> = (0..40u8)
            .map(|index| {
                let mut frame = vec![index; 2560];
                frame[..6].copy_from_slice(&[0x0B, 0x77, 0, 0, 36, 8 << 3]);
                frame
            })
            .collect();
        let mut packetizer = AudioPacketizer::new([0; 16], 0, false, 32);
        // The datagrams of each frame, sent at once every 32 ms.
        let sent: Vec<Vec<Vec<u8>>> = frames
            .iter()
            .enumerate()
            .map(|(index, frame)| {
                let parts = chunks(frame, index as u16);
                let last = parts.len() - 1;
                parts
                    .iter()
                    .enumerate()
                    .flat_map(|(part, piece)| packetizer.encode_part(piece, part == last).unwrap())
                    .collect()
            })
            .collect();
        let seq = |p: &[u8]| (p[1], u16::from_be_bytes([p[2], p[3]]));
        let mut receiver = AudioReceiver::new(AudioConfig {
            packet_duration_ms: 32,
            app_version: [7, 1, 431, -1],
            encryption: None,
        });
        receiver.assemble_frames();
        let mut played = Vec::new();
        for (index, datagrams) in sent.iter().enumerate() {
            for datagram in datagrams {
                // Seq 61 (frame 20) is recovered from its block's parity. Seq 64 to 66 (frame
                // 21 and 22) and that block's parity are lost: frames 21 and 22 are lost.
                match seq(datagram) {
                    (97, 61 | 64 | 65 | 66) | (127, 68 | 69) => continue,
                    _ => {}
                }
                receiver.receive(datagram, index as u64 * 32_000, &mut |out| {
                    played.push(match out {
                        AudioOutput::Packet(frame) => Some(frame.to_vec()),
                        AudioOutput::Lost => None,
                    })
                });
            }
            if index == 0 {
                // The first poll timeout ends the backlog drop.
                receiver.receive_timeout();
            }
        }
        let stats = receiver.stats();
        assert_eq!(stats.packet_count_fec_recovered, 1);
        assert_eq!(stats.packet_count_fec_failed, 1);
        assert_eq!(stats.packet_count_invalid, 0);
        let first = frames.len() - played.len();
        let mut expected: Vec<_> = frames[first..].iter().cloned().map(Some).collect();
        expected.splice(21 - first..23 - first, [None, None]);
        // Frame 0 is the dropped backlog, and frame 1's first chunk synchronizes the queue.
        assert_eq!(first, 2);
        assert!(
            played == expected,
            "frames out of order, missing or changed"
        );
    }
}
