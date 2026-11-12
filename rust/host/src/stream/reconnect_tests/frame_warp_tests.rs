//! Frame Warp (0x5543 to 0x5546) through the real RTSP server and control loop. The fixture
//! tags its frames as the video senders do; no GPU pass runs, so the host learns nothing and
//! sends only the model a request is answered with and the tags.
use super::*;
use butterpollo_core::frame_warp::{
    FRAME_WARP_MODEL_MESSAGE_TYPE, FRAME_WARP_REQUEST_MESSAGE_TYPE, FRAME_WARP_TAGS_MESSAGE_TYPE,
    FrameWarpRequest, Model, Tag, Tags,
};

/// A request that turns Frame Warp on or off, tracking no controller and taking no mask.
fn request(enabled: bool) -> Vec<u8> {
    FrameWarpRequest {
        enabled,
        controller: None,
        mask_max_width: 0,
    }
    .encode()
    .to_vec()
}

/// A relative mouse move, as Moonlight sends it (0x0206, magic 7).
fn mouse_move(dx: i16, dy: i16) -> Vec<u8> {
    let mut b = vec![0u8; 12];
    b[..4].copy_from_slice(&8u32.to_be_bytes());
    b[4..8].copy_from_slice(&7u32.to_le_bytes());
    b[8..10].copy_from_slice(&dx.to_be_bytes());
    b[10..12].copy_from_slice(&dy.to_be_bytes());
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

/// The newest tag the client heard.
fn newest_tag(client: &Client) -> Option<Tag> {
    heard(client, FRAME_WARP_TAGS_MESSAGE_TYPE)
        .last()
        .and_then(|payload| Tags::decode(payload))
        .and_then(|tags| tags.tags.last().copied())
}

/// A request turns Frame Warp on and is answered with a model; the relative mouse moves that
/// follow reach the frames' tags as running sums; a request to stop ends the tags.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_brings_a_model_and_tags_that_carry_the_mouse() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
    assert!(!session.frame_warp.enabled());

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
    let model =
        Model::decode(&heard(&client, FRAME_WARP_MODEL_MESSAGE_TYPE)[0]).expect("a valid model");
    assert!(!model.mouse_locked && !model.stick_locked, "{model:?}");
    until("the first tags", WAIT, || newest_tag(&client).is_some()).await;

    client.send_encrypted(&launch.key, 1, 0x0206, &mouse_move(40, -8));
    client.send_encrypted(&launch.key, 2, 0x0206, &mouse_move(2, 3));
    until("a tag that carries both moves", WAIT, || {
        newest_tag(&client).is_some_and(|tag| (tag.mouse_x, tag.mouse_y) == (42, -5))
    })
    .await;

    client.send_encrypted(
        &launch.key,
        3,
        FRAME_WARP_REQUEST_MESSAGE_TYPE,
        &request(false),
    );
    until("Frame Warp off", WAIT, || !session.frame_warp.enabled()).await;
    // What was already on its way arrives within a loopback round trip.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let tags = heard(&client, FRAME_WARP_TAGS_MESSAGE_TYPE).len();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(heard(&client, FRAME_WARP_TAGS_MESSAGE_TYPE).len(), tags);
    h.end(&session, client).await?;
    h.finish().await
}

/// With `frame_warp` off the host ignores a request: nothing is learned, tagged or sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_is_ignored_while_the_setting_is_off() -> Result<()> {
    let h = Harness::configured(10_000, 1, "frame_warp=false\n").await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
    client.send_encrypted(
        &launch.key,
        0,
        FRAME_WARP_REQUEST_MESSAGE_TYPE,
        &request(true),
    );
    client.send_encrypted(&launch.key, 1, 0x0206, &mouse_move(40, -8));
    // The reliable messages reach the host within the heartbeat's round trip.
    client.quiet_heartbeat().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!session.frame_warp.enabled());
    assert!(client.frame_warp.lock().unwrap().is_empty());
    h.end(&session, client).await?;
    h.finish().await
}
