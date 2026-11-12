//! Learning the running game's camera response: how far the picture moves per mouse count and
//! per second of stick, and how long after the input it moves (the game's lag).
//!
//! Each pair of analysed captures gives a measured content shift over a time span. Consecutive
//! pairs are summed into chunks of at least [`CHUNK_US`], so a game that renders slower than
//! the capture (many still pairs, then a jump) still lines up with its input. For every
//! candidate lag `L`, each chunk's input is what arrived in its span moved `L` earlier, and a
//! least-squares gain through the origin fits shift to input on each axis. The lag whose fit
//! explains the most of the measured motion wins, preferring the shortest lag that comes
//! within [`LAG_TOLERANCE`] of the best: a lag too long would shift too far, a lag too short
//! only falls short.
//!
//! A fit locks once it explains [`LOCK_FIT`] of the motion over enough input, and unlocks when
//! recent input with the locked model explains less than [`UNLOCK_FIT`] (a menu, a cutscene,
//! the mouse moving a cursor): a shift that matches nothing on screen is worse than none.

use std::collections::VecDeque;

use rubylight_protocol::frame_warp::{Model, STICK_EXPONENTS};

use super::history::InputHistory;
use super::motion::Motion;

/// Samples kept and fitted over.
pub const WINDOW_US: u64 = 4_000_000;
/// The recent span an unlock decision looks at.
pub const RECENT_US: u64 = 1_200_000;
/// Shortest span a chunk of samples covers.
pub const CHUNK_US: u64 = 30_000;
/// Lags tried: 0 to 200 ms.
pub const MAX_LAG_US: u64 = 200_000;
const LAG_STEP_US: u64 = 2_000;
/// The shortest lag within this fraction of the best fit wins.
const LAG_TOLERANCE: f64 = 0.995;
/// Fit (explained share of the motion) that locks, and that recent input must keep.
pub const LOCK_FIT: f64 = 0.80;
pub const UNLOCK_FIT: f64 = 0.50;
/// Input a fit needs: mouse counts, seconds of full deflection, and thumbnail pixels moved.
const MIN_MOUSE_COUNTS: f64 = 300.0;
const MIN_STICK_SECONDS: f64 = 0.4;
const MIN_MOTION: f64 = 30.0;
/// Samples whose estimate is less sure than this are left out.
const MIN_CONFIDENCE: f32 = 0.35;
/// How often the fit runs.
const EVERY_US: u64 = 200_000;
/// A locked model is sent again when a gain moves by this fraction or the lag by this much.
const GAIN_CHANGE: f32 = 0.04;
const LAG_CHANGE_US: u32 = 3_000;
/// Largest shift, as a fraction of the picture's width and height.
pub const MAX_OFFSET: f32 = 0.15;
/// Stick dead zones tried.
const DEADZONES: [f32; 5] = [0.05, 0.10, 0.15, 0.20, 0.25];
/// The lag before anything is fitted.
pub const DEFAULT_LAG_US: u64 = 40_000;

/// One measured shift between two analysed captures.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub t0_us: u64,
    pub t1_us: u64,
    pub motion: Motion,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Chunk {
    t0_us: u64,
    t1_us: u64,
    dx: f64,
    dy: f64,
}

/// A fit of one input kind.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    /// Thumbnail pixels per mouse count, or per second of full deflection.
    pub gain: [f64; 2],
    pub lag_us: u64,
    /// Share of the motion explained, 0 to 1.
    pub fit: f64,
    pub deadzone: f32,
    pub exponent: f32,
}

/// What a previous session learned for this game, to start from.
#[derive(Clone, Copy, Debug, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Prior {
    /// Picture pixels per count at the picture size below, if learned.
    pub mouse_gain: Option<[f32; 2]>,
    pub mouse_lag_us: Option<u64>,
    pub stick_gain: Option<[f32; 2]>,
    pub stick_lag_us: Option<u64>,
    pub stick_deadzone: Option<f32>,
    pub stick_exponent: Option<f32>,
    /// The picture size the gains are in.
    pub picture_width: u16,
    pub picture_height: u16,
}

#[derive(Debug)]
pub struct Calibrator {
    samples: VecDeque<Sample>,
    /// Picture pixels per thumbnail pixel, x and y.
    scale: [f64; 2],
    picture: (u16, u16),
    mouse: Option<Fit>,
    stick: Option<Fit>,
    mouse_locked: bool,
    stick_locked: bool,
    last_fit_us: Option<u64>,
    generation: u16,
    sent: Option<Model>,
    prior: Option<Prior>,
}

impl Calibrator {
    /// A calibrator for a stream of `picture` pixels whose thumbnails are `thumbnail` pixels.
    pub fn new(picture: (u16, u16), thumbnail: (usize, usize)) -> Self {
        let mut out = Self {
            samples: VecDeque::new(),
            scale: [1.0; 2],
            picture,
            mouse: None,
            stick: None,
            mouse_locked: false,
            stick_locked: false,
            last_fit_us: None,
            generation: 0,
            sent: None,
            prior: None,
        };
        out.set_sizes(picture, thumbnail);
        out
    }

    /// A new stream or thumbnail size; what was learned carries over in picture pixels.
    pub fn set_sizes(&mut self, picture: (u16, u16), thumbnail: (usize, usize)) {
        let old_scale = self.scale;
        self.picture = (picture.0.max(1), picture.1.max(1));
        self.scale = [
            f64::from(self.picture.0) / thumbnail.0.max(1) as f64,
            f64::from(self.picture.1) / thumbnail.1.max(1) as f64,
        ];
        if old_scale != self.scale {
            // Samples are in the old thumbnail's pixels.
            self.samples.clear();
            for fit in [&mut self.mouse, &mut self.stick].into_iter().flatten() {
                for (axis, gain) in fit.gain.iter_mut().enumerate() {
                    *gain *= old_scale[axis] / self.scale[axis];
                }
            }
        }
    }

    /// Forgets the measured moves (their input history is gone); what was learned stays.
    pub fn clear_samples(&mut self) {
        self.samples.clear();
    }

    /// Starts from what an earlier session learned: the lags apply at once and the gains lock
    /// as soon as recent input confirms them.
    pub fn set_prior(&mut self, prior: Prior) {
        let scale = |gain: [f32; 2]| {
            let ratio = [
                f64::from(self.picture.0) / f64::from(prior.picture_width.max(1)),
                f64::from(self.picture.1) / f64::from(prior.picture_height.max(1)),
            ];
            [
                f64::from(gain[0]) * ratio[0] / self.scale[0],
                f64::from(gain[1]) * ratio[1] / self.scale[1],
            ]
        };
        if let (Some(gain), Some(lag_us)) = (prior.mouse_gain, prior.mouse_lag_us) {
            self.mouse = Some(Fit {
                gain: scale(gain),
                lag_us: lag_us.min(MAX_LAG_US),
                fit: 0.0,
                deadzone: 0.0,
                exponent: 1.0,
            });
        }
        if let (Some(gain), Some(lag_us)) = (prior.stick_gain, prior.stick_lag_us) {
            self.stick = Some(Fit {
                gain: scale(gain),
                lag_us: lag_us.min(MAX_LAG_US),
                fit: 0.0,
                deadzone: prior.stick_deadzone.unwrap_or(0.15).clamp(0.0, 0.5),
                exponent: prior
                    .stick_exponent
                    .filter(|e| STICK_EXPONENTS.contains(e))
                    .unwrap_or(1.0),
            });
        }
        self.prior = Some(prior);
    }

    /// What to remember for this game: the locked fits, in picture pixels.
    pub fn prior(&self) -> Option<Prior> {
        let mouse = self.mouse.filter(|_| self.mouse_locked);
        let stick = self.stick.filter(|_| self.stick_locked);
        if mouse.is_none() && stick.is_none() {
            return self.prior;
        }
        let picture = |gain: [f64; 2]| {
            [
                (gain[0] * self.scale[0]) as f32,
                (gain[1] * self.scale[1]) as f32,
            ]
        };
        let earlier = self.prior.unwrap_or_default();
        Some(Prior {
            mouse_gain: mouse.map(|f| picture(f.gain)).or(earlier.mouse_gain),
            mouse_lag_us: mouse.map(|f| f.lag_us).or(earlier.mouse_lag_us),
            stick_gain: stick.map(|f| picture(f.gain)).or(earlier.stick_gain),
            stick_lag_us: stick.map(|f| f.lag_us).or(earlier.stick_lag_us),
            stick_deadzone: stick.map(|f| f.deadzone).or(earlier.stick_deadzone),
            stick_exponent: stick.map(|f| f.exponent).or(earlier.stick_exponent),
            picture_width: self.picture.0,
            picture_height: self.picture.1,
        })
    }

    /// The lag to tag mouse input with: the fitted one, else the default.
    pub fn mouse_lag_us(&self) -> u64 {
        self.mouse.map_or(DEFAULT_LAG_US, |f| f.lag_us)
    }

    pub fn stick_lag_us(&self) -> u64 {
        self.stick.map_or(self.mouse_lag_us(), |f| f.lag_us)
    }

    pub fn locked(&self) -> bool {
        self.mouse_locked || self.stick_locked
    }

    pub fn add(&mut self, sample: Sample) {
        if sample.t1_us <= sample.t0_us {
            return;
        }
        self.samples.push_back(sample);
        while self
            .samples
            .front()
            .is_some_and(|s| sample.t1_us.saturating_sub(s.t1_us) > WINDOW_US)
        {
            self.samples.pop_front();
        }
    }

    /// Fits again if it is time, and returns the model to send when it changed enough to.
    pub fn update(&mut self, history: &InputHistory, now_us: u64) -> Option<Model> {
        if self
            .last_fit_us
            .is_some_and(|at| now_us.saturating_sub(at) < EVERY_US)
        {
            return None;
        }
        self.last_fit_us = Some(now_us);
        let all = self.chunks(None);
        let recent = self.chunks(Some(now_us.saturating_sub(RECENT_US)));
        self.update_mouse(history, &all, &recent);
        self.update_stick(history, &all, &recent);
        self.take_model()
    }

    fn update_mouse(&mut self, history: &InputHistory, all: &[Chunk], recent: &[Chunk]) {
        let input = |c: &Chunk, lag: u64| {
            if history.stick_active(c.t0_us.saturating_sub(lag), c.t1_us.saturating_sub(lag)) {
                return None;
            }
            let (x, y) =
                history.mouse_between(c.t0_us.saturating_sub(lag), c.t1_us.saturating_sub(lag));
            Some([x as f64, y as f64])
        };
        let fresh = best_fit(all, &input, MIN_MOUSE_COUNTS);
        // Recent input under the model in force (or the fresh fit) decides an unlock.
        let current = if self.mouse_locked {
            self.mouse
        } else {
            fresh.or(self.mouse)
        };
        let recent_fit =
            current.and_then(|f| score(recent, &input, f.lag_us, Some(f.gain), MIN_MOUSE_COUNTS));
        if let Some(fit) = fresh {
            if fit.fit >= LOCK_FIT {
                self.mouse = Some(fit);
                self.mouse_locked = true;
            } else if !self.mouse_locked {
                self.mouse = Some(fit);
            }
        }
        // A prior locks on recent confirmation alone.
        if !self.mouse_locked
            && self.prior.is_some()
            && let Some(recent) = recent_fit.filter(|r| r.fit >= LOCK_FIT)
        {
            self.mouse_locked = true;
            if let Some(fit) = self.mouse.as_mut() {
                fit.fit = recent.fit;
            }
        }
        if self.mouse_locked && recent_fit.is_some_and(|r| r.fit < UNLOCK_FIT) {
            self.mouse_locked = false;
        }
    }

    fn update_stick(&mut self, history: &InputHistory, all: &[Chunk], recent: &[Chunk]) {
        let integral = |deadzone: f32, exponent: f32| {
            move |c: &Chunk, lag: u64| {
                let (t0, t1) = (c.t0_us.saturating_sub(lag), c.t1_us.saturating_sub(lag));
                let (mx, my) = history.mouse_between(t0, t1);
                if mx != 0 || my != 0 {
                    return None;
                }
                Some(history.stick_integral(t0, t1, deadzone, exponent))
            }
        };
        // The lag with a middling curve, then the curve at that lag.
        let mut fresh = best_fit(all, &integral(0.15, 1.0), MIN_STICK_SECONDS).map(|mut f| {
            f.deadzone = 0.15;
            f.exponent = 1.0;
            f
        });
        if let Some(found) = fresh {
            for deadzone in DEADZONES {
                for exponent in [1.0, 1.5, 2.0] {
                    if let Some(mut fit) = score(
                        all,
                        &integral(deadzone, exponent),
                        found.lag_us,
                        None,
                        MIN_STICK_SECONDS,
                    ) && fit.fit > fresh.map_or(0.0, |f| f.fit)
                    {
                        fit.deadzone = deadzone;
                        fit.exponent = exponent;
                        fresh = Some(fit);
                    }
                }
            }
            // The lag again, with the curve found.
            if let Some(curve) = fresh
                && let Some(mut refit) = best_fit(
                    all,
                    &integral(curve.deadzone, curve.exponent),
                    MIN_STICK_SECONDS,
                )
            {
                refit.deadzone = curve.deadzone;
                refit.exponent = curve.exponent;
                fresh = Some(refit);
            }
        }
        let current = if self.stick_locked {
            self.stick
        } else {
            fresh.or(self.stick)
        };
        let recent_fit = current.and_then(|f| {
            score(
                recent,
                &integral(f.deadzone, f.exponent),
                f.lag_us,
                Some(f.gain),
                MIN_STICK_SECONDS,
            )
        });
        if let Some(fit) = fresh {
            if fit.fit >= LOCK_FIT {
                self.stick = Some(fit);
                self.stick_locked = true;
            } else if !self.stick_locked {
                self.stick = Some(fit);
            }
        }
        if !self.stick_locked
            && self.prior.is_some()
            && let Some(recent) = recent_fit.filter(|r| r.fit >= LOCK_FIT)
        {
            self.stick_locked = true;
            if let Some(fit) = self.stick.as_mut() {
                fit.fit = recent.fit;
            }
        }
        if self.stick_locked && recent_fit.is_some_and(|r| r.fit < UNLOCK_FIT) {
            self.stick_locked = false;
        }
    }

    /// Confident samples since `since`, summed into chunks of at least [`CHUNK_US`]. A gap or
    /// an unsure sample ends a chunk.
    fn chunks(&self, since: Option<u64>) -> Vec<Chunk> {
        let mut out = Vec::new();
        let mut open: Option<Chunk> = None;
        for s in &self.samples {
            if since.is_some_and(|t| s.t0_us < t) {
                continue;
            }
            let usable = s.motion.confidence >= MIN_CONFIDENCE;
            match (&mut open, usable) {
                (Some(chunk), true) if chunk.t1_us == s.t0_us => {
                    chunk.t1_us = s.t1_us;
                    chunk.dx += f64::from(s.motion.dx);
                    chunk.dy += f64::from(s.motion.dy);
                }
                (_, true) => {
                    open = Some(Chunk {
                        t0_us: s.t0_us,
                        t1_us: s.t1_us,
                        dx: f64::from(s.motion.dx),
                        dy: f64::from(s.motion.dy),
                    });
                }
                (_, false) => open = None,
            }
            if let Some(chunk) = open.filter(|c| c.t1_us - c.t0_us >= CHUNK_US) {
                out.push(chunk);
                open = None;
            }
        }
        out
    }

    /// The model as it stands, if it differs enough from the last one sent.
    fn take_model(&mut self) -> Option<Model> {
        let model = self.model(self.generation.wrapping_add(1));
        let changed = match self.sent {
            None => model.locked(),
            Some(sent) => {
                let moved = |a: [f32; 2], b: [f32; 2]| {
                    (0..2).any(|i| {
                        let scale = a[i].abs().max(b[i].abs()).max(1e-6);
                        (a[i] - b[i]).abs() / scale > GAIN_CHANGE
                    })
                };
                sent.mouse_locked != model.mouse_locked
                    || sent.stick_locked != model.stick_locked
                    || model.mouse_locked && moved(sent.mouse_gain, model.mouse_gain)
                    || model.stick_locked && moved(sent.stick_gain, model.stick_gain)
                    || model.stick_locked
                        && (sent.stick_deadzone != model.stick_deadzone
                            || sent.stick_exponent != model.stick_exponent)
                    || model.locked() && sent.lag_us.abs_diff(model.lag_us) > LAG_CHANGE_US
            }
        };
        if !changed {
            return None;
        }
        self.generation = model.generation;
        self.sent = Some(model);
        Some(model)
    }

    /// The model for generation `generation`.
    pub fn model(&self, generation: u16) -> Model {
        let picture = |gain: [f64; 2]| {
            [
                (gain[0] * self.scale[0]) as f32,
                (gain[1] * self.scale[1]) as f32,
            ]
        };
        let mouse = self.mouse.unwrap_or(Fit {
            gain: [0.0; 2],
            lag_us: DEFAULT_LAG_US,
            fit: 0.0,
            deadzone: 0.0,
            exponent: 1.0,
        });
        let stick = self.stick.unwrap_or(Fit {
            gain: [0.0; 2],
            lag_us: mouse.lag_us,
            fit: 0.0,
            deadzone: 0.15,
            exponent: 1.0,
        });
        let confidence = match (self.mouse_locked, self.stick_locked) {
            (true, true) => mouse.fit.max(stick.fit),
            (true, false) => mouse.fit,
            (false, true) => stick.fit,
            (false, false) => mouse.fit.max(stick.fit),
        };
        let clamp_gain = |g: [f32; 2]| {
            g.map(|v| {
                if v.is_finite() {
                    v.clamp(-100_000.0, 100_000.0)
                } else {
                    0.0
                }
            })
        };
        Model {
            generation,
            mouse_locked: self.mouse_locked,
            stick_locked: self.stick_locked,
            picture_width: self.picture.0,
            picture_height: self.picture.1,
            mouse_gain: clamp_gain(picture(mouse.gain)),
            stick_gain: clamp_gain(picture(stick.gain)),
            stick_deadzone: stick.deadzone.clamp(0.0, 0.5),
            stick_exponent: if STICK_EXPONENTS.contains(&stick.exponent) {
                stick.exponent
            } else {
                1.0
            },
            max_offset: [
                f32::from(self.picture.0) * MAX_OFFSET,
                f32::from(self.picture.1) * MAX_OFFSET,
            ],
            lag_us: if self.mouse_locked || !self.stick_locked {
                mouse.lag_us
            } else {
                stick.lag_us
            } as u32,
            confidence: (confidence as f32).clamp(0.0, 1.0),
        }
    }
}

/// The best lag for `input`, and the gains there.
fn best_fit(
    chunks: &[Chunk],
    input: &dyn Fn(&Chunk, u64) -> Option<[f64; 2]>,
    min_input: f64,
) -> Option<Fit> {
    let mut fits = Vec::new();
    let mut lag = 0;
    while lag <= MAX_LAG_US {
        if let Some(fit) = score(chunks, input, lag, None, min_input) {
            fits.push(fit);
        }
        lag += LAG_STEP_US;
    }
    let best = fits.iter().map(|f| f.fit).fold(f64::NEG_INFINITY, f64::max);
    if !best.is_finite() {
        return None;
    }
    fits.into_iter().find(|f| f.fit >= best * LAG_TOLERANCE)
}

/// The fit of `input` at `lag`: with `gain` given, how much of the motion it explains; without,
/// the least-squares gains and theirs. `None` without enough input or motion.
///
/// An axis with too little input of its own takes the other axis's gain (square pixels, the
/// same sensitivity both ways, as games default to).
fn score(
    chunks: &[Chunk],
    input: &dyn Fn(&Chunk, u64) -> Option<[f64; 2]>,
    lag: u64,
    gain: Option<[f64; 2]>,
    min_input: f64,
) -> Option<Fit> {
    let (mut uu, mut um, mut mm, mut used_input) =
        ([0.0f64; 2], [0.0f64; 2], [0.0f64; 2], [0.0f64; 2]);
    let mut moved = 0.0;
    for chunk in chunks {
        let Some(u) = input(chunk, lag) else { continue };
        let m = [chunk.dx, chunk.dy];
        for axis in 0..2 {
            uu[axis] += u[axis] * u[axis];
            um[axis] += u[axis] * m[axis];
            mm[axis] += m[axis] * m[axis];
            used_input[axis] += u[axis].abs();
        }
        moved += m[0].abs() + m[1].abs();
    }
    // A fresh fit needs motion to fit; checking a given gain does not, as a picture that
    // stands still while input arrives is exactly what must fail it.
    if used_input[0] + used_input[1] < min_input || gain.is_none() && moved < MIN_MOTION {
        return None;
    }
    let checking = gain.is_some();
    let enough = |axis: usize| used_input[axis] >= min_input * 0.5 && uu[axis] > 0.0;
    let gain = gain.unwrap_or_else(|| {
        let own = |axis: usize| um[axis] / uu[axis];
        match (enough(0), enough(1)) {
            (true, true) => [own(0), own(1)],
            (true, false) => [own(0), own(0)],
            (false, true) => [own(1), own(1)],
            (false, false) => [0.0, 0.0],
        }
    });
    // Explained share: 1 - residual / total, residual = sum (m - g u)^2. A given gain is
    // checked against the measured and the predicted motion together, so a still picture
    // scores 0 instead of having nothing to compare with.
    let residual: f64 = (0..2)
        .map(|axis| mm[axis] - 2.0 * gain[axis] * um[axis] + gain[axis] * gain[axis] * uu[axis])
        .sum::<f64>()
        .max(0.0);
    let predicted: f64 = (0..2).map(|axis| gain[axis] * gain[axis] * uu[axis]).sum();
    let total = if checking {
        (mm[0] + mm[1] + predicted) * 0.5
    } else {
        mm[0] + mm[1]
    };
    if total <= 0.0 {
        return None;
    }
    Some(Fit {
        gain,
        lag_us: lag,
        fit: (1.0 - residual / total).clamp(0.0, 1.0),
        deadzone: 0.0,
        exponent: 1.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A game at 60 fps captured at 120 fps: frame `i` shows the camera as of the input that
    /// arrived `lag_us` before it was rendered, `gain` thumbnail pixels per count.
    fn mouse_session(gain: f64, lag_us: u64, noise: f32) -> (InputHistory, Calibrator, u64) {
        let mut history = InputHistory::new(None);
        let mut cal = Calibrator::new((1920, 1080), (256, 144));
        let mut seed = 7u32;
        let mut random = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        };
        // Input every 2 ms: a sweeping turn with pauses.
        let end = 3_500_000u64;
        let mut t = 0;
        while t < end {
            let phase = t as f64 / 1e6;
            let speed = (phase * 2.3).sin() * (phase * 0.7).cos() * 9.0;
            let dy = ((phase * 1.1).sin() * 4.0) as i16;
            history.on_mouse(speed as i16, dy, t);
            t += 2_000;
        }
        let camera = |t: u64, h: &InputHistory| {
            let (x, y) = h.mouse_at(t.saturating_sub(lag_us));
            (x as f64 * gain, y as f64 * gain)
        };
        let frame_us = 16_667u64;
        let capture_us = 8_333u64;
        let mut t = capture_us;
        while t < end {
            // The picture captured at t shows the game's last frame, rendered at a multiple
            // of 16.7 ms.
            let rendered = |at: u64| at / frame_us * frame_us;
            let (x0, y0) = camera(rendered(t - capture_us), &history);
            let (x1, y1) = camera(rendered(t), &history);
            cal.add(Sample {
                t0_us: t - capture_us,
                t1_us: t,
                motion: Motion {
                    // The content moves against the camera.
                    dx: (-(x1 - x0)) as f32 + noise * random(),
                    dy: (-(y1 - y0)) as f32 + noise * random(),
                    confidence: 0.9,
                },
            });
            t += capture_us;
        }
        (history, cal, end)
    }

    #[test]
    fn learns_a_mouse_gain_and_lag() {
        let (history, mut cal, end) = mouse_session(0.12, 50_000, 0.3);
        let model = cal.update(&history, end).expect("a locked model");
        assert!(model.mouse_locked && !model.stick_locked, "{model:?}");
        // 0.12 thumbnail px per count is 0.9 picture px per count at 1920 / 256.
        assert!((model.mouse_gain[0] + 0.9).abs() < 0.05, "{model:?}");
        assert!(
            (model.mouse_gain[1] + 0.12 * 1080.0 / 144.0).abs() < 0.08,
            "{model:?}"
        );
        // Within a game frame of the true lag, and not later than it plus one.
        assert!(
            model.lag_us >= 30_000 && model.lag_us <= 70_000,
            "{model:?}"
        );
        assert_eq!(model.max_offset, [288.0, 162.0]);
        assert_eq!(model.generation, 1);
        // Nothing new: no new model.
        assert_eq!(cal.update(&history, end + EVERY_US), None);
    }

    #[test]
    fn motion_that_ignores_the_mouse_never_locks() {
        let (history, _, end) = mouse_session(0.12, 50_000, 0.3);
        let mut cal = Calibrator::new((1920, 1080), (256, 144));
        // A menu: the mouse moves, the picture does not (only noise).
        let mut t = 8_333;
        while t < end {
            cal.add(Sample {
                t0_us: t - 8_333,
                t1_us: t,
                motion: Motion {
                    dx: ((t / 8_333) % 3) as f32 - 1.0,
                    dy: 0.0,
                    confidence: 0.9,
                },
            });
            t += 8_333;
        }
        assert_eq!(cal.update(&history, end), None);
        assert!(!cal.locked());
    }

    #[test]
    fn a_locked_model_unlocks_when_the_picture_stops_following() {
        let (history, mut cal, end) = mouse_session(0.12, 50_000, 0.3);
        assert!(cal.update(&history, end).is_some_and(|m| m.mouse_locked));
        // A cutscene: input continues (the same history), the picture stands still.
        let mut extra = history;
        let mut t = end;
        while t < end + 1_500_000 {
            extra.on_mouse(8, 0, t);
            t += 2_000;
        }
        let mut t = end;
        while t < end + 1_500_000 {
            cal.add(Sample {
                t0_us: t,
                t1_us: t + 8_333,
                motion: Motion {
                    dx: 0.0,
                    dy: 0.0,
                    confidence: 0.9,
                },
            });
            t += 8_333;
        }
        let model = cal.update(&extra, end + 1_500_000).expect("an unlock");
        assert!(!model.mouse_locked, "{model:?}");
    }

    #[test]
    fn a_prior_survives_the_round_trip_in_picture_pixels() {
        let (history, mut cal, end) = mouse_session(0.12, 50_000, 0.3);
        cal.update(&history, end);
        let prior = cal.prior().unwrap();
        assert!(
            (prior.mouse_gain.unwrap()[0] + 0.9).abs() < 0.05,
            "{prior:?}"
        );
        // A new session at a different size starts from it, scaled.
        let mut next = Calibrator::new((1280, 720), (256, 144));
        next.set_prior(prior);
        let model = next.model(1);
        assert!((model.mouse_gain[0] + 0.6).abs() < 0.04, "{model:?}");
        assert!(!model.mouse_locked);
        assert_eq!(next.mouse_lag_us(), prior.mouse_lag_us.unwrap());
    }

    #[test]
    fn learns_a_stick_gain_with_a_dead_zone() {
        let mut history = InputHistory::new(Some(0));
        let mut cal = Calibrator::new((1920, 1080), (256, 144));
        let lag = 34_000u64;
        let gain = 60.0f64; // thumbnail px per second of full deflection
        let end = 3_500_000u64;
        // Flicks: the stick holds a level for 120 to 280 ms, then jumps to another.
        const LEVELS: [f64; 7] = [-1.0, -0.6, -0.3, 0.0, 0.35, 0.7, 1.0];
        let mut seed = 0x2545_f491u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let (mut level, mut until) = (0.0f64, 0u64);
        let mut t = 0;
        while t < end {
            if t >= until {
                level = LEVELS[next() as usize % LEVELS.len()];
                until = t + 120_000 + u64::from(next() % 160_001);
            }
            history.on_controller(0, ((level * 32767.0) as i16, 0), t);
            t += 8_000;
        }
        let mut t = 8_333;
        while t < end {
            let (a, b) = ((t - 8_333).saturating_sub(lag), t.saturating_sub(lag));
            let turned = history.stick_integral(a, b, 0.20, 1.5);
            cal.add(Sample {
                t0_us: t - 8_333,
                t1_us: t,
                motion: Motion {
                    dx: (-turned[0] * gain) as f32,
                    dy: 0.0,
                    confidence: 0.9,
                },
            });
            t += 8_333;
        }
        let model = cal.update(&history, end).expect("a locked model");
        assert!(model.stick_locked && !model.mouse_locked, "{model:?}");
        assert_eq!(
            (model.stick_deadzone, model.stick_exponent),
            (0.20, 1.5),
            "{model:?}"
        );
        assert!((model.stick_gain[0] + 60.0 * 7.5).abs() < 15.0, "{model:?}");
        assert!(model.lag_us.abs_diff(34_000) <= 4_000, "{model:?}");
    }
}
