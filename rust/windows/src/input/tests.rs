use super::*;

#[test]
fn input_starts_before_the_stream_display_exists_and_ignores_absolute_input() {
    // Keyboard and controller input, including a controller's one-time
    // arrival, needs an injector before the stream's display exists.
    let mut injector = Injector::new(r"\\.\DISPLAY99", "vhf").unwrap();
    // Nothing below may reach Windows: there is no display to map onto.
    assert!(injector.rect.is_none());
    for event in [
        Event::Absolute {
            x: 10,
            y: 20,
            width: 100,
            height: 100,
        },
        Event::Touch {
            event: 1,
            id: 7,
            x: 0.5,
            y: 0.5,
            pressure: 0.5,
            major: 0.1,
            minor: 0.1,
            rotation: u16::MAX,
        },
        Event::Pen {
            event: 1,
            tool: 1,
            buttons: 0,
            x: 0.5,
            y: 0.5,
            pressure: 0.5,
            rotation: u16::MAX,
            tilt: u8::MAX,
            roll: u16::MAX,
        },
    ] {
        injector.apply(&event).unwrap();
    }
    assert!(injector.absolute);
    assert!(injector.touches.is_empty());
    assert!(injector.touch_device.is_none() && injector.pen_device.is_none());
    assert_eq!(injector.pen.pointerInfo.pointerFlags, POINTER_FLAG_NONE);
    // The session's display is followed while it is still missing.
    injector.set_output(r"\\.\DISPLAY98");
    assert!(injector.rect.is_none());
    assert_eq!(injector.output, r"\\.\DISPLAY98");
    assert_eq!(injector.lookup.as_ref().unwrap().output, r"\\.\DISPLAY98");
}

thread_local! {
    static CALLS: std::cell::RefCell<Vec<Vec<INPUT>>> = const { std::cell::RefCell::new(Vec::new()) };
    /// How many inputs of a call Windows takes.
    static TAKES: std::cell::Cell<usize> = const { std::cell::Cell::new(usize::MAX) };
}
/// SendInput's stand-in: nothing reaches the desktop.
fn record(inputs: &[INPUT]) -> usize {
    CALLS.with_borrow_mut(|calls| calls.push(inputs.to_vec()));
    TAKES.get().min(inputs.len())
}
fn recording() -> Injector {
    let mut injector = Injector::new(r"\\.\DISPLAY99", "vhf").unwrap();
    injector.inject = record;
    injector
}
#[test]
fn control_wait_tracks_release_and_repeat_deadlines() {
    let mut injector = recording();
    assert_eq!(injector.next_due(), None);
    injector.apply(&key(0x51, true)).unwrap();
    let repeat = injector.repeat.unwrap().3;
    assert_eq!(injector.next_due(), Some(repeat));
    let release = Instant::now() + Duration::from_millis(10);
    injector.left_release = Some(release);
    assert_eq!(injector.next_due(), Some(release.min(repeat)));
    injector.apply(&key(0x51, false)).unwrap();
    assert_eq!(injector.next_due(), Some(release));
    injector.release_all();
    assert_eq!(injector.next_due(), None);
    calls();
}
#[test]
fn rumble_feedback_obeys_forward_rumble_and_client_haptics() {
    let mut injector = recording();
    for forward in [false, true] {
        injector.policy.forward_rumble = forward;
        for haptics in [false, true] {
            injector.apply(&Event::Haptics(haptics)).unwrap();
            for kind in [0x010b, 0x5500, 0x5503, 0x5601] {
                assert_eq!(injector.feedback_allowed(kind), forward && haptics);
            }
            assert!(injector.feedback_allowed(0x5502));
        }
    }
}
fn describe(input: &INPUT) -> String {
    // SAFETY: The test inputs are keyboard or mouse INPUTs with the union field matching their type initialized.
    unsafe {
        if input.r#type == INPUT_KEYBOARD {
            let ki = input.Anonymous.ki;
            format!("key {} {} {:#x}", ki.wVk.0, ki.wScan, ki.dwFlags.0)
        } else {
            let mi = input.Anonymous.mi;
            format!(
                "mouse {} {} {} {:#x}",
                mi.dx, mi.dy, mi.mouseData, mi.dwFlags.0
            )
        }
    }
}
/// The SendInput calls made since the last look.
fn calls() -> Vec<Vec<String>> {
    CALLS
        .with_borrow_mut(std::mem::take)
        .iter()
        .map(|call| call.iter().map(describe).collect())
        .collect()
}
fn described(inputs: &[INPUT]) -> Vec<String> {
    inputs.iter().map(describe).collect()
}
fn key(key: u16, down: bool) -> Event {
    Event::Keyboard {
        key,
        modifiers: 0,
        flags: 0,
        down,
    }
}
fn click(button: u8, down: bool) -> Event {
    Event::MouseButton { button, down }
}
#[test]
fn a_trackpad_touch_profile_sends_touches_as_mouse_input() {
    use butterpollo_core::{config::TouchMode, touch_profile::Settings};
    let touch = |event, x| Event::Touch {
        event,
        id: 4,
        x,
        y: 0.5,
        pressure: 1.0,
        major: 0.0,
        minor: 0.0,
        rotation: u16::MAX,
    };
    let mut injector = recording();
    injector.set_stream_size(1000, 500);
    injector.policy.touch = Settings {
        mode: TouchMode::Trackpad,
        sensitivity: 100,
        gestures: true,
    };
    // A swipe of 0.125 of the stream's 1000 pixels moves the mouse, a tap clicks.
    let events = [touch(1, 0.25), touch(3, 0.375), touch(2, 0.375)];
    assert!(injector.apply_all(&events).is_empty());
    assert_eq!(calls(), [vec!["mouse 125 0 0 0x1".to_owned()]]);
    assert!(
        injector
            .apply_all(&[touch(1, 0.375), touch(2, 0.375)])
            .is_empty()
    );
    assert_eq!(
        calls().concat(),
        [
            described(&[Injector::button(1, true)]),
            described(&[Injector::button(1, false)])
        ]
        .concat()
    );
    assert!(injector.touches.is_empty(), "no native contact");
    // With native touch off the device emulates the mouse itself: no touch comes here.
    injector.policy.native_pen_touch = false;
    assert!(!injector.policy.allows(&touch(1, 0.25)));
}
#[test]
fn relative_mouse_releases_a_deferred_absolute_click_before_moving() {
    for batched in [false, true] {
        let mut injector = recording();
        injector.lookup = None;
        injector.rect = Some(RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        });
        let absolute = Event::Absolute {
            x: 50,
            y: 50,
            width: 100,
            height: 100,
        };
        let relative = Event::Relative { x: 3, y: 2 };
        injector.apply(&absolute).unwrap();
        let absolute_input = calls().into_iter().flatten().collect::<Vec<_>>();
        let events = [
            absolute.clone(),
            click(1, true),
            click(1, false),
            absolute.clone(),
            relative.clone(),
            click(1, true),
            absolute,
            click(1, false),
        ];
        if batched {
            assert!(injector.apply_all(&events).is_empty());
        } else {
            for event in &events {
                injector.apply(event).unwrap();
            }
        }
        // A second absolute release must not overwrite ownership retained by the first.
        *injector.left_release.as_mut().unwrap() = std::time::Instant::now();
        injector.due().unwrap();
        let first = calls().into_iter().flatten().collect::<Vec<_>>();
        injector.release_pointers().unwrap();
        let cleanup = calls();
        let retained = HELD.lock().unwrap().get(&(false, 1)).copied();
        injector.apply(&relative).unwrap();
        calls();
        injector.apply(&click(1, true)).unwrap();
        injector.apply(&click(1, false)).unwrap();
        let subsequent = calls().into_iter().flatten().collect::<Vec<_>>();
        let mut expected = absolute_input.clone();
        expected.extend(described(&[Injector::button(1, true)]));
        expected.extend(absolute_input.clone());
        expected.extend(described(&[
            Injector::button(1, false),
            Injector::mouse(3, 2, 0, MOUSEEVENTF_MOVE),
            Injector::button(1, true),
        ]));
        expected.extend(absolute_input);
        expected.extend(described(&[Injector::button(1, false)]));
        assert_eq!(
            first, expected,
            "batched={batched}; cleanup={cleanup:?}; retained={retained:?}; subsequent={subsequent:?}"
        );
        assert!(cleanup.is_empty());
        assert_eq!(retained, None);
        assert_eq!(
            subsequent,
            described(&[Injector::button(1, true), Injector::button(1, false)])
        );
        assert!(!HELD.lock().unwrap().contains_key(&(false, 1)));
    }

    let relative = Event::Relative { x: 3, y: 2 };
    let movement = Injector::mouse(3, 2, 0, MOUSEEVENTF_MOVE);
    let mut first = recording();
    let mut second = recording();
    first.absolute = true;
    first.apply(&click(1, true)).unwrap();
    first.apply(&click(1, false)).unwrap();
    assert!(first.left_release.is_some());
    second.apply(&click(1, true)).unwrap();
    calls();
    first.apply(&relative).unwrap();
    assert!(first.left_release.is_none());
    assert_eq!(calls(), [described(&[movement])]);
    assert_eq!(HELD.lock().unwrap().get(&(false, 1)), Some(&1));
    first.release_pointers().unwrap();
    assert!(calls().is_empty());
    second.apply(&click(1, false)).unwrap();
    assert_eq!(calls(), [described(&[Injector::button(1, false)])]);
    assert!(!HELD.lock().unwrap().contains_key(&(false, 1)));

    first.absolute = true;
    first.apply(&click(1, true)).unwrap();
    first.apply(&click(1, false)).unwrap();
    calls();
    TAKES.set(0);
    let errors = first.apply_all(std::slice::from_ref(&relative));
    TAKES.set(usize::MAX);
    assert_eq!(errors.len(), 2);
    assert_eq!(
        calls(),
        [
            described(&[Injector::button(1, false)]),
            described(&[movement]),
        ]
    );
    assert!(!first.absolute && first.left_release.is_none());
    assert!(!HELD.lock().unwrap().contains_key(&(false, 1)));
    first.apply(&click(1, true)).unwrap();
    first.apply(&click(1, false)).unwrap();
    assert_eq!(
        calls(),
        [
            described(&[Injector::button(1, true)]),
            described(&[Injector::button(1, false)]),
        ]
    );
    assert!(!HELD.lock().unwrap().contains_key(&(false, 1)));
}
// HELD is shared by every injector: each test below uses its own keys
// and buttons.

#[test]
fn overlapping_keybindings_keep_the_destination_held_and_repeating() {
    let mut injector = recording();
    for source in [0x70, 0x71, 0x72] {
        injector.policy.keybindings.insert(source, 0x46);
    }
    injector.apply(&key(0x70, true)).unwrap();
    assert_eq!(calls(), [described(&[injector.key_scan(0x46, true, 0)])]);
    let repeat = injector.repeat;
    let second = Event::Keyboard {
        key: 0x71,
        modifiers: 0,
        flags: 1,
        down: true,
    };
    assert!(injector.apply_all(&[second.clone(), second]).is_empty());
    // An unpressed source must not release another source's destination.
    injector.apply(&key(0x72, false)).unwrap();
    injector.apply(&key(0x70, false)).unwrap();
    assert!(calls().is_empty());
    assert_eq!(injector.key_sources, BTreeMap::from([(0x71, 0x46)]));
    assert_eq!(injector.key_flags.get(&0x46), Some(&0));
    assert_eq!(HELD.lock().unwrap().get(&(true, 0x46)), Some(&1));
    assert_eq!(injector.repeat, repeat);
    injector.repeat.as_mut().unwrap().3 = std::time::Instant::now();
    injector.due().unwrap();
    assert_eq!(calls(), [described(&[injector.key_scan(0x46, true, 0)])]);
    injector.apply(&key(0x71, false)).unwrap();
    assert_eq!(calls(), [described(&[injector.key_scan(0x46, false, 0)])]);
    assert!(injector.keys.is_empty() && injector.key_sources.is_empty());
    assert!(injector.repeat.is_none());
    assert!(!HELD.lock().unwrap().contains_key(&(true, 0x46)));
}

#[test]
fn mapped_sources_share_one_owner_with_other_clients_and_disconnect_cleanup() {
    let mut first = recording();
    first
        .policy
        .keybindings
        .extend([(0x73, 0x47), (0x74, 0x47)]);
    let mut second = recording();
    assert!(
        first
            .apply_all(&[key(0x73, true), key(0x74, true)])
            .is_empty()
    );
    second.apply(&key(0x47, true)).unwrap();
    assert_eq!(calls(), [described(&[first.key_scan(0x47, true, 0)])]);
    assert_eq!(HELD.lock().unwrap().get(&(true, 0x47)), Some(&2));
    first.release_all();
    first.release_all();
    assert!(calls().is_empty());
    assert!(first.keys.is_empty() && first.key_sources.is_empty());
    assert_eq!(HELD.lock().unwrap().get(&(true, 0x47)), Some(&1));
    second.apply(&key(0x47, false)).unwrap();
    assert_eq!(calls(), [described(&[second.key_scan(0x47, false, 0)])]);
    assert!(!HELD.lock().unwrap().contains_key(&(true, 0x47)));
}

#[test]
fn mapped_key_sources_are_forgotten_when_windows_refuses_input() {
    let mut injector = recording();
    injector
        .policy
        .keybindings
        .extend([(0x75, 0x48), (0x76, 0x48)]);
    TAKES.set(0);
    assert!(injector.apply(&key(0x75, true)).is_err());
    assert!(injector.keys.is_empty() && injector.key_sources.is_empty());
    assert!(!HELD.lock().unwrap().contains_key(&(true, 0x48)));
    TAKES.set(usize::MAX);
    assert!(
        injector
            .apply_all(&[key(0x75, true), key(0x76, true)])
            .is_empty()
    );
    assert_eq!(injector.key_sources.len(), 2);
    injector.apply(&key(0x75, false)).unwrap();
    TAKES.set(0);
    assert!(injector.apply(&key(0x76, false)).is_err());
    assert!(injector.keys.is_empty() && injector.key_sources.is_empty());
    assert!(injector.repeat.is_none());
    assert!(!HELD.lock().unwrap().contains_key(&(true, 0x48)));
    TAKES.set(usize::MAX);
}

#[test]
fn remapped_enter_keys_retain_their_sources_when_release_loses_extension() {
    let mut injector = recording();
    injector.policy.keybindings.insert(0x0d, 0x49);
    injector.apply(&key(0x0d, true)).unwrap();
    injector
        .apply(&Event::Keyboard {
            key: 0x0d,
            modifiers: 0x10,
            flags: 0,
            down: true,
        })
        .unwrap();
    assert_eq!(injector.key_sources.len(), 2);
    assert_eq!(calls(), [described(&[injector.key_scan(0x49, true, 0)])]);
    injector.apply(&key(0x0d, false)).unwrap();
    assert!(calls().is_empty());
    assert_eq!(injector.key_sources.len(), 1);
    injector.apply(&key(0x0d, false)).unwrap();
    assert_eq!(calls(), [described(&[injector.key_scan(0x49, false, 0)])]);
    assert!(injector.keys.is_empty() && injector.key_sources.is_empty());
}

#[test]
fn a_pass_of_moves_keys_buttons_and_text_is_one_call_in_order() {
    let mut injector = recording();
    let events = [
        Event::Relative { x: 1, y: 2 },
        key(0x42, true),
        click(2, true),
        Event::Scroll {
            amount: 120,
            horizontal: false,
        },
        Event::Text("hi".into()),
    ];
    assert!(injector.apply_all(&events).is_empty());
    assert_eq!(
        calls(),
        [described(&[
            Injector::mouse(1, 2, 0, MOUSEEVENTF_MOVE),
            injector.key_scan(0x42, true, 0),
            Injector::button(2, true),
            Injector::mouse(0, 0, 120, MOUSEEVENTF_WHEEL),
            Injector::key(u16::from(b'h'), true, true),
            Injector::key(u16::from(b'h'), false, true),
            Injector::key(u16::from(b'i'), true, true),
            Injector::key(u16::from(b'i'), false, true),
        ])]
    );
    assert!(injector.keys.contains(&0x42) && injector.buttons.contains(&2));
    // Their releases in the next pass also share a call.
    assert!(
        injector
            .apply_all(&[key(0x42, false), click(2, false)])
            .is_empty()
    );
    assert_eq!(
        calls(),
        [described(&[
            injector.key_scan(0x42, false, 0),
            Injector::button(2, false),
        ])]
    );
    assert!(injector.keys.is_empty() && injector.buttons.is_empty());
    let held = HELD.lock().unwrap();
    assert!(!held.contains_key(&(true, 0x42)) && !held.contains_key(&(false, 2)));
}

#[test]
fn a_press_and_its_release_in_one_pass_both_go_out_in_order() {
    let mut injector = recording();
    // A release goes out only for a key the client holds, and a press
    // is held once Windows took it: the press is sent first.
    let events = [
        key(0x43, true),
        key(0x43, false),
        click(3, true),
        click(3, false),
    ];
    assert!(injector.apply_all(&events).is_empty());
    assert_eq!(
        calls(),
        [
            described(&[injector.key_scan(0x43, true, 0)]),
            described(&[injector.key_scan(0x43, false, 0), Injector::button(3, true)]),
            described(&[Injector::button(3, false)]),
        ]
    );
    assert!(injector.keys.is_empty() && injector.buttons.is_empty());
    assert!(injector.repeat.is_none());
}

#[test]
fn control_silence_releases_keys_and_allows_them_to_be_pressed_again() {
    let mut injector = recording();
    assert!(
        injector
            .apply_all(&[key(0x57, true), click(5, true)])
            .is_empty()
    );
    calls();
    injector.release_all();
    assert!(injector.keys.is_empty() && injector.buttons.is_empty());
    assert!(injector.repeat.is_none());
    assert_eq!(
        calls(),
        [
            described(&[injector.key_scan(0x57, false, 0)]),
            described(&[Injector::button(5, false)]),
        ]
    );
    injector.release_all();
    injector.due().unwrap();
    assert!(calls().is_empty());
    assert!(
        injector
            .apply_all(&[key(0x57, true), click(5, true)])
            .is_empty()
    );
    assert_eq!(
        calls(),
        [described(&[
            injector.key_scan(0x57, true, 0),
            Injector::button(5, true),
        ])]
    );
}

#[test]
fn presses_windows_refuses_are_not_held_and_refused_releases_are_forgotten() {
    let mut injector = recording();
    TAKES.set(1);
    let errors = injector.apply_all(&[
        Event::Relative { x: 0, y: 0 },
        key(0x44, true),
        click(4, true),
    ]);
    assert_eq!(errors.len(), 2);
    assert!(injector.keys.is_empty() && injector.buttons.is_empty());
    assert!(injector.repeat.is_none());
    {
        let held = HELD.lock().unwrap();
        assert!(!held.contains_key(&(true, 0x44)) && !held.contains_key(&(false, 4)));
    }
    TAKES.set(usize::MAX);
    assert!(injector.apply_all(&[key(0x45, true)]).is_empty());
    assert!(injector.keys.contains(&0x45));
    TAKES.set(0);
    assert_eq!(injector.apply_all(&[key(0x45, false)]).len(), 1);
    assert!(injector.keys.is_empty() && injector.repeat.is_none());
    assert!(!HELD.lock().unwrap().contains_key(&(true, 0x45)));
    assert_eq!(calls().len(), 3);
}

/// An injector for the first display, if this machine has one.
fn on_first_display() -> Option<(Injector, String)> {
    let name = crate::capture::displays()
        .ok()?
        .first()?
        .display_name
        .clone();
    let mut injector = Injector::new(&name, "vhf").unwrap();
    injector.inject = record;
    Some((injector, name))
}

#[test]
fn absolute_input_waits_for_the_display_lookup_it_needs() {
    let Some((mut injector, _)) = on_first_display() else {
        return;
    };
    // Created at once; the display is looked up on another thread.
    assert!(injector.rect.is_none());
    assert!(injector.lookup.as_ref().is_some_and(|l| l.thread.is_some()));
    let events = [Event::Absolute {
        x: 0,
        y: 0,
        width: 100,
        height: 100,
    }];
    assert!(injector.apply_all(&events).is_empty());
    assert!(injector.rect.is_some());
    assert_eq!(calls().len(), 1);
}

#[test]
fn returning_to_the_current_display_discards_an_obsolete_lookup() {
    let mut injector = recording();
    injector.settle_lookup(true);
    let output = injector.output.clone();
    let current = RECT {
        left: 0,
        top: 0,
        right: 100,
        bottom: 100,
    };
    injector.rect = Some(current);
    injector.lookup = Some(Lookup {
        output: r"\\.\DISPLAY98".into(),
        started: std::time::Instant::now(),
        thread: Some(std::thread::spawn(|| {
            Ok(RECT {
                left: 100,
                top: 0,
                right: 200,
                bottom: 100,
            })
        })),
    });
    injector.set_output(&output);
    assert_eq!(injector.display(), Some(current));
    assert_eq!(injector.output, output);
}

#[test]
fn a_renamed_display_is_looked_up_without_holding_up_the_pass() {
    let Some((mut injector, first)) = on_first_display() else {
        return;
    };
    injector.settle_lookup(true);
    let rect = injector.rect.unwrap();
    // The session names a display that does not exist (yet): absolute
    // input keeps the display it has until the new one is found.
    injector.set_output(r"\\.\DISPLAY99");
    let lookup = injector.lookup.as_ref().unwrap();
    assert!(lookup.thread.is_some() && lookup.output == r"\\.\DISPLAY99");
    assert_eq!(injector.output, first);
    let started = lookup.started;
    injector.settle_lookup(true);
    assert_eq!(
        (injector.rect, injector.output.as_str()),
        (Some(rect), &*first)
    );
    // A name not found is not looked up again on every pass.
    injector.set_output(r"\\.\DISPLAY99");
    if started.elapsed() < RECT_INTERVAL {
        assert!(injector.lookup.as_ref().unwrap().thread.is_none());
    }
}

#[test]
fn touch_and_pen_keep_their_place_between_mouse_moves() {
    let mut injector = recording();
    // There is no display: touch and pen reach no one, but still split
    // the pass so mouse input around them keeps its order.
    let events = [
        Event::Relative { x: 1, y: 0 },
        Event::Touch {
            event: 1,
            id: 7,
            x: 0.5,
            y: 0.5,
            pressure: 0.5,
            major: 0.,
            minor: 0.,
            rotation: u16::MAX,
        },
        Event::Relative { x: 2, y: 0 },
        Event::Pen {
            event: 0,
            tool: 1,
            buttons: 0,
            x: 0.5,
            y: 0.5,
            pressure: 0.,
            rotation: u16::MAX,
            tilt: u8::MAX,
            roll: u16::MAX,
        },
        Event::Relative { x: 3, y: 0 },
    ];
    assert!(injector.apply_all(&events).is_empty());
    assert_eq!(
        calls(),
        [1, 2, 3].map(|x| described(&[Injector::mouse(x, 0, 0, MOUSEEVENTF_MOVE)]))
    );
}

#[test]
fn the_cursor_maps_onto_the_stream_bars_included() {
    // A 1920x1080 display right of the primary one.
    let rect = RECT {
        left: 2560,
        top: 0,
        right: 4480,
        bottom: 1080,
    };
    let at = |x, y, picture| cursor_in_stream(POINT { x, y }, rect, picture);
    assert_eq!(at(2560, 0, None), Some((0., 0.)));
    assert_eq!(at(2560 + 960, 540, None), Some((0.5, 0.5)));
    // Off the display, its right and bottom edges included.
    for (x, y) in [(2559, 10), (4480, 10), (3000, -1), (3000, 1080), (100, 100)] {
        assert_eq!(at(x, y, None), None, "{x},{y}");
    }
    // The display letterboxed into a 4:3 stream: its 16:9 picture in
    // 1600x1200 sits at (0, 150).
    let stream = (1600, 1200);
    let picture = butterpollo_core::display_policy::picture((1920, 1080), stream).unwrap();
    assert_eq!(picture, (0, 150, 1600, 900));
    assert_eq!(
        at(2560, 0, Some((stream, picture))),
        Some((0., 150. / 1200.))
    );
    assert_eq!(
        at(2560 + 960, 540, Some((stream, picture))),
        Some((0.5, 0.5))
    );
    // The stream point maps back onto the same display point, as absolute
    // input does.
    let (x, y) = at(2560 + 480, 270, Some((stream, picture))).unwrap();
    let back = butterpollo_core::display_policy::stream_to_picture(
        (f64::from(x), f64::from(y)),
        stream,
        picture,
    );
    assert!(
        (back.0 - 0.25).abs() < 1e-6 && (back.1 - 0.25).abs() < 1e-6,
        "{back:?}"
    );
    // A degenerate rectangle is no display.
    let empty = RECT {
        right: 2560,
        ..rect
    };
    assert_eq!(cursor_in_stream(POINT { x: 2560, y: 0 }, empty, None), None);
}

#[test]
fn a_turned_portrait_display_maps_the_stream_upright() {
    // A 2560x1440 monitor turned to portrait left of the primary one: Windows
    // gives its desktop the turned size, and capture streams it upright.
    let rect = RECT {
        left: -1440,
        top: 0,
        right: 0,
        bottom: 2560,
    };
    let at = |x, y, picture| cursor_in_stream(POINT { x, y }, rect, picture);
    assert_eq!(at(-1440, 0, None), Some((0., 0.)));
    assert_eq!(at(-720, 1280, None), Some((0.5, 0.5)));
    assert_eq!(at(-1440, 1920, None), Some((0., 0.75)));
    // Its old landscape extent is off the display now.
    assert_eq!(at(-1440, 2560, None), None);
    assert_eq!(at(1000, 100, None), None);
    // A portrait phone's 1080x2400 stream holds the 9:16 desktop with bars
    // above and below, and a point maps there and back.
    let stream = (1080, 2400);
    let picture = butterpollo_core::display_policy::picture((1440, 2560), stream).unwrap();
    assert_eq!(picture, (0, 240, 1080, 1920));
    assert_eq!(at(-1440, 0, Some((stream, picture))), Some((0., 0.1)));
    let (x, y) = at(-1440 + 360, 1920, Some((stream, picture))).unwrap();
    let back = butterpollo_core::display_policy::stream_to_picture(
        (f64::from(x), f64::from(y)),
        stream,
        picture,
    );
    assert!(
        (back.0 - 0.25).abs() < 1e-6 && (back.1 - 0.75).abs() < 1e-6,
        "{back:?}"
    );
}

fn non_normalized(key: u16, down: bool) -> Event {
    Event::Keyboard {
        key,
        modifiers: 0,
        flags: 1,
        down,
    }
}
/// Sets the layout with this id and lets it take over if it can.
fn use_layout(injector: &mut Injector, klid: Option<u32>) {
    injector.set_keyboard_layout(klid.map(|klid| crate::keylayout::Layout::load(klid).unwrap()));
    injector.follow_keyboard_layout();
}
/// The scan code and flags of each key sent since the last look.
fn scans() -> Vec<(u16, u32)> {
    CALLS
        .with_borrow_mut(std::mem::take)
        .iter()
        .flatten()
        // SAFETY: The keyboard tests send only keyboard INPUTs, so each ki union field is initialized.
        .map(|input| unsafe { (input.Anonymous.ki.wScan, input.Anonymous.ki.dwFlags.0) })
        .collect()
}
const SCAN: u32 = KEYEVENTF_SCANCODE.0;
const UP: u32 = KEYEVENTF_SCANCODE.0 | KEYEVENTF_KEYUP.0;

/// Keys the client did not normalize get the scan codes of the device's layout. A new
/// layout takes over only once no key is held, so a release goes up where it went down.
#[test]
fn non_normalized_keys_follow_the_device_layout_between_presses() {
    let mut injector = recording();
    use_layout(&mut injector, Some(0x0407));
    injector.apply(&non_normalized(0x5a, true)).unwrap();
    use_layout(&mut injector, Some(0x040c));
    assert_eq!(injector.layout.as_ref().map(|l| l.klid()), Some(0x0407));
    injector.apply(&non_normalized(0x5a, false)).unwrap();
    injector.follow_keyboard_layout();
    injector.apply(&non_normalized(0x5a, true)).unwrap();
    injector.apply(&non_normalized(0x5a, false)).unwrap();
    // Normalized keys keep the US table whatever the layout.
    injector.apply(&key(0x5a, true)).unwrap();
    injector.apply(&key(0x5a, false)).unwrap();
    assert_eq!(
        scans(),
        [
            (0x15, SCAN),
            (0x15, UP),
            (0x11, SCAN),
            (0x11, UP),
            (0x2c, SCAN),
            (0x2c, UP)
        ]
    );
    // Without scan codes the layout has nothing to do: the key goes as its VK.
    injector.policy.always_send_scancodes = false;
    injector.apply(&non_normalized(0xde, true)).unwrap();
    injector.apply(&non_normalized(0xde, false)).unwrap();
    assert_eq!(scans(), [(0, 0), (0, KEYEVENTF_KEYUP.0)]);
    // A client that stops reporting leaves keys to the host's layout again.
    use_layout(&mut injector, None);
    assert!(injector.layout.is_none());
}

/// AltGr is the right Alt, an E0 key, on every layout; the key pressed with it gets
/// the device layout's scan code.
#[test]
fn altgr_combinations_use_the_device_layout() {
    let altgr = 0x38;
    let extended = KEYEVENTF_EXTENDEDKEY.0;
    // German AltGr+M types the micro sign, French AltGr+0 the at sign, UK AltGr+4
    // the euro sign.
    for (klid, key, scan) in [
        (0x0407, 0x4d, 0x32),
        (0x040c, 0x30, 0x0b),
        (0x0809, 0x34, 0x05),
    ] {
        let mut injector = recording();
        use_layout(&mut injector, Some(klid));
        for event in [
            non_normalized(0xa5, true),
            non_normalized(key, true),
            non_normalized(key, false),
            non_normalized(0xa5, false),
        ] {
            injector.apply(&event).unwrap();
        }
        assert_eq!(
            scans(),
            [
                (altgr, SCAN | extended),
                (scan, SCAN),
                (scan, UP),
                (altgr, UP | extended)
            ],
            "{klid:08X}"
        );
    }
}
