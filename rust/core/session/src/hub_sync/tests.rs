//! Unit tests for Host Hub Sync session policy (`hub_sync`).

use super::*;
use rubylight_native_api::hub::StreamProfileSpec;
use serde_json::json;

fn sample_write(name: &str) -> ProfileWrite {
    ProfileWrite {
        scope: ProfileScope::Device,
        owner_device_uuid: None,
        name: name.into(),
        network_matcher: None,
        stream: StreamProfileSpec {
            width: Some(1920),
            height: Some(1080),
            fps: Some(60),
            bitrate_kbps: Some(40_000),
            upscaling: Some("sgsr1".into()),
            resolution_scale_pct: Some(100),
            ..Default::default()
        },
        input_deck_id: None,
    }
}

#[test]
fn validates_profile_ids_and_upscaling_and_scale() {
    assert!(validate_id("handheld-lan-90").is_ok());
    assert!(validate_id("a_0").is_ok());
    assert!(validate_id("").is_err());
    assert!(validate_id("-bad").is_err());
    assert!(validate_id("BadUpper").is_err());
    assert!(validate_id("a/b").is_err());
    assert!(validate_id("a#screen2").is_err());
    assert!(validate_id(&"a".repeat(65)).is_err());

    let mut bad_up = sample_write("Test");
    bad_up.stream.upscaling = Some("unknown_scaler".into());
    assert_eq!(
        validate_profile_write("ok", &bad_up).unwrap_err().code(),
        ErrorCode::InvalidValue
    );

    let mut bad_scale = sample_write("Test");
    bad_scale.stream.resolution_scale_pct = Some(25);
    assert_eq!(
        validate_profile_write("ok", &bad_scale).unwrap_err().code(),
        ErrorCode::InvalidValue
    );
}

#[test]
fn enforces_per_device_and_host_quotas() {
    let mut hub = HubState::default();
    for i in 0..MAX_PROFILES_PER_DEVICE {
        hub.put_profile(
            &format!("p-{i}"),
            "dev-a",
            false,
            0,
            sample_write("P"),
            1000,
        )
        .unwrap();
    }
    let over_device = hub
        .put_profile("p-64", "dev-a", false, 0, sample_write("P"), 1000)
        .unwrap_err();
    assert_eq!(over_device.code(), ErrorCode::InvalidValue);

    // Other devices can fill up to MAX_PROFILES_HOST total, then hit host cap.
    for dev in 0..3 {
        let uuid = format!("dev-{dev}");
        for i in 0..MAX_PROFILES_PER_DEVICE {
            hub.put_profile(
                &format!("d{dev}-{i}"),
                &uuid,
                false,
                0,
                sample_write("P"),
                1000,
            )
            .unwrap();
        }
    }
    assert_eq!(hub.profiles.len(), MAX_PROFILES_HOST);
    let over_host = hub
        .put_profile("extra", "dev-extra", false, 0, sample_write("P"), 1000)
        .unwrap_err();
    assert_eq!(over_host.code(), ErrorCode::InvalidValue);
}

#[test]
fn cas_conflicts_and_scoping_and_screen2_normalization() {
    let mut hub = HubState::default();
    let created = hub
        .put_profile("couch", "dev-a", false, 0, sample_write("Couch"), 10)
        .unwrap();
    assert!(created.created && created.profile.revision == 1);

    // Second writer with stale revision gets RevisionConflict with server revision.
    let conflict = hub
        .put_profile("couch", "dev-a", false, 0, sample_write("Stale"), 11)
        .unwrap_err();
    assert_eq!(conflict.code(), ErrorCode::RevisionConflict);
    assert_eq!(conflict.current_revision(), Some(1));

    // Another device without host_control is blocked cleanly with HostControlRequired.
    let denied = hub
        .put_profile("couch", "dev-b", false, 1, sample_write("Steal"), 12)
        .unwrap_err();
    assert_eq!(denied.code(), ErrorCode::HostControlRequired);

    // Screen2 client maps to the primary device UUID bucket.
    let screen2 = Client {
        name: "Second screen".into(),
        cert: String::new(),
        uuid: format!("dev-a{}", state::SECOND_SCREEN_SUFFIX),
        perm: 0,
        enabled: true,
        extra: BTreeMap::new(),
    };
    assert_eq!(normalized_device_uuid(&screen2), "dev-a");

    // Shared profile creation requires host_control.
    let mut shared = sample_write("Shared 4K");
    shared.scope = ProfileScope::Shared;
    assert_eq!(
        hub.put_profile("shared-4k", "dev-a", false, 0, shared.clone(), 13)
            .unwrap_err()
            .code(),
        ErrorCode::HostControlRequired
    );
    let ok_shared = hub
        .put_profile("shared-4k", "dev-a", true, 0, shared, 13)
        .unwrap();
    assert_eq!(ok_shared.profile.scope, ProfileScope::Shared);
    assert_eq!(hub.visible_profiles("dev-b", false, false).len(), 1);
}

#[test]
fn projects_app_device_app_and_device_overrides() {
    let mut app_extra = BTreeMap::new();
    app_extra.insert(
        device_overrides::SETTING.into(),
        json!({
            "dev-a": {
                "config-overrides": { "max_bitrate": 30000, "hdr": false },
                "radeon-profile": "competitive",
                "game-files": [{"path": "cfg.ini", "section": "Video", "key": "Width", "value": "1280"}]
            }
        }),
    );
    app_extra.insert(
        "config-overrides".into(),
        json!({ "codec": "av1", "max_bitrate": 60000 }),
    );
    let app = App {
        name: "Game".into(),
        cmd: String::new(),
        working_dir: String::new(),
        prep: Vec::new(),
        computed_id: Some(1042),
        aliases: Vec::new(),
        extra: app_extra,
    };
    let mut client_extra = BTreeMap::new();
    client_extra.insert("config_overrides".into(), json!({ "gamepad": "ds5" }));
    let client = Client {
        name: "Handheld".into(),
        cert: String::new(),
        uuid: "dev-a#screen2".into(),
        perm: 0,
        enabled: true,
        extra: client_extra,
    };
    let cleared_radeon_app = App {
        name: "Cleared".into(),
        cmd: String::new(),
        working_dir: String::new(),
        prep: Vec::new(),
        computed_id: Some(1043),
        aliases: Vec::new(),
        extra: BTreeMap::from([
            ("radeon-profile".into(), json!("quality")),
            ("radeon-settings".into(), json!({"sharp": true})),
            (
                device_overrides::SETTING.into(),
                json!({"dev-a": {"radeon-profile": ""}}),
            ),
        ]),
    };
    let summary = host_overrides_summary(&[app, cleared_radeon_app], &client);
    assert_eq!(summary.len(), 3);
    assert_eq!(
        summary[0].locked_keys,
        vec!["game_files", "hdr", "max_bitrate", "radeon_profile"]
    );
    assert_eq!(summary[1].source, "app");
    assert_eq!(summary[1].locked_keys, vec!["codec"]);
    assert_eq!(summary[2].app_id, 0);
    assert_eq!(summary[2].source, "device");
    assert_eq!(summary[2].locked_keys, vec!["gamepad"]);

    let mut hub = HubState::default();
    for i in 0..MAX_PROFILES_PER_DEVICE {
        hub.put_profile(&format!("d-{i}"), "dev-a", false, 0, sample_write("D"), 1)
            .unwrap();
    }
    let updated = hub
        .put_profile("d-0", "admin", true, 1, sample_write("Admin"), 2)
        .unwrap();
    assert_eq!(updated.profile.owner_device_uuid, "dev-a");
    let mut shared = sample_write("S");
    shared.scope = ProfileScope::Shared;
    hub.put_profile("s-0", "admin", true, 0, shared, 3).unwrap();
    let mut to_dev = sample_write("ToDev");
    to_dev.owner_device_uuid = Some("dev-a".into());
    assert_eq!(
        hub.put_profile("s-0", "admin", true, 1, to_dev, 4)
            .unwrap_err()
            .code(),
        ErrorCode::InvalidValue
    );
}
