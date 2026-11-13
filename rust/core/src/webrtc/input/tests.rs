use super::*;

fn channel() -> InputChannel {
    InputChannel::new(1920, 1080)
}

fn key(code: &str, down: bool) -> String {
    format!(r#"{{"v":1,"type":"key","code":"{code}","down":{down}}}"#)
}

#[test]
fn every_table_entry_maps_to_its_virtual_key() {
    let mut seen = BTreeSet::new();
    for (code, raw) in KEYS {
        assert!(seen.insert(*code), "{code} is listed twice");
        assert_eq!(
            virtual_key(code),
            Some((raw & 0xff, raw & EXT != 0)),
            "{code}"
        );
        let events = channel().handle(&key(code, true)).unwrap();
        assert_eq!(
            events,
            vec![Input::Keyboard {
                key: raw & 0xff,
                modifiers: if raw & EXT != 0 { MODIFIER_EXTENDED } else { 0 },
                flags: 0,
                down: true,
            }],
            "{code}"
        );
        assert!((1..=0xff).contains(&(raw & 0xff)), "{code}");
    }
}

#[test]
fn the_table_has_the_expected_keys() {
    for (code, vk) in [
        ("KeyA", 0x41),
        ("KeyZ", 0x5a),
        ("Digit0", 0x30),
        ("Digit9", 0x39),
        ("F1", 0x70),
        ("F12", 0x7b),
        ("F24", 0x87),
        ("Numpad0", 0x60),
        ("Numpad9", 0x69),
        ("NumpadAdd", 0x6b),
        ("NumpadSubtract", 0x6d),
        ("NumpadDivide", 0x6f),
        ("ArrowLeft", 0x25),
        ("ArrowDown", 0x28),
        ("PageUp", 0x21),
        ("Delete", 0x2e),
        ("ShiftLeft", 0xa0),
        ("ShiftRight", 0xa1),
        ("ControlLeft", 0xa2),
        ("ControlRight", 0xa3),
        ("AltLeft", 0xa4),
        ("AltRight", 0xa5),
        ("MetaLeft", 0x5b),
        ("MetaRight", 0x5c),
        ("Backquote", 0xc0),
        ("Minus", 0xbd),
        ("Equal", 0xbb),
        ("BracketLeft", 0xdb),
        ("BracketRight", 0xdd),
        ("Backslash", 0xdc),
        ("Semicolon", 0xba),
        ("Quote", 0xde),
        ("Comma", 0xbc),
        ("Period", 0xbe),
        ("Slash", 0xbf),
        ("IntlBackslash", 0xe2),
        ("IntlRo", 0xc1),
        ("Lang1", 0x15),
        ("Lang2", 0x19),
        ("ContextMenu", 0x5d),
        ("PrintScreen", 0x2c),
        ("Pause", 0x13),
        ("ScrollLock", 0x91),
        ("CapsLock", 0x14),
        ("NumLock", 0x90),
        ("MediaPlayPause", 0xb3),
        ("AudioVolumeUp", 0xaf),
        ("BrowserBack", 0xa6),
    ] {
        assert_eq!(virtual_key(code), Some((vk, false)), "{code}");
    }
    // The table holds every letter, digit and function key.
    for n in 0..26u16 {
        let code = format!("Key{}", (b'A' + n as u8) as char);
        assert_eq!(virtual_key(&code), Some((0x41 + n, false)));
    }
    for n in 1..=24u16 {
        assert_eq!(virtual_key(&format!("F{n}")), Some((0x6f + n, false)));
    }
    for n in 0..=9u16 {
        assert_eq!(virtual_key(&format!("Digit{n}")), Some((0x30 + n, false)));
        assert_eq!(virtual_key(&format!("Numpad{n}")), Some((0x60 + n, false)));
    }
}

#[test]
fn keypad_enter_is_the_extended_return_key() {
    assert_eq!(virtual_key("Enter"), Some((0x0d, false)));
    assert_eq!(virtual_key("NumpadEnter"), Some((0x0d, true)));
    assert_eq!(
        channel().handle(&key("NumpadEnter", true)).unwrap(),
        vec![Input::Keyboard {
            key: 0x0d,
            modifiers: 0x10,
            flags: 0,
            down: true
        }]
    );
}

#[test]
fn a_key_up_is_a_key_up() {
    assert_eq!(
        channel().handle(&key("KeyW", false)).unwrap(),
        vec![Input::Keyboard {
            key: 0x57,
            modifiers: 0,
            flags: 0,
            down: false
        }]
    );
}

#[test]
fn an_unknown_code_is_dropped_and_counted() {
    let mut input = channel();
    for code in ["Fn", "Hyper", "KeyÄ", "", "keya", "Unidentified"] {
        assert_eq!(input.handle(&key(code, true)).unwrap(), vec![]);
    }
    assert_eq!(input.unknown_codes(), 6);
    assert!(input.held().is_empty());
    // A known key afterwards still works and the count stays.
    assert_eq!(input.handle(&key("KeyA", true)).unwrap().len(), 1);
    assert_eq!(input.unknown_codes(), 6);
}

#[test]
fn text_becomes_input_text_and_is_capped_at_32_characters() {
    let mut input = channel();
    assert_eq!(
        input
            .handle(r#"{"v":1,"type":"text","text":"é漢😀"}"#)
            .unwrap(),
        vec![Input::Text("é漢😀".into())]
    );
    let at_cap = "é".repeat(32);
    assert_eq!(
        input
            .handle(&format!(r#"{{"v":1,"type":"text","text":"{at_cap}"}}"#))
            .unwrap(),
        vec![Input::Text(at_cap)]
    );
    let over = "é".repeat(33);
    assert!(matches!(
        input.handle(&format!(r#"{{"v":1,"type":"text","text":"{over}"}}"#)),
        Err(MessageError::Invalid(_))
    ));
    assert_eq!(
        input.handle(r#"{"v":1,"type":"text","text":""}"#).unwrap(),
        vec![]
    );
}

#[test]
fn relative_motion_is_clamped_to_i16() {
    let mut input = channel();
    let mut relative = |dx: &str, dy: &str| {
        input
            .handle(&format!(
                r#"{{"v":1,"type":"mouse-move","dx":{dx},"dy":{dy}}}"#
            ))
            .unwrap()
    };
    assert_eq!(relative("-3", "5"), vec![Input::Relative { x: -3, y: 5 }]);
    assert_eq!(
        relative("100000", "-100000"),
        vec![Input::Relative {
            x: i16::MAX,
            y: i16::MIN
        }]
    );
    // Whole pixels go out now; the fraction waits (see the next test).
    assert_eq!(
        relative("1.6", "-2.4"),
        vec![Input::Relative { x: 1, y: -2 }]
    );
}

#[test]
fn fractional_motion_accumulates_per_axis() {
    let mut input = channel();
    let mut relative = |dx: &str, dy: &str| {
        input
            .handle(&format!(
                r#"{{"v":1,"type":"mouse-move","dx":{dx},"dy":{dy}}}"#
            ))
            .unwrap()
    };
    // 0.4 steps: nothing, nothing, then one pixel when the sum passes 1.
    assert_eq!(relative("0.4", "-0.4"), vec![]);
    assert_eq!(relative("0.4", "-0.4"), vec![]);
    assert_eq!(
        relative("0.4", "-0.4"),
        vec![Input::Relative { x: 1, y: -1 }]
    );
    // 0.2 is left over on each axis; two more steps make 1.0 again.
    assert_eq!(relative("0.4", "0"), vec![]);
    assert_eq!(relative("0.4", "0"), vec![Input::Relative { x: 1, y: 0 }]);
    // The two axes carry separate remainders.
    assert_eq!(relative("0", "-0.4"), vec![]);
    // Total motion over many steps is not lost: 25 * 0.4 = 10.
    let mut sum = 0i32;
    for _ in 0..25 {
        for event in relative("0.4", "0") {
            if let Input::Relative { x, .. } = event {
                sum += i32::from(x);
            }
        }
    }
    assert_eq!(sum, 10);
}

#[test]
fn absolute_position_is_clamped_and_scaled_to_the_stream() {
    let mut input = InputChannel::new(1920, 1080);
    let mut at = |x: &str, y: &str| {
        input
            .handle(&format!(
                r#"{{"v":1,"type":"mouse-position","x":{x},"y":{y}}}"#
            ))
            .unwrap()
    };
    let absolute = |x, y| {
        vec![Input::Absolute {
            x,
            y,
            width: 1920,
            height: 1080,
        }]
    };
    assert_eq!(at("0", "0"), absolute(0, 0));
    assert_eq!(at("1", "1"), absolute(1920, 1080));
    assert_eq!(at("0.5", "0.25"), absolute(960, 270));
    assert_eq!(at("-3", "7.5"), absolute(0, 1080));
    assert_eq!(at("1e9", "-1e9"), absolute(1920, 0));
    // A different stream size scales against that size.
    let mut small = InputChannel::new(1280, 720);
    assert_eq!(
        small
            .handle(r#"{"v":1,"type":"mouse-position","x":0.5,"y":0.5}"#)
            .unwrap(),
        vec![Input::Absolute {
            x: 640,
            y: 360,
            width: 1280,
            height: 720
        }]
    );
    // A 4K or larger stream still fits Moonlight's i16 reference size.
    let mut wide = InputChannel::new(7680, 4320);
    assert_eq!(
        wide.handle(r#"{"v":1,"type":"mouse-position","x":1,"y":1}"#)
            .unwrap(),
        vec![Input::Absolute {
            x: 7680,
            y: 4320,
            width: 7680,
            height: 4320
        }]
    );
    let mut huge = InputChannel::new(100_000, 0);
    assert_eq!(
        huge.handle(r#"{"v":1,"type":"mouse-position","x":1,"y":1}"#)
            .unwrap(),
        vec![Input::Absolute {
            x: i16::MAX,
            y: 1,
            width: i16::MAX,
            height: 1
        }]
    );
}

#[test]
fn dom_buttons_become_moonlight_buttons() {
    let mut input = channel();
    for (dom, moonlight) in [(0, 1), (1, 2), (2, 3), (3, 4), (4, 5)] {
        for down in [true, false] {
            assert_eq!(
                input
                    .handle(&format!(
                        r#"{{"v":1,"type":"mouse-button","button":{dom},"down":{down}}}"#
                    ))
                    .unwrap(),
                vec![Input::MouseButton {
                    button: moonlight,
                    down
                }]
            );
        }
    }
    for bad in ["5", "-1", "255", "1.5", "\"0\""] {
        assert!(
            input
                .handle(&format!(
                    r#"{{"v":1,"type":"mouse-button","button":{bad},"down":true}}"#
                ))
                .is_err(),
            "{bad}"
        );
    }
    assert!(input.held().is_empty());
}

#[test]
fn wheel_signs_follow_moonlight_on_both_axes() {
    let mut input = channel();
    let mut wheel = |dx: &str, dy: &str| {
        input
            .handle(&format!(r#"{{"v":1,"type":"wheel","dx":{dx},"dy":{dy}}}"#))
            .unwrap()
    };
    let scroll = |amount, horizontal| Input::Scroll { amount, horizontal };
    // DOM +dy scrolls down; Moonlight's up is positive.
    assert_eq!(wheel("0", "120"), vec![scroll(-120, false)]);
    assert_eq!(wheel("0", "-120"), vec![scroll(120, false)]);
    // Both signs are positive to the right.
    assert_eq!(wheel("120", "0"), vec![scroll(120, true)]);
    assert_eq!(wheel("-240", "0"), vec![scroll(-240, true)]);
    assert_eq!(
        wheel("120", "-120"),
        vec![scroll(120, false), scroll(120, true)]
    );
    assert_eq!(wheel("0", "0"), vec![]);
    // Extreme values neither overflow nor flip sign.
    assert_eq!(wheel("0", "-32768"), vec![scroll(i16::MAX, false)]);
    assert_eq!(wheel("0", "1e12"), vec![scroll(-i16::MAX, false)]);
    assert_eq!(wheel("-1e12", "0"), vec![scroll(-i16::MAX, true)]);
    // A missing axis is zero.
    assert_eq!(
        input.handle(r#"{"v":1,"type":"wheel","dy":120}"#).unwrap(),
        vec![scroll(-120, false)]
    );
}

#[test]
fn a_wrong_version_or_unknown_type_is_an_error() {
    let mut input = channel();
    assert_eq!(
        input.handle(r#"{"v":2,"type":"key","code":"KeyA","down":true}"#),
        Err(MessageError::Version(Some(2)))
    );
    assert_eq!(
        input.handle(r#"{"type":"key","code":"KeyA","down":true}"#),
        Err(MessageError::Version(None))
    );
    assert_eq!(
        input.handle(r#"{"v":"1","type":"key","code":"KeyA","down":true}"#),
        Err(MessageError::Version(None))
    );
    assert_eq!(
        input.handle(r#"{"v":1,"type":"touch","x":1}"#),
        Err(MessageError::UnknownType("touch".into()))
    );
    assert!(matches!(
        input.handle(r#"{"v":1}"#),
        Err(MessageError::Malformed(_))
    ));
    assert!(input.held().is_empty());
}

#[test]
fn hostile_messages_are_errors_not_panics() {
    let mut input = channel();
    for text in [
        "",
        "null",
        "[]",
        "\"key\"",
        "{",
        r#"{"v":1,"type":"key"}"#,
        r#"{"v":1,"type":"key","code":5,"down":true}"#,
        r#"{"v":1,"type":"key","code":"KeyA"}"#,
        r#"{"v":1,"type":"mouse-move","dx":"a","dy":1}"#,
        r#"{"v":1,"type":"mouse-position","x":null,"y":0}"#,
        r#"{"v":1,"type":7}"#,
        r#"{"v":1e999,"type":"key"}"#,
    ] {
        assert!(input.handle(text).is_err(), "{text:?}");
    }
    // Unknown fields are ignored.
    assert_eq!(
        input
            .handle(r#"{"v":1,"type":"key","code":"KeyA","down":true,"extra":[1]}"#)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn closing_releases_every_held_key_and_button() {
    let mut input = channel();
    for text in [
        key("ShiftLeft", true),
        key("KeyW", true),
        key("KeyA", true),
        key("KeyA", false),
        key("NumpadEnter", true),
        r#"{"v":1,"type":"mouse-button","button":0,"down":true}"#.to_owned(),
        r#"{"v":1,"type":"mouse-button","button":2,"down":true}"#.to_owned(),
        r#"{"v":1,"type":"mouse-button","button":2,"down":false}"#.to_owned(),
        r#"{"v":1,"type":"mouse-move","dx":3,"dy":3}"#.to_owned(),
        r#"{"v":1,"type":"wheel","dy":120}"#.to_owned(),
    ] {
        input.handle(&text).unwrap();
    }
    // Gamepad mouse mode holds the middle button for this client too.
    input.observe(&Input::MouseButton {
        button: 2,
        down: true,
    });
    assert!(!input.held().is_empty());
    let key_up = |key, modifiers| Input::Keyboard {
        key,
        modifiers,
        flags: 0,
        down: false,
    };
    assert_eq!(
        input.release_all(),
        vec![
            Input::MouseButton {
                button: 1,
                down: false
            },
            Input::MouseButton {
                button: 2,
                down: false
            },
            key_up(0x0d, 0x10),
            key_up(0x57, 0),
            key_up(0xa0, 0),
        ]
    );
    assert!(input.held().is_empty());
    // Nothing is released twice.
    assert_eq!(input.release_all(), vec![]);
}

#[test]
fn a_release_matches_the_key_that_was_pressed() {
    let mut held = HeldInput::default();
    // The extended and plain Return keys are different keys.
    held.observe(&Input::Keyboard {
        key: 0x0d,
        modifiers: 0x10,
        flags: 0,
        down: true,
    });
    held.observe(&Input::Keyboard {
        key: 0x0d,
        modifiers: 0,
        flags: 0,
        down: true,
    });
    held.observe(&Input::Keyboard {
        key: 0x0d,
        modifiers: 0,
        flags: 0,
        down: false,
    });
    assert_eq!(
        held.release_all(),
        vec![Input::Keyboard {
            key: 0x0d,
            modifiers: 0x10,
            flags: 0,
            down: false
        }]
    );
}
