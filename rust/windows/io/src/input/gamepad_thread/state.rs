//! Event and audio queues, polling cadence, and timing histograms for [`super::GamepadThread`].

use super::*;

pub(super) struct AudioInput {
    pub(super) id: u16,
    pub(super) kind: u16,
    pub(super) payload: Vec<u8>,
    pub(super) at: Instant,
}

#[derive(Default)]
pub(super) struct AudioQueue {
    // Status is reliable transport state, not a 20 ms audio sample. Keep it
    // until the controller leaves so a slow USB attachment can replay it.
    pub(super) status: [Option<[u8; 8]>; 16],
    pending: u16,
    pub(super) pcm: VecDeque<AudioInput>,
}
impl AudioQueue {
    pub(super) fn push(&mut self, id: u16, kind: u16, payload: &[u8], now: Instant) {
        if id >= 16 || payload.len() > 972 {
            return;
        }
        if kind == controller_audio::CONTROL {
            if controller_audio::status(payload).is_some_and(|(index, _)| index == id) {
                let status = Some(payload.try_into().unwrap());
                if self.status[id as usize] != status {
                    self.status[id as usize] = status;
                    self.pending |= 1 << id;
                }
            }
            return;
        }
        self.pcm
            .retain(|input| now.saturating_duration_since(input.at) <= haptics::MAX_AGE);
        if self.pcm.len() >= 64 {
            self.pcm.pop_front();
        }
        self.pcm.push_back(AudioInput {
            id,
            kind,
            payload: payload.to_vec(),
            at: now,
        });
    }
    pub(super) fn arrived(&mut self, id: u16) {
        if id < 16 && self.status[id as usize].is_some() {
            self.pending |= 1 << id;
        }
    }
    pub(super) fn arrived_mask(&mut self, mask: u16) {
        for id in 0..16 {
            if mask & (1 << id) != 0 {
                self.arrived(id);
            }
        }
    }
    pub(super) fn retain_controllers(&mut self, active: u16) {
        for (id, status) in self.status.iter_mut().enumerate() {
            if active & (1 << id) == 0 {
                *status = None;
            }
        }
        self.pending &= active;
        self.pcm.retain(|input| active & (1 << input.id) != 0);
    }
    pub(super) fn release_pcm(&mut self) {
        self.pcm.clear();
    }
    pub(super) fn replay_status(&mut self) {
        // Releasing held input temporarily suspends the device's audio too.
        // Restore the same transport's status after that release completes.
        for id in 0..16 {
            self.arrived(id);
        }
    }
    pub(super) fn clear(&mut self) {
        self.status.fill(None);
        self.pending = 0;
        self.pcm.clear();
    }
    pub(super) fn take(&mut self, now: Instant) -> Vec<AudioInput> {
        let pending = std::mem::take(&mut self.pending);
        let mut inputs = Vec::new();
        for (id, status) in self.status.iter().enumerate() {
            if pending & (1 << id) != 0
                && let Some(payload) = status
            {
                inputs.push(AudioInput {
                    id: id as u16,
                    kind: controller_audio::CONTROL,
                    payload: payload.to_vec(),
                    at: now,
                });
            }
        }
        inputs.extend(
            self.pcm
                .drain(..)
                .filter(|input| now.saturating_duration_since(input.at) <= haptics::MAX_AGE),
        );
        inputs
    }
}

#[derive(Default)]
pub(super) struct Queue {
    pub(super) events: VecDeque<Event>,
    pub(super) policy: Option<butterpollo_input_policy::input_policy::Policy>,
    pub(super) release: bool,
    /// Whether events were dropped since the thread last took the queue.
    pub(super) full: bool,
    /// When the oldest event waiting was queued, for `gamepad timings`.
    pub(super) oldest: Option<Instant>,
}
impl Queue {
    /// Hand the worker one batch and give producers its empty spare buffer.
    /// Both allocations survive draining, without holding this lock in a driver call.
    pub(super) fn take_into(
        &mut self,
        batch: &mut VecDeque<Event>,
    ) -> (bool, Option<butterpollo_input_policy::input_policy::Policy>) {
        debug_assert!(batch.is_empty());
        std::mem::swap(&mut self.events, batch);
        self.full = false;
        (std::mem::take(&mut self.release), self.policy.take())
    }
}

/// Queue `event`, or fold it into a queued event of the same pad as
/// Input::merge allows: a stick or motion update replaces the queued one, so
/// a stalled driver catches up with the latest state, while every button
/// and active-mask change keeps its place. An update may pass over other
/// pads' events and other kinds of event, never over an arrival. Returns
/// false when the event was dropped: QUEUE_LIMIT events already wait.
pub(super) fn enqueue(events: &mut VecDeque<Event>, event: Event) -> bool {
    if !matches!(event, Event::Arrival { .. }) {
        for queued in events.iter_mut().rev() {
            if matches!(queued, Event::Arrival { .. }) {
                break;
            }
            let same_kind = std::mem::discriminant(&*queued) == std::mem::discriminant(&event);
            match queued.merge(&event) {
                Batch::Merged => return true,
                Batch::Stop if same_kind => break,
                Batch::Skip | Batch::Stop => {}
            }
        }
    }
    if events.len() >= QUEUE_LIMIT {
        return false;
    }
    events.push_back(event);
    true
}

/// How often the thread polls feedback while pads are plugged, and how soon
/// it opens a driver again; tests shorten both.
#[derive(Clone, Copy)]
pub(super) struct Polling {
    pub(super) every: Duration,
    /// `gamepad_feedback_fast_poll`: every FAST_POLL for FAST_SPAN after a
    /// driver had feedback.
    pub(super) fast: bool,
    /// Graduated retry backoff steps (`2s -> 5s -> 15s -> 60s` in production).
    pub(super) backoff: [Duration; 4],
    /// Fast poll interval while bounded PnP recovery is actively restarting the driver.
    pub(super) reopen: Duration,
}
impl Polling {
    pub(super) fn next(&self, now: Instant, fast_until: Option<Instant>) -> Instant {
        let fast = self.fast && fast_until.is_some_and(|until| now < until);
        now + if fast {
            FAST_POLL.min(self.every)
        } else {
            self.every
        }
    }
    pub(super) fn backoff_for(&self, step: usize) -> Duration {
        self.backoff[step.min(self.backoff.len() - 1)]
    }
    pub(super) fn slow_retry(&self) -> Duration {
        self.backoff[self.backoff.len() - 1]
    }
}

/// What the `gamepad timings` line reports: how long controller events
/// waited from [`GamepadThread::send`] until the driver had them, and how
/// much feedback each poll read.
#[derive(Default)]
pub(super) struct Stats {
    pub(super) since: Option<Instant>,
    /// Per batch: the oldest event's time from queued to submitted.
    pub(super) waits: Vec<Duration>,
    pub(super) depth_max: usize,
    pub(super) polls: usize,
    pub(super) drained: usize,
    pub(super) drained_max: usize,
}
impl Stats {
    pub(super) fn batch(&mut self, queued: Instant, depth: usize, now: Instant) {
        self.since.get_or_insert(now);
        if self.waits.len() < 1 << 16 {
            self.waits.push(now.saturating_duration_since(queued));
        }
        self.depth_max = self.depth_max.max(depth);
    }
    pub(super) fn poll(&mut self, drained: usize, now: Instant) {
        self.since.get_or_insert(now);
        self.polls += 1;
        self.drained += drained;
        self.drained_max = self.drained_max.max(drained);
    }
    pub(super) fn has_activity(&self) -> bool {
        !self.waits.is_empty() || self.drained > 0
    }
    /// The line for the window that ended, once STATS_EVERY has passed.
    pub(super) fn take_due(&mut self, now: Instant) -> Option<Self> {
        let since = self.since?;
        (now.saturating_duration_since(since) >= STATS_EVERY).then(|| std::mem::take(self))
    }
    pub(super) fn log(mut self) {
        let active = self.has_activity();
        if !active && !tracing::enabled!(tracing::Level::DEBUG) {
            return;
        }
        let micros = |d: Option<&Duration>| d.map_or(0, |d| d.as_micros() as u64);
        self.waits.sort_unstable();
        let at = |q: usize| {
            self.waits
                .get((self.waits.len().saturating_sub(1)) * q / 100)
        };
        macro_rules! emit {
            ($level:ident) => {
                tracing::$level!(
                    batches = self.waits.len(),
                    queued_p50_us = micros(at(50)),
                    queued_p99_us = micros(at(99)),
                    queued_max_us = micros(self.waits.last()),
                    queue_depth_max = self.depth_max,
                    feedback_polls = self.polls,
                    feedback_drained = self.drained,
                    feedback_drained_max = self.drained_max,
                    "gamepad timings"
                )
            };
        }
        if active {
            emit!(info);
        } else {
            emit!(debug);
        }
    }
}
