//! Unit tests for gamepad mappings, dead zones, clickpad emulation, DualSense effects, and gyroscope config.

use nightjar_core::input::packets::{button, capability};

use super::*;

#[test]
fn stick_y_inverts_without_overflow() {
    assert_eq!(invert(-32768), 32767);
    assert_eq!(invert(32767), -32767);
    assert_eq!(invert(0), 0);
}

/// The C button's flag (0x40_0000) is a Steam Controller's left grip touch, a
/// bit this client never sends: only a host that takes controller identities
/// knows it as C (nightjar#125).
#[test]
fn the_c_button_goes_only_to_a_host_that_takes_controller_identities() {
    assert_eq!(
        host_buttons(button::C | button::A, true),
        button::C | button::A
    );
    assert_eq!(host_buttons(button::C | button::A, false), button::A);
    assert_eq!(host_buttons(button::C, false), 0);
}

#[test]
fn button_map_follows_sdl_order() {
    assert_eq!(BUTTON_MAP[SDL_GAMEPAD_BUTTON_SOUTH.0 as usize], button::A);
    assert_eq!(BUTTON_MAP[SDL_GAMEPAD_BUTTON_START.0 as usize], button::PLAY);
    assert_eq!(BUTTON_MAP[SDL_GAMEPAD_BUTTON_DPAD_RIGHT.0 as usize], button::RIGHT);
    assert_eq!(BUTTON_MAP[SDL_GAMEPAD_BUTTON_TOUCHPAD.0 as usize], button::TOUCHPAD);
}

#[test]
fn mappings_apply_once_preserve_explicit_bindings_and_scale_dead_zones() {
    let mut mapping = Mapping::default();
    assert_eq!(mapping.map_buttons(1, true), button::B);
    mapping.buttons[0] = Some(button::X);
    mapping.buttons[2] = Some(button::Y);
    assert_eq!(mapping.map_buttons(1, true), button::X);
    assert_eq!(mapping.map_buttons(1 | 4, false), button::X | button::Y);
    mapping.axes = [-4, 3, 0, -1, 6, 0];
    let report = mapping.apply(0, [-32768, 1000, -2000, 32767, 0, 32767], PadSettings::default());
    assert_eq!(report.ls, (-32767, 2000));
    assert_eq!(report.rs, (0, -32767));
    assert_eq!((report.lt, report.rt), (255, 0));
    assert_eq!(deadzone((3000, 3000), 20), (0, 0));
    assert_eq!((trigger(1, 0), trigger(-5, 0), trigger(32767, 0)), (0, 0, 255));
    assert_eq!(trigger(3300, 0), 25, "no dead zone by default: a 10% press goes through");
    assert_eq!((trigger(3276, 10), trigger(3400, 10), trigger(32767, 10)), (0, 1, 255));
    assert_eq!(trigger(19660, 20), 127, "60% raw is 50% of the travel past a 20% dead zone");
    mapping.trigger_deadzone = 20;
    // lt reads source 6 here
    assert_eq!(mapping.apply(0, [0, 0, 0, 0, 0, 6000], PadSettings::default()).lt, 0);
    assert_eq!(mapping.apply(0, [0, 0, 0, 0, 0, 19660], PadSettings::default()).lt, 127);
    assert!(mapping.valid());
    mapping.trigger_deadzone = MAX_TRIGGER_DEADZONE + 1;
    assert!(!mapping.valid());
    mapping.trigger_deadzone = 0;
    assert_eq!(deadzone((32767, 0), 20), (32767, 0));
    let middle = deadzone((20000, 0), 20);
    assert!((16000..18000).contains(&middle.0));
    assert_eq!(middle.1, 0);
    assert!(mapping.valid());
    mapping.axes[4] = -6;
    assert!(!mapping.valid());
    mapping.axes[4] = 6;
    mapping.buttons[0] = Some(button::A | button::B);
    assert!(!mapping.valid());
}

/// "Stick dead zone" and "Trigger dead zone" in the settings apply to a controller whose
/// buttons page sets none of its own; a mapping's own value wins, each on its own.
#[test]
fn the_settings_dead_zones_fill_in_for_a_mapping_without_its_own() {
    let settings = crate::config::Settings { deadzone: 20, trigger_deadzone: 10, ..Default::default() };
    let applied = PadSettings::from(&settings);
    assert_eq!(applied.dead_zones, DeadZones { stick: 20, trigger: 10 });
    let plain = Mapping::default();
    assert_eq!(plain.dead_zones(applied.dead_zones), DeadZones { stick: 20, trigger: 10 });
    // Inside the settings' 20 % stick and 10 % trigger dead zones nothing reaches the PC.
    let movement = [3000, -3000, 0, 0, 3000, 3000];
    let report = plain.apply(0, movement, applied);
    assert_eq!((report.ls, report.lt, report.rt), ((0, 0), 0, 0));
    // Without them the same movement goes through.
    let raw = plain.apply(0, movement, PadSettings::default());
    assert_eq!(raw.ls, (3000, 3000));
    assert!(raw.lt > 0 && raw.rt > 0);
    // A mapping's own value wins over the settings'.
    let own_stick = Mapping { deadzone: 5, ..Mapping::default() };
    assert_eq!(own_stick.dead_zones(applied.dead_zones), DeadZones { stick: 5, trigger: 10 });
    assert_eq!(own_stick.apply(0, [3000, 0, 0, 0, 0, 0], applied).ls, deadzone((3000, 0), 5));
    let own_trigger = Mapping { trigger_deadzone: 30, ..Mapping::default() };
    assert_eq!(own_trigger.dead_zones(applied.dead_zones), DeadZones { stick: 20, trigger: 30 });
    // Settings past a mapping's range are held to it, and the defaults change nothing.
    let wide = PadSettings::from(&crate::config::Settings { deadzone: 90, trigger_deadzone: 90, ..Default::default() });
    assert_eq!(wide.dead_zones, DeadZones { stick: MAX_STICK_DEADZONE, trigger: MAX_TRIGGER_DEADZONE });
    assert_eq!(PadSettings::from(&crate::config::Settings::default()), PadSettings::default());
    assert_eq!(PadSettings::from(true), PadSettings { swap: true, ..PadSettings::default() });
}

/// "Back button as Guide": the physical Back reaches the PC as Guide, through what the mapping
/// sends for Guide; "Back opens the stream menu" keeps Back for Nightjar instead, as on Android.
#[test]
fn back_as_guide_sends_guide_for_back() {
    let back = 1 << SDL_GAMEPAD_BUTTON_BACK.0;
    let on = PadSettings { back_as_guide: true, ..PadSettings::default() };
    let mapping = Mapping::default();
    assert_eq!(mapping.apply(back, [0; 6], PadSettings::default()).buttons, button::BACK);
    assert_eq!(mapping.apply(back, [0; 6], on).buttons, button::SPECIAL);
    assert_eq!(mapping.apply(back | 1, [0; 6], on).buttons, button::SPECIAL | button::A, "other buttons stay");
    assert_eq!(mapping.apply(1 << SDL_GAMEPAD_BUTTON_GUIDE.0, [0; 6], on).buttons, button::SPECIAL, "Guide stays Guide");
    let mut guide_bound = Mapping::default();
    guide_bound.buttons[SDL_GAMEPAD_BUTTON_GUIDE.0 as usize] = Some(button::MISC);
    assert_eq!(guide_bound.apply(back, [0; 6], on).buttons, button::MISC, "Guide's binding applies");
    let from = |back_as_guide, back_opens_menu| {
        PadSettings::from(&crate::config::Settings { back_as_guide, back_opens_menu, ..Default::default() }).back_as_guide
    };
    assert!(from(true, false));
    assert!(!from(true, true), "Back opens the stream menu wins");
    assert!(!from(false, false) && !from(false, true));
}

#[test]
fn a_pinned_type_replaces_the_detected_one_and_automatic_keeps_it() {
    let mut mapping = Mapping::default();
    assert_eq!(mapping.connect_as, CTYPE_UNKNOWN);
    for detected in [CTYPE_UNKNOWN, CTYPE_XBOX, CTYPE_PS, CTYPE_NINTENDO] {
        assert_eq!(mapping.arrival_type(detected), detected);
    }
    // #53: a pad SDL reports as PlayStation can be told to the host as Xbox.
    mapping.connect_as = CTYPE_XBOX;
    assert_eq!(mapping.arrival_type(CTYPE_PS), CTYPE_XBOX);
    assert_eq!(mapping.arrival_type(CTYPE_UNKNOWN), CTYPE_XBOX);
    mapping.connect_as = CTYPE_PS;
    assert_eq!(mapping.arrival_type(CTYPE_XBOX), CTYPE_PS);
    mapping.connect_as = CTYPE_NINTENDO;
    assert_eq!(mapping.arrival_type(CTYPE_XBOX), CTYPE_NINTENDO);
    assert!(mapping.valid());
    // A value this version does not know is Automatic and keeps the mapping valid.
    mapping.connect_as = 9;
    assert_eq!(mapping.arrival_type(CTYPE_PS), CTYPE_PS);
    assert!(mapping.valid());
    assert_eq!(CONNECT_AS.map(|(kind, _)| kind), [0, 1, 2, 3]);
}

#[test]
fn an_xbox_pin_arrives_without_motion_or_touchpad() {
    let all = capability::ANALOG_TRIGGERS | capability::RUMBLE | capability::TOUCHPAD
        | capability::DUAL_TOUCHPAD | capability::ACCEL | capability::GYRO | capability::BATTERY_STATE;
    let mut mapping = Mapping::default();
    // Automatic and the other types keep what the pad has.
    assert_eq!(mapping.arrival_capabilities(all), all);
    for kind in [CTYPE_PS, CTYPE_NINTENDO] {
        mapping.connect_as = kind;
        assert_eq!(mapping.arrival_capabilities(all), all);
        assert!(!mapping.drops_motion_and_touchpad());
    }
    // Rubylight turns an Xbox pad with motion or a touchpad into a DualSense.
    mapping.connect_as = CTYPE_XBOX;
    assert!(mapping.drops_motion_and_touchpad());
    assert_eq!(mapping.arrival_capabilities(all),
        capability::ANALOG_TRIGGERS | capability::RUMBLE | capability::BATTERY_STATE);
}

#[test]
fn select_lb_sends_the_first_button_then_clicks_the_touchpad_of_a_pad_pinned_as_playstation() {
    let mut mapping = Mapping { connect_as: CTYPE_PS, ..Default::default() };
    assert!(mapping.emulates_clickpad(false));
    assert!(!mapping.emulates_clickpad(true), "a pad with a touchpad click keeps it");
    mapping.buttons[0] = Some(button::TOUCHPAD);
    assert!(!mapping.emulates_clickpad(false), "a button mapped to the touchpad replaces the combo");
    assert!(!Mapping::default().emulates_clickpad(false));
    assert!(!Mapping { connect_as: CTYPE_XBOX, ..Default::default() }.emulates_clickpad(false));

    let (back, lb) = (button::BACK, button::LB);
    let mut swallowed = 0;
    // The first button of the combo goes through: holding it back would add latency.
    assert_eq!(clickpad(back, &mut swallowed), back);
    assert_eq!(clickpad(back | lb, &mut swallowed), button::TOUCHPAD);
    assert_eq!(clickpad(back | lb | button::A, &mut swallowed), button::TOUCHPAD | button::A);
    // Letting go of LB lifts the click; Back stays silent until it is let go too.
    assert_eq!(clickpad(back, &mut swallowed), 0);
    assert_eq!(clickpad(back | lb, &mut swallowed), lb);
    assert_eq!(clickpad(0, &mut swallowed), 0);
    assert_eq!(swallowed, 0);
    // Select+LB as part of a larger chord is left alone.
    assert_eq!(clickpad(back | lb | button::PLAY, &mut swallowed), back | lb | button::PLAY);
}

#[test]
fn dualsense_effect_changes_only_requested_triggers() {
    let effect = trigger_effect(0xFF, 0x21, 0x26, &[3; 10], &[7; 10]);
    assert_eq!(effect[0], 0x0C);
    assert_eq!(&effect[1..10], &[0; 9]);
    assert_eq!(effect[10], 0x26);
    assert_eq!(&effect[11..21], &[7; 10]);
    assert_eq!(effect[21], 0x21);
    assert_eq!(&effect[22..32], &[3; 10]);
    assert_eq!(&effect[32..], &[0; 15]);
}

#[test]
fn gyro_is_sent_as_sdl_reads_it_until_a_mapping_changes_it() {
    let mut mapping = Mapping::default();
    assert_eq!(mapping.map_gyro([1.0, -2.0, 3.0]), [1.0, -2.0, 3.0]);
    // Each axis games read takes a controller axis, optionally inverted (Android's Flydigi preset).
    mapping.gyro_axes = [-2, 3, 1];
    assert!(mapping.valid());
    assert_eq!(mapping.map_gyro([1.0, 2.0, 3.0]), [-2.0, 3.0, 1.0]);
    // Up and down scales X, left and right scales Y, tilt is never scaled.
    mapping.gyro_axes = [1, 2, 3];
    mapping.gyro_pitch = 50;
    mapping.gyro_yaw = 200;
    assert_eq!(mapping.map_gyro([2.0, 2.0, 2.0]), [1.0, 4.0, 2.0]);
    assert!(mapping.valid());
}

#[test]
fn a_gyro_axis_change_swaps_so_each_source_stays_used_once() {
    let mut mapping = Mapping::default();
    mapping.set_gyro_axis(0, -2);
    assert_eq!(mapping.gyro_axes, [-2, 1, 3]);
    mapping.set_gyro_axis(2, 2);
    assert_eq!(mapping.gyro_axes, [-3, 1, 2], "the axis that had Y takes Z and keeps its inversion");
    mapping.set_gyro_axis(1, -1);
    assert_eq!(mapping.gyro_axes, [-3, -1, 2], "inverting in place swaps nothing");
    assert!(mapping.valid());
}

#[test]
fn a_gyro_mapping_out_of_range_is_refused() {
    for axes in [[1, 1, 3], [0, 2, 3], [4, 2, 3], [-1, -1, 2]] {
        assert!(!Mapping { gyro_axes: axes, ..Mapping::default() }.valid(), "{axes:?}");
    }
    assert!(!Mapping { gyro_yaw: MIN_GYRO_SPEED - 1, ..Mapping::default() }.valid());
    assert!(!Mapping { gyro_pitch: MAX_GYRO_SPEED + 1, ..Mapping::default() }.valid());
    assert!(Mapping { gyro_yaw: MIN_GYRO_SPEED, gyro_pitch: MAX_GYRO_SPEED, ..Mapping::default() }.valid());
}

#[test]
fn a_mapping_saved_before_gyro_settings_loads_with_the_gyro_unchanged() {
    let old: Mapping = serde_json::from_str(r#"{"name":"Pad","deadzone":10}"#).expect("an older mapping");
    assert_eq!((old.gyro_axes, old.gyro_yaw, old.gyro_pitch), ([1, 2, 3], 100, 100));
    assert!(old.valid());
}
