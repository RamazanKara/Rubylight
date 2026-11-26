//! Web console router, security guard middleware, and `/api/*` request dispatcher.
mod apps;
mod auth;
mod backup;
mod child_session;
mod clients;
mod coop;
#[cfg(any(test, feature = "test-support"))]
pub use clients::handle as update_clients;
#[cfg(any(test, feature = "test-support"))]
pub use clients::pairing_qr;
mod config;
mod displays;
mod guests;
mod logs;
mod maintenance;
mod neural;
mod nvidia_component;
mod power;
mod radeon;
mod session;
mod session_guests;
mod tokens;
mod webhooks;
mod webrtc_routes;

#[cfg(test)]
use auth::{BASIC_CACHE, BasicCache, SignIns};
pub(crate) use auth::{access, authenticated, cookie, refresh_browser};
use auth::{basic_auth_cross_site, origin_allowed, token_authenticated};
use axum::{
    Extension, Json, Router,
    body::Bytes,
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use butterpollo_crypto::{auth as crypto_auth, crypto};
#[cfg(test)]
use butterpollo_session::state::Credentials;
use butterpollo_stream::{state::Shared, tls::Connection, webrtc_bridge::WebRtcWebService};
use serde_json::{Value, json};
use std::sync::Arc;
#[cfg(test)]
use std::time::{Duration, Instant};
pub use webrtc_routes::GUEST_POLICY;
#[cfg(test)]
use webrtc_routes::{APP_POLICY, app_file};
use webrtc_routes::{
    guest_gate, guest_headers, guest_page, guest_signal, play_page, play_signal, site,
    webrtc_signal,
};

fn response(status: StatusCode, value: Value) -> Response {
    (status, Json(value)).into_response()
}
fn error(status: StatusCode, message: &str) -> Response {
    response(status, json!({"status":false,"error":message}))
}

/// The app of a cover request: /api/apps/ID/cover or /api/covers/ID. Not by
/// slicing: "/api/apps/cover" both starts and ends right, and a panic aborts
/// the host.
fn cover_id(path: &str) -> Option<&str> {
    path.strip_prefix("/api/apps/")
        .and_then(|rest| rest.strip_suffix("/cover"))
        .or_else(|| path.strip_prefix("/api/covers/"))
        .filter(|id| !id.is_empty())
}
/// The app of an icon request: /api/apps/UUID/icon.
fn icon_id(path: &str) -> Option<&str> {
    path.strip_prefix("/api/apps/")
        .and_then(|rest| rest.strip_suffix("/icon"))
        .filter(|id| !id.is_empty() && !id.contains('/'))
}

#[cfg(any(test, feature = "test-support"))]
pub fn token_catalog() -> Vec<crypto_auth::Scope> {
    tokens::catalog()
}
#[cfg(not(any(test, feature = "test-support")))]
fn token_catalog() -> Vec<crypto_auth::Scope> {
    tokens::catalog()
}

/// The console without a stream pipeline: uses whatever WebRTC service was installed on `h`.
#[cfg(any(test, feature = "test-support"))]
pub fn router(h: Shared) -> Router {
    let svc = h.webrtc_service();
    router_with(h, svc)
}
/// The guest port without a stream pipeline: uses whatever WebRTC service was installed on `h`.
#[cfg(any(test, feature = "test-support"))]
pub fn guest_router(h: Shared) -> Router {
    let svc = h.webrtc_service();
    guest_router_with(h, svc)
}
/// The guest port's router: the guest page and its socket (docs/browser-guests.md), the play
/// page and its socket (docs/webrtc.md, "Play in a TV browser"), and nothing of the console's.
/// A guest has no sign-in: its reach is the invite's. Until the owner makes a link, and once no
/// link is live and nobody watches, everything but `/play` answers 404. `/play` answers only the
/// local network and Tailscale, while the WebRTC server is on; its socket pairs or authenticates
/// as a TV does on `webrtc_port`, with `webrtc`'s stream pipeline.
pub fn guest_router_with(h: Shared, webrtc: Option<Arc<dyn WebRtcWebService>>) -> Router {
    Router::new()
        .route("/play/signal", get(play_signal))
        .route("/play", get(play_page))
        .route("/play/", get(play_page))
        .route("/play/{*path}", get(play_page))
        .route("/guest/signal", get(guest_signal))
        // The app icon, as the console's: the guest page's tab shows it.
        .route("/guest/favicon.svg", get(crate::console::favicon))
        .route("/favicon.ico", get(crate::console::favicon))
        .route("/guest", get(guest_page))
        .route("/guest/", get(guest_page))
        .route("/guest/{*path}", get(guest_page))
        .fallback(|| async { StatusCode::NOT_FOUND })
        .layer(middleware::from_fn_with_state(h.clone(), guest_gate))
        .layer(middleware::from_fn(guest_headers))
        .layer(Extension(webrtc))
        .with_state(h)
}
/// The console. `webrtc` is what its stream page needs from the WebRTC server. The guest page
/// has a port of its own ([`guest_router`]): forwarding it never opens the console.
pub fn router_with(h: Shared, webrtc: Option<Arc<dyn WebRtcWebService>>) -> Router {
    Router::new()
        .route("/api/webrtc/signal", get(webrtc_signal))
        // A settings backup may be larger than the other requests.
        .route(
            "/api/backup/inspect",
            axum::routing::post(api).layer(axum::extract::DefaultBodyLimit::max(
                butterpollo_session::settings_backup::MAX_ARCHIVE as usize,
            )),
        )
        .route(
            "/api/{*path}",
            get(api).post(api).patch(api).put(api).delete(api),
        )
        .route(
            "/console/action",
            axum::routing::post(crate::console::action),
        )
        .route("/console.css", get(crate::console::stylesheet))
        .route("/favicon.svg", get(crate::console::favicon))
        .fallback(site)
        // Uploaded cover images are the largest requests.
        .layer(axum::extract::DefaultBodyLimit::max(16 * 1024 * 1024))
        .layer(middleware::from_fn_with_state(h.clone(), guard))
        .layer(Extension(webrtc))
        .with_state(h)
}

async fn guard(State(h): State<Shared>, mut request: Request, next: Next) -> Response {
    // origin_web_ui_allowed: pc, lan (default) or wan, as in Vibepollo.
    if let Some(connection) = request.extensions().get::<Connection>()
        && butterpollo_stream::network::peer_reach(connection.peer)
            > butterpollo_stream::network::web_reach(&h.config.read().unwrap())
    {
        tracing::info!(peer = %connection.peer, "web interface request from outside the allowed network refused");
        return error(StatusCode::FORBIDDEN, "Forbidden");
    }
    let console_form = request.uri().path() == "/console/action";
    let (method, path) = if console_form {
        if request.method() != Method::POST {
            return error(StatusCode::METHOD_NOT_ALLOWED, "POST required");
        }
        if !request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.split(';').next() == Some("application/x-www-form-urlencoded"))
        {
            return error(StatusCode::BAD_REQUEST, "form content type required");
        }
        let (mut parts, body) = request.into_parts();
        let bytes = match axum::body::to_bytes(body, 1024 * 1024).await {
            Ok(bytes) => bytes,
            Err(_) => return error(StatusCode::PAYLOAD_TOO_LARGE, "form exceeds the limit"),
        };
        let fields: crate::console::Fields =
            url::form_urlencoded::parse(&bytes).into_owned().collect();
        let route = match crate::console::route(&fields) {
            Ok(route) => route,
            Err(message) => return error(StatusCode::BAD_REQUEST, &message),
        };
        let Some(csrf) = fields
            .get("_csrf")
            .and_then(|v| v.parse::<axum::http::HeaderValue>().ok())
        else {
            return error(StatusCode::BAD_REQUEST, "CSRF token required");
        };
        parts.headers.insert("X-CSRF-Token", csrf);
        request = Request::from_parts(parts, axum::body::Body::from(bytes));
        route
    } else {
        (request.method().clone(), request.uri().path().to_owned())
    };
    let path = path.as_str();
    let public = matches!(
        path,
        "/api/auth/login"
            | "/api/auth/refresh"
            | "/api/auth/status"
            | "/api/csrf-token"
            | "/api/configLocale"
    );
    if path.starts_with("/api/") {
        h.reload_credentials();
    }
    let fresh_password = path == "/api/password" && h.credentials.read().unwrap().is_none();
    let api_token = token_authenticated(&h, request.headers(), path, method.as_str());
    // The first password is set from a browser on this PC. A reverse proxy or tunnel on this PC
    // connects from here too, for a browser anywhere, so its headers count
    // (`browser_on_this_pc`); its users set the first password with `--creds`.
    if fresh_password
        && !request.extensions().get::<Connection>().is_some_and(|c| {
            butterpollo_stream::network::browser_on_this_pc(c.peer, c.local, request.headers())
        })
    {
        return error(
            StatusCode::FORBIDDEN,
            "initial credentials must be set from this computer",
        );
    }
    if path.starts_with("/api/")
        && !public
        && !fresh_password
        && !api_token
        && !authenticated(
            &h,
            request.headers(),
            request
                .extensions()
                .get::<Connection>()
                .map(|c| c.peer.ip()),
        )
        .await
    {
        return error(StatusCode::UNAUTHORIZED, "authentication required");
    }
    if !matches!(method, Method::GET | Method::HEAD | Method::OPTIONS) {
        if let Some(origin) = request
            .headers()
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
            && !origin_allowed(&h, request.headers(), origin)
        {
            return error(StatusCode::FORBIDDEN, "request origin is not allowed");
        }
        if !request.headers().contains_key(header::ORIGIN)
            && !public
            && !console_form
            && !api_token
            && access(request.headers()).is_none()
            && basic_auth_cross_site(request.headers())
        {
            return error(
                StatusCode::FORBIDDEN,
                "a browser form or cross-site request can't use Basic credentials; send JSON",
            );
        }
        if path == "/api/auth/login"
            || fresh_password
            || (console_form && access(request.headers()).is_none())
        {
            let expected = cookie(request.headers(), "__Host-apollo_anon_csrf");
            let got = request
                .headers()
                .get("X-CSRF-Token")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if (console_form && expected.is_none())
                || expected
                    .is_some_and(|expected| !crypto::equal(expected.as_bytes(), got.as_bytes()))
            {
                return error(StatusCode::BAD_REQUEST, "CSRF token required");
            }
        }
        if !public
            && !fresh_password
            && !api_token
            && let Some(token) = access(request.headers())
        {
            let sessions = h.web_sessions.lock().unwrap();
            let expected =
                butterpollo_stream::web_sessions::find(&sessions, &token).map(|s| s.csrf.as_str());
            let got = request
                .headers()
                .get("X-CSRF-Token")
                .and_then(|v| v.to_str().ok());
            if expected.is_none()
                || got.is_none()
                || !crypto::equal(expected.unwrap().as_bytes(), got.unwrap().as_bytes())
            {
                return error(StatusCode::BAD_REQUEST, "CSRF token required");
            }
        }
    }
    // A signed-in administrator using the console keeps the PC awake (sleep
    // after idle), once the request passed the origin and CSRF checks; API
    // tokens are automation, not someone at the console, and a page left open
    // polls (`X-Rubylight-Poll`, and the power page's own status) without
    // anyone there. Only a GET can be a poll.
    let polled = method == Method::GET
        && (path == "/api/power" || request.headers().contains_key("x-rubylight-poll"));
    if path.starts_with("/api/") && !public && !fresh_password && !api_token && !polled {
        butterpollo_stream::wake::note_console_activity(&h);
    }
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert("X-Content-Type-Options", "nosniff".parse().unwrap());
    headers.insert("X-Frame-Options", "DENY".parse().unwrap());
    headers
        .entry(header::CACHE_CONTROL)
        .or_insert("no-store".parse().unwrap());
    response
}

pub(crate) async fn api(
    State(h): State<Shared>,
    Extension(connection): Extension<Connection>,
    method: Method,
    uri: axum::http::Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path();
    // What is left of the removed child sessions takes no body.
    if path == "/api/child-session" || path.starts_with("/api/child-session/") {
        return child_session::handle(&h, &method, path).await;
    }
    // Settings backup: `inspect` takes the archive itself as the body.
    if path.starts_with("/api/backup/") {
        return backup::handle(&h, &method, path, &body).await;
    }
    let data: Value = if body.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(_) => return error(StatusCode::BAD_REQUEST, "invalid JSON"),
        }
    };
    if path == "/api/coop" || path.starts_with("/api/coop/") {
        return coop::handle(&h, &method, path, &data).await;
    }
    if path == "/api/guests" || path.starts_with("/api/guests/") {
        return guests::handle(&h, &method, path, &data);
    }
    if path.starts_with("/api/session-guests/invites") {
        // Making an invite can start the rendezvous and taking one back saves the paired devices:
        // both block.
        let (h, method, path, data) = (h.clone(), method.clone(), path.to_owned(), data.clone());
        return tokio::task::spawn_blocking(move || {
            session_guests::handle(&h, &method, &path, &data)
        })
        .await
        .unwrap_or_else(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "the invite failed"));
    }
    if path == "/api/session-guests" || path.starts_with("/api/session-guests/") {
        return session_guests::handle(&h, &method, path, &data);
    }
    if path == "/api/neural" || path.starts_with("/api/neural/") {
        return neural::handle(&h, &method, path, &uri, &data).await;
    }
    if path == "/api/nvidia-component" || path.starts_with("/api/nvidia-component/") {
        return nvidia_component::handle(&h, &method, path).await;
    }
    // The app editor's game settings files: which devices have a saved copy. Console-only.
    if method == Method::GET && path == "/api/game-settings" {
        let uuid = url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
            .find(|(key, _)| key == "uuid")
            .map(|(_, value)| value.into_owned())
            .unwrap_or_default();
        let host = h.clone();
        return match tokio::task::spawn_blocking(move || {
            butterpollo_stream::game_settings::status(&host, &uuid)
        })
        .await
        {
            Ok(Ok(status)) => Json(status).into_response(),
            Ok(Err(e)) => error(StatusCode::NOT_FOUND, &format!("{e:#}")),
            Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        };
    }
    // Put back now: a swap waiting for a process that is not the game.
    if method == Method::POST && path == "/api/game-settings/release" {
        let Some(id) = data.get("id").and_then(Value::as_str).map(str::to_owned) else {
            return error(StatusCode::BAD_REQUEST, "id is required");
        };
        let host = h.clone();
        return match tokio::task::spawn_blocking(move || {
            butterpollo_stream::game_settings::release(&host, &id)
        })
        .await
        {
            Ok(Ok(_)) => Json(json!({"ok": true})).into_response(),
            Ok(Err(e)) => error(StatusCode::CONFLICT, &format!("{e:#}")),
            Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        };
    }
    if method == Method::POST && path == "/api/covers/upload" {
        return apps::upload_cover(&h, &data).await;
    }
    if path == "/api/power" || path.starts_with("/api/power/") {
        return power::handle(&h, &method, &uri, connection.local.ip(), &data).await;
    }
    if let Some(action) = path.strip_prefix("/api/library/") {
        return apps::library(&h, &method, action, &data).await;
    }
    if let Some(action) = path.strip_prefix("/api/steam/") {
        return apps::steam(&h, &method, action, &uri, &data).await;
    }
    if let Some(action) = path.strip_prefix("/api/playnite/") {
        return apps::playnite(&h, &method, action, &data).await;
    }
    if method == Method::GET && path == "/api/lossless_scaling/status" {
        return apps::lossless_scaling(&h, &uri).await;
    }
    if method == Method::GET && path == "/api/radeon" {
        return radeon::read(&h).await;
    }
    if method == Method::GET && path == "/api/browse" {
        return apps::browse(&uri).await;
    }
    if method == Method::GET
        && let Some(id) = icon_id(path)
    {
        return apps::icon(&h, id, &headers).await;
    }
    if method == Method::POST && path == "/api/apps/purge_autosync" {
        return apps::purge_autosync(&h).await;
    }
    if method == Method::GET && path == "/api/logs/export_crash" {
        return logs::export_crash(&h, logs::include_dump(&uri)).await;
    }
    if method == Method::GET && path == "/api/logs/tail" {
        let h = h.clone();
        return tokio::task::spawn_blocking(move || logs::tail(&h, &uri))
            .await
            .unwrap_or_else(|e| error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()));
    }
    if method == Method::GET && path == "/api/logs/analysis" {
        return logs::analysis(&h).await;
    }
    if method == Method::GET && matches!(path, "/api/logs" | "/api/logs/export") {
        let h = h.clone();
        let path = path.to_owned();
        return tokio::task::spawn_blocking(move || logs::export(&h, &path))
            .await
            .unwrap_or_else(|e| error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()));
    }
    if method == Method::GET
        && ((path.starts_with("/api/apps/") && path.ends_with("/cover"))
            || path.starts_with("/api/covers/"))
    {
        return apps::cover(&h, path, &headers).await;
    }
    if path == "/api/auth/status" {
        return auth::status(&h, &headers, &connection).await;
    }
    if path == "/api/auth/login" {
        return auth::login(&h, &method, &headers, &connection, &data).await;
    }
    if path == "/api/auth/refresh" {
        return auth::refresh(&h, &method, &headers, &connection, &data);
    }
    if path == "/api/auth/logout" {
        return auth::logout(&h, &headers);
    }
    if path == "/api/csrf-token" {
        return auth::csrf_token(&h, &method, &headers);
    }
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
        let path = uri.path();
        Ok(match (method.as_str(), path) {
            ("GET", "/api/updates")
            | ("POST", "/api/updates/install")
            | ("POST", "/api/updates/install_now")
            | ("POST", "/api/updates/cancel")
            | ("POST", "/api/updates/check") => maintenance::handle(&h, method.as_str(), path)?,
            ("GET", "/api/health/crashdump")
            | ("POST", "/api/health/crashdump/dismiss")
            | ("GET", "/api/logs/export_crash/manifest") => {
                logs::handle(&h, method.as_str(), path, &data, logs::include_dump(&uri))?
            }
            ("GET", "/api/display/golden_status")
            | ("POST", "/api/display/export_golden")
            | ("POST", "/api/display/restore_golden")
            | ("POST", "/api/reset-display-device-persistence")
            | ("DELETE", "/api/display/golden")
            | ("POST", "/api/display/terminate_virtual")
            | ("GET", "/api/clients/display-layout")
            | ("PUT", "/api/clients/display-layout") => {
                displays::handle(&h, method.as_str(), &uri, &data)?
            }
            ("GET", "/api/internet-play") => butterpollo_stream::internet_play::console_status(&h)?,
            ("GET", "/api/rtss/status" | "/api/frame-limiter/status") => {
                session::handle(&h, method.as_str(), path)?
            }
            ("GET", "/api/sessions/history") => session::handle(&h, method.as_str(), path)?,
            ("GET", "/api/health/vulkan-hdr-layer")
            | ("POST", "/api/health/vulkan-hdr-layer/register") => {
                displays::handle(&h, method.as_str(), &uri, &data)?
            }
            ("GET", "/api/auth/sessions") => auth::list_sessions(&h, &headers),
            ("DELETE", p) if p.starts_with("/api/auth/sessions/") => {
                auth::delete_session(&h, &p[19..])?
            }
            ("GET", "/api/token/routes") => json!({"status":true,"routes":token_catalog()}),
            ("GET", "/api/tokens") => tokens::list(&h)?,
            ("POST", "/api/token") => tokens::issue(&h, &data)?,
            ("DELETE", p) if p.starts_with("/api/token/") => tokens::revoke(&h, &p[11..])?,
            ("GET", "/api/config")
            | ("POST" | "PATCH", "/api/config")
            | ("GET", "/api/configLocale")
            | ("GET", "/api/meta" | "/api/metadata")
            | ("POST", "/api/warnings/dismiss") => {
                config::handle(&h, method.as_str(), path, &data)?
            }
            ("GET", "/api/apps")
            | ("POST", "/api/apps")
            | ("POST", "/api/apps/delete")
            | ("POST", "/api/apps/reorder")
            | ("POST", "/api/apps/close")
            | ("POST", "/api/apps/launch") => apps::handle(&h, method.as_str(), path, &data)?,
            ("GET", "/api/clients/list") => clients::handle(&h, method.as_str(), path, &data)?,
            ("GET", "/api/clients/hdr-profiles") => {
                displays::handle(&h, method.as_str(), &uri, &data)?
            }
            ("POST", "/api/clients/unpair")
            | ("POST", "/api/clients/unpair-all")
            | ("POST", "/api/clients/disconnect")
            | ("POST", "/api/clients/reset-network-history")
            | ("POST", "/api/clients/update")
            | ("GET", "/api/clients/pending") => clients::handle(&h, method.as_str(), path, &data)?,
            ("GET", "/api/rtsp/sessions") | ("GET", "/api/session/status") => {
                session::handle(&h, method.as_str(), path)?
            }
            ("GET", "/api/usb") => json!({"status":true,"devices":h.usb.list()}),
            ("POST", "/api/usb/detach") => {
                let Some(id) = data.get("id").and_then(Value::as_u64) else {
                    anyhow::bail!("id is missing");
                };
                // A device the driver kept is detached again here; its error is the answer.
                if !h.usb.detach(id)? {
                    anyhow::bail!("no forwarded USB device {id}");
                }
                json!({"status":true})
            }
            ("GET" | "POST", "/api/session/console") => {
                session::console(&h, method.as_str(), &data)?
            }
            ("GET" | "POST", "/api/session/suspend") | ("POST", "/api/session/resume") => {
                session::quick_suspend(&h, method.as_str(), path)
            }
            ("GET", "/api/display-devices") | ("GET", "/api/framegen/edid-refresh") => {
                displays::handle(&h, method.as_str(), &uri, &data)?
            }
            ("POST", "/api/otp") | ("POST", "/api/pin") | ("POST", "/api/pairing/qr") => {
                clients::handle(&h, method.as_str(), path, &data)?
            }
            ("GET" | "POST", "/api/webhooks")
            | ("POST", "/api/webhooks/delete")
            | ("POST", "/api/webhooks/test") => webhooks::handle(&h, method.as_str(), path, &data)?,
            ("POST", "/api/password") => auth::change_password(&h, &data)?,
            ("POST", "/api/restart") | ("POST", "/api/quit") => {
                maintenance::handle(&h, method.as_str(), path)?
            }
            ("DELETE", p) if p.starts_with("/api/apps/") => {
                apps::handle(&h, method.as_str(), path, &data)?
            }
            _ => {
                return Err(anyhow::anyhow!("unknown API endpoint"));
            }
        })
    })
    .await
    .unwrap_or_else(|e| Err(e.into()));
    match result {
        Ok(v) => Json(v).into_response(),
        // A delete refused while the game is busy or a stream runs, as every streaming refusal.
        Err(e)
            if e.is::<butterpollo_stream::neural::Busy>()
                || e.is::<butterpollo_stream::neural::StreamRunning>() =>
        {
            error(StatusCode::CONFLICT, &e.to_string())
        }
        Err(e) => match e.downcast_ref::<butterpollo_stream::nucleus::Refusal>() {
            // A settings save's Nucleus Co-op folder, refused as the panels refuse it.
            Some(refusal) => error(
                StatusCode::from_u16(refusal.status).unwrap_or(StatusCode::BAD_REQUEST),
                &refusal.message,
            ),
            None => error(StatusCode::BAD_REQUEST, &e.to_string()),
        },
    }
}
#[cfg(test)]
mod tests;
