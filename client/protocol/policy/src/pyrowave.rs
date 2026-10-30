//! PyroWave's runtime bitrate: [`BitrateController`] is Android's `PyroWaveBitrateController.java`
//! and [`minimum_kbps`], [`default_kbps`] and [`start_kbps`] its `PyroWaveBitrateFloor.java`.
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

/// The lowest bitrate PyroWave still holds together at, in Kbps: the host's severe-detail-loss
/// floor (`pyrowave::minimum_kbps` in rust/core/src/pyrowave.rs), 2.5 bits per pixel per frame
/// below 1080 lines and 1.5 from 1080 up, the same for SDR and HDR, 4:2:0 and 4:4:4 (measured on
/// representative pictures, rust/PERFORMANCE.md). The host lowers PyroWave no further and warns
/// below it. PyroWave is intra-only, so below this every frame visibly falls apart instead of
/// just softening.
pub fn minimum_kbps(width: u32, height: u32, fps: u32) -> u32 {
    bitrate_kbps(width, height, fps, if height < 1080 { 25 } else { 15 })
}

/// The bitrate a PyroWave stream starts at when the user has not chosen one: the host's
/// clean-picture target (`pyrowave::recommended_kbps`), 5 bits per pixel per frame below 1080
/// lines and 3.2 from 1080 up, which is about 280 Mbps at 720p60 and 400 Mbps at 1080p60, the
/// starting points the client docs give. No more than a stream may ask for.
pub fn default_kbps(width: u32, height: u32, fps: u32) -> u32 {
    bitrate_kbps(width, height, fps, if height < 1080 { 50 } else { 32 }).min(MAX_RUNTIME_KBPS)
}

/// The bitrate a PyroWave stream starts at: [`default_kbps`] when the bitrate is automatic, else
/// the chosen one, raised to [`minimum_kbps`] (up to what a stream may ask for) when below it.
pub fn start_kbps(configured: u32, automatic: bool, width: u32, height: u32, fps: u32) -> u32 {
    if automatic {
        default_kbps(width, height, fps)
    } else {
        configured.max(minimum_kbps(width, height, fps).min(MAX_RUNTIME_KBPS))
    }
}

// The host's arithmetic at a whole frame rate: tenths of a bit per pixel per frame, rounded up.
fn bitrate_kbps(width: u32, height: u32, fps: u32, bpp_tenths: u32) -> u32 {
    (u128::from(width) * u128::from(height) * u128::from(fps) * u128::from(bpp_tenths))
        .div_ceil(10_000)
        .min(u128::from(u32::MAX)) as u32
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
                let a = Args::new(args, 3)?;
                Ok(minimum_kbps(a.int(0)?, a.int(1)?, a.int(2)?).into())
            }
            "default_kbps" => {
                let a = Args::new(args, 3)?;
                Ok(default_kbps(a.int(0)?, a.int(1)?, a.int(2)?).into())
            }
            "start_kbps" => {
                let a = Args::new(args, 5)?;
                let kbps = start_kbps(a.int(0)?, flag(args, 1)?, a.int(2)?, a.int(3)?, a.int(4)?);
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
        assert_eq!(minimum_kbps(3840, 2160, u32::MAX), u32::MAX);
        assert!(minimum_kbps(3840, 2160, 1_000_000) > MAX_RUNTIME_KBPS);
        assert_eq!(default_kbps(3840, 2160, u32::MAX), MAX_RUNTIME_KBPS);
        assert_eq!(start_kbps(5_000, false, 3840, 2160, u32::MAX), MAX_RUNTIME_KBPS);
    }

    // The host's own figures (rust/core/src/pyrowave.rs, quality_bitrates_scale_with_pixels_and_frame_rate),
    // so the client never starts PyroWave at a rate the host calls too low.
    #[test]
    fn the_floor_and_default_are_the_hosts() {
        assert_eq!(minimum_kbps(1280, 720, 60), 138_240);
        assert_eq!(minimum_kbps(1920, 1080, 60), 186_624);
        assert_eq!(minimum_kbps(3840, 2160, 60), 746_496);
        assert_eq!(minimum_kbps(1920, 1080, 30), 93_312);
        assert_eq!(minimum_kbps(1280, 720, 120), 276_480);
        assert_eq!(default_kbps(1280, 720, 60), 276_480);
        assert_eq!(default_kbps(1920, 1080, 60), 398_132);
        assert_eq!(default_kbps(3840, 2160, 60), 1_592_525);
        assert_eq!(default_kbps(1920, 1080, 120), 796_263);
        for (w, h, fps) in [(1280, 720, 60), (1920, 1080, 60), (2560, 1440, 120), (3840, 2160, 60)] {
            assert!(default_kbps(w, h, fps) > minimum_kbps(w, h, fps), "{w}x{h}@{fps}");
        }
    }

    #[test]
    fn a_stream_starts_at_the_default_or_a_chosen_rate_no_lower_than_the_floor() {
        // Automatic: the default, whatever the stored conventional default was.
        assert_eq!(start_kbps(20_000, true, 1920, 1080, 60), 398_132);
        assert_eq!(start_kbps(20_000, true, 2560, 1440, 120), 1_415_578);
        assert_eq!(start_kbps(20_000, true, 3840, 2160, 60), 1_592_525);
        assert_eq!(start_kbps(20_000, true, 3840, 2160, 120), MAX_RUNTIME_KBPS);
        // Chosen below the floor: raised to it. Above it: untouched, even past the default.
        assert_eq!(start_kbps(20_000, false, 1920, 1080, 60), 186_624);
        assert_eq!(start_kbps(200_000, false, 1920, 1080, 60), 200_000);
        assert_eq!(start_kbps(900_000, false, 1920, 1080, 60), 900_000);
        assert_eq!(start_kbps(MAX_RUNTIME_KBPS, false, 3840, 2160, 60), MAX_RUNTIME_KBPS);
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
