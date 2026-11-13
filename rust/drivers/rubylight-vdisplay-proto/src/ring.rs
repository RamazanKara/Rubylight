//! The frame push: shared GPU textures plus a small header section, one event.
//!
//! A ring comes in two kinds, which the driver tells apart by the IOCTL that delivered it:
//!
//! **Fence ring** ([`RING_VERSION_FENCE`], [`crate::control::SetFenceRingRequest`], present
//! when the driver reports [`crate::control::CAP_FENCE_RING`]). The host creates
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
//!
//! **Keyed-mutex ring** ([`RING_VERSION`], [`crate::control::SetFrameRingRequest`]), what
//! the default transport, also understood by drivers before the fence ring. The
//! textures are `SHARED_NTHANDLE | SHARED_KEYEDMUTEX` and pixel ownership is the keyed
//! mutex's alone:
//!
//! 1. The driver picks a texture whose keyed mutex it can take with [`KEY_DRIVER`] (never
//!    written, or handed back by the host), else the oldest one the host has not taken yet,
//!    with [`KEY_HOST`] ([`Picker::claim`]). With neither the frame is dropped and counted.
//! 2. It copies the surface into the texture, writes the slot's [`SlotMeta`] while holding the
//!    mutex, releases it with [`KEY_HOST`], publishes a [`Token`] and sets the event.
//! 3. The host reads the token, takes that slot's mutex with [`KEY_HOST`] (timeout 0), reads
//!    the meta, reads the texture and releases with [`KEY_DRIVER`].
//!
//! Either way only the newest frame is ever consumed, so neither side builds a queue.
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

/// Keyed-mutex key under which the driver may write a texture (keyed-mutex ring only).
pub const KEY_DRIVER: u64 = 0;
/// Keyed-mutex key under which the host may read it (keyed-mutex ring only).
pub const KEY_HOST: u64 = 1;

/// [`W_MAGIC`]: `"RLVDRING"`.
pub const MAGIC: u64 = u64::from_le_bytes(*b"RLVDRING");
/// [`W_LAYOUT`]'s low half for a keyed-mutex ring.
pub const RING_VERSION: u32 = 1;
/// [`W_LAYOUT`]'s low half for a fence ring: the keyed-mutex layout plus a state word in
/// every slot record.
pub const RING_VERSION_FENCE: u32 = 2;

// Header words. Host-written: MAGIC..GENERATION and HOST_CONSUMED. The rest is the driver's,
// except a fence ring's slot states, which both sides change by compare-and-swap.
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
/// A fence ring's [`SlotState`], packed; unused in a keyed-mutex ring.
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
/// The fence ring exhausted its monotonic sequence; the host replaces it with fresh fences.
pub const STATUS_RENEW_REQUIRED: u32 = 7;
/// Leave headroom for a host to observe retirement before any 32-bit sequence could wrap.
pub const FENCE_SEQUENCE_LIMIT: u32 = u32::MAX - 1024;

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

/// Who owns a slot's pixels: the state words and fences, or each texture's keyed mutex.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Fence,
    /// The default transport, compatible with hosts and drivers before fence rings.
    KeyedMutex,
}

impl Kind {
    #[must_use]
    pub const fn version(self) -> u32 {
        match self {
            Self::Fence => RING_VERSION_FENCE,
            Self::KeyedMutex => RING_VERSION,
        }
    }

    #[must_use]
    pub const fn from_version(version: u32) -> Option<Self> {
        match version {
            RING_VERSION_FENCE => Some(Self::Fence),
            RING_VERSION => Some(Self::KeyedMutex),
            _ => None,
        }
    }
}

/// The shape of a ring, as the host lays it out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub kind: Kind,
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

/// What the driver wrote about one slot, under that slot's keyed mutex.
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
        self.put(W_LAYOUT, pair(layout.kind.version(), layout.slot_count));
        self.put(W_SIZE, pair(layout.width, layout.height));
        self.put(W_FORMAT, u64::from(layout.format));
        self.put(W_GENERATION, u64::from(layout.generation));
        self.put(W_MAGIC, MAGIC);
    }

    /// The layout, or `None` when the header was never initialised for a known version.
    #[must_use]
    pub fn layout(&self) -> Option<Layout> {
        if self.get(W_MAGIC) != MAGIC {
            return None;
        }
        let (version, slot_count) = split(self.get(W_LAYOUT));
        let kind = Kind::from_version(version)?;
        let (width, height) = split(self.get(W_SIZE));
        Some(Layout {
            kind,
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

    /// Slot `slot`'s record. Read it only while holding the slot: its keyed mutex, or in a
    /// fence ring [`SlotState::Reading`].
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

    // Both sides, fence ring.

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

    /// Write slot `slot`'s record; only while holding it (its keyed mutex, or
    /// [`SlotState::Writing`]).
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
    /// Taken with [`KEY_DRIVER`]: never written, or already consumed.
    Free(usize),
    /// Taken with [`KEY_HOST`]: published, never consumed, now overwritten.
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

    /// Find a slot to write. `try_take(slot, key)` attempts the slot's keyed mutex without
    /// waiting and reports whether it was taken. Free slots are tried before any published
    /// one is reclaimed, so a reclaim only happens when the host holds or has not taken every
    /// other texture.
    pub fn claim(&self, mut try_take: impl FnMut(usize, u64) -> bool) -> Option<Claim> {
        let (order, n) = self.order();
        if let Some(&s) = order[..n].iter().find(|&&s| try_take(s, KEY_DRIVER)) {
            return Some(Claim::Free(s));
        }
        order[..n]
            .iter()
            .find(|&&s| self.published[s] != 0 && try_take(s, KEY_HOST))
            .map(|&s| Claim::Reclaimed(s))
    }

    /// Find a fence-ring slot for frame [`Self::next_seq`] and move it to
    /// [`SlotState::Writing`]. `released` is the released fence's completed value. A free
    /// slot whose last reads that value covers comes first, in the same order as
    /// [`Self::claim`], then the oldest publish the host has not claimed. Never waits: a slot
    /// the host still reads, on the CPU or the GPU, is skipped.
    pub fn claim_fenced(&self, header: &Header<'_>, released: u64) -> Option<Claim> {
        if self.needs_renewal() {
            return None;
        }
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

    /// Hand fence-ring slot `slot`, written with frame `seq`, to the host and record it.
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

    /// Give up fence-ring slot `slot`, taken for frame `seq`, without publishing: it goes back
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

    /// Never wrap a GPU fence to 1: an old completed value would release the new frame's
    /// reader before its copy. Reserve the final value and rebuild the ring instead.
    #[must_use]
    pub fn needs_renewal(&self) -> bool {
        self.next_seq >= FENCE_SEQUENCE_LIMIT
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

    /// Whether `meta`, read under the slot's mutex, is newer than what was consumed. A
    /// reclaim may have refilled the slot with an even newer frame, which is just as good.
    #[must_use]
    pub fn fresh(&self, meta: &SlotMeta) -> bool {
        meta.seq > u64::from(self.consumed) && meta.seq <= u64::from(u32::MAX)
    }

    /// Claim a fence ring's newest publish: the slot [`W_LATEST`] names, while it still holds
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

    /// Free a claimed fence-ring slot. The host signals the released fence with `release`
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
mod tests;
