//! Annex B units, emulation prevention and the bit fields of H.264 and HEVC headers.
use std::fmt;

/// Syntax a header rewrite does not handle, or a header cut short.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unsupported(pub &'static str);
impl fmt::Display for Unsupported {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}
impl std::error::Error for Unsupported {}
pub type Result<T> = std::result::Result<T, Unsupported>;
pub const TRUNCATED: Unsupported = Unsupported("header cut short");
/// A NAL unit's new bytes, and the index in the old unit from which its
/// bytes follow them unchanged.
pub type Replacement = (Vec<u8>, usize);

pub struct Reader<'a> {
    bytes: &'a [u8],
    pub at: usize,
}
impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }
    pub fn bits(&mut self, count: u32) -> Result<u32> {
        debug_assert!(count <= 32);
        let mut value = 0u32;
        for _ in 0..count {
            let byte = *self.bytes.get(self.at / 8).ok_or(TRUNCATED)?;
            value = (value << 1) | u32::from((byte >> (7 - self.at % 8)) & 1);
            self.at += 1;
        }
        Ok(value)
    }
    pub fn flag(&mut self) -> Result<bool> {
        Ok(self.bits(1)? == 1)
    }
    pub fn ue(&mut self) -> Result<u32> {
        let mut zeros = 0;
        while !self.flag()? {
            zeros += 1;
            if zeros > 31 {
                return Err(Unsupported("Exp-Golomb code over 32 bits"));
            }
        }
        Ok(((1u64 << zeros) - 1 + u64::from(self.bits(zeros)?)) as u32)
    }
    /// An unsigned code no larger than `limit`.
    pub fn ue_max(&mut self, limit: u32, what: &'static str) -> Result<u32> {
        let value = self.ue()?;
        if value > limit {
            return Err(Unsupported(what));
        }
        Ok(value)
    }
    pub fn se(&mut self) -> Result<i32> {
        let value = i64::from(self.ue()?);
        Ok(if value & 1 == 1 {
            (value + 1) / 2
        } else {
            -(value / 2)
        } as i32)
    }
    pub fn skip(&mut self, count: usize) -> Result<()> {
        self.at = self
            .at
            .checked_add(count)
            .filter(|&at| at <= self.bytes.len() * 8)
            .ok_or(TRUNCATED)?;
        Ok(())
    }
}

#[derive(Default)]
pub struct Writer {
    bytes: Vec<u8>,
    at: usize,
}
impl Writer {
    pub fn bits(&mut self, count: u32, value: u32) {
        for index in (0..count).rev() {
            if self.at.is_multiple_of(8) {
                self.bytes.push(0);
            }
            if (value >> index) & 1 == 1 {
                *self.bytes.last_mut().unwrap() |= 0x80 >> (self.at % 8);
            }
            self.at += 1;
        }
    }
    pub fn flag(&mut self, on: bool) {
        self.bits(1, u32::from(on));
    }
    pub fn ue(&mut self, value: u32) {
        let code = u64::from(value) + 1;
        let length = 64 - code.leading_zeros();
        self.bits(length - 1, 0);
        if length > 32 {
            self.bits(1, 1);
        }
        self.bits(length.min(32), code as u32);
    }
    /// Bits `start..end` of `from`, unchanged.
    pub fn copy(&mut self, from: &[u8], start: usize, end: usize) {
        for at in start..end {
            self.flag((from[at / 8] >> (7 - at % 8)) & 1 == 1);
        }
    }
    /// The bytes after a one bit and zeros to the byte boundary: RBSP
    /// trailing bits, or a slice header's byte alignment.
    pub fn finish(mut self) -> Vec<u8> {
        self.flag(true);
        while !self.at.is_multiple_of(8) {
            self.flag(false);
        }
        self.bytes
    }
}

/// The NAL units of an Annex B buffer, each without its start code. Zero
/// bytes before a start code belong to it, as a NAL unit never ends in zero.
pub fn units(bytes: &[u8]) -> Vec<(usize, usize)> {
    let mut starts = Vec::new();
    let mut index = 0;
    while index + 2 < bytes.len() {
        // No start code begins at a byte that is not zero: skip eight
        // bytes at a time while none is.
        if let Some(word) = bytes.get(index..index + 8) {
            let word = u64::from_le_bytes(word.try_into().unwrap());
            if word.wrapping_sub(0x0101_0101_0101_0101) & !word & 0x8080_8080_8080_8080 == 0 {
                index += 8;
                continue;
            }
        }
        if bytes[index + 2] > 1 {
            index += 3;
        } else if bytes[index + 2] == 1 && bytes[index] == 0 && bytes[index + 1] == 0 {
            starts.push(index + 3);
            index += 3;
        } else {
            index += 1;
        }
    }
    starts
        .iter()
        .enumerate()
        .map(|(position, &start)| {
            let mut end = starts
                .get(position + 1)
                .map_or(bytes.len(), |&next| next - 3);
            while end > start && bytes[end - 1] == 0 {
                end -= 1;
            }
            (start, end)
        })
        .collect()
}

/// Up to `limit` RBSP bytes of a NAL unit's payload, each with the index of
/// the payload byte it came from.
pub fn unescape(payload: &[u8], limit: usize) -> (Vec<u8>, Vec<usize>) {
    let capacity = limit.min(payload.len());
    let (mut rbsp, mut from) = (Vec::with_capacity(capacity), Vec::with_capacity(capacity));
    let mut zeros = 0;
    for (index, &byte) in payload.iter().enumerate() {
        if rbsp.len() >= limit {
            break;
        }
        if zeros >= 2 && byte == 3 {
            zeros = 0;
            continue;
        }
        rbsp.push(byte);
        from.push(index);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    (rbsp, from)
}

pub fn escape(rbsp: &[u8], output: &mut Vec<u8>) {
    let mut zeros = 0;
    for &byte in rbsp {
        if zeros >= 2 && byte <= 3 {
            output.push(3);
            zeros = 0;
        }
        output.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
}

/// A NAL unit whose first `header` RBSP bytes become `rewritten`. The rest
/// stays as it was, emulation prevention included: the last byte of both
/// headers is non-zero (it holds a one bit), so no escape spans the seam.
pub fn splice(
    nal: &[u8],
    prefix: usize,
    rewritten: &[u8],
    from: &[usize],
    header: usize,
) -> Replacement {
    debug_assert!(header > 0 && rewritten.last().is_some_and(|&byte| byte != 0));
    let mut head = Vec::with_capacity(prefix + rewritten.len() + rewritten.len() / 2);
    head.extend_from_slice(&nal[..prefix]);
    escape(rewritten, &mut head);
    (head, prefix + from[header - 1] + 1)
}

/// `bytes` with the NAL units at `edits` replaced, copied once.
pub fn assemble(bytes: &[u8], edits: Vec<((usize, usize), Replacement)>) -> Vec<u8> {
    let extra: usize = edits.iter().map(|(_, (head, _))| head.len()).sum();
    let mut output = Vec::with_capacity(bytes.len() + extra);
    let mut cursor = 0;
    for ((start, end), (head, rest)) in edits {
        output.extend_from_slice(&bytes[cursor..start]);
        output.extend_from_slice(&head);
        output.extend_from_slice(&bytes[start + rest..end]);
        cursor = end;
    }
    output.extend_from_slice(&bytes[cursor..]);
    output
}

pub fn ceil_log2(value: u32) -> u32 {
    if value <= 1 {
        0
    } else {
        32 - (value - 1).leading_zeros()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codes_round_trip_through_the_writer_and_reader() {
        let mut writer = Writer::default();
        let values = [0, 1, 2, 3, 7, 8, 255, 65_535, u32::MAX - 1];
        for &value in &values {
            writer.ue(value);
            writer.bits(5, value & 31);
        }
        let bytes = writer.finish();
        let mut reader = Reader::new(&bytes);
        for &value in &values {
            assert_eq!(reader.ue().unwrap(), value);
            assert_eq!(reader.bits(5).unwrap(), value & 31);
        }
        assert!(reader.flag().unwrap());
        assert_eq!(reader.at.div_ceil(8), bytes.len());
        for (code, signed) in [(0, 0), (1, 1), (2, -1), (3, 2), (4, -2)] {
            let mut writer = Writer::default();
            writer.ue(code);
            assert_eq!(Reader::new(&writer.finish()).se().unwrap(), signed);
        }
    }
    #[test]
    fn escaping_round_trips_and_units_keep_their_zero_bytes_apart() {
        let rbsp = [0, 0, 0, 0, 1, 0, 0, 2, 0, 0, 3, 7, 0, 0];
        let mut escaped = Vec::new();
        escape(&rbsp, &mut escaped);
        assert_eq!(
            escaped,
            [0, 0, 3, 0, 0, 3, 1, 0, 0, 3, 2, 0, 0, 3, 3, 7, 0, 0]
        );
        assert_eq!(unescape(&escaped, usize::MAX).0, rbsp);
        let (prefix, from) = unescape(&escaped, 5);
        assert_eq!(prefix, [0, 0, 0, 0, 1]);
        assert_eq!(from, [0, 1, 3, 4, 6]);
        let stream = [
            0, 0, 0, 1, 0x40, 1, 0, 0, 1, 0x42, 1, 9, 0, 0, 0, 0, 1, 0x26,
        ];
        assert_eq!(units(&stream), [(4, 6), (9, 12), (17, 18)]);
        // Long runs without a zero, and start codes at every alignment of
        // the eight-byte skip.
        for at in 0..24 {
            let mut stream = vec![7u8; 40];
            stream[at..at + 3].copy_from_slice(&[0, 0, 1]);
            assert_eq!(units(&stream), [(at + 3, 40)], "start code at {at}");
        }
        assert!(units(&[0, 0]).is_empty());
        assert_eq!(ceil_log2(1), 0);
        assert_eq!(ceil_log2(2), 1);
        assert_eq!(ceil_log2(920), 10);
    }
}
