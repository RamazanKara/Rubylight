//! Tests for direct mouse motion, event-ordering marks and adaptive controller polling.

use super::*;

#[test]
fn speed_scales_and_fractions_carry_over() {
    let mut rest = (0.0, 0.0);
    assert_eq!(scale(&mut rest, 1.0, -1.0, 1.5), Some((1, -1)));
    assert_eq!(scale(&mut rest, 1.0, -1.0, 1.5), Some((2, -2)), "the halves add up");
    assert_eq!(scale(&mut rest, 1.0, 0.0, 0.25), None);
    assert_eq!(rest, (0.25, 0.0));
    assert_eq!(speed_factor(10), 0.25);
    assert_eq!(speed_factor(1000), 4.0);
}

#[test]
fn motion_leaves_at_once_unless_a_click_waits() {
    let mouse = SDL_MouseID(7);
    assert_eq!(verdict(SDL_EVENT_MOUSE_MOTION, mouse, true, 0, false, false), Verdict::Sent);
    assert_eq!(verdict(SDL_EVENT_MOUSE_MOTION, mouse, true, 1, false, false), Verdict::Pass, "behind a click");
    assert_eq!(verdict(SDL_EVENT_MOUSE_MOTION, mouse, false, 0, false, false), Verdict::Pass, "not captured");
    for kind in [SDL_EVENT_MOUSE_BUTTON_DOWN, SDL_EVENT_MOUSE_BUTTON_UP, SDL_EVENT_MOUSE_WHEEL] {
        assert_eq!(verdict(kind, mouse, true, 0, false, false), Verdict::Ordered);
        assert_eq!(verdict(kind, mouse, false, 0, false, false), Verdict::Ordered, "queued before the capture");
    }
    assert_eq!(verdict(SDL_EVENT_KEY_DOWN, mouse, true, 0, false, false), Verdict::Pass);
}

#[test]
fn touch_and_pen_stay_on_the_main_loop_with_touch_on() {
    for which in [SDL_TOUCH_MOUSEID, SDL_PEN_MOUSEID] {
        assert_eq!(verdict(SDL_EVENT_MOUSE_MOTION, which, true, 0, true, false), Verdict::Pass);
        assert_eq!(verdict(SDL_EVENT_MOUSE_MOTION, which, true, 0, false, false), Verdict::Sent);
    }
}

fn event(kind: SDL_EventType) -> SDL_Event {
    // SAFETY: SDL_Event is plain data; all zero is a valid value.
    let mut event: SDL_Event = unsafe { std::mem::zeroed() };
    event.r#type = kind.0;
    event
}

/// Button down, then a motion SDL built where the marked down was, then button up: the stale
/// mark is cleared, so the queued button up still holds motion back after the down went out.
#[test]
fn a_stale_mark_does_not_let_motion_pass_a_queued_release() {
    let (ordered, queued) = (AtomicU32::new(0), AtomicU32::new(0));
    let mut down = event(SDL_EVENT_MOUSE_BUTTON_DOWN);
    mark(&mut down, ORDERED, Some(&ordered));
    let mut motion = down;
    motion.r#type = SDL_EVENT_MOUSE_MOTION.0;
    mark(&mut motion, 0, None);
    let mut up = down;
    up.r#type = SDL_EVENT_MOUSE_BUTTON_UP.0;
    mark(&mut up, ORDERED, Some(&ordered));
    assert_eq!(ordered.load(Ordering::Acquire), 2);
    release(&ordered, &queued, &down);
    release(&ordered, &queued, &motion);
    assert_eq!(ordered.load(Ordering::Acquire), 1, "the release still waits");
    release(&ordered, &queued, &up);
    release(&ordered, &queued, &up);
    assert_eq!(ordered.load(Ordering::Acquire), 0, "and never below zero");
}

/// The main loop swallows the touchpad's pointer moves while its contacts go to the PC: on
/// Windows SDL's global mouse, where a mouse of its own still goes straight out; elsewhere
/// the system does not say which mouse moved, so every mouse's moves stay on the main loop.
#[test]
fn the_touchpads_pointer_stays_on_the_main_loop_while_contacts_go_out() {
    let global = SDL_MouseID(0);
    assert_eq!(verdict(SDL_EVENT_MOUSE_MOTION, global, true, 0, false, true), Verdict::Pass);
    assert_eq!(verdict(SDL_EVENT_MOUSE_MOTION, global, true, 0, false, false), Verdict::Sent);
    let other = if cfg!(windows) { Verdict::Sent } else { Verdict::Pass };
    assert_eq!(verdict(SDL_EVENT_MOUSE_MOTION, SDL_MouseID(7), true, 0, false, true), other);
    assert_eq!(verdict(SDL_EVENT_MOUSE_MOTION, SDL_MouseID(7), true, 0, false, false), Verdict::Sent);
}

const KEYS: KeyRules = KeyRules { system_keys: false, right_alt_as_meta: false };
const PRESENTING: Rules = Rules { keys: Some(KEYS), clicks: Some(ClickRules { nav_buttons: true }), pads: true };

/// Outside the present the main loop reads its events itself and may change what they do, so
/// nothing leaves from the filter then.
#[test]
fn keys_clicks_and_controllers_leave_from_the_filter_only_during_the_present() {
    let a = direct_key(Some(PRESENTING), (0, 0), SDL_SCANCODE_A, SDL_KMOD_NONE, true).expect("A while presenting");
    assert_eq!(Some(a), key_input(SDL_SCANCODE_A, SDL_KMOD_NONE, false, false), "mapped as the main loop maps it");
    assert!(direct_click(Some(PRESENTING), (0, 0), true).is_some());
    assert!(direct_pad(Some(PRESENTING), (0, 0)));
    assert_eq!(direct_key(None, (0, 0), SDL_SCANCODE_A, SDL_KMOD_NONE, true), None);
    assert_eq!(direct_click(None, (0, 0), true), None);
    assert!(!direct_pad(None, (0, 0)));
    let menu = Rules { keys: None, clicks: None, pads: false };
    assert_eq!(direct_key(Some(menu), (0, 0), SDL_SCANCODE_A, SDL_KMOD_NONE, true), None, "the menu's");
    assert_eq!(direct_click(Some(menu), (0, 0), true), None);
    assert!(!direct_pad(Some(menu), (0, 0)));
}

/// Shift held through the main loop, then a click during the present: the click waits, or the
/// PC would get it before Shift. A queued key or controller event holds keys, clicks and
/// controller events back; motion only waits for clicks.
#[test]
fn anything_queued_holds_the_direct_events_after_it() {
    for held in [(1, 0), (0, 1), (2, 3)] {
        assert_eq!(direct_key(Some(PRESENTING), held, SDL_SCANCODE_A, SDL_KMOD_NONE, false), None, "{held:?}");
        assert_eq!(direct_click(Some(PRESENTING), held, true), None, "{held:?}");
        assert!(!direct_pad(Some(PRESENTING), held), "{held:?}");
    }
    assert_eq!(verdict(SDL_EVENT_MOUSE_MOTION, SDL_MouseID(7), true, 0, false, false), Verdict::Sent, "behind a key");
}

/// Nightjar's own chords and F12 (a screenshot) stay with the main loop, and their release
/// goes out as the main loop sends it; a key without a Windows code is the loop's to drop.
#[test]
fn shortcuts_and_keys_without_a_code_stay_on_the_main_loop() {
    let chord = SDL_Keymod(SDL_KMOD_CTRL.0 | SDL_KMOD_ALT.0 | SDL_KMOD_SHIFT.0);
    assert_eq!(direct_key(Some(PRESENTING), (0, 0), SDL_SCANCODE_TAB, chord, true), None, "stream controls");
    assert_eq!(direct_key(Some(PRESENTING), (0, 0), SDL_SCANCODE_F12, SDL_KMOD_NONE, true), None, "screenshot");
    assert!(direct_key(Some(PRESENTING), (0, 0), SDL_SCANCODE_TAB, chord, false).is_some(), "the chord's release");
    assert!(direct_key(Some(PRESENTING), (0, 0), SDL_SCANCODE_TAB, SDL_KMOD_NONE, true).is_some(), "Tab alone");
    assert_eq!(direct_key(Some(PRESENTING), (0, 0), SDL_SCANCODE_UNKNOWN, SDL_KMOD_NONE, true), None);
}

#[test]
fn touch_pen_and_touchpad_clicks_stay_on_the_main_loop() {
    assert_eq!(direct_click(Some(PRESENTING), (0, 0), false), None);
}

/// A sent key or click is the main loop's to keep track of, never to send again, and holds
/// nothing back; queued keys, controller events and applied actions hold until handled.
#[test]
fn marks_tell_the_main_loop_what_was_sent_and_what_still_holds() {
    let (ordered, queued) = (AtomicU32::new(0), AtomicU32::new(0));
    let mut key = event(SDL_EVENT_KEY_DOWN);
    mark(&mut key, SENT, None);
    assert!(sent(&key));
    assert_eq!(acted(&key), None);
    release(&ordered, &queued, &key);
    assert_eq!((ordered.load(Ordering::Acquire), queued.load(Ordering::Acquire)), (0, 0));
    mark(&mut key, QUEUED, Some(&queued));
    assert!(!sent(&key));
    let mut pad = event(SDL_EVENT_GAMEPAD_BUTTON_DOWN);
    for action in ACTIONS {
        mark(&mut pad, acted_mark(action), Some(&queued));
        assert_eq!(acted(&pad), Some(action));
        assert!(!sent(&pad));
        release(&ordered, &queued, &pad);
    }
    assert_eq!(queued.load(Ordering::Acquire), 1, "the queued key still holds");
    release(&ordered, &queued, &key);
    assert_eq!(queued.load(Ordering::Acquire), 0);
    for other in [0, ORDERED, QUEUED, SENT] {
        assert_eq!(action_of(other), None, "{other:#x}");
    }
}

#[test]
fn sdls_grab_steps_aside_while_the_hook_takes_the_shells_keys() {
    // SDL's hook on the main thread answers only between the present's waits.
    assert!(!sdl_grab(true, true), "the hook's thread answers Windows instead");
    assert!(sdl_grab(true, false), "no hook: SDL's grab as before");
    assert!(!sdl_grab(false, false));
}

#[test]
fn controller_input_goes_to_the_lent_controllers_and_devices_stay_on_the_main_loop() {
    for kind in [SDL_EVENT_GAMEPAD_BUTTON_DOWN, SDL_EVENT_GAMEPAD_BUTTON_UP, SDL_EVENT_GAMEPAD_AXIS_MOTION, SDL_EVENT_GAMEPAD_SENSOR_UPDATE] {
        assert!(applies_to_pads(kind), "{:#x}", kind.0);
    }
    for kind in [SDL_EVENT_GAMEPAD_ADDED, SDL_EVENT_GAMEPAD_REMOVED, SDL_EVENT_JOYSTICK_BUTTON_DOWN, SDL_EVENT_KEY_DOWN] {
        assert!(!applies_to_pads(kind), "{:#x}", kind.0);
    }
}

/// The controllers go to the filter for the present and come back unchanged; the loop draws
/// with them meanwhile, and nothing is read without a controller or once the present returns.
#[test]
fn lent_controllers_come_back_and_are_read_only_during_the_present() {
    let frozen = Arc::new(Mutex::new(Frozen::default()));
    let mut pads = Gamepads::default();
    pads.swap_face_buttons = true;
    lock(&frozen).pads = Some(Lent(pads));
    let loan = Loan(frozen.clone());
    assert!(loan.pads().swap_face_buttons, "the loop draws with its own controllers");
    assert!(!freeze(&frozen, Some(PRESENTING)), "no controller to read");
    assert_eq!(lock(&frozen).rules, Some(PRESENTING));
    assert!(!freeze(&frozen, None));
    assert_eq!(lock(&frozen).rules, None);
    assert!(loan.take_back().swap_face_buttons);
    assert!(lock(&frozen).pads.is_none());
}

#[test]
fn take_sent_down_does_not_deadlock_while_loan_pads_is_held() {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let frozen = Arc::new(Mutex::new(Frozen::default()));
        lock(&frozen).pads = Some(Lent(Gamepads::default()));
        let sent_down = Mutex::new(SentDown {
            keys: HashSet::from([0x57u16]),
            buttons: HashSet::from([1u8]),
        });
        let loan = Loan(frozen.clone());
        // Mirrors `draw_overlay(..., &mut pads.pads(), ...)`: `Frozen` is locked while
        // `toggle_menu` -> `raise_all_keys` -> `take_sent_down` runs on the same thread.
        let _pads = loan.pads();
        let (keys, buttons) = take_sent_down_from(&sent_down);
        drop(_pads);
        let _ = loan.take_back();
        let _ = tx.send((keys, buttons));
    });
    let (keys, buttons) = rx.recv_timeout(std::time::Duration::from_secs(2))
        .expect("take_sent_down deadlocked while Loan::pads held Frozen");
    assert_eq!(keys, HashSet::from([0x57u16]));
    assert_eq!(buttons, HashSet::from([1u8]));
}

#[test]
fn focus_lost_barrier_blocks_direct_input_until_dispatched() {
    for kind in [SDL_EVENT_WINDOW_FOCUS_LOST, SDL_EVENT_WINDOW_MINIMIZED, SDL_EVENT_WINDOW_HIDDEN] {
        assert!(is_barrier(kind), "{:#x}", kind.0);
    }
    assert!(!is_barrier(SDL_EVENT_WINDOW_FOCUS_GAINED));
    let (ordered, queued) = (AtomicU32::new(0), AtomicU32::new(0));
    let mut focus_lost = event(SDL_EVENT_WINDOW_FOCUS_LOST);
    mark_barrier(&mut focus_lost, &ordered, &queued);
    let held = (ordered.load(Ordering::Acquire), queued.load(Ordering::Acquire));
    assert_eq!(held, (1, 1));
    assert_eq!(verdict(SDL_EVENT_MOUSE_MOTION, SDL_MouseID(7), true, held.0, false, false), Verdict::Pass);
    assert_eq!(direct_key(Some(PRESENTING), held, SDL_SCANCODE_A, SDL_KMOD_NONE, true), None);
    assert_eq!(direct_click(Some(PRESENTING), held, true), None);
    assert!(!direct_pad(Some(PRESENTING), held));
    release(&ordered, &queued, &focus_lost);
    let held = (ordered.load(Ordering::Acquire), queued.load(Ordering::Acquire));
    assert_eq!(held, (0, 0));
    assert_eq!(verdict(SDL_EVENT_MOUSE_MOTION, SDL_MouseID(7), true, held.0, false, false), Verdict::Sent);
    assert!(direct_key(Some(PRESENTING), held, SDL_SCANCODE_A, SDL_KMOD_NONE, true).is_some());
    assert!(direct_click(Some(PRESENTING), held, true).is_some());
    assert!(direct_pad(Some(PRESENTING), held));
}

#[test]
fn controller_poller_relaxes_when_idle_and_speeds_up_on_input() {
    assert_eq!(POLL_ACTIVE_NS, 1_000_000, "1000 Hz in use, as SDL polls");
    const { assert!(POLL_IDLE_NS <= 4_000_000, "the first input after a pause waits at most 4 ms") };
    assert_eq!(poller_sleep_ns(500_000, 0), POLL_IDLE_NS, "no controller input seen yet");
    assert_eq!(poller_sleep_ns(1_000_000, 900_000), POLL_ACTIVE_NS, "recent input stays on 1000 Hz");
    assert_eq!(poller_sleep_ns(2_000_000, 1_000_001), POLL_ACTIVE_NS, "just inside the 1 s hold");
    assert_eq!(poller_sleep_ns(2_000_000, 1_000_000), POLL_IDLE_NS, "idle after 1 s relaxes to 250 Hz");
}

mod pad_jitter_tests {
    use super::super::*;

    fn ev(kind: SDL_EventType) -> SDL_Event {
        let mut e: SDL_Event = unsafe { std::mem::zeroed() };
        e.r#type = kind.0;
        e
    }

    fn axis_ev(pad: u32, axis: u8, value: i16) -> SDL_Event {
        let mut e = ev(SDL_EVENT_GAMEPAD_AXIS_MOTION);
        (e.gaxis.which, e.gaxis.axis, e.gaxis.value) = (SDL_JoystickID(pad), axis, value);
        e
    }

    fn sensor_ev(sensor: SDL_SensorType, data: [f32; 3]) -> SDL_Event {
        let mut e = ev(SDL_EVENT_GAMEPAD_SENSOR_UPDATE);
        e.gsensor.sensor = sensor.0;
        e.gsensor.data = data;
        e
    }

    #[test]
    fn sub_threshold_stick_drift_and_sensors_do_not_keep_poller_hot() {
        let mut act = PadActivity::default();
        for drift in [1, -1, 64, -128, 256, -256, 0] {
            assert!(!act.is_active(&axis_ev(1, 0, drift)));
        }
        assert!(!act.is_active(&ev(SDL_EVENT_GAMEPAD_SENSOR_UPDATE)));
        assert!(!act.is_active(&sensor_ev(SDL_SENSOR_ACCEL, [0.0, 9.81, 0.0])));
        assert!(!act.is_active(&sensor_ev(SDL_SENSOR_GYRO, [0.008, -0.012, 0.005])), "resting noise and bias");
        // 2 deg/s, split evenly over the three axes: the smallest peak such a turn can show.
        let slow = 2.0_f32.to_radians() / 3.0_f32.sqrt();
        assert!(act.is_active(&sensor_ev(SDL_SENSOR_GYRO, [slow, -slow, slow])), "slow aiming");
        assert!(!act.is_active(&sensor_ev(SDL_SENSOR_GYRO, [f32::NAN, 0.5, 0.0])));
        assert!(!act.is_active(&sensor_ev(SDL_SENSOR_GYRO, [f32::INFINITY, 0.0, 0.0])));
        assert!(act.is_active(&sensor_ev(SDL_SENSOR_GYRO, [0.0, -0.05, 0.0])));
        assert!(!act.is_active(&ev(SDL_EVENT_JOYSTICK_BATTERY_UPDATED)));
        assert!(act.is_active(&axis_ev(1, 0, 4000)));
        for drift in [3900, 4150, 3750, 4250] {
            assert!(!act.is_active(&axis_ev(1, 0, drift)));
        }
        assert!(act.is_active(&axis_ev(1, 0, 0)));
        assert!(act.is_active(&ev(SDL_EVENT_GAMEPAD_BUTTON_DOWN)));
        act.remove_pad(1);
        assert!(act.last_axis.is_empty());
    }
}
