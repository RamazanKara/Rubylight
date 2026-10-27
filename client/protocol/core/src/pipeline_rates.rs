//! Pipeline frame rates: how many frames per second each host stage handled, for a client's
//! statistics overlay (control message 0x553D).
//!
//! The client asks with a [`PipelineRatesRequest`]; the host then sends a
//! [`PipelineRatesReport`] every `interval_ms`. The rates are in millihertz over `window_ms`:
//! the frames the game or desktop presented, frames captured, frames submitted to the
//! encoder, encoded frames sent, and frames resent without new content. [`UNKNOWN`] marks a
//! stage the host can't count.
//!
//! Request, little-endian: version u8, enabled u8 (0 or 1), interval_ms u16 (500-10000).
//! Report: version u8, flags u8 (bit 0: the window is shorter than the interval), reserved
//! u16, window_ms u32, then the five rates as u32.

/// Control-stream message type of both directions.
pub const PIPELINE_RATES_MESSAGE_TYPE: u16 = 0x553D;
/// Encoded size of a [`PipelineRatesRequest`].
pub const PIPELINE_RATES_REQUEST_BYTES: usize = 4;
/// Encoded size of a [`PipelineRatesReport`].
pub const PIPELINE_RATES_REPORT_BYTES: usize = 28;
/// Shortest and longest report interval a client may ask for.
pub const MIN_INTERVAL_MS: u16 = 500;
pub const MAX_INTERVAL_MS: u16 = 10_000;
/// A rate the host can't measure.
pub const UNKNOWN: u32 = u32::MAX;
/// Serverinfo element: the host sends reports.
pub const SERVERINFO_FIELD: &str = "PipelineRatesVersion";

const VERSION: u8 = 1;

/// Client to host: start or stop reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PipelineRatesRequest {
    pub enabled: bool,
    pub interval_ms: u16,
}

impl PipelineRatesRequest {
    pub fn encode(&self) -> [u8; PIPELINE_RATES_REQUEST_BYTES] {
        let interval = self.interval_ms.to_le_bytes();
        [VERSION, u8::from(self.enabled), interval[0], interval[1]]
    }

    /// Parses a request; version 0, an `enabled` other than 0 or 1 and an interval outside
    /// 500-10000 ms reject it.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < PIPELINE_RATES_REQUEST_BYTES || data[0] == 0 || data[1] > 1 {
            return None;
        }
        let interval_ms = u16::from_le_bytes([data[2], data[3]]);
        (MIN_INTERVAL_MS..=MAX_INTERVAL_MS)
            .contains(&interval_ms)
            .then_some(Self {
                enabled: data[1] == 1,
                interval_ms,
            })
    }
}

/// Host to client: the stage rates over the last window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PipelineRatesReport {
    /// The window is shorter than the asked interval (the stream just started or changed).
    pub short_window: bool,
    pub window_ms: u32,
    pub source_mhz: u32,
    pub capture_mhz: u32,
    pub submit_mhz: u32,
    pub output_mhz: u32,
    pub repeat_mhz: u32,
}

impl PipelineRatesReport {
    const SHORT_WINDOW: u8 = 1;

    pub fn encode(&self) -> [u8; PIPELINE_RATES_REPORT_BYTES] {
        let mut out = [0; PIPELINE_RATES_REPORT_BYTES];
        out[0] = VERSION;
        out[1] = if self.short_window {
            Self::SHORT_WINDOW
        } else {
            0
        };
        let fields = [
            self.window_ms,
            self.source_mhz,
            self.capture_mhz,
            self.submit_mhz,
            self.output_mhz,
            self.repeat_mhz,
        ];
        for (index, field) in fields.iter().enumerate() {
            out[4 + index * 4..8 + index * 4].copy_from_slice(&field.to_le_bytes());
        }
        out
    }

    /// Parses a report; version 0 and an empty window reject it. Bytes past the 28 are
    /// ignored.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < PIPELINE_RATES_REPORT_BYTES || data[0] == 0 {
            return None;
        }
        let u32_at =
            |at: usize| u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
        let report = Self {
            short_window: data[1] & Self::SHORT_WINDOW != 0,
            window_ms: u32_at(4),
            source_mhz: u32_at(8),
            capture_mhz: u32_at(12),
            submit_mhz: u32_at(16),
            output_mhz: u32_at(20),
            repeat_mhz: u32_at(24),
        };
        (report.window_ms != 0).then_some(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip_and_refuse_bad_intervals() {
        let request = PipelineRatesRequest {
            enabled: true,
            interval_ms: 1000,
        };
        assert_eq!(request.encode(), [1, 1, 0xE8, 0x03]);
        assert_eq!(
            PipelineRatesRequest::decode(&request.encode()),
            Some(request)
        );
        for interval_ms in [MIN_INTERVAL_MS, MAX_INTERVAL_MS] {
            let stop = PipelineRatesRequest {
                enabled: false,
                interval_ms,
            };
            assert_eq!(PipelineRatesRequest::decode(&stop.encode()), Some(stop));
        }
        for interval_ms in [0, 499, 10_001, u16::MAX] {
            let request = PipelineRatesRequest {
                enabled: true,
                interval_ms,
            };
            assert_eq!(
                PipelineRatesRequest::decode(&request.encode()),
                None,
                "{interval_ms}"
            );
        }
        assert_eq!(PipelineRatesRequest::decode(&[1, 2, 0xE8, 0x03]), None);
        assert_eq!(PipelineRatesRequest::decode(&[0, 1, 0xE8, 0x03]), None);
        assert_eq!(PipelineRatesRequest::decode(&[1, 1, 0xE8]), None);
    }

    #[test]
    fn reports_round_trip() {
        let report = PipelineRatesReport {
            short_window: false,
            window_ms: 1000,
            source_mhz: 143_980,
            capture_mhz: 120_000,
            submit_mhz: 119_990,
            output_mhz: 119_990,
            repeat_mhz: UNKNOWN,
        };
        let wire = report.encode();
        assert_eq!(wire[..8], [1, 0, 0, 0, 0xE8, 0x03, 0, 0]);
        assert_eq!(wire[24..28], [0xFF; 4]);
        assert_eq!(PipelineRatesReport::decode(&wire), Some(report));
        let short = PipelineRatesReport {
            short_window: true,
            window_ms: 250,
            ..report
        };
        assert_eq!(short.encode()[1], 1);
        let mut longer = short.encode().to_vec();
        longer[0] = 2;
        longer.extend_from_slice(&[7; 4]);
        assert_eq!(PipelineRatesReport::decode(&longer), Some(short));
        assert_eq!(PipelineRatesReport::decode(&wire[..27]), None);
        let mut zero = wire;
        zero[0] = 0;
        assert_eq!(PipelineRatesReport::decode(&zero), None);
        let empty = PipelineRatesReport {
            window_ms: 0,
            ..report
        };
        assert_eq!(PipelineRatesReport::decode(&empty.encode()), None);
    }
}
