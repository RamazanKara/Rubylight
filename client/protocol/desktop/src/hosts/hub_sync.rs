//! Host Hub Sync worker driver (`Lane::Asked`), outbox reconciliation, conflict auto-merge
//! and manual resolution, and SHA-256 ETag cover download handoff to `hosts/covers.rs`.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use nightjar_core::http::native_api::error::ErrorCode;
use nightjar_core::http::native_api::hub::ProfileWrite;
use nightjar_core::http::{
    Error as HttpError, Feature, HostClient, HubCoverResult, HubDeleteResult, HubPutResult,
    HubSnapshotResult,
};
use nightjar_desktop_ui::hosts_model::HubStateView;
use nightjar_desktop_ui::ui::HubConflictChoice;
use nightjar_policy::hub_sync::{
    ConflictChoice, OutboxOp, SyncEngine, SyncStatus, covers_to_fetch,
};

use super::hub_cache;
use super::{Event, Host, Hosts, Lane, client_for};
use crate::config::Store;

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

pub(super) fn hub_view_from_engine(engine: &SyncEngine) -> HubStateView {
    HubStateView {
        status: engine.status,
        snapshot_revision: engine.snapshot.as_ref().map_or(0, |s| s.snapshot_revision),
        profiles: engine.effective_profiles(),
        input_decks: engine.input_decks().to_vec(),
        host_overrides: engine.host_overrides().to_vec(),
        pending_outbox_count: engine.outbox.len(),
        conflict: engine.conflict.clone().map(Box::new),
        error: engine.last_error.clone(),
    }
}

impl Host {
    pub(super) fn refresh_native_hub_view(&mut self) {
        self.native.hub = hub_view_from_engine(&self.hub_engine);
    }

    pub(super) fn app_cover_versions(&self) -> HashMap<u32, String> {
        let mut out = HashMap::new();
        if let Some(lib) = &self.native.library {
            for game in &lib.games {
                out.insert(game.app_id, game.artwork.version.clone());
            }
        }
        out
    }
}

impl Hosts {
    /// Loads cached `hub/<unique_id>/{snapshot,outbox,cover_etags}.json` for offline startup.
    pub fn load_cached_hubs(&mut self, store: &Store) {
        self.store_dir = Some(store.dir().to_path_buf());
        for host in &mut self.list {
            if host.record.server_certificate.is_some() && !host.record.unique_id.is_empty() {
                let cached = hub_cache::load_host_cache(store.dir(), &host.record.unique_id);
                let mut engine = SyncEngine::from_cached(cached.snapshot, cached.outbox);
                if engine.snapshot.is_none() && engine.outbox.is_empty() {
                    engine.status = SyncStatus::Unsupported;
                }
                host.hub_engine = engine;
                host.hub_cover_etags = cached.cover_etags;
                host.refresh_native_hub_view();
            }
        }
    }

    /// Starts one Hub Sync cycle on `Lane::Asked` if the PC offers `Feature::HubSync`.
    pub fn sync_hub(&mut self, id: u64) {
        let Some(identity) = self.identity.clone() else {
            return;
        };
        let store_dir = self.store_dir.clone();
        let Some(host) = self.get_mut(id) else {
            return;
        };
        let supported = host
            .info
            .as_ref()
            .is_some_and(|info| info.capabilities.has(Feature::HubSync));
        if !supported || host.record.server_certificate.is_none() || host.hub_inflight {
            return;
        }
        host.hub_inflight = true;
        let epoch = host.native_epoch;
        let record = host.connect_record();
        let addr = host.connect_address().to_owned();
        let unique_id = host.record.unique_id.clone();
        let mut engine = host.hub_engine.clone();
        let mut etags = host.hub_cover_etags.clone();
        let cover_versions = host.app_cover_versions();

        if !self.run_on(id, Lane::Asked, move || {
            let Ok(client) = client_for(&identity, &record, &addr) else {
                engine.on_offline(Some("PC client connection unavailable.".into()));
                return Some(Event::HubSync {
                    id,
                    epoch,
                    engine: Box::new(engine),
                    cover_etags: etags,
                    updated_covers: Vec::new(),
                });
            };
            let updated_covers = sync_hub_cycle(
                &client,
                &mut engine,
                &mut etags,
                store_dir.as_deref(),
                &unique_id,
                &cover_versions,
                now_unix_ms(),
            );
            Some(Event::HubSync {
                id,
                epoch,
                engine: Box::new(engine),
                cover_etags: etags,
                updated_covers,
            })
        }) && let Some(host) = self.get_mut(id)
        {
            host.hub_inflight = false;
        }
    }

    /// Queues a local profile write (`PUT`) to disk immediately and syncs if online.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn queue_hub_put(&mut self, id: u64, profile_id: String, write: ProfileWrite) {
        let store_dir = self.store_dir.clone();
        let now = now_unix_ms();
        let Some(host) = self.get_mut(id) else {
            return;
        };
        host.hub_engine.queue_put(profile_id, write, now);
        host.hub_engine.status = SyncStatus::ReconcilingOutbox;
        if let Some(ref dir) = store_dir
            && !host.record.unique_id.is_empty()
        {
            let _ = hub_cache::save_outbox(dir, &host.record.unique_id, &host.hub_engine.outbox);
        }
        host.refresh_native_hub_view();
        self.sync_hub(id);
    }

    /// Queues a local profile delete (`DELETE`) to disk immediately and syncs if online.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn queue_hub_delete(&mut self, id: u64, profile_id: &str) {
        let store_dir = self.store_dir.clone();
        let now = now_unix_ms();
        let Some(host) = self.get_mut(id) else {
            return;
        };
        host.hub_engine.queue_delete(profile_id, now);
        host.hub_engine.status = SyncStatus::ReconcilingOutbox;
        if let Some(ref dir) = store_dir
            && !host.record.unique_id.is_empty()
        {
            let _ = hub_cache::save_outbox(dir, &host.record.unique_id, &host.hub_engine.outbox);
        }
        host.refresh_native_hub_view();
        self.sync_hub(id);
    }

    /// Applies the user's conflict choice (`KeepMine` / `UseHost` / `SaveAsCopy`) and resumes sync.
    pub fn resolve_hub_conflict(&mut self, id: u64, choice: HubConflictChoice) {
        let store_dir = self.store_dir.clone();
        let now = now_unix_ms();
        let Some(host) = self.get_mut(id) else {
            return;
        };
        let policy_choice = match choice {
            HubConflictChoice::KeepMine => ConflictChoice::KeepMine,
            HubConflictChoice::UseHost => ConflictChoice::UseHost,
            HubConflictChoice::SaveAsCopy => {
                let conflict = host.hub_engine.conflict.as_ref();
                let base_id = conflict.map_or("profile", |c| c.id.as_str());
                let suffix = (now % 100_000).to_string();
                let prefix_len = base_id.len().min(63 - suffix.len() - 1);
                let new_id = format!("{}-{suffix}", &base_id[..prefix_len]);
                let base_name = conflict.map_or("Profile", |c| c.local.name.as_str());
                let new_name = format!("{base_name} (copy)");
                ConflictChoice::SaveAsCopy { new_id, new_name }
            }
        };
        host.hub_engine.resolve_conflict(policy_choice, now);
        if let Some(ref dir) = store_dir
            && !host.record.unique_id.is_empty()
        {
            let _ = hub_cache::save_snapshot(
                dir,
                &host.record.unique_id,
                host.hub_engine.snapshot.as_ref(),
            );
            let _ = hub_cache::save_outbox(dir, &host.record.unique_id, &host.hub_engine.outbox);
        }
        host.refresh_native_hub_view();
        if !host.hub_engine.outbox.is_empty() {
            self.sync_hub(id);
        }
    }
}

/// Runs one blocking Hub Sync pass: conditional snapshot, outbox replay + 3-way merge, ETag covers.
pub(crate) fn sync_hub_cycle(
    client: &HostClient,
    engine: &mut SyncEngine,
    cover_etags: &mut BTreeMap<u32, String>,
    store_dir: Option<&Path>,
    unique_id: &str,
    cover_versions: &HashMap<u32, String>,
    _now_ms: u64,
) -> Vec<u32> {
    let mut updated_covers = Vec::new();
    match client.hub_snapshot(engine.snapshot_etag.as_deref(), false) {
        Ok(HubSnapshotResult::Modified { snapshot, etag }) => {
            engine.apply_snapshot_200(snapshot, etag);
        }
        Ok(HubSnapshotResult::NotModified) => {
            engine.apply_snapshot_304();
        }
        Err(err) if is_permission_error(&err) => {
            engine.on_forbidden(err.to_string());
            persist_engine(store_dir, unique_id, engine, cover_etags);
            return updated_covers;
        }
        Err(err) => {
            engine.on_offline(Some(err.to_string()));
            persist_engine(store_dir, unique_id, engine, cover_etags);
            return updated_covers;
        }
    }

    // Reconcile pending outbox mutations in order.
    while let Some(op) = engine.outbox.first().cloned() {
        match op {
            OutboxOp::Put {
                id,
                base_revision,
                write,
                ..
            } => {
                if !replay_put(client, engine, &id, base_revision, &write) {
                    break;
                }
            }
            OutboxOp::Delete {
                id, base_revision, ..
            } => {
                if !replay_delete(client, engine, &id, base_revision) {
                    break;
                }
            }
        }
    }

    // Sync any changed covers when connected and permitted.
    if let Some(snap) = engine.snapshot.clone() {
        let missing: Vec<(u32, String)> = covers_to_fetch(&snap.covers, cover_etags)
            .into_iter()
            .map(|c| (c.app_id, c.etag.clone()))
            .collect();
        for (app_id, _expected_etag) in missing {
            let local_etag = cover_etags.get(&app_id).map(String::as_str);
            match client.hub_cover(app_id, local_etag) {
                Ok(HubCoverResult::Image { etag, bytes, .. }) => {
                    let version = cover_versions
                        .get(&app_id)
                        .map(String::as_str)
                        .unwrap_or("");
                    let written = if let Some(dir) = store_dir.filter(|_| !unique_id.is_empty()) {
                        hub_cache::save_cover_to_art(dir, unique_id, app_id, version, &bytes)
                            .unwrap_or(false)
                    } else {
                        true
                    };
                    if written {
                        if let Some(new_etag) = etag {
                            cover_etags.insert(app_id, new_etag);
                        }
                        updated_covers.push(app_id);
                    }
                }
                Ok(HubCoverResult::NotModified) => {}
                Err(_) => break,
            }
        }
    }

    persist_engine(store_dir, unique_id, engine, cover_etags);
    updated_covers
}

fn replay_put(
    client: &HostClient,
    engine: &mut SyncEngine,
    id: &str,
    base_revision: u64,
    write: &ProfileWrite,
) -> bool {
    match client.put_hub_profile(id, &base_revision.to_string(), write) {
        Ok(HubPutResult::Saved { profile, .. }) => {
            engine.on_put_succeeded(profile);
            true
        }
        Ok(HubPutResult::Conflict { .. }) => {
            if let Ok(HubSnapshotResult::Modified { snapshot, etag }) =
                client.hub_snapshot(None, false)
            {
                engine.apply_snapshot_200(snapshot, etag);
            }
            let remote = engine
                .snapshot
                .as_ref()
                .and_then(|s| s.profiles.iter().find(|p| p.id == id))
                .cloned();
            if let Some(OutboxOp::Put {
                base_revision: new_rev,
                write: merged,
                ..
            }) = engine.reconcile_conflict(id, remote.as_ref())
            {
                match client.put_hub_profile(id, &new_rev.to_string(), &merged) {
                    Ok(HubPutResult::Saved { profile, .. }) => {
                        engine.on_put_succeeded(profile);
                        return true;
                    }
                    Ok(HubPutResult::Conflict { .. }) => {
                        let _ = engine.reconcile_conflict(id, remote.as_ref());
                        return false;
                    }
                    Err(err) => {
                        classify_replay_error(engine, &err);
                        return false;
                    }
                }
            }
            false
        }
        Err(err) => {
            classify_replay_error(engine, &err);
            false
        }
    }
}

fn replay_delete(
    client: &HostClient,
    engine: &mut SyncEngine,
    id: &str,
    base_revision: u64,
) -> bool {
    match client.delete_hub_profile(id, &base_revision.to_string()) {
        Ok(HubDeleteResult::Deleted(res)) => {
            engine.on_delete_succeeded(id, res.snapshot_revision);
            true
        }
        Ok(HubDeleteResult::Conflict { .. }) => {
            if let Ok(HubSnapshotResult::Modified { snapshot, etag }) =
                client.hub_snapshot(None, false)
            {
                engine.apply_snapshot_200(snapshot, etag);
            }
            let remote = engine
                .snapshot
                .as_ref()
                .and_then(|s| s.profiles.iter().find(|p| p.id == id))
                .cloned();
            if let Some(OutboxOp::Delete {
                base_revision: new_rev,
                ..
            }) = engine.reconcile_conflict(id, remote.as_ref())
            {
                if let Ok(HubDeleteResult::Deleted(res)) =
                    client.delete_hub_profile(id, &new_rev.to_string())
                {
                    engine.on_delete_succeeded(id, res.snapshot_revision);
                    return true;
                }
                return false;
            }
            engine.conflict.is_none()
        }
        Err(err) => {
            classify_replay_error(engine, &err);
            false
        }
    }
}

fn is_permission_error(err: &HttpError) -> bool {
    match err {
        HttpError::Api { status: 403, .. } => true,
        HttpError::Api { error: e, .. } => matches!(
            e.code,
            ErrorCode::NotPaired | ErrorCode::PermissionRequired | ErrorCode::HostControlRequired
        ),
        _ => false,
    }
}

fn classify_replay_error(engine: &mut SyncEngine, err: &HttpError) {
    if is_permission_error(err) {
        engine.on_forbidden(err.to_string());
    } else if !matches!(
        err,
        HttpError::Api {
            status: 400 | 413 | 422,
            ..
        }
    ) {
        engine.on_offline(Some(err.to_string()));
    } else {
        engine.last_error = Some(err.to_string());
        engine.outbox.remove(0);
        if engine.outbox.is_empty() {
            engine.status = SyncStatus::Synced;
        }
    }
}

fn persist_engine(
    store_dir: Option<&Path>,
    unique_id: &str,
    engine: &SyncEngine,
    cover_etags: &BTreeMap<u32, String>,
) {
    if let Some(dir) = store_dir.filter(|_| !unique_id.is_empty()) {
        let _ = hub_cache::save_snapshot(dir, unique_id, engine.snapshot.as_ref());
        let _ = hub_cache::save_outbox(dir, unique_id, &engine.outbox);
        let _ = hub_cache::save_cover_etags(dir, unique_id, cover_etags);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nightjar_core::http::native_api::hub::ProfileScope;

    #[test]
    fn offline_outbox_survives_before_sync() {
        let mut engine = SyncEngine::from_cached(None, Vec::new());
        let write = ProfileWrite {
            scope: ProfileScope::Device,
            name: "Travel 720p".into(),
            ..Default::default()
        };
        engine.queue_put("travel-720".into(), write, 100);
        let view = hub_view_from_engine(&engine);
        assert_eq!(view.pending_outbox_count, 1);
        assert_eq!(view.profiles.len(), 1);
        assert_eq!(view.profiles[0].name, "Travel 720p");
    }

    #[test]
    fn hosts_queue_hub_put_and_delete_persist_outbox_to_store() {
        let dir = std::env::temp_dir().join(format!("nightjar-hosts-hub-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::at(dir.clone()).unwrap();
        let mut record = crate::config::HostRecord {
            unique_id: "host-hub-test".into(),
            name: "PC".into(),
            address: "127.0.0.1".into(),
            ..Default::default()
        };
        record.server_certificate =
            Some("-----BEGIN CERTIFICATE-----\nAA==\n-----END CERTIFICATE-----\n".into());
        let mut hosts = Hosts::without_discovery(vec![record], std::sync::Arc::new(|| {}));
        hosts.load_cached_hubs(&store);
        let host_id = hosts.list[0].id;

        let write = ProfileWrite {
            scope: ProfileScope::Device,
            name: "Couch 4K".into(),
            ..Default::default()
        };
        hosts.queue_hub_put(host_id, "couch-4k".into(), write);
        assert_eq!(hosts.list[0].native.hub.pending_outbox_count, 1);

        let cached = hub_cache::load_host_cache(store.dir(), "host-hub-test");
        assert_eq!(cached.outbox.len(), 1);

        hosts.queue_hub_delete(host_id, "couch-4k");
        assert_eq!(hosts.list[0].native.hub.pending_outbox_count, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
