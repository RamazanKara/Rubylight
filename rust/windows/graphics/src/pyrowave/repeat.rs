//! Static repeat-frame caching and 3-bit PyroWave sequence-counter patching.
//!
//! When constant-rate capture or frame pacing repeats an unchanged frame in
//! record-framed PyroWave mode, the first repeat caches the flat wire bytes
//! and every non-padding record's sequence-byte offset. Subsequent identical
//! repeats copy the cached frame from `frame_pool`, advance `seq_offset`
//! modulo 8, and patch the 3-bit sequence counter in place without running
//! colour conversion, Vulkan encoding, or record framing. Future fresh encodes
//! from the SDK shift their raw sequence counter by `seq_offset` before
//! framing so the decoder observes a strictly monotonic `(seq + 1) & 7` stream.

use crate::capture::{GpuImage, Pixel};
use crate::overlay::Overlay;
use anyhow::{Context, Result, bail};
use std::sync::Arc;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::core::Interface;

const HEADER_BIT: u32 = 0x8000_0000;
const PADDING: u32 = u32::MAX;

#[inline]
fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

#[inline]
pub(super) fn patch_byte(byte3: u8, seq: u8) -> u8 {
    (byte3 & 0x8f) | ((seq & 7) << 4)
}

/// Validates a PyroWave record bitstream and returns `(sequence, byte_offsets)`
/// where `byte_offsets` names byte 3 of the 8-byte sequence header and byte 3
/// of every non-padding block record (skipping `0xFFFF_FFFF` padding records).
pub(super) fn scan_sequence_offsets(bytes: &[u8]) -> Result<(u8, Vec<usize>)> {
    let len = bytes.len();
    if len < 8
        || !len.is_multiple_of(4)
        || word(bytes, 0) & HEADER_BIT == 0
        || word(bytes, 0) == PADDING
        || (word(bytes, 4) >> 24) & 3 != 0
    {
        bail!("invalid PyroWave sequence header");
    }
    let seq = ((word(bytes, 0) >> 28) & 7) as u8;
    let expected_records = (word(bytes, 4) & 0x00ff_ffff) as usize;
    let mut offsets = Vec::with_capacity(expected_records.saturating_add(1).min(65_536));
    offsets.push(3);
    let mut offset = 8;
    let mut records = 0usize;
    while offset < len {
        if len - offset < 8 {
            bail!("short PyroWave record");
        }
        let header = word(bytes, offset);
        if header == PADDING {
            let size = 8usize
                .checked_add((word(bytes, offset + 4) as usize).saturating_mul(4))
                .context("PyroWave padding overflow")?;
            if size < 8 || size > len - offset {
                bail!("invalid PyroWave padding record");
            }
            offset += size;
            continue;
        }
        let size = (((header >> 16) & 0xfff) as usize) * 4;
        if size < 8
            || size > len - offset
            || header & HEADER_BIT != 0
            || ((header >> 28) & 7) as u8 != seq
        {
            bail!("invalid PyroWave block record");
        }
        offsets.push(offset + 3);
        records += 1;
        offset += size;
    }
    if records != expected_records {
        bail!("PyroWave record count mismatch");
    }
    Ok((seq, offsets))
}

/// Shifts the 3-bit sequence counter in the header and every non-padding record
/// of `bytes` by `+delta` modulo 8 in place, skipping `0xFFFF_FFFF` padding.
/// Validates the bitstream before mutating any byte so errors leave `bytes`
/// untouched.
pub(super) fn shift_sequence(bytes: &mut [u8], delta: u8) -> Result<()> {
    let delta = delta & 7;
    if delta == 0 {
        return Ok(());
    }
    let len = bytes.len();
    if len < 8
        || !len.is_multiple_of(4)
        || word(bytes, 0) & HEADER_BIT == 0
        || word(bytes, 0) == PADDING
        || (word(bytes, 4) >> 24) & 3 != 0
    {
        bail!("invalid PyroWave sequence header");
    }
    let old_seq = ((word(bytes, 0) >> 28) & 7) as u8;
    let expected_records = (word(bytes, 4) & 0x00ff_ffff) as usize;
    let mut offset = 8;
    let mut records = 0usize;
    while offset < len {
        if len - offset < 8 {
            bail!("short PyroWave record");
        }
        let header = word(bytes, offset);
        if header == PADDING {
            let size = 8usize
                .checked_add((word(bytes, offset + 4) as usize).saturating_mul(4))
                .context("PyroWave padding overflow")?;
            if size < 8 || size > len - offset {
                bail!("invalid PyroWave padding record");
            }
            offset += size;
            continue;
        }
        let size = (((header >> 16) & 0xfff) as usize) * 4;
        if size < 8
            || size > len - offset
            || header & HEADER_BIT != 0
            || ((header >> 28) & 7) as u8 != old_seq
        {
            bail!("invalid PyroWave block record");
        }
        records += 1;
        offset += size;
    }
    if records != expected_records {
        bail!("PyroWave record count mismatch");
    }
    let new_seq = (old_seq + delta) & 7;
    bytes[3] = patch_byte(bytes[3], new_seq);
    let mut offset = 8;
    while offset < len {
        let header = word(bytes, offset);
        if header == PADDING {
            offset += 8 + (word(bytes, offset + 4) as usize) * 4;
            continue;
        }
        let size = (((header >> 16) & 0xfff) as usize) * 4;
        bytes[offset + 3] = patch_byte(bytes[offset + 3], new_seq);
        offset += size;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CursorKey {
    position: [i32; 2],
    width: u32,
    height: u32,
    logic: bool,
    pixels_ptr: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OverlayKey {
    revision: u64,
    origin: [u32; 2],
    size: [u32; 2],
    scale: u32,
    rgba_ptr: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct MetaKey {
    source: (u32, u32, Pixel),
    display_in_hdr: Option<bool>,
    origin: u64,
    ready: Option<(usize, u64)>,
    luminance: [u32; 2],
    cursor: Option<CursorKey>,
    overlay: Option<OverlayKey>,
    kbps: u32,
}

impl MetaKey {
    fn from_inputs(
        image: &GpuImage,
        luminance: [f32; 2],
        overlay: Option<&Overlay>,
        kbps: u32,
    ) -> Self {
        Self {
            source: (image.width, image.height, image.pixel),
            display_in_hdr: image.gpu.display_in_hdr(),
            origin: image.origin,
            ready: image
                .ready
                .as_ref()
                .map(|ready| (ready.fence.as_raw() as usize, ready.value)),
            luminance: [luminance[0].to_bits(), luminance[1].to_bits()],
            cursor: image.cursor.as_ref().map(|cursor| CursorKey {
                position: cursor.position,
                width: cursor.width,
                height: cursor.height,
                logic: cursor.logic,
                pixels_ptr: Arc::as_ptr(&cursor.pixels).cast::<u8>() as usize,
            }),
            overlay: overlay.map(|overlay| OverlayKey {
                revision: overlay.revision,
                origin: overlay.origin,
                size: overlay.size,
                scale: overlay.scale,
                rgba_ptr: Arc::as_ptr(&overlay.rgba).cast::<u8>() as usize,
            }),
            kbps,
        }
    }
}

/// Content key identifying an encoded repeat frame. Holds strong references to
/// the capture texture and cursor/overlay buffers while cached so neither the
/// capture pool nor allocator reuse can alias stale pixels.
#[derive(Clone)]
pub(super) struct RepeatKey {
    source_texture: Arc<ID3D11Texture2D>,
    _cursor_pixels: Option<Arc<[u8]>>,
    _overlay_rgba: Option<Arc<[u8]>>,
    meta: MetaKey,
}

impl std::fmt::Debug for RepeatKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RepeatKey")
            .field("texture", &(Arc::as_ptr(&self.source_texture) as usize))
            .field("meta", &self.meta)
            .finish()
    }
}

impl PartialEq for RepeatKey {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.source_texture, &other.source_texture) && self.meta == other.meta
    }
}
impl Eq for RepeatKey {}

impl RepeatKey {
    pub(super) fn meta_for(
        image: &GpuImage,
        luminance: [f32; 2],
        overlay: Option<&Overlay>,
        kbps: u32,
    ) -> MetaKey {
        MetaKey::from_inputs(image, luminance, overlay, kbps)
    }

    pub(super) fn with_meta(image: &GpuImage, overlay: Option<&Overlay>, meta: MetaKey) -> Self {
        Self {
            source_texture: image.texture.clone(),
            _cursor_pixels: image.cursor.as_ref().map(|c| c.pixels.clone()),
            _overlay_rgba: overlay.map(|o| o.rgba.clone()),
            meta,
        }
    }

    pub(super) fn matches_meta(&self, image: &GpuImage, meta: &MetaKey) -> bool {
        Arc::ptr_eq(&self.source_texture, &image.texture) && &self.meta == meta
    }
}

struct CachedFrame<K = RepeatKey> {
    key: K,
    bytes: Vec<u8>,
    sdk_seq: u8,
    seq_offsets: Vec<usize>,
}

/// Per-encoder repeat-frame cache and persistent 3-bit sequence offset.
pub(super) struct RepeatCacheCore<K = RepeatKey> {
    cached: Option<CachedFrame<K>>,
    seq_offset: u8,
}

impl<K> Default for RepeatCacheCore<K> {
    fn default() -> Self {
        Self {
            cached: None,
            seq_offset: 0,
        }
    }
}

pub(super) type RepeatCache = RepeatCacheCore<RepeatKey>;

impl<K> RepeatCacheCore<K> {
    /// Drop the cached frame while preserving `seq_offset` so subsequent SDK
    /// frames continue monotonically from the last emitted sequence number.
    pub(super) fn clear(&mut self) {
        self.cached = None;
    }

    pub(super) fn seq_offset(&self) -> u8 {
        self.seq_offset
    }

    /// Store the flat wire bytes emitted for a repeat frame whose raw SDK
    /// bitstream was shifted by `flight_seq_offset`.
    pub(super) fn store(&mut self, key: K, bytes: Vec<u8>, flight_seq_offset: u8) -> Result<()> {
        let (wire_seq, seq_offsets) = scan_sequence_offsets(&bytes)?;
        let sdk_seq = (wire_seq + 8 - (flight_seq_offset & 7)) & 7;
        self.cached = Some(CachedFrame {
            key,
            bytes,
            sdk_seq,
            seq_offsets,
        });
        Ok(())
    }

    fn matches_with(&self, limit: usize, predicate: impl FnOnce(&K) -> bool) -> bool {
        self.cached
            .as_ref()
            .is_some_and(|cached| cached.bytes.len() <= limit && predicate(&cached.key))
    }

    fn try_hit_with(
        &mut self,
        limit: usize,
        predicate: impl FnOnce(&K) -> bool,
    ) -> Option<Vec<u8>> {
        let cached = self.cached.as_ref()?;
        if cached.bytes.len() > limit || !predicate(&cached.key) {
            return None;
        }
        self.seq_offset = (self.seq_offset + 1) & 7;
        let target_seq = (cached.sdk_seq + self.seq_offset) & 7;
        let mut out = crate::frame_pool::copy(&cached.bytes);
        for &offset in &cached.seq_offsets {
            out[offset] = patch_byte(out[offset], target_seq);
        }
        Some(out)
    }
}

#[cfg(test)]
impl<K: PartialEq> RepeatCacheCore<K> {
    pub(super) fn matches_key(&self, key: &K, limit: usize) -> bool {
        self.matches_with(limit, |cached| cached == key)
    }

    /// Emit a sequence-patched copy of the cached frame if `key` matches and
    /// the cached wire length fits `limit`.
    pub(super) fn try_hit_key(&mut self, key: &K, limit: usize) -> Option<Vec<u8>> {
        self.try_hit_with(limit, |cached| cached == key)
    }
}

impl RepeatCache {
    pub(super) fn matches_meta(&self, image: &GpuImage, meta: &MetaKey, limit: usize) -> bool {
        self.matches_with(limit, |key| key.matches_meta(image, meta))
    }

    /// Emit a sequence-patched copy of the cached frame if the precomputed key metadata matches
    /// and the cached wire length fits `limit`.
    pub(super) fn try_hit_meta(
        &mut self,
        image: &GpuImage,
        meta: &MetaKey,
        limit: usize,
    ) -> Option<Vec<u8>> {
        self.try_hit_with(limit, |key| key.matches_meta(image, meta))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use butterpollo_media_policy::pyrowave::Framing;

    /// Build a minimal valid PyroWave record bitstream with `seq`, `records`
    /// block records, and optional `padding_words` padding inserted after the
    /// first record.
    fn make_frame(
        seq: u8,
        width: u32,
        height: u32,
        records: usize,
        padding_words: Option<u32>,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        let w0 = HEADER_BIT
            | (u32::from(seq & 7) << 28)
            | (((height - 1) & 0x3fff) << 14)
            | ((width - 1) & 0x3fff);
        let w1 = records as u32;
        out.extend_from_slice(&w0.to_le_bytes());
        out.extend_from_slice(&w1.to_le_bytes());
        for i in 0..records {
            let words = 3u32; // 12 bytes (header + meta + 1 payload word)
            let rw0 = (u32::from(seq & 7) << 28) | ((words & 0xfff) << 16) | (i as u32 & 0xffff);
            let rw1 = (i as u32) << 8;
            out.extend_from_slice(&rw0.to_le_bytes());
            out.extend_from_slice(&rw1.to_le_bytes());
            out.extend_from_slice(&(0xa5a5_0000u32 | i as u32).to_le_bytes());
            if i == 0
                && let Some(pad_words) = padding_words
            {
                out.extend_from_slice(&PADDING.to_le_bytes());
                out.extend_from_slice(&pad_words.to_le_bytes());
                out.resize(out.len() + pad_words as usize * 4, 0);
            }
        }
        out
    }

    #[test]
    fn scan_and_patch_skip_padding_and_preserve_header_bits() -> Result<()> {
        let frame = make_frame(3, 1920, 1080, 4, Some(2));
        let (seq, offsets) = scan_sequence_offsets(&frame)?;
        assert_eq!(seq, 3);
        // Header byte 3 + 4 non-padding records = 5 offsets; padding skipped.
        assert_eq!(offsets.len(), 5);
        assert_eq!(offsets[0], 3);
        // Padding starts at offset 8 + 12 = 20 (bytes 20..36); must not be in offsets.
        assert!(!offsets.contains(&23));

        let mut patched = frame.clone();
        for &at in &offsets {
            patched[at] = patch_byte(patched[at], 6);
        }
        // Padding header word remains 0xFFFF_FFFF untouched.
        assert_eq!(word(&patched, 20), PADDING);
        // Header bit 31 and width/height bits are preserved while sequence became 6.
        assert_eq!(
            word(&patched, 0) & 0x8fff_ffff,
            word(&frame, 0) & 0x8fff_ffff
        );
        assert_eq!((word(&patched, 0) >> 28) & 7, 6);
        let (new_seq, new_offsets) = scan_sequence_offsets(&patched)?;
        assert_eq!((new_seq, new_offsets), (6, offsets));
        // Patched frame still passes media-policy's record parser cleanly.
        Framing::records(&patched, 1024, 1_000_000, false)?;
        Ok(())
    }

    #[test]
    fn shift_sequence_wraps_mod_8_across_all_pairs() -> Result<()> {
        for start in 0..8u8 {
            for delta in 0..8u8 {
                let mut frame = make_frame(start, 640, 360, 3, Some(1));
                let orig_header_other = word(&frame, 0) & 0x8fff_ffff;
                shift_sequence(&mut frame, delta)?;
                let (got, _) = scan_sequence_offsets(&frame)?;
                assert_eq!(got, (start + delta) & 7);
                assert_eq!(word(&frame, 0) & 0x8fff_ffff, orig_header_other);
                assert_eq!(word(&frame, 20), PADDING);
            }
        }
        Ok(())
    }

    #[test]
    fn repeat_cache_hits_and_fresh_resume_preserve_monotonic_sequence() -> Result<()> {
        let mut cache: RepeatCacheCore<u32> = RepeatCacheCore::default();
        let mut sdk_seq = 0u8;
        let mut emitted = Vec::new();
        let limit = 1_000_000;

        // Frame 0: fresh SDK encode (not cached yet)
        let mut f0 = make_frame(sdk_seq, 320, 180, 2, None);
        sdk_seq = (sdk_seq + 1) & 7;
        shift_sequence(&mut f0, cache.seq_offset())?;
        emitted.push(scan_sequence_offsets(&f0)?.0);

        // Frame 1: repeat #1 (SDK encode at flight_seq_offset + store into RepeatCacheCore)
        let flight_offset = cache.seq_offset();
        let mut f1 = make_frame(sdk_seq, 320, 180, 2, Some(1));
        sdk_seq = (sdk_seq + 1) & 7;
        shift_sequence(&mut f1, flight_offset)?;
        emitted.push(scan_sequence_offsets(&f1)?.0);
        cache.store(42, f1, flight_offset)?;
        assert!(cache.matches_key(&42, limit));

        // Frames 2..=10: 9 consecutive cached repeat hits through RepeatCacheCore::try_hit_key
        for _ in 0..9 {
            let hit = cache
                .try_hit_key(&42, limit)
                .expect("cached key must hit cleanly within limit");
            emitted.push(scan_sequence_offsets(&hit)?.0);
        }

        // Different key misses; clearing cache preserves seq_offset for subsequent fresh frames.
        assert!(cache.try_hit_key(&99, limit).is_none());
        cache.clear();

        // Frames 11..=13: fresh SDK encodes resume monotonically using preserved cache.seq_offset()
        for _ in 0..3 {
            let mut fresh = make_frame(sdk_seq, 320, 180, 2, None);
            sdk_seq = (sdk_seq + 1) & 7;
            shift_sequence(&mut fresh, cache.seq_offset())?;
            emitted.push(scan_sequence_offsets(&fresh)?.0);
        }

        let expected: Vec<u8> = (0..emitted.len()).map(|i| (i as u8) & 7).collect();
        assert_eq!(emitted, expected);
        Ok(())
    }

    #[test]
    fn scan_and_shift_reject_malformed_bitstreams() {
        // Short header
        assert!(scan_sequence_offsets(&[0; 4]).is_err());
        assert!(shift_sequence(&mut [0; 4], 1).is_err());

        // Missing header bit 31
        let mut bad = make_frame(1, 320, 180, 1, None);
        bad[3] &= 0x7f;
        assert!(scan_sequence_offsets(&bad).is_err());

        // Mismatched sequence in record 1
        let mut bad = make_frame(2, 320, 180, 2, None);
        bad[8 + 12 + 3] = patch_byte(bad[8 + 12 + 3], 5);
        assert!(scan_sequence_offsets(&bad).is_err());
        let before = bad.clone();
        assert!(shift_sequence(&mut bad, 1).is_err());
        assert_eq!(bad, before, "failed shift must leave bytes unchanged");

        // Wrong record count
        let mut bad = make_frame(0, 320, 180, 2, None);
        bad[4..8].copy_from_slice(&3u32.to_le_bytes());
        assert!(scan_sequence_offsets(&bad).is_err());

        // Padding size past end of buffer
        let mut bad = make_frame(0, 320, 180, 2, Some(1));
        bad[24..28].copy_from_slice(&100u32.to_le_bytes());
        assert!(scan_sequence_offsets(&bad).is_err());
    }
}
