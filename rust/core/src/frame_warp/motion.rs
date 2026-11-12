//! How far the picture's content moved between two captures, from small luma thumbnails.
//!
//! The camera turns the whole scene, so the estimate works on intensity profiles: for a band
//! of rows, the mean of each column (and for a band of columns, the mean of each row), with
//! its slow trend taken out. A shift of the profile is a shift of the content. The thumbnail
//! is split into a 3 by 3 grid; each cell finds its own shift by normalised correlation, and
//! the median of the cells that found one clearly is the picture's. Cells that disagree (the
//! HUD, a character crossing) do not move the median. The estimate only teaches the model;
//! nothing on screen depends on one frame's estimate.

/// A capture's luma, downscaled, as the GPU returns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thumbnail {
    pub width: usize,
    pub height: usize,
    /// `width * height` bytes, rows top to bottom.
    pub luma: Vec<u8>,
    /// The capture's present time, microseconds on the session clock.
    pub at_us: u64,
}

impl Thumbnail {
    fn at(&self, x: usize, y: usize) -> f32 {
        f32::from(self.luma[y * self.width + x])
    }

    fn valid(&self) -> bool {
        self.width >= MIN_EDGE
            && self.height >= MIN_EDGE
            && self.luma.len() == self.width * self.height
    }
}

/// The content's shift from one thumbnail to the next, in thumbnail pixels (x right, y down).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    pub dx: f32,
    pub dy: f32,
    /// 0 to 1: how many cells agree with the median, and how clearly they found it.
    pub confidence: f32,
}

/// The thumbnail's longer side, as the GPU makes it.
pub const THUMBNAIL_WIDTH: usize = 256;
const MIN_EDGE: usize = 32;
const GRID: usize = 3;
/// Edges left out: vignettes, letterboxing and edge HUD.
const MARGIN: f32 = 0.04;
/// Largest shift searched, as a fraction of the profile's length.
const SEARCH: f32 = 0.2;
/// Correlation a cell needs to count.
const MIN_CORRELATION: f32 = 0.6;
/// How much a cell's best shift must beat any other more than 2 pixels away.
const MIN_UNIQUENESS: f32 = 0.04;
/// Profile variance below this is a flat cell (sky, a wall in shadow).
const MIN_VARIANCE: f32 = 0.5;
/// Cells within this many pixels of the median agree with it.
const AGREE: f32 = 1.5;
/// A pixel this far from its right and its lower neighbour (in luma codes) is texture: it
/// changes whichever way it moves (an edge along the move would not).
const TEXTURE: u8 = 8;
/// A cell whose texture is this much unchanged stands still.
const STILL_SHARE: f32 = 0.9;
/// A cell needs this many texture pixels to tell whether it stands still.
const MIN_TEXTURE: usize = 16;

/// The content's shift from `prev` to `cur`, or `None` when the thumbnails differ in size or
/// too few cells find a clear shift.
pub fn estimate(prev: &Thumbnail, cur: &Thumbnail) -> Option<Motion> {
    estimate_around(prev, cur, None)
}

/// [`estimate`], leaving out the thumbnail pixels `hud` marks (row by row, the thumbnail's
/// size): a HUD stays where it is, and a wide bar's rows left in would pull every vertical
/// shift toward 0. Ignored when its size does not match.
pub fn estimate_around(prev: &Thumbnail, cur: &Thumbnail, hud: Option<&[bool]>) -> Option<Motion> {
    estimate_with(prev, cur, hud, &mut Scratch::default())
}

/// The buffers an estimate works in, kept from one estimate to the next.
#[derive(Debug, Default)]
pub struct Scratch {
    /// The previous thumbnail's detrended row profile for each column band, unshifted: the
    /// same in both vertical passes.
    before_rows: [Vec<f32>; GRID],
    buffers: Buffers,
}

#[derive(Debug, Default)]
struct Buffers {
    before: Vec<f32>,
    after: Vec<f32>,
    sums: Vec<(f32, u32)>,
    prefix: Vec<f32>,
    count: Vec<u32>,
    scores: Vec<Option<f32>>,
}

/// [`estimate_around`] in `scratch`'s buffers.
pub fn estimate_with(
    prev: &Thumbnail,
    cur: &Thumbnail,
    hud: Option<&[bool]>,
    scratch: &mut Scratch,
) -> Option<Motion> {
    if !prev.valid() || !cur.valid() || prev.width != cur.width || prev.height != cur.height {
        return None;
    }
    let w = cur.width;
    // One branch here, so the loops below inline the test (or drop it without a HUD).
    match hud.filter(|hud| hud.len() == cur.width * cur.height) {
        None => measure(prev, cur, |_, _| true, scratch),
        Some(hud) => measure(prev, cur, |x, y| !hud[y * w + x], scratch),
    }
}

/// Where the cells are and how far each searches.
struct Grid {
    x_edges: [usize; GRID + 1],
    y_edges: [usize; GRID + 1],
    search_x: usize,
    search_y: usize,
}

/// [`estimate_around`] where `scene` is the picture (not the HUD).
fn measure<S: Fn(usize, usize) -> bool + Copy>(
    prev: &Thumbnail,
    cur: &Thumbnail,
    scene: S,
    scratch: &mut Scratch,
) -> Option<Motion> {
    let (w, h) = (cur.width, cur.height);
    let grid = Grid {
        x_edges: edges(w),
        y_edges: edges(h),
        search_x: ((w as f32 * SEARCH) as usize).max(2),
        search_y: ((h as f32 * SEARCH) as usize).max(2),
    };
    let (x_edges, y_edges) = (&grid.x_edges, &grid.y_edges);

    // A picture whose texture (the HUD left out) is mostly unchanged stands still, whatever
    // moves in one corner of it.
    let still = still_cells(prev, cur, scene, x_edges, y_edges);
    if still * 2 > GRID * GRID {
        return Some(Motion {
            dx: 0.0,
            dy: 0.0,
            confidence: still as f32 / (GRID * GRID) as f32,
        });
    }

    // Vertical first: one row profile per column band, full height, correlated per row band.
    // Then horizontal: one column profile per row band, the current one taken that band's
    // vertical shift lower so a tilt does not blur it. Then vertical again, the current
    // column band taken that band's horizontal shift across, so a fast pan does not blur it.
    let Scratch {
        before_rows,
        buffers,
    } = scratch;
    for (band, before) in before_rows.iter_mut().enumerate() {
        profile_rows(prev, x_edges[band], x_edges[band + 1], 0, scene, before);
        detrend(before, &mut buffers.prefix, &mut buffers.count);
    }
    let first: [[Option<Shift>; GRID]; GRID] = core::array::from_fn(|band| {
        vertical_band(cur, scene, &grid, band, &before_rows[band], 0, buffers)
    });
    let mut horizontal = [[None; GRID]; GRID];
    for cell in 0..GRID {
        let (y0, y1) = (y_edges[cell], y_edges[cell + 1]);
        let shifts: Vec<f32> = (0..GRID)
            .filter_map(|band| first[band][cell].map(|s| s.at))
            .collect();
        let shift_y = median(&shifts).unwrap_or(0.0).round() as isize;
        let Buffers {
            before,
            after,
            sums,
            prefix,
            count,
            scores,
        } = &mut *buffers;
        profile_columns(prev, y0, y1, 0, scene, sums, before);
        detrend(before, prefix, count);
        profile_columns(cur, y0, y1, shift_y, scene, sums, after);
        detrend(after, prefix, count);
        for (band, column) in horizontal.iter_mut().enumerate() {
            column[cell] = best_shift(
                before,
                after,
                x_edges[band],
                x_edges[band + 1],
                grid.search_x,
                scores,
            );
        }
    }
    let column_shift: [isize; GRID] = core::array::from_fn(|band| {
        let shifts: Vec<f32> = horizontal[band].iter().flatten().map(|s| s.at).collect();
        median(&shifts).unwrap_or(0.0).round() as isize
    });
    // A band that did not move across measures as it did the first time.
    let mut vertical = first;
    for (band, cells) in vertical.iter_mut().enumerate() {
        if column_shift[band] != 0 {
            *cells = vertical_band(
                cur,
                scene,
                &grid,
                band,
                &before_rows[band],
                column_shift[band],
                buffers,
            );
        }
    }
    let mut cells = Vec::with_capacity(GRID * GRID);
    for band in 0..GRID {
        for cell in 0..GRID {
            if let (Some(x), Some(y)) = (horizontal[band][cell], vertical[band][cell]) {
                cells.push((x.at, y.at, x.correlation.min(y.correlation)));
            }
        }
    }
    if cells.len() < 3 {
        return None;
    }
    let dx = median(&cells.iter().map(|c| c.0).collect::<Vec<_>>())?;
    let dy = median(&cells.iter().map(|c| c.1).collect::<Vec<_>>())?;
    let agreeing: Vec<f32> = cells
        .iter()
        .filter(|c| (c.0 - dx).abs() <= AGREE && (c.1 - dy).abs() <= AGREE)
        .map(|c| c.2)
        .collect();
    if agreeing.len() < 3 {
        return None;
    }
    let mean_correlation = agreeing.iter().sum::<f32>() / agreeing.len() as f32;
    let confidence = (agreeing.len() as f32 / (GRID * GRID) as f32) * mean_correlation;
    Some(Motion {
        dx,
        dy,
        confidence: confidence.clamp(0.0, 1.0),
    })
}

/// The vertical shift of each cell in column band `band`: `before` (the previous thumbnail's
/// detrended row profile over the band) found in the current one's, its columns taken
/// `column_shift` across.
fn vertical_band<S: Fn(usize, usize) -> bool + Copy>(
    cur: &Thumbnail,
    scene: S,
    grid: &Grid,
    band: usize,
    before: &[f32],
    column_shift: isize,
    buffers: &mut Buffers,
) -> [Option<Shift>; GRID] {
    let Buffers {
        after,
        prefix,
        count,
        scores,
        ..
    } = buffers;
    let (x0, x1) = (grid.x_edges[band], grid.x_edges[band + 1]);
    profile_rows(cur, x0, x1, column_shift, scene, after);
    detrend(after, prefix, count);
    core::array::from_fn(|cell| {
        best_shift(
            before,
            after,
            grid.y_edges[cell],
            grid.y_edges[cell + 1],
            grid.search_y,
            scores,
        )
    })
}

/// Cell boundaries along an edge of `len` pixels, margins left out.
fn edges(len: usize) -> [usize; GRID + 1] {
    let margin = (len as f32 * MARGIN) as usize;
    let inner = len - 2 * margin;
    let mut out = [0; GRID + 1];
    for (i, edge) in out.iter_mut().enumerate() {
        *edge = margin + inner * i / GRID;
    }
    out
}

/// How many grid cells stand still: their texture in `prev` where `scene` is at least
/// [`STILL_SHARE`] exactly the same in `cur`.
fn still_cells<S: Fn(usize, usize) -> bool>(
    prev: &Thumbnail,
    cur: &Thumbnail,
    scene: S,
    x_edges: &[usize; GRID + 1],
    y_edges: &[usize; GRID + 1],
) -> usize {
    let w = prev.width;
    let texture = |x: usize, y: usize| {
        let v = prev.luma[y * w + x];
        let right = prev.luma[y * w + (x + 1).min(w - 1)];
        let below = prev.luma[(y + 1).min(prev.height - 1) * w + x];
        v.abs_diff(right) >= TEXTURE && v.abs_diff(below) >= TEXTURE
    };
    let mut still = 0;
    for row in 0..GRID {
        for column in 0..GRID {
            let (mut textured, mut unchanged) = (0usize, 0usize);
            for y in y_edges[row]..y_edges[row + 1] {
                for x in x_edges[column]..x_edges[column + 1] {
                    if scene(x, y) && texture(x, y) {
                        textured += 1;
                        unchanged += usize::from(prev.luma[y * w + x] == cur.luma[y * w + x]);
                    }
                }
            }
            if textured >= MIN_TEXTURE && unchanged as f32 >= textured as f32 * STILL_SHARE {
                still += 1;
            }
        }
    }
    still
}

/// Into `out`: the mean of each row over columns `x0..x1` where `scene` (not the HUD), the
/// columns taken `shift` to the right (clamped at the edges); NaN for a row of HUD only.
fn profile_rows<S: Fn(usize, usize) -> bool>(
    t: &Thumbnail,
    x0: usize,
    x1: usize,
    shift: isize,
    scene: S,
    out: &mut Vec<f32>,
) {
    let last = t.width as isize - 1;
    out.clear();
    out.extend((0..t.height).map(|y| {
        let (mut sum, mut n) = (0.0f32, 0u32);
        for x in x0..x1 {
            let x = (x as isize + shift).clamp(0, last) as usize;
            if scene(x, y) {
                sum += t.at(x, y);
                n += 1;
            }
        }
        if n == 0 { f32::NAN } else { sum / n as f32 }
    }));
}

/// Into `out`: the mean of each column over rows `y0..y1` where `scene` (not the HUD), rows
/// taken `shift` lower (clamped at the edges); NaN for a column of HUD only. `sums` is
/// scratch.
fn profile_columns<S: Fn(usize, usize) -> bool>(
    t: &Thumbnail,
    y0: usize,
    y1: usize,
    shift: isize,
    scene: S,
    sums: &mut Vec<(f32, u32)>,
    out: &mut Vec<f32>,
) {
    sums.clear();
    sums.resize(t.width, (0.0, 0));
    for y in y0..y1 {
        let y = (y as isize + shift).clamp(0, t.height as isize - 1) as usize;
        for (x, (sum, n)) in sums.iter_mut().enumerate() {
            if scene(x, y) {
                *sum += t.at(x, y);
                *n += 1;
            }
        }
    }
    out.clear();
    out.extend(
        sums.iter()
            .map(|&(sum, n)| if n == 0 { f32::NAN } else { sum / n as f32 }),
    );
}

/// The profile, in place, less its moving average over 13 samples: lighting and slow
/// gradients out, edges and texture kept. NaN samples (HUD only) stay NaN and are left out of
/// the averages. `prefix` and `count` are scratch.
fn detrend(profile: &mut [f32], prefix: &mut Vec<f32>, count: &mut Vec<u32>) {
    const RADIUS: usize = 6;
    let n = profile.len();
    prefix.clear();
    count.clear();
    prefix.push(0.0f32);
    count.push(0u32);
    for v in profile.iter() {
        let known = !v.is_nan();
        prefix.push(prefix.last().unwrap() + if known { *v } else { 0.0 });
        count.push(count.last().unwrap() + u32::from(known));
    }
    // Each sample's average comes from the running sums, not its neighbours, so the profile
    // can take the result as it goes.
    for (i, value) in profile.iter_mut().enumerate() {
        let a = i.saturating_sub(RADIUS);
        let b = (i + RADIUS + 1).min(n);
        let known = count[b] - count[a];
        *value = if value.is_nan() || known == 0 {
            f32::NAN
        } else {
            *value - (prefix[b] - prefix[a]) / known as f32
        };
    }
}

#[derive(Clone, Copy, Debug)]
struct Shift {
    /// Sub-pixel shift: `after[i + at] ~ before[i]`.
    at: f32,
    correlation: f32,
}

/// The shift of `before[from..to]` within `after`, searched over `-search..=search`, by
/// normalised correlation and refined to sub-pixel by a parabola through the peak. `None` for
/// a flat cell, a weak or an ambiguous peak. `scores` is scratch.
fn best_shift(
    before: &[f32],
    after: &[f32],
    from: usize,
    to: usize,
    search: usize,
    scores: &mut Vec<Option<f32>>,
) -> Option<Shift> {
    let len = to.checked_sub(from)?;
    let known = || before[from..to].iter().copied().filter(|v| !v.is_nan());
    let count = known().count();
    if len < 8 || count < 8 {
        return None;
    }
    let mean = known().sum::<f32>() / count as f32;
    let variance = known().map(|v| (v - mean) * (v - mean)).sum::<f32>() / count as f32;
    if variance < MIN_VARIANCE {
        return None;
    }
    let search = search as isize;
    scores.clear();
    for s in -search..=search {
        scores.push(correlation(before, after, from, to, s));
    }
    let (best, peak) = scores
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.map(|s| (i, s)))
        .max_by(|a, b| a.1.total_cmp(&b.1))?;
    if peak < MIN_CORRELATION {
        return None;
    }
    let runner_up = scores
        .iter()
        .enumerate()
        .filter(|(i, _)| i.abs_diff(best) > 2)
        .filter_map(|(_, s)| *s)
        .fold(f32::NEG_INFINITY, f32::max);
    if peak - runner_up < MIN_UNIQUENESS {
        return None;
    }
    // A peak at the edge of the search is not a peak.
    if best == 0 || best + 1 == scores.len() {
        return None;
    }
    let (l, r) = (scores[best - 1]?, scores[best + 1]?);
    let curvature = l - 2.0 * peak + r;
    let offset = if curvature < 0.0 {
        (0.5 * (l - r) / curvature).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    let whole = best as f32 - search as f32;
    Some(Shift {
        at: refine(before, after, from, to, whole + offset, whole),
        correlation: peak,
    })
}

/// The sub-pixel shift, from `start` by gradient steps (Lucas-Kanade on the profiles): a
/// parabola through three correlation samples pulls shifts under half a pixel toward the
/// nearest whole one, which would teach the model too small a gain for slow turns. Stays
/// within a pixel of `whole`, the best whole shift, and keeps `start` when the profile has no
/// slope to follow.
fn refine(before: &[f32], after: &[f32], from: usize, to: usize, start: f32, whole: f32) -> f32 {
    // `after` at fractional index `x`, linearly interpolated; NaN outside or where unknown.
    let sample = |x: f32| -> f32 {
        if x < 0.0 || x > (after.len() - 1) as f32 {
            return f32::NAN;
        }
        let i = (x.floor() as usize).min(after.len() - 1);
        let j = (i + 1).min(after.len() - 1);
        let t = x - i as f32;
        after[i] * (1.0 - t) + after[j] * t
    };
    let mut at = start;
    for _ in 0..4 {
        let (mut num, mut den) = (0.0f32, 0.0f32);
        for (i, &a) in before.iter().enumerate().take(to).skip(from) {
            let x = i as f32 + at;
            let (b, slope) = (sample(x), 0.5 * (sample(x + 1.0) - sample(x - 1.0)));
            if a.is_nan() || b.is_nan() || slope.is_nan() {
                continue;
            }
            num += slope * (a - b);
            den += slope * slope;
        }
        if den <= f32::EPSILON {
            return start;
        }
        let step = (num / den).clamp(-0.5, 0.5);
        at = (at + step).clamp(whole - 1.0, whole + 1.0);
        if step.abs() < 0.005 {
            break;
        }
    }
    at
}

/// Normalised correlation of `before[from..to]` with `after[from + s..to + s]`, over the part
/// that stays inside `after` and saw some of the scene in both; `None` when that is less than
/// 60% of the cell.
fn correlation(before: &[f32], after: &[f32], from: usize, to: usize, s: isize) -> Option<f32> {
    let lo = (from as isize).max(-s).max(0) as usize;
    let hi = ((to as isize).min(after.len() as isize - s)).max(0) as usize;
    if hi <= lo {
        return None;
    }
    // The pairs where both profiles saw some of the scene.
    let (a, b) = (
        &before[lo..hi],
        &after[(lo as isize + s) as usize..(hi as isize + s) as usize],
    );
    let pairs = || {
        a.iter()
            .zip(b)
            .map(|(&a, &b)| (a, b))
            .filter(|(a, b)| !a.is_nan() && !b.is_nan())
    };
    let (mut count, mut sa, mut sb) = (0usize, 0.0f32, 0.0f32);
    for (a, b) in pairs() {
        count += 1;
        sa += a;
        sb += b;
    }
    if count * 10 < (to - from) * 6 {
        return None;
    }
    let n = count as f32;
    let (ma, mb) = (sa / n, sb / n);
    let (mut ab, mut aa, mut bb) = (0.0f32, 0.0f32, 0.0f32);
    for (a, b) in pairs() {
        let (a, b) = (a - ma, b - mb);
        ab += a * b;
        aa += a * a;
        bb += b * b;
    }
    if aa <= f32::EPSILON || bb <= f32::EPSILON {
        return None;
    }
    Some(ab / (aa * bb).sqrt())
}

fn median(values: &[f32]) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    // At most a value per cell here: sorted on the stack.
    let mut buffer = [0.0f32; GRID * GRID];
    let mut owned = Vec::new();
    let sorted = if values.len() <= buffer.len() {
        let sorted = &mut buffer[..values.len()];
        sorted.copy_from_slice(values);
        sorted
    } else {
        owned.extend_from_slice(values);
        owned.as_mut_slice()
    };
    sorted.sort_by(f32::total_cmp);
    let mid = sorted.len() / 2;
    Some(if sorted.len() % 2 == 1 {
        sorted[mid]
    } else {
        0.5 * (sorted[mid - 1] + sorted[mid])
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Value noise: a hashed lattice every `cell` pixels, bilinear between, 0 to 1.
    fn noise(u: f32, v: f32, cell: f32, seed: u32) -> f32 {
        let lattice = |x: i32, y: i32| {
            let mut h = (x as u32).wrapping_mul(0x8da6_b343)
                ^ (y as u32).wrapping_mul(0xd816_3841)
                ^ seed.wrapping_mul(0xcb1a_b31f);
            h ^= h >> 13;
            h = h.wrapping_mul(0x5bd1_e995);
            h ^= h >> 15;
            (h & 0xffff) as f32 / 65535.0
        };
        let (fu, fv) = (u / cell, v / cell);
        let (x0, y0) = (fu.floor(), fv.floor());
        let (tx, ty) = (fu - x0, fv - y0);
        let (x0, y0) = (x0 as i32, y0 as i32);
        let top = lattice(x0, y0) * (1.0 - tx) + lattice(x0 + 1, y0) * tx;
        let bottom = lattice(x0, y0 + 1) * (1.0 - tx) + lattice(x0 + 1, y0 + 1) * tx;
        top * (1.0 - ty) + bottom * ty
    }

    /// A textured scene without repeats, sampled at `(x + ox, y + oy)` so the content moves
    /// by `(-ox, -oy)`.
    pub(crate) fn scene(width: usize, height: usize, ox: f32, oy: f32, at_us: u64) -> Thumbnail {
        let mut luma = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let (u, v) = (x as f32 + ox, y as f32 + oy);
                let value = 128.0
                    + 70.0 * (noise(u, v, 4.0, 1) - 0.5)
                    + 60.0 * (noise(u, v, 9.0, 2) - 0.5)
                    + 50.0 * (noise(u, v, 23.0, 3) - 0.5);
                luma.push(value.clamp(0.0, 255.0) as u8);
            }
        }
        Thumbnail {
            width,
            height,
            luma,
            at_us,
        }
    }

    #[test]
    fn finds_a_pan_in_both_directions() {
        for (ox, oy) in [
            (6.0, 0.0),
            (-11.0, 3.0),
            (0.0, -7.0),
            (23.0, -9.0),
            (2.5, 1.5),
        ] {
            let a = scene(256, 144, 0.0, 0.0, 0);
            let b = scene(256, 144, ox, oy, 1);
            let m = estimate(&a, &b).unwrap_or_else(|| panic!("no estimate for {ox},{oy}"));
            assert!((m.dx + ox).abs() < 0.6, "dx {m:?} for {ox}");
            assert!((m.dy + oy).abs() < 0.6, "dy {m:?} for {oy}");
            assert!(m.confidence > 0.5, "{m:?}");
        }
    }

    #[test]
    fn slow_turns_are_measured_to_a_tenth_of_a_pixel() {
        for (ox, oy) in [
            (0.2f32, 0.0f32),
            (0.0, 0.3),
            (0.0, -0.15),
            (0.45, -0.4),
            (1.3, 0.7),
        ] {
            let a = scene(256, 144, 0.0, 0.0, 0);
            let b = scene(256, 144, ox, oy, 1);
            let m = estimate(&a, &b).unwrap_or_else(|| panic!("no estimate for {ox},{oy}"));
            assert!((m.dx + ox).abs() < 0.1, "dx {m:?} for {ox}");
            assert!((m.dy + oy).abs() < 0.1, "dy {m:?} for {oy}");
        }
    }

    #[test]
    fn a_slow_tilt_over_upright_edges_is_not_a_still_picture() {
        // Poles and walls: strong upright edges, which a tilt leaves unchanged, over texture.
        let upright = |oy: f32| {
            let mut t = scene(256, 144, 0.0, oy, 0);
            for (i, v) in t.luma.iter_mut().enumerate() {
                // Irregular widths: a periodic pattern has no one shift.
                if ((i % 256 / 5) as u32).wrapping_mul(2_654_435_761) >> 31 == 1 {
                    *v = v.saturating_add(70);
                }
            }
            t
        };
        let m = estimate(&upright(0.0), &upright(0.25)).unwrap();
        assert!((m.dy + 0.25).abs() < 0.1, "{m:?}");
    }

    #[test]
    fn still_pictures_have_no_motion() {
        let a = scene(256, 144, 0.0, 0.0, 0);
        let m = estimate(&a, &a.clone()).unwrap();
        assert!(m.dx.abs() < 0.1 && m.dy.abs() < 0.1, "{m:?}");
    }

    #[test]
    fn a_static_hud_cell_does_not_move_the_median() {
        let a = scene(256, 144, 0.0, 0.0, 0);
        let mut b = scene(256, 144, 9.0, 0.0, 1);
        // Copy the top-left cell unchanged, as a HUD would stay.
        for y in 0..48 {
            for x in 0..85 {
                b.luma[y * 256 + x] = a.luma[y * 256 + x];
            }
        }
        let m = estimate(&a, &b).unwrap();
        assert!((m.dx + 9.0).abs() < 0.6, "{m:?}");
    }

    /// Where [`with_hud`] draws: wide bars at the bottom left and right and a box at the top
    /// right.
    fn hud_at(x: usize, y: usize) -> bool {
        ((106..122).contains(&y) && !(104..166).contains(&x))
            || ((12..52).contains(&y) && (190..246).contains(&x))
    }

    /// `t` with a static HUD as a game draws one, the same in every capture.
    fn with_hud(mut t: Thumbnail) -> Thumbnail {
        for y in 0..t.height {
            for x in 0..t.width {
                if !hud_at(x, y) {
                    continue;
                }
                let bar = (110..118).contains(&y) && !(100..170).contains(&x);
                t.luma[y * t.width + x] = if bar {
                    235
                } else if (x + y) % 6 < 3 {
                    40
                } else {
                    20
                };
            }
        }
        t
    }

    #[test]
    fn a_known_hud_does_not_hold_back_a_tilt() {
        let hud: Vec<bool> = (0..256 * 144).map(|i| hud_at(i % 256, i / 256)).collect();
        for oy in [0.4f32, 0.8, 1.5, -1.2, 3.0] {
            let a = with_hud(scene(256, 144, 0.0, 0.0, 0));
            let b = with_hud(scene(256, 144, 4.0, oy, 1));
            let m = estimate_around(&a, &b, Some(&hud))
                .unwrap_or_else(|| panic!("no estimate for {oy}"));
            assert!((m.dy + oy).abs() < 0.35, "dy {m:?} for {oy}");
            assert!((m.dx + 4.0).abs() < 0.35, "dx {m:?} for {oy}");
        }
    }

    #[test]
    fn a_still_camera_with_one_thing_moving_stands_still() {
        let a = with_hud(scene(256, 144, 0.0, 0.0, 0));
        let mut b = a.clone();
        // Something crosses the middle cell; the rest of the picture holds.
        let mover = scene(256, 144, 7.0, 2.0, 1);
        for y in 55..90 {
            for x in 100..160 {
                b.luma[y * 256 + x] = mover.luma[y * 256 + x];
            }
        }
        let m = estimate(&a, &b).unwrap();
        assert_eq!((m.dx, m.dy), (0.0, 0.0), "{m:?}");
        assert!(m.confidence > 0.5, "{m:?}");
    }

    #[test]
    fn flat_pictures_and_mismatched_sizes_give_nothing() {
        let flat = Thumbnail {
            width: 64,
            height: 36,
            luma: vec![90; 64 * 36],
            at_us: 0,
        };
        assert_eq!(estimate(&flat, &flat), None);
        let a = scene(64, 36, 0.0, 0.0, 0);
        let b = scene(64, 40, 0.0, 0.0, 0);
        assert_eq!(estimate(&a, &b), None);
    }
}
