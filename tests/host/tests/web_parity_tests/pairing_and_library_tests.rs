//! Console API pairing, network history, OTP, log tail, library, and session history parity tests.
use super::*;

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
                address: "192.0.2.7".parse().unwrap(),
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
    assert_eq!(pending["requests"][0]["address"], "192.0.2.7");
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
        let pins = f.host.pins.lock().unwrap();
        assert_eq!(pins.len(), 2);
        assert!(pins.contains_key("first") && pins.contains_key("second"));
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
        ("1234".into(), "Living room".into(), None, None)
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
    let saved = butterpollo_session::state::PairedState::load(&f.host.paired_path).unwrap();
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
    assert!(f.host.otp.lock().unwrap().typed.is_none());
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
        let otp = otp.typed.as_ref().unwrap();
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
    assert_eq!(butterpollo_stream::maintenance::log_path(&f.host), path);
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
async fn library_restore_takes_chosen_or_all_removed_games_and_refuses_a_bad_list() {
    let f = Fixture::new();
    butterpollo_stream::steam::update_library(&f.host, |_, document| {
        document["launcher-dismissed"] = json!(["steam:1", "epic:2", "gog:3"]);
        true
    })
    .unwrap();
    let too_many: Vec<String> = (0..10_001).map(|i| format!("steam:{i}")).collect();
    for body in [
        Value::Null,
        json!({}),
        json!({"games": "steam:1"}),
        json!({"games": [1]}),
        json!({"games": []}),
        json!({"all": "yes"}),
        json!({"games": too_many}),
    ] {
        value(
            request(&f, "POST", "/api/library/restore", body.clone()).await,
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    let dismissed = || f.host.app_document.read().unwrap()["launcher-dismissed"].clone();
    assert_eq!(dismissed(), json!(["steam:1", "epic:2", "gog:3"]));
    let one = value(
        request(
            &f,
            "POST",
            "/api/library/restore",
            json!({"games": ["epic:2"]}),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(one["restored"], 1);
    assert_eq!(dismissed(), json!(["steam:1", "gog:3"]));
    let all = value(
        request(&f, "POST", "/api/library/restore", json!({"all": true})).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(all["restored"], 2);
    assert_eq!(dismissed(), json!([]));
}

#[tokio::test]
async fn library_discovery_and_import_require_authentication_and_validate_identities() {
    let f = Fixture::new();
    let credentials = parity_credentials();
    f.host.save_credentials(credentials).unwrap();
    *f.host.credentials.write().unwrap() = Some(credentials.clone());
    for (method, path) in [
        ("GET", "/api/library/discovery"),
        ("POST", "/api/library/import"),
        ("POST", "/api/library/restore"),
        ("POST", "/api/apps/delete"),
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
        butterpollo_config::config::Config::parse("library_epic_enabled = false").unwrap();
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

/// Nightjar issue 7: the Library's Steam switch, drawn from the launcher scan, showed on again
/// after a reload although Steam library (`steam_enabled`) was off. Both now read one setting.
#[tokio::test]
async fn the_library_steam_switch_and_the_steam_library_setting_agree() {
    /// What the Library shows (Steam's state in the scan, and whether it lists Steam games)
    /// and what All settings shows (`steam_enabled`), each read as the console reads it.
    async fn shown(f: &Fixture) -> (Value, bool, Value) {
        let discovery = value(
            request(f, "GET", "/api/library/discovery", Value::Null).await,
            StatusCode::OK,
        )
        .await;
        let state = discovery["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|source| source["source"] == "steam")
            .map(|source| source["state"].clone())
            .unwrap();
        let games = discovery["games"].as_array().unwrap();
        let steam_games = games.iter().any(|game| game["source"] == "steam");
        let config = value(
            request(f, "GET", "/api/config", Value::Null).await,
            StatusCode::OK,
        )
        .await;
        (state, steam_games, config["steam_enabled"].clone())
    }
    let f = Fixture::new();
    let (state, _, _) = shown(&f).await;
    assert_ne!(state, "disabled", "on by default");
    // The Library's switch saves the setting as All settings does, and a reload reads it back.
    for on in [false, true] {
        value(
            request(&f, "PATCH", "/api/config", json!({"steam_enabled": on})).await,
            StatusCode::OK,
        )
        .await;
        let (state, steam_games, setting) = shown(&f).await;
        assert_eq!(setting, json!(on));
        assert_eq!(state == "disabled", !on, "{on}: {state}");
        if !on {
            assert!(!steam_games, "an unread launcher lists no games");
        }
    }
}

#[tokio::test]
async fn the_session_history_route_lists_ended_streams_newest_first_and_requires_signing_in() {
    use butterpollo_session::session_history::{
        MAX_AGE_MS, MAX_BYTES, MAX_ENTRIES, REASON_ENDED, REASON_FAILED,
    };
    let f = Fixture::new();
    let empty = value(
        request(&f, "GET", "/api/sessions/history", Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(empty["sessions"], json!([]));
    assert_eq!(
        empty["limit"].as_u64(),
        Some(MAX_ENTRIES as u64),
        "the page names the count cap"
    );
    assert_eq!(
        empty["days"].as_u64(),
        Some(MAX_AGE_MS / 86_400_000),
        "and the age cap"
    );
    assert_eq!(
        empty["bytes"].as_u64(),
        Some(MAX_BYTES as u64),
        "and the byte cap that can trim below the count"
    );
    let client = f.client(0x071f1f00);
    let first = f.session(client.clone(), Role::Stream);
    f.host.record_session(&first, REASON_ENDED);
    let second = f.session(client, Role::Stream);
    second.fail("the encoder failed");
    f.host.record_session(&second, REASON_FAILED);
    let listed = value(
        request(&f, "GET", "/api/sessions/history", Value::Null).await,
        StatusCode::OK,
    )
    .await;
    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0]["id"], json!(second.launch.id));
    assert_eq!(sessions[0]["end_reason"], json!("failed"));
    assert_eq!(sessions[0]["device_name"], json!("Parity device"));
    assert_eq!(sessions[1]["id"], json!(first.launch.id));
    assert_eq!(sessions[1]["end_reason"], json!("ended"));
    // Like every other read, the history needs the console's sign-in or a token.
    let anonymous = router(f.host.clone())
        .layer(Extension(f.connection(true)))
        .oneshot(
            Request::builder()
                .uri("/api/sessions/history")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
}
