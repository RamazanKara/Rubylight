//! What lies behind every page: a game's ambient, or on a page without art the night sky with its
//! moon. Whatever shows cross-fades to the next. Chrome samples the same textures in screen space.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use egui::epaint::{Mesh, Vertex, WHITE_UV};
use egui::{
    pos2, vec2, Color32, Context, Pos2, Rect, Shape, Stroke, TextureHandle, TextureId,
    TextureOptions, Vec2,
};

use super::design_tokens as t;
use super::{glass, spring};
use crate::ambient;

/// Where a page's moon hangs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Moon {
    /// High on the right, clear of a column of rows.
    High,
    /// Large and low on the right, behind a page of cards.
    Low,
    /// The first run with nothing found: the page's middle, large, the bird on it.
    Searching,
    /// The first run with a PC found: the moon steps up and in for its card.
    Found,
}

impl Moon {
    fn index(self) -> usize {
        self as usize
    }

    /// The first run's moon, which carries the bird and moves between its places.
    fn first_run(self) -> bool {
        matches!(self, Moon::Searching | Moon::Found)
    }

    /// Its middle as fractions of the window, and its radius at the prototype's 800 high.
    fn place(self) -> ([f32; 2], f32) {
        match self {
            Moon::High => ([0.911, 0.335], 80.0),
            Moon::Low => ([0.8125, 0.65], 118.0),
            Moon::Searching => ([0.5, 0.395], 104.0),
            Moon::Found => ([0.5, 0.245], 72.0),
        }
    }
}

/// What the page wants behind it.
pub(super) enum Want {
    /// A cover's baked ambient, known by its key.
    Art {
        key: String,
        texture: TextureHandle,
        /// What its cover glows with, taken from the same bake.
        glow: Color32,
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
const RING_FACTORS: [f32; 4] = [ORBIT, 2.5, 3.3, 4.1];
/// The first ring, in moon radii: where the orbit dot runs while discovery does.
pub(super) const ORBIT: f32 = 1.7;
const ORBIT_SECONDS: f64 = 11.0;
/// Where the dot rests with reduced motion, as in the design: just below the right.
const ORBIT_REST: f32 = 0.2;
const ORBIT_DOT_R: f32 = 5.0;
const ORBIT_FRAME_SECONDS: f32 = 1.0 / 60.0;
const BIRD_SIZE: usize = 512;
/// How far a placed moon keeps from what the page holds, in its radii, and at least, in points: open
/// sky, not a gap between rows.
const SKY_GAP: f32 = 0.45;
const SKY_GAP_LEAST: f32 = 24.0;
/// Where the moon's middle may start, across the window: it hangs on the right.
const SKY_FROM: f32 = 0.62;
/// The smallest a moon gets for open sky, of its own size, before it steps back, and the sizes it
/// tries on the way.
const SKY_SMALLEST: f32 = 0.6;
const SKY_SIZES: usize = 5;
/// A placed moon's glow is baked in steps of the window this fine, and this many bakes are kept.
const SKY_BAKE_STEPS: f32 = 40.0;
const SKY_BAKES_KEPT: usize = 24;
/// How many places, by page and window, are kept for the moon.
const PLACEMENTS_KEPT: usize = 64;
/// A moon's place is kept by page, window and moon.
type PlaceKey = (u64, [u32; 4], usize);
const MOVE_TUNE: spring::Tune = spring::Tune {
    stiffness: t::MOTION_SHEET_STIFFNESS,
    ratio: t::MOTION_SHEET_RATIO,
};
/// The veil where the page passes beneath the chrome: how far it reaches down the window, how much
/// of the backdrop it brings at the top and behind the brand, how far past the brand its pool goes
/// and how far the page has scrolled when it is whole.
const VEIL_FRACTION: f32 = 0.19;
const VEIL_LEAST: f32 = 88.0;
const VEIL_MOST: f32 = 190.0;
const VEIL_TOP_ALPHA: f32 = 0.82;
const VEIL_BRAND_ALPHA: f32 = 0.96;
const VEIL_BRAND_REACH: f32 = 54.0;
const VEIL_WHOLE_AT: f32 = 40.0;
const VEIL_ROWS: u32 = 24;
const VEIL_COLUMNS: u32 = 24;

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
    /// When the fade now running was asked for: a second change in that frame changes its mind, it
    /// does not fade from what never showed.
    shown_at: Option<f64>,
    /// The page about to be drawn: the moon is placed against what it holds.
    page: u64,
    /// The sky whose moon waits for this frame's page to say what it holds (see [`Self::finish`]).
    pending: Option<Moon>,
    /// The first run's moon, flat, in high contrast.
    flat: Option<Moon>,
    tiles: HashMap<String, (TextureHandle, Color32)>,
    /// Skies baked with their moon's glow where it stood, by moon and place.
    skies: HashMap<u64, TextureHandle>,
    /// Where each page's moon stands in each window, None where it stepped back.
    placements: HashMap<PlaceKey, Option<(Pos2, f32)>>,
    /// The sky's moon this frame, None where it stepped back, and the page and window it is for.
    sky_disc: Option<(Pos2, f32)>,
    sky_at: Option<(u64, Rect)>,
    /// Until which frame the page and window just come into settle: a page's first frames may lay
    /// out a little differently, and a window being dragged changes every frame. Its moon moves then.
    settle_until: u64,
    /// The placed moon, fading in or standing, and those fading out where they stood.
    moon_now: Option<MoonFade>,
    moons_leaving: Vec<MoonFade>,
    bird: Option<TextureHandle>,
    /// Whether discovery runs behind the first run's moon, for its orbit dot.
    orbit: bool,
    /// The moon where it is this frame, for the veil.
    disc: Option<(Pos2, f32)>,
    /// The cover's glow this frame, the one it fades to with what shows, and the one it fades from.
    glow: Color32,
    glow_to: Color32,
    glow_before: Color32,
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
    fn show(
        &mut self,
        ctx: &Context,
        key: u64,
        texture: TextureHandle,
        shows: Shows,
        glow: Color32,
    ) {
        if self.shows.is_some() && self.key == key {
            return;
        }
        let now = ctx.input(|i| i.time);
        if self.shown_at != Some(now) {
            self.before = self.shows;
            self.glow_before = self.glow;
            self.shown_at = Some(now);
        }
        self.shows = Some(shows);
        self.key = key;
        self.glow_to = glow;
        self.fade.show(ctx, key, texture);
    }

    /// Whether a game's ambient shows, for a test.
    #[cfg(test)]
    pub(super) fn shows_art(&self) -> bool {
        self.shows == Some(Shows::Art)
    }

    /// The cover's glow this frame, fading as the ambient does. Nothing on the sky and in high contrast.
    pub(super) fn glow(&self) -> Color32 {
        self.glow
    }

    fn tile(&mut self, ctx: &Context, title: &str) -> (TextureHandle, Color32) {
        self.tiles
            .entry(title.to_owned())
            .or_insert_with(|| {
                let pair = nightjar_core::tile::tile_pair(nightjar_core::tile::title_hash(title));
                let (top, bottom) = super::cards::tile_colors(pair);
                let baked = ambient::bake_tile(top, bottom);
                let glow = ambient::glow_of(&baked);
                (
                    ctx.load_texture("ambient-tile", baked, TextureOptions::LINEAR),
                    glow,
                )
            })
            .clone()
    }

    /// The sky, baked for the same overscanned area as an ambient, so one fades into the other, with
    /// the glow around where its moon stands, as much smaller as the moon is. Where the moon stepped
    /// back the glow stays at its own place. Also the key it shows by.
    fn sky(
        &mut self,
        ctx: &Context,
        screen: Rect,
        moon: Moon,
        disc: Option<(Pos2, f32)>,
    ) -> (u64, TextureHandle) {
        let ([x, y], r) = moon.place();
        let (at, scale) = match disc {
            Some((center, placed_r)) if !moon.first_run() => {
                let own = moon_disc(screen, moon).1;
                let at = (center - screen.min) / screen.size();
                // Whole steps of the window, so a window being dragged bakes a few, not one a frame.
                let step = |v: f32| (v * SKY_BAKE_STEPS).round() / SKY_BAKE_STEPS;
                ([step(at.x), step(at.y)], step(placed_r / own))
            }
            _ => ([x, y], 1.0),
        };
        let key = hash_of(("sky", moon.index(), at.map(f32::to_bits), scale.to_bits()));
        if self.skies.len() > SKY_BAKES_KEPT {
            self.skies.clear();
        }
        let texture = self
            .skies
            .entry(key)
            .or_insert_with(|| {
                let span = 1.0 + 2.0 * OVERSCAN;
                let centre = [(at[0] + OVERSCAN) / span, (at[1] + OVERSCAN) / span];
                let r = r * scale;
                let glow = [r * MOON_GLOW / 1280.0 / span, r * MOON_GLOW / 800.0 / span];
                ctx.load_texture(
                    "sky",
                    ambient::bake_sky(centre, glow),
                    TextureOptions::LINEAR,
                )
            })
            .clone();
        (key, texture)
    }

    /// Names the page about to be drawn (see [`hold_page`]).
    pub(super) fn set_page(&mut self, ctx: &Context, page: impl Hash) {
        self.page = hash_of(page);
        ctx.data_mut(|d| d.insert_temp(page_id(), self.page));
    }

    /// The sky with its moon. The first run's moon stands at its place; a page's moon at the place it
    /// took on this page in this window, or, the first time, wherever [`Self::finish`] finds open sky
    /// once the page has said what it holds. Until then what shows stays and no moon shows.
    fn show_sky(&mut self, ctx: &Context, screen: Rect, moon: Moon) {
        if moon.first_run() {
            let disc = Some(moon_disc(screen, moon));
            let (key, texture) = self.sky(ctx, screen, moon, disc);
            self.show(ctx, key, texture, Shows::Sky(moon), Color32::TRANSPARENT);
            self.sky_disc = disc;
            self.sky_at = Some((self.page, screen));
            return;
        }
        self.pending = Some(moon);
        if let Some(&disc) = self.placements.get(&self.placement_key(screen, moon)) {
            self.show_placed(ctx, screen, moon, disc);
        }
    }

    fn placement_key(&self, screen: Rect, moon: Moon) -> PlaceKey {
        let bits = [screen.min.x, screen.min.y, screen.max.x, screen.max.y].map(f32::to_bits);
        (self.page, bits, moon.index())
    }

    /// Shows the sky for a placed moon. On the page it stood on, in a window that changed size, the
    /// moon moves (see [`Self::fade_moons`]). A sky still fading in on its page takes the new glow and
    /// goes on fading.
    fn show_placed(&mut self, ctx: &Context, screen: Rect, moon: Moon, disc: Option<(Pos2, f32)>) {
        let (key, texture) = self.sky(ctx, screen, moon, disc);
        let on_page = self.shows == Some(Shows::Sky(moon))
            && self.sky_at.is_some_and(|(page, _)| page == self.page);
        if self.sky_at != Some((self.page, screen)) {
            self.settle_until = ctx.cumulative_pass_nr() + 2;
        }
        if on_page && self.key != key && self.fade.running(ctx) {
            self.key = key;
            self.fade.swap(key, texture);
        } else {
            self.show(ctx, key, texture, Shows::Sky(moon), Color32::TRANSPARENT);
        }
        self.sky_disc = disc;
        self.sky_at = Some((self.page, screen));
    }

    /// The placed moons to paint this frame, with their alphas. The one that stands fades in as the
    /// ambient does, from when it came; one that leaves fades out from where it was, so neither ever
    /// shows or goes at once. While its page and window settle it moves as it is.
    fn fade_moons(&mut self, ctx: &Context, placed: Option<(Pos2, f32)>) -> Vec<(Pos2, f32, f32)> {
        let now = ctx.input(|i| i.time);
        let ms = glass::ambient_ms(ctx);
        if self.moon_now.map(|m| m.disc) != placed {
            match (self.moon_now.as_mut(), placed) {
                (Some(moon), Some(disc))
                    if moon.page == self.page && ctx.cumulative_pass_nr() <= self.settle_until =>
                {
                    moon.disc = disc
                }
                _ => {
                    if let Some(moon) = self.moon_now.take() {
                        let from = moon.alpha(now, ms);
                        self.moons_leaving.push(MoonFade {
                            from,
                            to: 0.0,
                            at: now,
                            ..moon
                        });
                    }
                    // One still fading out there comes back from where it is.
                    self.moon_now = placed.map(|disc| {
                        let back = self.moons_leaving.iter().position(|m| m.disc == disc);
                        let from =
                            back.map_or(0.0, |i| self.moons_leaving.remove(i).alpha(now, ms));
                        MoonFade {
                            disc,
                            page: self.page,
                            from,
                            to: 1.0,
                            at: now,
                        }
                    });
                }
            }
        }
        self.moons_leaving.retain(|m| m.alpha(now, ms) > 0.0);
        let moons: Vec<(Pos2, f32, f32)> = self
            .moons_leaving
            .iter()
            .chain(&self.moon_now)
            .map(|m| (m.disc.0, m.disc.1, m.alpha(now, ms)))
            .collect();
        if !self.moons_leaving.is_empty() || self.moon_now.is_some_and(|m| m.alpha(now, ms) < 1.0) {
            ctx.request_repaint();
        }
        moons
    }

    /// Places the waiting moon against what the page held this frame: where it stood, while the page
    /// at its top leaves that clear (a card that lifts or a focus ring stays within the gap, so
    /// neither moves it), else [`find_sky`]. A page first seen scrolled could not show what scrolled
    /// away, so the first place keeps out of that part. A page drawn some other way holds nothing.
    fn place_pending(&mut self, ctx: &Context, screen: Rect) {
        let Some(moon) = self.pending else {
            return;
        };
        let key = self.placement_key(screen, moon);
        let pass = ctx.cumulative_pass_nr();
        let held = ctx
            .data(|d| d.get_temp::<Held>(held_id()))
            .filter(|h| h.page == self.page && h.screen == screen && h.pass == pass);
        let disc = match (self.placements.get(&key).copied(), &held) {
            (Some(Some(disc)), Some(held))
                if held.at_top() && !clear_of(&held.rects, disc, 0.0) =>
            {
                find_sky(screen, moon, &held.rects)
            }
            (Some(disc), _) => disc,
            (None, Some(held)) => find_sky(screen, moon, &held.with_unseen()),
            // Kept only once the page has said what it holds.
            (None, None) => {
                let disc = find_sky(screen, moon, &[]);
                self.show_placed(ctx, screen, moon, disc);
                return;
            }
        };
        if self.placements.len() >= PLACEMENTS_KEPT && !self.placements.contains_key(&key) {
            self.placements.clear();
        }
        self.placements.insert(key, disc);
        self.show_placed(ctx, screen, moon, disc);
    }

    /// Sets whether the orbit dot runs: the first run's moon, with discovery going. It alone asks for
    /// frames while the page rests.
    pub(super) fn set_orbit(&mut self, on: bool) {
        self.orbit = on;
    }

    /// The bird to lay over the first run's moon, and the moon's disc there, carried by a spring
    /// between its places. Elsewhere it stands where the page's moon stands.
    fn disc_now(&mut self, ctx: &Context, screen: Rect, moon: Moon) -> (Pos2, f32) {
        let (center, r) = moon_disc(screen, moon);
        if !moon.first_run() {
            return (center, r);
        }
        let y = spring::animate(
            ctx,
            egui::Id::unique("first-run-moon-y"),
            center.y,
            MOVE_TUNE,
        );
        let r = spring::animate(ctx, egui::Id::unique("first-run-moon-r"), r, MOVE_TUNE);
        (pos2(center.x, y), r)
    }

    fn paint_bird(
        &mut self,
        out: &mut Vec<Shape>,
        ctx: &Context,
        center: Pos2,
        r: f32,
        alpha: f32,
    ) {
        let bird = self
            .bird
            .get_or_insert_with(|| {
                ctx.load_texture(
                    "moon-bird",
                    crate::theme::bird_mask(BIRD_SIZE),
                    TextureOptions::LINEAR,
                )
            })
            .id();
        let mut mesh = Mesh::with_texture(bird);
        mesh.add_rect_with_uv(
            Rect::from_center_size(center, Vec2::splat(r * 2.0)),
            full_uv(),
            t::COLOR_NIGHT.gamma_multiply(alpha),
        );
        out.push(Shape::mesh(mesh));
    }

    /// The dot on the first ring, while discovery runs; it holds still with reduced motion.
    fn paint_orbit(&self, out: &mut Vec<Shape>, ctx: &Context, center: Pos2, r: f32, alpha: f32) {
        if !self.orbit {
            return;
        }
        let moving = spring::motion(ctx);
        let turn = if moving {
            let along = ctx.input(|i| i.time) / ORBIT_SECONDS;
            along.fract() as f32 * std::f32::consts::TAU + ORBIT_REST
        } else {
            ORBIT_REST
        };
        let at = center + vec2(turn.cos(), turn.sin()) * r * ORBIT;
        let moon = t::COLOR_MOON_HI.gamma_multiply(alpha);
        if !crate::theme::high_contrast() {
            glass::soft(
                out,
                ctx,
                Rect::from_center_size(at, Vec2::splat(ORBIT_DOT_R * 2.0)),
                ORBIT_DOT_R,
                14.0,
                moon.gamma_multiply(0.75),
            );
        }
        out.push(Shape::circle_filled(at, ORBIT_DOT_R, moon));
        if moving {
            ctx.request_repaint_after(std::time::Duration::from_secs_f32(ORBIT_FRAME_SECONDS));
        }
    }

    /// The backdrop's shapes this frame, all at once; tells glass what to sample. For a page that
    /// holds nothing (see [`Self::prepare`] for one that does).
    pub(super) fn paint(&mut self, ctx: &Context, want: Want) -> Vec<Shape> {
        self.prepare(ctx, want);
        self.finish(ctx)
    }

    /// Takes what the page wants behind it, before the page is drawn: tells glass what to sample, the
    /// page whether to say what it holds and its cards where the moon is. [`Self::finish`] paints it
    /// once the page is drawn, under it.
    pub(super) fn prepare(&mut self, ctx: &Context, want: Want) {
        let screen = ctx.content_rect();
        self.pending = None;
        self.flat = None;
        if crate::theme::high_contrast() {
            // Ambient and sky are off: black, and solid glass needs no backdrop.
            glass::set_backdrop(ctx, glass::Backdrop::default());
            self.glow = Color32::TRANSPARENT;
            self.disc = None;
            self.moon_now = None;
            self.moons_leaving.clear();
            // The first run keeps its moon, flat, with one ring: it is all that page has.
            if let Want::Sky(moon) = want {
                self.flat = moon.first_run().then_some(moon);
            }
            publish(ctx, false, None);
            return;
        }
        match want {
            Want::Art { key, texture, glow } => {
                self.show(ctx, hash_of(("art", key)), texture, Shows::Art, glow)
            }
            Want::Tile { title } => {
                let (texture, glow) = self.tile(ctx, &title);
                self.show(ctx, hash_of(("tile", title)), texture, Shows::Art, glow);
            }
            // Nothing to keep yet: the sky rather than a bare canvas. A sky that is kept still finds
            // this page's open sky for its moon.
            Want::Keep => match self.shows {
                None => self.show_sky(ctx, screen, Moon::High),
                Some(Shows::Sky(moon)) => self.show_sky(ctx, screen, moon),
                Some(Shows::Art) => {}
            },
            Want::Sky(moon) => self.show_sky(ctx, screen, moon),
        }
        // A moon still to place has none yet for the cards.
        let disc = match self.shows {
            Some(Shows::Sky(moon)) if moon.first_run() => self.sky_disc,
            Some(Shows::Sky(_)) if self.sky_at == Some((self.page, screen)) => self.sky_disc,
            _ => None,
        };
        publish(ctx, self.pending.is_some(), disc);
        self.layers(ctx, screen);
    }

    /// This frame's layers, handed to glass, with the cover's glow along them.
    fn layers(&mut self, ctx: &Context, screen: Rect) -> glass::Backdrop {
        let area = screen.expand2(screen.size() * OVERSCAN);
        let backdrop = self.fade.backdrop(ctx, area);
        let along = backdrop.top.map_or(1.0, |top| top.alpha);
        self.glow = if backdrop.base.is_some() {
            self.glow_before.lerp_to_gamma(self.glow_to, along)
        } else {
            self.glow_to
        };
        backdrop
    }

    /// The backdrop's shapes, once the page [`Self::prepare`] was for is drawn: a moon still to place
    /// stands clear of what the page holds, in this same frame.
    pub(super) fn finish(&mut self, ctx: &Context) -> Vec<Shape> {
        let screen = ctx.content_rect();
        let mut out = Vec::new();
        if crate::theme::high_contrast() {
            out.push(Shape::rect_filled(screen, 0.0, *crate::theme::NIGHT));
            if let Some(moon) = self.flat {
                let (center, r) = self.disc_now(ctx, screen, moon);
                out.push(Shape::circle_stroke(
                    center,
                    r * ORBIT,
                    Stroke::new(1.0, t::HC_COLOR_RIM),
                ));
                out.push(Shape::circle_filled(center, r, t::COLOR_MOON_HI));
                self.paint_bird(&mut out, ctx, center, r, 1.0);
                self.paint_orbit(&mut out, ctx, center, r, 1.0);
            }
            return out;
        }
        self.place_pending(ctx, screen);
        out.push(Shape::rect_filled(screen, 0.0, t::COLOR_CANVAS));
        let backdrop = self.layers(ctx, screen);
        let along = backdrop.top.map_or(1.0, |top| top.alpha);
        let fading = backdrop.base.is_some();
        if let Some(base) = backdrop.base {
            out.push(layer_mesh(base.texture, base.area, 1.0));
        }
        if let Some(top) = backdrop.top {
            out.push(layer_mesh(top.texture, top.area, top.alpha));
        }
        // The page colour over art comes and goes with the art.
        let is_art = |shows: Option<Shows>| f32::from(u8::from(shows == Some(Shows::Art)));
        let art = if fading {
            egui::lerp(is_art(self.before)..=is_art(self.shows), along)
        } else {
            is_art(self.shows)
        };
        if art > 0.0 {
            out.push(fade_over(screen, art));
        }
        // The stars stay from sky to sky and come and go with the sky from and to art.
        let sky = |shows: Option<Shows>| matches!(shows, Some(Shows::Sky(_)));
        let stars_alpha = match (fading && sky(self.before), sky(self.shows)) {
            (true, true) => 1.0,
            (false, true) => along,
            (true, false) => 1.0 - along,
            (false, false) => 0.0,
        };
        if stars_alpha > 0.0 {
            out.extend(stars(screen, stars_alpha));
        }
        #[cfg(test)]
        let mut moons = Vec::new();
        self.disc = None;
        let placed = match self.shows {
            Some(Shows::Sky(moon)) if !moon.first_run() => self.sky_disc,
            _ => None,
        };
        // The placed moons: the one that stands, fading in, and those fading out where they stood.
        for (center, r, alpha) in self.fade_moons(ctx, placed) {
            paint_moon(&mut out, ctx, center, r, alpha);
            #[cfg(test)]
            moons.push(((center, r), alpha));
        }
        if let Some(Shows::Sky(moon)) = self.shows {
            if moon.first_run() {
                // Between the first run's two places it is one moon that moves, not two that fade.
                let same = matches!(self.before, Some(Shows::Sky(before)) if before.first_run());
                let alpha = if same { 1.0 } else { along };
                let (center, r) = self.disc_now(ctx, screen, moon);
                paint_moon(&mut out, ctx, center, r, alpha);
                #[cfg(test)]
                moons.push(((center, r), alpha));
                self.paint_bird(&mut out, ctx, center, r, alpha);
                self.paint_orbit(&mut out, ctx, center, r, alpha);
                self.disc = Some((center, r));
            } else {
                // Where it stepped back the stars stay.
                self.disc = placed;
            }
        }
        publish(ctx, self.pending.is_some(), self.disc);
        #[cfg(test)]
        ctx.data_mut(|d| {
            d.insert_temp(
                painted_id(),
                Painted {
                    moons,
                    along,
                    fading,
                },
            )
        });
        out
    }
}

/// A placed moon fading from one alpha to another, from a time on.
#[derive(Clone, Copy)]
struct MoonFade {
    disc: (Pos2, f32),
    /// The page it stands for.
    page: u64,
    from: f32,
    to: f32,
    at: f64,
}

impl MoonFade {
    fn alpha(&self, now: f64, ms: u32) -> f32 {
        if ms == 0 {
            return self.to;
        }
        let along = ((now - self.at) as f32 * 1000.0 / ms as f32).clamp(0.0, 1.0);
        egui::lerp(self.from..=self.to, along)
    }
}

/// Tells the page whether to say what it holds, and its cards where the moon is.
fn publish(ctx: &Context, placing: bool, disc: Option<(Pos2, f32)>) {
    ctx.data_mut(|d| {
        d.insert_temp(placing_id(), placing);
        d.insert_temp(disc_id(), disc);
    });
}

/// What the last frame painted of the sky, for a test to follow a fade.
#[cfg(test)]
#[derive(Clone, Default, Debug)]
pub(super) struct Painted {
    /// Each moon painted, with its alpha.
    pub moons: Vec<((Pos2, f32), f32)>,
    /// How far the backdrop's fade is, and whether one runs.
    pub along: f32,
    pub fading: bool,
}

#[cfg(test)]
fn painted_id() -> egui::Id {
    egui::Id::unique("moon-painted")
}

#[cfg(test)]
pub(super) fn painted(ctx: &Context) -> Painted {
    ctx.data(|d| d.get_temp(painted_id())).unwrap_or_default()
}

/// What a page laid out this frame, as at its top, in which window: where the moon must not stand.
#[derive(Clone)]
pub(super) struct Held {
    page: u64,
    screen: Rect,
    rects: Arc<[Rect]>,
    /// How far the page was scrolled: what lay above that was not drawn.
    above: f32,
    pass: u64,
}

impl Held {
    fn at_top(&self) -> bool {
        self.above < 0.5
    }

    /// What it holds, and where it was scrolled, all it could not show: the whole width above.
    fn with_unseen(&self) -> Vec<Rect> {
        let mut rects = self.rects.to_vec();
        if !self.at_top() {
            rects.push(Rect::from_min_max(
                self.screen.min,
                pos2(self.screen.right(), self.screen.top() + self.above),
            ));
        }
        rects
    }
}

fn held_id() -> egui::Id {
    egui::Id::unique("moon-held")
}

fn page_id() -> egui::Id {
    egui::Id::unique("moon-page")
}

fn placing_id() -> egui::Id {
    egui::Id::unique("moon-placing")
}

fn disc_id() -> egui::Id {
    egui::Id::unique("moon-disc")
}

/// The sky's moon this frame, for a card that passes over it; None where there is none.
pub(super) fn disc_of(ctx: &Context) -> Option<(Pos2, f32)> {
    ctx.data(|d| d.get_temp(disc_id())).flatten()
}

/// Whether the backdrop places a moon this frame and needs what the page holds.
pub(super) fn placing(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp(placing_id())).unwrap_or(false)
}

/// Where the page's shapes lie as laid out, moved by `down` to the page's top: cards, chips, rows,
/// words and covers. The blurred shadows and halos that hover and focus bring are left out, and so
/// is the reach of a blur.
pub(super) fn laid_out<'a>(
    soft: &glass::SoftTextures,
    shapes: impl Iterator<Item = &'a Shape>,
    down: Vec2,
    screen: Rect,
) -> Vec<Rect> {
    fn walk(shape: &Shape, soft: &glass::SoftTextures, out: &mut Vec<Rect>) {
        let rect = match shape {
            Shape::Noop => return,
            Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, soft, out);
                }
                return;
            }
            Shape::Mesh(mesh) if soft.contains(&mesh.texture_id) => return,
            Shape::Mesh(mesh) => mesh.calc_bounds(),
            Shape::Rect(rect) => rect.rect,
            shape => shape.visual_bounding_rect(),
        };
        out.push(rect);
    }
    let mut out = Vec::new();
    for shape in shapes {
        walk(shape, soft, &mut out);
    }
    out.retain_mut(|rect| {
        *rect = rect.translate(down);
        rect.is_positive() && rect.intersects(screen)
    });
    out
}

/// Keeps what the page named by [`Ambient::set_page`] laid out this frame, `rects` as at its top in
/// `screen`, scrolled `above` down, for [`Ambient::finish`] to place the moon against.
pub(super) fn hold_page(ctx: &Context, screen: Rect, rects: Vec<Rect>, above: f32) {
    let page = ctx.data(|d| d.get_temp(page_id())).unwrap_or(0);
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        d.insert_temp(
            held_id(),
            Held {
                page,
                screen,
                rects: rects.into(),
                above,
                pass,
            },
        )
    });
}

/// Whether a disc stands at least `gap` clear of every rect.
fn clear_of(rects: &[Rect], (center, r): (Pos2, f32), gap: f32) -> bool {
    rects
        .iter()
        .all(|rect| rect.expand(gap).distance_to_pos(center) >= r)
}

/// Where a page's moon stands in open sky, never under a card, chip or row and never cut by one:
/// its own place where that is clear of what the page holds by a gap, else the nearest clear place
/// on the window's right below the chrome, smaller if it must be, down to [`SKY_SMALLEST`]. None
/// where the page leaves no such sky: the moon steps back. The first run's moon keeps its place.
pub(super) fn find_sky(screen: Rect, moon: Moon, held: &[Rect]) -> Option<(Pos2, f32)> {
    let (want, own) = moon_disc(screen, moon);
    if moon.first_run() {
        return Some((want, own));
    }
    for size in 0..SKY_SIZES {
        let r = own * egui::lerp(1.0..=SKY_SMALLEST, size as f32 / (SKY_SIZES - 1) as f32);
        let gap = (r * SKY_GAP).max(SKY_GAP_LEAST);
        let room = Rect::from_min_max(
            pos2(
                screen.left() + screen.width() * SKY_FROM,
                screen.top() + super::chrome::PAGE_TOP + r + gap,
            ),
            pos2(screen.right() - r - gap, screen.bottom() - r - gap),
        );
        if room.min.x > room.max.x || room.min.y > room.max.y {
            continue;
        }
        // Only what a disc in the room can come near.
        let reach = room.expand(r + gap);
        let held: Vec<Rect> = held
            .iter()
            .copied()
            .filter(|h| h.intersects(reach))
            .collect();
        let near = room.clamp(want);
        if clear_of(&held, (near, r), gap) {
            return Some((near, r));
        }
        let step = (r / 4.0).max(4.0);
        let (columns, rows) = (
            (room.width() / step).ceil() as u32,
            (room.height() / step).ceil() as u32,
        );
        let mut best: Option<(f32, Pos2)> = None;
        for column in 0..=columns {
            for row in 0..=rows {
                let at = pos2(
                    (room.left() + column as f32 * step).min(room.right()),
                    (room.top() + row as f32 * step).min(room.bottom()),
                );
                let away = at.distance_sq(want);
                if best.is_none_or(|(least, _)| away < least) && clear_of(&held, (at, r), gap) {
                    best = Some((away, at));
                }
            }
        }
        if let Some((_, at)) = best {
            return Some((at, r));
        }
    }
    None
}

fn smoothstep(from: f32, to: f32, x: f32) -> f32 {
    let t = ((x - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// How much of the backdrop lies over the page at `p`, with the page fully scrolled: a long ease from
/// the window's top, and a soft pool behind the brand, which has no glass of its own.
fn veil_at(screen: Rect, brand: Rect, p: Pos2) -> f32 {
    let foot = screen.top() + veil_height(screen);
    let ramp = VEIL_TOP_ALPHA * (1.0 - smoothstep(screen.top(), foot, p.y));
    let reach = brand.size() / 2.0 + vec2(VEIL_BRAND_REACH, VEIL_BRAND_REACH * 0.7);
    let away = (p - brand.center()) / reach;
    let pool = VEIL_BRAND_ALPHA * (1.0 - smoothstep(0.5, 1.0, away.length()));
    ramp.max(pool)
}

/// The ramp ends above where the moon can hang and where the page colour starts to rise.
fn veil_height(screen: Rect) -> f32 {
    (screen.height() * VEIL_FRACTION).clamp(VEIL_LEAST, VEIL_MOST)
}

impl Ambient {
    /// Where the page passes beneath the chrome it dissolves into the backdrop: the same textures in
    /// the same place, laid over the page, so with nothing passing there it cannot be seen at all and
    /// over a bright cover it is a long soft ramp, never a bar. `brand` keeps its words readable.
    /// None with the page at its start.
    pub(super) fn veil(
        &self,
        ctx: &Context,
        screen: Rect,
        brand: Rect,
        scrolled: f32,
    ) -> Vec<Shape> {
        let strength = (scrolled / VEIL_WHOLE_AT).min(1.0);
        if strength <= 0.0 {
            return Vec::new();
        }
        let alpha = |p: Pos2| veil_at(screen, brand, p) * strength;
        let foot = screen.top() + veil_height(screen).max(brand.bottom() + VEIL_BRAND_REACH);
        // Rows close enough for the ease; columns only where the pool bends, the ramp is level.
        let rows: Vec<f32> = (0..=VEIL_ROWS)
            .map(|i| egui::lerp(screen.top()..=foot, i as f32 / VEIL_ROWS as f32))
            .collect();
        let mut columns: Vec<f32> = (0..=VEIL_COLUMNS)
            .map(|i| {
                let end = brand.right() + 2.0 * VEIL_BRAND_REACH;
                egui::lerp(screen.left()..=end, i as f32 / VEIL_COLUMNS as f32)
            })
            .collect();
        columns.push(screen.right());
        let grid = |texture: TextureId, area: Option<Rect>, layer: f32, plain: Color32| {
            let mut mesh = Mesh::with_texture(texture);
            for y in &rows {
                for x in &columns {
                    let pos = pos2(*x, *y);
                    let uv = area.map_or(WHITE_UV, |a| {
                        pos2(
                            (pos.x - a.min.x) / a.width(),
                            (pos.y - a.min.y) / a.height(),
                        )
                    });
                    mesh.vertices.push(Vertex {
                        pos,
                        uv,
                        color: plain.gamma_multiply(alpha(pos) * layer),
                    });
                }
            }
            let across = columns.len() as u32;
            for row in 0..rows.len() as u32 - 1 {
                for column in 0..across - 1 {
                    let a = row * across + column;
                    mesh.indices
                        .extend([a, a + 1, a + across + 1, a, a + across + 1, a + across]);
                }
            }
            Shape::mesh(mesh)
        };
        if crate::theme::high_contrast() {
            return vec![grid(TextureId::default(), None, 1.0, *crate::theme::NIGHT)];
        }
        let behind = self.fade.layers(ctx);
        let mut out = Vec::new();
        if behind.base.is_none() && behind.top.is_none() {
            out.push(grid(TextureId::default(), None, 1.0, t::COLOR_CANVAS));
        }
        for layer in [behind.base, behind.top].into_iter().flatten() {
            out.push(grid(
                layer.texture,
                Some(layer.area),
                layer.alpha,
                Color32::WHITE,
            ));
        }
        // What the sky has over its texture comes back with it.
        if let Some(Shows::Sky(_)) = self.shows {
            let along = behind.top.map_or(1.0, |top| top.alpha);
            sky_through_veil(&mut out, screen, self.disc, along, foot, &alpha);
        }
        out
    }
}

/// The stars and rings over the sky's texture, as strong as the veil over them, so they neither dim
/// nor shift when the page moves. The moon's disc stays below the veil's foot. A moon that stepped
/// back has no rings.
fn sky_through_veil(
    out: &mut Vec<Shape>,
    screen: Rect,
    disc: Option<(Pos2, f32)>,
    along: f32,
    foot: f32,
    alpha: &dyn Fn(Pos2) -> f32,
) {
    for star in stars(screen, along) {
        if let Shape::Circle(mut circle) = star {
            if circle.center.y < foot {
                let over = alpha(circle.center);
                circle.fill = circle.fill.gamma_multiply(over);
                out.push(Shape::Circle(circle));
            }
        }
    }
    let Some((center, r)) = disc else {
        return;
    };
    let ink = t::SKY_RING.gamma_multiply(along);
    let mut mesh = Mesh::default();
    for factor in RING_FACTORS {
        let radius = r * factor;
        // Only the arc above the foot: the rest of the ring already shows as it is.
        let base = mesh.vertices.len() as u32;
        let steps = 240u32;
        for i in 0..=steps {
            let turn = i as f32 / steps as f32 * std::f32::consts::TAU;
            let n = vec2(turn.cos(), turn.sin());
            let on = center + n * radius;
            let over = if on.y < foot { alpha(on) } else { 0.0 };
            for (d, shade) in [
                (-1.0, Color32::TRANSPARENT),
                (0.0, ink.gamma_multiply(over)),
                (1.0, Color32::TRANSPARENT),
            ] {
                mesh.vertices.push(Vertex {
                    pos: on + n * d,
                    uv: WHITE_UV,
                    color: shade,
                });
            }
        }
        for i in 0..steps {
            let (a, b) = (base + i * 3, base + (i + 1) * 3);
            if mesh.vertices[a as usize + 1].color.a() == 0
                && mesh.vertices[b as usize + 1].color.a() == 0
            {
                continue;
            }
            for k in 0..2 {
                mesh.indices
                    .extend([a + k, b + k, b + k + 1, a + k, b + k + 1, a + k + 1]);
            }
        }
    }
    out.push(Shape::mesh(mesh));
}

/// The moon's middle and radius in this window.
pub(super) fn moon_disc(screen: Rect, moon: Moon) -> (Pos2, f32) {
    let ([x, y], r) = moon.place();
    (
        pos2(
            screen.left() + screen.width() * x,
            screen.top() + screen.height() * y,
        ),
        r * (screen.height() / 800.0).clamp(0.75, 1.5),
    )
}

/// The moon and its rings, as faint as `alpha`.
fn paint_moon(out: &mut Vec<Shape>, ctx: &Context, center: Pos2, r: f32, alpha: f32) {
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
            glow: Color32::from_rgba_premultiplied(60, 20, 20, 60),
        }
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

    /// A long ramp, not a bar: no step between rows anywhere, half gone by the foot of the top bar.
    #[test]
    fn the_veil_eases_away_and_stays_deepest_behind_the_brand() {
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0));
        let brand = Rect::from_min_size(pos2(48.0, 29.0), vec2(118.0, 30.0));
        let at = |x: f32, y: f32| veil_at(screen, brand, pos2(x, y));
        let mut last = at(800.0, 0.0);
        for y in 1..200 {
            let here = at(800.0, y as f32);
            assert!(
                here <= last && last - here < 0.02,
                "at {y}: {last} to {here}"
            );
            last = here;
        }
        assert!(at(800.0, 68.0) < 0.6, "{}", at(800.0, 68.0));
        assert_eq!(at(800.0, veil_height(screen)), 0.0);
        assert!(at(100.0, 44.0) > 0.9, "the wordmark stands on the backdrop");
        assert!(
            at(800.0, 44.0) < 0.75,
            "what passes under the glass still shows"
        );
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
}
