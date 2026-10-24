//! The plane sets the GPU PyroWave decoder writes and the presenter reads (briefs G2 and G3).
//!
//! The decoder thread takes a free set (`Planes::acquire`), decodes into it and hands the picture
//! on holding a `Lease`; the presenting thread waits for the decode fence, draws from the set's
//! textures and gives the set back with the value it signalled on the reuse fence
//! (`Lease::release_after`), which the next decode into the set waits for on the GPU. A picture
//! that is dropped unshown frees its set too, so the queue, the pacer and the presenter can drop
//! pictures in any order without leaking a set.
//!
//! On Linux the sets are CPU buffers the decoder reads the planes back into, and the presenter
//! copies them to its textures while it holds the `Lease`; dropping the lease frees the set.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(windows)]
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

#[cfg(windows)]
use windows::Win32::Graphics::Direct3D11::{ID3D11Fence, ID3D11Texture2D};

use super::pyrowave_sdk::GpuDecoder;
#[cfg(unix)]
use super::pyrowave_sdk::GpuConfig;

/// Which sets are free, and the reuse-fence value a decode into each must wait for (0: none).
/// Sets come back in the order they were freed, so the one waited for longest is reused first.
pub struct Pool {
    free: VecDeque<usize>,
    after: Vec<u64>,
}

impl Pool {
    pub fn new(sets: usize) -> Self {
        Self { free: (0..sets).collect(), after: vec![0; sets] }
    }

    pub fn acquire(&mut self) -> Option<(usize, u64)> {
        let set = self.free.pop_front()?;
        Some((set, self.after[set]))
    }

    /// Frees `set`; `after` is the value signalled once the last draw that read it has run.
    pub fn release(&mut self, set: usize, after: Option<u64>) {
        debug_assert!(!self.free.contains(&set), "plane set {set} freed twice");
        if let Some(value) = after { self.after[set] = self.after[set].max(value); }
        self.free.push_back(set);
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

    pub fn textures(&self) -> &[ID3D11Texture2D; 3] {
        &self.planes.sets[self.set]
    }

    pub fn decode_fence(&self) -> &ID3D11Fence {
        &self.planes.decode_fence
    }

    pub fn reuse_fence(&self) -> &ID3D11Fence {
        &self.planes.reuse_fence
    }

    /// Gives the set back after the presenter queued a `Signal` of `value` on the reuse fence
    /// behind its last draw from it: the next decode into the set waits for that value.
    pub fn release_after(&self, value: u64) {
        if !self.freed.swap(true, Ordering::AcqRel) { self.planes.pool().release(self.set, Some(value)); }
    }
}

#[cfg(windows)]
impl Drop for Lease {
    fn drop(&mut self) {
        if !self.freed.swap(true, Ordering::AcqRel) { self.planes.pool().release(self.set, None); }
    }
}

/// The decoder's output sets: one CPU buffer per plane, shared by the decoder thread (which
/// fills a set it holds a `Lease` on) and the presenter (which reads it while it holds the picture).
#[cfg(unix)]
pub struct Planes {
    config: GpuConfig,
    sets: Vec<Mutex<[Vec<u8>; 3]>>,
    pool: Mutex<Pool>,
}

#[cfg(unix)]
impl Planes {
    pub fn new(gpu: &GpuDecoder) -> Arc<Self> {
        let config = gpu.config();
        Arc::new(Self {
            config,
            sets: (0..gpu.slot_count()).map(|_| Mutex::new([0, 1, 2].map(|plane| vec![0; config.plane_bytes(plane)]))).collect(),
            pool: Mutex::new(Pool::new(gpu.slot_count())),
        })
    }

    pub fn config(&self) -> GpuConfig {
        self.config
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

    /// A free set, or None when every set is held by a picture. The second value is the
    /// reuse-fence value the Windows decoder waits for; nothing is shared with the GPU here, so 0.
    pub fn acquire(self: &Arc<Self>) -> Option<(Lease, u64)> {
        let (set, _) = self.pool().acquire()?;
        Some((Lease { planes: self.clone(), set, freed: AtomicBool::new(false) }, 0))
    }
}

/// One plane set held by a picture.
#[cfg(unix)]
pub struct Lease {
    planes: Arc<Planes>,
    set: usize,
    freed: AtomicBool,
}

#[cfg(unix)]
impl Lease {
    #[cfg(test)]
    pub fn set(&self) -> usize {
        self.set
    }

    pub fn planes(&self) -> &Arc<Planes> {
        &self.planes
    }

    /// Runs `f` on the set's three plane buffers (tightly packed rows, `GpuConfig::plane_bytes`).
    pub fn with_buffers<R>(&self, f: impl FnOnce(&mut [Vec<u8>; 3]) -> R) -> R {
        f(&mut self.planes.sets[self.set].lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Gives the set back (dropping the lease does the same).
    pub fn release(&self) {
        if !self.freed.swap(true, Ordering::AcqRel) { self.planes.pool().release(self.set, None); }
    }
}

#[cfg(unix)]
impl Drop for Lease {
    fn drop(&mut self) {
        self.release();
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

    #[cfg(unix)]
    #[test]
    fn a_lease_holds_its_buffers_until_it_is_dropped_or_released() {
        let planes = unix_planes(2);
        let (first, _) = planes.acquire().unwrap();
        let (second, _) = planes.acquire().unwrap();
        assert!(planes.acquire().is_none(), "every set is held by a picture");
        first.with_buffers(|buffers| buffers[0][0] = 7);
        assert_eq!(first.with_buffers(|buffers| buffers[0][0]), 7);
        assert_eq!([0, 1, 2].map(|p| first.with_buffers(|buffers| buffers[p].len())), [64 * 48, 32 * 24, 32 * 24]);
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

    #[cfg(unix)]
    fn unix_planes(sets: usize) -> Arc<Planes> {
        let config = GpuConfig { width: 64, height: 48, chroma444: false, ten_bit: false, slots: sets };
        Arc::new(Planes {
            config,
            sets: (0..sets).map(|_| Mutex::new([0, 1, 2].map(|plane| vec![0; config.plane_bytes(plane)]))).collect(),
            pool: Mutex::new(Pool::new(sets)),
        })
    }
}
