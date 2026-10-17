//! Application queue limits, separate from codec capacity and texture ownership.
//!
//! The AMF learning rule follows Ramazan Kara's native AMF work in Vibepollo
//! PR #461 (GPL-3.0): https://github.com/Nonary/Vibepollo/pull/461.
//! This Rust adaptation keeps the sender's budget separate from encoder depth.

use std::time::{Duration, Instant};

/// AMF's existing blocking QueryOutput allowance.
pub const QUERY_TIMEOUT: Duration = Duration::from_millis(1);

/// One picture encoding and another queued or being sent.
pub const DEFAULT_LIMIT: usize = 2;
/// The native AMF adapter's existing bound on retained input surfaces.
pub const AMF_SURFACE_LIMIT: usize = 8;

/// Learn the smallest demonstrated AMF pipeline without shrinking its driver
/// queue or releasing surfaces before their native ownership callbacks.
#[derive(Debug)]
pub struct AmfBacklog {
    lookahead: Option<usize>,
    startup_limit: usize,
    retained: Option<usize>,
    outstanding: usize,
    service_time: Option<Duration>,
    frame_time: Option<Duration>,
    overlap_at: Option<Instant>,
    drain_until: Option<Instant>,
}

impl AmfBacklog {
    /// `None` preserves the fixed budget when the driver cannot report its
    /// pre-analysis requirements. An unsupported depth must not enable learning.
    pub fn new(lookahead: Option<usize>) -> Self {
        let lookahead = lookahead.filter(|depth| *depth < AMF_SURFACE_LIMIT);
        Self {
            lookahead,
            startup_limit: lookahead.map_or(DEFAULT_LIMIT, |depth| {
                (4 + depth * 2).min(AMF_SURFACE_LIMIT)
            }),
            retained: None,
            outstanding: 0,
            service_time: None,
            frame_time: None,
            overlap_at: None,
            drain_until: None,
        }
    }

    pub fn limit(&self) -> usize {
        self.retained.map_or(self.startup_limit, |depth| {
            depth.saturating_add(1).min(AMF_SURFACE_LIMIT)
        })
    }

    pub fn admission_limit(&self, now: Instant) -> usize {
        let limit = self.limit();
        if limit == 2 && self.drain_until.is_some_and(|due| now < due) {
            1
        } else if limit == 1 && self.overlap_at.is_some_and(|due| now >= due) {
            DEFAULT_LIMIT
        } else {
            limit
        }
    }

    /// A recovery picture must reach the sender before its next input is
    /// claimed, or that input can age behind a large paced IDR. Only a proven
    /// one-input encoder may pause its usual timed overlap. Bound the pause so
    /// a driver that unexpectedly needs another input can still make progress.
    pub fn recovery_admission_limit(&self, now: Instant, accepted: Option<Instant>) -> usize {
        let probe = self
            .service_time
            .unwrap_or(QUERY_TIMEOUT)
            .saturating_mul(2)
            .clamp(QUERY_TIMEOUT * 2, Duration::from_millis(10));
        if self.limit() == 1 && accepted.is_some_and(|at| now.saturating_duration_since(at) < probe)
        {
            1
        } else {
            self.admission_limit(now)
        }
    }

    /// Conservative work to overlap with the end of a paced recovery frame.
    /// A fastest valid sample prevents temporary GPU load from claiming an
    /// unnecessarily old picture. Leave one query allowance for earlier output.
    pub fn recovery_lead(&self) -> Option<Duration> {
        (self.limit() == 1)
            .then_some(self.frame_time)
            .flatten()
            .map(|time| time.saturating_sub(QUERY_TIMEOUT))
    }

    /// Call only after SubmitInput accepts a surface, never on INPUT_FULL.
    pub fn submitted(&mut self) {
        self.outstanding = self.outstanding.saturating_add(1);
    }

    /// Call only for valid output matched to an accepted submission. Expiring a
    /// timestamp or releasing a texture is not output and must not teach a
    /// smaller pipeline. Unreturned submissions remain in this accounting.
    pub fn completed(&mut self, now: Instant, encoding: Duration, preparation: Duration) {
        if self.outstanding == 0 {
            return;
        }
        self.outstanding -= 1;
        if let Some(lookahead) = self.lookahead {
            let observed = self.outstanding.max(lookahead);
            if self.retained.is_none() && lookahead == 0 && observed == 1 {
                // Once primed, give the remaining input one bounded chance to
                // return. Otherwise a busy source perpetually refills depth two
                // and never demonstrates whether one input can produce output.
                // A driver needing that second input resumes after this probe.
                self.drain_until = Some(now + encoding.min(Duration::from_millis(10)));
            }
            self.retained = Some(self.retained.map_or(observed, |old| old.min(observed)));
            // A saturated encoder needs conversion of its next input to overlap
            // the current picture. Claim near its predicted completion, rather
            // than keeping that next picture queued for an entire encode.
            // Keep the fastest sample so delayed polls and temporary GPU load
            // do not lengthen the wait and leave the encoder idle.
            let service = self.service_time.map_or(encoding, |old| old.min(encoding));
            self.service_time = Some(service);
            let total = encoding.saturating_add(preparation);
            self.frame_time = Some(self.frame_time.map_or(total, |old| old.min(total)));
            // Leave another poll allowance for the driver's wake-up jitter.
            self.overlap_at = Some(now + service.saturating_sub(preparation + QUERY_TIMEOUT * 2));
        }
    }
}

/// Sender occupancy keeps its existing two-picture allowance even when AMF
/// learns depth one. Larger codec startup/PA requirements must also fit.
pub fn total_limit(encoder_limit: usize) -> usize {
    DEFAULT_LIMIT.max(encoder_limit)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backpressure {
    None,
    Encoder,
    Sender,
}

pub fn backpressure(encoding: usize, sending: usize, encoder_limit: usize) -> Backpressure {
    if encoding >= encoder_limit {
        Backpressure::Encoder
    } else if encoding.saturating_add(sending) >= total_limit(encoder_limit) {
        Backpressure::Sender
    } else {
        Backpressure::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_waits_for_output_without_permanently_stopping_a_depth_one_encoder() {
        let now = Instant::now();
        let mut queue = AmfBacklog::new(Some(0));
        queue.submitted();
        queue.completed(now, Duration::from_millis(5), Duration::from_millis(1));
        let accepted = now + Duration::from_millis(1);
        queue.submitted();
        let overlap = accepted + Duration::from_millis(3);
        assert_eq!(queue.admission_limit(overlap), 2);
        assert_eq!(queue.recovery_admission_limit(overlap, Some(accepted)), 1);
        assert_eq!(queue.recovery_admission_limit(overlap, None), 2);
        // If this recovery picture unexpectedly requires another input, the
        // bounded probe restores overlap well before encoder-stall recovery.
        assert_eq!(
            queue.recovery_admission_limit(accepted + Duration::from_millis(10), Some(accepted)),
            2
        );
    }

    #[test]
    fn recovery_preserves_unknown_startup_and_preanalysis_priming() {
        let now = Instant::now();
        for depth in [None, Some(0), Some(1), Some(2), Some(7)] {
            let mut queue = AmfBacklog::new(depth);
            queue.submitted();
            assert_eq!(
                queue.recovery_admission_limit(now, Some(now)),
                queue.admission_limit(now)
            );
            assert_eq!(queue.recovery_lead(), None);
            if depth != Some(0) {
                queue.completed(now, Duration::from_millis(5), Duration::from_millis(1));
                assert_eq!(
                    queue.recovery_admission_limit(now, Some(now)),
                    queue.admission_limit(now)
                );
                assert_eq!(queue.recovery_lead(), None);
            }
        }
    }

    #[test]
    fn recovery_lead_uses_fastest_complete_work_and_resets_with_the_encoder() {
        let now = Instant::now();
        let mut queue = AmfBacklog::new(Some(0));
        for (encoding, preparation, expected) in [(5, 1, 5), (20, 3, 5), (2, 1, 2), (0, 0, 0)] {
            queue.submitted();
            queue.completed(
                now,
                Duration::from_millis(encoding),
                Duration::from_millis(preparation),
            );
            assert_eq!(queue.recovery_lead(), Some(Duration::from_millis(expected)));
        }
        assert_eq!(AmfBacklog::new(Some(0)).recovery_lead(), None);
    }

    fn complete(queue: &mut AmfBacklog) {
        queue.completed(
            Instant::now(),
            Duration::from_millis(5),
            Duration::from_millis(1),
        );
    }

    #[test]
    fn depth_one_admits_conversion_near_the_next_predicted_output() {
        let now = Instant::now();
        let mut queue = AmfBacklog::new(Some(0));
        queue.submitted();
        queue.completed(now, Duration::from_millis(5), Duration::from_millis(1));
        queue.submitted();
        assert_eq!(queue.admission_limit(now + Duration::from_micros(1999)), 1);
        assert_eq!(queue.admission_limit(now + Duration::from_millis(2)), 2);
        queue.submitted();
        assert_eq!(
            backpressure(2, 0, queue.admission_limit(now + Duration::from_secs(1))),
            Backpressure::Encoder
        );
    }

    #[test]
    fn each_valid_output_delays_the_next_claim_without_losing_network_overlap() {
        let now = Instant::now();
        let mut queue = AmfBacklog::new(Some(0));
        queue.submitted();
        queue.completed(now, Duration::from_millis(5), Duration::from_millis(1));
        assert_eq!(
            backpressure(0, 1, queue.admission_limit(now)),
            Backpressure::None
        );
        queue.submitted();
        let later = now + Duration::from_millis(6);
        assert_eq!(queue.admission_limit(later), 2);
        queue.completed(later, Duration::from_millis(5), Duration::from_millis(1));
        assert_eq!(queue.admission_limit(later), 1);
        assert_eq!(queue.admission_limit(later + Duration::from_millis(3)), 2);
    }

    #[test]
    fn faster_output_shortens_the_wait_and_a_slowdown_does_not_increase_it() {
        let now = Instant::now();
        let mut queue = AmfBacklog::new(Some(0));
        for encoding in [6, 4, 20] {
            queue.submitted();
            queue.completed(
                now,
                Duration::from_millis(encoding),
                Duration::from_millis(1),
            );
        }
        assert_eq!(queue.admission_limit(now), 1);
        assert_eq!(queue.admission_limit(now + Duration::from_millis(1)), 2);
        queue.submitted();
        queue.completed(now, Duration::from_millis(3), Duration::from_millis(4));
        assert_eq!(queue.admission_limit(now), 2);
    }

    #[test]
    fn overlap_timing_cannot_change_unknown_or_preanalysis_requirements() {
        let now = Instant::now();
        for lookahead in [None, Some(1), Some(2), Some(7)] {
            let mut queue = AmfBacklog::new(lookahead);
            queue.submitted();
            queue.completed(now, Duration::from_millis(5), Duration::from_millis(1));
            for delay in [0, 3, 1000] {
                assert_eq!(
                    queue.admission_limit(now + Duration::from_millis(delay)),
                    queue.limit()
                );
            }
        }
        let queue = AmfBacklog::new(Some(0));
        assert_eq!(queue.admission_limit(now + Duration::from_secs(1)), 4);
    }

    #[test]
    fn a_primed_pipeline_can_demonstrate_depth_one_without_a_source_pause() {
        let now = Instant::now();
        let mut queue = AmfBacklog::new(Some(0));
        queue.submitted();
        queue.submitted();
        queue.completed(now, Duration::from_millis(8), Duration::from_millis(1));
        assert_eq!(queue.limit(), 2);
        assert_eq!(queue.admission_limit(now), 1);
        assert_eq!(
            backpressure(1, 0, queue.admission_limit(now)),
            Backpressure::Encoder
        );
        let next = now + Duration::from_millis(4);
        queue.completed(next, Duration::from_millis(4), Duration::from_millis(1));
        assert_eq!(queue.limit(), 1);
        assert_eq!(
            backpressure(0, 1, queue.admission_limit(next)),
            Backpressure::None
        );
        assert_eq!(queue.admission_limit(next + Duration::from_millis(1)), 2);
    }

    #[test]
    fn a_driver_requiring_two_inputs_is_probed_only_once_and_never_stalls() {
        let now = Instant::now();
        let mut queue = AmfBacklog::new(Some(0));
        queue.submitted();
        queue.submitted();
        queue.completed(now, Duration::from_millis(500), Duration::from_millis(1));
        assert_eq!(queue.admission_limit(now + Duration::from_millis(9)), 1);
        assert_eq!(queue.admission_limit(now + Duration::from_millis(10)), 2);
        for frame in 1..100 {
            let next = now + Duration::from_millis(frame * 10);
            assert_eq!(queue.admission_limit(next), 2);
            queue.submitted();
            queue.completed(next, Duration::from_millis(5), Duration::from_millis(1));
            assert_eq!(queue.limit(), 2);
            assert_eq!(queue.admission_limit(next), 2);
        }
    }

    #[test]
    fn immediate_output_learns_one_and_waits_for_each_accepted_picture() {
        let mut queue = AmfBacklog::new(Some(0));
        assert_eq!(queue.limit(), 4);
        queue.submitted();
        complete(&mut queue);
        assert_eq!(queue.limit(), 1);
        queue.submitted();
        assert_eq!(backpressure(1, 0, queue.limit()), Backpressure::Encoder);
        complete(&mut queue);
        assert_eq!(backpressure(0, 1, queue.limit()), Backpressure::None);
    }

    #[test]
    fn startup_can_prime_a_driver_that_needs_four_inputs() {
        let mut queue = AmfBacklog::new(Some(0));
        for pending in 0..4 {
            assert_eq!(backpressure(pending, 0, queue.limit()), Backpressure::None);
            queue.submitted();
        }
        assert_eq!(backpressure(4, 0, queue.limit()), Backpressure::Encoder);
        complete(&mut queue);
        assert_eq!(queue.limit(), 4);
        // Once it demonstrably drains further, the next claim uses fewer inputs.
        complete(&mut queue);
        assert_eq!(queue.limit(), 3);
        complete(&mut queue);
        assert_eq!(queue.limit(), 2);
        complete(&mut queue);
        assert_eq!(queue.limit(), 1);
    }

    #[test]
    fn preanalysis_keeps_its_lookahead_even_if_output_temporarily_drains() {
        for depth in 1..AMF_SURFACE_LIMIT {
            let mut queue = AmfBacklog::new(Some(depth));
            assert!(queue.limit() > depth);
            assert!(queue.limit() <= AMF_SURFACE_LIMIT);
            for _ in 0..=depth {
                queue.submitted();
            }
            for _ in 0..=depth {
                complete(&mut queue);
                assert_eq!(queue.limit(), depth + 1);
            }
            assert_eq!(backpressure(depth, 0, queue.limit()), Backpressure::None);
            assert_eq!(
                backpressure(depth + 1, 0, queue.limit()),
                Backpressure::Encoder
            );
        }
    }

    #[test]
    fn a_driver_with_retained_inputs_keeps_producing_after_startup() {
        let mut queue = AmfBacklog::new(Some(0));
        let mut pending = 0;
        for _ in 0..100 {
            assert_eq!(backpressure(pending, 0, queue.limit()), Backpressure::None);
            queue.submitted();
            pending += 1;
            if pending == 4 {
                complete(&mut queue);
                pending -= 1;
                assert_eq!(queue.limit(), 4);
            }
        }
    }

    #[test]
    fn unavailable_or_unsupported_driver_requirements_keep_the_existing_limit() {
        for lookahead in [None, Some(AMF_SURFACE_LIMIT), Some(usize::MAX)] {
            let mut queue = AmfBacklog::new(lookahead);
            queue.submitted();
            complete(&mut queue);
            assert_eq!(queue.limit(), DEFAULT_LIMIT);
        }
    }

    #[test]
    fn empty_polls_and_unreturned_submissions_do_not_fake_a_shallower_pipeline() {
        let mut queue = AmfBacklog::new(Some(0));
        complete(&mut queue); // No accepted input: no observation.
        assert_eq!(queue.limit(), 4);
        queue.submitted(); // Its timestamp may expire, but no output arrived.
        queue.submitted();
        complete(&mut queue); // Only the other submission returned valid output.
        assert_eq!(queue.limit(), 2);
        for _ in 0..10 {
            queue.submitted();
            complete(&mut queue);
            assert_eq!(queue.limit(), 2);
        }
    }

    #[test]
    fn slow_output_cannot_grow_a_learned_queue_and_recreation_starts_fresh() {
        let mut queue = AmfBacklog::new(Some(0));
        queue.submitted();
        complete(&mut queue);
        queue.submitted();
        for _ in 0..100 {
            assert_eq!(backpressure(1, 0, queue.limit()), Backpressure::Encoder);
        }
        assert_eq!(queue.limit(), 1);
        queue = AmfBacklog::new(Some(0));
        assert_eq!(queue.limit(), 4);
    }

    #[test]
    fn sending_can_overlap_depth_one_without_starting_encoder_stall_recovery() {
        assert_eq!(backpressure(0, 1, 1), Backpressure::None);
        assert_eq!(backpressure(1, 1, 1), Backpressure::Encoder);
        assert_eq!(backpressure(0, 2, 1), Backpressure::Sender);
        assert_eq!(backpressure(0, usize::MAX, 1), Backpressure::Sender);
        assert_eq!(total_limit(1), 2);
        assert_eq!(backpressure(3, 0, 4), Backpressure::None);
        assert_eq!(backpressure(3, 1, 4), Backpressure::Sender);
    }

    #[test]
    fn other_encoders_keep_the_previous_combined_budget() {
        for encoding in 0..=4 {
            for sending in 0..=4 {
                let expected = if encoding >= 2 {
                    Backpressure::Encoder
                } else if encoding + sending >= 2 {
                    Backpressure::Sender
                } else {
                    Backpressure::None
                };
                assert_eq!(backpressure(encoding, sending, DEFAULT_LIMIT), expected);
            }
        }
    }
}
