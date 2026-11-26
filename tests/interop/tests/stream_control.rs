//! Nightjar stream, control extensions, cursor, Frame Warp, adaptive FEC, rate change, and roaming interop tests.
mod common;

use anyhow::Result;
use butterpollo_media_policy::{
    extensions::FecChoice,
    statistical_fec::{FEC_REPORT_MESSAGE_TYPE, FecReport},
};
use butterpollo_stream::stream::reconnect_harness::{CLOSED, Harness, until};
use common::{LOADED, MOVED, Recorder, device, stream, stream_asking, stream_with};
use nightjar_core::{
    control::{ControlEvent, ptype},
    http::{Availability, Feature, LaunchRequest},
    roaming::RoamEvent,
    sdp::VIDEO_FORMAT_H264,
};
use std::{
    net::IpAddr,
    sync::{Arc, atomic::Ordering},
};

/// The host's control extensions Nightjar must know, in the order the host announces them.
const CONTROL_EXTENSIONS: [u16; 18] = [
    ptype::PHASE_LOCK_REPORT,
    ptype::DISPLAY_CAPS,
    ptype::RECONFIGURE,
    ptype::CONTROLLER_SPEAKER,
    ptype::CONTROLLER_MICROPHONE,
    ptype::CONTROLLER_AUDIO_CONTROL,
    ptype::HOST_TIMING,
    ptype::CLOCK_SYNC,
    ptype::LINK_NOTICE,
    // The second line: the 3.0 extensions on by default.
    FEC_REPORT_MESSAGE_TYPE,
    butterpollo_input_policy::text_input::TEXT_INPUT_MESSAGE_TYPE,
    ptype::PIPELINE_RATES,
    butterpollo_input_policy::keyboard_layout::KEYBOARD_LAYOUT_MESSAGE_TYPE,
    butterpollo_media_policy::frame_warp::FRAME_WARP_REQUEST_MESSAGE_TYPE,
    butterpollo_media_policy::frame_warp::FRAME_WARP_MODEL_MESSAGE_TYPE,
    butterpollo_media_policy::frame_warp::FRAME_WARP_TAGS_MESSAGE_TYPE,
    butterpollo_media_policy::frame_warp::FRAME_WARP_MASK_MESSAGE_TYPE,
    butterpollo_media_policy::frame_warp::FRAME_WARP_V2_META_MESSAGE_TYPE,
];

/// Launch, the RTSP handshake and the control stream both ways: Nightjar's keyframe request
/// and clock sync probe reach the host, and the host's HDR mode and clock sync answer reach
/// Nightjar. Stopping the stream ends the host's session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nightjar_launches_and_streams_with_control_both_ways() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let client = device(&h, u32::MAX, &[])?;
    let (launch, session, stream, recorder) = stream(&h, &client).await?;
    h.counts(1, 0).await?;

    let outcome = stream.outcome();
    assert_eq!(outcome.negotiated_video_format, VIDEO_FORMAT_H264);
    assert_eq!(outcome.host_control_messages, CONTROL_EXTENSIONS);
    assert!(
        outcome.host_roaming,
        "the host offers roaming in its DESCRIBE"
    );
    assert_eq!(
        recorder.seen(|s| s.control_messages.clone()),
        Some(CONTROL_EXTENSIONS.to_vec())
    );
    let setup = recorder.seen(|s| s.setup).expect("the decoder was set up");
    assert_eq!((setup.width, setup.height), (640, 480));

    // Nightjar to the host: a keyframe request and a clock sync probe.
    let requests = session.stats.idr_requests.load(Ordering::Acquire);
    stream.request_idr_frame();
    until("Nightjar's keyframe request at the host", LOADED, || {
        session.stats.idr_requests.load(Ordering::Acquire) > requests
    })
    .await;
    stream.send_clock_sync_probe(nightjar_core::session::now_us())?;
    until("the host's clock sync answer", LOADED, || {
        recorder.saw(|e| matches!(e, ControlEvent::ClockSync { .. }))
    })
    .await;
    // The host to Nightjar: the fixture's display reports HDR metadata.
    until("the host's HDR mode", LOADED, || {
        recorder.saw(|e| matches!(e, ControlEvent::HdrMode { .. }))
    })
    .await;

    stream.stop();
    until("the host's session ended", LOADED, || session.stopping()).await;
    until("sessions released", LOADED, || {
        h.h.sessions.lock().unwrap().idle()
    })
    .await;
    assert!(!h.h.sessions.lock().unwrap().active.contains_key(&launch.id));
    assert_eq!(recorder.seen(|s| s.terminated), None);
    h.finish().await
}

/// The device-drawn cursor (0x5540/0x5541) with Desktop Duplication capture: the host offers
/// it in serverinfo and DESCRIBE, Nightjar's enable leaves the pointer out of the video, a
/// pointer the stream notes reaches Nightjar as an image, a pointer it saw before comes back
/// by id, and Nightjar's disable puts the pointer back into the video.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nightjar_receives_the_cursor_it_draws() -> Result<()> {
    use butterpollo_input_policy::client_cursor::Pointer;
    use nightjar_core::control::{ClientMessage, CursorShape};
    let h = Harness::configured(10_000, 1, "capture=ddx\n").await?;
    let client = device(&h, u32::MAX, &[])?;
    assert!(
        client
            .server_info(true)?
            .capabilities
            .has(Feature::CursorShape)
    );
    let (_launch, session, stream, recorder) = stream(&h, &client).await?;
    assert!(
        stream
            .outcome()
            .host_control_messages
            .contains(&ptype::CURSOR_SHAPE_REQUEST)
    );
    assert!(!session.client_cursor.active());
    stream.send(ClientMessage::CursorShapeRequest { enabled: true })?;
    until("the host leaves the pointer out", LOADED, || {
        session.client_cursor.active()
    })
    .await;

    // The fixture's capture notes nothing; the test notes the pointer as the stream does.
    let images = || {
        recorder.seen(|s| {
            s.events
                .iter()
                .filter_map(|e| match e {
                    ControlEvent::CursorShape(CursorShape::Image {
                        id,
                        width,
                        height,
                        hotspot_x,
                        hotspot_y,
                        ..
                    }) => Some((*id, *width, *height, *hotspot_x, *hotspot_y)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
    };
    let arrow: Arc<[u8]> = [0, 0, 0, 255].repeat(8 * 8).into();
    let beam: Arc<[u8]> = [255, 255, 255, 255].repeat(4 * 12).into();
    let pointer = |pixels, width, height| Pointer {
        width,
        height,
        hotspot: [1, 2],
        logic: false,
        pixels,
    };
    session
        .client_cursor
        .note(Some(pointer(&arrow, 8, 8)), "fake");
    until("the arrow at Nightjar", LOADED, || images().len() == 1).await;
    session
        .client_cursor
        .note(Some(pointer(&beam, 4, 12)), "fake");
    until("the beam at Nightjar", LOADED, || images().len() == 2).await;
    let shown = images();
    assert_eq!(
        (shown[0].1, shown[0].2, shown[0].3, shown[0].4),
        (8, 8, 1, 2)
    );
    assert_eq!((shown[1].1, shown[1].2), (4, 12));
    session
        .client_cursor
        .note(Some(pointer(&arrow, 8, 8)), "fake");
    let arrow_id = shown[0].0;
    until("the arrow again, by id", LOADED, || {
        recorder.saw(|e| *e == ControlEvent::CursorShape(CursorShape::Show { id: arrow_id }))
    })
    .await;
    assert_eq!(images().len(), 2, "a cached shape is not sent again");

    stream.send(ClientMessage::CursorShapeRequest { enabled: false })?;
    until("the pointer back in the video", LOADED, || {
        !session.client_cursor.active()
    })
    .await;
    stream.stop();
    until("sessions released", LOADED, || {
        h.h.sessions.lock().unwrap().idle()
    })
    .await;
    h.finish().await
}

/// Frame Warp 2 (0x5543 to 0x5546 and the meta 0x5549): the host offers it in serverinfo and
/// DESCRIBE, Nightjar's version 2 request turns it on and is answered with a model Nightjar
/// reads, and its stop turns it off. (An older Nightjar's version 1 request, which this
/// Nightjar cannot send, is ignored: `tests/host/tests/reconnect_tests/frame_warp_v2_tests.rs`.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nightjar_hears_the_frame_warp_model() -> Result<()> {
    use butterpollo_media_policy::frame_warp::{FrameWarpRequest, RequestV2};
    use nightjar_core::control::ClientMessage;
    let h = Harness::new(10_000, 1).await?;
    let client = device(&h, u32::MAX, &[])?;
    assert!(
        client
            .server_info(true)?
            .capabilities
            .has(Feature::FrameWarp)
    );
    let (_launch, session, stream, recorder) = stream(&h, &client).await?;
    assert!(
        stream
            .outcome()
            .host_control_messages
            .contains(&ptype::FRAME_WARP_REQUEST)
    );
    assert!(!session.frame_warp.enabled());
    stream.send(ClientMessage::FrameWarpRequest(RequestV2::new(
        FrameWarpRequest {
            enabled: true,
            controller: Some(0),
            mask_max_width: 1024,
            lens: true,
        },
    )))?;
    until("the host turns Frame Warp on", LOADED, || {
        session.frame_warp.enabled()
    })
    .await;
    until("the model at Nightjar", LOADED, || {
        recorder.saw(|e| matches!(e, ControlEvent::FrameWarpModel(_)))
    })
    .await;

    stream.send(ClientMessage::FrameWarpRequest(RequestV2::new(
        FrameWarpRequest {
            enabled: false,
            controller: None,
            mask_max_width: 0,
            lens: true,
        },
    )))?;
    until("the host turns Frame Warp off", LOADED, || {
        !session.frame_warp.enabled()
    })
    .await;
    stream.stop();
    until("sessions released", LOADED, || {
        h.h.sessions.lock().unwrap().idle()
    })
    .await;
    h.finish().await
}

/// Nightjar's `rlFecPercentage`: `auto` keeps the host's percentage until 0x5539 reports
/// arrive, then follows them up to `FEC_MAX_PERCENTAGE`; a fixed choice is capped by it; and
/// a launch without one keeps the host's own FEC.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nightjar_chooses_the_video_fec() -> Result<()> {
    let h = Harness::configured(
        10_000,
        1,
        "fec_percentage = 20
",
    )
    .await?;
    let client = device(&h, u32::MAX, &[])?;
    let caps = client.server_info(true)?.capabilities;
    assert_eq!(
        caps.availability(Feature::ClientFec),
        Availability::Supported
    );

    let (launch, session, stream, _) = stream_with(&h, &client, Some(FecChoice::Auto)).await?;
    assert_eq!(
        launch.options.get("rlFecPercentage").map(String::as_str),
        Some("auto")
    );
    assert!(session.client_fec.lock().unwrap().is_some());
    assert_eq!(
        session.fec_percent(),
        20,
        "the host's percentage until reports arrive"
    );
    let report = |seq, missing| FecReport {
        seq,
        data_shards: 900,
        parity_shards: 180,
        missing_shards: missing,
        unrecovered_blocks: 0,
        interval_ms: 1000,
    };
    let (_, used) = session.record_fec_report(&report(0, 0).encode()).unwrap();
    assert!(used);
    assert_eq!(
        session.video_fec_percent(false, 30_000),
        usize::from(butterpollo_media_policy::statistical_fec::MINIMUM_PERCENT),
        "no loss keeps the parity floor"
    );
    // Nightjar sends one a second; reports closer than this count without a
    // new estimate.
    tokio::time::sleep(butterpollo_media_policy::statistical_fec::MIN_ESTIMATE_INTERVAL).await;
    session.record_fec_report(&report(1, 540).encode()).unwrap();
    assert_eq!(
        session.video_fec_percent(true, 30_000),
        50,
        "25 % loss is capped"
    );
    assert!(
        session.record_fec_report(&report(1, 0).encode()).is_err(),
        "a repeat"
    );
    stream.stop();
    until("sessions released", LOADED, || {
        h.h.sessions.lock().unwrap().idle()
    })
    .await;

    let (_, session, stream, _) = stream_with(&h, &client, Some(FecChoice::Fixed(90))).await?;
    assert!(session.client_fec.lock().unwrap().is_none());
    assert_eq!(
        session.video_fec_percent(true, 30_000),
        50,
        "capped by FEC_MAX_PERCENTAGE"
    );
    let (_, used) = session.record_fec_report(&report(0, 108).encode()).unwrap();
    assert!(!used, "only auto acts on reports");
    stream.stop();
    until("sessions released", LOADED, || {
        h.h.sessions.lock().unwrap().idle()
    })
    .await;

    let (launch, session, stream, _) = stream_with(&h, &client, None).await?;
    assert!(!launch.options.contains_key("rlFecPercentage"));
    assert!(session.client_fec.lock().unwrap().is_none());
    assert_eq!(session.adaptive_fec.lock().unwrap().ceiling(), 20);
    stream.stop();
    until("sessions released", LOADED, || {
        h.h.sessions.lock().unwrap().idle()
    })
    .await;
    h.finish().await
}

/// Nightjar's `rlRateChange=1` adds 0x553C to its stream's DESCRIBE; its requests reach the
/// host, which caps a bitrate by `max_bitrate` and answers each with its serial. Nightjar
/// asks in the unit of its bitrate setting, which its launch carried: the answer names that
/// number and the encoder gets its share of it, as at launch (80 %, less 512 Kbps of audio
/// and 500 of overhead). A launch without `rlRateChange` is neither offered 0x553C nor sends
/// a request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nightjar_changes_the_bitrate_mid_stream() -> Result<()> {
    use butterpollo_media_policy::rate_change::{
        Outcome, RateChangeRequest, RateChangeResult, Reason,
    };
    use nightjar_core::control::{ClientMessage, SendError};
    let h = Harness::configured(
        10_000,
        1,
        "max_bitrate = 30000
",
    )
    .await?;
    let client = device(&h, u32::MAX, &[])?;
    let answer = |recorder: &Recorder, serial: u32| {
        recorder.seen(|s| {
            s.events.iter().find_map(|event| match event {
                ControlEvent::RateChange(result) if result.serial == serial => Some(*result),
                _ => None,
            })
        })
    };
    let ask = |serial, bitrate_kbps| {
        ClientMessage::RateChange(RateChangeRequest {
            serial,
            bitrate_kbps,
            fps_millihz: 0,
        })
    };

    let (launch, _, stream, recorder) = stream(&h, &client).await?;
    assert!(!launch.options.contains_key("rlRateChange"));
    assert!(
        !stream
            .outcome()
            .host_control_messages
            .contains(&ptype::RATE_CHANGE)
    );
    assert_eq!(stream.send(ask(1, 12_000)), Err(SendError::Unsupported));
    stream.stop();
    until("sessions released", LOADED, || {
        h.h.sessions.lock().unwrap().idle()
    })
    .await;
    assert!(recorder.seen(|s| {
        s.events
            .iter()
            .all(|e| !matches!(e, ControlEvent::RateChange(_)))
    }));

    let (launch, session, stream, recorder) = stream_asking(
        &h,
        &client,
        LaunchRequest {
            rate_change: true,
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(
        launch.options.get("rlRateChange").map(String::as_str),
        Some("1")
    );
    assert!(
        stream
            .outcome()
            .host_control_messages
            .contains(&ptype::RATE_CHANGE)
    );
    // The engine's query asks for nothing: the answer names the stream's bitrate in the
    // launch's unit and changes neither the encoder's bitrate nor the host's controllers.
    // Who owns the bitrate only: whether the client took the adaptive controller over, and
    // whether the link-rate and congestion controllers still read what was sent (a takeover
    // stops both); not their budgets, which move on their own.
    let controllers = || {
        (
            session.adaptive_bitrate.lock().unwrap().snapshot()["owner"] == "client",
            session.link_sent.live(),
            session.transport_sent.live(),
        )
    };
    let encoder = session.bitrate.load(Ordering::Acquire);
    let before = controllers();
    stream.send(ClientMessage::RateChange(RateChangeRequest::query(7)))?;
    until("the host's answer to the query", LOADED, || {
        answer(&recorder, 7).is_some()
    })
    .await;
    let known = answer(&recorder, 7).unwrap();
    assert_eq!(
        (known.bitrate, known.fps, known.fps_millihz),
        (
            Outcome::UNCHANGED,
            Outcome::refused(Reason::Invalid),
            60_000
        )
    );
    assert!(
        known.bitrate_kbps > encoder,
        "{known:?}: the launch's unit, not the encoder's {encoder}"
    );
    assert_eq!(session.bitrate.load(Ordering::Acquire), encoder);
    assert_eq!(controllers(), before, "no takeover");
    // Its number sent back keeps the encoder where it is, and that change is a takeover.
    stream.send(ask(8, known.bitrate_kbps))?;
    until("the host's answer", LOADED, || {
        answer(&recorder, 8).is_some()
    })
    .await;
    assert_eq!(
        answer(&recorder, 8).map(|a| (a.bitrate, a.bitrate_kbps)),
        Some((Outcome::APPLIED, known.bitrate_kbps))
    );
    assert_eq!(session.bitrate.load(Ordering::Acquire), encoder);
    assert_ne!(controllers(), before, "the change took the bitrate over");
    stream.send(ask(1, 50_000))?;
    until("the host's capped answer", LOADED, || {
        answer(&recorder, 1).is_some()
    })
    .await;
    assert_eq!(
        answer(&recorder, 1),
        Some(RateChangeResult {
            serial: 1,
            bitrate_kbps: 30_000,
            fps_millihz: 60_000,
            bitrate: Outcome::clamped(Reason::HostLimit),
            fps: Outcome::UNCHANGED,
        })
    );
    assert_ne!(session.config.configured_bitrate_kbps, 0);
    assert_eq!(session.bitrate.load(Ordering::Acquire), 22_988);
    stream.send(ask(2, 12_000))?;
    until("the host's answer", LOADED, || {
        answer(&recorder, 2).is_some()
    })
    .await;
    let applied = answer(&recorder, 2).unwrap();
    assert_eq!(
        (applied.bitrate, applied.bitrate_kbps),
        (Outcome::APPLIED, 12_000)
    );
    assert_eq!(session.bitrate.load(Ordering::Acquire), 8_588);
    stream.stop();
    until("sessions released", LOADED, || {
        h.h.sessions.lock().unwrap().idle()
    })
    .await;
    h.finish().await
}

/// Nightjar roams to 127.0.0.2: PROBE, CONFIRM and MIGRATED against the host's roaming table
/// move control and video, and the host's termination reaches Nightjar at the new address.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nightjar_roams_to_a_new_address_and_keeps_its_stream() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let client = device(&h, u32::MAX, &[])?;
    let (launch, session, stream, recorder) = stream(&h, &client).await?;
    assert_eq!(session.client_ip(), IpAddr::from([127, 0, 0, 1]));
    assert_eq!(session.roams(), 0);

    stream.roam_to(Some(IpAddr::from(MOVED)))?;
    until("Nightjar roamed", LOADED, || {
        recorder.saw(|e| matches!(e, ControlEvent::Roam(RoamEvent::Roamed { .. })))
    })
    .await;
    assert_eq!(session.roams(), 1);
    assert_eq!(session.client_ip(), IpAddr::from(MOVED));
    // The roam drops the video peer until Nightjar's first ping on the new path.
    until("video to the new address", LOADED, || {
        h.media
            .as_ref()
            .unwrap()
            .peers
            .lock()
            .unwrap()
            .get(&launch.id, false)
            .is_some_and(|peer| peer.ip() == IpAddr::from(MOVED))
    })
    .await;
    let frames = recorder.frames();
    until("video at Nightjar after the roam", LOADED, || {
        recorder.frames() > frames + 2
    })
    .await;

    // TEARDOWN as the host's console ends a stream; the sequence is past Nightjar's own.
    h.rtsp(&launch, 1000, "TEARDOWN", "").await?;
    until("the termination at Nightjar", LOADED, || {
        recorder.seen(|s| s.terminated.is_some())
    })
    .await;
    assert_eq!(session.termination_reason(), CLOSED);
    // ML_ERROR_GRACEFUL_TERMINATION.
    assert_eq!(recorder.seen(|s| s.terminated), Some(0));
    stream.stop();
    h.finish().await
}
