//! PyroWave's runtime bitrate: [`BitrateController`] is Android's `PyroWaveBitrateController.java`
//! and [`minimum_kbps`] its `PyroWaveBitrateFloor.java`.
//!
//! The controller steers only from what this client measures. The pacing hints
//! (`pyrowaveLinkMbps`, `pyrowavePaceMbps`) go to the host at launch and stay the host's source;
//! nothing here probes the link in their place.

use serde_json::Value;

use crate::stats::{known, Stats};
use crate::trace::{Args, Subject};

/// The highest bitrate a runtime request may ask for (`StreamConfiguration.MAX_BITRATE_KBPS`).
pub const MAX_RUNTIME_KBPS: u32 = 2_000_000;

/// Lowers PyroWave's bitrate on congestion or heat and raises it slowly after a healthy stretch.
#[derive(Clone, Debug, PartialEq)]
pub struct BitrateController {
    ceiling_kbps: u32,
    current_kbps: u32,
    floor_kbps: u32,
    frame_ms: f32,
    last_change_ms: u64,
    healthy_since_ms: Option<u64>,
    congested_samples: u32,
    previous_loss: Option<f32>,
    previous_queue_ms: Option<f32>,
}

impl BitrateController {
    pub fn new(ceiling_kbps: u32, current_kbps: u32, fps: u32, now_ms: u64) -> Self {
        let ceiling_kbps = ceiling_kbps.min(MAX_RUNTIME_KBPS);
        let current_kbps = current_kbps.min(ceiling_kbps);
        Self {
            ceiling_kbps,
            current_kbps,
            floor_kbps: current_kbps.min(10_000),
            frame_ms: 1000.0 / fps as f32,
            last_change_ms: now_ms,
            healthy_since_ms: None,
            congested_samples: 0,
            previous_loss: None,
            previous_queue_ms: None,
        }
    }

    /// Call once per second while no runtime request is pending. Returns the bitrate to request,
    /// or `None` to keep the current one. The loss includes recovered frames' missing records.
    /// Warm (thermal level 1) holds the rate, hot (2 and up) steps it down.
    pub fn sample(&mut self, stats: &Stats) -> Option<u32> {
        let now_ms = stats.now_ms;
        // Missing (negative) and NaN measurements are unknown, as in Java.
        let s = Stats {
            loss_percent: known(stats.loss_percent),
            queue_ms: known(stats.queue_ms),
            decode_ms: known(stats.decode_ms),
            jitter_ms: known(stats.jitter_ms),
            ..*stats
        };
        if !s.recent_video {
            self.healthy_since_ms = None;
            self.forget_previous();
            if !s.poor_connection {
                self.congested_samples = 0;
                return None;
            }
        }
        let frame_ms = self.frame_ms;
        let congested =
            s.poor_connection
                || (s.recent_video
                    && (s.loss_percent.is_some_and(|loss| loss >= 2.0)
                        || s.decode_ms.is_some_and(|decode| decode >= frame_ms)
                        || s.jitter_ms
                            .is_some_and(|jitter| jitter >= 5f32.max(frame_ms * 0.75))
                        || s.queue_ms
                            .is_some_and(|queue| queue >= 20f32.max(frame_ms * 2.0))
                        || self.previous_loss.zip(s.loss_percent).is_some_and(
                            |(previous, loss)| loss >= 0.5 && loss - previous >= 0.25,
                        )
                        || self.previous_queue_ms.zip(s.queue_ms).is_some_and(
                            |(previous, queue)| {
                                queue >= 8f32.max(frame_ms * 0.75) && queue - previous >= 3.0
                            },
                        )));
        self.previous_loss = s.loss_percent;
        self.previous_queue_ms = s.queue_ms;
        let since_change = now_ms.saturating_sub(self.last_change_ms);
        if congested || (s.recent_video && s.thermal_level >= 2) {
            self.healthy_since_ms = None;
            // Heat builds and fades slowly, so heat alone steps down less often and not as far.
            let floor = if congested {
                self.floor_kbps
            } else {
                self.floor_kbps.max(self.ceiling_kbps / 2)
            };
            self.congested_samples = self.congested_samples.saturating_add(1);
            if self.congested_samples >= 2 && since_change >= if congested { 3000 } else { 15000 } {
                let target = floor.max(self.current_kbps * 85 / 100);
                return (target < self.current_kbps && target > 0).then_some(target);
            }
        } else {
            self.congested_samples = 0;
            // Unknown measurements are not healthy.
            let healthy = s.thermal_level == 0
                && s.loss_percent.is_some_and(|loss| loss < 0.1)
                && s.queue_ms
                    .is_some_and(|queue| queue < 4f32.max(frame_ms / 2.0))
                && s.decode_ms.is_some_and(|decode| decode < frame_ms * 0.75)
                && s.jitter_ms
                    .is_some_and(|jitter| jitter < 5f32.max(frame_ms * 0.25));
            match self.healthy_since_ms {
                _ if !healthy => self.healthy_since_ms = None,
                None => self.healthy_since_ms = Some(now_ms),
                Some(since) => {
                    if now_ms.saturating_sub(since) >= 20_000 && since_change >= 20_000 {
                        let step = 250.max(self.current_kbps / 50);
                        let target = self.ceiling_kbps.min(self.current_kbps + step);
                        return (target > self.current_kbps).then_some(target);
                    }
                }
            }
        }
        None
    }

    /// The host's answer to a request. Less than asked for becomes the new ceiling.
    pub fn applied(&mut self, requested_kbps: u32, applied_kbps: u32, now_ms: u64) {
        if applied_kbps < requested_kbps {
            self.ceiling_kbps = self.ceiling_kbps.min(applied_kbps);
        }
        self.current_kbps = applied_kbps.min(MAX_RUNTIME_KBPS);
        self.floor_kbps = self.floor_kbps.min(self.current_kbps);
        self.last_change_ms = now_ms;
        self.suspend();
    }

    /// Forgets the healthy stretch and congestion, for example while a menu covers the stream.
    pub fn suspend(&mut self) {
        self.healthy_since_ms = None;
        self.congested_samples = 0;
        self.forget_previous();
    }

    fn forget_previous(&mut self) {
        self.previous_loss = None;
        self.previous_queue_ms = None;
    }
}

/// The regression covers 720p to 4K pixel counts; sizes outside use the nearest end.
const MIN_PIXELS: u64 = 1280 * 720;
const MAX_PIXELS: u64 = 3840 * 2160;

// Bytes per frame as a polynomial in sqrt(megapixels) - 2: the 30 dB, height factor 2.00 rows.
#[allow(clippy::excessive_precision)]
const YUV420: [f64; 8] = [
    309.25055550415766,
    8.351816219985253,
    -9.905584693249878,
    172.18790660175173,
    -64.17408945040484,
    -201.4015992260083,
    46.22089684565188,
    105.90424439986334,
];
#[allow(clippy::excessive_precision)]
const YUV444: [f64; 8] = [
    330.65932695689867,
    -12.487508056245487,
    13.060737308248315,
    139.64851902220505,
    -97.97414051841574,
    -112.81552415088743,
    57.365194646059074,
    57.19110311397152,
];

// The author allows 1.2x for HDR10.
const HDR_FACTOR: f64 = 1.2;

/// The lowest bitrate PyroWave still holds together at, in Kbps rounded up to a whole Mbps.
///
/// From its author's objective bitrate regression (Themaister/pyrowave,
/// eval-results/pyrowave_regression_results.h, MIT): its lowest level, 30 dB PSNR-HVS-M-H at a
/// viewing distance of twice the screen height. PyroWave is intra-only, so below this every frame
/// visibly falls apart instead of just softening.
pub fn minimum_kbps(width: u32, height: u32, fps: u32, yuv444: bool, hdr: bool) -> u32 {
    let pixels = (u64::from(width) * u64::from(height)).clamp(MIN_PIXELS, MAX_PIXELS);
    let x = (pixels as f64 * 1e-6).sqrt() - 2.0;
    let coefficients = if yuv444 { &YUV444 } else { &YUV420 };
    let (mut estimate, mut power) = (0.0, 1.0);
    for coefficient in coefficients {
        estimate += power * coefficient;
        power *= x;
    }
    let factor = if hdr { HDR_FACTOR } else { 1.0 };
    let mbps = estimate * 8e-3 * f64::from(fps.max(1)) * factor;
    // Clamped so an absurd frame rate cannot overflow the Kbps; Java's int would wrap.
    let mbps = mbps.ceil().clamp(0.0, f64::from(u32::MAX / 1000));
    mbps as u32 * 1000
}

pub(crate) fn traced_floor(args: &Value) -> Result<Box<dyn Subject>, String> {
    match args {
        Value::Null => Ok(Box::new(TracedFloor)),
        _ => Err("the floor has no instance (new must be null)".into()),
    }
}

pub(crate) fn traced_controller(args: &Value) -> Result<Box<dyn Subject>, String> {
    let a = Args::new(args, 4)?;
    let controller = BitrateController::new(a.int(0)?, a.int(1)?, a.int(2)?, a.int(3)?);
    Ok(Box::new(TracedController(controller, 0)))
}

// Java sets the thermal level apart from the samples; the adapter carries it into each tick.
struct TracedController(BitrateController, u32);

// Args reads only integers and text; samples also carry floats and flags. A float reaches the
// controller as Java passes it, -1 for a missing measurement included.
fn float(args: &Value, i: usize) -> Result<Option<f32>, String> {
    args[i]
        .as_f64()
        .map(|v| Some(v as f32))
        .ok_or_else(|| format!("argument {i} is not a number: {}", args[i]))
}

fn flag(args: &Value, i: usize) -> Result<bool, String> {
    args[i]
        .as_bool()
        .ok_or_else(|| format!("argument {i} is not a boolean: {}", args[i]))
}

impl Subject for TracedController {
    fn call(&mut self, method: &str, args: &Value) -> Result<Value, String> {
        let TracedController(controller, thermal_level) = self;
        match method {
            "sample" => {
                // Java's six-argument overload leaves jitter at 0.
                let count = args.as_array().map_or(0, Vec::len);
                let a = Args::new(args, if count == 6 { 6 } else { 7 })?;
                let stats = Stats {
                    now_ms: a.int(0)?,
                    recent_video: flag(args, 1)?,
                    poor_connection: flag(args, 2)?,
                    loss_percent: float(args, 3)?,
                    queue_ms: float(args, 4)?,
                    decode_ms: float(args, 5)?,
                    jitter_ms: if count == 6 {
                        Some(0.0)
                    } else {
                        float(args, 6)?
                    },
                    thermal_level: *thermal_level,
                    ..Stats::default()
                };
                Ok(controller.sample(&stats).unwrap_or(0).into())
            }
            "set_thermal_level" => {
                *thermal_level = Args::new(args, 1)?.int(0)?;
                Ok(Value::Null)
            }
            "applied" => {
                let a = Args::new(args, 3)?;
                controller.applied(a.int(0)?, a.int(1)?, a.int(2)?);
                Ok(Value::Null)
            }
            "suspend" => {
                Args::new(args, 0)?;
                controller.suspend();
                Ok(Value::Null)
            }
            _ => Err(format!("unknown method {method}")),
        }
    }
}

struct TracedFloor;

impl Subject for TracedFloor {
    fn call(&mut self, method: &str, args: &Value) -> Result<Value, String> {
        match method {
            "minimum_kbps" => {
                let a = Args::new(args, 5)?;
                let kbps = minimum_kbps(
                    a.int(0)?,
                    a.int(1)?,
                    a.int(2)?,
                    flag(args, 3)?,
                    flag(args, 4)?,
                );
                Ok(kbps.into())
            }
            _ => Err(format!("unknown method {method}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tick(second: u64) -> Stats {
        Stats {
            now_ms: second * 1000,
            recent_video: true,
            ..Stats::default()
        }
    }

    #[test]
    fn an_absurd_frame_rate_saturates_the_floor() {
        assert_eq!(minimum_kbps(3840, 2160, u32::MAX, true, true), u32::MAX / 1000 * 1000);
        assert!(minimum_kbps(3840, 2160, 1_000_000, true, true) > MAX_RUNTIME_KBPS);
    }

    #[test]
    fn a_tick_with_nothing_measured_never_raises_the_rate() {
        let mut controller = BitrateController::new(20_000, 10_000, 60, 0);
        assert!((1..=60).all(|second| controller.sample(&tick(second)).is_none()));
        let healthy = |second| Stats {
            loss_percent: Some(0.0),
            queue_ms: Some(0.0),
            decode_ms: Some(2.0),
            jitter_ms: Some(1.0),
            ..tick(second)
        };
        let raised = (61..=120).find_map(|second| controller.sample(&healthy(second)));
        assert!(raised.is_some_and(|kbps| kbps > 10_000), "{raised:?}");
    }
}
