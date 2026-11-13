//! Application queue limits, separate from codec capacity and texture ownership.
//!
//! The AMF learning rule follows Ramazan Kara's native AMF work in Vibepollo
//! PR #461 (GPL-3.0): https://github.com/Nonary/Vibepollo/pull/461.
//! This Rust adaptation keeps the sender's budget separate from encoder depth.

use std::time::{Duration, Instant};

/// AMF's existing blocking QueryOutput allowance.
pub const QUERY_TIMEOUT: Duration = Duration::from_millis(1);

/// Whether a poll that can block for [`QUERY_TIMEOUT`] still returns by
/// `deadline`; closer to it, the poll would overshoot the claim it precedes.
pub fn query_fits(now: Instant, deadline: Instant) -> bool {
    deadline.saturating_duration_since(now) >= QUERY_TIMEOUT
}

/// `amd_output_prediction`: a query is skipped while the oldest frame in
/// flight is predicted to finish more than this far ahead, so a skip never
/// trades a frame that is done for one blocking query.
pub const PREDICTION_SKIP_FROM: Duration = Duration::from_micros(1200);
/// The next query is due this long before the predicted finish.
pub const PREDICTION_LEAD: Duration = Duration::from_micros(200);

/// The earliest AMF can return the oldest frame in flight: when it accepted
/// that frame plus the fastest encode measured so far, which no frame beats.
/// Measured from acceptance to the query that returned it, so it errs late
/// by up to one poll; [`PREDICTION_LEAD`] covers that.
pub fn predicted_output(accepted: Instant, fastest: Option<Duration>) -> Option<Instant> {
    fastest.map(|fastest| accepted + fastest)
}

/// Whether a query now would only wait in the driver for a frame that cannot
/// be done yet.
pub fn skips_query(now: Instant, predicted: Option<Instant>) -> bool {
    predicted.is_some_and(|at| at.saturating_duration_since(now) > PREDICTION_SKIP_FROM)
}

/// When the next query is worth making for a predicted finish.
pub fn query_due(predicted: Instant) -> Instant {
    predicted.checked_sub(PREDICTION_LEAD).unwrap_or(predicted)
}

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
mod tests;
