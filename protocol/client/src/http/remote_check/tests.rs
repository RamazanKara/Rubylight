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
        || false,
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
fn a_probe_cut_off_by_a_cancel_is_a_cancel_not_a_failure() {
    // The shut socket reads as an early EOF, which the transport reports as Malformed.
    let cancelled = Cell::new(false);
    let probes = Cell::new(0);
    let check = run(
        "0123-ABCD",
        || {
            probes.set(probes.get() + 1);
            if probes.get() < REMOTE_CHECK_PROBES {
                return Ok((info(RUBYLIGHT), 10.0));
            }
            cancelled.set(true);
            Err(Error::Malformed(
                "connection closed before the HTTP header ended".into(),
            ))
        },
        || {},
        |_| panic!("the path is not read after a cancel"),
        || cancelled.get(),
    );
    assert!(matches!(check, Err(Error::Cancelled)), "{check:?}");
}

#[test]
fn a_cancel_during_the_path_is_a_cancel_not_a_verified_host() {
    for error in [
        Error::Cancelled,
        Error::Malformed("connection closed before the HTTP header ended".into()),
    ] {
        let cancelled = Cell::new(false);
        let check = run(
            "0123-ABCD",
            || Ok((info(RUBYLIGHT), 10.0)),
            || {},
            |_| {
                cancelled.set(true);
                Err(HostApiError::Host(error))
            },
            || cancelled.get(),
        );
        assert!(matches!(check, Err(Error::Cancelled)), "{check:?}");
    }
    // A cancel between the last probe and the path skips the path.
    let check = run(
        "0123-ABCD",
        || Ok((info(RUBYLIGHT), 10.0)),
        || {},
        |_| panic!("the path is not read after a cancel"),
        || true,
    );
    assert!(matches!(check, Err(Error::Cancelled)));
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
        || false,
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

/// A local stand-in for a PC's HTTP port: each connection gets `reply`'s bytes, or nothing
/// for `None` (it reads the request and holds the socket open).
fn http_pc(reply: Option<String>, connections: usize) -> (u16, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let served = std::thread::spawn(move || {
        let mut held = Vec::new();
        for _ in 0..connections {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let _ = socket.read(&mut request);
            assert!(
                request.starts_with(b"GET /serverinfo?"),
                "{}",
                String::from_utf8_lossy(&request)
            );
            match &reply {
                Some(reply) => socket.write_all(reply.as_bytes()).unwrap(),
                None => held.push(socket),
            }
        }
        // A held socket stays silent well past the probe's timeout.
        std::thread::sleep(Duration::from_millis(if held.is_empty() { 0 } else { 2500 }));
    });
    (port, served)
}

fn http_reply(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/xml\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{body}",
        body.len()
    )
}

#[test]
fn the_delay_probe_times_one_plain_serverinfo() {
    let (port, served) = http_pc(Some(http_reply("200 OK", RUBYLIGHT)), 1);
    let identity = std::sync::Arc::new(crate::http::identity::tests::fixture());
    let client = HostClient::new("127.0.0.1", port, identity).unwrap();
    let ms = client.probe_server_latency(Duration::from_secs(2)).expect("an answer");
    assert!(ms.is_finite() && (0.0..2000.0).contains(&ms), "{ms}");
    served.join().unwrap();
}

#[test]
fn a_delay_probe_fails_on_a_refusal_a_redirect_or_no_serverinfo() {
    let identity = std::sync::Arc::new(crate::http::identity::tests::fixture());
    for reply in [
        http_reply("404 Not Found", ""),
        "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/serverinfo\r\n\
             Content-Length: 0\r\nConnection: close\r\n\r\n"
            .to_owned(),
        http_reply("200 OK", "<html>not a PC</html>"),
    ] {
        let (port, served) = http_pc(Some(reply.clone()), 1);
        let client = HostClient::new("127.0.0.1", port, identity.clone()).unwrap();
        assert!(client.probe_server_latency(Duration::from_secs(2)).is_err(), "{reply}");
        served.join().unwrap();
    }
}

#[test]
fn a_delay_probe_the_pc_never_answers_ends_at_its_timeout() {
    let (port, served) = http_pc(None, 1);
    let identity = std::sync::Arc::new(crate::http::identity::tests::fixture());
    let client = HostClient::new("127.0.0.1", port, identity).unwrap();
    let started = Instant::now();
    assert!(client.probe_server_latency(Duration::from_millis(200)).is_err());
    assert!(started.elapsed() < Duration::from_millis(1500), "{:?}", started.elapsed());
    served.join().unwrap();
}
