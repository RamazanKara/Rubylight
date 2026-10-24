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
//! - Two threads: the reader ([`pump_reader`]) only reads the pipe and forwards to the real
//!   stderr, the writer ([`pump_writer`]) does the file work. A bounded channel joins them and
//!   never blocks the reader, so a stalled disk loses log lines (counted, and said so in the
//!   log once the disk recovers) instead of blocking `eprintln!` on the stream threads.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc::{Receiver, SyncSender};
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

/// Decodes UTF-8 that arrives in arbitrary chunks (for a console that wants UTF-16): an
/// incomplete sequence at the end of a chunk waits for the next one, bad bytes become U+FFFD.
#[derive(Default)]
pub struct Utf8Carry {
    pending: Vec<u8>,
}

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
    scratch: Vec<u8>,
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
            scratch: Vec::new(),
            warnings: rotation.warning.into_iter().collect(),
        })
    }

    /// Problems that did not stop the log and are for the user's stderr, once.
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    /// Stamp and write one chunk read from the pipe (not flushed; see [`LogFile::flush`]).
    pub fn write_chunk(&mut self, chunk: &[u8], now: SystemTime) -> io::Result<()> {
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
        let mut line = String::new();
        if !self.stamper.at_line_start {
            line.push('\n');
        }
        line.push_str(text);
        line.push('\n');
        self.write_chunk(line.as_bytes(), now)
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// One read from the pipe, on its way to the file writer.
pub struct Chunk {
    pub at: SystemTime,
    pub bytes: Vec<u8>,
    /// Bytes read before this chunk that the reader had to drop because the channel was full.
    pub lost: usize,
}

/// The reader thread's loop: `read` fills the buffer and returns the byte count (0 = closed),
/// `forward` passes the same bytes on to the real stderr, unchanged, and the chunk goes to the
/// file writer with `try_send`. It never waits for the file: if the writer is behind (a stalled
/// disk) the chunk is dropped and counted, so the pipe keeps draining and `eprintln!` on the
/// stream threads never blocks on the disk. The next chunk that fits carries the count.
pub fn pump_reader(mut read: impl FnMut(&mut [u8]) -> usize, mut forward: impl FnMut(&[u8]), files: SyncSender<Chunk>) {
    let mut buffer = [0u8; 16 * 1024];
    let mut lost = 0;
    loop {
        let n = read(&mut buffer);
        if n == 0 {
            break;
        }
        forward(&buffer[..n]);
        match files.try_send(Chunk { at: SystemTime::now(), bytes: buffer[..n].to_vec(), lost }) {
            Ok(()) => lost = 0,
            Err(_) => lost += n,
        }
    }
}

/// The file writer thread's loop, until the reader hangs up. A file error is reported once
/// through `warn` and stops the file side; chunks keep being taken so the channel does not fill
/// for that reason.
pub fn pump_writer(chunks: Receiver<Chunk>, mut log: LogFile, mut warn: impl FnMut(&str)) {
    fn write(log: &mut LogFile, chunk: &Chunk) -> io::Result<()> {
        if chunk.lost > 0 {
            log.note(&format!("{} bytes of log output were dropped, the disk was too slow", chunk.lost), chunk.at)?;
        }
        log.write_chunk(&chunk.bytes, chunk.at)?;
        log.flush()
    }
    for text in log.take_warnings() {
        warn(&text);
    }
    let mut healthy = true;
    for chunk in chunks.iter() {
        if !healthy {
            continue;
        }
        if let Err(e) = write(&mut log, &chunk) {
            healthy = false;
            warn(&format!("log file stopped: {e}"));
        }
        for text in log.take_warnings() {
            warn(&text);
        }
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

    #[test]
    fn the_pump_forwards_raw_bytes_and_logs_stamped_lines() {
        let dir = temp("pump");
        let log = LogFile::open(&dir, CAP).unwrap();
        let (files, chunks) = sync_channel(8);
        let mut input: Vec<&[u8]> = vec![b"hello wor", b"ld\r\nbye\n"];
        input.reverse();
        let mut forwarded = Vec::new();
        pump_reader(
            |buffer| match input.pop() {
                Some(chunk) => {
                    buffer[..chunk.len()].copy_from_slice(chunk);
                    chunk.len()
                }
                None => 0,
            },
            |bytes| forwarded.extend_from_slice(bytes),
            files,
        );
        let mut warnings = Vec::new();
        pump_writer(chunks, log, |text| warnings.push(text.to_owned()));
        assert_eq!(forwarded, b"hello world\r\nbye\n");
        assert!(warnings.is_empty());
        let text = std::fs::read_to_string(dir.join(CURRENT)).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with("Z hello world") && lines[1].ends_with("Z bye"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reader_never_waits_for_a_writer_that_is_behind() {
        // Room for one chunk. The writer takes nothing until the third read: the second chunk is
        // dropped but still forwarded, and the third (which fits again) carries the loss.
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
        );
        assert_eq!(forwarded, b"first\nsecond\nthird\n");
        taken.extend(chunks.try_iter());
        let seen: Vec<(&[u8], usize)> = taken.iter().map(|c| (&c.bytes[..], c.lost)).collect();
        assert_eq!(seen, vec![(&b"first\n"[..], 0), (&b"third\n"[..], b"second\n".len())]);
    }

    #[test]
    fn the_chunk_after_a_drop_carries_the_count_and_the_log_says_so() {
        let dir = temp("pump-lost");
        let log = LogFile::open(&dir, CAP).unwrap();
        let (files, chunks) = sync_channel(1);
        let at = UNIX_EPOCH;
        // What pump_reader leaves behind after a stall: a chunk that fits and, after two dropped
        // ones (13 bytes), the next one that does.
        files.send(Chunk { at, bytes: b"before\n".to_vec(), lost: 0 }).unwrap();
        let writer = std::thread::spawn(move || pump_writer(chunks, log, |_| {}));
        files.send(Chunk { at, bytes: b"after\n".to_vec(), lost: 13 }).unwrap();
        drop(files);
        writer.join().unwrap();
        let text = std::fs::read_to_string(dir.join(CURRENT)).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert!(lines[0].ends_with("Z before"));
        assert!(lines[1].ends_with("Z 13 bytes of log output were dropped, the disk was too slow"));
        assert!(lines[2].ends_with("Z after"));
        let _ = std::fs::remove_dir_all(&dir);
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
