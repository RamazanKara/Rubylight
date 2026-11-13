//! The persistent log: everything the process writes to stderr, timestamped, in
//! `<config dir>/logs/nightjar.log` (`logpipe.rs` holds the operating-system part that diverts
//! stderr into a pipe; this module is the file side and has no OS code).
//!
//! - At start `nightjar.log` becomes `nightjar.prev.log` (the older prev is replaced) and a new
//!   `nightjar.log` is created, so the last two runs are kept. If the old file cannot be moved
//!   (a viewer holds the prev file, a second instance runs) the current file is truncated instead
//!   and one line on stderr says so.
//! - A file is capped at [`CAP`]. When a run writes more, the file is rotated the same way and the
//!   run goes on in a fresh `nightjar.log`, so the newest output is always kept and the folder
//!   stays under twice the cap. (That mid-run rotation also replaces the previous run's prev file.)
//!   If that rotation fails the file keeps growing and is tried again after another cap.
//! - Every line gets a UTC wall-clock prefix with milliseconds. The stamp is taken when the
//!   reader thread reads a chunk, not when the line was written, which is within a few
//!   milliseconds. `\r\n` is stored as `\n`; a line that is still open when a chunk ends gets no
//!   second stamp from the next chunk.
//! - Lines are redacted before they reach the file (see [`Redactor`]), so a line is written, and
//!   stamped, when its end arrives. The real stderr gets the bytes unchanged.
//! - Two threads: the reader ([`pump_reader`]) only reads the pipe and forwards to the real
//!   stderr, the writer ([`pump_writer`]) does the file work. A channel joins them and never
//!   blocks the reader: what the writer has not taken yet is held back (up to
//!   [`BACKLOG_BYTES`]). Only above that, on a stalled disk, are lines lost; they are counted and
//!   the log says so at the next line break once the disk recovers. So a slow disk never blocks
//!   `eprintln!` on the stream threads, and a fast one never loses a line.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::time::{SystemTime, UNIX_EPOCH};

/// Size at which a log file is rotated.
pub const CAP: u64 = 4 * 1024 * 1024;
pub const CURRENT: &str = "nightjar.log";
pub const PREV: &str = "nightjar.prev.log";

/// `<config dir>/logs`, next to the settings (the same resolver as the config: `NIGHTJAR_CONFIG_DIR`
/// or the platform directory).
pub fn logs_dir() -> io::Result<PathBuf> {
    let logs = crate::config::config_dir()?.join("logs");
    // The second screen's process (dual screen mode) runs beside the first and keeps its own.
    Ok(if crate::second_screen::this_process() { logs.join("second-screen") } else { logs })
}

/// Show the logs folder in the system's file manager.
pub fn open_folder() -> Result<(), String> {
    let dir = logs_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let opener = match std::env::consts::OS {
        "windows" => "explorer",
        "macos" => "/usr/bin/open",
        _ => "xdg-open",
    };
    // Explorer exits with 1 even when it worked, so the child is not waited for. It must not
    // inherit our stderr, which is the log pipe: its output would land in the log and the pipe
    // would stay open after we leave.
    crate::amd_decode::for_child(&mut std::process::Command::new(opener))
        .arg(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(drop)
        .map_err(|e| format!("{opener}: {e}"))
}

/// Settings → App → Report a problem's file, beside the logs (Android's `ProblemReport`).
pub const REPORT: &str = "nightjar-problem.txt";

/// Write `header`, then this run's log and the run before's, to [`REPORT`] in `dir`, and return
/// its path. The logs are redacted as they are written; the report is scrubbed once more for a
/// PC's name or address learned after its line was written. A missing log is left out.
pub fn write_report(dir: &Path, header: &str) -> io::Result<PathBuf> {
    let mut text = String::from(header);
    for (title, name) in [("This run's log", CURRENT), ("The run before's log", PREV)] {
        match std::fs::read(dir.join(name)) {
            Ok(bytes) => {
                text.push_str(&format!("

{title} ({name}):
"));
                text.push_str(&String::from_utf8_lossy(&bytes));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(io::Error::new(e.kind(), format!("{name}: {e}"))),
        }
    }
    let path = dir.join(REPORT);
    std::fs::create_dir_all(dir)?;
    std::fs::write(&path, nightjar_core::report::redact::scrub(&text))?;
    Ok(path)
}

/// Move `nightjar.log` to `nightjar.prev.log`, replacing the old prev.
fn move_current(dir: &Path) -> io::Result<()> {
    let current = dir.join(CURRENT);
    if !current.exists() {
        return Ok(());
    }
    // rename replaces an existing prev on Unix; Windows needs it out of the way first.
    let prev = dir.join(PREV);
    match std::fs::remove_file(&prev) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    std::fs::rename(&current, &prev)
}

/// A new `nightjar.log`, and what went wrong on the way if the old one could not be moved.
pub struct Rotation {
    pub file: File,
    pub warning: Option<String>,
}

/// Make `nightjar.log` the new `nightjar.prev.log` and create an empty `nightjar.log`. If the
/// old file cannot be moved, `nightjar.log` is truncated instead (or appended to, if even that
/// fails) and the warning says so: a short log is better than none.
pub fn rotate(dir: &Path) -> io::Result<Rotation> {
    std::fs::create_dir_all(dir)?;
    match move_current(dir) {
        Ok(()) => Ok(Rotation { file: File::create(dir.join(CURRENT))?, warning: None }),
        Err(e) => {
            let warning = format!("could not move {CURRENT} to {PREV} ({e}), the earlier log is overwritten");
            let current = dir.join(CURRENT);
            let file = File::create(&current).or_else(|_| std::fs::OpenOptions::new().append(true).create(true).open(&current))?;
            Ok(Rotation { file, warning: Some(warning) })
        }
    }
}

/// `2026-10-25T14:03:07.123Z ` (UTC, trailing space).
pub fn timestamp(now: SystemTime) -> String {
    let since = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let (days, rest) = (since.as_secs() / 86_400, since.as_secs() % 86_400);
    // Civil date from the day count (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z ",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60,
        since.subsec_millis()
    )
}

/// Puts a stamp at the start of every non-empty line in a byte stream that arrives in
/// arbitrary chunks.
pub struct Stamper {
    at_line_start: bool,
    /// A `\r` seen at the end of a chunk: dropped if `\n` follows, kept otherwise.
    pending_cr: bool,
}

impl Default for Stamper {
    fn default() -> Self {
        Self { at_line_start: true, pending_cr: false }
    }
}

impl Stamper {
    /// Append `chunk` to `out`, with `stamp` before each line that starts in it.
    pub fn push(&mut self, chunk: &[u8], stamp: &str, out: &mut Vec<u8>) {
        for &byte in chunk {
            match byte {
                b'\n' => {
                    // \r\n and \n are the same line end; an empty line stays empty.
                    self.pending_cr = false;
                    out.push(b'\n');
                    self.at_line_start = true;
                }
                b'\r' => {
                    if self.pending_cr {
                        self.line_start(stamp, out);
                        out.push(b'\r');
                    }
                    self.pending_cr = true;
                }
                _ => {
                    self.line_start(stamp, out);
                    if self.pending_cr {
                        out.push(b'\r');
                        self.pending_cr = false;
                    }
                    out.push(byte);
                }
            }
        }
    }

    fn line_start(&mut self, stamp: &str, out: &mut Vec<u8>) {
        if self.at_line_start {
            out.extend_from_slice(stamp.as_bytes());
            self.at_line_start = false;
        }
    }

    /// The next byte starts a stamped line (a new file begins this way).
    fn restart(&mut self) {
        self.at_line_start = true;
        self.pending_cr = false;
    }
}

/// Longest line held back for redaction; a longer one is redacted and written in parts.
const OPEN_LINE_LIMIT: usize = 64 * 1024;

/// Redacts the log a whole line at a time (`nightjar_core::report::redact::scrub`, the rules of
/// the Android app's problem reports: the PCs' names and addresses, IDs, keys and PINs): a line
/// is held back until its end arrives.
#[derive(Default)]
pub struct Redactor {
    open: Vec<u8>,
}

impl Redactor {
    /// Append the redacted lines that end in `chunk` to `out`; the rest waits for its line end.
    pub fn push(&mut self, chunk: &[u8], out: &mut Vec<u8>) {
        let held = self.open.len();
        self.open.extend_from_slice(chunk);
        // Only the new bytes can end a line: the held ones have no line end.
        if let Some(last) = chunk.iter().rposition(|&b| b == b'\n') {
            let lines: Vec<u8> = self.open.drain(..held + last + 1).collect();
            for line in lines.split_inclusive(|&b| b == b'\n') {
                redact_line(line, out);
            }
        }
        while self.open.len() > OPEN_LINE_LIMIT {
            let part: Vec<u8> = self.open.drain(..cut(&self.open)).collect();
            redact_line(&part, out);
        }
    }

    /// Append what is held back, redacted, to `out`.
    pub fn release(&mut self, out: &mut Vec<u8>) {
        let open = std::mem::take(&mut self.open);
        redact_line(&open, out);
    }

    pub fn holds_text(&self) -> bool {
        !self.open.is_empty()
    }
}

/// Where a line longer than `OPEN_LINE_LIMIT` is cut: after its last whitespace before the limit,
/// so a word is redacted whole, else at the last character start, so no character is split.
/// Either way not inside a PC's name or between a secret's keyword and its value
/// (`redact::can_cut`), which the parts redacted alone would let through.
fn cut(open: &[u8]) -> usize {
    use nightjar_core::report::redact::can_cut;
    let mut end = OPEN_LINE_LIMIT;
    while let Some(space) = open[..end].iter().rposition(u8::is_ascii_whitespace) {
        if space == 0 {
            break;
        }
        if can_cut(open, space + 1) {
            return space + 1;
        }
        end = space;
    }
    let starts_character = |at: usize| open[at] & 0xC0 != 0x80;
    let mut cut = OPEN_LINE_LIMIT;
    while cut > 1 && !(starts_character(cut) && can_cut(open, cut)) {
        cut -= 1;
    }
    if cut > 1 {
        return cut;
    }
    // Nowhere safe: a line of 64 KiB that is one keyword and value. Never split a character.
    cut = OPEN_LINE_LIMIT;
    while cut > 1 && !starts_character(cut) {
        cut -= 1;
    }
    cut
}

/// Keeps a PC's names and addresses out of the log file from now on.
pub fn add_secrets(record: &crate::config::HostRecord) {
    let names = [&record.name, &record.unique_id, &record.address]
        .into_iter()
        .chain(&record.alias)
        .chain(&record.mac)
        .chain(&record.remote_address);
    for secret in names.chain(&record.other_addresses) {
        nightjar_core::report::redact::add_secret(secret);
    }
    // The PC's internet addresses from its internet play reply stay out of logs too.
    if let Some(reply) = record.internet_play.as_deref().and_then(|text| nightjar_core::http::InternetPlay::parse(text).ok()) {
        let addresses = reply.ipv6.iter().map(ToString::to_string)
            .chain(reply.external_ipv4.map(|ip| ip.to_string()))
            .chain(reply.endpoints.iter().map(|endpoint| endpoint.ip().to_string()));
        for address in addresses {
            nightjar_core::report::redact::add_secret(&address);
        }
    }
}

fn redact_line(line: &[u8], out: &mut Vec<u8>) {
    let (text, end) = match line.split_last() {
        Some((b'\n', text)) => (text, &b"\n"[..]),
        _ => (line, &b""[..]),
    };
    if !text.is_empty() {
        out.extend_from_slice(nightjar_core::report::redact::scrub(&String::from_utf8_lossy(text)).as_bytes());
    }
    out.extend_from_slice(end);
}

/// Decodes UTF-8 that arrives in arbitrary chunks (for a console that wants UTF-16): an
/// incomplete sequence at the end of a chunk waits for the next one, bad bytes become U+FFFD.
// Only the Windows console forwarder uses it.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Default)]
pub struct Utf8Carry {
    pending: Vec<u8>,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Utf8Carry {
    pub fn push(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let mut out = String::new();
        let mut rest = &self.pending[..];
        loop {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    out.push_str(text);
                    rest = &[];
                    break;
                }
                Err(e) => {
                    let (good, bad) = rest.split_at(e.valid_up_to());
                    out.push_str(std::str::from_utf8(good).unwrap_or_default());
                    match e.error_len() {
                        Some(len) => {
                            out.push('\u{FFFD}');
                            rest = &bad[len..];
                        }
                        // Cut off by the end of the chunk: keep it for the next one.
                        None => {
                            rest = bad;
                            break;
                        }
                    }
                }
            }
        }
        let keep = rest.to_vec();
        self.pending = keep;
        out
    }
}

/// The open `nightjar.log`: stamps what it is given, rotates at the cap.
pub struct LogFile {
    dir: PathBuf,
    cap: u64,
    /// Size at which the next rotation is tried (a failed one moves it on by another cap).
    limit: u64,
    file: BufWriter<File>,
    written: u64,
    stamper: Stamper,
    redactor: Redactor,
    scratch: Vec<u8>,
    redacted: Vec<u8>,
    warnings: Vec<String>,
}

impl LogFile {
    /// Rotate the files in `dir` and open the new current one.
    #[cfg(test)]
    pub fn open(dir: &Path, cap: u64) -> io::Result<Self> {
        Self::open_with(dir, cap, false)
    }

    /// [`LogFile::open`], or with `continuing` the current file appended to: a start the run before
    /// made itself (a restart on another GPU) keeps that run's lines, and the run before it in prev.
    pub fn open_with(dir: &Path, cap: u64, continuing: bool) -> io::Result<Self> {
        let rotation = if continuing {
            std::fs::create_dir_all(dir)?;
            Rotation { file: std::fs::OpenOptions::new().append(true).create(true).open(dir.join(CURRENT))?, warning: None }
        } else {
            rotate(dir)?
        };
        let written = if continuing { rotation.file.metadata().map_or(0, |meta| meta.len()) } else { 0 };
        Ok(Self {
            dir: dir.to_path_buf(),
            cap,
            limit: cap,
            file: BufWriter::new(rotation.file),
            written,
            stamper: Stamper::default(),
            redactor: Redactor::default(),
            scratch: Vec::new(),
            redacted: Vec::new(),
            warnings: rotation.warning.into_iter().collect(),
        })
    }

    /// Problems that did not stop the log and are for the user's stderr, once.
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    /// Redact, stamp and write one chunk read from the pipe (not flushed; see [`LogFile::flush`]).
    /// A line that has not ended yet is held back until it does (see [`Redactor`]).
    pub fn write_chunk(&mut self, chunk: &[u8], now: SystemTime) -> io::Result<()> {
        let mut lines = std::mem::take(&mut self.redacted);
        lines.clear();
        self.redactor.push(chunk, &mut lines);
        let written = self.write_lines(&lines, now);
        self.redacted = lines;
        written
    }

    /// Write the line held back for redaction, if any (at the end of the run).
    pub fn finish(&mut self, now: SystemTime) -> io::Result<()> {
        let mut open = Vec::new();
        self.redactor.release(&mut open);
        self.write_lines(&open, now)
    }

    fn write_lines(&mut self, chunk: &[u8], now: SystemTime) -> io::Result<()> {
        if chunk.is_empty() {
            return Ok(());
        }
        let stamp = timestamp(now);
        self.scratch.clear();
        self.stamper.push(chunk, &stamp, &mut self.scratch);
        if self.written > 0 && self.written + self.scratch.len() as u64 > self.limit {
            self.file.flush()?;
            // std opens files with delete sharing, so the open file can be renamed.
            match move_current(&self.dir).and_then(|()| File::create(self.dir.join(CURRENT))) {
                Ok(file) => {
                    self.file = BufWriter::new(file);
                    self.limit = self.cap;
                    self.stamper.restart();
                    let note = format!("{stamp}log continued, the part before is {PREV}\n");
                    self.file.write_all(note.as_bytes())?;
                    self.written = note.len() as u64;
                    // The chunk may have been cut mid-line; the new file starts with a stamp anyway.
                    self.scratch.clear();
                    self.stamper.push(chunk, &stamp, &mut self.scratch);
                }
                Err(e) => {
                    // Keep appending to the file that is open, and try again after another cap.
                    self.limit = self.written + self.cap;
                    self.warnings.push(format!("could not rotate {CURRENT} ({e}), it keeps growing"));
                }
            }
        }
        self.file.write_all(&self.scratch)?;
        self.written += self.scratch.len() as u64;
        Ok(())
    }

    /// A line of the log's own (not from the pipe), on a line of its own.
    pub fn note(&mut self, text: &str, now: SystemTime) -> io::Result<()> {
        self.finish(now)?;
        let mut line = String::new();
        if !self.stamper.at_line_start {
            line.push('\n');
        }
        line.push_str(text);
        line.push('\n');
        self.write_lines(line.as_bytes(), now)
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }

    /// The next byte written starts a line.
    pub fn at_line_start(&self) -> bool {
        self.stamper.at_line_start && !self.redactor.holds_text()
    }
}

/// Bytes the reader may have on their way to the file writer, in the channel and held back.
pub const BACKLOG_BYTES: usize = 4 * 1024 * 1024;

/// How many bytes are waiting for the file writer: output is dropped only above `limit`, so a
/// burst of short lines on a healthy disk is never lost, however many reads it takes.
pub struct Backlog {
    queued: AtomicUsize,
    limit: usize,
}

impl Backlog {
    pub fn new(limit: usize) -> Self {
        Self { queued: AtomicUsize::new(0), limit }
    }

    /// The writer took a chunk of `bytes`.
    pub fn taken(&self, bytes: usize) {
        self.queued.fetch_sub(bytes, Ordering::SeqCst);
    }
}

/// Bytes read from the pipe, on their way to the file writer.
pub struct Chunk {
    /// When the first of the bytes was read.
    pub at: SystemTime,
    pub bytes: Vec<u8>,
    /// Bytes read before these that the reader had to drop because the backlog was full.
    pub lost: usize,
}

/// The reader thread's loop: `read` fills the buffer and returns the byte count (0 = closed),
/// `forward` passes the same bytes on to the real stderr, unchanged, and a copy goes to the file
/// writer with `try_send`. It never waits for the file: if the channel is full the bytes are held
/// back and sent together with the next read, and only when `backlog` is used up (a stalled disk)
/// are they dropped and counted, so the pipe keeps draining and `eprintln!` on the stream threads
/// never blocks on the disk. The next chunk that fits carries the count.
pub fn pump_reader(mut read: impl FnMut(&mut [u8]) -> usize, mut forward: impl FnMut(&[u8]), files: SyncSender<Chunk>, backlog: &Backlog) {
    let mut buffer = [0u8; 16 * 1024];
    let mut pending: Vec<u8> = Vec::new();
    let mut pending_at = SystemTime::now();
    let mut lost = 0;
    let mut writer_gone = false;
    loop {
        let n = read(&mut buffer);
        if n == 0 {
            break;
        }
        forward(&buffer[..n]);
        if writer_gone {
            continue;
        }
        if backlog.queued.load(Ordering::SeqCst) + pending.len() + n > backlog.limit {
            lost += n;
        } else {
            if pending.is_empty() {
                pending_at = SystemTime::now();
            }
            pending.extend_from_slice(&buffer[..n]);
        }
        if pending.is_empty() {
            continue;
        }
        let size = pending.len();
        backlog.queued.fetch_add(size, Ordering::SeqCst);
        match files.try_send(Chunk { at: pending_at, bytes: std::mem::take(&mut pending), lost }) {
            Ok(()) => lost = 0,
            Err(TrySendError::Full(chunk)) => {
                backlog.queued.fetch_sub(size, Ordering::SeqCst);
                pending = chunk.bytes;
            }
            Err(TrySendError::Disconnected(_)) => writer_gone = true,
        }
    }
    // The pipe is closed and the writer is draining: this is the one place that may wait for it.
    if !writer_gone && (!pending.is_empty() || lost > 0) {
        let _ = files.send(Chunk { at: pending_at, bytes: pending, lost });
    }
}

/// Write one chunk. `owed` is the count of dropped bytes not yet reported: the note goes in at
/// the next line break, never inside a line.
fn write_chunk_noting(log: &mut LogFile, owed: &mut usize, chunk: &Chunk) -> io::Result<()> {
    *owed += chunk.lost;
    let mut bytes = &chunk.bytes[..];
    if *owed > 0 {
        if !log.at_line_start() {
            match bytes.iter().position(|&b| b == b'\n') {
                Some(end) => {
                    log.write_chunk(&bytes[..=end], chunk.at)?;
                    bytes = &bytes[end + 1..];
                }
                // Still inside the line: the note waits.
                None => return log.write_chunk(bytes, chunk.at),
            }
        }
        note_dropped(log, owed, chunk.at)?;
    }
    log.write_chunk(bytes, chunk.at)
}

fn note_dropped(log: &mut LogFile, owed: &mut usize, at: SystemTime) -> io::Result<()> {
    log.note(&format!("{owed} bytes of log output were dropped, the disk was too slow"), at)?;
    *owed = 0;
    Ok(())
}

/// The file writer thread's loop, until the reader hangs up. It takes everything that is queued
/// before it flushes. A file error is reported once through `warn` and stops the file side;
/// chunks keep being taken so the backlog does not fill for that reason.
pub fn pump_writer(chunks: Receiver<Chunk>, backlog: &Backlog, mut log: LogFile, mut warn: impl FnMut(&str)) {
    for text in log.take_warnings() {
        warn(&text);
    }
    let mut healthy = true;
    let mut owed = 0;
    let mut last = SystemTime::now();
    while let Ok(first) = chunks.recv() {
        let mut next = Some(first);
        while let Some(chunk) = next.take() {
            backlog.taken(chunk.bytes.len());
            last = chunk.at;
            if healthy {
                if let Err(e) = write_chunk_noting(&mut log, &mut owed, &chunk) {
                    healthy = false;
                    warn(&format!("log file stopped: {e}"));
                }
                for text in log.take_warnings() {
                    warn(&text);
                }
            }
            next = chunks.try_recv().ok();
        }
        if healthy {
            if let Err(e) = log.flush() {
                healthy = false;
                warn(&format!("log file stopped: {e}"));
            }
        }
    }
    if healthy && owed > 0 {
        let _ = note_dropped(&mut log, &mut owed, last);
    }
    if healthy {
        let _ = log.finish(last);
    }
    let _ = log.flush();
}

#[cfg(test)]
mod tests;
