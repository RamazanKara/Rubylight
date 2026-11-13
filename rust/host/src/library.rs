use crate::state::Shared;
use anyhow::{Result, ensure};
use butterpollo_core::{
    library::{self, Selection},
    native::Scan,
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
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
    // Only the files are read in background mode, which must hold no lock: what the scan keeps and
    // Playnite's catalog are locked after it.
    let (discovery, steam, collections) = butterpollo_windows::library::in_background(|| {
        let discovery = butterpollo_windows::library::discover(&settings);
        // The launcher scan leaves Steam's tools out; an app for one is still installed.
        let roots = butterpollo_windows::steam::roots();
        (
            discovery,
            butterpollo_core::steam::discover(&roots),
            butterpollo_core::steam::collections(&roots),
        )
    });
    let collections = LAST_COLLECTIONS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .keeping(collections);
    // Keeping the last ones and then dropping them are two things to tell, for one problem.
    let told = collections
        .problem
        .as_ref()
        .map(|problem| format!("{}: {problem}", collections.problem_line()));
    if newly_told(&TOLD_PROBLEM, told.as_deref())
        && let Some(told) = &told
    {
        tracing::warn!("{told}");
    }
    Scan::from_games(&discovery.games)
        .adding(
            steam
                .iter()
                .filter(|game| game.installed)
                .map(|game| format!("steam:{}", game.appid)),
        )
        // Steam's installs are read above with its library off too, so its apps still say
        // whether their game is installed.
        .skipping(
            discovery
                .skipped()
                .filter(|source| *source != library::Source::Steam),
        )
        .reading(settings)
        // `0` is Steam's never played; without a read history the time is unknown.
        .playing(
            steam
                .iter()
                .filter(|game| game.history)
                .map(|game| (format!("steam:{}", game.appid), game.last_played)),
        )
        .playing(crate::playnite::last_played())
        .collecting(
            collections
                .apps
                .into_iter()
                .map(|(appid, names)| (format!("steam:{appid}"), names)),
        )
        .collecting(crate::playnite::collections())
}
/// Tells once how many collection names `/library` leaves out or shortens for these apps.
pub fn tell_collections_cut(apps: &[butterpollo_core::state::App], scan: &Scan) {
    let cut = scan.collections_cut(apps);
    if newly_told(&TOLD_CUT, (cut > 0).then(|| cut.to_string()).as_deref()) {
        tracing::warn!(
            names = cut,
            per_game = butterpollo_core::native::MAX_COLLECTIONS_PER_GAME,
            name_bytes = butterpollo_core::native::MAX_COLLECTION_NAME_BYTES,
            json_bytes = butterpollo_core::native::MAX_COLLECTIONS_JSON_BYTES,
            "collection names left out or shortened in the library"
        );
    }
}
/// Tests never read the registry and the launchers' folders.
#[cfg(test)]
fn scan_installed(_: library::Settings) -> Scan {
    Scan::default()
}

/// The last collections Steam's file gave, kept through a read that fails.
#[cfg(not(test))]
static LAST_COLLECTIONS: LazyLock<Mutex<butterpollo_core::steam::LastCollections>> =
    LazyLock::new(Default::default);
/// The last problem told about Steam's collections: a scan every few minutes says it once.
#[cfg(not(test))]
static TOLD_PROBLEM: Mutex<Option<String>> = Mutex::new(None);
/// The same for how many collection names the library cuts.
static TOLD_CUT: Mutex<Option<String>> = Mutex::new(None);
/// Whether there is a problem and it is not the one told last. Notes it, and a scan without one
/// clears it, so a problem that comes back is told again.
fn newly_told(told: &Mutex<Option<String>>, problem: Option<&str>) -> bool {
    let mut told = told.lock().unwrap_or_else(PoisonError::into_inner);
    if told.as_deref() == problem {
        return false;
    }
    *told = problem.map(str::to_owned);
    problem.is_some()
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

/// The games deleted from the library that no sync may bring back.
pub fn dismissed(document: &Value) -> Vec<String> {
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

/// Remembers a deleted app's games so no sync brings them back: its Steam, launcher and
/// Playnite game, whoever added the app. `app` is the deleted app as JSON.
pub fn dismiss(document: &mut Value, app: &Value) {
    let mut list = dismissed(document);
    let before = list.len();
    for key in library::dismissal_keys(app) {
        if !list.contains(&key) {
            list.push(key);
        }
    }
    if list.len() != before {
        document[library::DISMISSED] = json!(list);
    }
}

/// `POST /api/library/restore`: takes deleted games off the list, the ones named by their
/// `launcher-dismissed` key or, for `None`, all of them, so the next sync adds them again.
pub fn restore(h: &Shared, keys: Option<&[String]>) -> Result<Value> {
    if keys.is_some_and(<[String]>::is_empty) {
        anyhow::bail!("Select at least one game.");
    }
    let chosen: Option<HashSet<&str>> = keys.map(|keys| keys.iter().map(String::as_str).collect());
    let mut restored = 0;
    crate::steam::update_library(h, |_, document| {
        let before = dismissed(document);
        let kept: Vec<&String> = before
            .iter()
            .filter(|key| {
                chosen
                    .as_ref()
                    .is_some_and(|chosen| !chosen.contains(key.as_str()))
            })
            .collect();
        restored = before.len() - kept.len();
        if restored > 0 {
            document[library::DISMISSED] = json!(kept);
        }
        restored > 0
    })?;
    Ok(json!({ "status": true, "restored": restored }))
}

/// The one-time ownership stamp at host start, before any sync runs (see
/// `library::stamp_synced`). Steam's apps are the sync's only when `sunshine.conf` itself turns
/// `steam_enabled` on: that user chose the opt-in sync of earlier versions. A library that can't
/// be saved stays unstamped, which leaves every app to the user, and is tried again next start.
pub fn stamp_synced(h: &Shared) -> Result<()> {
    let steam_opted_in = {
        let config = h.config.read().unwrap();
        config.values.contains_key("steam_enabled") && config.boolean("steam_enabled", false)
    };
    let mut stamped = None;
    crate::steam::update_library(h, |apps, document| {
        document["apps"] = Value::Array(std::mem::take(apps));
        stamped = library::stamp_synced(document, steam_opted_in);
        if let Value::Array(list) = document["apps"].take() {
            *apps = list;
        }
        stamped.is_some()
    })?;
    if let Some(stamped) = stamped {
        tracing::info!(
            steam = stamped.steam,
            launcher = stamped.launcher,
            playnite = stamped.playnite,
            steam_opted_in,
            "synced apps stamped with their sync"
        );
    }
    Ok(())
}

/// A launcher's PNG cover in the profile's covers folder, converted once: the launcher's own
/// file, or the box art on its image server (`cover_url`, Epic's), downloaded once as Steam's
/// store portrait is.
fn launcher_cover(h: &Shared, game: &mut library::Game) {
    let folder = h.directory.join("covers");
    let destination = folder.join(format!("launcher_{}.png", game.uuid()));
    let source = match game.cover.take() {
        Some(source) => source,
        None => {
            let Some(url) = game.cover_url.as_deref() else {
                return;
            };
            if destination.is_file() {
                game.cover = Some(destination);
                return;
            }
            match downloaded_cover(&folder, &game.uuid(), url) {
                Some(file) => file,
                None => return,
            }
        }
    };
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

/// The launcher's box art at `url`, downloaded into `folder` for the game `uuid`. A failed
/// download waits as long as a failed Steam store portrait before the next try.
fn downloaded_cover(folder: &Path, uuid: &str, url: &str) -> Option<PathBuf> {
    let file = folder.join(format!("launcher_{uuid}.download"));
    if file.is_file() {
        return Some(file);
    }
    let failed = folder.join(format!("launcher_{uuid}.download.failed"));
    let failures =
        std::fs::read(&failed).map_or(0, |marker| crate::steam::portrait_failures(&marker));
    let waiting = failures > 0
        && std::fs::metadata(&failed)
            .and_then(|marker| marker.modified())
            .is_ok_and(|at| {
                at.elapsed()
                    .is_ok_and(|age| age < crate::steam::portrait_retry_after(failures))
            });
    if waiting {
        return None;
    }
    let saved = crate::steam::download_artwork(url).and_then(|bytes| {
        std::fs::create_dir_all(folder)?;
        butterpollo_core::state::atomic_write(&file, &bytes)
    });
    match saved {
        Ok(()) => {
            let _ = std::fs::remove_file(&failed);
            Some(file)
        }
        Err(error) => {
            tracing::debug!(error = %format!("{error:#}"), url, "Launcher box art unavailable");
            let _ = std::fs::create_dir_all(folder);
            let _ = std::fs::write(&failed, failures.saturating_add(1).to_string());
            None
        }
    }
}

/// `POST /api/library/sync` (Sync now): every installed game of the launchers that are on joins
/// the library as each launcher's options choose, as Steam's Sync now does whether or not it
/// syncs automatically (see `library::reconcile`). While Playnite syncs the library, it is
/// Playnite's job and nothing changes here.
pub fn sync(h: &Shared) -> Result<Value> {
    sync_with(h, false)
}
/// [`sync`], or with `automatic` the automatic sync, which keeps only the launchers that sync
/// automatically (`<launcher>_auto_sync`).
fn sync_with(h: &Shared, automatic: bool) -> Result<Value> {
    sync_discovering(h, automatic, |settings| {
        let discovery = butterpollo_windows::library::in_background(|| {
            butterpollo_windows::library::discover(settings)
        });
        let read = discovery
            .sources
            .iter()
            .filter(|status| !matches!(status.state, "error" | "disabled"))
            .map(|status| status.source)
            .collect();
        (discovery.games, read)
    })
}
/// One sync at a time, Sync now's and the tick's: an older discovery reconciling last would remove
/// an app a newer one just added.
static SYNC: Mutex<()> = Mutex::new(());
fn sync_discovering(
    h: &Shared,
    automatic: bool,
    discover: impl FnOnce(&library::Settings) -> (Vec<library::Game>, Vec<library::Source>),
) -> Result<Value> {
    let _one = SYNC.lock().unwrap_or_else(PoisonError::into_inner);
    let settings = settings(h);
    if crate::playnite::syncs_launchers(h) {
        return Ok(json!({ "status": true, "added": 0, "removed": 0, "playnite": true }));
    }
    let (games, read) = discover(&settings);
    let keeping = if automatic {
        settings.automatic()
    } else {
        settings
    };
    let change = sync_games(h, &keeping, games, &read)?;
    Ok(
        json!({ "status": true, "added": change.added, "removed": change.removed, "playnite": false }),
    )
}

/// Each game's cover made ready where the sync may use it: for a game no app has yet, and for
/// one whose own app (by `launcher-source` and `launcher-id`) has no cover. Any other game's
/// cover is dropped, so only a converted cover reaches the library.
fn prepare_covers(h: &Shared, games: &mut [library::Game]) -> Result<()> {
    let apps: Vec<Value> = h
        .apps
        .read()
        .unwrap()
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<_, _>>()?;
    let index = library::AppIndex::new(&apps);
    let own: HashMap<(&str, &str), bool> = apps
        .iter()
        .filter_map(|app| {
            let covered = app["image-path"]
                .as_str()
                .is_some_and(|path| !path.is_empty());
            Some((
                (
                    app["launcher-source"].as_str()?,
                    app["launcher-id"].as_str()?,
                ),
                covered,
            ))
        })
        .collect();
    for game in games {
        let wants_cover = match own.get(&(game.source.key(), game.id.as_str())) {
            Some(covered) => !covered,
            None => !index.matches(&apps, game),
        };
        if wants_cover {
            launcher_cover(h, game);
        } else {
            game.cover = None;
        }
    }
    Ok(())
}

fn sync_games(
    h: &Shared,
    settings: &library::Settings,
    mut games: Vec<library::Game>,
    read: &[library::Source],
) -> Result<library::SyncChange> {
    // Only what the sync keeps needs a cover.
    games.retain(|game| settings.options(game.source).is_some_and(|o| o.enabled));
    prepare_covers(h, &mut games)?;
    let mut change = library::SyncChange::default();
    crate::steam::update_library(h, |apps, document| {
        change = library::reconcile(apps, &games, settings, read, &dismissed(document));
        change.changed()
    })?;
    if change.changed() {
        tracing::info!(
            added = change.added,
            removed = change.removed,
            covered = change.covered,
            "launcher library synced"
        );
        invalidate(h);
    }
    Ok(change)
}

/// The last automatic launcher sync by profile directory, with the settings it ran with.
static SYNCED: LazyLock<Mutex<HashMap<PathBuf, (Instant, String)>>> =
    LazyLock::new(Default::default);

/// What Steam's library looked like at the last housekeeping tick, by profile directory.
static STEAM_STAMPS: LazyLock<Mutex<HashMap<PathBuf, u64>>> = LazyLock::new(Default::default);

/// Notes `stamp` and says whether it moved since the last one. The first one has nothing to
/// compare with: the host's first scan and sync read Steam anyway.
fn steam_moved(directory: &Path, stamp: u64) -> bool {
    let mut stamps = STEAM_STAMPS.lock().unwrap_or_else(PoisonError::into_inner);
    stamps
        .insert(directory.to_owned(), stamp)
        .is_some_and(|last| last != stamp)
}
#[cfg(not(test))]
fn steam_stamp(roots: &crate::steam::Roots) -> u64 {
    butterpollo_windows::library::in_background(|| butterpollo_core::steam::library_stamp(roots))
}
/// Tests never read the registry and the launchers' folders.
#[cfg(test)]
fn steam_stamp(_: &crate::steam::Roots) -> u64 {
    0
}

/// The automatic launcher sync, from the 30-second housekeeping tick: at once when the launcher
/// settings change or Steam's library does, else every five minutes like the cached scan. A game
/// installed or removed in Steam, or a collection edited, reaches the next library request after
/// one tick, whether or not the sync is on.
pub fn watch(h: &Shared, steam: &crate::steam::Roots) {
    watch_stamp(h, steam_stamp(steam));
}
fn watch_stamp(h: &Shared, stamp: u64) {
    let moved = steam_moved(&h.directory, stamp);
    if moved {
        invalidate(h);
    }
    let settings = settings(h);
    if !settings.any_automatic() {
        return;
    }
    let key = format!("{settings:?}");
    {
        let mut synced = SYNCED.lock().unwrap_or_else(PoisonError::into_inner);
        let due = moved
            || synced
                .get(&h.directory)
                .is_none_or(|(at, last)| *last != key || at.elapsed() >= SCAN_EVERY);
        if !due {
            return;
        }
        synced.insert(h.directory.clone(), (Instant::now(), key));
    }
    if let Err(error) = sync_with(h, true) {
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

pub(crate) fn import_games(
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
mod tests;
