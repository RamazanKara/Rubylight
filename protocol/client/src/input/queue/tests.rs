use super::*;
use crate::control::negotiation::encryption;
use butterpollo_core::input::{decode, Input};

fn sunshine() -> HostCapabilities {
    HostCapabilities {
        app_version: [7, 1, 431, -1],
        feature_flags: feature::PEN_TOUCH_EVENTS | feature::CONTROLLER_TOUCH_EVENTS,
        encryption_enabled: encryption::CONTROL_V2,
        control_messages: vec![],
        client_roaming: false,
    }
}

fn gfe() -> HostCapabilities {
    HostCapabilities {
        app_version: [7, 1, 431, 0],
        feature_flags: 0,
        ..sunshine()
    }
}

fn queue(caps: &HostCapabilities) -> InputQueue {
    let mut q = InputQueue::new(caps);
    let mut out = Vec::new();
    q.poll(Instant::now(), false, &mut out);
    assert_eq!(decode(&out[0].data).unwrap(), Input::Haptics(true));
    q
}

fn take(q: &mut InputQueue, now: Instant) -> Vec<Input> {
    let mut out = Vec::new();
    q.poll(now, false, &mut out);
    out.iter().map(|p| decode(&p.data).unwrap()).collect()
}

/// A sent packet: a controller identity (0x55000080), read with the shared decoder the
/// host uses for it (the host core these tests pin predates the packet), or any other
/// input as the host core decodes it.
#[derive(Debug)]
enum Sent {
    Identity(rubylight_protocol::controller_identity::ControllerIdentity),
    Host(Input),
}

fn classify(data: &[u8]) -> Sent {
    use rubylight_protocol::controller_identity::{ControllerIdentity, CONTROLLER_IDENTITY_MAGIC};
    if data.len() >= 8 && data[4..8] == CONTROLLER_IDENTITY_MAGIC.to_le_bytes() {
        Sent::Identity(ControllerIdentity::decode(data).unwrap())
    } else {
        Sent::Host(decode(data).unwrap())
    }
}

#[test]
fn touchpad_contacts_go_only_to_a_host_that_takes_them() {
    let contact = TouchpadContact { event: touch_event::MOVE, pointer_id: 1, x: 0.5, y: 0.5, pressure: 0.0 };
    let mut q = queue(&sunshine());
    assert_eq!(q.send_touchpad_frame(&[contact], 0, 0, 0), Err(InputError::Unsupported));
    assert_eq!(q.send_touchpad(contact, 0, 0, 0), Err(InputError::Unsupported));

    let caps = HostCapabilities {
        feature_flags: feature::TOUCHPAD_EVENTS | feature::TOUCHPAD_FRAME_EVENTS,
        ..sunshine()
    };
    let mut q = queue(&caps);
    assert_eq!(q.send_touchpad_frame(&[contact; 6], 0, 0, 0), Err(InputError::Invalid));
    // Moves drop for newer ones; the click and its release, or a new contact, never.
    q.send_touchpad_frame(&[contact; 2], 0, 105, 70).unwrap();
    q.send_touchpad_frame(&[contact], touchpad::BUTTON_PRIMARY, 105, 70).unwrap();
    q.send_touchpad_frame(&[contact], touchpad::BUTTON_PRIMARY, 105, 70).unwrap();
    q.send_touchpad(contact, 0, 105, 70).unwrap();
    q.send_touchpad(TouchpadContact { event: touch_event::DOWN, ..contact }, 0, 105, 70).unwrap();
    let mut out = Vec::new();
    q.poll(Instant::now(), false, &mut out);
    let sent: Vec<(u32, Delivery)> = out
        .iter()
        .map(|p| (u32::from_le_bytes(p.data[4..8].try_into().unwrap()), p.delivery))
        .collect();
    use super::super::packets::magic;
    assert_eq!(
        sent,
        [
            (magic::TOUCHPAD_FRAME, Delivery::Unreliable),
            (magic::TOUCHPAD_FRAME, Delivery::Reliable),
            (magic::TOUCHPAD_FRAME, Delivery::Unreliable),
            (magic::TOUCHPAD, Delivery::Reliable),
            (magic::TOUCHPAD, Delivery::Reliable),
        ]
    );
    assert!(out.iter().all(|p| p.channel == crate::control::wire::channel::TOUCH));
}

#[test]
fn relative_motion_sums_and_splits() {
    let mut q = queue(&sunshine());
    let t0 = Instant::now();
    q.send_mouse_move(30_000, 1).unwrap();
    q.send_mouse_move(30_000, -1).unwrap();
    q.send_mouse_move(0, 0).unwrap();
    assert_eq!(q.len(), 1);
    assert_eq!(
        take(&mut q, t0),
        [
            Input::Relative { x: 32767, y: 0 },
            Input::Relative { x: 27233, y: 0 }
        ]
    );
    // Within the batching interval the next motion waits and keeps gathering.
    q.send_mouse_move(5, 5).unwrap();
    let mut out = Vec::new();
    assert_eq!(
        q.poll(t0, false, &mut out),
        Some(t0 + MOTION_BATCHING_INTERVAL)
    );
    assert!(out.is_empty());
    q.send_mouse_move(1, 1).unwrap();
    assert_eq!(
        take(&mut q, t0 + MOTION_BATCHING_INTERVAL),
        [Input::Relative { x: 6, y: 6 }]
    );
}

#[test]
fn unbatched_motion_does_not_wait() {
    let mut q = queue(&sunshine());
    q.set_unbatched(true);
    let t0 = Instant::now();
    q.send_mouse_move(5, 5).unwrap();
    assert_eq!(take(&mut q, t0), [Input::Relative { x: 5, y: 5 }]);
    // The same instant: batched input would wait the interval, unbatched sends now.
    q.send_mouse_move(1, 1).unwrap();
    let mut out = Vec::new();
    assert_eq!(q.poll(t0, false, &mut out), None);
    assert_eq!(out.len(), 1);
}

/// With the fast-mouse floor, unbatched relative motion from an 8 kHz mouse is merged up to
/// the floor, and a 1 kHz report that comes early by jitter still leaves at once.
#[test]
fn a_relative_motion_floor_merges_a_fast_mouse_but_never_delays_a_slow_one() {
    let mut q = queue(&sunshine());
    q.set_unbatched(true);
    q.set_relative_motion_floor(FAST_MOUSE_FLOOR);
    let t0 = Instant::now();
    q.send_mouse_move(2, 0).unwrap();
    assert_eq!(take(&mut q, t0), [Input::Relative { x: 2, y: 0 }]);
    // 8 kHz: reports every 125 us wait for the floor and leave as one packet.
    let step = Duration::from_micros(125);
    for i in 1..4 {
        q.send_mouse_move(1, 0).unwrap();
        let mut out = Vec::new();
        assert_eq!(q.poll(t0 + step * i, false, &mut out), Some(t0 + FAST_MOUSE_FLOOR));
        assert!(out.is_empty());
    }
    let sent = t0 + FAST_MOUSE_FLOOR;
    assert_eq!(take(&mut q, sent), [Input::Relative { x: 3, y: 0 }]);
    // 1 kHz with jitter: a report 0.6 ms after the last packet, then one 0.95 ms after that.
    q.send_mouse_move(1, 0).unwrap();
    let early = sent + Duration::from_micros(600);
    assert_eq!(take(&mut q, early), [Input::Relative { x: 1, y: 0 }]);
    q.send_mouse_move(1, 0).unwrap();
    assert_eq!(take(&mut q, early + Duration::from_micros(950)), [Input::Relative { x: 1, y: 0 }]);
    // Absolute positions keep the unbatched pace.
    q.send_mouse_position(10, 10, 100, 100).unwrap();
    assert_eq!(take(&mut q, early + Duration::from_micros(950)).len(), 1);
}

#[test]
fn batched_motion_still_waits_by_default() {
    let mut q = queue(&sunshine());
    let t0 = Instant::now();
    q.send_mouse_move(5, 5).unwrap();
    assert_eq!(take(&mut q, t0).len(), 1);
    q.send_mouse_move(1, 1).unwrap();
    let mut out = Vec::new();
    assert_eq!(
        q.poll(t0, false, &mut out),
        Some(t0 + MOTION_BATCHING_INTERVAL)
    );
    assert!(out.is_empty());
}

#[test]
fn a_disabled_queue_sends_nothing() {
    let mut q = InputQueue::new(&sunshine());
    assert_ne!(q.len(), 0, "the haptics enable is queued");
    q.disable();
    assert_eq!(q.len(), 0);
    assert!(q.send_mouse_move(1, 1).is_err());
    assert!(q.send_keyboard(0x41, true, 0, 0).is_err());
    let mut out = Vec::new();
    q.poll(Instant::now(), false, &mut out);
    assert!(out.is_empty());
}

#[test]
fn absolute_motion_sends_the_latest_position() {
    let mut q = queue(&sunshine());
    let t0 = Instant::now();
    q.send_mouse_position(10, 10, 1920, 1080).unwrap();
    q.send_mouse_position(20, 30, 1920, 1080).unwrap();
    assert_eq!(
        take(&mut q, t0),
        [Input::Absolute {
            x: 20,
            y: 30,
            width: 1919,
            height: 1079
        }]
    );
    q.send_mouse_move_as_position(5, -40, 1920, 1080).unwrap();
    let later = t0 + MOTION_BATCHING_INTERVAL;
    assert_eq!(
        take(&mut q, later),
        [Input::Absolute {
            x: 25,
            y: 0,
            width: 1919,
            height: 1079
        }]
    );
}

#[test]
fn controllers_batch_until_buttons_change() {
    let mut q = queue(&sunshine());
    let t0 = Instant::now();
    q.send_controller(1, 3, 0, 0, 0, (100, 0), (0, 0)).unwrap();
    q.send_controller(1, 3, 0, 0, 0, (200, 0), (0, 0)).unwrap();
    q.send_controller(0, 3, button::A, 0, 0, (0, 0), (0, 0))
        .unwrap();
    q.send_controller(1, 3, 0, 9, 0, (300, 0), (0, 0)).unwrap();
    q.send_controller(1, 3, button::B, 0, 0, (400, 0), (0, 0))
        .unwrap();
    q.send_controller(1, 3, button::B, 0, 0, (500, 0), (0, 0))
        .unwrap();
    let sent = take(&mut q, t0);
    let sticks: Vec<(u16, i16)> = sent
        .iter()
        .map(|i| match i {
            Input::Controller { id, sticks, .. } => (*id, sticks[0]),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(sticks, [(1, 300), (0, 0), (1, 500)]);
    // A state already taken is not updated any more.
    q.send_controller(1, 3, button::B, 0, 0, (600, 0), (0, 0))
        .unwrap();
    assert_eq!(take(&mut q, t0).len(), 1);
}

#[test]
fn gfe_rules() {
    let mut q = queue(&gfe());
    let t0 = Instant::now();
    q.send_controller(
        6,
        0xFF,
        button::MISC | button::PADDLE1,
        0,
        0,
        (0, 0),
        (0, 0),
    )
    .unwrap();
    q.send_keyboard(0xA1, true, super::super::packets::modifier::SHIFT | 0x10, 7)
        .unwrap();
    q.send_keyboard(0x5B, true, super::super::packets::modifier::META, 0)
        .unwrap();
    assert_eq!(q.send_high_res_hscroll(10), Err(InputError::Unsupported));
    assert_eq!(
        q.send_touch(1, 1, 0., 0., 0., 0., 0., 0),
        Err(InputError::Unsupported)
    );
    assert_eq!(
        q.send_controller_battery(0, 1, 1),
        Err(InputError::Unsupported)
    );
    q.send_high_res_scroll(60).unwrap();
    q.send_high_res_scroll(90).unwrap();
    q.send_high_res_scroll(-10).unwrap();
    let sent = take(&mut q, t0);
    assert_eq!(
        sent[0],
        Input::Controller {
            id: 2,
            active: 0xF,
            buttons: button::SPECIAL,
            left_trigger: 0,
            right_trigger: 0,
            sticks: [0; 4]
        }
    );
    assert_eq!(
        sent[1],
        Input::Keyboard {
            key: 0xA1,
            modifiers: 0,
            flags: 0,
            down: true
        }
    );
    assert_eq!(
        sent[2],
        Input::Keyboard {
            key: 0x5B,
            modifiers: 0,
            flags: 0,
            down: true
        }
    );
    assert_eq!(
        sent[3],
        Input::Scroll {
            amount: 120,
            horizontal: false
        }
    );
    assert_eq!(sent.len(), 4);
}

#[test]
fn arrival_sends_a_neutral_state_too() {
    let mut q = queue(&sunshine());
    q.send_controller_arrival(17, 2, 2, 0xFFFF, capability::DUAL_TOUCHPAD)
        .unwrap();
    let sent = take(&mut q, Instant::now());
    assert_eq!(
        sent[0],
        Input::Arrival {
            id: 1,
            kind: 2,
            capabilities: 0x108,
            buttons: 0xFFFF
        }
    );
    assert!(matches!(
        sent[1],
        Input::Controller {
            id: 1,
            active: 2,
            buttons: 0,
            ..
        }
    ));
    let mut g = queue(&gfe());
    g.send_controller_arrival(0, 1, 1, 0, 0).unwrap();
    assert_eq!(take(&mut g, Instant::now()).len(), 1);
}

#[test]
fn identities_and_the_c_button_go_only_to_a_host_that_takes_them() {
    use rubylight_protocol::controller_identity::ControllerIdentity;
    let mut q = queue(&sunshine());
    q.send_controller_identity(1, 0x057E, 0x2069).unwrap();
    q.send_controller(1, 2, button::C | button::A, 0, 0, (0, 0), (0, 0))
        .unwrap();
    let sent = take(&mut q, Instant::now());
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(matches!(
        sent[0],
        Input::Controller {
            buttons: button::A,
            ..
        }
    ));

    q.set_controller_identity(true);
    q.send_controller_identity(17, 0x057E, 0x2069).unwrap();
    q.send_controller_arrival(1, 2, 3, 0xFFFF, 0).unwrap();
    q.send_controller(1, 2, button::C, 0, 0, (0, 0), (0, 0))
        .unwrap();
    let mut out = Vec::new();
    q.poll(Instant::now(), false, &mut out);
    let packets: Vec<Sent> = out.iter().map(|p| classify(&p.data)).collect();
    assert!(
        matches!(
            packets[0],
            Sent::Identity(ControllerIdentity {
                controller: 1,
                vendor: 0x057E,
                product: 0x2069
            })
        ),
        "{packets:?}"
    );
    assert!(matches!(packets[1], Sent::Host(Input::Arrival { id: 1, .. })), "{packets:?}");
    assert!(matches!(
        packets.last().unwrap(),
        Sent::Host(Input::Controller {
            buttons: button::C,
            ..
        })
    ));
}

#[test]
fn a_held_identity_goes_before_its_arrival() {
    let mut q = queue(&sunshine());
    q.set_controller_identity(true);
    let t0 = Instant::now();
    let mut out = Vec::new();
    q.send_controller_identity(0, 0x057E, 0x2069).unwrap();
    q.send_controller_arrival(0, 1, 3, 0xFFFF, 0).unwrap();
    q.poll(t0, true, &mut out);
    assert!(out.is_empty());
    q.poll(t0, false, &mut out);
    let packets: Vec<Sent> = out.iter().map(|p| classify(&p.data)).collect();
    assert!(matches!(packets[0], Sent::Identity(_)), "{packets:?}");
    assert!(matches!(packets[1], Sent::Host(Input::Arrival { id: 0, .. })), "{packets:?}");
}

#[test]
fn motion_keeps_the_latest_reading_and_parks_the_gyro_reliably() {
    let mut q = queue(&sunshine());
    let t0 = Instant::now();
    assert_eq!(
        q.send_controller_motion(0, 3, 0., 0., 0.),
        Err(InputError::Invalid)
    );
    assert_eq!(
        q.send_controller_motion(0, 0, 0., 0., 0.),
        Err(InputError::Invalid)
    );
    q.send_controller_motion(0, motion_type::ACCEL, 1., 2., 3.)
        .unwrap();
    q.send_controller_motion(0, motion_type::ACCEL, 4., 5., 6.)
        .unwrap();
    q.send_controller_motion(0, motion_type::GYRO, 0., 0., 0.)
        .unwrap();
    let mut out = Vec::new();
    q.poll(t0, false, &mut out);
    assert_eq!(out.len(), 2);
    assert_eq!(
        decode(&out[0].data).unwrap(),
        Input::Motion {
            id: 0,
            kind: 1,
            xyz: [4., 5., 6.]
        }
    );
    assert_eq!(out[0].delivery, Delivery::Unreliable);
    assert_eq!(out[0].channel, 0x20);
    assert_eq!(out[1].delivery, Delivery::Reliable);
}

/// A pen's roll goes as 0x5500000B only to a host that takes it; otherwise, and for a
/// roll out of range, the pen packet stays the stock one.
#[test]
fn the_barrel_roll_goes_only_where_it_is_offered() {
    let magics = |offered: bool, roll: u16| {
        let mut q = queue(&sunshine());
        q.set_pen_barrel_roll(offered);
        q.send_pen(touch_event::DOWN, 1, 0, 0.5, 0.5, 0.5, 0., 0., 0, 0, roll).unwrap();
        let mut out = Vec::new();
        q.poll(Instant::now(), false, &mut out);
        assert_eq!(out.len(), 1);
        (u32::from_le_bytes(out[0].data[4..8].try_into().unwrap()), out[0].data.len())
    };
    assert_eq!(magics(false, 90), (0x5500_0003, 36));
    assert_eq!(magics(true, pen::ROLL_UNKNOWN), (0x5500_0003, 36));
    assert_eq!(magics(true, 360), (0x5500_0003, 36));
    assert_eq!(magics(true, 90), (pen::PEN_BARREL_ROLL_MAGIC, 40));
}

#[test]
fn pen_moves_collapse_and_button_changes_go_reliably() {
    let mut q = queue(&sunshine());
    let t0 = Instant::now();
    let pen = |q: &mut InputQueue, event, buttons, x| {
        q.send_pen(event, 1, buttons, x, 0.5, 0.5, 0., 0., 0, 0, pen::ROLL_UNKNOWN)
    };
    pen(&mut q, touch_event::DOWN, 0, 0.1).unwrap();
    pen(&mut q, touch_event::MOVE, 0, 0.2).unwrap();
    pen(&mut q, touch_event::MOVE, 0, 0.3).unwrap();
    pen(&mut q, touch_event::MOVE, 1, 0.4).unwrap();
    let mut out = Vec::new();
    q.poll(t0, false, &mut out);
    let xs: Vec<(f32, Delivery)> = out
        .iter()
        .map(|p| match decode(&p.data).unwrap() {
            Input::Pen { x, .. } => (x, p.delivery),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(xs, [(0.1, Delivery::Reliable), (0.3, Delivery::Unreliable)]);
    let mut out = Vec::new();
    assert_eq!(
        q.poll(t0, false, &mut out),
        Some(t0 + MOTION_BATCHING_INTERVAL)
    );
    q.poll(t0 + MOTION_BATCHING_INTERVAL, false, &mut out);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].delivery, Delivery::Reliable);
}

/// A move that changes the buttons, collapsed into a newer move with the same
/// buttons, still goes reliably: the newer one now carries the change.
#[test]
fn a_pen_button_change_stays_reliable_when_a_newer_move_replaces_it() {
    let mut q = queue(&sunshine());
    let t0 = Instant::now();
    let pen = |q: &mut InputQueue, event, buttons, x| {
        q.send_pen(event, 1, buttons, x, 0.5, 0.5, 0., 0., 0, 0, pen::ROLL_UNKNOWN)
    };
    pen(&mut q, touch_event::DOWN, 0, 0.1).unwrap();
    pen(&mut q, touch_event::MOVE, 1, 0.2).unwrap();
    pen(&mut q, touch_event::MOVE, 1, 0.3).unwrap();
    let mut out = Vec::new();
    q.poll(t0, false, &mut out);
    q.poll(t0 + MOTION_BATCHING_INTERVAL, false, &mut out);
    let xs: Vec<(f32, Delivery)> = out
        .iter()
        .map(|p| match decode(&p.data).unwrap() {
            Input::Pen { x, .. } => (x, p.delivery),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(xs, [(0.1, Delivery::Reliable), (0.3, Delivery::Reliable)]);
}

#[test]
fn text_waits_then_goes_one_code_point_per_packet() {
    let mut q = queue(&sunshine());
    let t0 = Instant::now();
    q.send_keyboard(0x41, true, 0, 0).unwrap();
    q.send_text("aé€😀").unwrap();
    let mut out = Vec::new();
    assert_eq!(q.poll(t0, false, &mut out), Some(t0 + TEXT_SETTLE));
    assert_eq!(out.len(), 1);
    let sent = take(&mut q, t0 + TEXT_SETTLE);
    assert_eq!(
        sent,
        ["a", "é", "€", "😀"]
            .map(|s| Input::Text(s.into()))
            .to_vec()
    );
    assert_eq!(code_points(&[b'a', 0xFF, b'b']), vec![&b"a"[..]]);
}

#[test]
fn silent_host_input_is_held_then_replaced_by_the_current_state() {
    let mut q = queue(&sunshine());
    let t0 = Instant::now();
    let mut out = Vec::new();
    q.send_keyboard(0x41, true, 0, 0).unwrap();
    q.send_keyboard(0x42, true, 0, 0).unwrap();
    q.send_keyboard(0x41, false, 0, 0).unwrap();
    q.send_mouse_button(1, true).unwrap();
    q.send_mouse_move(10, 10).unwrap();
    q.send_touch(touch_event::DOWN, 1, 0.5, 0.5, 1., 0., 0., 0)
        .unwrap();
    q.send_controller(0, 1, button::A, 0, 0, (5, 5), (0, 0))
        .unwrap();
    q.send_text("x").unwrap();
    q.poll(t0, true, &mut out);
    assert!(out.is_empty());
    assert!(q.is_holding());
    assert!(q.held_count() >= 7);

    q.poll(t0, false, &mut out);
    let sent: Vec<Input> = out.iter().map(|p| decode(&p.data).unwrap()).collect();
    assert_eq!(
        sent,
        [
            Input::Keyboard {
                key: 0x41,
                modifiers: 0,
                flags: 0,
                down: false
            },
            Input::Keyboard {
                key: 0x42,
                modifiers: 0,
                flags: 0,
                down: true
            },
            Input::MouseButton {
                button: 1,
                down: true
            },
            Input::Controller {
                id: 0,
                active: 1,
                buttons: button::A,
                left_trigger: 0,
                right_trigger: 0,
                sticks: [5, 5, 0, 0]
            },
            Input::Touch {
                event: 7,
                id: 1,
                x: 0.5,
                y: 0.5,
                pressure: 1.,
                major: 0.,
                minor: 0.,
                rotation: 0
            },
        ]
    );
    assert_eq!(out.last().unwrap().delivery, Delivery::Reliable);
    assert!(!q.is_holding());
}

#[test]
fn stopped_and_full_queues_refuse_input() {
    let mut q = queue(&sunshine());
    for i in 0..MAX_QUEUED {
        q.send_mouse_button(1, i % 2 == 0).unwrap();
    }
    assert_eq!(q.send_mouse_button(1, true), Err(InputError::QueueFull));
    let mut out = Vec::new();
    q.drain(Instant::now(), false, &mut out);
    assert_eq!(out.len(), MAX_QUEUED);
    q.stop();
    assert_eq!(q.send_keyboard(1, true, 0, 0), Err(InputError::Stopped));
}
