//! Unit tests for `overlay_must_run` pass-skipping rules.

use super::overlay_must_run;

#[test]
fn an_unchanged_overlay_repaints_its_shapes_without_egui() {
    assert!(!overlay_must_run(Some(7), 7, false, false, false));
}

#[test]
fn a_first_or_changed_overlay_runs_egui() {
    assert!(overlay_must_run(None, 7, false, false, false));
    assert!(overlay_must_run(Some(6), 7, false, false, false));
}

#[test]
fn a_live_screen_runs_egui_every_pass() {
    assert!(overlay_must_run(Some(7), 7, true, false, false), "the stream menu, mic-only and Remote Input");
}

#[test]
fn input_for_egui_runs_it_even_when_nothing_changed() {
    assert!(overlay_must_run(Some(7), 7, false, true, false));
}

#[test]
fn a_pass_egui_asked_to_repeat_runs_again() {
    assert!(overlay_must_run(Some(7), 7, false, false, true));
}
