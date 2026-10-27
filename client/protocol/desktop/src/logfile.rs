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
    Ok(crate::config::config_dir()?.join("logs"))
}

/// Show the logs folder in the system's file manager.
pub fn open_folder() -> Result<(), String> {
    let dir = logs_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let opener = if cfg!(windows) { "explorer" } else { "xdg-open" };
    // Explorer exits with 1 even when it worked, so the child is not waited for. It must not
    // inherit our stderr, which is the log pipe: its output would land in the log and the pipe
    // would stay open after we leave.
    std::process::Command::new(opener)
        .arg(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(drop)
        .map_err(|e| format!("{opener}: {e}"))
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
fn cut(open: &[u8]) -> usize {
    match open[..OPEN_LINE_LIMIT].iter().rposition(u8::is_ascii_whitespace) {
        Some(space) if space > 0 => space + 1,
        _ => {
            let mut cut = OPEN_LINE_LIMIT;
            while cut > 1 && open[cut] & 0xC0 == 0x80 {
                cut -= 1;
            }
            cut
        }
    }
}

/// Keeps a PC's names and addresses out of the log file from now on.
pub fn add_secrets(record: &crate::config::HostRecord) {
    let names = [&record.name, &record.unique_id, &record.address].into_iter().chain(&record.alias).chain(&record.mac);
    for secret in names.chain(&record.other_addresses) {
        nightjar_core::report::redact::add_secret(secret);
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
    pub fn open(dir: &Path, cap: u64) -> io::Result<Self> {
        let rotation = rotate(dir)?;
        Ok(Self {
            dir: dir.to_path_buf(),
            cap,
            limit: cap,
            file: BufWriter::new(rotation.file),
            written: 0,
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
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;
    use std::time::Duration;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nightjar-log-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn feed(chunks: &[&[u8]]) -> String {
        let mut stamper = Stamper::default();
        let mut out = Vec::new();
        for chunk in chunks {
            stamper.push(chunk, "T ", &mut out);
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn timestamp_is_utc_with_milliseconds() {
        assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01T00:00:00.000Z ");
        let at = UNIX_EPOCH + Duration::new(1_792_936_987, 123_000_000);
        assert_eq!(timestamp(at), "2026-10-25T14:03:07.123Z ");
        assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(951_868_800)), "2000-03-01T00:00:00.000Z ");
        // Leap day and the end of a year.
        assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(1_709_164_800 + 86_399)), "2024-02-29T23:59:59.000Z ");
        assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(1_767_225_599)), "2025-12-31T23:59:59.000Z ");
    }

    #[test]
    fn every_line_gets_one_stamp() {
        assert_eq!(feed(&[b"one\ntwo\n"]), "T one\nT two\n");
    }

    #[test]
    fn a_line_split_over_chunks_is_stamped_once() {
        assert_eq!(feed(&[b"nightj", b"ar: hel", b"lo\nnext"]), "T nightjar: hello\nT next");
        assert_eq!(feed(&[b"a\n", b"", b"b"]), "T a\nT b");
    }

    #[test]
    fn crlf_is_one_line_end_even_across_chunks() {
        assert_eq!(feed(&[b"a\r\nb\r\n"]), "T a\nT b\n");
        assert_eq!(feed(&[b"a\r", b"\nb"]), "T a\nT b");
        // A lone \r inside a line stays.
        assert_eq!(feed(&[b"a\rb\n"]), "T a\rb\n");
        assert_eq!(feed(&[b"a\r", b"b\n"]), "T a\rb\n");
    }

    #[test]
    fn empty_lines_are_not_stamped() {
        assert_eq!(feed(&[b"a\n\n\r\nb\n"]), "T a\n\n\nT b\n");
    }

    #[test]
    fn utf8_split_over_chunks_is_decoded_whole() {
        let bytes = "Größe ä€😀\n".as_bytes();
        // Every split point, including inside the 2, 3 and 4 byte sequences.
        for cut in 0..=bytes.len() {
            let mut carry = Utf8Carry::default();
            let text = carry.push(&bytes[..cut]) + &carry.push(&bytes[cut..]);
            assert_eq!(text, "Größe ä€😀\n", "cut at {cut}");
        }
        // Bad bytes do not stop the decoding or get stuck in the carry.
        let mut carry = Utf8Carry::default();
        assert_eq!(carry.push(b"a\xffb\xc3"), "a\u{FFFD}b");
        assert_eq!(carry.push(b"\xa4c"), "äc");
        assert_eq!(carry.push(b"\xe2\x82"), "");
        assert_eq!(carry.push(b"x"), "\u{FFFD}x");
    }

    #[test]
    fn rotation_moves_current_to_prev_and_replaces_the_old_prev() {
        let dir = temp("rotate");
        assert!(rotate(&dir).unwrap().warning.is_none());
        assert!(dir.join(CURRENT).exists() && !dir.join(PREV).exists());
        std::fs::write(dir.join(CURRENT), "first run").unwrap();
        rotate(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join(PREV)).unwrap(), "first run");
        assert_eq!(std::fs::read_to_string(dir.join(CURRENT)).unwrap(), "");
        std::fs::write(dir.join(CURRENT), "second run").unwrap();
        rotate(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join(PREV)).unwrap(), "second run");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotation_creates_the_folder() {
        let dir = temp("create").join("deeper").join("logs");
        rotate(&dir).unwrap();
        assert!(dir.join(CURRENT).exists());
        let _ = std::fs::remove_dir_all(dir.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn a_prev_file_that_cannot_be_replaced_does_not_stop_the_log() {
        let dir = temp("stuck");
        // A folder where the prev file goes: removing it as a file fails, as a locked file does.
        std::fs::create_dir_all(dir.join(PREV)).unwrap();
        std::fs::write(dir.join(CURRENT), "old run, long enough to see it truncated").unwrap();
        let mut log = LogFile::open(&dir, CAP).unwrap();
        let warnings = log.take_warnings();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("overwritten"));
        log.write_chunk(b"new run\n", UNIX_EPOCH).unwrap();
        log.flush().unwrap();
        assert_eq!(std::fs::read_to_string(dir.join(CURRENT)).unwrap(), "1970-01-01T00:00:00.000Z new run\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_keeps_the_last_two_runs() {
        let dir = temp("open");
        for run in ["a", "b", "c"] {
            let mut log = LogFile::open(&dir, CAP).unwrap();
            log.write_chunk(format!("run {run}\n").as_bytes(), UNIX_EPOCH).unwrap();
            log.flush().unwrap();
        }
        assert_eq!(std::fs::read_to_string(dir.join(CURRENT)).unwrap(), "1970-01-01T00:00:00.000Z run c\n");
        assert_eq!(std::fs::read_to_string(dir.join(PREV)).unwrap(), "1970-01-01T00:00:00.000Z run b\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_full_file_is_rotated_and_stays_under_the_cap() {
        let dir = temp("cap");
        let cap = 600;
        let mut log = LogFile::open(&dir, cap).unwrap();
        for i in 0..50 {
            log.write_chunk(format!("line number {i:03}\n").as_bytes(), UNIX_EPOCH).unwrap();
        }
        log.flush().unwrap();
        let current = std::fs::read_to_string(dir.join(CURRENT)).unwrap();
        let prev = std::fs::read_to_string(dir.join(PREV)).unwrap();
        assert!(current.len() as u64 <= cap && prev.len() as u64 <= cap, "{} {}", current.len(), prev.len());
        // The newest line is kept, the new file says where the rest went, and lines are whole.
        assert!(current.contains("line number 049\n"));
        assert!(current.lines().next().unwrap().contains("log continued"));
        assert!(current.lines().chain(prev.lines()).all(|l| l.starts_with("1970-01-01T00:00:00.000Z ")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_rotation_in_a_run_keeps_appending_and_says_so_once_per_try() {
        let dir = temp("cap-stuck");
        let mut log = LogFile::open(&dir, 300).unwrap();
        std::fs::create_dir_all(dir.join(PREV)).unwrap();
        for i in 0..40 {
            log.write_chunk(format!("line number {i:03}\n").as_bytes(), UNIX_EPOCH).unwrap();
        }
        log.flush().unwrap();
        let text = std::fs::read_to_string(dir.join(CURRENT)).unwrap();
        assert_eq!(text.lines().count(), 40, "nothing is lost");
        // About 1500 bytes in 300-byte steps: a handful of tries, not one per line.
        let warnings = log.take_warnings();
        assert!((2..=8).contains(&warnings.len()), "{warnings:?}");
        assert!(warnings[0].contains("keeps growing"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_chunk_cut_mid_line_over_the_cap_starts_the_new_file_with_a_stamp() {
        let dir = temp("cap-mid");
        let mut log = LogFile::open(&dir, 200).unwrap();
        log.write_chunk(b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n", UNIX_EPOCH).unwrap();
        log.write_chunk(b"part one of a line ", UNIX_EPOCH).unwrap();
        log.write_chunk(b"and its end, long enough to push the file over the cap\n", UNIX_EPOCH).unwrap();
        log.flush().unwrap();
        let current = std::fs::read_to_string(dir.join(CURRENT)).unwrap();
        assert!(current.lines().all(|l| l.starts_with("1970-01-01T00:00:00.000Z ")), "{current}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_logs_folder_follows_the_config_folder() {
        // The only test that sets the variable.
        let dir = temp("env");
        std::env::set_var("NIGHTJAR_CONFIG_DIR", &dir);
        let found = logs_dir().unwrap();
        std::env::remove_var("NIGHTJAR_CONFIG_DIR");
        assert_eq!(found, dir.join("logs"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Feed `reads` through a reader into a writer thread and return what the log file holds.
    fn through_pumps(reads: Vec<Vec<u8>>, name: &str) -> (Vec<u8>, String) {
        let dir = temp(name);
        let log = LogFile::open(&dir, CAP).unwrap();
        let backlog = std::sync::Arc::new(Backlog::new(BACKLOG_BYTES));
        let (files, chunks) = sync_channel(8);
        let writer = {
            let backlog = backlog.clone();
            std::thread::spawn(move || pump_writer(chunks, &backlog, log, |_| {}))
        };
        let mut reads = reads.into_iter();
        let mut forwarded = Vec::new();
        pump_reader(
            |buffer| match reads.next() {
                Some(read) => {
                    buffer[..read.len()].copy_from_slice(&read);
                    read.len()
                }
                None => 0,
            },
            |bytes| forwarded.extend_from_slice(bytes),
            files,
            &backlog,
        );
        writer.join().unwrap();
        let text = std::fs::read_to_string(dir.join(CURRENT)).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        (forwarded, text)
    }

    #[test]
    fn the_pump_forwards_raw_bytes_and_logs_stamped_lines() {
        let (forwarded, text) = through_pumps(vec![b"hello wor".to_vec(), b"ld\r\nbye\n".to_vec()], "pump");
        assert_eq!(forwarded, b"hello world\r\nbye\n");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with("Z hello world") && lines[1].ends_with("Z bye"));
    }

    #[test]
    fn a_burst_of_short_lines_on_a_healthy_disk_loses_nothing() {
        // One short line per read, as eprintln! on many threads produces, far faster than a
        // disk flush per read could keep up with.
        let reads: Vec<Vec<u8>> = (0..20_000).map(|i| format!("nightjar: stream timing line {i:05}\n").into_bytes()).collect();
        let (forwarded, text) = through_pumps(reads, "burst");
        assert_eq!(forwarded.iter().filter(|&&b| b == b'\n').count(), 20_000);
        assert!(!text.contains("dropped"), "nothing may be dropped");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 20_000);
        for (i, line) in lines.iter().enumerate() {
            assert!(line.ends_with(&format!("Z nightjar: stream timing line {i:05}")), "{line}");
        }
    }

    #[test]
    fn a_reader_holds_back_what_the_writer_has_not_taken_and_sends_it_whole() {
        // Room for one chunk. The writer takes nothing until the third read: the second read is
        // held back, forwarded all the same, and goes out with the third. Nothing is lost.
        let backlog = Backlog::new(BACKLOG_BYTES);
        let (files, chunks) = sync_channel(1);
        let script: [&[u8]; 3] = [b"first\n", b"second\n", b"third\n"];
        let mut reads = 0;
        let mut forwarded = Vec::new();
        let mut taken: Vec<Chunk> = Vec::new();
        pump_reader(
            |buffer| {
                if reads == 2 {
                    taken.extend(chunks.try_iter());
                }
                let Some(chunk) = script.get(reads) else { return 0 };
                reads += 1;
                buffer[..chunk.len()].copy_from_slice(chunk);
                chunk.len()
            },
            |bytes| forwarded.extend_from_slice(bytes),
            files,
            &backlog,
        );
        assert_eq!(forwarded, b"first\nsecond\nthird\n");
        taken.extend(chunks.try_iter());
        let seen: Vec<(&[u8], usize)> = taken.iter().map(|c| (&c.bytes[..], c.lost)).collect();
        assert_eq!(seen, vec![(&b"first\n"[..], 0), (&b"second\nthird\n"[..], 0)]);
    }

    #[test]
    fn a_reader_drops_only_above_the_byte_limit_and_counts_it() {
        // The writer is stalled until the third read; the backlog holds 10 bytes.
        let backlog = Backlog::new(10);
        let (files, chunks) = sync_channel(8);
        let script: [&[u8]; 3] = [b"first\n", b"second\n", b"third\n"];
        let mut reads = 0;
        let mut forwarded = Vec::new();
        let mut taken: Vec<Chunk> = Vec::new();
        pump_reader(
            |buffer| {
                if reads == 2 {
                    for chunk in chunks.try_iter() {
                        backlog.taken(chunk.bytes.len());
                        taken.push(chunk);
                    }
                }
                let Some(chunk) = script.get(reads) else { return 0 };
                reads += 1;
                buffer[..chunk.len()].copy_from_slice(chunk);
                chunk.len()
            },
            |bytes| forwarded.extend_from_slice(bytes),
            files,
            &backlog,
        );
        assert_eq!(forwarded, b"first\nsecond\nthird\n", "forwarding never stops");
        taken.extend(chunks.try_iter());
        let seen: Vec<(&[u8], usize)> = taken.iter().map(|c| (&c.bytes[..], c.lost)).collect();
        assert_eq!(seen, vec![(&b"first\n"[..], 0), (&b"third\n"[..], b"second\n".len())]);
    }

    fn write_through_writer(chunks: Vec<Chunk>, name: &str) -> String {
        let dir = temp(name);
        let log = LogFile::open(&dir, CAP).unwrap();
        let backlog = Backlog::new(BACKLOG_BYTES);
        let (files, receiver) = sync_channel(chunks.len().max(1));
        for chunk in chunks {
            files.send(chunk).unwrap();
        }
        drop(files);
        pump_writer(receiver, &backlog, log, |_| {});
        let text = std::fs::read_to_string(dir.join(CURRENT)).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        text
    }

    #[test]
    fn the_chunk_after_a_drop_carries_the_count_and_the_log_says_so() {
        let at = UNIX_EPOCH;
        let text = write_through_writer(
            vec![Chunk { at, bytes: b"before\n".to_vec(), lost: 0 }, Chunk { at, bytes: b"after\n".to_vec(), lost: 13 }],
            "pump-lost",
        );
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert!(lines[0].ends_with("Z before"));
        assert!(lines[1].ends_with("Z 13 bytes of log output were dropped, the disk was too slow"));
        assert!(lines[2].ends_with("Z after"));
    }

    #[test]
    fn the_drop_note_waits_for_the_end_of_the_line() {
        let at = UNIX_EPOCH;
        let text = write_through_writer(
            vec![
                Chunk { at, bytes: b"half a li".to_vec(), lost: 0 },
                Chunk { at, bytes: b"ne\nnext\n".to_vec(), lost: 5 },
                // Lost bytes before a chunk with no line end in it: the note waits for one.
                Chunk { at, bytes: b"x".to_vec(), lost: 0 },
                Chunk { at, bytes: b"y".to_vec(), lost: 3 },
                Chunk { at, bytes: b"z\nlast".to_vec(), lost: 0 },
            ],
            "pump-lost-line",
        );
        let lines: Vec<&str> = text.lines().map(|l| l.trim_start_matches("1970-01-01T00:00:00.000Z ")).collect();
        assert_eq!(
            lines,
            vec![
                "half a line",
                "5 bytes of log output were dropped, the disk was too slow",
                "next",
                "xyz",
                "3 bytes of log output were dropped, the disk was too slow",
                "last"
            ],
            "{text}"
        );
    }

    #[test]
    fn a_drop_at_the_very_end_is_still_reported() {
        let text = write_through_writer(vec![Chunk { at: UNIX_EPOCH, bytes: b"cut".to_vec(), lost: 0 }, Chunk { at: UNIX_EPOCH, bytes: Vec::new(), lost: 9 }], "pump-lost-end");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert!(lines[1].ends_with("Z 9 bytes of log output were dropped, the disk was too slow"));
    }

    #[test]
    fn lines_are_redacted_whole_before_they_reach_the_file() {
        nightjar_core::report::redact::add_secret("Logfile test PC");
        let dir = temp("redact");
        let mut log = LogFile::open(&dir, CAP).unwrap();
        // A PC name and an address cut by chunk ends, a PIN, and a line still open at the end.
        log.write_chunk(b"nightjar: pairing with Logfile te", UNIX_EPOCH).unwrap();
        log.write_chunk(b"st PC at 192.168.", UNIX_EPOCH).unwrap();
        log.write_chunk(b"1.20, PIN 4711\r\nnightjar: stream ", UNIX_EPOCH).unwrap();
        log.write_chunk(b"to fd01::5 ends", UNIX_EPOCH).unwrap();
        log.finish(UNIX_EPOCH).unwrap();
        log.flush().unwrap();
        // Other tests save a PC named "PC", which the Android rules replace inside a mark too.
        assert_eq!(
            std::fs::read_to_string(dir.join(CURRENT)).unwrap().replace("[[PC]]", "[PC]"),
            "1970-01-01T00:00:00.000Z nightjar: pairing with [PC] at [address], PIN [omitted]\n\
             1970-01-01T00:00:00.000Z nightjar: stream to [address] ends"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_line_longer_than_the_limit_is_written_in_parts() {
        let mut redactor = Redactor::default();
        let mut out = Vec::new();
        let mut line = vec![b'z'; OPEN_LINE_LIMIT - 6];
        line.extend_from_slice(b" 192.1");
        redactor.push(&line, &mut out);
        assert!(out.is_empty() && redactor.holds_text());
        // Cut after the last space before the limit: the address stays whole.
        redactor.push(b"68.1.20 ok", &mut out);
        assert_eq!(out, b"[data] ");
        redactor.push(b"\n", &mut out);
        assert_eq!(out, b"[data] [address] ok\n");
        assert!(!redactor.holds_text());

        // Without a space the cut falls between two characters.
        let mut out = Vec::new();
        let text = format!("q{}", "é".repeat(OPEN_LINE_LIMIT / 2));
        redactor.push(text.as_bytes(), &mut out);
        assert!(!out.is_empty() && redactor.holds_text());
        redactor.push(b"\n", &mut out);
        assert_eq!(String::from_utf8(out).unwrap(), format!("{text}\n"));
    }

    #[test]
    fn a_log_note_starts_a_line_of_its_own() {
        let dir = temp("note");
        let mut log = LogFile::open(&dir, CAP).unwrap();
        log.write_chunk(b"half a li", UNIX_EPOCH).unwrap();
        log.note("noted", UNIX_EPOCH).unwrap();
        log.flush().unwrap();
        assert_eq!(std::fs::read_to_string(dir.join(CURRENT)).unwrap(), "1970-01-01T00:00:00.000Z half a li\n1970-01-01T00:00:00.000Z noted\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
