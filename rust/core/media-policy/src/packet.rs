//! Moonlight's wire framing: video packetization with Reed-Solomon FEC,
//! PyroWave FEC, audio packets, encrypted control packets and replay
//! protection.
use anyhow::{Context, Result, bail};
use std::{borrow::Cow, cell::RefCell, sync::Arc};

use crate::pyrowave::{Payload, PayloadCursor};
use butterpollo_crypto::crypto;

mod audio;
mod inline_blocks;
pub use audio::AudioPacketizer;
pub use inline_blocks::InlineBlocks;
pub use rubylight_wire::crypto::CbcKey;
pub use rubylight_wire::mic::open_into as mic_open_into;
pub use rubylight_wire::mic::open_single_into as mic_open_single_into;
mod cauchy;
mod streamed;
#[cfg(test)]
use cauchy::{AFFINE, INVERSES, cauchy_encode_with, gf_mul, inverse};
use cauchy::{axpy, cauchy_encode_offset};
pub use cauchy::{cauchy_encode, gfni_active, use_gfni};
pub use streamed::{SliceOverflow, StreamedBlock, StreamedFrame};

thread_local! {
    /// Cached per-thread video packet sealer keyed by the 128-bit session key.
    static VIDEO_SEALER: RefCell<Option<([u8; 16], Arc<crypto::PacketSealer>)>> =
        const { RefCell::new(None) };
}

fn cached_sealer(key: &[u8; 16]) -> Arc<crypto::PacketSealer> {
    VIDEO_SEALER.with(|cell| {
        let mut cache = cell.borrow_mut();
        if let Some((k, s)) = cache.as_ref()
            && crypto::equal(k, key)
        {
            return Arc::clone(s);
        }
        let sealer = Arc::new(crypto::PacketSealer::new(key));
        *cache = Some((*key, Arc::clone(&sealer)));
        sealer
    })
}

/// Datagram storage for one video FEC block.
pub type PacketBlock = Vec<Vec<u8>>;

/// A pooled FEC block alongside its count of filled packets in `0..=block.len()`.
pub type PooledBlock = (PacketBlock, usize);

/// Blocks the paced sender has sent, for the packetizer to fill again instead
/// of allocating every packet of every frame. First in, first out: a frame's
/// blocks come back in order, so the next frame's block takes the buffer of
/// the same block before it, which already has its size.
#[derive(Default)]
pub struct BlockPool(std::sync::Mutex<std::collections::VecDeque<PacketBlock>>);

impl BlockPool {
    /// Blocks kept; more are freed. A large PyroWave frame takes about a dozen.
    const KEEP: usize = 16;

    pub fn take(&self) -> PacketBlock {
        self.0.lock().unwrap().pop_front().unwrap_or_default()
    }

    pub fn give(&self, block: PacketBlock) {
        let mut blocks = self.0.lock().unwrap();
        if blocks.len() < Self::KEEP {
            blocks.push_back(block);
            return;
        }
        drop(blocks);
        drop(block);
    }

    /// A block taken but not filled goes back first in line.
    fn unused(&self, block: PacketBlock) {
        self.0.lock().unwrap().push_front(block);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }
}

pub use rubylight_wire::packet::{
    ControlCipher, ControlCiphers, LegacyInput, ReplayWindow, control_header, decrypt_control,
    encrypted_control,
};
pub fn concat_and_insert(header: usize, slice: usize, a: &[u8], b: &[u8]) -> Result<Vec<u8>> {
    if slice == 0 {
        return Ok(vec![]);
    }
    let total = a
        .len()
        .checked_add(b.len())
        .context("packet payload overflow")?;
    if total == 0 {
        return Ok(vec![]);
    }
    let count = total.div_ceil(slice);
    let len = header
        .checked_mul(count)
        .and_then(|h| h.checked_add(total))
        .context("packet buffer overflow")?;
    let mut out = Vec::new();
    out.try_reserve_exact(len)?;
    out.resize(len, 0);
    let mut source = 0;
    for i in 0..count {
        let n = slice.min(total - source);
        let dst = i * (header + slice) + header;
        let an = n.min(a.len().saturating_sub(source));
        if an > 0 {
            out[dst..dst + an].copy_from_slice(&a[source..source + an]);
        }
        if an < n {
            let start = (source + an).saturating_sub(a.len());
            out[dst + an..dst + n].copy_from_slice(&b[start..start + n - an]);
        }
        source += n;
    }
    Ok(out)
}
#[derive(Clone)]
pub struct VideoPacketizer {
    /// Moonlight uses a 24-bit stream index alongside the 16-bit RTP sequence.
    pub sequence: u32,
    pub iv_counter: u64,
    pub frame: u32,
    pub packet_size: usize,
    pub fec_percent: usize,
    pub min_fec: usize,
    pub key: Option<[u8; 16]>,
}
pub struct PyrowaveFec {
    pub records: bool,
    pub critical_percentage: usize,
    pub detail_percentage: usize,
    pub wire_budget: usize,
    pub ipv6: bool,
}
/// FEC blocks and parity percentage for a frame of `total_shards`. Moonlight
/// takes at most four blocks of 255 shards each. A frame too large for the
/// requested percentage keeps as much parity as still fits in four blocks
/// (with at least `minimum` parity shards per block) instead of none: at
/// 4K60 and 80 Mbps large P-frames lost all protection just past the cutoff.
pub fn conventional_fec(total_shards: usize, percentage: usize, minimum: usize) -> (usize, usize) {
    let blocks = total_shards.div_ceil(255 * 100 / (100 + percentage));
    let fits = |blocks: usize, percentage| {
        let aligned = total_shards.div_ceil(blocks);
        (0..blocks).all(|block| {
            let data = total_shards.saturating_sub(block * aligned).min(aligned);
            data + block_parity(data, percentage, minimum).0 <= 255
        })
    };
    // A negotiated minimum can need more blocks than the configured percentage.
    for blocks in blocks.max(1)..=4 {
        if fits(blocks, percentage) {
            return (blocks, percentage);
        }
    }
    let per_block = total_shards.div_ceil(4);
    let room = 255usize.saturating_sub(per_block);
    if percentage == 0 || room == 0 || room < minimum {
        return (4, 0);
    }
    // ceil(per_block * fitted / 100) <= room for every block.
    let fitted = (100 * room / per_block).min(percentage);
    (4, if fits(4, fitted) { fitted } else { 0 })
}
/// Parity shards for a block of `count` data shards, and the percentage the
/// client is told. A block below the minimum parity is raised to it.
fn block_parity(count: usize, percentage: usize, minimum: usize) -> (usize, usize) {
    crate::pyrowave::fec_parameters(count, percentage, minimum)
}
/// The conventional FEC plan for one frame: its blocks and the percentage
/// every block carries. Computed once per frame so the block layout, the
/// FEC-limited flag and the block fill share it instead of running
/// [`conventional_fec`] again each.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConventionalPlan {
    /// Data and parity shards of each block, in send order, as
    /// [`VideoPacketizer::block_layout`] returns them.
    pub layout: InlineBlocks<(usize, usize)>,
    /// The percentage every block carries; below `fec_percent` when limited.
    pub percentage: usize,
}
impl ConventionalPlan {
    /// Whether parity was reduced below `fec_percent`.
    pub fn limited(&self, fec_percent: usize) -> bool {
        self.percentage < fec_percent
    }
}
impl VideoPacketizer {
    fn sealer(&self) -> Option<Arc<crypto::PacketSealer>> {
        self.key.as_ref().map(cached_sealer)
    }
    /// Data and parity shards of each FEC block of an H.264, HEVC or AV1
    /// frame of `payload_bytes`, in send order, as `encode` frames it. A block
    /// loses the frame once more of its packets are lost than it has parity.
    pub fn block_layout(&self, payload_bytes: usize) -> Vec<(usize, usize)> {
        self.conventional_plan(payload_bytes).layout.to_vec()
    }
    /// The conventional FEC plan of a frame of `payload_bytes`, computed once
    /// so the block layout, the FEC-limited flag and the block fill share it.
    pub fn conventional_plan(&self, payload_bytes: usize) -> ConventionalPlan {
        let slice = self.packet_size + 16 - 32;
        let total_shards = (payload_bytes + 8).div_ceil(slice);
        let (blocks, percentage) = conventional_fec(total_shards, self.fec_percent, self.min_fec);
        let aligned = total_shards.div_ceil(blocks.max(1));
        let layout = (0..blocks)
            .map(|block| {
                let data = total_shards.saturating_sub(block * aligned).min(aligned);
                (data, block_parity(data, percentage, self.min_fec).0)
            })
            .filter(|(data, _)| *data > 0)
            .collect();
        ConventionalPlan { layout, percentage }
    }
    pub fn fec_limited(&self, payload_bytes: usize) -> bool {
        let shards = (payload_bytes + 8).div_ceil(self.packet_size - 16);
        conventional_fec(shards, self.fec_percent, self.min_fec).1 < self.fec_percent
    }
    /// The largest payload that still gets the full `fec_percent` within
    /// Moonlight's four blocks ([`Self::fec_limited`] false). A larger frame
    /// gets less parity, and above about 1.40 MB at 1392-byte packets none.
    pub fn fec_whole_payload_limit(&self) -> usize {
        let slice = self.packet_size - 16;
        let shards = (1..=4 * 255)
            .rev()
            .find(|&shards| {
                conventional_fec(shards, self.fec_percent, self.min_fec).1 >= self.fec_percent
            })
            .unwrap_or(1);
        (shards * slice).saturating_sub(8)
    }

    pub fn encode(
        &mut self,
        payload: &[u8],
        idr: bool,
        timestamp: u32,
        latency_us: u64,
    ) -> Result<Vec<Vec<u8>>> {
        self.encode_recovery(payload, idr, false, timestamp, latency_us)
    }
    pub fn encode_recovery(
        &mut self,
        payload: &[u8],
        idr: bool,
        after_invalidation: bool,
        timestamp: u32,
        latency_us: u64,
    ) -> Result<Vec<Vec<u8>>> {
        self.encode_frame(
            payload.into(),
            idr,
            after_invalidation,
            timestamp,
            latency_us,
            None,
        )
    }
    /// `payload` is the framed picture: contiguous bytes, or the pieces of the
    /// encoder's bitstream it was framed from ([`crate::pyrowave::Framing`]),
    /// which are copied straight into the packets.
    pub fn encode_pyrowave<'a>(
        &mut self,
        payload: impl Into<Payload<'a>>,
        timestamp: u32,
        latency_us: u64,
        fec: PyrowaveFec,
    ) -> Result<Vec<Vec<u8>>> {
        self.encode_frame(
            payload.into(),
            true,
            false,
            timestamp,
            latency_us,
            Some(fec),
        )
    }
    /// Prepare each FEC block only when the sender needs it. Earlier blocks
    /// can leave while later blocks still need framing, parity and encryption.
    /// Dropping the iterator discards the unsent remainder of this frame.
    pub fn pyrowave_blocks<'a>(
        &'a mut self,
        payload: impl Into<Payload<'a>>,
        timestamp: u32,
        latency_us: u64,
        fec: PyrowaveFec,
    ) -> Result<(usize, bool, impl Iterator<Item = Result<PacketBlock>> + 'a)> {
        self.pyrowave_blocks_in(payload, timestamp, latency_us, fec, None)
    }
    /// [`Self::pyrowave_blocks`], building each block in a buffer from `pool`
    /// when there is one, so a sent block's packets are filled again.
    pub fn pyrowave_blocks_in<'a>(
        &'a mut self,
        payload: impl Into<Payload<'a>>,
        timestamp: u32,
        latency_us: u64,
        fec: PyrowaveFec,
        pool: Option<&'a BlockPool>,
    ) -> Result<(usize, bool, impl Iterator<Item = Result<PacketBlock>> + 'a)> {
        let (count, limited, blocks) =
            self.pyrowave_blocks_pooled(payload, timestamp, latency_us, fec, pool)?;
        Ok((
            count,
            limited,
            blocks.map(|item| {
                item.map(|(mut packets, filled)| {
                    packets.truncate(filled);
                    packets
                })
            }),
        ))
    }
    /// [`Self::pyrowave_blocks_in`] returning `(PacketBlock, valid_count)`
    /// without truncating `packets`, so pooled shard buffers are preserved
    /// across alternating critical and detail block sizes.
    pub fn pyrowave_blocks_pooled<'a>(
        &'a mut self,
        payload: impl Into<Payload<'a>>,
        timestamp: u32,
        latency_us: u64,
        fec: PyrowaveFec,
        pool: Option<&'a BlockPool>,
    ) -> Result<(usize, bool, impl Iterator<Item = Result<PooledBlock>> + 'a)> {
        let (count, limited, mut next) = self.frame_blocks(
            payload.into(),
            true,
            false,
            timestamp,
            latency_us,
            Some(fec),
            None,
        )?;
        let blocks = std::iter::from_fn(move || {
            let mut packets = pool.map(BlockPool::take).unwrap_or_default();
            match next(&mut packets) {
                Ok(0) => {
                    if let Some(pool) = pool {
                        pool.unused(packets);
                    }
                    None
                }
                Ok(filled) => Some(Ok((packets, filled))),
                Err(error) => {
                    if let Some(pool) = pool {
                        pool.give(packets);
                    }
                    Some(Err(error))
                }
            }
        });
        Ok((count, limited, blocks))
    }
    /// Build the next conventional FEC block in reusable shard buffers. The
    /// returned count selects its packets; zero means the frame is finished.
    /// Later blocks are never built if the sender abandons this frame.
    pub fn conventional_blocks<'a>(
        &'a mut self,
        payload: &'a [u8],
        idr: bool,
        after_invalidation: bool,
        timestamp: u32,
        latency_us: u64,
    ) -> Result<(usize, impl FnMut(&mut PacketBlock) -> Result<usize> + 'a)> {
        let (count, _, next) = self.frame_blocks(
            payload.into(),
            idr,
            after_invalidation,
            timestamp,
            latency_us,
            None,
            None,
        )?;
        Ok((count, next))
    }
    /// [`conventional_blocks`](Self::conventional_blocks) with the plan
    /// [`conventional_plan`](Self::conventional_plan) already computed for
    /// this frame, so the block fill does not run `conventional_fec` again.
    pub fn conventional_blocks_planned<'a>(
        &'a mut self,
        payload: &'a [u8],
        idr: bool,
        after_invalidation: bool,
        timestamp: u32,
        latency_us: u64,
        plan: &ConventionalPlan,
    ) -> Result<(usize, impl FnMut(&mut PacketBlock) -> Result<usize> + 'a)> {
        let (count, _, next) = self.frame_blocks(
            payload.into(),
            idr,
            after_invalidation,
            timestamp,
            latency_us,
            None,
            Some(plan),
        )?;
        Ok((count, next))
    }
    fn encode_frame(
        &mut self,
        payload: Payload,
        idr: bool,
        after_invalidation: bool,
        timestamp: u32,
        latency_us: u64,
        pyrowave: Option<PyrowaveFec>,
    ) -> Result<Vec<Vec<u8>>> {
        let (count, _, mut next) = self.frame_blocks(
            payload,
            idr,
            after_invalidation,
            timestamp,
            latency_us,
            pyrowave,
            None,
        )?;
        let mut packets = Vec::with_capacity(count);
        loop {
            let mut block = Vec::new();
            if next(&mut block)? == 0 {
                break;
            }
            packets.extend(block);
        }
        Ok(packets)
    }
    #[allow(clippy::too_many_arguments)]
    fn frame_blocks<'a>(
        &'a mut self,
        payload: Payload<'a>,
        idr: bool,
        after_invalidation: bool,
        timestamp: u32,
        latency_us: u64,
        pyrowave: Option<PyrowaveFec>,
        conventional: Option<&ConventionalPlan>,
    ) -> Result<(
        usize,
        bool,
        impl FnMut(&mut PacketBlock) -> Result<usize> + 'a,
    )> {
        if !(256..=usize::from(rubylight_protocol::video_packet::MAX_VIDEO_PACKET))
            .contains(&self.packet_size)
            || self.fec_percent > 100
        {
            bail!("invalid packetizer parameters");
        }
        let block_size = self.packet_size + 16;
        let slice = block_size - 32;
        let payload_len = payload.len();
        if payload_len == 0 {
            bail!("encoded frame is empty");
        }
        if payload_len > slice * 4092 - 8 {
            // The dropped frame still takes its index: Moonlight notices a
            // lost frame only from the gap, and would otherwise decode the
            // next frames against a picture it never received.
            self.frame = self.frame.wrapping_add(1);
            bail!("encoded frame exceeds Moonlight packet limit");
        }
        let total = payload_len + 8;
        let total_shards = total.div_ceil(slice);
        let aligned = crate::pyrowave::aligned_payload(self.packet_size);
        // A framed picture carries the layout its framing pass computed; one
        // that arrives as plain bytes is parsed.
        let layout: Option<Cow<'a, crate::pyrowave::Layout>> = match payload.stored_layout(aligned)
        {
            Some(stored) if pyrowave.as_ref().is_some_and(|p| p.records) => {
                Some(Cow::Borrowed(stored))
            }
            _ => pyrowave
                .as_ref()
                .filter(|p| p.records)
                .map(|_| crate::pyrowave::layout(payload, aligned).map(Cow::Owned))
                .transpose()?,
        };
        let critical = layout.as_ref().map_or(0, |l| l.critical_shards);
        let minimum = if pyrowave.is_some() {
            self.min_fec.max(2)
        } else {
            self.min_fec
        };
        let mut header = [0u8; 8];
        header[0] = 1;
        header[1..3].copy_from_slice(&((latency_us / 100).min(65535) as u16).to_le_bytes());
        header[3] = if idr {
            2
        } else if after_invalidation {
            5
        } else {
            1
        };
        let last = total % slice;
        header[4..6]
            .copy_from_slice(&((if last == 0 { slice } else { last }) as u16).to_le_bytes());
        header[6..8].copy_from_slice(&(critical as u16).to_le_bytes());
        if pyrowave.as_ref().is_some_and(|pyro| {
            total_shards > 4000 || pyro.critical_percentage > 255 || minimum > 255
        }) {
            bail!("invalid PyroWave FEC plan");
        }
        let plan: InlineBlocks<crate::pyrowave::FecBlock> = if let Some(pyro) = pyrowave
            .as_ref()
            .filter(|pyro| pyro.records || pyro.critical_percentage == 0)
        {
            let baseline = crate::pyrowave::plan_inline(
                total_shards,
                critical,
                pyro.critical_percentage,
                minimum,
                0,
                0,
            )?;
            let baseline_packets = total_shards
                + baseline
                    .iter()
                    .map(|b| crate::pyrowave::parity(b.data, b.percentage, minimum))
                    .sum::<usize>();
            let wire_bytes = block_size
                + 38
                + if pyro.ipv6 { 40 } else { 20 }
                + 8
                + if self.key.is_some() { 32 } else { 0 };
            let extra = (pyro.wire_budget / wire_bytes).saturating_sub(baseline_packets);
            crate::pyrowave::plan_inline(
                total_shards,
                critical,
                pyro.critical_percentage,
                minimum,
                pyro.detail_percentage,
                extra,
            )?
        } else if let Some(plan) = conventional {
            // The precomputed plan: the block fill, block layout and
            // FEC-limited flag share it instead of recomputing it here.
            plan.layout
                .iter()
                .map(|&(data, _)| crate::pyrowave::FecBlock {
                    data,
                    percentage: plan.percentage,
                })
                .collect()
        } else {
            // Legacy PyroWave's length container still uses Moonlight's normal
            // RTP/FEC layer. Only the selective coarse/detail scheme requires
            // record framing. Keep ordinary parity wherever four RS blocks fit.
            let requested = pyrowave
                .as_ref()
                .map_or(self.fec_percent, |pyro| pyro.critical_percentage);
            let (blocks, percentage) = conventional_fec(total_shards, requested, minimum);
            let aligned = total_shards.div_ceil(blocks);
            (0..blocks)
                .map(|block| crate::pyrowave::FecBlock {
                    data: total_shards.saturating_sub(block * aligned).min(aligned),
                    percentage,
                })
                .collect()
        };
        let fec_limited = pyrowave.as_ref().is_some_and(|p| {
            p.critical_percentage > 0
                && if p.records {
                    plan.iter().all(|block| block.percentage == 0)
                } else {
                    plan.iter()
                        .any(|block| block.percentage < p.critical_percentage)
                }
        });
        let blocks = plan.len();
        let sealer = self.sealer();
        let packet_count = plan
            .iter()
            .map(|b| b.data + crate::pyrowave::parity(b.data, b.percentage, minimum))
            .sum();
        let mut first_shard = 0;
        let frame = self.frame;
        let mut plan = plan.into_iter().enumerate();
        let mut cursor = payload.cursor();
        let next = move |shards: &mut PacketBlock| {
            let Some((block, planned)) = plan.next() else {
                return Ok(0);
            };
            if first_shard >= total_shards {
                bail!("invalid FEC block split");
            }
            let count = planned.data.min(total_shards - first_shard);
            let filled = self.fill_block(
                &BlockFill {
                    block,
                    blocks,
                    first_shard,
                    count,
                    percentage: planned.percentage,
                    minimum,
                    header: &header,
                    layout: layout.as_deref(),
                    sealer: sealer.as_deref(),
                    timestamp,
                    frame,
                },
                &mut cursor,
                shards,
            )?;
            first_shard += count;
            Ok(filled)
        };
        Ok((packet_count, fec_limited, next))
    }
    /// Build one FEC block, the shards `first_shard..first_shard + count` of
    /// the frame's byte stream (`header`, then `cursor`), with its parity,
    /// and advance the packet and nonce counters. Returns the packets in
    /// `shards`. The frame index advances once block 0 is built.
    fn fill_block(
        &mut self,
        spec: &BlockFill,
        cursor: &mut PayloadCursor<'_>,
        shards: &mut PacketBlock,
    ) -> Result<usize> {
        let BlockFill {
            block,
            blocks,
            first_shard,
            count,
            percentage,
            minimum,
            header,
            layout,
            sealer,
            timestamp,
            frame,
        } = *spec;
        let block_size = self.packet_size + 16;
        let slice = block_size - 32;
        let payload_len = cursor.len();
        let envelope = if sealer.is_some() { 32 } else { 0 };
        if count >= 1024 {
            bail!("FEC shard index overflow");
        }
        let (fec, effective) = block_parity(count, percentage, minimum);
        if count + fec > 255 && percentage > 0 {
            bail!("FEC parity count exceeds protocol limit");
        }
        if shards.len() < count + fec {
            shards.resize_with(count + fec, Vec::new);
        }
        let shards = &mut shards[..count + fec];
        // A reused buffer is not cleared as a whole: every byte is written
        // below. Data shards clear only the header bytes nothing writes and
        // the tail past the payload, the encoder clears the parity rows and
        // the sealer fills the envelope.
        for shard in shards.iter_mut() {
            shard.resize(envelope + block_size, 0);
        }
        for (i, shard) in shards.iter_mut().take(count).enumerate() {
            let s = &mut shard[envelope..];
            // The header bytes the loops below never write: the RTP payload
            // type, the SSRC and the reserved byte stay zero.
            s[1] = 0;
            s[8..16].fill(0);
            s[25] = 0;
            // Place the short header and source payload directly into their
            // final shards instead of allocating/copying a full packed frame.
            let offset = (first_shard + i) * slice;
            let h = header.len().saturating_sub(offset).min(slice);
            if h > 0 {
                s[32..32 + h].copy_from_slice(&header[offset..offset + h]);
            }
            let begin = offset.saturating_sub(header.len());
            let n = (slice - h).min(payload_len - begin);
            cursor.copy_to(begin, &mut s[32 + h..32 + h + n]);
            s[32 + h + n..].fill(0);
            s[16..20].copy_from_slice(&(self.sequence.wrapping_add(i as u32) << 8).to_le_bytes());
            s[24] = 1 | if i == 0 { 4 } else { 0 } | if i == count - 1 { 2 } else { 0 };
            s[26] = 0x10;
            if layout.is_some_and(|l| l.starts.get(first_shard + i) == Some(&true)) {
                s[26] |= 0x80;
            }
        }
        if fec > 0 {
            if envelope == 0 {
                cauchy_encode(shards, count, fec)?;
            } else {
                cauchy_encode_offset::<32>(shards, count, fec)?;
            }
        }
        for (i, shard) in shards.iter_mut().enumerate() {
            let s = &mut shard[envelope..];
            let seq = self.sequence.wrapping_add(i as u32) as u16;
            s[0] = 0x90;
            s[2..4].copy_from_slice(&seq.to_be_bytes());
            s[4..8].copy_from_slice(&timestamp.to_be_bytes());
            s[20..24].copy_from_slice(&frame.to_le_bytes());
            s[27] = ((block as u8) << 4) | (((blocks - 1) as u8) << 6);
            let info = ((i as u32) << 12) | ((count as u32) << 22) | ((effective as u32) << 4);
            s[28..32].copy_from_slice(&info.to_le_bytes());
            if let Some(sealer) = sealer {
                let mut iv = [0; 12];
                iv[..8].copy_from_slice(&self.iv_counter.to_le_bytes());
                iv[11] = b'V';
                self.iv_counter = self
                    .iv_counter
                    .checked_add(1)
                    .context("video nonce exhausted")?;
                // FEC excludes the reserved envelope. Its owned payload is
                // now encrypted in place with no extra allocation or move.
                let tag = sealer.seal(&iv, s)?;
                shard[..12].copy_from_slice(&iv);
                shard[12..16].copy_from_slice(&frame.to_le_bytes());
                shard[16..32].copy_from_slice(&tag);
            }
        }
        self.sequence = self.sequence.wrapping_add((count + fec) as u32);
        // Packets can escape now, even if the sender abandons later blocks.
        if block == 0 {
            self.frame = self.frame.wrapping_add(1);
        }
        Ok(count + fec)
    }
}
/// One FEC block of a frame: where it sits and what it is built from.
#[derive(Clone, Copy)]
struct BlockFill<'a> {
    block: usize,
    /// Blocks in the whole frame, announced on every packet.
    blocks: usize,
    first_shard: usize,
    count: usize,
    percentage: usize,
    minimum: usize,
    header: &'a [u8; 8],
    layout: Option<&'a crate::pyrowave::Layout>,
    sealer: Option<&'a crypto::PacketSealer>,
    timestamp: u32,
    frame: u32,
}
#[cfg(test)]
mod opt_tests;
#[cfg(test)]
mod streamed_tests;
#[cfg(test)]
mod tests;
