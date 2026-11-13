use super::*;
use nightjar_core::frame_warp::{Tag, Tags};

fn mask(width: u16, height: u16, set: impl Fn(u16, u16) -> bool) -> Mask {
    let mut mask = Mask::new(1, width, height);
    for y in 0..height {
        for x in 0..width {
            mask.set(x, y, set(x, y));
        }
    }
    mask
}

/// What every shader path samples for the output point `uv`, and how bright it draws it (the
/// module documentation).
fn sample_at(draw: &WarpDraw, uv: [f32; 2]) -> ([f32; 2], f32) {
    let on_hud = |p: [f32; 2]| {
        draw.mask().is_some_and(|hud| {
            let cell = |v: f32, n: u16| (v * f32::from(n)).floor().clamp(0.0, f32::from(n - 1)) as u16;
            hud.mask.get(cell(p[0], hud.mask.width), cell(p[1], hud.mask.height))
        })
    };
    if on_hud(uv) {
        return (uv, 1.0);
    }
    let turned = if draw.lens == FLAT { [uv[0] - draw.shift[0], uv[1] - draw.shift[1]] } else { source(uv, draw.shift, draw.lens) };
    let (sampled, fade) = edge(turned);
    // The HUD's own pixels, which would draw it again beside itself: the point itself instead.
    if on_hud(sampled) {
        return (uv, 1.0);
    }
    (sampled, fade)
}

/// Where the last triangle that covers `at` samples it and how bright, as the GPU draws them in
/// order, interpolating linearly across each.
fn mesh_sample(mesh: &Mesh, at: [f32; 2]) -> Option<([f32; 2], f32)> {
    mesh.indices.as_chunks::<3>().0.iter().rev().find_map(|triangle| {
        let [a, b, c] = triangle.map(|i| mesh.corners[i as usize]);
        let area = (b.at[1] - c.at[1]) * (a.at[0] - c.at[0]) + (c.at[0] - b.at[0]) * (a.at[1] - c.at[1]);
        if area.abs() < 1e-12 {
            return None;
        }
        let wa = ((b.at[1] - c.at[1]) * (at[0] - c.at[0]) + (c.at[0] - b.at[0]) * (at[1] - c.at[1])) / area;
        let wb = ((c.at[1] - a.at[1]) * (at[0] - c.at[0]) + (a.at[0] - c.at[0]) * (at[1] - c.at[1])) / area;
        let wc = 1.0 - wa - wb;
        (wa >= -1e-5 && wb >= -1e-5 && wc >= -1e-5).then(|| {
            let mix = |value: fn(&Corner) -> f32| wa * value(&a) + wb * value(&b) + wc * value(&c);
            ([mix(|c| c.uv[0]), mix(|c| c.uv[1])], mix(|c| c.fade))
        })
    })
}

fn close(a: [f32; 2], b: [f32; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4
}

#[test]
fn the_shader_bytes_hold_each_cell_where_the_shaders_look() {
    for (width, height) in [(1, 1), (7, 3), (32, 2), (33, 5), (100, 9), (2048, 2)] {
        let mask = mask(width, height, |x, y| (x * 7 + y * 3) % 5 == 0 || x == width - 1);
        let (row, bytes) = mask_bytes(&mask);
        assert_eq!(row, Mask::stride(width) * 4);
        assert_eq!(bytes.len(), row * usize::from(height));
        for y in 0..height {
            for x in 0..width {
                let byte = bytes[usize::from(y) * row + usize::from(x >> 3)];
                assert_eq!(byte >> (x & 7) & 1 != 0, mask.get(x, y), "cell {x},{y} of {width}x{height}");
            }
        }
    }
}

#[test]
fn row_runs_find_every_run_and_stop_at_the_width() {
    let mut runs = Vec::new();
    // Bits past the width are set in the word: they are not cells.
    row_runs(&[0b1110_0110 | 0xFFFF_0000], 12, &mut runs);
    assert_eq!(runs, [(1, 2), (5, 3)]);
    row_runs(&[u32::MAX, 1], 33, &mut runs);
    assert_eq!(runs, [(0, 33)], "a run across words");
    row_runs(&[1 << 31, 0b10], 64, &mut runs);
    assert_eq!(runs, [(31, 1), (33, 1)]);
    row_runs(&[0, 0], 64, &mut runs);
    assert!(runs.is_empty());
}

#[test]
fn cell_rects_cover_exactly_the_set_cells_and_merge_down_the_rows() {
    type Shape = (u16, u16, fn(u16, u16) -> bool);
    let shapes: [Shape; 4] = [
        (8, 6, |x, y| (2..5).contains(&x) && (1..4).contains(&y)),
        (40, 10, |x, y| x < 3 || (x >= 30 && y >= 8)),
        (33, 7, |x, y| (x + y) % 2 == 0),
        (64, 64, |x, y| (x / 5 + y / 3) % 3 == 0),
    ];
    for (width, height, set) in shapes {
        let mask = mask(width, height, set);
        let rects = cell_rects(&mask);
        let mut drawn = Mask::new(1, width, height);
        for r in &rects {
            for y in r.y..r.y + r.h {
                for x in r.x..r.x + r.w {
                    assert!(!drawn.get(x, y), "{r:?} overlaps another rectangle");
                    drawn.set(x, y, true);
                }
            }
        }
        assert_eq!(drawn.words, mask.words, "{width}x{height}");
    }
    let block = mask(8, 6, |x, y| (2..5).contains(&x) && (1..4).contains(&y));
    assert_eq!(cell_rects(&block), [CellRect { x: 2, y: 1, w: 3, h: 3 }], "a block is one rectangle");
}

#[test]
fn hud_areas_are_fractions_and_a_fine_mask_is_drawn_coarser_but_whole() {
    let corner = mask(10, 5, |x, y| x >= 8 && y == 0);
    assert_eq!(hud_areas(&corner, MAX_HUD_RECTS), [[0.8, 0.0, 1.0, 0.2]]);
    let checker = mask(64, 64, |x, y| (x + y) % 2 == 0);
    let areas = hud_areas(&checker, 100);
    assert!(areas.len() <= 100, "{}", areas.len());
    // Every set cell's centre stays inside some area.
    for y in 0..64u16 {
        for x in 0..64u16 {
            if checker.get(x, y) {
                let c = [(f32::from(x) + 0.5) / 64.0, (f32::from(y) + 0.5) / 64.0];
                assert!(areas.iter().any(|a| c[0] >= a[0] && c[0] <= a[2] && c[1] >= a[1] && c[1] <= a[3]), "cell {x},{y}");
            }
        }
    }
    assert!(hud_areas(&Mask::default(), MAX_HUD_RECTS).is_empty());
}

#[test]
fn the_mesh_samples_and_darkens_where_the_shaders_do() {
    // A HUD in a corner, one in the middle and one on the left edge, whose ghosts the shifts
    // bring in beside them, mirrored too.
    let hud = mask(10, 10, |x, y| (x >= 7 && y >= 8) || ((4..6).contains(&x) && (3..5).contains(&y)) || (x == 0 && (2..4).contains(&y)));
    let hud = Arc::new(Hud::new(Arc::new(hud)));
    let mut mesh = Mesh::default();
    // Past one edge, past two (both darkening in a corner, alike along a slanted line), past
    // the darkening, and past a whole picture.
    let shifts = [[0.1, 0.0], [-0.05, 0.2], [0.3, -0.3], [-1.5, 0.0], [0.05, 0.03], [-0.06, -0.02], [0.12, -0.1], [0.02, 0.07]];
    for shift in shifts {
        for with_hud in [false, true] {
            let draw = WarpDraw { shift, hud: with_hud.then(|| hud.clone()), ..WarpDraw::default() };
            mesh.build(draw.shift, draw.lens, draw.mask().map_or(&[][..], |hud| hud.areas()));
            // Off every multiple of 0.01, where the shifted cells' edges are.
            for j in 0..50 {
                for i in 0..50 {
                    let at = [(i as f32 + 0.37) / 50.0, (j as f32 + 0.37) / 50.0];
                    let (uv, fade) = mesh_sample(&mesh, at).expect("the triangles cover the destination");
                    let (want_uv, want_fade) = sample_at(&draw, at);
                    assert!(close(uv, want_uv) && (fade - want_fade).abs() < 1e-4,
                        "{shift:?} hud {with_hud} at {at:?}: {uv:?} {fade}, not {want_uv:?} {want_fade}");
                }
            }
        }
    }
}

#[test]
fn the_mesh_turns_a_picture_through_a_lens_as_the_shaders_do() {
    use nightjar_core::frame_warp::lens_terms;
    let hud = mask(10, 10, |x, y| (x >= 7 && y >= 8) || ((4..6).contains(&x) && (3..5).contains(&y)) || (x == 0 && (2..4).contains(&y)));
    let hud = Arc::new(Hud::new(Arc::new(hud)));
    let mut mesh = Mesh::default();
    // Turns of a 1920x1080 picture, in pixels at its centre, through lenses of 60 to 110
    // degrees: small, both ways at once, and far past what a stream shifts by; then terms no
    // host sends, which put part of the picture level with the camera (the least depth).
    let size = (1920u16, 1080u16);
    let mut draws: Vec<([f32; 2], [f32; 4])> = [([4.0, 0.0], 960.0), ([-60.0, 25.0], 960.0), ([35.0, -70.0], 1662.8), ([150.0, 90.0], 672.2), ([-260.0, 0.0], 960.0)]
        .map(|(pixels, focal)| {
            let (pixels, lens) = lens_terms(pixels, focal, size);
            ([pixels[0] / f32::from(size.0), pixels[1] / f32::from(size.1)], lens)
        })
        .to_vec();
    draws.push(([0.05, -0.02], [-1.7, 0.4, 1.01, 1.3]));
    for (shift, lens) in draws {
        for with_hud in [false, true] {
            let draw = WarpDraw { shift, lens, hud: with_hud.then(|| hud.clone()) };
            mesh.build(draw.shift, draw.lens, draw.mask().map_or(&[][..], |hud| hud.areas()));
            // What the cells' chords may be off by (`lens_cells`), across a cell's diagonal
            // and over a triangle: `LENS_ERROR` unless the turn takes more cells than drawn.
            let cells = lens_cells(shift, lens) as f32;
            let slack = 4.0 * (lens_bend(shift, lens) / (8.0 * cells * cells)).max(LENS_ERROR);
            let mut compared = 0;
            for j in 0..60 {
                for i in 0..60 {
                    let at = [(i as f32 + 0.37) / 60.0, (j as f32 + 0.37) / 60.0];
                    let (want_uv, want_fade) = sample_at(&draw, at);
                    // On the HUD's outline a point is on one side or the other of it, and what
                    // it samples jumps: not there.
                    let steady = [[-2e-3, 0.0], [2e-3, 0.0], [0.0, -2e-3], [0.0, 2e-3]].iter().all(|d| {
                        let (near, _) = sample_at(&draw, [at[0] + d[0], at[1] + d[1]]);
                        (near[0] - want_uv[0]).abs() < 0.02 && (near[1] - want_uv[1]).abs() < 0.02
                    });
                    if !steady {
                        continue;
                    }
                    compared += 1;
                    let (uv, fade) = mesh_sample(&mesh, at).expect("the triangles cover the destination");
                    // The fill darkens by 35% over 8% of the picture.
                    assert!((uv[0] - want_uv[0]).abs() < slack && (uv[1] - want_uv[1]).abs() < slack && (fade - want_fade).abs() < 6.0 * slack,
                        "{shift:?} {lens:?} hud {with_hud} at {at:?}: {uv:?} {fade}, not {want_uv:?} {want_fade} within {slack}");
                }
            }
            assert!(compared > 3000, "{shift:?} {lens:?}: only {compared} points compared");
        }
    }
    // A small turn takes few cells, a wide one more, and never more than the most.
    assert_eq!(lens_cells([0.0, 0.0], FLAT), 4);
    let small = lens_terms([4.0, 0.0], 960.0, size);
    let wide = lens_terms([150.0, 90.0], 672.2, size);
    let cells = |(pixels, lens): ([f32; 2], [f32; 4])| lens_cells([pixels[0] / 1920.0, pixels[1] / 1080.0], lens);
    assert!(cells(small) < 12 && cells(wide) > 24, "{} {}", cells(small), cells(wide));
    assert_eq!(lens_cells([0.0, 0.0], [f32::NAN, 0.0, 1.0, 1.0]), MAX_LENS_CELLS);
    assert_eq!(lens_cells([0.1, 0.0], [-3.0, 3.0, 1.0, 1.0]), MAX_LENS_CELLS);
}

#[test]
fn a_point_whose_shifted_source_is_hud_samples_itself() {
    // A HUD on cell (5, 5) of 10x10; turning right by 0.2 would draw it again at x 0.7..0.8.
    let hud = Arc::new(Hud::new(Arc::new(mask(10, 10, |x, y| x == 5 && y == 5))));
    let draw = WarpDraw { shift: [0.2, 0.0], hud: Some(hud), ..WarpDraw::default() };
    assert_eq!(sample_at(&draw, [0.75, 0.55]), ([0.75, 0.55], 1.0), "the scene, not the HUD again");
    assert_eq!(sample_at(&draw, [0.55, 0.55]), ([0.55, 0.55], 1.0), "the HUD in place");
    let (uv, fade) = sample_at(&draw, [0.65, 0.55]);
    assert!(close(uv, [0.45, 0.55]) && fade == 1.0, "the rest shifts: {uv:?} {fade}");
    // On the left edge, turning right by 0.15: x 0.2 would sample the HUD at x 0.05, and x 0.12
    // the HUD mirrored in the edge fill, at x 0.03 (and darkened).
    let edge_hud = Arc::new(Hud::new(Arc::new(mask(10, 10, |x, y| x == 0 && y == 5))));
    let draw = WarpDraw { shift: [0.15, 0.0], hud: Some(edge_hud), ..WarpDraw::default() };
    assert_eq!(sample_at(&draw, [0.2, 0.55]), ([0.2, 0.55], 1.0));
    assert_eq!(sample_at(&draw, [0.12, 0.55]), ([0.12, 0.55], 1.0), "nor the mirrored HUD");
    // The SDL renderer path's triangles draw the same.
    let mut mesh = Mesh::default();
    mesh.build(draw.shift, draw.lens, draw.mask().unwrap().areas());
    for at in [[0.2, 0.55], [0.12, 0.55]] {
        let (uv, fade) = mesh_sample(&mesh, at).unwrap();
        assert!(close(uv, at) && (fade - 1.0).abs() < 1e-6, "{at:?}: {uv:?} {fade}");
    }
}

#[test]
fn the_edge_fill_mirrors_and_darkens_rather_than_repeating_the_edge() {
    // Turning right by a tenth of the picture: its left tenth shows the picture mirrored.
    let draw = WarpDraw { shift: [0.1, 0.0], ..WarpDraw::default() };
    let (uv, fade) = sample_at(&draw, [0.02, 0.5]);
    assert!(close(uv, [0.08, 0.5]) && (fade - 0.65).abs() < 1e-6, "{uv:?} {fade}");
    let (uv, fade) = sample_at(&draw, [0.07, 0.5]);
    assert!(close(uv, [0.03, 0.5]) && (fade - (1.0 - 0.35 * 0.375)).abs() < 1e-6, "{uv:?} {fade}");
    assert_eq!(sample_at(&draw, [0.5, 0.5]).1, 1.0, "the picture itself as it is");
}

#[test]
fn the_geometry_maps_the_source_rectangle_as_sdl_does() {
    let mut mesh = Mesh::default();
    mesh.build([0.0, 0.0], FLAT, &[]);
    let whole = |x: f32, y: f32| Corner { at: [x, y], uv: [x, y], fade: 1.0 };
    assert_eq!(mesh.corners, [whole(0.0, 0.0), whole(1.0, 0.0), whole(1.0, 1.0), whole(0.0, 1.0)], "no shift is one quad");
    // A 1920x1080 picture in a 1920x1088 texture, drawn half size at (10, 20).
    let source = SDL_FRect { x: 0.0, y: 0.0, w: 1919.5, h: 1079.5 };
    let destination = SDL_FRect { x: 10.0, y: 20.0, w: 960.0, h: 540.0 };
    let (vertices, indices) = mesh.geometry(source, (1920.0, 1088.0), destination);
    assert_eq!(indices, [0, 1, 2, 0, 2, 3]);
    let corners: Vec<_> = vertices.iter().map(|v| (v.position.x, v.position.y, v.tex_coord.x, v.tex_coord.y, v.color.r)).collect();
    assert_eq!(corners, [
        (10.0, 20.0, 0.0, 0.0, 1.0),
        (970.0, 20.0, 1919.5 / 1920.0, 0.0, 1.0),
        (970.0, 560.0, 1919.5 / 1920.0, 1079.5 / 1088.0, 1.0),
        (10.0, 560.0, 0.0, 1079.5 / 1088.0, 1.0),
    ]);
}

#[test]
fn a_shifted_present_reuses_the_meshs_buffers() {
    let areas = hud_areas(&mask(64, 36, |x, y| (x + y) % 3 == 0), MAX_HUD_RECTS);
    let mut mesh = Mesh::default();
    let source = SDL_FRect { x: 0.0, y: 0.0, w: 64.0, h: 36.0 };
    // How many triangles a shift takes varies with where the HUD's ghosts fall: the buffers
    // grow to the most once. Through a lens too.
    let shifts = [([0.05, -0.03], FLAT), ([0.04, -0.02], [0.1, -0.04, 1.0, 1.01]), ([-0.01, 0.0], FLAT)];
    for (shift, lens) in shifts {
        mesh.build(shift, lens, &areas);
        mesh.geometry(source, (64.0, 36.0), source);
    }
    let buffers = (mesh.corners.as_ptr(), mesh.indices.as_ptr(), mesh.vertices.as_ptr());
    for (shift, lens) in shifts {
        mesh.build(shift, lens, &areas);
        mesh.geometry(source, (64.0, 36.0), source);
        assert_eq!((mesh.corners.as_ptr(), mesh.indices.as_ptr(), mesh.vertices.as_ptr()), buffers, "{shift:?}");
    }
}

#[test]
fn every_hlsl_presenter_includes_the_one_definition_and_direct3d_gets_it_spliced() {
    for (name, source) in [
        ("direct.hlsl", include_str!("../../direct.hlsl")),
        ("packed444.hlsl", include_str!("../../packed444.hlsl")),
        ("planes_direct.hlsl", include_str!("../../planes_direct.hlsl")),
        ("pyrowave.hlsl", include_str!("../../pyrowave.hlsl")),
    ] {
        assert_eq!(source.matches("#include \"frame_warp.hlsl\"").count(), 1, "{name}");
        assert!(!source.contains("hudMask.Load"), "{name} reads the mask itself");
        let spliced = hlsl(source.as_bytes());
        let spliced = std::str::from_utf8(&spliced).unwrap();
        assert!(!spliced.contains("#include") && spliced.contains("float2 frameWarpEdge("), "{name}");
    }
    assert!(matches!(hlsl(b"float4 x;"), Cow::Borrowed(_)), "a shader without it is left alone");
    let twice = hlsl(b"a\n#include \"frame_warp.hlsl\"\nb\n#include \"frame_warp.hlsl\"\nc");
    assert_eq!(twice.as_ref(), format!("a\n{HLSL}\nb\n{HLSL}\nc").as_bytes());
    // The shaders darken as nightjar_core's edge does.
    let darken = format!("1 - {} * saturate(max(outside.x, outside.y) / {})", nightjar_core::frame_warp::EDGE_DARKEN, EDGE_RAMP);
    assert!(HLSL.contains(&darken), "frame_warp.hlsl's fade is not `{darken}`");
    // And keep a point whose shifted source is on the HUD where it is, as `sample_at` does.
    assert!(HLSL.contains("if (frameWarpHud(source, size)) return q;"), "frame_warp.hlsl draws the HUD's ghost");
    // A flat picture is shifted as it was before lenses, and a lens holds the same least depth.
    assert!(HLSL.contains("if (frameWarpFlat()) return q - shift;"), "frame_warp.hlsl turns a flat picture");
    assert!(HLSL.contains(&format!("max(1 + dot(c, warpLens.xy), {MIN_DEPTH})")), "frame_warp.hlsl's least depth is not {MIN_DEPTH}");
    assert!(HLSL.contains(&format!("all(warpLens == float4({}, {}, {}, {}))", FLAT[0], FLAT[1], FLAT[2], FLAT[3])));
}

#[test]
fn the_warp_row_is_zero_without_a_shift_and_names_the_mask_only_when_bound() {
    let hud = Arc::new(Hud::new(Arc::new(mask(64, 36, |x, _| x == 0))));
    let still = WarpDraw { shift: [0.0, 0.0], lens: [0.1, 0.0, 1.0, 1.01], hud: Some(hud.clone()) };
    assert_eq!((still.row(true), still.lens_row()), ([0.0; 4], FLAT));
    assert!(still.mask().is_none(), "no shift binds no mask");
    let draw = WarpDraw { shift: [0.01, -0.02], hud: Some(hud), ..WarpDraw::default() };
    assert_eq!(draw.row(true), [0.01, -0.02, 64.0, 36.0]);
    assert_eq!(draw.row(false), [0.01, -0.02, 0.0, 0.0], "a mask that could not be bound is none");
    assert_eq!(draw.lens_row(), FLAT, "a flat picture");
    let inactive = Warp { pixels: [0.001, 0.0], uv: [0.000_001, 0.0], lens: [0.1, 0.0, 1.0, 1.01], active: false, mask_generation: None };
    assert!(WarpDraw::of(&inactive, None).is_identity());
    assert_eq!(WarpDraw::of(&inactive, None).lens, FLAT);
    let without_mask = Warp { pixels: [5.0, 0.0], uv: [0.005, 0.0], lens: [0.02, 0.0, 1.0, 1.0001], active: true, mask_generation: None };
    assert_eq!(WarpDraw::of(&without_mask, None).lens_row(), [0.02, 0.0, 1.0, 1.0001], "the lens goes to the shaders");
    let held = Arc::new(Hud::new(Arc::new(Mask::new(1, 1, 1))));
    assert!(WarpDraw::of(&without_mask, Some(&held)).hud.is_none(), "the predictor knows no mask");
}

const TIMING: Timing = Timing { refresh_us: 10_000, vsync: false, next_picture_us: None };

#[test]
fn a_repeat_needs_a_refresh_since_the_last_present_and_a_quarter_pixel() {
    let mut repeat = Repeat::default();
    assert!(!repeat.decide(0, [5.0, 0.0], TIMING), "nothing presented yet");
    repeat.presented(1_000, [1.0, 1.0]);
    assert_eq!(repeat.due_us(10_000), Some(11_000));
    assert!(!repeat.decide(10_999, [9.0, 9.0], TIMING), "within a refresh of the last present");
    assert!(!repeat.decide(11_000, [1.1, 1.1], TIMING), "0.14 px is too little");
    assert_eq!(repeat.due_us(10_000), Some(21_000), "the next look is a refresh later");
    assert!(!repeat.decide(15_000, [3.0, 1.0], TIMING));
    assert!(repeat.decide(21_000, [1.0, 1.25], TIMING), "a quarter pixel goes out");
}

#[test]
fn a_repeat_never_takes_the_next_pictures_refresh() {
    let mut repeat = Repeat::default();
    repeat.presented(0, [0.0, 0.0]);
    let vsync = |next: u64| Timing { refresh_us: 10_000, vsync: true, next_picture_us: Some(next) };
    assert!(!repeat.decide(10_000, [2.0, 0.0], vsync(19_000)), "the picture comes within a refresh");
    assert_eq!(repeat.due_us(10_000), Some(24_001), "looks again when it is half a refresh late");
    assert!(repeat.decide(24_001, [2.0, 0.0], vsync(19_000)), "a late picture leaves the refresh to the repeat");
    let mut repeat = Repeat::default();
    repeat.presented(0, [0.0, 0.0]);
    assert!(repeat.decide(10_000, [2.0, 0.0], vsync(20_001)), "the picture comes after the next refresh");
    // Without V-Sync half a refresh is enough room.
    let mut repeat = Repeat::default();
    repeat.presented(0, [0.0, 0.0]);
    let immediate = Timing { vsync: false, next_picture_us: Some(16_000), ..TIMING };
    assert!(repeat.decide(10_000, [2.0, 0.0], immediate));
    let mut repeat = Repeat::default();
    repeat.presented(0, [0.0, 0.0]);
    assert!(!repeat.decide(10_000, [2.0, 0.0], Timing { next_picture_us: Some(14_000), ..immediate }));
}

fn locked_mouse() -> Model {
    Model {
        generation: 1,
        mouse_locked: true,
        picture_width: 1000,
        picture_height: 500,
        mouse_gain: [-0.5, -0.25],
        max_offset: [200.0, 100.0],
        lag_us: 38_000,
        confidence: 0.94,
        ..Model::default()
    }
}

/// A Frame Warp whose frame 5 lacks 40 mouse counts in x: -20 px once its fade is in.
fn moving() -> Arc<FrameWarp> {
    let warp = FrameWarp::new(Some(0), MASK_EDGE);
    warp.on_model(locked_mouse());
    warp.note_mouse(100, 40);
    warp.on_tags(&Tags { model_generation: 1, tags: vec![Tag { frame: 5, mouse_x: 60, mouse_y: 40, ..Tag::default() }] });
    warp
}

#[test]
fn a_raced_frame_keeps_the_shift_of_its_first_slice_to_its_own_present() {
    let mut presenter = Presenter::new(moving());
    presenter.picture(4, 10, 0);
    let start = Instant::now();
    // The fade starts at the first slice: no shift yet.
    assert!(presenter.draw_at(Some(11), start).is_identity());
    let later = start + std::time::Duration::from_secs(1);
    assert!(presenter.draw_at(Some(11), later).is_identity(), "the frame's later slices draw as its first did");
    presenter.picture(5, 11, 0);
    assert!(presenter.draw_at(None, later).is_identity(), "its own present too");
    let redraw = presenter.draw_at(None, later);
    assert!((redraw.shift[0] + 0.02).abs() < 1e-6 && redraw.shift[1] == 0.0, "a redraw takes the shift now: {redraw:?}");
    let next = presenter.draw_at(Some(12), later);
    assert!(!next.is_identity(), "the next raced frame takes its own");
}

#[test]
fn a_repeat_draws_the_shift_it_checked_and_records_it() {
    let warp = moving();
    let mut presenter = Presenter::new(warp.clone());
    presenter.model(&locked_mouse());
    presenter.picture(5, 0, 1_000);
    let start = Instant::now();
    // The fade starts: drawn unshifted.
    presenter.draw_at(None, start);
    presenter.presented(1_000);
    assert_eq!(presenter.repeat_due_us(10_000, false), None, "no shift drawn and no input since: no wake");
    warp.note_mouse(2, 0);
    assert_eq!(presenter.repeat_due_us(10_000, false), Some(11_000), "locked, with input since: it looks a refresh later");
    assert_eq!(presenter.repeat_due_us(10_000, true), None, "never while slices race");
    // A second on, the fade is in.
    let later = start + std::time::Duration::from_secs(1);
    assert!(presenter.repeat_at(11_000, later, TIMING, 100_000), "the shift moved 21 px");
    let drawn = presenter.draw_at(None, later + std::time::Duration::from_secs(1));
    assert!((drawn.shift[0] + 0.021).abs() < 1e-6, "the checked shift: {drawn:?}");
    presenter.presented(11_500);
    assert!(!presenter.repeat_at(21_500, later, TIMING, 100_000), "nothing moved since");
    warp.note_mouse(10, 0);
    assert!(!presenter.repeat_at(31_500, later, Timing { next_picture_us: Some(33_000), ..TIMING }, 100_000),
        "a picture waiting within half a refresh takes it");
    assert!(presenter.repeat_at(38_501, later, TIMING, 100_000), "the 5 px it moved go out once no picture is that close");
}

#[test]
fn an_idle_locked_stream_wakes_only_for_input_a_held_stick_or_a_drawn_shift() {
    // Locked on both, nothing in flight: frame 5 reflects every count and packet sent.
    let warp = FrameWarp::new(Some(0), MASK_EDGE);
    warp.on_model(Model { stick_locked: true, stick_gain: [-900.0, 400.0], stick_deadzone: 0.2, ..locked_mouse() });
    warp.note_controller(0, (0, 0));
    warp.on_tags(&Tags { model_generation: 1, tags: vec![Tag { frame: 5, controller_seq: 1, controller_held_us: 0, ..Tag::default() }] });
    let mut presenter = Presenter::new(warp.clone());
    presenter.model(&warp.status().model.unwrap());
    presenter.picture(5, 0, 1_000);
    let start = Instant::now();
    presenter.draw_at(None, start);
    presenter.presented(1_000);
    assert_eq!(presenter.repeat_due_us(10_000, false), None, "idle: the loop sleeps");
    warp.note_controller(0, (2000, 0));
    assert_eq!(presenter.repeat_due_us(10_000, false), Some(11_000), "a packet since the present");
    presenter.draw_at(None, start);
    presenter.presented(2_000);
    assert_eq!(presenter.repeat_due_us(10_000, false), None, "drawn with it, and the stick rests in its dead zone");
    warp.note_controller(0, (20000, 0));
    presenter.draw_at(None, start);
    presenter.presented(3_000);
    assert_eq!(presenter.repeat_due_us(10_000, false), Some(13_000), "a held stick turns on every refresh");
    presenter.model(&Model::default());
    assert_eq!(presenter.repeat_due_us(10_000, false), None, "unlocked, with nothing drawn shifted");
    presenter.repeat.presented(3_000, [4.0, 0.0]);
    assert_eq!(presenter.repeat_due_us(10_000, false), Some(13_000), "a drawn shift fades out");
}

#[test]
fn the_model_logs_lock_and_unlock_once_each() {
    let mut presenter = Presenter::new(FrameWarp::new(Some(0), MASK_EDGE));
    assert_eq!(presenter.model(&Model::default()), None, "learning from the start");
    assert_eq!(presenter.model(&locked_mouse()).as_deref(), Some("nightjar: Frame Warp: locked (0.50 px/count), the picture follows the input"));
    assert_eq!(presenter.model(&locked_mouse()), None);
    assert_eq!(presenter.model(&Model::default()).as_deref(), Some("nightjar: Frame Warp: unlocked, learning again"));
}

#[test]
fn the_overlay_line_says_what_locked_and_how_far_it_shifts() {
    assert_eq!(status_line(None), "not offered by the PC");
    assert_eq!(status_line(Some(&Status::default())), "learning");
    let fitting = Status { model: Some(Model { confidence: 0.42, ..Model::default() }), ..Status::default() };
    assert_eq!(status_line(Some(&fitting)), "learning (42% fit)");
    let mouse = Status { model: Some(Model { mouse_gain: [0.94, -0.5], ..locked_mouse() }), pixels: [12.0, 0.0], peak: 41.0, ..Status::default() };
    assert_eq!(status_line(Some(&mouse)), "locked 0.94 px/count, lag 38 ms, shift 12 px (peak 41)");
    let both = Status {
        model: Some(Model { stick_locked: true, stick_gain: [-900.0, 400.0], ..locked_mouse() }),
        pixels: [3.0, 4.0],
        peak: 5.0,
        mask_cells: 120,
        ..Status::default()
    };
    assert_eq!(status_line(Some(&both)), "locked 0.50 px/count, 900 px/s, lag 38 ms, shift 5 px (peak 5), HUD 120 cells");
    let lens = Status { model: Some(Model { focal: 500.0, ..locked_mouse() }), ..Status::default() };
    assert_eq!(status_line(Some(&lens)), "locked 0.50 px/count, 90 degree lens, lag 38 ms, shift 0 px (peak 0)");
}

#[test]
fn the_notice_is_said_once_per_pc_until_frame_warp_runs_there() {
    let mut pc = HostRecord::default();
    let text = "This PC doesn’t offer Frame Warp, so Nightjar streams without it.";
    assert_eq!(frame_warp_notice(&mut pc, false, true), (Some(text.to_owned()), true), "the first time");
    assert_eq!(frame_warp_notice(&mut pc, false, true), (None, false), "then not again");
    assert_eq!(frame_warp_notice(&mut pc, false, false), (None, false), "the setting off changes nothing");
    assert_eq!(frame_warp_notice(&mut pc, true, false), (None, true), "it runs there now");
    assert_eq!(frame_warp_notice(&mut pc, true, false), (None, false));
    assert_eq!(frame_warp_notice(&mut pc, false, true), (Some(text.to_owned()), true), "and is said again once it is gone");
}

#[test]
fn the_hlsl_include_is_pasted_in_place_and_other_sources_stay_borrowed() {
    let plain = b"float4 main() : SV_Target { return 0; }";
    assert!(matches!(hlsl(plain), Cow::Borrowed(_)));
    let source = b"a\n#include \"frame_warp.hlsl\"\nb\n#include \"frame_warp.hlsl\"\nc";
    let pasted = hlsl(source);
    let expected = format!("a\n{HLSL}\nb\n{HLSL}\nc");
    assert_eq!(&*pasted, expected.as_bytes());
}
