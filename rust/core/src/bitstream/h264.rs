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
use super::rbsp::{Reader, Result, Unsupported, assemble, escape, unescape, units};

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

    fn unit(&mut self, nal: &[u8], skipped: u64) -> Result<Option<Vec<u8>>> {
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

    fn slice(&self, nal: &[u8], skipped: u64) -> Result<Option<Vec<u8>>> {
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
        let mut output = Vec::with_capacity(nal.len() + 4);
        output.push(nal[0]);
        escape(&rbsp[..end], &mut output);
        output.extend_from_slice(&nal[1 + from[end - 1] + 1..]);
        Ok(Some(output))
    }
}

/// The SPS id, log2_max_frame_num and separate_colour_plane_flag.
fn sequence_parameter_set(rbsp: &[u8]) -> Result<(usize, (u32, bool))> {
    let mut reader = Reader::new(rbsp);
    let profile = reader.bits(8)?;
    reader.skip(16)?;
    let id = reader.ue_max(31, "SPS id")? as usize;
    let mut separate_colour_plane = false;
    if [100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135].contains(&profile) {
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
