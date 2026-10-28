use crate::state::Shared;
use anyhow::{Result, ensure};
use butterpollo_core::{
    library::{self, Selection},
    native::Scan,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

/// How old the cached launcher scan may get before a request starts a new one.
const SCAN_EVERY: Duration = Duration::from_secs(5 * 60);

/// How long a failed scan waits before the next try, instead of a whole
/// `SCAN_EVERY`. The last good result stays in the meantime.
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// The time a scan "ran" such that the next one is due in `RETRY_AFTER`. Unset
/// (due at once) when the clock is too young to go back that far.
fn retry_soon() -> Option<Instant> {
    Instant::now().checked_sub(SCAN_EVERY - RETRY_AFTER)
}

/// One host's cached launcher scan, for the native API's `installed`.
#[derive(Default)]
struct Cache {
    scan: Option<Scan>,
    at: Option<Instant>,
    running: bool,
    /// Something changed (an import): scan again at the next request.
    stale: bool,
}
/// By profile directory, so each host (and each test host) has its own.
static CACHES: LazyLock<Mutex<HashMap<PathBuf, Cache>>> = LazyLock::new(Default::default);
fn lock_caches() -> MutexGuard<'static, HashMap<PathBuf, Cache>> {
    CACHES.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Which launchers the scan reads, from the host's settings.
fn settings(h: &Shared) -> library::Settings {
    library::Settings::from_config(&h.config.read().unwrap())
}
#[cfg(not(test))]
fn scan_installed(settings: library::Settings) -> Scan {
    let discovery = butterpollo_windows::library::discover(&settings);
    Scan::from_games(&discovery.games)
        .skipping(discovery.skipped())
        .reading(settings)
}
/// Tests never read the registry and the launchers' folders.
#[cfg(test)]
fn scan_installed(_: library::Settings) -> Scan {
    Scan::default()
}

/// The games the last launcher scan found installed, `None` before the first
/// one has finished. A scan runs on its own thread, at most every five
/// minutes and after an import, so a request never waits for it.
pub fn installed(h: &Shared) -> Option<Scan> {
    let settings = settings(h);
    refresh(&h.directory, move || scan_installed(settings))
}
/// Tests set what the last scan found.
#[cfg(test)]
pub fn seed(h: &Shared, scan: Scan) {
    let mut caches = lock_caches();
    let cache = caches.entry(h.directory.clone()).or_default();
    cache.scan = Some(scan);
    cache.at = Some(Instant::now());
}
/// The apps changed: scan again soon.
pub fn invalidate(h: &Shared) {
    if let Some(cache) = lock_caches().get_mut(&h.directory) {
        cache.stale = true;
    }
    installed(h);
}
fn refresh(
    directory: &Path,
    scanner: impl FnOnce() -> Scan + Send + std::panic::UnwindSafe + 'static,
) -> Option<Scan> {
    let mut caches = lock_caches();
    let cache = caches.entry(directory.to_owned()).or_default();
    let due = cache.stale || cache.at.is_none_or(|at| at.elapsed() >= SCAN_EVERY);
    if due && !cache.running {
        cache.running = true;
        cache.stale = false;
        let directory = directory.to_owned();
        let spawned = std::thread::Builder::new()
            .name("library-scan".into())
            .spawn(move || {
                let scan = std::panic::catch_unwind(scanner);
                let mut caches = lock_caches();
                let cache = caches.entry(directory).or_default();
                match scan {
                    Ok(scan) => {
                        cache.scan = Some(scan);
                        cache.at = Some(Instant::now());
                    }
                    Err(_) => {
                        tracing::error!("the launcher scan panicked");
                        cache.at = retry_soon();
                    }
                }
                cache.running = false;
            });
        if let Err(error) = spawned {
            tracing::error!(%error, "the launcher scan could not start");
            cache.running = false;
            cache.at = retry_soon();
        }
    }
    cache.scan.clone()
}

pub fn discovery(h: &Shared) -> Result<Value> {
    let mut catalog = butterpollo_windows::library::discover(&settings(h));
    let apps: Vec<Value> = h
        .apps
        .read()
        .unwrap()
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<_, _>>()?;
    let mut games = vec![];
    for game in &mut catalog.games {
        let existing = apps.iter().find(|app| game.matches(app));
        game.already_added = existing.is_some();
        let mut value = serde_json::to_value(&*game)?;
        value["uuid"] = existing
            .and_then(|app| app.get("uuid"))
            .cloned()
            .unwrap_or_else(|| json!(game.uuid()));
        value["has_cover"] = json!(
            existing.is_some_and(|app| app["image-path"].as_str().is_some_and(|s| !s.is_empty()))
        );
        games.push(value);
    }
    Ok(json!({ "status": true, "sources": catalog.sources, "games": games }))
}

/// The games deleted from the library that the launcher sync must not bring back.
fn dismissed(document: &Value) -> Vec<String> {
    document[library::DISMISSED]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Remembers a deleted synced app's game so the launcher sync leaves it out. `app` is the
/// deleted app as JSON; other apps change nothing.
pub fn dismiss(document: &mut Value, app: &Value) {
    let Some(key) = library::managed_key(app) else {
        return;
    };
    let mut list = dismissed(document);
    if !list.contains(&key) {
        list.push(key);
        document[library::DISMISSED] = json!(list);
    }
}

/// A launcher's PNG cover in the profile's covers folder, converted once.
fn launcher_cover(h: &Shared, game: &mut library::Game) {
    let Some(source) = game.cover.take() else {
        return;
    };
    let destination = h
        .directory
        .join("covers")
        .join(format!("launcher_{}.png", game.uuid()));
    if destination.is_file() {
        game.cover = Some(destination);
        return;
    }
    match butterpollo_windows::image::to_png(&source, &destination) {
        Ok(_) => game.cover = Some(destination),
        Err(error) => {
            tracing::debug!(%error, source = %source.display(), "Launcher cover unavailable")
        }
    }
}

/// `POST /api/library/sync` and the automatic sync: every installed game of the launchers that
/// are on joins the library, as Steam's sync does (see `library::reconcile`). While Playnite
/// syncs the library, it is Playnite's job and nothing changes here.
pub fn sync(h: &Shared) -> Result<Value> {
    let settings = settings(h);
    if crate::playnite::syncs_launchers(h) {
        return Ok(json!({ "status": true, "added": 0, "removed": 0, "playnite": true }));
    }
    let discovery = butterpollo_windows::library::discover(&settings);
    let read: Vec<library::Source> = discovery
        .sources
        .iter()
        .filter(|status| !matches!(status.state, "error" | "disabled"))
        .map(|status| status.source)
        .collect();
    let change = sync_games(h, &settings, discovery.games, &read)?;
    Ok(
        json!({ "status": true, "added": change.added, "removed": change.removed, "playnite": false }),
    )
}

fn sync_games(
    h: &Shared,
    settings: &library::Settings,
    mut games: Vec<library::Game>,
    read: &[library::Source],
) -> Result<library::SyncChange> {
    if !settings.auto_sync {
        return Ok(library::SyncChange::default());
    }
    {
        let apps: Vec<Value> = h
            .apps
            .read()
            .unwrap()
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<_, _>>()?;
        for game in &mut games {
            if !apps.iter().any(|app| game.matches(app)) {
                launcher_cover(h, game);
            }
        }
    }
    let mut change = library::SyncChange::default();
    crate::steam::update_library(h, |apps, document| {
        change = library::reconcile(apps, &games, settings, read, &dismissed(document));
        change.changed()
    })?;
    if change.changed() {
        tracing::info!(
            added = change.added,
            removed = change.removed,
            "launcher library synced"
        );
        invalidate(h);
    }
    Ok(change)
}

/// The last automatic launcher sync by profile directory, with the settings it ran with.
static SYNCED: LazyLock<Mutex<HashMap<PathBuf, (Instant, String)>>> =
    LazyLock::new(Default::default);

/// The automatic launcher sync, from the 30-second housekeeping tick: at once when the
/// launcher settings change, else every five minutes like the cached scan.
pub fn watch(h: &Shared) {
    let settings = settings(h);
    if !settings.auto_sync {
        return;
    }
    let key = format!("{settings:?}");
    {
        let mut synced = SYNCED.lock().unwrap_or_else(PoisonError::into_inner);
        let due = synced
            .get(&h.directory)
            .is_none_or(|(at, last)| *last != key || at.elapsed() >= SCAN_EVERY);
        if !due {
            return;
        }
        synced.insert(h.directory.clone(), (Instant::now(), key));
    }
    if let Err(error) = sync(h) {
        tracing::warn!(error = %format!("{error:#}"), "the launcher library sync failed");
    }
}

pub fn import(h: &Shared, selected: &[Selection]) -> Result<Value> {
    ensure!(!selected.is_empty(), "Select at least one installed game.");
    let result = import_games(
        h,
        butterpollo_windows::library::discover(&settings(h)).games,
        selected,
    );
    invalidate(h);
    result
}

fn import_games(
    h: &Shared,
    mut games: Vec<library::Game>,
    selected: &[Selection],
) -> Result<Value> {
    let existing: Vec<Value> = h
        .apps
        .read()
        .unwrap()
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<_, _>>()?;
    for game in &mut games {
        if existing.iter().any(|app| game.matches(app)) {
            continue;
        }
        if !selected
            .iter()
            .any(|entry| entry.source == game.source && entry.id == game.id)
        {
            continue;
        }
        launcher_cover(h, game);
    }
    let mut results = vec![];
    crate::steam::update_library(h, |apps, document| {
        results = library::import(apps, &games, selected);
        // A game imported again was wanted after all: the sync may keep it in step.
        let before = dismissed(document);
        let kept: Vec<&String> = before
            .iter()
            .filter(|key| {
                !results.iter().any(|result| {
                    result.result == "added"
                        && **key == library::dismissed_key(result.source, &result.id)
                })
            })
            .collect();
        let undismissed = kept.len() != before.len();
        if undismissed {
            document[library::DISMISSED] = json!(kept);
        }
        undismissed || results.iter().any(|result| result.result == "added")
    })?;
    Ok(json!({ "status": true, "results": results }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::Fixture;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

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
        let saved: Value =
            serde_json::from_slice(&std::fs::read(&f.host.apps_path).unwrap()).unwrap();
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
        // Off: nothing is synced.
        let off = library::Settings {
            auto_sync: false,
            ..settings
        };
        let other = library::ubisoft("1", Path::new("F:/Games/Other"), "Other").unwrap();
        assert!(
            !sync_games(&f.host, &off, vec![other], &all)
                .unwrap()
                .changed()
        );
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
            import_games(&f.host, vec![game], std::slice::from_ref(&selection)).unwrap()["results"]
                [0]["result"],
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
}
