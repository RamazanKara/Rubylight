//! Frame Warp 2 through the host: what a client is sent while the fast fits learn the synthetic
//! 60 fps game of [`crate::frame_warp::rls::tests`]. No fit job runs, so the calibrator never
//! locks and whatever locks is the fast fits' work.

use rubylight_protocol::frame_warp::{FrameWarpRequest, Tags};
use rubylight_protocol::frame_warp_v2::{GyroAxes, YawSource};

use super::*;
use crate::frame_warp::Measured;
use crate::frame_warp::calibrate::Prior;
use crate::frame_warp::gyro::GYRO_KIND;
use crate::frame_warp::rls::tests::{
    FIRST_CAPTURE_US, FRAME_US, gyro_turns, mouse_moves, mouse_turns, rendered, stick_flicks,
    stick_states,
};

const PICTURE: (u16, u16) = (1920, 1080);
/// Picture pixels per thumbnail pixel at that size (256 x 144 thumbnails).
const SCALE: f64 = 7.5;
/// An older Nightjar's request (version 1, eight bytes): on, no pad, the lens, no mask.
const VERSION_1: [u8; 8] = [1, 1, 0xFF, 1, 0, 0, 0, 0];

#[derive(Clone, Copy, Debug)]
enum Input {
    Mouse(i16, i16),
    Stick((i16, i16)),
    Gyro([f32; 3]),
}

fn request(gyro: Option<YawSource>, max_offset_milli: u16) -> RequestV2 {
    RequestV2 {
        gyro: gyro.is_some(),
        axes: GyroAxes {
            yaw: gyro.unwrap_or_default(),
            ..GyroAxes::default()
        },
        max_offset_milli,
        ..RequestV2::new(FrameWarpRequest {
            enabled: true,
            controller: Some(0),
            mask_max_width: 0,
            lens: true,
        })
    }
}

fn mouse_inputs(end: u64) -> Vec<(u64, Input)> {
    mouse_moves(end)
        .into_iter()
        .map(|(t, dx, dy)| (t, Input::Mouse(dx, dy)))
        .collect()
}

fn gyro_inputs(end: u64, upright: bool) -> Vec<(u64, Input)> {
    let mut out = Vec::new();
    gyro_turns(end, upright, &mut |_, _, xyz, t| {
        out.push((t, Input::Gyro(xyz)))
    });
    out
}

/// The content's offset of a game turned by the gyro readings of [`gyro_inputs`].
fn gyro_offset(end: u64, upright: bool, gain: f64, lag: u64) -> impl Fn(u64) -> [f64; 2] {
    let axes = GyroAxes {
        yaw: YawSource::Auto,
        ..GyroAxes::default()
    };
    let mut gyro = GyroHistory::new(Some(0), axes);
    gyro_turns(end, upright, &mut |n, k, xyz, t| {
        gyro.on_motion(n, k, xyz, t);
    });
    move |t| {
        let turned = gyro.angle(0, t.saturating_sub(lag), upright);
        [gain * turned[0], gain * turned[1]]
    }
}

/// The synthetic game streamed through the host, captured every frame and each capture's
/// shift measured against the one before.
struct Stream {
    host: FrameWarpHost,
    /// The player's input, by time.
    inputs: Vec<(u64, Input)>,
    next: usize,
    /// The last capture's time and content offset.
    last: Option<(u64, [f64; 2])>,
    frame: u32,
    models: Vec<(u64, Model)>,
    metas: Vec<(u64, FrameWarpV2Meta)>,
    tags: Option<Tags>,
}

impl Stream {
    fn new(request: RequestV2, inputs: Vec<(u64, Input)>) -> Self {
        let mut host = FrameWarpHost::new(PICTURE);
        assert_eq!(host.on_request(&request.encode()), Some(true));
        assert_eq!(host.thumbnail, (256, 144));
        Self {
            host,
            inputs,
            next: 0,
            last: None,
            frame: 0,
            models: Vec::new(),
            metas: Vec::new(),
            tags: None,
        }
    }

    /// Captures until `end` of a game whose content offset at its frame `t` is `offset(t)`;
    /// `scene(t)` is how the capture at `t` compares with the one before.
    fn run(
        &mut self,
        end: u64,
        offset: &impl Fn(u64) -> [f64; 2],
        scene: impl Fn(u64) -> Option<Scene>,
    ) {
        let mut t = self.next_capture();
        while t < end {
            self.capture(t, offset(rendered(t)), scene(t));
            t += FRAME_US;
        }
    }

    fn next_capture(&self) -> u64 {
        self.last.map_or(FIRST_CAPTURE_US, |(t, _)| t + FRAME_US)
    }

    /// One capture at `t`: the input before it, the measured shift, the model the control loop
    /// takes, the frame's tags and its meta.
    fn capture(&mut self, t: u64, offset: [f64; 2], scene: Option<Scene>) {
        while let Some(&(at, input)) = self.inputs.get(self.next).filter(|(at, _)| *at <= t) {
            match input {
                Input::Mouse(dx, dy) => self.host.on_mouse(dx, dy, at),
                Input::Stick(stick) => self.host.on_controller(0, stick, at),
                Input::Gyro(xyz) => self.host.on_motion(0, GYRO_KIND, xyz, at),
            }
            self.next += 1;
        }
        if let Some((t0, before)) = self.last {
            let moved = [offset[0] - before[0], offset[1] - before[1]];
            let measured = Measured {
                size: self.host.thumbnail,
                at_us: t,
                shift: Some((t0, Motion::uniform(moved[0] as f32, moved[1] as f32, 0.9))),
                resets: self.host.resets.thumbnail,
                scene,
            };
            assert!(self.host.on_measured(&measured, t));
        }
        self.last = Some((t, offset));
        if let Some(wire) = self.host.take_model() {
            self.models
                .push((t, Model::decode(&wire).expect("a valid model")));
        }
        self.frame += 1;
        self.host.tag(self.frame, t);
        while let Some(wire) = self.host.take_tags() {
            self.tags = Some(Tags::decode(&wire).expect("valid tags"));
        }
        while let Some(wire) = self.host.take_meta() {
            let meta = FrameWarpV2Meta::decode(&wire).expect("a valid meta");
            assert_eq!(meta.frame, self.frame);
            self.metas.push((t, meta));
        }
    }

    fn model(&self) -> Model {
        self.models.last().expect("a model").1
    }

    fn meta(&self) -> FrameWarpV2Meta {
        self.metas.last().expect("a meta").1
    }

    /// The first model that `locked` says is locked, and when it came.
    fn locked(&self, locked: impl Fn(&Model) -> bool) -> Option<(u64, Model)> {
        self.models.iter().copied().find(|(_, m)| locked(m))
    }

    /// When the first meta from `from` on that `wanted` takes came.
    fn meta_from(&self, from: u64, wanted: impl Fn(&FrameWarpV2Meta) -> bool) -> Option<u64> {
        self.metas
            .iter()
            .find(|(t, m)| *t >= from && wanted(m))
            .map(|(t, _)| *t)
    }
}

fn mouse_stream(end: u64, gain: f64, lag: u64) -> (Stream, impl Fn(u64) -> [f64; 2]) {
    let history = mouse_turns(end);
    let offset = move |t: u64| {
        let (x, y) = history.mouse_at(t.saturating_sub(lag));
        [gain * x as f64, gain * y as f64]
    };
    (Stream::new(request(None, 0), mouse_inputs(end)), offset)
}

fn near(value: f32, truth: f64, share: f64) -> bool {
    (f64::from(value) / truth - 1.0).abs() < share
}

#[test]
fn an_older_clients_request_is_ignored() {
    let mut host = FrameWarpHost::new(PICTURE);
    assert_eq!(host.on_request(&VERSION_1), None);
    // Nor padded to twelve bytes: its version byte says 1.
    let mut padded = [0u8; 12];
    padded[..8].copy_from_slice(&VERSION_1);
    assert_eq!(host.on_request(&padded), None);
    assert!(!host.enabled() && host.v2().is_none());
    host.on_mouse(5, 0, 1_000);
    host.on_motion(0, GYRO_KIND, [0.0, 90.0, 0.0], 2_000);
    host.tag(1, 20_000);
    assert_eq!(host.take_model(), None);
    assert_eq!(host.take_tags(), None);
    assert_eq!(host.take_meta(), None);
    assert_eq!(host.take_mask(), None);
    // One in the middle of a version 2 client's session changes nothing.
    let mut stream = Stream::new(request(None, 0), Vec::new());
    stream.run(100_000, &|_| [0.0; 2], |_| None);
    assert_eq!(stream.host.on_request(&VERSION_1), None);
    assert!(stream.host.enabled());
    let (models, metas) = (stream.models.len(), stream.metas.len());
    stream.run(200_000, &|_| [0.0; 2], |_| None);
    assert_eq!(stream.models.len(), models);
    assert!(stream.metas.len() > metas);
}

#[test]
fn a_cold_mouse_lock_reaches_the_client_within_a_second() {
    let (gain, lag) = (-0.12, 50_000u64);
    let (mut stream, offset) = mouse_stream(2_000_000, gain, lag);
    stream.run(1_500_000, &offset, |_| None);
    // The request's model first, unlocked.
    assert!(!stream.models[0].1.locked());
    let (at, model) = stream.locked(|m| m.mouse_locked).expect("a locked model");
    assert!(at < 1_000_000, "locked at {at} us");
    assert!(near(model.mouse_gain[0], gain * SCALE, 0.05), "{model:?}");
    assert!(!model.stick_locked);
    assert!((40_000..=72_000).contains(&model.lag_us), "{model:?}");
    assert!(!stream.host.calibrator.locked());
    // The tags and every frame's meta carry the generation of the model the client has.
    let generation = stream.model().generation;
    assert_eq!(stream.tags.as_ref().unwrap().model_generation, generation);
    assert_eq!(stream.metas.len(), stream.frame as usize);
    let meta = stream.meta();
    assert_eq!(meta.model_generation, generation);
    assert!(meta.confidence() > 0.9, "{meta:?}");
    assert!(!meta.has(meta_flags::GYRO_LOCKED) && !meta.has(meta_flags::PRIOR_WARM_LOCKED));
    assert_eq!(
        (meta.edge_fade_strength, meta.hud_guard),
        (DEFAULT_EDGE_FADE, DEFAULT_HUD_GUARD)
    );
    // And the statistics and the prior show it.
    assert_eq!(stream.host.status().model, Some(stream.model()));
    let prior = stream.host.prior_v2().expect("a prior");
    assert!(near(prior.base.mouse_gain.unwrap()[0], gain * SCALE, 0.05));
    assert_eq!(prior.gyro_gain, None);
}

#[test]
fn a_cold_stick_lock_reaches_the_client_within_a_second() {
    let (gain, lag, curve) = (-60.0, 34_000u64, (0.20f32, 1.5f32));
    let history = stick_flicks(2_000_000);
    let offset = |t: u64| {
        let turned = history.stick_integral(0, t.saturating_sub(lag), curve.0, curve.1);
        [gain * turned[0], gain * turned[1]]
    };
    let inputs = stick_states(2_000_000)
        .into_iter()
        .map(|(t, stick)| (t, Input::Stick(stick)))
        .collect();
    let mut stream = Stream::new(request(None, 0), inputs);
    stream.run(1_500_000, &offset, |_| None);
    let (at, model) = stream.locked(|m| m.stick_locked).expect("a locked model");
    assert!(at < 1_000_000, "locked at {at} us");
    assert!(near(model.stick_gain[0], gain * SCALE, 0.1), "{model:?}");
    assert!(!model.mouse_locked);
    // Neighbouring curves fit almost alike at first; the flicks tell them apart.
    let model = stream.model();
    assert_eq!(
        (model.stick_deadzone, model.stick_exponent),
        curve,
        "{model:?}"
    );
    assert!(near(model.stick_gain[0], gain * SCALE, 0.1), "{model:?}");
}

#[test]
fn a_cold_gyro_lock_reaches_the_client_within_a_second() {
    let (gain, lag) = (-110.0, 45_000u64);
    let offset = gyro_offset(2_000_000, false, gain, lag);
    let mut stream = Stream::new(
        request(Some(YawSource::Auto), 0),
        gyro_inputs(2_000_000, false),
    );
    stream.run(1_500_000, &offset, |_| None);
    let at = stream
        .meta_from(0, |m| m.has(meta_flags::GYRO_LOCKED))
        .expect("a gyro lock");
    assert!(at < 1_000_000, "locked at {at} us");
    let meta = stream.meta();
    assert!(near(meta.gyro_gain[0], gain * SCALE, 0.05), "{meta:?}");
    // A pad held level turns by its y axis; the readings are counted as the client counts.
    assert!(!meta.has(meta_flags::GYRO_YAW_Z));
    assert!(meta.gyro_seq > 300 && meta.gyro_held_us < 4_000, "{meta:?}");
    assert!(meta.confidence() > 0.9, "{meta:?}");
    assert!(meta.omega_mrad_s != [0, 0]);
    // The model has no gyro: the mouse and stick stay unlocked.
    assert!(stream.models.iter().all(|(_, m)| !m.locked()));
}

#[test]
fn an_upright_gyro_is_found_on_z_and_its_prior_locks_the_next_stream_at_once() {
    let (gain, lag) = (-110.0, 45_000u64);
    let offset = gyro_offset(3_000_000, true, gain, lag);
    let mut stream = Stream::new(
        request(Some(YawSource::Auto), 0),
        gyro_inputs(3_000_000, true),
    );
    stream.run(1_500_000, &offset, |_| None);
    let at = stream
        .meta_from(0, |m| {
            m.has(meta_flags::GYRO_LOCKED) && m.has(meta_flags::GYRO_YAW_Z)
        })
        .expect("a gyro lock on z");
    assert!(at < 1_000_000, "locked at {at} us");
    assert!(near(stream.meta().gyro_gain[0], gain * SCALE, 0.05));
    let prior = stream.host.prior_v2().expect("a prior");
    assert_eq!(prior.gyro_yaw_z, Some(true));
    assert!(
        near(prior.gyro_gain.unwrap()[0], gain * SCALE, 0.05),
        "{prior:?}"
    );
    // The next stream of the game starts from it.
    let mut next = Stream::new(
        request(Some(YawSource::Auto), 0),
        gyro_inputs(3_000_000, true),
    );
    next.host.set_prior_v2(prior);
    next.run(600_000, &offset, |_| None);
    assert!(
        next.metas
            .iter()
            .all(|(_, m)| m.has(meta_flags::GYRO_YAW_Z))
    );
    let at = next
        .meta_from(0, |m| {
            m.has(meta_flags::GYRO_LOCKED) && m.has(meta_flags::PRIOR_WARM_LOCKED)
        })
        .expect("a warm lock");
    assert!(at < 250_000, "locked at {at} us");
}

#[test]
fn a_warm_prior_locks_within_a_quarter_second() {
    let (gain, lag) = (-0.12, 40_000u64);
    let (mut stream, offset) = mouse_stream(1_000_000, gain, lag);
    // As the stream's worker hands it over: after the request.
    stream.host.set_prior_v2(PriorV2 {
        base: Prior {
            mouse_gain: Some([(gain * SCALE) as f32; 2]),
            mouse_lag_us: Some(48_000),
            picture_width: PICTURE.0,
            picture_height: PICTURE.1,
            ..Prior::default()
        },
        ..PriorV2::default()
    });
    stream.run(600_000, &offset, |_| None);
    let (at, model) = stream.locked(|m| m.mouse_locked).expect("a locked model");
    assert!(at < 250_000, "locked at {at} us");
    assert_eq!(model.lag_us, 48_000);
    assert!(
        stream
            .meta_from(at, |m| m.has(meta_flags::PRIOR_WARM_LOCKED))
            .is_some_and(|t| t == at)
    );
}

#[test]
fn a_cut_or_the_pointer_drops_the_confidence_within_80_ms() {
    let (gain, lag) = (-0.12, 40_000u64);
    let (mut stream, offset) = mouse_stream(3_000_000, gain, lag);
    stream.run(1_200_000, &offset, |_| None);
    assert!(stream.meta().confidence() > 0.9);
    let cut_at = stream.next_capture();
    let cut = Scene {
        histogram: 0.1,
        correlation: 0.0,
    };
    stream.run(1_600_000, &offset, |t| (t == cut_at).then_some(cut));
    let dropped = stream
        .meta_from(cut_at, |m| m.confidence() < 0.1)
        .expect("a drop");
    assert!(
        dropped - cut_at <= 80_000,
        "{dropped} after a cut at {cut_at}"
    );
    assert!(
        stream
            .meta_from(cut_at, |m| m.has(meta_flags::SCENE_CUT))
            .is_some_and(|t| t == dropped)
    );
    // Back once the cut's hold ends.
    assert!(stream.meta().confidence() > 0.9, "{:?}", stream.meta());
    let shown_at = stream.next_capture();
    stream.host.on_cursor(true);
    stream.run(shown_at + 100_000, &offset, |_| None);
    let dropped = stream
        .meta_from(shown_at, |m| m.confidence() < 0.1)
        .expect("a drop");
    assert!(
        dropped - shown_at <= 80_000,
        "{dropped} after the pointer at {shown_at}"
    );
    assert!(stream.meta().has(meta_flags::CURSOR_VISIBLE_PAUSE));
    stream.host.on_cursor(false);
    stream.run(shown_at + 500_000, &offset, |_| None);
    assert!(stream.meta().confidence() > 0.9, "{:?}", stream.meta());
    assert!(!stream.meta().has(meta_flags::CURSOR_VISIBLE_PAUSE));
}

#[test]
fn the_request_limits_the_shift_and_a_stop_ends_the_metas() {
    let limit = |milli| {
        let mut stream = Stream::new(request(None, milli), Vec::new());
        stream.run(50_000, &|_| [0.0; 2], |_| None);
        stream.model().max_offset
    };
    let close = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 0.5 && (a[1] - b[1]).abs() < 0.5;
    assert!(close(limit(100), [192.0, 108.0]), "{:?}", limit(100));
    // None asked: the host's 15%.
    assert!(close(limit(0), [288.0, 162.0]), "{:?}", limit(0));
    let mut stream = Stream::new(request(None, 0), Vec::new());
    stream.run(50_000, &|_| [0.0; 2], |_| None);
    let stop = RequestV2::new(FrameWarpRequest {
        enabled: false,
        controller: None,
        mask_max_width: 0,
        lens: false,
    });
    assert_eq!(stream.host.on_request(&stop.encode()), Some(false));
    assert!(stream.host.v2().is_none());
    stream.host.tag(99, 60_000);
    assert_eq!(stream.host.take_meta(), None);
    assert_eq!(stream.host.take_model(), None);
}

#[test]
fn a_version_1_prior_reads_and_a_version_2_one_round_trips() {
    let saved = r#"{"mouse_gain":[0.9,0.9],"mouse_lag_us":40000,"stick_gain":null,
        "stick_lag_us":null,"stick_deadzone":null,"stick_exponent":null,"focal":960.0,
        "picture_width":1920,"picture_height":1080}"#;
    let prior: PriorV2 = serde_json::from_str(saved).unwrap();
    assert_eq!(prior.base.mouse_gain, Some([0.9, 0.9]));
    assert_eq!(prior.base.focal, Some(960.0));
    assert_eq!(
        (prior.gyro_gain, prior.gyro_yaw_z, prior.confident_at),
        (None, None, None)
    );
    let full = PriorV2 {
        gyro_gain: Some([-825.0, -800.0]),
        gyro_lag_us: Some(45_000),
        gyro_yaw_z: Some(true),
        confident_at: Some(1_700_000_000),
        ..prior
    };
    let json = serde_json::to_string(&full).unwrap();
    assert_eq!(serde_json::from_str::<PriorV2>(&json).unwrap(), full);
    // Flat: a reader of the calibrator's prior alone reads the same entry.
    assert_eq!(serde_json::from_str::<Prior>(&json).unwrap(), full.base);
    // A gain out of bounds is not started from.
    let mut host = FrameWarpHost::new(PICTURE);
    host.set_prior_v2(PriorV2 {
        gyro_gain: Some([f32::INFINITY, 0.0]),
        ..full
    });
    assert_eq!(host.prior_v2().unwrap().gyro_gain, None);
}
