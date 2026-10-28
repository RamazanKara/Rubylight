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
mod tests {
    use super::*;
    #[test]
    fn feedback_for_frames_before_an_idr_does_not_request_another_idr() {
        let mut references = References::new(0, Counter::Explicit);
        for frame in 1..=12 {
            let plan = references.plan(frame, frame == 1 || frame == 10);
            references.accepted(frame, &plan);
        }
        assert!(references.invalidate(7, 8));
        assert!(references.invalidate(7, 9));
        assert!(!references.invalidate(7, 10));
        assert!(!references.invalidate(10, 12));
    }
    #[test]
    fn ltr_feedback_is_deduplicated_only_when_the_repair_covers_its_range() {
        let mut references = References::new(4, Counter::Explicit);
        for frame in 1..=20 {
            let plan = references.plan(frame, frame == 1);
            references.accepted(frame, &plan);
        }
        assert!(references.invalidate(17, 18));
        references.accepted(21, &references.plan(21, false));
        assert!(references.invalidate(17, 20));
        assert!(!references.invalidation_pending());
        assert!(!references.plan(22, false).after_invalidation);
        assert!(!references.invalidate(10, 12));
        assert!(references.invalidate(17, 21));
        assert!(references.invalidation_pending());
        assert!(references.plan(22, false).after_invalidation);
    }
    #[test]
    fn a_keyframe_re_encoding_an_accepted_frame_drops_every_older_reference() {
        // AMF encodes an oversize scene cut again as a keyframe at the same
        // wire frame: the inter frame never reaches the client, and nothing
        // may repair from a reference only it or the frames before it kept.
        let mut references = References::new(4, Counter::Explicit);
        for frame in 1..=24 {
            let plan = references.plan(frame, frame == 1);
            references.accepted(frame, &plan);
        }
        assert_eq!(references.slots, [Some(1), Some(16), Some(20), Some(24)]);
        references.accepted(24, &references.plan(24, true));
        assert_eq!(references.slots, [Some(24), None, None, None]);
        // Losses before it are covered by the keyframe, and a later one is
        // repaired from it, not from frame 16 or 20.
        assert!(references.invalidate(22, 23));
        assert!(!references.invalidation_pending());
        for frame in 25..=30 {
            references.accepted(frame, &references.plan(frame, false));
        }
        assert!(references.invalidate(27, 30));
        let repair = references.plan(31, false);
        assert_eq!(repair.reference, Some(0));
        assert_eq!(references.slots[0], Some(24));

        // The re-encoded frame was itself a repair: the keyframe ends it.
        let mut references = References::new(4, Counter::Explicit);
        for frame in 1..=20 {
            let plan = references.plan(frame, frame == 1);
            references.accepted(frame, &plan);
        }
        assert!(references.invalidate(17, 18));
        let repair = references.plan(21, false);
        assert_eq!(repair.reference, Some(1));
        references.accepted(21, &repair);
        let keyframe = references.plan(21, true);
        assert_eq!((keyframe.reference, keyframe.skipped), (None, 0));
        references.accepted(21, &keyframe);
        assert_eq!(references.slots, [Some(21), None, None, None]);
        assert!(!references.invalidation_pending());
        let next = references.plan(22, false);
        assert!(!next.after_invalidation);
        assert_eq!((next.reference, next.skipped), (None, 0));
        assert!(references.invalidate(17, 20));
        assert!(!references.invalidation_pending());
    }
    #[test]
    fn ltr_repair_preserves_previous_idr_coverage() {
        let mut references = References::new(4, Counter::Explicit);
        for frame in 1..=20 {
            let plan = references.plan(frame, frame == 1 || frame == 10);
            references.accepted(frame, &plan);
        }
        assert!(references.invalidate(17, 18));
        references.accepted(21, &references.plan(21, false));
        assert!(references.invalidate(7, 8));
        assert!(references.invalidate(7, 9));
        assert!(references.invalidate(17, 20));
        assert!(!references.invalidation_pending());
        assert!(!references.invalidate(7, 10));
        assert!(!references.invalidate(10, 12));
        assert!(!references.invalidate(16, 16));
        assert!(references.invalidate(17, 21));
        assert!(references.invalidation_pending());
        assert!(references.plan(22, false).after_invalidation);
    }
    #[test]
    fn delayed_loss_feedback_preserves_baseline_and_invalidates_newer_anchors() {
        let mut references = References::new(4, Counter::Explicit);
        let mut marks = vec![];
        for frame in 1..=20 {
            let plan = references.plan(frame, frame == 1);
            if let Some(slot) = plan.mark {
                marks.push((frame, slot));
            }
            references.accepted(frame, &plan);
        }
        assert_eq!(marks, [(1, 0), (4, 1), (8, 2), (12, 3), (16, 1), (20, 2)]);
        assert!(references.invalidate(17, 18));
        let recovery = references.plan(21, false);
        assert_eq!(recovery.reference, Some(1));
        assert!(recovery.after_invalidation);
        assert_eq!(references.plan(21, false), recovery); // unaccepted submit leaves state intact
        references.accepted(21, &recovery);
        assert_eq!(references.slots, [None, Some(16), None, None]);
        assert!(!references.plan(22, false).after_invalidation);
        assert!(!references.invalidate(10, 12));
        let idr = references.plan(22, true);
        references.accepted(22, &idr);
        assert_eq!(references.slots, [Some(22), None, None, None]);
        assert!(!references.invalidate(22, 22));
        assert!(!references.invalidate(24, 23));
    }
    #[test]
    fn single_slot_and_unsupported_drivers_have_bounded_fallbacks() {
        for count in [0, 1, 2, 4, 16] {
            let mut references = References::new(count, Counter::Explicit);
            for frame in 1..=1000 {
                let plan = references.plan(frame, frame == 1);
                assert!(plan.mark.is_none_or(|slot| slot < count.min(4)));
                references.accepted(frame, &plan);
            }
            references.disable();
            assert!(!references.invalidate(900, 910));
            assert!(!references.plan(1001, false).after_invalidation);
        }
    }
    /// Submits wire frames up to `last` with an IDR at frame 1.
    fn submitted(counter: Counter, last: u64) -> References {
        let mut references = References::new(4, counter);
        for frame in 1..=last {
            let plan = references.plan(frame, frame == 1);
            references.accepted(frame, &plan);
        }
        references
    }
    #[test]
    fn hevc_repairs_only_losses_a_decoder_still_orders() {
        // The RX 7900 XT run: frame 241 shown last, 242..=252 lost, the
        // repair from the IDR. With AMF's four POC bits every decoder derived
        // POC 236 instead of 252; with the eight the host writes, it orders.
        let mut references = submitted(Counter::PictureOrder, 252);
        assert!(references.invalidate(242, 252));
        let recovery = references.plan(253, false);
        assert!(recovery.after_invalidation);
        assert_eq!(
            recovery.reference.and_then(|slot| references.slots[slot]),
            Some(1)
        );
        // The window counts from the last frame shown: 128 frames on still
        // order, 129 don't, whatever was lost before.
        assert!(submitted(Counter::PictureOrder, 368).invalidate(242, 368));
        assert!(!submitted(Counter::PictureOrder, 369).invalidate(242, 369));
        assert!(submitted(Counter::Explicit, 369).invalidate(242, 369));
    }
    #[test]
    fn late_feedback_a_repair_covered_needs_no_keyframe_whatever_the_counter() {
        for counter in [Counter::Explicit, Counter::PictureOrder] {
            let mut references = submitted(counter, 252);
            let idr = references.plan(253, true);
            references.accepted(253, &idr);
            for frame in 254..=300 {
                let plan = references.plan(frame, false);
                references.accepted(frame, &plan);
            }
            assert!(references.invalidate(242, 252), "{counter:?}");
            assert!(!references.invalidation_pending());
            // A loss after the IDR gets its own repair, from the IDR.
            assert!(references.invalidate(254, 290), "{counter:?}");
            assert_eq!(references.plan(301, false).reference, Some(0));
        }
    }
    #[test]
    fn counters_follow_what_a_decoder_derives_from_the_low_bits() {
        assert_eq!(Counter::for_codec(0), Counter::Explicit);
        assert_eq!(Counter::for_codec(1), Counter::PictureOrder);
        assert_eq!(Counter::for_codec(2), Counter::Explicit);
        assert_eq!(Counter::for_codec(3), Counter::Explicit);
        // HEVC 8.3.1 with the MaxPicOrderCntLsb the host writes, counted from
        // the last IDR wherever it falls.
        let max = 1u64 << crate::bitstream::POC_LSB_BITS;
        for idr in [1, 2, 17, 255, 256, 257] {
            for shown in idr..idr + 2 * max {
                for recovery in shown + 1..=shown + max / 2 + 4 {
                    let (previous, current) = (shown - idr, recovery - idr);
                    let lsb = current % max;
                    let (previous_lsb, msb) = (previous % max, previous - previous % max);
                    let poc = if lsb < previous_lsb && previous_lsb - lsb >= max / 2 {
                        msb + max + lsb
                    } else if lsb > previous_lsb && lsb - previous_lsb > max / 2 {
                        (msb + lsb).wrapping_sub(max)
                    } else {
                        msb + lsb
                    };
                    assert_eq!(
                        Counter::PictureOrder.follows(shown, recovery),
                        poc == current,
                        "HEVC shown {shown} recovery {recovery} IDR {idr}"
                    );
                }
            }
        }
    }
    #[test]
    fn skipped_frames_count_what_the_client_dropped_since_the_idr() {
        let mut references = submitted(Counter::Explicit, 20);
        assert_eq!(references.plan(21, false).skipped, 0);
        // 17..=20 dropped, repaired by 21.
        assert!(references.invalidate(17, 18));
        let repair = references.plan(21, false);
        assert_eq!(repair.skipped, 4);
        references.accepted(21, &repair);
        for frame in 22..=26 {
            let plan = references.plan(frame, false);
            assert_eq!(plan.skipped, 4);
            references.accepted(frame, &plan);
        }
        // The repair itself lost: the client still has nothing after 16.
        assert!(references.invalidate(21, 26));
        let repair = references.plan(27, false);
        assert_eq!(repair.skipped, 10);
        references.accepted(27, &repair);
        // A later loss counts on from what came before it.
        for frame in 28..=32 {
            let plan = references.plan(frame, false);
            references.accepted(frame, &plan);
        }
        assert!(references.invalidate(31, 32));
        assert!(
            references.invalidate(32, 32),
            "a second report before the repair"
        );
        let repair = references.plan(33, false);
        assert_eq!(repair.skipped, 12);
        references.accepted(33, &repair);
        let idr = references.plan(34, true);
        assert_eq!(idr.skipped, 0);
        references.accepted(34, &idr);
        assert_eq!(references.plan(35, false).skipped, 0);
        // Beyond the repairs remembered, a keyframe.
        let mut references = submitted(Counter::Explicit, 8);
        for repair in 0..=REPAIRS as u64 {
            let frame = 9 + repair * 8;
            for frame in frame..frame + 7 {
                let plan = references.plan(frame, false);
                references.accepted(frame, &plan);
            }
            assert!(references.invalidate(frame + 6, frame + 6));
            let plan = references.plan(frame + 7, false);
            references.accepted(frame + 7, &plan);
        }
        assert_eq!(references.skipped_at(15), None);
        assert_eq!(references.skipped_at(16), Some(1));
    }
    #[test]
    fn a_repair_that_would_outweigh_a_keyframe_is_a_keyframe() {
        // Scrolling at 50 Mbps: 82 KB keyframes, 40 KB inter frames. A repair
        // two frames after its anchor, 240, is worth it; three are not.
        let weighed = |last: u64| {
            let mut references = submitted(Counter::Explicit, last);
            references.encoded(true, 82_000);
            for _ in 0..32 {
                references.encoded(false, 40_000);
            }
            references
        };
        assert!(weighed(241).invalidate(241, 241));
        assert!(!weighed(242).invalidate(241, 242));
        let mut forced = weighed(242);
        forced.repair_whatever_it_costs();
        assert!(forced.invalidate(241, 242));
        // A still picture: a repair from the IDR, 400 frames back, is small.
        let mut references = submitted(Counter::Explicit, 400);
        references.encoded(true, 90_000);
        for _ in 0..32 {
            references.encoded(false, 200);
        }
        assert!(references.invalidate(2, 400));
    }
}
