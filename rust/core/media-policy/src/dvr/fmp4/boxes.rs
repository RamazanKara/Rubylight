//! ISO BMFF box writing for the fragmented MP4 muxer: a size-patching box builder, the
//! `ftyp` + `moov` init segment skeleton (`mvhd`, `trak`, `mvex` with `mehd`/`trex`), and
//! `moof` + `mdat` fragments with `tfdt` and per-sample `trun` entries.

use super::{AUDIO_TIMESCALE, MOVIE_TIMESCALE, VIDEO_TIMESCALE};

/// `trun` sample flags of a sync sample: depends on no other sample.
pub const SYNC_SAMPLE: u32 = 0x0200_0000;
/// `trun` sample flags of a non-sync sample: depends on others.
pub const NON_SYNC_SAMPLE: u32 = 0x0101_0000;

/// Appends boxes to one buffer, patching each size when it closes.
#[derive(Default)]
pub struct Bmff {
    pub buf: Vec<u8>,
    open: Vec<usize>,
}

impl Bmff {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn begin(&mut self, kind: &[u8; 4]) {
        self.open.push(self.buf.len());
        self.buf.extend_from_slice(&[0; 4]);
        self.buf.extend_from_slice(kind);
    }

    pub fn begin_full(&mut self, kind: &[u8; 4], version: u8, flags: u32) {
        self.begin(kind);
        self.u32((u32::from(version) << 24) | (flags & 0x00FF_FFFF));
    }

    pub fn end(&mut self) {
        if let Some(start) = self.open.pop() {
            let size = u32::try_from(self.buf.len() - start).unwrap_or(u32::MAX);
            self.patch_u32(start, size);
        }
    }

    pub fn u8(&mut self, value: u8) {
        self.buf.push(value);
    }

    pub fn u16(&mut self, value: u16) {
        self.buf.extend_from_slice(&value.to_be_bytes());
    }

    pub fn u32(&mut self, value: u32) {
        self.buf.extend_from_slice(&value.to_be_bytes());
    }

    pub fn u64(&mut self, value: u64) {
        self.buf.extend_from_slice(&value.to_be_bytes());
    }

    pub fn bytes(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    pub fn zeros(&mut self, count: usize) {
        self.buf.resize(self.buf.len() + count, 0);
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn patch_u32(&mut self, at: usize, value: u32) {
        if let Some(slot) = self.buf.get_mut(at..at + 4) {
            slot.copy_from_slice(&value.to_be_bytes());
        }
    }
}

const MATRIX: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];

pub fn ftyp(b: &mut Bmff, codec_brand: Option<&[u8; 4]>) {
    b.begin(b"ftyp");
    b.bytes(b"isom");
    b.u32(0x200);
    for brand in [b"isom", b"iso6", b"iso2", b"mp41"] {
        b.bytes(brand);
    }
    if let Some(brand) = codec_brand {
        b.bytes(brand);
    }
    b.end();
}

/// A track's sample description and display properties.
pub struct TrackBox<'a> {
    pub id: u32,
    pub audio: bool,
    pub width: u32,
    pub height: u32,
    /// The complete sample entry box (`avc1`, `hvc1`, `av01` or `Opus`).
    pub sample_entry: &'a [u8],
}

/// Writes `moov`; returns the offset of the `mehd` 64-bit duration within `b`.
pub fn moov(b: &mut Bmff, tracks: &[TrackBox<'_>], duration_ms: u64) -> usize {
    b.begin(b"moov");
    b.begin_full(b"mvhd", 0, 0);
    b.u32(0);
    b.u32(0);
    b.u32(MOVIE_TIMESCALE);
    b.u32(0);
    b.u32(0x0001_0000);
    b.u16(0x0100);
    b.zeros(10);
    MATRIX.iter().for_each(|&value| b.u32(value));
    b.zeros(24);
    b.u32(tracks.iter().map(|track| track.id).max().unwrap_or(0) + 1);
    b.end();
    for track in tracks {
        trak(b, track);
    }
    b.begin(b"mvex");
    b.begin_full(b"mehd", 1, 0);
    let mehd = b.len();
    b.u64(duration_ms);
    b.end();
    for track in tracks {
        b.begin_full(b"trex", 0, 0);
        b.u32(track.id);
        b.u32(1);
        b.u32(0);
        b.u32(0);
        b.u32(0);
        b.end();
    }
    b.end();
    b.end();
    mehd
}

fn trak(b: &mut Bmff, track: &TrackBox<'_>) {
    b.begin(b"trak");
    b.begin_full(b"tkhd", 0, 0x3);
    b.u32(0);
    b.u32(0);
    b.u32(track.id);
    b.u32(0);
    b.u32(0);
    b.zeros(8);
    b.u16(0);
    b.u16(u16::from(track.audio));
    b.u16(if track.audio { 0x0100 } else { 0 });
    b.u16(0);
    MATRIX.iter().for_each(|&value| b.u32(value));
    b.u32(track.width.min(0xFFFF) << 16);
    b.u32(track.height.min(0xFFFF) << 16);
    b.end();
    b.begin(b"mdia");
    b.begin_full(b"mdhd", 0, 0);
    b.u32(0);
    b.u32(0);
    b.u32(if track.audio {
        AUDIO_TIMESCALE
    } else {
        VIDEO_TIMESCALE
    });
    b.u32(0);
    b.u16(0x55C4);
    b.u16(0);
    b.end();
    b.begin_full(b"hdlr", 0, 0);
    b.u32(0);
    b.bytes(if track.audio { b"soun" } else { b"vide" });
    b.zeros(12);
    b.bytes(if track.audio {
        b"SoundHandler\0"
    } else {
        b"VideoHandler\0"
    });
    b.end();
    b.begin(b"minf");
    if track.audio {
        b.begin_full(b"smhd", 0, 0);
        b.u32(0);
    } else {
        b.begin_full(b"vmhd", 0, 1);
        b.zeros(8);
    }
    b.end();
    b.begin(b"dinf");
    b.begin_full(b"dref", 0, 0);
    b.u32(1);
    b.begin_full(b"url ", 0, 1);
    b.end();
    b.end();
    b.end();
    b.begin(b"stbl");
    b.begin_full(b"stsd", 0, 0);
    b.u32(1);
    b.bytes(track.sample_entry);
    b.end();
    for kind in [b"stts", b"stsc", b"stco"] {
        b.begin_full(kind, 0, 0);
        b.u32(0);
        b.end();
    }
    b.begin_full(b"stsz", 0, 0);
    b.u32(0);
    b.u32(0);
    b.end();
    b.end();
    b.end();
    b.end();
    b.end();
}

/// One sample of a `trun`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SampleMeta {
    pub size: u32,
    pub duration: u32,
    pub sync: bool,
}

/// One track's samples in a fragment, with their bytes in sample order.
pub struct Run<'a> {
    pub track_id: u32,
    pub base_time: u64,
    pub samples: &'a [SampleMeta],
    pub payload: &'a [u8],
    /// Writes per-sample sync flags (video); audio samples are all sync.
    pub sample_flags: bool,
}

/// One `moof` + `mdat`; runs without samples are left out.
pub fn fragment(sequence: u32, runs: &[Run<'_>]) -> Vec<u8> {
    let runs: Vec<&Run<'_>> = runs.iter().filter(|run| !run.samples.is_empty()).collect();
    let mut b = Bmff::new();
    b.begin(b"moof");
    b.begin_full(b"mfhd", 0, 0);
    b.u32(sequence);
    b.end();
    let mut offsets = Vec::with_capacity(runs.len());
    for run in &runs {
        b.begin(b"traf");
        b.begin_full(b"tfhd", 0, 0x0002_0000);
        b.u32(run.track_id);
        b.end();
        b.begin_full(b"tfdt", 1, 0);
        b.u64(run.base_time);
        b.end();
        let flags = 0x001 | 0x100 | 0x200 | if run.sample_flags { 0x400 } else { 0 };
        b.begin_full(b"trun", 0, flags);
        b.u32(u32::try_from(run.samples.len()).unwrap_or(u32::MAX));
        offsets.push(b.len());
        b.u32(0);
        for sample in run.samples {
            b.u32(sample.duration);
            b.u32(sample.size);
            if run.sample_flags {
                b.u32(if sample.sync {
                    SYNC_SAMPLE
                } else {
                    NON_SYNC_SAMPLE
                });
            }
        }
        b.end();
        b.end();
    }
    b.end();
    let payload: u64 = runs.iter().map(|run| run.payload.len() as u64).sum();
    let large = payload + 8 > u64::from(u32::MAX);
    let header = if large { 16 } else { 8 };
    let mut at = b.len() + header;
    for (slot, run) in offsets.into_iter().zip(&runs) {
        b.patch_u32(slot, u32::try_from(at).unwrap_or(u32::MAX));
        at += run.payload.len();
    }
    let mut out = b.buf;
    out.reserve(header + usize::try_from(payload).unwrap_or(0));
    if large {
        out.extend_from_slice(&1u32.to_be_bytes());
        out.extend_from_slice(b"mdat");
        out.extend_from_slice(&(payload + 16).to_be_bytes());
    } else {
        out.extend_from_slice(&(payload as u32 + 8).to_be_bytes());
        out.extend_from_slice(b"mdat");
    }
    for run in &runs {
        out.extend_from_slice(run.payload);
    }
    out
}
