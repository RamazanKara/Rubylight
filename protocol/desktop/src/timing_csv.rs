//! The client timing CSV (3.0 A8, measure only): one row per picture shown, in the format of
//! Nightjar Android's `nightjar-latency.csv` (`FrameLatencyStats.CSV_HEADER`), so Rubylight's
//! `rust/release/timing_report.py` joins it with the host record through the 0x5537 clock
//! offset (docs/timing-record.md, section 2).
//!
//! Off unless `NIGHTJAR_TIMING_CSV` names a file, or a folder that gets `nightjar-latency.csv`.
//! The UI thread hands each row to a bounded channel with `try_send` and never waits: a writer
//! thread formats and writes, holding every row for a short while so the host's report for the
//! frame (it follows the frame's last packet) can be joined. A full channel drops the row and
//! counts it in `csv_rows_lost`.
//!
//! Times are nanoseconds on the session clock (`session::now_us`, the clock 0x5537 offsets are
//! measured against, `client = host + offset`). Columns that exist only for MediaCodec (release,
//! callback, frame rates, the host stage milliseconds) are empty; the nine host columns come
//! from the extended 0x5536 report. Two desktop columns follow the host columns (a reader
//! that goes by name ignores them): `early` (0 decoded whole, 1 early decode, 2 early decode
//! fell back to the whole frame) and `first_block_ns` (when the first block of an early frame
//! was decoded; empty otherwise), for the early-decode comparison in docs/early-decode.md.
//!
//! Four columns follow for the beam race (docs/beam-racing.md): `slices_decoded` (slice decodes
//! done for the frame, the final decode included), `first_slice_ready_ns` and
//! `last_slice_ready_ns` (when the first slice's decode and the last one finished) and
//! `row_ordered` (1 when the frame's records arrived in the host's row order, 0 when not); all
//! empty when the beam race is off.
//! The seven present-scheduler columns Android appends to the host's (`sched_vsync_ns` to
//! `drop_reason`) are empty here, so a reader that goes by name sees the same header.
//!
//! Seven more desktop columns break the wait between decoding and showing a picture down
//! (docs/pyrowave-decode.md, "Where the time between decoded and shown goes"): `due_ns` (when the
//! pacer meant to present it), `present_begin_ns` (the presenter began drawing it), and, on
//! Windows, the swap chain's frame statistics read right after the present call:
//! `present_count` (the chain's count for this present), `stat_present_count`,
//! `stat_present_refresh` and `stat_sync_refresh` (DXGI_FRAME_STATISTICS' PresentCount,
//! PresentRefreshCount and SyncRefreshCount) and `stat_sync_ns` (SyncQPCTime on the session clock).

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use nightjar_core::session;
use nightjar_core::timing_record::{self as record, PendingRow, Record};
use rubylight_protocol::control::{HostTiming, HostTimingExtra};

use crate::stream::pyrowave::RaceReport;

/// The receiver's own columns, in `FrameLatencyStats.CSV_HEADER` order; the nine host columns
/// (`record::HEADER`) follow.
pub const OWN_COLUMNS: [&str; 32] = [
    "frame_number", "pts_us", "receive_ns", "decoder_input_ns", "decoder_output_ns",
    "render_ns", "release_ns", "status", "receive_to_input_ms", "input_to_output_ms", "output_to_render_ms",
    "receive_to_render_ms", "csv_rows_lost", "host_processing_ms",
    "decode_to_present_ms", "receive_to_present_ms",
    "decode_to_release_ms", "decode_interval_ms", "release_interval_ms",
    "fps_sample_ns", "received_fps", "released_fps", "shown_fps", "present_drops", "enqueue_ns", "queue_wait_ms",
    "host_present_to_sent_ms", "host_queue_ms", "host_encode_ms", "host_send_ms",
    "callback_received_ns", "callback_delay_ms",
];

/// The Android present scheduler's columns after the host's, in `FrameLatencyStats.CSV_HEADER`
/// order: the desktop has no such scheduler, so they are always empty.
pub const SCHEDULER_COLUMNS: [&str; 7] = [
    "sched_vsync_ns", "sched_hold_ms", "sched_late", "queue_depth", "residency_excess_ms", "in_flight", "drop_reason",
];

/// The desktop's own columns after those.
pub const EXTRA_COLUMNS: [&str; 13] = [
    "early", "first_block_ns", "due_ns", "present_begin_ns", "present_count", "stat_present_count", "stat_present_refresh",
    "stat_sync_refresh", "stat_sync_ns", "slices_decoded", "first_slice_ready_ns", "last_slice_ready_ns", "row_ordered",
];

pub fn header() -> String {
    format!("{},{},{},{}\n", OWN_COLUMNS.join(","), record::HEADER, SCHEDULER_COLUMNS.join(","), EXTRA_COLUMNS.join(","))
}

/// Rows waiting for the writer.
const QUEUE: usize = 4096;

/// What the UI thread knows when a picture has been presented. Times are µs on the session
/// clock, 0 when unknown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Row {
    pub number: u32,
    pub rtp: u32,
    /// The frame's first packet arrived.
    pub receive_us: u64,
    /// The frame was complete and queued for decoding.
    pub enqueue_us: u64,
    pub input_us: u64,
    /// Decoding finished.
    pub output_us: u64,
    /// The picture was handed to the display.
    pub render_us: u64,
    /// The host's processing latency in 1/10 ms (0 when not reported).
    pub host_latency: u16,
    /// Pictures replaced before they were shown, since the stream began.
    pub drops: u64,
    /// The clock offset (`client = host + offset`) and round trip, µs, when known.
    pub clock: Option<(i64, i64)>,
    /// 0 decoded whole, 1 early decode, 2 early decode fell back (see `Decoded::early`).
    pub early: u8,
    /// The first block of an early frame was decoded (µs), 0 when not.
    pub first_block_us: u64,
    /// When the pacer meant to present the picture, and when the presenter began on it (µs).
    pub due_us: u64,
    pub begin_us: u64,
    /// How the beam race went for the frame (empty columns when it is off).
    pub race: RaceReport,
    /// The swap chain's counters and vblank time just after the present call (0 when unknown).
    pub flip: Flip,
}

/// `IDXGISwapChain::GetFrameStatistics` and the chain's present count, read after a present.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flip {
    pub present_count: u32,
    pub stat_present_count: u32,
    pub stat_present_refresh: u32,
    pub stat_sync_refresh: u32,
    /// SyncQPCTime on the session clock, µs.
    pub stat_sync_us: u64,
}

enum Message {
    Row(Row),
    Reports(Vec<(HostTiming, Option<HostTimingExtra>)>),
}

/// The open file. Dropping it writes what is waiting and closes the file.
pub struct TimingCsv {
    sender: Option<SyncSender<Message>>,
    lost: Arc<AtomicU64>,
    writer: Option<JoinHandle<()>>,
}

/// The file named by `NIGHTJAR_TIMING_CSV`, if it is set.
pub fn path_from_env() -> Option<PathBuf> {
    let value = std::env::var_os("NIGHTJAR_TIMING_CSV").filter(|v| !v.is_empty())?;
    Some(resolve(Path::new(&value)))
}

/// A folder gets `nightjar-latency.csv` (Android's file name); anything else is the file.
pub fn resolve(path: &Path) -> PathBuf {
    if path.is_dir() { path.join("nightjar-latency.csv") } else { path.to_owned() }
}

impl TimingCsv {
    pub fn create(path: &Path) -> std::io::Result<Self> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let mut file = BufWriter::new(File::create(path)?);
        file.write_all(header().as_bytes())?;
        file.flush()?;
        let (sender, inbox) = sync_channel(QUEUE);
        let lost = Arc::new(AtomicU64::new(0));
        let counter = lost.clone();
        let writer = std::thread::Builder::new()
            .name("timing-csv".into())
            .spawn(move || run(file, inbox, &counter))?;
        Ok(Self { sender: Some(sender), lost, writer: Some(writer) })
    }

    /// Queues one row. Never blocks; a full queue counts the row as lost.
    pub fn row(&self, row: Row) {
        self.send(Message::Row(row));
    }

    /// Hands over the records of a 0x5536 report.
    pub fn reports(&self, records: &[(HostTiming, Option<HostTimingExtra>)]) {
        self.send(Message::Reports(records.to_vec()));
    }

    fn send(&self, message: Message) {
        let Some(sender) = &self.sender else { return };
        match sender.try_send(message) {
            Ok(()) => {}
            Err(TrySendError::Full(Message::Row(_))) => {
                self.lost.fetch_add(1, Ordering::Relaxed);
            }
            // A lost report leaves empty host columns; the row is still written.
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
        }
    }

    /// Rows lost because the writer fell behind.
    pub fn lost(&self) -> u64 {
        self.lost.load(Ordering::Relaxed)
    }
}

impl Drop for TimingCsv {
    fn drop(&mut self) {
        self.sender = None;
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        if self.lost() > 0 {
            eprintln!("nightjar: the timing CSV lost {} rows (the writer fell behind)", self.lost());
        }
    }
}

fn run(mut file: BufWriter<File>, inbox: std::sync::mpsc::Receiver<Message>, lost: &AtomicU64) {
    let mut record = Record::default();
    let mut previous_output = 0u64;
    let mut shown = ShownFps::default();
    // The desktop columns of the rows the record holds, in the order it gives them back.
    let mut extras: VecDeque<String> = VecDeque::new();
    loop {
        let done = match inbox.recv_timeout(Duration::from_millis(100)) {
            Ok(Message::Row(row)) => {
                let fps = shown.push(row.render_us);
                let line = own_fields(&row, lost.load(Ordering::Relaxed), previous_output, fps);
                previous_output = row.output_us;
                extras.push_back(format!("{},{}", scheduler_fields(), extra_fields(&row)));
                record.push(PendingRow {
                    number: row.number,
                    line,
                    rtp_timestamp: row.rtp,
                    clock: row.clock,
                    due_us: session::now_us() + record::ROW_DELAY_US,
                });
                false
            }
            Ok(Message::Reports(reports)) => {
                record.add_reports(reports);
                false
            }
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => true,
        };
        let now = if done { u64::MAX } else { session::now_us() };
        let rows = record.take_due(now);
        for line in &rows {
            let extra = extras.pop_front().unwrap_or_else(|| extra_fields(&Row::default()));
            let _ = writeln!(file, "{line},{extra}");
        }
        if !rows.is_empty() || done {
            let _ = file.flush();
        }
        if done {
            return;
        }
    }
}

fn timestamp(us: u64) -> String {
    if us == 0 { String::new() } else { (us * 1000).to_string() }
}

/// Milliseconds with six decimals between two times, empty when either is missing or the end
/// is before the start (as the Android file does).
fn duration(start_us: u64, end_us: u64) -> String {
    if start_us == 0 || end_us == 0 || end_us < start_us {
        return String::new();
    }
    milliseconds((end_us - start_us) * 1000)
}

fn milliseconds(ns: u64) -> String {
    format!("{}.{:06}", ns / 1_000_000, ns % 1_000_000)
}

/// Shown pictures per second over the last second of presents.
#[derive(Default)]
struct ShownFps {
    renders: VecDeque<u64>,
}

impl ShownFps {
    /// Adds a present and gives the rate when the window spans two or more.
    fn push(&mut self, render_us: u64) -> Option<f64> {
        self.renders.push_back(render_us);
        while self.renders.front().is_some_and(|first| render_us.saturating_sub(*first) > 1_000_000) {
            self.renders.pop_front();
        }
        let span = render_us.saturating_sub(*self.renders.front()?);
        (self.renders.len() >= 2 && span > 0).then(|| (self.renders.len() - 1) as f64 * 1e6 / span as f64)
    }
}

/// The receiver's own 32 columns of one row, comma separated.
pub fn own_fields(row: &Row, lost: u64, previous_output_us: u64, shown_fps: Option<f64>) -> String {
    let mut fields = vec![String::new(); OWN_COLUMNS.len()];
    let mut set = |name: &str, value: String| {
        let at = OWN_COLUMNS.iter().position(|c| *c == name).expect("a known column");
        fields[at] = value;
    };
    let output_to_render = duration(row.output_us, row.render_us);
    let receive_to_render = duration(row.receive_us, row.render_us);
    set("frame_number", row.number.to_string());
    set("pts_us", record::rtp_host_us(row.rtp).to_string());
    set("receive_ns", timestamp(row.receive_us));
    set("decoder_input_ns", timestamp(row.input_us));
    set("decoder_output_ns", timestamp(row.output_us));
    set("render_ns", timestamp(row.render_us));
    set("status", "rendered".into());
    set("receive_to_input_ms", duration(row.receive_us, row.input_us));
    set("input_to_output_ms", duration(row.input_us, row.output_us));
    set("output_to_render_ms", output_to_render.clone());
    set("receive_to_render_ms", receive_to_render.clone());
    set("csv_rows_lost", lost.to_string());
    if row.host_latency != 0 {
        set("host_processing_ms", milliseconds(u64::from(row.host_latency) * 100_000));
    }
    set("decode_to_present_ms", output_to_render);
    set("receive_to_present_ms", receive_to_render);
    set("decode_interval_ms", duration(previous_output_us, row.output_us));
    set("fps_sample_ns", timestamp(row.render_us));
    if let Some(fps) = shown_fps {
        set("shown_fps", format!("{fps:.2}"));
    }
    set("present_drops", row.drops.to_string());
    set("enqueue_ns", timestamp(row.enqueue_us));
    set("queue_wait_ms", duration(row.enqueue_us, row.input_us));
    fields.join(",")
}

/// The scheduler columns of one row: all empty.
pub fn scheduler_fields() -> String {
    vec![""; SCHEDULER_COLUMNS.len()].join(",")
}

/// The desktop columns of one row, comma separated.
pub fn extra_fields(row: &Row) -> String {
    let count = |n: u32| if n == 0 { String::new() } else { n.to_string() };
    let flip = &row.flip;
    let race = &row.race;
    format!(
        "{},{},{},{},{},{},{},{},{},{},{},{},{}",
        row.early, timestamp(row.first_block_us), timestamp(row.due_us), timestamp(row.begin_us), count(flip.present_count),
        count(flip.stat_present_count), count(flip.stat_present_refresh), count(flip.stat_sync_refresh), timestamp(flip.stat_sync_us),
        count(u32::from(race.slices)), timestamp(race.first_ready_us), timestamp(race.last_ready_us),
        match race.ordered { RaceReport::ORDERED => "1", RaceReport::UNORDERED => "0", _ => "" },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The header the Android app writes, read from its source so the two cannot drift apart.
    fn android_header() -> Option<String> {
        let source = include_str!("../../../app/src/main/java/com/limelight/binding/video/FrameLatencyStats.java");
        let start = source.find("CSV_HEADER =")?;
        let end = start + source[start..].find(';')?;
        let mut header = String::new();
        for line in source[start..end].lines() {
            let line = line.trim();
            if line.starts_with("//") {
                continue;
            }
            let mut quoted = line.split('"');
            quoted.next();
            while let Some(text) = quoted.next() {
                header.push_str(text);
                quoted.next();
            }
        }
        // The source spells the line end as a backslash and an n.
        Some(header.trim_end_matches("\\n").to_owned())
    }

    fn row() -> Row {
        Row {
            number: 7,
            rtp: 90_000,
            receive_us: 1_000_000,
            enqueue_us: 1_001_000,
            input_us: 1_001_500,
            output_us: 1_004_500,
            render_us: 1_010_000,
            host_latency: 25,
            drops: 3,
            clock: Some((-5_000, 800)),
            early: 1,
            first_block_us: 1_000_900,
            due_us: 0,
            begin_us: 0,
            race: RaceReport::default(),
            flip: Flip::default(),
        }
    }

    #[test]
    fn the_header_is_the_androids_byte_for_byte() {
        let android = android_header().expect("Android's CSV_HEADER");
        let ours = header();
        let ours = ours.trim_end();
        assert!(ours.starts_with(android.trim_end()), "Android's columns come first, unchanged");
        assert_eq!(&ours[android.trim_end().len()..], format!(",{}", EXTRA_COLUMNS.join(",")));
        assert_eq!(OWN_COLUMNS.len() + 9 + SCHEDULER_COLUMNS.len(), android.split(',').count());
        assert_eq!(ours.split(',').count(), android.split(',').count() + EXTRA_COLUMNS.len());
    }

    #[test]
    fn shown_fps_counts_the_presents_of_the_last_second() {
        let mut shown = ShownFps::default();
        assert_eq!(shown.push(5_000_000), None, "one present has no rate");
        let mut last = None;
        for i in 1..=120u64 {
            last = shown.push(5_000_000 + i * 16_667);
        }
        assert!((last.unwrap() - 60.0).abs() < 0.1, "{last:?}");
        // A stall: the next present, 2 s later, has only itself in the window.
        assert_eq!(shown.push(5_000_000 + 120 * 16_667 + 2_000_000), None);
    }

    #[test]
    fn a_row_fills_the_columns_the_report_reads() {
        let fields = own_fields(&row(), 2, 1_000_000 + 1_000, Some(59.94));
        let fields: Vec<&str> = fields.split(',').collect();
        assert_eq!(fields.len(), OWN_COLUMNS.len());
        let at = |name: &str| fields[OWN_COLUMNS.iter().position(|c| *c == name).unwrap()];
        assert_eq!(at("frame_number"), "7");
        assert_eq!(at("pts_us"), "1000000", "90000 ticks of 90 kHz is one second");
        assert_eq!(at("receive_ns"), "1000000000");
        assert_eq!(at("decoder_input_ns"), "1001500000");
        assert_eq!(at("decoder_output_ns"), "1004500000");
        assert_eq!(at("render_ns"), "1010000000");
        assert_eq!(at("status"), "rendered");
        assert_eq!(at("input_to_output_ms"), "3.000000");
        assert_eq!(at("receive_to_present_ms"), "10.000000");
        assert_eq!(at("host_processing_ms"), "2.500000");
        assert_eq!(at("decode_interval_ms"), "3.500000");
        assert_eq!(at("csv_rows_lost"), "2");
        assert_eq!(at("present_drops"), "3");
        assert_eq!(at("shown_fps"), "59.94");
        assert_eq!(at("release_ns"), "", "MediaCodec's release time has no counterpart");
    }

    #[test]
    fn the_file_has_the_host_columns_from_the_report_and_the_offset() {
        let dir = std::env::temp_dir().join(format!("nightjar-timing-{}", std::process::id()));
        let path = resolve(&dir);
        assert_eq!(path, dir, "a path that is not a folder yet is the file itself");
        let csv = TimingCsv::create(&path).unwrap();
        let timing = HostTiming { frame: 7, host_us: 900, queue_us: 300, encode_us: 200, send_us: 100 };
        let extra = HostTimingExtra { presented_at_us: 2_000_000, capture_us: 100, convert_us: 150 };
        csv.reports(&[(timing, Some(extra))]);
        csv.row(row());
        // A frame without a report is still written, with empty host stamps.
        csv.row(Row { number: 8, ..row() });
        drop(csv);
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert_eq!(format!("{}\n", lines[0]), header());
        let tail = |line: &str| line.split(',').skip(OWN_COLUMNS.len()).collect::<Vec<_>>().join(",");
        assert_eq!(tail(lines[1]), "1000000,2000000,2000100,2000300,2000500,2000800,2000900,-5000,800,,,,,,,,1,1000900000,,,,,,,,,,,");
        assert_eq!(tail(lines[2]), "1000000,,,,,,,-5000,800,,,,,,,,1,1000900000,,,,,,,,,,,");
        for line in &lines {
            assert_eq!(line.split(',').count(), header().trim_end().split(',').count());
        }
    }

    #[test]
    fn the_early_columns_say_how_the_frame_was_decoded() {
        assert_eq!(extra_fields(&Row { early: 0, first_block_us: 0, ..row() }), "0,,,,,,,,,,,,");
        assert_eq!(extra_fields(&Row { early: 2, first_block_us: 0, ..row() }), "2,,,,,,,,,,,,");
        assert_eq!(extra_fields(&row()), "1,1000900000,,,,,,,,,,,");
        let timed = Row {
            due_us: 1_008_000,
            begin_us: 1_008_100,
            flip: Flip { present_count: 7, stat_present_count: 6, stat_present_refresh: 100, stat_sync_refresh: 101, stat_sync_us: 1_009_500 },
            ..row()
        };
        assert_eq!(extra_fields(&timed), "1,1000900000,1008000000,1008100000,7,6,100,101,1009500000,,,,");
        // Rows come back in the order they went in, each with its own columns.
        let path = std::env::temp_dir().join(format!("nightjar-timing-early-{}", std::process::id()));
        let csv = TimingCsv::create(&path).unwrap();
        for (number, early) in [(1, 0u8), (2, 1), (3, 2)] {
            csv.row(Row { number, early, first_block_us: if early == 1 { 1_000_900 } else { 0 }, ..row() });
        }
        drop(csv);
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let ends: Vec<String> = text.lines().skip(1).map(|l| { let f: Vec<&str> = l.split(',').skip(l.split(',').count() - EXTRA_COLUMNS.len()).collect(); format!("{}|{}", f[1], f[0]) }).collect();
        assert_eq!(ends, ["|0", "1000900000|1", "|2"]);
    }

    #[test]
    fn the_race_columns_say_how_the_slices_went() {
        let raced = |slices, ordered| Row {
            race: RaceReport { slices, first_ready_us: 1_003_000, last_ready_us: 1_004_400, ordered },
            ..row()
        };
        let tail = |row: &Row| extra_fields(row).split(',').skip(EXTRA_COLUMNS.len() - 4).collect::<Vec<_>>().join(",");
        assert_eq!(tail(&raced(4, RaceReport::ORDERED)), "4,1003000000,1004400000,1");
        assert_eq!(tail(&raced(1, RaceReport::UNORDERED)), "1,1003000000,1004400000,0", "a frame that left the row order says so");
        assert_eq!(tail(&row()), ",,,", "the race off leaves the columns empty");
        let names = &EXTRA_COLUMNS[EXTRA_COLUMNS.len() - 4..];
        assert_eq!(names, ["slices_decoded", "first_slice_ready_ns", "last_slice_ready_ns", "row_ordered"]);
    }

    #[test]
    fn a_stalled_writer_never_blocks_the_caller() {
        let dir = std::env::temp_dir().join(format!("nightjar-timing-stall-{}", std::process::id()));
        let csv = TimingCsv::create(&dir).unwrap();
        // The channel holds 4096; the writer drains it, but nothing may ever wait on it.
        let started = std::time::Instant::now();
        for number in 0..20_000 {
            csv.row(Row { number, ..row() });
        }
        assert!(started.elapsed() < Duration::from_secs(5), "queuing 20000 rows took {:?}", started.elapsed());
        let lost = csv.lost();
        drop(csv);
        let rows = std::fs::read_to_string(&dir).unwrap().lines().count() - 1;
        let _ = std::fs::remove_file(&dir);
        assert_eq!(rows as u64 + lost, 20_000, "every row is written or counted as lost");
    }

    /// Manual: `RUBYLIGHT_TIMING_REPORT=<path to rust/release/timing_report.py> cargo test -p
    /// nightjar-desktop timing_report -- --ignored`: Rubylight's report reads what is written.
    #[test]
    #[ignore = "needs Rubylight's timing_report.py; see the doc comment"]
    fn timing_report_accepts_a_file_the_desktop_wrote() {
        let Some(script) = crate::sdl::test_input("RUBYLIGHT_TIMING_REPORT", "Rubylight's timing_report.py") else { return };
        let script = script.to_string_lossy().into_owned();
        let path = std::env::temp_dir().join(format!("nightjar-timing-report-{}.csv", std::process::id()));
        let csv = TimingCsv::create(&path).unwrap();
        for number in 0..40u32 {
            let base = 5_000_000 + u64::from(number) * 16_667;
            let present = 100_000 + u64::from(number) * 16_667;
            let timing = HostTiming { frame: number, host_us: 3_000, queue_us: 1_000, encode_us: 1_500, send_us: 200 };
            let extra = HostTimingExtra { presented_at_us: present, capture_us: 200, convert_us: 300 };
            csv.reports(&[(timing, Some(extra))]);
            csv.row(Row {
                number,
                rtp: (u64::from(number) * 1_500) as u32,
                receive_us: base,
                enqueue_us: base + 300,
                input_us: base + 400,
                output_us: base + 2_400,
                render_us: base + 5_000,
                host_latency: 0,
                drops: 0,
                // client = host + 4.9 s
                clock: Some((4_900_000, 600)),
                early: 1,
                first_block_us: base + 1_000,
                due_us: 0,
                begin_us: 0,
                race: RaceReport::default(),
                flip: Flip::default(),
            });
        }
        drop(csv);
        let output = std::process::Command::new("python").args([&script]).arg(&path).output().expect("python");
        let _ = std::fs::remove_file(&path);
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "timing_report.py failed: {}{}", text, String::from_utf8_lossy(&output.stderr));
        println!("{text}");
        assert!(text.contains("clock_sync"), "joined with the clock offset: {text}");
        assert!(text.contains("end_to_end") && text.contains("decode"), "{text}");
    }
}
