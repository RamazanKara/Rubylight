//! Vibepollo 2.0 PyroWave framing, partial recovery and transport budgets.
use anyhow::{Context, Result, bail};
use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};
pub const BITSTREAM_ID: &str = "186f0393";
pub const PACKET_BOUNDARY: usize = 1024;
const PADDING: u32 = u32::MAX;
fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
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
    if frame.len() < 8
        || !frame.len().is_multiple_of(4)
        || word(frame, 0) & 0x80000000 == 0
        || word(frame, 0) == PADDING
        || (word(frame, 4) >> 24) & 3 != 0
    {
        bail!("invalid PyroWave sequence header");
    }
    let sequence = (word(frame, 0) >> 28) & 7;
    let mut records = Vec::new();
    let mut offset = 8;
    while offset < frame.len() {
        if frame.len() - offset < 8 {
            bail!("short PyroWave record");
        }
        let header = word(frame, offset);
        let padding = header == PADDING;
        let size = if padding {
            8usize
                .checked_add(word(frame, offset + 4) as usize * 4)
                .context("padding overflow")?
        } else {
            (((header >> 16) & 0xfff) as usize) * 4
        };
        if size < 8
            || size > frame.len() - offset
            || (!padding && (header & 0x80000000 != 0 || (header >> 28) & 7 != sequence))
        {
            bail!("invalid PyroWave block record");
        }
        if !padding {
            records.push(Record {
                offset,
                size,
                index: word(frame, offset + 4) >> 8,
            });
        }
        offset += size;
    }
    if records.len() != (word(frame, 4) & 0xffffff) as usize {
        bail!("PyroWave record count mismatch");
    }
    Ok(records)
}
fn coarse_count(frame: &[u8]) -> u32 {
    let extent = |n: u32| n.div_ceil(32).max(4).div_ceil(32);
    let header = word(frame, 0);
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
pub fn record_frame(bitstream: &[u8], payload: usize, limit: usize) -> Result<Vec<u8>> {
    let all = records(bitstream)?;
    if bitstream.len() > limit {
        bail!("PyroWave frame exceeds transport capacity");
    }
    if payload == 0 {
        return Ok(bitstream.to_vec());
    }
    let coarse = coarse_count(bitstream);
    let mut out = Vec::with_capacity(bitstream.len());
    out.extend_from_slice(&bitstream[..8]);
    for required in [true, false] {
        let mut ordinary = Vec::new();
        for r in all.iter().filter(|r| (r.index < coarse) == required) {
            if r.size > payload - 8 {
                let room = payload - (out.len() + 8) % payload;
                if r.size % payload == room - 4 {
                    padding(&mut out, 8);
                }
                out.extend_from_slice(&bitstream[r.offset..r.offset + r.size]);
            } else {
                ordinary.push(*r);
            }
        }
        let mut fits = Fits::new(&ordinary);
        for _ in 0..ordinary.len() {
            loop {
                let room = payload - (out.len() + 8) % payload;
                let mut chosen = fits.fitting(room / 4);
                if chosen != usize::MAX && room - ordinary[chosen].size == 4 {
                    chosen = fits.exact(room / 4).min(if room >= 16 {
                        fits.fitting((room - 8) / 4)
                    } else {
                        usize::MAX
                    });
                }
                if chosen == usize::MAX {
                    padding(&mut out, room);
                    continue;
                }
                let r = ordinary[chosen];
                fits.remove(r.size / 4);
                out.extend_from_slice(&bitstream[r.offset..r.offset + r.size]);
                break;
            }
        }
    }
    if out.len() > limit {
        return Ok(bitstream.to_vec());
    }
    Ok(out)
}
pub struct Layout {
    pub critical_shards: usize,
    pub starts: Vec<bool>,
}
pub fn layout(frame: &[u8], payload: usize) -> Result<Layout> {
    let all = records(frame)?;
    let mut layout = Layout {
        critical_shards: 0,
        starts: vec![false; (frame.len() + 8).div_ceil(payload.max(1))],
    };
    if payload == 0 {
        return Ok(layout);
    }
    let coarse = coarse_count(frame);
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
    while offset < frame.len() {
        if offset == 0 || (offset + 8).is_multiple_of(payload) {
            layout.starts[(offset + 8) / payload] = true;
        }
        let header = word(frame, offset);
        offset += if header == PADDING {
            8 + word(frame, offset + 4) as usize * 4
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
mod tests {
    use super::*;
    fn frame(records: &[(u32, usize)]) -> Vec<u8> {
        let mut out = vec![];
        out.extend_from_slice(&(0x80000000u32 | 1919 | (1079 << 14)).to_le_bytes());
        out.extend_from_slice(&(records.len() as u32).to_le_bytes());
        for &(index, size) in records {
            out.extend_from_slice(&((size as u32 / 4) << 16).to_le_bytes());
            out.extend_from_slice(&(index << 8).to_le_bytes());
            out.resize(out.len() + size - 8, 42);
        }
        out
    }
    #[test]
    fn minimum_parity_rounding_keeps_every_planned_block_representable() {
        for total in [230, 231, 460, 462, 600, 800] {
            for critical in [1, 2, 201] {
                for minimum in [24, 42] {
                    for extra in [0, 50, 200] {
                        let blocks = plan(total, critical, 20, minimum, 50, extra).unwrap();
                        assert_eq!(blocks.iter().map(|b| b.data).sum::<usize>(), total);
                        assert!(blocks.len() <= 4);
                        for block in blocks {
                            let (parity, percentage) =
                                fec_parameters(block.data, block.percentage, minimum);
                            assert_eq!(parity, (block.data * percentage).div_ceil(100));
                            assert!(block.data <= 1023);
                            assert!(percentage == 0 || block.data + parity <= 255);
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn quality_bitrates_scale_with_pixels_and_frame_rate() {
        assert_eq!(minimum_kbps(1280, 720, 60_000), 138_240);
        assert_eq!(minimum_kbps(1920, 1080, 60_000), 186_624);
        assert_eq!(minimum_kbps(3840, 2160, 60_000), 746_496);
        assert_eq!(recommended_kbps(1280, 720, 60_000), 276_480);
        assert_eq!(recommended_kbps(1920, 1080, 60_000), 398_132);
        assert_eq!(recommended_kbps(3840, 2160, 60_000), 1_592_525);
        assert_eq!(minimum_kbps(1920, 1080, 30_000), 93_312);
        assert_eq!(minimum_kbps(1280, 720, 120_000), 276_480);
        assert_eq!(recommended_kbps(1280, 720, 30_000), 138_240);
        assert_eq!(recommended_kbps(1920, 1080, 120_000), 796_263);
        assert_eq!(minimum_kbps(3840, 2160, 59_940), 745_750);
        assert_eq!(recommended_kbps(3840, 2160, 59_940), 1_590_933);
    }
    #[test]
    fn quality_bitrates_round_up_and_saturate() {
        for calculate in [minimum_kbps, recommended_kbps] {
            assert_eq!(calculate(1, 1, 1), 1);
            assert_eq!(calculate(1920, 1080, 0), 0);
            assert_eq!(calculate(0, 720, 60_000), 0);
            assert_eq!(calculate(1920, 0, 60_000), 0);
            assert_eq!(calculate(u32::MAX, u32::MAX, u32::MAX), u32::MAX);
        }
    }
    #[test]
    fn record_hash_ignores_only_the_sequence_bits() {
        let record: Vec<u8> = (0..37u8).collect();
        let base = record_hash(&record);
        let mut sequence = record.clone();
        sequence[3] ^= 0x70;
        assert_eq!(record_hash(&sequence), base);
        for at in [0, 3, 7, 8, 20, 36] {
            let mut changed = record.clone();
            changed[at] ^= if at == 3 { 0x01 } else { 0x80 };
            assert_ne!(record_hash(&changed), base, "byte {at}");
        }
        assert_ne!(record_hash(&record[..36]), base);
        let short = [1u8, 2, 3, 4, 5];
        let mut short_sequence = short;
        short_sequence[3] ^= 0x70;
        assert_eq!(record_hash(&short_sequence), record_hash(&short));
    }
    #[test]
    fn compatibility_matches_clients_and_checks_slices() {
        assert_eq!(
            container(&[10, 11, 12, 13], &[(1, 2), (0, 1)]).unwrap(),
            [2, 0, 0, 0, 2, 0, 0, 0, 11, 12, 1, 0, 0, 0, 10]
        );
        assert!(container(&[1], &[(usize::MAX, 1)]).is_err());
        let cap = max_frame_bytes(1392, false);
        let raw = budget(2_000_000, Duration::from_secs(1), 1392, false, 0);
        assert!(4 + raw + (raw / 8) * 4 <= cap);
        // 833333 wire bytes at 800 Mbps and 120 fps hold 546 packets; one
        // critical block of 212 takes 43 parity.
        assert_eq!(
            budget(800000, Duration::from_secs_f64(1. / 120.), 1392, true, 20),
            692120
        );
    }
    #[test]
    fn legacy_parity_fits_the_existing_length_framing_wire_allowance() {
        let period = Duration::from_secs_f64(1. / 60.);
        for size in [256, 1392] {
            let slice = size - 16;
            for kbps in [2000, 10_000, 80_000, 300_000, 2_000_000] {
                let original = budget(kbps, period, size, false, 0);
                let packets = (12 + original + original / 8 * 4).div_ceil(slice);
                for rate in [20, 100, 255] {
                    for minimum in [2, 24, 255] {
                        let image = budget_with_minimum(kbps, period, size, false, rate, minimum);
                        assert!(image <= original);
                        if image == 0 {
                            continue;
                        }
                        let data = (12 + image + image / 8 * 4).div_ceil(slice);
                        let (blocks, fitted) =
                            crate::packet::conventional_fec(data, rate as usize, minimum);
                        let aligned = data.div_ceil(blocks);
                        let actual = data
                            + (0..blocks)
                                .map(|block| {
                                    parity(
                                        data.saturating_sub(block * aligned).min(aligned),
                                        fitted,
                                        minimum,
                                    )
                                })
                                .sum::<usize>();
                        assert!(
                            actual <= packets,
                            "{size}/{kbps}/{rate}/{minimum}: {actual}>{packets}"
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn record_budgets_reserve_the_negotiated_representable_minimum() {
        let period = Duration::from_secs_f64(1. / 60.);
        for minimum in [2, 24, 44, 255] {
            for rate in [1, 20, 100, 255] {
                let kbps = 80_000;
                let allowance = (f64::from(kbps) * 125. * period.as_secs_f64()) as usize;
                let packets = allowance / (1392 + WIRE_OVERHEAD);
                let image = budget_with_minimum(kbps, period, 1392, true, rate, minimum);
                let data = (image + 8).div_ceil(1376);
                for critical in 1..=data.min(255) {
                    let fec = parity(critical, rate as usize, minimum);
                    if critical + fec <= 255 {
                        assert!(data + fec <= packets);
                    }
                }
            }
        }
    }
    #[test]
    fn record_budget_is_the_wire_rate_less_overhead_and_parity() {
        // The GPU end-to-end test's 40 dB control for 1080p 4:4:4 needs about
        // 3.3 MB of picture. A record budget is a wire budget, so 1 Gbps
        // allows 2.08 MB per frame and leaves 4:4:4 at 42.8/37.3/36.6 dB:
        // that is the rate, not a codec fault. The control runs at 2 Gbps.
        let period = Duration::from_secs_f64(1. / 60.);
        for (kbps, least, most) in [
            (1_000_000, 1_700_000, 1_900_000),
            (2_000_000, 3_500_000, 3_900_000),
        ] {
            let image = budget(kbps, period, 1392, true, 20);
            let wire = (f64::from(kbps) * 125. * period.as_secs_f64()) as usize;
            assert!((least..=most).contains(&image), "{kbps}: {image}");
            assert!(image < wire, "{kbps}: {image} of {wire}");
        }
    }
    #[test]
    fn critical_parity_bound_covers_every_adaptive_percentage() {
        for minimum in [2, 24, 42, 44, 255] {
            for critical in 0..=256 {
                let mut maximum = 0;
                for percentage in 0..=255 {
                    let parity = parity(critical, percentage, minimum);
                    if critical + parity <= 255 {
                        maximum = maximum.max(parity);
                    }
                    assert_eq!(
                        critical_parity_bound(critical, percentage, minimum),
                        maximum,
                        "{critical}/{percentage}/{minimum}"
                    );
                }
            }
        }
    }
    #[test]
    fn a_lower_adaptive_percentage_cannot_exceed_the_reserved_wire_budget() {
        let packet_size = 1392;
        let allowance = 254 * (packet_size + WIRE_OVERHEAD);
        let image = image_bytes_with_minimum(allowance, packet_size, 120, 2);
        let data = (image + 8).div_ceil(packet_size - 16);
        for critical in 1..=data {
            for percentage in 0..=120 {
                let blocks = plan(data, critical, percentage, 2, 0, 0).unwrap();
                let packets: usize = blocks
                    .iter()
                    .map(|block| block.data + parity(block.data, block.percentage, 2))
                    .sum();
                assert!(
                    packets * (packet_size + WIRE_OVERHEAD) <= allowance,
                    "{critical}/{percentage}: {packets} packets"
                );
            }
        }
    }
    #[test]
    fn aligned_frame_cost_reserves_parity_restored_by_adaptation() {
        let original = frame(
            &(0..48)
                .map(|index| (index, 6100))
                .chain((48..98).map(|index| (index, 700)))
                .collect::<Vec<_>>(),
        );
        let packet_size = 1392;
        let aligned = record_frame(
            &original,
            aligned_payload(packet_size),
            max_frame_bytes(packet_size, true),
        )
        .unwrap();
        let critical = layout(&aligned, aligned_payload(packet_size))
            .unwrap()
            .critical_shards;
        assert_eq!(critical, 213);
        let data = (aligned.len() + 8).div_ceil(packet_size - 16);
        let maximum = data + critical_parity_bound(critical, 20, 2);
        let allowance = 437_500;
        assert!(data * (packet_size + WIRE_OVERHEAD) <= allowance);
        assert!(maximum * (packet_size + WIRE_OVERHEAD) > allowance);
        for percentage in 0..=20 {
            let mut packetizer = crate::packet::VideoPacketizer {
                sequence: 0,
                iv_counter: 0,
                frame: 1,
                packet_size,
                fec_percent: 0,
                min_fec: 2,
                key: Some([42; 16]),
            };
            let packets = packetizer
                .encode_pyrowave(
                    &aligned,
                    0,
                    0,
                    crate::packet::PyrowaveFec {
                        records: true,
                        critical_percentage: percentage,
                        detail_percentage: 0,
                        wire_budget: 0,
                        ipv6: true,
                    },
                )
                .unwrap();
            let wire: usize = packets
                .iter()
                .map(|packet| packet.len() + crate::network_pacing::overhead(true))
                .sum();
            assert!(wire <= maximum * (packet_size + WIRE_OVERHEAD));
            if percentage == 19 {
                assert!(wire > allowance);
            }
        }
    }
    #[test]
    fn a_viable_frame_budget_stays_viable_as_bitrate_increases() {
        for records in [false, true] {
            for rate in [1, 20, 100, 255] {
                for minimum in [2, 24, 255] {
                    let mut viable = false;
                    for kbps in (0..10_000).step_by(13) {
                        let image = budget_with_minimum(
                            kbps,
                            Duration::from_secs_f64(1. / 60.),
                            1392,
                            records,
                            rate,
                            minimum,
                        );
                        assert!(!viable || image >= 16, "{records}/{rate}/{minimum}/{kbps}");
                        viable |= image >= 16;
                    }
                }
            }
        }
    }
    /// Nonary's Moonlight calibrates a total wire rate and converts it to a
    /// picture allowance with Vibeshine's `pyrowave_bandwidth.h`; the host
    /// must make the same conversion. Values from that header, compiled.
    #[test]
    fn a_wire_allowance_holds_the_picture_vibeshine_computes() {
        for (wire, percentage, image) in [
            (833_333, 20, 692_120),
            (5_000, 20, 1_368),
            (1526 * 3, 20, 1_368),
            (1526 * 300, 20, 353_624),
            (3_333_333, 20, 2_946_008),
            (100, 20, 0),
            (833_333, 0, 751_288),
            (833_333, 50, 634_328),
        ] {
            assert_eq!(
                image_bytes(wire, 1392, percentage),
                image,
                "{wire} at {percentage}%"
            );
        }
        // 1080p60 and 4K120 at the same total: each frame gets its share.
        assert_eq!(
            budget(500_000, Duration::from_secs_f64(1. / 60.), 1392, true, 20),
            image_bytes(1_041_666, 1392, 20)
        );
        assert_eq!(
            budget(
                2_000_000,
                Duration::from_secs_f64(1. / 120.),
                1392,
                true,
                20
            ),
            image_bytes(2_083_333, 1392, 20)
        );
    }
    #[test]
    fn a_new_picture_after_a_repeat_keeps_a_full_frame_budget() {
        let ms = Duration::from_millis;
        let start = Instant::now();
        let mut interval = Interval::new(ms(10), false);
        assert_eq!(interval.next(start, false), ms(10));
        // A game at 50 fps: each picture pays for its whole gap, as before.
        assert_eq!(interval.next(start + ms(20), false), ms(20));
        // A still picture repeated at 20 fps pays for the time since the last encode.
        assert_eq!(interval.next(start + ms(70), true), ms(50));
        // The game's next picture 2 ms after the repeat gets a whole period, not 2 ms.
        assert_eq!(interval.next(start + ms(72), false), ms(10));
        // One period after the repeat it gets that period.
        assert_eq!(interval.next(start + ms(122), true), ms(50));
        assert_eq!(interval.next(start + ms(134), false), ms(12));
        // Repeats close together stay small: they show nothing new.
        assert_eq!(interval.next(start + ms(136), true), ms(2));

        let mut stable = Interval::new(ms(10), true);
        stable.next(start, false);
        assert_eq!(stable.next(start + ms(50), true), ms(10));
        assert_eq!(stable.next(start + ms(53), false), ms(10));
        assert_eq!(stable.next(start + ms(58), false), ms(5));
    }
    #[test]
    fn aligned_records_keep_coarse_first_and_restart_after_loss() {
        let original = frame(&[(24, 40), (0, 16), (1, 60), (27, 100), (26, 12)]);
        let packed = record_frame(&original, 64, 10000).unwrap();
        assert_eq!(records(&packed).unwrap().len(), 5);
        let l = layout(&packed, 64).unwrap();
        assert!(l.critical_shards >= 2);
        assert!(l.starts[0]);
        assert_eq!(aligned_payload(1390), 0);
        assert!(record_frame(&[0; 8], 64, 10000).is_err());
        let mut bad = original;
        bad[8..12].copy_from_slice(&0x10000u32.to_le_bytes());
        assert!(record_frame(&bad, 64, 10000).is_err());
    }
    #[test]
    fn critical_and_adaptive_parity_obey_wire_and_block_limits() {
        let maximum = plan(4000, 0, 20, 2, 0, 0).unwrap();
        assert_eq!(maximum.len(), 4);
        assert_eq!(maximum.iter().map(|b| b.data).sum::<usize>(), 4000);
        assert!(maximum.iter().all(|b| b.data <= 1023 && b.percentage == 0));
        for total in [1, 100, 800, 2000, 3000] {
            let p = plan(total, 2, 20, 2, 50, 100).unwrap();
            assert_eq!(p.iter().map(|b| b.data).sum::<usize>(), total);
            assert!(p.len() <= 4);
            assert!(
                p.iter()
                    .map(|b| parity(b.data, b.percentage, 2))
                    .sum::<usize>()
                    <= 102
            );
        }
        assert_eq!(
            plan(1000, 0, 20, 2, 0, 0)
                .unwrap()
                .iter()
                .map(|b| parity(b.data, b.percentage, 2))
                .sum::<usize>(),
            0
        );
    }
    #[test]
    fn detail_fec_matches_pre_pipeline_decisions() {
        let mut controller = DetailFec::new(120_000);
        let start = Instant::now();
        let mut elapsed = Duration::ZERO;
        for i in 0..80u32 {
            let order = if i % 2 == 0 {
                [0, 30, 31, 32]
            } else {
                [32, 31, 0, 30]
            };
            let mut raw = frame(&order.map(|index| (index, 20)));
            raw[3] |= ((i % 8) << 4) as u8;
            for (at, index) in order.into_iter().enumerate() {
                let offset = 8 + at * 20;
                raw[offset + 3] |= ((i % 8) << 4) as u8;
                if ((20..40).contains(&i) && index == 30) || ((40..50).contains(&i) && index >= 31)
                {
                    raw[offset + 8..offset + 20].fill(i as u8);
                }
            }
            elapsed += Duration::from_millis(match i {
                60 => 8,
                79 => 100,
                _ => 20,
            });
            // Captured with the ordered map: sequence changes and record order
            // do not alter stability, while motion and nominal cadence reset it.
            let expected = match i {
                14..40 => (50, 2_000_000),
                74 => (50, 1_947_223),
                75 => (50, 1_957_778),
                76 => (50, 1_966_223),
                77 => (50, 1_972_978),
                78 => (50, 1_978_382),
                79 => (50, 3_582_706),
                _ => (0, 0),
            };
            assert_eq!(
                controller.observe(&raw, start + elapsed, 800_000).unwrap(),
                expected,
                "frame {i}"
            );
        }
    }
    #[test]
    fn low_cadence_static_detail_gets_parity_and_motion_disables_it() {
        let mut controller = DetailFec::new(120000);
        let start = Instant::now();
        let same = frame(&[(0, 16), (30, 20)]);
        let mut decision = (0, 0);
        for i in 0..20 {
            decision = controller
                .observe(&same, start + Duration::from_millis(i * 20), 800000)
                .unwrap();
        }
        assert_eq!(decision.0, 50);
        assert!(decision.1 <= 2_000_000);
        assert_eq!(
            controller
                .observe(
                    &frame(&[(0, 20), (30, 24)]),
                    start + Duration::from_millis(400),
                    800000
                )
                .unwrap(),
            (0, 0)
        );
    }
}
