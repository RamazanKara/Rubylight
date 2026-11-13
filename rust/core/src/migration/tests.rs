use super::*;
#[test]
fn oversized_state_is_rejected_before_parsing() {
    let temp = tempfile::tempdir().unwrap();
    let old = temp.path().join("old");
    std::fs::create_dir(&old).unwrap();
    std::fs::write(old.join("sunshine.conf"), "").unwrap();
    let file = std::fs::File::create(old.join("sunshine_state.json")).unwrap();
    file.set_len(FILE_LIMIT + 1).unwrap();
    let next = temp.path().join("next");
    let error = import(&old, &next).unwrap_err().to_string();
    assert!(error.contains("larger than 64 MiB"), "{error}");
    assert!(!next.exists());
}
#[test]
fn realistic_profiles_keep_pairings_credentials_settings_and_unknown_fields() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let identity = crypto::Identity::generate()?;
    let credentials = state::Credentials::new("admin".into(), "existing-password")?;
    for family in [
        "Vibepollo 2.0",
        "Apollo",
        "Sunshine",
        "Butterpollo 2.0.0-rc.20",
    ] {
        let old = temp.path().join(family).join("Çağrı Müller/config");
        std::fs::create_dir_all(old.join("credentials"))?;
        let settings = "sunshine_name = Çağrı Müller\nencoder = amdvce_experimental\namdvce_experimental = true\namd_quality = quality\nvirtual_display_mode = per_client\nvirtual_display_layout = extended\nport = 48000\ngamepad = x360\nfuture_setting = {\"version\":99,\"keep\":true}\n";
        let mut utf16 = vec![0xff, 0xfe];
        utf16.extend(settings.encode_utf16().flat_map(u16::to_le_bytes));
        std::fs::write(old.join("sunshine.conf"), &utf16)?;
        let paired = if family == "Sunshine" {
            json!({"root":{"uniqueid":"same-host","devices":[{"certs":[identity.certificate]}]},"future":99})
        } else {
            json!({"root":{"uniqueid":"same-host","named_devices":[{
                "name":"Living room","cert":identity.certificate,"uuid":"same-client",
                "perm":"119480064","enabled":"true","display_mode":"2560x1440x120",
                "virtual_display_guid":"same-display","config_overrides":{"amd_quality":"speed","gamepad":"ds4"},
                "future_device":{"keep":true}}]},"future":99})
        };
        let aliases = json!({"root":{"shared_virtual_display_guid":"f425b740-e441-4b87-8e78-07b91579bf70",
            "session_tokens":[{"hash":"saved-session","future_token":true}],"future_state":[1,2,3]}});
        let apps = json!({"env":{"CUSTOM":"kept"},"future_library":99,"apps":[{
            "name":"Game","uuid":"same-app","cmd":"game.exe","image-path":old.join("cover.png"),
            "prep-cmd":[{"do":"before.cmd","undo":"after.cmd","elevated":"false"}],
            "future_app":{"keep":true}}]});
        state::write_json(&old.join("sunshine_state.json"), &paired)?;
        state::write_json(&old.join("vibeshine_state.json"), &aliases)?;
        state::write_json(&old.join("sunshine_credentials.json"), &credentials)?;
        state::write_json(&old.join("apps.json"), &apps)?;
        std::fs::write(old.join("credentials/cacert.pem"), &identity.certificate)?;
        std::fs::write(old.join("credentials/cakey.pem"), &identity.private_pem)?;
        std::fs::write(old.join("cover.png"), b"\x89PNG\r\n\x1a\ncover")?;
        std::fs::write(old.join("display-state.json"), b"saved display state")?;
        let next = temp.path().join(format!("imported {family}"));
        import(&old, &next)?;
        let config = Config::load(&next.join("sunshine.conf"))?;
        assert_eq!(config.values, Config::parse(settings)?.values);
        // ViGEmBus controller types load as the VHF pads that replace them.
        assert_eq!(config.get("gamepad", ""), "vhf_xbox_one");
        assert_eq!(
            crate::encoder_policy::canonical_name(config.get("encoder", "")),
            "amf"
        );
        for file in [
            "sunshine_state.json",
            "vibeshine_state.json",
            "sunshine_credentials.json",
            "credentials/cacert.pem",
            "credentials/cakey.pem",
            "display-state.json",
        ] {
            assert_eq!(
                std::fs::read(next.join(file))?,
                std::fs::read(old.join(file))?,
                "{family}: {file}"
            );
        }
        let clients = state::PairedState::load(&next.join("sunshine_state.json"))?;
        assert_eq!(clients.unique_id, "same-host");
        assert_eq!(clients.clients.len(), 1);
        assert_eq!(clients.clients[0].cert, identity.certificate);
        if family != "Sunshine" {
            assert_eq!(
                clients.clients[0].extra["config_overrides"],
                json!({"amd_quality":"speed","gamepad":"vhf_ds4"})
            );
        }
        assert!(
            state::Credentials::load(&next.join("sunshine_credentials.json"))?
                .unwrap()
                .verifies("admin", "existing-password")
        );
        let mut imported = state::load_json(&next.join("apps.json"), Value::Null)?;
        let cover = PathBuf::from(imported["apps"][0]["image-path"].as_str().unwrap());
        assert_eq!(std::fs::read(&cover)?, b"\x89PNG\r\n\x1a\ncover");
        imported["apps"][0]["image-path"] = apps["apps"][0]["image-path"].clone();
        assert_eq!(imported, apps);
        assert_eq!(std::fs::read(old.join("sunshine.conf"))?, utf16);
        assert!(import(&old, &next).is_err());
        std::fs::remove_file(old.join("credentials/cakey.pem"))?;
        let incomplete = temp.path().join(format!("incomplete {family}"));
        assert!(
            import(&old, &incomplete)
                .unwrap_err()
                .to_string()
                .contains("private key")
        );
        assert!(!incomplete.exists());
    }
    Ok(())
}
#[test]
fn an_apollo_sign_in_kept_in_the_paired_state_still_signs_in() -> Result<()> {
    // Apollo, Vibepollo and Sunshine keep the console sign-in in
    // sunshine_state.json unless credentials_file names another file,
    // hashed as SHA-256(password + salt) in C++ util::hex order.
    let temp = tempfile::tempdir()?;
    let identity = crypto::Identity::generate()?;
    let old = temp.path().join("Apollo/config");
    std::fs::create_dir_all(old.join("credentials"))?;
    std::fs::write(old.join("sunshine.conf"), "sunshine_name = Gaming PC\n")?;
    let salt = "aB3!%&()=-xYz012";
    state::write_json(
        &old.join("sunshine_state.json"),
        &json!({"username":"Ramazan","salt":salt,
            "password":crypto::legacy_hash(format!("Apollo pässword{salt}").as_bytes()),
            "root":{"uniqueid":"same-host","named_devices":[]}}),
    )?;
    std::fs::write(old.join("apps.json"), r#"{"env":{},"apps":[]}"#)?;
    std::fs::write(old.join("credentials/cacert.pem"), &identity.certificate)?;
    std::fs::write(old.join("credentials/cakey.pem"), &identity.private_pem)?;
    let next = temp.path().join("next");
    import(&old, &next)?;
    let files = state::ProfileFiles::new(&Config::load(&next.join("sunshine.conf"))?, &next);
    assert_eq!(files.credentials, next.join("sunshine_state.json"));
    let credentials = state::Credentials::load(&files.credentials)?.unwrap();
    assert!(credentials.verifies("ramazan", "Apollo pässword"));
    assert!(!credentials.verifies("ramazan", "apollo pässword"));
    Ok(())
}
#[test]
fn importing_keeps_source_intact_unknown_fields_and_external_files_owned() {
    let temp = tempfile::tempdir().unwrap();
    let old = temp.path().join("old");
    std::fs::create_dir(&old).unwrap();
    let apps = temp.path().join("apps.json");
    let original = br#"{"env":{"CUSTOM":"yes"},"apps":[],"future":{"keep":true}}"#;
    std::fs::write(&apps, original).unwrap();
    let conf = format!(
        "file_apps={}\nlog_path={}\nfuture_setting=unchanged\n",
        apps.display(),
        temp.path().join("old.log").display()
    );
    std::fs::write(old.join("sunshine.conf"), &conf).unwrap();
    let next = temp.path().join("rust");
    import(&old, &next).unwrap();
    assert_eq!(
        std::fs::read(old.join("sunshine.conf")).unwrap(),
        conf.as_bytes()
    );
    assert_eq!(std::fs::read(&apps).unwrap(), original);
    assert_eq!(std::fs::read(next.join("apps.json")).unwrap(), original);
    let config = Config::load(&next.join("sunshine.conf")).unwrap();
    assert_eq!(config.get("file_apps", ""), "apps.json");
    assert_eq!(config.get("future_setting", ""), "unchanged");
    assert!(import(&old, &next).is_err());
    assert!(import(&old, &old.join("inside")).is_err());
}
#[test]
fn configured_files_that_are_missing_fall_back_to_the_defaults() {
    let temp = tempfile::tempdir().unwrap();
    let old = temp.path().join("old");
    std::fs::create_dir_all(old.join("credentials")).unwrap();
    std::fs::write(old.join("apps.json"), br#"{"apps":[{"name":"Kept"}]}"#).unwrap();
    // The certificate is in its default place, its configured key is not.
    std::fs::write(old.join("credentials/cacert.pem"), "certificate").unwrap();
    let gone = temp.path().join("gone");
    std::fs::write(
        old.join("sunshine.conf"),
        format!(
            "file_state={0}\\state.json\nfile_apps={0}\\apps.json\ncredentials_file={0}\\credentials.json\ncert=credentials\\cacert.pem\npkey={0}\\cakey.pem\nvibeshine_file_state={0}\\vibeshine.json\n",
            gone.display()
        ),
    )
    .unwrap();
    let next = temp.path().join("next");
    import(&old, &next).unwrap();
    let config = Config::load(&next.join("sunshine.conf")).unwrap();
    for key in [
        "file_state",
        "file_apps",
        "credentials_file",
        "cert",
        "pkey",
    ] {
        assert!(!config.values.contains_key(key), "{key}");
    }
    assert_eq!(
        config.get("vibeshine_file_state", ""),
        "vibeshine_state.json"
    );
    let apps = state::load_json(&next.join("apps.json"), serde_json::json!({})).unwrap();
    assert_eq!(apps["apps"][0]["name"], "Kept");
    // Without its key the certificate is useless; the host makes a new pair.
    assert!(!next.join("credentials/cacert.pem").exists());
    assert!(old.join("credentials/cacert.pem").is_file());
}
#[test]
fn logs_and_oversized_files_and_covers_are_left_out() {
    let temp = tempfile::tempdir().unwrap();
    let old = temp.path().join("old");
    std::fs::create_dir_all(old.join("logs")).unwrap();
    let sized = |path: &Path, bytes: &[u8], size: u64| {
        let mut file = std::fs::File::create(path).unwrap();
        std::io::Write::write_all(&mut file, bytes).unwrap();
        file.set_len(size).unwrap();
    };
    sized(&old.join("logs/sunshine.log"), b"log", 70 * 1024 * 1024);
    sized(&old.join("dump.bin"), b"dump", 65 * 1024 * 1024);
    let cover = temp.path().join("huge.png");
    sized(&cover, b"\x89PNG\r\n\x1a\n", 17 * 1024 * 1024);
    let apps = serde_json::json!({"apps":[{"name":"Game","image-path":cover}]});
    std::fs::write(old.join("apps.json"), apps.to_string()).unwrap();
    std::fs::write(old.join("sunshine.conf"), "sunshine_name = PC\n").unwrap();
    let next = temp.path().join("next");
    import(&old, &next).unwrap();
    assert!(!next.join("logs").exists());
    assert!(!next.join("dump.bin").exists());
    let imported = state::load_json(&next.join("apps.json"), serde_json::json!({})).unwrap();
    assert_eq!(imported, apps);
    assert_eq!(
        Config::load(&next.join("sunshine.conf"))
            .unwrap()
            .get("sunshine_name", ""),
        "PC"
    );
}
#[test]
fn a_profile_the_host_would_refuse_fails_the_import() {
    let temp = tempfile::tempdir().unwrap();
    let cases: [&[(&str, &str)]; 6] = [
        &[("apps.json", r#"{"apps":{"future_format":true}}"#)],
        &[(
            "apps.json",
            r#"{"apps":[{"name":"Game","prep-cmd":"future_format"}]}"#,
        )],
        &[("vibeshine_state.json", r#"{"root":[]}"#)],
        &[(
            "vibeshine_state.json",
            r#"{"root":{"app_id_aliases":{"a":{"aliases":7}}}}"#,
        )],
        &[("sunshine_credentials.json", r#"{"username":"admin"}"#)],
        &[
            ("credentials/cacert.pem", "not a certificate"),
            ("credentials/cakey.pem", "not a key"),
        ],
    ];
    for (number, files) in cases.into_iter().enumerate() {
        let old = temp.path().join(format!("old{number}"));
        std::fs::create_dir_all(old.join("credentials")).unwrap();
        std::fs::write(old.join("sunshine.conf"), "sunshine_name = PC\n").unwrap();
        for (file, text) in files {
            std::fs::write(old.join(file), text).unwrap();
        }
        let next = temp.path().join(format!("next{number}"));
        let error = format!("{:#}", import(&old, &next).unwrap_err());
        assert!(error.contains("from starting"), "{error}");
        assert!(!next.exists());
    }
    assert!(!std::fs::read_dir(temp.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".butterpollo-import-")
    }));
}
#[test]
fn a_vibeshine_profile_imports_and_loads() {
    let temp = tempfile::tempdir().unwrap();
    let old = temp.path().join("config");
    std::fs::create_dir_all(old.join("logs")).unwrap();
    let library = temp.path().join("library");
    std::fs::create_dir(&library).unwrap();
    let cover = library.join("game.png");
    std::fs::write(&cover, b"\x89PNG\r\n\x1a\ncover").unwrap();
    let bom = |text: &str| [b"\xef\xbb\xbf".as_slice(), text.as_bytes()].concat();
    let uuid = "9b7c0b6e-1c1a-4f0e-9a43-1f5d2f9e8a10";
    let apps = json!({"env":{},"apps":[{
        "name":"Game","uuid":uuid,"cmd":"game.exe","image-path":cover,
        "detached":["steam://rungameid/1"],
        "prep-cmd":[
            {"do":"before.cmd","undo":"after.cmd","elevated":"true"},
            {"do":"","undo":"","elevated":"false"}
        ]
    }]});
    std::fs::write(library.join("apps.json"), bom(&apps.to_string())).unwrap();
    let host = "5a2f6a4e-3b1d-4c55-9a77-0d0c6a3c2b11";
    let state = json!({"root":{"uniqueid":host,"named_devices":""}});
    std::fs::write(old.join("sunshine_state.json"), bom(&state.to_string())).unwrap();
    let conf = format!(
        "# Vibeshine\r\nsunshine_name = \"Living room # TV\"\r\nfile_apps = {}\r\ncredentials_file = {}\r\n",
        library.join("apps.json").display(),
        temp.path().join("gone").join("credentials.json").display()
    );
    std::fs::write(old.join("sunshine.conf"), bom(&conf)).unwrap();
    std::fs::File::create(old.join("logs/sunshine.log"))
        .unwrap()
        .set_len(70 * 1024 * 1024)
        .unwrap();
    let next = temp.path().join("butterpollo");
    import(&old, &next).unwrap();
    check(&next).unwrap();
    assert!(!next.join("logs").exists());
    let config = Config::load(&next.join("sunshine.conf")).unwrap();
    assert_eq!(config.get("sunshine_name", ""), "\"Living room # TV\"");
    assert_eq!(config.get("file_apps", ""), "apps.json");
    assert!(!config.values.contains_key("credentials_file"));
    let paired = state::PairedState::load(&next.join("sunshine_state.json")).unwrap();
    assert_eq!(paired.unique_id, host);
    assert!(paired.clients.is_empty());
    let imported = state::load_json(&next.join("apps.json"), json!({})).unwrap();
    let apps: Vec<state::App> = serde_json::from_value(imported["apps"].clone()).unwrap();
    assert!(apps[0].prep[0].elevated && !apps[0].prep[1].elevated);
    assert_eq!(apps[0].extra["uuid"], uuid);
    assert_eq!(apps[0].extra["detached"], json!(["steam://rungameid/1"]));
    let image = PathBuf::from(apps[0].extra["image-path"].as_str().unwrap());
    assert!(image.starts_with(next.canonicalize().unwrap()));
}
#[test]
fn covers_survive_original_profile_removal_and_a_failed_import_rolls_back() {
    let temp = tempfile::tempdir().unwrap();
    let old = temp.path().join("old");
    std::fs::create_dir(&old).unwrap();
    let cover = old.join("game.png");
    let bytes = b"\x89PNG\r\n\x1a\nexisting-artwork-fixture";
    std::fs::write(&cover, bytes).unwrap();
    let apps = serde_json::json!({"apps":[{"name":"game","uuid":"same-identity","cmd":"original command","image-path":cover,"unknown":{"keep":true}}]});
    std::fs::write(old.join("apps.json"), serde_json::to_vec(&apps).unwrap()).unwrap();
    std::fs::write(old.join("sunshine.conf"), "future=keep\n").unwrap();
    let destination = temp.path().join("rust");
    import(&old, &destination).unwrap();
    let imported = state::load_json(&destination.join("apps.json"), serde_json::json!({})).unwrap();
    let owned = PathBuf::from(imported["apps"][0]["image-path"].as_str().unwrap());
    assert!(owned.starts_with(destination.canonicalize().unwrap()));
    std::fs::remove_dir_all(&old).unwrap();
    assert_eq!(std::fs::read(owned).unwrap(), bytes);
    assert_eq!(imported["apps"][0]["uuid"], "same-identity");
    assert_eq!(imported["apps"][0]["cmd"], "original command");
    assert_eq!(imported["apps"][0]["unknown"], apps["apps"][0]["unknown"]);
    std::fs::create_dir(&old).unwrap();
    // The credentials cannot be left out, and they cannot be copied.
    std::fs::File::create(old.join("credentials.json"))
        .unwrap()
        .set_len(65 * 1024 * 1024)
        .unwrap();
    std::fs::write(
        old.join("sunshine.conf"),
        "credentials_file=credentials.json\n",
    )
    .unwrap();
    let failed = temp.path().join("failed");
    assert!(import(&old, &failed).is_err());
    assert!(!failed.exists());
    assert!(!std::fs::read_dir(temp.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".butterpollo-import-")
    }));
}
