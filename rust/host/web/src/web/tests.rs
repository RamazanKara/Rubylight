//! Unit tests for the web console router, CSRF protections, and static asset confinement.
use super::*;

#[test]
fn cover_requests_name_their_app_and_short_paths_do_not_panic() {
    assert_eq!(super::cover_id("/api/apps/42/cover"), Some("42"));
    assert_eq!(super::cover_id("/api/covers/7"), Some("7"));
    assert_eq!(super::cover_id("/api/apps/cover"), None);
    assert_eq!(super::cover_id("/api/apps//cover"), None);
    assert_eq!(super::cover_id("/api/covers/"), None);
    assert_eq!(super::icon_id("/api/apps/abc-1/icon"), Some("abc-1"));
    assert_eq!(super::icon_id("/api/apps/icon"), None);
    assert_eq!(super::icon_id("/api/apps//icon"), None);
    assert_eq!(super::icon_id("/api/apps/a/b/icon"), None);
}

#[test]
fn sign_ins_throttle_only_failed_attempts_including_loopback_peers() {
    use std::net::IpAddr;
    let fresh = || super::SignIns {
        attempts: Default::default(),
    };
    let now = Instant::now();
    let lan: IpAddr = "192.168.1.20".parse().unwrap();
    let mut sign_ins = fresh();
    // Pre-checks without recording failures never lock out the address.
    for _ in 0..50 {
        assert!(sign_ins.allowed(lan, now));
    }
    for _ in 0..10 {
        assert!(sign_ins.allowed(lan, now));
        sign_ins.failed(lan, now);
    }
    assert!(
        !sign_ins.allowed(lan, now),
        "the 11th attempt after 10 failures"
    );
    assert!(sign_ins.allowed("192.168.1.21".parse().unwrap(), now));
    assert!(
        sign_ins.allowed(lan, now + Duration::from_secs(61)),
        "a minute later"
    );
    sign_ins.succeeded(lan);
    assert!(!sign_ins.attempts.contains_key(&lan));
    // Loopback (including internet-play tunnel peers) is also throttled on repeated failures,
    // while canonical v4-mapped IPv6 maps to 127.0.0.1.
    for local in ["127.0.0.1", "::1"] {
        let mut s = fresh();
        let addr: IpAddr = local.parse().unwrap();
        for _ in 0..10 {
            assert!(s.allowed(addr, now), "{local}");
            s.failed(addr, now);
        }
        assert!(!s.allowed(addr, now), "{local} blocked after 10 failures");
    }
    let mut s = fresh();
    for _ in 0..10 {
        s.failed("::ffff:127.0.0.1".parse().unwrap(), now);
    }
    assert!(!s.allowed("127.0.0.1".parse().unwrap(), now));
    // One LAN host can take any address in its /64.
    let mut sign_ins = fresh();
    for host in 0..10 {
        let ip: IpAddr = format!("fe80::{host:x}").parse().unwrap();
        assert!(sign_ins.allowed(ip, now));
        sign_ins.failed(ip, now);
    }
    assert!(!sign_ins.allowed("fe80::abcd".parse().unwrap(), now));
    assert!(sign_ins.allowed("fd00:1:2:3::1".parse().unwrap(), now));
    // A full table forgets the oldest address instead of refusing new ones.
    let mut sign_ins = fresh();
    for host in 0..super::SignIns::TRACKED as u32 {
        let address = IpAddr::from(std::net::Ipv4Addr::from(0x0a00_0000 + host));
        sign_ins.failed(address, now + Duration::from_millis(u64::from(host)));
    }
    let newcomer: IpAddr = "172.16.0.1".parse().unwrap();
    assert!(sign_ins.allowed(newcomer, now + Duration::from_secs(2)));
    sign_ins.failed(newcomer, now + Duration::from_secs(2));
    assert_eq!(sign_ins.attempts.len(), super::SignIns::TRACKED);
    assert!(
        !sign_ins
            .attempts
            .contains_key(&"10.0.0.0".parse::<IpAddr>().unwrap())
    );
}

#[tokio::test]
async fn a_new_console_password_revokes_the_api_tokens() {
    use axum::{Extension, body::Body, http::Request};
    use tower::ServiceExt;
    let f = butterpollo_stream::state::test_support::Fixture::new();
    f.host.aliases.lock().unwrap()["root"]["api_tokens"] = json!([{"hash": "leaked"}]);
    let request = Request::builder()
        .method("POST")
        .uri("/api/password")
        .header("Content-Type", "application/json")
        .body(Body::from(
            json!({"newUsername": "user", "newPassword": "new-pass", "confirmNewPassword": "new-pass"})
                .to_string(),
        ))
        .unwrap();
    let response = router(f.host.clone())
        .layer(Extension(f.connection(true)))
        .oneshot(request)
        .await
        .unwrap();
    crate::test_support::value(response, StatusCode::OK).await;
    assert!(f.host.credentials.read().unwrap().is_some());
    assert_eq!(
        f.host.aliases.lock().unwrap()["root"]["api_tokens"],
        json!([])
    );
}

/// A launch holds this lock from its check of the running game to its start of one: closing
/// the game from the console waits for it, as Terminate and the tray do.
#[test]
fn closing_the_game_from_the_console_waits_for_a_launch_in_progress() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let app = butterpollo_session::state::App::desktop();
    *f.host.current_app.lock().unwrap() = Some(
        butterpollo_stream::process::RunningApp::with_environment(&app, Default::default())
            .unwrap(),
    );
    let launching = f.host.launch_transition.lock().unwrap();
    let host = f.host.clone();
    let close = std::thread::spawn(move || {
        apps::handle(&host, "POST", "/api/apps/close", &json!({})).unwrap();
    });
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(
        f.host.current_app.lock().unwrap().is_some(),
        "closed during a launch"
    );
    drop(launching);
    close.join().unwrap();
    assert!(f.host.current_app.lock().unwrap().is_none());
}

#[tokio::test]
async fn basic_credentials_do_not_carry_a_cross_site_form() {
    use axum::{Extension, body::Body, http::Request};
    use tower::ServiceExt;
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let credentials = Credentials::new("user".into(), "password1").unwrap();
    f.host.save_credentials(&credentials).unwrap();
    *f.host.credentials.write().unwrap() = Some(credentials);
    let send = |headers: &[(&str, &str)]| {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/api/apps/close")
            .header(header::AUTHORIZATION, "Basic dXNlcjpwYXNzd29yZDE=");
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        router(f.host.clone())
            .layer(Extension(f.connection(true)))
            .oneshot(builder.body(Body::from("{}")).unwrap())
    };
    for refused in [
        &[("Content-Type", "text/plain")][..],
        &[(
            "Content-Type",
            "application/x-www-form-urlencoded; charset=utf-8",
        )],
        &[("Content-Type", "Multipart/Form-Data; boundary=x")],
        &[
            ("Content-Type", "application/json"),
            ("Sec-Fetch-Site", "cross-site"),
        ],
    ] {
        let response = send(refused).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{refused:?}");
    }
    // A script, and the console's own page, still get through.
    for allowed in [
        &[("Content-Type", "application/json")][..],
        &[],
        &[
            ("Content-Type", "application/json"),
            ("Sec-Fetch-Site", "same-origin"),
        ],
    ] {
        let response = send(allowed).await.unwrap();
        assert_ne!(response.status(), StatusCode::FORBIDDEN, "{allowed:?}");
    }
    assert!(!basic_auth_cross_site(&HeaderMap::new()));
}

#[test]
fn web_app_files_stay_in_their_folder() {
    let base = std::env::temp_dir().join(format!("butterpollo-web-{}", uuid::Uuid::new_v4()));
    let root = base.join("web");
    std::fs::create_dir_all(root.join("assets")).unwrap();
    std::fs::write(root.join("index.html"), "<html></html>").unwrap();
    std::fs::write(root.join("assets/index-1.js"), "export {}").unwrap();
    std::fs::write(base.join("outside.txt"), "x").unwrap();
    let get = |path: &str| app_file(&root, &Method::GET, path, APP_POLICY);
    let header = |response: &Response, name: header::HeaderName| {
        response
            .headers()
            .get(name)
            .map(|v| v.to_str().unwrap().to_owned())
    };

    let script = get("/assets/index-1.js");
    assert_eq!(script.status(), StatusCode::OK);
    assert!(
        header(&script, header::CACHE_CONTROL)
            .unwrap()
            .contains("immutable")
    );
    assert!(header(&script, header::CONTENT_SECURITY_POLICY).is_none());

    let page = get("/devices");
    assert_eq!(page.status(), StatusCode::OK);
    assert!(
        header(&page, header::CONTENT_TYPE)
            .unwrap()
            .starts_with("text/html")
    );
    assert!(header(&page, header::CACHE_CONTROL).is_none());
    assert_eq!(
        header(&page, header::CONTENT_SECURITY_POLICY).as_deref(),
        Some(APP_POLICY)
    );

    for escape in [
        "/../outside.txt",
        "/assets/../../outside.txt",
        "/C:/Windows/win.ini",
    ] {
        let response = get(escape);
        assert_ne!(
            header(&response, header::CONTENT_TYPE).as_deref(),
            Some("application/octet-stream"),
            "{escape}"
        );
    }
    assert_eq!(get("/assets/index-0.js").status(), StatusCode::NOT_FOUND);
    assert_eq!(
        app_file(&root, &Method::POST, "/devices", APP_POLICY).status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    std::fs::remove_dir_all(&base).ok();
}

#[tokio::test]
async fn login_and_basic_auth_rehash_legacy_credentials_on_signin() {
    use axum::{Extension, body::Body, http::Request};
    use butterpollo_crypto::crypto;
    use tower::ServiceExt;

    let legacy_creds = |user: &str, pass: &str| {
        let salt = "0123456789abcdef0123456789abcdef".to_owned();
        let digest = hex::encode(crypto::hash(format!("{pass}{salt}").as_bytes()));
        Credentials {
            username: user.to_owned(),
            password: digest,
            salt,
        }
    };

    // Browser sign-in (`POST /api/auth/login`) transparently upgrades legacy hashes to PBKDF2-HMAC-SHA256.
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let legacy = legacy_creds("user", "password1");
    f.host.save_credentials(&legacy).unwrap();
    *f.host.credentials.write().unwrap() = Some(legacy);
    let response = router(f.host.clone())
        .layer(Extension(f.connection(true)))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"username": "user", "password": "password1", "remember_me": false})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let saved = f.host.credentials.read().unwrap().clone().unwrap();
    assert!(saved.password.starts_with("pbkdf2-sha256$600000$"));
    assert!(!saved.needs_rehash());
    assert!(saved.verifies("user", "password1"));

    // HTTP Basic auth also upgrades legacy hashes transparently.
    let f2 = butterpollo_stream::state::test_support::Fixture::new();
    let legacy2 = legacy_creds("user", "password1");
    f2.host.save_credentials(&legacy2).unwrap();
    *f2.host.credentials.write().unwrap() = Some(legacy2);
    let response2 = router(f2.host.clone())
        .layer(Extension(f2.connection(true)))
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/config")
                .header(header::AUTHORIZATION, "Basic dXNlcjpwYXNzd29yZDE=")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response2.status(), StatusCode::OK);
    let saved2 = f2.host.credentials.read().unwrap().clone().unwrap();
    assert!(saved2.password.starts_with("pbkdf2-sha256$600000$"));
    assert!(!saved2.needs_rehash());
    assert!(saved2.verifies("user", "password1"));
}

#[test]
fn basic_cache_binds_the_stored_hash_and_expires_and_stays_bounded() {
    let stored = Credentials {
        username: "User".into(),
        password: "pbkdf2-sha256$600000$aa".into(),
        salt: "salt-a".into(),
    };
    let key = BasicCache::key(&stored, "user", "secret-pass");
    // Usernames compare as `Credentials::verifies` does; the password exactly.
    assert_eq!(key, BasicCache::key(&stored, " USER ", "secret-pass"));
    assert_ne!(key, BasicCache::key(&stored, "user", "secret-pasS"));
    // A new salt, hash or username never matches an old entry.
    for changed in [
        Credentials {
            salt: "salt-b".into(),
            ..stored.clone()
        },
        Credentials {
            password: "pbkdf2-sha256$600000$bb".into(),
            ..stored.clone()
        },
        Credentials {
            username: "other".into(),
            ..stored.clone()
        },
    ] {
        assert_ne!(key, BasicCache::key(&changed, "user", "secret-pass"));
    }
    // Length-prefixed: moving bytes between fields changes the key.
    let shifted = Credentials {
        username: "Use".into(),
        salt: "rsalt-a".into(),
        ..stored.clone()
    };
    assert_ne!(key, BasicCache::key(&shifted, "user", "secret-pass"));

    let mut cache = BasicCache {
        entries: Default::default(),
    };
    let now = Instant::now();
    assert!(!cache.hit(&key, now));
    cache.insert(key, &stored, now);
    assert!(cache.hit(&key, now + BasicCache::TTL - Duration::from_secs(1)));
    assert!(!cache.hit(&key, now + BasicCache::TTL), "expired");
    assert!(cache.entries.is_empty());

    // Full: the oldest entry goes.
    for i in 0..BasicCache::CAPACITY as u64 {
        let key = BasicCache::key(&stored, "user", &i.to_string());
        cache.insert(key, &stored, now + Duration::from_millis(i));
    }
    let late = now + Duration::from_secs(1);
    cache.insert(key, &stored, late);
    assert_eq!(cache.entries.len(), BasicCache::CAPACITY);
    assert!(!cache.hit(&BasicCache::key(&stored, "user", "0"), late));
    assert!(cache.hit(&BasicCache::key(&stored, "user", "1"), late));
    assert!(cache.hit(&key, late));

    // Replacing the credentials forgets every attempt checked against them.
    let other = Credentials {
        salt: "salt-b".into(),
        ..stored.clone()
    };
    let other_key = BasicCache::key(&other, "user", "secret-pass");
    cache.insert(other_key, &other, late);
    cache.forget(&stored);
    assert_eq!(cache.entries.len(), 1);
    assert!(cache.hit(&other_key, late));
}

#[tokio::test]
async fn basic_auth_reuses_a_recent_verification_until_the_password_changes() {
    use base64::Engine;
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let stored = Credentials::new("cache-user".into(), "right-password").unwrap();
    f.host.save_credentials(&stored).unwrap();
    *f.host.credentials.write().unwrap() = Some(stored.clone());
    // A documentation address of its own, so no other test shares its sign-in count.
    let peer: std::net::IpAddr = "198.51.100.71".parse().unwrap();
    let basic = |user: &str, pass: &str| {
        let mut headers = HeaderMap::new();
        let encoded =
            base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}").as_bytes());
        headers.insert(
            header::AUTHORIZATION,
            format!("Basic {encoded}").parse().unwrap(),
        );
        headers
    };

    // A real verification is remembered.
    assert!(authenticated(&f.host, &basic("cache-user", "right-password"), Some(peer)).await);
    let key = BasicCache::key(&stored, "cache-user", "right-password");
    assert!(BASIC_CACHE.lock().unwrap().hit(&key, Instant::now()));
    assert!(!authenticated(&f.host, &basic("cache-user", "wrong-password"), Some(peer)).await);

    // A remembered attempt passes without PBKDF2: this password would fail it.
    let unverified = BasicCache::key(&stored, "Cache-User", "never-verified");
    BASIC_CACHE
        .lock()
        .unwrap()
        .insert(unverified, &stored, Instant::now());
    assert!(authenticated(&f.host, &basic("cache-user", "never-verified"), Some(peer)).await);

    // A new password (same text, new salt) forgets both.
    super::auth::change_password(
        &f.host,
        &json!({
            "currentUsername": "cache-user",
            "currentPassword": "right-password",
            "newUsername": "cache-user",
            "newPassword": "right-password",
            "confirmNewPassword": "right-password",
        }),
    )
    .unwrap();
    assert!(!BASIC_CACHE.lock().unwrap().hit(&key, Instant::now()));
    assert!(!BASIC_CACHE.lock().unwrap().hit(&unverified, Instant::now()));
    assert!(!authenticated(&f.host, &basic("cache-user", "never-verified"), Some(peer)).await);
    assert!(authenticated(&f.host, &basic("cache-user", "right-password"), Some(peer)).await);
}

#[tokio::test]
async fn parallel_valid_basic_requests_coalesce_and_parallel_wrong_guesses_stop_at_limit() {
    use base64::Engine;
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let stored = Credentials::new("burst-user".into(), "right-password").unwrap();
    f.host.save_credentials(&stored).unwrap();
    *f.host.credentials.write().unwrap() = Some(stored.clone());
    let basic = |user: &str, pass: &str| {
        let mut headers = HeaderMap::new();
        let encoded =
            base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}").as_bytes());
        headers.insert(
            header::AUTHORIZATION,
            format!("Basic {encoded}").parse().unwrap(),
        );
        headers
    };

    // 15 concurrent valid Basic requests on a cold cache must all pass via SIGN_IN_VERIFY + cache.
    let valid_peer: std::net::IpAddr = "198.51.100.72".parse().unwrap();
    let valid_headers = basic("burst-user", "right-password");
    let mut valid_set = tokio::task::JoinSet::new();
    for _ in 0..15 {
        let host = f.host.clone();
        let headers = valid_headers.clone();
        valid_set.spawn(async move { authenticated(&host, &headers, Some(valid_peer)).await });
    }
    while let Some(res) = valid_set.join_next().await {
        assert!(res.unwrap(), "concurrent valid Basic request failed");
    }
    let key = BasicCache::key(&stored, "burst-user", "right-password");
    assert!(BASIC_CACHE.lock().unwrap().hit(&key, Instant::now()));

    // 15 concurrent wrong guesses serialize against the 10-attempt limit and lock out the peer.
    let brute_peer: std::net::IpAddr = "198.51.100.73".parse().unwrap();
    let wrong_headers = basic("burst-user", "wrong-password");
    let mut brute_set = tokio::task::JoinSet::new();
    for _ in 0..15 {
        let host = f.host.clone();
        let headers = wrong_headers.clone();
        brute_set.spawn(async move { authenticated(&host, &headers, Some(brute_peer)).await });
    }
    while let Some(res) = brute_set.join_next().await {
        assert!(!res.unwrap(), "wrong password must be rejected");
    }
    assert!(
        !authenticated(&f.host, &valid_headers, Some(brute_peer)).await,
        "peer must remain locked out after hitting SignIns::LIMIT"
    );
}

/// One request to the console from this PC's loopback, with `headers` and a JSON `body`.
async fn console_request(
    f: &butterpollo_stream::state::test_support::Fixture,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> axum::response::Response {
    use axum::{Extension, body::Body, http::Request};
    use tower::ServiceExt;
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header("Content-Type", "application/json");
    }
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let body = body.map_or_else(Body::empty, |body| Body::from(body.to_string()));
    router(f.host.clone())
        .layer(Extension(f.connection(true)))
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap()
}

fn signed_up(f: &butterpollo_stream::state::test_support::Fixture) {
    let credentials = Credentials::new("user".into(), "password1").unwrap();
    f.host.save_credentials(&credentials).unwrap();
    *f.host.credentials.write().unwrap() = Some(credentials);
}

#[tokio::test]
async fn a_first_password_through_a_proxy_on_this_pc_is_refused() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let password =
        json!({"newUsername": "user", "newPassword": "new-pass", "confirmNewPassword": "new-pass"});
    // nginx, cloudflared or `tailscale serve` on this PC connect over loopback for a browser
    // anywhere; their headers give them away.
    for proxied in [
        &[("X-Forwarded-For", "203.0.113.9")][..],
        &[("Forwarded", "for=203.0.113.9;proto=https")],
        &[("Host", "console.example.com")],
        &[("Host", "localhost:443")],
    ] {
        let response =
            console_request(&f, "POST", "/api/password", proxied, Some(password.clone())).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{proxied:?}");
        assert!(f.host.credentials.read().unwrap().is_none(), "{proxied:?}");
    }
    // A browser on this PC still sets it.
    let response = console_request(
        &f,
        "POST",
        "/api/password",
        &[("Host", "localhost:47984")],
        Some(password),
    )
    .await;
    crate::test_support::value(response, StatusCode::OK).await;
    assert!(f.host.credentials.read().unwrap().is_some());
}

#[tokio::test]
async fn a_cookie_refresh_needs_the_pages_csrf_token() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    signed_up(&f);
    let login = console_request(
        &f,
        "POST",
        "/api/auth/login",
        &[],
        Some(json!({"username": "user", "password": "password1"})),
    )
    .await;
    let session = crate::test_support::value(login, StatusCode::OK).await;
    let refresh_cookie = |s: &Value| {
        format!(
            "__Host-apollo_refresh={}",
            s["refresh_token"].as_str().unwrap()
        )
    };
    // The cookie alone, as a form on another site makes the browser send it, is refused.
    for headers in [
        vec![("Cookie", refresh_cookie(&session))],
        vec![
            ("Cookie", refresh_cookie(&session)),
            ("X-CSRF-Token", "not-the-token".into()),
        ],
    ] {
        let headers: Vec<(&str, &str)> = headers.iter().map(|(n, v)| (*n, v.as_str())).collect();
        let response = console_request(&f, "POST", "/api/auth/refresh", &headers, None).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{headers:?}");
    }
    // With the session's token it renews.
    let csrf = session["csrf_token"].as_str().unwrap().to_owned();
    let cookie = refresh_cookie(&session);
    let response = console_request(
        &f,
        "POST",
        "/api/auth/refresh",
        &[("Cookie", &cookie), ("X-CSRF-Token", &csrf)],
        None,
    )
    .await;
    let renewed = crate::test_support::value(response, StatusCode::OK).await;
    // A reloaded tab has only the anonymous token /api/csrf-token gave it, with its cookie.
    let cookie = format!(
        "{}; __Host-apollo_anon_csrf=anonymous-token",
        refresh_cookie(&renewed)
    );
    let response = console_request(
        &f,
        "POST",
        "/api/auth/refresh",
        &[("Cookie", &cookie), ("X-CSRF-Token", "anonymous-token")],
        None,
    )
    .await;
    let renewed = crate::test_support::value(response, StatusCode::OK).await;
    // A script that sends its refresh token itself needs none: no other site can send it.
    let authorization = format!("Refresh {}", renewed["refresh_token"].as_str().unwrap());
    let response = console_request(
        &f,
        "POST",
        "/api/auth/refresh",
        &[("Authorization", &authorization)],
        None,
    )
    .await;
    crate::test_support::value(response, StatusCode::OK).await;
}

#[tokio::test]
async fn api_tokens_reach_neither_the_password_nor_the_file_picker() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    signed_up(&f);
    let catalog = token_catalog();
    assert!(
        !catalog
            .iter()
            .any(|s| s.path == "/api/password" || s.path == "/api/browse")
    );
    // A token saved while both were offered loses them and keeps the rest.
    let scope = |path: &str, method: &str| crypto_auth::Scope {
        path: path.into(),
        methods: vec![method.into()],
    };
    let mut offered_before = catalog.clone();
    offered_before.extend([scope("/api/password", "POST"), scope("/api/browse", "GET")]);
    let (secret, token) = crypto_auth::issue(
        "user".into(),
        vec![
            scope("/api/password", "POST"),
            scope("/api/browse", "GET"),
            scope("/api/apps", "GET"),
        ],
        &offered_before,
    )
    .unwrap();
    f.host.aliases.lock().unwrap()["root"]["api_tokens"] = json!([token]);
    let bearer = format!("Bearer {secret}");
    let headers = [("Authorization", bearer.as_str())];
    let response = console_request(&f, "GET", "/api/browse?path=C%3A%5C", &headers, None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let password = json!({"newUsername": "user", "newPassword": "taken-over", "confirmNewPassword": "taken-over"});
    let response = console_request(&f, "POST", "/api/password", &headers, Some(password)).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        f.host
            .credentials
            .read()
            .unwrap()
            .as_ref()
            .unwrap()
            .verifies("user", "password1")
    );
    let response = console_request(&f, "GET", "/api/apps", &headers, None).await;
    assert_eq!(response.status(), StatusCode::OK);
}
