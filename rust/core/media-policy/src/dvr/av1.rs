//! AV1 low-overhead OBU streams: OBU iteration, size-field normalization for MP4 samples
//! and `av1C`, and the sequence header fields `av1C` and the sample entry need. Parsers
//! stop or return `None` on malformed input.

use super::nal::Bits;

pub const OBU_SEQUENCE_HEADER: u8 = 1;
pub const OBU_TEMPORAL_DELIMITER: u8 = 2;
pub const OBU_TILE_LIST: u8 = 8;
pub const OBU_PADDING: u8 = 15;

/// One OBU: its one- or two-byte header and its payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Obu<'a> {
    pub kind: u8,
    pub header: &'a [u8],
    pub payload: &'a [u8],
}

impl Obu<'_> {
    /// Appends the OBU with `obu_has_size_field` set, as MP4 samples and `av1C` carry it.
    pub fn write_sized(&self, out: &mut Vec<u8>) {
        if let Some((&first, extension)) = self.header.split_first() {
            out.push(first | 0x02);
            out.extend_from_slice(extension);
            write_leb128(out, self.payload.len() as u64);
            out.extend_from_slice(self.payload);
        }
    }

    pub fn sized(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.header.len() + 8 + self.payload.len());
        self.write_sized(&mut out);
        out
    }
}

/// OBUs of a temporal unit; an OBU without a size field runs to the end of the data.
pub fn obus(data: &[u8]) -> Obus<'_> {
    Obus { data, pos: 0 }
}

pub struct Obus<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Iterator for Obus<'a> {
    type Item = Obu<'a>;

    fn next(&mut self) -> Option<Obu<'a>> {
        let rest = self.data.get(self.pos..)?;
        let first = *rest.first()?;
        if first & 0x80 != 0 {
            self.pos = self.data.len();
            return None;
        }
        let header_len = if first & 0x04 != 0 { 2 } else { 1 };
        let Some(header) = rest.get(..header_len) else {
            self.pos = self.data.len();
            return None;
        };
        let (payload_at, payload_len) = if first & 0x02 != 0 {
            let Some((size, used)) = leb128(&rest[header_len..]) else {
                self.pos = self.data.len();
                return None;
            };
            (
                header_len + used,
                usize::try_from(size).unwrap_or(usize::MAX),
            )
        } else {
            (header_len, rest.len() - header_len)
        };
        let Some(payload) = payload_at
            .checked_add(payload_len)
            .and_then(|end| rest.get(payload_at..end))
        else {
            self.pos = self.data.len();
            return None;
        };
        self.pos += payload_at + payload_len;
        Some(Obu {
            kind: (first >> 3) & 0x0F,
            header,
            payload,
        })
    }
}

/// A `leb128()` value and the bytes it used (at most eight).
pub fn leb128(data: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0u64;
    for (index, &byte) in data.iter().take(8).enumerate() {
        value |= u64::from(byte & 0x7F) << (7 * index);
        if byte & 0x80 == 0 {
            return Some((value, index + 1));
        }
    }
    None
}

pub fn write_leb128(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// The sequence header fields `av1C` and the visual sample entry need.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SequenceHeader {
    pub profile: u8,
    pub level_idx: u8,
    pub tier: bool,
    pub high_bitdepth: bool,
    pub twelve_bit: bool,
    pub monochrome: bool,
    pub subsampling_x: bool,
    pub subsampling_y: bool,
    pub chroma_sample_position: u8,
    pub width: u32,
    pub height: u32,
}

impl SequenceHeader {
    pub fn bit_depth(&self) -> u8 {
        match (self.high_bitdepth, self.twelve_bit) {
            (false, _) => 8,
            (true, false) => 10,
            (true, true) => 12,
        }
    }
}

/// Parses a sequence header OBU payload (AV1 spec 5.5).
pub fn sequence_header(payload: &[u8]) -> Option<SequenceHeader> {
    let mut bits = Bits::new(payload);
    let profile = bits.u8(3)?;
    if profile > 2 {
        return None;
    }
    bits.flag()?;
    let reduced = bits.flag()?;
    let mut tier = false;
    let level_idx = if reduced {
        bits.u8(5)?
    } else {
        let mut buffer_delay_bits = 0;
        let mut decoder_model = false;
        if bits.flag()? {
            bits.skip(64)?;
            if bits.flag()? {
                bits.uvlc()?;
            }
            decoder_model = bits.flag()?;
            if decoder_model {
                buffer_delay_bits = usize::from(bits.u8(5)?) + 1;
                bits.skip(32 + 5 + 5)?;
            }
        }
        let display_delay = bits.flag()?;
        let points = bits.u8(5)? + 1;
        let mut first_level = 0;
        for point in 0..points {
            bits.skip(12)?;
            let level = bits.u8(5)?;
            let point_tier = level > 7 && bits.flag()?;
            if point == 0 {
                first_level = level;
                tier = point_tier;
            }
            if decoder_model && bits.flag()? {
                bits.skip(2 * buffer_delay_bits + 1)?;
            }
            if display_delay && bits.flag()? {
                bits.skip(4)?;
            }
        }
        first_level
    };
    let width_bits = bits.u32(4)? + 1;
    let height_bits = bits.u32(4)? + 1;
    let width = bits.u32(width_bits)?.checked_add(1)?;
    let height = bits.u32(height_bits)?.checked_add(1)?;
    if !reduced && bits.flag()? {
        bits.skip(7)?;
    }
    bits.skip(3)?;
    if !reduced {
        bits.skip(4)?;
        let order_hint = bits.flag()?;
        if order_hint {
            bits.skip(2)?;
        }
        let force_screen_content = if bits.flag()? { 2 } else { bits.u8(1)? };
        // seq_choose_integer_mv, then seq_force_integer_mv unless it chose.
        if force_screen_content > 0 && !bits.flag()? {
            bits.skip(1)?;
        }
        if order_hint {
            bits.skip(3)?;
        }
    }
    bits.skip(3)?;
    let high_bitdepth = bits.flag()?;
    let twelve_bit = profile == 2 && high_bitdepth && bits.flag()?;
    let monochrome = profile != 1 && bits.flag()?;
    let (mut primaries, mut transfer, mut matrix) = (2, 2, 2);
    if bits.flag()? {
        primaries = bits.u8(8)?;
        transfer = bits.u8(8)?;
        matrix = bits.u8(8)?;
    }
    let (subsampling_x, subsampling_y, mut chroma_sample_position) = if monochrome {
        bits.flag()?;
        (true, true, 0)
    } else if primaries == 1 && transfer == 13 && matrix == 0 {
        (false, false, 0)
    } else {
        bits.flag()?;
        let (x, y) = match profile {
            0 => (true, true),
            1 => (false, false),
            _ if twelve_bit => {
                let x = bits.flag()?;
                (x, x && bits.flag()?)
            }
            _ => (true, false),
        };
        (x, y, 0)
    };
    if !monochrome && subsampling_x && subsampling_y {
        chroma_sample_position = bits.u8(2)?;
    }
    Some(SequenceHeader {
        profile,
        level_idx,
        tier,
        high_bitdepth,
        twelve_bit,
        monochrome,
        subsampling_x,
        subsampling_y,
        chroma_sample_position,
        width,
        height,
    })
}
