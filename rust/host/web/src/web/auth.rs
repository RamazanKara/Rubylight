//! Browser session authentication, sign-in rate limiting, CSRF checks, and auth/password endpoints.
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
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

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

/// Sign-in attempts per address over the last minute. The login form and
/// Basic credentials share it, so neither lets a peer guess the password at
/// full speed. This PC is never throttled, and IPv6 counts per /64, the
/// block one LAN host can take addresses from.
pub(super) struct SignIns {
    pub(super) attempts: std::collections::HashMap<std::net::IpAddr, (Instant, u32)>,
}

static SIGN_INS: std::sync::LazyLock<std::sync::Mutex<SignIns>> = std::sync::LazyLock::new(|| {
    std::sync::Mutex::new(SignIns {
        attempts: Default::default(),
    })
});

impl SignIns {
    const LIMIT: u32 = 10;
    const WINDOW: Duration = Duration::from_secs(60);
    pub(super) const TRACKED: usize = 1024;
    fn key(peer: std::net::IpAddr) -> std::net::IpAddr {
        let peer = peer.to_canonical();
        match peer {
            std::net::IpAddr::V6(address) if !address.is_loopback() => {
                std::net::Ipv6Addr::from(u128::from(address) & (u128::MAX << 64)).into()
            }
            other => other,
        }
    }
    /// Whether `peer` may attempt sign-in without counting against the limit.
    pub(super) fn allowed(&mut self, peer: std::net::IpAddr, now: Instant) -> bool {
        let key = Self::key(peer);
        self.attempts
            .retain(|_, (since, _)| now.saturating_duration_since(*since) < Self::WINDOW);
        self.attempts
            .get(&key)
            .is_none_or(|(_, count)| *count < Self::LIMIT)
    }
    /// Record a failed sign-in attempt for `peer`.
    pub(super) fn failed(&mut self, peer: std::net::IpAddr, now: Instant) {
        let key = Self::key(peer);
        self.attempts
            .retain(|_, (since, _)| now.saturating_duration_since(*since) < Self::WINDOW);
        // Full: forget the oldest address instead of refusing every new
        // one, which would let a flood of addresses lock everyone out.
        if self.attempts.len() >= Self::TRACKED
            && !self.attempts.contains_key(&key)
            && let Some(oldest) = self
                .attempts
                .iter()
                .min_by_key(|(_, (since, _))| *since)
                .map(|(address, _)| *address)
        {
            self.attempts.remove(&oldest);
        }
        let (_, count) = self.attempts.entry(key).or_insert((now, 0));
        *count = count.saturating_add(1);
    }
    pub(super) fn succeeded(&mut self, peer: std::net::IpAddr) {
        let key = Self::key(peer);
        self.attempts.remove(&key);
    }
}

/// Basic credentials that passed PBKDF2 in the last five minutes, so a
/// script that sends them with every request is not slowed (and the host
/// not loaded) by 600,000 rounds each time. A key binds the stored username,
/// salt and hash, so it never matches once the password or its hash changes.
pub(super) struct BasicCache {
    /// Key: when it was verified, and the stored credentials it was checked against.
    pub(super) entries: std::collections::HashMap<[u8; 32], (Instant, [u8; 32])>,
}

pub(super) static BASIC_CACHE: std::sync::LazyLock<std::sync::Mutex<BasicCache>> =
    std::sync::LazyLock::new(|| {
        std::sync::Mutex::new(BasicCache {
            entries: Default::default(),
        })
    });

/// Serializes uncached PBKDF2 checks so concurrent bursts with the same valid
/// Basic credentials verify once and coalesce onto [`BASIC_CACHE`] instead of
/// tripping the failed-attempt limiter while the first hash is in flight.
static SIGN_IN_VERIFY: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

impl BasicCache {
    pub(super) const TTL: Duration = Duration::from_secs(300);
    pub(super) const CAPACITY: usize = 64;
    fn digest(parts: &[&[u8]]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        for part in parts {
            hasher.update((part.len() as u64).to_le_bytes());
            hasher.update(part);
        }
        hasher.finalize().into()
    }
    /// The stored credentials, without the attempt.
    fn owner(stored: &Credentials) -> [u8; 32] {
        Self::digest(&[
            stored.username.as_bytes(),
            stored.salt.as_bytes(),
            stored.password.as_bytes(),
        ])
    }
    /// The attempt `user`/`pass` against `stored`; usernames compare as
    /// [`Credentials::verifies`] does.
    pub(super) fn key(stored: &Credentials, user: &str, pass: &str) -> [u8; 32] {
        Self::digest(&[
            &Self::owner(stored),
            user.trim().to_ascii_lowercase().as_bytes(),
            pass.as_bytes(),
        ])
    }
    fn expire(&mut self, now: Instant) {
        self.entries
            .retain(|_, (since, _)| now.saturating_duration_since(*since) < Self::TTL);
    }
    /// Whether `key` was verified within the last [`Self::TTL`].
    pub(super) fn hit(&mut self, key: &[u8; 32], now: Instant) -> bool {
        self.expire(now);
        self.entries.contains_key(key)
    }
    /// Remember that `key` passed against `stored` at `now`; full, the oldest entry goes.
    pub(super) fn insert(&mut self, key: [u8; 32], stored: &Credentials, now: Instant) {
        self.expire(now);
        if self.entries.len() >= Self::CAPACITY
            && !self.entries.contains_key(&key)
            && let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (since, _))| *since)
                .map(|(key, _)| *key)
        {
            self.entries.remove(&oldest);
        }
        self.entries.insert(key, (now, Self::owner(stored)));
    }
    /// Drop every attempt verified against `stored`, which was just replaced.
    pub(super) fn forget(&mut self, stored: &Credentials) {
        let owner = Self::owner(stored);
        self.entries.retain(|_, (_, of)| *of != owner);
    }
}

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
    let Some(stored) = h.credentials.read().unwrap().clone() else {
        SIGN_INS.lock().unwrap().failed(peer, now);
        return false;
    };
    let key = BasicCache::key(&stored, user, pass);
    if !stored.needs_rehash() && BASIC_CACHE.lock().unwrap().hit(&key, now) {
        SIGN_INS.lock().unwrap().succeeded(peer);
        return true;
    }
    let (user, pass) = (user.to_owned(), pass.to_owned());
    let verified = tokio::task::spawn_blocking({
        let stored = stored.clone();
        let pass = pass.clone();
        move || stored.verifies(&user, &pass)
    })
    .await
    .unwrap_or(false);
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
            if let Err(error) = h.save_web_sessions(&sessions) {
                sessions.get_mut(&key).unwrap().last_seen = previous;
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

fn issued(
    h: &Shared,
    username: String,
    remember_me: bool,
    headers: &HeaderMap,
    connection: &Connection,
    previous: Option<web_sessions::WebSession>,
) -> Response {
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
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .chars()
        .take(512)
        .collect();
    let (access, refresh, csrf, refresh_ttl) = match h.new_web_session(
        username,
        remember_me,
        user_agent,
        connection.peer.ip().to_string(),
        previous,
    ) {
        Ok(tokens) => tokens,
        Err(e) => {
            if e.is::<web_sessions::Rotated>() {
                return error(StatusCode::UNAUTHORIZED, "refresh token expired");
            }
            tracing::error!(error=%e, "browser session could not be persisted");
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "browser session could not be saved",
            );
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
    r
}

pub(crate) fn refresh_browser(
    h: &Shared,
    headers: &HeaderMap,
    connection: &Connection,
) -> Option<Response> {
    let token = cookie(headers, "__Host-apollo_refresh")?;
    let previous = h
        .web_sessions
        .lock()
        .unwrap()
        .values()
        .find(|s| {
            crypto::matches_hash(token.as_bytes(), &s.refresh) && s.refresh_expires > Instant::now()
        })
        .cloned()?;
    Some(issued(
        h,
        previous.username.clone(),
        previous.remember_me,
        headers,
        connection,
        Some(previous),
    ))
}

pub(super) async fn status(h: &Shared, headers: &HeaderMap, connection: &Connection) -> Response {
    let configured = h.credentials.read().unwrap().is_some();
    let authenticated = authenticated(h, headers, Some(connection.peer.ip())).await;
    // The web app asks this on load. A remembered browser whose access
    // cookie lapsed (after session_token_ttl_seconds) is renewed here, as
    // the console page does, instead of being sent to the login page.
    if configured
        && !authenticated
        && let Some(issued) = refresh_browser(h, headers, connection)
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
        return issued(h, username.into(), remember, headers, connection, None);
    }
    error(StatusCode::UNAUTHORIZED, "invalid username or password")
}

pub(super) fn refresh(
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
    let previous = {
        let sessions = h.web_sessions.lock().unwrap();
        sessions
            .iter()
            .find(|(_, s)| {
                crypto::matches_hash(token.as_bytes(), &s.refresh)
                    && s.refresh_expires > Instant::now()
            })
            .map(|(_, s)| s.clone())
    };
    // The browser sends the refresh cookie with any request to the console, so a refresh that
    // rests on it needs the page's CSRF token: the session's, or the anonymous one
    // `/api/csrf-token` hands a tab whose access cookie lapsed. A token sent in a header or the
    // body is a script's, which no other site can make the browser send.
    if from_cookie && !refresh_csrf_matches(headers, previous.as_ref().map(|s| s.csrf.as_str())) {
        return error(StatusCode::BAD_REQUEST, "CSRF token required");
    }
    match previous {
        Some(s) => issued(
            h,
            s.username.clone(),
            s.remember_me,
            headers,
            connection,
            Some(s),
        ),
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

pub(super) fn logout(h: &Shared, headers: &HeaderMap) -> Response {
    if let Some(token) = access(headers) {
        let mut sessions = h.web_sessions.lock().unwrap();
        let mut next = sessions.clone();
        if let Some(key) = web_sessions::resolve_hash(&next, &token) {
            next.remove(&key);
        }
        if let Err(e) = h.save_web_sessions(&next) {
            return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
        }
        *sessions = next;
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

pub(super) fn csrf_token(h: &Shared, method: &Method, headers: &HeaderMap) -> Response {
    if *method != Method::GET {
        return error(StatusCode::METHOD_NOT_ALLOWED, "GET required");
    }
    let token = access(headers);
    let csrf = token
        .and_then(|t| {
            web_sessions::find(&h.web_sessions.lock().unwrap(), &t).map(|s| s.csrf.clone())
        })
        .unwrap_or_default();
    if !csrf.is_empty() {
        return Json(json!({"status":true,"csrf_token":csrf,"token":csrf})).into_response();
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
