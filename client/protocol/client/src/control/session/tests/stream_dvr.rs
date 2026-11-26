//! Unit tests for Stream DVR (0x554A) on [`ControlSession`]: requests go out only to a host that
//! announced the id, and its states and clip results come back as events.

use super::*;
use rubylight_protocol::stream_dvr::{
    ClipResult, HostState, Message, Op, Outcome, Reason, Request, State,
};

fn state(request_id: u32) -> HostState {
    HostState {
        request_id,
        state: State::Armed,
        reason: Reason::None,
        ring_buffered_secs: 60,
        ring_capacity_secs: 60,
        recording_elapsed_secs: 0,
        quota_used_bytes: 1 << 30,
        quota_limit_bytes: 20 << 30,
    }
}

#[test]
fn stream_dvr_is_asked_and_answered_only_where_announced() {
    let mut s = ControlSession::new(
        HostCapabilities {
            control_messages: vec![0x5530, 0x554a],
            ..rubylight()
        },
        &KEY,
        SessionOptions::default(),
    );
    let t0 = Instant::now();
    s.start(t0);
    host_reads(&mut s);
    let request = Request {
        request_id: 7,
        op: Op::SaveClip,
        duration_secs: 0,
    };
    s.send(ClientMessage::StreamDvr(request)).unwrap();
    let sent = host_reads(&mut s);
    assert_eq!(sent.len(), 1);
    assert_eq!(
        (sent[0].0, sent[0].1, sent[0].2),
        (0, Delivery::Reliable, 0x554A)
    );
    assert_eq!(Message::decode(&sent[0].3), Some(Message::Request(request)));

    let clip = ClipResult {
        request_id: 7,
        outcome: Outcome::Saved,
        reason: Reason::None,
        duration_secs: 60,
        file_bytes: 84_000_000,
        clip_id: "20261126-153000-ab12".into(),
    };
    let mut seq = 0;
    let mut feed = |s: &mut ControlSession, message: Message| {
        seq += 1;
        let payload = message.encode().unwrap();
        s.handle_datagram(t0, &from_host(seq, 0x554A, &payload))
            .unwrap();
    };
    feed(&mut s, Message::State(state(0)));
    feed(&mut s, Message::Clip(clip.clone()));
    // The client's own kind and a frame that breaks the format are dropped.
    feed(&mut s, Message::Request(request));
    seq += 1;
    s.handle_datagram(t0, &from_host(seq, 0x554A, &[1, 2, 0]))
        .unwrap();
    assert_eq!(
        s.take_events(),
        [
            ControlEvent::StreamDvrState(state(0)),
            ControlEvent::StreamDvrClip(clip),
        ]
    );

    // A host that did not announce 0x554A is neither asked nor heard.
    let mut bare = session(SessionOptions::default());
    bare.start(t0);
    host_reads(&mut bare);
    assert_eq!(
        bare.send(ClientMessage::StreamDvr(request)),
        Err(SendError::Unsupported)
    );
    let payload = Message::State(state(0)).encode().unwrap();
    bare.handle_datagram(t0, &from_host(1, 0x554A, &payload))
        .unwrap();
    assert!(bare.take_events().is_empty());
}

#[test]
fn an_unknown_state_reads_as_unsupported_and_an_unknown_reason_as_unknown() {
    let mut s = ControlSession::new(
        HostCapabilities {
            control_messages: vec![0x5530, 0x554a],
            ..rubylight()
        },
        &KEY,
        SessionOptions::default(),
    );
    let t0 = Instant::now();
    s.start(t0);
    host_reads(&mut s);
    let mut payload = Message::State(state(3)).encode().unwrap();
    payload[8] = 200;
    payload[9] = 201;
    s.handle_datagram(t0, &from_host(1, 0x554A, &payload))
        .unwrap();
    assert_eq!(
        s.take_events(),
        [ControlEvent::StreamDvrState(HostState {
            state: State::Unsupported,
            reason: Reason::Unknown,
            ..state(3)
        })]
    );
}
