//! Audio beyond stock Opus: PCM, AC3 and E-AC3 passthrough codecs, and 7.1.4.
//!
//! A host lists its codecs in the serverinfo element [`CODECS_SERVERINFO_FIELD`] and, per
//! stream, in the DESCRIBE attribute [`CODECS_DESCRIBE_ATTRIBUTE`] (Opus is always set). A
//! client picks one with the ANNOUNCE attribute [`CODEC_ANNOUNCE_ATTRIBUTE`]; 0, Opus or no
//! attribute keeps stock Opus. 7.1.4 shows as bit [`LAYOUT_MASK_714`] of
//! [`LAYOUTS_SERVERINFO_FIELD`] and as the two Opus layouts [`LAYOUT_714`] and
//! [`LAYOUT_714_HQ`] in DESCRIBE.
//!
//! PCM is s16le, interleaved in Moonlight's channel order, on RTP payload type 97 with
//! [`pcm_packet_ms`] per packet so each packet stays within 1200 bytes. AC3 and E-AC3 frames
//! (1536 samples, 32 ms) go out in [`CHUNK_BYTES`] pieces behind a [`ChunkHeader`]; the last
//! piece of a frame is zero-padded, and all pieces carry the frame's RTP timestamp.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

/// Codec bits.
pub const CODEC_OPUS: u8 = 1;
pub const CODEC_PCM: u8 = 2;
pub const CODEC_AC3: u8 = 4;
pub const CODEC_EAC3: u8 = 8;
/// Every codec bit this version defines.
pub const CODECS_ALL: u8 = CODEC_OPUS | CODEC_PCM | CODEC_AC3 | CODEC_EAC3;

/// Layout bits of [`LAYOUTS_SERVERINFO_FIELD`]. A host sends the field only when it offers
/// 7.1.4, so its value is then 15.
pub const LAYOUT_MASK_STEREO: u8 = 1;
pub const LAYOUT_MASK_51: u8 = 2;
pub const LAYOUT_MASK_71: u8 = 4;
pub const LAYOUT_MASK_714: u8 = 8;

/// Serverinfo element: the codecs the host can send, decimal.
pub const CODECS_SERVERINFO_FIELD: &str = "AudioCodecs";
/// Serverinfo element: the channel layouts the host can send, decimal.
pub const LAYOUTS_SERVERINFO_FIELD: &str = "AudioLayouts";
/// DESCRIBE attribute: the codecs this stream can use.
pub const CODECS_DESCRIBE_ATTRIBUTE: &str = "x-rl-audio.codecs";
/// DESCRIBE attributes: the 7.1.4 Opus layouts at normal and high quality.
pub const LAYOUT_714_DESCRIBE_ATTRIBUTE: &str = "x-rl-audio.layout714";
pub const LAYOUT_714_HQ_DESCRIBE_ATTRIBUTE: &str = "x-rl-audio.layout714Hq";
/// ANNOUNCE attribute: the one codec the client picked.
pub const CODEC_ANNOUNCE_ATTRIBUTE: &str = "x-rl-audio.codec";

/// 7.1.4's channel count and its WAVEFORMATEXTENSIBLE mask (FL FR FC LFE BL BR SL SR TFL TFR
/// TBL TBR), which the host captures with.
pub const CHANNELS_714: u8 = 12;
pub const CHANNEL_MASK_714: u32 = 0x2D63F;
/// The mask a client sends for 7.1.4 in the launch's `surroundAudioInfo` and ANNOUNCE's
/// `x-nv-audio.surround.channelMask`: Moonlight V+'s `AUDIO_CONFIGURATION_714_SURROUND`
/// (its moonlight-common-c `Limelight.h`), which fits the launch's 16 bits. Hosts read the
/// channel count only.
pub const WIRE_CHANNEL_MASK_714: u16 = 0xF63F;
/// Opus bitrates of [`LAYOUT_714`] and [`LAYOUT_714_HQ`], Foundation Sunshine's.
pub const BITRATE_714: u32 = 600_000;
pub const BITRATE_714_HQ: u32 = 3_072_000;

/// Bitrates the host encodes AC3 and E-AC3 at, by channel count.
pub const AC3_BITRATES: &[(u8, u32)] = &[(2, 192_000), (6, 640_000)];
pub const EAC3_BITRATES: &[(u8, u32)] = &[(2, 256_000), (6, 768_000), (8, 1_024_000)];
/// Samples per AC3 or E-AC3 frame (32 ms at 48 kHz).
pub const AC3_FRAME_SAMPLES: u32 = 1536;
/// Frame bytes per chunk, after the [`ChunkHeader`].
pub const CHUNK_BYTES: usize = 1024;
/// Encoded size of a [`ChunkHeader`].
pub const CHUNK_HEADER_BYTES: usize = 4;
/// Every AC3 or E-AC3 RTP payload's size.
pub const CHUNK_PAYLOAD_BYTES: usize = CHUNK_HEADER_BYTES + CHUNK_BYTES;

const VERSION: u8 = 1;

/// Whether `codec` (one bit, or 0 for Opus) can carry `channels`.
pub fn channels_fit(codec: u8, channels: u8) -> bool {
    match codec {
        0 | CODEC_OPUS | CODEC_PCM => matches!(channels, 2 | 6 | 8 | 12),
        CODEC_AC3 => matches!(channels, 2 | 6),
        CODEC_EAC3 => matches!(channels, 2 | 6 | 8),
        _ => false,
    }
}

/// Milliseconds of PCM per packet for `channels`, so a packet stays within 1200 bytes.
pub fn pcm_packet_ms(channels: u8) -> Option<u8> {
    match channels {
        2 => Some(5),
        6 => Some(2),
        8 | 12 => Some(1),
        _ => None,
    }
}

/// Bytes of 48 kHz s16le PCM per packet for `channels`.
pub fn pcm_packet_bytes(channels: u8) -> Option<usize> {
    pcm_packet_ms(channels).map(|ms| 48 * usize::from(ms) * usize::from(channels) * 2)
}

/// Chunks an AC3 or E-AC3 frame of `len` bytes takes.
pub fn chunks_for(len: usize) -> usize {
    len.div_ceil(CHUNK_BYTES)
}

/// Header of one AC3 or E-AC3 chunk: version, flags (bit 0 first chunk of the frame, bit 1
/// last), and the frame's index, which wraps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChunkHeader {
    pub first: bool,
    pub last: bool,
    pub frame_index: u16,
}

impl ChunkHeader {
    const FIRST: u8 = 1;
    const LAST: u8 = 2;

    pub fn encode(&self) -> [u8; CHUNK_HEADER_BYTES] {
        let flags =
            if self.first { Self::FIRST } else { 0 } | if self.last { Self::LAST } else { 0 };
        let index = self.frame_index.to_le_bytes();
        [VERSION, flags, index[0], index[1]]
    }

    /// Parses the header at the start of a chunk payload; version 0 rejects it.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < CHUNK_HEADER_BYTES || data[0] == 0 {
            return None;
        }
        Some(Self {
            first: data[1] & Self::FIRST != 0,
            last: data[1] & Self::LAST != 0,
            frame_index: u16::from_le_bytes([data[2], data[3]]),
        })
    }
}

/// An Opus multistream layout of 12 channels: channel count, streams, coupled streams and
/// the channel mapping, as in `a=x-rl-audio.layout714:12,8,4,0,1,2,3,4,5,6,7,8,9,10,11`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub channels: u8,
    pub streams: u8,
    pub coupled: u8,
    pub mapping: [u8; 12],
}

/// 7.1.4 at [`BITRATE_714`]: Foundation Sunshine's layout (`audio.cpp` `stream_configs`,
/// `map_surround714`), which Moonlight V+ also assumes when a host sends no parameters. Four
/// coupled streams (FL FR, FC LFE, BL BR, SL SR) and the four heights mono, mapped in order.
pub const LAYOUT_714: Layout = Layout {
    channels: 12,
    streams: 8,
    coupled: 4,
    mapping: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
};
/// 7.1.4 at [`BITRATE_714_HQ`]: one mono stream per channel.
pub const LAYOUT_714_HQ: Layout = Layout {
    channels: 12,
    streams: 12,
    coupled: 0,
    mapping: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
};

impl Layout {
    /// Parses the attribute value; anything but 12 channels in a consistent layout is `None`.
    pub fn parse(text: &str) -> Option<Self> {
        let mut numbers = text.split(',').map(|part| {
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            part.parse::<u8>().ok()
        });
        let mut next = || numbers.next().flatten();
        let (channels, streams, coupled) = (next()?, next()?, next()?);
        let mut mapping = [0; 12];
        for entry in &mut mapping {
            *entry = next()?;
        }
        if numbers.next().is_some() {
            return None;
        }
        let layout = Self {
            channels,
            streams,
            coupled,
            mapping,
        };
        layout.valid().then_some(layout)
    }

    /// The attribute value [`parse`](Self::parse) reads back.
    pub fn to_attr(&self) -> String {
        let mut out = String::new();
        let numbers = [self.channels, self.streams, self.coupled]
            .into_iter()
            .chain(self.mapping);
        for (index, number) in numbers.enumerate() {
            let separator = if index == 0 { "" } else { "," };
            let _ = write!(out, "{separator}{number}");
        }
        out
    }

    fn valid(&self) -> bool {
        self.channels == CHANNELS_714
            && self.coupled <= self.streams
            && u16::from(self.streams) + u16::from(self.coupled) == u16::from(self.channels)
            && self.mapping.iter().all(|&entry| entry < self.channels)
    }
}

/// The DESCRIBE lines that offer 7.1.4: ours, then Foundation Sunshine's `surround-params`
/// in its comma form (mapping entries reach 11), normal before high quality as
/// moonlight-common-c reads them. Stock clients look up `surround-params=6` or `8` only, so
/// they skip the `12,` lines.
pub fn layout_714_lines() -> Vec<String> {
    let ours = [
        (LAYOUT_714_DESCRIBE_ATTRIBUTE, LAYOUT_714),
        (LAYOUT_714_HQ_DESCRIBE_ATTRIBUTE, LAYOUT_714_HQ),
    ]
    .map(|(attribute, layout)| alloc::format!("a={attribute}:{}\r\n", layout.to_attr()));
    let surround_params = [LAYOUT_714, LAYOUT_714_HQ]
        .map(|layout| alloc::format!("a=fmtp:97 surround-params={}\r\n", layout.to_attr()));
    ours.into_iter().chain(surround_params).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_and_layout_bits_are_distinct() {
        let codecs = [CODEC_OPUS, CODEC_PCM, CODEC_AC3, CODEC_EAC3];
        assert_eq!(codecs.iter().fold(0, |all, bit| all | bit), CODECS_ALL);
        assert_eq!(codecs.iter().map(|bit| bit.count_ones()).sum::<u32>(), 4);
        let layouts = [
            LAYOUT_MASK_STEREO,
            LAYOUT_MASK_51,
            LAYOUT_MASK_71,
            LAYOUT_MASK_714,
        ];
        assert_eq!(layouts.iter().fold(0, |all, bit| all | bit), 15);
        assert_eq!(CHANNEL_MASK_714.count_ones(), 12);
    }

    #[test]
    fn codecs_take_their_channel_counts() {
        for channels in [2, 6, 8, 12] {
            assert!(channels_fit(0, channels));
            assert!(channels_fit(CODEC_OPUS, channels));
            assert!(channels_fit(CODEC_PCM, channels));
        }
        assert!(channels_fit(CODEC_AC3, 6) && !channels_fit(CODEC_AC3, 8));
        assert!(channels_fit(CODEC_EAC3, 8) && !channels_fit(CODEC_EAC3, 12));
        assert!(!channels_fit(CODEC_PCM, 4) && !channels_fit(CODEC_PCM | CODEC_AC3, 2));
        for (channels, _) in AC3_BITRATES {
            assert!(channels_fit(CODEC_AC3, *channels));
        }
        for (channels, _) in EAC3_BITRATES {
            assert!(channels_fit(CODEC_EAC3, *channels));
        }
    }

    #[test]
    fn pcm_packets_stay_within_1200_bytes() {
        for (channels, ms, bytes) in [(2, 5, 960), (6, 2, 1152), (8, 1, 768), (12, 1, 1152)] {
            assert_eq!(pcm_packet_ms(channels), Some(ms), "{channels}");
            assert_eq!(pcm_packet_bytes(channels), Some(bytes), "{channels}");
        }
        assert_eq!(pcm_packet_ms(4), None);
        assert_eq!(pcm_packet_bytes(0), None);
    }

    #[test]
    fn chunks_round_trip_and_cover_a_frame() {
        let header = ChunkHeader {
            first: true,
            last: false,
            frame_index: 0x1234,
        };
        assert_eq!(header.encode(), [1, 1, 0x34, 0x12]);
        assert_eq!(ChunkHeader::decode(&header.encode()), Some(header));
        let last = ChunkHeader {
            first: false,
            last: true,
            frame_index: u16::MAX,
        };
        assert_eq!(last.encode(), [1, 2, 0xFF, 0xFF]);
        assert_eq!(
            ChunkHeader::decode(&[2, 3, 0, 0, 9]),
            Some(ChunkHeader {
                first: true,
                last: true,
                frame_index: 0
            })
        );
        assert_eq!(ChunkHeader::decode(&[0, 1, 0, 0]), None);
        assert_eq!(ChunkHeader::decode(&[1, 1, 0]), None);
        assert_eq!(CHUNK_PAYLOAD_BYTES, 1028);
        for (len, chunks) in [(0, 0), (1, 1), (1024, 1), (1025, 2), (2560, 3), (4096, 4)] {
            assert_eq!(chunks_for(len), chunks, "{len}");
        }
    }

    #[test]
    fn layouts_parse_and_print() {
        assert_eq!(LAYOUT_714.to_attr(), "12,8,4,0,1,2,3,4,5,6,7,8,9,10,11");
        assert_eq!(LAYOUT_714_HQ.to_attr(), "12,12,0,0,1,2,3,4,5,6,7,8,9,10,11");
        for layout in [LAYOUT_714, LAYOUT_714_HQ] {
            assert_eq!(Layout::parse(&layout.to_attr()), Some(layout));
        }
        // The last two are Foundation Sunshine's 7.1.4 DESCRIBE lines, byte for byte.
        assert_eq!(
            layout_714_lines(),
            [
                "a=x-rl-audio.layout714:12,8,4,0,1,2,3,4,5,6,7,8,9,10,11\r\n",
                "a=x-rl-audio.layout714Hq:12,12,0,0,1,2,3,4,5,6,7,8,9,10,11\r\n",
                "a=fmtp:97 surround-params=12,8,4,0,1,2,3,4,5,6,7,8,9,10,11\r\n",
                "a=fmtp:97 surround-params=12,12,0,0,1,2,3,4,5,6,7,8,9,10,11\r\n",
            ]
        );
        for text in [
            "",
            "12,7,5",
            "12,7,5,0,1,10,11,2,3,4,5,6,7,8",
            "12,7,5,0,1,10,11,2,3,4,5,6,7,8,9,0",
            "12,7,6,0,1,10,11,2,3,4,5,6,7,8,9",
            "12,5,7,0,1,10,11,2,3,4,5,6,7,8,9",
            "12,7,5,0,1,10,12,2,3,4,5,6,7,8,9",
            "8,4,4,0,1,4,5,2,3,6,7,0,0,0,0",
            "12,7,5,0,1,10,11,2,3,4,5,6,7,8,+9",
            "12,7,5,0,1,10,11,2,3,4,5,6,7,8, 9",
            "12,7,5,0,1,10,11,2,3,4,5,6,7,8,",
            "12,7,5,0,1,10,11,2,3,4,5,6,7,8,256",
        ] {
            assert_eq!(Layout::parse(text), None, "{text:?}");
        }
    }
}
