//! Keybinding ownership, pass batching, modifier wrapping, and mixed-route button isolation tests.

use super::*;

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

/// A burst of mouse and keyboard events through one pass: the press splits it,
/// and every event reaches Windows exactly once, in the order it arrived.
#[test]
fn a_burst_of_mixed_mouse_and_keys_keeps_every_event_in_order() {
    let mut injector = recording();
    let events = [
        Event::Relative { x: 1, y: 2 },
        key(0x5c, true),
        Event::Relative { x: 3, y: 1 },
        click(9, true),
        Event::Scroll {
            amount: 120,
            horizontal: false,
        },
        key(0x5c, false),
        click(9, false),
        Event::Relative { x: 5, y: 0 },
    ];
    assert!(injector.apply_all(&events).is_empty());
    assert_eq!(
        calls(),
        [
            described(&[
                Injector::mouse(1, 2, 0, MOUSEEVENTF_MOVE),
                injector.key_scan(0x5c, true, 0),
                Injector::mouse(3, 1, 0, MOUSEEVENTF_MOVE),
                Injector::button(9, true),
                Injector::mouse(0, 0, 120, MOUSEEVENTF_WHEEL),
            ]),
            described(&[
                injector.key_scan(0x5c, false, 0),
                Injector::button(9, false),
                Injector::mouse(5, 0, 0, MOUSEEVENTF_MOVE),
            ])
        ]
    );
    assert!(injector.keys.is_empty() && injector.buttons.is_empty());
    let held = HELD.lock().unwrap();
    assert!(!held.contains_key(&(true, 0x5c)) && !held.contains_key(&(false, 9)));
}

/// A key Moonlight reports with a modifier no one holds is pressed inside that
/// modifier only; the disconnect release frees it exactly like a plain key's.
#[test]
fn keys_pressed_inside_reported_modifiers_release_on_disconnect_like_plain_ones() {
    let mut injector = recording();
    let with_ctrl = Event::Keyboard {
        key: 0x5e,
        modifiers: 0x02,
        flags: 0,
        down: true,
    };
    injector.apply(&with_ctrl).unwrap();
    injector.apply(&key(0x5f, true)).unwrap();
    assert_eq!(
        calls(),
        [
            described(&Injector::with_modifiers(
                injector.key_scan(0x5e, true, 0),
                0x02,
                injector.policy.always_send_scancodes,
            )),
            described(&[injector.key_scan(0x5f, true, 0)]),
        ]
    );
    {
        let held = HELD.lock().unwrap();
        assert_eq!(held.get(&(true, 0x5e)), Some(&1));
        assert_eq!(held.get(&(true, 0x5f)), Some(&1));
        // The synthetic Ctrl was never held as a key.
        assert!(!held.contains_key(&(true, 0x11)));
    }
    // Disconnect: both keys release, and nothing stays held.
    injector.release_all();
    assert_eq!(
        calls(),
        [
            described(&[injector.key_scan(0x5e, false, 0)]),
            described(&[injector.key_scan(0x5f, false, 0)]),
        ]
    );
    assert!(injector.keys.is_empty() && injector.key_sources.is_empty());
    let held = HELD.lock().unwrap();
    assert!(!held.contains_key(&(true, 0x5e)));
    assert!(!held.contains_key(&(true, 0x5f)));
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

#[test]
fn mixed_mode_and_virtual_mouse_button_ownership_stay_isolated() {
    let _button1 = BUTTON1_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let held_count = |code| HELD.lock().unwrap().get(&(false, code)).copied();
    let mut send_input_client = recording();
    let mut virtual_client = recording();
    // Simulate a button held via the virtual mouse route alongside a SendInput client
    // holding the same button: both routes stay independent in HELD and release cleanly.
    virtual_client.buttons.insert(1);
    virtual_client.virtual_buttons.insert(1);
    HELD.lock()
        .unwrap()
        .insert((false, 1 | VIRTUAL_BUTTON_FLAG), 1);

    send_input_client.apply(&click(1, true)).unwrap();
    assert_eq!(calls(), [described(&[Injector::button(1, true)])]);
    assert_eq!(held_count(1), Some(1));

    // Releasing the virtual button does not drop SendInput's held button 1.
    virtual_client.apply(&click(1, false)).unwrap();
    assert!(calls().is_empty());
    assert_eq!(held_count(1 | VIRTUAL_BUTTON_FLAG), None);
    assert_eq!(held_count(1), Some(1));

    // Releasing SendInput's button sends the SendInput UP.
    send_input_client.apply(&click(1, false)).unwrap();
    assert_eq!(calls(), [described(&[Injector::button(1, false)])]);
    assert_eq!(held_count(1), None);
}

#[test]
fn due_flushes_deferred_virtual_and_injected_left_releases_on_their_own_routes() {
    let _button1 = BUTTON1_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let held_count = |code| HELD.lock().unwrap().get(&(false, code)).copied();
    // WebRTC path: deferred virtual-mouse left release flushed ONLY via `due()` must
    // clear `(false, 1 | VIRTUAL_BUTTON_FLAG)` without calling `flush_virtual_left_release()`,
    // must not emit SendInput UP, and must allow subsequent virtual left-clicks.
    let mut injector = recording();
    injector.absolute = true;
    injector.policy.left_release_delay = Duration::from_secs(60);
    injector.buttons.insert(1);
    injector.virtual_buttons.insert(1);
    HELD.lock()
        .unwrap()
        .insert((false, 1 | VIRTUAL_BUTTON_FLAG), 1);

    // Defer left-up in absolute mode.
    injector.apply(&click(1, false)).unwrap();
    assert!(injector.left_release.is_some());
    assert!(injector.left_release_virtual);
    assert!(calls().is_empty());
    assert_eq!(held_count(1 | VIRTUAL_BUTTON_FLAG), Some(1));

    // A key repeat firing before `left_release` expires must not clobber `left_release_virtual`.
    injector.apply(&key(0x52, true)).unwrap();
    calls();
    injector.repeat.as_mut().unwrap().3 = Instant::now();
    injector.due().unwrap();
    assert_eq!(calls(), [described(&[injector.key_scan(0x52, true, 0)])]);
    assert!(injector.left_release.is_some());
    assert!(injector.left_release_virtual);
    injector.apply(&key(0x52, false)).unwrap();
    calls();

    // Expire the deferred release and call ONLY `due()` (as WebRTC does).
    *injector.left_release.as_mut().unwrap() = Instant::now();
    injector.due().unwrap();
    assert!(injector.left_release.is_none());
    assert!(!injector.left_release_virtual);
    assert!(
        calls().is_empty(),
        "virtual deferred release must not fall through to SendInput"
    );
    assert_eq!(
        held_count(1 | VIRTUAL_BUTTON_FLAG),
        None,
        "deferred virtual button must not leak in global HELD"
    );

    // Subsequent virtual-mouse left press is no longer suppressed by a leaked HELD entry.
    let mut emitted = Vec::new();
    Injector::update_held(
        &mut HELD.lock().unwrap(),
        (false, 1 | VIRTUAL_BUTTON_FLAG),
        true,
        false,
        Injector::button(1, true),
        |inputs| {
            emitted.extend_from_slice(inputs);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(emitted.len(), 1);
    Injector::update_held(
        &mut HELD.lock().unwrap(),
        (false, 1 | VIRTUAL_BUTTON_FLAG),
        false,
        true,
        Injector::button(1, false),
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(held_count(1 | VIRTUAL_BUTTON_FLAG), None);

    // Non-virtual (SendInput) deferred left release flushed via `due()` still releases `(false, 1)`.
    let mut injected = recording();
    injected.absolute = true;
    injected.policy.left_release_delay = Duration::from_secs(60);
    injected.apply(&click(1, true)).unwrap();
    assert_eq!(calls(), [described(&[Injector::button(1, true)])]);
    injected.apply(&click(1, false)).unwrap();
    assert!(injected.left_release.is_some());
    assert!(!injected.left_release_virtual);
    assert_eq!(held_count(1), Some(1));

    *injected.left_release.as_mut().unwrap() = Instant::now();
    injected.due().unwrap();
    assert!(injected.left_release.is_none());
    assert_eq!(calls(), [described(&[Injector::button(1, false)])]);
    assert_eq!(held_count(1), None);
}

#[test]
fn failed_virtual_mouse_press_stops_batch_without_sticking_trailing_keys() {
    let mut injector = recording();
    let held_key = |vk| HELD.lock().unwrap().get(&(true, vk)).copied();
    let held_vbtn = |btn: u8| {
        HELD.lock()
            .unwrap()
            .get(&(false, u32::from(btn) | VIRTUAL_BUTTON_FLAG))
            .copied()
    };
    let stage_virtual_button = |inj: &mut Injector, btn: u8, down: bool| {
        let held_id = u32::from(btn) | VIRTUAL_BUTTON_FLAG;
        if down {
            HELD.lock().unwrap().insert((false, held_id), 1);
            inj.virtual_buttons.insert(btn);
            inj.pending
                .push_routed(Injector::button(btn, true), MouseRoute::Virtual);
            let end = inj.pending.inputs.len();
            inj.pending.events.push((
                end,
                Some(Press {
                    keyboard: false,
                    id: held_id,
                    source: 0,
                    flags: 0,
                    counted: true,
                    repeat: None,
                }),
            ));
        } else {
            inj.pending
                .push_routed(Injector::button(btn, false), MouseRoute::Virtual);
            let end = inj.pending.inputs.len();
            inj.pending.events.push((end, None));
        }
    };

    // Case 1: [VirtualMouseButton(down), Keyboard(0x5a, down)] — virtual mouse is not
    // plugged in, so `!applied` must stop immediately without sending key 0x5a via SendInput.
    stage_virtual_button(&mut injector, 4, true);
    let errors = injector.apply_all(&[key(0x5a, true)]);
    assert_eq!(errors.len(), 2);
    assert!(
        calls().is_empty(),
        "trailing key must not be injected after a failed virtual mouse button press"
    );
    assert!(injector.buttons.is_empty() && injector.virtual_buttons.is_empty());
    assert!(injector.keys.is_empty() && injector.key_sources.is_empty());
    assert_eq!(held_vbtn(4), None);
    assert_eq!(held_key(0x5a), None);

    // Case 2: [Relative(5, -3), VirtualMouseButton(down), Keyboard(0x59, down)] —
    // only the prefix before the failed virtual mouse entry is sent via SendInput;
    // the trailing keyboard press is neither sent nor left held.
    let prefix = Injector::mouse(5, -3, 0, MOUSEEVENTF_MOVE);
    injector.pending.push_auto(prefix);
    injector.pending.events.push((1, None));
    stage_virtual_button(&mut injector, 4, true);
    let errors = injector.apply_all(&[key(0x59, true)]);
    assert_eq!(errors.len(), 2);
    assert_eq!(calls(), [described(&[prefix])]);
    assert!(!injector.keys.contains(&0x59));
    assert!(injector.buttons.is_empty() && injector.virtual_buttons.is_empty());
    assert_eq!(held_key(0x59), None);
    assert_eq!(held_vbtn(4), None);

    // Case 3: Virtual mouse button UP succeeds even when detached, so a trailing key proceeds.
    stage_virtual_button(&mut injector, 4, false);
    assert!(injector.apply_all(&[key(0x5b, true)]).is_empty());
    assert_eq!(calls(), [described(&[injector.key_scan(0x5b, true, 0)])]);
    assert!(injector.keys.contains(&0x5b));
    injector.release_all();
    assert_eq!(calls(), [described(&[injector.key_scan(0x5b, false, 0)])]);
    assert_eq!(held_key(0x5b), None);
}
