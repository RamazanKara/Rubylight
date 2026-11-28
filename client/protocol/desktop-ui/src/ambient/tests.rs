//! Unit tests for ambient backdrop baking, Day/Night luminance envelopes, hero washes,
//! direct-indexed Gaussian blur, and worker-prepared cover registration.

use super::*;

fn luma(c: Color32) -> f32 {
    let linear = [c.r(), c.g(), c.b()].map(|v| to_linear(f32::from(v) / 255.0));
    (0..3).map(|i| linear[i] * LUMA[i]).sum()
}

#[test]
fn a_white_cover_bakes_to_the_safe_grey_the_spec_names() {
    let white = ColorImage::filled([600, 900], Color32::WHITE);
    let out = bake(&white);
    assert_eq!(out.size, SIZE);
    for pixel in &out.pixels {
        assert!(
            luma(*pixel) <= t::AMBIENT_TEXT_ZONE_MAX_LUMA + 0.003,
            "{pixel:?}"
        );
        let grey = (to_srgb(t::AMBIENT_TEXT_ZONE_MAX_LUMA) * 255.0).round() as i32;
        assert!((i32::from(pixel.r()) - grey).abs() <= 1, "{pixel:?}");
    }
}

#[test]
fn a_dark_cover_keeps_its_colour_and_a_black_one_stays_black() {
    let dark = ColorImage::filled([300, 450], Color32::from_rgb(40, 10, 60));
    let c = bake(&dark).pixels[500];
    assert!(c.b() > c.r() && c.r() > c.g(), "{c:?}");
    let black = bake(&ColorImage::filled([300, 450], Color32::BLACK));
    assert!(black
        .pixels
        .iter()
        .all(|p| p.r() <= 1 && p.g() <= 1 && p.b() <= 1));
}

#[test]
fn yellow_moves_toward_amber() {
    let [h, ..] = rgb_to_hsv(grade([0.9, 0.85, 0.1]));
    assert!(h < 45.0, "{h}");
}

#[test]
fn the_bake_sees_the_whole_cover_squashed_not_a_crop() {
    // Red top, blue bottom: both ends of the cover reach the thumbnail.
    let mut image = ColorImage::filled([200, 300], Color32::from_rgb(90, 0, 0));
    for pixel in &mut image.pixels[200 * 150..] {
        *pixel = Color32::from_rgb(0, 0, 90);
    }
    let out = bake(&image);
    let (top, bottom) = (out.pixels[48], out.pixels[96 * 53 + 48]);
    assert!(top.r() > top.b(), "{top:?}");
    assert!(bottom.b() > bottom.r(), "{bottom:?}");
}

#[test]
fn a_warm_cover_glows_warm_a_blue_one_blue_and_a_black_one_not_at_all() {
    let warm = glow_of(&bake(&ColorImage::filled(
        [200, 300],
        Color32::from_rgb(200, 70, 50),
    )));
    assert!(warm.r() > warm.g() * 2 && warm.r() > warm.b() * 2, "{warm:?}");
    let alpha = f32::from(warm.a()) / 255.0;
    assert!((alpha - GLOW_ALPHA).abs() < 0.02, "the spec's strength: {alpha}");
    let blue = glow_of(&bake(&ColorImage::filled(
        [200, 300],
        Color32::from_rgb(30, 60, 200),
    )));
    assert!(blue.b() > blue.r() * 2, "{blue:?}");
    let black = glow_of(&bake(&ColorImage::filled([200, 300], Color32::BLACK)));
    assert!(black.a() < 16, "{black:?}");
}

#[test]
fn a_tile_gradient_stays_under_the_cap() {
    for (top, bottom) in [
        (t::TILE_PAIR6_TOP, t::TILE_PAIR6_BOTTOM),
        (t::TILE_PAIR0_TOP, t::TILE_PAIR0_BOTTOM),
    ] {
        let out = bake_tile(top, bottom);
        assert!(out
            .pixels
            .iter()
            .all(|p| luma(*p) <= t::AMBIENT_TEXT_ZONE_MAX_LUMA + 0.003));
    }
}

/// The stage reads as light on a matte floor, never as a disc or a cone: brightest under the
/// light, dimmer every way from there, no step anywhere larger than the noise, and the floor's
/// own colour where the light ends.
#[test]
fn the_stage_light_falls_off_with_no_edge_and_leaves_the_floor() {
    crate::theme::own_palette();
    let baked = bake_stage([0.5, 0.02], [0.6, 0.66], 0.6);
    let [w, h] = baked.size;
    assert_eq!([w, h], STAGE_SIZE);
    let at = |x: usize, y: usize| f32::from(baked.pixels[y * w + x].g());
    // Over a few texels, so the half level of noise does not count.
    let smooth = |x: usize, y: usize| (0..16).map(|k| at(x + k, y)).sum::<f32>() / 16.0;
    let middle = w / 2 - 8;
    let lit = smooth(middle, 0);
    assert!(lit > f32::from(shade::SKY_TOP.get().g()) + 8.0, "the light shows: {lit}");
    assert!(lit > smooth(0, 0) + 4.0, "less to the sides: {lit} and {}", smooth(0, 0));
    let mut last = lit;
    for y in 1..h {
        let here = smooth(middle, y);
        assert!(here <= last + 0.5, "row {y}: it only falls, {last} to {here}");
        last = here;
    }
    for y in 0..h {
        for x in 0..w {
            let here = at(x, y);
            if x + 1 < w {
                assert!((at(x + 1, y) - here).abs() <= 2.0, "an edge at {x}, {y}");
            }
            if y + 1 < h {
                assert!((at(x, y + 1) - here).abs() <= 2.0, "an edge at {x}, {y}");
            }
        }
    }
    let floor = f32::from(shade::SKY_BOTTOM.get().g());
    assert!((smooth(middle, h - 1) - floor).abs() <= 1.0, "the floor at the foot");
    assert_eq!(stage_light(1.0), 0.0);
    assert_eq!(stage_light(0.0), 1.0);
}

#[test]
fn tiny_covers_bake_without_gaps() {
    let out = bake(&ColorImage::filled([2, 3], Color32::from_rgb(10, 60, 90)));
    assert!(
        out.pixels.iter().all(|p| p.b() > 20),
        "no empty cell stays black"
    );
}

#[test]
fn bake_day_lifts_dark_and_coloured_covers_to_pastel_alabaster_floor() {
    let black = bake_day(&ColorImage::filled([300, 450], Color32::BLACK));
    assert!(
        black
            .pixels
            .iter()
            .all(|p| luma(*p) >= t::DAY_AMBIENT_TEXT_ZONE_MIN_LUMA - 0.01),
        "black cover lifts cleanly to Day floor"
    );
    let cobalt = bake_day(&ColorImage::filled(
        [300, 450],
        Color32::from_rgb(20, 60, 210),
    ));
    let mid = cobalt.pixels[500];
    assert!(
        luma(mid) >= t::DAY_AMBIENT_TEXT_ZONE_MIN_LUMA - 0.01 && mid.b() > mid.r() + 18,
        "pastel chroma kept: {mid:?}"
    );
}

#[test]
fn bake_hero_dissolves_high_contrast_lettering_into_smooth_wash() {
    let mut cover = ColorImage::filled([300, 450], Color32::from_rgb(20, 40, 120));
    // Sharp white horizontal/vertical bars simulating giant title lettering in the upper third.
    for y in 40..110 {
        for x in 30..270 {
            if (x / 12) % 2 == 0 || (y / 14) % 2 == 0 {
                cover.pixels[y * 300 + x] = Color32::WHITE;
            }
        }
    }
    let night = bake_hero(&cover, false);
    let day = bake_hero(&cover, true);
    assert_eq!(night.size, HERO_SIZE);
    assert_eq!(day.size, HERO_SIZE);
    let [w, h] = HERO_SIZE;
    for y in 0..h {
        for x in 0..w {
            let p = night.pixels[y * w + x];
            assert!(luma(p) <= t::AMBIENT_TEXT_ZONE_MAX_LUMA + 0.004, "{p:?}");
            if x + 1 < w {
                let next = night.pixels[y * w + x + 1];
                let step = (i32::from(p.r()) - i32::from(next.r()))
                    .abs()
                    .max((i32::from(p.g()) - i32::from(next.g())).abs())
                    .max((i32::from(p.b()) - i32::from(next.b())).abs());
                assert!(step <= 4, "no sharp letter edge survives at ({x},{y}): step={step}");
            }
            let dp = day.pixels[y * w + x];
            assert!(luma(dp) >= t::DAY_AMBIENT_TEXT_ZONE_MIN_LUMA - 0.01, "{dp:?}");
        }
    }
}

#[test]
fn dark_covers_and_large_libraries_resolve_by_texture_id_without_collision_or_eviction() {
    let ctx = Context::default();
    let other_ctx = Context::default();
    // Two dark/monochrome covers with identical `glow == TRANSPARENT` but distinct top/bottom luminance
    // and >300 registered covers never cross-contaminate or evict live entries, nor leak across Contexts.
    let mut cover_a = ColorImage::filled([120, 180], Color32::BLACK);
    for p in &mut cover_a.pixels[..120 * 60] {
        *p = Color32::from_rgb(38, 38, 38);
    }
    let mut cover_b = ColorImage::filled([120, 180], Color32::BLACK);
    for p in &mut cover_b.pixels[120 * 120..] {
        *p = Color32::from_rgb(220, 40, 40);
    }
    let art_a = TextureId::Managed(0);
    let amb_a = TextureId::Managed(1);
    let art_b = TextureId::Managed(2);
    let amb_b = TextureId::Managed(3);
    register_cover(&ctx, Some(art_a), Some(amb_a), &cover_a);
    for idx in 0..300u64 {
        let filler = ColorImage::filled([24, 36], Color32::from_rgb((idx % 200) as u8, 10, 20));
        register_cover(
            &ctx,
            Some(TextureId::User(910_000 + idx * 2)),
            Some(TextureId::User(910_001 + idx * 2)),
            &filler,
        );
    }
    register_cover(&ctx, Some(art_b), Some(amb_b), &cover_b);

    // Separate `egui::Context` never sees `ctx`'s `Managed(0..1)` entries.
    assert!(day_for(&other_ctx, amb_a, Color32::TRANSPARENT).is_none());
    assert!(day_for(&ctx, TextureId::User(999_999), Color32::TRANSPARENT).is_none());
    assert!(hero_for(&ctx, TextureId::User(999_998), None, Some(Color32::TRANSPARENT), false).is_none());

    // `cover_a` survives >256 later registrations and keeps its neutral top-lit wash (not `cover_b`'s red bottom).
    let hero_a = hero_for(&ctx, art_a, Some(amb_a), Some(Color32::TRANSPARENT), false).expect("hero_a");
    let hero_b = hero_for(&ctx, art_b, Some(amb_b), Some(Color32::TRANSPARENT), false).expect("hero_b");
    let bottom_idx = HERO_SIZE[0] * (HERO_SIZE[1] - 8) + HERO_SIZE[0] / 2;
    assert!(
        hero_b.pixels[bottom_idx].r() > hero_b.pixels[bottom_idx].b() + 15,
        "cover_b keeps red bottom: {:?}",
        hero_b.pixels[bottom_idx]
    );
    assert!(
        (i32::from(hero_a.pixels[bottom_idx].r()) - i32::from(hero_a.pixels[bottom_idx].b())).abs() <= 2,
        "cover_a stays neutral rather than colliding with cover_b: {:?}",
        hero_a.pixels[bottom_idx]
    );
    assert!(day_for(&ctx, amb_a, Color32::TRANSPARENT).is_some(), "day_for survives >256 covers");

    // Even if `art_a` is dropped (`Art::Dropped`), `amb_a` still resolves `hero_for`.
    retain_covers(&ctx, |id| id == amb_a || id == art_b || id == amb_b);
    assert!(
        hero_for(&ctx, TextureId::User(888_888), Some(amb_a), None, true).is_some(),
        "retained ambient_id still resolves Day hero wash after cover texture drop"
    );
}

#[test]
fn prepared_cover_and_rebind_art_match_direct_bake_hero_for_night_and_day() {
    let mut cover = ColorImage::filled([120, 180], Color32::from_rgb(200, 40, 30));
    for p in &mut cover.pixels[120 * 90..] {
        *p = Color32::from_rgb(20, 60, 210);
    }
    let prepared = PreparedCover::from_cover(&cover);
    assert!(prepared.hero_blur.is_some());

    let ctx = Context::default();
    let art_id = TextureId::Managed(10);
    let amb_id = TextureId::Managed(11);
    let sharp_id = TextureId::Managed(12);
    register_prepared(&ctx, Some(art_id), Some(amb_id), prepared);

    for day in [false, true] {
        let direct = bake_hero(&cover, day);
        let precomputed = hero_for(&ctx, art_id, Some(amb_id), None, day).expect("prepared hero");
        assert_eq!(precomputed.size, direct.size);
        for (a, b) in direct.pixels.iter().zip(&precomputed.pixels) {
            assert!(
                (i32::from(a.r()) - i32::from(b.r())).abs() <= 2
                    && (i32::from(a.g()) - i32::from(b.g())).abs() <= 2
                    && (i32::from(a.b()) - i32::from(b.b())).abs() <= 2,
                "prepared wash differs from direct bake (day={day}): {a:?} vs {b:?}"
            );
        }
    }

    assert!(rebind_art(&ctx, art_id, sharp_id, Some(amb_id)));
    assert!(hero_for(&ctx, sharp_id, None, None, false).is_some());
    assert!(day_for(&ctx, amb_id, Color32::TRANSPARENT).is_some());
}
