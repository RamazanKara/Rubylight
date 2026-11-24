//! Sky placement, scroll veil blending, moon geometry, and starfield helpers.

use super::*;

/// A placed moon fading from one alpha to another, from a time on.
/// Passes after its page or window changed in which a moon placed again moves rather than fades.
pub(super) const SETTLE_PASSES: u64 = 8;

#[derive(Clone, Copy)]
pub(super) struct MoonFade {
    pub(super) disc: (Pos2, f32),
    /// The page it stands for.
    pub(super) page: u64,
    pub(super) from: f32,
    pub(super) to: f32,
    pub(super) at: f64,
}

impl MoonFade {
    pub(super) fn alpha(&self, now: f64, ms: u32) -> f32 {
        if ms == 0 {
            return self.to;
        }
        let along = ((now - self.at) as f32 * 1000.0 / ms as f32).clamp(0.0, 1.0);
        egui::lerp(self.from..=self.to, along)
    }
}

/// Tells the page whether to say what it holds, and its cards where the moon is.
pub(super) fn publish(ctx: &Context, placing: bool, disc: Option<(Pos2, f32)>) {
    ctx.data_mut(|d| {
        d.insert_temp(placing_id(), placing);
        d.insert_temp(disc_id(), disc);
    });
}

/// What the last frame painted of the sky, for a test to follow a fade.
#[cfg(test)]
#[derive(Clone, Default, Debug)]
pub(in crate::ui) struct Painted {
    /// Each moon painted, with its alpha.
    pub moons: Vec<((Pos2, f32), f32)>,
    /// How far the backdrop's fade is, and whether one runs.
    pub along: f32,
    pub fading: bool,
}

#[cfg(test)]
pub(in crate::ui) fn painted_id() -> egui::Id {
    egui::Id::unique("moon-painted")
}

#[cfg(test)]
pub(in crate::ui) fn painted(ctx: &Context) -> Painted {
    ctx.data(|d| d.get_temp(painted_id())).unwrap_or_default()
}

/// What a page laid out this frame, as at its top, in which window: where the moon must not stand.
#[derive(Clone)]
pub(in crate::ui) struct Held {
    pub(super) page: u64,
    pub(super) screen: Rect,
    pub(super) rects: Arc<[Rect]>,
    /// How far the page was scrolled: what lay above that was not drawn.
    pub(super) above: f32,
    pub(super) pass: u64,
}

impl Held {
    pub(super) fn at_top(&self) -> bool {
        self.above < 0.5
    }

    /// What it holds, and where it was scrolled, all it could not show: the whole width above.
    pub(super) fn with_unseen(&self) -> Vec<Rect> {
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

pub(super) fn held_id() -> egui::Id {
    egui::Id::unique("moon-held")
}

pub(super) fn page_id() -> egui::Id {
    egui::Id::unique("moon-page")
}

pub(in crate::ui) fn placing_id() -> egui::Id {
    egui::Id::unique("moon-placing")
}

pub(super) fn disc_id() -> egui::Id {
    egui::Id::unique("moon-disc")
}

/// The sky's moon this frame, for a card that passes over it; None where there is none.
pub(in crate::ui) fn disc_of(ctx: &Context) -> Option<(Pos2, f32)> {
    ctx.data(|d| d.get_temp(disc_id())).flatten()
}

/// Whether the backdrop places a moon this frame and needs what the page holds.
pub(in crate::ui) fn placing(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp(placing_id())).unwrap_or(false)
}

/// Where the page's shapes lie as laid out, moved by `down` to the page's top: cards, chips, rows,
/// words and covers. The blurred shadows and halos that hover and focus bring are left out, and so
/// is the reach of a blur.
pub(in crate::ui) fn laid_out<'a>(
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
pub(in crate::ui) fn hold_page(ctx: &Context, screen: Rect, rects: Vec<Rect>, above: f32) {
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
pub(super) fn clear_of(rects: &[Rect], (center, r): (Pos2, f32), gap: f32) -> bool {
    rects
        .iter()
        .all(|rect| rect.expand(gap).distance_to_pos(center) >= r)
}

/// Where a page's moon stands in open sky, never under a card, chip or row and never cut by one:
/// its own place where that is clear of what the page holds by a gap, else the nearest clear place
/// on the window's right below the chrome, smaller if it must be, down to [`SKY_SMALLEST`]. None
/// where the page leaves no such sky: the moon steps back. The first run's moon keeps its place.
pub(in crate::ui) fn find_sky(screen: Rect, moon: Moon, held: &[Rect]) -> Option<(Pos2, f32)> {
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
                screen.top() + super::super::chrome::PAGE_TOP + r + gap,
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

pub(super) fn smoothstep(from: f32, to: f32, x: f32) -> f32 {
    let t = ((x - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Where the frosted band under the chrome ends: a little below the capsules. Down to here the page
/// never shows, so nothing reads between the capsules or behind the brand.
pub(in crate::ui) fn band_foot(screen: Rect) -> f32 {
    screen.top() + super::super::chrome::BAR_TOP + super::super::chrome::BAR_H + BAND_BELOW
}

/// How much of the backdrop lies over the page at `p`, with the page fully scrolled: all of it in
/// the band behind the chrome, then a soft ease to nothing, and a pool behind the brand, which has
/// no glass of its own.
pub(super) fn veil_at(screen: Rect, brand: Rect, p: Pos2) -> f32 {
    let foot = screen.top() + veil_height(screen);
    let ramp = 1.0 - smoothstep(band_foot(screen), foot, p.y);
    let reach = brand.size() / 2.0 + vec2(VEIL_BRAND_REACH, VEIL_BRAND_REACH * 0.7);
    let away = (p - brand.center()) / reach;
    let pool = VEIL_BRAND_ALPHA * (1.0 - smoothstep(0.5, 1.0, away.length()));
    ramp.max(pool)
}

/// The ramp ends above where the moon can hang and where the page colour starts to rise.
pub(super) fn veil_height(screen: Rect) -> f32 {
    (screen.height() * VEIL_FRACTION).clamp(VEIL_LEAST, VEIL_MOST)
}

impl Ambient {
    /// Where the page passes beneath the chrome it dissolves into the backdrop: the same textures in
    /// the same place, laid over the page, so with nothing passing there it cannot be seen at all.
    /// Behind the capsules it is whole, a frosted band, so nothing shows between them; below them it
    /// eases away, never a hard edge. `brand` keeps its words readable. None with the page at its
    /// start.
    pub(in crate::ui) fn veil(
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
        // A few rows through the band, which is level, and rows close enough for the ease below it;
        // columns only where the pool bends.
        let band = band_foot(screen).min(foot);
        let rows: Vec<f32> = (0..BAND_ROWS)
            .map(|i| egui::lerp(screen.top()..=band, i as f32 / BAND_ROWS as f32))
            .chain((0..=VEIL_ROWS).map(|i| egui::lerp(band..=foot, i as f32 / VEIL_ROWS as f32)))
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
            out.push(grid(TextureId::default(), None, 1.0, shade::COLOR_CANVAS.get()));
        }
        for layer in [behind.base, behind.top].into_iter().flatten() {
            out.push(grid(
                layer.texture,
                Some(layer.area),
                layer.alpha,
                Color32::WHITE,
            ));
        }
        // The page's wash over art by day, as the backdrop has it under the page.
        if look() == Look::Day {
            let along = behind.top.map_or(1.0, |top| top.alpha);
            let is_art = |shows: Option<Shows>| f32::from(u8::from(shows == Some(Shows::Art)));
            let art = if behind.base.is_some() { egui::lerp(is_art(self.before)..=is_art(self.shows), along) } else { is_art(self.shows) };
            if art > 0.0 {
                out.push(grid(TextureId::default(), None, DAY_WASH * art, shade::COLOR_CANVAS.get()));
            }
        }
        // What the sky has over its texture comes back with it.
        if let Some(Shows::Sky(_)) = self.shows {
            let along = behind.top.map_or(1.0, |top| top.alpha);
            sky_through_veil(&mut out, screen, self.disc, along, foot, &alpha, &glass::tinted(ctx));
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
    tinted: &[Rect],
) {
    for star in clear_of_tints(stars(screen, along), tinted) {
        if let Shape::Circle(mut circle) = star
            && circle.center.y < foot {
                let over = alpha(circle.center);
                circle.fill = circle.fill.gamma_multiply(over);
                out.push(Shape::Circle(circle));
            }
    }
    let Some((center, r)) = disc else {
        return;
    };
    let ink = shade::SKY_RING.get().gamma_multiply(along);
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
pub(in crate::ui) fn moon_disc(screen: Rect, moon: Moon) -> (Pos2, f32) {
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
pub(super) fn paint_moon(out: &mut Vec<Shape>, ctx: &Context, center: Pos2, r: f32, alpha: f32) {
    for factor in RING_FACTORS {
        out.push(Shape::circle_stroke(
            center,
            r * factor,
            Stroke::new(1.0, shade::SKY_RING.get().gamma_multiply(alpha)),
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

/// By day, the page's light colour over a game's ambient, as strong as the art shows
/// ([`DAY_WASH`]); none in the other looks.
pub(super) fn day_wash(screen: Rect, strength: f32) -> Option<Shape> {
    (look() == Look::Day).then(|| Shape::rect_filled(screen, 0.0, shade::COLOR_CANVAS.get().gamma_multiply(DAY_WASH * strength)))
}

/// The page's colour rising over the ambient, so shelves sit on a calm ground.
pub(super) fn fade_over(screen: Rect, strength: f32) -> Shape {
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
        shade::TINT_FADE.get().gamma_multiply(FADE_NEAR_ALPHA),
    );
    row(&mut mesh, at(1.0), shade::TINT_FADE.get());
    for r in 0..2u32 {
        let a = r * 2;
        mesh.indices.extend([a, a + 1, a + 3, a, a + 3, a + 2]);
    }
    Shape::mesh(mesh)
}

/// The stars that no see-through tint lies over: a tint blurs nothing, so a star under a card or a
/// button would read as a speck on it rather than the sky behind glass.
pub(super) fn clear_of_tints(stars: Vec<Shape>, tinted: &[Rect]) -> Vec<Shape> {
    stars
        .into_iter()
        .filter(|star| match star {
            Shape::Circle(circle) => !tinted
                .iter()
                .any(|rect| rect.expand(circle.radius + 1.0).contains(circle.center)),
            _ => true,
        })
        .collect()
}

/// A few faint stars, the same every frame; none by day, whose sky has a low sun instead.
pub(super) fn stars(screen: Rect, strength: f32) -> Vec<Shape> {
    if look() == Look::Day {
        return Vec::new();
    }
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
