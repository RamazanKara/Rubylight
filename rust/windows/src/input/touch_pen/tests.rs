use super::*;
fn touch_frame(
    touches: &BTreeMap<u32, POINTER_TOUCH_INFO>,
    id: u32,
    event: u8,
    location: POINT,
) -> Option<POINTER_TOUCH_INFO> {
    let mut frame = touch_start(touches, id, event).unwrap()?;
    pointer_event(&mut frame.pointerInfo, event, location);
    Some(frame)
}
#[test]
fn a_touch_move_needs_a_successful_down_and_refused_contacts_own_no_slot() {
    let mut touches = BTreeMap::new();
    let location = POINT { x: 10, y: 20 };
    // A lost down, or a move arriving after cancel-all, must not poison
    // every later frame with an UPDATE for an unknown Windows contact.
    assert!(touch_frame(&touches, 8, 3, location).is_none());
    for event in [0, 1] {
        let frame = touch_frame(&touches, 8, event, location).unwrap();
        assert!(
            submit_touch(&mut touches, 8, frame, event, |_| {
                bail!("injection refused")
            })
            .is_err()
        );
        assert!(touches.is_empty());
        assert_eq!(
            touch_start(&touches, 9, 1)
                .unwrap()
                .unwrap()
                .pointerInfo
                .pointerId,
            1
        );
    }
    let hover = touch_frame(&touches, 8, 0, location).unwrap();
    submit_touch(&mut touches, 8, hover, 0, |_| Ok(())).unwrap();
    assert!(touch_frame(&touches, 8, 3, location).is_none());
    let down = touch_frame(&touches, 8, 1, location).unwrap();
    assert_ne!(
        down.pointerInfo.pointerFlags & POINTER_FLAG_DOWN,
        POINTER_FLAG_NONE
    );
    submit_touch(&mut touches, 8, down, 1, |_| Ok(())).unwrap();
    assert!(touch_frame(&touches, 8, 3, location).is_some());
}
#[test]
fn a_refused_move_keeps_the_last_accepted_position_for_release() {
    let mut touches = BTreeMap::new();
    let original = POINT { x: 10, y: 20 };
    let down = touch_frame(&touches, 8, 1, original).unwrap();
    submit_touch(&mut touches, 8, down, 1, |_| Ok(())).unwrap();
    let moved = touch_frame(&touches, 8, 3, POINT { x: 30, y: 40 }).unwrap();
    assert!(submit_touch(&mut touches, 8, moved, 3, |_| bail!("busy")).is_err());
    assert_eq!(touches[&8].pointerInfo.ptPixelLocation, original);
    let up = touch_frame(&touches, 8, 2, POINT::default()).unwrap();
    assert_eq!(up.pointerInfo.ptPixelLocation, original);
    // A failed up retains the accepted contact for cancellation fallback.
    assert!(submit_touch(&mut touches, 8, up, 2, |_| bail!("busy")).is_err());
    assert_eq!(touches.len(), 1);
    submit_touch(&mut touches, 8, up, 2, |_| Ok(())).unwrap();
    assert!(touches.is_empty());
}
#[test]
fn failed_batch_cancellation_attempts_every_contact_and_clears_state() {
    let mut touches = BTreeMap::new();
    for id in 1..=3 {
        let frame = touch_frame(
            &touches,
            id,
            1,
            POINT {
                x: id as i32,
                y: 20,
            },
        )
        .unwrap();
        submit_touch(&mut touches, id, frame, 1, |_| Ok(())).unwrap();
    }
    let mut attempted = Vec::new();
    let result = cancel_touch_contacts(&mut touches, |contacts| {
        let ids: Vec<_> = contacts
            .iter()
            .map(|contact| {
                // SAFETY: cancel_touch_contacts creates initialized touch frames.
                let touch = unsafe { contact.Anonymous.touchInfo };
                assert_eq!(
                    touch.pointerInfo.ptPixelLocation.x,
                    touch.pointerInfo.pointerId as i32
                );
                assert_eq!(
                    touch.pointerInfo.pointerFlags,
                    POINTER_FLAG_UP | POINTER_FLAG_CANCELED
                );
                touch.pointerInfo.pointerId
            })
            .collect();
        let rejected = ids.contains(&2);
        attempted.push(ids);
        if rejected {
            bail!("stale contact");
        }
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(attempted, [vec![1, 2, 3], vec![1], vec![2], vec![3]]);
    assert!(touches.is_empty());
    assert!(touch_frame(&touches, 1, 3, POINT::default()).is_none());
    cancel_touch_contacts(&mut touches, |_| panic!("empty state must not inject")).unwrap();
}
#[test]
fn cancellation_uses_one_batch_when_accepted_and_can_recover_a_failed_batch() {
    for fail_batch in [false, true] {
        let mut touches = BTreeMap::new();
        for id in 1..=2 {
            let hover = touch_frame(&touches, id, 0, POINT::default()).unwrap();
            submit_touch(&mut touches, id, hover, 0, |_| Ok(())).unwrap();
        }
        let mut calls = 0;
        cancel_touch_contacts(&mut touches, |contacts| {
            calls += 1;
            if fail_batch && calls == 1 {
                bail!("batch refused");
            }
            for contact in contacts {
                // SAFETY: The helper constructs only initialized touch frames.
                let flags = unsafe { contact.Anonymous.touchInfo.pointerInfo.pointerFlags };
                assert_eq!(flags, POINTER_FLAG_UPDATE | POINTER_FLAG_CANCELED);
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(calls, if fail_batch { 3 } else { 1 });
        assert!(touches.is_empty());
    }
}
#[test]
fn native_pointer_rectangles_use_the_virtual_screen_origin() {
    let primary = RECT {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };
    assert_eq!(pointer_rect(primary, POINT::default()), primary);
    let origin = POINT { x: -1920, y: -1080 };
    assert_eq!(
        pointer_rect(primary, origin),
        RECT {
            left: 1920,
            top: 1080,
            right: 3840,
            bottom: 2160
        }
    );
    assert_eq!(
        pointer_rect(
            RECT {
                left: -1920,
                top: 0,
                right: 0,
                bottom: 1080
            },
            origin
        ),
        RECT {
            left: 0,
            top: 1080,
            right: 1920,
            bottom: 2160
        }
    );
    assert_eq!(
        pointer_rect(
            RECT {
                left: 0,
                top: -1080,
                right: 1920,
                bottom: 0
            },
            origin
        ),
        RECT {
            left: 1920,
            top: 0,
            right: 3840,
            bottom: 1080
        }
    );
}
#[test]
fn native_pointer_locations_keep_letterboxing_and_display_edges() {
    let mut injector = Injector::new(r"\\.\DISPLAY99", "vhf").unwrap();
    let display = RECT {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };
    injector.rect = Some(display);
    injector.set_stream_size(1920, 1440);
    for origin in [POINT::default(), POINT { x: -1920, y: -1080 }] {
        let rect = pointer_rect(display, origin);
        assert_eq!(
            injector.location(rect, 0., 0.125),
            POINT {
                x: rect.left,
                y: rect.top
            }
        );
        assert_eq!(
            injector.location(rect, 1., 0.875),
            POINT {
                x: rect.right - 1,
                y: rect.bottom - 1
            }
        );
        assert_eq!(
            injector.location(rect, 0.5, 0.5),
            POINT {
                x: 959 - origin.x,
                y: 539 - origin.y
            }
        );
        assert_eq!(
            injector.location(rect, 0., 0.),
            POINT {
                x: rect.left,
                y: rect.top
            }
        );
        assert_eq!(
            injector.location(rect, 1., 1.),
            POINT {
                x: rect.right - 1,
                y: rect.bottom - 1
            }
        );
    }
}
#[test]
fn an_out_of_range_contact_size_stays_inside_the_display() {
    let rect = RECT {
        left: -1920,
        top: 0,
        right: 0,
        bottom: 1080,
    };
    let center = POINT { x: -960, y: 540 };
    let angle = 45.0f32.to_radians();
    for size in [1e30, f32::INFINITY, f32::MAX] {
        assert_eq!(
            contact_rect(center, rect, (size, size), angle, (1., 1.)),
            rect
        );
    }
    let point = contact_rect(center, rect, (f32::NAN, f32::NAN), angle, (1., 1.));
    assert_eq!((point.left, point.right), (-961, -959));
    assert_eq!((point.top, point.bottom), (539, 541));
    let edge = contact_rect(POINT { x: 0, y: 1080 }, rect, (1e30, 0.), 0., (1., 1.));
    assert_eq!((edge.right, edge.bottom), (0, 1080));
    let normal = contact_rect(center, rect, (0.25, 0.25), 0., (1., 1.));
    assert_eq!((normal.left, normal.right), (-1200, -720));
    assert_eq!((normal.top, normal.bottom), (405, 675));
}
#[test]
fn a_held_touch_is_repeated_while_pen_events_keep_coming() {
    use std::time::{Duration, Instant};
    let start = Instant::now();
    // A touch is held still while the pen hovers, sending an event every
    // 8 ms (each injection restarts the pen's period), for one second.
    let mut touch = start;
    let mut repeats = vec![];
    for ms in (8..=1000).step_by(8) {
        let now = start + Duration::from_millis(ms);
        let mut pen = now;
        assert!(!refresh_due(&mut pen, now));
        if refresh_due(&mut touch, now) {
            repeats.push(ms);
        }
    }
    assert_eq!(repeats, [256, 512, 768]);
    // And a hovering pen is repeated while touch events keep coming.
    let mut pen = start;
    let mut repeats = 0;
    for ms in (8..=1000).step_by(8) {
        let now = start + Duration::from_millis(ms);
        let mut touch = now;
        assert!(!refresh_due(&mut touch, now));
        repeats += usize::from(refresh_due(&mut pen, now));
    }
    assert_eq!(repeats, 3);
}
fn pen_event(pen: POINTER_PEN_INFO, event: u8, pressure: f32) -> Option<POINTER_PEN_INFO> {
    let location = POINT { x: 10, y: 20 };
    pen_frame(
        pen,
        event,
        1,
        0,
        location,
        pressure,
        u16::MAX,
        u8::MAX,
        u16::MAX,
    )
}
#[test]
fn pen_pressure_and_button_frames_match_the_cpp_host() {
    let idle = POINTER_PEN_INFO::default();
    let in_contact = POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT;
    // A button change with no active pen injects nothing.
    assert!(
        pen_frame(
            idle,
            5,
            1,
            1,
            POINT::default(),
            0.,
            u16::MAX,
            u8::MAX,
            u16::MAX
        )
        .is_none()
    );
    // A pressure of 0 in contact is passed as none.
    let down = pen_event(idle, 1, 0.).unwrap();
    assert_eq!(
        down.pointerInfo.pointerFlags,
        POINTER_FLAG_DOWN | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT
    );
    assert_eq!((down.penMask, down.pressure), (PEN_MASK_NONE, 0));
    let held = pen_after(down, 1, false);
    assert_eq!(held.pointerInfo.pointerFlags, in_contact);
    let moved = pen_event(held, 3, 0.).unwrap();
    assert_eq!((moved.penMask, moved.pressure), (PEN_MASK_NONE, 0));
    let pressed = pen_event(held, 3, 0.5).unwrap();
    assert_eq!(
        (pressed.penMask, pressed.pressure),
        (PEN_MASK_PRESSURE, 512)
    );
    // A hover's distance is never passed as pressure.
    let hover = pen_event(idle, 0, 0.5).unwrap();
    assert_eq!((hover.penMask, hover.pressure), (PEN_MASK_NONE, 0));
    // With an active pen, a button change is passed on.
    let hovering = pen_after(hover, 0, false);
    let barrel = pen_frame(
        hovering,
        5,
        1,
        1,
        POINT::default(),
        0.,
        u16::MAX,
        u8::MAX,
        u16::MAX,
    )
    .unwrap();
    assert_eq!(barrel.penFlags, PEN_FLAG_BARREL);
    assert_eq!(
        barrel.pointerInfo.pointerFlags,
        POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE
    );
    assert_eq!(barrel.pointerInfo.ptPixelLocation, POINT { x: 10, y: 20 });
}
/// A known barrel roll is injected as the pen's twist, and the tilt still follows the
/// tilt's direction; an unknown roll keeps the stock mapping.
#[test]
fn a_barrel_roll_is_injected_as_the_twist() {
    let frame = |rotation, tilt, roll| {
        let location = POINT { x: 10, y: 20 };
        pen_frame(
            POINTER_PEN_INFO::default(),
            1,
            1,
            0,
            location,
            0.5,
            rotation,
            tilt,
            roll,
        )
        .unwrap()
    };
    let tilt = PEN_MASK_TILT_X | PEN_MASK_TILT_Y;
    let stock = frame(90, 30, u16::MAX);
    assert_eq!(stock.penMask, PEN_MASK_PRESSURE | PEN_MASK_ROTATION | tilt);
    assert_eq!(stock.rotation, 90);
    for roll in [0, 45, 359] {
        let rolled = frame(90, 30, roll);
        assert_eq!(rolled.penMask, stock.penMask, "{roll}");
        assert_eq!(rolled.rotation, u32::from(roll));
        assert_eq!((rolled.tiltX, rolled.tiltY), (stock.tiltX, stock.tiltY));
        // With no tilt's direction the roll is still passed, without a tilt.
        let upright = frame(u16::MAX, u8::MAX, roll);
        assert_eq!(upright.penMask, PEN_MASK_PRESSURE | PEN_MASK_ROTATION);
        assert_eq!(upright.rotation, u32::from(roll));
    }
    assert_eq!(
        frame(u16::MAX, u8::MAX, u16::MAX).penMask,
        PEN_MASK_PRESSURE
    );
}
#[test]
fn a_pen_frame_windows_refuses_is_not_repeated_as_a_stale_frame() {
    let in_contact = POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT;
    let down = pen_event(POINTER_PEN_INFO::default(), 1, 0.5).unwrap();
    // Windows never saw the pen go down: there is nothing to repeat.
    assert_eq!(
        pen_after(down, 1, true).pointerInfo.pointerFlags,
        POINTER_FLAG_NONE
    );
    let held = pen_after(down, 1, false);
    // An ended pen is inactive whether or not its last frame went through.
    for event in [2, 4, 6, 7] {
        for failed in [false, true] {
            let frame = pen_event(held, event, 0.).unwrap();
            assert_eq!(
                pen_after(frame, event, failed).pointerInfo.pointerFlags,
                POINTER_FLAG_NONE
            );
        }
    }
    // A refused move keeps the pen's state, without edge flags.
    let moved = pen_event(held, 3, 0.5).unwrap();
    assert_eq!(
        pen_after(moved, 3, true).pointerInfo.pointerFlags,
        in_contact
    );
}
#[test]
fn an_active_pen_is_lifted_before_its_device_goes_away() {
    let idle = POINTER_PEN_INFO::default();
    assert!(pen_cancel(idle).is_none());
    let held = pen_after(pen_event(idle, 1, 0.5).unwrap(), 1, false);
    let lifted = pen_cancel(held).unwrap();
    assert_eq!(
        lifted.pointerInfo.pointerFlags,
        POINTER_FLAG_UP | POINTER_FLAG_CANCELED
    );
    assert_eq!((lifted.penMask, lifted.pressure), (PEN_MASK_NONE, 0));
    assert_eq!(
        pen_after(lifted, 7, true).pointerInfo.pointerFlags,
        POINTER_FLAG_NONE
    );
    let hovering = pen_after(pen_event(idle, 0, 0.).unwrap(), 0, false);
    assert_eq!(
        pen_cancel(hovering).unwrap().pointerInfo.pointerFlags,
        POINTER_FLAG_UPDATE | POINTER_FLAG_CANCELED
    );
}
