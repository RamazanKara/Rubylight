//! Long-term reference recovery, using Moonlight's one-based wire frame indexes.

/// The picture counter a decoder that never saw the lost frames has to infer
/// from the low bits the recovery frame carries, which bounds how long after
/// the last frame the client showed a long-term reference can still repair it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Counter {
    /// Every frame header carries its order hint and names its references
    /// by slot (AV1): no bound.
    #[default]
    Explicit,
    /// VCN's H.264 counts a four-bit frame_num (POC type 2). A wrap the
    /// decoder cannot see breaks the order, so the repair stays within one
    /// 16-frame period.
    FrameNumber,
    /// HEVC slices carry the POC's low bits, at least four. A decoder takes
    /// the value nearest its previous picture, so beyond half of 16 frames it
    /// derives a POC 16 too low and the long-term reference names a picture
    /// it never had ("Could not find ref with POC -16").
    PictureOrder,
}
impl Counter {
    pub fn for_codec(codec: u8) -> Self {
        match codec {
            0 => Self::FrameNumber,
            1 => Self::PictureOrder,
            _ => Self::Explicit,
        }
    }
    /// Whether a decoder whose last picture is wire frame `shown` still
    /// derives the order of wire frame `recovery`.
    pub fn follows(self, shown: u64, recovery: u64) -> bool {
        match self {
            Self::Explicit => true,
            Self::FrameNumber => shown.saturating_sub(1) / 16 == recovery.saturating_sub(1) / 16,
            Self::PictureOrder => recovery.saturating_sub(shown) <= 8,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub mark: Option<usize>,
    pub reference: Option<usize>,
    pub after_invalidation: bool,
    reset: bool,
    next: usize,
}
#[derive(Default)]
pub struct References {
    slots: Vec<Option<u64>>,
    next: usize,
    pending: Option<(usize, u64)>,
    submitted: u64,
    last_idr: u64,
    recovered: u64,
    recovered_from: u64,
    counter: Counter,
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
    pub fn plan(&self, frame: u64, idr: bool) -> Plan {
        let mut plan = Plan {
            mark: None,
            reference: None,
            after_invalidation: false,
            reset: idr,
            next: self.next,
        };
        if !self.enabled() {
            return plan;
        }
        if idr {
            plan.mark = Some(0);
            plan.next = usize::from(self.slots.len() > 1);
        } else if let Some((reference, _)) = self.pending {
            plan.reference = Some(reference);
            plan.after_invalidation = true;
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
                self.pending.unwrap().1
            } else {
                self.last_idr = frame;
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
        let Some((reference, _)) = reference else {
            if self.enabled() {
                tracing::info!(
                    first,
                    last,
                    "reference invalidation answered with a keyframe: no long-term reference is older than the lost frames"
                );
            }
            return false;
        };
        for frame in &mut self.slots {
            if frame.is_some_and(|frame| frame >= first && frame <= last.max(self.submitted)) {
                *frame = None;
            }
        }
        self.pending = Some((reference, first));
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
        // The RX 7900 XT run: frame 241 shown last, 242..=252 lost. Its
        // repair, frame 253, named the IDR as a long-term reference, and every
        // decoder derived POC 236 instead of 252 from the low bits 12.
        let mut references = submitted(Counter::PictureOrder, 252);
        assert!(!references.invalidate(242, 252));
        assert!(!references.invalidation_pending());
        assert!(!references.plan(253, false).after_invalidation);
        // Gap 15: the repair's low bits equal the last shown frame's.
        assert!(!submitted(Counter::PictureOrder, 256).invalidate(242, 256));
        // Gap 8, one frame past half the four-bit range, failed the same way.
        assert!(!submitted(Counter::PictureOrder, 249).invalidate(242, 249));
        // Gap 7 decoded on every decoder: the repair names frame 240.
        let mut references = submitted(Counter::PictureOrder, 248);
        assert!(references.invalidate(242, 248));
        let recovery = references.plan(249, false);
        assert!(recovery.after_invalidation);
        assert_eq!(
            recovery.reference.and_then(|slot| references.slots[slot]),
            Some(240)
        );
        // H.264 keeps one 16-frame period: gap 11 repairs, gap 15 crosses it.
        assert!(submitted(Counter::FrameNumber, 252).invalidate(242, 252));
        assert!(!submitted(Counter::FrameNumber, 256).invalidate(242, 256));
        assert!(submitted(Counter::Explicit, 256).invalidate(242, 256));
    }
    #[test]
    fn late_feedback_a_repair_covered_needs_no_keyframe_whatever_the_counter() {
        for counter in [Counter::FrameNumber, Counter::PictureOrder] {
            let mut references = submitted(counter, 252);
            let idr = references.plan(253, true);
            references.accepted(253, &idr);
            for frame in 254..=300 {
                let plan = references.plan(frame, false);
                references.accepted(frame, &plan);
            }
            assert!(references.invalidate(242, 252), "{counter:?}");
            assert!(!references.invalidation_pending());
            assert!(!references.invalidate(254, 290), "{counter:?}");
        }
    }
    #[test]
    fn counters_follow_what_a_decoder_derives_from_the_low_bits() {
        assert_eq!(Counter::for_codec(0), Counter::FrameNumber);
        assert_eq!(Counter::for_codec(1), Counter::PictureOrder);
        assert_eq!(Counter::for_codec(2), Counter::Explicit);
        assert_eq!(Counter::for_codec(3), Counter::Explicit);
        // Counted from the last IDR, wherever it falls; 16 is the smallest
        // range either codec allows.
        for idr in 1..=17u64 {
            for shown in idr..idr + 48 {
                for recovery in shown + 1..=shown + 20 {
                    let (previous, current) = (shown - idr, recovery - idr);
                    // HEVC 8.3.1 with MaxPicOrderCntLsb 16.
                    let lsb = current % 16;
                    let (previous_lsb, msb) = (previous % 16, previous - previous % 16);
                    let poc = if lsb < previous_lsb && previous_lsb - lsb >= 8 {
                        msb + 16 + lsb
                    } else if lsb > previous_lsb && lsb - previous_lsb > 8 {
                        (msb + lsb).wrapping_sub(16)
                    } else {
                        msb + lsb
                    };
                    assert_eq!(
                        Counter::PictureOrder.follows(shown, recovery),
                        poc == current,
                        "HEVC shown {shown} recovery {recovery} IDR {idr}"
                    );
                    // H.264 8.2.1.3, POC type 2: FrameNumOffset sees one wrap.
                    let number = current % 16;
                    let previous_number = previous % 16;
                    let offset =
                        previous - previous_number + if previous_number > number { 16 } else { 0 };
                    if Counter::FrameNumber.follows(shown, recovery) {
                        assert_eq!(
                            offset + number,
                            current,
                            "H.264 shown {shown} recovery {recovery} IDR {idr}"
                        );
                    }
                }
            }
        }
    }
}
