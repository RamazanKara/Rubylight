//! Client-hint bitrate adaptation over 0x553C rate changes: [`Bitrate`] is Android's
//! `ClientHintBitrateController.java`.
//!
//! The decisions are Moonlight V+'s local controller (`AdaptiveBitrateService.tickLocal` and
//! `bitrateRange`, balanced mode, GPL-3.0): loss cuts the bitrate by a step that grows with
//! the loss, and five clean seconds probe it up by 5 % (2 % after a cut), with two seconds
//! between changes. The client hints the host over the control stream instead of HTTP, so a
//! change reaches the encoder on its next frame without a keyframe.
//!
//! Beyond V+: no decision without recent video; one request outstanding at a time, its
//! result the stream's bitrate; a host that caps a request lowers the ceiling to the cap; a
//! refusal stops the controller; a standing network queue holds probes (it never cuts by
//! itself, as V+ reads only loss) so a probe never adds latency.

use serde_json::Value;

use crate::stats::{known, Stats};
use crate::trace::{Args, Subject};

/// Time between two changes (V+'s balanced cooldown).
pub const COOLDOWN_MS: u64 = 2000;
/// Time after the start before the first decision (V+'s start delay).
pub const START_DELAY_MS: u64 = 3000;
/// Clean samples before a probe up (V+'s balanced threshold).
pub const STABLE_SAMPLES: u32 = 5;
/// A network queue at or above this holds probes.
pub const QUEUE_HOLD_MS: f32 = 20.0;

/// The balanced range around a starting bitrate, as V+'s `bitrateRange`: 0.3x (at least 3
/// Mbps) to 2x (at most 150 Mbps), always holding the start, within 0.5-800 Mbps.
pub fn range(initial_kbps: u32) -> (u32, u32) {
    let baseline = initial_kbps.clamp(500, 800_000);
    let low = 3000.max((f64::from(baseline) * 0.3) as u32);
    let high = 150_000.min(baseline * 2);
    (baseline.min(low).max(500), baseline.max(high).min(800_000))
}

/// Steers the stream's bitrate from this client's loss, one 0x553C request at a time.
#[derive(Clone, Debug, PartialEq)]
pub struct Bitrate {
    min_kbps: u32,
    max_kbps: u32,
    /// This client's cap, lowered to a cap the host answered with; 0 for none.
    limit_kbps: u32,
    current_kbps: u32,
    started_ms: u64,
    last_change_ms: Option<u64>,
    stable_samples: u32,
    loss_streak: u32,
    last_down: bool,
    pending_kbps: Option<u32>,
    stopped: bool,
}

impl Bitrate {
    /// `initial_kbps` is the stream's bitrate and `limit_kbps` this client's own cap (0 for
    /// none), which the range never exceeds.
    pub fn new(initial_kbps: u32, limit_kbps: u32, now_ms: u64) -> Self {
        let (min_kbps, max_kbps) = range(initial_kbps);
        let max_kbps = if limit_kbps > 0 {
            max_kbps.min(limit_kbps)
        } else {
            max_kbps
        };
        Self {
            min_kbps: min_kbps.min(max_kbps),
            max_kbps,
            limit_kbps,
            current_kbps: initial_kbps,
            started_ms: now_ms,
            last_change_ms: None,
            stable_samples: 0,
            loss_streak: 0,
            last_down: false,
            pending_kbps: None,
            stopped: false,
        }
    }

    /// Call once per second. Returns the bitrate to request, which stays pending until
    /// [`Self::applied`] or [`Self::refused`], or `None` to keep the current one.
    pub fn sample(&mut self, stats: &Stats) -> Option<u32> {
        let now_ms = stats.now_ms;
        if self.stopped || self.pending_kbps.is_some() {
            return None;
        }
        // Without video the loss is not current.
        if !stats.recent_video {
            self.stable_samples = 0;
            self.loss_streak = 0;
            return None;
        }
        if now_ms < self.started_ms + START_DELAY_MS
            || self
                .last_change_ms
                .is_some_and(|at| now_ms < at + COOLDOWN_MS)
        {
            return None;
        }
        // Unknown loss is no decision, and no clean sample.
        let Some(loss) = known(stats.loss_percent) else {
            self.stable_samples = 0;
            return None;
        };
        let current = f64::from(self.current_kbps);
        let mut target = current;
        if loss > 5.0 {
            target = current * 0.7;
            self.stable_samples = 0;
            self.loss_streak += 1;
        } else if loss > 2.0 {
            self.loss_streak += 1;
            if self.loss_streak >= 2 {
                target = current * 0.9;
                self.stable_samples = 0;
            }
        } else if loss > 0.5 {
            self.loss_streak += 1;
            self.stable_samples = 0;
            if self.loss_streak >= 4 {
                target = current * 0.95;
            }
        } else if known(stats.queue_ms).is_some_and(|queue| queue >= QUEUE_HOLD_MS) {
            self.loss_streak = 0;
            self.stable_samples = 0;
        } else {
            self.loss_streak = 0;
            self.stable_samples += 1;
            if self.stable_samples >= STABLE_SAMPLES && self.current_kbps < self.max_kbps {
                target = current * if self.last_down { 1.02 } else { 1.05 };
                self.stable_samples = 0;
            }
        }
        let target = (target as u32).clamp(self.min_kbps, self.max_kbps);
        if target == self.current_kbps {
            return None;
        }
        self.pending_kbps = Some(target);
        Some(target)
    }

    /// The host's result for the pending request: the stream now runs at `kbps`. Below the
    /// request, the host capped it, and the ceiling follows.
    pub fn applied(&mut self, kbps: u32, now_ms: u64) {
        let Some(requested) = self.pending_kbps.take() else {
            return;
        };
        if kbps < requested {
            self.limit_kbps = kbps;
            self.max_kbps = self.max_kbps.min(kbps);
            self.min_kbps = self.min_kbps.min(self.max_kbps);
        }
        if kbps != self.current_kbps {
            self.last_down = kbps < self.current_kbps;
        }
        self.current_kbps = kbps;
        self.last_change_ms = Some(now_ms);
    }

    /// The host refused the pending request: the controller stops for this stream.
    pub fn refused(&mut self) {
        self.pending_kbps = None;
        self.stopped = true;
    }

    /// The stream runs at `kbps` the user set by hand: the range follows it, as a new start
    /// under the same caps.
    pub fn overridden(&mut self, kbps: u32, now_ms: u64) {
        *self = Self {
            last_change_ms: Some(now_ms),
            stopped: self.stopped,
            ..Self::new(kbps, self.limit_kbps, self.started_ms)
        };
    }

    pub fn current_kbps(&self) -> u32 {
        self.current_kbps
    }

    pub fn min_kbps(&self) -> u32 {
        self.min_kbps
    }

    pub fn max_kbps(&self) -> u32 {
        self.max_kbps
    }

    /// A request is waiting for the host's result.
    pub fn pending(&self) -> bool {
        self.pending_kbps.is_some()
    }

    /// A refusal stopped the controller.
    pub fn stopped(&self) -> bool {
        self.stopped
    }
}

pub(crate) fn traced(args: &Value) -> Result<Box<dyn Subject>, String> {
    let a = Args::new(args, 3)?;
    Ok(Box::new(Bitrate::new(a.int(0)?, a.int(1)?, a.int(2)?)))
}

// Args reads only integers and text; samples also carry floats and a flag. A float reaches
// the controller as Java passes it, -1 for a missing measurement included.
fn float(args: &Value, i: usize) -> Result<Option<f32>, String> {
    args[i]
        .as_f64()
        .map(|v| Some(v as f32))
        .ok_or_else(|| format!("argument {i} is not a number: {}", args[i]))
}

impl Subject for Bitrate {
    fn call(&mut self, method: &str, args: &Value) -> Result<Value, String> {
        match method {
            "sample" => {
                let a = Args::new(args, 4)?;
                let stats = Stats {
                    now_ms: a.int(0)?,
                    recent_video: args[1]
                        .as_bool()
                        .ok_or_else(|| format!("argument 1 is not a boolean: {}", args[1]))?,
                    loss_percent: float(args, 2)?,
                    queue_ms: float(args, 3)?,
                    ..Stats::default()
                };
                Ok(self.sample(&stats).unwrap_or(0).into())
            }
            "applied" => {
                let a = Args::new(args, 2)?;
                self.applied(a.int(0)?, a.int(1)?);
                Ok(Value::Null)
            }
            "refused" => {
                Args::new(args, 0)?;
                self.refused();
                Ok(Value::Null)
            }
            "overridden" => {
                let a = Args::new(args, 2)?;
                self.overridden(a.int(0)?, a.int(1)?);
                Ok(Value::Null)
            }
            "current_kbps" | "min_kbps" | "max_kbps" | "pending" | "stopped" => {
                Args::new(args, 0)?;
                Ok(match method {
                    "current_kbps" => self.current_kbps.into(),
                    "min_kbps" => self.min_kbps.into(),
                    "max_kbps" => self.max_kbps.into(),
                    "pending" => self.pending().into(),
                    _ => self.stopped.into(),
                })
            }
            _ => Err(format!("unknown method {method}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tick(second: u64, loss: f32) -> Stats {
        Stats {
            now_ms: second * 1000,
            recent_video: true,
            loss_percent: Some(loss),
            ..Stats::default()
        }
    }

    #[test]
    fn the_range_is_v_plus_balanced() {
        assert_eq!(range(20_000), (6000, 40_000));
        assert_eq!(range(5000), (3000, 10_000));
        assert_eq!(range(100), (500, 1000));
        assert_eq!(range(100_000), (30_000, 150_000));
        assert_eq!(range(500_000), (150_000, 500_000));
        assert_eq!(range(u32::MAX), (240_000, 800_000));
    }

    #[test]
    fn one_request_at_a_time_and_nothing_after_a_refusal() {
        let mut c = Bitrate::new(20_000, 0, 0);
        assert_eq!(c.sample(&tick(3, 10.0)), Some(14_000));
        assert!(c.pending());
        assert_eq!(c.sample(&tick(4, 10.0)), None, "the first is still pending");
        c.applied(14_000, 4000);
        assert_eq!(c.sample(&tick(5, 10.0)), None, "cooldown");
        assert_eq!(c.sample(&tick(6, 10.0)), Some(9800));
        c.refused();
        assert!(c.stopped() && !c.pending());
        assert!((7..60).all(|s| c.sample(&tick(s, 10.0)).is_none()));
        c.overridden(30_000, 60_000);
        assert!(
            c.stopped(),
            "a manual change does not restart a refused controller"
        );
    }

    #[test]
    fn a_client_limit_caps_the_range() {
        let c = Bitrate::new(20_000, 25_000, 0);
        assert_eq!((c.min_kbps(), c.max_kbps()), (6000, 25_000));
        let c = Bitrate::new(20_000, 1000, 0);
        assert_eq!((c.min_kbps(), c.max_kbps()), (1000, 1000));
    }
}
