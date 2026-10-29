//! Which failures reconnect automatically. The retry budget is nightjar-policy's [`Backoff`] on the
//! session clock: it outlives each stream, so a stream's own start cannot be its time base.

use nightjar_core::session::{self, Stage};
pub use nightjar_policy::recovery::{
    reconnects_at_once, transient_termination, Backoff, RouteWait, PEER_TIMEOUT_MS,
    ROUTE_PROBE_TIMEOUT_MS, ROUTE_RETRY_MS,
};

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

/// A route check's immediate no-route error: no address toward the PC (yet). A check that
/// times out (a slow link, or a closed port, which Windows retries before it says so) or fails
/// otherwise is no reason to wait: the attempt itself decides, loudly.
pub fn route_missing(error: &std::io::Error) -> bool {
    use std::io::ErrorKind;
    matches!(error.kind(), ErrorKind::NetworkUnreachable | ErrorKind::HostUnreachable | ErrorKind::AddrNotAvailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nightjar_policy::recovery::{STABLE_MS, WINDOW_MS};

    #[test]
    fn only_an_immediate_no_route_error_waits_for_the_route() {
        use std::io::{Error, ErrorKind};
        for kind in [ErrorKind::NetworkUnreachable, ErrorKind::HostUnreachable, ErrorKind::AddrNotAvailable] {
            assert!(route_missing(&Error::from(kind)), "{kind:?}");
        }
        for kind in [ErrorKind::TimedOut, ErrorKind::ConnectionRefused, ErrorKind::ConnectionReset, ErrorKind::Other] {
            assert!(!route_missing(&Error::from(kind)), "{kind:?}");
        }
        // The OS's own codes map the same way: Windows' WSAENETUNREACH, WSAEHOSTUNREACH,
        // WSAEADDRNOTAVAIL and WSAETIMEDOUT, Linux's errnos.
        #[cfg(windows)]
        let (unreachable, timed_out) = ([10051, 10065, 10049], 10060);
        #[cfg(not(windows))]
        let (unreachable, timed_out) = ([101, 113, 99], 110);
        for code in unreachable {
            assert!(route_missing(&Error::from_raw_os_error(code)), "{code}");
        }
        assert!(!route_missing(&Error::from_raw_os_error(timed_out)));
    }

    // The reconnect decision assumes the engine's connection timeout (nightjar-policy cannot
    // depend on the engine; the desktop links both).
    #[test]
    fn the_peer_timeout_is_the_engines_connection_timeout() {
        assert_eq!(
            std::time::Duration::from_millis(PEER_TIMEOUT_MS.into()),
            nightjar_core::control::driver::CONTROL_PEER_TIMEOUT
        );
    }

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
