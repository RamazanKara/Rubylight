//! Adaptive bitrate: cuts the stream's rate for congestion, heat and a slow decoder, and steps it
//! back once the stream is healthy again.
//!
//! [`AdaptiveBitrateController`] is Android's (`AdaptiveBitrateController.java`). It computes in
//! `f32` like the Java original, so both decide the same on the shared traces.

use serde_json::Value;

use crate::trace::{Args, Subject};

/// Lowest rate the controller steps down to unless the person sets a higher minimum.
pub const DEFAULT_FLOOR_KBPS: u32 = 2000;
/// The highest rate the client asks for (`StreamConfiguration.MAX_BITRATE_KBPS`).
pub const MAX_BITRATE_KBPS: u32 = 2_000_000;

// A rate with no poor sample for this long counts as stable and is what recovery steps back to.
const STABLE_MS: u64 = 10_000;
// Healthy time before stepping back toward the stable rate, and the pause after a step back failed.
const STEP_BACK_HEALTHY_MS: u64 = 5000;
const STEP_BACK_RETRY_HEALTHY_MS: u64 = 15_000;
const STEP_BACK_SPACING_MS: u64 = 3000;
const STEP_BACK_MIN_KBPS: u32 = 250;
// Jitter or queueing that a cut did not lower to this share of what triggered it was not helped by it.
const CUT_HELPED_RATIO: f32 = 0.85;
// This many cuts in a row that did not help stop further cuts for jitter and queueing.
const INEFFECTIVE_CUTS_TO_HOLD: u32 = 2;
const INSENSITIVE_CLEAR_MS: u64 = 5000;
// The last few poor samples of the current run are judged together.
const RUN_SAMPLES: usize = 3;
// A queue rising by this much per sample over the run is bitrate-caused growth, never held.
const QUEUE_RISE_MS_PER_SAMPLE: f32 = 2.0;
const QUEUE_RISE_MIN_MS: f32 = 3.0;

/// Video codec of the stream; a newer codec needs fewer bits per pixel for a watchable picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    H264,
    Hevc,
    Av1,
}

impl Codec {
    // Bits per thousand pixels. HEVC 1080p60 lands at 8.7 Mbps and 720p30 stays under the 2 Mbps minimum.
    fn bits_per_kilo_pixel(self) -> u64 {
        match self {
            Codec::H264 => 100,
            Codec::Hevc => 70,
            Codec::Av1 => 65,
        }
    }
}

/// The lowest rate that still leaves a usable picture at this size, rate and codec, never below
/// [`DEFAULT_FLOOR_KBPS`]. An unknown size gives the flat default.
pub fn built_in_floor_kbps(width: u32, height: u32, fps: u32, codec: Option<Codec>) -> u32 {
    let Some(codec) = codec else {
        return DEFAULT_FLOOR_KBPS;
    };
    if width == 0 || height == 0 || fps == 0 {
        return DEFAULT_FLOOR_KBPS;
    }
    let kbps = u64::from(width)
        .saturating_mul(height.into())
        .saturating_mul(fps.into())
        .saturating_mul(codec.bits_per_kilo_pixel())
        / 1_000_000;
    (kbps.min(MAX_BITRATE_KBPS.into()) as u32).max(DEFAULT_FLOOR_KBPS)
}

/// One second of stream statistics. A measurement is `None` when unknown.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    /// Frames arrived recently.
    pub recent_video: bool,
    /// The native connection-quality warning.
    pub poor_connection: bool,
    /// Frames lost after FEC, in percent.
    pub loss_percent: Option<f32>,
    pub decode_ms: Option<f32>,
    pub jitter_ms: Option<f32>,
    /// Network queueing delay above its recent minimum. A queue building at the access point
    /// shows there before packets are dropped, so a rising queue lowers the bitrate (and with it
    /// the host's pacing rate) before losses pile up.
    pub queue_ms: Option<f32>,
}

/// Picks the stream's rate from once-a-second samples; the caller requests it from the host and
/// reports what the host applied.
#[derive(Clone, Debug, PartialEq)]
pub struct AdaptiveBitrateController {
    ceiling_kbps: u32,
    start_kbps: u32,
    current_kbps: u32,
    floor_kbps: u32,
    // Lowest rate the host applied below the floor, so a recomputed floor never exceeds a host cap.
    host_floor_cap_kbps: u32,
    frame_ms: f32,
    fps: u32,
    width: u32,
    height: u32,
    codec: Option<Codec>,
    last_change_ms: u64,
    healthy_since_ms: Option<u64>,
    poor_samples: u32,
    thermal_level: u32,
    previous_queue_ms: Option<f32>,
    decode_before_reduction_ms: Option<f32>,
    decode_before_reduction_kbps: u32,
    decode_bitrate_insensitive: bool,
    // The last poor samples of the current run: jitter and queue against their trigger
    // thresholds, and the queue delay itself.
    run_severity: [f32; RUN_SAMPLES],
    run_queue_ms: [Option<f32>; RUN_SAMPLES],
    run_count: usize,
    last_cut_severity: Option<f32>,
    ineffective_cuts: u32,
    network_bitrate_insensitive: bool,
    calm_since_ms: Option<u64>,
    stable_kbps: u32,
    // The rate a step back asked for; applied() with this request is that step back. Kept across suspend().
    step_back_target_kbps: Option<u32>,
    step_back_from_kbps: Option<u32>,
    step_back_failures: u32,
}

impl AdaptiveBitrateController {
    /// A controller for a stream of unknown size, with the flat default floor.
    pub fn new(ceiling_kbps: u32, current_kbps: u32, fps: u32, now_ms: u64) -> Self {
        Self::with_stream(ceiling_kbps, current_kbps, fps, now_ms, 0, 0, None)
    }

    /// The ceiling is the rate the stream starts at. The floor follows the stream's size, rate
    /// and codec (see [`built_in_floor_kbps`]), but never goes above half the starting rate, so
    /// congestion can still bring the rate down.
    pub fn with_stream(
        ceiling_kbps: u32,
        current_kbps: u32,
        fps: u32,
        now_ms: u64,
        width: u32,
        height: u32,
        codec: Option<Codec>,
    ) -> Self {
        let ceiling_kbps = ceiling_kbps.min(MAX_BITRATE_KBPS);
        let current_kbps = current_kbps.min(ceiling_kbps);
        let mut controller = Self {
            ceiling_kbps,
            start_kbps: current_kbps,
            current_kbps,
            floor_kbps: 0,
            host_floor_cap_kbps: u32::MAX,
            frame_ms: frame_ms(fps),
            fps,
            width,
            height,
            codec,
            last_change_ms: now_ms,
            healthy_since_ms: None,
            poor_samples: 0,
            thermal_level: 0,
            previous_queue_ms: None,
            decode_before_reduction_ms: None,
            decode_before_reduction_kbps: 0,
            decode_bitrate_insensitive: false,
            run_severity: [0.0; RUN_SAMPLES],
            run_queue_ms: [None; RUN_SAMPLES],
            run_count: 0,
            last_cut_severity: None,
            ineffective_cuts: 0,
            network_bitrate_insensitive: false,
            calm_since_ms: None,
            stable_kbps: 0,
            step_back_target_kbps: None,
            step_back_from_kbps: None,
            step_back_failures: 0,
        };
        controller.update_floor();
        controller
    }

    fn update_floor(&mut self) {
        let nominal = built_in_floor_kbps(self.width, self.height, self.fps, self.codec)
            .min(DEFAULT_FLOOR_KBPS.max(self.start_kbps / 2));
        self.floor_kbps = nominal
            .min(self.start_kbps.min(self.ceiling_kbps))
            .min(self.host_floor_cap_kbps);
    }

    /// Called once per second, only while no bitrate request is in flight. Returns the rate to
    /// request, or `None` to keep the current one.
    pub fn sample(&mut self, now_ms: u64, s: Sample) -> Option<u32> {
        // As in Java, only a value of at least 0 is measured; a negative or NaN one is unknown.
        let s = Sample {
            loss_percent: s.loss_percent.filter(|v| *v >= 0.0),
            decode_ms: s.decode_ms.filter(|v| *v >= 0.0),
            jitter_ms: s.jitter_ms.filter(|v| *v >= 0.0),
            queue_ms: s.queue_ms.filter(|v| *v >= 0.0),
            ..s
        };
        let recent = s.recent_video;
        if !recent && !s.poor_connection {
            self.healthy_since_ms = None;
            self.calm_since_ms = None;
            self.poor_samples = 0;
            self.run_count = 0;
            self.reset_hold();
            self.previous_queue_ms = None;
            return None;
        }
        let frame_ms = self.frame_ms;
        let jitter_limit = 5f32.max(frame_ms * 0.75);
        let queue_limit = 8f32.max(frame_ms * 0.75);
        let previous_queue_ms = self.previous_queue_ms;
        let queue_building = recent
            && s.queue_ms.is_some_and(|queue| {
                queue >= 20f32.max(frame_ms * 2.0)
                    || previous_queue_ms
                        .is_some_and(|previous| queue >= queue_limit && queue - previous >= 3.0)
            });
        self.previous_queue_ms = if recent { s.queue_ms } else { None };
        // Loss the FEC could not repair and the native warning are congestion at any bitrate.
        // Jitter and queueing can come from the access point instead, whatever rate is sent.
        let hard_poor = s.poor_connection || (recent && s.loss_percent.is_some_and(|l| l >= 3.0));
        let poor = hard_poor
            || queue_building
            || (recent && s.jitter_ms.is_some_and(|j| j >= jitter_limit));
        if hard_poor {
            self.network_bitrate_insensitive = false;
        }
        if recent && s.decode_ms.is_some_and(|d| d < frame_ms * 0.75) {
            self.decode_bitrate_insensitive = false;
            self.decode_before_reduction_ms = None;
        }
        let mut slow_decoder = recent
            && s.decode_ms.is_some_and(|d| d >= frame_ms)
            && !self.decode_bitrate_insensitive;
        if slow_decoder
            && !poor
            && self.thermal_level < 2
            && self.current_kbps < self.decode_before_reduction_kbps
            && now_ms.saturating_sub(self.last_change_ms) >= 15_000
            && s.decode_ms.is_some_and(|decode| {
                self.decode_before_reduction_ms
                    .is_some_and(|before| decode >= before * 0.95)
            })
        {
            // Fixed decoder buffering does not improve with bitrate. Stop cutting and allow recovery.
            self.decode_bitrate_insensitive = true;
            slow_decoder = false;
        }
        if poor || (recent && self.thermal_level >= 2) || slow_decoder {
            self.healthy_since_ms = None;
            self.calm_since_ms = None;
            self.poor_samples = self.poor_samples.saturating_add(1);
            if poor {
                let jitter = s
                    .jitter_ms
                    .filter(|j| *j > 0.0)
                    .map_or(0.0, |j| j / jitter_limit);
                let queue = s
                    .queue_ms
                    .filter(|q| *q > 0.0)
                    .map_or(0.0, |q| q / queue_limit);
                self.add_to_run(jitter.max(queue), if recent { s.queue_ms } else { None });
            }
            // Heat and decoder pressure step down less often and not as far as network congestion.
            let (floor, wait_ms) = if poor {
                (self.cut_floor_kbps(), 3000)
            } else {
                (self.floor_kbps.max(self.ceiling_kbps / 2), 15_000)
            };
            if self.poor_samples >= 2 && now_ms.saturating_sub(self.last_change_ms) >= wait_ms {
                let target = if poor {
                    self.network_target(now_ms, hard_poor, floor)
                } else {
                    floor.max(self.current_kbps * 80 / 100)
                };
                if target < self.current_kbps && self.thermal_level < 2 && slow_decoder {
                    self.decode_before_reduction_ms = s.decode_ms;
                    self.decode_before_reduction_kbps = self.current_kbps;
                }
                return (target < self.current_kbps).then_some(target);
            }
            return None;
        }
        self.poor_samples = 0;
        self.run_count = 0;
        match self.calm_since_ms {
            None => self.calm_since_ms = Some(now_ms),
            Some(calm) if now_ms.saturating_sub(calm) >= STABLE_MS => {
                // No poor sample for a while at this rate: remember it, and a step back has worked.
                self.stable_kbps = self.stable_kbps.max(self.current_kbps);
                self.step_back_from_kbps = None;
                self.step_back_failures = 0;
            }
            Some(_) => {}
        }
        let healthy = self.thermal_level == 0
            && s.loss_percent.is_some_and(|l| l < 0.5)
            && s.decode_ms
                .is_some_and(|d| self.decode_bitrate_insensitive || d < frame_ms * 0.75)
            && s.jitter_ms.is_some_and(|j| j < 5f32.max(frame_ms * 0.25))
            && !s.queue_ms.is_some_and(|q| q >= 4f32.max(frame_ms / 2.0));
        let Some(healthy_since) = self.healthy_since_ms.filter(|_| healthy) else {
            self.healthy_since_ms = healthy.then_some(now_ms);
            return None;
        };
        let healthy_ms = now_ms.saturating_sub(healthy_since);
        let since_change_ms = now_ms.saturating_sub(self.last_change_ms);
        if healthy_ms >= INSENSITIVE_CLEAR_MS {
            // The jitter or queueing is gone; cuts may be judged afresh.
            self.network_bitrate_insensitive = false;
            self.last_cut_severity = None;
            self.ineffective_cuts = 0;
        }
        let stable = self.stable_kbps.min(self.ceiling_kbps);
        if stable > self.current_kbps {
            // Trouble cut the rate from a stable one; come back in big steps instead of 5% per 15 s.
            let needed_ms = if self.step_back_failures > 0 {
                STEP_BACK_RETRY_HEALTHY_MS
            } else {
                STEP_BACK_HEALTHY_MS
            };
            if healthy_ms >= needed_ms && since_change_ms >= STEP_BACK_SPACING_MS {
                let gap = stable - self.current_kbps;
                let step = gap.div_ceil(2).max(gap.min(STEP_BACK_MIN_KBPS));
                let target = self.current_kbps + step;
                self.step_back_target_kbps = Some(target);
                self.step_back_from_kbps = Some(self.current_kbps);
                return Some(target);
            }
        } else if healthy_ms >= 15_000 && since_change_ms >= 15_000 {
            let target = self
                .ceiling_kbps
                .min(self.current_kbps + 250.max(self.current_kbps / 20));
            return (target > self.current_kbps).then_some(target);
        }
        None
    }

    // The rate to cut to for a poor network sample, or the current rate to hold it.
    fn network_target(&mut self, now_ms: u64, hard_poor: bool, floor: u32) -> u32 {
        let current = self.current_kbps;
        if let Some(from) = self
            .step_back_from_kbps
            .filter(|&from| from > 0 && from < current)
        {
            if now_ms.saturating_sub(self.last_change_ms) < STABLE_MS {
                // The step back brought the trouble again: return to the rate that was fine and
                // remember that the stable rate lies between the two.
                let between = (from + current) / 2;
                self.stable_kbps = if between - from < STEP_BACK_MIN_KBPS {
                    from
                } else {
                    between
                };
                self.step_back_failures += 1;
                self.step_back_from_kbps = None;
                return floor.max(from);
            }
        }
        let cut = floor.max(current * 80 / 100);
        if cut >= current {
            return current;
        }
        if hard_poor {
            self.last_cut_severity = None;
            self.ineffective_cuts = 0;
            return cut;
        }
        let severity = self.run_severity();
        if self.queue_rising() {
            // A queue that keeps growing is the stream outrunning the link; only a lower rate
            // stops it, even when the last cut slowed the growth without ending it.
            self.network_bitrate_insensitive = false;
            self.ineffective_cuts = 0;
            self.last_cut_severity = Some(severity);
            return cut;
        }
        if self.network_bitrate_insensitive {
            return current;
        }
        // Only jitter or a queue that is not growing: judge whether the previous cut lowered it.
        // Cutting further for a signal that does not follow the bitrate only ratchets the picture
        // down to the floor.
        if let Some(last) = self.last_cut_severity {
            self.ineffective_cuts = if severity > last * CUT_HELPED_RATIO {
                self.ineffective_cuts + 1
            } else {
                0
            };
        }
        if self.ineffective_cuts >= INEFFECTIVE_CUTS_TO_HOLD {
            self.network_bitrate_insensitive = true;
            return current;
        }
        self.last_cut_severity = Some(severity);
        cut
    }

    fn add_to_run(&mut self, severity: f32, queue_ms: Option<f32>) {
        if self.run_count == RUN_SAMPLES {
            self.run_severity.copy_within(1.., 0);
            self.run_queue_ms.copy_within(1.., 0);
            self.run_count -= 1;
        }
        self.run_severity[self.run_count] = severity;
        self.run_queue_ms[self.run_count] = queue_ms;
        self.run_count += 1;
    }

    // Mean over the last poor samples, those just before the decision rather than the ones right
    // after the previous change.
    fn run_severity(&self) -> f32 {
        let sum = self.run_severity[..self.run_count]
            .iter()
            .fold(0.0f32, |sum, severity| sum + severity);
        sum / self.run_count.max(1) as f32
    }

    fn queue_rising(&self) -> bool {
        if self.run_count < 2 {
            return false;
        }
        let (Some(first), Some(last)) =
            (self.run_queue_ms[0], self.run_queue_ms[self.run_count - 1])
        else {
            return false;
        };
        let rise = last - first;
        rise >= QUEUE_RISE_MIN_MS && rise / (self.run_count - 1) as f32 >= QUEUE_RISE_MS_PER_SAMPLE
    }

    // What cuts judged earlier no longer say anything: the frame rate changed or video stopped.
    fn reset_hold(&mut self) {
        self.network_bitrate_insensitive = false;
        self.ineffective_cuts = 0;
        self.last_cut_severity = None;
    }

    // A floor raised above the current rate (a larger stream size after congestion cut the rate)
    // only bounds cuts that start above it; loss must still be able to cut, down to the default floor.
    fn cut_floor_kbps(&self) -> u32 {
        if self.floor_kbps <= self.current_kbps {
            self.floor_kbps
        } else {
            DEFAULT_FLOOR_KBPS.min(self.current_kbps)
        }
    }

    /// The thermal monitor's level: warm (1) holds the rate, hot (2 and up) steps it down.
    pub fn set_thermal_level(&mut self, level: u32) {
        self.thermal_level = level;
    }

    pub fn set_frame_rate(&mut self, fps: u32) {
        self.fps = fps;
        self.frame_ms = frame_ms(fps);
        self.update_floor();
        self.suspend();
        self.reset_hold();
        self.decode_before_reduction_ms = None;
        self.decode_bitrate_insensitive = false;
    }

    /// The stream changed size (automatic quality); the built-in floor follows it.
    pub fn set_stream_size(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.update_floor();
    }

    /// Prefer bitrate reductions before lowering stream resolution for network congestion.
    pub fn has_room_before_quality_reduction(&self) -> bool {
        self.ceiling_kbps == 0
            || self.current_kbps > self.floor_kbps.max(self.ceiling_kbps * 65 / 100)
    }

    /// The host applied a requested rate.
    pub fn applied(&mut self, requested_kbps: u32, applied_kbps: u32, now_ms: u64) {
        // The host may enforce a lower cap. Do not repeatedly probe beyond it.
        if applied_kbps < requested_kbps {
            self.ceiling_kbps = self.ceiling_kbps.min(applied_kbps);
            self.host_floor_cap_kbps = self.host_floor_cap_kbps.min(applied_kbps);
        }
        let raised = applied_kbps > self.current_kbps;
        let stepped_back = self.step_back_target_kbps.take() == Some(requested_kbps) && raised;
        self.current_kbps = applied_kbps.min(MAX_BITRATE_KBPS);
        self.update_floor();
        self.last_change_ms = now_ms;
        // Steps back follow each other without waiting for a fresh healthy window.
        if !stepped_back || self.current_kbps >= self.stable_kbps.min(self.ceiling_kbps) {
            self.healthy_since_ms = None;
        }
        if !stepped_back {
            self.step_back_from_kbps = None;
        }
        if raised {
            self.last_cut_severity = None;
            self.ineffective_cuts = 0;
        }
        self.calm_since_ms = None;
        self.poor_samples = 0;
        self.run_count = 0;
        self.previous_queue_ms = None;
    }

    /// No sample counts toward a decision while the stream is paused or a request is in flight.
    pub fn suspend(&mut self) {
        self.healthy_since_ms = None;
        self.calm_since_ms = None;
        self.poor_samples = 0;
        self.run_count = 0;
        self.previous_queue_ms = None;
    }
}

fn frame_ms(fps: u32) -> f32 {
    1000.0 / fps as f32
}

// Trace arguments. A measurement is a number, or a string such as "NaN" that JSON cannot hold.
// It reaches the controller as Java passes it, so the controller's own filtering is replayed.
fn measurement(args: &Value, i: usize) -> Result<Option<f32>, String> {
    let value = match &args[i] {
        Value::Number(number) => number.as_f64().map(|v| v as f32),
        Value::String(text) => text.parse::<f32>().ok(),
        _ => None,
    }
    .ok_or_else(|| format!("argument {i} is not a float: {}", args[i]))?;
    Ok(Some(value))
}

fn flag(args: &Value, i: usize) -> Result<bool, String> {
    args[i]
        .as_bool()
        .ok_or_else(|| format!("argument {i} is not a boolean: {}", args[i]))
}

fn codec(name: Option<&str>) -> Result<Option<Codec>, String> {
    match name {
        None => Ok(None),
        Some("H264") => Ok(Some(Codec::H264)),
        Some("HEVC") => Ok(Some(Codec::Hevc)),
        Some("AV1") => Ok(Some(Codec::Av1)),
        Some(other) => Err(format!("unknown codec {other}")),
    }
}

fn count(args: &Value, counts: &[usize]) -> Result<usize, String> {
    let count = args
        .as_array()
        .ok_or_else(|| format!("arguments are not a list: {args}"))?
        .len();
    if !counts.contains(&count) {
        return Err(format!("expected {counts:?} arguments, got {count}"));
    }
    Ok(count)
}

struct Traced(Option<AdaptiveBitrateController>);

pub(crate) fn traced_controller(args: &Value) -> Result<Box<dyn Subject>, String> {
    if args.is_null() {
        return Ok(Box::new(Traced(None)));
    }
    let a = Args::new(args, count(args, &[4, 7])?)?;
    let (ceiling, current, fps, now) = (a.int(0)?, a.int(1)?, a.int(2)?, a.int(3)?);
    let controller = if args.as_array().is_some_and(|list| list.len() == 7) {
        let codec = codec(a.text(6)?)?;
        AdaptiveBitrateController::with_stream(
            ceiling,
            current,
            fps,
            now,
            a.int(4)?,
            a.int(5)?,
            codec,
        )
    } else {
        AdaptiveBitrateController::new(ceiling, current, fps, now)
    };
    Ok(Box::new(Traced(Some(controller))))
}

impl Subject for Traced {
    fn call(&mut self, method: &str, args: &Value) -> Result<Value, String> {
        if method == "built_in_floor_kbps" {
            let a = Args::new(args, 4)?;
            let codec = codec(a.text(3)?)?;
            return Ok(built_in_floor_kbps(a.int(0)?, a.int(1)?, a.int(2)?, codec).into());
        }
        let controller = self.0.as_mut().ok_or("no instance (new is null)")?;
        match method {
            "sample" => {
                // Java's shorter overloads pass no jitter (0) and an unknown queue (-1).
                let count = count(args, &[5, 6, 7])?;
                let a = Args::new(args, count)?;
                let sample = Sample {
                    recent_video: flag(args, 1)?,
                    poor_connection: flag(args, 2)?,
                    loss_percent: measurement(args, 3)?,
                    decode_ms: measurement(args, 4)?,
                    jitter_ms: if count > 5 {
                        measurement(args, 5)?
                    } else {
                        Some(0.0)
                    },
                    queue_ms: if count > 6 {
                        measurement(args, 6)?
                    } else {
                        None
                    },
                };
                Ok(controller.sample(a.int(0)?, sample).unwrap_or(0).into())
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
            "set_thermal_level" => {
                controller.set_thermal_level(Args::new(args, 1)?.int(0)?);
                Ok(Value::Null)
            }
            "set_frame_rate" => {
                controller.set_frame_rate(Args::new(args, 1)?.int(0)?);
                Ok(Value::Null)
            }
            "set_stream_size" => {
                let a = Args::new(args, 2)?;
                controller.set_stream_size(a.int(0)?, a.int(1)?);
                Ok(Value::Null)
            }
            "has_room_before_quality_reduction" => {
                Args::new(args, 0).map(|_| controller.has_room_before_quality_reduction().into())
            }
            _ => Err(format!("unknown method {method}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn measurements_reach_the_controller_as_java_passes_them() {
        let args = json!([-1, "NaN", 2.5, 0, true]);
        assert_eq!(measurement(&args, 0), Ok(Some(-1.0)));
        assert!(measurement(&args, 1).unwrap().unwrap().is_nan());
        assert_eq!(measurement(&args, 2), Ok(Some(2.5)));
        assert_eq!(measurement(&args, 3), Ok(Some(0.0)));
        assert!(measurement(&args, 4).is_err());
    }

    // The highest rate requested over a minute of samples that are healthy except for `unknown`.
    fn raised_with(unknown: impl Fn(&mut Sample)) -> Option<u32> {
        let mut controller = AdaptiveBitrateController::new(20_000, 10_000, 60, 0);
        (1..=60)
            .filter_map(|second| {
                let mut sample = Sample {
                    recent_video: true,
                    poor_connection: false,
                    loss_percent: Some(0.0),
                    decode_ms: Some(2.0),
                    jitter_ms: Some(1.0),
                    queue_ms: Some(0.0),
                };
                unknown(&mut sample);
                controller.sample(second * 1000, sample)
            })
            .max()
    }

    #[test]
    fn negative_or_nan_measurements_are_unknown_and_hold_the_rate() {
        assert!(raised_with(|_| {}).is_some_and(|kbps| kbps > 10_000));
        for value in [-1.0, f32::NAN] {
            assert_eq!(raised_with(|s| s.loss_percent = Some(value)), None);
            assert_eq!(raised_with(|s| s.decode_ms = Some(value)), None);
            assert_eq!(raised_with(|s| s.jitter_ms = Some(value)), None);
        }
    }

    #[test]
    fn malformed_calls_fail() {
        let mut subject = traced_controller(&json!([20000, 20000, 60, 0])).unwrap();
        for (method, args) in [
            ("sample", json!([1000, true, false, 0])),
            ("sample", json!([1000, 1, false, 0, 2])),
            ("applied", json!([1000, -1, 0])),
            ("built_in_floor_kbps", json!([1920, 1080, 60, "VP9"])),
        ] {
            assert!(subject.call(method, &args).is_err(), "{method}{args}");
        }
        assert!(traced_controller(&json!([20000, 20000, 60])).is_err());
        let mut statics = traced_controller(&Value::Null).unwrap();
        assert_eq!(
            statics.call("built_in_floor_kbps", &json!([1920, 1080, 60, "HEVC"])),
            Ok(json!(8709))
        );
        assert!(statics.call("suspend", &json!([])).is_err());
    }
}
