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
use crate::theme::high_contrast;

/// A glass material of the spec.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code, reason = "menus and chips take their glass when their pages are redesigned")]
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
        }
    };
}

pub fn recipe(material: Material, tier: Tier) -> Recipe {
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
    previous: Option<TextureHandle>,
    started: f64,
}

impl CrossFade {
    /// Fades to `texture`, unless `key` already shows.
    pub fn show(&mut self, ctx: &Context, key: u64, texture: TextureHandle) {
        if self.current.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        self.previous = self.current.take().map(|(_, t)| t);
        self.current = Some((key, texture));
        self.started = ctx.input(|i| i.time);
    }

    /// This frame's layers, covering `area` of the screen; also hands them to glass.
    pub fn backdrop(&mut self, ctx: &Context, area: Rect) -> Backdrop {
        let ms = if spring::motion(ctx) {
            t::MOTION_AMBIENT_MS
        } else {
            t::REDUCED_AMBIENT_MS
        };
        let along = if ms == 0 {
            1.0
        } else {
            let elapsed = (ctx.input(|i| i.time) - self.started) as f32 * 1000.0;
            (elapsed / ms as f32).clamp(0.0, 1.0)
        };
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
            base: self.previous.as_ref().map(|p| layer(p, 1.0)),
            top: self.current.as_ref().map(|(_, c)| layer(c, along)),
        };
        set_backdrop(ctx, backdrop);
        backdrop
    }
}

fn backdrop_id() -> Id {
    Id::unique("nightjar-glass-backdrop")
}

pub fn set_backdrop(ctx: &Context, backdrop: Backdrop) {
    ctx.data_mut(|d| d.insert_temp(backdrop_id(), backdrop));
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
pub fn gradient(rect: Rect, degrees: f32, stops: &[(f32, Color32)]) -> impl Fn(Pos2) -> Color32 {
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

/// The alpha of a rounded rectangle under a Gaussian, `blur` as CSS means it (sigma is half).
fn bake_soft(size: Vec2, r: f32, blur: f32, hollow: bool) -> egui::ColorImage {
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
    let texture = ctx
        .data(|d| d.get_temp::<egui::TextureHandle>(id))
        .unwrap_or_else(|| {
            let handle = ctx.load_texture(
                "glass-soft",
                bake_soft(baked, r.min(baked.x / 2.0).min(baked.y / 2.0), blur, hollow),
                egui::TextureOptions::LINEAR,
            );
            ctx.data_mut(|d| d.insert_temp(id, handle.clone()));
            handle
        });
    let pad = blur;
    let xs = [
        rect.left() - pad,
        rect.left() + baked.x / 2.0,
        rect.right() - baked.x / 2.0,
        rect.right() + pad,
    ];
    let ys = [
        rect.top() - pad,
        rect.top() + baked.y / 2.0,
        rect.bottom() - baked.y / 2.0,
        rect.bottom() + pad,
    ];
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

/// The focus language of every page: a dark gap, a thick ring, a warm halo behind them. High
/// contrast: a thicker ring, no halo. `on_moon`: the paper ring that reads against the Play button.
/// The halo stays outside the rectangle, so it goes before or after the focused thing's own shapes.
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
            t::FOCUS_RING_COLOR_ON_MOON,
            t::FOCUS_RING_WIDTH,
            t::FOCUS_GAP,
        )
    } else {
        (t::FOCUS_RING_COLOR, t::FOCUS_RING_WIDTH, t::FOCUS_GAP)
    };
    if !high {
        let spread = gap + width;
        soft_around(
            out,
            ctx,
            rect.expand(spread),
            r + spread,
            t::FOCUS_HALO_BLUR,
            t::FOCUS_HALO_COLOR,
        );
    }
    let gap_rect = rect.expand(gap);
    rim(out, ctx, gap_rect, r + gap, gap, &|_| t::FOCUS_GAP_COLOR);
    let ring_rect = rect.expand(gap + width);
    rim(out, ctx, ring_rect, r + gap + width, width, &|_| ring);
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
                scale_rgb(t::COLOR_CANVAS, recipe.brightness),
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
}

fn scale_rgb(color: Color32, by: f32) -> Color32 {
    let channel = |v: u8| (f32::from(v) * by).round() as u8;
    Color32::from_rgba_premultiplied(
        channel(color.r()),
        channel(color.g()),
        channel(color.b()),
        color.a(),
    )
}

/// Where `rect` lies inside `area`, as a uv rectangle: glass samples the screen behind it.
fn uv_in(area: Rect, rect: Rect) -> Rect {
    let at = |p: Pos2| {
        pos2(
            (p.x - area.min.x) / area.width().max(1e-3),
            (p.y - area.min.y) / area.height().max(1e-3),
        )
    };
    Rect::from_min_max(at(rect.min), at(rect.max))
}

/// A flat tint inside glass or on art: the rim of a group. No blur, no nesting.
pub fn group(out: &mut Vec<Shape>, ctx: &Context, rect: Rect, r: f32, fill: Color32) {
    if high_contrast() {
        solid(out, ctx, rect, r, t::SOLID_GROUP_FILL);
        rim(out, ctx, rect, r, 1.5, &|_| t::HC_COLOR_RIM);
        return;
    }
    solid(out, ctx, rect, r, fill);
    let color = gradient(
        rect,
        168.0,
        &[
            (0.0, t::TINT_GROUP_RIM_TOP),
            (0.4, t::TINT_GROUP_RIM_MID),
            (1.0, t::TINT_GROUP_RIM_BOTTOM),
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
    solid(out, ctx, rect, r, t::TINT_CONTROL_FILL);
    let color = gradient(
        rect,
        168.0,
        &[
            (0.0, t::TINT_CONTROL_RIM_TOP),
            (0.4, t::TINT_CONTROL_RIM_MID),
            (1.0, t::TINT_CONTROL_RIM_BOTTOM),
        ],
    );
    rim(out, ctx, rect, r, 1.0, &color);
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
        t::PLAY_GLOW,
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
            (1.0, t::PLAY_RIM_BOTTOM),
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
    let boost = |c: Color32| {
        let a = (f32::from(c.a()) * hover_boost).min(255.0) as u8;
        Color32::from_rgba_premultiplied(a, a, a, a)
    };
    let color = gradient(
        rect,
        168.0,
        &[
            (0.0, boost(t::COVER_RIM_TOP)),
            (0.3, boost(t::COVER_RIM_BOTTOM)),
            (1.0, boost(t::COVER_RIM_BOTTOM)),
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

fn deferred_id() -> Id {
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

/// Paints what was deferred, each in the clip it was asked for in.
pub fn flush_deferred(ui: &Ui) {
    let deferred = ui.ctx().data_mut(|d| {
        std::mem::take(d.get_temp_mut_or_default::<Vec<(Rect, Vec<Shape>)>>(deferred_id()))
    });
    for (clip, shapes) in deferred {
        ui.painter().with_clip_rect(clip).extend(shapes);
    }
}

#[cfg(test)]
mod tests {
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
}
