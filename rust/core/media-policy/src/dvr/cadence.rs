//! Soft keyframe cadence while Stream DVR is armed: Moonlight streams run an infinite
//! GOP, so the replay ring asks the encoder for an IDR whenever the last one is older than
//! the DVR GOP, at most once per retry interval. Disarmed streams never consult it.

use std::time::{Duration, Instant};

/// Default DVR GOP length in seconds.
pub const GOP_SECS: u64 = 4;

/// How long a request may stay unanswered before it is repeated.
pub const RETRY: Duration = Duration::from_secs(1);

/// The DVR GOP length, clamped to 2..=10 seconds.
pub fn gop(secs: u64) -> Duration {
    Duration::from_secs(secs.clamp(2, 10))
}

#[derive(Clone, Debug)]
pub struct KeyframeCadence {
    gop: Duration,
    last_idr: Option<Instant>,
    requested: Option<Instant>,
}

impl KeyframeCadence {
    pub fn new(gop: Duration) -> Self {
        Self {
            gop,
            last_idr: None,
            requested: None,
        }
    }

    pub fn gop(&self) -> Duration {
        self.gop
    }

    /// The encoder produced a keyframe (requested or not).
    pub fn observe_idr(&mut self, now: Instant) {
        self.last_idr = Some(now);
        self.requested = None;
    }

    /// Whether to request a keyframe now. After `true` it waits for a keyframe or
    /// [`RETRY`] before asking again.
    pub fn due(&mut self, now: Instant) -> bool {
        if self
            .requested
            .is_some_and(|at| now.saturating_duration_since(at) < RETRY)
        {
            return false;
        }
        let stale = self
            .last_idr
            .is_none_or(|at| now.saturating_duration_since(at) >= self.gop);
        if stale {
            self.requested = Some(now);
        }
        stale
    }

    /// Forgets the history (DVR was disarmed or the stream restarted).
    pub fn reset(&mut self) {
        self.last_idr = None;
        self.requested = None;
    }
}
