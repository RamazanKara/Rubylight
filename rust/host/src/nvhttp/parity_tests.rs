use super::*;
use crate::state::{OneTimePin, test_support::Fixture};
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use butterpollo_core::power_action::SleepStates;
use std::sync::atomic::Ordering;
use tower::ServiceExt;

thread_local! {
    static SESSION_ERROR: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

#[test]
fn background_game_can_only_resume_on_its_owner_in_the_same_mode() {
    let fixture = Fixture::new();
    let mut client = fixture.client(u32::MAX);
    client.extra.insert("background_stream".into(), true.into());
    let mut launch = fixture.launch(client, Role::Stream);
    assert!(validate_background_launch(&fixture.host, &launch).is_ok());
    let app = fixture.host.apps.read().unwrap()[0].clone();
    let mut running =
        crate::process::RunningApp::with_environment(&app, Default::default()).unwrap();
    running.owner = launch.client.uuid.clone();
    running.background = true;
    let generation = running.generation.clone();
    *fixture.host.current_app.lock().unwrap() = Some(running);
    assert!(validate_background_launch(&fixture.host, &launch).is_ok());

    launch.client.uuid = "another-device".into();
    assert!(
        validate_background_launch(&fixture.host, &launch)
            .unwrap_err()
            .to_string()
            .contains("another device")
    );
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

pub(super) fn ensure_streaming_session() -> Result<()> {
    match SESSION_ERROR.get() {
        Some(message) => bail!(message),
        None => Ok(()),
    }
}

/// The clipboard `/actions/clipboard` reaches in tests: in memory, with the
/// real one's checks and conversions. Only
/// `clipboard_images_travel_as_png_within_the_limit` puts an image there.
pub(super) mod clipboard {
    use anyhow::Result;
    use butterpollo_core::clipboard_image::{self, ImageError};
    use std::sync::Mutex;
    pub(super) static PNG: Mutex<Option<Vec<u8>>> = Mutex::new(None);
    pub fn read() -> Result<String> {
        Ok(String::new())
    }
    pub fn write(_text: &str) -> Result<()> {
        Ok(())
    }
    pub fn read_png(max_bytes: usize) -> Result<Option<Vec<u8>>> {
        match PNG.lock().unwrap().clone() {
            Some(png) if png.len() > max_bytes => Err(ImageError::TooLarge.into()),
            png => Ok(png),
        }
    }
    pub fn write_png(png: &[u8]) -> Result<()> {
        let image = clipboard_image::decode_png(png)?;
        *PNG.lock().unwrap() = Some(clipboard_image::encode_png(&image)?);
        Ok(())
    }
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

async fn document(response: Response) -> xmltree::Element {
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("application/xml")
    );
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    xmltree::Element::parse(bytes.as_ref()).unwrap()
}
fn field(root: &xmltree::Element, name: &str) -> String {
    root.get_child(name)
        .unwrap()
        .get_text()
        .unwrap_or_default()
        .into_owned()
}
async fn request(f: &Fixture, https: bool, method: &str, path: &str) -> Response {
    router(f.host.clone(), https)
        .layer(Extension(f.connection(https)))
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
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
            butterpollo_core::config::Config::parse(&format!("hardware_metrics={enabled}"))
                .unwrap();
        f.host.paired.write().unwrap().clients[0].perm = permission;
        let connection = f.connection(tls);
        let client = authenticated(&f.host, &connection, 0).ok();
        let root = document(serverinfo_response(
            &f.host,
            &connection,
            &config,
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
            &butterpollo_core::config::Config::default(),
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
async fn serverinfo_extras_preserve_permissions_codecs_commands_and_limiter_units() {
    let f = Fixture::new();
    let client = f.client(0x071f1f00);
    let config = butterpollo_core::config::Config::parse("sunshine_name=PC <&>\nvirtual_display_mode=shared\nframe_limiter_fps_limit=59.94\nserver_cmd=[{\"name\":\"Lock <&>\",\"cmd\":\"must never run\"},{\"name\":\"Sleep\"}]").unwrap();
    f.host
        .codecs
        .store(0x4000_0000 | 0x0003_0301, Ordering::Release);
    for ready in [false, true] {
        let root = document(serverinfo_response(
            &f.host,
            &f.connection(true),
            &config,
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
        &config,
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
        &config,
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
/// an offer (`PipelineRatesVersion`, last); only the host's own name, identity, version, profile
/// and sleep support vary.
#[tokio::test]
async fn serverinfo_for_a_default_config_is_byte_identical() {
    let f = Fixture::new();
    let client = f.client(u32::MAX);
    let config = butterpollo_core::config::Config::default();
    let text = |response: Response| async move {
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    };
    let unpaired = text(serverinfo_response(
        &f.host,
        &f.connection(false),
        &config,
        None,
        true,
        "00:00:00:00:00:00".into(),
        0,
        false,
    ))
    .await;
    let paired = text(serverinfo_response(
        &f.host,
        &f.connection(true),
        &config,
        Some(&client),
        true,
        "01:02:03:04:05:06".into(),
        1000,
        true,
    ))
    .await;
    let hostname = crate::network::host_name(&config);
    let unique_id = f.host.paired.read().unwrap().unique_id.clone();
    let version = env!("CARGO_PKG_VERSION");
    let profile = butterpollo_core::migration::profile_id(&f.host.directory);
    let sleep = u8::from(f.host.power.supported());
    // AudioCodecs 3 is Opus and PCM: the test binary's folder has no FFmpeg for AC3 and E-AC3.
    assert_eq!(
        paired,
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\">\
             <NetworkPathVersion>1</NetworkPathVersion>\
             <HardwareMetricsVersion>0</HardwareMetricsVersion>\
             <NativeApiVersion>1</NativeApiVersion>\
             <hostname>{hostname}</hostname>\
             <appversion>7.1.431.-1</appversion>\
             <GfeVersion>3.23.0.74</GfeVersion>\
             <uniqueid>{unique_id}</uniqueid>\
             <HttpsPort>47984</HttpsPort>\
             <ExternalPort>47989</ExternalPort>\
             <PairStatus>1</PairStatus>\
             <currentgame>0</currentgame>\
             <currentgameuuid></currentgameuuid>\
             <state>SUNSHINE_SERVER_FREE</state>\
             <LocalIP>127.0.0.1</LocalIP>\
             <mac>01:02:03:04:05:06</mac>\
             <MaxLumaPixelsHEVC>1869449984</MaxLumaPixelsHEVC>\
             <ServerCodecModeSupport>769</ServerCodecModeSupport>\
             <RustHostVersion>{version}</RustHostVersion>\
             <RustHostEncoderBitrateKbps>0</RustHostEncoderBitrateKbps>\
             <RustHostInitialEncoderBitrateKbps>0</RustHostInitialEncoderBitrateKbps>\
             <RustHostSessionCount>0</RustHostSessionCount>\
             <RustHostPendingSessionCount>0</RustHostPendingSessionCount>\
             <RustHostApplicationActive>0</RustHostApplicationActive>\
             <RustHostProfile>{profile}</RustHostProfile>\
             <PyroWaveHostLinkMbps>1000</PyroWaveHostLinkMbps>\
             <PyroWaveBandwidthProbeBytes>33554432</PyroWaveBandwidthProbeBytes>\
             <PyroWaveUdpProbeVersion>1</PyroWaveUdpProbeVersion>\
             <PyroWaveUdpHandshakeVersion>1</PyroWaveUdpHandshakeVersion>\
             <PyroWaveUdpProbeBurstVersion>1</PyroWaveUdpProbeBurstVersion>\
             <PyroWaveWireBudgetVersion>1</PyroWaveWireBudgetVersion>\
             <PyroWaveCriticalFecPercentage>20</PyroWaveCriticalFecPercentage>\
             <PyroWaveMinParityShards>2</PyroWaveMinParityShards>\
             <Permission>4294967295</Permission>\
             <FrameLimiterSupported>1</FrameLimiterSupported>\
             <FrameLimiterEnabled>1</FrameLimiterEnabled>\
             <VirtualDisplayFrameLimiterEnabled>1</VirtualDisplayFrameLimiterEnabled>\
             <FrameLimiterFpsLimitMilliHz>0</FrameLimiterFpsLimitMilliHz>\
             <VirtualDisplayCapable>true</VirtualDisplayCapable>\
             <VirtualDisplayDriverReady>true</VirtualDisplayDriverReady>\
             <ControllerPreparation>1</ControllerPreparation>\
             <VirtualDisplayHDRCapable>true</VirtualDisplayHDRCapable>\
             <PowerActionVersion>1</PowerActionVersion>\
             <SleepSupported>{sleep}</SleepSupported>\
             <SleepAllowed>0</SleepAllowed>\
             <HdrFormats>15</HdrFormats>\
             <FecControlVersion>1</FecControlVersion>\
             <BandwidthProbeVersion>1</BandwidthProbeVersion>\
             <AudioCodecs>3</AudioCodecs>\
             <AudioLayouts>15</AudioLayouts>\
             <TextInputVersion>1</TextInputVersion>\
             <KeyboardLayoutVersion>1</KeyboardLayoutVersion>\
             <PipelineRatesVersion>1</PipelineRatesVersion>\
             </root>"
        ),
        "{paired}"
    );
    assert_eq!(
        unpaired,
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\">\
             <NetworkPathVersion>0</NetworkPathVersion>\
             <HardwareMetricsVersion>0</HardwareMetricsVersion>\
             <NativeApiVersion>0</NativeApiVersion>\
             <hostname>{hostname}</hostname>\
             <appversion>7.1.431.-1</appversion>\
             <GfeVersion>3.23.0.74</GfeVersion>\
             <uniqueid>{unique_id}</uniqueid>\
             <HttpsPort>47984</HttpsPort>\
             <ExternalPort>47989</ExternalPort>\
             <PairStatus>0</PairStatus>\
             <currentgame>0</currentgame>\
             <currentgameuuid></currentgameuuid>\
             <state>SUNSHINE_SERVER_FREE</state>\
             <LocalIP>127.0.0.1</LocalIP>\
             <mac>00:00:00:00:00:00</mac>\
             <MaxLumaPixelsHEVC>1869449984</MaxLumaPixelsHEVC>\
             <ServerCodecModeSupport>769</ServerCodecModeSupport>\
             <RustHostVersion>{version}</RustHostVersion>\
             <RustHostEncoderBitrateKbps>0</RustHostEncoderBitrateKbps>\
             <RustHostInitialEncoderBitrateKbps>0</RustHostInitialEncoderBitrateKbps>\
             <RustHostSessionCount>0</RustHostSessionCount>\
             <RustHostPendingSessionCount>0</RustHostPendingSessionCount>\
             <RustHostApplicationActive>0</RustHostApplicationActive>\
             <RustHostProfile>{profile}</RustHostProfile>\
             <PyroWaveHostLinkMbps>0</PyroWaveHostLinkMbps>\
             <PyroWaveBandwidthProbeBytes>0</PyroWaveBandwidthProbeBytes>\
             <PyroWaveUdpProbeVersion>0</PyroWaveUdpProbeVersion>\
             <PyroWaveUdpHandshakeVersion>0</PyroWaveUdpHandshakeVersion>\
             <PyroWaveUdpProbeBurstVersion>0</PyroWaveUdpProbeBurstVersion>\
             <PyroWaveWireBudgetVersion>0</PyroWaveWireBudgetVersion>\
             <PyroWaveCriticalFecPercentage></PyroWaveCriticalFecPercentage>\
             <PyroWaveMinParityShards></PyroWaveMinParityShards>\
             <Permission>0</Permission>\
             <FrameLimiterSupported>1</FrameLimiterSupported>\
             <FrameLimiterEnabled>1</FrameLimiterEnabled>\
             <VirtualDisplayFrameLimiterEnabled>1</VirtualDisplayFrameLimiterEnabled>\
             <FrameLimiterFpsLimitMilliHz>0</FrameLimiterFpsLimitMilliHz>\
             <VirtualDisplayCapable>true</VirtualDisplayCapable>\
             <VirtualDisplayDriverReady>false</VirtualDisplayDriverReady>\
             <ControllerPreparation>1</ControllerPreparation>\
             <VirtualDisplayHDRCapable>true</VirtualDisplayHDRCapable>\
             <HdrFormats>15</HdrFormats>\
             <FecControlVersion>1</FecControlVersion>\
             <BandwidthProbeVersion>1</BandwidthProbeVersion>\
             <AudioCodecs>3</AudioCodecs>\
             <AudioLayouts>15</AudioLayouts>\
             <TextInputVersion>1</TextInputVersion>\
             <KeyboardLayoutVersion>1</KeyboardLayoutVersion>\
             <PipelineRatesVersion>1</PipelineRatesVersion>\
             </root>"
        ),
        "{unpaired}"
    );
}

#[tokio::test]
async fn serverinfo_carries_the_packed_444_modes_only_when_packed_is_on() {
    use butterpollo_core::packed444::mode_bit;
    let f = Fixture::new();
    let client = f.client(0x071f1f00);
    let packed = |config: &'static str| {
        let host = f.host.clone();
        let connection = f.connection(true);
        let client = client.clone();
        async move {
            let config = butterpollo_core::config::Config::parse(config).unwrap();
            let root = document(serverinfo_response(
                &host,
                &connection,
                &config,
                Some(&client),
                true,
                String::new(),
                0,
                false,
            ))
            .await;
            root.get_child("Packed444Modes")
                .map(|element| element.get_text().unwrap_or_default().into_owned())
        }
    };
    // A client could ask for 10-bit packed, be refused at ANNOUNCE and lose
    // the session; it now sees the startup probe's result before /launch.
    f.host.packed_modes.store(0, Ordering::Release);
    assert_eq!(
        packed("experimental_packed_444=true").await.as_deref(),
        Some("0")
    );
    f.host
        .packed_modes
        .store(mode_bit(1, false) | mode_bit(2, true), Ordering::Release);
    assert_eq!(
        packed("experimental_packed_444=true").await.as_deref(),
        Some("9")
    );
    assert_eq!(packed("experimental_packed_444=false").await, None);
    assert_eq!(packed("").await, None);
}

#[tokio::test]
async fn successful_launch_and_resume_xml_include_driver_readiness_and_the_called_endpoint() {
    for (reply, resume) in [("gamesession", false), ("resume", true)] {
        for ready in [false, true] {
            let url = "rtspenc://[::1]:48010";
            let root = document(launch_response(
                Ok((reply.into(), url.into())),
                1,
                reply,
                resume,
                ready,
            ))
            .await;
            assert_eq!(root.attributes["status_code"], "200");
            assert_eq!(field(&root, reply), "1");
            assert_eq!(field(&root, "sessionUrl0"), url);
            assert_eq!(field(&root, "VirtualDisplayDriverReady"), ready.to_string());
            assert_eq!(root.children.len(), 3);
        }
    }
}

#[test]
fn paired_permissions_require_tls_an_enabled_certificate_and_each_requested_bit() {
    let f = Fixture::new();
    f.client(1 << 25);
    let c = f.connection(true);
    assert!(authenticated_viewer(&f.host, &c).is_ok());
    assert!(authenticated(&f.host, &c, 1 << 26).is_err());
    assert!(authenticated(&f.host, &c, VIEW).is_err());
    assert!(authenticated(&f.host, &f.connection(false), 0).is_err());
    let mut unknown = c.clone();
    unknown.certificate = Some(vec![1, 2, 3]);
    assert!(authenticated(&f.host, &unknown, 0).is_err());
    unknown.certificate = None;
    assert!(authenticated(&f.host, &unknown, 0).is_err());
    f.host.paired.write().unwrap().clients[0].perm = 1 << 26;
    assert!(authenticated_viewer(&f.host, &c).is_ok());
    assert!(validate_launch_client(&f.host, &c, None, false).is_ok());
    f.host.paired.write().unwrap().clients[0].enabled = false;
    assert!(authenticated(&f.host, &c, 0).is_err());
    assert!(authenticated_viewer(&f.host, &c).is_err());
}

fn otp_request(f: &Fixture) -> Args {
    Args::from([
        ("uniqueid".into(), "otp-fixture".into()),
        ("phrase".into(), "getservercert".into()),
        ("salt".into(), hex::encode([0x12; 16])),
        (
            "clientcert".into(),
            hex::encode(f.host.identity.certificate.as_bytes()),
        ),
        ("devicename".into(), "Client name".into()),
        (
            "otpauth".into(),
            hex::encode(crypto::hash(
                format!("1234{}passphrase", hex::encode([0x12; 16])).as_bytes(),
            )),
        ),
    ])
}
fn set_otp(f: &Fixture, age: Duration) {
    *f.host.otp.lock().unwrap() = Some(OneTimePin {
        pin: "1234".into(),
        passphrase: "passphrase".into(),
        device_name: "Console name".into(),
        created: Instant::now() - age,
        attempts: 0,
    });
}

#[tokio::test]
async fn otp_handshake_persists_the_device_name_and_consumes_the_pin_once() {
    let f = Fixture::new();
    set_otp(&f, Duration::ZERO);
    let args = otp_request(&f);
    let peer = "127.0.0.1".parse().unwrap();
    let start = do_pair(f.host.clone(), &args, peer).await.unwrap();
    assert_eq!(start[0], ("paired".into(), "1".into()));
    assert!(f.host.otp.lock().unwrap().is_none());
    let aes = crypto::pin_key(&[0x12; 16], "1234");
    let challenge = [0x34; 16];
    let step = |name: &str, value: Vec<u8>| {
        Args::from([
            ("uniqueid".into(), "otp-fixture".into()),
            (name.into(), hex::encode(value)),
        ])
    };
    let response = do_pair(
        f.host.clone(),
        &step(
            "clientchallenge",
            crypto::ecb(&aes, &challenge, true).unwrap(),
        ),
        peer,
    )
    .await
    .unwrap();
    let response = crypto::ecb(&aes, &hex::decode(&response[1].1).unwrap(), false).unwrap();
    let secret = [0x56; 16];
    let proof = crypto::hash(&[&response[32..], &f.host.identity.signature, &secret].concat());
    let server = do_pair(
        f.host.clone(),
        &step(
            "serverchallengeresp",
            crypto::ecb(&aes, &proof, true).unwrap(),
        ),
        peer,
    )
    .await
    .unwrap();
    let server = hex::decode(&server[1].1).unwrap();
    assert_eq!(
        &response[..32],
        crypto::hash(
            &[
                challenge.as_slice(),
                &f.host.identity.signature,
                &server[..16]
            ]
            .concat()
        )
    );
    assert!(crypto::verify(&f.host.identity.certificate, &server[..16], &server[16..]).unwrap());
    do_pair(
        f.host.clone(),
        &step(
            "clientpairingsecret",
            [&secret[..], &f.host.identity.sign(&secret)].concat(),
        ),
        peer,
    )
    .await
    .unwrap();
    let saved = butterpollo_core::state::PairedState::load(&f.host.paired_path).unwrap();
    assert_eq!(saved.clients.len(), 1);
    assert_eq!(saved.clients[0].name, "Console name");
    assert_eq!(
        saved.clients[0].perm,
        butterpollo_core::state::FIRST_DEVICE_PERMISSION
    );
    assert!(saved.clients[0].enabled);
    // Pairing never grants Host control, even to the first device.
    assert!(!saved.clients[0].extra.contains_key("host_control"));
    assert!(!saved.clients[0].allows_host_control());
    assert!(f.host.pairings.lock().unwrap().sessions.is_empty());
    assert!(
        do_pair(f.host.clone(), &args, peer)
            .await
            .unwrap_err()
            .to_string()
            .contains("not available")
    );
}

#[tokio::test]
async fn otp_expiry_wrong_auth_and_pairing_disable_do_not_create_devices() {
    let f = Fixture::new();
    let mut args = otp_request(&f);
    let peer = "127.0.0.1".parse().unwrap();
    set_otp(&f, Duration::from_secs(181));
    assert!(do_pair(f.host.clone(), &args, peer).await.is_err());
    assert!(f.host.otp.lock().unwrap().is_none());
    set_otp(&f, Duration::ZERO);
    args.insert("otpauth".into(), "wrong".into());
    assert!(do_pair(f.host.clone(), &args, peer).await.is_ok());
    assert!(f.host.otp.lock().unwrap().is_some());
    assert!(f.host.paired.read().unwrap().clients.is_empty());
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("enable_pairing".into(), "false".into());
    assert_eq!(
        do_pair(f.host.clone(), &args, peer)
            .await
            .unwrap_err()
            .to_string(),
        "pairing is disabled"
    );
    assert!(f.host.otp.lock().unwrap().is_some());
}

#[tokio::test]
async fn one_time_pin_is_disabled_after_five_wrong_attempts() {
    let f = Fixture::new();
    set_otp(&f, Duration::ZERO);
    let mut args = otp_request(&f);
    args.insert("otpauth".into(), "wrong".into());
    for _ in 0..5 {
        assert!(
            do_pair(f.host.clone(), &args, "127.0.0.1".parse().unwrap())
                .await
                .is_ok()
        );
    }
    assert!(f.host.otp.lock().unwrap().is_none());
    assert!(
        do_pair(f.host.clone(), &args, "127.0.0.1".parse().unwrap())
            .await
            .is_err()
    );
    assert!(f.host.paired.read().unwrap().clients.is_empty());
}

#[tokio::test]
async fn pairing_ids_are_scoped_to_the_peer_and_pending_requests_are_bounded() {
    let f = Fixture::new();
    let mut args = otp_request(&f);
    args.remove("otpauth");
    let peer = "192.0.2.1".parse().unwrap();
    let other = "192.0.2.2".parse().unwrap();
    let id = pairing_id(&args["uniqueid"], peer);
    let (sender, _receiver) = tokio::sync::oneshot::channel();
    f.host.pins.lock().unwrap().insert(
        id,
        PendingPin {
            name: "Existing".into(),
            certificate: "different certificate".into(),
            created: Instant::now(),
            sender,
        },
    );
    assert!(do_pair(f.host.clone(), &args, peer).await.is_err());
    let host = f.host.clone();
    let request = args.clone();
    let task = tokio::spawn(async move { do_pair(host, &request, other).await });
    tokio::task::yield_now().await;
    let key = pairing_id(&args["uniqueid"], other);
    assert!(f.host.pins.lock().unwrap().contains_key(&key));
    task.abort();
    let (sender, _receiver) = tokio::sync::oneshot::channel();
    f.host.pins.lock().unwrap().insert(
        pairing_id("second", peer),
        PendingPin {
            name: "Second".into(),
            certificate: "different certificate".into(),
            created: Instant::now(),
            sender,
        },
    );
    args.insert("uniqueid".into(), "third".into());
    assert!(
        do_pair(f.host.clone(), &args, peer)
            .await
            .unwrap_err()
            .to_string()
            .contains("limit reached")
    );
}

#[tokio::test]
async fn applist_permissions_placeholder_and_hdr_xml_match_moonlight() {
    let f = Fixture::new();
    f.client(1 << 25);
    let denied = document(request(&f, true, "GET", "/applist").await).await;
    let app = denied.get_child("App").unwrap();
    assert_eq!(field(app, "ID"), "114514");
    assert_eq!(field(app, "IsHdrSupported"), "0");
    assert_eq!(
        field(app, "AppTitle"),
        "Permission denied - enable \"List applications\" for this device in the host's Web UI"
    );
    assert_eq!(denied.children.len(), 1);
    f.host.paired.write().unwrap().clients[0].perm = 1 << 24;
    let title = "Game <&> \"日本語\"";
    f.host.apps.write().unwrap()[0].name = title.into();
    let allowed = document(request(&f, true, "GET", "/applist").await).await;
    assert_eq!(allowed.children.len(), f.host.apps.read().unwrap().len());
    let app = allowed.get_child("App").unwrap();
    assert_eq!(field(app, "AppTitle"), title);
    assert_eq!(
        field(app, "ID"),
        f.host.apps.read().unwrap()[0].id().to_string()
    );
    assert_eq!(field(app, "IsHdrSupported"), "1");
    assert!(!field(app, "UUID").is_empty());
    f.host.paired.write().unwrap().clients[0].enabled = false;
    let disabled = document(request(&f, true, "GET", "/applist").await).await;
    assert_eq!(disabled.attributes["status_code"], "401");
    assert!(disabled.children.is_empty());
}

#[tokio::test]
async fn unpair_get_and_post_require_tls_and_stop_only_the_paired_device() {
    for method in ["GET", "POST"] {
        let f = Fixture::new();
        let client = f.client(u32::MAX);
        let mine = f.session(client.clone(), Role::InputOnly);
        let mut other = client;
        other.uuid = "another-device".into();
        let theirs = f.session(other, Role::RemoteMonitor);
        let plain = document(request(&f, false, method, "/unpair").await).await;
        assert_eq!(field(&plain, "unpaired"), "0");
        assert_eq!(f.host.paired.read().unwrap().clients.len(), 1);
        assert!(!mine.stopping());
        let secure = document(request(&f, true, method, "/unpair").await).await;
        assert_eq!(field(&secure, "unpaired"), "1");
        assert!(mine.stopping());
        assert!(!theirs.stopping());
        assert!(
            butterpollo_core::state::PairedState::load(&f.host.paired_path)
                .unwrap()
                .clients
                .is_empty()
        );
        assert_eq!(
            field(
                &document(request(&f, true, method, "/unpair").await).await,
                "unpaired"
            ),
            "0"
        );
    }
}

#[tokio::test]
async fn launch_and_resume_reject_disconnected_sessions_before_preparation() {
    let f = Fixture::new();
    f.client(1 << 26);
    let app_id = f.host.apps.read().unwrap()[0].id();
    let args = Args::from([
        ("appid".into(), app_id.to_string()),
        ("rikey".into(), "00000000000000000000000000000000".into()),
        ("rikeyid".into(), "1".into()),
    ]);
    let message = "The host Windows session is disconnected after RDP. Retry in a moment.";
    for (resume, reply) in [(false, "gamesession"), (true, "resume")] {
        SESSION_ERROR.set(Some(message));
        let response = start(f.host.clone(), f.connection(true), args.clone(), resume);
        SESSION_ERROR.set(None);
        let root = document(response).await;
        assert_eq!(root.attributes["status_code"], "503");
        assert_eq!(root.attributes["status_message"], message);
        assert_eq!(field(&root, reply), "0");
        assert!(root.get_child("sessionUrl0").is_none());
        assert!(f.host.sessions.lock().unwrap().pending.is_empty());
        assert!(f.host.current_app.lock().unwrap().is_none());
        assert!(f.host.app_display.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn the_session_gate_keeps_authorization_and_terminate_semantics() {
    let f = Fixture::new();
    f.client(1 << 24);
    SESSION_ERROR.set(Some("session gate must not run for these requests"));
    let unauthorized = start(f.host.clone(), f.connection(true), Args::new(), false);
    f.host.paired.write().unwrap().clients[0].perm = 1 << 26;
    let terminate = start(
        f.host.clone(),
        f.connection(true),
        Args::from([("appid".into(), "2147483504".into())]),
        false,
    );
    SESSION_ERROR.set(None);
    assert_eq!(
        document(unauthorized).await.attributes["status_code"],
        "401"
    );
    let terminate = document(terminate).await;
    assert_eq!(terminate.attributes["status_code"], "409");
    assert_eq!(
        terminate.attributes["status_message"],
        "No application is running"
    );
    assert!(f.host.sessions.lock().unwrap().pending.is_empty());
}

#[tokio::test]
async fn launch_and_resume_reject_permissions_and_report_protocol_failures() {
    let f = Fixture::new();
    f.client(1 << 24);
    for endpoint in ["launch", "resume"] {
        let root = document(request(&f, true, "GET", &format!("/{endpoint}")).await).await;
        assert_eq!(root.attributes["status_code"], "401");
    }
    f.host.paired.write().unwrap().clients[0].perm = 1 << 26;
    for (endpoint, reply) in [("launch", "gamesession"), ("resume", "resume")] {
        let root = document(request(&f, true, "GET", &format!("/{endpoint}")).await).await;
        assert_eq!(root.attributes["status_code"], "503");
        assert_eq!(field(&root, reply), "0");
        assert_eq!(root.attributes["status_message"], "missing stream key");
    }
    assert!(f.host.sessions.lock().unwrap().pending.is_empty());
}

#[tokio::test]
async fn an_unavailable_encoder_does_not_queue_a_launch_or_start_an_app() {
    let f = Fixture::new();
    f.client(1 << 26);
    f.host.codecs.store(0, Ordering::Release);
    let app_id = f.host.apps.read().unwrap()[0].id();
    let root = document(
        request(
            &f,
            true,
            "GET",
            &format!("/launch?appid={app_id}&rikey=00000000000000000000000000000000&rikeyid=1"),
        )
        .await,
    )
    .await;
    assert_eq!(root.attributes["status_code"], "503");
    let message = &root.attributes["status_message"];
    assert!(message.contains("No video encoder is ready"), "{message}");
    assert!(message.contains("AMD Radeon GPU"), "{message}");
    assert!(message.contains("update the Radeon driver"), "{message}");
    assert!(message.contains("stay on Rubylight 2.6.x"), "{message}");
    assert!(message.contains("/releases/tag/v2.6.0"), "{message}");
    assert!(f.host.sessions.lock().unwrap().pending.is_empty());
    assert!(f.host.current_app.lock().unwrap().is_none());
    assert!(f.host.app_display.lock().unwrap().is_empty());
}

#[tokio::test]
async fn discovery_does_not_wait_for_a_stuck_encoder_probe() {
    let f = Fixture::new();
    f.client(1 << 24);
    // The startup probe found nothing; a retry (no signed-in user, a driver
    // failure) must not hold discovery back.
    f.host.codecs.store(0, Ordering::Release);
    f.host.probing_codecs.store(true, Ordering::Release);
    let response = tokio::time::timeout(
        Duration::from_millis(500),
        request(&f, true, "GET", "/applist"),
    )
    .await
    .unwrap();
    let root = document(response).await;
    assert_eq!(root.attributes["status_code"], "200");
    assert_eq!(field(root.get_child("App").unwrap(), "IsHdrSupported"), "0");
}

/// The startup probe publishes `codecs` after a moment, as on a host that
/// Windows or Wake-on-LAN has just started.
fn finish_startup_probe_later(f: &Fixture, codecs: u32) -> std::thread::JoinHandle<()> {
    f.host.codecs.store(0, Ordering::Release);
    f.host.probing_codecs.store(true, Ordering::Release);
    f.host.startup_codecs.send_replace(false);
    let h = f.host.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        h.codecs.store(codecs, Ordering::Release);
        h.probing_codecs.store(false, Ordering::Release);
        h.startup_codecs.send_replace(true);
    })
}

#[tokio::test]
async fn serverinfo_offers_sleep_fields_to_paired_https_clients_only() {
    let f = Fixture::new();
    for https in [false, true] {
        let root = document(request(&f, https, "GET", "/serverinfo").await).await;
        for name in ["PowerActionVersion", "SleepSupported", "SleepAllowed"] {
            assert!(root.get_child(name).is_none(), "{name} https={https}");
        }
    }
    let mut client = f.client(u32::MAX);
    let info = |client: &Client| {
        let config = f.host.config.read().unwrap().clone();
        serverinfo_response(
            &f.host,
            &f.connection(true),
            &config,
            Some(client),
            false,
            "00:00:00:00:00:00".into(),
            0,
            false,
        )
    };
    // Tests have no sleep backend.
    let root = document(info(&client)).await;
    assert_eq!(field(&root, "PowerActionVersion"), "1");
    assert_eq!(field(&root, "SleepSupported"), "0");
    assert_eq!(field(&root, "SleepAllowed"), "0");
    client.extra.insert("allow_sleep".into(), true.into());
    assert_eq!(field(&document(info(&client)).await, "SleepAllowed"), "1");
    // Only S1 to S3 count: Modern Standby alone hibernated a laptop.
    let supported = |states| {
        f.host
            .power
            .set_backend(std::sync::Arc::new(crate::power_action::FixedStates(
                states,
            )));
        let root = info(&client);
        async move { field(&document(root).await, "SleepSupported") }
    };
    let s3 = SleepStates {
        s3: true,
        hibernate: true,
        ..Default::default()
    };
    assert_eq!(supported(s3).await, "1");
    for hibernate in [true, false] {
        let laptop = SleepStates {
            modern_standby: true,
            hibernate,
            ..Default::default()
        };
        assert_eq!(supported(laptop).await, "0", "hibernate {hibernate}");
    }
}

#[tokio::test]
async fn discovery_during_the_startup_probe_reports_its_codecs() {
    let f = Fixture::new();
    let probe = finish_startup_probe_later(&f, 0x1 | 0x100 | 0x200);
    let root = document(request(&f, false, "GET", "/serverinfo").await).await;
    probe.join().unwrap();
    // Not 0, which a client caches as a host without HEVC or HDR.
    assert_eq!(field(&root, "ServerCodecModeSupport"), "769");
}

#[tokio::test]
async fn a_launch_during_the_startup_probe_waits_for_its_result() {
    let f = Fixture::new();
    f.client(1 << 26);
    // Stop the launch at a later step, before any display is prepared.
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("lan_encryption_mode".into(), "2".into());
    let app_id = f.host.apps.read().unwrap()[0].id();
    let args = Args::from([
        ("appid".into(), app_id.to_string()),
        ("rikey".into(), "00000000000000000000000000000000".into()),
        ("rikeyid".into(), "1".into()),
    ]);
    let probe = finish_startup_probe_later(&f, 0x1 | 0x100);
    let launch = {
        let (h, connection) = (f.host.clone(), f.connection(true));
        tokio::task::spawn_blocking(move || start(h, connection, args, false))
            .await
            .unwrap()
    };
    probe.join().unwrap();
    let root = document(launch).await;
    let message = &root.attributes["status_message"];
    assert!(!message.contains("No video encoder is ready"), "{message}");
    assert!(message.contains("encrypted RTSP is required"), "{message}");
    assert!(f.host.sessions.lock().unwrap().pending.is_empty());
}

#[tokio::test]
async fn a_stuck_startup_probe_holds_requests_only_for_the_limit() {
    let f = Fixture::new();
    f.host.startup_codecs.send_replace(false);
    let limit = Duration::from_millis(100);
    let started = Instant::now();
    f.host.wait_for_startup_codecs(limit).await;
    f.host.wait_for_startup_codecs_blocking(limit);
    let waited = started.elapsed();
    assert!(
        waited >= 2 * limit && waited < Duration::from_secs(2),
        "{waited:?}"
    );
    // Once the startup probe has finished, nothing waits again.
    f.host.startup_codecs.send_replace(true);
    let started = Instant::now();
    f.host.wait_for_startup_codecs(Duration::from_secs(5)).await;
    f.host
        .wait_for_startup_codecs_blocking(Duration::from_secs(5));
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[tokio::test]
async fn launch_failures_preserve_the_actionable_cause() {
    let error = anyhow::anyhow!("executable was not found").context("application could not start");
    let root = document(launch_response(Err(error), 1, "gamesession", false, false)).await;
    assert_eq!(
        root.attributes["status_message"],
        "application could not start: executable was not found"
    );
}

#[tokio::test]
async fn virtual_display_advertisement_matches_configuration_and_codecs() {
    let f = Fixture::new();
    for (config, windows_11, codecs, capable, hdr) in [
        ("", false, 0x200, "false", "false"),
        (
            "virtual_display_mode=disabled",
            true,
            0x200,
            "false",
            "false",
        ),
        ("virtual_display_mode=shared", true, 1, "true", "false"),
        ("virtual_display_mode=shared", true, 0x200, "true", "true"),
    ] {
        let config = butterpollo_core::config::Config::parse(config).unwrap();
        f.host.codecs.store(codecs, Ordering::Release);
        let root = document(serverinfo_response(
            &f.host,
            &f.connection(true),
            &config,
            None,
            windows_11,
            String::new(),
            0,
            true,
        ))
        .await;
        assert_eq!(field(&root, "VirtualDisplayCapable"), capable);
        assert_eq!(field(&root, "VirtualDisplayHDRCapable"), hdr);
    }
}

#[tokio::test]
async fn serverinfo_encoder_bitrate_tracks_only_own_video_sessions() {
    let f = Fixture::new();
    let client = f.client(1 << 25);
    let config = butterpollo_core::config::Config::parse("fec_percentage=20").unwrap();
    let mut negotiated = butterpollo_core::rtsp::Negotiated {
        configured_bitrate_kbps: 50_000,
        audio_channels: 2,
        audio_quality: true,
        ..Default::default()
    };
    butterpollo_core::stream_policy::apply(&mut negotiated, 60_000, &config);
    assert_eq!(negotiated.bitrate_kbps, 38_988);
    let mine = crate::state::Session::new(f.launch(client.clone(), Role::Stream), negotiated);
    f.host
        .sessions
        .lock()
        .unwrap()
        .active
        .insert(mine.launch.id.clone(), mine.clone());
    f.session(client.clone(), Role::InputOnly);
    let stopping = f.session(client.clone(), Role::Stream);
    stopping.stop();
    let mut other = client.clone();
    other.uuid = "other".into();
    let theirs = f.session(other, Role::Stream);
    let mut connection = f.connection(true);
    connection.peer = "192.0.2.1:12345".parse().unwrap();
    let info = || {
        serverinfo_response(
            &f.host,
            &connection,
            &config,
            Some(&client),
            true,
            String::new(),
            0,
            false,
        )
    };
    for current in [38_988, 24_000] {
        if current == 24_000 {
            let applied = document(request(&f, true, "GET", "/bitrate?bitrate=24000").await).await;
            assert_eq!(field(&applied, "bitrate"), "24000");
        }
        let root = document(info()).await;
        assert_eq!(
            field(&root, "RustHostEncoderBitrateKbps"),
            current.to_string()
        );
        assert_eq!(field(&root, "RustHostInitialEncoderBitrateKbps"), "38988");
        assert_eq!(mine.bitrate.load(Ordering::Acquire), current);
        assert_eq!(theirs.bitrate.load(Ordering::Acquire), 20_000);
    }
    let second = crate::state::Session::new(
        f.launch(client.clone(), Role::RemoteMonitor),
        butterpollo_core::rtsp::Negotiated {
            bitrate_kbps: 18_000,
            ..Default::default()
        },
    );
    second.bitrate.store(30_000, Ordering::Release);
    f.host
        .sessions
        .lock()
        .unwrap()
        .active
        .insert(second.launch.id.clone(), second.clone());
    let root = document(info()).await;
    assert_eq!(field(&root, "RustHostEncoderBitrateKbps"), "24000");
    assert_eq!(field(&root, "RustHostInitialEncoderBitrateKbps"), "18000");
    mine.stop();
    second.stop();
    let root = document(info()).await;
    assert_eq!(field(&root, "RustHostEncoderBitrateKbps"), "0");
    assert_eq!(field(&root, "RustHostInitialEncoderBitrateKbps"), "0");
}

#[tokio::test]
async fn serverinfo_encoder_bitrate_requires_authenticated_view_or_launch_permission() {
    let f = Fixture::new();
    let client = f.client(0);
    f.session(client, Role::Stream);
    let config = f.host.config.read().unwrap().clone();
    for (tls, permission, expected) in [
        (true, 0, "0"),
        (true, 1 << 24, "0"),
        (true, 1 << 25, "20000"),
        (true, 1 << 26, "20000"),
        (false, VIEW, "0"),
    ] {
        f.host.paired.write().unwrap().clients[0].perm = permission;
        let connection = f.connection(tls);
        let client = authenticated(&f.host, &connection, 0).ok();
        let root = document(serverinfo_response(
            &f.host,
            &connection,
            &config,
            client.as_ref(),
            true,
            String::new(),
            0,
            false,
        ))
        .await;
        assert_eq!(field(&root, "RustHostEncoderBitrateKbps"), expected);
        assert_eq!(field(&root, "RustHostInitialEncoderBitrateKbps"), expected);
    }
}

#[tokio::test]
async fn bitrate_caps_and_alias_update_only_the_authenticated_devices_sessions() {
    let f = Fixture::new();
    let client = f.client(1 << 25);
    let mine = f.session(client.clone(), Role::Stream);
    let second = f.session(client.clone(), Role::RemoteMonitor);
    let mut other = client;
    other.uuid = "other".into();
    let theirs = f.session(other, Role::Stream);
    let original = theirs.bitrate.load(Ordering::Acquire);
    for (ceiling, query, expected) in [
        ("60000", "bitrate=80000", 60000),
        ("0", "bitrate_kbps=900000", 500000),
        ("0", "bitrate=12345", 12345),
    ] {
        f.host
            .config
            .write()
            .unwrap()
            .values
            .insert("max_bitrate".into(), ceiling.into());
        let root = document(request(&f, true, "GET", &format!("/bitrate?{query}")).await).await;
        assert_eq!(root.attributes["status_code"], "200");
        assert_eq!(field(&root, "bitrate"), expected.to_string());
        assert_eq!(field(&root, "updated"), "2");
        assert_eq!(mine.bitrate.load(Ordering::Acquire), expected);
        assert_eq!(second.bitrate.load(Ordering::Acquire), expected);
        assert_eq!(theirs.bitrate.load(Ordering::Acquire), original);
    }
    for query in [
        "",
        "bitrate=0",
        "bitrate=-1",
        "bitrate=4294967296",
        "bitrate=no",
    ] {
        let root = document(request(&f, true, "GET", &format!("/bitrate?{query}")).await).await;
        assert_eq!(root.attributes["status_code"], "400");
        assert_eq!(field(&root, "bitrate"), "0");
        assert_eq!(mine.bitrate.load(Ordering::Acquire), 12345);
    }
    f.host.sessions.lock().unwrap().active.clear();
    assert_eq!(
        document(request(&f, true, "GET", "/bitrate?bitrate=1").await)
            .await
            .attributes["status_code"],
        "404"
    );
    f.host.paired.write().unwrap().clients[0].perm = 1 << 24;
    assert_eq!(
        document(request(&f, true, "GET", "/bitrate?bitrate=1").await)
            .await
            .attributes["status_code"],
        "403"
    );
}

#[tokio::test]
async fn abr_capabilities_are_client_driven_and_require_view_or_launch_permission() {
    let f = Fixture::new();
    f.client(0);
    for perm in [0, 1 << 24, 1 << 25, 1 << 26] {
        f.host.paired.write().unwrap().clients[0].perm = perm;
        let response = request(&f, true, "GET", "/api/abr/capabilities").await;
        if perm & VIEW == 0 {
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        } else {
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), 1024).await.unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
                json!({"supported":false,"version":1,"features":["runtime_bitrate","host_loss_adaptation"],"host_adaptive_bitrate":false,"client_bitrate_takes_ownership":true})
            );
        }
    }
}

#[tokio::test]
async fn clipboard_requires_direction_view_text_and_an_active_session_for_this_device() {
    let f = Fixture::new();
    let client = f.client((1 << 16) | (1 << 17) | (1 << 25));
    let args = Args::from([("type".into(), "text".into())]);
    let c = f.connection(true);
    assert!(clipboard_client(&f.host, &c, 1 << 17, &args).is_err());
    let mut other = client.clone();
    other.uuid = "other".into();
    f.session(other, Role::Stream);
    assert!(clipboard_client(&f.host, &c, 1 << 17, &args).is_err());
    let mine = f.session(client, Role::InputOnly);
    assert!(clipboard_client(&f.host, &c, 1 << 17, &args).is_ok());
    assert!(clipboard_client(&f.host, &c, 1 << 16, &args).is_ok());
    assert!(clipboard_client(&f.host, &c, 1 << 17, &Args::new()).is_err());
    assert!(
        clipboard_client(
            &f.host,
            &c,
            1 << 17,
            &Args::from([("type".into(), "image".into())])
        )
        .is_err()
    );
    let invalid = clipboard_write(
        State(f.host.clone()),
        Extension(c.clone()),
        Query(args.clone()),
        Request::new(Body::from(vec![0xff])),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    for perm in [1 << 17, 1 << 25] {
        f.host.paired.write().unwrap().clients[0].perm = perm;
        for method in ["GET", "POST"] {
            assert_eq!(
                request(&f, true, method, "/actions/clipboard?type=text")
                    .await
                    .status(),
                StatusCode::FORBIDDEN
            );
        }
    }
    f.host.paired.write().unwrap().clients[0].perm = (1 << 17) | (1 << 26);
    assert!(clipboard_client(&f.host, &c, 1 << 17, &args).is_ok());
    // With the image clipboard off, `type=png` is 403 as any other type is,
    // even for a device allowed both directions.
    f.host.paired.write().unwrap().clients[0].perm = (1 << 16) | (1 << 17) | (1 << 26);
    let png = Args::from([("type".into(), "png".into())]);
    assert_eq!(
        clipboard_client(&f.host, &c, 1 << 17, &png).ok(),
        Some(ClipboardType::Png)
    );
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("clipboard_image".into(), "false".into());
    assert!(clipboard_client(&f.host, &c, 1 << 17, &png).is_err());
    for method in ["GET", "POST"] {
        assert_eq!(
            request(&f, true, method, "/actions/clipboard?type=png")
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let canceled = f
        .host
        .sessions
        .lock()
        .unwrap()
        .request_stop(Some(&mine.launch.client.uuid));
    drop(canceled);
    assert!(clipboard_client(&f.host, &c, 1 << 17, &args).is_err());
}

async fn post_png(
    f: &Fixture,
    content_type: Option<&str>,
    length: Option<usize>,
    body: Vec<u8>,
) -> StatusCode {
    let mut request = Request::builder()
        .method("POST")
        .uri("/actions/clipboard?type=png");
    if let Some(content_type) = content_type {
        request = request.header(header::CONTENT_TYPE, content_type);
    }
    if let Some(length) = length {
        request = request.header(header::CONTENT_LENGTH, length);
    }
    router(f.host.clone(), true)
        .layer(Extension(f.connection(true)))
        .oneshot(request.body(Body::from(body)).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn clipboard_images_travel_as_png_within_the_limit() {
    use butterpollo_core::clipboard_image::{Image, decode_png, encode_png};
    const MIB: usize = 1024 * 1024;
    let f = Fixture::new();
    let client = f.client((1 << 16) | (1 << 17) | (1 << 25));
    let _session = f.session(client, Role::InputOnly);
    let set = |key: &str, value: &str| {
        f.host
            .config
            .write()
            .unwrap()
            .values
            .insert(key.into(), value.into());
    };
    *clipboard::PNG.lock().unwrap() = None;
    let get = || request(&f, true, "GET", "/actions/clipboard?type=png");
    assert_eq!(get().await.status(), StatusCode::NOT_FOUND);

    let image = Image {
        width: 2,
        height: 1,
        rgba: vec![255, 0, 0, 255, 0, 0, 255, 128],
    };
    let png = encode_png(&image).unwrap();
    assert_eq!(
        post_png(&f, None, Some(png.len()), png.clone()).await,
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    assert_eq!(
        post_png(&f, Some("image/png"), Some(png.len()), png.clone()).await,
        StatusCode::OK
    );
    let read = get().await;
    assert_eq!(read.status(), StatusCode::OK);
    assert_eq!(read.headers()[header::CONTENT_TYPE], "image/png");
    let body = to_bytes(read.into_body(), MIB).await.unwrap();
    assert_eq!(decode_png(&body).unwrap(), image);
    assert_eq!(
        post_png(&f, Some("Image/PNG; x=1"), None, b"GIF89a".to_vec()).await,
        StatusCode::BAD_REQUEST
    );
    // The 1 MiB limit of the other routes doesn't hold an image back...
    assert_eq!(
        post_png(&f, Some("image/png"), Some(2 * MIB), vec![0; 2 * MIB]).await,
        StatusCode::BAD_REQUEST
    );
    // ...but `clipboard_image_max_mb` does, by the Content-Length or, without
    // one, once the body passes it.
    set("clipboard_image_max_mb", "1");
    for length in [Some(MIB + 1), None] {
        assert_eq!(
            post_png(&f, Some("image/png"), length, vec![0; MIB + 1]).await,
            StatusCode::PAYLOAD_TOO_LARGE,
            "{length:?}"
        );
    }
    // A larger image on the PC's clipboard isn't sent either.
    *clipboard::PNG.lock().unwrap() = Some(vec![0; MIB + 1]);
    assert_eq!(get().await.status(), StatusCode::PAYLOAD_TOO_LARGE);
    *clipboard::PNG.lock().unwrap() = None;
}

#[test]
fn input_and_monitor_roles_resolve_without_starting_an_app_or_preparing_hardware() {
    let f = Fixture::new();
    let client = f.client(u32::MAX);
    let args = Args::new();
    let resolve = |control, owner, resume| {
        resolve_launch_app(&f.host, &args, &client, 0, Some(control), owner, resume)
    };
    assert!(resolve(Control::Input, remote::Owner::None, false).is_err());
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("enable_input_only_mode".into(), "true".into());
    for (control, role) in [
        (Control::Input, Role::InputOnly),
        (Control::Monitor, Role::RemoteMonitor),
    ] {
        let (resolved, app, _) = resolve(control, remote::Owner::None, false).unwrap();
        assert_eq!(resolved, role);
        assert!(app.is_none());
    }
    assert_eq!(
        resolve(Control::Resume, remote::Owner::Monitor, true)
            .unwrap()
            .0,
        Role::RemoteMonitor
    );
    assert!(resolve(Control::Resume, remote::Owner::None, true).is_err());
    let session = f.session(client.clone(), Role::InputOnly);
    assert!(remote_owner(&f.host, &client.uuid) == remote::Owner::Input);
    assert!(validate_launch_request(&f.host, &client, Some(Control::Monitor)).is_break());
    assert!(validate_launch_request(&f.host, &client, Some(Control::Input)).is_break());
    assert_eq!(session.launch.role, Role::InputOnly);
    assert!(f.host.current_app.lock().unwrap().is_none());
    assert!(session.launch.preparation.lock().unwrap().is_none());
}

#[test]
fn only_hdr_mode_zero_makes_a_ten_bit_format_sdr() {
    use butterpollo_core::{rtsp::Negotiated, stream_policy::infer_sdr_10bit};
    let f = Fixture::new();
    let client = f.client(0);
    let sdp = b"a=x-nv-vqos[0].bitStreamFormat:1\na=x-nv-video[0].dynamicRangeMode:1\n";
    // /launch and /resume build the session from the same query.
    for (query, sdr) in [(None, false), (Some("0"), true), (Some("1"), false)] {
        let args: Args = query
            .into_iter()
            .map(|v| ("hdrMode".to_string(), v.to_string()))
            .collect();
        let launch = build_launch_session(
            &f.connection(true),
            &args,
            client.clone(),
            1,
            Role::Stream,
            [0; 16],
            1,
        );
        let mut negotiated = Negotiated::from_sdp(sdp).unwrap();
        assert!(negotiated.hdr);
        infer_sdr_10bit(&mut negotiated, launch.ten_bit_sdr_requested());
        assert_eq!(
            (negotiated.hdr, negotiated.sdr_10bit),
            (!sdr, sdr),
            "hdrMode={query:?}"
        );
        assert!(negotiated.ten_bit());
    }
}
