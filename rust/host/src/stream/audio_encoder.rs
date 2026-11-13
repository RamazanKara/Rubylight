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

/// The folder FFmpeg's libraries load from: the host executable's.
pub(crate) fn ffmpeg_directory() -> Result<PathBuf> {
    let exe = std::env::current_exe()
        .context("the host's own path, where FFmpeg's libraries load from, is unknown")?;
    exe.parent().map(Path::to_owned).with_context(|| {
        format!(
            "{} has no folder for FFmpeg's libraries to load from",
            exe.display()
        )
    })
}

/// A launch's `surroundAudioInfo` (mask << 16 | count). Java clients build it as an `int`, so
/// 7.1.4's mask 0xF63F can arrive negative (`-163643380`): the signed form is read as the same
/// 32 bits. `None` for anything that isn't a 32-bit number.
pub(crate) fn surround_audio_info(value: &str) -> Option<u32> {
    let value = value.parse::<i64>().ok()?;
    (i64::from(i32::MIN)..=i64::from(u32::MAX))
        .contains(&value)
        .then_some(value as u32)
}

/// The channel count a launch's `surroundAudioInfo` asks for, read as `/launch` reads it: 2, 6,
/// 8 or 12 (7.1.4), and stereo for anything else.
pub(crate) fn launch_channels(options: &BTreeMap<String, String>) -> u8 {
    surround_channels(options.get("surroundAudioInfo").map(String::as_str))
}

/// [`launch_channels`] from the `surroundAudioInfo` value itself.
pub(crate) fn surround_channels(info: Option<&str>) -> u8 {
    let channels = info
        .and_then(surround_audio_info)
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
mod tests;
