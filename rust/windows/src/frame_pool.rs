//! Reused buffers for large encoded frames.
//!
//! From around a megabyte, the Windows heap maps every block with its own
//! VirtualAlloc and returns it with VirtualFree: a new keyframe or PyroWave
//! picture of that size costs a page fault per 4 KB as it is written, and the
//! free costs again on the sender's thread that drops it. [`Encoded`] gives
//! its buffer back here when it is dropped, and the encoders copy their
//! output into one taken from here, so the same few regions carry the stream.
//!
//! [`Encoded`]: crate::encoder::Encoded
use std::sync::{Mutex, PoisonError};

/// Smaller buffers come from the heap as ever: it serves them from its own
/// segments without mapping them.
pub const POOLED: usize = 512 * 1024;
/// More than one frame encoding, one waiting and one being sent.
const KEPT: usize = 4;
/// A buffer past this is freed rather than kept: an exceptional frame must
/// not hold its memory for the rest of the process.
const LARGEST: usize = 16 * 1024 * 1024;

static POOL: Mutex<Pool> = Mutex::new(Pool(Vec::new()));

/// An empty buffer that holds at least `len` bytes, reused when `len` is
/// large enough to be worth it.
pub fn take(len: usize) -> Vec<u8> {
    if len < POOLED {
        return Vec::with_capacity(len);
    }
    let reused = POOL
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take(len);
    reused.unwrap_or_else(|| Vec::with_capacity(len))
}

/// `bytes` in a buffer from [`take`].
pub fn copy(bytes: &[u8]) -> Vec<u8> {
    let mut buffer = take(bytes.len());
    buffer.extend_from_slice(bytes);
    buffer
}

/// Keep `buffer` for a later [`take`], if it is large and the pool has room;
/// otherwise it is freed.
pub fn give(buffer: Vec<u8>) {
    if !(POOLED..=LARGEST).contains(&buffer.capacity()) {
        return;
    }
    // Freed, if it is, outside the lock.
    let _freed = POOL
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .give(buffer);
}

struct Pool(Vec<Vec<u8>>);

impl Pool {
    /// The smallest kept buffer that fits, so a keyframe's stays for keyframes.
    fn take(&mut self, len: usize) -> Option<Vec<u8>> {
        let index = self
            .0
            .iter()
            .enumerate()
            .filter(|(_, buffer)| buffer.capacity() >= len)
            .min_by_key(|(_, buffer)| buffer.capacity())
            .map(|(index, _)| index)?;
        Some(self.0.swap_remove(index))
    }
    /// Keeps `buffer`, emptied; returns the buffer that does not fit, if any.
    /// A full pool keeps its larger buffers: a smaller frame fits in them,
    /// not the reverse.
    fn give(&mut self, mut buffer: Vec<u8>) -> Option<Vec<u8>> {
        buffer.clear();
        if self.0.len() < KEPT {
            self.0.push(buffer);
            return None;
        }
        if let Some(smallest) = self
            .0
            .iter_mut()
            .min_by_key(|kept| kept.capacity())
            .filter(|kept| kept.capacity() < buffer.capacity())
        {
            std::mem::swap(smallest, &mut buffer);
        }
        Some(buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_large_buffer_is_reused_by_the_next_frame_that_fits() {
        let mut pool = Pool(Vec::new());
        let frame = copy(&vec![1; POOLED + 10]);
        assert_eq!(frame, vec![1; POOLED + 10]);
        let address = frame.as_ptr();
        assert!(pool.give(frame).is_none());
        assert!(pool.take(POOLED + 20_000_000).is_none());
        let again = pool.take(POOLED + 5).unwrap();
        assert!(again.is_empty() && again.capacity() >= POOLED + 5);
        assert_eq!(again.as_ptr(), address);
        assert!(pool.take(1).is_none());
    }

    #[test]
    fn the_smallest_fitting_buffer_is_taken_and_a_full_pool_keeps_its_largest() {
        let mut pool = Pool(Vec::new());
        let sizes = [POOLED * 4, POOLED, POOLED * 2, POOLED * 3, POOLED * 5];
        for size in sizes {
            pool.give(Vec::with_capacity(size));
        }
        assert_eq!(pool.0.len(), KEPT);
        assert!(pool.0.iter().all(|kept| kept.capacity() >= POOLED * 2));
        assert_eq!(pool.take(POOLED * 2 + 1).unwrap().capacity(), POOLED * 3);
        // A full pool frees a buffer smaller than all it keeps.
        pool.give(Vec::with_capacity(POOLED * 6));
        let refused = pool.give(Vec::with_capacity(POOLED)).unwrap();
        assert_eq!(refused.capacity(), POOLED);
    }

    #[test]
    fn small_frames_are_not_pooled() {
        let small = take(16);
        assert!(small.capacity() >= 16 && small.capacity() < POOLED);
        assert_eq!(copy(&[7; 16]), [7; 16]);
    }
}
