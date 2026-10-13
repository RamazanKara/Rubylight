//! Application queue limits, separate from codec capacity and texture ownership.
//!
//! The AMF learning rule follows Ramazan Kara's native AMF work in Vibepollo
//! PR #461 (GPL-3.0): https://github.com/Nonary/Vibepollo/pull/461.
//! This Rust adaptation keeps the sender's budget separate from encoder depth.

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
        }
    }

    pub fn limit(&self) -> usize {
        self.retained.map_or(self.startup_limit, |depth| {
            depth.saturating_add(1).min(AMF_SURFACE_LIMIT)
        })
    }

    /// Call only after SubmitInput accepts a surface, never on INPUT_FULL.
    pub fn submitted(&mut self) {
        self.outstanding = self.outstanding.saturating_add(1);
    }

    /// Call only for valid output matched to an accepted submission. Expiring a
    /// timestamp or releasing a texture is not output and must not teach a
    /// smaller pipeline. Unreturned submissions remain in this accounting.
    pub fn completed(&mut self) {
        if self.outstanding == 0 {
            return;
        }
        self.outstanding -= 1;
        if let Some(lookahead) = self.lookahead {
            let observed = self.outstanding.max(lookahead);
            self.retained = Some(self.retained.map_or(observed, |old| old.min(observed)));
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
    fn immediate_output_learns_one_and_waits_for_each_accepted_picture() {
        let mut queue = AmfBacklog::new(Some(0));
        assert_eq!(queue.limit(), 4);
        queue.submitted();
        queue.completed();
        assert_eq!(queue.limit(), 1);
        queue.submitted();
        assert_eq!(backpressure(1, 0, queue.limit()), Backpressure::Encoder);
        queue.completed();
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
        queue.completed();
        assert_eq!(queue.limit(), 4);
        // Once it demonstrably drains further, the next claim uses fewer inputs.
        queue.completed();
        assert_eq!(queue.limit(), 3);
        queue.completed();
        assert_eq!(queue.limit(), 2);
        queue.completed();
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
                queue.completed();
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
                queue.completed();
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
            queue.completed();
            assert_eq!(queue.limit(), DEFAULT_LIMIT);
        }
    }

    #[test]
    fn empty_polls_and_unreturned_submissions_do_not_fake_a_shallower_pipeline() {
        let mut queue = AmfBacklog::new(Some(0));
        queue.completed(); // No accepted input: no observation.
        assert_eq!(queue.limit(), 4);
        queue.submitted(); // Its timestamp may expire, but no output arrived.
        queue.submitted();
        queue.completed(); // Only the other submission returned valid output.
        assert_eq!(queue.limit(), 2);
        for _ in 0..10 {
            queue.submitted();
            queue.completed();
            assert_eq!(queue.limit(), 2);
        }
    }

    #[test]
    fn slow_output_cannot_grow_a_learned_queue_and_recreation_starts_fresh() {
        let mut queue = AmfBacklog::new(Some(0));
        queue.submitted();
        queue.completed();
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
