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

/// The PC's Tailscale address, where Tailscale peers arrive.
const TAILNET_HOST: &str = "100.90.1.2:47991";
/// A connection from `peer` to the PC's address it reaches: the Tailscale one for a 100.64/10
/// peer, the LAN one otherwise.
fn connection(peer: &str) -> Connection {
    let tailscale = peer.starts_with("100.");
    connection_to(
        peer,
        if tailscale {
            TAILNET_HOST
        } else {
            "192.168.4.69:47990"
        },
    )
}
fn connection_to(peer: &str, local: &str) -> Connection {
    Connection {
        peer: peer.parse().unwrap(),
        local: local.parse().unwrap(),
        certificate: None,
        tls: true,
        second_screen: false,
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
    send_on(router, connection(peer), method, path, headers, body).await
}

/// A request on the guest port from `peer` to this PC's `local` address.
async fn guest_on(f: &Fixture, peer: &str, local: &str, method: &str, path: &str) -> Reply {
    send_on(
        guest_router(f.host.clone()),
        connection_to(peer, local),
        method,
        path,
        &[],
        None,
    )
    .await
}

async fn send_on(
    router: Router,
    connection: Connection,
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
        .layer(Extension(connection))
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
                second_screen: false,
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

/// The play page's build beside the console's and the guest page's.
fn install_play(f: &Fixture) {
    install_pages(f);
    let root = &f.host.assets;
    std::fs::create_dir_all(root.join("play/assets")).unwrap();
    std::fs::write(root.join("play/index.html"), "<p>play page</p>").unwrap();
    std::fs::write(root.join("play/assets/play-1.js"), "export {}").unwrap();
}

/// The headers the play page sends with its socket's upgrade.
fn play_upgrade() -> Vec<(&'static str, String)> {
    without("sec-websocket-protocol", Some("rubylight.play.v2"))
}

/// Peers of the local network: the LAN, Tailscale, a unique-local IPv6 address and this PC.
const LOCAL: [&str; 4] = [
    LAN,
    "100.101.102.103:50000",
    "[fd00::5]:50000",
    "127.0.0.1:50000",
];

#[tokio::test]
async fn the_play_page_is_its_own_build_for_the_local_network_and_the_guest_gate_stays() {
    let f = Fixture::new();
    install_play(&f);
    // Off with the WebRTC server.
    for path in ["/play/", "/play/signal"] {
        let off = guest(&f, LAN, "GET", path, &play_upgrade()).await;
        assert_eq!(off.status, StatusCode::NOT_FOUND, "{path}");
        assert!(!off.text().contains("play page"));
    }
    configure(&f, json!({"webrtc_server": true}));
    for peer in LOCAL {
        for path in [
            "/play",
            "/play/",
            "/play/index.html",
            "/play/library",
            "/play/../index.html",
            "/play/%2e%2e/index.html",
            "/play/..%5cindex.html",
            "/play/api/config",
            "/play/guest/",
        ] {
            let page = guest(&f, peer, "GET", path, &[]).await;
            assert_eq!(page.status, StatusCode::OK, "{peer} {path}");
            assert_eq!(page.text(), "<p>play page</p>", "{peer} {path}");
            assert_eq!(page.headers[header::CONTENT_SECURITY_POLICY], GUEST_POLICY);
            assert_eq!(page.headers["x-frame-options"], "DENY");
            assert_eq!(page.headers[header::REFERRER_POLICY], "no-referrer");
        }
        let script = guest(&f, peer, "GET", "/play/assets/play-1.js", &[]).await;
        assert_eq!(script.status, StatusCode::OK);
        for path in [
            "/play/assets/guest-1.js",
            "/play/assets/console-1.js",
            "/play/assets/../../index.html",
            "/play/assets/..%5c..%5cguest%5cassets%5cguest-1.js",
        ] {
            let gone = guest(&f, peer, "GET", path, &[]).await;
            assert_eq!(gone.status, StatusCode::NOT_FOUND, "{peer} {path}");
        }
        let posted = guest(&f, peer, "POST", "/play/", &[]).await;
        assert_eq!(posted.status, StatusCode::METHOD_NOT_ALLOWED);
        // The guest gate is as before: with no link, nothing else answers, the console least.
        for path in [
            "/",
            "/index.html",
            "/guest/",
            "/guest/signal",
            "/api/config",
            "/playground",
            "/stream",
        ] {
            let closed = guest(&f, peer, "GET", path, &[]).await;
            assert_eq!(closed.status, StatusCode::NOT_FOUND, "{peer} {path}");
            assert!(!closed.text().contains("page"), "{peer} {path}");
        }
    }
    // Once a link is live, the guest routes are the guest page's alone.
    open_guests(&f);
    assert_eq!(
        guest(&f, LAN, "GET", "/guest/", &[]).await.text(),
        "<p>guest page</p>"
    );
    assert_eq!(
        guest(&f, LAN, "GET", "/play/", &[]).await.text(),
        "<p>play page</p>"
    );
}

#[tokio::test]
async fn an_internet_peer_gets_no_play_page_or_socket_whatever_its_headers_say() {
    let f = Fixture::new();
    configure(&f, json!({"webrtc_server": true}));
    install_play(&f);
    // A live link opens the guest port to the internet, never /play.
    open_guests(&f);
    let spoofed: [Vec<(&str, String)>; 5] = [
        vec![],
        vec![("x-forwarded-for", "192.168.4.20".into())],
        vec![("x-real-ip", "127.0.0.1".into())],
        vec![("forwarded", "for=192.168.4.20;proto=https".into())],
        vec![
            ("x-forwarded-for", "127.0.0.1, 192.168.4.20".into()),
            ("x-client-ip", "192.168.4.20".into()),
            ("true-client-ip", "192.168.4.20".into()),
        ],
    ];
    // A router forwarding TCP 47991 hands the internet peer's address to the LAN socket as it is.
    // One that source-NATs its forwards hands over its own, the default gateway; a neighbour
    // behind the ISP's carrier-grade NAT arrives from 100.64/10 on the LAN address.
    let gateway = format!("{}:50000", crate::network::TEST_GATEWAY);
    let peers = [
        (WAN, "192.168.4.69:47991"),
        ("[2001:db8::7]:50000", "192.168.4.69:47991"),
        ("8.8.8.8:443", "192.168.4.69:47991"),
        (gateway.as_str(), "192.168.4.69:47991"),
        ("100.101.102.103:50000", "192.168.4.69:47991"),
        ("100.64.0.9:50000", "[::ffff:192.168.4.69]:47991"),
    ];
    // The guest port's answer to a path it does not serve: what the internet gets for /play too.
    let unserved = guest(&f, WAN, "GET", "/nothing", &[]).await;
    assert_eq!(unserved.status, StatusCode::NOT_FOUND);
    for (peer, local) in peers {
        for headers in &spoofed {
            let mut upgrade = play_upgrade();
            upgrade.extend(headers.iter().cloned());
            for (method, path, headers) in [
                ("GET", "/play", headers),
                ("GET", "/play/", headers),
                ("GET", "/play/index.html", headers),
                ("GET", "/play/assets/play-1.js", headers),
                ("POST", "/play/signal", headers),
                ("GET", "/play/signal", &upgrade),
            ] {
                let refused = send_on(
                    guest_router(f.host.clone()),
                    connection_to(peer, local),
                    method,
                    path,
                    headers,
                    None,
                )
                .await;
                assert_eq!(
                    (refused.status, refused.text()),
                    (unserved.status, unserved.text()),
                    "{peer} on {local} {method} {path} {headers:?}"
                );
            }
        }
    }
    // Real Tailscale traffic arrives on the PC's Tailscale address.
    let tailnet = guest_on(&f, "100.101.102.103:50000", TAILNET_HOST, "GET", "/play/").await;
    assert_eq!(tailnet.text(), "<p>play page</p>");
    // A LAN peer is not refused for a header naming the internet either.
    let page = guest(
        &f,
        LAN,
        "GET",
        "/play/",
        &[("x-forwarded-for", "203.0.113.7".into())],
    )
    .await;
    assert_eq!(page.text(), "<p>play page</p>");
    // The guest page still answers the internet with its link.
    assert_eq!(
        guest(&f, WAN, "GET", "/guest/", &[]).await.text(),
        "<p>guest page</p>"
    );
}

#[tokio::test]
async fn with_no_link_open_the_internet_gets_the_closed_guest_port_for_play_too() {
    let f = Fixture::new();
    configure(&f, json!({"webrtc_server": true}));
    install_play(&f);
    let closed = guest(&f, WAN, "GET", "/", &[]).await;
    assert_eq!(closed.status, StatusCode::NOT_FOUND);
    for (peer, local) in [
        (WAN, "192.168.4.69:47991"),
        ("100.101.102.103:50000", "192.168.4.69:47991"),
        ("192.168.4.1:50000", "192.168.4.69:47991"),
    ] {
        for path in ["/play", "/play/", "/play/signal"] {
            let refused = guest_on(&f, peer, local, "GET", path).await;
            assert_eq!(
                (refused.status, refused.text()),
                (closed.status, closed.text()),
                "{peer} {path}"
            );
        }
    }
    assert_eq!(
        guest(&f, LAN, "GET", "/play/", &[]).await.text(),
        "<p>play page</p>"
    );
}

#[tokio::test]
async fn a_play_pairing_request_may_not_carry_its_pin_in_the_name_and_shows_its_address() {
    let f = Fixture::new();
    configure(&f, json!({"webrtc_server": true}));
    let address = serve(&f).await;
    let (mut ws, _) = play_socket(address, "").await;
    // More refusals than the five requests a minute a device may make: a name to fix uses none.
    for (name, message) in [
        ("Living room TV - PIN 4821", "PIN"),
        ("4821", "PIN"),
        ("TV 4-8-2-1", "PIN"),
        ("TV \u{ff14}\u{ff18}\u{ff12}\u{ff11}", "0-9"),
        ("Xbox, PIN ④⑧②①", "0-9"),
        ("TV \u{0664}\u{0668}\u{0662}\u{0661}", "0-9"),
        ("TV \u{1d7d2}\u{1d7d6}\u{1d7d0}\u{1d7cf}", "0-9"),
    ] {
        ws.send(
            json!({"version": 2, "type": "request-pairing", "pin": "4821", "clientName": name}),
        )
        .await;
        let refused = ws.expect("error").await;
        assert_eq!(refused["code"], "invalid-message", "{name}");
        assert!(
            refused["message"].as_str().unwrap().contains(message),
            "{refused}"
        );
    }
    let pending =
        crate::web::update_clients(&f.host, "GET", "/api/clients/pending", &Value::Null).unwrap();
    assert_eq!(pending["requests"], json!([]));
    // A name with other digits pairs; the console lists it with the address it came from.
    ws.send(json!({"version": 2, "type": "request-pairing", "pin": "4821", "clientName": "Living room TV 2024"}))
        .await;
    let mut request = Value::Null;
    for _ in 0..200 {
        let pending =
            crate::web::update_clients(&f.host, "GET", "/api/clients/pending", &Value::Null)
                .unwrap();
        if let Some(first) = pending["requests"].as_array().and_then(|r| r.first()) {
            request = first.clone();
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(request["name"], "Living room TV 2024", "{request}");
    assert_eq!(request["address"], "127.0.0.1", "{request}");
}

#[test]
fn a_pairing_notice_shows_the_address_apart_from_a_name_that_could_hide_or_fake_it() {
    let lan = "192.168.4.20".parse().unwrap();
    assert_eq!(
        crate::state::pairing_notice("Living room TV 2024", lan),
        (
            "Pairing request from 192.168.4.20".into(),
            "Living room TV 2024 wants to pair. Enter the PIN it shows in the Rubylight console."
                .into()
        )
    );
    // 128 math bold letters are 256 UTF-16 units, more than Windows keeps of a notice's text.
    let long: String = std::iter::repeat_n('\u{1d400}', 128).collect();
    butterpollo_core::pairing::validate_device_name(&long).unwrap();
    let (title, text) = crate::state::pairing_notice(&long, lan);
    assert_eq!(title, "Pairing request from 192.168.4.20");
    assert!(text.encode_utf16().count() < 256, "{text}");
    assert!(text.starts_with(&format!("{}…", "\u{1d400}".repeat(40))));
    // A name that names an address does not change the one in the title.
    let (title, text) =
        crate::state::pairing_notice("Living room TV (from 192.168.4.20)", "::1".parse().unwrap());
    assert_eq!(title, "Pairing request from this PC (a tunnel or proxy)");
    assert!(!title.contains("192.168"), "{title}");
    assert!(text.starts_with("Living room TV (from 192.168.4.20) wants"));
    // The longest address still fits Windows' 63-unit title.
    let (title, _) = crate::state::pairing_notice(
        "TV",
        "fd12:3456:789a:bcde:f012:3456:789a:bcde".parse().unwrap(),
    );
    assert!(title.encode_utf16().count() <= 63, "{title}");
    // A request from the internet asks the owner to expect it; the local network's does not.
    for address in ["203.0.113.7", "2001:db8::7", "127.77.3.4"] {
        let (_, text) = crate::state::pairing_notice("Phone", address.parse().unwrap());
        assert_eq!(
            text,
            "Phone wants to pair from the internet. Enter its PIN in the Rubylight console only if you expect it.",
            "{address}"
        );
    }
    for address in ["192.168.4.20", "100.101.102.103", "fd12::7", "::1"] {
        let (_, text) = crate::state::pairing_notice("Phone", address.parse().unwrap());
        assert!(!text.contains("internet"), "{address}: {text}");
    }
}

#[tokio::test]
async fn the_play_socket_needs_its_page_origin_and_subprotocol() {
    let f = Fixture::new();
    configure(&f, json!({"webrtc_server": true}));
    let with = |name: &'static str, value: Option<&str>| {
        let mut headers = play_upgrade();
        headers.retain(|(n, _)| *n != name);
        if let Some(value) = value {
            headers.push((name, value.to_owned()));
        }
        headers
    };
    for (headers, status) in [
        (with("origin", None), StatusCode::FORBIDDEN),
        (
            with("origin", Some("https://evil.example")),
            StatusCode::FORBIDDEN,
        ),
        (with("origin", Some("http://test")), StatusCode::FORBIDDEN),
        (
            with("sec-websocket-protocol", None),
            StatusCode::BAD_REQUEST,
        ),
        (
            with("sec-websocket-protocol", Some("rubylight.guest.v1")),
            StatusCode::BAD_REQUEST,
        ),
        (
            with(
                "sec-websocket-protocol",
                Some("rubylight.signal.v2, csrf.x"),
            ),
            StatusCode::BAD_REQUEST,
        ),
        (play_upgrade(), StatusCode::UPGRADE_REQUIRED),
    ] {
        let reply = guest(&f, LAN, "GET", "/play/signal", &headers).await;
        assert_eq!(reply.status, status, "{headers:?}: {}", reply.text());
    }
}

async fn play_socket(address: SocketAddr, extra: &str) -> (Ws, String) {
    let mut ws = Ws::connect_with(
        address,
        "/play/signal",
        &format!("Origin: https://test\r\nSec-WebSocket-Protocol: rubylight.play.v2\r\n{extra}"),
    )
    .await;
    assert!(
        ws.head
            .to_ascii_lowercase()
            .contains("\r\nsec-websocket-protocol: rubylight.play.v2\r\n"),
        "{}",
        ws.head
    );
    let nonce = ws.expect("auth-required").await["nonce"]
        .as_str()
        .unwrap()
        .to_owned();
    (ws, nonce)
}

fn proof(secret: &str, nonce: &str) -> String {
    butterpollo_core::webrtc::auth::proof_hex(secret, nonce).unwrap()
}

#[tokio::test]
async fn an_unpaired_play_socket_can_only_pair_and_unpairing_ends_its_access_at_once() {
    let f = Fixture::new();
    configure(&f, json!({"webrtc_server": true}));
    let address = serve(&f).await;
    let (cookie, _) = sign_in(&f);
    // Before pairing, even with a console sign-in in the request, nothing but pairing works.
    let (mut ws, nonce) = play_socket(address, &format!("Cookie: {cookie}\r\n")).await;
    for message in [
        json!({"version": 2, "type": "get-apps"}),
        json!({"version": 2, "type": "get-app-artwork", "appId": "1"}),
        json!({"version": 2, "type": "start-session", "appId": "1", "video": {"width": 1920, "height": 1080, "fps": 60, "codec": "h264", "bitrateKbps": 20000, "hdr": false}, "audio": {"channels": 2}}),
        json!({"version": 2, "type": "stop-host-session"}),
        json!({"version": 2, "type": "answer", "sessionId": 1, "sdp": "v=0"}),
    ] {
        ws.send(message.clone()).await;
        let refused = ws.expect("error").await;
        assert_eq!(refused["code"], "not-authenticated", "{message}");
    }
    // A guest's join is no message here.
    ws.send(json!({"type": "join", "token": "0".repeat(32)}))
        .await;
    assert_ne!(ws.expect("error").await["code"], "not-authenticated");
    ws.send(json!({"version": 2, "type": "authenticate", "clientId": "ab".repeat(16), "proof": proof(&"cd".repeat(32), &nonce)}))
        .await;
    assert_eq!(ws.expect("error").await["code"], "authentication-failed");
    // The console confirms the PIN the page shows.
    ws.send(json!({"version": 2, "type": "request-pairing", "pin": "2468", "clientName": "Xbox browser"}))
        .await;
    let mut request = Value::Null;
    for _ in 0..200 {
        let pending =
            crate::web::update_clients(&f.host, "GET", "/api/clients/pending", &Value::Null)
                .unwrap();
        if let Some(first) = pending["requests"].as_array().and_then(|r| r.first()) {
            request = first.clone();
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(request["webrtc"], true, "{request}");
    assert_eq!(request["name"], "Xbox browser");
    crate::web::update_clients(
        &f.host,
        "POST",
        "/api/pin",
        &json!({"pin": "2468", "uniqueid": request["uniqueid"]}),
    )
    .unwrap();
    let paired = ws.expect("paired").await;
    ws.expect("gateway-status").await;
    ws.expect("capabilities").await;
    let client_id = paired["clientId"].as_str().unwrap().to_owned();
    let secret = paired["clientSecret"].as_str().unwrap().to_owned();
    // Paired, it gets what a TV gets: the library (this test host has no stream pipeline).
    ws.send(json!({"version": 2, "type": "get-apps"})).await;
    ws.expect("apps").await;
    drop(ws);
    // It signs in again with its secret; unpaired in the console, it is dropped at once.
    let (mut ws, nonce) = play_socket(address, "").await;
    ws.send(json!({"version": 2, "type": "authenticate", "clientId": client_id, "proof": proof(&secret, &nonce)}))
        .await;
    ws.expect("authenticated").await;
    ws.expect("gateway-status").await;
    ws.expect("capabilities").await;
    let uuid = f.host.paired.read().unwrap().clients[0].uuid.clone();
    crate::web::update_clients(
        &f.host,
        "POST",
        "/api/clients/unpair",
        &json!({"uuid": uuid}),
    )
    .unwrap();
    assert_eq!(ws.expect("error").await["code"], "authentication-failed");
    ws.closed().await;
    let (mut ws, nonce) = play_socket(address, "").await;
    ws.send(json!({"version": 2, "type": "authenticate", "clientId": client_id, "proof": proof(&secret, &nonce)}))
        .await;
    assert_eq!(ws.expect("error").await["code"], "authentication-failed");
    ws.send(json!({"version": 2, "type": "get-apps"})).await;
    assert_eq!(ws.expect("error").await["code"], "not-authenticated");
}
