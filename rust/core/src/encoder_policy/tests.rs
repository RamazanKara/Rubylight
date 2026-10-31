use super::{KeyframeCapScale, keyframe_cap_bytes};

/// #59 rc.4 run 1's cap, and two nominal 219.5 Mbps frames at 100 FPS.
const TARGET: u64 = 1_166_893;
const TWO_FRAMES: u64 = 548_750;

#[test]
fn a_keyframe_cap_scale_is_inert_while_keyframes_keep_to_the_cap() {
    let mut scale = KeyframeCapScale::default();
    for target in [50_000, 300_000, TARGET, 4_000_000] {
        for share in [0.1, 0.5, 0.9, 1.0, 1.1] {
            let bytes = (target as f64 * share) as u64;
            scale.observe(bytes, target, target);
            assert_eq!(scale.scale(), 1.);
            assert_eq!(scale.apply(target, TWO_FRAMES.min(target)), target);
            assert_eq!(scale.apply(target, TWO_FRAMES), target);
        }
    }
    // Nothing to learn without a target or a cap the driver held.
    scale.observe(5_000_000, 0, TARGET);
    scale.observe(5_000_000, TARGET, 0);
    assert_eq!(scale.scale(), 1.);
}

#[test]
fn a_proportional_overshoot_lands_within_the_cap_in_two_keyframes() {
    let mut scale = KeyframeCapScale::default();
    let mut keyframes = 0;
    loop {
        let written = scale.apply(TARGET, TWO_FRAMES);
        let bytes = (written as f64 * 1.64) as u64;
        keyframes += 1;
        if bytes as f64 <= 1.1 * TARGET as f64 {
            break;
        }
        assert!(keyframes < 3, "still {bytes} B after {keyframes} keyframes");
        scale.observe(bytes, TARGET, written);
    }
    assert!(keyframes <= 2, "{keyframes}");
    assert!(
        (scale.scale() - 1. / 1.64).abs() < 1e-3,
        "{}",
        scale.scale()
    );
}

#[test]
fn a_large_overshoot_floors_the_scale_and_the_cap_at_two_frames() {
    let mut scale = KeyframeCapScale::default();
    scale.observe(4 * TARGET, TARGET, TARGET);
    assert_eq!(scale.scale(), KeyframeCapScale::FLOOR);
    assert_eq!(scale.apply(TARGET, TWO_FRAMES), 583_447);
    // Two frames above the scaled cap hold it there; a target under two
    // frames is not raised.
    assert_eq!(scale.apply(TARGET, 700_000), 700_000);
    assert_eq!(scale.apply(400_000, 700_000), 400_000);
}

#[test]
fn a_cap_the_driver_ignores_for_keyframes_returns_the_scale_to_one_for_good() {
    // RX 9070 XT: keyframes come out the same size whatever the cap.
    let mut scale = KeyframeCapScale::default();
    assert_eq!(scale.observe(1_908_217, TARGET, TARGET), None);
    assert!(scale.scale() < 0.62);
    let written = scale.apply(TARGET, TWO_FRAMES);
    let (lowered, now) = scale.observe(1_908_217, TARGET, written).unwrap();
    assert!(
        (lowered - 1.635).abs() < 1e-3 && lowered == now,
        "{lowered} {now}"
    );
    assert_eq!(scale.scale(), 1.);
    assert_eq!(scale.apply(TARGET, TWO_FRAMES), TARGET);
    // Inter frames are not held under the target again, and the switch
    // is reported once.
    for bytes in [1_908_217, 4 * TARGET, TARGET, TARGET / 2] {
        assert_eq!(scale.observe(bytes, TARGET, TARGET), None);
        assert_eq!(scale.scale(), 1.);
    }
    // A lower cap that shrank the keyframe, if not enough, lowers it more.
    let mut scale = KeyframeCapScale::default();
    scale.observe(3 * TARGET / 2, TARGET, TARGET);
    let first = scale.scale();
    let written = scale.apply(TARGET, TWO_FRAMES);
    scale.observe(13 * TARGET / 10, TARGET, written);
    assert!(scale.scale() < first, "{} {first}", scale.scale());
}

#[test]
fn a_keyframe_cap_scale_relaxes_back_once_keyframes_keep_to_the_lower_cap() {
    let mut scale = KeyframeCapScale::default();
    scale.observe(1_908_217, TARGET, TARGET);
    let lowered = scale.scale();
    assert!(lowered < 0.62);
    // Small keyframes say nothing about the cap: it holds.
    let written = scale.apply(TARGET, TWO_FRAMES);
    scale.observe(written / 3, TARGET, written);
    assert_eq!(scale.scale(), lowered);
    // Keyframes as large as the lower cap allows, and no larger.
    let mut keyframes = 0;
    while scale.scale() < 1. {
        let written = scale.apply(TARGET, TWO_FRAMES);
        scale.observe(written, TARGET, written);
        keyframes += 1;
        assert!(keyframes <= 5, "scale {} after {keyframes}", scale.scale());
    }
    assert_eq!(scale.apply(TARGET, TWO_FRAMES), TARGET);
}

#[test]
fn only_an_automatic_cap_in_frame_mode_is_scaled() {
    let mut scale = KeyframeCapScale::default();
    scale.observe(1_908_217, TARGET, TARGET);
    let scaled = scale.apply(TARGET, TWO_FRAMES);
    assert!(scaled < TARGET);
    assert_eq!(
        keyframe_cap_bytes(true, 0, &scale, TARGET, TWO_FRAMES),
        scaled
    );
    // An explicit amd_max_frame_size, and slice delivery, keep the target.
    assert_eq!(
        keyframe_cap_bytes(false, 0, &scale, TARGET, TWO_FRAMES),
        TARGET
    );
    assert_eq!(
        keyframe_cap_bytes(true, 2, &scale, TARGET, TWO_FRAMES),
        TARGET
    );
}

#[test]
fn the_probe_accepts_amf_and_auto_and_rejects_any_other_encoder() {
    assert!(super::probe_option("amf").is_ok());
    assert!(super::probe_option("auto").is_ok());
    for other in ["nvenc", "qsv", "software", "amdvce", ""] {
        let error = super::probe_option(other).unwrap_err().to_string();
        assert!(error.contains("AMF-only"), "{other}: {error}");
    }
}

use super::*;
#[test]
fn amf_preanalysis_and_quality_rc_require_eight_bit_sdr() {
    for codec in 0..=2 {
        for (hdr, sdr_10bit) in [(false, false), (true, false), (false, true)] {
            for (rc, value) in [("vbr_peak", 2), ("qvbr", 4), ("hqvbr", 5), ("hqcbr", 6)] {
                for pa in [false, true] {
                    let config =
                        Config::parse(&format!("amd_rc={rc}\namd_preanalysis={pa}")).unwrap();
                    let stream = Negotiated {
                        codec,
                        hdr,
                        sdr_10bit,
                        ..Default::default()
                    };
                    let properties = amf(&config, &stream).unwrap();
                    let expected_pa = !stream.ten_bit() && (pa || value >= 4);
                    let expected_rc = if stream.ten_bit() && value >= 4 {
                        2
                    } else {
                        value
                    };
                    assert!(
                        properties
                            .iter()
                            .any(|p| p.name.ends_with("RateControlMethod")
                                && p.value == Value::Integer(expected_rc))
                    );
                    assert!(
                        properties
                            .iter()
                            .any(|p| p.name.ends_with("EnablePreAnalysis")
                                && p.value == Value::Boolean(expected_pa)
                                && p.required == expected_pa)
                    );
                    assert_eq!(
                        properties
                            .iter()
                            .any(|p| p.name == "PALookAheadBufferDepth"),
                        expected_pa
                    );
                }
            }
        }
    }
}
#[test]
fn amf_default_policy_bytes_are_unchanged_for_sdr_hdr_and_ten_bit_sdr() {
    // Full ordered policy before the eligibility fix, including requiredness.
    for (codec, expected) in [
        (
            0,
            "Usage=Integer(1):true\nQualityPreset=Integer(1):true\nRateControlMethod=Integer(3):false\nEnablePreAnalysis=Boolean(false):false\nEnableVBAQ=Boolean(false):true\nEnforceHRD=Boolean(false):false\nRateControlSkipFrameEnable=Boolean(false):false\nProfile=Integer(100):true\nBPicturesPattern=Integer(0):false\n",
        ),
        (
            1,
            "HevcUsage=Integer(1):true\nHevcQualityPreset=Integer(10):true\nHevcRateControlMethod=Integer(1):false\nHevcEnablePreAnalysis=Boolean(false):false\nHevcEnableVBAQ=Boolean(true):true\nHevcEnforceHRD=Boolean(false):false\nHevcRateControlSkipFrameEnable=Boolean(false):false\n",
        ),
        (
            2,
            "Av1Usage=Integer(2):true\nAv1QualityPreset=Integer(100):true\nAv1RateControlMethod=Integer(1):false\nAv1EnablePreAnalysis=Boolean(false):false\nAv1AQMode=Integer(1):true\nAv1EnforceHRD=Boolean(false):false\nAv1BPicturesPattern=Integer(0):false\n",
        ),
    ] {
        for (hdr, sdr_10bit) in [(false, false), (true, false), (false, true)] {
            let stream = Negotiated {
                codec,
                hdr,
                sdr_10bit,
                ..Default::default()
            };
            let actual: String = amf(&Config::default(), &stream)
                .unwrap()
                .iter()
                .map(|p| format!("{}={:?}:{}\n", p.name, p.value, p.required))
                .collect();
            assert_eq!(actual.as_bytes(), expected.as_bytes());
        }
    }
}
#[test]
fn amf_disables_b_frames_only_for_codecs_with_a_b_picture_pattern() {
    for (codec, expected) in [
        (0, Some("BPicturesPattern")),
        (1, None),
        (2, Some("Av1BPicturesPattern")),
    ] {
        let stream = Negotiated {
            codec,
            ..Default::default()
        };
        let properties = amf(&Config::default(), &stream).unwrap();
        let patterns: Vec<_> = properties
            .iter()
            .filter(|property| property.name.ends_with("BPicturesPattern"))
            .map(|property| {
                assert_eq!(property.value, Value::Integer(0));
                assert!(!property.required);
                property.name.as_str()
            })
            .collect();
        assert_eq!(patterns, expected.into_iter().collect::<Vec<_>>());
    }
}
#[test]
fn amf_rate_limits_default_to_two_frames_preserve_opt_out_and_use_codec_bits() {
    for (codec, prefix, cap) in [
        (0, "", "MaxAUSize"),
        (1, "Hevc", "MaxAUSize"),
        (2, "Av1", "MaxCompressedFrameSize"),
    ] {
        let stream = Negotiated {
            codec,
            bitrate_kbps: 40_000,
            fps: 60,
            rate_millihz: 59_940,
            ..Default::default()
        };
        for text in [
            "",
            "amd_max_frame_size=   ",
            "amd_max_frame_size=auto",
            "amd_max_frame_size=\"AUTO\"",
            "amd_max_frame_size=2",
        ] {
            let properties = amf_rate_control(&Config::parse(text).unwrap(), &stream).unwrap();
            assert_eq!(properties.len(), 1);
            assert_eq!(properties[0].name, format!("{prefix}{cap}"));
            assert_eq!(properties[0].value, Value::Integer(1_334_668));
            assert!(!properties[0].required);
        }
        for text in [
            "amd_max_frame_size=0",
            "amd_peak_bitrate_ratio=0\namd_vbv_buffer_frames=0\namd_max_frame_size=0",
        ] {
            assert!(
                amf_rate_control(&Config::parse(text).unwrap(), &stream)
                    .unwrap()
                    .is_empty()
            );
        }
        for (peak, vbv, cap_frames, expected) in [
            (1., 0.5, 1., [40_000_000, 333_667, 667_334]),
            (1.5, 1., 2., [60_000_000, 667_334, 1_334_668]),
            (2., 2., 4., [80_000_000, 1_334_668, 2_669_336]),
        ] {
            let config = Config::parse(&format!(
                    "amd_peak_bitrate_ratio={peak}\namd_vbv_buffer_frames={vbv}\namd_max_frame_size={cap_frames}"
                )).unwrap();
            let properties = amf_rate_control(&config, &stream).unwrap();
            for ((property, suffix), expected) in properties
                .iter()
                .zip(["PeakBitrate", "VBVBufferSize", cap])
                .zip(expected)
            {
                assert_eq!(property.name, format!("{prefix}{suffix}"));
                assert_eq!(property.value, Value::Integer(expected));
                assert_eq!(property.required, suffix != cap);
            }
        }
    }
}

#[test]
fn automatic_frame_caps_do_not_override_explicit_profiles() {
    for text in [
        "",
        "amd_max_frame_size=",
        "amd_max_frame_size=auto",
        "amd_max_frame_size=\"AUTO\"",
    ] {
        assert!(amf_frame_cap_automatic(&Config::parse(text).unwrap()));
    }
    for value in ["0", "1", "2", "4", "8"] {
        assert!(!amf_frame_cap_automatic(
            &Config::parse(&format!("amd_max_frame_size={value}")).unwrap()
        ));
    }
}

#[test]
fn amf_rate_limits_reject_invalid_settings() {
    for (key, invalid) in [
        ("amd_peak_bitrate_ratio", "0.9"),
        ("amd_peak_bitrate_ratio", "2.1"),
        ("amd_vbv_buffer_frames", "0.4"),
        ("amd_vbv_buffer_frames", "2.1"),
        ("amd_max_frame_size", "0.5"),
        ("amd_max_frame_size", "8.1"),
    ] {
        for value in [invalid, "-1", "NaN", "inf", "invalid"] {
            let config = Config::parse(&format!("{key}={value}")).unwrap();
            assert!(amf_rate_control(&config, &Negotiated::default()).is_err());
        }
    }
}

#[test]
fn amf_ltr_anchors_share_the_negotiated_reference_budget() {
    let config = Config::parse("amd_ltr_frames=4\n").unwrap();
    for codec in 0..=2 {
        for (references, expected_ltr) in [(1, 0), (2, 1), (3, 2), (4, 3), (5, 4), (16, 4)] {
            let stream = Negotiated {
                codec,
                references,
                ..Default::default()
            };
            let count = amf_ltr_frames(&config, &stream);
            assert_eq!(count, expected_ltr);
            assert!(count < references as usize);
            let properties = amf(&config, &stream).unwrap();
            assert!(properties.iter().any(|property| {
                property.name.ends_with("MaxNumRefFrames")
                    && property.value == Value::Integer(i64::from(references))
                    && property.required
            }));
        }
    }
}

#[test]
fn amf_keeps_long_term_references_on_every_codec_for_every_client() {
    let one = Config::parse("amd_ltr_frames=1\n").unwrap();
    let off = Config::parse("amd_ltr_frames=0\n").unwrap();
    for codec in [0, 1, 2] {
        // A stock client, and Nightjar's launch parameter, which no
        // longer changes anything: the same setting and budget.
        for ltr_recovery in [false, true] {
            let stream = Negotiated {
                codec,
                ltr_recovery,
                ..Default::default()
            };
            assert_eq!(amf_ltr_requested(&Config::default(), &stream), 4);
            assert_eq!(amf_ltr_frames(&Config::default(), &stream), 4);
            assert_eq!(amf_ltr_requested(&one, &stream), 1);
            assert_eq!(amf_ltr_requested(&off, &stream), 0);
            assert_eq!(amf_ltr_frames(&off, &stream), 0);
            let budget = Negotiated {
                references: 3,
                ..stream.clone()
            };
            assert_eq!(amf_ltr_frames(&Config::default(), &budget), 2);
        }
    }
    // PyroWave keeps no long-term references.
    let pyrowave = Negotiated {
        codec: 3,
        ltr_recovery: true,
        ..Default::default()
    };
    assert_eq!(amf_ltr_requested(&Config::default(), &pyrowave), 0);
}

#[test]
fn amf_offers_invalidation_with_four_anchors_unless_turned_off() {
    let unset = Config::default();
    assert!(amf_offers_invalidation(&unset));
    let av1 = Negotiated {
        codec: 2,
        ..Default::default()
    };
    assert_eq!(amf_ltr_requested(&unset, &av1), 4);
    assert_eq!(amf_ltr_frames(&unset, &av1), 4);
    let off = Config::parse("amd_ltr_frames=0\n").unwrap();
    assert!(!amf_offers_invalidation(&off));
    assert_eq!(amf_ltr_requested(&off, &av1), 0);
}

#[test]
fn one_reference_avc_intra_refresh_falls_back_without_expanding_the_budget() {
    let config = Config::parse("amd_ltr_frames=4\n").unwrap();
    for references in [0, 1, 2, 4] {
        let stream = Negotiated {
            references,
            intra_refresh: true,
            ..Default::default()
        };
        let enabled = references != 1;
        assert_eq!(amf_intra_refresh(&stream), enabled);
        let properties = amf(&config, &stream).unwrap();
        assert!(properties.iter().any(|property| {
            property.name == "MaxNumRefFrames"
                && property.value
                    == Value::Integer(if references == 0 {
                        2
                    } else {
                        i64::from(references)
                    })
        }));
        assert_eq!(
            properties
                .iter()
                .any(|property| { property.name == "IntraRefreshMBsNumberPerSlot" }),
            enabled
        );
        assert_eq!(amf_ltr_frames(&config, &stream), 0);
    }
}

#[test]
fn hevc_intra_refresh_uses_actual_ctb_rows_and_preserves_decoder_budget() {
    for (width, height, ctbs, fallback) in [
        (1280, 720, 20, 1),
        (1920, 1080, 30, 2),
        (2560, 1440, 40, 4),
        (3840, 2160, 60, 7),
        (1921, 1081, 31, 2),
        (1, 1, 1, 1),
        (64, 7680, 1, 1),
        (64, 7681, 2, 1),
    ] {
        for hdr in [false, true] {
            let stream = Negotiated {
                width,
                height,
                codec: 1,
                intra_refresh: true,
                references: 1,
                hdr,
                ..Default::default()
            };
            assert_eq!(
                amf_hevc_intra_refresh(&stream),
                Some(AmfHevcIntraRefresh {
                    gop_frames: 120,
                    ctbs_per_slot: ctbs,
                    fallback_ctbs_per_slot: fallback,
                })
            );
            let properties = amf(&Config::default(), &stream).unwrap();
            assert!(properties.iter().any(|p| {
                p.name == "HevcIntraRefreshCTBsNumberPerSlot" && p.value == Value::Integer(fallback)
            }));
            assert!(
                properties
                    .iter()
                    .any(|p| { p.name == "HevcMaxNumRefFrames" && p.value == Value::Integer(1) })
            );
            assert!(!properties.iter().any(|p| p.name == "HevcGOPSPerIDR"));
            assert_eq!(amf_ltr_frames(&Config::default(), &stream), 0);
        }
    }
}

#[test]
fn hevc_intra_refresh_override_requires_a_valid_negotiated_hevc_stream() {
    for codec in 0..=3 {
        for requested in [false, true] {
            let stream = Negotiated {
                codec,
                intra_refresh: requested,
                ..Default::default()
            };
            assert_eq!(
                amf_hevc_intra_refresh(&stream).is_some(),
                codec == 1 && requested
            );
        }
    }
    for (width, height) in [(0, 2160), (3840, 0), (0, 0)] {
        assert!(
            amf_hevc_intra_refresh(&Negotiated {
                width,
                height,
                codec: 1,
                intra_refresh: true,
                ..Default::default()
            })
            .is_none()
        );
    }
    let stream = Negotiated {
        width: 3840,
        height: 2160,
        intra_refresh: true,
        ..Default::default()
    };
    let properties = amf(&Config::default(), &stream).unwrap();
    assert!(
        properties.iter().any(|p| {
            p.name == "IntraRefreshMBsNumberPerSlot" && p.value == Value::Integer(109)
        })
    );
    assert!(!properties.iter().any(|p| p.name.starts_with("Hevc")));
}

#[test]
fn av1_that_asked_for_intra_refresh_is_configured_as_a_stream_that_never_asked() {
    for references in [0, 1, 2, 5] {
        let asked = Negotiated {
            codec: 2,
            width: 2560,
            height: 1440,
            references,
            intra_refresh: true,
            ..Default::default()
        };
        let never_asked = Negotiated {
            intra_refresh: false,
            ..asked.clone()
        };
        assert!(!amf_intra_refresh(&asked));
        assert!(amf_hevc_intra_refresh(&asked).is_none());
        let config = Config::default();
        let properties = |stream| {
            amf(&config, stream)
                .unwrap()
                .into_iter()
                .map(|p| (p.name, p.value, p.required))
                .collect::<Vec<_>>()
        };
        assert!(
            !properties(&asked)
                .iter()
                .any(|p| p.0.contains("IntraRefresh"))
        );
        assert_eq!(properties(&asked), properties(&never_asked));
        // Loss recovery keeps its long-term references for invalidation.
        assert_eq!(
            amf_ltr_frames(&config, &asked),
            amf_ltr_frames(&config, &never_asked)
        );
        assert_eq!(
            amf_ltr_frames(&config, &asked) > 0,
            references != 1,
            "{references}"
        );
    }
}

#[test]
fn slice_delivery_is_off_unless_two_or_four_blocks_are_asked_for_on_avc_or_hevc() {
    let set = |value: &str| Config::parse(&format!("video_slice_delivery={value}\n")).unwrap();
    assert_eq!(slice_delivery(&Config::default(), 0), 0);
    for (value, blocks) in [
        ("off", 0),
        ("0", 0),
        ("2", 2),
        ("4", 4),
        ("3", 0),
        ("on", 0),
    ] {
        for codec in [0, 1] {
            assert_eq!(slice_delivery(&set(value), codec), blocks, "{value}");
        }
        assert_eq!(slice_delivery(&set(value), 2), 0, "AV1 is never sliced");
    }
}

#[test]
fn slice_delivery_asks_for_slice_output_with_at_least_one_slice_per_block() {
    let find = |properties: &[Property], name: &str| {
        properties
            .iter()
            .find(|p| p.name == name)
            .map(|p| (p.value, p.required))
    };
    let on = Config::parse("video_slice_delivery=4\n").unwrap();
    for (codec, prefix) in [(0, ""), (1, "Hevc")] {
        let slices = format!("{prefix}SlicesPerFrame");
        let mode = format!("{prefix}OutputMode");
        for (client, expected) in [(1, 4), (2, 4), (4, 4), (8, 8)] {
            let stream = Negotiated {
                codec,
                slices: client,
                ..Default::default()
            };
            let properties = amf(&on, &stream).unwrap();
            assert_eq!(find(&properties, &mode), Some((Value::Integer(1), true)));
            assert_eq!(
                find(&properties, &slices),
                Some((Value::Integer(expected), true))
            );
            let off = amf(&Config::default(), &stream).unwrap();
            assert_eq!(find(&off, &mode), None, "frame output is the default");
        }
    }
    let av1 = Negotiated {
        codec: 2,
        slices: 4,
        ..Default::default()
    };
    let properties = amf(&on, &av1).unwrap();
    assert!(properties.iter().all(|p| !p.name.contains("OutputMode")));
}

#[test]
fn slice_delivery_turns_intra_refresh_off() {
    let on = Config::parse("video_slice_delivery=2\n").unwrap();
    for codec in [0, 1] {
        let stream = Negotiated {
            codec,
            intra_refresh: true,
            references: 2,
            ..Default::default()
        };
        let refresh =
            |properties: &[Property]| properties.iter().any(|p| p.name.contains("IntraRefresh"));
        assert!(refresh(&amf(&Config::default(), &stream).unwrap()));
        assert!(!refresh(&amf(&on, &stream).unwrap()));
    }
}

#[test]
fn client_slice_requests_are_best_effort_but_configured_tiles_are_required() {
    let find = |properties: &[Property], name: &str| {
        properties
            .iter()
            .find(|p| p.name == name)
            .map(|p| (p.value, p.required))
    };
    for (codec, name) in [(0, "SlicesPerFrame"), (1, "HevcSlicesPerFrame")] {
        let stream = Negotiated {
            codec,
            slices: 4,
            ..Default::default()
        };
        let properties = amf(&Config::default(), &stream).unwrap();
        assert_eq!(find(&properties, name), Some((Value::Integer(4), false)));
        let single = Negotiated {
            codec,
            slices: 1,
            ..Default::default()
        };
        let properties = amf(&Config::default(), &single).unwrap();
        assert_eq!(find(&properties, name), None);
    }
    let av1 = Negotiated {
        codec: 2,
        slices: 4,
        ..Default::default()
    };
    let properties = amf(&Config::default(), &av1).unwrap();
    assert_eq!(
        find(&properties, "Av1NumTilesPerFrame"),
        Some((Value::Integer(4), false))
    );
    let configured = Config::parse("amd_av1_tiles=2\n").unwrap();
    let properties = amf(&configured, &av1).unwrap();
    assert_eq!(
        find(&properties, "Av1NumTilesPerFrame"),
        Some((Value::Integer(2), true))
    );
}

#[test]
fn one_reference_avc_uses_idr_recovery_without_ltr_marks() {
    let config = Config::parse("amd_ltr_frames=4\n").unwrap();
    let stream = Negotiated {
        references: 1,
        ..Default::default()
    };
    let mut recovery = crate::ltr::References::new(
        amf_ltr_frames(&config, &stream),
        crate::ltr::Counter::for_codec(stream.codec),
    );
    for frame in 1..=64 {
        let plan = recovery.plan(frame, frame == 1);
        assert!(plan.mark.is_none());
        assert!(plan.reference.is_none());
        recovery.accepted(frame, &plan);
    }
    assert!(!recovery.enabled());
    assert!(!recovery.invalidate(60, 61));
}

#[test]
fn amf_ltr_preserves_unrestricted_clients_and_explicit_disable() {
    let stream = Negotiated {
        codec: 2,
        ..Default::default()
    };
    for (requested, expected) in [(-1, 0), (0, 0), (1, 1), (4, 4), (99, 4)] {
        let config = Config::parse(&format!("amd_ltr_frames={requested}\n")).unwrap();
        assert_eq!(amf_ltr_frames(&config, &stream), expected);
        // AV1 does not refresh, so asking for it keeps the anchors.
        assert_eq!(
            amf_ltr_frames(
                &config,
                &Negotiated {
                    intra_refresh: true,
                    ..stream.clone()
                }
            ),
            expected
        );
    }
}

#[test]
fn amf_split_frame_hint_follows_the_setting_and_the_engine_count() {
    let request = |text: &str, codec, instances, current| {
        amf_split_frame(&Config::parse(text).unwrap(), codec, instances, current).map(|property| {
            assert!(!property.required);
            (property.name, property.value)
        })
    };
    let on = |name: &str| Some((name.to_owned(), Value::Boolean(true)));
    let off = |name: &str| Some((name.to_owned(), Value::Boolean(false)));
    for (codec, name) in [
        (1, "HevcMultiHwInstanceEncode"),
        (2, "Av1MultiHwInstanceEncode"),
    ] {
        // Auto asks only where the driver left it off or cannot say.
        for text in ["", "amd_split_frame=auto\n"] {
            assert_eq!(request(text, codec, 2, Some(true)), None);
            assert_eq!(request(text, codec, 2, Some(false)), on(name));
            assert_eq!(request(text, codec, 2, None), on(name));
        }
        for current in [Some(true), Some(false), None] {
            assert_eq!(
                request("amd_split_frame=enabled\n", codec, 2, current),
                on(name)
            );
            assert_eq!(
                request("amd_split_frame=disabled\n", codec, 2, current),
                off(name)
            );
            // One engine, as the RX 9070 XT's HEVC, or no readable cap.
            for instances in [-1, 0, 1] {
                for text in [
                    "",
                    "amd_split_frame=enabled\n",
                    "amd_split_frame=disabled\n",
                ] {
                    assert_eq!(request(text, codec, instances, current), None);
                }
            }
        }
    }
    // H.264 has no multi-instance property.
    for text in [
        "",
        "amd_split_frame=enabled\n",
        "amd_split_frame=disabled\n",
    ] {
        assert_eq!(request(text, 0, 2, None), None);
    }
    assert_eq!(amf_multi_instance(0), None);
    assert_eq!(
        amf_multi_instance(1),
        Some(("HevcNumOfHwInstances", "HevcMultiHwInstanceEncode"))
    );
    assert_eq!(
        amf_multi_instance(2),
        Some(("Av1CapNumOfHwInstances", "Av1MultiHwInstanceEncode"))
    );
    // The encoder properties written for every stream never include it.
    for codec in 0..=2 {
        let stream = Negotiated {
            codec,
            ..Default::default()
        };
        let config = Config::parse("amd_split_frame=enabled\n").unwrap();
        assert!(
            !amf(&config, &stream)
                .unwrap()
                .iter()
                .any(|property| property.name.contains("MultiHwInstance"))
        );
    }
}
#[test]
fn imported_amd_encoder_names_select_amf_and_others_select_automatically() {
    for name in [
        "amdvce",
        "amdvce_experimental",
        "amdvce_ffmpeg",
        "amdvce_legacy",
        "amf",
    ] {
        let config = Config::parse(&format!("encoder={name}\n")).unwrap();
        assert_eq!(canonical_name(config.get("encoder", "auto")), "amf");
        assert_eq!(config.get("encoder", "auto"), name);
    }
    for name in ["", "auto"] {
        assert_eq!(canonical_name(name), name);
    }
    for unusable in [
        "mediafoundation",
        "unknown",
        "pyrowave",
        "nvenc",
        "nvenc_experimental",
        "nvenc_legacy",
        "quicksync",
        "qsv",
        "software",
    ] {
        assert_eq!(canonical_name(unusable), "auto");
    }
}
#[test]
fn legacy_codec_specific_settings_and_auto_are_preserved() {
    let config = Config::parse("amd_usage=lowlatency\namd_quality=balanced\namd_rc=qvbr\namd_qvbr_quality_level=20\namd_vbaq=disabled\namd_av1_latency_mode=lowest\n").unwrap();
    let mut stream = Negotiated::default();
    let h264 = amf(&config, &stream).unwrap();
    assert!(
        h264.iter()
            .any(|p| p.name == "Usage" && p.value == Value::Integer(2))
    );
    assert!(
        h264.iter()
            .any(|p| p.name == "EnablePreAnalysis" && p.value == Value::Boolean(true))
    );
    stream.codec = 2;
    let av1 = amf(&config, &stream).unwrap();
    assert!(
        av1.iter()
            .any(|p| p.name == "Av1Usage" && p.value == Value::Integer(1))
    );
    assert!(
        av1.iter()
            .any(|p| p.name == "Av1AQMode" && p.value == Value::Integer(0))
    );
    assert!(
        av1.iter()
            .any(|p| p.name == "Av1EncodingLatencyMode" && p.value == Value::Integer(3))
    );
    // Adaptive quantization defaults on for HEVC and AV1, off for H.264.
    for (codec, name, on) in [(0, "EnableVBAQ", false), (1, "HevcEnableVBAQ", true)] {
        stream.codec = codec;
        assert!(
            amf(&Config::default(), &stream)
                .unwrap()
                .iter()
                .any(|p| p.name == name && p.value == Value::Boolean(on))
        );
    }
    stream.codec = 2;
    assert!(
        amf(&Config::default(), &stream)
            .unwrap()
            .iter()
            .any(|p| p.name == "Av1AQMode" && p.value == Value::Integer(1))
    );
    // AV1 entropy-context update stays the driver's unless set.
    assert!(
        !amf(&Config::default(), &stream)
            .unwrap()
            .iter()
            .any(|p| p.name.starts_with("Av1Cdf"))
    );
    let cdf = amf(
        &Config::parse("amd_av1_cdf_update=disabled\namd_av1_cdf_frame_end=1\n").unwrap(),
        &stream,
    )
    .unwrap();
    assert!(
        cdf.iter()
            .any(|p| p.name == "Av1CdfUpdate" && p.value == Value::Boolean(false))
    );
    assert!(
        cdf.iter()
            .any(|p| p.name == "Av1CdfFrameEndUpdateMode" && p.value == Value::Integer(1))
    );
    let auto = amf(
        &Config::parse("amd_quality=auto\namd_vbaq=auto\n").unwrap(),
        &stream,
    )
    .unwrap();
    assert!(
        !auto
            .iter()
            .any(|p| p.name.ends_with("QualityPreset") || p.name == "Av1AQMode")
    );
    for codec in 0..=2 {
        stream.codec = codec;
        for (mode, expected) in [("high_quality", 4), ("lowlatency_high_quality", 5)] {
            let properties = amf(
                &Config::parse(&format!("amd_usage={mode}\n")).unwrap(),
                &stream,
            )
            .unwrap();
            assert!(
                properties
                    .iter()
                    .any(|p| p.name.ends_with("Usage") && p.value == Value::Integer(expected))
            );
        }
    }
}
