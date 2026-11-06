//! What lies behind every page: on Home the hero's ambient, cross-fading when the hero changes; on
//! a page without art the night sky. Chrome samples the same textures in screen space.

use std::collections::HashMap;

use egui::epaint::{Mesh, Vertex, WHITE_UV};
use egui::{pos2, vec2, Color32, Context, Pos2, Rect, Shape, Stroke, TextureHandle, TextureOptions};

use super::design_tokens as t;
use super::{glass, spring};
use crate::ambient;

/// What the page wants behind it.
pub(super) enum Want {
    /// A cover's baked ambient, known by its key.
    Art { key: String, texture: TextureHandle },
    /// A game without art: its tile's gradient.
    Tile { title: String },
    /// A cover still on its way: whatever shows now stays.
    Keep,
    Sky,
}

/// The ambient shows past the window's edges, so its soft edge is never seen.
const OVERSCAN: f32 = 0.14;
/// Where the page's own colour starts to rise over the ambient, and where it nearly covers it.
const FADE_FROM: f32 = 0.30;
const FADE_NEAR: f32 = 0.58;
const FADE_NEAR_ALPHA: f32 = 0.95;
/// The moon of art-less pages, as fractions of the window, and its radius in points.
const MOON_AT: [f32; 2] = [0.91, 0.335];
const MOON_R: f32 = 80.0;
const MOON_GLOW: f32 = 6.0;
const RING_FACTORS: [f32; 4] = [1.7, 2.5, 3.3, 4.1];

#[derive(Default)]
pub(super) struct Ambient {
    current: Option<(String, TextureHandle)>,
    previous: Option<TextureHandle>,
    started: f64,
    tiles: HashMap<String, TextureHandle>,
    sky: Option<TextureHandle>,
}

impl Ambient {
    fn show(&mut self, ctx: &Context, key: String, texture: TextureHandle) {
        if self.current.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        self.previous = self.current.take().map(|(_, t)| t);
        self.current = Some((key, texture));
        self.started = ctx.input(|i| i.time);
    }

    fn tile(&mut self, ctx: &Context, title: &str) -> TextureHandle {
        self.tiles
            .entry(title.to_owned())
            .or_insert_with(|| {
                let pair = nightjar_core::tile::tile_pair(nightjar_core::tile::title_hash(title));
                let (top, bottom) = super::cards::tile_colors(pair);
                ctx.load_texture("ambient-tile", ambient::bake_tile(top, bottom), TextureOptions::LINEAR)
            })
            .clone()
    }

    /// How far the cross-fade is: 1 when done. Asks for frames only while it runs.
    fn progress(&mut self, ctx: &Context) -> f32 {
        let ms = if spring::motion(ctx) { t::MOTION_AMBIENT_MS } else { t::REDUCED_AMBIENT_MS };
        let along = if ms == 0 { 1.0 } else { ((ctx.input(|i| i.time) - self.started) as f32 * 1000.0 / ms as f32).clamp(0.0, 1.0) };
        if along < 1.0 {
            ctx.request_repaint();
        } else {
            self.previous = None;
        }
        along
    }

    /// The backdrop's shapes this frame; tells glass what to sample.
    pub(super) fn paint(&mut self, ctx: &Context, want: Want) -> Vec<Shape> {
        let screen = ctx.content_rect();
        let mut out = Vec::new();
        if crate::theme::high_contrast() {
            // Ambient and sky are off: black, and solid glass needs no backdrop.
            out.push(Shape::rect_filled(screen, 0.0, *crate::theme::NIGHT));
            glass::set_backdrop(ctx, glass::Backdrop::default());
            return out;
        }
        match want {
            Want::Art { key, texture } => self.show(ctx, key, texture),
            Want::Tile { title } => {
                let texture = self.tile(ctx, &title);
                self.show(ctx, format!("tile:{title}"), texture);
            }
            Want::Keep => {}
            Want::Sky => {
                self.current = None;
                self.previous = None;
                return self.paint_sky(ctx, screen);
            }
        }
        out.push(Shape::rect_filled(screen, 0.0, t::COLOR_CANVAS));
        let along = self.progress(ctx);
        let area = screen.expand2(screen.size() * OVERSCAN);
        let layer = |texture: &TextureHandle, alpha: f32| glass::Layer { texture: texture.id(), alpha, area };
        let base = self.previous.as_ref().map(|p| layer(p, 1.0));
        let top = self.current.as_ref().map(|(_, c)| layer(c, along));
        let full = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        for one in [base, top].into_iter().flatten() {
            let mut mesh = Mesh::with_texture(one.texture);
            mesh.add_rect_with_uv(area, full, Color32::WHITE.gamma_multiply(one.alpha));
            out.push(Shape::mesh(mesh));
        }
        glass::set_backdrop(ctx, glass::Backdrop { base, top });
        out.push(fade_over(screen));
        out
    }

    fn paint_sky(&mut self, ctx: &Context, screen: Rect) -> Vec<Shape> {
        let texture = self
            .sky
            .get_or_insert_with(|| {
                let glow = [MOON_R * MOON_GLOW / 1280.0, MOON_R * MOON_GLOW / 800.0];
                ctx.load_texture("sky", ambient::bake_sky(MOON_AT, glow), TextureOptions::LINEAR)
            })
            .clone();
        let mut mesh = Mesh::with_texture(texture.id());
        mesh.add_rect_with_uv(screen, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        let mut out = vec![Shape::mesh(mesh)];
        glass::set_backdrop(ctx, glass::Backdrop { base: Some(glass::Layer { texture: texture.id(), alpha: 1.0, area: screen }), top: None });
        let moon = pos2(screen.left() + screen.width() * MOON_AT[0], screen.top() + screen.height() * MOON_AT[1]);
        out.extend(stars(screen));
        for factor in RING_FACTORS {
            out.push(Shape::circle_stroke(moon, MOON_R * factor, Stroke::new(1.0, t::SKY_RING)));
        }
        let disc = Rect::from_center_size(moon, egui::Vec2::splat(MOON_R * 2.0));
        let lit = glass::gradient(disc, 160.0, &[(0.0, t::COLOR_MOON_HI), (1.0, t::COLOR_MOON_LO)]);
        glass::shaded(&mut out, ctx, disc, MOON_R, &[0.25, 0.5, 0.75], &lit);
        out
    }
}

/// The page's colour rising over the ambient, so shelves sit on a calm ground.
fn fade_over(screen: Rect) -> Shape {
    let mut mesh = Mesh::default();
    let row = |mesh: &mut Mesh, y: f32, color: Color32| {
        for x in [screen.left(), screen.right()] {
            mesh.vertices.push(Vertex { pos: pos2(x, y), uv: WHITE_UV, color });
        }
    };
    let at = |f: f32| screen.top() + screen.height() * f;
    row(&mut mesh, at(FADE_FROM), Color32::TRANSPARENT);
    row(&mut mesh, at(FADE_NEAR), t::TINT_FADE.gamma_multiply(FADE_NEAR_ALPHA));
    row(&mut mesh, at(1.0), t::TINT_FADE);
    for r in 0..2u32 {
        let a = r * 2;
        mesh.indices.extend([a, a + 1, a + 3, a, a + 3, a + 2]);
    }
    Shape::mesh(mesh)
}

/// A few faint stars, the same every frame.
fn stars(screen: Rect) -> Vec<Shape> {
    let mut seed = 0x2F6E_2B1Du32;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed & 0xFFFF) as f32 / 65535.0
    };
    (0..26)
        .map(|_| {
            let at: Pos2 = screen.min + vec2(next() * screen.width(), next() * screen.height() * 0.62);
            let r = 0.6 + next() * 0.7;
            let alpha = 0.2 + next() * 0.45;
            Shape::circle_filled(at, r, Color32::WHITE.gamma_multiply(alpha))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(ctx: &Context, ambient: &mut Ambient, want: Want, time: f64) -> Vec<Shape> {
        let mut shapes = Vec::new();
        let output = ctx.run_ui(egui::RawInput { time: Some(time), ..Default::default() }, |ui| shapes = ambient.paint(ui.ctx(), want));
        let mut delta = output.textures_delta;
        delta.clear();
        shapes
    }

    fn texture(ctx: &Context, value: u8) -> TextureHandle {
        ctx.load_texture("t", egui::ColorImage::filled([2, 2], Color32::from_gray(value)), TextureOptions::LINEAR)
    }

    #[test]
    fn a_new_hero_cross_fades_and_then_asks_for_no_more_frames() {
        crate::theme::own_palette();
        let ctx = Context::default();
        let mut ambient = Ambient::default();
        let (a, b) = (texture(&ctx, 10), texture(&ctx, 20));
        frame(&ctx, &mut ambient, Want::Art { key: "a".into(), texture: a.clone() }, 0.0);
        frame(&ctx, &mut ambient, Want::Art { key: "a".into(), texture: a.clone() }, 1.0);
        frame(&ctx, &mut ambient, Want::Art { key: "a".into(), texture: a.clone() }, 1.1);
        assert!(!ctx.has_requested_repaint(), "an ambient at rest stays at rest");
        frame(&ctx, &mut ambient, Want::Art { key: "b".into(), texture: b.clone() }, 2.0);
        assert!(ctx.has_requested_repaint(), "the fade runs");
        assert!(ambient.previous.is_some());
        frame(&ctx, &mut ambient, Want::Art { key: "b".into(), texture: b.clone() }, 2.0 + f64::from(t::MOTION_AMBIENT_MS) / 1000.0 + 0.01);
        assert!(ambient.previous.is_none(), "the old one is let go");
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
        frame(&ctx, &mut ambient, Want::Art { key: "a".into(), texture: a }, 0.0);
        frame(&ctx, &mut ambient, Want::Art { key: "b".into(), texture: b }, 0.1);
        assert!(ambient.previous.is_none());
    }

    #[test]
    fn a_cover_on_its_way_keeps_what_shows() {
        crate::theme::own_palette();
        let ctx = Context::default();
        let mut ambient = Ambient::default();
        let a = texture(&ctx, 10);
        frame(&ctx, &mut ambient, Want::Art { key: "a".into(), texture: a }, 0.0);
        frame(&ctx, &mut ambient, Want::Keep, 0.1);
        assert_eq!(ambient.current.as_ref().map(|(k, _)| k.as_str()), Some("a"));
    }

    #[test]
    fn high_contrast_paints_plain_night_with_no_backdrop_to_sample() {
        crate::theme::own_palette();
        crate::theme::set_high_contrast(true);
        let ctx = Context::default();
        let mut ambient = Ambient::default();
        let shapes = frame(&ctx, &mut ambient, Want::Sky, 0.0);
        assert_eq!(shapes.len(), 1);
        crate::theme::set_high_contrast(false);
    }
}
