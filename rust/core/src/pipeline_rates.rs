//! Pipeline frame rates for the console: how many frames each host stage
//! handled, the same numbers control message 0x553D reports to a client
//! (`rubylight_protocol::pipeline_rates`). Defined for 3.0; nothing measures
//! them yet.
use rubylight_protocol::pipeline_rates::{PipelineRatesReport, UNKNOWN};
use serde::Serialize;

/// One window's rates in millihertz; `None` (JSON `null`) for a stage the host
/// can't count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct PipelineRates {
    pub window_ms: u32,
    /// Frames the game or desktop presented.
    pub source_mhz: Option<u32>,
    /// Frames captured.
    pub capture_mhz: Option<u32>,
    /// Frames submitted to the encoder.
    pub submit_mhz: Option<u32>,
    /// Encoded frames sent.
    pub output_mhz: Option<u32>,
    /// Frames resent without new content.
    pub repeat_mhz: Option<u32>,
}

impl From<&PipelineRatesReport> for PipelineRates {
    fn from(report: &PipelineRatesReport) -> Self {
        let known = |rate: u32| (rate != UNKNOWN).then_some(rate);
        Self {
            window_ms: report.window_ms,
            source_mhz: known(report.source_mhz),
            capture_mhz: known(report.capture_mhz),
            submit_mhz: known(report.submit_mhz),
            output_mhz: known(report.output_mhz),
            repeat_mhz: known(report.repeat_mhz),
        }
    }
}

/// `frames` over `window_ms` in millihertz, saturating; [`UNKNOWN`] for an
/// empty window.
pub fn millihertz(frames: u64, window_ms: u32) -> u32 {
    if window_ms == 0 {
        return UNKNOWN;
    }
    let rate = u128::from(frames) * 1_000_000 / u128::from(window_ms);
    u32::try_from(rate).unwrap_or(UNKNOWN - 1).min(UNKNOWN - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_stages_are_null_in_the_console_json() {
        let report = PipelineRatesReport {
            short_window: false,
            window_ms: 1000,
            source_mhz: 59_940,
            capture_mhz: 60_000,
            submit_mhz: UNKNOWN,
            output_mhz: 60_000,
            repeat_mhz: 0,
        };
        assert_eq!(
            serde_json::to_value(PipelineRates::from(&report)).unwrap(),
            serde_json::json!({
                "window_ms": 1000,
                "source_mhz": 59940,
                "capture_mhz": 60000,
                "submit_mhz": null,
                "output_mhz": 60000,
                "repeat_mhz": 0,
            })
        );
    }

    #[test]
    fn millihertz_counts_frames_per_window() {
        assert_eq!(millihertz(60, 1000), 60_000);
        assert_eq!(millihertz(599, 10_000), 59_900);
        assert_eq!(millihertz(0, 1000), 0);
        assert_eq!(millihertz(1, 0), UNKNOWN);
        // A count that would overflow stays below the unknown marker.
        assert_eq!(millihertz(u64::MAX, 1), UNKNOWN - 1);
    }
}
