use super::*;
use std::io::Cursor;

const HOOKS: &str =
    r#"[{"url":"https://example.invalid/a","events":["stream_started"],"secret":"old-a"}]"#;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A host data folder with settings, an app, a cover, a game settings file,
/// one paired device with its own settings, and its pairing identity.
fn profile() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        &root.join(CONFIG),
        &format!("sunshine_name = Den\nport = 47989\napi_token = tokval\nwebhooks = {HOOKS}\n"),
    );
    write(
        &root.join(APPS),
        r#"{"apps":[{"uuid":"u1","name":"Desktop"}],"env":{}}"#,
    );
    write(&root.join("covers/u1.png"), "png");
    write(&root.join("game-settings/phone/game.json"), "{}");
    write(
        &root.join(PAIRED),
        r#"{"root":{"named_devices":[{"name":"Phone","cert":"PEM","uuid":"d1","perm":7,"enabled":true,"display_mode":"1920x1080x60","webrtc_secret":"s"}],"username":"admin","password":"hash","salt":"x"}}"#,
    );
    write(&root.join(CERTIFICATE), "cert");
    write(&root.join(KEY), "key");
    dir
}

fn backup(root: &Path, secrets: bool) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    export(root, secrets, "4.1.0", 1_700_000_000, &mut out).unwrap();
    out.into_inner()
}

fn names(archive: &Archive) -> Vec<&str> {
    archive.files.keys().map(String::as_str).collect()
}

#[test]
fn settings_backup_without_secrets_leaves_out_pairing_identity_and_credentials() {
    let profile = profile();
    let bytes = backup(profile.path(), false);
    let archive = read(Cursor::new(&bytes)).unwrap();
    assert!(!archive.manifest.secrets);
    assert_eq!(archive.manifest.format, FORMAT);
    assert_eq!(archive.manifest.host_version, "4.1.0");
    assert_eq!(
        names(&archive),
        [
            APPS,
            "covers/u1.png",
            DEVICES,
            "game-settings/phone/game.json",
            CONFIG,
        ]
    );
    let config = archive.config();
    assert_eq!(config.get("sunshine_name", ""), "Den");
    assert!(!config.values.contains_key("api_token"));
    assert!(!config.text().contains("old-a"));
    let devices = archive.json(DEVICES).unwrap();
    let device = &devices["devices"][0];
    assert_eq!(device["uuid"], "d1");
    assert_eq!(device["settings"]["display_mode"], "1920x1080x60");
    for secret in ["cert", "perm", "enabled", "webrtc_secret"] {
        assert!(device["settings"].get(secret).is_none(), "{secret}");
    }
    let text = String::from_utf8_lossy(&bytes);
    for secret in ["PEM", "hash", "old-a", "tokval"] {
        assert!(!text.contains(secret), "{secret} leaked");
    }
}

#[test]
fn settings_backup_with_secrets_carries_pairing_identity_and_sign_in() {
    let profile = profile();
    let archive = read(Cursor::new(backup(profile.path(), true))).unwrap();
    assert!(archive.manifest.secrets);
    assert_eq!(
        names(&archive),
        [
            APPS,
            "covers/u1.png",
            CERTIFICATE,
            KEY,
            crate::webhook::SECRETS_FILE,
            "game-settings/phone/game.json",
            CONFIG,
            PAIRED,
        ]
    );
    assert_eq!(archive.config().get("api_token", ""), "tokval");
    assert!(!archive.config().text().contains("old-a"));
    assert_eq!(
        archive.json(crate::webhook::SECRETS_FILE).unwrap()["secrets"]["https://example.invalid/a"],
        "old-a"
    );
}

/// Rewrites an archive with `change` applied to its files and manifest,
/// keeping or refreshing the checksums as `relist` says.
fn tampered(
    bytes: &[u8],
    change: impl FnOnce(&mut BTreeMap<String, Vec<u8>>, &mut Manifest),
    relist: bool,
) -> Vec<u8> {
    let archive = read(Cursor::new(bytes)).unwrap();
    let mut files = archive.files;
    let mut manifest = archive.manifest;
    change(&mut files, &mut manifest);
    if relist {
        manifest.files = files
            .iter()
            .map(|(path, bytes)| Entry {
                path: path.clone(),
                size: bytes.len() as u64,
                sha256: sha256(bytes),
            })
            .collect();
    }
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in &files {
        writer.start_file(name.as_str(), options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.start_file(MANIFEST, options).unwrap();
    writer
        .write_all(&serde_json::to_vec(&manifest).unwrap())
        .unwrap();
    writer.finish().unwrap().into_inner()
}

#[test]
fn settings_backup_refuses_damaged_foreign_or_unsafe_archives() {
    let profile = profile();
    let plain = backup(profile.path(), false);
    let refused = |bytes: Vec<u8>, says: &str| {
        let error = format!("{:#}", read(Cursor::new(bytes)).unwrap_err());
        assert!(error.contains(says), "{says}: {error}");
    };
    refused(b"not a zip".to_vec(), "not a ZIP");
    refused(
        tampered(
            &plain,
            |files, _| {
                files.insert(APPS.into(), br#"{"apps":[]} "#.to_vec());
            },
            false,
        ),
        "changed or damaged",
    );
    refused(
        tampered(&plain, |_, manifest| manifest.version = VERSION + 1, true),
        "newer Rubylight",
    );
    refused(
        tampered(&plain, |_, manifest| manifest.format = "other".into(), true),
        "no Rubylight settings backup",
    );
    for unsafe_path in [
        "../evil.txt",
        "covers/../../x.png",
        "C:/x",
        "/abs",
        "notes.txt",
    ] {
        refused(
            tampered(
                &plain,
                |files, _| {
                    files.insert(unsafe_path.into(), b"x".to_vec());
                },
                true,
            ),
            "not a file a settings backup holds",
        );
    }
    refused(
        tampered(
            &plain,
            |files, _| {
                files.insert(KEY.into(), b"key".to_vec());
            },
            true,
        ),
        "holds secrets",
    );
    refused(
        tampered(
            &plain,
            |files, _| {
                files.insert("covers/extra.png".into(), b"x".to_vec());
            },
            false,
        ),
        "not in its backup.json",
    );
    refused(
        tampered(
            &plain,
            |files, _| {
                files.remove(CONFIG);
            },
            true,
        ),
        "no sunshine.conf",
    );
    refused(
        tampered(
            &plain,
            |files, _| {
                files.insert(APPS.into(), b"[1]".to_vec());
            },
            true,
        ),
        "apps.json",
    );
}

#[test]
fn settings_backup_restore_without_secrets_keeps_this_pcs_identity_and_secrets() {
    let source = profile();
    write(
        &source.path().join(CONFIG),
        &format!(
            "sunshine_name = Studio\nwebhooks = {}\n",
            r#"[{"url":"https://example.invalid/a","events":[],"secret":"src-a"},{"url":"https://example.invalid/b","events":[],"secret":"src-b"}]"#
        ),
    );
    let archive = read(Cursor::new(backup(source.path(), false))).unwrap();

    let target = profile();
    let root = target.path();
    write(&root.join("covers/only-here.png"), "keep");
    write(
        &root.join(PAIRED),
        r#"{"root":{"named_devices":[{"name":"Old name","cert":"TARGET","uuid":"d1","perm":3,"enabled":false,"display_mode":"1280x720x60"},{"name":"Tablet","cert":"T2","uuid":"d2","perm":1,"enabled":true}],"username":"admin","password":"target-hash","salt":"y"}}"#,
    );
    let summary = summarize(&archive, root);
    let detail = |item: &str| {
        summary
            .changes
            .iter()
            .find(|c| c.item == item)
            .map(|c| c.detail.clone())
            .unwrap_or_default()
    };
    assert!(detail("settings").contains("sunshine_name"));
    assert!(!detail("settings").contains("api_token"));
    assert!(detail("devices").contains("Settings of 1 paired devices"));
    assert!(detail("pairing").contains("stay"));
    assert!(detail("webhooks").contains("1 get a new secret"));

    let mut fresh = 0;
    apply(&archive, root, || {
        fresh += 1;
        format!("fresh-{fresh}")
    })
    .unwrap();
    let config = Config::load(&root.join(CONFIG)).unwrap();
    assert_eq!(config.get("sunshine_name", ""), "Studio");
    // This PC's own API token and webhook secret stay; a webhook it did not
    // have gets a fresh one.
    assert_eq!(config.get("api_token", ""), "tokval");
    assert!(!config.get("webhooks", "").contains("old-a"));
    assert!(!config.get("webhooks", "").contains("fresh-1"));
    let stored_secrets = crate::webhook::load_secrets(&crate::webhook::secrets_path(root)).unwrap();
    assert_eq!(stored_secrets["https://example.invalid/a"], "old-a");
    assert_eq!(stored_secrets["https://example.invalid/b"], "fresh-1");
    let paired: Value = serde_json::from_slice(&std::fs::read(root.join(PAIRED)).unwrap()).unwrap();
    let devices = &paired["root"]["named_devices"];
    assert_eq!(devices[0]["name"], "Phone");
    assert_eq!(devices[0]["display_mode"], "1920x1080x60");
    assert_eq!(devices[0]["cert"], "TARGET");
    assert_eq!(devices[0]["perm"], 3);
    assert_eq!(devices[0]["enabled"], false);
    assert_eq!(devices[1]["name"], "Tablet");
    assert_eq!(paired["root"]["password"], "target-hash");
    assert_eq!(std::fs::read(root.join(KEY)).unwrap(), b"key");
    assert_eq!(
        std::fs::read(root.join("covers/only-here.png")).unwrap(),
        b"keep"
    );
}

#[test]
fn settings_backup_restore_with_secrets_replaces_pairing_identity() {
    let source = profile();
    write(&source.path().join(KEY), "source-key");
    let archive = read(Cursor::new(backup(source.path(), true))).unwrap();
    let target = profile();
    let root = target.path();
    write(&root.join(KEY), "target-key");
    write(&root.join(PAIRED), r#"{"root":{"named_devices":[]}}"#);
    let summary = summarize(&archive, root);
    assert!(summary.secrets);
    assert!(summary.changes.iter().any(|c| c.item == "credentials"));
    apply(&archive, root, || unreachable!("secrets are in the backup")).unwrap();
    assert_eq!(std::fs::read(root.join(KEY)).unwrap(), b"source-key");
    let paired: Value = serde_json::from_slice(&std::fs::read(root.join(PAIRED)).unwrap()).unwrap();
    assert_eq!(paired["root"]["named_devices"][0]["cert"], "PEM");
}

#[test]
fn settings_backup_a_staged_restore_applies_once_and_a_bad_one_is_set_aside() {
    let source = profile();
    write(&source.path().join(CONFIG), "sunshine_name = Studio\n");
    let bytes = backup(source.path(), false);
    let target = profile();
    let root = target.path();
    assert!(!apply_staged(root, String::new).unwrap());
    std::fs::create_dir_all(root.join(BACKUPS)).unwrap();
    std::fs::write(root.join(BACKUPS).join(STAGED), &bytes).unwrap();
    assert!(apply_staged(root, String::new).unwrap());
    assert!(!root.join(BACKUPS).join(STAGED).exists());
    assert_eq!(
        Config::load(&root.join(CONFIG))
            .unwrap()
            .get("sunshine_name", ""),
        "Studio"
    );
    assert!(!apply_staged(root, String::new).unwrap());

    std::fs::write(root.join(BACKUPS).join(STAGED), b"broken").unwrap();
    assert!(apply_staged(root, String::new).is_err());
    assert!(!root.join(BACKUPS).join(STAGED).exists());
    assert!(root.join(BACKUPS).join("restore-failed.zip").exists());
    assert_eq!(
        Config::load(&root.join(CONFIG))
            .unwrap()
            .get("sunshine_name", ""),
        "Studio"
    );
}

#[test]
fn settings_backup_restore_never_moves_where_this_pc_keeps_its_files() {
    let source = profile();
    let plain = backup(source.path(), false);
    let elsewhere = tempfile::tempdir().unwrap();
    let redirect = elsewhere.path().join("apps.json");
    let crafted = tampered(
        &plain,
        |files, _| {
            let mut config = String::from_utf8(files[CONFIG].clone()).unwrap();
            config.push_str(&format!("file_apps = {}\n", redirect.display()));
            files.insert(CONFIG.into(), config.into_bytes());
        },
        true,
    );
    let archive = read(Cursor::new(crafted)).unwrap();
    let target = profile();
    let root = target.path();
    std::fs::write(root.join(APPS), r#"{"apps":[]}"#).unwrap();
    apply(&archive, root, String::new).unwrap();
    assert!(!redirect.exists());
    assert!(
        !Config::load(&root.join(CONFIG))
            .unwrap()
            .values
            .contains_key("file_apps")
    );
    assert!(
        std::fs::read_to_string(root.join(APPS))
            .unwrap()
            .contains("Desktop")
    );
}

#[test]
fn console_sign_in_sessions_never_leave_in_a_backup_and_a_restore_keeps_the_targets() {
    let state = br#"{"root":{"session_tokens":[{"hash":"theirs","username":"admin","address":"10.0.0.2","user_agent":"Browser"}],"api_tokens":[{"hash":"their-api"}],"display_aliases":{"a":"b"}}}"#;
    let exported_plain: Value =
        serde_json::from_slice(&without_sessions(state, false).unwrap()).unwrap();
    assert!(
        exported_plain["root"].get(SESSIONS).is_none(),
        "{exported_plain}"
    );
    assert!(
        exported_plain["root"].get(API_TOKENS).is_none(),
        "{exported_plain}"
    );
    assert_eq!(
        exported_plain["root"]["display_aliases"]["a"], "b",
        "the rest is kept"
    );
    let exported_secrets: Value =
        serde_json::from_slice(&without_sessions(state, true).unwrap()).unwrap();
    assert!(
        exported_secrets["root"].get(SESSIONS).is_none(),
        "{exported_secrets}"
    );
    assert_eq!(
        exported_secrets["root"][API_TOKENS],
        json!([{"hash": "their-api"}])
    );
    assert!(
        without_sessions(b"not json", false).is_none(),
        "an unreadable file stays out whole"
    );
    // A non-secret restore writes the archive's settings with the target's own sessions and API tokens.
    let target =
        br#"{"root":{"session_tokens":[{"hash":"mine"}],"api_tokens":[{"hash":"my-api"}]}}"#;
    let restored: Value =
        serde_json::from_slice(&keeping_sessions(state, Some(target), false).unwrap()).unwrap();
    assert_eq!(restored["root"][SESSIONS], json!([{"hash":"mine"}]));
    assert_eq!(restored["root"][API_TOKENS], json!([{"hash":"my-api"}]));
    assert_eq!(restored["root"]["display_aliases"]["a"], "b");
    // A secret restore keeps target sessions but restores archived API tokens.
    let restored_secrets: Value =
        serde_json::from_slice(&keeping_sessions(state, Some(target), true).unwrap()).unwrap();
    assert_eq!(restored_secrets["root"][SESSIONS], json!([{"hash":"mine"}]));
    assert_eq!(
        restored_secrets["root"][API_TOKENS],
        json!([{"hash":"their-api"}])
    );
    // A target without sessions or API tokens gets neither from a non-secret archive.
    let fresh: Value =
        serde_json::from_slice(&keeping_sessions(state, None, false).unwrap()).unwrap();
    assert!(fresh["root"].get(SESSIONS).is_none(), "{fresh}");
    assert!(fresh["root"].get(API_TOKENS).is_none(), "{fresh}");
}

#[test]
fn non_secret_backup_does_not_leak_or_overwrite_device_grants_or_invites() {
    let source = profile();
    write(
        &source.path().join(PAIRED),
        r#"{"root":{"named_devices":[{"name":"Phone","cert":"PEM","uuid":"d1","perm":7,"enabled":true,"host_control":true,"pairing_approve":true,"allow_sleep":true,"allow_restart_shutdown":true,"allow_usb":true,"coop_join":true,"shared_folders":["s"],"allow_client_commands":true,"invited_guest":{"until":999,"permission":"view"},"display_mode":"1920x1080x60"}]}}"#,
    );
    let bytes = backup(source.path(), false);
    let archive = read(Cursor::new(&bytes)).unwrap();
    let devices = archive.json(DEVICES).unwrap();
    let settings = &devices["devices"][0]["settings"];
    assert_eq!(settings["display_mode"], "1920x1080x60");
    for restricted in [
        "host_control",
        "pairing_approve",
        "allow_sleep",
        "allow_restart_shutdown",
        "allow_usb",
        "coop_join",
        "shared_folders",
        "allow_client_commands",
        crate::invites::MARKER,
    ] {
        assert!(settings.get(restricted).is_none(), "{restricted}");
    }

    let target = profile();
    write(
        &target.path().join(PAIRED),
        r#"{"root":{"named_devices":[{"name":"Old","cert":"TARGET","uuid":"d1","perm":3,"enabled":true,"host_control":false,"allow_usb":false,"invited_guest":{"until":12345,"permission":"view"},"display_mode":"1280x720x60"}]}}"#,
    );
    apply(&archive, target.path(), String::new).unwrap();
    let paired: Value =
        serde_json::from_slice(&std::fs::read(target.path().join(PAIRED)).unwrap()).unwrap();
    let device = &paired["root"]["named_devices"][0];
    assert_eq!(device["display_mode"], "1920x1080x60");
    assert_eq!(device["host_control"], false);
    assert_eq!(device["allow_usb"], false);
    assert!(device.get("pairing_approve").is_none());
    assert_eq!(
        device[crate::invites::MARKER],
        json!({"until": 12345, "permission": "view"}),
        "a non-secret restore must keep an invited guest's expiry and permission"
    );
}

#[test]
fn settings_backup_restore_with_secrets_replaces_stale_separate_credentials_file() {
    use crate::state::{Credentials, ProfileFiles};

    let source = profile();
    let src_creds = Credentials::new("source_user".into(), "sourcepass123").unwrap();
    let mut src_paired = json!({"root": {"named_devices": []}});
    for (k, v) in serde_json::to_value(&src_creds)
        .unwrap()
        .as_object()
        .unwrap()
    {
        src_paired[k] = v.clone();
    }
    write(
        &source.path().join(PAIRED),
        &serde_json::to_string(&src_paired).unwrap(),
    );
    let archive = read(Cursor::new(backup(source.path(), true))).unwrap();
    assert!(!archive.files.contains_key(CREDENTIALS_FILE));

    // Case 1: target has default sunshine_credentials.json shadowing sunshine_state.json.
    let target = profile();
    let target_creds = Credentials::new("old_user".into(), "oldpassword1").unwrap();
    crate::state::write_json(&target.path().join(CREDENTIALS_FILE), &target_creds).unwrap();
    apply(&archive, target.path(), String::new).unwrap();
    assert!(!target.path().join(CREDENTIALS_FILE).exists());
    let cfg = Config::load(&target.path().join(CONFIG)).unwrap();
    let loaded = Credentials::load(&ProfileFiles::new(&cfg, target.path()).credentials)
        .unwrap()
        .expect("restored credentials");
    assert!(loaded.verifies("source_user", "sourcepass123"));
    assert!(!loaded.verifies("old_user", "oldpassword1"));

    // Case 2: target configures an explicit credentials_file preserved by LOCATION_KEYS.
    let custom_target = profile();
    write(
        &custom_target.path().join(CONFIG),
        "credentials_file = custom_creds.json\n",
    );
    crate::state::write_json(
        &custom_target.path().join("custom_creds.json"),
        &target_creds,
    )
    .unwrap();
    apply(&archive, custom_target.path(), String::new).unwrap();
    let custom_cfg = Config::load(&custom_target.path().join(CONFIG)).unwrap();
    let loaded_custom =
        Credentials::load(&ProfileFiles::new(&custom_cfg, custom_target.path()).credentials)
            .unwrap()
            .expect("restored custom credentials");
    assert!(loaded_custom.verifies("source_user", "sourcepass123"));
    assert!(!loaded_custom.verifies("old_user", "oldpassword1"));
}

#[test]
fn settings_backup_round_trips_nested_covers_and_merges_single_file_credentials() {
    use crate::state::{Credentials, ProfileFiles};

    let source = profile();
    write(&source.path().join("covers/sub/nested.png"), "nested-png");
    let src_creds = Credentials::new("merged_user".into(), "mergedpass123").unwrap();
    crate::state::write_json(&source.path().join(CREDENTIALS_FILE), &src_creds).unwrap();
    let bytes = backup(source.path(), true);
    let archive = read(Cursor::new(&bytes)).unwrap();
    assert_eq!(archive.files["covers/sub/nested.png"], b"nested-png");

    let target = profile();
    write(
        &target.path().join(CONFIG),
        &format!("credentials_file = {PAIRED}\n"),
    );
    apply(&archive, target.path(), String::new).unwrap();
    assert_eq!(
        std::fs::read(target.path().join("covers/sub/nested.png")).unwrap(),
        b"nested-png"
    );
    let cfg = Config::load(&target.path().join(CONFIG)).unwrap();
    let files = ProfileFiles::new(&cfg, target.path());
    assert_eq!(files.credentials, files.paired);
    let loaded = Credentials::load(&files.credentials)
        .unwrap()
        .expect("credentials merged into paired file");
    assert!(loaded.verifies("merged_user", "mergedpass123"));
}

#[test]
fn settings_backup_apply_rolls_back_all_written_files_on_failure() {
    let source = profile();
    write(
        &source.path().join(CONFIG),
        "sunshine_name = Changed\nport = 47989\n",
    );
    write(
        &source.path().join(APPS),
        r#"{"apps":[{"uuid":"new","name":"NewApp"}],"env":{}}"#,
    );
    write(&source.path().join("covers/new_cover.png"), "new-bytes");
    let archive = read(Cursor::new(backup(source.path(), true))).unwrap();

    let target = profile();
    let root = target.path();
    let orig_config = std::fs::read(root.join(CONFIG)).unwrap();
    let orig_apps = std::fs::read(root.join(APPS)).unwrap();
    // Place a directory at `credentials/cakey.pem` so atomic rename fails mid-restore.
    std::fs::remove_file(root.join(KEY)).unwrap();
    std::fs::create_dir_all(root.join(KEY)).unwrap();

    assert!(apply(&archive, root, String::new).is_err());
    assert_eq!(std::fs::read(root.join(CONFIG)).unwrap(), orig_config);
    assert_eq!(std::fs::read(root.join(APPS)).unwrap(), orig_apps);
    assert!(!root.join("covers/new_cover.png").exists());
}

#[test]
fn settings_backup_verify_rejects_bad_archive_and_staged_claim_clears_staged_file() {
    let source = profile();
    let bytes = backup(source.path(), false);
    assert!(verify(Cursor::new(&bytes)).is_ok());
    assert!(verify(Cursor::new(b"bad-zip")).is_err());

    // Simulate a leftover applying marker alongside a newly staged restore.
    let target = profile();
    let backups = target.path().join(BACKUPS);
    std::fs::create_dir_all(&backups).unwrap();
    std::fs::write(backups.join("restore-applying.zip"), b"stale").unwrap();
    std::fs::write(backups.join(STAGED), &bytes).unwrap();
    assert!(apply_staged(target.path(), String::new).unwrap());
    assert!(!backups.join(STAGED).exists());
    assert!(!backups.join("restore-applying.zip").exists());
    assert!(!apply_staged(target.path(), String::new).unwrap());
}

#[test]
fn settings_backup_gates_wake_relay_secrets_on_export_read_and_restore() {
    let relay_json = r#"{"epoch":3,"relays":[{"id":"r1","name":"NUC","secret":"wake-relay-secret-value","enrolled_unix":1,"last_seen_unix":2}]}"#;
    let source = profile();
    write(&source.path().join(WAKE_RELAY_FILE), relay_json);

    // Export without secrets excludes credentials/wake-relay.json and never leaks its secret.
    let plain = backup(source.path(), false);
    let plain_archive = read(Cursor::new(&plain)).unwrap();
    assert!(!plain_archive.files.contains_key(WAKE_RELAY_FILE));
    assert!(!plain_archive.files.contains_key(LEGACY_WAKE_RELAY_FILE));
    assert!(!String::from_utf8_lossy(&plain).contains("wake-relay-secret-value"));

    // An archive marked secrets=false carrying wake-relay.json at either path is rejected.
    for path in [WAKE_RELAY_FILE, LEGACY_WAKE_RELAY_FILE] {
        let crafted = tampered(
            &plain,
            |files, _| {
                files.insert(path.into(), relay_json.as_bytes().to_vec());
            },
            true,
        );
        let err = format!("{:#}", read(Cursor::new(crafted)).unwrap_err());
        assert!(err.contains("holds secrets"), "{path}: {err}");
    }

    // Export with secrets carries credentials/wake-relay.json.
    let with_secrets = backup(source.path(), true);
    let secret_archive = read(Cursor::new(&with_secrets)).unwrap();
    assert_eq!(
        secret_archive.files.get(WAKE_RELAY_FILE).map(Vec::as_slice),
        Some(relay_json.as_bytes())
    );

    // Unmigrated legacy root wake-relay.json is omitted without secrets and promoted with secrets.
    let legacy_source = profile();
    write(
        &legacy_source.path().join(LEGACY_WAKE_RELAY_FILE),
        relay_json,
    );
    let legacy_plain = backup(legacy_source.path(), false);
    assert!(!String::from_utf8_lossy(&legacy_plain).contains("wake-relay-secret-value"));
    let legacy_secret = read(Cursor::new(backup(legacy_source.path(), true))).unwrap();
    assert_eq!(
        legacy_secret.files.get(WAKE_RELAY_FILE).map(Vec::as_slice),
        Some(relay_json.as_bytes())
    );
    assert!(!legacy_secret.files.contains_key(LEGACY_WAKE_RELAY_FILE));

    // Restoring without secrets leaves the target PC's wake-relay.json untouched.
    let target = profile();
    let target_relay = r#"{"epoch":1,"relays":[{"id":"local","name":"Local","secret":"local-secret","enrolled_unix":1,"last_seen_unix":1}]}"#;
    write(&target.path().join(WAKE_RELAY_FILE), target_relay);
    apply(&plain_archive, target.path(), String::new).unwrap();
    assert_eq!(
        std::fs::read_to_string(target.path().join(WAKE_RELAY_FILE)).unwrap(),
        target_relay
    );

    // Restoring with secrets replaces credentials/wake-relay.json and cleans up any stale root file.
    write(&target.path().join(LEGACY_WAKE_RELAY_FILE), target_relay);
    apply(&secret_archive, target.path(), String::new).unwrap();
    assert_eq!(
        std::fs::read_to_string(target.path().join(WAKE_RELAY_FILE)).unwrap(),
        relay_json
    );
    assert!(!target.path().join(LEGACY_WAKE_RELAY_FILE).exists());

    // Restoring a legacy secrets=true archive carrying root wake-relay.json migrates it into credentials/.
    let legacy_archive_bytes = tampered(
        &with_secrets,
        |files, _| {
            let bytes = files.remove(WAKE_RELAY_FILE).unwrap();
            files.insert(LEGACY_WAKE_RELAY_FILE.into(), bytes);
        },
        true,
    );
    let legacy_archive = read(Cursor::new(legacy_archive_bytes)).unwrap();
    let migrated_target = profile();
    write(
        &migrated_target.path().join(LEGACY_WAKE_RELAY_FILE),
        target_relay,
    );
    apply(&legacy_archive, migrated_target.path(), String::new).unwrap();
    assert_eq!(
        std::fs::read_to_string(migrated_target.path().join(WAKE_RELAY_FILE)).unwrap(),
        relay_json
    );
    assert!(!migrated_target.path().join(LEGACY_WAKE_RELAY_FILE).exists());
}

#[test]
fn tx_writer_rollback_attempts_every_entry_and_collects_errors() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("a.txt");
    let blocked = dir.path().join("b.txt");
    let last = dir.path().join("c.txt");
    std::fs::write(&first, b"orig-a").unwrap();
    std::fs::write(&blocked, b"orig-b").unwrap();

    let mut tx = restore::TxWriter::new();
    tx.write(&first, b"new-a").unwrap();
    tx.write(&blocked, b"new-b").unwrap();
    tx.write(&last, b"new-c").unwrap();
    // Replace `blocked` with a directory so rolling `blocked` back fails.
    std::fs::remove_file(&blocked).unwrap();
    std::fs::create_dir(&blocked).unwrap();

    let err = format!("{:#}", tx.rollback().unwrap_err());
    assert!(err.contains("b.txt"), "combined rollback error: {err}");
    // Even though `b.txt` failed mid-rollback, both `a.txt` and `c.txt` were still rolled back.
    assert_eq!(std::fs::read(&first).unwrap(), b"orig-a");
    assert!(!last.exists(), "newly created c.txt must still be removed");
}

#[test]
fn streaming_inspect_reader_matches_summarize_and_rejects_tampered_cover() {
    let source = profile();
    let target = profile();
    let bytes = backup(source.path(), false);
    let archive = read(Cursor::new(&bytes)).unwrap();
    let expected = summarize(&archive, target.path());
    let streamed = inspect_reader(Cursor::new(&bytes), target.path()).unwrap();
    assert_eq!(streamed, expected);

    let broken = tampered(
        &bytes,
        |files, _| {
            files.insert("covers/u1.png".into(), b"CORRUPTED".to_vec());
        },
        false,
    );
    let err = format!(
        "{:#}",
        inspect_reader(Cursor::new(&broken), target.path()).unwrap_err()
    );
    assert!(err.contains("changed or damaged"), "{err}");
}
