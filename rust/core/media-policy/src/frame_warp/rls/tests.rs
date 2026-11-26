//! A deterministic game for the fast fits: mouse, stick and gyro turns that start at time 0, a
//! game that renders at 60 fps with a known gain and lag, and captures at 60 fps summed into
//! chunks of two, as the host sums them.

use super::*;
use crate::frame_warp::gyro::{GYRO_KIND, GyroHistory};
use crate::frame_warp::history::InputHistory;
use rubylight_protocol::frame_warp_v2::{GyroAxes, YawSource};

/// The game's frame time and the capture's.
pub(crate) const FRAME_US: u64 = 16_667;
/// The first capture, mid-frame.
pub(crate) const FIRST_CAPTURE_US: u64 = 8_333;

/// The game's frame a capture at `t` shows.
pub(crate) fn rendered(t: u64) -> u64 {
    t / FRAME_US * FRAME_US
}

/// Relative mouse moves every 2 ms from time 0: turns right and left with the speed changing,
/// so the lag shows.
pub(crate) fn mouse_moves(end: u64) -> Vec<(u64, i16, i16)> {
    let mut out = Vec::new();
    let mut t = 0;
    while t < end {
        let phase = t as f64 / 1e6;
        let speed = 9.0 * (std::f64::consts::TAU * 1.2 * phase + 0.4).sin() + 1.5;
        let dy = 2.0 * (std::f64::consts::TAU * 0.7 * phase).cos();
        out.push((t, speed.round() as i16, dy.round() as i16));
        t += 2_000;
    }
    out
}

pub(crate) fn mouse_turns(end: u64) -> InputHistory {
    let mut history = InputHistory::new(Some(0));
    for (t, dx, dy) in mouse_moves(end) {
        history.on_mouse(dx, dy, t);
    }
    history
}

/// Right-stick flicks every 8 ms from time 0 (pad 0): levels held for 120 to 280 ms.
pub(crate) fn stick_states(end: u64) -> Vec<(u64, (i16, i16))> {
    const LEVELS: [f64; 6] = [-1.0, -0.6, -0.35, 0.4, 0.7, 1.0];
    let mut out = Vec::new();
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
        out.push((t, ((level * 32767.0) as i16, 0)));
        t += 8_000;
    }
    out
}

pub(crate) fn stick_flicks(end: u64) -> InputHistory {
    let mut history = InputHistory::new(Some(0));
    for (t, stick) in stick_states(end) {
        history.on_controller(0, stick, t);
    }
    history
}

/// Gyro readings every 4 ms from time 0 for pad 0: yaw on the packets' y axis (or z when
/// `upright`) turning left and right, some pitch.
pub(crate) fn gyro_turns(end: u64, upright: bool, motion: &mut impl FnMut(u8, u8, [f32; 3], u64)) {
    let mut t = 0;
    while t < end {
        let phase = t as f64 / 1e6;
        let yaw = 140.0 * (std::f64::consts::TAU * 1.1 * phase + 0.5).sin() + 20.0;
        let pitch = 25.0 * (std::f64::consts::TAU * 0.6 * phase).cos();
        let xyz = if upright {
            [pitch as f32, 0.0, yaw as f32]
        } else {
            [pitch as f32, yaw as f32, 0.0]
        };
        motion(0, GYRO_KIND, xyz, t);
        t += 4_000;
    }
}

/// The content's moves over chunks of two 60 fps captures, for a camera whose content offset
/// (thumbnail pixels) at the game's frame `t` is `offset(t)`: `(t0, t1, move)`.
pub(crate) fn chunks(end: u64, offset: impl Fn(u64) -> [f64; 2]) -> Vec<(u64, u64, [f64; 2])> {
    let mut out = Vec::new();
    let mut t0 = FIRST_CAPTURE_US;
    while t0 + 2 * FRAME_US < end {
        let t1 = t0 + 2 * FRAME_US;
        let (a, b) = (offset(rendered(t0)), offset(rendered(t1)));
        out.push((t0, t1, [b[0] - a[0], b[1] - a[1]]));
        t0 = t1;
    }
    out
}

/// Feeds `chunks` to `rls` with `input`, and returns when it locked (the chunk's end).
fn lock_time(
    rls: &mut Rls,
    chunks: &[(u64, u64, [f64; 2])],
    input: impl Fn((f32, f32), u64, u64, u64) -> Option<[f64; 2]>,
) -> Option<u64> {
    let mut locked = None;
    for &(t0, t1, m) in chunks {
        rls.update(m, |curve, lag| input(curve, t0, t1, lag));
        if locked.is_none() && rls.locked() {
            locked = Some(t1);
        }
    }
    locked
}

fn mouse_input(history: &InputHistory) -> impl Fn((f32, f32), u64, u64, u64) -> Option<[f64; 2]> {
    move |_, t0, t1, lag| {
        let (x, y) = history.mouse_between(t0.saturating_sub(lag), t1.saturating_sub(lag));
        Some([x as f64, y as f64])
    }
}

#[test]
fn a_cold_mouse_fit_locks_within_a_second() {
    let (gain, lag) = (-0.12, 50_000u64);
    let history = mouse_turns(3_000_000);
    let moves = chunks(3_000_000, |t| {
        let (x, y) = history.mouse_at(t.saturating_sub(lag));
        [gain * x as f64, gain * y as f64]
    });
    let mut rls = Rls::new(Kind::Mouse);
    let locked = lock_time(&mut rls, &moves, mouse_input(&history)).expect("a lock");
    assert!(locked < 1_000_000, "locked at {locked} us");
    let fit = rls.locked_fit().unwrap();
    assert!((fit.gain[0] / gain - 1.0).abs() < 0.05, "{fit:?}");
    assert!((fit.gain[1] / gain - 1.0).abs() < 0.1, "{fit:?}");
    // The render quantisation adds up to a frame.
    assert!((40_000..=72_000).contains(&fit.lag_us), "{fit:?}");
    assert!(fit.fit >= LOCK_FIT);
}

#[test]
fn a_warm_prior_locks_on_the_first_turn() {
    let (gain, lag) = (-0.12, 40_000u64);
    let history = mouse_turns(1_000_000);
    let moves = chunks(1_000_000, |t| {
        let (x, y) = history.mouse_at(t.saturating_sub(lag));
        [gain * x as f64, gain * y as f64]
    });
    let mut rls = Rls::warm(Kind::Mouse, [gain; 2], 48_000, None);
    assert!(rls.is_warm());
    let locked = lock_time(&mut rls, &moves, mouse_input(&history)).expect("a lock");
    assert!(locked < 250_000, "locked at {locked} us");
    assert!(rls.warm_locked());
    assert_eq!(rls.locked_fit().unwrap().lag_us, 48_000);
}

#[test]
fn a_prior_the_game_disagrees_with_is_dropped() {
    // The prior says twice the sensitivity: the fit runs on cold and finds the real one.
    let (gain, lag) = (-0.12, 40_000u64);
    let history = mouse_turns(3_000_000);
    let moves = chunks(3_000_000, |t| {
        let (x, y) = history.mouse_at(t.saturating_sub(lag));
        [gain * x as f64, gain * y as f64]
    });
    let mut rls = Rls::warm(Kind::Mouse, [-gain * 2.0; 2], 40_000, None);
    let locked = lock_time(&mut rls, &moves, mouse_input(&history)).expect("a lock");
    assert!(!rls.is_warm() && !rls.warm_locked());
    assert!(locked < 1_500_000, "locked at {locked} us");
    let fit = rls.locked_fit().unwrap();
    assert!((fit.gain[0] / gain - 1.0).abs() < 0.05, "{fit:?}");
}

#[test]
fn a_cold_stick_fit_finds_the_curve_within_a_second() {
    let (gain, lag, curve) = (-60.0, 34_000u64, (0.20f32, 1.5f32));
    let history = stick_flicks(3_000_000);
    let moves = chunks(3_000_000, |t| {
        let turned = history.stick_integral(0, t.saturating_sub(lag), curve.0, curve.1);
        [gain * turned[0], gain * turned[1]]
    });
    let mut rls = Rls::new(Kind::Stick);
    let locked = lock_time(&mut rls, &moves, |(deadzone, exponent), t0, t1, lag| {
        Some(history.stick_integral(
            t0.saturating_sub(lag),
            t1.saturating_sub(lag),
            deadzone,
            exponent,
        ))
    })
    .expect("a lock");
    assert!(locked < 1_000_000, "locked at {locked} us");
    let fit = rls.locked_fit().unwrap();
    assert!((fit.gain[0] / gain - 1.0).abs() < 0.1, "{fit:?}");
    assert_eq!((fit.deadzone, fit.exponent), curve, "{fit:?}");
    assert!(fit.lag_us.abs_diff(lag + 8_000) <= 12_000, "{fit:?}");
}

#[test]
fn a_cold_gyro_fit_locks_within_a_second() {
    let (gain, lag) = (-110.0, 45_000u64);
    let mut gyro = GyroHistory::new(
        Some(0),
        GyroAxes {
            yaw: YawSource::Auto,
            ..GyroAxes::default()
        },
    );
    gyro_turns(3_000_000, false, &mut |n, k, xyz, t| {
        gyro.on_motion(n, k, xyz, t);
    });
    let moves = chunks(3_000_000, |t| {
        let turned = gyro.angle(0, t.saturating_sub(lag), false);
        [gain * turned[0], gain * turned[1]]
    });
    let mut rls = Rls::new(Kind::Gyro);
    let locked = lock_time(&mut rls, &moves, |_, t0, t1, lag| {
        Some(gyro.angle(t0.saturating_sub(lag), t1.saturating_sub(lag), false))
    })
    .expect("a lock");
    assert!(locked < 1_000_000, "locked at {locked} us");
    let fit = rls.locked_fit().unwrap();
    assert!((fit.gain[0] / gain - 1.0).abs() < 0.05, "{fit:?}");
}

#[test]
fn a_picture_that_stops_following_unlocks_and_forgets_fast() {
    let (gain, lag) = (-0.12, 40_000u64);
    let history = mouse_turns(4_000_000);
    let moves = chunks(4_000_000, |t| {
        // A menu opens at 2 s: the mouse goes on, the picture stands still.
        let (x, y) = history.mouse_at(t.saturating_sub(lag).min(2_000_000));
        [gain * x as f64, gain * y as f64]
    });
    let mut rls = Rls::new(Kind::Mouse);
    let input = mouse_input(&history);
    let mut unlocked_at = None;
    let mut lowest = LAMBDA_MAX;
    for &(t0, t1, m) in &moves {
        rls.update(m, |curve, lag| input(curve, t0, t1, lag));
        if t1 > 2_100_000 {
            lowest = lowest.min(rls.lambda());
            if unlocked_at.is_none() && !rls.locked() {
                unlocked_at = Some(t1);
            }
        } else if t1 > 1_000_000 {
            assert!(rls.locked(), "locked by 1 s");
        }
    }
    let unlocked = unlocked_at.expect("an unlock");
    assert!(unlocked < 2_500_000, "unlocked at {unlocked}");
    assert!(lowest < 0.96, "{lowest}");
}

#[test]
fn input_that_moves_nothing_never_locks() {
    let history = mouse_turns(3_000_000);
    let moves = chunks(3_000_000, |t| [((t / FRAME_US) % 3) as f64 * 0.3, 0.0]);
    let mut rls = Rls::new(Kind::Mouse);
    assert_eq!(lock_time(&mut rls, &moves, mouse_input(&history)), None);
}

#[test]
fn chunks_another_input_turned_teach_nothing() {
    let history = mouse_turns(2_000_000);
    let moves = chunks(2_000_000, |t| [t as f64 * 1e-4, 0.0]);
    let mut rls = Rls::new(Kind::Gyro);
    assert_eq!(lock_time(&mut rls, &moves, |_, _, _, _| None), None);
    assert_eq!(rls.fit(), None);
    let _ = history;
}
