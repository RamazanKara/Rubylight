//! Loss-responsive packet spacing, independent of encoder bitrate.
//!
//! Start with room for the stream's wire demand, then cautiously probe the
//! route's ordinary ceiling. Validated video loss can reduce bursts, but never
//! below the current wire demand plus headroom. This is not a measurement of
//! path capacity: control pings establish activity, not successful video
//! delivery. Silence alone never authorizes faster probing.
use std::time::{Duration, Instant};

use crate::adaptive_fec::Block;

#[derive(Clone, Copy, Default)]
pub struct Traffic {
    pub frames: u64,
    pub bytes: u64,
    pub packets: u64,
    pub send_losses: u64,
}

pub struct AdaptivePacing {
    enabled: bool,
    initialized: bool,
    startup: bool,
    ceiling: u64,
    floor: u64,
    limit: Option<u64>,
    last_sample: Option<(Instant, Traffic)>,
    last_ping: Option<Instant>,
    minimum_rtt: Option<Duration>,
    /// Feedback at/before this frame describes the previous rate.
    after_frame: Option<u32>,
    /// Recent frames may report their blocks out of order. Retain whether a
    /// failed block was already counted, independently of a recovered block.
    reported_frames: Vec<(u32, bool)>,
    lost_frames: u32,
    failed_frames: u32,
    lost_packets: u64,
    any_loss: bool,
    clean_since: Option<Instant>,
    /// A rate increase is tentative until its feedback interval has elapsed.
    probe: Option<Probe>,
    /// A changed wire budget invalidates feedback for the old rate. The
    /// control loop supplies the new sent-frame fence before sampling again.
    awaiting_fence: bool,
    recovery_hold: Duration,
}

struct Probe {
    stable: u64,
    started: Instant,
    settle: Duration,
}

impl Default for AdaptivePacing {
    fn default() -> Self {
        Self::new(false)
    }
}

impl AdaptivePacing {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            initialized: false,
            startup: enabled,
            ceiling: 0,
            floor: 0,
            limit: None,
            last_sample: None,
            last_ping: None,
            minimum_rtt: None,
            after_frame: None,
            reported_frames: Vec::new(),
            lost_frames: 0,
            failed_frames: 0,
            lost_packets: 0,
            any_loss: false,
            clean_since: None,
            probe: None,
            awaiting_fence: false,
            recovery_hold: Duration::from_secs(8),
        }
    }

    /// Whether this controller owns the stream's pacing.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Refresh on each frame: FEC, FPS, bitrate and the route can change.
    /// The physical/configured ceiling wins when the requested stream cannot
    /// fit. The caller reports that shortfall; this controller cannot fix it.
    pub fn rate(&mut self, ceiling: u64, needed: u64, encoder_kbps: u32) -> u64 {
        let previous_rate = self.current();
        let previous_ceiling = self.ceiling;
        let previous_floor = self.floor;
        self.ceiling = ceiling.max(1);
        self.floor = needed
            .saturating_add(needed / 4)
            .max(1_000_000)
            .min(self.ceiling);
        if !self.enabled {
            return ceiling;
        }
        if !self.initialized {
            self.initialized = true;
            self.limit = Some(
                u64::from(encoder_kbps)
                    .saturating_mul(2000)
                    .clamp(self.floor, self.ceiling),
            );
        } else if self.ceiling != previous_ceiling || self.floor != previous_floor {
            if self.ceiling != previous_ceiling {
                // Route activity and RTT from before a changed ceiling do
                // not validate probing the replacement route.
                self.last_ping = None;
                self.minimum_rtt = None;
            }
            if self.ceiling > previous_ceiling && self.limit.is_none() {
                // A newly faster host route says nothing about downstream
                // capacity. Keep the previously confirmed rate and probe the
                // extra headroom using fresh evidence, without reseeding.
                self.limit = Some(previous_rate);
                self.startup = !self.any_loss;
            }
            // A tentative faster rate has not earned promotion. Retain its
            // preceding rate, adjusted for the new demand, and gather fresh
            // evidence instead of confirming across a changed wire budget.
            if let Some(probe) = self.probe.take() {
                self.limit = Some(probe.stable);
                if self.any_loss {
                    // The same report may raise FEC before the control loop
                    // samples it. A changed wire floor must not erase this
                    // failed probe or restart fast startup probing.
                    self.startup = false;
                    self.recovery_hold = (self.recovery_hold * 2).min(Duration::from_secs(60));
                }
            }
            self.clean_since = None;
            self.last_sample = None;
            self.awaiting_fence = true;
        }
        if let Some(limit) = &mut self.limit {
            let next = (*limit).clamp(self.floor, self.ceiling);
            if next != *limit {
                self.probe = None;
                self.clean_since = None;
            }
            *limit = next;
        }
        self.current()
    }

    /// Preview the rate for a possible frame without changing the sender's
    /// demand, tentative probe or feedback fence. Encoder recovery caps use
    /// more parity than ordinary frames; querying that budget must not reset
    /// the controller's evidence on every input picture.
    pub fn preview_rate(&self, ceiling: u64, needed: u64, encoder_kbps: u32) -> u64 {
        if !self.enabled {
            return ceiling;
        }
        let ceiling = ceiling.max(1);
        let floor = needed
            .saturating_add(needed / 4)
            .max(1_000_000)
            .min(ceiling);
        let limit = if !self.initialized {
            Some(u64::from(encoder_kbps).saturating_mul(2000))
        } else if ceiling != self.ceiling || floor != self.floor {
            // A changed budget retracts an unconfirmed probe. A confirmed
            // fast route retains its previous ceiling when the route grows.
            self.probe.as_ref().map(|probe| probe.stable).or_else(|| {
                self.limit
                    .or_else(|| (ceiling > self.ceiling).then(|| self.current()))
            })
        } else {
            self.limit
        };
        limit.map_or(ceiling, |limit| limit.clamp(floor, ceiling))
    }

    fn current(&self) -> u64 {
        self.limit.unwrap_or(self.ceiling)
    }

    pub fn on_ping(&mut self, now: Instant, payload: &[u8]) {
        if self.enabled && payload.len() == 8 && payload[..4] == 4u32.to_le_bytes() {
            self.last_ping = Some(now);
        }
    }

    /// Call only after protocol validation and block deduplication. Count
    /// distinct lost frames, not their (up to four) block reports. Delayed
    /// reports must not penalize the replacement rate for an earlier burst.
    pub fn on_block(&mut self, frame: u32, last_sent: u32, period: Duration, block: Block) {
        let recent_frames =
            (Duration::from_secs(2).as_nanos() / period.as_nanos().max(1)).clamp(1, 1024) as u32;
        if !self.enabled
            || self.awaiting_fence
            || last_sent.wrapping_sub(frame) >= recent_frames
            || self.after_frame.is_some_and(|after| !newer(frame, after))
            || matches!(block, Block::Clean | Block::Recovered { lost: 0, .. })
        {
            return;
        }
        self.any_loss = true;
        self.clean_since = None;
        if let Block::Recovered { lost, .. } = block {
            self.lost_packets = self.lost_packets.saturating_add(u64::from(lost));
        }
        self.reported_frames
            .retain(|(seen, _)| last_sent.wrapping_sub(*seen) < recent_frames);
        let failed = matches!(block, Block::Unrecoverable);
        if let Some((_, was_failed)) = self
            .reported_frames
            .iter_mut()
            .find(|(seen, _)| *seen == frame)
        {
            if failed && !*was_failed {
                *was_failed = true;
                self.failed_frames = self.failed_frames.saturating_add(1);
            }
        } else {
            self.reported_frames.push((frame, failed));
            self.lost_frames = self.lost_frames.saturating_add(1);
            self.failed_frames = self.failed_frames.saturating_add(u32::from(failed));
        }
    }

    /// The control loop supplies counters and ENet RTT, never the send loop.
    /// Outside tentative probes, cuts require two distinct failed frames, or
    /// multiple recovered frames missing at least 2% of all sent packets.
    /// Sparse, fully repaired losses therefore don't ratchet every connection
    /// to the floor; any loss can reject an unconfirmed increase.
    pub fn tick(
        &mut self,
        now: Instant,
        traffic: Traffic,
        last_sent: Option<u32>,
        period: Duration,
        encoder_kbps: u32,
        rtt: Duration,
    ) -> Option<(u64, u64)> {
        if !self.enabled || self.ceiling == 0 {
            return None;
        }
        if self.awaiting_fence {
            self.awaiting_fence = false;
            self.after_frame = last_sent;
            self.lost_frames = 0;
            self.failed_frames = 0;
            self.lost_packets = 0;
            self.any_loss = false;
        }
        let Some((previous, before)) = self.last_sample else {
            self.last_sample = Some((now, traffic));
            return None;
        };
        let elapsed = now.saturating_duration_since(previous);
        // A validated loss during a tentative increase need not wait for a
        // full sampling window before restoring the preceding rate.
        let probe_loss = self.probe.is_some() && self.any_loss;
        if elapsed < Duration::from_millis(500) && !probe_loss {
            return None;
        }
        self.last_sample = Some((now, traffic));
        let frames = traffic.frames.saturating_sub(before.frames);
        let packets = traffic.packets.saturating_sub(before.packets);
        let loss = std::mem::take(&mut self.any_loss) || traffic.send_losses > before.send_losses;
        let substantial = self.failed_frames >= 2
            || (self.lost_frames >= 2 && packets > 0 && self.lost_packets >= packets.div_ceil(50));
        self.lost_frames = 0;
        self.failed_frames = 0;
        self.lost_packets = 0;
        let old = self.current();
        let recent = elapsed <= Duration::from_secs(2);
        let ping = self
            .last_ping
            .is_some_and(|at| now.saturating_duration_since(at) < Duration::from_secs(2));
        if ping && !rtt.is_zero() {
            self.minimum_rtt = Some(self.minimum_rtt.map_or(rtt, |minimum| minimum.min(rtt)));
        }
        let queued = self.minimum_rtt.is_some_and(|minimum| {
            rtt > minimum.saturating_add((minimum / 2).max(Duration::from_millis(8)))
        });
        if (substantial || (self.probe.is_some() && loss)) && recent && (frames > 0 || probe_loss) {
            let next = if let Some(probe) = self.probe.take() {
                // A failed probe restores the last working rate, and increases
                // the quiet period before another attempt (up to a minute).
                self.recovery_hold = (self.recovery_hold * 2).min(Duration::from_secs(60));
                probe.stable
            } else {
                old / 2
            };
            self.limit = Some(next.clamp(self.floor, self.ceiling));
            self.startup = false;
            self.after_frame = last_sent;
            self.clean_since = None;
        } else {
            let seconds = elapsed.as_secs_f64();
            let busy = recent
                && frames as f64 >= seconds / period.as_secs_f64().max(0.001) * 0.6
                && traffic.bytes.saturating_sub(before.bytes) as f64 * 8.
                    >= f64::from(encoder_kbps) * 1000. * seconds * 0.6;
            if loss || !ping || !busy || rtt.is_zero() || queued {
                self.clean_since = None;
            } else if self.limit.is_some() {
                // The counters describe the interval just completed. Startup
                // can use that first active interval; recovery retains its
                // longer quiet hold after an established loss.
                let clean =
                    *self
                        .clean_since
                        .get_or_insert(if self.startup { previous } else { now });
                if self.probe.as_ref().is_some_and(|probe| {
                    let settle = if self.startup {
                        probe.settle.max(startup_feedback_wait(rtt))
                    } else {
                        probe.settle
                    };
                    now.saturating_duration_since(probe.started.max(clean)) >= settle
                }) {
                    self.probe = None;
                    if old == self.ceiling {
                        self.limit = None;
                        self.startup = false;
                    }
                }
                let hold = if self.startup {
                    startup_feedback_wait(rtt)
                } else {
                    self.recovery_hold
                };
                if self.probe.is_none() && now.saturating_duration_since(clean) >= hold {
                    if old < self.ceiling {
                        let step = if self.startup {
                            // Smaller probes leave ordinary FEC more room to
                            // repair losses while feedback returns. This is
                            // headroom, not a guarantee of loss-free probing.
                            (old / 8).max(1)
                        } else {
                            (old / 20).max(1_000_000)
                        };
                        self.limit = Some(old.saturating_add(step).min(self.ceiling));
                        self.probe = Some(Probe {
                            stable: old,
                            started: now,
                            settle: if self.startup {
                                hold
                            } else {
                                Duration::from_secs(2)
                            },
                        });
                        self.after_frame = last_sent;
                    } else if self.startup {
                        // The initial wire-demand floor can already equal a
                        // low physical ceiling. Confirm it without probing.
                        self.limit = None;
                        self.startup = false;
                    }
                }
            }
        }
        let next = self.current();
        (next != old).then_some((old, next))
    }

    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "automatic": self.enabled,
            "startup": self.startup,
            "probing": self.probe.is_some(),
            "limited": self.enabled && self.current() < self.ceiling,
            "current_kbps": self.current() / 1000,
            "ceiling_kbps": self.ceiling / 1000,
            "wire_floor_kbps": self.floor / 1000,
            "recovery_hold_seconds": self.recovery_hold.as_secs(),
        })
    }
}

fn startup_feedback_wait(rtt: Duration) -> Duration {
    Duration::from_millis(500).max(
        rtt.saturating_mul(2)
            .saturating_add(Duration::from_millis(100)),
    )
}

fn newer(frame: u32, previous: u32) -> bool {
    let distance = frame.wrapping_sub(previous);
    distance > 0 && distance < (1 << 31)
}

#[cfg(test)]
mod tests;
