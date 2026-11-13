use super::*;

#[test]
fn bitrate_scales_with_pixels_and_rate() {
    assert_eq!(default_bitrate_kbps(1920, 1080, 60), 20_000);
    assert_eq!(default_bitrate_kbps(1280, 720, 60), 8_889);
    assert_eq!(default_bitrate_kbps(3840, 2160, 60), 80_000);
    assert!(default_bitrate_kbps(1920, 1080, 120) < 40_000);
    assert_eq!(default_bitrate_kbps(640, 360, 30), 2_000);
}

fn recent(host: &str, install: &str, app: u32, at: u64) -> Recent {
    Recent { host: host.into(), install_id: install.into(), app_id: app, title: format!("Game {app}"), at }
}

#[test]
fn recents_put_the_latest_first_without_repeats_and_stay_capped() {
    let mut list = Vec::new();
    remember(&mut list, recent("pc", "A", 1, 10));
    remember(&mut list, recent("pc", "B", 2, 20));
    remember(&mut list, recent("other", "A", 1, 30));
    // The same game again (install ids compare without case) moves up instead of repeating.
    remember(&mut list, recent("pc", "a", 7, 40));
    assert_eq!(list.iter().map(|r| (r.host.as_str(), r.at)).collect::<Vec<_>>(), [("pc", 40), ("other", 30), ("pc", 20)]);
    // Without install ids the app id decides.
    remember(&mut list, recent("pc", "", 2, 50));
    remember(&mut list, recent("pc", "", 2, 60));
    assert_eq!(list.len(), 3);
    for n in 0..30 {
        remember(&mut list, recent("pc", &format!("G{n}"), 100 + n, 100 + u64::from(n)));
    }
    assert_eq!(list.len(), RECENT_LIMIT);
    assert_eq!(list[0].install_id, "G29");
}

#[test]
fn the_automatic_start_defaults_to_the_pcs_and_keeps_its_state_over_a_restart() {
    let old: Settings = serde_json::from_str("{}").unwrap();
    assert_eq!(old.open_on_start, OpenOnStart::Pcs, "a settings file from before it starts nothing");
    let dir = std::env::temp_dir().join(format!("nightjar-open-play-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = Store::at(dir.clone()).unwrap();
    assert_eq!(store.open_play(), OpenPlayState::default());
    let game = OpenGame { host: "uid".into(), install_id: "x".into(), app_id: 7, title: "Game".into() };
    store.save_open_play(&OpenPlayState { game: Some(game.clone()), armed: true }).unwrap();
    assert_eq!(Store::at(dir.clone()).unwrap().open_play(), OpenPlayState { game: Some(game), armed: true });
    store.forget_host("other", &[], &[]);
    assert!(store.open_play().game.is_some(), "another PC's removal leaves it");
    store.forget_host("uid", &[], &[]);
    assert_eq!(store.open_play(), OpenPlayState { game: None, armed: true }, "its PC's removal takes it");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn recents_survive_a_restart() {
    let dir = std::env::temp_dir().join(format!("nightjar-recent-{}", std::process::id()));
    let store = Store::at(dir.clone()).unwrap();
    assert!(store.recents().is_empty());
    let mut list = Vec::new();
    remember(&mut list, recent("pc", "A", 1, 10));
    store.save_recents(&list).unwrap();
    assert_eq!(Store::at(dir.clone()).unwrap().recents(), list);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn the_second_screens_store_reads_the_files_and_writes_none() {
    let dir = std::env::temp_dir().join(format!("nightjar-read-only-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = Store::at(dir.clone()).unwrap();
    let settings = Settings { dual_screen: true, second_screen_width: 1280, second_screen_height: 720, ..Settings::default() };
    store.save_settings(&settings).unwrap();
    store.save_hosts(&[HostRecord { name: "pc".into(), ..HostRecord::default() }]).unwrap();
    let before: Vec<_> = ["settings.json", "hosts.json"].iter().map(|f| std::fs::read(dir.join(f)).unwrap()).collect();

    let second = Store::at(dir.clone()).unwrap().read_only();
    assert_eq!(second.settings(), settings, "it reads what the first wrote");
    assert_eq!(second.hosts()[0].name, "pc");
    second.save_settings(&Settings::default()).unwrap();
    second.save_hosts(&[]).unwrap();
    second.save_recents(&[recent("pc", "A", 1, 10)]).unwrap();
    second.save_hotkeys(&BTreeMap::from([("overlay".to_owned(), "Ctrl+O".to_owned())])).unwrap();
    let library = second.library_path("pc");
    std::fs::create_dir_all(library.parent().unwrap()).unwrap();
    std::fs::write(&library, "{}").unwrap();
    second.forget_host("pc", &[], &[]);
    assert!(library.exists(), "a PC's library stays");
    let after: Vec<_> = ["settings.json", "hosts.json"].iter().map(|f| std::fs::read(dir.join(f)).unwrap()).collect();
    assert_eq!(after, before);
    for written in ["recent.json", HOTKEYS_FILE] {
        assert!(!dir.join(written).exists(), "{written} written");
    }
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn one_unreadable_value_costs_neither_the_pcs_nor_the_other_settings() {
    let dir = std::env::temp_dir().join(format!("nightjar-unreadable-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = Store::at(dir.clone()).unwrap();
    let kept = |name: &str| std::fs::read_to_string(dir.join(name)).ok();

    // A codec a newer version wrote, as after a downgrade.
    let settings = r#"{"fps":90,"codec":"a newer version's codec"}"#;
    std::fs::write(dir.join("settings.json"), settings).unwrap();
    let loaded = store.settings();
    assert_eq!((loaded.fps, loaded.codec), (90, Settings::default().codec));
    assert_eq!(kept("settings.json.bad").as_deref(), Some(settings), "the next save replaces the file");

    // The same in one PC's and one game's settings, and a PC that can't be read at all.
    let hosts = r#"[
            {"unique_id":"a","name":"A","server_certificate":"pem","settings":{"fps":120,"codec":"newer"},
             "app_settings":{"id:7":{"codec":"newer","bitrate_kbps":5000}}},
            {"unique_id":"b","name":"B","http_port":"not a port"},
            {"unique_id":"c","name":"C","server_certificate":"pem","settings":null}]"#;
    std::fs::write(dir.join("hosts.json"), hosts).unwrap();
    let loaded = store.hosts();
    assert_eq!(loaded.iter().map(|host| host.unique_id.as_str()).collect::<Vec<_>>(), ["a", "c"]);
    assert_eq!(loaded[0].server_certificate.as_deref(), Some("pem"), "the pairing stays");
    let pc = loaded[0].settings.as_ref().unwrap();
    assert!(pc.has(&["fps"]) && !pc.has(&["codec"]));
    assert_eq!(pc.apply(&Settings::default()).fps, 120);
    let game = &loaded[0].app_settings["id:7"];
    assert!(game.has(&["bitrate_kbps"]) && !game.has(&["codec"]));
    assert_eq!(loaded[1].settings, None);
    assert_eq!(kept("hosts.json.bad").as_deref(), Some(hosts));

    let profiles = r#"{"active":1,"entries":[{"id":1,"name":"Couch","settings":{"fps":30,"codec":"newer"}}]}"#;
    std::fs::write(dir.join("profiles.json"), profiles).unwrap();
    let loaded = store.profiles();
    assert_eq!(loaded.active, Some(1));
    assert!(loaded.entries[0].settings.has(&["fps"]) && !loaded.entries[0].settings.has(&["codec"]));
    assert_eq!(kept("profiles.json.bad").as_deref(), Some(profiles));

    // A file that is no JSON reads as none, and is kept beside the earlier copy.
    std::fs::write(dir.join("hosts.json"), "[{ cut off").unwrap();
    assert!(store.hosts().is_empty());
    assert_eq!(kept("hosts.json.bad.2").as_deref(), Some("[{ cut off"));

    // A file that reads whole leaves no copy, and the second screen's store writes none.
    std::fs::remove_file(dir.join("settings.json.bad")).unwrap();
    std::fs::write(dir.join("settings.json"), r#"{"fps":90}"#).unwrap();
    assert_eq!(store.settings().fps, 90);
    std::fs::write(dir.join("settings.json"), settings).unwrap();
    assert_eq!(Store::at(dir.clone()).unwrap().read_only().settings().fps, 90);
    assert_eq!(kept("settings.json.bad"), None);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn connect_as_survives_a_restart_and_older_mappings_load_as_automatic() {
    let dir = std::env::temp_dir().join(format!("nightjar-connect-as-{}", std::process::id()));
    let store = Store::at(dir.clone()).unwrap();
    // A file from before the choice existed: the mapping stays and connects as Automatic.
    std::fs::write(dir.join("controllers.json"),
        r#"{"usb:1532:0717":{"name":"Razer Kishi","buttons":[null,null,null,null,null,null,null,null,null,null,null,null,null,null,null,null,null,null,null,null,null],"axes":[1,2,3,4,5,6],"deadzone":7}}"#).unwrap();
    let older = store.controller_mappings();
    assert_eq!(older["usb:1532:0717"].deadzone, 7);
    assert_eq!(older["usb:1532:0717"].connect_as, 0);
    let mut mapping = older["usb:1532:0717"].clone();
    mapping.connect_as = 1;
    let mappings = BTreeMap::from([("usb:1532:0717".to_string(), mapping)]);
    store.save_controller_mappings(&mappings).unwrap();
    assert_eq!(Store::at(dir.clone()).unwrap().controller_mappings(), mappings);
    // A type a newer Nightjar adds, or a value no build writes, reads as Automatic and keeps
    // this and every other mapping.
    for bad in ["4", "256", "-1", "\"x\"", "null", "1.5"] {
        std::fs::write(dir.join("controllers.json"), format!(
            r#"{{"usb:1532:0717":{{"name":"Razer Kishi","deadzone":7,"connect_as":{bad}}},"usb:045e:028e":{{"name":"Xbox 360","connect_as":2}}}}"#)).unwrap();
        let loaded = store.controller_mappings();
        assert_eq!(loaded.len(), 2, "{bad}");
        assert_eq!(loaded["usb:1532:0717"].deadzone, 7, "{bad}");
        assert_eq!(loaded["usb:1532:0717"].connect_as, 0, "{bad}");
        assert_eq!(loaded["usb:1532:0717"].arrival_type(2), 2, "{bad}");
        assert_eq!(loaded["usb:045e:028e"].connect_as, 2, "{bad}");
    }
    std::fs::remove_dir_all(dir).ok();
}

fn star(host: &str, install: &str, app: u32) -> Favourite {
    Favourite { host: host.into(), install_id: install.into(), app_id: app, title: format!("Game {app}") }
}

#[test]
fn a_star_goes_first_and_the_same_game_on_another_pc_is_its_own_favourite() {
    let mut list = Vec::new();
    assert!(toggle_favourite(&mut list, star("pc", "A", 1)));
    assert!(toggle_favourite(&mut list, star("other", "A", 1)));
    assert!(toggle_favourite(&mut list, star("pc", "B", 2)));
    assert_eq!(list.iter().map(|f| (f.host.as_str(), f.app_id)).collect::<Vec<_>>(), [("pc", 2), ("other", 1), ("pc", 1)]);
    // Starring again removes it (install ids compare without case), and only on that PC.
    assert!(!toggle_favourite(&mut list, star("pc", "a", 9)));
    assert_eq!(list.iter().map(|f| (f.host.as_str(), f.app_id)).collect::<Vec<_>>(), [("pc", 2), ("other", 1)]);
    // Without install ids the app id decides.
    assert!(toggle_favourite(&mut list, star("pc", "", 5)));
    assert!(!toggle_favourite(&mut list, star("pc", "", 5)));
    assert_eq!(list.len(), 2);
}

#[test]
fn hidden_games_survive_a_restart_and_show_again() {
    let dir = std::env::temp_dir().join(format!("nightjar-hidden-{}", std::process::id()));
    let store = Store::at(dir.clone()).unwrap();
    assert!(store.hidden_games().is_empty());
    let mut list = Vec::new();
    assert!(set_hidden(&mut list, vec![star("pc", "a", 1), star("pc2", "a", 1)], true));
    assert!(!set_hidden(&mut list, vec![star("pc", "a", 1)], true), "hidden already");
    assert_eq!(list.len(), 2);
    store.save_hidden_games(&list).unwrap();
    assert_eq!(Store::at(dir.clone()).unwrap().hidden_games(), list);
    assert!(set_hidden(&mut list, vec![star("pc", "A", 9)], false), "by install id, in any case");
    assert!(!set_hidden(&mut list, vec![star("pc", "other", 9)], false));
    assert!(set_hidden(&mut list, vec![star("pc", "a", 1)], true));
    assert!(set_hidden(&mut list, vec![star("pc", "uuid-of-the-pcs-list", 1)], false), "by app id, from the PC's list");
    assert_eq!(list, [star("pc2", "a", 1)]);
    std::fs::write(dir.join("hidden-games.json"), "{ not json").unwrap();
    assert!(store.hidden_games().is_empty());
    assert_eq!(std::fs::read_to_string(dir.join("hidden-games.json.bak")).unwrap(), "{ not json");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn favourites_survive_a_restart_and_a_broken_file_reads_as_none() {
    let dir = std::env::temp_dir().join(format!("nightjar-favourites-{}", std::process::id()));
    let store = Store::at(dir.clone()).unwrap();
    assert!(store.favourites().is_empty());
    let mut list = Vec::new();
    toggle_favourite(&mut list, star("pc", "A", 1));
    toggle_favourite(&mut list, star("pc", "B", 2));
    store.save_favourites(&list).unwrap();
    assert_eq!(Store::at(dir.clone()).unwrap().favourites(), list);
    std::fs::write(dir.join("favourites.json"), "{ not json").unwrap();
    assert!(store.favourites().is_empty());
    // The unreadable file is kept aside, not silently lost.
    assert_eq!(std::fs::read_to_string(dir.join("favourites.json.bak")).unwrap(), "{ not json");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_bad_favourite_is_skipped_and_the_rest_and_the_original_are_kept() {
    let text = r#"[
            {"host":"pc","install_id":"A","app_id":1,"title":"One"},
            {"host":"pc"},
            {"host":"pc","install_id":"B","app_id":"x","title":"Two"},
            {"host":"pc","install_id":"C","app_id":3,"title":"Three"}
        ]"#;
    let (list, damaged) = parse_favourites(text);
    assert!(damaged);
    assert_eq!(list.iter().map(|f| f.title.as_str()).collect::<Vec<_>>(), ["One", "Three"]);
    assert_eq!(parse_favourites("[]"), (Vec::new(), false));

    let dir = std::env::temp_dir().join(format!("nightjar-favourites-bad-{}", std::process::id()));
    let store = Store::at(dir.clone()).unwrap();
    std::fs::write(dir.join("favourites.json"), text).unwrap();
    assert_eq!(store.favourites().len(), 2);
    assert_eq!(std::fs::read_to_string(dir.join("favourites.json.bak")).unwrap(), text);
    std::fs::remove_dir_all(dir).ok();
}

/// stream_hotkeys.json is read entry by entry; one that doesn't read, or one a newer Nightjar
/// wrote, is never lost to the next change.
#[test]
fn a_corrupt_or_newer_hotkeys_file_is_kept() {
    let dir = std::env::temp_dir().join(format!("nightjar-hotkeys-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    let store = Store::at(dir.clone()).unwrap();
    let path = dir.join("stream_hotkeys.json");
    assert!(store.hotkeys().is_empty(), "no file");
    let bindings = nightjar_policy::shortcuts::HotkeyBindings::from([("quit".to_string(), "CTRL+F9".to_string())]);
    store.save_hotkeys(&bindings).unwrap();
    assert_eq!(store.hotkeys(), bindings);
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved, serde_json::json!({"version": 1, "bindings": {"quit": "CTRL+F9"}}));

    // One bad value: the others still read, and it stays in the file.
    let text = r#"{"version": 1, "bindings": {"quit": "CTRL+F9", "overlay": 7, "capture": "off"}}"#;
    std::fs::write(&path, text).unwrap();
    let read = store.hotkeys();
    assert_eq!((read.get("quit").map(String::as_str), read.get("capture").map(String::as_str), read.get("overlay")), (Some("CTRL+F9"), Some("off"), None));
    store.save_hotkeys(&read).unwrap();
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved["bindings"]["overlay"], 7);

    // A newer version: the entries this one knows are read, the rest and the version kept, the file copied first.
    let newer = r#"{"version": 2, "bindings": {"quit": "CTRL+F9", "teleport": "CTRL+T", "overlay": {"keys": ["CTRL", "O"]}}, "extra": true}"#;
    std::fs::write(&path, newer).unwrap();
    let read = store.hotkeys();
    assert_eq!(read.get("quit").map(String::as_str), Some("CTRL+F9"));
    let mut changed = read.clone();
    changed.insert("capture".into(), "off".into());
    store.save_hotkeys(&changed).unwrap();
    assert_eq!(std::fs::read_to_string(dir.join("stream_hotkeys.json.v2")).unwrap(), newer);
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved["version"], 2);
    assert_eq!(saved["bindings"]["teleport"], "CTRL+T");
    assert_eq!(saved["bindings"]["overlay"], serde_json::json!({"keys": ["CTRL", "O"]}));
    assert_eq!(saved["bindings"]["capture"], "off");

    // Not the format at all: defaults, and the file is copied before a change replaces it.
    for (n, broken) in ["{not json", r#"{"quit": "CTRL+F9"}"#].into_iter().enumerate() {
        std::fs::write(&path, broken).unwrap();
        assert!(store.hotkeys().is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken, "reading never rewrites it");
        store.save_hotkeys(&bindings).unwrap();
        let copy = if n == 0 { "stream_hotkeys.json.unreadable".to_string() } else { format!("stream_hotkeys.json.unreadable.{}", n + 1) };
        assert_eq!(std::fs::read_to_string(dir.join(copy)).unwrap(), broken);
        assert_eq!(store.hotkeys(), bindings);
    }
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn removing_a_pc_clears_its_covers_recent_games_and_favourites() {
    let dir = std::env::temp_dir().join(format!("nightjar-forget-{}", std::process::id()));
    let store = Store::at(dir.clone()).unwrap();
    let mut list = Vec::new();
    remember(&mut list, recent("gone", "A", 1, 10));
    remember(&mut list, recent("kept", "B", 2, 20));
    store.save_recents(&list).unwrap();
    for host in ["gone", "kept"] {
        let cover = store.art_path(host, 1, "");
        std::fs::create_dir_all(cover.parent().unwrap()).unwrap();
        std::fs::write(&cover, b"x").unwrap();
    }
    let mut stars = Vec::new();
    toggle_favourite(&mut stars, star("gone", "A", 1));
    toggle_favourite(&mut stars, star("kept", "B", 2));
    store.save_favourites(&stars).unwrap();
    let (after, starred) = store.forget_host("gone", &list, &stars);
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].host, "kept");
    assert_eq!(store.recents(), after, "the file is filtered too");
    assert_eq!(starred.iter().map(|f| f.host.as_str()).collect::<Vec<_>>(), ["kept"]);
    assert_eq!(store.favourites(), starred, "favourites.json is filtered too");
    assert!(!store.art_path("gone", 1, "").exists(), "its covers are deleted");
    assert!(store.art_path("kept", 1, "").exists(), "another PC's are not");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn addresses_parse() {
    assert_eq!(parse_address("pc.local"), Some(("pc.local".into(), None)));
    assert_eq!(
        parse_address(" 10.0.0.2:48989 "),
        Some(("10.0.0.2".into(), Some(48989)))
    );
    assert_eq!(parse_address("[fe80::1]:47989"), Some(("fe80::1".into(), Some(47989))));
    assert_eq!(parse_address("fe80::1"), Some(("fe80::1".into(), None)));
    assert_eq!(parse_address("pc:notaport"), None);
    assert_eq!(parse_address(""), None);
}

#[test]
fn saved_libraries_and_versioned_covers() {
    let library: nightjar_core::http::native_api::library::Library = serde_json::from_str(include_str!("../../../client/tests/fixtures/native-api/library.json")).unwrap();
    let dir = std::env::temp_dir().join(format!("nightjar-library-{}", std::process::id()));
    let store = Store::at(dir.clone()).unwrap();
    let host = "5E0F4B1C-7A3D-4C52-9E10-0B6F2D8A4C11";
    assert!(store.library(host).is_none());
    let mut library = NativeLibrary::from(library);
    library.last_played.insert(library.library.games[0].install_id.clone(), 1788025796);
    store.save_library(host, &library).unwrap();
    let back = store.library(host).unwrap();
    assert_eq!((back.library.revision.as_str(), back.library.games.len()), ("9f2c41d0", 2));
    assert_eq!(back, library, "with when its games were last played");
    // Another PC does not see it, and a path-like id cannot leave the folder.
    assert!(store.library("other").is_none());
    assert!(store.library_path("../../x").starts_with(dir.join("library")));
    store.forget_library(host);
    assert!(store.library(host).is_none());
    // A new cover version is a new file; no version keeps the classic name.
    assert_ne!(store.art_path(host, 7, "a41c09e2"), store.art_path(host, 7, "0c1d2e3f"));
    assert!(store.art_path(host, 7, "").ends_with("7.img"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_classic_app_list_is_saved_without_its_control_tiles_and_goes_with_the_library() {
    use nightjar_core::http::{App, AppControl};
    let app = |id: i32, name: &str, uuid: &str, control: AppControl| App {
        name: name.into(),
        id,
        hdr_supported: false,
        uuid: uuid.into(),
        host_index: None,
        art_version: if id == 1 { "v2".into() } else { String::new() },
        control,
        monitor_resume: false,
    };
    let apps = ClassicApps::of(&[
        app(1, "Desktop", "A1", AppControl::None),
        app(2, "Terminate", "E16CBE1B-295D-4632-9A76-EC4180C857D3", AppControl::Terminate),
        app(3, "Hades", "", AppControl::None),
    ]);
    assert_eq!(apps.version, CLASSIC_APPS_VERSION);
    assert_eq!(apps.apps.iter().map(|a| (a.id, a.name.as_str(), a.uuid.as_str(), a.art_version.as_str())).collect::<Vec<_>>(), [(1, "Desktop", "A1", "v2"), (3, "Hades", "", "")]);

    let dir = std::env::temp_dir().join(format!("nightjar-classic-apps-{}", std::process::id()));
    let store = Store::at(dir.clone()).unwrap();
    let host = "pc-1";
    assert!(store.classic_apps(host).is_none());
    store.save_classic_apps(host, &apps).unwrap();
    assert_eq!(store.classic_apps(host), Some(apps.clone()));
    assert!(store.classic_apps("other").is_none());
    assert!(store.classic_apps_path("../../x").starts_with(dir.join("apps")));
    // An older or newer file reads too: missing fields take defaults, unknown ones are ignored.
    std::fs::write(store.classic_apps_path("old"), r#"{"apps":[{"id":5}]}"#).unwrap();
    let old = store.classic_apps("old").unwrap();
    assert_eq!((old.version, old.apps[0].id, old.apps[0].name.as_str()), (0, 5, ""));
    std::fs::write(store.classic_apps_path("new"), r#"{"version":2,"apps":[{"id":5,"name":"A","hdr":true}],"more":1}"#).unwrap();
    assert_eq!(store.classic_apps("new").unwrap().apps[0].name, "A");
    // Unpairing or removing the PC drops it with the native library.
    store.forget_library(host);
    assert!(store.classic_apps(host).is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn saving_a_cover_prunes_the_older_versions_of_that_app_only() {
    let dir = std::env::temp_dir().join(format!("nightjar-art-{}", std::process::id()));
    let store = Store::at(dir.clone()).unwrap();
    let host = "5E0F4B1C";
    let old = store.art_path(host, 7, "a41c09e2");
    let new = store.art_path(host, 7, "0c1d2e3f");
    let plain = store.art_path(host, 7, "");
    let other_app = store.art_path(host, 70, "a41c09e2");
    let other_app_plain = store.art_path(host, 8, "");
    std::fs::create_dir_all(old.parent().unwrap()).unwrap();
    for path in [&old, &new, &plain, &other_app, &other_app_plain] {
        std::fs::write(path, b"x").unwrap();
    }
    let mut files = ArtFiles::default();
    files.saved(&new, 7);
    assert!(new.exists(), "the cover just saved stays");
    assert!(!old.exists() && !plain.exists(), "older versions of app 7 go");
    assert!(other_app.exists() && other_app_plain.exists(), "other apps keep theirs (70 is not 7)");
    let _ = std::fs::remove_dir_all(dir);
}

/// A library's covers are saved one after another: the folder is read for the first of them
/// only, and every one still leaves its app a single file.
#[test]
fn saving_many_covers_reads_the_folder_once_and_leaves_no_older_version() {
    let dir = std::env::temp_dir().join(format!("nightjar-art-many-{}", std::process::id()));
    let store = Store::at(dir.clone()).unwrap();
    let host = "5E0F4B1C";
    let folder = store.art_path(host, 0, "").parent().unwrap().to_owned();
    std::fs::create_dir_all(&folder).unwrap();
    // What an earlier run left: a versioned and a plain cover of each app.
    for app in 1..=20 {
        std::fs::write(store.art_path(host, app, "old"), b"x").unwrap();
        std::fs::write(store.art_path(host, app, ""), b"x").unwrap();
    }
    let mut files = ArtFiles::default();
    for app in 1..=20 {
        let new = store.art_path(host, app, "new");
        std::fs::write(&new, b"x").unwrap();
        files.saved(&new, app);
    }
    // A cover that changes again in the same run is known without the folder.
    let newer = store.art_path(host, 7, "newer");
    std::fs::write(&newer, b"x").unwrap();
    files.saved(&newer, 7);
    assert_eq!(files.folders_read, 1, "not once per cover");
    let mut left: Vec<String> = std::fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    left.sort();
    let mut expected: Vec<String> = (1..=20).map(|app| if app == 7 { "7-newer.img".to_owned() } else { format!("{app}-new.img") }).collect();
    expected.sort();
    assert_eq!(left, expected, "one file per app, the newest");

    // Another PC's folder is its own.
    let other = store.art_path("0A1B2C3D", 7, "new");
    std::fs::create_dir_all(other.parent().unwrap()).unwrap();
    std::fs::write(store.art_path("0A1B2C3D", 7, ""), b"x").unwrap();
    std::fs::write(&other, b"x").unwrap();
    files.saved(&other, 7);
    assert_eq!(files.folders_read, 2);
    assert!(other.exists() && !store.art_path("0A1B2C3D", 7, "").exists() && newer.exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_cover_file_names_its_app() {
    assert_eq!(art_app("7.img"), Some(7));
    assert_eq!(art_app("7-0c1d2e3f.img"), Some(7));
    assert_eq!(art_app("881448767-881448767.img"), Some(881448767), "a version of digits is no app id");
    assert_eq!(art_app("-5.img"), Some(-5));
    assert_eq!(art_app("-5-a41c09e2.img"), Some(-5));
    assert_eq!(art_app("7.png"), None);
    assert_eq!(art_app("notes.img"), None);
}

#[test]
fn home_is_on_unless_the_file_says_otherwise() {
    assert!(Settings::default().console_home);
    let old: Settings = serde_json::from_str("{\"fps\": 90}").unwrap();
    assert!(old.console_home, "a settings file from before Home keeps it on");
    let off: Settings = serde_json::from_str("{\"console_home\": false}").unwrap();
    assert!(!off.console_home);
}

#[test]
fn a_settings_file_from_before_accessibility_loads_with_the_defaults() {
    let old: Settings = serde_json::from_str("{\"fps\": 90}").unwrap();
    assert_eq!(old.ui_zoom, 100);
    assert!(!old.high_contrast);
    assert_eq!(old.reduce_motion, ReduceMotion::System);
    assert_eq!(old.language, Language::System);
    let set: Settings =
        serde_json::from_str("{\"ui_zoom\":150,\"high_contrast\":true,\"reduce_motion\":\"Reduce\",\"language\":\"German\"}").unwrap();
    assert_eq!((set.ui_zoom, set.high_contrast, set.reduce_motion, set.language), (150, true, ReduceMotion::Reduce, Language::German));
}

#[test]
fn a_value_from_a_newer_version_does_not_reset_the_other_settings() {
    let set: Settings = serde_json::from_str("{\"fps\":90,\"reduce_motion\":\"Sometimes\",\"language\":\"Klingon\",\"ui_zoom\":130}").unwrap();
    assert_eq!((set.fps, set.ui_zoom), (90, 130));
    assert_eq!((set.reduce_motion, set.language), (ReduceMotion::System, Language::System));
}

#[test]
fn zoom_steps_stay_inside_the_range_and_odd_values_snap() {
    assert_eq!(step_zoom(100, 1), 115);
    assert_eq!(step_zoom(100, -1), 85);
    assert_eq!(step_zoom(150, 1), 150, "the top step stays");
    assert_eq!(step_zoom(85, -1), 85, "the bottom step stays");
    assert_eq!(nearest_zoom(0), 85);
    assert_eq!(nearest_zoom(9999), 150);
    assert_eq!(nearest_zoom(120), 115);
    assert_eq!(step_zoom(120, 1), 130);
}

#[test]
fn early_decode_is_off_unless_the_file_says_otherwise() {
    assert!(!Settings::default().early_decode);
    let old: Settings = serde_json::from_str("{\"fps\": 90}").unwrap();
    assert!(!old.early_decode, "a settings file from before early decode keeps it off");
    let on: Settings = serde_json::from_str("{\"early_decode\": true}").unwrap();
    assert!(on.early_decode);
}

#[test]
fn pyrowave_async_compute_is_off_unless_the_file_says_otherwise() {
    assert!(!Settings::default().pyrowave_async_compute);
    let old: Settings = serde_json::from_str("{}").unwrap();
    assert!(!old.pyrowave_async_compute, "a settings file from before the setting keeps it off");
    let on: Settings = serde_json::from_str("{\"pyrowave_async_compute\": true}").unwrap();
    assert!(on.pyrowave_async_compute);
    let saved = serde_json::to_value(&on).unwrap();
    assert_eq!(saved["pyrowave_async_compute"], serde_json::Value::Bool(true), "saved under its key");
    let back: Settings = serde_json::from_value(saved).unwrap();
    assert!(back.pyrowave_async_compute);
}

#[test]
fn beam_race_is_off_unless_the_file_says_otherwise() {
    assert!(!Settings::default().beam_race);
    let old: Settings = serde_json::from_str("{\"early_decode\": true}").unwrap();
    assert!(!old.beam_race, "a settings file from before the beam race keeps it off");
    let on: Settings = serde_json::from_str("{\"beam_race\": true}").unwrap();
    assert!(on.beam_race);
    assert_eq!(old.beam_race_margin_ms, 0, "the margin is automatic unless the file sets one");
    let margin: Settings = serde_json::from_str("{\"beam_race_margin_ms\": 2}").unwrap();
    assert_eq!(margin.beam_race_margin_ms, 2);
}

#[test]
fn pyrowave_decoder_uses_the_platform_default_and_reads_from_the_file() {
    let default = if cfg!(target_os = "macos") { PyroWaveDecoder::Cpu } else { PyroWaveDecoder::Gpu };
    assert_eq!(Settings::default().pyrowave_decoder, default);
    let old: Settings = serde_json::from_str("{\"fps\": 90}").unwrap();
    assert_eq!(old.pyrowave_decoder, default, "an older file gets the default");
    let cpu: Settings = serde_json::from_str("{\"pyrowave_decoder\": \"Cpu\"}").unwrap();
    assert_eq!(cpu.pyrowave_decoder, PyroWaveDecoder::Cpu, "a saved CPU choice stays");
    let gpu: Settings = serde_json::from_str("{\"pyrowave_decoder\": \"Gpu\"}").unwrap();
    assert_eq!(gpu.pyrowave_decoder, PyroWaveDecoder::Gpu);
    let mut overrides = Overrides::default();
    overrides.0.insert("pyrowave_decoder".into(), serde_json::json!("Gpu"));
    assert_eq!(overrides.apply(&Settings::default()).pyrowave_decoder, PyroWaveDecoder::Gpu);
}

#[test]
fn yuv444_and_sdr10_default_off_and_ask_for_the_gpu_renderer() {
    let old: Settings = serde_json::from_str("{\"fps\": 90}").unwrap();
    assert!(!old.prefer_yuv444 && !old.sdr_10bit, "an older file gets them off");
    assert!(!Settings::default().needs_gpu());
    let on: Settings = serde_json::from_str("{\"prefer_yuv444\": true}").unwrap();
    assert!(on.needs_gpu(), "Linux draws 4:4:4 pictures with the plane pass");
    let on: Settings = serde_json::from_str("{\"sdr_10bit\": true}").unwrap();
    assert!(on.needs_gpu(), "and software 10-bit pictures too");
    // A game or PC that turns one on starts Nightjar with the GPU renderer as well.
    let mut overrides = Overrides::default();
    assert!(!overrides.needs_gpu());
    overrides.0.insert("sdr_10bit".into(), serde_json::json!(true));
    assert!(overrides.needs_gpu());
}

#[test]
fn hdr_brightness_is_the_pcs_by_default_and_a_pc_can_follow_the_desktop() {
    assert!(!Settings::default().hdr_desktop_brightness, "absolute: what HDR10 means and direct scanout needs");
    let old: Settings = serde_json::from_str("{\"hdr\": true}").unwrap();
    assert!(!old.hdr_desktop_brightness, "an older file gets the default");
    let on: Settings = serde_json::from_str("{\"hdr_desktop_brightness\": true}").unwrap();
    assert!(on.hdr_desktop_brightness);
    let saved = serde_json::to_value(&on).unwrap();
    assert_eq!(saved["hdr_desktop_brightness"], serde_json::json!(true), "saved under its own key");
    let mut overrides = Overrides::default();
    overrides.0.insert("hdr_desktop_brightness".into(), serde_json::json!(true));
    assert!(overrides.apply(&Settings::default()).hdr_desktop_brightness, "a PC's own setting");
    assert!(!overrides.needs_gpu(), "the renderer HDR takes is enough");
}

#[test]
fn frame_warp_is_off_by_default_and_saved_under_its_own_key() {
    assert!(!Settings::default().frame_warp, "off until the player turns it on");
    let old: Settings = serde_json::from_str("{\"upscale_sharpness\": 70}").unwrap();
    assert!(!old.frame_warp, "an older file gets the default");
    let on: Settings = serde_json::from_str("{\"frame_warp\": true}").unwrap();
    assert!(on.frame_warp);
    let saved = serde_json::to_value(&on).unwrap();
    assert_eq!(saved["frame_warp"], serde_json::json!(true), "saved under its own key");
    let again: Settings = serde_json::from_value(saved).unwrap();
    assert_eq!(again, on, "it round-trips");
    let mut overrides = Overrides::default();
    overrides.0.insert("frame_warp".into(), serde_json::json!(true));
    assert!(overrides.apply(&Settings::default()).frame_warp, "a PC's or game's own setting");
    assert!(!overrides.needs_gpu(), "every renderer draws it");
}

#[test]
fn the_renderer_choice_defaults_to_auto_and_reads_from_the_file() {
    assert_eq!(Settings::default().renderer, Renderer::Auto);
    let old: Settings = serde_json::from_str("{\"fps\": 90}").unwrap();
    assert_eq!(old.renderer, Renderer::Auto, "an older file gets the default");
    for choice in Renderer::ALL {
        let saved = serde_json::to_string(&Settings { renderer: choice, ..Settings::default() }).unwrap();
        assert_eq!(serde_json::from_str::<Settings>(&saved).unwrap().renderer, choice);
    }
    let gpu: Settings = serde_json::from_str("{\"renderer\": \"Gpu\"}").unwrap();
    assert_eq!(gpu.renderer, Renderer::Gpu);
    let opengl: Settings = serde_json::from_str("{\"renderer\": \"OpenGl\"}").unwrap();
    assert_eq!(opengl.renderer, Renderer::OpenGl);
}

#[test]
fn the_gpu_need_names_hdr_only_when_nothing_else_needs_the_renderer() {
    assert_eq!(Settings::default().gpu_need(), None);
    let mut settings = Settings { hdr: true, ..Settings::default() };
    assert_eq!(settings.gpu_need(), Some("HDR"));
    settings.upscaling = Upscaling::Fsr1;
    assert_eq!(settings.gpu_need(), Some("upscaling"), "a stream that needs the renderer even in SDR");
    settings.codec = CodecPreference::PyroWave;
    assert_eq!(settings.gpu_need(), Some("PyroWave"));
}

#[test]
fn files_round_trip() {
    let dir = std::env::temp_dir().join(format!("nightjar-config-{}", std::process::id()));
    let store = Store::at(dir.clone()).unwrap();
    assert_eq!(store.settings(), Settings::default());
    let settings = Settings {
        fps: 120,
        codec: CodecPreference::Av1,
        ..Default::default()
    };
    store.save_settings(&settings).unwrap();
    assert_eq!(store.settings(), settings);
    let mut profiles = Profiles::default();
    let id = profiles.create("Travel", serde_json::from_str(r#"{"fps":60,"hdr":false}"#).unwrap()).unwrap();
    profiles.active = Some(id);
    store.save_profiles(&profiles).unwrap();
    assert_eq!(store.profiles(), profiles);
    let mut shortcuts = crate::shortcuts::Shortcuts::default();
    crate::shortcuts::save(&mut shortcuts, None, "Task manager", "CTRL + SHIFT + ESC").unwrap();
    store.save_shortcuts(&shortcuts).unwrap();
    assert_eq!(store.shortcuts(), shortcuts);
    // Stream hotkeys hold across a restart.
    assert!(store.hotkeys().is_empty());
    let hotkeys = BTreeMap::from([("quit".to_string(), "CTRL+F9".to_string()), ("overlay".into(), "off".into())]);
    store.save_hotkeys(&hotkeys).unwrap();
    assert_eq!(store.hotkeys(), hotkeys);
    let mapping = crate::gamepad::Mapping { name: "Test pad".into(), deadzone: 12, ..Default::default() };
    let mut mappings = BTreeMap::from([("usb:1234:0001".into(), mapping)]);
    store.save_controller_mappings(&mappings).unwrap();
    assert_eq!(store.controller_mappings(), mappings);
    mappings.get_mut("usb:1234:0001").unwrap().axes[0] = -128;
    store.save_controller_mappings(&mappings).unwrap();
    assert!(store.controller_mappings().is_empty());
    let mut hosts = vec![HostRecord {
        unique_id: "ABC".into(),
        name: "Desk".into(),
        address: "10.0.0.2".into(),
        http_port: 47989,
        server_certificate: Some("PEM".into()),
        ..Default::default()
    }];
    hosts[0].save_app_settings("game-a", 1, serde_json::from_str(r#"{"mouse_mode":"Absolute"}"#).unwrap());
    store.save_hosts(&hosts).unwrap();
    assert_eq!(store.hosts(), hosts);
    // A settings file from an older version keeps the new fields' defaults.
    std::fs::write(dir.join("settings.json"), r#"{"fps": 90}"#).unwrap();
    assert_eq!(store.settings().fps, 90);
    assert!(!store.settings().present_at_host_times);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn host_settings_inherit_until_overridden() {
    let mut host: HostRecord = serde_json::from_str(r#"{"address":"pc.local"}"#).unwrap();
    let mut global = Settings { fps: 120, ..Default::default() };
    assert_eq!(host.settings(&global).fps, 120);
    host.settings = Some(serde_json::from_str(r#"{"fps":60,"microphone":true}"#).unwrap());
    global.fps = 144;
    let saved: HostRecord = serde_json::from_str(&serde_json::to_string(&host).unwrap()).unwrap();
    assert_eq!(saved.settings(&global).fps, 60);
    assert!(saved.settings(&global).microphone);
    host.settings = None;
    assert_eq!(host.settings(&global).fps, 144);
}

#[test]
fn a_pcs_own_settings_count_each_game_once_and_clear_to_the_layers_below() {
    // As 4.1.0 wrote it: Frame rate pinned for the PC, one game with a UUID (saved under both
    // keys) and one without.
    let mut host: HostRecord = serde_json::from_value(serde_json::json!({
        "address": "pc.local",
        "settings": { "fps": 120 },
        "app_settings": { "uuid:a": { "hdr": true }, "id:1": { "hdr": true }, "id:2": { "fps": 30 } },
    })).unwrap();
    assert_eq!(host.own_settings(), (true, 2));
    host.clear_own_settings();
    assert_eq!(host.own_settings(), (false, 0));
    let main = Settings { fps: 60, ..Default::default() };
    assert_eq!(host.settings(&main).fps, 60);
    assert!(host.app_settings("a", 1).is_none());

    let untouched: HostRecord = serde_json::from_str(r#"{"address":"pc.local","settings":{}}"#).unwrap();
    assert_eq!(untouched.own_settings(), (false, 0));
    // Settings saved before the look at kept settings have it still to run; after, it stays done.
    let old: Settings = serde_json::from_str(r#"{"fps":60}"#).unwrap();
    assert!(!old.kept_settings_checked);
    let done = Settings { kept_settings_checked: true, ..old };
    assert!(serde_json::from_str::<Settings>(&serde_json::to_string(&done).unwrap()).unwrap().kept_settings_checked);
}

#[test]
fn a_setting_names_the_layer_it_comes_from() {
    let profile = Profile { id: 1, name: "Travel".into(), settings: serde_json::from_str(r#"{"bitrate_kbps":5000}"#).unwrap() };
    let pc: Overrides = serde_json::from_str(r#"{"fps":120,"bitrate_kbps":9000}"#).unwrap();
    let game: Overrides = serde_json::from_str(r#"{"hdr":true}"#).unwrap();
    assert_eq!(source(&["fps"], Some(&profile), Some(&pc), Some(&game)), Source::Pc);
    assert_eq!(source(&["bitrate_kbps"], Some(&profile), None, Some(&game)), Source::Profile("Travel".into()));
    assert_eq!(source(&["hdr"], Some(&profile), Some(&pc), Some(&game)), Source::Game);
    assert_eq!(source(&["width", "height"], Some(&profile), Some(&pc), Some(&game)), Source::Main);
    assert_eq!(source(&["fps"], None, None, None), Source::Main);
}

#[test]
fn a_pc_alias_round_trips_and_an_old_file_has_none() {
    let old: HostRecord = serde_json::from_str(r#"{"address":"pc.local","name":"DESKTOP-1"}"#).unwrap();
    assert_eq!(old.alias, None);
    let mut host = old;
    host.set_alias("  Living room  ");
    assert_eq!(host.alias.as_deref(), Some("Living room"));
    let saved: HostRecord = serde_json::from_str(&serde_json::to_string(&host).unwrap()).unwrap();
    assert_eq!(saved.alias.as_deref(), Some("Living room"));
    assert_eq!(saved.name, "DESKTOP-1", "the PC's own name is untouched");
    host.set_alias(&"x".repeat(60));
    assert_eq!(host.alias.as_ref().map(|a| a.chars().count()), Some(ALIAS_MAX_CHARS));
    host.set_alias("   ");
    assert_eq!(host.alias, None, "an empty name restores the PC's own");
}

#[test]
fn partial_profiles_and_game_overrides_follow_their_layers() {
    let mut global = Settings { fps: 120, microphone: true, ..Default::default() };
    let mut profiles = Profiles::default();
    let id = profiles.create(" Home ", serde_json::from_str(r#"{"codec":"Av1","fps":90}"#).unwrap()).unwrap();
    profiles.active = Some(id);
    assert!(profiles.create("HOME", Overrides::default()).is_err());
    let mut host = HostRecord { settings: Some(serde_json::from_str(r#"{"fps":60}"#).unwrap()), ..Default::default() };
    host.save_app_settings("game-a", 1, serde_json::from_str(r#"{"hdr":true,"microphone":false}"#).unwrap());
    global.upscale_sharpness = 80;
    let base = host.settings(&profiles.settings(&global, crate::network::Kind::Other));
    let game = host.app_settings("game-a", 1).unwrap().apply(&base);
    assert_eq!(game.fps, 60);
    assert_eq!(game.codec, CodecPreference::Av1);
    assert_eq!(game.upscale_sharpness, 80);
    assert!(game.hdr);
    assert!(!game.microphone);
    assert!(host.app_settings("game-b", 1).is_none());
    assert!(host.app_settings("game-a", 7).is_some());
    assert!(host.app_settings("", 1).is_some());
    host.save_app_settings("game-a", 1, Overrides::default());
    assert!(host.app_settings.is_empty());
    profiles.delete(id);
    assert_eq!(profiles.active, None);
    assert_eq!(profiles.settings(&global, crate::network::Kind::Other), global);
}

#[test]
fn network_choices_fall_back_and_deleted_rules_do_not_select_reused_ids() {
    use crate::network::{Kind, Network};
    let mut profiles = Profiles::default();
    let active = profiles.create("Default", serde_json::from_str(r#"{"fps":60}"#).unwrap()).unwrap();
    let travel = profiles.create("Travel", serde_json::from_str(r#"{"fps":30}"#).unwrap()).unwrap();
    profiles.active = Some(active);
    profiles.mobile = Some(travel);
    profiles.wifi = Some(travel);
    profiles.home = Some(active);
    profiles.home_wifi.push("Home".into());
    profiles.mobile_limit_kbps = 5000;
    let mut network = Network { wifi: true, ssid: Some("Home".into()), ..Default::default() };
    assert_eq!(network.kind(&profiles.home_wifi), Kind::HomeWifi);
    assert_eq!(profiles.selected(Kind::Mobile), Some(active));
    profiles.automatic = true;
    assert_eq!(profiles.settings(&Settings::default(), Kind::Mobile).fps, 30);
    assert_eq!(profiles.selected(Kind::Other), Some(active));
    network.metered = true;
    assert_eq!(network.kind(&profiles.home_wifi), Kind::Mobile);
    assert_eq!(profiles.bitrate_limit(Kind::Mobile), 5000);
    assert_eq!(profiles.bitrate_limit(Kind::HomeWifi), 0);
    network.metered = false;
    network.ssid = None;
    assert_eq!(network.kind(&profiles.home_wifi), Kind::Wifi);
    profiles.delete(travel);
    assert_eq!((profiles.mobile, profiles.wifi), (None, None));
    assert_eq!(profiles.selected(Kind::Mobile), Some(active));
    profiles.mobile = Some(999);
    assert_eq!(profiles.settings(&Settings::default(), Kind::Mobile), Settings::default());
    let restored: Profiles = serde_json::from_str(&serde_json::to_string(&profiles).unwrap()).unwrap();
    assert_eq!(restored, profiles);
}

#[test]
fn telling_the_host_about_wireless_is_on_by_default_and_in_old_files() {
    assert!(Settings::default().tell_host_wireless);
    // A settings file from before the field existed keeps it on.
    let old: Settings = serde_json::from_str(r#"{"fps":60,"auto_reconnect":true}"#).unwrap();
    assert!(old.tell_host_wireless && old.auto_reconnect);
    let off: Settings = serde_json::from_str(r#"{"tell_host_wireless":false}"#).unwrap();
    assert!(!off.tell_host_wireless);
}

#[test]
fn client_roaming_is_on_and_mobile_data_off_by_default_and_in_old_files() {
    let defaults = Settings::default();
    assert!(defaults.client_roaming && !defaults.roaming_metered);
    let old: Settings = serde_json::from_str(r#"{"fps":60,"tell_host_wireless":false}"#).unwrap();
    assert!(old.client_roaming && !old.roaming_metered);
    let set: Settings = serde_json::from_str(r#"{"client_roaming":false,"roaming_metered":true}"#).unwrap();
    assert!(!set.client_roaming && set.roaming_metered);
}

#[test]
fn play_over_the_internet_is_on_by_default_and_in_old_files() {
    assert!(Settings::default().internet_play);
    let old: Settings = serde_json::from_str(r#"{"fps":60}"#).unwrap();
    assert!(old.internet_play);
    let off: Settings = serde_json::from_str(r#"{"internet_play":false}"#).unwrap();
    assert!(!off.internet_play);
    // The host's reply is kept per PC and survives a save.
    let record = HostRecord { internet_play: Some("{\"version\":1}".into()), ..Default::default() };
    let saved: HostRecord = serde_json::from_str(&serde_json::to_string(&record).unwrap()).unwrap();
    assert_eq!(saved.internet_play.as_deref(), Some("{\"version\":1}"));
    let old: HostRecord = serde_json::from_str(r#"{"address":"192.0.2.1"}"#).unwrap();
    assert!(old.internet_play.is_none());
}

#[test]
fn grouped_video_packets_are_on_by_default_and_in_old_files() {
    assert!(Settings::default().receive_coalescing);
    let old: Settings = serde_json::from_str(r#"{"fps":60,"client_roaming":false}"#).unwrap();
    assert!(old.receive_coalescing);
    let off: Settings = serde_json::from_str(r#"{"receive_coalescing":false}"#).unwrap();
    assert!(!off.receive_coalescing);
}

#[test]
fn recover_interrupted_streams_turns_on_once_and_a_later_off_is_kept() {
    assert!(Settings::default().auto_reconnect);
    let dir = std::env::temp_dir().join(format!("nightjar-reconnect-{}", std::process::id()));
    let store = Store::at(dir.clone()).unwrap();
    std::fs::write(dir.join("settings.json"), r#"{"auto_reconnect": false}"#).unwrap();
    let mut settings = store.settings();
    assert!(settings.auto_reconnect, "an old file's off is switched on once");
    settings.auto_reconnect = false;
    store.save_settings(&settings).unwrap();
    assert!(!store.settings().auto_reconnect, "the user's off after the switch is kept");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_saved_pc_render_resolution_is_kept_in_androids_range() {
    let dir = std::env::temp_dir().join(format!("nightjar-render-scale-{}", std::process::id()));
    let store = Store::at(dir.clone()).unwrap();
    for (saved, read) in [(300, 200), (225, 200), (200, 200), (150, 150), (50, 50), (25, 50)] {
        std::fs::write(dir.join("settings.json"), format!(r#"{{"virtual_display_scale": {saved}}}"#)).unwrap();
        assert_eq!(store.settings().virtual_display_scale, read, "settings.json at {saved} %");
        // A profile's, PC's or game's own value is kept in the range too.
        let own: Overrides = serde_json::from_str(&format!(r#"{{"virtual_display_scale": {saved}}}"#)).unwrap();
        assert_eq!(own.apply(&Settings::default()).virtual_display_scale, read, "an override at {saved} %");
    }
    assert_eq!(Settings::default().virtual_display_scale, 100);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_saved_pc_display_scaling_windows_does_not_offer_keeps_the_pcs_own() {
    let dir = std::env::temp_dir().join(format!("nightjar-display-scale-{}", std::process::id()));
    let store = Store::at(dir.clone()).unwrap();
    for (saved, read) in [(175, 175), (500, 500), (0, 0), (275, 0), (600, 0), (99, 0)] {
        std::fs::write(dir.join("settings.json"), format!(r#"{{"virtual_display_dpi": {saved}}}"#)).unwrap();
        assert_eq!(store.settings().virtual_display_dpi, read, "settings.json at {saved} %");
        let own: Overrides = serde_json::from_str(&format!(r#"{{"virtual_display_dpi": {saved}}}"#)).unwrap();
        assert_eq!(own.apply(&Settings::default()).virtual_display_dpi, read, "an override at {saved} %");
    }
    // Files from before the setting keep the PC's own scale and the render resolution apart.
    std::fs::write(dir.join("settings.json"), r#"{"virtual_display_scale": 150}"#).unwrap();
    assert_eq!((store.settings().virtual_display_dpi, store.settings().virtual_display_scale), (0, 150));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn image_enhancements_follow_the_pcs_app_setting_until_a_pc_chooses() {
    let old = serde_json::from_str::<Settings>("{}").unwrap();
    assert_eq!((old.rtx_hdr, old.rtx_video), (None, None));
    assert_eq!((Settings::default().rtx_hdr, Settings::default().rtx_video), (None, None));
    let own: Overrides = serde_json::from_str(r#"{"rtx_hdr": true, "rtx_video": false}"#).unwrap();
    let applied = own.apply(&Settings::default());
    assert_eq!((applied.rtx_hdr, applied.rtx_video), (Some(true), Some(false)));
    let follow: Overrides = serde_json::from_str(r#"{"rtx_hdr": null}"#).unwrap();
    assert_eq!(follow.apply(&Settings { rtx_hdr: Some(true), ..Settings::default() }).rtx_hdr, None);
}

#[test]
fn clipboard_sync_is_off_by_default_and_in_old_files() {
    assert!(!Settings::default().clipboard_sync);
    assert!(!serde_json::from_str::<Settings>("{}").unwrap().clipboard_sync);
    assert!(serde_json::from_str::<Settings>(r#"{"clipboard_sync":true}"#).unwrap().clipboard_sync);
}

#[test]
fn usb_forwarding_is_off_by_default_and_in_old_files() {
    assert!(!Settings::default().usb_forwarding);
    assert!(!serde_json::from_str::<Settings>("{}").unwrap().usb_forwarding);
    assert!(serde_json::from_str::<Settings>(r#"{"usb_forwarding":true}"#).unwrap().usb_forwarding);
    assert!(Settings::default().usb_auto.is_empty());
    assert!(serde_json::from_str::<Settings>("{}").unwrap().usb_auto.is_empty());
    assert_eq!(serde_json::from_str::<Settings>(r#"{"usb_auto":["28de:1304"]}"#).unwrap().usb_auto, ["28de:1304"]);
}

#[test]
fn telling_the_host_the_link_speed_is_on_by_default_and_in_old_files() {
    assert!(Settings::default().tell_host_link_speed);
    let old: Settings = serde_json::from_str(r#"{"fps":60,"tell_host_wireless":false}"#).unwrap();
    assert!(old.tell_host_link_speed && !old.tell_host_wireless);
    let off: Settings = serde_json::from_str(r#"{"tell_host_link_speed":false}"#).unwrap();
    assert!(!off.tell_host_link_speed);
}

#[test]
fn client_hint_bitrate_is_off_by_default_and_in_old_files() {
    assert!(!Settings::default().client_hint_bitrate);
    let old: Settings = serde_json::from_str(r#"{"fps":60,"adaptive_bitrate":true}"#).unwrap();
    assert!(!old.client_hint_bitrate);
    let on: Settings = serde_json::from_str(r#"{"client_hint_bitrate":true}"#).unwrap();
    assert!(on.client_hint_bitrate);
    let back: Settings = serde_json::from_str(&serde_json::to_string(&on).unwrap()).unwrap();
    assert!(back.client_hint_bitrate);
}

#[test]
fn the_link_test_before_the_first_bitrate_is_off_by_default_and_in_old_files() {
    assert!(!Settings::default().link_probe);
    let old: Settings = serde_json::from_str(r#"{"fps":60,"bitrate_kbps":0}"#).unwrap();
    assert!(!old.link_probe);
    let on: Settings = serde_json::from_str(r#"{"link_probe":true}"#).unwrap();
    assert!(on.link_probe);
}

#[test]
fn instant_input_and_stream_priority_are_on_by_default_and_round_trip() {
    let defaults = Settings::default();
    assert!(defaults.unbatched_input && defaults.network_priority);
    // A settings file from before the fields existed turns both on, as Android does.
    let old: Settings = serde_json::from_str(r#"{"fps":60,"network_grace_ms":5000}"#).unwrap();
    assert!(old.unbatched_input && old.network_priority);
    let off = Settings { unbatched_input: false, network_priority: false, ..Settings::default() };
    let back: Settings = serde_json::from_str(&serde_json::to_string(&off).unwrap()).unwrap();
    assert_eq!(back, off);
    assert_eq!(serde_json::from_str::<Settings>(&serde_json::to_string(&defaults).unwrap()).unwrap(), defaults);
}

#[test]
fn calibration_follows_the_route_to_the_host_not_the_internet_connection() {
    use crate::network::{Fallback, Network, Toward};
    let wired = PaceCalibration { pace_mbps: 900, client_link_mbps: 2500, host_link_mbps: 2500,
        wifi: false, ssid: None, qualified: true };
    // Wi-Fi carries the internet, but the PC is on the local Ethernet segment.
    let wifi = Network { wifi: true, ssid: Some("Home".into()), link_mbps: 866, ..Default::default() };
    let route = wifi.toward(Ok(Toward { wireless: false, link_mbps: 2500 }));
    assert_eq!(wired.for_network(&route, 2500), 900);
    // A Wi-Fi calibration stays found when the route to the PC is the Wi-Fi.
    let on_wifi = PaceCalibration { pace_mbps: 400, client_link_mbps: 866, host_link_mbps: 2500,
        wifi: true, ssid: Some("Home".into()), qualified: true };
    let route = wifi.toward(Ok(Toward { wireless: true, link_mbps: 0 }));
    assert_eq!(route.link_mbps, 866);
    assert_eq!(on_wifi.for_network(&route, 2500), 400);
    // Without a route answer the internet connection decides, as before.
    assert_eq!(wired.for_network(&wifi.toward(Err(Fallback::NotTelling)), 2500), 0);
}

#[test]
fn calibration_is_used_only_on_the_measured_network_and_link() {
    let mut network = crate::network::Network { wifi: true, ssid: Some("Home".into()), link_mbps: 1200, ..Default::default() };
    let calibration = PaceCalibration { pace_mbps: 900, client_link_mbps: 1200, host_link_mbps: 2500,
        wifi: true, ssid: Some("Home".into()), qualified: true };
    assert_eq!(calibration.for_network(&network, 2500), 900);
    assert_eq!(calibration.for_network(&network, 1000), 0);
    network.ssid = Some("Hotel".into());
    assert_eq!(calibration.for_network(&network, 2500), 0);
    network.ssid = None;
    assert_eq!(calibration.for_network(&network, 2500), 0);
    let restored: PaceCalibration = serde_json::from_str(&serde_json::to_string(&calibration).unwrap()).unwrap();
    assert_eq!(restored, calibration);
}

#[test]
fn old_pc_snapshots_and_explicit_false_survive_serialization() {
    let settings = Settings { fps: 90, microphone: false, ..Default::default() };
    let old: Overrides = serde_json::from_value(serde_json::to_value(&settings).unwrap()).unwrap();
    assert_eq!(old.apply(&Settings { fps: 144, microphone: true, ..Default::default() }), settings);
    let mut own: Overrides = serde_json::from_str(r#"{"microphone":false}"#).unwrap();
    assert_eq!(serde_json::to_string(&own).unwrap(), r#"{"microphone":false}"#);
    own.clear(&["microphone"]);
    assert!(own.is_empty());
    assert!(serde_json::from_str::<Overrides>(r#"{"fps":"bad"}"#).is_err());
    assert!(serde_json::from_str::<Overrides>(r#"{"codec":"unknown"}"#).is_err());
}

#[test]
fn the_overlay_keeps_androids_values_and_defaults() {
    let defaults = Settings::default();
    assert!(!defaults.overlay);
    assert_eq!((defaults.overlay_mode, defaults.overlay_position), (OverlayMode::Compact, OverlayCorner::TopLeft));
    let old: Settings = serde_json::from_str(r#"{"overlay":true}"#).unwrap();
    assert_eq!((old.overlay, old.overlay_mode, old.overlay_position), (true, OverlayMode::Compact, OverlayCorner::TopLeft));
    let mode = |value: &str| serde_json::from_value::<OverlayMode>(serde_json::json!(value)).unwrap();
    assert_eq!(["compact", "statistics", "debug"].map(mode), OverlayMode::ALL);
    // Android's normalizeMode: the old names, and anything else reads as Compact.
    assert_eq!(["advanced", "expanded", "newer"].map(mode), [OverlayMode::Debug, OverlayMode::Statistics, OverlayMode::Compact]);
    let corner = |value: &str| serde_json::from_value::<OverlayCorner>(serde_json::json!(value)).unwrap();
    assert_eq!(["top_left", "top_right", "bottom_left", "bottom_right"].map(corner), OverlayCorner::ALL);
    assert_eq!(corner("middle"), OverlayCorner::TopLeft);
    let set = Settings { overlay_mode: OverlayMode::Debug, overlay_position: OverlayCorner::BottomRight, ..Default::default() };
    let json = serde_json::to_value(&set).unwrap();
    assert_eq!((&json["overlay_mode"], &json["overlay_position"]), (&serde_json::json!("debug"), &serde_json::json!("bottom_right")));
}

#[test]
fn quick_resume_and_input_only_on_any_pc_are_off_by_default_and_in_old_files() {
    let old: Settings = serde_json::from_str(r#"{"fps":60}"#).unwrap();
    assert!(!old.quick_resume && !old.input_only_any_host);
    assert!(!Settings::default().quick_resume && !Settings::default().input_only_any_host);
    let on: Settings = serde_json::from_str(r#"{"quick_resume":true,"input_only_any_host":true}"#).unwrap();
    assert!(on.quick_resume && on.input_only_any_host);
}

#[test]
fn home_view_keeps_the_rows_per_store_until_chosen_and_reads_back() {
    let old: Settings = serde_json::from_str(r#"{"fps":60}"#).unwrap();
    assert_eq!((old.home_layout, old.home_group, old.home_card_size), (HomeLayout::Rows, HomeGroup::Store, CardSize::Medium));
    assert_eq!(Settings::default().home_layout, HomeLayout::Rows);
    let set = Settings { home_layout: HomeLayout::Grid, home_group: HomeGroup::None, home_card_size: CardSize::Large, ..Default::default() };
    let json = serde_json::to_value(&set).unwrap();
    // The words Android stores for the same choices.
    assert_eq!(
        (&json["home_layout"], &json["home_group"], &json["home_card_size"]),
        (&serde_json::json!("grid"), &serde_json::json!("none"), &serde_json::json!("large"))
    );
    assert_eq!(serde_json::from_value::<Settings>(json).unwrap(), set);
    // By type is Android's "type" too.
    let by_collection: Settings = serde_json::from_str(r#"{"home_group":"collection"}"#).unwrap();
    assert_eq!(by_collection.home_group, HomeGroup::Collection);
    let by_type: Settings = serde_json::from_str(r#"{"home_group":"type"}"#).unwrap();
    assert_eq!(by_type.home_group, HomeGroup::Type);
    assert_eq!(serde_json::to_value(by_type).unwrap()["home_group"], serde_json::json!("type"));
    // A value from a newer Nightjar keeps the default instead of dropping every setting.
    let newer: Settings = serde_json::from_str(r#"{"home_layout":"carousel","home_card_size":"huge","fps":90}"#).unwrap();
    assert_eq!((newer.home_layout, newer.home_card_size, newer.fps), (HomeLayout::Rows, CardSize::Medium, 90));
    assert!(CardSize::Small.scale() < CardSize::Medium.scale() && CardSize::Medium.scale() < CardSize::Large.scale());
}

#[test]
fn quit_on_leave_is_kept_per_game_and_a_known_uuid_decides_alone() {
    let mut record = HostRecord::default();
    assert!(!record.quits_on_leave("game", 5));
    record.set_quit_on_leave("game", 5, true);
    assert_eq!(record.quit_on_leave.iter().map(String::as_str).collect::<Vec<_>>(), ["id:5", "uuid:game"]);
    assert!(record.quits_on_leave("game", 5) && record.quits_on_leave("game", 9));
    assert!(record.quits_on_leave("", 5), "a host without UUIDs matches the ID");
    assert!(!record.quits_on_leave("other", 5), "a reused ID with another UUID does not");
    record.set_quit_on_leave("", 0, true);
    assert_eq!(record.quit_on_leave.len(), 2, "no ID and no UUID keeps nothing");
    assert!(!record.quits_on_leave("", 0));
    let restored: HostRecord = serde_json::from_str(&serde_json::to_string(&record).unwrap()).unwrap();
    assert_eq!(restored, record);
    record.set_quit_on_leave("game", 5, false);
    assert!(record.quit_on_leave.is_empty() && !record.quits_on_leave("", 5));
    let old: HostRecord = serde_json::from_str(r#"{"name":"PC"}"#).unwrap();
    assert!(old.quit_on_leave.is_empty());
}

#[test]
fn files_are_replaced_whole_and_only_the_user_reads_them() {
    let dir = std::env::temp_dir().join(format!("nightjar-private-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    let store = Store::at(dir.clone()).unwrap();
    let identity = Identity { unique_id: "id".into(), certificate_pem: "cert".into(), private_key_pem: "key".into() };
    store.save_identity(&identity).unwrap();
    store.save_identity(&identity).unwrap();
    let names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["identity.json"], "no temporary file is left behind");
    assert!(store.identity().is_some());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join("identity.json")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the private key is the user's alone");
    }
    std::fs::remove_dir_all(dir).ok();
}

/// Settings saved before "Sharper colored text", the shared value lists and the merged
/// Controllers choice read on, in every layer, and a second load changes nothing.
#[test]
fn earlier_settings_read_as_the_merged_choices_in_every_layer() {
    let dir = std::env::temp_dir().join(format!("nightjar-merged-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    let store = Store::at(dir.clone()).unwrap();
    // Main settings: only "Prefer YUV 4:4:4" on, a 12-second wait, desktop's old stepper floors.
    std::fs::write(dir.join("settings.json"), r#"{"prefer_yuv444":true,"network_grace_ms":12000,"quality_min_short_edge":256,"quality_min_fps":75,"controllers":false,"mic_gain":1000}"#).unwrap();
    let settings = store.settings();
    assert!(settings.prefer_yuv444 && settings.experimental_packed_444, "either on reads as Automatic");
    assert_eq!((settings.network_grace_ms, settings.quality_min_short_edge, settings.quality_min_fps), (15_000, 360, 60));
    assert_eq!(settings.mic_gain, 400);
    assert!(!settings.controllers && settings.multi_controller, "Controllers off stays Off; a player each comes back with it");
    store.save_settings(&settings).unwrap();
    assert_eq!(store.settings(), settings, "a second load changes nothing");
    // The defaults stay as they were: 3 s reads as Off and is kept.
    let defaults = Settings::default();
    let mut migrated = defaults.clone();
    migrated.migrate();
    assert_eq!(migrated, defaults);
    assert_eq!((defaults.network_grace_ms, defaults.quality_min_short_edge, defaults.quality_min_fps), (3000, 720, 30));
    assert!(!defaults.prefer_yuv444 && !defaults.experimental_packed_444 && defaults.controllers && defaults.multi_controller && defaults.sops);
    // A PC's, a game's and a profile's own values: either key gets both, neither stays unset.
    let layer = |json: &str| -> Overrides { serde_json::from_str(json).unwrap() };
    let packed = layer(r#"{"experimental_packed_444":true}"#);
    assert!(packed.has(&["prefer_yuv444"]) && packed.apply(&Settings::default()).prefer_yuv444);
    let off = layer(r#"{"prefer_yuv444":false}"#);
    let on = Settings { prefer_yuv444: true, experimental_packed_444: true, ..Settings::default() };
    let applied = off.apply(&on);
    assert!(!applied.prefer_yuv444 && !applied.experimental_packed_444, "a layer's Off wins over the main Automatic");
    let neither = layer(r#"{"fps":120}"#);
    assert!(!neither.has(&SHARP_COLOR_KEYS) && neither.apply(&on).experimental_packed_444, "inherited");
    assert_eq!(layer(r#"{"network_grace_ms":5000}"#).apply(&Settings::default()).network_grace_ms, 5000, "Off as stored");
    assert_eq!(layer(r#"{"quality_min_fps":15}"#).apply(&Settings::default()).quality_min_fps, 30);
    let hosts_file = serde_json::json!([{"name": "Desk", "settings": {"experimental_packed_444": true}, "app_settings": {"id:7": {"prefer_yuv444": true, "experimental_packed_444": false}}}]);
    std::fs::write(dir.join("hosts.json"), hosts_file.to_string()).unwrap();
    let hosts = store.hosts();
    let own = hosts[0].settings.as_ref().unwrap();
    assert_eq!(serde_json::to_value(own).unwrap(), serde_json::json!({"experimental_packed_444": true, "prefer_yuv444": true}));
    let game = hosts[0].app_settings("", 7).unwrap();
    assert_eq!(serde_json::to_value(game).unwrap(), serde_json::json!({"experimental_packed_444": true, "prefer_yuv444": true}));
    std::fs::write(dir.join("profiles.json"), r#"{"entries":[{"id":1,"name":"Travel","settings":{"prefer_yuv444":true,"network_grace_ms":40000}}]}"#).unwrap();
    let profile = &store.profiles().entries[0].settings;
    assert_eq!(serde_json::to_value(profile).unwrap(), serde_json::json!({"prefer_yuv444": true, "experimental_packed_444": true, "network_grace_ms": 30000}));
    std::fs::remove_dir_all(dir).ok();
}
