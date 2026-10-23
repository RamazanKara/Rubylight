//! The output units of a frame in AMF's slice output mode.
//!
//! QueryOutput returns one buffer per slice, typed 1 (slice) or 2 (last slice
//! of the frame). A frame that may be sent block by block leaves unit by unit.
//! Any other (a keyframe, one after an invalidation, one with no byte cap, or
//! any frame once the host turned slice delivery off) is joined and returned
//! whole at its last unit, as frame output mode would have returned it.

use crate::encoder::Unit;

/// What the first unit of a frame decides for all of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Meta {
    /// Units leave one by one; otherwise they are joined.
    pub streamed: bool,
    /// The frame is a keyframe.
    pub idr: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// A unit of a frame that is joined: nothing to hand on yet.
    Hold,
    /// One unit of a streamed frame.
    Unit(Unit, Vec<u8>),
    /// A whole frame: a joined one, or one AMF returned in frame mode.
    Whole(Vec<u8>),
}

#[derive(Debug)]
pub struct Pushed {
    pub step: Step,
    /// Set by the frame's first unit and kept to its last.
    pub meta: Meta,
    /// The frame still open when this unit came belonged to another PTS: it
    /// never got its last unit.
    pub abandoned: Option<i64>,
}

struct Open {
    pts: i64,
    next: u32,
    meta: Meta,
    joined: Vec<u8>,
}

#[derive(Default)]
pub struct Sequencer {
    open: Option<Open>,
}

impl Sequencer {
    /// Take the next buffer AMF returned. `kind` is its `OutputBufferType`
    /// (1 slice, 2 last slice; anything else is a whole frame). `first` runs
    /// on a frame's first unit only.
    pub fn push(
        &mut self,
        pts: i64,
        kind: Option<i64>,
        bytes: Vec<u8>,
        first: impl FnOnce() -> Meta,
    ) -> Pushed {
        let abandoned = match &self.open {
            Some(open) if open.pts != pts => self.open.take().map(|open| open.pts),
            _ => None,
        };
        if !matches!(kind, Some(1 | 2)) {
            let meta = first();
            self.open = None;
            return Pushed {
                step: Step::Whole(bytes),
                meta: Meta {
                    streamed: false,
                    ..meta
                },
                abandoned,
            };
        }
        let last = kind == Some(2);
        let open = self.open.get_or_insert_with(|| Open {
            pts,
            next: 0,
            meta: first(),
            joined: Vec::new(),
        });
        let index = open.next;
        open.next += 1;
        let meta = open.meta;
        let step = if meta.streamed {
            let unit = if last {
                Unit::Last { index }
            } else {
                Unit::Slice { index }
            };
            Step::Unit(unit, bytes)
        } else if !last {
            open.joined.extend_from_slice(&bytes);
            Step::Hold
        } else if index == 0 {
            Step::Whole(bytes)
        } else {
            let mut joined = std::mem::take(&mut open.joined);
            joined.extend_from_slice(&bytes);
            Step::Whole(joined)
        };
        if last {
            self.open = None;
        }
        Pushed {
            step,
            meta,
            abandoned,
        }
    }
    /// A frame AMF never finished: forget its units.
    pub fn forget(&mut self, pts: i64) {
        if self.open.as_ref().is_some_and(|open| open.pts == pts) {
            self.open = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STREAMED: Meta = Meta {
        streamed: true,
        idr: false,
    };
    const JOINED: Meta = Meta {
        streamed: false,
        idr: true,
    };

    fn run(sequencer: &mut Sequencer, pts: i64, meta: Meta, units: &[(i64, &[u8])]) -> Vec<Step> {
        let mut calls = 0;
        let steps = units
            .iter()
            .map(|&(kind, bytes)| {
                sequencer
                    .push(pts, Some(kind), bytes.to_vec(), || {
                        calls += 1;
                        meta
                    })
                    .step
            })
            .collect();
        assert_eq!(calls, 1, "the first unit alone decides the frame");
        steps
    }

    #[test]
    fn a_streamed_frame_leaves_unit_by_unit() {
        let mut sequencer = Sequencer::default();
        let steps = run(
            &mut sequencer,
            7,
            STREAMED,
            &[(1, b"aa"), (1, b"bbb"), (2, b"c")],
        );
        assert_eq!(
            steps,
            [
                Step::Unit(Unit::Slice { index: 0 }, b"aa".to_vec()),
                Step::Unit(Unit::Slice { index: 1 }, b"bbb".to_vec()),
                Step::Unit(Unit::Last { index: 2 }, b"c".to_vec()),
            ]
        );
        assert!(sequencer.open.is_none());
    }

    #[test]
    fn a_joined_frame_returns_whole_at_its_last_unit() {
        let mut sequencer = Sequencer::default();
        let steps = run(
            &mut sequencer,
            7,
            JOINED,
            &[(1, b"aa"), (1, b"bbb"), (2, b"c")],
        );
        assert_eq!(
            steps,
            [Step::Hold, Step::Hold, Step::Whole(b"aabbbc".to_vec())]
        );
        let pushed = sequencer.push(8, Some(2), b"x".to_vec(), || JOINED);
        assert_eq!(pushed.step, Step::Whole(b"x".to_vec()));
        assert!(pushed.meta.idr && !pushed.meta.streamed);
        assert_eq!(pushed.abandoned, None);
    }

    #[test]
    fn the_first_unit_meta_is_kept_for_the_whole_frame() {
        let mut sequencer = Sequencer::default();
        sequencer.push(1, Some(1), vec![1], || JOINED);
        let pushed = sequencer.push(1, Some(2), vec![2], || panic!("decided twice"));
        assert_eq!(pushed.meta, JOINED);
        assert_eq!(pushed.step, Step::Whole(vec![1, 2]));
    }

    #[test]
    fn a_frame_returned_whole_passes_through_in_either_mode() {
        let mut sequencer = Sequencer::default();
        for kind in [None, Some(0)] {
            let pushed = sequencer.push(3, kind, b"frame".to_vec(), || STREAMED);
            assert_eq!(pushed.step, Step::Whole(b"frame".to_vec()));
            assert!(!pushed.meta.streamed);
        }
    }

    #[test]
    fn a_frame_without_its_last_unit_is_reported_when_the_next_arrives() {
        let mut sequencer = Sequencer::default();
        sequencer.push(1, Some(1), b"a".to_vec(), || STREAMED);
        let pushed = sequencer.push(2, Some(1), b"b".to_vec(), || STREAMED);
        assert_eq!(pushed.abandoned, Some(1));
        assert_eq!(
            pushed.step,
            Step::Unit(Unit::Slice { index: 0 }, b"b".to_vec())
        );
        // A joined frame's leftovers never reach the next one.
        let mut sequencer = Sequencer::default();
        sequencer.push(1, Some(1), b"stale".to_vec(), || JOINED);
        let pushed = sequencer.push(2, Some(2), b"new".to_vec(), || JOINED);
        assert_eq!(pushed.abandoned, Some(1));
        assert_eq!(pushed.step, Step::Whole(b"new".to_vec()));
    }

    #[test]
    fn forgetting_a_frame_drops_its_units_only() {
        let mut sequencer = Sequencer::default();
        sequencer.push(1, Some(1), b"a".to_vec(), || JOINED);
        sequencer.forget(2);
        assert!(sequencer.open.is_some());
        sequencer.forget(1);
        let pushed = sequencer.push(1, Some(2), b"b".to_vec(), || JOINED);
        assert_eq!(pushed.step, Step::Whole(b"b".to_vec()));
        assert_eq!(pushed.abandoned, None);
    }
}
