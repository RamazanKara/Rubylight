use super::*;

#[test]
fn no_map_on_keyframes_recovery_pictures_or_the_first_frame() {
    let fresh = Some((7, false));
    assert_eq!(pick(true, false, false, false, fresh, 3), Pick::Skip);
    assert_eq!(pick(false, true, false, false, fresh, 3), Pick::Skip);
    assert_eq!(pick(false, false, true, false, fresh, 3), Pick::Skip);
    assert_eq!(pick(false, false, false, false, fresh, 3), Pick::Attach);
    let mut stats = Stats::default();
    for picked in [
        Pick::Skip,
        Pick::Skip,
        Pick::Attach,
        Pick::Stale,
        Pick::Uniform,
    ] {
        count(&mut stats, picked);
    }
    assert_eq!(
        (
            stats.skipped_recovery,
            stats.maps,
            stats.stale,
            stats.uniform
        ),
        (2, 1, 1, 1)
    );
}

#[test]
fn a_map_is_used_once() {
    assert_eq!(
        pick(false, false, false, false, Some((7, false)), 7),
        Pick::Stale
    );
    assert_eq!(
        pick(false, false, false, false, Some((6, false)), 7),
        Pick::Stale
    );
    assert_eq!(pick(false, false, false, false, None, 7), Pick::Stale);
    assert_eq!(
        pick(false, false, false, false, Some((8, false)), 7),
        Pick::Attach
    );
}

#[test]
fn a_uniform_map_is_never_attached() {
    assert_eq!(
        pick(false, false, false, false, Some((8, true)), 7),
        Pick::Uniform
    );
    // A keyframe skips before the map is looked at.
    assert_eq!(
        pick(true, false, false, false, Some((8, true)), 7),
        Pick::Skip
    );
}

#[test]
fn no_map_while_the_rate_control_settles() {
    // Frame 0 is the first, frame 100 a keyframe, frame 200 a recovery
    // picture, and the encoder makes a keyframe of its own before frame
    // 250. Focus is off until frame 10, then every frame has a fresh map.
    let mut state = State::default();
    let mut picks = Vec::new();
    for frame in 0..300u64 {
        if frame == 250 {
            state.keyframe_out();
        }
        let (idr, recover, first) = (frame == 100, frame == 200, frame == 0);
        let map = (frame >= 10).then_some(Some((frame + 1, false)));
        picks.push(state.frame(idr, recover, first, map));
    }
    assert!(picks[..10].iter().all(Option::is_none));
    let settle = SETTLE_FRAMES as usize;
    let skipped = [
        10..=settle,
        100..=100 + settle,
        200..=200 + settle,
        250..=249 + settle,
    ];
    for (frame, picked) in picks.iter().enumerate().skip(10) {
        let wanted = if skipped.iter().any(|window| window.contains(&frame)) {
            Pick::Skip
        } else {
            Pick::Attach
        };
        assert_eq!(*picked, Some(wanted), "frame {frame}");
    }
    assert_eq!((state.stats.skipped_recovery, state.stats.maps), (113, 177));
    // The last map was used, so it is not used again.
    assert_eq!(
        state.frame(false, false, false, Some(Some((300, false)))),
        Some(Pick::Stale)
    );
    // A settled stream stays settled.
    state.settled = u32::MAX;
    assert_eq!(
        state.frame(false, false, false, Some(Some((301, false)))),
        Some(Pick::Attach)
    );
    assert_eq!(state.settled, u32::MAX);
}

#[test]
fn a_failed_map_turns_focus_off_once() {
    let mut state = State {
        settings: Some(crate::compute::FocusSettings {
            mode: encoder_focus::Mode::Static,
            levels: Default::default(),
            cursor: None,
        }),
        ..Default::default()
    };
    state.fail(anyhow::anyhow!("AMF surface property HevcROIData"));
    assert!(state.settings.is_none());
    let failure = state
        .failure
        .take()
        .expect("the failure waits for the stream");
    assert!(format!("{failure:#}").contains("HevcROIData"));
    assert!(state.failure.is_none());
}

#[test]
fn the_map_grid_matches_the_coded_frame() -> Result<()> {
    // 1080 rows code as 1088: 68 macroblock rows, 17 rows of 64.
    assert_eq!(coded_size((1920, 1080)), (1920, 1088));
    check_grid(0, (1920, 1080), (120, 68))?;
    check_grid(1, (1920, 1080), (30, 17))?;
    check_grid(2, (1920, 1080), (30, 17))?;
    check_grid(0, (1280, 720), (80, 45))?;
    let error = check_grid(0, (1920, 1080), (120, 67)).unwrap_err();
    assert!(
        format!("{error}").contains("1920x1088 in 120x68"),
        "{error}"
    );
    assert!(check_grid(1, (1920, 1080), (120, 68)).is_err());
    assert!(check_grid(0, (3840, 2160), (120, 68)).is_err());
    Ok(())
}

#[test]
fn codec_property_names() {
    assert_eq!(
        [roi_name(0), roi_name(1), roi_name(2)],
        ["ROIData", "HevcROIData", "Av1ROIData"]
    );
    assert_eq!(
        [cap_name(0), cap_name(1), cap_name(2)],
        [Some("ROIMap"), Some("HevcROIMap"), None]
    );
    assert_eq!(qp_feedback(2), "Av1BlockQIndexFeedback");
    assert_eq!(qp_map(1), "HevcBlockQpMap");
}

#[test]
fn half_and_importance_deltas() {
    let qp = Grid {
        width: 5,
        height: 2,
        values: vec![30, 30, 99, 34, 34, 30, 30, 0, 34, 34],
    };
    assert_eq!(half_delta(&qp), Some(4.0));
    let importance = Grid {
        width: 5,
        height: 1,
        values: vec![10, 10, 5, 0, 0],
    };
    assert_eq!(importance_delta(&qp, &importance), Some(4.0));
    let uniform = Grid {
        width: 5,
        height: 1,
        values: vec![6; 5],
    };
    assert_eq!(importance_delta(&qp, &uniform), None);
}

/// What the RX 7900 XT's driver returns for `truth`: rows `stride` apart
/// in its own memory, copied out as if they were `width` apart.
fn strided(truth: &Grid, stride: usize) -> Grid {
    let width = truth.width as usize;
    let mut source = vec![0; stride * truth.height as usize];
    for (y, row) in truth.values.chunks_exact(width).enumerate() {
        source[y * stride..y * stride + width].copy_from_slice(row);
    }
    source.truncate(truth.values.len());
    Grid {
        values: source,
        ..truth.clone()
    }
}
/// QP 24 on the left half and 34 on the right, plus the row number.
fn halves_grid(width: u32, height: u32) -> Grid {
    Grid {
        width,
        height,
        values: (0..width * height)
            .map(|index| (index / width) + if index % width < width / 2 { 24 } else { 34 })
            .collect(),
    }
}

#[test]
fn strided_qp_maps_are_laid_out_again() {
    // HEVC and AV1 at 1080p: 30x17 blocks, a 1024-byte pitch. Rows 0 and 1
    // survive; at plain layout row 1 reads as zeros and the halves mix.
    let truth = halves_grid(30, 17);
    let read = strided(&truth, 256);
    assert_eq!(&read.values[30..60], &[0; 30]);
    let fixed = unstride(read, 256);
    assert_eq!(fixed.values[..60], truth.values[..60]);
    assert!(fixed.values[60..].iter().all(|&value| value == 0));
    assert_eq!(half_delta(&fixed), Some(10.0));
    // H.264 at 640x480: 40x30 macroblocks, rows 0 to 4 survive.
    let truth = halves_grid(40, 30);
    let fixed = unstride(strided(&truth, 256), 256);
    assert_eq!(fixed.values[..200], truth.values[..200]);
    assert!(fixed.values[200..].iter().all(|&value| value == 0));
    assert_eq!(half_delta(&fixed), Some(10.0));
    let importance = Grid {
        width: 40,
        height: 30,
        values: (0..1200)
            .map(|index| u32::from(index % 40 < 20) * 10)
            .collect(),
    };
    assert_eq!(importance_delta(&fixed, &importance), Some(10.0));
    // 10x8 at 640x480 HEVC: only row 0 fits below the stride.
    let truth = halves_grid(10, 8);
    let fixed = unstride(strided(&truth, 256), 256);
    assert_eq!(fixed.values[..10], truth.values[..10]);
    assert!(fixed.values[10..].iter().all(|&value| value == 0));
}

#[test]
fn plain_qp_maps_are_kept() {
    for (width, height) in [(30, 17), (10, 8), (120, 68), (40, 1)] {
        let truth = halves_grid(width, height);
        assert_eq!(unstride(truth.clone(), 256), truth);
    }
    // A packed pitch is the same either way.
    let truth = halves_grid(64, 4);
    assert_eq!(unstride(truth.clone(), 64), truth);
    assert_eq!(unstride(strided(&truth, 64), 64), truth);
    let empty = Grid {
        width: 30,
        height: 17,
        values: vec![0; 510],
    };
    assert_eq!(half_delta(&unstride(empty, 256)), None);
}

#[test]
fn verdict_needs_the_mirrored_maps_to_differ() {
    // HEVC at 1080p on the RX 7900 XT: +10.0 and -10.1, the picture +0.0.
    assert_eq!(effect(Some(10.0), Some(-10.0)), (Some(10.0), Some(0.0)));
    // A picture that codes its right half 3 QP higher cancels out.
    assert_eq!(effect(Some(7.0), Some(-1.0)), (Some(4.0), Some(3.0)));
    // A driver that ignores the map sees only the picture.
    assert_eq!(effect(Some(1.8), Some(1.8)), (Some(0.0), Some(1.8)));
    assert_eq!(effect(Some(1.0), None), (None, None));
    assert_eq!(verdict(1, Some(3.5)), Support::Verified { delta: 3.5 });
    assert!(matches!(
        verdict(1, Some(1.5)),
        Support::Unsupported {
            reason: Reason::CodecUnsupported,
            ..
        }
    ));
    // A map applied the wrong way round does not verify.
    assert!(matches!(
        verdict(0, Some(-10.0)),
        Support::Unsupported { .. }
    ));
    // AV1 q-index steps are four to a QP.
    assert!(matches!(verdict(2, Some(6.0)), Support::Unsupported { .. }));
    assert_eq!(verdict(2, Some(9.0)), Support::Verified { delta: 9.0 });
    let Support::Unsupported { message, .. } = verdict(0, None) else {
        panic!("no feedback must not verify");
    };
    assert!(message.contains("no block QP map"), "{message}");
}
