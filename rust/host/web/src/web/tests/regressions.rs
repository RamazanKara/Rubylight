//! Regression tests for WEB-01..WEB-04, WEB-10, WEB-12, and WEB-13.

use super::*;
use crate::test_support::value;
use butterpollo_stream::state::test_support::Fixture;
use std::time::{Duration, Instant};

async fn log_in(f: &Fixture) -> Value {
    let response = console_request(
        f,
        "POST",
        "/api/auth/login",
        &[],
        Some(json!({"username": "user", "password": "password1"})),
    )
    .await;
    value(response, StatusCode::OK).await
}

#[tokio::test]
async fn concurrent_tab_refresh_uses_grace_window_until_revoked() {
    let f = Fixture::new();
    signed_up(&f);
    let initial = log_in(&f).await;
    let old_refresh = initial["refresh_token"].as_str().unwrap().to_owned();
    let old_csrf = initial["csrf_token"].as_str().unwrap().to_owned();
    let old_refresh_cookie = format!("__Host-apollo_refresh={old_refresh}");

    // Tab 1 rotates the refresh token.
    let first = value(
        console_request(
            &f,
            "POST",
            "/api/auth/refresh",
            &[("Cookie", &old_refresh_cookie), ("X-CSRF-Token", &old_csrf)],
            None,
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let rotated_access = first["access_token"].as_str().unwrap().to_owned();
    let rotated_refresh = first["refresh_token"].as_str().unwrap().to_owned();

    // Another unrelated session save must NOT wipe live rotated entries inside the grace window.
    let _other = log_in(&f).await;

    // Tab 2 arrives late with the pre-rotation refresh cookie + old CSRF token -> gets rotated session.
    let second = value(
        console_request(
            &f,
            "POST",
            "/api/auth/refresh",
            &[("Cookie", &old_refresh_cookie), ("X-CSRF-Token", &old_csrf)],
            None,
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        second["access_token"].as_str(),
        Some(rotated_access.as_str())
    );
    assert_eq!(
        second["refresh_token"].as_str(),
        Some(rotated_refresh.as_str())
    );

    // Tab 3 hits /api/auth/status with only the stale refresh cookie -> also succeeds via grace window.
    let status = value(
        console_request(
            &f,
            "GET",
            "/api/auth/status",
            &[("Cookie", &old_refresh_cookie)],
            None,
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(status["authenticated"], true);

    // Password change revokes sign-ins and clears the rotation grace cache immediately.
    f.host.revoke_sign_ins().unwrap();
    f.host.web_sessions.lock().unwrap().clear();
    let revoked_auth = format!("Refresh {old_refresh}");
    let revoked = console_request(
        &f,
        "POST",
        "/api/auth/refresh",
        &[("Authorization", &revoked_auth)],
        None,
    )
    .await;
    assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn csrf_token_refreshes_lapsed_access_cookie_from_refresh_cookie() {
    let f = Fixture::new();
    signed_up(&f);
    let session = log_in(&f).await;
    let access_token = session["access_token"].as_str().unwrap().to_owned();
    let refresh_token = session["refresh_token"].as_str().unwrap().to_owned();

    // Expire the access session while keeping the refresh window alive.
    {
        let mut sessions = f.host.web_sessions.lock().unwrap();
        for entry in sessions.values_mut() {
            entry.expires = Instant::now() - Duration::from_secs(1);
        }
    }
    let cookies =
        format!("__Host-apollo_session={access_token}; __Host-apollo_refresh={refresh_token}");
    let response =
        console_request(&f, "GET", "/api/csrf-token", &[("Cookie", &cookies)], None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let set_cookies: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok().map(str::to_owned))
        .collect();
    assert!(
        set_cookies
            .iter()
            .any(|c| c.starts_with("__Host-apollo_session=")),
        "expected refreshed session cookie, got {set_cookies:?}"
    );
    assert!(
        !set_cookies
            .iter()
            .any(|c| c.starts_with("__Host-apollo_anon_csrf=")),
        "must not issue anonymous CSRF when refresh cookie is valid"
    );
    let body = value(response, StatusCode::OK).await;
    let csrf = body["csrf_token"].as_str().unwrap();
    assert!(!csrf.is_empty());
    // Returned CSRF matches the live refreshed session in memory.
    assert!(
        f.host
            .web_sessions
            .lock()
            .unwrap()
            .values()
            .any(|s| s.csrf == csrf && s.expires > Instant::now())
    );
}

#[tokio::test]
async fn logout_requires_post_method() {
    let f = Fixture::new();
    signed_up(&f);
    let session = log_in(&f).await;
    let cookie = format!(
        "__Host-apollo_session={}",
        session["access_token"].as_str().unwrap()
    );
    let csrf = session["csrf_token"].as_str().unwrap().to_owned();

    let get_res =
        console_request(&f, "GET", "/api/auth/logout", &[("Cookie", &cookie)], None).await;
    assert_eq!(get_res.status(), StatusCode::METHOD_NOT_ALLOWED);
    // Session is still valid after rejected GET logout.
    let check = console_request(&f, "GET", "/api/apps", &[("Cookie", &cookie)], None).await;
    assert_eq!(check.status(), StatusCode::OK);

    let post_res = console_request(
        &f,
        "POST",
        "/api/auth/logout",
        &[("Cookie", &cookie), ("X-CSRF-Token", &csrf)],
        None,
    )
    .await;
    assert_eq!(post_res.status(), StatusCode::OK);
    let after = console_request(&f, "GET", "/api/apps", &[("Cookie", &cookie)], None).await;
    assert_eq!(after.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn credentials_reload_is_throttled_but_login_reloads_updated_credentials_immediately() {
    let f = Fixture::new();
    signed_up(&f);
    // First check runs; immediate second check is throttled.
    assert!(f.host.credentials_reload_due());
    assert!(!f.host.credentials_reload_due());

    // Simulate `--creds` changing the sign-in on disk while inside the throttle window.
    // Use a distinct username length so `file_stamp` (`(modified, len)`) changes even within one tick.
    let updated = Credentials::new("updated-user".into(), "new-password-2").unwrap();
    let doc = serde_json::to_value(&updated).unwrap();
    butterpollo_session::state::write_json(&f.host.credentials_path, &doc).unwrap();

    let response = console_request(
        &f,
        "POST",
        "/api/auth/login",
        &[],
        Some(json!({"username": "updated-user", "password": "new-password-2"})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn clients_and_apps_reject_invalid_config_overrides() {
    let f = Fixture::new();
    signed_up(&f);
    let client = f.client(u32::MAX);
    let session = log_in(&f).await;
    let cookie = format!(
        "__Host-apollo_session={}",
        session["access_token"].as_str().unwrap()
    );
    let csrf = session["csrf_token"].as_str().unwrap().to_owned();
    let headers = [("Cookie", cookie.as_str()), ("X-CSRF-Token", csrf.as_str())];

    for bad in [
        json!({"fec_percentage": 15.5}),
        json!({"dd_manual_resolution": "1920*1080"}),
        json!({"dd_manual_refresh_rate": 59.9401}),
        json!({"dd_manual_rotation": 45}),
    ] {
        let client_res = console_request(
            &f,
            "POST",
            "/api/clients/update",
            &headers,
            Some(json!({"uuid": client.uuid, "config_overrides": bad})),
        )
        .await;
        assert_eq!(client_res.status(), StatusCode::BAD_REQUEST, "client {bad}");

        let app_res = console_request(
            &f,
            "POST",
            "/api/apps",
            &headers,
            Some(json!({"name": "Game", "cmd": "game.exe", "config-overrides": bad})),
        )
        .await;
        assert_eq!(app_res.status(), StatusCode::BAD_REQUEST, "app {bad}");
    }
}

#[tokio::test]
async fn backup_inspect_streams_to_disk_and_cleans_up_temp_files() {
    let f = Fixture::new();
    signed_up(&f);
    std::fs::write(
        f.host.directory.join("sunshine.conf"),
        "sunshine_name = Den\n",
    )
    .unwrap();
    let archive = butterpollo_stream::settings_backup::export(&f.host.directory, false).unwrap();
    let session = log_in(&f).await;
    let cookie = format!(
        "__Host-apollo_session={}",
        session["access_token"].as_str().unwrap()
    );
    let csrf = session["csrf_token"].as_str().unwrap().to_owned();

    let send_raw = |bytes: Vec<u8>| {
        let router = router(f.host.clone()).layer(axum::Extension(f.connection(true)));
        let cookie = cookie.clone();
        let csrf = csrf.clone();
        async move {
            let req = axum::http::Request::builder()
                .method("POST")
                .uri("/api/backup/inspect")
                .header("Cookie", cookie)
                .header("X-CSRF-Token", csrf)
                .body(axum::body::Body::from(bytes))
                .unwrap();
            tower::ServiceExt::oneshot(router, req).await.unwrap()
        }
    };

    // Valid archive returns 200 + token and leaves no `.inspect-*.tmp` behind.
    let ok = value(send_raw(archive).await, StatusCode::OK).await;
    assert!(ok["token"].as_str().is_some_and(|t| t.len() == 32));

    // Invalid archive and empty body return 400 and leave no `.inspect-*.tmp` behind.
    assert_eq!(
        send_raw(b"not a zip".to_vec()).await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(send_raw(Vec::new()).await.status(), StatusCode::BAD_REQUEST);

    let backups_dir = f
        .host
        .directory
        .join(butterpollo_session::settings_backup::BACKUPS);
    let temps: Vec<_> = std::fs::read_dir(&backups_dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".inspect-"))
        .collect();
    assert!(temps.is_empty(), "leftover temp files: {temps:?}");
}

#[tokio::test]
async fn pin_endpoint_and_fallback_devices_ignore_expired_pairing_requests() {
    let f = Fixture::new();
    signed_up(&f);
    let session = log_in(&f).await;
    let cookie = format!(
        "__Host-apollo_session={}",
        session["access_token"].as_str().unwrap()
    );
    let csrf = session["csrf_token"].as_str().unwrap().to_owned();
    let headers = [("Cookie", cookie.as_str()), ("X-CSRF-Token", csrf.as_str())];

    let (tx_old, _rx_old) = tokio::sync::oneshot::channel();
    let (tx_new, rx_new) = tokio::sync::oneshot::channel();
    {
        let mut pins = f.host.pins.lock().unwrap();
        pins.insert(
            "expired-id".into(),
            butterpollo_stream::state::PendingPin {
                name: "Stale TV".into(),
                certificate: String::new(),
                address: "127.0.0.1".parse().unwrap(),
                created: Instant::now() - Duration::from_secs(305),
                sender: tx_old,
            },
        );
        pins.insert(
            "fresh-id".into(),
            butterpollo_stream::state::PendingPin {
                name: "Fresh TV".into(),
                certificate: String::new(),
                address: "127.0.0.1".parse().unwrap(),
                created: Instant::now(),
                sender: tx_new,
            },
        );
    }

    // Fallback console /devices omits the expired PIN and shows only the fresh one.
    let page = console_request(&f, "GET", "/devices", &[("Cookie", cookie.as_str())], None).await;
    let html = String::from_utf8(
        axum::body::to_bytes(page.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(!html.contains("Stale TV"), "expired PIN leaked to /devices");
    assert!(html.contains("Fresh TV"), "fresh PIN missing from /devices");

    // Submitting PIN without uniqueid succeeds for the lone fresh request despite stale entry.
    let res = console_request(
        &f,
        "POST",
        "/api/pin",
        &headers,
        Some(json!({"pin": "1234"})),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let (pin, name, _, _) = rx_new.await.unwrap();
    assert_eq!(pin, "1234");
    assert_eq!(name, "Fresh TV");
    assert!(!f.host.pins.lock().unwrap().contains_key("expired-id"));
}
