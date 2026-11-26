//! Frame Warp 2's fast field of view: the game's lens from the last few turning moves, decided
//! within [`MIN_MOVES`] to a dozen moves instead of the thirty-odd the faded sweep of
//! [`super::lens::Lens`] waits for.
//!
//! It keeps the last [`MAX_MOVES`] moves the player's turning made and fits their cells with the
//! same per-cell model [`super::lens`] uses (a camera turn, a zoom about the centre and a roll,
//! leaving out cells where something else moves, [`Layout::fit`]). Two passes find the field of
//! view whose fit leaves the least: a coarse sweep from 30 to 120 degrees in steps of
//! [`COARSE_STEP`] degrees, then [`REFINE_STEPS`] parabola steps (a Gauss-Newton step on a
//! one-dimensional remainder) through the best point and its neighbours, each half as wide as
//! the one before. The lens is taken when it leaves at most [`super::lens`]'s share of what a
//! flat picture leaves, and a flat picture is decided when the best lens leaves nearly as much as
//! flat does.

use std::collections::VecDeque;

use super::lens::{DROP, Layout, MIN_CELLS, MIN_MOVE, OUTLIER, TAKE, tangent_of};
use super::motion::Motion;

/// Fields of view swept: 30 to 120 degrees in steps of this.
pub const COARSE_STEP: f64 = 6.0;
const FIRST_DEGREES: f64 = 30.0;
const LAST_DEGREES: f64 = 120.0;
const COARSE: usize = 16;
/// Parabola steps after the sweep.
pub const REFINE_STEPS: usize = 3;
/// Moves before a decision.
pub const MIN_MOVES: usize = 8;
/// Moves kept and fitted.
pub const MAX_MOVES: usize = 16;
/// Once decided, the lens is fitted again every this many moves.
const REFIT_EVERY: usize = 4;

/// The fast lens (see the module documentation).
#[derive(Clone, Debug, Default)]
pub struct LensFast {
    moves: VecDeque<Motion>,
    /// Moves taken since the last fit.
    since_fit: usize,
    /// Moves taken in all.
    taken: usize,
    /// The decision: `Some(None)` is a flat picture.
    decided: Option<Option<f64>>,
}

impl LensFast {
    /// Forgets the moves and the decision (a new picture size).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Takes a move the player's turning made. Returns whether the decision changed (by more
    /// than a tenth of a degree).
    pub fn observe(&mut self, layout: &Layout, motion: &Motion) -> bool {
        let found = motion.cells.iter().flatten().count();
        if found < MIN_CELLS || motion.dx.hypot(motion.dy) < MIN_MOVE {
            return false;
        }
        if layout.fit(motion, 0.0, OUTLIER).is_none() {
            return false;
        }
        if self.moves.len() == MAX_MOVES {
            self.moves.pop_front();
        }
        self.moves.push_back(*motion);
        self.taken += 1;
        self.since_fit += 1;
        if self.moves.len() < MIN_MOVES || self.decided.is_some() && self.since_fit < REFIT_EVERY {
            return false;
        }
        self.since_fit = 0;
        let before = self.decided;
        if let Some(found) = self.solve(layout) {
            self.decided = Some(found);
        }
        match (before, self.decided) {
            (Some(Some(a)), Some(Some(b))) => (a - b).abs() > 0.1,
            (a, b) => a != b,
        }
    }

    /// The moves taken so far.
    pub fn moves(&self) -> usize {
        self.taken
    }

    /// Whether the moves decided (a lens or a flat picture).
    pub fn decided(&self) -> bool {
        self.decided.is_some()
    }

    /// The field of view found, in degrees; `None` for a flat picture or none decided yet.
    pub fn degrees(&self) -> Option<f64> {
        self.decided.flatten()
    }

    /// The lens found, as the squared tangent of half the horizontal field of view.
    pub fn tangent(&self) -> Option<f64> {
        self.degrees().map(tangent_of)
    }

    /// What the moves leave under a lens of `degrees` (0: flat), summed over the moves. A move
    /// the lens cannot fit leaves what a flat picture does.
    fn rest(&self, layout: &Layout, degrees: f64) -> f64 {
        let tangent = if degrees > 0.0 {
            tangent_of(degrees)
        } else {
            0.0
        };
        self.moves
            .iter()
            .map(|motion| {
                layout
                    .fit(motion, tangent, OUTLIER)
                    .or_else(|| layout.fit(motion, 0.0, OUTLIER))
                    .map_or(0.0, |fit| fit.rest)
            })
            .sum()
    }

    /// The decision the moves make, or `None` while they leave it open.
    fn solve(&self, layout: &Layout) -> Option<Option<f64>> {
        let flat = self.rest(layout, 0.0);
        let coarse: [f64; COARSE] =
            core::array::from_fn(|i| self.rest(layout, FIRST_DEGREES + COARSE_STEP * i as f64));
        let (best, least) = coarse
            .iter()
            .copied()
            .enumerate()
            .min_by(|a, b| a.1.total_cmp(&b.1))?;
        let mut at = FIRST_DEGREES + COARSE_STEP * best as f64;
        let mut around = (
            coarse.get(best.wrapping_sub(1)).copied(),
            least,
            coarse.get(best + 1).copied(),
        );
        let mut step = COARSE_STEP;
        for refine in 0..REFINE_STEPS {
            if refine > 0 {
                let side = |d: f64| {
                    let x = at + d;
                    (FIRST_DEGREES..=LAST_DEGREES)
                        .contains(&x)
                        .then(|| self.rest(layout, x))
                };
                around = (side(-step), self.rest(layout, at), side(step));
            }
            at = vertex(at, step, around).clamp(FIRST_DEGREES, LAST_DEGREES);
            step *= 0.5;
        }
        let left = self.rest(layout, at).min(least);
        if left <= flat * TAKE {
            Some(Some(at))
        } else if left > flat * DROP {
            Some(None)
        } else {
            None
        }
    }
}

/// Where a parabola through `(at - step, left)`, `(at, middle)` and `(at + step, right)` is
/// least, within half a step of `at`; `at` itself where a side is missing or the parabola opens
/// downward.
fn vertex(at: f64, step: f64, (left, middle, right): (Option<f64>, f64, Option<f64>)) -> f64 {
    match (left, right) {
        (Some(left), Some(right)) => {
            let curve = left - 2.0 * middle + right;
            if curve > 0.0 {
                at + step * (0.5 * (left - right) / curve).clamp(-0.5, 0.5)
            } else {
                at
            }
        }
        // At an end of the sweep: toward the lower side.
        (None, Some(right)) if right < middle => at + step * 0.5,
        (Some(left), None) if left < middle => at - step * 0.5,
        _ => at,
    }
}

#[cfg(test)]
mod tests;
