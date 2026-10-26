//! The `wake` and `audio` examples of `docs/native-api.md`, one fixture each.
//! Nightjar's tests read the same files. Kept apart from `fixtures.rs` so the
//! two sets of endpoints change without touching each other.
use rubylight_native_api::audio::{self, Audio, AudioWrite};
use rubylight_native_api::feature;
use rubylight_native_api::radeon::{Change, ControlState, Kind, Value};
use rubylight_native_api::wake::{
    self, AdapterKind, CheckState, FastStartup, IdleSleepState, ShutdownWake, Sleep, TargetState,
    Wake,
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value as Json};

fn fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

/// Parses the fixture, writes it back and compares the JSON values, so a
/// field the types drop, rename or add fails here.
fn round_trip<T: DeserializeOwned + Serialize + PartialEq + std::fmt::Debug>(name: &str) -> T {
    let text = fixture(name);
    assert!(!text.contains('\r'), "{name}.json must use LF");
    let expected: Json = serde_json::from_str(&text).unwrap();
    let typed: T = serde_json::from_str(&text).unwrap_or_else(|error| panic!("{name}: {error}"));
    assert_eq!(serde_json::to_value(&typed).unwrap(), expected, "{name}");
    let again: T = serde_json::from_value(serde_json::to_value(&typed).unwrap()).unwrap();
    assert_eq!(again, typed, "{name}");
    typed
}

#[test]
fn feature_names_match_the_spec() {
    assert_eq!(feature::WAKE, "wake");
    assert_eq!(feature::AUDIO_NIGHT_MODE, "audio.night_mode");
    assert_eq!(
        feature::AUDIO_HEADPHONE_SURROUND,
        "audio.headphone_surround"
    );
    assert_eq!(audio::NIGHT_MODE, feature::AUDIO_NIGHT_MODE);
    assert_eq!(audio::HEADPHONE_SURROUND, feature::AUDIO_HEADPHONE_SURROUND);
    assert_eq!(audio::GROUP, "sound");
    for (reason, name) in [
        (audio::reason::NO_SESSION, "no_session"),
        (audio::reason::CLIENT_SURROUND, "client_surround"),
        (audio::reason::GAME_ONLY_AUDIO, "game_only_audio"),
        (audio::reason::STEREO_ENDPOINT, "stereo_endpoint"),
        (audio::reason::MUTED, "muted"),
        (audio::reason::NEXT_STREAM, "next_stream"),
    ] {
        assert_eq!(reason, name);
    }
}

#[test]
fn wake_round_trip() {
    let wake: Wake = round_trip("wake");
    assert_eq!(wake.primary.as_deref(), Some("02:7c:16:b4:13:4f"));
    assert_eq!(wake.targets.len(), 2);
    let wired = &wake.targets[0];
    assert_eq!(wired.kind, AdapterKind::Ethernet);
    assert!(wired.connected && wired.route);
    assert_eq!(wired.broadcasts, ["192.168.4.255"]);
    assert_eq!(wired.state, TargetState::Ready);
    assert_eq!(wired.checks[0].id, wake::check::WAKE_ENABLED);
    assert_eq!(wired.checks[1].id, wake::check::MAGIC_PACKET);
    assert_eq!(wired.checks[2].id, wake::check::MAGIC_PACKET_ONLY);
    assert_eq!(wired.checks[2].state, CheckState::Warning);
    let wifi = &wake.targets[1];
    assert_eq!(wifi.kind, AdapterKind::Wifi);
    assert_eq!(wifi.state, TargetState::Experimental);
    assert_eq!(wifi.checks[0].state, CheckState::Error);
    assert_eq!(wake.system.sleep, Sleep::S3);
    assert!(!wake.system.modern_standby && !wake.system.hibernate);
    assert_eq!(wake.system.fast_startup, FastStartup::Inactive);
    assert_eq!(wake.system.shutdown_wake, ShutdownWake::FirmwareUnknown);
    assert_eq!(wake.idle_sleep.state, IdleSleepState::Off);
    assert_eq!(wake.idle_sleep.minutes, 0);
    assert_eq!(wake.idle_sleep.reason, None);
}

#[test]
fn wake_keeps_its_nulls_on_purpose() {
    let mut wake: Wake = round_trip("wake");
    wake.primary = None;
    wake.targets.clear();
    let value = serde_json::to_value(&wake).unwrap();
    assert_eq!(value["primary"], Json::Null);
    assert_eq!(value["idle_sleep"]["reason"], Json::Null);
    assert_eq!(value["targets"], json!([]));
}

#[test]
fn wake_states_and_kinds_serialise_in_snake_case() {
    for (state, name) in [
        (TargetState::Ready, "ready"),
        (TargetState::NeedsSetup, "needs_setup"),
        (TargetState::Experimental, "experimental"),
        (TargetState::Unknown, "unknown"),
    ] {
        assert_eq!(serde_json::to_value(state).unwrap(), name);
        assert_eq!(
            serde_json::from_value::<TargetState>(json!(name)).unwrap(),
            state
        );
    }
    for (state, name) in [
        (CheckState::Ok, "ok"),
        (CheckState::Warning, "warning"),
        (CheckState::Error, "error"),
        (CheckState::Unknown, "unknown"),
    ] {
        assert_eq!(serde_json::to_value(state).unwrap(), name);
        assert_eq!(
            serde_json::from_value::<CheckState>(json!(name)).unwrap(),
            state
        );
    }
    for (state, name) in [
        (IdleSleepState::Off, "off"),
        (IdleSleepState::Armed, "armed"),
        (IdleSleepState::Blocked, "blocked"),
        (IdleSleepState::Unsupported, "unsupported"),
    ] {
        assert_eq!(serde_json::to_value(state).unwrap(), name);
        assert_eq!(
            serde_json::from_value::<IdleSleepState>(json!(name)).unwrap(),
            state
        );
    }
    assert_eq!(serde_json::to_value(Sleep::None).unwrap(), "none");
    assert_eq!(serde_json::to_value(FastStartup::Active).unwrap(), "active");
    assert_eq!(serde_json::to_value(FastStartup::Off).unwrap(), "off");
}

#[test]
fn unknown_wake_fields_and_values_do_not_fail_the_response() {
    let mut value: Json = serde_json::from_str(&fixture("wake")).unwrap();
    value["relay"] = json!({ "host": "nas" });
    value["targets"][0]["kind"] = json!("thunderbolt");
    value["targets"][0]["state"] = json!("asleep");
    value["targets"][0]["speed_mbps"] = json!(2500);
    value["targets"][0]["checks"][0]["state"] = json!("degraded");
    value["targets"][0]["checks"][0]["hint"] = json!("x");
    value["targets"][0]["checks"][1]["id"] = json!("wake_on_link");
    value["system"]["sleep"] = json!("s0ix");
    value["system"]["fast_startup"] = json!("pending");
    value["system"]["shutdown_wake"] = json!("ready");
    value["system"]["s4"] = json!(true);
    value["idle_sleep"]["state"] = json!("sleeping");
    value["idle_sleep"]["sleep_in_s"] = json!(120);
    let wake: Wake = serde_json::from_value(value).unwrap();
    let target = &wake.targets[0];
    assert_eq!(target.kind, AdapterKind::Unknown);
    assert_eq!(target.state, TargetState::Unknown);
    assert_eq!(target.checks[0].state, CheckState::Unknown);
    assert_eq!(target.checks[1].id, "wake_on_link");
    assert_eq!(wake.system.sleep, Sleep::Unknown);
    assert_eq!(wake.system.fast_startup, FastStartup::Unknown);
    assert_eq!(wake.system.shutdown_wake, ShutdownWake::Unknown);
    assert_eq!(wake.idle_sleep.state, IdleSleepState::Unknown);
    // The target next to it still reads as before.
    assert_eq!(wake.targets[1].kind, AdapterKind::Wifi);
}

#[test]
fn a_target_without_broadcasts_or_checks_reads_as_empty() {
    let target: wake::Target = serde_json::from_value(json!({
        "mac": "02:00:00:00:00:01", "kind": "ethernet", "connected": true, "route": false,
        "state": "unknown"
    }))
    .unwrap();
    assert!(target.broadcasts.is_empty() && target.checks.is_empty());
    assert_eq!(target.state, TargetState::Unknown);
}

#[test]
fn audio_round_trip() {
    let audio: Audio = round_trip("audio");
    assert_eq!(audio.revision, 3);
    let [night, surround] = &audio.settings[..] else {
        panic!("{audio:?}");
    };
    assert_eq!(night.id, audio::NIGHT_MODE);
    assert_eq!(night.group, audio::GROUP);
    assert_eq!(night.kind, Kind::Toggle);
    assert_eq!(night.state, ControlState::Configurable);
    assert_eq!(night.value, Some(Value::Bool(true)));
    assert_eq!(surround.id, audio::HEADPHONE_SURROUND);
    assert_eq!(surround.state, ControlState::TemporarilyUnavailable);
    assert_eq!(surround.value, Some(Value::Bool(true)));
    assert_eq!(surround.reason.as_deref(), Some(audio::reason::NEXT_STREAM));
}

#[test]
fn unknown_audio_fields_and_values_do_not_fail_the_response() {
    let mut value: Json = serde_json::from_str(&fixture("audio")).unwrap();
    value["device"] = json!("Headphones");
    value["settings"][0]["state"] = json!("beta");
    value["settings"][0]["value"] = json!(0.5);
    value["settings"][1]["reason"] = json!("bluetooth_endpoint");
    value["settings"].as_array_mut().unwrap().push(
        json!({ "id": "audio.eq", "group": "sound", "kind": "slider", "state": "configurable" }),
    );
    let audio: Audio = serde_json::from_value(value).unwrap();
    assert_eq!(audio.settings[0].state, ControlState::Unknown);
    assert_eq!(audio.settings[0].value, None);
    assert_eq!(
        audio.settings[1].reason.as_deref(),
        Some("bluetooth_endpoint")
    );
    assert_eq!(audio.settings[2].kind, Kind::Unknown);
}

#[test]
fn audio_write_round_trip() {
    let write: AudioWrite = round_trip("audio_write");
    assert_eq!(
        write.changes,
        vec![
            Change {
                id: audio::NIGHT_MODE.into(),
                value: Value::Bool(false)
            },
            Change {
                id: audio::HEADPHONE_SURROUND.into(),
                value: Value::Bool(true)
            },
        ]
    );
}

#[test]
fn audio_writes_stay_strict() {
    let mut value: Json = serde_json::from_str(&fixture("audio_write")).unwrap();
    value["gpu"] = json!(1);
    let error = serde_json::from_value::<AudioWrite>(value).unwrap_err();
    assert!(error.to_string().contains("gpu"), "{error}");
    for body in [
        json!({}),
        json!({ "changes": [{ "id": "audio.night_mode" }] }),
        json!({ "changes": [{ "id": "audio.night_mode", "value": null }] }),
        json!({ "changes": [{ "id": "audio.night_mode", "value": 0.5 }] }),
        json!({ "changes": [{ "id": "audio.night_mode", "value": true, "scope": "host" }] }),
    ] {
        assert!(
            serde_json::from_value::<AudioWrite>(body.clone()).is_err(),
            "{body}"
        );
    }
}
