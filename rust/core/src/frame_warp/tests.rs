use super::*;
use motion::tests::scene;

/// The GPU's HUD bits, shaped, as the worker hands them over.
fn hud(host: &mut FrameWarpHost, shaped: &Mask, now_us: u64) -> Option<(usize, usize)> {
    let placement = host.placement();
    host.on_hud(&placement.place(shaped), placement, now_us)
}

fn enabled(picture: (u16, u16)) -> FrameWarpHost {
    let mut host = FrameWarpHost::new(picture);
    let request = FrameWarpRequest {
        enabled: true,
        controller: Some(0),
        mask_max_width: 480,
        lens: true,
    };
    assert_eq!(host.on_request(&request.encode()), Some(true));
    host
}

/// A thumbnail through the worker's steps (`FrameWarpState::on_thumbnail`), in place.
fn learn(host: &mut FrameWarpHost, measurer: &mut Measurer, thumbnail: Thumbnail, now: u64) {
    let measured = measurer.measure(&thumbnail);
    let keep = host.on_measured(&measured, now);
    measurer.keep(thumbnail, keep, host.resets());
    if let Some(job) = host.fit_job(now) {
        host.publish_fit(job.run());
    }
}

#[test]
fn only_a_client_that_draws_the_lens_is_sent_one() {
    let prior = Prior {
        mouse_gain: Some([1.0, 1.0]),
        mouse_lag_us: Some(30_000),
        focal: Some(960.0),
        picture_width: 1920,
        picture_height: 1080,
        ..Prior::default()
    };
    let request = |lens| {
        FrameWarpRequest {
            enabled: true,
            controller: None,
            mask_max_width: 480,
            lens,
        }
        .encode()
    };
    let focal = |host: &mut FrameWarpHost| {
        let model = host.take_model().expect("a model for the request");
        Model::decode(&model).expect("a valid model").focal
    };
    let mut host = FrameWarpHost::new((1920, 1080));
    host.set_prior(prior);
    // A client that slides the picture is sent no lens, whatever the game's is. Its gains
    // are the picture centre's under that lens all the same: the median cell's, longer,
    // would carry the middle of its picture too far.
    assert_eq!(host.on_request(&request(false)), Some(true));
    assert_eq!(focal(&mut host), 0.0);
    assert!(host.calibrator.lens().is_some());
    // And the game's lens is remembered.
    assert_eq!(host.prior().unwrap().focal, Some(960.0));
    // One that draws it gets it with its request, and loses it with the next that doesn't.
    host.on_request(&request(true));
    assert!((focal(&mut host) - 960.0).abs() < 0.01);
    host.on_request(&request(false));
    assert_eq!(focal(&mut host), 0.0);
}

#[test]
fn the_measurer_leaves_out_what_stands_still_with_the_gpus_hud() {
    assert_eq!(both(None, Some(&[true])), None);
    assert_eq!(both(Some(&[true]), None), None);
    assert_eq!(
        both(Some(&[true, false, false]), Some(&[false, false, true])),
        Some(vec![true, false, true])
    );
    assert_eq!(both(Some(&[true]), Some(&[true, false])), None);
    // A chequered box stays where it is while the scene pans behind it.
    let boxed = |ox: f32, at_us: u64| {
        let mut t = scene(256, 144, ox, 0.0, at_us);
        for y in 20..50 {
            for x in 190..240 {
                t.luma[y * 256 + x] = if (x / 5 + y / 5) % 2 == 0 { 230 } else { 20 };
            }
        }
        t
    };
    let mut measurer = Measurer::default();
    let mut last = None;
    for i in 0..12u64 {
        let thumbnail = boxed(i as f32 * 3.0, 1 + i * 16_000);
        last = measurer.measure(&thumbnail).shift;
        measurer.keep(thumbnail, true, Resets::default());
    }
    let found = measurer.still.mask().expect("the box");
    assert!(found[30 * 256 + 200] && !found[100 * 256 + 100]);
    let (_, motion) = last.expect("a move");
    assert!((motion.dx + 3.0).abs() < 0.3, "{motion:?}");
    // With the GPU's HUD both are left out, and what stood still stays when the HUD goes.
    let mut hud = vec![false; 256 * 144];
    hud[100 * 256 + 100] = true;
    measurer.set_hud(hud, Resets::default());
    let merged = measurer.both.as_deref().expect("both");
    assert!(merged[30 * 256 + 200] && merged[100 * 256 + 100]);
    measurer.catch_up(Resets {
        hud: 1,
        ..Resets::default()
    });
    assert!(measurer.both.is_none() && measurer.hud.is_none());
    assert!(measurer.still.mask().is_some());
}

#[test]
fn a_reset_drops_what_was_measured_against_before_it() {
    let mut host = enabled((1920, 1080));
    let mut measurer = Measurer::default();
    let h = thumbnail_height((1920, 1080));
    learn(
        &mut host,
        &mut measurer,
        scene(256, h, 0.0, 0.0, 1_000),
        1_000,
    );
    learn(
        &mut host,
        &mut measurer,
        scene(256, h, 3.0, 0.0, 9_000),
        9_000,
    );
    assert_eq!(host.samples, 1);
    // Measured against the last thumbnail, but the picture changed meanwhile.
    let measured = measurer.measure(&scene(256, h, 6.0, 0.0, 17_000));
    assert!(measured.shift.is_some());
    host.set_picture((2560, 1440), (0, 0, 2560, 1440));
    assert!(host.on_measured(&measured, 17_000));
    assert_eq!(host.samples, 1);
    // The HUD laid over the old thumbnail goes with it.
    let mut mask = Mask::new(0, 480, 270);
    mask.set(10, 10, true);
    let shaped = shape(&mask);
    host.set_picture((1920, 1080), (0, 0, 1920, 1080));
    let size = hud(&mut host, &shaped, 20_000).unwrap();
    measurer.set_hud(hud_pixels(&shaped, size), host.resets());
    assert!(measurer.hud.is_some());
    host.set_picture((2560, 1440), (0, 0, 2560, 1440));
    measurer.catch_up(host.resets());
    assert!(measurer.hud.is_none() && measurer.previous.is_none());
    // A fit started before a request is not taken; the next thumbnail fits again.
    let job = host.fit_job(300_000).expect("a fit is due");
    host.on_request(
        &FrameWarpRequest {
            enabled: true,
            controller: None,
            mask_max_width: 480,
            lens: true,
        }
        .encode(),
    );
    host.publish_fit(job.run());
    assert!(host.fit_job(300_000).is_some());
}

#[test]
fn nothing_happens_until_a_client_asks() {
    let mut host = FrameWarpHost::new((1920, 1080));
    host.on_mouse(5, 5, 10);
    host.tag(1, 100);
    assert_eq!(host.take_tags(), None);
    assert_eq!(host.mask_width(), 0);
    assert_eq!(host.on_request(&[0, 1]), None);
}

#[test]
fn hud_cells_cover_the_thumbnail_pixels_under_them() {
    let mut mask = Mask::new(0, 8, 4);
    mask.set(7, 0, true);
    mask.set(0, 3, true);
    let hud = hud_pixels(&mask, (16, 8));
    let set: Vec<(usize, usize)> = (0..16 * 8)
        .filter(|&i| hud[i])
        .map(|i| (i % 16, i / 16))
        .collect();
    assert_eq!(
        set,
        [
            (14, 0),
            (15, 0),
            (14, 1),
            (15, 1),
            (0, 6),
            (1, 6),
            (0, 7),
            (1, 7)
        ]
    );
}

#[test]
fn a_start_request_restarts_the_counts_and_a_repeat_keeps_them() {
    let newest = |host: &mut FrameWarpHost| {
        let mut last = None;
        while let Some(wire) = host.take_tags() {
            last = Some(Tags::decode(&wire).unwrap());
        }
        *last.unwrap().tags.last().unwrap()
    };
    let request = |enabled| {
        FrameWarpRequest {
            enabled,
            controller: Some(0),
            mask_max_width: 480,
            lens: true,
        }
        .encode()
    };
    let mut host = enabled((1920, 1080));
    host.on_mouse(30, 0, 1_000);
    host.on_controller(0, (0, 0), 1_000);
    host.tag(1, 100_000);
    let tag = newest(&mut host);
    assert_eq!((tag.mouse_x, tag.controller_seq), (30, 1));
    // A repeated start keeps the counts.
    host.on_request(&request(true));
    host.tag(3, 100_000);
    let tag = newest(&mut host);
    assert_eq!((tag.mouse_x, tag.controller_seq), (30, 1));
    // A stop and a new start restart them.
    host.on_request(&request(false));
    host.on_request(&request(true));
    host.tag(4, 200_000);
    let tag = newest(&mut host);
    assert_eq!((tag.mouse_x, tag.controller_seq), (0, 0));
}

#[test]
fn another_pad_counts_from_zero_and_drops_what_named_the_old_one() {
    let request = |controller| {
        FrameWarpRequest {
            enabled: true,
            controller,
            mask_max_width: 480,
            lens: true,
        }
        .encode()
    };
    let mut host = enabled((1920, 1080));
    host.on_mouse(30, 0, 1_000);
    host.on_controller(0, (0, 0), 1_000);
    host.on_controller(0, (900, 0), 2_000);
    host.tag(1, 100_000);
    host.calibrator.add(calibrate::Sample {
        t0_us: 50_000,
        t1_us: 60_000,
        motion: Motion::default(),
    });
    assert!(host.calibrator.earliest_input_us().is_some());
    // The same pad again keeps everything.
    host.on_request(&request(Some(0)));
    assert!(host.calibrator.earliest_input_us().is_some());
    assert_eq!(host.recent.len(), 1);
    // Another pad: the old pad's tags and the samples that read its stick go.
    host.on_request(&request(Some(1)));
    assert_eq!(host.take_tags(), None);
    assert!(host.recent.is_empty());
    assert_eq!(host.calibrator.earliest_input_us(), None);
    assert_eq!(host.history.controller(), Some(1));
    // The old pad no longer counts; the new one counts from its first packet. The mouse sums
    // carry on.
    host.on_controller(0, (5, 5), 110_000);
    host.tag(2, 200_000);
    let tag = Tags::decode(&host.take_tags().unwrap()).unwrap().tags;
    assert_eq!(tag.len(), 1);
    assert_eq!(
        (
            tag[0].mouse_x,
            tag[0].controller_seq,
            tag[0].controller_held_us
        ),
        (30, 0, rubylight_protocol::frame_warp::HELD_NONE)
    );
    host.on_controller(1, (0, 0), 210_000);
    host.tag(3, 300_000);
    let mut last = None;
    while let Some(wire) = host.take_tags() {
        last = Some(Tags::decode(&wire).unwrap());
    }
    let tag = *last.unwrap().tags.last().unwrap();
    assert_eq!((tag.mouse_x, tag.controller_seq), (30, 1));
    // No pad at all: nothing counts.
    host.on_request(&request(None));
    host.on_controller(1, (0, 0), 310_000);
    host.tag(4, 400_000);
    let tag = Tags::decode(&host.take_tags().unwrap()).unwrap().tags;
    assert_eq!(
        (tag[0].controller_seq, tag[0].controller_held_us),
        (0, rubylight_protocol::frame_warp::HELD_NONE)
    );
}

#[test]
fn a_fit_job_copies_only_the_history_its_samples_read() {
    let mut host = enabled((1920, 1080));
    for i in 0..1_000u64 {
        host.on_mouse(1, 0, i * 1_000);
        host.on_controller(0, ((i % 7) as i16 * 1_000, 0), i * 1_000);
    }
    assert!(
        host.fit_job(1_000_000)
            .unwrap()
            .history
            .last_input_us()
            .is_some()
    );
    host.calibrator.add(calibrate::Sample {
        t0_us: 800_000,
        t1_us: 810_000,
        motion: Motion::default(),
    });
    let job = host.fit_job(1_000_000).unwrap();
    let from = 800_000 - calibrate::MAX_LAG_US;
    for t in [from, from + 1, 700_000, 805_000, 999_000, 2_000_000] {
        assert_eq!(job.history.mouse_at(t), host.history.mouse_at(t));
        assert_eq!(
            job.history.stick_integral(from, t, 0.1, 1.0),
            host.history.stick_integral(from, t, 0.1, 1.0)
        );
        assert_eq!(job.history.tag(1, t, t), host.history.tag(1, t, t));
    }
    assert!(job.history.mouse_entries() < host.history.mouse_entries() / 2);
}

#[test]
fn tags_carry_the_newest_frames_and_the_input_before_the_lag() {
    let mut host = enabled((1920, 1080));
    // The model sent on request first.
    let first = Model::decode(&host.take_model().unwrap()).unwrap();
    assert!(!first.locked());
    host.on_mouse(10, 0, 1_000);
    host.on_mouse(10, 0, 100_000);
    for frame in 1..=6 {
        host.tag(frame, 100_000 + u64::from(frame) * 1_000);
    }
    let mut last = None;
    while let Some(wire) = host.take_tags() {
        last = Some(Tags::decode(&wire).unwrap());
    }
    let tags = last.unwrap();
    assert_eq!(
        tags.tags.iter().map(|t| t.frame).collect::<Vec<_>>(),
        [3, 4, 5, 6]
    );
    // With the default 40 ms lag, a capture at 106 ms reflects input up to 66 ms.
    assert_eq!(tags.tags[3].mouse_x, 10);
}

#[test]
fn learns_from_thumbnails_and_flags_the_camera_moving() {
    let mut host = enabled((1920, 1080));
    let mut measurer = Measurer::default();
    host.take_model();
    let h = thumbnail_height((1920, 1080));
    // The camera pans 2 thumbnail pixels per 8 ms frame, for 40 ms of mouse input each.
    let mut t = 0u64;
    let mut offset = 0.0;
    let mut sum = 0;
    while t < 3_000_000 {
        host.on_mouse(16, 0, t);
        sum += 16;
        let shown = (sum as f32 - 16.0 * 5.0).max(0.0);
        offset = shown * 0.125;
        learn(&mut host, &mut measurer, scene(256, h, offset, 0.0, t), t);
        t += 8_000;
    }
    assert!(offset > 100.0);
    assert!(host.camera_moving(t - 8_000));
    assert!(!host.camera_moving(t + MOTION_HOLD_US));
    // Over a thumbnail pixel between captures: the HUD pass compares each with the one before.
    assert!(
        host.hud_every_us(t - 8_000)
            .is_some_and(|every| every <= 8_000)
    );
    let model = host.status().model.expect("a model");
    assert!(model.mouse_locked, "{model:?}");
    // The content moves left 0.125 thumbnail px per count: -0.9375 picture px.
    assert!((model.mouse_gain[0] + 0.9375).abs() < 0.08, "{model:?}");
}

#[test]
fn a_slow_turn_counts_over_the_window_and_noise_does_not() {
    // 0.25 thumbnail px per 8 ms capture: the scene moves a pixel every 32 ms.
    let mut host = enabled((1920, 1080));
    for n in 0..20u64 {
        host.add_shift((n * 8_000, (n + 1) * 8_000, 0.25, 0.0), (n + 1) * 8_000);
    }
    let every = host.hud_every_us(160_000).expect("turning");
    assert!((31_000..=33_000).contains(&every), "{every}");
    assert_eq!(host.hud_every_us(160_000 + MOTION_HOLD_US), None);
    // Shifts back and forth add up to nothing: the camera stands still.
    let mut host = enabled((1920, 1080));
    for n in 0..40u64 {
        let dx = if n % 2 == 0 { 0.4 } else { -0.4 };
        host.add_shift((n * 8_000, (n + 1) * 8_000, dx, 0.0), (n + 1) * 8_000);
    }
    assert_eq!(host.hud_every_us(320_000), None);
    // Nor does a turn that moves less than a pixel in the window.
    let mut host = enabled((1920, 1080));
    for n in 0..40u64 {
        host.add_shift((n * 8_000, (n + 1) * 8_000, 0.07, 0.0), (n + 1) * 8_000);
    }
    assert_eq!(host.hud_every_us(320_000), None);
}

#[test]
fn hud_masks_are_shaped_halved_to_fit_and_rate_limited() {
    let mut host = enabled((1920, 1080));
    assert_eq!(host.mask_width(), 480);
    let mut mask = Mask::new(0, 480, 270);
    mask.set(10, 10, true);
    hud(&mut host, &shape(&mask), 0);
    let sent = Mask::decode(&host.take_mask().unwrap()).unwrap();
    assert_eq!(sent.count(), 9);
    assert_eq!(sent.generation, 1);
    // The same mask again, or a changed one too soon: nothing.
    hud(&mut host, &shape(&mask), 2_000_000);
    assert_eq!(host.take_mask(), None);
    for x in 200..205 {
        for y in 100..105 {
            mask.set(x, y, true);
        }
    }
    hud(&mut host, &shape(&mask), 2_100_000);
    assert!(host.take_mask().is_some());
    hud(&mut host, &shape(&Mask::new(0, 480, 270)), 2_200_000);
    assert_eq!(host.take_mask(), None);
    // A client asking again gets the last one.
    let again = FrameWarpRequest {
        enabled: true,
        controller: None,
        mask_max_width: 480,
        lens: true,
    };
    host.on_request(&again.encode());
    assert!(host.take_mask().is_some());
}

#[test]
fn hud_pieces_close_together_are_joined_and_far_ones_are_not() {
    // 960x540 cells: pieces up to 16 cells apart join (8 each way).
    let mut mask = Mask::new(0, 960, 540);
    for x in [100, 110, 125] {
        mask.set(x, 50, true);
    }
    mask.set(110, 62, true);
    mask.set(170, 50, true);
    mask.set(959, 539, true);
    let shaped = shape(&mask);
    // The three pieces of a line, and the one under it, are one region, grown a cell.
    for x in 99..=126 {
        assert!(shaped.get(x, 50), "{x}");
    }
    for y in 49..=63 {
        assert!(shaped.get(110, y), "{y}");
    }
    assert!(!shaped.get(98, 50) && !shaped.get(127, 50));
    // The piece 45 cells on stays apart, as does the corner, grown a cell and not shrunk.
    assert!((127..169).all(|x| !shaped.get(x, 50)));
    assert!((169..=171).all(|x| shaped.get(x, 50)) && !shaped.get(172, 50));
    assert!(shaped.get(958, 538) && shaped.get(959, 539));
    assert!(!shaped.get(957, 539));
}

#[test]
fn a_narrow_client_gets_a_halved_mask() {
    let mut host = FrameWarpHost::new((1920, 1080));
    let request = FrameWarpRequest {
        enabled: true,
        controller: None,
        mask_max_width: 300,
        lens: true,
    };
    host.on_request(&request.encode());
    assert_eq!(host.mask_width(), 240);
    let mut mask = Mask::new(0, 960, 540);
    mask.set(100, 100, true);
    hud(&mut host, &shape(&mask), 0);
    let sent = Mask::decode(&host.take_mask().unwrap()).unwrap();
    assert_eq!((sent.width, sent.height), (240, 135));
}

#[test]
fn a_letterboxed_capture_is_measured_alone_and_its_hud_lands_where_it_shows() {
    // A 16:9 capture in a 16:10 stream, with bars 90 pixels high above and below it.
    let mut host = FrameWarpHost::new((2880, 1800));
    host.set_picture((2880, 1800), (0, 90, 2880, 1620));
    host.on_request(
        &FrameWarpRequest {
            enabled: true,
            controller: None,
            mask_max_width: 720,
            lens: true,
        }
        .encode(),
    );
    assert_eq!(host.thumbnail_size(), (256, 144));
    let placement = host.placement();
    assert_eq!(placement.mask, (720, 450));
    assert_eq!(host.mask_width(), 720);
    // The GPU's grid over the 2560x1440 capture: 720 by 405 cells of 4 picture pixels.
    let mut grid = Mask::new(0, 720, 405);
    grid.set(100, 200, true);
    let placed = placement.place(&grid);
    // Grid row 200 shows at picture rows 890 to 894, centred on 892: mask row 223 (892 to 896).
    assert!(placed.get(100, 223) && !placed.get(100, 222) && !placed.get(100, 200));
    // The bars, and cells reaching into them, are HUD; the rest of the picture is not.
    for x in [0, 359, 719] {
        assert!((0..=22).all(|y| placed.get(x, y)));
        assert!((427..450).all(|y| placed.get(x, y)));
        assert!(!placed.get(x, 23) && !placed.get(x, 426));
    }
    assert_eq!(placed.count(), 46 * 720 + 1);
    assert!(hud(&mut host, &grid, 0).is_some());
    let sent = Mask::decode(&host.take_mask().unwrap()).unwrap();
    assert_eq!(
        (sent.width, sent.height, sent.count()),
        (720, 450, 46 * 720 + 1)
    );
    // Pillarboxed: a 4:3 capture in a 16:9 stream, bars left and right.
    host.set_picture((1920, 1080), (240, 0, 1440, 1080));
    let placement = host.placement();
    assert_eq!((placement.mask, host.mask_width()), ((480, 270), 360));
    let clear = Mask::new(0, 360, 270);
    let placed = placement.place(&clear);
    assert!((0..60).all(|x| placed.get(x, 0)) && !placed.get(60, 0) && placed.get(420, 269));
    // A stale placement is not sent: the picture changed meanwhile.
    let stale = placement.place(&clear);
    host.set_picture((1920, 1080), (0, 0, 1920, 1080));
    assert_eq!(host.on_hud(&stale, placement, 3_000_000), None);
    // Without bars, a grid at the mask's size is the mask itself.
    let placement = host.placement();
    let whole = Mask::new(0, 480, 270);
    assert!(matches!(placement.place(&whole), Cow::Borrowed(_)));
}

#[test]
fn thumbnail_height_keeps_the_aspect() {
    assert_eq!(thumbnail_height((1920, 1080)), 144);
    assert_eq!(thumbnail_height((2560, 1600)), 160);
    assert_eq!(thumbnail_height((3440, 1440)), 107);
    assert_eq!(thumbnail_height((1080, 1920)), 256);
}
