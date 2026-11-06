//! What lies behind every page: a game's ambient, or on a page without art the night sky with its
//! moon. Whatever shows cross-fades to the next. Chrome samples the same textures in screen space.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use egui::epaint::{Mesh, Vertex, WHITE_UV};
use egui::{
    pos2, vec2, Color32, Context, Pos2, Rect, Shape, Stroke, TextureHandle, TextureId,
    TextureOptions, Vec2,
};

use super::design_tokens as t;
use super::glass;
use crate::ambient;

/// Where a page's moon hangs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Moon {
    /// High on the right, clear of a column of rows.
    High,
    /// Large and low on the right, behind a page of cards.
    Low,
}

impl Moon {
    fn index(self) -> usize {
        self as usize
    }

    /// Its middle as fractions of the window, and its radius at the prototype's 800 high.
    fn place(self) -> ([f32; 2], f32) {
        match self {
            Moon::High => ([0.911, 0.335], 80.0),
            Moon::Low => ([0.8125, 0.65], 118.0),
        }
    }
}

/// What the page wants behind it.
pub(super) enum Want {
    /// A cover's baked ambient, known by its key.
    Art {
        key: String,
        texture: TextureHandle,
    },
    /// A game without art: its tile's gradient.
    Tile {
        title: String,
    },
    /// A cover still on its way, or a page that belongs to the last: what shows now stays.
    Keep,
    Sky(Moon),
}

/// The ambient shows past the window's edges, so its soft edge is never seen.
const OVERSCAN: f32 = 0.14;
/// Where the page's own colour starts to rise over the ambient, and where it nearly covers it.
const FADE_FROM: f32 = 0.30;
const FADE_NEAR: f32 = 0.58;
const FADE_NEAR_ALPHA: f32 = 0.95;
/// How far the moon's glow reaches, in moon radii.
const MOON_GLOW: f32 = 6.0;
const RING_FACTORS: [f32; 4] = [1.7, 2.5, 3.3, 4.1];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shows {
    Art,
    Sky(Moon),
}

#[derive(Default)]
pub(super) struct Ambient {
    fade: glass::CrossFade,
    shows: Option<Shows>,
    /// What showed before the fade now running, for the page colour over art to fade with it.
    before: Option<Shows>,
    key: u64,
    tiles: HashMap<String, TextureHandle>,
    skies: [Option<TextureHandle>; 2],
}

fn hash_of(value: impl Hash) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn full_uv() -> Rect {
    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))
}

fn layer_mesh(texture: TextureId, area: Rect, alpha: f32) -> Shape {
    let mut mesh = Mesh::with_texture(texture);
    mesh.add_rect_with_uv(area, full_uv(), Color32::WHITE.gamma_multiply(alpha));
    Shape::mesh(mesh)
}

impl Ambient {
    fn show(&mut self, ctx: &Context, key: u64, texture: TextureHandle, shows: Shows) {
        if self.shows.is_some() && self.key == key {
            return;
        }
        self.before = self.shows;
        self.shows = Some(shows);
        self.key = key;
        self.fade.show(ctx, key, texture);
    }

    fn tile(&mut self, ctx: &Context, title: &str) -> TextureHandle {
        self.tiles
            .entry(title.to_owned())
            .or_insert_with(|| {
                let pair = nightjar_core::tile::tile_pair(nightjar_core::tile::title_hash(title));
                let (top, bottom) = super::cards::tile_colors(pair);
                ctx.load_texture(
                    "ambient-tile",
                    ambient::bake_tile(top, bottom),
                    TextureOptions::LINEAR,
                )
            })
            .clone()
    }

    /// The sky, baked for the same overscanned area as an ambient, so one fades into the other.
    fn sky(&mut self, ctx: &Context, moon: Moon) -> TextureHandle {
        self.skies[moon.index()]
            .get_or_insert_with(|| {
                let ([x, y], r) = moon.place();
                let span = 1.0 + 2.0 * OVERSCAN;
                let at = [(x + OVERSCAN) / span, (y + OVERSCAN) / span];
                let glow = [r * MOON_GLOW / 1280.0 / span, r * MOON_GLOW / 800.0 / span];
                ctx.load_texture("sky", ambient::bake_sky(at, glow), TextureOptions::LINEAR)
            })
            .clone()
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
            Want::Art { key, texture } => {
                self.show(ctx, hash_of(("art", key)), texture, Shows::Art)
            }
            Want::Tile { title } => {
                let texture = self.tile(ctx, &title);
                self.show(ctx, hash_of(("tile", title)), texture, Shows::Art);
            }
            Want::Keep => {
                // Nothing to keep yet: the sky rather than a bare canvas.
                if self.shows.is_none() {
                    let texture = self.sky(ctx, Moon::High);
                    self.show(
                        ctx,
                        hash_of(("sky", Moon::High.index())),
                        texture,
                        Shows::Sky(Moon::High),
                    );
                }
            }
            Want::Sky(moon) => {
                let texture = self.sky(ctx, moon);
                self.show(
                    ctx,
                    hash_of(("sky", moon.index())),
                    texture,
                    Shows::Sky(moon),
                );
            }
        }
        out.push(Shape::rect_filled(screen, 0.0, t::COLOR_CANVAS));
        let area = screen.expand2(screen.size() * OVERSCAN);
        let backdrop = self.fade.backdrop(ctx, area);
        let along = backdrop.top.map_or(1.0, |top| top.alpha);
        if let Some(base) = backdrop.base {
            out.push(layer_mesh(base.texture, area, 1.0));
        }
        if let Some(top) = backdrop.top {
            out.push(layer_mesh(top.texture, area, top.alpha));
        }
        // The page colour over art comes and goes with the art.
        let is_art = |shows: Option<Shows>| f32::from(u8::from(shows == Some(Shows::Art)));
        let art = if backdrop.base.is_some() {
            egui::lerp(is_art(self.before)..=is_art(self.shows), along)
        } else {
            is_art(self.shows)
        };
        if art > 0.0 {
            out.push(fade_over(screen, art));
        }
        if let Some(Shows::Sky(moon)) = self.shows {
            paint_moon(&mut out, ctx, screen, moon, along);
        }
        out
    }
}

/// The moon, its rings and a few stars, all as faint as the sky is far along.
fn paint_moon(out: &mut Vec<Shape>, ctx: &Context, screen: Rect, moon: Moon, alpha: f32) {
    let ([x, y], r) = moon.place();
    let r = r * (screen.height() / 800.0).clamp(0.75, 1.5);
    let center = pos2(
        screen.left() + screen.width() * x,
        screen.top() + screen.height() * y,
    );
    out.extend(stars(screen, alpha));
    for factor in RING_FACTORS {
        out.push(Shape::circle_stroke(
            center,
            r * factor,
            Stroke::new(1.0, t::SKY_RING.gamma_multiply(alpha)),
        ));
    }
    let disc = Rect::from_center_size(center, Vec2::splat(r * 2.0));
    let lit = glass::gradient(
        disc,
        160.0,
        &[
            (0.0, t::COLOR_MOON_HI.gamma_multiply(alpha)),
            (1.0, t::COLOR_MOON_LO.gamma_multiply(alpha)),
        ],
    );
    glass::shaded(out, ctx, disc, r, &[0.25, 0.5, 0.75], &lit);
}

/// The page's colour rising over the ambient, so shelves sit on a calm ground.
fn fade_over(screen: Rect, strength: f32) -> Shape {
    let mut mesh = Mesh::default();
    let row = |mesh: &mut Mesh, y: f32, color: Color32| {
        for x in [screen.left(), screen.right()] {
            mesh.vertices.push(Vertex {
                pos: pos2(x, y),
                uv: WHITE_UV,
                color: color.gamma_multiply(strength),
            });
        }
    };
    let at = |f: f32| screen.top() + screen.height() * f;
    row(&mut mesh, at(FADE_FROM), Color32::TRANSPARENT);
    row(
        &mut mesh,
        at(FADE_NEAR),
        t::TINT_FADE.gamma_multiply(FADE_NEAR_ALPHA),
    );
    row(&mut mesh, at(1.0), t::TINT_FADE);
    for r in 0..2u32 {
        let a = r * 2;
        mesh.indices.extend([a, a + 1, a + 3, a, a + 3, a + 2]);
    }
    Shape::mesh(mesh)
}

/// A few faint stars, the same every frame.
fn stars(screen: Rect, strength: f32) -> Vec<Shape> {
    let mut seed = 0x2F6E_2B1Du32;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed & 0xFFFF) as f32 / 65535.0
    };
    (0..26)
        .map(|_| {
            let at: Pos2 =
                screen.min + vec2(next() * screen.width(), next() * screen.height() * 0.62);
            let r = 0.6 + next() * 0.7;
            let alpha = (0.2 + next() * 0.45) * strength;
            Shape::circle_filled(at, r, Color32::WHITE.gamma_multiply(alpha))
        })
        .collect()
}

#[cfg(test)]
mod tests {
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
        }
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
}
