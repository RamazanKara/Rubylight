//! The rate a game presents at, from the host's frame timestamps.
//!
//! The Android client votes a frame rate for the surface (`Surface.setFrameRate`) so the
//! panel can pick a mode that is a multiple of the content. The vote must follow the game
//! (30, 60, 90 fps) but not its hitches, and must not flap, because a mode switch can blank the
//! screen. [`ContentRate`] takes the median interval of the last second, leaves out an isolated
//! long gap, and reports a rate only after it held within 3 % for two seconds; a reported rate
//! changes only when a new one has held that long. A reported rate that the game has left for
//! two seconds, without settling on another, is withdrawn: the vote it fed should not outlive it.

/// Intervals kept; at 240 fps a second of them fits.
const RING: usize = 256;
/// The span of host time the median covers, in microseconds.
const WINDOW_US: i64 = 1_000_000;
/// Intervals needed before a median means anything.
const MIN_SAMPLES: usize = 8;
/// How long a rate must hold before it is reported, in microseconds.
const STABLE_US: i64 = 2_000_000;
/// How far a rate may move, in thousandths, and still be the same rate.
const SAME_PERMILLE: i64 = 30;
/// An interval this many times the median is a hitch, not a new rate.
const HITCH_FACTOR: i64 = 3;
/// Consecutive hitches that are a new rate after all (the game dropped to 30 fps).
const HITCHES_FOR_NEW_RATE: u8 = 4;
/// A gap this long is a pause or a restarted stream: start over.
const PAUSE_US: i64 = 1_000_000;

#[derive(Clone, Debug)]
pub struct ContentRate {
    last_host_us: Option<i64>,
    /// (host time of the later frame, interval), newest at `next - 1`.
    ring: [(i64, i64); RING],
    len: usize,
    next: usize,
    hitches: u8,
    /// The rate being timed (millihertz) and the host time it first appeared.
    candidate: Option<(i64, i64)>,
    reported: Option<u32>,
    /// Host time the median first left the reported rate; none while it is back inside.
    outside_since: Option<i64>,
}

impl Default for ContentRate {
    fn default() -> Self {
        Self::new()
    }
}

fn same_rate(a: i64, b: i64) -> bool {
    (a - b).abs() * 1000 <= a.max(b) * SAME_PERMILLE
}

impl ContentRate {
    pub const fn new() -> Self {
        Self {
            last_host_us: None,
            ring: [(0, 0); RING],
            len: 0,
            next: 0,
            hitches: 0,
            candidate: None,
            reported: None,
            outside_since: None,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// The reported rate in millihertz (60 000 for 60 fps), or `None` before one held for
    /// two seconds, and again once the game has been away from it for two seconds.
    pub fn rate_mhz(&self) -> Option<u32> {
        self.reported
    }

    fn clear_window(&mut self) {
        self.len = 0;
        self.next = 0;
        self.candidate = None;
    }

    /// The median interval of the last second of host time ending at `now_us`.
    fn median_interval(&self, now_us: i64) -> Option<(i64, usize)> {
        let mut values = [0i64; RING];
        let mut count = 0;
        for &(host, interval) in &self.ring[..self.len] {
            if host > now_us - WINDOW_US {
                values[count] = interval;
                count += 1;
            }
        }
        if count < MIN_SAMPLES {
            return None;
        }
        let (_, median, _) = values[..count].select_nth_unstable(count / 2);
        Some((*median, count))
    }

    /// A frame with host time `host_us` (microseconds, unwrapped, in presentation order).
    pub fn observe(&mut self, host_us: i64) {
        let Some(last) = self.last_host_us else {
            self.last_host_us = Some(host_us);
            return;
        };
        if host_us <= last {
            return;
        }
        self.last_host_us = Some(host_us);
        let interval = host_us - last;
        if interval >= PAUSE_US {
            self.clear_window();
            self.hitches = 0;
            return;
        }
        if let Some((median, _)) = self.median_interval(last) {
            if interval > median * HITCH_FACTOR {
                self.hitches += 1;
                if self.hitches < HITCHES_FOR_NEW_RATE {
                    return;
                }
                // The game moved to a slower rate: forget the old one.
                self.clear_window();
            }
        }
        self.hitches = 0;
        self.ring[self.next] = (host_us, interval);
        self.next = (self.next + 1) % RING;
        self.len = (self.len + 1).min(RING);
        let Some((median, _)) = self.median_interval(host_us) else {
            return;
        };
        let rate = 1_000_000_000 / median.max(1);
        let since = match self.candidate {
            Some((candidate, since)) if same_rate(candidate, rate) => since,
            _ => {
                self.candidate = Some((rate, host_us));
                host_us
            }
        };
        let Some(reported) = self.reported else {
            if host_us - since >= STABLE_US {
                self.reported = Some(rate.min(i64::from(u32::MAX)) as u32);
            }
            return;
        };
        if same_rate(i64::from(reported), rate) {
            self.outside_since = None;
        } else if host_us - since >= STABLE_US {
            self.reported = Some(rate.min(i64::from(u32::MAX)) as u32);
            self.outside_since = None;
        } else if host_us - *self.outside_since.get_or_insert(host_us) >= STABLE_US {
            // Unsteady (90 to 120 and back): no rate to vote.
            self.reported = None;
            self.outside_since = None;
            self.candidate = None;
        }
    }
}
