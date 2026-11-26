//! Each guest's keyframe backoff. The owner's encoder makes one picture for everyone, so every
//! keyframe a guest asks for is a keyframe in the owner's stream too. A guest on a lossy link
//! that loses its picture again soon after each keyframe waits longer for the next one: 2 s,
//! then 4 s, then 8 s, while its losses keep coming that close together; one that asks after a
//! quiet spell starts at 2 s again. A guest's connection (or its return after an outage) is
//! never held back by its own backoff, and all guests together still share
//! [`super::KEYFRAME_INTERVAL`]. A guest that asks too soon is asked for again by its transport.

use std::time::{Duration, Instant};

/// A guest's wait after a keyframe, at first and after a quiet spell.
pub const BASE_INTERVAL: Duration = super::KEYFRAME_INTERVAL;
/// The longest a guest waits.
pub const MAX_INTERVAL: Duration = Duration::from_secs(8);

/// One guest's keyframe allowance.
#[derive(Debug)]
pub struct KeyframeBackoff {
    /// When the guest last had a keyframe asked for.
    last: Option<Instant>,
    /// How long after `last` it may have the next.
    interval: Duration,
}

impl Default for KeyframeBackoff {
    fn default() -> Self {
        Self {
            last: None,
            interval: BASE_INTERVAL,
        }
    }
}

impl KeyframeBackoff {
    /// Asks `ask`, the guests' shared allowance, for a keyframe when this guest may have one.
    /// `fresh` is a connection or a return after an outage: it needs a first picture, so the
    /// guest's own backoff does not hold it back, and the backoff starts over. Returns whether
    /// a keyframe was asked for.
    pub fn request(&mut self, now: Instant, fresh: bool, ask: impl FnOnce() -> bool) -> bool {
        if !fresh && !self.ready(now) {
            return false;
        }
        if !ask() {
            return false;
        }
        self.granted(now, fresh);
        true
    }

    /// Whether the guest's wait since its last keyframe is over.
    pub fn ready(&self, now: Instant) -> bool {
        self.last
            .is_none_or(|last| now.saturating_duration_since(last) >= self.interval)
    }

    /// How long the guest waits after its last keyframe.
    pub fn interval(&self) -> Duration {
        self.interval
    }

    fn granted(&mut self, now: Instant, fresh: bool) {
        if let Some(last) = self.last.filter(|_| !fresh) {
            // Asked again within twice its wait: the losses go on, so the wait doubles. After
            // a quiet spell it starts over.
            self.interval = if now.saturating_duration_since(last) < self.interval * 2 {
                (self.interval * 2).min(MAX_INTERVAL)
            } else {
                BASE_INTERVAL
            };
        } else {
            self.interval = BASE_INTERVAL;
        }
        self.last = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A guest that asks every 250 ms for `seconds`, as a transport waiting for a keyframe
    /// does, with the shared allowance always granting: the milliseconds it was granted at.
    fn grants(backoff: &mut KeyframeBackoff, start: Instant, from_ms: u64, to_ms: u64) -> Vec<u64> {
        (from_ms..=to_ms)
            .step_by(250)
            .filter(|ms| backoff.request(start + Duration::from_millis(*ms), false, || true))
            .collect()
    }

    #[test]
    fn a_guest_that_keeps_losing_waits_longer_each_time_up_to_the_cap() {
        let start = Instant::now();
        let mut backoff = KeyframeBackoff::default();
        assert!(backoff.request(start, true, || true));
        // The connection's keyframe, then 2, 4, 8 and 8 s waits.
        assert_eq!(
            grants(&mut backoff, start, 250, 30_000),
            [2_000, 6_000, 14_000, 22_000, 30_000]
        );
        assert_eq!(backoff.interval(), MAX_INTERVAL);
    }

    #[test]
    fn a_quiet_spell_starts_the_wait_over() {
        let start = Instant::now();
        let mut backoff = KeyframeBackoff::default();
        assert!(backoff.request(start, true, || true));
        assert_eq!(grants(&mut backoff, start, 250, 6_000), [2_000, 6_000]);
        assert_eq!(backoff.interval(), Duration::from_secs(8));
        // No loss for 20 s, more than twice the wait: back to 2 s.
        assert_eq!(
            grants(&mut backoff, start, 26_000, 30_000),
            [26_000, 28_000]
        );
        assert_eq!(backoff.interval(), Duration::from_secs(4));
    }

    #[test]
    fn a_connection_or_a_return_is_never_held_back_and_starts_over() {
        let start = Instant::now();
        let mut backoff = KeyframeBackoff::default();
        assert!(backoff.request(start, true, || true));
        grants(&mut backoff, start, 250, 14_000);
        assert_eq!(backoff.interval(), MAX_INTERVAL);
        let back = start + Duration::from_millis(14_500);
        assert!(!backoff.ready(back));
        assert!(backoff.request(back, true, || true));
        assert_eq!(backoff.interval(), BASE_INTERVAL);
    }

    #[test]
    fn a_refused_request_leaves_the_backoff_as_it_was() {
        let start = Instant::now();
        let mut backoff = KeyframeBackoff::default();
        assert!(!backoff.request(start, true, || false));
        assert!(backoff.ready(start));
        assert!(backoff.request(start, false, || true));
        assert!(!backoff.request(start + Duration::from_secs(1), false, || true));
        // The shared allowance refusing does not count as a keyframe.
        let later = start + Duration::from_secs(3);
        assert!(!backoff.request(later, false, || false));
        assert_eq!(backoff.interval(), BASE_INTERVAL);
        assert!(backoff.request(later, false, || true));
        assert_eq!(backoff.interval(), Duration::from_secs(4));
    }
}
