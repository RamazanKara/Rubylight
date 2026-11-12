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
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Motion {
    pub dx: f32,
    pub dy: f32,
    /// 0 to 1: how many cells agree with the median, and how clearly they found it.
    pub confidence: f32,
    /// Each grid cell's own shift where it found one clearly, rows top to bottom and columns
    /// left to right ([`cell_spans`] says where they are). A camera turn moves a perspective
    /// picture's edges farther than its centre, which the cells show and the median hides
    /// ([`super::lens`]).
    pub cells: [Option<CellShift>; CELLS],
}

/// One grid cell's shift, in thumbnail pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellShift {
    pub dx: f32,
    pub dy: f32,
    pub correlation: f32,
    /// Where `dx` and where `dy` were measured; `None` for the whole cell alike.
    pub place: Option<[Place; 2]>,
}

/// Where one of a cell's shifts was measured, in thumbnail pixels from the thumbnail's centre. A
/// profile is followed by its slopes, each weighing in with its square, so a cell that is half
/// sky moves as its textured half does, a picture stretched toward its edge by a lens as the
/// less stretched part of the cell, and a cell half under a HUD as the half beside it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    /// The slopes' mean place along the shift's axis, and its mean square.
    pub along: f32,
    pub squared: f32,
    /// The mean place, across that axis, of the pixels behind the slopes, and the mean of the
    /// two places' product.
    pub across: f32,
    pub cross: f32,
}

impl Motion {
    /// A shift every cell shares, as a flat picture's is.
    pub fn uniform(dx: f32, dy: f32, confidence: f32) -> Self {
        Self {
            dx,
            dy,
            confidence,
            cells: [Some(CellShift {
                dx,
                dy,
                correlation: confidence,
                place: None,
            }); CELLS],
        }
    }
}

/// The thumbnail's longer side, as the GPU makes it.
pub const THUMBNAIL_WIDTH: usize = 256;
const MIN_EDGE: usize = 32;
pub const GRID: usize = 3;
/// Cells in the grid.
pub const CELLS: usize = GRID * GRID;
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

/// A pixel that stays within this many luma codes of what it was has not changed: a HUD's
/// pixel in the thumbnail is the mean of many, so a game's dither over it comes to less.
const STILL_CHANGE: u8 = 2;
/// A move that should have changed a pixel by at least this much tells whether the pixel moved
/// along; less is within [`STILL_CHANGE`] and the capture's noise.
const STILL_STEP: f32 = 3.0;
/// A pixel stands still once the moves since it last changed should have changed it by this
/// many luma codes in all, of which one move counts for a quarter at most: where the measured
/// shift is wrong for once (a thin pole that passes over a pixel of sky), it is not four times.
const STILL_TOTAL: f32 = 24.0;
/// Moves shorter than this (thumbnail pixels) say nothing about what stands still.
const STILL_MIN_MOVE: f32 = 1.0;

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
    /// Where across each of those profiles' rows its pixels lie ([`Lines::across`]).
    before_across: [Vec<f32>; GRID],
    buffers: Buffers,
}

#[derive(Debug, Default)]
struct Buffers {
    before: Vec<f32>,
    across: Vec<f32>,
    after: Vec<f32>,
    scores: Vec<Option<f32>>,
    lines: Lines,
}

/// What a profile is built in ([`profile_rows`], [`profile_columns`]).
#[derive(Debug, Default)]
struct Lines {
    line: Vec<f32>,
    /// Per sample of the profile: its pixels' sum, their count and the sum of their places
    /// across the profile.
    sums: Vec<(f32, u32, f32)>,
    prefix: Vec<f32>,
    count: Vec<u32>,
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

/// What stands still in a picture that moves: a HUD's frames, bars and letters, down to the
/// thumbnail's pixel. The host's HUD mask ([`super::Mask`]) is what the client holds still, so
/// it takes only cells that are HUD all over and textured, and leaves out a HUD's rim and its
/// plain parts; and a client may want none. Left in the measurement, a minimap's frame or a
/// health bar's edge in a corner cell holds that cell's shift back, and the corners are where
/// a lens shows most ([`super::lens`]).
///
/// A pixel is told by what the moves should have done to it: each brings it what its cell's
/// shift says lay that far away, and what that would change it by adds up, one way or the
/// other, for as long as the pixel stays what it was. One that should have changed by
/// [`STILL_TOTAL`] and has not stands still, until it changes. A pixel the moves bring more of
/// the same (a sky, a horizon while the camera pans, the middle of a plain panel) is not told
/// and not marked.
#[derive(Debug, Default)]
pub struct Still {
    size: (usize, usize),
    /// Per pixel: what it was when it last changed, and what the moves since should have
    /// changed it by.
    pixels: Vec<(u8, f32)>,
    /// Per pixel: whether it or a neighbour stands still (a pixel on a HUD's rim is part HUD).
    mask: Vec<bool>,
    marked: usize,
}

impl Still {
    /// Takes the move `motion` from `prev` to `cur`. Returns whether the mask changed.
    pub fn observe(&mut self, prev: &Thumbnail, cur: &Thumbnail, motion: &Motion) -> bool {
        let (w, h) = (cur.width, cur.height);
        if !prev.valid() || !cur.valid() || prev.width != w || prev.height != h {
            return false;
        }
        if self.size != (w, h) {
            self.size = (w, h);
            self.pixels.clear();
            self.pixels
                .extend(prev.luma.iter().map(|value| (*value, 0.0)));
            self.mask.clear();
            self.mask.resize(w * h, false);
            self.marked = 0;
        }
        let (x_edges, y_edges) = (edges(w), edges(h));
        // A cell's pixels, and the margin beside an outer cell with it.
        let span = |edges: &[usize; GRID + 1], cell: usize, len: usize| {
            let from = if cell == 0 { 0 } else { edges[cell] };
            let to = if cell + 1 == GRID {
                len
            } else {
                edges[cell + 1]
            };
            from..to
        };
        let mut changed = false;
        for row in 0..GRID {
            for column in 0..GRID {
                // The cell's own shift while it goes with the picture's: a lens moves a corner
                // up to half again as far as the median cell. A cell that is mostly HUD finds
                // no shift, or the HUD's, which is none.
                let whole = motion.dx.hypot(motion.dy);
                let (dx, dy) = match motion.cells[row * GRID + column] {
                    Some(cell)
                        if (cell.dx - motion.dx).hypot(cell.dy - motion.dy)
                            <= 0.5 + 0.6 * whole =>
                    {
                        (cell.dx, cell.dy)
                    }
                    _ => (motion.dx, motion.dy),
                };
                let moved = dx.hypot(dy) >= STILL_MIN_MOVE;
                for y in span(&y_edges, row, h) {
                    for x in span(&x_edges, column, w) {
                        let at = y * w + x;
                        let (was, should) = &mut self.pixels[at];
                        let stood = should.abs() >= STILL_TOTAL;
                        if was.abs_diff(cur.luma[at]) > STILL_CHANGE {
                            (*was, *should) = (cur.luma[at], 0.0);
                        } else if moved
                            && let Some(brought) = sample(prev, x as f32 - dx, y as f32 - dy)
                        {
                            let step = brought - f32::from(prev.luma[at]);
                            if step.abs() >= STILL_STEP {
                                let most = STILL_TOTAL * 0.25;
                                *should += step.clamp(-most, most);
                            }
                        }
                        changed |= stood != (should.abs() >= STILL_TOTAL);
                    }
                }
            }
        }
        if changed {
            self.mask.fill(false);
            self.marked = 0;
            for y in 0..h {
                for x in 0..w {
                    if self.pixels[y * w + x].1.abs() >= STILL_TOTAL {
                        self.marked += 1;
                        for row in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                            let row = row * w;
                            self.mask[row + x.saturating_sub(1)..=row + (x + 1).min(w - 1)]
                                .fill(true);
                        }
                    }
                }
            }
        }
        changed
    }

    /// Per thumbnail pixel, row by row, whether it stands still; `None` while none does.
    pub fn mask(&self) -> Option<&[bool]> {
        (self.marked > 0).then_some(&self.mask[..])
    }

    /// Forgets what stood still.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// The thumbnail at a place between its pixels; `None` outside it.
fn sample(t: &Thumbnail, x: f32, y: f32) -> Option<f32> {
    if x < 0.0 || y < 0.0 || x > (t.width - 1) as f32 || y > (t.height - 1) as f32 {
        return None;
    }
    let (x0, y0) = (x as usize, y as usize);
    let (x1, y1) = ((x0 + 1).min(t.width - 1), (y0 + 1).min(t.height - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let top = t.at(x0, y0) * (1.0 - fx) + t.at(x1, y0) * fx;
    let bottom = t.at(x0, y1) * (1.0 - fx) + t.at(x1, y1) * fx;
    Some(top * (1.0 - fy) + bottom * fy)
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
            cells: [None; CELLS],
        });
    }

    // Vertical first: one row profile per column band, full height, correlated per row band.
    // Then horizontal: one column profile per row band, the current one taken that band's
    // vertical shift lower so a tilt does not blur it. Then vertical again, the current
    // column band taken that band's horizontal shift across, so a fast pan does not blur it.
    let Scratch {
        before_rows,
        before_across,
        buffers,
    } = scratch;
    for (band, (before, across)) in before_rows.iter_mut().zip(&mut *before_across).enumerate() {
        let (x0, x1) = (x_edges[band], x_edges[band + 1]);
        profile_rows(prev, x0, x1, 0, scene, &mut buffers.lines, before);
        buffers.lines.across(across);
    }
    let before_rows: [(&[f32], &[f32]); GRID] =
        core::array::from_fn(|band| (&before_rows[band][..], &before_across[band][..]));
    let first: [[Option<Shift>; GRID]; GRID] = core::array::from_fn(|band| {
        vertical_band(cur, scene, &grid, band, before_rows[band], 0, buffers)
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
            across,
            after,
            scores,
            lines,
        } = &mut *buffers;
        profile_columns(prev, y0, y1, 0, scene, lines, before);
        lines.across(across);
        profile_columns(cur, y0, y1, shift_y, scene, lines, after);
        for (band, column) in horizontal.iter_mut().enumerate() {
            column[cell] = best_shift(
                (before, across),
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
                before_rows[band],
                column_shift[band],
                buffers,
            );
        }
    }
    let mut cells = Vec::with_capacity(GRID * GRID);
    let mut each = [None; CELLS];
    for band in 0..GRID {
        for cell in 0..GRID {
            if let (Some(x), Some(y)) = (horizontal[band][cell], vertical[band][cell]) {
                let correlation = x.correlation.min(y.correlation);
                cells.push((x.at, y.at, correlation));
                each[cell * GRID + band] = Some(CellShift {
                    dx: x.at,
                    dy: y.at,
                    correlation,
                    place: x.place.zip(y.place).map(|(x, y)| [x, y]),
                });
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
        cells: each,
    })
}

/// Where the grid's cells are along an edge of `len` thumbnail pixels: each cell's first pixel
/// and the one past its last.
pub fn cell_spans(len: usize) -> [(usize, usize); GRID] {
    let edges = edges(len);
    core::array::from_fn(|i| (edges[i], edges[i + 1]))
}

/// The vertical shift of each cell in column band `band`: `before` (the previous thumbnail's
/// detrended row profile over the band) found in the current one's, its columns taken
/// `column_shift` across.
fn vertical_band<S: Fn(usize, usize) -> bool + Copy>(
    cur: &Thumbnail,
    scene: S,
    grid: &Grid,
    band: usize,
    before: (&[f32], &[f32]),
    column_shift: isize,
    buffers: &mut Buffers,
) -> [Option<Shift>; GRID] {
    let Buffers {
        after,
        scores,
        lines,
        ..
    } = buffers;
    let (x0, x1) = (grid.x_edges[band], grid.x_edges[band + 1]);
    profile_rows(cur, x0, x1, column_shift, scene, lines, after);
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

/// Into `out`: each row's profile over columns `x0..x1`, the columns taken `shift` to the right
/// (clamped at the edges): the mean of its pixels where `scene` (not the HUD), each less its
/// own column's moving average ([`detrend`]); NaN for a row of HUD only. Taking the average out
/// column by column leaves the profile of a whole picture as taking it out of the rows' means
/// would, and keeps a HUD from marking it: rows that a HUD covers half of are the mean of other
/// columns than their neighbours, at another level, and that step, standing where the HUD
/// stands in every capture, would hold the measured shift back.
fn profile_rows<S: Fn(usize, usize) -> bool>(
    t: &Thumbnail,
    x0: usize,
    x1: usize,
    shift: isize,
    scene: S,
    lines: &mut Lines,
    out: &mut Vec<f32>,
) {
    let last = t.width as isize - 1;
    let Lines {
        line,
        sums,
        prefix,
        count,
    } = lines;
    sums.clear();
    sums.resize(t.height, (0.0, 0, 0.0));
    for x in x0..x1 {
        let x = (x as isize + shift).clamp(0, last) as usize;
        line.clear();
        line.extend((0..t.height).map(|y| if scene(x, y) { t.at(x, y) } else { f32::NAN }));
        detrend(line, prefix, count);
        gather(line, x as f32 + 0.5 - t.width as f32 * 0.5, sums);
    }
    means(sums, out);
}

/// Into `out`: each column's profile over rows `y0..y1`, the rows taken `shift` lower (clamped
/// at the edges), as [`profile_rows`] with each pixel less its own row's moving average; NaN
/// for a column of HUD only.
fn profile_columns<S: Fn(usize, usize) -> bool>(
    t: &Thumbnail,
    y0: usize,
    y1: usize,
    shift: isize,
    scene: S,
    lines: &mut Lines,
    out: &mut Vec<f32>,
) {
    let Lines {
        line,
        sums,
        prefix,
        count,
    } = lines;
    sums.clear();
    sums.resize(t.width, (0.0, 0, 0.0));
    for y in y0..y1 {
        let y = (y as isize + shift).clamp(0, t.height as isize - 1) as usize;
        line.clear();
        line.extend((0..t.width).map(|x| if scene(x, y) { t.at(x, y) } else { f32::NAN }));
        detrend(line, prefix, count);
        gather(line, y as f32 + 0.5 - t.height as f32 * 0.5, sums);
    }
    means(sums, out);
}

/// Adds a detrended line's known samples to the profile's sums; `at` is the line's place
/// across the profile, from the thumbnail's centre.
fn gather(line: &[f32], at: f32, sums: &mut [(f32, u32, f32)]) {
    for (value, (sum, n, across)) in line.iter().zip(sums) {
        if !value.is_nan() {
            *sum += value;
            *n += 1;
            *across += at;
        }
    }
}

fn means(sums: &[(f32, u32, f32)], out: &mut Vec<f32>) {
    out.clear();
    out.extend(
        sums.iter()
            .map(|&(sum, n, _)| if n == 0 { f32::NAN } else { sum / n as f32 }),
    );
}

impl Lines {
    /// Into `out`: where across the profile built last each sample's pixels lie on average,
    /// from the thumbnail's centre; NaN for a sample of HUD only.
    fn across(&self, out: &mut Vec<f32>) {
        out.clear();
        out.extend(self.sums.iter().map(
            |&(_, n, across)| {
                if n == 0 { f32::NAN } else { across / n as f32 }
            },
        ));
    }
}

/// A line of the picture, in place, less its moving average over 13 samples: lighting and slow
/// gradients out, edges and texture kept. NaN samples (HUD) stay NaN, and so does a sample with
/// one among those 13: its average would lean to the side away from the HUD, and what a slow
/// gradient leaves of itself there stands where the HUD stands in every capture, holding the
/// measured shift back as the HUD itself would. (At the picture's own edge the average leans
/// too; the cells keep a margin from it.) `prefix` and `count` are scratch.
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
        *value = if known as usize == b - a {
            *value - (prefix[b] - prefix[a]) / known as f32
        } else {
            f32::NAN
        };
    }
}

#[derive(Clone, Copy, Debug)]
struct Shift {
    /// Sub-pixel shift: `after[i + at] ~ before[i]`.
    at: f32,
    correlation: f32,
    /// Where the shift was measured.
    place: Option<Place>,
}

/// The shift of `before[from..to]` within `after`, searched over `-search..=search`, by
/// normalised correlation and refined to sub-pixel by a parabola through the peak. `None` for
/// a flat cell, a weak or an ambiguous peak. `before` comes with where across it its samples'
/// pixels lie ([`Lines::across`]); `scores` is scratch.
fn best_shift(
    (before, across): (&[f32], &[f32]),
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
        place: place(before, across, from, to),
    })
}

/// Where `refine` measures the shift of `before[from..to]`: its steps weigh each sample with its
/// squared slope, so the place is those weights' mean, along the profile from its middle and
/// across it as `across` has each sample. `None` for a profile without slopes.
fn place(before: &[f32], across: &[f32], from: usize, to: usize) -> Option<Place> {
    let middle = before.len() as f32 * 0.5;
    let mut sums = [0.0f64; 5];
    for i in from.max(1)..to.min(before.len().saturating_sub(1)) {
        let slope = before[i + 1] - before[i - 1];
        let beside = across.get(i).copied().unwrap_or(f32::NAN);
        if slope.is_nan() || beside.is_nan() {
            continue;
        }
        let (weight, at, beside) = (
            f64::from(slope * slope),
            f64::from(i as f32 + 0.5 - middle),
            f64::from(beside),
        );
        for (sum, term) in sums.iter_mut().zip([1.0, at, at * at, beside, at * beside]) {
            *sum += weight * term;
        }
    }
    let [weight, along, squared, beside, cross] = sums;
    (weight > f64::from(f32::EPSILON)).then(|| Place {
        along: (along / weight) as f32,
        squared: (squared / weight) as f32,
        across: (beside / weight) as f32,
        cross: (cross / weight) as f32,
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
    pub(crate) fn noise(u: f32, v: f32, cell: f32, seed: u32) -> f32 {
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

    /// The textured world's luma at `(u, v)`.
    pub(crate) fn world(u: f32, v: f32) -> u8 {
        let value = 128.0
            + 70.0 * (noise(u, v, 4.0, 1) - 0.5)
            + 60.0 * (noise(u, v, 9.0, 2) - 0.5)
            + 50.0 * (noise(u, v, 23.0, 3) - 0.5);
        value.clamp(0.0, 255.0) as u8
    }

    /// A textured scene without repeats, sampled at `(x + ox, y + oy)` so the content moves
    /// by `(-ox, -oy)`.
    pub(crate) fn scene(width: usize, height: usize, ox: f32, oy: f32, at_us: u64) -> Thumbnail {
        let mut luma = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                luma.push(world(x as f32 + ox, y as f32 + oy));
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
    fn a_cell_is_measured_where_its_texture_is() {
        // A profile flat but for one ramp: its slopes, and so its shift, are the ramp's.
        let mut profile = vec![0.0f32; 100];
        for (i, value) in profile.iter_mut().enumerate().take(70).skip(60) {
            *value = (i - 60) as f32;
        }
        for value in profile.iter_mut().skip(70) {
            *value = 10.0;
        }
        let beside = [7.0f32; 100];
        let found = place(&profile, &beside, 40, 90).unwrap();
        let (mean, squared) = (found.along, found.squared);
        assert!((found.across - 7.0).abs() < 1e-4 && (found.cross - 7.0 * mean).abs() < 1e-2);
        // The ramp runs from sample 60 to 70, 10 to 20 past the middle.
        assert!((mean - 15.0).abs() < 0.6, "{mean}");
        assert!((squared - 15.0 * 15.0).abs() < 25.0, "{squared}");
        // Textured alike all over, it is the cell's middle; unknown samples are left out.
        let even: Vec<f32> = (0..100)
            .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 } * (i % 4) as f32)
            .collect();
        let mean = place(&even, &beside, 40, 90).unwrap().along;
        assert!((mean - 15.0).abs() < 1.5, "{mean}");
        let mut holed = profile.clone();
        holed[65] = f32::NAN;
        assert!(place(&holed, &beside, 40, 90).is_some());
        assert_eq!(place(&[0.0; 100], &beside, 40, 90), None);
        // A measured move carries each cell's place.
        let motion =
            estimate(&scene(256, 144, 0.0, 0.0, 0), &scene(256, 144, 3.0, 0.0, 1)).unwrap();
        let [x, y] = motion.cells[0].unwrap().place.unwrap();
        assert!(
            x.along < -40.0 && y.along < -20.0 && x.squared > x.along * x.along * 0.9,
            "{x:?} {y:?}"
        );
        // Each across the other's axis in the cell's middle: all of it is scene.
        assert!(
            (x.across + 44.0).abs() < 3.0 && (y.across + 78.0).abs() < 3.0,
            "{x:?} {y:?}"
        );
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
    fn what_stands_still_while_the_picture_moves_is_found_and_let_go() {
        let near_hud = |x: usize, y: usize| {
            (y.saturating_sub(2)..=y + 2)
                .any(|y| (x.saturating_sub(2)..=x + 2).any(|x| hud_at(x, y)))
        };
        let mut still = Still::default();
        let (mut ox, mut oy) = (0.0f32, 0.0f32);
        let mut previous = with_hud(scene(256, 144, ox, oy, 0));
        // Pans with a little tilt, to and fro; then the HUD goes (a menu closes).
        for i in 1..=60u64 {
            ox += if (i / 10) % 2 == 0 { 3.3 } else { -2.7 };
            oy += if (i / 7) % 2 == 0 { 0.8 } else { -0.6 };
            let current = scene(256, 144, ox, oy, i);
            let current = if i <= 40 { with_hud(current) } else { current };
            let motion = estimate_around(&previous, &current, still.mask()).expect("a move");
            still.observe(&previous, &current, &motion);
            if i == 40 {
                let mask = still.mask().expect("a HUD");
                let marked = |wanted: bool| {
                    (0..256 * 144)
                        .filter(|i| hud_at(i % 256, i / 256) == wanted && mask[*i])
                        .count()
                };
                let hud = (0..256 * 144).filter(|i| hud_at(i % 256, i / 256)).count();
                // Its rims and its pattern; the middle of its plain bar says nothing.
                assert!(marked(true) * 10 >= hud * 8, "{} of {hud}", marked(true));
                // Of the scene, only the pixels at its rim.
                let astray = (0..256 * 144)
                    .filter(|i| mask[*i] && !near_hud(i % 256, i / 256))
                    .count();
                assert!(astray <= 8, "{astray} of the scene");
                // And the shift is the scene's, though the host was told of no HUD.
                assert!(
                    (motion.dx + 3.3).abs() < 0.35 && (motion.dy - 0.6).abs() < 0.35,
                    "{motion:?}"
                );
            }
            previous = current;
        }
        let left = still
            .mask()
            .map_or(0, |mask| mask.iter().filter(|m| **m).count());
        assert!(left <= 8, "{left} still marked");
    }

    #[test]
    fn a_pan_tells_nothing_of_a_horizon_or_a_sky() {
        // A plain sky over a plain strip of ground with a straight horizon between, texture
        // below: the pan brings every pixel of the upper two more of the same.
        let seen = |ox: f32, at_us: u64| {
            let mut t = scene(256, 144, ox, 0.0, at_us);
            for (i, value) in t.luma.iter_mut().enumerate().take(256 * 70) {
                *value = if i / 256 < 60 { 200 } else { 90 };
            }
            t
        };
        let mut still = Still::default();
        let mut previous = seen(0.0, 0);
        for i in 1..=60u64 {
            let current = seen(i as f32 * 3.5, i);
            let motion = estimate(&previous, &current).expect("a move");
            assert!(!still.observe(&previous, &current, &motion));
            previous = current;
        }
        assert_eq!(still.mask(), None);
    }

    #[test]
    fn a_line_is_not_measured_beside_a_hud() {
        // A slow gradient: all of it goes, but where the average would lean away from the HUD.
        let mut line: Vec<f32> = (0..40).map(|i| 3.0 * i as f32).collect();
        line[20] = f32::NAN;
        detrend(&mut line, &mut Vec::new(), &mut Vec::new());
        assert!(line[14..=26].iter().all(|v| v.is_nan()), "{line:?}");
        assert!(line[13].abs() < 1e-3 && line[27].abs() < 1e-3, "{line:?}");
        // At the picture's edge it leans and stays: the cells keep their margin from it.
        assert!((line[0] + 9.0).abs() < 1e-3, "{line:?}");

        // Rows half under a HUD are the mean of other columns than their neighbours, here
        // darker ones: no step for that in the profile.
        let t = Thumbnail {
            width: 64,
            height: 64,
            luma: (0..64 * 64)
                .map(|i| if i % 64 < 32 { 200 } else { 50 })
                .collect(),
            at_us: 0,
        };
        let hud = |x: usize, y: usize| x < 32 && (24..40).contains(&y);
        let (mut lines, mut profile) = (Lines::default(), Vec::new());
        profile_rows(&t, 0, 64, 0, |x, y| !hud(x, y), &mut lines, &mut profile);
        assert!(profile.iter().all(|v| v.abs() < 1e-3), "{profile:?}");
        // And those rows lie where their columns do.
        let mut across = Vec::new();
        lines.across(&mut across);
        assert!(
            (across[10] - 0.0).abs() < 1e-3 && (across[30] - 16.0).abs() < 1e-3,
            "{across:?}"
        );
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
