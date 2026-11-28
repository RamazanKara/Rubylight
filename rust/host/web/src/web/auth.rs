//! Browser session authentication, sign-in rate limiting, CSRF checks, and auth/password endpoints.
mod cache;
use super::{error, response};
use axum::{
    Json,
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::Engine;
use butterpollo_crypto::{auth, crypto};
use butterpollo_session::state::Credentials;
use butterpollo_stream::{state::Shared, tls::Connection, web_sessions};
use cache::SIGN_INS;
#[cfg(test)]
pub(super) use cache::SignIns;
pub(super) use cache::{BASIC_CACHE, BasicCache};
use serde_json::{Value, json};
use std::time::Instant;

pub(crate) fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|c| {
            let (k, v) = c.trim().split_once('=')?;
            if k == name { Some(v.to_owned()) } else { None }
        })
}

pub(crate) fn access(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_owned)
        .or_else(|| cookie(headers, "__Host-apollo_session"))
}

/// Serializes uncached PBKDF2 checks so concurrent bursts with the same valid
/// Basic credentials verify once and coalesce onto [`BASIC_CACHE`] instead of
/// tripping the failed-attempt limiter while the first hash is in flight.
static SIGN_IN_VERIFY: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Replace `stored`, just verified with `password`, by a current PBKDF2
/// hash. Blocking: run it on the blocking pool.
fn rehash_if_needed(h: &Shared, stored: &Credentials, password: &str) {
    match Credentials::new(stored.username.clone(), password) {
        Ok(upgraded) => match h.save_credentials(&upgraded) {
            Ok(()) => {
                *h.credentials.write().unwrap() = Some(upgraded);
                BASIC_CACHE.lock().unwrap().forget(stored);
            }
            Err(error) => tracing::warn!(%error, "could not save upgraded password hash"),
        },
        Err(error) => tracing::warn!(%error, "could not upgrade password hash"),
    }
}

/// Check `user`/`pass` against the stored credentials on the blocking pool,
/// as one sign-in attempt from `peer`, and upgrade a legacy hash that
/// passes. Concurrent uncached attempts wait on `SIGN_IN_VERIFY` so parallel
/// guesses still count sequentially against [`SIGN_INS`] without holding the
/// mutex across `.await`, while matching valid requests coalesce onto
/// [`BASIC_CACHE`].
async fn sign_in(h: &Shared, peer: std::net::IpAddr, user: &str, pass: &str) -> bool {
    if !SIGN_INS.lock().unwrap().allowed(peer, Instant::now()) {
        return false;
    }
    let _verify = SIGN_IN_VERIFY.lock().await;
    let now = Instant::now();
    if !SIGN_INS.lock().unwrap().allowed(peer, now) {
        return false;
    }
    let initial = h.credentials.read().unwrap().clone();
    let mut stored = match initial {
        Some(stored) => stored,
        None => {
            let host = h.clone();
            let _ = tokio::task::spawn_blocking(move || host.reload_credentials()).await;
            let reloaded = h.credentials.read().unwrap().clone();
            let Some(stored) = reloaded else {
                SIGN_INS.lock().unwrap().failed(peer, now);
                return false;
            };
            stored
        }
    };
    let mut key = BasicCache::key(&stored, user, pass);
    if !stored.needs_rehash() && BASIC_CACHE.lock().unwrap().hit(&key, now) {
        SIGN_INS.lock().unwrap().succeeded(peer);
        return true;
    }
    let (user, pass) = (user.to_owned(), pass.to_owned());
    let mut verified = tokio::task::spawn_blocking({
        let stored = stored.clone();
        let user = user.clone();
        let pass = pass.clone();
        move || stored.verifies(&user, &pass)
    })
    .await
    .unwrap_or(false);
    // If `--creds` updated the file within the hot-path stat-throttle window, reload once now.
    if !verified {
        let host = h.clone();
        let refreshed = tokio::task::spawn_blocking(move || {
            host.reload_credentials();
            host.credentials.read().unwrap().clone()
        })
        .await
        .ok()
        .flatten();
        if let Some(fresh) = refreshed
            && fresh != stored
        {
            stored = fresh;
            key = BasicCache::key(&stored, &user, &pass);
            verified = tokio::task::spawn_blocking({
                let stored = stored.clone();
                let pass = pass.clone();
                move || stored.verifies(&user, &pass)
            })
            .await
            .unwrap_or(false);
        }
    }
    if !verified {
        SIGN_INS.lock().unwrap().failed(peer, Instant::now());
        return false;
    }
    SIGN_INS.lock().unwrap().succeeded(peer);
    if stored.needs_rehash() {
        let h = h.clone();
        let _ = tokio::task::spawn_blocking(move || rehash_if_needed(&h, &stored, &pass)).await;
    } else {
        BASIC_CACHE
            .lock()
            .unwrap()
            .insert(key, &stored, Instant::now());
    }
    true
}

/// Whether the request is signed in: a browser session, or Basic
/// credentials from `peer`, which count against its sign-in attempts.
pub(crate) async fn authenticated(
    h: &Shared,
    headers: &HeaderMap,
    peer: Option<std::net::IpAddr>,
) -> bool {
    if let Some(token) = access(headers) {
        let save = {
            let mut sessions = h.web_sessions.lock().unwrap();
            let Some(key) = web_sessions::resolve_hash(&sessions, &token) else {
                return false;
            };
            let Some(session) = sessions
                .get_mut(&key)
                .filter(|s| s.expires > Instant::now())
            else {
                return false;
            };
            let wall = web_sessions::now();
            if wall.saturating_sub(session.last_seen) >= 300 {
                let previous = session.last_seen;
                session.last_seen = wall;
                Some((key, previous, wall, sessions.clone()))
            } else {
                None
            }
        };
        if let Some((key, previous, wall, snapshot)) = save {
            let host = h.clone();
            let saved = tokio::task::spawn_blocking(move || host.save_web_sessions(&snapshot))
                .await
                .unwrap_or_else(|e| Err(e.into()));
            if let Err(error) = saved {
                if let Some(session) = h.web_sessions.lock().unwrap().get_mut(&key)
                    && session.last_seen == wall
                {
                    session.last_seen = previous;
                }
                tracing::debug!(%error, "browser session activity could not be saved");
            }
        }
        return true;
    }
    if let Some(encoded) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        && let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded)
        && let Ok(text) = std::str::from_utf8(&decoded)
        && let Some((user, pass)) = text.split_once(':')
    {
        let peer = peer.unwrap_or(std::net::Ipv4Addr::UNSPECIFIED.into());
        // Verified lately: no PBKDF2. A legacy hash always goes through
        // `sign_in`, which upgrades it.
        let cached = h
            .credentials
            .read()
            .unwrap()
            .as_ref()
            .filter(|c| !c.needs_rehash())
            .map(|c| BasicCache::key(c, user, pass));
        let now = Instant::now();
        if let Some(key) = cached
            && BASIC_CACHE.lock().unwrap().hit(&key, now)
        {
            let mut sign_ins = SIGN_INS.lock().unwrap();
            if !sign_ins.allowed(peer, now) {
                return false;
            }
            sign_ins.succeeded(peer);
            return true;
        }
        return sign_in(h, peer, user, pass).await;
    }
    false
}

pub(super) fn token_authenticated(
    h: &Shared,
    headers: &HeaderMap,
    path: &str,
    method: &str,
) -> bool {
    let Some(secret) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        return false;
    };
    let credentials = h.credentials.read().unwrap();
    let Some(credentials) = credentials.as_ref() else {
        return false;
    };
    // A token reaches only what the catalogue offers now: a scope saved before its route left the
    // catalogue (`/api/password` and `/api/browse` did) no longer works.
    let catalog = super::token_catalog();
    auth::read(&h.aliases.lock().unwrap()).is_ok_and(|mut tokens| {
        for token in &mut tokens {
            token.scopes.retain_mut(|scope| {
                catalog
                    .iter()
                    .find(|offered| offered.path == scope.path)
                    .is_some_and(|offered| {
                        scope.methods.retain(|m| offered.methods.contains(m));
                        !scope.methods.is_empty()
                    })
            });
        }
        auth::permits(&tokens, secret, &credentials.username, path, method)
    })
}

/// A state-changing request with Basic credentials needs no CSRF token
/// (docs/api.md), and a browser that cached those credentials attaches them
/// to a cross-site form too, with no Origin if the browser or an extension
/// drops it. A form's content type, or the browser's own word that the
/// request is cross-site, gives that away. Scripts send JSON or no body and
/// no `Sec-Fetch-Site`.
pub(super) fn basic_auth_cross_site(headers: &HeaderMap) -> bool {
    let basic = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("Basic "));
    let cross_site = headers
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|site| !matches!(site, "same-origin" | "none"));
    let form = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(|kind| kind.trim().to_ascii_lowercase())
        .is_some_and(|kind| {
            matches!(
                kind.as_str(),
                "application/x-www-form-urlencoded" | "multipart/form-data" | "text/plain"
            )
        });
    basic && (cross_site || form)
}

/// Whether `origin` is the console's own (`https://` and the request's Host) or one of
/// `csrf_allowed_origins`.
pub(super) fn origin_allowed(h: &Shared, headers: &HeaderMap, origin: &str) -> bool {
    let own = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(|host| format!("https://{host}"));
    if own.as_deref() == Some(origin) {
        return true;
    }
    let configured = h
        .config
        .read()
        .unwrap()
        .get("csrf_allowed_origins", "[]")
        .to_owned();
    butterpollo_config::config::Config::parse(&format!("csrf_allowed_origins = {configured}\n"))
        .map(|c| c.list("csrf_allowed_origins"))
        .unwrap_or_default()
        .iter()
        .any(|allowed| allowed == origin)
}

async fn issue_session(
    h: &Shared,
    username: String,
    remember_me: bool,
    headers: &HeaderMap,
    connection: &Connection,
    previous: Option<web_sessions::WebSession>,
) -> Result<(Response, String), Response> {
    let username = h
        .credentials
        .read()
        .unwrap()
        .as_ref()
        .filter(|c| c.username.eq_ignore_ascii_case(&username))
        .map(|c| c.username.clone())
        .unwrap_or(username);
    let ttl = h
        .config
        .read()
        .unwrap()
        .integer("session_token_ttl_seconds", 7200)
        .clamp(60, 604800) as u64;
    let user_agent: String = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .chars()
        .take(512)
        .collect();
    let peer_ip = connection.peer.ip().to_string();
    let host = h.clone();
    let created = tokio::task::spawn_blocking(move || {
        host.new_web_session(username, remember_me, user_agent, peer_ip, previous)
    })
    .await
    .unwrap_or_else(|e| Err(e.into()));
    let (access, refresh, csrf, refresh_ttl) = match created {
        Ok(tokens) => tokens,
        Err(e) => {
            if e.is::<web_sessions::Rotated>() {
                return Err(error(StatusCode::UNAUTHORIZED, "refresh token expired"));
            }
            tracing::error!(error=%e, "browser session could not be persisted");
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "browser session could not be saved",
            ));
        }
    };
    let ttl = ttl.min(refresh_ttl);
    let mut r = response(
        StatusCode::OK,
        json!({"status":true,"access_token":access,"refresh_token":refresh,"csrf_token":csrf,"expires_in":ttl,"refresh_expires_in":refresh_ttl,"remember_me":remember_me,"redirect":"/"}),
    );
    for (name, value, lifetime) in [
        ("__Host-apollo_session", access, ttl),
        ("__Host-apollo_refresh", refresh, refresh_ttl),
    ] {
        let expiry = if remember_me {
            format!("; Max-Age={lifetime}")
        } else {
            String::new()
        };
        r.headers_mut().append(
            header::SET_COOKIE,
            format!("{name}={value}; Path=/; HttpOnly; SameSite=Strict; Secure{expiry}")
                .parse()
                .unwrap(),
        );
    }
    Ok((r, csrf))
}

async fn issued(
    h: &Shared,
    username: String,
    remember_me: bool,
    headers: &HeaderMap,
    connection: &Connection,
    previous: Option<web_sessions::WebSession>,
) -> Response {
    match issue_session(h, username, remember_me, headers, connection, previous).await {
        Ok((r, _)) | Err(r) => r,
    }
}

async fn refresh_browser_with_csrf(
    h: &Shared,
    headers: &HeaderMap,
    connection: &Connection,
) -> Option<Result<(Response, String), Response>> {
    let token = cookie(headers, "__Host-apollo_refresh")?;
    let previous = h.find_refresh_session(&token)?;
    Some(
        issue_session(
            h,
            previous.username.clone(),
            previous.remember_me,
            headers,
            connection,
            Some(previous),
        )
        .await,
    )
}

pub(crate) async fn refresh_browser(
    h: &Shared,
    headers: &HeaderMap,
    connection: &Connection,
) -> Option<Response> {
    refresh_browser_with_csrf(h, headers, connection)
        .await
        .map(|res| match res {
            Ok((r, _)) | Err(r) => r,
        })
}

pub(super) async fn status(h: &Shared, headers: &HeaderMap, connection: &Connection) -> Response {
    let configured = h.credentials.read().unwrap().is_some();
    let authenticated = authenticated(h, headers, Some(connection.peer.ip())).await;
    // The web app asks this on load. A remembered browser whose access
    // cookie lapsed (after session_token_ttl_seconds) is renewed here, as
    // the console page does, instead of being sent to the login page.
    if configured
        && !authenticated
        && let Some(issued) = refresh_browser(h, headers, connection).await
        && issued.status().is_success()
    {
        let mut result = Json(json!({"authenticated":true,"credentials_configured":true,"login_required":false,"status":true})).into_response();
        for value in issued.headers().get_all(header::SET_COOKIE) {
            result
                .headers_mut()
                .append(header::SET_COOKIE, value.clone());
        }
        return result;
    }
    let mut status = json!({"authenticated":authenticated,"credentials_configured":configured,"login_required":configured&&!authenticated,"status":true});
    // The sign-in page on this PC shows how to set a new sign-in: an
    // administrator runs this program with --creds.
    if configured
        && !authenticated
        && butterpollo_stream::network::browser_on_this_pc(
            connection.peer,
            connection.local,
            headers,
        )
        && let Ok(program) = std::env::current_exe()
    {
        status["creds_program"] = program.display().to_string().into();
    }
    Json(status).into_response()
}

pub(super) async fn login(
    h: &Shared,
    method: &Method,
    headers: &HeaderMap,
    connection: &Connection,
    data: &Value,
) -> Response {
    if *method != Method::POST {
        return error(StatusCode::METHOD_NOT_ALLOWED, "POST required");
    }
    if !SIGN_INS
        .lock()
        .unwrap()
        .allowed(connection.peer.ip(), Instant::now())
    {
        return error(StatusCode::TOO_MANY_REQUESTS, "try again in one minute");
    }
    let text = |k: &str| data.get(k).and_then(Value::as_str).unwrap_or("");
    let username = text("username");
    let password = text("password");
    if sign_in(h, connection.peer.ip(), username, password).await {
        let remember = data.get("remember_me").is_some_and(|v| {
            v.as_bool()
                .unwrap_or_else(|| matches!(v.as_str(), Some("true" | "1" | "on")))
        });
        return issued(h, username.into(), remember, headers, connection, None).await;
    }
    error(StatusCode::UNAUTHORIZED, "invalid username or password")
}

pub(super) async fn refresh(
    h: &Shared,
    method: &Method,
    headers: &HeaderMap,
    connection: &Connection,
    data: &Value,
) -> Response {
    if *method != Method::POST {
        return error(StatusCode::METHOD_NOT_ALLOWED, "POST required");
    }
    let text = |k: &str| data.get(k).and_then(Value::as_str).unwrap_or("");
    let header_token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Refresh "))
        .map(str::to_owned);
    let cookie_token = header_token
        .is_none()
        .then(|| cookie(headers, "__Host-apollo_refresh"))
        .flatten();
    let from_cookie = cookie_token.is_some();
    let token = header_token
        .or(cookie_token)
        .unwrap_or_else(|| text("refresh_token").into());
    let previous = h.find_refresh_session(&token);
    // The browser sends the refresh cookie with any request to the console, so a refresh that
    // rests on it needs the page's CSRF token: the session's, or the anonymous one
    // `/api/csrf-token` hands a tab whose access cookie lapsed. A token sent in a header or the
    // body is a script's, which no other site can make the browser send.
    if from_cookie && !refresh_csrf_matches(headers, previous.as_ref().map(|s| s.csrf.as_str())) {
        return error(StatusCode::BAD_REQUEST, "CSRF token required");
    }
    match previous {
        Some(s) => {
            issued(
                h,
                s.username.clone(),
                s.remember_me,
                headers,
                connection,
                Some(s),
            )
            .await
        }
        None => error(StatusCode::UNAUTHORIZED, "refresh token expired"),
    }
}

/// Whether a cookie refresh carries the page's CSRF token: `session`'s, or the anonymous token in
/// `__Host-apollo_anon_csrf`.
fn refresh_csrf_matches(headers: &HeaderMap, session: Option<&str>) -> bool {
    let Some(got) = headers.get("X-CSRF-Token").and_then(|v| v.to_str().ok()) else {
        return false;
    };
    !got.is_empty()
        && [
            session.map(str::to_owned),
            cookie(headers, "__Host-apollo_anon_csrf"),
        ]
        .into_iter()
        .flatten()
        .any(|expected| crypto::equal(expected.as_bytes(), got.as_bytes()))
}

pub(super) async fn logout(h: &Shared, method: &Method, headers: &HeaderMap) -> Response {
    if *method != Method::POST {
        return error(StatusCode::METHOD_NOT_ALLOWED, "POST required");
    }
    if let Some(token) = access(headers) {
        let host = h.clone();
        let saved = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let mut sessions = host.web_sessions.lock().unwrap();
            let mut next = sessions.clone();
            if let Some(key) = web_sessions::resolve_hash(&next, &token) {
                next.remove(&key);
            }
            host.save_web_sessions(&next)?;
            *sessions = next;
            Ok(())
        })
        .await
        .unwrap_or_else(|e| Err(e.into()));
        if let Err(e) = saved {
            return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
        }
    }
    let mut r = Json(json!({"status":true})).into_response();
    for name in ["__Host-apollo_session", "__Host-apollo_refresh"] {
        r.headers_mut().append(
            header::SET_COOKIE,
            format!("{name}=; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=0")
                .parse()
                .unwrap(),
        );
    }
    r
}

pub(super) async fn csrf_token(
    h: &Shared,
    method: &Method,
    headers: &HeaderMap,
    connection: &Connection,
) -> Response {
    if *method != Method::GET {
        return error(StatusCode::METHOD_NOT_ALLOWED, "GET required");
    }
    let token = access(headers);
    let csrf = token
        .and_then(|t| {
            web_sessions::find(&h.web_sessions.lock().unwrap(), &t)
                .filter(|s| s.expires > Instant::now())
                .map(|s| s.csrf.clone())
        })
        .unwrap_or_default();
    if !csrf.is_empty() {
        return Json(json!({"status":true,"csrf_token":csrf,"token":csrf})).into_response();
    }
    let configured = h.credentials.read().unwrap().is_some();
    if configured
        && let Some(Ok((issued, csrf))) = refresh_browser_with_csrf(h, headers, connection).await
    {
        let mut result =
            Json(json!({"status":true,"csrf_token":csrf,"token":csrf})).into_response();
        for value in issued.headers().get_all(header::SET_COOKIE) {
            result
                .headers_mut()
                .append(header::SET_COOKIE, value.clone());
        }
        return result;
    }
    let csrf = hex::encode(crypto::random::<32>());
    let mut result = Json(json!({"status":true,"csrf_token":csrf,"token":csrf})).into_response();
    result.headers_mut().insert(
        header::SET_COOKIE,
        format!("__Host-apollo_anon_csrf={csrf}; Path=/; HttpOnly; SameSite=Strict; Secure; Max-Age=3600")
            .parse()
            .unwrap(),
    );
    result
}

pub(super) fn list_sessions(h: &Shared, headers: &HeaderMap) -> Value {
    let sessions = h.web_sessions.lock().unwrap();
    let current = web_sessions::resolve_hash(&sessions, &access(headers).unwrap_or_default());
    let sessions: Vec<_> = sessions
        .iter()
        .filter(|(_, s)| s.refresh_expires > Instant::now())
        .map(|(token, s)| {
            let mut row = web_sessions::record(token, s);
            let object = row.as_object_mut().unwrap();
            object.remove("refresh_token_hash");
            object.remove("rotation_id");
            object.remove(web_sessions::SIGN_IN_ID);
            object.remove("hash");
            object.insert("id".into(), json!(token));
            object.insert(
                "current".into(),
                json!(
                    current
                        .as_ref()
                        .is_some_and(|current| crypto::equal(token.as_bytes(), current.as_bytes()))
                ),
            );
            row
        })
        .collect();
    json!({"status":true,"sessions":sessions})
}

pub(super) fn delete_session(h: &Shared, hash: &str) -> anyhow::Result<Value> {
    let mut sessions = h.web_sessions.lock().unwrap();
    let mut next = sessions.clone();
    next.retain(|token, _| !token.eq_ignore_ascii_case(hash));
    h.save_web_sessions(&next)?;
    *sessions = next;
    Ok(json!({"status":true,"deleted":true}))
}

pub(super) fn change_password(h: &Shared, data: &Value) -> anyhow::Result<Value> {
    let text = |k: &str| data.get(k).and_then(Value::as_str).unwrap_or("");
    let previous = h.credentials.read().unwrap().clone();
    if let Some(c) = &previous
        && !c.verifies(text("currentUsername"), text("currentPassword"))
    {
        anyhow::bail!("current credentials do not match");
    }
    let password = text("newPassword");
    if password != text("confirmNewPassword") {
        anyhow::bail!("password confirmation does not match");
    }
    let c = Credentials::new(text("newUsername").into(), password)?;
    // Invalidate durable sessions and API tokens before changing
    // the password. A failed credential write may require another
    // login; it cannot resurrect what the previous password let in.
    let mut sessions = h.web_sessions.lock().unwrap();
    h.revoke_sign_ins()?;
    sessions.clear();
    h.save_credentials(&c)?;
    *h.credentials.write().unwrap() = Some(c);
    if let Some(previous) = &previous {
        BASIC_CACHE.lock().unwrap().forget(previous);
    }
    Ok(json!({"status":true}))
}
