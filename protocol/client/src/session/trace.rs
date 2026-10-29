//! The packet trace, for telling a network that delivers packets in bursts from a receive
//! thread that wakes late (debug only, off unless `NIGHTJAR_PACKET_TRACE` names a file prefix).
//!
//! Each receive thread keeps one record per datagram in memory and writes them when it ends:
//! `<prefix>-video.csv` and `<prefix>-audio.csv` with `at_us,waited_us,len,seq,rtp_ts`, where
//! `at_us` is when the receive returned (session clock, [`super::now_us`]) and `waited_us` how
//! long the thread sat in it. A gap in `at_us` that the thread spent waiting, followed by
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
/// The canary's sleep.
const CANARY_SLEEP: Duration = Duration::from_millis(1);
/// A canary wake later than the sleep plus this is written down.
pub const CANARY_LATE: Duration = Duration::from_millis(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Record {
    pub at_us: u64,
    pub waited_us: u32,
    pub len: u32,
    /// The RTP header's sequence number and timestamp (0 for a datagram too short to have them).
    pub seq: u16,
    pub rtp_ts: u32,
}

impl Record {
    pub fn new(before_us: u64, at_us: u64, datagram: &[u8]) -> Self {
        let (seq, rtp_ts) = match datagram {
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

pub(crate) struct PacketTrace {
    path: PathBuf,
    records: Vec<Record>,
    dropped: u64,
    canary: Option<Canary>,
}

impl PacketTrace {
    /// The trace of one stream (`video` or `audio`) when `NIGHTJAR_PACKET_TRACE` is set; the
    /// video one also starts the canary.
    pub fn from_env(stream: &str, canary: bool) -> Option<Self> {
        let prefix = std::env::var_os("NIGHTJAR_PACKET_TRACE").filter(|v| !v.is_empty())?;
        let prefix = prefix.to_string_lossy();
        Some(Self {
            canary: canary.then(|| Canary::start(PathBuf::from(format!("{prefix}-canary.csv")))),
            path: PathBuf::from(format!("{prefix}-{stream}.csv")),
            records: Vec::with_capacity(CAPACITY.min(1 << 20)),
            dropped: 0,
        })
    }

    pub fn push(&mut self, before_us: u64, at_us: u64, datagram: &[u8]) {
        if self.records.len() < CAPACITY {
            self.records.push(Record::new(before_us, at_us, datagram));
        } else {
            self.dropped += 1;
        }
    }

    /// Writes the files and says where they went.
    pub fn finish(mut self, log: impl Fn(&str)) {
        let canary = self.canary.take().map(Canary::stop);
        match write_records(&self.path, &self.records) {
            Ok(()) => log(&format!(
                "Packet trace: {} packets to {} ({} not kept)",
                self.records.len(),
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

fn write_records(path: &std::path::Path, records: &[Record]) -> io::Result<()> {
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
        (self.path, result)
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
        let record = Record::new(1_000, 1_250, &datagram);
        assert_eq!(
            record,
            Record { at_us: 1_250, waited_us: 250, len: 20, seq: 0x1234, rtp_ts: 90_000 }
        );
        // A runt has no header to read, and a clock read out of order waited nothing.
        assert_eq!(Record::new(5, 4, &[1, 2, 3]), Record { at_us: 4, waited_us: 0, len: 3, seq: 0, rtp_ts: 0 });
    }

    #[test]
    fn the_trace_writes_its_packets_and_the_canary_its_late_wakes() {
        let dir = std::env::temp_dir().join(format!("nightjar-packet-trace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t-video.csv");
        let mut trace = PacketTrace {
            canary: Some(Canary::start(dir.join("t-canary.csv"))),
            path: path.clone(),
            records: Vec::new(),
            dropped: 0,
        };
        trace.push(10, 30, &[0, 0, 0, 7, 0, 0, 0, 9]);
        std::thread::sleep(Duration::from_millis(20));
        let lines = std::sync::Mutex::new(Vec::new());
        trace.finish(|line| lines.lock().unwrap().push(line.to_owned()));
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "at_us,waited_us,len,seq,rtp_ts\n30,20,8,7,9\n");
        let canary = std::fs::read_to_string(dir.join("t-canary.csv")).unwrap();
        assert!(canary.starts_with("at_us,late_us\n"), "{canary}");
        let lines = lines.into_inner().unwrap();
        assert!(lines[0].starts_with("Packet trace: 1 packets"), "{lines:?}");
        assert!(lines[1].starts_with("Packet trace canary: "), "{lines:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
