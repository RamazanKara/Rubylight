//! Live mid-stream monitor switching (`Ctrl + Alt + Shift + F1`–`F12`) over the real
//! RTSP, ENet control, and session workers.
use super::*;
use butterpollo_input_policy::monitor_switch::{
    MODIFIER_ALT, MODIFIER_CTRL, MODIFIER_SHIFT, VK_CONTROL, VK_F1, VK_MENU, VK_SHIFT,
};

fn key_packet(vk: u16, modifiers: u8, down: bool) -> Vec<u8> {
    let mut b = vec![0u8; 14];
    b[..4].copy_from_slice(&10u32.to_be_bytes());
    b[4..8].copy_from_slice(&if down { 3u32 } else { 4u32 }.to_le_bytes());
    b[9..11].copy_from_slice(&(0x8000u16 | vk).to_le_bytes());
    b[11] = modifiers;
    b
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_monitor_switch_via_enet_switches_output_without_disconnect_or_stuck_modifiers() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    h.fixture.record_inputs.store(true, Ordering::Release);
    *h.fixture.monitor_displays.lock().unwrap() = vec![
        r"\\.\DISPLAY10".into(),
        r"\\.\DISPLAY2".into(),
        r"\\.\DISPLAY1".into(),
    ];

    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;

    until("fixture stream initialized DISPLAY1", WAIT, || {
        *session.output.read().unwrap() == r"\\.\DISPLAY1"
    })
    .await;

    let mut seq = 0u32;
    let mut send_key = |vk: u16, modifiers: u8, down: bool| {
        client.send_encrypted(&launch.key, seq, 0x0206, &key_packet(vk, modifiers, down));
        seq += 1;
    };

    // Ctrl + Alt + Shift + F2 (plus auto-repeat) switches to DISPLAY2.
    send_key(VK_CONTROL, 0, true);
    send_key(VK_MENU, MODIFIER_CTRL, true);
    send_key(VK_SHIFT, MODIFIER_CTRL | MODIFIER_ALT, true);
    send_key(
        VK_F1 + 1,
        MODIFIER_CTRL | MODIFIER_ALT | MODIFIER_SHIFT,
        true,
    );
    send_key(
        VK_F1 + 1,
        MODIFIER_CTRL | MODIFIER_ALT | MODIFIER_SHIFT,
        true,
    );
    send_key(VK_F1 + 1, MODIFIER_CTRL | MODIFIER_ALT | MODIFIER_SHIFT, false);
    send_key(VK_SHIFT, MODIFIER_CTRL | MODIFIER_ALT, false);
    send_key(VK_MENU, MODIFIER_CTRL, false);
    send_key(VK_CONTROL, 0, false);

    until("session output switches to DISPLAY2", WAIT, || {
        *session.output.read().unwrap() == r"\\.\DISPLAY2"
    })
    .await;
    assert_eq!(
        *h.fixture.monitor_switches.lock().unwrap(),
        vec![r"\\.\DISPLAY2".to_owned()]
    );

    // Stream remains alive and keeps sending video frames after the switch.
    let frames_after_f2 = client.frames.load(Ordering::Acquire);
    until("video frames continue after switch", WAIT, || {
        client.frames.load(Ordering::Acquire) > frames_after_f2
    })
    .await;

    // Ctrl + Alt + Shift + F3 selects naturally-ordered DISPLAY10.
    send_key(
        VK_F1 + 2,
        MODIFIER_CTRL | MODIFIER_ALT | MODIFIER_SHIFT,
        true,
    );
    send_key(VK_F1 + 2, 0, false);
    until("session output switches to DISPLAY10", WAIT, || {
        *session.output.read().unwrap() == r"\\.\DISPLAY10"
    })
    .await;

    // Out-of-range F6 does nothing.
    send_key(
        VK_F1 + 5,
        MODIFIER_CTRL | MODIFIER_ALT | MODIFIER_SHIFT,
        true,
    );
    send_key(VK_F1 + 5, 0, false);

    // Ctrl + Alt + Shift + F1 switches back to DISPLAY1.
    send_key(
        VK_F1,
        MODIFIER_CTRL | MODIFIER_ALT | MODIFIER_SHIFT,
        true,
    );
    send_key(VK_F1, 0, false);
    until("session output switches back to DISPLAY1", WAIT, || {
        *session.output.read().unwrap() == r"\\.\DISPLAY1"
    })
    .await;

    // Plain F1 (without Ctrl+Alt+Shift) passes through as ordinary key input.
    send_key(VK_F1, 0, true);
    send_key(VK_F1, 0, false);
    until("plain F1 reached forwarded inputs", WAIT, || {
        h.fixture
            .forwarded_inputs
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(
                e,
                butterpollo_input_policy::input::Input::Keyboard { key, down: true, .. }
                    if *key == VK_F1
            ))
    })
    .await;

    let forwarded = h.fixture.forwarded_inputs.lock().unwrap().clone();
    // Modifiers passed through symmetrically (downs and ups) without leaking matched F-keys.
    for modifier in [VK_CONTROL, VK_MENU, VK_SHIFT] {
        assert!(
            forwarded.iter().any(|e| matches!(
                e,
                butterpollo_input_policy::input::Input::Keyboard { key, down: true, .. }
                    if *key == modifier
            )),
            "modifier down {modifier:#x} missing"
        );
        assert!(
            forwarded.iter().any(|e| matches!(
                e,
                butterpollo_input_policy::input::Input::Keyboard { key, down: false, .. }
                    if *key == modifier
            )),
            "modifier up {modifier:#x} missing"
        );
    }
    assert!(
        !forwarded.iter().any(|e| matches!(
            e,
            butterpollo_input_policy::input::Input::Keyboard { key, .. }
                if *key == VK_F1 + 1 || *key == VK_F1 + 2 || *key == VK_F1 + 5
        )),
        "matched monitor-switch F-keys must be swallowed: {forwarded:?}"
    );
    assert_eq!(
        *h.fixture.monitor_switches.lock().unwrap(),
        vec![
            r"\\.\DISPLAY2".to_owned(),
            r"\\.\DISPLAY10".to_owned(),
            r"\\.\DISPLAY1".to_owned(),
        ]
    );

    h.end(&session, client).await?;
    h.finish().await
}
