use super::*;

#[test]
fn last_played_is_per_game_zero_for_never_and_absent_when_unknown() {
    let game = |id: &str, at: Option<i64>| Game {
        id: id.into(),
        last_played: at,
        ..Game::default()
    };
    CATALOG.lock().unwrap().games = vec![
        game("AA-bb", Some(1_788_025_796)),
        game("cc", None),
        game("dd", Some(-5)),
        Game {
            last_played_unknown: true,
            ..game("ee", None)
        },
    ];
    let played = last_played();
    CATALOG.lock().unwrap().games.clear();
    assert_eq!(
        played,
        [
            ("playnite:aa-bb".to_owned(), 1_788_025_796),
            ("playnite:cc".to_owned(), 0),
            ("playnite:dd".to_owned(), 0),
        ]
    );
}

fn launch_state() -> LaunchState {
    LaunchState::default()
}
fn status(name: &str, id: &str) -> Message {
    playnite::parse(
        &json!({"type":"status","status":{"name":name,"id":id,"installDir":"C:/Games/Nightfire"}})
            .to_string(),
    )
}
#[test]
fn a_stop_before_start_does_not_end_the_stream() {
    let mut state = launch_state();
    state.status("game", status("gameStopped", "game"));
    assert_eq!(state.phase, Phase::Starting);
}
#[test]
fn a_lost_plugin_connection_is_not_a_game_exit() {
    let mut state = launch_state();
    state.status("game", status("gameStarted", "game"));
    state.disconnected();
    assert_eq!(state.phase, Phase::Running);
}
#[test]
fn store_handoffs_and_transient_stops_wait_for_the_real_game_exit() {
    let mut state = launch_state();
    let at = Instant::now();
    state.status("game", status("gameStarted", "game"));
    state.status("game", status("gameStopped", "game"));
    state.poll(at, Some(false));
    state.poll(at + EXIT_GRACE - Duration::from_secs(1), Some(true));
    assert_eq!(state.phase, Phase::Running);
    state.poll(at + Duration::from_secs(30), Some(false));
    state.poll(at + Duration::from_secs(44), Some(false));
    assert_eq!(state.phase, Phase::Running);
    state.poll(at + Duration::from_secs(45), Some(false));
    assert_eq!(state.phase, Phase::Exited);
}
#[test]
fn closing_playnite_tracks_the_game_until_its_process_exits() {
    let mut state = launch_state();
    let at = Instant::now();
    state.status("game", status("gameStarted", "game"));
    state.status("game", status("playniteExiting", ""));
    state.poll(at, Some(true));
    state.poll(at + Duration::from_secs(1), Some(false));
    assert_eq!(state.phase, Phase::Running);
    state.poll(at + Duration::from_secs(1) + EXIT_GRACE, Some(false));
    assert_eq!(state.phase, Phase::Exited);
}
#[test]
fn unknown_and_slow_launches_remain_open_and_accept_late_start_events() {
    let mut state = launch_state();
    assert!(!state.start_timeout(START_TIMEOUT - Duration::from_secs(1)));
    assert!(state.start_timeout(START_TIMEOUT));
    assert_eq!(state.phase, Phase::Untracked);
    state.status("game", status("gameStarted", "game"));
    assert_eq!(state.phase, Phase::Running);
    state.disconnected();
    let at = Instant::now();
    state.poll(at, Some(false));
    state.poll(at + START_TIMEOUT, Some(false));
    assert_eq!(state.phase, Phase::Untracked);
}
#[test]
fn unrelated_or_missing_game_ids_cannot_end_the_stream() {
    let mut state = launch_state();
    state.status("game", status("gameStarted", "{GAME}"));
    for id in ["other", ""] {
        state.status("game", status("gameStopped", id));
        assert!(!state.verifying());
    }
    state.status("game", status("stopRequested", "game"));
    assert_eq!(state.phase, Phase::Stopping);
}
#[test]
fn failed_process_checks_do_not_count_towards_exit_grace() {
    let mut state = launch_state();
    state.status("game", status("gameStarted", "game"));
    state.status("game", status("gameStopped", "game"));
    let at = Instant::now();
    state.poll(at, Some(false));
    state.poll(at + EXIT_GRACE, None);
    state.poll(at + EXIT_GRACE + Duration::from_secs(1), Some(false));
    assert_eq!(state.phase, Phase::Running);
}
/// Issue #83: how long a closed game's stream stays is `playnite_exit_wait_secs`, which
/// was a fixed 15 s whatever the settings said.
#[test]
fn a_closed_games_stream_ends_after_the_wait_that_is_set() {
    let closed = |wait: u64| {
        let mut state = LaunchState {
            exit_grace: ExitGrace(Duration::from_secs(wait)),
            ..Default::default()
        };
        state.status("game", status("gameStarted", "game"));
        state.status("game", status("gameStopped", "game"));
        state
    };
    let at = Instant::now();
    let mut state = closed(5);
    state.poll(at, Some(false));
    state.poll(at + Duration::from_secs(4), Some(false));
    assert_eq!(state.phase, Phase::Running);
    state.poll(at + Duration::from_secs(5), Some(false));
    assert_eq!(state.phase, Phase::Exited);
    // 0 ends the stream at the first check that finds no process of the game.
    let mut at_once = closed(0);
    at_once.poll(at, Some(true));
    assert_eq!(at_once.phase, Phase::Running);
    at_once.poll(at + Duration::from_secs(1), Some(false));
    assert_eq!(at_once.phase, Phase::Exited);
    assert_eq!(LaunchState::default().exit_grace.0, Duration::from_secs(15));
}
/// Issue #82: a Playnite game's processes were never recorded for its swapped settings
/// files, so every stream of one ended with the "no process was seen" warning.
#[test]
fn a_running_games_new_processes_are_found_once_for_its_settings_files() {
    let process = |pid, started| butterpollo_core::steam::Process {
        pid,
        parent: 1,
        started,
        name: String::new(),
    };
    let programs = BTreeMap::from([
        (10, "C:\\Games\\Nightfire\\Nightfire.exe"),
        (11, "C:\\Games\\Nightfire\\bin\\Crash Reporter.exe"),
        (12, "C:\\Windows\\explorer.exe"),
        (13, "C:\\Games\\Nightfire\\Nightfire.exe"),
        (14, "C:\\Program Files (x86)\\Steam\\steam.exe"),
    ]);
    let reads = std::cell::Cell::new(0);
    let program = |pid: u32| {
        reads.set(reads.get() + 1);
        programs.get(&pid).map(|program| (*program).to_owned())
    };
    let game = ("C:/Games/Nightfire", "");
    let at = Instant::now();
    // Process 13 is the game left running at the desk before this launch.
    let mut watch = GameWatch::new(&[process(13, 5)], at);
    assert!(watch.due(at));
    // The launcher is up, the game not yet; process 15 cannot be read.
    let starting = [
        process(12, 90),
        process(13, 5),
        process(14, 95),
        process(15, 96),
    ];
    assert!(watch.scan(at, game, &starting, program).is_empty());
    assert!(watch.due(at + WATCH_SOON), "looked for again at once");
    let started = at + WATCH_SOON;
    let running = [
        process(10, 100),
        process(12, 90),
        process(13, 5),
        process(14, 95),
        process(15, 96),
    ];
    reads.set(0);
    assert_eq!(
        watch.scan(started, game, &running, program),
        [Watched {
            pid: 10,
            started: 100,
            program: "C:\\Games\\Nightfire\\Nightfire.exe".into(),
        }]
    );
    assert_eq!(reads.get(), 2, "the new process and the unreadable one");
    assert!(!watch.due(started + WATCH_SOON));
    assert!(watch.due(started + WATCH_LATER));
    // A helper it starts later is found; the game is not returned a second time.
    let helper = [process(10, 100), process(11, 200), process(13, 5)];
    let found = watch.scan(started + WATCH_LATER, game, &helper, program);
    assert_eq!(found.iter().map(|p| p.pid).collect::<Vec<_>>(), [11]);
    // Another game from the fullscreen menu: its processes are looked for afresh.
    let other = ("C:/Games/Other", "");
    assert!(
        watch
            .scan(started + WATCH_LATER, other, &helper, program)
            .is_empty()
    );
    assert!(watch.due(started + WATCH_LATER + WATCH_SOON));
    // A game that never shows a process is looked for less often after its first minute.
    let late = started + WATCH_LATER + WATCH_START;
    assert!(watch.scan(late, other, &helper, program).is_empty());
    assert!(!watch.due(late + WATCH_SOON));
    assert!(watch.due(late + WATCH_LATER));
    watch.failed(late + WATCH_LATER);
    assert!(!watch.due(late + WATCH_LATER + WATCH_SOON));
}

#[test]
fn fullscreen_waits_for_the_ui_and_ignores_the_short_lived_launcher() {
    let mut state = LaunchState {
        fullscreen: true,
        ..Default::default()
    };
    state.poll_fullscreen(false);
    assert!(!state.start_timeout(Duration::from_secs(8)));
    state.poll_fullscreen(true);
    assert!(!state.start_timeout(START_TIMEOUT + Duration::from_secs(1)));
    assert_eq!(state.phase, Phase::Starting);
    state.status("", status("playniteExiting", ""));
    assert_eq!(state.phase, Phase::Exited);
}

#[test]
fn fullscreen_returns_to_the_menu_after_a_game_and_can_launch_another() {
    let mut state = LaunchState {
        fullscreen: true,
        ..Default::default()
    };
    let at = Instant::now();
    state.poll_fullscreen(true);
    state.status("", status("gameStarted", "first"));
    state.status("", status("gameStopped", "other"));
    assert!(!state.verifying());
    state.status("", status("gameStopped", "first"));
    state.poll(at, Some(false));
    state.poll(at + EXIT_GRACE, Some(false));
    state.poll_fullscreen(true);
    assert_eq!(state.phase, Phase::Starting);
    assert!(state.game_id.is_empty());
    state.status("", status("gameStarted", "second"));
    assert_eq!(state.phase, Phase::Running);
    assert_eq!(state.game_id, "second");
}

#[test]
fn a_started_game_and_the_returning_menu_ask_for_focus() {
    let mut state = LaunchState {
        fullscreen: true,
        ..Default::default()
    };
    let at = Instant::now();
    state.poll_fullscreen(true);
    assert!(std::mem::take(&mut state.focus_menu));
    state.poll_fullscreen(true);
    assert!(!state.focus_menu);
    state.status("", status("gameStarted", "game"));
    assert!(std::mem::take(&mut state.focus_game));
    state.status("", status("gameStopped", "game"));
    state.poll(at, Some(false));
    state.poll(at + EXIT_GRACE, Some(false));
    state.poll_fullscreen(true);
    assert!(state.focus_menu);
    assert!(!state.focus_game);
}

#[test]
fn fullscreen_hands_off_to_the_game_when_playnite_exits() {
    let mut state = LaunchState {
        fullscreen: true,
        ..Default::default()
    };
    let at = Instant::now();
    state.status("", status("gameStarted", "game"));
    state.disconnected();
    state.poll(at, Some(true));
    state.poll_fullscreen(false);
    assert_eq!(state.phase, Phase::Running);
    state.poll(at + Duration::from_secs(1), Some(false));
    state.poll(at + Duration::from_secs(1) + EXIT_GRACE, Some(false));
    state.poll_fullscreen(false);
    assert_eq!(state.phase, Phase::Exited);
    // The game ended with Playnite closed: back to the menu.
    assert!(state.relaunch_menu(false));
    assert_eq!(state.phase, Phase::Starting);
    assert!(state.game_id.is_empty());
    assert!(!state.relaunch_menu(false));
    state.poll_fullscreen(true);
    assert!(state.focus_menu);
}

#[test]
fn closing_playnite_fullscreen_with_no_game_ends_the_stream() {
    let mut state = LaunchState {
        fullscreen: true,
        ..Default::default()
    };
    state.poll_fullscreen(true);
    state.status("", status("playniteExiting", ""));
    assert_eq!(state.phase, Phase::Exited);
    assert!(!state.relaunch_menu(false));
    let mut desktop = launch_state();
    desktop.status("game", status("gameStarted", "game"));
    desktop.status("game", status("gameStopped", "game"));
    let at = Instant::now();
    desktop.poll(at, Some(false));
    desktop.poll(at + EXIT_GRACE, Some(false));
    assert_eq!(desktop.phase, Phase::Exited);
    assert!(!desktop.relaunch_menu(false));
}

#[test]
fn fullscreen_connection_sets_environment_without_starting_a_second_game() {
    let env = BTreeMap::from([("SUNSHINE_CLIENT_WIDTH".into(), "1920".into())]);
    let fullscreen = launch_command("", &env);
    assert_eq!(fullscreen["command"], "set-environment");
    assert_eq!(fullscreen["env"]["SUNSHINE_CLIENT_WIDTH"], "1920");
    assert!(fullscreen.get("id").is_none());
    let game = launch_command("game", &env);
    assert_eq!(game["command"], "launch");
    assert_eq!(game["id"], "game");
    assert_eq!(game["env"], fullscreen["env"]);
}

#[test]
fn plugin_repairs_missing_and_partial_installs_without_downgrading() {
    let target =
        std::env::temp_dir().join(format!("butterpollo-playnite-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&target).unwrap();
    assert!(needs_update("0.4.14", &target));
    std::fs::write(target.join("extension.yaml"), "Version: 0.4.14").unwrap();
    assert!(needs_update("0.4.14", &target));
    std::fs::write(target.join("SunshinePlaynite.psm1"), "module").unwrap();
    assert!(!needs_update("0.4.14", &target));
    assert!(needs_update("0.4.15", &target));
    assert!(!needs_update("0.4.13", &target));
    std::fs::write(target.join("extension.yaml"), "Version: 0.5.0").unwrap();
    std::fs::remove_file(target.join("SunshinePlaynite.psm1")).unwrap();
    assert!(!needs_update("0.4.14", &target));
    std::fs::remove_dir_all(target).unwrap();
}

#[test]
fn plugin_copy_publishes_the_version_only_after_the_module() {
    let root = std::env::temp_dir().join(format!("butterpollo-playnite-{}", uuid::Uuid::new_v4()));
    let source = root.join("packaged");
    let target = root.join("Çağrı/Extensions/SunshinePlaynite");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(target.join("SunshinePlaynite.psm1")).unwrap();
    std::fs::write(source.join("extension.yaml"), "Version: 0.4.14").unwrap();
    std::fs::write(source.join("SunshinePlaynite.psm1"), "module").unwrap();
    std::fs::write(target.join("extension.yaml"), "Version: 0.4.13").unwrap();
    assert!(install_worker(&source, &target).is_err());
    assert_eq!(version(&target).as_deref(), Some("0.4.13"));
    std::fs::remove_dir(target.join("SunshinePlaynite.psm1")).unwrap();
    install_worker(&source, &target).unwrap();
    assert_eq!(version(&target).as_deref(), Some("0.4.14"));
    assert!(plugin_present(&target));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_sync_retries_on_the_next_watch_tick() {
    let mut watch = Watch {
        playnite: None,
        settings: String::new(),
        synced: None,
    };
    let at = Instant::now();
    assert!(
        sync_if_due(&mut watch, 1, "settings".into(), at, || bail!(
            "plugin still starting"
        ))
        .is_err()
    );
    assert!(watch.synced.is_none());
    let mut calls = 0;
    for seconds in [30, 60] {
        sync_if_due(
            &mut watch,
            1,
            "settings".into(),
            at + Duration::from_secs(seconds),
            || {
                calls += 1;
                Ok(Outcome {
                    changed: false,
                    games: 2,
                })
            },
        )
        .unwrap();
    }
    assert_eq!(calls, 1);
    assert_eq!(watch.synced, Some(at + Duration::from_secs(30)));
}
