use super::*;
#[test]
fn game_processes_survive_store_handoffs_without_tracking_the_store() {
    let folder = r"C:\Games\Nightfire";
    assert!(game_process(
        r"C:\Games\Nightfire\bin\game.exe",
        folder,
        "steam://run/123"
    ));
    for store in [r"C:\Steam\steam.exe", r"C:\Epic\EpicGamesLauncher.exe"] {
        assert!(!game_process(store, r"C:\", store));
    }
    assert!(!game_process(r"C:\Games\Nightfire2\game.exe", folder, ""));
}
#[test]
fn game_process_paths_preserve_unicode_and_match_external_emulators() {
    assert!(game_process(
        r"C:\Çağrı\Oyun\game.exe",
        "C:/Çağrı/Oyun/",
        ""
    ));
    assert!(game_process(
        r"D:\Emulators\emu.exe",
        r"C:\ROMs",
        "D:/Emulators/emu.exe"
    ));
    assert!(!game_process(
        r"D:\Other\emu.exe",
        r"C:\ROMs",
        "D:/Emulators/emu.exe"
    ));
}
#[test]
fn messages_and_times_parse_like_vibepollo() {
    let Message::Games(invalid) = parse(
        r#"{"type":"games","payload":[{"id":"x\\..\\..\\Users\\Public\\loot"},{"id":"not-a-uuid"}]}"#,
    ) else {
        panic!()
    };
    assert!(invalid.is_empty());
    assert_eq!(parse_time("1970-01-02T00:00:00Z"), Some(86400));
    assert_eq!(parse_time("2026-08-29T17:49:56Z"), Some(1788025796));
    assert_eq!(
        parse_time("2026-08-29T19:49:56.1234567+02:00"),
        Some(1788025796)
    );
    assert_eq!(parse_time("2026-08-29"), None);
    assert_eq!(parse_time("2026-08-29T19:49:56+0200"), Some(1788025796));
    assert_eq!(parse_time("2026-08-29T19:49:56+02"), Some(1788025796));
    assert_eq!(parse_time("2026-08-29T15:49:56.5-02:00"), Some(1788025796));
    assert_eq!(parse_time("2026-08-29T17:49:56+2"), None);
    // Four bytes that are not four characters.
    assert_eq!(parse_time("2026-08-29T17:49:56+1\u{e9}2"), None);
    assert_eq!(parse_time("2026-08-29T17:49:56+\u{e9}\u{e9}"), None);
    assert_eq!(parse_time("2026-08-29T17:49:56Zulu"), None);
    // Without an offset: the PC's wall clock, through `local`.
    assert_eq!(parse_time("2026-08-29T17:49:56"), Some(1788025796));
    let in_berlin = |wall: i64| Some(wall - 7200);
    assert_eq!(
        parse_time_with("2026-08-29T19:49:56.1234567", &in_berlin),
        Some(1788025796)
    );
    assert_eq!(
        parse_time_with("2026-08-29T19:49:56+00:00", &in_berlin),
        Some(1788025796 + 7200)
    );
    assert_eq!(parse_time_with("2026-08-29T19:49:56", &|_| None), None);
    // A missing or empty time is never played; one that doesn't parse is unknown.
    let Message::Games(games) = parse_with(
        r#"{"type":"games","payload":[
                {"id":"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa","lastPlayed":""},
                {"id":"bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb"},
                {"id":"cccccccc-cccc-cccc-cccc-cccccccccccc","lastPlayed":"last Tuesday"},
                {"id":"dddddddd-dddd-dddd-dddd-dddddddddddd","lastPlayed":"2026-08-29T19:49:56"}]}"#,
        &in_berlin,
    ) else {
        panic!()
    };
    let played: Vec<_> = games
        .iter()
        .map(|g| (g.last_played, g.last_played_unknown))
        .collect();
    assert_eq!(
        played,
        [
            (None, false),
            (None, false),
            (None, true),
            (Some(1788025796), false)
        ]
    );
    assert_eq!(format_time(1788025796), "2026-08-29T17:49:56Z");
    assert_eq!(
        parse(
            r#"{"type":"commandResult","command":"set-cover","requestId":"cover-1","success":false,"error":"Playnite rejected the cover metadata update"}"#
        ),
        Message::CommandResult {
            command: "set-cover".into(),
            request_id: "cover-1".into(),
            success: false,
            error: "Playnite rejected the cover metadata update".into(),
        }
    );
    assert!(matches!(
        parse(r#"{"type":"commandResult","requestId":"cover-2","success":true}"#),
        Message::CommandResult { success: true, .. }
    ));
    let line = "\u{feff}{\"type\":\"games\",\"payload\":[{\"id\":\"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa\",\"name\":\"Nightfire\",\"categories\":[\"Shooter\"],\"playtimeMinutes\":\"42\",\"lastPlayed\":\"2026-08-29T17:49:56Z\",\"pluginId\":\"P\"},{\"id\":\"bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb\",\"installed\":false},{\"name\":\"no id\"}]}";
    let Message::Games(games) = parse(line) else {
        panic!()
    };
    assert_eq!(games.len(), 2);
    assert!(games[0].installed && !games[1].installed);
    assert_eq!(games[0].playtime_minutes, 42);
    assert_eq!(games[0].last_played, Some(1788025796));
    assert_eq!(
        parse(
            "{\"type\":\"status\",\"status\":{\"name\":\"gameStarted\",\"id\":\"A1\",\"installDir\":\"C:\\\\G\"}}"
        ),
        Message::Status {
            name: "gameStarted".into(),
            id: "A1".into(),
            install_dir: "C:\\G".into(),
            exe: String::new()
        }
    );
    assert_eq!(parse("not json"), Message::Other);
    assert_eq!(
        plugin_version("Name: x\nVersion: 0.4.14\n").as_deref(),
        Some("0.4.14")
    );
}
fn game(id: &str, name: &str, last: Option<i64>) -> Game {
    Game {
        id: id.into(),
        name: name.into(),
        last_played: last,
        installed: true,
        ..Default::default()
    }
}
#[test]
fn a_steam_game_in_the_export_does_not_duplicate_the_steam_app() {
    let settings = Settings::from_config(&Config::default());
    let mut dota = game("aa", "Dota 2", None);
    dota.plugin_id = "cb91dfc9-b977-43bf-8e70-55f46e410fab".into();
    dota.plugin_name = "Steam".into();
    let mut apps = vec![
        json!({"name":"Dota 2","cmd":"steam://rungameid/570","steam-id":"570","steam-managed":"auto","synced-by":"steam"}),
    ];
    reconcile(
        &mut apps,
        &[dota, game("bb", "Kart", None)],
        &settings,
        0,
        &Artwork::new(),
        true,
        &[],
    );
    let names: Vec<_> = apps.iter().map(|a| field(a, "name")).collect();
    assert_eq!(
        names,
        ["Dota 2", "Kart"],
        "every installed game, Dota 2 once"
    );
    assert_eq!(apps[0]["synced-by"], "steam");
}
#[test]
fn synced_games_follow_the_host_exit_timeout_and_a_users_own_value_stays() {
    let settings = Settings::from_config(&Config::default());
    let mut apps = vec![
        // Synced before the host-wide default: the sync's 10 goes.
        json!({"name":"Kart","playnite-id":"aa","playnite-managed":"auto","synced-by":"playnite","exit-timeout":10}),
        // Saved by the user, so theirs, with the same value: it stays.
        json!({"name":"Party","playnite-id":"bb","exit-timeout":10}),
        json!({"name":"Playnite (Fullscreen)","playnite-fullscreen":true,"exit-timeout":10}),
    ];
    let games = [
        game("aa", "Kart", None),
        game("bb", "Party", None),
        game("cc", "Racer", None),
    ];
    assert!(reconcile(
        &mut apps,
        &games,
        &settings,
        0,
        &Artwork::new(),
        true,
        &[]
    ));
    let timeouts: Vec<_> = apps
        .iter()
        .map(|app| {
            (
                field(app, "name").to_owned(),
                app.get("exit-timeout").cloned(),
            )
        })
        .collect();
    assert_eq!(
        timeouts,
        [
            ("Kart".into(), None),
            ("Party".into(), Some(json!(10))),
            ("Playnite (Fullscreen)".into(), Some(json!(10))),
            ("Racer".into(), None),
        ]
    );
    assert!(fullscreen_entry(&mut apps, true));
    assert!(apps[2].get("exit-timeout").is_none());
    assert!(!fullscreen_entry(&mut apps, true));
    apps.retain(|app| field(app, "name") != "Playnite (Fullscreen)");
    assert!(fullscreen_entry(&mut apps, true));
    assert!(apps.last().unwrap().get("exit-timeout").is_none());
}
#[test]
fn sync_selects_recent_games_and_keeps_user_apps() {
    let now = 100 * 86400;
    let mut settings = Settings::from_config(&Config::default());
    assert!(
        settings.sync_all_installed,
        "every installed game, as for Steam"
    );
    settings.sync_all_installed = false;
    settings.recent_games = 2;
    let games = vec![
        game("aa", "Old", Some(now - 40 * 86400)),
        game("bb", "Newer", Some(now - 86400)),
        game("cc", "Newest", Some(now - 60)),
        Game {
            installed: false,
            ..game("dd", "Gone", Some(now))
        },
    ];
    let mut apps = vec![
        json!({"name": "Desktop"}),
        // Added by hand with the game's name: the game is in the library, and the app stays
        // the user's.
        json!({"name": "  newest ", "cmd": "C:/x.exe"}),
        json!({"name": "Gone", "playnite-id": "dd", "playnite-managed": "auto", "synced-by": "playnite"}),
    ];
    assert!(reconcile(
        &mut apps,
        &games,
        &settings,
        now,
        &Artwork::new(),
        true,
        &[]
    ));
    let names: Vec<_> = apps.iter().map(|a| field(a, "name").to_owned()).collect();
    assert_eq!(names, ["Desktop", "  newest ", "Newer"]);
    assert_eq!(apps[1]["cmd"], "C:/x.exe");
    assert!(apps[1].get("playnite-managed").is_none());
    assert_eq!(apps[2]["uuid"], "BB");
    assert_eq!(apps[2]["synced-by"], "playnite");
    assert_eq!(apps[2]["playnite-source"], "recent");
    assert_eq!(apps[2]["playnite-added-at"], format_time(now));
    assert!(!reconcile(
        &mut apps,
        &games,
        &settings,
        now,
        &Artwork::new(),
        true,
        &[]
    ));
    // A game played since replaces the oldest managed one.
    let mut games = games;
    games[0].last_played = Some(now);
    assert!(reconcile(
        &mut apps,
        &games,
        &settings,
        now + 1,
        &Artwork::new(),
        true,
        &[]
    ));
    let ids: Vec<_> = apps
        .iter()
        .map(|a| field(a, "playnite-id").to_owned())
        .collect();
    assert_eq!(ids, ["", "", "aa"]);
    // Without auto sync only linked apps are refreshed.
    games[0].name = "Renamed".into();
    assert!(reconcile(
        &mut apps,
        &games,
        &settings,
        now + 2,
        &Artwork::new(),
        false,
        &[]
    ));
    assert_eq!(apps.len(), 3);
    assert_eq!(apps[2]["name"], "Renamed");
}
#[test]
fn the_sync_leaves_apps_it_did_not_stamp_and_games_deleted_from_the_library() {
    let now = 100 * 86400;
    let mut settings = Settings::from_config(&Config::default());
    settings.sync_all_installed = true;
    let games = vec![
        game("aa", "Kart", Some(now)),
        game("bb", "Party", Some(now)),
        Game {
            installed: false,
            ..game("cc", "Gone", Some(now))
        },
    ];
    // Vibepollo's synced apps, imported without the stamp, and a stamped one renamed by hand.
    let mut apps = vec![
        json!({"name": "My Kart", "playnite-id": "aa", "playnite-managed": "auto", "cmd": "C:/kart.exe"}),
        json!({"name": "Gone", "playnite-id": "cc", "playnite-managed": "auto", "playnite-source": "installed"}),
    ];
    let artwork = Artwork::from([(
        "aa".into(),
        (Some(PathBuf::from("C:\\covers\\aa.png")), None),
    )]);
    assert!(reconcile(
        &mut apps,
        &games,
        &settings,
        now,
        &artwork,
        true,
        &["playnite:bb".into()]
    ));
    assert_eq!(
        apps.len(),
        2,
        "Party was deleted: not added again; Gone stays"
    );
    assert_eq!(apps[0]["name"], "My Kart");
    assert_eq!(apps[0]["cmd"], "C:/kart.exe");
    assert_eq!(apps[0]["image-path"], "C:/covers/aa.png");
    assert!(apps[0].get("synced-by").is_none());
    assert_eq!(apps[1]["name"], "Gone");
    assert_eq!(purge(&mut apps), 0);
    // Once the deletion is lifted, the game joins as the sync's own app.
    assert!(reconcile(
        &mut apps,
        &games,
        &settings,
        now,
        &artwork,
        true,
        &[]
    ));
    assert_eq!(apps.len(), 3);
    assert_eq!(apps[2]["synced-by"], "playnite");
}
#[test]
fn exclusions_categories_ttl_and_the_fullscreen_entry() {
    let now = 1000 * 86400;
    let mut settings = Settings::from_config(
        &Config::parse("playnite_sync_all_installed = false\nplaynite_recent_games = 0\nplaynite_sync_categories = [{\"id\":\"1\",\"name\":\"Couch\"}]\nplaynite_exclude_games = [\"bb\"]\n").unwrap(),
    );
    assert_eq!(settings.sync_categories, ["Couch"]);
    let mut couch = game("aa", "Kart", None);
    couch.categories = vec!["couch".into()];
    let mut excluded_game = game("bb", "Party", None);
    excluded_game.categories = vec!["Couch".into()];
    let mut apps = vec![
        json!({"name":"Stale","playnite-id":"zz","playnite-managed":"auto","synced-by":"playnite","playnite-added-at":format_time(now - 20 * 86400)}),
    ];
    assert!(reconcile(
        &mut apps,
        &[couch, excluded_game],
        &settings,
        now,
        &Artwork::new(),
        true,
        &[]
    ));
    let ids: Vec<_> = apps
        .iter()
        .map(|a| field(a, "playnite-id").to_owned())
        .collect();
    assert_eq!(ids, ["aa"]);
    assert_eq!(apps[0]["playnite-source"], "category");
    settings.fullscreen_entry = true;
    assert!(fullscreen_entry(&mut apps, true));
    assert!(!fullscreen_entry(&mut apps, true));
    assert!(fullscreen_entry(&mut apps, false));
    assert_eq!(apps.len(), 1);
    // The user's own apps of that name or command are not the host's to remove.
    apps.push(json!({"name":"Playnite (Fullscreen)","cmd":"C:/Mine/start.bat"}));
    apps.push(json!({"name":"Big screen","cmd":"playnite-launcher.exe --fullscreen"}));
    assert!(!fullscreen_entry(&mut apps, false));
    assert!(
        !fullscreen_entry(&mut apps, true),
        "a user's entry counts as present"
    );
    assert_eq!(apps.len(), 3);
    apps.truncate(1);
    assert_eq!(purge(&mut apps), 1);
}

fn focus_settings(attempts: i64, timeout: i64, exit_on_first: bool) -> Settings {
    let mut settings = Settings::from_config(&Config::default());
    settings.focus_attempts = attempts;
    settings.focus_timeout_secs = timeout;
    settings.focus_exit_on_first = exit_on_first;
    settings
}
#[test]
fn focus_defaults_match_vibepollo_and_zero_turns_it_off() {
    let settings = Settings::from_config(&Config::default());
    assert_eq!(
        (settings.focus_attempts, settings.focus_timeout_secs),
        (3, 15)
    );
    assert!(!settings.focus_exit_on_first);
    let now = Instant::now();
    assert!(Focus::arm(&focus_settings(0, 15, false), now).is_none());
    assert!(Focus::arm(&focus_settings(3, 0, true), now).is_none());
}
/// Issue #83: the wait after a game closes is a setting; unset, it stays 15 s.
#[test]
fn the_wait_after_a_game_closes_is_fifteen_seconds_unless_set() {
    let wait = |config: &str| Settings::from_config(&Config::parse(config).unwrap()).exit_wait;
    assert_eq!(wait(""), Duration::from_secs(15));
    assert_eq!(
        wait("playnite_exit_wait_secs = 5\n"),
        Duration::from_secs(5)
    );
    assert_eq!(wait("playnite_exit_wait_secs = 0\n"), Duration::ZERO);
    assert_eq!(wait("playnite_exit_wait_secs = -3\n"), Duration::ZERO);
    assert_eq!(wait("playnite_exit_wait_secs = 900\n"), EXIT_WAIT_MAX);
}
#[test]
fn focus_retries_once_a_second_until_confirmed_the_set_number_of_times() {
    let at = Instant::now();
    let mut focus = Focus::arm(&focus_settings(2, 15, false), at).unwrap();
    assert!(focus.due(at));
    focus.checked(at, false);
    assert!(!focus.due(at + Duration::from_millis(500)));
    let second = at + Focus::INTERVAL;
    assert!(focus.due(second));
    focus.checked(second, true);
    assert!(!focus.finished(second));
    let third = second + Focus::INTERVAL;
    focus.checked(third, true);
    assert!(focus.finished(third));
    assert!(!focus.due(third + Focus::INTERVAL));
}
#[test]
fn focus_can_stop_at_the_first_success_and_always_stops_at_the_timeout() {
    let at = Instant::now();
    let mut first = Focus::arm(&focus_settings(5, 15, true), at).unwrap();
    first.checked(at, true);
    assert!(first.finished(at));
    let mut timed = Focus::arm(&focus_settings(5, 3, false), at).unwrap();
    timed.checked(at, false);
    assert!(timed.due(at + Duration::from_secs(2)));
    assert!(!timed.due(at + Duration::from_secs(3)));
    assert!(timed.finished(at + Duration::from_secs(3)));
}
