//! The once-a-second stats tick every policy reads.

/// One stats tick, passed by reference to every controller and the stream doctor. A measurement
/// is `None` when unknown, so [`Stats::default`] is a tick at time 0 with nothing measured: no
/// controller reads it as healthy.
///
/// Each policy also reads a negative or NaN measurement as unknown, as its Java original does,
/// but callers pass `None` instead. The trace adapters alone pass Java's -1 and NaN through.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    /// Monotonic time of the tick.
    pub now_ms: u64,
    /// Frames arrived in the last two seconds, so the counters below are current.
    pub recent_video: bool,
    /// The native connection-quality warning.
    pub poor_connection: bool,
    /// The stream's frame rate, 0 when unknown. Only the stream doctor reads it; the controllers
    /// keep the rate they were given.
    pub fps: u32,
    /// Frames lost after FEC, in percent.
    pub loss_percent: Option<f32>,
    pub decode_ms: Option<f32>,
    pub jitter_ms: Option<f32>,
    /// Network queueing delay above its recent minimum.
    pub queue_ms: Option<f32>,
    pub host_p95_ms: Option<f32>,
    pub display_p95_ms: Option<f32>,
    pub received_fps: Option<f32>,
    pub released_fps: Option<f32>,
    pub shown_fps: Option<f32>,
    /// True when `host_p95_ms` runs to the last packet sent, as the host's timing reports measure it.
    pub host_includes_send: bool,
    /// The thermal monitor's level: 0 normal (or no thermal reading), 1 warm, 2 and up hot.
    pub thermal_level: u32,
}

/// A measured value: at least 0, which also drops NaN.
pub(crate) fn known(value: Option<f32>) -> Option<f32> {
    value.filter(|v| *v >= 0.0)
}
