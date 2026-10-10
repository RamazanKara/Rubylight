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
}

impl PhaseSync {
    /// Without a report for this long, the client is no longer locked.
    pub const TIMEOUT: Duration = Duration::from_secs(3);
    /// How often the applied interval is logged.
    const LOG_PERIOD: Duration = Duration::from_secs(10);

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
        if self.last_report.is_some_and(|at| now.saturating_duration_since(at) >= Self::TIMEOUT) {
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
        Report { frames: 60, period_ns, lead_ns, spread_ns: 200_000 }.encode().to_vec()
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
        assert_eq!(sync.interval(now, nominal), Some(Duration::from_nanos(8_334_000)));
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
