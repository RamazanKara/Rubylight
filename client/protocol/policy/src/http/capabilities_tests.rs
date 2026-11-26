//! Unit tests for [`HostCapabilities`], including versioned JSON round-trips and native power/pairing features.

use super::*;

#[test]
fn rubylight_versions() {
    assert_eq!(
        parse_rubylight_version("2.2.21"),
        Some([2, 2, 21, u32::MAX])
    );
    assert_eq!(
        parse_rubylight_version("2.0.0-rc.19"),
        Some([2, 0, 0, 3_000_019])
    );
    assert_eq!(
        parse_rubylight_version("3.0.0-beta.1"),
        Some([3, 0, 0, 2_000_001])
    );
    assert_eq!(
        parse_rubylight_version("3.0.0-alpha.2+abc.1"),
        Some([3, 0, 0, 1_000_002])
    );
    // alpha < beta < rc < release, whatever the numbers.
    let ascending = [
        "3.0.0-alpha.1",
        "3.0.0-alpha.99",
        "3.0.0-beta.1",
        "3.0.0-beta.12",
        "3.0.0-rc.1",
        "3.0.0-rc.30",
        "3.0.0",
    ];
    for pair in ascending.windows(2) {
        assert!(
            parse_rubylight_version(pair[0]).unwrap() < parse_rubylight_version(pair[1]).unwrap(),
            "{pair:?}"
        );
    }
    assert_eq!(
        parse_rubylight_version("2.2.1+abc.1"),
        Some([2, 2, 1, u32::MAX])
    );
    for bad in [
        "",
        "2.2",
        "02.1.1",
        "2.2.1-rc.0",
        "2.2.1+",
        "2.2.1x",
        "3.0.0-beta",
        "3.0.0-beta.0",
        "3.0.0-gamma.1",
    ] {
        assert_eq!(parse_rubylight_version(bad), None, "{bad}");
    }
    assert!(parse_rubylight_version("2.2.1").unwrap() >= HOST_DISPLAY_MODES_VERSION);
    assert!(parse_rubylight_version("2.2.1-rc.3").unwrap() < HOST_DISPLAY_MODES_VERSION);
}

#[test]
fn unknown_hosts_rule_nothing_out() {
    let caps = HostCapabilities::UNKNOWN;
    assert_eq!(caps.family(), Family::Unknown);
    assert!(caps.may_have(Feature::RemoteSessionTiles));
    assert!(!caps.has(Feature::PyroWave));
    assert_eq!(caps.trusted_codec_mode_support(0x0180_0101), 0x0000_0101);
}

#[test]
fn file_transfer_follows_the_native_features_and_permissions() {
    let cases = include_str!("../../../client/tests/fixtures/files/files-capabilities.txt");
    let availability = |name: &str| match name {
        "supported" => Availability::Supported,
        "never" => Availability::Unsupported(None),
        "unknown" => Availability::Unknown,
        "pair" => Availability::Unsupported(Some(Reason::Pair)),
        "update_host" => Availability::Unsupported(Some(Reason::UpdateHost)),
        "file_permission" => Availability::Unsupported(Some(Reason::FilePermission)),
        other => panic!("unknown availability {other}"),
    };
    let (mut count, mut invalid) = (0, 0);
    for line in cases
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let fields: Vec<&str> = line.splitn(4, '|').collect();
        if fields[0] == "invalid" {
            let parsed =
                serde_json::from_str::<super::super::native_types::NativeCapabilities>(fields[2]);
            assert!(parsed.is_err(), "{line}");
            invalid += 1;
            continue;
        }
        let mut caps = super::super::serverinfo::ServerInfo::parse(fields[3], 47989)
            .unwrap()
            .capabilities;
        // Serverinfo alone never decides file transfer on a host with the native API.
        for feature in [Feature::FileUpload, Feature::FileDownload] {
            assert_ne!(
                caps.availability(feature),
                Availability::Supported,
                "{line}"
            );
        }
        if fields[2] != "-" {
            caps = caps.with_native(&serde_json::from_str(fields[2]).unwrap());
        }
        for (feature, expected) in [
            (Feature::FileUpload, fields[0]),
            (Feature::FileDownload, fields[1]),
        ] {
            let expected = availability(expected);
            assert_eq!(caps.availability(feature), expected, "{feature:?} {line}");
            assert_eq!(caps.supported().contains(&feature), expected.is_supported());
        }
        count += 1;
    }
    assert_eq!((count, invalid), (13, 15));
    // Without serverinfo there is nothing to add to.
    let native = serde_json::from_str(
        cases
            .lines()
            .find(|l| l.starts_with("supported|supported|"))
            .unwrap()
            .split('|')
            .nth(2)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        HostCapabilities::UNKNOWN.with_native(&native),
        HostCapabilities::UNKNOWN
    );
}

#[test]
fn jumbo_packets_are_offered_by_a_serverinfo_field_and_zero_means_off() {
    let caps = |field: &str| {
        let xml = format!(
            "<root status_code=\"200\"><uniqueid>uid</uniqueid><state>SUNSHINE_SERVER_FREE</state>\
             <PairStatus>1</PairStatus><RustHostVersion>3.0.0</RustHostVersion>{field}</root>"
        );
        super::super::serverinfo::ServerInfo::parse(&xml, 47989)
            .unwrap()
            .capabilities
    };
    use rubylight_protocol::video_packet::{MAX_VIDEO_PACKET, STOCK_MAX_VIDEO_PACKET};
    assert_eq!(
        caps("<MaxVideoPacket>8192</MaxVideoPacket>").max_video_packet(),
        MAX_VIDEO_PACKET
    );
    assert_eq!(
        caps("<MaxVideoPacket>8160</MaxVideoPacket>").max_video_packet(),
        8160
    );
    assert_eq!(
        caps("<MaxVideoPacket>1424</MaxVideoPacket>").max_video_packet(),
        1424
    );
    assert_eq!(
        caps("<MaxVideoPacket>1420</MaxVideoPacket>").max_video_packet(),
        1408
    );
    assert_eq!(
        caps("<MaxVideoPacket>9000</MaxVideoPacket>").max_video_packet(),
        MAX_VIDEO_PACKET
    );
    assert_eq!(
        caps("<MaxVideoPacket>8193</MaxVideoPacket>").max_video_packet(),
        MAX_VIDEO_PACKET
    );
    assert_eq!(
        caps(&format!(
            "<MaxVideoPacket>{STOCK_MAX_VIDEO_PACKET}</MaxVideoPacket>"
        ))
        .max_video_packet(),
        0
    );
    for field in [
        "<MaxVideoPacket>0</MaxVideoPacket>",
        "<MaxVideoPacket>-16</MaxVideoPacket>",
        "<MaxVideoPacket>x</MaxVideoPacket>",
        "<MaxVideoPacket>999999999999</MaxVideoPacket>",
        "",
    ] {
        assert_eq!(caps(field).max_video_packet(), 0, "{field}");
    }
    assert_eq!(HostCapabilities::UNKNOWN.max_video_packet(), 0);
}

#[test]
fn the_30_extensions_follow_their_serverinfo_bits() {
    let xml =
        include_str!("../../../../app/src/test/resources/protocol/rubylight-30-serverinfo.xml");
    let caps = |field: &str, value: &str| {
        let re = format!("<{field}>{value}</{field}>");
        let start = xml.find(&format!("<{field}>")).unwrap();
        let end = xml[start..].find(&format!("</{field}>")).unwrap() + start + field.len() + 3;
        let xml = format!("{}{re}{}", &xml[..start], &xml[end..]);
        super::super::serverinfo::ServerInfo::parse(&xml, 47989)
            .unwrap()
            .capabilities
    };
    let never = Availability::Unsupported(None);
    for (field, feature, cases) in [
        (
            "HdrFormats",
            Feature::DolbyVision,
            &[
                ("4", true),
                ("8", true),
                ("16", true),
                ("3", false),
                ("x", false),
            ][..],
        ),
        (
            "HdrFormats",
            Feature::Hlg,
            &[
                ("2", true),
                ("131", true),
                ("143", true),
                ("13", false),
                ("128", false),
                ("256", false),
            ],
        ),
        (
            "AudioCodecs",
            Feature::AudioPassthrough,
            &[("2", true), ("4", true), ("8", true), ("1", false)],
        ),
        (
            "AudioLayouts",
            Feature::Surround714,
            &[("8", true), ("7", false)],
        ),
        (
            "FecControlVersion",
            Feature::ClientFec,
            &[("2", true), ("0", false), ("-1", false)],
        ),
        (
            "PipelineRatesVersion",
            Feature::PipelineRates,
            &[("1", true), ("", false)],
        ),
        (
            "SecondScreenVersion",
            Feature::SecondScreen,
            &[("1", true), ("2", true), ("0", false), ("x", false)],
        ),
        (
            "RustHostInternetPlayVersion",
            Feature::InternetPlay,
            &[("1", true), ("0", false), ("", false)],
        ),
        (
            "BandwidthProbeVersion",
            Feature::BandwidthProbe,
            &[("1", true), ("0", false), ("x", false)],
        ),
        (
            "TextInputVersion",
            Feature::TextInput,
            &[("1", true), ("0", false), ("x", false)],
        ),
        (
            "KeyboardLayoutVersion",
            Feature::ClientKeyboardLayout,
            &[("1", true), ("0", false), ("x", false)],
        ),
        (
            "PenBarrelRollVersion",
            Feature::PenBarrelRoll,
            &[("1", true), ("0", false), ("x", false)],
        ),
        (
            "ControllerIdentityVersion",
            Feature::ControllerIdentity,
            &[("1", true), ("0", false), ("x", false)],
        ),
    ] {
        for &(value, supported) in cases {
            let expected = if supported {
                Availability::Supported
            } else {
                never
            };
            assert_eq!(
                caps(field, value).availability(feature),
                expected,
                "{field} {value}"
            );
        }
    }
    let caps = caps("HdrFormats", "15");
    for feature in [
        Feature::UsbForwarding,
        Feature::ImageClipboard,
        Feature::SharedFolders,
        Feature::Coop,
        Feature::NeuralFrameGeneration,
        Feature::MicOnly,
        Feature::ContinueHere,
        Feature::PcRestart,
        Feature::PcShutdown,
        Feature::HostUpdate,
        Feature::PairingApprove,
    ] {
        assert_eq!(
            caps.availability(feature),
            Availability::Unknown,
            "{feature:?}"
        );
    }
    assert!(caps.supported().contains(&Feature::TextInput));
}

#[test]
fn reading_the_native_capabilities_again_drops_what_the_host_stopped_offering() {
    let cases = include_str!("../../../client/tests/fixtures/files/files-capabilities.txt");
    let fields: Vec<&str> = cases
        .lines()
        .find(|l| l.starts_with("supported|supported|"))
        .unwrap()
        .splitn(4, '|')
        .collect();
    let caps = super::super::serverinfo::ServerInfo::parse(fields[3], 47989)
        .unwrap()
        .capabilities;
    let offered = caps.with_native(&serde_json::from_str(fields[2]).unwrap());
    assert!(offered.has(Feature::FileUpload) && offered.has(Feature::FileDownload));
    let withdrawn = fields[2]
        .replace(r#","files.upload","files.download""#, "")
        .replace(r#","file_upload":true,"file_download":true"#, "");
    let withdrawn = offered.with_native(&serde_json::from_str(&withdrawn).unwrap());
    for feature in [Feature::FileUpload, Feature::FileDownload] {
        assert_eq!(
            withdrawn.availability(feature),
            Availability::Unsupported(None)
        );
    }
}

#[test]
fn versioned_json_round_trip_and_validation() {
    let xml =
        include_str!("../../../../app/src/test/resources/protocol/rubylight-30-serverinfo.xml");
    let caps = HostCapabilities::from_server_info_xml(xml).unwrap();
    let json = caps.to_versioned_json().unwrap();
    let restored = HostCapabilities::from_versioned_json(&json);
    assert_eq!(caps, restored);

    assert_eq!(HostCapabilities::UNKNOWN.to_versioned_json(), None);
    for damaged in [
        "",
        "{",
        "[]",
        "{\"v\":2}",
        "{\"v\":1}",
        &json.replace("\"v\":1", "\"v\":99"),
        &json.replace("117506048", "4294967296"),
        &json.replace("\"3.1.0\"", "\"\""),
    ] {
        assert_eq!(
            HostCapabilities::from_versioned_json(damaged),
            HostCapabilities::UNKNOWN,
            "{damaged}"
        );
    }
}

#[test]
fn power_update_and_pairing_approve_follow_native_permissions() {
    let xml =
        include_str!("../../../../app/src/test/resources/protocol/rubylight-30-serverinfo.xml");
    let base = HostCapabilities::from_server_info_xml(xml).unwrap();
    let native_json = r#"{"api_version":1,"host":{"name":"RL","version":"3.1.0","unique_id":"rl"},"features":["power.restart","power.shutdown","update","pairing.approve"],"permissions":{"list":true,"view":true,"launch":true,"host_control":false,"restart_shutdown":false,"pairing_approve":false}}"#;
    let caps = base.with_native(&serde_json::from_str(native_json).unwrap());
    assert_eq!(
        caps.availability(Feature::PcRestart),
        Availability::Unsupported(Some(Reason::RestartShutdownPermission))
    );
    assert_eq!(
        caps.availability(Feature::PcShutdown),
        Availability::Unsupported(Some(Reason::RestartShutdownPermission))
    );
    assert_eq!(
        caps.availability(Feature::HostUpdate),
        Availability::Unsupported(None)
    );
    assert_eq!(
        caps.availability(Feature::PairingApprove),
        Availability::Unsupported(Some(Reason::PairingApprovePermission))
    );

    let granted = native_json.replace("\"host_control\":false", "\"host_control\":true");
    let caps = base.with_native(&serde_json::from_str(&granted).unwrap());
    assert!(caps.has(Feature::HostUpdate));
    assert!(caps.has(Feature::PairingApprove));
    assert_eq!(
        caps.availability(Feature::PcRestart),
        Availability::Unsupported(Some(Reason::RestartShutdownPermission))
    );

    let restart_granted =
        granted.replace("\"restart_shutdown\":false", "\"restart_shutdown\":true");
    let caps = base.with_native(&serde_json::from_str(&restart_granted).unwrap());
    assert!(caps.has(Feature::PcRestart));
    assert!(caps.has(Feature::PcShutdown));
    assert_eq!(
        HostCapabilities::from_versioned_json(&caps.to_versioned_json().unwrap()),
        caps
    );
}

#[test]
fn frame_warp_2_needs_frame_warp_version_2() {
    let xml =
        include_str!("../../../../app/src/test/resources/protocol/rubylight-30-serverinfo.xml");
    let caps = |extra: &str| {
        let xml = xml.replacen("</root>", &format!("{extra}</root>"), 1);
        super::super::serverinfo::ServerInfo::parse(&xml, 47989)
            .unwrap()
            .capabilities
    };
    for (field, v1, v2) in [
        ("", false, false),
        ("<FrameWarpVersion>1</FrameWarpVersion>", true, false),
        ("<FrameWarpVersion>2</FrameWarpVersion>", true, true),
        ("<FrameWarpVersion>3</FrameWarpVersion>", true, true),
        ("<FrameWarpVersion>bad</FrameWarpVersion>", false, false),
    ] {
        let caps = caps(field);
        assert_eq!(caps.has(Feature::FrameWarp), v1, "{field}");
        assert_eq!(caps.has(Feature::FrameWarpV2), v2, "{field}");
    }
}
