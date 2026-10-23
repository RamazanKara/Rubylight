//! The host half of the timing CSV (docs: Rubylight's docs/timing-record.md, section 2): nine
//! columns appended to every row, in microseconds on the host's media clock, so a report script
//! can join the row with the host's own record. A column is empty where its source is missing.

use std::collections::{BTreeMap, VecDeque};

use crate::session;
use rubylight_protocol::control::{HostTiming, HostTimingExtra};
use rubylight_protocol::present_clock::ClockOffset;

/// The appended header columns, after the receiver's own.
pub const HEADER: &str = "rtp_host_us,host_present_us,host_acquired_us,host_submitted_us,\
host_encoded_us,host_first_sent_us,host_last_sent_us,clock_offset_us,clock_rtt_us";

/// A report arrives after the frame's last packet left the host, so a row waits this long (in
/// microseconds) for it before it is written without one.
pub const ROW_DELAY_US: u64 = 250_000;

/// Reports kept for frames that have not been written yet.
const MAX_REPORTS: usize = 1024;

/// The RTP timestamp of the frame's first packet in host microseconds, `rtp * 1000 / 90` rounded
/// down as the host's record and moonlight-common-c's `presentationTimeUs` compute it.
pub fn rtp_host_us(rtp_timestamp: u32) -> u64 {
    u64::from(rtp_timestamp) * 1000 / 90
}

fn field(value: Option<u64>) -> String {
    value.map_or_else(String::new, |v| v.to_string())
}

/// The nine fields (no leading comma). The stamps come from the frame's extended report:
/// acquired is the capture present plus `capture_us`, submitted that plus the record's
/// `queue_us`, encoded `encode_us` later, last sent the capture present plus `host_us` and first
/// sent `send_us` before that. `clock` is the offset (`client = host + offset`) and round trip.
pub fn columns(
    rtp_timestamp: u32,
    report: Option<&(HostTiming, Option<HostTimingExtra>)>,
    clock: Option<(i64, i64)>,
) -> String {
    let mut stamps = [None; 6];
    if let Some((timing, Some(extra))) = report {
        let present = extra.presented_at_us;
        let submitted = present + u64::from(timing.queue_us);
        let last_sent = present + u64::from(timing.host_us);
        stamps = [
            Some(present),
            Some(present + u64::from(extra.capture_us)),
            Some(submitted),
            Some(submitted + u64::from(timing.encode_us)),
            Some(last_sent.saturating_sub(u64::from(timing.send_us))),
            Some(last_sent),
        ];
    }
    let mut out = field(Some(rtp_host_us(rtp_timestamp)));
    for stamp in stamps {
        out.push(',');
        out.push_str(&field(stamp));
    }
    match clock {
        Some((offset, rtt)) => out.push_str(&format!(",{offset},{rtt}")),
        None => out.push_str(",,"),
    }
    out
}

/// A row waiting for its report.
pub struct PendingRow {
    pub number: u32,
    /// The row's own fields.
    pub line: String,
    pub rtp_timestamp: u32,
    pub clock: Option<(i64, i64)>,
    pub due_us: u64,
}

/// What the control thread learned (reports, clock sync) and the rows waiting for it.
#[derive(Default)]
pub struct Record {
    reports: BTreeMap<u32, (HostTiming, Option<HostTimingExtra>)>,
    pub offset: ClockOffset,
    pending: VecDeque<PendingRow>,
}

impl Record {
    pub fn add_reports(&mut self, records: Vec<(HostTiming, Option<HostTimingExtra>)>) {
        for (timing, extra) in records {
            self.reports.insert(timing.frame, (timing, extra));
        }
        while self.reports.len() > MAX_REPORTS {
            self.reports.pop_first();
        }
    }

    /// The clock offset and round trip at receiver time `local_us`, when clock sync has an exchange.
    pub fn clock(&self, local_us: u64) -> Option<(i64, i64)> {
        Some((
            self.offset.offset_at(local_us as i64)?,
            self.offset.rtt_us()?,
        ))
    }

    /// Queues a row for `write_due`.
    pub fn push(&mut self, row: PendingRow) {
        self.pending.push_back(row);
    }

    /// Complete rows (with their host columns) whose wait is over at `now_us`; all of them when
    /// `now_us` is `u64::MAX`.
    pub fn take_due(&mut self, now_us: u64) -> Vec<String> {
        let mut rows = Vec::new();
        while self.pending.front().is_some_and(|row| row.due_us <= now_us) {
            let row = self.pending.pop_front().expect("front row");
            let report = self.reports.remove(&row.number);
            rows.push(format!(
                "{},{}",
                row.line,
                columns(row.rtp_timestamp, report.as_ref(), row.clock)
            ));
        }
        rows
    }
}

/// Microseconds on the receiver's clock (`session::now_us`) at which `received` happened.
pub fn local_us_at(received: std::time::Instant) -> u64 {
    session::now_us().saturating_sub(received.elapsed().as_micros() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rubylight_protocol::control::ClockSyncReply;

    fn timing(frame: u32) -> HostTiming {
        HostTiming {
            frame,
            host_us: 900,
            queue_us: 300,
            encode_us: 200,
            send_us: 100,
        }
    }

    const EXTRA: HostTimingExtra = HostTimingExtra {
        presented_at_us: 1_000_000,
        capture_us: 100,
        convert_us: 150,
    };

    #[test]
    fn the_header_keeps_the_receivers_columns_and_appends_nine() {
        assert_eq!(HEADER.split(',').count(), 9);
        assert!(HEADER.starts_with("rtp_host_us,"));
        assert!(HEADER.ends_with(",clock_offset_us,clock_rtt_us"));
    }

    #[test]
    fn stamps_come_from_the_extended_report_and_the_offset() {
        let report = (timing(5), Some(EXTRA));
        assert_eq!(
            columns(9_000, Some(&report), Some((-3_955, 90))),
            // rtp 9000 ticks is exactly 100000 us; acquired 1000100, submitted 1000300,
            // encoded 1000500, last sent 1000900, first sent 1000800.
            "100000,1000000,1000100,1000300,1000500,1000800,1000900,-3955,90"
        );
    }

    #[test]
    fn rtp_rounds_down_like_the_host() {
        assert_eq!(rtp_host_us(1), 11);
        assert_eq!(rtp_host_us(8), 88);
        assert_eq!(rtp_host_us(u32::MAX), 47_721_858_833);
    }

    #[test]
    fn a_v1_report_or_no_clock_sync_leaves_the_columns_empty() {
        let v1 = (timing(5), None);
        assert_eq!(columns(90, Some(&v1), None), "1000,,,,,,,,");
        assert_eq!(columns(90, None, None), "1000,,,,,,,,");
        let report = (timing(5), Some(EXTRA));
        assert_eq!(
            columns(90, Some(&report), None),
            "1000,1000000,1000100,1000300,1000500,1000800,1000900,,"
        );
    }

    #[test]
    fn rows_wait_for_their_report_and_clock_is_read_when_the_row_is_made() {
        let mut record = Record::default();
        record.offset.add(
            &ClockSyncReply {
                sequence: 1,
                client_us: 1_000,
                host_receive_us: 5_000,
                host_send_us: 5_010,
            },
            1_100,
        );
        assert_eq!(record.clock(1_050), Some((-3_955, 90)));
        record.push(PendingRow {
            number: 5,
            line: "5,x".into(),
            rtp_timestamp: 90,
            clock: record.clock(1_050),
            due_us: 300_000,
        });
        record.push(PendingRow {
            number: 6,
            line: "6,y".into(),
            rtp_timestamp: 180,
            clock: None,
            due_us: 400_000,
        });
        record.add_reports(vec![(timing(5), Some(EXTRA))]);
        assert!(record.take_due(299_999).is_empty());
        assert_eq!(
            record.take_due(300_000),
            ["5,x,1000,1000000,1000100,1000300,1000500,1000800,1000900,-3955,90"]
        );
        // The report was consumed; a frame without one is written empty at the end.
        assert_eq!(record.take_due(u64::MAX), ["6,y,2000,,,,,,,,"]);
    }
}
