use super::*;

fn model(mouse: bool, stick: bool) -> Model {
    Model {
        generation: 1,
        mouse_locked: mouse,
        stick_locked: stick,
        picture_width: 1000,
        picture_height: 500,
        mouse_gain: [-0.5, -0.25],
        stick_gain: [-1000.0, 500.0],
        stick_deadzone: 0.0,
        stick_exponent: 1.0,
        max_offset: [200.0, 100.0],
        lag_us: 30_000,
        confidence: 0.9,
        focal: 0.0,
    }
}

fn tags(tags: &[Tag]) -> Tags {
    Tags {
        model_generation: 1,
        tags: tags.to_vec(),
    }
}

const SETTLED: u64 = 10_000_000;

#[test]
fn mouse_input_the_frame_lacks_shifts_it() {
    let mut p = Predictor::new(None);
    p.on_model(model(true, false));
    // The model fades in over 150 ms; settle it first.
    p.warp(0, SETTLED - FADE.as_micros() as u64 - 1);
    p.note_mouse(100, 40, SETTLED);
    p.on_tags(&tags(&[Tag { frame: 5, mouse_x: 60, mouse_y: 40, ..Tag::default() }]), SETTLED);
    let warp = p.warp(5, SETTLED + 1000);
    // 40 counts in x still in flight: -0.5 px per count.
    assert_eq!(warp.pixels, [-20.0, 0.0]);
    assert_eq!(warp.uv, [-0.02, 0.0]);
    assert!(warp.active);
}

#[test]
fn a_peek_shifts_alike_but_counts_nothing() {
    let mut p = Predictor::new(None);
    p.on_model(model(true, false));
    p.warp(0, SETTLED - FADE.as_micros() as u64 - 1);
    p.note_mouse(100, 40, SETTLED);
    p.on_tags(&tags(&[Tag { frame: 5, mouse_x: 60, mouse_y: 40, ..Tag::default() }]), SETTLED);
    let before = p.status(SETTLED);
    assert_eq!(p.peek(5, SETTLED + 1000).pixels, [-20.0, 0.0]);
    assert_eq!(p.peek(50, SETTLED + 1000), Warp::default());
    let after = p.status(SETTLED + 1000);
    assert_eq!((after.warped, after.untagged), (before.warped, before.untagged));
    assert_eq!(p.warp(5, SETTLED + 1000).pixels, [-20.0, 0.0]);
    assert_eq!(p.status(SETTLED + 1000).warped, before.warped + 1);
}

#[test]
fn shift_fades_in_over_the_fade_time() {
    let mut p = Predictor::new(None);
    p.on_model(model(true, false));
    p.note_mouse(100, 0, 0);
    p.on_tags(&tags(&[Tag { frame: 1, ..Tag::default() }]), 0);
    // A rebase does not happen at once: input was just sent.
    let first = p.warp(1, 0);
    assert!(!first.active, "{first:?}");
    let half = p.warp(1, FADE.as_micros() as u64 / 2);
    assert!((half.pixels[0] + 25.0).abs() < 0.5, "{half:?}");
    let full = p.warp(1, FADE.as_micros() as u64 * 2);
    assert_eq!(full.pixels, [-50.0, 0.0]);
}

#[test]
fn idle_input_rebases_lost_counts() {
    let mut p = Predictor::new(None);
    p.on_model(model(true, false));
    p.warp(0, 0);
    // The host never counted these 30.
    p.note_mouse(30, 0, 0);
    let idle = REBASE_IDLE.as_micros() as u64;
    p.on_tags(&tags(&[Tag { frame: 1, ..Tag::default() }]), idle);
    assert_eq!(p.warp(1, idle + 1).pixels, [0.0, 0.0]);
    // New input counts from the new base.
    p.note_mouse(10, 0, idle + 2);
    p.on_tags(&tags(&[Tag { frame: 2, ..Tag::default() }]), idle + 3);
    assert_eq!(p.warp(2, idle + 4).pixels, [-5.0, 0.0]);
}

#[test]
fn a_missing_tag_is_extrapolated_from_the_two_before() {
    let mut p = Predictor::new(None);
    p.on_model(model(true, false));
    p.warp(0, SETTLED - 200_000);
    p.note_mouse(500, 0, SETTLED);
    p.on_tags(
        &tags(&[
            Tag { frame: 10, mouse_x: 100, ..Tag::default() },
            Tag { frame: 11, mouse_x: 200, ..Tag::default() },
        ]),
        SETTLED,
    );
    // Frame 13 would reflect 400 if the rate held, leaving 100 counts.
    let warp = p.warp(13, SETTLED + 1);
    assert_eq!(warp.pixels, [-50.0, 0.0]);
    assert_eq!(p.status(SETTLED + 1).extrapolated, 1);
    // Too far ahead: no tag at all.
    let none = p.warp(30, SETTLED + 2);
    assert!(!none.active);
    assert_eq!(p.status(SETTLED + 2).untagged, 1);
}

#[test]
fn a_flat_model_slides_the_picture() {
    let mut p = Predictor::new(None);
    p.on_model(model(true, false));
    p.warp(0, SETTLED - 200_000);
    p.note_mouse(100, 40, SETTLED);
    p.on_tags(&tags(&[Tag { frame: 1, ..Tag::default() }]), SETTLED);
    let warp = p.warp(1, SETTLED + 1);
    assert_eq!((warp.pixels, warp.lens), ([-50.0, -10.0], FLAT));
    assert_eq!(source([0.9, 0.2], warp.uv, warp.lens), [0.9 + 0.05, 0.2 + 0.02]);
    assert_eq!(Warp::default().lens, FLAT);
}

/// Where a camera with a lens of `focal` pixels, turned so that the picture's centre moved by
/// the angles `turn` (radians, about the upright axis then the level one), shows the output
/// point `p` of a picture of `size` pixels: its ray turned back, as fractions of the picture.
fn turned_back(p: [f32; 2], turn: [f64; 2], focal: f64, size: [f64; 2]) -> [f64; 2] {
    let (x, y, z) = ((f64::from(p[0]) - 0.5) * size[0], (f64::from(p[1]) - 0.5) * size[1], focal);
    let (x, z) = (x * turn[0].cos() - z * turn[0].sin(), x * turn[0].sin() + z * turn[0].cos());
    let (y, z) = (y * turn[1].cos() - z * turn[1].sin(), y * turn[1].sin() + z * turn[1].cos());
    [0.5 + focal * x / z / size[0], 0.5 + focal * y / z / size[1]]
}

#[test]
fn a_lens_turns_the_picture_as_a_camera_does() {
    // 1000 pixels wide through a 500 pixel lens: a 90 degree field of view.
    let mut p = Predictor::new(None);
    p.on_model(Model { focal: 500.0, ..model(true, false) });
    p.warp(0, SETTLED - 200_000);
    p.note_mouse(100, 0, SETTLED);
    p.on_tags(&tags(&[Tag { frame: 1, ..Tag::default() }]), SETTLED);
    let warp = p.warp(1, SETTLED + 1);
    // The game turned by 50 / 500 radians; the centre shows it as the tangent.
    let angle = -50.0f64 / 500.0;
    assert!((f64::from(warp.pixels[0]) - 500.0 * angle.tan()).abs() < 1e-3, "{warp:?}");
    assert_eq!(warp.pixels[1], 0.0);
    for at in [[0.5, 0.5], [0.0, 0.5], [1.0, 0.5], [0.25, 0.1], [0.9, 0.95], [1.0, 0.0]] {
        let exact = turned_back(at, [angle, 0.0], 500.0, [1000.0, 500.0]);
        let drawn = source(at, warp.uv, warp.lens);
        assert!((f64::from(drawn[0]) - exact[0]).abs() < 1e-5 && (f64::from(drawn[1]) - exact[1]).abs() < 1e-5, "{at:?}: {drawn:?}, {exact:?}");
    }
    // The right edge moves almost twice as far as the centre, and a flat shift misses that.
    let edge = 1.0 - source([1.0, 0.5], warp.uv, warp.lens)[0];
    assert!(edge / -warp.uv[0] < -1.7, "{edge}");

    // Both axes at once, 30 by 12 pixels at the centre: the two turns' order, which the terms
    // leave out, moves the corners by less than a pixel.
    let (pixels, lens) = lens_terms([30.0, -12.0], 500.0, (1000, 500));
    let uv = [pixels[0] / 1000.0, pixels[1] / 500.0];
    for at in [[0.5, 0.5], [0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0], [0.3, 0.8]] {
        let exact = turned_back(at, [30.0 / 500.0, -12.0 / 500.0], 500.0, [1000.0, 500.0]);
        let drawn = source(at, uv, lens);
        let off = ((f64::from(drawn[0]) - exact[0]) * 1000.0).hypot((f64::from(drawn[1]) - exact[1]) * 500.0);
        assert!(off < 1.0, "{at:?}: {off} px");
        let back = destination(drawn, uv, lens).unwrap();
        assert!((back[0] - at[0]).abs() < 1e-4 && (back[1] - at[1]).abs() < 1e-4, "{at:?}: {back:?}");
    }
    assert!((field_of_view(&Model { focal: 500.0, ..model(true, false) }).unwrap() - 90.0).abs() < 1e-3);
    assert_eq!(field_of_view(&model(true, false)), None);
    // No lens, or a broken one, is a flat picture.
    assert_eq!(lens_terms([30.0, -12.0], 0.0, (1000, 500)), ([30.0, -12.0], FLAT));
    assert_eq!(lens_terms([30.0, -12.0], f32::NAN, (1000, 500)), ([30.0, -12.0], FLAT));
    // A point level with the camera or behind it is held at the least depth.
    let wide = source([1.0, 0.5], [0.0, 0.0], [-3.0, 0.0, 1.0, 1.0]);
    assert_eq!(wide, [0.5 + 0.5 / MIN_DEPTH, 0.5]);
    assert_eq!(destination([40.0, 0.5], [0.0, 0.0], [-3.0, 0.0, 1.0, 1.0]), None);
}

#[test]
fn shifts_are_clamped_to_the_model_limit() {
    let mut p = Predictor::new(None);
    p.on_model(model(true, false));
    p.warp(0, SETTLED - 200_000);
    p.note_mouse(10_000, -10_000, SETTLED);
    p.on_tags(&tags(&[Tag { frame: 1, ..Tag::default() }]), SETTLED);
    assert_eq!(p.warp(1, SETTLED + 1).pixels, [-200.0, 100.0]);
}

#[test]
fn stick_shift_integrates_what_was_held_after_the_tags_packet() {
    let mut p = Predictor::new(Some(0));
    p.on_model(model(false, true));
    p.warp(0, SETTLED - 200_000);
    // Packet 1 centred at t, packet 2 full right 10 ms later, held since.
    p.note_controller(0, (0, 0), SETTLED);
    p.note_controller(0, (32767, 0), SETTLED + 10_000);
    // A packet of another pad is not counted.
    p.note_controller(1, (32767, 32767), SETTLED + 10_500);
    // The frame reflects packet 2 held for 5 ms.
    p.on_tags(
        &tags(&[Tag {
            frame: 1,
            controller_seq: 2,
            controller_held_us: 5_000,
            ..Tag::default()
        }]),
        SETTLED + 20_000,
    );
    // At +40 ms: 25 ms of full right not reflected = 0.025 s * -1000 px/s.
    let warp = p.warp(1, SETTLED + 40_000);
    assert!((warp.pixels[0] + 25.0).abs() < 0.01, "{warp:?}");
    assert!(warp.pixels[1].abs() < 1e-6);
}

#[test]
fn stick_counts_ahead_of_the_client_shift_nothing() {
    let mut p = Predictor::new(Some(0));
    p.on_model(model(false, true));
    p.warp(0, SETTLED - 200_000);
    p.note_controller(0, (32767, 0), SETTLED);
    p.on_tags(
        &tags(&[Tag { frame: 1, controller_seq: 9, controller_held_us: 0, ..Tag::default() }]),
        SETTLED + 1,
    );
    assert_eq!(p.warp(1, SETTLED + 50_000).pixels, [0.0, 0.0]);
}

#[test]
fn unlocking_fades_out_with_the_last_locked_gains() {
    let mut p = Predictor::new(None);
    p.on_model(model(true, false));
    p.warp(0, SETTLED - 200_000);
    p.note_mouse(40, 0, SETTLED);
    p.on_tags(&tags(&[Tag { frame: 1, ..Tag::default() }]), SETTLED);
    assert_eq!(p.warp(1, SETTLED + 1).pixels, [-20.0, 0.0]);
    p.on_model(Model { mouse_locked: false, generation: 2, ..model(false, false) });
    let half = p.warp(1, SETTLED + 1 + FADE.as_micros() as u64 / 2);
    assert!((half.pixels[0] + 10.0).abs() < 0.2, "{half:?}");
    assert!(!p.warp(1, SETTLED + 1 + FADE.as_micros() as u64).active);
}

#[test]
fn a_start_request_restarts_the_counts_and_a_repeat_keeps_them() {
    let warp = FrameWarp::new(None, 64);
    warp.note_mouse(50, 0);
    warp.on_model(model(true, false));
    let mut mask = Mask::new(2, 8, 8);
    mask.set(1, 1, true);
    warp.on_mask(mask);
    let request = warp.request(true);
    assert!(request.enabled && request.controller.is_none() && request.mask_max_width == 64);
    assert_eq!(warp.status().model, None);
    assert_eq!(warp.mask_since(None), None);
    warp.on_model(model(true, false));
    warp.request(true);
    assert!(warp.status().model.is_some());
    warp.request(false);
    warp.request(true);
    assert_eq!(warp.status().model, None);
}

#[test]
fn mask_generation_follows_the_mask() {
    let warp = FrameWarp::new(None, 64);
    assert_eq!(warp.mask_since(None), None);
    let mut mask = Mask::new(3, 4, 2);
    mask.set(1, 1, true);
    warp.on_mask(mask.clone());
    assert_eq!(warp.mask_since(None), Some(Some(Arc::new(mask))));
    assert_eq!(warp.mask_since(Some(3)), None);
    warp.on_mask(Mask { generation: 4, ..Mask::default() });
    assert_eq!(warp.mask_since(Some(3)), Some(None));
    assert_eq!(warp.status().mask_cells, 0);
}

#[test]
fn stale_tags_do_not_rebase() {
    let mut p = Predictor::new(None);
    p.on_model(model(true, false));
    p.warp(0, 0);
    p.on_tags(&tags(&[Tag { frame: 9, ..Tag::default() }]), REBASE_IDLE.as_micros() as u64);
    p.note_mouse(50, 0, 2_000_000);
    // An older frame's tag arriving late after an idle spell must not reset the base.
    p.on_tags(&tags(&[Tag { frame: 8, ..Tag::default() }]), 4_000_000);
    p.on_tags(&tags(&[Tag { frame: 10, ..Tag::default() }]), 2_000_001);
    assert_eq!(p.warp(10, 4_000_001).pixels, [-25.0, 0.0]);
}

#[test]
fn input_seq_counts_the_input_that_can_move_the_shift() {
    let warp = FrameWarp::new(Some(1), 0);
    assert_eq!(warp.input_seq(), 0);
    warp.note_mouse(0, 0);
    assert_eq!(warp.input_seq(), 0, "no motion");
    warp.note_mouse(3, -2);
    assert_eq!(warp.input_seq(), 1);
    warp.note_controller(0, (32767, 0));
    assert_eq!(warp.input_seq(), 1, "another pad's packet");
    warp.note_controller(1, (0, 0));
    assert_eq!(warp.input_seq(), 2, "the tracked pad's, even at rest");
    warp.request(true);
    warp.note_mouse(1, 0);
    assert_eq!(warp.input_seq(), 3, "a start does not go back");
}

#[test]
fn stick_deflected_follows_the_tracked_stick_and_the_models_dead_zone() {
    let warp = FrameWarp::new(Some(0), 0);
    assert!(!warp.stick_deflected(), "nothing sent");
    warp.note_controller(0, (300, 0));
    assert!(warp.stick_deflected(), "before a model any deflection counts");
    warp.on_model(Model { stick_deadzone: 0.25, ..model(false, true) });
    assert!(!warp.stick_deflected(), "inside the model's dead zone");
    warp.note_controller(0, (0, -9000));
    assert!(warp.stick_deflected(), "past it");
    warp.note_controller(1, (0, 0));
    assert!(warp.stick_deflected(), "another pad changes nothing");
    warp.note_controller(0, (-8000, 0));
    assert!(!warp.stick_deflected(), "back inside");
    warp.note_controller(0, (i16::MIN, i16::MIN));
    assert!(warp.stick_deflected(), "fully down left");
    warp.note_controller(0, (-8000, 0));
    warp.request(true);
    assert!(warp.stick_deflected(), "a start forgets the model and its dead zone");
    let untracked = FrameWarp::new(None, 0);
    untracked.note_controller(0, (32767, 0));
    assert!(!untracked.stick_deflected(), "no tracked pad");
}

/// The stick integral as it was: every kept sample walked from the oldest.
fn walked(samples: &[StickSample], seq_offset: u32, tag: &Tag, model: &Model, now_us: u64) -> [f32; 2] {
    let earliest = now_us.saturating_sub(MAX_STICK_WINDOW.as_micros() as u64);
    let host_seq = tag.controller_seq.wrapping_add(seq_offset);
    let Some(newest) = samples.last() else {
        return [0.0; 2];
    };
    if host_seq != newest.seq && after(host_seq, newest.seq) {
        return [0.0; 2];
    }
    let start = if tag.controller_held_us == HELD_NONE || tag.controller_seq == 0 {
        earliest
    } else {
        match samples.iter().find(|s| s.seq == host_seq) {
            Some(sample) => sample.at_us + u64::from(tag.controller_held_us),
            None => earliest,
        }
    }
    .clamp(earliest, now_us);
    let mut sum = [0.0f32; 2];
    for (i, sample) in samples.iter().enumerate() {
        let end = samples.get(i + 1).map_or(now_us, |next| next.at_us).min(now_us);
        let from = sample.at_us.max(start);
        if end <= from {
            continue;
        }
        let held = (end - from) as f32 / 1_000_000.0;
        let curve = stick_curve(sample.stick.0, sample.stick.1, model.stick_deadzone, model.stick_exponent);
        sum[0] += curve[0] * held;
        sum[1] += curve[1] * held;
    }
    sum
}

#[test]
fn the_stick_integral_from_the_newest_end_matches_walking_every_packet() {
    let model = Model { stick_deadzone: 0.1, stick_exponent: 2.0, ..model(false, true) };
    let mut p = Predictor::new(Some(0));
    // Every packet ever sent, which the predictor trims to its window and margin.
    let mut sent: Vec<StickSample> = Vec::new();
    let mut seed = 0x9E37_79B9u32;
    let mut next = move |n: u32| {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed % n
    };
    let mut now = 1_000_000u64;
    let mut checked = 0;
    for packet in 1..=4000u32 {
        // Up to 8 ms apart, at times none.
        now += u64::from(next(8000));
        let stick = ((next(65536) as i32 - 32768) as i16, (next(65536) as i32 - 32768) as i16);
        let stick = if next(4) == 0 { (0, 0) } else { stick };
        p.note_controller(0, stick, now);
        sent.push(StickSample { at_us: now, seq: packet, stick });
        assert!(p.stick.len() <= sent.len() && p.stick.back().is_some_and(|s| s.seq == packet));
        if packet % 7 != 0 {
            continue;
        }
        // Tags naming packets up to 120 back (under a second, all kept), none, or one ahead.
        for back in [0u32, 1, 5, 40, 120] {
            for held in [0u32, 3_000, 20_000, 250_000, HELD_NONE] {
                let controller_seq = if back == 120 && held == 0 { packet + 2 } else { packet.saturating_sub(back) };
                let tag = Tag { frame: 1, controller_seq, controller_held_us: held, ..Tag::default() };
                // Presents a little after the newest packet, or computed for just before it.
                let at = (now + u64::from(next(50_000))).saturating_sub(u64::from(next(4) == 0) * 20_000);
                let newest_end = p.stick_unreflected(&tag, &model, at);
                assert_eq!(newest_end, walked(&sent, p.seq_offset, &tag, &model, at), "packet {packet}, {back} back, held {held}, at {at}");
                checked += usize::from(newest_end != [0.0; 2]);
            }
        }
    }
    assert!(checked > 1000, "the integrals were mostly not zero: {checked}");
    let kept = p.stick.back().unwrap().at_us - p.stick[1].at_us;
    assert!(kept <= STICK_HISTORY.as_micros() as u64 && p.stick.len() < 400, "the history is trimmed: {kept} us, {} packets", p.stick.len());
}

#[test]
fn the_edge_mirrors_the_picture_and_darkens_past_it() {
    let close = |a: f32, b: f32| (a - b).abs() < 1e-6;
    assert_eq!(edge([0.3, 0.9]), ([0.3, 0.9], 1.0), "inside, as it is");
    assert_eq!(edge([0.0, 1.0]), ([0.0, 1.0], 1.0), "the edges too");
    let (at, fade) = edge([-0.04, 0.5]);
    assert!(close(at[0], 0.04) && at[1] == 0.5 && close(fade, 1.0 - EDGE_DARKEN * 0.5), "{at:?} {fade}");
    let (at, fade) = edge([0.5, 1.2]);
    assert!(close(at[1], 0.8) && close(fade, 1.0 - EDGE_DARKEN), "{at:?} {fade}");
    let (_, fade) = edge([-0.02, 1.06]);
    assert!(close(fade, 1.0 - EDGE_DARKEN * 0.75), "the farther axis darkens: {fade}");
    assert_eq!(edge([-1.5, 2.5]).0, [1.0, 0.0], "past a whole picture, held at the far edge");
}
