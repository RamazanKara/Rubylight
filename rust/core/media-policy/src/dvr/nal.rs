//! Annex-B byte streams for H.264 and HEVC: NAL unit iteration, emulation-prevention
//! removal, an Exp-Golomb bit reader (shared with the AV1 parser), and the SPS fields the
//! MP4 sample entries need: profile, level, chroma format, bit depth and the cropped
//! picture size. Every parser returns `None` on truncated or invalid input.

pub const H264_SEI: u8 = 6;
pub const H264_SPS: u8 = 7;
pub const H264_PPS: u8 = 8;
pub const H264_AUD: u8 = 9;
pub const H264_FILLER: u8 = 12;
pub const H264_SPS_EXT: u8 = 13;

pub const HEVC_VPS: u8 = 32;
pub const HEVC_SPS: u8 = 33;
pub const HEVC_PPS: u8 = 34;
pub const HEVC_AUD: u8 = 35;
pub const HEVC_FILLER: u8 = 38;
pub const HEVC_PREFIX_SEI: u8 = 39;

/// H.264 `nal_unit_type`.
pub fn h264_type(unit: &[u8]) -> Option<u8> {
    unit.first().map(|byte| byte & 0x1F)
}

/// HEVC `nal_unit_type`.
pub fn hevc_type(unit: &[u8]) -> Option<u8> {
    unit.first().map(|byte| (byte >> 1) & 0x3F)
}

/// NAL units of an Annex-B stream, without start codes or trailing zero bytes. Bytes
/// before the first start code are skipped.
pub fn units(data: &[u8]) -> Units<'_> {
    Units {
        data,
        next: start_code(data, 0),
    }
}

pub struct Units<'a> {
    data: &'a [u8],
    /// Where the next start code begins, with its length.
    next: Option<(usize, usize)>,
}

impl<'a> Iterator for Units<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        loop {
            let (at, length) = self.next?;
            let begin = at + length;
            self.next = start_code(self.data, begin);
            let end = self.next.map_or(self.data.len(), |(next, _)| next);
            let mut unit = &self.data[begin..end];
            while let [rest @ .., 0] = unit {
                unit = rest;
            }
            if !unit.is_empty() {
                return Some(unit);
            }
        }
    }
}

/// The next `00 00 01` at or after `from`: where it begins (a preceding zero byte makes
/// it a four-byte code) and its length.
fn start_code(data: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut at = from;
    while at + 2 < data.len() {
        let third = data[at + 2];
        if third > 1 {
            at += 3;
        } else if third == 1 && data[at] == 0 && data[at + 1] == 0 {
            return Some(if at > from && data[at - 1] == 0 {
                (at - 1, 4)
            } else {
                (at, 3)
            });
        } else {
            at += 1;
        }
    }
    None
}

/// A NAL unit's payload with the emulation-prevention `03` of each `00 00 03` removed.
pub fn rbsp(unit: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(unit.len());
    let mut zeros = 0;
    for &byte in unit {
        if zeros >= 2 && byte == 3 {
            zeros = 0;
            continue;
        }
        out.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// MSB-first bit reader over an RBSP or an AV1 OBU payload.
pub struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn bit(&mut self) -> Option<bool> {
        let byte = *self.data.get(self.pos / 8)?;
        let bit = (byte >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Some(bit == 1)
    }

    pub fn flag(&mut self) -> Option<bool> {
        self.bit()
    }

    /// `count` (at most 64) bits as an unsigned number.
    pub fn bits(&mut self, count: u32) -> Option<u64> {
        if count > 64 {
            return None;
        }
        let mut value = 0u64;
        for _ in 0..count {
            value = (value << 1) | u64::from(self.bit()?);
        }
        Some(value)
    }

    pub fn u8(&mut self, count: u32) -> Option<u8> {
        u8::try_from(self.bits(count.min(8))?).ok()
    }

    pub fn u32(&mut self, count: u32) -> Option<u32> {
        u32::try_from(self.bits(count.min(32))?).ok()
    }

    pub fn skip(&mut self, count: usize) -> Option<()> {
        let end = self.pos.checked_add(count)?;
        if end > self.data.len() * 8 {
            return None;
        }
        self.pos = end;
        Some(())
    }

    /// Unsigned Exp-Golomb `ue(v)`.
    pub fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while !self.bit()? {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        let rest = self.bits(zeros)?;
        u32::try_from((1u64 << zeros) - 1 + rest).ok()
    }

    /// Signed Exp-Golomb `se(v)`.
    pub fn se(&mut self) -> Option<i32> {
        let code = i64::from(self.ue()?);
        let value = if code % 2 == 1 {
            (code + 1) / 2
        } else {
            -(code / 2)
        };
        i32::try_from(value).ok()
    }

    /// AV1 `uvlc()`.
    pub fn uvlc(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while !self.bit()? {
            zeros += 1;
            if zeros >= 32 {
                return Some(u32::MAX);
            }
        }
        let rest = self.bits(zeros)?;
        u32::try_from((1u64 << zeros) - 1 + rest).ok()
    }
}

/// The H.264 SPS fields `avcC` and the visual sample entry need.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct H264Sps {
    pub profile_idc: u8,
    pub constraint_flags: u8,
    pub level_idc: u8,
    pub chroma_format_idc: u8,
    pub bit_depth_luma: u8,
    pub bit_depth_chroma: u8,
    /// Cropped picture size.
    pub width: u32,
    pub height: u32,
}

impl H264Sps {
    /// Profiles whose `avcC` carries the chroma format and bit depth extension.
    pub fn high_profile(&self) -> bool {
        matches!(self.profile_idc, 100 | 110 | 122 | 144)
    }
}

/// Parses an H.264 SPS NAL unit (with its one-byte header).
pub fn h264_sps(unit: &[u8]) -> Option<H264Sps> {
    if h264_type(unit)? != H264_SPS {
        return None;
    }
    let payload = rbsp(unit.get(1..)?);
    let mut bits = Bits::new(&payload);
    let profile_idc = bits.u8(8)?;
    let constraint_flags = bits.u8(8)?;
    let level_idc = bits.u8(8)?;
    bits.ue()?;
    let (mut chroma_format_idc, mut separate_planes) = (1, false);
    let (mut bit_depth_luma, mut bit_depth_chroma) = (8, 8);
    if matches!(
        profile_idc,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        chroma_format_idc = bits.ue()?;
        if chroma_format_idc > 3 {
            return None;
        }
        if chroma_format_idc == 3 {
            separate_planes = bits.flag()?;
        }
        bit_depth_luma = bits.ue()?.checked_add(8)?;
        bit_depth_chroma = bits.ue()?.checked_add(8)?;
        if bit_depth_luma > 14 || bit_depth_chroma > 14 {
            return None;
        }
        bits.flag()?;
        if bits.flag()? {
            let lists = if chroma_format_idc == 3 { 12 } else { 8 };
            for list in 0..lists {
                if bits.flag()? {
                    skip_scaling_list(&mut bits, if list < 6 { 16 } else { 64 })?;
                }
            }
        }
    }
    bits.ue()?;
    match bits.ue()? {
        0 => {
            bits.ue()?;
        }
        1 => {
            bits.flag()?;
            bits.se()?;
            bits.se()?;
            let cycle = bits.ue()?;
            if cycle > 255 {
                return None;
            }
            for _ in 0..cycle {
                bits.se()?;
            }
        }
        2 => {}
        _ => return None,
    }
    bits.ue()?;
    bits.flag()?;
    let width_mbs = u64::from(bits.ue()?) + 1;
    let height_map_units = u64::from(bits.ue()?) + 1;
    let frame_mbs_only = bits.flag()?;
    if !frame_mbs_only {
        bits.flag()?;
    }
    bits.flag()?;
    let field_factor = if frame_mbs_only { 1 } else { 2 };
    let (crop_x, crop_y) = match (separate_planes, chroma_format_idc) {
        (true, _) | (false, 0) => (1, field_factor),
        (false, 1) => (2, 2 * field_factor),
        (false, 2) => (2, field_factor),
        _ => (1, field_factor),
    };
    let mut crop = [0u64; 4];
    if bits.flag()? {
        for value in &mut crop {
            *value = u64::from(bits.ue()?);
        }
    }
    let width = (width_mbs * 16).checked_sub(crop_x * (crop[0] + crop[1]))?;
    let height =
        (field_factor * height_map_units * 16).checked_sub(crop_y * (crop[2] + crop[3]))?;
    Some(H264Sps {
        profile_idc,
        constraint_flags,
        level_idc,
        chroma_format_idc: u8::try_from(chroma_format_idc).ok()?,
        bit_depth_luma: u8::try_from(bit_depth_luma).ok()?,
        bit_depth_chroma: u8::try_from(bit_depth_chroma).ok()?,
        width: u32::try_from(width).ok().filter(|&w| w > 0)?,
        height: u32::try_from(height).ok().filter(|&h| h > 0)?,
    })
}

fn skip_scaling_list(bits: &mut Bits<'_>, size: usize) -> Option<()> {
    let (mut last, mut next) = (8i64, 8i64);
    for _ in 0..size {
        if next != 0 {
            next = (last + i64::from(bits.se()?) + 256).rem_euclid(256);
        }
        if next != 0 {
            last = next;
        }
    }
    Some(())
}

/// The HEVC SPS fields `hvcC` and the visual sample entry need.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HevcSps {
    pub profile_space: u8,
    pub tier: bool,
    pub profile_idc: u8,
    pub compatibility: u32,
    /// The 48 general constraint indicator bits.
    pub constraints: u64,
    pub level_idc: u8,
    pub max_sub_layers: u8,
    pub temporal_id_nesting: bool,
    pub chroma_format_idc: u8,
    pub bit_depth_luma: u8,
    pub bit_depth_chroma: u8,
    /// Picture size inside the conformance window.
    pub width: u32,
    pub height: u32,
}

/// Parses an HEVC SPS NAL unit (with its two-byte header).
pub fn hevc_sps(unit: &[u8]) -> Option<HevcSps> {
    if hevc_type(unit)? != HEVC_SPS {
        return None;
    }
    let payload = rbsp(unit.get(2..)?);
    let mut bits = Bits::new(&payload);
    bits.skip(4)?;
    let max_sub_layers_minus1 = bits.u8(3)?;
    let temporal_id_nesting = bits.flag()?;
    let profile_space = bits.u8(2)?;
    let tier = bits.flag()?;
    let profile_idc = bits.u8(5)?;
    let compatibility = bits.u32(32)?;
    let constraints = bits.bits(48)?;
    let level_idc = bits.u8(8)?;
    let sub_layers = usize::from(max_sub_layers_minus1);
    let mut present = [(false, false); 8];
    for flags in present.iter_mut().take(sub_layers) {
        *flags = (bits.flag()?, bits.flag()?);
    }
    if sub_layers > 0 {
        bits.skip(2 * (8 - sub_layers))?;
    }
    for &(profile, level) in present.iter().take(sub_layers) {
        if profile {
            bits.skip(88)?;
        }
        if level {
            bits.skip(8)?;
        }
    }
    bits.ue()?;
    let chroma_format_idc = bits.ue()?;
    if chroma_format_idc > 3 {
        return None;
    }
    let separate_planes = chroma_format_idc == 3 && bits.flag()?;
    let width = u64::from(bits.ue()?);
    let height = u64::from(bits.ue()?);
    let (sub_width, sub_height) = match (separate_planes, chroma_format_idc) {
        (false, 1) => (2, 2),
        (false, 2) => (2, 1),
        _ => (1, 1),
    };
    let mut window = [0u64; 4];
    if bits.flag()? {
        for value in &mut window {
            *value = u64::from(bits.ue()?);
        }
    }
    let bit_depth_luma = bits.ue()?.checked_add(8)?;
    let bit_depth_chroma = bits.ue()?.checked_add(8)?;
    if bit_depth_luma > 16 || bit_depth_chroma > 16 {
        return None;
    }
    let width = width.checked_sub(sub_width * (window[0] + window[1]))?;
    let height = height.checked_sub(sub_height * (window[2] + window[3]))?;
    Some(HevcSps {
        profile_space,
        tier,
        profile_idc,
        compatibility,
        constraints,
        level_idc,
        max_sub_layers: max_sub_layers_minus1 + 1,
        temporal_id_nesting,
        chroma_format_idc: u8::try_from(chroma_format_idc).ok()?,
        bit_depth_luma: u8::try_from(bit_depth_luma).ok()?,
        bit_depth_chroma: u8::try_from(bit_depth_chroma).ok()?,
        width: u32::try_from(width).ok().filter(|&w| w > 0)?,
        height: u32::try_from(height).ok().filter(|&h| h > 0)?,
    })
}
