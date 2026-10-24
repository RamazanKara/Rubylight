//! Rust decoder for PyroWave's 186f0393 bitstream.
//! Coefficient ordering and CDF 9/7 lifting follow Themaister/pyrowave a0d2e71 (MIT,
//! Copyright (c) 2026 Hans-Kristian Arntzen; app/src/main/assets/pyrowave_notices.txt).

use std::cell::RefCell;
use std::time::Instant;

use rubylight_protocol::pyrowave::rows::{band_rows, slice_ends, RowReport, RowTracker};
use rubylight_protocol::pyrowave::{push_container, Records};

use crate::media::Picture;
use super::stage_times::Stages;
use super::pyrowave_sdk::{GpuConfig, GpuDecoder};

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
            || width * height > 16_777_216 || (!chroma444 && (width % 2 != 0 || height % 2 != 0)) {
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

pub fn minimum_bitrate(width: u32, height: u32, fps: u32, chroma444: bool, hdr: bool) -> u32 {
    let coefficients = if chroma444 {
        [330.65932695689867, -12.487508056245487, 13.060737308248315, 139.64851902220505, -97.97414051841574, -112.81552415088743, 57.365194646059074, 57.19110311397152]
    } else {
        [309.25055550415766, 8.351816219985253, -9.905584693249878, 172.18790660175173, -64.17408945040484, -201.4015992260083, 46.22089684565188, 105.90424439986334]
    };
    let x = (f64::from(width) * f64::from(height)).clamp(921_600.0, 8_294_400.0).sqrt() * 0.001 - 2.0;
    let estimate = coefficients.into_iter().rev().fold(0.0, |acc, c| acc * x + c);
    (estimate * 0.008 * f64::from(fps.max(1)) * if hdr { 1.2 } else { 1.0 }).ceil() as u32 * 1000
}

/// One reconstructed frame, ready for the presenter.
pub struct Output {
    pub picture: Picture,
    pub loss_percent: f32,
    pub first_block_us: u64,
    /// The slices decoded for the frame, in order (empty unless the beam race is on).
    pub bands: Vec<BandReady>,
    pub race: RaceReport,
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
    /// Every plane set is held by a picture that has not been shown or dropped yet: the frame is
    /// dropped and counted with the replaced ones.
    NoPlanes,
    /// The GPU decoder itself broke (a failed submit, a decode that never finished, a lost
    /// device): nothing falls back to the CPU, the stream ends with this message.
    Device(String),
}

/// The PyroWave decoder of a stream, on the CPU or on the GPU (`Settings::pyrowave_decoder`).
pub enum Pyrowave {
    Cpu(Box<Decoder>),
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
                    bands: Vec::new(), race: RaceReport::default() })
            }
            Self::Gpu(stream) => stream.frame(data, evict),
        }
    }

    pub fn stage_summary(&self) -> Option<String> {
        match self {
            Self::Cpu(decoder) => decoder.stage_summary(),
            Self::Gpu(stream) => stream.stage_summary(),
        }
    }
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
#[cfg(windows)]
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
    pushed: std::collections::HashSet<u32>,
    /// Where the last frame's records came from (`Pyrowave::source`).
    source: Source,
    stages: Stages<5>,
    /// The beam race (`set_slices`); None decodes each frame once, as before.
    race: Option<Race>,
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
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Race {
    fn begin_frame(&mut self) {
        self.plan.begin_frame();
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

    /// Creates the SDK decoder (on the Vulkan driver's default device) with `slots` plane sets.
    #[cfg(unix)]
    pub fn new(width: usize, height: usize, chroma444: bool, ten_bit: bool, slots: usize) -> Result<Self, String> {
        let config = GpuConfig { width: width as u32, height: height as u32, chroma444, ten_bit, slots };
        GpuDecoder::new(config).map(Self::with_decoder)
    }

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
            pushed: std::collections::HashSet::new(),
            source: Source::Frame,
            stages: Stages::new(GPU_STAGE_NAMES),
            race: None,
        }
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
        });
    }

    #[cfg(all(test, windows))]
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
        match gpu.decode(lease.set(), *after).and_then(|fence| gpu.wait(fence, GPU_DECODE_TIMEOUT).map(|()| fence)) {
            Ok(fence_value) => race.bands.push(BandReady {
                slice: due.slice as u8, first_row: due.first_row, end_row: due.end_row, fence_value, ready_us: nightjar_core::session::now_us(),
            }),
            Err(error) => race.failure = Some(format!("PyroWave GPU decode failed: {error}")),
        }
        stages.add(GPU_SLICES, step);
    }

    #[cfg(not(windows))]
    fn decode_due_slice(&mut self) {}

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
                    if word(packet, 0) & HEADER_BIT != 0 || !pushed.contains(&(word(packet, 4) >> 8)) { push_checked(gpu, packet) } else { true }
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
        final_decode_gate(sliced, finished, loss_percent, || self.gpu.is_ready(true)).map_err(|why| Failure::Frame(why.into()))?;
        let step = Instant::now();
        #[cfg(windows)]
        let fence_value = {
            let fence_value = self.gpu.decode(lease.set(), after).map_err(|e| Failure::Device(format!("PyroWave GPU decode failed: {e}")))?;
            self.stages.add(GPU_SUBMIT, step);
            let step = Instant::now();
            self.gpu.wait(fence_value, GPU_DECODE_TIMEOUT).map_err(|e| Failure::Device(format!("PyroWave GPU decode failed: {e}")))?;
            self.stages.add(GPU_WAIT, step);
            fence_value
        };
        // The decode into the set's CPU buffers blocks until the GPU is done and the planes are read
        // back, so the whole of it is the "gpu" stage.
        #[cfg(unix)]
        {
            let _ = after;
            let gpu = &mut self.gpu;
            let decoded = lease.with_buffers(|[y, cb, cr]| gpu.decode([y, cb, cr]));
            if let Err(e) = decoded {
                // A decode that missed its deadline may still write into these buffers once the
                // device comes back: the set is never given back, so nothing reads or reuses it.
                if self.gpu.stalled() { std::mem::forget(lease); }
                return Err(Failure::Device(format!("PyroWave GPU decode failed: {e}")));
            }
            self.stages.add(GPU_WAIT, step);
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
        })
    }

    /// The stage times; the slices stage only when the beam race is on.
    pub fn stage_summary(&self) -> Option<String> {
        self.stages.summary_of(if self.race.is_some() { GPU_STAGE_NAMES.len() } else { GPU_SLICES })
    }
}

#[cfg(all(test, windows))]
mod gpu_tests;

#[cfg(all(test, unix))]
mod gpu_tests_unix;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_storage_rounds_ties_to_even() {
        for (value, bits) in [(0.0, 0), (-0.0, 0x8000), (1.0, 0x3c00), (-2.0, 0xc000), (65504.0, 0x7bff),
            (2.0f32.powi(-24), 1), (2.0f32.powi(-14), 0x0400), (1.0 + 2.0f32.powi(-11), 0x3c00)] {
            assert_eq!(half(value), bits);
        }
        #[cfg(target_arch = "x86_64")]
        if std::is_x86_feature_detected!("f16c") {
            let values: Vec<_> = (-32768..32768).map(|i| i as f32 / 32768.0).collect();
            let mut row = vec![[0; 4]; values.len()];
            unsafe { pack_half_row(&mut row, [&values; 3]); }
            for (pixel, value) in row.iter().zip(values) { assert_eq!(*pixel, [half(value), half(value), half(value), 0x3c00]); }
        }
    }

    #[test]
    fn bitrate_floors_match_androids_reference_values() {
        for (width, height, expected) in [(1280, 720, 89_000), (1920, 1080, 133_000), (2560, 1440, 149_000), (3840, 2160, 167_000)] {
            assert_eq!(minimum_bitrate(width, height, 60, false, false), expected);
        }
        assert_eq!(minimum_bitrate(1920, 1080, 60, true, false), 151_000);
        assert_eq!(minimum_bitrate(1920, 1080, 120, false, false), 265_000);
        assert_eq!(minimum_bitrate(1920, 1080, 60, false, true), 159_000);
    }

    #[test]
    fn reference_codec_matches_all_chroma_and_bit_depths() {
        // Generated by a0d2e71's encoder and Vulkan decoder (FP32 storage) from
        // ramps with sharp edges; odd 4:4:4 dimensions also exercise band padding.
        let fixtures: [(&[u8], &[u8], bool, bool); 4] = [
            (include_bytes!("../../tests/fixtures/pyrowave-420-8.bin"), include_bytes!("../../tests/fixtures/pyrowave-420-8.yuv"), false, false),
            (include_bytes!("../../tests/fixtures/pyrowave-444-8.bin"), include_bytes!("../../tests/fixtures/pyrowave-444-8.yuv"), true, false),
            (include_bytes!("../../tests/fixtures/pyrowave-420-10.bin"), include_bytes!("../../tests/fixtures/pyrowave-420-10.yuv"), false, true),
            (include_bytes!("../../tests/fixtures/pyrowave-444-10.bin"), include_bytes!("../../tests/fixtures/pyrowave-444-10.yuv"), true, true),
        ];
        for (data, golden, full, ten) in fixtures {
            let (width, height) = if full { (131, 97) } else { (132, 98) };
            let mut decoder = Decoder::new(width, height, full).unwrap();
            decoder.decode(data).unwrap();
            let mut offset = 0;
            let max = if ten { 1023.0 } else { 255.0 };
            let mut error = 0.0f32;
            for p in 0..3 {
                let shift = usize::from(p != 0 && !full);
                for y in 0..height >> shift { for x in 0..width >> shift {
                    let expected = if ten {
                        let v = u16::from_le_bytes([golden[offset], golden[offset + 1]]); offset += 2; v
                    } else { let v = u16::from(golden[offset]); offset += 1; v };
                    let actual = decoder.sample(p, x << shift, y << shift) * max;
                    error = error.max((actual - f32::from(expected)).abs());
                } }
            }
            assert!(error < 1.1, "444={full}, 10-bit={ten}, maximum error={error} code values");
            assert_eq!(offset, golden.len());
        }
    }

    #[test]
    fn empty_frame_is_neutral_and_changed_geometry_is_rejected() {
        let mut decoder = Decoder::new(128, 128, true).unwrap();
        let mut frame = vec![1, 0, 0, 0, 8, 0, 0, 0];
        frame.extend_from_slice(&(0x8000_0000u32 | 127 | (127 << 14)).to_le_bytes());
        frame.extend_from_slice(&(1u32 << 26).to_le_bytes());
        decoder.decode(&frame).unwrap();
        for plane in 0..3 { assert_eq!(decoder.sample(plane, 127, 127), 0.5); }
        frame[8] = 126;
        assert!(decoder.decode(&frame).is_err());
        assert!(Decoder::new(16384, 16384, false).is_err());
    }

    #[test]
    fn coefficient_signs_swizzle_and_truncation() {
        let mut block = vec![1, 0, 6, 0, 128, 0, 0, 0, 0x55, 0x55, 0x60];
        block.extend_from_slice(&[1; 8]);
        block.push(0b10101010);
        block.resize(24, 0);
        let mut out = [0.0; 1024];
        decode_block(&block, &mut out).unwrap();
        assert!(out[0] > 0.0 && out[64] < 0.0 && out[128] > 0.0 && out[192] < 0.0);
        assert_eq!(out.iter().filter(|v| **v != 0.0).count(), 8);
        assert!(decode_block(&block[..19], &mut out).is_err());
    }

    /// The wavelet level of every band of the decoder, in `bands` order (the loops of `new`).
    fn band_levels(decoder: &Decoder) -> Vec<usize> {
        let mut levels = Vec::new();
        for level in (0..5).rev() {
            for component in 0..3 {
                if level == 0 && component != 0 && !decoder.chroma444 { continue; }
                for _ in 0..if level == 4 { 4 } else { 3 } { levels.push(level); }
            }
        }
        assert_eq!(levels.len(), decoder.bands.len());
        levels
    }

    /// The negative control's step: `row_steps` without its one band of margin. Level 4 is sent
    /// first of all.
    fn unmargined_step(level: usize, row: usize, bands: usize) -> usize {
        if level == 4 { 0 } else { (row << level).min(bands - 1) }
    }

    /// For each picture band k: decode only the blocks the row order sends before band k's cut
    /// (every block whose step is at most k, the planes as they stand being the full frame's
    /// blocks), and the largest difference to the full decode over band k's rows, in the decoder's
    /// float units (a code value is 1/255 at 8 bits). A copy of the planes is zeroed where blocks
    /// are missing, as the decoder leaves a block that did not arrive.
    fn prefix_errors(decoder: &mut Decoder, lookahead: usize) -> Vec<f32> {
        let (stride, rows, full) = (decoder.stride, decoder.rows, decoder.chroma444);
        let bands = rows.div_ceil(64);
        let whole = decoder.planes.clone();
        let mut reference = whole.clone();
        let reconstruct = |planes: &mut [Vec<f32>; 3], decoder: &mut Decoder| {
            std::mem::swap(&mut decoder.planes, planes);
            decoder.reconstruct();
            std::mem::swap(&mut decoder.planes, planes);
        };
        reconstruct(&mut reference, decoder);
        let levels = band_levels(decoder);
        // The steps the host and the client share, in the decoder's block order.
        let steps = rubylight_protocol::pyrowave::rows::row_steps(stride as u32, rows as u32, full);
        assert_eq!(steps.len(), decoder.bands.iter().map(|band| band.count).sum::<usize>());
        let mut errors = Vec::new();
        for k in 0..bands {
            let mut planes = whole.clone();
            let mut index = 0;
            for (band, &level) in decoder.bands.iter().zip(&levels) {
                let across = band.width.div_ceil(32);
                for block in 0..band.count {
                    let row = block / across;
                    let step = if lookahead == 1 { usize::from(steps[index]) } else { unmargined_step(level, row, bands) };
                    index += 1;
                    if step <= k { continue; }
                    let (bx, by) = (block % across * 32, row * 32);
                    for y in by..(by + 32).min(band.height) {
                        let at = (band.y + y) * stride + band.x + bx;
                        planes[band.component][at..at + (bx + 32).min(band.width) - bx].fill(0.0);
                    }
                }
            }
            reconstruct(&mut planes, decoder);
            let mut error = 0.0f32;
            for (component, (got, want)) in planes.iter().zip(&reference).enumerate() {
                // A 4:2:0 chroma plane has half the picture's rows.
                let scale = if component != 0 && !full { 2 } else { 1 };
                let (first, last) = (64 * k / scale, (64 * (k + 1) / scale).min(rows / scale));
                for y in first..last {
                    for x in 0..stride / scale {
                        error = error.max((got[y * stride + x] - want[y * stride + x]).abs());
                    }
                }
            }
            errors.push(error);
        }
        errors
    }

    /// Random coefficients in every block of a fresh decoder's planes.
    fn random_pyramid(width: usize, height: usize, full: bool) -> Decoder {
        let mut decoder = Decoder::new(width, height, full).unwrap();
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        for plane in &mut decoder.planes {
            for value in plane.iter_mut() {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *value = (state >> 40) as f32 / (1u64 << 24) as f32 - 0.5;
            }
        }
        decoder
    }

    #[test]
    fn a_band_decodes_exactly_from_the_blocks_a_row_order_sends_before_its_cut() {
        // The inverse wavelet's four +-1 lifting steps reach under one block row at every level, so
        // block row r of level L is needed from band r * 2^L - 1 on: one band of lookahead.
        for (width, height, full) in [(1920, 1080, false), (1280, 720, true), (131, 97, true), (132, 98, false)] {
            let mut decoder = random_pyramid(width, height, full);
            let one = prefix_errors(&mut decoder, 1);
            println!("{width}x{height} 444={full}, one band of lookahead, max error per band: {one:?}");
            assert!(one.iter().all(|&e| e == 0.0), "{width}x{height}: {one:?}");
            // The negative control: no lookahead is wrong in every band but the last.
            let none = prefix_errors(&mut decoder, 0);
            println!("{width}x{height} 444={full}, no lookahead: {none:?}");
            if one.len() > 1 { assert!(none[..none.len() - 1].iter().any(|&e| e > 0.0), "{width}x{height}: {none:?}"); }
        }
    }

    /// Feeds `order` (block indices) to a plan `per_packet` records at a time, as the packets of a
    /// frame, and returns the slices that came due, with the packet each came due in.
    fn run_plan(plan: &mut SlicePlan, order: &[u32], per_packet: usize) -> Vec<(usize, Due)> {
        plan.begin_frame();
        let mut dues = Vec::new();
        let packets = order.chunks(per_packet).count();
        for (i, packet) in order.chunks(per_packet).enumerate() {
            plan.packet();
            for &index in packet { plan.record(index); }
            if i + 1 == packets { plan.finish(); }
            if let Some(due) = plan.due() { dues.push((i, due)); }
        }
        dues
    }

    #[test]
    fn a_row_ordered_frame_comes_due_slice_by_slice_before_it_ends() {
        let (width, height) = (1920u32, 1080u32);
        let steps = rubylight_protocol::pyrowave::rows::row_steps(width, height, false);
        let mut ordered: Vec<u32> = (0..steps.len() as u32).collect();
        ordered.sort_by_key(|&i| (steps[i as usize], i));
        let mut plan = SlicePlan::new(width, height, false, 4);
        let packets = ordered.chunks(40).count();
        let dues = run_plan(&mut plan, &ordered, 40);
        assert!(plan.report().ordered(), "{:?}", plan.report());
        assert_eq!(dues.len(), 3, "four slices, the last being the final decode: {dues:?}");
        assert!(dues.iter().enumerate().all(|(n, (_, due))| due.slice == n), "{dues:?}");
        assert!(dues.windows(2).all(|pair| pair[0].0 < pair[1].0), "each in a later packet: {dues:?}");
        assert!(dues.iter().all(|&(packet, _)| packet + 1 < packets), "all before the frame ends");
        // The slices tile the picture from the top, and the final decode takes the rest.
        assert_eq!(dues[0].1.first_row, 0);
        assert!(dues.windows(2).all(|pair| pair[0].1.end_row == pair[1].1.first_row));
        assert_eq!(plan.rest(), (dues[2].1.end_row, height));
        assert!(dues[2].1.end_row < height);
    }

    /// The order of a row-ordered frame with its first record (step 0) delivered `late` packets
    /// after the packet slice 0 comes due in.
    fn order_with_a_late_record(width: u32, height: u32, late: usize) -> Vec<u32> {
        let ordered = row_order(width, height);
        let mut plan = SlicePlan::new(width, height, false, 4);
        let first_due = run_plan(&mut plan, &ordered, 40)[0].0;
        let mut order = ordered;
        let record = order.remove(0);
        order.insert((first_due + late) * 40, record);
        order
    }

    #[test]
    fn one_record_two_packets_late_after_slice_zero_stops_slicing_and_is_not_exact() {
        let (width, height) = (1920u32, 1080u32);
        let order = order_with_a_late_record(width, height, 2);
        let mut plan = SlicePlan::new(width, height, false, 4);
        let dues = run_plan(&mut plan, &order, 40);
        // The frame is still ordered by the counter's tolerance (one violation) ...
        assert_eq!(plan.report().violations, 1, "{:?}", plan.report());
        assert!(plan.report().ordered(), "{:?}", plan.report());
        // ... but slice 0 came due before the record that belongs in it, and no slice follows.
        assert_eq!(dues.len(), 1, "{dues:?}");
        assert_eq!(dues[0].1.slice, 0);
    }

    #[test]
    fn an_index_ordered_frame_is_reported_unordered_and_stops_slicing() {
        let (width, height) = (1920u32, 1080u32);
        let count = rubylight_protocol::pyrowave::rows::row_steps(width, height, false).len() as u32;
        let index_order: Vec<u32> = (0..count).collect();
        let mut plan = SlicePlan::new(width, height, false, 4);
        let dues = run_plan(&mut plan, &index_order, 40);
        assert!(!plan.report().ordered(), "{:?}", plan.report());
        assert!(plan.due().is_none(), "no slice is offered once the order is broken");
        // Whatever came due before the first violation was seen is a prefix of the picture.
        assert!(dues.len() < 3, "{dues:?}");
        // The next frame starts clean: it is the same plan, reset.
        let steps = rubylight_protocol::pyrowave::rows::row_steps(width, height, false);
        let mut ordered: Vec<u32> = (0..count).collect();
        ordered.sort_by_key(|&i| (steps[i as usize], i));
        run_plan(&mut plan, &ordered, 40);
        assert!(plan.report().ordered());
    }

    fn row_order(width: u32, height: u32) -> Vec<u32> {
        let steps = rubylight_protocol::pyrowave::rows::row_steps(width, height, false);
        let mut ordered: Vec<u32> = (0..steps.len() as u32).collect();
        ordered.sort_by_key(|&i| (steps[i as usize], i));
        ordered
    }

    #[test]
    fn a_lost_packet_stops_slicing_and_makes_the_frame_unordered() {
        let (width, height) = (1920u32, 1080u32);
        let ordered = row_order(width, height);
        let mut plan = SlicePlan::new(width, height, false, 4);
        plan.begin_frame();
        let mut dues = 0;
        for (i, packet) in ordered.chunks(40).enumerate() {
            if i == 1 { plan.gap(); }
            plan.packet();
            for &index in packet { plan.record(index); }
            dues += usize::from(plan.due().is_some());
        }
        assert_eq!(dues, 0, "no slice after a gap, although the records themselves are in order");
        assert!(!plan.report().ordered(), "{:?}", plan.report());
        // The next frame starts clean.
        assert_eq!(run_plan(&mut plan, &ordered, 40).len(), 3);
    }

    #[test]
    fn the_final_decode_gate_judges_a_sliced_frame_by_its_loss_and_any_other_by_the_sdk() {
        let ready = || -> bool { panic!("a sliced frame never asks the SDK: its slice decodes cleared the flag") };
        assert_eq!(final_decode_gate(true, true, 0.0, ready), Ok(()));
        assert_eq!(final_decode_gate(true, true, SLICED_LOSS_LIMIT_PERCENT, ready), Ok(()), "the limit itself passes");
        assert!(final_decode_gate(true, true, SLICED_LOSS_LIMIT_PERCENT + 0.1, ready).unwrap_err().contains("lost too much"));
        // Slices decoded but the chunks were not the whole envelope: the SDK's gate decides.
        assert_eq!(final_decode_gate(true, false, 50.0, || true), Ok(()));
        assert!(final_decode_gate(true, false, 0.0, || false).unwrap_err().contains("no picture data"));
        // No slices: the SDK's gate, whatever the loss.
        assert_eq!(final_decode_gate(false, true, 50.0, || true), Ok(()));
        assert!(final_decode_gate(false, true, 0.0, || false).is_err());
    }

    #[cfg(windows)]
    fn race(width: u32, height: u32) -> Race {
        Race {
            plan: SlicePlan::new(width, height, false, 4),
            header_pushed: false,
            lease: None,
            bands: Vec::new(),
            failure: None,
            no_plane: false,
            ordered_run: ORDERED_FRAMES_TO_RESUME,
            slicing: true,
        }
    }

    /// Feeds a frame's records (40 a packet) to the race's plan, and finishes it as
    /// `GpuStream::frame` does, with a slice recorded for each one that came due.
    #[cfg(windows)]
    fn race_frame(race: &mut Race, order: &[u32], chunks: bool) -> (RaceReport, Vec<BandReady>) {
        race.begin_frame();
        for packet in order.chunks(40) {
            race.plan.packet();
            for &index in packet { race.plan.record(index); }
            if race.slicing {
                if let Some(due) = race.plan.due() {
                    race.bands.push(BandReady { slice: due.slice as u8, first_row: due.first_row, end_row: due.end_row, fence_value: 1, ready_us: 10 });
                }
            }
        }
        race.plan.finish();
        race.finish(2, 20, chunks)
    }

    #[cfg(windows)]
    #[test]
    fn an_unordered_frame_has_no_slices_and_slicing_waits_for_ordered_frames() {
        let (width, height) = (1920u32, 1080u32);
        let ordered = row_order(width, height);
        // In order for the first half, so slices come due before the break is seen, then reversed.
        let mut broken = ordered.clone();
        let tail = broken.len() / 2;
        broken[tail..].reverse();
        let mut race = race(width, height);
        let (report, bands) = race_frame(&mut race, &ordered, true);
        assert_eq!((report.ordered, report.slices), (RaceReport::ORDERED, 4));
        assert_eq!(bands.len(), 4);
        assert_eq!(bands.last().map(|b| b.end_row), Some(height));
        // Slices came due on the first half, which is in order; the frame then left the order.
        let (report, bands) = race_frame(&mut race, &broken, true);
        assert_eq!(report.ordered, RaceReport::UNORDERED);
        assert_eq!((report.slices, bands.len()), (1, 1), "the slices decoded before the break are not marked exact");
        assert_eq!((bands[0].first_row, bands[0].end_row), (0, height));
        // Quiet: ordered frames do not slice until ORDERED_FRAMES_TO_RESUME of them in a row.
        for n in 0..ORDERED_FRAMES_TO_RESUME {
            let (report, bands) = race_frame(&mut race, &ordered, true);
            assert_eq!(report.ordered, RaceReport::ORDERED);
            assert_eq!(bands.len(), 1, "ordered frame {n} after the break is still decoded whole");
        }
        let (_, bands) = race_frame(&mut race, &ordered, true);
        assert_eq!(bands.len(), 4, "slicing resumed");
        // A break in the middle of the run starts it over.
        race_frame(&mut race, &broken, true);
        race_frame(&mut race, &ordered, true);
        race_frame(&mut race, &broken, true);
        for _ in 0..ORDERED_FRAMES_TO_RESUME - 1 { race_frame(&mut race, &ordered, true); }
        assert_eq!(race_frame(&mut race, &ordered, true).1.len(), 1, "one frame short of the run");
        assert_eq!(race_frame(&mut race, &ordered, true).1.len(), 4);
    }

    #[cfg(windows)]
    #[test]
    fn a_late_record_after_a_slice_falls_the_frame_back_to_one_whole_decode() {
        let (width, height) = (1920u32, 1080u32);
        let late = order_with_a_late_record(width, height, 2);
        let mut race = race(width, height);
        let (report, bands) = race_frame(&mut race, &late, true);
        assert_eq!(report.ordered, RaceReport::UNORDERED, "slice 0 lacks the late record");
        assert_eq!((report.slices, bands.len()), (1, 1));
        assert_eq!((bands[0].first_row, bands[0].end_row), (0, height));
        // The run of ordered frames starts over: the next ordered frame is not sliced.
        let (report, bands) = race_frame(&mut race, &row_order(width, height), true);
        assert_eq!((report.ordered, bands.len()), (RaceReport::ORDERED, 1));
    }

    #[cfg(windows)]
    #[test]
    fn a_late_record_before_any_slice_still_counts_as_ordered() {
        let (width, height) = (1920u32, 1080u32);
        // A step 0 record delivered three packets after the cursor left step 0, but before slice 0
        // is due: late, with no slice yet to lack it.
        let steps = rubylight_protocol::pyrowave::rows::row_steps(width, height, false);
        let mut order = row_order(width, height);
        let left_step_0 = order.iter().position(|&i| steps[i as usize] > steps[order[0] as usize]).unwrap() / 40;
        let first_due = run_plan(&mut SlicePlan::new(width, height, false, 4), &order, 40)[0].0;
        assert!(left_step_0 + 3 < first_due, "{left_step_0} {first_due}");
        let record = order.remove(0);
        order.insert((left_step_0 + 3) * 40, record);
        let mut race = race(width, height);
        let (report, bands) = race_frame(&mut race, &order, true);
        assert_eq!(race.plan.report().violations, 1);
        assert_eq!(report.ordered, RaceReport::ORDERED);
        // Slicing stopped at the violation, so the one entry is the whole picture.
        assert_eq!((bands.len(), bands[0].first_row, bands[0].end_row), (1, 0, height));
        let (report, bands) = race_frame(&mut race, &row_order(width, height), true);
        assert_eq!((report.ordered, bands.len()), (RaceReport::ORDERED, 4), "the run was not reset");
    }

    #[cfg(windows)]
    #[test]
    fn a_frame_without_chunks_is_one_slice_of_the_whole_picture() {
        let (width, height) = (1920u32, 1080u32);
        let mut race = race(width, height);
        race_frame(&mut race, &row_order(width, height), true);
        // A frame that came whole never began: the plan still holds the last frame's slices, and
        // the final decode must not take its rows from them.
        assert!(race.plan.rest().0 > 0);
        let (report, bands) = race.finish(3, 30, false);
        assert_eq!(report, RaceReport::default());
        assert_eq!(bands, [BandReady { slice: 0, first_row: 0, end_row: height, fence_value: 3, ready_us: 30 }]);
    }

    #[test]
    #[ignore = "real 1080p frames from NIGHTJAR_PYROWAVE_FRAMES"]
    fn a_real_frame_decodes_its_bands_exactly_from_the_blocks_before_their_cut() {
        let Some(dir) = crate::sdl::test_input("NIGHTJAR_PYROWAVE_FRAMES", "a NIGHTJAR_PYROWAVE_DUMP directory") else { return };
        for (width, height, full, data) in dumped_frames(std::path::Path::new(&dir)).into_iter().take(3) {
            let mut decoder = Decoder::new(width, height, full).unwrap();
            decoder.coefficients(&data).unwrap();
            for lookahead in [1, 0] {
                let errors = prefix_errors(&mut decoder, lookahead);
                println!("real {width}x{height} 444={full}, lookahead {lookahead}, max error per band: {errors:?}");
                if lookahead == 1 { assert!(errors.iter().all(|&e| e == 0.0), "{errors:?}"); }
            }
        }
    }

    /// The legacy container of a fixture as one bitstream: the sequence header, then blocks.
    pub(super) fn bitstream(container: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        assert!(push_container(container, |packet| { out.extend_from_slice(packet); true }));
        out
    }

    /// The fixture's blocks packed whole into 1400-byte host packets, as (flags, payload).
    pub(super) fn host_packets(bitstream: &[u8]) -> Vec<(u32, Vec<u8>)> {
        use rubylight_protocol::pyrowave::{PACKET_FRAME_END, PACKET_FRAME_START, PACKET_RECORD_START};
        let mut payloads: Vec<Vec<u8>> = vec![Vec::new()];
        let mut at = 8;
        while at < bitstream.len() {
            let length = ((word(bitstream, at) >> 16) & 0xfff) as usize * 4;
            let limit = if payloads.len() == 1 { 1400 - 16 } else { 1400 };
            assert!(length <= limit, "the fixture has a record larger than a packet");
            if payloads.last().unwrap().len() + length > limit { payloads.push(Vec::new()); }
            payloads.last_mut().unwrap().extend_from_slice(&bitstream[at..at + length]);
            at += length;
        }
        let last = payloads.last().unwrap().len() + if payloads.len() == 1 { 16 } else { 0 };
        let mut first = vec![1, 0, 0, 0, last as u8, (last >> 8) as u8, 0, 0];
        first.extend_from_slice(&bitstream[..8]);
        first.extend_from_slice(&payloads[0]);
        payloads[0] = first;
        let count = payloads.len();
        payloads.into_iter().enumerate().map(|(i, payload)| {
            let mut flags = PACKET_RECORD_START;
            if i == 0 { flags |= PACKET_FRAME_START; }
            if i == count - 1 { flags |= PACKET_FRAME_END; }
            (flags, payload)
        }).collect()
    }

    /// The packets that arrived, in order, the one after a gap not contiguous.
    pub(super) fn arrived(packets: &[(u32, Vec<u8>)], lost: &[usize]) -> Vec<(u32, Vec<u8>)> {
        let mut out = Vec::new();
        let mut previous = None;
        for (i, (flags, payload)) in packets.iter().enumerate() {
            if lost.contains(&i) { continue; }
            let contiguous = if previous == Some(i.wrapping_sub(1)) { rubylight_protocol::pyrowave::PACKET_CONTIGUOUS } else { 0 };
            out.push((flags | contiguous, payload.clone()));
            previous = Some(i);
        }
        out
    }

    /// The client's frame data for those packets.
    pub(super) fn envelope(packets: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let mut out = vec![0; 4];
        for (flags, payload) in packets {
            out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            out.extend_from_slice(&flags.to_le_bytes());
            out.extend_from_slice(payload);
        }
        out
    }

    /// Decodes each frame whole and chunk by chunk: the same planes and loss, frame after frame.
    fn decode_both(width: usize, height: usize, full: bool, frames: &[Vec<(u32, Vec<u8>)>]) -> (Decoder, Decoder) {
        let mut whole = Decoder::new(width, height, full).unwrap();
        let mut early = Decoder::new(width, height, full).unwrap();
        for packets in frames {
            let data = envelope(packets);
            whole.coefficients(&data).unwrap();
            for (i, (flags, payload)) in packets.iter().enumerate() { early.chunk(i == 0, *flags, payload); }
            early.coefficients(&data).unwrap();
            assert_eq!(early.source, Source::Early);
            assert_eq!(whole.source, Source::Frame);
            assert!(early.first_block_us > 0 && whole.first_block_us == 0, "only an early frame has a first-block time");
            assert!(early.planes == whole.planes, "the planes differ");
            assert_eq!(early.loss_percent, whole.loss_percent);
        }
        (whole, early)
    }

    pub(super) fn fixture(which: usize) -> (&'static [u8], usize, usize, bool) {
        match which {
            0 => (include_bytes!("../../tests/fixtures/pyrowave-420-8.bin"), 132, 98, false),
            1 => (include_bytes!("../../tests/fixtures/pyrowave-420-10.bin"), 132, 98, false),
            2 => (include_bytes!("../../tests/fixtures/pyrowave-444-8.bin"), 131, 97, true),
            _ => (include_bytes!("../../tests/fixtures/pyrowave-444-10.bin"), 131, 97, true),
        }
    }

    #[test]
    fn early_chunks_decode_exactly_like_the_whole_frame() {
        for which in 0..4 {
            let (container, width, height, full) = fixture(which);
            let bits = bitstream(container);
            let packets = host_packets(&bits);
            assert!(packets.len() >= 2, "the fixture spans packets");
            let records = (word(&bits, 4) & 0xff_ffff) as usize;
            let (whole, early) = decode_both(width, height, full, &[arrived(&packets, &[])]);
            assert_eq!(early.early_blocks, records, "every block was decoded before the frame completed");
            // The envelope path agrees with the legacy container of the same blocks.
            let mut legacy = Decoder::new(width, height, full).unwrap();
            legacy.coefficients(container).unwrap();
            assert!(legacy.planes == whole.planes, "the envelope and the legacy container differ");
            early.coefficient_frame().unwrap();
        }
    }

    /// The fixture's records cut into packets of `piece` bytes wherever the cut falls, so a
    /// record runs over several packets: only the first has RECORD_START, the others continue it.
    pub(super) fn spanning_packets(bitstream: &[u8], piece: usize) -> Vec<(u32, Vec<u8>)> {
        use rubylight_protocol::pyrowave::{PACKET_CONTIGUOUS, PACKET_FRAME_END, PACKET_FRAME_START, PACKET_RECORD_START};
        let pieces: Vec<&[u8]> = bitstream[8..].chunks(piece).collect();
        let count = pieces.len();
        let last = pieces[count - 1].len() + if count == 1 { 16 } else { 0 };
        let mut prefix = vec![1, 0, 0, 0, last as u8, (last >> 8) as u8, 0, 0];
        prefix.extend_from_slice(&bitstream[..8]);
        pieces.iter().enumerate().map(|(i, piece)| {
            let mut flags = if i == 0 { PACKET_FRAME_START | PACKET_RECORD_START } else { PACKET_CONTIGUOUS };
            if i == count - 1 { flags |= PACKET_FRAME_END; }
            let mut payload = if i == 0 { prefix.clone() } else { Vec::new() };
            payload.extend_from_slice(piece);
            (flags, payload)
        }).collect()
    }

    #[test]
    fn records_spanning_packets_decode_early_like_the_whole_frame() {
        for which in 0..4 {
            let (container, width, height, full) = fixture(which);
            let bits = bitstream(container);
            let records = (word(&bits, 4) & 0xff_ffff) as usize;
            for piece in [97, 300, 1000] {
                let packets = spanning_packets(&bits, piece);
                assert!(packets.len() > 3, "the fixture spans packets at {piece} bytes");
                // Two frames, so the second can reuse the first's decoder state.
                let (whole, early) = decode_both(width, height, full, &[packets.clone(), packets]);
                assert_eq!(early.early_blocks, records, "fixture {which}, {piece}-byte packets");
                assert_eq!(early.loss_percent, 0.0);
                early.coefficient_frame().unwrap();
                drop(whole);
            }
        }
    }

    #[test]
    fn early_chunks_with_lost_packets_give_the_whole_frames_picture() {
        let (older, width, height, full) = fixture(0);
        let (newer, ..) = fixture(1);
        let first = host_packets(&bitstream(older));
        let second = host_packets(&bitstream(newer));
        let count = second.len();
        for lost in [vec![1], vec![1, count - 2], vec![count - 1], (1..count - 1).step_by(2).collect()] {
            // The second frame lacks packets; its missing records come from the first.
            let frames = [arrived(&first, &[]), arrived(&second, &lost)];
            let (whole, early) = decode_both(width, height, full, &frames);
            assert!(whole.loss_percent != 0.0, "the loss was seen ({lost:?})");
            early.coefficient_frame().unwrap();
        }
    }

    #[test]
    fn chunks_that_do_not_match_the_frame_decode_it_whole() {
        let (container, width, height, full) = fixture(0);
        let packets = arrived(&host_packets(&bitstream(container)), &[]);
        let data = envelope(&packets);
        let mut whole = Decoder::new(width, height, full).unwrap();
        whole.coefficients(&data).unwrap();

        // A chunk the queue dropped.
        let mut early = Decoder::new(width, height, full).unwrap();
        for (i, (flags, payload)) in packets.iter().enumerate() {
            if i != 1 { early.chunk(i == 0, *flags, payload); }
        }
        early.coefficients(&data).unwrap();
        assert_eq!((early.source, early.early_blocks), (Source::Fallback, 0));
        assert!(early.planes == whole.planes);

        // Chunks of a frame whose start the decoder never saw (the queue dropped its first
        // chunk) count as a fallback, once: the next frame is not marked.
        let mut early = Decoder::new(width, height, full).unwrap();
        for (flags, payload) in &packets[1..] { early.chunk(false, *flags, payload); }
        early.coefficients(&data).unwrap();
        assert_eq!((early.source, early.first_block_us), (Source::Fallback, 0));
        assert!(early.planes == whole.planes);
        early.coefficients(&data).unwrap();
        assert_eq!(early.source, Source::Frame);

        // A frame over the packet budget is refused and leaves no early state behind.
        let mut early = Decoder::new(width, height, full).unwrap();
        for (i, (flags, payload)) in packets.iter().enumerate() { early.chunk(i == 0, *flags, payload); }
        let oversized = vec![0u8; rubylight_protocol::pyrowave::MAX_FRAME_BYTES + 1];
        assert!(early.coefficients(&oversized).is_err());
        assert_eq!((early.source, early.early_blocks), (Source::Frame, 0));
        early.coefficients(&data).unwrap();
        assert_eq!(early.source, Source::Frame, "the refused frame took the early state with it");

        // A chunk the decoder cannot parse.
        let mut early = Decoder::new(width, height, full).unwrap();
        for (i, (flags, payload)) in packets.iter().enumerate() {
            early.chunk(i == 0, if i == 1 { 0xf0 } else { *flags }, payload);
        }
        early.coefficients(&data).unwrap();
        assert_eq!(early.source, Source::Fallback);
        assert!(early.planes == whole.planes);

        // Chunks of an abandoned frame do not leak into the next one.
        let mut early = Decoder::new(width, height, full).unwrap();
        for (i, (flags, payload)) in packets.iter().enumerate().take(3) { early.chunk(i == 0, *flags, payload); }
        for (i, (flags, payload)) in packets.iter().enumerate() { early.chunk(i == 0, *flags, payload); }
        early.coefficients(&data).unwrap();
        assert_eq!(early.source, Source::Early);
        assert!(early.planes == whole.planes);
    }

    /// `cargo test -p nightjar-desktop --release --bin nightjar -- --ignored --nocapture
    /// cpu_costs_of_a_pyrowave_frame`: what a 1080p frame costs on the CPU before the GPU
    /// wavelets, in the pieces early decode moves around (docs/early-decode.md).
    /// The envelopes `NIGHTJAR_PYROWAVE_DUMP` wrote, in frame order, with their geometry.
    pub(super) fn dumped_frames(dir: &std::path::Path) -> Vec<(usize, usize, bool, Vec<u8>)> {
        let mut names: Vec<_> = std::fs::read_dir(dir).unwrap().filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            parse_dump_name(&name).map(|geometry| (name, geometry))
        }).collect();
        names.sort();
        names.into_iter().map(|(name, (w, h, full))| (w, h, full, std::fs::read(dir.join(name)).unwrap())).collect()
    }

    #[test]
    #[ignore = "manual CPU timing"]
    fn cpu_costs_of_a_pyrowave_frame() {
        use std::time::Instant;
        let median = |mut v: Vec<f64>| { v.sort_by(f64::total_cmp); v[v.len() / 2] };
        // Entropy decoding, per megabyte of blocks, from the fixtures' own records.
        let mut bytes = 0usize;
        let mut blocks = Vec::new();
        for which in 0..4 {
            let bits = bitstream(fixture(which).0);
            let mut at = 8;
            while at < bits.len() {
                let length = ((word(&bits, at) >> 16) & 0xfff) as usize * 4;
                blocks.push(bits[at..at + length].to_vec());
                bytes += length;
                at += length;
            }
        }
        let mut out = [0.0; 1024];
        let rounds: Vec<f64> = (0..200).map(|_| {
            let start = Instant::now();
            for block in &blocks { decode_block(block, &mut out).unwrap(); }
            start.elapsed().as_secs_f64() * 1e3
        }).collect();
        println!("entropy decode: {:.2} ms per MB of blocks ({} blocks, {} bytes)", median(rounds) * 1e6 / bytes as f64, blocks.len(), bytes);
        // The plane clear and the half conversion of a whole 1080p 4:2:0 frame.
        let mut decoder = Decoder::new(1920, 1080, false).unwrap();
        let clear: Vec<f64> = (0..50).map(|_| {
            let start = Instant::now();
            for plane in &mut decoder.planes { plane.fill(0.0); }
            decoder.seen.fill(false);
            start.elapsed().as_secs_f64() * 1e3
        }).collect();
        println!("plane clear: {:.2} ms", median(clear));
        let convert: Vec<f64> = (0..50).map(|_| {
            let start = Instant::now();
            decoder.coefficient_frame().unwrap();
            start.elapsed().as_secs_f64() * 1e3
        }).collect();
        println!("half conversion plus frame allocation: {:.2} ms", median(convert));
        // Real frames from `NIGHTJAR_PYROWAVE_DUMP`: the whole CPU side with the stage split.
        let Some(dir) = std::env::var_os("NIGHTJAR_PYROWAVE_FRAMES") else { return };
        let frames = dumped_frames(std::path::Path::new(&dir));
        assert!(!frames.is_empty(), "no frame-*.bin envelopes in {dir:?}");
        let (width, height, full, _) = frames[0];
        let mut decoder = Decoder::new(width, height, full).unwrap();
        let mut total = Vec::new();
        for round in 0..3 {
            for &(w, h, f, ref data) in &frames {
                assert_eq!((w, h, f), (width, height, full), "one geometry per dump");
                let start = Instant::now();
                decoder.coefficients(data).unwrap();
                decoder.coefficient_frame().unwrap();
                if round > 0 { total.push(start.elapsed().as_secs_f64() * 1e3); }
            }
        }
        let bytes = frames.iter().map(|f| f.3.len()).sum::<usize>() / frames.len();
        println!("{} real frames of {bytes} bytes, {width}x{height} full={full}: decoder total {:.2} ms median, {:.2} ms p95",
            frames.len(), median(total.clone()), { total.sort_by(f64::total_cmp); total[total.len() * 95 / 100] });
        println!("stages {}", decoder.stage_summary().unwrap());
    }

    #[test]
    fn dumped_envelopes_are_named_for_their_geometry_and_read_back_in_order() {
        assert_eq!(dump_name(1920, 1080, false, 7), "frame-1920x1080-420-007.bin");
        assert_eq!(parse_dump_name("frame-1920x1080-420-007.bin"), Some((1920, 1080, false)));
        assert_eq!(parse_dump_name("frame-64x32-444-120.bin"), Some((64, 32, true)));
        assert_eq!(parse_dump_name("frame-64x32-422-001.bin"), None);
        assert_eq!(parse_dump_name("notes.txt"), None);
        let dir = std::env::temp_dir().join(format!("nightjar-dump-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for n in [10u32, 2, 1] { std::fs::write(dir.join(dump_name(132, 98, false, n)), [n as u8]).unwrap(); }
        std::fs::write(dir.join("other.bin"), [9]).unwrap();
        let frames = dumped_frames(&dir);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(frames.iter().map(|f| f.3[0]).collect::<Vec<_>>(), [1, 2, 10]);
        assert!(frames.iter().all(|f| (f.0, f.1, f.2) == (132, 98, false)));
    }

    #[test]
    fn a_frame_that_fails_to_decode_fails_the_same_way_early() {
        let (container, width, height, full) = fixture(0);
        let mut packets = arrived(&host_packets(&bitstream(container)), &[]);
        // The sequence header names another picture size.
        packets[0].1[8] ^= 1;
        let data = envelope(&packets);
        let mut whole = Decoder::new(width, height, full).unwrap();
        let expected = whole.coefficients(&data);
        assert!(expected.is_err());
        let mut early = Decoder::new(width, height, full).unwrap();
        for (i, (flags, payload)) in packets.iter().enumerate() { early.chunk(i == 0, *flags, payload); }
        assert_eq!(early.coefficients(&data), expected);
    }
}
