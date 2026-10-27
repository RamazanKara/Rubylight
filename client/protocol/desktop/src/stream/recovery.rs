use std::time::{Duration, Instant};

use nightjar_core::session::Stage;

pub const WINDOW: Duration = Duration::from_secs(60);
const DELAYS: [u64; 5] = [1, 2, 4, 8, 12];

#[derive(Clone, Default)]
pub struct Recovery {
    pub attempts: usize,
    started: Option<Instant>,
}

impl Recovery {
    pub fn begin(&mut self, now: Instant) {
        self.started.get_or_insert(now);
    }

    pub fn reset_after(&mut self, connected: Duration) {
        if connected >= Duration::from_secs(30) {
            *self = Self::default();
        }
    }

    pub fn remaining(&self, now: Instant) -> Duration {
        self.started.map_or(WINDOW, |started| {
            WINDOW.saturating_sub(now.duration_since(started))
        })
    }

    pub fn next_delay(&mut self, now: Instant) -> Option<Duration> {
        let delay = Duration::from_secs(*DELAYS.get(self.attempts)?);
        self.started.get_or_insert(now);
        if delay >= self.remaining(now) {
            return None;
        }
        self.attempts += 1;
        Some(delay)
    }
}

pub fn transient_termination(code: i32) -> bool {
    matches!(code, -102..=-100 | -1 | 32 | 54 | 57 | 60 | 64 | 100..=104 | 107 | 110..=113
        | 10050..=10054 | 10057 | 10060 | 10061 | 10064 | 10065)
}

pub fn transient_start(stage: Stage, code: i32) -> bool {
    matches!(
        stage,
        Stage::NameResolution
            | Stage::RtspHandshake
            | Stage::ControlStreamInit
            | Stage::ControlStreamStart
            | Stage::InputStreamStart
    ) && transient_termination(code)
}

pub fn transient_http(error: &nightjar_core::http::Error) -> bool {
    use std::io::ErrorKind;
    matches!(error, nightjar_core::http::Error::Io(e) if matches!(e.kind(),
        ErrorKind::TimedOut | ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted
        | ErrorKind::ConnectionRefused | ErrorKind::NotConnected | ErrorKind::BrokenPipe
        | ErrorKind::UnexpectedEof | ErrorKind::NetworkDown | ErrorKind::NetworkUnreachable
        | ErrorKind::HostUnreachable))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_are_bounded_across_short_connections() {
        let start = Instant::now();
        let mut recovery = Recovery::default();
        let mut now = start;
        for (attempt, seconds) in DELAYS.into_iter().enumerate() {
            assert_eq!(recovery.next_delay(now), Some(Duration::from_secs(seconds)));
            assert_eq!(recovery.attempts, attempt + 1);
            now += Duration::from_secs(seconds + 1);
            recovery.reset_after(Duration::from_secs(1));
        }
        assert_eq!(recovery.next_delay(now), None);
        recovery.reset_after(Duration::from_secs(30));
        assert_eq!(recovery.next_delay(now), Some(Duration::from_secs(1)));
        assert_eq!(recovery.next_delay(now + WINDOW), None);
    }

    #[test]
    fn only_transport_failures_retry() {
        for code in [0, -103, -104, 401, 403, 500, -999] {
            assert!(!transient_termination(code));
        }
        assert!(transient_termination(-100));
        assert!(transient_start(Stage::ControlStreamStart, 10060));
        assert!(!transient_start(Stage::VideoStreamInit, -1));
        assert!(!transient_start(Stage::RtspHandshake, 403));
        assert!(transient_http(
            &std::io::Error::from(std::io::ErrorKind::TimedOut).into()
        ));
        assert!(!transient_http(
            &nightjar_core::http::Error::CertificateMismatch
        ));
        assert!(!transient_http(&nightjar_core::http::Error::NotPaired));
    }
}
