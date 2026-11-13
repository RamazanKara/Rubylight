use super::*;

/// A game at 60 fps captured at 120 fps: frame `i` shows the camera as of the input that
/// arrived `lag_us` before it was rendered, `gain` thumbnail pixels per count.
fn mouse_session(gain: f64, lag_us: u64, noise: f32) -> (InputHistory, Calibrator, u64) {
    mouse_session_with(
        gain,
        lag_us,
        noise,
        |phase, _| (phase * 1.1).sin() * 4.0,
        0.0,
    )
}

/// [`mouse_session`] with the mouse's dy at a time and turn speed given by `dy_at`, and the
/// picture also rolling up and down by `roll` thumbnail pixels, whatever the input.
fn mouse_session_with(
    gain: f64,
    lag_us: u64,
    noise: f32,
    dy_at: fn(f64, f64) -> f64,
    roll: f64,
) -> (InputHistory, Calibrator, u64) {
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
        let dy = dy_at(phase, speed) as i16;
        history.on_mouse(speed as i16, dy, t);
        t += 2_000;
    }
    let camera = |t: u64, h: &InputHistory| {
        let (x, y) = h.mouse_at(t.saturating_sub(lag_us));
        let rolled = roll * (t as f64 / 1e6 * 1.7).sin();
        (x as f64 * gain, y as f64 * gain - rolled)
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
                ..Motion::default()
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
fn a_sliver_of_vertical_input_takes_the_horizontal_gain() {
    // Turns that drift down a count for every 12 across, while the picture rolls by 3
    // thumbnail pixels: the vertical gain fitted alone would mostly fit the roll.
    let (history, mut cal, end) =
        mouse_session_with(0.12, 50_000, 0.3, |_, speed| (speed / 12.0).round(), 3.0);
    let model = cal.update(&history, end).expect("a locked model");
    assert!(model.mouse_locked, "{model:?}");
    assert!((model.mouse_gain[0] + 0.9).abs() < 0.05, "{model:?}");
    assert!(
        (model.mouse_gain[1] - model.mouse_gain[0]).abs() < 1e-4,
        "{model:?}"
    );
}

#[test]
fn a_locked_model_stays_until_a_fit_explains_clearly_more() {
    let (history, mut cal, end) = mouse_session(0.12, 50_000, 0.3);
    cal.update(&history, end).expect("a locked model");
    let learned = cal.mouse.expect("a mouse fit");
    // A model a couple of milliseconds off explains the motion almost as well as the best
    // fit, so its lag stays rather than wandering with every fit.
    let near = Fit {
        lag_us: learned.lag_us + 4_000,
        ..learned
    };
    cal.mouse = Some(near);
    cal.update(&history, end + EVERY_US);
    assert!(cal.mouse_locked);
    let kept = cal.mouse.expect("a mouse fit");
    assert_eq!(kept.lag_us, near.lag_us);
    assert!(
        (kept.gain[0] / learned.gain[0] - 1.0).abs() < 0.02,
        "{kept:?}"
    );
    // A gain 4% off at the right lag explains almost as much too: the lag stays and the
    // gain settles on the least-squares one there.
    cal.mouse = Some(Fit {
        gain: [learned.gain[0] * 0.96, learned.gain[1] * 0.96],
        ..learned
    });
    cal.update(&history, end + 2 * EVERY_US);
    let settled = cal.mouse.expect("a mouse fit");
    assert_eq!(settled.lag_us, learned.lag_us);
    assert!(
        (settled.gain[0] / learned.gain[0] - 1.0).abs() < 0.005,
        "{settled:?} vs {learned:?}"
    );
    // One with half the gain explains clearly less: the fresh fit replaces it.
    cal.mouse = Some(Fit {
        gain: [near.gain[0] * 0.5, near.gain[1] * 0.5],
        ..near
    });
    cal.update(&history, end + 3 * EVERY_US);
    let refit = cal.mouse.expect("a mouse fit");
    assert!(cal.mouse_locked);
    assert!(
        (refit.gain[0] - learned.gain[0]).abs() < 0.01,
        "{refit:?} vs {learned:?}"
    );
}

#[test]
fn a_locked_lag_gives_way_to_a_fit_that_explains_a_quarter_of_what_it_leaves() {
    let locked = Fit {
        gain: [1.0; 2],
        lag_us: 50_000,
        fit: 0.0,
        deadzone: 0.0,
        exponent: 1.0,
    };
    // The locked model explains `share` of the motion; the least-squares gain at its lag no
    // more.
    let at = |share: f64| {
        move |model: &Fit, gain: Option<[f64; 2]>| {
            Some(Fit {
                fit: share,
                gain: gain.unwrap_or(model.gain),
                ..*model
            })
        }
    };
    let fresh = |fit: f64| {
        Some(Fit {
            fit,
            lag_us: 54_000,
            ..locked
        })
    };
    let replaced = |share: f64, fit: f64| {
        let (mut model, mut found) = (locked, fresh(fit));
        keep_unless_better(&mut model, &mut found, at(share));
        found.is_some()
    };
    // At 99% a quarter of the rest is a quarter point.
    assert!(!replaced(0.990, 0.992));
    assert!(replaced(0.990, 0.9935));
    // At 80% the 3 points apply.
    assert!(!replaced(0.80, 0.82));
    assert!(replaced(0.80, 0.84));
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
                ..Motion::default()
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
                ..Motion::default()
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
fn a_lens_goes_into_the_model_and_the_prior_and_limits_the_shift() {
    let (history, mut cal, end) = mouse_session(0.12, 50_000, 0.3);
    // 110 degrees across the picture's 1920 pixels.
    let half = 55f64.to_radians().tan();
    let focal = 960.0 / half;
    cal.set_lens(Some(half * half));
    let model = cal.update(&history, end).expect("a locked model");
    assert!((f64::from(model.focal) - focal).abs() < 0.01, "{model:?}");
    // So wide a lens stretches the picture's left and right before the flat limit does.
    let limit = (MAX_LENS_OFFSET * focal * focal / 1920.0) as f32;
    assert!(
        limit < 288.0 && (model.max_offset[0] - limit).abs() < 0.01,
        "{model:?}"
    );
    assert_eq!(model.max_offset[1], 162.0);
    // A client that slides the picture is sent neither the lens nor its limit; the lens
    // stays in force for the gains.
    cal.set_lens_drawn(false);
    let slid = cal.model(1);
    assert_eq!((slid.focal, slid.max_offset), (0.0, [288.0, 162.0]));
    assert_eq!(slid.mouse_gain, model.mouse_gain);
    assert!(cal.lens().is_some());
    cal.set_lens_drawn(true);
    // The next session, at another size, starts with the same field of view.
    let prior = cal.prior().unwrap();
    assert!(
        (f64::from(prior.focal.unwrap()) - focal).abs() < 0.01,
        "{prior:?}"
    );
    let mut next = Calibrator::new((1280, 720), (256, 144));
    next.set_prior(prior);
    let scaled = focal * 1280.0 / 1920.0;
    assert!((f64::from(next.model(1).focal) - scaled).abs() < 0.01);
    // What an earlier version saved has no lens.
    let old: Prior = serde_json::from_str(
        r#"{"mouse_gain":[1.0,1.0],"mouse_lag_us":30000,"stick_gain":null,"stick_lag_us":null,
                "stick_deadzone":null,"stick_exponent":null,"picture_width":1920,"picture_height":1080}"#,
    )
    .unwrap();
    assert_eq!(old.focal, None);
    let mut flat = Calibrator::new((1920, 1080), (256, 144));
    flat.set_prior(old);
    assert_eq!(flat.model(1).focal, 0.0);
    assert_eq!(flat.model(1).max_offset, [288.0, 162.0]);

    // A field of view that wobbles is not sent again; another one is, and so is its loss.
    cal.set_lens(Some(half * half * 1.02));
    assert_eq!(cal.update(&history, end + EVERY_US), None);
    cal.set_lens(Some(1.0));
    let turned = cal
        .update(&history, end + 2 * EVERY_US)
        .expect("a new lens");
    assert_eq!((turned.focal, turned.generation), (960.0, 2));
    assert_eq!(turned.max_offset, [288.0, 162.0]);
    cal.set_lens(None);
    let lost = cal.update(&history, end + 3 * EVERY_US).expect("no lens");
    assert_eq!((lost.focal, lost.generation), (0.0, 3));
    // Nonsense is no lens.
    cal.set_lens(Some(f64::NAN));
    assert_eq!(cal.lens(), None);
    cal.set_lens(Some(1e-20));
    assert_eq!(
        cal.model(4).focal,
        0.0,
        "a focal length past the protocol's"
    );
}

#[test]
fn a_letterboxed_capture_keeps_its_gains_in_its_own_pixels() {
    // A 16:9 capture with bars in a 16:10 stream: the thumbnails cover the capture alone.
    let mut cal = Calibrator::new((2880, 1800), (256, 160));
    cal.set_sizes((2880, 1800), (2880, 1620), (256, 144));
    cal.set_prior(Prior {
        mouse_gain: Some([2.0, 2.0]),
        mouse_lag_us: Some(20_000),
        picture_width: 1920,
        picture_height: 1080,
        ..Default::default()
    });
    let model = cal.model(1);
    assert_eq!((model.picture_width, model.picture_height), (2880, 1800));
    for gain in model.mouse_gain {
        assert!((gain - 3.0).abs() < 1e-4, "{model:?}");
    }
    assert!((model.max_offset[1] - 1800.0 * MAX_OFFSET).abs() < 1e-3);
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
                ..Motion::default()
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
