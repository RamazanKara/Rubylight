//! RTT-floor tracking and standing-queue detection for unknown-link wireless streams.

use super::{AdaptivePacing, Instant};
use std::time::Duration;

/// Rolling window for the RTT floor used to detect delay growth without
/// locking onto a one-off minimum from an earlier route state.
pub(super) const RTT_FLOOR_WINDOW: Duration = Duration::from_secs(30);
pub(super) const RTT_FLOOR_BUCKETS: usize = 32;
/// Excess RTT over the rolling floor required before an unknown-link wireless
/// stream treats delay as a standing queue rather than ordinary Wi-Fi jitter.
pub(super) const QUEUE_DELAY_MARGIN: Duration = Duration::from_millis(15);
/// Continuous busy, non-draining queue delay required before stepping down.
pub(super) const QUEUE_DELAY_HOLD: Duration = Duration::from_secs(2);

#[derive(Clone, Copy)]
pub(super) struct QueueWatch {
    pub(super) started: Instant,
    pub(super) min_excess: Duration,
}

impl AdaptivePacing {
    pub(super) fn observe_rtt(&mut self, now: Instant, rtt: Duration) {
        match self.rtt_floor.back_mut() {
            Some((at, low)) if now.saturating_duration_since(*at) < Duration::from_secs(1) => {
                *low = (*low).min(rtt);
            }
            _ => {
                if self.rtt_floor.len() >= RTT_FLOOR_BUCKETS {
                    self.rtt_floor.pop_front();
                }
                self.rtt_floor.push_back((now, rtt));
            }
        }
        while self
            .rtt_floor
            .front()
            .is_some_and(|(at, _)| now.saturating_duration_since(*at) > RTT_FLOOR_WINDOW)
        {
            self.rtt_floor.pop_front();
        }
    }

    pub(super) fn rtt_floor(&self) -> Option<Duration> {
        self.rtt_floor.iter().map(|(_, low)| *low).min()
    }

    pub(super) fn advance_queue_watch(
        &mut self,
        now: Instant,
        excess: Duration,
        prior_started: Option<Instant>,
    ) -> bool {
        let Some(watch) = &mut self.queue_watch else {
            let started = prior_started.unwrap_or(now);
            self.queue_watch = Some(QueueWatch {
                started,
                min_excess: excess,
            });
            return now.saturating_duration_since(started) >= QUEUE_DELAY_HOLD;
        };
        if excess < watch.min_excess {
            // ENet's smoothed RTT reached a new lower excess since the hold
            // started, so delay is actively draining toward the floor. Wait
            // instead of stepping pacing down through the decay tail.
            watch.started = now;
            watch.min_excess = excess;
            return false;
        }
        now.saturating_duration_since(watch.started) >= QUEUE_DELAY_HOLD
    }
}
