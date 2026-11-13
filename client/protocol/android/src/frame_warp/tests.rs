use super::*;

#[test]
fn the_ring_finds_frames_by_their_exact_time_and_forgets_old_ones() {
    let mut ring = FrameRing::new();
    assert_eq!(ring.find(0), None, "an empty ring knows no time, not even zero");
    for frame in 0..FRAME_RING as u32 + 10 {
        ring.note(1_000 + i64::from(frame) * 8_333, frame + 100);
    }
    let newest = FRAME_RING as u32 + 9;
    assert_eq!(ring.find(1_000 + i64::from(newest) * 8_333), Some(newest + 100));
    assert_eq!(ring.find(1_000 + 10 * 8_333), Some(110), "the oldest still kept");
    assert_eq!(ring.find(1_000 + 9 * 8_333), None, "pushed out");
    assert_eq!(ring.find(1_000 + 10 * 8_333 + 1), None, "only an exact time counts");
    ring.clear();
    assert_eq!(ring.find(1_000 + i64::from(newest) * 8_333), None);
}

#[test]
fn a_repeated_time_finds_the_newest_frame() {
    let mut ring = FrameRing::new();
    ring.note(5_000, 1);
    ring.note(6_000, 2);
    ring.note(5_000, 3);
    assert_eq!(ring.find(5_000), Some(3));
    assert_eq!(ring.find(6_000), Some(2));
}

#[test]
fn masks_pack_one_byte_per_cell_rows_from_the_top() {
    let mut mask = Mask::new(7, 40, 3);
    mask.set(0, 0, true);
    mask.set(31, 0, true);
    mask.set(32, 0, true);
    mask.set(39, 2, true);
    mask.set(5, 1, true);
    let mut out = vec![9u8; 40 * 3];
    assert!(pack_r8(&mask, &mut out));
    for y in 0..3u16 {
        for x in 0..40u16 {
            let expected = if mask.get(x, y) { CELL_SET } else { CELL_CLEAR };
            assert_eq!(out[usize::from(y) * 40 + usize::from(x)], expected, "cell {x},{y}");
        }
    }
    assert_eq!(out.iter().filter(|&&c| c == CELL_SET).count(), 5);
    assert!(!pack_r8(&mask, &mut [0u8; 40 * 3 - 1]), "too small");
    let mut larger = vec![7u8; 40 * 3 + 2];
    assert!(pack_r8(&mask, &mut larger));
    assert_eq!(&larger[120..], &[7, 7], "nothing past the cells is written");
}

#[test]
fn warps_encode_their_mask_and_inactive_draws_as_is() {
    let inactive = Warp {
        pixels: [3.0, 4.0],
        uv: [0.1, 0.2],
        lens: [0.05, -0.02, 1.001, 1.004],
        active: false,
        mask_generation: Some(3),
    };
    assert_eq!(WARP_STILL, [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0], "no shift, a flat picture");
    assert_eq!(encode_warp(&inactive), (WARP_INACTIVE, WARP_STILL));
    let active = Warp {
        active: true,
        mask_generation: None,
        ..inactive
    };
    assert_eq!(
        encode_warp(&active),
        (WARP_NO_MASK, [0.1, 0.2, 3.0, 4.0, 0.05, -0.02, 1.001, 1.004]),
        "the shift, then the lens terms"
    );
    let flat = Warp { lens: FLAT, ..active };
    assert_eq!(encode_warp(&flat).1, [0.1, 0.2, 3.0, 4.0, 0.0, 0.0, 1.0, 1.0]);
    let masked = Warp {
        mask_generation: Some(65535),
        ..active
    };
    assert_eq!(encode_warp(&masked).0, 65535);
}

#[test]
fn mask_updates_follow_the_generation_the_renderer_holds() {
    assert_eq!(mask_update(None, -1), MaskUpdate::Unchanged);
    assert_eq!(mask_update(None, 4), MaskUpdate::Cleared, "the stream ended");
    let warp = FrameWarp::new(Some(CONTROLLER), MASK_MAX_WIDTH);
    assert_eq!(mask_update(Some(&warp), -1), MaskUpdate::Unchanged);
    let mut mask = Mask::new(4, 8, 2);
    mask.set(1, 1, true);
    warp.on_mask(mask.clone());
    assert_eq!(mask_update(Some(&warp), -1), MaskUpdate::New(Arc::new(mask.clone())));
    assert_eq!(mask_update(Some(&warp), 4), MaskUpdate::Unchanged);
    assert_eq!(mask_update(Some(&warp), 3), MaskUpdate::New(Arc::new(mask)));
    warp.on_mask(Mask::new(5, 8, 2));
    assert_eq!(mask_update(Some(&warp), 4), MaskUpdate::Cleared, "a clear mask clears");
    assert_eq!(mask_update(Some(&warp), -1), MaskUpdate::Unchanged);
}

fn model(mouse: bool, stick: bool) -> Model {
    Model {
        generation: 2,
        mouse_locked: mouse,
        stick_locked: stick,
        picture_width: 1920,
        picture_height: 1080,
        mouse_gain: [0.94, -0.9],
        stick_gain: [850.0, 900.0],
        lag_us: 38_400,
        confidence: 0.92,
        ..Model::default()
    }
}

#[test]
fn status_numbers_say_what_the_overlay_shows() {
    let mut n = [9.0; STATUS_NUMBERS];
    assert_eq!(status_numbers(false, true, false, None, &mut n), STATUS_OFF);
    assert_eq!(n, [0.0; STATUS_NUMBERS]);
    assert_eq!(status_numbers(true, false, false, None, &mut n), STATUS_NOT_OFFERED);
    assert_eq!(status_numbers(true, true, false, None, &mut n), STATUS_LEARNING);
    let mut status = Status {
        pixels: [12.0, 5.0],
        peak: 41.0,
        ..Status::default()
    };
    assert_eq!(status_numbers(true, true, false, Some(&status), &mut n), STATUS_LEARNING, "no model yet");
    status.model = Some(model(false, false));
    assert_eq!(status_numbers(true, true, false, Some(&status), &mut n), STATUS_LEARNING, "unlocked");
    status.model = Some(model(true, true));
    assert_eq!(status_numbers(true, true, false, Some(&status), &mut n), STATUS_MOUSE);
    assert_eq!(n, [0.94, 38.4, 13.0, 41.0]);
    status.model = Some(model(false, true));
    assert_eq!(status_numbers(true, true, false, Some(&status), &mut n), STATUS_STICK);
    assert_eq!(n, [900.0, 38.4, 13.0, 41.0]);
    status.peak = 1.0;
    status_numbers(true, true, false, Some(&status), &mut n);
    assert_eq!(n[3], 13.0, "the peak is never below the shift shown with it");
    assert_eq!(status_numbers(true, true, true, Some(&status), &mut n), STATUS_DECLINED);
    assert_eq!(n, [0.0; STATUS_NUMBERS]);
    assert_eq!(
        status_numbers(true, false, true, Some(&status), &mut n),
        STATUS_NOT_OFFERED,
        "not offered says more than declined"
    );
}

#[test]
fn model_lines_name_the_lock_and_numbers() {
    let line = model_line(&model(true, false));
    assert!(line.contains("model 2 mouse locked for 1920x1080"), "{line}");
    assert!(line.contains("0.940/-0.900 px/count"), "{line}");
    assert!(line.contains("lag 38.4 ms"), "{line}");
    assert!(line.ends_with(", no lens"), "{line}");
    let lens = model_line(&Model { focal: 960.0, ..model(true, false) });
    assert!(lens.ends_with(", a 90 degree lens"), "{lens}");
    assert!(model_line(&model(false, false)).contains("unlocked"));
}

#[test]
fn peeking_is_not_drawing() {
    let _serial = crate::engine::tests_serial();
    let warp = FrameWarp::new(Some(CONTROLLER), MASK_MAX_WIDTH);
    begin(Some(warp.clone()));
    warp.on_model(model(true, false));
    for _ in 0..5 {
        assert_eq!(peek_for(Some(7)), warp_for(Some(7)), "the same shift either way");
    }
    let drawn = warp.status();
    for _ in 0..50 {
        peek_for(Some(7));
        peek_for(None);
    }
    let peeked = warp.status();
    assert_eq!(
        (peeked.warped, peeked.extrapolated, peeked.untagged),
        (drawn.warped, drawn.extrapolated, drawn.untagged),
        "only drawn pictures count"
    );
    assert_eq!(peek_for(None), (WARP_INACTIVE, WARP_STILL));
    end();
    assert_eq!(peek_for(Some(7)), (WARP_INACTIVE, WARP_STILL), "no stream, nothing to shift");
}

#[test]
fn the_c_table_draws_as_is_without_a_stream() {
    // Serialised with the engine's tests: this reads the stream's state.
    let _serial = crate::engine::tests_serial();
    end();
    assert_eq!((API.frame_for_pts)(5), -1);
    // One float more than the table writes: nothing past WARP_FLOATS is touched.
    let mut out = [7.0f32; WARP_FLOATS + 1];
    assert_eq!(unsafe { (API.warp)(3, out.as_mut_ptr()) }, WARP_INACTIVE);
    assert_eq!(out[..WARP_FLOATS], WARP_STILL);
    assert_eq!(out[WARP_FLOATS], 7.0);
    assert_eq!(unsafe { (API.warp)(-1, std::ptr::null_mut()) }, WARP_INACTIVE);
    let mut peeked = [7.0f32; WARP_FLOATS + 1];
    assert_eq!(unsafe { (API.peek)(3, peeked.as_mut_ptr()) }, WARP_INACTIVE);
    assert_eq!(peeked[..WARP_FLOATS], WARP_STILL);
    assert_eq!(peeked[WARP_FLOATS], 7.0);
    assert_eq!(unsafe { (API.peek)(-1, std::ptr::null_mut()) }, WARP_INACTIVE);
    let (mut w, mut h) = (0u32, 0u32);
    assert_eq!(unsafe { (API.mask)(-1, &mut w, &mut h, std::ptr::null_mut(), 0) }, MASK_UNCHANGED);
    assert_eq!(unsafe { (API.mask)(2, &mut w, &mut h, std::ptr::null_mut(), 0) }, MASK_CLEARED);
    assert_eq!((API.input_seq)(), 0);
    assert_eq!((API.stick_deflected)(), 0);
    assert_eq!(input_state(), 0);
    assert_eq!(API.version, API_VERSION);
    assert_ne!(api_address(), 0);
}

#[test]
fn the_input_state_packs_the_count_below_the_stick() {
    assert_eq!(pack_input_state(0, false), 0);
    assert_eq!(pack_input_state(7, false), 7);
    assert_eq!(pack_input_state(u32::MAX, false), 0xFFFF_FFFF, "the count stays positive");
    assert_eq!(pack_input_state(7, true), INPUT_STICK_DEFLECTED | 7);
    assert_eq!(
        pack_input_state(u32::MAX, true) as u32,
        u32::MAX,
        "Java reads the count as the low int"
    );
}

#[test]
fn the_c_table_counts_the_input_noted() {
    let _serial = crate::engine::tests_serial();
    let warp = FrameWarp::new(Some(CONTROLLER), MASK_MAX_WIDTH);
    begin(Some(warp.clone()));
    let before = (API.input_seq)();
    warp.note_mouse(4, -2);
    let after = (API.input_seq)();
    assert_ne!(after, before, "a mouse move is new input");
    assert_eq!(input_state() as u32, after);
    assert_eq!((API.stick_deflected)(), i32::from(warp.stick_deflected()));
    end();
    assert_eq!((API.input_seq)(), 0, "no stream, nothing counted");
}

#[test]
fn the_c_table_reads_the_streams_frames_and_mask() {
    let _serial = crate::engine::tests_serial();
    let warp = FrameWarp::new(Some(CONTROLLER), MASK_MAX_WIDTH);
    begin(Some(warp.clone()));
    note_frame(16_000, 41);
    assert_eq!((API.frame_for_pts)(16_000), 41);
    assert_eq!((API.frame_for_pts)(16_001), -1);
    let mut out = [7.0f32; WARP_FLOATS];
    assert_eq!(unsafe { (API.warp)(41, out.as_mut_ptr()) }, WARP_INACTIVE, "no model yet");
    assert_eq!(out, WARP_STILL);
    let mut peeked = [7.0f32; WARP_FLOATS];
    assert_eq!(unsafe { (API.peek)(41, peeked.as_mut_ptr()) }, WARP_INACTIVE, "peek agrees with warp");
    assert_eq!(peeked, WARP_STILL);

    let mut mask = Mask::new(9, 3, 2);
    mask.set(2, 1, true);
    warp.on_mask(mask);
    let (mut w, mut h) = (0u32, 0u32);
    assert_eq!(unsafe { (API.mask)(-1, &mut w, &mut h, std::ptr::null_mut(), 0) }, MASK_TOO_SMALL);
    assert_eq!((w, h), (3, 2));
    let mut cells = [1u8; 6];
    assert_eq!(unsafe { (API.mask)(-1, &mut w, &mut h, cells.as_mut_ptr(), cells.len()) }, 9);
    assert_eq!(cells, [0, 0, 0, 0, 0, CELL_SET]);
    assert_eq!(unsafe { (API.mask)(9, &mut w, &mut h, cells.as_mut_ptr(), cells.len()) }, MASK_UNCHANGED);

    decline();
    assert!(declined());
    decline();
    assert!(declined(), "declining twice is declining");
    end();
    assert!(!declined(), "each stream decides again");
    assert_eq!((API.frame_for_pts)(16_000), -1, "a new stream starts its frames over");
    assert_eq!(unsafe { (API.mask)(9, &mut w, &mut h, cells.as_mut_ptr(), cells.len()) }, MASK_CLEARED);
}
