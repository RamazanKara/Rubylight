use super::super::capabilities::{Availability, Feature, Reason};
use super::*;

/// A paired Rubylight reply, field for field as rust/host/src/nvhttp.rs writes it.
pub(crate) const RUBYLIGHT: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\">\
        <NetworkPathVersion>1</NetworkPathVersion><HardwareMetricsVersion>0</HardwareMetricsVersion>\
        <hostname>Gaming PC</hostname><appversion>7.1.431.-1</appversion><GfeVersion>3.23.0.74</GfeVersion>\
        <uniqueid>0123-ABCD</uniqueid><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort>\
        <PairStatus>1</PairStatus><currentgame>5</currentgame><currentgameuuid>uuid-5</currentgameuuid>\
        <state>SUNSHINE_SERVER_BUSY</state><LocalIP>192.168.1.20</LocalIP><mac>AA:BB:CC:DD:EE:FF</mac>\
        <MaxLumaPixelsHEVC>1869449984</MaxLumaPixelsHEVC><ServerCodecModeSupport>8457985</ServerCodecModeSupport>\
        <RustHostVersion>2.2.21</RustHostVersion><RustHostEncoderBitrateKbps>40000</RustHostEncoderBitrateKbps>\
        <RustHostInitialEncoderBitrateKbps>50000</RustHostInitialEncoderBitrateKbps>\
        <RustHostSessionCount></RustHostSessionCount><PyroWaveHostLinkMbps>2500</PyroWaveHostLinkMbps>\
        <PyroWaveBandwidthProbeBytes>33554432</PyroWaveBandwidthProbeBytes>\
        <PyroWaveUdpProbeVersion>1</PyroWaveUdpProbeVersion><PyroWaveUdpHandshakeVersion>1</PyroWaveUdpHandshakeVersion>\
        <PyroWaveUdpProbeBurstVersion>1</PyroWaveUdpProbeBurstVersion><PyroWaveWireBudgetVersion>1</PyroWaveWireBudgetVersion>\
        <PyroWaveCriticalFecPercentage>20</PyroWaveCriticalFecPercentage><PyroWaveMinParityShards>2</PyroWaveMinParityShards>\
        <Permission>119480064</Permission><FrameLimiterSupported>1</FrameLimiterSupported>\
        <FrameLimiterEnabled>1</FrameLimiterEnabled><VirtualDisplayFrameLimiterEnabled>0</VirtualDisplayFrameLimiterEnabled>\
        <FrameLimiterFpsLimitMilliHz>119880</FrameLimiterFpsLimitMilliHz><VirtualDisplayCapable>true</VirtualDisplayCapable>\
        <VirtualDisplayDriverReady>false</VirtualDisplayDriverReady><ControllerPreparation>1</ControllerPreparation>\
        <VirtualDisplayHDRCapable>true</VirtualDisplayHDRCapable><ServerCommand>Sleep</ServerCommand>\
        <ServerCommand></ServerCommand></root>";

#[test]
fn the_hosts_last_failure_is_read_when_it_says_one() {
    assert_eq!(ServerInfo::parse(RUBYLIGHT, 47989).unwrap().last_failure, None);
    let reply = RUBYLIGHT.replace(
        "<RustHostSessionCount>",
        "<RustHostLastFailure> HDR is on, but the display is not in HDR &amp; mode </RustHostLastFailure><RustHostSessionCount>",
    );
    assert_eq!(
        ServerInfo::parse(&reply, 47989).unwrap().last_failure.as_deref(),
        Some("HDR is on, but the display is not in HDR & mode")
    );
    let empty = RUBYLIGHT.replace("<RustHostSessionCount>", "<RustHostLastFailure></RustHostLastFailure><RustHostSessionCount>");
    assert_eq!(ServerInfo::parse(&empty, 47989).unwrap().last_failure, None);
}

#[test]
fn rubylight_reply() {
    let info = ServerInfo::parse(RUBYLIGHT, 47989).unwrap();
    assert_eq!(info.hostname, "Gaming PC");
    assert_eq!(info.unique_id, "0123-ABCD");
    assert_eq!(info.version_quad().unwrap(), [7, 1, 431, -1]);
    assert_eq!((info.https_port, info.external_port), (47984, 47989));
    assert!(info.paired && !info.session_idle);
    assert_eq!(
        (info.current_game, info.current_game_uuid.as_deref()),
        (5, Some("uuid-5"))
    );
    assert_eq!(info.mac_address.as_deref(), Some("AA:BB:CC:DD:EE:FF"));
    assert_eq!(info.server_codec_mode_support, 8_457_985);
    assert_eq!(info.permission, Some(119_480_064));
    assert_eq!(info.server_commands, ["Sleep", ""]);
    assert!(info.encoder_bitrate_supported);
    assert_eq!(
        (info.encoder_bitrate_kbps, info.initial_encoder_bitrate_kbps),
        (40_000, 50_000)
    );
    assert!(info.frame_limiter_enabled && !info.virtual_display_frame_limiter_enabled);
    assert_eq!(info.frame_limiter_fps_limit_millihz, 119_880);
    assert_eq!(info.pyrowave_host_link_mbps, 2500);
    assert_eq!(info.packed444_modes, None, "a host that does not say");
    let packed = RUBYLIGHT.replace(
        "<RustHostSessionCount>",
        "<Packed444Modes>3</Packed444Modes><RustHostSessionCount>",
    );
    assert_eq!(
        ServerInfo::parse(&packed, 47989).unwrap().packed444_modes,
        Some(3)
    );
    assert!(info.pyrowave_burst_probe);
    assert_eq!(
        (
            info.pyrowave_critical_fec_percentage,
            info.pyrowave_min_parity_shards
        ),
        (20, 2)
    );
    assert!(!info.nvidia_server() && info.supports_4k());

    let caps = &info.capabilities;
    assert_eq!(caps.family(), Family::Rubylight);
    assert_eq!(caps.host_version(), Some("2.2.21"));
    for feature in [
        Feature::Permissions,
        Feature::OtpPairing,
        Feature::Clipboard,
        Feature::ServerCommands,
        Feature::AppUuids,
        Feature::LaunchByUuid,
        Feature::FractionalRefresh,
        Feature::HostVrr,
        Feature::RuntimeBitrate,
        Feature::PyroWave,
        Feature::PyroWaveRecords,
        Feature::PyroWaveBandwidthProbe,
        Feature::NetworkPath,
        Feature::FrameLimiterStatus,
        Feature::ControllerPreparation,
        Feature::RemoteSessionTiles,
        Feature::HostTiming,
        Feature::WebUi,
    ] {
        assert!(caps.has(feature), "{feature:?}");
    }
    assert_eq!(
        caps.availability(Feature::VirtualDisplay),
        Availability::Unsupported(Some(Reason::VirtualDisplayDriver))
    );
    assert_eq!(
        caps.availability(Feature::HardwareMetrics),
        Availability::Unsupported(Some(Reason::HardwareMetricsOff))
    );
    assert_eq!(
        caps.availability(Feature::CursorShape),
        Availability::Unsupported(None)
    );
    assert_eq!(info.trusted_codec_mode_support(), 8_457_985);
}

/// The 3.0 hosts report "3.0.0-beta.N". Their version did not parse, so the host counted as an
/// old one without records and no stream asked for them (early decode saw nothing, silently).
#[test]
fn a_3_0_beta_host_has_records_and_the_features_of_its_version() {
    for version in ["3.0.0-beta.1", "3.0.0-alpha.3", "3.0.0", "3.1.0-rc.2"] {
        let reply = RUBYLIGHT.replace("<RustHostVersion>2.2.21", &format!("<RustHostVersion>{version}"));
        let caps = ServerInfo::parse(&reply, 47989).unwrap().capabilities;
        assert_eq!(caps.host_version(), Some(version));
        for feature in [Feature::PyroWave, Feature::PyroWaveRecords] {
            assert!(caps.has(feature), "{version}: {feature:?}");
        }
    }
    // A host older than records still does not get them.
    let old = RUBYLIGHT.replace("<RustHostVersion>2.2.21", "<RustHostVersion>1.9.0");
    assert!(!ServerInfo::parse(&old, 47989).unwrap().capabilities.has(Feature::PyroWaveRecords));
}

#[test]
fn native_api_version_decides_the_native_api() {
    let native = |reply: &str| ServerInfo::parse(reply, 47989).unwrap().capabilities.availability(Feature::NativeApi);
    let with = |version: &str| RUBYLIGHT.replace("<NetworkPathVersion>",
        &format!("<NativeApiVersion>{version}</NativeApiVersion><NetworkPathVersion>"));
    assert_eq!(native(&with("1")), Availability::Supported);
    // A newer host also serves v1.
    assert_eq!(native(&with("2")), Availability::Supported);
    // A Rubylight host before 3.0 has no field: update it. Never a silent classic fallback.
    assert_eq!(native(RUBYLIGHT), Availability::Unsupported(Some(Reason::UpdateHost)));
    // 0: plain HTTP or an unpaired certificate.
    assert_eq!(native(&with("0")), Availability::Unsupported(Some(Reason::Pair)));
    assert_eq!(native(&with("1").replace("<PairStatus>1", "<PairStatus>0")),
        Availability::Unsupported(Some(Reason::Pair)));
    let info = ServerInfo::parse(&with("1"), 47989).unwrap();
    assert_eq!(info.capabilities.evidence().unwrap().native_api_version, 1);
    assert_eq!(ServerInfo::parse(RUBYLIGHT, 47989).unwrap().capabilities.evidence().unwrap().native_api_version, -1);
}

/// The cases HostCapabilitiesTest reads too, so the two parsers gate Sleep PC alike.
#[test]
fn power_action_fields_decide_sleep() {
    let cases = include_str!("../../../tests/fixtures/power/sleep-serverinfo.txt");
    let mut count = 0;
    for line in cases.lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let (expected, reply) = line.split_once('|').unwrap();
        let expected = match expected {
            "supported" => Availability::Supported,
            "never" => Availability::Unsupported(None),
            "unknown" => Availability::Unknown,
            "pair" => Availability::Unsupported(Some(Reason::Pair)),
            "update_host" => Availability::Unsupported(Some(Reason::UpdateHost)),
            "sleep_permission" => Availability::Unsupported(Some(Reason::SleepPermission)),
            other => panic!("unknown availability {other}"),
        };
        let caps = ServerInfo::parse(reply, 47989).unwrap().capabilities;
        assert_eq!(caps.availability(Feature::Sleep), expected, "{reply}");
        assert_eq!(caps.supported().contains(&Feature::Sleep), expected.is_supported());
        count += 1;
    }
    assert_eq!(count, 14);
    let paired = ServerInfo::parse(&RUBYLIGHT.replace("<NetworkPathVersion>",
        "<PowerActionVersion>1</PowerActionVersion><SleepSupported>1</SleepSupported>\
             <SleepAllowed>0</SleepAllowed><NetworkPathVersion>"), 47989).unwrap();
    let evidence = paired.capabilities.evidence().unwrap();
    assert_eq!((evidence.power_action_version, evidence.sleep_supported, evidence.sleep_allowed), (1, true, false));
    // The host's Sleep server command is not the power API.
    assert_eq!(ServerInfo::parse(RUBYLIGHT, 47989).unwrap().capabilities.availability(Feature::Sleep),
        Availability::Unsupported(Some(Reason::UpdateHost)));
    assert!(!HostCapabilities::UNKNOWN.has(Feature::Sleep));
}

#[test]
fn hosts_without_the_field_get_the_classic_grid() {
    // Sunshine, GFE and Apollo-style 2.6 replies carry no NativeApiVersion: never supported,
    // and nothing a user could change on the host alters that.
    let sunshine = "<root status_code=\"200\"><hostname>S</hostname><appversion>7.1.431.-1</appversion>\
            <uniqueid>abc</uniqueid><HttpsPort>47984</HttpsPort><PairStatus>1</PairStatus><currentgame>0</currentgame>\
            <state>SUNSHINE_SERVER_FREE</state></root>";
    let gfe = "<root status_code=\"200\"><appversion>7.1.431.0</appversion><GfeVersion>2.1.0</GfeVersion>\
            <uniqueid>gfe</uniqueid><PairStatus>1</PairStatus><currentgame>0</currentgame>\
            <state>MJOLNIR_STATE_SERVER_AVAILABLE</state></root>";
    let apollo = sunshine.replace("<currentgame>", "<Permission>119480064</Permission><currentgame>");
    for reply in [sunshine.to_owned(), gfe.to_owned(), apollo] {
        let caps = ServerInfo::parse(&reply, 47989).unwrap().capabilities;
        assert_eq!(caps.evidence().unwrap().native_api_version, -1, "{reply}");
        assert_eq!(caps.availability(Feature::NativeApi), Availability::Unsupported(None), "{reply}");
        assert!(!caps.has(Feature::NativeApi) && !caps.supported().contains(&Feature::NativeApi));
    }
    // A host never reached rules nothing out, but doesn't offer it either.
    assert!(!HostCapabilities::UNKNOWN.has(Feature::NativeApi));
    // The 2.6.x Rubylight reply (what the host's own test fixture carries) is also classic.
    assert!(!ServerInfo::parse(RUBYLIGHT, 47989).unwrap().capabilities.has(Feature::NativeApi));
}

#[test]
fn sunshine_and_gamestream_replies() {
    let sunshine = "<root status_code=\"200\"><hostname></hostname><appversion>7.1.431.-1</appversion>\
            <uniqueid>abc</uniqueid><HttpsPort>0</HttpsPort><PairStatus>0</PairStatus><currentgame>0</currentgame>\
            <state>SUNSHINE_SERVER_FREE</state><ServerCodecModeSupport>25167617</ServerCodecModeSupport></root>";
    let info = ServerInfo::parse(sunshine, 47989).unwrap();
    assert_eq!(info.hostname, "UNKNOWN");
    assert_eq!((info.https_port, info.external_port), (47984, 47989));
    assert!(info.session_idle && !info.paired);
    assert_eq!(info.capabilities.family(), Family::Sunshine);
    assert!(!info.capabilities.has(Feature::FractionalRefresh));
    // PyroWave bits from a host that does not show PyroWave are not trusted.
    assert_eq!(info.trusted_codec_mode_support(), 25_167_617 & !0x0780_0000);

    let gfe = "<root status_code=\"200\"><appversion>7.1.431.0</appversion><GfeVersion>2.1.0</GfeVersion>\
            <uniqueid>gfe</uniqueid><PairStatus>1</PairStatus><currentgame>9</currentgame>\
            <state>MJOLNIR_STATE_SERVER_AVAILABLE</state></root>";
    let info = ServerInfo::parse(gfe, 47989).unwrap();
    assert!(info.nvidia_server() && !info.supports_4k());
    assert_eq!(
        info.current_game, 0,
        "only a busy host reports a running game"
    );
    assert!(!info.session_idle);
    assert_eq!(
        info.capabilities.availability(Feature::WebUi),
        Availability::Unsupported(None)
    );
}

#[test]
fn invalid_replies() {
    let ok = "<root status_code=\"200\"><uniqueid>x</uniqueid><PairStatus>1</PairStatus><state>S</state></root>";
    assert!(ServerInfo::parse(ok, 1).is_ok());
    for bad in [
        ok.replace("<uniqueid>x", "<uniqueid>a/b"),
        ok.replace("<state>S</state>", ""),
        ok.replace("<PairStatus>1</PairStatus>", ""),
        ok.replace("<state>S", "<state>X_SERVER_BUSY"),
    ] {
        assert!(ServerInfo::parse(&bad, 1).is_err(), "{bad}");
    }
    assert_eq!(parse_version_quad("7.1.431.-1").unwrap(), [7, 1, 431, -1]);
    assert!(parse_version_quad("7.1.431").is_err());
    assert!(parse_version_quad("7.-1.431.0").is_err());
}
