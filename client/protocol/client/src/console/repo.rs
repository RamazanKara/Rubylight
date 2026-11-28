//! `ConsoleRepository`: merges cached + live host libraries across PCs via
//! `nightjar_policy::console`, coordinates parallel `std::thread::scope` host refreshes with a
//! 5-minute capability retry cooldown, tracks active host `Session`s, and repairs renumbered
//! classic favourite `app_id`s.

use super::store::{FavouritesStore, HiddenStore, LibraryCacheStore, RecentsStore};
pub use super::transport::HostClientTransport;
use nightjar_policy::console::{
    build_hidden_listings, classic_library, continue_playing, hide_keys_by_id,
    merge_rich_library, reconcile_starred, split_hidden, ClassicApps, Continue, Entry, Favourite,
    HiddenListing, HomeProjection, LibraryFilter, Recent, RichSource, Sort, Source,
};
use nightjar_policy::http::native_api::library::Library;
use nightjar_policy::http::native_api::session::Session;
use nightjar_policy::http::{
    AddedGame, GameExtra, NativeCapabilities, NativeLibrary, FEATURE_LIBRARY_UNADDED,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

/// Minimum wait before retrying a failed `GET /capabilities` probe on an online host.
pub const FEATURES_RETRY_COOLDOWN: Duration = Duration::from_secs(300);
/// Refresh cached capabilities periodically without requiring a process restart.
const CAPS_REFRESH_TTL: Duration = Duration::from_secs(300);

/// Metadata describing one paired/known host supplied by the platform host registry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostDescriptor {
    pub uuid: String,
    pub name: String,
    pub online: bool,
    pub paired: bool,
    pub is_native: bool,
    pub address: String,
    pub http_port: u16,
    pub https_port: u16,
    pub server_cert_pem: Option<String>,
    pub classic_apps: Option<ClassicApps>,
    pub running_app_id: Option<i32>,
    pub running_app_uuid: Option<String>,
}

/// Network transport used by [`ConsoleRepository::refresh`] and [`ConsoleRepository::add_to_library`].
pub trait NativeTransport: Send + Sync {
    fn capabilities(&self, host: &HostDescriptor) -> Result<NativeCapabilities, String>;
    fn library(
        &self,
        host: &HostDescriptor,
        include_unadded: bool,
    ) -> Result<NativeLibrary, String>;
    fn session(&self, host: &HostDescriptor) -> Result<Session, String>;
    fn add_to_library(
        &self,
        host: &HostDescriptor,
        game_id: &str,
    ) -> Result<Option<AddedGame>, String>;
}

/// Per-host status tracked across refreshes for `ConsoleSnapshot::host_states`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostSnapshotState {
    pub uuid: String,
    pub name: String,
    pub status: String,
    pub fetched_at_ms: u64,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
    pub features: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub capabilities_fetched: bool,
}

/// Complete snapshot exposed by [`ConsoleRepository::cached`] and [`ConsoleRepository::refresh`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleSnapshot {
    pub titles: Vec<Entry>,
    pub hidden_titles: Vec<Entry>,
    pub continue_playing: Vec<Continue>,
    pub recents: Vec<Recent>,
    pub starred: Vec<Favourite>,
    pub sessions: BTreeMap<String, Session>,
    pub host_states: Vec<HostSnapshotState>,
    pub library_revision: String,
    pub has_any_library: bool,
}

#[derive(Default)]
struct RepoState {
    capabilities: HashMap<String, NativeCapabilities>,
    capabilities_fetched_at: HashMap<String, Instant>,
    cached_features: HashMap<String, Vec<String>>,
    features_retry_at: HashMap<String, Instant>,
    libraries: HashMap<String, NativeLibrary>,
    fetched_at_ms: HashMap<String, u64>,
    errors: HashMap<String, String>,
    live: HashMap<String, bool>,
    sessions: HashMap<String, Session>,
    clear_gen: u64,
    forget_gen: HashMap<String, u64>,
}

/// Unified console repository coordinating disk stores, parallel host refreshes, and pure merge logic.
pub struct ConsoleRepository {
    base_dir: std::path::PathBuf,
    libraries: LibraryCacheStore,
    recents: RecentsStore,
    favourites: FavouritesStore,
    hidden: HiddenStore,
    state: RwLock<RepoState>,
}

struct OwnedHostSource {
    id: u64,
    uuid: String,
    name: String,
    online: bool,
    paired: bool,
    classic: bool,
    library: Option<Library>,
    extra: Option<Vec<GameExtra>>,
    last_played: BTreeMap<String, u64>,
    playtime_minutes: BTreeMap<String, u64>,
    collections: BTreeMap<String, Vec<String>>,
    session: Option<Session>,
    host_control: bool,
}

impl ConsoleRepository {
    pub fn new(base_dir: impl AsRef<Path>) -> Self {
        let base_dir = base_dir.as_ref().to_path_buf();
        Self {
            libraries: LibraryCacheStore::new(&base_dir),
            recents: RecentsStore::new(&base_dir),
            favourites: FavouritesStore::new(&base_dir),
            hidden: HiddenStore::new(&base_dir),
            base_dir,
            state: RwLock::new(RepoState::default()),
        }
    }

    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    pub fn library_store(&self) -> &LibraryCacheStore {
        &self.libraries
    }

    pub fn recents_store(&self) -> &RecentsStore {
        &self.recents
    }

    pub fn favourites_store(&self) -> &FavouritesStore {
        &self.favourites
    }

    pub fn hidden_store(&self) -> &HiddenStore {
        &self.hidden
    }

    /// Clears transient in-memory host/session state (used in tests or host teardown).
    pub fn clear_in_memory_cache(&self) {
        if let Ok(mut guard) = self.state.write() {
            let next_gen = guard.clear_gen.wrapping_add(1);
            *guard = RepoState {
                clear_gen: next_gen,
                ..RepoState::default()
            };
        }
    }

    pub fn host_control_for(&self, host_id: &str) -> bool {
        self.state
            .read()
            .unwrap()
            .capabilities
            .get(host_id)
            .is_some_and(|c| c.capabilities.permissions.host_control)
    }

    /// Loads the current snapshot from disk + in-memory session state with zero network I/O.
    pub fn cached(&self, hosts: &[HostDescriptor]) -> ConsoleSnapshot {
        self.build_snapshot(hosts, false, &HashSet::new())
    }

    /// Refreshes online native hosts in parallel via `std::thread::scope`, persists updated
    /// libraries to `LibraryCacheStore`, updates active `sessions`, and returns the merged snapshot.
    pub fn refresh(
        &self,
        hosts: &[HostDescriptor],
        transport: &dyn NativeTransport,
    ) -> ConsoleSnapshot {
        let now_unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        self.refresh_at(hosts, transport, now_unix_ms)
    }

    /// Refreshes online native hosts using caller-provided `now_unix_ms` for persisted timestamps.
    pub fn refresh_at(
        &self,
        hosts: &[HostDescriptor],
        transport: &dyn NativeTransport,
        now_unix_ms: u64,
    ) -> ConsoleSnapshot {
        let now = Instant::now();
        type RefreshResult = (
            String,
            Option<NativeCapabilities>,
            bool,
            bool,
            Vec<String>,
            Result<NativeLibrary, String>,
            Option<Session>,
        );
        let results: Mutex<Vec<RefreshResult>> = Mutex::new(Vec::new());

        let (start_clear_gen, start_forget_gens, snapshot_state) = {
            let guard = self.state.read().unwrap();
            let forget_gens: HashMap<String, u64> = hosts
                .iter()
                .map(|h| (h.uuid.clone(), guard.forget_gen.get(&h.uuid).copied().unwrap_or(0)))
                .collect();
            let state_map: HashMap<String, _> = hosts
                .iter()
                .map(|h| {
                    let fresh_caps = guard
                        .capabilities_fetched_at
                        .get(&h.uuid)
                        .is_some_and(|&at| now.saturating_duration_since(at) < CAPS_REFRESH_TTL)
                        .then(|| guard.capabilities.get(&h.uuid).cloned())
                        .flatten();
                    let stale_caps = guard.capabilities.get(&h.uuid);
                    let disk_entry =
                        if stale_caps.is_none() && !guard.cached_features.contains_key(&h.uuid) {
                            self.libraries.load_entry(&h.uuid)
                        } else {
                            None
                        };
                    let fallback = stale_caps
                        .map(|c| c.capabilities.features.clone())
                        .or_else(|| guard.cached_features.get(&h.uuid).cloned())
                        .or_else(|| disk_entry.map(|(_, _, f)| f))
                        .unwrap_or_default();
                    let cooldown_ok = guard
                        .features_retry_at
                        .get(&h.uuid)
                        .is_none_or(|&until| now >= until);
                    (h.uuid.clone(), (fresh_caps, fallback, cooldown_ok))
                })
                .collect();
            (guard.clear_gen, forget_gens, state_map)
        };

        std::thread::scope(|scope| {
            for host in hosts {
                if !host.paired || !host.online || !host.is_native || host.uuid.is_empty() {
                    continue;
                }
                let (cached_caps, fallback_features, can_probe_caps) = snapshot_state
                    .get(&host.uuid)
                    .cloned()
                    .unwrap_or((None, Vec::new(), true));
                let results = &results;
                scope.spawn(move || {
                    let mut caps = cached_caps;
                    let mut caps_failed = false;
                    let mut caps_newly_fetched = false;
                    if caps.is_none() && can_probe_caps {
                        match transport.capabilities(host) {
                            Ok(fetched) => {
                                caps = Some(fetched);
                                caps_newly_fetched = true;
                            }
                            Err(_) => caps_failed = true,
                        }
                    }
                    let effective_features = caps
                        .as_ref()
                        .map(|c| c.capabilities.features.clone())
                        .unwrap_or(fallback_features);
                    let include_unadded = effective_features
                        .iter()
                        .any(|f| f == FEATURE_LIBRARY_UNADDED);
                    let lib = transport.library(host, include_unadded);
                    let sess = transport.session(host).ok();
                    results.lock().unwrap().push((
                        host.uuid.clone(),
                        caps,
                        caps_failed,
                        caps_newly_fetched,
                        effective_features,
                        lib,
                        sess,
                    ));
                });
            }
        });

        let collected = results.into_inner().unwrap();
        let mut newly_fetched_caps = HashSet::new();
        {
            let mut guard = self.state.write().unwrap();
            if guard.clear_gen == start_clear_gen {
                for (
                    uuid,
                    caps,
                    caps_failed,
                    caps_newly_fetched,
                    effective_features,
                    lib_res,
                    sess,
                ) in collected
                {
                    let started_gen = start_forget_gens.get(&uuid).copied().unwrap_or(0);
                    if guard.forget_gen.get(&uuid).copied().unwrap_or(0) != started_gen {
                        continue;
                    }
                    if caps_newly_fetched {
                        newly_fetched_caps.insert(uuid.clone());
                    }
                    if let Some(c) = caps {
                        guard
                            .cached_features
                            .insert(uuid.clone(), c.capabilities.features.clone());
                        guard.capabilities.insert(uuid.clone(), c);
                        if caps_newly_fetched {
                            guard.capabilities_fetched_at.insert(uuid.clone(), Instant::now());
                        }
                        guard.features_retry_at.remove(&uuid);
                    } else {
                        if !effective_features.is_empty() {
                            guard
                                .cached_features
                                .insert(uuid.clone(), effective_features.clone());
                        }
                        if caps_failed {
                            guard
                                .features_retry_at
                                .insert(uuid.clone(), Instant::now() + FEATURES_RETRY_COOLDOWN);
                        }
                    }
                    match lib_res {
                        Ok(l) => {
                            let _ = self.libraries.save_entry(
                                &uuid,
                                &l,
                                now_unix_ms,
                                &effective_features,
                            );
                            guard.libraries.insert(uuid.clone(), l);
                            guard.fetched_at_ms.insert(uuid.clone(), now_unix_ms);
                            guard.errors.remove(&uuid);
                            guard.live.insert(uuid.clone(), true);
                        }
                        Err(err) => {
                            guard.errors.insert(uuid.clone(), err);
                            guard.live.insert(uuid.clone(), false);
                        }
                    }
                    if let Some(s) = sess {
                        guard.sessions.insert(uuid, s);
                    } else {
                        guard.sessions.remove(&uuid);
                    }
                }
            }
        }

        self.build_snapshot(hosts, true, &newly_fetched_caps)
    }

    /// Adds an unadded game to `host`'s library (`POST /api/native/v1/library/add`).
    /// Does not mutate the cached library snapshot; callers refresh the host when ready.
    pub fn add_to_library(
        &self,
        host: &HostDescriptor,
        game_id: &str,
        transport: &dyn NativeTransport,
    ) -> Result<Option<AddedGame>, String> {
        transport.add_to_library(host, game_id)
    }

    pub fn set_favourite(&self, entry: Favourite) -> Result<bool, String> {
        self.favourites.toggle(entry)
    }

    pub fn set_favourite_explicit(&self, entry: Favourite, starred: bool) -> Result<(), String> {
        self.favourites.set_explicit(entry, starred)
    }

    pub fn is_favourite(&self, host_id: &str, install_id: &str, app_id: u32) -> bool {
        self.favourites.is_favourite(host_id, install_id, app_id)
    }

    pub fn set_hidden(&self, keys: Vec<Favourite>, hidden: bool) -> Result<bool, String> {
        self.hidden.set_hidden(keys, hidden)
    }

    pub fn hide_game(&self, hosts: &[HostDescriptor], host_id: &str, app_id: u32, hidden: bool) -> Result<bool, String> {
        let owned = self.prepare_sources(hosts);
        let sources: Vec<Source<'_>> = owned.iter().map(OwnedHostSource::as_source).collect();
        let keys = hide_keys_by_id(&sources, host_id, app_id);
        self.hidden.set_hidden(keys, hidden)
    }

    pub fn record_launch(&self, entry: Recent) -> Result<(), String> {
        self.recents.record(entry)
    }

    pub fn forget_host(&self, host_id: &str) {
        if host_id.is_empty() {
            return;
        }
        if let Ok(mut guard) = self.state.write() {
            guard
                .forget_gen
                .entry(host_id.to_owned())
                .and_modify(|g| *g = g.wrapping_add(1))
                .or_insert(1);
            guard.capabilities.remove(host_id);
            guard.capabilities_fetched_at.remove(host_id);
            guard.cached_features.remove(host_id);
            guard.features_retry_at.remove(host_id);
            guard.libraries.remove(host_id);
            guard.fetched_at_ms.remove(host_id);
            guard.errors.remove(host_id);
            guard.live.remove(host_id);
            guard.sessions.remove(host_id);
            self.libraries.forget(host_id);
        } else {
            self.libraries.forget(host_id);
        }
        let _ = self.recents.forget_host(host_id);
        let _ = self.favourites.forget_host(host_id);
        let _ = self.hidden.forget_host(host_id);
        let _ = crate::hosts::HostMetaStore::open(&self.base_dir).forget(host_id);
    }

    pub fn hidden_listings(&self, hosts: &[HostDescriptor]) -> Vec<HiddenListing> {
        let owned = self.prepare_sources(hosts);
        let sources: Vec<Source<'_>> = owned.iter().map(OwnedHostSource::as_source).collect();
        let hidden = self.hidden.list();
        build_hidden_listings(&hidden, &sources)
    }

    pub fn project_home(
        &self,
        hosts: &[HostDescriptor],
        filter: &LibraryFilter,
        query: &str,
        sort: Sort,
    ) -> HomeProjection {
        let owned = self.prepare_sources(hosts);
        let sources: Vec<Source<'_>> = owned.iter().map(OwnedHostSource::as_source).collect();
        let rich: Vec<RichSource<'_>> = owned.iter().map(OwnedHostSource::as_rich).collect();
        let hidden_keys = self.hidden.list();
        let recents = self.recents.list();
        let all = merge_rich_library(&rich);
        let (shown, hidden_entries) = split_hidden(all, &sources, &hidden_keys);
        nightjar_policy::console::filters::project_home(
            &shown,
            &hidden_entries,
            &sources,
            &recents,
            filter,
            query,
            sort,
        )
    }

    fn prepare_sources(&self, hosts: &[HostDescriptor]) -> Vec<OwnedHostSource> {
        let mut guard = self.state.write().unwrap();
        let mut out = Vec::with_capacity(hosts.len());
        for (idx, host) in hosts.iter().enumerate() {
            let native_lib = if host.paired && host.is_native && !host.uuid.is_empty() {
                if let Some(mem) = guard.libraries.get(&host.uuid) {
                    Some(mem.clone())
                } else if let Some((disk, fetched_ms, feats)) = self.libraries.load_entry(&host.uuid) {
                    guard.libraries.insert(host.uuid.clone(), disk.clone());
                    if fetched_ms > 0 {
                        guard.fetched_at_ms.entry(host.uuid.clone()).or_insert(fetched_ms);
                    }
                    if !feats.is_empty() {
                        guard.cached_features.entry(host.uuid.clone()).or_insert(feats);
                    }
                    Some(disk)
                } else {
                    None
                }
            } else {
                None
            };

            let (library, extra, last_played, playtime_minutes, collections, classic) =
                if let Some(nl) = native_lib {
                    (
                        Some(nl.library),
                        Some(nl.extra),
                        nl.last_played,
                        nl.playtime_minutes,
                        nl.collections,
                        false,
                    )
                } else if host.paired
                    && let Some(classic_apps) = &host.classic_apps
                {
                    let running = host
                        .running_app_id
                        .map(|id| (id, host.running_app_uuid.as_deref()));
                    (
                        Some(classic_library(classic_apps, running)),
                        None,
                        BTreeMap::new(),
                        BTreeMap::new(),
                        BTreeMap::new(),
                        true,
                    )
                } else {
                    (None, None, BTreeMap::new(), BTreeMap::new(), BTreeMap::new(), !host.is_native)
                };

            let session = if host.online {
                guard.sessions.get(&host.uuid).cloned()
            } else {
                None
            };
            let host_control = guard
                .capabilities
                .get(&host.uuid)
                .is_some_and(|c| c.capabilities.permissions.host_control);

            out.push(OwnedHostSource {
                id: idx as u64 + 1,
                uuid: host.uuid.clone(),
                name: host.name.clone(),
                online: host.online,
                paired: host.paired,
                classic,
                library,
                extra,
                last_played,
                playtime_minutes,
                collections,
                session,
                host_control,
            });
        }
        out
    }

    fn build_snapshot(
        &self,
        hosts: &[HostDescriptor],
        is_refresh: bool,
        newly_fetched_caps: &HashSet<String>,
    ) -> ConsoleSnapshot {
        let owned = self.prepare_sources(hosts);
        let sources: Vec<Source<'_>> = owned.iter().map(OwnedHostSource::as_source).collect();
        let rich: Vec<RichSource<'_>> = owned.iter().map(OwnedHostSource::as_rich).collect();
        let initial_hidden = self.hidden.list();
        let mut reconciled_hidden = false;
        for o in &owned {
            if !o.paired || o.uuid.is_empty() || !initial_hidden.iter().any(|h| h.host == o.uuid)
            {
                continue;
            }
            let Some(lib) = &o.library else {
                continue;
            };
            let mut games_map: BTreeMap<String, u32> = BTreeMap::new();
            let mut seen_lower: HashMap<String, String> = HashMap::new();
            let mut duplicate_ids: HashSet<String> = HashSet::new();
            for g in &lib.games {
                if g.install_id.is_empty() || g.app_id == 0 {
                    continue;
                }
                let lower = g.install_id.to_ascii_lowercase();
                if duplicate_ids.contains(&lower) {
                    continue;
                }
                if let Some(prev_key) = seen_lower.remove(&lower) {
                    games_map.remove(&prev_key);
                    duplicate_ids.insert(lower);
                } else {
                    seen_lower.insert(lower, g.install_id.clone());
                    games_map.insert(g.install_id.clone(), g.app_id);
                }
            }
            let _ = self.hidden.following(&o.uuid, &games_map);
            reconciled_hidden = true;
        }
        let hidden_keys = if reconciled_hidden {
            self.hidden.list()
        } else {
            initial_hidden
        };
        let recents = self.recents.list();
        let mut favs = self.favourites.list();

        if let Some(repaired) = reconcile_starred(&favs, &sources) {
            let _ = self.favourites.replace(repaired.clone());
            favs = repaired;
        }

        let all = merge_rich_library(&rich);
        let (titles, hidden_titles) = split_hidden(all, &sources, &hidden_keys);
        let continue_list = continue_playing(&sources);

        let rev_parts: Vec<String> = owned
            .iter()
            .filter_map(|o| {
                o.library
                    .as_ref()
                    .map(|l| format!("{}:{}", o.uuid, l.revision))
            })
            .collect();
        let library_revision = rev_parts.join("|");
        let has_any_library = owned
            .iter()
            .any(|o| o.paired && o.library.as_ref().is_some_and(|l| !l.games.is_empty()))
            || self.libraries.has_any();

        let (sessions, host_states): (BTreeMap<String, Session>, Vec<HostSnapshotState>) = {
            let guard = self.state.read().unwrap();
            let sessions = hosts
                .iter()
                .filter(|h| h.paired && h.online)
                .filter_map(|h| {
                    guard
                        .sessions
                        .get(&h.uuid)
                        .map(|s| (h.uuid.clone(), s.clone()))
                })
                .collect();
            let host_states = hosts
                .iter()
                .filter(|h| h.paired)
                .map(|h| {
                    let owned_host = owned.iter().find(|o| o.uuid == h.uuid);
                    let has_lib = owned_host.is_some_and(|o| o.library.is_some());
                    let is_classic = owned_host.is_some_and(|o| o.classic);
                    let is_live = is_refresh && guard.live.get(&h.uuid).copied().unwrap_or(false);
                    let (error_kind, err) = match guard.errors.get(&h.uuid) {
                        Some(raw) => split_error_kind(raw),
                        None => (None, None),
                    };
                    let status = if !has_lib {
                        "UNAVAILABLE"
                    } else if is_classic {
                        "CLASSIC"
                    } else if h.online && is_live {
                        "LIVE"
                    } else {
                        "CACHED"
                    };
                    let fetched_at_ms = guard.fetched_at_ms.get(&h.uuid).copied().unwrap_or(0);
                    let features = guard
                        .capabilities
                        .get(&h.uuid)
                        .map(|c| c.capabilities.features.clone())
                        .or_else(|| guard.cached_features.get(&h.uuid).cloned())
                        .unwrap_or_default();
                    HostSnapshotState {
                        uuid: h.uuid.clone(),
                        name: h.name.clone(),
                        status: status.to_owned(),
                        fetched_at_ms,
                        error: err,
                        error_kind,
                        features,
                        capabilities_fetched: newly_fetched_caps.contains(&h.uuid),
                    }
                })
                .collect();
            (sessions, host_states)
        };

        ConsoleSnapshot {
            titles,
            hidden_titles,
            continue_playing: continue_list,
            recents,
            starred: favs,
            sessions,
            host_states,
            library_revision,
            has_any_library,
        }
    }

    #[cfg(test)]
    pub(crate) fn expire_capabilities_for_test(&self, uuid: &str) {
        let ts = Instant::now() - (CAPS_REFRESH_TTL + Duration::from_secs(1));
        self.state.write().unwrap().capabilities_fetched_at.insert(uuid.to_owned(), ts);
    }
}

fn split_error_kind(raw: &str) -> (Option<String>, Option<String>) {
    if let Some(rest) = raw.strip_prefix("__nj_kind:")
        && let Some((kind, msg)) = rest.split_once("__")
    {
        return (Some(kind.to_owned()), Some(msg.to_owned()));
    }
    (None, Some(raw.to_owned()))
}

impl OwnedHostSource {
    fn as_source(&self) -> Source<'_> {
        Source {
            id: self.id,
            unique_id: &self.uuid,
            name: &self.name,
            online: self.online,
            paired: self.paired,
            library: self.library.as_ref(),
            classic: self.classic,
            last_played: (!self.last_played.is_empty()).then_some(&self.last_played),
            playtime_minutes: (!self.playtime_minutes.is_empty()).then_some(&self.playtime_minutes),
            collections: (!self.collections.is_empty()).then_some(&self.collections),
            session: self.session.as_ref(),
            host_control: self.host_control,
        }
    }

    fn as_rich(&self) -> RichSource<'_> {
        RichSource {
            source: self.as_source(),
            extra: self.extra.as_deref(),
        }
    }
}
