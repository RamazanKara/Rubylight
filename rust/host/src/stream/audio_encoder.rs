//! The audio thread's encoder: Opus for every stock client, or the PCM, AC3 or E-AC3
//! passthrough a client picked in its ANNOUNCE (`x-rl-audio.codec`).

use super::audio_sink::AudioSink;
use anyhow::{Context, Result, bail};
use butterpollo_core::{
    audio_passthrough::{self, Codec},
    rtsp::Negotiated,
};
use butterpollo_windows::{audio::Opus, ffmpeg::Ac3Encoder};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

/// The wait before a failed FFmpeg probe runs again, doubled after each failure up to
/// [`PROBE_RETRY_MAX`].
const PROBE_RETRY: Duration = Duration::from_secs(10);
const PROBE_RETRY_MAX: Duration = Duration::from_secs(600);

static PROBES: LazyLock<Probes> = LazyLock::new(Probes::default);

enum Probe {
    Ready,
    Failed {
        error: String,
        retry_at: Instant,
        backoff: Duration,
    },
}

/// Whether FFmpeg's AC3 or E-AC3 encoder opens for a channel count, so DESCRIBE and ANNOUNCE
/// don't load FFmpeg each time: a success is kept, a failure until its retry is due, which
/// waits twice as long after each failure (FFmpeg's libraries may be put back while the host
/// runs).
#[derive(Default)]
struct Probes {
    entries: Mutex<HashMap<(PathBuf, &'static str, u8), Probe>>,
}

impl Probes {
    /// The cached result for `codec` and `channels`, or `probe`'s, run with whether it is a
    /// retry. Concurrent callers wait for one probe.
    fn get(
        &self,
        directory: &Path,
        codec: Codec,
        channels: u8,
        now: Instant,
        probe: impl FnOnce(bool) -> Result<()>,
    ) -> Result<()> {
        let key = (directory.to_owned(), codec.name(), channels);
        let mut entries = self.entries.lock().unwrap();
        let backoff = match entries.get(&key) {
            Some(Probe::Ready) => return Ok(()),
            Some(Probe::Failed {
                error, retry_at, ..
            }) if now < *retry_at => bail!("{error}"),
            Some(Probe::Failed { backoff, .. }) => (*backoff * 2).min(PROBE_RETRY_MAX),
            None => PROBE_RETRY,
        };
        let result = probe(backoff > PROBE_RETRY);
        entries.insert(
            key,
            match &result {
                Ok(()) => Probe::Ready,
                Err(error) => Probe::Failed {
                    error: format!("{error:#}"),
                    retry_at: now + backoff,
                    backoff,
                },
            },
        );
        result
    }

    /// The kept result for `codec` and `channels` without probing: `None` while none is kept
    /// or a failure's retry is due.
    fn peek(&self, directory: &Path, codec: Codec, channels: u8, now: Instant) -> Option<bool> {
        let key = (directory.to_owned(), codec.name(), channels);
        match self.entries.lock().unwrap().get(&key) {
            Some(Probe::Ready) => Some(true),
            Some(Probe::Failed { retry_at, .. }) if now < *retry_at => Some(false),
            _ => None,
        }
    }
}

/// The `audio_ext` AC3 and E-AC3 bits that can carry `channels` and whose FFmpeg encoder opens
/// for them ([`dolby_ready`]; blocking): what DESCRIBE offers and ANNOUNCE accepts for a
/// launch, so a 7.1 launch isn't offered the E-AC3 FFmpeg can't encode.
pub(crate) fn dolby_codecs(directory: &Path, channels: u8) -> u8 {
    dolby_bits(channels, |codec| {
        Some(dolby_ready(directory, codec, channels).is_ok())
    })
    .unwrap_or_default()
}

/// [`dolby_codecs`] from the kept probes, without loading FFmpeg: `None` while a probe is due.
pub(crate) fn dolby_codecs_probed(directory: &Path, channels: u8) -> Option<u8> {
    dolby_bits(channels, |codec| {
        PROBES.peek(directory, codec, channels, Instant::now())
    })
}

/// The AC3 and E-AC3 bits that fit `channels` and `opens`, or `None` when `opens` doesn't know.
fn dolby_bits(channels: u8, mut opens: impl FnMut(Codec) -> Option<bool>) -> Option<u8> {
    use butterpollo_core::audio_ext::{CODEC_AC3, CODEC_EAC3, channels_fit};
    let mut bits = 0;
    for (bit, codec) in [(CODEC_AC3, Codec::Ac3), (CODEC_EAC3, Codec::Eac3)] {
        if channels_fit(bit, channels) && opens(codec)? {
            bits |= bit;
        }
    }
    Some(bits)
}

/// The channel count a launch's `surroundAudioInfo` asks for, read as `/launch` reads it: 2, 6,
/// 8 or 12 (7.1.4), and stereo for anything else.
pub(crate) fn launch_channels(options: &BTreeMap<String, String>) -> u8 {
    let channels = options
        .get("surroundAudioInfo")
        .and_then(|info| info.parse::<u32>().ok())
        .map_or(2, |info| info & 0xffff);
    match channels {
        2 | 6 | 8 | 12 => channels as u8,
        _ => 2,
    }
}

/// Opens FFmpeg's encoder for `codec` and `channels` once (blocking), and again after a failure
/// once its retry is due. A failure is logged, as a warning the first time.
fn dolby_ready(directory: &Path, codec: Codec, channels: u8) -> Result<()> {
    PROBES.get(directory, codec, channels, Instant::now(), |retry| {
        let result = dolby(directory, codec, channels).map(drop);
        if let Err(error) = &result {
            let error = format!("{error:#}");
            let codec = codec.name();
            if retry {
                tracing::debug!(%error, codec, channels, "FFmpeg's encoder still does not open");
            } else {
                tracing::warn!(%error, codec, channels, "FFmpeg's encoder does not open: the codec is not offered for these channels");
            }
        }
        result
    })
}

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
/// has no E-AC3 7.1) by opening the encoder the stream would use, once per codec and channel
/// count ([`dolby_ready`]; blocking), so the client hears why before it sets up its audio, and
/// no stream starts with other audio than it picked.
pub(crate) fn check_announce(directory: &Path, n: &Negotiated) -> Result<()> {
    check(directory, n.audio_codec, n.audio_channels)
}

fn check(directory: &Path, bits: u8, channels: u8) -> Result<()> {
    let codec = Codec::from_announce(bits, channels)?;
    match codec {
        Codec::Opus | Codec::Pcm => Ok(()),
        Codec::Ac3 | Codec::Eac3 => dolby_ready(directory, codec, channels).with_context(|| {
            format!(
                "this host cannot encode {} for {channels} audio channels",
                codec.name()
            )
        }),
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
            (Some("4".to_owned()), 2),
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
            error.starts_with(
                "this host cannot encode Dolby Digital Plus (E-AC3) for 8 audio channels"
            ),
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
