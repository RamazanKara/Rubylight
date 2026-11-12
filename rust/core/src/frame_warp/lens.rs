//! The game's field of view, learned from how a camera turn moves the picture.
//!
//! A game draws through a perspective camera, so a turn does not move its picture alike
//! everywhere. With the focal length `f` (the distance, in pixels, at which the picture's width
//! spans the horizontal field of view), a small turn that moves the centre by `(a, b)` moves the
//! point `(x, y)` from the centre by
//!
//! ```text
//! u = a (1 + x²/f²) + b xy/f²
//! v = b (1 + y²/f²) + a xy/f²
//! ```
//!
//! At a 90 degree field of view the left and right edges move twice as far as the centre. A shift
//! that is right in the middle of the picture is then far too short at its edges, and the median
//! of the grid's cells ([`super::motion`]), most of which lie off centre, is too long in the
//! middle. A flat picture (a map, a side-scroller) moves alike everywhere: `1/f²` is 0.
//!
//! Each measured move gives every grid cell's own shift, and where in the cell its profile
//! measured it ([`super::motion::Place`]): a lens stretches the picture toward its edge, so an
//! outer cell's texture, and with it the shift, lies nearer the middle than the cell does, by
//! enough to read a 90 degree lens as 78. For every candidate field of view the
//! cells are fitted with the camera's turn `(a, b)`, a zoom about the centre (walking forward)
//! and a roll, and the squared remainder is kept, fading over the last few seconds of turning.
//! The candidate that leaves the least wins once it leaves clearly less than a flat picture
//! does; until then, and for a game that pans a flat picture, there is no lens and the model's
//! shift stays a translation.

use super::motion::{CELLS, GRID, Motion, cell_spans};

/// Fields of view tried: 30 to 120 degrees across the picture's width, in steps of 2.
const FIRST_DEGREES: f64 = 30.0;
const STEP_DEGREES: f64 = 2.0;
const CANDIDATES: usize = 46;
/// What a move's remainders keep of their weight when the next move comes: half after about
/// 140 moves, a little over two seconds of turning at 60 captures a second.
const FADE: f64 = 0.995;
/// Moves (faded) and their squared length (thumbnail pixels, faded) before a lens is chosen.
const MIN_MOVES: f64 = 30.0;
const MIN_TRAVEL: f64 = 200.0;
/// A move shorter than this (thumbnail pixels at the median cell) says too little.
const MIN_MOVE: f32 = 1.0;
/// Cells a move needs to tell a lens from a zoom or a roll.
const MIN_CELLS: usize = 6;
/// A lens is taken when it leaves at most this share of what a flat picture leaves, and given
/// up when it leaves more than [`DROP`] of it.
const TAKE: f64 = 0.75;
const DROP: f64 = 0.9;
/// A lens in force gives way to one at least this many degrees away that leaves at most this
/// share of what it leaves: the estimate wobbles by a degree as moves fade.
const CHANGE_DEGREES: f64 = 1.5;
const CHANGE: f64 = 0.98;
/// A cell this far (thumbnail pixels, plus a tenth of the move) from where a candidate puts it
/// is left out of that candidate's fit, and no cell costs a candidate more than this far would:
/// something else moves there, and a move that is no camera turn at all (a cut, a menu opening)
/// then costs every candidate alike instead of choosing among them by chance.
const OUTLIER: f64 = 0.5;
/// Cells within this many thumbnail pixels of the fitted turn agree with it
/// ([`Layout::resolve`]), as cells within it of the median do in the flat estimate.
const AGREE: f64 = 1.5;

/// Where the grid's cells lie in the game's picture, for a thumbnail of some size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// Per cell (rows top to bottom, columns left to right), with positions from the centre in
    /// half picture widths: its centre and the means of x², y² and xy over it.
    cells: [Cell; CELLS],
    /// Picture pixels per thumbnail pixel, x and y.
    scale: [f64; 2],
    /// Half the picture's width in picture pixels.
    half_width: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Cell {
    x: f64,
    y: f64,
    xx: f64,
    yy: f64,
    xy: f64,
}

/// A camera turn read from a move's cells ([`Layout::resolve`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Turn {
    /// How far the picture's centre moved, in thumbnail pixels.
    pub dx: f32,
    pub dy: f32,
    /// 0 to 1, as [`Motion::confidence`].
    pub confidence: f32,
}

impl Layout {
    /// The layout of a `thumbnail` (width, height) of a game picture `content` pixels in size.
    pub fn new(content: (u16, u16), thumbnail: (usize, usize)) -> Self {
        let (tw, th) = (thumbnail.0.max(1), thumbnail.1.max(1));
        let scale = [
            f64::from(content.0.max(1)) / tw as f64,
            f64::from(content.1.max(1)) / th as f64,
        ];
        let half_width = f64::from(content.0.max(1)) * 0.5;
        // A thumbnail pixel edge's place from the centre, in half picture widths.
        let place = |edge: usize, len: usize, scale: f64| {
            (edge as f64 - len as f64 * 0.5) * scale / half_width
        };
        let (columns, rows) = (cell_spans(tw), cell_spans(th));
        let mut cells = [Cell::default(); CELLS];
        for (row, &(y0, y1)) in rows.iter().enumerate() {
            for (column, &(x0, x1)) in columns.iter().enumerate() {
                let (x0, x1) = (place(x0, tw, scale[0]), place(x1, tw, scale[0]));
                let (y0, y1) = (place(y0, th, scale[1]), place(y1, th, scale[1]));
                let (x, y) = ((x0 + x1) * 0.5, (y0 + y1) * 0.5);
                cells[row * GRID + column] = Cell {
                    x,
                    y,
                    xx: (x0 * x0 + x0 * x1 + x1 * x1) / 3.0,
                    yy: (y0 * y0 + y0 * y1 + y1 * y1) / 3.0,
                    xy: x * y,
                };
            }
        }
        Self {
            cells,
            scale,
            half_width,
        }
    }

    /// The focal length, in picture pixels, of a lens whose `tangent` is the squared tangent of
    /// half the horizontal field of view.
    pub fn focal(&self, tangent: f64) -> f64 {
        self.half_width / tangent.sqrt()
    }

    /// The squared tangent of half the field of view of a focal length in picture pixels.
    pub fn tangent(&self, focal: f64) -> f64 {
        let ratio = self.half_width / focal;
        ratio * ratio
    }

    /// The camera's turn in `motion`: under `lens` (the squared tangent of half the horizontal
    /// field of view), how far the picture's centre moved, fitted to the cells with a zoom and
    /// a roll when enough of them found a shift. Without a lens, or for a move without cells (a
    /// still picture), the flat estimate as it is.
    pub fn resolve(&self, motion: &Motion, lens: Option<f64>) -> Turn {
        let flat = Turn {
            dx: motion.dx,
            dy: motion.dy,
            confidence: motion.confidence,
        };
        let Some(lens) = lens else { return flat };
        let Some(fit) = self.fit(motion, lens, AGREE) else {
            return flat;
        };
        let agreeing = fit.agree.iter().filter(|agree| **agree).count();
        if agreeing < 3 {
            return flat;
        }
        let correlation: f64 = motion
            .cells
            .iter()
            .zip(fit.agree)
            .filter(|(_, agree)| *agree)
            .filter_map(|(cell, _)| cell.map(|cell| f64::from(cell.correlation)))
            .sum::<f64>()
            / agreeing as f64;
        Turn {
            dx: (fit.turn[0] / self.scale[0]) as f32,
            dy: (fit.turn[1] / self.scale[1]) as f32,
            confidence: ((agreeing as f64 / CELLS as f64) * correlation).clamp(0.0, 1.0) as f32,
        }
    }

    /// `motion`'s cells fitted under `lens`, leaving out up to two cells farther than
    /// `tolerance` thumbnail pixels (plus a tenth of the move) from where the fit puts them.
    /// `None` with fewer than three cells.
    fn fit(&self, motion: &Motion, lens: f64, tolerance: f64) -> Option<Fitted> {
        let mut used: [bool; CELLS] = core::array::from_fn(|i| motion.cells[i].is_some());
        let tolerance = (tolerance + 0.1 * f64::from(motion.dx.hypot(motion.dy))) * self.scale[0];
        let mut dropped = 0;
        loop {
            let count = used.iter().filter(|used| **used).count();
            if count < 3 {
                return None;
            }
            // With few cells a zoom and a roll would fit anything.
            let terms = if count >= 5 { 4 } else { 2 };
            let solution = self.solve(motion, lens, &used, terms)?;
            let (mut worst, mut worst_at, mut rest) = (0.0f64, None, 0.0f64);
            let mut agree = [false; CELLS];
            for (i, _) in used.iter().enumerate().filter(|(_, used)| **used) {
                let r = self.remainder(motion, lens, i, &solution);
                let squared = r[0] * r[0] + r[1] * r[1];
                rest += squared.min(tolerance * tolerance);
                agree[i] = squared <= tolerance * tolerance;
                if squared > worst {
                    (worst, worst_at) = (squared, Some(i));
                }
            }
            match worst_at {
                Some(i) if worst > tolerance * tolerance && dropped < 2 && count > 3 => {
                    used[i] = false;
                    dropped += 1;
                }
                _ => {
                    return Some(Fitted {
                        turn: [solution[0], solution[1]],
                        agree,
                        // A cell left out costs what it may at most be off by.
                        rest: rest + dropped as f64 * tolerance * tolerance,
                    });
                }
            }
        }
    }

    /// Cell `i`'s rows of the fit: its shift in picture pixels, and what each unknown (turn x,
    /// turn y, zoom, roll) contributes to it.
    fn rows(&self, motion: &Motion, lens: f64, i: usize) -> Option<([f64; 2], [[f64; 4]; 2])> {
        let shift = motion.cells[i]?;
        let c = &self.cells[i];
        // Each shift where its profile measured it ([`super::motion::Place`]).
        let (kx, ky) = (
            self.scale[0] / self.half_width,
            self.scale[1] / self.half_width,
        );
        let terms = match shift.place {
            Some([x, y]) => {
                let at = |place: f32, scale: f64| f64::from(place) * scale;
                [
                    [
                        1.0 + lens * at(x.squared, kx * kx),
                        lens * at(x.cross, kx * ky),
                        at(x.along, kx),
                        -at(x.across, ky),
                    ],
                    [
                        lens * at(y.cross, kx * ky),
                        1.0 + lens * at(y.squared, ky * ky),
                        at(y.along, ky),
                        at(y.across, kx),
                    ],
                ]
            }
            None => [
                [1.0 + lens * c.xx, lens * c.xy, c.x, -c.y],
                [lens * c.xy, 1.0 + lens * c.yy, c.y, c.x],
            ],
        };
        Some((
            [
                f64::from(shift.dx) * self.scale[0],
                f64::from(shift.dy) * self.scale[1],
            ],
            terms,
        ))
    }

    fn remainder(&self, motion: &Motion, lens: f64, i: usize, solution: &[f64; 4]) -> [f64; 2] {
        let Some((shift, rows)) = self.rows(motion, lens, i) else {
            return [0.0; 2];
        };
        core::array::from_fn(|axis| {
            shift[axis]
                - rows[axis]
                    .iter()
                    .zip(solution)
                    .map(|(a, b)| a * b)
                    .sum::<f64>()
        })
    }

    /// Least squares over the `used` cells for the first `terms` unknowns; the rest are 0.
    fn solve(
        &self,
        motion: &Motion,
        lens: f64,
        used: &[bool; CELLS],
        terms: usize,
    ) -> Option<[f64; 4]> {
        let mut normal = [[0.0f64; 5]; 4];
        for (i, _) in used.iter().enumerate().filter(|(_, used)| **used) {
            let (shift, rows) = self.rows(motion, lens, i)?;
            for axis in 0..2 {
                for p in 0..terms {
                    for q in 0..terms {
                        normal[p][q] += rows[axis][p] * rows[axis][q];
                    }
                    normal[p][4] += rows[axis][p] * shift[axis];
                }
            }
        }
        // Gaussian elimination with partial pivoting.
        for p in 0..terms {
            let pivot =
                (p..terms).max_by(|a, b| normal[*a][p].abs().total_cmp(&normal[*b][p].abs()))?;
            if normal[pivot][p].abs() < 1e-9 {
                return None;
            }
            normal.swap(p, pivot);
            for q in 0..terms {
                if q != p {
                    let factor = normal[q][p] / normal[p][p];
                    let pivot_row = normal[p];
                    for (value, pivot) in normal[q].iter_mut().zip(pivot_row).skip(p) {
                        *value -= factor * pivot;
                    }
                }
            }
        }
        let mut solution = [0.0; 4];
        for p in 0..terms {
            solution[p] = normal[p][4] / normal[p][p];
        }
        Some(solution)
    }
}

struct Fitted {
    /// The centre's shift in picture pixels.
    turn: [f64; 2],
    /// The cells within the tolerance of the fit.
    agree: [bool; CELLS],
    /// The squared remainder, picture pixels.
    rest: f64,
}

/// The squared tangent of half a field of view of `degrees`.
fn tangent_of(degrees: f64) -> f64 {
    let half = degrees.to_radians() * 0.5;
    half.tan() * half.tan()
}

/// The squared tangent of half the field of view of candidate `index`.
fn candidate(index: usize) -> f64 {
    tangent_of(FIRST_DEGREES + STEP_DEGREES * index as f64)
}

/// Chooses the lens from the moves the camera's turns made.
#[derive(Clone, Debug, PartialEq)]
pub struct Lens {
    /// What a flat picture and each candidate left of the moves, faded.
    flat: f64,
    rest: [f64; CANDIDATES],
    moves: f64,
    travel: f64,
    /// The field of view in force, in degrees: between the candidates, where they leave least.
    chosen: Option<f64>,
    /// A lens from an earlier session, in force until the moves choose.
    prior: Option<f64>,
}

impl Default for Lens {
    fn default() -> Self {
        Self {
            flat: 0.0,
            rest: [0.0; CANDIDATES],
            moves: 0.0,
            travel: 0.0,
            chosen: None,
            prior: None,
        }
    }
}

impl Lens {
    /// Forgets the moves; the lens in force stays until new moves choose.
    pub fn clear(&mut self) {
        self.prior = self.tangent();
        self.flat = 0.0;
        self.rest = [0.0; CANDIDATES];
        self.moves = 0.0;
        self.travel = 0.0;
        self.chosen = None;
    }

    /// Starts from a lens an earlier session learned.
    pub fn set_prior(&mut self, tangent: Option<f64>) {
        self.prior = tangent.filter(|t| t.is_finite() && *t > 0.0);
    }

    /// The lens in force: the squared tangent of half the horizontal field of view, or `None`
    /// for a flat picture or one not learned yet.
    pub fn tangent(&self) -> Option<f64> {
        if self.decided() {
            self.chosen.map(tangent_of)
        } else {
            self.prior
        }
    }

    /// Whether the moves have said enough to choose (a lens or a flat picture).
    fn decided(&self) -> bool {
        self.moves >= MIN_MOVES && self.travel >= MIN_TRAVEL
    }

    /// Takes a move the player's turning made. Returns whether the lens in force changed.
    pub fn observe(&mut self, layout: &Layout, motion: &Motion) -> bool {
        let found = motion.cells.iter().flatten().count();
        if found < MIN_CELLS || motion.dx.hypot(motion.dy) < MIN_MOVE {
            return false;
        }
        let Some(flat) = layout.fit(motion, 0.0, OUTLIER) else {
            return false;
        };
        let before = self.tangent();
        // In thumbnail pixels, so the thresholds mean the same at every picture size.
        let unit = 1.0 / (layout.scale[0] * layout.scale[0]);
        self.flat = self.flat * FADE + flat.rest * unit;
        for (index, rest) in self.rest.iter_mut().enumerate() {
            // A candidate that cannot be fitted leaves what the flat picture does.
            let left = layout
                .fit(motion, candidate(index), OUTLIER)
                .map_or(flat.rest, |fit| fit.rest);
            *rest = *rest * FADE + left * unit;
        }
        self.moves = self.moves * FADE + 1.0;
        self.travel = self.travel * FADE + f64::from(motion.dx * motion.dx + motion.dy * motion.dy);
        if self.decided() {
            self.choose();
        }
        self.tangent() != before
    }

    fn choose(&mut self) {
        let Some((best, least)) = self
            .rest
            .iter()
            .copied()
            .enumerate()
            .min_by(|a, b| a.1.total_cmp(&b.1))
        else {
            return;
        };
        // Between the candidates: where a parabola through the best and its neighbours is least.
        let between = if best > 0 && best + 1 < CANDIDATES {
            let (left, right) = (self.rest[best - 1], self.rest[best + 1]);
            let curve = left - 2.0 * least + right;
            if curve > 0.0 {
                (0.5 * (left - right) / curve).clamp(-0.5, 0.5)
            } else {
                0.0
            }
        } else {
            0.0
        };
        let found = FIRST_DEGREES + STEP_DEGREES * (best as f64 + between);
        match self.chosen {
            None => {
                if least <= self.flat * TAKE {
                    self.chosen = Some(found);
                }
            }
            Some(current) => {
                if least > self.flat * DROP {
                    self.chosen = None;
                } else if (found - current).abs() >= CHANGE_DEGREES
                    && least <= self.left_at(current) * CHANGE
                {
                    self.chosen = Some(found);
                }
            }
        }
    }

    /// What a field of view of `degrees` leaves, between the candidates beside it.
    fn left_at(&self, degrees: f64) -> f64 {
        let at = ((degrees - FIRST_DEGREES) / STEP_DEGREES).clamp(0.0, (CANDIDATES - 1) as f64);
        let below = (at as usize).min(CANDIDATES - 2);
        let share = at - below as f64;
        self.rest[below] * (1.0 - share) + self.rest[below + 1] * share
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::frame_warp::motion::tests::world;
    use crate::frame_warp::motion::{CellShift, Thumbnail, estimate};

    const CONTENT: (u16, u16) = (2560, 1440);
    const THUMBNAIL: (usize, usize) = (256, 144);

    fn tangent(degrees: f64) -> f64 {
        let half = degrees.to_radians() * 0.5;
        half.tan() * half.tan()
    }

    /// The move a camera turning by `turn` thumbnail pixels at the centre makes through a lens
    /// of `degrees` (0: a flat picture), with a zoom, a roll and noise of `noise` pixels.
    pub(crate) fn turned(
        layout: &Layout,
        degrees: f64,
        turn: (f64, f64),
        zoom: f64,
        roll: f64,
        noise: f64,
        seed: u32,
    ) -> Motion {
        let lens = if degrees > 0.0 { tangent(degrees) } else { 0.0 };
        let mut state = seed.wrapping_mul(0x9e37_79b9) | 1;
        let mut random = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            f64::from(state) / f64::from(u32::MAX) - 0.5
        };
        let (a, b) = (turn.0 * layout.scale[0], turn.1 * layout.scale[1]);
        let cells: [Option<CellShift>; CELLS] = core::array::from_fn(|i| {
            let c = &layout.cells[i];
            let u = a * (1.0 + lens * c.xx) + b * lens * c.xy + zoom * c.x - roll * c.y;
            let v = a * lens * c.xy + b * (1.0 + lens * c.yy) + zoom * c.y + roll * c.x;
            Some(CellShift {
                dx: (u / layout.scale[0] + noise * 2.0 * random()) as f32,
                dy: (v / layout.scale[1] + noise * 2.0 * random()) as f32,
                correlation: 0.9,
                place: None,
            })
        });
        let median = |axis: usize| {
            let mut values: Vec<f32> = cells
                .iter()
                .flatten()
                .map(|c| if axis == 0 { c.dx } else { c.dy })
                .collect();
            values.sort_by(f32::total_cmp);
            values[values.len() / 2]
        };
        Motion {
            dx: median(0),
            dy: median(1),
            confidence: 0.9,
            cells,
        }
    }

    fn learn(degrees: f64, noise: f64, moves: usize) -> (Layout, Lens) {
        let layout = Layout::new(CONTENT, THUMBNAIL);
        let mut lens = Lens::default();
        for i in 0..moves {
            // Turns left and right, a little up and down, some while walking or rolling.
            let phase = i as f64 * 0.37;
            let turn = (4.0 * phase.sin() + 1.5, 1.2 * (phase * 0.6).cos());
            let zoom = if i % 3 == 0 { 3.0 } else { 0.0 };
            let roll = if i % 5 == 0 { -2.0 } else { 0.0 };
            lens.observe(
                &layout,
                &turned(&layout, degrees, turn, zoom, roll, noise, i as u32 + 1),
            );
        }
        (layout, lens)
    }

    fn degrees(lens: &Lens) -> Option<f64> {
        lens.tangent().map(|t| 2.0 * t.sqrt().atan().to_degrees())
    }

    #[test]
    fn a_perspective_picture_teaches_its_field_of_view() {
        for field in [50.0, 70.0, 90.0, 104.0] {
            let (_, lens) = learn(field, 0.15, 240);
            let found = degrees(&lens).unwrap_or_else(|| panic!("no lens for {field}"));
            assert!((found - field).abs() <= 4.0, "{found} for {field}");
        }
    }

    #[test]
    fn a_flat_picture_has_no_lens() {
        let (_, lens) = learn(0.0, 0.15, 240);
        assert_eq!(lens.tangent(), None);
        // And noise alone, at a narrow field of view's size, does not make one.
        let (_, lens) = learn(0.0, 0.4, 240);
        assert_eq!(lens.tangent(), None);
    }

    #[test]
    fn a_lens_waits_for_enough_turning_and_starts_from_the_prior() {
        let (layout, mut lens) = learn(90.0, 0.15, 12);
        assert_eq!(lens.tangent(), None);
        lens.set_prior(Some(tangent(80.0)));
        assert_eq!(degrees(&lens).map(f64::round), Some(80.0));
        // The moves then say otherwise.
        let mut changed = false;
        for i in 0..240 {
            let turn = (4.0 * (i as f64 * 0.37).sin() + 1.5, 1.0);
            changed |= lens.observe(&layout, &turned(&layout, 90.0, turn, 0.0, 0.0, 0.15, i + 7));
        }
        assert!(changed);
        let found = degrees(&lens).unwrap();
        assert!((found - 90.0).abs() <= 4.0, "{found}");
        // Cleared, the lens in force stays as the prior.
        lens.clear();
        assert!((degrees(&lens).unwrap() - found).abs() < 1e-9);
    }

    #[test]
    fn a_lens_follows_a_game_that_changes_its_field_of_view_and_goes_when_it_goes_flat() {
        let (layout, mut lens) = learn(90.0, 0.15, 240);
        for i in 0..900 {
            let turn = (4.0 * (i as f64 * 0.37).sin() + 1.5, 1.0);
            lens.observe(
                &layout,
                &turned(&layout, 60.0, turn, 0.0, 0.0, 0.15, i + 11),
            );
        }
        let found = degrees(&lens).unwrap();
        assert!((found - 60.0).abs() <= 4.0, "{found}");
        for i in 0..900 {
            let turn = (4.0 * (i as f64 * 0.37).sin() + 1.5, 1.0);
            lens.observe(&layout, &turned(&layout, 0.0, turn, 0.0, 0.0, 0.15, i + 13));
        }
        assert_eq!(lens.tangent(), None);
    }

    #[test]
    fn slow_or_sparse_moves_teach_nothing() {
        let layout = Layout::new(CONTENT, THUMBNAIL);
        let mut lens = Lens::default();
        for i in 0..400 {
            let mut slow = turned(&layout, 90.0, (0.4, 0.1), 0.0, 0.0, 0.1, i + 1);
            assert!(!lens.observe(&layout, &slow));
            // A fast move seen by four cells only.
            slow = turned(&layout, 90.0, (5.0, 0.0), 0.0, 0.0, 0.1, i + 1);
            for cell in slow.cells.iter_mut().skip(4) {
                *cell = None;
            }
            assert!(!lens.observe(&layout, &slow));
        }
        assert_eq!(lens.tangent(), None);
    }

    #[test]
    fn the_turn_is_the_centres_under_a_lens_and_the_median_without() {
        let layout = Layout::new(CONTENT, THUMBNAIL);
        let mut motion = turned(&layout, 90.0, (6.0, -2.0), 2.0, 1.0, 0.0, 1);
        // Most cells lie off centre, so the median is longer than the centre's move.
        assert!(motion.dx > 6.5, "{motion:?}");
        let flat = layout.resolve(&motion, None);
        assert_eq!((flat.dx, flat.dy), (motion.dx, motion.dy));
        let turn = layout.resolve(&motion, Some(tangent(90.0)));
        assert!(
            (turn.dx - 6.0).abs() < 0.01 && (turn.dy + 2.0).abs() < 0.01,
            "{turn:?}"
        );
        assert!(turn.confidence > 0.85, "{turn:?}");
        // A cell where something else moves is left out and costs confidence.
        motion.cells[2] = Some(CellShift {
            dx: -9.0,
            dy: 4.0,
            correlation: 0.9,
            place: None,
        });
        let turn = layout.resolve(&motion, Some(tangent(90.0)));
        assert!(
            (turn.dx - 6.0).abs() < 0.05 && (turn.dy + 2.0).abs() < 0.05,
            "{turn:?}"
        );
        assert!(turn.confidence < 0.85, "{turn:?}");
        // A still picture has no cells and stays as it was measured.
        let still = Motion {
            confidence: 0.8,
            ..Motion::default()
        };
        assert_eq!(layout.resolve(&still, Some(tangent(90.0))).confidence, 0.8);
    }

    /// The textured world on a cylinder around the camera, seen through a lens of `focal`
    /// thumbnail pixels by a camera pitched by `pitch` and then turned by `yaw` (radians), as a
    /// game's is.
    fn seen(focal: f64, yaw: f64, pitch: f64, at_us: u64) -> Thumbnail {
        let (width, height) = THUMBNAIL;
        let mut luma = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let px = x as f64 + 0.5 - width as f64 * 0.5;
                let py = y as f64 + 0.5 - height as f64 * 0.5;
                let (py, pz) = (
                    py * pitch.cos() + focal * pitch.sin(),
                    focal * pitch.cos() - py * pitch.sin(),
                );
                let (px, pz) = (
                    px * yaw.cos() + pz * yaw.sin(),
                    pz * yaw.cos() - px * yaw.sin(),
                );
                let (u, v) = (focal * px.atan2(pz), focal * py / px.hypot(pz));
                luma.push(world(u as f32 + 4000.0, v as f32 + 4000.0));
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
    fn pictures_drawn_through_a_lens_teach_it_and_give_the_centres_turn() {
        for field in [60.0f64, 90.0] {
            let layout = Layout::new(CONTENT, THUMBNAIL);
            let focal = THUMBNAIL.0 as f64 * 0.5 / (field.to_radians() * 0.5).tan();
            let mut lens = Lens::default();
            let (mut yaw, mut pitch) = (0.0f64, 0.0f64);
            let mut previous = seen(focal, yaw, pitch, 0);
            let mut worst = 0.0f64;
            for i in 1..=150u64 {
                // Turns of 1.5 to 5 thumbnail pixels a capture, left and right, with some pitch.
                let step = (3.2 + 1.8 * (i as f64 * 0.21).sin())
                    * if (i / 40) % 2 == 0 { 1.0 } else { -1.0 };
                let tilt = 0.6 * (i as f64 * 0.13).cos();
                yaw += step / focal;
                pitch += tilt / focal;
                let current = seen(focal, yaw, pitch, i);
                let motion = estimate(&previous, &current).expect("a move");
                lens.observe(&layout, &motion);
                if i > 100 {
                    // The content moves against the camera, by the turn at the picture's centre.
                    let turn = layout.resolve(&motion, lens.tangent());
                    worst = worst.max((f64::from(turn.dx) + step).abs());
                    assert!(turn.confidence >= 0.5, "{turn:?} at {i} for {field}");
                }
                previous = current;
            }
            let found = degrees(&lens).unwrap_or_else(|| panic!("no lens for {field}"));
            assert!((found - field).abs() <= 2.0, "{found} for {field}");
            assert!(worst < 0.35, "{worst} for {field}");
        }
    }

    /// Where [`with_hud`] draws: a framed minimap in the top right corner, a counter in the
    /// bottom right, a bar in the bottom left, and in the top left a stamp that changes with
    /// every capture.
    fn hud_at(x: usize, y: usize) -> Option<u8> {
        if (218..253).contains(&x) && (3..38).contains(&y) {
            Some(0)
        } else if (230..253).contains(&x) && (132..141).contains(&y) {
            Some(1)
        } else if (3..44).contains(&x) && (137..141).contains(&y) {
            Some(2)
        } else if x < 58 && (1..11).contains(&y) {
            Some(3)
        } else {
            None
        }
    }

    /// `t` under a HUD as a game draws one, the same in every capture but for its stamp.
    fn with_hud(mut t: Thumbnail, capture: u64) -> Thumbnail {
        for y in 0..t.height {
            for x in 0..t.width {
                let Some(part) = hud_at(x, y) else { continue };
                t.luma[y * t.width + x] = match part {
                    // A light frame around dark ground with blocks on it.
                    0 if !(220..251).contains(&x) || !(5..36).contains(&y) => 200,
                    0 if (x / 9 + y / 11) % 3 == 0 => 120,
                    0 => 40,
                    1 if (x / 3) % 2 == 0 => 220,
                    1 => 25,
                    2 if x < 32 => 90,
                    2 => 30,
                    _ => {
                        let bit = (x as u64 / 2).wrapping_mul(0x9e37_79b9_7f4a_7c15)
                            ^ (capture + y as u64 / 3 * 77).wrapping_mul(0xbf58_476d_1ce4_e5b9);
                        if (bit ^ (bit >> 31)) & 1 == 0 { 0 } else { 255 }
                    }
                };
            }
        }
        t
    }

    #[test]
    fn a_hud_in_the_corners_does_not_narrow_the_lens() {
        use crate::frame_warp::motion::{Still, estimate_around};
        // Left in the measurement, this HUD's corner cells move too little and a 90 degree lens
        // reads as 81. The host knows it from the GPU's mask, or finds what stands still.
        for known in [true, false] {
            let layout = Layout::new(CONTENT, THUMBNAIL);
            let focal = THUMBNAIL.0 as f64 * 0.5;
            let hud: Vec<bool> = (0..THUMBNAIL.0 * THUMBNAIL.1)
                .map(|i| hud_at(i % THUMBNAIL.0, i / THUMBNAIL.0).is_some_and(|part| part < 3))
                .collect();
            let mut still = Still::default();
            let mut lens = Lens::default();
            let (mut yaw, mut pitch) = (0.0f64, 0.0f64);
            let mut previous = with_hud(seen(focal, yaw, pitch, 0), 0);
            for i in 1..=if known { 150u64 } else { 300 } {
                let step = (3.2 + 1.8 * (i as f64 * 0.21).sin())
                    * if (i / 40) % 2 == 0 { 1.0 } else { -1.0 };
                yaw += step / focal;
                pitch += 0.6 * (i as f64 * 0.13).cos() / focal;
                let current = with_hud(seen(focal, yaw, pitch, i), i);
                let left_out = if known { Some(&hud[..]) } else { still.mask() };
                if let Some(motion) = estimate_around(&previous, &current, left_out) {
                    still.observe(&previous, &current, &motion);
                    lens.observe(&layout, &motion);
                }
                previous = current;
            }
            let found = degrees(&lens).unwrap_or_else(|| panic!("no lens, known {known}"));
            assert!((found - 90.0).abs() <= 2.5, "{found}, known {known}");
        }
    }

    #[test]
    fn a_move_that_is_no_turn_does_not_choose_a_lens() {
        let (layout, mut lens) = learn(90.0, 0.15, 240);
        let before = degrees(&lens).unwrap();
        // Cuts: every cell somewhere else, far from any lens's turn.
        for i in 0..40u32 {
            let mut cut = turned(&layout, 90.0, (4.0, 1.0), 0.0, 0.0, 0.15, i + 3);
            for (index, cell) in cut.cells.iter_mut().enumerate() {
                let cell = cell.as_mut().unwrap();
                let wild = ((index as u32 * 7 + i * 13) % 11) as f32 - 5.0;
                (cell.dx, cell.dy) = (cell.dx + 3.0 * wild, cell.dy - 2.0 * wild);
            }
            lens.observe(&layout, &cut);
            // And a turn between them, as play goes on.
            lens.observe(
                &layout,
                &turned(&layout, 90.0, (4.0, 1.0), 0.0, 0.0, 0.15, i + 90),
            );
        }
        let after = degrees(&lens).expect("still a lens");
        assert!((after - before).abs() <= 2.0, "{before} then {after}");
    }

    #[test]
    fn focal_lengths_and_tangents_convert() {
        let layout = Layout::new(CONTENT, THUMBNAIL);
        let focal = layout.focal(tangent(90.0));
        assert!((focal - 1280.0).abs() < 1e-6, "{focal}");
        assert!((layout.tangent(focal) - tangent(90.0)).abs() < 1e-9);
    }
}
