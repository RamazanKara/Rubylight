use super::*;
/// A game's devices go back with every process it started, which game-only
/// audio routed too, and nothing older that only shares a pid.
#[test]
fn a_games_processes_are_the_game_and_its_children() {
    let process = |pid, parent, started| butterpollo_core::steam::Process {
        pid,
        parent,
        started,
        name: "game.exe".into(),
    };
    let processes = [
        process(10, 1, 100),
        process(11, 10, 101),
        process(12, 11, 102),
        process(13, 10, 50),
        process(20, 1, 100),
    ];
    let owned = BTreeMap::from([(10, 100), (12, 102)]);
    assert_eq!(game_trees(&owned, &processes), [10, 11, 12]);
}
#[test]
fn swapped_settings_files_wait_for_the_game_not_for_its_launcher_or_shell() {
    assert!(watches_program("D:\\Games\\Racer\\Racer.exe"));
    for program in [
        "C:\\Windows\\System32\\cmd.exe",
        "C:\\Program Files (x86)\\Steam\\steam.exe",
        "C:\\Program Files (x86)\\Epic Games\\Launcher\\Portal\\Binaries\\Win64\\EpicGamesLauncher.exe",
        "C:\\Windows\\explorer.exe",
    ] {
        assert!(!watches_program(program), "{program}");
    }
}
/// Issue #82: a game started through Playnite is no process of the app's, so none was
/// recorded for its swapped settings files and every stream of one ended with the "no
/// process was seen" warning.
#[test]
fn a_playnite_games_processes_are_recorded_for_its_settings_files() {
    let f = crate::state::test_support::Fixture::new();
    let folder = f.host.directory.join("game");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("Settings.ini"), "pc").unwrap();
    let app: App = serde_json::from_value(serde_json::json!({
        "name": "Game",
        "uuid": "game-1",
        "game-settings-files": ["%GameDir%\\Settings.ini"],
    }))
    .unwrap();
    f.host.apps.write().unwrap().push(app.clone());
    let environment =
        BTreeMap::from([("GAMEDIR".to_owned(), folder.to_string_lossy().into_owned())]);
    let args = HashMap::from([
        ("clientUuid".to_owned(), "tv".to_owned()),
        ("clientName".to_owned(), "tv name".to_owned()),
    ]);
    let mut running = RunningApp::with_environment(&app, environment.clone()).unwrap();
    running.settings_swap =
        crate::game_settings::launching(&f.host, &app, &args, &environment).unwrap();
    assert!(running.settings_swap.is_some());
    running.playnite = Some(crate::playnite::Launch::with_game_processes(vec![
        butterpollo_core::game_settings::Watched {
            pid: 7171,
            started: 9,
            program: folder.join("Game.exe").to_string_lossy().into_owned(),
        },
    ]));
    running.stop();
    assert!(
        !f.host
            .warnings
            .snapshot()
            .iter()
            .any(|warning| warning.code == crate::game_settings::UNWATCHED_WARNING),
        "the game was seen"
    );
}
#[test]
fn an_apps_exit_timeout_overrides_the_hosts_and_the_hosts_overrides_ten_seconds() {
    use butterpollo_core::config::Config;
    let app = |extra: serde_json::Value| -> App {
        let mut value = serde_json::json!({"name": "Game"});
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_value(value).unwrap()
    };
    let plain = app(serde_json::json!({}));
    let own = app(serde_json::json!({"exit-timeout": 3}));
    let host = Config::parse("exit_timeout = 5\n").unwrap();
    assert_eq!(
        exit_timeout(&plain, &Config::default()),
        Duration::from_secs(10)
    );
    assert_eq!(exit_timeout(&plain, &host), Duration::from_secs(5));
    assert_eq!(exit_timeout(&own, &host), Duration::from_secs(3));
    assert_eq!(
        exit_timeout(&app(serde_json::json!({"exit-timeout": 0})), &host),
        Duration::ZERO,
        "0 ends the app at once"
    );
    assert_eq!(
        exit_timeout(&plain, &Config::parse("exit_timeout = 900\n").unwrap()),
        Duration::from_secs(300)
    );
    assert_eq!(
        exit_timeout(&plain, &Config::parse("exit_timeout = -4\n").unwrap()),
        Duration::ZERO
    );
}
#[test]
fn state_commands_start_without_waiting_and_a_failed_start_does_not_stop_the_rest() {
    let command = |r#do: &str, undo: &str| PrepCommand {
        r#do: r#do.into(),
        undo: undo.into(),
        ..serde_json::from_str("{}").unwrap()
    };
    let commands = [
        command("resume.ahk", "pause.ahk"),
        command("", "broken"),
        command("resume $(APOLLO_APP_STATUS)", "after"),
    ];
    let mut environment = BTreeMap::new();
    environment.insert("APOLLO_APP_STATUS".to_string(), "PAUSING".to_string());
    let mut started = Vec::new();
    start_state_commands(&commands, false, &environment, |cmd, _| {
        started.push(cmd.to_string());
        if cmd == "broken" {
            bail!("cannot start")
        } else {
            Ok(())
        }
    });
    assert_eq!(started, ["pause.ahk", "broken", "after"]);
    environment.insert("APOLLO_APP_STATUS".to_string(), "RESUMING".to_string());
    started.clear();
    start_state_commands(&commands, true, &environment, |cmd, _| {
        started.push(cmd.to_string());
        Ok(())
    });
    assert_eq!(started, ["resume.ahk", "resume RESUMING"]);
}
#[test]
fn early_exit_notification_requires_auto_detach_and_a_nonzero_exit_before_five_seconds() {
    let early = Duration::from_millis(4999);
    assert_eq!(
        exit_action(true, early, Some(1), false),
        ExitAction::Detached(Some(1))
    );
    assert_eq!(
        exit_action(true, early, Some(0xc0000005), false),
        ExitAction::Detached(Some(0xc0000005))
    );
    assert_eq!(
        exit_action(true, early, Some(0), false),
        ExitAction::Detached(None)
    );
    assert_eq!(
        exit_action(false, early, Some(1), false),
        ExitAction::Finished
    );
    assert_eq!(
        exit_action(true, Duration::from_secs(5), Some(1), false),
        ExitAction::Finished
    );
    assert_eq!(
        exit_action(true, Duration::from_secs(6), Some(1), false),
        ExitAction::Finished
    );
    assert_eq!(exit_action(true, early, None, false), ExitAction::Running);
    assert_eq!(exit_action(true, early, Some(1), true), ExitAction::Running);
    assert_eq!(exit_action(true, early, Some(0), true), ExitAction::Running);
    assert_eq!(
        exit_action(true, early, Some(9009), false),
        ExitAction::Finished
    );
}

#[test]
fn a_missing_executable_fails_instead_of_detaching_a_desktop_stream() {
    let mut app = App::desktop();
    app.cmd = format!(
        "\"{}\"",
        std::env::temp_dir()
            .join(format!("missing-{}.exe", uuid::Uuid::new_v4()))
            .display()
    );
    let result = RunningApp::with_environment(&app, BTreeMap::new());
    assert!(
        result
            .err()
            .unwrap()
            .to_string()
            .contains("application executable not found")
    );
}

#[test]
fn a_slow_steam_launch_keeps_its_process_tracker() {
    let mut app = RunningApp::with_environment(&App::desktop(), BTreeMap::new()).unwrap();
    app.steam = Some(butterpollo_core::steam::Tracker::new(
        &[],
        "C:\\not-a-game",
        Duration::ZERO,
    ));
    app.steam_check = std::time::Instant::now() - Duration::from_secs(2);
    assert!(app.wants_processes());
    let processes = butterpollo_windows::process::processes().unwrap();
    let paths = app
        .unlooked(&processes)
        .into_iter()
        .map(|pid| (pid, butterpollo_windows::process::image_path(pid)))
        .collect();
    assert!(!app.exited(Some(&ProcessScan { processes, paths })).unwrap());
    assert!(app.steam.is_some());
    // Looked once: the next look is a second away.
    assert!(!app.wants_processes());
}
#[test]
fn quitting_closes_a_store_launched_game_but_not_the_store_or_older_programs() {
    let launched = 1_000;
    let game = r"C:\XboxGames\Forza Horizon 5\Content\ForzaHorizon5.exe";
    assert!(foreign_game(game, launched + 1, launched));
    // A program that was already running before the launch stays open.
    assert!(!foreign_game(game, launched - 1, launched));
    // So does a process whose creation time cannot be read.
    assert!(!foreign_game(game, 0, launched));
    for client in [
        r"C:\Program Files (x86)\Steam\steam.exe",
        r"C:\Program Files (x86)\Epic Games\Launcher\Portal\Binaries\Win64\EpicGamesLauncher.exe",
        r"C:\Program Files\Butterpollo\butterpollo.exe",
    ] {
        assert!(!foreign_game(client, launched + 1, launched), "{client}");
    }
}
/// An app that runs `ping` for a while, so its job has a process to name.
fn app_with_a_child() -> RunningApp {
    let mut app = App::desktop();
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    app.cmd = format!(r#""{root}\System32\PING.EXE" -n 30 127.0.0.1"#);
    RunningApp::with_environment(&app, BTreeMap::new()).unwrap()
}

#[test]
fn a_process_of_the_apps_own_job_is_the_game_and_an_unrelated_one_is_not() {
    let mut app = app_with_a_child();
    assert_eq!(app.launch_progress().map(|(up, _)| up), Some(false));
    let owned = app.child.as_ref().unwrap().process_ids().unwrap();
    assert!(!owned.is_empty());
    // A window of some other program in front says nothing about the game.
    app.observe_foreground(std::process::id(), "notepad.exe");
    assert_eq!(app.launch_progress().map(|(up, _)| up), Some(false));
    // A launcher's own window is never the game, even from the job.
    app.observe_foreground(owned[0], "steamwebhelper.exe");
    assert_eq!(app.launch_progress().map(|(up, _)| up), Some(false));
    // The job's program in front is the game.
    app.observe_foreground(owned[0], "game.exe");
    assert_eq!(app.launch_progress().map(|(up, _)| up), Some(true));
    // And once seen it stays seen.
    app.observe_foreground(std::process::id(), "notepad.exe");
    assert_eq!(app.launch_progress().map(|(up, _)| up), Some(true));
}

#[test]
fn the_launcher_watch_skips_launcher_apps_and_ends_with_its_limit() {
    let app = |cmd: &str, detached: &[&str]| {
        let mut app = App::desktop();
        app.cmd = cmd.into();
        app.extra
            .insert("detached".into(), serde_json::json!(detached));
        app
    };
    // The default Steam Big Picture app: the launcher is the app.
    assert!(only_opens_launchers(&app("", &["steam://open/bigpicture"])));
    assert!(only_opens_launchers(&app(
        r"C:\Program Files (x86)\Steam\steam.exe",
        &[]
    )));
    // A game by Steam, a program of its own, or a mix of the two is waited for.
    assert!(!only_opens_launchers(&app("", &["steam://rungameid/570"])));
    assert!(!only_opens_launchers(&app(r"C:\Games\game.exe", &[])));
    assert!(!only_opens_launchers(&app(
        r"C:\Games\game.exe",
        &["steam://open/bigpicture"]
    )));
    assert!(!only_opens_launchers(&app("", &[])));

    // Nothing is started here: only the watch's own state is looked at.
    let mut running = RunningApp::with_environment(&App::desktop(), BTreeMap::new()).unwrap();
    running.launches = true;
    assert!(running.launch_progress().is_some());
    running.launcher_only = true;
    assert_eq!(running.launch_progress(), None);
    running.launcher_only = false;
    running.started = std::time::Instant::now()
        - butterpollo_core::launcher_prompt::WATCH_LIMIT
        - Duration::from_secs(1);
    assert_eq!(
        running.launch_progress(),
        None,
        "a watch that ran out stops"
    );
}

#[test]
fn existing_environment_syntax_is_case_insensitive_and_checked() {
    let env = BTreeMap::from([("PATH".into(), "C:\\tools".into())]);
    assert_eq!(
        expand("$(Path);$$;$(MISSING);$literal$", &env).unwrap(),
        "C:\\tools;$;;$literal$"
    );
    assert!(expand("$(Path", &env).is_err());
}
#[test]
fn a_program_the_desktop_started_is_the_users_not_the_apps() {
    use butterpollo_core::steam::Process;
    let process = |pid, parent, started, name: &str| Process {
        pid,
        parent,
        started,
        name: name.into(),
    };
    let processes = [
        process(10, 1, 100, "explorer.exe"),
        process(20, 10, 500, "player.exe"),
        process(30, 40, 500, "game.exe"),
        process(40, 1, 200, "EpicGamesLauncher.exe"),
        // Parent id 10 reused after the child started: not its parent.
        process(50, 60, 300, "old.exe"),
        process(60, 1, 900, "explorer.exe"),
    ];
    assert!(started_from_desktop(20, &processes));
    assert!(!started_from_desktop(30, &processes));
    assert!(!started_from_desktop(50, &processes));
    assert!(!started_from_desktop(99, &processes));
}
#[test]
fn a_store_started_game_is_found_by_a_check_handed_out_one_at_a_time() {
    use butterpollo_core::steam::Process;
    let mut app = RunningApp::with_environment(&App::desktop(), BTreeMap::new()).unwrap();
    app.launches = true;
    let launched = app.launched_at;
    let process = |pid, parent, started, name: &str| Process {
        pid,
        parent,
        started,
        name: name.into(),
    };
    let processes = || {
        Ok(vec![
            process(10, 1, 100, "explorer.exe"),
            process(40, 1, 200, "EpicGamesLauncher.exe"),
            process(20, 10, launched + 1, "player.exe"),
            process(30, 40, launched + 2, "game.exe"),
        ])
    };
    // The check is handed out, not run: the process list is not read here.
    let check = app.observe_foreground(30, "game.exe").unwrap();
    // One at a time: nothing more while it runs.
    assert!(app.observe_foreground(20, "player.exe").is_none());
    let verdict = check.run_with(|_| launched + 2, processes);
    app.finish_foreground(check, verdict);
    assert_eq!(app.foreign.get(&30), Some(&(launched + 2)));
    assert_eq!(app.launch_progress().map(|(up, _)| up), Some(true));
    assert!(app.observe_foreground(30, "game.exe").is_none());
    // A program the user opened from the desktop is theirs, and not looked at again.
    let check = app.observe_foreground(20, "player.exe").unwrap();
    let verdict = check.run_with(|_| launched + 1, processes);
    app.finish_foreground(check, verdict);
    assert!(!app.foreign.contains_key(&20));
    assert!(app.observe_foreground(20, "player.exe").is_none());
    // Another program under that id is.
    assert!(app.observe_foreground(20, "other.exe").is_some());
}
#[test]
fn a_deferred_start_keeps_its_placeholders_session_and_where_the_game_is() {
    let mut placeholder = RunningApp::with_environment(&App::desktop(), BTreeMap::new()).unwrap();
    placeholder.generation = "launch".into();
    placeholder.display = Some("phone".into());
    let mut started = RunningApp::with_environment(&App::desktop(), BTreeMap::new()).unwrap();
    started.take_place_of(&mut placeholder, "tv".into());
    assert_eq!(started.generation, "launch");
    assert_eq!(started.owner, "tv");
    assert_eq!(started.display.as_deref(), Some("phone"));
}
#[test]
fn a_display_leaving_brings_back_only_a_game_with_processes_to_name() {
    // Desktop or a detached command: nothing to bring back, and no error to log.
    let desktop = RunningApp::with_environment(&App::desktop(), BTreeMap::new()).unwrap();
    assert!(desktop.is_desktop());
    assert!(desktop.departure_windows().unwrap().is_empty());
    let mut detached = RunningApp::with_environment(&App::desktop(), BTreeMap::new()).unwrap();
    detached.launches = true;
    assert!(!detached.is_desktop());
    assert!(detached.departure_windows().unwrap().is_empty());
    // A game Steam follows, this test's own process standing in for it: its
    // windows may come back, but not a background game's. It is never dropped:
    // stopping the app would end this test's own process.
    let me = std::process::id();
    let started = butterpollo_windows::process::processes()
        .unwrap()
        .into_iter()
        .find(|process| process.pid == me)
        .unwrap()
        .started;
    let mut game = std::mem::ManuallyDrop::new(
        RunningApp::with_environment(&App::desktop(), BTreeMap::new()).unwrap(),
    );
    let mut tracker = butterpollo_core::steam::Tracker::new(&[], "C:\\not-a-game", Duration::ZERO);
    tracker.tracked.insert(me, started);
    game.steam = Some(tracker);
    assert!(!game.is_desktop());
    assert!(game.departure_windows().unwrap().contains(&me));
    game.background = true;
    assert!(game.departure_windows().unwrap().is_empty());
}
#[test]
fn a_foreground_check_for_a_replaced_app_is_not_applied_to_its_successor() {
    let mut app = RunningApp::with_environment(&App::desktop(), BTreeMap::new()).unwrap();
    app.launches = true;
    let check = app.observe_foreground(30, "game.exe").unwrap();
    let created = app.launched_at + 1;
    let verdict = check.run_with(
        |_| created,
        || {
            Ok(vec![butterpollo_core::steam::Process {
                pid: 30,
                parent: 40,
                started: created,
                name: "game.exe".into(),
            }])
        },
    );
    assert_eq!(verdict, ForegroundVerdict::Foreign(created));
    let mut successor = RunningApp::with_environment(&App::desktop(), BTreeMap::new()).unwrap();
    successor.launches = true;
    successor.foreground_check = Some(77);
    successor.finish_foreground(check, verdict);
    assert!(successor.foreign.is_empty());
    assert_eq!(successor.foreground_check, Some(77));
}
#[test]
fn a_foreground_check_records_nothing_for_a_program_gone_or_replaced_under_its_id() {
    use butterpollo_core::steam::Process;
    let mut app = RunningApp::with_environment(&App::desktop(), BTreeMap::new()).unwrap();
    app.launches = true;
    let created = app.launched_at + 5;
    let list = |name: &str, started| {
        let name = name.to_owned();
        move || {
            Ok(vec![
                Process {
                    pid: 1,
                    parent: 0,
                    started: 1,
                    name: "launcher.exe".into(),
                },
                Process {
                    pid: 30,
                    parent: 1,
                    started,
                    name,
                },
            ])
        }
    };
    type List = Box<dyn FnOnce() -> Result<Vec<Process>>>;
    let cases: [(&str, u64, List); 5] = [
        // Gone before the check ran: no creation time.
        ("gone", 0, Box::new(list("game.exe", created))),
        // Gone from the list.
        ("missing", created, Box::new(|| Ok(vec![]))),
        // The id now names an unrelated program, which quitting must not close.
        ("reused", created, Box::new(list("notepad.exe", created))),
        // The same name, a later process under a reused id.
        (
            "restarted",
            created,
            Box::new(list("game.exe", created + 1)),
        ),
        ("unreadable", created, Box::new(|| anyhow::bail!("no list"))),
    ];
    for (case, time, processes) in cases {
        let check = app.observe_foreground(30, r"C:\Games\game.exe").unwrap();
        let verdict = check.run_with(|_| time, processes);
        assert_eq!(verdict, ForegroundVerdict::Retry, "{case}");
        app.finish_foreground(check, verdict);
        assert!(app.foreign.is_empty(), "{case}");
        assert!(app.not_foreign.is_empty(), "{case}");
        assert_eq!(app.foreground_check, None, "{case}");
    }
    // The program itself, still running under its id, is the game.
    let check = app.observe_foreground(30, r"C:\Games\game.exe").unwrap();
    let verdict = check.run_with(|_| created, list("GAME.EXE", created));
    app.finish_foreground(check, verdict);
    assert_eq!(app.foreign.get(&30), Some(&created));
}
#[test]
fn client_hooks_use_the_saved_environment_on_disconnect() {
    let elevated = butterpollo_windows::process::user_environment().is_err();
    let path = std::env::temp_dir().join(format!(
        "butterpollo-client-hooks-{}.txt",
        uuid::Uuid::new_v4()
    ));
    let environment = BTreeMap::from([("MARKER".into(), path.to_string_lossy().into_owned())]);
    let extra = BTreeMap::from([
        (
            "do".into(),
            serde_json::json!([{"cmd":"echo connected>\"$(MARKER)\"","elevated":elevated}]),
        ),
        (
            "undo".into(),
            serde_json::json!([{"cmd":"echo disconnected>>\"$(MARKER)\"","elevated":elevated}]),
        ),
    ]);
    let wait = |needle: &str| {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !std::fs::read_to_string(&path)
            .unwrap_or_default()
            .contains(needle)
        {
            assert!(
                std::time::Instant::now() < deadline,
                "client hook did not write {needle}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let hooks = ClientCommands::with_environment(&extra, environment.clone()).unwrap();
    wait("connected");
    assert!(
        !std::fs::read_to_string(&path)
            .unwrap()
            .contains("disconnected")
    );
    drop(hooks);
    wait("disconnected");
    assert_eq!(
        std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["connected", "disconnected"]
    );
    std::fs::remove_file(&path).unwrap();
    let invalid = BTreeMap::from([
        ("do".into(), extra["do"].clone()),
        ("undo".into(), serde_json::json!([{"cmd":null}])),
    ]);
    assert!(ClientCommands::with_environment(&invalid, environment).is_err());
    assert!(!path.exists());
}
