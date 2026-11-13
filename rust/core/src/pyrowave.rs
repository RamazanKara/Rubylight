//! Vibepollo 2.0 PyroWave framing, partial recovery and transport budgets.
use anyhow::{Context, Result, bail};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
mod framing;
pub use framing::{Framed, Framing, Payload};
pub const BITSTREAM_ID: &str = "186f0393";
pub const PACKET_BOUNDARY: usize = 1024;
const PADDING: u32 = u32::MAX;
fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}
/// Where record framing writes: the framed bytes themselves
/// ([`record_frame_into`]), or the pieces of the bitstream they come from
/// ([`Framing::records`]). One framing runs for both.
trait Sink {
    fn written(&self) -> usize;
    fn clear(&mut self);
    fn reserve(&mut self, bytes: usize);
    /// `len` bytes of `bitstream` from `offset`.
    fn copy(&mut self, bitstream: &[u8], offset: usize, len: usize);
    /// A padding record of `bytes`, at least eight and a multiple of four.
    fn pad(&mut self, bytes: usize);
}
impl Sink for Vec<u8> {
    fn written(&self) -> usize {
        Vec::len(self)
    }
    fn clear(&mut self) {
        Vec::clear(self);
    }
    fn reserve(&mut self, bytes: usize) {
        Vec::reserve(self, bytes);
    }
    fn copy(&mut self, bitstream: &[u8], offset: usize, len: usize) {
        self.extend_from_slice(&bitstream[offset..offset + len]);
    }
    fn pad(&mut self, bytes: usize) {
        padding(self, bytes);
    }
}
pub fn aligned_payload(packet_size: usize) -> usize {
    let size = packet_size.saturating_sub(16);
    if size >= 24 && size.is_multiple_of(4) {
        size
    } else {
        0
    }
}
/// Severe detail loss warning: 2.5 bpp below 1080p, otherwise 1.5 bpp.
/// One-pixel text in the 720p desktop needed more bits per pixel.
/// These synthetic-scene floors are not clean-picture targets; see rust/PERFORMANCE.md.
pub fn minimum_kbps(width: u32, height: u32, fps_millihz: u32) -> u32 {
    bitrate_kbps(
        width,
        height,
        fps_millihz,
        if height < 1080 { 25 } else { 15 },
    )
}
/// Clean-picture target: 5 bpp below 1080p, otherwise 3.2 bpp.
/// A conservative SDR/HDR, 4:2:0/4:4:4 target for the measured scenes,
/// not a guarantee for every picture or a sustained network rate.
pub fn recommended_kbps(width: u32, height: u32, fps_millihz: u32) -> u32 {
    bitrate_kbps(
        width,
        height,
        fps_millihz,
        if height < 1080 { 50 } else { 32 },
    )
}
fn bitrate_kbps(width: u32, height: u32, fps_millihz: u32, bpp_tenths: u32) -> u32 {
    (u128::from(width) * u128::from(height) * u128::from(fps_millihz) * u128::from(bpp_tenths))
        .div_ceil(10_000_000)
        .min(u128::from(u32::MAX)) as u32
}
pub fn max_frame_bytes(packet_size: usize, critical_fec: bool) -> usize {
    (packet_size.saturating_sub(16) * if critical_fec { 3000 } else { 4000 }).saturating_sub(8)
}
/// Wire bytes a packet costs beyond its packet size: RTP, encryption, UDP,
/// IPv6 and Ethernet preamble, gap and FCS.
pub const WIRE_OVERHEAD: usize = 134;
/// Parity shards a critical block always gets, at any percentage above zero.
pub const MIN_PARITY_SHARDS: usize = 2;
/// Kbps a PyroWave stream's total keeps for audio and control: eight
/// high-quality channels with their parity, as Vibeshine's calibration assumes.
pub const AUDIO_CONTROL_KBPS: f64 = 3572.;
/// The parity of the largest critical block `packets` data packets can need.
fn max_critical_parity(critical_fec_percentage: u32, packets: usize, minimum: usize) -> usize {
    if critical_fec_percentage == 0 {
        return 0;
    }
    (1..=packets.min(255))
        .map(|critical| critical_parity_bound(critical, critical_fec_percentage as usize, minimum))
        .max()
        .unwrap_or(0)
}
pub fn critical_parity_bound(critical: usize, percentage: usize, minimum: usize) -> usize {
    let percentage = percentage.min(255usize.saturating_sub(critical) * 100 / critical.max(1));
    let parity = parity(critical, percentage, minimum);
    if critical + parity <= 255 { parity } else { 0 }
}
/// The picture bytes a frame's wire allowance carries: whole packets with
/// their wire overhead, less one critical block's parity, the frame header
/// and the shard headers. Vibeshine's `pyrowave::bandwidth::image_bytes`,
/// which Nonary's Moonlight calibrates with.
pub fn image_bytes(wire_bytes: usize, packet_size: usize, critical_fec_percentage: u32) -> usize {
    image_bytes_with_minimum(
        wire_bytes,
        packet_size,
        critical_fec_percentage,
        MIN_PARITY_SHARDS,
    )
}
fn image_bytes_with_minimum(
    wire_bytes: usize,
    packet_size: usize,
    critical_fec_percentage: u32,
    minimum: usize,
) -> usize {
    if packet_size <= 16 {
        return 0;
    }
    let packets = wire_bytes / (packet_size + WIRE_OVERHEAD);
    let parity = max_critical_parity(critical_fec_percentage, packets, minimum);
    if packets <= parity {
        return 0;
    }
    ((packets - parity) * (packet_size - 16)).saturating_sub(8) & !3
}
/// A frame's picture budget. With record framing (Nonary's Moonlight,
/// Nightjar, Aurora) the bitrate is the video's total on the wire, as
/// Vibeshine takes it since `PyroWaveWireBudgetVersion` 1, so a calibrated
/// rate is never exceeded by packet overhead and FEC.
pub fn budget(
    kbps: u32,
    interval: Duration,
    packet_size: usize,
    records: bool,
    critical_fec_percentage: u32,
) -> usize {
    budget_with_minimum(
        kbps,
        interval,
        packet_size,
        records,
        critical_fec_percentage,
        MIN_PARITY_SHARDS,
    )
}
/// The encoder and adaptive bitrate floor must reserve the same negotiated
/// minimum parity as the packetizer, including its representable wire rate.
pub fn budget_with_minimum(
    kbps: u32,
    interval: Duration,
    packet_size: usize,
    records: bool,
    critical_fec_percentage: u32,
    minimum: usize,
) -> usize {
    let minimum = minimum.clamp(MIN_PARITY_SHARDS, 255);
    let limit = max_frame_bytes(packet_size, records && critical_fec_percentage > 0);
    let allowance = (f64::from(kbps) * 125. * interval.as_secs_f64()) as usize;
    if records {
        image_bytes_with_minimum(allowance, packet_size, critical_fec_percentage, minimum)
            .min(limit * 95 / 100)
            & !3
    } else {
        legacy_budget(
            kbps,
            interval,
            packet_size,
            critical_fec_percentage,
            minimum,
        )
    }
}
/// Legacy bitrate has always paid for picture bytes, with up to 50% length
/// framing overhead. Reserve ordinary parity within that existing worst-case
/// packet count instead of adding it on top of the client's bitrate.
fn legacy_budget(
    kbps: u32,
    interval: Duration,
    packet_size: usize,
    percentage: u32,
    minimum: usize,
) -> usize {
    if packet_size <= 16 {
        return 0;
    }
    let allowance = (f64::from(kbps) * 125. * interval.as_secs_f64()) as usize;
    let limit = max_frame_bytes(packet_size, false);
    // Every codec block may be only eight bytes, with a four-byte length.
    let raw = allowance.min(limit.saturating_sub(4) / 12 * 8) & !3;
    if percentage == 0 {
        return raw;
    }
    let slice = packet_size - 16;
    let packets = (12 + raw + raw / 8 * 4).div_ceil(slice);
    let percentage = (percentage as usize).min(255);
    let minimum = minimum.clamp(MIN_PARITY_SHARDS, 255);
    let mut data = packets * 100 / (100 + percentage);
    while data > 0 {
        let (blocks, _) = crate::packet::conventional_fec(data, percentage, minimum);
        let aligned = data.div_ceil(blocks);
        // Budget the requested percentage even where the transport has to
        // reduce or omit it. This keeps bitrate adaptation conservative.
        let fec: usize = (0..blocks)
            .map(|block| {
                parity(
                    data.saturating_sub(block * aligned).min(aligned),
                    percentage,
                    minimum,
                )
            })
            .sum();
        if data + fec <= packets {
            return ((data * slice).saturating_sub(12) / 12 * 8).min(raw);
        }
        data -= 1;
    }
    0
}
/// The time each frame's budget pays for: the gap since the previous encode.
/// A new picture right after a repeat of the old one would get only that short
/// gap, a blurred frame; it gets at least the time since the previous new
/// picture, up to one stream period. Critical FEC keeps every frame within one
/// period, as Vibepollo's stable frame size does.
pub struct Interval {
    nominal: Duration,
    stable: bool,
    previous: Option<Instant>,
    fresh: Option<Instant>,
}
impl Interval {
    pub fn new(nominal: Duration, stable: bool) -> Self {
        Self {
            nominal,
            stable,
            previous: None,
            fresh: None,
        }
    }
    pub fn next(&mut self, now: Instant, repeat: bool) -> Duration {
        let nominal = self.nominal;
        let since =
            |at: Option<Instant>| at.map_or(nominal, |at| now.saturating_duration_since(at));
        let mut interval = since(self.previous.replace(now));
        if !repeat {
            interval = interval.max(since(self.fresh.replace(now)).min(nominal));
        }
        if self.stable {
            interval.min(nominal)
        } else {
            interval
        }
    }
}
pub fn container(bitstream: &[u8], packets: &[(usize, usize)]) -> Result<Vec<u8>> {
    if packets.is_empty() || packets.len() > u32::MAX as usize {
        bail!("invalid PyroWave packet count");
    }
    let mut out = Vec::with_capacity(bitstream.len() + 4 + packets.len() * 4);
    out.extend_from_slice(&(packets.len() as u32).to_le_bytes());
    for &(offset, size) in packets {
        let end = offset
            .checked_add(size)
            .context("PyroWave packet overflow")?;
        if size == 0 || size > u32::MAX as usize || end > bitstream.len() {
            bail!("invalid PyroWave packet slice");
        }
        out.extend_from_slice(&(size as u32).to_le_bytes());
        out.extend_from_slice(&bitstream[offset..end]);
    }
    Ok(out)
}
#[derive(Clone, Copy)]
struct Record {
    offset: usize,
    size: usize,
    index: u32,
}
fn records(frame: &[u8]) -> Result<Vec<Record>> {
    records_in(&mut Payload::Bytes(frame).reader())
}
/// [`records`] of a contiguous or a framed picture.
fn records_in(frame: &mut framing::Reader) -> Result<Vec<Record>> {
    let len = frame.len();
    if len < 8
        || !len.is_multiple_of(4)
        || frame.word(0) & 0x80000000 == 0
        || frame.word(0) == PADDING
        || (frame.word(4) >> 24) & 3 != 0
    {
        bail!("invalid PyroWave sequence header");
    }
    let sequence = (frame.word(0) >> 28) & 7;
    let mut records = Vec::new();
    let mut offset = 8;
    while offset < len {
        if len - offset < 8 {
            bail!("short PyroWave record");
        }
        let header = frame.word(offset);
        let padding = header == PADDING;
        let size = if padding {
            8usize
                .checked_add(frame.word(offset + 4) as usize * 4)
                .context("padding overflow")?
        } else {
            (((header >> 16) & 0xfff) as usize) * 4
        };
        if size < 8
            || size > len - offset
            || (!padding && (header & 0x80000000 != 0 || (header >> 28) & 7 != sequence))
        {
            bail!("invalid PyroWave block record");
        }
        if !padding {
            records.push(Record {
                offset,
                size,
                index: frame.word(offset + 4) >> 8,
            });
        }
        offset += size;
    }
    if records.len() != (frame.word(4) & 0xffffff) as usize {
        bail!("PyroWave record count mismatch");
    }
    Ok(records)
}
/// The coarse records of a picture whose sequence header is `header`.
fn coarse_count(header: u32) -> u32 {
    let extent = |n: u32| n.div_ceil(32).max(4).div_ceil(32);
    extent((header & 0x3fff) + 1) * extent(((header >> 14) & 0x3fff) + 1) * 12
}
fn padding(out: &mut Vec<u8>, bytes: usize) {
    debug_assert!(bytes >= 8 && bytes.is_multiple_of(4));
    out.extend_from_slice(&PADDING.to_le_bytes());
    out.extend_from_slice(&((bytes - 8) as u32 / 4).to_le_bytes());
    out.resize(out.len() + bytes - 8, 0);
}
// Earliest fitting record in O(log 4096), preserving encoder order for ties.
struct Fits {
    queues: Vec<VecDeque<usize>>,
    tree: Vec<usize>,
}
impl Fits {
    fn new(records: &[Record]) -> Self {
        let size = (records.iter().map(|r| r.size / 4).max().unwrap_or(0) + 1).next_power_of_two();
        let mut s = Self {
            queues: vec![VecDeque::new(); size],
            tree: vec![usize::MAX; size * 2],
        };
        for (i, r) in records.iter().enumerate() {
            s.queues[r.size / 4].push_back(i);
        }
        for i in 0..size {
            s.tree[size + i] = s.queues[i].front().copied().unwrap_or(usize::MAX);
        }
        for i in (1..size).rev() {
            s.tree[i] = s.tree[2 * i].min(s.tree[2 * i + 1]);
        }
        s
    }
    fn fitting(&self, words: usize) -> usize {
        let size = self.queues.len();
        let (mut lo, mut hi, mut result) = (size, size + words.min(size - 1) + 1, usize::MAX);
        while lo < hi {
            if lo & 1 != 0 {
                result = result.min(self.tree[lo]);
                lo += 1;
            }
            if hi & 1 != 0 {
                hi -= 1;
                result = result.min(self.tree[hi]);
            }
            lo /= 2;
            hi /= 2;
        }
        result
    }
    fn exact(&self, words: usize) -> usize {
        self.tree
            .get(self.queues.len() + words)
            .copied()
            .unwrap_or(usize::MAX)
    }
    fn remove(&mut self, words: usize) {
        self.queues[words].pop_front();
        let mut at = self.queues.len() + words;
        self.tree[at] = self.queues[words].front().copied().unwrap_or(usize::MAX);
        while at > 1 {
            at /= 2;
            let first = self.tree[2 * at].min(self.tree[2 * at + 1]);
            if self.tree[at] == first {
                break;
            }
            self.tree[at] = first;
        }
    }
}
/// One block of the picture's index space: its wavelet level (0 is the finest) and its row of
/// 32x32 blocks inside its band. The index space runs level 4 to 0, per level the components,
/// per component the bands, per band the blocks in rows; `coarse_count` is its level 4 part.
struct Block {
    level: u32,
    row: u32,
}
fn blocks(width: u32, height: u32, chroma444: bool) -> Vec<Block> {
    let stride = width.div_ceil(32).max(4) * 32;
    let rows = height.div_ceil(32).max(4) * 32;
    let mut out = Vec::new();
    for level in (0..5).rev() {
        for component in 0..3 {
            // 4:2:0 chroma has no level 0.
            if level == 0 && component != 0 && !chroma444 {
                continue;
            }
            let (across, down) = (
                (stride >> (level + 1)).div_ceil(32),
                (rows >> (level + 1)).div_ceil(32),
            );
            for _band in 0..if level == 4 { 4 } else { 3 } {
                out.extend(
                    (0..down).flat_map(|row| (0..across).map(move |_| Block { level, row })),
                );
            }
        }
    }
    out
}
/// The picture band (64 rows, one level 0 block row) after which each block of the index space is
/// sent in row order. Band k needs level L's block row r when `r * 2^L - 1 <= k`: the inverse
/// wavelet's four +-1 lifting steps reach under one block row at every level, so one band of
/// lookahead covers it. Level 4, the critical FEC block, is all step 0.
pub fn row_steps(width: u32, height: u32, chroma444: bool) -> Vec<u16> {
    let bands = (height.div_ceil(32).max(4) * 32).div_ceil(64);
    blocks(width, height, chroma444)
        .iter()
        .map(|b| {
            if b.level == 4 {
                0
            } else {
                ((b.row << b.level).saturating_sub(1)).min(bands - 1) as u16
            }
        })
        .collect()
}
/// `row_steps` for the frame's own geometry, kept from the frame before: a stream's pictures all
/// have one size, so the table is built once.
type RowStepsEntry = (u32, bool, Arc<Vec<u16>>);
fn cached_row_steps(bitstream: &[u8]) -> Arc<Vec<u16>> {
    static LAST: Mutex<Option<RowStepsEntry>> = Mutex::new(None);
    let header = word(bitstream, 0);
    let chroma444 = (word(bitstream, 4) >> 26) & 1 != 0;
    let mut last = LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((h, c, steps)) = last.as_ref()
        && *h == header
        && *c == chroma444
    {
        return steps.clone();
    }
    let steps = Arc::new(row_steps(
        (header & 0x3fff) + 1,
        ((header >> 14) & 0x3fff) + 1,
        chroma444,
    ));
    *last = Some((header, chroma444, steps.clone()));
    steps
}
pub fn record_frame(bitstream: &[u8], payload: usize, limit: usize) -> Result<Vec<u8>> {
    record_frame_with(bitstream, payload, limit, false)
}
/// `record_frame`, with the fine records in row order when `rows` is set (the client asked with
/// `x-ss-video[0].pyrowaveRowOrder`): the coarse records first as ever, then each picture band's
/// records with what it needs before the next band's. The same records and bytes; only the order
/// and the padding between packets differ.
pub fn record_frame_with(
    bitstream: &[u8],
    payload: usize,
    limit: usize,
    rows: bool,
) -> Result<Vec<u8>> {
    record_frame_into(bitstream, payload, limit, rows, Vec::new())
}
/// [`record_frame_with`] written into `out`, whose contents are replaced: a caller that reuses
/// large buffers keeps its allocation.
pub fn record_frame_into(
    bitstream: &[u8],
    payload: usize,
    limit: usize,
    rows: bool,
    mut out: Vec<u8>,
) -> Result<Vec<u8>> {
    frame_into(&mut out, bitstream, payload, limit, rows)?;
    Ok(out)
}
/// The record framing of `bitstream` into `out`, as bytes or as pieces.
fn frame_into<S: Sink>(
    out: &mut S,
    bitstream: &[u8],
    payload: usize,
    limit: usize,
    rows: bool,
) -> Result<()> {
    let all = records(bitstream)?;
    if bitstream.len() > limit {
        bail!("PyroWave frame exceeds transport capacity");
    }
    out.clear();
    if payload == 0 {
        out.copy(bitstream, 0, bitstream.len());
        return Ok(());
    }
    let coarse = coarse_count(word(bitstream, 0));
    let steps = rows.then(|| cached_row_steps(bitstream));
    out.reserve(bitstream.len());
    out.copy(bitstream, 0, 8);
    for required in [true, false] {
        if let Some(steps) = steps.as_ref().filter(|_| !required) {
            let mut class: Vec<Record> =
                all.iter().filter(|r| r.index >= coarse).copied().collect();
            let step = |r: &Record| {
                steps
                    .get(r.index as usize)
                    .copied()
                    .context("PyroWave block index exceeds the picture")
            };
            for r in &class {
                step(r)?;
            }
            class.sort_by_key(|r| (steps[r.index as usize], r.index));
            // A record over a packet goes out once every ordinary record ranked before it has; the
            // ones ranked after it may still fill the packet it leaves open.
            let (mut ordinary, mut bigs) = (Vec::new(), Vec::new());
            for r in class {
                if r.size > payload - 8 {
                    bigs.push((ordinary.len(), r));
                } else {
                    ordinary.push(r);
                }
            }
            first_fit(out, bitstream, &ordinary, &bigs, payload);
            continue;
        }
        let mut ordinary = Vec::new();
        for r in all.iter().filter(|r| (r.index < coarse) == required) {
            if r.size > payload - 8 {
                big(out, bitstream, r, payload);
            } else {
                ordinary.push(*r);
            }
        }
        first_fit(out, bitstream, &ordinary, &[], payload);
    }
    if out.written() > limit {
        out.clear();
        out.copy(bitstream, 0, bitstream.len());
    }
    Ok(())
}
fn big(out: &mut impl Sink, bitstream: &[u8], r: &Record, payload: usize) {
    let room = payload - (out.written() + 8) % payload;
    if r.size % payload == room - 4 {
        out.pad(8);
    }
    out.copy(bitstream, r.offset, r.size);
}
/// Packs `ordinary` first-fit. Each of `bigs` (with how many ordinary records are ranked before
/// it) goes out as soon as those are all placed.
fn first_fit(
    out: &mut impl Sink,
    bitstream: &[u8],
    ordinary: &[Record],
    bigs: &[(usize, Record)],
    payload: usize,
) {
    let mut fits = Fits::new(ordinary);
    // Only a frame with records over a packet in row order needs to know what is placed.
    let mut placed = if bigs.is_empty() {
        Vec::new()
    } else {
        vec![false; ordinary.len()]
    };
    let (mut front, mut next) = (0, 0);
    for _ in 0..ordinary.len() {
        if !bigs.is_empty() {
            while front < placed.len() && placed[front] {
                front += 1;
            }
            while next < bigs.len() && bigs[next].0 <= front {
                big(out, bitstream, &bigs[next].1, payload);
                next += 1;
            }
        }
        loop {
            let room = payload - (out.written() + 8) % payload;
            let mut chosen = fits.fitting(room / 4);
            if chosen != usize::MAX && room - ordinary[chosen].size == 4 {
                chosen = fits.exact(room / 4).min(if room >= 16 {
                    fits.fitting((room - 8) / 4)
                } else {
                    usize::MAX
                });
            }
            if chosen == usize::MAX {
                out.pad(room);
                continue;
            }
            let r = ordinary[chosen];
            fits.remove(r.size / 4);
            if !placed.is_empty() {
                placed[chosen] = true;
            }
            out.copy(bitstream, r.offset, r.size);
            break;
        }
    }
    for (_, r) in &bigs[next..] {
        big(out, bitstream, r, payload);
    }
}
pub struct Layout {
    pub critical_shards: usize,
    pub starts: Vec<bool>,
}
/// Where `frame`'s records start packets, and how many packets its coarse
/// records fill; `frame` is the framed picture, contiguous or in pieces.
pub fn layout<'a>(frame: impl Into<Payload<'a>>, payload: usize) -> Result<Layout> {
    let frame = frame.into();
    let all = records_in(&mut frame.reader())?;
    let len = frame.len();
    let mut layout = Layout {
        critical_shards: 0,
        starts: vec![false; (len + 8).div_ceil(payload.max(1))],
    };
    if payload == 0 {
        return Ok(layout);
    }
    let mut words = frame.reader();
    let coarse = coarse_count(words.word(0));
    let mut fine_seen = false;
    let mut critical = 8;
    for r in &all {
        if r.index < coarse {
            if fine_seen {
                return Ok(layout);
            }
            critical = r.offset + r.size;
        } else {
            fine_seen = true;
        }
        if r.size <= payload - 8 && (r.offset + 8) % payload + r.size > payload {
            return Ok(layout);
        }
    }
    layout.critical_shards = (critical + 8).div_ceil(payload);
    let mut offset = 0;
    while offset < len {
        if offset == 0 || (offset + 8).is_multiple_of(payload) {
            layout.starts[(offset + 8) / payload] = true;
        }
        let header = words.word(offset);
        offset += if header == PADDING {
            8 + words.word(offset + 4) as usize * 4
        } else if header & 0x80000000 != 0 {
            8
        } else {
            ((header >> 16) & 0xfff) as usize * 4
        };
    }
    Ok(layout)
}
#[derive(Debug, Clone, Copy)]
pub struct FecBlock {
    pub data: usize,
    pub percentage: usize,
}
pub fn parity(data: usize, percentage: usize, minimum: usize) -> usize {
    fec_parameters(data, percentage, minimum).0
}
/// The parity count must be exactly derivable from the percentage on the wire:
/// it also selects the receiver's Reed-Solomon matrix, not just its packet count.
pub(crate) fn fec_parameters(data: usize, percentage: usize, minimum: usize) -> (usize, usize) {
    if data == 0 || percentage == 0 {
        return (0, percentage);
    }
    let fec = (data * percentage).div_ceil(100);
    let minimum = minimum_parity(data, minimum);
    if fec >= minimum {
        return (fec, percentage);
    }
    let mut effective = 100 * minimum / data;
    if (data * effective).div_ceil(100) < minimum {
        effective += 1;
    }
    ((data * effective).div_ceil(100), effective)
}
/// A client's minimum parity, capped where the 8-bit FEC percentage that
/// carries it can still describe it: 3 parity shards for 1 data shard would
/// need 300% and spill into the shard index bits.
pub fn minimum_parity(data: usize, minimum: usize) -> usize {
    minimum.min(data * 255 / 100)
}
fn split(blocks: &mut Vec<FecBlock>, data: usize, slots: usize) {
    if data == 0 {
        return;
    }
    let count = data.div_ceil(255).clamp(1, slots);
    let size = data.div_ceil(count);
    for i in 0..count {
        blocks.push(FecBlock {
            data: if i + 1 == count {
                data - size * i
            } else {
                size
            },
            percentage: 0,
        });
    }
}
pub fn plan(
    total: usize,
    critical: usize,
    rate: usize,
    minimum: usize,
    detail: usize,
    extra: usize,
) -> Result<Vec<FecBlock>> {
    if total == 0 || total > 4000 || rate > 255 || minimum > 255 {
        bail!("invalid PyroWave FEC plan");
    }
    let critical = critical.min(total);
    let mut out = vec![];
    if critical == 0 || rate == 0 || critical + parity(critical, rate, minimum) > 255 {
        split(&mut out, total, 4);
    } else {
        out.push(FecBlock {
            data: critical,
            percentage: rate,
        });
        let fine = total - critical;
        let detail = detail.min(50);
        let mut extra = extra.min(parity(fine, detail, 0));
        let capacity = |rate| {
            (1..=255)
                .rev()
                .find(|&data| data + parity(data, rate, minimum) <= 255)
                .unwrap_or(0)
        };
        for percentage in (1..=detail).rev() {
            let cap = capacity(percentage);
            if cap == 0 || fine == 0 {
                continue;
            }
            let count = fine.div_ceil(cap);
            if count > 3 {
                continue;
            }
            let candidate: Vec<_> = (0..count)
                .map(|i| FecBlock {
                    data: fine / count + usize::from(i < fine % count),
                    percentage,
                })
                .collect();
            if candidate
                .iter()
                .all(|b| b.data + parity(b.data, b.percentage, minimum) <= 255)
                && candidate
                    .iter()
                    .map(|b| parity(b.data, b.percentage, minimum))
                    .sum::<usize>()
                    <= extra
            {
                out.extend(candidate);
                return Ok(out);
            }
        }
        let base = parity(critical, rate, minimum);
        if detail > 0 && extra > 0 {
            let mut leading = critical;
            while leading < total.min(capacity(rate))
                && leading + 1 + parity(leading + 1, rate, minimum) <= 255
                && parity(leading + 1, rate, minimum).saturating_sub(base) <= extra
            {
                leading += 1;
            }
            out[0].data = leading;
            extra -= parity(leading, rate, minimum).saturating_sub(base);
            let mut remaining = total - leading;
            while remaining > 0 && out.len() < 4 {
                let mut data = remaining.min(capacity(detail));
                while data > 0
                    && (parity(data, detail, minimum) > extra
                        || data + parity(data, detail, minimum) > 255)
                {
                    data -= 1;
                }
                if data == 0 || remaining - data > (3 - out.len()) * 1023 {
                    break;
                }
                out.push(FecBlock {
                    data,
                    percentage: detail,
                });
                extra -= parity(data, detail, minimum);
                remaining -= data;
            }
            if remaining > 0 {
                let slots = 4 - out.len();
                split(&mut out, remaining, slots);
            }
        } else {
            split(&mut out, fine, 3);
        }
    }
    if out.len() > 4
        || out.iter().any(|b| {
            b.data > 1023
                || (b.percentage > 0 && b.data + parity(b.data, b.percentage, minimum) > 255)
        })
    {
        bail!("PyroWave frame exceeds FEC capacity");
    }
    Ok(out)
}
/// A record's contents without the sequence bits of its fourth byte, which
/// change every frame. Eight bytes a step: a byte at a time took 0.9 ms of a
/// 0.83 MB frame (800 Mbps at 120 fps) before its first packet could leave.
fn record_hash(bytes: &[u8]) -> u64 {
    let mix = |h: u64, word: u64| {
        (h ^ word)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            .rotate_left(29)
    };
    let (words, rest) = bytes.as_chunks::<8>();
    let mut h = 0xcbf2_9ce4_8422_2325u64 ^ bytes.len() as u64;
    for (i, word) in words.iter().enumerate() {
        let word = u64::from_le_bytes(*word);
        h = mix(h, if i == 0 { word & !(0x70 << 24) } else { word });
    }
    let start = words.len() * 8;
    for (i, &b) in rest.iter().enumerate() {
        h = mix(h, u64::from(if start + i == 3 { b & 0x8f } else { b }));
    }
    h
}
/// No idle credit: detail protection is bounded by this frame's allowance.
pub struct DetailFec {
    nominal: f64,
    previous: Option<Instant>,
    interval: f64,
    stable: f64,
    blocks: HashMap<u32, (u64, usize)>,
}
impl DetailFec {
    pub fn new(fps_millihz: u32) -> Self {
        Self {
            nominal: 1000. / f64::from(fps_millihz.max(1)),
            previous: None,
            interval: 0.,
            stable: 0.,
            blocks: HashMap::new(),
        }
    }
    pub fn observe(&mut self, frame: &[u8], now: Instant, kbps: u32) -> Result<(usize, usize)> {
        let elapsed = self
            .previous
            .replace(now)
            .map(|p| now.saturating_duration_since(p).as_secs_f64())
            .unwrap_or(0.);
        if elapsed <= self.nominal * 1.01 {
            self.blocks.clear();
            self.stable = 0.;
            self.interval = elapsed;
            return Ok((0, 0));
        }
        let records = records(frame)?;
        let mut next = HashMap::with_capacity(records.len());
        let (mut total, mut unchanged) = (0, 0);
        for r in records {
            let hash = record_hash(&frame[r.offset..r.offset + r.size]);
            if self.blocks.get(&r.index) == Some(&(hash, r.size)) {
                unchanged += r.size;
            }
            total += r.size;
            next.insert(r.index, (hash, r.size));
        }
        self.blocks = next;
        self.interval = if self.interval > 0. {
            self.interval * 0.8 + elapsed * 0.2
        } else {
            elapsed
        };
        if total == 0 || unchanged * 4 < total * 3 {
            self.stable = 0.;
            return Ok((0, 0));
        }
        self.stable += elapsed;
        if self.stable < 0.25 {
            return Ok((0, 0));
        }
        Ok((
            (100. * (self.interval / self.nominal - 1.)).clamp(0., 50.) as usize,
            (f64::from(kbps) * 125. * elapsed.min(self.interval).min(0.05)) as usize,
        ))
    }
}
#[cfg(test)]
mod tests;

#[cfg(test)]
mod rows_tests;

#[cfg(test)]
mod framing_tests;
