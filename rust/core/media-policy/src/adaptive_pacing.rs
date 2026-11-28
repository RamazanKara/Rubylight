//! Loss-responsive packet spacing, independent of encoder bitrate.
//!
//! Start with room for the stream's wire demand, then cautiously probe the
//! route's ordinary ceiling. Validated video loss can reduce bursts, but never
//! below the current wire demand plus headroom. This is not a measurement of
//! path capacity: control pings establish activity, not successful video
//! delivery. Silence alone never authorizes faster probing.
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::adaptive_fec::Block;
mod post_loss;
mod rtt_watch;
use post_loss::{PostLoss, PostLossConfig};
use rtt_watch::{QUEUE_DELAY_MARGIN, QueueWatch};

#[derive(Clone, Copy, Default)]
pub struct Traffic {
    pub frames: u64,
    pub bytes: u64,
    pub packets: u64,
    pub send_losses: u64,
}

/// Reason why automatic burst pacing changed its rate on a control tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacingReason {
    /// Clean startup feedback allowed a tentative headroom step.
    StartupHeadroom,
    /// Clean post-recovery feedback allowed a tentative probe step.
    RecoveryProbe,
    /// A tentative increase met loss and restored the previous stable rate.
    ProbeLoss,
    /// Sustained or unrecoverable frame loss cut the pacing rate in half.
    PacketLoss,
    /// Mild loss on a wireless client without a known link rate stepped pacing down.
    MinorLoss,
    /// Sustained RTT queue growth on an unknown-link wireless route stepped pacing down.
    QueueDelay,
    /// Control-channel liveness or RTT feedback cleared on an unknown-link wireless route.
    HeadroomCleared,
}

impl PacingReason {
    /// Stable log key for this pacing transition reason.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StartupHeadroom => "startup_headroom",
            Self::RecoveryProbe => "recovery_probe",
            Self::ProbeLoss => "probe_loss",
            Self::PacketLoss => "packet_loss",
            Self::MinorLoss => "minor_loss",
            Self::QueueDelay => "queue_delay",
            Self::HeadroomCleared => "headroom_cleared",
        }
    }
}

/// One pacing rate step returned by [`AdaptivePacing::tick_step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PacingStep {
    pub old_bps: u64,
    pub new_bps: u64,
    pub reason: PacingReason,
    /// True when `reason` differs from the previous logged step's reason.
    pub reason_changed: bool,
}

pub struct AdaptivePacing {
    enabled: bool,
    initialized: bool,
    startup: bool,
    wireless_unknown_link: bool,
    route_ceiling: u64,
    ceiling: u64,
    floor: u64,
    limit: Option<u64>,
    last_sample: Option<(Instant, Traffic)>,
    last_ping: Option<Instant>,
    /// Lowest RTT per second over [`RTT_FLOOR_WINDOW`], oldest first.
    rtt_floor: VecDeque<(Instant, Duration)>,
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
    unhealthy_since: Option<(PacingReason, Instant)>,
    queue_watch: Option<QueueWatch>,
    last_logged_reason: Option<PacingReason>,
    /// A rate increase is tentative until its feedback interval has elapsed.
    probe: Option<Probe>,
    /// A changed wire budget invalidates feedback for the old rate. The
    /// control loop supplies the new sent-frame fence before sampling again.
    awaiting_fence: bool,
    recovery_hold: Duration,
    /// Wired post-loss refinement (`None` for Wi-Fi/`Always`/`Off`).
    post_loss: Option<PostLoss>,
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
            wireless_unknown_link: false,
            route_ceiling: 0,
            ceiling: 0,
            floor: 0,
            limit: None,
            last_sample: None,
            last_ping: None,
            rtt_floor: VecDeque::new(),
            after_frame: None,
            reported_frames: Vec::new(),
            lost_frames: 0,
            failed_frames: 0,
            lost_packets: 0,
            any_loss: false,
            clean_since: None,
            unhealthy_since: None,
            queue_watch: None,
            last_logged_reason: None,
            probe: None,
            awaiting_fence: false,
            recovery_hold: Duration::from_secs(8),
            post_loss: None,
        }
    }

    /// Start adaptive pacing after sustained loss, seeded at twice the bitrate
    /// and fenced so reports up to `after_frame` from the prior rate cannot
    /// trigger an immediate cut.
    pub fn new_after_loss(after_frame: Option<u32>, failed_bps: Option<u64>) -> Self {
        Self::new_after_loss_with(after_frame, failed_bps, PostLossConfig::DEFAULT)
    }

    /// Start post-loss adaptive pacing with an explicit [`PostLossConfig`].
    pub(self) fn new_after_loss_with(
        after_frame: Option<u32>,
        failed_bps: Option<u64>,
        config: PostLossConfig,
    ) -> Self {
        let mut pacing = Self::new(true);
        pacing.after_frame = after_frame;
        pacing.post_loss = Some(PostLoss::new(failed_bps, config));
        pacing
    }

    /// Update the active FEC percentage (used by post-loss low-FEC probe guards).
    pub fn set_fec_percent(&mut self, fec_percent: usize) {
        if let Some(post_loss) = &mut self.post_loss {
            post_loss.set_fec_percent(fec_percent);
        }
    }

    /// Mark whether this stream goes to a wireless client whose downstream link
    /// speed is unknown, enabling step-down when headroom triggers clear.
    pub fn set_wireless_unknown_link(&mut self, wireless_unknown_link: bool) {
        self.wireless_unknown_link = wireless_unknown_link;
        if !wireless_unknown_link {
            self.unhealthy_since = None;
            self.queue_watch = None;
        }
    }

    fn seed_rate(&self, encoder_kbps: u32) -> u64 {
        u64::from(encoder_kbps)
            .saturating_mul(2000)
            .clamp(self.floor, self.ceiling)
    }

    /// Whether this controller owns the stream's pacing.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    fn effective_ceiling(
        wireless_unknown_link: bool,
        post_loss: Option<&PostLoss>,
        route_ceiling: u64,
        needed: u64,
        encoder_kbps: u32,
    ) -> u64 {
        let route_ceiling = post_loss.map_or(route_ceiling.max(1), |p| {
            p.ceiling(route_ceiling, needed, encoder_kbps)
        });
        if wireless_unknown_link {
            crate::network_pacing::wireless_unknown_link_ceiling_bps(
                route_ceiling,
                encoder_kbps,
                needed,
            )
        } else {
            route_ceiling
        }
    }

    fn refresh_bounds(&mut self, needed: u64, encoder_kbps: u32) {
        self.ceiling = Self::effective_ceiling(
            self.wireless_unknown_link,
            self.post_loss.as_ref(),
            self.route_ceiling,
            needed,
            encoder_kbps,
        );
        self.floor = needed
            .saturating_add(needed / 4)
            .max(1_000_000)
            .min(self.ceiling);
    }

    /// Refresh on each frame: FEC, FPS, bitrate and the route can change.
    /// The physical/configured ceiling wins when the requested stream cannot
    /// fit. The caller reports that shortfall; this controller cannot fix it.
    pub fn rate(&mut self, route_ceiling: u64, needed: u64, encoder_kbps: u32) -> u64 {
        let previous_rate = self.current();
        let previous_route_ceiling = self.route_ceiling;
        let previous_ceiling = self.ceiling;
        let previous_floor = self.floor;
        self.route_ceiling = route_ceiling.max(1);
        self.refresh_bounds(needed, encoder_kbps);
        if !self.enabled {
            return route_ceiling;
        }
        if !self.initialized {
            self.initialized = true;
            self.limit = Some(self.seed_rate(encoder_kbps));
        } else if self.route_ceiling != previous_route_ceiling
            || self.ceiling != previous_ceiling
            || self.floor != previous_floor
        {
            let route_changed = self.route_ceiling != previous_route_ceiling;
            if route_changed {
                // Route activity and RTT from before a changed route ceiling do
                // not validate probing the replacement route.
                self.last_ping = None;
                self.rtt_floor.clear();
                self.unhealthy_since = None;
                self.queue_watch = None;
            }
            if self.ceiling > previous_ceiling && self.limit.is_none() {
                // A newly faster ceiling says nothing about downstream
                // capacity. Keep the previously confirmed rate and probe the
                // extra headroom using fresh evidence, without reseeding.
                self.limit = Some(previous_rate);
                self.startup = !self.any_loss;
            }
            // A tentative faster rate has not earned promotion. Retain its
            // preceding rate, adjusted for the new demand, and gather fresh
            // evidence instead of confirming across a changed wire budget.
            let retracted_probe = if let Some(probe) = self.probe.take() {
                self.limit = Some(probe.stable);
                if self.any_loss {
                    // The same report may raise FEC before the control loop
                    // samples it. A changed wire floor must not erase this
                    // failed probe or restart fast startup probing.
                    self.startup = false;
                    self.recovery_hold = (self.recovery_hold * 2).min(Duration::from_secs(60));
                    if let Some(post_loss) = &mut self.post_loss {
                        post_loss.on_probe_loss(probe.stable);
                        self.refresh_bounds(needed, encoder_kbps);
                    }
                }
                true
            } else {
                false
            };
            if let Some(limit) = &mut self.limit {
                *limit = (*limit).clamp(self.floor, self.ceiling);
            }
            let next_rate = self.current();
            if route_changed {
                self.clean_since = None;
                self.last_sample = None;
                self.awaiting_fence = true;
            } else {
                if retracted_probe || next_rate != previous_rate {
                    self.clean_since = None;
                }
                if retracted_probe || next_rate > previous_rate {
                    self.awaiting_fence = true;
                }
            }
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
    pub fn preview_rate(&self, route_ceiling: u64, needed: u64, encoder_kbps: u32) -> u64 {
        self.preview_rate_for(
            route_ceiling,
            needed,
            encoder_kbps,
            self.wireless_unknown_link,
        )
    }

    /// Like [`Self::preview_rate`], evaluating the effective ceiling for the
    /// caller's current `wireless_unknown_link` state without mutating `self`.
    pub fn preview_rate_for(
        &self,
        route_ceiling: u64,
        needed: u64,
        encoder_kbps: u32,
        wireless_unknown_link: bool,
    ) -> u64 {
        if !self.enabled {
            return route_ceiling;
        }
        let route_ceiling = route_ceiling.max(1);
        let ceiling = Self::effective_ceiling(
            wireless_unknown_link,
            self.post_loss.as_ref(),
            route_ceiling,
            needed,
            encoder_kbps,
        );
        let floor = needed
            .saturating_add(needed / 4)
            .max(1_000_000)
            .min(ceiling);
        let limit = if !self.initialized {
            Some(u64::from(encoder_kbps).saturating_mul(2000))
        } else if route_ceiling != self.route_ceiling
            || ceiling != self.ceiling
            || floor != self.floor
        {
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
        self.on_block_kind(frame, last_sent, period, block, false);
    }

    /// Like [`Self::on_block`], with whether `frame` was sent as a keyframe.
    pub fn on_block_kind(
        &mut self,
        frame: u32,
        last_sent: u32,
        period: Duration,
        block: Block,
        keyframe: bool,
    ) {
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
        let mut newly_failed = false;
        if let Some((_, was_failed)) = self
            .reported_frames
            .iter_mut()
            .find(|(seen, _)| *seen == frame)
        {
            if failed && !*was_failed {
                *was_failed = true;
                self.failed_frames = self.failed_frames.saturating_add(1);
                newly_failed = true;
            }
        } else {
            self.reported_frames.push((frame, failed));
            self.lost_frames = self.lost_frames.saturating_add(1);
            self.failed_frames = self.failed_frames.saturating_add(u32::from(failed));
            newly_failed = failed;
        }
        if newly_failed && let Some(post_loss) = &mut self.post_loss {
            post_loss.on_failed_block(keyframe);
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
        self.tick_step(now, traffic, last_sent, period, encoder_kbps, rtt)
            .map(|step| (step.old_bps, step.new_bps))
    }

    /// Like [`Self::tick`], also reporting the trigger reason and whether the
    /// reason changed since the previous logged step.
    pub fn tick_step(
        &mut self,
        now: Instant,
        traffic: Traffic,
        last_sent: Option<u32>,
        period: Duration,
        encoder_kbps: u32,
        rtt: Duration,
    ) -> Option<PacingStep> {
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
            if let Some(post_loss) = &mut self.post_loss {
                post_loss.clear_keyframe_failed();
            }
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
        let keyframe_failed = self
            .post_loss
            .as_mut()
            .is_some_and(PostLoss::take_keyframe_failed);
        let substantial = keyframe_failed
            || self.failed_frames >= 2
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
            self.observe_rtt(now, rtt);
        }
        let rtt_floor = self.rtt_floor();
        let queued = rtt_floor.is_some_and(|floor| {
            !rtt.is_zero() && rtt > floor.saturating_add((floor / 2).max(Duration::from_millis(8)))
        });
        let mut reason = None;
        if (substantial || (self.probe.is_some() && loss)) && recent && (frames > 0 || probe_loss) {
            self.unhealthy_since = None;
            self.queue_watch = None;
            let next = if let Some(probe) = self.probe.take() {
                // A failed probe restores the last working rate, and increases
                // the quiet period before another attempt (up to a minute).
                self.recovery_hold = (self.recovery_hold * 2).min(Duration::from_secs(60));
                reason = Some(PacingReason::ProbeLoss);
                if let Some(post_loss) = &mut self.post_loss {
                    post_loss.on_probe_loss(probe.stable);
                    self.ceiling = post_loss.cap_ceiling(self.ceiling, self.floor);
                }
                probe.stable
            } else {
                reason = Some(PacingReason::PacketLoss);
                if let Some(post_loss) = &mut self.post_loss {
                    let next = post_loss.packet_loss_rate(old, self.floor, self.ceiling);
                    self.ceiling = post_loss.cap_ceiling(self.ceiling, self.floor);
                    next
                } else {
                    old / 2
                }
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
            let queue_excess = (busy && ping && !rtt.is_zero())
                .then_some(rtt_floor)
                .flatten()
                .and_then(|floor| {
                    let excess = rtt.saturating_sub(floor);
                    (excess > (floor / 2).max(QUEUE_DELAY_MARGIN)).then_some(excess)
                });
            if loss || !ping || !busy || rtt.is_zero() || queued {
                self.clean_since = None;
                if self.wireless_unknown_link {
                    let clear_reason = if loss {
                        Some(PacingReason::MinorLoss)
                    } else if queue_excess.is_some() {
                        Some(PacingReason::QueueDelay)
                    } else if !ping || rtt.is_zero() {
                        Some(PacingReason::HeadroomCleared)
                    } else {
                        None
                    };
                    let prior_queue_started = if clear_reason != Some(PacingReason::QueueDelay) {
                        self.queue_watch.take().map(|watch| watch.started)
                    } else {
                        None
                    };
                    if let Some(clear_reason) = clear_reason {
                        let ready = if let Some(excess) =
                            queue_excess.filter(|_| clear_reason == PacingReason::QueueDelay)
                        {
                            let prior_headroom_started = match self.unhealthy_since.take() {
                                Some((PacingReason::HeadroomCleared, at)) => Some(at),
                                _ => None,
                            };
                            self.advance_queue_watch(now, excess, prior_headroom_started)
                        } else {
                            let started = match self.unhealthy_since {
                                Some((prev, at)) if prev == clear_reason => at,
                                _ => {
                                    let at = if clear_reason == PacingReason::HeadroomCleared {
                                        prior_queue_started.unwrap_or(now)
                                    } else {
                                        now
                                    };
                                    self.unhealthy_since = Some((clear_reason, at));
                                    at
                                }
                            };
                            let hold = match clear_reason {
                                PacingReason::MinorLoss => Duration::ZERO,
                                _ => Duration::from_secs(2),
                            };
                            now.saturating_duration_since(started) >= hold
                        };
                        let baseline = self.seed_rate(encoder_kbps);
                        if old > baseline && ready {
                            let step = (old / 8).max(1_000_000);
                            let next = old
                                .saturating_sub(step)
                                .max(baseline)
                                .clamp(self.floor, self.ceiling);
                            if next < old {
                                self.limit = Some(next);
                                self.probe = None;
                                self.after_frame = last_sent;
                                if let Some(excess) = queue_excess
                                    .filter(|_| clear_reason == PacingReason::QueueDelay)
                                {
                                    self.queue_watch = Some(QueueWatch {
                                        started: now,
                                        min_excess: excess,
                                    });
                                } else {
                                    self.unhealthy_since = Some((clear_reason, now));
                                }
                                if matches!(
                                    clear_reason,
                                    PacingReason::MinorLoss | PacingReason::QueueDelay
                                ) {
                                    self.startup = false;
                                }
                                reason = Some(clear_reason);
                            }
                        }
                    } else {
                        self.unhealthy_since = None;
                    }
                }
            } else {
                self.unhealthy_since = None;
                self.queue_watch = None;
                if self.limit.is_some() {
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
                        if !self.post_loss.as_ref().is_none_or(PostLoss::probe_allowed) {
                            // Hold the current safe rate when low FEC leaves too little parity for probes.
                        } else if old < self.ceiling {
                            let step = if self.startup {
                                // Smaller probes leave ordinary FEC more room to
                                // repair losses while feedback returns. This is
                                // headroom, not a guarantee of loss-free probing.
                                (old / 8).max(1)
                            } else {
                                (old / 20).max(1_000_000)
                            };
                            reason = Some(if self.startup {
                                PacingReason::StartupHeadroom
                            } else {
                                PacingReason::RecoveryProbe
                            });
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
        }
        let next = self.current();
        if next == old {
            return None;
        }
        let reason = reason.unwrap_or(PacingReason::RecoveryProbe);
        let reason_changed = self.last_logged_reason != Some(reason);
        self.last_logged_reason = Some(reason);
        Some(PacingStep {
            old_bps: old,
            new_bps: next,
            reason,
            reason_changed,
        })
    }

    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "automatic": self.enabled,
            "startup": self.startup,
            "wireless_unknown_link": self.wireless_unknown_link,
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
