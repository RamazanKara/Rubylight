//! The optional host APIs (hardware metrics, network path) and the remote access check against
//! a mock HTTPS host: the pinned certificate, the paths, the content type checked before the
//! body is read, the size limits, and the host identity. Parsing and feature gating are
//! covered in `http/hardware_metrics.rs`, `http/network_path.rs` and `http/remote_check.rs`.

use butterpollo_core::crypto;
use nightjar_core::http::{
    ClientIdentity, Error, HostApiError, HostClient, NetworkPath, RemoteOutcome, ServerInfo,
    MAX_HARDWARE_METRICS_BYTES, MAX_NETWORK_PATH_BYTES,
};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex, OnceLock};

const HOST_ID: &str = "0123-ABCD";
const METRICS: &str = r#"{"version":1,"sampled_at_unix_ms":1791712800000,"interval_ms":1000,"cpu":{"cpu_usage_percent":12.5},"gpus":[]}"#;

fn client_identity() -> Arc<ClientIdentity> {
    static ID: OnceLock<Arc<ClientIdentity>> = OnceLock::new();
    ID.get_or_init(|| Arc::new(ClientIdentity::generate().unwrap()))
        .clone()
}

fn host_identity() -> &'static crypto::Identity {
    static ID: OnceLock<crypto::Identity> = OnceLock::new();
    ID.get_or_init(|| crypto::Identity::generate().unwrap())
}

/// A paired Rubylight serverinfo that offers both APIs.
fn server_info_xml(unique_id: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\">\
         <hostname>Gaming PC</hostname><uniqueid>{unique_id}</uniqueid><HttpsPort>47984</HttpsPort>\
         <PairStatus>1</PairStatus><currentgame>0</currentgame><state>SUNSHINE_SERVER_FREE</state>\
         <RustHostVersion>3.0.0</RustHostVersion><HardwareMetricsVersion>1</HardwareMetricsVersion>\
         <NetworkPathVersion>1</NetworkPathVersion></root>"
    )
}

fn capabilities() -> nightjar_core::http::HostCapabilities {
    ServerInfo::parse(&server_info_xml(HOST_ID), 47989)
        .unwrap()
        .capabilities
}

/// A reply: status, content type, the Content-Length to announce and the body.
struct Reply {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
}

fn reply(status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) -> Reply {
    Reply {
        status,
        content_type,
        body: body.into(),
    }
}

#[derive(Debug)]
struct ClientCertificate(Arc<rustls::crypto::CryptoProvider>);

impl rustls::server::danger::ClientCertVerifier for ClientCertificate {
    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &[]
    }
    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
        Ok(rustls::server::danger::ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// The request target, once the head has arrived.
fn read_target(stream: &mut impl Read) -> Option<String> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).ok()? == 0 {
            return None;
        }
        head.push(byte[0]);
    }
    String::from_utf8(head)
        .ok()?
        .split(' ')
        .nth(1)
        .map(str::to_owned)
}

struct MockHost {
    port: u16,
    /// Each request's path, and whether it came with a client certificate.
    seen: Arc<Mutex<Vec<(String, bool)>>>,
}

/// Serves `handler(path)` over TLS with the certificate of `identity`, asking for a client
/// certificate. Write errors are ignored: the client may hang up after the headers.
fn serve_as(
    identity: &crypto::Identity,
    handler: impl Fn(&str) -> Reply + Send + 'static,
) -> MockHost {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = Arc::new(
        rustls::ServerConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_client_cert_verifier(Arc::new(ClientCertificate(provider)))
            .with_single_cert(
                vec![CertificateDer::from_pem_slice(identity.certificate.as_bytes()).unwrap()],
                PrivateKeyDer::from_pem_slice(identity.private_pem.as_bytes()).unwrap(),
            )
            .unwrap(),
    );
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let connection = rustls::ServerConnection::new(config.clone()).unwrap();
            let mut tls = rustls::StreamOwned::new(connection, stream.unwrap());
            let Some(target) = read_target(&mut tls) else {
                continue;
            };
            let path = target.split('?').next().unwrap_or_default().to_owned();
            let certificate = tls.conn.peer_certificates().is_some_and(|c| !c.is_empty());
            log.lock().unwrap().push((path.clone(), certificate));
            let reply = handler(&path);
            let head = format!(
                "HTTP/1.1 {} Status\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                reply.status,
                reply.content_type,
                reply.body.len()
            );
            let _ = tls.write_all(head.as_bytes());
            let _ = tls.write_all(&reply.body);
            tls.conn.send_close_notify();
            let _ = tls.flush();
        }
    });
    MockHost { port, seen }
}

fn serve(handler: impl Fn(&str) -> Reply + Send + 'static) -> MockHost {
    serve_as(host_identity(), handler)
}

fn paired_client(host: &MockHost) -> HostClient {
    HostClient::new("127.0.0.1", 1, client_identity())
        .unwrap()
        .with_https_port(host.port)
        .with_server_certificate_pem(&host_identity().certificate)
        .unwrap()
}

fn malformed(result: Result<impl std::fmt::Debug, HostApiError>, needle: &str) -> bool {
    matches!(&result, Err(HostApiError::Host(Error::Malformed(m))) if m.contains(needle))
}

#[test]
fn hardware_metrics_are_read_over_pinned_https() {
    let host = serve(|_| reply(200, "application/json; charset=utf-8", METRICS));
    let sample = paired_client(&host)
        .hardware_metrics(&capabilities())
        .unwrap();
    assert_eq!(sample.cpu.get("cpu_usage_percent"), Some(12.5));
    assert_eq!(
        *host.seen.lock().unwrap(),
        [("/api/hardware-metrics".to_owned(), true)]
    );
}

#[test]
fn another_content_type_is_refused_before_the_body_is_read() {
    // The body is far over the limit: only a check before reading it says "not JSON".
    let body = vec![b'x'; 4 * MAX_HARDWARE_METRICS_BYTES];
    let host = serve(move |_| reply(200, "text/html", body.clone()));
    let client = paired_client(&host);
    assert!(malformed(
        client.hardware_metrics(&capabilities()),
        "is not JSON"
    ));
    assert!(malformed(
        client.network_path(&capabilities()),
        "is not JSON"
    ));
}

#[test]
fn oversized_bodies_are_refused() {
    let host = serve(|path| {
        let limit = if path == "/api/network-path" {
            MAX_NETWORK_PATH_BYTES
        } else {
            MAX_HARDWARE_METRICS_BYTES
        };
        reply(200, "application/json", vec![b' '; limit + 1])
    });
    let client = paired_client(&host);
    assert!(malformed(
        client.hardware_metrics(&capabilities()),
        "exceeds"
    ));
    assert!(malformed(client.network_path(&capabilities()), "exceeds"));
}

#[test]
fn the_network_path_is_read_over_pinned_https() {
    let host = serve(|_| reply(200, "application/json", r#"{"version":1,"kind":"relay"}"#));
    assert_eq!(
        paired_client(&host).network_path(&capabilities()).unwrap(),
        NetworkPath::Relay
    );
    assert_eq!(
        *host.seen.lock().unwrap(),
        [("/api/network-path".to_owned(), true)]
    );
}

fn remote_host(identity: &crypto::Identity, unique_id: &'static str) -> MockHost {
    serve_as(identity, move |path| match path {
        "/serverinfo" => reply(200, "application/xml", server_info_xml(unique_id)),
        "/api/network-path" => reply(200, "application/json", r#"{"version":1,"kind":"direct"}"#),
        _ => reply(404, "text/plain", "not found"),
    })
}

#[test]
fn the_remote_check_verifies_the_paired_host_and_reads_the_path() {
    let host = remote_host(host_identity(), HOST_ID);
    let check = paired_client(&host).remote_check(HOST_ID).unwrap();
    assert!(
        matches!(
            &check.outcome,
            RemoteOutcome::Verified { server_info, path: Ok(NetworkPath::Direct) }
                if server_info.unique_id == HOST_ID
        ),
        "{:?}",
        check.outcome
    );
    assert_eq!(check.stats.loss_percent(), Some(0.0));
    assert!(check.stats.latency_ms().is_some() && check.stats.jitter_ms().is_some());
    let seen = host.seen.lock().unwrap();
    assert_eq!(seen.len(), 6);
    assert!(seen[..5]
        .iter()
        .all(|(path, cert)| path == "/serverinfo" && *cert));
}

#[test]
fn another_pc_at_the_address_is_the_wrong_host() {
    let host = remote_host(host_identity(), "other-pc");
    let check = paired_client(&host).remote_check(HOST_ID).unwrap();
    assert!(
        matches!(check.outcome, RemoteOutcome::WrongHost),
        "{:?}",
        check.outcome
    );
    // The first answer ends the check, and the path is never asked.
    assert_eq!(host.seen.lock().unwrap().len(), 1);
}

#[test]
fn another_certificate_is_unreachable_not_verified() {
    let other = crypto::Identity::generate().unwrap();
    let host = remote_host(&other, HOST_ID);
    let check = paired_client(&host).remote_check(HOST_ID).unwrap();
    assert!(
        matches!(
            check.outcome,
            RemoteOutcome::Unreachable(Error::CertificateMismatch)
        ),
        "{:?}",
        check.outcome
    );
    assert_eq!(check.stats.loss_percent(), Some(100.0));
    assert!(host.seen.lock().unwrap().is_empty());
}
