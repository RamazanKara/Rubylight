//! Native API authentication, 404 shape, capabilities, permissions, and library tests.
use super::*;

#[tokio::test]
async fn an_unpaired_request_gets_403_not_paired_on_every_endpoint() {
    let f = Fixture::new();
    let endpoints = [
        ("GET", "/capabilities"),
        ("GET", "/library"),
        ("GET", "/session"),
        ("POST", "/session/disconnect"),
        ("POST", "/session/end"),
        ("POST", "/session/continue-here"),
        ("POST", "/session/close-game"),
        ("POST", "/session/couch-menu"),
        ("GET", "/radeon"),
        ("POST", "/radeon/settings"),
        ("POST", "/radeon/restore"),
        ("GET", "/neural?app=game"),
        ("POST", "/neural/settings"),
        ("GET", "/wake"),
        ("GET", "/audio"),
        ("POST", "/audio/settings"),
        ("GET", "/coop"),
        ("POST", "/coop/join"),
        ("GET", "/power"),
        ("POST", "/power/restart"),
        ("POST", "/power/shutdown"),
        ("GET", "/update"),
        ("POST", "/update/install"),
        ("GET", "/folders"),
        ("GET", "/folders/games/entries?path="),
        ("GET", "/folders/games/content?path=a"),
        ("PUT", "/folders/games/content?path=a"),
    ];
    let certificate_less = Connection {
        certificate: None,
        ..f.connection(true)
    };
    let not_tls = f.connection(false);
    for (method, path) in endpoints {
        // The host's own certificate, but nobody is paired with it.
        let unknown = f.connection(true);
        api(&f, &unknown, method, path)
            .await
            .assert_error(403, "not_paired");
        api(&f, &certificate_less, method, path)
            .await
            .assert_error(403, "not_paired");
        api(&f, &not_tls, method, path)
            .await
            .assert_error(403, "not_paired");
        // Garbage in the body does not change the answer.
        send(
            &f,
            true,
            &unknown,
            method,
            &format!("{PREFIX}{path}"),
            Some("text/xml"),
            "{",
        )
        .await
        .assert_error(403, "not_paired");
        // Plain HTTP has no native API at all.
        assert_eq!(
            send(
                &f,
                false,
                &unknown,
                method,
                &format!("{PREFIX}{path}"),
                None,
                vec![]
            )
            .await
            .status,
            StatusCode::NOT_FOUND
        );
    }
    // A device turned off, or one without any permission, is not paired either.
    let client = f.client(ALL);
    f.host.paired.write().unwrap().clients[0].enabled = false;
    api(&f, &f.connection(true), "GET", "/session")
        .await
        .assert_error(403, "not_paired");
    f.host.paired.write().unwrap().clients[0].enabled = true;
    f.host.paired.write().unwrap().clients[0].perm = 0;
    api(&f, &f.connection(true), "GET", "/session")
        .await
        .assert_error(403, "not_paired");
    f.host.paired.write().unwrap().clients[0].perm = client.perm;
    assert_eq!(
        api(&f, &f.connection(true), "GET", "/session").await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn unknown_paths_and_methods_answer_404_not_found_in_the_error_shape() {
    let s = Setup::new();
    for (method, path) in [
        ("GET", "/nope"),
        ("GET", ""),
        ("GET", "/"),
        ("GET", "/session/nope"),
        ("GET", "/radeon/nope"),
        // Wrong methods.
        ("POST", "/radeon"),
        ("GET", "/radeon/settings"),
        ("GET", "/radeon/restore"),
        ("POST", "/capabilities"),
        ("POST", "/library"),
        ("DELETE", "/session"),
        ("GET", "/session/end"),
        ("GET", "/session/close-game"),
        ("GET", "/session/couch-menu"),
        ("POST", "/audio"),
        ("GET", "/audio/settings"),
        ("POST", "/usb/tunnel"),
        ("POST", "/folders"),
        ("PUT", "/folders/games/entries?path="),
        ("DELETE", "/folders/games/content?path=a"),
        ("GET", "/folders/games"),
        // Defined for 3.0, not yet served.
        ("PROPFIND", "/folders/games/dav/"),
        ("GET", "/folders/games/dav/"),
    ] {
        api(&s.f, &s.connection, method, path)
            .await
            .assert_error(404, "not_found");
    }
    // An unpaired caller learns nothing more from an unknown path.
    api(
        &s.f,
        &Connection {
            certificate: None,
            ..s.connection.clone()
        },
        "GET",
        "/radeon/nope",
    )
    .await
    .assert_error(404, "not_found");
}

#[tokio::test]
async fn capabilities_list_the_features_and_the_callers_permissions() {
    let s = Setup::with(LIST | VIEW_BIT);
    s.host()
        .config
        .write()
        .unwrap()
        .values
        .insert("sunshine_name".into(), "Couch PC".into());
    let reply = s.get("/capabilities").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.headers[header::CACHE_CONTROL], "no-store");
    let capabilities: Capabilities = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(capabilities.api_version, 1);
    assert_eq!(capabilities.host.name, "Couch PC");
    assert_eq!(capabilities.host.version, HOST_VERSION);
    assert_eq!(
        capabilities.host.unique_id,
        s.host().paired.read().unwrap().unique_id
    );
    // Radeon features appear when the host started its Radeon worker (see native_tests/radeon.rs).
    assert_eq!(
        capabilities.features,
        [
            "library",
            "session",
            "session.close_game",
            "session.launcher",
            "wake",
            "audio.night_mode",
            "audio.headphone_surround",
            "session.mic_only",
            "session.continue_here",
            "power.restart",
            "power.shutdown",
            "neural.frame_generation",
            "session.couch_menu",
            "pairing.approve",
            "session.guest-invites",
            "clipboard.image",
            "stream.dvr"
        ]
    );
    // Guests from other devices are listed only while `guest_join` is not off (the default).
    s.host()
        .config
        .write()
        .unwrap()
        .values
        .insert("guest_join".into(), "ask".into());
    let asking: Capabilities = serde_json::from_slice(&s.get("/capabilities").await.body).unwrap();
    let guests_at = asking.features.iter().position(|f| f == "session.guests");
    assert_eq!(
        guests_at,
        Some(asking.features.len() - 4),
        "{:?}",
        asking.features
    );
    s.host().config.write().unwrap().values.remove("guest_join");
    let permissions = capabilities.permissions;
    assert_eq!(
        (permissions.list, permissions.view, permissions.launch),
        (true, true, false)
    );
    assert!(!permissions.host_control);
    assert!(!permissions.coop_join);
    // While this host can run Nucleus Co-op sessions, `coop` follows the couch menu.
    s.host()
        .nucleus
        .use_pc(Arc::new(butterpollo_stream::nucleus::fake::Pc::installed()));
    let coop: Capabilities = serde_json::from_slice(&s.get("/capabilities").await.body).unwrap();
    assert_eq!(
        coop.features,
        [
            "library",
            "session",
            "session.close_game",
            "session.launcher",
            "wake",
            "audio.night_mode",
            "audio.headphone_surround",
            "session.mic_only",
            "session.continue_here",
            "power.restart",
            "power.shutdown",
            "neural.frame_generation",
            "session.couch_menu",
            "coop",
            "pairing.approve",
            "session.guest-invites",
            "clipboard.image",
            "stream.dvr"
        ]
    );
    s.host().nucleus.use_pc(Arc::new(
        butterpollo_stream::nucleus::fake::Pc::unavailable(),
    ));
    // The console grants Host control: the very next request shows it.
    s.host_control(true);
    let granted: Capabilities = serde_json::from_slice(&s.get("/capabilities").await.body).unwrap();
    assert!(granted.permissions.host_control);
    s.host_control(false);
    let revoked: Capabilities = serde_json::from_slice(&s.get("/capabilities").await.body).unwrap();
    assert!(!revoked.permissions.host_control);
}

#[tokio::test]
async fn usb_and_shared_folders_show_what_the_device_holds_and_list_no_feature() {
    let s = Setup::new();
    let capabilities = || async {
        serde_json::from_slice::<Capabilities>(&s.get("/capabilities").await.body).unwrap()
    };
    let none = capabilities().await;
    assert_eq!(
        (none.permissions.usb, none.permissions.shared_folders),
        (false, false)
    );
    {
        let mut paired = s.host().paired.write().unwrap();
        let extra = &mut paired.clients[0].extra;
        extra.insert("allow_usb".into(), true.into());
        extra.insert(
            "shared_folders".into(),
            serde_json::json!({"games": "read"}),
        );
    }
    let granted = capabilities().await;
    assert_eq!(
        (granted.permissions.usb, granted.permissions.shared_folders),
        (true, true)
    );
    // USB forwarding is off by default and the host shares no folder, so the features
    // stay what a 2.x client reads.
    assert_eq!(granted.features, none.features);
}

#[tokio::test]
async fn a_missing_permission_is_403_and_names_what_to_turn_on() {
    // Library needs List apps.
    let s = Setup::with(VIEW_BIT | LAUNCH);
    let reply = s.get("/library").await;
    reply.assert_error(403, "permission_required");
    assert!(
        reply.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("List applications")
    );
    // The session is readable by any paired device.
    assert_eq!(s.get("/session").await.status, StatusCode::OK);
    // End needs Launch.
    let s = Setup::with(LIST | VIEW_BIT);
    let reply = s.post("/session/end").await;
    reply.assert_error(403, "permission_required");
    assert!(
        reply.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Launch")
    );
    // Close game needs Launch before it looks at Host control.
    s.host_control(true);
    s.post("/session/close-game")
        .await
        .assert_error(403, "permission_required");
    // Disconnect needs View or Launch.
    let s = Setup::with(LIST);
    s.post("/session/disconnect")
        .await
        .assert_error(403, "permission_required");
    for perm in [VIEW_BIT, LAUNCH] {
        let s = Setup::with(perm);
        s.post("/session/disconnect")
            .await
            .assert_error(409, "no_session");
    }
}

#[tokio::test]
async fn library_lists_the_added_games_with_ids_that_survive_a_rename() {
    let s = Setup::new();
    let assets = s.host().assets.parent().unwrap().to_owned();
    std::fs::create_dir_all(&assets).unwrap();
    let cover = assets.join("dota.png");
    std::fs::write(&cover, b"\x89PNG\r\n\x1a\nfirst").unwrap();
    let apps = |title: &str| -> Vec<App> {
        [
            json!({"name": "Desktop", "uuid": "11111111-1111-4111-8111-111111111111"}),
            json!({"name": title, "uuid": "22222222-2222-4222-8222-222222222222",
                   "steam-id": "570", "image-path": cover.to_string_lossy()}),
            json!({"name": "Mirage", "uuid": "33333333-3333-4333-8333-333333333333",
                   "launcher-source": "ubisoft", "launcher-id": "13504"}),
            // The synthetic tile of the Remote session, not a game.
            json!({"name": "Remote Monitor", "uuid": "9a1c5a25-58fe-40e0-b9aa-7d3f00000005"}),
        ]
        .into_iter()
        .map(|app| serde_json::from_value(app).unwrap())
        .collect()
    };
    *s.host().apps.write().unwrap() = apps("Dota 2");
    let first = s.get("/library").await;
    assert_eq!(first.status, StatusCode::OK);
    assert_eq!(first.headers[header::CACHE_CONTROL], "no-store");
    let first: Library = serde_json::from_slice(&first.body).unwrap();
    let titles: Vec<_> = first.games.iter().map(|game| game.title.as_str()).collect();
    assert_eq!(titles, ["Desktop", "Dota 2", "Mirage"]);
    let dota = &first.games[1];
    assert_eq!(dota.install_id, "22222222-2222-4222-8222-222222222222");
    assert_eq!(dota.game_id.as_deref(), Some("steam:570"));
    assert_eq!(dota.store.as_ref().unwrap().source, "steam");
    assert_eq!(first.games[2].game_id.as_deref(), Some("ubisoft:13504"));
    assert_eq!(first.games[0].game_id, None);
    assert!(first.games.iter().all(|game| !game.running));
    let artwork = dota.artwork.cover.clone().unwrap();
    assert_eq!(
        artwork,
        format!("/appasset?appid={}&AssetType=2&AssetIdx=0", dota.app_id)
    );

    // The first request started the scan and did not wait for it. Once it is
    // in, a game of a store is installed or not, and a manual app is null.
    let scanned = loop {
        let library: Library = serde_json::from_slice(&s.get("/library").await.body).unwrap();
        if library.games[1].installed.is_some() {
            break library;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    assert_eq!(scanned.games[0].installed, None);
    assert_eq!(scanned.games[1].installed, Some(false));
    library::seed(s.host(), Scan::new(["steam:570".to_owned()]));
    let installed: Library = serde_json::from_slice(&s.get("/library").await.body).unwrap();
    assert_eq!(
        (installed.games[1].installed, installed.games[2].installed),
        (Some(true), Some(false))
    );
    // Installing changes the revision, which hashes the whole body.
    assert_ne!(installed.revision, first.revision);

    // A game that runs sets `running` and leaves the revision alone.
    let app = s.host().apps.read().unwrap()[1].clone();
    let mut running =
        butterpollo_stream::process::RunningApp::with_environment(&app, Default::default())
            .unwrap();
    running.owner = s.client.uuid.clone();
    *s.host().current_app.lock().unwrap() = Some(running);
    let playing: Library = serde_json::from_slice(&s.get("/library").await.body).unwrap();
    assert_eq!(playing.revision, installed.revision);
    assert_eq!(
        playing
            .games
            .iter()
            .map(|game| game.running)
            .collect::<Vec<_>>(),
        [false, true, false]
    );
    s.host().stop_app(butterpollo_stream::state::AppStop::Test);

    // A rename keeps the ids and changes the revision. The app id is the CRC
    // of the name, so it moves, and `install_id` and `game_id` are the keys.
    *s.host().apps.write().unwrap() = apps("Dota 2 (renamed)");
    let renamed: Library = serde_json::from_slice(&s.get("/library").await.body).unwrap();
    let after = &renamed.games[1];
    assert_eq!(after.title, "Dota 2 (renamed)");
    assert_eq!(after.install_id, dota.install_id);
    assert_eq!(after.game_id, dota.game_id);
    assert_ne!(after.app_id, dota.app_id);
    assert_ne!(renamed.revision, installed.revision);

    // The cover version follows the image file.
    let version = after.artwork.version.clone();
    assert_eq!(version.len(), 8);
    let again: Library = serde_json::from_slice(&s.get("/library").await.body).unwrap();
    assert_eq!(again.games[1].artwork.version, version);
    std::fs::write(&cover, b"\x89PNG\r\n\x1a\nsecond, longer").unwrap();
    let changed: Library = serde_json::from_slice(&s.get("/library").await.body).unwrap();
    assert_ne!(changed.games[1].artwork.version, version);
    assert_ne!(changed.revision, again.revision);
}
