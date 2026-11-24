//! Soft shadow nine-slices, glass surface finishes, hero play fills, feel springs, and deferred painting.

use super::*;

/// The alpha of a rounded rectangle under a Gaussian, `blur` as CSS means it (sigma is half).
pub(super) fn bake_soft(size: Vec2, r: f32, blur: f32, hollow: bool) -> egui::ColorImage {
    // Half resolution holds a blur this soft, and keeps the bake well under a millisecond each.
    let scale = if blur >= 8.0 { 0.5 } else { 1.0 };
    let pad = blur;
    let w = (((size.x + 2.0 * pad) * scale).ceil() as usize).max(2);
    let h = (((size.y + 2.0 * pad) * scale).ceil() as usize).max(2);
    let mut alpha = vec![0.0f32; w * h];
    let half = size / 2.0;
    let center = vec2(w as f32, h as f32) / (2.0 * scale);
    for y in 0..h {
        for x in 0..w {
            let p = vec2((x as f32 + 0.5) / scale, (y as f32 + 0.5) / scale) - center;
            let q = vec2(p.x.abs() - half.x + r, p.y.abs() - half.y + r);
            let d = vec2(q.x.max(0.0), q.y.max(0.0)).length() + q.x.max(q.y).min(0.0) - r;
            alpha[y * w + x] = (0.5 - d * scale).clamp(0.0, 1.0);
        }
    }
    let sigma = (blur / 2.0 * scale).max(0.5);
    let reach = (sigma * 3.0).ceil() as isize;
    let weights: Vec<f32> = (-reach..=reach)
        .map(|i| (-(i as f32).powi(2) / (2.0 * sigma * sigma)).exp())
        .collect();
    let total: f32 = weights.iter().sum();
    let mut across = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            across[y * w + x] = (-reach..=reach)
                .zip(&weights)
                .map(|(i, k)| alpha[y * w + (x as isize + i).clamp(0, w as isize - 1) as usize] * k)
                .sum::<f32>()
                / total;
        }
    }
    let mut image = egui::ColorImage::filled([w, h], Color32::TRANSPARENT);
    for y in 0..h {
        for x in 0..w {
            let a: f32 = (-reach..=reach)
                .zip(&weights)
                .map(|(i, k)| {
                    across[(y as isize + i).clamp(0, h as isize - 1) as usize * w + x] * k
                })
                .sum::<f32>()
                / total;
            // A hollow shadow, as CSS's box-shadow: nothing under the shape itself.
            let a = if hollow { a * (1.0 - alpha[y * w + x]) } else { a };
            let v = (a * 255.0).round() as u8;
            image.pixels[y * w + x] = Color32::from_rgba_premultiplied(v, v, v, v);
        }
    }
    image
}

/// A blurred rounded shape, drawn as a nine-slice of a bake kept for its radius and blur. A shape
/// too small for flat middles (a capsule under a wide blur) gets a bake of its own short side.
pub fn soft(out: &mut Vec<Shape>, ctx: &Context, rect: Rect, r: f32, blur: f32, color: Color32) {
    soft_shape(out, ctx, rect, r, blur, color, false);
}

/// [`soft`] with nothing under `rect` itself: a glow around a shape, in any order with its fill.
pub fn soft_around(
    out: &mut Vec<Shape>,
    ctx: &Context,
    rect: Rect,
    r: f32,
    blur: f32,
    color: Color32,
) {
    soft_shape(out, ctx, rect, r, blur, color, true);
}

/// How long a window keeps still before it counts as resting.
const WINDOW_REST_S: f64 = 0.25;

/// Whether the window's size changed in the last moments, as seen by the bakes that ask.
pub(super) fn window_resizing(ctx: &Context) -> bool {
    let id = Id::unique("nightjar-soft-window");
    let (size, now) = (ctx.content_rect().size(), ctx.input(|i| i.time));
    match ctx.data(|d| d.get_temp::<(Vec2, f64)>(id)) {
        Some((seen, since)) if seen == size => now - since < WINDOW_REST_S,
        Some(_) => {
            ctx.data_mut(|d| d.insert_temp(id, (size, now)));
            true
        }
        None => {
            ctx.data_mut(|d| d.insert_temp(id, (size, f64::NEG_INFINITY)));
            false
        }
    }
}

/// The bakes [`soft`] and [`soft_around`] draw with: shadows and halos, not what a page lays out.
pub type SoftTextures = Arc<std::collections::HashSet<TextureId>>;

fn soft_textures_id() -> Id {
    Id::unique("nightjar-soft-textures")
}

/// Forgets the soft bakes (a new renderer, Linux): they are baked again as they are drawn.
#[cfg_attr(windows, allow(dead_code))]
pub fn forget_soft(ctx: &Context) {
    ctx.data_mut(|d| {
        d.remove_by_type::<TextureHandle>();
        d.remove_by_type::<(TextureHandle, Vec2)>();
        d.remove_by_type::<SoftTextures>();
    });
}

/// Every texture a blurred shape was drawn with so far.
pub fn soft_textures(ctx: &Context) -> SoftTextures {
    ctx.data(|d| d.get_temp(soft_textures_id())).unwrap_or_default()
}

fn soft_shape(
    out: &mut Vec<Shape>,
    ctx: &Context,
    rect: Rect,
    r: f32,
    blur: f32,
    color: Color32,
    hollow: bool,
) {
    if blur <= 0.0 || color.a() == 0 || !rect.is_positive() {
        return;
    }
    let r = fit_radius(rect, r);
    let need = 2.0 * (r.max(blur) + 1.0);
    let baked = vec2(
        rect.width().min(need).round().max(1.0),
        rect.height().min(need).round().max(1.0),
    );
    let id = Id::unique((
        "nightjar-soft",
        r.round() as i32,
        blur.round() as i32,
        baked.x as i32,
        baked.y as i32,
        hollow,
    ));
    let latest_id = Id::unique(("nightjar-soft-latest", r.round() as i32, blur.round() as i32, hollow));
    let exact = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id));
    let (texture, baked) = match exact {
        Some(texture) => (texture, baked),
        None => {
            // A side under a wide blur follows the window; a drag would bake at each size on the
            // UI thread. It takes the last bake until the window rests, then its own.
            let stand_in = window_resizing(ctx)
                .then(|| ctx.data(|d| d.get_temp::<(egui::TextureHandle, Vec2)>(latest_id)))
                .flatten();
            match stand_in {
                Some(latest) => {
                    ctx.request_repaint_after(std::time::Duration::from_secs_f64(WINDOW_REST_S));
                    latest
                }
                None => {
                    let handle = ctx.load_texture(
                        "glass-soft",
                        bake_soft(baked, r.min(baked.x / 2.0).min(baked.y / 2.0), blur, hollow),
                        egui::TextureOptions::LINEAR,
                    );
                    ctx.data_mut(|d| {
                        d.insert_temp(id, handle.clone());
                        d.insert_temp(latest_id, (handle.clone(), baked));
                        let soft = d.get_temp_mut_or_default::<SoftTextures>(soft_textures_id());
                        Arc::make_mut(soft).insert(handle.id());
                    });
                    (handle, baked)
                }
            }
        }
    };
    let pad = blur;
    let (mid_x, mid_y) = (
        slice_middle(rect.x_range(), baked.x),
        slice_middle(rect.y_range(), baked.y),
    );
    let xs = [rect.left() - pad, mid_x.0, mid_x.1, rect.right() + pad];
    let ys = [rect.top() - pad, mid_y.0, mid_y.1, rect.bottom() + pad];
    let (us, vs) = ([0.0, 0.5, 0.5, 1.0], [0.0, 0.5, 0.5, 1.0]);
    let mut mesh = Mesh::with_texture(texture.id());
    for j in 0..4 {
        for i in 0..4 {
            mesh.vertices.push(Vertex {
                pos: pos2(xs[i], ys[j]),
                uv: pos2(us[i], vs[j]),
                color,
            });
        }
    }
    for j in 0..3u32 {
        for i in 0..3u32 {
            let a = j * 4 + i;
            mesh.indices.extend([a, a + 1, a + 5, a, a + 5, a + 4]);
        }
    }
    out.push(Shape::mesh(mesh));
}

/// Where a nine-slice's stretching middle runs along one side. A side that rounded up for its bake
/// is shorter than the bake: its two halves meet at the centre and never overlap, since an overlap
/// paints the blur twice and shows as a bright line out to the bake's edge.
pub(super) fn slice_middle(side: egui::Rangef, baked: f32) -> (f32, f32) {
    if side.span() >= baked {
        (side.min + baked / 2.0, side.max - baked / 2.0)
    } else {
        (side.center(), side.center())
    }
}

/// The focus language of every page: a dark gap, a thick ring, a warm halo behind them. High
/// contrast: a thicker ring, no halo. `on_moon`: the paper ring that reads against the Play button.
/// The halo stays outside the rectangle, so it goes before or after the focused thing's own shapes.
/// Under [`with_halo_reach`] it fades out within that reach.
pub fn focus(out: &mut Vec<Shape>, ctx: &Context, rect: Rect, r: f32, on_moon: bool) {
    let high = high_contrast();
    let (ring, width, gap) = if high {
        (
            t::HC_FOCUS_RING_COLOR,
            t::HC_FOCUS_RING_WIDTH,
            t::HC_FOCUS_GAP,
        )
    } else if on_moon {
        (
            shade::FOCUS_RING_COLOR_ON_MOON.get(),
            t::FOCUS_RING_WIDTH,
            t::FOCUS_GAP,
        )
    } else {
        (shade::FOCUS_RING_COLOR.get(), t::FOCUS_RING_WIDTH, t::FOCUS_GAP)
    };
    if !high {
        let spread = gap + width;
        soft_around(
            out,
            ctx,
            rect.expand(spread),
            r + spread,
            ctx.data(|d| d.get_temp(halo_reach_id()))
                .unwrap_or(t::FOCUS_HALO_BLUR),
            shade::FOCUS_HALO_COLOR.get(),
        );
    }
    let gap_rect = rect.expand(gap);
    let gap_color = shade::FOCUS_GAP_COLOR.get();
    rim(out, ctx, gap_rect, r + gap, gap, &|_| gap_color);
    let ring_rect = rect.expand(gap + width);
    rim(out, ctx, ring_rect, r + gap + width, width, &|_| ring);
}

fn halo_reach_id() -> Id {
    Id::unique("nightjar-halo-reach")
}

/// Runs `add` with every focus halo fading out within `reach` points of its ring: for a panel whose
/// edge would otherwise cut the glow in a hard line.
pub fn with_halo_reach<R>(ctx: &Context, reach: f32, add: impl FnOnce() -> R) -> R {
    ctx.data_mut(|d| d.insert_temp(halo_reach_id(), reach));
    let inner = add();
    ctx.data_mut(|d| d.remove::<f32>(halo_reach_id()));
    inner
}

/// The glass surface of `material`: shadow, backdrop, fill, sheen, rim. Solid in high contrast.
pub fn surface(out: &mut Vec<Shape>, ctx: &Context, rect: Rect, r: f32, material: Material) {
    let tier = tier(ctx);
    let recipe = recipe(material, tier);
    soft(
        out,
        ctx,
        rect.translate(vec2(0.0, recipe.shadow_y)),
        r,
        recipe.shadow_blur,
        recipe.shadow,
    );
    let screen = ctx.content_rect();
    let shade = Color32::from_gray((recipe.brightness * 255.0).round() as u8);
    match tier {
        Tier::Solid => {}
        Tier::Live => image(out, ctx, rect, r, LIVE_BACKDROP, uv_in(screen, rect), shade),
        Tier::Baked => {
            let behind = backdrop(ctx);
            // Under the layers, so a cross-fade from nothing has the canvas behind it.
            solid(
                out,
                ctx,
                rect,
                r,
                scale_rgb(shade::COLOR_CANVAS.get(), recipe.brightness),
            );
            for layer in [behind.base, behind.top].into_iter().flatten() {
                let tint = shade.gamma_multiply(layer.alpha);
                image(
                    out,
                    ctx,
                    rect,
                    r,
                    layer.texture,
                    uv_in(layer.area, rect),
                    tint,
                );
            }
        }
    }
    finish(out, ctx, rect, r, &recipe);
}

/// The stream menu's glass over a stream's video: shadow, fill, sheen and rim of the baked recipe,
/// solid in high contrast. No backdrop of any kind, whichever tier the launcher runs on: nothing
/// reads the video, and it is only painted while the menu is open.
pub fn menu_surface(out: &mut Vec<Shape>, ctx: &Context, rect: Rect, r: f32) {
    without_backdrop(out, ctx, rect, r, Material::Menu);
}

/// What stays up over a stream while it plays (the statistics, a notice): see-through with a lit
/// rim, no shadow and nothing read behind it, so the video is never composed again for it; the
/// menu's solid in high contrast.
pub fn hud_surface(out: &mut Vec<Shape>, ctx: &Context, rect: Rect, r: f32) {
    let recipe = if tier(ctx) == Tier::Solid {
        recipe(Material::Menu, Tier::Solid)
    } else {
        hud_recipe()
    };
    finish(out, ctx, rect, r, &recipe);
}

/// The HUD's recipe: `glass.menu.hud`.
pub fn hud_recipe() -> Recipe {
    recipe!(GLASS_MENU_HUD_BRIGHTNESS GLASS_MENU_HUD_FILL GLASS_MENU_HUD_SHEEN_TOP GLASS_MENU_HUD_SHEEN_END GLASS_MENU_HUD_RIM_TOP GLASS_MENU_HUD_RIM_MID GLASS_MENU_HUD_RIM_BOTTOM GLASS_MENU_HUD_RIM_WIDTH GLASS_MENU_HUD_SHADOW_COLOR GLASS_MENU_HUD_SHADOW_Y GLASS_MENU_HUD_SHADOW_BLUR)
}

/// The baked recipe of `material`, reading nothing behind it: for what floats over anything on any
/// tier, and has no blur region of its own.
pub fn without_backdrop(out: &mut Vec<Shape>, ctx: &Context, rect: Rect, r: f32, material: Material) {
    let tier = if tier(ctx) == Tier::Solid { Tier::Solid } else { Tier::Baked };
    let recipe = recipe(material, tier);
    soft(out, ctx, rect.translate(vec2(0.0, recipe.shadow_y)), r, recipe.shadow_blur, recipe.shadow);
    finish(out, ctx, rect, r, &recipe);
}

/// What every glass surface has over its backdrop: fill, sheen, rim.
fn finish(out: &mut Vec<Shape>, ctx: &Context, rect: Rect, r: f32, recipe: &Recipe) {
    solid(out, ctx, rect, r, recipe.fill);
    if recipe.sheen_top.a() > 0 && recipe.sheen_end > 0.0 {
        let end = recipe.sheen_end;
        let sheen = gradient(
            rect,
            180.0,
            &[(0.0, recipe.sheen_top), (end, Color32::TRANSPARENT)],
        );
        shaded(out, ctx, rect, r, &[end], &sheen);
    }
    let rim_color = gradient(
        rect,
        168.0,
        &[
            (0.0, recipe.rim_top),
            (0.34, recipe.rim_mid),
            (0.62, recipe.rim_mid),
            (1.0, recipe.rim_bottom),
        ],
    );
    rim(out, ctx, rect, r, recipe.rim_width, &rim_color);
    if recipe.hairline.a() > 0 {
        let hairline = recipe.hairline;
        rim(out, ctx, rect, r, 1.0, &|_| hairline);
    }
}

pub(super) fn scale_rgb(color: Color32, by: f32) -> Color32 {
    let channel = |v: u8| (f32::from(v) * by).round() as u8;
    Color32::from_rgba_premultiplied(
        channel(color.r()),
        channel(color.g()),
        channel(color.b()),
        color.a(),
    )
}

/// Where `rect` lies inside `area`, as a uv rectangle: glass samples the screen behind it.
pub(super) fn uv_in(area: Rect, rect: Rect) -> Rect {
    let at = |p: Pos2| {
        pos2(
            (p.x - area.min.x) / area.width().max(1e-3),
            (p.y - area.min.y) / area.height().max(1e-3),
        )
    };
    Rect::from_min_max(at(rect.min), at(rect.max))
}

/// What lies under a hero's cover: the colour of its ambient glowing all round (`glow`, as wide as
/// `glow_blur`) and a dark shadow below it. Both are baked nine-slices, the same on every tier that
/// has art; high contrast has neither.
#[allow(clippy::too_many_arguments, reason = "two shadows, each with its own shape")]
pub fn cover_glow(
    out: &mut Vec<Shape>,
    ctx: &Context,
    rect: Rect,
    r: f32,
    glow: Color32,
    glow_blur: f32,
    shadow_y: f32,
    shadow_blur: f32,
) {
    if high_contrast() {
        return;
    }
    soft(out, ctx, rect, r, glow_blur, glow);
    soft(
        out,
        ctx,
        rect.translate(vec2(0.0, shadow_y)),
        r,
        shadow_blur,
        Color32::from_black_alpha(140),
    );
}

fn tinted_id() -> Id {
    Id::unique("nightjar-glass-tinted")
}

/// Notes where a see-through tint went this pass. A tint blurs nothing, so the sky's stars, which
/// are sharp dots over its texture, would read through it as specks on a card or a button: the
/// backdrop leaves them out there ([`tinted`]).
fn note_tint(ctx: &Context, rect: Rect) {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        let noted = d.get_temp_mut_or_default::<(u64, Vec<Rect>)>(tinted_id());
        if noted.0 != pass {
            *noted = (pass, Vec::new());
        }
        noted.1.push(rect);
    });
}

/// Where see-through tints went this pass, so far.
pub fn tinted(ctx: &Context) -> Vec<Rect> {
    let pass = ctx.cumulative_pass_nr();
    ctx.data(|d| d.get_temp::<(u64, Vec<Rect>)>(tinted_id()))
        .filter(|(at, _)| *at == pass)
        .map(|(_, rects)| rects)
        .unwrap_or_default()
}

/// A flat tint inside glass or on art: the rim of a group. No blur, no nesting.
pub fn group(out: &mut Vec<Shape>, ctx: &Context, rect: Rect, r: f32, fill: Color32) {
    if high_contrast() {
        solid(out, ctx, rect, r, t::SOLID_GROUP_FILL);
        rim(out, ctx, rect, r, 1.5, &|_| t::HC_COLOR_RIM);
        return;
    }
    note_tint(ctx, rect);
    solid(out, ctx, rect, r, fill);
    let color = gradient(
        rect,
        168.0,
        &[
            (0.0, shade::TINT_GROUP_RIM_TOP.get()),
            (0.4, shade::TINT_GROUP_RIM_MID.get()),
            (1.0, shade::TINT_GROUP_RIM_BOTTOM.get()),
        ],
    );
    rim(out, ctx, rect, r, 1.0, &color);
}

/// A control's tinted capsule.
pub fn control(out: &mut Vec<Shape>, ctx: &Context, rect: Rect, r: f32) {
    if high_contrast() {
        solid(out, ctx, rect, r, t::SOLID_CONTROL_FILL);
        rim(out, ctx, rect, r, 1.5, &|_| t::HC_COLOR_RIM);
        return;
    }
    note_tint(ctx, rect);
    solid(out, ctx, rect, r, shade::TINT_CONTROL_FILL.get());
    let color = gradient(
        rect,
        168.0,
        &[
            (0.0, shade::TINT_CONTROL_RIM_TOP.get()),
            (0.4, shade::TINT_CONTROL_RIM_MID.get()),
            (1.0, shade::TINT_CONTROL_RIM_BOTTOM.get()),
        ],
    );
    rim(out, ctx, rect, r, 1.0, &color);
}

/// The ink on [`play`]: its amber is the same in every look, so its dark ink is too (black on high
/// contrast's moon).
pub fn on_play() -> Color32 {
    if high_contrast() { t::HC_COLOR_ON_MOON } else { t::COLOR_ON_MOON }
}

/// The solid amber of Play, the one thing lit from inside.
pub fn play(out: &mut Vec<Shape>, ctx: &Context, rect: Rect, r: f32) {
    if high_contrast() {
        solid(out, ctx, rect, r, t::HC_COLOR_MOON);
        return;
    }
    soft(
        out,
        ctx,
        rect.translate(vec2(0.0, 8.0)),
        r,
        t::PLAY_GLOW_BLUR,
        shade::PLAY_GLOW.get(),
    );
    soft(
        out,
        ctx,
        rect.translate(vec2(0.0, 2.0)),
        r,
        6.0,
        Color32::from_rgba_premultiplied(0, 0, 0, 90),
    );
    let fill = gradient(
        rect,
        180.0,
        &[(0.0, t::PLAY_FILL_TOP), (1.0, t::PLAY_FILL_BOTTOM)],
    );
    shaded(out, ctx, rect, r, &[], &fill);
    let edge = gradient(
        rect,
        180.0,
        &[
            (0.0, t::PLAY_RIM_TOP),
            (0.45, Color32::TRANSPARENT),
            (1.0, shade::PLAY_RIM_BOTTOM.get()),
        ],
    );
    rim(out, ctx, rect, r, 1.0, &edge);
}

/// A cover's lip and sheen, over its art.
pub fn cover_finish(out: &mut Vec<Shape>, ctx: &Context, rect: Rect, r: f32, hover_boost: f32) {
    if high_contrast() {
        rim(out, ctx, rect, r, 1.5, &|_| t::HC_COLOR_RIM);
        return;
    }
    let sheen = gradient(
        rect,
        165.0,
        &[(0.0, t::COVER_SHEEN_TOP), (0.26, Color32::TRANSPARENT)],
    );
    shaded(out, ctx, rect, r, &[0.1, 0.2, 0.3], &sheen);
    // Every channel, as the colour is premultiplied: the night rim is white, the day one's foot dark.
    let boost = |c: Color32| {
        let up = |v: u8| (f32::from(v) * hover_boost).min(255.0) as u8;
        let a = up(c.a());
        Color32::from_rgba_premultiplied(up(c.r()).min(a), up(c.g()).min(a), up(c.b()).min(a), a)
    };
    let color = gradient(
        rect,
        168.0,
        &[
            (0.0, boost(shade::COVER_RIM_TOP.get())),
            (0.3, boost(shade::COVER_RIM_BOTTOM.get())),
            (1.0, boost(shade::COVER_RIM_BOTTOM.get())),
        ],
    );
    rim(out, ctx, rect, r, 1.0, &color);
}

/// How a control or cover feels this frame: its states and the sprung scale that follows them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Feel {
    pub hovered: bool,
    pub focused: bool,
    pub pressed: bool,
    pub scale: f32,
}

/// The states of `response` and the scale they spring to: `focus_scale` on focus, `hover_scale`
/// under the pointer, the press scale while held. With reduced motion nothing scales.
pub fn feel(ctx: &Context, response: &egui::Response, focus_scale: f32, hover_scale: f32) -> Feel {
    let (hovered, focused, pressed) = (
        response.hovered(),
        response.has_focus(),
        response.is_pointer_button_down_on(),
    );
    let target = if !spring::motion(ctx) {
        t::REDUCED_FOCUS_SCALE
    } else if pressed {
        t::PRESS_SCALE
    } else if focused {
        focus_scale
    } else if hovered {
        hover_scale
    } else {
        1.0
    };
    let tune = if pressed {
        spring::PRESS
    } else {
        spring::FOCUS
    };
    let scale = spring::animate(ctx, response.id.with("feel"), target, tune);
    Feel {
        hovered,
        focused,
        pressed,
        scale,
    }
}

pub(super) fn deferred_id() -> Id {
    Id::unique("nightjar-glass-deferred")
}

/// Shapes to paint after the rest of the page: a lifted cover rises over its neighbours.
pub fn defer(ui: &Ui, shapes: Vec<Shape>) {
    let clip = ui.clip_rect();
    ui.ctx().data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<(Rect, Vec<Shape>)>>(deferred_id())
            .push((clip, shapes))
    });
}

/// [`defer`] for what has the focus: its halo may reach past the scroll area, whose edge would cut it
/// straight. A focused thing has been scrolled into view, so nothing else shows over its neighbours.
pub fn defer_focused(ui: &Ui, shapes: Vec<Shape>) {
    let clip = ui.clip_rect().expand(t::FOCUS_HALO_BLUR + t::FOCUS_GAP + t::FOCUS_RING_WIDTH);
    ui.ctx().data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<(Rect, Vec<Shape>)>>(deferred_id())
            .push((clip, shapes))
    });
}

/// What is deferred so far, left in place.
pub fn deferred(ctx: &Context) -> Vec<(Rect, Vec<Shape>)> {
    ctx.data(|d| d.get_temp(deferred_id())).unwrap_or_default()
}

/// What is deferred so far, taken away, for a test to look at.
#[cfg(test)]
pub fn take_deferred(ui: &Ui) -> Vec<(Rect, Vec<Shape>)> {
    ui.ctx().data_mut(|d| {
        std::mem::take(d.get_temp_mut_or_default::<Vec<(Rect, Vec<Shape>)>>(deferred_id()))
    })
}

/// Paints what was deferred, each in the clip it was asked for in.
pub fn flush_deferred(ui: &Ui) {
    let deferred = ui.ctx().data_mut(|d| {
        std::mem::take(d.get_temp_mut_or_default::<Vec<(Rect, Vec<Shape>)>>(deferred_id()))
    });
    for (clip, shapes) in deferred {
        ui.painter().with_clip_rect(clip).extend(shapes);
    }
}
