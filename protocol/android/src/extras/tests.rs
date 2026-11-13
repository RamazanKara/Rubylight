use super::*;

#[test]
fn passthrough_keeps_only_the_codecs_java_can_route() {
    use rubylight_protocol::audio_ext::{CODEC_AC3, CODEC_EAC3, CODEC_OPUS, CODEC_PCM};
    assert_eq!(audio_passthrough(0), 0);
    assert_eq!(audio_passthrough(i32::from(CODEC_AC3 | CODEC_EAC3)), CODEC_AC3 | CODEC_EAC3);
    assert_eq!(audio_passthrough(i32::from(CODEC_OPUS | CODEC_PCM)), CODEC_PCM);
    assert_eq!(audio_passthrough(-1), CODEC_PCM | CODEC_AC3 | CODEC_EAC3);
    assert_eq!(audio_passthrough(0x100), 0);
}

#[test]
fn the_video_fec_choice_comes_back_from_its_summary_percentage() {
    use rubylight_protocol::fec_report::FecChoice;
    for choice in [FecChoice::Host, FecChoice::Auto, FecChoice::Fixed(0), FecChoice::Fixed(30), FecChoice::Fixed(100)] {
        assert_eq!(video_fec(choice.summary_percentage()), choice);
    }
    assert_eq!(video_fec(i32::MIN), FecChoice::Host);
    assert_eq!(video_fec(255), FecChoice::Fixed(100));
}

#[test]
fn the_roaming_settings_reach_the_session_with_platform_hints() {
    for (enabled, metered) in [(true, false), (true, true), (false, false), (false, true)] {
        assert_eq!(
            roaming_config(enabled, metered),
            RoamingConfig {
                enabled,
                metered,
                platform_hints: true,
            }
        );
    }
}

#[test]
fn text_input_and_layout_messages_reach_the_wire() {
    let on = text_input_request(true).payload();
    assert_eq!(
        TextInputRequest::decode(&on),
        Some(TextInputRequest {
            notices: true,
            surrounding_text: false,
            element_ids: true,
            max_text: 0,
        })
    );
    assert!(
        !TextInputRequest::decode(&text_input_request(false).payload())
            .unwrap()
            .notices
    );

    let german = keyboard_layout_message("de-DE").unwrap();
    assert_eq!(
        KeyboardLayout::decode(&german.payload()),
        Some(KeyboardLayout {
            source: LayoutSource::Bcp47,
            klid: 0,
            tag: "de-DE".into(),
        })
    );
    // Nothing to report, or a tag the wire refuses, sends nothing.
    assert_eq!(keyboard_layout_message(""), None);
    assert_eq!(keyboard_layout_message(&"a".repeat(65)), None);
    assert_eq!(keyboard_layout_message("de DE"), None);
}

#[test]
fn rate_changes_go_out_as_asked_and_refuse_what_the_host_would() {
    let both = rate_change_message(-2, 40_000, 120_000).unwrap();
    assert_eq!(
        RateChangeRequest::decode(&both.payload()),
        Some(RateChangeRequest {
            serial: u32::MAX - 1,
            bitrate_kbps: 40_000,
            fps_millihz: 120_000,
        })
    );
    assert!(rate_change_message(1, 40_000, 0).is_some());
    assert!(rate_change_message(1, 0, 60_000).is_some());
    assert_eq!(rate_change_message(1, 0, 0), None);
    assert_eq!(rate_change_message(1, -1, 60_000), None);
    assert_eq!(rate_change_message(1, 40_000, -1), None);
    assert_eq!(rate_change_message(1, 40_000, 9_999), None);
    assert_eq!(rate_change_message(1, 0, 500_001), None);
}

#[test]
fn audio_configurations_round_trip() {
    assert_eq!(
        audio_configuration_to_int(AudioConfiguration::STEREO),
        0x0003_02CA
    );
    assert_eq!(
        audio_configuration_to_int(AudioConfiguration::SURROUND_71),
        0x063F_08CA
    );
    assert_eq!(
        audio_configuration_from_int(0x003F_06CA),
        Some(AudioConfiguration::SURROUND_51)
    );
    assert_eq!(audio_configuration_from_int(0x0003_0200), None);
    // 7.1.4's mask sets the sign bit, as Java's MoonBridge.AUDIO_CONFIGURATION_714_SURROUND.
    let seven_one_four = audio_configuration_to_int(AudioConfiguration::SURROUND_714);
    assert_eq!(seven_one_four as u32, 0xF63F_0CCA);
    assert_eq!(
        audio_configuration_from_int(seven_one_four),
        Some(AudioConfiguration::SURROUND_714)
    );
}

#[test]
fn input_errors_map_to_the_c_codes() {
    assert_eq!(input_result(Ok(())), 0);
    assert_eq!(input_result(Err(InputError::Unsupported)), -5501);
    assert_eq!(input_result(Err(InputError::Stopped)), -2);
    assert_eq!(input_result(Err(InputError::Invalid)), -3);
}

#[test]
fn hdr_metadata_has_the_c_layout() {
    let m = HdrMetadata {
        display_primaries: [(1, 2), (3, 4), (5, 6)],
        white_point: (7, 8),
        max_display_luminance: 9,
        min_display_luminance: 10,
        max_content_light_level: 11,
        max_frame_average_light_level: 12,
        max_full_frame_luminance: 0x0d0e,
    };
    let bytes = hdr_metadata_bytes(&m);
    assert_eq!(&bytes[..4], &[1, 0, 2, 0]);
    assert_eq!(&bytes[12..16], &[7, 0, 8, 0]);
    assert_eq!(&bytes[24..], &[0x0e, 0x0d]);
}

#[test]
fn cursor_shapes_encode_back_to_the_wire_form() {
    let shapes = [
        CursorShape::Hide,
        CursorShape::Default,
        CursorShape::Show { id: 7 },
        CursorShape::Image {
            id: 9,
            width: 2,
            height: 1,
            hotspot_x: 1,
            hotspot_y: 0,
            scale_percent: 150,
            rgba: vec![0xab; 8],
        },
    ];
    for shape in shapes {
        let message = cursor_shape_message(&shape);
        assert_eq!(CursorShape::decode(&message), Ok(shape));
    }
}

#[test]
fn rtt_info_packs_like_the_c() {
    assert_eq!(rtt_info(None), -1);
    let packed = rtt_info(Some((Duration::from_millis(12), Duration::from_millis(3))));
    assert_eq!(packed, (12 << 32) | 3);
}

#[test]
fn receive_buffer_info_packs_asked_over_granted() {
    assert_eq!(receive_buffer_info(None), -1);
    let packed = receive_buffer_info(Some(ReceiveBuffer {
        asked: 16 << 20,
        granted: 212_992,
    }));
    assert_eq!(packed >> 32, 16 << 20);
    assert_eq!(packed & 0xffff_ffff, 212_992);
    let huge = receive_buffer_info(Some(ReceiveBuffer {
        asked: usize::MAX,
        granted: usize::MAX,
    }));
    assert!(huge > 0, "never the -1 of no session");
}

#[test]
fn display_caps_scale_like_the_ffi() {
    let caps = display_caps(true, 1000.0, 400.0, 0.0005);
    assert_eq!(
        (
            caps.hdr,
            caps.max_centinits,
            caps.max_average_centinits,
            caps.min_decimillinits
        ),
        (true, 100_000, 40_000, 5)
    );
    assert_eq!(display_caps(false, f32::NAN, -1.0, 0.0).max_centinits, 0);
}

#[test]
fn controller_audio_is_checked() {
    assert_eq!(
        controller_audio_message(0x5535, &[1, 1, 3, 0, 5, 0, 0, 0]),
        Some(ClientMessage::ControllerAudioStatus {
            controller: 3,
            flags: 5
        })
    );
    assert!(controller_audio_message(0x5535, &[1, 1, 16, 0, 5, 0, 0, 0]).is_none());
    assert!(controller_audio_message(0x5535, &[1, 1, 3, 0, 5, 0, 0, 1]).is_none());
    let pcm = nightjar_core::control::haptics::encode(2, 1, &[0; 8]).unwrap();
    assert!(controller_audio_message(0x5534, &pcm).is_some());
    assert!(controller_audio_message(0x5534, &pcm[..pcm.len() - 1]).is_none());
    assert!(controller_audio_message(0x5533, &pcm).is_none());
}

#[test]
fn rtp_timestamps_survive_the_presentation_time() {
    for rtp in [0u32, 1, 89, 90, 12_345_678, u32::MAX] {
        let us = u64::from(rtp) * 1000 / 90;
        assert_eq!(rtp_from_presentation_us(us), rtp);
    }
}

#[test]
fn phase_lock_reports_every_half_second() {
    let mut reports = PhaseLockReports::default();
    assert_eq!(reports.lead_us(), i32::MIN);
    let mut ready = false;
    for frame in 0..60u64 {
        ready |= reports.push(2_000_000, 16_666_667, 1000 + frame * 10);
    }
    assert!(ready);
    assert!(reports.lead_us() != i32::MIN);
    assert!(reports.take(1600).is_some());
    assert!(reports.take(1600).is_none());
    // Too few frames in a window make no report.
    let mut sparse = PhaseLockReports::default();
    assert!(!sparse.push(1, 16_666_667, 1));
    assert!(!sparse.push(1, 16_666_667, 600));
    // A stale report is dropped.
    let mut stale = PhaseLockReports::default();
    for frame in 0..60u64 {
        stale.push(2_000_000, 16_666_667, 1 + frame * 10);
    }
    assert!(stale.take(5000).is_none());
}

#[test]
fn phase_lock_asks_for_more_room_at_one_frame_per_refresh() {
    // 60 fps on a 60 Hz screen: half a second holds 30 frames, one per refresh.
    let lead = |frame_ms: u64, period_ns: i64| {
        let mut reports = PhaseLockReports::default();
        let mut frame = 0;
        while !reports.push(2_000_000 + (frame % 10) as i64 * 400_000, period_ns, 1000 + frame * frame_ms) {
            frame += 1;
        }
        reports.lead_us()
    };
    let sixty = lead(16, 16_666_667);
    // The same slack, one frame per two refreshes of a 120 Hz screen: the plain edge.
    let half = lead(16, 8_333_333);
    assert_eq!(half, 2_000, "the 10th percentile");
    assert!(sixty < half, "{sixty} us at 60 Hz, {half} us at 120 Hz");
}

#[test]
fn microphone_frames_are_opus_sizes() {
    assert!(valid_microphone_frame(960));
    assert!(!valid_microphone_frame(1000));
}

#[test]
fn the_grid_is_converted_from_nanoseconds() {
    let grid = grid_from_ns(5_000_123_456, 16_666_667, 4_000_000, 8_333_333);
    assert_eq!(grid.anchor_us, 5_000_123);
    assert_eq!(grid.interval_us, 16_667, "rounded to the nearest microsecond");
    assert_eq!(grid.lead_us, 4_000);
    assert_eq!(grid.slack_us, 8_333);
    assert_eq!(grid_from_ns(0, 8_333_333, 0, 0).interval_us, 8_333);
    assert_eq!(grid_from_ns(0, 8_333_500, 0, 0).interval_us, 8_334);
}

#[test]
fn a_grid_schedule_is_reported_in_nanoseconds() {
    use rubylight_protocol::present_clock::PresentClock;
    let mut clock = PresentClock::default();
    let (anchor_ns, interval_ns, lead_ns) = (10_000_000_000i64, 16_666_667i64, 4_000_000i64);
    let grid = grid_from_ns(anchor_ns, interval_ns, lead_ns, 0);
    // Ready 8 ms after the anchor's vsync: the next vsync is 8.67 ms away, release 4.67 ms
    // after ready.
    let ready_ns = anchor_ns + 8_000_000;
    let s = clock.schedule_on_grid(0, (ready_ns / 1000) as u64, grid);
    let out = grid_schedule_to_ns(&s, ready_ns, anchor_ns, interval_ns, lead_ns);
    assert_eq!(out[1], anchor_ns + interval_ns, "the next vsync");
    assert_eq!(out[2], 1);
    assert_eq!(out[0], out[1] - lead_ns, "released a lead before it");
    assert_eq!(out[3], 0);
    assert!((out[4] - (out[0] - ready_ns) / 1000).abs() <= 1, "the hold, to rounding");
    assert_eq!(out[5], -1, "no clock sync");
    assert_eq!(out[6], 0);
    // Ready 2 ms before the next vsync, inside the lead: released at the ready time, to the
    // nanosecond.
    let ready_ns = anchor_ns + 2 * interval_ns - 2_000_123;
    let grid = grid_from_ns(anchor_ns + interval_ns, interval_ns, lead_ns, 0);
    let s = clock.schedule_on_grid(1_500, (ready_ns / 1000) as u64, grid);
    let out = grid_schedule_to_ns(&s, ready_ns, anchor_ns + interval_ns, interval_ns, lead_ns);
    assert_eq!(out[0], ready_ns);
    assert_eq!(out[4], 0);
}

#[test]
fn grid_flags_carry_late_and_shared() {
    let s = GridSchedule {
        release_us: 5_000,
        vsync_us: 9_000,
        vsync_index: 2,
        late: true,
        late_by_us: 700,
        shares_vsync: true,
        hold_us: 100,
        latency_us: Some(33),
    };
    let out = grid_schedule_to_ns(&s, 4_900_000, 0, 4_500_000, 4_000_000);
    assert_eq!(out, [5_000_000, 9_000_000, 2, 3, 100, 33, 700]);
}
