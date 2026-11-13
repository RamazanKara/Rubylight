use super::*;
use crate::live_glass::set_tier;

fn in_frame(mut run: impl FnMut(&Context)) {
    let ctx = Context::default();
    let output = ctx.run_ui(egui::RawInput::default(), |ui| run(ui.ctx()));
    let mut delta = output.textures_delta;
    delta.clear();
}

fn rect() -> Rect {
    Rect::from_min_size(pos2(10.0, 10.0), vec2(200.0, 48.0))
}

#[test]
fn a_fill_mesh_stays_inside_its_feather_and_indexes_only_its_vertices() {
    let mut mesh = Mesh::default();
    fill_mesh(
        &mut mesh,
        rect(),
        24.0,
        &[0.48],
        2.0,
        &|_| Color32::WHITE,
        &|_| WHITE_UV,
    );
    let bounds = rect().expand(0.51);
    assert!(mesh.vertices.iter().all(|v| bounds.contains(v.pos)));
    assert!(mesh
        .indices
        .iter()
        .all(|i| (*i as usize) < mesh.vertices.len()));
    // The stop adds a row of its own at the sheen's end, so the gradient bends there.
    let y = rect().top() + 48.0 * 0.48;
    assert!(mesh.vertices.iter().any(|v| (v.pos.y - y).abs() < 0.01));
}

#[test]
fn corners_are_circular_on_the_outline() {
    let nodes = outline(rect(), 24.0, 1.0);
    for node in &nodes {
        let p = node.c + node.n * 24.0;
        // Every outline point lies on the capsule: distance to the spine is the radius.
        let spine_x = p.x.clamp(rect().left() + 24.0, rect().right() - 24.0);
        let d = (p - pos2(spine_x, rect().center().y)).length();
        assert!((d - 24.0).abs() < 0.01, "{p:?} is {d} from the spine");
    }
}

#[test]
fn a_rim_follows_a_slanted_gradient_along_a_long_edge() {
    let color = gradient(
        rect(),
        168.0,
        &[(0.0, Color32::WHITE), (1.0, Color32::BLACK)],
    );
    let mut mesh = Mesh::default();
    ring_mesh(&mut mesh, rect(), 24.0, 1.0, 2.0, &color);
    let on_top: Vec<u8> = mesh
        .vertices
        .iter()
        .filter(|v| (v.pos.y - 10.5).abs() < 0.3 && v.color.a() > 0)
        .map(|v| v.color.r())
        .collect();
    let (min, max) = (on_top.iter().min().copied(), on_top.iter().max().copied());
    assert!(
        max > min,
        "the top edge changes along its length: {on_top:?}"
    );
}

#[test]
fn a_white_backdrop_stays_dark_enough_for_text_at_every_material() {
    // The spec's worst case: white x brightness x (1 - fill alpha) + fill.
    for material in [
        Material::Chrome,
        Material::Panel,
        Material::Menu,
        Material::Chip,
    ] {
        for tier in [Tier::Live, Tier::Baked] {
            let r = recipe(material, tier);
            let through = r.brightness * (1.0 - f32::from(r.fill.a()) / 255.0)
                + f32::from(r.fill.r()) / 255.0;
            assert!(through < 0.36, "{material:?} {tier:?} shows {through}");
        }
    }
}

#[test]
fn the_hud_lets_the_game_through_but_keeps_text_readable_and_casts_nothing() {
    let r = hud_recipe();
    assert!((0xB0..0xE0).contains(&r.fill.a()), "fill alpha {}", r.fill.a());
    assert_eq!(r.shadow.a(), 0);
    assert_eq!(r.shadow_blur, 0.0);
    let through = r.brightness * (1.0 - f32::from(r.fill.a()) / 255.0)
        + f32::from(r.fill.r()) / 255.0;
    assert!(through < 0.36, "a white game shows {through}");
    in_frame(|ctx| {
        set_tier(ctx, Tier::Live);
        let mut out = Vec::new();
        hud_surface(&mut out, ctx, rect(), 12.0);
        // Fill, sheen and rim only: no backdrop callback reads the stream.
        assert!(!out.is_empty());
        assert!(out.iter().all(|s| !matches!(s, Shape::Callback(_))));
    });
}

#[test]
fn a_shadow_bake_is_soft_and_a_nine_slice_stretches_only_its_middle() {
    let image = bake_soft(vec2(74.0, 74.0), 24.0, 36.0, false);
    let center = image.pixels[image.size[1] / 2 * image.size[0] + image.size[0] / 2].a();
    let corner = image.pixels[0].a();
    assert!(center > 200, "{center}");
    assert!(corner < 8, "{corner}");
    in_frame(|ctx| {
        let mut out = Vec::new();
        soft(
            &mut out,
            ctx,
            Rect::from_min_size(pos2(50.0, 50.0), vec2(600.0, 48.0)),
            24.0,
            36.0,
            Color32::BLACK,
        );
        let Some(Shape::Mesh(mesh)) = out.first() else {
            panic!("a mesh")
        };
        assert_eq!(mesh.vertices.len(), 16);
        assert_eq!(mesh.vertices[1].uv.x, 0.5);
        assert_eq!(mesh.vertices[2].uv.x, 0.5);
    });
}

/// A side that rounds up for its bake (a capsule 67.6 high at a fractional scale) once made the
/// two halves overlap: a bright line either side of a wide focused button.
#[test]
fn a_nine_slice_never_overlaps_its_halves_on_a_fractional_side() {
    in_frame(|ctx| {
        for height in [40.0, 67.4, 67.5, 67.6, 67.75, 68.0] {
            for width in [20.3, 20.7, 432.6] {
                let mut out = Vec::new();
                let rect = Rect::from_min_size(pos2(50.25, 50.25), vec2(width, height));
                soft_around(&mut out, ctx, rect, 34.0, 26.0, t::FOCUS_HALO_COLOR);
                let Some(Shape::Mesh(mesh)) = out.first() else {
                    panic!("a mesh")
                };
                let x = |i: usize| mesh.vertices[i].pos.x;
                let y = |row: usize| mesh.vertices[row * 4].pos.y;
                assert!(
                    x(0) < x(1) && x(1) <= x(2) && x(2) < x(3),
                    "{width}x{height}: {} {} {} {}",
                    x(0),
                    x(1),
                    x(2),
                    x(3)
                );
                assert!(
                    y(0) < y(1) && y(1) <= y(2) && y(2) < y(3),
                    "{width}x{height}: {} {} {} {}",
                    y(0),
                    y(1),
                    y(2),
                    y(3)
                );
            }
        }
    });
}

/// A cover narrower than its glow's blur follows the window. A drag bakes nothing, and at rest
/// it is the very bake a window that always had this size paints.
#[test]
fn dragging_the_window_bakes_no_shadows_and_rest_is_the_exact_bake() {
    const R: f32 = 16.0;
    const BLUR: f32 = 110.0;
    let frame = |ctx: &Context, width: f32, time: f64| {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width * 4.0, 800.0))),
            time: Some(time),
            ..Default::default()
        };
        let mut mesh = None;
        let output = ctx.run_ui(input, |ui| {
            let mut out = Vec::new();
            soft(&mut out, ui.ctx(), Rect::from_min_size(pos2(100.0, 100.0), vec2(width, width * 1.5)), R, BLUR, Color32::RED);
            if let Some(Shape::Mesh(found)) = out.pop() {
                mesh = Some(found.vertices.iter().map(|v| (v.pos, v.uv)).collect::<Vec<_>>());
            }
        });
        let uploads = output.textures_delta.set.iter().filter(|(id, _)| matches!(id, TextureId::Managed(n) if *n != 0)).count();
        let mut delta = output.textures_delta;
        delta.clear();
        (uploads, mesh.expect("a mesh"))
    };
    let ctx = Context::default();
    assert_eq!(frame(&ctx, 160.0, 0.0).0, 1, "the first bake");
    let mut time = 0.0;
    for width in 161..=215 {
        time += 0.016;
        assert_eq!(frame(&ctx, width as f32, time).0, 0, "a drag through {width}");
    }
    assert_eq!(frame(&ctx, 215.0, time + 0.1).0, 0, "not at rest yet");
    let (uploads, rest) = frame(&ctx, 215.0, time + 1.0);
    assert_eq!(uploads, 1, "its own bake at rest");
    let (_, fresh) = frame(&Context::default(), 215.0, 0.0);
    assert_eq!(rest, fresh);
    assert_eq!(frame(&ctx, 215.0, time + 1.1).0, 0, "and once");
}

#[test]
fn the_soft_bake_is_reused_not_baked_each_frame() {
    let ctx = Context::default();
    let run = |ctx: &Context| {
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut out = Vec::new();
            soft(
                &mut out,
                ui.ctx(),
                Rect::from_min_size(pos2(50.0, 50.0), vec2(300.0, 300.0)),
                16.0,
                26.0,
                Color32::BLACK,
            );
        });
        let uploads = output
            .textures_delta
            .set
            .iter()
            .filter(|(id, _)| matches!(id, TextureId::Managed(n) if *n != 0))
            .count();
        let mut delta = output.textures_delta;
        delta.clear();
        uploads
    };
    assert_eq!(run(&ctx), 1);
    assert_eq!(run(&ctx), 0);
}

#[test]
fn baked_and_solid_paint_no_boundary_and_live_does() {
    let count = |tier_set: Tier| {
        let ctx = Context::default();
        set_tier(&ctx, tier_set);
        let mut found = 0;
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            boundary(ui.painter(), vec![Rect::ZERO])
        });
        for shape in &output.shapes {
            if matches!(shape.shape, Shape::Callback(_)) {
                found += 1;
            }
        }
        let mut delta = output.textures_delta;
        delta.clear();
        found
    };
    crate::theme::own_palette();
    assert_eq!(count(Tier::Baked), 0);
    assert_eq!(count(Tier::Live), 1);
}

#[test]
fn day_glass_has_its_own_recipe_and_solid_stays_the_same_in_every_look() {
    use crate::theme::{own_look, Look};
    let materials = [Material::Chrome, Material::Panel, Material::Menu, Material::Chip];
    let all = |look: Look, tier: Tier| {
        own_look(look);
        materials.map(|material| recipe(material, tier))
    };
    for tier in [Tier::Live, Tier::Baked] {
        let (night, day, oled) = (all(Look::Night, tier), all(Look::Day, tier), all(Look::Oled, tier));
        assert_eq!(oled, night, "{tier:?}: OLED black has the night glass");
        for (material, (day, night)) in materials.iter().zip(day.iter().zip(&night)) {
            assert_ne!(day, night, "{material:?} on {tier:?} has a day recipe");
            assert!(day.hairline.a() > 0, "{material:?} on {tier:?}: the day glass's edge shows on a pale page");
            assert_eq!(night.hairline, Color32::TRANSPARENT, "{material:?} on {tier:?}: no edge at night");
        }
        // The menus take the panel's glass by day.
        assert_eq!(day[2], day[1], "{tier:?}");
    }
    assert_eq!(all(Look::Day, Tier::Solid), all(Look::Night, Tier::Solid), "solid glass is high contrast's");
    crate::theme::own_palette();
}

#[test]
fn live_glass_samples_the_screen_and_baked_samples_the_ambient() {
    crate::theme::own_palette();
    let textures = |tier_set: Tier| {
        let ctx = Context::default();
        set_tier(&ctx, tier_set);
        let mut seen = Vec::new();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let ambient = TextureId::User(77);
            set_backdrop(
                ui.ctx(),
                Backdrop {
                    base: Some(Layer {
                        texture: ambient,
                        alpha: 1.0,
                        area: ui.ctx().content_rect(),
                    }),
                    top: None,
                },
            );
            let mut out = Vec::new();
            surface(&mut out, ui.ctx(), rect(), 24.0, Material::Chrome);
            for shape in &out {
                if let Shape::Mesh(mesh) = shape {
                    seen.push(mesh.texture_id);
                }
            }
        });
        let mut delta = output.textures_delta;
        delta.clear();
        seen
    };
    assert!(textures(Tier::Live).contains(&LIVE_BACKDROP));
    assert!(!textures(Tier::Baked).contains(&LIVE_BACKDROP));
    assert!(textures(Tier::Baked).contains(&TextureId::User(77)));
}

/// A stream plays under the menu: nothing it paints may read a backdrop, on any tier.
#[test]
fn the_stream_menus_glass_reads_nothing_behind_it() {
    for tier_set in [Tier::Live, Tier::Baked, Tier::Solid] {
        let ctx = Context::default();
        set_tier(&ctx, tier_set);
        let mut seen = Vec::new();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let layer = Layer { texture: TextureId::User(77), alpha: 1.0, area: ui.ctx().content_rect() };
            set_backdrop(ui.ctx(), Backdrop { base: Some(layer), top: None });
            let mut out = Vec::new();
            menu_surface(&mut out, ui.ctx(), rect(), 32.0);
            for shape in &out {
                if let Shape::Mesh(mesh) = shape {
                    seen.push(mesh.texture_id);
                }
            }
        });
        let mut delta = output.textures_delta;
        delta.clear();
        assert!(!seen.is_empty());
        assert!(!seen.contains(&LIVE_BACKDROP) && !seen.contains(&TextureId::User(77)), "{tier_set:?}: {seen:?}");
    }
}

#[test]
fn deferred_shapes_wait_for_the_flush() {
    let ctx = Context::default();
    let output = ctx.run_ui(egui::RawInput::default(), |ui| {
        defer(ui, vec![Shape::Noop]);
        flush_deferred(ui);
        let left = ui.ctx().data_mut(|d| {
            d.get_temp_mut_or_default::<Vec<(Rect, Vec<Shape>)>>(deferred_id())
                .len()
        });
        assert_eq!(left, 0);
    });
    let mut delta = output.textures_delta;
    delta.clear();
}
