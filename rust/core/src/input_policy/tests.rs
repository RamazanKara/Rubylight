use super::*;
/// #74: the chosen ViGEmBus pad is the only one its driver is asked for,
/// whatever the client is, and the retired names stay VHF pads.
#[test]
fn vigem_x360_is_chosen_by_name_only_and_never_replaced() {
    assert_eq!(gamepad_profile("vigem_x360"), VIGEM_X360);
    assert_eq!(gamepad_profile("x360"), VHF_XBOX_ONE);
    let policy = Policy::resolve(&Config::default()).unwrap();
    for kind in 0..=4 {
        for caps in [0, 8, 0x30, 0x38] {
            assert_eq!(
                policy.controller_profile(VIGEM_X360, kind, caps, VIGEM_PROFILES),
                Some(VIGEM_X360)
            );
        }
    }
    assert_eq!(policy.session_pad, VHF_XBOX_SERIES);
    for other in ["vhf_ds5", "usbip_ds5", "x360"] {
        let config = Config::parse(&format!("gamepad={other}\n")).unwrap();
        assert_eq!(
            Policy::resolve(&config).unwrap().session_pad,
            VHF_XBOX_SERIES
        );
    }
    let config = Config::parse("gamepad=vigem_x360\n").unwrap();
    assert_eq!(Policy::resolve(&config).unwrap().session_pad, VIGEM_X360);
}
/// #108: the ViGEmBus DualShock 4 is chosen by name only, whatever the client
/// is; a co-op session, which needs XInput, gets ViGEmBus's Xbox 360 pad.
#[test]
fn vigem_ds4_is_chosen_by_name_only_and_never_replaced() {
    assert_eq!(gamepad_profile("vigem_ds4"), VIGEM_DS4);
    assert_eq!(gamepad_profile("ds4"), VHF_DS4);
    let policy = Policy::resolve(&Config::default()).unwrap();
    for kind in 0..=4 {
        for caps in [0, 8, 0x30, 0x38] {
            assert_eq!(
                policy.controller_profile(VIGEM_DS4, kind, caps, VIGEM_PROFILES),
                Some(VIGEM_DS4)
            );
            // Automatic never picks a ViGEmBus pad.
            assert!(!matches!(
                policy.controller_profile(0, kind, caps, VIGEM_PROFILES),
                Some(VIGEM_X360 | VIGEM_DS4)
            ));
        }
    }
    let config = Config::parse("gamepad=vigem_ds4\n").unwrap();
    let policy = Policy::resolve(&config).unwrap();
    assert_eq!(policy.session_pad, VIGEM_X360);
    // Vibepollo's default: Back also clicks the touchpad.
    assert!(policy.ds4_back_as_touchpad_click);
    let config = Config::parse("ds4_back_as_touchpad_click=false\n").unwrap();
    assert!(!Policy::resolve(&config).unwrap().ds4_back_as_touchpad_click);
}
#[test]
fn only_a_switch2_pro_or_the_named_choice_gets_the_usb_switch2_pad() {
    assert_eq!(gamepad_profile("usbip_switch2"), USBIP_SWITCH2);
    let pro = Some((0x057E, 0x2069));
    assert!(wants_switch2(0, pro));
    assert!(wants_switch2(VHF_AUTO, pro));
    // A Switch 1 Pro Controller and controllers without an identity stay as they were.
    assert!(!wants_switch2(0, Some((0x057E, 0x2009))));
    assert!(!wants_switch2(0, None));
    // A pad chosen by name wins over the identity, and the named Switch 2 pad
    // is every controller's.
    assert!(!wants_switch2(VHF_SWITCH, pro));
    assert!(!wants_switch2(VHF_XBOX_SERIES, pro));
    assert!(wants_switch2(USBIP_SWITCH2, None));
}
#[test]
fn retired_vigem_choices_select_the_vhf_pad_of_their_family() {
    assert_eq!(gamepad_profile("x360"), VHF_XBOX_ONE);
    assert_eq!(gamepad_profile("ds4"), VHF_DS4);
    assert_eq!(gamepad_profile("vhf_xbox_one"), VHF_XBOX_ONE);
    assert_eq!(gamepad_profile("vhf_ds4"), VHF_DS4);
    assert_eq!(gamepad_profile("auto"), 0);
    let policy = Policy::resolve(&Config::default()).unwrap();
    let vhf = (1 << 2) | (1 << 3) | (1 << 4) | (1 << 5) | (1 << 6);
    // Whatever the client is, including an Android phone with no motion
    // or touchpad and one that sends no arrival at all.
    for kind in 0..=4 {
        for caps in [0, 8, 0x30, 0x38] {
            assert_eq!(
                policy.controller_profile(gamepad_profile("x360"), kind, caps, vhf),
                Some(VHF_XBOX_ONE)
            );
            assert_eq!(
                policy.controller_profile(gamepad_profile("ds4"), kind, caps, vhf),
                Some(VHF_DS4)
            );
        }
    }
    // A driver without the Xbox One profile still gives an Xbox pad.
    assert_eq!(
        policy.controller_profile(gamepad_profile("x360"), 1, 0, 1 << 3),
        Some(VHF_XBOX_SERIES)
    );
}
#[test]
fn independent_controllers_match_client_type_and_available_driver_profiles() {
    let policy = Policy::resolve(&Config::default()).unwrap();
    let all = (1 << 2) | (1 << 3) | (1 << 4) | (1 << 5) | (1 << 6);
    assert_eq!(policy.controller_profile(0, 1, 0x4, all), Some(4));
    assert_eq!(policy.controller_profile(0, 2, 0, all), Some(6));
    assert_eq!(policy.controller_profile(0, 3, 0x30, all), Some(7));
    assert_eq!(policy.controller_profile(0, 0, 8, all), Some(6));
    assert_eq!(policy.controller_profile(5, 3, 0, all), Some(5));
    assert_eq!(policy.controller_profile(0, 2, 0, 1 << 3), Some(4));
    // An Xbox-type controller with motion or a touchpad (a Steam Deck), as
    // in Vibepollo; without either it stays an Xbox pad.
    assert_eq!(policy.controller_profile(0, 1, 0x30 | 8, all), Some(6));
    assert_eq!(policy.controller_profile(0, 1, 0x30, all), Some(6));
    let plain =
        Policy::resolve(&Config::parse("motion_as_ds4=false\ntouchpad_as_ds4=false\n").unwrap())
            .unwrap();
    assert_eq!(plain.controller_profile(0, 1, 0x30 | 8, all), Some(4));
}
#[test]
fn a_nintendo_controller_keeps_its_gyro_on_a_driver_without_the_switch_profile() {
    let policy = Policy::resolve(&Config::default()).unwrap();
    let no_switch = (1 << 2) | (1 << 3) | (1 << 4) | (1 << 5);
    assert_eq!(
        policy.controller_profile(0, 3, 0x30, no_switch),
        Some(VHF_DUALSENSE)
    );
    assert_eq!(
        policy.controller_profile(0, 3, 0x30, (1 << 3) | (1 << 4)),
        Some(VHF_DS4)
    );
    // Without motion sensors, or with only Xbox pads, it is an Xbox pad.
    assert_eq!(
        policy.controller_profile(0, 3, 0, no_switch),
        Some(VHF_XBOX_SERIES)
    );
    assert_eq!(
        policy.controller_profile(0, 3, 0x30, 1 << 3),
        Some(VHF_XBOX_SERIES)
    );
    // A PlayStation controller whose DualSense is missing keeps motion on a DS4.
    assert_eq!(
        policy.controller_profile(0, 2, 0x30, (1 << 3) | (1 << 4)),
        Some(VHF_DS4)
    );
}
#[test]
fn a_steam_deck_with_its_sensors_gets_a_dualsense_and_without_them_an_xbox_pad() {
    let policy = Policy::resolve(&Config::default()).unwrap();
    let vhf = (1 << 2) | (1 << 3) | (1 << 4) | (1 << 5) | (1 << 6);
    // What Moonlight announces for a Deck with Steam Input off: gyro and
    // accelerometer, and with SDL 3 both trackpads.
    for caps in [0x30, 0x38, 0x138] {
        assert_eq!(
            policy.controller_profile(0, CTYPE_STEAM, caps, vhf),
            Some(VHF_DUALSENSE)
        );
    }
    assert_eq!(policy.controller_profile(0, CTYPE_STEAM, 0, vhf), Some(4));
    let plain =
        Policy::resolve(&Config::parse("motion_as_ds4=false\ntouchpad_as_ds4=false\n").unwrap())
            .unwrap();
    assert_eq!(
        plain.controller_profile(0, CTYPE_STEAM, 0x138, vhf),
        Some(4)
    );
    // An explicit profile is kept.
    assert_eq!(
        policy.controller_profile(VHF_DS4, CTYPE_STEAM, 0x138, vhf),
        Some(VHF_DS4)
    );
}
#[test]
fn the_steam_controller_and_a_switch2_pro_carry_their_back_grips() {
    let all = PADDLES.iter().fold(0, |mask, flag| mask | flag);
    assert_eq!(pad_back_grips(VHF_STEAM_CONTROLLER, None), all);
    assert_eq!(pad_back_grips(USBIP_SWITCH2, None), PADDLES[0] | PADDLES[1]);
    use rubylight_protocol::controller_identity::{NINTENDO, SWITCH2_PRO};
    // Automatic: a client's Switch 2 Pro Controller becomes one.
    assert_eq!(
        pad_back_grips(0, Some((NINTENDO, SWITCH2_PRO))),
        PADDLES[0] | PADDLES[1]
    );
    assert_eq!(pad_back_grips(0, None), 0);
    assert_eq!(
        pad_back_grips(VHF_XBOX_SERIES, Some((NINTENDO, SWITCH2_PRO))),
        0
    );
}

#[test]
fn back_grips_pass_through_until_mapped() {
    let all = PADDLES.iter().fold(0, |mask, flag| mask | flag);
    let policy = Policy::resolve(&Config::default()).unwrap();
    assert_eq!(policy.back_grips, [None; 4]);
    assert_eq!(policy.unmapped_back_grips(), all);
    assert_eq!(
        policy.map_back_grips(all | 0x1000, 7, 9),
        (all | 0x1000, 7, 9)
    );
}
#[test]
fn touchpad_click_and_misc_reach_the_pad_unchanged() {
    // A client's remapped extra buttons arrive as these flags; the VHF
    // driver gets them as they are, so nothing on the way may drop them.
    let extended = 0x10_0000 | 0x20_0000;
    let mapped = Policy::resolve(
        &Config::parse("back_grip_r4=a\nback_grip_l4=b\nback_grip_r5=x\nback_grip_l5=y\n").unwrap(),
    )
    .unwrap();
    for policy in [Policy::resolve(&Config::default()).unwrap(), mapped] {
        assert_eq!(policy.map_back_grips(extended, 0, 0), (extended, 0, 0));
    }
    let mut back = BackButton::default();
    let timeout = Some(Duration::from_millis(500));
    assert_eq!(
        back.update(extended | 0x20, Duration::ZERO, timeout),
        extended | 0x20
    );
    assert_eq!(
        back.update(extended, Duration::from_millis(10), timeout),
        extended
    );
}
#[test]
fn mapped_back_grips_press_their_button_or_trigger_instead() {
    let policy = Policy::resolve(
        &Config::parse("back_grip_r4=a\nback_grip_l4=lt\nback_grip_r5=rt\nback_grip_l5=nonsense\n")
            .unwrap(),
    )
    .unwrap();
    let [r4, l4, r5, l5] = PADDLES;
    assert_eq!(
        policy.back_grips,
        [
            Some(GripTarget::Buttons(0x1000)),
            Some(GripTarget::LeftTrigger),
            Some(GripTarget::RightTrigger),
            None
        ]
    );
    assert_eq!(policy.unmapped_back_grips(), l5);
    // R4 presses A; held with A itself, A stays down.
    assert_eq!(policy.map_back_grips(r4, 0, 0), (0x1000, 0, 0));
    assert_eq!(policy.map_back_grips(r4 | 0x1000, 0, 0), (0x1000, 0, 0));
    // The trigger grips pull their trigger all the way, whatever it was at.
    assert_eq!(policy.map_back_grips(l4 | 0x10, 40, 50), (0x10, 255, 50));
    assert_eq!(policy.map_back_grips(r5, 40, 50), (0, 40, 255));
    // Released grips press nothing; the unmapped one passes through.
    assert_eq!(policy.map_back_grips(0x2000, 40, 50), (0x2000, 40, 50));
    assert_eq!(policy.map_back_grips(l5, 0, 0), (l5, 0, 0));
    // Every value the console offers maps to something.
    for value in [
        "a",
        "b",
        "x",
        "y",
        "lb",
        "rb",
        "lt",
        "rt",
        "l3",
        "r3",
        "back",
        "start",
        "guide",
        "dpad_up",
        "dpad_down",
        "dpad_left",
        "dpad_right",
        "touchpad",
        "misc",
    ] {
        assert!(grip_target("back_grip_r4", value).is_some(), "{value}");
    }
    assert_eq!(grip_target("back_grip_r4", "none"), None);
}
#[test]
fn the_steam_input_hint_names_a_deck_whose_sensors_did_not_arrive() {
    for name in ["steamdeck", "Steam Deck", "steam-deck (OLED)"] {
        let hint = steam_input_hint(name, 0x47).unwrap();
        assert!(hint.starts_with(name));
        assert!(hint.contains("disable Steam Input"));
        assert!(steam_input_hint(name, 0x03).is_some());
    }
    // A Deck that passes its own controls on, and other devices.
    assert_eq!(steam_input_hint("Steam Deck", 0x138), None);
    assert_eq!(steam_input_hint("Steam Deck", 0x30), None);
    assert_eq!(steam_input_hint("Pixel 8", 0), None);
    assert_eq!(steam_input_hint("Deck", 0), None);
}
#[test]
fn the_steam_deck_controller_setting_falls_back_to_auto() {
    assert_eq!(deck_pad("auto"), DeckPad::Auto);
    assert_eq!(deck_pad("steam_deck"), DeckPad::SteamDeck);
    assert_eq!(deck_pad("virtual_pad"), DeckPad::VirtualPad);
    assert_eq!(deck_pad("dualsense"), DeckPad::Auto);
    let policy = Policy::resolve(&Default::default()).unwrap();
    assert_eq!(policy.steam_deck, DeckPad::Auto);
}
#[test]
fn key_repeat_delay_follows_vibepollo() {
    let delay = |value: &str| {
        let config = Config::parse(&format!("key_repeat_delay={value}\n")).unwrap();
        Policy::resolve(&config).unwrap().repeat_delay
    };
    assert_eq!(delay("0"), None);
    assert_eq!(delay("-5"), Some(Duration::from_millis(500)));
    assert_eq!(delay("250"), Some(Duration::from_millis(250)));
    assert_eq!(delay("999999"), Some(Duration::from_millis(60000)));
    assert_eq!(
        Policy::resolve(&Config::default()).unwrap().repeat_delay,
        Some(Duration::from_millis(500))
    );
}
#[test]
fn the_absolute_left_release_delay_defaults_to_sunshines_and_zero_turns_it_off() {
    let delay = |value: &str| {
        let config = Config::parse(&format!("absolute_left_release_delay={value}\n")).unwrap();
        Policy::resolve(&config).unwrap().left_release_delay
    };
    assert_eq!(
        Policy::resolve(&Config::default())
            .unwrap()
            .left_release_delay,
        Duration::from_millis(10)
    );
    assert_eq!(delay("0"), Duration::ZERO);
    assert_eq!(delay("-1"), Duration::from_millis(10));
    assert_eq!(delay("40"), Duration::from_millis(40));
    assert_eq!(delay("999999"), Duration::from_millis(1000));
}
#[test]
fn key_repeat_frequency_cannot_overflow_the_repeat_timer() {
    let period = |value: &str| {
        let config = Config::parse(&format!("key_repeat_frequency={value}\n")).unwrap();
        Policy::resolve(&config).unwrap().repeat_period
    };
    let default = Duration::from_secs_f64(1.0 / 24.9);
    for value in ["0", "1e-30", "5e-324", "NaN", "inf", "-1", "1001"] {
        assert_eq!(period(value), default, "{value}");
    }
    for value in ["1e-19", "1e-15", "0.001"] {
        assert!(
            std::time::Instant::now()
                .checked_add(period(value))
                .is_some()
        );
    }
    assert_eq!(period("0.5"), Duration::from_secs(2));
    assert_eq!(period("1000"), Duration::from_millis(1));
}
#[test]
fn a_quoted_key_repeat_frequency_is_read_like_the_console_shows_it() {
    let config = Config::parse("key_repeat_frequency=\"30\"\n").unwrap();
    assert_eq!(
        Policy::resolve(&config).unwrap().repeat_period,
        Duration::from_secs_f64(1.0 / 30.0)
    );
    let config = Config::parse("key_repeat_frequency=\" 12.5 \"\n").unwrap();
    assert_eq!(
        Policy::resolve(&config).unwrap().repeat_period,
        Duration::from_millis(80)
    );
}
#[test]
fn held_back_emits_one_home_pulse_and_requires_release_before_rearming() {
    let mut button = BackButton::default();
    let delay = Some(Duration::from_millis(500));
    let at = Duration::from_millis;
    assert_eq!(button.update(0x20 | 0x1000, at(0), delay), 0x1020);
    assert_eq!(button.poll(at(499), delay), None);
    assert_eq!(button.poll(at(500), delay), Some(0x1400));
    assert_eq!(button.update(0x20 | 0x2000, at(550), delay), 0x2400);
    assert_eq!(button.poll(at(600), delay), Some(0x2000));
    assert_eq!(button.poll(at(5000), delay), None);
    assert_eq!(button.update(0, at(5001), delay), 0);
    assert_eq!(button.update(0x20, at(5002), None), 0x20);
    assert_eq!(button.poll(at(8000), None), None);
}
#[test]
fn disabled_devices_and_explicit_right_alt_mapping_keep_legacy_precedence() {
    let p=Policy::resolve(&Config::parse("keyboard=disabled\nmouse=enabled\ncontroller=false\nkeybindings=[165,166]\nkey_rightalt_to_key_win=true\n").unwrap()).unwrap();
    assert!(!p.allows(&Input::Text("a".into())));
    assert!(p.allows(&Input::Relative { x: 2, y: -3 }));
    assert!(!p.allows(&Input::Arrival {
        id: 0,
        kind: 0,
        capabilities: 0,
        buttons: 0
    }));
    assert_eq!(p.key(0xa5), 0xa6);
    assert_eq!(p.key(0x11), 0xa2);
}

#[test]
fn mouse_mode_defaults_to_auto_and_reads_foundations_names() {
    assert_eq!(
        Policy::resolve(&Config::default()).unwrap().mouse_mode,
        MouseMode::Auto
    );
    for (value, mode) in [
        ("auto", MouseMode::Auto),
        ("virtual_mouse", MouseMode::VirtualMouse),
        ("vmouse", MouseMode::VirtualMouse),
        ("injection", MouseMode::Injection),
        ("SendInput", MouseMode::Injection),
        ("nonsense", MouseMode::Auto),
    ] {
        let config = Config::parse(&format!(
            "mouse_mode = {value}
"
        ))
        .unwrap();
        assert_eq!(MouseMode::resolve(&config), mode, "{value}");
    }
    assert!(MouseMode::Auto.wants_virtual());
    assert!(!MouseMode::Injection.wants_virtual());
}

#[test]
fn touchpad_packets_need_the_mouse_and_native_touchpad() {
    let touchpad = Input::Touchpad {
        frame: true,
        buttons: 0,
        width_mm: 0,
        height_mm: 0,
        contacts: vec![],
    };
    assert!(
        Policy::resolve(&Config::default())
            .unwrap()
            .allows(&touchpad)
    );
    for text in [
        "native_touchpad = false
",
        "mouse = false
",
    ] {
        let policy = Policy::resolve(&Config::parse(text).unwrap()).unwrap();
        assert!(!policy.allows(&touchpad), "{text}");
    }
}
