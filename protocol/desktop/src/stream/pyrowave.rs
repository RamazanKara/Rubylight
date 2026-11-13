//! Rust decoder for PyroWave's 186f0393 bitstream.
//! Coefficient ordering and CDF 9/7 lifting follow Themaister/pyrowave a0d2e71 (MIT,
//! Copyright (c) 2026 Hans-Kristian Arntzen; app/src/main/assets/pyrowave_notices.txt).

use std::cell::RefCell;
use std::time::Instant;

use rubylight_protocol::pyrowave::rows::{band_rows, slice_ends, RowReport, RowTracker};
use rubylight_protocol::pyrowave::{push_container, Records};

use crate::media::Picture;
use super::stage_times::Stages;
use super::pyrowave_sdk::GpuDecoder;
#[cfg(not(target_os = "macos"))]
use super::pyrowave_sdk::GpuConfig;

struct Band {
    component: usize,
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    first: usize,
    count: usize,
}

/// Where early decoding of the frame in flight stands.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Early {
    /// No chunk of this frame was taken: the whole-frame path decodes it.
    Off,
    /// Chunks since the first one are decoded into the planes as their records complete.
    Active,
    /// Something did not parse or decode: the frame decodes whole, from its envelope.
    Failed,
}

/// How the last `coefficients` call got its blocks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Source {
    /// All in the call, from the finished frame.
    #[default]
    Frame,
    /// The blocks that arrived before the frame were already decoded; the call did the rest.
    Early,
    /// Early chunks were taken but did not match the frame, so the call decoded all of it.
    Fallback,
}

pub struct Decoder {
    pub width: usize,
    pub height: usize,
    pub chroma444: bool,
    pub loss_percent: f32,
    /// How the last frame was decoded, and how many of its blocks were decoded before it
    /// completed.
    pub source: Source,
    pub early_blocks: usize,
    /// When the first block of the last early-decoded frame was decoded (session clock, us);
    /// 0 for frames decoded whole.
    pub first_block_us: u64,
    early: Early,
    /// A chunk came that is not the first of a frame while none was in flight: the start of
    /// the frame was lost, and the call that gets the frame counts it as a fallback.
    missed_start: bool,
    /// Envelope bytes the chunks add up to, to check against the frame.
    early_bytes: usize,
    early_done: usize,
    stride: usize,
    rows: usize,
    planes: [Vec<f32>; 3],
    bands: Vec<Band>,
    seen: Vec<bool>,
    records: Records,
    packet: Vec<u8>,
    /// Where each frame's time went (`STAGE_NAMES`); `coefficient_frame` takes `&self`.
    stages: RefCell<Stages<5>>,
    #[cfg(test)] scratch: Vec<f32>,
    #[cfg(test)] line: Vec<f32>,
}

const ASSEMBLE: usize = 0;
const CLEAR: usize = 1;
const ENTROPY: usize = 2;
const PACK: usize = 3;
const ALLOC: usize = 4;
const STAGE_NAMES: [&str; 5] = ["assemble", "clear", "entropy", "pack", "alloc"];

/// The file name of dumped envelope `n` (`NIGHTJAR_PYROWAVE_DUMP`).
pub fn dump_name(width: usize, height: usize, chroma444: bool, n: u32) -> String {
    format!("frame-{width}x{height}-{}-{n:03}.bin", if chroma444 { 444 } else { 420 })
}

/// The picture size and chroma format a dumped file name carries.
#[cfg(test)]
fn parse_dump_name(name: &str) -> Option<(usize, usize, bool)> {
    let mut parts = name.strip_prefix("frame-")?.strip_suffix(".bin")?.split('-');
    let (width, height) = parts.next()?.split_once('x')?;
    let full = match parts.next()? { "420" => false, "444" => true, _ => return None };
    parts.next()?.parse::<u32>().ok()?;
    Some((width.parse().ok()?, height.parse().ok()?, full))
}

fn word(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

impl Decoder {
    pub fn new(width: usize, height: usize, chroma444: bool) -> Result<Self, &'static str> {
        if width == 0 || height == 0 || width > 8192 || height > 8192
            || width * height > 16_777_216 || (!chroma444 && (!width.is_multiple_of(2) || !height.is_multiple_of(2))) {
            return Err("PyroWave supports up to 16 megapixels and even 4:2:0 sizes");
        }
        let stride = width.div_ceil(32).max(4) * 32;
        let rows = height.div_ceil(32).max(4) * 32;
        let mut bands = Vec::new();
        let mut count = 0;
        for level in (0..5).rev() {
            for component in 0..3 {
                if level == 0 && component != 0 && !chroma444 { continue; }
                let (w, h) = (stride >> (level + 1), rows >> (level + 1));
                for band in if level == 4 { 0..4 } else { 1..4 } {
                    let blocks = w.div_ceil(32) * h.div_ceil(32);
                    bands.push(Band { component, width: w, height: h, x: (band & 1) * w,
                        y: (band >> 1) * h, first: count, count: blocks });
                    count += blocks;
                }
            }
        }
        Ok(Self { width, height, chroma444, loss_percent: 0.0, source: Source::Frame, early_blocks: 0,
            first_block_us: 0, early: Early::Off, missed_start: false, early_bytes: 0, early_done: 0, stride, rows,
            planes: std::array::from_fn(|_| vec![0.0; stride * rows]), bands, seen: vec![false; count],
            records: Records::with_geometry(width as u32, height as u32, chroma444), packet: Vec::new(),
            stages: RefCell::new(Stages::new(STAGE_NAMES)),
            #[cfg(test)] scratch: vec![0.0; stride * rows], #[cfg(test)] line: vec![0.0; stride.max(rows)] })
    }

    #[cfg(test)]
    pub fn decode(&mut self, data: &[u8]) -> Result<(), &'static str> {
        self.coefficients(data)?;
        self.reconstruct();
        Ok(())
    }

    /// Takes one packet of the records envelope of the frame that `coefficients` will get next,
    /// as it arrives (`first` starts the frame), and decodes the blocks it completes. A frame
    /// that does not parse here is left for `coefficients`, which finds out from the envelope.
    pub fn chunk(&mut self, first: bool, flags: u32, payload: &[u8]) {
        if first {
            let step = Instant::now();
            self.stages.get_mut().reset();
            self.records.begin_frame();
            for plane in &mut self.planes { plane.fill(0.0); }
            self.seen.fill(false);
            self.early = Early::Active;
            self.early_bytes = 4;
            self.early_done = 0;
            self.first_block_us = 0;
            self.missed_start = false;
            self.stages.get_mut().add(CLEAR, step);
        } else if self.early == Early::Off {
            self.missed_start = true;
            return;
        }
        let step = Instant::now();
        self.take_chunk(flags, payload);
        self.stages.get_mut().add(ENTROPY, step);
    }

    fn take_chunk(&mut self, flags: u32, payload: &[u8]) {
        if self.early != Early::Active { return; }
        self.early_bytes += 8 + payload.len();
        if !self.records.feed_packet(flags, payload) { self.early = Early::Failed; return; }
        let Self { records, bands, planes, seen, stride, early_done, .. } = self;
        let mut failed = false;
        records.drain_new(|_, record| {
            if failed { return; }
            match record_index(record).and_then(|(index, record)| place_block(bands, *stride, planes, seen, index, record)) {
                Ok(placed) => *early_done += usize::from(placed),
                Err(_) => failed = true,
            }
        });
        if failed { self.early = Early::Failed; }
        if self.early_done > 0 && self.first_block_us == 0 { self.first_block_us = nightjar_core::session::now_us(); }
    }

    /// Passes over a frame that a newer one replaces: nothing is decoded, and the records cache
    /// takes the frame as `coefficients` would have. What its chunks placed in the planes goes
    /// with the next frame, which clears them either way.
    pub fn skip(&mut self, data: &[u8]) {
        let early = std::mem::replace(&mut self.early, Early::Off);
        self.missed_start = false;
        self.source = Source::Frame;
        self.early_blocks = 0;
        self.first_block_us = 0;
        pass_records(&mut self.records, early == Early::Active && self.early_bytes == data.len(), data);
    }

    pub fn coefficients(&mut self, data: &[u8]) -> Result<(), &'static str> {
        self.source = Source::Frame;
        let early = std::mem::replace(&mut self.early, Early::Off);
        let missed_start = std::mem::take(&mut self.missed_start);
        if early == Early::Off { self.stages.get_mut().reset(); }
        self.source = Source::Frame;
        self.early_blocks = 0;
        if data.len() > rubylight_protocol::pyrowave::MAX_FRAME_BYTES { return Err("PyroWave frame exceeds the packet budget"); }
        let records = data.starts_with(&[0, 0, 0, 0]);
        self.packet.clear();
        let mut finished = false;
        if early != Early::Off {
            // The chunks are the frame only if they add up to its envelope; the frame decides.
            if early == Early::Active && records && self.early_bytes == data.len() {
                let step = Instant::now();
                let packet = &mut self.packet;
                finished = self.records.finish_frame(|record| { packet.extend_from_slice(record); true });
                if !finished { self.packet.clear(); }
                self.stages.get_mut().add(ASSEMBLE, step);
            }
            self.source = if finished { Source::Early } else { Source::Fallback };
            if finished { self.early_blocks = self.early_done; }
        } else if missed_start {
            self.source = Source::Fallback;
        }
        if self.source != Source::Early { self.first_block_us = 0; }
        if !finished {
            let step = Instant::now();
            let push = |packet: &[u8]| { self.packet.extend_from_slice(packet); true };
            let valid = if records { self.records.push_frame(data, push) } else { push_container(data, push) };
            self.stages.get_mut().add(ASSEMBLE, step);
            if !valid { return Err("Malformed PyroWave frame container"); }
            let step = Instant::now();
            for plane in &mut self.planes { plane.fill(0.0); }
            self.seen.fill(false);
            self.stages.get_mut().add(CLEAR, step);
        }
        self.loss_percent = if records { self.records.loss_percent } else { 0.0 };
        let step = Instant::now();
        let mut at = 0;
        let mut header = false;
        while at < self.packet.len() {
            let first = word(&self.packet, at);
            let second = word(&self.packet, at + 4);
            if first & 0x8000_0000 != 0 {
                if (first as usize & 0x3fff) + 1 != self.width || ((first >> 14) as usize & 0x3fff) + 1 != self.height
                    || (second >> 24) & 3 != 0 || (second & (1 << 26) != 0) != self.chroma444 {
                    return Err("PyroWave sequence does not match the negotiated picture");
                }
                header = true;
                at += 8;
                continue;
            }
            let length = ((first >> 16) & 0xfff) as usize * 4;
            place_block(&self.bands, self.stride, &mut self.planes, &mut self.seen, (second >> 8) as usize, &self.packet[at..at + length])?;
            at += length;
        }
        if !header { return Err("PyroWave frame is missing its sequence header"); }
        self.stages.get_mut().add(ENTROPY, step);
        Ok(())
    }

    /// `(ms p50/p95 over N frames): assemble .., clear .., entropy .., pack .., alloc ..` for the
    /// frames converted so far. Entropy includes what early chunks decoded as they arrived.
    pub fn stage_summary(&self) -> Option<String> {
        self.stages.borrow().summary()
    }

    #[cfg(test)]
    fn reconstruct(&mut self) {
        for (component, plane) in self.planes.iter_mut().enumerate() {
            let last = usize::from(component != 0 && !self.chroma444);
            for level in (last..5).rev() {
                inverse(plane, self.stride, self.stride >> level, self.rows >> level, &mut self.scratch, &mut self.line);
            }
        }
    }

    pub fn coefficient_frame(&self) -> Result<crate::media::Frame, String> {
        let step = Instant::now();
        let frame = crate::media::Frame::wavelet(self.stride, self.rows)?;
        self.stages.borrow_mut().add(ALLOC, step);
        let step = Instant::now();
        let info = frame.info();
        #[cfg(target_arch = "x86_64")]
        let fast_half = std::is_x86_feature_detected!("f16c");
        unsafe {
            for y in 0..self.rows {
                let row = std::slice::from_raw_parts_mut(info.data[0].add(y * info.linesize[0] as usize).cast::<[u16; 4]>(), self.stride);
                #[cfg(target_arch = "x86_64")]
                if fast_half {
                    let start = y * self.stride;
                    pack_half_row(row, std::array::from_fn(|p| &self.planes[p][start..start + self.stride]));
                    continue;
                }
                for (x, pixel) in row.iter_mut().enumerate() {
                    let at = y * self.stride + x;
                    *pixel = [half(self.planes[0][at]), half(self.planes[1][at]), half(self.planes[2][at]), 0x3c00];
                }
            }
        }
        let mut stages = self.stages.borrow_mut();
        stages.add(PACK, step);
        stages.commit();
        Ok(frame)
    }

    #[cfg(test)]
    pub fn sample(&self, plane: usize, x: usize, y: usize) -> f32 {
        let scale = usize::from(plane != 0 && !self.chroma444);
        (self.planes[plane][(y >> scale) * self.stride + (x >> scale)] + 0.5).clamp(0.0, 1.0)
    }

}

/// The block index and the record's own bytes, cut to the length it declares.
fn record_index(record: &[u8]) -> Result<(usize, &[u8]), &'static str> {
    if record.len() < 8 { return Err("Truncated PyroWave record"); }
    let length = ((word(record, 0) >> 16) & 0xfff) as usize * 4;
    if length > record.len() { return Err("Truncated PyroWave record"); }
    Ok(((word(record, 4) >> 8) as usize, &record[..length]))
}

/// Decodes block `index` into the planes unless an earlier record already did; whether it did.
fn place_block(bands: &[Band], stride: usize, planes: &mut [Vec<f32>; 3], seen: &mut [bool], index: usize, record: &[u8]) -> Result<bool, &'static str> {
    if index >= seen.len() { return Err("PyroWave block index exceeds the picture"); }
    if seen[index] { return Ok(false); }
    seen[index] = true;
    let band = bands.iter().find(|band| index >= band.first && index < band.first + band.count).unwrap();
    let mut values = [0.0; 1024];
    decode_block(record, &mut values)?;
    let block = index - band.first;
    let (bx, by) = (block % band.width.div_ceil(32) * 32, block / band.width.div_ceil(32) * 32);
    for y in 0..(band.height - by).min(32) {
        let w = (band.width - bx).min(32);
        let offset = (band.y + by + y) * stride + band.x + bx;
        planes[band.component][offset..offset + w].copy_from_slice(&values[y * 32..y * 32 + w]);
    }
    Ok(true)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "f16c")]
unsafe fn pack_half_row(row: &mut [[u16; 4]], planes: [&[f32]; 3]) {
    use std::arch::x86_64::*;
    for (x, pixel) in row.iter_mut().enumerate() {
        let values = _mm_set_ps(1.0, planes[2][x], planes[1][x], planes[0][x]);
        _mm_storel_epi64(pixel.as_mut_ptr().cast(), _mm_cvtps_ph::<0>(values));
    }
}

fn decode_block(data: &[u8], output: &mut [f32; 1024]) -> Result<(), &'static str> {
    let ballot = word(data, 0) & 0xffff;
    let count = ballot.count_ones() as usize;
    let mut offset = 8 + count * 3;
    if offset > data.len() { return Err("Truncated PyroWave block metadata"); }
    let quant = data[4];
    let scale32 = (1.0 + f32::from(quant & 7) / 8.0) * 2.0f32.powi(4 - i32::from(quant >> 3));
    let mut magnitudes = [0u32; 1024];
    let mut scales = [0.0; 16];
    let mut compact = 0;
    for (block, scale) in scales.iter_mut().enumerate() {
        if ballot & (1 << block) == 0 { continue; }
        let code = u16::from_le_bytes([data[8 + compact * 2], data[9 + compact * 2]]);
        let q = data[8 + count * 2 + compact];
        *scale = scale32 * (f32::from(q >> 4) / 8.0 + 0.25);
        for sub in 0..8 {
            let bits = ((code >> (sub * 2)) & 3) as usize + usize::from(q & 15);
            if offset + bits > data.len() { return Err("Truncated PyroWave coefficient planes"); }
            // An all-zero control word is the encoder's empty 8x8 block marker.
            if code != 0 {
                for byte in &data[offset..offset + bits] {
                    for bit in 0..8 {
                        let value = &mut magnitudes[block * 64 + sub * 8 + bit];
                        *value = (*value << 1) | u32::from((byte >> bit) & 1);
                    }
                }
            }
            offset += bits;
        }
        compact += 1;
    }
    let significant = magnitudes.iter().filter(|m| **m != 0).count();
    if offset + significant.div_ceil(8) > data.len() { return Err("Truncated PyroWave coefficient signs"); }
    let mut sign = 0;
    for (index, magnitude) in magnitudes.into_iter().enumerate() {
        if magnitude == 0 { continue; }
        let block = index / 64;
        let local = index % 64;
        let x = (block % 4) * 8 + ((local >> 1) & 3) + ((local >> 5) << 2);
        let y = (block / 4) * 8 + (local & 1) + (((local >> 3) & 3) << 1);
        let negative = (data[offset + sign / 8] >> (sign % 8)) & 1 != 0;
        let value = (magnitude as f32 + 0.5) * scales[block];
        if value > 65504.0 { return Err("PyroWave coefficient exceeds FP16 storage"); }
        output[y * 32 + x] = if negative { -value } else { value };
        sign += 1;
    }
    Ok(())
}

fn half(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = (bits >> 16) as u16 & 0x8000;
    let exponent = ((bits >> 23) & 255) as i32 - 112;
    let mantissa = bits & 0x7f_ffff;
    if exponent <= 0 {
        if exponent < -10 { return sign; }
        let mantissa = mantissa | 0x80_0000;
        let shift = 14 - exponent;
        return sign | ((mantissa + (1 << (shift - 1)) - 1 + ((mantissa >> shift) & 1)) >> shift) as u16;
    }
    let rounded = mantissa + 0xfff + ((mantissa >> 13) & 1);
    sign | (((exponent as u32) << 10) + (rounded >> 13)) as u16
}

#[cfg(test)]
fn lift(line: &mut [f32]) {
    const K: f32 = 1.230_174_1;
    for (i, value) in line.iter_mut().enumerate() { *value *= if i % 2 == 0 { K } else { 1.0 / K }; }
    let end = line.len() - 1;
    for (parity, coefficient) in [(0, 0.443_506_87), (1, 0.882_911_1), (0, -0.052_980_117), (1, -1.586_134_3)] {
        for i in (parity..line.len()).step_by(2) {
            line[i] -= coefficient * (line[if i == 0 { 1 } else { i - 1 }] + line[if i == end { end - 1 } else { i + 1 }]);
        }
    }
}

#[cfg(test)]
fn inverse(plane: &mut [f32], stride: usize, width: usize, height: usize, scratch: &mut [f32], line: &mut [f32]) {
    for x in 0..width {
        for y in 0..height / 2 {
            line[y * 2] = plane[y * stride + x];
            line[y * 2 + 1] = plane[(y + height / 2) * stride + x];
        }
        lift(&mut line[..height]);
        for y in 0..height { scratch[y * width + x] = line[y]; }
    }
    for y in 0..height {
        for x in 0..width / 2 {
            line[x * 2] = scratch[y * width + x];
            line[x * 2 + 1] = scratch[y * width + x + width / 2];
        }
        lift(&mut line[..width]);
        plane[y * stride..y * stride + width].copy_from_slice(&line[..width]);
    }
}

/// One reconstructed frame, ready for the presenter.
pub struct Output {
    pub picture: Picture,
    pub loss_percent: f32,
    pub first_block_us: u64,
    /// The slices decoded for the frame, in order (empty unless the beam race is on).
    pub bands: Vec<BandReady>,
    pub race: RaceReport,
    /// The sequence number of the frame's slices (`SliceReady::seq`); 0 when it had none.
    pub race_seq: u64,
}

/// A slice of a frame still being received, whose decode has finished: rows `band.first_row..
/// band.end_row` can be drawn from the plane set `pin` holds once `band.fence_value` is signalled.
/// The frame's final decode follows in its `Output`.
pub struct SliceReady {
    pub seq: u64,
    pub band: BandReady,
    pub pin: super::gpu_planes::Pin,
}

/// A slice of the picture whose decode has finished: rows `first_row..end_row` of the frame being
/// received are exact in the plane set once `fence_value` is signalled on the decode fence. The
/// last slice is the final decode of the whole frame (its rows run to the end of the picture).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BandReady {
    pub slice: u8,
    pub first_row: u32,
    pub end_row: u32,
    pub fence_value: u64,
    /// When the decode was seen to finish (session clock, us).
    pub ready_us: u64,
}

/// How the beam race went for one frame (the timing CSV's `slices_decoded`, `first_slice_ready_ns`,
/// `last_slice_ready_ns` and `row_ordered`). All zero when the race is off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RaceReport {
    /// Slice decodes for the frame, the final one included.
    pub slices: u8,
    pub first_ready_us: u64,
    pub last_ready_us: u64,
    /// 0 not known, `ORDERED` or `UNORDERED`.
    pub ordered: u8,
}

impl RaceReport {
    pub const ORDERED: u8 = 1;
    pub const UNORDERED: u8 = 2;
}

/// Which slice of the picture is due while the records of a row-ordered frame arrive: the
/// shared row tracker and the slice ends. The last slice is the final decode, so it is never due
/// here.
#[cfg_attr(not(windows), allow(dead_code))]
pub struct SlicePlan {
    tracker: RowTracker,
    /// Slice `i` holds the bands before `ends[i]`.
    ends: Vec<u32>,
    /// The next slice not yet decoded.
    next: usize,
    height: u32,
}

/// Rows of a slice that is due.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Due {
    pub slice: usize,
    pub first_row: u32,
    pub end_row: u32,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl SlicePlan {
    pub fn new(width: u32, height: u32, chroma444: bool, slices: u32) -> Self {
        let tracker = RowTracker::new(width, height, chroma444);
        let ends = slice_ends(tracker.bands(), slices);
        Self { tracker, ends, next: 0, height }
    }

    pub fn begin_frame(&mut self) {
        self.tracker.begin_frame();
        self.next = 0;
    }

    /// A packet was fed; the records it completed follow with `record`.
    pub fn packet(&mut self) {
        self.tracker.packet();
    }

    pub fn record(&mut self, index: u32) {
        self.tracker.record(index);
    }

    /// Packets were lost before the one that was just fed: no slice is due for the rest of the frame.
    pub fn gap(&mut self) {
        self.tracker.gap();
    }

    /// The whole picture, for a frame that is decoded once.
    pub fn whole(&self) -> (u32, u32) {
        (0, self.height)
    }

    /// The frame's last packet was fed.
    pub fn finish(&mut self) {
        self.tracker.finish();
    }

    pub fn report(&self) -> RowReport {
        self.tracker.report()
    }

    /// The furthest slice that can be decoded exactly now, if it is one not decoded yet. A frame
    /// that has left the row order, or lost a packet, is never sliced from there on: it decodes whole.
    pub fn due(&mut self) -> Option<Due> {
        let report = self.tracker.report();
        if report.violations > 0 || report.gaps > 0 { return None; }
        let done = self.tracker.complete_bands();
        let bands = self.tracker.bands();
        let target = self.ends.iter().rposition(|&end| end <= done && end < bands)?;
        if target < self.next { return None; }
        let first = self.first_band();
        let (first_row, end_row) = band_rows(self.height, first, self.ends[target]);
        self.next = target + 1;
        Some(Due { slice: target, first_row, end_row })
    }

    /// Rows still to show when the frame completes: after the last slice decoded.
    pub fn rest(&self) -> (u32, u32) {
        band_rows(self.height, self.first_band(), self.tracker.bands())
    }

    fn first_band(&self) -> u32 {
        if self.next == 0 { 0 } else { self.ends[self.next - 1] }
    }
}

/// Why a frame produced no picture.
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    /// The frame did not decode (malformed, or the sequence does not match): counted, and the
    /// host is asked for a new IDR, as on the CPU path.
    Frame(String),
    /// The frame lost too much of its data on the way: counted and an IDR asked for as with
    /// `Frame`, but it says nothing about the decoder, so it never ends the stream.
    Loss(String),
    /// Every plane set is held by a picture that has not been shown or dropped yet: the frame is
    /// dropped and counted with the replaced ones.
    NoPlanes,
    /// The GPU decoder itself broke (a failed submit, a decode that never finished, a lost
    /// device): nothing falls back to the CPU, the stream ends with this message.
    Device(String),
}

/// Brings the records cache up to date with a frame that is not decoded, so the frames after it
/// recover what they lose from it and not from an older frame. `early`: every chunk since
/// `begin_frame` was fed and they are as long as `data`, which then only needs finishing if it
/// is the records envelope they belong to.
fn pass_records(records: &mut Records, early: bool, data: &[u8]) {
    let whole_early = early && data.starts_with(&[0, 0, 0, 0]);
    if !(whole_early && records.finish_frame(|_| true)) {
        // Begins the frame anew, and refuses what is not a records envelope.
        records.push_frame(data, |_| true);
    }
}

/// The PyroWave decoder of a stream, on the CPU or on the GPU (`Settings::pyrowave_decoder`).
pub enum Pyrowave {
    Cpu(Box<Decoder>),
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Gpu(Box<GpuStream>),
}

impl Pyrowave {
    pub fn cpu(width: usize, height: usize, chroma444: bool) -> Result<Self, &'static str> {
        Decoder::new(width, height, chroma444).map(|decoder| Self::Cpu(Box::new(decoder)))
    }

    /// Takes one packet of the records envelope of the frame `frame` will get next.
    pub fn chunk(&mut self, first: bool, flags: u32, payload: &[u8]) {
        match self {
            Self::Cpu(decoder) => decoder.chunk(first, flags, payload),
            Self::Gpu(stream) => stream.chunk(first, flags, payload),
        }
    }

    /// Decodes the picture in `slices` slices as the bands of a row-ordered frame complete (the
    /// beam race). Only the GPU decoder does; the CPU decoder says so, loudly, to its caller.
    pub fn set_beam_race(&mut self, slices: u32) -> Result<(), &'static str> {
        let _ = slices;
        match self {
            Self::Cpu(_) => Err("beam racing needs the GPU PyroWave decoder, so frames decode whole"),
            #[cfg(windows)]
            Self::Gpu(stream) => { stream.set_slices(slices); Ok(()) }
            #[cfg(not(windows))]
            Self::Gpu(_) => Err("beam racing is Windows-only so far, so frames decode whole"),
        }
    }

    /// The slices decoded while the frame in flight arrived, since the last call.
    pub fn take_slices(&mut self) -> Vec<SliceReady> {
        match self {
            Self::Cpu(_) => Vec::new(),
            Self::Gpu(stream) => stream.take_slices(),
        }
    }

    /// Where the picture of the last `frame` came from, also after a frame that failed (the
    /// early and fallback counters count those too).
    pub fn source(&self) -> Source {
        match self {
            Self::Cpu(decoder) => decoder.source,
            Self::Gpu(stream) => stream.source,
        }
    }

    /// Reconstructs the frame. `evict` drops the oldest picture still queued for the presenter
    /// and says whether there was one; the GPU decoder calls it while no plane set is free.
    pub fn frame(&mut self, data: &[u8], evict: &mut dyn FnMut() -> bool) -> Result<Output, Failure> {
        match self {
            Self::Cpu(decoder) => {
                decoder.coefficients(data).map_err(|e| Failure::Frame(e.into()))?;
                let frame = decoder.coefficient_frame().map_err(Failure::Frame)?;
                Ok(Output { picture: Picture::Frame(frame), loss_percent: decoder.loss_percent, first_block_us: decoder.first_block_us,
                    bands: Vec::new(), race: RaceReport::default(), race_seq: 0 })
            }
            Self::Gpu(stream) => stream.frame(data, evict),
        }
    }

    /// Whether a frame may be passed over for a newer one ([`Pyrowave::skip`]): every frame
    /// decodes on its own, but the slices of a beam-raced frame are with the presenter before
    /// the frame is complete.
    pub fn skips_frames(&self) -> bool {
        match self {
            Self::Cpu(_) => true,
            Self::Gpu(stream) => stream.race.is_none(),
        }
    }

    /// Passes over the frame `frame` would have decoded: no picture, and no decode time spent.
    /// The records cache takes the frame all the same, for the frames after it.
    pub fn skip(&mut self, data: &[u8]) {
        match self {
            Self::Cpu(decoder) => decoder.skip(data),
            Self::Gpu(stream) => stream.skip(data),
        }
    }

    pub fn stage_summary(&self) -> Option<String> {
        match self {
            Self::Cpu(decoder) => decoder.stage_summary(),
            Self::Gpu(stream) => stream.stage_summary(),
        }
    }

    /// The log lines a GPU decoder adds at the stream's end: the SDK's GPU time per frame and the
    /// GPU's clocks, each where available.
    pub fn gpu_report(&self) -> Vec<String> {
        match self {
            Self::Cpu(_) => Vec::new(),
            Self::Gpu(stream) => [
                stream.pass_summary().map(|passes| format!("PyroWave GPU passes {passes}")),
                stream.clock_summary().map(|clocks| format!("GPU during the stream: {clocks}")),
            ].into_iter().flatten().collect(),
        }
    }
}

/// Where a Linux decode call that began at `start` splits into its submit and its wait for the
/// GPU: when the SDK's submit returned, or `start` when that is unknown (the whole call is then
/// the wait, as before the split) or earlier than `start`.
#[cfg_attr(not(unix), allow(dead_code))]
fn submit_split(start: Instant, submitted: Option<Instant>) -> Instant {
    submitted.filter(|&at| at >= start).unwrap_or(start)
}

/// The most a frame may have lost (the SDK gate's 90% packet ratio) once its slices were decoded.
#[cfg_attr(not(windows), allow(dead_code))]
const SLICED_LOSS_LIMIT_PERCENT: f32 = 10.0;
/// Frames in a row that must arrive in row order before slicing starts again after one did not.
#[cfg_attr(not(windows), allow(dead_code))]
const ORDERED_FRAMES_TO_RESUME: u32 = 8;

/// Whether the final decode of a frame may go ahead. A slice decode marks the SDK's sequence as
/// decoded, and its ready flag is false for the rest of it, so a frame whose slices were decoded
/// is judged on the loss its records cache had to fill in (the gate's own 90% packet ratio)
/// when its chunks were the whole envelope (`finished`). Any other frame asks the SDK.
#[cfg_attr(not(windows), allow(dead_code))]
fn final_decode_gate(sliced: bool, finished: bool, loss_percent: f32, sdk_ready: impl FnOnce() -> bool) -> Result<(), &'static str> {
    if sliced && finished {
        if loss_percent > SLICED_LOSS_LIMIT_PERCENT {
            return Err("PyroWave frame lost too much of its data after its slices were decoded");
        }
    } else if !sdk_ready() {
        return Err("PyroWave frame holds no picture data");
    }
    Ok(())
}
/// How long the decoder thread waits for the GPU to finish a frame before the stream ends.
pub(crate) const GPU_DECODE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const GPU_STAGE_NAMES: [&str; 5] = ["clear", "push", "submit", "gpu", "slices"];
const GPU_CLEAR: usize = 0;
const GPU_PUSH: usize = 1;
const GPU_SUBMIT: usize = 2;
const GPU_WAIT: usize = 3;
/// The slice decodes made while the frame arrives (beam race), submit and wait together.
#[cfg_attr(not(windows), allow(dead_code))]
const GPU_SLICES: usize = 4;
/// The first word of a record that is the sequence header.
const HEADER_BIT: u32 = 0x8000_0000;

/// Gives one record to the SDK; false if it is a sequence header for another picture than the
/// one negotiated (the CPU path's check, `Decoder::coefficients`) or the SDK refuses it.
fn push_checked(gpu: &mut GpuDecoder, packet: &[u8]) -> bool {
    if word(packet, 0) & HEADER_BIT != 0 {
        let (first, second, config) = (word(packet, 0), word(packet, 4), gpu.config());
        if (first as usize & 0x3fff) + 1 != config.width as usize || ((first >> 14) as usize & 0x3fff) + 1 != config.height as usize
            || (second >> 24) & 3 != 0 || (second & (1 << 26) != 0) != config.chroma444 {
            return false;
        }
    }
    gpu.push_packet(packet).is_ok()
}

/// The block indices of the records a frame gave the SDK early, as a bitmap. A 4K 4:4:4 frame
/// has about 24,000 records, each inserted as it arrives and looked up again when the frame
/// completes; a `HashSet` hashed each of them (SipHash) on the decoder thread, inside the push.
/// Indices are 24 bits (`word >> 8`), so the map is at most 2 MiB; `clear` zeroes only the words
/// the frame touched.
#[derive(Default)]
struct BlockSet {
    words: Vec<u64>,
    /// One past the highest word set since `clear`.
    used: usize,
    len: usize,
}

impl BlockSet {
    fn clear(&mut self) {
        self.words[..self.used].fill(0);
        self.used = 0;
        self.len = 0;
    }

    /// Adds `index`; false if it was there already.
    fn insert(&mut self, index: u32) -> bool {
        let (word, bit) = (index as usize / 64, 1u64 << (index % 64));
        if word >= self.words.len() { self.words.resize(word + 1, 0); }
        self.used = self.used.max(word + 1);
        let added = self.words[word] & bit == 0;
        self.words[word] |= bit;
        self.len += usize::from(added);
        added
    }

    fn contains(&self, index: u32) -> bool {
        self.words.get(index as usize / 64).is_some_and(|word| word & (1u64 << (index % 64)) != 0)
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// PyroWave frames decoded by the SDK on the GPU into shared D3D11 planes. The records go to
/// the SDK as they complete (early decode) or when the frame does; the SDK runs the entropy
/// decoding and the inverse wavelets, and the presenter converts the planes to RGB.
pub struct GpuStream {
    gpu: GpuDecoder,
    planes: std::sync::Arc<super::gpu_planes::Planes>,
    records: Records,
    early: Early,
    missed_start: bool,
    early_bytes: usize,
    first_block_us: u64,
    /// The records already given to the SDK for the frame in flight, by block index.
    pushed: BlockSet,
    /// Where the last frame's records came from (`Pyrowave::source`).
    source: Source,
    stages: Stages<5>,
    /// The beam race (`set_slices`); None decodes each frame once, as before.
    race: Option<Race>,
    /// The AMD GPU's clocks while the stream runs (None without amdgpu).
    #[cfg(target_os = "linux")]
    clocks: Option<super::gpu_clocks::Sampler>,
}

/// The beam race state of the frame in flight.
#[cfg_attr(not(windows), allow(dead_code))]
struct Race {
    plan: SlicePlan,
    /// The sequence header went to the SDK ahead of the records, so a prefix decodes.
    header_pushed: bool,
    /// The plane set the slices decode into, which the final decode then fills (with its `after`).
    lease: Option<(super::gpu_planes::Lease, u64)>,
    bands: Vec<BandReady>,
    /// A slice decode failed: the frame ends with this (a loud presenter error), nothing falls back.
    failure: Option<String>,
    /// No plane set was free for the slices: the frame decodes once, whole.
    no_plane: bool,
    /// Frames in a row that arrived in row order, up to `ORDERED_FRAMES_TO_RESUME`.
    ordered_run: u32,
    /// This frame may be sliced (the last frames were in order).
    slicing: bool,
    /// The frame in flight (`race::next_seq`: it grows across decoders; slices and the frame's `Output` carry it).
    seq: u64,
    /// Slices decoded and not yet taken by the decoder thread.
    events: Vec<SliceReady>,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Race {
    fn begin_frame(&mut self) {
        self.plan.begin_frame();
        self.seq = super::race::next_seq();
        self.events.clear();
        self.header_pushed = false;
        self.lease = None;
        self.bands.clear();
        self.failure = None;
        self.no_plane = false;
        self.slicing = self.ordered_run >= ORDERED_FRAMES_TO_RESUME;
    }

    /// The final decode is the last slice; what the frame's slices were. A frame that was not
    /// received as chunks, or not in row order, has no slices: slices decoded before its order
    /// broke are not exact, so the one entry is the final decode of the whole picture. The same
    /// goes for the one late record `ordered` tolerates, when a slice was decoded before it came:
    /// that slice lacks the record, and the frame counts as unordered, ending the run of ordered
    /// frames. A late record before the first slice costs nothing and still counts as ordered.
    fn finish(&mut self, fence_value: u64, ready_us: u64, chunks: bool) -> (RaceReport, Vec<BandReady>) {
        let report = self.plan.report();
        let ordered = chunks && report.ordered();
        let sliced = ordered && report.violations == 0;
        let fell_back = ordered && !sliced && !self.bands.is_empty();
        if chunks {
            self.ordered_run = if ordered && !fell_back { (self.ordered_run + 1).min(ORDERED_FRAMES_TO_RESUME) } else { 0 };
        }
        let (first_row, end_row) = if sliced { self.plan.rest() } else { self.plan.whole() };
        if !sliced { self.bands.clear(); }
        self.bands.push(BandReady { slice: self.bands.len() as u8, first_row, end_row, fence_value, ready_us });
        let bands = std::mem::take(&mut self.bands);
        if !chunks { return (RaceReport::default(), bands); }
        let flag = if ordered && !fell_back { RaceReport::ORDERED } else { RaceReport::UNORDERED };
        (RaceReport { slices: bands.len() as u8, first_ready_us: bands[0].ready_us, last_ready_us: ready_us, ordered: flag }, bands)
    }
}

impl GpuStream {
    /// Creates the SDK decoder on `device` (the renderer's) with `slots` plane sets.
    #[cfg(windows)]
    pub fn new(device: &windows::Win32::Graphics::Direct3D11::ID3D11Device, width: usize, height: usize, chroma444: bool, ten_bit: bool, slots: usize) -> Result<Self, String> {
        let config = GpuConfig { width: width as u32, height: height as u32, chroma444, ten_bit, slots };
        GpuDecoder::new(device, config).map(Self::with_decoder)
    }

    /// Creates the SDK decoder with `slots` plane sets, on the Vulkan device of the renderer whose
    /// SDL GPU device is `device` where it can be shared, else on the driver's default device
    /// (`GpuDecoder::for_renderer`).
    #[cfg(target_os = "linux")]
    pub fn new(device: *mut std::ffi::c_void, width: usize, height: usize, chroma444: bool, ten_bit: bool, slots: usize) -> Result<Self, String> {
        let config = GpuConfig { width: width as u32, height: height as u32, chroma444, ten_bit, slots };
        GpuDecoder::for_renderer(device, config).map(Self::with_decoder)
    }

    #[cfg(not(target_os = "macos"))]
    pub fn with_decoder(gpu: GpuDecoder) -> Self {
        let config = gpu.config();
        Self {
            planes: super::gpu_planes::Planes::new(&gpu),
            records: Records::with_geometry(config.width, config.height, config.chroma444),
            gpu,
            early: Early::Off,
            missed_start: false,
            early_bytes: 0,
            first_block_us: 0,
            pushed: BlockSet::default(),
            source: Source::Frame,
            stages: Stages::new(GPU_STAGE_NAMES),
            race: None,
            #[cfg(target_os = "linux")]
            clocks: super::gpu_clocks::Sampler::start(),
        }
    }

    /// The SDK's own GPU time per frame over the stream (`pyrowave_sdk::pass_summary`), for the
    /// log beside the stages, whose "gpu" stage also holds the time the decode waited for the queue.
    pub fn pass_summary(&self) -> Option<String> {
        // A stalled decoder is never called again.
        #[cfg(unix)]
        if self.gpu.stalled() { return None; }
        super::pyrowave_sdk::pass_summary(&self.gpu.performance_stats(false))
    }

    /// What the AMD GPU's clocks did during the stream (`gpu_clocks`), where they can be read:
    /// on Linux.
    pub fn clock_summary(&self) -> Option<String> {
        #[cfg(target_os = "linux")]
        {
            self.clocks.as_ref().and_then(super::gpu_clocks::Sampler::summary)
        }
        #[cfg(not(target_os = "linux"))]
        None
    }

    /// Decodes the picture in `slices` slices while a row-ordered frame arrives (0 turns it off).
    /// Frames that do not arrive in row order are decoded once, whole, and say so in their report.
    #[cfg(windows)]
    pub fn set_slices(&mut self, slices: u32) {
        let config = self.gpu.config();
        self.race = (slices > 0).then(|| Race {
            plan: SlicePlan::new(config.width, config.height, config.chroma444, slices),
            header_pushed: false,
            lease: None,
            bands: Vec::new(),
            failure: None,
            no_plane: false,
            ordered_run: ORDERED_FRAMES_TO_RESUME,
            slicing: true,
            seq: 0,
            events: Vec::new(),
        });
    }

    #[cfg(all(test, not(target_os = "macos")))]
    pub fn planes(&self) -> &std::sync::Arc<super::gpu_planes::Planes> {
        &self.planes
    }

    pub fn chunk(&mut self, first: bool, flags: u32, payload: &[u8]) {
        if first {
            let step = Instant::now();
            self.stages.reset();
            self.gpu.clear();
            self.records.begin_frame();
            self.pushed.clear();
            self.early = Early::Active;
            self.early_bytes = 4;
            self.first_block_us = 0;
            self.missed_start = false;
            if let Some(race) = &mut self.race { race.begin_frame(); }
            self.stages.add(GPU_CLEAR, step);
        } else if self.early == Early::Off {
            self.missed_start = true;
            return;
        }
        if self.early != Early::Active { return; }
        let step = Instant::now();
        self.early_bytes += 8 + payload.len();
        if !self.records.feed_packet(flags, payload) { self.early = Early::Failed; return; }
        let Self { records, gpu, pushed, race, .. } = self;
        let mut failed = false;
        if let Some(race) = race.as_mut() {
            // A packet that is not the first and does not continue the one before it follows a loss.
            if !first && flags & rubylight_protocol::pyrowave::PACKET_CONTIGUOUS == 0 { race.plan.gap(); }
            race.plan.packet();
            if flags & rubylight_protocol::pyrowave::PACKET_FRAME_START != 0 && payload.len() >= 16 {
                // The sequence header first, so a slice decode knows the picture. `finish_frame`
                // pushes it again with the final record count, which the SDK takes for the same sequence.
                if push_checked(gpu, &payload[8..16]) { race.header_pushed = true; } else { failed = true; }
            }
            if flags & rubylight_protocol::pyrowave::PACKET_FRAME_END != 0 { race.plan.finish(); }
        }
        records.drain_new(|index, record| {
            if failed { return; }
            if !push_checked(gpu, record) { failed = true; return; }
            pushed.insert(index);
            if let Some(race) = race.as_mut() { race.plan.record(index); }
        });
        if failed { self.early = Early::Failed; }
        if !self.pushed.is_empty() && self.first_block_us == 0 { self.first_block_us = nightjar_core::session::now_us(); }
        self.stages.add(GPU_PUSH, step);
        self.decode_due_slice();
    }

    /// Decodes the slice that the records so far complete, if there is one: the prefix of the
    /// frame, missing blocks as zero (the SDK needs no `is_ready` for that, which a prefix never
    /// satisfies), into the plane set the final decode will use. Waits for it, because the next
    /// decode needs the SDK's payload buffer free.
    #[cfg(windows)]
    fn decode_due_slice(&mut self) {
        if self.early != Early::Active { return; }
        let Self { race: Some(race), gpu, planes, stages, .. } = self else { return };
        if !race.slicing || !race.header_pushed || race.failure.is_some() || race.no_plane { return; }
        let Some(due) = race.plan.due() else { return };
        let step = Instant::now();
        if race.lease.is_none() {
            match planes.acquire() {
                Some(lease) => race.lease = Some(lease),
                None => { race.no_plane = true; return; }
            }
        }
        let Some((lease, after)) = &race.lease else { return };
        // The slices already drawn from the set (by the presenter) are waited for as well. A slice
        // that is ready but not drawn yet is not: this decode rewrites its rows while the draw may read
        // them. That is safe for two reasons. The rewrite is the same bytes: a slice's rows depend only
        // on the records of the steps it was cut at, all of which this prefix has, so every decode of
        // the frame from that slice on gives those rows the final decode's values (the ignored GPU
        // test `a_slice_decode_is_exact_on_its_rows_on_real_row_ordered_frames` holds each slice
        // to them; this relies on the SDK's output planes being written by its last inverse wavelet
        // pass, which that test checks only through the values). And the set is acquired from
        // `QUEUE_FAMILY_EXTERNAL` once it holds a slice (`decode_again`): the SDK then records GENERAL
        // to GENERAL, where acquiring from IGNORED records UNDEFINED to GENERAL and lets the
        // driver discard the held rows. The rows below the slice, which this decode does change, are
        // outside the draw's scissor.
        let wait = (*after).max(planes.wait_for(lease.set()));
        let decoded = if race.bands.is_empty() { gpu.decode(lease.set(), wait) } else { gpu.decode_again(lease.set(), wait) };
        match decoded.and_then(|fence| gpu.wait(fence, GPU_DECODE_TIMEOUT).map(|()| fence)) {
            Ok(fence_value) => {
                let band = BandReady {
                    slice: due.slice as u8, first_row: due.first_row, end_row: due.end_row, fence_value, ready_us: nightjar_core::session::now_us(),
                };
                race.bands.push(band);
                race.events.push(SliceReady { seq: race.seq, band, pin: lease.pin() });
            }
            Err(error) => race.failure = Some(format!("PyroWave GPU decode failed: {error}")),
        }
        stages.add(GPU_SLICES, step);
    }

    /// The slices decoded since the last call, for the presenter.
    pub fn take_slices(&mut self) -> Vec<SliceReady> {
        self.race.as_mut().map(|race| std::mem::take(&mut race.events)).unwrap_or_default()
    }

    #[cfg(not(windows))]
    fn decode_due_slice(&mut self) {}

    /// Passes over a frame that a newer one replaces: nothing is decoded and no plane set is
    /// taken, and the records cache takes the frame as `frame` would have. The records its
    /// chunks pushed to the SDK go with the next frame, which clears them either way.
    pub fn skip(&mut self, data: &[u8]) {
        let early = std::mem::replace(&mut self.early, Early::Off);
        self.missed_start = false;
        self.source = Source::Frame;
        self.first_block_us = 0;
        pass_records(&mut self.records, early == Early::Active && self.early_bytes == data.len(), data);
    }

    pub fn frame(&mut self, data: &[u8], evict: &mut dyn FnMut() -> bool) -> Result<Output, Failure> {
        let early = std::mem::replace(&mut self.early, Early::Off);
        let missed_start = std::mem::take(&mut self.missed_start);
        if let Some(race) = &mut self.race {
            if let Some(failure) = race.failure.take() {
                race.lease = None;
                return Err(Failure::Device(failure));
            }
        }
        if early == Early::Off { self.stages.reset(); }
        let records_envelope = data.starts_with(&[0, 0, 0, 0]);
        let sliced = self.race.as_ref().is_some_and(|race| race.lease.is_some());
        let whole_early = early == Early::Active && records_envelope && self.early_bytes == data.len();
        // The slices decoded into a plane set already; the final decode fills that one.
        let mut slot = self.race.as_mut().and_then(|race| race.lease.take()).or_else(|| self.planes.acquire());
        while slot.is_none() && evict() { slot = self.planes.acquire(); }
        let Some((lease, after)) = slot else {
            // Keep the records cache current, so the frames after this one recover from it.
            if whole_early { self.records.finish_frame(|_| true); } else if records_envelope { self.records.push_frame(data, |_| true); }
            return Err(Failure::NoPlanes);
        };
        if data.len() > rubylight_protocol::pyrowave::MAX_FRAME_BYTES {
            return Err(Failure::Frame("PyroWave frame exceeds the packet budget".into()));
        }
        let mut finished = false;
        let mut source = Source::Frame;
        if early != Early::Off {
            // The chunks are the frame only if they add up to its envelope; the frame decides.
            if whole_early {
                let step = Instant::now();
                let Self { records, gpu, pushed, .. } = self;
                finished = records.finish_frame(|packet| {
                    if word(packet, 0) & HEADER_BIT != 0 || !pushed.contains(word(packet, 4) >> 8) { push_checked(gpu, packet) } else { true }
                });
                self.stages.add(GPU_PUSH, step);
            }
            source = if finished { Source::Early } else { Source::Fallback };
        } else if missed_start {
            source = Source::Fallback;
        }
        self.source = source;
        if !finished {
            let step = Instant::now();
            self.gpu.clear();
            let Self { records, gpu, .. } = self;
            let push = |packet: &[u8]| push_checked(gpu, packet);
            let valid = if records_envelope { records.push_frame(data, push) } else { push_container(data, push) };
            self.stages.add(GPU_PUSH, step);
            if !valid { return Err(Failure::Frame("Malformed PyroWave frame container, or a sequence that does not match the negotiated picture".into())); }
        }
        let loss_percent = if records_envelope { self.records.loss_percent } else { 0.0 };
        // A slice decode marks the SDK's sequence as decoded, and `is_ready` is then false for the
        // rest of it. The slices' frame is complete if its chunks were the envelope (`finished`);
        // what the cache had to fill in is the loss the gate would have judged.
        // A frame the cache had to fill in failed for what the network lost, not for the decoder.
        final_decode_gate(sliced, finished, loss_percent, || self.gpu.is_ready(true))
            .map_err(|why| if loss_percent > 0.0 { Failure::Loss(why.into()) } else { Failure::Frame(why.into()) })?;
        let step = Instant::now();
        #[cfg(windows)]
        let fence_value = {
            // Draws of the frame's slices from this set come first (none without the race).
            let after = if self.race.is_some() { after.max(self.planes.wait_for(lease.set())) } else { after };
            // The set of a sliced frame holds its slices (see `decode_due_slice`); a set that holds nothing is overwritten.
            let decoded = if sliced { self.gpu.decode_again(lease.set(), after) } else { self.gpu.decode(lease.set(), after) };
            let fence_value = decoded.map_err(|e| Failure::Device(format!("PyroWave GPU decode failed: {e}")))?;
            self.stages.add(GPU_SUBMIT, step);
            let step = Instant::now();
            self.gpu.wait(fence_value, GPU_DECODE_TIMEOUT).map_err(|e| Failure::Device(format!("PyroWave GPU decode failed: {e}")))?;
            self.stages.add(GPU_WAIT, step);
            fence_value
        };
        // The decode blocks until the GPU is done and the planes are final (in the set's images on
        // the renderer's device, or read back into its CPU buffers). Up to the submit's return
        // (on the wait thread) it is the "submit" stage, as on Windows, which holds the wait for
        // the renderer's queue lock; the rest is the "gpu" stage.
        #[cfg(unix)]
        {
            let decoded = if self.gpu.images().is_some() {
                self.gpu.decode_into(lease.set(), after)
            } else {
                let gpu = &mut self.gpu;
                lease.with_buffers(|[y, cb, cr]| gpu.decode([y, cb, cr])).unwrap_or_else(|| Err("the plane set has no CPU buffers".into()))
            };
            if let Err(e) = decoded {
                // A decode that missed its deadline may still write into this set once the
                // device comes back: the set is never given back, so nothing reads or reuses it.
                if self.gpu.stalled() { std::mem::forget(lease); }
                return Err(Failure::Device(format!("PyroWave GPU decode failed: {e}")));
            }
            let submitted = submit_split(step, self.gpu.submitted_at());
            self.stages.add_between(GPU_SUBMIT, step, submitted);
            self.stages.add(GPU_WAIT, submitted);
        }
        self.stages.commit();
        let first_block_us = if source == Source::Early { self.first_block_us } else { 0 };
        #[cfg(windows)]
        let (race, bands) = match &mut self.race {
            Some(race) => race.finish(fence_value, nightjar_core::session::now_us(), source == Source::Early),
            None => (RaceReport::default(), Vec::new()),
        };
        #[cfg(not(windows))]
        let (race, bands) = (RaceReport::default(), Vec::new());
        let race_seq = if race.slices > 0 { self.race.as_ref().map_or(0, |race| race.seq) } else { 0 };
        Ok(Output {
            picture: Picture::GpuPlanes {
                lease,
                #[cfg(windows)]
                fence_value,
            },
            loss_percent,
            first_block_us,
            bands,
            race,
            race_seq,
        })
    }

    /// The stage times; the slices stage only when the beam race is on.
    pub fn stage_summary(&self) -> Option<String> {
        self.stages.summary_of(if self.race.is_some() { GPU_STAGE_NAMES.len() } else { GPU_SLICES })
    }
}

#[cfg(all(test, windows))]
mod gpu_tests;

#[cfg(all(test, target_os = "linux"))]
mod gpu_tests_unix;

#[cfg(test)]
mod tests;
