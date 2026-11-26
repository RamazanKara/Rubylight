//! The framebuffers of the pictures a decoder's pool recycles, kept for the next time the same
//! DMA-BUF comes back: one `ADDFB2` per buffer of the pool instead of one per picture. Bounded,
//! least recently used first out; a framebuffer that may be on the screen is never put out while
//! another can go. What goes out is handed back to be removed once the next commit has taken the
//! screen off it; a new pool (a mode change) and the end of the stream hand back all of them.

/// How many framebuffers a stream keeps. A VA-API pool holds the decoder's references and a few
/// pictures in flight, about 20 for HEVC; a pool larger than this makes some again, as before.
pub(super) const FB_CACHE_CAPACITY: usize = 32;

/// A DMA-BUF a framebuffer was made of: the VA surface, the DMA-BUF's inode (the same buffer
/// exported again has it) and its modifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FbKey {
    pub surface: u32,
    pub inode: u64,
    pub modifier: u64,
}

/// The kept framebuffers (`V`: the framebuffer and what goes with it), most recently used last.
#[derive(Debug)]
pub(super) struct FbCache<V> {
    capacity: usize,
    entries: Vec<(FbKey, V)>,
}

impl<V> FbCache<V> {
    pub fn new(capacity: usize) -> Self {
        Self { capacity: capacity.max(1), entries: Vec::new() }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// The framebuffer kept for `key`, now the most recently used.
    pub fn get(&mut self, key: FbKey) -> Option<&V> {
        let index = self.entries.iter().position(|(k, _)| *k == key)?;
        let entry = self.entries.remove(index);
        self.entries.push(entry);
        self.entries.last().map(|(_, value)| value)
    }

    /// Keeps `value` for `key` as the most recently used, and returns what it put out: an older
    /// one of the same key, and past the capacity the least recently used ones that `busy` does
    /// not hold (the framebuffer on the screen and the one the next commit shows).
    pub fn insert(&mut self, key: FbKey, value: V, busy: impl Fn(&V) -> bool) -> Vec<V> {
        let mut out: Vec<V> = Vec::new();
        if let Some(index) = self.entries.iter().position(|(k, _)| *k == key) {
            out.push(self.entries.remove(index).1);
        }
        self.entries.push((key, value));
        while self.entries.len() > self.capacity {
            let newest = self.entries.len() - 1;
            let Some(index) = self.entries[..newest].iter().position(|(_, v)| !busy(v)) else { break };
            out.push(self.entries.remove(index).1);
        }
        out
    }

    /// Every kept framebuffer, the cache left empty: a new pool's surfaces are other buffers, and
    /// the stream's end frees them all.
    pub fn take_all(&mut self) -> Vec<V> {
        self.entries.drain(..).map(|(_, value)| value).collect()
    }
}

/// The inode of DMA-BUF `fd`: the same buffer exported again has the same one. None when it
/// cannot be read.
pub(super) fn dmabuf_inode(fd: i32) -> Option<u64> {
    // SAFETY: fstat writes the stat of an open descriptor into the zeroed struct it is given.
    unsafe {
        let mut stat: libc::stat = std::mem::zeroed();
        (libc::fstat(fd, &mut stat) == 0).then_some(stat.st_ino as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(surface: u32) -> FbKey {
        FbKey { surface, inode: 100 + u64::from(surface), modifier: 0 }
    }

    #[test]
    fn a_recycled_buffer_gets_its_framebuffer_back_without_another_one_made() {
        let mut cache = FbCache::new(4);
        for surface in 0..3 {
            assert!(cache.insert(key(surface), surface * 10, |_| false).is_empty());
        }
        // The pool goes round: every picture finds its framebuffer.
        for _ in 0..5 {
            for surface in 0..3 {
                assert_eq!(cache.get(key(surface)), Some(&(surface * 10)));
            }
        }
        assert_eq!(cache.len(), 3);
        // The same surface with another buffer behind it (another inode or modifier) is not it.
        assert_eq!(cache.get(FbKey { inode: 7, ..key(1) }), None);
        assert_eq!(cache.get(FbKey { modifier: 1, ..key(1) }), None);
    }

    #[test]
    fn past_its_capacity_the_least_recently_used_goes_but_never_one_on_the_screen() {
        let mut cache = FbCache::new(3);
        for surface in 0..3 {
            cache.insert(key(surface), surface, |_| false);
        }
        cache.get(key(0));
        // 1 is the least recently used now.
        assert_eq!(cache.insert(key(3), 3, |_| false), vec![1]);
        assert_eq!(cache.len(), 3);
        // 2 is the oldest, but on the screen: 0 goes instead.
        assert_eq!(cache.insert(key(4), 4, |v| *v == 2), vec![0]);
        assert_eq!(cache.get(key(2)), Some(&2));
        // Everything busy: the cache grows rather than take a framebuffer off the screen.
        assert!(cache.insert(key(5), 5, |_| true).is_empty());
        assert_eq!(cache.len(), 4);
        // The next insert with nothing busy brings it back to its capacity.
        assert_eq!(cache.insert(key(6), 6, |_| false).len(), 2);
        assert_eq!(cache.len(), 3);
    }

    #[test]
    fn a_key_made_again_hands_back_its_old_framebuffer() {
        let mut cache = FbCache::new(3);
        cache.insert(key(1), 10, |_| false);
        assert_eq!(cache.insert(key(1), 11, |_| false), vec![10]);
        assert_eq!(cache.get(key(1)), Some(&11));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn a_new_pool_and_the_end_of_the_stream_hand_back_every_framebuffer() {
        let mut cache = FbCache::new(FB_CACHE_CAPACITY);
        for surface in 0..5 {
            cache.insert(key(surface), surface, |_| false);
        }
        let mut all = cache.take_all();
        all.sort_unstable();
        assert_eq!(all, vec![0, 1, 2, 3, 4]);
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.get(key(0)), None);
    }

    #[test]
    fn a_descriptor_exported_again_has_the_same_inode() {
        use std::os::fd::AsRawFd;
        let file = std::fs::File::open("/proc/self/exe").unwrap();
        let again = file.try_clone().unwrap();
        let inode = dmabuf_inode(file.as_raw_fd()).unwrap();
        assert_eq!(dmabuf_inode(again.as_raw_fd()), Some(inode));
        assert_eq!(dmabuf_inode(-1), None);
    }
}
