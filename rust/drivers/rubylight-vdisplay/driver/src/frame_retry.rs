//! GPU-independent state for delivering the final image after a busy ring becomes free.

/// One frame as the swap chain delivered it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub present_qpc: u64,
    pub acquire_qpc: u64,
    pub frame_number: u64,
}

/// What a ring publication did with a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pushed {
    Published,
    /// No slot could be published without waiting for the host.
    Dropped,
    /// The surface is not the ring's size or format; the host rebuilds the ring.
    Mismatch,
    /// The device failed under the copy; the worker exits for a reassignment.
    DeviceLost,
}

/// Metadata for the owned retained texture. Retrying never creates another acquisition or
/// changes its original timing. The worker retries an existing ring only after IddCx reports
/// E_PENDING, so a fresh acquired image always takes precedence. A new ring may also be
/// seeded from the owned image without waiting for a desktop update.
#[derive(Default)]
pub struct PendingFrame {
    current: Option<Frame>,
    pending: bool,
    misses: u32,
}

impl PendingFrame {
    pub fn current(&self) -> Option<Frame> {
        self.current
    }

    pub fn set(&mut self, frame: Frame) {
        self.current = Some(frame);
        self.pending = true;
        self.misses = 0;
    }

    pub fn clear(&mut self) {
        self.current = None;
        self.pending = false;
        self.misses = 0;
    }

    pub fn is_pending(&self) -> bool {
        self.pending
    }

    /// One nonblocking publication attempt; a successful image is never repeated.
    pub fn retry(&mut self, publish: impl FnOnce(Frame) -> Pushed) -> Option<Pushed> {
        if !self.pending {
            return None;
        }
        let result = publish(self.current?);
        if result == Pushed::Published {
            // The owned texture still holds the latest desktop image. Keep its metadata
            // for a later ring replacement, but never publish it twice into this ring.
            self.pending = false;
            self.misses = 0;
        } else {
            self.misses = (self.misses + 1).min(4);
        }
        Some(result)
    }

    /// Only used while a pending texture fits the current ring. With no pending image (or a
    /// size/format mismatch) the worker retains its ordinary idle wait instead.
    pub fn wait_ms(&self) -> u32 {
        (1 << self.misses).max(2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    const FINAL: Frame = Frame {
        present_qpc: 100,
        acquire_qpc: 110,
        frame_number: 7,
    };

    #[test]
    fn final_static_image_is_published_once_when_a_slot_becomes_free() {
        let mut pending = PendingFrame::default();
        pending.set(FINAL);
        let mut published = Vec::new();
        // No new acquisition or ring generation is needed between these idle passes.
        for available in [false, false, false, true, true] {
            pending.retry(|frame| {
                if available {
                    published.push(frame);
                    Pushed::Published
                } else {
                    Pushed::Dropped
                }
            });
        }
        assert_eq!(published, [FINAL]);
        assert_eq!(pending.current(), Some(FINAL));
        assert!(!pending.is_pending());
    }

    #[test]
    fn newer_acquisition_supersedes_pending_image_and_resets_backoff() {
        let mut pending = PendingFrame::default();
        pending.set(FINAL);
        for expected_wait in [2, 4, 8, 16, 16, 16] {
            assert_eq!(pending.retry(|_| Pushed::Dropped), Some(Pushed::Dropped));
            assert_eq!(pending.wait_ms(), expected_wait);
            assert_eq!(pending.current(), Some(FINAL));
        }
        let newer = Frame {
            present_qpc: 200,
            acquire_qpc: 210,
            frame_number: 8,
        };
        pending.set(newer);
        assert_eq!(pending.wait_ms(), 2);
        assert_eq!(
            pending.retry(|frame| {
                assert_eq!(frame, newer);
                Pushed::Published
            }),
            Some(Pushed::Published)
        );
    }

    #[test]
    fn direct_publication_or_failed_retention_cancels_old_image() {
        let mut pending = PendingFrame::default();
        pending.set(FINAL);
        pending.clear();
        assert_eq!(pending.retry(|_| panic!("old frame was retried")), None);
    }

    #[test]
    fn failure_preserves_original_metadata_for_recovery() {
        let mut pending = PendingFrame::default();
        pending.set(FINAL);
        for failure in [Pushed::Mismatch, Pushed::DeviceLost] {
            assert_eq!(pending.retry(|_| failure), Some(failure));
            assert_eq!(pending.current(), Some(FINAL));
        }
    }

    #[test]
    fn published_owned_image_can_seed_a_replacement_ring() {
        let mut pending = PendingFrame::default();
        pending.set(FINAL);
        assert_eq!(
            pending.retry(|_| Pushed::Published),
            Some(Pushed::Published)
        );
        assert_eq!(
            pending.retry(|_| panic!("same ring was seeded twice")),
            None
        );
        let seed = Frame {
            present_qpc: 500,
            acquire_qpc: 500,
            ..pending.current().expect("owned image survives publication")
        };
        pending.set(seed);
        assert_eq!(
            pending.retry(|frame| {
                assert_eq!(frame, seed);
                Pushed::Published
            }),
            Some(Pushed::Published)
        );
    }
}
