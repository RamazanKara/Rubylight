//! Unit tests for FEC reporting, FEC summaries and transport feedback on [`ControlSession`].

use super::*;
use rubylight_protocol::fec_report::FecReport;

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
