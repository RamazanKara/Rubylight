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
fn a_gamepad_arrives_reports_and_leaves_through_the_injector() {
    let mut r = rig(u32::MAX);
    r.pad(3, 0);
    r.state(3, 1, A, 1., 10);
    r.core.gamepad(
        r#"{"v":2,"type":"gamepad-disconnected","controllerId":3}"#,
        r.at(20),
    );
    let events = r.events();
    assert!(matches!(
        events[0],
        Input::Arrival {
            id: 0,
            kind: 1,
            ..
        }
    ));
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
fn an_idle_session_has_nothing_to_wake_for() {
    let mut r = rig(u32::MAX);
    let now = r.at(0);
    assert_eq!(r.core.wait(now), None);
    // A key held is watched for the session stopping; nothing else.
    r.input(r#"{"v":1,"type":"key","code":"KeyA","down":true}"#);
    assert_eq!(r.core.wait(now), Some(STOP_POLL));
    r.input(r#"{"v":1,"type":"key","code":"KeyA","down":false}"#);
    assert_eq!(r.core.wait(now), None);
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
    // Gamepad mouse mode needs the mouse bit as well as the pad's.
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
    core.input(r#"{"v":1,"type":"key","code":"KeyC","down":true}"#, ms(1200));
    assert_eq!(rec.events(), vec![key_down(0x43)]);
    assert_eq!(rec.sinks(), (1, 1));
}

#[test]
fn the_worker_thread_buffers_early_frames_and_releases_when_the_transport_closes() {
    let f = Fixture::new();
    let rec = Recorder::default();
    let make = rec.clone();
    let (tx, rx) = mpsc::channel();
    let handle = InputHandle(tx);
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
    let (tx, rx) = mpsc::channel();
    let handle = InputHandle(tx);
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
