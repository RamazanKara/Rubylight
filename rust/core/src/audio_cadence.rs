//! A per-packet timing trace of the endpoint capture and the sends (`audio_cadence_trace`, hidden,
//! off by default), for telling an endpoint's uneven delivery from a late host.

use std::collections::VecDeque;
use std::io::Write;

/// Entries each trace keeps, the newest: over ten minutes of 5 ms packets.
pub const TRACE_ENTRIES: usize = 131_072;

/// One `GetBuffer` packet: when it was read, the device's own position and QPC for its first
/// frame, its length and flags.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Captured {
    pub read_ms: f64,
    pub device_qpc_ms: f64,
    pub position: u64,
    pub frames: u32,
    pub flags: u32,
}

/// One packet handed to the socket: when, and where it sits in the stream in 48 kHz samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sent {
    pub send_ms: f64,
    pub position: u64,
}

/// The newest [`TRACE_ENTRIES`] captured and sent packets. Times are QPC milliseconds, the clock
/// `GetBuffer` reports its QPC position in.
#[derive(Debug, Default)]
pub struct Trace {
    pub captured: VecDeque<Captured>,
    pub sent: VecDeque<Sent>,
    limit: usize,
}

impl Trace {
    pub fn new() -> Self {
        Self::with_limit(TRACE_ENTRIES)
    }
    pub fn with_limit(limit: usize) -> Self {
        Self {
            captured: VecDeque::new(),
            sent: VecDeque::new(),
            limit: limit.max(1),
        }
    }
    pub fn capture(&mut self, entry: Captured) {
        if self.captured.len() == self.limit {
            self.captured.pop_front();
        }
        self.captured.push_back(entry);
    }
    pub fn send(&mut self, entry: Sent) {
        if self.sent.len() == self.limit {
            self.sent.pop_front();
        }
        self.sent.push_back(entry);
    }
    /// Both as one CSV ordered by time: `kind,time_ms,device_qpc_ms,position,frames,flags`;
    /// a send has no device QPC, frames or flags.
    pub fn write_csv(&self, out: &mut impl Write) -> std::io::Result<()> {
        writeln!(out, "kind,time_ms,device_qpc_ms,position,frames,flags")?;
        let (mut c, mut s) = (self.captured.iter().peekable(), self.sent.iter().peekable());
        loop {
            let capture_first = match (c.peek(), s.peek()) {
                (Some(a), Some(b)) => a.read_ms <= b.send_ms,
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (None, None) => return Ok(()),
            };
            if capture_first {
                let e = c.next().unwrap();
                writeln!(
                    out,
                    "capture,{:.4},{:.4},{},{},{}",
                    e.read_ms, e.device_qpc_ms, e.position, e.frames, e.flags
                )?;
            } else {
                let e = s.next().unwrap();
                writeln!(out, "send,{:.4},,{},,", e.send_ms, e.position)?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_trace_keeps_the_newest_entries_and_writes_them_in_time_order() {
        let mut trace = Trace::with_limit(2);
        for (n, at) in [0., 10., 20.].into_iter().enumerate() {
            trace.capture(Captured {
                read_ms: at,
                device_qpc_ms: at - 0.5,
                position: n as u64 * 480,
                frames: 480,
                flags: 0,
            });
            trace.send(Sent {
                send_ms: at + 0.1,
                position: n as u64 * 240,
            });
        }
        assert_eq!(trace.captured.len(), 2);
        assert_eq!(trace.captured[0].position, 480);
        assert_eq!(trace.sent[1].position, 480);
        let mut out = Vec::new();
        trace.write_csv(&mut out).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "kind,time_ms,device_qpc_ms,position,frames,flags\n\
             capture,10.0000,9.5000,480,480,0\n\
             send,10.1000,,240,,\n\
             capture,20.0000,19.5000,960,480,0\n\
             send,20.1000,,480,,\n"
        );
    }
}
