use super::*;
use butterpollo_core::input::{decode, Input};

fn host(event: &InputEvent) -> Input {
    decode(&event.encode(true)).unwrap_or_else(|e| panic!("{event:?}: {e}"))
}

#[test]
fn a_controller_identity_is_the_shared_packet_and_decodes_on_the_host() {
    use rubylight_protocol::controller_identity::ControllerIdentity;
    let event = InputEvent::ControllerIdentity {
        number: 2,
        vendor: 0x057E,
        product: 0x2069,
    };
    let identity = ControllerIdentity {
        controller: 2,
        vendor: 0x057E,
        product: 0x2069,
    };
    assert_eq!(event.encode(true), identity.encode().to_vec());
    assert_eq!(event.channel(), channel::GAMEPAD_BASE + 2);
    // The host decodes 0x55000080 with this same shared decoder (core/src/input.rs); the
    // host core these tests pin predates the packet, so it is checked here directly.
    assert_eq!(ControllerIdentity::decode(&event.encode(true)), Ok(identity));
}

#[test]
fn packets_decode_on_the_host() {
    assert_eq!(host(&InputEvent::EnableHaptics), Input::Haptics(true));
    assert_eq!(
        host(&InputEvent::Keyboard {
            key_code: 0x8041,
            down: true,
            modifiers: modifier::SHIFT,
            flags: 1
        }),
        Input::Keyboard {
            key: 0x41,
            modifiers: 1,
            flags: 1,
            down: true
        }
    );
    assert_eq!(
        host(&InputEvent::Keyboard {
            key_code: 0x80A5,
            down: false,
            modifiers: modifier::EXTENDED,
            flags: 0
        }),
        Input::Keyboard {
            key: 0xA5,
            modifiers: 0x10,
            flags: 0,
            down: false
        }
    );
    assert_eq!(
        host(&InputEvent::Text("é".as_bytes().to_vec())),
        Input::Text("é".into())
    );
    assert_eq!(
        host(&InputEvent::MouseMove { dx: -300, dy: 7 }),
        Input::Relative { x: -300, y: 7 }
    );
    assert_eq!(
        host(&InputEvent::MousePosition {
            x: 100,
            y: 200,
            width: 1919,
            height: 1079
        }),
        Input::Absolute {
            x: 100,
            y: 200,
            width: 1919,
            height: 1079
        }
    );
    assert_eq!(
        host(&InputEvent::MouseButton {
            button: mouse_button::X2,
            down: true
        }),
        Input::MouseButton {
            button: 5,
            down: true
        }
    );
    assert_eq!(
        host(&InputEvent::MouseButton {
            button: mouse_button::LEFT,
            down: false
        }),
        Input::MouseButton {
            button: 1,
            down: false
        }
    );
    assert_eq!(
        host(&InputEvent::Scroll { amount: -120 }),
        Input::Scroll {
            amount: -120,
            horizontal: false
        }
    );
    assert_eq!(
        host(&InputEvent::HScroll { amount: 60 }),
        Input::Scroll {
            amount: 60,
            horizontal: true
        }
    );
    assert_eq!(
        host(&InputEvent::Controller {
            number: 3,
            active_mask: 0b1001,
            buttons: button::A | button::PADDLE2 | button::MISC,
            left_trigger: 10,
            right_trigger: 255,
            left_stick: (-32768, 32767),
            right_stick: (1, -1),
        }),
        Input::Controller {
            id: 3,
            active: 0b1001,
            buttons: button::A | button::PADDLE2 | button::MISC,
            left_trigger: 10,
            right_trigger: 255,
            sticks: [-32768, 32767, 1, -1],
        }
    );
    assert_eq!(
        host(&InputEvent::ControllerArrival {
            number: 2,
            kind: 2,
            capabilities: 0x8109,
            supported_buttons: 0x3f_ffff
        }),
        Input::Arrival {
            id: 2,
            kind: 2,
            capabilities: 0x8109,
            buttons: 0x3f_ffff
        }
    );
    assert_eq!(
        host(&InputEvent::Touch {
            event: touch_event::MOVE,
            pointer_id: 9,
            x: 0.25,
            y: 0.5,
            pressure_or_distance: 0.75,
            contact_major: 0.1,
            contact_minor: 0.05,
            rotation: 90,
        }),
        Input::Touch {
            event: 3,
            id: 9,
            x: 0.25,
            y: 0.5,
            pressure: 0.75,
            major: 0.1,
            minor: 0.05,
            rotation: 90
        }
    );
    assert_eq!(
        host(&InputEvent::Pen {
            event: touch_event::DOWN,
            tool: 2,
            buttons: 1,
            x: 0.1,
            y: 0.2,
            pressure_or_distance: 0.3,
            contact_major: 0.0,
            contact_minor: 0.0,
            rotation: 45,
            tilt: 30,
            roll: rubylight_protocol::pen::ROLL_UNKNOWN,
        }),
        Input::Pen {
            event: 1,
            tool: 2,
            buttons: 1,
            x: 0.1,
            y: 0.2,
            pressure: 0.3,
            rotation: 45,
            tilt: 30
        }
    );
    assert_eq!(
        host(&InputEvent::ControllerTouch {
            number: 1,
            event: 1,
            touchpad: 1,
            pointer_id: 4,
            x: 0.5,
            y: 0.5,
            pressure: 1.0
        }),
        Input::ControllerTouch {
            id: 1,
            event: 1,
            touchpad: 1,
            pointer: 4,
            x: 0.5,
            y: 0.5,
            pressure: 1.0
        }
    );
    assert_eq!(
        host(&InputEvent::ControllerMotion {
            number: 0,
            motion_type: motion_type::GYRO,
            x: 1.0,
            y: -2.0,
            z: 3.5
        }),
        Input::Motion {
            id: 0,
            kind: 2,
            xyz: [1.0, -2.0, 3.5]
        }
    );
    assert_eq!(
        host(&InputEvent::ControllerBattery {
            number: 15,
            state: 3,
            percentage: 80
        }),
        Input::Battery {
            id: 15,
            state: 3,
            percent: 80
        }
    );
}

/// A known roll makes the pen packet 0x5500000B, the stock packet plus the roll; an
/// unknown roll leaves it byte for byte the stock 0x55000003.
#[test]
fn a_known_barrel_roll_extends_the_pen_packet() {
    use rubylight_protocol::pen::{PenBarrelRoll, ROLL_UNKNOWN};
    let pen = |roll| InputEvent::Pen {
        event: touch_event::MOVE,
        tool: 1,
        buttons: 0,
        x: 0.25,
        y: 0.75,
        pressure_or_distance: 0.5,
        contact_major: 0.0,
        contact_minor: 0.0,
        rotation: 90,
        tilt: 30,
        roll,
    };
    let stock = pen(ROLL_UNKNOWN).encode(true);
    assert_eq!(stock.len(), 36);
    assert_eq!(stock[4..8], magic::PEN.to_le_bytes());
    for roll in [0, 270, 359] {
        let wire = pen(roll).encode(true);
        let decoded = PenBarrelRoll::decode(&wire).unwrap_or_else(|| panic!("{roll}"));
        assert_eq!(decoded.roll(), roll);
        assert_eq!(decoded.pen()[..], stock[..], "{roll}");
    }
}

#[test]
fn sizes_match_input_h() {
    let sizes = [
        (InputEvent::EnableHaptics, 10),
        (
            InputEvent::Keyboard {
                key_code: 0,
                down: true,
                modifiers: 0,
                flags: 0,
            },
            14,
        ),
        (InputEvent::MouseMove { dx: 0, dy: 0 }, 12),
        (
            InputEvent::MousePosition {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            18,
        ),
        (
            InputEvent::MouseButton {
                button: 1,
                down: true,
            },
            9,
        ),
        (InputEvent::Scroll { amount: 0 }, 14),
        (InputEvent::HScroll { amount: 0 }, 10),
        (
            InputEvent::ControllerBattery {
                number: 0,
                state: 0,
                percentage: 0,
            },
            12,
        ),
        (
            InputEvent::ControllerMotion {
                number: 0,
                motion_type: 1,
                x: 0.,
                y: 0.,
                z: 0.,
            },
            24,
        ),
        (
            InputEvent::ControllerArrival {
                number: 0,
                kind: 0,
                capabilities: 0,
                supported_buttons: 0,
            },
            16,
        ),
    ];
    for (event, size) in sizes {
        let bytes = event.encode(true);
        assert_eq!(bytes.len(), size, "{event:?}");
        assert_eq!(
            u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize,
            size - 4
        );
    }
}

/// SS_TOUCHPAD_PACKET and SS_TOUCHPAD_FRAME_PACKET of Moonlight V+'s moonlight-common-c
/// (`Input.h`, `LiSendTouchpadEvent`, `LiSendTouchpadFrameEvent`), field by field.
#[test]
fn touchpad_packets_match_v_plus_input_h() {
    let single = InputEvent::Touchpad {
        event: touch_event::MOVE,
        buttons: touchpad::BUTTON_PRIMARY,
        rotation: touchpad::ROTATION_UNKNOWN,
        pointer_id: 0x0102_0304,
        width_mm: 105,
        height_mm: 70,
        x: 0.25,
        y: 0.5,
        pressure: 1.0,
        contact_major: 0.0,
        contact_minor: 0.0,
    }
    .encode(true);
    let mut expected = vec![0, 0, 0, 36, 0x09, 0, 0, 0x55, 3, 1, 0xFF, 0xFF, 4, 3, 2, 1, 105, 0, 70, 0];
    for v in [0.25f32, 0.5, 1.0, 0.0, 0.0] {
        expected.extend_from_slice(&v.to_le_bytes());
    }
    assert_eq!(single, expected);

    let mut contacts = [TouchpadContact::default(); touchpad::MAX_FRAME_CONTACTS];
    contacts[0] = TouchpadContact { event: touch_event::DOWN, pointer_id: 7, x: 0.0, y: 1.0, pressure: 0.5 };
    contacts[1] = TouchpadContact { event: touch_event::UP, pointer_id: 9, x: 2.0, y: -1.0, pressure: 0.0 };
    // A contact past `count` goes as zeros whatever it holds.
    contacts[2] = TouchpadContact { event: touch_event::MOVE, pointer_id: 11, x: 0.5, y: 0.5, pressure: 0.5 };
    let frame = InputEvent::TouchpadFrame {
        count: 2,
        buttons: 0,
        rotation: touchpad::ROTATION_UNKNOWN,
        width_mm: 105,
        height_mm: 70,
        contacts,
    }
    .encode(true);
    let mut expected = vec![0, 0, 0, 72, 0x0A, 0, 0, 0x55, 2, 0, 0xFF, 0xFF, 105, 0, 70, 0];
    expected.extend_from_slice(&[1, 0, 0x00, 0x80, 7, 0, 0, 0, 0, 0, 0xFF, 0xFF]);
    expected.extend_from_slice(&[2, 0, 0, 0, 9, 0, 0, 0, 0xFF, 0xFF, 0, 0]);
    expected.extend_from_slice(&[0; 36]);
    assert_eq!(frame, expected);
    assert_eq!(frame.len(), 76);
}

#[test]
fn gfe_controllers_carry_no_extended_buttons() {
    let event = InputEvent::Controller {
        number: 0,
        active_mask: 1,
        buttons: button::A | button::PADDLE1,
        left_trigger: 0,
        right_trigger: 0,
        left_stick: (0, 0),
        right_stick: (0, 0),
    };
    let Input::Controller { buttons, .. } = decode(&event.encode(false)).unwrap() else {
        panic!("controller");
    };
    assert_eq!(buttons, button::A);
    assert_eq!(event.channel(), 0x10);
}
