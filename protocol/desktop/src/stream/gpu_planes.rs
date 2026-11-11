//! The plane sets the GPU PyroWave decoder writes and the presenter reads (briefs G2 and G3).
//!
//! The decoder thread takes a free set (`Planes::acquire`), decodes into it and hands the picture
//! on holding a `Lease`; the presenting thread waits for the decode fence, draws from the set's
//! textures and gives the set back with the value it signalled on the reuse fence
//! (`Lease::release_after`), which the next decode into the set waits for on the GPU. A picture
//! that is dropped unshown frees its set too, so the queue, the pacer and the presenter can drop
//! pictures in any order without leaking a set.
//!
//! On Linux the sets are images on the renderer's own Vulkan device when the decoder shares it
//! (`zero_copy.rs`). The presenter samples them in place: it takes the picture's lease over
//! (`Lease::hand_over`), keeps it while the picture is shown and gives the set back with the reuse
//! value signalled behind the last draw that read it when the next picture replaces it
//! (`Handover`). With `NIGHTJAR_PYROWAVE_SAMPLE_PLANES=0` it copies the set out instead and
//! releases it as soon as the copy is queued with its reuse signal, as before. Or else the sets
//! are CPU buffers the decoder reads the planes back into, which the presenter uploads while it
//! holds the `Lease`, and dropping the lease frees the set.

#![cfg_attr(target_os = "macos", allow(dead_code, reason = "the Vulkan PyroWave SDK uses these leases on Linux"))]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(windows)]
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

#[cfg(windows)]
use windows::Win32::Graphics::Direct3D11::{ID3D11Fence, ID3D11Texture2D};

use super::pyrowave_sdk::GpuDecoder;
#[cfg(unix)]
use super::pyrowave_sdk::{zero_copy::SharedImages, GpuConfig};

/// Which sets are free, and the reuse-fence value a decode into each must wait for (0: none).
/// Sets come back in the order they were freed, so the one waited for longest is reused first.
pub struct Pool {
    free: VecDeque<usize>,
    after: Vec<u64>,
    /// Readers that hold a set without holding its lease (a slice waiting to be drawn): a set
    /// released while one holds it goes back to the pool when the last of them lets go.
    pins: Vec<u32>,
    released: Vec<bool>,
}

impl Pool {
    pub fn new(sets: usize) -> Self {
        Self { free: (0..sets).collect(), after: vec![0; sets], pins: vec![0; sets], released: vec![false; sets] }
    }

    pub fn acquire(&mut self) -> Option<(usize, u64)> {
        let set = self.free.pop_front()?;
        Some((set, self.after[set]))
    }

    /// Frees `set`; `after` is the value signalled once the last draw that read it has run.
    pub fn release(&mut self, set: usize, after: Option<u64>) {
        debug_assert!(!self.free.contains(&set), "plane set {set} freed twice");
        if let Some(value) = after { self.after[set] = self.after[set].max(value); }
        if self.pins[set] > 0 { self.released[set] = true; } else { self.free.push_back(set); }
    }

    /// A reader holds `set` until `unpin`.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn pin(&mut self, set: usize) {
        self.pins[set] += 1;
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn unpin(&mut self, set: usize) {
        debug_assert!(self.pins[set] > 0, "plane set {set} unpinned more than pinned");
        self.pins[set] = self.pins[set].saturating_sub(1);
        if self.pins[set] == 0 && std::mem::take(&mut self.released[set]) { self.free.push_back(set); }
    }

    /// The reuse-fence value a decode into `set` has to wait for now.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn after(&self, set: usize) -> u64 {
        self.after[set]
    }

    /// A draw read `set` and will signal `value`: whatever decodes into it next waits for that.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn read_until(&mut self, set: usize, value: u64) {
        self.after[set] = self.after[set].max(value);
    }

    #[cfg(test)]
    pub fn free_count(&self) -> usize {
        self.free.len()
    }
}

/// The decoder's output sets and fences, shared by the decoder thread and the presenter. The
/// presenter's references keep the D3D11 objects alive after the decoder is gone.
#[cfg(windows)]
pub struct Planes {
    sets: Vec<[ID3D11Texture2D; 3]>,
    decode_fence: ID3D11Fence,
    reuse_fence: ID3D11Fence,
    pool: Mutex<Pool>,
    reuse_value: AtomicU64,
}

#[cfg(windows)]
impl Planes {
    pub fn new(gpu: &GpuDecoder) -> Arc<Self> {
        Arc::new(Self {
            sets: (0..gpu.slot_count()).map(|set| gpu.planes(set).clone()).collect(),
            decode_fence: gpu.fence().clone(),
            reuse_fence: gpu.reuse_fence().clone(),
            pool: Mutex::new(Pool::new(gpu.slot_count())),
            reuse_value: AtomicU64::new(0),
        })
    }

    #[cfg(all(test, windows))]
    pub fn set_count(&self) -> usize {
        self.sets.len()
    }

    #[cfg(test)]
    pub fn free_count(&self) -> usize {
        self.pool().free_count()
    }

    fn pool(&self) -> std::sync::MutexGuard<'_, Pool> {
        self.pool.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A free set and the reuse value the decode into it must wait for, or None when every set
    /// is held by a picture.
    pub fn acquire(self: &Arc<Self>) -> Option<(Lease, u64)> {
        let (set, after) = self.pool().acquire()?;
        Some((Lease { planes: self.clone(), set, freed: AtomicBool::new(false) }, after))
    }

    /// The next value for the presenter to signal on the reuse fence (only grows).
    pub fn next_reuse_value(&self) -> u64 {
        self.reuse_value.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// What a decode into `set` has to wait for on the reuse fence right now: the draws that read
    /// it so far, which a slice's draw adds to while the frame is still being decoded.
    pub fn wait_for(&self, set: usize) -> u64 {
        self.pool().after(set)
    }

    pub fn textures(&self, set: usize) -> &[ID3D11Texture2D; 3] {
        &self.sets[set]
    }

    pub fn decode_fence(&self) -> &ID3D11Fence {
        &self.decode_fence
    }

    pub fn reuse_fence(&self) -> &ID3D11Fence {
        &self.reuse_fence
    }
}

/// A reader of a plane set that is not its owner (a slice waiting to be drawn while its frame is
/// still decoding): the set is not reused until the reader is dropped, even if the frame is
/// abandoned and its lease dropped.
#[cfg(windows)]
pub struct Pin {
    planes: Arc<Planes>,
    set: usize,
}

#[cfg(windows)]
impl Pin {
    pub fn set(&self) -> usize { self.set }

    pub fn planes(&self) -> &Arc<Planes> { &self.planes }

    /// A draw from the set will signal `value` on the reuse fence: the next decode waits for it.
    pub fn read_until(&self, value: u64) {
        self.planes.pool().read_until(self.set, value);
    }
}

#[cfg(windows)]
impl Drop for Pin {
    fn drop(&mut self) {
        self.planes.pool().unpin(self.set);
    }
}

/// One plane set held by a picture.
#[cfg(windows)]
pub struct Lease {
    planes: Arc<Planes>,
    set: usize,
    freed: AtomicBool,
}

#[cfg(windows)]
impl Lease {
    pub fn set(&self) -> usize {
        self.set
    }

    pub fn planes(&self) -> &Arc<Planes> {
        &self.planes
    }

    #[cfg(test)]
    pub fn textures(&self) -> &[ID3D11Texture2D; 3] {
        &self.planes.sets[self.set]
    }

    #[cfg(test)]
    pub fn decode_fence(&self) -> &ID3D11Fence {
        &self.planes.decode_fence
    }

    /// A reader for the slices of the frame that will fill this set.
    pub fn pin(&self) -> Pin {
        self.planes.pool().pin(self.set);
        Pin { planes: self.planes.clone(), set: self.set }
    }

    /// Gives the set back after the presenter queued a `Signal` of `value` on the reuse fence
    /// behind its last draw from it: the next decode into the set waits for that value.
    pub fn release_after(&self, value: u64) {
        if !self.freed.swap(true, Ordering::AcqRel) { self.planes.pool().release(self.set, Some(value)); }
    }

    /// Moves the set into a lease of the caller's, for a presenter that draws it at present time
    /// and for as long as the picture stays on screen: this lease no longer frees it. None if it
    /// was given back already.
    pub fn hand_over(&self) -> Option<Lease> {
        (!self.freed.swap(true, Ordering::AcqRel)).then(|| Lease { planes: self.planes.clone(), set: self.set, freed: AtomicBool::new(false) })
    }
}

#[cfg(windows)]
impl Drop for Lease {
    fn drop(&mut self) {
        if !self.freed.swap(true, Ordering::AcqRel) { self.planes.pool().release(self.set, None); }
    }
}

/// The decoder's output sets, shared by the decoder thread (which fills a set it holds a `Lease`
/// on) and the presenter (which reads it while it holds the picture).
#[cfg(unix)]
pub struct Planes {
    config: GpuConfig,
    store: Store,
    pool: Mutex<Pool>,
}

#[cfg(unix)]
enum Store {
    /// One CPU buffer per plane, which the decoder reads the planes back into.
    Buffers(Vec<Mutex<[Vec<u8>; 3]>>),
    /// The decoder's images on the renderer's device.
    Images(Arc<SharedImages>),
}

#[cfg(unix)]
impl Planes {
    pub fn new(gpu: &GpuDecoder) -> Arc<Self> {
        let config = gpu.config();
        let store = match gpu.images() {
            Some(images) => Store::Images(images.clone()),
            None => Store::Buffers((0..gpu.slot_count()).map(|_| Mutex::new([0, 1, 2].map(|plane| vec![0; config.plane_bytes(plane)]))).collect()),
        };
        Arc::new(Self { config, store, pool: Mutex::new(Pool::new(gpu.slot_count())) })
    }

    pub fn config(&self) -> GpuConfig {
        self.config
    }

    /// The sets on the renderer's device, when they are there and not in CPU buffers.
    pub fn images(&self) -> Option<&Arc<SharedImages>> {
        match &self.store {
            Store::Images(images) => Some(images),
            Store::Buffers(_) => None,
        }
    }

    #[cfg(test)]
    pub fn free_count(&self) -> usize {
        self.pool().free_count()
    }

    fn pool(&self) -> std::sync::MutexGuard<'_, Pool> {
        self.pool.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A free set and the reuse value the decode into it must wait for (always 0 for CPU
    /// buffers), or None when every set is held by a picture.
    pub fn acquire(self: &Arc<Self>) -> Option<(Lease, u64)> {
        let (set, after) = self.pool().acquire()?;
        Some((Lease { planes: self.clone(), set, freed: AtomicBool::new(false) }, after))
    }
}

/// There are no slices to read on this platform.
#[cfg(unix)]
pub struct Pin;

/// One plane set held by a picture.
#[cfg(unix)]
pub struct Lease {
    planes: Arc<Planes>,
    set: usize,
    freed: AtomicBool,
}

#[cfg(unix)]
impl Lease {
    pub fn set(&self) -> usize {
        self.set
    }

    pub fn planes(&self) -> &Arc<Planes> {
        &self.planes
    }

    /// Runs `f` on the set's three plane buffers (tightly packed rows, `GpuConfig::plane_bytes`),
    /// or returns None if the set is images on the GPU.
    pub fn with_buffers<R>(&self, f: impl FnOnce(&mut [Vec<u8>; 3]) -> R) -> Option<R> {
        match &self.planes.store {
            Store::Buffers(sets) => Some(f(&mut sets[self.set].lock().unwrap_or_else(|e| e.into_inner()))),
            Store::Images(_) => None,
        }
    }

    /// Gives the set back (dropping the lease does the same).
    pub fn release(&self) {
        if !self.freed.swap(true, Ordering::AcqRel) { self.planes.pool().release(self.set, None); }
    }

    /// Gives the set back after the presenter signalled `value` on the reuse timeline behind its
    /// last read of it (its copy, or the last draw that sampled it): the next decode into the set
    /// waits for that value.
    pub fn release_after(&self, value: u64) {
        if !self.freed.swap(true, Ordering::AcqRel) { self.planes.pool().release(self.set, Some(value)); }
    }

    /// Moves the set into a lease of the caller's, for a presenter that reads it in place after
    /// the picture is gone (`Handover`): this lease no longer frees it. None if it was given back
    /// already.
    pub fn hand_over(&self) -> Option<Lease> {
        (!self.freed.swap(true, Ordering::AcqRel)).then(|| Lease { planes: self.planes.clone(), set: self.set, freed: AtomicBool::new(false) })
    }
}

#[cfg(unix)]
impl Drop for Lease {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(unix)]
impl HeldSet for Lease {
    fn set(&self) -> usize {
        self.set
    }

    fn give_back(self, after: u64) {
        self.release_after(after);
    }

    fn abandon(self) {
        // Never back in the pool: the decoder must not write a set in a layout it does not expect.
        self.freed.store(true, Ordering::Release);
    }
}

/// A plane set the presenter holds while it samples it in place (Linux): it goes back to the
/// decoder only with a reuse value signalled behind the draws that read it.
#[cfg_attr(not(unix), allow(dead_code))]
pub trait HeldSet {
    fn set(&self) -> usize;
    /// Back to the pool; the next decode into the set waits for `after` on the reuse timeline.
    fn give_back(self, after: u64);
    /// Never back to the pool (its return could not be queued: the device is lost or hung).
    fn abandon(self);
}

/// The GPU work around a set the presenter samples in place (`zero_copy::SharedImages`).
#[cfg_attr(not(unix), allow(dead_code))]
pub trait SetGpu {
    /// Submits what the renderer has queued, so every draw recorded so far is ahead of what
    /// follows on the queue.
    fn flush(&mut self) -> Result<(), String>;
    /// Makes `set` readable by the draws queued after this (after its decode).
    fn sampled(&mut self, set: usize) -> Result<(), String>;
    /// Makes `set` writable by the decoder again behind everything queued so far, and returns the
    /// reuse value signalled once that is done.
    fn returned(&mut self, set: usize) -> Result<u64, String>;
}

/// The set the shown picture's planes are in, held from the picture that brought it until the
/// next one replaces it or the presenter stops; then given back behind the last draw of it.
#[cfg_attr(not(unix), allow(dead_code))]
pub struct Handover<L: HeldSet> {
    held: Option<L>,
    /// Why a set could not be given back, said once.
    said: bool,
}

impl<L: HeldSet> Default for Handover<L> {
    fn default() -> Self {
        Self { held: None, said: false }
    }
}

#[cfg_attr(not(unix), allow(dead_code))]
impl<L: HeldSet> Handover<L> {
    /// The set held, if any.
    pub fn held(&self) -> Option<usize> {
        self.held.as_ref().map(HeldSet::set)
    }

    /// Shows `next`: makes its set readable, then gives the one it replaces back behind the draws
    /// recorded so far. On an error `next` goes back unread (dropped: the caller's lease frees it
    /// as it is, never sampled) and the held set stays, so the last picture stays drawable.
    pub fn show(&mut self, gpu: &mut impl SetGpu, next: L) -> Result<(), String> {
        gpu.sampled(next.set())?;
        if let Some(old) = self.held.replace(next) {
            self.give_back(gpu, old);
        }
        Ok(())
    }

    /// Gives the held set back (the stream ends, the decoder changes, the presenter goes).
    pub fn finish(&mut self, gpu: &mut impl SetGpu) {
        if let Some(old) = self.held.take() {
            self.give_back(gpu, old);
        }
    }

    fn give_back(&mut self, gpu: &mut impl SetGpu, old: L) {
        let set = old.set();
        match gpu.flush().and_then(|()| gpu.returned(set)) {
            Ok(after) => old.give_back(after),
            Err(error) => {
                if !std::mem::replace(&mut self.said, true) {
                    eprintln!("nightjar: PyroWave plane set {set} cannot go back to the decoder ({error}); it stays out of use");
                }
                old.abandon();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_are_handed_out_once_and_come_back_in_order() {
        let mut pool = Pool::new(3);
        let taken: Vec<_> = std::iter::from_fn(|| pool.acquire()).collect();
        assert_eq!(taken, [(0, 0), (1, 0), (2, 0)]);
        assert_eq!(pool.acquire(), None, "an empty pool hands out nothing");
        pool.release(1, None);
        pool.release(0, Some(7));
        assert_eq!(pool.free_count(), 2);
        assert_eq!(pool.acquire(), Some((1, 0)));
        assert_eq!(pool.acquire(), Some((0, 7)), "the set a draw read waits for the draw's signal");
    }

    #[test]
    fn a_set_waits_for_the_largest_value_it_was_released_with() {
        let mut pool = Pool::new(1);
        for value in [Some(5), None, Some(3)] {
            let (set, _) = pool.acquire().unwrap();
            pool.release(set, value);
        }
        assert_eq!(pool.acquire(), Some((0, 5)), "an unshown picture or an older value never lowers the wait");
    }

    #[test]
    fn a_pinned_set_is_not_reused_until_its_readers_let_go() {
        let mut pool = Pool::new(1);
        let (set, _) = pool.acquire().unwrap();
        pool.pin(set);
        pool.pin(set);
        pool.release(set, Some(4));
        assert_eq!(pool.acquire(), None, "released while read: not free yet");
        pool.unpin(set);
        assert_eq!(pool.acquire(), None, "one reader left");
        pool.unpin(set);
        assert_eq!(pool.acquire(), Some((0, 4)), "free when the last reader lets go, with the wait it was released with");
    }

    #[test]
    fn a_reader_that_lets_go_of_a_held_set_does_not_free_it() {
        let mut pool = Pool::new(1);
        let (set, _) = pool.acquire().unwrap();
        pool.pin(set);
        pool.unpin(set);
        assert_eq!(pool.acquire(), None, "the lease still holds it");
        pool.release(set, None);
        assert_eq!(pool.acquire(), Some((0, 0)));
    }

    #[test]
    fn a_slice_draw_raises_the_wait_of_the_next_decode_into_the_set() {
        let mut pool = Pool::new(1);
        let (set, _) = pool.acquire().unwrap();
        pool.read_until(set, 3);
        pool.read_until(set, 2);
        assert_eq!(pool.after(set), 3, "never lowered");
        pool.release(set, Some(1));
        assert_eq!(pool.acquire(), Some((0, 3)));
    }

    #[cfg(unix)]
    #[test]
    fn a_lease_holds_its_buffers_until_it_is_dropped_or_released() {
        let planes = unix_planes(2);
        let (first, _) = planes.acquire().unwrap();
        let (second, _) = planes.acquire().unwrap();
        assert!(planes.acquire().is_none(), "every set is held by a picture");
        first.with_buffers(|buffers| buffers[0][0] = 7).unwrap();
        assert_eq!(first.with_buffers(|buffers| buffers[0][0]), Some(7));
        assert_eq!([0, 1, 2].map(|p| first.with_buffers(|buffers| buffers[p].len())), [Some(64 * 48), Some(32 * 24), Some(32 * 24)]);
        assert!(planes.images().is_none(), "CPU buffers are not GPU images");
        let (set, _) = (first.set(), first.release());
        first.release();
        assert_eq!(planes.free_count(), 1, "releasing twice frees the set once");
        drop(second);
        assert_eq!(planes.free_count(), 2);
        drop(first);
        assert_eq!(planes.free_count(), 2, "a released lease does not free again on drop");
        let (again, _) = planes.acquire().unwrap();
        assert_eq!(again.set(), set, "sets are reused in the order they were freed");
    }

    /// A set the presenter copied out of on the GPU comes back with its reuse signal, which the
    /// next decode into it waits for; a picture dropped unshown adds no wait.
    #[cfg(unix)]
    #[test]
    fn a_set_released_after_a_copy_hands_its_reuse_value_to_the_next_decode() {
        let planes = unix_planes(1);
        let (lease, after) = planes.acquire().unwrap();
        assert_eq!(after, 0);
        lease.release_after(5);
        lease.release_after(9);
        drop(lease);
        let (lease, after) = planes.acquire().unwrap();
        assert_eq!(after, 5, "released once, with the first value");
        drop(lease);
        assert_eq!(planes.acquire().map(|(_, after)| after), Some(5), "an unshown picture does not lower the wait");
    }

    /// The GPU side of a `Handover`, as a log of what was queued, in order.
    #[derive(Default)]
    struct Gpu {
        log: Vec<String>,
        value: u64,
        refuse_sampled: bool,
        refuse_returned: bool,
    }

    impl SetGpu for Gpu {
        fn flush(&mut self) -> Result<(), String> {
            self.log.push("flush".into());
            Ok(())
        }

        fn sampled(&mut self, set: usize) -> Result<(), String> {
            if self.refuse_sampled { return Err("device lost".into()); }
            self.log.push(format!("sampled {set}"));
            Ok(())
        }

        fn returned(&mut self, set: usize) -> Result<u64, String> {
            if self.refuse_returned { return Err("device lost".into()); }
            self.value += 1;
            self.log.push(format!("returned {set} at {}", self.value));
            Ok(self.value)
        }
    }

    /// A set the presenter holds, over a pool shared with the "decoder".
    struct Held {
        set: usize,
        pool: Arc<Mutex<Pool>>,
        freed: bool,
    }

    impl HeldSet for Held {
        fn set(&self) -> usize { self.set }

        fn give_back(mut self, after: u64) {
            self.freed = true;
            self.pool.lock().unwrap().release(self.set, Some(after));
        }

        fn abandon(mut self) {
            self.freed = true;
        }
    }

    impl Drop for Held {
        /// A lease dropped unread goes back without a wait, as `Lease`'s drop does.
        fn drop(&mut self) {
            if !self.freed { self.pool.lock().unwrap().release(self.set, None); }
        }
    }

    fn take(pool: &Arc<Mutex<Pool>>) -> Held {
        let (set, _) = pool.lock().unwrap().acquire().expect("a free set");
        Held { set, pool: pool.clone(), freed: false }
    }

    /// The set a picture is drawn from stays out of the pool while it is shown, and goes back
    /// only with a value signalled after the renderer flushed the draws that read it.
    #[test]
    fn a_sampled_set_goes_back_only_behind_the_draws_that_read_it() {
        let pool = Arc::new(Mutex::new(Pool::new(3)));
        let (mut gpu, mut handover) = (Gpu::default(), Handover::default());
        handover.show(&mut gpu, take(&pool)).unwrap();
        assert_eq!(handover.held(), Some(0));
        assert_eq!(gpu.log, ["sampled 0"], "nothing is given back while the first picture is shown");
        assert_eq!(pool.lock().unwrap().free_count(), 2, "the shown set is not free");
        handover.show(&mut gpu, take(&pool)).unwrap();
        assert_eq!(gpu.log, ["sampled 0", "sampled 1", "flush", "returned 0 at 1"], "the old set's return is queued after the flush");
        assert_eq!(handover.held(), Some(1));
        let mut pool_now = pool.lock().unwrap();
        assert_eq!(pool_now.free_count(), 2);
        assert_eq!(pool_now.acquire(), Some((2, 0)), "the set never read comes first, without a wait");
        assert_eq!(pool_now.acquire(), Some((0, 1)), "the replaced set: its next decode waits for the value behind its last draw");
    }

    /// Pictures that are replaced before they are drawn (skipped), dropped before they reach the
    /// presenter, and the one shown when the stream ends all come back; none is lost.
    #[test]
    fn replaced_dropped_and_last_sets_all_come_back() {
        let pool = Arc::new(Mutex::new(Pool::new(4)));
        let (mut gpu, mut handover) = (Gpu::default(), Handover::default());
        for _ in 0..10 {
            let dropped = take(&pool);
            handover.show(&mut gpu, take(&pool)).unwrap();
            drop(dropped);
        }
        assert_eq!(pool.lock().unwrap().free_count(), 3, "only the shown set is held");
        handover.finish(&mut gpu);
        handover.finish(&mut gpu);
        assert_eq!(handover.held(), None);
        assert_eq!(pool.lock().unwrap().free_count(), 4, "the stream's end gives the last set back, once");
        assert_eq!(gpu.log.iter().filter(|line| line.starts_with("returned")).count(), 10);
        assert_eq!(gpu.log.last().map(String::as_str), Some("returned 3 at 10"));
        assert_eq!(gpu.log[gpu.log.len() - 2], "flush", "the last return follows a flush too");
    }

    /// A picture whose set cannot be made readable is not shown: it goes back unread, and the
    /// held set (the last picture, still on screen) stays.
    #[test]
    fn a_set_that_cannot_be_sampled_goes_back_unread_and_the_shown_one_stays() {
        let pool = Arc::new(Mutex::new(Pool::new(2)));
        let (mut gpu, mut handover) = (Gpu::default(), Handover::default());
        handover.show(&mut gpu, take(&pool)).unwrap();
        gpu.refuse_sampled = true;
        assert!(handover.show(&mut gpu, take(&pool)).is_err());
        assert_eq!(handover.held(), Some(0));
        assert_eq!(pool.lock().unwrap().acquire(), Some((1, 0)), "the refused set is free again, with no wait");
    }

    /// A set whose return cannot be queued (a lost device) never goes back: the decoder must not
    /// write a set still in the presenter's layout, with draws of it possibly pending.
    #[test]
    fn a_set_whose_return_fails_stays_out_of_use() {
        let pool = Arc::new(Mutex::new(Pool::new(2)));
        let (mut gpu, mut handover) = (Gpu::default(), Handover::default());
        handover.show(&mut gpu, take(&pool)).unwrap();
        gpu.refuse_returned = true;
        handover.show(&mut gpu, take(&pool)).unwrap();
        handover.finish(&mut gpu);
        assert_eq!(pool.lock().unwrap().free_count(), 0, "both sets are abandoned, neither is freed unsynchronised");
    }

    /// The presenter takes a picture's set over: the picture's lease no longer frees it, and the
    /// presenter's frees it once.
    #[cfg(unix)]
    #[test]
    fn a_handed_over_lease_frees_its_set_once_through_the_new_owner() {
        let planes = unix_planes(2);
        let (picture, _) = planes.acquire().unwrap();
        let held = picture.hand_over().expect("not given back yet");
        assert!(picture.hand_over().is_none(), "handed over once");
        drop(picture);
        assert_eq!(planes.free_count(), 1, "the picture's lease no longer holds it, the presenter's does");
        HeldSet::give_back(held, 4);
        assert_eq!(planes.free_count(), 2);
        let (set, _) = planes.acquire().unwrap();
        let (second, after) = planes.acquire().unwrap();
        assert_eq!((set.set(), second.set(), after), (1, 0, 4), "the given-back set waits for the presenter's value");
        set.hand_over().unwrap().abandon();
        drop(set);
        assert_eq!(planes.free_count(), 0, "an abandoned set never comes back");
        drop(second);
    }

    #[cfg(unix)]
    fn unix_planes(sets: usize) -> Arc<Planes> {
        let config = GpuConfig { width: 64, height: 48, chroma444: false, ten_bit: false, slots: sets };
        Arc::new(Planes {
            config,
            store: Store::Buffers((0..sets).map(|_| Mutex::new([0, 1, 2].map(|plane| vec![0; config.plane_bytes(plane)]))).collect()),
            pool: Mutex::new(Pool::new(sets)),
        })
    }
}
