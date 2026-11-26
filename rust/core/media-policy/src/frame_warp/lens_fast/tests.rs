//! The fast lens: the field of view from a few turning moves.

use super::*;
use crate::frame_warp::lens::tests::turned;

const CONTENT: (u16, u16) = (2560, 1440);
const THUMBNAIL: (usize, usize) = (256, 144);

/// Moves as [`super::super::lens`]'s tests make them: turns left and right, a little up and
/// down, some while walking or rolling, with `noise` thumbnail pixels on every cell. Returns
/// the decision after each move.
fn learn(degrees: f64, noise: f64, moves: usize) -> (LensFast, Vec<Option<f64>>) {
    let layout = Layout::new(CONTENT, THUMBNAIL);
    let mut lens = LensFast::default();
    let mut found = Vec::new();
    for i in 0..moves {
        let phase = i as f64 * 0.37;
        let turn = (4.0 * phase.sin() + 1.5, 1.2 * (phase * 0.6).cos());
        let zoom = if i % 3 == 0 { 3.0 } else { 0.0 };
        let roll = if i % 5 == 0 { -2.0 } else { 0.0 };
        lens.observe(
            &layout,
            &turned(&layout, degrees, turn, zoom, roll, noise, i as u32 + 1),
        );
        found.push(lens.degrees());
    }
    (lens, found)
}

#[test]
fn the_field_of_view_is_found_within_fifteen_moves() {
    for field in [60.0, 90.0, 110.0] {
        let (lens, found) = learn(field, 0.15, 15);
        let degrees = lens
            .degrees()
            .unwrap_or_else(|| panic!("no lens for {field}: {found:?}"));
        assert!((degrees - field).abs() <= 2.5, "{degrees} for {field}");
        // Decided within a dozen moves the sweep could use.
        let first = found.iter().position(Option::is_some).unwrap();
        assert!(first < 12, "decided after {} moves for {field}", first + 1);
        assert!(lens.moves() <= 15);
    }
}

#[test]
fn other_fields_of_view_are_found_as_well() {
    for field in [40.0, 50.0, 75.0, 100.0, 118.0] {
        let (lens, _) = learn(field, 0.15, 15);
        let degrees = lens
            .degrees()
            .unwrap_or_else(|| panic!("no lens for {field}"));
        assert!((degrees - field).abs() <= 2.5, "{degrees} for {field}");
    }
}

#[test]
fn a_flat_picture_decides_flat() {
    let (lens, _) = learn(0.0, 0.15, 15);
    assert!(lens.decided(), "a flat picture is decided too");
    assert_eq!(lens.degrees(), None);
    assert_eq!(lens.tangent(), None);
}

#[test]
fn too_few_or_too_small_moves_decide_nothing() {
    let (lens, _) = learn(90.0, 0.15, MIN_MOVES - 1);
    assert!(!lens.decided());
    let layout = Layout::new(CONTENT, THUMBNAIL);
    let mut lens = LensFast::default();
    for i in 0..40 {
        // Too short a move, and one seen by four cells only.
        assert!(!lens.observe(
            &layout,
            &turned(&layout, 90.0, (0.4, 0.1), 0.0, 0.0, 0.1, i + 1)
        ));
        let mut sparse = turned(&layout, 90.0, (5.0, 0.0), 0.0, 0.0, 0.1, i + 1);
        for cell in sparse.cells.iter_mut().skip(4) {
            *cell = None;
        }
        assert!(!lens.observe(&layout, &sparse));
    }
    assert_eq!(lens.moves(), 0);
    lens.clear();
    assert!(!lens.decided());
}

#[test]
fn a_parabola_step_stays_within_half_a_step() {
    // A parabola least at 2.0 through 0, 3 and 6 with a step of 3: within half a step of 3.
    let f = |x: f64| (x - 2.0) * (x - 2.0);
    assert!((vertex(3.0, 3.0, (Some(f(0.0)), f(3.0), Some(f(6.0)))) - 2.0).abs() < 1e-9);
    assert_eq!(vertex(3.0, 3.0, (Some(9.0), 0.0, Some(-1.0))), 4.5);
    assert_eq!(vertex(3.0, 3.0, (None, 1.0, Some(0.5))), 4.5);
    assert_eq!(vertex(3.0, 3.0, (Some(2.0), 1.0, None)), 3.0);
    // Opening downward: stay.
    assert_eq!(vertex(3.0, 3.0, (Some(0.0), 1.0, Some(0.0))), 3.0);
}
