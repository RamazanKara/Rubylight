//! The opt-in host-side timing record (`BUTTERPOLLO_TIMING_RECORD=<dir>`): one CSV row per
//! sent picture with every host stage stamp as integer microseconds on the media clock,
//! for receivers that cannot speak the 0x5536 and 0x5537 control messages. This module holds
//! the pure parts, the row, the file text and the bounded hand-off. The writer thread that
//! does the file IO is in the host.
//!
//! The send path only ever calls [`Sink::record`], which never blocks: a full queue drops
//! the row and counts it.
use crate::performance::Stamps;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
    mpsc::{Receiver, SyncSender, TrySendError, sync_channel},
};
use std::time::Instant;

/// The environment variable that turns the record on: a directory for `host-<stream_id>.csv`.
pub const ENV: &str = "BUTTERPOLLO_TIMING_RECORD";
/// Rows the writer may fall behind by before new ones are dropped.
pub const QUEUE_ROWS: usize = 4096;
/// The column names, in row order.
pub const COLUMNS: &str = "wire_frame,rtp_us,present_us,acquired_us,published_us,claimed_us,submitted_us,encoded_us,packetized_us,prepared_us,first_sent_us,last_sent_us,first_unit_us,early_blocks";

/// What the header comments say about the stream.
#[derive(Clone, Debug, PartialEq)]
pub struct Meta {
    pub codec: &'static str,
    /// `wgc` or `dda`.
    pub capture: String,
    /// Where the RTP present stamp comes from: `etw` (the stamper refines it) or `capture`.
    pub present_source: &'static str,
    /// QPC in milliseconds at the media epoch, as `clock_ms()` in `moonlight_client.c`.
    pub media_epoch_qpc_ms: f64,
}

/// The stream's codec as the header names it (`Negotiated::codec`).
pub fn codec_name(codec: u8) -> &'static str {
    match codec {
        1 => "hevc",
        2 => "av1",
        3 => "pyrowave",
        _ => "h264",
    }
}

/// Where the presented time comes from: the ETW present tracker, or else the capture time
/// (so `game_to_present` is not a measurement).
pub fn present_source(etw: bool) -> &'static str {
    if etw { "etw" } else { "capture" }
}

/// The file's first lines, ending with the column names.
pub fn header(meta: &Meta) -> String {
    format!(
        "# rubylight timing record 1\n# media_epoch_qpc_ms={:.3}\n# codec={} capture={} present_source={}\n{COLUMNS}\n",
        meta.media_epoch_qpc_ms, meta.codec, meta.capture, meta.present_source
    )
}

/// The file's last line, after the rows.
pub fn footer(dropped: u64) -> String {
    format!("# dropped={dropped}\n")
}

/// One sent picture. Every time is in microseconds on the media clock, negative before its
/// start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub wire_frame: u32,
    /// The RTP timestamp on the wire as the receiver reads it, `rtp * 1000 / 90`.
    pub rtp_us: u64,
    /// `Stamps`, in order: presented, acquired, published, claimed, submitted, encoded,
    /// packetized, prepared, first sent, last sent.
    pub points: [i64; 10],
    /// A streamed frame: when its first unit was in hand (the `encoded` point is its last),
    /// and the blocks sent before the last unit. Empty columns for any other frame.
    pub first_unit_us: Option<i64>,
    pub early_blocks: Option<u32>,
}

impl Row {
    pub fn new(wire_frame: u32, rtp_timestamp: u32, stamps: &Stamps, epoch: Instant) -> Self {
        let at = |point: Instant| match point.checked_duration_since(epoch) {
            Some(after) => i64::try_from(after.as_micros()).unwrap_or(i64::MAX),
            None => -i64::try_from(epoch.duration_since(point).as_micros()).unwrap_or(i64::MAX),
        };
        Self {
            first_unit_us: stamps.streamed.map(|streamed| at(streamed.first_unit)),
            early_blocks: stamps.streamed.map(|streamed| streamed.early_blocks),
            wire_frame,
            rtp_us: u64::from(rtp_timestamp) * 1000 / 90,
            points: [
                at(stamps.presented),
                at(stamps.acquired),
                at(stamps.published),
                at(stamps.claimed),
                at(stamps.submitted),
                at(stamps.encoded),
                at(stamps.packetized),
                at(stamps.prepared),
                at(stamps.first_sent),
                at(stamps.last_sent),
            ],
        }
    }

    /// The row's CSV line, with its newline.
    pub fn line(&self) -> String {
        let mut line = format!("{},{}", self.wire_frame, self.rtp_us);
        for point in self.points {
            line.push(',');
            line.push_str(&point.to_string());
        }
        // Empty unless the frame was streamed.
        line.push(',');
        if let Some(first_unit) = self.first_unit_us {
            line.push_str(&first_unit.to_string());
        }
        line.push(',');
        if let Some(early) = self.early_blocks {
            line.push_str(&early.to_string());
        }
        line.push('\n');
        line
    }
}

/// The send path's end of the record. Cloning the `Arc` shares the queue; the writer ends
/// and writes its footer once every sink is gone.
pub struct Sink {
    rows: SyncSender<Row>,
    dropped: Arc<AtomicU64>,
}

/// A sink and the writer's end of its queue, holding at most `capacity` rows.
pub fn channel(capacity: usize) -> (Sink, Receiver<Row>) {
    let (rows, receiver) = sync_channel(capacity);
    (
        Sink {
            rows,
            dropped: Arc::default(),
        },
        receiver,
    )
}

impl Sink {
    /// Queues a row without blocking. A full queue, or a writer that is gone, drops the row
    /// and counts it.
    pub fn record(&self, row: Row) {
        match self.rows.try_send(row) {
            Ok(()) => {}
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Rows dropped so far.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// The count the writer reads for the footer.
    pub fn dropped_counter(&self) -> Arc<AtomicU64> {
        self.dropped.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_present_source_is_etw_only_with_a_tracker() {
        assert_eq!(present_source(true), "etw");
        assert_eq!(present_source(false), "capture");
    }

    fn stamps(at: Instant) -> Stamps {
        let us = |n: u64| at + Duration::from_micros(n);
        Stamps {
            presented: us(100),
            acquired: us(300),
            published: us(400),
            claimed: us(500),
            submitted: us(700),
            encoded: us(3_200),
            packetized: us(3_300),
            prepared: us(3_400),
            first_sent: us(3_450),
            last_sent: us(3_650),
            streamed: None,
        }
    }

    #[test]
    fn a_row_holds_every_stamp_point_on_the_media_clock() {
        let epoch = Instant::now();
        // 90 kHz ticks: 1.5 s is 135000, and 135001 rounds down to 1500011 us.
        let row = Row::new(41, 135_001, &stamps(epoch), epoch);
        assert_eq!(row.rtp_us, 1_500_011);
        assert_eq!(
            row.points,
            [100, 300, 400, 500, 700, 3_200, 3_300, 3_400, 3_450, 3_650]
        );
        assert_eq!(
            row.line(),
            "41,1500011,100,300,400,500,700,3200,3300,3400,3450,3650,,\n"
        );
        assert_eq!((row.first_unit_us, row.early_blocks), (None, None));
        // A point before the epoch is negative, not clamped.
        let later = epoch + Duration::from_micros(250);
        let row = Row::new(1, 0, &stamps(epoch), later);
        assert_eq!(row.points[0], -150);
        assert_eq!(row.points[9], 3_400);
    }

    #[test]
    fn a_streamed_frame_adds_its_first_unit_and_early_blocks() {
        let epoch = Instant::now();
        // The last unit was in hand at 3200 us; the first block left at 1450, so the
        // encoder was still running for 1750 us of the frame's time on the wire.
        let mut streamed = stamps(epoch);
        streamed.first_sent = epoch + Duration::from_micros(1_450);
        streamed.prepared = streamed.first_sent;
        let streamed = streamed.with_streamed(epoch + Duration::from_micros(900), 3);
        assert_eq!(streamed.overlap(), Some(1_750));
        let row = Row::new(42, 0, &streamed, epoch);
        assert_eq!((row.first_unit_us, row.early_blocks), (Some(900), Some(3)));
        assert_eq!(
            row.line(),
            "42,0,100,300,400,500,700,3200,3300,1450,1450,3650,900,3\n"
        );
        assert_eq!(
            row.line().matches(',').count(),
            COLUMNS.matches(',').count()
        );
        // Before the epoch the first unit is negative like any point.
        let row = Row::new(42, 0, &streamed, epoch + Duration::from_micros(1_000));
        assert_eq!(row.first_unit_us, Some(-100));
        // A frame that is not streamed has no overlap.
        assert_eq!(stamps(epoch).overlap(), None);
    }

    #[test]
    fn the_file_text_names_the_stream_and_the_columns() {
        let meta = Meta {
            codec: codec_name(3),
            capture: "wgc".into(),
            present_source: "etw",
            media_epoch_qpc_ms: 123456.7894,
        };
        assert_eq!(
            header(&meta),
            format!(
                "# rubylight timing record 1\n# media_epoch_qpc_ms=123456.789\n# codec=pyrowave capture=wgc present_source=etw\n{COLUMNS}\n"
            )
        );
        assert_eq!(COLUMNS.split(',').count(), 14);
        assert_eq!(footer(7), "# dropped=7\n");
        assert_eq!(
            [0, 1, 2, 3, 9].map(codec_name),
            ["h264", "hevc", "av1", "pyrowave", "h264"]
        );
    }

    #[test]
    fn a_full_queue_drops_rows_and_counts_them_without_blocking() {
        let epoch = Instant::now();
        let (sink, receiver) = channel(4);
        let row = Row::new(1, 0, &stamps(epoch), epoch);
        let started = Instant::now();
        for _ in 0..10 {
            sink.record(row);
        }
        assert!(started.elapsed() < Duration::from_millis(500));
        assert_eq!(sink.dropped(), 6);
        assert_eq!(receiver.try_iter().count(), 4);
        // A writer that is gone drops too.
        drop(receiver);
        sink.record(row);
        assert_eq!(sink.dropped(), 7);
        assert_eq!(sink.dropped_counter().load(Ordering::Relaxed), 7);
    }
}
