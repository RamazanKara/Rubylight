//! Minimal bounded SPS edit for H.264 long-term reference packet-loss recovery.
mod h264;
mod hevc;
mod rbsp;

pub use h264::FrameNumbers;
pub use hevc::{ORDER_WINDOW, POC_LSB_BITS, PictureOrder};
pub use rbsp::Unsupported;

use std::borrow::Cow;

/// The header rewrites AMF's H.264 and HEVC need before a client that
/// dropped frames until a long-term reference repair can decode it. AV1
/// names its references by slot with an explicit order hint: none.
pub enum RepairHeaders {
    H264(FrameNumbers),
    Hevc(PictureOrder),
}
impl RepairHeaders {
    pub fn for_codec(codec: u8) -> Option<Self> {
        match codec {
            0 => Some(Self::H264(FrameNumbers::default())),
            1 => Some(Self::Hevc(PictureOrder::default())),
            _ => None,
        }
    }
    /// `skipped` counts the frames since the IDR the client never decoded.
    pub fn rewrite(&mut self, bytes: &[u8], skipped: u64) -> Result<Option<Vec<u8>>, Unsupported> {
        match self {
            Self::H264(numbers) => numbers.rewrite(bytes, skipped),
            Self::Hevc(order) => order.rewrite(bytes),
        }
    }
}

/// The bytes a stream sends: the encoder's own, untouched, unless it keeps
/// [`RepairHeaders`] because it negotiated long-term reference repair.
pub fn repaired<'a>(
    headers: Option<&mut RepairHeaders>,
    bytes: &'a [u8],
    skipped: u64,
) -> Result<Cow<'a, [u8]>, Unsupported> {
    Ok(match headers {
        Some(headers) => headers
            .rewrite(bytes, skipped)?
            .map_or(Cow::Borrowed(bytes), Cow::Owned),
        None => Cow::Borrowed(bytes),
    })
}

struct Bits<'a> {
    bytes: &'a mut [u8],
    at: usize,
}
impl Bits<'_> {
    fn read(&mut self, count: usize) -> Option<u32> {
        if count > 32 || self.at.checked_add(count)? > self.bytes.len() * 8 {
            return None;
        }
        let mut value = 0;
        for _ in 0..count {
            value = (value << 1) | u32::from((self.bytes[self.at / 8] >> (7 - self.at % 8)) & 1);
            self.at += 1;
        }
        Some(value)
    }
    fn ue(&mut self) -> Option<u32> {
        let mut zeroes = 0;
        while self.read(1)? == 0 {
            zeroes += 1;
            if zeroes > 30 {
                return None;
            }
        }
        Some(((1u32 << zeroes) - 1) + self.read(zeroes)?)
    }
    fn se(&mut self) -> Option<i32> {
        let value = self.ue()?;
        Some(if value & 1 != 0 {
            (value.div_ceil(2)) as i32
        } else {
            -((value / 2) as i32)
        })
    }
}
fn allow_gaps(sps: &[u8]) -> Option<Vec<u8>> {
    if sps.len() > 4096 || sps.first()? & 31 != 7 {
        return None;
    }
    let mut rbsp = vec![];
    let mut zeros = 0;
    for &byte in &sps[1..] {
        if byte == 3 && zeros == 2 {
            zeros = 0;
            continue;
        }
        rbsp.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    let mut bits = Bits {
        bytes: &mut rbsp,
        at: 0,
    };
    let profile = bits.read(8)?;
    bits.read(16)?;
    bits.ue()?;
    if [
        100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135, 144,
    ]
    .contains(&profile)
    {
        let chroma = bits.ue()?;
        if chroma > 3 {
            return None;
        }
        if chroma == 3 {
            bits.read(1)?;
        }
        bits.ue()?;
        bits.ue()?;
        bits.read(1)?;
        if bits.read(1)? != 0 {
            for index in 0..if chroma == 3 { 12 } else { 8 } {
                if bits.read(1)? == 0 {
                    continue;
                }
                let (mut last, mut next) = (8i32, 8i32);
                for _ in 0..if index < 6 { 16 } else { 64 } {
                    if next != 0 {
                        next = (last + bits.se()?).rem_euclid(256);
                    }
                    if next != 0 {
                        last = next;
                    }
                }
            }
        }
    } else if ![66, 77, 88].contains(&profile) {
        return None;
    }
    bits.ue()?;
    match bits.ue()? {
        0 => {
            bits.ue()?;
        }
        1 => {
            bits.read(1)?;
            bits.se()?;
            bits.se()?;
            let count = bits.ue()?;
            if count > 256 {
                return None;
            }
            for _ in 0..count {
                bits.se()?;
            }
        }
        2 => {}
        _ => return None,
    }
    bits.ue()?;
    let gap = bits.at;
    bits.read(1)?;
    bits.bytes[gap / 8] |= 1 << (7 - gap % 8);
    let mut output = vec![sps[0]];
    let mut zeros = 0;
    for byte in rbsp {
        if zeros == 2 && byte <= 3 {
            output.push(3);
            zeros = 0;
        }
        output.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    Some(output)
}
/// Preserve every NAL except the SPS flag allowing missing frame numbers.
/// Hardware LTR references remain usable when a decoder inserts placeholders
/// for packets deliberately discarded during Moonlight's recovery window.
pub fn h264_reference_recovery(bytes: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let prefix = if bytes[at..].starts_with(&[0, 0, 0, 1]) {
            4
        } else if bytes[at..].starts_with(&[0, 0, 1]) {
            3
        } else {
            output.push(bytes[at]);
            at += 1;
            continue;
        };
        output.extend_from_slice(&bytes[at..at + prefix]);
        let start = at + prefix;
        let end = (start..bytes.len())
            .find(|&index| {
                bytes[index..].starts_with(&[0, 0, 1]) || bytes[index..].starts_with(&[0, 0, 0, 1])
            })
            .unwrap_or(bytes.len());
        if let Some(sps) = allow_gaps(&bytes[start..end]) {
            output.extend(sps);
        } else {
            output.extend_from_slice(&bytes[start..end]);
        }
        at = end;
    }
    output
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn truncated_and_unrelated_nals_are_unchanged_and_edits_are_idempotent() {
        for length in 0..256 {
            let bytes = vec![0u8; length];
            assert_eq!(h264_reference_recovery(&bytes), bytes);
        }
        let nal = [0, 0, 0, 1, 0x65, 0x88, 0x84, 0, 0, 3, 1];
        assert_eq!(h264_reference_recovery(&nal), nal);
        let sps = [0, 0, 0, 1, 0x67, 0x42, 0, 0x1f, 0xe5, 0x88, 0x68];
        let edited = h264_reference_recovery(&sps);
        assert_ne!(edited, sps);
        assert_eq!(h264_reference_recovery(&edited), edited);
        for end in 0..sps.len() {
            let bytes = &sps[..end];
            let edited = h264_reference_recovery(bytes);
            assert_eq!(h264_reference_recovery(&edited), edited);
        }
    }
    #[test]
    fn a_stream_without_repair_headers_sends_the_encoders_bytes() {
        let stream: &[u8] = include_bytes!("bitstream/fixtures/amf-hevc-ltr.bin");
        assert!(RepairHeaders::for_codec(2).is_none());
        for skipped in [0, 5] {
            let sent = repaired(None, stream, skipped).unwrap();
            assert!(matches!(sent, Cow::Borrowed(bytes) if std::ptr::eq(bytes, stream)));
        }
        let mut hevc = RepairHeaders::for_codec(1);
        assert!(matches!(
            repaired(hevc.as_mut(), stream, 0),
            Ok(Cow::Owned(_))
        ));
        let mut h264 = RepairHeaders::for_codec(0);
        let stream: &[u8] = include_bytes!("bitstream/fixtures/amf-h264-ltr.bin");
        let sent = repaired(h264.as_mut(), stream, 0).unwrap();
        assert!(matches!(sent, Cow::Borrowed(bytes) if std::ptr::eq(bytes, stream)));
    }
    /// Rewrites whole AMF streams frame by frame and prints the time each
    /// frame took, and what copying it alone takes. `BUTTERPOLLO_TEST_REWRITE`
    /// lists `<h264|hevc>,<in>,<out>` separated by `;`, each optionally with
    /// `,<from>,<skipped>`: H.264 frames from index `from` on as if the client
    /// skipped `skipped`. Run it in a release build.
    #[test]
    #[ignore = "reads and writes task-owned stream files"]
    fn rewrite_time_per_frame() {
        let specs = std::env::var("BUTTERPOLLO_TEST_REWRITE").expect("BUTTERPOLLO_TEST_REWRITE");
        for spec in specs.split(';') {
            let fields: Vec<&str> = spec.split(',').collect();
            let hevc = fields[0] == "hevc";
            let stream = std::fs::read(fields[1]).unwrap();
            let (from, skipped) = match fields.get(3..5) {
                Some([from, skipped]) => (from.parse().unwrap(), skipped.parse().unwrap()),
                _ => (usize::MAX, 0),
            };
            // A frame starts at the first unit after a slice that is not
            // followed by a later slice of the same picture.
            let mut cuts = vec![0];
            let mut after_slice = false;
            for (start, _) in rbsp::units(&stream) {
                let nal = &stream[start..];
                let (slice, first) = if hevc {
                    ((nal[0] >> 1) & 63 <= 21, nal[2] & 0x80 != 0)
                } else {
                    (matches!(nal[0] & 31, 1 | 5), nal[1] & 0x80 != 0)
                };
                if after_slice && (!slice || first) {
                    let mut cut = start - 3;
                    while cut > 0 && stream[cut - 1] == 0 {
                        cut -= 1;
                    }
                    cuts.push(cut);
                }
                after_slice = slice;
            }
            cuts.push(stream.len());
            let mut headers = RepairHeaders::for_codec(u8::from(hevc)).unwrap();
            let mut output = Vec::with_capacity(stream.len());
            let (mut times, mut copies) = (Vec::new(), Vec::new());
            for (index, cut) in cuts.windows(2).enumerate() {
                let frame = &stream[cut[0]..cut[1]];
                let skipped = if index >= from { skipped } else { 0 };
                let started = std::time::Instant::now();
                let copy = std::hint::black_box(frame.to_vec());
                copies.push(started.elapsed());
                drop(copy);
                let started = std::time::Instant::now();
                let sent = repaired(Some(&mut headers), frame, skipped)
                    .unwrap()
                    .into_owned();
                times.push(started.elapsed());
                output.extend_from_slice(&sent);
            }
            std::fs::write(fields[2], &output).unwrap();
            let percentile = |times: &mut Vec<std::time::Duration>, share: f64| {
                times.sort();
                times[((times.len() - 1) as f64 * share) as usize]
            };
            println!(
                "{}: {} frames, {} bytes: rewrite p50 {:?} p99 {:?} max {:?}; copy alone p50 {:?} p99 {:?}",
                fields[2],
                times.len(),
                stream.len(),
                percentile(&mut times, 0.5),
                percentile(&mut times, 0.99),
                percentile(&mut times, 1.0),
                percentile(&mut copies, 0.5),
                percentile(&mut copies, 0.99),
            );
        }
    }
}
