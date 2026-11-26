//! NVHTTP background-stream, hardware metrics, network path, and serverinfo tests.
use super::*;

#[test]
fn background_game_can_only_resume_on_its_owner_in_the_same_mode() {
    let fixture = Fixture::new();
    let mut client = fixture.client(u32::MAX);
    client.extra.insert("background_stream".into(), true.into());
    let mut launch = fixture.launch(client, Role::Stream);
    assert!(validate_background_launch(&fixture.host, &launch).is_ok());
    let app = fixture.host.apps.read().unwrap()[0].clone();
    let mut running =
        butterpollo_stream::process::RunningApp::with_environment(&app, Default::default())
            .unwrap();
    running.owner = launch.client.uuid.clone();
    running.background = true;
    let generation = running.generation.clone();
    *fixture.host.current_app.lock().unwrap() = Some(running);
    assert!(validate_background_launch(&fixture.host, &launch).is_ok());
    // The device's second screen is the same owner.
    let device = launch.client.uuid.clone();
    launch.client.uuid = butterpollo_session::state::second_screen_uuid(&device);
    assert!(validate_background_launch(&fixture.host, &launch).is_ok());

    launch.client.uuid = "another-device".into();
    assert!(
        validate_background_launch(&fixture.host, &launch)
            .unwrap_err()
            .to_string()
            .contains("another device")
    );
    launch.client.uuid = butterpollo_session::state::second_screen_uuid("another-device");
    assert!(validate_background_launch(&fixture.host, &launch).is_err());
    launch.client.uuid = "parity-device".into();
    launch.client.extra.clear();
    assert!(
        validate_background_launch(&fixture.host, &launch)
            .unwrap_err()
            .to_string()
            .contains("changing background")
    );
    assert_eq!(
        fixture
            .host
            .current_app
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .generation,
        generation
    );

    fixture
        .host
        .current_app
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .background = false;
    launch.client.uuid = "another-device".into();
    assert!(validate_background_launch(&fixture.host, &launch).is_ok());
    launch
        .client
        .extra
        .insert("background_stream".into(), true.into());
    assert!(validate_background_launch(&fixture.host, &launch).is_err());
    for role in [Role::InputOnly, Role::RemoteMonitor] {
        launch.role = role;
        assert!(validate_background_launch(&fixture.host, &launch).is_ok());
    }
}

#[test]
fn a_background_stream_never_plays_its_game_on_the_pc() {
    let fixture = Fixture::new();
    let mut launch = fixture.launch(fixture.client(u32::MAX), Role::Stream);
    launch.host_audio = true;
    // A normal stream and a co-op player keep what the device asked for.
    for background in [None, Some(false)] {
        launch.background = background;
        keep_background_sound_off_the_pc(&mut launch);
        assert!(launch.host_audio);
    }
    launch.background = Some(true);
    keep_background_sound_off_the_pc(&mut launch);
    assert!(!launch.host_audio);
    // So the session's routes move the instance, as the launch reads it.
    let config = butterpollo_config::config::Config::default();
    assert!(!butterpollo_stream::stream::plays_on_pc(
        &config,
        launch.host_audio
    ));
}

#[test]
fn serverinfo_hardware_cache_is_route_scoped_private_and_expires() {
    let now = Instant::now();
    let route = Some((
        "192.168.1.2".parse().unwrap(),
        "192.168.1.3:0".parse().unwrap(),
    ));
    let other = Some((
        "192.168.1.2".parse().unwrap(),
        "192.168.1.4:0".parse().unwrap(),
    ));
    let facts = ("AA:BB:CC:DD:EE:FF".to_owned(), 1000, true);
    let mut cache = ServerInfoCache::default();
    cache.entries.insert(route, (now, facts.clone()));
    assert_eq!(cache.get(route, now + Duration::from_secs(9)), Some(facts));
    assert_eq!(cache.get(other, now + Duration::from_secs(9)), None);
    assert_eq!(cache.get(None, now + Duration::from_secs(9)), None);
    assert_eq!(cache.get(route, now + Duration::from_secs(10)), None);
    assert!(cache.entries.is_empty());
}

#[tokio::test]
async fn hardware_metrics_are_opt_in_authenticated_and_scoped_to_active_video() {
    let f = Fixture::new();
    let path = "/api/hardware-metrics";
    assert_eq!(
        request(&f, false, "GET", path).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(&f, true, "GET", path).await.status(),
        StatusCode::FORBIDDEN
    );
    let mut client = f.client(0);
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("hardware_metrics".into(), "true".into());
    assert_eq!(
        request(&f, true, "GET", path).await.status(),
        StatusCode::FORBIDDEN
    );
    client.perm = VIEW;
    f.host.paired.write().unwrap().clients[0].perm = VIEW;
    assert_eq!(
        request(&f, true, "GET", path).await.status(),
        StatusCode::CONFLICT
    );
    let input = f.session(client.clone(), Role::InputOnly);
    let mut other = client.clone();
    other.uuid = "other".into();
    let theirs = f.session(other, Role::Stream);
    assert_eq!(
        request(&f, true, "GET", path).await.status(),
        StatusCode::CONFLICT
    );
    assert!(f.host.hardware_metrics.lock().unwrap().is_none());
    let mine = f.session(client, Role::Stream);
    {
        let mut config = f.host.config.write().unwrap();
        config.values.insert(
            "hardware_metrics_selection".into(),
            "[cpu_usage_percent]".into(),
        );
        config
            .values
            .insert("hardware_metrics_interval".into(), "10000".into());
    }
    let first = request(&f, true, "GET", path).await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(first.headers()[header::CACHE_CONTROL], "no-store");
    let first: serde_json::Value =
        serde_json::from_slice(&to_bytes(first.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(first["version"], 1);
    assert_eq!(first["interval_ms"], 10000);
    assert!(first["sampled_at_unix_ms"].as_u64().unwrap() > 0);
    assert_eq!(first["gpus"], json!([]));
    tokio::time::sleep(Duration::from_millis(15)).await;
    let second = request(&f, true, "GET", path).await;
    let second: serde_json::Value =
        serde_json::from_slice(&to_bytes(second.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(
        first, second,
        "polling faster than the configured cadence reuses the sample"
    );
    mine.stop();
    assert_eq!(
        request(&f, true, "GET", path).await.status(),
        StatusCode::CONFLICT
    );
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("hardware_metrics".into(), "false".into());
    assert_eq!(
        request(&f, true, "GET", path).await.status(),
        StatusCode::FORBIDDEN
    );
    input.stop();
    theirs.stop();
    f.host.stop.store(true, Ordering::Release);
}

#[tokio::test]
async fn hardware_metrics_capability_requires_opt_in_and_viewer_permission() {
    let f = Fixture::new();
    f.client(VIEW);
    for (enabled, tls, permission, expected) in [
        (false, true, VIEW, "0"),
        (true, false, VIEW, "0"),
        (true, true, 0, "0"),
        (true, true, 1 << 25, "1"),
        (true, true, 1 << 26, "1"),
    ] {
        let config =
            butterpollo_config::config::Config::parse(&format!("hardware_metrics={enabled}"))
                .unwrap();
        f.host.paired.write().unwrap().clients[0].perm = permission;
        let connection = f.connection(tls);
        let client = authenticated(&f.host, &connection, 0).ok();
        let root = document(serverinfo_response(
            &f.host,
            &connection,
            &axum::http::HeaderMap::new(),
            &config,
            None,
            client.as_ref(),
            true,
            String::new(),
            0,
            false,
        ))
        .await;
        assert_eq!(field(&root, "HardwareMetricsVersion"), expected);
    }
    assert!(f.host.hardware_metrics.lock().unwrap().is_none());
}

#[tokio::test]
async fn network_path_requires_paired_tls_viewer_and_returns_only_route_category() {
    let f = Fixture::new();
    let path = "/api/network-path";
    assert_eq!(
        request(&f, false, "GET", path).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(&f, true, "GET", path).await.status(),
        StatusCode::FORBIDDEN
    );
    f.client(0);
    assert_eq!(
        request(&f, true, "GET", path).await.status(),
        StatusCode::FORBIDDEN
    );
    f.host.paired.write().unwrap().clients[0].perm = VIEW;
    let response = request(&f, true, "GET", path).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let value: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 512).await.unwrap()).unwrap();
    assert_eq!(value, json!({"version": 1, "kind": "unknown"}));
    for (tls, permission, expected) in [(false, VIEW, "0"), (true, 0, "0"), (true, VIEW, "1")] {
        f.host.paired.write().unwrap().clients[0].perm = permission;
        let connection = f.connection(tls);
        let client = authenticated(&f.host, &connection, 0).ok();
        let root = document(serverinfo_response(
            &f.host,
            &connection,
            &axum::http::HeaderMap::new(),
            &butterpollo_config::config::Config::default(),
            None,
            client.as_ref(),
            true,
            String::new(),
            0,
            false,
        ))
        .await;
        assert_eq!(field(&root, "NetworkPathVersion"), expected);
    }
}

#[tokio::test]
async fn xml_preserves_protocol_status_and_escapes_values_and_errors() {
    let text = "<Game & \"screen\"> '日本語'";
    let root = document(xml(403, &[("AppTitle", text.into())], Some(text.into()))).await;
    assert_eq!(root.attributes["status_code"], "403");
    assert_eq!(root.attributes["status_message"], text);
    assert_eq!(field(&root, "AppTitle"), text);
    assert_eq!(root.children.len(), 1);
}

#[tokio::test]
async fn serverinfo_shows_a_devices_game_on_both_its_screens_and_no_other_device() {
    let f = Fixture::new();
    let client = f.client(0x071f1f00);
    let app = f.host.apps.read().unwrap()[0].clone();
    let mut running =
        butterpollo_stream::process::RunningApp::with_environment(&app, Default::default())
            .unwrap();
    running.owner = client.uuid.clone();
    let id = running.id;
    *f.host.current_app.lock().unwrap() = Some(running);
    let config = butterpollo_config::config::Config::default();
    let mut other = client.clone();
    other.uuid = "another-device".into();
    for (asking, expected) in [
        (client.clone(), id),
        (client.clone().second_screen(), id),
        (other.clone(), 0),
        (other.second_screen(), 0),
    ] {
        let root = document(serverinfo_response(
            &f.host,
            &f.connection(true),
            &axum::http::HeaderMap::new(),
            &config,
            None,
            Some(&asking),
            true,
            String::new(),
            0,
            false,
        ))
        .await;
        assert_eq!(
            field(&root, "currentgame"),
            expected.to_string(),
            "{}",
            asking.uuid
        );
    }
}

#[tokio::test]
async fn serverinfo_extras_preserve_permissions_codecs_commands_and_limiter_units() {
    let f = Fixture::new();
    let client = f.client(0x071f1f00);
    let config = butterpollo_config::config::Config::parse("sunshine_name=PC <&>\nvirtual_display_mode=shared\nframe_limiter_fps_limit=59.94\nserver_cmd=[{\"name\":\"Lock <&>\",\"cmd\":\"must never run\"},{\"name\":\"Sleep\"}]").unwrap();
    f.host
        .codecs
        .store(0x4000_0000 | 0x0003_0301, Ordering::Release);
    for ready in [false, true] {
        let root = document(serverinfo_response(
            &f.host,
            &f.connection(true),
            &axum::http::HeaderMap::new(),
            &config,
            None,
            Some(&client),
            true,
            "01:02:03:04:05:06".into(),
            1000,
            ready,
        ))
        .await;
        for (name, expected) in [
            ("hostname", "PC <&>"),
            ("PairStatus", "1"),
            ("ServerCodecModeSupport", "197377"),
            ("MaxLumaPixelsHEVC", "1869449984"),
            ("FrameLimiterSupported", "1"),
            ("FrameLimiterEnabled", "1"),
            ("VirtualDisplayFrameLimiterEnabled", "1"),
            ("FrameLimiterFpsLimitMilliHz", "59940"),
            ("VirtualDisplayCapable", "true"),
            ("VirtualDisplayHDRCapable", "true"),
            ("state", "SUNSHINE_SERVER_FREE"),
            ("currentgame", "0"),
            ("RustHostEncoderBitrateKbps", "0"),
            ("RustHostInitialEncoderBitrateKbps", "0"),
            ("RustHostSessionCount", "0"),
            ("RustHostPendingSessionCount", "0"),
            ("LocalIP", "127.0.0.1"),
            ("mac", "01:02:03:04:05:06"),
            ("PyroWaveHostLinkMbps", "1000"),
            ("PyroWaveBandwidthProbeBytes", "33554432"),
        ] {
            assert_eq!(field(&root, name), expected, "{name}");
        }
        assert_eq!(field(&root, "Permission"), client.perm.to_string());
        assert_eq!(field(&root, "VirtualDisplayDriverReady"), ready.to_string());
        let commands: Vec<_> = root
            .children
            .iter()
            .filter_map(xmltree::XMLNode::as_element)
            .filter(|e| e.name == "ServerCommand")
            .map(|e| e.get_text().unwrap().into_owned())
            .collect();
        assert_eq!(commands, ["Lock <&>", "Sleep"]);
    }
    let mut c = f.connection(false);
    c.local = "[::1]:47989".parse().unwrap();
    c.peer = "[2001:db8::1]:12345".parse().unwrap();
    let root = document(serverinfo_response(
        &f.host,
        &c,
        &axum::http::HeaderMap::new(),
        &config,
        None,
        None,
        true,
        "00:00:00:00:00:00".into(),
        0,
        false,
    ))
    .await;
    assert_eq!(field(&root, "LocalIP"), "127.0.0.1");
    assert_eq!(field(&root, "Permission"), "0");
    assert_eq!(field(&root, "PairStatus"), "0");
    assert_eq!(field(&root, "RustHostProfile"), "");
    assert_eq!(field(&root, "RustHostSessionCount"), "");
    assert_eq!(field(&root, "RustHostEncoderBitrateKbps"), "0");
    assert_eq!(field(&root, "RustHostInitialEncoderBitrateKbps"), "0");
    assert_eq!(field(&root, "PyroWaveBandwidthProbeBytes"), "0");
    assert!(root.get_child("ServerCommand").is_none());
    let mut restricted = client;
    restricted.perm &= !(1 << 20);
    let root = document(serverinfo_response(
        &f.host,
        &f.connection(true),
        &axum::http::HeaderMap::new(),
        &config,
        None,
        Some(&restricted),
        true,
        String::new(),
        0,
        false,
    ))
    .await;
    assert!(root.get_child("ServerCommand").is_none());
}

/// `/serverinfo` of a host with the default configuration, byte for byte, unpaired over HTTP
/// and paired over HTTPS. Anything a host adds later must be offered, so these change only with
/// an offer (`HdrDynamicControl`, last); only the host's own name, identity, version, profile
/// and sleep support vary.
/// What serverinfo shows only a program on this PC (session counts, the running game for local
/// maintenance, the host's profile) a tunnelled device never sees, though it connects over
/// loopback: neither from its own loopback address nor from a registered shared one.
#[tokio::test]
async fn serverinfo_shows_a_tunnelled_device_nothing_only_this_pc_sees() {
    let f = Fixture::new();
    let client = f.client(u32::MAX);
    let config = butterpollo_config::config::Config::default();
    let local = document(serverinfo_response(
        &f.host,
        &f.connection(true),
        &axum::http::HeaderMap::new(),
        &config,
        None,
        Some(&client),
        true,
        String::new(),
        0,
        false,
    ))
    .await;
    assert_eq!(field(&local, "RustHostSessionCount"), "0");
    assert_ne!(field(&local, "RustHostProfile"), "");
    for (peer, register) in [("127.77.3.4:40010", false), ("127.0.0.1:40011", true)] {
        let peer: std::net::SocketAddr = peer.parse().unwrap();
        let _mark = register.then(|| rubylight_traverse::peers::register(peer, "phone"));
        let mut connection = f.connection(true);
        connection.peer = peer;
        let root = document(serverinfo_response(
            &f.host,
            &connection,
            &axum::http::HeaderMap::new(),
            &config,
            None,
            Some(&client),
            true,
            String::new(),
            0,
            false,
        ))
        .await;
        for name in [
            "RustHostSessionCount",
            "RustHostPendingSessionCount",
            "RustHostApplicationActive",
            "RustHostProfile",
        ] {
            assert_eq!(field(&root, name), "", "{peer}: {name}");
        }
        // Still a paired device: the fields any paired device sees stay.
        assert_eq!(field(&root, "PairStatus"), "1", "{peer}");
    }
}
