//! Frame Warp 2 on the host: what a client gets on top of the calibrator's model, tags and
//! HUD mask.
//!
//! A [`V2State`] lives in the [`FrameWarpHost`] while Frame Warp is on: a version 2 request
//! (`rubylight_protocol::frame_warp_v2::RequestV2`) makes it, a stop drops it, and a version 1
//! request changes nothing (the host does not run Frame Warp 1). It learns from what the host
//! already has (the measured moves, the HUD masks, the mouse and stick history) and the gyro
//! pad's motion packets:
//!
//! - fast fits of each kind of input ([`super::rls`]) and of the field of view
//!   ([`super::lens_fast`]), which give the client a locked model as soon as they lock, before
//!   the calibrator does: that model's gains, curve and lags, the calibrator's for each kind it
//!   has locked, under the request's limit of the shift; the tags then count by that model's
//!   lags and carry its generation;
//! - the gyro's gain and yaw axis ([`super::gyro`]);
//! - the camera confidence and its state machine ([`super::confidence`]);
//! - and with every frame's tags one [`FrameWarpV2Meta`] (0x5549, [`FrameWarpHost::take_meta`]).
//!
//! What it learned is kept for the game's next stream as a [`PriorV2`].

use std::collections::VecDeque;

use rubylight_protocol::frame_warp::{MAX_GAIN, MODEL_BYTES, Mask, Model};
use rubylight_protocol::frame_warp_v2::{
    DEFAULT_EDGE_FADE, DEFAULT_HUD_GUARD, FrameWarpV2Meta, META_BYTES, RequestV2, meta_flags,
    to_unit16,
};

use super::calibrate::{CHUNK_US, MAX_OFFSET};
use super::confidence::{Confidence, HudStability, Scene};
use super::gyro::{GyroHistory, YawChoice};
use super::history::InputHistory;
use super::lens::Turn;
use super::lens_fast::LensFast;
use super::rls::{Kind, Rls, RlsFit};
use super::{FrameWarpHost, Motion};

mod prior;

pub(super) use prior::GyroPrior;
pub use prior::PriorV2;
use prior::Warm;

/// Moves whose estimate is less sure than this are left out, as the calibrator leaves them out.
const MIN_CONFIDENCE: f32 = 0.35;
/// Encoded metas kept for the control loop; older ones go (unreliable anyway).
const PENDING_METAS: usize = 8;
/// A model is sent again when a gain moves by this fraction, the lag by this much or the focal
/// length by this fraction (the calibrator's thresholds).
const GAIN_CHANGE: f32 = 0.02;
const LAG_CHANGE_US: u32 = 3_000;
const FOCAL_CHANGE: f32 = 0.03;
/// How far the camera's rate moves toward each chunk's.
const OMEGA_RATE: f64 = 0.5;
/// The HUD mask counts as soft-edged while its steadiness is under this.
const HUD_STEADY: f32 = 0.9;

/// Measured moves summed until they span [`CHUNK_US`].
#[derive(Clone, Copy, Debug, PartialEq)]
struct Chunk {
    t0_us: u64,
    t1_us: u64,
    dx: f64,
    dy: f64,
}

/// Frame Warp 2's state (see the module documentation).
#[derive(Debug)]
pub struct V2State {
    request: RequestV2,
    gyro: GyroHistory,
    yaw: YawChoice,
    mouse: Rls,
    stick: Rls,
    /// The gyro's fits with the yaw from the packets' y and z axes.
    gyro_fit: [Rls; 2],
    lens: LensFast,
    confidence: Confidence,
    hud: HudStability,
    chunk: Option<Chunk>,
    /// The camera's yaw and pitch rate, radians per second, smoothed.
    omega: [f64; 2],
    /// The generation of the last model sent to this client.
    generation: u16,
    /// That model and the lags of mouse and pad its tags count by.
    sent: Option<(Model, u64, u64)>,
    /// Send the model whatever it is (a request).
    force: bool,
    metas: VecDeque<[u8; META_BYTES]>,
    /// Picture pixels per thumbnail pixel, x and y.
    scale: [f64; 2],
}

impl V2State {
    fn new(request: RequestV2, generation: u16, scale: [f64; 2]) -> Self {
        Self {
            request,
            gyro: GyroHistory::new(request.gyro_pad(), request.axes),
            yaw: YawChoice::new(request.axes.yaw),
            mouse: Rls::new(Kind::Mouse),
            stick: Rls::new(Kind::Stick),
            gyro_fit: [Rls::new(Kind::Gyro), Rls::new(Kind::Gyro)],
            lens: LensFast::default(),
            confidence: Confidence::new(false),
            hud: HudStability::default(),
            chunk: None,
            omega: [0.0; 2],
            generation,
            sent: None,
            force: true,
            metas: VecDeque::new(),
            scale,
        }
    }

    pub fn request(&self) -> &RequestV2 {
        &self.request
    }

    pub fn confidence(&self) -> &Confidence {
        &self.confidence
    }

    /// The fast fit of mouse, stick and the gyro (with the yaw axis in force), while locked.
    pub fn locked_fits(&self) -> [Option<RlsFit>; 3] {
        [
            self.mouse.locked_fit(),
            self.stick.locked_fit(),
            self.gyro_locked_fit(),
        ]
    }

    /// The fast lens's field of view, degrees, once decided.
    pub fn fast_degrees(&self) -> Option<f64> {
        self.lens.degrees()
    }

    fn gyro_rls(&self) -> &Rls {
        &self.gyro_fit[usize::from(self.yaw.yaw_z())]
    }

    fn gyro_locked_fit(&self) -> Option<RlsFit> {
        self.gyro.pad().and_then(|_| self.gyro_rls().locked_fit())
    }

    /// Starts the fits that have not locked from a saved prior (thumbnail pixels per unit).
    fn warm(&mut self, warm: &Warm) {
        let mut any = false;
        if let Some((gain, lag)) = warm.mouse
            && !self.mouse.locked()
        {
            self.mouse = Rls::warm(Kind::Mouse, gain, lag, None);
            any = true;
        }
        if let Some((gain, lag, curve)) = warm.stick
            && !self.stick.locked()
        {
            self.stick = Rls::warm(Kind::Stick, gain, lag, Some(curve));
            any = true;
        }
        if let Some((gain, lag)) = warm.gyro
            && self.gyro.pad().is_some()
        {
            for fit in self.gyro_fit.iter_mut().filter(|f| !f.locked()) {
                *fit = Rls::warm(Kind::Gyro, gain, lag, None);
            }
            any = true;
        }
        if let Some(yaw_z) = warm.yaw_z {
            self.yaw.prefer(yaw_z);
        }
        if any && !self.any_locked() {
            self.confidence = Confidence::new(true);
        }
    }

    fn any_locked(&self) -> bool {
        self.mouse.locked() || self.stick.locked() || self.gyro_locked_fit().is_some()
    }

    fn any_warm(&self) -> bool {
        self.mouse.is_warm()
            || self.stick.is_warm()
            || self.gyro.pad().is_some() && self.gyro_rls().is_warm()
    }

    fn warm_locked(&self) -> bool {
        self.mouse.warm_locked()
            || self.stick.warm_locked()
            || self.gyro_locked_fit().is_some() && self.gyro_rls().warm_locked()
    }

    /// The best explained share of any fast fit, for the confidence while none is locked.
    fn best_fit(&self) -> f64 {
        [
            self.mouse.fit(),
            self.stick.fit(),
            self.gyro.pad().and_then(|_| self.gyro_rls().fit()),
        ]
        .into_iter()
        .flatten()
        .map(|f| f.fit)
        .fold(0.0, f64::max)
    }

    /// A new thumbnail scale: what was learned carries over in picture pixels, warm.
    fn rescale(&mut self, scale: [f64; 2]) {
        if scale == self.scale {
            return;
        }
        let ratio = [self.scale[0] / scale[0], self.scale[1] / scale[1]];
        let carry = |rls: &Rls| match rls.fit() {
            Some(fit) => Rls::warm(
                rls.kind(),
                [fit.gain[0] * ratio[0], fit.gain[1] * ratio[1]],
                fit.lag_us,
                Some((fit.deadzone, fit.exponent)),
            ),
            None => Rls::new(rls.kind()),
        };
        self.mouse = carry(&self.mouse);
        self.stick = carry(&self.stick);
        self.gyro_fit = [carry(&self.gyro_fit[0]), carry(&self.gyro_fit[1])];
        self.lens.clear();
        self.chunk = None;
        self.scale = scale;
    }

    /// A measured move under the lens in force: summed into chunks, each of which teaches the
    /// fits, checks the model the client has and moves the confidence.
    fn on_turn(&mut self, span: (u64, u64), turn: Turn, history: &InputHistory, focal: f64) {
        let (t0_us, t1_us) = span;
        if turn.confidence < MIN_CONFIDENCE {
            self.chunk = None;
            return;
        }
        let (dx, dy) = (f64::from(turn.dx), f64::from(turn.dy));
        let chunk = match self.chunk {
            Some(mut open) if open.t1_us == t0_us => {
                open.t1_us = t1_us;
                open.dx += dx;
                open.dy += dy;
                open
            }
            _ => Chunk {
                t0_us,
                t1_us,
                dx,
                dy,
            },
        };
        if chunk.t1_us - chunk.t0_us < CHUNK_US {
            self.chunk = Some(chunk);
            return;
        }
        self.chunk = None;
        self.feed(chunk, history, focal);
    }

    fn feed(&mut self, c: Chunk, history: &InputHistory, focal: f64) {
        let m = [c.dx, c.dy];
        let span = |lag: u64| (c.t0_us.saturating_sub(lag), c.t1_us.saturating_sub(lag));
        let gyro = &self.gyro;
        let gyro_on = gyro.pad().is_some();
        // Each kind learns only from chunks the others did not turn.
        self.mouse.update(m, |_, lag| {
            let (a, b) = span(lag);
            if history.stick_active(a, b) || gyro_on && gyro.active(a, b) {
                return None;
            }
            let (x, y) = history.mouse_between(a, b);
            Some([x as f64, y as f64])
        });
        self.stick.update(m, |(deadzone, exponent), lag| {
            let (a, b) = span(lag);
            if history.mouse_between(a, b) != (0, 0) || gyro_on && gyro.active(a, b) {
                return None;
            }
            Some(history.stick_integral(a, b, deadzone, exponent))
        });
        if gyro_on {
            for (yaw_z, fit) in [false, true].into_iter().zip(self.gyro_fit.iter_mut()) {
                fit.update(m, |_, lag| {
                    let (a, b) = span(lag);
                    if history.mouse_between(a, b) != (0, 0) || history.stick_active(a, b) {
                        return None;
                    }
                    Some(gyro.angle(a, b, yaw_z))
                });
            }
            let fits = [&self.gyro_fit[0], &self.gyro_fit[1]].map(|f| f.fit().map(|f| f.fit));
            self.yaw.observe(fits);
        }
        // The camera's rate: the content moves against it.
        let seconds = (c.t1_us - c.t0_us) as f64 / 1e6;
        if focal > 0.0 && seconds > 0.0 {
            for (axis, value) in self.omega.iter_mut().enumerate() {
                let rate = -m[axis] * self.scale[axis] / focal / seconds;
                *value += (rate - *value) * OMEGA_RATE;
            }
        }
        if let Some(predicted) = self.predict(c, history) {
            self.confidence.on_check(m, predicted, c.t1_us);
        }
    }

    /// What the model the client has (and the gyro's fit) predicted for a chunk, thumbnail
    /// pixels; `None` while nothing of it is locked.
    fn predict(&self, c: Chunk, history: &InputHistory) -> Option<[f64; 2]> {
        let span = |lag: u64| (c.t0_us.saturating_sub(lag), c.t1_us.saturating_sub(lag));
        let thumb = |g: [f32; 2]| {
            [
                f64::from(g[0]) / self.scale[0],
                f64::from(g[1]) / self.scale[1],
            ]
        };
        let mut predicted = [0.0f64; 2];
        let mut any = false;
        if let Some((model, mouse_lag, stick_lag)) = self.sent {
            if model.mouse_locked {
                let (a, b) = span(mouse_lag);
                let (x, y) = history.mouse_between(a, b);
                let g = thumb(model.mouse_gain);
                predicted[0] += g[0] * x as f64;
                predicted[1] += g[1] * y as f64;
                any = true;
            }
            if model.stick_locked {
                let (a, b) = span(stick_lag);
                let u = history.stick_integral(a, b, model.stick_deadzone, model.stick_exponent);
                let g = thumb(model.stick_gain);
                predicted[0] += g[0] * u[0];
                predicted[1] += g[1] * u[1];
                any = true;
            }
        }
        if let Some(fit) = self.gyro_locked_fit() {
            let (a, b) = span(fit.lag_us);
            let u = self.gyro.angle(a, b, self.yaw.yaw_z());
            predicted[0] += fit.gain[0] * u[0];
            predicted[1] += fit.gain[1] * u[1];
            any = true;
        }
        any.then_some(predicted)
    }

    /// The model the client is sent: the calibrator's `base`, with each kind the calibrator
    /// has not locked taken from its fast fit once that locked, under the request's limit of
    /// the shift. With the lags of mouse and pad its tags count by (the calibrator's are
    /// `lags`).
    fn compose(&self, base: Model, lags: (u64, u64)) -> (Model, u64, u64) {
        let mut model = base;
        let (mut mouse_lag, mut stick_lag) = lags;
        let picture = |g: [f64; 2]| {
            let clamp = |v: f64| {
                if v.is_finite() {
                    v.clamp(-f64::from(MAX_GAIN), f64::from(MAX_GAIN)) as f32
                } else {
                    0.0
                }
            };
            [clamp(g[0] * self.scale[0]), clamp(g[1] * self.scale[1])]
        };
        let mut fits = Vec::new();
        if !model.mouse_locked
            && let Some(fit) = self.mouse.locked_fit()
        {
            model.mouse_locked = true;
            model.mouse_gain = picture(fit.gain);
            mouse_lag = fit.lag_us;
            fits.push(fit.fit);
        }
        if !model.stick_locked
            && let Some(fit) = self.stick.locked_fit()
        {
            model.stick_locked = true;
            model.stick_gain = picture(fit.gain);
            model.stick_deadzone = fit.deadzone;
            model.stick_exponent = fit.exponent;
            stick_lag = fit.lag_us;
            fits.push(fit.fit);
        }
        if !fits.is_empty() {
            model.lag_us = if model.mouse_locked || !model.stick_locked {
                mouse_lag
            } else {
                stick_lag
            } as u32;
            model.confidence = fits
                .into_iter()
                .fold(f64::from(model.confidence), f64::max)
                .clamp(0.0, 1.0) as f32;
        }
        model.max_offset = self.max_offset(&model);
        (model, mouse_lag, stick_lag)
    }

    /// The model's limit of the shift with the request's share of the picture
    /// (`RequestV2::max_offset`) in place of the host's 15%, keeping a lens's tighter limit.
    fn max_offset(&self, model: &Model) -> [f32; 2] {
        let share = self.request.max_offset();
        let edges = [model.picture_width, model.picture_height];
        core::array::from_fn(|axis| {
            let edge = f32::from(edges[axis]);
            let flat = edge * MAX_OFFSET;
            let wanted = edge * share;
            if model.max_offset[axis] < flat - 0.01 {
                model.max_offset[axis].min(wanted)
            } else {
                wanted
            }
        })
    }

    /// The lags of mouse and pad the tags count by: the last model sent's.
    fn lags(&self) -> Option<(u64, u64)> {
        self.sent.map(|(_, mouse, stick)| (mouse, stick))
    }

    /// The meta of `frame`, captured at `capture_us`.
    fn meta(&self, frame: u32, capture_us: u64) -> FrameWarpV2Meta {
        let gyro_fit = self.gyro_locked_fit();
        let gyro_lag = gyro_fit
            .map(|f| f.lag_us)
            .or_else(|| self.gyro_rls().fit().map(|f| f.lag_us))
            .or_else(|| self.lags().map(|(mouse, _)| mouse))
            .unwrap_or(super::calibrate::DEFAULT_LAG_US);
        let (gyro_seq, gyro_held_us) = self.gyro.tag(capture_us.saturating_sub(gyro_lag));
        let gain = self
            .gyro
            .pad()
            .and_then(|_| self.gyro_rls().fit())
            .map_or([0.0; 2], |f| {
                let picture = |g: f64, scale: f64| {
                    let v = g * scale;
                    if v.is_finite() {
                        v.clamp(-f64::from(MAX_GAIN), f64::from(MAX_GAIN)) as f32
                    } else {
                        0.0
                    }
                };
                [
                    picture(f.gain[0], self.scale[0]),
                    picture(f.gain[1], self.scale[1]),
                ]
            });
        let mut flags = self.confidence.flags(capture_us);
        if gyro_fit.is_some() {
            flags |= meta_flags::GYRO_LOCKED;
        }
        if self.gyro.pad().is_some() && self.yaw.yaw_z() {
            flags |= meta_flags::GYRO_YAW_Z;
        }
        if self.warm_locked() {
            flags |= meta_flags::PRIOR_WARM_LOCKED;
        }
        if self.hud.known() && self.hud.value() < HUD_STEADY {
            flags |= meta_flags::HUD_SOFT_EDGE;
        }
        let milli = |rate: f64| (rate * 1000.0).round().clamp(-1e9, 1e9) as i32;
        FrameWarpV2Meta {
            flags,
            model_generation: self.generation,
            frame,
            camera_confidence: to_unit16(self.confidence.value()),
            hud_stability: to_unit16(self.hud.value()),
            gyro_seq,
            gyro_held_us,
            gyro_gain: gain,
            omega_mrad_s: [milli(self.omega[0]), milli(self.omega[1])],
            edge_fade_strength: DEFAULT_EDGE_FADE,
            hud_guard: DEFAULT_HUD_GUARD,
        }
    }
}

/// The hooks the host calls; each does nothing without a [`V2State`].
impl FrameWarpHost {
    /// Picture pixels per thumbnail pixel.
    fn v2_scale(&self) -> [f64; 2] {
        [
            f64::from(self.content.2.max(1)) / self.thumbnail.0.max(1) as f64,
            f64::from(self.content.3.max(1)) / self.thumbnail.1.max(1) as f64,
        ]
    }

    /// After a request: one that turns Frame Warp on keeps (the same request again, not
    /// starting) or makes the [`V2State`], and the model goes out again; a stop drops it.
    pub(super) fn v2_request(&mut self, request: RequestV2, starting: bool) {
        if !request.base.enabled {
            self.v2 = None;
            return;
        }
        if starting || self.v2.as_ref().is_none_or(|v2| v2.request != request) {
            // The generations go on counting, so a new state's first model is a new one.
            let generation = self.v2.as_ref().map_or(0, |v2| v2.generation);
            let mut v2 = Box::new(V2State::new(request, generation, self.v2_scale()));
            v2.warm(&self.v2_warm());
            self.v2 = Some(v2);
        }
        if let Some(v2) = self.v2.as_mut() {
            v2.force = true;
        }
    }

    /// A motion packet the control thread decoded at `at_us`: the gyro pad's gyro readings
    /// count for a client that turns by the gyro.
    pub fn on_motion(&mut self, number: u8, kind: u8, xyz: [f32; 3], at_us: u64) {
        if let Some(v2) = self.v2.as_mut().filter(|_| self.enabled) {
            v2.gyro.on_motion(number, kind, xyz, at_us);
        }
    }

    /// Whether the host's pointer shows: a menu or a pause, not the camera.
    pub fn on_cursor(&mut self, visible: bool) {
        if let Some(v2) = self.v2.as_mut() {
            v2.confidence.on_cursor(visible);
        }
    }

    /// The next meta to send (unreliable).
    pub fn take_meta(&mut self) -> Option<[u8; META_BYTES]> {
        self.v2.as_mut()?.metas.pop_front()
    }

    /// The [`V2State`], while Frame Warp is on.
    pub fn v2(&self) -> Option<&V2State> {
        self.v2.as_deref()
    }

    /// The thumbnail or picture size changed.
    pub(super) fn v2_resized(&mut self) {
        let scale = self.v2_scale();
        if let Some(v2) = self.v2.as_mut() {
            v2.rescale(scale);
        }
    }

    /// A measured thumbnail's scene: a cut drops the confidence.
    pub(super) fn v2_scene(&mut self, scene: Option<Scene>, motion: Option<&Motion>, at_us: u64) {
        let Some(v2) = self.v2.as_mut() else { return };
        if scene.is_some_and(|s| s.cut(motion.map(|m| m.confidence))) {
            v2.confidence.on_cut(at_us);
            v2.chunk = None;
        }
    }

    /// Whether the gyro turned the camera in the input that captures from `t0_us` to `t1_us`
    /// show (the lens learns from gyro turns too).
    pub(super) fn v2_turning(&self, t0_us: u64, t1_us: u64) -> bool {
        self.v2.as_deref().is_some_and(|v2| {
            let lag = self.calibrator.mouse_lag_us() + super::LENS_INPUT_MARGIN_US;
            v2.gyro.pad().is_some()
                && v2.gyro.active(
                    t0_us.saturating_sub(lag),
                    t1_us.saturating_sub(lag) + 2 * super::LENS_INPUT_MARGIN_US,
                )
        })
    }

    /// A move the player's turning made: the fast lens learns from it, and until the field of
    /// view the calibrator's own lens learns is decided, the fast one is in force.
    pub(super) fn v2_lens(&mut self, motion: &Motion) {
        let Some(v2) = self.v2.as_mut() else { return };
        if v2.lens.observe(self.calibrator.layout(), motion) {
            self.lens.set_prior(v2.lens.tangent());
            self.apply_lens();
            self.fit_epoch += 1;
        }
    }

    /// A confident measured move from `t0_us` to `t1_us`, as the turn at the picture's centre.
    pub(super) fn v2_turn(&mut self, span: (u64, u64), turn: Turn, now_us: u64) {
        let focal = self
            .calibrator
            .lens()
            .map(|lens| self.calibrator.layout().focal(lens))
            // No lens learned: a 90 degree field of view, for the rate only.
            .unwrap_or(f64::from(self.content.2.max(1)) * 0.5);
        let locked = self.calibrator.locked();
        let Some(v2) = self.v2.as_mut() else { return };
        v2.on_turn(span, turn, &self.history, focal);
        let (locked, warm, fit) = (locked || v2.any_locked(), v2.any_warm(), v2.best_fit());
        v2.confidence.update(locked, warm, fit, now_us.max(span.1));
    }

    /// A HUD mask the client is sent, as laid over the picture.
    pub(super) fn v2_hud(&mut self, placed: &Mask) {
        if let Some(v2) = self.v2.as_mut() {
            v2.hud.observe(placed);
        }
    }

    /// The lags of mouse and pad the tags count by, once a model was sent.
    pub(super) fn v2_lags(&self) -> Option<(u64, u64)> {
        self.v2.as_deref()?.lags()
    }

    /// The model generation the tags carry: that of the model the client was sent last.
    pub(super) fn wire_generation(&self) -> u16 {
        self.v2.as_deref().map_or(0, |v2| v2.generation)
    }

    /// The model the client was sent last, for the statistics.
    pub(super) fn v2_model(&self) -> Option<Model> {
        self.v2.as_deref()?.sent.map(|(model, ..)| model)
    }

    /// Queues frame `frame`'s meta, captured at `capture_us`.
    pub(super) fn v2_tag(&mut self, frame: u32, capture_us: u64) {
        let locked = self.calibrator.locked();
        let Some(v2) = self.v2.as_mut() else { return };
        let (locked, warm, fit) = (locked || v2.any_locked(), v2.any_warm(), v2.best_fit());
        v2.confidence.update(locked, warm, fit, capture_us);
        if let Some(wire) = v2.meta(frame, capture_us).encode() {
            if v2.metas.len() == PENDING_METAS {
                v2.metas.pop_front();
            }
            v2.metas.push_back(wire);
        }
    }

    /// The model to send, when it changed: the composed model ([`V2State`]); a change of the
    /// calibrator's shows in it when it matters.
    pub(super) fn v2_take_model(&mut self) -> Option<[u8; MODEL_BYTES]> {
        let base = self.calibrator.model(0);
        let lags = (
            self.calibrator.mouse_lag_us(),
            self.calibrator.stick_lag_us(),
        );
        let v2 = self.v2.as_mut()?;
        let (mut model, mouse_lag, stick_lag) = v2.compose(base, lags);
        let changed = v2.force || v2.sent.is_none_or(|(sent, ..)| changed(&sent, &model));
        if !changed {
            return None;
        }
        v2.force = false;
        v2.generation = v2.generation.wrapping_add(1);
        model.generation = v2.generation;
        v2.sent = Some((model, mouse_lag, stick_lag));
        model.encode()
    }
}

/// Whether `model` differs enough from `sent` to send it.
fn changed(sent: &Model, model: &Model) -> bool {
    let moved = |a: [f32; 2], b: [f32; 2], by: f32| {
        (0..2).any(|i| {
            let scale = a[i].abs().max(b[i].abs()).max(1e-6);
            (a[i] - b[i]).abs() / scale > by
        })
    };
    sent.mouse_locked != model.mouse_locked
        || sent.stick_locked != model.stick_locked
        || (sent.picture_width, sent.picture_height) != (model.picture_width, model.picture_height)
        || model.mouse_locked && moved(sent.mouse_gain, model.mouse_gain, GAIN_CHANGE)
        || model.stick_locked && moved(sent.stick_gain, model.stick_gain, GAIN_CHANGE)
        || model.stick_locked
            && (sent.stick_deadzone != model.stick_deadzone
                || sent.stick_exponent != model.stick_exponent)
        || model.locked() && sent.lag_us.abs_diff(model.lag_us) > LAG_CHANGE_US
        || model.locked()
            && (sent.focal - model.focal).abs() > FOCAL_CHANGE * sent.focal.max(model.focal)
        || moved(sent.max_offset, model.max_offset, GAIN_CHANGE)
}

#[cfg(test)]
mod tests;
