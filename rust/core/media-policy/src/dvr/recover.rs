//! Crash salvage of a torn recording (`<id>.mp4.tmp`): walks the top-level boxes and
//! finds where the last complete `moof` + `mdat` pair ends, so the host can truncate the
//! file there and keep it when at least one fragment survived. It also sums the video
//! track's sample durations in those fragments (from `trun`, `tfhd` and `trex` defaults)
//! and returns the bytes that rewrite the init segment's `mehd` to that duration. Never
//! panics on arbitrary bytes.

use std::io::{self, Read, Seek, SeekFrom};

/// `moov` and `moof` larger than this are taken as corrupt rather than read.
const MAX_HEADER_BOX: u64 = 16 << 20;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Salvage {
    /// Length of the playable prefix: up to the end of the last complete fragment, or 0.
    pub valid_len: u64,
    /// Complete `moof` + `mdat` pairs in that prefix.
    pub fragments: u32,
    /// Video duration of those fragments.
    pub duration_us: u64,
    /// File offset and bytes that set the `mehd` duration to `duration_us`; `None` when
    /// the file has no `mehd` or no video track.
    pub mehd_patch: Option<(u64, Vec<u8>)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Expect {
    Ftyp,
    Moov,
    Moof,
    Mdat,
}

pub fn salvage<R: Read + Seek>(r: &mut R) -> io::Result<Salvage> {
    let len = r.seek(SeekFrom::End(0))?;
    let mut pos = 0u64;
    let mut expect = Expect::Ftyp;
    let mut found = Salvage::default();
    let mut movie = Movie::default();
    let (mut end_ticks, mut pending_end) = (0u64, 0u64);
    while let Some((kind, size, header_len)) = header(r, pos, len)? {
        let Some(end) = pos.checked_add(size).filter(|&end| end <= len) else {
            break;
        };
        expect = match (expect, &kind) {
            (Expect::Ftyp, b"ftyp") => Expect::Moov,
            (Expect::Moov, b"moov") => {
                if let Some(body) = read_body(r, pos, size, header_len)? {
                    movie = Movie::parse(&body, pos + header_len);
                }
                Expect::Moof
            }
            (Expect::Moov | Expect::Moof, b"free" | b"skip") => expect,
            (Expect::Moof, b"moof") => {
                let body = read_body(r, pos, size, header_len)?;
                pending_end = body
                    .and_then(|body| movie.fragment_end(&body, end_ticks))
                    .unwrap_or(end_ticks);
                Expect::Mdat
            }
            (Expect::Mdat, b"mdat") => {
                found.valid_len = end;
                found.fragments = found.fragments.saturating_add(1);
                end_ticks = end_ticks.max(pending_end);
                Expect::Moof
            }
            _ => break,
        };
        pos = end;
    }
    if let Some((_, timescale)) = movie.video.filter(|&(_, timescale)| timescale > 0) {
        found.duration_us = rescale(end_ticks, u64::from(timescale), 1_000_000);
        found.mehd_patch = movie.mehd.map(|(at, version)| {
            let value = rescale(found.duration_us, 1_000_000, u64::from(movie.timescale));
            let bytes = if version == 1 {
                value.to_be_bytes().to_vec()
            } else {
                u32::try_from(value)
                    .unwrap_or(u32::MAX)
                    .to_be_bytes()
                    .to_vec()
            };
            (at, bytes)
        });
    }
    Ok(found)
}

fn rescale(value: u64, from: u64, to: u64) -> u64 {
    if from == 0 {
        return 0;
    }
    let scaled = (u128::from(value) * u128::from(to) + u128::from(from / 2)) / u128::from(from);
    u64::try_from(scaled).unwrap_or(u64::MAX)
}

/// The box at `pos`: its type, total size and header length, or `None` when its header
/// is torn or invalid (a size-0 box would run to a torn end, so it counts as incomplete).
fn header<R: Read + Seek>(
    r: &mut R,
    pos: u64,
    len: u64,
) -> io::Result<Option<([u8; 4], u64, u64)>> {
    if pos.checked_add(8).is_none_or(|end| end > len) {
        return Ok(None);
    }
    r.seek(SeekFrom::Start(pos))?;
    let mut head = [0u8; 8];
    r.read_exact(&mut head)?;
    let mut kind = [0u8; 4];
    kind.copy_from_slice(&head[4..]);
    let size = u64::from(u32::from_be_bytes([head[0], head[1], head[2], head[3]]));
    let (size, header_len) = match size {
        0 => return Ok(None),
        1 => {
            if pos.checked_add(16).is_none_or(|end| end > len) {
                return Ok(None);
            }
            let mut large = [0u8; 8];
            r.read_exact(&mut large)?;
            (u64::from_be_bytes(large), 16)
        }
        _ => (size, 8),
    };
    Ok((size >= header_len).then_some((kind, size, header_len)))
}

/// A complete box's payload, unless it is implausibly large for a header box.
fn read_body<R: Read + Seek>(
    r: &mut R,
    pos: u64,
    size: u64,
    header_len: u64,
) -> io::Result<Option<Vec<u8>>> {
    if size > MAX_HEADER_BOX {
        return Ok(None);
    }
    r.seek(SeekFrom::Start(pos + header_len))?;
    let mut body = vec![0u8; usize::try_from(size - header_len).unwrap_or(0)];
    r.read_exact(&mut body)?;
    Ok(Some(body))
}

/// Child boxes of a payload: type, payload and the payload's offset in `data`.
fn children(data: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8], usize)> {
    let mut pos = 0usize;
    std::iter::from_fn(move || {
        let head = data.get(pos..pos.checked_add(8)?)?;
        let mut kind = [0u8; 4];
        kind.copy_from_slice(&head[4..]);
        let (size, header_len) = match be32(head, 0)? {
            0 => (data.len() - pos, 8),
            1 => (usize::try_from(be64(data, pos + 8)?).ok()?, 16),
            size => (usize::try_from(size).ok()?, 8),
        };
        let end = pos
            .checked_add(size)
            .filter(|&end| size >= header_len && end <= data.len())?;
        let item = (kind, &data[pos + header_len..end], pos + header_len);
        pos = end;
        Some(item)
    })
}

fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<(&'a [u8], usize)> {
    children(data).find_map(|(found, body, at)| (&found == kind).then_some((body, at)))
}

fn be32(data: &[u8], at: usize) -> Option<u32> {
    let bytes = data.get(at..at.checked_add(4)?)?;
    Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn be64(data: &[u8], at: usize) -> Option<u64> {
    let bytes = data.get(at..at.checked_add(8)?)?;
    let mut value = [0u8; 8];
    value.copy_from_slice(bytes);
    Some(u64::from_be_bytes(value))
}

/// What `moov` says about timing: the movie timescale, the `mehd` duration field, the
/// video track and the `trex` default durations.
#[derive(Default)]
struct Movie {
    timescale: u32,
    /// Absolute file offset of the `mehd` duration and the box version.
    mehd: Option<(u64, u8)>,
    /// Video track id and media timescale.
    video: Option<(u32, u32)>,
    trex: Vec<(u32, u32)>,
}

impl Movie {
    fn parse(body: &[u8], body_offset: u64) -> Self {
        let mut movie = Self::default();
        for (kind, payload, at) in children(body) {
            match &kind {
                b"mvhd" => {
                    let skip = if payload.first() == Some(&1) { 20 } else { 12 };
                    movie.timescale = be32(payload, skip).unwrap_or(0);
                }
                b"trak" => {
                    if let Some(video) = video_track(payload) {
                        movie.video = movie.video.or(Some(video));
                    }
                }
                b"mvex" => {
                    for (kind, payload, inner) in children(payload) {
                        match &kind {
                            b"mehd" => {
                                if let Some(&version) = payload.first() {
                                    let offset = (at + inner + 4) as u64;
                                    movie.mehd = Some((body_offset + offset, version));
                                }
                            }
                            b"trex" => {
                                if let (Some(id), Some(duration)) =
                                    (be32(payload, 4), be32(payload, 12))
                                {
                                    movie.trex.push((id, duration));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        movie
    }

    /// The video timeline end after this `moof`, continuing from `end`.
    fn fragment_end(&self, moof: &[u8], end: u64) -> Option<u64> {
        let (video_id, _) = self.video?;
        let mut fragment_end = end;
        for (kind, traf, _) in children(moof) {
            if &kind != b"traf" {
                continue;
            }
            let (tfhd, _) = child(traf, b"tfhd")?;
            let flags = be32(tfhd, 0)? & 0x00FF_FFFF;
            if be32(tfhd, 4)? != video_id {
                continue;
            }
            let mut at = 8;
            at += if flags & 0x01 != 0 { 8 } else { 0 };
            at += if flags & 0x02 != 0 { 4 } else { 0 };
            let default_duration = if flags & 0x08 != 0 {
                be32(tfhd, at)?
            } else {
                self.trex
                    .iter()
                    .find(|&&(id, _)| id == video_id)
                    .map_or(0, |&(_, duration)| duration)
            };
            let base = match child(traf, b"tfdt") {
                Some((tfdt, _)) if tfdt.first() == Some(&1) => be64(tfdt, 4)?,
                Some((tfdt, _)) => u64::from(be32(tfdt, 4)?),
                None => fragment_end,
            };
            let mut total = 0u64;
            for (kind, trun, _) in children(traf) {
                if &kind == b"trun" {
                    total = total.saturating_add(run_duration(trun, default_duration)?);
                }
            }
            fragment_end = fragment_end.max(base.saturating_add(total));
        }
        Some(fragment_end)
    }
}

/// A `trak`'s id and timescale when its handler is video.
fn video_track(trak: &[u8]) -> Option<(u32, u32)> {
    let (tkhd, _) = child(trak, b"tkhd")?;
    let id = be32(tkhd, if tkhd.first() == Some(&1) { 20 } else { 12 })?;
    let (mdia, _) = child(trak, b"mdia")?;
    let (hdlr, _) = child(mdia, b"hdlr")?;
    if hdlr.get(8..12)? != b"vide" {
        return None;
    }
    let (mdhd, _) = child(mdia, b"mdhd")?;
    let timescale = be32(mdhd, if mdhd.first() == Some(&1) { 20 } else { 12 })?;
    Some((id, timescale))
}

/// The summed sample durations of one `trun`.
fn run_duration(trun: &[u8], default_duration: u32) -> Option<u64> {
    let flags = be32(trun, 0)? & 0x00FF_FFFF;
    let count = usize::try_from(be32(trun, 4)?).ok()?;
    let mut at = 8;
    at += if flags & 0x01 != 0 { 4 } else { 0 };
    at += if flags & 0x04 != 0 { 4 } else { 0 };
    let entry = [0x100, 0x200, 0x400, 0x800]
        .iter()
        .filter(|&&bit| flags & bit != 0)
        .count()
        * 4;
    let table = count.checked_mul(entry)?;
    if trun.len() < at.checked_add(table)? {
        return None;
    }
    if flags & 0x100 == 0 {
        return Some(count as u64 * u64::from(default_duration));
    }
    Some(
        (0..count)
            .filter_map(|index| be32(trun, at + index * entry))
            .map(u64::from)
            .sum(),
    )
}
