//! The packet trace, for telling a network that delivers packets in bursts from a receive
//! thread that wakes late (debug only, off unless `NIGHTJAR_PACKET_TRACE` names a file prefix).
//!
//! Each receive thread keeps one record per datagram in memory and, when it ends, hands them to a
//! writer thread: `<prefix>-video.csv` and `<prefix>-audio.csv` with
//! `at_us,waited_us,len,seq,rtp_ts`, where `at_us` is when the receive returned (session clock,
//! [`super::now_us`]) and `waited_us` how long the thread sat in it since the packet before,
//! across its poll timeouts. `seq` and `rtp_ts` are 0 for encrypted video, whose datagrams start
//! with the IV instead of the RTP header. A gap in `at_us` that the thread spent waiting, followed by
//! packets that waited nothing, is packets that were not there yet (or a thread that was not
//! scheduled); the canary tells those apart. It is a thread beside the video one that sleeps
//! 1 ms at a time and writes every wake more than [`CANARY_LATE`] late to `<prefix>-canary.csv`
//! (`at_us,late_us`): a scheduler that keeps threads off the CPU makes it late too, a network
//! that holds packets back does not. With the variable unset nothing is allocated or timed.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use super::now_us;

/// Records kept per stream; later packets are counted, not kept (a 60 s stream at 50 Mbps is
/// about 300 000 video packets).
const CAPACITY: usize = 2_000_000;
/// Records are kept in chunks of this many, so the receive thread never copies what it has: a
/// full chunk stays and the next one starts (1.5 MB of video, one every few seconds at most).
const VIDEO_CHUNK: usize = 1 << 16;
/// Audio comes at 200 packets a second: a chunk holds 40 s of it.
const AUDIO_CHUNK: usize = 1 << 13;
/// The canary's sleep.
const CANARY_SLEEP: Duration = Duration::from_millis(1);
/// A canary wake later than the sleep plus this is written down.
pub const CANARY_LATE: Duration = Duration::from_millis(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Record {
    pub at_us: u64,
    pub waited_us: u32,
    pub len: u32,
    /// The RTP header's sequence number and timestamp (0 for a datagram too short to have them
    /// or an encrypted one).
    pub seq: u16,
    pub rtp_ts: u32,
}

impl Record {
    /// A datagram received at `at_us` after waiting since `before_us`; `encrypted` when it starts
    /// with an IV, not the RTP header.
    pub fn new(before_us: u64, at_us: u64, datagram: &[u8], encrypted: bool) -> Self {
        let (seq, rtp_ts) = match datagram {
            _ if encrypted => (0, 0),
            [_, _, s0, s1, t0, t1, t2, t3, ..] => (
                u16::from_be_bytes([*s0, *s1]),
                u32::from_be_bytes([*t0, *t1, *t2, *t3]),
            ),
            _ => (0, 0),
        };
        Self {
            at_us,
            waited_us: at_us.saturating_sub(before_us).min(u64::from(u32::MAX)) as u32,
            len: datagram.len() as u32,
            seq,
            rtp_ts,
        }
    }
}

/// Which stream a trace records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stream {
    /// Video, with the canary; `encrypted` under `SS_ENC_VIDEO`.
    Video { encrypted: bool },
    /// Audio, whose RTP header stays readable under `SS_ENC_AUDIO`.
    Audio,
}

pub(crate) struct PacketTrace {
    path: PathBuf,
    encrypted: bool,
    chunk: usize,
    chunks: Vec<Vec<Record>>,
    kept: usize,
    dropped: u64,
    /// When the thread started waiting for the next datagram, over all its poll timeouts.
    waiting_since: Option<u64>,
    canary: Option<Canary>,
}

impl PacketTrace {
    /// The trace of one stream when `NIGHTJAR_PACKET_TRACE` is set; the video one also starts
    /// the canary.
    pub fn from_env(stream: Stream) -> Option<Self> {
        let prefix = std::env::var_os("NIGHTJAR_PACKET_TRACE").filter(|v| !v.is_empty())?;
        let prefix = prefix.to_string_lossy();
        Some(match stream {
            Stream::Video { encrypted } => Self::new(
                PathBuf::from(format!("{prefix}-video.csv")),
                encrypted,
                VIDEO_CHUNK,
                Some(Canary::start(PathBuf::from(format!("{prefix}-canary.csv")))),
            ),
            Stream::Audio => {
                Self::new(PathBuf::from(format!("{prefix}-audio.csv")), false, AUDIO_CHUNK, None)
            }
        })
    }

    fn new(path: PathBuf, encrypted: bool, chunk: usize, canary: Option<Canary>) -> Self {
        Self {
            path,
            encrypted,
            chunk,
            chunks: vec![Vec::with_capacity(chunk)],
            kept: 0,
            dropped: 0,
            waiting_since: None,
            canary,
        }
    }

    /// The thread is about to wait for a datagram. A wait that runs over poll timeouts counts
    /// from the first.
    pub fn waiting(&mut self) {
        self.waiting_since.get_or_insert_with(now_us);
    }

    /// A datagram arrived at `at_us`.
    pub fn push(&mut self, at_us: u64, datagram: &[u8]) {
        let before_us = self.waiting_since.take().unwrap_or(at_us);
        if self.kept == CAPACITY {
            self.dropped += 1;
            return;
        }
        if self.chunks.last().map_or(true, |chunk| chunk.len() == self.chunk) {
            self.chunks.push(Vec::with_capacity(self.chunk));
        }
        if let Some(chunk) = self.chunks.last_mut() {
            chunk.push(Record::new(before_us, at_us, datagram, self.encrypted));
            self.kept += 1;
        }
    }

    /// Stops the canary and writes the files on a thread of their own, which says where they
    /// went: the receive thread ends at once, and the stream's stop does not wait on the disk.
    pub fn finish(mut self, log: impl Fn(&str) + Clone + Send + 'static) -> Option<JoinHandle<()>> {
        let canary = self.canary.take();
        let report = log.clone();
        let spawned = std::thread::Builder::new()
            .name("TraceWriter".into())
            .spawn(move || self.write(canary, &log));
        match spawned {
            Ok(thread) => Some(thread),
            Err(e) => {
                // The failed spawn drops the trace, and the canary stops by its drop.
                report(&format!("Packet trace: not written, its writer did not start: {e}"));
                None
            }
        }
    }

    fn write(&self, canary: Option<Canary>, log: &impl Fn(&str)) {
        let canary = canary.map(Canary::stop);
        match write_records(&self.path, self.chunks.iter().flatten()) {
            Ok(()) => log(&format!(
                "Packet trace: {} packets to {} ({} not kept)",
                self.kept,
                self.path.display(),
                self.dropped
            )),
            Err(e) => log(&format!("Packet trace: could not write {}: {e}", self.path.display())),
        }
        if let Some((path, result)) = canary {
            match result {
                Ok((wakes, late, worst_us)) => log(&format!(
                    "Packet trace canary: {wakes} wakes, {late} late by over {} ms (worst {:.1} ms) to {}",
                    CANARY_LATE.as_millis(),
                    worst_us as f64 / 1000.0,
                    path.display()
                )),
                Err(e) => log(&format!("Packet trace canary: could not write {}: {e}", path.display())),
            }
        }
    }
}

fn write_records<'a>(
    path: &std::path::Path,
    records: impl Iterator<Item = &'a Record>,
) -> io::Result<()> {
    let mut file = BufWriter::new(File::create(path)?);
    writeln!(file, "at_us,waited_us,len,seq,rtp_ts")?;
    for r in records {
        writeln!(file, "{},{},{},{},{}", r.at_us, r.waited_us, r.len, r.seq, r.rtp_ts)?;
    }
    file.flush()
}

type CanaryResult = io::Result<(u64, u64, u64)>;

struct Canary {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<CanaryResult>>,
}

impl Canary {
    fn start(path: PathBuf) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let out = path.clone();
        let thread = std::thread::Builder::new()
            .name("TraceCanary".into())
            .spawn(move || canary_loop(&out, &flag))
            .ok();
        Self { path, stop, thread }
    }

    fn stop(mut self) -> (PathBuf, CanaryResult) {
        self.stop.store(true, Ordering::Release);
        let result = match self.thread.take().map(JoinHandle::join) {
            Some(Ok(result)) => result,
            Some(Err(_)) => Err(io::Error::other("the canary thread panicked")),
            None => Err(io::Error::other("the canary thread did not start")),
        };
        (std::mem::take(&mut self.path), result)
    }
}

/// A trace dropped without [`PacketTrace::finish`] (a receive thread that panicked) still ends
/// its canary, which writes its file and exits.
impl Drop for Canary {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// Sleeps [`CANARY_SLEEP`] until stopped, keeping the late wakes; gives the wakes, the late
/// ones and the worst lateness (µs).
fn canary_loop(path: &std::path::Path, stop: &AtomicBool) -> CanaryResult {
    let mut late = Vec::new();
    let (mut wakes, mut worst) = (0u64, 0u64);
    let allowed = (CANARY_SLEEP + CANARY_LATE).as_micros() as u64;
    while !stop.load(Ordering::Acquire) {
        let before = now_us();
        std::thread::sleep(CANARY_SLEEP);
        let at = now_us();
        wakes += 1;
        let slept = at - before;
        worst = worst.max(slept.saturating_sub(CANARY_SLEEP.as_micros() as u64));
        if slept > allowed && late.len() < CAPACITY {
            late.push((at, slept - CANARY_SLEEP.as_micros() as u64));
        }
    }
    let mut file = BufWriter::new(File::create(path)?);
    writeln!(file, "at_us,late_us")?;
    for (at, by) in &late {
        writeln!(file, "{at},{by}")?;
    }
    file.flush()?;
    Ok((wakes, late.len() as u64, worst))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_has_the_rtp_sequence_and_timestamp_and_the_wait() {
        let mut datagram = [0u8; 20];
        datagram[2..4].copy_from_slice(&0x1234u16.to_be_bytes());
        datagram[4..8].copy_from_slice(&90_000u32.to_be_bytes());
        let record = Record::new(1_000, 1_250, &datagram, false);
        assert_eq!(
            record,
            Record { at_us: 1_250, waited_us: 250, len: 20, seq: 0x1234, rtp_ts: 90_000 }
        );
        // A runt has no header to read, and a clock read out of order waited nothing.
        assert_eq!(
            Record::new(5, 4, &[1, 2, 3], false),
            Record { at_us: 4, waited_us: 0, len: 3, seq: 0, rtp_ts: 0 }
        );
        // Encrypted video starts with the IV: its bytes are no sequence or timestamp.
        assert_eq!(
            Record::new(1_000, 1_250, &datagram, true),
            Record { at_us: 1_250, waited_us: 250, len: 20, seq: 0, rtp_ts: 0 }
        );
    }

    #[test]
    fn a_wait_spans_the_poll_timeouts_since_the_packet_before() {
        let mut trace = PacketTrace::new(PathBuf::new(), false, 2, None);
        trace.waiting();
        let first = trace.waiting_since.unwrap();
        // Two 100 ms poll timeouts later the thread waits again: the wait still counts from the
        // first.
        std::thread::sleep(Duration::from_millis(5));
        trace.waiting();
        trace.waiting();
        assert_eq!(trace.waiting_since, Some(first));
        trace.push(first + 250_000, &[0; 8]);
        // The next wait starts over; a push without one waited nothing.
        trace.push(first + 260_000, &[0; 8]);
        trace.push(first + 270_000, &[0; 8]);
        let waited: Vec<u32> = trace.chunks.iter().flatten().map(|r| r.waited_us).collect();
        assert_eq!(waited, [250_000, 0, 0]);
        // Full chunks stay where they are and the next one starts.
        assert_eq!(trace.chunks.iter().map(Vec::len).collect::<Vec<_>>(), [2, 1]);
        assert_eq!(trace.chunks[0].capacity(), 2);
    }

    #[test]
    fn the_trace_writes_its_packets_and_the_canary_its_late_wakes() {
        let dir = std::env::temp_dir().join(format!("nightjar-packet-trace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t-video.csv");
        let mut trace =
            PacketTrace::new(path.clone(), false, 1, Some(Canary::start(dir.join("t-canary.csv"))));
        trace.waiting_since = Some(10);
        trace.push(30, &[0, 0, 0, 7, 0, 0, 0, 9]);
        trace.push(40, &[0, 0, 0, 8, 0, 0, 0, 10]);
        std::thread::sleep(Duration::from_millis(20));
        let lines = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = lines.clone();
        let writer = trace.finish(move |line| sink.lock().unwrap().push(line.to_owned()));
        writer.expect("the writer starts").join().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "at_us,waited_us,len,seq,rtp_ts\n30,20,8,7,9\n40,0,8,8,10\n");
        let canary = std::fs::read_to_string(dir.join("t-canary.csv")).unwrap();
        assert!(canary.starts_with("at_us,late_us\n"), "{canary}");
        let lines = lines.lock().unwrap();
        assert!(lines[0].starts_with("Packet trace: 2 packets"), "{lines:?}");
        assert!(lines[1].starts_with("Packet trace canary: "), "{lines:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dropped_trace_stops_its_canary() {
        let dir = std::env::temp_dir().join(format!("nightjar-packet-canary-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut canary = Canary::start(dir.join("c-canary.csv"));
        let thread = canary.thread.take().expect("the canary starts");
        drop(canary);
        // Without the drop's stop the canary would sleep on forever.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !thread.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(thread.is_finished(), "the canary still runs");
        assert!(thread.join().unwrap().is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
