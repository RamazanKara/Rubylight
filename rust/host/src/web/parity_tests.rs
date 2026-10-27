use super::*;
use crate::state::{PendingPin, test_support::Fixture};
use axum::body::{Body, to_bytes};
use butterpollo_core::{session::Role, state::Credentials};
use tower::ServiceExt;

pub(super) async fn request(f: &Fixture, method: &str, path: &str, data: Value) -> Response {
    request_with(f, method, path, data, &[]).await
}
/// [`request`] with extra headers.
pub(super) async fn request_with(
    f: &Fixture,
    method: &str,
    path: &str,
    data: Value,
    headers: &[(&str, &str)],
) -> Response {
    // On disk too: the host takes up the sign-in the file holds.
    let credentials = Credentials::new("parity".into(), "parity-test").unwrap();
    f.host.save_credentials(&credentials).unwrap();
    *f.host.credentials.write().unwrap() = Some(credentials);
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, "Basic cGFyaXR5OnBhcml0eS10ZXN0")
        .header(header::CONTENT_TYPE, "application/json");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    router(f.host.clone())
        .layer(Extension(f.connection(true)))
        .oneshot(
            builder
                .body(Body::from(serde_json::to_vec(&data).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
}
pub(super) async fn value(response: Response, status: StatusCode) -> Value {
    assert_eq!(response.status(), status);
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn all_offered_token_permissions_can_be_created_persisted_used_and_revoked() {
    let f = Fixture::new();
    let offered = value(
        request(&f, "GET", "/api/token/routes", Value::Null).await,
        StatusCode::OK,
    )
    .await;
    let scopes = offered["routes"].clone();
    assert!(scopes.as_array().unwrap().len() > 64);
    let mut canonical: Vec<auth::Scope> = serde_json::from_value(scopes.clone()).unwrap();
    for scope in &mut canonical {
        scope.methods.sort();
    }
    let expected = serde_json::to_value(&canonical).unwrap();
    let issued = value(
        request(&f, "POST", "/api/token", json!({"scopes":scopes})).await,
        StatusCode::OK,
    )
    .await;
    let secret = issued["token"].as_str().unwrap();
    let saved = std::fs::read_to_string(&f.host.aliases_path).unwrap();
    assert!(!saved.contains(secret));
    let document: Value = serde_json::from_str(&saved).unwrap();
    let tokens = auth::read(&document).unwrap();
    assert_eq!(serde_json::to_value(&tokens[0].scopes).unwrap(), expected);
    // Exercise the saved representation as a newly loaded host would.
    *f.host.aliases.lock().unwrap() = document;
    let listed = value(
        request(&f, "GET", "/api/tokens", Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(listed["tokens"][0]["scopes"], expected);
    assert!(!listed.to_string().contains(secret));
    let hash = listed["tokens"][0]["hash"].as_str().unwrap();
    for scope in &tokens[0].scopes {
        let path = scope
            .path
            .replace("[^/]+", "example")
            .replace("[0-9]+", "123");
        for method in &scope.methods {
            assert!(auth::permits(&tokens, secret, "parity", &path, method));
        }
    }
    // An all-permissions token still cannot mint/list tokens or broaden a
    // listed route's methods. Only the administrator can manage API tokens.
    for (method, path, status) in [
        ("GET", "/api/apps", StatusCode::OK),
        ("GET", "/api/tokens", StatusCode::UNAUTHORIZED),
        ("POST", "/api/token", StatusCode::UNAUTHORIZED),
        ("PATCH", "/api/apps", StatusCode::UNAUTHORIZED),
    ] {
        let response = router(f.host.clone())
            .layer(Extension(f.connection(true)))
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::AUTHORIZATION, format!("Bearer {secret}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status, "{method} {path}");
    }
    value(
        request(&f, "DELETE", &format!("/api/token/{hash}"), Value::Null).await,
        StatusCode::OK,
    )
    .await;
    let revoked = router(f.host.clone())
        .layer(Extension(f.connection(true)))
        .oneshot(
            Request::builder()
                .uri("/api/apps")
                .header(header::AUTHORIZATION, format!("Bearer {secret}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
    let saved = state::load_json(&f.host.aliases_path, Value::Null).unwrap();
    assert!(auth::read(&saved).unwrap().is_empty());
}

#[tokio::test]
async fn both_metadata_routes_require_authentication_and_preserve_gpu_inventory() {
    let f = Fixture::new();
    let expected = json!({
        "status": true,
        "gpus": [{"name": "AMD Radeon RX 7600M XT", "vendor": 4098,
            "dedicated_memory": 8589934592u64, "luid": [2, 0], "pnp_id": null}],
        "gpu_error": null,
        "capture_status": {"displays": [], "error": null}
    });
    for path in ["/api/metadata", "/api/meta"] {
        // Populate the cache without opening GPU devices or probing encoders.
        *f.host.metadata.lock().unwrap() = Some((Instant::now(), expected.clone()));
        let actual = value(request(&f, "GET", path, Value::Null).await, StatusCode::OK).await;
        assert_eq!(actual, expected);
        let anonymous = router(f.host.clone())
            .layer(Extension(f.connection(true)))
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let denied = value(anonymous, StatusCode::UNAUTHORIZED).await;
        assert!(denied.get("gpus").is_none());
    }
}

#[tokio::test]
async fn device_updates_persist_permissions_overrides_and_revoke_only_that_devices_sessions() {
    let f = Fixture::new();
    let client = f.client(0x071f1f00);
    let mine = f.session(client.clone(), Role::Stream);
    let mut other = client.clone();
    other.uuid = "other".into();
    let theirs = f.session(other, Role::InputOnly);
    let body = json!({"uuid":client.uuid,"name":"Renamed","cert":"cannot replace the certificate","perm":u32::MAX,
        "config_overrides":{"max_bitrate":20000},"display_mode":"2560x1600x120","custom":"preserved"});
    let updated = value(
        request(&f, "POST", "/api/clients/update", body).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(updated["disconnected"], false);
    assert!(!mine.stopping());
    let listed = value(
        request(&f, "GET", "/api/clients/list", json!({})).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(listed["clients"], listed["named_certs"]);
    let device = &listed["clients"][0];
    assert_eq!(device["perm"], 0x071f1f00);
    assert_eq!(device["name"], "Renamed");
    assert_eq!(device["connected"], true);
    assert!(device.get("cert").is_none());
    assert_eq!(device["config_overrides"]["max_bitrate"], 20000);
    for body in [
        json!({"uuid":client.uuid,"perm":1<<25}),
        json!({"uuid":client.uuid,"enabled":false,"display_mode":null}),
    ] {
        let updated = value(
            request(&f, "POST", "/api/clients/update", body).await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(updated["disconnected"], true);
        assert!(mine.stopping());
        assert!(!theirs.stopping());
    }
    let saved = butterpollo_core::state::PairedState::load(&f.host.paired_path).unwrap();
    assert!(!saved.clients[0].enabled);
    assert_eq!(saved.clients[0].perm, 1 << 25);
    assert_eq!(saved.clients[0].cert, client.cert);
    assert_eq!(saved.clients[0].extra["custom"], "preserved");
    assert!(!saved.clients[0].extra.contains_key("display_mode"));
    assert_eq!(
        saved.clients[0].extra["config_overrides"]["max_bitrate"],
        20000
    );
}

#[tokio::test]
async fn host_control_is_granted_only_as_a_boolean_and_revoking_keeps_the_stream() {
    let f = Fixture::new();
    // A full permission value must not switch it on.
    let client = f.client(u32::MAX);
    let mine = f.session(client.clone(), Role::Stream);
    assert!(!f.host.paired.read().unwrap().clients[0].allows_host_control());
    for bad in [
        json!("yes"),
        json!("true"),
        json!(1),
        json!([true]),
        json!({}),
    ] {
        value(
            request(
                &f,
                "POST",
                "/api/clients/update",
                json!({"uuid":client.uuid,"name":"Renamed","host_control":bad}),
            )
            .await,
            StatusCode::BAD_REQUEST,
        )
        .await;
        let paired = f.host.paired.read().unwrap();
        assert!(!paired.clients[0].allows_host_control(), "{bad}");
        assert_eq!(paired.clients[0].name, client.name, "nothing else changed");
    }
    let granted = value(
        request(
            &f,
            "POST",
            "/api/clients/update",
            json!({"uuid":client.uuid,"host_control":true}),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(granted["disconnected"], false);
    assert!(f.host.paired.read().unwrap().clients[0].allows_host_control());
    let saved = butterpollo_core::state::PairedState::load(&f.host.paired_path).unwrap();
    assert!(saved.clients[0].allows_host_control());
    let listed = value(
        request(&f, "GET", "/api/clients/list", json!({})).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(listed["clients"][0]["host_control"], true);
    // Revoking, with false or null, takes effect now and keeps the session.
    for revoke in [json!(false), json!(null)] {
        let granted = value(
            request(
                &f,
                "POST",
                "/api/clients/update",
                json!({"uuid":client.uuid,"host_control":true}),
            )
            .await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(granted["disconnected"], false);
        let revoked = value(
            request(
                &f,
                "POST",
                "/api/clients/update",
                json!({"uuid":client.uuid,"host_control":revoke}),
            )
            .await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(revoked["disconnected"], false);
        assert!(!f.host.paired.read().unwrap().clients[0].allows_host_control());
        assert!(!mine.stopping());
    }
}

#[tokio::test]
async fn sleep_is_granted_only_as_a_boolean() {
    let f = Fixture::new();
    let client = f.client(u32::MAX);
    assert!(!f.host.paired.read().unwrap().clients[0].allows_sleep());
    for bad in [json!("true"), json!(1), json!([true]), json!({})] {
        value(
            request(
                &f,
                "POST",
                "/api/clients/update",
                json!({"uuid":client.uuid,"name":"Renamed","allow_sleep":bad}),
            )
            .await,
            StatusCode::BAD_REQUEST,
        )
        .await;
        let paired = f.host.paired.read().unwrap();
        assert!(!paired.clients[0].allows_sleep(), "{bad}");
        assert!(!paired.clients[0].extra.contains_key("allow_sleep"));
        assert_eq!(paired.clients[0].name, client.name, "nothing else changed");
    }
    let update = |allow: Value| {
        request(
            &f,
            "POST",
            "/api/clients/update",
            json!({"uuid":client.uuid,"allow_sleep":allow}),
        )
    };
    value(update(json!(true)).await, StatusCode::OK).await;
    let saved = butterpollo_core::state::PairedState::load(&f.host.paired_path).unwrap();
    assert!(saved.clients[0].allows_sleep());
    let listed = value(
        request(&f, "GET", "/api/clients/list", json!({})).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(listed["clients"][0]["allow_sleep"], true);
    value(update(json!(null)).await, StatusCode::OK).await;
    assert!(!f.host.paired.read().unwrap().clients[0].allows_sleep());
    let saved = butterpollo_core::state::PairedState::load(&f.host.paired_path).unwrap();
    assert!(!saved.clients[0].extra.contains_key("allow_sleep"));
}

#[tokio::test]
async fn usb_and_folder_grants_are_saved_only_in_their_shape() {
    let f = Fixture::new();
    let client = f.client(u32::MAX);
    let update = |change: Value| {
        let mut body = json!({"uuid":client.uuid,"name":"Renamed"});
        body.as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        request(&f, "POST", "/api/clients/update", body)
    };
    for bad in [
        json!({"allow_usb": "true"}),
        json!({"allow_usb": 1}),
        json!({"shared_folders": ["games"]}),
        json!({"shared_folders": "games"}),
        json!({"shared_folders": {"games": "admin"}}),
        json!({"shared_folders": {"games": true}}),
        json!({"shared_folders": {"": "read"}}),
        json!({"shared_folders": {"../games": "read"}}),
    ] {
        value(update(bad.clone()).await, StatusCode::BAD_REQUEST).await;
        let paired = f.host.paired.read().unwrap();
        let device = &paired.clients[0];
        assert!(!device.allows_usb() && !device.has_folders(), "{bad}");
        assert!(!device.extra.contains_key("allow_usb"), "{bad}");
        assert!(!device.extra.contains_key("shared_folders"), "{bad}");
        assert_eq!(device.name, client.name, "nothing else changed");
    }
    value(
        update(json!({"allow_usb": true, "shared_folders": {"games": "read", "saves-2": "write"}}))
            .await,
        StatusCode::OK,
    )
    .await;
    let saved = butterpollo_core::state::PairedState::load(&f.host.paired_path).unwrap();
    let device = &saved.clients[0];
    assert!(device.allows_usb() && device.has_folders());
    use butterpollo_core::native_api::folders::FolderAccess;
    assert_eq!(
        (
            device.folder_access("games"),
            device.folder_access("saves-2")
        ),
        (Some(FolderAccess::Read), Some(FolderAccess::Write))
    );
    value(
        update(json!({"allow_usb": null, "shared_folders": null})).await,
        StatusCode::OK,
    )
    .await;
    let saved = butterpollo_core::state::PairedState::load(&f.host.paired_path).unwrap();
    assert!(!saved.clients[0].extra.contains_key("allow_usb"));
    assert!(!saved.clients[0].extra.contains_key("shared_folders"));
}

#[tokio::test]
async fn host_control_cannot_be_granted_without_the_update_route() {
    let f = Fixture::new();
    let client = f.client(u32::MAX);
    let issued = value(
        request(
            &f,
            "POST",
            "/api/token",
            json!({"scopes":[{"path":"/api/apps","methods":["GET"]}]}),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let secret = issued["token"].as_str().unwrap();
    let body = json!({"uuid":client.uuid,"host_control":true}).to_string();
    for authorization in [Some(format!("Bearer {secret}")), None] {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/api/clients/update")
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(authorization) = authorization {
            builder = builder.header(header::AUTHORIZATION, authorization);
        }
        let response = router(f.host.clone())
            .layer(Extension(f.connection(true)))
            .oneshot(builder.body(Body::from(body.clone())).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(!f.host.paired.read().unwrap().clients[0].allows_host_control());
    }
    let saved = butterpollo_core::state::PairedState::load(&f.host.paired_path).unwrap();
    assert!(!saved.clients[0].extra.contains_key("host_control"));
}

#[tokio::test]
async fn pending_pairing_requests_expire_and_pin_submission_selects_one_device() {
    let f = Fixture::new();
    let mut receivers = Vec::new();
    for (id, age) in [("first", 0), ("second", 0), ("expired", 301)] {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        receivers.push(receiver);
        f.host.pins.lock().unwrap().insert(
            id.into(),
            PendingPin {
                name: id.into(),
                certificate: "secret".into(),
                created: Instant::now() - Duration::from_secs(age),
                sender,
            },
        );
    }
    let pending = value(
        request(&f, "GET", "/api/clients/pending", json!({})).await,
        StatusCode::OK,
    )
    .await;
    let ids: Vec<_> = pending["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["uniqueid"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["first", "second"]);
    assert!(!pending.to_string().contains("secret"));
    for body in [
        json!({"pin":"1234"}),
        json!({"pin":"12x4","uniqueid":"first"}),
        json!({"pin":"123","uniqueid":"first"}),
    ] {
        value(
            request(&f, "POST", "/api/pin", body).await,
            StatusCode::BAD_REQUEST,
        )
        .await;
        assert_eq!(f.host.pins.lock().unwrap().len(), 3);
    }
    value(
        request(
            &f,
            "POST",
            "/api/pin",
            json!({"pin":"1234","uniqueid":"first","name":"Living room"}),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        receivers[0].try_recv().unwrap(),
        ("1234".into(), "Living room".into())
    );
    assert!(receivers[1].try_recv().is_err());
    assert!(!f.host.pins.lock().unwrap().contains_key("first"));
}

#[tokio::test]
async fn network_history_is_visible_and_only_authenticated_reset_clears_it() {
    let f = Fixture::new();
    let client = f.client(0x071f1f00);
    let session = f.session(client.clone(), Role::Stream);
    let guard = f.host.configure_keyframe_loss(&session, true);
    // Three lost frames, not just a rough start: only that is remembered.
    for frame in 1..=3 {
        session.keyframe_loss.lock().unwrap().unrecoverable(
            frame,
            frame,
            Duration::from_millis(16),
            Instant::now(),
        );
    }
    assert!(session.keyframe_loss_sustained_at().is_some());
    drop(guard);
    let listed = value(
        request(&f, "GET", "/api/clients/list", Value::Null).await,
        StatusCode::OK,
    )
    .await;
    let routes = &listed["clients"][0]["network_loss_routes"];
    assert_eq!(routes[0]["address"], "127.0.0.1");
    assert_eq!(routes[0]["sessions_until_probe"], 2);
    assert!(routes[0]["expires_in_seconds"].as_u64().unwrap() <= 86400);
    let rejected = router(f.host.clone())
        .layer(Extension(f.connection(true)))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/clients/reset-network-history")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"uuid":client.uuid}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
    assert!(!f.host.keyframe_loss_routes().is_empty());
    value(
        request(
            &f,
            "POST",
            "/api/clients/update",
            json!({"uuid":client.uuid,"network_loss_routes":[{"address":"forged"}]}),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert!(
        !f.host.paired.read().unwrap().clients[0]
            .extra
            .contains_key("network_loss_routes")
    );
    value(
        request(
            &f,
            "POST",
            "/api/clients/reset-network-history",
            json!({"uuid":client.uuid}),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert!(f.host.keyframe_loss_routes().is_empty());
    let saved = butterpollo_core::state::PairedState::load(&f.host.paired_path).unwrap();
    assert_eq!(saved.document["root"]["keyframe_loss_routes"], json!([]));
    assert!(!session.stopping());
}

#[tokio::test]
async fn otp_api_validates_passphrases_and_replaces_the_previous_pin() {
    let f = Fixture::new();
    value(
        request(&f, "POST", "/api/otp", json!({"passphrase":"abc"})).await,
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert!(f.host.otp.lock().unwrap().is_none());
    for phrase in ["abcd", "日本語文"] {
        let response = value(
            request(
                &f,
                "POST",
                "/api/otp",
                json!({"passphrase":phrase,"deviceName":"Phone"}),
            )
            .await,
            StatusCode::OK,
        )
        .await;
        let pin = response["otp"].as_str().unwrap();
        assert_eq!(pin.len(), 4);
        assert!(pin.bytes().all(|c| c.is_ascii_digit()));
        let otp = f.host.otp.lock().unwrap();
        let otp = otp.as_ref().unwrap();
        assert_eq!(otp.pin, pin);
        assert_eq!(otp.passphrase, phrase);
        assert_eq!(otp.device_name, "Phone");
    }
}

#[tokio::test]
async fn client_names_and_lossless_paths_reject_command_and_network_inputs() {
    let f = Fixture::new();
    let client = f.client(0x071f1f00);
    value(
        request(
            &f,
            "POST",
            "/api/clients/update",
            json!({"uuid":client.uuid,"name":"Phone\" & whoami"}),
        )
        .await,
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(f.host.paired.read().unwrap().clients[0].name, client.name);
    for path in [
        "%5C%5Chost%5Cshare",
        "%5C%5C%3F%5CUNC%5Chost%5Cshare",
        "relative",
    ] {
        value(
            request(
                &f,
                "GET",
                &format!("/api/lossless_scaling/status?path={path}"),
                json!({}),
            )
            .await,
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
}

#[tokio::test]
async fn log_tail_tracks_offsets_appends_rotation_and_a_configured_path() {
    let f = Fixture::new();
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("log_path".into(), "custom.log".into());
    let path = f.host.directory.join("custom.log");
    let missing = value(
        request(&f, "GET", "/api/logs/tail?offset=0", json!({})).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        missing,
        json!({"status":true,"offset":0,"size":0,"reset":false,"text":""})
    );
    let first = format!("{}\n", "a".repeat(699));
    let second = format!("{}\n", "b".repeat(699));
    std::fs::write(&path, format!("{first}{second}")).unwrap();
    assert_eq!(crate::maintenance::log_path(&f.host), path);
    let chunk = value(
        request(&f, "GET", "/api/logs/tail?offset=0&max=1024", json!({})).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(chunk["text"], first);
    assert_eq!(chunk["offset"], 700);
    assert_eq!(chunk["reset"], false);
    let chunk = value(
        request(&f, "GET", "/api/logs/tail?offset=700&max=1024", json!({})).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(chunk["text"], second);
    assert_eq!(chunk["offset"], 1400);
    std::fs::write(&path, format!("{first}{second}new 日本語\n")).unwrap();
    let chunk = value(
        request(&f, "GET", "/api/logs/tail?offset=1400", json!({})).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(chunk["text"], "new 日本語\n");
    std::fs::write(&path, "rotated\n").unwrap();
    let chunk = value(
        request(&f, "GET", "/api/logs/tail?offset=1400", json!({})).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(chunk["reset"], true);
    assert_eq!(chunk["offset"], 8);
    assert_eq!(chunk["text"], "rotated\n");
}

#[tokio::test]
async fn browse_and_app_icon_routes_return_the_local_library_assets() {
    let f = Fixture::new();
    let icons = f.host.directory.join("covers");
    std::fs::create_dir_all(&icons).unwrap();
    let png = b"\x89PNG\r\n\x1a\nfixture";
    let icon = icons.join("playnite_fixture.png");
    std::fs::write(&icon, png).unwrap();
    let app = json!({"name":"Playnite game","uuid":"icon-app","playnite-id":"fixture","playnite-icon-path":icon.to_string_lossy()});
    value(request(&f, "POST", "/api/apps", app).await, StatusCode::OK).await;
    let response = request(&f, "GET", "/api/apps/icon-app/icon", json!({})).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
        png
    );
    let query: String = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("path", &icon.to_string_lossy())
        .append_pair("type", "file")
        .finish();
    let listing = value(
        request(&f, "GET", &format!("/api/browse?{query}"), json!({})).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(listing["path"], icons.to_string_lossy().as_ref());
    assert!(
        listing["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "playnite_fixture.png")
    );
}

#[tokio::test]
async fn library_discovery_and_import_require_authentication_and_validate_identities() {
    let f = Fixture::new();
    f.host
        .save_credentials(&Credentials::new("parity".into(), "parity-test").unwrap())
        .unwrap();
    *f.host.credentials.write().unwrap() =
        Some(Credentials::new("parity".into(), "parity-test").unwrap());
    for (method, path) in [
        ("GET", "/api/library/discovery"),
        ("POST", "/api/library/import"),
    ] {
        let response = router(f.host.clone())
            .layer(Extension(f.connection(true)))
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    for body in [
        json!({"games": []}),
        json!({"games": [{"source": "unknown", "id": "1"}]}),
        json!({"games": [{"source": "epic", "cmd": "must not execute"}]}),
    ] {
        value(
            request(&f, "POST", "/api/library/import", body).await,
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    let discovery = value(
        request(&f, "GET", "/api/library/discovery", Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(discovery["sources"].as_array().unwrap().len(), 8);
    assert!(
        discovery["sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|source| source["source"] == "microsoft"),
        "the Microsoft Store and Xbox source is scanned"
    );
    let state = |key: &str| {
        discovery["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|source| source["source"] == key)
            .map(|source| source["state"].clone())
    };
    assert_eq!(
        state("local"),
        Some(json!("disabled")),
        "no game folders by default"
    );
    assert_ne!(state("epic"), Some(json!("disabled")));
    assert_ne!(state("gog"), Some(json!("disabled")));
    // Turned off in the settings: the next scan skips the launcher.
    *f.host.config.write().unwrap() =
        butterpollo_core::config::Config::parse("library_epic_enabled = false").unwrap();
    let discovery = value(
        request(&f, "GET", "/api/library/discovery", Value::Null).await,
        StatusCode::OK,
    )
    .await;
    let epic = discovery["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|source| source["source"] == "epic")
        .unwrap();
    assert_eq!(epic["state"], "disabled");
    assert!(
        discovery["games"]
            .as_array()
            .unwrap()
            .iter()
            .all(|game| game["source"] != "epic")
    );
    assert!(discovery["games"].is_array());
    for game in discovery["games"].as_array().unwrap() {
        assert!(game["source"].is_string() && game["id"].is_string());
        assert!(
            game.get("launch").is_none(),
            "launch actions are resolved only on the host"
        );
    }
}
