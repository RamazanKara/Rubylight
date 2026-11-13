use super::*;
use crate::state::test_support::Fixture;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[test]
fn steam_moved_says_so_only_when_the_stamp_changes() {
    let directory = Path::new("steam-moved-test");
    assert!(!steam_moved(directory, 1), "nothing to compare with yet");
    assert!(!steam_moved(directory, 1));
    assert!(steam_moved(directory, 2), "an install or removal");
    assert!(!steam_moved(directory, 2), "told once");
    assert!(
        !steam_moved(Path::new("steam-moved-other"), 3),
        "each host keeps its own"
    );
}

#[test]
fn a_collections_problem_is_told_once_until_another_comes_or_a_scan_has_none() {
    let told = Mutex::new(None);
    assert!(newly_told(&told, Some("one")));
    assert!(!newly_told(&told, Some("one")));
    assert!(newly_told(&told, Some("two")));
    assert!(!newly_told(&told, None), "nothing to tell");
    assert!(newly_told(&told, Some("two")), "it came back");
}

#[test]
fn a_sync_waits_for_the_one_in_flight_before_it_discovers() {
    let f = Fixture::new();
    let discovered = std::sync::Arc::new(AtomicBool::new(false));
    let waiting = SYNC.lock().unwrap_or_else(PoisonError::into_inner);
    let second = {
        let (h, discovered) = (f.host.clone(), discovered.clone());
        std::thread::spawn(move || {
            sync_discovering(&h, false, |_| {
                discovered.store(true, Ordering::SeqCst);
                (Vec::new(), Vec::new())
            })
            .map(|_| ())
            .map_err(|error| error.to_string())
        })
    };
    std::thread::sleep(Duration::from_millis(100));
    assert!(!discovered.load(Ordering::SeqCst), "the first still runs");
    drop(waiting);
    second.join().unwrap().unwrap();
    assert!(discovered.load(Ordering::SeqCst));
}

#[test]
fn a_moved_steam_library_rescans_on_the_next_tick_with_the_sync_off() {
    let f = Fixture::new();
    // Off for every launcher (the earlier shared switch, read as each launcher's): the tick
    // never reaches the sync, which would read the launchers' folders.
    let off = butterpollo_core::config::Config::parse("library_auto_sync = false\n").unwrap();
    f.host.config.write().unwrap().values.extend(off.values);
    assert!(!settings(&f.host).any_automatic());
    seed(&f.host, Scan::new(["steam:570".to_owned()]));
    let found = |h: &Shared| installed(h).is_some_and(|scan| scan.contains("steam:570"));
    watch_stamp(&f.host, 1);
    watch_stamp(&f.host, 1);
    assert!(found(&f.host), "unchanged: the cached scan stands");
    watch_stamp(&f.host, 2);
    // Tests' scans find nothing: the seeded game goes when the new one is in.
    wait_for(|| !found(&f.host));
}

static OPEN: AtomicBool = AtomicBool::new(false);
static RUNS: AtomicUsize = AtomicUsize::new(0);
fn gated_scan() -> Scan {
    while !OPEN.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(5));
    }
    RUNS.fetch_add(1, Ordering::AcqRel);
    Scan::new(["steam:570".to_owned()])
}
fn wait_for(condition: impl Fn() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(started.elapsed() < Duration::from_secs(10), "timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_scan_runs_in_the_background_once_and_is_cached_until_stale() {
    let directory = std::env::temp_dir().join(format!("scan-{}", uuid::Uuid::new_v4()));
    // The scan is blocked, yet the requests return at once, and a second
    // request does not start a second scan.
    assert_eq!(refresh(&directory, gated_scan), None);
    assert_eq!(refresh(&directory, gated_scan), None);
    OPEN.store(true, Ordering::Release);
    wait_for(|| refresh(&directory, gated_scan).is_some());
    let scan = refresh(&directory, gated_scan).unwrap();
    assert!(scan.contains("steam:570") && !scan.contains("steam:10"));
    assert_eq!(RUNS.load(Ordering::Acquire), 1);
    // Fresh: no new scan. Stale (an import): the old answer now, a new scan behind it.
    assert!(refresh(&directory, gated_scan).is_some());
    assert_eq!(RUNS.load(Ordering::Acquire), 1);
    lock_caches().get_mut(&directory).unwrap().stale = true;
    assert!(refresh(&directory, gated_scan).is_some());
    wait_for(|| RUNS.load(Ordering::Acquire) == 2);
    wait_for(|| !lock_caches()[&directory].running);
    assert!(!lock_caches()[&directory].stale);
}

fn failing_scan() -> Scan {
    panic!("a launcher folder could not be read");
}

#[test]
fn a_failed_scan_is_retried_in_thirty_seconds_and_keeps_the_last_good_result() {
    let directory = std::env::temp_dir().join(format!("scan-{}", uuid::Uuid::new_v4()));
    // Nothing yet: the failure leaves no result and no five-minute hold.
    assert_eq!(refresh(&directory, failing_scan), None);
    wait_for(|| !lock_caches()[&directory].running);
    assert_eq!(refresh(&directory, failing_scan), None);
    let held = lock_caches()[&directory].at;
    let wait = SCAN_EVERY - held.map_or(SCAN_EVERY, |at| at.elapsed());
    assert!(wait <= RETRY_AFTER, "retry in {wait:?}");
    // Due again after the retry delay, not after five minutes.
    lock_caches().get_mut(&directory).unwrap().at =
        Instant::now().checked_sub(SCAN_EVERY - RETRY_AFTER + Duration::from_secs(1));
    assert_eq!(refresh(&directory, failing_scan), None);
    wait_for(|| !lock_caches()[&directory].running);
    // A good result stays through a later failure.
    let mut caches = lock_caches();
    let cache = caches.get_mut(&directory).unwrap();
    cache.scan = Some(Scan::new(["steam:570".to_owned()]));
    cache.stale = true;
    drop(caches);
    assert!(refresh(&directory, failing_scan).is_some());
    wait_for(|| !lock_caches()[&directory].running);
    assert!(refresh(&directory, failing_scan).is_some());
}

#[test]
fn the_launcher_sync_adds_games_keeps_deleted_ones_out_and_an_import_brings_one_back() {
    let f = Fixture::new();
    let settings = library::Settings::default();
    let game = library::ubisoft("13504", Path::new("F:/Games/Mirage"), "Mirage").unwrap();
    let all = library::Source::ALL;
    let change = sync_games(&f.host, &settings, vec![game.clone()], &all).unwrap();
    assert_eq!(change.added, 1);
    let synced = |h: &Shared| {
        h.apps
            .read()
            .unwrap()
            .iter()
            .map(|app| serde_json::to_value(app).unwrap())
            .find(|app| game.matches(app))
    };
    let app = synced(&f.host).unwrap();
    assert_eq!(app["launcher-managed"], "auto");
    // Deleted from the library (as the console's delete does): the sync leaves it out.
    crate::steam::update_library(&f.host, |apps, document| {
        apps.retain(|app| !game.matches(app));
        dismiss(document, &app);
        true
    })
    .unwrap();
    assert_eq!(
        f.host.app_document.read().unwrap()["launcher-dismissed"],
        json!(["ubisoft:13504"])
    );
    let again = sync_games(&f.host, &settings, vec![game.clone()], &all).unwrap();
    assert!(!again.changed() && synced(&f.host).is_none());
    // Persisted with the apps.
    let saved: Value = serde_json::from_slice(&std::fs::read(&f.host.apps_path).unwrap()).unwrap();
    assert_eq!(saved["launcher-dismissed"], json!(["ubisoft:13504"]));
    // Added again by hand: back, and off the list.
    let selection = Selection {
        source: game.source,
        id: game.id.clone(),
    };
    import_games(&f.host, vec![game.clone()], &[selection]).unwrap();
    assert!(synced(&f.host).is_some());
    assert_eq!(
        f.host.app_document.read().unwrap()["launcher-dismissed"],
        json!([])
    );
    // Ubisoft not synced automatically: the automatic sync leaves its new game out, Sync now
    // adds it, as Steam's does.
    let by_hand = library::Settings::from_config(
        &butterpollo_core::config::Config::parse("ubisoft_auto_sync = false\n").unwrap(),
    );
    let other = library::ubisoft("1", Path::new("F:/Games/Other"), "Other").unwrap();
    assert!(
        !sync_games(&f.host, &by_hand.automatic(), vec![other.clone()], &all)
            .unwrap()
            .changed()
    );
    assert_eq!(
        sync_games(&f.host, &by_hand, vec![other], &all)
            .unwrap()
            .added,
        1
    );
}

#[test]
fn a_launchers_cover_reaches_a_new_app_and_an_app_without_one_but_never_replaces_one() {
    let f = Fixture::new();
    let all = library::Source::ALL;
    let settings = library::Settings::default();
    // Synced before its cover was read, and one of the user's with their own cover.
    let bare = library::ubisoft("13504", Path::new("F:/Games/Mirage"), "Mirage").unwrap();
    sync_games(&f.host, &settings, vec![bare.clone()], &all).unwrap();
    crate::steam::update_apps(&f.host, |apps| {
        apps.push(
            json!({"name": "Mine", "uuid": "mine", "launcher-source": "ubisoft",
            "launcher-id": "2", "cmd": "uplay://launch/2", "image-path": "D:/mine.png"}),
        );
        true
    })
    .unwrap();
    let mut epic = bare.clone();
    epic.source = library::Source::Epic;
    epic.id = "ns:item:App".into();
    epic.name = "Fresh".into();
    // The converted covers are there already: no conversion and no download.
    let covers = f.host.directory.join("covers");
    std::fs::create_dir_all(&covers).unwrap();
    let mut games = vec![];
    for (mut game, id) in [
        (bare, "13504"),
        (epic, "ns:item:App"),
        (bare_with("2"), "2"),
    ] {
        game.id = id.into();
        let converted = covers.join(format!("launcher_{}.png", game.uuid()));
        std::fs::write(&converted, b"png").unwrap();
        if game.source == library::Source::Epic {
            game.cover_url = Some("https://cdn1.epicgames.com/tall.jpg".into());
        } else {
            game.cover = Some(PathBuf::from(r"F:\Games\cover.jpg"));
        }
        games.push(game);
    }
    let change = sync_games(&f.host, &settings, games.clone(), &all).unwrap();
    assert_eq!((change.added, change.covered), (1, 1));
    let apps: Vec<Value> = f
        .host
        .apps
        .read()
        .unwrap()
        .iter()
        .map(|app| serde_json::to_value(app).unwrap())
        .collect();
    let image = |name: &str| {
        apps.iter()
            .find(|app| app["name"] == name)
            .map(|app| app["image-path"].clone())
            .unwrap()
    };
    let png = |game: &library::Game| {
        json!(
            covers
                .join(format!("launcher_{}.png", game.uuid()))
                .to_string_lossy()
                .replace('\\', "/")
        )
    };
    assert_eq!(image("Mirage"), png(&games[0]));
    assert_eq!(image("Fresh"), png(&games[1]));
    assert_eq!(image("Mine"), "D:/mine.png");
}
fn bare_with(id: &str) -> library::Game {
    library::ubisoft(id, Path::new("F:/Games/Mine"), "Mine").unwrap()
}

#[test]
fn a_restore_brings_back_the_chosen_removed_games_or_all_of_them_on_the_next_sync() {
    let f = Fixture::new();
    let settings = library::Settings::default();
    let all = library::Source::ALL;
    let games = vec![
        library::ubisoft("13504", Path::new("F:/Games/Mirage"), "Mirage").unwrap(),
        library::ubisoft("1", Path::new("F:/Games/Other"), "Other").unwrap(),
    ];
    sync_games(&f.host, &settings, games.clone(), &all).unwrap();
    let synced = |h: &Shared| {
        let apps: Vec<Value> = h
            .apps
            .read()
            .unwrap()
            .iter()
            .map(|app| serde_json::to_value(app).unwrap())
            .collect();
        games
            .iter()
            .map(|game| apps.iter().any(|app| game.matches(app)))
            .collect::<Vec<_>>()
    };
    assert_eq!(synced(&f.host), [true, true]);
    crate::steam::update_library(&f.host, |apps, document| {
        for app in std::mem::take(apps) {
            dismiss(document, &app);
        }
        true
    })
    .unwrap();
    assert_eq!(
        f.host.app_document.read().unwrap()["launcher-dismissed"],
        json!(["ubisoft:13504", "ubisoft:1"])
    );
    // Nothing chosen is refused; a key not on the list restores nothing.
    assert!(restore(&f.host, Some(&[])).is_err());
    let none = restore(&f.host, Some(&["steam:570".to_owned()])).unwrap();
    assert_eq!(none["restored"], 0);
    // One chosen game: off the list, and the sync adds it again; the other stays out.
    let one = restore(&f.host, Some(&["ubisoft:13504".to_owned()])).unwrap();
    assert_eq!(one["restored"], 1);
    sync_games(&f.host, &settings, games.clone(), &all).unwrap();
    assert_eq!(synced(&f.host), [true, false]);
    let saved: Value = serde_json::from_slice(&std::fs::read(&f.host.apps_path).unwrap()).unwrap();
    assert_eq!(saved["launcher-dismissed"], json!(["ubisoft:1"]));
    // All of them.
    assert_eq!(restore(&f.host, None).unwrap()["restored"], 1);
    sync_games(&f.host, &settings, games.clone(), &all).unwrap();
    assert_eq!(synced(&f.host), [true, true]);
    assert_eq!(
        f.host.app_document.read().unwrap()["launcher-dismissed"],
        json!([])
    );
}

#[test]
fn the_start_stamps_synced_apps_once_and_steam_apps_only_after_the_old_opt_in() {
    for (conf, steam) in [
        ("", Value::Null),
        ("steam_enabled = false\n", Value::Null),
        ("steam_enabled = true\n", json!("steam")),
    ] {
        let directory = Fixture::profile();
        std::fs::write(directory.join("sunshine.conf"), conf).unwrap();
        let apps = json!({"apps": [
            {"name": "Portal", "uuid": "p", "steam-id": "620", "steam-managed": "auto"},
            {"name": "Mirage", "uuid": "m", "launcher-source": "ubisoft",
             "launcher-id": "13504", "launcher-managed": "auto"},
            {"name": "Kart", "uuid": "k", "playnite-id": "aa", "playnite-managed": "auto"},
            {"name": "Mine", "uuid": "x", "cmd": "C:/mine.exe"},
        ]});
        std::fs::write(directory.join("apps.json"), apps.to_string()).unwrap();
        let f = Fixture::load(directory.clone());
        stamp_synced(&f.host).unwrap();
        let saved = butterpollo_core::state::load_json(&f.host.apps_path, json!({})).unwrap();
        assert_eq!(saved[library::SYNC_OWNER_VERSION], 1, "{conf}");
        let stamps: Vec<Value> = saved["apps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|app| app[library::SYNCED_BY].clone())
            .collect();
        assert_eq!(
            stamps,
            [
                steam.clone(),
                json!("launcher"),
                json!("playnite"),
                Value::Null
            ],
            "{conf:?}"
        );
        // Once: an app handed over since keeps no stamp at the next start.
        let mut later = saved.clone();
        later["apps"][1]
            .as_object_mut()
            .unwrap()
            .remove(library::SYNCED_BY);
        let directory = Fixture::profile();
        std::fs::write(directory.join("sunshine.conf"), conf).unwrap();
        std::fs::write(directory.join("apps.json"), later.to_string()).unwrap();
        let f = Fixture::load(directory);
        stamp_synced(&f.host).unwrap();
        assert_eq!(
            butterpollo_core::state::load_json(&f.host.apps_path, json!({})).unwrap(),
            later
        );
    }
}

#[test]
fn imports_persist_without_replacing_document_metadata_or_existing_customizations() {
    let f = Fixture::new();
    f.host.app_document.write().unwrap()["custom-root"] = json!({ "keep": true });
    let game = library::ubisoft("13504", Path::new("F:/Games/Mirage"), "Mirage").unwrap();
    let selection = Selection {
        source: game.source,
        id: game.id.clone(),
    };
    assert_eq!(
        import_games(
            &f.host,
            vec![game.clone()],
            std::slice::from_ref(&selection)
        )
        .unwrap()["results"][0]["result"],
        "added"
    );
    crate::steam::update_apps(&f.host, |apps| {
        let app = apps.iter_mut().find(|app| game.matches(app)).unwrap();
        app["name"] = json!("My Mirage");
        app["custom"] = json!([1, 2, 3]);
        app["cmd"] = json!("my command");
        true
    })
    .unwrap();
    let before = std::fs::read(&f.host.apps_path).unwrap();
    assert_eq!(
        import_games(&f.host, vec![game], std::slice::from_ref(&selection)).unwrap()["results"][0]
            ["result"],
        "already_present"
    );
    assert_eq!(
        import_games(&f.host, vec![], &[selection]).unwrap()["results"][0]["result"],
        "failed"
    );
    assert_eq!(std::fs::read(&f.host.apps_path).unwrap(), before);
    let saved: Value = serde_json::from_slice(&before).unwrap();
    assert_eq!(saved["custom-root"], json!({ "keep": true }));
    assert!(
        saved["apps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|app| app["name"] == "My Mirage" && app["custom"] == json!([1, 2, 3]))
    );
}
