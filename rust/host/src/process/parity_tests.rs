use super::*;
use crate::state::test_support::Fixture;
use serde_json::json;

#[test]
fn background_mode_survives_a_deferred_launch() {
    let fixture = Fixture::new();
    let app: App = serde_json::from_value(json!({"name":"Background"})).unwrap();
    for background in [false, true] {
        let args = HashMap::from([(
            BACKGROUND_ARG.into(),
            if background { "1" } else { "0" }.into(),
        )]);
        for environment in [
            Ok(BTreeMap::new()),
            Err(anyhow::anyhow!("no signed-in user")),
        ] {
            let deferred = environment.is_err();
            let mut app = app.clone();
            if deferred {
                app.cmd = "game.exe".into();
            }
            let running = launch_with_environment(&fixture.host, &app, &args, environment).unwrap();
            assert_eq!(running.background, background);
            assert_eq!(running.deferred.is_some(), deferred);
            if let Some((_, saved_args)) = running.deferred.as_ref() {
                assert_eq!(saved_args, &args);
            }
        }
    }
}

/// A launch checks Neural Rendering's files first and fails loudly when they
/// are gone or the switch is busy with the game.
#[test]
fn a_launch_fails_when_neural_rendering_is_broken_or_busy() {
    let fixture = Fixture::new();
    // In the temp folder, under `%LOCALAPPDATA%`: other games' anti-cheat
    // there (BattlEye's own folder) doesn't refuse this one.
    let folder = fixture.host.directory.join("Game");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("Game.exe"), "MZ").unwrap();
    let app: App = serde_json::from_value(json!({"name": "Game", "uuid": "game",
        crate::neural::KEY: {"exe": folder.join("Game.exe")}}))
    .unwrap();
    let launch =
        || launch_with_environment(&fixture.host, &app, &HashMap::new(), Ok(BTreeMap::new()));
    let error = format!("{:#}", launch().err().unwrap());
    assert!(error.contains("files are gone"), "{error}");
    crate::neural::start_job(&fixture.host, "game", "remove", "remove").unwrap();
    let error = format!("{:#}", launch().err().unwrap());
    assert!(error.contains("busy"), "{error}");
}

#[test]
fn background_controller_environment_wins_over_saved_environment() {
    let fixture = Fixture::new();
    fixture.host.app_document.write().unwrap()["env"] =
        json!({"SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS":"0"});
    let app: App = serde_json::from_value(json!({"name":"Background"})).unwrap();
    for background in [false, true] {
        let value = if background { "1" } else { "0" };
        let args = HashMap::from([(BACKGROUND_ARG.into(), value.into())]);
        let running =
            launch_with_environment(&fixture.host, &app, &args, Ok(BTreeMap::new())).unwrap();
        assert_eq!(
            running.environment["SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS"],
            value
        );
        assert_eq!(
            fixture.host.app_document.read().unwrap()["env"]["SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS"],
            "0"
        );
    }
}

#[test]
fn close_on_disconnect_inherits_the_global_default_and_preserves_app_overrides() {
    let fixture = Fixture::new();
    for global in [None, Some(false), Some(true)] {
        let mut config = fixture.host.config.write().unwrap();
        config.values.remove("terminate_on_pause");
        if let Some(global) = global {
            config
                .values
                .insert("terminate_on_pause".into(), global.to_string());
        }
        drop(config);
        for local in [
            None,
            Some(json!(true)),
            Some(json!(false)),
            Some(json!("yes")),
            Some(json!("false")),
            Some(json!("1")),
            Some(json!("0")),
            Some(json!(null)),
        ] {
            let expected = match local.as_ref() {
                Some(serde_json::Value::Bool(value)) => *value,
                Some(serde_json::Value::String(value)) => {
                    matches!(value.as_str(), "yes" | "1")
                }
                _ => global.unwrap_or(false),
            };
            for signed_in in [false, true] {
                let mut app: App = serde_json::from_value(json!({"name":"Game"})).unwrap();
                if let Some(local) = &local {
                    app.extra.insert("terminate-on-pause".into(), local.clone());
                }
                if !signed_in {
                    app.cmd = "must wait for sign-in".into();
                }
                let saved = serde_json::to_value(&app).unwrap();
                let environment = if signed_in {
                    Ok(BTreeMap::new())
                } else {
                    Err(anyhow::anyhow!("no signed-in user"))
                };
                let mut running =
                    launch_with_environment(&fixture.host, &app, &HashMap::new(), environment)
                        .unwrap();
                assert!(!running.connection_state(false), "not connected yet");
                assert!(!running.connection_state(true));
                assert!(!running.connection_state(true), "still connected");
                assert_eq!(running.connection_state(false), expected);
                assert!(!running.connection_state(false), "already disconnected");
                assert_eq!(serde_json::to_value(&app).unwrap(), saved);
                if !signed_in {
                    assert_eq!(
                        serde_json::to_value(&running.deferred.as_ref().unwrap().0).unwrap(),
                        saved
                    );
                }
            }
        }
    }
}

#[test]
fn stopping_an_app_cancels_running_and_queued_state_commands_before_undo() {
    let fixture = Fixture::new();
    let trace = fixture.host.directory.join("trace.txt");
    let ready = fixture.host.directory.join("ready.txt");
    let elevated = butterpollo_windows::process::user_environment().is_err();
    let app: App = serde_json::from_value(json!({
        "name": "State command cancellation",
        "prep-cmd": [{"do": format!("echo prepared>\"{}\"", trace.display()),
            "undo": format!("echo undone>>\"{}\"", trace.display()), "elevated": elevated}],
        "state-cmd": [{"do": format!("echo state>>\"{}\" & echo ready>\"{}\" & ping -n 120 127.0.0.1 >nul & echo stale>>\"{}\"", trace.display(), ready.display(), trace.display()),
            "undo": format!("echo queued-pause>>\"{}\"", trace.display()), "elevated": elevated}]
    })).unwrap();
    let mut running = RunningApp::with_environment(&app, BTreeMap::new()).unwrap();
    running.connection_state(true);
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !ready.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "state command did not start"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    running.connection_state(false);
    running.connection_state(true);
    running.stop();
    let log = std::fs::read_to_string(trace).unwrap();
    assert_eq!(
        log.lines().map(str::trim).collect::<Vec<_>>(),
        ["prepared", "state", "undone"]
    );
    assert!(running.state_events.is_none());
}

#[test]
fn before_sign_in_app_commands_are_deferred_and_the_requesting_device_keeps_ownership() {
    let f = Fixture::new();
    let app: App = serde_json::from_value(json!({"name":"Deferred","cmd":"must not execute",
        "prep-cmd":[{"do":"must not prepare","undo":"must not undo"}],
        "detached":["must not detach"],"state-cmd":[{"do":"must not run","undo":""}]}))
    .unwrap();
    let args = HashMap::from([
        ("clientUuid".into(), "owner".into()),
        ("clientName".into(), "Phone".into()),
    ]);
    let running = launch_with_environment(
        &f.host,
        &app,
        &args,
        Err(anyhow::anyhow!("no signed-in user")),
    )
    .unwrap();
    assert!(running.child.is_none());
    assert!(running.undo.is_empty());
    assert!(running.state_events.is_none());
    assert_eq!(running.owner, "owner");
    assert_eq!(running.environment["APOLLO_CLIENT_UUID"], "owner");
    assert_eq!(running.environment["APOLLO_CLIENT_NAME"], "Phone");
    assert!(!running.environment.contains_key("USERPROFILE"));
    assert!(!running.environment.contains_key("TEMP"));
    let (deferred, options) = running.deferred.as_ref().unwrap();
    assert_eq!(
        serde_json::to_value(deferred).unwrap(),
        serde_json::to_value(&app).unwrap()
    );
    assert_eq!(options, &args);
}

#[test]
fn launch_environment_refuses_client_shell_syntax() {
    let f = Fixture::new();
    let app: App = serde_json::from_value(json!({"name":"Game"})).unwrap();
    for key in [
        "gcmap",
        "scaleFactor",
        "surroundParams",
        "surroundAudioInfo",
    ] {
        let args = HashMap::from([(key.into(), "Phone\" & whoami".into())]);
        assert!(
            launch_with_environment(&f.host, &app, &args, Ok(BTreeMap::new())).is_err(),
            "{key}"
        );
    }
}

#[test]
fn launch_takes_seven_one_four_sent_as_a_signed_java_int() {
    let f = Fixture::new();
    let app: App = serde_json::from_value(json!({"name":"Game"})).unwrap();
    // Moonlight V+ (and Nightjar's 7.1.4 before it sent the unsigned form) builds
    // 0xF63F << 16 | 12 as a Java int.
    let args = HashMap::from([("surroundAudioInfo".into(), "-163643380".into())]);
    let running = launch_with_environment(&f.host, &app, &args, Ok(BTreeMap::new())).unwrap();
    assert_eq!(
        running.environment["SUNSHINE_CLIENT_AUDIO_CONFIGURATION"],
        "-163643380"
    );
    // /launch's audio route and the stream's encoder both see 12 channels, not stereo.
    assert_eq!(
        crate::stream::audio_encoder::surround_channels(Some("-163643380")),
        12
    );
    let options = BTreeMap::from_iter(args);
    assert_eq!(crate::stream::audio_encoder::launch_channels(&options), 12);
    // Out of 32 bits, or not a number at all, still fails the launch.
    for value in ["4294967308", "-2147483649", "12a", ""] {
        let args = HashMap::from([("surroundAudioInfo".into(), value.into())]);
        assert!(
            launch_with_environment(&f.host, &app, &args, Ok(BTreeMap::new())).is_err(),
            "{value}"
        );
    }
}

#[test]
fn app_environment_expands_apollo_variables_and_preserves_fractional_rates_and_saved_apps() {
    let f = Fixture::new();
    let app: App =
        serde_json::from_value(json!({"name":"Game","uuid":"game-id","scale-factor":150})).unwrap();
    let original = serde_json::to_value(&app).unwrap();
    *f.host.app_document.write().unwrap() = json!({"env":{"GAME_DATA":"$(base)/data"},"apps":[]});
    let args = HashMap::from([
        ("mode".into(), "1920x1080x59940".into()),
        ("clientName".into(), "Phone".into()),
        ("clientUuid".into(), "phone-id".into()),
        ("scaleFactor".into(), "200".into()),
        ("hdrMode".into(), "1".into()),
        ("surroundAudioInfo".into(), "6".into()),
        ("localAudioPlayMode".into(), "1".into()),
    ]);
    let running = launch_with_environment(
        &f.host,
        &app,
        &args,
        Ok(BTreeMap::from([("BASE".into(), "fixture".into())])),
    )
    .unwrap();
    for (key, expected) in [
        ("GAME_DATA", "fixture/data"),
        ("APOLLO_APP_UUID", "game-id"),
        ("APOLLO_APP_NAME", "Game"),
        ("APOLLO_CLIENT_NAME", "Phone"),
        ("APOLLO_CLIENT_UUID", "phone-id"),
        ("APOLLO_CLIENT_WIDTH", "2880"),
        ("APOLLO_CLIENT_HEIGHT", "1620"),
        ("APOLLO_CLIENT_RENDER_WIDTH", "1920"),
        ("APOLLO_CLIENT_RENDER_HEIGHT", "1080"),
        ("APOLLO_CLIENT_SCALE_FACTOR", "200"),
        ("APOLLO_CLIENT_FPS", "59.940"),
        ("APOLLO_CLIENT_HDR", "true"),
        ("APOLLO_CLIENT_HOST_AUDIO", "true"),
        ("APOLLO_CLIENT_AUDIO_CONFIGURATION", "6"),
        ("APOLLO_APP_STATUS", "STARTING"),
    ] {
        assert_eq!(running.environment[key], expected, "{key}");
    }
    assert_eq!(running.owner, "phone-id");
    assert_eq!(serde_json::to_value(&app).unwrap(), original);
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("envvar_compatibility_mode".into(), "true".into());
    let running = launch_with_environment(&f.host, &app, &args, Ok(BTreeMap::new())).unwrap();
    assert_eq!(running.environment["APOLLO_CLIENT_FPS"], "60");
}

#[test]
fn previously_paired_names_keep_streaming_with_safe_command_values() {
    let f = Fixture::new();
    let app: App = serde_json::from_value(json!({"name":"Desktop"})).unwrap();
    for (name, expected) in [
        ("Phone (Living room)", "Phone _Living room_"),
        ("John’s iPhone", "John_s iPhone"),
        ("Phone\" & whoami", "Phone_ _ whoami"),
    ] {
        let args = HashMap::from([("clientName".into(), name.into())]);
        let running = launch_with_environment(&f.host, &app, &args, Ok(BTreeMap::new())).unwrap();
        assert_eq!(running.environment["SUNSHINE_CLIENT_NAME"], expected);
        assert_eq!(running.environment["APOLLO_CLIENT_NAME"], expected);
        assert_eq!(args["clientName"], name);
    }
}

#[test]
fn preparation_undo_and_failed_preparation_keep_global_order_and_rollback() {
    let elevated = butterpollo_windows::process::user_environment().is_err();
    let f = Fixture::new();
    let trace = f.host.directory.join("commands.txt");
    let command = |name: &str| format!("echo {name}>>\"{}\"", trace.display());
    f.host.config.write().unwrap().values.insert(
        "global_prep_cmd".into(),
        json!([
            {"do":command("global"),"undo":command("undo-global"),"elevated":elevated}
        ])
        .to_string(),
    );
    let mut app: App = serde_json::from_value(json!({"name":"Commands","prep-cmd":[
        {"do":command("app"),"undo":command("undo-app"),"elevated":elevated}
    ]}))
    .unwrap();
    let running =
        launch_with_environment(&f.host, &app, &HashMap::new(), Ok(BTreeMap::new())).unwrap();
    assert_eq!(
        std::fs::read_to_string(&trace)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["global", "app"]
    );
    drop(running);
    assert_eq!(
        std::fs::read_to_string(&trace)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["global", "app", "undo-app", "undo-global"]
    );
    std::fs::write(&trace, "").unwrap();
    app.prep.push(PrepCommand {
        r#do: "exit /b 7".into(),
        undo: command("must-not-run"),
        elevated,
        extra: Default::default(),
    });
    let error = launch_with_environment(&f.host, &app, &HashMap::new(), Ok(BTreeMap::new()))
        .err()
        .unwrap();
    assert_eq!(error.to_string(), "preparation command 3 exited with 7");
    assert_eq!(
        std::fs::read_to_string(&trace)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["global", "app", "undo-app", "undo-global"]
    );
}

#[test]
fn application_executable_paths_preserve_shell_environment_expansion() {
    let elevated = butterpollo_windows::process::user_environment().is_err();
    let system_root = std::env::var("SystemRoot").unwrap();
    let app: App = serde_json::from_value(json!({
        "name":"Shell expansion",
        "cmd":format!("\"{system_root}\\%RUBYLIGHT_TEST_SYSTEM32%\\cmd.exe\" /d /c exit 0"),
        "elevated":elevated
    }))
    .unwrap();
    let running = RunningApp::with_environment(
        &app,
        BTreeMap::from([("RUBYLIGHT_TEST_SYSTEM32".into(), "System32".into())]),
    )
    .unwrap();
    assert_eq!(
        running
            .child
            .as_ref()
            .unwrap()
            .wait(Duration::from_secs(10))
            .unwrap(),
        0
    );
}

#[test]
fn programs_on_drives_the_host_cannot_see_start_with_the_users_token() {
    // As the service, the host runs as SYSTEM and does not see a drive the
    // signed-in user mapped; the app itself starts with the user's token.
    let elevated = butterpollo_windows::process::user_environment().is_err();
    let drives = butterpollo_windows::files::drives();
    let unused = (b'D'..=b'Z')
        .map(char::from)
        .find(|letter| !drives.contains(&format!("{letter}:\\")))
        .expect("a free drive letter");
    let app: App = serde_json::from_value(json!({
        "name":"Mapped drive",
        "cmd":format!("\"{unused}:\\Games\\game.exe\""),
        "elevated":elevated
    }))
    .unwrap();
    // The launch goes ahead; cmd.exe reports the program missing when the
    // user cannot see it either. Starting a user-token process can still fail
    // in an elevated non-SYSTEM test process, which is not what this checks.
    match RunningApp::with_environment(&app, BTreeMap::new()) {
        Ok(running) => assert_ne!(
            running
                .child
                .as_ref()
                .unwrap()
                .wait(Duration::from_secs(10))
                .unwrap(),
            0
        ),
        Err(error) => assert!(
            !format!("{error:#}").contains("application executable not found"),
            "{error:#}"
        ),
    }
}

#[test]
fn working_directory_inference_uses_only_absolute_programs_with_existing_parents() {
    let f = Fixture::new();
    let folder = f.host.directory.join("A game");
    std::fs::create_dir(&folder).unwrap();
    let command = format!("\"{}\" --windowed", folder.join("game.exe").display());
    assert_eq!(inferred_working_dir(&command), folder.to_string_lossy());
    for command in ["game.exe --windowed", "steam://rungameid/570", ""] {
        assert_eq!(inferred_working_dir(command), "");
    }
    assert_eq!(
        inferred_working_dir(&format!(
            "\"{}\"",
            folder.join("missing/game.exe").display()
        )),
        ""
    );
}

#[test]
fn detached_app_commands_run_with_the_supplied_environment() {
    let elevated = butterpollo_windows::process::user_environment().is_err();
    let f = Fixture::new();
    let output = f.host.directory.join("detached.txt");
    let app: App =
        serde_json::from_value(json!({"name":"Detached","elevated":elevated,"detached":[
            format!("echo $(APOLLO_APP_NAME)>\"{}\"", output.display())
        ]}))
        .unwrap();
    let running =
        launch_with_environment(&f.host, &app, &HashMap::new(), Ok(BTreeMap::new())).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if std::fs::read_to_string(&output).is_ok_and(|text| text.trim() == "Detached") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "detached command did not finish"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(running);
}
