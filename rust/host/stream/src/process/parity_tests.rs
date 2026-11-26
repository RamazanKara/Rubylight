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
fn stopping_an_app_ends_its_state_commands_and_runs_undo() {
    let fixture = Fixture::new();
    let trace = fixture.host.directory.join("trace.txt");
    let ready = fixture.host.directory.join("ready.txt");
    let elevated = butterpollo_win_io::process::user_environment().is_err();
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
    // Pause and resume commands start detached as they come (c094b5c1c), so
    // the pause sent before the stop may already have run, in either order
    // with the undo; the running state command never gets to finish.
    let mut lines = log.lines().map(str::trim).collect::<Vec<_>>();
    let paused = lines.len();
    lines.retain(|line| *line != "queued-pause");
    assert!(paused - lines.len() <= 1, "{log}");
    assert_eq!(lines, ["prepared", "state", "undone"], "{log}");
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
        "psmap",
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
    let elevated = butterpollo_win_io::process::user_environment().is_err();
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
    assert_eq!(
        error.to_string(),
        "Commands: preparation command 3 ended with exit code 7. Check the app's and the global preparation commands in the host console."
    );
    assert_eq!(
        std::fs::read_to_string(&trace)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["global", "app", "undo-app", "undo-global"]
    );
}

#[test]
fn a_stopped_launch_whose_command_would_not_end_says_stopped_not_timed_out() {
    assert_eq!(wait_failure("Game", 0, true).to_string(), LAUNCH_STOPPED);
    assert_eq!(
        wait_failure("Game", 0, false).to_string(),
        "Game: preparation command 1 did not finish within 120 seconds. Check the app's and the global preparation commands in the host console."
    );
}

#[test]
fn a_stopped_launch_ends_its_running_preparation_command_and_undoes_the_finished_ones() {
    let elevated = butterpollo_win_io::process::user_environment().is_err();
    let f = Fixture::new();
    let trace = f.host.directory.join("commands.txt");
    let pid = f.host.directory.join("helper.pid");
    let command = |name: &str| format!("echo {name}>>\"{}\"", trace.display());
    let app: App = serde_json::from_value(json!({"name":"Commands","prep-cmd":[
        {"do":command("first"),"undo":command("undo-first"),"elevated":elevated},
        // The second leaves a helper running, which the stop ends with it.
        {"do":format!("{} & {}& ping -n 60 127.0.0.1 >nul", starts_helper(&pid), command("second")),"undo":command("must-not-run"),"elevated":elevated},
        {"do":command("must-not-run"),"undo":"","elevated":elevated}
    ]}))
    .unwrap();
    // The launch names itself, as `/launch` does under the launch lock; Quit then stops it.
    let started = f.host.launch_started("device");
    let cancel = f.host.launch_cancellation().unwrap();
    // Stopped while the second command runs, so the first has finished
    // however slowly `cmd` starts under load.
    let stopper = std::thread::spawn({
        let trace = trace.clone();
        let pid = pid.clone();
        move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            while !std::fs::read_to_string(&trace)
                .is_ok_and(|written| written.lines().any(|line| line == "second"))
            {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the second preparation command did not start within 30 s"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            // The pid file is complete before `second` is written, and the helper runs.
            let helper = Helper::from(&pid);
            assert!(!helper.ends_within(Duration::ZERO));
            let cancelled = std::time::Instant::now();
            cancel.cancel().unwrap();
            (cancelled, helper)
        }
    });
    let error = launch_with_environment(&f.host, &app, &HashMap::new(), Ok(BTreeMap::new()))
        .err()
        .unwrap();
    let ended = std::time::Instant::now();
    let (cancelled, helper) = stopper.join().unwrap();
    assert_eq!(error.to_string(), LAUNCH_STOPPED);
    // The launch ends soon after the stop: not the 60 s of the command, nor
    // its 120 s limit.
    assert!(ended.saturating_duration_since(cancelled) < Duration::from_secs(20));
    assert!(f.host.launch_stopped());
    assert_eq!(
        std::fs::read_to_string(&trace)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["first", "second", "undo-first"]
    );
    assert!(helper.ends_within(Duration::from_secs(10)));
    drop(started);
    assert!(f.host.launch_cancellation().is_none());
    assert!(!f.host.launch_stopped());
}

/// A command that starts a hidden `ping` (a helper that outlives the command), writes its pid
/// and creation time (100 ns since 1601) to `pid` and exits 0.
fn starts_helper(pid: &std::path::Path) -> String {
    format!(
        "powershell -NoProfile -NonInteractive -Command \"$p = Start-Process -FilePath ping.exe -ArgumentList '-n','60','127.0.0.1' -WindowStyle Hidden -PassThru; '{{0}} {{1}}' -f $p.Id, $p.StartTime.ToFileTimeUtc() | Set-Content -Encoding ascii -LiteralPath '{}'\"",
        pid.display()
    )
}
/// The helper `starts_helper` started, ended when the test is done whatever its outcome. It is
/// opened by its pid and creation time together, so a process that was given the pid after the
/// helper ended is never taken for it, nor ended.
struct Helper {
    handle: Option<windows::Win32::Foundation::HANDLE>,
    created: u64,
}
// SAFETY: a process handle may be used and closed from any thread.
unsafe impl Send for Helper {}
impl Helper {
    fn from(pid: &std::path::Path) -> Self {
        use windows::Win32::{
            Foundation::{CloseHandle, FILETIME},
            System::Threading::{
                GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
                PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
            },
        };
        let written = std::fs::read_to_string(pid).unwrap();
        let (pid, created) = written.trim().split_once(' ').unwrap();
        let (pid, created): (u32, u64) = (pid.parse().unwrap(), created.parse().unwrap());
        let access = PROCESS_SYNCHRONIZE | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION;
        // SAFETY: OpenProcess has no memory preconditions; the handle is closed here when it
        // names another process, otherwise in drop. It fails when the helper has already ended
        // and nothing holds its pid.
        let handle = unsafe { OpenProcess(access, false, pid) }
            .ok()
            .filter(|&handle| {
                let mut times = [FILETIME::default(); 4];
                let [start, exit, kernel, user] = &mut times;
                // SAFETY: the handle is open and each out-pointer is a local FILETIME.
                let read = unsafe { GetProcessTimes(handle, start, exit, kernel, user) };
                let same = read.is_ok()
                    && (u64::from(times[0].dwHighDateTime) << 32
                        | u64::from(times[0].dwLowDateTime))
                        == created;
                if !same {
                    // SAFETY: the handle is open and closed once, here.
                    let _ = unsafe { CloseHandle(handle) };
                }
                same
            });
        Self { handle, created }
    }
    /// Whether it ended (or was already gone) within `timeout`.
    fn ends_within(&self, timeout: Duration) -> bool {
        use windows::Win32::{Foundation::WAIT_OBJECT_0, System::Threading::WaitForSingleObject};
        // SAFETY: the handle is open until drop.
        self.handle.is_none_or(|handle| unsafe {
            WaitForSingleObject(handle, timeout.as_millis() as u32) == WAIT_OBJECT_0
        })
    }
}
impl Drop for Helper {
    fn drop(&mut self) {
        use windows::Win32::{
            Foundation::CloseHandle,
            System::Threading::{TerminateProcess, WaitForSingleObject},
        };
        if let Some(handle) = self.handle.take() {
            // SAFETY: the handle was opened with PROCESS_TERMINATE and is closed once, here.
            unsafe {
                let _ = TerminateProcess(handle, 1);
                WaitForSingleObject(handle, 5000);
                let _ = CloseHandle(handle);
            }
        }
    }
}

#[test]
fn a_program_a_preparation_command_left_running_is_never_taken_for_the_game() {
    // The helper is a fullscreen window on the stream's display, created after the launch
    // began but before the app's own command: it is not a game another program started.
    use butterpollo_session::steam::Process;
    let elevated = butterpollo_win_io::process::user_environment().is_err();
    let f = Fixture::new();
    let pid = f.host.directory.join("helper.pid");
    let app: App = serde_json::from_value(json!({"name":"Helper","prep-cmd":[
        {"do":starts_helper(&pid),"undo":"","elevated":elevated}
    ]}))
    .unwrap();
    let mut running =
        launch_with_environment(&f.host, &app, &HashMap::new(), Ok(BTreeMap::new())).unwrap();
    let helper = Helper::from(&pid);
    assert!(!helper.ends_within(Duration::ZERO));
    running.launches = true;
    let check = running.observe_foreground(99_999, "PING.EXE").unwrap();
    let created = helper.created;
    let verdict = check.run_with(
        |_| created,
        || {
            Ok(vec![Process {
                pid: 99_999,
                parent: 99_998,
                started: created,
                name: "PING.EXE".into(),
            }])
        },
    );
    assert_eq!(verdict, ForegroundVerdict::NotForeign);
}
#[test]
fn what_a_finished_preparation_command_started_keeps_running() {
    // Issue 135: a boot-screen helper the preparation command started must still run when the
    // app's own command looks for it.
    let elevated = butterpollo_win_io::process::user_environment().is_err();
    let f = Fixture::new();
    let pid = f.host.directory.join("helper.pid");
    let app: App = serde_json::from_value(json!({"name":"Helper","prep-cmd":[
        {"do":starts_helper(&pid),"undo":"","elevated":elevated}
    ]}))
    .unwrap();
    let running =
        launch_with_environment(&f.host, &app, &HashMap::new(), Ok(BTreeMap::new())).unwrap();
    let helper = Helper::from(&pid);
    assert!(!helper.ends_within(Duration::from_secs(2)));
    drop(running);
    assert!(!helper.ends_within(Duration::from_millis(500)));
}

#[test]
fn what_a_failed_preparation_command_started_is_ended_with_it() {
    // Only a command that ended with exit code 0 leaves its programs running: a failed one
    // cancels the launch and its undo is never queued, so nothing else would end the helper.
    let elevated = butterpollo_win_io::process::user_environment().is_err();
    let f = Fixture::new();
    let pid = f.host.directory.join("helper.pid");
    let failing = starts_helper(&pid).strip_suffix('"').unwrap().to_owned() + "; exit 1\"";
    let app: App = serde_json::from_value(json!({"name":"Helper","prep-cmd":[
        {"do":failing,"undo":"","elevated":elevated}
    ]}))
    .unwrap();
    let error = launch_with_environment(&f.host, &app, &HashMap::new(), Ok(BTreeMap::new()))
        .err()
        .expect("the failed preparation command fails the launch");
    let helper = Helper::from(&pid);
    assert!(
        error.to_string().contains("ended with exit code 1"),
        "{error:#}"
    );
    assert!(helper.ends_within(Duration::from_secs(5)));
}

#[test]
fn what_a_finished_undo_command_started_keeps_running() {
    // `undo: start "" app.exe` brings a program back after the stream.
    let elevated = butterpollo_win_io::process::user_environment().is_err();
    let f = Fixture::new();
    let pid = f.host.directory.join("helper.pid");
    let app: App = serde_json::from_value(json!({"name":"Helper","prep-cmd":[
        {"do":"","undo":starts_helper(&pid),"elevated":elevated}
    ]}))
    .unwrap();
    let running =
        launch_with_environment(&f.host, &app, &HashMap::new(), Ok(BTreeMap::new())).unwrap();
    drop(running);
    let helper = Helper::from(&pid);
    assert!(!helper.ends_within(Duration::from_secs(2)));
}

#[test]
fn application_executable_paths_preserve_shell_environment_expansion() {
    let elevated = butterpollo_win_io::process::user_environment().is_err();
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
    let elevated = butterpollo_win_io::process::user_environment().is_err();
    let drives = butterpollo_win_io::files::drives();
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
    let elevated = butterpollo_win_io::process::user_environment().is_err();
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

#[test]
fn stopping_an_app_undoes_its_menu_commands_once_last_run_first() {
    let fixture = Fixture::new();
    let trace = fixture.host.directory.join("menu-undo.txt");
    let elevated = butterpollo_win_io::process::user_environment().is_err();
    let undo = |name: &str| format!("echo {name}>>\"{}\"", trace.display());
    let app: App = serde_json::from_value(json!({
        "name": "Menu commands",
        "menu-cmd": [
            {"name": "First", "cmd": "rem first", "undo": undo("undo-first"), "elevated": elevated},
            {"name": "Second", "cmd": "rem second", "undo": undo("undo-second"), "elevated": elevated},
            {"name": "Never run", "cmd": "rem never", "undo": undo("undo-never"), "elevated": elevated},
            {"name": "No undo", "cmd": "rem plain", "elevated": elevated}
        ]
    }))
    .unwrap();
    let mut running = RunningApp::with_environment(&app, BTreeMap::new()).unwrap();
    // The menu command itself is started by the control worker; here only its
    // record counts.
    assert!(running.menu_command(0).is_some());
    assert!(running.menu_command(1).is_some());
    assert!(running.menu_command(0).is_some());
    assert!(running.menu_command(3).is_some());
    assert!(running.menu_command(9).is_none());
    running.stop();
    let log = std::fs::read_to_string(&trace).unwrap();
    let lines = log.lines().map(str::trim).collect::<Vec<_>>();
    assert_eq!(lines, ["undo-first", "undo-second"], "{log}");
    // A second stop (the drop) undoes nothing again.
    running.stop();
    assert_eq!(std::fs::read_to_string(&trace).unwrap(), log);
}

#[test]
fn connection_commands_report_the_display_size_not_the_capped_picture() {
    // A 4K request under a 1440p host cap: the device's display runs at 3840x2160
    // while the encoder codes 2560x1440.
    let mut stream = butterpollo_media_policy::rtsp::Negotiated {
        width: 2560,
        height: 1440,
        hdr: true,
        ..Default::default()
    };
    stream.set_display_size((3840, 2160), Some((2560, 1440)));
    assert!(stream.capped());
    let environment = client_stream_environment(&stream, "60".into());
    assert_eq!(environment["SUNSHINE_CLIENT_WIDTH"], "3840");
    assert_eq!(environment["SUNSHINE_CLIENT_HEIGHT"], "2160");
    assert_eq!(environment["SUNSHINE_CLIENT_FPS"], "60");
    assert_eq!(environment["SUNSHINE_CLIENT_HDR"], "true");
    // Without a cap the request is the display size, as before.
    let mut plain = stream.clone();
    plain.set_display_size((3840, 2160), None);
    let environment = client_stream_environment(&plain, "60".into());
    assert_eq!(environment["SUNSHINE_CLIENT_WIDTH"], "3840");
    assert_eq!(environment["SUNSHINE_CLIENT_HEIGHT"], "2160");
}
