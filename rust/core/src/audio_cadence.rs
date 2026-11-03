//! How the endpoint capture is opened, for measuring the cadence its packets reach the wire, and
//! a per-packet timing trace of the capture and the sends. The switches are hidden (not in the
//! console); their defaults are the capture as it has always been opened.

use anyhow::{Result, bail};
use std::collections::VecDeque;
use std::io::Write;

/// The capture buffer Windows holds for a loopback stream when nothing else is asked, 100 ms.
pub const DEFAULT_BUFFER_US: u32 = 100_000;
/// Entries each trace keeps, the newest: over ten minutes of 5 ms packets.
pub const TRACE_ENTRIES: usize = 131_072;

/// The engine period the silent keep-alive render stream asks of the captured endpoint. Shared
/// mode runs the endpoint at the smallest period any of its streams asked for, and the loopback
/// tap delivers once per engine pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeepAlivePeriod {
    /// The endpoint's default period (`IAudioClient::Initialize`).
    Default,
    /// The driver's minimum shared-mode period (`IAudioClient3::InitializeSharedAudioStream`).
    Min,
}

impl KeepAlivePeriod {
    /// The period, in frames, to open the keep-alive with; None for the default `Initialize`.
    /// `min` and `fundamental` are what `GetSharedModeEnginePeriod` reported.
    pub fn frames(self, fundamental: u32, min: u32) -> Option<u32> {
        match self {
            Self::Default => None,
            Self::Min => Some(min.max(fundamental).max(1)),
        }
    }
}

/// How an endpoint capture is opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureOptions {
    /// `audio_capture_buffer_us`: the loopback stream's buffer duration; 0 asks for the minimum.
    pub buffer_us: u32,
    /// `audio_keepalive_period = default|min`.
    pub keepalive: KeepAlivePeriod,
    /// `audio_capture_event`: wake on Windows' capture event, else poll every millisecond.
    pub event: bool,
    /// `audio_cadence_trace`: record every captured and sent packet, written at stream end.
    pub trace: bool,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            buffer_us: DEFAULT_BUFFER_US,
            keepalive: KeepAlivePeriod::Default,
            event: true,
            trace: false,
        }
    }
}

impl CaptureOptions {
    pub fn from_config(config: &crate::config::Config) -> Result<Self> {
        let buffer = config.integer("audio_capture_buffer_us", i64::from(DEFAULT_BUFFER_US));
        if !(0..=1_000_000).contains(&buffer) {
            bail!("audio_capture_buffer_us must be 0 to 1000000, not {buffer}");
        }
        let keepalive = match config.get("audio_keepalive_period", "default") {
            "default" => KeepAlivePeriod::Default,
            "min" => KeepAlivePeriod::Min,
            other => bail!("audio_keepalive_period must be default or min, not {other}"),
        };
        Ok(Self {
            buffer_us: buffer as u32,
            keepalive,
            event: config.boolean("audio_capture_event", true),
            trace: config.boolean("audio_cadence_trace", false),
        })
    }
    /// The buffer duration in the 100 ns units `IAudioClient::Initialize` takes.
    pub fn buffer_hns(&self) -> i64 {
        i64::from(self.buffer_us) * 10
    }
}

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
    fn the_capture_opens_as_it_always_has_unless_a_switch_says_otherwise() {
        let config = crate::config::Config::parse("").unwrap();
        let options = CaptureOptions::from_config(&config).unwrap();
        assert_eq!(options, CaptureOptions::default());
        assert_eq!(options.buffer_hns(), 1_000_000);
        let config = crate::config::Config::parse(
            "audio_capture_buffer_us = 20000\naudio_keepalive_period = min\naudio_capture_event = false\naudio_cadence_trace = true\n",
        )
        .unwrap();
        let options = CaptureOptions::from_config(&config).unwrap();
        assert_eq!(
            options,
            CaptureOptions {
                buffer_us: 20_000,
                keepalive: KeepAlivePeriod::Min,
                event: false,
                trace: true,
            }
        );
    }

    #[test]
    fn a_bad_capture_switch_is_an_error_not_the_default() {
        for text in [
            "audio_keepalive_period = smallest",
            "audio_capture_buffer_us = -1",
            "audio_capture_buffer_us = 2000000",
        ] {
            let config = crate::config::Config::parse(text).unwrap();
            assert!(CaptureOptions::from_config(&config).is_err(), "{text}");
        }
    }

    #[test]
    fn the_minimum_keepalive_period_is_never_below_one_fundamental() {
        assert_eq!(KeepAlivePeriod::Default.frames(48, 128), None);
        assert_eq!(KeepAlivePeriod::Min.frames(48, 144), Some(144));
        assert_eq!(KeepAlivePeriod::Min.frames(480, 0), Some(480));
        assert_eq!(KeepAlivePeriod::Min.frames(0, 0), Some(1));
    }

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
