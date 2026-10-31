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
}

impl PhaseSync {
    /// Without a report for this long, the client is no longer locked.
    pub const TIMEOUT: Duration = Duration::from_secs(3);
    /// How often the applied interval is logged.
    const LOG_PERIOD: Duration = Duration::from_secs(10);
    /// Frames in a row a changed source takes to change how frames move.
    const SOURCE_FRAMES: u32 = 30;

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
    }

    /// Interval from the frame captured now to the next one while locked: a whole number
    /// of client refreshes with the phase correction. `None` without a lock, so a client
    /// that never reports (stock Moonlight, older Rubylight) keeps the host's pacing
    /// untouched, and `None` when the stream rate is not a whole number of client
    /// refreshes (120 fps to a 60 Hz phone, 90 fps to 120 Hz), so the rate the user chose
    /// stays. Call once per frame.
    pub fn interval(&mut self, now: Instant, nominal: Duration) -> Option<Duration> {
        self.expire(now);
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
mod tests {
    use super::*;

    fn report(period_ns: u32, lead_ns: i32) -> Vec<u8> {
        Report {
            frames: 60,
            period_ns,
            lead_ns,
            spread_ns: 200_000,
        }
        .encode()
        .to_vec()
    }

    #[test]
    fn nominal_until_the_first_report() {
        let now = Instant::now();
        let mut sync = PhaseSync::default();
        let nominal = Duration::from_nanos(8_333_333);
        assert!(!sync.locked(now));
        assert_eq!(sync.interval(now, nominal), None);
    }

    #[test]
    fn follows_the_client_period_and_lets_go_after_the_timeout() {
        let now = Instant::now();
        let mut sync = PhaseSync::default();
        let nominal = Duration::from_nanos(8_333_333);
        // A client whose 120 Hz is a little slower than the host's, at the target margin.
        let lead = PhaseLock::DEFAULT_MARGIN_NS as i32;
        assert!(sync.on_payload(now, &report(8_334_000, lead)).is_some());
        assert!(sync.locked(now));
        assert_eq!(
            sync.interval(now, nominal),
            Some(Duration::from_nanos(8_334_000))
        );
        // Still locked just before the timeout, released at it.
        let later = now + PhaseSync::TIMEOUT;
        assert!(sync.locked(later - Duration::from_millis(1)));
        assert!(!sync.locked(later));
        assert_eq!(sync.interval(later, nominal), None);
    }

    #[test]
    fn keeps_the_stream_rate_the_user_chose() {
        let now = Instant::now();
        let lead = PhaseLock::DEFAULT_MARGIN_NS as i32;
        // 60 fps to a 120 Hz phone: every second refresh, not 120 fps.
        let mut sync = PhaseSync::default();
        sync.on_payload(now, &report(8_334_000, lead));
        assert_eq!(
            sync.interval(now, Duration::from_nanos(16_666_667)),
            Some(Duration::from_nanos(16_668_000))
        );
        // 120 fps to a 60 Hz phone, and 90 fps to a 120 Hz phone: no lock.
        let mut sync = PhaseSync::default();
        sync.on_payload(now, &report(16_666_667, lead));
        assert_eq!(sync.interval(now, Duration::from_nanos(8_333_333)), None);
        let mut sync = PhaseSync::default();
        sync.on_payload(now, &report(8_333_333, lead));
        assert_eq!(sync.interval(now, Duration::from_nanos(11_111_111)), None);
    }

    #[test]
    fn a_lower_client_refresh_never_slows_the_stream() {
        // An LTPO panel resting at 60, 48, 30 or 24 Hz reports that rate, not its top
        // one. A 120 fps stream is no whole number of those refreshes: no lock, so the
        // host keeps 120 fps instead of dropping to 60, 40 or 20.
        let now = Instant::now();
        let lead = PhaseLock::DEFAULT_MARGIN_NS as i32;
        let nominal = Duration::from_nanos(8_333_333);
        for hz in [60, 48, 40, 30, 24] {
            let mut sync = PhaseSync::default();
            assert!(
                sync.on_payload(now, &report(1_000_000_000 / hz, lead))
                    .is_some()
            );
            assert_eq!(sync.interval(now, nominal), None, "{hz} Hz");
        }
        // Whatever the phase correction asks for, a locked interval stays within half
        // to twice the stream period: 60 to 240 fps for a 120 fps stream.
        for (period, stream) in [
            (8_334_000, 8_333_333u64),
            (4_166_667, 8_333_333),
            (8_334_000, 16_666_667),
        ] {
            for lead in [-(period as i32) / 2, 0, period as i32 / 2] {
                let mut sync = PhaseSync::default();
                sync.on_payload(now, &report(period, lead));
                for _ in 0..200 {
                    let interval = sync.interval(now, Duration::from_nanos(stream)).unwrap();
                    assert!(interval.as_nanos() as u64 >= stream / 2);
                    assert!(interval.as_nanos() as u64 <= stream * 2);
                }
            }
        }
    }

    const SIXTY: u64 = 16_666_667;
    const GAME: u64 = 8_333_333;

    /// The interval after a report with frames at the client's latch (below the margin),
    /// once the source has given `frames` frames of (median, mean) picture intervals in
    /// turn: longer when frames move a picture later, shorter when the lock asks for
    /// earlier ones.
    fn after_frames_at_the_latch(
        client_ns: u64,
        stream_ns: u64,
        pictures: &[(u64, u64)],
        frames: usize,
    ) -> Duration {
        let now = Instant::now();
        let nominal = Duration::from_nanos(stream_ns);
        let mut sync = PhaseSync::default();
        for frame in 0..frames {
            let (picture, mean) = pictures[frame % pictures.len()];
            let (picture, mean) = (Duration::from_nanos(picture), Duration::from_nanos(mean));
            sync.set_source(Some(picture), Some(mean), nominal);
        }
        sync.on_payload(now, &report(client_ns as u32, 0));
        sync.interval(now, nominal).unwrap()
    }

    #[test]
    fn a_game_at_twice_the_stream_rate_never_asks_for_earlier_frames() {
        // The host takes every second picture of a 120 fps game at 60 fps and cannot take
        // the one before, so frames below the margin go a picture later instead of asking
        // for shorter intervals that never bring them earlier: on a 60 Hz client, and a
        // 240 Hz source at 120 fps on a 120 Hz client. Every third picture of a 180 fps
        // source moves the same way.
        let sixty = Duration::from_nanos(SIXTY);
        assert!(after_frames_at_the_latch(SIXTY, SIXTY, &[(GAME, GAME)], 30) > sixty);
        let desktop = [(4_166_667, 4_166_667)];
        assert!(after_frames_at_the_latch(GAME, GAME, &desktop, 30) > Duration::from_nanos(GAME));
        assert!(after_frames_at_the_latch(SIXTY, SIXTY, &[(5_555_556, 5_555_556)], 30) > sixty);
        // Not before the source has held for 30 frames, nor on the host's own grid.
        assert!(after_frames_at_the_latch(SIXTY, SIXTY, &[(GAME, GAME)], 29) < sixty);
        let now = Instant::now();
        let mut sync = PhaseSync::default();
        sync.set_source(None, None, sixty);
        sync.on_payload(now, &report(SIXTY as u32, 0));
        assert!(sync.interval(now, sixty).unwrap() < sixty);
    }

    #[test]
    fn a_source_the_host_can_take_earlier_pictures_of_keeps_the_plain_lock() {
        // The pacer takes the picture before whenever it came at least 3/4 of a stream
        // interval after the last claim: 144 fps content at 60 fps (2.4 pictures a frame),
        // 120 fps content at 90 fps on a 90 Hz client, a 120 fps game at 120 fps on a 120 Hz
        // client (its median reads 8.0 ms), and 240 Hz content at 60 fps (4 pictures a
        // frame). The lock asks for earlier frames there.
        for (client_ns, stream_ns, picture) in [
            (SIXTY, SIXTY, 6_944_444),
            (11_111_111, 11_111_111, GAME),
            (GAME, GAME, 8_000_000),
            (SIXTY, SIXTY, 4_166_667),
        ] {
            let interval =
                after_frames_at_the_latch(client_ns, stream_ns, &[(picture, picture)], 300);
            assert!(
                interval < Duration::from_nanos(stream_ns),
                "{client_ns} {stream_ns} {picture}: {interval:?}"
            );
        }
    }

    #[test]
    fn a_median_between_two_picture_intervals_keeps_the_plain_lock() {
        // A source between 120 and 240 fps: the median flips between 8.33 and 4.17 ms while
        // the mean stays near 6.25 ms, so neither reading is the source's interval.
        let sixty = Duration::from_nanos(SIXTY);
        let flipping = [(GAME, 6_250_000), (4_166_667, 6_250_000)];
        assert!(after_frames_at_the_latch(SIXTY, SIXTY, &flipping, 300) < sixty);
        // A median that agrees with the mean only every other frame never holds 30 frames.
        let flipping = [(GAME, GAME), (4_166_667, GAME)];
        assert!(after_frames_at_the_latch(SIXTY, SIXTY, &flipping, 300) < sixty);
    }

    #[test]
    fn a_reset_forgets_the_source() {
        let now = Instant::now();
        let sixty = Duration::from_nanos(SIXTY);
        let game = Some(Duration::from_nanos(GAME));
        let mut sync = PhaseSync::default();
        for _ in 0..30 {
            sync.set_source(game, game, sixty);
        }
        sync.reset();
        sync.set_source(None, None, sixty);
        sync.on_payload(now, &report(SIXTY as u32, 0));
        assert!(sync.interval(now, sixty).unwrap() < sixty);
    }

    #[test]
    fn a_reset_lets_go_until_the_next_report() {
        let now = Instant::now();
        let mut sync = PhaseSync::default();
        let nominal = Duration::from_nanos(8_333_333);
        sync.on_payload(now, &report(8_334_000, 0));
        assert!(sync.locked(now));
        sync.reset();
        assert!(!sync.locked(now));
        assert_eq!(sync.interval(now, nominal), None);
        sync.on_payload(now, &report(8_334_000, 0));
        assert!(sync.locked(now));
    }

    #[test]
    fn rejects_malformed_payloads() {
        let now = Instant::now();
        let mut sync = PhaseSync::default();
        assert!(sync.on_payload(now, &[1, 0, 0]).is_none());
        let mut wrong_version = report(8_333_333, 0);
        wrong_version[0] = 99;
        assert!(sync.on_payload(now, &wrong_version).is_none());
        // A 120 Hz report claiming a slack beyond one refresh is implausible.
        let mut implausible = report(8_333_333, 0);
        implausible[8..12].copy_from_slice(&20_000_000i32.to_le_bytes());
        assert!(sync.on_payload(now, &implausible).is_none());
        assert!(!sync.locked(now));
        assert_eq!(sync.interval(now, Duration::from_nanos(8_333_333)), None);
    }

    #[test]
    fn logs_about_every_ten_seconds_while_locked() {
        let now = Instant::now();
        let mut sync = PhaseSync::default();
        assert!(!sync.log_due(now));
        sync.on_payload(now, &report(8_333_333, 0));
        assert!(sync.log_due(now));
        assert!(!sync.log_due(now + Duration::from_secs(1)));
        sync.on_payload(now + Duration::from_secs(9), &report(8_333_333, 0));
        assert!(sync.log_due(now + Duration::from_secs(10)));
    }
}
