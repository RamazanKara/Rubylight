//! Settings' Connection test, as Android's `PyroWaveBandwidthTest`: first the delay to the PC,
//! [`PROBES`] serverinfo requests over HTTP [`PROBE_INTERVAL_MS`] apart, each given
//! [`PROBE_TIMEOUT_MS`] (Android's `NvHTTP.probeServerLatency`), then the download speed where
//! the PC has a speed test.
//!
//! The round trips are summed up by `nightjar_core::http::ConnectionTestStats` (Android's
//! `ConnectionTestStats`, which the remote access check shares): a lost probe counts as loss and
//! breaks the jitter sequence. What is here is the rest both platforms share: the plan, which
//! line follows the delay, and how the values read. Numbers come with a decimal point; the
//! caller swaps it for its language's comma and puts them in its translated templates
//! ("Ping {} · Jitter {} · Loss {}", "{} ms", "{} %").

/// Probes of the delay test.
pub const PROBES: u32 = 10;
/// The pause between two probes.
pub const PROBE_INTERVAL_MS: u64 = 100;
/// One probe, connect included: a probe that takes longer is lost.
pub const PROBE_TIMEOUT_MS: u64 = 2_000;
/// What a value nothing measured shows, in every language.
pub const UNAVAILABLE: &str = "–";

/// What follows the delay once its probes are done, in Android's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpeedStep {
    /// The PC has no speed test: the delay is the whole result, with no second line.
    Skip,
    /// "Speed not measured: pair this PC first".
    NeedsPairing,
    /// No probe answered: "Speed not measured: the PC didn’t answer".
    NoAnswer,
    /// Run the PC's speed test.
    Measure,
}

/// The step after the probes: a PC that can never measure speed gets the delay alone; one that
/// can is measured only once paired and when a probe answered.
pub fn speed_step(measures_speed: bool, paired: bool, latency_ms: Option<f64>) -> SpeedStep {
    if !measures_speed {
        SpeedStep::Skip
    } else if !paired {
        SpeedStep::NeedsPairing
    } else if latency_ms.is_none() {
        SpeedStep::NoAnswer
    } else {
        SpeedStep::Measure
    }
}

/// The result's second line: the speed, or why it was not measured.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Speed {
    /// Megabits a second received; `link_mbps` is the PC's own link where it reports one (0 for
    /// none), shown beside it.
    Measured { mbps: f64, link_mbps: u32 },
    NeedsPairing,
    NoAnswer,
    /// The PC answered that it has no speed test after all.
    Unsupported,
    /// The speed test, or the request before it, failed.
    Failed,
}

impl Speed {
    /// The line for a step that measures nothing; `None` for [`SpeedStep::Skip`] and
    /// [`SpeedStep::Measure`].
    pub fn of_step(step: SpeedStep) -> Option<Self> {
        match step {
            SpeedStep::NeedsPairing => Some(Self::NeedsPairing),
            SpeedStep::NoAnswer => Some(Self::NoAnswer),
            SpeedStep::Skip | SpeedStep::Measure => None,
        }
    }
}

/// `value` with `decimals` digits after the point, rounded half up as Java's `%.Nf` rounds.
fn fixed(value: f64, decimals: i32) -> String {
    let scale = 10f64.powi(decimals);
    format!("{:.*}", decimals as usize, (value * scale).round() / scale)
}

/// A delay as the result shows it: whole milliseconds, with one decimal below 10 ms where a tenth
/// still tells two links apart. `None` when nothing was measured ([`UNAVAILABLE`]).
pub fn milliseconds(value: Option<f64>) -> Option<String> {
    let value = value.filter(|v| v.is_finite())?;
    Some(fixed(value, if value < 10.0 { 1 } else { 0 }))
}

/// A loss with one decimal; `None` when nothing was measured.
pub fn percent(value: Option<f64>) -> Option<String> {
    value.filter(|v| v.is_finite()).map(|value| fixed(value, 1))
}

/// A speed with one decimal, as Android's `%1$.1f Mbps`.
pub fn megabits(value: f64) -> String {
    fixed(value, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_is_androids() {
        assert_eq!((PROBES, PROBE_INTERVAL_MS, PROBE_TIMEOUT_MS), (10, 100, 2_000));
        assert_eq!(UNAVAILABLE, "–");
    }

    // The cases of Android's ConnectionTestResultTest: "Ping 63 ms · Jitter 41 ms · Loss 0.0%",
    // "Ping 2.5 ms · Jitter 1.0 ms · Loss 25.0%" and "Ping – · Jitter – · Loss 100.0%".
    #[test]
    fn delays_read_whole_with_a_tenth_below_ten_milliseconds() {
        assert_eq!(milliseconds(Some(63.0)).as_deref(), Some("63"));
        assert_eq!(milliseconds(Some(41.0)).as_deref(), Some("41"));
        assert_eq!(milliseconds(Some(2.5)).as_deref(), Some("2.5"));
        assert_eq!(milliseconds(Some(1.0)).as_deref(), Some("1.0"));
        assert_eq!(milliseconds(Some(0.0)).as_deref(), Some("0.0"));
        assert_eq!(milliseconds(Some(9.94)).as_deref(), Some("9.9"));
        assert_eq!(milliseconds(Some(10.0)).as_deref(), Some("10"));
        assert_eq!(milliseconds(None), None);
        assert_eq!(milliseconds(Some(f64::NAN)), None);
        assert_eq!(percent(Some(0.0)).as_deref(), Some("0.0"));
        assert_eq!(percent(Some(25.0)).as_deref(), Some("25.0"));
        assert_eq!(percent(Some(100.0)).as_deref(), Some("100.0"));
        assert_eq!(percent(Some(100.0 / 3.0)).as_deref(), Some("33.3"));
        assert_eq!(percent(None), None);
        assert_eq!(megabits(312.4), "312.4");
        assert_eq!(megabits(87.0), "87.0");
    }

    #[test]
    fn halves_round_up_as_javas_format_does() {
        assert_eq!(milliseconds(Some(62.5)).as_deref(), Some("63"));
        assert_eq!(milliseconds(Some(2.25)).as_deref(), Some("2.3"));
        assert_eq!(percent(Some(12.25)).as_deref(), Some("12.3"));
    }

    #[test]
    fn the_speed_line_follows_androids_order() {
        assert_eq!(speed_step(false, false, None), SpeedStep::Skip, "no speed test, no second line");
        assert_eq!(speed_step(false, true, Some(5.0)), SpeedStep::Skip);
        assert_eq!(speed_step(true, false, Some(5.0)), SpeedStep::NeedsPairing, "pairing comes first");
        assert_eq!(speed_step(true, false, None), SpeedStep::NeedsPairing);
        assert_eq!(speed_step(true, true, None), SpeedStep::NoAnswer);
        assert_eq!(speed_step(true, true, Some(5.0)), SpeedStep::Measure);
        assert_eq!(Speed::of_step(SpeedStep::NeedsPairing), Some(Speed::NeedsPairing));
        assert_eq!(Speed::of_step(SpeedStep::NoAnswer), Some(Speed::NoAnswer));
        assert_eq!(Speed::of_step(SpeedStep::Skip), None);
        assert_eq!(Speed::of_step(SpeedStep::Measure), None);
    }
}
