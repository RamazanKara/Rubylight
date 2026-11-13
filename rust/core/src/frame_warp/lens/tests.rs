use super::*;
use crate::frame_warp::motion::tests::world;
use crate::frame_warp::motion::{CellShift, Thumbnail, estimate};

const CONTENT: (u16, u16) = (2560, 1440);
const THUMBNAIL: (usize, usize) = (256, 144);

fn tangent(degrees: f64) -> f64 {
    let half = degrees.to_radians() * 0.5;
    half.tan() * half.tan()
}

/// The move a camera turning by `turn` thumbnail pixels at the centre makes through a lens
/// of `degrees` (0: a flat picture), with a zoom, a roll and noise of `noise` pixels.
pub(crate) fn turned(
    layout: &Layout,
    degrees: f64,
    turn: (f64, f64),
    zoom: f64,
    roll: f64,
    noise: f64,
    seed: u32,
) -> Motion {
    let lens = if degrees > 0.0 { tangent(degrees) } else { 0.0 };
    let mut state = seed.wrapping_mul(0x9e37_79b9) | 1;
    let mut random = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        f64::from(state) / f64::from(u32::MAX) - 0.5
    };
    let (a, b) = (turn.0 * layout.scale[0], turn.1 * layout.scale[1]);
    let cells: [Option<CellShift>; CELLS] = core::array::from_fn(|i| {
        let c = &layout.cells[i];
        let u = a * (1.0 + lens * c.xx) + b * lens * c.xy + zoom * c.x - roll * c.y;
        let v = a * lens * c.xy + b * (1.0 + lens * c.yy) + zoom * c.y + roll * c.x;
        Some(CellShift {
            dx: (u / layout.scale[0] + noise * 2.0 * random()) as f32,
            dy: (v / layout.scale[1] + noise * 2.0 * random()) as f32,
            correlation: 0.9,
            place: None,
        })
    });
    let median = |axis: usize| {
        let mut values: Vec<f32> = cells
            .iter()
            .flatten()
            .map(|c| if axis == 0 { c.dx } else { c.dy })
            .collect();
        values.sort_by(f32::total_cmp);
        values[values.len() / 2]
    };
    Motion {
        dx: median(0),
        dy: median(1),
        confidence: 0.9,
        cells,
    }
}

fn learn(degrees: f64, noise: f64, moves: usize) -> (Layout, Lens) {
    let layout = Layout::new(CONTENT, THUMBNAIL);
    let mut lens = Lens::default();
    for i in 0..moves {
        // Turns left and right, a little up and down, some while walking or rolling.
        let phase = i as f64 * 0.37;
        let turn = (4.0 * phase.sin() + 1.5, 1.2 * (phase * 0.6).cos());
        let zoom = if i % 3 == 0 { 3.0 } else { 0.0 };
        let roll = if i % 5 == 0 { -2.0 } else { 0.0 };
        lens.observe(
            &layout,
            &turned(&layout, degrees, turn, zoom, roll, noise, i as u32 + 1),
        );
    }
    (layout, lens)
}

fn degrees(lens: &Lens) -> Option<f64> {
    lens.tangent().map(|t| 2.0 * t.sqrt().atan().to_degrees())
}

#[test]
fn a_perspective_picture_teaches_its_field_of_view() {
    for field in [50.0, 70.0, 90.0, 104.0] {
        let (_, lens) = learn(field, 0.15, 240);
        let found = degrees(&lens).unwrap_or_else(|| panic!("no lens for {field}"));
        assert!((found - field).abs() <= 4.0, "{found} for {field}");
    }
}

#[test]
fn a_flat_picture_has_no_lens() {
    let (_, lens) = learn(0.0, 0.15, 240);
    assert_eq!(lens.tangent(), None);
    // And noise alone, at a narrow field of view's size, does not make one.
    let (_, lens) = learn(0.0, 0.4, 240);
    assert_eq!(lens.tangent(), None);
}

#[test]
fn a_lens_waits_for_enough_turning_and_starts_from_the_prior() {
    let (layout, mut lens) = learn(90.0, 0.15, 12);
    assert_eq!(lens.tangent(), None);
    lens.set_prior(Some(tangent(80.0)));
    assert_eq!(degrees(&lens).map(f64::round), Some(80.0));
    // The moves then say otherwise.
    let mut changed = false;
    for i in 0..240 {
        let turn = (4.0 * (i as f64 * 0.37).sin() + 1.5, 1.0);
        changed |= lens.observe(&layout, &turned(&layout, 90.0, turn, 0.0, 0.0, 0.15, i + 7));
    }
    assert!(changed);
    let found = degrees(&lens).unwrap();
    assert!((found - 90.0).abs() <= 4.0, "{found}");
    // Cleared, the lens in force stays as the prior.
    lens.clear();
    assert!((degrees(&lens).unwrap() - found).abs() < 1e-9);
}

#[test]
fn a_lens_follows_a_game_that_changes_its_field_of_view_and_goes_when_it_goes_flat() {
    let (layout, mut lens) = learn(90.0, 0.15, 240);
    for i in 0..900 {
        let turn = (4.0 * (i as f64 * 0.37).sin() + 1.5, 1.0);
        lens.observe(
            &layout,
            &turned(&layout, 60.0, turn, 0.0, 0.0, 0.15, i + 11),
        );
    }
    let found = degrees(&lens).unwrap();
    assert!((found - 60.0).abs() <= 4.0, "{found}");
    for i in 0..900 {
        let turn = (4.0 * (i as f64 * 0.37).sin() + 1.5, 1.0);
        lens.observe(&layout, &turned(&layout, 0.0, turn, 0.0, 0.0, 0.15, i + 13));
    }
    assert_eq!(lens.tangent(), None);
}

#[test]
fn slow_or_sparse_moves_teach_nothing() {
    let layout = Layout::new(CONTENT, THUMBNAIL);
    let mut lens = Lens::default();
    for i in 0..400 {
        let mut slow = turned(&layout, 90.0, (0.4, 0.1), 0.0, 0.0, 0.1, i + 1);
        assert!(!lens.observe(&layout, &slow));
        // A fast move seen by four cells only.
        slow = turned(&layout, 90.0, (5.0, 0.0), 0.0, 0.0, 0.1, i + 1);
        for cell in slow.cells.iter_mut().skip(4) {
            *cell = None;
        }
        assert!(!lens.observe(&layout, &slow));
    }
    assert_eq!(lens.tangent(), None);
}

#[test]
fn the_turn_is_the_centres_under_a_lens_and_the_median_without() {
    let layout = Layout::new(CONTENT, THUMBNAIL);
    let mut motion = turned(&layout, 90.0, (6.0, -2.0), 2.0, 1.0, 0.0, 1);
    // Most cells lie off centre, so the median is longer than the centre's move.
    assert!(motion.dx > 6.5, "{motion:?}");
    let flat = layout.resolve(&motion, None);
    assert_eq!((flat.dx, flat.dy), (motion.dx, motion.dy));
    let turn = layout.resolve(&motion, Some(tangent(90.0)));
    assert!(
        (turn.dx - 6.0).abs() < 0.01 && (turn.dy + 2.0).abs() < 0.01,
        "{turn:?}"
    );
    assert!(turn.confidence > 0.85, "{turn:?}");
    // A cell where something else moves is left out and costs confidence.
    motion.cells[2] = Some(CellShift {
        dx: -9.0,
        dy: 4.0,
        correlation: 0.9,
        place: None,
    });
    let turn = layout.resolve(&motion, Some(tangent(90.0)));
    assert!(
        (turn.dx - 6.0).abs() < 0.05 && (turn.dy + 2.0).abs() < 0.05,
        "{turn:?}"
    );
    assert!(turn.confidence < 0.85, "{turn:?}");
    // A still picture has no cells and stays as it was measured.
    let still = Motion {
        confidence: 0.8,
        ..Motion::default()
    };
    assert_eq!(layout.resolve(&still, Some(tangent(90.0))).confidence, 0.8);
}

/// The textured world on a cylinder around the camera, seen through a lens of `focal`
/// thumbnail pixels by a camera pitched by `pitch` and then turned by `yaw` (radians), as a
/// game's is.
fn seen(focal: f64, yaw: f64, pitch: f64, at_us: u64) -> Thumbnail {
    let (width, height) = THUMBNAIL;
    let mut luma = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let px = x as f64 + 0.5 - width as f64 * 0.5;
            let py = y as f64 + 0.5 - height as f64 * 0.5;
            let (py, pz) = (
                py * pitch.cos() + focal * pitch.sin(),
                focal * pitch.cos() - py * pitch.sin(),
            );
            let (px, pz) = (
                px * yaw.cos() + pz * yaw.sin(),
                pz * yaw.cos() - px * yaw.sin(),
            );
            let (u, v) = (focal * px.atan2(pz), focal * py / px.hypot(pz));
            luma.push(world(u as f32 + 4000.0, v as f32 + 4000.0));
        }
    }
    Thumbnail {
        width,
        height,
        luma,
        at_us,
    }
}

#[test]
fn pictures_drawn_through_a_lens_teach_it_and_give_the_centres_turn() {
    for field in [60.0f64, 90.0] {
        let layout = Layout::new(CONTENT, THUMBNAIL);
        let focal = THUMBNAIL.0 as f64 * 0.5 / (field.to_radians() * 0.5).tan();
        let mut lens = Lens::default();
        let (mut yaw, mut pitch) = (0.0f64, 0.0f64);
        let mut previous = seen(focal, yaw, pitch, 0);
        let mut worst = 0.0f64;
        for i in 1..=150u64 {
            // Turns of 1.5 to 5 thumbnail pixels a capture, left and right, with some pitch.
            let step =
                (3.2 + 1.8 * (i as f64 * 0.21).sin()) * if (i / 40) % 2 == 0 { 1.0 } else { -1.0 };
            let tilt = 0.6 * (i as f64 * 0.13).cos();
            yaw += step / focal;
            pitch += tilt / focal;
            let current = seen(focal, yaw, pitch, i);
            let motion = estimate(&previous, &current).expect("a move");
            lens.observe(&layout, &motion);
            if i > 100 {
                // The content moves against the camera, by the turn at the picture's centre.
                let turn = layout.resolve(&motion, lens.tangent());
                worst = worst.max((f64::from(turn.dx) + step).abs());
                assert!(turn.confidence >= 0.5, "{turn:?} at {i} for {field}");
            }
            previous = current;
        }
        let found = degrees(&lens).unwrap_or_else(|| panic!("no lens for {field}"));
        assert!((found - field).abs() <= 2.0, "{found} for {field}");
        assert!(worst < 0.35, "{worst} for {field}");
    }
}

/// Where [`with_hud`] draws: a framed minimap in the top right corner, a counter in the
/// bottom right, a bar in the bottom left, and in the top left a stamp that changes with
/// every capture.
fn hud_at(x: usize, y: usize) -> Option<u8> {
    if (218..253).contains(&x) && (3..38).contains(&y) {
        Some(0)
    } else if (230..253).contains(&x) && (132..141).contains(&y) {
        Some(1)
    } else if (3..44).contains(&x) && (137..141).contains(&y) {
        Some(2)
    } else if x < 58 && (1..11).contains(&y) {
        Some(3)
    } else {
        None
    }
}

/// `t` under a HUD as a game draws one, the same in every capture but for its stamp.
fn with_hud(mut t: Thumbnail, capture: u64) -> Thumbnail {
    for y in 0..t.height {
        for x in 0..t.width {
            let Some(part) = hud_at(x, y) else { continue };
            t.luma[y * t.width + x] = match part {
                // A light frame around dark ground with blocks on it.
                0 if !(220..251).contains(&x) || !(5..36).contains(&y) => 200,
                0 if (x / 9 + y / 11) % 3 == 0 => 120,
                0 => 40,
                1 if (x / 3) % 2 == 0 => 220,
                1 => 25,
                2 if x < 32 => 90,
                2 => 30,
                _ => {
                    let bit = (x as u64 / 2).wrapping_mul(0x9e37_79b9_7f4a_7c15)
                        ^ (capture + y as u64 / 3 * 77).wrapping_mul(0xbf58_476d_1ce4_e5b9);
                    if (bit ^ (bit >> 31)) & 1 == 0 { 0 } else { 255 }
                }
            };
        }
    }
    t
}

#[test]
fn a_hud_in_the_corners_does_not_narrow_the_lens() {
    use crate::frame_warp::motion::{Still, estimate_around};
    // Left in the measurement, this HUD's corner cells move too little and a 90 degree lens
    // reads as 81. The host knows it from the GPU's mask, or finds what stands still.
    for known in [true, false] {
        let layout = Layout::new(CONTENT, THUMBNAIL);
        let focal = THUMBNAIL.0 as f64 * 0.5;
        let hud: Vec<bool> = (0..THUMBNAIL.0 * THUMBNAIL.1)
            .map(|i| hud_at(i % THUMBNAIL.0, i / THUMBNAIL.0).is_some_and(|part| part < 3))
            .collect();
        let mut still = Still::default();
        let mut lens = Lens::default();
        let (mut yaw, mut pitch) = (0.0f64, 0.0f64);
        let mut previous = with_hud(seen(focal, yaw, pitch, 0), 0);
        for i in 1..=if known { 150u64 } else { 300 } {
            let step =
                (3.2 + 1.8 * (i as f64 * 0.21).sin()) * if (i / 40) % 2 == 0 { 1.0 } else { -1.0 };
            yaw += step / focal;
            pitch += 0.6 * (i as f64 * 0.13).cos() / focal;
            let current = with_hud(seen(focal, yaw, pitch, i), i);
            let left_out = if known { Some(&hud[..]) } else { still.mask() };
            if let Some(motion) = estimate_around(&previous, &current, left_out) {
                still.observe(&previous, &current, &motion);
                lens.observe(&layout, &motion);
            }
            previous = current;
        }
        let found = degrees(&lens).unwrap_or_else(|| panic!("no lens, known {known}"));
        assert!((found - 90.0).abs() <= 2.5, "{found}, known {known}");
    }
}

#[test]
fn a_move_that_is_no_turn_does_not_choose_a_lens() {
    let (layout, mut lens) = learn(90.0, 0.15, 240);
    let before = degrees(&lens).unwrap();
    // Cuts: every cell somewhere else, far from any lens's turn.
    for i in 0..40u32 {
        let mut cut = turned(&layout, 90.0, (4.0, 1.0), 0.0, 0.0, 0.15, i + 3);
        for (index, cell) in cut.cells.iter_mut().enumerate() {
            let cell = cell.as_mut().unwrap();
            let wild = ((index as u32 * 7 + i * 13) % 11) as f32 - 5.0;
            (cell.dx, cell.dy) = (cell.dx + 3.0 * wild, cell.dy - 2.0 * wild);
        }
        lens.observe(&layout, &cut);
        // And a turn between them, as play goes on.
        lens.observe(
            &layout,
            &turned(&layout, 90.0, (4.0, 1.0), 0.0, 0.0, 0.15, i + 90),
        );
    }
    let after = degrees(&lens).expect("still a lens");
    assert!((after - before).abs() <= 2.0, "{before} then {after}");
}

#[test]
fn focal_lengths_and_tangents_convert() {
    let layout = Layout::new(CONTENT, THUMBNAIL);
    let focal = layout.focal(tangent(90.0));
    assert!((focal - 1280.0).abs() < 1e-6, "{focal}");
    assert!((layout.tangent(focal) - tangent(90.0)).abs() < 1e-9);
}
