//! Automatic resolution and frame rate: a bounded ladder from the stream's size down to the
//! user's floors, stepped down under sustained pressure and back up after healthy time.
//!
//! [`AdaptiveQualityController`] is Android's (`AdaptiveQualityController.java`). Decisions do no
//! I/O: the caller sends the returned [`Target`] and reports it with
//! [`AdaptiveQualityController::applied`] once the request went out.

use std::fmt;

use serde_json::Value;

use crate::trace::{Args, Subject};

pub const DOWN_INTERVAL_MS: u64 = 15_000;
pub const RECOVER_MS: u64 = 30_000;
const STEP_RATES: [u32; 5] = [120, 90, 60, 45, 30];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Off,
    Motion,
    Detail,
    Battery,
}

impl Mode {
    /// The saved preference; anything unknown is off.
    pub fn parse(value: Option<&str>) -> Self {
        match value {
            Some("motion") => Self::Motion,
            Some("detail") => Self::Detail,
            Some("battery") => Self::Battery,
            _ => Self::Off,
        }
    }

    pub fn preference(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Motion => "motion",
            Self::Detail => "detail",
            Self::Battery => "battery",
        }
    }
}

/// One rung of the ladder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl Target {
    /// Width, height and the frame rate in millihertz, as the host's resize request takes them.
    pub fn request(&self) -> [u32; 3] {
        [self.width, self.height, self.fps.saturating_mul(1000)]
    }

    pub fn matches(&self, width: u32, height: u32, fps: u32) -> bool {
        (self.width, self.height, self.fps) == (width, height, fps)
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}@{}", self.width, self.height, self.fps)
    }
}

/// Observations from one bounded, non-overlapping decoder sampling interval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodeTiming {
    pub generation: i64,
    pub window_start_ns: i64,
    pub window_end_ns: i64,
    pub first_sample_ns: i64,
    pub last_sample_ns: i64,
    pub fps: i32,
    pub samples: i32,
    pub late_frames: i32,
}

/// Input-to-output latency includes pipeline depth, not just work on a single frame.
pub fn decode_budget_ms(fps: u32) -> f64 {
    2000.0 / f64::from(fps.max(1))
}

/// The user's floors and the decoder's size alignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub min_short_edge: u32,
    pub min_fps: u32,
    pub align_width: u32,
    pub align_height: u32,
}

/// One second of stream statistics. Negative metrics are unknown.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    pub now_ms: u64,
    pub recent_video: bool,
    pub poor_connection: bool,
    pub loss: f32,
    pub decode_ms: f32,
    pub jitter_ms: f32,
    pub queue_ms: f32,
    pub thermal_level: i32,
    pub bitrate_has_room: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QualityError {
    /// A zero width, height or frame rate.
    InvalidStream,
    /// A target that is not on the current ladder.
    ForeignTarget,
}

impl fmt::Display for QualityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidStream => "invalid stream size or FPS",
            Self::ForeignTarget => "target belongs to a different ceiling",
        })
    }
}

impl std::error::Error for QualityError {}

/// Bounded, opt-in resolution/FPS adaptation.
#[derive(Clone, Debug)]
pub struct AdaptiveQualityController {
    mode: Mode,
    bounds: Bounds,
    levels: Vec<Target>,
    level: usize,
    battery_ceiling: usize,
    last_change_ms: u64,
    healthy_since_ms: Option<u64>,
    device_pressure_samples: u32,
    network_pressure_samples: u32,
    tail_pressure_samples: u32,
    timing_generation: i64,
    timing_window_end_ns: i64,
    timing_tick_ms: Option<u64>,
}

impl AdaptiveQualityController {
    /// The ladder starts at `width`x`height`@`fps`. Floors below 240 lines, 15 FPS or an
    /// alignment of 2 are raised to them.
    pub fn new(
        mode: Mode,
        width: u32,
        height: u32,
        fps: u32,
        bounds: Bounds,
        now_ms: u64,
    ) -> Result<Self, QualityError> {
        let mut controller = Self {
            mode,
            bounds: Bounds {
                min_short_edge: bounds.min_short_edge.max(240),
                min_fps: bounds.min_fps.max(15),
                align_width: bounds.align_width.max(2),
                align_height: bounds.align_height.max(2),
            },
            levels: Vec::new(),
            level: 0,
            battery_ceiling: 0,
            last_change_ms: now_ms,
            healthy_since_ms: None,
            device_pressure_samples: 0,
            network_pressure_samples: 0,
            tail_pressure_samples: 0,
            // Java's Long.MIN_VALUE: no generation seen yet.
            timing_generation: i64::MIN,
            timing_window_end_ns: 0,
            timing_tick_ms: None,
        };
        controller.rebuild(width, height, fps)?;
        Ok(controller)
    }

    fn rebuild(&mut self, width: u32, height: u32, fps: u32) -> Result<(), QualityError> {
        if width == 0 || height == 0 || fps == 0 {
            return Err(QualityError::InvalidStream);
        }
        let Bounds {
            min_short_edge,
            min_fps,
            align_width,
            align_height,
        } = self.bounds;
        let short_edge = width.min(height);
        let floor = short_edge.min(min_short_edge);
        let mut sizes = vec![(width, height)];
        let mut next = short_edge;
        while next > floor {
            next = floor.max((u64::from(next) * 4 / 5) as u32);
            let scale = f64::from(next) / f64::from(short_edge);
            let size = (
                width.min(align_width.max(aligned(width, scale, align_width))),
                height.min(align_height.max(aligned(height, scale, align_height))),
            );
            if sizes.last() != Some(&size) {
                sizes.push(size);
            }
        }
        let floor_fps = fps.min(min_fps);
        let mut rates = vec![fps];
        rates.extend(STEP_RATES.into_iter().filter(|&r| r < fps && r > floor_fps));
        if rates.last() != Some(&floor_fps) {
            rates.push(floor_fps);
        }
        let mut levels = vec![Target { width, height, fps }];
        let (mut size, mut rate) = (0, 0);
        while size + 1 < sizes.len() || rate + 1 < rates.len() {
            let lower_size = match self.mode {
                Mode::Motion => size + 1 < sizes.len(),
                Mode::Detail => rate + 1 >= rates.len(),
                Mode::Off | Mode::Battery => {
                    size + 1 < sizes.len() && (rate + 1 >= rates.len() || levels.len() % 2 == 0)
                }
            };
            if lower_size {
                size += 1;
            } else {
                rate += 1;
            }
            let (width, height) = sizes[size];
            levels.push(Target {
                width,
                height,
                fps: rates[rate],
            });
        }
        let mut battery_ceiling = 0;
        if self.mode == Mode::Battery {
            // Battery mode deliberately caps work even on a healthy connection, within the
            // user's floors.
            while battery_ceiling + 1 < levels.len() {
                let target = levels[battery_ceiling];
                if target.fps <= floor_fps.max(60)
                    && target.width.min(target.height) <= floor.max(1080)
                {
                    break;
                }
                battery_ceiling += 1;
            }
        }
        self.level = self.level.min(levels.len() - 1);
        self.levels = levels;
        self.battery_ceiling = battery_ceiling;
        Ok(())
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn ceiling(&self) -> Target {
        self.levels[0]
    }

    pub fn current(&self) -> Target {
        self.levels[self.level]
    }

    /// A fold or window change keeps the current degradation level and the chosen user floors.
    /// Returns the new current target, or `None` when the ceiling did not change.
    pub fn resize_ceiling(
        &mut self,
        width: u32,
        height: u32,
        fps: u32,
        now_ms: u64,
    ) -> Result<Option<Target>, QualityError> {
        if self.ceiling().matches(width, height, fps) {
            return Ok(None);
        }
        self.rebuild(width, height, fps)?;
        self.suspend();
        self.last_change_ms = now_ms;
        Ok(Some(self.current()))
    }

    /// Decides without decoder timing; the tail neither adds pressure nor blocks recovery.
    pub fn sample(&mut self, sample: &Sample) -> Option<Target> {
        self.decide(sample, None, false)
    }

    /// A missing timing observation is unknown, never evidence that the decoder recovered.
    pub fn sample_timed(
        &mut self,
        sample: &Sample,
        timing: Option<&DecodeTiming>,
    ) -> Option<Target> {
        self.decide(sample, timing, true)
    }

    fn decide(
        &mut self,
        s: &Sample,
        timing: Option<&DecodeTiming>,
        observe_tail: bool,
    ) -> Option<Target> {
        if self.mode == Mode::Off || !s.recent_video {
            self.suspend();
            return None;
        }
        let fps = self.current().fps;
        let frame_ms = 1000.0f32 / fps as f32;
        let budget_ms = decode_budget_ms(fps);
        let timing = match observe_tail {
            true => self.accept_timing(s.now_ms, timing),
            false => None,
        };
        // A single keyframe cannot satisfy both the count and ratio. Four fresh intervals
        // must agree, so even a whole brief burst cannot be reused to force a reduction.
        let tail_pressure = timing.is_some_and(|t| {
            t.late_frames >= 3 && i64::from(t.late_frames) * 10 >= i64::from(t.samples)
        });
        self.tail_pressure_samples = count(tail_pressure, self.tail_pressure_samples);
        let device_pressure = s.thermal_level >= 2 || f64::from(s.decode_ms) >= budget_ms;
        let network_pressure =
            s.poor_connection || s.loss >= 3.0 || s.queue_ms >= 20f32.max(frame_ms * 2.0);
        self.device_pressure_samples = count(device_pressure, self.device_pressure_samples);
        self.network_pressure_samples = count(
            network_pressure && !s.bitrate_has_room,
            self.network_pressure_samples,
        );
        let battery_step = self.level < self.battery_ceiling;
        if device_pressure || tail_pressure || network_pressure || battery_step {
            self.healthy_since_ms = None;
            if s.now_ms.saturating_sub(self.last_change_ms) >= DOWN_INTERVAL_MS
                && self.level + 1 < self.levels.len()
                && (self.device_pressure_samples >= 4
                    || self.tail_pressure_samples >= 4
                    || self.network_pressure_samples >= 8
                    || battery_step)
            {
                return Some(self.levels[self.level + 1]);
            }
            return None;
        }
        let tail_healthy = !observe_tail
            || timing.is_some_and(|t| i64::from(t.late_frames) * 50 <= i64::from(t.samples));
        // A negative (unknown) queue counts as healthy.
        let healthy = tail_healthy
            && s.thermal_level == 0
            && s.loss >= 0.0
            && s.loss < 0.5
            && s.decode_ms >= 0.0
            && f64::from(s.decode_ms) < budget_ms * 0.75
            && s.jitter_ms >= 0.0
            && s.jitter_ms < 5f32.max(frame_ms * 0.25)
            && s.queue_ms < 4f32.max(frame_ms / 2.0);
        match self.healthy_since_ms {
            _ if !healthy => self.healthy_since_ms = None,
            None => self.healthy_since_ms = Some(s.now_ms),
            Some(since)
                if self.level > self.battery_ceiling
                    && s.now_ms.saturating_sub(since) >= RECOVER_MS
                    && s.now_ms.saturating_sub(self.last_change_ms) >= RECOVER_MS =>
            {
                return Some(self.levels[self.level - 1]);
            }
            Some(_) => {}
        }
        None
    }

    /// The timing when it is a fresh, plausible interval at the current frame rate.
    fn accept_timing<'a>(
        &mut self,
        now_ms: u64,
        timing: Option<&'a DecodeTiming>,
    ) -> Option<&'a DecodeTiming> {
        if self
            .timing_tick_ms
            .is_some_and(|tick| now_ms < tick || now_ms - tick > 2000)
        {
            self.reset_votes();
        }
        self.timing_tick_ms = Some(now_ms);
        let t = timing?;
        if t.generation != self.timing_generation {
            self.timing_generation = t.generation;
            self.timing_window_end_ns = 0;
            self.reset_votes();
        }
        let new_window = t.window_end_ns > self.timing_window_end_ns
            && t.window_start_ns >= self.timing_window_end_ns;
        self.timing_window_end_ns = self.timing_window_end_ns.max(t.window_end_ns);
        // A quiet host may send fewer frames than the negotiated rate. Require enough fresh
        // outputs spread over half a second, so pauses and short bursts still cannot recover.
        // Saturating differences reject the absurd spans Java's wrapping would accept.
        let minimum_samples = 12;
        let valid = new_window
            && i64::from(t.fps) == i64::from(self.current().fps)
            && t.window_start_ns >= 0
            && t.window_end_ns.saturating_sub(t.window_start_ns) <= 2_000_000_000
            && t.samples >= minimum_samples
            && t.late_frames >= 0
            && t.late_frames <= t.samples
            && t.first_sample_ns > t.window_start_ns
            && t.last_sample_ns <= t.window_end_ns
            && t.last_sample_ns.saturating_sub(t.first_sample_ns) >= 500_000_000
            && t.window_end_ns.saturating_sub(t.last_sample_ns) <= 500_000_000;
        valid.then_some(t)
    }

    /// Called only after the request was sent successfully; absent requests cannot advance the
    /// ladder. Targets compare by value, where Java requires the very object it returned.
    pub fn applied(&mut self, target: Target, now_ms: u64) -> Result<(), QualityError> {
        self.level = self
            .levels
            .iter()
            .position(|t| *t == target)
            .ok_or(QualityError::ForeignTarget)?;
        self.last_change_ms = now_ms;
        self.suspend();
        Ok(())
    }

    pub fn suspend(&mut self) {
        self.reset_votes();
        self.timing_tick_ms = None;
    }

    fn reset_votes(&mut self) {
        self.healthy_since_ms = None;
        self.device_pressure_samples = 0;
        self.network_pressure_samples = 0;
        self.tail_pressure_samples = 0;
    }
}

/// Rounds a scaled length up to the alignment, as Java's `(int) Math.ceil(..) * align` does.
fn aligned(length: u32, scale: f64, align: u32) -> u32 {
    ((f64::from(length) * scale / f64::from(align)).ceil() as u32).saturating_mul(align)
}

fn count(pressure: bool, samples: u32) -> u32 {
    match pressure {
        true => samples + 1,
        false => 0,
    }
}

/// Traces call it with plain values; Java's `AdaptiveQualityTrace` adapts the same calls.
struct Traced(Option<AdaptiveQualityController>);

pub(crate) fn traced_controller(args: &Value) -> Result<Box<dyn Subject>, String> {
    let controller = match args {
        Value::Null => None,
        _ => {
            let a = Args::new(args, 9)?;
            let bounds = Bounds {
                min_short_edge: a.int(4)?,
                min_fps: a.int(5)?,
                align_width: a.int(6)?,
                align_height: a.int(7)?,
            };
            let mode = Mode::parse(a.text(0)?);
            let controller = AdaptiveQualityController::new(
                mode,
                a.int(1)?,
                a.int(2)?,
                a.int(3)?,
                bounds,
                a.int(8)?,
            )
            .map_err(|e| e.to_string())?;
            Some(controller)
        }
    };
    Ok(Box::new(Traced(controller)))
}

fn float(args: &Value, i: usize) -> Result<f32, String> {
    args[i]
        .as_f64()
        .map(|v| v as f32)
        .ok_or_else(|| format!("argument {i} is not a number: {}", args[i]))
}

fn flag(args: &Value, i: usize) -> Result<bool, String> {
    args[i]
        .as_bool()
        .ok_or_else(|| format!("argument {i} is not a boolean: {}", args[i]))
}

/// The first nine arguments of every sample call.
fn traced_sample(args: &Value, a: &Args) -> Result<Sample, String> {
    Ok(Sample {
        now_ms: a.int(0)?,
        recent_video: flag(args, 1)?,
        poor_connection: flag(args, 2)?,
        loss: float(args, 3)?,
        decode_ms: float(args, 4)?,
        jitter_ms: float(args, 5)?,
        queue_ms: float(args, 6)?,
        thermal_level: a.int(7)?,
        bitrate_has_room: flag(args, 8)?,
    })
}

fn parse_target(text: &str) -> Option<Target> {
    let (width, rest) = text.split_once('x')?;
    let (height, fps) = rest.split_once('@')?;
    Some(Target {
        width: width.parse().ok()?,
        height: height.parse().ok()?,
        fps: fps.parse().ok()?,
    })
}

fn traced_target(target: Option<Target>) -> Value {
    target.map_or(Value::Null, |t| t.to_string().into())
}

impl Subject for Traced {
    fn call(&mut self, method: &str, args: &Value) -> Result<Value, String> {
        if method == "parse_mode" {
            let a = Args::new(args, 1)?;
            return Ok(Mode::parse(a.text(0)?).preference().into());
        }
        let controller = self.0.as_mut().ok_or("no instance (new is null)")?;
        match method {
            "sample" => {
                let a = Args::new(args, 9)?;
                let sample = traced_sample(args, &a)?;
                Ok(traced_target(controller.sample(&sample)))
            }
            "sample_without_timing" => {
                let a = Args::new(args, 9)?;
                let sample = traced_sample(args, &a)?;
                Ok(traced_target(controller.sample_timed(&sample, None)))
            }
            "sample_timed" => {
                let a = Args::new(args, 17)?;
                let sample = traced_sample(args, &a)?;
                let timing = DecodeTiming {
                    generation: a.int(9)?,
                    window_start_ns: a.int(10)?,
                    window_end_ns: a.int(11)?,
                    first_sample_ns: a.int(12)?,
                    last_sample_ns: a.int(13)?,
                    fps: a.int(14)?,
                    samples: a.int(15)?,
                    late_frames: a.int(16)?,
                };
                Ok(traced_target(
                    controller.sample_timed(&sample, Some(&timing)),
                ))
            }
            "applied" => {
                let a = Args::new(args, 2)?;
                let text = a.text(0)?.ok_or("argument 0 is null")?;
                let target = parse_target(text).ok_or_else(|| format!("bad target {text}"))?;
                controller
                    .applied(target, a.int(1)?)
                    .map_err(|e| e.to_string())?;
                Ok(Value::Null)
            }
            "resize_ceiling" => {
                let a = Args::new(args, 4)?;
                controller
                    .resize_ceiling(a.int(0)?, a.int(1)?, a.int(2)?, a.int(3)?)
                    .map(traced_target)
                    .map_err(|e| e.to_string())
            }
            "suspend" => {
                Args::new(args, 0)?;
                controller.suspend();
                Ok(Value::Null)
            }
            "current" => Args::new(args, 0).map(|_| controller.current().to_string().into()),
            "ceiling" => Args::new(args, 0).map(|_| controller.ceiling().to_string().into()),
            _ => Err(format!("unknown method {method}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDS: Bounds = Bounds {
        min_short_edge: 720,
        min_fps: 30,
        align_width: 2,
        align_height: 2,
    };

    fn hot(quality: &mut AdaptiveQualityController, now_ms: u64) -> Option<Target> {
        (0..4)
            .map(|i| {
                quality.sample(&Sample {
                    now_ms: now_ms + i * 1000,
                    recent_video: true,
                    decode_ms: 2.0,
                    thermal_level: 2,
                    bitrate_has_room: true,
                    ..Sample::default()
                })
            })
            .last()
            .flatten()
    }

    #[test]
    fn invalid_streams_are_refused() {
        for (width, height, fps) in [(0, 1080, 60), (1920, 0, 60), (1920, 1080, 0)] {
            let result =
                AdaptiveQualityController::new(Mode::Motion, width, height, fps, BOUNDS, 0);
            assert_eq!(result.err(), Some(QualityError::InvalidStream));
        }
        let mut quality =
            AdaptiveQualityController::new(Mode::Motion, 1920, 1080, 120, BOUNDS, 0).unwrap();
        let lower = hot(&mut quality, 15_000).unwrap();
        quality.applied(lower, 18_000).unwrap();
        // A refused resize leaves the ladder and the level alone.
        assert_eq!(
            quality.resize_ceiling(0, 904, 120, 20_000),
            Err(QualityError::InvalidStream)
        );
        assert_eq!(quality.current(), lower);
        assert!(quality.ceiling().matches(1920, 1080, 120));
    }

    #[test]
    fn targets_off_the_ladder_are_refused() {
        let mut quality =
            AdaptiveQualityController::new(Mode::Motion, 1920, 1080, 120, BOUNDS, 0).unwrap();
        let foreign = Target {
            width: 1000,
            height: 1000,
            fps: 120,
        };
        assert_eq!(
            quality.applied(foreign, 1000),
            Err(QualityError::ForeignTarget)
        );
        assert!(quality.current().matches(1920, 1080, 120));
    }

    #[test]
    fn requests_use_millihertz_and_modes_round_trip() {
        let target = Target {
            width: 1280,
            height: 720,
            fps: 60,
        };
        assert_eq!(target.request(), [1280, 720, 60_000]);
        assert_eq!(parse_target(&target.to_string()), Some(target));
        for mode in [Mode::Off, Mode::Motion, Mode::Detail, Mode::Battery] {
            assert_eq!(Mode::parse(Some(mode.preference())), mode);
        }
        assert_eq!(Mode::parse(None), Mode::Off);
    }
}
