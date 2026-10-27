//! The performance overlay: the Compact line's health mark and when to ask the host for its
//! hardware readings.
//!
//! Android's `PerformanceOverlay.health` and `healthGlyph`, `NvConnection.pollHardwareMetrics`
//! and `Game.receiveHardwareMetrics`.

/// The stream's health as the Compact line's mark shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    /// Nothing measured yet.
    Unknown,
    Good,
    Fair,
    Poor,
}

impl Health {
    /// A dot while the stream is fine, a triangle or a cross when not, so the shape says it too.
    pub fn glyph(self) -> &'static str {
        match self {
            Health::Fair => "▲",
            Health::Poor => "✕",
            Health::Unknown | Health::Good => "●",
        }
    }
}

fn known(value: Option<f32>) -> Option<f32> {
    value.filter(|v| v.is_finite() && *v >= 0.0)
}

/// Poor for loss of 5 % or more, a round trip of 80 ms or more, or a quarter of the received
/// frames not shown; Fair from 1 % loss, 40 ms, a tenth of the frames not shown, or decoding
/// slower than a received frame; Good otherwise. Unknown until the frame rate, the round trip or
/// the loss is measured. `shown_fps` is the rate the screen confirmed, else the rendered one.
pub fn health(
    shown_fps: Option<f32>,
    received_fps: Option<f32>,
    network_ms: Option<f32>,
    loss_percent: Option<f32>,
    decode_ms: Option<f32>,
) -> Health {
    let (shown, received, network, loss, decode) = (
        known(shown_fps),
        known(received_fps).filter(|fps| *fps > 0.0),
        known(network_ms),
        known(loss_percent),
        known(decode_ms),
    );
    if shown.is_none() && network.is_none() && loss.is_none() {
        return Health::Unknown;
    }
    let frames = shown.zip(received);
    if loss.is_some_and(|l| l >= 5.0)
        || network.is_some_and(|n| n >= 80.0)
        || frames.is_some_and(|(s, r)| s < r * 0.75)
    {
        return Health::Poor;
    }
    if loss.is_some_and(|l| l >= 1.0)
        || network.is_some_and(|n| n >= 40.0)
        || frames.is_some_and(|(s, r)| s < r * 0.9)
        || decode.zip(received).is_some_and(|(d, r)| d > 1000.0 / r)
    {
        return Health::Fair;
    }
    Health::Good
}

/// When to ask the host for its hardware readings (`/api/hardware-metrics`) and how long the
/// last answer stays on screen. Polls only while the overlay shows them (Statistics or Debug):
/// the first request at once, then after the host's sample interval but at least 2 s, or 10 s
/// after a failed one. A host that does not offer the readings (404, 403, 409 or another
/// schema version) is not asked again this stream. A reading shows until twice the host's
/// interval, at least 5 s, after its sample time first arrived, so a host that stops sampling
/// does not leave old values up.
#[derive(Clone, Copy, Debug, Default)]
pub struct HardwarePoll {
    generation: u32,
    active: bool,
    in_flight: bool,
    unavailable: bool,
    next_ms: u64,
    sampled_at: Option<i64>,
    fresh_until_ms: u64,
}

/// A host answer for [`HardwarePoll::answered`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HardwareAnswer {
    /// A sample: its time (Unix ms) and the host's sample interval.
    Sample {
        sampled_at_unix_ms: i64,
        interval_ms: u32,
    },
    /// The request failed; the next one may work.
    Failed,
    /// The host does not offer the readings.
    Unavailable,
}

impl HardwarePoll {
    pub const MIN_INTERVAL_MS: u64 = 2_000;
    pub const RETRY_MS: u64 = 10_000;
    pub const MIN_FRESH_MS: u64 = 5_000;

    /// The request to send now, as its generation, if one is due. `wanted`: the overlay shows
    /// the readings and the stream is not input-only. Not wanted stops the poll and drops the
    /// reading; a later want starts again at once.
    pub fn due(&mut self, now_ms: u64, wanted: bool) -> Option<u32> {
        if !wanted || self.unavailable {
            self.stop();
            return None;
        }
        if !self.active {
            self.active = true;
            self.next_ms = now_ms;
        }
        if self.in_flight || now_ms < self.next_ms {
            return None;
        }
        self.in_flight = true;
        Some(self.generation)
    }

    /// Drops the reading; answers to requests already sent are ignored.
    pub fn stop(&mut self) {
        if self.active {
            self.generation = self.generation.wrapping_add(1);
        }
        self.active = false;
        self.in_flight = false;
        self.sampled_at = None;
        self.fresh_until_ms = 0;
    }

    /// The answer to the request of `generation`. Returns whether to show its sample; what
    /// shows stays only while [`fresh`](Self::fresh).
    pub fn answered(&mut self, generation: u32, now_ms: u64, answer: HardwareAnswer) -> bool {
        if !self.active || generation != self.generation {
            return false;
        }
        self.in_flight = false;
        match answer {
            HardwareAnswer::Sample {
                sampled_at_unix_ms,
                interval_ms,
            } => {
                let interval = u64::from(interval_ms);
                self.next_ms = now_ms + interval.max(Self::MIN_INTERVAL_MS);
                if self.sampled_at != Some(sampled_at_unix_ms) {
                    self.sampled_at = Some(sampled_at_unix_ms);
                    self.fresh_until_ms = now_ms + (2 * interval).max(Self::MIN_FRESH_MS);
                }
                now_ms < self.fresh_until_ms
            }
            HardwareAnswer::Failed => {
                self.next_ms = now_ms + Self::RETRY_MS;
                false
            }
            HardwareAnswer::Unavailable => {
                self.unavailable = true;
                self.stop();
                false
            }
        }
    }

    /// Whether the last sample may still show.
    pub fn fresh(&self, now_ms: u64) -> bool {
        self.active && now_ms < self.fresh_until_ms
    }

    /// The host does not offer the readings to this stream.
    pub fn unavailable(&self) -> bool {
        self.unavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mark_follows_android_thresholds() {
        assert_eq!(health(None, None, None, None, None), Health::Unknown);
        assert_eq!(
            health(Some(f32::NAN), Some(60.0), Some(-1.0), None, Some(2.0)),
            Health::Unknown
        );
        assert_eq!(
            health(Some(60.0), Some(60.0), Some(8.0), Some(0.1), Some(2.0)),
            Health::Good
        );
        // Loss, round trip and frames not shown, each at its Fair and Poor edge.
        assert_eq!(health(None, None, None, Some(1.0), None), Health::Fair);
        assert_eq!(health(None, None, None, Some(5.0), None), Health::Poor);
        assert_eq!(health(None, None, Some(40.0), None, None), Health::Fair);
        assert_eq!(health(None, None, Some(80.0), None, None), Health::Poor);
        assert_eq!(
            health(Some(53.0), Some(60.0), None, None, None),
            Health::Fair
        );
        assert_eq!(
            health(Some(44.0), Some(60.0), None, None, None),
            Health::Poor
        );
        assert_eq!(
            health(Some(55.0), Some(60.0), None, None, None),
            Health::Good
        );
        // Decoding slower than a received frame is Fair; without a received rate it is not judged.
        assert_eq!(
            health(Some(60.0), Some(60.0), None, None, Some(17.0)),
            Health::Fair
        );
        assert_eq!(
            health(Some(60.0), None, None, None, Some(17.0)),
            Health::Good
        );
        assert_eq!(
            health(Some(60.0), Some(0.0), None, None, Some(17.0)),
            Health::Good
        );
        assert_eq!(
            [Health::Unknown, Health::Good, Health::Fair, Health::Poor].map(Health::glyph),
            ["●", "●", "▲", "✕"]
        );
    }

    #[test]
    fn the_poll_runs_at_the_hosts_interval_while_wanted() {
        let mut poll = HardwarePoll::default();
        assert_eq!(poll.due(0, false), None);
        let first = poll
            .due(1_000, true)
            .expect("the first request goes at once");
        assert_eq!(poll.due(1_500, true), None, "one request at a time");
        let sample = |at| HardwareAnswer::Sample {
            sampled_at_unix_ms: at,
            interval_ms: 1_000,
        };
        assert!(poll.answered(first, 1_200, sample(100)));
        assert!(
            poll.fresh(6_100) && !poll.fresh(6_200),
            "fresh for 5 s, twice 1 s being less"
        );
        assert_eq!(poll.due(3_100, true), None, "at least 2 s apart");
        let second = poll.due(3_200, true).unwrap();
        // The same sample again does not extend it.
        assert!(!poll.answered(second, 6_300, sample(100)));
        assert!(!poll.fresh(6_300));
        let third = poll.due(8_300, true).unwrap();
        assert!(poll.answered(third, 8_300, sample(200)));
        // A failed request waits 10 s and drops nothing more than the sample's own freshness.
        let fourth = poll.due(10_300, true).unwrap();
        assert!(!poll.answered(fourth, 10_300, HardwareAnswer::Failed));
        assert!(poll.fresh(13_200));
        assert_eq!(poll.due(20_299, true), None);
        assert!(poll.due(20_300, true).is_some());
    }

    #[test]
    fn stopping_drops_the_reading_and_late_answers() {
        let mut poll = HardwarePoll::default();
        let sent = poll.due(0, true).unwrap();
        assert_eq!(poll.due(100, false), None);
        let long = HardwareAnswer::Sample {
            sampled_at_unix_ms: 1,
            interval_ms: 10_000,
        };
        assert!(
            !poll.answered(sent, 200, long),
            "an answer after the stop is ignored"
        );
        assert!(!poll.fresh(200));
        let again = poll.due(300, true).expect("wanted again: at once");
        assert_ne!(again, sent);
        assert!(poll.answered(again, 400, long));
        assert!(
            poll.fresh(20_399) && !poll.fresh(20_400),
            "twice a 10 s interval"
        );
        poll.stop();
        assert!(!poll.fresh(500));
        // Wanted again, the host's unchanged sample shows again.
        let after = poll.due(600, true).unwrap();
        assert!(poll.answered(after, 700, long));
    }

    #[test]
    fn a_host_without_readings_is_not_asked_again() {
        let mut poll = HardwarePoll::default();
        let sent = poll.due(0, true).unwrap();
        assert!(!poll.answered(sent, 100, HardwareAnswer::Unavailable));
        assert!(poll.unavailable());
        assert_eq!(poll.due(60_000, true), None);
        assert!(!poll.fresh(60_000));
    }
}
