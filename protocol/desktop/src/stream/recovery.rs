//! Which failures reconnect automatically. The retry budget is nightjar-policy's [`Backoff`] on the
//! session clock: it outlives each stream, so a stream's own start cannot be its time base.

use nightjar_core::session::{self, Stage};
pub use nightjar_policy::recovery::{transient_termination, Backoff};

pub fn now_ms() -> u64 {
    session::now_us() / 1000
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
    use nightjar_policy::recovery::{STABLE_MS, WINDOW_MS};

    #[test]
    fn retries_are_bounded_across_short_connections() {
        let mut recovery = Backoff::default();
        let mut now = 5_000;
        for (attempt, seconds) in [1, 2, 4, 8, 12].into_iter().enumerate() {
            assert_eq!(recovery.next_delay(now), Some(seconds * 1000));
            assert_eq!(recovery.attempts() as usize, attempt + 1);
            now += (seconds + 1) * 1000;
            recovery.reset_after(1000);
        }
        assert_eq!(recovery.next_delay(now), None);
        recovery.reset_after(STABLE_MS);
        assert_eq!(recovery.next_delay(now), Some(1000));
        assert_eq!(recovery.next_delay(now + WINDOW_MS), None);
        // The window starts at the drop, before the first attempt.
        let mut dropped = Backoff::default();
        dropped.begin(now);
        assert_eq!(dropped.remaining(now + WINDOW_MS), 0);
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
