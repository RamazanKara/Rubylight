//! The capabilities contract shared with Android. Every serverinfo fixture in
//! `app/src/test/resources/protocol` has an expected-features JSON in its `capabilities/` folder,
//! and both this test and the JUnit `HostCapabilitiesContractTest` assert it, so
//! `HostCapabilities.java` and `http/capabilities.rs` cannot drift apart unnoticed.
//!
//! A feature is `"supported"`, `"unknown"`, `"unsupported"` (nothing on the host changes that) or
//! `"unsupported:<reason>"`. A feature only one side has is an object
//! `{"only": "java" | "rust", "availability": ..., "comment": why}`.

use nightjar_core::http::{Availability, Family, Feature, Reason, ServerInfo};
use serde_json::Value;
use std::collections::BTreeMap;

const PROTOCOL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../app/src/test/resources/protocol"
);

macro_rules! contract {
    ($name:literal) => {
        (
            concat!($name, "-serverinfo.xml"),
            include_str!(concat!(
                "../../../app/src/test/resources/protocol/",
                $name,
                "-serverinfo.xml"
            )),
            include_str!(concat!(
                "../../../app/src/test/resources/protocol/capabilities/",
                $name,
                "-serverinfo.json"
            )),
        )
    };
}

const CONTRACTS: [(&str, &str, &str); 11] = [
    contract!("apollo"),
    contract!("apollo-http"),
    contract!("apollo-input-only"),
    contract!("butterpollo"),
    contract!("butterpollo-viewer"),
    contract!("gamestream"),
    contract!("rubylight"),
    contract!("rubylight-native"),
    contract!("sunshine"),
    contract!("vibepollo"),
    contract!("vibepollo-http"),
];

/// The contract name of each feature. The match has no wildcard, so a new Rust feature does not
/// compile until it is named here, and then fails every contract until the JSON lists it.
fn feature_name(feature: Feature) -> &'static str {
    match feature {
        Feature::Permissions => "permissions",
        Feature::OtpPairing => "otp_pairing",
        Feature::Clipboard => "clipboard",
        Feature::ServerCommands => "server_commands",
        Feature::AppUuids => "app_uuids",
        Feature::LaunchByUuid => "launch_by_uuid",
        Feature::VirtualDisplay => "virtual_display",
        Feature::VirtualDisplayHdr => "virtual_display_hdr",
        Feature::HostDisplayModes => "host_display_modes",
        Feature::FractionalRefresh => "fractional_refresh",
        Feature::HostVrr => "host_vrr",
        Feature::RuntimeBitrate => "runtime_bitrate",
        Feature::PyroWave => "pyrowave",
        Feature::PyroWaveRecords => "pyrowave_records",
        Feature::PyroWaveBandwidthProbe => "pyrowave_bandwidth_probe",
        Feature::HardwareMetrics => "hardware_metrics",
        Feature::NetworkPath => "network_path",
        Feature::FrameLimiterStatus => "frame_limiter_status",
        Feature::ControllerPreparation => "controller_preparation",
        Feature::RemoteSessionTiles => "remote_session_tiles",
        Feature::HostConfirmsReplacement => "host_confirms_replacement",
        Feature::HostTiming => "host_timing",
        Feature::WebUi => "web_ui",
        Feature::CursorShape => "cursor_shape",
        Feature::NativeApi => "native_api",
        Feature::Sleep => "sleep",
    }
}

fn reason_name(reason: Reason) -> &'static str {
    match reason {
        Reason::UpdateHost => "update_host",
        Reason::Pair => "pair",
        Reason::ViewPermission => "view_permission",
        Reason::ClipboardPermission => "clipboard_permission",
        Reason::CommandPermission => "command_permission",
        Reason::NoCommands => "no_commands",
        Reason::VirtualDisplayOff => "virtual_display_off",
        Reason::VirtualDisplayDriver => "virtual_display_driver",
        Reason::HardwareMetricsOff => "hardware_metrics_off",
        Reason::SleepPermission => "sleep_permission",
    }
}

fn family_name(family: Family) -> &'static str {
    match family {
        Family::GameStream => "gamestream",
        Family::Sunshine => "sunshine",
        Family::Apollo => "apollo",
        Family::Vibepollo => "vibepollo",
        Family::Rubylight => "rubylight",
        Family::Unknown => "unknown",
    }
}

fn availability_name(availability: Availability) -> String {
    match availability {
        Availability::Supported => "supported".into(),
        Availability::Unknown => "unknown".into(),
        Availability::Unsupported(None) => "unsupported".into(),
        Availability::Unsupported(Some(reason)) => format!("unsupported:{}", reason_name(reason)),
    }
}

/// The features this side must have, with their availability; Java-only ones are left out.
fn expected_for_rust(
    fixture: &str,
    features: &serde_json::Map<String, Value>,
) -> BTreeMap<String, String> {
    let mut expected = BTreeMap::new();
    for (name, value) in features {
        let availability = match value {
            Value::String(availability) => availability,
            Value::Object(entry) => {
                let only = entry["only"].as_str();
                assert!(
                    matches!(only, Some("java" | "rust")),
                    "{fixture}: {name} needs \"only\": \"java\" or \"rust\""
                );
                assert!(
                    entry
                        .get("comment")
                        .and_then(Value::as_str)
                        .is_some_and(|c| !c.is_empty()),
                    "{fixture}: platform-specific {name} needs a comment"
                );
                if only != Some("rust") {
                    continue;
                }
                entry["availability"]
                    .as_str()
                    .unwrap_or_else(|| panic!("{fixture}: {name} has no availability"))
            }
            _ => panic!("{fixture}: {name} is neither a string nor an object"),
        };
        expected.insert(name.clone(), availability.to_owned());
    }
    expected
}

#[test]
fn rust_capabilities_match_the_shared_contract() {
    let mut drift = Vec::new();
    for (fixture, xml, json) in CONTRACTS {
        let contract: Value =
            serde_json::from_str(json).unwrap_or_else(|e| panic!("{fixture}: {e}"));
        assert_eq!(
            contract["serverinfo"].as_str(),
            Some(fixture),
            "{fixture}: serverinfo field"
        );
        let caps = ServerInfo::parse(xml, 47989)
            .unwrap_or_else(|e| panic!("{fixture}: {e:?}"))
            .capabilities;
        if contract["family"].as_str() != Some(family_name(caps.family())) {
            drift.push(format!(
                "{fixture}: family {} != {}",
                family_name(caps.family()),
                contract["family"]
            ));
        }
        if contract["hostVersion"].as_str() != caps.host_version() {
            drift.push(format!(
                "{fixture}: hostVersion {:?} != {}",
                caps.host_version(),
                contract["hostVersion"]
            ));
        }
        let features = contract["features"]
            .as_object()
            .unwrap_or_else(|| panic!("{fixture}: no features object"));
        let mut expected = expected_for_rust(fixture, features);
        for feature in Feature::ALL {
            let name = feature_name(feature);
            let actual = availability_name(caps.availability(feature));
            match expected.remove(name) {
                None => drift.push(format!("{fixture}: {name} is not in the contract")),
                Some(want) if want != actual => {
                    drift.push(format!("{fixture}: {name} {actual} != {want}"))
                }
                Some(_) => {}
            }
        }
        for name in expected.keys() {
            drift.push(format!(
                "{fixture}: {name} is in the contract but not in Feature::ALL"
            ));
        }
    }
    assert!(
        drift.is_empty(),
        "capabilities drift:\n{}",
        drift.join("\n")
    );
}

/// Every serverinfo fixture has a contract, and every contract is replayed here.
#[test]
fn every_serverinfo_fixture_has_a_contract() {
    let names = |dir: &str, suffix: &str| -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("{dir}: {e}"))
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter_map(|name| name.strip_suffix(suffix).map(str::to_owned))
            .collect();
        names.sort();
        names
    };
    let fixtures = names(PROTOCOL, "-serverinfo.xml");
    let contracts = names(&format!("{PROTOCOL}/capabilities"), "-serverinfo.json");
    let mut listed: Vec<String> = CONTRACTS
        .iter()
        .map(|(fixture, _, _)| fixture.strip_suffix("-serverinfo.xml").unwrap().to_owned())
        .collect();
    listed.sort();
    assert_eq!(
        fixtures, contracts,
        "a serverinfo fixture without a contract, or the reverse"
    );
    assert_eq!(
        fixtures, listed,
        "CONTRACTS must list every serverinfo fixture"
    );
}
