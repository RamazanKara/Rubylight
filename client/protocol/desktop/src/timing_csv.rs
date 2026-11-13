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
//! Three more tell where the display's beam was when the present was issued (the same
//! doc, "Scan line and capability"): `scan_line_at_present` and `in_vblank_at_present`
//! (`D3DKMTGetScanLine`, 1 or 0) and `race_reason` (`on`, or what rules racing out: `no tearing`,
//! `no scanline`, `no scan seen`, `scan rows differ from the display's`, `not fullscreen`,
//! `no VRR`, `no idle at this frame rate`, `no VRR scan restart`, `no scanline, VRR on`, the words of
//! `stream::scanline::Reason`; empty while it is being checked); all empty when the beam race is
//! off.
//! Four more describe how the frame was put on the screen (docs/beam-racing.md, "Raced
//! presenter"), all empty when the beam race is off or the display's scan is not known:
//! `scan_start_ns` (when the first picture row was expected on the screen: the first present's
//! issue time plus the measured start delay), `present_ns` (the issue time of each present of the
//! frame, `|` separated, a raced frame has several), `mid_row_age_ns` (when the beam reaches the
//! middle row minus when the slice that holds it was ready, for raced frames and for frames shown
//! whole alike) and `late_slices` (slices the beam had already passed when they were ready).
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
use crate::stream::race_probe::ScanReport;

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
pub const EXTRA_COLUMNS: [&str; 23] = [
    "early", "first_block_ns", "due_ns", "present_begin_ns", "present_count", "stat_present_count", "stat_present_refresh",
    "stat_sync_refresh", "stat_sync_ns", "slices_decoded", "first_slice_ready_ns", "last_slice_ready_ns", "row_ordered",
    "scan_line_at_present", "in_vblank_at_present", "race_reason",
    "scan_start_ns", "present_ns", "mid_row_age_ns", "late_slices",
    "glass_frame", "present_to_glass_ms", "presentation_flags",
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
    /// The display's beam at the present and whether racing it can work (empty when the race is off).
    pub scan: ScanReport,
    /// How the frame went to the screen (empty when the race is off).
    pub presented: Presented,
    /// The swap chain's counters and vblank time just after the present call (0 when unknown).
    pub flip: Flip,
    /// The last picture the compositor reported on the glass since the row before (Wayland).
    pub glass: Glass,
}

/// A `wp_presentation` answer (`stream::wl_presentation`): it comes a refresh or more after its
/// present, so a row carries the newest one answered since the row before, with its own frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Glass {
    pub frame: Option<u32>,
    /// From the present call to the glass (µs), None when the compositor's clock cannot time it.
    pub to_glass_us: Option<u64>,
    /// `wp_presentation_feedback.kind` bits.
    pub flags: Option<u32>,
}

/// Presents of one frame a row keeps; a raced frame has its slices and the rest (at most four
/// slices, the last rows, a whole picture after a late slice).
pub const PRESENTS: usize = 8;

/// When and how a frame was put on the screen, for the beam race columns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Presented {
    /// When the first picture row was expected on the screen (us), 0 when the scan is not known.
    pub scan_start_us: u64,
    /// The issue times of the frame's presents (us); the first `count` are set.
    pub presents_us: [u64; PRESENTS],
    pub count: u8,
    /// The age of the picture at the middle row when the beam reaches it (ns).
    pub mid_row_age_ns: Option<i64>,
    /// Slices that came after the beam had passed their rows; None when the frame was not under the racer.
    pub late_slices: Option<u32>,
}

impl Presented {
    /// The issue times of the presents, as many as a row keeps.
    pub fn set_presents(&mut self, times_us: &[u64]) {
        let kept = times_us.len().min(PRESENTS);
        self.presents_us[..kept].copy_from_slice(&times_us[..kept]);
        self.count = kept as u8;
    }
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
    Row(Box<Row>),
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
        self.send(Message::Row(Box::new(row)));
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
        "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
        row.early, timestamp(row.first_block_us), timestamp(row.due_us), timestamp(row.begin_us), count(flip.present_count),
        count(flip.stat_present_count), count(flip.stat_present_refresh), count(flip.stat_sync_refresh), timestamp(flip.stat_sync_us),
        count(u32::from(race.slices)), timestamp(race.first_ready_us), timestamp(race.last_ready_us),
        match race.ordered { RaceReport::ORDERED => "1", RaceReport::UNORDERED => "0", _ => "" },
        row.scan.line.map_or_else(String::new, |line| line.to_string()),
        row.scan.vblank.map_or("", |vblank| if vblank { "1" } else { "0" }),
        row.scan.reason,
        timestamp(row.presented.scan_start_us),
        row.presented.presents_us[..usize::from(row.presented.count)].iter().map(|&us| timestamp(us)).collect::<Vec<_>>().join("|"),
        row.presented.mid_row_age_ns.map_or_else(String::new, |ns| ns.to_string()),
        row.presented.late_slices.map_or_else(String::new, |n| n.to_string()),
        row.glass.frame.map_or_else(String::new, |n| n.to_string()),
        row.glass.to_glass_us.map_or_else(String::new, |us| milliseconds(us * 1000)),
        row.glass.flags.map_or_else(String::new, crate::stream::wl_presentation::flag_names),
    )
}

#[cfg(test)]
mod tests;
