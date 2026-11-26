//! Frame Warp 2 (0x5543 to 0x5546 and the meta 0x5549) through the real RTSP server and control
//! loop: a version 2 request brings a model, the frames' tags and a meta for each, and the gyro
//! pad's readings reach the metas; an older Nightjar's version 1 request, and a client that never
//! asks, get nothing while the stream goes on. No GPU pass runs, so the host learns nothing.
use super::*;
use butterpollo_media_policy::frame_warp::{
    FRAME_WARP_MODEL_MESSAGE_TYPE, FRAME_WARP_REQUEST_MESSAGE_TYPE, FRAME_WARP_TAGS_MESSAGE_TYPE,
    FRAME_WARP_V2_META_MESSAGE_TYPE, FrameWarpRequest, FrameWarpV2Meta, RequestV2, Tags,
};

/// An older Nightjar's request: version 1, eight bytes, on, no pad, the lens, no mask.
const VERSION_1: [u8; 8] = [1, 1, 0xFF, 1, 0, 0, 0, 0];

/// A version 2 request that turns by pad 0's stick and gyro, taking no mask.
fn request(enabled: bool) -> Vec<u8> {
    RequestV2 {
        gyro: true,
        ..RequestV2::new(FrameWarpRequest {
            enabled,
            controller: Some(0),
            mask_max_width: 0,
            lens: true,
        })
    }
    .encode()
    .to_vec()
}

/// A gyro reading of pad 0 (0x55000006, kind 2), degrees per second.
fn gyro(xyz: [f32; 3]) -> Vec<u8> {
    let mut b = vec![0u8; 24];
    b[..4].copy_from_slice(&20u32.to_be_bytes());
    b[4..8].copy_from_slice(&0x5500_0006u32.to_le_bytes());
    b[8] = 0;
    b[9] = 2;
    for (axis, value) in xyz.iter().enumerate() {
        b[12 + 4 * axis..16 + 4 * axis].copy_from_slice(&value.to_le_bytes());
    }
    b
}

/// The Frame Warp messages of `kind` the client heard.
fn heard(client: &Client, kind: u16) -> Vec<Vec<u8>> {
    client
        .frame_warp
        .lock()
        .unwrap()
        .iter()
        .filter(|(k, _)| *k == kind)
        .map(|(_, payload)| payload.clone())
        .collect()
}

/// The newest meta the client heard.
fn newest_meta(client: &Client) -> Option<FrameWarpV2Meta> {
    heard(client, FRAME_WARP_V2_META_MESSAGE_TYPE)
        .last()
        .and_then(|payload| FrameWarpV2Meta::decode(payload))
}

/// A version 2 request is answered with a model; every tagged frame's tags come with its meta,
/// which counts the gyro pad's readings; a stop ends both.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_version_2_request_brings_a_meta_with_every_frames_tags() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
    client.send_encrypted(
        &launch.key,
        0,
        FRAME_WARP_REQUEST_MESSAGE_TYPE,
        &request(true),
    );
    until("Frame Warp on", WAIT, || session.frame_warp.enabled()).await;
    until("the first model", WAIT, || {
        !heard(&client, FRAME_WARP_MODEL_MESSAGE_TYPE).is_empty()
    })
    .await;
    until("the first meta", WAIT, || newest_meta(&client).is_some()).await;
    let meta = newest_meta(&client).unwrap();
    assert_eq!((meta.edge_fade_strength, meta.hud_guard), (90, 2));
    assert!(meta.confidence() < 0.5, "nothing learned: {meta:?}");
    // Each meta names a frame the tags named.
    let frames: Vec<u32> = heard(&client, FRAME_WARP_TAGS_MESSAGE_TYPE)
        .iter()
        .filter_map(|payload| Tags::decode(payload))
        .flat_map(|tags| tags.tags.into_iter().map(|tag| tag.frame))
        .collect();
    assert!(frames.contains(&meta.frame), "{meta:?} {frames:?}");

    for seq in 1..=3 {
        client.send_encrypted(&launch.key, seq, 0x0206, &gyro([0.0, 40.0, 0.0]));
    }
    until("the gyro's readings in a meta", WAIT, || {
        newest_meta(&client).is_some_and(|meta| meta.gyro_seq == 3)
    })
    .await;

    client.send_encrypted(
        &launch.key,
        4,
        FRAME_WARP_REQUEST_MESSAGE_TYPE,
        &request(false),
    );
    until("Frame Warp off", WAIT, || !session.frame_warp.enabled()).await;
    // What was already on its way arrives within a loopback round trip.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let metas = heard(&client, FRAME_WARP_V2_META_MESSAGE_TYPE).len();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(heard(&client, FRAME_WARP_V2_META_MESSAGE_TYPE).len(), metas);
    h.end(&session, client).await?;
    h.finish().await
}

/// A client that never asks hears no Frame Warp; an older Nightjar's version 1 request is
/// ignored (no model, tags, mask or meta) and the stream goes on, so a version 2 request still
/// works on it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_older_nightjars_request_gets_nothing_and_the_stream_goes_on() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
    client.quiet_heartbeat().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(client.frame_warp.lock().unwrap().is_empty());

    client.send_encrypted(&launch.key, 0, FRAME_WARP_REQUEST_MESSAGE_TYPE, &VERSION_1);
    client.send_encrypted(&launch.key, 1, 0x0206, &gyro([0.0, 40.0, 0.0]));
    // The reliable messages reach the host within the heartbeat's round trip.
    client.quiet_heartbeat().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!session.frame_warp.enabled());
    assert!(client.frame_warp.lock().unwrap().is_empty());
    assert!(!session.frame_warp.first_ignored(), "logged once");

    client.send_encrypted(
        &launch.key,
        2,
        FRAME_WARP_REQUEST_MESSAGE_TYPE,
        &request(true),
    );
    until("Frame Warp on", WAIT, || session.frame_warp.enabled()).await;
    until("a model and a meta", WAIT, || {
        !heard(&client, FRAME_WARP_MODEL_MESSAGE_TYPE).is_empty() && newest_meta(&client).is_some()
    })
    .await;
    h.end(&session, client).await?;
    h.finish().await
}
