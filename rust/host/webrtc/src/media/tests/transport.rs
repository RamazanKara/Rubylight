//! Tests for ICE outages, spoofed datagrams, channel reliability, backlog and worker commands.

use super::super::*;
use super::*;
use crate::sdp::{AUDIO_PT, VIDEO_PT};
use anyhow::anyhow;
use std::sync::atomic::{AtomicUsize, Ordering};
use str0m::IceConnectionState;
use str0m::channel::Reliability;

/// A STUN binding request (RFC 8489) with only a USERNAME: no integrity, so no real peer.
fn forged_stun(username: &str) -> Vec<u8> {
    let mut attr = username.as_bytes().to_vec();
    let length = attr.len() as u16;
    attr.resize(attr.len().div_ceil(4) * 4, 0);
    let mut message = vec![0, 1];
    message.extend(((attr.len() + 4) as u16).to_be_bytes());
    message.extend([0x21, 0x12, 0xa4, 0x42]);
    message.extend([7u8; 12]);
    message.extend([0, 6]);
    message.extend(length.to_be_bytes());
    message.extend(attr);
    message
}

#[test]
fn audio_goes_out_unpaced_beside_a_paced_keyframe() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    link.video(h264_frame(true, 60_000), 0, true);
    link.audio(filler(160, 1), 0, 0);
    // str0m hands a frame over across a few polls; collect what the engine releases.
    let mut out = Vec::new();
    for ms in 0..20 {
        link.host
            .advance(link.now + Duration::from_millis(ms))
            .unwrap();
        out.extend(link.host.take_outgoing());
    }
    let by_pt = |pt: u8| -> Vec<&Outgoing> {
        out.iter()
            .filter(|o| is_media_rtp(&o.data) && o.data[1] & 0x7f == pt)
            .collect()
    };
    let (video, audio) = (by_pt(VIDEO_PT), by_pt(AUDIO_PT));
    assert!(
        video.len() > 20 && audio.len() == 1,
        "{} {}",
        video.len(),
        audio.len()
    );
    assert!(video.iter().all(|o| o.paced), "video waits for the pacer");
    assert!(audio.iter().all(|o| !o.paced), "audio does not");
    // Through a queue still holding the keyframe, the audio leaves in the first batch.
    let mut queue = SendQueue::new(capped(12_000), 1_000, PERIOD, false, link.now);
    for o in out {
        queue.push(o);
    }
    let first = queue.next_batch(link.now);
    assert!(
        first
            .packets
            .iter()
            .any(|o| !o.paced && o.data[1] & 0x7f == AUDIO_PT)
    );
    assert!(first.paced_packets <= 2, "{}", first.paced_packets);
}

#[test]
fn foreign_and_spoofed_datagrams_cannot_end_or_hijack_the_session() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    let client: SocketAddr = CLIENT.parse().unwrap();
    let attacker: SocketAddr = "203.0.113.9:4444".parse().unwrap();
    let rtp = {
        let mut p = vec![0x80, 96, 0, 1, 0, 0, 0, 1, 0xde, 0xad, 0xbe, 0xef];
        p.extend(filler(200, 1));
        p
    };
    let rtcp = vec![0x80, 201, 0, 1, 1, 2, 3, 4];
    let dtls = [
        vec![22, 0xfe, 0xfd, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4],
        filler(4, 1),
    ]
    .concat();
    let own = format!("{}:abcd", link.host_ufrag);
    let junk: Vec<Vec<u8>> = vec![
        rtp,
        rtcp,
        dtls,
        vec![0xff; 3000],
        vec![],
        vec![0],
        forged_stun("nobody:else"),
        forged_stun(&own),
    ];
    let now = link.now;
    for from in [attacker, client] {
        for data in &junk {
            // Whatever it is called, it is neither fatal nor a reason to stop.
            link.host.handle_datagram(now, from, data);
        }
    }
    // Foreign sources are refused outright, apart from STUN carrying our own ufrag.
    for data in &junk[..7] {
        assert_eq!(
            link.host.handle_datagram(now, attacker, data),
            Datagram::Rejected,
            "{:?}",
            &data[..data.len().min(8)]
        );
    }
    link.host.advance(link.now).unwrap();
    let out = link.host.take_outgoing();
    assert!(
        out.iter().all(|o| o.destination == client),
        "nothing is sent to the stranger: {:?}",
        out.iter().map(|o| o.destination).collect::<Vec<_>>()
    );
    assert!(link.host.is_connected() && !link.host.is_closed());
    let (rejected, _) = link.host.dropped_datagrams();
    assert!(rejected >= 14, "{rejected}");

    // The real client still gets its picture, to the address it nominated.
    link.video(h264_frame(true, 5_000), 0, true);
    link.run(Duration::from_millis(40));
    assert!(link.media.iter().any(|m| m.pt == 96), "video still flows");
    assert!(
        !link.host_events.contains(&TransportEvent::Closed)
            && !link.host_events.contains(&TransportEvent::Disconnected),
        "{:?}",
        link.host_events
    );
}

#[test]
fn an_ice_outage_is_surfaced_and_ends_the_session_after_the_timeout() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    link.video(h264_frame(true, 2_000), 0, true);
    link.run(Duration::from_millis(20));
    link.host_events.clear();
    link.idr.clear();
    link.partition = true;
    let started = link.now;
    while !link.host.is_disconnected() {
        link.step(Duration::from_millis(100));
        assert!(
            link.now - started < Duration::from_secs(60),
            "ICE never noticed the outage"
        );
    }
    let went_down = link.now;
    assert!(link.host_events.contains(&TransportEvent::Disconnected));
    assert!(!link.host.is_connected() && !link.host.is_closed());
    // While down, nothing is written, so the next frames wait for a keyframe.
    let frame = VideoFrame {
        data: h264_frame(true, 1_000).into(),
        capture: link.at(0),
        keyframe: true,
    };
    assert!(!link.host.write_video(link.now, frame).unwrap());

    while !link.host.is_closed() {
        link.step(Duration::from_millis(50));
        assert!(
            link.now - went_down < DISCONNECT_TIMEOUT * 2,
            "never closed"
        );
    }
    let waited = link.now - went_down;
    assert!(
        waited >= DISCONNECT_TIMEOUT - Duration::from_millis(200)
            && waited <= DISCONNECT_TIMEOUT + Duration::from_millis(300),
        "{waited:?}"
    );
}

#[test]
fn an_outage_that_ends_in_time_resumes_with_a_keyframe_request() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    link.partition = true;
    let started = link.now;
    while !link.host.is_disconnected() {
        link.step(Duration::from_millis(100));
        assert!(link.now - started < Duration::from_secs(60));
    }
    link.host_events.clear();
    link.idr.clear();
    link.partition = false;
    for _ in 0..25 {
        link.step(Duration::from_millis(100));
        if link.host.is_connected() {
            break;
        }
    }
    assert!(link.host.is_connected(), "{:?}", link.host_events);
    assert!(link.host_events.contains(&TransportEvent::Reconnected));
    assert!(link.idr.contains(&IdrReason::Reconnected), "{:?}", link.idr);
}

#[test]
fn a_session_outlives_five_seconds_of_a_lost_path() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    link.partition = true;
    let started = link.now;
    while !link.host.is_disconnected() {
        link.step(Duration::from_millis(100));
        assert!(link.now - started < Duration::from_secs(60));
    }
    // The drop lasts 5 s more: the old 3 s grace ended the session here. (The simulated
    // client's ICE has stopped checking by then, so the return is shown below with the ICE
    // states alone; a browser keeps checking until it reports `failed`.)
    let went_down = link.now;
    while link.now - went_down < Duration::from_secs(5) {
        link.step(Duration::from_millis(100));
    }
    assert!(!link.host.is_closed(), "{:?}", link.host_events);
    assert!(link.host.is_disconnected());
}

#[test]
fn a_path_back_after_five_seconds_resumes_with_one_keyframe() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    link.collect_host();
    link.host_events.clear();
    link.idr.clear();
    ice(&mut link, IceConnectionState::Disconnected);
    link.now += Duration::from_secs(5);
    link.host.advance(link.now).unwrap();
    assert!(!link.host.is_closed());
    ice(&mut link, IceConnectionState::Completed);
    link.collect_host();
    assert!(!link.host.is_disconnected() && !link.host.is_closed());
    assert_eq!(
        link.host_events,
        [TransportEvent::Disconnected, TransportEvent::Reconnected]
    );
    assert_eq!(link.idr, [IdrReason::Reconnected]);
}

#[test]
fn a_channel_closed_by_the_client_is_reported_and_stops_sends() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    link.authenticated.store(true, Ordering::Release);
    let input = link.client_opens("input");
    link.run(Duration::from_millis(60));
    assert!(
        link.host_events
            .contains(&TransportEvent::ChannelOpen("input".into()))
    );
    assert!(link.host.send_control(b"x", false).unwrap());
    for label in ["control", "input"] {
        let id = if label == "input" {
            input
        } else {
            link.client_channel(label)
        };
        link.client.direct_api().close_data_channel(id);
        link.run(Duration::from_millis(60));
        assert!(
            link.host_events.contains(&TransportEvent::ChannelClosed {
                label: label.into()
            }),
            "{label}: {:?}",
            link.host_events
        );
    }
    assert!(
        !link.host.send_control(b"x", false).unwrap(),
        "a closed channel takes nothing"
    );
    // The gamepad channel is untouched.
    assert!(link.host.send_gamepad(&[1], true).unwrap());
}

#[test]
fn the_input_channel_must_be_ordered_and_reliable() {
    use str0m::channel::ChannelConfig;
    let cases: [(&str, bool, Reliability, bool); 4] = [
        ("ordered and reliable", true, Reliability::Reliable, true),
        ("unordered", false, Reliability::Reliable, false),
        (
            "unreliable",
            true,
            Reliability::MaxRetransmits { retransmits: 0 },
            false,
        ),
        (
            "lossy and unordered",
            false,
            Reliability::MaxPacketLifetime { lifetime: 50 },
            false,
        ),
    ];
    for (name, ordered, reliability, accepted) in cases {
        let mut link = Link::connected(spec(VideoCodec::H264, false));
        link.authenticated.store(true, Ordering::Release);
        let mut api = link.client.sdp_api();
        api.add_channel_with_config(ChannelConfig {
            label: "input".into(),
            ordered,
            reliability,
            ..Default::default()
        });
        assert!(api.apply().is_none());
        link.run(Duration::from_millis(60));
        let open = link
            .host_events
            .contains(&TransportEvent::ChannelOpen("input".into()));
        let rejected = link
            .host_events
            .contains(&TransportEvent::ChannelRejected("input".into()));
        assert_eq!((open, rejected), (accepted, !accepted), "{name}");
    }
}

#[test]
fn messages_that_cannot_be_sent_are_counted() {
    let (handle, _rx, _idr) = detached(TransportOptions::default());
    let shared = &handle.shared;
    note_channel_send(shared, "control", Ok(true));
    note_channel_send(shared, "control", Ok(false));
    note_channel_send(shared, "gamepad", Ok(false));
    note_channel_send(shared, "gamepad", Err(anyhow!("channel write failed")));
    let stats = handle.stats();
    assert_eq!((stats.channel_sent, stats.channel_dropped), (1, 3));
    // Before the channels open, the engine itself says no rather than pretending.
    let mut link = Link::new(spec(VideoCodec::H264, false));
    assert!(!link.host.send_control(b"x", false).unwrap());
    assert!(!link.host.send_gamepad(&[1], true).unwrap());
}

#[test]
fn no_frame_but_a_keyframe_leaves_before_the_first_one() {
    let (handle, rx, _idr) = detached(TransportOptions::default());
    assert_eq!(handle.submit_video(frame(false)), Submit::Dropped);
    assert_eq!(handle.stats().video_dropped, 1);
    assert_eq!(handle.submit_video(frame(true)), Submit::Queued);
    assert_eq!(handle.submit_video(frame(false)), Submit::Queued);
    assert_eq!(rx.try_iter().count(), 2);
}

#[test]
fn stale_deltas_after_a_drop_are_discarded_by_the_thread() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    let (handle, _rx, _idr) = detached(TransportOptions::default());
    let shared = &handle.shared;
    shared.pending_video.store(3, Ordering::Release);
    let command = |keyframe| Command::Video(frame_of(h264_frame(keyframe, 800), keyframe));
    handle_command(&mut link.host, shared, command(false));
    let stats = handle.stats();
    assert_eq!((stats.video_frames, stats.video_dropped), (0, 1));
    handle_command(&mut link.host, shared, command(true));
    assert!(!shared.need_keyframe.load(Ordering::Acquire));
    handle_command(&mut link.host, shared, command(false));
    let stats = handle.stats();
    assert_eq!((stats.video_frames, stats.video_dropped), (2, 1));
}

#[test]
fn a_pacer_pile_up_drops_the_backlog_and_asks_for_a_keyframe() {
    let (handle, _rx, idr) = detached(TransportOptions::default());
    let shared = &handle.shared;
    shared.need_keyframe.store(false, Ordering::Release);
    let now = Instant::now();
    let mut queue = SendQueue::new(capped(8_000), 5_000, PERIOD, false, now);
    queue.note_frame(10_000);
    for _ in 0..30 {
        queue.push(rtp_packet(1100));
    }
    enforce_backlog(&mut queue, shared);
    assert!(idr.lock().unwrap().is_empty(), "30 KB is no backlog yet");
    for _ in 0..300 {
        queue.push(rtp_packet(1100));
    }
    enforce_backlog(&mut queue, shared);
    assert_eq!(idr.lock().unwrap().as_slice(), [IdrReason::PacerBacklog]);
    assert!(shared.need_keyframe.load(Ordering::Acquire));
    assert_eq!(handle.stats().paced_dropped, 330);
    assert!(queue.is_empty());
}

#[test]
fn a_datagram_burst_beyond_the_queue_is_dropped_not_buffered() {
    let (handle, rx, _idr) = detached(TransportOptions {
        datagram_queue: 3,
        ..TransportOptions::default()
    });
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    let target = socket.local_addr().unwrap();
    let (tx, shared) = (handle.tx.clone(), handle.shared.clone());
    let thread = std::thread::spawn(move || receive_loop(socket, tx, shared));
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    for i in 0..10u8 {
        sender.send_to(&[i; 40], target).unwrap();
    }
    wait_for("ten datagrams", || handle.stats().datagrams_received == 10);
    let stats = handle.stats();
    assert_eq!(stats.datagrams_dropped, 7, "{stats:?}");
    assert_eq!(rx.try_iter().count(), 3, "only the first three are queued");
    handle.shared.stop.store(true, Ordering::Release);
    thread.join().unwrap();
}

#[test]
fn checking_after_an_outage_is_not_back_and_does_not_reset_the_timeout() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    link.collect_host();
    link.host_events.clear();
    link.idr.clear();
    ice(&mut link, IceConnectionState::Disconnected);
    let down_at = link.host.disconnected_since.expect("down");
    link.now += Duration::from_secs(1);
    ice(&mut link, IceConnectionState::Checking);
    assert_eq!(
        link.host.disconnected_since,
        Some(down_at),
        "the clock runs on"
    );
    assert!(link.host.is_disconnected());
    link.collect_host();
    assert_eq!(link.host_events, [TransportEvent::Disconnected]);
    assert!(link.idr.is_empty(), "{:?}", link.idr);
    // Only a usable pair is back, and it asks for exactly one keyframe.
    ice(&mut link, IceConnectionState::Completed);
    ice(&mut link, IceConnectionState::Connected);
    link.collect_host();
    assert!(!link.host.is_disconnected());
    assert_eq!(
        link.host_events,
        [TransportEvent::Disconnected, TransportEvent::Reconnected]
    );
    assert_eq!(link.idr, [IdrReason::Reconnected]);
}

#[test]
fn checking_after_connecting_counts_as_down() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    link.collect_host();
    link.host_events.clear();
    // A Wi-Fi handover: Completed, then ICE goes looking for a new pair.
    ice(&mut link, IceConnectionState::Completed);
    ice(&mut link, IceConnectionState::Checking);
    assert!(link.host.is_disconnected());
    assert!(!link.host.is_connected());
    link.collect_host();
    assert_eq!(link.host_events, [TransportEvent::Disconnected]);
    // The timeout runs from the first sign of trouble and ends the session.
    link.now += DISCONNECT_TIMEOUT + Duration::from_millis(1);
    link.host.advance(link.now).unwrap();
    assert!(link.host.is_closed());
}

#[test]
fn checking_before_the_session_connected_is_not_an_outage() {
    let mut link = Link::new(spec(VideoCodec::H264, false));
    ice(&mut link, IceConnectionState::Checking);
    ice(&mut link, IceConnectionState::Disconnected);
    assert!(!link.host.is_disconnected());
    link.collect_host();
    assert!(link.host_events.is_empty());
}

#[test]
fn a_keyframe_cannot_overwrite_the_threads_demand_for_the_next_one() {
    let (handle, rx, _idr) = detached(TransportOptions::default());
    let shared = handle.shared.clone();
    let processed = Arc::new(AtomicUsize::new(0));
    let thread = {
        let (shared, processed) = (shared.clone(), processed.clone());
        std::thread::spawn(move || {
            // The media thread, discarding every keyframe at once.
            while let Ok(Command::Video(_)) = rx.recv() {
                shared.pending_video.fetch_sub(1, Ordering::AcqRel);
                shared.need_keyframe.store(true, Ordering::Release);
                processed.fetch_add(1, Ordering::Release);
            }
        })
    };
    for i in 1..=20_000 {
        shared.need_keyframe.store(true, Ordering::Release);
        assert_eq!(handle.submit_video(frame(true)), Submit::Queued);
        while processed.load(Ordering::Acquire) < i {
            std::hint::spin_loop();
        }
        assert!(
            shared.need_keyframe.load(Ordering::Acquire),
            "round {i}: the clear overwrote the thread's demand"
        );
    }
    drop(handle);
    thread.join().unwrap();
}

#[test]
fn receive_errors_only_back_off_when_they_persist() {
    let mut consecutive = 0;
    for _ in 1..RECV_ERRORS_BEFORE_BACKOFF {
        assert_eq!(recv_error_backoff(&mut consecutive), None);
    }
    assert_eq!(
        recv_error_backoff(&mut consecutive),
        Some(RECV_ERROR_BACKOFF)
    );
    assert_eq!(
        recv_error_backoff(&mut consecutive),
        Some(RECV_ERROR_BACKOFF)
    );
    // A datagram (or a quiet timeout) in between starts the count again.
    consecutive = 0;
    assert_eq!(recv_error_backoff(&mut consecutive), None);
}

#[test]
fn worker_drain_commands_caps_each_pass_and_keeps_wake_armed() {
    let (handle, rx, _idr) = detached(TransportOptions::default());
    let shared = handle.shared.clone();
    let timer = butterpollo_win_graphics::timing::Timer::new().unwrap();
    let total = MAX_COMMANDS_PER_PASS + 16;
    for _ in 0..total {
        handle
            .tx
            .send(Command::Control {
                data: vec![1],
                binary: false,
            })
            .unwrap();
    }

    shared.wake.reset().unwrap();
    let mut handled = 0usize;
    assert!(drain_commands(&rx, &shared, |_| handled += 1));
    assert_eq!(handled, MAX_COMMANDS_PER_PASS);
    assert!(
        timer
            .until_or_signal(Instant::now() + Duration::from_millis(50), &shared.wake)
            .unwrap(),
        "capped pass must keep wake armed"
    );

    shared.wake.reset().unwrap();
    assert!(drain_commands(&rx, &shared, |_| handled += 1));
    assert_eq!(handled, total);
    assert!(
        !timer
            .until_or_signal(Instant::now() + Duration::from_millis(10), &shared.wake)
            .unwrap(),
        "uncapped pass should not re-arm wake"
    );

    handle.tx.send(Command::Stop).unwrap();
    assert!(!drain_commands(&rx, &shared, |_| {}));

    drop(handle);
    assert!(!drain_commands(&rx, &shared, |_| {}));
}
