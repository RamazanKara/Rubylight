//! The camera confidence: scene cuts, the pointer, missed chunks and the HUD's steadiness.

use super::*;
use crate::frame_warp::motion::estimate;
use crate::frame_warp::motion::tests::scene;

#[test]
fn a_pan_is_one_scene_and_a_cut_is_another() {
    let a = scene(256, 144, 0.0, 0.0, 1);
    let panned = scene(256, 144, 6.0, 1.0, 2);
    let pan = scene_change(&a, &panned).unwrap();
    assert!(pan.histogram > 0.8 && pan.correlation > 0.6, "{pan:?}");
    let motion = estimate(&a, &panned).map(|m| m.confidence);
    assert!(!pan.cut(motion), "{pan:?} {motion:?}");
    // Somewhere else in the world: the blocks no longer correlate and nothing moved there.
    let elsewhere = scene(256, 144, 5_000.0, 3_000.0, 3);
    let cut = scene_change(&a, &elsewhere).unwrap();
    let motion = estimate(&a, &elsewhere).map(|m| m.confidence);
    assert!(cut.correlation < CORRELATION_CUT, "{cut:?}");
    assert!(cut.cut(motion), "{cut:?} {motion:?}");
    // A fade to black.
    let black = Thumbnail {
        luma: vec![4; 256 * 144],
        ..a.clone()
    };
    let fade = scene_change(&a, &black).unwrap();
    assert!(
        fade.histogram < HISTOGRAM_CUT && fade.cut(Some(0.9)),
        "{fade:?}"
    );
    // Two black pictures are one scene; sizes that differ are none.
    assert!(!scene_change(&black, &black).unwrap().cut(None));
    assert_eq!(scene_change(&a, &scene(128, 72, 0.0, 0.0, 4)), None);
}

#[test]
fn a_cut_or_a_pointer_drops_the_confidence_at_once() {
    let mut c = Confidence::new(false);
    assert_eq!(c.state(), State::ColdAcquiring);
    assert!((c.update(false, false, 0.5, 0) - 0.5).abs() < 1e-6);
    assert_eq!(c.update(true, false, 0.9, 10_000), 1.0);
    assert_eq!(c.state(), State::Locked);
    c.on_cut(20_000);
    assert_eq!(c.update(true, false, 0.9, 20_000), 0.0);
    assert_eq!(c.state(), State::ConfidenceDecaying);
    assert_ne!(c.flags(30_000) & meta_flags::SCENE_CUT, 0);
    // The cut's hold ends: locked again.
    assert_eq!(c.update(true, false, 0.9, 20_000 + CUT_HOLD_US), 1.0);
    assert_eq!(c.flags(20_000 + CUT_HOLD_US), 0);
    c.on_cursor(true);
    assert_eq!(c.update(true, false, 0.9, 300_000), 0.0);
    assert_eq!(c.flags(300_000), meta_flags::CURSOR_VISIBLE_PAUSE);
    // A pointer that stays unlocks; once it goes, a turn the model predicts confirms it.
    assert_eq!(
        c.update(true, false, 0.9, 300_000 + UNLOCK_AFTER_US + 1),
        0.0
    );
    assert_eq!(c.state(), State::Unlocked);
    c.on_cursor(false);
    assert_eq!(c.update(true, false, 0.9, 1_000_000), 0.0);
    c.on_check([5.0, 0.0], [5.2, 0.1], 1_010_000);
    c.on_check([4.0, 0.0], [4.1, 0.0], 1_040_000);
    assert_eq!(c.update(true, false, 0.9, 1_040_000), 1.0);
    assert_eq!(c.state(), State::Locked);
}

#[test]
fn two_missed_chunks_in_a_row_are_a_spike() {
    let mut c = Confidence::new(true);
    assert_eq!(c.state(), State::WarmPrior);
    // A warm start that the fit leaves is cold.
    c.update(false, false, 0.1, 0);
    assert_eq!(c.state(), State::ColdAcquiring);
    c.update(true, false, 0.9, 1_000);
    // One miss is noise; small predictions say nothing.
    c.on_check([0.0, 0.0], [6.0, 0.0], 10_000);
    assert_eq!(c.update(true, false, 0.9, 10_000), 1.0);
    c.on_check([0.0, 0.0], [1.0, 0.0], 20_000);
    c.on_check([0.0, 0.0], [6.0, 0.0], 40_000);
    assert_eq!(c.update(true, false, 0.9, 40_000), 0.0);
    // The picture follows again: back as soon as it confirms.
    c.on_check([6.0, 0.0], [6.0, 0.0], 70_000);
    c.on_check([6.0, 0.0], [6.0, 0.0], 100_000);
    assert_eq!(c.update(true, false, 0.9, 100_000), 1.0);
    // Spikes that stop without a confirmation hold for their time.
    c.on_check([0.0, 0.0], [-6.0, 0.0], 200_000);
    c.on_check([6.0, 0.0], [-6.0, 0.0], 230_000);
    assert_eq!(c.update(true, false, 0.9, 230_000 + SPIKE_HOLD_US - 1), 0.0);
    assert_eq!(c.update(true, false, 0.9, 230_000 + SPIKE_HOLD_US), 1.0);
    // A model that unlocks starts over cold.
    c.update(false, false, 0.2, 500_000);
    assert_eq!(c.state(), State::ColdAcquiring);
}

#[test]
fn the_huds_steadiness_follows_how_its_masks_agree() {
    let mut hud = HudStability::default();
    let mut mask = Mask::new(0, 64, 32);
    for x in 0..10 {
        mask.set(x, 3, true);
    }
    hud.observe(&mask);
    assert!(hud.known() && hud.value() == 0.0);
    hud.observe(&mask);
    hud.observe(&mask);
    assert!((hud.value() - 0.75).abs() < 1e-6, "{}", hud.value());
    // Half the cells move.
    let mut moved = Mask::new(0, 64, 32);
    for x in 5..15 {
        moved.set(x, 3, true);
    }
    hud.observe(&moved);
    assert!(hud.value() < 0.6, "{}", hud.value());
    // Another size starts over.
    hud.observe(&Mask::new(0, 32, 16));
    assert_eq!(hud.value(), 0.0);
    assert!(!hud.known());
}
