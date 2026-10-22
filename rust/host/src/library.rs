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

#[cfg(not(test))]
fn scan_installed() -> Scan {
    Scan::from_games(&butterpollo_windows::library::discover().games)
}
/// Tests never read the registry and the launchers' folders.
#[cfg(test)]
fn scan_installed() -> Scan {
    Scan::default()
}

/// The games the last launcher scan found installed, `None` before the first
/// one has finished. A scan runs on its own thread, at most every five
/// minutes and after an import, so a request never waits for it.
pub fn installed(h: &Shared) -> Option<Scan> {
    refresh(&h.directory, scan_installed)
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
fn refresh(directory: &Path, scanner: fn() -> Scan) -> Option<Scan> {
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
                    Ok(scan) => cache.scan = Some(scan),
                    Err(_) => tracing::error!("the launcher scan panicked"),
                }
                cache.at = Some(Instant::now());
                cache.running = false;
            });
        if let Err(error) = spawned {
            tracing::error!(%error, "the launcher scan could not start");
            cache.running = false;
            cache.at = Some(Instant::now());
        }
    }
    cache.scan.clone()
}

pub fn discovery(h: &Shared) -> Result<Value> {
    let mut catalog = butterpollo_windows::library::discover();
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

pub fn import(h: &Shared, selected: &[Selection]) -> Result<Value> {
    ensure!(!selected.is_empty(), "Select at least one installed game.");
    let result = import_games(h, butterpollo_windows::library::discover().games, selected);
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
        if let Some(source) = game.cover.take() {
            let destination = h
                .directory
                .join("covers")
                .join(format!("launcher_{}.png", game.uuid()));
            match butterpollo_windows::image::to_png(&source, &destination) {
                Ok(_) => game.cover = Some(destination),
                Err(error) => {
                    tracing::debug!(%error, source = %source.display(), "Launcher cover unavailable")
                }
            }
        }
    }
    let mut results = vec![];
    crate::steam::update_apps(h, |apps| {
        results = library::import(apps, &games, selected);
        results.iter().any(|result| result.result == "added")
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
