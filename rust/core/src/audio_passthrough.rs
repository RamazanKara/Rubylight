//! Audio passthrough of the 3.0 audio extension: the codec a client's ANNOUNCE picked, PCM's
//! s16le packets and the RTP pieces of an AC3 or E-AC3 frame. `rubylight_protocol::audio_ext`
//! defines the wire; the AC3 and E-AC3 encoder is FFmpeg's (`butterpollo_windows::ffmpeg`).
use anyhow::{Result, bail};
use rubylight_protocol::audio_ext::{self, CHUNK_BYTES, ChunkHeader};

/// What a stream's audio thread sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Opus,
    Pcm,
    Ac3,
    Eac3,
}

impl Codec {
    /// The codec of an ANNOUNCE's `x-rl-audio.codec` (0 or Opus keeps stock Opus), refused
    /// when it can't carry `channels`.
    pub fn from_announce(bits: u8, channels: u8) -> Result<Self> {
        let codec = match bits {
            0 | audio_ext::CODEC_OPUS => Self::Opus,
            audio_ext::CODEC_PCM => Self::Pcm,
            audio_ext::CODEC_AC3 => Self::Ac3,
            audio_ext::CODEC_EAC3 => Self::Eac3,
            other => bail!("the client picked unknown audio codec {other}"),
        };
        if !audio_ext::channels_fit(bits, channels) {
            bail!("{} cannot carry {channels} audio channels", codec.name());
        }
        Ok(codec)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Opus => "Opus",
            Self::Pcm => "PCM",
            Self::Ac3 => "Dolby Digital (AC3)",
            Self::Eac3 => "Dolby Digital Plus (E-AC3)",
        }
    }

    /// Milliseconds of audio each RTP timestamp step covers: the packet duration Opus
    /// negotiated, PCM's fixed packet for `channels`, or one 32 ms AC3 frame.
    pub fn packet_ms(self, channels: u8, opus_ms: u8) -> Result<u8> {
        Ok(match self {
            Self::Opus => opus_ms,
            Self::Pcm => match audio_ext::pcm_packet_ms(channels) {
                Some(ms) => ms,
                None => bail!("PCM cannot carry {channels} audio channels"),
            },
            Self::Ac3 | Self::Eac3 => (audio_ext::AC3_FRAME_SAMPLES / 48) as u8,
        })
    }

    /// The AC3 or E-AC3 bitrate for `channels`; `None` for Opus and PCM.
    pub fn bitrate(self, channels: u8) -> Option<u32> {
        let table = match self {
            Self::Ac3 => audio_ext::AC3_BITRATES,
            Self::Eac3 => audio_ext::EAC3_BITRATES,
            Self::Opus | Self::Pcm => return None,
        };
        table
            .iter()
            .find(|(count, _)| *count == channels)
            .map(|(_, bitrate)| *bitrate)
    }
}

/// Interleaved samples as PCM's s16le, clamped to full scale.
pub fn pcm_s16le(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

/// The RTP payloads of one AC3 or E-AC3 frame: [`CHUNK_BYTES`] of it behind each
/// [`ChunkHeader`], the last zero-padded to the same size.
pub fn chunks(frame: &[u8], frame_index: u16) -> Vec<Vec<u8>> {
    let count = audio_ext::chunks_for(frame.len());
    frame
        .chunks(CHUNK_BYTES)
        .enumerate()
        .map(|(index, piece)| {
            let header = ChunkHeader {
                first: index == 0,
                last: index + 1 == count,
                frame_index,
            };
            let mut payload = Vec::with_capacity(audio_ext::CHUNK_PAYLOAD_BYTES);
            payload.extend_from_slice(&header.encode());
            payload.extend_from_slice(piece);
            payload.resize(audio_ext::CHUNK_PAYLOAD_BYTES, 0);
            payload
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announce_picks_a_codec_only_for_channels_it_carries() {
        assert_eq!(Codec::from_announce(0, 12).unwrap(), Codec::Opus);
        assert_eq!(Codec::from_announce(1, 2).unwrap(), Codec::Opus);
        assert_eq!(Codec::from_announce(2, 12).unwrap(), Codec::Pcm);
        assert_eq!(Codec::from_announce(4, 6).unwrap(), Codec::Ac3);
        assert_eq!(Codec::from_announce(8, 8).unwrap(), Codec::Eac3);
        for (bits, channels) in [(4, 8), (8, 12), (2, 4), (3, 2), (16, 2)] {
            assert!(
                Codec::from_announce(bits, channels).is_err(),
                "{bits} {channels}"
            );
        }
    }

    #[test]
    fn each_codec_steps_the_timestamp_by_its_own_packet() {
        assert_eq!(Codec::Opus.packet_ms(6, 10).unwrap(), 10);
        for (channels, ms) in [(2, 5), (6, 2), (8, 1), (12, 1)] {
            assert_eq!(Codec::Pcm.packet_ms(channels, 5).unwrap(), ms);
        }
        assert_eq!(Codec::Ac3.packet_ms(6, 5).unwrap(), 32);
        assert_eq!(Codec::Eac3.packet_ms(2, 20).unwrap(), 32);
        assert_eq!(Codec::Ac3.bitrate(6), Some(640_000));
        assert_eq!(Codec::Eac3.bitrate(8), Some(1_024_000));
        assert_eq!(Codec::Ac3.bitrate(8), None);
        assert_eq!(Codec::Pcm.bitrate(2), None);
    }

    #[test]
    fn pcm_is_clamped_little_endian_s16() {
        let bytes = pcm_s16le(&[0.0, 1.0, -1.0, 2.0, -2.0, 0.5, f32::NAN]);
        let values: Vec<i16> = bytes
            .chunks(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        assert_eq!(values, [0, 32767, -32767, 32767, -32767, 16384, 0]);
        let packet = vec![0.25; 48 * 2 * 6];
        assert_eq!(
            pcm_s16le(&packet).len(),
            audio_ext::pcm_packet_bytes(6).unwrap()
        );
    }

    #[test]
    fn a_frame_splits_into_equal_chunks_that_reassemble_it() {
        let frame: Vec<u8> = (0..2560u32).map(|byte| byte as u8).collect();
        let pieces = chunks(&frame, 0xFFFF);
        assert_eq!(pieces.len(), 3);
        let mut joined = Vec::new();
        for (index, piece) in pieces.iter().enumerate() {
            assert_eq!(piece.len(), audio_ext::CHUNK_PAYLOAD_BYTES);
            let header = ChunkHeader::decode(piece).unwrap();
            assert_eq!(
                (header.first, header.last, header.frame_index),
                (index == 0, index == 2, 0xFFFF)
            );
            joined.extend_from_slice(&piece[audio_ext::CHUNK_HEADER_BYTES..]);
        }
        assert_eq!(&joined[..frame.len()], &frame[..]);
        assert!(joined[frame.len()..].iter().all(|&byte| byte == 0));
        let single = chunks(&frame[..1024], 7);
        let header = ChunkHeader::decode(&single[0]).unwrap();
        assert!(single.len() == 1 && header.first && header.last);
    }
}
