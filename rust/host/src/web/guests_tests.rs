//! The guest port and the Stream page's guest panel: a guest reaches its page and its socket
//! and nothing of the console's, the guest port answers only while a link is live, and only a
//! signed-in console tab makes, lists or withdraws links.
use super::*;
use crate::state::test_support::Fixture;
use crate::webrtc::server::tests::Ws;
use axum::body::{Body, to_bytes};
use butterpollo_core::webrtc::guests::Permission;
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
    service::TowerToHyperService,
};
use std::net::SocketAddr;
use std::time::Duration;
use tower::ServiceExt;

/// A peer on the LAN and one on the internet.
const LAN: &str = "192.168.4.20:50000";
const WAN: &str = "203.0.113.7:50000";

fn connection(peer: &str) -> Connection {
    Connection {
        peer: peer.parse().unwrap(),
        local: "192.168.4.69:47990".parse().unwrap(),
        certificate: None,
        tls: true,
    }
}

/// A console tab signed in as "admin": its session cookie and CSRF token.
fn sign_in(f: &Fixture) -> (String, String) {
    f.host
        .save_credentials(&Credentials::new("admin".into(), "guests-test").unwrap())
        .unwrap();
    let (access, _, csrf, _) = f
        .host
        .new_web_session(
            "admin".into(),
            false,
            "test".into(),
            "192.168.4.20".into(),
            None,
        )
        .unwrap();
    (format!("__Host-apollo_session={access}"), csrf)
}

fn configure(f: &Fixture, settings: Value) {
    f.host
        .config
        .write()
        .unwrap()
        .update(settings.as_object().unwrap())
        .unwrap();
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_default()
    }
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    fn error(&self) -> String {
        self.json()["error"].as_str().unwrap_or_default().to_owned()
    }
}

/// A request to the console's port.
async fn call(
    f: &Fixture,
    peer: &str,
    method: &str,
    path: &str,
    headers: &[(&str, String)],
    body: Option<Value>,
) -> Reply {
    send(router(f.host.clone()), peer, method, path, headers, body).await
}

/// A request to the guest port.
async fn guest(
    f: &Fixture,
    peer: &str,
    method: &str,
    path: &str,
    headers: &[(&str, String)],
) -> Reply {
    send(
        guest_router(f.host.clone()),
        peer,
        method,
        path,
        headers,
        None,
    )
    .await
}

/// A live link, so the guest port answers.
fn open_guests(f: &Fixture) -> String {
    let (invite, _) = f
        .host
        .guests
        .create(Permission::View, Duration::from_secs(300), None)
        .unwrap();
    invite.id
}

async fn send(
    router: Router,
    peer: &str,
    method: &str,
    path: &str,
    headers: &[(&str, String)],
    body: Option<Value>,
) -> Reply {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "test");
    for (name, value) in headers {
        request = request.header(*name, value);
    }
    let body = match body {
        Some(body) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    let response = router
        .layer(Extension(connection(peer)))
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap()
        .to_vec();
    Reply {
        status,
        headers,
        body,
    }
}

/// What the console tab sends with a POST.
fn console(cookie: &str, csrf: &str) -> Vec<(&'static str, String)> {
    vec![
        ("cookie", cookie.to_owned()),
        ("x-csrf-token", csrf.to_owned()),
        ("origin", "https://test".into()),
    ]
}

async fn invite(f: &Fixture, cookie: &str, csrf: &str, body: Value) -> Reply {
    call(
        f,
        LAN,
        "POST",
        "/api/guests/invite",
        &console(cookie, csrf),
        Some(body),
    )
    .await
}

#[tokio::test]
async fn only_a_signed_in_console_tab_reaches_the_guest_panel() {
    let f = Fixture::new();
    let (cookie, csrf) = sign_in(&f);
    let body = json!({"permission": "controller", "seconds": 3600});

    let anonymous = call(&f, LAN, "GET", "/api/guests", &[], None).await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    let anonymous = call(
        &f,
        LAN,
        "POST",
        "/api/guests/invite",
        &[],
        Some(body.clone()),
    )
    .await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    // An API token is automation: the panel is not in its catalogue.
    assert!(
        !token_catalog()
            .iter()
            .any(|scope| scope.path.starts_with("/api/guests"))
    );
    let bearer = call(
        &f,
        LAN,
        "GET",
        "/api/guests",
        &[("authorization", "Bearer 0123456789abcdef".into())],
        None,
    )
    .await;
    assert_eq!(bearer.status, StatusCode::UNAUTHORIZED);

    // Signed in, a POST needs the session's CSRF token and the console's own origin.
    let mut headers = console(&cookie, &csrf);
    headers.retain(|(name, _)| *name != "x-csrf-token");
    let forged = call(
        &f,
        LAN,
        "POST",
        "/api/guests/invite",
        &headers,
        Some(body.clone()),
    )
    .await;
    assert_eq!(forged.status, StatusCode::BAD_REQUEST);
    assert_eq!(forged.error(), "CSRF token required");
    let mut headers = console(&cookie, &csrf);
    headers.retain(|(name, _)| *name != "origin");
    headers.push(("origin", "https://evil.example".into()));
    let cross = call(
        &f,
        LAN,
        "POST",
        "/api/guests/invite",
        &headers,
        Some(body.clone()),
    )
    .await;
    assert_eq!(cross.status, StatusCode::FORBIDDEN);
    assert!(f.host.guests.status().invites.is_empty());

    let made = invite(&f, &cookie, &csrf, body).await;
    assert_eq!(made.status, StatusCode::OK, "{}", made.text());
    let token = made.json()["token"].as_str().unwrap().to_owned();
    assert_eq!(token.len(), 32);
    let listed = call(&f, LAN, "GET", "/api/guests", &[("cookie", cookie)], None).await;
    assert_eq!(listed.status, StatusCode::OK);
    assert_eq!(listed.json()["invites"][0]["permission"], "controller");
    assert_eq!(listed.json()["streaming"], false);
    assert!(
        !listed.text().contains(&token),
        "the list never has a token"
    );
}

#[tokio::test]
async fn the_panel_makes_and_revokes_links_and_says_what_it_refuses() {
    let f = Fixture::new();
    let (cookie, csrf) = sign_in(&f);
    for (body, status) in [
        (
            json!({"permission": "admin", "seconds": 3600}),
            StatusCode::BAD_REQUEST,
        ),
        (json!({"seconds": 3600}), StatusCode::BAD_REQUEST),
        (json!({"permission": "view"}), StatusCode::BAD_REQUEST),
        (
            json!({"permission": "view", "seconds": 3600, "address": "https://home"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"permission": "view", "seconds": 3600, "address": "home/x:1"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"permission": "view", "seconds": 3600, "address": 7}),
            StatusCode::BAD_REQUEST,
        ),
        // Five minutes to seven days.
        (
            json!({"permission": "view", "seconds": 60}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"permission": "view", "seconds": 8 * 86_400}),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let refused = invite(&f, &cookie, &csrf, body.clone()).await;
        assert_eq!(refused.status, status, "{body}");
        assert!(!refused.error().is_empty(), "{body}");
    }
    let mut ids = Vec::new();
    let addresses = [
        json!(null),
        json!("home.example.org"),
        json!("203.0.113.9:47991"),
        json!("[2001:db8::1]:47991"),
    ];
    for n in 0..butterpollo_core::webrtc::guests::MAX_INVITES {
        let made = invite(
            &f,
            &cookie,
            &csrf,
            json!({"permission": "view", "seconds": 300, "address": addresses[n % 4]}),
        )
        .await;
        assert_eq!(made.status, StatusCode::OK, "{}", made.text());
        assert_eq!(made.json()["invite"]["permission"], "view");
        ids.push(made.json()["invite"]["id"].as_str().unwrap().to_owned());
    }
    let full = invite(
        &f,
        &cookie,
        &csrf,
        json!({"permission": "view", "seconds": 300}),
    )
    .await;
    assert_eq!(full.status, StatusCode::CONFLICT);
    assert!(
        full.error().contains("revoke one first"),
        "{}",
        full.error()
    );

    let (host, headers) = (&f, console(&cookie, &csrf));
    let post = move |path: &'static str, body: Value| {
        let headers = headers.clone();
        async move { call(host, LAN, "POST", path, &headers, Some(body)).await }
    };
    let revoked = post("/api/guests/revoke", json!({"id": ids[0]})).await;
    assert_eq!(revoked.status, StatusCode::OK, "{}", revoked.text());
    assert!(
        f.host
            .guests
            .status()
            .invites
            .iter()
            .all(|i| i.id != ids[0])
    );
    let again = post("/api/guests/revoke", json!({"id": ids[0]})).await;
    assert_eq!(again.status, StatusCode::NOT_FOUND);
    // The panel builds links on the guest port, never the console's.
    let listed = call(
        &f,
        LAN,
        "GET",
        "/api/guests",
        &console(&cookie, &csrf),
        None,
    )
    .await;
    assert_eq!(listed.json()["port"], 47991, "{}", listed.text());
    let lan = call(
        &f,
        LAN,
        "GET",
        "/api/guests/addresses",
        &console(&cookie, &csrf),
        None,
    )
    .await;
    assert!(lan.json()["addresses"].is_array(), "{}", lan.text());
    assert_eq!(
        post("/api/guests/revoke", json!({})).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post("/api/guests/kick", json!({"id": 7})).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        post("/api/guests/kick", json!({"id": "7"})).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post("/api/guests/nothing", json!({})).await.status,
        StatusCode::BAD_REQUEST
    );
}

/// A console build and a guest build in the host's assets.
fn install_pages(f: &Fixture) {
    let root = &f.host.assets;
    std::fs::create_dir_all(root.join("guest/assets")).unwrap();
    std::fs::write(root.join("index.html"), "<p>console page</p>").unwrap();
    std::fs::write(root.join("guest/index.html"), "<p>guest page</p>").unwrap();
    std::fs::write(root.join("guest/assets/guest-1.js"), "export {}").unwrap();
}

#[tokio::test]
async fn the_guest_port_answers_only_while_a_link_is_live() {
    let f = Fixture::new();
    configure(&f, json!({"webrtc_server": true}));
    install_pages(&f);
    for path in ["/guest/", "/guest/index.html", "/guest/signal", "/"] {
        let closed = guest(&f, WAN, "GET", path, &guest_upgrade()).await;
        assert_eq!(closed.status, StatusCode::NOT_FOUND, "{path}");
        assert!(!closed.text().contains("guest page"), "{path}");
    }
    let id = open_guests(&f);
    assert_eq!(
        guest(&f, WAN, "GET", "/guest/", &[]).await.text(),
        "<p>guest page</p>"
    );
    // The page's tab icon, and the one a browser asks for itself (404 in 3.4.0).
    for path in ["/guest/favicon.svg", "/favicon.ico"] {
        let icon = guest(&f, WAN, "GET", path, &[]).await;
        assert_eq!(icon.status, StatusCode::OK, "{path}");
        assert_eq!(
            icon.headers[header::CONTENT_TYPE],
            "image/svg+xml",
            "{path}"
        );
        assert!(icon.text().starts_with("<svg"), "{path}");
    }
    f.host.guests.revoke(&id);
    for path in ["/guest/", "/guest/favicon.svg", "/favicon.ico"] {
        assert_eq!(
            guest(&f, WAN, "GET", path, &[]).await.status,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
}

#[tokio::test]
async fn the_guest_page_is_its_own_build_and_never_the_consoles() {
    let f = Fixture::new();
    open_guests(&f);
    let missing = guest(&f, WAN, "GET", "/guest/", &[]).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    install_pages(&f);
    for path in [
        "/guest",
        "/guest/",
        "/guest/index.html",
        "/guest/join",
        "/guest/../index.html",
        "/guest/%2e%2e/index.html",
        "/guest/api/config",
        "/guest/..%5cindex.html",
    ] {
        let page = guest(&f, WAN, "GET", path, &[]).await;
        assert_eq!(page.status, StatusCode::OK, "{path}");
        assert_eq!(page.text(), "<p>guest page</p>", "{path}");
        assert_eq!(page.headers[header::CONTENT_SECURITY_POLICY], GUEST_POLICY);
        assert_eq!(page.headers["x-frame-options"], "DENY");
        assert_eq!(page.headers["x-content-type-options"], "nosniff");
        assert_eq!(page.headers[header::REFERRER_POLICY], "no-referrer");
        assert_eq!(page.headers[header::CACHE_CONTROL], "no-store");
    }
    let script = guest(&f, WAN, "GET", "/guest/assets/guest-1.js", &[]).await;
    assert_eq!(script.status, StatusCode::OK);
    assert!(
        script.headers[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/javascript")
    );
    let gone = guest(&f, WAN, "GET", "/guest/assets/console-1.js", &[]).await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
    let posted = guest(&f, LAN, "POST", "/guest/", &[]).await;
    assert_eq!(posted.status, StatusCode::METHOD_NOT_ALLOWED);
    // Nothing but the guest routes, even from this PC: no console page, file or API.
    for path in [
        "/",
        "/index.html",
        "/stream",
        "/api/config",
        "/api/guests",
        "/pin",
        "/serverinfo",
    ] {
        for peer in [WAN, LAN, "127.0.0.1:50000"] {
            let other = guest(&f, peer, "GET", path, &[]).await;
            assert_eq!(other.status, StatusCode::NOT_FOUND, "{peer} {path}");
            assert!(other.text().is_empty(), "{peer} {path}: {}", other.text());
        }
    }
    // And the console's port no longer serves the guest page or its socket.
    let console_page = call(&f, LAN, "GET", "/guest/", &[], None).await;
    assert_ne!(console_page.text(), "<p>guest page</p>");
    // The guest page loads its own files and talks to this host only.
    assert!(GUEST_POLICY.contains("connect-src 'self';"));
    assert!(GUEST_POLICY.contains("script-src 'self';"));
    assert!(!GUEST_POLICY.contains("http"));
}

/// The headers a guest page sends with its socket's upgrade.
fn guest_upgrade() -> Vec<(&'static str, String)> {
    vec![
        ("connection", "upgrade".into()),
        ("upgrade", "websocket".into()),
        ("sec-websocket-version", "13".into()),
        ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==".into()),
        ("origin", "https://test".into()),
        ("sec-websocket-protocol", "rubylight.guest.v1".into()),
    ]
}

fn without(name: &str, value: Option<&str>) -> Vec<(&'static str, String)> {
    let mut headers = guest_upgrade();
    let key = headers.iter().find(|(n, _)| *n == name).unwrap().0;
    headers.retain(|(n, _)| *n != name);
    if let Some(value) = value {
        headers.push((key, value.to_owned()));
    }
    headers
}

#[tokio::test]
async fn an_internet_guest_reaches_the_guest_routes_and_no_console_route() {
    let f = Fixture::new();
    configure(&f, json!({"webrtc_server": true}));
    install_pages(&f);
    let (cookie, csrf) = sign_in(&f);
    // The console stays on the LAN (origin_web_ui_allowed = lan), even with a sign-in.
    for (method, path) in [
        ("GET", "/"),
        ("GET", "/stream"),
        ("GET", "/api/guests"),
        ("GET", "/api/config"),
        ("GET", "/api/auth/status"),
        ("POST", "/api/guests/invite"),
    ] {
        let refused = call(&f, WAN, method, path, &console(&cookie, &csrf), None).await;
        assert_eq!(refused.status, StatusCode::FORBIDDEN, "{method} {path}");
    }
    open_guests(&f);
    let page = guest(&f, WAN, "GET", "/guest/", &[]).await;
    assert_eq!(page.text(), "<p>guest page</p>");
    // On the console's port a guest's upgrade is a console request, refused from the internet.
    let console_socket = call(&f, WAN, "GET", "/guest/signal", &guest_upgrade(), None).await;
    assert_eq!(console_socket.status, StatusCode::FORBIDDEN);
    // A oneshot request cannot be upgraded: one that passes every check ends in 426.
    let socket = guest(&f, WAN, "GET", "/guest/signal", &guest_upgrade()).await;
    assert_eq!(
        socket.status,
        StatusCode::UPGRADE_REQUIRED,
        "{}",
        socket.text()
    );
}

#[tokio::test]
async fn the_guest_socket_needs_its_page_origin_its_subprotocol_and_the_server_on() {
    let f = Fixture::new();
    open_guests(&f);
    let host = &f;
    let socket = move |headers: Vec<(&'static str, String)>| async move {
        guest(host, WAN, "GET", "/guest/signal", &headers).await
    };
    let off = socket(guest_upgrade()).await;
    assert_eq!(off.status, StatusCode::CONFLICT);
    configure(&f, json!({"webrtc_server": true}));
    for (headers, status) in [
        (without("origin", None), StatusCode::FORBIDDEN),
        (
            without("origin", Some("https://evil.example")),
            StatusCode::FORBIDDEN,
        ),
        (
            without("origin", Some("http://test")),
            StatusCode::FORBIDDEN,
        ),
        (
            without("sec-websocket-protocol", None),
            StatusCode::BAD_REQUEST,
        ),
        // The console's own socket protocol opens nothing here.
        (
            without(
                "sec-websocket-protocol",
                Some("rubylight.signal.v2, csrf.x"),
            ),
            StatusCode::BAD_REQUEST,
        ),
        (guest_upgrade(), StatusCode::UPGRADE_REQUIRED),
    ] {
        let reply = socket(headers.clone()).await;
        assert_eq!(reply.status, status, "{headers:?}: {}", reply.text());
    }
}

/// The guest port on a real socket, as `tls::serve` runs it (without TLS).
async fn serve(f: &Fixture) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = listener.local_addr().unwrap();
    let router = guest_router(f.host.clone());
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

async fn guest_socket(address: SocketAddr, extra: &str) -> Ws {
    let ws = Ws::connect_with(
        address,
        "/guest/signal",
        &format!("Origin: https://test\r\nSec-WebSocket-Protocol: rubylight.guest.v1\r\n{extra}"),
    )
    .await;
    assert!(
        ws.head
            .to_ascii_lowercase()
            .contains("\r\nsec-websocket-protocol: rubylight.guest.v1\r\n"),
        "{}",
        ws.head
    );
    ws
}

/// The one message the host ends a refused guest's socket with, and the close after it.
async fn refused(ws: &mut Ws) -> String {
    let message = ws.next().await.expect("the host's reason");
    assert_eq!(message["type"], "error", "{message}");
    assert!(!message["message"].as_str().unwrap_or_default().is_empty());
    ws.closed().await;
    message["code"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn the_guest_socket_speaks_only_its_own_messages_even_with_a_console_sign_in() {
    let f = Fixture::new();
    configure(&f, json!({"webrtc_server": true}));
    open_guests(&f);
    let address = serve(&f).await;
    let (cookie, csrf) = sign_in(&f);
    // A right link while the owner is not streaming: the page is told to wait.
    let made = invite(
        &f,
        &cookie,
        &csrf,
        json!({"permission": "view", "seconds": 300}),
    )
    .await;
    let token = made.json()["token"].as_str().unwrap().to_owned();
    let mut ws = guest_socket(address, "").await;
    ws.send(json!({"type": "join", "token": token, "name": "Sam"}))
        .await;
    assert_eq!(refused(&mut ws).await, "not-streaming");
    // Withdrawn, the same link opens nothing.
    let id = made.json()["invite"]["id"].as_str().unwrap().to_owned();
    let revoked = call(
        &f,
        LAN,
        "POST",
        "/api/guests/revoke",
        &console(&cookie, &csrf),
        Some(json!({"id": id})),
    )
    .await;
    assert_eq!(revoked.status, StatusCode::OK);
    let mut ws = guest_socket(address, "").await;
    ws.send(json!({"type": "join", "token": token})).await;
    assert_eq!(refused(&mut ws).await, "invalid-link");

    // The console's messages, from a tab that is signed in to it, get nothing.
    let signed_in = format!("Cookie: {cookie}\r\n");
    for message in [
        json!({"version": 2, "type": "get-apps"}),
        json!({"version": 2, "type": "start-session", "appId": "1"}),
        json!({"type": "answer", "sdp": "v=0"}),
        json!({"type": "join"}),
    ] {
        let mut ws = guest_socket(address, &signed_in).await;
        ws.send(message.clone()).await;
        assert_eq!(refused(&mut ws).await, "invalid-message", "{message}");
    }
    // Each counted as a wrong token, and with the withdrawn link's that is five from this
    // address: its next socket waits out the limit before anything is looked at.
    let mut ws = guest_socket(address, "").await;
    assert_eq!(refused(&mut ws).await, "rate-limited");
}
