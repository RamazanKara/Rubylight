use super::*;

/// Value noise: a hashed lattice every `cell` pixels, bilinear between, 0 to 1.
pub(crate) fn noise(u: f32, v: f32, cell: f32, seed: u32) -> f32 {
    let lattice = |x: i32, y: i32| {
        let mut h = (x as u32).wrapping_mul(0x8da6_b343)
            ^ (y as u32).wrapping_mul(0xd816_3841)
            ^ seed.wrapping_mul(0xcb1a_b31f);
        h ^= h >> 13;
        h = h.wrapping_mul(0x5bd1_e995);
        h ^= h >> 15;
        (h & 0xffff) as f32 / 65535.0
    };
    let (fu, fv) = (u / cell, v / cell);
    let (x0, y0) = (fu.floor(), fv.floor());
    let (tx, ty) = (fu - x0, fv - y0);
    let (x0, y0) = (x0 as i32, y0 as i32);
    let top = lattice(x0, y0) * (1.0 - tx) + lattice(x0 + 1, y0) * tx;
    let bottom = lattice(x0, y0 + 1) * (1.0 - tx) + lattice(x0 + 1, y0 + 1) * tx;
    top * (1.0 - ty) + bottom * ty
}

/// The textured world's luma at `(u, v)`.
pub(crate) fn world(u: f32, v: f32) -> u8 {
    let value = 128.0
        + 70.0 * (noise(u, v, 4.0, 1) - 0.5)
        + 60.0 * (noise(u, v, 9.0, 2) - 0.5)
        + 50.0 * (noise(u, v, 23.0, 3) - 0.5);
    value.clamp(0.0, 255.0) as u8
}

/// A textured scene without repeats, sampled at `(x + ox, y + oy)` so the content moves
/// by `(-ox, -oy)`.
pub(crate) fn scene(width: usize, height: usize, ox: f32, oy: f32, at_us: u64) -> Thumbnail {
    let mut luma = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            luma.push(world(x as f32 + ox, y as f32 + oy));
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
fn finds_a_pan_in_both_directions() {
    for (ox, oy) in [
        (6.0, 0.0),
        (-11.0, 3.0),
        (0.0, -7.0),
        (23.0, -9.0),
        (2.5, 1.5),
    ] {
        let a = scene(256, 144, 0.0, 0.0, 0);
        let b = scene(256, 144, ox, oy, 1);
        let m = estimate(&a, &b).unwrap_or_else(|| panic!("no estimate for {ox},{oy}"));
        assert!((m.dx + ox).abs() < 0.6, "dx {m:?} for {ox}");
        assert!((m.dy + oy).abs() < 0.6, "dy {m:?} for {oy}");
        assert!(m.confidence > 0.5, "{m:?}");
    }
}

#[test]
fn slow_turns_are_measured_to_a_tenth_of_a_pixel() {
    for (ox, oy) in [
        (0.2f32, 0.0f32),
        (0.0, 0.3),
        (0.0, -0.15),
        (0.45, -0.4),
        (1.3, 0.7),
    ] {
        let a = scene(256, 144, 0.0, 0.0, 0);
        let b = scene(256, 144, ox, oy, 1);
        let m = estimate(&a, &b).unwrap_or_else(|| panic!("no estimate for {ox},{oy}"));
        assert!((m.dx + ox).abs() < 0.1, "dx {m:?} for {ox}");
        assert!((m.dy + oy).abs() < 0.1, "dy {m:?} for {oy}");
    }
}

#[test]
fn a_slow_tilt_over_upright_edges_is_not_a_still_picture() {
    // Poles and walls: strong upright edges, which a tilt leaves unchanged, over texture.
    let upright = |oy: f32| {
        let mut t = scene(256, 144, 0.0, oy, 0);
        for (i, v) in t.luma.iter_mut().enumerate() {
            // Irregular widths: a periodic pattern has no one shift.
            if ((i % 256 / 5) as u32).wrapping_mul(2_654_435_761) >> 31 == 1 {
                *v = v.saturating_add(70);
            }
        }
        t
    };
    let m = estimate(&upright(0.0), &upright(0.25)).unwrap();
    assert!((m.dy + 0.25).abs() < 0.1, "{m:?}");
}

#[test]
fn a_cell_is_measured_where_its_texture_is() {
    // A profile flat but for one ramp: its slopes, and so its shift, are the ramp's.
    let mut profile = vec![0.0f32; 100];
    for (i, value) in profile.iter_mut().enumerate().take(70).skip(60) {
        *value = (i - 60) as f32;
    }
    for value in profile.iter_mut().skip(70) {
        *value = 10.0;
    }
    let beside = [7.0f32; 100];
    let found = place(&profile, &beside, 40, 90).unwrap();
    let (mean, squared) = (found.along, found.squared);
    assert!((found.across - 7.0).abs() < 1e-4 && (found.cross - 7.0 * mean).abs() < 1e-2);
    // The ramp runs from sample 60 to 70, 10 to 20 past the middle.
    assert!((mean - 15.0).abs() < 0.6, "{mean}");
    assert!((squared - 15.0 * 15.0).abs() < 25.0, "{squared}");
    // Textured alike all over, it is the cell's middle; unknown samples are left out.
    let even: Vec<f32> = (0..100)
        .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 } * (i % 4) as f32)
        .collect();
    let mean = place(&even, &beside, 40, 90).unwrap().along;
    assert!((mean - 15.0).abs() < 1.5, "{mean}");
    let mut holed = profile.clone();
    holed[65] = f32::NAN;
    assert!(place(&holed, &beside, 40, 90).is_some());
    assert_eq!(place(&[0.0; 100], &beside, 40, 90), None);
    // A measured move carries each cell's place.
    let motion = estimate(&scene(256, 144, 0.0, 0.0, 0), &scene(256, 144, 3.0, 0.0, 1)).unwrap();
    let [x, y] = motion.cells[0].unwrap().place.unwrap();
    assert!(
        x.along < -40.0 && y.along < -20.0 && x.squared > x.along * x.along * 0.9,
        "{x:?} {y:?}"
    );
    // Each across the other's axis in the cell's middle: all of it is scene.
    assert!(
        (x.across + 44.0).abs() < 3.0 && (y.across + 78.0).abs() < 3.0,
        "{x:?} {y:?}"
    );
}

#[test]
fn still_pictures_have_no_motion() {
    let a = scene(256, 144, 0.0, 0.0, 0);
    let m = estimate(&a, &a.clone()).unwrap();
    assert!(m.dx.abs() < 0.1 && m.dy.abs() < 0.1, "{m:?}");
}

#[test]
fn a_static_hud_cell_does_not_move_the_median() {
    let a = scene(256, 144, 0.0, 0.0, 0);
    let mut b = scene(256, 144, 9.0, 0.0, 1);
    // Copy the top-left cell unchanged, as a HUD would stay.
    for y in 0..48 {
        for x in 0..85 {
            b.luma[y * 256 + x] = a.luma[y * 256 + x];
        }
    }
    let m = estimate(&a, &b).unwrap();
    assert!((m.dx + 9.0).abs() < 0.6, "{m:?}");
}

/// Where [`with_hud`] draws: wide bars at the bottom left and right and a box at the top
/// right.
fn hud_at(x: usize, y: usize) -> bool {
    ((106..122).contains(&y) && !(104..166).contains(&x))
        || ((12..52).contains(&y) && (190..246).contains(&x))
}

/// `t` with a static HUD as a game draws one, the same in every capture.
fn with_hud(mut t: Thumbnail) -> Thumbnail {
    for y in 0..t.height {
        for x in 0..t.width {
            if !hud_at(x, y) {
                continue;
            }
            let bar = (110..118).contains(&y) && !(100..170).contains(&x);
            t.luma[y * t.width + x] = if bar {
                235
            } else if (x + y) % 6 < 3 {
                40
            } else {
                20
            };
        }
    }
    t
}

#[test]
fn a_known_hud_does_not_hold_back_a_tilt() {
    let hud: Vec<bool> = (0..256 * 144).map(|i| hud_at(i % 256, i / 256)).collect();
    for oy in [0.4f32, 0.8, 1.5, -1.2, 3.0] {
        let a = with_hud(scene(256, 144, 0.0, 0.0, 0));
        let b = with_hud(scene(256, 144, 4.0, oy, 1));
        let m =
            estimate_around(&a, &b, Some(&hud)).unwrap_or_else(|| panic!("no estimate for {oy}"));
        assert!((m.dy + oy).abs() < 0.35, "dy {m:?} for {oy}");
        assert!((m.dx + 4.0).abs() < 0.35, "dx {m:?} for {oy}");
    }
}

#[test]
fn what_stands_still_while_the_picture_moves_is_found_and_let_go() {
    let near_hud = |x: usize, y: usize| {
        (y.saturating_sub(2)..=y + 2).any(|y| (x.saturating_sub(2)..=x + 2).any(|x| hud_at(x, y)))
    };
    let mut still = Still::default();
    let (mut ox, mut oy) = (0.0f32, 0.0f32);
    let mut previous = with_hud(scene(256, 144, ox, oy, 0));
    // Pans with a little tilt, to and fro; then the HUD goes (a menu closes).
    for i in 1..=60u64 {
        ox += if (i / 10) % 2 == 0 { 3.3 } else { -2.7 };
        oy += if (i / 7) % 2 == 0 { 0.8 } else { -0.6 };
        let current = scene(256, 144, ox, oy, i);
        let current = if i <= 40 { with_hud(current) } else { current };
        let motion = estimate_around(&previous, &current, still.mask()).expect("a move");
        still.observe(&previous, &current, &motion);
        if i == 40 {
            let mask = still.mask().expect("a HUD");
            let marked = |wanted: bool| {
                (0..256 * 144)
                    .filter(|i| hud_at(i % 256, i / 256) == wanted && mask[*i])
                    .count()
            };
            let hud = (0..256 * 144).filter(|i| hud_at(i % 256, i / 256)).count();
            // Its rims and its pattern; the middle of its plain bar says nothing.
            assert!(marked(true) * 10 >= hud * 8, "{} of {hud}", marked(true));
            // Of the scene, only the pixels at its rim.
            let astray = (0..256 * 144)
                .filter(|i| mask[*i] && !near_hud(i % 256, i / 256))
                .count();
            assert!(astray <= 8, "{astray} of the scene");
            // And the shift is the scene's, though the host was told of no HUD.
            assert!(
                (motion.dx + 3.3).abs() < 0.35 && (motion.dy - 0.6).abs() < 0.35,
                "{motion:?}"
            );
        }
        previous = current;
    }
    let left = still
        .mask()
        .map_or(0, |mask| mask.iter().filter(|m| **m).count());
    assert!(left <= 8, "{left} still marked");
}

#[test]
fn a_pan_tells_nothing_of_a_horizon_or_a_sky() {
    // A plain sky over a plain strip of ground with a straight horizon between, texture
    // below: the pan brings every pixel of the upper two more of the same.
    let seen = |ox: f32, at_us: u64| {
        let mut t = scene(256, 144, ox, 0.0, at_us);
        for (i, value) in t.luma.iter_mut().enumerate().take(256 * 70) {
            *value = if i / 256 < 60 { 200 } else { 90 };
        }
        t
    };
    let mut still = Still::default();
    let mut previous = seen(0.0, 0);
    for i in 1..=60u64 {
        let current = seen(i as f32 * 3.5, i);
        let motion = estimate(&previous, &current).expect("a move");
        assert!(!still.observe(&previous, &current, &motion));
        previous = current;
    }
    assert_eq!(still.mask(), None);
}

#[test]
fn a_line_is_not_measured_beside_a_hud() {
    // A slow gradient: all of it goes, but where the average would lean away from the HUD.
    let mut line: Vec<f32> = (0..40).map(|i| 3.0 * i as f32).collect();
    line[20] = f32::NAN;
    detrend(&mut line, &mut Vec::new(), &mut Vec::new());
    assert!(line[14..=26].iter().all(|v| v.is_nan()), "{line:?}");
    assert!(line[13].abs() < 1e-3 && line[27].abs() < 1e-3, "{line:?}");
    // At the picture's edge it leans and stays: the cells keep their margin from it.
    assert!((line[0] + 9.0).abs() < 1e-3, "{line:?}");

    // Rows half under a HUD are the mean of other columns than their neighbours, here
    // darker ones: no step for that in the profile.
    let t = Thumbnail {
        width: 64,
        height: 64,
        luma: (0..64 * 64)
            .map(|i| if i % 64 < 32 { 200 } else { 50 })
            .collect(),
        at_us: 0,
    };
    let hud = |x: usize, y: usize| x < 32 && (24..40).contains(&y);
    let (mut lines, mut profile) = (Lines::default(), Vec::new());
    profile_rows(&t, 0, 64, 0, |x, y| !hud(x, y), &mut lines, &mut profile);
    assert!(profile.iter().all(|v| v.abs() < 1e-3), "{profile:?}");
    // And those rows lie where their columns do.
    let mut across = Vec::new();
    lines.across(&mut across);
    assert!(
        (across[10] - 0.0).abs() < 1e-3 && (across[30] - 16.0).abs() < 1e-3,
        "{across:?}"
    );
}

#[test]
fn a_still_camera_with_one_thing_moving_stands_still() {
    let a = with_hud(scene(256, 144, 0.0, 0.0, 0));
    let mut b = a.clone();
    // Something crosses the middle cell; the rest of the picture holds.
    let mover = scene(256, 144, 7.0, 2.0, 1);
    for y in 55..90 {
        for x in 100..160 {
            b.luma[y * 256 + x] = mover.luma[y * 256 + x];
        }
    }
    let m = estimate(&a, &b).unwrap();
    assert_eq!((m.dx, m.dy), (0.0, 0.0), "{m:?}");
    assert!(m.confidence > 0.5, "{m:?}");
}

#[test]
fn flat_pictures_and_mismatched_sizes_give_nothing() {
    let flat = Thumbnail {
        width: 64,
        height: 36,
        luma: vec![90; 64 * 36],
        at_us: 0,
    };
    assert_eq!(estimate(&flat, &flat), None);
    let a = scene(64, 36, 0.0, 0.0, 0);
    let b = scene(64, 40, 0.0, 0.0, 0);
    assert_eq!(estimate(&a, &b), None);
}
