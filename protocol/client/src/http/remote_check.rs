//! The remote access check of Android's RemoteSetupActivity: five serverinfo probes over pinned
//! HTTPS only (no HTTP fallback), each verifying it reached the paired host, then the network
//! path when the host offers it. Latency, jitter and loss follow ConnectionTestStats.java.

use super::network_path::NetworkPath;
use super::{Error, HostApiError, HostClient, ServerInfo, PERMISSION_LAUNCH, PERMISSION_VIEW};
use std::time::{Duration, Instant};

pub const REMOTE_CHECK_PROBES: usize = 5;
const PROBE_INTERVAL: Duration = Duration::from_millis(150);
/// One probe, connect included, as Android's call timeout.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_PROBE_BYTES: usize = 64 * 1024;

/// Round-trip samples of a connection test. A failed probe counts as loss and breaks the jitter
/// sequence; nothing measured is `None`, never zero.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConnectionTestStats {
    attempts: u32,
    received: u32,
    differences: u32,
    total_ms: f64,
    total_difference_ms: f64,
    previous_ms: Option<f64>,
}

impl ConnectionTestStats {
    /// One probe's round trip; a negative or non-finite time is a failed probe.
    pub fn add(&mut self, milliseconds: f64) {
        self.attempts += 1;
        if !milliseconds.is_finite() || milliseconds < 0.0 {
            self.previous_ms = None;
            return;
        }
        self.received += 1;
        self.total_ms += milliseconds;
        if let Some(previous) = self.previous_ms {
            self.total_difference_ms += (milliseconds - previous).abs();
            self.differences += 1;
        }
        self.previous_ms = Some(milliseconds);
    }

    pub fn add_failure(&mut self) {
        self.add(f64::NAN);
    }

    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    /// The mean round trip of the probes that answered.
    pub fn latency_ms(&self) -> Option<f64> {
        (self.received > 0).then(|| self.total_ms / f64::from(self.received))
    }

    /// The mean difference between consecutive answered probes.
    pub fn jitter_ms(&self) -> Option<f64> {
        (self.differences > 0).then(|| self.total_difference_ms / f64::from(self.differences))
    }

    pub fn loss_percent(&self) -> Option<f64> {
        (self.attempts > 0)
            .then(|| f64::from(self.attempts - self.received) * 100.0 / f64::from(self.attempts))
    }
}

/// What the check found.
#[derive(Debug)]
pub enum RemoteOutcome {
    /// The paired host answered with view and launch permission. `path` is the host's route
    /// category, or why there is none ([`HostApiError::Unavailable`] for a host without it).
    Verified {
        server_info: Box<ServerInfo>,
        path: Result<NetworkPath, HostApiError>,
    },
    /// A host answered with another uniqueid.
    WrongHost,
    /// The host no longer lists this client as paired.
    PairRevoked,
    /// Paired, but without view and launch permission.
    PermissionDenied,
    /// No probe reached the paired host; the last probe's error.
    Unreachable(Error),
}

#[derive(Debug)]
pub struct RemoteCheck {
    pub outcome: RemoteOutcome,
    pub stats: ConnectionTestStats,
}

/// RemoteSetupActivity.verificationProblem: identity first, then pairing, then permission.
fn verification_problem(expected_unique_id: &str, info: &ServerInfo) -> Option<RemoteOutcome> {
    if info.unique_id != expected_unique_id {
        Some(RemoteOutcome::WrongHost)
    } else if !info.paired {
        Some(RemoteOutcome::PairRevoked)
    } else if info
        .permission
        .is_some_and(|p| p & (PERMISSION_VIEW | PERMISSION_LAUNCH) == 0)
    {
        Some(RemoteOutcome::PermissionDenied)
    } else {
        None
    }
}

/// The probe loop. `probe` answers serverinfo and its round trip in ms; a verification problem
/// ends the check at once, and a cancel ends it with [`Error::Cancelled`].
fn run(
    expected_unique_id: &str,
    mut probe: impl FnMut() -> Result<(ServerInfo, f64), Error>,
    mut pause: impl FnMut(),
    path: impl FnOnce(&ServerInfo) -> Result<NetworkPath, HostApiError>,
) -> Result<RemoteCheck, Error> {
    let mut stats = ConnectionTestStats::default();
    let mut verified = None;
    let mut last_error = None;
    for n in 0..REMOTE_CHECK_PROBES {
        if n > 0 {
            pause();
        }
        match probe() {
            Ok((info, milliseconds)) => {
                if let Some(outcome) = verification_problem(expected_unique_id, &info) {
                    stats.add_failure();
                    return Ok(RemoteCheck { outcome, stats });
                }
                stats.add(milliseconds);
                verified = Some(info);
            }
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(e) => {
                stats.add_failure();
                last_error = Some(e);
            }
        }
    }
    let outcome = match verified {
        Some(server_info) => RemoteOutcome::Verified {
            path: path(&server_info),
            server_info: Box::new(server_info),
        },
        None => RemoteOutcome::Unreachable(
            last_error.expect("every probe failed, so the last one left its error"),
        ),
    };
    Ok(RemoteCheck { outcome, stats })
}

impl HostClient {
    /// Checks that this address reaches the paired host whose serverinfo `uniqueid` is
    /// `expected_unique_id`: five pinned HTTPS probes 150 ms apart, then the network path.
    /// Blocks for up to about 20 s; [`HostClient::cancel_pending_requests`] ends it with
    /// [`Error::Cancelled`]. Needs the pinned certificate and the HTTPS port, and never
    /// discovers either.
    pub fn remote_check(&self, expected_unique_id: &str) -> Result<RemoteCheck, Error> {
        if expected_unique_id.is_empty() {
            return Err(Error::InvalidArgument(
                "The host's uniqueid is not known".into(),
            ));
        }
        self.pinned()?;
        run(
            expected_unique_id,
            || {
                let started = Instant::now();
                let response =
                    self.pinned_get("serverinfo", MAX_PROBE_BYTES, PROBE_TIMEOUT, false)?;
                let raw = String::from_utf8(response.body)
                    .map_err(|_| Error::Malformed("host reply is not UTF-8".into()))?;
                let info = ServerInfo::parse(&raw, self.http_port)?;
                Ok((info, started.elapsed().as_secs_f64() * 1000.0))
            },
            || std::thread::sleep(PROBE_INTERVAL),
            |info| self.network_path(&info.capabilities),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::serverinfo::tests::RUBYLIGHT;
    use crate::http::{Availability, Feature};
    use std::cell::Cell;

    fn close(actual: Option<f64>, expected: f64) -> bool {
        actual.is_some_and(|a| (a - expected).abs() < 0.001)
    }

    // The cases of Android's ConnectionTestStatsTest.
    #[test]
    fn computes_mean_latency_and_adjacent_variation() {
        let mut stats = ConnectionTestStats::default();
        for ms in [10.0, 20.0, 15.0] {
            stats.add(ms);
        }
        assert!(close(stats.latency_ms(), 15.0));
        assert!(close(stats.jitter_ms(), 7.5));
        assert_eq!(stats.loss_percent(), Some(0.0));
    }

    #[test]
    fn failed_probes_count_as_loss_and_break_the_jitter_sequence() {
        let mut stats = ConnectionTestStats::default();
        stats.add(10.0);
        stats.add_failure();
        stats.add(100.0);
        stats.add(110.0);
        assert_eq!(stats.loss_percent(), Some(25.0));
        assert_eq!(stats.jitter_ms(), Some(10.0));
        assert!(close(stats.latency_ms(), 220.0 / 3.0));
    }

    #[test]
    fn no_successful_samples_are_unavailable_not_zero_latency() {
        let mut stats = ConnectionTestStats::default();
        assert_eq!(
            (stats.latency_ms(), stats.jitter_ms(), stats.loss_percent()),
            (None, None, None)
        );
        stats.add(-1.0);
        stats.add(f64::INFINITY);
        assert_eq!(stats.loss_percent(), Some(100.0));
        assert_eq!(stats.latency_ms(), None);
        stats.add(0.0);
        assert_eq!(stats.latency_ms(), Some(0.0));
        assert_eq!(stats.jitter_ms(), None);
    }

    fn info(xml: &str) -> ServerInfo {
        ServerInfo::parse(xml, 47989).unwrap()
    }

    fn io() -> Error {
        Error::Io(std::io::ErrorKind::TimedOut.into())
    }

    /// Runs the loop over scripted probe answers; returns the check and how many probes ran.
    fn script(
        answers: Vec<Result<(ServerInfo, f64), Error>>,
        path: Result<NetworkPath, HostApiError>,
    ) -> (Result<RemoteCheck, Error>, usize) {
        let mut answers = answers.into_iter();
        let probes = Cell::new(0_usize);
        let pauses = Cell::new(0_usize);
        let check = run(
            "0123-ABCD",
            || {
                probes.set(probes.get() + 1);
                answers.next().expect("no more than five probes")
            },
            || pauses.set(pauses.get() + 1),
            |_| path,
        );
        assert_eq!(pauses.get(), probes.get().saturating_sub(1));
        (check, probes.get())
    }

    #[test]
    fn five_verified_probes_measure_and_read_the_path() {
        let answers = [10.0, 20.0, 15.0, 15.0, 20.0].map(|ms| Ok((info(RUBYLIGHT), ms)));
        let (check, probes) = script(answers.into(), Ok(NetworkPath::Relay));
        let check = check.unwrap();
        assert_eq!(probes, 5);
        assert!(matches!(
            &check.outcome,
            RemoteOutcome::Verified { server_info, path: Ok(NetworkPath::Relay) }
                if server_info.unique_id == "0123-ABCD"
        ));
        assert!(close(check.stats.latency_ms(), 16.0));
        assert!(close(check.stats.jitter_ms(), 5.0));
        assert_eq!(check.stats.loss_percent(), Some(0.0));
    }

    #[test]
    fn one_answer_verifies_and_the_rest_count_as_loss() {
        let (check, _) = script(
            vec![
                Err(io()),
                Ok((info(RUBYLIGHT), 30.0)),
                Err(io()),
                Err(io()),
                Err(io()),
            ],
            Ok(NetworkPath::Direct),
        );
        let check = check.unwrap();
        assert!(matches!(check.outcome, RemoteOutcome::Verified { .. }));
        assert_eq!(check.stats.loss_percent(), Some(80.0));
        assert_eq!(
            (check.stats.latency_ms(), check.stats.jitter_ms()),
            (Some(30.0), None)
        );
    }

    #[test]
    fn a_wrong_revoked_or_restricted_host_ends_the_check() {
        for (xml, expected) in [
            (RUBYLIGHT.replace("0123-ABCD", "other-pc"), "WrongHost"),
            (
                RUBYLIGHT.replace("<PairStatus>1", "<PairStatus>0"),
                "PairRevoked",
            ),
            (
                RUBYLIGHT.replace("<Permission>119480064", "<Permission>256"),
                "PermissionDenied",
            ),
        ] {
            let (check, probes) = script(
                vec![Ok((info(RUBYLIGHT), 10.0)), Ok((info(&xml), 10.0))],
                Ok(NetworkPath::Direct),
            );
            let check = check.unwrap();
            assert_eq!(probes, 2, "{expected}");
            assert!(
                format!("{:?}", check.outcome).starts_with(expected),
                "{:?}",
                check.outcome
            );
            assert_eq!(check.stats.loss_percent(), Some(50.0));
        }
    }

    #[test]
    fn no_answer_is_unreachable_with_the_last_error() {
        let (check, probes) = script(
            vec![
                Err(io()),
                Err(io()),
                Err(io()),
                Err(io()),
                Err(Error::CertificateMismatch),
            ],
            Ok(NetworkPath::Direct),
        );
        let check = check.unwrap();
        assert_eq!(probes, 5);
        assert!(matches!(
            check.outcome,
            RemoteOutcome::Unreachable(Error::CertificateMismatch)
        ));
        assert_eq!(check.stats.loss_percent(), Some(100.0));
        assert_eq!(check.stats.latency_ms(), None);
    }

    #[test]
    fn a_cancel_stops_the_probes() {
        let (check, probes) = script(
            vec![Ok((info(RUBYLIGHT), 10.0)), Err(Error::Cancelled)],
            Ok(NetworkPath::Direct),
        );
        assert!(matches!(check, Err(Error::Cancelled)));
        assert_eq!(probes, 2);
    }

    #[test]
    fn a_host_without_the_network_path_reports_why() {
        let identity = std::sync::Arc::new(crate::http::identity::tests::fixture());
        let client = HostClient::new("127.0.0.1", 1, identity)
            .unwrap()
            .with_https_port(1);
        client.pin(Some(client.identity().certificate_der().to_vec()));
        let without = RUBYLIGHT.replace("<NetworkPathVersion>1</NetworkPathVersion>", "");
        let check = run(
            "0123-ABCD",
            || Ok((info(&without), 10.0)),
            || {},
            |info| client.network_path(&info.capabilities),
        )
        .unwrap();
        assert!(matches!(
            check.outcome,
            RemoteOutcome::Verified {
                path: Err(HostApiError::Unavailable {
                    feature: Feature::NetworkPath,
                    availability: Availability::Unsupported(_),
                }),
                ..
            }
        ));
    }

    #[test]
    fn the_check_needs_the_pin_and_the_host_identity() {
        let identity = std::sync::Arc::new(crate::http::identity::tests::fixture());
        let client = HostClient::new("127.0.0.1", 1, identity)
            .unwrap()
            .with_https_port(1);
        assert!(matches!(
            client.remote_check("0123-ABCD"),
            Err(Error::NotPaired)
        ));
        client.pin(Some(client.identity().certificate_der().to_vec()));
        assert!(matches!(
            client.remote_check(""),
            Err(Error::InvalidArgument(_))
        ));
        let unknown_port = HostClient::new("127.0.0.1", 1, client.identity.clone()).unwrap();
        unknown_port.pin(Some(client.identity().certificate_der().to_vec()));
        assert!(matches!(
            unknown_port.remote_check("0123-ABCD"),
            Err(Error::InvalidArgument(_))
        ));
    }
}
