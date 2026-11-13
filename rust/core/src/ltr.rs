//! Long-term reference recovery, using Moonlight's one-based wire frame indexes.
use std::collections::VecDeque;

/// The picture counter a decoder that never saw the lost frames has to infer
/// from the low bits the recovery frame carries, which bounds how long after
/// the last frame the client showed a long-term reference can still repair it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Counter {
    /// Every frame header carries its order hint and names its references
    /// by slot (AV1), or the host renumbers H.264's frame_num past the frames
    /// the client skipped (`bitstream::FrameNumbers`): no bound.
    #[default]
    Explicit,
    /// HEVC slices carry the POC's low bits, [`ORDER_WINDOW`] doubled once
    /// the host widens AMF's four (`bitstream::PictureOrder`). A decoder takes
    /// the value nearest its previous picture, so beyond half of them it
    /// derives a POC too low and the long-term reference names a picture it
    /// never had ("Could not find ref with POC -16").
    ///
    /// [`ORDER_WINDOW`]: crate::bitstream::ORDER_WINDOW
    PictureOrder,
}
impl Counter {
    pub fn for_codec(codec: u8) -> Self {
        match codec {
            1 => Self::PictureOrder,
            _ => Self::Explicit,
        }
    }
    /// Whether a decoder whose last picture is wire frame `shown` still
    /// derives the order of wire frame `recovery`.
    pub fn follows(self, shown: u64, recovery: u64) -> bool {
        match self {
            Self::Explicit => true,
            Self::PictureOrder => recovery.saturating_sub(shown) <= crate::bitstream::ORDER_WINDOW,
        }
    }
}
/// Repairs since the IDR whose skipped frames are remembered: a loss whose
/// last shown frame is older gets a keyframe.
const REPAIRS: usize = 64;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub mark: Option<usize>,
    pub reference: Option<usize>,
    pub after_invalidation: bool,
    /// Frames since the IDR the client never decoded, as of this one: what
    /// `bitstream::RepairHeaders` renumbers H.264 by.
    pub skipped: u64,
    reset: bool,
    next: usize,
}
#[derive(Default)]
pub struct References {
    slots: Vec<Option<u64>>,
    next: usize,
    /// The repair's slot, the first frame the client hasn't decoded since,
    /// and the frames it had skipped before that one.
    pending: Option<(usize, u64, u64)>,
    submitted: u64,
    last_idr: u64,
    recovered: u64,
    recovered_from: u64,
    counter: Counter,
    /// Frames skipped from each repair on, oldest first, after `skipped_base`:
    /// the frame the history starts at and the count until the first entry.
    skipped: VecDeque<(u64, u64)>,
    skipped_base: (u64, u64),
    /// The last keyframe's bytes and a running mean of the other frames',
    /// which weigh a repair against a keyframe.
    keyframe_bytes: u64,
    inter_bytes: u64,
    whatever_it_costs: bool,
}
impl References {
    pub fn new(count: usize, counter: Counter) -> Self {
        Self {
            slots: vec![None; count.min(4)],
            counter,
            ..Default::default()
        }
    }
    pub fn enabled(&self) -> bool {
        !self.slots.is_empty()
    }
    pub fn invalidation_pending(&self) -> bool {
        self.pending.is_some()
    }
    pub fn disable(&mut self) {
        *self = Self::default();
    }
    /// What [`Self::plan`] makes of a keyframe once [`Self::disable`]d, known
    /// before: a re-encode submits it and disables the references only once
    /// the encoder has accepted it.
    pub fn disabled_keyframe_plan() -> Plan {
        Self::default().plan(0, true)
    }
    /// Repairs whatever a decoder can still order, even where a keyframe
    /// would be smaller: for measuring what [`Self::invalidate`] weighs.
    pub fn repair_whatever_it_costs(&mut self) {
        self.whatever_it_costs = true;
    }
    /// The size of each encoded frame, in order.
    pub fn encoded(&mut self, idr: bool, bytes: usize) {
        let bytes = bytes as u64;
        if idr {
            self.keyframe_bytes = bytes;
        } else if self.inter_bytes == 0 {
            self.inter_bytes = bytes;
        } else {
            self.inter_bytes = self.inter_bytes - self.inter_bytes / 8 + bytes / 8;
        }
    }
    fn skipped_now(&self) -> u64 {
        self.skipped
            .back()
            .map_or(self.skipped_base.1, |&(_, skipped)| skipped)
    }
    /// The frames skipped before the client decoded wire frame `frame`,
    /// unless that is older than the history kept.
    fn skipped_at(&self, frame: u64) -> Option<u64> {
        if frame < self.skipped_base.0 {
            return None;
        }
        Some(
            self.skipped
                .iter()
                .rev()
                .find(|(from, _)| *from <= frame)
                .map_or(self.skipped_base.1, |&(_, skipped)| skipped),
        )
    }
    pub fn plan(&self, frame: u64, idr: bool) -> Plan {
        let mut plan = Plan {
            mark: None,
            reference: None,
            after_invalidation: false,
            skipped: if idr { 0 } else { self.skipped_now() },
            reset: idr,
            next: self.next,
        };
        if !self.enabled() {
            return plan;
        }
        if idr {
            plan.mark = Some(0);
            plan.next = usize::from(self.slots.len() > 1);
        } else if let Some((reference, first, skipped)) = self.pending {
            plan.reference = Some(reference);
            plan.after_invalidation = true;
            plan.skipped = skipped + frame.saturating_sub(first);
            plan.reset = true;
        } else if frame.is_multiple_of(4) {
            plan.mark = Some(self.next);
            plan.next = if self.slots.len() == 1 {
                0
            } else if self.next + 1 >= self.slots.len() {
                1
            } else {
                self.next + 1
            };
        }
        plan
    }
    /// Commit only once the codec has accepted the input, including when it
    /// applied back pressure before accepting the same frame.
    pub fn accepted(&mut self, frame: u64, plan: &Plan) {
        // Disabled since this was planned (output polled while the input waited showed
        // headers the repair cannot rewrite): no slot or repair the plan names is left.
        let disabled;
        let plan = if self.enabled() {
            plan
        } else {
            disabled = Plan {
                mark: None,
                reference: None,
                after_invalidation: false,
                reset: plan.reset && !plan.after_invalidation,
                next: 0,
                ..*plan
            };
            &disabled
        };
        if plan.reset {
            self.recovered = frame;
            self.recovered_from = if plan.after_invalidation {
                self.skipped.push_back((frame, plan.skipped));
                if self.skipped.len() > REPAIRS {
                    self.skipped_base = self.skipped.pop_front().unwrap();
                }
                self.pending.unwrap().1
            } else {
                self.last_idr = frame;
                self.skipped.clear();
                self.skipped_base = (frame, 0);
                1
            };
            for (slot, value) in self.slots.iter_mut().enumerate() {
                if Some(slot) != plan.reference {
                    *value = None;
                }
            }
            self.pending = None;
        }
        if let Some(slot) = plan.mark {
            self.slots[slot] = Some(frame);
        }
        self.next = plan.next;
        self.submitted = frame;
    }
    /// Plans the next submission, `submitted + 1`, as a repair of the lost
    /// frames `first..=last`. False asks for a keyframe instead.
    pub fn invalidate(&mut self, first: u64, last: u64) -> bool {
        if first == 0 || first > last || first > self.submitted {
            return false;
        }
        // Feedback can still be in flight when an IDR or LTR repair is sent.
        // A range that includes that repair must recover again.
        if last < self.last_idr || (first >= self.recovered_from && last < self.recovered) {
            return true;
        }
        // A client drops every frame from a loss to its repair: if it lost
        // the repair too, or one is already planned, it still has nothing
        // newer than the frame before the first loss.
        let mut first = first;
        if first >= self.recovered_from && first <= self.recovered {
            first = self.recovered_from;
        }
        if let Some((_, pending, _)) = self.pending {
            first = first.min(pending);
        }
        let recovery = self.submitted.saturating_add(1);
        if !self.counter.follows(first - 1, recovery) {
            if self.enabled() {
                tracing::info!(
                    first,
                    last,
                    recovery,
                    counter = ?self.counter,
                    "reference invalidation answered with a keyframe: the client's decoder cannot order a recovery frame this long after the last frame it showed"
                );
            }
            return false;
        }
        let reference = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(slot, frame)| {
                frame
                    .filter(|frame| *frame < first)
                    .map(|frame| (slot, frame))
            })
            .max_by_key(|(_, frame)| *frame);
        let Some((reference, anchor)) = reference else {
            if self.enabled() {
                tracing::info!(
                    first,
                    last,
                    "reference invalidation answered with a keyframe: no long-term reference is older than the lost frames"
                );
            }
            return false;
        };
        let Some(skipped) = self.skipped_at(first - 1) else {
            tracing::info!(
                first,
                last,
                "reference invalidation answered with a keyframe: the client's last frame is older than the repairs the host remembers"
            );
            return false;
        };
        // Each frame since the anchor has changed the picture about as much
        // as an inter frame codes: a repair from further back than a
        // keyframe's worth of them is the larger frame.
        let age = recovery - anchor;
        let estimate = self.inter_bytes.saturating_mul(age);
        if !self.whatever_it_costs && self.keyframe_bytes > 0 && estimate >= self.keyframe_bytes {
            tracing::info!(
                first,
                last,
                age,
                estimate,
                keyframe_bytes = self.keyframe_bytes,
                "reference invalidation answered with a keyframe: a repair from a long-term reference this old would be the larger frame"
            );
            return false;
        }
        for frame in &mut self.slots {
            if frame.is_some_and(|frame| frame >= first && frame <= last.max(self.submitted)) {
                *frame = None;
            }
        }
        self.pending = Some((reference, first, skipped));
        true
    }
}
#[cfg(test)]
mod tests;
