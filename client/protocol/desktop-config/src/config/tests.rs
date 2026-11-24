//! Unit tests for desktop config storage, recents, favourites, libraries and covers.

mod defaults;
mod overrides;

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
    let dir = std::env::temp_dir().join(format!("nightjar-unreadable-value-{}", std::process::id()));
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
    let library: rubylight_native_api::library::Library = serde_json::from_str(include_str!("../../../native-api/tests/fixtures/library.json")).unwrap();
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
    use nightjar_policy::http::{App, AppControl};
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
