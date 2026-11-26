use super::*;
use butterpollo_config::config::Config;
use butterpollo_stream::state::{Host, test_support::Fixture};
use butterpollo_win_graphics::capture::{Display, Gpu};

fn hybrid_gpus() -> Vec<Gpu> {
    vec![
        Gpu {
            name: "AMD Radeon(TM) 780M Graphics".into(),
            vendor: 0x1002,
            dedicated_memory: 512 * 1024 * 1024,
            device: 0,
            luid: (1, 0),
            pnp_id: Some("PCI\\VEN_1002&DEV_15BF\\1".into()),
            preference: None,
        },
        Gpu {
            name: "AMD Radeon RX 7600M XT".into(),
            vendor: 0x1002,
            dedicated_memory: 8 * 1024 * 1024 * 1024,
            device: 0,
            luid: (2, 0),
            pnp_id: None,
            preference: None,
        },
    ]
}

fn integrated_display() -> Vec<Display> {
    vec![Display {
        device_id: r"\\.\DISPLAY1".into(),
        display_name: r"\\.\DISPLAY1".into(),
        friendly_name: "Laptop panel".into(),
        width: 1920,
        height: 1080,
        x: 0,
        y: 0,
        primary: true,
        adapter: "AMD Radeon(TM) 780M Graphics".into(),
        adapter_index: 0,
        output_index: 0,
    }]
}

#[test]
fn metadata_lists_render_only_gpus_independently_of_display_adapters() {
    let fixture = Fixture::new();
    let value = metadata(
        &fixture.host,
        || Ok(hybrid_gpus()),
        || Ok(integrated_display()),
    )
    .unwrap();
    assert_eq!(value["status"], true);
    assert_eq!(value["gpus"], json!(hybrid_gpus()));
    assert!(value["gpu_error"].is_null());
    assert_eq!(
        value["capture_status"]["displays"],
        json!(integrated_display())
    );
    assert!(value["capture_status"]["error"].is_null());
    let cached = metadata(
        &fixture.host,
        || panic!("a cached request must not enumerate GPUs again"),
        || panic!("a cached request must not enumerate displays again"),
    )
    .unwrap();
    assert_eq!(cached, value);
}

#[test]
fn metadata_keeps_gpu_and_display_enumeration_failures_separate() {
    let fixture = Fixture::new();
    let missing_displays = metadata(
        &fixture.host,
        || Ok(hybrid_gpus()),
        || anyhow::bail!("display enumeration failed"),
    )
    .unwrap();
    assert_eq!(missing_displays["gpus"], json!(hybrid_gpus()));
    assert!(missing_displays["gpu_error"].is_null());
    assert!(missing_displays["capture_status"]["displays"].is_null());
    assert_eq!(
        missing_displays["capture_status"]["error"],
        "display enumeration failed"
    );

    fixture.host.metadata.lock().unwrap().take();
    let missing_gpus = metadata(
        &fixture.host,
        || anyhow::bail!("GPU enumeration failed"),
        || Ok(integrated_display()),
    )
    .unwrap();
    assert!(missing_gpus["gpus"].is_null());
    assert_eq!(missing_gpus["gpu_error"], "GPU enumeration failed");
    assert_eq!(
        missing_gpus["capture_status"]["displays"],
        json!(integrated_display())
    );
    assert!(missing_gpus["capture_status"]["error"].is_null());
}

#[test]
fn metadata_distinguishes_empty_gpu_inventory_from_an_enumeration_failure() {
    let fixture = Fixture::new();
    let value = metadata(&fixture.host, || Ok(vec![]), || Ok(vec![])).unwrap();
    assert_eq!(value["gpus"], json!([]));
    assert!(value["gpu_error"].is_null());
    assert_eq!(value["capture_status"]["displays"], json!([]));
    assert!(value["capture_status"]["error"].is_null());
}

#[test]
fn concurrent_configuration_patches_preserve_both_updates() {
    let fixture = Fixture::new();
    let barrier = std::sync::Barrier::new(2);
    let patches = [
        json!({"sunshine_name": "Concurrent host"}),
        json!({"locale": "de"}),
    ];
    std::thread::scope(|scope| {
        let workers: Vec<_> = patches
            .iter()
            .map(|patch| {
                let barrier = &barrier;
                let host = &fixture.host;
                scope.spawn(move || {
                    barrier.wait();
                    handle(host, "PATCH", "/api/config", patch).unwrap();
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
    });
    let saved = Config::load(&fixture.host.config_path).unwrap();
    assert_eq!(saved.get("sunshine_name", ""), "Concurrent host");
    assert_eq!(saved.get("locale", ""), "de");
    assert_eq!(fixture.host.config.read().unwrap().values, saved.values);
}

/// Full chroma applies when saved: answering every save with a restart
/// showed "Some changes apply after the host restarts" for it.
#[test]
fn only_a_save_with_a_restart_bound_setting_asks_for_a_restart() {
    let fixture = Fixture::new();
    let restart = |patch: Value| {
        handle(&fixture.host, "PATCH", "/api/config", &patch).unwrap()["restart_required"]
            .as_bool()
            .unwrap()
    };
    assert!(!restart(json!({"packed_444": false})));
    // Under its earlier name too.
    assert!(!restart(json!({"experimental_packed_444": false})));
    // HEVC support changes the codecs only the startup probe advertises.
    assert!(restart(json!({"hevc_mode": "1"})));
    assert!(restart(json!({"packed_444": false, "av1_mode": "1"})));
    assert!(restart(json!({"sunshine_name": "Renamed host"})));
    // Read where they are used: the updater's next check, the console's language.
    assert!(!restart(json!({"updates": "install"})));
    assert!(!restart(json!({"locale": "de", "updates": "off"})));
    // The listeners bind their ports once.
    assert!(restart(json!({"port": "48100", "locale": "en"})));
}

/// The restart list names each setting once, by the name a save is compared
/// under.
#[test]
fn restart_keys_are_current_names_listed_once() {
    let mut seen = std::collections::BTreeSet::new();
    for key in RESTART_KEYS {
        assert_eq!(butterpollo_config::config::current_name(key), *key);
        assert!(seen.insert(*key), "{key} twice");
    }
}

/// The console's save reaches the offered packed modes: turning full
/// chroma off withdraws them before the save answers.
#[test]
fn saving_full_chroma_off_withdraws_its_modes_at_once() {
    use std::sync::atomic::Ordering;
    let fixture = Fixture::new();
    let host = &fixture.host;
    host.config
        .write()
        .unwrap()
        .values
        .insert("packed_444".into(), "true".into());
    host.packed_modes
        .store(rubylight_protocol::packed444::ALL_MODES, Ordering::Release);
    handle(
        host,
        "PATCH",
        "/api/config",
        &json!({"experimental_packed_444": false}),
    )
    .unwrap();
    assert_eq!(host.packed_modes.load(Ordering::Acquire), 0);
}

#[test]
fn saving_a_library_setting_scans_the_launchers_again() {
    use butterpollo_session::native::Scan;
    let fixture = Fixture::new();
    let host = &fixture.host;
    let seeded = Scan::new(["epic:kept".to_owned()]);
    butterpollo_stream::library::seed(host, seeded.clone());
    handle(host, "PATCH", "/api/config", &json!({"locale": "de"})).unwrap();
    assert_eq!(
        butterpollo_stream::library::installed(host),
        Some(seeded.clone()),
        "other settings keep the scan"
    );
    handle(
        host,
        "PATCH",
        "/api/config",
        &json!({"library_epic_enabled": false}),
    )
    .unwrap();
    // Tests scan nothing, so the new scan replaces the seeded one with an empty one.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while butterpollo_stream::library::installed(host) == Some(seeded.clone()) {
        assert!(
            std::time::Instant::now() < deadline,
            "the library setting did not start a new scan"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn the_stream_preset_round_trips_without_saving_the_values_it_fills() {
    let fixture = Fixture::new();
    let host = &fixture.host;
    let shown = handle(host, "GET", "/api/config", &Value::Null).unwrap();
    assert!(shown.get("stream_preset").is_none());
    handle(
        host,
        "PATCH",
        "/api/config",
        &json!({"stream_preset": "reference"}),
    )
    .unwrap();
    let shown = handle(host, "GET", "/api/config", &Value::Null).unwrap();
    assert_eq!(shown["stream_preset"], "reference");
    // The preset's values are not written to the file, so changing the
    // preset later changes them; only the name is saved.
    assert!(shown.get("amd_max_frame_size").is_none());
    let saved = Config::load(&host.config_path).unwrap();
    assert_eq!(saved.get("stream_preset", ""), "reference");
    assert!(!saved.values.contains_key("amd_max_frame_size"));
    // A manual value saves next to it and stays after the preset changes.
    handle(
        host,
        "PATCH",
        "/api/config",
        &json!({"amd_max_frame_size": "2", "stream_preset": "lowest_latency"}),
    )
    .unwrap();
    let saved = Config::load(&host.config_path).unwrap();
    assert_eq!(saved.get("stream_preset", ""), "lowest_latency");
    assert_eq!(saved.get("amd_max_frame_size", ""), "2");
    // An unknown name is refused and the saved one stays.
    assert!(
        handle(
            host,
            "PATCH",
            "/api/config",
            &json!({"stream_preset": "fastest"})
        )
        .is_err()
    );
    let saved = Config::load(&host.config_path).unwrap();
    assert_eq!(saved.get("stream_preset", ""), "lowest_latency");
    // Clearing it means Balanced again.
    handle(
        host,
        "PATCH",
        "/api/config",
        &json!({"stream_preset": null}),
    )
    .unwrap();
    let saved = Config::load(&host.config_path).unwrap();
    assert!(!saved.values.contains_key("stream_preset"));
    assert_eq!(saved.get("amd_max_frame_size", ""), "2");
}

#[test]
fn the_console_gets_settings_saved_under_an_earlier_name_under_the_current_one() {
    let fixture = Fixture::new();
    std::fs::write(
        &fixture.host.config_path,
        "rtss_path = D:\\RTSS\nnvenc_force_split_encode = forced\nrtx_hdr_peak_brightness = 1200\nfuture_setting_extra = kept\n",
    )
    .unwrap();
    let host = Host::load(
        fixture.host.directory.clone(),
        fixture.host.assets.clone(),
        None,
    )
    .unwrap();
    let shown = handle(&host, "GET", "/api/config", &Value::Null).unwrap();
    assert_eq!(shown["hdr_peak_brightness"], 1200);
    assert_eq!(shown["rtss_install_path"], "D:\\RTSS");
    assert_eq!(shown["nvenc_split_encode"], "forced");
    assert_eq!(shown["future_setting_extra"], "kept");
    for old in [
        "rtx_hdr_peak_brightness",
        "rtss_path",
        "nvenc_force_split_encode",
    ] {
        assert!(shown.get(old).is_none(), "{old}");
    }
    // Loading leaves the file alone; the console's first save writes the
    // current names, so changing the setting in the console changes it,
    // and a later reset cannot bring back the earlier value.
    let saved = std::fs::read_to_string(&host.config_path).unwrap();
    assert!(saved.contains("nvenc_force_split_encode"), "{saved}");
    handle(
        &host,
        "PATCH",
        "/api/config",
        &json!({"nvenc_split_encode": "disabled"}),
    )
    .unwrap();
    let saved = std::fs::read_to_string(&host.config_path).unwrap();
    assert!(!saved.contains("nvenc_force_split_encode"), "{saved}");
    let reloaded = Config::load(&host.config_path).unwrap();
    assert_eq!(reloaded.get("nvenc_split_encode", ""), "disabled");
    assert_eq!(reloaded.integer("hdr_peak_brightness", 0), 1200);
    handle(
        &host,
        "PATCH",
        "/api/config",
        &json!({"nvenc_split_encode": null}),
    )
    .unwrap();
    let reset = Config::load(&host.config_path).unwrap();
    assert!(!reset.values.contains_key("nvenc_split_encode"));
}

#[test]
fn the_permanent_virtual_display_settings_are_dropped_without_a_warning() {
    let fixture = Fixture::new();
    std::fs::write(
        &fixture.host.config_path,
        "dd_vdd_static_monitor_count = 2\ndd_virtual_display_permanent_count = 1\nfuture_setting_extra = kept\n",
    )
    .unwrap();
    let host = Host::load(
        fixture.host.directory.clone(),
        fixture.host.assets.clone(),
        None,
    )
    .unwrap();
    let names = [
        "dd_vdd_static_monitor_count",
        "dd_virtual_display_permanent_count",
    ];
    let shown = handle(&host, "GET", "/api/config", &Value::Null).unwrap();
    for key in names {
        assert!(shown.get(key).is_none(), "{key}");
    }
    assert_eq!(shown["future_setting_extra"], "kept");
    let warnings = host.warnings.snapshot();
    assert!(warnings.is_empty(), "{warnings:?}");
    // The next save leaves them out, also when the console still sends one.
    handle(
        &host,
        "PATCH",
        "/api/config",
        &json!({"dd_virtual_display_permanent_count": 3, "nvenc_split_encode": "disabled"}),
    )
    .unwrap();
    let saved = std::fs::read_to_string(&host.config_path).unwrap();
    for key in names {
        assert!(!saved.contains(key), "{saved}");
    }
    assert!(saved.contains("future_setting_extra = kept"), "{saved}");
    assert!(saved.contains("nvenc_split_encode = disabled"), "{saved}");
}

#[test]
fn turning_usb_forwarding_off_in_the_console_detaches_attached_devices() {
    let fixture = Fixture::new();
    let host = &fixture.host;
    host.usb
        .set_vhci(butterpollo_stream::usb_forward::test_support::installed());
    let attach = |request_id| {
        host.usb.on_message(
            "session-1",
            &butterpollo_stream::usb_forward::test_support::client(true),
            true,
            4,
            rubylight_protocol::usb::UsbMessage::AttachRequest {
                request_id,
                vid: 0x0781,
                pid: 0x5581,
                class: 8,
                speed: 3,
                busid: "1-1".into(),
                name: "Flash drive".into(),
            },
        )
    };
    handle(
        host,
        "PATCH",
        "/api/config",
        &json!({"usb_forwarding": true}),
    )
    .unwrap();
    attach(1);
    // Other settings leave the device attached.
    handle(host, "PATCH", "/api/config", &json!({"locale": "de"})).unwrap();
    assert_eq!(host.usb.list().len(), 1);
    handle(
        host,
        "PATCH",
        "/api/config",
        &json!({"usb_forwarding": false}),
    )
    .unwrap();
    assert!(host.usb.list().is_empty());
    attach(2);
    save_setting(host, "locale", |_| Ok("en".into())).unwrap();
    assert!(host.usb.list().is_empty(), "a page's own save ends it too");
}

#[test]
fn saving_configuration_does_not_persist_the_command_line_port() {
    let fixture = Fixture::new();
    std::fs::write(&fixture.host.config_path, "port = 48000\n").unwrap();
    let host = Host::load(
        fixture.host.directory.clone(),
        fixture.host.assets.clone(),
        Some(48518),
    )
    .unwrap();
    assert_eq!(host.config.read().unwrap().port().unwrap(), 48518);
    let mut saved = handle(&host, "GET", "/api/config", &Value::Null).unwrap();
    assert_eq!(saved["port"], 48000);
    saved["sunshine_name"] = "Test host".into();
    handle(&host, "POST", "/api/config", &saved).unwrap();
    assert_eq!(
        Config::load(&host.config_path).unwrap().port().unwrap(),
        48000
    );
    assert_eq!(host.config.read().unwrap().port().unwrap(), 48518);
    saved["port"] = "48100".into();
    handle(&host, "POST", "/api/config", &saved).unwrap();
    assert_eq!(
        Config::load(&host.config_path).unwrap().port().unwrap(),
        48100
    );
    assert_eq!(host.config.read().unwrap().port().unwrap(), 48518);
}

#[test]
fn the_steamgriddb_key_is_saved_for_administrators_only_never_shown_and_the_mask_keeps_it() {
    let fixture = Fixture::new();
    let host = &fixture.host;
    let key = "0123456789abcdef0123456789abcdef";
    let file = host
        .directory
        .join("credentials")
        .join("steamgriddb_api_key");
    let in_conf = || {
        Config::load(&host.config_path)
            .unwrap()
            .values
            .contains_key("steamgriddb_api_key")
    };
    let shown = handle(host, "GET", "/api/config", &Value::Null).unwrap();
    assert!(shown.get("steamgriddb_api_key").is_none(), "unset");
    handle(
        host,
        "PATCH",
        "/api/config",
        &json!({"steamgriddb_api_key": key}),
    )
    .unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), key);
    assert!(
        !in_conf(),
        "never in sunshine.conf, which every local user reads"
    );
    assert!(
        !std::fs::read_to_string(&host.config_path)
            .unwrap()
            .contains(key)
    );
    let shown = handle(host, "GET", "/api/config", &Value::Null).unwrap();
    assert_eq!(shown["steamgriddb_api_key"], "********");
    assert!(!shown.to_string().contains(key), "{shown}");
    assert_eq!(
        butterpollo_stream::steamgriddb::api_key(host).as_deref(),
        Some(key)
    );
    // The console sends the mask back with other settings: the key stays.
    handle(
        host,
        "PATCH",
        "/api/config",
        &json!({"steamgriddb_api_key": "********", "sunshine_name": "Den"}),
    )
    .unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), key);
    // A key that is not one is refused without repeating it.
    let error = handle(
        host,
        "PATCH",
        "/api/config",
        &json!({"steamgriddb_api_key": "not a key; X 1"}),
    )
    .unwrap_err()
    .to_string();
    assert!(!error.contains("not a key"), "{error}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), key);
    // A key written into sunshine.conf by hand is read and masked, and a save moves it.
    std::fs::remove_file(&file).unwrap();
    host.config
        .write()
        .unwrap()
        .values
        .insert("steamgriddb_api_key".into(), "fedcba9876543210".into());
    let shown = handle(host, "GET", "/api/config", &Value::Null).unwrap();
    assert_eq!(shown["steamgriddb_api_key"], "********");
    assert_eq!(
        butterpollo_stream::steamgriddb::api_key(host).as_deref(),
        Some("fedcba9876543210")
    );
    handle(
        host,
        "PATCH",
        "/api/config",
        &json!({"steamgriddb_api_key": key}),
    )
    .unwrap();
    assert!(
        !host
            .config
            .read()
            .unwrap()
            .values
            .contains_key("steamgriddb_api_key")
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), key);
    // Emptied, it is gone.
    handle(
        host,
        "PATCH",
        "/api/config",
        &json!({"steamgriddb_api_key": ""}),
    )
    .unwrap();
    assert!(!file.exists());
    assert!(!in_conf());
    let shown = handle(host, "GET", "/api/config", &Value::Null).unwrap();
    assert!(shown.get("steamgriddb_api_key").is_none());
}

/// A settings save that fails must not change the SteamGridDB key: the console
/// would report an error after the change had already taken effect.
#[test]
fn a_failed_settings_save_keeps_the_older_steamgriddb_key() {
    let fixture = Fixture::new();
    let host = &fixture.host;
    let key = "0123456789abcdef0123456789abcdef";
    let file = host
        .directory
        .join("credentials")
        .join("steamgriddb_api_key");
    // The configuration write cannot land on a directory.
    let _ = std::fs::remove_file(&host.config_path);
    std::fs::create_dir(&host.config_path).unwrap();
    let saved = handle(
        host,
        "PATCH",
        "/api/config",
        &json!({"steamgriddb_api_key": key}),
    );
    assert!(saved.is_err(), "the configuration did not save");
    assert!(
        !file.exists(),
        "the key was stored although the settings did not save"
    );
}

/// The settings are already written when the key cannot be stored: they must
/// apply, and the key failure is a warning rather than losing the whole save.
#[test]
fn a_key_store_failure_after_a_saved_configuration_is_a_warning() {
    let fixture = Fixture::new();
    let host = &fixture.host;
    let key = "0123456789abcdef0123456789abcdef";
    let file = host
        .directory
        .join("credentials")
        .join("steamgriddb_api_key");
    std::fs::create_dir(&file).unwrap();
    let saved = handle(
        host,
        "PATCH",
        "/api/config",
        &json!({"steamgriddb_api_key": key, "sunshine_name": "Den"}),
    )
    .unwrap();
    assert_eq!(saved["status"], true);
    let warning = saved["warning"].as_str().expect("a key warning");
    assert!(warning.contains("SteamGridDB"), "{warning}");
    assert!(warning.contains("Enter it again"), "{warning}");
    assert_eq!(
        host.config.read().unwrap().get("sunshine_name", ""),
        "Den",
        "the saved settings still applied"
    );
    assert!(file.is_dir(), "the key was not stored");
}

/// An emptied key that cannot be removed from disk is still in use; say that
/// rather than claim the settings did not save.
#[test]
fn a_key_removal_failure_says_the_key_is_still_in_use() {
    let fixture = Fixture::new();
    let host = &fixture.host;
    let file = host
        .directory
        .join("credentials")
        .join("steamgriddb_api_key");
    std::fs::create_dir(&file).unwrap();
    let saved = handle(
        host,
        "PATCH",
        "/api/config",
        &json!({"steamgriddb_api_key": "", "sunshine_name": "Den"}),
    )
    .unwrap();
    assert_eq!(saved["status"], true);
    let warning = saved["warning"].as_str().expect("a key warning");
    assert!(warning.contains("could not be removed"), "{warning}");
    assert!(warning.contains("still in use"), "{warning}");
    assert_eq!(host.config.read().unwrap().get("sunshine_name", ""), "Den");
    assert!(file.is_dir(), "the key file could not be removed");
}

#[test]
fn any_save_moves_a_typed_steamgriddb_key_and_any_key_save_ends_a_refusal() {
    let fixture = Fixture::new();
    let host = &fixture.host;
    let key = "0123456789abcdef0123456789abcdef";
    let file = host
        .directory
        .join("credentials")
        .join("steamgriddb_api_key");
    host.config
        .write()
        .unwrap()
        .values
        .insert("steamgriddb_api_key".into(), key.into());
    butterpollo_stream::steamgriddb::AS_SYSTEM.with(|system| system.set(true));
    // A save of another setting, or of the mask, moves the typed key out of sunshine.conf.
    handle(
        host,
        "PATCH",
        "/api/config",
        &json!({"steamgriddb_api_key": "********", "sunshine_name": "Den"}),
    )
    .unwrap();
    butterpollo_stream::steamgriddb::AS_SYSTEM.with(|system| system.set(false));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), key);
    assert!(
        !Config::load(&host.config_path)
            .unwrap()
            .values
            .contains_key("steamgriddb_api_key")
    );
    assert!(
        !host
            .config
            .read()
            .unwrap()
            .values
            .contains_key("steamgriddb_api_key")
    );

    // Saving the same key again ends a refusal and takes its warning back, as emptying does.
    for value in [key, ""] {
        butterpollo_stream::steamgriddb::refuse(host, key);
        handle(
            host,
            "PATCH",
            "/api/config",
            &json!({"steamgriddb_api_key": value}),
        )
        .unwrap();
        assert!(!butterpollo_stream::steamgriddb::refused(host), "{value:?}");
        assert!(
            !host
                .warnings
                .snapshot()
                .iter()
                .any(|warning| warning.code == butterpollo_stream::steamgriddb::REFUSED_WARNING),
            "{value:?}"
        );
    }
}
