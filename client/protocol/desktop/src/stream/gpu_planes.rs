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
mod tests;
