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
    fn a_game_capped_at_the_stream_rate_is_claimed_as_it_arrives() {
        // A Shield TV at 60 Hz, a game capped at 60 fps by RTSS: the lock held its frames for
        // the client's refresh, up to 12 ms as the clocks drifted, and the game's present to
        // the TV's render took 8-11 ms longer at the median while it held.
        let now = Instant::now();
        let sixty = Duration::from_nanos(SIXTY);
        // Frames a refresh early at the client: the plain lock would hold them.
        let early = (SIXTY / 2) as i32;
        let capped = |frames: usize, hold: bool, picture: u64, mean: u64| {
            let mut sync = PhaseSync::default();
            sync.set_hold_at_rate(hold);
            let (picture, mean) = (Duration::from_nanos(picture), Duration::from_nanos(mean));
            for _ in 0..frames {
                sync.set_source(Some(picture), Some(mean), sixty);
            }
            sync.on_payload(now, &report(SIXTY as u32, early));
            sync
        };
        let mut sync = capped(30, false, SIXTY, SIXTY);
        assert!(sync.standing_aside());
        assert_eq!(sync.interval(now, sixty), None);
        assert_eq!(sync.aside_changed(), Some(true));
        assert_eq!(sync.aside_changed(), None);
        // A slower game too (50 fps at 60), one whose median the capture reads fast, and one
        // on a 144 Hz desktop (pictures 13.9 and 20.8 ms apart).
        for (picture, mean) in [
            (20_000_000, 20_000_000),
            (SIXTY * 15 / 16, SIXTY),
            (13_888_889, SIXTY),
        ] {
            assert_eq!(capped(30, false, picture, mean).interval(now, sixty), None);
        }
        // Not before the source has held for 30 frames, nor with the hold kept on purpose:
        // then the lock holds the early frames as before.
        assert!(
            capped(29, false, SIXTY, SIXTY)
                .interval(now, sixty)
                .unwrap()
                > sixty
        );
        let mut held = capped(30, true, SIXTY, SIXTY);
        assert!(!held.standing_aside());
        assert!(held.interval(now, sixty).unwrap() > sixty);
        // A faster source keeps the lock: the host takes a later picture, not an older one.
        // A 64 fps game at 60 fps as well, whose mean is faster than the stream's.
        assert!(capped(30, false, GAME, GAME).interval(now, sixty).is_some());
        let fast = SIXTY * 15 / 16;
        assert!(capped(30, false, fast, fast).interval(now, sixty).is_some());
        // Nor without a lock.
        let mut sync = PhaseSync::default();
        for _ in 0..30 {
            sync.set_source(Some(sixty), Some(sixty), sixty);
        }
        assert!(!sync.standing_aside());
    }

    #[test]
    fn the_lock_starts_over_when_it_steers_again() {
        // While it stands aside the lock keeps taking reports, and with frames a refresh
        // early that it no longer steers, its correction ran to its clamp within seconds:
        // back in charge, it would have moved frames through the client's refresh at about
        // 10 ms a second. It starts over instead, and only after the source has left the
        // stream rate for 120 frames, so a source near the threshold does not restart it on
        // every flicker.
        let mut now = Instant::now();
        let sixty = Duration::from_nanos(SIXTY);
        let mut sync = PhaseSync::default();
        for _ in 0..30 {
            sync.set_source(Some(sixty), Some(sixty), sixty);
        }
        for _ in 0..40 {
            sync.on_payload(now, &report(SIXTY as u32, (SIXTY / 2) as i32));
            assert_eq!(sync.interval(now, sixty), None);
            now += Duration::from_millis(500);
        }
        assert_eq!(sync.aside_changed(), Some(true));
        // 144 fps content: the host can take earlier pictures again.
        let content = Duration::from_nanos(6_944_444);
        for _ in 1..PhaseSync::LEAVE_RATE_FRAMES {
            sync.set_source(Some(content), Some(content), sixty);
            assert_eq!(sync.interval(now, sixty), None);
        }
        assert_eq!(sync.aside_changed(), None);
        sync.set_source(Some(content), Some(content), sixty);
        assert_eq!(sync.interval(now, sixty), None, "locking again");
        assert_eq!(sync.aside_changed(), Some(false));
        assert!(sync.reporting(now));
        // Frames at the margin: the client's own period, with no correction left over.
        let margin = PhaseLock::DEFAULT_MARGIN_NS as i32;
        sync.on_payload(now, &report(SIXTY as u32, margin));
        assert_eq!(sync.interval(now, sixty), Some(sixty));
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
        // 120 fps content at 90 fps on a 90 Hz client, and 240 Hz content at 60 fps (4
        // pictures a frame), and 125 fps content at 120 fps (its median and mean read 8.0 ms).
        // The lock asks for earlier frames there.
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
        // A 120 fps game capped at 120 fps (a median read at 8.0 ms, the mean at the cap) has
        // no earlier picture: its frames are claimed as they arrive, as early as they come.
        let now = Instant::now();
        let nominal = Duration::from_nanos(GAME);
        let mut sync = PhaseSync::default();
        for _ in 0..300 {
            sync.set_source(
                Some(Duration::from_nanos(8_000_000)),
                Some(nominal),
                nominal,
            );
        }
        sync.on_payload(now, &report(GAME as u32, 0));
        assert_eq!(sync.interval(now, nominal), None);
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
    fn a_game_with_second_compositions_moves_a_picture_at_a_time() {
        // WGC delivers each picture of a 120 fps game with a second composition 0.1 or
        // 1 ms later, so the mean of all captures reads about 4.2 ms; in some sessions
        // every fourth picture has a third 3.5 ms later, which splits it in two. The
        // pictures' own median and mean still read 8.3 ms and the host moves a picture
        // at a time.
        for third in [false, true] {
            let now = Instant::now();
            let sixty = Duration::from_nanos(SIXTY);
            let mut cadence = crate::capture_policy::Freshness::default();
            let mut sync = PhaseSync::default();
            for frame in 0..120u64 {
                let start = 1_000_000_000 + frame * GAME + frame * 7_919 % 400_000;
                cadence.observe(start, 100_000);
                cadence.observe(start + [100_000, 1_000_000][(frame % 2) as usize], 100_000);
                if third && frame % 4 == 1 {
                    cadence.observe(start + 3_500_000, 100_000);
                }
                let ns = |nanos: Option<u64>| nanos.map(Duration::from_nanos);
                if frame % 2 == 0 {
                    sync.set_source(
                        ns(cadence.picture_interval()),
                        ns(cadence.picture_mean_interval()),
                        sixty,
                    );
                }
            }
            assert!(
                cadence.mean_interval().unwrap() < 5_000_000,
                "third={third}"
            );
            assert_eq!(sync.pictures, Some(2), "third={third}");
            sync.on_payload(now, &report(SIXTY as u32, 0));
            assert!(sync.interval(now, sixty).unwrap() > sixty, "third={third}");
        }
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
