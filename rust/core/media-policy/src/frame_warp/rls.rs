//! Frame Warp 2's fast fit: the game's gain and lag for one kind of input (mouse counts, seconds
//! of curved stick, radians of gyro), learned online by recursive least squares, so a version 2
//! client gets a locked model within a second of turning instead of the several seconds the
//! batch calibrator ([`super::calibrate`]) needs.
//!
//! The host's measured moves are summed into chunks of at least
//! [`super::calibrate::CHUNK_US`], as the calibrator sums them (a game that renders slower than
//! the capture then still lines up with its input). Each chunk updates, for every candidate lag
//! from 0 to [`super::calibrate::MAX_LAG_US`] in steps of [`LAG_STEP_US`] (and for the stick
//! every candidate curve), a scalar least-squares gain through the origin per axis:
//!
//! ```text
//! k     = P u / (lambda + P u²)
//! gain += k (m - gain u)
//! P     = (P - k u P) / lambda
//! ```
//!
//! with `u` the input that arrived in the chunk's span moved the lag earlier and `m` the
//! picture's measured move. The forgetting factor `lambda` adapts between [`LAMBDA_MIN`] and
//! [`LAMBDA_MAX`]: the more the fit in force misses a chunk, the faster it forgets, so a changed
//! sensitivity or a menu shows within a few chunks. The share of the motion each candidate
//! explains, over the same faded window, picks the lag (the shortest within a hair of the best,
//! as the calibrator picks it) and decides the lock: [`LOCK_FIT`] over [`COLD_CHUNKS`] chunks
//! from a cold start, or [`WARM_LOCK_FIT`] over [`WARM_CHUNKS`] when the game's saved prior
//! started the gains (small `P`) and the prior's lag still explains the motion. A prior that
//! does not ([`DISAGREE_FIT`]) is dropped and the fit runs on cold. [`UNLOCK_MISSES`] chunks in a
//! row that the fit in force misses unlock it at once (a menu opened).

use super::calibrate::MAX_LAG_US;

/// Lags tried: 0 to [`MAX_LAG_US`] in steps of this.
pub const LAG_STEP_US: u64 = 4_000;
const LAGS: usize = (MAX_LAG_US / LAG_STEP_US) as usize + 1;
/// The forgetting factor's range, per chunk: about 17 to 100 chunks of memory.
pub const LAMBDA_MIN: f64 = 0.94;
pub const LAMBDA_MAX: f64 = 0.99;
/// Explained share that locks a cold fit, a warm one at the prior's lag, and that unlocks.
pub const LOCK_FIT: f64 = 0.80;
pub const WARM_LOCK_FIT: f64 = 0.75;
pub const UNLOCK_FIT: f64 = 0.50;
/// Chunks with input (faded) a cold and a warm lock need.
pub const COLD_CHUNKS: f64 = 8.0;
pub const WARM_CHUNKS: f64 = 3.0;
/// A prior whose lag explains less than this once [`DISAGREE_CHUNKS`] chunks came in is wrong
/// for the game as it runs now (a patch, a changed sensitivity).
pub const DISAGREE_FIT: f64 = 0.5;
const DISAGREE_CHUNKS: f64 = 6.0;
/// The shortest lag within this share of the best fit wins.
const LAG_TOLERANCE: f64 = 0.995;
/// A warm fit keeps the prior's lag and curve while they explain this share of the best.
const PRIOR_TOLERANCE: f64 = 0.97;
/// Measured motion (thumbnail pixels, faded) a lock needs.
const MIN_MOTION: f64 = 8.0;
/// `P` of a cold and a warm start, times [`Kind::unit`]: a cold start's first chunk sets the
/// gain, a warm start's moves it by a few percent.
const P_COLD: f64 = 1e4;
const P_WARM: f64 = 0.02;
/// Thumbnail pixels of a chunk's move under which a miss says nothing about the fit in force.
const NOISE: f64 = 0.5;
/// A chunk whose measured or predicted move (thumbnail pixels, larger axis) is under this says
/// nothing of whether the fit in force follows the picture.
const CHECK: f64 = 2.0;
/// Surprise from which a chunk counts as missed.
const MISS: f64 = 0.5;
/// Chunks in a row the fit in force missed that unlock it (a menu, a cut scene), long before its
/// faded share falls; and a fit locks only after a chunk it did not miss.
const UNLOCK_MISSES: u32 = 5;
/// A locked fit keeps its lag and curve unless another explains this much more of the motion,
/// or this share of what it leaves unexplained when that is less (the calibrator's margins):
/// on smooth input many lags fit almost alike, and a lag that wandered would jolt every tag.
const REFIT_MARGIN: f64 = 0.03;
const REFIT_SHARE: f64 = 0.25;

/// Stick curves tried: the calibrator's dead zones and exponents.
pub const STICK_CURVES: [(f32, f32); 15] = [
    (0.05, 1.0),
    (0.05, 1.5),
    (0.05, 2.0),
    (0.10, 1.0),
    (0.10, 1.5),
    (0.10, 2.0),
    (0.15, 1.0),
    (0.15, 1.5),
    (0.15, 2.0),
    (0.20, 1.0),
    (0.20, 1.5),
    (0.20, 2.0),
    (0.25, 1.0),
    (0.25, 1.5),
    (0.25, 2.0),
];
/// The one curve of an input without one.
const NO_CURVE: [(f32, f32); 1] = [(0.0, 1.0)];

/// The kinds of input a fit learns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Relative mouse counts.
    Mouse,
    /// Seconds of full right-stick deflection after the curve.
    Stick,
    /// Radians of gyro turn.
    Gyro,
}

impl Kind {
    /// A chunk's typical squared input, which scales `P`: 100 counts, 10 ms of full stick,
    /// 0.1 radians.
    fn unit(self) -> f64 {
        match self {
            Kind::Mouse => 1e4,
            Kind::Stick => 1e-4,
            Kind::Gyro => 1e-2,
        }
    }

    /// Input (faded, both axes) a lock needs: far less than the calibrator's 300 counts or
    /// 0.4 s of stick.
    fn min_input(self) -> f64 {
        match self {
            Kind::Mouse => 80.0,
            Kind::Stick => 0.12,
            Kind::Gyro => 0.06,
        }
    }

    fn curves(self) -> &'static [(f32, f32)] {
        match self {
            Kind::Stick => &STICK_CURVES,
            Kind::Mouse | Kind::Gyro => &NO_CURVE,
        }
    }
}

/// One axis of one candidate.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Axis {
    gain: f64,
    p: f64,
    /// Faded sums of input², input × motion, motion² and |input|, and of chunks with input.
    uu: f64,
    um: f64,
    mm: f64,
    input: f64,
    chunks: f64,
}

impl Axis {
    fn new(gain: f64, p: f64) -> Self {
        Self {
            gain,
            p,
            uu: 0.0,
            um: 0.0,
            mm: 0.0,
            input: 0.0,
            chunks: 0.0,
        }
    }

    fn update(&mut self, u: f64, m: f64, lambda: f64, p_cap: f64) {
        let k = self.p * u / (lambda + self.p * u * u);
        self.gain += k * (m - self.gain * u);
        self.p = ((self.p - k * u * self.p) / lambda).min(p_cap);
        self.uu = lambda * self.uu + u * u;
        self.um = lambda * self.um + u * m;
        self.mm = lambda * self.mm + m * m;
        self.input = lambda * self.input + u.abs();
        self.chunks = lambda * self.chunks + f64::from(u8::from(u != 0.0));
    }

    /// What `gain` leaves of this axis's motion.
    fn residual(&self, gain: f64) -> f64 {
        (self.mm - 2.0 * gain * self.um + gain * gain * self.uu).max(0.0)
    }
}

/// A candidate's place: its curve and lag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Place {
    curve: usize,
    lag: usize,
}

/// A fit of one kind ([`Rls::fit`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RlsFit {
    /// Thumbnail pixels per unit of input, x and y.
    pub gain: [f64; 2],
    pub lag_us: u64,
    /// Share of the motion explained, 0 to 1.
    pub fit: f64,
    pub deadzone: f32,
    pub exponent: f32,
}

/// The online fit of one kind of input (see the module documentation).
#[derive(Clone, Debug)]
pub struct Rls {
    kind: Kind,
    /// Per curve, per lag, x and y.
    cells: Vec<[[Axis; 2]; LAGS]>,
    lambda: f64,
    /// Faded |motion| of the chunks this kind was fed.
    motion: f64,
    best: Option<(Place, f64, [f64; 2])>,
    /// The saved prior's place, while the fit runs warm from it.
    prior: Option<Place>,
    locked: bool,
    /// The lock came from the prior, confirmed.
    warm_locked: bool,
    /// Chunks in a row the fit in force missed ([`UNLOCK_MISSES`]).
    misses: u32,
}

impl Rls {
    /// A cold fit of `kind`.
    pub fn new(kind: Kind) -> Self {
        let cold = Axis::new(0.0, P_COLD / kind.unit());
        Self {
            kind,
            cells: vec![[[cold; 2]; LAGS]; kind.curves().len()],
            lambda: LAMBDA_MAX,
            motion: 0.0,
            best: None,
            prior: None,
            locked: false,
            warm_locked: false,
            misses: 0,
        }
    }

    /// Starts every candidate from a saved prior: `gain` (thumbnail pixels per unit) with a small
    /// `P`, and the prior's lag and curve preferred while they explain the motion.
    pub fn warm(kind: Kind, gain: [f64; 2], lag_us: u64, curve: Option<(f32, f32)>) -> Self {
        let mut out = Self::new(kind);
        if !gain.iter().all(|g| g.is_finite()) {
            return out;
        }
        let p = P_WARM / kind.unit();
        for lags in &mut out.cells {
            for axes in lags.iter_mut() {
                for (axis, value) in axes.iter_mut().enumerate() {
                    *value = Axis::new(gain[axis], p);
                }
            }
        }
        let curve = curve
            .and_then(|c| kind.curves().iter().position(|k| *k == c))
            .unwrap_or_else(|| {
                kind.curves()
                    .iter()
                    .position(|k| *k == (0.15, 1.0))
                    .unwrap_or(0)
            });
        let lag = (lag_us.min(MAX_LAG_US) + LAG_STEP_US / 2) / LAG_STEP_US;
        out.prior = Some(Place {
            curve,
            lag: lag as usize,
        });
        out
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// The lag of candidate `index`.
    fn lag_us(index: usize) -> u64 {
        index as u64 * LAG_STEP_US
    }

    /// Takes a chunk's measured move `m` (thumbnail pixels). `input(curve, lag_us)` is the
    /// input of the chunk's span moved `lag_us` earlier under curve `curve` ([`STICK_CURVES`]
    /// for the stick, `(0, 1)` otherwise), or `None` where it may not count (another kind of
    /// input was turning the camera then).
    pub fn update(&mut self, m: [f64; 2], input: impl Fn((f32, f32), u64) -> Option<[f64; 2]>) {
        let (lambda, hit) = self.check(m, &input);
        self.lambda = lambda;
        let p_cap = P_COLD / self.kind.unit();
        let mut fed = false;
        for (curve, lags) in self.kind.curves().iter().zip(self.cells.iter_mut()) {
            for (lag, axes) in lags.iter_mut().enumerate() {
                let Some(u) = input(*curve, Self::lag_us(lag)) else {
                    continue;
                };
                fed = true;
                for axis in 0..2 {
                    axes[axis].update(u[axis], m[axis], lambda, p_cap);
                }
            }
        }
        if fed {
            match hit {
                Some(true) => self.misses = 0,
                Some(false) => self.misses = self.misses.saturating_add(1),
                None => {}
            }
            self.motion = lambda * self.motion + m[0].abs() + m[1].abs();
            self.select();
            self.decide();
        }
    }

    /// How the fit in force did on a chunk: the forgetting factor (the more it misses, the
    /// smaller) and whether it followed the picture (`None` for a chunk too small to say).
    fn check(
        &self,
        m: [f64; 2],
        input: &impl Fn((f32, f32), u64) -> Option<[f64; 2]>,
    ) -> (f64, Option<bool>) {
        let Some((place, _, gain)) = self.best else {
            return (LAMBDA_MAX, None);
        };
        let Some(u) = input(self.kind.curves()[place.curve], Self::lag_us(place.lag)) else {
            return (self.lambda, None);
        };
        let predicted = [gain[0] * u[0], gain[1] * u[1]];
        let miss: f64 = (0..2).map(|a| (m[a] - predicted[a]).powi(2)).sum();
        let size: f64 = (0..2)
            .map(|a| m[a] * m[a] + predicted[a] * predicted[a])
            .sum();
        let surprise = (miss / (size + NOISE * NOISE)).clamp(0.0, 1.0);
        let large = (0..2).any(|a| m[a].abs().max(predicted[a].abs()) >= CHECK);
        (
            LAMBDA_MAX - (LAMBDA_MAX - LAMBDA_MIN) * surprise,
            large.then_some(surprise < MISS),
        )
    }

    /// A candidate's explained share and gains. An axis with too little input of its own (a
    /// quarter of what a lock needs, or of the other axis's) takes the other's gain, as the
    /// calibrator does.
    fn share(&self, place: Place) -> Option<(f64, [f64; 2])> {
        let axes = &self.cells[place.curve][place.lag];
        let min = self.kind.min_input() * 0.25;
        let enough = |a: usize| {
            axes[a].input >= min && axes[a].input * 4.0 >= axes[1 - a].input && axes[a].uu > 0.0
        };
        let gain = match (enough(0), enough(1)) {
            (true, true) => [axes[0].gain, axes[1].gain],
            (true, false) => [axes[0].gain; 2],
            (false, true) => [axes[1].gain; 2],
            (false, false) => return None,
        };
        let total = axes[0].mm + axes[1].mm;
        if total <= 1e-9 || !gain.iter().all(|g| g.is_finite()) {
            return None;
        }
        let residual = axes[0].residual(gain[0]) + axes[1].residual(gain[1]);
        Some(((1.0 - residual / total).clamp(0.0, 1.0), gain))
    }

    /// The best candidate: the shortest lag within [`LAG_TOLERANCE`] of the best fit, in the
    /// best fit's curve; a warm fit's prior place while it comes within [`PRIOR_TOLERANCE`].
    fn select(&mut self) {
        let mut best: Option<(Place, f64)> = None;
        for curve in 0..self.cells.len() {
            for lag in 0..LAGS {
                let place = Place { curve, lag };
                if let Some((share, _)) = self.share(place)
                    && best.is_none_or(|(_, b)| share > b)
                {
                    best = Some((place, share));
                }
            }
        }
        let Some((top, top_share)) = best else {
            self.best = None;
            return;
        };
        if self.locked
            && let Some((current, ..)) = self.best
            && let Some((share, gain)) = self.share(current)
            && share + REFIT_MARGIN.min(REFIT_SHARE * (1.0 - share)) >= top_share
        {
            self.best = Some((current, share, gain));
            return;
        }
        if let Some(prior) = self.prior
            && let Some((share, gain)) = self.share(prior)
            && share >= top_share * PRIOR_TOLERANCE
        {
            self.best = Some((prior, share, gain));
            return;
        }
        let chosen = (0..=top.lag)
            .map(|lag| Place {
                curve: top.curve,
                lag,
            })
            .find_map(|place| {
                self.share(place)
                    .filter(|(share, _)| *share >= top_share * LAG_TOLERANCE)
                    .map(|(share, gain)| (place, share, gain))
            });
        self.best = chosen.or_else(|| self.share(top).map(|(share, gain)| (top, share, gain)));
    }

    /// Locks, unlocks, or drops a prior that disagrees.
    fn decide(&mut self) {
        let Some((place, share, _)) = self.best else {
            return;
        };
        let axes = &self.cells[place.curve][place.lag];
        let input = axes[0].input + axes[1].input;
        let chunks = axes[0].chunks.max(axes[1].chunks);
        let evidence = input >= self.kind.min_input() && self.motion >= MIN_MOTION;
        let at_prior = self.prior == Some(place);
        let (needed, lock_fit) = if at_prior {
            (WARM_CHUNKS, WARM_LOCK_FIT)
        } else {
            (COLD_CHUNKS, LOCK_FIT)
        };
        if !self.locked && evidence && chunks >= needed && share >= lock_fit && self.misses == 0 {
            self.locked = true;
            self.warm_locked = at_prior;
        } else if self.locked && (evidence && share < UNLOCK_FIT || self.misses >= UNLOCK_MISSES) {
            self.locked = false;
            self.warm_locked = false;
        }
        // A prior whose lag explains too little after enough chunks goes: the fit runs on cold.
        if !self.locked
            && let Some(prior) = self.prior
            && evidence
            && chunks >= DISAGREE_CHUNKS
            && self.share(prior).is_none_or(|(s, _)| s < DISAGREE_FIT)
        {
            self.prior = None;
            let p = P_COLD / self.kind.unit();
            for lags in &mut self.cells {
                for axes in lags.iter_mut() {
                    for axis in axes.iter_mut() {
                        axis.p = p;
                    }
                }
            }
        }
    }

    /// The best candidate's fit, once there is one.
    pub fn fit(&self) -> Option<RlsFit> {
        let (place, share, gain) = self.best?;
        let (deadzone, exponent) = self.kind.curves()[place.curve];
        Some(RlsFit {
            gain,
            lag_us: Self::lag_us(place.lag),
            fit: share,
            deadzone,
            exponent,
        })
    }

    /// The fit, while it is locked.
    pub fn locked_fit(&self) -> Option<RlsFit> {
        self.fit().filter(|_| self.locked)
    }

    pub fn locked(&self) -> bool {
        self.locked
    }

    /// Whether the lock came from the saved prior, confirmed by this stream's turns.
    pub fn warm_locked(&self) -> bool {
        self.locked && self.warm_locked
    }

    /// Whether the fit still runs from a saved prior.
    pub fn is_warm(&self) -> bool {
        self.prior.is_some()
    }

    /// The forgetting factor in force.
    pub fn lambda(&self) -> f64 {
        self.lambda
    }
}

#[cfg(test)]
pub(crate) mod tests;
