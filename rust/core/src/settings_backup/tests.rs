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
            "game-settings/phone/game.json",
            CONFIG,
            PAIRED,
        ]
    );
    assert_eq!(archive.config().get("api_token", ""), "tokval");
    assert!(archive.config().text().contains("old-a"));
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
    let hooks: Value = serde_json::from_str(config.get("webhooks", "")).unwrap();
    assert_eq!(hooks[0]["secret"], "old-a");
    assert_eq!(hooks[1]["secret"], "fresh-1");
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
    let state = br#"{"root":{"session_tokens":[{"hash":"theirs","username":"admin","address":"10.0.0.2","user_agent":"Browser"}],"display_aliases":{"a":"b"}}}"#;
    let exported: Value = serde_json::from_slice(&without_sessions(state).unwrap()).unwrap();
    assert!(exported["root"].get(SESSIONS).is_none(), "{exported}");
    assert_eq!(
        exported["root"]["display_aliases"]["a"], "b",
        "the rest is kept"
    );
    assert!(
        without_sessions(b"not json").is_none(),
        "an unreadable file stays out whole"
    );
    // A restore writes the archive's settings with the target's own sessions.
    let target = br#"{"root":{"session_tokens":[{"hash":"mine"}]}}"#;
    let restored: Value =
        serde_json::from_slice(&keeping_sessions(state, Some(target)).unwrap()).unwrap();
    assert_eq!(restored["root"][SESSIONS], json!([{"hash":"mine"}]));
    assert_eq!(restored["root"]["display_aliases"]["a"], "b");
    // A target without sessions gets none from the archive.
    let fresh: Value = serde_json::from_slice(&keeping_sessions(state, None).unwrap()).unwrap();
    assert!(fresh["root"].get(SESSIONS).is_none(), "{fresh}");
}
