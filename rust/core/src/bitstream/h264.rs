//! Renumbers H.264 frame_num past the frames a client never decoded.
//!
//! A Moonlight client decodes nothing from its first lost frame until the
//! frame that repairs it from a long-term reference: it drops them. The
//! repair's frame_num then jumps, and FFmpeg's d3d11va and dxva2 decoding,
//! which fill such a gap with copies of the last frame, showed the long-term
//! reference itself instead of every picture from the repair on
//! (rust/PERFORMANCE.md, October 28). The host knows how many frames the
//! client skipped since the last IDR, so it writes each later frame_num as if
//! they had never existed. References name long-term pictures by index and
//! short-term ones by difference, so the pictures stay as they were.
use super::rbsp::{Reader, Replacement, Result, Unsupported, assemble, escape, unescape, units};

/// RBSP bytes unescaped before looking further: frame_num comes within the
/// first few.
const PREFIX: usize = 64;

#[derive(Default)]
pub struct FrameNumbers {
    /// log2_max_frame_num and separate_colour_plane_flag by SPS id.
    sps: Vec<Option<(u32, bool)>>,
    pps: Vec<Option<usize>>,
}

impl FrameNumbers {
    /// The buffer with frame_num lowered by `skipped`, the frames since the
    /// last IDR that the client's decoder never saw; `None` when nothing in
    /// it changes. An error leaves the stream unrenumberable.
    pub fn rewrite(&mut self, bytes: &[u8], skipped: u64) -> Result<Option<Vec<u8>>> {
        let mut edits = Vec::new();
        for (start, end) in units(bytes) {
            if let Some(nal) = self.unit(&bytes[start..end], skipped)? {
                edits.push(((start, end), nal));
            }
        }
        Ok((!edits.is_empty()).then(|| assemble(bytes, edits)))
    }

    fn unit(&mut self, nal: &[u8], skipped: u64) -> Result<Option<Replacement>> {
        if nal.len() < 2 {
            return Ok(None);
        }
        match nal[0] & 31 {
            7 => {
                let (rbsp, _) = unescape(&nal[1..], usize::MAX);
                let (id, sps) = sequence_parameter_set(&rbsp)?;
                self.sps.resize(self.sps.len().max(id + 1), None);
                self.sps[id] = Some(sps);
                Ok(None)
            }
            8 => {
                let (rbsp, _) = unescape(&nal[1..], PREFIX);
                let mut reader = Reader::new(&rbsp);
                let id = reader.ue_max(255, "PPS id")? as usize;
                let sps = reader.ue_max(31, "SPS id")? as usize;
                self.pps.resize(self.pps.len().max(id + 1), None);
                self.pps[id] = Some(sps);
                Ok(None)
            }
            kind @ (1 | 5) => {
                if nal[0] & 0x60 == 0 {
                    return Err(Unsupported("non-reference H.264 frame"));
                }
                if kind == 5 {
                    return Ok(None);
                }
                self.slice(nal, skipped)
            }
            2..=4 | 14 | 20 => Err(Unsupported("H.264 data partitions or layers")),
            _ => Ok(None),
        }
    }

    fn slice(&self, nal: &[u8], skipped: u64) -> Result<Option<Replacement>> {
        let (mut rbsp, mut from) = unescape(&nal[1..], PREFIX);
        let mut reader = Reader::new(&rbsp);
        reader.ue()?;
        reader.ue()?;
        let pps = reader.ue_max(255, "PPS id")? as usize;
        let &(bits, separate_colour_plane) = self
            .pps
            .get(pps)
            .copied()
            .flatten()
            .and_then(|sps| self.sps.get(sps))
            .and_then(Option::as_ref)
            .ok_or(Unsupported("slice before its parameter sets"))?;
        let period = 1u64 << bits;
        if skipped.is_multiple_of(period) {
            return Ok(None);
        }
        if separate_colour_plane {
            reader.skip(2)?;
        }
        let at = reader.at;
        let number = u64::from(reader.bits(bits)?);
        let renumbered = (number + period - skipped % period) % period;
        let original = rbsp.clone();
        for bit in 0..bits as usize {
            let (byte, mask) = ((at + bit) / 8, 0x80u8 >> ((at + bit) % 8));
            if (renumbered >> (bits as usize - 1 - bit)) & 1 == 1 {
                rbsp[byte] |= mask;
            } else {
                rbsp[byte] &= !mask;
            }
        }
        // Re-escape up to a byte that is non-zero before and after the edit,
        // so no escape in the bytes kept spans the seam.
        let seam = |rbsp: &[u8], original: &[u8]| {
            ((at + bits as usize).div_ceil(8)..=rbsp.len())
                .find(|&end| end > 0 && rbsp[end - 1] != 0 && original[end - 1] != 0)
        };
        let end = match seam(&rbsp, &original) {
            Some(end) => end,
            None => {
                let (full, map) = unescape(&nal[1..], usize::MAX);
                let mut edited = full.clone();
                edited[..rbsp.len()].copy_from_slice(&rbsp);
                let end = seam(&edited, &full).ok_or(Unsupported("H.264 slice of zero bytes"))?;
                (rbsp, from) = (edited, map);
                end
            }
        };
        let mut head = Vec::with_capacity(end + end / 2 + 1);
        head.push(nal[0]);
        escape(&rbsp[..end], &mut head);
        Ok(Some((head, 1 + from[end - 1] + 1)))
    }
}

/// The SPS id, log2_max_frame_num and separate_colour_plane_flag.
fn sequence_parameter_set(rbsp: &[u8]) -> Result<(usize, (u32, bool))> {
    let mut reader = Reader::new(rbsp);
    let profile = reader.bits(8)?;
    reader.skip(16)?;
    let id = reader.ue_max(31, "SPS id")? as usize;
    let mut separate_colour_plane = false;
    if [100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135, 144].contains(&profile) {
        let chroma = reader.ue_max(3, "chroma format")?;
        if chroma == 3 {
            separate_colour_plane = reader.flag()?;
        }
        reader.ue()?;
        reader.ue()?;
        reader.skip(1)?;
        if reader.flag()? {
            for index in 0..if chroma == 3 { 12 } else { 8 } {
                if !reader.flag()? {
                    continue;
                }
                let (mut last, mut next) = (8i32, 8i32);
                for _ in 0..if index < 6 { 16 } else { 64 } {
                    if next != 0 {
                        next = (last + reader.se()?).rem_euclid(256);
                    }
                    if next != 0 {
                        last = next;
                    }
                }
            }
        }
    }
    let bits = reader.ue_max(12, "frame_num length")? + 4;
    match reader.ue()? {
        // POC type 0 counts its own low bits, which a gap would also break.
        0 => return Err(Unsupported("H.264 POC type 0")),
        1 => {
            reader.skip(1)?;
            reader.se()?;
            reader.se()?;
            for _ in 0..reader.ue_max(255, "POC cycle")? {
                reader.se()?;
            }
        }
        2 => {}
        _ => return Err(Unsupported("H.264 POC type")),
    }
    reader.ue()?;
    reader.skip(1)?;
    reader.ue()?;
    reader.ue()?;
    if !reader.flag()? {
        return Err(Unsupported("H.264 field coding"));
    }
    Ok((id, (bits, separate_colour_plane)))
}

#[cfg(test)]
mod tests {
    use super::super::rbsp::Writer;
    use super::*;

    /// AMF's H.264 from the RX 7900 XT gap 1 run: one slice a frame for
    /// frames 0 to 249, each cut after its first bytes.
    const STREAM: &[u8] = include_bytes!("fixtures/amf-h264-ltr.bin");

    /// Each slice's NAL type and frame_num, with AMF's four bits.
    fn frame_numbers(stream: &[u8]) -> Vec<(u8, u32)> {
        units(stream)
            .into_iter()
            .map(|(start, end)| &stream[start..end])
            .filter(|nal| matches!(nal[0] & 31, 1 | 5))
            .map(|nal| {
                let (rbsp, _) = unescape(&nal[1..], usize::MAX);
                let mut reader = Reader::new(&rbsp);
                for _ in 0..3 {
                    reader.ue().unwrap();
                }
                (nal[0] & 31, reader.bits(4).unwrap())
            })
            .collect()
    }

    #[test]
    fn renumbering_lowers_frame_num_by_the_skipped_frames_and_nothing_else() {
        let original = frame_numbers(STREAM);
        assert_eq!(original.len(), 250);
        assert!(original.iter().enumerate().all(|(index, &(_, number))| number == index as u32 % 16));
        // Nothing skipped, or whole periods: AMF's stream as it was.
        for skipped in [0, 16, 32] {
            assert_eq!(FrameNumbers::default().rewrite(STREAM, skipped), Ok(None));
        }
        for skipped in [1, 5, 15, 130] {
            let rewritten = FrameNumbers::default().rewrite(STREAM, skipped).unwrap().unwrap();
            let numbers = frame_numbers(&rewritten);
            for (index, (&(kind, before), &(_, after))) in original.iter().zip(&numbers).enumerate() {
                let expected = if kind == 5 { before } else { (before + 16 - skipped as u32 % 16) % 16 };
                assert_eq!(after, expected, "frame {index} skipped {skipped}");
            }
            // Only frame_num moved: the rest of every unit comes back.
            let back = FrameNumbers::default().rewrite(&rewritten, 16 - skipped % 16).unwrap().unwrap();
            assert_eq!(back, STREAM, "skipped {skipped}");
        }
        // Frame by frame, as AMF returns them, gives the same stream.
        let units = units(STREAM);
        let mut cuts: Vec<usize> = units[3..].iter().map(|unit| unit.0 - 4).collect();
        cuts.insert(0, 0);
        cuts.push(STREAM.len());
        let mut numbers = FrameNumbers::default();
        let mut frames = Vec::new();
        for cut in cuts.windows(2) {
            let frame = &STREAM[cut[0]..cut[1]];
            frames.extend(numbers.rewrite(frame, 3).unwrap().unwrap_or_else(|| frame.to_vec()));
        }
        assert_eq!(Some(frames), FrameNumbers::default().rewrite(STREAM, 3).unwrap());
    }

    #[test]
    fn malformed_h264_is_an_error_and_never_a_panic() {
        let units = units(STREAM);
        let numbers = |bytes: &[u8]| FrameNumbers::default().rewrite(bytes, 5);
        assert_eq!(
            numbers(&STREAM[units[3].0 - 4..]),
            Err(Unsupported("slice before its parameter sets"))
        );
        let mut disposable = STREAM.to_vec();
        disposable[units[3].0] &= !0x60;
        assert_eq!(numbers(&disposable), Err(Unsupported("non-reference H.264 frame")));
        // POC type 0 counts its own low bits; a gap would break those too.
        let mut sps = Writer::default();
        sps.bits(24, 66 << 16 | 40);
        sps.ue(0);
        sps.ue(0);
        sps.ue(0);
        let mut bytes = vec![0, 0, 0, 1, 0x67];
        bytes.extend(sps.finish());
        assert_eq!(numbers(&bytes), Err(Unsupported("H.264 POC type 0")));
        for cut in 2..9 {
            let mut bytes = STREAM[..units[0].0 + cut].to_vec();
            bytes.extend_from_slice(&STREAM[units[0].1..]);
            assert!(numbers(&bytes).is_err(), "SPS cut to {cut}");
        }
        let mut state = 0x2545_F491_4F6C_DD1D_u64;
        let mut random = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut rewrites = 0;
        for _ in 0..3000 {
            let mut bytes = STREAM[..units[12].1].to_vec();
            for _ in 0..1 + random() % 4 {
                let at = random() as usize % bytes.len();
                bytes[at] ^= 1 << (random() % 8);
            }
            bytes.truncate(bytes.len() - random() as usize % 48);
            if let Ok(Some(rewritten)) = numbers(&bytes) {
                assert_eq!(super::units(&rewritten).len(), super::units(&bytes).len());
                rewrites += 1;
            }
        }
        assert!(rewrites > 100, "{rewrites}");
    }
}
