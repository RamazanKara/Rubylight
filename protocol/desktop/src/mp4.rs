//! The samples of an MP4 file's first video track, as a stream hands frames to the decoder: H.264
//! and HEVC as Annex B (start codes, the parameter sets before the key frame), AV1 as the low
//! overhead OBU stream with a temporal delimiter before each frame. The latency benchmark decodes
//! Nightjar Android's bundled clips with it (`latency_bench`); FFmpeg here is decode-only, without
//! libavformat.
//!
//! Only what those clips use is read: one sample description, `stsz`, `stco`/`co64`, `stsc`,
//! `stss` and `stts`. Anything malformed is an error, never a panic.

/// The codecs a clip can carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    H264,
    Hevc,
    Av1,
}

/// A video track's frames, ready for the decoder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clip {
    pub codec: Codec,
    pub width: u16,
    pub height: u16,
    /// Frames a second, from the first `stts` entry; 0 when the file doesn't say.
    pub fps: u32,
    /// Each frame's bytes in decoding order.
    pub samples: Vec<Vec<u8>>,
}

const START: [u8; 4] = [0, 0, 0, 1];
/// An AV1 temporal delimiter: OBU type 2 with an empty payload and its size field.
const TEMPORAL_DELIMITER: [u8; 2] = [0x12, 0x00];
const OBU_SEQUENCE_HEADER: u8 = 1;
/// The most frames a track may have here.
const MAX_FRAMES: usize = 1 << 16;

/// The boxes in `data`, each its type and its payload.
fn boxes(data: &[u8]) -> impl Iterator<Item = Result<([u8; 4], &[u8]), String>> {
    let mut rest = data;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        Some(match next_box(rest) {
            Ok((kind, payload, after)) => {
                rest = after;
                Ok((kind, payload))
            }
            Err(error) => {
                rest = &[];
                Err(error)
            }
        })
    })
}

/// A box's type, its payload and what follows it.
type BoxParts<'a> = ([u8; 4], &'a [u8], &'a [u8]);

/// The first box of `data`: its type, its payload and what follows it.
fn next_box(data: &[u8]) -> Result<BoxParts<'_>, String> {
    let header = data.get(..8).ok_or("a box header is cut short")?;
    let kind: [u8; 4] = header[4..8].try_into().unwrap();
    let (size, skip) = match u32::from_be_bytes(header[..4].try_into().unwrap()) {
        0 => (data.len(), 8),
        1 => {
            let large = data.get(8..16).ok_or("a large box header is cut short")?;
            (usize::try_from(u64::from_be_bytes(large.try_into().unwrap())).map_err(|_| "a box is too large")?, 16)
        }
        size => (size as usize, 8),
    };
    if size < skip || size > data.len() {
        return Err(format!("the {} box runs past its parent", String::from_utf8_lossy(&kind)));
    }
    Ok((kind, &data[skip..size], &data[size..]))
}

/// The payload of the first `kind` box in `data`.
fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Result<&'a [u8], String> {
    for found in boxes(data) {
        let (found, payload) = found?;
        if &found == kind {
            return Ok(payload);
        }
    }
    Err(format!("no {} box", String::from_utf8_lossy(kind)))
}

fn path<'a>(data: &'a [u8], kinds: &[&[u8; 4]]) -> Result<&'a [u8], String> {
    kinds.iter().try_fold(data, |data, kind| child(data, kind))
}

fn u16_at(data: &[u8], at: usize) -> Result<u16, String> {
    data.get(at..at + 2).map(|b| u16::from_be_bytes(b.try_into().unwrap())).ok_or_else(|| "a field is cut short".into())
}

fn u32_at(data: &[u8], at: usize) -> Result<u32, String> {
    data.get(at..at + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap())).ok_or_else(|| "a field is cut short".into())
}

/// A full box's entries after its version, flags and count: `count` entries of `size` bytes.
fn entries(payload: &[u8], size: usize) -> Result<impl Iterator<Item = &[u8]>, String> {
    let count = u32_at(payload, 4)? as usize;
    let table = payload.get(8..).ok_or("a table is cut short")?;
    if count.checked_mul(size).is_none_or(|bytes| bytes > table.len()) {
        return Err("a table is cut short".into());
    }
    Ok(table.chunks_exact(size).take(count))
}

/// The first video track's frames.
pub fn read(data: &[u8]) -> Result<Clip, String> {
    let moov = child(data, b"moov")?;
    for trak in boxes(moov) {
        let (kind, trak) = trak?;
        if &kind != b"trak" {
            continue;
        }
        let mdia = child(trak, b"mdia")?;
        if child(mdia, b"hdlr")?.get(8..12) != Some(&b"vide"[..]) {
            continue;
        }
        let mdhd = child(mdia, b"mdhd")?;
        let timescale = if mdhd.first() == Some(&1) { u32_at(mdhd, 20)? } else { u32_at(mdhd, 12)? };
        return track(path(mdia, &[b"minf", b"stbl"])?, data, timescale);
    }
    Err("no video track".into())
}

fn track(stbl: &[u8], file: &[u8], timescale: u32) -> Result<Clip, String> {
    let stsd = child(stbl, b"stsd")?;
    let (kind, entry) = boxes(stsd.get(8..).ok_or("the sample description is cut short")?).next().ok_or("no sample description")??;
    // A visual sample entry's fields, then its boxes.
    let (width, height) = (u16_at(entry, 24)?, u16_at(entry, 26)?);
    let config_boxes = entry.get(78..).ok_or("the sample description is cut short")?;
    let (codec, config) = match &kind {
        b"avc1" | b"avc3" => (Codec::H264, child(config_boxes, b"avcC")?),
        b"hvc1" | b"hev1" => (Codec::Hevc, child(config_boxes, b"hvcC")?),
        b"av01" => (Codec::Av1, child(config_boxes, b"av1C")?),
        other => return Err(format!("{} video is not read", String::from_utf8_lossy(other))),
    };
    let fps = match child(stbl, b"stts").ok().map(|stts| entries(stts, 8)).transpose()?.and_then(|mut e| e.next()) {
        Some(first) => {
            let delta = u32_at(first, 4)?;
            if delta == 0 { 0 } else { ((f64::from(timescale) / f64::from(delta)).round()) as u32 }
        }
        None => 0,
    };
    let frames = sample_ranges(stbl)?;
    let keyframes: Option<Vec<u32>> = match child(stbl, b"stss") {
        Ok(stss) => Some(entries(stss, 4)?.map(|e| u32::from_be_bytes(e.try_into().unwrap())).collect()),
        Err(_) => None,
    };
    let key = |index: usize| keyframes.as_ref().is_none_or(|keys| keys.contains(&(index as u32 + 1)));
    let mut samples = Vec::with_capacity(frames.len());
    match codec {
        Codec::H264 | Codec::Hevc => {
            let (length_size, parameters) = if codec == Codec::H264 { avc_config(config)? } else { hevc_config(config)? };
            for (index, &(start, size)) in frames.iter().enumerate() {
                let sample = frame_bytes(file, start, size)?;
                let mut out = Vec::with_capacity(size + 64);
                if key(index) {
                    for parameter in &parameters {
                        out.extend_from_slice(&START);
                        out.extend_from_slice(parameter);
                    }
                }
                annex_b(sample, length_size, &mut out)?;
                samples.push(out);
            }
        }
        Codec::Av1 => {
            // av1C: four bytes of its own, then the configuration OBUs (the sequence header).
            let headers = config.get(4..).ok_or("the AV1 configuration is cut short")?;
            for (index, &(start, size)) in frames.iter().enumerate() {
                let sample = frame_bytes(file, start, size)?;
                let mut out = Vec::with_capacity(size + headers.len() + 2);
                out.extend_from_slice(&TEMPORAL_DELIMITER);
                if key(index) && !obu_types(sample)?.contains(&OBU_SEQUENCE_HEADER) {
                    out.extend_from_slice(headers);
                }
                out.extend_from_slice(sample);
                samples.push(out);
            }
        }
    }
    if samples.is_empty() {
        return Err("the track has no frames".into());
    }
    Ok(Clip { codec, width, height, fps, samples })
}

fn frame_bytes(file: &[u8], start: usize, size: usize) -> Result<&[u8], String> {
    start.checked_add(size).and_then(|end| file.get(start..end)).ok_or_else(|| "a frame lies past the end of the file".into())
}

/// Each sample's offset in the file and its size, from `stsz`, `stsc` and `stco` or `co64`.
fn sample_ranges(stbl: &[u8]) -> Result<Vec<(usize, usize)>, String> {
    let stsz = child(stbl, b"stsz")?;
    let fixed = u32_at(stsz, 4)? as usize;
    let count = u32_at(stsz, 8)? as usize;
    // Far more than any clip read here; a damaged count must not ask for gigabytes.
    if count > MAX_FRAMES {
        return Err(format!("{count} frames are more than this reader takes"));
    }
    let sizes: Vec<usize> = if fixed != 0 {
        vec![fixed; count]
    } else {
        let table = stsz.get(12..).ok_or("the sample sizes are cut short")?;
        if count.checked_mul(4).is_none_or(|bytes| bytes > table.len()) {
            return Err("the sample sizes are cut short".into());
        }
        table.as_chunks::<4>().0.iter().take(count).map(|b| u32::from_be_bytes(*b) as usize).collect()
    };
    let chunks: Vec<u64> = match child(stbl, b"stco") {
        Ok(stco) => entries(stco, 4)?.map(|e| u64::from(u32::from_be_bytes(e.try_into().unwrap()))).collect(),
        Err(_) => entries(child(stbl, b"co64")?, 8)?.map(|e| u64::from_be_bytes(e.try_into().unwrap())).collect(),
    };
    let runs: Vec<(u32, u32)> = entries(child(stbl, b"stsc")?, 12)?
        .map(|e| (u32::from_be_bytes(e[..4].try_into().unwrap()), u32::from_be_bytes(e[4..8].try_into().unwrap())))
        .collect();
    let mut ranges = Vec::with_capacity(count);
    let mut sizes = sizes.into_iter();
    for (index, &offset) in chunks.iter().enumerate() {
        let chunk = index as u32 + 1;
        let per_chunk = runs.iter().rev().find(|(first, _)| *first <= chunk).map_or(0, |&(_, n)| n);
        let mut at = usize::try_from(offset).map_err(|_| "a chunk lies past the end of the file")?;
        for _ in 0..per_chunk {
            let Some(size) = sizes.next() else { break };
            ranges.push((at, size));
            at = at.checked_add(size).ok_or("a chunk lies past the end of the file")?;
        }
    }
    if ranges.len() != count {
        return Err(format!("the chunks hold {} of {count} frames", ranges.len()));
    }
    Ok(ranges)
}

/// `avcC`: the NAL length size and the SPS and PPS.
fn avc_config(config: &[u8]) -> Result<(usize, Vec<Vec<u8>>), String> {
    let short = || "the H.264 configuration is cut short".to_owned();
    let length_size = usize::from(config.get(4).ok_or_else(short)? & 3) + 1;
    let mut at = 5;
    let mut parameters = Vec::new();
    for mask in [0x1f, 0xff] {
        let count = config.get(at).ok_or_else(short)? & mask;
        at += 1;
        for _ in 0..count {
            let size = usize::from(u16_at(config, at)?);
            parameters.push(config.get(at + 2..at + 2 + size).ok_or_else(short)?.to_vec());
            at += 2 + size;
        }
    }
    Ok((length_size, parameters))
}

/// `hvcC`: the NAL length size and its arrays' NAL units (VPS, SPS, PPS and SEI) in order.
fn hevc_config(config: &[u8]) -> Result<(usize, Vec<Vec<u8>>), String> {
    let short = || "the HEVC configuration is cut short".to_owned();
    let length_size = usize::from(config.get(21).ok_or_else(short)? & 3) + 1;
    let arrays = *config.get(22).ok_or_else(short)?;
    let mut at = 23;
    let mut parameters = Vec::new();
    for _ in 0..arrays {
        let count = u16_at(config, at + 1)?;
        at += 3;
        for _ in 0..count {
            let size = usize::from(u16_at(config, at)?);
            parameters.push(config.get(at + 2..at + 2 + size).ok_or_else(short)?.to_vec());
            at += 2 + size;
        }
    }
    Ok((length_size, parameters))
}

/// Length-prefixed NAL units to start codes.
fn annex_b(sample: &[u8], length_size: usize, out: &mut Vec<u8>) -> Result<(), String> {
    let mut rest = sample;
    while !rest.is_empty() {
        let length = rest.get(..length_size).ok_or("a NAL length is cut short")?;
        let size = length.iter().fold(0usize, |size, &b| (size << 8) | usize::from(b));
        let nal = rest.get(length_size..length_size + size).ok_or("a NAL unit runs past its frame")?;
        out.extend_from_slice(&START);
        out.extend_from_slice(nal);
        rest = &rest[length_size + size..];
    }
    Ok(())
}

/// The types of the OBUs in a low overhead AV1 sample, each with its size field.
fn obu_types(sample: &[u8]) -> Result<Vec<u8>, String> {
    let mut types = Vec::new();
    let mut at = 0;
    while at < sample.len() {
        let header = sample[at];
        let extension = usize::from(header >> 2 & 1);
        if header >> 1 & 1 == 0 {
            // No size field: it runs to the end of the sample.
            types.push(header >> 3 & 0xf);
            break;
        }
        at += 1 + extension;
        let (mut size, mut shift) = (0usize, 0);
        loop {
            let byte = *sample.get(at).ok_or("an OBU size is cut short")?;
            at += 1;
            size |= usize::from(byte & 0x7f) << shift;
            shift += 7;
            if byte & 0x80 == 0 {
                break;
            }
            if shift > 56 {
                return Err("an OBU size is too long".into());
            }
        }
        types.push(header >> 3 & 0xf);
        at = at.checked_add(size).filter(|&end| end <= sample.len()).ok_or("an OBU runs past its frame")?;
    }
    Ok(types)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::latency_bench::{AV1_CLIP, H264_CLIP, HEVC_CLIP};

    /// The NAL units of an Annex B frame (start codes as this reader writes them).
    fn nal_units(frame: &[u8]) -> Vec<&[u8]> {
        assert!(frame.starts_with(&START), "a frame starts with a start code");
        let mut units = Vec::new();
        let mut rest = &frame[4..];
        loop {
            match rest.windows(4).position(|w| w == START) {
                Some(at) => {
                    units.push(&rest[..at]);
                    rest = &rest[at + 4..];
                }
                None => {
                    units.push(rest);
                    return units;
                }
            }
        }
    }

    #[test]
    fn the_h264_clip_reads_as_annex_b_with_its_parameter_sets() {
        let clip = read(H264_CLIP).unwrap();
        assert_eq!((clip.codec, clip.width, clip.height, clip.fps, clip.samples.len()), (Codec::H264, 1920, 1080, 60, 24));
        let types = |frame: &[u8]| nal_units(frame).iter().map(|nal| nal[0] & 0x1f).collect::<Vec<_>>();
        assert_eq!(types(&clip.samples[0]), [7, 8, 6, 5, 5, 5, 5], "SPS and PPS, then the key frame's SEI and slices");
        for frame in &clip.samples[1..] {
            assert!(types(frame).iter().all(|&t| t == 1), "the other frames are slices only");
        }
    }

    #[test]
    fn the_hevc_clip_reads_as_annex_b_with_its_parameter_sets() {
        let clip = read(HEVC_CLIP).unwrap();
        assert_eq!((clip.codec, clip.width, clip.height, clip.fps, clip.samples.len()), (Codec::Hevc, 1920, 1080, 60, 24));
        let types = |frame: &[u8]| nal_units(frame).iter().map(|nal| nal[0] >> 1 & 0x3f).collect::<Vec<_>>();
        assert_eq!(types(&clip.samples[0]), [32, 33, 34, 39, 20], "VPS, SPS, PPS and SEI, then the IDR frame");
        for frame in &clip.samples[1..] {
            assert_eq!(types(frame), [1]);
        }
    }

    #[test]
    fn the_av1_clip_reads_as_obus_with_temporal_delimiters() {
        let clip = read(AV1_CLIP).unwrap();
        assert_eq!((clip.codec, clip.width, clip.height, clip.fps, clip.samples.len()), (Codec::Av1, 1920, 1080, 60, 24));
        for frame in &clip.samples {
            assert!(frame.starts_with(&TEMPORAL_DELIMITER));
        }
        assert_eq!(obu_types(&clip.samples[0]).unwrap(), [2, OBU_SEQUENCE_HEADER, 6], "the key frame keeps its one sequence header");
        assert_eq!(obu_types(&clip.samples[1]).unwrap(), [2, 6]);
    }

    #[test]
    fn a_key_frame_without_a_sequence_header_gets_the_configured_one() {
        let headers = [0x0a, 0x01, 0xaa];
        let frame = [0x32, 0x01, 0xbb];
        let mut out = TEMPORAL_DELIMITER.to_vec();
        if !obu_types(&frame).unwrap().contains(&OBU_SEQUENCE_HEADER) {
            out.extend_from_slice(&headers);
        }
        out.extend_from_slice(&frame);
        assert_eq!(obu_types(&out).unwrap(), [2, 1, 6]);
    }

    #[test]
    fn every_frame_has_the_bytes_of_its_sample() {
        for (data, overhead) in [(H264_CLIP, 4), (HEVC_CLIP, 4), (AV1_CLIP, 0)] {
            let clip = read(data).unwrap();
            // Start codes replace the four-byte lengths one for one after the first frame.
            let ranges = sample_ranges(path(child(child(child(data, b"moov").unwrap(), b"trak").unwrap(), b"mdia").unwrap(), &[b"minf", b"stbl"]).unwrap()).unwrap();
            for (frame, (_, size)) in clip.samples.iter().zip(ranges).skip(1) {
                let extra = if overhead == 0 { TEMPORAL_DELIMITER.len() } else { 0 };
                assert_eq!(frame.len(), size + extra);
            }
        }
    }

    #[test]
    fn a_cut_or_damaged_file_is_an_error_not_a_panic() {
        for data in [H264_CLIP, HEVC_CLIP, AV1_CLIP] {
            let moov_end = data.windows(4).position(|w| w == b"mdat").unwrap();
            for cut in (0..moov_end + 64).step_by(7) {
                let _ = read(&data[..cut]);
            }
            assert!(read(&data[..moov_end + 64]).is_err(), "the frames are missing");
            let mut damaged = data.to_vec();
            for at in (0..moov_end).step_by(13) {
                damaged[at] ^= 0xff;
                let _ = read(&damaged);
                damaged[at] ^= 0xff;
            }
        }
        assert!(read(b"").is_err());
        assert!(read(&[0, 0, 0, 1, b'm', b'o', b'o', b'v']).is_err());
    }
}
