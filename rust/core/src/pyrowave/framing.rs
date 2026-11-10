//! A framed PyroWave picture kept as pieces of the encoder's own output: the
//! codec's records, the padding records between them and the length words of
//! the legacy container, in wire order. The packetizer copies each byte once,
//! from the codec bitstream straight into its packet, where record framing
//! used to copy the whole picture into a framed buffer first.
//!
//! [`Framing::records`] and [`Framing::container`] run the same framing as
//! [`super::record_frame_with`] and [`super::container`]; their bytes on the
//! wire are the same (`framing_tests`).
use super::{PADDING, Sink};
use anyhow::{Result, bail, ensure};
use std::sync::{Mutex, PoisonError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    /// Bytes of the codec bitstream from this offset.
    Raw(usize),
    /// A padding record: its two header words, then zeros.
    Padding,
    /// A little-endian word of the legacy container.
    Word(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Piece {
    /// Where the piece starts on the wire, after the frame header.
    at: usize,
    len: usize,
    source: Source,
}

impl Piece {
    /// `out.len()` bytes of the piece from `within`, which the caller keeps
    /// inside it.
    fn fill(&self, raw: &[u8], within: usize, out: &mut [u8]) {
        match self.source {
            Source::Raw(offset) => {
                let from = offset + within;
                out.copy_from_slice(&raw[from..from + out.len()]);
            }
            Source::Word(word) => {
                out.copy_from_slice(&word.to_le_bytes()[within..within + out.len()]);
            }
            Source::Padding => {
                let mut head = [0; 8];
                head[..4].copy_from_slice(&PADDING.to_le_bytes());
                head[4..].copy_from_slice(&((self.len - 8) as u32 / 4).to_le_bytes());
                let header = 8usize.saturating_sub(within).min(out.len());
                if header > 0 {
                    out[..header].copy_from_slice(&head[within..within + header]);
                }
                out[header..].fill(0);
            }
        }
    }
}

/// Piece lists kept for the next frames: a 4K picture in row order has tens of
/// thousands of records, a list of a megabyte that would otherwise be mapped
/// and faulted in anew every frame.
static SPARE: Mutex<Vec<Vec<Piece>>> = Mutex::new(Vec::new());
/// One framing being built, one waiting and one being sent, and one more.
const KEPT: usize = 4;

/// A framed picture as pieces of the bitstream it was framed from; see the
/// module. It reads only that bitstream: [`Framing::view`] pairs them.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Framing {
    pieces: Vec<Piece>,
    len: usize,
    /// The end of the furthest bitstream bytes a piece takes.
    extent: usize,
}

impl Drop for Framing {
    fn drop(&mut self) {
        let mut pieces = std::mem::take(&mut self.pieces);
        if pieces.capacity() == 0 {
            return;
        }
        pieces.clear();
        let mut spare = SPARE.lock().unwrap_or_else(PoisonError::into_inner);
        if spare.len() < KEPT {
            spare.push(pieces);
        }
    }
}

impl Framing {
    fn new() -> Self {
        let pieces = SPARE
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop()
            .unwrap_or_default();
        Self {
            pieces,
            len: 0,
            extent: 0,
        }
    }

    /// [`super::record_frame_with`] as pieces of `bitstream`.
    pub fn records(bitstream: &[u8], payload: usize, limit: usize, rows: bool) -> Result<Self> {
        let mut out = Self::new();
        super::frame_into(&mut out, bitstream, payload, limit, rows)?;
        Ok(out)
    }

    /// [`super::container`] as pieces of `bitstream`: the packet count, then
    /// each packet's length and its bytes.
    pub fn container(
        bitstream: &[u8],
        packets: impl ExactSizeIterator<Item = (usize, usize)>,
    ) -> Result<Self> {
        let count = packets.len();
        if count == 0 || count > u32::MAX as usize {
            bail!("invalid PyroWave packet count");
        }
        let mut out = Self::new();
        out.push(4, Source::Word(count as u32));
        for (offset, size) in packets {
            let Some(end) = offset.checked_add(size) else {
                bail!("PyroWave packet overflow");
            };
            if size == 0 || size > u32::MAX as usize || end > bitstream.len() {
                bail!("invalid PyroWave packet slice");
            }
            out.push(4, Source::Word(size as u32));
            out.copy(bitstream, offset, size);
        }
        Ok(out)
    }

    /// Bytes on the wire.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether the framed picture is `raw` itself, unchanged.
    pub fn is_whole(&self, raw: &[u8]) -> bool {
        match self.pieces.as_slice() {
            [piece] => piece.at == 0 && piece.source == Source::Raw(0) && piece.len == raw.len(),
            _ => false,
        }
    }

    /// The framed picture over `raw`, the bitstream it was framed from.
    pub fn view<'a>(&'a self, raw: &'a [u8]) -> Result<Framed<'a>> {
        ensure!(
            self.extent <= raw.len(),
            "PyroWave framing reaches past its bitstream"
        );
        Ok(Framed { raw, framing: self })
    }

    fn push(&mut self, len: usize, source: Source) {
        if len == 0 {
            return;
        }
        if let Source::Raw(offset) = source
            && let Some(last) = self.pieces.last_mut()
            && let Source::Raw(previous) = last.source
            && previous + last.len == offset
        {
            last.len += len;
        } else {
            self.pieces.push(Piece {
                at: self.len,
                len,
                source,
            });
        }
        self.len += len;
    }
}

impl Sink for Framing {
    fn written(&self) -> usize {
        self.len
    }
    fn clear(&mut self) {
        self.pieces.clear();
        self.len = 0;
        self.extent = 0;
    }
    fn reserve(&mut self, _bytes: usize) {}
    fn copy(&mut self, bitstream: &[u8], offset: usize, len: usize) {
        debug_assert!(offset + len <= bitstream.len());
        self.extent = self.extent.max(offset + len);
        self.push(len, Source::Raw(offset));
    }
    fn pad(&mut self, bytes: usize) {
        debug_assert!(bytes >= 8 && bytes.is_multiple_of(4));
        self.push(bytes, Source::Padding);
    }
}

/// A [`Framing`] over the bitstream it was framed from.
#[derive(Clone, Copy, Debug)]
pub struct Framed<'a> {
    raw: &'a [u8],
    framing: &'a Framing,
}

impl Framed<'_> {
    pub fn len(&self) -> usize {
        self.framing.len
    }

    pub fn is_empty(&self) -> bool {
        self.framing.len == 0
    }

    /// The framed bytes from `at` into `out`; panics past the end, as a slice
    /// does.
    pub fn copy_to(&self, at: usize, out: &mut [u8]) {
        assert!(
            at + out.len() <= self.len(),
            "framed PyroWave read past its end"
        );
        let pieces = &self.framing.pieces;
        let mut index = pieces.partition_point(|p| p.at + p.len <= at);
        let (mut at, mut out) = (at, out);
        while !out.is_empty() {
            let piece = pieces[index];
            let within = at - piece.at;
            let n = (piece.len - within).min(out.len());
            let (head, rest) = std::mem::take(&mut out).split_at_mut(n);
            piece.fill(self.raw, within, head);
            out = rest;
            at += n;
            index += 1;
        }
    }
}

/// A picture as the packetizer reads it: contiguous bytes, or framed pieces.
#[derive(Clone, Copy, Debug)]
pub enum Payload<'a> {
    Bytes(&'a [u8]),
    Framed(Framed<'a>),
}

impl<'a> From<&'a [u8]> for Payload<'a> {
    fn from(bytes: &'a [u8]) -> Self {
        Self::Bytes(bytes)
    }
}

impl<'a> From<&'a Vec<u8>> for Payload<'a> {
    fn from(bytes: &'a Vec<u8>) -> Self {
        Self::Bytes(bytes)
    }
}

impl<'a, const N: usize> From<&'a [u8; N]> for Payload<'a> {
    fn from(bytes: &'a [u8; N]) -> Self {
        Self::Bytes(bytes)
    }
}

impl<'a> From<Framed<'a>> for Payload<'a> {
    fn from(framed: Framed<'a>) -> Self {
        Self::Framed(framed)
    }
}

impl<'a> Payload<'a> {
    pub fn len(&self) -> usize {
        match self {
            Self::Bytes(bytes) => bytes.len(),
            Self::Framed(framed) => framed.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The bytes from `at` into `out`; panics past the end, as a slice does.
    pub fn copy_to(&self, at: usize, out: &mut [u8]) {
        match self {
            Self::Bytes(bytes) => out.copy_from_slice(&bytes[at..at + out.len()]),
            Self::Framed(framed) => framed.copy_to(at, out),
        }
    }

    /// The whole picture in one buffer, for tests and diagnostics.
    pub fn to_vec(&self) -> Vec<u8> {
        let mut out = vec![0; self.len()];
        self.copy_to(0, &mut out);
        out
    }

    pub(super) fn reader(self) -> Reader<'a> {
        Reader {
            payload: self,
            index: 0,
        }
    }
}

/// Words of a payload, read mostly in increasing order (the record walks): a
/// framed one keeps its place among the pieces instead of searching for every
/// word.
pub(super) struct Reader<'a> {
    payload: Payload<'a>,
    index: usize,
}

impl Reader<'_> {
    pub(super) fn len(&self) -> usize {
        self.payload.len()
    }

    /// The little-endian word at `at`; panics past the end, as a slice does.
    pub(super) fn word(&mut self, at: usize) -> u32 {
        let framed = match self.payload {
            Payload::Bytes(bytes) => return super::word(bytes, at),
            Payload::Framed(framed) => framed,
        };
        let pieces = &framed.framing.pieces;
        if pieces.get(self.index).is_none_or(|piece| piece.at > at) {
            self.index = pieces.partition_point(|p| p.at + p.len <= at);
        }
        while pieces
            .get(self.index)
            .is_some_and(|piece| piece.at + piece.len <= at)
        {
            self.index += 1;
        }
        let mut bytes = [0; 4];
        match pieces.get(self.index) {
            Some(piece) if at + 4 <= piece.at + piece.len => {
                piece.fill(framed.raw, at - piece.at, &mut bytes);
            }
            // Across pieces, or past the end (which panics there).
            _ => framed.copy_to(at, &mut bytes),
        }
        u32::from_le_bytes(bytes)
    }
}
