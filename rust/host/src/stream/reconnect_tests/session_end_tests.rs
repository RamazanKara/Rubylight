//! Every way a stream ends releases the same things: /serverinfo counts no session and no
//! pending launch, and the display lease, audio route and input slots are back
//! (`Harness::empty`).

use super::*;

async fn streaming(h: &Harness) -> Result<(Arc<Session>, Client)> {
    let launch = h.launch(0, false).await?;
    let streaming = if h.short_timeout {
        h.play_short_timeout(&launch).await?
    } else {
        h.play(&launch).await?
    };
    h.counts(1, 0).await?;
    Ok(streaming)
}

/// Moonlight's stop streaming (RTSP TEARDOWN) and quit app (/cancel).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_that_stops_or_quits_leaves_nothing_behind() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let (session, client) = streaming(&h).await?;
    h.end(&session, client).await?;

    let (session, client) = streaming(&h).await?;
    let response = h.clients[0]
        .get(format!("https://127.0.0.1:{}/cancel", h.ports.https))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    assert_eq!(xml(&response, "cancel"), "1", "{response}");
    until("quit stops the stream", WAIT, || session.stopping()).await;
    drop(client);
    h.empty().await?;
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_network_drop_past_the_ping_timeout_leaves_nothing_behind() -> Result<()> {
    let h = Harness::new(2_000, 1).await?;
    let (session, client) = streaming(&h).await?;
    client.network(true);
    until(
        "the outage ends the session",
        Duration::from_millis(2_000) + WAIT,
        || session.stopping(),
    )
    .await;
    assert_eq!(session.termination_reason(), FAILED);
    drop(client);
    h.empty().await?;
    h.finish().await
}

/// Capture or the encoder failing mid-stream fails the session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_encoder_failure_leaves_nothing_behind() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let (session, client) = streaming(&h).await?;
    session.fail();
    until("the failure reaches the client", WAIT, || {
        client.reason.load(Ordering::Acquire) != 0
    })
    .await;
    assert_eq!(client.reason.load(Ordering::Acquire), FAILED);
    drop(client);
    h.empty().await?;
    h.finish().await
}

/// The host stopping, as `main` stops it, ends its streams first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_stop_leaves_nothing_behind() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let (session, client) = streaming(&h).await?;
    h.h.stop.store(true, Ordering::Release);
    let canceled = h.h.sessions.lock().unwrap().request_stop(None);
    drop(canceled);
    until("the stop ends the stream", WAIT, || session.stopping()).await;
    drop(client);
    // A stopping host takes no new launch: no `finish`.
    h.empty().await
}

/// End session from the console or another device's native API.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kick_leaves_nothing_behind() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let (session, client) = streaming(&h).await?;
    let host = h.h.clone();
    let outcome = tokio::task::spawn_blocking(move || host.end_session(None)).await?;
    assert!(outcome.is_ok(), "{outcome:?}");
    until("the kick ends the stream", WAIT, || session.stopping()).await;
    until("the kick reaches the client", WAIT, || {
        client.reason.load(Ordering::Acquire) != 0
    })
    .await;
    drop(client);
    h.empty().await?;
    h.finish().await
}

/// The same device launching again takes over its stream.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replaced_stream_leaves_only_its_replacement() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let (old, old_client) = streaming(&h).await?;
    let launch = h.launch(0, true).await?;
    until("the replacement stops the old stream", WAIT, || {
        old.stopping()
    })
    .await;
    let (session, client) = h.play(&launch).await?;
    h.counts(1, 0).await?;
    drop(old_client);
    h.end(&session, client).await?;
    h.finish().await
}

/// A launch whose client never connects expires after 30 s.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_expired_launch_leaves_nothing_behind() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    h.counts(0, 1).await?;
    age_launch(&h, &launch, Duration::from_secs(31));
    // The harness runs no maintenance loop; /serverinfo from this PC expires launches too.
    h.counts(0, 0).await?;
    // The expired launch holds its display until the last copy goes.
    drop(launch);
    h.empty().await?;
    h.finish().await
}
