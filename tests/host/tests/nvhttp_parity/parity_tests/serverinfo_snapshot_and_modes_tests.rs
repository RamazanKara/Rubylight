//! NVHTTP default serverinfo snapshot, packed 4:4:4 modes, launch XML, and permission tests.
use super::*;

#[tokio::test]
async fn serverinfo_for_a_default_config_is_byte_identical() {
    let f = Fixture::new();
    let client = f.client(u32::MAX);
    let config = butterpollo_config::config::Config::default();
    let text = |response: Response| async move {
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    };
    let unpaired = text(serverinfo_response(
        &f.host,
        &f.connection(false),
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
    let paired = text(serverinfo_response(
        &f.host,
        &f.connection(true),
        &axum::http::HeaderMap::new(),
        &config,
        None,
        Some(&client),
        true,
        "01:02:03:04:05:06".into(),
        1000,
        true,
    ))
    .await;
    let hostname = butterpollo_stream::network::host_name(&config);
    let unique_id = f.host.paired.read().unwrap().unique_id.clone();
    let version = butterpollo_stream::stream::reconnect_harness::HOST_VERSION;
    let profile = butterpollo_session::migration::profile_id(&f.host.directory);
    let sleep = u8::from(f.host.power.supported());
    // AudioCodecs 3 is Opus and PCM: the test binary's folder has no FFmpeg for AC3 and E-AC3.
    assert_eq!(
        paired,
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\">\
             <NetworkPathVersion>1</NetworkPathVersion>\
             <HardwareMetricsVersion>1</HardwareMetricsVersion>\
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
             <RustHostLastFailure></RustHostLastFailure>\
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
             <RustHostInternetPlayVersion>0</RustHostInternetPlayVersion>\
             <Packed444Modes>0</Packed444Modes>\
             <HdrFormats>79</HdrFormats><MaxVideoPacket>8192</MaxVideoPacket>\
             <FecControlVersion>1</FecControlVersion>\
             <BandwidthProbeVersion>1</BandwidthProbeVersion>\
             <AudioCodecs>3</AudioCodecs>\
             <AudioLayouts>15</AudioLayouts>\
             <TextInputVersion>2</TextInputVersion>\
             <KeyboardLayoutVersion>1</KeyboardLayoutVersion>\
             <PenBarrelRollVersion>1</PenBarrelRollVersion>\
             <PipelineRatesVersion>1</PipelineRatesVersion>\
             <DisplayScaleVersion>1</DisplayScaleVersion>\
             <SecondScreenVersion>1</SecondScreenVersion>\
             <FrameWarpProtocol>2</FrameWarpProtocol>\
             <ControllerIdentityVersion>1</ControllerIdentityVersion>\
             <HdrDynamicControl>1</HdrDynamicControl>\
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
             <RustHostLastFailure></RustHostLastFailure>\
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
             <Packed444Modes>0</Packed444Modes>\
             <HdrFormats>79</HdrFormats><MaxVideoPacket>8192</MaxVideoPacket>\
             <FecControlVersion>1</FecControlVersion>\
             <BandwidthProbeVersion>1</BandwidthProbeVersion>\
             <AudioCodecs>3</AudioCodecs>\
             <AudioLayouts>15</AudioLayouts>\
             <TextInputVersion>2</TextInputVersion>\
             <KeyboardLayoutVersion>1</KeyboardLayoutVersion>\
             <PenBarrelRollVersion>1</PenBarrelRollVersion>\
             <PipelineRatesVersion>1</PipelineRatesVersion>\
             <DisplayScaleVersion>1</DisplayScaleVersion>\
             <SecondScreenVersion>1</SecondScreenVersion>\
             <FrameWarpProtocol>2</FrameWarpProtocol>\
             <ControllerIdentityVersion>1</ControllerIdentityVersion>\
             <HdrDynamicControl>1</HdrDynamicControl>\
             </root>"
        ),
        "{unpaired}"
    );
}

#[tokio::test]
async fn serverinfo_carries_the_packed_444_modes_only_when_packed_is_on() {
    use rubylight_protocol::packed444::mode_bit;
    let f = Fixture::new();
    let client = f.client(0x071f1f00);
    let packed = |config: &'static str| {
        let host = f.host.clone();
        let connection = f.connection(true);
        let client = client.clone();
        async move {
            let config = butterpollo_config::config::Config::parse(config).unwrap();
            let root = document(serverinfo_response(
                &host,
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
            root.get_child("Packed444Modes")
                .map(|element| element.get_text().unwrap_or_default().into_owned())
        }
    };
    // A client could ask for 10-bit packed, be refused at ANNOUNCE and lose
    // the session; it now sees the startup probe's result before /launch.
    f.host.packed_modes.store(0, Ordering::Release);
    assert_eq!(packed("packed_444=true").await.as_deref(), Some("0"));
    f.host
        .packed_modes
        .store(mode_bit(1, false) | mode_bit(2, true), Ordering::Release);
    assert_eq!(
        packed("experimental_packed_444=true").await.as_deref(),
        Some("9")
    );
    assert_eq!(packed("packed_444=false").await, None);
    // On by default: an unset value offers the modes.
    assert_eq!(packed("").await.as_deref(), Some("9"));
}

/// Full chroma took a host restart: only the startup probe found its modes.
/// A save now probes them once no stream runs, and turning a mode's codec or
/// full chroma off withdraws it at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saving_packed_444_changes_the_offered_modes_without_a_restart() {
    use rubylight_protocol::packed444::mode_bit;
    fn probe(
        _: &butterpollo_config::config::Config,
        stream: &butterpollo_media_policy::rtsp::Negotiated,
        _: &dyn Fn() -> bool,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(stream.codec == 1 || !stream.ten_bit(), "no 10-bit AV1 here");
        Ok(())
    }
    let f = Fixture::new();
    // Full chroma starts off here, so the save below turns it on.
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("packed_444".into(), "false".into());
    // The startup probe found HEVC and AV1.
    f.host.codecs.fetch_or(0x10000 | 0x20000, Ordering::AcqRel);
    let client = f.client(0x071f1f01);
    let offered = || {
        let host = f.host.clone();
        let connection = f.connection(true);
        let client = client.clone();
        async move {
            let config = host.config.read().unwrap().clone();
            let root = document(serverinfo_response(
                &host,
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
            root.get_child("Packed444Modes")
                .map(|element| element.get_text().unwrap_or_default().into_owned())
        }
    };
    let save = |key: &str, value: &str| {
        let before = f.host.config.read().unwrap().clone();
        f.host
            .config
            .write()
            .unwrap()
            .values
            .insert(key.into(), value.into());
        f.host.packed_settings_saved_with(&before, probe);
    };
    let probed = mode_bit(1, false) | mode_bit(1, true) | mode_bit(2, false);
    // A save during a stream leaves the encoder to it.
    f.host
        .sessions
        .lock()
        .unwrap()
        .teardown
        .insert("stream".into(), Instant::now());
    save("packed_444", "true");
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(offered().await.as_deref(), Some("0"));
    f.host.sessions.lock().unwrap().teardown.clear();
    let deadline = Instant::now() + Duration::from_secs(5);
    while f.host.packed_modes.load(Ordering::Acquire) != probed && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(offered().await, Some(probed.to_string()));
    // Turning HEVC off withdraws its modes without waiting for a probe.
    save("hevc_mode", "1");
    assert_eq!(offered().await, Some(mode_bit(2, false).to_string()));
    save("packed_444", "false");
    assert_eq!(f.host.packed_modes.load(Ordering::Acquire), 0);
    assert_eq!(offered().await, None);
}

#[tokio::test]
async fn successful_launch_and_resume_xml_include_driver_readiness_and_the_called_endpoint() {
    for (reply, resume) in [("gamesession", false), ("resume", true)] {
        for ready in [false, true] {
            let url = "rtspenc://[::1]:48010";
            let root = document(launch_response(
                Ok((reply.into(), url.into(), None)),
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
