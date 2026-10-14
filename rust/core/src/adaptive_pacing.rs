//! Loss-responsive packet spacing, independent of encoder bitrate.
//!
//! Start at the route's ordinary pacing rate. Repeated *validated* video loss
//! can reduce bursts, but never below the current wire demand plus headroom.
//! This is not a measurement of path capacity. Clients without FEC reports
//! keep the ordinary rate; silence alone never authorizes faster probing.
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
    /// A rate increase is tentative until two seconds of healthy traffic.
    probe: Option<(u64, Instant)>,
    recovery_hold: Duration,
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
            recovery_hold: Duration::from_secs(8),
        }
    }

    /// Refresh on each frame: FEC, FPS, bitrate and the route can change.
    /// The physical/configured ceiling wins when the requested stream cannot
    /// fit. The caller reports that shortfall; this controller cannot fix it.
    pub fn rate(&mut self, ceiling: u64, needed: u64) -> u64 {
        self.ceiling = ceiling.max(1);
        self.floor = needed
            .saturating_add(needed / 4)
            .max(1_000_000)
            .min(self.ceiling);
        if !self.enabled {
            return ceiling;
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
    /// Cuts require two distinct failed frames, or multiple recovered frames
    /// missing at least 2% of all sent packets. Sparse, fully
    /// repaired losses therefore don't ratchet every connection to the floor.
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
        let Some((previous, before)) = self.last_sample else {
            self.last_sample = Some((now, traffic));
            return None;
        };
        let elapsed = now.saturating_duration_since(previous);
        if elapsed < Duration::from_millis(500) {
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
        let queued = self
            .minimum_rtt
            .is_some_and(|minimum| rtt > minimum + (minimum / 2).max(Duration::from_millis(8)));
        if (substantial || (self.probe.is_some() && loss)) && recent && frames > 0 {
            let next = if let Some((stable, _)) = self.probe.take() {
                // A failed probe restores the last working rate, and increases
                // the quiet period before another attempt (up to a minute).
                self.recovery_hold = (self.recovery_hold * 2).min(Duration::from_secs(60));
                stable
            } else {
                old / 2
            };
            self.limit = Some(next.clamp(self.floor, self.ceiling));
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
                let clean = *self.clean_since.get_or_insert(now);
                if self.probe.is_some_and(|(_, at)| {
                    now.saturating_duration_since(at.max(clean)) >= Duration::from_secs(2)
                }) {
                    self.probe = None;
                    if old == self.ceiling {
                        self.limit = None;
                    }
                }
                if self.probe.is_none()
                    && old < self.ceiling
                    && now.saturating_duration_since(clean) >= self.recovery_hold
                {
                    self.limit = Some(
                        old.saturating_add((old / 20).max(1_000_000))
                            .min(self.ceiling),
                    );
                    self.probe = Some((old, now));
                    self.after_frame = last_sent;
                }
            }
        }
        let next = self.current();
        (next != old).then_some((old, next))
    }

    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "automatic": self.enabled,
            "limited": self.enabled && self.current() < self.ceiling,
            "current_kbps": self.current() / 1000,
            "ceiling_kbps": self.ceiling / 1000,
            "wire_floor_kbps": self.floor / 1000,
            "recovery_hold_seconds": self.recovery_hold.as_secs(),
        })
    }
}

fn newer(frame: u32, previous: u32) -> bool {
    let distance = frame.wrapping_sub(previous);
    distance > 0 && distance < (1 << 31)
}

#[cfg(test)]
mod tests;
