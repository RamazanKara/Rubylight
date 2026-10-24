//! The persistent log: everything the process writes to stderr, timestamped, in
//! `<config dir>/logs/nightjar.log` (`logpipe.rs` holds the operating-system part that diverts
//! stderr into a pipe; this module is the file side and has no OS code).
//!
//! - At start `nightjar.log` becomes `nightjar.prev.log` (the older prev is replaced) and a new
//!   `nightjar.log` is created, so the last two runs are kept.
//! - A file is capped at [`CAP`]. When a run writes more, the file is rotated the same way and the
//!   run goes on in a fresh `nightjar.log`, so the newest output is always kept and the folder
//!   stays under twice the cap. (That mid-run rotation also replaces the previous run's prev file.)
//! - Every line gets a UTC wall-clock prefix with milliseconds. The stamp is taken when the
//!   logger thread reads a chunk, not when the line was written, which is within a few
//!   milliseconds. `\r\n` is stored as `\n`; a line that is still open when a chunk ends gets no
//!   second stamp from the next chunk.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
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
    // Explorer exits with 1 even when it worked, so the child is not waited for.
    std::process::Command::new(opener).arg(&dir).spawn().map(drop).map_err(|e| format!("{opener}: {e}"))
}

/// Make `nightjar.log` the new `nightjar.prev.log` and create an empty `nightjar.log`.
pub fn rotate(dir: &Path) -> io::Result<File> {
    std::fs::create_dir_all(dir)?;
    let current = dir.join(CURRENT);
    if current.exists() {
        // rename replaces an existing prev on Unix; Windows needs it out of the way first.
        let prev = dir.join(PREV);
        match std::fs::remove_file(&prev) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
        std::fs::rename(&current, &prev)?;
    }
    File::create(current)
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

/// The open `nightjar.log`: stamps what it is given, rotates at the cap.
pub struct LogFile {
    dir: PathBuf,
    cap: u64,
    file: BufWriter<File>,
    written: u64,
    stamper: Stamper,
    scratch: Vec<u8>,
}

impl LogFile {
    /// Rotate the files in `dir` and open the new current one.
    pub fn open(dir: &Path, cap: u64) -> io::Result<Self> {
        Ok(Self {
            dir: dir.to_path_buf(),
            cap,
            file: BufWriter::new(rotate(dir)?),
            written: 0,
            stamper: Stamper::default(),
            scratch: Vec::new(),
        })
    }

    /// Stamp and write one chunk read from the pipe (not flushed; see [`LogFile::flush`]).
    pub fn write_chunk(&mut self, chunk: &[u8], now: SystemTime) -> io::Result<()> {
        let stamp = timestamp(now);
        self.scratch.clear();
        self.stamper.push(chunk, &stamp, &mut self.scratch);
        if self.written > 0 && self.written + self.scratch.len() as u64 > self.cap {
            self.file.flush()?;
            // std opens files with delete sharing, so the open file can be renamed.
            self.file = BufWriter::new(rotate(&self.dir)?);
            let note = format!("{stamp}log continued, the part before is {PREV}
");
            self.file.write_all(note.as_bytes())?;
            self.written = note.len() as u64;
            // The chunk may have been cut mid-line; the new file starts with a stamp anyway.
            self.stamper.restart();
            self.scratch.clear();
            self.stamper.push(chunk, &stamp, &mut self.scratch);
        }
        self.file.write_all(&self.scratch)?;
        self.written += self.scratch.len() as u64;
        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// The logger thread's loop: `read` fills the buffer and returns the byte count (0 = closed),
/// `forward` passes the same bytes on to the real stderr, unchanged. A file error is reported once
/// through `forward` and stops the file side; forwarding goes on.
pub fn pump(mut read: impl FnMut(&mut [u8]) -> usize, mut forward: impl FnMut(&[u8]), mut log: LogFile) {
    let mut buffer = [0u8; 16 * 1024];
    let mut healthy = true;
    loop {
        let n = read(&mut buffer);
        if n == 0 {
            break;
        }
        let chunk = &buffer[..n];
        forward(chunk);
        if healthy {
            if let Err(e) = log.write_chunk(chunk, SystemTime::now()).and_then(|()| log.flush()) {
                healthy = false;
                forward(format!("nightjar: log file stopped: {e}\n").as_bytes());
            }
        }
    }
    let _ = log.flush();
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn rotation_moves_current_to_prev_and_replaces_the_old_prev() {
        let dir = temp("rotate");
        assert!(rotate(&dir).is_ok());
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
    fn pump_forwards_raw_bytes_and_logs_stamped_lines() {
        let dir = temp("pump");
        let log = LogFile::open(&dir, CAP).unwrap();
        let mut input: Vec<&[u8]> = vec![b"hello wor", b"ld\r\nbye\n"];
        input.reverse();
        let mut forwarded = Vec::new();
        pump(
            |buffer| match input.pop() {
                Some(chunk) => {
                    buffer[..chunk.len()].copy_from_slice(chunk);
                    chunk.len()
                }
                None => 0,
            },
            |bytes| forwarded.extend_from_slice(bytes),
            log,
        );
        assert_eq!(forwarded, b"hello world\r\nbye\n");
        let text = std::fs::read_to_string(dir.join(CURRENT)).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with("Z hello world") && lines[1].ends_with("Z bye"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
