//! SDL renderer triangle mesh and texture draw for Frame Warp (flat and lens warps).

use nightjar_core::frame_warp::{edge, edge_axis, source, EDGE_RAMP, FLAT, MIN_DEPTH};
use sdl3_sys::everything::*;

use super::WarpDraw;

/// A corner of the triangles the SDL renderer path draws: where it is on the destination and
/// where it samples the picture (fractions of each), and how bright it is (the edge fill's fade).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Corner {
    pub at: [f32; 2],
    pub uv: [f32; 2],
    pub fade: f32,
}

/// Where one axis of the destination is cut for the shift `s`: its ends, and each point between
/// where the edge fill bends (the picture's edges, a whole picture past them, and the end of the
/// darkening), so that sampling and darkening are linear between cuts. Returns the cuts in order.
fn edge_cuts(s: f32) -> ([f32; 8], usize) {
    let mut cuts = [0.0f32; 8];
    cuts[1] = 1.0;
    let mut count = 2;
    for bend in [-1.0, -EDGE_RAMP, 0.0, 1.0, 1.0 + EDGE_RAMP, 2.0] {
        let at = s + bend;
        if at > 0.0 && at < 1.0 {
            cuts[count] = at;
            count += 1;
        }
    }
    cuts[..count].sort_by(f32::total_cmp);
    let sorted = cuts;
    let mut kept = 1;
    for &at in &sorted[1..count] {
        if at > cuts[kept - 1] {
            cuts[kept] = at;
            kept += 1;
        }
    }
    (cuts, kept)
}

/// The stretches of one destination axis, under the shift `s` on it, whose point samples inside
/// `a..b` of the picture with the edge fill (`edge_axis`): `a..b` itself and its mirrors past
/// either edge, each moved by the shift and cut to the destination. A stretch that reaches an edge
/// reaches out all the way past it, since the fill holds that edge past a whole picture.
fn edge_sources(s: f32, a: f32, b: f32) -> [Option<(f32, f32)>; 3] {
    edge_ranges(a, b).map(|(lo, hi)| {
        let (lo, hi) = ((lo + s).max(0.0), (hi + s).min(1.0));
        (lo < hi).then_some((lo, hi))
    })
}

/// The stretches of one axis, before the edge fill, that sample inside `a..b` of the picture with
/// it (`edge_axis`): `a..b` itself and its mirrors past either edge. A stretch that reaches an
/// edge reaches out all the way past it, since the fill holds that edge past a whole picture.
fn edge_ranges(a: f32, b: f32) -> [(f32, f32); 3] {
    let below = if b >= 1.0 { f32::NEG_INFINITY } else { -b };
    let above = if a <= 0.0 { f32::INFINITY } else { 2.0 - a };
    [(a, b), (below, -a), (2.0 - b, above)]
}

/// Where one axis of the picture bends under the edge fill (`edge_cuts`), as what it samples.
const EDGE_BENDS: [f32; 6] = [-1.0, -EDGE_RAMP, 0.0, 1.0, 1.0 + EDGE_RAMP, 2.0];
/// How far off, in fractions of the picture, the SDL renderer path may draw a point of a picture
/// turned through a lens: a turn bends the mapping and each triangle draws it straight, so the
/// path draws as many cells as keep it this close (`lens_cells`). A fifth of a pixel of 4K.
pub(super) const LENS_ERROR: f32 = 5e-5;
/// The most cells a side it draws: a turn at the limit of both axes through a wide lens bends
/// its stretched corner more than this many keep within `LENS_ERROR`, by half a pixel of 1080p.
pub(super) const MAX_LENS_CELLS: usize = 96;
/// The most corners of a piece of a lens cell: its four, and one for each line that cuts it.
const PIECE: usize = 20;

/// `a x + b y + c` over the destination, in fractions of it.
type Form = [f32; 3];

fn form_at(form: Form, at: [f32; 2]) -> f32 {
    form[0] * at[0] + form[1] * at[1] + form[2]
}

/// A convex polygon on the destination, in fractions of it.
#[derive(Clone, Copy)]
struct Piece {
    at: [[f32; 2]; PIECE],
    len: usize,
}

impl Piece {
    fn rect([x0, y0, x1, y1]: [f32; 4]) -> Self {
        let mut at = [[0.0; 2]; PIECE];
        at[..4].copy_from_slice(&[[x0, y0], [x1, y0], [x1, y1], [x0, y1]]);
        Self { at, len: 4 }
    }

    fn corners(&self) -> &[[f32; 2]] {
        &self.at[..self.len]
    }

    /// The part on which `sign * form` is not negative.
    fn cut(&self, form: Form, sign: f32) -> Self {
        let mut out = Self { at: self.at, len: 0 };
        let mut push = |at: [f32; 2]| {
            if out.len < PIECE {
                out.at[out.len] = at;
                out.len += 1;
            }
        };
        for (i, &a) in self.corners().iter().enumerate() {
            let b = self.at[(i + 1) % self.len];
            let (va, vb) = (sign * form_at(form, a), sign * form_at(form, b));
            if va >= 0.0 {
                push(a);
            }
            if (va < 0.0) != (vb < 0.0) && va != vb {
                let t = va / (va - vb);
                push([a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]);
            }
        }
        out
    }
}

/// The most a picture turned through `lens` under `shift` bends: a bound on the second derivative
/// of `source` along any line across the destination.
pub(super) fn lens_bend(shift: [f32; 2], lens: [f32; 4]) -> f32 {
    // source = 0.5 + n / d with n = q * lens.zw - shift and d = 1 + dot(q, lens.xy); its second
    // derivative along a line is at most 2 k (|lens.zw| + |n| k / d) / d².
    let k = lens[0].abs() + lens[1].abs();
    let d = (1.0 - 0.5 * k).max(MIN_DEPTH);
    let scale = lens[2].abs().max(lens[3].abs());
    let n = 0.5 * scale + shift[0].abs().max(shift[1].abs());
    2.0 * k * (scale + n * k / d) / (d * d)
}

/// How many cells a side draw a picture turned through `lens` under `shift` within `LENS_ERROR`
/// of the mapping, from how much it bends at most (`lens_bend`): 4 to `MAX_LENS_CELLS`.
pub(super) fn lens_cells(shift: [f32; 2], lens: [f32; 4]) -> usize {
    // A chord of length h is off by bend h² / 8 at most.
    let cells = (lens_bend(shift, lens) / (8.0 * LENS_ERROR)).sqrt().ceil();
    if cells.is_finite() { (cells as usize).clamp(4, MAX_LENS_CELLS) } else { MAX_LENS_CELLS }
}

/// The triangles that draw a shifted picture on the SDL renderer path, and the buffers they are
/// made in: the presenter keeps one, so a shifted present allocates nothing once they have grown.
#[derive(Default)]
pub struct Mesh {
    pub corners: Vec<Corner>,
    /// Each triangle's three corners.
    pub indices: Vec<i32>,
    pub(super) vertices: Vec<SDL_Vertex>,
}

impl std::fmt::Debug for Mesh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mesh").field("corners", &self.corners.len()).field("triangles", &(self.indices.len() / 3)).finish()
    }
}

impl Mesh {
    /// The picture shifted by `shift` (fractions of the picture) through `lens`, with the edge
    /// fill where the shift uncovers the outside; over it the picture unshifted wherever the
    /// shifted one would show the HUD `areas` a second time, beside themselves; then the `areas` in
    /// place. Cut where the fill bends (`edge_cuts`), and where both axes darken along the line on
    /// which they darken alike, each triangle of a flat picture samples and darkens linearly:
    /// exactly what the shaders do per pixel. A lens bends the mapping, which is drawn as cells
    /// small enough to be within `LENS_ERROR` of it (`lens`).
    pub fn build(&mut self, shift: [f32; 2], lens: [f32; 4], areas: &[[f32; 4]]) {
        self.corners.clear();
        self.indices.clear();
        if lens != FLAT {
            return self.lens(shift, lens, areas);
        }
        let (xs, columns) = edge_cuts(shift[0]);
        let (ys, rows) = edge_cuts(shift[1]);
        for y in ys[..rows].windows(2) {
            for x in xs[..columns].windows(2) {
                self.cell(shift, [x[0], y[0], x[1], y[1]]);
            }
        }
        // The shaders sample a point itself where its shifted source is on the HUD
        // (`frameWarpShifted`): each area moved by the shift, and its mirrors in the edge fill. The
        // edge fill acts on each axis alone, so these are exact; only a mask finer than
        // `MAX_HUD_RECTS` covers a little more, as its areas do.
        for area in areas {
            let across = edge_sources(shift[0], area[0], area[2]);
            let down = edge_sources(shift[1], area[1], area[3]);
            for &(y0, y1) in down.iter().flatten() {
                for &(x0, x1) in across.iter().flatten() {
                    self.in_place([x0, y0, x1, y1]);
                }
            }
        }
        for area in areas {
            self.in_place(*area);
        }
    }

    /// `build` for a picture turned through a lens. `source` is a ratio of two linear forms of the
    /// output point, so every line of the picture is a line of the destination: the cells are cut
    /// exactly along the lines on which the edge fill bends, and the HUD's ghosts are the areas'
    /// exact outlines; only between those lines is the mapping drawn straight.
    fn lens(&mut self, shift: [f32; 2], lens: [f32; 4], areas: &[[f32; 4]]) {
        // source = 0.5 + (across, down) / max(depth, MIN_DEPTH), each a form of the output point.
        let depth: Form = [lens[0], lens[1], 1.0 - 0.5 * (lens[0] + lens[1])];
        let across: Form = [lens[2], 0.0, -0.5 * lens[2] - shift[0]];
        let down: Form = [0.0, lens[3], -0.5 * lens[3] - shift[1]];
        // Where the depth is its own and where it is held at the least, on either side of a line.
        let ahead: Form = [depth[0], depth[1], depth[2] - MIN_DEPTH];
        let whole = Piece::rect([0.0, 0.0, 1.0, 1.0]);
        let held = whole.corners().iter().any(|&at| form_at(ahead, at) < 0.0);
        let sides = [(depth, 1.0), ([0.0, 0.0, MIN_DEPTH], -1.0)];
        let sides = &sides[..if held { 2 } else { 1 }];
        // The line on which `axis` samples the picture at `at`: axis / depth = at - 0.5.
        let line = |axis: Form, depth: Form, at: f32| -> Form { std::array::from_fn(|i| axis[i] - (at - 0.5) * depth[i]) };
        let cells = lens_cells(shift, lens);
        let step = 1.0 / cells as f32;
        for &(depth, side) in sides {
            let mut bends = [[0.0; 3]; 12];
            for (i, &bend) in EDGE_BENDS.iter().enumerate() {
                bends[i] = line(across, depth, bend);
                bends[EDGE_BENDS.len() + i] = line(down, depth, bend);
            }
            for row in 0..cells {
                for column in 0..cells {
                    let (x, y) = (column as f32 * step, row as f32 * step);
                    // The last cell ends on the edge itself.
                    let end = |cell: usize, at: f32| if cell + 1 == cells { 1.0 } else { at + step };
                    let cell = Piece::rect([x, y, end(column, x), end(row, y)]);
                    self.pieces(shift, lens, if held { cell.cut(ahead, side) } else { cell }, &bends);
                }
            }
        }
        // A point whose source is on the HUD samples itself: each area and its mirrors in the edge
        // fill, as the outline that samples them.
        for area in areas {
            let (columns, rows) = (edge_ranges(area[0], area[2]), edge_ranges(area[1], area[3]));
            for &(depth, side) in sides {
                let part = if held { whole.cut(ahead, side) } else { whole };
                for (y0, y1) in rows {
                    for (x0, x1) in columns {
                        let mut piece = part;
                        for (axis, lo, hi) in [(across, x0, x1), (down, y0, y1)] {
                            if lo.is_finite() {
                                piece = piece.cut(line(axis, depth, lo), 1.0);
                            }
                            if hi.is_finite() {
                                piece = piece.cut(line(axis, depth, hi), -1.0);
                            }
                        }
                        let mut corners = [Corner { at: [0.0; 2], uv: [0.0; 2], fade: 1.0 }; PIECE];
                        for (corner, &at) in corners.iter_mut().zip(piece.corners()) {
                            (corner.at, corner.uv) = (at, at);
                        }
                        self.polygon(&corners[..piece.len]);
                    }
                }
            }
        }
        for area in areas {
            self.in_place(*area);
        }
    }

    /// `piece` of a lens cell cut along each of `bends` that crosses it, each part then drawn
    /// (`darkened`).
    fn pieces(&mut self, shift: [f32; 2], lens: [f32; 4], piece: Piece, bends: &[Form]) {
        if piece.len < 3 {
            return;
        }
        let Some((&bend, rest)) = bends.split_first() else {
            let corner = |at: [f32; 2]| {
                let (uv, fade) = edge(source(at, shift, lens));
                Corner { at, uv, fade }
            };
            let mut corners = [corner(piece.at[0]); PIECE];
            let mut gaps = [0.0; PIECE];
            for (i, &at) in piece.corners().iter().enumerate() {
                let sampled = source(at, shift, lens);
                (corners[i], gaps[i]) = (corner(at), edge_axis(sampled[1]).1 - edge_axis(sampled[0]).1);
            }
            return self.darkened(&corners[..piece.len], &gaps[..piece.len], corner);
        };
        // A corner on the line, as a cut leaves it, is on neither side.
        let (mut below, mut above) = (false, false);
        for &at in piece.corners() {
            let value = form_at(bend, at);
            below |= value < -1e-6;
            above |= value > 1e-6;
        }
        if below && above {
            self.pieces(shift, lens, piece.cut(bend, 1.0), rest);
            self.pieces(shift, lens, piece.cut(bend, -1.0), rest);
        } else {
            self.pieces(shift, lens, piece, rest);
        }
    }

    /// The rectangle `[x0, y0, x1, y1]` of the picture where it is, at full brightness.
    fn in_place(&mut self, [x0, y0, x1, y1]: [f32; 4]) {
        let corner = |x: f32, y: f32| Corner { at: [x, y], uv: [x, y], fade: 1.0 };
        self.polygon(&[corner(x0, y0), corner(x1, y0), corner(x1, y1), corner(x0, y1)]);
    }

    /// The cell `[x0, y0, x1, y1]` between cuts: a quad, or, where one axis alone would draw it
    /// darker than the other in one part and lighter in another, the two parts on either side of
    /// where they are alike, each darkened by its darker axis.
    fn cell(&mut self, shift: [f32; 2], [x0, y0, x1, y1]: [f32; 4]) {
        let corner = |at: [f32; 2]| {
            let (uv, fade) = edge([at[0] - shift[0], at[1] - shift[1]]);
            Corner { at, uv, fade }
        };
        // How much brighter the y axis alone draws a corner than the x axis: linear in a cell.
        let gap = |c: &Corner| edge_axis(c.at[1] - shift[1]).1 - edge_axis(c.at[0] - shift[0]).1;
        let quad = [corner([x0, y0]), corner([x1, y0]), corner([x1, y1]), corner([x0, y1])];
        self.darkened(&quad, &quad.map(|c| gap(&c)), corner);
    }

    /// The convex polygon `corners` (at most `PIECE`), each of which the y axis alone draws
    /// brighter than the x axis by its `gaps`: as it is, or, where one axis would draw it darker
    /// than the other in one part and lighter in another, the two parts on either side of where
    /// they are alike (a `corner` made there), each darkened by its darker axis.
    fn darkened(&mut self, corners: &[Corner], gaps: &[f32], corner: impl Fn([f32; 2]) -> Corner) {
        if gaps.iter().all(|&g| g >= 0.0) || gaps.iter().all(|&g| g <= 0.0) {
            self.polygon(corners);
            return;
        }
        let count = corners.len();
        let (mut darker_x, mut x_count, mut darker_y, mut y_count) = ([corners[0]; PIECE + 2], 0, [corners[0]; PIECE + 2], 0);
        for i in 0..count {
            let (a, b, ga, gb) = (corners[i], corners[(i + 1) % count], gaps[i], gaps[(i + 1) % count]);
            if ga >= 0.0 {
                darker_x[x_count] = a;
                x_count += 1;
            }
            if ga <= 0.0 {
                darker_y[y_count] = a;
                y_count += 1;
            }
            if ga * gb < 0.0 {
                let t = ga / (ga - gb);
                let alike = corner([a.at[0] + t * (b.at[0] - a.at[0]), a.at[1] + t * (b.at[1] - a.at[1])]);
                darker_x[x_count] = alike;
                darker_y[y_count] = alike;
                x_count += 1;
                y_count += 1;
            }
        }
        self.polygon(&darker_x[..x_count]);
        self.polygon(&darker_y[..y_count]);
    }

    /// A convex polygon as a fan of triangles.
    fn polygon(&mut self, corners: &[Corner]) {
        if corners.len() < 3 {
            return;
        }
        let first = self.corners.len() as i32;
        self.corners.extend_from_slice(corners);
        for i in 1..corners.len() as i32 - 1 {
            self.indices.extend([first, first + i, first + i + 1]);
        }
    }

    /// The vertices and indices of the built triangles drawn into `destination` from `source` of a
    /// texture `texture` texels large, as `SDL_RenderTexture` maps a source rectangle.
    pub fn geometry(&mut self, source: SDL_FRect, texture: (f32, f32), destination: SDL_FRect) -> (&[SDL_Vertex], &[i32]) {
        self.vertices.clear();
        self.vertices.extend(self.corners.iter().map(|c| SDL_Vertex {
            position: SDL_FPoint { x: destination.x + c.at[0] * destination.w, y: destination.y + c.at[1] * destination.h },
            color: SDL_FColor { r: c.fade, g: c.fade, b: c.fade, a: 1.0 },
            tex_coord: SDL_FPoint { x: (source.x + c.uv[0] * source.w) / texture.0, y: (source.y + c.uv[1] * source.h) / texture.1 },
        }));
        (&self.vertices, &self.indices)
    }
}

/// Says once, for the stream's life of the process, that a present path drew the picture without
/// its shift.
pub fn unshifted(path: &str, error: &str) {
    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
        eprintln!("nightjar: Frame Warp: {path} could not shift the picture ({error}); it draws the picture unshifted");
    }
}

/// Draws `source` of `texture` (None: all of it) into `destination` as `SDL_RenderTexture` does,
/// shifted by `warp` with its HUD in place and not repeated beside itself, building the triangles
/// in `mesh`. An identity warp is `SDL_RenderTexture` itself; a shifted draw SDL refuses is logged
/// once (`unshifted`) and drawn unshifted.
///
/// # Safety
/// `renderer` and `texture` are live, on the calling thread.
pub unsafe fn render_texture(renderer: *mut SDL_Renderer, texture: *mut SDL_Texture, source: Option<&SDL_FRect>, destination: &SDL_FRect, warp: &WarpDraw,
    mesh: &mut Mesh) -> bool {
    // SAFETY: caller guarantees `renderer` and `texture` are live on the calling thread.
    unsafe {
        let plain = || SDL_RenderTexture(renderer, texture, source.map_or(std::ptr::null(), |rect| rect as *const _), destination);
        if warp.is_identity() {
            return plain();
        }
        let (mut width, mut height) = (0.0, 0.0);
        if SDL_GetTextureSize(texture, &mut width, &mut height) && width > 0.0 && height > 0.0 {
            let source = source.copied().unwrap_or(SDL_FRect { x: 0.0, y: 0.0, w: width, h: height });
            mesh.build(warp.shift, warp.lens, warp.mask().map_or(&[][..], |hud| hud.areas()));
            // SDL copies the vertices into its queue: the mesh is free again once this returns.
            let (vertices, indices) = mesh.geometry(source, (width, height), *destination);
            if SDL_RenderGeometry(renderer, texture, vertices.as_ptr(), vertices.len() as i32, indices.as_ptr(), indices.len() as i32) {
                return true;
            }
        }
        unshifted("SDL's renderer", &crate::sdl::error());
        plain()
    }
}
