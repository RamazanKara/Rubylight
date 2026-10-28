//! Clipboard sync (0x5508) through the real RTSP server and control loop, on the in-memory
//! clipboard (`clipboard_sync::tests::FAKE`).
use super::*;
use crate::stream::clipboard_sync::{Clipboard, tests::FAKE};
use butterpollo_core::clipboard_sync::{
    CLIPBOARD_SYNC_MESSAGE_TYPE, Frame, KIND_TEXT, LAUNCH_PARAMETER,
};

/// Asks for clipboard sync on the pending `launch`, from a device of `perm`.
fn ask(h: &Harness, launch: &Launch, perm: u32) -> Launch {
    let mut sessions = h.h.sessions.lock().unwrap();
    let pending = sessions
        .pending
        .values_mut()
        .find(|l| l.id == launch.id)
        .expect("launch must be pending");
    pending.options.insert(LAUNCH_PARAMETER.into(), "1".into());
    pending.client.perm = perm;
    pending.clone()
}

/// The DESCRIBE's second `a=x-rl-control` line.
async fn control_ids(h: &Harness, launch: &Launch, sequence: u32) -> Result<String> {
    let describe = h.rtsp(launch, sequence, "DESCRIBE", "").await?;
    Ok(describe
        .lines()
        .filter(|line| line.starts_with("a=x-rl-control:"))
        .nth(1)
        .unwrap_or_default()
        .to_owned())
}

/// Only a launch that asked, from a device that may use the clipboard, gets 0x5508 offered;
/// stock DESCRIBE answers stay as `extension_tests` has them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clipboard_sync_is_offered_only_to_a_launch_that_asks() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    assert_eq!(
        control_ids(&h, &launch, 10).await?,
        "a=x-rl-control:0x5539,0x553b,0x553d,0x553e"
    );
    let asked = ask(&h, &launch, u32::MAX);
    assert_eq!(
        control_ids(&h, &asked, 11).await?,
        "a=x-rl-control:0x5508,0x5539,0x553b,0x553d,0x553e"
    );
    // Without the clipboard permissions, or view access, it is not offered.
    for (sequence, perm) in [
        (12, !((1 << 16) | (1 << 17))),
        (13, !((1 << 25) | (1 << 26))),
    ] {
        let refused = ask(&h, &launch, perm);
        assert_eq!(
            control_ids(&h, &refused, sequence).await?,
            "a=x-rl-control:0x5539,0x553b,0x553d,0x553e",
            "{perm:#x}"
        );
    }
    let asked = ask(&h, &launch, u32::MAX);
    let (session, client) = h.play(&asked).await?;
    h.end(&session, client).await?;
    h.finish().await
}

/// The device's copies go on the PC's clipboard from its first frame, as a Moonlight V+
/// client sends them unasked, and the PC's copies go to it from then on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn copies_travel_both_ways_after_the_first_frame() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
    let frame = |token, text: &str| {
        Frame {
            kind: KIND_TEXT,
            token,
            payload: text.as_bytes().to_vec(),
        }
        .encode()
        .unwrap()
    };
    client.send_control(CLIPBOARD_SYNC_MESSAGE_TYPE, &frame(7, "from the device"));
    until("the device's copy is on the clipboard", WAIT, || {
        FAKE.text() == "from the device"
    })
    .await;
    assert!(client.clipboard.lock().unwrap().is_empty());

    FAKE.copy("from the PC");
    until("the PC's copy arrives", WAIT, || {
        !client.clipboard.lock().unwrap().is_empty()
    })
    .await;
    let sent = Frame::decode(&client.clipboard.lock().unwrap()[0].1).expect("a valid frame");
    assert_eq!(
        (sent.kind, sent.payload.as_slice()),
        (KIND_TEXT, &b"from the PC"[..])
    );
    assert_eq!(sent.token, FAKE.sequence());
    h.end(&session, client).await?;
    h.finish().await
}
