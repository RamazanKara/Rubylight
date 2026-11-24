//! Moonlit glass as meshes: rounded shapes with a feathered edge, rim gradients as ring meshes,
//! sheen as vertex colours, blurred shadows and focus halos as baked nine-slices, and the backdrop
//! sampled in screen space under the material's fill. Every shape here has circular corners and
//! is built in points. No per-frame CPU blur: textures are baked once and reused.

use std::sync::Arc;

use egui::epaint::{Mesh, PaintCallback, Vertex, WHITE_UV};
use egui::{
    pos2, vec2, Color32, Context, Id, Painter, Pos2, Rect, Shape, TextureHandle, TextureId, Ui,
    Vec2,
};

pub use crate::live_glass::{tier, Boundary, Tier, LIVE_BACKDROP};

use super::design_tokens as t;
use super::spring;
use crate::theme::{high_contrast, look, shade, Look};

/// A glass material of the spec.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Material {
    Chrome,
    Panel,
    Menu,
    Chip,
}

/// One material on one tier: the layers from the backdrop's brightness up to the rim.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Recipe {
    pub brightness: f32,
    pub fill: Color32,
    pub sheen_top: Color32,
    pub sheen_end: f32,
    pub rim_top: Color32,
    pub rim_mid: Color32,
    pub rim_bottom: Color32,
    pub rim_width: f32,
    pub shadow: Color32,
    pub shadow_y: f32,
    pub shadow_blur: f32,
    /// The day glass's thin dark edge, where its light rim would vanish on a pale page; none at night.
    pub hairline: Color32,
}

macro_rules! recipe {
    ($b:ident $f:ident $st:ident $se:ident $rt:ident $rm:ident $rb:ident $rw:ident $sc:ident $sy:ident $sb:ident) => {
        Recipe {
            brightness: t::$b,
            fill: t::$f,
            sheen_top: t::$st,
            sheen_end: t::$se,
            rim_top: t::$rt,
            rim_mid: t::$rm,
            rim_bottom: t::$rb,
            rim_width: t::$rw,
            shadow: t::$sc,
            shadow_y: t::$sy,
            shadow_blur: t::$sb,
            hairline: Color32::TRANSPARENT,
        }
    };
    ($b:ident $f:ident $st:ident $se:ident $rt:ident $rm:ident $rb:ident $rw:ident $sc:ident $sy:ident $sb:ident $h:ident) => {
        Recipe { hairline: t::$h, ..recipe!($b $f $st $se $rt $rm $rb $rw $sc $sy $sb) }
    };
}

/// `material` on `tier`, in the look in use: by day the day glass (Android's `GlassSpec` with day
/// tokens; the menus take the panel's), at night and on OLED black the night glass. Solid is the
/// same in every look: it is high contrast's.
pub fn recipe(material: Material, tier: Tier) -> Recipe {
    if look() == Look::Day && tier != Tier::Solid {
        return day_recipe(material, tier);
    }
    night_recipe(material, tier)
}

fn day_recipe(material: Material, tier: Tier) -> Recipe {
    match (material, tier) {
        (Material::Chrome, Tier::Live) => {
            recipe!(DAY_GLASS_CHROME_LIVE_BRIGHTNESS DAY_GLASS_CHROME_LIVE_FILL DAY_GLASS_CHROME_LIVE_SHEEN_TOP DAY_GLASS_CHROME_LIVE_SHEEN_END DAY_GLASS_CHROME_LIVE_RIM_TOP DAY_GLASS_CHROME_LIVE_RIM_MID DAY_GLASS_CHROME_LIVE_RIM_BOTTOM DAY_GLASS_CHROME_LIVE_RIM_WIDTH DAY_GLASS_CHROME_LIVE_SHADOW_COLOR DAY_GLASS_CHROME_LIVE_SHADOW_Y DAY_GLASS_CHROME_LIVE_SHADOW_BLUR DAY_GLASS_CHROME_LIVE_HAIRLINE)
        }
        (Material::Chrome, _) => {
            recipe!(DAY_GLASS_CHROME_BAKED_BRIGHTNESS DAY_GLASS_CHROME_BAKED_FILL DAY_GLASS_CHROME_BAKED_SHEEN_TOP DAY_GLASS_CHROME_BAKED_SHEEN_END DAY_GLASS_CHROME_BAKED_RIM_TOP DAY_GLASS_CHROME_BAKED_RIM_MID DAY_GLASS_CHROME_BAKED_RIM_BOTTOM DAY_GLASS_CHROME_BAKED_RIM_WIDTH DAY_GLASS_CHROME_BAKED_SHADOW_COLOR DAY_GLASS_CHROME_BAKED_SHADOW_Y DAY_GLASS_CHROME_BAKED_SHADOW_BLUR DAY_GLASS_CHROME_BAKED_HAIRLINE)
        }
        (Material::Panel | Material::Menu, Tier::Live) => {
            recipe!(DAY_GLASS_PANEL_LIVE_BRIGHTNESS DAY_GLASS_PANEL_LIVE_FILL DAY_GLASS_PANEL_LIVE_SHEEN_TOP DAY_GLASS_PANEL_LIVE_SHEEN_END DAY_GLASS_PANEL_LIVE_RIM_TOP DAY_GLASS_PANEL_LIVE_RIM_MID DAY_GLASS_PANEL_LIVE_RIM_BOTTOM DAY_GLASS_PANEL_LIVE_RIM_WIDTH DAY_GLASS_PANEL_LIVE_SHADOW_COLOR DAY_GLASS_PANEL_LIVE_SHADOW_Y DAY_GLASS_PANEL_LIVE_SHADOW_BLUR DAY_GLASS_PANEL_LIVE_HAIRLINE)
        }
        (Material::Panel | Material::Menu, _) => {
            recipe!(DAY_GLASS_PANEL_BAKED_BRIGHTNESS DAY_GLASS_PANEL_BAKED_FILL DAY_GLASS_PANEL_BAKED_SHEEN_TOP DAY_GLASS_PANEL_BAKED_SHEEN_END DAY_GLASS_PANEL_BAKED_RIM_TOP DAY_GLASS_PANEL_BAKED_RIM_MID DAY_GLASS_PANEL_BAKED_RIM_BOTTOM DAY_GLASS_PANEL_BAKED_RIM_WIDTH DAY_GLASS_PANEL_BAKED_SHADOW_COLOR DAY_GLASS_PANEL_BAKED_SHADOW_Y DAY_GLASS_PANEL_BAKED_SHADOW_BLUR DAY_GLASS_PANEL_BAKED_HAIRLINE)
        }
        (Material::Chip, Tier::Live) => {
            recipe!(DAY_GLASS_CHIP_LIVE_BRIGHTNESS DAY_GLASS_CHIP_LIVE_FILL DAY_GLASS_CHIP_LIVE_SHEEN_TOP DAY_GLASS_CHIP_LIVE_SHEEN_END DAY_GLASS_CHIP_LIVE_RIM_TOP DAY_GLASS_CHIP_LIVE_RIM_MID DAY_GLASS_CHIP_LIVE_RIM_BOTTOM DAY_GLASS_CHIP_LIVE_RIM_WIDTH DAY_GLASS_CHIP_LIVE_SHADOW_COLOR DAY_GLASS_CHIP_LIVE_SHADOW_Y DAY_GLASS_CHIP_LIVE_SHADOW_BLUR DAY_GLASS_CHIP_LIVE_HAIRLINE)
        }
        (Material::Chip, _) => {
            recipe!(DAY_GLASS_CHIP_BAKED_BRIGHTNESS DAY_GLASS_CHIP_BAKED_FILL DAY_GLASS_CHIP_BAKED_SHEEN_TOP DAY_GLASS_CHIP_BAKED_SHEEN_END DAY_GLASS_CHIP_BAKED_RIM_TOP DAY_GLASS_CHIP_BAKED_RIM_MID DAY_GLASS_CHIP_BAKED_RIM_BOTTOM DAY_GLASS_CHIP_BAKED_RIM_WIDTH DAY_GLASS_CHIP_BAKED_SHADOW_COLOR DAY_GLASS_CHIP_BAKED_SHADOW_Y DAY_GLASS_CHIP_BAKED_SHADOW_BLUR DAY_GLASS_CHIP_BAKED_HAIRLINE)
        }
    }
}

fn night_recipe(material: Material, tier: Tier) -> Recipe {
    match (material, tier) {
        (Material::Chrome, Tier::Live) => {
            recipe!(GLASS_CHROME_LIVE_BRIGHTNESS GLASS_CHROME_LIVE_FILL GLASS_CHROME_LIVE_SHEEN_TOP GLASS_CHROME_LIVE_SHEEN_END GLASS_CHROME_LIVE_RIM_TOP GLASS_CHROME_LIVE_RIM_MID GLASS_CHROME_LIVE_RIM_BOTTOM GLASS_CHROME_LIVE_RIM_WIDTH GLASS_CHROME_LIVE_SHADOW_COLOR GLASS_CHROME_LIVE_SHADOW_Y GLASS_CHROME_LIVE_SHADOW_BLUR)
        }
        (Material::Chrome, Tier::Baked) => {
            recipe!(GLASS_CHROME_BAKED_BRIGHTNESS GLASS_CHROME_BAKED_FILL GLASS_CHROME_BAKED_SHEEN_TOP GLASS_CHROME_BAKED_SHEEN_END GLASS_CHROME_BAKED_RIM_TOP GLASS_CHROME_BAKED_RIM_MID GLASS_CHROME_BAKED_RIM_BOTTOM GLASS_CHROME_BAKED_RIM_WIDTH GLASS_CHROME_BAKED_SHADOW_COLOR GLASS_CHROME_BAKED_SHADOW_Y GLASS_CHROME_BAKED_SHADOW_BLUR)
        }
        (Material::Chrome, Tier::Solid) => {
            recipe!(GLASS_CHROME_SOLID_BRIGHTNESS GLASS_CHROME_SOLID_FILL GLASS_CHROME_SOLID_SHEEN_TOP GLASS_CHROME_SOLID_SHEEN_END GLASS_CHROME_SOLID_RIM_TOP GLASS_CHROME_SOLID_RIM_MID GLASS_CHROME_SOLID_RIM_BOTTOM GLASS_CHROME_SOLID_RIM_WIDTH GLASS_CHROME_SOLID_SHADOW_COLOR GLASS_CHROME_SOLID_SHADOW_Y GLASS_CHROME_SOLID_SHADOW_BLUR)
        }
        (Material::Panel, Tier::Live) => {
            recipe!(GLASS_PANEL_LIVE_BRIGHTNESS GLASS_PANEL_LIVE_FILL GLASS_PANEL_LIVE_SHEEN_TOP GLASS_PANEL_LIVE_SHEEN_END GLASS_PANEL_LIVE_RIM_TOP GLASS_PANEL_LIVE_RIM_MID GLASS_PANEL_LIVE_RIM_BOTTOM GLASS_PANEL_LIVE_RIM_WIDTH GLASS_PANEL_LIVE_SHADOW_COLOR GLASS_PANEL_LIVE_SHADOW_Y GLASS_PANEL_LIVE_SHADOW_BLUR)
        }
        (Material::Panel, Tier::Baked) => {
            recipe!(GLASS_PANEL_BAKED_BRIGHTNESS GLASS_PANEL_BAKED_FILL GLASS_PANEL_BAKED_SHEEN_TOP GLASS_PANEL_BAKED_SHEEN_END GLASS_PANEL_BAKED_RIM_TOP GLASS_PANEL_BAKED_RIM_MID GLASS_PANEL_BAKED_RIM_BOTTOM GLASS_PANEL_BAKED_RIM_WIDTH GLASS_PANEL_BAKED_SHADOW_COLOR GLASS_PANEL_BAKED_SHADOW_Y GLASS_PANEL_BAKED_SHADOW_BLUR)
        }
        (Material::Panel, Tier::Solid) => {
            recipe!(GLASS_PANEL_SOLID_BRIGHTNESS GLASS_PANEL_SOLID_FILL GLASS_PANEL_SOLID_SHEEN_TOP GLASS_PANEL_SOLID_SHEEN_END GLASS_PANEL_SOLID_RIM_TOP GLASS_PANEL_SOLID_RIM_MID GLASS_PANEL_SOLID_RIM_BOTTOM GLASS_PANEL_SOLID_RIM_WIDTH GLASS_PANEL_SOLID_SHADOW_COLOR GLASS_PANEL_SOLID_SHADOW_Y GLASS_PANEL_SOLID_SHADOW_BLUR)
        }
        (Material::Menu, Tier::Live) => {
            recipe!(GLASS_MENU_LIVE_BRIGHTNESS GLASS_MENU_LIVE_FILL GLASS_MENU_LIVE_SHEEN_TOP GLASS_MENU_LIVE_SHEEN_END GLASS_MENU_LIVE_RIM_TOP GLASS_MENU_LIVE_RIM_MID GLASS_MENU_LIVE_RIM_BOTTOM GLASS_MENU_LIVE_RIM_WIDTH GLASS_MENU_LIVE_SHADOW_COLOR GLASS_MENU_LIVE_SHADOW_Y GLASS_MENU_LIVE_SHADOW_BLUR)
        }
        (Material::Menu, Tier::Baked) => {
            recipe!(GLASS_MENU_BAKED_BRIGHTNESS GLASS_MENU_BAKED_FILL GLASS_MENU_BAKED_SHEEN_TOP GLASS_MENU_BAKED_SHEEN_END GLASS_MENU_BAKED_RIM_TOP GLASS_MENU_BAKED_RIM_MID GLASS_MENU_BAKED_RIM_BOTTOM GLASS_MENU_BAKED_RIM_WIDTH GLASS_MENU_BAKED_SHADOW_COLOR GLASS_MENU_BAKED_SHADOW_Y GLASS_MENU_BAKED_SHADOW_BLUR)
        }
        (Material::Menu, Tier::Solid) => {
            recipe!(GLASS_MENU_SOLID_BRIGHTNESS GLASS_MENU_SOLID_FILL GLASS_MENU_SOLID_SHEEN_TOP GLASS_MENU_SOLID_SHEEN_END GLASS_MENU_SOLID_RIM_TOP GLASS_MENU_SOLID_RIM_MID GLASS_MENU_SOLID_RIM_BOTTOM GLASS_MENU_SOLID_RIM_WIDTH GLASS_MENU_SOLID_SHADOW_COLOR GLASS_MENU_SOLID_SHADOW_Y GLASS_MENU_SOLID_SHADOW_BLUR)
        }
        (Material::Chip, Tier::Live) => {
            recipe!(GLASS_CHIP_LIVE_BRIGHTNESS GLASS_CHIP_LIVE_FILL GLASS_CHIP_LIVE_SHEEN_TOP GLASS_CHIP_LIVE_SHEEN_END GLASS_CHIP_LIVE_RIM_TOP GLASS_CHIP_LIVE_RIM_MID GLASS_CHIP_LIVE_RIM_BOTTOM GLASS_CHIP_LIVE_RIM_WIDTH GLASS_CHIP_LIVE_SHADOW_COLOR GLASS_CHIP_LIVE_SHADOW_Y GLASS_CHIP_LIVE_SHADOW_BLUR)
        }
        (Material::Chip, Tier::Baked) => {
            recipe!(GLASS_CHIP_BAKED_BRIGHTNESS GLASS_CHIP_BAKED_FILL GLASS_CHIP_BAKED_SHEEN_TOP GLASS_CHIP_BAKED_SHEEN_END GLASS_CHIP_BAKED_RIM_TOP GLASS_CHIP_BAKED_RIM_MID GLASS_CHIP_BAKED_RIM_BOTTOM GLASS_CHIP_BAKED_RIM_WIDTH GLASS_CHIP_BAKED_SHADOW_COLOR GLASS_CHIP_BAKED_SHADOW_Y GLASS_CHIP_BAKED_SHADOW_BLUR)
        }
        (Material::Chip, Tier::Solid) => {
            recipe!(GLASS_CHIP_SOLID_BRIGHTNESS GLASS_CHIP_SOLID_FILL GLASS_CHIP_SOLID_SHEEN_TOP GLASS_CHIP_SOLID_SHEEN_END GLASS_CHIP_SOLID_RIM_TOP GLASS_CHIP_SOLID_RIM_MID GLASS_CHIP_SOLID_RIM_BOTTOM GLASS_CHIP_SOLID_RIM_WIDTH GLASS_CHIP_SOLID_SHADOW_COLOR GLASS_CHIP_SOLID_SHADOW_Y GLASS_CHIP_SOLID_SHADOW_BLUR)
        }
    }
}

/// One layer of what baked glass samples: a texture covering `area` of the screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layer {
    pub texture: TextureId,
    pub alpha: f32,
    pub area: Rect,
}

/// What lies behind chrome on the baked tier: the ambient, cross-fading from `base` to `top`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Backdrop {
    pub base: Option<Layer>,
    pub top: Option<Layer>,
}

/// The ambient cross-fade baked glass samples and the backdrop paints: show a texture (`Host.ambient`)
/// and take this frame's [`Backdrop`]. Frames are asked for only while it runs.
#[derive(Default)]
pub struct CrossFade {
    current: Option<(u64, TextureHandle)>,
    previous: Option<(u64, TextureHandle)>,
    started: f64,
}

impl CrossFade {
    /// Fades to `texture`, unless `key` already shows.
    pub fn show(&mut self, ctx: &Context, key: u64, texture: TextureHandle) {
        if self.current.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        let now = ctx.input(|i| i.time);
        // Back to what it fades from, while it runs: it turns where it stands, not from the full.
        if self.previous.as_ref().is_some_and(|(k, _)| *k == key) && self.running(ctx) {
            let back = 1.0 - self.along(ctx);
            std::mem::swap(&mut self.previous, &mut self.current);
            self.started = now - f64::from(back) * f64::from(ambient_ms(ctx)) / 1000.0;
            return;
        }
        // A second change in the frame the fade started changes its mind: the fade still runs from
        // what showed, not from what never did.
        if self.current.is_none() || self.started != now {
            self.previous = self.current.take();
        }
        self.current = Some((key, texture));
        self.started = now;
    }

    /// Puts `texture` in the place of what fades in, the fade going on as it was.
    pub fn swap(&mut self, key: u64, texture: TextureHandle) {
        self.current = Some((key, texture));
    }

    /// Whether a fade still runs.
    pub fn running(&self, ctx: &Context) -> bool {
        self.along(ctx) < 1.0
    }

    fn along(&self, ctx: &Context) -> f32 {
        let ms = ambient_ms(ctx);
        if ms == 0 {
            1.0
        } else {
            let elapsed = (ctx.input(|i| i.time) - self.started) as f32 * 1000.0;
            (elapsed / ms as f32).clamp(0.0, 1.0)
        }
    }

    /// The layers [`Self::backdrop`] handed out this frame.
    pub fn layers(&self, ctx: &Context) -> Backdrop {
        backdrop(ctx)
    }

    /// This frame's layers, covering `area` of the screen; also hands them to glass.
    pub fn backdrop(&mut self, ctx: &Context, area: Rect) -> Backdrop {
        let along = self.along(ctx);
        if along < 1.0 {
            ctx.request_repaint();
        } else {
            self.previous = None;
        }
        let layer = |texture: &TextureHandle, alpha| Layer {
            texture: texture.id(),
            alpha,
            area,
        };
        let backdrop = Backdrop {
            base: self.previous.as_ref().map(|(_, p)| layer(p, 1.0)),
            top: self.current.as_ref().map(|(_, c)| layer(c, along)),
        };
        set_backdrop(ctx, backdrop);
        backdrop
    }
}

/// How long the ambient takes to fade, none with reduced motion.
pub fn ambient_ms(ctx: &Context) -> u32 {
    if spring::motion(ctx) {
        t::MOTION_AMBIENT_MS
    } else {
        t::REDUCED_AMBIENT_MS
    }
}

fn backdrop_id() -> Id {
    Id::unique("nightjar-glass-backdrop")
}

pub fn set_backdrop(ctx: &Context, backdrop: Backdrop) {
    ctx.data_mut(|d| d.insert_temp(backdrop_id(), backdrop));
}

/// The layers handed out this frame, for a test to look at.
#[cfg(test)]
pub fn backdrop_of(ctx: &Context) -> Backdrop {
    backdrop(ctx)
}

fn backdrop(ctx: &Context) -> Backdrop {
    ctx.data(|d| d.get_temp(backdrop_id())).unwrap_or_default()
}

/// The boundary between content and chrome, on the live tier: everything painted before it is what
/// the chain blurs. No shape at all on the other tiers.
pub fn boundary(painter: &Painter, regions: Vec<Rect>) {
    if tier(painter.ctx()) == Tier::Live {
        let screen = painter.ctx().content_rect();
        painter.add(Shape::Callback(PaintCallback {
            rect: screen,
            callback: Arc::new(Boundary { regions }),
        }));
    }
}

fn page_slot_id() -> Id {
    Id::unique("nightjar-glass-page-slot")
}

/// The place [`keep_page_slot`] kept: the pass it is for, its layer and its index there.
type PageSlot = Option<(u64, egui::LayerId, egui::layers::ShapeIdx)>;

/// Keeps a place right after the boundary, under the chrome `painter` paints after it, for the
/// blurred page a dialog asks for ([`page_behind`]). Only on the live tier; for this pass only.
pub fn keep_page_slot(painter: &Painter) {
    let ctx = painter.ctx();
    let slot: PageSlot = (tier(ctx) == Tier::Live)
        .then(|| (ctx.cumulative_pass_nr(), painter.layer_id(), painter.add(Shape::Noop)));
    ctx.data_mut(|d| d.insert_temp(page_slot_id(), slot));
}

/// The page behind a dialog, blurred as the panel's glass sees it (Android's `PageBlur`): the
/// live chain's output over the whole screen, which the chain already blurs whole while a dialog
/// is up (`Launcher::panel_is_up`), so this costs one textured quad and no pass of its own. It
/// goes into the place the chrome kept under itself ([`keep_page_slot`]): past the boundary, so
/// the chain never blurs it again, and under the bar, which stays itself above it, both under the
/// dialog's dim. `opacity`: the dialog's own fade. Only on the live tier, which is never over a
/// stream's video, and only in a pass that kept the place. True when it drew.
pub fn page_behind(ctx: &Context, opacity: f32) -> bool {
    if tier(ctx) != Tier::Live {
        return false;
    }
    let slot = ctx
        .data(|d| d.get_temp::<PageSlot>(page_slot_id()))
        .flatten();
    let Some((_, layer, idx)) = slot.filter(|(pass, ..)| *pass == ctx.cumulative_pass_nr()) else {
        return false;
    };
    let screen = ctx.content_rect();
    let mut out = Vec::new();
    image(
        &mut out,
        ctx,
        screen,
        0.0,
        LIVE_BACKDROP,
        uv_in(screen, screen),
        Color32::WHITE.gamma_multiply(opacity),
    );
    ctx.layer_painter(layer).set(idx, Shape::Vec(out));
    true
}

fn ppp(ctx: &Context) -> f32 {
    ctx.pixels_per_point().max(0.25)
}

/// A colour times an alpha, premultiplied as egui's are.
pub fn scaled(color: Color32, by: f32) -> Color32 {
    color.gamma_multiply(by.clamp(0.0, 1.0))
}

fn mix(a: Color32, b: Color32, along: f32) -> Color32 {
    let along = along.clamp(0.0, 1.0);
    let channel =
        |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * along).round() as u8;
    Color32::from_rgba_premultiplied(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
        channel(a.a(), b.a()),
    )
}

/// A CSS linear gradient over `rect`: `degrees` as CSS counts them, `stops` as (place, colour).
pub fn gradient(rect: Rect, degrees: f32, stops: &[(f32, Color32)]) -> impl Fn(Pos2) -> Color32 + use<> {
    let (dx, dy) = (degrees.to_radians().sin(), -degrees.to_radians().cos());
    let span = (dx.abs() * rect.width() + dy.abs() * rect.height()).max(1e-3);
    let center = rect.center();
    let stops = stops.to_vec();
    move |p| {
        let along = ((p.x - center.x) * dx + (p.y - center.y) * dy) / span + 0.5;
        let Some(first) = stops.first() else {
            return Color32::TRANSPARENT;
        };
        if along <= first.0 {
            return first.1;
        }
        for pair in stops.windows(2) {
            if along <= pair[1].0 {
                let width = (pair[1].0 - pair[0].0).max(1e-5);
                return mix(pair[0].1, pair[1].1, (along - pair[0].0) / width);
            }
        }
        stops[stops.len() - 1].1
    }
}

/// A point of a rounded outline: the corner's centre (or a place on a straight edge's inner line)
/// and the outward direction. The outline at offset `d` is `c + n * (r + d)`.
#[derive(Clone, Copy)]
struct Node {
    c: Pos2,
    n: Vec2,
}

fn corner_steps(r: f32, ppp: f32) -> usize {
    ((r * ppp).sqrt() * 1.8).ceil().clamp(2.0, 28.0) as usize
}

fn fit_radius(rect: Rect, r: f32) -> f32 {
    r.min(rect.width() / 2.0).min(rect.height() / 2.0).max(0.0)
}

/// The left side of a rounded rectangle, top to bottom.
fn left_side(rect: Rect, r: f32, ppp: f32) -> Vec<Node> {
    let steps = corner_steps(r, ppp);
    let (top, bottom) = (
        pos2(rect.left() + r, rect.top() + r),
        pos2(rect.left() + r, rect.bottom() - r),
    );
    let mut nodes = Vec::with_capacity(2 * steps + 2);
    for i in 0..=steps {
        let a = i as f32 / steps as f32 * std::f32::consts::FRAC_PI_2;
        nodes.push(Node {
            c: top,
            n: vec2(-a.sin(), -a.cos()),
        });
    }
    for i in 0..=steps {
        let a = i as f32 / steps as f32 * std::f32::consts::FRAC_PI_2;
        nodes.push(Node {
            c: bottom,
            n: vec2(-a.cos(), a.sin()),
        });
    }
    nodes
}

/// A filled rounded shape with a feathered edge. Rows run left to right, so vertex colours follow
/// `color` down the shape; `stops` (fractions of the height) add rows where a gradient bends.
fn fill_mesh(
    mesh: &mut Mesh,
    rect: Rect,
    r: f32,
    stops: &[f32],
    ppp: f32,
    color: &dyn Fn(Pos2) -> Color32,
    uv: &dyn Fn(Pos2) -> Pos2,
) {
    let r = fit_radius(rect, r);
    let feather = 1.0 / ppp;
    let mut rows: Vec<(Pos2, Vec2)> = left_side(rect, r, ppp)
        .iter()
        .map(|n| (n.c + n.n * (r - feather / 2.0), n.n))
        .collect();
    for stop in stops {
        let y = rect.top() + rect.height() * stop;
        if let Some(at) = rows.windows(2).position(|w| w[0].0.y < y && y < w[1].0.y) {
            let (a, b) = (rows[at], rows[at + 1]);
            let along = (y - a.0.y) / (b.0.y - a.0.y);
            let n = (a.1 + (b.1 - a.1) * along).normalized();
            rows.insert(at + 1, (a.0 + (b.0 - a.0) * along, n));
        }
    }
    let center = rect.center().x;
    let base = mesh.vertices.len() as u32;
    let mirror = |p: Pos2| pos2(2.0 * center - p.x, p.y);
    let flip = |n: Vec2| vec2(-n.x, n.y);
    for (p, n) in &rows {
        let right = mirror(*p);
        mesh.vertices.push(Vertex {
            pos: *p,
            uv: uv(*p),
            color: color(*p),
        });
        mesh.vertices.push(Vertex {
            pos: right,
            uv: uv(right),
            color: color(right),
        });
        let outer = *p + *n * feather;
        let outer_right = right + flip(*n) * feather;
        mesh.vertices.push(Vertex {
            pos: outer,
            uv: uv(outer),
            color: Color32::TRANSPARENT,
        });
        mesh.vertices.push(Vertex {
            pos: outer_right,
            uv: uv(outer_right),
            color: Color32::TRANSPARENT,
        });
    }
    for i in 0..rows.len() as u32 - 1 {
        let (a, b) = (base + i * 4, base + (i + 1) * 4);
        // The row, then the feather left and right of it.
        mesh.indices.extend([a, a + 1, b + 1, a, b + 1, b]);
        mesh.indices.extend([a + 2, a, b, a + 2, b, b + 2]);
        mesh.indices
            .extend([a + 1, a + 3, b + 3, a + 1, b + 3, b + 1]);
    }
    // The feather along the top and the bottom edge.
    let (top, bottom) = (base, base + (rows.len() as u32 - 1) * 4);
    mesh.indices
        .extend([top + 2, top + 3, top + 1, top + 2, top + 1, top]);
    mesh.indices.extend([
        bottom,
        bottom + 1,
        bottom + 3,
        bottom,
        bottom + 3,
        bottom + 2,
    ]);
}

/// The whole outline clockwise; the long straight edges are divided so a slanted gradient bends
/// where it should along them.
fn outline(rect: Rect, r: f32, ppp: f32) -> Vec<Node> {
    let steps = corner_steps(r, ppp);
    let inner = rect.shrink(r);
    let mut nodes = Vec::new();
    let mut arc = |c: Pos2, from: f32| {
        for i in 0..=steps {
            let a = from + i as f32 / steps as f32 * std::f32::consts::FRAC_PI_2;
            nodes.push(Node {
                c,
                n: vec2(a.cos(), a.sin()),
            });
        }
    };
    arc(inner.left_top(), std::f32::consts::PI);
    arc(inner.right_top(), 1.5 * std::f32::consts::PI);
    arc(inner.right_bottom(), 0.0);
    arc(inner.left_bottom(), std::f32::consts::FRAC_PI_2);
    // Divide what lies between corners.
    let mut out = Vec::with_capacity(nodes.len() + 32);
    for (i, node) in nodes.iter().enumerate() {
        out.push(*node);
        let next = nodes[(i + 1) % nodes.len()];
        let gap = next.c - node.c;
        if gap.length() > 1.0 {
            let parts = (gap.length() / 32.0).ceil().clamp(1.0, 64.0) as usize;
            for part in 1..parts {
                out.push(Node {
                    c: node.c + gap * (part as f32 / parts as f32),
                    n: node.n,
                });
            }
        }
    }
    out
}

/// A ring just inside `rect`, `width` wide, feathered both ways.
fn ring_mesh(
    mesh: &mut Mesh,
    rect: Rect,
    r: f32,
    width: f32,
    ppp: f32,
    color: &dyn Fn(Pos2) -> Color32,
) {
    let r = fit_radius(rect, r);
    let feather = 1.0 / ppp;
    let nodes = outline(rect, r, ppp);
    let at = |node: &Node, d: f32| node.c + node.n * (r + d);
    let (d1, d2) = if width <= feather {
        (-width / 2.0, -width / 2.0)
    } else {
        (-feather / 2.0, -width + feather / 2.0)
    };
    let strength = (width / feather).min(1.0);
    let base = mesh.vertices.len() as u32;
    for node in &nodes {
        let middle = at(node, -width / 2.0);
        let c = scaled(color(middle), strength);
        for (d, shade) in [
            (feather / 2.0, Color32::TRANSPARENT),
            (d1, c),
            (d2, c),
            (-width - feather / 2.0, Color32::TRANSPARENT),
        ] {
            mesh.vertices.push(Vertex {
                pos: at(node, d),
                uv: WHITE_UV,
                color: shade,
            });
        }
    }
    let count = nodes.len() as u32;
    for i in 0..count {
        let (a, b) = (base + i * 4, base + (i + 1) % count * 4);
        for k in 0..3 {
            mesh.indices
                .extend([a + k, b + k, b + k + 1, a + k, b + k + 1, a + k + 1]);
        }
    }
}

/// `rect` around its centre, scaled.
pub fn scaled_rect(rect: Rect, scale: f32) -> Rect {
    Rect::from_center_size(rect.center(), rect.size() * scale)
}

/// `rect` scaled up from the middle of its foot.
pub fn risen_rect(rect: Rect, scale: f32) -> Rect {
    let size = rect.size() * scale;
    Rect::from_min_size(pos2(rect.center().x - size.x / 2.0, rect.bottom() - size.y), size)
}

/// A flat rounded fill.
pub fn solid(out: &mut Vec<Shape>, ctx: &Context, rect: Rect, r: f32, color: Color32) {
    let mut mesh = Mesh::default();
    fill_mesh(&mut mesh, rect, r, &[], ppp(ctx), &|_| color, &|_| WHITE_UV);
    out.push(Shape::mesh(mesh));
}

/// A rounded fill whose colour follows `color`; `stops` are where it bends, as fractions of the height.
pub fn shaded(
    out: &mut Vec<Shape>,
    ctx: &Context,
    rect: Rect,
    r: f32,
    stops: &[f32],
    color: &dyn Fn(Pos2) -> Color32,
) {
    let mut mesh = Mesh::default();
    fill_mesh(&mut mesh, rect, r, stops, ppp(ctx), color, &|_| WHITE_UV);
    out.push(Shape::mesh(mesh));
}

/// A rounded picture: `uv` of `texture` fills `rect`, multiplied by `tint`.
pub fn image(
    out: &mut Vec<Shape>,
    ctx: &Context,
    rect: Rect,
    r: f32,
    texture: TextureId,
    uv: Rect,
    tint: Color32,
) {
    let mut mesh = Mesh::with_texture(texture);
    let map = |p: Pos2| {
        pos2(
            uv.min.x + (p.x - rect.min.x) / rect.width().max(1e-3) * uv.width(),
            uv.min.y + (p.y - rect.min.y) / rect.height().max(1e-3) * uv.height(),
        )
    };
    fill_mesh(&mut mesh, rect, r, &[], ppp(ctx), &|_| tint, &map);
    out.push(Shape::mesh(mesh));
}

/// A rim following `color`.
pub fn rim(
    out: &mut Vec<Shape>,
    ctx: &Context,
    rect: Rect,
    r: f32,
    width: f32,
    color: &dyn Fn(Pos2) -> Color32,
) {
    let mut mesh = Mesh::default();
    ring_mesh(&mut mesh, rect, r, width, ppp(ctx), color);
    out.push(Shape::mesh(mesh));
}

mod surfaces;

pub use surfaces::{
    control, cover_finish, cover_glow, defer, defer_focused, deferred, feel, flush_deferred,
    focus, forget_soft, group, hud_recipe, hud_surface, menu_surface, on_play, play, soft,
    soft_around, soft_textures, surface, tinted, with_halo_reach, without_backdrop, Feel,
    SoftTextures,
};
use surfaces::uv_in;
#[cfg(test)]
pub use surfaces::take_deferred;
#[cfg(test)]
use surfaces::{bake_soft, deferred_id};

#[cfg(test)]
mod tests;
