//! WebRTC console/guest/play signalling routes, guest port gate, and static web app serving.
use super::{
    auth::{cookie, origin_allowed},
    error,
};
use axum::{
    Extension,
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use butterpollo_crypto::crypto;
use butterpollo_stream::{
    network,
    state::Shared,
    tls::Connection,
    web_sessions,
    webrtc_bridge::{
        CONSOLE_PROTOCOL, ConsoleSignIn, GUEST_PROTOCOL, PLAY_PROTOCOL, WebRtcWebService,
    },
};
use serde_json::Value;
use std::{sync::Arc, time::Instant};

/// `GET /api/webrtc/signal`: WebRTC signalling for the console's own stream page, a WebSocket on
/// the console's HTTPS listener. Only a browser signed in to the console gets it: the session
/// cookie (not Basic credentials or API tokens), the console's origin and the session's CSRF
/// token as the subprotocol `csrf.<token>` next to `rubylight.signal.v2`, as a cookie also goes
/// out on a cross-site upgrade. The browser streams as the WebRTC device "Console (<username>)".
pub(super) async fn webrtc_signal(
    State(h): State<Shared>,
    Extension(connection): Extension<Connection>,
    Extension(webrtc): Extension<Option<Arc<dyn WebRtcWebService>>>,
    headers: HeaderMap,
    upgrade: Result<
        axum::extract::WebSocketUpgrade,
        axum::extract::ws::rejection::WebSocketUpgradeRejection,
    >,
) -> Response {
    let signed_in = cookie(&headers, "__Host-apollo_session").and_then(|token| {
        let mut sessions = h.web_sessions.lock().unwrap();
        let key = web_sessions::resolve_hash(&sessions, &token)?;
        let session = sessions
            .get_mut(&key)
            .filter(|s| s.expires > Instant::now())?;
        // A sign-in gets its ID at login; a session saved before that gets one now.
        let id = session
            .extra
            .entry(web_sessions::SIGN_IN_ID)
            .or_insert_with(|| hex::encode(crypto::random::<16>()).into())
            .as_str()?
            .to_owned();
        Some((session.username.clone(), session.csrf.clone(), id))
    });
    let Some((username, csrf, sign_in)) = signed_in else {
        return error(StatusCode::UNAUTHORIZED, "sign in to the console to stream");
    };
    if !headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|origin| origin_allowed(&h, &headers, origin))
    {
        return error(StatusCode::FORBIDDEN, "request origin is not allowed");
    }
    let protocols: Vec<&str> = headers
        .get_all(header::SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .collect();
    let token = protocols.iter().find_map(|p| p.strip_prefix("csrf."));
    if !token.is_some_and(|token| crypto::equal(token.as_bytes(), csrf.as_bytes())) {
        return error(StatusCode::FORBIDDEN, "CSRF token required");
    }
    if !protocols.contains(&CONSOLE_PROTOCOL) {
        return error(
            StatusCode::BAD_REQUEST,
            "the rubylight.signal.v2 subprotocol is required",
        );
    }
    // Loopback counts as the local network, so behind a reverse proxy on this PC every browser
    // passes, as with origin_web_ui_allowed in `guard`.
    if !network::local_network(connection.peer.ip()) {
        tracing::info!(peer = %connection.peer, "console stream refused: the browser is outside the local network");
        return error(
            StatusCode::FORBIDDEN,
            "streaming in the console works on the local network and Tailscale only",
        );
    }
    if !h.config.read().unwrap().webrtc_server() {
        return error(
            StatusCode::CONFLICT,
            "turn on the WebRTC server in Settings to stream in the console",
        );
    }
    let Some(webrtc) = webrtc else {
        return error(StatusCode::CONFLICT, "WebRTC streaming is unavailable");
    };
    // Only a real upgrade creates the device.
    let upgrade = match upgrade {
        Ok(upgrade) => upgrade,
        Err(rejection) => return rejection.into_response(),
    };
    let device = {
        let webrtc = webrtc.clone();
        let host = h.clone();
        let user = username.clone();
        match tokio::task::spawn_blocking(move || webrtc.console_device(&host, &user)).await {
            Ok(Ok(device)) => device,
            Ok(Err(butterpollo_session::webrtc::protocol::ErrorCode::ClientDisabled)) => {
                return error(
                    StatusCode::LOCKED,
                    "this console's device is disabled on the Devices page",
                );
            }
            Ok(Err(_)) | Err(_) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "the console's device could not be saved",
                );
            }
        }
    };
    let Some((client_id, _)) = device.webrtc() else {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "not a WebRTC device");
    };
    let watched = h.clone();
    webrtc.console_upgrade(
        h.clone(),
        upgrade,
        ConsoleSignIn {
            client_id: client_id.to_owned(),
            name: device.name.clone(),
            peer: connection.peer,
            local: connection.local,
            this_pc: network::browser_on_this_pc(connection.peer, connection.local, &headers),
            signed_in: Box::new(move || {
                watched.web_sessions.lock().unwrap().values().any(|s| {
                    s.refresh_expires > Instant::now()
                        && s.extra
                            .get(web_sessions::SIGN_IN_ID)
                            .and_then(Value::as_str)
                            == Some(sign_in.as_str())
                })
            }),
        },
    )
}

/// `GET /guest/signal`: the guest page's socket (docs/browser-guests.md). It needs the page's
/// own origin and the `rubylight.guest.v1` subprotocol; the invite token is its first message,
/// never part of a URL.
pub(super) async fn guest_signal(
    State(h): State<Shared>,
    Extension(connection): Extension<Connection>,
    Extension(webrtc): Extension<Option<Arc<dyn WebRtcWebService>>>,
    headers: HeaderMap,
    upgrade: Result<
        axum::extract::WebSocketUpgrade,
        axum::extract::ws::rejection::WebSocketUpgradeRejection,
    >,
) -> Response {
    if !headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|origin| origin_allowed(&h, &headers, origin))
    {
        return error(StatusCode::FORBIDDEN, "request origin is not allowed");
    }
    let wanted = headers
        .get_all(header::SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|p| p.trim() == GUEST_PROTOCOL);
    if !wanted {
        return error(
            StatusCode::BAD_REQUEST,
            "the rubylight.guest.v1 subprotocol is required",
        );
    }
    if !h.config.read().unwrap().webrtc_server() {
        return error(
            StatusCode::CONFLICT,
            "streaming in the browser is off on this host",
        );
    }
    let Some(webrtc) = webrtc else {
        return error(StatusCode::CONFLICT, "WebRTC streaming is unavailable");
    };
    let upgrade = match upgrade {
        Ok(upgrade) => upgrade,
        Err(rejection) => return rejection.into_response(),
    };
    webrtc.guest_upgrade(h.clone(), upgrade, connection.peer, connection.local)
}

/// The guest port answers only while a link is live or a guest watches; `/play` only the local
/// network while the WebRTC server is on, whatever the links.
pub(super) async fn guest_gate(
    State(h): State<Shared>,
    Extension(connection): Extension<Connection>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if path == "/play" || path.starts_with("/play/") {
        if let Some(refused) = play_refused(&h, &connection, path) {
            return refused;
        }
    } else if !h.guest_net.open() {
        return guest_port_closed();
    }
    next.run(request).await
}

/// What the guest port answers any path while no link is open.
fn guest_port_closed() -> Response {
    error(StatusCode::NOT_FOUND, "no invite link is open on this host")
}

/// Why the play page or its socket is refused to this connection, if it is. Only the TCP peer
/// counts: `X-Forwarded-For` and its kind are the client's to write, and a router that forwards
/// the guest port keeps the internet peer's address ([`network::play_refusal`] has the
/// forwards that do not). Loopback counts as the local network, as for the console's Stream page.
/// A refused peer gets what the guest port answers a path it does not serve, so the internet
/// learns nothing of `/play`.
fn play_refused(h: &Shared, connection: &Connection, path: &str) -> Option<Response> {
    if let Some(reason) = network::play_refused(connection.peer, connection.local) {
        // A forwarded guest port takes the internet's scans: one line in ten seconds.
        static REFUSED: std::sync::Mutex<network::Throttle> =
            std::sync::Mutex::new(network::Throttle::new());
        if let Some(more) = REFUSED.lock().unwrap().allow() {
            tracing::info!(peer = %connection.peer, local = %connection.local, path, reason, more, "play page refused");
        }
        return Some(if h.guest_net.open() {
            StatusCode::NOT_FOUND.into_response()
        } else {
            guest_port_closed()
        });
    }
    if !h.config.read().unwrap().webrtc_server() {
        return Some(error(
            StatusCode::NOT_FOUND,
            "turn on the WebRTC server in the Rubylight console to play in the browser",
        ));
    }
    None
}

/// The play page (docs/webrtc.md, "Play in a TV browser"): its own build of the web app
/// (`dist/play`), and nothing of the console's.
pub(super) async fn play_page(
    State(h): State<Shared>,
    method: Method,
    uri: axum::http::Uri,
) -> Response {
    let root = h.assets.join("play");
    let path = uri
        .path()
        .strip_prefix("/play")
        .unwrap_or_default()
        .to_owned();
    tokio::task::spawn_blocking(move || {
        if !root.join("index.html").is_file() {
            return error(
                StatusCode::NOT_FOUND,
                "the play page is not installed on this host",
            );
        }
        app_file(&root, &method, &path, GUEST_POLICY)
    })
    .await
    .unwrap_or_else(|e| error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))
}

/// `GET /play/signal`: the play page's socket, protocol v2 as on `webrtc_port`. It needs the
/// page's own origin and the `rubylight.play.v2` subprotocol; [`guest_gate`] let in only the
/// local network. It pairs or authenticates first, with no console sign-in.
pub(super) async fn play_signal(
    State(h): State<Shared>,
    Extension(connection): Extension<Connection>,
    Extension(webrtc): Extension<Option<Arc<dyn WebRtcWebService>>>,
    headers: HeaderMap,
    upgrade: Result<
        axum::extract::WebSocketUpgrade,
        axum::extract::ws::rejection::WebSocketUpgradeRejection,
    >,
) -> Response {
    if !headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|origin| origin_allowed(&h, &headers, origin))
    {
        return error(StatusCode::FORBIDDEN, "request origin is not allowed");
    }
    let wanted = headers
        .get_all(header::SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|p| p.trim() == PLAY_PROTOCOL);
    if !wanted {
        return error(
            StatusCode::BAD_REQUEST,
            "the rubylight.play.v2 subprotocol is required",
        );
    }
    let Some(webrtc) = webrtc else {
        return error(StatusCode::CONFLICT, "WebRTC streaming is unavailable");
    };
    let upgrade = match upgrade {
        Ok(upgrade) => upgrade,
        Err(rejection) => return rejection.into_response(),
    };
    webrtc.play_upgrade(
        h.clone(),
        upgrade,
        connection.peer,
        connection.local,
        &headers,
    )
}

/// The guest page: its own build of the web app (`dist/guest`), and nothing of the console's.
pub(super) async fn guest_page(
    State(h): State<Shared>,
    method: Method,
    uri: axum::http::Uri,
) -> Response {
    let root = h.assets.join("guest");
    let path = uri
        .path()
        .strip_prefix("/guest")
        .unwrap_or_default()
        .to_owned();
    tokio::task::spawn_blocking(move || {
        if !root.join("index.html").is_file() {
            return error(
                StatusCode::NOT_FOUND,
                "the guest page is not installed on this host",
            );
        }
        app_file(&root, &method, &path, GUEST_POLICY)
    })
    .await
    .unwrap_or_else(|e| error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))
}

/// The console's own response headers (`guard`), for the guest routes, which it does not
/// guard; and no referrer, so a link the page opens learns nothing of the invite's.
pub(super) async fn guest_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert("X-Content-Type-Options", "nosniff".parse().unwrap());
    headers.insert("X-Frame-Options", "DENY".parse().unwrap());
    headers.insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    headers
        .entry(header::CACHE_CONTROL)
        .or_insert("no-store".parse().unwrap());
    response
}

/// Allowed sources for the web app: covers come from IGDB and the cover
/// search reads LizardByte's game database.
pub(super) const APP_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob: https://images.igdb.com; connect-src 'self' https://raw.githubusercontent.com; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";
/// The guest page loads nothing but its own files and talks to nothing but this host.
pub const GUEST_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

/// The console: the web app when it is installed, else the server-rendered
/// pages.
pub(super) async fn site(
    state: State<Shared>,
    connection: Extension<Connection>,
    method: Method,
    uri: axum::http::Uri,
    headers: HeaderMap,
) -> Response {
    let root = state.0.assets.clone();
    let check_method = method.clone();
    let check_path = uri.path().to_owned();
    let served = tokio::task::spawn_blocking(move || {
        root.join("index.html")
            .is_file()
            .then(|| app_file(&root, &check_method, &check_path, APP_POLICY))
    })
    .await
    .ok()
    .flatten();
    if let Some(response) = served {
        return response;
    }
    crate::console::page(state, connection, method, uri, headers).await
}

/// A file of the web app; the app's own page paths get index.html, with `policy` as its CSP.
pub(super) fn app_file(
    root: &std::path::Path,
    method: &Method,
    path: &str,
    policy: &str,
) -> Response {
    if !matches!(*method, Method::GET | Method::HEAD) {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let relative = path.trim_start_matches('/');
    let safe = !relative.is_empty()
        && relative.split('/').all(|part| {
            !part.is_empty() && part != "." && part != ".." && !part.contains(['\\', ':'])
        });
    let found = safe
        .then(|| root.join(relative))
        .filter(|file| file.is_file());
    let asset = relative.starts_with("assets/");
    if asset && found.is_none() {
        // A page built before an upgrade asks for files that are gone; HTML
        // in their place would fail as a script.
        return StatusCode::NOT_FOUND.into_response();
    }
    let file = found.unwrap_or_else(|| root.join("index.html"));
    let Ok(bytes) = std::fs::read(&file) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let kind = match file.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("json") => "application/json",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    };
    let mut response = ([(header::CONTENT_TYPE, kind)], bytes).into_response();
    let headers = response.headers_mut();
    if asset {
        // Built files carry a content hash in their names.
        headers.insert(
            header::CACHE_CONTROL,
            "public, max-age=31536000, immutable".parse().unwrap(),
        );
    }
    if kind.starts_with("text/html") {
        headers.insert(header::CONTENT_SECURITY_POLICY, policy.parse().unwrap());
    }
    response
}
