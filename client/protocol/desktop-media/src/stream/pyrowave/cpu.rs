//! CPU PyroWave bitstream decoder: entropy decoding, early chunk assembly, and FP16 plane packing.
//! Coefficient ordering and CDF 9/7 lifting follow Themaister/pyrowave a0d2e71 (MIT,
//! Copyright (c) 2026 Hans-Kristian Arntzen; app/src/main/assets/pyrowave_notices.txt).

use std::cell::RefCell;
use std::time::Instant;

use rubylight_protocol::pyrowave::{push_container, Records};

use super::super::stage_times::Stages;
use super::{pack, pass_records, word, Early, Source};

pub(super) struct Band {
    pub(super) component: usize,
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) x: usize,
    pub(super) y: usize,
    first: usize,
    pub(super) count: usize,
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
    pub(super) stride: usize,
    pub(super) rows: usize,
    pub(super) planes: [Vec<f32>; 3],
    pub(super) bands: Vec<Band>,
    pub(super) seen: Vec<bool>,
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
pub(super) fn parse_dump_name(name: &str) -> Option<(usize, usize, bool)> {
    let mut parts = name.strip_prefix("frame-")?.strip_suffix(".bin")?.split('-');
    let (width, height) = parts.next()?.split_once('x')?;
    let full = match parts.next()? { "420" => false, "444" => true, _ => return None };
    parts.next()?.parse::<u32>().ok()?;
    Some((width.parse().ok()?, height.parse().ok()?, full))
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

    /// Zeroes luma and only the active chroma rows (`rows / 2` for 4:2:0, full `rows` for 4:4:4).
    fn clear_planes(&mut self) {
        self.planes[0].fill(0.0);
        let chroma = self.stride * if self.chroma444 { self.rows } else { self.rows / 2 };
        self.planes[1][..chroma].fill(0.0);
        self.planes[2][..chroma].fill(0.0);
    }

    /// The stream starts over on this decoder (a reconnect that reuses it): drops any frame in
    /// flight, so no picture of the last stream can come out.
    pub fn flush(&mut self) {
        self.early = Early::Off;
        self.missed_start = false;
        self.source = Source::Frame;
        self.early_blocks = 0;
        self.early_done = 0;
        self.early_bytes = 0;
        self.first_block_us = 0;
        self.loss_percent = 0.0;
        self.packet.clear();
        self.records = Records::with_geometry(self.width as u32, self.height as u32, self.chroma444);
        self.clear_planes();
        self.seen.fill(false);
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
            self.clear_planes();
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
            // The clear starts where the assembly ended: one clock reading for both.
            let step = self.stages.get_mut().lap(ASSEMBLE, step);
            if !valid { return Err("Malformed PyroWave frame container"); }
            self.clear_planes();
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
    pub(super) fn reconstruct(&mut self) {
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
        let step = self.stages.borrow_mut().lap(ALLOC, step);
        let info = frame.info();
        #[cfg(target_arch = "x86_64")]
        let fast_half = std::is_x86_feature_detected!("f16c");
        #[cfg(not(target_arch = "x86_64"))]
        let fast_half = false;
        let chroma_rows = if self.chroma444 { self.rows } else { self.rows / 2 };
        // SAFETY: `frame` holds `self.rows` rows of `self.stride` RGBA16F pixels.
        unsafe {
            for y in 0..self.rows {
                let row = std::slice::from_raw_parts_mut(info.data[0].add(y * info.linesize[0] as usize).cast::<[u16; 4]>(), self.stride);
                let start = y * self.stride;
                let end = start + self.stride;
                if y < chroma_rows {
                    pack::pack_row(row, [&self.planes[0][start..end], &self.planes[1][start..end], &self.planes[2][start..end]], fast_half);
                } else {
                    pack::pack_luma_row(row, &self.planes[0][start..end], fast_half);
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
    // `bands` is strictly sorted by `first` (`bands[0].first == 0`, contiguous ranges covering `0..seen.len()`).
    let band = &bands[bands.partition_point(|band| band.first <= index) - 1];
    if index >= band.first + band.count { return Err("PyroWave block index exceeds the picture"); }
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

pub(super) fn decode_block(data: &[u8], output: &mut [f32; 1024]) -> Result<(), &'static str> {
    let ballot = word(data, 0) & 0xffff;
    let count = ballot.count_ones() as usize;
    let mut offset = 8 + count * 3;
    if offset > data.len() { return Err("Truncated PyroWave block metadata"); }
    let quant = data[4];
    let scale32 = (1.0 + f32::from(quant & 7) / 8.0) * 2.0f32.powi(4 - i32::from(quant >> 3));
    let mut magnitudes = [0u32; 1024];
    let mut scales = [0.0; 16];
    let mut compact = 0;
    let mut nonzero_blocks = 0u32;
    let mut significant = 0usize;
    let mut active = ballot;
    while active != 0 {
        let block = active.trailing_zeros() as usize;
        active &= active - 1;
        let code = u16::from_le_bytes([data[8 + compact * 2], data[9 + compact * 2]]);
        let q = data[8 + count * 2 + compact];
        scales[block] = scale32 * (f32::from(q >> 4) / 8.0 + 0.25);
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
        if code != 0 {
            nonzero_blocks |= 1 << block;
            significant += magnitudes[block * 64..block * 64 + 64].iter().filter(|&&m| m != 0).count();
        }
        compact += 1;
    }
    if offset + significant.div_ceil(8) > data.len() { return Err("Truncated PyroWave coefficient signs"); }
    let mut sign = 0;
    let mut active = nonzero_blocks;
    while active != 0 {
        let block = active.trailing_zeros() as usize;
        active &= active - 1;
        let scale = scales[block];
        let (base_x, base_y) = ((block % 4) * 8, (block / 4) * 8);
        for (local, &magnitude) in magnitudes[block * 64..block * 64 + 64].iter().enumerate() {
            if magnitude == 0 { continue; }
            let x = base_x + ((local >> 1) & 3) + ((local >> 5) << 2);
            let y = base_y + (local & 1) + (((local >> 3) & 3) << 1);
            let negative = (data[offset + sign / 8] >> (sign % 8)) & 1 != 0;
            let value = (magnitude as f32 + 0.5) * scale;
            if value > 65504.0 { return Err("PyroWave coefficient exceeds FP16 storage"); }
            output[y * 32 + x] = if negative { -value } else { value };
            sign += 1;
        }
    }
    Ok(())
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
