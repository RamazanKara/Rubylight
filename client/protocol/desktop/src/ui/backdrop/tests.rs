use super::*;

fn frame(ctx: &Context, ambient: &mut Ambient, want: Want, time: f64) -> Vec<Shape> {
    let mut want = Some(want);
    let mut shapes = Vec::new();
    let output = ctx.run_ui(
        egui::RawInput {
            time: Some(time),
            ..Default::default()
        },
        |ui| shapes = ambient.paint(ui.ctx(), want.take().expect("one pass")),
    );
    let mut delta = output.textures_delta;
    delta.clear();
    shapes
}

fn texture(ctx: &Context, value: u8) -> TextureHandle {
    ctx.load_texture(
        "t",
        egui::ColorImage::filled([2, 2], Color32::from_gray(value)),
        TextureOptions::LINEAR,
    )
}

fn art(key: &str, texture: &TextureHandle) -> Want {
    Want::Art {
        key: key.to_owned(),
        texture: texture.clone(),
        glow: Color32::from_rgba_premultiplied(60, 20, 20, 60),
    }
}

fn star_heights(shapes: &[Shape]) -> Vec<f32> {
    shapes
        .iter()
        .filter_map(|shape| match shape {
            Shape::Circle(circle) if circle.radius < 2.0 && circle.fill.a() > 0 => {
                Some(circle.center.y)
            }
            _ => None,
        })
        .collect()
}

/// pcs-1280x800 had a star by the S of a PC's name and game-1280x800 one inside Favourite: a
/// tint blurs nothing, so no star is painted under one, and the open sky keeps its stars.
#[test]
fn no_star_shows_through_a_see_through_tint() {
    crate::theme::own_palette();
    let ctx = Context::default();
    let mut ambient = Ambient::default();
    frame(&ctx, &mut ambient, Want::Sky(Moon::High), 0.0);
    let bare = frame(&ctx, &mut ambient, Want::Sky(Moon::High), 5.0);
    assert!(star_heights(&bare).len() > 10, "the sky has its stars");
    let mut shapes = Vec::new();
    let mut half = 0.0;
    let output = ctx.run_ui(
        egui::RawInput {
            time: Some(5.1),
            ..Default::default()
        },
        |ui| {
            let screen = ui.ctx().content_rect();
            half = screen.center().y;
            let card = Rect::from_min_max(screen.min, pos2(screen.right(), half));
            glass::group(&mut Vec::new(), ui.ctx(), card, 12.0, Color32::from_black_alpha(80));
            shapes = ambient.paint(ui.ctx(), Want::Sky(Moon::High));
        },
    );
    let mut delta = output.textures_delta;
    delta.clear();
    let left = star_heights(&shapes);
    assert!(left.len() < star_heights(&bare).len(), "the tint hid some");
    assert!(left.iter().all(|y| *y > half), "none under the tint: {left:?}");
}

/// The orbit is the only thing that asks for frames at rest, and only while discovery runs.
#[test]
fn the_first_runs_orbit_repaints_only_while_discovery_runs() {
    crate::theme::own_palette();
    let ctx = Context::default();
    let mut ambient = Ambient::default();
    for time in [0.0, 5.0, 5.1] {
        frame(&ctx, &mut ambient, Want::Sky(Moon::Searching), time);
    }
    assert!(
        !ctx.has_requested_repaint(),
        "no discovery, no dot, no frames"
    );
    ambient.set_orbit(true);
    let one = frame(&ctx, &mut ambient, Want::Sky(Moon::Searching), 6.0);
    assert!(ctx.has_requested_repaint(), "the dot moves");
    let two = frame(&ctx, &mut ambient, Want::Sky(Moon::Searching), 7.0);
    let dot = |shapes: &[Shape]| {
        shapes.iter().rev().find_map(|s| match s {
            Shape::Circle(c) => Some(c.center),
            _ => None,
        })
    };
    assert_ne!(dot(&one), dot(&two), "around its ring");
    // The moon's other pages never run it, even with discovery on.
    frame(&ctx, &mut ambient, Want::Sky(Moon::High), 20.0);
    ambient.set_orbit(false);
    for time in [21.0, 22.0] {
        frame(&ctx, &mut ambient, Want::Sky(Moon::High), time);
    }
    assert!(!ctx.has_requested_repaint(), "at rest again");
}

/// One moon moves between the first run's two places: it never fades out and in again.
#[test]
fn the_first_runs_moon_moves_and_does_not_fade() {
    crate::theme::own_palette();
    let ctx = Context::default();
    let mut ambient = Ambient::default();
    for time in [0.0, 5.0, 5.1] {
        frame(&ctx, &mut ambient, Want::Sky(Moon::Searching), time);
    }
    let screen = ctx.content_rect();
    assert_eq!(ambient.disc, Some(moon_disc(screen, Moon::Searching)));
    frame(&ctx, &mut ambient, Want::Sky(Moon::Found), 6.0);
    frame(&ctx, &mut ambient, Want::Sky(Moon::Found), 6.05);
    let (midway, r) = ambient.disc.expect("the moon");
    let (found, found_r) = moon_disc(screen, Moon::Found);
    let (searching, searching_r) = moon_disc(screen, Moon::Searching);
    assert!(midway.y < searching.y && midway.y > found.y, "{midway:?}");
    assert!(r < searching_r && r > found_r, "{r}");
    for n in 0..40 {
        frame(
            &ctx,
            &mut ambient,
            Want::Sky(Moon::Found),
            7.0 + f64::from(n) * 0.05,
        );
    }
    assert_eq!(ambient.disc, Some((found, found_r)));
}

#[test]
fn the_glow_comes_with_the_ambient_and_rests_when_it_does() {
    crate::theme::own_palette();
    let ctx = Context::default();
    let mut ambient = Ambient::default();
    let a = texture(&ctx, 10);
    frame(&ctx, &mut ambient, Want::Sky(Moon::High), 0.0);
    assert_eq!(ambient.glow(), Color32::TRANSPARENT, "no cover on the sky");
    frame(&ctx, &mut ambient, Want::Sky(Moon::High), 5.0);
    frame(&ctx, &mut ambient, art("a", &a), 6.0);
    let midway = 6.0 + f64::from(t::MOTION_AMBIENT_MS) / 2000.0;
    frame(&ctx, &mut ambient, art("a", &a), midway);
    let half = ambient.glow();
    assert!(
        half.a() > 0 && half.a() < 60,
        "it fades in with it: {half:?}"
    );
    frame(&ctx, &mut ambient, art("a", &a), 10.0);
    frame(&ctx, &mut ambient, art("a", &a), 10.1);
    assert_eq!(ambient.glow().a(), 60);
    assert!(!ctx.has_requested_repaint(), "no idle repaint");
}

#[test]
fn a_new_hero_cross_fades_and_then_asks_for_no_more_frames() {
    crate::theme::own_palette();
    let ctx = Context::default();
    let mut ambient = Ambient::default();
    let (a, b) = (texture(&ctx, 10), texture(&ctx, 20));
    frame(&ctx, &mut ambient, art("a", &a), 0.0);
    frame(&ctx, &mut ambient, art("a", &a), 1.0);
    frame(&ctx, &mut ambient, art("a", &a), 1.1);
    assert!(
        !ctx.has_requested_repaint(),
        "an ambient at rest stays at rest"
    );
    let during = frame(&ctx, &mut ambient, art("b", &b), 2.0);
    assert!(ctx.has_requested_repaint(), "the fade runs");
    let done = 2.0 + f64::from(t::MOTION_AMBIENT_MS) / 1000.0 + 0.01;
    let after = frame(&ctx, &mut ambient, art("b", &b), done);
    assert_eq!(during.len(), after.len() + 1, "the old layer is let go");
}

#[test]
fn with_reduced_motion_the_ambient_cuts() {
    crate::theme::own_palette();
    let ctx = Context::default();
    let mut style = (*ctx.global_style()).clone();
    style.animation_time = 0.0;
    ctx.set_global_style(style);
    let mut ambient = Ambient::default();
    let (a, b) = (texture(&ctx, 10), texture(&ctx, 20));
    frame(&ctx, &mut ambient, art("a", &a), 0.0);
    let cut = frame(&ctx, &mut ambient, art("b", &b), 0.1);
    assert_eq!(
        cut.len(),
        3,
        "the canvas, one layer, the page colour: {cut:?}"
    );
}

#[test]
fn a_cover_on_its_way_keeps_what_shows() {
    crate::theme::own_palette();
    let ctx = Context::default();
    let mut ambient = Ambient::default();
    let a = texture(&ctx, 10);
    frame(&ctx, &mut ambient, art("a", &a), 0.0);
    let key = ambient.key;
    frame(&ctx, &mut ambient, Want::Keep, 0.1);
    assert_eq!(ambient.key, key);
}

#[test]
fn nothing_to_keep_shows_the_sky_with_its_moon() {
    crate::theme::own_palette();
    let ctx = Context::default();
    let mut ambient = Ambient::default();
    frame(&ctx, &mut ambient, Want::Keep, 0.0);
    assert!(ambient.shows == Some(Shows::Sky(Moon::High)));
}

#[test]
fn art_and_sky_cross_fade_both_ways_and_rest_afterwards() {
    crate::theme::own_palette();
    let ctx = Context::default();
    let mut ambient = Ambient::default();
    let a = texture(&ctx, 10);
    frame(&ctx, &mut ambient, art("a", &a), 0.0);
    frame(&ctx, &mut ambient, Want::Sky(Moon::Low), 1.0);
    assert!(ctx.has_requested_repaint(), "the sky fades in");
    frame(&ctx, &mut ambient, Want::Sky(Moon::Low), 2.0);
    frame(&ctx, &mut ambient, Want::Sky(Moon::Low), 2.1);
    assert!(
        !ctx.has_requested_repaint(),
        "the sky at rest asks for nothing"
    );
    frame(&ctx, &mut ambient, art("a", &a), 3.0);
    assert!(ctx.has_requested_repaint(), "the art fades back");
}

fn veil_of(ctx: &Context, ambient: &Ambient, scrolled: f32) -> Vec<Shape> {
    let screen = ctx.content_rect();
    let brand = Rect::from_min_size(pos2(48.0, 29.0), vec2(118.0, 30.0));
    ambient.veil(ctx, screen, brand, scrolled)
}

/// The flaw it replaces: a dark band with a hard foot. This is the backdrop itself, in its own
/// place, so where nothing passes beneath it there is nothing to see.
#[test]
fn the_veil_is_the_backdrop_itself_in_its_own_place_and_nothing_at_the_top() {
    crate::theme::own_palette();
    let ctx = Context::default();
    let mut ambient = Ambient::default();
    let a = texture(&ctx, 40);
    frame(&ctx, &mut ambient, art("a", &a), 0.0);
    let shapes = frame(&ctx, &mut ambient, art("a", &a), 5.0);
    let Shape::Mesh(layer) = &shapes[1] else {
        panic!("the layer")
    };
    assert_eq!(layer.texture_id, a.id());
    let first = layer.vertices[0];
    let last = layer.vertices[layer.vertices.len() - 1];
    let uv = |p: Pos2| {
        pos2(
            egui::remap(p.x, first.pos.x..=last.pos.x, first.uv.x..=last.uv.x),
            egui::remap(p.y, first.pos.y..=last.pos.y, first.uv.y..=last.uv.y),
        )
    };
    let output = ctx.run_ui(egui::RawInput::default(), |ui| {
        assert!(veil_of(ui.ctx(), &ambient, 0.0).is_empty(), "at the top");
        let veil = veil_of(ui.ctx(), &ambient, 200.0);
        assert_eq!(veil.len(), 1, "one layer, no dark fill: {veil:?}");
        let Shape::Mesh(mesh) = &veil[0] else {
            panic!("a mesh")
        };
        assert_eq!(mesh.texture_id, a.id());
        let foot = mesh.vertices.iter().map(|v| v.pos.y).fold(0.0, f32::max);
        for v in &mesh.vertices {
            let want = uv(v.pos);
            assert!((v.uv - want).length() < 1e-3, "{v:?} wants {want:?}");
            if v.pos.y >= foot - 0.01 {
                assert_eq!(v.color.a(), 0, "no edge at its foot: {v:?}");
            }
        }
    });
    let mut delta = output.textures_delta;
    delta.clear();
}

/// The audit's flaw: a settings row and a cover read between the capsules and behind the logo.
/// Behind the chrome the band is whole, then it eases away with no step anywhere.
#[test]
fn the_band_behind_the_chrome_is_whole_and_then_eases_away() {
    for height in [600.0, 800.0, 1080.0] {
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1280.0, height));
        let brand = Rect::from_min_size(pos2(48.0, 29.0), vec2(118.0, 30.0));
        let at = |x: f32, y: f32| veil_at(screen, brand, pos2(x, y));
        let bar_bottom = super::super::chrome::BAR_TOP + super::super::chrome::BAR_H;
        for y in 0..=bar_bottom as i32 {
            for x in [0.0, 400.0, 800.0, 1279.0] {
                assert_eq!(at(x, y as f32), 1.0, "{height}: nothing shows at {x}, {y}");
            }
        }
        let mut last = at(800.0, 0.0);
        for y in 1..250 {
            let here = at(800.0, y as f32);
            assert!(here <= last && last - here < 0.035, "{height} at {y}: {last} to {here}");
            last = here;
        }
        assert_eq!(at(800.0, veil_height(screen)), 0.0);
        assert!(band_foot(screen) + 40.0 <= veil_height(screen), "{height}: an ease, not an edge");
    }
}

#[test]
fn the_veil_ends_above_where_the_moon_and_the_page_colour_start() {
    for height in [600.0, 800.0, 1080.0, 1440.0] {
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1280.0, height));
        let (center, r) = moon_disc(screen, Moon::High);
        let foot = veil_height(screen);
        assert!(foot <= center.y - r, "{height}: {foot}");
        assert!(foot <= height * FADE_FROM, "{height}: {foot}");
    }
}

#[test]
fn with_high_contrast_the_veil_is_plain_night() {
    crate::theme::own_palette();
    crate::theme::set_high_contrast(true);
    let ctx = Context::default();
    let ambient = Ambient::default();
    let output = ctx.run_ui(egui::RawInput::default(), |ui| {
        let veil = veil_of(ui.ctx(), &ambient, 200.0);
        assert_eq!(veil.len(), 1);
        let Shape::Mesh(mesh) = &veil[0] else {
            panic!("a mesh")
        };
        assert_eq!(mesh.texture_id, TextureId::default());
    });
    let mut delta = output.textures_delta;
    delta.clear();
    crate::theme::set_high_contrast(false);
}

#[test]
fn high_contrast_paints_plain_night_with_no_backdrop_to_sample() {
    crate::theme::own_palette();
    crate::theme::set_high_contrast(true);
    let ctx = Context::default();
    let mut ambient = Ambient::default();
    let shapes = frame(&ctx, &mut ambient, Want::Sky(Moon::High), 0.0);
    assert_eq!(shapes.len(), 1);
    crate::theme::set_high_contrast(false);
}
