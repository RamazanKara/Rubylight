//! Unit tests for [`InputQueue`] mouse motion, scrolling, keyboard, text ordering,
//! backpressure and grace-period state holding.

mod devices;

use super::poll::code_points;
use super::*;
use crate::control::negotiation::{encryption, feature};
use crate::input::packets::button;
use rubylight_protocol::pen;
use rubylight_wire::input::{decode, Input};

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
fn a_release_is_never_dropped_for_a_full_queue() {
    let mut q = queue(&sunshine());
    for _ in 0..MAX_QUEUED {
        q.send_touch(touch_event::MOVE, 1, 0.5, 0.5, 1., 0., 0., 0).unwrap();
    }
    q.send_mouse_button(1, true).expect("a move gives way to a press");
    let mut out = Vec::new();
    q.drain(Instant::now(), false, &mut out);
    assert_eq!(out.len(), MAX_QUEUED, "the queue stays bounded");
    assert_eq!(decode(&out.last().unwrap().data).unwrap(), Input::MouseButton { button: 1, down: true });
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

// A trackpad sends a fraction of a notch per event: the fraction is carried to the next turn,
// not truncated away, so a slow scroll still reaches the PC.
#[test]
fn a_wheel_turn_smaller_than_a_unit_is_carried_to_the_next() {
    let mut q = queue(&gfe());
    for _ in 0..999 {
        q.send_wheel_turn(0.0, 0.001).unwrap();
    }
    let mut out = Vec::new();
    q.poll(Instant::now(), false, &mut out);
    assert!(out.is_empty(), "999 turns of 1/1000 notch are 119.88 of the 120 in a click");
    q.send_wheel_turn(0.0, 0.001).unwrap();
    out.clear();
    q.poll(Instant::now(), false, &mut out);
    assert_eq!(out.len(), 1);
    assert_eq!(decode(&out[0].data).unwrap(), Input::Scroll { amount: 120, horizontal: false });
}

// On a host that takes whole clicks only, the accumulator owns the clicks of a turn whose send
// failed: the next turn must not send them a second time.
#[test]
fn a_failed_wheel_send_does_not_scroll_twice_on_the_next_turn() {
    let mut q = queue(&gfe());
    for i in 0..MAX_QUEUED {
        q.send_mouse_button(1, i % 2 == 0).unwrap();
    }
    assert_eq!(q.send_wheel_turn(0.0, 1.5), Err(InputError::QueueFull));
    let mut out = Vec::new();
    q.drain(Instant::now(), false, &mut out);
    out.clear();
    q.send_wheel_turn(0.0, 0.5).unwrap();
    q.drain(Instant::now(), false, &mut out);
    let sent: i32 = out
        .iter()
        .map(|p| match decode(&p.data).unwrap() {
            Input::Scroll { amount, .. } => i32::from(amount),
            other => panic!("unexpected {other:?}"),
        })
        .sum();
    assert_eq!(sent, 240, "two notches were turned, two clicks go out");
}

// The vertical axis goes first: a host that refuses horizontal scroll (GFE) must not cost the
// turn's scroll.
#[test]
fn a_wheel_turn_sends_the_vertical_axis_when_the_horizontal_one_is_refused() {
    let mut q = queue(&gfe());
    assert_eq!(q.send_wheel_turn(1.0, 1.0), Err(InputError::Unsupported), "GFE takes no horizontal scroll");
    let mut out = Vec::new();
    q.poll(Instant::now(), false, &mut out);
    assert_eq!(out.len(), 1, "the vertical click went out");
    assert_eq!(decode(&out[0].data).unwrap(), Input::Scroll { amount: 120, horizontal: false });
}

// A full queue keeps a small reserve for releases: a burst of presses cannot take the room a
// key or button going up needs.
#[test]
fn a_release_fits_the_reserve_above_a_full_queue() {
    let mut q = queue(&sunshine());
    for i in 0..MAX_QUEUED {
        q.send_mouse_button(1, i % 2 == 0).unwrap();
    }
    assert_eq!(q.send_keyboard(0x41, true, 0, 0), Err(InputError::QueueFull), "a press does not take the reserve");
    q.send_keyboard(0x41, false, 0, 0).expect("a release fits its reserve");
    let mut out = Vec::new();
    q.drain(Instant::now(), false, &mut out);
    assert_eq!(out.len(), MAX_QUEUED + 1, "one release above the bound");
    assert!(matches!(decode(&out.last().unwrap().data).unwrap(), Input::Keyboard { key: 0x41, down: false, .. }));
}

#[test]
fn discrete_events_flush_preceding_batched_motion_without_waiting() {
    let mut q = queue(&sunshine());
    let t0 = Instant::now();

    // Prime last_mouse and last_pen so the batch interval would normally apply.
    q.send_mouse_move(1, 0).unwrap();
    q.send_pen(touch_event::MOVE, 1, 0, 0.1, 0.5, 0.5, 0., 0., 0, 0, pen::ROLL_UNKNOWN)
        .unwrap();
    assert_eq!(take(&mut q, t0).len(), 2);

    // Relative mouse move followed by a click flushes both immediately in order.
    q.send_mouse_move(4, 5).unwrap();
    q.send_mouse_button(1, true).unwrap();
    let mut out = Vec::new();
    assert_eq!(q.poll(t0, false, &mut out), None);
    let sent: Vec<Input> = out.iter().map(|p| decode(&p.data).unwrap()).collect();
    assert_eq!(
        sent,
        [
            Input::Relative { x: 4, y: 5 },
            Input::MouseButton {
                button: 1,
                down: true
            }
        ]
    );

    // Absolute mouse position followed by a click flushes both immediately.
    q.send_mouse_position(10, 20, 100, 100).unwrap();
    q.send_mouse_button(1, false).unwrap();
    out.clear();
    assert_eq!(q.poll(t0, false, &mut out), None);
    assert_eq!(out.len(), 2);

    // Pen move followed by a pen button change flushes the move immediately.
    q.send_pen(touch_event::MOVE, 1, 0, 0.2, 0.5, 0.5, 0., 0., 0, 0, pen::ROLL_UNKNOWN)
        .unwrap();
    q.send_pen(touch_event::UP, 1, 0, 0.2, 0.5, 0.5, 0., 0., 0, 0, pen::ROLL_UNKNOWN)
        .unwrap();
    out.clear();
    assert_eq!(q.poll(t0, false, &mut out), None);
    assert_eq!(out.len(), 2);
}

#[test]
fn pending_text_does_not_block_unrelated_channels_while_preserving_keyboard_order() {
    let mut q = queue(&sunshine());
    let t0 = Instant::now();
    q.send_text("hi").unwrap();
    q.send_mouse_button(1, true).unwrap();
    q.send_controller(0, 1, button::A, 0, 0, (0, 0), (0, 0)).unwrap();
    q.send_keyboard(0x41, true, 0, 0).unwrap();
    q.send_mouse_button(1, false).unwrap();

    // Mouse press and controller before the keyboard event bypass the waiting text;
    // the keyboard event and the click queued behind it wait for text to settle.
    let mut out = Vec::new();
    assert_eq!(q.poll(t0, false, &mut out), Some(t0 + TEXT_SETTLE));
    let first: Vec<Input> = out.iter().map(|p| decode(&p.data).unwrap()).collect();
    assert!(matches!(
        first.as_slice(),
        [
            Input::MouseButton { button: 1, down: true },
            Input::Controller { buttons: button::A, .. },
        ]
    ), "{first:?}");

    let rest = take(&mut q, t0 + TEXT_SETTLE);
    assert_eq!(
        rest,
        [
            Input::Text("h".into()),
            Input::Text("i".into()),
            Input::Keyboard {
                key: 0x41,
                modifiers: 0,
                flags: 0,
                down: true
            },
            Input::MouseButton {
                button: 1,
                down: false
            }
        ]
    );
}
