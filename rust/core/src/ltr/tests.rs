use super::*;
#[test]
fn a_disabled_keyframe_plan_is_known_before_the_references_are_disabled() {
    // AMF rejecting a re-encode's LTR properties plans this keyframe but
    // disables the references only after SubmitInput succeeds: a failed
    // SubmitInput leaves them as they were.
    let mut references = References::new(4, Counter::Explicit);
    for frame in 1..=20 {
        let plan = references.plan(frame, frame == 1);
        references.accepted(frame, &plan);
    }
    assert!(references.invalidate(17, 18));
    let plan = References::disabled_keyframe_plan();
    assert!(references.enabled() && references.invalidation_pending());
    assert_eq!((plan.mark, plan.reference, plan.skipped), (None, None, 0));

    references.disable();
    assert_eq!(plan, references.plan(21, true));
    references.accepted(21, &plan);
    assert!(!references.enabled());
    let next = references.plan(24, false);
    assert_eq!((next.mark, next.reference), (None, None));
}
#[test]
fn a_plan_made_before_the_references_were_disabled_is_accepted_without_them() {
    // AMF's input queue is full, the output polled meanwhile has headers the repair
    // cannot rewrite, and the references are disabled before the input is accepted.
    for repair in [false, true] {
        let mut references = References::new(4, Counter::Explicit);
        for frame in 1..=23 {
            let plan = references.plan(frame, frame == 1);
            references.accepted(frame, &plan);
        }
        if repair {
            assert!(references.invalidate(22, 23));
        }
        let plan = references.plan(24, false);
        assert_eq!(plan.after_invalidation, repair);
        assert_eq!(plan.mark.is_some(), !repair);
        references.disable();
        references.accepted(24, &plan);
        assert!(!references.enabled());
        let keyframe = references.plan(25, true);
        assert_eq!(keyframe, References::disabled_keyframe_plan());
        references.accepted(25, &keyframe);
        assert!(references.invalidate(23, 24));
        assert!(!references.invalidate(25, 25));
    }
}
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
