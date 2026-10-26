//! `GET /api/webrtc/signal`, the console's own stream page: who gets the socket, and how a
//! sign-out ends it.
use super::*;
use crate::state::test_support::Fixture;
use crate::webrtc::server::{Console, tests::Ws};
use axum::body::{Body, to_bytes};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
    service::TowerToHyperService,
};
use std::net::SocketAddr;
use tower::ServiceExt;

/// A browser signed in to the console as "admin".
struct SignIn {
    access: String,
    refresh: String,
    csrf: String,
}

fn sign_in(f: &Fixture) -> SignIn {
    let (access, refresh, csrf, _) = f
        .host
        .new_web_session(
            "admin".into(),
            false,
            "test".into(),
            "127.0.0.1".into(),
            None,
        )
        .unwrap();
    SignIn {
        access,
        refresh,
        csrf,
    }
}

fn configure(f: &Fixture, settings: Value) {
    f.host
        .config
        .write()
        .unwrap()
        .update(settings.as_object().unwrap())
        .unwrap();
}

/// What the stream page sends with its upgrade.
fn page(s: &SignIn) -> Vec<(&'static str, String)> {
    vec![
        ("cookie", format!("__Host-apollo_session={}", s.access)),
        ("origin", "https://test".into()),
        (
            "sec-websocket-protocol",
            format!("rubylight.signal.v2, csrf.{}", s.csrf),
        ),
    ]
}

/// `page` with `name` replaced by `value`, or left out when `value` is None.
fn changed(s: &SignIn, name: &str, value: Option<&str>) -> Vec<(&'static str, String)> {
    let mut headers = page(s);
    headers.retain(|(n, _)| *n != name);
    if let Some(value) = value {
        let name = page(s).into_iter().find(|(n, _)| *n == name).unwrap().0;
        headers.push((name, value.to_owned()));
    }
    headers
}

/// The status and error of an upgrade request. A oneshot request cannot be upgraded, so one
/// that passes every check ends in 426.
async fn attempt(
    f: &Fixture,
    connection: Connection,
    headers: &[(&str, String)],
) -> (StatusCode, String) {
    let mut request = Request::builder()
        .uri("/api/webrtc/signal")
        .header(header::HOST, "test")
        .header(header::CONNECTION, "upgrade")
        .header(header::UPGRADE, "websocket")
        .header(header::SEC_WEBSOCKET_VERSION, "13")
        .header(header::SEC_WEBSOCKET_KEY, "dGhlIHNhbXBsZSBub25jZQ==");
    for (name, value) in headers {
        request = request.header(*name, value);
    }
    let response = router(f.host.clone())
        .layer(Extension(connection))
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let error = serde_json::from_slice::<Value>(&body).unwrap_or_default()["error"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    (status, error)
}

fn console_devices(f: &Fixture) -> Vec<state::Client> {
    f.host
        .paired
        .read()
        .unwrap()
        .clients
        .iter()
        .filter(|c| c.name.starts_with("Console"))
        .cloned()
        .collect()
}

#[tokio::test]
async fn only_a_console_sign_in_opens_the_stream_socket() {
    let f = Fixture::new();
    configure(&f, json!({"webrtc_server": true}));
    let s = sign_in(&f);
    let local = f.connection(true);
    // Basic credentials and API tokens are good for the API, not for this socket.
    let credentials = Credentials::new("admin".into(), "console-test").unwrap();
    f.host.save_credentials(&credentials).unwrap();
    *f.host.credentials.write().unwrap() = Some(credentials);
    let issued = router(f.host.clone())
        .layer(Extension(f.connection(true)))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/token")
                .header(header::AUTHORIZATION, "Basic YWRtaW46Y29uc29sZS10ZXN0")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"scopes":[{"path":"/api/apps","methods":["GET"]}]}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(issued.status(), StatusCode::OK);
    let issued: Value =
        serde_json::from_slice(&to_bytes(issued.into_body(), 1 << 20).await.unwrap()).unwrap();
    let token = issued["token"].as_str().unwrap();
    for (what, cookie) in [
        ("no cookie", None),
        ("an unknown session", Some("__Host-apollo_session=00")),
    ] {
        let mut headers = changed(&s, "cookie", cookie);
        let (status, _) = attempt(&f, local.clone(), &headers).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{what}");
        headers.push(("authorization", "Basic YWRtaW46Y29uc29sZS10ZXN0".into()));
        let (status, error) = attempt(&f, local.clone(), &headers).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{what} with Basic credentials"
        );
        // The credentials get past the console's guard; with a stale cookie the guard refuses.
        if cookie.is_none() {
            assert_eq!(error, "sign in to the console to stream");
        }
        headers.pop();
        headers.push(("authorization", format!("Bearer {token}")));
        let (status, _) = attempt(&f, local.clone(), &headers).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{what} with an API token");
    }
    // An expired session is no sign-in either.
    for session in f.host.web_sessions.lock().unwrap().values_mut() {
        session.expires = Instant::now();
    }
    let (status, _) = attempt(&f, local.clone(), &page(&s)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(console_devices(&f).is_empty(), "no device for a refusal");
    let s = sign_in(&f);
    let (status, _) = attempt(&f, local, &page(&s)).await;
    assert_eq!(status, StatusCode::UPGRADE_REQUIRED, "every check passed");
}

#[tokio::test]
async fn the_stream_socket_needs_the_consoles_origin_and_csrf_token() {
    let f = Fixture::new();
    configure(&f, json!({"webrtc_server": true}));
    let s = sign_in(&f);
    let local = f.connection(true);
    let protocol = |p: String| changed(&s, "sec-websocket-protocol", Some(&p));
    for (what, headers, status) in [
        (
            "no origin",
            changed(&s, "origin", None),
            StatusCode::FORBIDDEN,
        ),
        (
            "another site",
            changed(&s, "origin", Some("https://evil.example")),
            StatusCode::FORBIDDEN,
        ),
        (
            "plain http",
            changed(&s, "origin", Some("http://test")),
            StatusCode::FORBIDDEN,
        ),
        (
            "no subprotocols",
            changed(&s, "sec-websocket-protocol", None),
            StatusCode::FORBIDDEN,
        ),
        (
            "no CSRF token",
            protocol("rubylight.signal.v2".into()),
            StatusCode::FORBIDDEN,
        ),
        (
            "a wrong CSRF token",
            protocol(format!("rubylight.signal.v2, csrf.{}", "0".repeat(64))),
            StatusCode::FORBIDDEN,
        ),
        (
            "another session's CSRF token",
            protocol(format!("rubylight.signal.v2, csrf.{}", sign_in(&f).csrf)),
            StatusCode::FORBIDDEN,
        ),
        (
            "no signalling protocol",
            protocol(format!("csrf.{}", s.csrf)),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let (got, _) = attempt(&f, local.clone(), &headers).await;
        assert_eq!(got, status, "{what}");
    }
    assert!(console_devices(&f).is_empty(), "no device for a refusal");
    let headers = protocol(format!("csrf.{}, rubylight.signal.v2", s.csrf));
    let (status, _) = attempt(&f, local.clone(), &headers).await;
    assert_eq!(status, StatusCode::UPGRADE_REQUIRED, "in either order");
    // An origin the settings allow is the console's too.
    configure(
        &f,
        json!({"csrf_allowed_origins": "[\"https://console.example\"]"}),
    );
    let headers = changed(&s, "origin", Some("https://console.example"));
    let (status, _) = attempt(&f, local, &headers).await;
    assert_eq!(status, StatusCode::UPGRADE_REQUIRED);
}

#[tokio::test]
async fn the_stream_socket_needs_the_local_network_the_webrtc_server_and_an_enabled_device() {
    let f = Fixture::new();
    let s = sign_in(&f);
    // A console the settings open to the internet still streams on the local network only.
    configure(
        &f,
        json!({"origin_web_ui_allowed": "wan", "webrtc_server": true}),
    );
    let mut outside = f.connection(true);
    outside.peer = "8.8.8.8:50000".parse().unwrap();
    let (status, _) = attempt(&f, outside, &page(&s)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let local = f.connection(true);
    configure(&f, json!({"webrtc_server": false}));
    let (status, _) = attempt(&f, local.clone(), &page(&s)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    // A request that passes every check but is no upgrade makes no device either.
    configure(&f, json!({"webrtc_server": true}));
    let (status, _) = attempt(&f, local.clone(), &page(&s)).await;
    assert_eq!(status, StatusCode::UPGRADE_REQUIRED);
    assert!(
        console_devices(&f).is_empty(),
        "no device without an upgrade"
    );
    // The first stream makes the console a device that can launch apps.
    let address = serve(&f, Console::default()).await;
    let ws = open(address, &s).await;
    let devices = console_devices(&f);
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].name, "Console (admin)");
    assert!(devices[0].is_webrtc());
    assert_eq!(devices[0].console(), Some("admin"));
    assert_eq!(devices[0].perm, state::FIRST_DEVICE_PERMISSION);
    // It leaves the first phone or TV everything.
    assert_eq!(
        f.host.paired.read().unwrap().new_device_permission(),
        state::FIRST_DEVICE_PERMISSION
    );
    drop(ws);
    // The same sign-in streams as the same device.
    open(address, &s).await;
    assert_eq!(console_devices(&f).len(), 1);
    // A disabled device stays disabled.
    crate::web::update_clients(
        &f.host,
        "POST",
        "/api/clients/update",
        &json!({"uuid": devices[0].uuid, "enabled": false}),
    )
    .unwrap();
    let head = refused(address, &s).await;
    assert!(head.starts_with("HTTP/1.1 423"), "{head}");
    assert!(
        head.contains("this console's device is disabled on the Devices page"),
        "{head}"
    );
    assert_eq!(console_devices(&f).len(), 1);
}

#[tokio::test]
async fn the_consoles_device_unpairs_no_tv_and_gets_everything_in_one_save() {
    let f = Fixture::new();
    configure(&f, json!({"webrtc_server": true}));
    // As many TVs as the host keeps.
    for n in 0..butterpollo_core::webrtc::auth::MAX_PAIRED as u8 {
        let id = format!("{:02x}", n + 1).repeat(16);
        let secret = format!("{:02x}", n + 1).repeat(32);
        f.host
            .paired
            .write()
            .unwrap()
            .add_webrtc(
                &f.host.paired_path,
                state::Client::paired_over_webrtc(format!("TV {n}"), &id, &secret),
                butterpollo_core::webrtc::auth::MAX_PAIRED,
            )
            .unwrap();
    }
    let tvs: Vec<String> = f
        .host
        .paired
        .read()
        .unwrap()
        .clients
        .iter()
        .map(|c| c.uuid.clone())
        .collect();
    let address = serve(&f, Console::default()).await;
    open(address, &sign_in(&f)).await;
    // Every TV stays paired, and the saved device has every permission.
    let saved = state::PairedState::load(&f.host.paired_path).unwrap();
    for uuid in &tvs {
        assert!(saved.clients.iter().any(|c| &c.uuid == uuid), "{uuid}");
    }
    let console = saved
        .clients
        .iter()
        .find(|c| c.console() == Some("admin"))
        .unwrap();
    assert_eq!(console.perm, state::FIRST_DEVICE_PERMISSION);
    assert_eq!(saved.clients.len(), tvs.len() + 1);
}

/// The response head of an upgrade the console refuses.
async fn refused(address: SocketAddr, s: &SignIn) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let headers: String = page(s)
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect();
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    stream
        .write_all(
            format!("GET /api/webrtc/signal HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{headers}\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = Vec::new();
    let mut buffer = [0u8; 4096];
    // The head and the JSON error; the console keeps the connection open after it.
    while !String::from_utf8_lossy(&response).contains("}") {
        let n = stream.read(&mut buffer).await.unwrap();
        assert!(n > 0, "{}", String::from_utf8_lossy(&response));
        response.extend_from_slice(&buffer[..n]);
    }
    String::from_utf8_lossy(&response).into_owned()
}

/// The console on a real socket, as `tls::serve` runs it (without TLS).
async fn serve(f: &Fixture, console: Console) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = listener.local_addr().unwrap();
    let router = router_with(f.host.clone(), console);
    tokio::spawn(async move {
        while let Ok((socket, peer)) = listener.accept().await {
            let service = TowerToHyperService::new(router.clone().layer(Extension(Connection {
                peer,
                local,
                certificate: None,
                tls: true,
            })));
            tokio::spawn(async move {
                let _ = Builder::new(TokioExecutor::new())
                    .serve_connection_with_upgrades(TokioIo::new(socket), service)
                    .await;
            });
        }
    });
    local
}

async fn open(address: SocketAddr, s: &SignIn) -> Ws {
    let headers: String = page(s)
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect();
    let mut ws = Ws::connect_with(address, "/api/webrtc/signal", &headers).await;
    assert!(
        ws.head
            .to_ascii_lowercase()
            .contains("\r\nsec-websocket-protocol: rubylight.signal.v2\r\n"),
        "{}",
        ws.head
    );
    // Signed in already: no auth-required, straight to the stream's capabilities.
    ws.expect("authenticated").await;
    ws.expect("gateway-status").await;
    ws.expect("capabilities").await;
    ws
}

/// Whether the socket stays open, with nothing to say, for a while.
async fn quiet(ws: &mut Ws) -> bool {
    tokio::time::timeout(Duration::from_millis(400), ws.next())
        .await
        .is_err()
}

async fn post(f: &Fixture, path: &str, headers: &[(&str, String)]) -> Value {
    let mut request = Request::builder().method("POST").uri(path);
    for (name, value) in headers {
        request = request.header(*name, value);
    }
    let response = router(f.host.clone())
        .layer(Extension(f.connection(true)))
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{path}");
    serde_json::from_slice(&to_bytes(response.into_body(), 1 << 20).await.unwrap()).unwrap()
}

#[tokio::test]
async fn a_console_stream_socket_outlives_a_token_refresh_but_not_a_sign_out() {
    let f = Fixture::new();
    configure(&f, json!({"webrtc_server": true}));
    let address = serve(
        &f,
        Console::default().checking_every(Duration::from_millis(50)),
    )
    .await;
    let s = sign_in(&f);
    let mut ws = open(address, &s).await;
    // The access token expires during the stream; the sign-in holds while it can be refreshed.
    for session in f.host.web_sessions.lock().unwrap().values_mut() {
        session.expires = Instant::now();
    }
    assert!(
        quiet(&mut ws).await,
        "an expired access token keeps the stream"
    );
    // The page refreshes its tokens; the old ones are gone after it.
    let refreshed = post(
        &f,
        "/api/auth/refresh",
        &[("cookie", format!("__Host-apollo_refresh={}", s.refresh))],
    )
    .await;
    assert!(quiet(&mut ws).await, "a refresh keeps the stream");
    // Signing out of the console ends it.
    post(
        &f,
        "/api/auth/logout",
        &[
            (
                "cookie",
                format!(
                    "__Host-apollo_session={}",
                    refreshed["access_token"].as_str().unwrap()
                ),
            ),
            (
                "x-csrf-token",
                refreshed["csrf_token"].as_str().unwrap().into(),
            ),
        ],
    )
    .await;
    ws.closed().await;
    assert_eq!(ws.close_reason.as_deref(), Some("signed out"));
}

#[tokio::test]
async fn a_console_stream_socket_ends_when_the_webrtc_server_is_turned_off() {
    let f = Fixture::new();
    configure(&f, json!({"webrtc_server": true}));
    let address = serve(
        &f,
        Console::default().checking_every(Duration::from_millis(50)),
    )
    .await;
    let mut ws = open(address, &sign_in(&f)).await;
    assert!(quiet(&mut ws).await);
    configure(&f, json!({"webrtc_server": false}));
    ws.closed().await;
    assert_eq!(ws.close_reason.as_deref(), Some("the WebRTC server is off"));
}

#[tokio::test]
async fn a_refresh_keeps_the_sign_in_the_live_session_has_not_the_copy_it_started_from() {
    let f = Fixture::new();
    let s = sign_in(&f);
    let id = |access: &str| {
        let sessions = f.host.web_sessions.lock().unwrap();
        crate::web_sessions::find(&sessions, access).unwrap().extra[crate::web_sessions::SIGN_IN_ID]
            .as_str()
            .unwrap()
            .to_owned()
    };
    // Every sign-in has its ID from the login on.
    let first = id(&s.access);
    assert_eq!(first.len(), 32);
    assert_ne!(first, id(&sign_in(&f).access), "one per sign-in");
    // A refresh that copied the session before the stream socket gave it a new ID (a session
    // saved before IDs existed) keeps the live session's ID.
    let mut previous = {
        let sessions = f.host.web_sessions.lock().unwrap();
        crate::web_sessions::find(&sessions, &s.access)
            .unwrap()
            .clone()
    };
    previous.extra.remove(crate::web_sessions::SIGN_IN_ID);
    let (access, ..) = f
        .host
        .new_web_session(
            "admin".into(),
            false,
            "test".into(),
            "127.0.0.1".into(),
            Some(previous),
        )
        .unwrap();
    assert_eq!(id(&access), first);
}

#[tokio::test]
async fn a_console_stream_socket_ends_when_its_device_is_disabled_or_it_opens_again() {
    let f = Fixture::new();
    configure(&f, json!({"webrtc_server": true}));
    let address = serve(&f, Console::default()).await;
    let s = sign_in(&f);
    let mut first = open(address, &s).await;
    // A second tab replaces the first: one connection per device.
    let mut second = open(address, &s).await;
    first.closed().await;
    assert_eq!(first.close_code, Some(crate::webrtc::server::REPLACED));
    assert_eq!(
        first.close_reason.as_deref(),
        Some("Opened in another tab or browser")
    );
    assert!(quiet(&mut second).await);
    let device = console_devices(&f).remove(0);
    crate::web::update_clients(
        &f.host,
        "POST",
        "/api/clients/update",
        &json!({"uuid": device.uuid, "enabled": false}),
    )
    .unwrap();
    second.expect("error").await;
    second.closed().await;
}
