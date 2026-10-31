use super::*;
fn keyboard(key: u32, down: bool, flags: u8) -> KEYBDINPUT {
    // No input injection or layout API calls: normalized scan codes come
    // from the static US table; non-normalized keys retain their VK.
    // SAFETY: keyboard_input constructs a keyboard INPUT with its ki union field initialized.
    unsafe {
        Injector::keyboard_input(key, down, flags, false, None)
            .Anonymous
            .ki
    }
}
#[test]
fn voidlink_media_commands_and_older_browser_keys_use_extended_scans() {
    for (key, scan) in [
        (0xad, 0x20),
        (0xae, 0x2e),
        (0xaf, 0x30),
        (0xb0, 0x19),
        (0xb1, 0x10),
        (0xb2, 0x24),
        (0xb3, 0x22),
        (0xa6, 0x6a),
        (0xa7, 0x69),
    ] {
        for modifiers in [0, MODIFIER_EXTENDED] {
            for down in [true, false] {
                let input = keyboard(keyboard_identity(key, modifiers), down, 0);
                assert_eq!(input.wVk, VIRTUAL_KEY(0));
                assert_eq!(input.wScan, scan);
                assert_eq!(
                    input.dwFlags,
                    KEYEVENTF_SCANCODE
                        | KEYEVENTF_EXTENDEDKEY
                        | if down {
                            KEYBD_EVENT_FLAGS(0)
                        } else {
                            KEYEVENTF_KEYUP
                        }
                );
            }
        }
    }
}
#[test]
fn print_screen_uses_e0_37_with_or_without_moonlights_extended_modifier() {
    for modifiers in [0, MODIFIER_EXTENDED] {
        for flags in [0, 1] {
            for down in [true, false] {
                // SAFETY: keyboard_input initializes the keyboard union field.
                let input = unsafe {
                    Injector::keyboard_input(
                        keyboard_identity(0x2c, modifiers),
                        down,
                        flags,
                        true,
                        None,
                    )
                    .Anonymous
                    .ki
                };
                assert_eq!(input.wScan, 0x37);
                assert_eq!(input.wVk, VIRTUAL_KEY(0));
                assert_eq!(
                    input.dwFlags,
                    KEYEVENTF_SCANCODE
                        | KEYEVENTF_EXTENDEDKEY
                        | if down {
                            KEYBD_EVENT_FLAGS(0)
                        } else {
                            KEYEVENTF_KEYUP
                        }
                );
            }
        }
    }
    assert_eq!(
        keyboard_identity(0x2c, 0),
        keyboard_identity(0x2c, MODIFIER_EXTENDED)
    );
}
#[test]
fn moonlight_keypad_enter_retains_its_identity_for_press_repeat_and_release() {
    let enter = keyboard_identity(0x0d, 0);
    let keypad = keyboard_identity(0x0d, MODIFIER_EXTENDED);
    assert_ne!(enter, keypad);
    assert_eq!(
        keyboard_identity(0x0d, MODIFIER_EXTENDED | MODIFIER_SHIFT),
        keypad
    );
    for flags in [0, 1] {
        for down in [true, true, false] {
            let ordinary = keyboard(enter, down, flags);
            let extended = keyboard(keypad, down, flags);
            assert_eq!(ordinary.wVk, extended.wVk);
            assert_eq!(ordinary.wScan, extended.wScan);
            assert_eq!(
                ordinary.dwFlags & KEYEVENTF_EXTENDEDKEY,
                KEYBD_EVENT_FLAGS(0)
            );
            assert_eq!(extended.dwFlags, ordinary.dwFlags | KEYEVENTF_EXTENDEDKEY);
            assert_eq!(
                extended.dwFlags & KEYEVENTF_KEYUP,
                if down {
                    KEYBD_EVENT_FLAGS(0)
                } else {
                    KEYEVENTF_KEYUP
                }
            );
        }
    }
}
#[test]
fn simultaneous_enter_keys_and_shared_clients_release_independently() {
    let enter = keyboard_identity(0x0d, 0);
    let keypad = keyboard_identity(0x0d, MODIFIER_EXTENDED);
    let mut held = BTreeMap::new();
    let mut sent = Vec::new();
    for (key, down, owned) in [
        (enter, true, false),
        (keypad, true, false),
        (keypad, true, true), // A repeated press is neither sent nor counted.
        (enter, true, false), // A second client also holds ordinary Enter.
        (enter, false, true), // First client must not release that key yet.
        (keypad, false, true),
        (enter, false, true),
    ] {
        Injector::update_held(
            &mut held,
            (true, key),
            down,
            owned,
            Injector::keyboard_input(key, down, 0, false, None),
            |inputs| {
                sent.extend(
                    inputs
                        .iter()
                        // SAFETY: Every input here was constructed by keyboard_input, so its ki union field is initialized.
                        .map(|input| unsafe { input.Anonymous.ki.dwFlags }),
                );
                Ok(())
            },
        )
        .unwrap();
    }
    assert!(held.is_empty());
    assert_eq!(
        sent,
        vec![
            KEYEVENTF_SCANCODE,
            KEYEVENTF_SCANCODE | KEYEVENTF_EXTENDEDKEY,
            KEYEVENTF_SCANCODE | KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP,
            KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP,
        ]
    );
}
#[test]
fn a_release_windows_refuses_still_forgets_the_key() {
    let key = (true, keyboard_identity(0x0d, 0));
    let mut held = BTreeMap::from([(key, 1)]);
    let refused = Injector::update_held(
        &mut held,
        key,
        false,
        true,
        Injector::keyboard_input(key.1, false, 0, false, None),
        |_| anyhow::bail!("the secure desktop refused input"),
    );
    assert!(refused.is_err());
    // Otherwise every later press of the key, in every later session, is
    // skipped as already held.
    assert!(held.is_empty());
}
#[test]
fn cleanup_retries_a_refused_final_mouse_release_once() {
    let identity = (false, 1);
    for recovered in [false, true] {
        let mut held = BTreeMap::from([(identity, 1)]);
        let mut attempts = 0;
        let result =
            Injector::release_held(&mut held, identity, Injector::button(1, false), |inputs| {
                assert_eq!(inputs.len(), 1);
                attempts += 1;
                if recovered && attempts == 2 {
                    Ok(())
                } else {
                    bail!("desktop refused release")
                }
            });
        assert_eq!(result.is_ok(), recovered);
        assert_eq!(attempts, 2);
        // Exhausted retries cannot poison ownership for future clients.
        assert!(held.is_empty());
    }
    let mut shared = BTreeMap::from([(identity, 2)]);
    Injector::release_held(&mut shared, identity, Injector::button(1, false), |_| {
        panic!("another client still holds this button")
    })
    .unwrap();
    assert_eq!(shared[&identity], 1);
}
#[test]
fn key_remapping_uses_the_destination_extension_and_legacy_vk_width() {
    let config =
        butterpollo_core::config::Config::parse("keybindings=[13,65,66,163,67,4660]\n").unwrap();
    let policy = butterpollo_core::input_policy::Policy::resolve(&config).unwrap();
    assert_eq!(
        mapped_keyboard_identity(&policy, 0x0d, MODIFIER_EXTENDED),
        0x41
    );
    let right_ctrl = mapped_keyboard_identity(&policy, 0x42, 0);
    assert_eq!(
        keyboard(right_ctrl, true, 0).dwFlags & KEYEVENTF_EXTENDEDKEY,
        KEYEVENTF_EXTENDEDKEY
    );
    assert_eq!(
        mapped_keyboard_identity(&policy, 0x43, MODIFIER_EXTENDED),
        0x1234
    );
    assert_eq!(keyboard_identity(0x1234, MODIFIER_EXTENDED) as u16, 0x1234);
}
#[test]
fn modifier_tracking_ignores_the_internal_extension_bit() {
    let ctrl = keyboard_identity(0x11, MODIFIER_EXTENDED);
    assert!(is_modifier(ctrl));
    let alternatives = [0x11, 0xa2, 0xa3];
    let empty_keys = BTreeSet::new();
    let empty_held = BTreeMap::new();
    assert!(modifier_held(
        &BTreeSet::from([ctrl]),
        &empty_held,
        alternatives
    ));
    assert!(modifier_held(
        &empty_keys,
        &BTreeMap::from([((true, ctrl), 1)]),
        alternatives
    ));
    assert!(!modifier_held(
        &empty_keys,
        &BTreeMap::from([((false, ctrl), 1)]),
        alternatives
    ));
    assert!(!modifier_held(
        &BTreeSet::from([keyboard_identity(0x0d, MODIFIER_EXTENDED)]),
        &empty_held,
        alternatives
    ));
}
#[test]
fn a_key_repeat_does_not_release_a_shift_pressed_after_the_key() {
    let a = keyboard_identity(0x41, 0);
    let mut keys = BTreeSet::from([a]);
    let held = BTreeMap::from([((true, a), 1)]);
    // `a` went down with Moonlight's Shift flag and no Shift key held:
    // Shift is pressed around it.
    let modifiers = unheld_modifiers(a, MODIFIER_SHIFT, &keys, &held);
    assert_eq!(modifiers, MODIFIER_SHIFT);
    let sent = |inputs: Vec<INPUT>| -> Vec<_> {
        inputs
            .iter()
            // SAFETY: repeat_inputs constructs only keyboard INPUTs, so each ki union field is initialized.
            .map(|input| unsafe { (input.Anonymous.ki.wScan, input.Anonymous.ki.dwFlags) })
            .collect()
    };
    let (shift, key) = (
        u16::from(crate::keylayout::SCANCODES[0x10]),
        u16::from(crate::keylayout::SCANCODES[0x41]),
    );
    assert_eq!(
        sent(Injector::repeat_inputs(
            a, 0, modifiers, &keys, &held, false, None
        )),
        [
            (shift, KEYEVENTF_SCANCODE),
            (key, KEYEVENTF_SCANCODE),
            (shift, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP),
        ]
    );
    // The user then holds the real left Shift: a repeat that released it
    // would let go of the user's Shift while it is still down.
    keys.insert(0xa0);
    let repeat = sent(Injector::repeat_inputs(
        a, 0, modifiers, &keys, &held, false, None,
    ));
    assert_eq!(repeat, [(key, KEYEVENTF_SCANCODE)]);
    assert!(
        repeat
            .iter()
            .all(|(_, flags)| *flags & KEYEVENTF_KEYUP == KEYBD_EVENT_FLAGS(0))
    );
    // A Shift another client holds counts too.
    let shared = BTreeMap::from([((true, a), 1), ((true, 0xa1), 1)]);
    assert_eq!(
        sent(Injector::repeat_inputs(
            a,
            0,
            modifiers,
            &BTreeSet::from([a]),
            &shared,
            false,
            None
        )),
        [(key, KEYEVENTF_SCANCODE)]
    );
    // A modifier that was really held at key-down is not added later.
    assert_eq!(
        sent(Injector::repeat_inputs(
            a,
            0,
            0,
            &BTreeSet::from([a]),
            &held,
            false,
            None
        )),
        [(key, KEYEVENTF_SCANCODE)]
    );
}
#[test]
fn new_extended_modifier_preserves_legacy_navigation_and_right_modifier_identity() {
    for key in [
        0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x2c, 0x2d, 0x2e, 0xa3, 0xa5, 0x5b, 0x5c,
        0x5d, 0x6f,
    ] {
        let legacy = keyboard_identity(key, 0);
        assert_eq!(legacy, keyboard_identity(key, MODIFIER_EXTENDED));
        for down in [true, false] {
            assert_eq!(
                keyboard(legacy, down, 0).dwFlags & KEYEVENTF_EXTENDEDKEY,
                KEYEVENTF_EXTENDEDKEY
            );
        }
    }
    for key in [0x0d, 0x41, 0xa0, 0xa1, 0xa2, 0xa4] {
        assert_eq!(
            keyboard(keyboard_identity(key, 0), true, 0).dwFlags & KEYEVENTF_EXTENDEDKEY,
            KEYBD_EVENT_FLAGS(0)
        );
    }
}
