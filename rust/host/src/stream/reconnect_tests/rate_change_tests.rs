//! Rate changes (0x553C) through the real HTTP, RTSP, ENet and session workers: offered only
//! to a stream whose client launched with `rlRateChange=1`, a bitrate set at once and capped
//! by `max_bitrate`, a rate queued as a mode switch. The fixture replaces the session loop, so
//! the switch itself and its answer are covered by `butterpollo_core::rate_change`'s tests
//! and the hardware queue.
use super::*;
use butterpollo_core::rate_change::{
    Outcome, RATE_CHANGE_MESSAGE_TYPE, RateChangeRequest, RateChangeResult, Reason,
};

/// The rate change answers `client` heard.
fn rate_answers(client: &Client) -> Vec<RateChangeResult> {
    client
        .rate_answers
        .lock()
        .unwrap()
        .iter()
        .map(|(_, payload)| RateChangeResult::decode(payload).expect("a valid answer"))
        .collect()
}

fn ask(serial: u32, bitrate_kbps: u32, fps_millihz: u32) -> [u8; 16] {
    RateChangeRequest {
        serial,
        bitrate_kbps,
        fps_millihz,
    }
    .encode()
}

/// Waits for answer `serial` and returns it; every request is answered once, in order.
async fn answer(client: &Client, serial: u32) -> RateChangeResult {
    until("the rate change answer", WAIT, || {
        rate_answers(client)
            .iter()
            .any(|answer| answer.serial == serial)
    })
    .await;
    let answers = rate_answers(client);
    assert_eq!(
        answers.last().map(|answer| answer.serial),
        Some(serial),
        "{answers:?}"
    );
    *answers.last().unwrap()
}

/// A stock launch is not offered 0x553C and its requests go unanswered; `rlRateChange=1`
/// adds it to that stream's DESCRIBE, and a value other than 0 or 1 fails the launch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rate_changes_reach_only_a_client_that_launched_asking() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let refused = h
        .launch_response_with(0, false, None, &[("rlRateChange", "yes")])
        .await;
    assert!(
        refused
            .as_ref()
            .map_or(true, |response| !response.contains("status_code=\"200\"")),
        "{refused:?}"
    );

    let launch = h.launch(0, false).await?;
    let describe = h.rtsp(&launch, 10, "DESCRIBE", "").await?;
    assert!(!describe.contains("0x553c"), "{describe}");
    let (session, client) = h.play(&launch).await?;
    let before = session.bitrate.load(Ordering::Acquire);
    client.send_control(RATE_CHANGE_MESSAGE_TYPE, &ask(1, before + 1000, 0));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(rate_answers(&client).is_empty());
    assert_eq!(session.bitrate.load(Ordering::Acquire), before);
    h.end(&session, client).await?;

    let launch = h
        .launch_with(0, false, None, &[("rlRateChange", "1")])
        .await?;
    let describe = h.rtsp(&launch, 10, "DESCRIBE", "").await?;
    assert!(
        describe.contains("a=x-rl-control:0x5539,0x553b,0x553c,0x553d,0x553e\r\n"),
        "{describe}"
    );
    let (session, client) = h.play(&launch).await?;
    client.send_control(RATE_CHANGE_MESSAGE_TYPE, &ask(1, 12_000, 0));
    let applied = answer(&client, 1).await;
    assert_eq!(
        (applied.bitrate, applied.bitrate_kbps, applied.fps),
        (Outcome::APPLIED, 12_000, Outcome::UNCHANGED)
    );
    assert_eq!(session.bitrate.load(Ordering::Acquire), 12_000);
    h.end(&session, client).await?;
    h.finish().await
}

/// The device's `max_bitrate` caps a bitrate, which applies at once; a rate is capped by the
/// throughput the encoder allows and queued as a switch, which a later request supersedes;
/// nonsense and a host with switching off are refused, and the stream keeps its values.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rate_changes_set_the_bitrate_and_queue_the_rate() -> Result<()> {
    let h = Harness::configured(10_000, 1, "max_bitrate = 30000\n").await?;
    let launch = h
        .launch_with(0, false, None, &[("rlRateChange", "1")])
        .await?;
    let (session, client) = h.play(&launch).await?;

    client.send_control(RATE_CHANGE_MESSAGE_TYPE, &ask(1, 50_000, 0));
    let capped = answer(&client, 1).await;
    assert_eq!(
        (capped.bitrate, capped.bitrate_kbps),
        (Outcome::clamped(Reason::HostLimit), 30_000)
    );
    assert_eq!(session.bitrate.load(Ordering::Acquire), 30_000);

    // The launch's 60 fps down to 30: queued as a switch at the stream's size, and answered
    // only once the encoder runs at it. The fixture stands in for the session loop, which
    // never takes it here.
    client.send_control(RATE_CHANGE_MESSAGE_TYPE, &ask(2, 0, 30_000));
    until("the rate switch queued", WAIT, || {
        session.reconfigure.lock().unwrap().requested().fps_millihz == 30_000
    })
    .await;
    let queued = session.reconfigure.lock().unwrap().requested();
    assert_eq!((queued.width, queued.height), (640, 480));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(rate_answers(&client).len(), 1);
    // The test encoder reports no throughput: up to 60 fps, the launch's rate, which the
    // stream runs at. The queued switch is withdrawn and its request superseded; the
    // bitrate applies at once.
    client.send_control(RATE_CHANGE_MESSAGE_TYPE, &ask(3, 20_000, 120_000));
    let faster = answer(&client, 3).await;
    assert_eq!(
        (faster.fps, faster.fps_millihz),
        (Outcome::clamped(Reason::EncoderLimit), 60_000)
    );
    assert_eq!(
        (faster.bitrate, faster.bitrate_kbps),
        (Outcome::APPLIED, 20_000)
    );
    assert_eq!(session.bitrate.load(Ordering::Acquire), 20_000);
    let superseded = rate_answers(&client)[1];
    assert_eq!(
        (superseded.serial, superseded.fps, superseded.fps_millihz),
        (2, Outcome::refused(Reason::Superseded), 60_000)
    );
    assert_eq!(
        session.reconfigure.lock().unwrap().requested().fps_millihz,
        60_000
    );

    client.send_control(RATE_CHANGE_MESSAGE_TYPE, &ask(4, 0, 0));
    let invalid = answer(&client, 4).await;
    assert_eq!(invalid.fps, Outcome::refused(Reason::Invalid));
    assert_eq!(
        (invalid.bitrate_kbps, invalid.fps_millihz),
        (20_000, 60_000)
    );

    h.h.config
        .write()
        .unwrap()
        .values
        .insert("stream_reconfigure".into(), "false".into());
    client.send_control(RATE_CHANGE_MESSAGE_TYPE, &ask(5, 0, 30_000));
    let disabled = answer(&client, 5).await;
    assert_eq!(
        (disabled.fps, disabled.fps_millihz),
        (Outcome::refused(Reason::Disabled), 60_000)
    );
    assert_eq!(rate_answers(&client).len(), 5);
    h.end(&session, client).await?;
    h.finish().await
}
