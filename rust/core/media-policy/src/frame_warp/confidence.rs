//! Frame Warp 2's camera confidence: how sure the host is, frame by frame, that the player's
//! camera and not a menu, a cutscene or a pointer moves the picture, which a version 2 client
//! fades the whole shift by (`camera_confidence` of 0x5549).
//!
//! Three signals drop it at once:
//!
//! - a scene cut: consecutive thumbnails whose luma histograms or coarse block pictures no
//!   longer match ([`scene_change`], [`Scene::cut`]), held for [`CUT_HOLD_US`];
//! - a pointer on the host's screen (a menu, a pause), for as long as it shows;
//! - a residual spike: [`SPIKES`] chunks in a row in which the model in force predicted a move
//!   the picture did not make ([`Confidence::on_check`]), held for [`SPIKE_HOLD_US`] or until
//!   the picture follows the input again.
//!
//! The state machine (design 04): a stream starts warm from the game's saved prior or cold,
//! locks once a model fits, decays while a signal holds, and unlocks after
//! [`UNLOCK_AFTER_US`] of it until [`CONFIRM`] chunks in a row confirm the model again. The host
//! sends the confidence as it stands; the client shapes the fall and the rise.

use rubylight_protocol::frame_warp::Mask;
use rubylight_protocol::frame_warp_v2::meta_flags;

use super::motion::Thumbnail;

/// How long a scene cut holds the confidence at 0.
pub const CUT_HOLD_US: u64 = 200_000;
/// How long a residual spike holds it at 0 at least.
pub const SPIKE_HOLD_US: u64 = 150_000;
/// Decaying this long is a menu or a cutscene that stays: the model counts as unlocked.
pub const UNLOCK_AFTER_US: u64 = 600_000;
/// Chunks in a row whose move the model missed that make a spike.
pub const SPIKES: u32 = 2;
/// Chunks in a row the model predicts well that confirm it after an unlock.
pub const CONFIRM: u32 = 2;
/// A chunk the model predicts less than this many thumbnail pixels of says nothing.
const CHECK_MIN: f64 = 2.0;
/// A chunk's explained share (against measured and predicted together) under which the model
/// missed it, and over which it confirms the model.
const MISSED: f64 = 0.0;
const CONFIRMED: f64 = 0.5;
/// Histogram bins over the luma codes.
const BINS: usize = 32;
/// Histograms that overlap less than this are another scene.
const HISTOGRAM_CUT: f32 = 0.5;
/// Block pictures that correlate less than this are another scene when the motion estimate
/// found no sure move between them either.
const CORRELATION_CUT: f32 = 0.25;
const MOTION_SURE: f32 = 0.3;
/// The coarse block picture: columns and rows.
const BLOCKS: (usize, usize) = (16, 9);
/// The HUD's steadiness moves this far toward each new mask's overlap with the last.
const HUD_RATE: f32 = 0.5;

/// How alike two consecutive thumbnails are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scene {
    /// The overlap of their luma histograms, 0 to 1.
    pub histogram: f32,
    /// The correlation of their coarse block pictures, -1 to 1.
    pub correlation: f32,
}

impl Scene {
    /// Whether the picture cut to another scene: the histograms no longer overlap, or the block
    /// pictures no longer correlate and the motion estimate found no sure move (`None`: none
    /// at all) that would explain it.
    pub fn cut(&self, motion_confidence: Option<f32>) -> bool {
        self.histogram < HISTOGRAM_CUT
            || self.correlation < CORRELATION_CUT
                && motion_confidence.is_none_or(|c| c < MOTION_SURE)
    }
}

/// How alike `prev` and `cur` are; `None` when their sizes differ.
pub fn scene_change(prev: &Thumbnail, cur: &Thumbnail) -> Option<Scene> {
    if (prev.width, prev.height) != (cur.width, cur.height)
        || prev.luma.len() != cur.luma.len()
        || cur.luma.is_empty()
    {
        return None;
    }
    let histogram = |t: &Thumbnail| {
        let mut bins = [0u32; BINS];
        for value in &t.luma {
            bins[usize::from(*value) * BINS / 256] += 1;
        }
        bins
    };
    let (a, b) = (histogram(prev), histogram(cur));
    let overlap: u32 = a.iter().zip(&b).map(|(x, y)| (*x).min(*y)).sum();
    let histogram = overlap as f32 / cur.luma.len() as f32;
    let (pa, pb) = (blocks(prev), blocks(cur));
    let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
    let (ma, mb) = (mean(&pa), mean(&pb));
    let (mut ab, mut aa, mut bb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in pa.iter().zip(&pb) {
        ab += (x - ma) * (y - mb);
        aa += (x - ma) * (x - ma);
        bb += (y - mb) * (y - mb);
    }
    // Two plain pictures (a black screen) match when their levels do.
    let correlation = if aa < 1e-3 || bb < 1e-3 {
        if aa < 1e-3 && bb < 1e-3 && (ma - mb).abs() < 8.0 {
            1.0
        } else {
            0.0
        }
    } else {
        ab / (aa * bb).sqrt()
    };
    Some(Scene {
        histogram,
        correlation,
    })
}

/// The mean luma of each block of the [`BLOCKS`] grid.
fn blocks(t: &Thumbnail) -> Vec<f32> {
    let (columns, rows) = BLOCKS;
    let mut sums = vec![0.0f32; columns * rows];
    let mut counts = vec![0u32; columns * rows];
    for y in 0..t.height {
        let row = (y * rows / t.height.max(1)).min(rows - 1);
        for x in 0..t.width {
            let column = (x * columns / t.width.max(1)).min(columns - 1);
            sums[row * columns + column] += f32::from(t.luma[y * t.width + x]);
            counts[row * columns + column] += 1;
        }
    }
    sums.iter()
        .zip(&counts)
        .map(|(sum, count)| sum / (*count).max(1) as f32)
        .collect()
}

/// The calibration and confidence states of design 04.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Off: no work.
    Disabled,
    /// Started from the game's saved prior, waiting for the first turn to confirm it.
    WarmPrior,
    /// Learning from nothing (no prior, or one this stream's turns disagreed with).
    ColdAcquiring,
    /// A model fits and nothing says otherwise.
    Locked,
    /// A model fits, but a cut, a pointer or a spike says the camera is not what moves now.
    ConfidenceDecaying,
    /// That lasted: a new turn has to confirm the model.
    Unlocked,
}

/// The confidence and its state machine (see the module documentation).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Confidence {
    state: State,
    cursor: bool,
    cut_until_us: Option<u64>,
    spike_until_us: Option<u64>,
    spikes: u32,
    confirms: u32,
    decaying_since_us: Option<u64>,
    value: f32,
}

impl Confidence {
    /// A stream that starts from a saved prior (`warm`) or from nothing.
    pub fn new(warm: bool) -> Self {
        Self {
            state: if warm {
                State::WarmPrior
            } else {
                State::ColdAcquiring
            },
            cursor: false,
            cut_until_us: None,
            spike_until_us: None,
            spikes: 0,
            confirms: 0,
            decaying_since_us: None,
            value: 0.0,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// The confidence, 0 to 1, as of the last [`Self::update`].
    pub fn value(&self) -> f32 {
        self.value
    }

    /// The host's pointer shows (`true`) or not.
    pub fn on_cursor(&mut self, visible: bool) {
        self.cursor = visible;
    }

    pub fn cursor(&self) -> bool {
        self.cursor
    }

    /// The thumbnail captured at `at_us` cut to another scene.
    pub fn on_cut(&mut self, at_us: u64) {
        self.cut_until_us = Some(at_us + CUT_HOLD_US);
    }

    /// A chunk's `measured` move against what the model in force `predicted` for its input
    /// (thumbnail pixels), at `at_us`.
    pub fn on_check(&mut self, measured: [f64; 2], predicted: [f64; 2], at_us: u64) {
        let size = predicted[0].hypot(predicted[1]);
        if size < CHECK_MIN {
            return;
        }
        let residual: f64 = (0..2).map(|a| (measured[a] - predicted[a]).powi(2)).sum();
        let total: f64 = (0..2)
            .map(|a| measured[a] * measured[a] + predicted[a] * predicted[a])
            .sum::<f64>()
            * 0.5;
        let share = 1.0 - residual / total.max(1e-9);
        if share < MISSED {
            self.confirms = 0;
            self.spikes += 1;
            if self.spikes >= SPIKES {
                self.spike_until_us = Some(at_us + SPIKE_HOLD_US);
            }
        } else {
            self.spikes = 0;
            if share >= CONFIRMED {
                self.confirms += 1;
                if self.confirms >= CONFIRM {
                    self.spike_until_us = None;
                }
            }
        }
    }

    /// Whether a signal holds the confidence down at `now_us`.
    fn gated(&self, now_us: u64) -> bool {
        self.cursor
            || self.cut_until_us.is_some_and(|until| now_us < until)
            || self.spike_until_us.is_some_and(|until| now_us < until)
    }

    /// Moves the state machine to `now_us`: whether a model is `locked`, whether the fit
    /// still runs from the saved prior (`warm`), and the best fit's explained share while none
    /// is locked. Returns the confidence.
    pub fn update(&mut self, locked: bool, warm: bool, fit: f64, now_us: u64) -> f32 {
        let gated = self.gated(now_us);
        self.state = match self.state {
            State::Disabled => State::Disabled,
            State::WarmPrior | State::ColdAcquiring if locked => State::Locked,
            State::WarmPrior if !warm => State::ColdAcquiring,
            state @ (State::WarmPrior | State::ColdAcquiring) => state,
            State::Locked | State::ConfidenceDecaying | State::Unlocked if !locked => {
                State::ColdAcquiring
            }
            State::Locked if gated => {
                self.decaying_since_us = Some(now_us);
                self.confirms = 0;
                State::ConfidenceDecaying
            }
            State::Locked => State::Locked,
            State::ConfidenceDecaying if !gated => State::Locked,
            State::ConfidenceDecaying
                if self
                    .decaying_since_us
                    .is_some_and(|since| now_us.saturating_sub(since) > UNLOCK_AFTER_US) =>
            {
                State::Unlocked
            }
            State::ConfidenceDecaying => State::ConfidenceDecaying,
            State::Unlocked if !gated && self.confirms >= CONFIRM => State::Locked,
            State::Unlocked => State::Unlocked,
        };
        self.value = match self.state {
            State::Locked => 1.0,
            State::WarmPrior | State::ColdAcquiring if !gated => fit.clamp(0.0, 1.0) as f32,
            _ => 0.0,
        };
        self.value
    }

    /// The meta flags the signals set at `now_us`.
    pub fn flags(&self, now_us: u64) -> u8 {
        let mut flags = 0;
        if self.cut_until_us.is_some_and(|until| now_us < until) {
            flags |= meta_flags::SCENE_CUT;
        }
        if self.cursor {
            flags |= meta_flags::CURSOR_VISIBLE_PAUSE;
        }
        flags
    }
}

/// How steady the HUD mask is: each mask the GPU's pass delivers compared with the one before
/// (the share of their cells that agree, of those either marks), smoothed. The GPU pass itself
/// is unchanged (no second pass scores stability on the GPU): the masks it already sends are the
/// temporal history.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HudStability {
    last: Option<Mask>,
    value: f32,
}

impl HudStability {
    /// A new mask, laid over the picture as the client gets it.
    pub fn observe(&mut self, mask: &Mask) {
        self.value = match &self.last {
            Some(last) if (last.width, last.height) == (mask.width, mask.height) => {
                let (both, either) = last.words.iter().zip(&mask.words).fold(
                    (0u32, 0u32),
                    |(both, either), (a, b)| {
                        (both + (a & b).count_ones(), either + (a | b).count_ones())
                    },
                );
                let agreement = if either == 0 {
                    1.0
                } else {
                    both as f32 / either as f32
                };
                self.value + (agreement - self.value) * HUD_RATE
            }
            // The first mask, or one of another size, says nothing yet.
            _ => 0.0,
        };
        self.last = Some(mask.clone());
    }

    /// 0 to 1; 0 before two masks of one size came.
    pub fn value(&self) -> f32 {
        self.value
    }

    /// Whether a mask has come.
    pub fn known(&self) -> bool {
        self.last.as_ref().is_some_and(|m| !m.is_clear())
    }
}

#[cfg(test)]
mod tests;
