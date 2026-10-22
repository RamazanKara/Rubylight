//! The JSON examples of `docs/native-api.md`, one fixture each. Nightjar's
//! tests read the same files.
use rubylight_native_api::capabilities::Capabilities;
use rubylight_native_api::error::{ErrorBody, ErrorCode};
use rubylight_native_api::library::Library;
use rubylight_native_api::radeon::{Change, ControlState, Kind, Radeon, SettingsWrite, Value};
use rubylight_native_api::session::{ActionId, Owner, Session, State};
use rubylight_native_api::{feature, API_VERSION, PREFIX, SERVERINFO_FIELD};
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
fn constants_match_the_spec() {
    assert_eq!(API_VERSION, 1);
    assert_eq!(PREFIX, "/api/native/v1");
    assert_eq!(SERVERINFO_FIELD, "NativeApiVersion");
}

#[test]
fn capabilities_round_trip() {
    let capabilities: Capabilities = round_trip("capabilities");
    assert_eq!(capabilities.api_version, API_VERSION);
    assert!(capabilities.features.iter().any(|f| f == feature::LIBRARY));
    assert!(capabilities
        .features
        .iter()
        .any(|f| f == feature::CLOSE_GAME));
    assert!(capabilities.permissions.launch && !capabilities.permissions.host_control);
}

#[test]
fn library_round_trip_keeps_nulls_on_purpose() {
    let library: Library = round_trip("library");
    assert_eq!(library.games.len(), 2);
    let manual = &library.games[1];
    assert!(manual.game_id.is_none() && manual.store.is_none() && manual.installed.is_none());
    assert!(manual.running);
    let text = serde_json::to_string(&library).unwrap();
    assert!(text.contains("\"game_id\":null"));
    assert!(text.contains("\"store\":null"));
    assert!(text.contains("\"installed\":null"));
}

#[test]
fn session_round_trip() {
    let session: Session = round_trip("session");
    assert_eq!(session.state, State::Suspended);
    assert_eq!(session.owner, Owner::You);
    assert_eq!(session.actions.len(), 4);
    assert_eq!(session.actions[1].id, ActionId::Disconnect);
    assert_eq!(session.actions[1].reason, Some(ErrorCode::NotStreaming));
    assert_eq!(
        session.actions[3].reason,
        Some(ErrorCode::HostControlRequired)
    );
}

#[test]
fn session_without_details_serialises_null_app_and_no_times() {
    let session = Session {
        revision: 3,
        state: State::Streaming,
        owner: Owner::OtherDevice,
        app: None,
        game_running: true,
        since_unix_ms: None,
        suspended_until_unix_ms: None,
        actions: vec![],
    };
    let value = serde_json::to_value(&session).unwrap();
    assert_eq!(value["app"], Json::Null);
    assert!(value.get("since_unix_ms").is_none());
    assert!(value.get("suspended_until_unix_ms").is_none());
    assert_eq!(value["owner"], "other_device");
}

#[test]
fn radeon_round_trip() {
    let radeon: Radeon = round_trip("radeon");
    let settings = &radeon.gpus[0].settings;
    assert_eq!(settings.len(), 5);
    assert_eq!(settings[0].value, Some(Value::Bool(true)));
    assert_eq!(settings[1].value, Some(Value::Integer(144)));
    assert_eq!(settings[1].kind, Kind::Integer);
    let restore = settings[1].restore.as_ref().unwrap();
    assert_eq!(restore.original, Value::Integer(120));
    assert_eq!(settings[2].state, ControlState::NeedsExternalSetup);
    assert_eq!(settings[3].state, ControlState::Unsupported);
    assert!(settings[3].value.is_none());
    assert_eq!(settings[4].state, ControlState::TemporarilyUnavailable);
}

#[test]
fn settings_write_round_trip() {
    let write: SettingsWrite = round_trip("settings_write");
    assert_eq!(write.gpu, 11520);
    assert_eq!(
        write.changes,
        vec![
            Change {
                id: "chill.enabled".into(),
                value: Value::Bool(true)
            },
            Change {
                id: "chill.max_fps".into(),
                value: Value::Integer(144)
            },
        ]
    );
}

#[test]
fn error_round_trip_and_status_table() {
    let body: ErrorBody = round_trip("error");
    assert_eq!(body.error.code, ErrorCode::HostControlRequired);
    let table = [
        (ErrorCode::NotPaired, 403),
        (ErrorCode::PermissionRequired, 403),
        (ErrorCode::HostControlRequired, 403),
        (ErrorCode::NotFound, 404),
        (ErrorCode::InvalidRequest, 400),
        (ErrorCode::InvalidValue, 422),
        (ErrorCode::NotWritable, 422),
        (ErrorCode::NoSession, 409),
        (ErrorCode::NotSessionOwner, 409),
        (ErrorCode::SessionBusy, 409),
        (ErrorCode::DriverRejected, 409),
        (ErrorCode::TemporarilyUnavailable, 503),
        (ErrorCode::Internal, 500),
    ];
    for (code, status) in table {
        assert_eq!(code.status(), status, "{code:?}");
    }
}

#[test]
fn every_error_code_has_its_snake_case_name() {
    for (code, name) in [
        (ErrorCode::NotPaired, "not_paired"),
        (ErrorCode::PermissionRequired, "permission_required"),
        (ErrorCode::HostControlRequired, "host_control_required"),
        (ErrorCode::NotFound, "not_found"),
        (ErrorCode::InvalidRequest, "invalid_request"),
        (ErrorCode::InvalidValue, "invalid_value"),
        (ErrorCode::NotWritable, "not_writable"),
        (ErrorCode::NoSession, "no_session"),
        (ErrorCode::NotSessionOwner, "not_session_owner"),
        (ErrorCode::SessionBusy, "session_busy"),
        (ErrorCode::DriverRejected, "driver_rejected"),
        (ErrorCode::TemporarilyUnavailable, "temporarily_unavailable"),
        (ErrorCode::Internal, "internal"),
        (ErrorCode::NotStreaming, "not_streaming"),
    ] {
        assert_eq!(serde_json::to_value(code).unwrap(), name);
        assert_eq!(
            serde_json::from_value::<ErrorCode>(json!(name)).unwrap(),
            code
        );
    }
}

#[test]
fn unknown_enum_values_decode_to_unknown() {
    let code: ErrorCode = serde_json::from_value(json!("quota_exceeded")).unwrap();
    assert_eq!(code, ErrorCode::Unknown);
    assert_eq!(
        serde_json::from_value::<State>(json!("hibernating")).unwrap(),
        State::Unknown
    );
    assert_eq!(
        serde_json::from_value::<Owner>(json!("team")).unwrap(),
        Owner::Unknown
    );
    assert_eq!(
        serde_json::from_value::<ActionId>(json!("reboot")).unwrap(),
        ActionId::Unknown
    );
    assert_eq!(
        serde_json::from_value::<ControlState>(json!("beta")).unwrap(),
        ControlState::Unknown
    );
    assert_eq!(
        serde_json::from_value::<Kind>(json!("slider")).unwrap(),
        Kind::Unknown
    );
}

#[test]
fn unknown_values_inside_a_response_do_not_fail_it() {
    let mut value: Json = serde_json::from_str(&fixture("session")).unwrap();
    value["state"] = json!("hibernating");
    value["actions"][0]["id"] = json!("reboot");
    value["actions"][1]["reason"] = json!("quota_exceeded");
    let session: Session = serde_json::from_value(value).unwrap();
    assert_eq!(session.state, State::Unknown);
    assert_eq!(session.actions[0].id, ActionId::Unknown);
    assert_eq!(session.actions[1].reason, Some(ErrorCode::Unknown));
}

#[test]
fn responses_ignore_unknown_fields() {
    let mut value: Json = serde_json::from_str(&fixture("library")).unwrap();
    value["next_page"] = json!("later");
    value["games"][0]["artwork"]["hero"] = json!("/appasset?appid=1&AssetType=3");
    let library: Library = serde_json::from_value(value).unwrap();
    assert_eq!(library.games.len(), 2);

    let mut value: Json = serde_json::from_str(&fixture("radeon")).unwrap();
    value["gpus"][0]["settings"][0]["tooltip"] = json!("new");
    serde_json::from_value::<Radeon>(value).unwrap();
}

#[test]
fn unknown_value_shapes_do_not_fail_a_radeon_response() {
    let mut value: Json = serde_json::from_str(&fixture("radeon")).unwrap();
    let settings = &mut value["gpus"][0]["settings"];
    settings[0]["value"] = json!(0.75);
    settings[1]["value"] = json!([144, 60]);
    settings[1]["range"] = json!({ "min": 0.5, "max": 1.5, "step": 0.25 });
    settings[1]["restore"] = json!({ "original": [120], "written": { "fps": 144 } });
    let radeon: Radeon = serde_json::from_value(value).unwrap();
    let settings = &radeon.gpus[0].settings;
    assert_eq!(settings.len(), 5);
    assert_eq!(settings[0].value, None);
    assert_eq!(settings[0].state, ControlState::Configurable);
    assert_eq!(settings[1].value, None);
    assert_eq!(settings[1].range, None);
    assert_eq!(settings[1].restore, None);
    // The settings next to it still read as before.
    assert_eq!(settings[2].value, Some(Value::Bool(false)));
    // A null stays absent too.
    let setting: rubylight_native_api::radeon::Setting = serde_json::from_value(json!({
        "id": "x.y", "group": "x", "kind": "toggle", "state": "configurable", "value": null
    }))
    .unwrap();
    assert_eq!(setting.value, None);
}

#[test]
fn requests_stay_strict_about_value_shapes() {
    for value in [json!(1.5), json!([1]), json!({ "a": 1 }), json!(null)] {
        let body = json!({ "gpu": 1, "changes": [{ "id": "x.y", "value": value }] });
        assert!(
            serde_json::from_value::<SettingsWrite>(body).is_err(),
            "{value}"
        );
    }
}

#[test]
fn requests_reject_unknown_fields() {
    let mut value: Json = serde_json::from_str(&fixture("settings_write")).unwrap();
    value["dry_run"] = json!(true);
    let error = serde_json::from_value::<SettingsWrite>(value).unwrap_err();
    assert!(error.to_string().contains("dry_run"), "{error}");

    let mut value: Json = serde_json::from_str(&fixture("settings_write")).unwrap();
    value["changes"][0]["unit"] = json!("fps");
    let error = serde_json::from_value::<SettingsWrite>(value).unwrap_err();
    assert!(error.to_string().contains("unit"), "{error}");
}

#[test]
fn requests_reject_malformed_bodies() {
    for body in [
        json!({}),
        json!({"gpu": "11520", "changes": []}),
        json!({"gpu": 1, "changes": [{"id": "x"}]}),
        json!({"gpu": 1, "changes": [{"id": "x", "value": 1.5}]}),
        json!({"gpu": 1, "changes": [{"id": "x", "value": null}]}),
    ] {
        assert!(
            serde_json::from_value::<SettingsWrite>(body.clone()).is_err(),
            "{body}"
        );
    }
}

#[test]
fn setting_values_pick_their_variant() {
    assert_eq!(
        serde_json::from_value::<Value>(json!(false)).unwrap(),
        Value::Bool(false)
    );
    assert_eq!(
        serde_json::from_value::<Value>(json!(-3)).unwrap(),
        Value::Integer(-3)
    );
    assert_eq!(
        serde_json::from_value::<Value>(json!("balanced")).unwrap(),
        Value::Choice("balanced".into())
    );
}

#[test]
fn choice_settings_carry_options() {
    let setting: rubylight_native_api::radeon::Setting = serde_json::from_value(json!({
        "id": "vsync.mode", "group": "vsync", "kind": "choice", "state": "configurable",
        "value": "off",
        "options": [{"value": "off", "label": "Off"}, {"value": "on", "label": "Always on"}]
    }))
    .unwrap();
    assert_eq!(setting.options.len(), 2);
    assert_eq!(setting.value, Some(Value::Choice("off".into())));
    let without: rubylight_native_api::radeon::Setting = serde_json::from_value(json!({
        "id": "rsr.enabled", "group": "rsr", "kind": "toggle", "state": "unsupported"
    }))
    .unwrap();
    assert!(without.options.is_empty());
    assert!(serde_json::to_value(&without)
        .unwrap()
        .get("options")
        .is_none());
}
