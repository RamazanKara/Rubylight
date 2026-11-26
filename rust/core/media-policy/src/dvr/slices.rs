//! Reassembles frames that host encoders deliver as slices (`video_slice_delivery`) into
//! whole access units, dropping a partial frame and reporting a discontinuity whenever a
//! slice arrives out of order.

use super::VideoFrame;

/// How an encoder output unit relates to its frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitKind {
    Whole,
    Slice { index: u32 },
    Last { index: u32 },
}

struct Partial {
    next: u32,
    idr: bool,
    pts_us: u64,
    data: Vec<u8>,
}

#[derive(Default)]
pub struct Reassembler {
    partial: Option<Partial>,
    discontinuity: bool,
}

impl Reassembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one unit; returns the frame once its last slice arrived.
    pub fn push(
        &mut self,
        kind: UnitKind,
        idr: bool,
        pts_us: u64,
        bytes: &[u8],
    ) -> Option<VideoFrame> {
        let (index, last) = match kind {
            UnitKind::Whole => {
                self.drop_partial();
                return Some(VideoFrame::new(pts_us, idr, bytes.to_vec()));
            }
            UnitKind::Slice { index } => (index, false),
            UnitKind::Last { index } => (index, true),
        };
        if index == 0 {
            self.drop_partial();
            self.partial = Some(Partial {
                next: 0,
                idr,
                pts_us,
                data: Vec::with_capacity(bytes.len()),
            });
        }
        let Some(partial) = self.partial.as_mut() else {
            // A slice from the middle of a frame whose start was lost.
            self.discontinuity = true;
            return None;
        };
        if partial.next != index || partial.pts_us != pts_us {
            self.partial = None;
            self.discontinuity = true;
            return None;
        }
        partial.data.extend_from_slice(bytes);
        partial.idr |= idr;
        partial.next = index.saturating_add(1);
        if !last {
            return None;
        }
        self.partial
            .take()
            .map(|done| VideoFrame::new(done.pts_us, done.idr, done.data))
    }

    /// Whether a frame was lost since the last call; clears the flag.
    pub fn take_discontinuity(&mut self) -> bool {
        std::mem::take(&mut self.discontinuity)
    }

    /// Forgets a partial frame without reporting it (the stream restarted).
    pub fn reset(&mut self) {
        self.partial = None;
        self.discontinuity = false;
    }

    fn drop_partial(&mut self) {
        if self.partial.take().is_some() {
            self.discontinuity = true;
        }
    }
}
