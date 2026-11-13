//! Phase lock with a client: frames are captured at the client's exact refresh
//! period and at a phase that gets them there just before its display latch.
//!
//! The client sends a [`Report`] about every 500 ms (control message 0x5530).
//! [`PhaseSync`] keeps the [`PhaseLock`] from `rubylight-protocol` and lets it
//! go when the reports stop, so a client that stops reporting falls back to the
//! host's own stream period.
use rubylight_protocol::phase_lock::{PhaseLock, Report};
use std::time::{Duration, Instant};

pub use rubylight_protocol::phase_lock::REPORT_MESSAGE_TYPE;

#[derive(Default)]
pub struct PhaseSync {
    lock: PhaseLock,
    last_report: Option<Instant>,
    logged_at: Option<Instant>,
    /// Source pictures per stream frame while frames move a picture at a time
    /// ([`Self::set_source`]).
    pictures: Option<i64>,
    /// Frames in a row whose source disagreed with `pictures`.
    votes: u32,
    /// The source presents at the stream rate or slower ([`Self::set_source`]), so a frame
    /// the lock holds can only age.
    at_rate: bool,
    /// Frames in a row whose source disagreed with `at_rate`.
    at_rate_votes: u32,
    /// The lock holds frames of such a source anyway (`phase_lock_hold_at_stream_rate`).
    hold_at_rate: bool,
    /// The last [`Self::standing_aside`] state reported, for logging a change once.
    aside_reported: Option<bool>,
    /// [`Self::interval`] stood aside on its last call.
    aside: bool,
}

impl PhaseSync {
    /// Without a report for this long, the client is no longer locked.
    pub const TIMEOUT: Duration = Duration::from_secs(3);
    /// How often the applied interval is logged.
    const LOG_PERIOD: Duration = Duration::from_secs(10);
    /// Frames in a row a changed source takes to change how frames move.
    const SOURCE_FRAMES: u32 = 30;
    /// Frames in a row a source that left the stream rate takes to bring the lock back, so a
    /// game or desktop near the threshold does not restart the lock again and again.
    const LEAVE_RATE_FRAMES: u32 = 4 * Self::SOURCE_FRAMES;

    /// Applies a report payload. Returns the report, or `None` when it is malformed.
    pub fn on_payload(&mut self, now: Instant, payload: &[u8]) -> Option<Report> {
        let report = Report::decode(payload)?;
        self.expire(now);
        self.lock.on_report(&report);
        self.last_report = Some(now);
        Some(report)
    }

    /// True while reports keep arriving.
    pub fn locked(&mut self, now: Instant) -> bool {
        self.expire(now);
        self.lock.locked()
    }

    /// The source's median (`picture`) and mean (`mean`) interval between pictures while the
    /// host claims frames as the source presents them (arrival pacing), `None` while it
    /// captures on its own schedule. Call once per frame, before [`Self::interval`].
    ///
    /// The arrival pacer claims a frame at least 3/4 of a stream interval (`nominal`) after
    /// the last, so with 2 or 3 pictures per stream frame (a 120 fps game at 60 fps) the host
    /// never takes the picture before the one it takes: frames move a picture later or not at
    /// all ([`PhaseLock::set_step`]). Anything else, such as 144 fps content at 60 fps, moves
    /// by any amount. A changed source takes [`Self::SOURCE_FRAMES`] frames in a row to
    /// change that, so a median between two picture intervals does not flip it.
    pub fn set_source(
        &mut self,
        picture: Option<Duration>,
        mean: Option<Duration>,
        nominal: Duration,
    ) {
        let ns = |duration: Duration| i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX);
        let nominal = ns(nominal);
        let fits = match (picture.map(ns), mean.map(ns)) {
            (Some(picture), Some(mean))
                if picture > 0 && (mean - picture).abs() <= picture / 16 =>
            {
                let pictures = nominal.saturating_add(picture / 2) / picture;
                ((2..=3).contains(&pictures)
                    && (pictures * picture - nominal).abs() <= nominal / 32)
                    .then_some(pictures)
            }
            _ => None,
        };
        if fits == self.pictures {
            self.votes = 0;
        } else {
            self.votes += 1;
            if self.votes >= Self::SOURCE_FRAMES {
                self.pictures = fits;
                self.votes = 0;
            }
        }
        self.lock
            .set_step(self.pictures.map_or(0, |pictures| nominal / pictures));
        // A mean within 1/64 of a stream interval or longer: no more pictures than frames, so
        // the host has no newer picture to wait for, as with a game capped at the stream rate
        // (its mean reads the cap to a few ppm) or slower. Not the median: the capture reads a
        // capped game's median fast (8.0 ms at 120 fps, or 13.9 ms for 60 fps on a 144 Hz
        // desktop, which delivers its pictures 13.9 and 20.8 ms apart), and reads a 125-137 fps
        // game at 120 fps, whose spare pictures let the lock take a newer one, as slow as that.
        let at_rate = match (picture.map(ns), mean.map(ns)) {
            (Some(picture), Some(mean)) => picture > 0 && mean * 64 >= nominal * 63,
            _ => false,
        };
        if at_rate == self.at_rate {
            self.at_rate_votes = 0;
        } else {
            self.at_rate_votes += 1;
            let needed = if at_rate {
                Self::SOURCE_FRAMES
            } else {
                Self::LEAVE_RATE_FRAMES
            };
            if self.at_rate_votes >= needed {
                self.at_rate = at_rate;
                self.at_rate_votes = 0;
            }
        }
    }

    /// Keep holding frames of a source at the stream rate (`phase_lock_hold_at_stream_rate`),
    /// as the lock did before it stood aside for them.
    pub fn set_hold_at_rate(&mut self, hold: bool) {
        self.hold_at_rate = hold;
    }

    /// True while the lock stands aside for a source at the stream rate or slower
    /// ([`Self::interval`]).
    pub fn standing_aside(&self) -> bool {
        self.at_rate && !self.hold_at_rate && self.lock.locked()
    }

    /// The new [`Self::standing_aside`] state when it changed since the last call, for logging.
    pub fn aside_changed(&mut self) -> Option<bool> {
        let aside = self.standing_aside();
        (self.aside_reported != Some(aside)).then(|| {
            self.aside_reported = Some(aside);
            aside
        })
    }

    /// Interval from the frame captured now to the next one while locked: a whole number
    /// of client refreshes with the phase correction. `None` without a lock, so a client
    /// that never reports (stock Moonlight, older Rubylight) keeps the host's pacing
    /// untouched, and `None` when the stream rate is not a whole number of client
    /// refreshes (120 fps to a 60 Hz phone, 90 fps to 120 Hz), so the rate the user chose
    /// stays. Call once per frame.
    ///
    /// `None` as well while frames are claimed as they arrive from a source at the stream rate
    /// or slower: the host cannot take such a frame earlier, only hold it, and the client's
    /// refresh drifts against the game's clock, so the hold grew until the game skipped a
    /// frame. A Shield TV streaming a game capped at 60 fps held frames up to 12 ms, and the
    /// game's present to the TV's render took 76-79 ms at the median while it held against
    /// 68 ms while it didn't, with half the display gaps (0.03-0.18% of frames against
    /// 0.07-0.33%). Claimed as it arrives, such a frame waits at the client instead, for the
    /// same refresh at best.
    ///
    /// The lock keeps taking reports while it stands aside, and its correction follows a
    /// phase error it no longer steers (to its clamp, a 1% rate error, within seconds): when
    /// it steers again it starts over, locking on the next report.
    pub fn interval(&mut self, now: Instant, nominal: Duration) -> Option<Duration> {
        self.expire(now);
        let aside = self.standing_aside();
        if std::mem::replace(&mut self.aside, aside) && !aside {
            self.lock.reset();
        }
        if aside {
            return None;
        }
        let client_ns = self.lock.period_ns().filter(|&period| period > 0)?;
        let nominal_ns = i64::try_from(nominal.as_nanos()).unwrap_or(i64::MAX);
        // Client refreshes per stream frame: 2 for 60 fps on a 120 Hz phone.
        let refreshes = nominal_ns.saturating_add(client_ns / 2) / client_ns;
        if refreshes < 1 || (refreshes * client_ns - nominal_ns).abs() > nominal_ns / 50 {
            return None;
        }
        let interval = self.lock.next_interval_ns(nominal_ns) + (refreshes - 1) * client_ns;
        // Never less than half or more than twice the nominal period.
        let interval = interval.clamp(nominal_ns / 2, nominal_ns.saturating_mul(2));
        Some(Duration::from_nanos(interval.max(1) as u64))
    }

    /// Lets the lock go at once, as when the stream's rate changes; the client's
    /// next reports lock it again.
    pub fn reset(&mut self) {
        self.lock.reset();
        self.last_report = None;
        self.logged_at = None;
        self.pictures = None;
        self.votes = 0;
        self.at_rate = false;
        self.at_rate_votes = 0;
        self.aside_reported = None;
        self.aside = false;
    }

    /// True while the client's reports keep arriving, locked or about to lock again.
    pub fn reporting(&mut self, now: Instant) -> bool {
        self.expire(now);
        self.last_report.is_some()
    }

    /// True about once every ten seconds while locked, for logging the applied interval.
    pub fn log_due(&mut self, now: Instant) -> bool {
        self.expire(now);
        if !self.lock.locked() || self.logged_at.is_some_and(|at| now < at + Self::LOG_PERIOD) {
            return false;
        }
        self.logged_at = Some(now);
        true
    }

    fn expire(&mut self, now: Instant) {
        if self
            .last_report
            .is_some_and(|at| now.saturating_duration_since(at) >= Self::TIMEOUT)
        {
            self.lock.reset();
            self.last_report = None;
            self.logged_at = None;
        }
    }
}

#[cfg(test)]
mod tests;
