#[test]
fn a_client_display_scale_is_one_windows_offers() {
    use super::{DISPLAY_SCALE_PARAMETER, client_display_scale};
    let options = |value: &str| {
        std::collections::BTreeMap::from([(DISPLAY_SCALE_PARAMETER.to_owned(), value.to_owned())])
    };
    assert_eq!(client_display_scale(&Default::default()).unwrap(), None);
    assert_eq!(client_display_scale(&options("150")).unwrap(), Some(150));
    assert_eq!(client_display_scale(&options(" 500 ")).unwrap(), Some(500));
    for refused in ["0", "275", "-1", "600", "big", ""] {
        assert!(
            client_display_scale(&options(refused)).is_err(),
            "{refused}"
        );
    }
    // The render scale is a separate option and never reads as a display scale.
    let render = std::collections::BTreeMap::from([("scaleFactor".to_owned(), "200".to_owned())]);
    assert_eq!(client_display_scale(&render).unwrap(), None);
}

#[test]
fn a_kept_resolution_is_width_by_height() {
    use super::parse_resolution;
    assert_eq!(parse_resolution("2560x1440"), Some((2560, 1440)));
    assert_eq!(parse_resolution(" 1920X1080 "), Some((1920, 1080)));
    assert_eq!(parse_resolution("2560x1440x120"), None);
    assert_eq!(parse_resolution("0x1440"), None);
    assert_eq!(parse_resolution(""), None);
}

#[test]
fn rotations_are_displayconfig_codes_one_to_four() {
    use super::{rotation_code, rotation_degrees};
    for (code, degrees) in [(1, 0), (2, 90), (3, 180), (4, 270)] {
        assert_eq!(rotation_degrees(code), Some(degrees));
        assert_eq!(rotation_code(degrees), Some(code));
    }
    assert_eq!(rotation_degrees(0), None);
    assert_eq!(rotation_degrees(5), None);
    assert_eq!(rotation_code(45), None);
    assert_eq!(rotation_code(360), None);
}

#[test]
fn a_physical_display_turns_to_the_chosen_angle_or_the_streams_shape() {
    use super::physical_rotation as turn;
    use crate::config::DisplayAdjust::{Auto, Disabled, Manual};
    let landscape = (2560, 1440);
    let portrait = (1440, 2560);
    let phone = (1080, 2400);
    assert_eq!(turn(Disabled, 90, 0, landscape, phone), None);
    // A chosen angle, unless the display has it already.
    assert_eq!(turn(Manual, 90, 0, landscape, (1920, 1080)), Some(90));
    assert_eq!(turn(Manual, 0, 270, portrait, phone), Some(0));
    assert_eq!(turn(Manual, 180, 180, landscape, phone), None);
    // A portrait phone turns a landscape display, a landscape stream turns
    // it back; one already in the stream's shape stays as it is.
    assert_eq!(turn(Auto, 0, 0, landscape, phone), Some(90));
    assert_eq!(turn(Auto, 0, 180, landscape, phone), Some(90));
    assert_eq!(turn(Auto, 0, 270, portrait, (2400, 1080)), Some(0));
    assert_eq!(turn(Auto, 0, 270, portrait, phone), None);
    assert_eq!(turn(Auto, 0, 0, landscape, (1920, 1080)), None);
    // A monitor that is portrait upright turns to landscape for a landscape stream.
    assert_eq!(turn(Auto, 0, 0, portrait, (1920, 1080)), Some(90));
    // A square has no shape to follow.
    assert_eq!(turn(Auto, 0, 0, landscape, (1024, 1024)), None);
    assert_eq!(turn(Auto, 0, 0, (1024, 1024), phone), None);
}

#[test]
fn a_turned_physical_display_gets_its_mode_in_its_shape_before_the_turn() {
    use super::mode_before_turn;
    // A portrait phone's mode on a landscape display, which then turns to portrait.
    assert_eq!(mode_before_turn((1080, 2400), (2560, 1440)), (2400, 1080));
    assert_eq!(mode_before_turn((1920, 1080), (2560, 1440)), (1920, 1080));
    assert_eq!(mode_before_turn((1920, 1080), (1440, 2560)), (1080, 1920));
    assert_eq!(mode_before_turn((1024, 1024), (1440, 2560)), (1024, 1024));
}

#[test]
fn a_virtual_display_is_shaped_rather_than_turned() {
    use super::virtual_shape as shape;
    use crate::config::DisplayAdjust::{Auto, Disabled, Manual};
    let phone = (1080, 2400);
    assert_eq!(shape(Disabled, 90, (2560, 1440), phone), (2560, 1440));
    assert_eq!(shape(Manual, 90, (1920, 1080), (1920, 1080)), (1080, 1920));
    assert_eq!(shape(Manual, 270, (1080, 2400), phone), (2400, 1080));
    assert_eq!(shape(Manual, 180, (1920, 1080), (1920, 1080)), (1920, 1080));
    assert_eq!(shape(Manual, 0, (1920, 1080), phone), (1920, 1080));
    // A device's landscape display mode takes a portrait phone's shape.
    assert_eq!(shape(Auto, 0, (2560, 1440), phone), (1440, 2560));
    assert_eq!(shape(Auto, 0, phone, phone), phone);
    assert_eq!(shape(Auto, 0, (1440, 2560), (1920, 1080)), (2560, 1440));
    assert_eq!(shape(Auto, 0, (2560, 1440), (1024, 1024)), (2560, 1440));
}

#[test]
fn a_layout_chosen_during_the_stream_is_kept_for_the_device() {
    use super::chosen_layout;
    let on = |list: &[(&str, bool)]| -> Vec<(String, bool)> {
        list.iter().map(|(n, p)| (n.to_string(), *p)).collect()
    };
    let vd = r"\\.\DISPLAY9";
    // Issue #33: extended, then Windows' "Show only on 2".
    assert_eq!(
        chosen_layout("extended", vd, &on(&[(vd, true)])),
        Some("exclusive")
    );
    // Made the main display while extended.
    assert_eq!(
        chosen_layout("extended", vd, &on(&[(vd, true), (r"\\.\DISPLAY1", false)])),
        Some("extended_primary")
    );
    assert_eq!(
        chosen_layout(
            "extended_primary",
            vd,
            &on(&[(vd, false), (r"\\.\DISPLAY1", true)])
        ),
        Some("extended")
    );
    // Unchanged, the stream's display off ("Show only on 1"), an exclusive
    // stream (kept by the host itself) and isolated layouts learn nothing.
    assert_eq!(
        chosen_layout("extended", vd, &on(&[(vd, false), (r"\\.\DISPLAY1", true)])),
        None
    );
    assert_eq!(
        chosen_layout("extended", vd, &on(&[(r"\\.\DISPLAY1", true)])),
        None
    );
    assert_eq!(
        chosen_layout(
            "exclusive",
            vd,
            &on(&[(vd, false), (r"\\.\DISPLAY1", true)])
        ),
        None
    );
    assert_eq!(
        chosen_layout("extended_isolated", vd, &on(&[(vd, true)])),
        None
    );
    // Windows names displays in either case.
    assert_eq!(
        chosen_layout("extended", r"\\.\display9", &on(&[(vd, true)])),
        Some("exclusive")
    );
}

#[test]
fn virtual_opt_in_preserves_host_policy_and_device_force_overrides_physical_output() {
    use super::VirtualDisplayRequest;
    let host = VirtualDisplayRequest {
        configured: true,
        ..Default::default()
    };
    assert!(host.uses_virtual(false, || true));
    let physical_app = VirtualDisplayRequest {
        output_override: Some("physical-monitor"),
        ..host
    };
    assert!(!physical_app.requested());
    let forced = VirtualDisplayRequest {
        client_forced: true,
        ..physical_app
    };
    assert!(forced.uses_virtual(false, || true));
    let opt_in = VirtualDisplayRequest {
        client_requested: true,
        client_forced: false,
        ..forced
    };
    assert!(opt_in.uses_virtual(false, || true));
    assert!(!opt_in.uses_virtual(true, || panic!(
        "physical fallback must not query the driver"
    )));
}
#[test]
fn a_requested_virtual_display_without_its_driver_streams_the_physical_display() {
    use super::VirtualDisplayRequest;
    let client = VirtualDisplayRequest {
        client_requested: true,
        ..Default::default()
    };
    assert!(client.uses_virtual(false, || true));
    assert!(!client.uses_virtual(false, || false));
    assert!(!client.uses_virtual(true, || true));
    for request in [
        VirtualDisplayRequest {
            client_forced: true,
            ..Default::default()
        },
        VirtualDisplayRequest {
            app_requested: true,
            ..Default::default()
        },
        VirtualDisplayRequest {
            configured: true,
            ..Default::default()
        },
    ] {
        assert!(request.uses_virtual(false, || true));
        assert!(!request.uses_virtual(false, || false));
    }
    let named = VirtualDisplayRequest {
        configured_output: "virtual_display",
        ..Default::default()
    };
    assert!(named.uses_virtual(false, || false));
    let none = VirtualDisplayRequest::default();
    assert!(!none.uses_virtual(false, || panic!("no request needs no driver check")));
    // With no active display there is nothing physical to stream, even
    // when the settings and the app name a physical display.
    let headless = VirtualDisplayRequest {
        headless: true,
        output_override: Some("physical-monitor"),
        ..Default::default()
    };
    assert!(headless.uses_virtual(false, || true));
    assert!(!headless.uses_virtual(false, || false));
    assert!(!headless.uses_virtual(true, || true));
}
#[test]
fn a_client_asking_for_the_physical_display_overrides_the_virtual_display_settings() {
    use super::VirtualDisplayRequest;
    for request in [
        VirtualDisplayRequest {
            configured: true,
            ..Default::default()
        },
        VirtualDisplayRequest {
            app_requested: true,
            ..Default::default()
        },
        VirtualDisplayRequest {
            client_forced: true,
            ..Default::default()
        },
        VirtualDisplayRequest {
            client_requested: true,
            ..Default::default()
        },
    ] {
        assert!(request.requested());
        let physical = VirtualDisplayRequest {
            client_physical: true,
            ..request
        };
        assert!(!physical.requested());
        assert!(!physical.uses_virtual(false, || panic!("no virtual display, no driver check")));
    }
    // Nothing physical to show: a headless host, or an output that names
    // the virtual display itself.
    for request in [
        VirtualDisplayRequest {
            client_physical: true,
            headless: true,
            ..Default::default()
        },
        VirtualDisplayRequest {
            client_physical: true,
            output_override: Some("virtual_display"),
            ..Default::default()
        },
        VirtualDisplayRequest {
            client_physical: true,
            configured_output: "sunshine:virtual_display",
            ..Default::default()
        },
    ] {
        assert!(request.uses_virtual(false, || true));
    }
}
#[test]
fn host_display_choices_parse_and_unknown_values_leave_the_host_in_control() {
    use super::{Arrangement, HostDisplay};
    assert_eq!(HostDisplay::PARAMETER, "hostDisplay");
    assert_eq!(HostDisplay::parse("physical"), Some(HostDisplay::Physical));
    assert_eq!(
        HostDisplay::parse(" Physical "),
        Some(HostDisplay::Physical)
    );
    assert_eq!(
        HostDisplay::parse("exclusive"),
        Some(HostDisplay::Virtual(Arrangement::Exclusive))
    );
    assert_eq!(
        HostDisplay::parse("extended"),
        Some(HostDisplay::Virtual(Arrangement::Extended))
    );
    assert_eq!(
        HostDisplay::parse("extended-primary"),
        Some(HostDisplay::Virtual(Arrangement::Primary))
    );
    for unknown in [
        "",
        " ",
        "default",
        "host",
        "mirror",
        "virtual",
        "0",
        "1",
        "ensure_active",
        "ensure_primary",
        "ensure_only_display",
        "ensure-only-display",
    ] {
        assert_eq!(HostDisplay::parse(unknown), None, "{unknown}");
    }
    for arrangement in [
        Arrangement::Extended,
        Arrangement::Primary,
        Arrangement::Exclusive,
        Arrangement::Isolated,
        Arrangement::PrimaryIsolated,
    ] {
        let choice = HostDisplay::Virtual(arrangement);
        assert_eq!(Arrangement::parse(arrangement.name()).unwrap(), arrangement);
        assert_eq!(HostDisplay::parse(choice.name()), Some(choice));
    }
    assert_eq!(HostDisplay::Physical.name(), "physical");
}
#[test]
fn a_kept_display_serves_a_launch_only_when_it_was_set_up_for_the_same_choice() {
    use super::{Arrangement, HostDisplay};
    let exclusive = Some(HostDisplay::Virtual(Arrangement::Exclusive));
    let extended = Some(HostDisplay::Virtual(Arrangement::Extended));
    // A launch without a choice takes the display as it is.
    for prepared in [None, exclusive, extended, Some(HostDisplay::Physical)] {
        assert!(HostDisplay::reusable(prepared, None));
    }
    assert!(HostDisplay::reusable(exclusive, exclusive));
    assert!(!HostDisplay::reusable(exclusive, extended));
    assert!(!HostDisplay::reusable(None, exclusive));
    assert!(!HostDisplay::reusable(
        exclusive,
        Some(HostDisplay::Physical)
    ));
}
#[test]
fn unapplied_display_modes_are_visible_and_recovery_clears_them() {
    let warnings = crate::session::Warnings::default();
    let requested = (Some((3840, 2160)), Some(116_000), Some(true));
    super::report_mode(&warnings, requested, (1920, 1080, 60_000, false), 116_000);
    let entries = warnings.snapshot();
    assert_eq!(
        entries.iter().map(|w| w.code.as_str()).collect::<Vec<_>>(),
        ["display_hdr", "display_refresh", "display_resolution"]
    );
    assert!(entries[1].message.contains("60.000 Hz"));
    super::report_mode(&warnings, requested, (3840, 2160, 116_000, true), 116_000);
    assert!(warnings.snapshot().is_empty());
    super::report_mode(
        &warnings,
        (None, None, None),
        (3840, 2160, 59_940, false),
        60_000,
    );
    assert!(warnings.snapshot().is_empty());
    super::report_mode(
        &warnings,
        (None, None, None),
        (3840, 2160, 60_000, false),
        116_000,
    );
    assert_eq!(warnings.snapshot()[0].code, "display_refresh");
}
use super::*;
#[test]
fn other_shapes_are_centred_with_bars() {
    // The 32:9 desktop on a 16:10 tablet.
    assert_eq!(letterbox((5120, 1440), (2560, 1600)), (0, 440, 2560, 720));
    // A 16:9 source on a tall phone stream.
    assert_eq!(letterbox((1920, 1080), (1968, 2184)), (0, 538, 1968, 1106));
    assert_eq!(letterbox((3840, 2160), (1920, 1080)), (0, 0, 1920, 1080));
    // Within a pixel of the stream's shape fills it.
    assert_eq!(letterbox((1921, 1080), (1920, 1080)), (0, 0, 1920, 1080));
    assert_eq!(letterbox((0, 0), (1920, 1080)), (0, 0, 1920, 1080));
}
#[test]
fn touch_on_a_letterboxed_stream_lands_on_the_picture_not_the_bars() {
    // A 16:9 display on a 4:3 iPad stream: bars above and below.
    let ipad = picture((2560, 1440), (2732, 2048)).unwrap();
    assert_eq!(ipad, (0, 256, 2732, 1536));
    let at = |x: f64, y: f64| stream_to_picture((x, y), (2732, 2048), ipad);
    assert_eq!(at(0., 256. / 2048.), (0., 0.));
    assert_eq!(at(1., 1792. / 2048.), (1., 1.));
    assert_eq!(at(0.5, 0.5), (0.5, 0.5));
    // On a bar: the picture's edge.
    assert_eq!(at(0.25, 0.), (0.25, 0.));
    assert_eq!(at(0.25, 1.), (0.25, 1.));
    // A display of the stream's shape fills it: nothing to map.
    assert_eq!(picture((3840, 2160), (1920, 1080)), None);
    assert_eq!(picture((1920, 1080), (0, 0)), None);
}
#[test]
fn device_display_modes_parse_like_vibepollo() {
    use crate::framegen::Rate;
    assert_eq!(
        parse_display_mode("1920x1080x59.94"),
        Some((1920, 1080, Rate(59940)))
    );
    assert_eq!(
        parse_display_mode(" 2560X1600x120 "),
        Some((2560, 1600, Rate(120000)))
    );
    for bad in [
        "",
        "1920x1080",
        "1920x1080x0",
        "0x1080x60",
        "1920x1080x60x1",
        "axbxc",
    ] {
        assert_eq!(parse_display_mode(bad), None, "{bad}");
    }
}
#[test]
fn unsupported_physical_resolutions_keep_the_current_mode_and_rate() {
    let odyssey = [
        (5120, 1440, 240),
        (5120, 1440, 120),
        (5120, 1440, 60),
        (3840, 1080, 60),
        (2560, 1440, 120),
        (2560, 1440, 60),
    ];
    let current = (5120, 1440, 240_000);
    // The tablet case: no 2560x1600 mode; keep 5120x1440, use 120 Hz.
    assert_eq!(
        physical_mode(&odyssey, (Some((2560, 1600)), Some(120_000)), current),
        (5120, 1440, 120_000)
    );
    // A supported request is applied as asked.
    assert_eq!(
        physical_mode(&odyssey, (Some((2560, 1440)), Some(120_000)), current),
        (2560, 1440, 120_000)
    );
    // A refresh the chosen resolution lacks keeps the current one.
    assert_eq!(
        physical_mode(&odyssey, (None, Some(144_000)), current),
        (5120, 1440, 240_000)
    );
    // A fractional request is kept when the nearest whole-hertz mode exists.
    assert_eq!(
        physical_mode(&odyssey, (None, Some(119_880)), current),
        (5120, 1440, 119_880)
    );
    // A resolution that only lacks the current refresh takes its highest.
    assert_eq!(
        physical_mode(&odyssey, (Some((3840, 1080)), Some(120_000)), current),
        (3840, 1080, 60_000)
    );
    assert_eq!(
        physical_mode(&[], (Some((1, 1)), Some(60_000)), current),
        current
    );
}
use crate::topology::{Kind, Mode};
#[test]
fn previous_virtual_driver_identity_vectors_use_windows_guid_bytes() {
    assert_eq!(
        virtual_display_id("f773d31b-43da-470c-80d5-02e777a6d993"),
        0xed9b7be9fc3f0b82
    );
    assert_eq!(virtual_display_id("example-client"), 0xbce6a166cf4c68bc);
    assert_eq!(
        virtual_identity("example-client"),
        *uuid::Uuid::parse_str("f12b4b23-a6df-5a22-8eed-f5aaef994ef8")
            .unwrap()
            .as_bytes()
    );
    let combined = app_client_identity(
        "f773d31b-43da-470c-80d5-02e777a6d993",
        "d9428888-122b-11e1-b85c-61cd3cbb3210",
    );
    assert_eq!(
        app_client_identity(&combined, "d9428888-122b-11e1-b85c-61cd3cbb3210"),
        "f773d31b-43da-470c-80d5-02e777a6d993"
    );
}
#[test]
fn libdisplaydevice_uuid_vectors_survive_the_unstable_driver_instance_counter() {
    let path = r"\\?\DISPLAY#ACI27EC#5&4FD2DE4&5&UID4352";
    let edid: Vec<u8> = (0..128).collect();
    let expected = "{ccbebcd3-0583-5b71-92e4-6e28da2e07ff}";
    assert_eq!(
        legacy_device_id(path, Some(r"DISPLAY\ACI27EC\5&4FD2DE4&5&UID4352"), &edid),
        expected
    );
    assert_eq!(
        legacy_device_id(path, Some(r"DISPLAY\ACI27EC\5&4FD2DE4&99&UID4352"), &edid),
        expected
    );
    assert_eq!(
        legacy_device_id(path, None, &[]),
        "{3355dc9e-a978-5cee-9a57-36824dcba1e6}"
    );
    assert_ne!(
        legacy_device_id(path, Some(r"DISPLAY\ACI27EC\5&4FD2DE4&5&UID4353"), &edid),
        expected
    );
}
#[test]
fn retained_render_scale_precedence_and_odd_resolution_vectors() {
    assert_eq!(render_dimensions(1920, 1080, 150, 100), (2880, 1620));
    assert_eq!(render_dimensions(1920, 1080, 150, 200), (3840, 2160));
    assert_eq!(render_dimensions(1365, 767, 75, 100), (1022, 574));
    assert_eq!(render_dimensions(1920, 1080, -1, 100), (1920, 1080));
    assert_eq!(render_dimensions(1920, 1080, i64::MAX, 100), (1920, 1080));
}
fn node(id: &str, x: i32) -> Node {
    Node {
        id: id.into(),
        label: id.into(),
        device_id: id.into(),
        kind: Kind::Physical,
        active: true,
        primary: x == 0,
        desired_position: Position { x, y: 0 },
        mode: Mode {
            width: 1920,
            height: 1080,
            refresh_hz: 60.,
        },
    }
}
#[test]
fn extended_hotplug_keeps_the_original_primary_when_windows_remembers_the_virtual_one() {
    let before = [node("physical", 0), node("other", 1920)];
    // Windows recalls the virtual screen at the origin and moves the
    // physical screens left. All displays remain active, so off-screen
    // recovery does not detect this change.
    let current = [
        node("physical", -1920),
        node("other", 1920),
        node("virtual", 0),
    ];
    for arrangement in [Arrangement::Extended, Arrangement::Isolated] {
        let result = arrangement
            .compose_after_hotplug(&current, "virtual", &[], &before)
            .unwrap();
        assert!(result[0].active && result[0].primary);
        assert_eq!(result[0].desired_position, Position { x: 0, y: 0 });
        assert!(result[1].active && !result[1].primary);
        assert_eq!(result[1].desired_position.x, 3840);
        assert!(result[2].active && !result[2].primary);
        assert_eq!(
            result[2].desired_position,
            if arrangement == Arrangement::Isolated {
                Position { x: 64000, y: 64000 }
            } else {
                Position { x: 1920, y: 0 }
            }
        );
        assert_eq!(
            serde_json::to_value(&result[0].mode).unwrap(),
            serde_json::to_value(&current[0].mode).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&result[1].mode).unwrap(),
            serde_json::to_value(&current[1].mode).unwrap()
        );
    }
}
#[test]
fn deliberate_virtual_primary_layouts_still_promote_the_stream_display() {
    let before = [node("physical", 0)];
    let current = [node("physical", 0), node("virtual", 1920)];
    for arrangement in [
        Arrangement::Primary,
        Arrangement::Exclusive,
        Arrangement::PrimaryIsolated,
    ] {
        let result = arrangement
            .compose_after_hotplug(&current, "virtual", &[], &before)
            .unwrap();
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            serde_json::to_value(arrangement.compose(&current, "virtual", &[]).unwrap()).unwrap()
        );
        assert!(result[1].active && result[1].primary);
        assert_eq!(result[1].desired_position, Position { x: 0, y: 0 });
    }
}
#[test]
fn extended_hotplug_does_not_choose_a_missing_or_inactive_original_primary() {
    let before = [node("physical", 0)];
    let current = [node("virtual", 0)];
    assert_eq!(
        serde_json::to_value(
            Arrangement::Extended
                .compose_after_hotplug(&current, "virtual", &[], &before)
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(&current).unwrap()
    );
    assert_eq!(
        serde_json::to_value(
            Arrangement::Extended
                .compose_after_hotplug(&current, "virtual", &[], &[])
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(&current).unwrap()
    );
    let mut physical = node("physical", -1920);
    physical.active = false;
    let current = [physical, node("virtual", 0)];
    assert_eq!(
        serde_json::to_value(
            Arrangement::Extended
                .compose_after_hotplug(&current, "virtual", &[], &before)
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(&current).unwrap()
    );
}
#[test]
fn shared_and_recreated_displays_keep_the_first_owners_original_primary() {
    let before = [node("physical", 0)];
    for arrangement in [Arrangement::Extended, Arrangement::Isolated] {
        // A second display arrives as primary while the first still owns
        // the layout. Both streamed displays must remain active.
        let targets = ["first".to_owned(), "second".to_owned()];
        let current = [
            node("physical", -3840),
            node("first", -1920),
            node("second", 0),
        ];
        let result = arrangement
            .compose_all_after_hotplug(&current, &targets, &[], &before)
            .unwrap();
        assert!(result[0].active && result[0].primary);
        assert_eq!(result[0].desired_position, Position { x: 0, y: 0 });
        assert!(result[1].active && !result[1].primary);
        assert!(result[2].active && !result[2].primary);
        let first = if arrangement == Arrangement::Isolated {
            Position { x: 64000, y: 64000 }
        } else {
            Position { x: 1920, y: 0 }
        };
        assert_eq!(result[1].desired_position, first);
        assert_eq!(
            result[2].desired_position,
            Position {
                x: first.x + 1920,
                y: first.y
            }
        );
        // Recovery can replace the first display's identity. The saved
        // physical primary, not the old target identity, is the anchor.
        let current = [node("physical", -1920), node("recreated", 0)];
        let result = arrangement
            .compose_all_after_hotplug(&current, &["recreated".into()], &[], &before)
            .unwrap();
        assert!(result[0].active && result[0].primary);
        assert_eq!(result[0].desired_position, Position { x: 0, y: 0 });
        assert!(result[1].active && !result[1].primary);
        assert_eq!(result[1].desired_position, first);
    }
}
#[test]
fn shared_hotplug_keeps_deliberate_primary_modes_and_missing_baseline_fallbacks() {
    let before = [node("physical", 0)];
    let targets = ["first".to_owned(), "second".to_owned()];
    let current = [
        node("physical", -3840),
        node("first", -1920),
        node("second", 0),
    ];
    for arrangement in [
        Arrangement::Primary,
        Arrangement::Exclusive,
        Arrangement::PrimaryIsolated,
    ] {
        let result = arrangement
            .compose_all_after_hotplug(&current, &targets, &[], &before)
            .unwrap();
        assert!(result[1].active && result[1].primary);
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            serde_json::to_value(arrangement.compose_all(&current, &targets, &[]).unwrap())
                .unwrap()
        );
    }
    let mut off = current.clone();
    off[0].active = false;
    for arrangement in [Arrangement::Extended, Arrangement::Isolated] {
        for (current, before) in [
            (&current[..], &[][..]),
            (&current[1..], &before[..]),
            (&off[..], &before[..]),
        ] {
            assert_eq!(
                serde_json::to_value(
                    arrangement
                        .compose_all_after_hotplug(current, &targets, &[], before)
                        .unwrap()
                )
                .unwrap(),
                serde_json::to_value(arrangement.compose_all(current, &targets, &[]).unwrap())
                    .unwrap()
            );
        }
    }
}
#[test]
fn several_streams_share_one_layout_without_overlapping() {
    let mut second = node("second", 1920);
    second.mode.width = 1968;
    let nodes = vec![node("physical", 0), node("first", 1920), second];
    let targets = ["first".to_owned(), "second".to_owned()];
    let at = |nodes: &[Node], id: &str| {
        let n = nodes.iter().find(|n| n.device_id == id).unwrap();
        (
            n.active,
            n.primary,
            n.desired_position.x,
            n.desired_position.y,
        )
    };
    // Exclusive: only the streamed displays stay on; the first is primary.
    let exclusive = Arrangement::Exclusive
        .compose_all(&nodes, &targets, &[])
        .unwrap();
    assert!(!at(&exclusive, "physical").0);
    assert_eq!(at(&exclusive, "first"), (true, true, 0, 0));
    assert_eq!(at(&exclusive, "second"), (true, false, 1920, 0));
    // Extended: the second display goes right of everything active.
    let extended = Arrangement::Extended
        .compose_all(&nodes, &targets, &[])
        .unwrap();
    assert_eq!(at(&extended, "physical"), (true, true, 0, 0));
    assert_eq!(at(&extended, "first"), (true, false, 1920, 0));
    assert_eq!(at(&extended, "second"), (true, false, 3840, 0));
    // Isolated: streamed displays sit next to each other, away from the rest.
    let isolated = Arrangement::Isolated
        .compose_all(&nodes, &targets, &[])
        .unwrap();
    assert_eq!(at(&isolated, "first"), (true, false, 64000, 64000));
    assert_eq!(at(&isolated, "second"), (true, false, 65920, 64000));
    assert_eq!(at(&isolated, "physical"), (true, true, 0, 0));
    // One target is the single-stream layout.
    let single = Arrangement::Exclusive
        .compose_all(&nodes, &targets[..1], &[])
        .unwrap();
    assert!(!at(&single, "second").0);
    assert!(
        Arrangement::Exclusive
            .compose_all(&nodes, &[], &[])
            .is_err()
    );
}
#[test]
fn exclusive_retains_remote_monitors_and_isolation_keeps_physical_geometry() {
    let nodes = vec![
        node("physical", 0),
        node("game", 1920),
        node("remote", 3840),
    ];
    let exclusive = Arrangement::Exclusive
        .compose(&nodes, "game", &["remote".into()])
        .unwrap();
    assert!(!exclusive[0].active);
    assert!(exclusive[1].primary);
    assert!(exclusive[2].active);
    assert_eq!(exclusive[1].desired_position, Position { x: 0, y: 0 });
    let isolated = Arrangement::PrimaryIsolated
        .compose(&nodes, "game", &[])
        .unwrap();
    assert_eq!(
        isolated[0].desired_position,
        Position { x: 64000, y: 64000 }
    );
    assert_eq!(
        isolated[2].desired_position.x - isolated[0].desired_position.x,
        3840
    );
}
#[test]
fn remote_monitors_follow_which_displays_stay_on_but_keep_their_place() {
    let layout = |value| Arrangement::parse(value).unwrap().for_remote_monitor();
    assert_eq!(layout("exclusive"), Some(Arrangement::Exclusive));
    assert_eq!(layout("extended_primary"), Some(Arrangement::Primary));
    assert_eq!(
        layout("extended_primary_isolated"),
        Some(Arrangement::Primary)
    );
    assert_eq!(layout("extended"), None);
    assert_eq!(layout("extended_isolated"), None);
    // Exclusive on the remote monitor: the physical display goes off and
    // the remote monitor becomes primary, with another device's remote
    // monitor kept on beside it.
    let nodes = vec![
        node("physical", 0),
        node("remote", 1920),
        node("other", 3840),
    ];
    let exclusive = Arrangement::Exclusive
        .compose(&nodes, "remote", &["other".into()])
        .unwrap();
    assert!(!exclusive[0].active && !exclusive[0].primary);
    assert!(exclusive[1].active && exclusive[1].primary);
    assert_eq!(exclusive[1].desired_position, Position { x: 0, y: 0 });
    assert!(exclusive[2].active);
}
#[test]
fn a_display_the_exclusive_layout_switched_off_is_put_back_off_after_it_settles() {
    use std::time::{Duration, Instant};
    // Before the stream: the physical monitor on its own, a TV switched
    // off. The exclusive layout keeps only the virtual display on.
    let mut phys = node("phys", 0);
    phys.primary = true;
    let mut tv = node("tv", 1920);
    tv.active = false;
    let mut vdd = node("vdd", 0);
    vdd.active = false;
    let before = vec![phys.clone(), tv.clone(), vdd.clone()];
    let applied = Arrangement::Exclusive
        .compose(&[phys.clone(), tv.clone(), node("vdd", 3840)], "vdd", &[])
        .unwrap();
    let on = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert!(switched_back_on(&before, &applied, &on(&["vdd"])).is_empty());
    // The Win key over an exclusive-fullscreen game: Windows recalls its
    // saved layout and the physical monitor comes back beside the stream.
    assert_eq!(
        switched_back_on(&before, &applied, &on(&["vdd", "phys"])),
        ["phys"]
    );
    // The TV was off before the stream: switching it on is the user's call.
    assert!(switched_back_on(&before, &applied, &on(&["vdd", "tv"])).is_empty());
    // Another client's display arriving was never laid out by this stream.
    assert!(switched_back_on(&before, &applied, &on(&["vdd", "vdd2"])).is_empty());
    // The extended layout switches nothing off, so there is nothing to keep off.
    let extended = Arrangement::Extended
        .compose(&[phys.clone(), tv, node("vdd", 3840)], "vdd", &[])
        .unwrap();
    assert!(switched_back_on(&before, &extended, &on(&["vdd", "phys"])).is_empty());

    let start = Instant::now();
    let at = |ms: u64| start + Duration::from_millis(ms);
    let mut watch = LayoutWatch::default();
    assert!(watch.due(at(0)));
    assert!(!watch.due(at(100)));
    assert!(watch.due(at(250)));
    // A game's own mode switch that settles within the window is left alone.
    assert!(!watch.observe(true, at(0)));
    assert!(!watch.observe(true, at(250)));
    assert!(!watch.observe(false, at(500)));
    assert!(!watch.observe(true, at(750)));
    assert!(!watch.observe(true, at(1000)));
    // Still on after the settle time: put the layout back once.
    assert!(watch.observe(true, at(1250)));
    assert!(!watch.observe(true, at(1500)));
    // A refused attempt waits before the next check.
    watch.failed(at(1500));
    assert!(!watch.due(at(3000)));
    assert!(watch.due(at(3500)));
}
