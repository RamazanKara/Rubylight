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

use rubylight_protocol::frame_warp::{MAX_FOCAL, MIN_FOCAL, Model, STICK_EXPONENTS};

use super::history::{InputHistory, StickIntegrals};
use super::lens::{Layout, Turn};
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
/// How much more of the motion a fresh fit must explain than the locked model does over the
/// same samples to replace it. Without it every fit (5 a second) replaced the model: on smooth
/// input many lags fit almost equally, the chosen lag wandered by 10-20 ms between fits, and each
/// change moved every frame's tag by the input in between, a visible jolt while turning.
const REFIT_MARGIN: f64 = 0.03;
/// Or this share of what the locked model leaves unexplained, when that is less: at a 99% fit
/// no fit could explain 3 points more, and a lag a few milliseconds off, picked on the first few
/// turns, would stay for good.
const REFIT_SHARE: f64 = 0.25;
/// A locked model is sent again when a gain moves by this fraction or the lag by this much. A
/// gain change is seamless on the client (the shift of the input still outstanding scales with
/// it); a lag change moves every tag.
const GAIN_CHANGE: f32 = 0.02;
const LAG_CHANGE_US: u32 = 3_000;
/// Largest shift, as a fraction of the picture's width and height.
pub const MAX_OFFSET: f32 = 0.15;
/// Under a lens the shift at the centre also stays within this share of the squared focal length
/// over the picture's edge: at the whole of it the turn would bring the picture's edge level
/// with the camera, and near it the edge is stretched beyond use.
const MAX_LENS_OFFSET: f64 = 0.6;
/// A model is sent again when its focal length moves by this fraction.
const FOCAL_CHANGE: f32 = 0.03;
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
    /// The game camera's focal length in pixels of the picture below, if it has a lens
    /// ([`super::lens`]); the gains are then the picture centre's.
    #[serde(default)]
    pub focal: Option<f32>,
    /// The size of the game's picture the gains are in: the stream's, less any black bars.
    pub picture_width: u16,
    pub picture_height: u16,
}

#[derive(Clone, Debug)]
pub struct Calibrator {
    samples: VecDeque<Sample>,
    /// Picture pixels per thumbnail pixel, x and y.
    scale: [f64; 2],
    /// The stream's picture, which the model is sent for.
    picture: (u16, u16),
    /// The game's part of it, which the thumbnails cover: the picture less any black bars.
    content: (u16, u16),
    /// Where the motion estimate's cells lie in that part.
    layout: Layout,
    /// The game's lens, as the squared tangent of half its horizontal field of view; `None`
    /// for a flat picture or one not learned yet. With one, samples count as the turn at the
    /// picture's centre ([`Layout::resolve`]) and so do the gains.
    lens: Option<f64>,
    /// Whether the model carries the lens ([`Self::set_lens_drawn`]).
    lens_drawn: bool,
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
            content: picture,
            layout: Layout::new(picture, thumbnail),
            lens: None,
            lens_drawn: true,
            mouse: None,
            stick: None,
            mouse_locked: false,
            stick_locked: false,
            last_fit_us: None,
            generation: 0,
            sent: None,
            prior: None,
        };
        out.set_sizes(picture, picture, thumbnail);
        out
    }

    /// A new stream or thumbnail size: the stream's `picture`, the `content` of it the game
    /// fills (the picture less black bars) and the `thumbnail` of that content. What was
    /// learned carries over in picture pixels.
    pub fn set_sizes(
        &mut self,
        picture: (u16, u16),
        content: (u16, u16),
        thumbnail: (usize, usize),
    ) {
        let old_scale = self.scale;
        self.picture = (picture.0.max(1), picture.1.max(1));
        self.content = (content.0.max(1), content.1.max(1));
        self.scale = [
            f64::from(self.content.0) / thumbnail.0.max(1) as f64,
            f64::from(self.content.1) / thumbnail.1.max(1) as f64,
        ];
        self.layout = Layout::new(self.content, thumbnail);
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

    /// The earliest input time a fit reads: the oldest sample's start less [`MAX_LAG_US`], or
    /// `None` without samples ([`InputHistory::since`]).
    pub fn earliest_input_us(&self) -> Option<u64> {
        self.samples
            .iter()
            .map(|s| s.t0_us)
            .min()
            .map(|t0| t0.saturating_sub(MAX_LAG_US))
    }

    /// The game's lens changed ([`super::lens::Lens`]). The samples kept are read again under
    /// it, so the next fit's gains are the picture centre's under the new lens.
    pub fn set_lens(&mut self, lens: Option<f64>) {
        self.lens = lens.filter(|t| t.is_finite() && *t > 0.0);
    }

    /// Whether the client draws the picture through the lens. One that slides it is sent no
    /// focal length and no limit for one; its gains are the picture centre's either way.
    pub fn set_lens_drawn(&mut self, drawn: bool) {
        self.lens_drawn = drawn;
    }

    pub fn lens(&self) -> Option<f64> {
        self.lens
    }

    /// Where the motion estimate's cells lie in the game's picture.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// The camera's turn in a measured move, under the lens in force.
    pub fn resolve(&self, motion: &Motion) -> Turn {
        self.layout.resolve(motion, self.lens)
    }

    /// Starts from what an earlier session learned: the lags apply at once and the gains lock
    /// as soon as recent input confirms them.
    pub fn set_prior(&mut self, prior: Prior) {
        let scale = |gain: [f32; 2]| {
            let ratio = [
                f64::from(self.content.0) / f64::from(prior.picture_width.max(1)),
                f64::from(self.content.1) / f64::from(prior.picture_height.max(1)),
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
        if let Some(focal) = prior.focal.filter(|f| f.is_finite() && *f > 0.0) {
            // A field of view is the same at every picture size.
            let ratio = f64::from(prior.picture_width.max(1)) * 0.5 / f64::from(focal);
            self.set_lens(Some(ratio * ratio));
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
        let focal = self.lens.map(|lens| self.layout.focal(lens) as f32);
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
            focal,
            picture_width: self.content.0,
            picture_height: self.content.1,
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

    /// Whether [`Self::update`] would fit at `now_us`.
    pub fn due(&self, now_us: u64) -> bool {
        self.last_fit_us
            .is_none_or(|at| now_us.saturating_sub(at) >= EVERY_US)
    }

    /// Fits again if it is time, and returns the model to send when it changed enough to.
    pub fn update(&mut self, history: &InputHistory, now_us: u64) -> Option<Model> {
        if !self.due(now_us) {
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
        let mut fresh = best_fit(all, &input, MIN_MOUSE_COUNTS);
        if self.mouse_locked
            && let Some(model) = self.mouse.as_mut()
        {
            keep_unless_better(model, &mut fresh, |m, gain| {
                explained(all, &input, m.lag_us, gain, MIN_MOUSE_COUNTS)
            });
        }
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
        // Each curve's running integral is built once per fit (every chunk at every lag reads
        // it with two binary searches, instead of walking the chunk's pad packets).
        let input = |curve: &StickIntegrals<'_>, c: &Chunk, lag: u64| {
            let (t0, t1) = (c.t0_us.saturating_sub(lag), c.t1_us.saturating_sub(lag));
            let (mx, my) = history.mouse_between(t0, t1);
            if mx != 0 || my != 0 {
                return None;
            }
            Some(curve.between(t0, t1))
        };
        // The lag with a middling curve, then the curve at that lag.
        let middling = history.stick_integrals(0.15, 1.0);
        let mut fresh =
            best_fit(all, &|c, lag| input(&middling, c, lag), MIN_STICK_SECONDS).map(|mut f| {
                f.deadzone = 0.15;
                f.exponent = 1.0;
                f
            });
        if let Some(found) = fresh {
            for deadzone in DEADZONES {
                for exponent in [1.0, 1.5, 2.0] {
                    let curve = if (deadzone, exponent) == (0.15, 1.0) {
                        None
                    } else {
                        Some(history.stick_integrals(deadzone, exponent))
                    };
                    let curve = curve.as_ref().unwrap_or(&middling);
                    if let Some(mut fit) = score(
                        all,
                        &|c, lag| input(curve, c, lag),
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
            if let Some(curve) = fresh {
                let integrals = history.stick_integrals(curve.deadzone, curve.exponent);
                if let Some(mut refit) =
                    best_fit(all, &|c, lag| input(&integrals, c, lag), MIN_STICK_SECONDS)
                {
                    refit.deadzone = curve.deadzone;
                    refit.exponent = curve.exponent;
                    fresh = Some(refit);
                }
            }
        }
        if self.stick_locked
            && let Some(model) = self.stick.as_mut()
        {
            let integrals = history.stick_integrals(model.deadzone, model.exponent);
            keep_unless_better(model, &mut fresh, |m, gain| {
                explained(
                    all,
                    &|c, lag| input(&integrals, c, lag),
                    m.lag_us,
                    gain,
                    MIN_STICK_SECONDS,
                )
            });
        }
        let current = if self.stick_locked {
            self.stick
        } else {
            fresh.or(self.stick)
        };
        let recent_fit = current.and_then(|f| {
            let integrals = history.stick_integrals(f.deadzone, f.exponent);
            score(
                recent,
                &|c, lag| input(&integrals, c, lag),
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
            let turn = self.resolve(&s.motion);
            let usable = turn.confidence >= MIN_CONFIDENCE;
            match (&mut open, usable) {
                (Some(chunk), true) if chunk.t1_us == s.t0_us => {
                    chunk.t1_us = s.t1_us;
                    chunk.dx += f64::from(turn.dx);
                    chunk.dy += f64::from(turn.dy);
                }
                (_, true) => {
                    open = Some(Chunk {
                        t0_us: s.t0_us,
                        t1_us: s.t1_us,
                        dx: f64::from(turn.dx),
                        dy: f64::from(turn.dy),
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
                    || model.locked()
                        && (sent.focal - model.focal).abs()
                            > FOCAL_CHANGE * sent.focal.max(model.focal)
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
        // In picture pixels, which the game's part of the picture shares.
        let focal = self
            .lens
            .filter(|_| self.lens_drawn)
            .map(|lens| self.layout.focal(lens))
            .filter(|f| (f64::from(MIN_FOCAL)..=f64::from(MAX_FOCAL)).contains(f));
        let lens_limit = |edge: u16| {
            focal.map_or(f32::INFINITY, |f| {
                (MAX_LENS_OFFSET * f * f / f64::from(edge.max(1))) as f32
            })
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
                (f32::from(self.picture.0) * MAX_OFFSET).min(lens_limit(self.picture.0)),
                (f32::from(self.picture.1) * MAX_OFFSET).min(lens_limit(self.picture.1)),
            ],
            lag_us: if self.mouse_locked || !self.stick_locked {
                mouse.lag_us
            } else {
                stick.lag_us
            } as u32,
            confidence: (confidence as f32).clamp(0.0, 1.0),
            focal: focal.map_or(0.0, |f| f as f32),
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

/// A locked `model` keeps its lag unless `fresh` explains at least [`REFIT_MARGIN`] more of the
/// motion than `model` does over the same samples (`at_lag`, with `model`'s gain), or
/// [`REFIT_SHARE`] of what `model` leaves unexplained when that is less; otherwise `fresh`
/// is dropped and `model` takes the least-squares gain at its lag (`at_lag` with `None`) when
/// that explains as much. That gain settles as samples gather, so a model locked early on a
/// few turns still converges, where a lag picked afresh would wander.
fn keep_unless_better(
    model: &mut Fit,
    fresh: &mut Option<Fit>,
    at_lag: impl Fn(&Fit, Option<[f64; 2]>) -> Option<Fit>,
) {
    let Some(found) = *fresh else { return };
    if let Some(now) = at_lag(model, Some(model.gain))
        && now.fit + REFIT_MARGIN.min(REFIT_SHARE * (1.0 - now.fit).max(0.0)) >= found.fit
    {
        let settled = at_lag(model, None).filter(|fit| fit.fit >= now.fit);
        model.gain = settled.map_or(model.gain, |fit| fit.gain);
        model.fit = settled.map_or(now.fit, |fit| fit.fit);
        *fresh = None;
    }
}

/// How much of the motion `gain` at `lag` explains, or without one the least-squares gain there
/// and its share, counted as a fresh fit's share is (against the measured motion alone), so the
/// two compare.
fn explained(
    chunks: &[Chunk],
    input: &dyn Fn(&Chunk, u64) -> Option<[f64; 2]>,
    lag: u64,
    gain: Option<[f64; 2]>,
    min_input: f64,
) -> Option<Fit> {
    fit_at(chunks, input, lag, gain, min_input, false)
}

/// The fit of `input` at `lag`: with `gain` given, how much of the motion it explains; without,
/// the least-squares gains and theirs. `None` without enough input or motion.
///
/// An axis with too little input of its own, under half of `min_input` or a quarter of the other
/// axis's, takes the other axis's gain (square pixels, the same sensitivity both ways, as games
/// default to): a gain fitted to a sliver of input, a turn's slight drift up or down, mostly fits
/// whatever else moves the picture that way (a ship's roll) and changes with every fit.
fn score(
    chunks: &[Chunk],
    input: &dyn Fn(&Chunk, u64) -> Option<[f64; 2]>,
    lag: u64,
    gain: Option<[f64; 2]>,
    min_input: f64,
) -> Option<Fit> {
    fit_at(chunks, input, lag, gain, min_input, gain.is_some())
}

/// [`score`], with `checking` saying how a given gain is counted: against the measured and the
/// predicted motion together (`true`), or against the measured motion alone like a fresh fit.
fn fit_at(
    chunks: &[Chunk],
    input: &dyn Fn(&Chunk, u64) -> Option<[f64; 2]>,
    lag: u64,
    gain: Option<[f64; 2]>,
    min_input: f64,
    checking: bool,
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
    if used_input[0] + used_input[1] < min_input || !checking && moved < MIN_MOTION {
        return None;
    }
    let enough = |axis: usize| {
        used_input[axis] >= min_input * 0.5
            && used_input[axis] * 4.0 >= used_input[1 - axis]
            && uu[axis] > 0.0
    };
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
mod tests;
