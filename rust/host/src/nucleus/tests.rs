use super::*;
use anyhow::anyhow;
use butterpollo_core::nucleus::coop::{Phase, Refused};
use butterpollo_core::nucleus::elevation::Why;
use serde_json::json;

fn app(name: &str, cmd: &str) -> App {
    App {
        name: name.into(),
        cmd: cmd.into(),
        working_dir: String::new(),
        prep: Vec::new(),
        computed_id: None,
        aliases: Vec::new(),
        extra: BTreeMap::new(),
    }
}
fn client(uuid: &str, coop_join: bool) -> Client {
    let mut client: Client = serde_json::from_value(serde_json::json!({
        "name": format!("Device {uuid}"), "cert": "", "uuid": uuid,
        "perm": butterpollo_core::state::PLAY_DEVICE_PERMISSION,
    }))
    .unwrap();
    // Set either way: never set, a device that launches apps may join.
    client.extra.insert("coop_join".into(), coop_join.into());
    client
}
fn setup() -> (Registry, Arc<fake::Pc>, Vec<App>) {
    let pc = Arc::new(fake::Pc::installed());
    let apps = vec![
        app("Desktop", ""),
        app("Portal 2", &format!("\"{}\" -novid", fake::EXE)),
        app("Steam", "steam://open/bigpicture"),
    ];
    (Registry::new(pc.clone()), pc, apps)
}
fn admit(
    registry: &Registry,
    coop: &CoopApp,
    launch: &str,
    client: &Client,
) -> Result<Seat, Refusal> {
    registry.admit(Admission {
        launch,
        client,
        game: Game::Coop(coop),
        size: (1920, 1080),
        config: &Config::default(),
        warnings: Default::default(),
    })
}

#[test]
fn a_download_stops_past_its_size() {
    let mut body = Vec::new();
    chunk(&mut body, &[1; 6], 10).unwrap();
    chunk(&mut body, &[2; 4], 10).unwrap();
    assert_eq!(body.len(), 10);
    assert!(chunk(&mut body, &[3], 10).is_err());
    assert_eq!(body.len(), 10);
}

#[test]
fn entries_list_apps_with_a_handler_and_an_added_game_only() {
    let (registry, pc, apps) = setup();
    let entries = registry.entries(&apps, "");
    assert_eq!(entries.len(), 1);
    let coop = &entries[0];
    assert_eq!(coop.entry.name, "Portal 2 (Co-op)");
    assert_eq!(coop.app.name, "Portal 2");
    assert_eq!(coop.id(), app("Portal 2 (Co-op)", "").id());
    // The id follows from the name (its CRC-32), as for every app
    // without its own: the rename changed it, before any release.
    assert_eq!(coop.id(), 242_170_942);
    assert!(coop.entry.cmd.is_empty());
    assert_eq!(
        registry.resolve(&apps, "", coop.id()).unwrap().app.name,
        "Portal 2"
    );
    assert!(registry.resolve(&apps, "", apps[1].id()).is_none());
    // A detached command names the program too.
    let mut detached = app("Portal 2 detached", "");
    detached.extra.insert(
        "detached".into(),
        serde_json::json!([format!("\"{}\"", fake::EXE)]),
    );
    assert_eq!(registry.entries(&[detached], "").len(), 1);
    // A name an app already has is not listed twice.
    let taken = app("Portal 2 (Co-op)", "");
    assert!(entries_of(&pc.catalog.lock().unwrap(), &[apps[1].clone(), taken]).is_empty());
    // Not added in Nucleus: no entry.
    pc.catalog.lock().unwrap().games.clear();
    registry.use_pc(pc.clone());
    assert!(registry.entries(&apps, "").is_empty());
}

#[test]
fn no_entries_without_nucleus_or_where_it_cannot_run() {
    let (registry, pc, apps) = setup();
    *pc.installed.lock().unwrap() = None;
    assert!(registry.entries(&apps, "").is_empty());
    assert_eq!(registry.install(""), Install::Missing);
    let unavailable = Arc::new(fake::Pc::unavailable());
    registry.use_pc(unavailable);
    assert!(registry.entries(&apps, "").is_empty());
    assert_eq!(
        registry.unavailable().as_deref(),
        Some("not on this test PC")
    );
}

#[test]
fn an_app_can_hide_its_coop_entry_and_its_status_stays() {
    let (registry, _pc, mut apps) = setup();
    // As every app switch reads (and the console's flagValue): true, "true", "1" and "yes"
    // are on, any other text is off, and any other value is the default, on.
    for hidden in [
        json!(false),
        json!("false"),
        json!("0"),
        json!("off"),
        json!("TRUE"),
    ] {
        apps[1].extra.insert(ENTRY_KEY.into(), hidden.clone());
        assert!(registry.entries(&apps, "").is_empty(), "{hidden}");
    }
    for shown in [
        json!(true),
        json!("true"),
        json!("1"),
        json!("yes"),
        json!(0),
        json!(null),
    ] {
        apps[1].extra.insert(ENTRY_KEY.into(), shown.clone());
        assert_eq!(registry.entries(&apps, "").len(), 1, "{shown}");
    }
    apps[1].extra.insert(ENTRY_KEY.into(), json!(false));
    let games = registry.games(&apps, &Config::default()).unwrap().unwrap();
    assert_eq!(
        games[1],
        GameStatus::Ready {
            handler: "Portal 2".into(),
            players: 2
        }
    );
    assert!(matches!(
        games[0],
        GameStatus::Missing(Missing::NoExecutable { .. })
    ));
    assert!(matches!(
        &games[2],
        GameStatus::Missing(Missing::NoHandler { .. })
    ));
    assert!(!names_executable(&apps[0]) && names_executable(&apps[1]));
    // A launcher's link names no game .exe: no handler can match it.
    let steam = app(
        "Counter-Strike 2",
        "cmd /c start \"\" steam://rungameid/730",
    );
    assert!(names_executable(&steam) && !starts_exe(&steam));
    assert!(!starts_exe(&apps[2]) && starts_exe(&apps[1]));
    assert!(starts_exe(&app("Puzzle", r"D:\Games\PUZZLE.EXE")));
}

#[test]
fn the_console_shows_why_a_background_stream_would_be_refused() {
    // These showed "streams in the background without a handler" and
    // were then refused at launch.
    let (registry, _pc, _) = setup();
    let dir = std::env::temp_dir().join(format!("nucleus-status-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(dir.join("Shooter").join("EasyAntiCheat")).unwrap();
    std::fs::create_dir_all(dir.join("Ice")).unwrap();
    std::fs::write(dir.join("Shooter").join("Shooter.exe"), b"").unwrap();
    std::fs::write(dir.join("Ice").join("Ice.exe"), b"").unwrap();
    let quoted = |path: PathBuf| format!("\"{}\"", path.display());
    let apps = [
        app("Notepad", r"C:\Windows\notepad.exe"),
        app("Shared", r"\\nas\games\Ice\Ice.exe"),
        app("Shooter", &quoted(dir.join("Shooter").join("Shooter.exe"))),
        app("Relative", "Ice.exe"),
        app("Gone", r"C:\NoSuchGames\Gone\Gone.exe"),
        app("Ice", &quoted(dir.join("Ice").join("Ice.exe"))),
        app("Link", "steam://rungameid/400"),
    ];
    let statuses = registry.games(&apps, &Config::default()).unwrap().unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let shown: Vec<String> = statuses
        .iter()
        .map(|status| match status {
            GameStatus::Missing(missing) | GameStatus::Own { missing, .. } => missing.to_string(),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(
        shown[0],
        r"C:\Windows\notepad.exe is part of Windows, not a game"
    );
    assert!(
        shown[1].ends_with(
            "is on a network share: Rubylight starts only a game on this PC's own drives"
        )
    );
    assert_eq!(
        shown[2],
        "Shooter.exe uses Easy Anti-Cheat, and Rubylight never hooks a game with anti-cheat"
    );
    assert!(
        shown[3].starts_with("Nucleus Co-op starts games by their .exe"),
        "{}",
        shown[3]
    );
    assert!(shown[4].ends_with("is not on this PC: set the app's command to the game's .exe"));
    // A game ours streams, and a link checked only at launch.
    assert!(
        shown[5].starts_with("No Nucleus Co-op handler for Ice.exe"),
        "{}",
        shown[5]
    );
    // Without administrator rights, as nothing asks for them.
    assert!(matches!(
        statuses[5],
        GameStatus::Own {
            missing: Missing::NoHandler { .. },
            run: Run::User
        }
    ));
    assert!(matches!(
        statuses[6],
        GameStatus::Missing(Missing::NoHandler { .. })
    ));
    assert!(matches!(
        statuses[2],
        GameStatus::Missing(Missing::AntiCheat { .. })
    ));
    assert!(matches!(
        statuses[0],
        GameStatus::Missing(Missing::Refused { .. })
    ));
}

#[test]
fn game_statuses_need_an_install_and_follow_the_player_cap() {
    let (registry, pc, apps) = setup();
    let mut config = Config::default();
    config
        .values
        .insert("nucleus_coop_players".into(), "1".into());
    assert_eq!(
        registry.games(&apps, &config).unwrap().unwrap()[1],
        GameStatus::Ready {
            handler: "Portal 2".into(),
            players: 1
        }
    );
    // Not added in Nucleus Co-op yet, and the .exe isn't on this PC: a
    // background stream would say so.
    pc.catalog.lock().unwrap().games.clear();
    registry.use_pc(pc.clone());
    assert!(matches!(
        &registry.games(&apps, &config).unwrap().unwrap()[1],
        GameStatus::Missing(Missing::Refused { reason }) if reason.contains("is not on this PC")
    ));
    *pc.installed.lock().unwrap() = None;
    assert!(registry.games(&apps, &config).is_none());
}

#[test]
fn a_hub_handler_without_the_added_game_says_to_add_it_unless_that_cant_help() {
    // Adding the game makes the hub handler start it, anti-cheat and
    // all: the console says to add it. A game that isn't there says so.
    let (registry, pc, _) = setup();
    pc.catalog.lock().unwrap().games.clear();
    registry.use_pc(pc.clone());
    let dir = std::env::temp_dir().join(format!("nucleus-own-{}", uuid::Uuid::new_v4()));
    let game = dir.join("Portal 2");
    std::fs::create_dir_all(game.join("EasyAntiCheat")).unwrap();
    std::fs::write(game.join("portal2.exe"), b"").unwrap();
    let apps = [app(
        "Portal 2",
        &format!("\"{}\"", game.join("portal2.exe").display()),
    )];
    assert!(matches!(
        &registry.games(&apps, &Config::default()).unwrap().unwrap()[0],
        GameStatus::Missing(Missing::NotAdded { .. })
    ));
    // Not on this PC, or no .exe named: adding can't help.
    let gone = [
        app(
            "Portal 2",
            &format!("\"{}\"", dir.join(r"Gone\portal2.exe").display()),
        ),
        app("Portal 2", "portal2.exe"),
    ];
    let statuses = registry.games(&gone, &Config::default()).unwrap().unwrap();
    assert!(matches!(
        &statuses[0],
        GameStatus::Missing(Missing::Refused { reason }) if reason.contains("is not on this PC")
    ));
    assert!(matches!(
        &statuses[1],
        GameStatus::Missing(Missing::NoExecutable { .. })
    ));
    // A game ours streams keeps the hint to add it for co-op.
    let other = dir.join("Clean");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("portal2.exe"), b"").unwrap();
    let apps = [app(
        "Portal 2",
        &format!("\"{}\"", other.join("portal2.exe").display()),
    )];
    assert!(matches!(
        &registry.games(&apps, &Config::default()).unwrap().unwrap()[0],
        GameStatus::Own {
            missing: Missing::NotAdded { .. },
            run: Run::User
        }
    ));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_consoles_answer_for_an_exe_is_kept_until_it_changes_or_ages() {
    let (registry, _pc, _) = setup();
    let dir = std::env::temp_dir().join(format!("nucleus-own-{}", uuid::Uuid::new_v4()));
    let exe = dir.join("Ice").join("Ice.exe");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, b"").unwrap();
    let apps = [app("Ice", &format!("\"{}\"", exe.display()))];
    let status = || registry.games(&apps, &Config::default()).unwrap().unwrap()[0].clone();
    let streams = |status: GameStatus| {
        matches!(
            status,
            GameStatus::Own {
                missing: Missing::NoHandler { .. },
                ..
            }
        )
    };
    assert!(streams(status()));
    // Anti-cheat added beside it: the console asks again within the
    // minute and gets the kept answer, without a scan.
    std::fs::create_dir(dir.join("Ice").join("EasyAntiCheat")).unwrap();
    assert!(streams(status()));
    // The .exe changed: read again.
    let file = std::fs::File::options().write(true).open(&exe).unwrap();
    file.set_modified(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000))
        .unwrap();
    drop(file);
    assert!(matches!(
        status(),
        GameStatus::Missing(Missing::AntiCheat { .. })
    ));
    // Aged out: read again.
    std::fs::remove_dir(dir.join("Ice").join("EasyAntiCheat")).unwrap();
    assert!(matches!(
        status(),
        GameStatus::Missing(Missing::AntiCheat { .. })
    ));
    for kept in registry.own_statuses.lock().unwrap().values_mut() {
        kept.at = Instant::now() - OWN_STATUS_TTL;
    }
    assert!(streams(status()));
    // A share is refused before anything opens it, and never kept.
    let shared = [app("Shared", r"\\nas\games\Ice\Ice.exe")];
    let _ = registry.games(&shared, &Config::default());
    assert_eq!(registry.own_statuses.lock().unwrap().len(), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_console_asks_again_when_it_could_not_tell() {
    let (registry, pc, _) = setup();
    let dir = std::env::temp_dir().join(format!("nucleus-own-{}", uuid::Uuid::new_v4()));
    let exe = dir.join("Ice").join("Ice.exe");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, b"").unwrap();
    let apps = [app("Ice", &format!("\"{}\"", exe.display()))];
    let status = || registry.games(&apps, &Config::default()).unwrap().unwrap()[0].clone();
    // No signed-in user yet: said, and asked again at the next refresh.
    *pc.need_error.lock().unwrap() = Some("nobody is signed in".into());
    let GameStatus::Missing(Missing::Refused { reason }) = status() else {
        panic!("{:?}", status());
    };
    assert_eq!(
        reason,
        "Rubylight could not tell whether Ice.exe needs administrator rights: nobody is signed in"
    );
    *pc.need_error.lock().unwrap() = None;
    assert!(matches!(status(), GameStatus::Own { run: Run::User, .. }));
    // A refusal stands until the .exe changes or the minute passes.
    for kept in registry.own_statuses.lock().unwrap().values_mut() {
        kept.at = Instant::now() - OWN_STATUS_TTL;
    }
    *pc.account.lock().unwrap() = Account::Standard;
    pc.needs.lock().unwrap().insert(
        exe.to_string_lossy().into_owned(),
        Need::Administrator(Why::Manifest),
    );
    let refused = status();
    assert!(
        matches!(refused, GameStatus::Missing(Missing::Refused { .. })),
        "{refused:?}"
    );
    pc.needs.lock().unwrap().clear();
    assert_eq!(status(), refused);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn set_up_installs_once_and_reports_its_progress() {
    let (registry, pc, _apps) = setup();
    assert_eq!(registry.set_up("").unwrap(), SetUp::Installed);
    *pc.installed.lock().unwrap() = None;
    assert_eq!(registry.set_up("").unwrap(), SetUp::Started);
    assert_eq!(registry.install(""), Install::Installing(0));
    assert_eq!(registry.set_up("").unwrap(), SetUp::Installing);
    registry.install_now("");
    assert_eq!(
        registry.install(""),
        Install::Installed(PathBuf::from(r"C:\Nucleus"))
    );
    assert_eq!(pc.installs.load(std::sync::atomic::Ordering::Relaxed), 1);
}

#[test]
fn a_launch_during_set_up_waits_for_that_install_and_shares_its_result() {
    use std::sync::atomic::Ordering::Relaxed;
    // Where panics unwind (tests), a panicking install still ends the
    // wait with its reason; a release build aborts the host instead.
    for (error, panics) in [
        (None, false),
        (Some("the download failed"), false),
        (Some("the install stopped: the unpacker broke"), true),
    ] {
        let (registry, pc, apps) = setup();
        let registry = Arc::new(registry);
        let coop = registry.entries(&apps, "").remove(0);
        *pc.installed.lock().unwrap() = None;
        *pc.install_panics.lock().unwrap() = panics;
        if !panics {
            *pc.install_error.lock().unwrap() = error.map(String::from);
        }
        *pc.hold.lock().unwrap() = true;
        assert_eq!(registry.set_up("").unwrap(), SetUp::Started);
        let console = {
            let registry = registry.clone();
            std::thread::spawn(move || registry.install_now(""))
        };
        while pc.installs.load(Relaxed) == 0 {
            std::thread::sleep(Duration::from_millis(5));
        }
        let launch = {
            let registry = registry.clone();
            std::thread::spawn(move || {
                admit(&registry, &coop, "a", &client("A", false)).map(|_| ())
            })
        };
        while registry.lock().install_waiters == 0 {
            std::thread::sleep(Duration::from_millis(5));
        }
        // One download, one progress: the launch did not start its own.
        assert_eq!(registry.install(""), Install::Installing(1));
        *pc.hold.lock().unwrap() = false;
        pc.release.notify_all();
        console.join().unwrap();
        let launched = launch.join().unwrap();
        assert_eq!(pc.installs.load(Relaxed), 1, "{error:?}");
        match error {
            None => {
                launched.unwrap();
                assert_eq!(
                    registry.install(""),
                    Install::Installed(PathBuf::from(r"C:\Nucleus"))
                );
            }
            Some(error) => {
                let refused = launched.unwrap_err();
                let message = format!("Nucleus Co-op is not ready: {error}");
                assert_eq!(
                    (refused.status, refused.message.as_str()),
                    (503, message.as_str())
                );
                assert_eq!(registry.install(""), Install::Failed(message));
            }
        }
    }
}

#[test]
fn set_up_and_open_are_refused_where_nucleus_cannot_run() {
    let (registry, _pc, _apps) = setup();
    let unavailable = Arc::new(fake::Pc::unavailable());
    *unavailable.installed.lock().unwrap() = None;
    registry.use_pc(unavailable.clone());
    let refused = registry.set_up("").unwrap_err();
    assert_eq!(
        (refused.status, refused.message.as_str()),
        (503, "not on this test PC")
    );
    assert_eq!(registry.install(""), Install::Missing);
    let refused = registry.open("").unwrap_err();
    assert_eq!(
        (refused.status, refused.message.as_str()),
        (503, "not on this test PC")
    );
    assert_eq!(
        unavailable
            .opened
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

#[test]
fn open_needs_the_install_and_no_running_session() {
    let (registry, pc, apps) = setup();
    *pc.installed.lock().unwrap() = None;
    let refused = registry.open("").unwrap_err();
    assert_eq!(refused.status, 409);
    assert!(refused.message.contains("set up co-op first"), "{refused}");
    *pc.installed.lock().unwrap() = Some(PathBuf::from(r"C:\Nucleus"));
    registry.open("").unwrap();
    assert_eq!(pc.opened.load(std::sync::atomic::Ordering::Relaxed), 1);
    let coop = registry.entries(&apps, "").remove(0);
    admit(&registry, &coop, "a", &client("A", false)).unwrap();
    let refused = registry.open("").unwrap_err();
    assert_eq!(
        (refused.status, refused.message.as_str()),
        (
            409,
            "Nucleus Co-op is running Portal 2: end that session first"
        )
    );
    assert_eq!(pc.opened.load(std::sync::atomic::Ordering::Relaxed), 1);
}

#[test]
fn a_session_closes_the_window_the_console_opened_first() {
    use std::sync::atomic::Ordering::Relaxed;
    let (registry, pc, apps) = setup();
    registry.open("").unwrap();
    assert!(registry.opened());
    let window = pc.window();
    let coop = registry.entries(&apps, "").remove(0);
    admit(&registry, &coop, "a", &client("A", false)).unwrap();
    assert_eq!(window.closes.load(Relaxed), 1);
    assert!(!window.open.load(Relaxed));
    assert!(!registry.opened());
    assert_eq!(pc.started.lock().unwrap().len(), 1);

    // A background session closes it too.
    let (registry, pc, apps) = setup();
    registry.open("").unwrap();
    registry
        .admit(Admission {
            launch: "b",
            client: &client("B", false),
            game: Game::Background(&apps[1]),
            size: (1920, 1080),
            config: &Config::default(),
            warnings: Default::default(),
        })
        .unwrap();
    assert!(!pc.window().open.load(Relaxed));
    assert_eq!(pc.started.lock().unwrap().len(), 1);
}

#[test]
fn a_window_that_wont_close_refuses_the_session_and_stays_open() {
    use std::sync::atomic::Ordering::Relaxed;
    let (registry, pc, apps) = setup();
    registry.open("").unwrap();
    let window = pc.window();
    window.stuck.store(true, Relaxed);
    let coop = registry.entries(&apps, "").remove(0);
    let refused = admit(&registry, &coop, "a", &client("A", false)).unwrap_err();
    assert_eq!(
        (refused.status, refused.message.as_str()),
        (
            503,
            "Nucleus Co-op did not start Portal 2: Nucleus Co-op (process 9000) did not close within 5 s: close it on the PC"
        )
    );
    // Asked to close, never ended.
    assert_eq!(window.closes.load(Relaxed), 1);
    assert!(window.open.load(Relaxed));
    assert!(registry.opened());
    assert!(pc.started.lock().unwrap().is_empty());
    // Once the user closes it at the PC, the next launch starts.
    window.stuck.store(false, Relaxed);
    window.open.store(false, Relaxed);
    admit(&registry, &coop, "a", &client("A", false)).unwrap();
    assert_eq!(window.closes.load(Relaxed), 1);
}

#[test]
fn a_launch_refused_for_another_reason_leaves_the_window_open() {
    use std::sync::atomic::Ordering::Relaxed;
    let (registry, pc, apps) = setup();
    registry.open("").unwrap();
    let window = pc.window();
    *pc.free.lock().unwrap() = 0;
    let coop = registry.entries(&apps, "").remove(0);
    admit(&registry, &coop, "a", &client("A", false)).unwrap_err();
    assert_eq!(window.closes.load(Relaxed), 0);
    assert!(registry.opened());
    assert!(pc.started.lock().unwrap().is_empty());
}

#[test]
fn a_launch_the_pc_cant_start_leaves_the_window_open() {
    // Refused for administrator rights, a handler file that isn't ours,
    // a game list the host can't edit or compatibility settings an
    // earlier session left that don't come back, only after the
    // console's Nucleus window had closed.
    use std::sync::atomic::Ordering::Relaxed;
    let (registry, pc, apps) = setup();
    registry.open("").unwrap();
    let window = pc.window();
    *pc.ready_error.lock().unwrap() = Some("no administrator rights".into());
    let coop = registry.entries(&apps, "").remove(0);
    let refused = admit(&registry, &coop, "a", &client("A", false)).unwrap_err();
    assert_eq!(
        (refused.status, refused.message.as_str()),
        (
            503,
            "Nucleus Co-op did not start Portal 2: no administrator rights"
        )
    );
    let ice = app("Ice", r"C:\NoSuchGames\Ice\Ice.exe");
    let refused = background(
        &registry,
        &ice,
        "b",
        &client("B", false),
        &Config::default(),
    )
    .unwrap_err();
    assert!(
        refused.message.ends_with("no administrator rights"),
        "{refused}"
    );
    assert_eq!(window.closes.load(Relaxed), 0);
    assert!(registry.opened());
    assert!(pc.own.lock().unwrap().is_empty());
    assert!(pc.started.lock().unwrap().is_empty());
}

#[test]
fn a_window_that_cant_be_read_refuses_the_session_with_why() {
    use std::sync::atomic::Ordering::Relaxed;
    let (registry, pc, apps) = setup();
    registry.open("").unwrap();
    let window = pc.window();
    window.unreadable.store(true, Relaxed);
    let coop = registry.entries(&apps, "").remove(0);
    let refused = admit(&registry, &coop, "a", &client("A", false)).unwrap_err();
    assert_eq!(
        (refused.status, refused.message.as_str()),
        (
            503,
            "Nucleus Co-op did not start Portal 2: Nucleus Co-op's window (process 9000) could not be read: access denied"
        )
    );
    assert_eq!(window.closes.load(Relaxed), 0);
    assert!(pc.started.lock().unwrap().is_empty());
}

#[test]
fn open_is_refused_while_a_launch_is_being_admitted() {
    use std::sync::atomic::Ordering::Relaxed;
    let (registry, pc, apps) = setup();
    let registry = Arc::new(registry);
    let coop = registry.entries(&apps, "").remove(0);
    *pc.installed.lock().unwrap() = None;
    *pc.hold.lock().unwrap() = true;
    let launch = {
        let registry = registry.clone();
        std::thread::spawn(move || admit(&registry, &coop, "a", &client("A", false)).map(|_| ()))
    };
    while pc.installs.load(Relaxed) == 0 {
        std::thread::sleep(Duration::from_millis(5));
    }
    // The launch holds the admission while it installs; to Open, Nucleus
    // Co-op is there already.
    *pc.installed.lock().unwrap() = Some(PathBuf::from(r"C:\Nucleus"));
    let refused = registry.open("").unwrap_err();
    assert_eq!(
        (refused.status, refused.message.as_str()),
        (
            409,
            "A co-op or background stream is starting: open Nucleus Co-op once it has"
        )
    );
    assert_eq!(pc.opened.load(Relaxed), 0);
    *pc.hold.lock().unwrap() = false;
    pc.release.notify_all();
    launch.join().unwrap().unwrap();
}

#[test]
fn a_nucleus_that_restarted_itself_is_still_the_one_the_console_opened() {
    use std::sync::atomic::Ordering::Relaxed;
    let (registry, pc, _apps) = setup();
    registry.open("").unwrap();
    let window = pc.window();
    *window.restarted_as.lock().unwrap() = Some(9500);
    let refused = registry.open("").unwrap_err();
    assert_eq!(
        (refused.status, refused.message.as_str()),
        (
            409,
            "Nucleus Co-op is already open on the PC's screen: use it there, or close it here"
        )
    );
    assert!(registry.close().unwrap());
    assert_eq!(window.closes.load(Relaxed), 1);
    assert!(!registry.opened());
}

#[test]
fn open_and_close_leave_a_nucleus_the_host_did_not_start_alone() {
    use std::sync::atomic::Ordering::Relaxed;
    let (registry, pc, apps) = setup();
    *pc.foreign.lock().unwrap() = Some(77);
    let refused = registry.open("").unwrap_err();
    assert_eq!(
        (refused.status, refused.message.as_str()),
        (
            409,
            "Nucleus Co-op is already running on the PC (process 77): close it there first"
        )
    );
    assert_eq!(pc.opened.load(Relaxed), 0);
    let refused = registry.close().unwrap_err();
    assert_eq!(
        (refused.status, refused.message.as_str()),
        (
            409,
            "Nucleus Co-op (process 77) was not opened from this console: close it on the PC"
        )
    );
    // A launch is refused as before, loudly.
    let coop = registry.entries(&apps, "").remove(0);
    let refused = admit(&registry, &coop, "a", &client("A", false)).unwrap_err();
    assert!(
            refused
                .message
                .ends_with("Nucleus Co-op is already running on the PC (process 77): close it, then start the stream again"),
            "{refused}"
        );
}

#[test]
fn open_twice_is_refused_and_close_closes_it_once() {
    use std::sync::atomic::Ordering::Relaxed;
    let (registry, pc, _apps) = setup();
    assert!(!registry.close().unwrap(), "nothing to close");
    registry.open("").unwrap();
    let refused = registry.open("").unwrap_err();
    assert_eq!(
        (refused.status, refused.message.as_str()),
        (
            409,
            "Nucleus Co-op is already open on the PC's screen: use it there, or close it here"
        )
    );
    assert_eq!(pc.opened.load(Relaxed), 1);
    // A question it asks keeps it open, and says so.
    pc.window().stuck.store(true, Relaxed);
    let refused = registry.close().unwrap_err();
    assert_eq!(refused.status, 409);
    assert!(refused.message.contains("did not close"), "{refused}");
    assert!(registry.opened());
    pc.window().stuck.store(false, Relaxed);
    assert!(registry.close().unwrap());
    assert!(!registry.opened());
    assert!(!registry.close().unwrap());
    // Closed, it opens again.
    registry.open("").unwrap();
    assert_eq!(pc.opened.load(Relaxed), 2);
}

#[test]
fn the_catalog_is_read_once_for_the_library_and_again_for_a_launch() {
    let (registry, pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    registry.entries(&apps, "");
    assert_eq!(pc.reads.load(std::sync::atomic::Ordering::Relaxed), 1);
    admit(&registry, &coop, "a", &client("A", false)).unwrap();
    assert_eq!(pc.reads.load(std::sync::atomic::Ordering::Relaxed), 2);
}

#[test]
fn launches_start_a_session_and_join_it_by_permission_and_free_slot() {
    let (registry, pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    let owner = client("A", false);
    assert_eq!(
        admit(&registry, &coop, "a", &owner).unwrap(),
        Seat {
            session: 1,
            slot: 0
        }
    );
    let session = pc.last();
    assert_eq!(session.session.lock().unwrap().slots.len(), 2);
    // Without Join co-op another device is refused.
    let refused = admit(&registry, &coop, "b", &client("B", false)).unwrap_err();
    assert_eq!(refused.status, 409);
    assert_eq!(refused.message, Refused::NotPermitted.to_string());
    let second = client("B", true);
    assert_eq!(
        admit(&registry, &coop, "b", &second).unwrap(),
        Seat {
            session: 1,
            slot: 1
        }
    );
    let full = admit(&registry, &coop, "c", &client("C", true)).unwrap_err();
    assert_eq!(
        full,
        Refusal::busy(
            Refused::Full {
                title: "Portal 2".into()
            }
            .to_string()
        )
    );
    assert_eq!(pc.started.lock().unwrap().len(), 1);
    // The registry hands the stream its session.
    let instance = registry.instance(1).unwrap();
    assert!(instance.capture_target(1, "d").is_err());
    pc.last().show();
    assert_eq!(instance.capture_target(1, "d").unwrap(), "window:101@d");
    assert!(registry.instance(2).is_none());
}

fn background(
    registry: &Registry,
    app: &App,
    launch: &str,
    client: &Client,
    config: &Config,
) -> Result<Seat, Refusal> {
    registry.admit(Admission {
        launch,
        client,
        game: Game::Background(app),
        size: (1280, 720),
        config,
        warnings: Default::default(),
    })
}

#[test]
fn a_background_stream_is_a_private_session_its_owner_returns_to() {
    let (registry, pc, apps) = setup();
    let owner = client("A", true);
    let config = Config::default();
    let seat = background(&registry, &apps[1], "a", &owner, &config).unwrap();
    assert_eq!(
        seat,
        Seat {
            session: 1,
            slot: 0
        }
    );
    let listed = registry.sessions();
    assert_eq!(listed[0].session.kind, Kind::Background);
    // One player whatever the handler allows, listed under the app.
    assert_eq!(listed[0].session.slots.len(), 1);
    assert_eq!(listed[0].app_id, apps[1].id());
    assert_eq!(pc.last().sizes.lock().unwrap()[..], [(1280, 720)]);
    // Nobody else joins it, by the app or by its co-op entry.
    let other = background(&registry, &apps[1], "b", &client("B", true), &config).unwrap_err();
    assert_eq!(other.status, 409);
    assert!(
            other
                .message
                .starts_with("Portal 2 can't stream in the background: Nucleus Co-op is running Portal 2 for a background stream"),
            "{}",
            other.message
        );
    let coop = registry.entries(&apps, "").remove(0);
    assert_eq!(
        admit(&registry, &coop, "c", &client("B", true))
            .unwrap_err()
            .status,
        409
    );
    // Nor from a session list: its id is no co-op session, for anyone.
    assert_eq!(
        registry.join(seat.session, &client("B", true)),
        Err(JoinError::NoSession)
    );
    assert_eq!(
        registry.join(seat.session, &owner),
        Err(JoinError::NoSession)
    );
    // Its owner returns to its own slot of the same session.
    registry.leave(seat, "a");
    assert_eq!(
        background(&registry, &apps[1], "d", &owner, &config).unwrap(),
        seat
    );
    assert_eq!(pc.started.lock().unwrap().len(), 1);
}

#[test]
fn a_background_stream_nucleus_cannot_run_is_refused_with_the_reason() {
    let (registry, pc, apps) = setup();
    let owner = client("A", false);
    let config = Config::default();
    // A launcher's link, or no program at all.
    let store = app(
        "Store Game",
        "com.epicgames.launcher://apps/x?action=launch",
    );
    let missing = background(&registry, &store, "a", &owner, &config).unwrap_err();
    assert_eq!(missing.status, 404);
    assert_eq!(
        missing.message,
        "Store Game can't stream in the background: Nucleus Co-op starts games by their .exe, and Store Game does not name one: set the app's command to the game's .exe"
    );
    let desktop = background(&registry, &apps[0], "a", &owner, &config).unwrap_err();
    assert_eq!(desktop.status, 404);
    assert!(
        desktop.message.starts_with(
            "Desktop can't stream in the background: Nucleus Co-op starts games by their .exe"
        ),
        "{}",
        desktop.message
    );
    // Windows' own program, or a game on a network share, which the
    // host as SYSTEM would open: refused before anything reads it.
    let notepad = app("Notepad", r"C:\Windows\notepad.exe");
    let refused = background(&registry, &notepad, "a", &owner, &config).unwrap_err();
    assert_eq!(refused.status, 404);
    assert_eq!(
        refused.message,
        r"Notepad can't stream in the background: C:\Windows\notepad.exe is part of Windows, not a game"
    );
    let shared = app("Shared", r"\\nas\games\Ice\Ice.exe");
    let refused = background(&registry, &shared, "a", &owner, &config).unwrap_err();
    assert!(
        refused.message.ends_with(
            "is on a network share: Rubylight starts only a game on this PC's own drives"
        ),
        "{refused}"
    );
    assert!(pc.own.lock().unwrap().is_empty());
    // A background_mode that is no mode runs none.
    let bad = Config::parse("background_mode = rdp\n").unwrap();
    let refused = background(&registry, &apps[1], "a", &owner, &bad).unwrap_err();
    assert_eq!(refused.status, 503);
    assert!(
        refused
            .message
            .contains("background_mode = rdp is not a mode"),
        "{}",
        refused.message
    );
    // Values earlier builds wrote run in Nucleus Co-op.
    for old in ["auto", "child_session", "virtual_display"] {
        let config = Config::parse(&format!("background_mode = {old}\n")).unwrap();
        let seat = background(&registry, &apps[1], old, &owner, &config).unwrap();
        assert!(registry.end(seat.session), "{old}");
    }
    // Where Nucleus Co-op can't run, the reason.
    registry.use_pc(Arc::new(fake::Pc::unavailable()));
    let unavailable = background(&registry, &apps[1], "a", &owner, &config).unwrap_err();
    assert_eq!(unavailable.status, 503);
    assert_eq!(
        unavailable.message,
        "Portal 2 can't stream in the background: not on this test PC"
    );
    assert_eq!(pc.started.lock().unwrap().len(), 3);
}

#[test]
fn a_one_player_game_without_a_hub_handler_streams_in_the_background_with_ours() {
    let (registry, pc, mut apps) = setup();
    let owner = client("A", false);
    let config = Config::default();
    apps.push(app("Ice", r#""C:\NoSuchGames\Ice\Ice.exe" -windowed"#));
    let seat = background(&registry, &apps[3], "a", &owner, &config).unwrap();
    let own = pc.own.lock().unwrap().clone();
    assert_eq!(own.len(), 1);
    assert_eq!(own[0].exe_path, r"C:\NoSuchGames\Ice\Ice.exe");
    assert_eq!(own[0].name, "Ice");
    // The game starts with the app's arguments.
    assert_eq!(own[0].arguments, "-windowed");
    // One player, our handler's game, listed under the app.
    let listed = registry.sessions();
    assert_eq!(listed[0].session.kind, Kind::Background);
    assert_eq!(listed[0].session.slots.len(), 1);
    assert_eq!(listed[0].session.game, own[0].guid());
    assert_eq!(listed[0].app_id, apps[3].id());
    // Its owner returns to it without another write, Steam read or
    // folder scan.
    assert_eq!(pc.lookups.load(std::sync::atomic::Ordering::Relaxed), 1);
    registry.leave(seat, "a");
    assert_eq!(
        background(&registry, &apps[3], "b", &owner, &config).unwrap(),
        seat
    );
    assert_eq!(pc.lookups.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(pc.own.lock().unwrap().len(), 1);
    assert_eq!(pc.started.lock().unwrap().len(), 1);
    // Co-op still needs a hub handler: no entry, and a co-op launch is refused.
    assert!(
        registry
            .entries(&apps, "")
            .iter()
            .all(|coop| coop.app.name != "Ice")
    );
    let coop = CoopApp {
        app: apps[3].clone(),
        ..registry.entries(&apps, "").remove(0)
    };
    let refused = admit(&registry, &coop, "c", &client("B", true)).unwrap_err();
    assert_eq!(refused.status, 404);
    assert!(
        refused
            .message
            .starts_with("No Nucleus Co-op handler for Ice.exe"),
        "{refused}"
    );
}

#[test]
fn ours_runs_only_in_the_nucleus_rubylight_installs() {
    // Another release at nucleus_path may read another game list, where
    // our game would never show.
    let (registry, pc, mut apps) = setup();
    let owner = client("A", false);
    let config = Config::parse("nucleus_path = D:\\Nucleus\n").unwrap();
    apps.push(app("Ice", r"C:\NoSuchGames\Ice\Ice.exe"));
    let refused = background(&registry, &apps[3], "a", &owner, &config).unwrap_err();
    assert_eq!(refused.status, 404);
    assert_eq!(
        refused.message,
        "Ice can't stream in the background: Rubylight adds a game without a hub handler only to the Nucleus Co-op it installs, and nucleus_path names another: add the game's handler from Nucleus Co-op's hub, or clear nucleus_path"
    );
    assert!(pc.own.lock().unwrap().is_empty());
    assert!(pc.started.lock().unwrap().is_empty());
    // The game page says so before a launch, of a game on this PC: one
    // that is not says that first, as the launch does.
    let dir = std::env::temp_dir().join(format!("nucleus-own-{}", uuid::Uuid::new_v4()));
    let exe = dir.join("Ice").join("Ice.exe");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, b"").unwrap();
    let mut here = apps.clone();
    here[3] = app("Ice", &format!("\"{}\"", exe.display()));
    let statuses = registry.games(&here, &config).unwrap().unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(
        matches!(&statuses[3], GameStatus::Missing(Missing::Refused { reason }) if reason.contains("nucleus_path")),
        "{:?}",
        statuses[3]
    );
    // A hub handler's game streams, without our list's diagnosis.
    background(&registry, &apps[1], "b", &owner, &config).unwrap();
    assert_eq!(*pc.known_lists.lock().unwrap(), [false]);
    let first = registry.sessions()[0].session.id;
    assert!(registry.end(first));
    background(&registry, &apps[3], "c", &owner, &Config::default()).unwrap();
    assert_eq!(*pc.known_lists.lock().unwrap(), [false, true]);
}

#[test]
fn a_steam_game_with_ours_starts_nucleus_with_its_app_id() {
    let (registry, pc, _) = setup();
    let owner = client("A", false);
    let config = Config::default();
    let link = app("Black Flag", "steam://rungameid/242050");
    pc.one_player.lock().unwrap().insert(
        link.name.clone(),
        Setup::new(
            r"D:\Steam\steamapps\common\Black Flag\AC4BFSP.exe",
            &link.name,
            Some((242050, r"D:\Steam\steamapps\common\Black Flag")),
        ),
    );
    background(&registry, &link, "a", &owner, &config).unwrap();
    assert_eq!(pc.own.lock().unwrap().len(), 1);
    assert_eq!(*pc.steam_apps.lock().unwrap(), [Some(242050)]);
    // A game started by its own .exe gets none.
    let ice = app("Ice", r"C:\NoSuchGames\Ice\Ice.exe");
    let first = registry.sessions()[0].session.id;
    assert!(registry.end(first));
    background(&registry, &ice, "b", &owner, &config).unwrap();
    assert_eq!(*pc.steam_apps.lock().unwrap(), [Some(242050), None]);
}

/// The copy the fake PC makes for the signed-in user.
fn user_copy() -> PathBuf {
    PathBuf::from(format!(
        r"C:\ProgramData\Butterpollo\nucleus-user\{}\0123456789abcdef",
        fake::USER
    ))
}

#[test]
fn ours_runs_as_the_user_for_a_game_that_needs_no_administrator_rights() {
    let (registry, pc, _) = setup();
    let ice = app("Ice", r"C:\NoSuchGames\Ice\Ice.exe");
    // Least privilege: also on an account that could elevate, and on one
    // that can't.
    for (launch, account) in [
        ("a", Account::Elevates),
        ("b", Account::Standard),
        ("c", Account::Protected),
    ] {
        *pc.account.lock().unwrap() = account;
        let seat = background(
            &registry,
            &ice,
            launch,
            &client(launch, false),
            &Config::default(),
        )
        .unwrap();
        assert!(registry.end(seat.session));
    }
    assert_eq!(
        *pc.ready_runs.lock().unwrap(),
        [Run::User, Run::User, Run::User]
    );
    // Its handler and Nucleus in the user's copy, never the host's.
    assert_eq!(*pc.own_in.lock().unwrap(), vec![Some(user_copy()); 3]);
    assert_eq!(*pc.user_copies.lock().unwrap(), vec![Some(user_copy()); 3]);
    // The game's folder was walked at launch, as Nucleus will.
    assert_eq!(pc.walks.load(std::sync::atomic::Ordering::Relaxed), 3);
    // Made from the host's Nucleus at each launch, which checks it.
    assert_eq!(
        *pc.copied_from.lock().unwrap(),
        [
            PathBuf::from(r"C:\Nucleus"),
            PathBuf::from(r"C:\Nucleus"),
            PathBuf::from(r"C:\Nucleus")
        ]
    );
}

#[test]
fn no_users_copy_is_made_of_a_configured_nucleus() {
    // Rubylight's own handler runs only in the Nucleus Co-op it installs.
    let (registry, pc, _) = setup();
    let config = Config::parse("nucleus_path = D:\\Games\\Nucleus\n").unwrap();
    let refused = background(
        &registry,
        &app("Ice", r"C:\NoSuchGames\Ice\Ice.exe"),
        "a",
        &client("A", false),
        &config,
    )
    .unwrap_err();
    assert!(
        refused.message.contains("nucleus_path"),
        "{}",
        refused.message
    );
    assert!(pc.copied_from.lock().unwrap().is_empty());
    assert!(pc.user_copies.lock().unwrap().is_empty());
}

#[test]
fn the_users_copy_is_made_once_the_consoles_nucleus_closed() {
    use std::sync::atomic::Ordering::Relaxed;
    let (registry, pc, _) = setup();
    registry.open("").unwrap();
    let window = pc.window();
    let ice = app("Ice", r"C:\NoSuchGames\Ice\Ice.exe");
    let seat = background(
        &registry,
        &ice,
        "a",
        &client("A", false),
        &Config::default(),
    )
    .unwrap();
    assert!(registry.end(seat.session));
    assert!(!window.open.load(Relaxed));
    // Copied with no Nucleus Co-op running from the host's folder.
    assert_eq!(*pc.copied_while.lock().unwrap(), [None]);
    assert_eq!(*pc.user_copies.lock().unwrap(), [Some(user_copy())]);
}

#[test]
fn ours_runs_elevated_for_a_game_that_needs_administrator_rights() {
    let (registry, pc, _) = setup();
    let exe = r"C:\NoSuchGames\Ice\Ice.exe";
    let ice = app("Ice", exe);
    for (launch, need) in [
        ("a", Need::Administrator(Why::Manifest)),
        ("b", Need::Administrator(Why::Compatibility)),
        ("c", Need::Highest),
    ] {
        pc.needs.lock().unwrap().insert(exe.into(), need);
        let seat = background(
            &registry,
            &ice,
            launch,
            &client("A", false),
            &Config::default(),
        )
        .unwrap();
        assert!(registry.end(seat.session));
    }
    // Found only by walking its folder (a game under Program Files).
    pc.needs.lock().unwrap().clear();
    pc.folder_needs
        .lock()
        .unwrap()
        .insert(exe.into(), Need::Administrator(Why::Folder));
    background(
        &registry,
        &ice,
        "d",
        &client("A", false),
        &Config::default(),
    )
    .unwrap();
    assert_eq!(*pc.ready_runs.lock().unwrap(), [Run::Elevated; 4]);
    assert_eq!(*pc.own_in.lock().unwrap(), [None, None, None, None]);
    assert_eq!(*pc.user_copies.lock().unwrap(), [None, None, None, None]);
}

#[test]
fn an_admin_game_or_a_hub_handler_on_an_account_without_elevation_is_refused() {
    let (registry, pc, apps) = setup();
    *pc.account.lock().unwrap() = Account::Standard;
    let exe = r"C:\NoSuchGames\Ice\Ice.exe";
    pc.needs
        .lock()
        .unwrap()
        .insert(exe.into(), Need::Administrator(Why::Manifest));
    let refused = background(
        &registry,
        &app("Ice", exe),
        "a",
        &client("A", false),
        &Config::default(),
    )
    .unwrap_err();
    assert_eq!(refused.status, 503);
    assert_eq!(
        refused.message,
        format!(
            "Ice can't stream in the background: Nucleus Co-op did not start Ice: The game needs administrator rights (its manifest asks for them), so Nucleus Co-op runs elevated, and {}",
            elevation::STANDARD
        )
    );
    // Under Windows' Administrator protection: that the game can't run,
    // not to sign in as an administrator, which doesn't help.
    *pc.account.lock().unwrap() = Account::Protected;
    let refused = background(
        &registry,
        &app("Ice", exe),
        "p",
        &client("P", false),
        &Config::default(),
    )
    .unwrap_err();
    assert!(
            refused.message.ends_with(", and Windows' Administrator protection keeps them from Rubylight, so Nucleus Co-op can't run this game on this PC; games Rubylight adds itself still stream if they need no administrator rights"),
            "{refused:?}"
        );
    *pc.account.lock().unwrap() = Account::Standard;
    // A hub handler, whatever it says, background or co-op.
    let refused = background(
        &registry,
        &apps[1],
        "b",
        &client("B", false),
        &Config::default(),
    )
    .unwrap_err();
    assert!(
        refused
            .message
            .contains("needs administrator rights for a hub handler"),
        "{refused:?}"
    );
    let coop = registry.entries(&apps, "").remove(0);
    let refused = admit(&registry, &coop, "c", &client("C", true)).unwrap_err();
    assert!(
        refused
            .message
            .contains("needs administrator rights for a hub handler"),
        "{refused:?}"
    );
    // Nothing written, opened or started another way.
    assert!(pc.own.lock().unwrap().is_empty());
    assert!(pc.ready_runs.lock().unwrap().is_empty());
    assert!(pc.started.lock().unwrap().is_empty());
}

#[test]
fn a_hub_handler_runs_elevated_whatever_it_says() {
    let (registry, pc, apps) = setup();
    background(
        &registry,
        &apps[1],
        "a",
        &client("A", false),
        &Config::default(),
    )
    .unwrap();
    assert_eq!(*pc.ready_runs.lock().unwrap(), [Run::Elevated]);
    assert_eq!(*pc.user_copies.lock().unwrap(), [None]);
    // Its game's needs are never read.
    assert_eq!(pc.walks.load(std::sync::atomic::Ordering::Relaxed), 0);
}

#[test]
fn a_user_copy_that_is_not_made_refuses_the_stream() {
    let (registry, pc, _) = setup();
    registry.open("").unwrap();
    *pc.copy_error.lock().unwrap() = Some(
        r"an administrator of this PC can delete C:\ProgramData\Butterpollo\nucleus-user".into(),
    );
    let refused = background(
        &registry,
        &app("Ice", r"C:\NoSuchGames\Ice\Ice.exe"),
        "a",
        &client("A", false),
        &Config::default(),
    )
    .unwrap_err();
    assert_eq!(refused.status, 503);
    assert!(
        refused.message.contains(
            "the signed-in user's own copy of Nucleus Co-op was not made: an administrator"
        ),
        "{refused:?}"
    );
    assert!(pc.started.lock().unwrap().is_empty());
    // Made only once the console's Nucleus Co-op is closed.
    assert!(
        !pc.windows.lock().unwrap()[0]
            .open
            .load(std::sync::atomic::Ordering::Relaxed)
    );
}

#[test]
fn the_console_reads_whether_the_account_elevates_once_a_minute() {
    let (registry, pc, _) = setup();
    let reads = || pc.elevations.load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(registry.account(), Some(Account::Elevates));
    // The 3 s poll asks Windows once.
    *pc.account.lock().unwrap() = Account::Standard;
    assert_eq!(registry.account(), Some(Account::Elevates));
    assert_eq!(reads(), 1);
    // A minute later it reads the account again.
    registry.account.lock().unwrap().as_mut().unwrap().0 = Instant::now() - OWN_STATUS_TTL;
    assert_eq!(registry.account(), Some(Account::Standard));
    assert_eq!(reads(), 2);
    // One Rubylight could not tell is unknown, and asked again next time.
    registry.account.lock().unwrap().as_mut().unwrap().0 = Instant::now() - OWN_STATUS_TTL;
    *pc.account_error.lock().unwrap() = Some("nobody is signed in".into());
    assert_eq!(registry.account(), None);
    *pc.account_error.lock().unwrap() = None;
    assert_eq!(registry.account(), Some(Account::Standard));
    assert_eq!(reads(), 4);
}

#[test]
fn the_console_says_how_ours_runs_or_why_it_cant() {
    let (registry, pc, _) = setup();
    let dir = std::env::temp_dir().join(format!("nucleus-own-{}", uuid::Uuid::new_v4()));
    let exe = dir.join("Ice").join("Ice.exe");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, b"").unwrap();
    let apps = [app("Ice", &format!("\"{}\"", exe.display()))];
    let path = exe.to_string_lossy().into_owned();
    let status = || {
        registry.own_statuses.lock().unwrap().clear();
        registry.games(&apps, &Config::default()).unwrap().unwrap()[0].clone()
    };
    let run = |status: GameStatus| match status {
        GameStatus::Own { run, .. } => Ok(run),
        GameStatus::Missing(missing) => Err(missing.to_string()),
        other => Err(format!("{other:?}")),
    };
    assert_eq!(run(status()), Ok(Run::User));
    pc.needs
        .lock()
        .unwrap()
        .insert(path.clone(), Need::Administrator(Why::Compatibility));
    assert_eq!(run(status()), Ok(Run::Elevated));
    *pc.account.lock().unwrap() = Account::Standard;
    let refused = run(status()).unwrap_err();
    assert!(
            refused.starts_with("The game needs administrator rights (its compatibility settings run it as administrator)"),
            "{refused}"
        );
    // "The most the account has" is the user's own rights without elevation.
    pc.needs.lock().unwrap().insert(path, Need::Highest);
    assert_eq!(run(status()), Ok(Run::User));
    // The console never walks the game's folder.
    assert_eq!(pc.walks.load(std::sync::atomic::Ordering::Relaxed), 0);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_hub_handler_always_wins_over_ours() {
    let (registry, pc, apps) = setup();
    let owner = client("A", false);
    let config = Config::default();
    // Portal 2 has the hub's handler and the user added it.
    let seat = background(&registry, &apps[1], "a", &owner, &config).unwrap();
    assert_eq!(registry.sessions()[0].session.game, fake::GUID);
    assert!(pc.own.lock().unwrap().is_empty());
    assert!(registry.end(seat.session));
    // Found from a link rather than the app's command: still the hub's.
    let link = app("Portal 2 Link", "steam://rungameid/620");
    pc.one_player.lock().unwrap().insert(
        link.name.clone(),
        Setup::new(fake::EXE, &link.name, Some((620, r"C:\Games\Portal 2"))),
    );
    background(&registry, &link, "b", &owner, &config).unwrap();
    assert_eq!(pc.last().session.lock().unwrap().game, fake::GUID);
    assert!(pc.own.lock().unwrap().is_empty());
    // A hub handler sets up Steam as it needs: Nucleus gets no app id.
    assert_eq!(*pc.steam_apps.lock().unwrap(), [None, None]);
    // A hub handler for a game the user did not add: ours streams it.
    for listed in registry.sessions() {
        registry.end(listed.session.id);
    }
    pc.catalog.lock().unwrap().games.clear();
    background(&registry, &apps[1], "c", &owner, &config).unwrap();
    assert_eq!(pc.own.lock().unwrap().len(), 1);
}

#[test]
fn ours_goes_once_the_user_added_the_game_with_a_hub_handler() {
    let (registry, pc, apps) = setup();
    let owner = client("A", false);
    let config = Config::default();
    // Ours from before the hub's handler came, for the same .exe.
    let ours = Setup::new(fake::EXE, "Portal 2", None);
    {
        let mut catalog = pc.catalog.lock().unwrap();
        let hub = catalog.handlers[0].clone();
        catalog.handlers.push(Handler {
            file: ours.file(),
            guid: ours.guid(),
            own: true,
            ..hub
        });
        catalog
            .games
            .push(butterpollo_core::nucleus::handler::Game {
                guid: ours.guid(),
                exe_path: fake::EXE.into(),
            });
    }
    // A refused launch removes nothing: Nucleus' window stays open.
    *pc.free.lock().unwrap() = 0;
    background(&registry, &apps[1], "a", &owner, &config).unwrap_err();
    assert!(pc.removed.lock().unwrap().is_empty());
    *pc.free.lock().unwrap() = 4;
    background(&registry, &apps[1], "a", &owner, &config).unwrap();
    assert_eq!(registry.sessions()[0].session.game, fake::GUID);
    assert_eq!(*pc.removed.lock().unwrap(), [ours.file()]);
    // Checked, with the list it edits, before the window closed.
    assert_eq!(
        pc.ready_stale.lock().unwrap().last().unwrap(),
        &[ours.file()]
    );
    // Nucleus starts the game the user added, at its path.
    assert_eq!(*pc.exe_paths.lock().unwrap(), [fake::EXE]);
}

#[test]
fn ours_goes_as_soon_as_a_hub_handler_for_its_exe_is_installed() {
    // Nucleus' add-game search skips a path in its list: our entry would
    // keep the user from adding the game with the hub's handler.
    let (registry, pc, apps) = setup();
    let owner = client("A", false);
    let config = Config::default();
    let ice = app("Ice", r"C:\Games\Ice\portal2.exe");
    let ours = Setup::new(r"C:\Games\Ice\portal2.exe", "Ice", None);
    let unused = Setup::new(r"C:\Games\Old\Old.exe", "Old", None);
    {
        let mut catalog = pc.catalog.lock().unwrap();
        for setup in [&ours, &unused] {
            catalog.handlers.push(
                butterpollo_core::nucleus::handler::parse(
                    &setup.file(),
                    &setup.handler_of(Some(fake::USER)),
                )
                .unwrap(),
            );
        }
        catalog
            .games
            .push(butterpollo_core::nucleus::handler::Game {
                guid: ours.guid(),
                exe_path: ours.exe_path.clone(),
            });
    }
    // Ice itself streams with ours (the hub's portal2.exe is another
    // game's); only the unused one goes.
    let seat = background(&registry, &ice, "a", &owner, &config).unwrap();
    assert_eq!(*pc.removed.lock().unwrap(), [unused.file()]);
    assert_eq!(
        *pc.exe_paths.lock().unwrap(),
        std::slice::from_ref(&ours.exe_path)
    );
    assert!(registry.end(seat.session));
    // Another session's start removes ours; a failure is the stream's
    // warning, and the session still starts.
    pc.removed.lock().unwrap().clear();
    *pc.remove_error.lock().unwrap() = Some("access denied".into());
    let warnings = Arc::new(Warnings::default());
    let coop = registry.entries(&apps, "").remove(0);
    registry
        .admit(Admission {
            launch: "b",
            client: &owner,
            game: Game::Coop(&coop),
            size: (1280, 720),
            config: &config,
            warnings: warnings.clone(),
        })
        .unwrap();
    let shown = warnings.snapshot();
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert_eq!(shown[0].code, HANDLER_NOT_REMOVED);
    assert_eq!(
        shown[0].message,
        format!(
            "Rubylight's Nucleus Co-op handlers {}, {} were not fully removed: access denied",
            ours.file(),
            unused.file()
        )
    );
    assert_eq!(pc.started.lock().unwrap().len(), 2);
    *pc.remove_error.lock().unwrap() = None;
    for listed in registry.sessions() {
        registry.end(listed.session.id);
    }
    admit(&registry, &coop, "c", &client("C", false)).unwrap();
    assert_eq!(*pc.removed.lock().unwrap(), [ours.file(), unused.file()]);
}

#[test]
fn a_steam_link_passes_only_its_launch_options_arguments() {
    // An imported Steam game's command handed cmd's own arguments on.
    assert_eq!(
        steam_arguments("-game portal", r#"cmd /c start "" steam://rungameid/400"#),
        "-game portal"
    );
    assert_eq!(
        steam_arguments(" -game portal ", "steam://rungameid/400  -novid"),
        "-game portal -novid"
    );
    assert_eq!(steam_arguments("", "steam://rungameid/400"), "");
}

#[test]
fn a_launcher_is_refused_and_an_unreal_stub_starts_its_game() {
    let (registry, pc, _) = setup();
    let owner = client("A", false);
    let config = Config::default();
    // Steam would start the game outside Nucleus' hooks.
    let steam = app(
        "Portal",
        r#""C:\Program Files (x86)\Steam\steam.exe" -applaunch 400"#,
    );
    let refused = background(&registry, &steam, "a", &owner, &config).unwrap_err();
    assert_eq!(refused.status, 404);
    assert_eq!(
        refused.message,
        "Portal can't stream in the background: steam.exe is a launcher, not the game: set the app's command to the game's .exe"
    );
    assert!(pc.own.lock().unwrap().is_empty());
    // The stub at an Unreal game's root only starts its Shipping .exe.
    let root = std::env::temp_dir().join(format!("nucleus-ue-{}", uuid::Uuid::new_v4()));
    let win64 = root.join("Stray").join("Binaries").join("Win64");
    std::fs::create_dir_all(&win64).unwrap();
    std::fs::create_dir_all(root.join("Engine")).unwrap();
    std::fs::write(win64.join("Stray-Win64-Shipping.exe"), b"").unwrap();
    let stub = root.join("Stray.exe");
    std::fs::write(&stub, b"").unwrap();
    let stray = app("Stray", &format!("\"{}\" -dx12", stub.display()));
    let started = background(&registry, &stray, "b", &owner, &config);
    std::fs::remove_dir_all(&root).unwrap();
    started.unwrap();
    let own = pc.own.lock().unwrap().clone();
    assert_eq!(
        own[0].exe_path,
        win64.join("Stray-Win64-Shipping.exe").to_string_lossy()
    );
    assert_eq!(own[0].arguments, "-dx12");
}

#[test]
fn another_games_exe_of_the_same_file_name_streams_with_ours() {
    // Portal's hl2.exe started Half-Life 2's hub handler by its name.
    let (registry, pc, _) = setup();
    let mod_game = app(
        "Portal 2 Mod",
        r#""C:\NoSuchGames\Portal 2 Mod\portal2.exe" -game mod"#,
    );
    background(
        &registry,
        &mod_game,
        "a",
        &client("A", false),
        &Config::default(),
    )
    .unwrap();
    let own = pc.own.lock().unwrap().clone();
    assert_eq!(own.len(), 1);
    assert_eq!(own[0].exe_path, r"C:\NoSuchGames\Portal 2 Mod\portal2.exe");
    assert_eq!(registry.sessions()[0].session.game, own[0].guid());
}

#[test]
fn a_link_to_another_game_of_the_same_file_name_streams_with_ours() {
    // Portal's hl2.exe found Half-Life 2's handler by its file name.
    let (registry, pc, _) = setup();
    let owner = client("A", false);
    let config = Config::default();
    let link = app("Portal 2 Mod", "steam://rungameid/12345");
    let exe = r"C:\Games\Portal 2 Mod\portal2.exe";
    pc.one_player.lock().unwrap().insert(
        link.name.clone(),
        Setup::new(exe, &link.name, Some((12345, r"C:\Games\Portal 2 Mod"))),
    );
    background(&registry, &link, "a", &owner, &config).unwrap();
    let own = pc.own.lock().unwrap().clone();
    assert_eq!(own.len(), 1);
    assert_eq!(own[0].exe_path, exe);
    assert_eq!(registry.sessions()[0].session.game, own[0].guid());
}

#[test]
fn a_background_stream_of_an_anti_cheat_game_is_refused_and_never_hooked() {
    let (registry, pc, _) = setup();
    let owner = client("A", false);
    let config = Config::default();
    let dir = std::env::temp_dir().join(format!("nucleus-ac-{}", uuid::Uuid::new_v4()));
    let game = dir.join("Shooter");
    std::fs::create_dir_all(game.join("EasyAntiCheat")).unwrap();
    let shooter = app(
        "Shooter",
        &format!("\"{}\"", game.join("Shooter.exe").display()),
    );
    let refused = background(&registry, &shooter, "a", &owner, &config).unwrap_err();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(refused.status, 404);
    assert_eq!(
        refused.message,
        "Shooter can't stream in the background: Shooter.exe uses Easy Anti-Cheat, and Rubylight never hooks a game with anti-cheat"
    );
    assert!(pc.own.lock().unwrap().is_empty());
    assert!(pc.started.lock().unwrap().is_empty());
    // Steam lists Valve Anti-Cheat for a game whose files say nothing.
    let link = app("Portal", "steam://rungameid/400");
    pc.one_player.lock().unwrap().insert(
        link.name.clone(),
        Setup {
            anti_cheat: Some("Valve Anti-Cheat"),
            ..Setup::new(
                r"Z:\NoSuch\common\Portal\hl2.exe",
                &link.name,
                Some((400, r"Z:\NoSuch\common\Portal")),
            )
        },
    );
    let refused = background(&registry, &link, "b", &owner, &config).unwrap_err();
    assert_eq!(
        refused.message,
        "Portal can't stream in the background: hl2.exe uses Valve Anti-Cheat, and Rubylight never hooks a game with anti-cheat"
    );
    assert!(pc.own.lock().unwrap().is_empty());
}

#[test]
fn our_handler_is_never_written_while_nucleus_runs_or_when_it_fails() {
    let (registry, pc, _) = setup();
    let owner = client("A", false);
    let config = Config::default();
    let ice = app("Ice", r"C:\NoSuchGames\Ice\Ice.exe");
    *pc.foreign.lock().unwrap() = Some(77);
    let refused = background(&registry, &ice, "a", &owner, &config).unwrap_err();
    assert_eq!(refused.status, 503);
    assert!(
            refused
                .message
                .starts_with("Ice can't stream in the background: Rubylight's Nucleus Co-op handler for Ice was not written: "),
            "{refused}"
        );
    assert!(pc.own.lock().unwrap().is_empty());
    *pc.foreign.lock().unwrap() = None;
    *pc.write_error.lock().unwrap() = Some("access denied".into());
    let refused = background(&registry, &ice, "b", &owner, &config).unwrap_err();
    assert!(
        refused.message.ends_with("was not written: access denied"),
        "{refused}"
    );
    assert!(pc.started.lock().unwrap().is_empty());
}

#[test]
fn another_game_is_busy_while_a_session_runs() {
    let (registry, pc, mut apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    admit(&registry, &coop, "a", &client("A", false)).unwrap();
    {
        let mut catalog = pc.catalog.lock().unwrap();
        let mut handler = catalog.handlers[0].clone();
        handler.guid = "Racer".into();
        handler.executable = "racer.exe".into();
        catalog.handlers.push(handler);
        catalog
            .games
            .push(butterpollo_core::nucleus::handler::Game {
                guid: "Racer".into(),
                exe_path: r"C:\Games\racer.exe".into(),
            });
    }
    apps.push(app("Racer", r"C:\Games\racer.exe"));
    registry.use_pc(pc.clone());
    let racer = registry
        .entries(&apps, "")
        .into_iter()
        .find(|coop| coop.app.name == "Racer")
        .unwrap();
    let busy = admit(&registry, &racer, "b", &client("B", true)).unwrap_err();
    assert_eq!(busy.status, 409);
    assert!(busy.message.contains("one game at a time"), "{busy}");
}

#[test]
fn refusals_carry_their_status() {
    let (registry, pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    let a = client("A", false);
    // Removed from Nucleus since the library was read: 404 with the reason.
    pc.catalog.lock().unwrap().games.clear();
    let missing = admit(&registry, &coop, "a", &a).unwrap_err();
    assert_eq!(missing.status, 404);
    assert!(
        missing.message.contains("the game is not added"),
        "{missing}"
    );
    pc.catalog.lock().unwrap().handlers.clear();
    let missing = admit(&registry, &coop, "a", &a).unwrap_err();
    assert_eq!(
        missing,
        Refusal::missing(&Missing::NoHandler {
            executable: "portal2.exe".into()
        })
    );
    // Unavailable here: 503 with the reason, before any install.
    *pc.unavailable.lock().unwrap() = Some("no service".into());
    *pc.installed.lock().unwrap() = None;
    assert_eq!(
        admit(&registry, &coop, "a", &a).unwrap_err(),
        Refusal::unavailable("no service")
    );
    assert_eq!(pc.installs.load(std::sync::atomic::Ordering::Relaxed), 0);
}

#[test]
fn an_install_runs_at_the_first_launch_and_a_failure_shows() {
    let (registry, pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    *pc.installed.lock().unwrap() = None;
    *pc.install_error.lock().unwrap() = Some("hash mismatch".into());
    let failed = admit(&registry, &coop, "a", &client("A", false)).unwrap_err();
    assert_eq!(failed.status, 503);
    assert_eq!(failed.message, "Nucleus Co-op is not ready: hash mismatch");
    assert_eq!(registry.install(""), Install::Failed(failed.message));
    *pc.install_error.lock().unwrap() = None;
    admit(&registry, &coop, "a", &client("A", false)).unwrap();
    assert_eq!(pc.installs.load(std::sync::atomic::Ordering::Relaxed), 2);
    assert_eq!(
        registry.install(""),
        Install::Installed(PathBuf::from(r"C:\Nucleus"))
    );
    assert_eq!(
        registry.install(r"D:\Nucleus"),
        Install::Configured(PathBuf::from(r"D:\Nucleus"))
    );
}

#[test]
fn a_failed_start_or_no_free_pad_is_unavailable() {
    let (registry, pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    *pc.free.lock().unwrap() = 0;
    let no_pad = admit(&registry, &coop, "a", &client("A", false)).unwrap_err();
    assert_eq!(no_pad.status, 503);
    assert!(no_pad.message.contains("XInput"), "{no_pad}");
    *pc.free.lock().unwrap() = 1;
    *pc.start_error.lock().unwrap() = Some("Nucleus Co-op is already running".into());
    let failed = admit(&registry, &coop, "a", &client("A", false)).unwrap_err();
    assert_eq!(failed.status, 503);
    assert!(failed.message.contains("already running"), "{failed}");
    assert!(registry.sessions().is_empty());
    // One free pad: a one-player session.
    *pc.start_error.lock().unwrap() = None;
    admit(&registry, &coop, "a", &client("A", false)).unwrap();
    assert_eq!(pc.last().session.lock().unwrap().slots.len(), 1);
}

#[test]
fn only_the_launch_holding_a_slot_leaves_it() {
    let (registry, pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    let a = client("A", false);
    let first = admit(&registry, &coop, "a1", &a).unwrap();
    // The same device launches again: the newer launch holds the slot.
    assert_eq!(admit(&registry, &coop, "a2", &a).unwrap(), first);
    registry.leave(first, "a1");
    let session = pc.last();
    let away = |session: &fake::Instance| session.session.lock().unwrap().players()[0].away;
    assert!(!away(&session));
    assert_eq!(registry.sessions()[0].held, [true, false]);
    registry.leave(first, "a2");
    assert!(away(&session));
    assert_eq!(registry.sessions()[0].held, [false, false]);
    // Leaving twice changes nothing.
    registry.leave(first, "a2");
    // Back within the grace: its own slot.
    assert_eq!(admit(&registry, &coop, "a3", &a).unwrap(), first);
    assert!(!away(&session));
}

#[test]
fn a_join_from_the_list_holds_the_slot_until_the_launch() {
    let (registry, pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    let a = client("A", false);
    admit(&registry, &coop, "a", &a).unwrap();
    assert_eq!(registry.join(9, &a), Err(JoinError::NoSession));
    assert_eq!(
        registry.join(1, &client("B", false)),
        Err(JoinError::Refused(Refused::NotPermitted))
    );
    let b = client("B", true);
    assert_eq!(registry.join(1, &b), Ok((1, coop.id())));
    let session = pc.last();
    assert!(session.session.lock().unwrap().players()[1].away);
    assert_eq!(
        admit(&registry, &coop, "b", &b).unwrap(),
        Seat {
            session: 1,
            slot: 1
        }
    );
    assert!(!session.session.lock().unwrap().players()[1].away);
    // The streaming owner asking again is not marked away.
    assert_eq!(registry.join(1, &a), Ok((0, coop.id())));
    assert!(!session.session.lock().unwrap().players()[0].away);
    // Join co-op taken away: the player cannot come back to its slot,
    // from the list or by launching the entry; the owner still can.
    let revoked = client("B", false);
    assert_eq!(
        registry.join(1, &revoked),
        Err(JoinError::Refused(Refused::NotPermitted))
    );
    assert_eq!(
        admit(&registry, &coop, "b2", &revoked).unwrap_err(),
        Refusal::busy(Refused::NotPermitted.to_string())
    );
    assert_eq!(registry.join(1, &client("A", false)), Ok((0, coop.id())));
}

#[test]
fn kick_and_end_reach_the_session() {
    let (registry, pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    let b = client("B", true);
    admit(&registry, &coop, "a", &client("A", false)).unwrap();
    admit(&registry, &coop, "b", &b).unwrap();
    assert_eq!(registry.kick(1, 1).as_deref(), Some("B"));
    assert_eq!(registry.kick(1, 1), None);
    assert_eq!(registry.kick(7, 0), None);
    assert_eq!(registry.sessions()[0].held, [true, false]);
    assert_eq!(
        admit(&registry, &coop, "b2", &b).unwrap_err(),
        Refusal::busy(
            Refused::Kicked {
                title: "Portal 2".into()
            }
            .to_string()
        )
    );
    assert!(!registry.end(7));
    assert!(registry.end(1));
    let session = pc.last();
    assert!(session.stopped());
    let listed = registry.sessions();
    assert_eq!(listed[0].session.phase, Phase::Ended(End::Ended));
    assert_eq!(
        registry.join(1, &b),
        Err(JoinError::Refused(Refused::Ended {
            title: "Portal 2".into()
        }))
    );
    // An ended session no longer holds Nucleus: the next launch starts anew.
    assert_eq!(
        admit(&registry, &coop, "c", &client("C", false)).unwrap(),
        Seat {
            session: 2,
            slot: 0
        }
    );
    assert_eq!(pc.started.lock().unwrap().len(), 2);
}

#[test]
fn a_session_that_ends_by_itself_is_stopped_once_seen() {
    let (registry, pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    let seat = admit(&registry, &coop, "a", &client("A", false)).unwrap();
    let session = pc.last();
    session
        .session
        .lock()
        .unwrap()
        .end(End::Crashed("gone".into()));
    assert!(!session.stopped());
    registry.leave(seat, "a");
    assert!(session.stopped());
    // Still listed for a while so the console can say why.
    assert_eq!(registry.sessions().len(), 1);
}

fn pad(id: u16, active: u16, buttons: u32) -> Input {
    Input::Controller {
        id,
        active,
        buttons,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    }
}
fn codes(warnings: &Warnings) -> Vec<String> {
    warnings.snapshot().into_iter().map(|w| w.code).collect()
}

#[test]
fn a_player_drives_its_slot_and_hears_of_each_dropped_kind_once() {
    let (registry, pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    admit(&registry, &coop, "a", &client("A", false)).unwrap();
    let seat = admit(&registry, &coop, "b", &client("B", true)).unwrap();
    pc.last().show();
    let mut player = Player {
        instance: registry.instance(seat.session).unwrap(),
        seat,
        device: "B".into(),
        rumble: true,
        told: Vec::new(),
    };
    let warnings = Warnings::default();
    player.input(pad(0, 1, 0x1000), &warnings);
    let key = Input::Keyboard {
        key: 0x41,
        modifiers: 0,
        flags: 0,
        down: true,
    };
    player.input(key.clone(), &warnings);
    player.input(key, &warnings);
    assert_eq!(codes(&warnings), ["coop_keyboard_mouse"]);
    player.input(pad(1, 0b11, 0x1000), &warnings);
    player.input(pad(1, 0b11, 0), &warnings);
    assert_eq!(
        codes(&warnings),
        ["coop_keyboard_mouse", "coop_second_controller"]
    );
    assert_eq!(player.told.len(), 2);
    // Only the first controller reached the pad, as slot 1's.
    let inputs = pc.last().inputs.lock().unwrap().clone();
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0].0, 1);
    assert!(matches!(
        inputs[0].1,
        Input::Controller {
            buttons: 0x1000,
            ..
        }
    ));
}

#[test]
fn rumble_reaches_a_player_only_where_its_stream_forwards_it() {
    let (registry, pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    let seat = admit(&registry, &coop, "a", &client("A", false)).unwrap();
    let mut player = Player {
        instance: registry.instance(seat.session).unwrap(),
        seat,
        device: "A".into(),
        rumble: false,
        told: Vec::new(),
    };
    assert!(!player.feedback_allowed(0x010b));
    assert!(player.feedback_allowed(0x5502));
    player.rumble = true;
    assert!(player.feedback_allowed(0x010b));
    drop(pc);
}

#[test]
fn each_slot_reads_its_own_feedback_whoever_drains_the_pads() {
    let (registry, pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    let a = admit(&registry, &coop, "a", &client("A", false)).unwrap();
    let b = admit(&registry, &coop, "b", &client("B", true)).unwrap();
    let rumble = |id, data: u8| PadReport::Feedback {
        id,
        kind: 0x010b,
        data: vec![data],
    };
    pc.last()
        .reports
        .lock()
        .unwrap()
        .extend([(0, rumble(0, 1)), (1, rumble(0, 2))]);
    let data = |reports: Vec<PadReport>| -> Vec<u8> {
        reports
            .into_iter()
            .filter_map(|r| match r {
                PadReport::Feedback { data, .. } => Some(data[0]),
                _ => None,
            })
            .collect()
    };
    assert_eq!(data(registry.feedback(a)), [1]);
    // Slot 1's report waited for its own stream.
    assert_eq!(data(registry.feedback(b)), [2]);
    assert!(registry.feedback(b).is_empty());
    // A stream that stopped reading keeps only the newest reports.
    pc.last()
        .reports
        .lock()
        .unwrap()
        .extend((0..FEEDBACK_KEPT as u8 + 4).map(|k| (1, rumble(0, k))));
    assert!(registry.feedback(a).is_empty());
    let kept = data(registry.feedback(b));
    assert_eq!(kept.len(), FEEDBACK_KEPT);
    assert_eq!(kept[0], 4);
}

#[test]
fn the_first_window_is_waited_for_and_its_absence_is_refused() {
    let (registry, pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    let seat = admit(&registry, &coop, "a", &client("A", false)).unwrap();
    let instance = registry.instance(seat.session).unwrap();
    let none = first_window(instance.as_ref(), 0, "d", Duration::ZERO).unwrap_err();
    assert!(
        format!("{none:#}").contains("showed no window for Player 1 within 0 s"),
        "{none:#}"
    );
    assert_eq!(none.downcast_ref::<Refusal>().unwrap().status, 503);
    let shown = std::thread::spawn({
        let session = pc.last();
        move || {
            std::thread::sleep(Duration::from_millis(50));
            session.show();
        }
    });
    let first = first_window(instance.as_ref(), 0, "d", Duration::from_secs(10)).unwrap();
    assert_eq!(first, "window:100@d");
    shown.join().unwrap();
    pc.last()
        .session
        .lock()
        .unwrap()
        .end(End::Crashed("gone".into()));
    let ended = first_window(instance.as_ref(), 0, "d", Duration::from_secs(10)).unwrap_err();
    assert!(
        format!("{ended:#}").contains("co-op session ended: "),
        "{ended:#}"
    );
}

#[test]
fn a_player_stream_leaves_the_desktop_input_and_clipboard_alone() {
    let mut config = Config::default();
    assert!(!player(&config));
    isolate(&mut config);
    assert!(player(&config));
    assert!(!config.boolean("keyboard", true));
    assert!(!config.boolean("mouse", true));
    assert_eq!(config.get("audio_capture_mode", "system"), "game");
    // Not a background stream: no isolated display is made for it.
    assert!(!butterpollo_core::background::enabled(&config));
    let mut owner = client("A", false);
    owner.perm = u32::MAX;
    assert!(!crate::stream::clipboard_sync::allowed(&config, &owner).any());
}

#[test]
fn the_stream_says_when_windows_cannot_capture_a_players_audio() {
    let warnings = Warnings::default();
    warn_audio(None, &warnings);
    warn_audio(Some(22631), &warnings);
    assert!(codes(&warnings).is_empty());
    warn_audio(Some(19045), &warnings);
    assert_eq!(codes(&warnings), ["coop_audio"]);
    assert!(warnings.snapshot()[0].message.contains("20348"));
}

/// The app a hardware case streams in the background: the game's .exe as
/// its command, or a Steam game's link.
fn case_app(game: Option<&str>, steam_app: Option<u32>) -> Result<App> {
    match (game, steam_app) {
        (Some(game), None) => {
            let name = Path::new(game)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .context("the game's .exe has no name")?;
            Ok(app(&name, &format!("\"{game}\"")))
        }
        (None, Some(id)) => Ok(app(
            &format!("Steam app {id}"),
            &format!("steam://rungameid/{id}"),
        )),
        _ => anyhow::bail!("set RUBYLIGHT_NUCLEUS_GAME or RUBYLIGHT_NUCLEUS_STEAM_APP"),
    }
}

#[test]
fn a_hardware_case_streams_the_exe_or_the_steam_link_as_an_app_would() {
    let exe = case_app(Some(r"D:\games\BFME1\lotrbfme.exe"), None).unwrap();
    assert_eq!(exe.name, "lotrbfme");
    assert!(starts_exe(&exe));
    assert_eq!(executable(&exe), Some(r"D:\games\BFME1\lotrbfme.exe"));
    let link = case_app(None, Some(2155180)).unwrap();
    assert!(!starts_exe(&link));
    assert_eq!(
        butterpollo_core::steam::game_of(&serde_json::to_value(&link).unwrap()),
        Some(2155180)
    );
    assert!(case_app(None, None).is_err());
    assert!(case_app(Some("a.exe"), Some(1)).is_err());
}

/// The share of a BGRA frame's pixels that are not near black.
fn lit(image: &butterpollo_windows::capture::Image) -> Result<f64> {
    ensure!(
        image.pixel == butterpollo_windows::capture::Pixel::Bgra8,
        "a {:?} frame, not BGRA",
        image.pixel
    );
    let (width, height) = (image.width as usize, image.height as usize);
    ensure!(
        width > 0 && height > 0 && image.bytes.len() >= image.stride * (height - 1) + width * 4,
        "a {width}x{height} frame of {} bytes",
        image.bytes.len()
    );
    let (mut seen, mut bright) = (0usize, 0usize);
    for y in (0..height).step_by(4) {
        let row = &image.bytes[y * image.stride..][..width * 4];
        for pixel in row.as_chunks::<4>().0.iter().step_by(4) {
            seen += 1;
            bright += usize::from(pixel[..3].iter().any(|&c| c > 24));
        }
    }
    Ok(bright as f64 / seen as f64)
}

#[test]
fn a_black_frame_is_told_from_a_picture() {
    let frame = |fill: &dyn Fn(usize) -> [u8; 4]| butterpollo_windows::capture::Image {
        width: 64,
        height: 32,
        stride: 64 * 4 + 16,
        bytes: (0..(64 * 4 + 16) * 32)
            .map(|at| fill(at % (64 * 4 + 16) / 4)[at % 4])
            .collect(),
        captured: Instant::now(),
        pixel: butterpollo_windows::capture::Pixel::Bgra8,
    };
    assert_eq!(lit(&frame(&|_| [8, 8, 8, 255])).unwrap(), 0.0);
    assert_eq!(lit(&frame(&|_| [0, 0, 200, 255])).unwrap(), 1.0);
    // The left half lit.
    let half = lit(&frame(&|x| if x < 32 { [90, 90, 90, 255] } else { [0; 4] })).unwrap();
    assert!((0.4..0.6).contains(&half), "{half}");
    let mut hdr = frame(&|_| [0; 4]);
    hdr.pixel = butterpollo_windows::capture::Pixel::RgbaF16;
    assert!(lit(&hdr).is_err());
}

/// Where Nucleus sets the DPI mode for the game at `game`: that exact
/// path, and the instances of its `executable` below the `content` folder
/// of each Nucleus copy it may run from (the host's, the user's).
fn nucleus_sets(name: &str, game: &str, contents: &[PathBuf], executable: &str) -> bool {
    let plain = |path: &str| path.trim().replace('/', "\\").to_lowercase();
    let name = plain(name);
    let below = |content: &PathBuf| {
        name.strip_prefix(plain(&content.to_string_lossy()).trim_end_matches('\\'))
            .is_some_and(|rest| rest.starts_with('\\'))
    };
    name == plain(game)
        || (contents.iter().any(below) && file_name(&name).eq_ignore_ascii_case(executable))
}

/// The compatibility settings the case changes back to `before`: each
/// value Nucleus sets for the game at `game` ([`nucleus_sets`]) that is
/// not as it was, with the data it gets, None to remove it. Another
/// program's, even one of the same file name, is never touched.
fn layer_changes(
    before: &BTreeMap<String, butterpollo_windows::nucleus::layers::Value>,
    after: &BTreeMap<String, butterpollo_windows::nucleus::layers::Value>,
    game: &str,
    contents: &[PathBuf],
    executable: &str,
) -> Vec<(String, Option<butterpollo_windows::nucleus::layers::Value>)> {
    let named = |name: &&String| nucleus_sets(name, game, contents, executable);
    let mut changes: Vec<_> = after
        .iter()
        .filter(|(name, _)| named(name))
        .filter(|(name, value)| before.get(*name) != Some(*value))
        .map(|(name, _)| (name.clone(), before.get(name).cloned()))
        .collect();
    changes.extend(
        before
            .iter()
            .filter(|(name, _)| named(name) && !after.contains_key(*name))
            .map(|(name, value)| (name.clone(), Some(value.clone()))),
    );
    changes
}

#[test]
fn a_hardware_case_puts_back_only_its_games_compatibility_settings() {
    use butterpollo_windows::nucleus::layers::Value;
    let text = |byte: u8| Value {
        kind: 1,
        data: vec![byte, 0, 0, 0],
    };
    let content = [PathBuf::from(r"C:\Nucleus\content")];
    let content = &content[..];
    let before = BTreeMap::from([
        (r"D:\G\Game.exe".to_string(), text(1)),
        (r"D:\Gone\Game.exe".to_string(), text(2)),
        (r"C:\Other.exe".to_string(), text(3)),
        (
            r"C:\Nucleus\content\G\Instance1\Game.exe".to_string(),
            text(4),
        ),
    ]);
    let after = BTreeMap::from([
        (r"d:/g/game.EXE".to_string(), text(9)),
        (
            r"C:\Nucleus\content\G\Instance0\GAME.exe".to_string(),
            text(9),
        ),
        (
            r"C:\Nucleus\content\G\Instance0\Tool.exe".to_string(),
            text(9),
        ),
        (r"C:\Nucleus\contents\Game.exe".to_string(), text(9)),
        (r"E:\Mine\Game.exe".to_string(), text(9)),
        (r"C:\Other.exe".to_string(), text(9)),
    ]);
    assert_eq!(
        layer_changes(&before, &after, r"D:\G\Game.exe", content, "game.exe"),
        [
            (r"C:\Nucleus\content\G\Instance0\GAME.exe".to_string(), None),
            (r"d:/g/game.EXE".to_string(), None),
            (
                r"C:\Nucleus\content\G\Instance1\Game.exe".to_string(),
                Some(text(4))
            ),
            (r"D:\G\Game.exe".to_string(), Some(text(1))),
        ]
    );
    assert!(layer_changes(&before, &before, r"D:\G\Game.exe", content, "game.exe").is_empty());
    // A session from the user's own copy sets its instances below that
    // copy's content folder; the host's still count.
    let user = r"C:\ProgramData\Butterpollo\nucleus-user\S-1-5-21-7\2.4.2\content";
    let both = [PathBuf::from(r"C:\Nucleus\content"), PathBuf::from(user)];
    let after = BTreeMap::from([
        (format!(r"{user}\G\Instance0\Game.exe"), text(9)),
        (
            r"C:\Nucleus\content\G\Instance0\Game.exe".to_string(),
            text(9),
        ),
        (format!(r"{user}\G\Instance0\Tool.exe"), text(9)),
        (format!(r"{user}s\Game.exe"), text(9)),
    ]);
    let empty = BTreeMap::new();
    assert_eq!(
        layer_changes(&empty, &after, r"D:\G\Game.exe", &both, "game.exe"),
        [
            (r"C:\Nucleus\content\G\Instance0\Game.exe".to_string(), None),
            (format!(r"{user}\G\Instance0\Game.exe"), None),
        ]
    );
    assert_eq!(
        layer_changes(&empty, &after, r"D:\G\Game.exe", content, "game.exe"),
        [(r"C:\Nucleus\content\G\Instance0\Game.exe".to_string(), None)]
    );
}

/// Files a hardware case may change, as they were before it: a game
/// list, and each handler of Rubylight's (its marker) by name, in each
/// handlers folder. Nothing is put back through a link: the user may
/// change their own copy (whose files the case takes and puts back as
/// them).
#[derive(Debug, Default)]
struct KeptFiles {
    /// Each path's bytes; None when it was not there.
    files: BTreeMap<PathBuf, Option<Vec<u8>>>,
    handlers: Vec<PathBuf>,
}
impl KeptFiles {
    fn own_handlers(handlers: &Path) -> Result<Vec<PathBuf>> {
        let mut own = Vec::new();
        let entries = match std::fs::read_dir(handlers) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(own),
            Err(error) => return Err(error).context(handlers.display().to_string()),
        };
        for entry in entries {
            let path = entry?.path();
            if path.is_file()
                && std::fs::read(&path).is_ok_and(|text| {
                    butterpollo_core::nucleus::generic::owns(&String::from_utf8_lossy(&text))
                })
            {
                own.push(path);
            }
        }
        Ok(own)
    }

    fn take(handlers: &[&Path], list: &Path) -> Result<Self> {
        let mut files = BTreeMap::new();
        let mut own = Vec::new();
        for folder in handlers {
            own.extend(Self::own_handlers(folder)?);
        }
        for path in own.into_iter().chain([list.to_path_buf()]) {
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error).context(path.display().to_string()),
            };
            files.insert(path, bytes);
        }
        Ok(Self {
            files,
            handlers: handlers.iter().map(|folder| folder.to_path_buf()).collect(),
        })
    }

    /// An error when `path` or its folder is a link.
    fn unlinked(path: &Path) -> Result<()> {
        use std::os::windows::fs::MetadataExt;
        for at in [Some(path), path.parent()].into_iter().flatten() {
            match std::fs::symlink_metadata(at) {
                Ok(metadata) => ensure!(
                    metadata.file_attributes() & 0x400 == 0,
                    "{} is a link, so the case puts nothing back through it",
                    at.display()
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).context(at.display().to_string()),
            }
        }
        Ok(())
    }

    /// Each file as it was, and a handler of ours the case wrote gone.
    /// Returns what could not be put back.
    fn put_back(&self) -> Vec<String> {
        let mut failures = Vec::new();
        for folder in &self.handlers {
            if let Err(error) = Self::unlinked(folder) {
                failures.push(format!("{error:#}"));
                continue;
            }
            match Self::own_handlers(folder) {
                Ok(now) => {
                    for path in now.iter().filter(|path| !self.files.contains_key(*path)) {
                        match std::fs::remove_file(path) {
                            Ok(()) => println!("cleanup: removed {}", path.display()),
                            Err(error) => failures.push(format!("{}: {error}", path.display())),
                        }
                    }
                }
                Err(error) => failures.push(format!("{error:#}")),
            }
        }
        for (path, bytes) in &self.files {
            if let Err(error) = Self::unlinked(path) {
                failures.push(format!("{error:#}"));
                continue;
            }
            let now = std::fs::read(path).ok();
            if now == *bytes {
                continue;
            }
            let done = match bytes {
                Some(bytes) => std::fs::write(path, bytes),
                None => std::fs::remove_file(path),
            };
            match done {
                Ok(()) => println!("cleanup: {} as before", path.display()),
                Err(error) => failures.push(format!("{}: {error}", path.display())),
            }
        }
        failures
    }
}

#[test]
fn a_hardware_case_puts_the_game_list_and_our_handlers_back_exactly() {
    let root = std::env::temp_dir().join(format!("nucleus-case-{}", uuid::Uuid::new_v4()));
    let handlers = root.join("handlers");
    std::fs::create_dir_all(&handlers).unwrap();
    let ours = |name: &str| {
        format!(
            "{}\nGame.GUID='Rubylight-{name}'\n",
            butterpollo_core::nucleus::generic::MARKER
        )
    };
    let (earlier, hub, list) = (
        handlers.join("Rubylight-earlier.js"),
        handlers.join("Hub.js"),
        root.join("userprofile.json"),
    );
    std::fs::write(&earlier, ours("earlier")).unwrap();
    std::fs::write(&hub, "Game.GUID='Hub'\n").unwrap();
    std::fs::write(&list, r#"{"Games":[{"GameGuid":"Rubylight-earlier"}]}"#).unwrap();
    let kept = KeptFiles::take(&[&handlers], &list).unwrap();
    // The case's sweep takes the earlier one and its entry, and writes
    // its own; the user's hub handler changes meanwhile.
    std::fs::remove_file(&earlier).unwrap();
    std::fs::write(handlers.join("Rubylight-case.js"), ours("case")).unwrap();
    std::fs::write(&list, r#"{"Games":[{"GameGuid":"Rubylight-case"}]}"#).unwrap();
    std::fs::write(&hub, "Game.GUID='Hub'\n// edited\n").unwrap();
    assert!(kept.put_back().is_empty());
    assert_eq!(std::fs::read_to_string(&earlier).unwrap(), ours("earlier"));
    assert!(!handlers.join("Rubylight-case.js").exists());
    assert_eq!(
        std::fs::read_to_string(&list).unwrap(),
        r#"{"Games":[{"GameGuid":"Rubylight-earlier"}]}"#
    );
    assert_eq!(
        std::fs::read_to_string(&hub).unwrap(),
        "Game.GUID='Hub'\n// edited\n"
    );
    // A list that was not there goes again.
    std::fs::remove_file(&list).unwrap();
    let kept = KeptFiles::take(&[&handlers], &list).unwrap();
    std::fs::write(&list, "{}").unwrap();
    assert!(kept.put_back().is_empty());
    assert!(!list.exists());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_hardware_case_puts_the_users_copy_handlers_back_too() {
    let root = std::env::temp_dir().join(format!("nucleus-case-{}", uuid::Uuid::new_v4()));
    let (host, copy) = (root.join("host"), root.join("copy").join("handlers"));
    std::fs::create_dir_all(&host).unwrap();
    let list = root.join("userprofile.json");
    let ours = format!(
        "{}\nGame.GUID='Rubylight-case'\n",
        butterpollo_core::nucleus::generic::MARKER
    );
    std::fs::write(host.join("Rubylight-earlier.js"), &ours).unwrap();
    // The user's copy is not there yet: the session makes it.
    let kept = KeptFiles::take(&[&host, &copy], &list).unwrap();
    std::fs::create_dir_all(&copy).unwrap();
    std::fs::write(copy.join("Rubylight-earlier.js"), &ours).unwrap();
    std::fs::write(copy.join("Rubylight-case.js"), &ours).unwrap();
    std::fs::write(copy.join("Hub.js"), "Game.GUID='Hub'\n").unwrap();
    std::fs::remove_file(host.join("Rubylight-earlier.js")).unwrap();
    assert!(kept.put_back().is_empty());
    assert!(host.join("Rubylight-earlier.js").is_file());
    assert_eq!(
        KeptFiles::own_handlers(&copy).unwrap(),
        Vec::<PathBuf>::new()
    );
    // Only ours go: a hub handler mirrored into the copy stays.
    assert!(copy.join("Hub.js").is_file());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_hardware_case_puts_nothing_back_through_a_link() {
    let root = std::env::temp_dir().join(format!("nucleus-case-{}", uuid::Uuid::new_v4()));
    let (copy, elsewhere) = (root.join("copy"), root.join("elsewhere"));
    let handlers = copy.join("handlers");
    std::fs::create_dir_all(&handlers).unwrap();
    std::fs::create_dir_all(&elsewhere).unwrap();
    let ours = format!(
        "{}\nGame.GUID='Rubylight-case'\n",
        butterpollo_core::nucleus::generic::MARKER
    );
    let list = copy.join("content").join("userprofile.json");
    let kept = KeptFiles::take(&[&handlers], &list).unwrap();
    // The user makes their handlers folder and their content folder links
    // to files of the host's.
    std::fs::remove_dir(&handlers).unwrap();
    std::fs::write(elsewhere.join("Rubylight-host.js"), &ours).unwrap();
    std::fs::write(elsewhere.join("userprofile.json"), "host's").unwrap();
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&handlers)
        .arg(&elsewhere)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "mklink /J failed");
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(copy.join("content"))
        .arg(&elsewhere)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "mklink /J failed");
    let failures = kept.put_back();
    assert_eq!(failures.len(), 2, "{failures:?}");
    assert!(
        failures.iter().all(|f| f.contains("is a link")),
        "{failures:?}"
    );
    assert!(elsewhere.join("Rubylight-host.js").is_file());
    assert_eq!(
        std::fs::read_to_string(elsewhere.join("userprofile.json")).unwrap(),
        "host's"
    );
    std::fs::remove_dir(&handlers).unwrap();
    std::fs::remove_dir(copy.join("content")).unwrap();
    std::fs::remove_dir_all(&root).unwrap();
}

/// What a hardware case leaves: the session, the game list and
/// Rubylight's handlers ([`KeptFiles`]), and the game's compatibility
/// settings at its exact paths, put back as they were once, also when
/// the case fails.
struct CaseCleanup {
    registry: Arc<Registry>,
    /// The host's files; the user's copy's, put back as `user`.
    kept: KeptFiles,
    kept_copy: KeptFiles,
    user: Option<std::os::windows::io::OwnedHandle>,
    layers_before: BTreeMap<String, butterpollo_windows::nucleus::layers::Value>,
    /// The `content` folder of each Nucleus copy the session may run
    /// from: the host's and the user's own.
    contents: Vec<PathBuf>,
    /// The game's path and its handler's .exe, once admitted.
    game: Option<(String, String)>,
    done: bool,
}
impl CaseCleanup {
    fn run(&mut self) -> Result<()> {
        if std::mem::replace(&mut self.done, true) {
            return Ok(());
        }
        for listed in self.registry.sessions() {
            self.registry.end(listed.session.id);
        }
        let mut failures = self.kept.put_back();
        match user_copy::as_user(self.user.as_ref(), || self.kept_copy.put_back()) {
            Ok(more) => failures.extend(more),
            Err(error) => failures.push(format!("the user's copy: {error:#}")),
        }
        if let Some((game, executable)) = &self.game {
            let changed = (|| -> Result<usize> {
                use butterpollo_windows::nucleus::layers::Layers;
                let mut hive = butterpollo_windows::nucleus::layers::UserHive::signed_in()?;
                let changes = layer_changes(
                    &self.layers_before,
                    &hive.values()?,
                    game,
                    &self.contents,
                    executable,
                );
                for (name, value) in &changes {
                    match value {
                        Some(value) => hive.set(name, value)?,
                        None => hive.delete(name)?,
                    }
                }
                Ok(changes.len())
            })();
            match changed {
                Ok(0) => {}
                Ok(changed) => println!("cleanup: {changed} compatibility settings put back"),
                Err(error) => failures.push(format!("compatibility settings: {error:#}")),
            }
        }
        ensure!(
            failures.is_empty(),
            "cleanup failed: {}",
            failures.join("; ")
        );
        Ok(())
    }
}
impl Drop for CaseCleanup {
    fn drop(&mut self) {
        if let Err(error) = self.run() {
            eprintln!("{error:#}");
        }
    }
}

/// How a hardware case expects the session to run, from
/// `RUBYLIGHT_NUCLEUS_RUN`: "user" or "elevated". Unset or empty, it
/// takes whichever the admission chooses.
fn expected_run(value: Option<&str>) -> Result<Option<Run>> {
    match value.map(str::trim) {
        None | Some("") => Ok(None),
        Some("user") => Ok(Some(Run::User)),
        Some("elevated") => Ok(Some(Run::Elevated)),
        Some(other) => {
            anyhow::bail!("RUBYLIGHT_NUCLEUS_RUN is {other:?}, not user or elevated")
        }
    }
}

/// How a session ran, from its NucleusCoop.exe's program path `image`:
/// as the user from their own `copy`, elevated from the host's `folder`.
fn ran_from(image: &str, folder: &Path, copy: &Path) -> Result<Run> {
    let plain = |path: &Path| {
        let path = path.to_string_lossy().replace('/', "\\").to_lowercase();
        let path = path.strip_prefix(r"\\?\").unwrap_or(&path);
        path.trim_end_matches('\\').to_string()
    };
    let image = Path::new(image);
    ensure!(
        image
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("NucleusCoop.exe")),
        "{} is not NucleusCoop.exe",
        image.display()
    );
    match image.parent().map(plain) {
        Some(parent) if parent == plain(copy) => Ok(Run::User),
        Some(parent) if parent == plain(folder) => Ok(Run::Elevated),
        _ => anyhow::bail!(
            "NucleusCoop.exe ran from {}, neither the user's own copy {} nor the host's {}",
            image.display(),
            copy.display(),
            folder.display()
        ),
    }
}

/// SECURITY_MANDATORY_MEDIUM_RID: the integrity level of a program
/// running with a user's standard rights.
const MEDIUM_INTEGRITY: u32 = 0x2000;

/// That `name` (process `pid`), whose token is `elevated` at `integrity`,
/// runs with the user's standard rights: not elevated, medium integrity.
fn standard_rights(name: &str, pid: u32, elevated: bool, integrity: u32) -> Result<()> {
    ensure!(
        !elevated && integrity == MEDIUM_INTEGRITY,
        "{name} {pid} runs {} at integrity 0x{integrity:x}, not unelevated at medium (0x2000)",
        if elevated { "elevated" } else { "unelevated" }
    );
    Ok(())
}

/// Whether process `pid`'s token is elevated, and its integrity level
/// (the last RID of its mandatory label).
fn token_rights(pid: u32) -> Result<(bool, u32)> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_ELEVATION,
        TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenElevation, TokenIntegrityLevel,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    struct Owned(HANDLE);
    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: the handle was opened for this value and is closed once.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
    // SAFETY: each handle is closed once by `Owned`. The buffers outlive
    // the calls that fill them, which get their sizes; the label's SID
    // points into `label`, read while it lives.
    unsafe {
        let process = Owned(
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
                .with_context(|| format!("opening process {pid}"))?,
        );
        let mut token = HANDLE::default();
        OpenProcessToken(process.0, TOKEN_QUERY, &mut token)
            .with_context(|| format!("opening process {pid}'s token"))?;
        let token = Owned(token);
        let mut elevation = TOKEN_ELEVATION::default();
        let mut bytes = 0;
        GetTokenInformation(
            token.0,
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut bytes,
        )?;
        // TOKEN_MANDATORY_LABEL and the SID it points to, 8-byte aligned.
        let mut label = [0u64; 16];
        GetTokenInformation(
            token.0,
            TokenIntegrityLevel,
            Some(label.as_mut_ptr().cast()),
            size_of_val(&label) as u32,
            &mut bytes,
        )?;
        let sid = (*label.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()).Label.Sid;
        let count = *GetSidSubAuthorityCount(sid);
        ensure!(count > 0, "process {pid}'s integrity label has no level");
        let integrity = *GetSidSubAuthority(sid, u32::from(count) - 1);
        Ok((elevation.TokenIsElevated != 0, integrity))
    }
}

#[test]
fn a_hardware_case_reads_the_run_it_expects() {
    assert_eq!(expected_run(None).unwrap(), None);
    assert_eq!(expected_run(Some(" ")).unwrap(), None);
    assert_eq!(expected_run(Some("user")).unwrap(), Some(Run::User));
    assert_eq!(expected_run(Some("elevated")).unwrap(), Some(Run::Elevated));
    assert!(expected_run(Some("admin")).is_err());
}

#[test]
fn a_hardware_case_tells_the_users_copy_from_the_hosts() {
    let folder = Path::new(r"C:\ProgramData\Butterpollo\nucleus\2.4.2");
    let copy = Path::new(r"C:\ProgramData\Butterpollo\nucleus-user\S-1-5-21-7\2.4.2\");
    let run = |image: &str| ran_from(image, folder, copy);
    assert_eq!(
        run(r"C:\ProgramData\Butterpollo\nucleus-user\S-1-5-21-7\2.4.2\NucleusCoop.exe").unwrap(),
        Run::User
    );
    assert_eq!(
        run(r"\\?\c:\programdata\butterpollo\NUCLEUS-USER\s-1-5-21-7\2.4.2\nucleuscoop.EXE")
            .unwrap(),
        Run::User
    );
    assert_eq!(
        run(r"C:\ProgramData\Butterpollo\nucleus\2.4.2\NucleusCoop.exe").unwrap(),
        Run::Elevated
    );
    // Another user's copy, a folder below the copy, another program.
    for image in [
        r"C:\ProgramData\Butterpollo\nucleus-user\S-1-5-21-8\2.4.2\NucleusCoop.exe",
        r"C:\ProgramData\Butterpollo\nucleus-user\S-1-5-21-7\2.4.2\content\NucleusCoop.exe",
        r"C:\ProgramData\Butterpollo\nucleus-user\S-1-5-21-7\2.4.2\Game.exe",
    ] {
        assert!(run(image).is_err(), "{image}");
    }
}

#[test]
fn a_hardware_case_asks_for_standard_rights_at_medium_integrity() {
    standard_rights("NucleusCoop.exe", 1, false, 0x2000).unwrap();
    let elevated = standard_rights("NucleusCoop.exe", 1, true, 0x3000).unwrap_err();
    assert_eq!(
        elevated.to_string(),
        "NucleusCoop.exe 1 runs elevated at integrity 0x3000, not unelevated at medium (0x2000)"
    );
    // UAC off: high integrity without an elevated token; a low one.
    assert!(standard_rights("Game.exe", 2, false, 0x3000).is_err());
    assert!(standard_rights("Game.exe", 2, false, 0x1000).is_err());
    // This test's own process, read as the case reads the session's.
    let (elevated, integrity) = token_rights(std::process::id()).unwrap();
    assert_eq!(elevated, butterpollo_windows::process::elevated().unwrap());
    if elevated {
        assert!(integrity >= 0x3000, "0x{integrity:x}");
    } else {
        assert!(integrity >= MEDIUM_INTEGRITY, "0x{integrity:x}");
    }
    assert!(token_rights(0).is_err());
}

/// The values of a user's Nucleus `Settings.ini` that stalled a
/// background stream at Nucleus' start (#59): its folder check on,
/// favourites only, and only games for a single keyboard listed.
const COLD: [(&str, &str, &str); 3] = [
    ("Dev", "DisablePathCheck", "False"),
    ("Dev", "ShowFavoriteOnly", "True"),
    ("Misc", "GameSortingOpt", "Sort_2,Show_Type2"),
];
/// What they are while a session from them runs.
const FORCED: [(&str, &str, &str); 6] = [
    ("Dev", "UseXinputIndex", "True"),
    ("Misc", "DisableGameProfiles", "False"),
    ("Dev", "DisablePathCheck", "True"),
    ("Dev", "ShowFavoriteOnly", "False"),
    ("Misc", "IgnoreInputLockReminder", "True"),
    ("Misc", "GameSortingOpt", "Sort_2"),
];

/// Nucleus' `Settings.ini` with [`COLD`]'s values.
fn cold_settings(text: &[u8]) -> Vec<u8> {
    butterpollo_core::nucleus::profile::ini_set(text, &COLD)
}

/// Fails unless `Settings.ini` has each of `values`, naming the first
/// that differs.
fn settings_are(text: &[u8], values: &[(&str, &str, &str)]) -> Result<()> {
    for &(section, key, value) in values {
        let found = butterpollo_core::nucleus::profile::ini_value(text, section, key)
            .map(|v| String::from_utf8_lossy(&v).into_owned());
        ensure!(
            found.as_deref() == Some(value),
            "Settings.ini has {key}={}, not {value}",
            found.as_deref().unwrap_or("nothing")
        );
    }
    Ok(())
}

/// How long after "Nucleus Co-op started" the session logged
/// "profile Rubylight clicked", from the log lines and when each came.
fn click_delay(lines: &[(Instant, String)]) -> Result<Duration> {
    let started = lines
        .iter()
        .find(|(_, line)| line.contains("Nucleus Co-op started"))
        .map(|(at, _)| *at)
        .context("no \"Nucleus Co-op started\" in the log")?;
    let clicked = lines
        .iter()
        .find(|(at, line)| *at >= started && line.contains("profile Rubylight clicked"))
        .map(|(at, _)| *at)
        .context("no \"profile Rubylight clicked\" after \"Nucleus Co-op started\"")?;
    Ok(clicked - started)
}

#[test]
fn the_cold_case_seeds_the_bad_settings_and_checks_the_forced_ones() {
    let shipped = butterpollo_core::nucleus::profile::DEFAULT_SETTINGS.as_bytes();
    let cold = cold_settings(shipped);
    settings_are(&cold, &COLD).unwrap();
    assert!(settings_are(&cold, &FORCED).is_err());
    // What a session writes passes, and what it puts back is the cold file.
    let settings = butterpollo_core::nucleus::profile::settings_ini(&cold);
    settings_are(&settings.text, &FORCED).unwrap();
    let (back, _) =
        butterpollo_core::nucleus::profile::put_back_settings(&settings.text, &settings.forced);
    settings_are(&back, &COLD).unwrap();
    // Each value is checked and named.
    let filtered = butterpollo_core::nucleus::profile::ini_set(
        &settings.text,
        &[("Misc", "GameSortingOpt", "Show_Type1")],
    );
    let error = settings_are(&filtered, &FORCED).unwrap_err().to_string();
    assert_eq!(
        error,
        "Settings.ini has GameSortingOpt=Show_Type1, not Sort_2"
    );
    let error = settings_are(b"", &FORCED).unwrap_err().to_string();
    assert_eq!(error, "Settings.ini has UseXinputIndex=nothing, not True");
}

#[test]
fn the_click_is_timed_from_nucleus_start() {
    let t0 = Instant::now();
    let line = |s: u64, text: &str| (t0 + Duration::from_secs(s), text.to_string());
    let lines = [
        line(0, "INFO profile written"),
        line(1, "INFO Nucleus Co-op started pid=4"),
        line(
            5,
            "WARN Nucleus Co-op shows \"Warning: x\" on the PC's screen",
        ),
        line(9, "INFO Nucleus Co-op profile Rubylight clicked"),
    ];
    assert_eq!(click_delay(&lines).unwrap(), Duration::from_secs(8));
    assert!(click_delay(&lines[..3]).is_err());
    assert!(click_delay(&lines[2..]).is_err());
}

/// The host's log during a hardware case: printed, and each line kept
/// with when it came.
#[derive(Clone, Default)]
struct CaseLog(Arc<std::sync::Mutex<Vec<(Instant, String)>>>);
impl std::io::Write for CaseLog {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        print!("{text}");
        let mut lines = self.0.lock().unwrap();
        lines.extend(text.lines().map(|line| (Instant::now(), line.to_string())));
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl CaseLog {
    fn install() -> Result<CaseLog> {
        let log = CaseLog::default();
        let writer = log.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::set_global_default(subscriber)
            .context("a log was set up before the case")?;
        Ok(log)
    }
}

/// A file's bytes, None when it is not there.
fn read_if_there(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| path.display().to_string()),
    }
}

/// The windows of the message boxes Windows shows now, for any program:
/// none of them is the case's.
fn windows_boxes(
    desktop: &mut butterpollo_windows::nucleus::desktop::Win32,
) -> Result<BTreeSet<u64>> {
    use butterpollo_windows::nucleus::instances::Desktop;
    Ok(desktop
        .message_boxes(&BTreeSet::new())?
        .into_iter()
        .map(|shown| shown.window)
        .collect())
}

/// Fails while Nucleus Co-op or the game started since `since` shows a
/// message box, naming it; Windows' boxes up `before` the case aside.
fn no_message_box(
    desktop: &mut butterpollo_windows::nucleus::desktop::Win32,
    before: &BTreeSet<u64>,
    since: u64,
    executable: &str,
) -> Result<()> {
    use butterpollo_windows::nucleus::instances::Desktop;
    let processes: BTreeSet<u32> = butterpollo_windows::process::processes()?
        .into_iter()
        .filter(|p| {
            p.started >= since
                && (p.name.eq_ignore_ascii_case("NucleusCoop.exe")
                    || p.name.eq_ignore_ascii_case(executable))
        })
        .map(|p| p.pid)
        .collect();
    let mut shown = desktop.message_boxes(&processes)?;
    shown.retain(|b| !before.contains(&b.window));
    match shown.first() {
        Some(shown) => anyhow::bail!(
            "Nucleus Co-op shows \"{}\" on the PC's screen",
            shown.summary()
        ),
        None => Ok(()),
    }
}

#[test]
fn a_file_that_is_not_there_reads_as_none() {
    let dir = std::env::temp_dir().join(format!("nucleus-read-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("crashlog.txt");
    assert_eq!(read_if_there(&path).unwrap(), None);
    std::fs::write(&path, "x").unwrap();
    assert_eq!(read_if_there(&path).unwrap(), Some(b"x".to_vec()));
    // A folder in its place is an error, not a missing file.
    assert!(read_if_there(&dir).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Nucleus' `Settings.ini` as it was before a case, put back when the
/// case ends, also on a failure. The session's journal in the host's
/// folder goes with it: it would put back the seeded values over the
/// user's later. A user's copy's file is put back as `user`.
struct KeptSettings {
    path: PathBuf,
    before: Vec<u8>,
    journal: Option<PathBuf>,
    user: Option<std::os::windows::io::OwnedHandle>,
}
impl Drop for KeptSettings {
    fn drop(&mut self) {
        let put_back = || {
            match std::fs::write(&self.path, &self.before) {
                Ok(()) => println!("Settings.ini as before: {}", self.path.display()),
                Err(error) => eprintln!("Settings.ini not put back: {error}"),
            }
            let Some(journal) = &self.journal else {
                return;
            };
            match std::fs::remove_file(journal) {
                Ok(()) => println!("cleanup: {} removed", journal.display()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => eprintln!("{} not removed: {error}", journal.display()),
            }
        };
        if let Err(error) = user_copy::as_user(self.user.as_ref(), put_back) {
            eprintln!("{} not put back: {error:#}", self.path.display());
        }
    }
}

#[test]
fn kept_settings_go_back_and_take_the_sessions_journal_along() {
    let dir = std::env::temp_dir().join(format!("nucleus-kept-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Settings.ini");
    let journal = dir.join("Rubylight-settings.json");
    std::fs::write(&path, "seeded").unwrap();
    std::fs::write(&journal, "[]").unwrap();
    drop(KeptSettings {
        path: path.clone(),
        before: b"users".to_vec(),
        journal: Some(journal.clone()),
        user: None,
    });
    assert_eq!(std::fs::read(&path).unwrap(), b"users");
    assert!(!journal.exists());
    // None left is fine, and a copy has none.
    for journal in [Some(journal.clone()), None] {
        drop(KeptSettings {
            path: path.clone(),
            before: b"users".to_vec(),
            journal,
            user: None,
        });
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A background stream of a one-player game without a hub handler on
/// this PC, through the host's own admission (tools/hw/catalog.yaml,
/// nucleus-generic-background): the app's .exe `RUBYLIGHT_NUCLEUS_GAME`
/// or the Steam link of `RUBYLIGHT_NUCLEUS_STEAM_APP` goes through the
/// path, launcher, anti-cheat, argument and Unreal checks, Rubylight's
/// handler and list entry are written, and Nucleus Co-op starts the game.
/// Asserted: the handler file and the entry; a process of the handler's
/// .exe started by the session; the session running with its window;
/// that window captured through WGC with a frame not black; the first
/// player's A press and release at the session's XInput index (where
/// Nucleus' input hooks read the pad; whether the game reacts is not
/// observable here); and the game's compatibility settings as before once
/// it stops. Nucleus Co-op's game list, Rubylight's handlers (an earlier one
/// the session's sweep took included) and the settings at the game's
/// paths are then exactly as before, also on a failure. Run as SYSTEM,
/// like the service, with Nucleus Co-op
/// installed by the console and not running. No message box of Nucleus
/// or the game shows at any point, and Nucleus' crashlog.txt is
/// unchanged in the host's folder and in the user's copy (read as them).
///
/// Where its NucleusCoop.exe runs from says how the admission ran it: as
/// the user from their own copy (`nucleus-user\<SID>\<files>`, the
/// handler written there and the game in its list), or elevated from the
/// host's. `RUBYLIGHT_NUCLEUS_RUN` (user or elevated) asserts which
/// (nucleus-generic-background-user). Run as the user, NucleusCoop.exe
/// and the game must hold tokens that are not elevated, at medium
/// integrity, and Rubylight's handlers and the game list in the copy are
/// as before too; the copy's files are read and put back as the user.
///
/// With `RUBYLIGHT_NUCLEUS_COLD=1` (nucleus-generic-background-cold), the
/// Settings.ini of the host's Nucleus and of the user's copy first get
/// the values that stalled #59 ([`cold_settings`]) and are put back
/// afterwards, also on a failure; the profile must be clicked within
/// 15 s of Nucleus' start, Nucleus must set the game's DPI mode, and the
/// Settings.ini Nucleus ran from must hold the forced values while the
/// session runs, the other one untouched. Afterwards the host's has the
/// user's values back with no journal left; the copy's keeps the
/// session's, with no journal in it.
#[test]
#[ignore = "starts a game through Nucleus Co-op on this PC as SYSTEM"]
fn a_background_stream_without_a_hub_handler_on_this_pc() -> Result<()> {
    use butterpollo_windows::nucleus::layers::{Layers, UserHive};
    let log = CaseLog::install()?;
    let cold = std::env::var("RUBYLIGHT_NUCLEUS_COLD").is_ok_and(|v| v == "1");
    let game = std::env::var("RUBYLIGHT_NUCLEUS_GAME").ok();
    let steam_app = std::env::var("RUBYLIGHT_NUCLEUS_STEAM_APP")
        .ok()
        .map(|id| id.parse::<u32>().context("RUBYLIGHT_NUCLEUS_STEAM_APP"))
        .transpose()?;
    let hold = Duration::from_secs(
        std::env::var("RUBYLIGHT_NUCLEUS_HOLD")
            .ok()
            .map(|s| s.parse().context("RUBYLIGHT_NUCLEUS_HOLD"))
            .transpose()?
            .unwrap_or(20),
    );
    let expect = expected_run(std::env::var("RUBYLIGHT_NUCLEUS_RUN").ok().as_deref())?;
    let app = case_app(game.as_deref(), steam_app)?;
    let pc = OnPc;
    if let Some(why) = pc.unavailable() {
        anyhow::bail!(why);
    }
    let folder = pc.installed("")?.context(
        "Nucleus Co-op is not installed: set it up in the console's Play together panel",
    )?;
    if let Some(pid) = pc.running()? {
        anyhow::bail!("Nucleus Co-op runs (process {pid}): close it first");
    }
    // The signed-in user's own copy, made here as the admission would
    // when the session runs as them: its folder is named by the host's
    // program files, so it is only known once made. What is in it is
    // the user's to change: read and put back as them.
    let user = user_copy::user()?;
    let copy = pc.user_copy(&folder)?;
    // Nucleus' crashlog.txt where either may run from.
    let crashlogs = [folder.join("crashlog.txt"), copy.join("crashlog.txt")];
    let crashes = || -> Result<Vec<Option<Vec<u8>>>> {
        Ok(vec![
            read_if_there(&crashlogs[0])?,
            user_copy::as_user(Some(&user), || read_if_there(&crashlogs[1]))??,
        ])
    };
    let crashes_before = crashes()?;
    let settings = folder.join("Settings.ini");
    let copy_settings = copy.join("Settings.ini");
    // Declared before the cleanup, so they are put back after the session ended.
    let _kept_settings = if cold {
        let mut kept = Vec::new();
        for (path, journal, token) in [
            (
                &settings,
                Some(folder.join("Rubylight-settings.json")),
                None,
            ),
            (&copy_settings, None, Some(user.try_clone()?)),
        ] {
            let before = user_copy::as_user(token.as_ref(), || std::fs::read(path))?
                .with_context(|| path.display().to_string())?;
            let seeded = cold_settings(&before);
            kept.push(KeptSettings {
                path: path.clone(),
                before,
                journal,
                user: token,
            });
            let user = kept.last().and_then(|kept| kept.user.as_ref());
            user_copy::as_user(user, || std::fs::write(path, seeded))?
                .with_context(|| path.display().to_string())?;
            println!(
                "Settings.ini seeded: DisablePathCheck=False, ShowFavoriteOnly=True, GameSortingOpt=Sort_2,Show_Type2 in {}",
                path.display()
            );
        }
        kept
    } else {
        Vec::new()
    };
    let registry = Arc::new(Registry::new(Arc::new(OnPc)));
    // The game lists Nucleus Co-op reads (#59): the host's, the copy's.
    let (list, copy_list) = (files::list(&folder), files::list(&copy));
    let own_in = |handlers: &Path| -> Result<Vec<PathBuf>> {
        let mut own = user_copy::as_user(Some(&user), || KeptFiles::own_handlers(handlers))??;
        own.sort();
        Ok(own)
    };
    let list_of = |list: &Path| user_copy::as_user(Some(&user), || read_if_there(list))?;
    let before = pc.catalog(&folder)?;
    let copy_before = (own_in(&copy.join("handlers"))?, list_of(&copy_list)?);
    let mut cleanup = CaseCleanup {
        registry: registry.clone(),
        kept: KeptFiles::take(&[&folder.join("handlers")], &list)?,
        kept_copy: user_copy::as_user(Some(&user), || {
            KeptFiles::take(&[&copy.join("handlers")], &copy_list)
        })??,
        user: Some(user.try_clone()?),
        layers_before: UserHive::signed_in()?.values()?,
        contents: vec![folder.join("content"), copy.join("content")],
        game: None,
        done: false,
    };
    let owner = client("hw-case", false);
    let config = Config::default();
    let warnings = Arc::new(Warnings::default());
    let mut desktop = butterpollo_windows::nucleus::desktop::Win32::default();
    let boxes_before = windows_boxes(&mut desktop)?;
    let since = butterpollo_windows::process::now();
    let seat = registry
        .admit(Admission {
            launch: "hw-case",
            client: &owner,
            game: Game::Background(&app),
            size: (1280, 720),
            config: &config,
            warnings: warnings.clone(),
        })
        .map_err(|refused| anyhow!("refused ({}): {}", refused.status, refused.message))?;
    let instance = registry
        .instance(seat.session)
        .context("the admitted session is not listed")?;
    let guid = instance.session(|s| s.game.clone());
    // Where its NucleusCoop.exe runs from says how it was run.
    let looking = Instant::now();
    let nucleus: Vec<(u32, String)> = loop {
        let found = butterpollo_windows::process::processes()?
            .into_iter()
            .filter(|p| p.started >= since && p.name.eq_ignore_ascii_case("NucleusCoop.exe"))
            .map(|p| {
                let image = butterpollo_windows::process::image_path(p.pid)
                    .with_context(|| format!("NucleusCoop.exe {}'s program path", p.pid))?;
                Ok((p.pid, image))
            })
            .collect::<Result<Vec<_>>>()?;
        if !found.is_empty() {
            break found;
        }
        ensure!(
            looking.elapsed() < Duration::from_secs(30),
            "no NucleusCoop.exe started with the session"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    let runs = nucleus
        .iter()
        .map(|(_, image)| ran_from(image, &folder, &copy))
        .collect::<Result<Vec<Run>>>()?;
    let run = runs[0];
    ensure!(
        runs.iter().all(|other| *other == run),
        "NucleusCoop.exe runs from both copies: {nucleus:?}"
    );
    if let Some(expect) = expect {
        ensure!(
            run == expect,
            "the session runs {run:?}, not {expect:?}: {nucleus:?}"
        );
    }
    let from = match run {
        Run::User => &copy,
        Run::Elevated => &folder,
    };
    println!("runs {run:?} from {}", from.display());
    let catalog = pc.catalog(&folder)?;
    let handler = catalog
        .handlers
        .iter()
        .find(|h| h.guid == guid)
        .with_context(|| format!("no handler {guid} in Nucleus Co-op's handlers"))?
        .clone();
    ensure!(handler.own, "{} is not Rubylight's handler", handler.file);
    let entry = catalog
        .games
        .iter()
        .find(|g| g.guid == guid)
        .with_context(|| format!("{guid} is not in Nucleus Co-op's game list"))?;
    // As Nucleus Co-op 2.4.2 loads the list of the folder it runs from:
    // shown, so it can be selected. The copy's is read as the user.
    let reader = (run == Run::User).then_some(&user);
    let listed = user_copy::as_user(reader, || files::listed_by_nucleus(from))??;
    ensure!(
        listed.contains(&guid),
        "Nucleus Co-op does not show {guid} from {}",
        files::list(from).display()
    );
    println!("listed in {}", files::list(from).display());
    cleanup.game = Some((entry.exe_path.clone(), handler.executable.clone()));
    println!(
        "handler {} for {} at {}",
        handler.file, handler.executable, entry.exe_path
    );
    let started = Instant::now();
    loop {
        match instance.session(|s| s.phase.clone()) {
            Phase::Running => break,
            Phase::Ended(why) => anyhow::bail!("{why}"),
            Phase::Starting => {
                no_message_box(&mut desktop, &boxes_before, since, &handler.executable)?;
                ensure!(
                    started.elapsed() < Duration::from_secs(180),
                    "not running after {:?}",
                    started.elapsed()
                );
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    println!("running after {:?}", started.elapsed());
    let delay = click_delay(&log.0.lock().unwrap())?;
    println!("profile clicked {delay:?} after Nucleus Co-op started");
    ensure!(
        !cold || delay <= Duration::from_secs(15),
        "the profile was clicked {delay:?} after Nucleus Co-op started, not within 15 s"
    );
    let set = layer_changes(
        &cleanup.layers_before,
        &UserHive::signed_in()?.values()?,
        &entry.exe_path,
        &cleanup.contents,
        &handler.executable,
    );
    println!("compatibility settings Nucleus set: {}", set.len());
    // The Settings.ini Nucleus runs from, and the other one.
    let (ran_settings, other_settings) = match run {
        Run::User => (&copy_settings, &settings),
        Run::Elevated => (&settings, &copy_settings),
    };
    let settings_of = |path: &Path| -> Result<Vec<u8>> {
        let reader = (path == copy_settings).then_some(&user);
        user_copy::as_user(reader, || std::fs::read(path))?
            .with_context(|| path.display().to_string())
    };
    if cold {
        settings_are(&settings_of(ran_settings)?, &FORCED)?;
        settings_are(&settings_of(other_settings)?, &COLD)?;
        println!("Settings.ini has the forced values, the other one is untouched");
    }
    ensure!(
        !cold || !set.is_empty(),
        "Nucleus Co-op set no DPI mode for {}",
        handler.executable
    );
    let game_processes: Vec<u32> = butterpollo_windows::process::processes()?
        .into_iter()
        .filter(|p| p.started >= since && p.name.eq_ignore_ascii_case(&handler.executable))
        .map(|p| p.pid)
        .collect();
    ensure!(
        !game_processes.is_empty(),
        "no {} started with the session",
        handler.executable
    );
    println!("game process {game_processes:?}");
    if run == Run::User {
        let names = nucleus
            .iter()
            .map(|(pid, _)| ("NucleusCoop.exe", *pid))
            .chain(
                game_processes
                    .iter()
                    .map(|pid| (handler.executable.as_str(), *pid)),
            );
        for (name, pid) in names {
            let (elevated, integrity) = token_rights(pid)?;
            standard_rights(name, pid, elevated, integrity)?;
        }
        println!("rights: NucleusCoop.exe and the game unelevated at medium integrity");
    }
    // The window streams through WGC, on the primary display's GPU.
    let display = butterpollo_windows::capture::primary_display().context("no display")?;
    let _com = butterpollo_windows::capture::ComGuard::new()?;
    let target = instance.capture_target(0, &display)?;
    let mut capture = butterpollo_windows::capture::Capture::open_for_stream_reported(
        &target,
        "wgc",
        false,
        &Default::default(),
        warnings.clone(),
    )?;
    let (mut staging, mut best) = (None, 0.0f64);
    let deadline = Instant::now() + Duration::from_secs(60);
    let image = loop {
        if let Some(frame) = capture.next_gpu()? {
            let image = frame.readback(&mut staging)?;
            let share = lit(&image)?;
            best = best.max(share);
            if share >= 0.05 {
                break image;
            }
        }
        ensure!(
            Instant::now() < deadline,
            "the window gave no frame that is not black within 60 s (at most {:.1} % lit)",
            best * 100.0
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    println!(
        "capture {target}: {} {}x{}, {:.0} % lit",
        capture.backend(),
        image.width,
        image.height,
        lit(&image)? * 100.0
    );
    drop(capture);
    // A held, then released, through the player's slot to its XInput index.
    let index = instance.session(|s| s.slots[0].pad);
    let xinput = pads::System::open()?;
    for (buttons, expected) in [(0x1000u32, 0x1000u16), (0, 0)] {
        instance
            .input(
                &owner.uuid,
                Input::Controller {
                    id: 0,
                    active: 1,
                    buttons,
                    left_trigger: 0,
                    right_trigger: 0,
                    sticks: [0; 4],
                },
            )
            .map_err(|dropped| anyhow!("the pad input was dropped: {dropped:?}"))?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let seen = xinput.buttons(index)?;
            if seen.map(|b| b & 0x1000) == Some(expected) {
                break;
            }
            ensure!(
                Instant::now() < deadline,
                "XInput index {index} shows {seen:?}, not A {}",
                if expected == 0 { "released" } else { "held" }
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    println!("pad A held and released at XInput index {index}");
    let holding = Instant::now();
    while holding.elapsed() < hold {
        no_message_box(&mut desktop, &boxes_before, since, &handler.executable)?;
        std::thread::sleep(Duration::from_millis(250));
    }
    let phase = instance.session(|s| s.phase.clone());
    ensure!(phase == Phase::Running, "after {hold:?}: {phase:?}");
    let stopping = Instant::now();
    ensure!(registry.end(seat.session), "the session was gone");
    println!("stopped in {:?}", stopping.elapsed());
    let left: Vec<String> = butterpollo_windows::process::processes()?
        .into_iter()
        .filter(|p| p.started >= since)
        .filter(|p| {
            p.name.eq_ignore_ascii_case("NucleusCoop.exe")
                || p.name.eq_ignore_ascii_case(&handler.executable)
        })
        .map(|p| format!("{} {}", p.name, p.pid))
        .collect();
    ensure!(left.is_empty(), "still running after the stop: {left:?}");
    // The session put them back itself; the cleanup only covers a failure.
    let changes = layer_changes(
        &cleanup.layers_before,
        &UserHive::signed_in()?.values()?,
        &entry.exe_path,
        &cleanup.contents,
        &handler.executable,
    );
    ensure!(
        changes.is_empty(),
        "the compatibility settings changed: {changes:?}"
    );
    println!("compatibility settings as before");
    ensure!(
        crashes()? == crashes_before,
        "Nucleus Co-op wrote to {} or {}",
        crashlogs[0].display(),
        crashlogs[1].display()
    );
    println!("crashlog.txt unchanged");
    // No journal of Settings.ini in either: the copy's is never one.
    let journal_in = |folder: &Path| {
        let reader = (folder == copy).then_some(&user);
        user_copy::as_user(reader, || folder.join("Rubylight-settings.json").exists())
    };
    ensure!(
        !journal_in(&copy)? && (run == Run::User || !journal_in(&folder)?),
        "the session left Rubylight-settings.json behind"
    );
    if cold {
        match run {
            // The session put the user's values back once Nucleus closed.
            Run::Elevated => {
                settings_are(&settings_of(&settings)?, &COLD)?;
                println!("Settings.ini has the user's values back");
            }
            // Rubylight's own copy keeps them; the host's was never touched.
            Run::User => {
                settings_are(&settings_of(&copy_settings)?, &FORCED)?;
                settings_are(&settings_of(&settings)?, &COLD)?;
                println!("Settings.ini of the user's copy keeps the session's values");
            }
        }
    }
    for warning in warnings.snapshot() {
        println!("warning {}: {}", warning.code, warning.message);
    }
    cleanup.run()?;
    let after = pc.catalog(&folder)?;
    let own = |catalog: &Catalog| -> Vec<String> {
        let mut own: Vec<String> = catalog
            .handlers
            .iter()
            .filter(|h| h.own)
            .map(|h| h.file.clone())
            .collect();
        own.sort();
        own
    };
    ensure!(
        own(&after) == own(&before) && after.games == before.games,
        "the handlers or game list are not as before: {:?} and {:?}, were {:?} and {:?}",
        own(&after),
        after.games,
        own(&before),
        before.games
    );
    let copy_after = (own_in(&copy.join("handlers"))?, list_of(&copy_list)?);
    ensure!(
        copy_after == copy_before,
        "Rubylight's handlers or the game list in the user's copy are not as before: {:?}, were {:?}",
        copy_after.0,
        copy_before.0
    );
    println!("handlers and game list as before");
    Ok(())
}
