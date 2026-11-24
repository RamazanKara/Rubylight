//! Unit tests for Rubylight announced control messages (USB, link notices, pipeline rates,
//! rate changes, text input, keyboard layout, clipboard sync and display caps).

use super::*;
use rubylight_protocol::control::{
    DisplayCaps, LinkNotice, LinkNoticeRequest, LinkReason, LinkState,
};

#[test]
fn usb_messages_need_the_hosts_announcement_both_ways() {
    use rubylight_protocol::usb::{UsbMessage, UsbReason, UsbState};
    let usb_host = || HostCapabilities {
        control_messages: vec![0x5536, 0x553A],
        ..rubylight()
    };
    let mut s = ControlSession::new(usb_host(), &KEY, SessionOptions::default());
    let t0 = Instant::now();
    s.start(t0);
    host_reads(&mut s);
    let detach = UsbMessage::Detach { request_id: 3 };
    s.send(ClientMessage::Usb(detach.clone())).unwrap();
    let sent = host_reads(&mut s);
    assert_eq!(sent.len(), 1);
    assert_eq!((sent[0].1, sent[0].2), (Delivery::Reliable, 0x553A));
    assert_eq!(UsbMessage::decode(&sent[0].3), Some(detach.clone()));

    let status = UsbMessage::Status {
        request_id: 3,
        state: UsbState::Detached,
        reason: UsbReason::SessionEnd,
    };
    let ready = UsbMessage::TunnelReady {
        request_id: 3,
        token: [9; 16],
    };
    let attach = UsbMessage::AttachRequest {
        request_id: 3,
        vid: 1,
        pid: 2,
        class: 3,
        speed: 2,
        busid: "1-1".into(),
        name: String::new(),
    };
    let mut seq = 0;
    let mut feed = |s: &mut ControlSession, payload: &[u8]| {
        seq += 1;
        s.handle_datagram(t0, &from_host(seq, 0x553A, payload))
            .unwrap();
    };
    for message in [&ready, &status, &detach, &attach] {
        feed(&mut s, &message.encode().unwrap());
    }
    // Runts and attach requests (the client's own kind) are dropped.
    feed(&mut s, &[1, 1, 0, 0]);
    assert_eq!(
        s.take_events(),
        [
            ControlEvent::Usb(ready.clone()),
            ControlEvent::Usb(status.clone()),
            ControlEvent::Usb(detach.clone()),
        ]
    );

    // A host that did not announce 0x553A is neither asked nor heard.
    let mut bare = session(SessionOptions::default());
    bare.start(t0);
    host_reads(&mut bare);
    assert_eq!(
        bare.send(ClientMessage::Usb(detach)),
        Err(SendError::Unsupported)
    );
    bare.handle_datagram(t0, &from_host(1, 0x553A, &status.encode().unwrap()))
        .unwrap();
    assert!(bare.take_events().is_empty());
}

#[test]
fn link_notices_are_requested_and_heard() {
    let mut s = session(SessionOptions::default());
    let t0 = Instant::now();
    s.start(t0);
    host_reads(&mut s);
    let limited = LinkNotice {
        state: LinkState::Limited,
        reason: LinkReason::Network,
        ceiling_kbps: 150_000,
        current_kbps: 75_000,
    };
    s.send(ClientMessage::LinkNoticeRequest(LinkNoticeRequest {
        enabled: true,
    }))
    .unwrap();
    let sent = host_reads(&mut s);
    assert_eq!(sent.len(), 1);
    assert_eq!(
        (sent[0].0, sent[0].1, sent[0].2),
        (0, Delivery::Reliable, 0x5538)
    );
    assert_eq!(
        LinkNoticeRequest::decode(&sent[0].3),
        Some(LinkNoticeRequest { enabled: true })
    );

    let mut seq = 0;
    let mut feed = |s: &mut ControlSession, payload: &[u8]| {
        seq += 1;
        s.handle_datagram(t0, &from_host(seq, 0x5538, payload))
            .unwrap();
    };
    feed(&mut s, &limited.encode());
    // Fields a newer host appends are skipped; nonsense, runts and giants are dropped.
    let mut longer = limited.encode().to_vec();
    longer.extend_from_slice(&[9; 8]);
    feed(&mut s, &longer);
    let mut full = limited.encode();
    full[1] = 0;
    feed(&mut s, &full);
    feed(&mut s, &limited.encode()[..11]);
    feed(&mut s, &[0; 300]);
    let mut zero = limited.encode();
    zero[8..12].fill(0);
    feed(&mut s, &zero);
    let cleared = LinkNotice {
        state: LinkState::Full,
        reason: LinkReason::None,
        ..limited
    };
    feed(&mut s, &cleared.encode());
    assert_eq!(
        s.take_events(),
        [
            ControlEvent::LinkNotice(limited),
            ControlEvent::LinkNotice(limited),
            ControlEvent::LinkNotice(cleared),
        ]
    );

    // A host that did not announce 0x5538 is not asked.
    let mut bare = ControlSession::new(
        HostCapabilities {
            control_messages: vec![0x5536],
            ..rubylight()
        },
        &KEY,
        SessionOptions::default(),
    );
    assert_eq!(
        bare.send(ClientMessage::LinkNoticeRequest(LinkNoticeRequest {
            enabled: true
        })),
        Err(SendError::Unsupported)
    );
}

#[test]
fn pipeline_rates_are_requested_and_heard() {
    use rubylight_protocol::pipeline_rates::{PipelineRatesReport, PipelineRatesRequest};
    let mut s = session(SessionOptions::default());
    let t0 = Instant::now();
    s.start(t0);
    host_reads(&mut s);
    let request = PipelineRatesRequest {
        enabled: true,
        interval_ms: 1000,
    };
    s.send(ClientMessage::PipelineRatesRequest(request)).unwrap();
    let sent = host_reads(&mut s);
    assert_eq!(sent.len(), 1);
    assert_eq!((sent[0].0, sent[0].1, sent[0].2), (0, Delivery::Reliable, 0x553D));
    assert_eq!(PipelineRatesRequest::decode(&sent[0].3), Some(request));

    let report = PipelineRatesReport {
        short_window: false,
        window_ms: 1000,
        source_mhz: 119_880,
        capture_mhz: 120_000,
        submit_mhz: 120_000,
        output_mhz: 120_000,
        repeat_mhz: 0,
    };
    let mut seq = 0;
    let mut feed = |s: &mut ControlSession, payload: &[u8]| {
        seq += 1;
        s.handle_datagram(t0, &from_host(seq, 0x553D, payload))
            .unwrap();
    };
    feed(&mut s, &report.encode());
    // Fields a newer host appends are skipped; runts, giants and empty windows are dropped.
    let mut longer = report.encode().to_vec();
    longer.extend_from_slice(&[9; 4]);
    feed(&mut s, &longer);
    feed(&mut s, &report.encode()[..27]);
    feed(&mut s, &[1; 300]);
    let mut empty = report.encode();
    empty[4..8].fill(0);
    feed(&mut s, &empty);
    assert_eq!(
        s.take_events(),
        [
            ControlEvent::PipelineRates(report),
            ControlEvent::PipelineRates(report),
        ]
    );

    // A host that did not announce 0x553D is not asked.
    let mut bare = ControlSession::new(
        HostCapabilities {
            control_messages: vec![0x5536],
            ..rubylight()
        },
        &KEY,
        SessionOptions::default(),
    );
    assert_eq!(
        bare.send(ClientMessage::PipelineRatesRequest(request)),
        Err(SendError::Unsupported)
    );
}

#[test]
fn rate_changes_are_asked_and_answered_only_where_announced() {
    use rubylight_protocol::rate_change::{
        Outcome, RateChangeRequest, RateChangeResult, Reason,
    };
    let mut s = ControlSession::new(
        HostCapabilities {
            control_messages: vec![0x5530, 0x553c],
            ..rubylight()
        },
        &KEY,
        SessionOptions::default(),
    );
    let t0 = Instant::now();
    s.start(t0);
    host_reads(&mut s);
    let request = RateChangeRequest {
        serial: 3,
        bitrate_kbps: 40_000,
        fps_millihz: 0,
    };
    s.send(ClientMessage::RateChange(request)).unwrap();
    let sent = host_reads(&mut s);
    assert_eq!(sent.len(), 1);
    assert_eq!((sent[0].0, sent[0].1, sent[0].2), (0, Delivery::Reliable, 0x553C));
    assert_eq!(RateChangeRequest::decode(&sent[0].3), Some(request));
    // A request that asks for nothing is the query for the stream's bitrate and rate: sent.
    let query = RateChangeRequest::query(4);
    s.send(ClientMessage::RateChange(query)).unwrap();
    let sent = host_reads(&mut s);
    assert_eq!(sent.len(), 1);
    assert_eq!((sent[0].1, sent[0].2), (Delivery::Reliable, 0x553C));
    assert_eq!(RateChangeRequest::decode(&sent[0].3), Some(query));
    // A rate past the protocol's limits is not sent.
    for invalid in [(40_000, 9_999), (0, 500_001)] {
        assert_eq!(
            s.send(ClientMessage::RateChange(RateChangeRequest {
                serial: 4,
                bitrate_kbps: invalid.0,
                fps_millihz: invalid.1,
            })),
            Err(SendError::Invalid)
        );
    }

    let result = RateChangeResult {
        serial: 3,
        bitrate_kbps: 30_000,
        fps_millihz: 60_000,
        bitrate: Outcome::clamped(Reason::HostLimit),
        fps: Outcome::UNCHANGED,
    };
    let mut seq = 0;
    let mut feed = |s: &mut ControlSession, payload: &[u8]| {
        seq += 1;
        s.handle_datagram(t0, &from_host(seq, 0x553C, payload))
            .unwrap();
    };
    feed(&mut s, &result.encode());
    // Fields a newer host appends are skipped; runts, giants and unknown statuses are
    // dropped.
    let mut longer = result.encode().to_vec();
    longer.extend_from_slice(&[9; 4]);
    feed(&mut s, &longer);
    feed(&mut s, &result.encode()[..19]);
    feed(&mut s, &[1; 300]);
    let mut unknown = result.encode();
    unknown[16] = 9;
    feed(&mut s, &unknown);
    assert_eq!(
        s.take_events(),
        [ControlEvent::RateChange(result), ControlEvent::RateChange(result)]
    );

    // A host that did not announce 0x553C, as for a launch without `rlRateChange=1`, is
    // neither asked nor heard.
    let mut bare = session(SessionOptions::default());
    bare.start(t0);
    host_reads(&mut bare);
    assert_eq!(
        bare.send(ClientMessage::RateChange(request)),
        Err(SendError::Unsupported)
    );
    bare.handle_datagram(t0, &from_host(1, 0x553C, &result.encode()))
        .unwrap();
    assert!(bare.take_events().is_empty());
}

#[test]
fn text_input_is_requested_and_heard_and_the_layout_reported() {
    use rubylight_protocol::keyboard_layout::{KeyboardLayout, LayoutSource};
    use rubylight_protocol::text_input::{
        InputType, NoticeKind, TextInputNotice, TextInputRequest,
    };
    let mut s = ControlSession::new(
        HostCapabilities {
            control_messages: vec![0x5530, 0x553b, 0x553e],
            ..rubylight()
        },
        &KEY,
        SessionOptions::default(),
    );
    let t0 = Instant::now();
    s.start(t0);
    host_reads(&mut s);
    let request = TextInputRequest {
        notices: true,
        surrounding_text: false,
        element_ids: false,
        max_text: 0,
    };
    let layout = KeyboardLayout {
        source: LayoutSource::Bcp47,
        klid: 0,
        tag: "de-DE".into(),
    };
    s.send(ClientMessage::TextInputRequest(request)).unwrap();
    s.send(ClientMessage::KeyboardLayout(layout.clone()))
        .unwrap();
    let sent = host_reads(&mut s);
    assert_eq!(sent.len(), 2);
    assert_eq!((sent[0].1, sent[0].2), (Delivery::Reliable, 0x553b));
    assert_eq!(TextInputRequest::decode(&sent[0].3), Some(request));
    assert_eq!((sent[1].1, sent[1].2), (Delivery::Reliable, 0x553e));
    assert_eq!(KeyboardLayout::decode(&sent[1].3), Some(layout));
    // A tag that cannot be sent is refused before it goes out.
    assert_eq!(
        s.send(ClientMessage::KeyboardLayout(KeyboardLayout {
            source: LayoutSource::Bcp47,
            klid: 0,
            tag: "x".repeat(65),
        })),
        Err(SendError::Invalid)
    );

    let raise = TextInputNotice {
        kind: NoticeKind::Raise,
        input_type: InputType::Email,
        sensitive: false,
        no_autocorrect: true,
        no_autocapitalize: true,
        enter_action: false,
        serial: 1,
        selection_start: 0,
        selection_end: 0,
        text: String::new(),
        element: 0,
    };
    let lower = TextInputNotice {
        kind: NoticeKind::Lower,
        input_type: InputType::Text,
        serial: 2,
        ..raise.clone()
    };
    let mut seq = 0;
    let mut feed = |s: &mut ControlSession, payload: &[u8]| {
        seq += 1;
        s.handle_datagram(t0, &from_host(seq, 0x553b, payload))
            .unwrap();
    };
    feed(&mut s, &raise.encode().unwrap());
    // Runts and broken notices are dropped.
    feed(&mut s, &raise.encode().unwrap()[..15]);
    let mut broken = raise.encode().unwrap();
    broken[1] = 9;
    feed(&mut s, &broken);
    feed(&mut s, &lower.encode().unwrap());
    assert_eq!(
        s.take_events(),
        [
            ControlEvent::TextInput(raise),
            ControlEvent::TextInput(lower)
        ]
    );

    // A host that did not announce them is not asked, and hears no layout.
    let mut bare = session(SessionOptions::default());
    assert_eq!(
        bare.send(ClientMessage::TextInputRequest(request)),
        Err(SendError::Unsupported)
    );
    assert_eq!(
        bare.send(ClientMessage::KeyboardLayout(KeyboardLayout {
            source: LayoutSource::Klid,
            klid: 0x0407,
            tag: String::new(),
        })),
        Err(SendError::Unsupported)
    );
}

#[test]
fn clipboard_frames_go_both_ways_only_once_announced() {
    use crate::control::clipboard_sync::{ClipboardItem, ClipboardSync, Local};
    use rubylight_protocol::clipboard_sync::{Frame, KIND_TEXT};
    let mut s = ControlSession::new(
        HostCapabilities {
            control_messages: vec![0x5530, 0x5508],
            ..rubylight()
        },
        &KEY,
        SessionOptions::default(),
    );
    let t0 = Instant::now();
    s.start(t0);
    host_reads(&mut s);
    let mut sync = ClipboardSync::new(false);
    s.send(sync.hello()).unwrap();
    let Local::Send(copy) = sync.local(ClipboardItem::Text("hi".into()), t0) else {
        panic!();
    };
    s.send(copy).unwrap();
    let sent = host_reads(&mut s);
    assert_eq!(sent.len(), 2);
    assert_eq!(
        (sent[0].0, sent[0].1, sent[0].2),
        (0, Delivery::Reliable, 0x5508)
    );
    assert!(Frame::decode(&sent[0].3).unwrap().is_hello());
    let frame = Frame::decode(&sent[1].3).unwrap();
    assert_eq!(
        (frame.kind, frame.payload.as_slice()),
        (KIND_TEXT, &b"hi"[..])
    );

    let pc = Frame {
        kind: KIND_TEXT,
        token: 9,
        payload: b"pc".to_vec(),
    };
    s.handle_datagram(t0, &from_host(1, 0x5508, &pc.encode().unwrap()))
        .unwrap();
    s.handle_datagram(t0, &from_host(2, 0x5508, &[2, 1, 0]))
        .unwrap();
    assert_eq!(
        s.take_events(),
        [
            ControlEvent::ClipboardSync(pc.clone()),
            ControlEvent::ClipboardSyncRejected(3)
        ]
    );

    // A host that did not offer it, as one the launch did not ask, neither hears nor
    // sends frames.
    let mut bare = session(SessionOptions::default());
    assert_eq!(bare.send(sync.hello()), Err(SendError::Unsupported));
    bare.handle_datagram(t0, &from_host(1, 0x5508, &pc.encode().unwrap()))
        .unwrap();
    assert_eq!(bare.take_events(), []);
}

#[test]
fn rubylight_messages_need_announcement() {
    let mut s = session(SessionOptions::default());
    let caps = DisplayCaps {
        hdr: true,
        max_centinits: 100_000,
        max_average_centinits: 50_000,
        min_decimillinits: 5,
    };
    s.send(ClientMessage::DisplayCaps(caps)).unwrap();
    let sent = host_reads(&mut s);
    assert_eq!(sent[0].2, 0x5531);
    assert_eq!(DisplayCaps::decode(&sent[0].3), Some(caps));

    let mut bare = ControlSession::new(
        HostCapabilities {
            control_messages: vec![],
            ..rubylight()
        },
        &KEY,
        SessionOptions::default(),
    );
    assert_eq!(
        bare.send(ClientMessage::DisplayCaps(caps)),
        Err(SendError::Unsupported)
    );
    assert_eq!(
        bare.send(ClientMessage::CursorShapeRequest { enabled: true }),
        Err(SendError::Unsupported)
    );
    assert!(!bare.cursor_shapes_requested());

    let mut gfe = ControlSession::new(
        HostCapabilities {
            app_version: [7, 1, 431, 0],
            ..rubylight()
        },
        &KEY,
        SessionOptions::default(),
    );
    assert_eq!(
        gfe.send(ClientMessage::ServerCommand(vec![1])),
        Err(SendError::Unsupported)
    );
    gfe.send(ClientMessage::RequestIdrFrame).unwrap();
    let out = gfe.poll_transmit().unwrap();
    assert_eq!((out.channel, out.delivery), (0, Delivery::Reliable));

    let seq = s.send_clock_sync_probe(42).unwrap();
    assert_eq!(seq, 1);
    let sent = host_reads(&mut s);
    assert_eq!(sent[0].1, Delivery::Unsequenced);
    assert_eq!(
        ClockSyncRequest::decode(&sent[0].3),
        Some(ClockSyncRequest {
            sequence: 1,
            client_us: 42
        })
    );
}
