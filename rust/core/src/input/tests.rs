use super::*;
#[test]
fn denied_input_names_the_device_permission_to_turn_on() {
    let pad = Input::Controller {
        id: 0,
        active: 1,
        buttons: 0,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    };
    let (code, message) = pad.denied_warning("Pixel 8");
    assert_eq!(code, "input_permission_controller");
    assert!(message.starts_with("Pixel 8 sends controller input"));
    assert!(message.contains("Turn on Controllers"));
    // Arrivals, motion and haptics share the controller permission and warning.
    for event in [
        Input::Arrival {
            id: 0,
            kind: 1,
            capabilities: 0,
            buttons: 0,
        },
        Input::Haptics(true),
    ] {
        assert_eq!(event.required_permission(), 1 << 8);
        assert_eq!(
            event.denied_warning("Pixel 8"),
            (code.clone(), message.clone())
        );
    }
    let (code, message) = Input::Text("a".into()).denied_warning("Tablet");
    assert_eq!(code, "input_permission_keyboard");
    assert!(message.contains("sends keyboard input") && message.contains("Turn on Keyboard"));
    let (code, message) = Input::Relative { x: 1, y: 1 }.denied_warning("Tablet");
    assert_eq!(code, "input_permission_mouse");
    assert!(message.contains("Turn on Mouse"));
}
fn touchpad_frame(count: u8, buttons: u8, contacts: &[(u8, u32, u16, u16)]) -> Vec<u8> {
    let mut b = vec![0, 0, 0, 72];
    b.extend_from_slice(&TOUCHPAD_FRAME_MAGIC.to_le_bytes());
    b.extend_from_slice(&[count, buttons, 0xff, 0xff, 105, 0, 70, 0]);
    for n in 0..5 {
        let (event, id, x, y) = contacts.get(n).copied().unwrap_or_default();
        b.extend_from_slice(&[event, 0, 0, 0x80]);
        b.extend_from_slice(&id.to_le_bytes());
        b.extend_from_slice(&x.to_le_bytes());
        b.extend_from_slice(&y.to_le_bytes());
    }
    b
}
#[test]
fn touchpad_packets_decode_as_v_plus_sends_them() {
    use crate::virtual_hid::Contact;
    let frame = touchpad_frame(2, 1, &[(1, 7, 0, 65535), (3, 9, 32768, 0)]);
    assert_eq!(frame.len(), 76);
    let Input::Touchpad {
        frame: true,
        buttons: 1,
        width_mm: 105,
        height_mm: 70,
        contacts,
    } = decode(&frame).unwrap()
    else {
        panic!("a touchpad frame");
    };
    assert_eq!(contacts.len(), 2);
    assert_eq!(
        contacts[0],
        Contact {
            event: 1,
            id: 7,
            x: 0.0,
            y: 1.0
        }
    );
    assert_eq!((contacts[1].event, contacts[1].id), (3, 9));
    assert!((contacts[1].x - 0.5).abs() < 0.001);
    // Six contacts, or an unknown event, are refused.
    assert!(decode(&touchpad_frame(6, 0, &[])).is_err());
    assert!(decode(&touchpad_frame(1, 0, &[(8, 1, 0, 0)])).is_err());
    // A contact past the count is ignored whatever it holds.
    assert!(decode(&touchpad_frame(0, 0, &[(9, 1, 0, 0)])).is_ok());
    // Another length is another packet.
    let mut long = frame.clone();
    long.extend_from_slice(&[0; 4]);
    long[..4].copy_from_slice(&76u32.to_be_bytes());
    assert!(decode(&long).is_err());

    let mut single = vec![0, 0, 0, 36];
    single.extend_from_slice(&TOUCHPAD_MAGIC.to_le_bytes());
    single.extend_from_slice(&[3, 1, 0xff, 0xff, 4, 3, 2, 1, 105, 0, 70, 0]);
    for v in [0.25f32, 0.5, 1.0, 0.0, 0.0] {
        single.extend_from_slice(&v.to_le_bytes());
    }
    assert_eq!(
        decode(&single).unwrap(),
        Input::Touchpad {
            frame: false,
            buttons: 1,
            width_mm: 105,
            height_mm: 70,
            contacts: vec![Contact {
                event: 3,
                id: 0x0102_0304,
                x: 0.25,
                y: 0.5
            }],
        }
    );
    assert_eq!(decode(&single).unwrap().required_permission(), 1 << 11);
    single[20..24].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(decode(&single).is_err());
}
#[test]
fn touchpad_moves_of_the_same_fingers_coalesce_and_nothing_else_does() {
    let moves = |x: u16| decode(&touchpad_frame(2, 0, &[(3, 1, x, 0), (3, 2, x, 0)])).unwrap();
    let mut first = moves(1);
    assert_eq!(first.merge(&moves(2)), Batch::Merged);
    assert_eq!(first, moves(2));
    let other = decode(&touchpad_frame(1, 0, &[(3, 5, 0, 0)])).unwrap();
    assert_eq!(first.merge(&other), Batch::Skip);
    let lift = decode(&touchpad_frame(1, 0, &[(2, 1, 0, 0)])).unwrap();
    assert_eq!(first.merge(&lift), Batch::Stop);
    let click = decode(&touchpad_frame(2, 1, &[(3, 1, 0, 0), (3, 2, 0, 0)])).unwrap();
    assert_eq!(first.merge(&click), Batch::Stop);
}
#[test]
fn overflow_leaves_both_axes_untouched() {
    let mut a = Input::Relative {
        x: 100,
        y: i16::MAX,
    };
    let b = Input::Relative { x: 1, y: 1 };
    assert_eq!(a.merge(&b), Batch::Stop);
    assert_eq!(
        a,
        Input::Relative {
            x: 100,
            y: i16::MAX
        }
    );
    assert_eq!(b, Input::Relative { x: 1, y: 1 });
}
#[test]
fn relative_negative_and_scroll_merge() {
    let mut a = Input::Relative { x: -200, y: 50 };
    assert_eq!(a.merge(&Input::Relative { x: 5, y: -4 }), Batch::Merged);
    assert_eq!(a, Input::Relative { x: -195, y: 46 });
}
#[test]
fn never_swallow_button_transition() {
    let mut a = Input::MouseButton {
        button: 1,
        down: true,
    };
    assert_eq!(
        a.merge(&Input::MouseButton {
            button: 1,
            down: false
        }),
        Batch::Stop
    );
}
#[test]
fn malformed_packets_never_panic() {
    for len in 0..96 {
        for magic in [
            3u32,
            4,
            5,
            7,
            8,
            10,
            12,
            23,
            0x55000002,
            0x55000003,
            0x55000005,
            0x55000006,
            0x5500000B,
            STEAM_ATTRIBUTES_MAGIC,
            u32::MAX,
        ] {
            let mut b = vec![0; len];
            if len >= 8 {
                b[..4].copy_from_slice(&((len - 4) as u32).to_be_bytes());
                b[4..8].copy_from_slice(&magic.to_le_bytes());
            }
            let _ = decode(&b);
        }
    }
}
/// 0x5500000B is the pen packet with the barrel roll after it (`rubylight_protocol::pen`);
/// the stock pen packet leaves the roll unknown.
#[test]
fn pen_barrel_roll_decodes_beside_the_stock_pen() {
    use rubylight_protocol::pen::{PEN_BYTES, PenBarrelRoll, ROLL_UNKNOWN};
    let mut pen = [0u8; PEN_BYTES];
    pen[..4].copy_from_slice(&((PEN_BYTES - 4) as u32).to_be_bytes());
    pen[4..8].copy_from_slice(&0x5500_0003u32.to_le_bytes());
    pen[8] = 1;
    pen[9] = 1;
    pen[12..16].copy_from_slice(&0.25f32.to_le_bytes());
    pen[16..20].copy_from_slice(&0.75f32.to_le_bytes());
    pen[20..24].copy_from_slice(&0.5f32.to_le_bytes());
    pen[24..26].copy_from_slice(&90u16.to_le_bytes());
    pen[26] = 30;
    let stock = decode(&pen).unwrap();
    let Input::Pen { roll, .. } = stock else {
        panic!("{stock:?}");
    };
    assert_eq!(roll, u16::MAX);
    for value in [0, 270, 359, ROLL_UNKNOWN] {
        let wire = PenBarrelRoll::new(&pen, value).unwrap().encode();
        let Input::Pen {
            event,
            tool,
            x,
            y,
            pressure,
            rotation,
            tilt,
            roll,
            ..
        } = decode(&wire).unwrap()
        else {
            panic!("not a pen");
        };
        assert_eq!(
            (event, tool, x, y, pressure, rotation, tilt, roll),
            (1, 1, 0.25, 0.75, 0.5, 90, 30, value)
        );
    }
    // A roll past 359 that is not unknown, a short packet and the pen's own checks
    // refuse it.
    let mut wire = PenBarrelRoll::new(&pen, 10).unwrap().encode();
    wire[36..38].copy_from_slice(&360u16.to_le_bytes());
    assert!(decode(&wire).is_err());
    let mut short = PenBarrelRoll::new(&pen, 10).unwrap().encode().to_vec();
    short.truncate(38);
    short[..4].copy_from_slice(&34u32.to_be_bytes());
    assert!(decode(&short).is_err());
    let mut tool = PenBarrelRoll::new(&pen, 10).unwrap().encode();
    tool[9] = 3;
    assert!(decode(&tool).is_err());
}
#[test]
fn keyboard_codes_drop_moonlights_high_byte() {
    // Left arrow (VK 0x25) with Shift, as moonlight-qt and Artemis send it.
    let p = hex::decode("0000000a03000000002580010000").unwrap();
    assert_eq!(
        decode(&p).unwrap(),
        Input::Keyboard {
            key: 0x25,
            modifiers: 1,
            flags: 0,
            down: true
        }
    );
}
fn controller_touch(touchpad: u8, event: u8, pointer: u32, x: f32) -> Input {
    Input::ControllerTouch {
        id: 2,
        event,
        touchpad,
        pointer,
        x,
        y: 0.5,
        pressure: 0.75,
    }
}
#[test]
fn controller_touch_decodes_the_wire_touchpad_index_without_changing_coordinates() {
    for touchpad in [0, 1, 2, 255] {
        let mut packet = vec![0; 28];
        packet[..4].copy_from_slice(&24u32.to_be_bytes());
        packet[4..8].copy_from_slice(&0x55000005u32.to_le_bytes());
        packet[8] = 2;
        packet[9] = 3;
        packet[11] = touchpad;
        packet[12..16].copy_from_slice(&17u32.to_le_bytes());
        packet[16..20].copy_from_slice(&0.25f32.to_le_bytes());
        packet[20..24].copy_from_slice(&0.5f32.to_le_bytes());
        packet[24..28].copy_from_slice(&0.75f32.to_le_bytes());
        assert_eq!(
            decode(&packet).unwrap(),
            controller_touch(touchpad, 3, 17, 0.25)
        );
    }
}
#[test]
fn controller_touch_coalescing_keeps_surfaces_and_cancel_all_separate() {
    let mut primary = controller_touch(0, 3, 17, 0.25);
    let original = primary.clone();
    assert_eq!(primary.merge(&controller_touch(1, 3, 17, 0.9)), Batch::Skip);
    assert_eq!(primary, original);
    assert_eq!(primary.merge(&controller_touch(1, 7, 0, 0.0)), Batch::Skip);
    assert_eq!(primary.merge(&controller_touch(0, 7, 0, 0.0)), Batch::Stop);
    assert_eq!(primary, original);
    assert_eq!(primary.merge(&controller_touch(0, 2, 17, 0.0)), Batch::Stop);
    assert_eq!(
        primary.merge(&controller_touch(0, 3, 17, 0.5)),
        Batch::Merged
    );
    assert_eq!(primary, controller_touch(0, 3, 17, 0.5));
    let mut cancelled = controller_touch(0, 7, 0, 0.0);
    assert_eq!(cancelled.merge(&primary), Batch::Stop);
}

#[test]
fn actual_moonlight_mouse_packet() {
    let p = hex::decode("0000000807000000fffb0004").unwrap();
    assert_eq!(decode(&p).unwrap(), Input::Relative { x: -5, y: 4 });
}

fn native_touch_packet(event: u8, pressure: f32) -> Vec<u8> {
    let mut packet = vec![0; 36];
    packet[..4].copy_from_slice(&32u32.to_be_bytes());
    packet[4..8].copy_from_slice(&0x55000002u32.to_le_bytes());
    packet[8] = event;
    packet[10..12].copy_from_slice(&u16::MAX.to_le_bytes());
    packet[12..16].copy_from_slice(&7u32.to_le_bytes());
    packet[16..20].copy_from_slice(&0.25f32.to_le_bytes());
    packet[20..24].copy_from_slice(&0.75f32.to_le_bytes());
    packet[24..28].copy_from_slice(&pressure.to_le_bytes());
    packet
}

#[test]
fn native_touch_preserves_transitions_with_unknown_pressure() {
    for event in [1, 3, 2] {
        for pressure in [0., 0.5, 1., f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(
                decode(&native_touch_packet(event, pressure)).unwrap(),
                Input::Touch {
                    event,
                    id: 7,
                    x: 0.25,
                    y: 0.75,
                    pressure: if pressure.is_finite() { pressure } else { 0. },
                    major: 0.,
                    minor: 0.,
                    rotation: u16::MAX,
                }
            );
        }
    }
}

/// The codec's golden attributes packet, byte for byte.
fn steam_attributes_packet() -> Vec<u8> {
    let mut packet = vec![0; 84];
    packet[..8].copy_from_slice(&[0, 0, 0, 80, 0x08, 0x00, 0x00, 0x55]);
    packet[8..12].copy_from_slice(&[1, 2, 0xff, 0]);
    packet[12..16].copy_from_slice(&[0xe3, 0x85, 0x4d, 0x6a]);
    packet[16..20].copy_from_slice(&[0x2e, 0xf9, 0xd2, 0x68]);
    packet[20..24].copy_from_slice(&[74, 0, 0, 0]);
    packet[24..28].copy_from_slice(&[0x04, 0x03, 0x02, 0x01]);
    packet[28..32].copy_from_slice(&[0x14, 0x13, 0x12, 0x11]);
    packet[32..36].copy_from_slice(&[0x24, 0x23, 0x22, 0x21]);
    packet[36..48].copy_from_slice(b"26e1f7ef1955");
    packet[52..59].copy_from_slice(b"FXA0001");
    packet[68..75].copy_from_slice(b"MXA0002");
    packet
}

#[test]
fn a_controller_identity_decodes_and_a_bad_one_is_refused() {
    let identity = ControllerIdentity {
        controller: 3,
        vendor: 0x057E,
        product: 0x2069,
    };
    assert_eq!(
        decode(&identity.encode()).unwrap(),
        Input::ControllerIdentity(identity)
    );
    assert_eq!(
        Input::ControllerIdentity(identity).required_permission(),
        1 << 8
    );
    let mut bad = identity.encode();
    bad[8] = 16;
    assert!(decode(&bad).is_err());
    assert!(decode(&identity.encode()[..15]).is_err());
}
fn steam_attributes_error(packet: &[u8]) -> SteamAttributesError {
    *decode(packet)
        .unwrap_err()
        .downcast_ref::<SteamAttributesError>()
        .expect("typed Steam attributes error")
}

#[test]
fn steam_attributes_decode_the_codecs_golden_packet() {
    let Input::SteamAttributes(attributes) = decode(&steam_attributes_packet()).unwrap() else {
        panic!("not Steam attributes");
    };
    let padded = |text: &[u8]| {
        let mut out = [0; 16];
        out[..text.len()].copy_from_slice(text);
        out
    };
    assert_eq!(
        attributes,
        SteamAttributes {
            controller: 1,
            transport: rubylight_protocol::steam_controller::LI_SCATTR_TRANSPORT_PUCK,
            valid_mask: 0xff,
            firmware_build_time: 0x6a4d_85e3,
            bootloader_build_time: 0x68d2_f92e,
            board_revision: 74,
            capabilities: 0x0102_0304,
            radio_firmware_build_time: 0x1112_1314,
            dongle_firmware_build_time: 0x2122_2324,
            build_id: padded(b"26e1f7ef1955"),
            unit_serial: padded(b"FXA0001"),
            board_serial: padded(b"MXA0002"),
        }
    );
    // Shares the controller permission; never coalesced with anything.
    let event = Input::SteamAttributes(attributes);
    assert_eq!(event.required_permission(), 1 << 8);
    assert_eq!(event.clone().merge(&event), Batch::Stop);
}

#[test]
fn steam_attributes_of_the_wrong_length_are_a_typed_error() {
    let golden = steam_attributes_packet();
    for len in [8, 12, 80, 83, 85, 120] {
        let mut packet = golden.clone();
        packet.resize(len, 0);
        // A consistent framing header must not let a wrong length through.
        packet[..4].copy_from_slice(&((len - 4) as u32).to_be_bytes());
        assert_eq!(
            steam_attributes_error(&packet),
            SteamAttributesError::Length { len }
        );
        packet[..4].copy_from_slice(&80u32.to_be_bytes());
        assert_eq!(
            steam_attributes_error(&packet),
            SteamAttributesError::Length { len }
        );
    }
    let mut header = golden.clone();
    header[..4].copy_from_slice(&84u32.to_be_bytes());
    assert_eq!(
        steam_attributes_error(&header),
        SteamAttributesError::Codec(SteamError::BadHeader {
            length: 84,
            magic: STEAM_ATTRIBUTES_MAGIC
        })
    );
    let mut controller = golden;
    controller[8] = 16;
    assert_eq!(
        steam_attributes_error(&controller),
        SteamAttributesError::Controller { id: 16 }
    );
}

#[test]
fn refused_or_unknown_steam_packets_leave_other_input_alone() {
    let mouse = hex::decode("0000000807000000fffb0004").unwrap();
    let mut unknown = steam_attributes_packet();
    unknown[4] = 0x09;
    for refused in [vec![0, 0, 0, 4, 0x08, 0, 0, 0x55], unknown] {
        assert!(decode(&refused).is_err());
        assert_eq!(decode(&mouse).unwrap(), Input::Relative { x: -5, y: 4 });
    }
}

#[test]
fn native_touch_still_rejects_nonfinite_coordinates() {
    for offset in [16, 20] {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut packet = native_touch_packet(1, f32::NAN);
            packet[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(decode(&packet).is_err());
        }
    }
}
