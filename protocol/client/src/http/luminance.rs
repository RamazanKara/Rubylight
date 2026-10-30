//! The display's luminance as Sunshine-family hosts read it from the launch query
//! (`minBrightness`, `maxBrightness`, `maxAverageBrightness`). A Rubylight host gets it in the
//! DisplayCaps control message instead. Ported from Moonlight V+ (HdrBrightness.kt, 3e954d8;
//! nvhttp.cpp), the same rules as Android's `HdrBrightness`.

/// The SDR white an HDR/SDR ratio is relative to when the peak is estimated from it.
const ASSUMED_SDR_WHITE_NITS: f32 = 300.0;

/// One complete, consistent luminance report in nits. Unknown values give no report: the host
/// then picks its own fallback.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplayLuminance {
    pub max_nits: f32,
    pub min_nits: f32,
    pub max_full_frame_nits: f32,
    /// The peak is 300 nits times the HDR/SDR ratio, not what the display reported.
    pub estimated_from_ratio: bool,
}

// Every value is finite by construction, so equality is total.
impl Eq for DisplayLuminance {}

fn within(value: f32, low: f32, high: f32) -> bool {
    value.is_finite() && value >= low && value <= high
}

/// A ratio is usable only when finite and above 1 (1 is SDR-equal: no headroom).
fn usable_ratio(ratio: f32) -> Option<f32> {
    (ratio.is_finite() && ratio > 1.0).then_some(ratio)
}

impl DisplayLuminance {
    /// A complete, consistent report, or `None` when any field is unknown or contradicts another.
    pub fn new(max_nits: f32, min_nits: f32, max_full_frame_nits: f32) -> Option<Self> {
        if !within(max_nits, 1.0, 10_000.0)
            || !within(min_nits, 0.0, 100.0)
            || !within(max_full_frame_nits, 1.0, max_nits)
            || min_nits > max_full_frame_nits
        {
            return None;
        }
        Some(Self { max_nits, min_nits, max_full_frame_nits, estimated_from_ratio: false })
    }

    /// What the display reports. A complete report is sent as it is. Only a missing or invalid
    /// peak is estimated, from the HDR/SDR ratio times an assumed 300-nit SDR white; the estimate
    /// keeps a known black level and full-frame level, and fills unknown ones with 0.001 nits and
    /// 20% of the peak. Anything else is left to the host. `hdr_sdr_ratio` is NaN when unknown.
    pub fn from_display(max_nits: f32, min_nits: f32, max_full_frame_nits: f32, hdr_sdr_ratio: f32) -> Option<Self> {
        if within(max_nits, 1.0, 10_000.0) {
            return Self::new(max_nits, min_nits, max_full_frame_nits);
        }
        let estimated_peak = ASSUMED_SDR_WHITE_NITS * usable_ratio(hdr_sdr_ratio)?;
        if !within(estimated_peak, 1.0, 10_000.0) {
            return None;
        }
        let full_frame_known = within(max_full_frame_nits, 1.0, 10_000.0);
        let peak = estimated_peak.max(if full_frame_known { max_full_frame_nits } else { 0.0 });
        let minimum = if within(min_nits, 0.0, 100.0) { min_nits } else { 0.001 };
        let full_frame = if full_frame_known { max_full_frame_nits } else { minimum.max(peak * 0.2) };
        // Known fields are kept; contradictory system values still fall back to the host.
        Self::new(peak, minimum, full_frame).map(|report| Self { estimated_from_ratio: true, ..report })
    }

    /// The launch parameters, in Android's order. `{:?}` keeps a decimal point, as Java's
    /// `Float.toString` does, with a locale-independent separator.
    pub fn query_parameters(&self) -> String {
        format!(
            "&minBrightness={:?}&maxBrightness={:?}&maxAverageBrightness={:?}",
            self.min_nits, self.max_nits, self.max_full_frame_nits
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_complete_report_is_sent_as_it_is() {
        let report = DisplayLuminance::from_display(1000.0, 0.005, 400.0, 4.0).unwrap();
        assert_eq!(report, DisplayLuminance { max_nits: 1000.0, min_nits: 0.005, max_full_frame_nits: 400.0, estimated_from_ratio: false });
        assert_eq!(report.query_parameters(), "&minBrightness=0.005&maxBrightness=1000.0&maxAverageBrightness=400.0");
    }

    #[test]
    fn a_missing_peak_is_estimated_from_the_ratio() {
        let report = DisplayLuminance::from_display(0.0, f32::NAN, f32::NAN, 5.0).unwrap();
        assert_eq!((report.max_nits, report.min_nits, report.max_full_frame_nits), (1500.0, 0.001, 300.0));
        assert!(report.estimated_from_ratio);
        // A known full-frame level is a lower bound for the peak.
        let bright = DisplayLuminance::from_display(f32::NAN, 0.0, 800.0, 2.0).unwrap();
        assert_eq!((bright.max_nits, bright.max_full_frame_nits), (800.0, 800.0));
    }

    #[test]
    fn unknown_or_contradictory_values_are_left_to_the_host() {
        assert_eq!(DisplayLuminance::from_display(0.0, 0.0, 0.0, f32::NAN), None);
        assert_eq!(DisplayLuminance::from_display(0.0, 0.0, 0.0, 1.0), None);
        assert_eq!(DisplayLuminance::from_display(1000.0, f32::NAN, 400.0, 5.0), None);
        assert_eq!(DisplayLuminance::from_display(1000.0, 0.01, 1200.0, 5.0), None);
        assert_eq!(DisplayLuminance::from_display(20_000.0, 0.01, 400.0, f32::INFINITY), None);
    }
}
