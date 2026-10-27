//! Input over the DataChannels, through the real signalling server, session actor and media
//! transport. The host's input device is the fixture's recorder, so nothing reaches the desktop.

use super::*;
use crate::webrtc::input::Recorder;
use butterpollo_core::input::Input;

const PAD: u32 = 1 << 8;
const MOUSE: u32 = 1 << 11;
const KEYBOARD: u32 = 1 << 12;
const START: u32 = 1 << 7;
const A: u32 = 1;
const B: u32 = 1 << 1;

fn recorder(web: &Web) -> Recorder {
    web.harness.fixture.webrtc_input.clone()
}

fn connected(id: u32) -> String {
    format!(
        r#"{{"v":2,"type":"gamepad-connected","controllerId":{id},"id":"Pad","mapping":"standard","buttons":17,"axes":4,"dualRumble":true,"triggerRumble":true}}"#
    )
}

fn state(id: u32, seq: u64, buttons: u32, stick_x: f64) -> String {
    format!(
        r#"{{"v":2,"type":"gamepad-state","controllerId":{id},"seq":{seq},"buttons":{buttons},"leftTrigger":0,"rightTrigger":0,"leftStickX":{stick_x},"leftStickY":0,"rightStickX":0,"rightStickY":0}}"#
    )
}

fn key(code: &str, down: bool) -> String {
    format!(r#"{{"v":1,"type":"key","code":"{code}","down":{down}}}"#)
}

fn key_event(key: u16, down: bool) -> Input {
    Input::Keyboard {
        key,
        modifiers: 0,
        flags: 0,
        down,
    }
}

fn button_event(button: u8, down: bool) -> Input {
    Input::MouseButton { button, down }
}

fn buttons_of(events: &[Input], down: bool) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Input::MouseButton { down: d, .. } if *d == down))
        .count()
}

/// Wait for `event` to reach the recorder.
async fn injected(rec: &Recorder, event: Input) {
    until(&format!("{event:?} injected"), WAIT, || {
        rec.events().contains(&event)
    })
    .await;
}

/// A streaming session for a device with `permission`, and its client with the `input` channel
/// open.
async fn live(web: &Web, permission: u32) -> (Ws, Peer) {
    let mut ws = web.device("TV", permission).await;
    let peer = stream(web, &mut ws).await;
    peer.open_input();
    (ws, peer)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_gamepad_connects_changes_and_leaves_through_the_dispatcher() -> Result<()> {
    let web = Web::new().await?;
    let rec = recorder(&web);
    let (_ws, peer) = live(&web, u32::MAX).await;
    // The gamepad channel is unordered without retransmits: each message goes again on
    // every poll until the host has it, as a browser sends a pad's state.
    until("the pad arrived", WAIT, || {
        peer.gamepad(&connected(4));
        rec.events()
            .iter()
            .any(|e| matches!(e, Input::Arrival { id: 0, kind: 1, .. }))
    })
    .await;
    until("A pressed", WAIT, || {
        peer.gamepad(&state(4, 5, A, 0.));
        rec.events().iter().any(|e| {
            matches!(
                e,
                Input::Controller {
                    buttons: 0x1000,
                    ..
                }
            )
        })
    })
    .await;
    // An older snapshot does not undo it; the next one does count.
    peer.gamepad(&state(4, 4, B, 0.));
    until("A released", WAIT, || {
        peer.gamepad(&state(4, 6, 0, 0.));
        rec.events().iter().any(|e| {
            matches!(
                e,
                Input::Controller {
                    active: 1,
                    buttons: 0,
                    ..
                }
            )
        })
    })
    .await;
    assert!(
        !rec.events().iter().any(|e| matches!(
            e,
            Input::Controller {
                buttons: 0x2000,
                ..
            }
        )),
        "the stale B press reached the injector"
    );
    until("the pad left", WAIT, || {
        peer.gamepad(r#"{"v":2,"type":"gamepad-disconnected","controllerId":4}"#);
        rec.events()
            .iter()
            .any(|e| matches!(e, Input::Controller { active: 0, .. }))
    })
    .await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn rumble_reaches_the_client_scaled_to_zero_to_one() -> Result<()> {
    let web = Web::new().await?;
    let rec = recorder(&web);
    let (_ws, peer) = live(&web, u32::MAX).await;
    until("the pad arrived", WAIT, || {
        peer.gamepad(&connected(9));
        !rec.events().is_empty()
    })
    .await;
    rec.post_feedback(0, crate::stream::RUMBLE_DATA, &[0xff, 0xff, 0x00, 0x80]);
    until("rumble reached the client", WAIT, || {
        !peer.heard("rumble").is_empty()
    })
    .await;
    let rumble = peer.heard("rumble");
    assert_eq!(rumble[0]["v"], 2);
    assert_eq!(rumble[0]["controllerId"], 9);
    assert_eq!(rumble[0]["strongMagnitude"], 1.0);
    let weak = rumble[0]["weakMagnitude"].as_f64().unwrap();
    assert!((weak - 32768. / 65535.).abs() < 1e-9, "{weak}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn gamepad_mouse_mode_moves_the_pointer_and_lets_go_of_its_buttons() -> Result<()> {
    let web = Web::new().await?;
    let rec = recorder(&web);
    let (_ws, peer) = live(&web, u32::MAX).await;
    // The gamepad channel is unordered without retransmits, and a busy host
    // drops what it cannot take. So the pad is sent as a browser sends it:
    // again on every poll, each state with the next sequence number.
    let mut seq = 0;
    let mut send = |buttons: u32, stick_x: f64| {
        seq += 1;
        peer.gamepad(&connected(1));
        peer.gamepad(&state(1, seq, buttons, stick_x));
    };
    // Holding Start for 750 ms is real time on the host: wait it out, then let go.
    // The hold starts when the host handles the press, which reaches the pad
    // in the same step; on a busy PC that was well after it was sent.
    until("Start pressed on the host", WAIT, || {
        send(START, 0.);
        rec.events()
            .iter()
            .any(|e| matches!(e, Input::Controller { buttons, .. } if *buttons != 0))
    })
    .await;
    tokio::time::sleep(Duration::from_millis(850)).await;
    until("mouse mode on", WAIT, || {
        send(0, 0.);
        peer.heard("mouse-mode")
            .iter()
            .any(|m| m["active"] == true && m["controllerId"] == 1)
    })
    .await;
    // The stick moves the pointer on the 50 ms tick, A presses the left button.
    until("the pointer moved", WAIT, || {
        send(A, 1.);
        rec.events()
            .iter()
            .any(|e| matches!(e, Input::Relative { x, .. } if *x > 0))
    })
    .await;
    injected(&rec, button_event(1, true)).await;
    // The pad leaving with the button down lets go of it and leaves the mode.
    until("mouse mode off", WAIT, || {
        peer.gamepad(r#"{"v":2,"type":"gamepad-disconnected","controllerId":1}"#);
        peer.heard("mouse-mode")
            .iter()
            .any(|m| m["active"] == false && m["controllerId"] == 1)
    })
    .await;
    injected(&rec, button_event(1, false)).await;
    // The pointer stops with the mode.
    tokio::time::sleep(Duration::from_millis(120)).await;
    let before = rec.events().len();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(rec.events().len(), before, "the tick kept running");
    assert_eq!(
        buttons_of(&rec.events(), true),
        buttons_of(&rec.events(), false)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn keyboard_and_mouse_messages_reach_the_dispatcher_in_order() -> Result<()> {
    let web = Web::new().await?;
    let rec = recorder(&web);
    let (_ws, peer) = live(&web, u32::MAX).await;
    for text in [
        key("KeyA", true),
        key("KeyA", false),
        key("Fn", true),
        r#"{"v":1,"type":"text","text":"hi"}"#.to_owned(),
        r#"{"v":1,"type":"mouse-move","dx":3,"dy":-4}"#.to_owned(),
        r#"{"v":1,"type":"mouse-position","x":0.5,"y":0.25}"#.to_owned(),
        r#"{"v":1,"type":"mouse-button","button":2,"down":true}"#.to_owned(),
        r#"{"v":1,"type":"mouse-button","button":2,"down":false}"#.to_owned(),
        r#"{"v":1,"type":"wheel","dx":0,"dy":120}"#.to_owned(),
    ] {
        peer.input(&text);
    }
    until("every message injected", WAIT, || rec.events().len() >= 8).await;
    assert_eq!(
        rec.events(),
        vec![
            key_event(0x41, true),
            key_event(0x41, false),
            Input::Text("hi".into()),
            Input::Relative { x: 3, y: -4 },
            Input::Absolute {
                x: 960,
                y: 270,
                width: 1920,
                height: 1080
            },
            button_event(3, true),
            button_event(3, false),
            Input::Scroll {
                amount: -120,
                horizontal: false
            },
        ]
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_devices_permissions_gate_keyboard_mouse_and_gamepad() -> Result<()> {
    let web = Web::new().await?;
    let rec = recorder(&web);
    let (_ws, peer) = live(&web, !(KEYBOARD | PAD)).await;
    peer.input(&key("KeyA", true));
    peer.gamepad(&connected(0));
    peer.input(r#"{"v":1,"type":"mouse-move","dx":1,"dy":2}"#);
    // The mouse is allowed and comes after the others: once it is through, they were refused.
    injected(&rec, Input::Relative { x: 1, y: 2 }).await;
    assert_eq!(rec.events(), vec![Input::Relative { x: 1, y: 2 }]);
    let warnings = web.session().launch.warnings.snapshot();
    let codes: Vec<_> = warnings.iter().map(|w| w.code.as_str()).collect();
    assert!(codes.contains(&"input_permission_keyboard"), "{codes:?}");
    assert!(codes.contains(&"input_permission_controller"), "{codes:?}");
    assert!(!codes.contains(&"input_permission_mouse"), "{codes:?}");
    drop(peer);

    // Without the mouse, the keyboard still works.
    let web = Web::new().await?;
    let rec = recorder(&web);
    let (_ws, peer) = live(&web, !MOUSE).await;
    peer.input(r#"{"v":1,"type":"mouse-move","dx":1,"dy":2}"#);
    peer.input(&key("KeyB", true));
    injected(&rec, key_event(0x42, true)).await;
    assert_eq!(rec.events(), vec![key_event(0x42, true)]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_the_input_channel_releases_every_held_key_and_button() -> Result<()> {
    let web = Web::new().await?;
    let rec = recorder(&web);
    let (_ws, peer) = live(&web, u32::MAX).await;
    peer.input(&key("KeyA", true));
    peer.input(&key("ShiftLeft", true));
    peer.input(r#"{"v":1,"type":"mouse-button","button":0,"down":true}"#);
    injected(&rec, button_event(1, true)).await;
    peer.close_input();
    injected(&rec, key_event(0x41, false)).await;
    injected(&rec, key_event(0xa0, false)).await;
    injected(&rec, button_event(1, false)).await;
    // The stream goes on: the gamepad and control channels are untouched.
    assert!(!web.session().stopping());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stop_or_switch_releases_held_input_and_drops_the_injector() -> Result<()> {
    let web = Web::new().await?;
    let rec = recorder(&web);
    let mut ws = web.device("TV", u32::MAX).await;
    let peer = stream(&web, &mut ws).await;
    peer.open_input();
    peer.input(&key("KeyA", true));
    until("the pad is pressed", WAIT, || {
        peer.gamepad(&connected(2));
        peer.gamepad(&state(2, 1, A, 0.));
        rec.events().iter().any(|e| {
            matches!(
                e,
                Input::Controller {
                    buttons: 0x1000,
                    ..
                }
            )
        })
    })
    .await;
    injected(&rec, key_event(0x41, true)).await;
    assert_eq!(rec.sinks(), (1, 1));

    // Switching to the running app restarts the stream.
    ws.send(web.request("switch-session", "h264", false, 2))
        .await;
    status(&mut ws, "host-session-status", "resuming").await;
    status(&mut ws, "session-status", "stopping").await;
    status(&mut ws, "session-status", "idle").await;
    status(&mut ws, "host-session-status", "starting").await;
    injected(&rec, key_event(0x41, false)).await;
    until("the pad was neutralised and unplugged", WAIT, || {
        rec.events()
            .iter()
            .any(|e| matches!(e, Input::Controller { active: 0, .. }))
    })
    .await;
    until("the first injector dropped", WAIT, || rec.sinks().1 == 0).await;
    drop(peer);

    // The next session starts clean and a stop releases its keys too.
    rec.clear();
    let (id, peer) = answer(&mut ws, VideoCodec::H264).await;
    streaming(&mut ws, id).await;
    // As `stream` does: the transport is up before the input channel opens.
    until("video received", WAIT, || peer.count() >= 2).await;
    peer.open_input();
    peer.input(&key("KeyB", true));
    peer.input(r#"{"v":1,"type":"mouse-button","button":0,"down":true}"#);
    injected(&rec, button_event(1, true)).await;
    ws.send(json!({"version":2,"type":"stop-session"})).await;
    status(&mut ws, "session-status", "stopping").await;
    status(&mut ws, "session-status", "idle").await;
    injected(&rec, key_event(0x42, false)).await;
    injected(&rec, button_event(1, false)).await;
    until("the second injector dropped", WAIT, || {
        rec.sinks() == (2, 0)
    })
    .await;
    web.idle().await;
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_drops_away_releases_held_input() -> Result<()> {
    let web = Web::new().await?;
    let rec = recorder(&web);
    let (_ws, peer) = live(&web, u32::MAX).await;
    peer.input(&key("KeyA", true));
    injected(&rec, key_event(0x41, true)).await;
    // The browser goes away without a word: the host's link times out or stops, and either way the
    // key must not stay down. Stopping the session is the host's reaction.
    web.session().stop();
    drop(peer);
    injected(&rec, key_event(0x41, false)).await;
    web.idle().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_without_the_controller_bit_gets_no_pad_and_no_mouse_mode() -> Result<()> {
    let web = Web::new().await?;
    let rec = recorder(&web);
    let (_ws, peer) = live(&web, !(PAD | KEYBOARD)).await;
    peer.gamepad(&connected(0));
    peer.gamepad(&state(0, 1, START, 0.));
    tokio::time::sleep(Duration::from_millis(850)).await;
    peer.gamepad(&state(0, 2, START, 1.));
    peer.gamepad(&state(0, 3, 0, 1.));
    tokio::time::sleep(Duration::from_millis(200)).await;
    // The mouse it may use works, so the gamepad frames had their chance to arrive.
    peer.input(r#"{"v":1,"type":"mouse-move","dx":5,"dy":0}"#);
    injected(&rec, Input::Relative { x: 5, y: 0 }).await;
    assert_eq!(rec.events(), vec![Input::Relative { x: 5, y: 0 }]);
    assert!(peer.heard("mouse-mode").is_empty());
    assert!(peer.heard("rumble").is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_moonlight_launch_that_supersedes_a_webrtc_stream_releases_its_held_keys() -> Result<()> {
    let web = Web::new().await?;
    let rec = recorder(&web);
    let mut ws = web.device("TV", u32::MAX).await;
    let peer = stream(&web, &mut ws).await;
    peer.open_input();
    peer.input(&key("KeyW", true));
    peer.input(r#"{"v":1,"type":"mouse-button","button":0,"down":true}"#);
    injected(&rec, key_event(0x57, true)).await;
    injected(&rec, button_event(1, true)).await;
    let webrtc = web.session();

    let launch = web.harness.launch(0, false).await?;
    status(&mut ws, "session-status", "idle").await;
    assert!(webrtc.stopping());
    injected(&rec, key_event(0x57, false)).await;
    injected(&rec, button_event(1, false)).await;
    until("the WebRTC injector dropped", WAIT, || rec.sinks().1 == 0).await;
    let (session, client) = web.harness.play(&launch).await?;
    web.harness.end(&session, client).await?;
    drop(peer);
    Ok(())
}
