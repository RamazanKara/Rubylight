//! The Rubylight native API v1 against the host's own code: the JSON fixtures of the pinned
//! `rubylight-native-api` crate, the host's real serialised output (`butterpollo_core::native`),
//! and `HostClient` talking to a mock HTTPS host that checks the pinned certificate, the client
//! certificate, the paths and the POST content type. Hosts without the field are covered at the
//! parse layer in `http/serverinfo.rs`.

use butterpollo_core::native::{self, SessionSnapshot};
use butterpollo_core::state::{App, Client};
use butterpollo_core::{crypto, native_api as host_api};
use nightjar_core::http::native_api::capabilities::Capabilities;
use nightjar_core::http::native_api::error::{ApiError, ErrorBody, ErrorCode};
use nightjar_core::http::native_api::library::Library;
use nightjar_core::http::native_api::radeon::{Radeon, SettingsWrite};
use nightjar_core::http::native_api::session::{ActionId, Owner, Session, SessionApp, State};
use nightjar_core::http::{ClientIdentity, Error, HostClient};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex, OnceLock};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/native-api");
const NAMES: [&str; 7] = [
    "capabilities",
    "library",
    "session",
    "session_launcher",
    "radeon",
    "settings_write",
    "error",
];

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{FIXTURES}/{name}.json")).unwrap()
}

fn client_identity() -> Arc<ClientIdentity> {
    static ID: OnceLock<Arc<ClientIdentity>> = OnceLock::new();
    ID.get_or_init(|| Arc::new(ClientIdentity::generate().unwrap()))
        .clone()
}

fn host_identity() -> &'static crypto::Identity {
    static ID: OnceLock<crypto::Identity> = OnceLock::new();
    ID.get_or_init(|| crypto::Identity::generate().unwrap())
}

/// Parses a fixture and writes it back: a field the types drop or rename fails here.
fn round_trip<T: DeserializeOwned + Serialize + PartialEq + std::fmt::Debug>(name: &str) -> T {
    let text = fixture(name);
    let typed: T = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(
        serde_json::to_value(&typed).unwrap(),
        serde_json::from_str::<Value>(&text).unwrap(),
        "{name}"
    );
    typed
}

#[test]
fn the_hosts_fixtures_parse_and_write_back() {
    let capabilities: Capabilities = round_trip("capabilities");
    assert_eq!(capabilities.api_version, 1);
    assert!(capabilities.permissions.launch && !capabilities.permissions.host_control);
    let library: Library = round_trip("library");
    assert_eq!(library.games.len(), 2);
    assert!(library.games[1].running && library.games[1].game_id.is_none());
    let session: Session = round_trip("session");
    assert_eq!(
        (session.state, session.owner),
        (State::Suspended, Owner::You)
    );
    assert_eq!(session.actions.len(), 4);
    let radeon: Radeon = round_trip("radeon");
    assert!(radeon.host_control && radeon.writable_now);
    let write: SettingsWrite = round_trip("settings_write");
    assert_eq!((write.gpu, write.changes.len()), (11520, 2));
    let error: ErrorBody = round_trip("error");
    assert_eq!(error.error.code, ErrorCode::HostControlRequired);
}

/// The vendored fixtures are copies of the pinned crate's, byte for byte, so a pin bump that
/// changes one fails here. The crate is located through `cargo metadata`; a failure to find it
/// fails the test.
#[test]
fn the_vendored_fixtures_match_the_pinned_crate() {
    let output = std::process::Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--locked"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run cargo metadata");
    assert!(
        output.status.success(),
        "cargo metadata: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Value = serde_json::from_slice(&output.stdout).unwrap();
    let manifest = metadata["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|package| package["name"] == "rubylight-native-api")
        .and_then(|package| package["manifest_path"].as_str())
        .expect("rubylight-native-api is not in cargo metadata");
    let dir = std::path::Path::new(manifest)
        .parent()
        .unwrap()
        .join("tests/fixtures");
    let mut compared = 0;
    for name in NAMES {
        let upstream = std::fs::read_to_string(dir.join(format!("{name}.json")))
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        assert_eq!(
            fixture(name),
            upstream,
            "{name}.json drifted from {manifest}"
        );
        compared += 1;
    }
    assert_eq!(compared, NAMES.len(), "fixtures compared");
    // Every fixture upstream is vendored.
    let upstream = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .count();
    assert_eq!(
        upstream,
        NAMES.len(),
        "{} has fixtures that are not vendored",
        dir.display()
    );
}

// ---------------------------------------------------------------------------------------
// The host's real output.

fn client(extra: Value) -> Client {
    let mut value = json!({"name": "Phone", "cert": "", "uuid": "me", "perm": (1u32 << 24) | (1 << 25) | (1 << 26)});
    value
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    serde_json::from_value(value).unwrap()
}

fn dota() -> App {
    serde_json::from_value(json!({
        "name": "Dota 2", "uuid": "c6f1a9a4-4b51-5f43-9d0b-7a2b1c2d3e4f",
        "launcher-source": "steam", "launcher-id": "570", "image-path": "dota.png"
    }))
    .unwrap()
}

fn suspended_snapshot() -> SessionSnapshot {
    SessionSnapshot {
        revision: 42,
        state: host_api::session::State::Suspended,
        game_running: true,
        owner: Some("me".into()),
        app: Some(host_api::session::SessionApp {
            install_id: "c6f1a9a4-4b51-5f43-9d0b-7a2b1c2d3e4f".into(),
            app_id: 1093052211,
            title: "Dota 2".into(),
        }),
        since_unix_ms: Some(1792303200000),
        suspended_until_unix_ms: Some(1792310400000),
        ..Default::default()
    }
}

#[test]
fn the_hosts_session_view_is_the_session_fixture() {
    let view = native::session_view(&suspended_snapshot(), &client(json!({})));
    assert_eq!(
        serde_json::to_value(&view).unwrap(),
        serde_json::from_str::<Value>(&fixture("session")).unwrap()
    );
}

#[test]
fn the_hosts_library_reads_back() {
    let apps = [dota()];
    let host = native::library(
        &apps,
        Some(&native::Scan::new(["steam:570".into()])),
        Some(apps[0].id()),
    );
    let text = serde_json::to_string(&host).unwrap();
    let library: Library = serde_json::from_str(&text).unwrap();
    assert_eq!(library.revision, host.revision);
    let game = &library.games[0];
    assert_eq!(
        (
            game.title.as_str(),
            game.game_id.as_deref(),
            game.installed,
            game.running
        ),
        ("Dota 2", Some("steam:570"), Some(true), true)
    );
    assert_eq!(game.store.as_ref().map(|s| s.name.as_str()), Some("Steam"));
    assert_eq!(
        game.artwork.cover.as_deref(),
        Some(format!("/appasset?appid={}&AssetType=2&AssetIdx=0", apps[0].id()).as_str())
    );
}

// ---------------------------------------------------------------------------------------
// A mock HTTPS host.

#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    query: String,
    content_type: Option<String>,
    body: Vec<u8>,
    client_certificate: Option<Vec<u8>>,
}

struct MockHost {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
}

#[derive(Debug)]
struct AnyClientCertificate(Arc<rustls::crypto::CryptoProvider>);

impl rustls::server::danger::ClientCertVerifier for AnyClientCertificate {
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
    fn client_auth_mandatory(&self) -> bool {
        false
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

/// Reads the request head and body.
fn read_request(stream: &mut impl Read) -> Option<(String, String, Option<String>, Vec<u8>)> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).ok()? == 0 {
            return None;
        }
        head.push(byte[0]);
    }
    let head = String::from_utf8(head).unwrap();
    let header = |name: &str| {
        head.lines()
            .find_map(|line| line.strip_prefix(name))
            .map(|v| v.trim().to_owned())
    };
    let mut words = head.split(' ');
    let (method, target) = (words.next()?.to_owned(), words.next()?.to_owned());
    let mut body = vec![0; header("Content-Length: ").map_or(0, |n| n.parse().unwrap())];
    stream.read_exact(&mut body).unwrap();
    Some((method, target, header("Content-Type: "), body))
}

/// Serves `handler(method, path, body) -> (status, body)` over TLS, asking for a client certificate.
fn serve(handler: impl Fn(&str, &str, &[u8]) -> (u16, String) + Send + 'static) -> MockHost {
    serve_as(host_identity(), handler)
}

/// [`serve`] with the TLS certificate of `identity`.
fn serve_as(
    identity: &crypto::Identity,
    handler: impl Fn(&str, &str, &[u8]) -> (u16, String) + Send + 'static,
) -> MockHost {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = Arc::new(
        rustls::ServerConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_client_cert_verifier(Arc::new(AnyClientCertificate(provider)))
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
            let Some((method, target, content_type, body)) = read_request(&mut tls) else {
                continue;
            };
            let (path, query) = target.split_once('?').unwrap_or((&target, ""));
            let client_certificate = tls
                .conn
                .peer_certificates()
                .and_then(|c| c.first())
                .map(|c| c.to_vec());
            log.lock().unwrap().push(Seen {
                method: method.clone(),
                path: path.to_owned(),
                query: query.to_owned(),
                content_type,
                body: body.clone(),
                client_certificate,
            });
            let (status, reply) = handler(&method, path, &body);
            write!(
                tls,
                "HTTP/1.1 {status} Status\r\nContent-Type: application/json; charset=utf-8\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            )
            .unwrap();
            tls.conn.send_close_notify();
            let _ = tls.flush();
        }
    });
    MockHost { port, seen }
}

fn paired_client(host: &MockHost) -> HostClient {
    HostClient::new("127.0.0.1", 1, client_identity())
        .unwrap()
        .with_https_port(host.port)
        .with_server_certificate_pem(&host_identity().certificate)
        .unwrap()
}

fn error_body(code: ErrorCode, message: &str) -> String {
    serde_json::to_string(&ErrorBody {
        error: ApiError {
            code,
            message: message.into(),
            setting: None,
            retry_after_ms: None,
        },
    })
    .unwrap()
}

#[test]
fn every_call_reaches_its_path_with_the_certificate_and_the_content_type() {
    let apps = [dota()];
    let library = serde_json::to_string(&native::library(&apps, None, None)).unwrap();
    let view = |snapshot: &SessionSnapshot| {
        serde_json::to_string(&native::session_view(
            snapshot,
            &client(json!({"host_control": true})),
        ))
        .unwrap()
    };
    let (suspended, ended) = (
        view(&suspended_snapshot()),
        view(&SessionSnapshot::default()),
    );
    let host = serve(move |method, path, _| match (method, path) {
        ("GET", "/api/native/v1/capabilities") => (200, fixture("capabilities")),
        ("GET", "/api/native/v1/library") => (200, library.clone()),
        ("GET", "/api/native/v1/session") => (200, suspended.clone()),
        (
            "POST",
            "/api/native/v1/session/disconnect"
            | "/api/native/v1/session/end"
            | "/api/native/v1/session/close-game",
        ) => (200, ended.clone()),
        ("GET", "/api/native/v1/radeon")
        | ("POST", "/api/native/v1/radeon/settings" | "/api/native/v1/radeon/restore") => {
            (200, fixture("radeon"))
        }
        other => (404, error_body(ErrorCode::NotFound, &format!("{other:?}"))),
    });
    let client = paired_client(&host);

    let capabilities = client.native_capabilities().unwrap();
    assert_eq!(
        capabilities.features,
        [
            "library",
            "session",
            "session.close_game",
            "radeon.read",
            "radeon.write"
        ]
    );
    assert_eq!(client.library().unwrap().games[0].title, "Dota 2");
    let session = client.session().unwrap();
    assert_eq!(
        (session.state, session.owner),
        (State::Suspended, Owner::You)
    );
    assert_eq!(
        session.app,
        Some(SessionApp {
            install_id: "c6f1a9a4-4b51-5f43-9d0b-7a2b1c2d3e4f".into(),
            app_id: 1093052211,
            title: "Dota 2".into()
        })
    );
    for action in [ActionId::Disconnect, ActionId::End, ActionId::CloseGame] {
        assert_eq!(client.session_action(action).unwrap().state, State::Idle);
    }
    assert!(client.radeon().unwrap().writable_now);
    let write: SettingsWrite = serde_json::from_str(&fixture("settings_write")).unwrap();
    assert!(client.write_radeon(&write).unwrap().host_control);
    assert_eq!(client.restore_radeon().unwrap().revision, 7);

    let seen = host.seen.lock().unwrap();
    let requests: Vec<_> = seen
        .iter()
        .map(|s| format!("{} {}", s.method, s.path))
        .collect();
    assert_eq!(
        requests,
        [
            "GET /api/native/v1/capabilities",
            "GET /api/native/v1/library",
            "GET /api/native/v1/session",
            "POST /api/native/v1/session/disconnect",
            "POST /api/native/v1/session/end",
            "POST /api/native/v1/session/close-game",
            "GET /api/native/v1/radeon",
            "POST /api/native/v1/radeon/settings",
            "POST /api/native/v1/radeon/restore",
        ]
    );
    for request in seen.iter() {
        assert_eq!(
            request.client_certificate.as_deref(),
            Some(client_identity().certificate_der()),
            "{}",
            request.path
        );
        assert!(request.query.contains("uniqueid="), "{}", request.query);
        if request.method == "POST" {
            assert_eq!(
                request.content_type.as_deref(),
                Some("application/json"),
                "{}",
                request.path
            );
        } else {
            assert!(request.content_type.is_none() && request.body.is_empty());
        }
    }
    // Actions and restore post an empty object; a write posts the host's request type as it is.
    assert_eq!(seen[3].body, b"{}");
    assert_eq!(seen[8].body, b"{}");
    assert_eq!(
        serde_json::from_slice::<Value>(&seen[7].body).unwrap(),
        serde_json::from_str::<Value>(&fixture("settings_write")).unwrap()
    );
    // The host's request type accepts exactly what the client sent.
    assert!(serde_json::from_slice::<host_api::radeon::SettingsWrite>(&seen[7].body).is_ok());
}

#[test]
fn resume_is_not_a_native_post() {
    let host = serve(|_, _, _| (500, String::new()));
    let client = paired_client(&host);
    assert!(matches!(
        client.session_action(ActionId::Resume),
        Err(Error::InvalidArgument(_))
    ));
    assert!(host.seen.lock().unwrap().is_empty());
}

#[test]
fn error_answers_carry_the_code_and_the_status() {
    // The host answers each code with the status the crate says it does.
    let codes = [
        ErrorCode::NotPaired,
        ErrorCode::PermissionRequired,
        ErrorCode::HostControlRequired,
        ErrorCode::NotFound,
        ErrorCode::InvalidRequest,
        ErrorCode::InvalidValue,
        ErrorCode::NotWritable,
        ErrorCode::NoSession,
        ErrorCode::NotSessionOwner,
        ErrorCode::SessionBusy,
        ErrorCode::DriverRejected,
        ErrorCode::TemporarilyUnavailable,
        ErrorCode::Internal,
    ];
    for code in codes {
        let host = serve(move |_, _, _| (code.status(), error_body(code, "Plain English.")));
        let error = paired_client(&host).session().unwrap_err();
        let api = error
            .api_error()
            .unwrap_or_else(|| panic!("{code:?}: {error}"));
        assert_eq!((api.code, api.message.as_str()), (code, "Plain English."));
        assert_eq!(error.http_status(), Some(code.status()));
        assert_eq!(error.to_string(), "Plain English.");
    }

    // A code from a newer host keeps its status.
    let host = serve(|_, _, _| {
        (
            418,
            r#"{"error":{"code":"teapot","message":"A newer host.","retry_after_ms":250}}"#.into(),
        )
    });
    let error = paired_client(&host).library().unwrap_err();
    let api = error.api_error().unwrap();
    assert_eq!(
        (api.code, api.retry_after_ms),
        (ErrorCode::Unknown, Some(250))
    );
    assert_eq!(error.http_status(), Some(418));

    // A 404 with no error body is a host without the native API (2.6.x, Sunshine, Apollo).
    let host = serve(|_, _, _| (404, "<html>Not Found</html>".into()));
    let error = paired_client(&host).native_capabilities().unwrap_err();
    assert!(
        matches!(error, Error::NotFound(ref url) if url == "/api/native/v1/capabilities"),
        "{error:?}"
    );
    assert_eq!(error.http_status(), Some(404));

    // A proxy's empty 502 is a plain status error.
    let host = serve(|_, _, _| (502, String::new()));
    let error = paired_client(&host).radeon().unwrap_err();
    assert!(
        matches!(error, Error::Status { code: 502, .. }),
        "{error:?}"
    );
}

#[test]
fn an_unpaired_client_does_not_connect() {
    let client = HostClient::new("127.0.0.1", 1, client_identity())
        .unwrap()
        .with_https_port(1);
    assert!(matches!(client.session(), Err(Error::NotPaired)));
    assert!(matches!(
        client.write_radeon(&serde_json::from_str(&fixture("settings_write")).unwrap()),
        Err(Error::NotPaired)
    ));
}

#[test]
fn another_host_certificate_is_refused_for_every_native_call() {
    // The client pinned host_identity() at pairing; a host showing another certificate gets
    // no request through, whatever it would answer.
    let other = crypto::Identity::generate().unwrap();
    assert_ne!(other.certificate, host_identity().certificate);
    let host = serve_as(&other, |_, _, _| (200, fixture("session")));
    let client = paired_client(&host);
    assert!(matches!(client.session(), Err(Error::CertificateMismatch)));
    assert!(matches!(
        client.session_action(ActionId::End),
        Err(Error::CertificateMismatch)
    ));
    assert!(matches!(
        client.native_capabilities(),
        Err(Error::CertificateMismatch)
    ));
    assert!(host.seen.lock().unwrap().is_empty());
}
