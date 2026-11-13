//! Frame Warp on the desktop's presenter (docs/frame-warp.md). `nightjar_core::frame_warp`
//! predicts how far the picture's content has to move for the mouse and right-stick input the
//! host had not seen when it rendered the picture; this module applies that shift on every
//! present path, keeps the HUD mask's cells in place, and presents the held picture again when
//! the shift moves between pictures.
//!
//! What every path does, in fractions of the visible picture: the output point `uv` samples the
//! picture at `uv - shift`, or, when the host knows the game's lens, where a camera turned by
//! that shift at the picture's centre shows it (`nightjar_core::frame_warp::source`: the edges
//! move farther than the centre), with the edge fill where that is outside the picture (mirrored
//! at the edge and darkened, `nightjar_core::frame_warp::edge`), unless the HUD mask cell
//! `floor(uv * (width, height))` is set, or the cell that sample lands on is, which samples at
//! `uv` as it is: the HUD stays in place and isn't drawn a second time beside itself, a shift
//! away (the scene behind it isn't in the picture, so it stays unshifted there). The shader
//! paths share `frame_warp.hlsl` (`hlsl`) and read the mask as bytes (byte `x / 8` of row `y`,
//! bit `x % 8`: `Mask::words` in little-endian order) through two rows of their constants, `warp`
//! `(shift x, shift y, mask width, mask height)` and `warpLens` (the lens terms); the SDL renderer
//! path draws the same mapping as triangles (`Mesh`, `render_texture`). An identity shift takes
//! each path's draw as it was.

#[cfg(any(windows, test))]
use std::borrow::Cow;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use nightjar_core::frame_warp::{edge, edge_axis, source, FrameWarp, Mask, Model, Status, Warp, EDGE_RAMP, FLAT, MIN_DEPTH};
use sdl3_sys::everything::*;

use crate::config::{HostRecord, NOTICE_FRAME_WARP_UNAVAILABLE};
use crate::strings::tr;

/// The HUD masks the presenter draws, the most the protocol sends (`MAX_MASK_EDGE` cells a side).
pub const MASK_EDGE: u16 = rubylight_protocol::frame_warp::MAX_MASK_EDGE;
/// The least move of the shift, in picture pixels, that presents the held picture again.
pub const REPEAT_STEP: f32 = 0.25;
/// The most HUD rectangles the SDL renderer path draws; a finer mask is drawn coarser.
pub const MAX_HUD_RECTS: usize = 1024;
/// Frame Warp's HLSL, which the HLSL shaders include as `frame_warp.hlsl` (`hlsl`).
#[cfg(any(windows, test))]
pub const HLSL: &str = include_str!("../frame_warp.hlsl");
#[cfg(any(windows, test))]
const HLSL_INCLUDE: &[u8] = b"#include \"frame_warp.hlsl\"";

/// `source` with each line that includes `frame_warp.hlsl` replaced by the file, as glslang finds
/// it beside the shader: Direct3D 11 compiles from memory, with no file to include from.
#[cfg(any(windows, test))]
pub fn hlsl(source: &[u8]) -> Cow<'_, [u8]> {
    let find = |text: &[u8]| text.windows(HLSL_INCLUDE.len()).position(|window| window == HLSL_INCLUDE);
    let Some(mut at) = find(source) else { return Cow::Borrowed(source) };
    let mut out = Vec::with_capacity(source.len() + HLSL.len());
    let mut rest = source;
    loop {
        out.extend_from_slice(&rest[..at]);
        out.extend_from_slice(HLSL.as_bytes());
        rest = &rest[at + HLSL_INCLUDE.len()..];
        match find(rest) {
            Some(next) => at = next,
            None => break,
        }
    }
    out.extend_from_slice(rest);
    Cow::Owned(out)
}

/// A HUD mask and, made on first use, its rectangles for the SDL renderer path.
#[derive(Debug)]
pub struct Hud {
    pub mask: Arc<Mask>,
    areas: OnceLock<Vec<[f32; 4]>>,
}

impl Hud {
    pub fn new(mask: Arc<Mask>) -> Self {
        Self { mask, areas: OnceLock::new() }
    }

    /// The set cells as at most `MAX_HUD_RECTS` rectangles, `[x0, y0, x1, y1]` in fractions of the
    /// picture (`hud_areas`).
    pub fn areas(&self) -> &[[f32; 4]] {
        self.areas.get_or_init(|| hud_areas(&self.mask, MAX_HUD_RECTS))
    }
}

/// What one present draws the picture with.
#[derive(Clone, Debug)]
pub struct WarpDraw {
    /// How far the picture's centre moves, in fractions of the visible picture: a flat picture
    /// samples at `uv - shift`.
    pub shift: [f32; 2],
    /// How the rest of the picture follows (`Warp::lens`): the point `uv` samples at
    /// `source(uv, shift, lens)`. `FLAT` for a picture that moves alike everywhere.
    pub lens: [f32; 4],
    /// The cells that stay in place; None for none.
    pub hud: Option<Arc<Hud>>,
}

impl Default for WarpDraw {
    fn default() -> Self {
        Self { shift: [0.0; 2], lens: FLAT, hud: None }
    }
}

impl WarpDraw {
    /// The draw for `warp`, with `hud` while the shift is on and the predictor knows a mask.
    pub fn of(warp: &Warp, hud: Option<&Arc<Hud>>) -> Self {
        if !warp.active {
            return Self::default();
        }
        Self { shift: warp.uv, lens: warp.lens, hud: hud.filter(|_| warp.mask_generation.is_some()).cloned() }
    }

    /// Nothing moves: every path draws as it does without Frame Warp.
    pub fn is_identity(&self) -> bool {
        self.shift == [0.0, 0.0]
    }

    /// The shaders' `warp` row: the shift, then the mask's size in cells when `mask` (its texture
    /// is bound), else 0 for no mask. All zero for an identity draw, which the shaders skip.
    pub fn row(&self, mask: bool) -> [f32; 4] {
        if self.is_identity() {
            return [0.0; 4];
        }
        let cells = self.hud.as_ref().filter(|_| mask).map_or([0.0; 2], |hud| [f32::from(hud.mask.width), f32::from(hud.mask.height)]);
        [self.shift[0], self.shift[1], cells[0], cells[1]]
    }

    /// The shaders' `warpLens` row, which they read only beside a `warp` row that shifts.
    pub fn lens_row(&self) -> [f32; 4] {
        if self.is_identity() { FLAT } else { self.lens }
    }

    /// The HUD mask whose texture a shader path binds, when the draw shifts at all.
    pub fn mask(&self) -> Option<&Arc<Hud>> {
        self.hud.as_ref().filter(|hud| !self.is_identity() && hud.mask.width > 0 && hud.mask.height > 0)
    }
}

/// The mask as the shaders read it: rows of `Mask::stride(width) * 4` bytes, byte `x / 8` of row
/// `y` holding cell `x` in bit `x % 8`. Returns the row's bytes and the rows.
pub fn mask_bytes(mask: &Mask) -> (usize, Vec<u8>) {
    let row = Mask::stride(mask.width) * 4;
    let mut bytes: Vec<u8> = mask.words.iter().flat_map(|word| word.to_le_bytes()).collect();
    bytes.resize(row * usize::from(mask.height), 0);
    (row, bytes)
}

/// A rectangle of mask cells: its top left cell and its size in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellRect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

/// The runs of set cells in one row of `mask` (`stride` words), as (first cell, length).
fn row_runs(words: &[u32], width: u16, out: &mut Vec<(u16, u16)>) {
    out.clear();
    let width = u32::from(width);
    let mut start: Option<u32> = None;
    for (index, &word) in words.iter().enumerate() {
        let base = index as u32 * 32;
        if base >= width {
            break;
        }
        // Cells past the width are clear, so a run ends at the width at the latest.
        let valid = if width - base >= 32 { u64::from(u32::MAX) } else { (1u64 << (width - base)) - 1 };
        let bits = u64::from(word) & valid;
        let mut at = 0u32;
        while at < 32 {
            let looking = if start.is_none() { bits } else { !bits & u64::from(u32::MAX) };
            let rest = looking >> at;
            if rest == 0 {
                break;
            }
            at += rest.trailing_zeros();
            if at >= 32 {
                break;
            }
            match start.take() {
                None => start = Some(base + at),
                Some(first) => out.push((first as u16, (base + at - first) as u16)),
            }
        }
    }
    if let Some(first) = start {
        out.push((first as u16, (width - first) as u16));
    }
}

/// The set cells of `mask` as rectangles: each row's runs, a run below one of the same columns
/// growing that rectangle down. Every set cell is in exactly one rectangle.
pub fn cell_rects(mask: &Mask) -> Vec<CellRect> {
    let stride = Mask::stride(mask.width);
    let mut done = Vec::new();
    // Rectangles that reach the row above, by first column.
    let mut open: Vec<CellRect> = Vec::new();
    let mut next = Vec::new();
    let mut runs = Vec::new();
    for y in 0..mask.height {
        let start = usize::from(y) * stride;
        let Some(words) = mask.words.get(start..start + stride) else { break };
        row_runs(words, mask.width, &mut runs);
        next.clear();
        let mut above = open.drain(..).peekable();
        for &(x, w) in &runs {
            // The open rectangles left of this run end above it.
            while above.peek().is_some_and(|r| r.x < x) {
                done.push(above.next().expect("peeked"));
            }
            match above.peek() {
                Some(r) if r.x == x && r.w == w => {
                    let mut grown = above.next().expect("peeked");
                    grown.h += 1;
                    next.push(grown);
                }
                _ => next.push(CellRect { x, y, w, h: 1 }),
            }
        }
        done.extend(above);
        std::mem::swap(&mut open, &mut next);
    }
    done.append(&mut open);
    done
}

/// The HUD of `mask` as at most `limit` rectangles, `[x0, y0, x1, y1]` in fractions of the
/// picture: exact when that many are enough, else from the mask halved (`Mask::halved`) until they
/// are, which keeps a little more around the HUD in place.
pub fn hud_areas(mask: &Mask, limit: usize) -> Vec<[f32; 4]> {
    if mask.width == 0 || mask.height == 0 {
        return Vec::new();
    }
    let (width, height) = (f32::from(mask.width), f32::from(mask.height));
    let mut level = std::borrow::Cow::Borrowed(mask);
    let mut scale = 1u32;
    loop {
        let rects = cell_rects(&level);
        if rects.len() <= limit || (level.width <= 1 && level.height <= 1) {
            let edge = |cells: u16, scale: u32, size: f32| (cells as f32 * scale as f32 / size).min(1.0);
            return rects
                .iter()
                .map(|r| {
                    [edge(r.x, scale, width), edge(r.y, scale, height), edge(r.x + r.w, scale, width), edge(r.y + r.h, scale, height)]
                })
                .collect();
        }
        level = std::borrow::Cow::Owned(level.halved());
        scale *= 2;
    }
}

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
const LENS_ERROR: f32 = 5e-5;
/// The most cells a side it draws: a turn at the limit of both axes through a wide lens bends
/// its stretched corner more than this many keep within `LENS_ERROR`, by half a pixel of 1080p.
const MAX_LENS_CELLS: usize = 96;
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
fn lens_bend(shift: [f32; 2], lens: [f32; 4]) -> f32 {
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
fn lens_cells(shift: [f32; 2], lens: [f32; 4]) -> usize {
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
    vertices: Vec<SDL_Vertex>,
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

/// When the display waits and when the next picture is expected, for `Repeat::decide`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timing {
    pub refresh_us: u64,
    /// The present waits for the refresh (V-Sync): a repeat must leave the next picture's refresh to it.
    pub vsync: bool,
    /// When the next picture is expected: a waiting one's due time, else the held one's arrival plus a frame.
    pub next_picture_us: Option<u64>,
}

/// Presents the held picture again when the shift moved by `REPEAT_STEP` since the last present:
/// at most once a refresh, and never into the refresh the next picture is expected for.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Repeat {
    last_present_us: Option<u64>,
    last_pixels: [f32; 2],
    /// A check that found nothing to present waits until then.
    next_check_us: u64,
}

impl Repeat {
    /// A present went out at `at_us` with the shift `pixels`.
    pub fn presented(&mut self, at_us: u64, pixels: [f32; 2]) {
        self.last_present_us = Some(at_us);
        self.last_pixels = pixels;
    }

    pub fn last_pixels(&self) -> [f32; 2] {
        self.last_pixels
    }

    /// When to look again: a refresh after the last present, or the later check.
    pub fn due_us(&self, refresh_us: u64) -> Option<u64> {
        self.last_present_us.map(|last| (last + refresh_us).max(self.next_check_us))
    }

    /// Whether to present the held picture again now, its shift `pixels` now.
    pub fn decide(&mut self, now_us: u64, pixels: [f32; 2], timing: Timing) -> bool {
        let refresh = timing.refresh_us.max(1);
        let Some(due) = self.due_us(refresh) else { return false };
        if now_us < due {
            return false;
        }
        let moved = (pixels[0] - self.last_pixels[0]).hypot(pixels[1] - self.last_pixels[1]);
        if moved < REPEAT_STEP {
            self.next_check_us = now_us + refresh;
            return false;
        }
        if let Some(next) = timing.next_picture_us {
            let guard = if timing.vsync { refresh } else { refresh / 2 };
            let late = now_us > next + refresh / 2;
            if !late && next < now_us + guard {
                // The next picture takes this refresh; look again once it is late.
                self.next_check_us = next + refresh / 2 + 1;
                return false;
            }
        }
        true
    }
}

/// The picture on the screen.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Held {
    number: u32,
    race_seq: u64,
    ready_us: u64,
}

/// Frame Warp on the presenting thread for one stream.
pub struct Presenter {
    warp: Arc<FrameWarp>,
    held: Option<Held>,
    /// The raced frame whose slices are on the screen and the shift they were drawn with,
    /// computed at its first slice.
    raced: Option<(u64, Shift)>,
    /// The shift the next present of the held picture uses (a repeat's check, or the raced
    /// frame's slices), instead of a new one.
    pending: Option<Shift>,
    /// The shift of the last draw, for `presented`.
    drawn: Shift,
    /// The input count (`FrameWarp::input_seq`) the last present's shift saw.
    presented_seq: u32,
    hud: Option<Arc<Hud>>,
    mask_known: Option<u16>,
    repeat: Repeat,
    locked: bool,
}

/// A shift and the input count (`FrameWarp::input_seq`) read before it was computed.
#[derive(Clone, Copy, Debug, Default)]
struct Shift {
    warp: Warp,
    input_seq: u32,
}

impl Presenter {
    pub fn new(warp: Arc<FrameWarp>) -> Self {
        Self { warp, held: None, raced: None, pending: None, drawn: Shift::default(), presented_seq: 0, hud: None, mask_known: None, repeat: Repeat::default(),
            locked: false }
    }

    /// The shift of picture `frame` drawn at `now`, counted as drawn.
    fn shift(&self, frame: u32, now: Instant) -> Shift {
        let input_seq = self.warp.input_seq();
        Shift { warp: self.warp.warp_at(frame, now), input_seq }
    }

    /// The decoder's picture `number` goes on the screen; `race_seq` is its raced frame (0 for
    /// none) and `ready_us` when it was decoded. A raced frame keeps its slices' shift.
    pub fn picture(&mut self, number: u32, race_seq: u64, ready_us: u64) {
        self.held = Some(Held { number, race_seq, ready_us });
        self.pending = match self.raced {
            Some((seq, shift)) if race_seq != 0 && seq == race_seq => Some(shift),
            _ => None,
        };
    }

    /// The frame a slice of raced frame `seq` belongs to: as many frames after the held one as
    /// its raced frame is after the held one's.
    fn slice_frame(&self, seq: u64) -> u32 {
        match self.held {
            Some(held) if held.race_seq != 0 && seq > held.race_seq => held.number.wrapping_add((seq - held.race_seq).min(u64::from(u32::MAX)) as u32),
            Some(held) => held.number.wrapping_add(1),
            None => 0,
        }
    }

    /// Takes a new HUD mask, if one came.
    fn poll_mask(&mut self) {
        if let Some(mask) = self.warp.mask_since(self.mask_known) {
            self.mask_known = mask.as_ref().map(|mask| mask.generation);
            self.hud = mask.map(|mask| Arc::new(Hud::new(mask)));
        }
    }

    /// The draw of this present: of a slice of raced frame `slice`, or of the held picture.
    pub fn draw(&mut self, slice: Option<u64>) -> WarpDraw {
        self.draw_at(slice, Instant::now())
    }

    pub fn draw_at(&mut self, slice: Option<u64>, now: Instant) -> WarpDraw {
        self.poll_mask();
        let shift = match slice {
            Some(seq) => match self.raced {
                Some((raced, shift)) if raced == seq => shift,
                _ => {
                    let shift = self.shift(self.slice_frame(seq), now);
                    self.raced = Some((seq, shift));
                    shift
                }
            },
            None => match (self.pending.take(), self.held) {
                (Some(shift), _) => shift,
                (None, Some(held)) => self.shift(held.number, now),
                (None, None) => Shift { warp: Warp::default(), input_seq: self.warp.input_seq() },
            },
        };
        self.drawn = shift;
        WarpDraw::of(&shift.warp, self.hud.as_ref())
    }

    /// The present of the last draw went out at `at_us`.
    pub fn presented(&mut self, at_us: u64) {
        self.repeat.presented(at_us, self.drawn.warp.pixels);
        self.presented_seq = self.drawn.input_seq;
    }

    /// When the held picture may go out again, for the loop's wait: None while nothing is held
    /// or slices race, and unless the last present was shifted, or the model is locked and input
    /// that can move the shift was sent since that present's shift (`FrameWarp::input_seq`) or
    /// the stick is held out of its dead zone. Otherwise the shift stays where it was drawn, so
    /// an idle stream does not wake every refresh to find that out.
    pub fn repeat_due_us(&self, refresh_us: u64, racing: bool) -> Option<u64> {
        if racing || self.held.is_none() {
            return None;
        }
        let moved = self.repeat.last_pixels() != [0.0, 0.0];
        let input = self.locked && (self.warp.input_seq() != self.presented_seq || self.warp.stick_deflected());
        if !moved && !input {
            return None;
        }
        self.repeat.due_us(refresh_us)
    }

    /// Whether the held picture goes out again now with a new shift (`Repeat`). `timing`'s next
    /// picture is one already waiting, if any; the held picture's arrival plus `frame_us` (one
    /// frame of the stream) is expected too.
    pub fn repeat_now(&mut self, now_us: u64, timing: Timing, frame_us: u64) -> bool {
        self.repeat_at(now_us, Instant::now(), timing, frame_us)
    }

    fn repeat_at(&mut self, now_us: u64, now: Instant, timing: Timing, frame_us: u64) -> bool {
        let Some(held) = self.held else { return false };
        if self.repeat_due_us(timing.refresh_us, false).is_none_or(|due| now_us < due) {
            return false;
        }
        let expected = held.ready_us + frame_us;
        let next_picture_us = Some(timing.next_picture_us.map_or(expected, |waiting| waiting.min(expected)));
        // Deciding counts nothing; only a repeat that goes out is counted as drawn.
        let peeked = self.warp.peek_at(held.number, now);
        let again = self.repeat.decide(now_us, peeked.pixels, Timing { next_picture_us, ..timing });
        if again {
            self.pending = Some(self.shift(held.number, now));
        }
        again
    }

    /// A model from the host: the log line when it locked or unlocked.
    pub fn model(&mut self, model: &Model) -> Option<String> {
        let locked = model.locked();
        if std::mem::replace(&mut self.locked, locked) == locked {
            return None;
        }
        Some(if locked {
            format!("nightjar: Frame Warp: locked ({}), the picture follows the input", locked_text(model))
        } else {
            "nightjar: Frame Warp: unlocked, learning again".to_owned()
        })
    }

    pub fn status(&self) -> Status {
        self.warp.status()
    }
}

/// The input a locked model follows and its gain.
fn locked_text(model: &Model) -> String {
    let gain = |g: [f32; 2]| g[0].abs().max(g[1].abs());
    let mut parts = Vec::new();
    if model.mouse_locked {
        parts.push(format!("{:.2} px/count", gain(model.mouse_gain)));
    }
    if model.stick_locked {
        parts.push(format!("{:.0} px/s", gain(model.stick_gain)));
    }
    if let Some(degrees) = nightjar_core::frame_warp::field_of_view(model) {
        parts.push(format!("{degrees:.0} degree lens"));
    }
    parts.join(", ")
}

/// The statistics overlay's Frame Warp line: what the host's model locked, its lag and the shift,
/// "learning" before it locks, or "not offered by the PC".
pub fn status_line(status: Option<&Status>) -> String {
    let Some(status) = status else { return "not offered by the PC".to_owned() };
    let Some(model) = status.model.filter(Model::locked) else {
        return match status.model {
            Some(model) if model.confidence > 0.0 => format!("learning ({:.0}% fit)", model.confidence * 100.0),
            _ => "learning".to_owned(),
        };
    };
    let shift = status.pixels[0].hypot(status.pixels[1]);
    let mut line = format!(
        "locked {}, lag {:.0} ms, shift {shift:.0} px (peak {:.0})",
        locked_text(&model),
        f64::from(model.lag_us) / 1000.0,
        status.peak
    );
    if status.mask_cells > 0 {
        line.push_str(&format!(", HUD {} cells", status.mask_cells));
    }
    line
}

/// The notice that this PC doesn't offer Frame Warp, once per PC like the PyroWave fallback's
/// (`pyrowave_notice`): the first stream from it with the setting on and Frame Warp refused says
/// it, then nothing until Frame Warp runs there again. Returns the text and whether `record` changed.
pub fn frame_warp_notice(record: &mut HostRecord, running: bool, refused: bool) -> (Option<String>, bool) {
    if refused {
        let first = record.claim_notice(NOTICE_FRAME_WARP_UNAVAILABLE);
        return (first.then(|| tr("This PC doesn’t offer Frame Warp, so Nightjar streams without it.").to_owned()), first);
    }
    (None, running && record.reset_notice(NOTICE_FRAME_WARP_UNAVAILABLE))
}

/// A HUD mask as a Direct3D 11 texture (R8, `mask_bytes`), made again when the mask changes.
#[cfg(windows)]
#[derive(Default)]
pub struct D3d11Mask {
    /// The HUD last asked for and its view, None when it could not be made.
    made: Option<(Arc<Hud>, Option<windows::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView>)>,
    failed: bool,
}

#[cfg(windows)]
impl D3d11Mask {
    /// The view of `warp`'s HUD mask on `device`, None without one. A mask that cannot be made is
    /// logged once and drawn as none: the HUD then moves with the picture.
    ///
    /// # Safety
    /// `device` is live, used on this thread.
    pub unsafe fn view(&mut self, device: &windows::Win32::Graphics::Direct3D11::ID3D11Device, warp: &WarpDraw)
        -> Option<windows::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView> {
        let hud = warp.mask()?;
        if let Some((made, view)) = &self.made {
            if Arc::ptr_eq(made, hud) {
                return view.clone();
            }
        }
        let view = match Self::make(device, &hud.mask) {
            Ok(view) => Some(view),
            Err(error) => {
                if !std::mem::replace(&mut self.failed, true) {
                    eprintln!("nightjar: Frame Warp: the HUD mask cannot be drawn ({error}); the HUD moves with the picture");
                }
                None
            }
        };
        self.made = Some((hud.clone(), view.clone()));
        view
    }

    unsafe fn make(device: &windows::Win32::Graphics::Direct3D11::ID3D11Device, mask: &Mask)
        -> Result<windows::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView, String> {
        use windows::Win32::Graphics::Direct3D11::*;
        use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8_UNORM, DXGI_SAMPLE_DESC};
        let (row, bytes) = mask_bytes(mask);
        let data = D3D11_SUBRESOURCE_DATA { pSysMem: bytes.as_ptr().cast(), SysMemPitch: row as u32, SysMemSlicePitch: 0 };
        let mut texture = None;
        device.CreateTexture2D(&D3D11_TEXTURE2D_DESC { Width: row as u32, Height: u32::from(mask.height), MipLevels: 1, ArraySize: 1,
            Format: DXGI_FORMAT_R8_UNORM, SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_IMMUTABLE,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32, ..Default::default() }, Some(std::ptr::from_ref(&data)), Some(&mut texture))
            .map_err(|e| format!("its texture: {e}"))?;
        let texture = texture.ok_or("no texture")?;
        let mut view = None;
        device.CreateShaderResourceView(&texture, None, Some(&mut view)).map_err(|e| format!("its view: {e}"))?;
        view.ok_or_else(|| "no view".to_owned())
    }
}

#[cfg(test)]
mod tests;
