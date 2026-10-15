//! The frame push: shared GPU textures plus a small header section, one event.
//!
//! The host creates `slot_count` textures (`SHARED_NTHANDLE | SHARED_KEYEDMUTEX`) on the
//! adapter that renders the monitor, a section of [`SECTION_BYTES`] and an event, and hands
//! them to the driver with [`crate::control::SetFrameRingRequest`]. Then, per composed frame:
//!
//! 1. The driver picks a slot ([`Picker`]): a texture whose keyed mutex it can take with
//!    [`KEY_DRIVER`] (never written, or handed back by the host), else the oldest one the host
//!    has not taken yet, with [`KEY_HOST`]. With neither the frame is dropped and counted: the
//!    driver never waits on the host inside the swap-chain window.
//! 2. It copies the surface into the texture, writes the slot's [`SlotMeta`] (sequence,
//!    present QPC) while holding the mutex, releases it with [`KEY_HOST`], publishes a
//!    [`Token`] in [`W_LATEST`] and sets the event.
//! 3. The host reads the token, takes that slot's mutex with [`KEY_HOST`] (timeout 0), reads
//!    the meta, copies the texture into its own pool and releases with [`KEY_DRIVER`].
//!
//! Only the newest frame is ever consumed, so neither side builds a queue. Pixel ownership is
//! the keyed mutex's alone; the header only says which slot is newest and what it holds.
//!
//! The header is an array of `u64` words used atomically by both processes. The driver
//! writes only driver words and never trusts a host-written one: the slot count, size and
//! format it uses come from the IOCTL, which it validated.

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

/// Keyed-mutex key under which the driver may write a texture.
pub const KEY_DRIVER: u64 = 0;
/// Keyed-mutex key under which the host may read it.
pub const KEY_HOST: u64 = 1;

/// [`W_MAGIC`]: `"RLVDRING"`.
pub const MAGIC: u64 = u64::from_le_bytes(*b"RLVDRING");
/// [`W_LAYOUT`]'s low half.
pub const RING_VERSION: u32 = 1;

// Header words. Host-written: MAGIC..GENERATION and HOST_CONSUMED. The rest is the driver's.
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
        self.put(W_LAYOUT, pair(RING_VERSION, layout.slot_count));
        self.put(W_SIZE, pair(layout.width, layout.height));
        self.put(W_FORMAT, u64::from(layout.format));
        self.put(W_GENERATION, u64::from(layout.generation));
        self.put(W_MAGIC, MAGIC);
    }

    /// The layout, or `None` when the header was never initialised for this version.
    #[must_use]
    pub fn layout(&self) -> Option<Layout> {
        if self.get(W_MAGIC) != MAGIC {
            return None;
        }
        let (version, slot_count) = split(self.get(W_LAYOUT));
        if version != RING_VERSION {
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

    /// Slot `slot`'s record. Read it only while holding that slot's keyed mutex.
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

    // Driver side.

    /// Write slot `slot`'s record; only while holding its keyed mutex.
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
#[derive(Clone, Debug)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
        w[W_LAYOUT].store(pair(2, 4), Ordering::Relaxed);
        assert_eq!(h.layout(), None, "another ring version");
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

    /// A model of the textures' keyed mutexes: `None` = held, `Some(key)` = released with key.
    struct Mutexes(Vec<Option<u64>>);

    impl Mutexes {
        fn new(n: usize) -> Self {
            Self(std::vec![Some(KEY_DRIVER); n])
        }
        fn take(&mut self, slot: usize, key: u64) -> bool {
            if self.0[slot] == Some(key) {
                self.0[slot] = None;
                true
            } else {
                false
            }
        }
        fn release(&mut self, slot: usize, key: u64) {
            assert_eq!(self.0[slot], None, "release of a free mutex");
            self.0[slot] = Some(key);
        }
    }

    /// The driver's whole publish step against the model.
    fn push(picker: &mut Picker, m: &mut Mutexes) -> Option<Claim> {
        let claim = picker.claim(|s, key| m.take(s, key))?;
        let seq = picker.next_seq();
        m.release(claim.slot(), KEY_HOST);
        picker.published(claim.slot(), seq);
        Some(claim)
    }

    #[test]
    fn the_driver_fills_free_slots_before_reclaiming() {
        let mut m = Mutexes::new(3);
        let mut p = Picker::new(3);
        assert_eq!(push(&mut p, &mut m), Some(Claim::Free(0)));
        assert_eq!(push(&mut p, &mut m), Some(Claim::Free(1)));
        assert_eq!(push(&mut p, &mut m), Some(Claim::Free(2)));
        // The host took none: the oldest unconsumed frame (slot 0) goes first.
        assert_eq!(push(&mut p, &mut m), Some(Claim::Reclaimed(0)));
        assert_eq!(push(&mut p, &mut m), Some(Claim::Reclaimed(1)));
        assert_eq!(p.next_seq(), 6);
    }

    #[test]
    fn the_newest_publish_is_the_seed_for_a_replacement_ring() {
        let mut m = Mutexes::new(3);
        let mut p = Picker::new(3);
        assert_eq!(p.newest(), None, "nothing published yet");
        for _ in 0..4 {
            push(&mut p, &mut m);
        }
        // Slots 0..2 carried 1..3, then slot 0 was reclaimed for 4.
        assert_eq!(p.newest(), Some(0));
        push(&mut p, &mut m);
        assert_eq!(p.newest(), Some(1));
    }

    #[test]
    fn a_slot_the_host_returned_is_free_again() {
        let mut m = Mutexes::new(2);
        let mut p = Picker::new(2);
        push(&mut p, &mut m);
        push(&mut p, &mut m);
        // The host consumes slot 1 (newest) and hands it back.
        assert!(m.take(1, KEY_HOST));
        m.release(1, KEY_DRIVER);
        // Slot 1 is free; slot 0 is published but older. Free wins even though slot 0 is
        // older, because a reclaim loses a frame the host could still take.
        assert_eq!(push(&mut p, &mut m), Some(Claim::Free(1)));
    }

    #[test]
    fn the_driver_drops_rather_than_waits_when_the_host_holds_everything() {
        let mut m = Mutexes::new(2);
        let mut p = Picker::new(2);
        push(&mut p, &mut m);
        push(&mut p, &mut m);
        // The host is reading both.
        assert!(m.take(0, KEY_HOST));
        assert!(m.take(1, KEY_HOST));
        assert_eq!(push(&mut p, &mut m), None);
        assert_eq!(p.next_seq(), 3, "a drop consumes no sequence number");
        // One comes back: the driver resumes on it.
        m.release(0, KEY_DRIVER);
        assert_eq!(push(&mut p, &mut m), Some(Claim::Free(0)));
    }

    #[test]
    fn a_never_written_slot_is_not_reclaimed_with_the_host_key() {
        // A texture nobody wrote is released under KEY_DRIVER; trying KEY_HOST on it would
        // fail anyway, but the picker must not even rely on that.
        let p = Picker::new(3);
        let mut tried = Vec::new();
        let claim = p.claim(|s, key| {
            tried.push((s, key));
            false
        });
        assert_eq!(claim, None);
        assert!(tried.iter().all(|&(_, key)| key == KEY_DRIVER));
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
        // Meta read under the mutex: a reclaim may have made it newer, never older.
        let meta = |seq| SlotMeta {
            seq,
            ..Default::default()
        };
        assert!(c.fresh(&meta(6)));
        assert!(c.fresh(&meta(9)));
        assert!(!c.fresh(&meta(5)));
        assert!(!c.fresh(&meta(0)));
    }

    #[test]
    fn a_reassigned_swap_chain_continues_the_rings_sequence() {
        // The picker belongs to the ring, so a worker started for a replacement swap chain on
        // the same ring publishes after what the host already consumed.
        let mut m = Mutexes::new(4);
        let mut picker = Picker::new(4);
        let mut consumer = Consumer::new(3, 4);
        let host_takes = |m: &mut Mutexes, consumer: &mut Consumer, token: Token| {
            let slot = consumer.wanted(Some(token))?;
            assert!(m.take(slot, KEY_HOST));
            consumer.consumed(u64::from(token.seq));
            m.release(slot, KEY_DRIVER);
            Some(slot)
        };
        let publish = |picker: &mut Picker, m: &mut Mutexes| {
            let claim = push(picker, m).expect("a slot");
            Token {
                generation: 3,
                seq: picker.next_seq() - 1,
                slot: claim.slot() as u8,
            }
        };
        for _ in 0..1000 {
            let token = publish(&mut picker, &mut m);
            assert!(host_takes(&mut m, &mut consumer, token).is_some());
        }
        // The swap chain is reassigned: the new worker reuses the ring's picker.
        let token = publish(&mut picker, &mut m);
        assert_eq!(token.seq, 1001);
        assert!(host_takes(&mut m, &mut consumer, token).is_some());
        // A picker restarted at 1 would publish frames the host rejects as consumed until
        // the sequence caught up: the freeze this ownership prevents.
        let mut restarted = Picker::new(4);
        let token = publish(&mut restarted, &mut m);
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
            generation: 9,
        };
        header.init(layout);
        let mut m = Mutexes::new(3);
        let mut picker = Picker::new(3);
        let mut consumer = Consumer::new(layout.generation, layout.slot_count);
        let mut seen = Vec::new();
        for frame in 1..=20u64 {
            // Driver: three frames per host poll, so the host must skip.
            let claim = picker.claim(|s, key| m.take(s, key)).expect("a slot");
            let seq = picker.next_seq();
            header.write_slot(
                claim.slot(),
                SlotMeta {
                    seq: u64::from(seq),
                    present_qpc: frame * 100,
                    ..Default::default()
                },
            );
            m.release(claim.slot(), KEY_HOST);
            picker.published(claim.slot(), seq);
            header.publish(Token {
                generation: layout.generation,
                seq,
                slot: claim.slot() as u8,
            });
            if frame % 3 != 0 {
                continue;
            }
            // Host.
            let slot = consumer.wanted(header.latest()).expect("a new frame");
            assert!(m.take(slot, KEY_HOST));
            let meta = header.slot(slot);
            assert!(consumer.fresh(&meta));
            consumer.consumed(meta.seq);
            m.release(slot, KEY_DRIVER);
            seen.push(meta.present_qpc);
        }
        assert_eq!(seen, [300, 600, 900, 1200, 1500, 1800]);
    }
}
