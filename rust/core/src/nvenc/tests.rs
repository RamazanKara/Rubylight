use super::*;
#[test]
fn native_444_is_h264_and_hevc_as_the_sdk_defines_it() {
    assert_eq!(sdk_444_refusal(0), None);
    assert_eq!(sdk_444_refusal(1), None);
    let av1 = sdk_444_refusal(2).unwrap();
    assert!(av1.contains("AV1 in 4:2:0 only"), "{av1}");
    assert!(av1.contains("Choose HEVC"), "{av1}");
    assert!(sdk_444_refusal(3).is_some());
    // 8-bit is AYUV on the D3D11 device; 10-bit goes through CUDA as
    // three 16-bit planes.
    assert_eq!(input_444(false), Input444::Ayuv);
    assert_eq!(input_444(true), Input444::Planar16);
}
#[test]
fn reviewed_driver_negotiation_keeps_older_h264_hevc_and_av1_layouts() {
    assert_eq!(ApiVersion::from_driver(0xc1), ApiVersion(12, 1));
    assert_eq!(
        ApiVersion::candidates(1, ApiVersion(12, 1)).collect::<Vec<_>>(),
        vec![ApiVersion(12, 1), ApiVersion(12, 0), ApiVersion(11, 0)]
    );
    // AV1 needs API 12.0 (driver 522.25), not 12.1.
    assert_eq!(
        ApiVersion::candidates(2, ApiVersion(12, 0)).collect::<Vec<_>>(),
        vec![ApiVersion(12, 0)]
    );
    assert_eq!(ApiVersion::candidates(2, ApiVersion(11, 0)).count(), 0);
    assert_eq!(
        ApiVersion::candidates(2, ApiVersion(12, 2)).collect::<Vec<_>>(),
        vec![ApiVersion(12, 2), ApiVersion(12, 1), ApiVersion(12, 0)]
    );
    assert_eq!(
        ApiVersion(12, 0).config(),
        ApiVersion(12, 1).config() & !(1 << 24)
    );
    assert_eq!(
        ApiVersion(12, 0).picture(),
        ApiVersion(12, 1).picture() & !(1 << 24)
    );
    // SDK 12.2 already changed these revisions; using 12.1 silently breaks 10-bit input.
    assert_eq!(ApiVersion(12, 2).initialize(), 0xf007000c | 2 << 24);
    assert_eq!(ApiVersion(12, 2).config(), 0xf009000c | 2 << 24);
    assert_eq!(ApiVersion(12, 1).lock(), 0xf001000c | 1 << 24);
    assert_eq!(ApiVersion(12, 0).lock(), 0x7002000c);
    assert_eq!(ApiVersion(11, 0).picture(), 0xf004000b);
}
#[test]
fn loss_recovery_includes_dependent_frames_is_bounded_and_confirms_once() {
    let mut references = References::default();
    references.completed(16, false);
    assert_eq!(
        references.plan(14, 14, 5, true),
        Recovery::Invalidate {
            first: 14,
            last: 16
        }
    );
    assert_eq!(references.plan(12, 12, 5, true), Recovery::Idr);
    assert_eq!(references.plan(17, 18, 5, true), Recovery::Idr);
    assert_eq!(references.plan(15, 14, 5, true), Recovery::Idr);
    assert_eq!(references.plan(0, 1, 5, true), Recovery::Idr);
    references.applied(14, 16);
    assert_eq!(references.plan(14, 15, 5, true), Recovery::AlreadyApplied);
    assert!(references.confirmation(false));
    assert!(!references.confirmation(false));
    references.applied(14, 16);
    assert!(!references.confirmation(true));
    references.completed(17, true);
    assert_eq!(references.plan(14, 15, 5, true), Recovery::AlreadyApplied);
    assert_eq!(references.plan(17, 17, 5, true), Recovery::Idr);
    references.completed(19, false);
    assert_eq!(references.plan(16, 18, 5, true), Recovery::Idr);
    assert_eq!(
        references.plan(18, 18, 5, true),
        Recovery::Invalidate {
            first: 18,
            last: 19
        }
    );
    assert_eq!(references.plan(14, 15, 5, false), Recovery::Idr);
}
fn tuning(text: &str, codec: u8) -> Tuning {
    let stream = Negotiated {
        codec,
        ..Default::default()
    };
    Tuning::new(&Config::parse(text).unwrap(), &stream).unwrap()
}
#[test]
fn defaults_stay_latency_first_with_spatial_aq_and_the_budget_guard_on() {
    let default = tuning("", 1);
    assert_eq!(default.preset, 1);
    assert_eq!(default.multipass, 1);
    assert_eq!(default.split, 0);
    assert_eq!(default.rate_control, RateControl::Cbr);
    assert!(default.spatial_aq);
    assert!(default.frame_budget_guard);
    assert!(!default.temporal_aq && !default.cavlc && !default.filler);
    assert_eq!(default.min_qp, None);
    let off = tuning(
        "nvenc_spatial_aq = false\nnvenc_frame_budget_guard = false\n",
        1,
    );
    assert!(!off.spatial_aq && !off.frame_budget_guard);
}
#[test]
fn foundation_settings_parse_with_codec_bounds_and_fall_back_on_unknown_values() {
    for (value, mode) in [
        ("disabled", SPLIT_DISABLED),
        ("false", SPLIT_DISABLED),
        ("auto", 0),
        ("driver_decides", 0),
        ("enabled", 1),
        ("forced", 1),
        ("two_strips", 2),
        ("three_strips", 3),
        ("four_strips", 4),
        ("sideways", 0),
    ] {
        assert_eq!(
            tuning(&format!("nvenc_split_encode = {value}\n"), 1).split,
            mode,
            "{value}"
        );
    }
    let vbr = "nvenc_rate_control = vbr\nnvenc_target_quality = 70\n";
    let vbr_at = |target_quality, peak_percent| RateControl::Vbr {
        target_quality,
        peak_percent,
    };
    assert_eq!(tuning(vbr, 1).rate_control, vbr_at(51, 150));
    assert_eq!(tuning(vbr, 2).rate_control, vbr_at(63, 150));
    assert_eq!(
        tuning("nvenc_rate_control = vbr\n", 0).rate_control,
        vbr_at(0, 150)
    );
    for (value, percent) in [
        ("1", 100),
        ("1.25", 125),
        ("2", 200),
        ("3", 300),
        ("4", 150),
    ] {
        assert_eq!(
            tuning(
                &format!("nvenc_rate_control = vbr\nnvenc_peak_bitrate_ratio = {value}\n"),
                1
            )
            .rate_control,
            vbr_at(0, percent),
            "{value}"
        );
    }
    // CBR has no peak of its own.
    assert_eq!(
        tuning("nvenc_peak_bitrate_ratio = 3\n", 1).rate_control,
        RateControl::Cbr
    );
    assert_eq!(vbr_at(0, 200).max_bitrate(20_000_000), 40_000_000);
    assert_eq!(
        tuning("nvenc_rate_control = cq\n", 0).rate_control,
        RateControl::Cbr
    );
    assert_eq!(RateControl::Cbr.max_bitrate(20_000_000), 20_000_000);
    assert_eq!(vbr_at(0, 150).max_bitrate(20_000_000), 30_000_000);
    assert_eq!(vbr_at(0, 150).max_bitrate(u32::MAX), u32::MAX);
    assert_eq!(tuning("nvenc_preset = 9\n", 1).preset, 7);
    assert_eq!(tuning("nvenc_twopass = full_res\n", 1).multipass, 2);
    assert_eq!(tuning("nvenc_twopass = disabled\n", 1).multipass, 0);
}
#[test]
fn slice_delivery_follows_the_amf_setting_with_a_slice_per_block() {
    assert_eq!(tuning("", 1).slice_blocks, 0, "auto is off at 1080p");
    let four = tuning("video_slice_delivery = 4\n", 0);
    assert_eq!(four.slice_blocks, 4);
    assert_eq!(four.subframe_slices(1), 4);
    assert_eq!(four.subframe_slices(8), 8);
    assert_eq!(
        tuning("video_slice_delivery = 2\n", 1).subframe_slices(0),
        2
    );
    // AV1 returns tiles, which slice delivery does not stream.
    assert_eq!(tuning("video_slice_delivery = 4\n", 2).slice_blocks, 0);
}
#[test]
fn a_fixed_slice_mode_is_set_only_where_it_cannot_block_split_encoding() {
    let auto = tuning("", 1);
    assert!(!auto.slice_mode(1, ApiVersion(13, 0)));
    assert!(auto.slice_mode(4, ApiVersion(13, 0)));
    // Older drivers have no split encoding to protect.
    assert!(auto.slice_mode(1, ApiVersion(12, 1)));
    let disabled = tuning("nvenc_split_encode = disabled\n", 1);
    assert!(disabled.slice_mode(1, ApiVersion(13, 0)));
}
#[test]
fn the_frame_budget_guard_lowers_slow_presets_only_when_they_miss_the_budget() {
    // 4K120 on one engine: 8.3 MPix in 5.4 ms leaves only P1, and even
    // P1 (5.8 ms) misses.
    let uhd = frame_budget(4, (3840, 2160), 120., 1, true);
    assert_eq!((uhd.configured, uhd.effective), (4, 1));
    assert!(uhd.over_at_floor);
    assert!((uhd.budget_ms - 1000. / 120. * 0.65).abs() < 1e-9);
    // Two engines with split encoding make P5 fit, not P7.
    let split = frame_budget(7, (3840, 2160), 120., 2, true);
    assert_eq!(split.effective, 5);
    assert!(!split.over_at_floor);
    assert!(split.estimated_ms <= split.budget_ms);
    // 1080p60 fits P7.
    let easy = frame_budget(7, (1920, 1080), 60., 1, true);
    assert_eq!(easy.effective, 7);
    assert!(!easy.over_at_floor);
    // The guard off keeps the preset and still reports what it costs.
    let kept = frame_budget(4, (3840, 2160), 120., 1, false);
    assert_eq!(kept.effective, 4);
    assert_eq!(kept.estimated_ms, kept.configured_ms);
    assert!(kept.estimated_ms > kept.budget_ms);
    // P1, the default, is never changed; unknown rates keep the preset.
    assert_eq!(frame_budget(1, (7680, 4320), 240., 1, true).effective, 1);
    assert_eq!(frame_budget(5, (1920, 1080), 0., 1, true).effective, 5);
    let unknown = frame_budget(5, (1920, 1080), f64::NAN, 1, true);
    assert_eq!((unknown.effective, unknown.budget_ms), (5, 0.));
    assert_eq!(frame_budget(5, (1920, 1080), -60., 1, true).effective, 5);
    assert_eq!(frame_budget(0, (1920, 1080), 60., 1, true).configured, 1);
}
#[test]
fn the_d3d12_interface_and_block_statistics_follow_the_api() {
    assert!(!ApiVersion(11, 0).d3d12());
    assert!(ApiVersion(12, 0).d3d12() && ApiVersion(13, 0).d3d12());
    assert!(!ApiVersion(12, 0).block_stats());
    assert!(ApiVersion(12, 1).block_stats() && ApiVersion(13, 0).block_stats());
}
#[test]
fn the_focus_delta_map_moves_qp_between_blocks_around_the_mean() {
    let mut out = Vec::new();
    // H.264: a delta per macroblock, half a QP per importance step.
    focus_deltas(0, (32, 16), &[10, 0], (2, 1), &mut out).unwrap();
    assert_eq!(out, [-3, 3]);
    // A uniform map moves nothing.
    focus_deltas(0, (32, 16), &[7, 7], (2, 1), &mut out).unwrap();
    assert_eq!(out, [0, 0]);
    // HEVC's 32x32 CTBs each take the 64x64 block they lie in.
    focus_deltas(1, (128, 64), &[10, 0], (2, 1), &mut out).unwrap();
    assert_eq!(out, [-3, -3, 3, 3, -3, -3, 3, 3]);
    // A partial CTB at the edge takes the last block.
    focus_deltas(1, (100, 40), &[10, 0], (2, 1), &mut out).unwrap();
    assert_eq!(out.len(), 8);
    assert_eq!(out[3], 3);
    // AV1 is in q-index, four steps per QP.
    focus_deltas(2, (128, 64), &[10, 0], (2, 1), &mut out).unwrap();
    assert_eq!(out, [-10, 10]);
    // Values over 10 count as 10.
    focus_deltas(0, (32, 16), &[30, 0], (2, 1), &mut out).unwrap();
    assert_eq!(out, [-3, 3]);
    assert_eq!(delta_grid(1, 1920, 1080), (60, 34));
    assert_eq!(delta_grid(2, 1920, 1080), (30, 17));
    // A map of another size is refused.
    let error = focus_deltas(0, (32, 16), &[1, 2, 3], (3, 1), &mut out).unwrap_err();
    assert!(error.contains("3x1"), "{error}");
    assert!(focus_deltas(0, (32, 16), &[1], (2, 1), &mut out).is_err());
}
