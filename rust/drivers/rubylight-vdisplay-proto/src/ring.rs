//! The frame push: shared GPU textures plus a small header section, one event.
//!
//! The ring is a *fence ring* ([`RING_VERSION_FENCE`], [`crate::control::SetFenceRingRequest`],
//! present when the driver reports [`crate::control::CAP_FENCE_RING`]). The host creates
//! `slot_count` plain shared textures, a section of [`SECTION_BYTES`], an event and two shared
//! fences, *written* and *released*. Who owns a slot's pixels is the slot's state word
//! ([`SlotState`]), changed only by compare-and-swap. Per composed frame:
//!
//! 1. The driver picks a slot ([`Picker::claim_fenced`]): a [`SlotState::Free`] one whose
//!    release value the released fence has reached, else the oldest [`SlotState::Published`]
//!    one the host has not claimed. It moves it to [`SlotState::Writing`]; with no such slot
//!    the frame is dropped and counted. It never waits on the host inside the swap-chain
//!    window: the released fence is only read, never waited for.
//! 2. It copies the surface into the texture, signals the written fence with the frame's
//!    sequence, writes the slot's [`SlotMeta`], moves the slot to [`SlotState::Published`],
//!    publishes a [`Token`] in [`W_LATEST`] and sets the event.
//! 3. The host takes the newest publish only ([`Consumer::claim`]): it moves that slot from
//!    `Published` to [`SlotState::Reading`], and its GPU reads wait for the written fence to
//!    reach the sequence. After the last read it signals the released fence with a new value
//!    and records it in the slot as it moves the slot back to `Free` ([`Consumer::release`]).
//!
//! A slot is written only in `Writing`, which the driver enters from `Free` once the host's
//! reads are done on the GPU, or from `Published` before the host claimed it; it is read only
//! in `Reading`, which the host enters from `Published` after the driver's copy was queued.
//! Only the newest frame is ever consumed, so neither side builds a queue.
//!
//! The older keyed-mutex ring ([`crate::control::IOCTL_SET_FRAME_RING`]) is not part of this
//! module: the driver refuses it, and a header that says version 1 is not a ring
//! ([`Header::layout`]).
//!
//! The header is an array of `u64` words used atomically by both processes. The driver
//! writes only driver words and the slot states, and never trusts a host-written one: the
//! slot count, size and format it uses come from the IOCTL, which it validated, and a state
//! word it does not expect only makes that slot busy.

use core::sync::atomic::{AtomicU64, Ordering};

/// The pixel formats a ring may carry. The wire and the header carry them as numbers, which
/// are `DXGI_FORMAT` values on Windows; another platform maps its own format codes here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// 8-bit BGRA, the SDR desktop.
    Bgra8,
    /// 16-bit float RGBA, the scRGB desktop composed for HDR and wide colour.
    Rgba16F,
}

impl Format {
    #[must_use]
    pub const fn wire(self) -> u32 {
        match self {
            Self::Bgra8 => FORMAT_BGRA8,
            Self::Rgba16F => FORMAT_RGBA16F,
        }
    }

    #[must_use]
    pub const fn from_wire(format: u32) -> Option<Self> {
        match format {
            FORMAT_BGRA8 => Some(Self::Bgra8),
            FORMAT_RGBA16F => Some(Self::Rgba16F),
            _ => None,
        }
    }

    /// Bytes per pixel.
    #[must_use]
    pub const fn bytes(self) -> u32 {
        match self {
            Self::Bgra8 => 4,
            Self::Rgba16F => 8,
        }
    }
}

/// [`Format::Bgra8`] on the wire: `DXGI_FORMAT_B8G8R8A8_UNORM`.
pub const FORMAT_BGRA8: u32 = 87;
/// [`Format::Rgba16F`] on the wire: `DXGI_FORMAT_R16G16B16A16_FLOAT`.
pub const FORMAT_RGBA16F: u32 = 10;

/// Formats a ring may carry.
#[must_use]
pub const fn supported_format(format: u32) -> bool {
    Format::from_wire(format).is_some()
}

/// Fewest and most textures in a ring.
pub const MIN_SLOTS: usize = 2;
pub const MAX_SLOTS: usize = 8;
/// What the host allocates: one slot being read, one newest, one being written, one spare.
pub const DEFAULT_SLOTS: usize = 4;

/// [`W_MAGIC`]: `"RLVDRING"`.
pub const MAGIC: u64 = u64::from_le_bytes(*b"RLVDRING");
/// [`W_LAYOUT`]'s low half. Version 1 was the keyed-mutex ring's, which no header carries any
/// more; it is rejected like any other number.
pub const RING_VERSION_FENCE: u32 = 2;

// Header words. Host-written: MAGIC..GENERATION and HOST_CONSUMED. The rest is the driver's,
// except the slot states, which both sides change by compare-and-swap.
pub const W_MAGIC: usize = 0;
/// `version | slot_count << 32`.
pub const W_LAYOUT: usize = 1;
/// `width | height << 32` of the textures.
pub const W_SIZE: usize = 2;
/// The textures' `DXGI_FORMAT`.
pub const W_FORMAT: usize = 3;
/// The ring's generation, as in the IOCTL.
pub const W_GENERATION: usize = 4;
/// The newest publish, a packed [`Token`]; `0` before the first.
pub const W_LATEST: usize = 5;
/// `status | detail << 32`, a `STATUS_*` and its argument.
pub const W_STATUS: usize = 6;
/// QPC of the driver's last swap-chain pass, frame or not: the drain loop is alive.
pub const W_HEARTBEAT: usize = 7;
/// Frames the swap chain delivered while this ring was attached.
pub const W_ACQUIRED: usize = 8;
/// Frames published into the ring.
pub const W_PUSHED: usize = 9;
/// Frames dropped because every texture was held by the host.
pub const W_DROPPED: usize = 10;
/// Published frames the host never took, overwritten by a newer one.
pub const W_OVERWRITTEN: usize = 11;
/// `width | height << 32` of the last composed surface.
pub const W_SURFACE_SIZE: usize = 12;
/// `DXGI_FORMAT` of the last composed surface.
pub const W_SURFACE_FORMAT: usize = 13;
/// The last sequence the host consumed (diagnostics only; the driver does not read it).
pub const W_HOST_CONSUMED: usize = 14;
/// First slot record; each is [`SLOT_WORDS`] long.
pub const W_SLOTS: usize = 16;
pub const SLOT_WORDS: usize = 8;
// Slot record words.
const S_SEQ: usize = 0;
const S_PRESENT_QPC: usize = 1;
const S_ACQUIRE_QPC: usize = 2;
const S_PUBLISH_QPC: usize = 3;
const S_FRAME_NUMBER: usize = 4;
/// The slot's [`SlotState`], packed.
const S_STATE: usize = 5;

/// Words in a header.
pub const HEADER_WORDS: usize = W_SLOTS + MAX_SLOTS * SLOT_WORDS;
/// Bytes the driver maps.
pub const HEADER_BYTES: usize = HEADER_WORDS * 8;
/// Bytes the host allocates: one page.
pub const SECTION_BYTES: usize = 4096;
const _: () = assert!(HEADER_BYTES <= SECTION_BYTES);

/// [`W_STATUS`]: attached, no frame composed yet.
pub const STATUS_WAITING: u32 = 0;
/// Frames are being pushed.
pub const STATUS_STREAMING: u32 = 1;
/// The surface is not the ring's size; [`W_SURFACE_SIZE`] has the real one.
pub const STATUS_SIZE_MISMATCH: u32 = 2;
/// The surface is not the ring's format; [`W_SURFACE_FORMAT`] has the real one.
pub const STATUS_FORMAT_MISMATCH: u32 = 3;
/// The textures did not open on the rendering device; the detail is the HRESULT.
pub const STATUS_OPEN_FAILED: u32 = 4;
/// The textures live on another adapter than the one rendering the monitor; the detail is
/// the rendering adapter's LUID low part.
pub const STATUS_WRONG_ADAPTER: u32 = 5;
/// The rendering device was lost; the driver reopens on the new one.
pub const STATUS_DEVICE_LOST: u32 = 6;

/// One publish: which slot holds the newest frame of which ring.
/// Packed `generation << 40 | seq << 8 | slot`, generation 24-bit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub generation: u32,
    pub seq: u32,
    pub slot: u8,
}

impl Token {
    pub const GENERATION_MASK: u32 = 0x00FF_FFFF;

    #[must_use]
    pub const fn pack(self) -> u64 {
        (((self.generation & Self::GENERATION_MASK) as u64) << 40)
            | ((self.seq as u64) << 8)
            | self.slot as u64
    }

    #[must_use]
    pub const fn unpack(v: u64) -> Self {
        Self {
            generation: ((v >> 40) as u32) & Self::GENERATION_MASK,
            seq: (v >> 8) as u32,
            slot: v as u8,
        }
    }
}

/// The shape of a ring, as the host lays it out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub slot_count: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub generation: u32,
}

/// A fence ring slot's state word. Packed `tag | value << 8`, so an all-zero header is a ring
/// of never-written free slots. Every change is a compare-and-swap from the state the changer
/// last read ([`Header::swap_state`]), and the sequence or release value in it makes each
/// state distinct, so a stale swap fails instead of taking a slot that moved on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotState {
    /// Nobody holds the pixels once the released fence reaches `release` (`0`: never read).
    /// The driver may write it then.
    Free { release: u64 },
    /// The driver is copying frame `seq` into it.
    Writing { seq: u32 },
    /// It holds frame `seq`, which the host may claim; the written fence reaching `seq` means
    /// the copy is done. The driver may still reclaim it until the host does.
    Published { seq: u32 },
    /// The host claimed frame `seq` and reads it until it frees the slot again.
    Reading { seq: u32 },
    /// A word neither side writes; the slot is never used.
    Invalid(u64),
}

impl SlotState {
    const TAG_FREE: u64 = 0;
    const TAG_WRITING: u64 = 1;
    const TAG_PUBLISHED: u64 = 2;
    const TAG_READING: u64 = 3;
    /// The largest release value a state word carries.
    pub const MAX_RELEASE: u64 = u64::MAX >> 8;

    #[must_use]
    pub const fn pack(self) -> u64 {
        match self {
            Self::Free { release } => Self::TAG_FREE | (release & Self::MAX_RELEASE) << 8,
            Self::Writing { seq } => Self::TAG_WRITING | (seq as u64) << 8,
            Self::Published { seq } => Self::TAG_PUBLISHED | (seq as u64) << 8,
            Self::Reading { seq } => Self::TAG_READING | (seq as u64) << 8,
            Self::Invalid(word) => word,
        }
    }

    #[must_use]
    pub const fn unpack(word: u64) -> Self {
        let value = word >> 8;
        let seq = value as u32;
        let fits = value <= u32::MAX as u64;
        match word & 0xFF {
            Self::TAG_FREE => Self::Free { release: value },
            Self::TAG_WRITING if fits => Self::Writing { seq },
            Self::TAG_PUBLISHED if fits => Self::Published { seq },
            Self::TAG_READING if fits => Self::Reading { seq },
            _ => Self::Invalid(word),
        }
    }
}

/// What the driver wrote about one slot while it held it in [`SlotState::Writing`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct SlotMeta {
    /// The publish sequence that filled it; `0` = never written.
    pub seq: u64,
    /// When the frame is to be scanned out (IddCx's `PresentDisplayQPCTime`), normally
    /// after `acquire_qpc`: the host's present stamp is the earlier of the two.
    pub present_qpc: u64,
    /// When the driver acquired the surface.
    pub acquire_qpc: u64,
    /// When it released the texture to the host.
    pub publish_qpc: u64,
    /// `PresentationFrameNumber`.
    pub frame_number: u64,
}

/// The driver's counters, for the host's diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Counters {
    pub acquired: u64,
    pub pushed: u64,
    pub dropped: u64,
    pub overwritten: u64,
}

fn pair(lo: u32, hi: u32) -> u64 {
    u64::from(lo) | (u64::from(hi) << 32)
}

fn split(v: u64) -> (u32, u32) {
    (v as u32, (v >> 32) as u32)
}

/// A view of a header. Both processes build one over the same mapped words; every access is
/// atomic, so neither side can tear the other's writes.
#[derive(Clone, Copy)]
pub struct Header<'a> {
    words: &'a [AtomicU64],
}

impl<'a> Header<'a> {
    /// `None` unless `words` holds a whole header.
    #[must_use]
    pub fn new(words: &'a [AtomicU64]) -> Option<Self> {
        (words.len() >= HEADER_WORDS).then_some(Self { words })
    }

    fn get(&self, w: usize) -> u64 {
        self.words[w].load(Ordering::Acquire)
    }

    fn put(&self, w: usize, v: u64) {
        self.words[w].store(v, Ordering::Release);
    }

    fn bump(&self, w: usize) {
        self.words[w].fetch_add(1, Ordering::AcqRel);
    }

    // Host side.

    /// Lay out a fresh ring: every word zeroed, then the layout, magic last.
    pub fn init(&self, layout: Layout) {
        for word in &self.words[..HEADER_WORDS] {
            word.store(0, Ordering::Relaxed);
        }
        self.put(W_LAYOUT, pair(RING_VERSION_FENCE, layout.slot_count));
        self.put(W_SIZE, pair(layout.width, layout.height));
        self.put(W_FORMAT, u64::from(layout.format));
        self.put(W_GENERATION, u64::from(layout.generation));
        self.put(W_MAGIC, MAGIC);
    }

    /// The layout, or `None` when the header was never initialised or is not a fence ring's.
    #[must_use]
    pub fn layout(&self) -> Option<Layout> {
        if self.get(W_MAGIC) != MAGIC {
            return None;
        }
        let (version, slot_count) = split(self.get(W_LAYOUT));
        if version != RING_VERSION_FENCE {
            return None;
        }
        let (width, height) = split(self.get(W_SIZE));
        Some(Layout {
            slot_count,
            width,
            height,
            format: self.get(W_FORMAT) as u32,
            generation: self.get(W_GENERATION) as u32,
        })
    }

    /// The newest publish, if any.
    #[must_use]
    pub fn latest(&self) -> Option<Token> {
        match self.get(W_LATEST) {
            0 => None,
            v => Some(Token::unpack(v)),
        }
    }

    /// `(status, detail)`.
    #[must_use]
    pub fn status(&self) -> (u32, u32) {
        split(self.get(W_STATUS))
    }

    /// The last composed surface: `(width, height, format)`, zeros before the first.
    #[must_use]
    pub fn surface(&self) -> (u32, u32, u32) {
        let (w, h) = split(self.get(W_SURFACE_SIZE));
        (w, h, self.get(W_SURFACE_FORMAT) as u32)
    }

    #[must_use]
    pub fn heartbeat(&self) -> u64 {
        self.get(W_HEARTBEAT)
    }

    #[must_use]
    pub fn counters(&self) -> Counters {
        Counters {
            acquired: self.get(W_ACQUIRED),
            pushed: self.get(W_PUSHED),
            dropped: self.get(W_DROPPED),
            overwritten: self.get(W_OVERWRITTEN),
        }
    }

    /// Slot `slot`'s record. Read it only while holding the slot in [`SlotState::Reading`].
    #[must_use]
    pub fn slot(&self, slot: usize) -> SlotMeta {
        let base = W_SLOTS + slot.min(MAX_SLOTS - 1) * SLOT_WORDS;
        SlotMeta {
            seq: self.get(base + S_SEQ),
            present_qpc: self.get(base + S_PRESENT_QPC),
            acquire_qpc: self.get(base + S_ACQUIRE_QPC),
            publish_qpc: self.get(base + S_PUBLISH_QPC),
            frame_number: self.get(base + S_FRAME_NUMBER),
        }
    }

    pub fn set_consumed(&self, seq: u32) {
        self.put(W_HOST_CONSUMED, u64::from(seq));
    }

    // Both sides.

    /// Slot `slot`'s state word.
    #[must_use]
    pub fn slot_state(&self, slot: usize) -> SlotState {
        SlotState::unpack(self.get(W_SLOTS + slot.min(MAX_SLOTS - 1) * SLOT_WORDS + S_STATE))
    }

    /// Move slot `slot` from `from` to `to` if it is still in `from`. The swap orders the
    /// changer's earlier header writes (the meta) before the other side's later reads.
    pub fn swap_state(&self, slot: usize, from: SlotState, to: SlotState) -> bool {
        self.words[W_SLOTS + slot.min(MAX_SLOTS - 1) * SLOT_WORDS + S_STATE]
            .compare_exchange(from.pack(), to.pack(), Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    // Driver side.

    /// Write slot `slot`'s record; only while holding the slot in [`SlotState::Writing`].
    pub fn write_slot(&self, slot: usize, meta: SlotMeta) {
        let base = W_SLOTS + slot.min(MAX_SLOTS - 1) * SLOT_WORDS;
        self.put(base + S_PRESENT_QPC, meta.present_qpc);
        self.put(base + S_ACQUIRE_QPC, meta.acquire_qpc);
        self.put(base + S_PUBLISH_QPC, meta.publish_qpc);
        self.put(base + S_FRAME_NUMBER, meta.frame_number);
        self.put(base + S_SEQ, meta.seq);
    }

    pub fn publish(&self, token: Token) {
        self.put(W_LATEST, token.pack());
        self.bump(W_PUSHED);
    }

    pub fn set_status(&self, status: u32, detail: u32) {
        self.put(W_STATUS, pair(status, detail));
    }

    pub fn set_surface(&self, width: u32, height: u32, format: u32) {
        self.put(W_SURFACE_SIZE, pair(width, height));
        self.put(W_SURFACE_FORMAT, u64::from(format));
    }

    pub fn beat(&self, qpc: u64) {
        self.put(W_HEARTBEAT, qpc);
    }

    pub fn count_acquired(&self) {
        self.bump(W_ACQUIRED);
    }

    pub fn count_dropped(&self) {
        self.bump(W_DROPPED);
    }

    pub fn count_overwritten(&self) {
        self.bump(W_OVERWRITTEN);
    }
}

/// A slot the driver may write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Claim {
    /// Free: never written, or already consumed.
    Free(usize),
    /// Published, never consumed, now overwritten.
    Reclaimed(usize),
}

impl Claim {
    #[must_use]
    pub const fn slot(self) -> usize {
        match self {
            Self::Free(s) | Self::Reclaimed(s) => s,
        }
    }
}

/// The driver's side of the ring: which sequence each slot last carried, and the next one.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Picker {
    published: [u64; MAX_SLOTS],
    slots: usize,
    next_seq: u32,
}

impl Picker {
    #[must_use]
    pub fn new(slots: usize) -> Self {
        Self {
            published: [0; MAX_SLOTS],
            slots: slots.clamp(MIN_SLOTS, MAX_SLOTS),
            next_seq: 1,
        }
    }

    /// Slots in the order they should be tried: never written first, then oldest publish
    /// first, so the newest unconsumed frame is the last one overwritten.
    fn order(&self) -> ([usize; MAX_SLOTS], usize) {
        let mut order = [0; MAX_SLOTS];
        for (i, o) in order.iter_mut().enumerate().take(self.slots) {
            *o = i;
        }
        // Keys are unique, so the unstable sort is deterministic, and it does not allocate
        // inside the driver's swap-chain window.
        order[..self.slots].sort_unstable_by_key(|&s| (self.published[s], s));
        (order, self.slots)
    }

    /// Find a slot for frame [`Self::next_seq`] and move it to [`SlotState::Writing`].
    /// `released` is the released fence's completed value. A free slot whose last reads that
    /// value covers comes first (never written ones, then the oldest publish), then the oldest
    /// publish the host has not claimed, so a reclaim only happens when the host holds or has
    /// not taken every other slot. Never waits: a slot the host still reads, on the CPU or the
    /// GPU, is skipped.
    pub fn claim_fenced(&self, header: &Header<'_>, released: u64) -> Option<Claim> {
        let (order, n) = self.order();
        let writing = SlotState::Writing { seq: self.next_seq };
        if let Some(&s) = order[..n].iter().find(|&&s| match header.slot_state(s) {
            state @ SlotState::Free { release } => {
                release <= released && header.swap_state(s, state, writing)
            }
            _ => false,
        }) {
            return Some(Claim::Free(s));
        }
        order[..n]
            .iter()
            .find(|&&s| {
                let seq = self.published[s] as u32;
                seq != 0 && header.swap_state(s, SlotState::Published { seq }, writing)
            })
            .map(|&s| Claim::Reclaimed(s))
    }

    /// Hand slot `slot`, written with frame `seq`, to the host and record it.
    /// `false` when the slot was not in `Writing { seq }` (a host that changed it), and the
    /// frame is dropped.
    pub fn publish_fenced(&mut self, header: &Header<'_>, slot: usize, seq: u32) -> bool {
        if !header.swap_state(
            slot,
            SlotState::Writing { seq },
            SlotState::Published { seq },
        ) {
            // Not ours after all: it holds no frame we know of.
            self.skip(slot, seq);
            return false;
        }
        self.published(slot, seq);
        true
    }

    /// Give up slot `slot`, taken for frame `seq`, without publishing: it goes back
    /// to free with nothing to wait for (any read of it ended before it was taken) and holds no
    /// frame.
    pub fn abandon_fenced(&mut self, header: &Header<'_>, slot: usize, seq: u32) {
        header.swap_state(
            slot,
            SlotState::Writing { seq },
            SlotState::Free { release: 0 },
        );
        self.skip(slot, seq);
    }

    /// `slot` holds no frame, and `seq` is used up: the written fence may already carry it
    /// for a copy that was given up, so a later frame reusing it would let the host read that
    /// frame before its own copy finished.
    fn skip(&mut self, slot: usize, seq: u32) {
        self.forget(slot);
        self.next_seq = seq.wrapping_add(1).max(1);
    }

    /// `slot` holds no frame any more, so it is neither reclaimed nor copied as the newest.
    pub fn forget(&mut self, slot: usize) {
        if slot < self.slots {
            self.published[slot] = 0;
        }
    }

    /// The sequence the next publish carries; record it with [`Self::published`].
    #[must_use]
    pub fn next_seq(&self) -> u32 {
        self.next_seq
    }

    /// The slot holding the newest publish, if any; what a replacement ring is seeded from.
    #[must_use]
    pub fn newest(&self) -> Option<usize> {
        (0..self.slots)
            .filter(|&s| self.published[s] != 0)
            .max_by_key(|&s| self.published[s])
    }

    /// Record that `slot` now carries `seq`.
    pub fn published(&mut self, slot: usize, seq: u32) {
        if slot < self.slots {
            self.published[slot] = u64::from(seq);
        }
        self.next_seq = seq.wrapping_add(1).max(1);
    }
}

/// The host's side: which publish to take next.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Consumer {
    generation: u32,
    slots: u32,
    consumed: u32,
}

impl Consumer {
    #[must_use]
    pub const fn new(generation: u32, slots: u32) -> Self {
        Self {
            generation: generation & Token::GENERATION_MASK,
            slots,
            consumed: 0,
        }
    }

    /// The slot to try, when `latest` is a new publish of this ring. A token from another
    /// generation or naming a slot outside the ring is ignored, never trusted.
    #[must_use]
    pub fn wanted(&self, latest: Option<Token>) -> Option<usize> {
        let token = latest?;
        (token.generation == self.generation
            && u32::from(token.slot) < self.slots
            && token.seq > self.consumed)
            .then_some(usize::from(token.slot))
    }

    /// Claim the newest publish: the slot [`W_LATEST`] names, while it still holds
    /// a frame newer than the last consumed one. Returns the slot and that frame's sequence;
    /// the slot is the host's until [`Self::release`]. `None` when there is nothing new, or
    /// the driver reclaimed the slot first (its next publish sets the event again).
    #[must_use]
    pub fn claim(&self, header: &Header<'_>) -> Option<(usize, u32)> {
        let slot = self.wanted(header.latest())?;
        self.claim_slot(header, slot)
    }

    /// The second half of [`Self::claim`], for a slot [`Self::wanted`] named.
    #[must_use]
    pub fn claim_slot(&self, header: &Header<'_>, slot: usize) -> Option<(usize, u32)> {
        match header.slot_state(slot) {
            state @ SlotState::Published { seq }
                if seq > self.consumed
                    && header.swap_state(slot, state, SlotState::Reading { seq }) =>
            {
                Some((slot, seq))
            }
            _ => None,
        }
    }

    /// Free a claimed slot. The host signals the released fence with `release`
    /// after its last read of the slot; release values increase with every call and are
    /// signalled in that order on one queue, so the driver reusing the slot once the fence
    /// reaches `release` never overwrites pixels still being read. `false` when the slot was
    /// not `Reading { seq }`.
    pub fn release(header: &Header<'_>, slot: usize, seq: u32, release: u64) -> bool {
        header.swap_state(
            slot,
            SlotState::Reading { seq },
            SlotState::Free {
                release: release.min(SlotState::MAX_RELEASE),
            },
        )
    }

    pub fn consumed(&mut self, seq: u64) {
        self.consumed = seq.min(u64::from(u32::MAX)) as u32;
    }

    #[must_use]
    pub fn last(&self) -> u32 {
        self.consumed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn words() -> Vec<AtomicU64> {
        (0..SECTION_BYTES / 8).map(|_| AtomicU64::new(0)).collect()
    }

    #[test]
    fn formats_round_trip_through_the_wire() {
        for format in [Format::Bgra8, Format::Rgba16F] {
            assert_eq!(Format::from_wire(format.wire()), Some(format));
            assert!(supported_format(format.wire()));
        }
        assert_eq!(Format::from_wire(24), None, "R10G10B10A2 is never pushed");
        assert_eq!(Format::Rgba16F.bytes(), 8);
    }

    #[test]
    fn tokens_pack_into_their_fields() {
        let t = Token {
            generation: 0x00AB_CDEF,
            seq: 0xDEAD_BEEF,
            slot: 3,
        };
        assert_eq!(t.pack(), 0xABCD_EFDE_ADBE_EF03);
        assert_eq!(Token::unpack(t.pack()), t);
        // Generation keeps 24 bits; a wider one cannot leak into the sequence.
        let wide = Token {
            generation: 0x0100_0001,
            ..t
        };
        assert_eq!(Token::unpack(wide.pack()).generation, 1);
        assert_eq!(Token::unpack(wide.pack()).seq, 0xDEAD_BEEF);
    }

    #[test]
    fn the_header_fits_its_page_and_initialises() {
        assert_eq!(HEADER_BYTES, 640);
        let w = words();
        assert!(Header::new(&w[..HEADER_WORDS - 1]).is_none());
        let h = Header::new(&w).unwrap();
        assert_eq!(h.layout(), None, "uninitialised");
        let layout = Layout {
            slot_count: 4,
            width: 3840,
            height: 2160,
            format: FORMAT_RGBA16F,
            generation: 7,
        };
        w[W_LATEST].store(123, Ordering::Relaxed);
        h.init(layout);
        assert_eq!(h.layout(), Some(layout));
        assert_eq!(h.latest(), None, "init clears a stale publish");
        assert_eq!(
            w[W_MAGIC].load(Ordering::Relaxed).to_le_bytes(),
            *b"RLVDRING"
        );
        // Version 1 was the keyed-mutex ring's: no longer a ring this crate describes.
        w[W_LAYOUT].store(pair(1, 4), Ordering::Relaxed);
        assert_eq!(h.layout(), None, "a keyed-mutex ring header");
        w[W_LAYOUT].store(pair(3, 4), Ordering::Relaxed);
        assert_eq!(h.layout(), None, "another ring version");
        w[W_LAYOUT].store(pair(RING_VERSION_FENCE, 4), Ordering::Relaxed);
        assert_eq!(h.layout(), Some(layout));
    }

    #[test]
    fn driver_writes_reach_the_host_view() {
        let w = words();
        let driver = Header::new(&w).unwrap();
        let host = Header::new(&w).unwrap();
        host.init(Layout {
            slot_count: 3,
            width: 1920,
            height: 1080,
            format: FORMAT_BGRA8,
            generation: 1,
        });
        let meta = SlotMeta {
            seq: 5,
            present_qpc: 1_000_000,
            acquire_qpc: 1_000_100,
            publish_qpc: 1_000_200,
            frame_number: 77,
        };
        driver.write_slot(2, meta);
        driver.publish(Token {
            generation: 1,
            seq: 5,
            slot: 2,
        });
        driver.set_status(STATUS_STREAMING, 0);
        driver.set_surface(1920, 1080, FORMAT_BGRA8);
        driver.count_acquired();
        driver.count_dropped();
        driver.beat(42);
        assert_eq!(host.slot(2), meta);
        assert_eq!(host.slot(1), SlotMeta::default());
        assert_eq!(
            host.latest(),
            Some(Token {
                generation: 1,
                seq: 5,
                slot: 2
            })
        );
        assert_eq!(host.status(), (STATUS_STREAMING, 0));
        assert_eq!(host.surface(), (1920, 1080, FORMAT_BGRA8));
        assert_eq!(host.heartbeat(), 42);
        assert_eq!(
            host.counters(),
            Counters {
                acquired: 1,
                pushed: 1,
                dropped: 1,
                overwritten: 0
            }
        );
        // An out-of-range slot index is clamped, never an out-of-bounds read.
        let _ = host.slot(usize::MAX);
    }

    /// The driver's whole publish step: pick, write the meta, hand the slot over.
    fn push(picker: &mut Picker, header: &Header<'_>, released: u64) -> Option<Claim> {
        let claim = picker.claim_fenced(header, released)?;
        let seq = picker.next_seq();
        header.write_slot(
            claim.slot(),
            SlotMeta {
                seq: u64::from(seq),
                ..Default::default()
            },
        );
        assert!(picker.publish_fenced(header, claim.slot(), seq));
        header.publish(Token {
            generation: 1,
            seq,
            slot: claim.slot() as u8,
        });
        Some(claim)
    }

    #[test]
    fn the_driver_fills_free_slots_before_reclaiming() {
        let w = words();
        let header = Header::new(&w).unwrap();
        let mut p = Picker::new(3);
        assert_eq!(push(&mut p, &header, 0), Some(Claim::Free(0)));
        assert_eq!(push(&mut p, &header, 0), Some(Claim::Free(1)));
        assert_eq!(push(&mut p, &header, 0), Some(Claim::Free(2)));
        // The host took none: the oldest unconsumed frame (slot 0) goes first.
        assert_eq!(push(&mut p, &header, 0), Some(Claim::Reclaimed(0)));
        assert_eq!(push(&mut p, &header, 0), Some(Claim::Reclaimed(1)));
        assert_eq!(p.next_seq(), 6);
    }

    #[test]
    fn the_newest_publish_is_the_seed_for_a_replacement_ring() {
        let w = words();
        let header = Header::new(&w).unwrap();
        let mut p = Picker::new(3);
        assert_eq!(p.newest(), None, "nothing published yet");
        for _ in 0..4 {
            push(&mut p, &header, 0);
        }
        // Slots 0..2 carried 1..3, then slot 0 was reclaimed for 4.
        assert_eq!(p.newest(), Some(0));
        push(&mut p, &header, 0);
        assert_eq!(p.newest(), Some(1));
    }

    #[test]
    fn a_slot_the_host_returned_is_free_again() {
        let w = words();
        let header = Header::new(&w).unwrap();
        let mut p = Picker::new(2);
        let consumer = Consumer::new(1, 2);
        push(&mut p, &header, 0);
        push(&mut p, &header, 0);
        // The host consumes slot 1 (newest) and hands it back with release value 1.
        assert_eq!(consumer.claim(&header), Some((1, 2)));
        assert!(Consumer::release(&header, 1, 2, 1));
        // Slot 1 is free; slot 0 is published but older. Free wins even though slot 0 is
        // older, because a reclaim loses a frame the host could still take.
        assert_eq!(push(&mut p, &header, 1), Some(Claim::Free(1)));
    }

    #[test]
    fn the_driver_drops_rather_than_waits_when_the_host_holds_everything() {
        let w = words();
        let header = Header::new(&w).unwrap();
        let mut p = Picker::new(2);
        push(&mut p, &header, 0);
        push(&mut p, &header, 0);
        // The host is reading both.
        assert!(header.swap_state(
            0,
            SlotState::Published { seq: 1 },
            SlotState::Reading { seq: 1 }
        ));
        assert!(header.swap_state(
            1,
            SlotState::Published { seq: 2 },
            SlotState::Reading { seq: 2 }
        ));
        assert_eq!(push(&mut p, &header, 0), None);
        assert_eq!(p.next_seq(), 3, "a drop consumes no sequence number");
        // One comes back, but the released fence has not reached its value: still a drop.
        assert!(Consumer::release(&header, 0, 1, 5));
        assert_eq!(push(&mut p, &header, 4), None);
        // Once it has, the driver resumes on it.
        assert_eq!(push(&mut p, &header, 5), Some(Claim::Free(0)));
    }

    #[test]
    fn the_host_takes_only_new_frames_of_its_own_ring() {
        let mut c = Consumer::new(4, 3);
        let token = |generation, seq, slot| {
            Some(Token {
                generation,
                seq,
                slot,
            })
        };
        assert_eq!(c.wanted(None), None);
        assert_eq!(c.wanted(token(4, 1, 2)), Some(2));
        assert_eq!(c.wanted(token(3, 9, 0)), None, "an old ring's publish");
        assert_eq!(c.wanted(token(4, 9, 3)), None, "a slot outside the ring");
        c.consumed(5);
        assert_eq!(c.last(), 5);
        assert_eq!(c.wanted(token(4, 5, 1)), None, "already consumed");
        assert_eq!(c.wanted(token(4, 6, 1)), Some(1));
    }

    #[test]
    fn a_reassigned_swap_chain_continues_the_rings_sequence() {
        // The picker belongs to the ring, so a worker started for a replacement swap chain on
        // the same ring publishes after what the host already consumed.
        let w = words();
        let header = Header::new(&w).unwrap();
        let mut picker = Picker::new(4);
        let mut consumer = Consumer::new(1, 4);
        let mut release = 0;
        let mut host_takes = |header: &Header<'_>, consumer: &mut Consumer| {
            let (slot, seq) = consumer.claim(header)?;
            consumer.consumed(u64::from(seq));
            release += 1;
            assert!(Consumer::release(header, slot, seq, release));
            Some(release)
        };
        let mut released = 0;
        for _ in 0..1000 {
            push(&mut picker, &header, released).expect("a slot");
            released = host_takes(&header, &mut consumer).expect("a frame");
        }
        // The swap chain is reassigned: the new worker reuses the ring's picker.
        push(&mut picker, &header, released).expect("a slot");
        assert_eq!(header.latest().unwrap().seq, 1001);
        assert!(host_takes(&header, &mut consumer).is_some());
        // A picker restarted at 1 would publish frames the host rejects as consumed until
        // the sequence caught up: the freeze this ownership prevents.
        let fresh_w = words();
        let fresh_header = Header::new(&fresh_w).unwrap();
        let mut restarted = Picker::new(4);
        push(&mut restarted, &fresh_header, 0).expect("a slot");
        let token = fresh_header.latest().unwrap();
        assert_eq!(token.seq, 1);
        assert_eq!(consumer.wanted(Some(token)), None);
    }

    #[test]
    fn end_to_end_the_host_always_sees_the_newest_frame() {
        let w = words();
        let header = Header::new(&w).unwrap();
        let layout = Layout {
            slot_count: 3,
            width: 640,
            height: 480,
            format: FORMAT_BGRA8,
            generation: 1,
        };
        header.init(layout);
        let mut picker = Picker::new(3);
        let mut consumer = Consumer::new(layout.generation, layout.slot_count);
        let mut release = 0;
        let mut seen = Vec::new();
        for frame in 1..=20u64 {
            // Driver: three frames per host poll, so the host must skip. Its present stamp
            // travels in the slot's meta.
            let claim = picker.claim_fenced(&header, release).expect("a slot");
            let seq = picker.next_seq();
            header.write_slot(
                claim.slot(),
                SlotMeta {
                    seq: u64::from(seq),
                    present_qpc: frame * 100,
                    ..Default::default()
                },
            );
            assert!(picker.publish_fenced(&header, claim.slot(), seq));
            header.publish(Token {
                generation: layout.generation,
                seq,
                slot: claim.slot() as u8,
            });
            if frame % 3 != 0 {
                continue;
            }
            // Host.
            let (slot, seq) = consumer.claim(&header).expect("a new frame");
            let meta = header.slot(slot);
            assert_eq!(meta.seq, u64::from(seq));
            consumer.consumed(meta.seq);
            release += 1;
            assert!(Consumer::release(&header, slot, seq, release));
            seen.push(meta.present_qpc);
        }
        assert_eq!(seen, [300, 600, 900, 1200, 1500, 1800]);
    }

    #[test]
    fn slot_states_round_trip_and_reject_foreign_words() {
        for state in [
            SlotState::Free { release: 0 },
            SlotState::Free {
                release: SlotState::MAX_RELEASE,
            },
            SlotState::Writing { seq: u32::MAX },
            SlotState::Published { seq: 1 },
            SlotState::Reading { seq: 7 },
        ] {
            assert_eq!(SlotState::unpack(state.pack()), state);
        }
        assert_eq!(
            SlotState::unpack(0),
            SlotState::Free { release: 0 },
            "zeroed"
        );
        assert!(matches!(SlotState::unpack(9), SlotState::Invalid(9)));
        let wide = 1 | (1u64 << 40);
        assert_eq!(
            SlotState::unpack(wide),
            SlotState::Invalid(wide),
            "seq past 32 bits"
        );
        // A slot holding a word nobody writes is never taken.
        let w = words();
        let header = Header::new(&w).unwrap();
        w[W_SLOTS + S_STATE].store(9, Ordering::Relaxed);
        let picker = Picker::new(2);
        assert_eq!(picker.claim_fenced(&header, 0), Some(Claim::Free(1)));
    }

    #[test]
    fn the_driver_skips_slots_whose_reads_the_released_fence_does_not_cover() {
        let w = words();
        let header = Header::new(&w).unwrap();
        let mut picker = Picker::new(2);
        let consumer = Consumer::new(0, 2);
        // Frame 1 into slot 0, frame 2 into slot 1; the host claims and frees frame 2 with
        // release value 5, not yet reached on the GPU.
        for slot in 0..2 {
            let claim = picker.claim_fenced(&header, 0).unwrap();
            assert_eq!(claim, Claim::Free(slot));
            let seq = picker.next_seq();
            assert!(picker.publish_fenced(&header, slot, seq));
            header.publish(Token {
                generation: 0,
                seq,
                slot: slot as u8,
            });
        }
        assert_eq!(consumer.claim(&header), Some((1, 2)));
        assert!(Consumer::release(&header, 1, 2, 5));
        assert_eq!(header.slot_state(1), SlotState::Free { release: 5 });
        // Slot 1 is busy until the fence reaches 5, so the driver overwrites frame 1 instead.
        assert_eq!(picker.claim_fenced(&header, 4), Some(Claim::Reclaimed(0)));
        picker.abandon_fenced(&header, 0, 3);
        assert_eq!(header.slot_state(0), SlotState::Free { release: 0 });
        assert_eq!(picker.newest(), Some(1), "an abandoned slot holds no frame");
        assert_eq!(picker.claim_fenced(&header, 5), Some(Claim::Free(0)));
    }

    #[test]
    fn the_host_takes_only_the_newest_publish_and_loses_it_to_a_reclaim() {
        let w = words();
        let header = Header::new(&w).unwrap();
        let mut picker = Picker::new(2);
        let mut consumer = Consumer::new(3, 2);
        let publish = |picker: &mut Picker| {
            let slot = picker.claim_fenced(&header, 0).unwrap().slot();
            let seq = picker.next_seq();
            assert!(picker.publish_fenced(&header, slot, seq));
            header.publish(Token {
                generation: 3,
                seq,
                slot: slot as u8,
            });
            (slot, seq)
        };
        publish(&mut picker);
        let (newest, seq) = publish(&mut picker);
        // The host looks at the newest publish (slot 1, frame 2); before it swaps, the driver
        // runs out of free slots and reclaims the oldest unclaimed one, slot 0.
        let wanted = consumer.wanted(header.latest()).unwrap();
        assert_eq!((wanted, seq), (1, 2));
        assert_eq!(picker.claim_fenced(&header, 0), Some(Claim::Reclaimed(0)));
        // Next it would reclaim slot 1, the host's choice; the swap stops the host reading it.
        let third = picker.next_seq();
        assert!(picker.publish_fenced(&header, 0, third));
        assert_eq!(
            picker.claim_fenced(&header, 0),
            Some(Claim::Reclaimed(newest))
        );
        assert_eq!(consumer.claim_slot(&header, wanted), None, "being written");
        let fourth = picker.next_seq();
        assert!(picker.publish_fenced(&header, newest, fourth));
        header.publish(Token {
            generation: 3,
            seq: fourth,
            slot: newest as u8,
        });
        // Frame 3 in slot 0 is unconsumed but not the newest publish: the host takes 4.
        assert_eq!(consumer.claim(&header), Some((newest, fourth)));
        consumer.consumed(u64::from(fourth));
        assert_eq!(consumer.claim(&header), None, "already claimed");
        assert!(
            !Consumer::release(&header, newest, fourth + 1, 1),
            "not that frame"
        );
        assert!(Consumer::release(&header, newest, fourth, 1));
        assert_eq!(header.slot_state(0), SlotState::Published { seq: third });
    }

    /// One state of the exhaustive model below: both processes' views plus the two GPU
    /// queues, each running in order.
    #[derive(Clone, PartialEq, Eq, Hash)]
    struct World {
        header: Vec<u64>,
        picker: Picker,
        consumer: Consumer,
        frames_left: u8,
        /// The driver took this slot for this frame and has not published it.
        writing: Option<(usize, u32)>,
        /// The driver's queue: each copy and the written-fence signal after it.
        copies: Vec<(usize, u32)>,
        written: u32,
        /// The slot the host saw in `W_LATEST` and is about to swap.
        wanted: Option<usize>,
        /// Claimed and not yet freed: `(slot, seq)`.
        holds: Vec<(usize, u32)>,
        /// The host's queue: `Ok((slot, seq))` waits for the written fence to reach `seq`,
        /// then reads the slot; `Err((slot, value))` signals the released fence after the
        /// slot's last read.
        reads: Vec<Result<(usize, u32), (usize, u64)>>,
        /// Slots the host's GPU is reading: past the wait, before the released signal.
        gpu_reads: Vec<usize>,
        released: u64,
        next_release: u64,
    }

    /// The host holds at most the newest frame and the one being encoded.
    const MAX_HOLDS: usize = 2;

    impl World {
        fn new(slots: usize, frames: u8) -> Self {
            Self {
                header: std::vec![0; HEADER_WORDS],
                picker: Picker::new(slots),
                consumer: Consumer::new(1, slots as u32),
                frames_left: frames,
                writing: None,
                copies: Vec::new(),
                written: 0,
                wanted: None,
                holds: Vec::new(),
                reads: Vec::new(),
                gpu_reads: Vec::new(),
                released: 0,
                next_release: 1,
            }
        }

        /// Run `f` on the header these words hold, then store them back.
        fn with_header<T>(&mut self, f: impl FnOnce(&Header<'_>, &mut Self) -> T) -> T {
            let atomics: Vec<AtomicU64> = self.header.iter().map(|&v| AtomicU64::new(v)).collect();
            let out = f(&Header::new(&atomics).unwrap(), self);
            self.header = atomics.iter().map(|a| a.load(Ordering::Relaxed)).collect();
            out
        }

        fn state(&mut self, slot: usize) -> SlotState {
            self.with_header(|h, _| h.slot_state(slot))
        }

        /// Every state one step from this one.
        fn steps(&self, slots: usize) -> Vec<Self> {
            let mut next = Vec::new();
            // Driver: take a slot for a new frame, or drop it.
            if self.writing.is_none() && self.frames_left > 0 {
                let mut w = self.clone();
                w.frames_left -= 1;
                let released = w.released;
                let idle = w.holds.is_empty() && w.reads.iter().all(Result::is_ok);
                match w.with_header(|h, w| w.picker.claim_fenced(h, released)) {
                    Some(claim) => {
                        let seq = w.picker.next_seq();
                        assert_eq!(w.state(claim.slot()), SlotState::Writing { seq });
                        w.writing = Some((claim.slot(), seq));
                        w.copies.push((claim.slot(), seq));
                    }
                    // Dropping is the driver's only answer to a busy ring, and it happens
                    // only while the host holds or still reads slots.
                    None => assert!(!idle, "a ring the host left alone has no free slot"),
                }
                next.push(w);
            }
            // Driver: publish what it wrote, or give it up (a failed GPU signal).
            if let Some((slot, seq)) = self.writing {
                let mut w = self.clone();
                w.writing = None;
                w.with_header(|h, w| {
                    h.write_slot(
                        slot,
                        SlotMeta {
                            seq: u64::from(seq),
                            ..Default::default()
                        },
                    );
                    assert!(
                        w.picker.publish_fenced(h, slot, seq),
                        "the driver's own slot"
                    );
                    h.publish(Token {
                        generation: 1,
                        seq,
                        slot: slot as u8,
                    });
                });
                next.push(w);
                let mut w = self.clone();
                w.writing = None;
                w.with_header(|h, w| w.picker.abandon_fenced(h, slot, seq));
                next.push(w);
            }
            // Host: look at the newest publish.
            if self.wanted.is_none() && self.holds.len() < MAX_HOLDS {
                let mut w = self.clone();
                if let Some(slot) = w.with_header(|h, w| w.consumer.wanted(h.latest())) {
                    w.wanted = Some(slot);
                    next.push(w);
                }
            }
            // Host: swap the slot it looked at to Reading.
            if let Some(slot) = self.wanted {
                let mut w = self.clone();
                w.wanted = None;
                let before = w.consumer.last();
                if let Some((s, seq)) = w.with_header(|h, w| w.consumer.claim_slot(h, slot)) {
                    assert_eq!(s, slot, "only the slot the newest publish named");
                    assert!(seq > before, "only a frame not consumed yet");
                    let meta = w.with_header(|h, _| h.slot(slot));
                    assert_eq!(meta.seq, u64::from(seq), "the meta of the claimed frame");
                    w.consumer.consumed(u64::from(seq));
                    w.holds.push((slot, seq));
                    w.reads.push(Ok((slot, seq)));
                }
                next.push(w);
            }
            // Host: free any claimed slot after its last read.
            for i in 0..self.holds.len() {
                let mut w = self.clone();
                let (slot, seq) = w.holds.remove(i);
                let release = w.next_release;
                w.next_release += 1;
                w.reads.push(Err((slot, release)));
                assert!(w.with_header(|h, _| Consumer::release(h, slot, seq, release)));
                next.push(w);
            }
            // GPU: the driver's next copy and its written signal finish.
            if !self.copies.is_empty() {
                let mut w = self.clone();
                let (_, seq) = w.copies.remove(0);
                w.written = seq;
                next.push(w);
            }
            // GPU: the host's queue moves past a satisfied wait or a signal.
            match self.reads.first() {
                Some(&Ok((slot, seq))) if self.written >= seq => {
                    let mut w = self.clone();
                    let _ = w.reads.remove(0);
                    w.gpu_reads.push(slot);
                    next.push(w);
                }
                Some(&Err((slot, value))) => {
                    let mut w = self.clone();
                    let _ = w.reads.remove(0);
                    w.released = value;
                    let read = w.gpu_reads.iter().position(|&s| s == slot).unwrap();
                    w.gpu_reads.remove(read);
                    next.push(w);
                }
                _ => {}
            }
            for w in &mut next {
                w.check(slots);
            }
            next
        }

        /// The invariants, after every step.
        fn check(&mut self, slots: usize) {
            for slot in 0..slots {
                let state = self.state(slot);
                // The driver's copy is in flight from its pick until its written signal ran;
                // a host read from the end of its wait until its released signal ran.
                let written_on_gpu = self.copies.iter().any(|&(s, _)| s == slot);
                let read_on_gpu = self.gpu_reads.contains(&slot);
                assert!(
                    !(written_on_gpu && read_on_gpu),
                    "slot {slot} written and read at once ({state:?})"
                );
                if let Some(&(_, seq)) = self.holds.iter().find(|&&(s, _)| s == slot) {
                    assert_eq!(state, SlotState::Reading { seq });
                }
                if let Some((s, seq)) = self.writing
                    && s == slot
                {
                    assert_eq!(state, SlotState::Writing { seq });
                }
                assert!(!matches!(state, SlotState::Invalid(_)));
            }
        }
    }

    /// Explore every reachable state; returns how many there were.
    fn explore(slots: usize, frames: u8) -> usize {
        let mut seen = std::collections::HashSet::new();
        let mut todo = std::vec![World::new(slots, frames)];
        let mut finished = 0;
        while let Some(world) = todo.pop() {
            if !seen.insert(world.clone()) {
                continue;
            }
            let next = world.steps(slots);
            if next.is_empty() {
                // Nothing left to do: every frame was published or dropped and both GPU
                // queues drained, so no wait is left hanging.
                assert!(world.frames_left == 0 && world.writing.is_none());
                assert!(world.copies.is_empty() && world.reads.is_empty());
                finished += 1;
            }
            todo.extend(next);
        }
        assert!(finished > 0);
        seen.len()
    }

    #[test]
    fn every_interleaving_of_driver_host_and_gpus_keeps_slots_apart() {
        // Every order of the driver's pick, reclaim, publish and abandon against the host's
        // look, claim and free, with both GPU queues finishing at any point; the invariants
        // are checked after every step (`World::check`), at every pick and every claim.
        for (slots, frames) in [(2, 7), (3, 6), (4, 5)] {
            let states = explore(slots, frames);
            assert!(
                states > 1000,
                "{slots} slots: only {states} states explored"
            );
        }
    }
}
