use super::*;
use crate::control::negotiation::encryption;
use butterpollo_core::packet;
use rubylight_protocol::control::{
    ClockSyncReply, DisplayCaps, HostTiming, HostTimingExtra, LinkNotice, LinkNoticeRequest,
    LinkReason, LinkState, HOST_TIMING_EXTENDED_MAX_RECORDS,
};
use rubylight_protocol::fec_report::FecReport;

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
fn fec_reports_ride_with_the_ping_and_are_bounded() {
    let mut s = session(SessionOptions::default());
    let t0 = Instant::now();
    s.start(t0);
    for frame in 0..10 {
        s.frame_fec_status(FrameFecStatus {
            frame_index: frame,
            multi_fec_block_count: 1,
            ..Default::default()
        });
    }
    s.poll(t0);
    let sent = host_reads(&mut s);
    let fec: Vec<_> = sent.iter().filter(|m| m.2 == 0x5502).collect();
    assert_eq!(fec.len(), FEC_STATUS_QUEUE_BOUND);
    assert!(fec.iter().all(|m| m.1 == Delivery::Unsequenced));
    assert_eq!(sent.last().unwrap().2, 0x0200);
}

/// Ten data and two parity shards of one frame, `missing` of them lost.
fn block(missing: u32, unrecovered: bool) -> FecBlock {
    FecBlock {
        frame: 1,
        block_count: 1,
        data_shards: 10,
        parity_shards: 2,
        arrived_shards: 12 - missing,
        missing_shards: if unrecovered { 0 } else { missing },
        unrecovered,
        ..FecBlock::default()
    }
}

#[test]
fn transport_feedback_goes_only_to_a_host_that_listed_it() {
    use rubylight_protocol::transport_feedback::TransportFeedback;
    let report = TransportFeedback {
        stride: 1,
        entries: vec![0, 10, rubylight_protocol::transport_feedback::NOT_RECEIVED],
        ..Default::default()
    };
    let t0 = Instant::now();
    let mut stock = session(SessionOptions::default());
    stock.start(t0);
    host_reads(&mut stock);
    stock.transport_feedback(report.clone());
    assert!(host_reads(&mut stock).is_empty());

    let mut s = ControlSession::new(
        HostCapabilities {
            control_messages: vec![0x5547],
            ..rubylight()
        },
        &KEY,
        SessionOptions::default(),
    );
    s.start(t0);
    host_reads(&mut s);
    s.transport_feedback(report.clone());
    let sent = host_reads(&mut s);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].1, Delivery::Unsequenced);
    assert_eq!(sent[0].2, 0x5547);
    assert_eq!(TransportFeedback::decode(&sent[0].3), Some(report.clone()));
    // A backlog drops reports instead of growing.
    for _ in 0..TRANSPORT_FEEDBACK_BACKLOG + 10 {
        s.transport_feedback(report.clone());
    }
    assert_eq!(host_reads(&mut s).len(), TRANSPORT_FEEDBACK_BACKLOG);
}

fn fec_reporting() -> ControlSession {
    ControlSession::new(
        HostCapabilities {
            control_messages: vec![0x5539],
            ..rubylight()
        },
        &KEY,
        SessionOptions::default(),
    )
}

fn reports(s: &mut ControlSession) -> Vec<(Delivery, FecReport)> {
    host_reads(s)
        .into_iter()
        .filter(|m| m.2 == 0x5539)
        .map(|m| (m.1, FecReport::decode(&m.3).unwrap()))
        .collect()
}

#[test]
fn a_block_that_could_not_be_recovered_is_reported_after_a_quarter_second() {
    let mut s = fec_reporting();
    let t0 = Instant::now();
    s.start(t0);
    host_reads(&mut s);
    let ms = |ms: u64| t0 + Duration::from_millis(ms);
    // A clean link reports once a second, on the ping's tick.
    s.fec_block(block(1, false), t0);
    for at in [100, 250, 500, 900] {
        s.poll(ms(at));
    }
    assert!(reports(&mut s).is_empty());
    s.poll(ms(1000));
    let clean = reports(&mut s);
    assert_eq!(clean.len(), 1);
    assert_eq!(clean[0].1.interval_ms, 1000);

    // A block lost 50 ms later goes at the first poll a quarter second after the last
    // report, between the ping's ticks too.
    s.fec_block(block(4, true), ms(1050));
    s.poll(ms(1100));
    s.poll(ms(1249));
    assert!(reports(&mut s).is_empty(), "never two within 250 ms");
    s.poll(ms(1255));
    let early = reports(&mut s);
    assert_eq!(early.len(), 1);
    assert_eq!(
        (early[0].1.seq, early[0].1.unrecovered_blocks, early[0].1.interval_ms),
        (1, 1, 255)
    );

    // Another loss right after waits out the quarter second again; then the cadence goes on
    // a second after the last report.
    s.fec_block(block(4, true), ms(1260));
    s.poll(ms(1300));
    s.poll(ms(1504));
    assert!(reports(&mut s).is_empty());
    s.poll(ms(1505));
    assert_eq!(reports(&mut s)[0].1.interval_ms, 250);
    s.fec_block(block(0, false), ms(1600));
    for at in [1700, 1800, 2000, 2400] {
        s.poll(ms(at));
    }
    assert!(reports(&mut s).is_empty());
    s.poll(ms(2600));
    assert_eq!(reports(&mut s)[0].1.interval_ms, 1095);
}

#[test]
fn a_lost_block_asks_the_control_thread_to_wake_for_its_early_report() {
    let mut s = fec_reporting();
    let t0 = Instant::now();
    s.start(t0);
    host_reads(&mut s);
    let ms = |ms: u64| t0 + Duration::from_millis(ms);
    s.poll(t0);
    s.fec_block(block(1, false), t0);
    s.fec_block(block(4, true), ms(50));
    s.poll(ms(100));
    // The ping at 200 ms is sooner than the quarter second the lost block waits for, so
    // the ping's tick and then the report's deadline are the wake-ups.
    assert_eq!(s.poll(ms(200)), Some(ms(250)));
    assert!(reports(&mut s).is_empty());
    assert_eq!(s.poll(ms(250)), Some(ms(300)));
    let lost = reports(&mut s);
    assert_eq!(lost.len(), 1);
    assert_eq!(lost[0].1.unrecovered_blocks, 1);
}

#[test]
fn the_fec_report_goes_once_a_second_to_a_host_that_announced_it() {
    let mut s = fec_reporting();
    let t0 = Instant::now();
    s.fec_block(block(1, false), t0);
    s.start(t0);
    host_reads(&mut s);
    s.fec_block(block(1, false), t0);
    s.fec_block(block(4, false), t0 + Duration::from_millis(10));
    s.poll(t0 + Duration::from_millis(900));
    assert!(reports(&mut s).is_empty(), "not before a second");
    let first = t0 + Duration::from_millis(1000);
    s.poll(first);
    let expected = FecReport {
        seq: 0,
        data_shards: 20,
        parity_shards: 4,
        missing_shards: 5,
        unrecovered_blocks: 0,
        interval_ms: 1000,
    };
    assert_eq!(
        reports(&mut s),
        [(Delivery::Unreliable, expected)],
        "blocks before start are not counted"
    );

    // A second without video sends a report without shards, so the host's reports stay
    // fresh; the next one follows with seq 2.
    s.poll(first + Duration::from_millis(1000));
    let idle = reports(&mut s);
    assert_eq!(idle.len(), 1);
    assert_eq!(
        (
            idle[0].1.seq,
            idle[0].1.data_shards,
            idle[0].1.parity_shards
        ),
        (1, 0, 0)
    );
    let second = first + Duration::from_millis(2100);
    s.fec_block(block(0, false), second - Duration::from_millis(500));
    s.poll(second);
    let sent = reports(&mut s);
    assert_eq!(sent.len(), 1);
    assert_eq!((sent[0].1.seq, sent[0].1.interval_ms), (2, 1100));

    // A stall past 5 s can't be timed: dropped, and counting starts again.
    s.fec_block(block(0, false), second);
    s.poll(second + Duration::from_millis(6000));
    assert!(reports(&mut s).is_empty());
}

#[test]
fn no_fec_report_goes_to_a_host_that_did_not_announce_it() {
    let mut s = session(SessionOptions::default());
    let t0 = Instant::now();
    s.start(t0);
    s.fec_block(block(3, true), t0);
    s.poll(t0 + Duration::from_secs(1));
    assert!(host_reads(&mut s).iter().all(|m| m.2 != 0x5539));
}

/// A final FEC status of block `block` of `frame`: `data` data shards, `received` arrived.
fn final_status(frame: u32, block: u8, data: u16, received: u16) -> FrameFecStatus {
    FrameFecStatus {
        frame_index: frame,
        total_data_packets: data,
        received_data_packets: received,
        multi_fec_block_index: block,
        multi_fec_block_count: 2,
        ..Default::default()
    }
}

#[test]
fn the_fec_summary_goes_once_a_second_in_v_plus_form() {
    // A Foundation Sunshine host: no a=x-rl-control, FEC control from DESCRIBE.
    let foundation = HostCapabilities {
        control_messages: vec![],
        ..rubylight()
    };
    let mut s = ControlSession::new(
        foundation,
        &KEY,
        SessionOptions {
            fec_summaries: true,
            ..Default::default()
        },
    );
    let summaries = |s: &mut ControlSession| -> Vec<(u8, Delivery, Vec<u8>)> {
        host_reads(s)
            .into_iter()
            .filter(|m| m.2 == 0x5510)
            .map(|m| (m.0, m.1, m.3))
            .collect()
    };
    let t0 = Instant::now();
    s.start(t0);
    // The first tick closes an empty second: sequence 1 is used but nothing goes out.
    s.poll(t0);
    assert!(summaries(&mut s).is_empty());
    // Recovered: the final status counts, the completion after it is the same block.
    s.frame_fec_status(final_status(7, 0, 10, 8));
    let mut done = block(2, false);
    done.frame = 7;
    s.fec_block(done, t0);
    // Clean: no status, the completion counts whole.
    done.block_index = 1;
    s.fec_block(done, t0);
    // Given up: counted at its status, never again at the completion.
    s.frame_fec_status(final_status(8, 0, 10, 3));
    let mut lost = block(7, true);
    lost.frame = 8;
    s.fec_block(lost, t0);
    s.poll(t0 + Duration::from_millis(900));
    assert!(summaries(&mut s).is_empty(), "not before a second");
    s.poll(t0 + Duration::from_millis(1000));
    // seq 2, 30 data shards, 9 missing: BE32s on the generic channel, unreliable.
    assert_eq!(
        summaries(&mut s),
        [(
            channel::GENERIC,
            Delivery::Unreliable,
            vec![0, 0, 0, 2, 0, 0, 0, 30, 0, 0, 0, 9]
        )]
    );
    // A quiet second sends nothing; the next one says 4.
    s.poll(t0 + Duration::from_millis(2000));
    assert!(summaries(&mut s).is_empty());
    s.frame_fec_status(final_status(9, 0, 5, 5));
    s.poll(t0 + Duration::from_millis(3000));
    assert_eq!(summaries(&mut s)[0].2, [0, 0, 0, 4, 0, 0, 0, 5, 0, 0, 0, 0]);
}

#[test]
fn no_fec_summary_goes_unless_the_handshake_turned_it_on() {
    let mut s = session(SessionOptions::default());
    let t0 = Instant::now();
    s.start(t0);
    s.frame_fec_status(final_status(1, 0, 10, 2));
    s.fec_block(block(0, false), t0);
    for second in 0..3 {
        s.poll(t0 + Duration::from_secs(second));
    }
    assert!(host_reads(&mut s).iter().all(|m| m.2 != 0x5510));
}

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
    // Nothing asked, or a rate past the protocol's limits, is not sent.
    for invalid in [(0, 0), (40_000, 9_999), (0, 500_001)] {
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
    use super::super::clipboard_sync::{ClipboardItem, ClipboardSync, Local};
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
