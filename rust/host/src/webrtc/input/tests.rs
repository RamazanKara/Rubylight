use super::*;
use crate::state::test_support::Fixture;
use butterpollo_core::{rtsp::Negotiated, session::Role};
use serde_json::Value;
use std::sync::Mutex;

const PAD: u32 = 1 << 8;
const MOUSE: u32 = 1 << 11;
const KEYBOARD: u32 = 1 << 12;
const START: u32 = 1 << 7;
const A: u32 = 1;
const X: u32 = 1 << 2;

struct Rig {
    _f: Fixture,
    rec: Recorder,
    core: Core,
    sent: Arc<Mutex<Vec<Value>>>,
    t0: Instant,
}

fn session(f: &Fixture, perm: u32) -> Arc<Session> {
    Session::new(
        f.launch(f.client(perm), Role::Stream),
        Negotiated {
            width: 1920,
            height: 1080,
            ..Negotiated::default()
        },
    )
}

fn rig(perm: u32) -> Rig {
    let f = Fixture::new();
    let session = session(&f, perm);
    let rec = Recorder::default();
    let t0 = Instant::now();
    let make = rec.clone();
    let mut core = Core::new(session, Box::new(move |_| make.make()), t0);
    let sent = Arc::new(Mutex::new(Vec::new()));
    let log = sent.clone();
    core.control = Some(Box::new(move |data| {
        log.lock()
            .unwrap()
            .push(serde_json::from_slice(&data).unwrap());
        true
    }));
    Rig {
        _f: f,
        rec,
        core,
        sent,
        t0,
    }
}

impl Rig {
    fn at(&self, ms: u64) -> Instant {
        self.t0 + Duration::from_millis(ms)
    }

    fn pad(&mut self, id: u32, ms: u64) {
        let text = format!(
            r#"{{"v":2,"type":"gamepad-connected","controllerId":{id},"id":"Pad","mapping":"standard","buttons":17,"axes":4,"dualRumble":true,"triggerRumble":true}}"#
        );
        self.core.gamepad(&text, self.at(ms));
    }

    fn state(&mut self, id: u32, seq: u64, buttons: u32, stick_x: f64, ms: u64) {
        let text = format!(
            r#"{{"v":2,"type":"gamepad-state","controllerId":{id},"seq":{seq},"buttons":{buttons},"leftTrigger":0,"rightTrigger":0,"leftStickX":{stick_x},"leftStickY":0,"rightStickX":0,"rightStickY":0}}"#
        );
        self.core.gamepad(&text, self.at(ms));
    }

    fn input(&mut self, text: &str) {
        self.core.input(text, self.at(0));
    }

    /// An `input` message as the worker handles it: received, taken off the queue at once, then
    /// handled, so the wait and queue delays are measured. Returns how long that took from
    /// receipt, which bounds every delay it recorded however long the machine held this thread.
    fn deliver(&mut self, text: &str) -> Duration {
        let received = Instant::now();
        self.core.dequeued = Some(received);
        self.core.input(text, received);
        received.elapsed()
    }

    fn events(&self) -> Vec<Input> {
        self.rec.events()
    }

    fn sent(&self, kind: &str) -> Vec<Value> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m["type"] == kind)
            .cloned()
            .collect()
    }

    fn warned(&self, code: &str) -> bool {
        self.core
            .session
            .launch
            .warnings
            .snapshot()
            .iter()
            .any(|w| w.code == code)
    }

    /// Hold Start for 800 ms on pad `id`, then let go: mouse mode toggles.
    fn toggle_mouse_mode(&mut self, id: u32, seq: u64, ms: u64) {
        self.state(id, seq, START, 0., ms);
        self.state(id, seq + 1, 0, 0., ms + 800);
    }
}

fn key_down(key: u16) -> Input {
    Input::Keyboard {
        key,
        modifiers: 0,
        flags: 0,
        down: true,
    }
}

fn key_up(key: u16) -> Input {
    Input::Keyboard {
        key,
        modifiers: 0,
        flags: 0,
        down: false,
    }
}

fn button(button: u8, down: bool) -> Input {
    Input::MouseButton { button, down }
}

#[test]
fn a_pad_announced_on_both_channels_is_plugged_in_once() {
    let mut r = rig(u32::MAX);
    // The control channel and the gamepad channel each carry the same announcement.
    r.pad(3, 0);
    r.state(3, 1, A, 1., 5);
    r.pad(3, 10);
    r.state(3, 2, A, 1., 15);
    let events = r.events();
    let arrivals = events
        .iter()
        .filter(|e| matches!(e, Input::Arrival { .. }))
        .count();
    assert_eq!(arrivals, 1, "{events:?}");
    // No neutral state in between either: the pad was never unplugged.
    assert!(
        events
            .iter()
            .all(|e| !matches!(e, Input::Controller { active: 0, .. })),
        "{events:?}"
    );
    assert_eq!(r.core.pads.active_mask(), 1);
}

#[test]
fn a_gamepad_arrives_reports_and_leaves_through_the_injector() {
    let mut r = rig(u32::MAX);
    r.pad(3, 0);
    r.state(3, 1, A, 1., 10);
    r.core.gamepad(
        r#"{"v":2,"type":"gamepad-disconnected","controllerId":3}"#,
        r.at(20),
    );
    let events = r.events();
    assert!(matches!(events[0], Input::Arrival { id: 0, kind: 1, .. }));
    assert!(matches!(
        events[1],
        Input::Controller {
            id: 0,
            active: 1,
            buttons: 0x1000,
            sticks: [32767, ..],
            ..
        }
    ));
    // Neutral with the bit still set, then the bit leaves the mask.
    assert!(matches!(
        events[2],
        Input::Controller {
            active: 1,
            buttons: 0,
            ..
        }
    ));
    assert!(matches!(
        events[3],
        Input::Controller {
            active: 0,
            buttons: 0,
            ..
        }
    ));
    assert_eq!(events.len(), 4);
    assert_eq!(r.core.pads.active_mask(), 0);
}

#[test]
fn a_stale_snapshot_is_dropped_and_a_keepalive_changes_nothing() {
    let mut r = rig(u32::MAX);
    r.pad(0, 0);
    r.state(0, 5, A, 0., 1);
    assert_eq!(r.events().len(), 2);
    // An older sequence number must not undo the newer state.
    r.state(0, 4, 0, 0., 2);
    r.state(0, 5, 0, 0., 3);
    assert_eq!(r.events().len(), 2);
    assert_eq!(r.core.summary().gamepad_ignored, 2);
    assert_eq!(r.core.pads.stale_states(0), 2);
    // The 250 ms keepalive repeats the state: nothing new for the injector.
    r.state(0, 6, A, 0., 250);
    r.state(0, 7, A, 0., 500);
    assert_eq!(r.events().len(), 2);
    // A change goes straight through.
    r.state(0, 8, 0, 0., 510);
    assert_eq!(r.events().len(), 3);
}

#[test]
fn rumble_reaches_the_client_scaled_to_zero_to_one() {
    let mut r = rig(u32::MAX);
    r.pad(7, 0);
    // The driver's low motor at full, the high motor at half.
    r.rec
        .post_feedback(0, RUMBLE_DATA, &[0xff, 0xff, 0x00, 0x80]);
    assert!(r.core.poll(r.at(8)));
    let rumble = r.sent("rumble");
    assert_eq!(rumble.len(), 1);
    assert_eq!(rumble[0]["v"], 2);
    assert_eq!(rumble[0]["controllerId"], 7);
    assert_eq!(rumble[0]["controllerSlot"], 0);
    assert_eq!(rumble[0]["strongMagnitude"], 1.0);
    let weak = rumble[0]["weakMagnitude"].as_f64().unwrap();
    assert!((weak - 32768. / 65535.).abs() < 1e-9, "{weak}");
    // An Xbox report carries the trigger motors as well, as on the GameStream path.
    r.rec
        .post_feedback(0, 4, &[0, 0, 0, 0, 0xff, 0xff, 0x00, 0x00]);
    assert!(r.core.poll(r.at(20)));
    let rumble = r.sent("rumble");
    assert_eq!(rumble.len(), 3);
    assert_eq!(rumble[2]["leftTrigger"], 1.0);
    assert_eq!(rumble[2]["rightTrigger"], 0.0);
    // An idle pad sends nothing.
    assert!(r.core.poll(r.at(40)));
    assert_eq!(r.sent("rumble").len(), 3);
    assert_eq!(r.core.summary().rumble_sent, 3);
}

#[test]
fn rumble_the_host_forbids_or_a_pad_without_motors_is_not_sent() {
    let mut r = rig(u32::MAX);
    r.pad(0, 0);
    r.rec.forbid_feedback(RUMBLE_DATA);
    r.rec.post_feedback(0, RUMBLE_DATA, &[1, 0, 1, 0]);
    r.core.poll(r.at(8));
    assert!(r.sent("rumble").is_empty());
    // A pad that announced no rumble gets none; slot 5 has no pad at all.
    let mut r = rig(u32::MAX);
    r.core.gamepad(
        r#"{"v":2,"type":"gamepad-connected","controllerId":1,"id":"Pad","mapping":"","buttons":17}"#,
        r.at(0),
    );
    r.rec.post_feedback(0, RUMBLE_DATA, &[1, 0, 1, 0]);
    r.rec.post_feedback(5, RUMBLE_DATA, &[1, 0, 1, 0]);
    r.core.poll(r.at(8));
    assert!(r.sent("rumble").is_empty());
}

#[test]
fn mouse_mode_toggles_ticks_and_tells_the_client() {
    let mut r = rig(u32::MAX);
    r.pad(2, 0);
    // A plugged pad needs no pointer tick.
    assert_eq!(r.core.pads.next_tick(), None);
    r.toggle_mouse_mode(2, 1, 100);
    let modes = r.sent("mouse-mode");
    assert_eq!(modes.len(), 1);
    assert_eq!(modes[0]["v"], 2);
    assert_eq!(modes[0]["controllerId"], 2);
    assert_eq!(modes[0]["controllerSlot"], 0);
    assert_eq!(modes[0]["active"], true);
    assert!(r.core.pads.mouse_mode(2));
    // The stick drives the pointer, once per 50 ms and not before.
    r.state(2, 3, 0, 1., 910);
    let before = r.events().len();
    assert!(r.core.pads.next_tick().is_some());
    assert!(r.core.poll(r.at(920)));
    assert_eq!(r.events().len(), before, "the tick is not due yet");
    assert!(r.core.poll(r.at(960)));
    let moved: Vec<_> = r
        .events()
        .into_iter()
        .filter_map(|e| match e {
            Input::Relative { x, y } => Some((x, y)),
            _ => None,
        })
        .collect();
    assert_eq!(moved.len(), 1);
    assert!(moved[0].0 > 0 && moved[0].1 == 0);
    // Leaving the mode says so and stops the tick.
    r.toggle_mouse_mode(2, 4, 1000);
    let modes = r.sent("mouse-mode");
    assert_eq!(modes.len(), 2);
    assert_eq!(modes[1]["active"], false);
    assert_eq!(r.core.pads.next_tick(), None);
}

#[test]
fn an_idle_session_wakes_only_to_see_whether_it_is_stopping() {
    let mut r = rig(u32::MAX);
    let now = r.at(0);
    // The injector exists, so the session stopping must be noticed; no pointer, repeat or
    // rumble timer runs.
    assert_eq!(r.core.wait(now), Some(STOP_POLL));
    r.input(r#"{"v":1,"type":"key","code":"KeyA","down":true}"#);
    assert_eq!(r.core.wait(now), Some(STOP_POLL));
    r.input(r#"{"v":1,"type":"key","code":"KeyA","down":false}"#);
    assert_eq!(r.core.wait(now), Some(STOP_POLL));
    // With no injector and nothing held there is nothing to give up and nothing to wake for.
    let f = Fixture::new();
    let rec = Recorder::default();
    rec.fail_makes(1);
    let make = rec.clone();
    let core = Core::new(session(&f, u32::MAX), Box::new(move |_| make.make()), now);
    assert_eq!(core.wait(now), None);
    // A plugged pad is polled for rumble, but never ticked for the pointer.
    r.pad(0, 0);
    assert_eq!(r.core.wait(now), Some(Duration::ZERO));
    assert_eq!(r.core.pads.next_tick(), None);
}

#[test]
fn simulated_buttons_are_released_on_leaving_mouse_mode_and_on_disconnect() {
    let mut r = rig(u32::MAX);
    r.pad(1, 0);
    r.toggle_mouse_mode(1, 1, 0);
    // A is the left button; X the middle one.
    r.state(1, 3, A | X, 0., 900);
    let down = r
        .events()
        .into_iter()
        .filter(|e| matches!(e, Input::MouseButton { down: true, .. }))
        .count();
    assert_eq!(down, 2);
    assert!(!r.core.channel.held().is_empty());
    // Leaving the mode with them held lets them go.
    r.state(1, 4, A | X | START, 0., 1000);
    r.state(1, 5, A | X, 0., 1800);
    let up = r
        .events()
        .into_iter()
        .filter(|e| matches!(e, Input::MouseButton { down: false, .. }))
        .count();
    assert_eq!(up, 2);
    assert!(r.core.channel.held().is_empty());
    // The same when the pad goes away in mouse mode.
    r.state(1, 6, 0, 0., 1810);
    r.rec.clear();
    r.toggle_mouse_mode(1, 7, 2000);
    r.state(1, 9, A, 0., 2900);
    assert!(!r.core.channel.held().is_empty());
    r.core.gamepad(
        r#"{"v":2,"type":"gamepad-disconnected","controllerId":1}"#,
        r.at(3000),
    );
    assert!(r.events().contains(&button(1, false)));
    assert!(r.core.channel.held().is_empty());
    assert_eq!(r.sent("mouse-mode").last().unwrap()["active"], false);
}

#[test]
fn each_keyboard_and_mouse_message_reaches_the_injector() {
    let mut r = rig(u32::MAX);
    for text in [
        r#"{"v":1,"type":"key","code":"KeyA","down":true}"#,
        r#"{"v":1,"type":"key","code":"NumpadEnter","down":true}"#,
        r#"{"v":1,"type":"text","text":"héllo"}"#,
        r#"{"v":1,"type":"mouse-move","dx":3,"dy":-4}"#,
        r#"{"v":1,"type":"mouse-position","x":0.5,"y":0.25}"#,
        r#"{"v":1,"type":"mouse-button","button":2,"down":true}"#,
        r#"{"v":1,"type":"wheel","dx":0,"dy":120}"#,
        r#"{"v":1,"type":"wheel","dx":-120,"dy":0}"#,
    ] {
        r.input(text);
    }
    assert_eq!(
        r.events(),
        vec![
            key_down(0x41),
            Input::Keyboard {
                key: 0x0d,
                modifiers: 0x10,
                flags: 0,
                down: true
            },
            Input::Text("héllo".into()),
            Input::Relative { x: 3, y: -4 },
            Input::Absolute {
                x: 960,
                y: 270,
                width: 1920,
                height: 1080
            },
            // DOM button 2 is the right one, Moonlight's 3.
            button(3, true),
            // DOM down is positive, Moonlight's up is.
            Input::Scroll {
                amount: -120,
                horizontal: false
            },
            Input::Scroll {
                amount: -120,
                horizontal: true
            },
        ]
    );
}

#[test]
fn unknown_codes_and_bad_messages_are_dropped_and_counted() {
    let mut r = rig(u32::MAX);
    r.input(r#"{"v":1,"type":"key","code":"Fn","down":true}"#);
    r.input(r#"{"v":1,"type":"key","code":"Hyper","down":true}"#);
    r.input(r#"{"v":1,"type":"text","text":"123456789012345678901234567890123"}"#);
    r.input(r#"{"v":1,"type":"mouse-button","button":5,"down":true}"#);
    r.input("not json");
    assert!(r.events().is_empty());
    let summary = r.core.summary();
    assert_eq!(summary.unknown_codes, 2);
    assert_eq!(summary.invalid, 3);
    assert_eq!(summary.input_messages, 5);
    // 32 characters are fine.
    r.input(&format!(
        r#"{{"v":1,"type":"text","text":"{}"}}"#,
        "x".repeat(32)
    ));
    assert_eq!(r.events().len(), 1);
}

#[test]
fn the_devices_permission_bits_gate_keyboard_mouse_and_gamepad() {
    // Only the keyboard.
    let mut r = rig(KEYBOARD);
    r.input(r#"{"v":1,"type":"key","code":"KeyA","down":true}"#);
    r.input(r#"{"v":1,"type":"mouse-move","dx":1,"dy":1}"#);
    r.input(r#"{"v":1,"type":"mouse-button","button":0,"down":true}"#);
    r.input(r#"{"v":1,"type":"wheel","dx":0,"dy":1}"#);
    r.pad(0, 0);
    assert_eq!(r.events(), vec![key_down(0x41)]);
    assert_eq!(r.core.summary().dropped, 4);
    // Say so on the stream card, naming the permission to turn on.
    assert!(r.warned("input_permission_mouse"));
    assert!(r.warned("input_permission_controller"));
    assert!(!r.warned("input_permission_keyboard"));
    // A blocked button is never held, so it is never released either.
    r.core.release_held(r.at(1));
    assert_eq!(r.events(), vec![key_down(0x41), key_up(0x41)]);
    // Only the mouse and the gamepad.
    let mut r = rig(MOUSE | PAD);
    r.input(r#"{"v":1,"type":"key","code":"KeyA","down":true}"#);
    r.input(r#"{"v":1,"type":"text","text":"a"}"#);
    r.input(r#"{"v":1,"type":"mouse-move","dx":2,"dy":0}"#);
    r.pad(0, 0);
    assert_eq!(r.events().len(), 2);
    assert!(matches!(r.events()[0], Input::Relative { x: 2, y: 0 }));
    assert!(matches!(r.events()[1], Input::Arrival { .. }));
    assert!(r.warned("input_permission_keyboard"));
    // A device with nothing allowed sends nothing through.
    let mut r = rig(0);
    r.input(r#"{"v":1,"type":"mouse-move","dx":2,"dy":0}"#);
    r.pad(0, 0);
    assert!(r.events().is_empty());
    // A pad without the mouse bit may toggle mouse mode, but the pointer events and buttons it
    // would simulate are refused like any other mouse input.
    let mut r = rig(PAD);
    r.pad(0, 0);
    r.toggle_mouse_mode(0, 1, 0);
    r.state(0, 3, A, 0., 900);
    assert!(
        !r.events()
            .iter()
            .any(|e| matches!(e, Input::MouseButton { .. }))
    );
    assert!(r.core.channel.held().is_empty());
}

#[test]
fn a_closed_input_channel_releases_every_held_key_and_button() {
    let mut r = rig(u32::MAX);
    r.input(r#"{"v":1,"type":"key","code":"ShiftLeft","down":true}"#);
    r.input(r#"{"v":1,"type":"key","code":"KeyW","down":true}"#);
    r.input(r#"{"v":1,"type":"key","code":"KeyA","down":true}"#);
    r.input(r#"{"v":1,"type":"key","code":"KeyA","down":false}"#);
    r.input(r#"{"v":1,"type":"mouse-button","button":0,"down":true}"#);
    r.rec.clear();
    r.core.release_held(r.at(1));
    // Buttons first, then keys; the key that was let go is not released twice.
    assert_eq!(
        r.events(),
        vec![button(1, false), key_up(0x57), key_up(0xa0)]
    );
    // Nothing is left to release.
    r.rec.clear();
    r.core.release_held(r.at(2));
    assert!(r.events().is_empty());
}

#[test]
fn the_end_of_the_stream_releases_keys_buttons_and_pads_and_drops_the_injector() {
    let mut r = rig(u32::MAX);
    r.pad(0, 0);
    r.toggle_mouse_mode(0, 1, 0);
    r.state(0, 3, A, 0., 900);
    r.input(r#"{"v":1,"type":"key","code":"KeyW","down":true}"#);
    assert_eq!(r.rec.sinks(), (1, 1));
    r.rec.clear();
    r.core.finish(r.at(1000));
    let events = r.events();
    assert!(events.contains(&button(1, false)));
    assert!(events.contains(&key_up(0x57)));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Input::Controller { active: 0, .. }))
    );
    assert_eq!(r.rec.sinks(), (1, 0), "the injector is dropped");
    assert_eq!(r.core.pads.active_mask(), 0);
    assert!(r.core.channel.held().is_empty());
}

#[test]
fn a_stopping_session_ends_the_worker_without_waiting_for_the_transport() {
    let mut r = rig(u32::MAX);
    assert!(r.core.poll(r.at(1)));
    r.core.session.stop();
    assert!(!r.core.poll(r.at(2)));

    // In the real loop: nothing is held and the transport never says it closed, yet the
    // injector goes and the worker ends.
    let f = Fixture::new();
    let rec = Recorder::default();
    let make = rec.clone();
    let (handle, rx) = channel();
    let s = session(&f, u32::MAX);
    let worker = std::thread::spawn(move || run(rx, Box::new(move |_| make.make())));
    handle.bind(s.clone());
    wait_for("the injector made", || rec.sinks() == (1, 1));
    s.stop();
    wait_for("the injector dropped", || rec.sinks() == (1, 0));
    worker.join().unwrap();
    // The handle outlived the worker: only the stopping session ended it.
    drop(handle);
}

/// Poll `condition` for up to thirty seconds: long enough for a machine under a parallel build,
/// and it returns as soon as the condition holds.
fn wait_for(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out: {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_device_without_the_controller_bit_gets_no_slot_no_mouse_mode_and_no_timers() {
    let mut r = rig(MOUSE | KEYBOARD);
    let idle = r.core.wait(r.at(0));
    r.pad(0, 0);
    r.toggle_mouse_mode(0, 1, 100);
    r.state(0, 3, A, 1., 1000);
    assert!(r.events().is_empty());
    assert!(r.sent("mouse-mode").is_empty());
    assert_eq!(r.core.pads.active_mask(), 0);
    assert!(!r.core.pads.mouse_mode(0));
    assert_eq!(r.core.pads.next_tick(), None);
    assert_eq!(r.core.wait(r.at(1000)), idle);
    assert!(r.warned("input_permission_controller"));
    let summary = r.core.summary();
    assert_eq!(summary.gamepad_messages, 4);
    assert_eq!(summary.dropped, 4);
    // The keyboard and mouse it may use still work.
    r.input(r#"{"v":1,"type":"mouse-move","dx":2,"dy":0}"#);
    assert_eq!(r.events().len(), 1);
}

#[test]
fn the_queue_delay_is_measured_from_receipt_to_the_injector() {
    let mut r = rig(u32::MAX);
    let received = Instant::now() - Duration::from_millis(40);
    r.core.input(
        r#"{"v":1,"type":"key","code":"KeyA","down":true}"#,
        received,
    );
    // However long the machine held this thread, no delay exceeds this.
    let bound_us = received.elapsed().as_micros() as u64;
    r.core.input(
        r#"{"v":1,"type":"key","code":"KeyA","down":false}"#,
        Instant::now(),
    );
    let summary = r.core.summary();
    assert_eq!(summary.queued, 2);
    assert!(
        (40_000..=bound_us).contains(&summary.queue_max_us),
        "{summary:?}"
    );
    // Two samples: the 99th percentile is the slower one, to within a bucket and never above it.
    assert!(
        (40_000..=summary.queue_max_us).contains(&summary.queue_p99_us),
        "{summary:?}"
    );
    // A message that injected nothing is not a sample.
    r.core
        .input(r#"{"v":1,"type":"key","code":"Fn","down":true}"#, received);
    assert_eq!(r.core.summary().queued, 2);
}

#[test]
fn a_slow_injector_call_is_not_the_hosts_queue_delay() {
    // SendInput can take milliseconds for a click (Windows' own handling, not the host's). That is
    // reported as the injector's time, not as the queue's.
    let mut r = rig(u32::MAX);
    r.rec.slow_apply(Duration::from_millis(15));
    let mut took = Duration::ZERO;
    for i in 0..4 {
        took = took.max(r.deliver(&format!(
            r#"{{"v":1,"type":"mouse-button","button":0,"down":{}}}"#,
            i % 2 == 0
        )));
    }
    let summary = r.core.summary();
    assert_eq!(summary.queued, 4);
    assert!(summary.apply_max_us >= 15_000, "{summary:?}");
    // Each delivery took its queue delay plus a call of at least 15 ms: a queue delay that
    // counted the call would leave no room for it, however long the machine held this thread.
    let took_us = took.as_micros() as u64;
    assert!(
        summary.queue_max_us + 15_000 <= took_us,
        "{summary:?} {took_us}"
    );
    // The message was taken off the queue as it arrived.
    assert_eq!(summary.wait_max_us, 0, "{summary:?}");
    assert_eq!(summary.slow_calls, 4);
}

#[test]
fn slow_calls_are_all_counted_but_only_the_first_few_logged() {
    let mut r = rig(u32::MAX);
    r.rec.slow_apply(Duration::from_millis(3));
    for i in 0..20 {
        r.deliver(&format!(
            r#"{{"v":1,"type":"mouse-button","button":0,"down":{}}}"#,
            i % 2 == 0
        ));
    }
    assert_eq!(r.core.summary().slow_calls, 20);
    assert_eq!(r.core.slow_logged, SLOW_LOGGED);
}

#[test]
fn the_kinds_in_a_slow_call_are_listed_once_each() {
    let events = [key_down(1), key_up(1), key_down(2)];
    assert_eq!(event_kinds(&events), ["Keyboard"]);
    assert!(event_kinds(&[]).is_empty());
}

#[test]
fn a_burst_through_the_worker_reaches_the_injector_without_a_timer_in_the_way() {
    let f = Fixture::new();
    let rec = Recorder::default();
    let make = rec.clone();
    let (handle, rx) = channel();
    let worker = std::thread::spawn(move || run(rx, Box::new(move |_| make.make())));
    handle.bind(session(&f, u32::MAX));
    // The worker is up and has its injector before the messages, and idle.
    handle.input(
        br#"{"v":1,"type":"mouse-move","dx":1,"dy":0}"#.to_vec(),
        false,
    );
    wait_for("the first move", || rec.events().len() == 1);
    // A thread that only waits on a channel, woken beside the worker each time: how long this
    // machine takes to wake a thread right now, which a parallel build stretches to milliseconds.
    let (ping, pings) = mpsc::channel::<Instant>();
    let (woke, wakes) = mpsc::channel::<Duration>();
    let control = std::thread::spawn(move || {
        for sent in pings {
            let _ = woke.send(sent.elapsed());
        }
    });
    let mut baseline = Delays::default();
    std::thread::sleep(Duration::from_millis(50));
    // Single messages to an idle worker, spread so they meet every phase of a timer tick.
    for i in 0..20 {
        ping.send(Instant::now()).unwrap();
        baseline.record(wakes.recv().unwrap());
        handle.input(
            format!(r#"{{"v":1,"type":"mouse-move","dx":{},"dy":1}}"#, i % 5 + 1).into_bytes(),
            false,
        );
        wait_for("the message", || rec.events().len() == 2 + i);
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(ping);
    control.join().unwrap();
    handle.closed();
    let summary = worker.join().unwrap().expect("the worker ran");
    assert_eq!(summary.queued, 21);
    // The median, not a tail, and over the machine's own wake-up time: a loaded machine can hold
    // a thread for milliseconds now and then, but a worker that wakes on a timer tick instead of
    // on the channel holds most messages for 8 to 25 ms more than a thread blocked on a channel.
    let limit = baseline.median_us() + 2_000;
    assert!(
        summary.queue_p50_us < limit && summary.wait_p50_us < limit,
        "{summary:?} baseline_p50_us={}",
        baseline.median_us()
    );
}

#[test]
fn the_delay_percentile_follows_the_bulk_not_the_outlier() {
    let mut d = Delays::default();
    assert_eq!(d.p99_us(), 0);
    for _ in 0..990 {
        d.record(Duration::from_micros(100));
    }
    for _ in 0..10 {
        d.record(Duration::from_millis(20));
    }
    assert_eq!(d.max_us, 20_000);
    // 99 % of 1000 samples are the fast ones.
    assert!((100..=128).contains(&d.p99_us()), "{}", d.p99_us());
    d.record(Duration::from_millis(20));
    assert!(d.p99_us() > 1000, "{}", d.p99_us());
    // A delay beyond the last bucket lands in it rather than overflowing.
    d.record(Duration::MAX);
    assert_eq!(d.max_us, u64::MAX);
}

#[test]
fn a_worker_that_panics_still_releases_held_keys_and_buttons() {
    let f = Fixture::new();
    let rec = Recorder::default();
    let make = rec.clone();
    let (handle, rx) = channel();
    let s = session(&f, u32::MAX);
    let worker = std::thread::spawn(move || run(rx, Box::new(move |_| make.make())));
    handle.bind(s.clone());
    handle.input(
        br#"{"v":1,"type":"key","code":"KeyA","down":true}"#.to_vec(),
        false,
    );
    handle.input(
        br#"{"v":1,"type":"mouse-button","button":0,"down":true}"#.to_vec(),
        false,
    );
    wait_for("both held", || rec.events().len() == 2);
    // The injector fails on the next event, with the key and the button still down.
    rec.panic_once();
    handle.input(
        br#"{"v":1,"type":"key","code":"KeyB","down":true}"#.to_vec(),
        false,
    );
    // The thread survives its panic, releases, and ends; the handle is still open.
    worker.join().unwrap();
    let events = rec.events();
    assert!(events.contains(&button(1, false)), "{events:?}");
    assert!(events.contains(&key_up(0x41)), "{events:?}");
    assert!(events.contains(&key_up(0x42)), "{events:?}");
    assert_eq!(rec.sinks(), (1, 0));
    assert!(
        s.launch
            .warnings
            .snapshot()
            .iter()
            .any(|w| w.code == "input_worker")
    );
    drop(handle);
}

#[test]
fn an_injector_that_cannot_be_made_is_loud_and_tried_again_later() {
    let f = Fixture::new();
    let rec = Recorder::default();
    rec.fail_makes(2);
    let make = rec.clone();
    let t0 = Instant::now();
    let mut core = Core::new(session(&f, u32::MAX), Box::new(move |_| make.make()), t0);
    let ms = |n: u64| t0 + Duration::from_millis(n);
    // Made at the start and failed, so the first key finds no injector.
    core.input(r#"{"v":1,"type":"key","code":"KeyA","down":true}"#, ms(100));
    assert!(rec.events().is_empty());
    assert_eq!(core.summary().dropped, 1);
    // Not again before the retry delay; then one more failure, then it works.
    core.input(r#"{"v":1,"type":"key","code":"KeyB","down":true}"#, ms(600));
    assert!(rec.events().is_empty());
    core.input(
        r#"{"v":1,"type":"key","code":"KeyC","down":true}"#,
        ms(1200),
    );
    assert_eq!(rec.events(), vec![key_down(0x43)]);
    assert_eq!(rec.sinks(), (1, 1));
}

#[test]
fn the_worker_thread_buffers_early_frames_and_releases_when_the_transport_closes() {
    let f = Fixture::new();
    let rec = Recorder::default();
    let make = rec.clone();
    let (handle, rx) = channel();
    let worker = std::thread::spawn(move || run(rx, Box::new(move |_| make.make())));
    // A frame that arrives before the session is bound waits for it.
    handle.input(
        br#"{"v":1,"type":"key","code":"KeyA","down":true}"#.to_vec(),
        false,
    );
    handle.bind(session(&f, u32::MAX));
    handle.input(
        br#"{"v":1,"type":"key","code":"KeyB","down":true}"#.to_vec(),
        false,
    );
    // A binary frame is not part of the protocol.
    handle.input(vec![1, 2, 3], true);
    handle.closed();
    worker.join().unwrap();
    assert_eq!(
        rec.events(),
        vec![key_down(0x41), key_down(0x42), key_up(0x41), key_up(0x42)]
    );
    assert_eq!(rec.sinks(), (1, 0));
}

#[test]
fn the_input_channel_closing_releases_and_dropping_every_handle_ends_the_worker() {
    let f = Fixture::new();
    let rec = Recorder::default();
    let make = rec.clone();
    let (handle, rx) = channel();
    let worker = std::thread::spawn(move || run(rx, Box::new(move |_| make.make())));
    handle.bind(session(&f, u32::MAX));
    handle.input(
        br#"{"v":1,"type":"mouse-button","button":0,"down":true}"#.to_vec(),
        false,
    );
    handle.channel_closed("input");
    drop(handle);
    worker.join().unwrap();
    assert_eq!(rec.events(), vec![button(1, true), button(1, false)]);
}

#[test]
fn the_histogram_buckets_are_exact_below_eight_and_a_quarter_octave_above() {
    for (us, bucket) in [(0, 0), (7, 7), (8, 8), (9, 8), (10, 9), (15, 11), (16, 12)] {
        assert_eq!(Delays::bucket(us), bucket, "{us} us");
    }
    assert_eq!(Delays::bucket(u64::MAX), BUCKETS - 1);
    assert_eq!(BUCKETS - 1, 155);
    // Every value is within its bucket's upper edge, which is under 25 % above it.
    let mut x = 0u64;
    while x < 5_000_000 {
        let upper = Delays::upper(Delays::bucket(x));
        assert!(x <= upper, "{x} us above its bucket's edge {upper}");
        assert!(
            (upper as f64) < 1.25 * x as f64 + 1.0,
            "{x} us, edge {upper}"
        );
        x += 1 + x / 97;
    }
}

/// A co-op player's WebRTC input drives its slot's pad and nothing else: its
/// first pad becomes the slot's, a keyboard is dropped and said once, and the
/// slot's rumble reaches the device's pad. A launch whose session the host
/// no longer has gets no sink, never one that reaches the desktop.
#[test]
fn a_co_op_players_input_drives_only_its_slots_pad() {
    use crate::nucleus::{Admission, Game, fake};
    use butterpollo_core::{config::Config, state::App};
    use butterpollo_windows::input::PadReport;
    let f = Fixture::new();
    let h = f.host.clone();
    let pc = Arc::new(fake::Pc::installed());
    h.nucleus.use_pc(pc.clone());
    let apps = [App {
        name: "Portal 2".into(),
        cmd: format!("\"{}\"", fake::EXE),
        working_dir: String::new(),
        prep: Vec::new(),
        computed_id: None,
        aliases: Vec::new(),
        extra: Default::default(),
    }];
    let entry = h.nucleus.entries(&apps, "").remove(0);
    let mut launch = f.launch(f.client(u32::MAX), Role::Stream);
    let seat = h
        .nucleus
        .admit(Admission {
            launch: &launch.id,
            client: &launch.client,
            game: Game::Coop(&entry),
            size: (1920, 1080),
            config: &Config::default(),
            warnings: Default::default(),
            release_parked: &|| Ok(()),
            instances: None,
        })
        .unwrap();
    launch.nucleus = Some(seat);
    let session = Session::new(launch, Negotiated::default());
    let t0 = Instant::now();
    let make = h.clone();
    let mut core = Core::new(
        session.clone(),
        Box::new(move |s| CoopSink::create(&make, s)),
        t0,
    );
    let sent = Arc::new(Mutex::new(Vec::<Value>::new()));
    let log = sent.clone();
    core.control = Some(Box::new(move |data| {
        log.lock()
            .unwrap()
            .push(serde_json::from_slice(&data).unwrap());
        true
    }));
    let at = |ms| t0 + Duration::from_millis(ms);
    core.gamepad(
        r#"{"v":2,"type":"gamepad-connected","controllerId":7,"id":"Pad","mapping":"standard","buttons":17,"axes":4,"dualRumble":true,"triggerRumble":true}"#,
        at(0),
    );
    core.gamepad(
        &format!(
            r#"{{"v":2,"type":"gamepad-state","controllerId":7,"seq":1,"buttons":{A},"leftTrigger":0,"rightTrigger":0,"leftStickX":0,"leftStickY":0,"rightStickX":0,"rightStickY":0}}"#
        ),
        at(5),
    );
    for _ in 0..2 {
        core.input(r#"{"v":1,"type":"key","code":"KeyA","down":true}"#, at(6));
    }
    let instance = pc.last();
    let inputs = instance.inputs.lock().unwrap().clone();
    assert_eq!(inputs.len(), 1, "{inputs:?}");
    assert!(
        matches!(inputs[0], (0, Input::Controller { id: 0, active: 0b11, buttons, .. }) if buttons != 0),
        "{inputs:?}"
    );
    let dropped: Vec<_> = session
        .launch
        .warnings
        .snapshot()
        .into_iter()
        .filter(|w| w.code.starts_with("coop_"))
        .map(|w| w.code)
        .collect();
    assert_eq!(dropped, ["coop_keyboard_mouse"]);
    // Slot 0's rumble, and slot 1's, which is not this player's.
    let rumble = |id| PadReport::Feedback {
        id,
        kind: RUMBLE_DATA,
        data: vec![0xff, 0xff, 0x00, 0x80],
    };
    instance
        .reports
        .lock()
        .unwrap()
        .extend([(1, rumble(0)), (0, rumble(0))]);
    assert!(core.poll(at(20)));
    let rumble: Vec<_> = sent
        .lock()
        .unwrap()
        .iter()
        .filter(|m| m["type"] == "rumble")
        .cloned()
        .collect();
    assert_eq!(rumble.len(), 1, "{rumble:?}");
    assert_eq!(rumble[0]["controllerId"], 7);
    assert_eq!(rumble[0]["strongMagnitude"], 1.0);
    // A session the registry no longer has: no sink at all.
    let mut gone = f.launch(f.client(u32::MAX), Role::Stream);
    gone.nucleus = Some(butterpollo_core::session::Seat {
        session: seat.session + 1,
        slot: 0,
    });
    let error = CoopSink::create(&h, &Session::new(gone, Negotiated::default()))
        .err()
        .unwrap();
    assert!(
        format!("{error:#}").contains("the co-op session ended"),
        "{error:#}"
    );
}

/// A guest's pad announcement and snapshots, as its page sends them.
fn guest_pad(r: &mut Rig, guest: u64, id: u32, ms: u64) {
    let text = format!(
        r#"{{"v":2,"type":"gamepad-connected","controllerId":{id},"id":"Pad","mapping":"standard","buttons":17,"axes":4,"dualRumble":true,"triggerRumble":true}}"#
    );
    r.core.guest_gamepad(guest, &text, r.at(ms));
}

fn guest_state(r: &mut Rig, guest: u64, id: u32, seq: u64, buttons: u32, ms: u64) {
    let text = format!(
        r#"{{"v":2,"type":"gamepad-state","controllerId":{id},"seq":{seq},"buttons":{buttons},"leftTrigger":0,"rightTrigger":0,"leftStickX":0,"leftStickY":0,"rightStickX":0,"rightStickY":0}}"#
    );
    r.core.guest_gamepad(guest, &text, r.at(ms));
}

/// What a guest's control channel got.
fn guest_control(r: &mut Rig, guest: u64) -> Arc<Mutex<Vec<Value>>> {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let log = sent.clone();
    r.core.guests.get_mut(&guest).unwrap().control = Some(Box::new(move |data| {
        log.lock()
            .unwrap()
            .push(serde_json::from_slice(&data).unwrap());
        true
    }));
    sent
}

#[test]
fn a_view_only_guests_input_is_dropped_by_the_host() {
    let mut r = rig(u32::MAX);
    r.core.guest_joined(1, Permission::View);
    guest_pad(&mut r, 1, 0, 0);
    guest_state(&mut r, 1, 0, 1, A, 10);
    r.core.guest_input(
        1,
        r#"{"v":1,"type":"key","code":"KeyA","down":true}"#,
        r.at(20),
    );
    r.core
        .guest_input(1, r#"{"v":1,"type":"mouse-move","dx":3,"dy":0}"#, r.at(30));
    assert!(r.events().is_empty(), "{:?}", r.events());
    assert_eq!(r.core.pads.active_mask(), 0, "no slot either");
    assert_eq!(r.core.summary().dropped, 4);
    // Nor does a guest the worker never heard of get anything through.
    guest_pad(&mut r, 9, 0, 40);
    assert!(r.events().is_empty());
}

#[test]
fn a_controller_guest_gets_its_own_pad_but_no_keyboard_mouse_or_mouse_mode() {
    let mut r = rig(u32::MAX);
    r.pad(0, 0);
    r.core.guest_joined(1, Permission::Controller);
    let sent = guest_control(&mut r, 1);
    // The guest's browser numbers its pad 0 too: it is a second pad, in the next slot.
    guest_pad(&mut r, 1, 0, 0);
    guest_state(&mut r, 1, 0, 1, A, 10);
    assert_eq!(r.core.pads.active_mask(), 0b11);
    let events = r.events();
    assert!(
        matches!(events[1], Input::Arrival { id: 1, .. }),
        "{events:?}"
    );
    assert!(matches!(
        events[2],
        Input::Controller {
            id: 1,
            active: 0b11,
            buttons: 0x1000,
            ..
        }
    ));
    r.rec.clear();
    r.core.guest_input(
        1,
        r#"{"v":1,"type":"key","code":"KeyA","down":true}"#,
        r.at(20),
    );
    // Holding Start would make the pad a mouse; for this guest it stays a pad.
    guest_state(&mut r, 1, 0, 2, START, 30);
    guest_state(&mut r, 1, 0, 3, 0, 900);
    guest_state(&mut r, 1, 0, 4, A, 950);
    assert!(!r.core.pads.mouse_mode(GUEST_IDS | 1 << GUEST_ID_BITS));
    assert!(
        r.events()
            .iter()
            .all(|e| matches!(e, Input::Controller { id: 1, .. })),
        "{:?}",
        r.events()
    );
    assert!(sent.lock().unwrap().is_empty());
    assert!(r.sent("mouse-mode").is_empty());
}

#[test]
fn a_guests_rumble_and_mouse_mode_go_to_the_guest_under_its_own_id() {
    let mut r = rig(u32::MAX);
    r.core.guest_joined(4, Permission::ControllerKeyboardMouse);
    let sent = guest_control(&mut r, 4);
    guest_pad(&mut r, 4, 2, 0);
    r.rec
        .post_feedback(0, RUMBLE_DATA, &[0xff, 0xff, 0x00, 0x00]);
    assert!(r.core.poll(r.at(8)));
    guest_state(&mut r, 4, 2, 1, START, 10);
    guest_state(&mut r, 4, 2, 2, 0, 800);
    let sent = sent.lock().unwrap().clone();
    let kinds: Vec<_> = sent.iter().map(|m| m["type"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["rumble", "mouse-mode"]);
    assert!(sent.iter().all(|m| m["controllerId"] == 2));
    assert!(r.sent("rumble").is_empty(), "nothing reaches the owner");
    assert!(r.sent("mouse-mode").is_empty());
}

#[test]
fn a_keyboard_guest_types_and_what_it_holds_is_let_go_when_it_leaves() {
    let mut r = rig(u32::MAX);
    r.core.guest_joined(2, Permission::ControllerKeyboardMouse);
    r.input(r#"{"v":1,"type":"key","code":"KeyW","down":true}"#);
    for text in [
        r#"{"v":1,"type":"key","code":"KeyA","down":true}"#,
        r#"{"v":1,"type":"mouse-button","button":0,"down":true}"#,
        r#"{"v":1,"type":"text","text":"hi"}"#,
    ] {
        r.core.guest_input(2, text, r.at(1));
    }
    guest_pad(&mut r, 2, 0, 2);
    assert_eq!(
        r.events()[..4],
        [
            key_down(0x57),
            key_down(0x41),
            button(1, true),
            Input::Text("hi".into())
        ]
    );
    r.rec.clear();
    r.core.guest_left(2, r.at(3));
    let events = r.events();
    assert!(events.contains(&key_up(0x41)) && events.contains(&button(1, false)));
    assert!(
        !events.contains(&key_up(0x57)),
        "the owner's key stays down"
    );
    assert_eq!(r.core.pads.active_mask(), 0, "its pad is unplugged");
    assert!(r.core.guests.is_empty());
}

#[test]
fn the_owners_gamepad_channel_closing_leaves_the_guests_pads_plugged() {
    let mut r = rig(u32::MAX);
    r.pad(0, 0);
    r.core.guest_joined(1, Permission::Controller);
    r.core.guest_joined(2, Permission::Controller);
    guest_pad(&mut r, 1, 0, 0);
    guest_pad(&mut r, 2, 0, 0);
    assert_eq!(r.core.pads.active_mask(), 0b111);
    r.core.unplug(r.at(1));
    assert_eq!(r.core.pads.active_mask(), 0b110);
    r.core.guest_unplug(1, r.at(2));
    assert_eq!(r.core.pads.active_mask(), 0b100, "one guest's pads only");
    // The end of the stream unplugs everyone's.
    r.core.finish(r.at(3));
    assert_eq!(r.core.pads.active_mask(), 0);
}

#[test]
fn guests_share_the_owners_sixteen_slots() {
    let mut r = rig(u32::MAX);
    r.core.guest_joined(1, Permission::Controller);
    r.core.guest_joined(2, Permission::Controller);
    for id in 0..10 {
        guest_pad(&mut r, 1, id, 0);
    }
    for id in 0..10 {
        guest_pad(&mut r, 2, id, 0);
    }
    assert_eq!(r.core.pads.active_mask(), 0xff);
    assert_eq!(
        r.core.summary().gamepad_ignored,
        12,
        "each guest's pads past its fourth get no slot"
    );
    // An id beyond the guest's range is refused, not folded onto another pad.
    guest_pad(&mut r, 1, 1 << 16, 0);
    assert_eq!(r.core.summary().invalid, 1);
}

#[test]
fn guest_controller_ids_round_trip_and_never_look_like_the_owners() {
    for (tag, id) in [(1, 0), (2, 0), (MAX_GUEST_TAG, 0xffff)] {
        let shared = guest_controller(tag, id).unwrap();
        assert_eq!(guest_of(shared), Some((tag, id)));
    }
    assert_eq!(guest_of(3), None);
    assert_eq!(guest_controller(1, 1 << 16), None);
}

#[test]
fn a_guest_plugs_in_four_pads_at_most_and_the_owner_keeps_four_slots() {
    let mut r = rig(u32::MAX);
    for guest in 1..=4 {
        r.core.guest_joined(guest, Permission::Controller);
    }
    // One guest's browser numbering pads up to 65535 gets four of them.
    for id in [0, 1, 2, 3, 4, 900, 65_535] {
        guest_pad(&mut r, 1, id, 0);
    }
    assert_eq!(r.core.pads.controller_ids().len(), 4);
    // Announcing a pad it has already plugged in again changes nothing.
    guest_pad(&mut r, 1, 0, 5);
    assert_eq!(r.core.pads.controller_ids().len(), 4);
    for guest in 2..=4 {
        for id in 0..4 {
            guest_pad(&mut r, guest, id, 10);
        }
    }
    // The guests together stop at twelve: the fourth guest got none.
    assert_eq!(r.core.pads.controller_ids().len(), 12);
    assert!(
        r.core
            .pads
            .controller_ids()
            .into_iter()
            .all(|c| guest_of(c).is_none_or(|(tag, _)| tag != r.core.guests[&4].tag))
    );
    // The owner's four still plug in, into every slot left.
    for id in 0..4 {
        r.pad(id, 20);
    }
    assert_eq!(r.core.pads.active_mask(), u16::MAX);
    // A guest that left frees its share for the next.
    r.core.guest_left(1, r.at(30));
    guest_pad(&mut r, 4, 0, 40);
    assert_eq!(r.core.pads.controller_ids().len(), 13);
}

#[test]
fn a_guest_handle_queues_only_what_its_permission_allows_and_a_bounded_amount() {
    let (handle, queues) = channel();
    assert!(
        handle.guest(1, Permission::View).is_none(),
        "view only sends nothing"
    );
    let guest = handle.guest(2, Permission::Controller).unwrap();
    let now = Instant::now();
    for _ in 0..GUEST_QUEUE * 4 {
        guest.gamepad(b"{}".to_vec(), false, now);
    }
    // A controller guest's keys and mice never reach the queue.
    guest.input(
        br#"{"v":1,"type":"key","code":"KeyA","down":true}"#.to_vec(),
        false,
    );
    {
        let inbox = queues.guests.state.lock().unwrap();
        assert_eq!(
            inbox.queue.len(),
            1 + GUEST_QUEUE,
            "its join, then the bound"
        );
        assert_eq!(inbox.waiting[&2].data, GUEST_QUEUE);
        assert!(
            inbox.queue.iter().all(|(m, _)| matches!(
                m,
                GuestMessage::Joined(2, _) | GuestMessage::Gamepad(2, ..)
            ))
        );
    }
    // However much it sent, the owner's queue got one wake.
    handle.gamepad(b"{}".to_vec(), false, now);
    let owner: Vec<Message> = queues.rx.try_iter().collect();
    assert_eq!(owner.len(), 2);
    assert!(matches!(owner[0], Message::Guests));
    assert!(matches!(owner[1], Message::Gamepad(..)));
    // Leaving always queues, even at the bound, and taking the queue frees the guest's share.
    drop(guest);
    let mut taken = 0;
    while let Some(message) = queues.guests.pop() {
        taken += 1;
        if taken == 2 + GUEST_QUEUE {
            assert!(matches!(message, GuestMessage::Left(2)));
        }
    }
    assert_eq!(taken, 2 + GUEST_QUEUE);
    assert!(queues.guests.state.lock().unwrap().waiting.is_empty());
    // A wake already queued covers what comes after it; once the worker took it, the next
    // message wakes it again.
    assert!(queues.rx.try_recv().is_err());
    queues.guests.woken.store(false, Ordering::Release);
    let again = handle
        .guest(3, Permission::ControllerKeyboardMouse)
        .unwrap();
    again.input(
        br#"{"v":1,"type":"key","code":"KeyA","down":true}"#.to_vec(),
        false,
    );
    let owner: Vec<Message> = queues.rx.try_iter().collect();
    assert_eq!(owner.len(), 1);
    assert!(matches!(owner[0], Message::Guests));
}

#[test]
fn the_worker_takes_the_owners_input_before_a_guests() {
    let f = Fixture::new();
    let rec = Recorder::default();
    let make = rec.clone();
    let (handle, queues) = channel();
    let s = session(&f, u32::MAX);
    let worker = std::thread::spawn(move || run(queues, Box::new(move |_| make.make())));
    // The guest's pad is announced first; the owner's after it, before the worker runs.
    let guest = handle.guest(1, Permission::Controller).unwrap();
    // The owner's pad has no motors, so its arrival tells the two apart.
    let announce = |id: u32, rumble: bool| {
        format!(
            r#"{{"v":2,"type":"gamepad-connected","controllerId":{id},"id":"Pad","mapping":"standard","buttons":17,"axes":4,"dualRumble":{rumble},"triggerRumble":false}}"#
        )
        .into_bytes()
    };
    guest.gamepad(announce(0, true), false, Instant::now());
    handle.gamepad(announce(5, false), false, Instant::now());
    handle.bind(s.clone());
    wait_for("both pads arrived", || {
        rec.events()
            .iter()
            .filter(|e| matches!(e, Input::Arrival { .. }))
            .count()
            == 2
    });
    // The owner's pad arrived first, in the first slot.
    let arrivals: Vec<(u8, u16)> = rec
        .events()
        .iter()
        .filter_map(|e| match e {
            Input::Arrival {
                id, capabilities, ..
            } => Some((*id, *capabilities)),
            _ => None,
        })
        .collect();
    assert_eq!((arrivals[0].0, arrivals[1].0), (0, 1));
    assert!(arrivals[0].1 < arrivals[1].1, "{arrivals:?}");
    drop(guest);
    s.stop();
    worker.join().unwrap();
    drop(handle);
}

/// Feed what the guests' queue holds to the rig's worker, in order.
fn drain(r: &mut Rig, queues: &Queues) {
    while let Some(message) = queues.guests.pop() {
        let now = r.at(1);
        r.core.guest(message, now);
    }
}

fn keyboard(events: &[Input]) -> Vec<bool> {
    events
        .iter()
        .filter_map(|e| match e {
            Input::Keyboard { down, .. } => Some(*down),
            _ => None,
        })
        .collect()
}

const SHIFT_DOWN: &[u8] = br#"{"v":1,"type":"key","code":"ShiftLeft","down":true}"#;
const SHIFT_UP: &[u8] = br#"{"v":1,"type":"key","code":"ShiftLeft","down":false}"#;

fn pad_state(id: u32, seq: u64, buttons: u32) -> Vec<u8> {
    format!(
        r#"{{"v":2,"type":"gamepad-state","controllerId":{id},"seq":{seq},"buttons":{buttons},"leftTrigger":0,"rightTrigger":0,"leftStickX":0,"leftStickY":0,"rightStickX":0,"rightStickY":0}}"#
    )
    .into_bytes()
}

fn pad_connected(id: u32) -> Vec<u8> {
    format!(
        r#"{{"v":2,"type":"gamepad-connected","controllerId":{id},"id":"Pad","mapping":"standard","buttons":17,"axes":4,"dualRumble":true,"triggerRumble":true}}"#
    )
    .into_bytes()
}

#[test]
fn a_guests_key_release_queues_past_the_bound_while_motion_gives_way() {
    let mut r = rig(u32::MAX);
    let (handle, queues) = channel();
    let guest = handle
        .guest(1, Permission::ControllerKeyboardMouse)
        .unwrap();
    let now = Instant::now();
    guest.input(SHIFT_DOWN.to_vec(), false);
    // The worker stalls while the guest moves the mouse: past the bound the motion is dropped.
    for _ in 0..GUEST_QUEUE * 2 {
        guest.input(
            br#"{"v":1,"type":"mouse-move","dx":1,"dy":0}"#.to_vec(),
            false,
        );
    }
    // A pad plugs in and changes state three times: only its latest state waits.
    guest.gamepad(pad_connected(0), false, now);
    guest.gamepad(pad_state(0, 1, A), false, now);
    guest.gamepad(pad_state(0, 2, 0), false, now);
    guest.gamepad(pad_state(0, 3, A), false, now);
    // The guest lets go of Shift while the queue is full.
    guest.input(SHIFT_UP.to_vec(), false);
    {
        let inbox = queues.guests.state.lock().unwrap();
        // Its join, the bound (Shift and 63 moves), the pad, one state and Shift's release.
        assert_eq!(inbox.queue.len(), 1 + GUEST_QUEUE + 3);
        let states: Vec<&GuestMessage> = inbox
            .queue
            .iter()
            .map(|(m, _)| m)
            .filter(|m| matches!(m, GuestMessage::Gamepad(_, text, _) if text.contains("gamepad-state")))
            .collect();
        assert_eq!(states.len(), 1);
        assert!(
            matches!(states[0], GuestMessage::Gamepad(_, text, _) if text.contains(r#""seq":3"#))
        );
    }
    drain(&mut r, &queues);
    let events = r.events();
    assert_eq!(keyboard(&events), [true, false], "Shift ends up released");
    assert!(
        events.iter().any(|e| matches!(
            e,
            Input::Controller {
                buttons: 0x1000,
                ..
            }
        )),
        "the pad's latest state arrived: {events:?}"
    );
    assert!(queues.guests.state.lock().unwrap().waiting.is_empty());
}

#[test]
fn a_guest_past_its_backlog_has_everything_it_held_released() {
    let mut r = rig(u32::MAX);
    let (handle, queues) = channel();
    let guest = handle
        .guest(1, Permission::ControllerKeyboardMouse)
        .unwrap();
    let now = Instant::now();
    guest.gamepad(pad_connected(0), false, now);
    guest.input(SHIFT_DOWN.to_vec(), false);
    // A page that clicks faster than a stalled desktop takes it fills its whole backlog.
    for i in 0..GUEST_BACKLOG - 2 {
        let down = i % 2 == 0;
        guest.input(
            format!(r#"{{"v":1,"type":"mouse-button","button":0,"down":{down}}}"#).into_bytes(),
            false,
        );
    }
    // Shift's release and the pad's unplugging no longer fit.
    guest.input(SHIFT_UP.to_vec(), false);
    guest.gamepad(
        br#"{"v":2,"type":"gamepad-disconnected","controllerId":0}"#.to_vec(),
        false,
        now,
    );
    {
        let inbox = queues.guests.state.lock().unwrap();
        assert_eq!(inbox.queue.len(), 1 + GUEST_BACKLOG + 1);
        assert!(matches!(
            inbox.queue.back(),
            Some((GuestMessage::Lost(1), _))
        ));
        assert_eq!(
            inbox
                .queue
                .iter()
                .filter(|(m, _)| matches!(m, GuestMessage::Lost(_)))
                .count(),
            1,
            "one release covers every message dropped in a row"
        );
    }
    drain(&mut r, &queues);
    let events = r.events();
    assert_eq!(keyboard(&events), [true, false], "Shift ends up released");
    assert_eq!(r.core.pads.active_mask(), 0, "no pad is left plugged in");
    // The guest still plays: what it sends next goes through.
    guest.input(SHIFT_DOWN.to_vec(), false);
    drain(&mut r, &queues);
    assert_eq!(keyboard(&r.events()), [true, false, true]);
    assert!(queues.guests.state.lock().unwrap().waiting.is_empty());
}
