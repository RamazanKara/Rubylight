use super::*;

const MS: Duration = Duration::from_millis(1);

fn key(key: u16, modifiers: u8, down: bool) -> Input {
    Input::Keyboard {
        key,
        modifiers,
        flags: 0,
        down,
    }
}
fn touch(event: u8, id: u32, x: f32, y: f32) -> Input {
    Input::Touch {
        event,
        id,
        x,
        y,
        pressure: 1.,
        major: 0.,
        minor: 0.,
        rotation: 0,
    }
}
fn pen(event: u8, x: f32, y: f32) -> Input {
    Input::Pen {
        event,
        tool: 1,
        buttons: 0,
        x,
        y,
        pressure: 1.,
        rotation: 0,
        tilt: 0,
        roll: u16::MAX,
    }
}
fn absolute(x: i16, y: i16) -> Input {
    Input::Absolute {
        x,
        y,
        width: 1920,
        height: 1080,
    }
}
fn button(down: bool) -> Input {
    Input::MouseButton {
        button: MOUSE_LEFT,
        down,
    }
}

#[test]
fn couch_menu_pad_chord_fires_at_1000_ms_not_999() {
    let t0 = Instant::now();
    let mut chord = PadChord::default();
    chord.update(2, LS_CLK, t0);
    chord.update(2, PAD_CHORD, t0);
    assert_eq!(chord.next_due(), Some(t0 + 1000 * MS));
    // Stick moves resend the same buttons: the deadline stays.
    chord.update(2, PAD_CHORD, t0 + 500 * MS);
    assert!(!chord.fire(t0 + 999 * MS));
    assert!(chord.fire(t0 + 1000 * MS));
    assert_eq!(chord.next_due(), None);
    assert!(!chord.fire(t0 + 5000 * MS), "fires once per hold");
    chord.update(2, PAD_CHORD, t0 + 5000 * MS);
    assert!(!chord.fire(t0 + 9000 * MS), "still the same hold");
    chord.update(2, 0, t0 + 9000 * MS);
    chord.update(2, PAD_CHORD, t0 + 9000 * MS);
    assert!(chord.fire(t0 + 10_000 * MS), "a new hold fires again");
}

#[test]
fn couch_menu_pad_chord_extra_button_or_release_cancels_per_pad() {
    let t0 = Instant::now();
    let mut chord = PadChord::default();
    chord.update(0, PAD_CHORD, t0);
    chord.update(1, PAD_CHORD, t0 + 100 * MS);
    chord.update(0, PAD_CHORD | 0x1000, t0 + 200 * MS);
    assert_eq!(chord.next_due(), Some(t0 + 1100 * MS), "pad 1 still armed");
    assert!(!chord.fire(t0 + 1099 * MS));
    chord.update(1, RS_CLK, t0 + 1050 * MS);
    assert!(!chord.fire(t0 + 2000 * MS));
    assert_eq!(chord.next_due(), None);
    // Going back to exactly L3+R3 starts a new hold.
    chord.update(0, PAD_CHORD, t0 + 3000 * MS);
    assert!(!chord.fire(t0 + 3999 * MS));
    assert!(chord.fire(t0 + 4000 * MS));
    // Pad ids beyond Moonlight's 16 are ignored, not a panic.
    chord.update(16, PAD_CHORD, t0);
    assert_eq!(chord.next_due(), None);
}

#[test]
fn couch_menu_key_chord_needs_ctrl_alt_without_shift() {
    let ctrl_alt = MODIFIER_CTRL | MODIFIER_ALT;
    assert!(KeyChord::default().matches(&key(VK_BACK, ctrl_alt, true)));
    assert!(
        KeyChord::default().matches(&key(VK_BACK, ctrl_alt | 0x08, true)),
        "Meta does not matter"
    );
    assert!(!KeyChord::default().matches(&key(VK_BACK, ctrl_alt | MODIFIER_SHIFT, true)));
    assert!(!KeyChord::default().matches(&key(VK_BACK, ctrl_alt, false)));
    assert!(!KeyChord::default().matches(&key(VK_BACK, MODIFIER_CTRL, true)));
    assert!(!KeyChord::default().matches(&key(VK_BACK, 0, true)));
    assert!(
        !KeyChord::default().matches(&key(0x2e, ctrl_alt, true)),
        "not Ctrl+Alt+Del"
    );
}

#[test]
fn couch_menu_open_with_parses_the_list_and_rejects_typos() {
    assert_eq!(
        OpenWith::parse("controller,keyboard,touch"),
        Ok(OpenWith::default())
    );
    assert_eq!(
        OpenWith::parse(" Keyboard "),
        Ok(OpenWith {
            controller: false,
            keyboard: true,
            touch: false,
            ..OpenWith::default()
        })
    );
    assert_eq!(
        OpenWith::parse(""),
        Ok(OpenWith {
            controller: false,
            keyboard: false,
            touch: false,
            ..OpenWith::default()
        })
    );
    assert!(
        OpenWith::parse("controler")
            .unwrap_err()
            .contains("controler")
    );
}

#[test]
fn couch_menu_controller_chord_refuses_what_moonlight_takes_and_its_supersets() {
    let refused = |value: &str| PadButtons::parse(value).unwrap_err();
    let quit = refused("start+select+l1+r1");
    let guide = refused("select+start");
    let mouse = refused("start");
    let stats = refused("select+l1+r1+x");
    let clickpad_guide = refused("select+guide");
    let clickpad_l1 = refused("select+l1");
    assert!(quit.contains("quits the stream"), "{quit}");
    assert!(guide.contains("into Guide"), "{guide}");
    assert!(mouse.contains("mouse mode"), "{mouse}");
    assert!(stats.contains("statistics"), "{stats}");
    assert!(clickpad_guide.contains("Select+Guide into a clickpad"));
    assert!(clickpad_l1.contains("Select+L1 into a clickpad"));
    for (superset, message) in [
        ("guide+select+a", &clickpad_guide),
        ("select+l1+r1", &clickpad_l1),
        ("l1+select+up", &clickpad_l1),
        ("start+select+l1+r1+a", &quit),
        ("r1+l1+select+start+l3", &quit),
        ("start+select+y", &guide),
        ("start+select+l1", &guide),
        ("start+y", &mouse),
        ("l3+r3+start", &mouse),
        ("select+l1+r1+x+up", &stats),
        ("x+r1+l1+select+paddle1", &stats),
    ] {
        // The same reason; only the chord's name differs.
        let reason = |m: &str| m.rsplit(": ").next().unwrap().to_owned();
        assert_eq!(reason(&refused(superset)), reason(message), "{superset}");
    }
    // Subsets of a reserved set are fine.
    for value in [
        "select",
        "select+r1",
        "l1+r1+x",
        "select+x",
        "l3+r3",
        "l1+r1+y",
    ] {
        let chord = PadButtons::parse(value).unwrap();
        assert_eq!(chord.warning(), None, "{value}");
    }
    // The buttons the open menu reads, alone or in a chord.
    for (value, message) in [
        ("a", "A presses the focused row"),
        ("l1+r1+a", "A presses the focused row"),
        ("b", "B goes back"),
        ("l3+r3+b", "B goes back"),
        ("up", "Up moves the focus"),
        ("down+select", "Down moves the focus"),
        ("left+paddle1", "Left moves the focus"),
        ("right+l3", "Right moves the focus"),
    ] {
        let error = refused(value);
        assert!(error.contains(message), "{value}: {error}");
    }
    assert!(refused("l2").contains("unknown button \"l2\""));
    assert!(
        !refused("l2").contains(" a, "),
        "refused buttons are not offered"
    );
    assert!(refused("l3++r3").contains("unknown button \"\""));
    assert!(refused(" ").contains("at least one button"));
}

#[test]
fn couch_menu_controller_chord_accepts_guide_with_a_warning() {
    let guide = PadButtons::parse("guide").unwrap();
    assert_eq!(guide.buttons(), GUIDE);
    let warning = guide.warning().unwrap();
    assert!(warning.contains("hand Guide to the OS"), "{warning}");
    assert!(warning.contains("opens Xbox Game Bar"), "{warning}");
    assert_eq!(
        check("couch_menu_controller_chord", "guide+y"),
        Ok(guide.warning())
    );
    assert_eq!(check("couch_menu_controller_chord", "l3+r3"), Ok(None));
}

#[test]
fn couch_menu_chords_round_trip_through_their_text() {
    for value in [
        "l3+r3",
        "select",
        "guide",
        "l1+r1+y",
        "select+y+paddle4+misc",
    ] {
        let chord = PadButtons::parse(value).unwrap();
        assert_eq!(chord.to_string(), value);
        assert_eq!(PadButtons::parse(&chord.to_string()), Ok(chord));
    }
    assert_eq!(PadButtons::parse(" R3 + L3 ").unwrap().to_string(), "l3+r3");
    assert_eq!(PadButtons::parse("l3+r3"), Ok(PadButtons::default()));
    assert_eq!(PadButtons::default().buttons(), PAD_CHORD);
    for (name, key) in KEY_CHORDS {
        let chord = KeyChord::parse(name).unwrap();
        assert_eq!(chord.to_string(), name);
        assert_eq!(chord.key(), key);
        assert_eq!(KeyChord::parse(&chord.to_string()), Ok(chord));
    }
    assert_eq!(
        KeyChord::parse(" Ctrl + ALT + Home ").unwrap().to_string(),
        "ctrl+alt+home"
    );
    assert_eq!(KeyChord::default().key(), VK_BACK);
    // The modifiers in any order.
    for value in ["alt+ctrl+home", " ALT + Ctrl + Home "] {
        assert_eq!(KeyChord::parse(value).unwrap().to_string(), "ctrl+alt+home");
    }
    for value in [
        "ctrl+alt+shift+backspace",
        "shift+alt+ctrl+backspace",
        "alt+shift+ctrl+end",
    ] {
        let shift = KeyChord::parse(value).unwrap_err();
        assert!(shift.contains("Ctrl+Alt+Shift"), "{value}: {shift}");
    }
    for value in [
        "ctrl+alt+m",
        "ctrl+ctrl+alt+backspace",
        "alt+backspace+ctrl+home",
        "ctrl+backspace",
        "ctrl+alt+delete",
    ] {
        let error = KeyChord::parse(value).unwrap_err();
        assert!(
            error.contains("ctrl+alt+backspace, ctrl+alt+home"),
            "{value}: {error}"
        );
    }
}

#[test]
fn couch_menu_hold_bounds() {
    for (key, low, high) in [
        ("couch_menu_hold_ms", 300, 3000),
        ("couch_menu_corner_hold_ms", 500, 3000),
    ] {
        for ok in [low, high] {
            assert_eq!(check(key, &ok.to_string()), Ok(None), "{key}={ok}");
        }
        for bad in [low - 1, high + 1] {
            let error = check(key, &bad.to_string()).unwrap_err();
            assert_eq!(
                error,
                format!("{key} must be between {low} and {high} milliseconds")
            );
        }
        for bad in ["", "-1", "1.5s", "fast"] {
            assert!(check(key, bad).is_err(), "{key}={bad:?}");
        }
    }
    assert_eq!(
        parse_hold("couch_menu_hold_ms", "\"300\"", PAD_HOLD_MS),
        Ok(300 * MS)
    );
}

#[test]
fn couch_menu_single_button_hold_fires_at_its_own_time() {
    let t0 = Instant::now();
    let select = PadButtons::parse("select").unwrap();
    let mut chord = PadChord::new(select, 300 * MS);
    chord.update(0, select.buttons(), t0);
    assert_eq!(chord.next_due(), Some(t0 + 300 * MS));
    assert!(!chord.fire(t0 + 299 * MS));
    assert!(chord.fire(t0 + 300 * MS));
    // It waits for a button change after firing.
    chord.update(0, select.buttons(), t0 + 400 * MS);
    assert!(!chord.fire(t0 + 2000 * MS));
    // An extra button cancels the hold.
    chord.update(0, 0, t0 + 2000 * MS);
    chord.update(0, select.buttons(), t0 + 2000 * MS);
    chord.update(0, select.buttons() | 0x1000, t0 + 2100 * MS);
    assert!(!chord.fire(t0 + 3000 * MS));
    // A cancelled armed pad waits for a change too.
    chord.update(0, select.buttons(), t0 + 3000 * MS);
    chord.cancel();
    chord.update(0, select.buttons(), t0 + 3100 * MS);
    assert_eq!(chord.next_due(), None);
    // L3+R3 is not the chord any more.
    chord.update(1, PAD_CHORD, t0);
    assert_eq!(chord.next_due(), None);
}

#[test]
fn couch_menu_corner_hold_takes_its_own_time() {
    let t0 = Instant::now();
    let mut never = || -> Option<(f32, f32)> { None };
    let mut corner = CornerHold::new(1920, 1080, 500 * MS);
    corner.observe(&touch(TOUCH_DOWN, 1, 0.01, 0.01), t0, true);
    assert!(!corner.fire(t0 + 499 * MS, &mut never));
    assert!(corner.fire(t0 + 500 * MS, &mut never));
}

#[test]
fn couch_menu_open_with_reads_the_settings_and_refuses_hand_edits_loudly() {
    let config = Config::parse(
        "couch_menu_open_with = keyboard\n\
             couch_menu_controller_chord = select+y\n\
             couch_menu_hold_ms = 300\n\
             couch_menu_corner_hold_ms = 3000\n\
             couch_menu_keyboard_chord = ctrl+alt+end\n",
    )
    .unwrap();
    let mut refused = Vec::new();
    let read = OpenWith::read(&config, &mut |k, v, e| {
        refused.push(format!("{k}={v}: {e}"))
    });
    assert_eq!(
        read,
        OpenWith {
            controller: false,
            keyboard: true,
            touch: false,
            pad_chord: PadButtons::parse("select+y").unwrap(),
            pad_hold: 300 * MS,
            corner_hold: 3000 * MS,
            key_chord: KeyChord::parse("ctrl+alt+end").unwrap(),
        }
    );
    assert!(refused.is_empty(), "{refused:?}");
    assert_eq!(
        OpenWith::from_config(&Config::default()),
        OpenWith::default()
    );

    let config = Config::parse(
        "couch_menu_open_with = controler\n\
             couch_menu_controller_chord = start+select\n\
             couch_menu_hold_ms = 299\n\
             couch_menu_corner_hold_ms = 3001\n\
             couch_menu_keyboard_chord = ctrl+alt+shift+m\n",
    )
    .unwrap();
    let mut refused = Vec::new();
    let read = OpenWith::read(&config, &mut |k, v, e| {
        refused.push(format!("{k}={v}: {e}"))
    });
    assert_eq!(read, OpenWith::default(), "every bad value falls back");
    // Each report carries the error that refused the value.
    let errors = [
        OpenWith::parse("controler").unwrap_err(),
        PadButtons::parse("start+select").unwrap_err(),
        parse_hold("couch_menu_hold_ms", "299", PAD_HOLD_MS).unwrap_err(),
        parse_hold("couch_menu_corner_hold_ms", "3001", CORNER_HOLD_MS).unwrap_err(),
        KeyChord::parse("ctrl+alt+shift+m").unwrap_err(),
    ];
    let expected: Vec<String> = [
        "couch_menu_open_with=controler",
        "couch_menu_controller_chord=start+select",
        "couch_menu_hold_ms=299",
        "couch_menu_corner_hold_ms=3001",
        "couch_menu_keyboard_chord=ctrl+alt+shift+m",
    ]
    .iter()
    .zip(errors)
    .map(|(entry, error)| format!("{entry}: {error}"))
    .collect();
    assert_eq!(refused, expected);
    assert!(refused[1].ends_with("into Guide on pads without a Home button"));
}

#[test]
fn couch_menu_select_chord_warns_when_the_host_turns_select_into_guide_first() {
    let read = |text: &str| {
        let config = Config::parse(text).unwrap();
        let open_with = OpenWith::from_config(&config);
        open_with.select_conflict(crate::input_policy::back_button_timeout(&config))
    };
    let select = "couch_menu_controller_chord = select
";
    // Off by default and for any negative value.
    assert_eq!(read(select), None);
    assert_eq!(
        read(&format!(
            "{select}back_button_timeout = -1
"
        )),
        None
    );
    // On from 0 up to, not including, the hold.
    for timeout in [0, 500, 999] {
        let warning = read(&format!(
            "{select}back_button_timeout = {timeout}
"
        ))
        .unwrap();
        assert!(
            warning.contains(&format!("Guide (Xbox Game Bar) after {timeout} ms")),
            "{warning}"
        );
        assert!(warning.contains("holds Select for 1000 ms"), "{warning}");
    }
    assert_eq!(
        read(&format!(
            "{select}back_button_timeout = 1000
"
        )),
        None
    );
    // The hold counts, and so does any chord holding Select.
    let short = "couch_menu_hold_ms = 400
back_button_timeout = 500
";
    assert_eq!(read(&format!("{select}{short}")), None);
    let long = "couch_menu_hold_ms = 600
back_button_timeout = 500
";
    assert!(read(&format!("{select}{long}")).is_some());
    assert!(
        read(&format!(
            "couch_menu_controller_chord = select+y
{long}"
        ))
        .is_some()
    );
    assert_eq!(read(long), None, "l3+r3 holds no Select");
    // No controller way, no conflict.
    assert_eq!(
        read(&format!(
            "{select}{long}couch_menu_open_with = keyboard
"
        )),
        None
    );
}

#[test]
fn couch_menu_corner_box_is_six_percent_of_the_shorter_side() {
    let corner = CornerHold::new(1920, 1080, CORNER_HOLD);
    // 64.8 px on each axis.
    assert!(corner.inside((64. / 1920., 64. / 1080.)));
    assert!(!corner.inside((65. / 1920., 10. / 1080.)));
    assert!(!corner.inside((10. / 1920., 65. / 1080.)));
    let portrait = CornerHold::new(1080, 2400, CORNER_HOLD);
    assert!(portrait.inside((64. / 1080., 64. / 2400.)));
    assert!(!portrait.inside((64. / 1080., 66. / 2400.)));
}

/// Each source: the hold fires at 1500 ms, not 1499, and leaving the box
/// or letting go cancels it.
#[test]
fn couch_menu_corner_hold_for_each_source() {
    let t0 = Instant::now();
    let mut never = || -> Option<(f32, f32)> { panic!("only relative holds read the cursor") };
    let sources: [(&str, Vec<Input>, Input, Input); 3] = [
        (
            "touch",
            vec![touch(TOUCH_DOWN, 7, 0.01, 0.02)],
            touch(TOUCH_MOVE, 7, 0.5, 0.02),
            touch(TOUCH_UP, 7, 0.01, 0.02),
        ),
        (
            "pen",
            vec![pen(TOUCH_DOWN, 0.01, 0.02)],
            pen(TOUCH_MOVE, 0.01, 0.5),
            pen(TOUCH_UP, 0.01, 0.02),
        ),
        (
            "absolute mouse",
            vec![absolute(20, 20), button(true)],
            absolute(900, 20),
            button(false),
        ),
    ];
    for (name, down, leave, up) in sources {
        let mut corner = CornerHold::new(1920, 1080, CORNER_HOLD);
        for e in &down {
            corner.observe(e, t0, true);
        }
        assert_eq!(corner.next_due(), Some(t0 + 1500 * MS), "{name}");
        assert!(!corner.fire(t0 + 1499 * MS, &mut never), "{name}");
        assert!(corner.fire(t0 + 1500 * MS, &mut never), "{name}");
        assert_eq!(corner.next_due(), None, "{name}");
        for cancel in [leave, up] {
            let mut corner = CornerHold::new(1920, 1080, CORNER_HOLD);
            for e in &down {
                corner.observe(e, t0, true);
            }
            corner.observe(&cancel, t0 + 700 * MS, true);
            assert_eq!(corner.next_due(), None, "{name}: {cancel:?}");
            assert!(!corner.fire(t0 + 2000 * MS, &mut never), "{name}");
        }
        let mut corner = CornerHold::new(1920, 1080, CORNER_HOLD);
        for e in &down {
            corner.observe(e, t0, false);
        }
        assert_eq!(corner.next_due(), None, "{name}: no hold while not armed");
    }
    // Outside the box nothing starts.
    let mut corner = CornerHold::new(1920, 1080, CORNER_HOLD);
    corner.observe(&touch(TOUCH_DOWN, 1, 0.2, 0.2), t0, true);
    corner.observe(&absolute(300, 300), t0, true);
    corner.observe(&button(true), t0, true);
    assert_eq!(corner.next_due(), None);
    // A second finger cancels a touch hold.
    let mut corner = CornerHold::new(1920, 1080, CORNER_HOLD);
    corner.observe(&touch(TOUCH_DOWN, 1, 0.01, 0.01), t0, true);
    corner.observe(&touch(TOUCH_DOWN, 2, 0.5, 0.5), t0 + 10 * MS, true);
    assert_eq!(corner.next_due(), None);
}

#[test]
fn couch_menu_relative_mouse_hold_reads_the_cursor_only_when_it_fires() {
    let t0 = Instant::now();
    let mut corner = CornerHold::new(1920, 1080, CORNER_HOLD);
    let mut reads = 0;
    corner.observe(&Input::Relative { x: -5, y: -5 }, t0, true);
    corner.observe(&button(true), t0, true);
    corner.observe(&Input::Relative { x: -5, y: -5 }, t0 + 100 * MS, true);
    let mut in_corner = || {
        reads += 1;
        Some((0.01, 0.01))
    };
    assert!(!corner.fire(t0 + 1499 * MS, &mut in_corner));
    assert!(corner.fire(t0 + 1500 * MS, &mut in_corner));
    assert_eq!(reads, 1, "one read, when the hold was due");
    // Cursor elsewhere or unknown: no open.
    let mut reads = 0;
    for p in [Some((0.5, 0.5)), None] {
        corner.observe(&button(false), t0, true);
        corner.observe(&button(true), t0, true);
        let mut cursor = || {
            reads += 1;
            p
        };
        assert!(!corner.fire(t0 + 1500 * MS, &mut cursor));
    }
    assert_eq!(reads, 2);
}
