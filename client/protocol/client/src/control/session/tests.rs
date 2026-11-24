//! Unit tests for [`ControlSession`] lifecycle, RFI/IDR video feedback, host events and
//! connection status.

mod extensions;
mod fec;

use super::*;
use crate::control::negotiation::encryption;
use rubylight_protocol::control::{
    ClockSyncReply, HostTiming, HostTimingExtra, HOST_TIMING_EXTENDED_MAX_RECORDS,
};
use rubylight_wire::packet;

const KEY: Key = *b"0011223344556677";

fn rubylight() -> HostCapabilities {
    HostCapabilities {
        app_version: [7, 1, 431, -1],
        feature_flags: 3,
        encryption_enabled: encryption::CONTROL_V2,
        control_messages: vec![
            0x5530, 0x5531, 0x5532, 0x5534, 0x5535, 0x5536, 0x5537, 0x5538, 0x553D, 0x5540,
        ],
        client_roaming: false,
    }
}

fn session(options: SessionOptions) -> ControlSession {
    ControlSession::new(rubylight(), &KEY, options)
}

/// Opens everything queued as the host would.
fn host_reads(session: &mut ControlSession) -> Vec<(u8, Delivery, u16, Vec<u8>)> {
    std::iter::from_fn(|| session.poll_transmit())
        .map(|out| {
            let (_, kind, payload) =
                packet::decrypt_control(&KEY, &out.datagram, true).unwrap();
            (out.channel, out.delivery, kind, payload)
        })
        .collect()
}

fn from_host(seq: u32, kind: u16, payload: &[u8]) -> Vec<u8> {
    packet::encrypted_control(&KEY, seq, kind, payload, true).unwrap()
}

#[test]
fn start_sends_start_messages_then_pings() {
    let mut s = session(SessionOptions::default());
    let t0 = Instant::now();
    assert_eq!(s.poll(t0), None, "nothing before start");
    s.start(t0);
    assert_eq!(s.poll(t0), Some(t0 + PERIODIC_PING_INTERVAL));
    let sent = host_reads(&mut s);
    let kinds: Vec<u16> = sent.iter().map(|m| m.2).collect();
    assert_eq!(kinds, [0x0302, 0x0307, 0x0200]);
    assert_eq!(
        sent[2],
        (0, Delivery::Reliable, 0x0200, vec![4, 0, 0, 0, 0, 0, 0, 0])
    );
    assert_eq!(
        s.poll(t0 + Duration::from_millis(50)),
        Some(t0 + PERIODIC_PING_INTERVAL)
    );
    assert!(host_reads(&mut s).is_empty());
    s.poll(t0 + PERIODIC_PING_INTERVAL);
    assert_eq!(host_reads(&mut s).len(), 1);
}

#[test]
fn rfi_batches_ranges_and_idr_supersedes_them() {
    let mut s = session(SessionOptions {
        reference_frame_invalidation: true,
        ..Default::default()
    });
    let t0 = Instant::now();
    s.start(t0);
    s.poll(t0);
    host_reads(&mut s);

    s.frame_loss(10, 12);
    s.frame_complete(13, true);
    s.frame_loss(14, 15);
    s.poll(t0);
    let sent = host_reads(&mut s);
    assert_eq!(sent.len(), 2);
    assert_eq!((sent[0].0, sent[0].2), (channel::URGENT, 0x0350));
    assert_eq!(&sent[0].3[..4], &13u32.to_le_bytes());
    assert_eq!(sent[1].2, 0x0301);
    assert_eq!(&sent[1].3[..4], &10u32.to_le_bytes());
    assert_eq!(&sent[1].3[8..12], &15u32.to_le_bytes());

    s.frame_loss(20, 21);
    s.request_idr_frame();
    s.poll(t0);
    let sent = host_reads(&mut s);
    assert_eq!(sent.iter().map(|m| m.2).collect::<Vec<_>>(), [0x0302]);

    for frame in 0..REFERENCE_FRAME_QUEUE_BOUND as u32 + 1 {
        s.frame_loss(frame, frame);
    }
    s.poll(t0);
    assert_eq!(
        host_reads(&mut s).iter().map(|m| m.2).collect::<Vec<_>>(),
        [0x0302]
    );
}

#[test]
fn without_rfi_loss_requests_idr_and_ltr_acks_are_refused() {
    let mut s = session(SessionOptions::default());
    let t0 = Instant::now();
    s.start(t0);
    s.poll(t0);
    host_reads(&mut s);
    s.frame_loss(3, 4);
    s.frame_complete(5, true);
    s.poll(t0);
    assert_eq!(
        host_reads(&mut s).iter().map(|m| m.2).collect::<Vec<_>>(),
        [0x0302]
    );
    assert_eq!(
        s.send(ClientMessage::LtrFrameAck { frame: 1 }),
        Err(SendError::Unsupported)
    );
}

#[test]
fn host_messages_become_events() {
    let mut s = session(SessionOptions::default());
    let t0 = Instant::now();
    s.start(t0);
    let mut seq = 0;
    let mut feed = |s: &mut ControlSession, kind, payload: &[u8]| {
        seq += 1;
        s.handle_datagram(t0, &from_host(seq, kind, payload))
            .unwrap();
    };
    feed(&mut s, 0x010b, &[0, 0, 0, 0, 1, 0, 2, 0, 3, 0]);
    feed(&mut s, 0x5500, &[1, 0, 4, 0, 5, 0]);
    let mut hdr = vec![1];
    hdr.extend_from_slice(&[0x10, 0x27].repeat(12));
    feed(&mut s, 0x010e, &hdr);
    let timing = HostTiming {
        frame: 5,
        host_us: 100,
        queue_us: 10,
        encode_us: 50,
        send_us: 20,
    };
    feed(&mut s, 0x5536, &HostTiming::encode_report(&[timing]));
    let reply = ClockSyncReply {
        sequence: 1,
        client_us: 42,
        host_receive_us: 1000,
        host_send_us: 1010,
    };
    feed(&mut s, 0x5537, &reply.encode());
    // Not requested yet: dropped.
    feed(&mut s, 0x5541, &[1, 0, 0, 0, 0, 0, 0, 0]);
    s.send(ClientMessage::CursorShapeRequest { enabled: true })
        .unwrap();
    feed(&mut s, 0x5541, &[1, 0, 0, 0, 0, 0, 0, 0]);
    let pcm = crate::control::haptics::encode(0, 9, &[1; 8]).unwrap();
    feed(&mut s, 0x5601, &pcm);
    feed(&mut s, 0x5533, &pcm);

    let events = s.take_events();
    assert_eq!(
        events[0],
        ControlEvent::Rumble {
            controller: 1,
            low_frequency: 2,
            high_frequency: 3
        }
    );
    assert_eq!(
        events[1],
        ControlEvent::RumbleTriggers {
            controller: 1,
            left: 4,
            right: 5
        }
    );
    let ControlEvent::HdrMode {
        enabled: true,
        metadata: Some(metadata),
    } = events[2]
    else {
        panic!("{:?}", events[2]);
    };
    assert_eq!(metadata.max_display_luminance, 10_000);
    assert!(s.hdr_enabled() && s.hdr_metadata().is_some());
    assert_eq!(events[3], ControlEvent::HostTiming(vec![(timing, None)]));
    assert_eq!(
        events[4],
        ControlEvent::ClockSync {
            reply,
            received: t0
        }
    );
    assert_eq!(
        events[5],
        ControlEvent::CursorShape(events::CursorShape::Hide)
    );
    assert!(matches!(&events[6], ControlEvent::ControllerHaptics(p) if p.sequence == 9));
    assert!(matches!(&events[7], ControlEvent::ControllerSpeaker(p) if p.frames() == 2));
    assert_eq!(events.len(), 8);
}

#[test]
fn steam_controller_messages_become_events_only_from_sunshine_hosts() {
    use rubylight_protocol::steam_controller::{
        SteamHaptic, SteamSettings, STEAM_SETTINGS_LOAD_DEFAULTS,
    };
    let haptic = SteamHaptic::new(2, &[0x81, 1, 2, 3]).unwrap();
    let settings = SteamSettings::new(2, STEAM_SETTINGS_LOAD_DEFAULTS, &[]).unwrap();
    let t0 = Instant::now();
    let feed = |s: &mut ControlSession| {
        s.handle_datagram(t0, &from_host(1, 0x5504, &haptic.encode()))
            .unwrap();
        s.handle_datagram(t0, &from_host(2, 0x5505, &settings.encode()))
            .unwrap();
    };

    let mut s = session(SessionOptions::default());
    s.start(t0);
    feed(&mut s);
    assert_eq!(
        s.take_events(),
        [
            ControlEvent::SteamHaptic(haptic),
            ControlEvent::SteamSettings(settings)
        ]
    );

    let mut gfe = ControlSession::new(
        HostCapabilities {
            app_version: [7, 1, 431, 0],
            ..rubylight()
        },
        &KEY,
        SessionOptions::default(),
    );
    gfe.start(t0);
    feed(&mut gfe);
    assert!(gfe.take_events().is_empty());
}

#[test]
fn extended_host_timing_reports_keep_their_extension() {
    let mut s = session(SessionOptions::default());
    let t0 = Instant::now();
    s.start(t0);
    let record = |frame| HostTiming {
        frame,
        host_us: 900,
        queue_us: 300,
        encode_us: 200,
        send_us: 100,
    };
    let extra = HostTimingExtra {
        presented_at_us: 5_000_000_123,
        capture_us: 100,
        convert_us: 150,
    };
    let full = vec![(record(7), extra); HOST_TIMING_EXTENDED_MAX_RECORDS];
    let report = HostTiming::encode_report_with(&full);
    assert_eq!(report.len(), 616);
    let mut seq = 0;
    let mut feed = |s: &mut ControlSession, payload: &[u8]| {
        seq += 1;
        s.handle_datagram(t0, &from_host(seq, 0x5536, payload))
            .unwrap();
    };
    feed(&mut s, &report);
    // A v1 report has no extension; one with capture and convert past queue loses it.
    feed(&mut s, &HostTiming::encode_report(&[record(8)]));
    let bad = HostTimingExtra {
        capture_us: 200,
        convert_us: 200,
        ..extra
    };
    feed(&mut s, &HostTiming::encode_report_with(&[(record(9), bad)]));
    // The widest report the decoder takes is 32 records with 16 extra bytes each (1156 bytes),
    // so a host's record cap can rise; past 17 records the extension is not used.
    let mut widest = HostTiming::encode_report(&[record(10); 32]);
    widest[2] = 16;
    widest.resize(4 + 32 * 36, 0);
    feed(&mut s, &widest);

    let events = s.take_events();
    let ControlEvent::HostTiming(records) = &events[0] else {
        panic!("{:?}", events[0]);
    };
    assert_eq!(records.len(), 17);
    assert!(records.iter().all(|r| *r == (record(7), Some(extra))));
    assert_eq!(events[1], ControlEvent::HostTiming(vec![(record(8), None)]));
    assert_eq!(events[2], ControlEvent::HostTiming(vec![(record(9), None)]));
    // Over 17 records the base records stay and the extension is dropped.
    let ControlEvent::HostTiming(records) = &events[3] else {
        panic!("{:?}", events[3]);
    };
    assert_eq!(records.len(), 32);
    assert!(records.iter().all(|r| *r == (record(10), None)));
}

#[test]
fn termination_ends_the_session_once() {
    let mut s = session(SessionOptions::default());
    let t0 = Instant::now();
    s.start(t0);
    s.frame_seen(1, t0);
    s.handle_datagram(t0, &from_host(1, 0x0109, &0x8003_0023u32.to_be_bytes()))
        .unwrap();
    assert_eq!(s.terminated(), Some(0));
    s.disconnected();
    assert_eq!(s.take_events(), [ControlEvent::Terminated { error: 0 }]);
    assert_eq!(
        s.send(ClientMessage::RequestIdrFrame),
        Err(SendError::Stopped)
    );
    assert_eq!(s.poll(t0), None);

    let mut early = session(SessionOptions::default());
    early
        .handle_datagram(t0, &from_host(1, 0x0109, &0x8003_0023u32.to_be_bytes()))
        .unwrap();
    assert_eq!(early.terminated(), Some(-102));
    let mut silent = session(SessionOptions::default());
    silent.disconnected();
    assert_eq!(
        silent.take_events(),
        [ControlEvent::Terminated { error: -1 }]
    );
}

#[test]
fn connection_status_follows_frame_loss() {
    let mut s = session(SessionOptions::default());
    let t0 = Instant::now();
    let ms = |n| t0 + Duration::from_millis(n);
    s.frame_seen(1, t0);
    s.frame_seen(2, ms(3100));
    // 40 % loss over the next interval.
    let mut frame = 2;
    for i in 0..100u32 {
        frame += 1;
        if i % 5 >= 2 {
            s.frame_complete(frame, false);
        }
        s.frame_seen(frame, ms(3100 + u64::from(i) * 30));
    }
    s.frame_seen(frame + 1, ms(6200));
    assert_eq!(
        s.take_events(),
        [ControlEvent::ConnectionStatus(ConnectionStatus::Poor)]
    );
    for i in 0..100u32 {
        frame += 1;
        s.frame_complete(frame, false);
        s.frame_seen(frame, ms(6200 + u64::from(i) * 30));
    }
    s.frame_seen(frame + 1, ms(9300));
    assert_eq!(
        s.take_events(),
        [ControlEvent::ConnectionStatus(ConnectionStatus::Okay)]
    );
}
