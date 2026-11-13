use super::*;

#[test]
fn termination_codes_map_like_the_c() {
    assert_eq!(
        termination_error(&0x8003_0023u32.to_be_bytes(), true),
        Some(0)
    );
    assert_eq!(
        termination_error(&0x8003_0023u32.to_be_bytes(), false),
        Some(-102)
    );
    assert_eq!(
        termination_error(&0x800e_9403u32.to_be_bytes(), true),
        Some(-104)
    );
    assert_eq!(
        termination_error(&0x800e_9302u32.to_be_bytes(), true),
        Some(-103)
    );
    assert_eq!(
        termination_error(&0x8004_0001u32.to_be_bytes(), true),
        Some(0x8004_0001u32 as i32)
    );
    assert_eq!(termination_error(&[0x00, 0x01], true), Some(0));
    assert_eq!(termination_error(&[0x00, 0x01], false), Some(-102));
    assert_eq!(termination_error(&[0x05, 0x02], true), Some(0x0205));
    assert_eq!(termination_error(&[1], true), None);
}

#[test]
fn simple_host_messages_decode() {
    assert_eq!(
        rumble(&[0, 0, 0, 0, 1, 0, 0x34, 0x12, 0xff, 0xff]),
        Some(ControlEvent::Rumble {
            controller: 1,
            low_frequency: 0x1234,
            high_frequency: 0xffff
        })
    );
    assert_eq!(rumble(&[0; 9]), None);
    assert_eq!(
        rumble_triggers(&[2, 0, 1, 0, 2, 0]),
        Some(ControlEvent::RumbleTriggers {
            controller: 2,
            left: 1,
            right: 2
        })
    );
    assert_eq!(
        motion_event(&[3, 0, 200, 0, 2]),
        Some(ControlEvent::SetMotionEventState {
            controller: 3,
            motion_type: 2,
            report_rate_hz: 200
        })
    );
    assert_eq!(
        controller_led(&[1, 0, 10, 20, 30]),
        Some(ControlEvent::SetControllerLed {
            controller: 1,
            r: 10,
            g: 20,
            b: 30
        })
    );
    let mut triggers = vec![1, 0, 0x0c, 0x21, 0x26];
    triggers.extend(1..=20);
    let Some(ControlEvent::AdaptiveTriggers {
        left,
        right,
        event_flags,
        ..
    }) = adaptive_triggers(&triggers)
    else {
        panic!("adaptive triggers");
    };
    assert_eq!((left[0], right[9], event_flags), (1, 20, 0x0c));
    assert_eq!(adaptive_triggers(&triggers[..24]), None);
}

#[test]
fn hdr_metadata_reads_what_is_present() {
    let mut payload = vec![1];
    for word in 1..=13u16 {
        payload.extend_from_slice(&word.to_le_bytes());
    }
    let (enabled, metadata) = hdr_mode(&payload, true).unwrap();
    let metadata = metadata.unwrap();
    assert!(enabled);
    assert_eq!(metadata.display_primaries, [(1, 2), (3, 4), (5, 6)]);
    assert_eq!(metadata.white_point, (7, 8));
    assert_eq!(metadata.max_full_frame_luminance, 13);
    let (_, short) = hdr_mode(&payload[..9], true).unwrap();
    assert_eq!(short.unwrap().display_primaries[1], (3, 4));
    assert_eq!(short.unwrap().white_point, (0, 0));
    assert_eq!(hdr_mode(&[0], false), Some((false, None)));
    assert_eq!(hdr_mode(&[], true), None);
}

#[test]
fn cursor_shapes_follow_the_spec() {
    assert_eq!(
        CursorShape::decode(&[1, 0, 0, 0, 0, 0, 0, 0]),
        Ok(CursorShape::Hide)
    );
    assert_eq!(
        CursorShape::decode(&[1, 1, 0, 0, 0, 0, 0, 0]),
        Ok(CursorShape::Default)
    );
    assert_eq!(
        CursorShape::decode(&[1, 2, 0, 0, 5, 0, 0, 0]),
        Ok(CursorShape::Show { id: 5 })
    );
    assert!(CursorShape::decode(&[1, 2, 0, 0, 0, 0, 0, 0]).is_err());
    assert!(CursorShape::decode(&[1, 0, 0, 0, 1, 0, 0, 0]).is_err());
    assert!(CursorShape::decode(&[2, 0, 0, 0, 0, 0, 0, 0]).is_err());
    assert!(CursorShape::decode(&[1, 4, 0, 0, 0, 0, 0, 0]).is_err());

    let mut image = vec![1, 3, 0, 0, 9, 0, 0, 0];
    image.extend_from_slice(&[2, 0, 1, 0, 1, 0, 0, 0, 1, 0, 150, 0]);
    image.extend_from_slice(&[0xab; 8]);
    let Ok(CursorShape::Image {
        id,
        width,
        height,
        hotspot_x,
        scale_percent,
        rgba,
        ..
    }) = CursorShape::decode(&image)
    else {
        panic!("image");
    };
    assert_eq!(
        (id, width, height, hotspot_x, scale_percent, rgba.len()),
        (9, 2, 1, 1, 150, 8)
    );
    let mut hotspot = image.clone();
    hotspot[12] = 2;
    assert_eq!(
        CursorShape::decode(&hotspot),
        Err("hotspot outside the image")
    );
    assert_eq!(
        CursorShape::decode(&image[..image.len() - 1]),
        Err("wrong length")
    );
    assert!(matches!(cursor_shape(&image), ControlEvent::CursorShape(_)));
    assert!(matches!(
        cursor_shape(&[1, 0]),
        ControlEvent::CursorShapeRejected(_)
    ));
}

#[test]
fn coalesce_keeps_the_newest_of_a_run() {
    let rumble = |controller, low| ControlEvent::Rumble {
        controller,
        low_frequency: low,
        high_frequency: 0,
    };
    let mut events = vec![
        rumble(0, 1),
        rumble(0, 2),
        rumble(1, 3),
        rumble(1, 4),
        ControlEvent::HdrMode {
            enabled: true,
            metadata: None,
        },
        ControlEvent::HdrMode {
            enabled: false,
            metadata: None,
        },
        rumble(1, 5),
        ControlEvent::SetMotionEventState {
            controller: 0,
            motion_type: 1,
            report_rate_hz: 100,
        },
        ControlEvent::SetMotionEventState {
            controller: 0,
            motion_type: 1,
            report_rate_hz: 0,
        },
    ];
    coalesce(&mut events);
    assert_eq!(events.len(), 6);
    assert_eq!(events[0], rumble(0, 2));
    assert_eq!(events[1], rumble(1, 4));
    assert_eq!(
        events[2],
        ControlEvent::HdrMode {
            enabled: false,
            metadata: None
        }
    );
    assert_eq!(events[3], rumble(1, 5));
}

#[test]
fn steam_controller_messages_decode_and_are_never_coalesced() {
    use rubylight_protocol::steam_controller::STEAM_SETTINGS_SET_VALUES;
    let haptic = SteamHaptic::new(1, &[0x80, 0x10, 0x20]).unwrap();
    let event = steam_haptic(&haptic.encode());
    assert_eq!(event, ControlEvent::SteamHaptic(haptic));
    let settings = SteamSettings::new(1, STEAM_SETTINGS_SET_VALUES, &[[0x30, 1, 0]]).unwrap();
    assert_eq!(
        steam_settings(&settings.encode()),
        ControlEvent::SteamSettings(settings)
    );

    // Steam re-sends the same report to keep a buzz going; each one must reach the pads.
    let mut events = vec![event.clone(), event.clone(), event];
    coalesce(&mut events);
    assert_eq!(events.len(), 3);
}

#[test]
fn broken_steam_controller_messages_are_rejected_by_name() {
    let mut haptic = SteamHaptic::new(0, &[0x80]).unwrap().encode();
    haptic[2] = 13;
    assert_eq!(
        steam_haptic(&haptic),
        ControlEvent::SteamRejected {
            message_type: 0x5504,
            error: SteamError::ReportTooLong { len: 13 }
        }
    );
    let mut settings = [0u8; 31];
    settings[3] = 10;
    assert_eq!(
        steam_settings(&settings),
        ControlEvent::SteamRejected {
            message_type: 0x5505,
            error: SteamError::TooManyTriples { count: 10 }
        }
    );
    assert_eq!(
        steam_settings(&settings[..4]),
        ControlEvent::SteamRejected {
            message_type: 0x5505,
            error: SteamError::Short { len: 4 }
        }
    );
}

#[test]
fn empty_steam_haptic_report_is_rejected() {
    // Java's steamHaptic gets the report id first; an empty array has none.
    let mut haptic = SteamHaptic::new(3, &[0x80]).unwrap().encode();
    haptic[2] = 0;
    assert_eq!(
        steam_haptic(&haptic),
        ControlEvent::SteamRejected {
            message_type: 0x5504,
            error: SteamError::EmptyReport
        }
    );
}

#[test]
fn unknown_steam_settings_command_is_rejected() {
    let mut settings = [0u8; 31];
    settings[2] = 0x86;
    assert_eq!(
        steam_settings(&settings),
        ControlEvent::SteamRejected {
            message_type: 0x5505,
            error: SteamError::UnknownCommand { command: 0x86 }
        }
    );
}

#[test]
fn steam_load_defaults_with_triples_is_rejected() {
    let mut settings = [0u8; 31];
    settings[2] = 0x8e;
    settings[3] = 1;
    assert_eq!(
        steam_settings(&settings),
        ControlEvent::SteamRejected {
            message_type: 0x5505,
            error: SteamError::TriplesWithDefaults { count: 1 }
        }
    );
}
