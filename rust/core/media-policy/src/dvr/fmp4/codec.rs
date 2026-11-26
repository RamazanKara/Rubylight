//! Codec sample entries for the fragmented MP4 muxer: `avc1`+`avcC`, `hvc1`+`hvcC` and
//! `av01`+`av1C` from the keyframe's parameter sets, the `colr` (nclx), `mdcv` and `clli`
//! colour boxes, and `Opus`+`dOps` with the channel mapping permuted from WAVE order to
//! the Vorbis order Opus mapping family 1 uses.

use super::boxes::Bmff;
use crate::dvr::nal::{H264Sps, HevcSps};
use crate::dvr::{HdrInfo, OpusConfig, av1::SequenceHeader};

/// The decoder configuration a visual sample entry wraps.
pub enum Config<'a> {
    Avc {
        sps: &'a [Vec<u8>],
        pps: &'a [Vec<u8>],
        parsed: H264Sps,
    },
    Hevc {
        vps: &'a [Vec<u8>],
        sps: &'a [Vec<u8>],
        pps: &'a [Vec<u8>],
        parsed: HevcSps,
    },
    Av1 {
        sequence_header: &'a [u8],
        parsed: SequenceHeader,
    },
}

/// A complete visual sample entry box.
pub fn visual_entry(
    config: &Config<'_>,
    width: u32,
    height: u32,
    hdr: Option<&HdrInfo>,
) -> Vec<u8> {
    let mut b = Bmff::new();
    b.begin(match config {
        Config::Avc { .. } => b"avc1",
        Config::Hevc { .. } => b"hvc1",
        Config::Av1 { .. } => b"av01",
    });
    b.zeros(6);
    b.u16(1);
    b.zeros(16);
    b.u16(u16::try_from(width).unwrap_or(u16::MAX));
    b.u16(u16::try_from(height).unwrap_or(u16::MAX));
    b.u32(0x0048_0000);
    b.u32(0x0048_0000);
    b.u32(0);
    b.u16(1);
    b.zeros(32);
    b.u16(0x0018);
    b.u16(0xFFFF);
    match config {
        Config::Avc { sps, pps, parsed } => avcc(&mut b, sps, pps, parsed),
        Config::Hevc {
            vps,
            sps,
            pps,
            parsed,
        } => hvcc(&mut b, [*vps, *sps, *pps], parsed),
        Config::Av1 {
            sequence_header,
            parsed,
        } => av1c(&mut b, sequence_header, parsed),
    }
    colour(&mut b, hdr);
    b.end();
    b.buf
}

fn avcc(b: &mut Bmff, sps: &[Vec<u8>], pps: &[Vec<u8>], parsed: &H264Sps) {
    b.begin(b"avcC");
    b.u8(1);
    b.u8(parsed.profile_idc);
    b.u8(parsed.constraint_flags);
    b.u8(parsed.level_idc);
    b.u8(0xFC | 3);
    b.u8(0xE0 | sps.len().min(31) as u8);
    for unit in sps.iter().take(31) {
        nal_entry(b, unit);
    }
    b.u8(pps.len().min(255) as u8);
    for unit in pps.iter().take(255) {
        nal_entry(b, unit);
    }
    if parsed.high_profile() {
        b.u8(0xFC | (parsed.chroma_format_idc & 3));
        b.u8(0xF8 | (parsed.bit_depth_luma.saturating_sub(8) & 7));
        b.u8(0xF8 | (parsed.bit_depth_chroma.saturating_sub(8) & 7));
        b.u8(0);
    }
    b.end();
}

fn hvcc(b: &mut Bmff, arrays: [&[Vec<u8>]; 3], parsed: &HevcSps) {
    b.begin(b"hvcC");
    b.u8(1);
    b.u8((parsed.profile_space << 6) | (u8::from(parsed.tier) << 5) | (parsed.profile_idc & 0x1F));
    b.u32(parsed.compatibility);
    b.bytes(&parsed.constraints.to_be_bytes()[2..]);
    b.u8(parsed.level_idc);
    b.u16(0xF000);
    b.u8(0xFC);
    b.u8(0xFC | (parsed.chroma_format_idc & 3));
    b.u8(0xF8 | (parsed.bit_depth_luma.saturating_sub(8) & 7));
    b.u8(0xF8 | (parsed.bit_depth_chroma.saturating_sub(8) & 7));
    b.u16(0);
    b.u8(((parsed.max_sub_layers & 7) << 3) | (u8::from(parsed.temporal_id_nesting) << 2) | 3);
    b.u8(3);
    for (kind, units) in [32u8, 33, 34].into_iter().zip(arrays) {
        // array_completeness: hvc1 keeps every parameter set out of band.
        b.u8(0x80 | kind);
        b.u16(units.len().min(0xFFFF) as u16);
        for unit in units.iter().take(0xFFFF) {
            nal_entry(b, unit);
        }
    }
    b.end();
}

fn nal_entry(b: &mut Bmff, unit: &[u8]) {
    b.u16(unit.len().min(0xFFFF) as u16);
    b.bytes(&unit[..unit.len().min(0xFFFF)]);
}

fn av1c(b: &mut Bmff, sequence_header: &[u8], parsed: &SequenceHeader) {
    b.begin(b"av1C");
    b.u8(0x81);
    b.u8((parsed.profile << 5) | (parsed.level_idx & 0x1F));
    b.u8((u8::from(parsed.tier) << 7)
        | (u8::from(parsed.high_bitdepth) << 6)
        | (u8::from(parsed.twelve_bit) << 5)
        | (u8::from(parsed.monochrome) << 4)
        | (u8::from(parsed.subsampling_x) << 3)
        | (u8::from(parsed.subsampling_y) << 2)
        | (parsed.chroma_sample_position & 3));
    b.u8(0);
    b.bytes(sequence_header);
    b.end();
}

/// `colr` nclx (BT.709 for SDR; BT.2020 + PQ for HDR), then `mdcv` and `clli` when the
/// HDR stream's metadata is known.
fn colour(b: &mut Bmff, hdr: Option<&HdrInfo>) {
    let (primaries, transfer, matrix) = if hdr.is_some() { (9, 16, 9) } else { (1, 1, 1) };
    b.begin(b"colr");
    b.bytes(b"nclx");
    b.u16(primaries);
    b.u16(transfer);
    b.u16(matrix);
    b.u8(0);
    b.end();
    let Some(metadata) = hdr.and_then(|hdr| hdr.metadata) else {
        return;
    };
    b.begin(b"mdcv");
    // ST 2086 order: green, blue, red.
    for index in [1, 2, 0] {
        b.u16(metadata.primaries[index][0]);
        b.u16(metadata.primaries[index][1]);
    }
    b.u16(metadata.white[0]);
    b.u16(metadata.white[1]);
    b.u32(u32::from(metadata.maximum_nits) * 10_000);
    b.u32(u32::from(metadata.minimum));
    b.end();
    b.begin(b"clli");
    b.u16(metadata.max_cll);
    b.u16(metadata.max_fall);
    b.end();
}

/// WAVE input channel index of each Vorbis-order output channel, by channel count.
fn vorbis_order(channels: u8) -> Option<&'static [usize]> {
    let order: &'static [usize] = match channels {
        1 => &[0],
        2 => &[0, 1],
        3 => &[0, 2, 1],
        4 => &[0, 1, 2, 3],
        5 => &[0, 2, 1, 3, 4],
        6 => &[0, 2, 1, 4, 5, 3],
        7 => &[0, 2, 1, 5, 6, 4, 3],
        8 => &[0, 2, 1, 6, 7, 4, 5, 3],
        _ => return None,
    };
    Some(order)
}

/// The `dOps` fields after the version: (family, channel mapping table).
pub fn opus_mapping(config: &OpusConfig) -> (u8, Vec<u8>) {
    let identity = config.mapping.iter().copied().eq(0..config.channels);
    if config.channels <= 2
        && config.streams == 1
        && config.coupled == config.channels.saturating_sub(1)
        && identity
    {
        return (0, Vec::new());
    }
    match vorbis_order(config.channels) {
        Some(order) => (
            1,
            order
                .iter()
                .map(|&wave| config.mapping.get(wave).copied().unwrap_or(255))
                .collect(),
        ),
        None => {
            let mut mapping = config.mapping.clone();
            mapping.resize(usize::from(config.channels), 255);
            (255, mapping)
        }
    }
}

/// A complete `Opus` sample entry with its `dOps` (pre-skip 0: clips start mid-stream).
pub fn opus_entry(config: &OpusConfig) -> Vec<u8> {
    let mut b = Bmff::new();
    b.begin(b"Opus");
    b.zeros(6);
    b.u16(1);
    b.zeros(8);
    b.u16(u16::from(config.channels));
    b.u16(16);
    b.u16(0);
    b.u16(0);
    b.u32(48_000 << 16);
    b.begin(b"dOps");
    b.u8(0);
    b.u8(config.channels);
    b.u16(0);
    b.u32(48_000);
    b.u16(0);
    let (family, mapping) = opus_mapping(config);
    b.u8(family);
    if family != 0 {
        b.u8(config.streams);
        b.u8(config.coupled);
        b.bytes(&mapping);
    }
    b.end();
    b.end();
    b.buf
}
