//! Shared Nightjar Host Hub Sync policy (`docs/designs/09-host-hub-sync.md`):
//! offline-first [`SyncEngine`], outbox compaction, 3-way profile merge
//! (`Base + Local + Remote -> Merged | Conflict`), conflict resolution actions,
//! and SHA-256 cover diff planning (`covers_to_fetch`).
//!
//! Pure logic: zero I/O, zero clocks, shared identically across Desktop and Android.

use rubylight_native_api::hub::{
    CoverEntry, HostOverrideSummary, HubSnapshot, InputDeckSummary, ProfileWrite,
    StreamProfileSpec, SyncProfile,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// High-level state of Hub Sync for UI status badges and offline banners.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncStatus {
    #[default]
    Synced,
    RefreshingSnapshot,
    ReconcilingOutbox,
    ConflictPending,
    OfflineDegraded,
    PermissionDenied,
    Unsupported,
}

/// Queued mutation stored locally until acknowledged by the host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
#[allow(
    clippy::large_enum_variant,
    reason = "outbox vec keeps a handful of entries and serde tag layout stays flat"
)]
pub enum OutboxOp {
    Put {
        id: String,
        base_revision: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base: Option<SyncProfile>,
        write: ProfileWrite,
        queued_unix_ms: u64,
    },
    Delete {
        id: String,
        base_revision: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base: Option<SyncProfile>,
        queued_unix_ms: u64,
    },
}

impl OutboxOp {
    pub fn id(&self) -> &str {
        match self {
            Self::Put { id, .. } | Self::Delete { id, .. } => id,
        }
    }

    pub fn base_revision(&self) -> u64 {
        match self {
            Self::Put { base_revision, .. } | Self::Delete { base_revision, .. } => *base_revision,
        }
    }
}

/// Compacts consecutive pending operations for the same profile ID cleanly.
pub fn compact_outbox(ops: Vec<OutboxOp>) -> Vec<OutboxOp> {
    let mut order: Vec<String> = Vec::new();
    let mut by_id: BTreeMap<String, OutboxOp> = BTreeMap::new();

    for op in ops {
        let id = op.id().to_string();
        match by_id.get(&id).cloned() {
            None => {
                order.push(id.clone());
                by_id.insert(id, op);
            }
            Some(prev) => {
                let merged = match (prev, op) {
                    (
                        OutboxOp::Put {
                            base_revision,
                            base,
                            ..
                        },
                        OutboxOp::Put {
                            id,
                            write,
                            queued_unix_ms,
                            ..
                        },
                    ) => Some(OutboxOp::Put {
                        id,
                        base_revision,
                        base,
                        write,
                        queued_unix_ms,
                    }),
                    (
                        OutboxOp::Put {
                            base_revision: 0, ..
                        },
                        OutboxOp::Delete { .. },
                    ) => None,
                    (
                        OutboxOp::Put {
                            base_revision,
                            base,
                            ..
                        },
                        OutboxOp::Delete {
                            id, queued_unix_ms, ..
                        },
                    ) => Some(OutboxOp::Delete {
                        id,
                        base_revision,
                        base,
                        queued_unix_ms,
                    }),
                    (
                        OutboxOp::Delete {
                            base_revision,
                            base,
                            ..
                        },
                        OutboxOp::Put {
                            id,
                            write,
                            queued_unix_ms,
                            ..
                        },
                    ) => Some(OutboxOp::Put {
                        id,
                        base_revision,
                        base,
                        write,
                        queued_unix_ms,
                    }),
                    (
                        OutboxOp::Delete {
                            base_revision,
                            base,
                            ..
                        },
                        OutboxOp::Delete {
                            id, queued_unix_ms, ..
                        },
                    ) => Some(OutboxOp::Delete {
                        id,
                        base_revision,
                        base,
                        queued_unix_ms,
                    }),
                };
                if let Some(m) = merged {
                    by_id.insert(id, m);
                } else {
                    by_id.remove(&id);
                    order.retain(|k| k != &id);
                }
            }
        }
    }

    order
        .into_iter()
        .filter_map(|id| by_id.remove(&id))
        .collect()
}

/// Same-field collision requiring user choice (`KeepMine` / `UseHost` / `SaveAsCopy`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileConflict {
    pub id: String,
    pub server_revision: u64,
    pub conflicting_fields: Vec<String>,
    pub base: Option<SyncProfile>,
    pub local: ProfileWrite,
    pub remote: SyncProfile,
}

/// Result of 3-way merge (`Base + Local + Remote`).
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(
    clippy::large_enum_variant,
    reason = "keeps Merged(ProfileWrite) unboxed beside boxed ProfileConflict"
)]
pub enum MergeOutcome {
    Merged(ProfileWrite),
    Conflict(Box<ProfileConflict>),
}

/// User choice in the conflict resolver UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConflictChoice {
    KeepMine,
    UseHost,
    SaveAsCopy { new_id: String, new_name: String },
}

mod merge;
pub use merge::merge_three_way;

/// Converts an existing [`SyncProfile`] into an editable [`ProfileWrite`].
pub fn profile_to_write(p: &SyncProfile) -> ProfileWrite {
    ProfileWrite {
        scope: p.scope,
        owner_device_uuid: (!p.owner_device_uuid.is_empty()).then(|| p.owner_device_uuid.clone()),
        name: p.name.clone(),
        network_matcher: p.network_matcher.clone(),
        stream: p.stream.clone(),
        input_deck_id: p.input_deck_id.clone(),
    }
}

/// Strips optional weak indicator (`W/`) and surrounding quotes from an HTTP `ETag`.
pub fn normalize_etag(tag: &str) -> &str {
    let trimmed = tag.trim();
    let strong = trimmed.strip_prefix("W/").unwrap_or(trimmed).trim();
    strong
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(strong)
}

/// Returns remote covers whose content ETag differs from the caller's local SHA-256 index.
pub fn covers_to_fetch<'a>(
    remote: &'a [CoverEntry],
    local_etags: &BTreeMap<u32, String>,
) -> Vec<&'a CoverEntry> {
    let mut missing: Vec<&CoverEntry> = remote
        .iter()
        .filter(|c| {
            !c.etag.is_empty()
                && local_etags.get(&c.app_id).map(|t| normalize_etag(t))
                    != Some(normalize_etag(&c.etag))
        })
        .collect();
    missing.sort_by_key(|c| c.app_id);
    missing
}

/// Returns locally cached `app_id`s that no longer have a custom cover on the host.
pub fn stale_local_cover_ids(
    remote: &[CoverEntry],
    local_etags: &BTreeMap<u32, String>,
) -> Vec<u32> {
    let active: BTreeSet<u32> = remote.iter().map(|c| c.app_id).collect();
    local_etags
        .keys()
        .copied()
        .filter(|app_id| !active.contains(app_id))
        .collect()
}

/// Pure offline-first Hub Sync state machine for one host.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncEngine {
    #[serde(default)]
    pub snapshot: Option<HubSnapshot>,
    #[serde(default)]
    pub snapshot_etag: Option<String>,
    #[serde(default)]
    pub outbox: Vec<OutboxOp>,
    #[serde(default)]
    pub conflict: Option<ProfileConflict>,
    #[serde(default)]
    pub status: SyncStatus,
    #[serde(default)]
    pub last_error: Option<String>,
}

impl SyncEngine {
    pub fn from_cached(snapshot: Option<HubSnapshot>, outbox: Vec<OutboxOp>) -> Self {
        let snapshot_etag = snapshot
            .as_ref()
            .map(|s| format!("\"rev:{}\"", s.snapshot_revision));
        let compacted = compact_outbox(outbox);
        Self {
            snapshot,
            snapshot_etag,
            outbox: compacted,
            conflict: None,
            status: SyncStatus::OfflineDegraded,
            last_error: None,
        }
    }

    pub fn apply_snapshot_200(&mut self, snapshot: HubSnapshot, etag: Option<String>) {
        self.snapshot_etag =
            Some(etag.unwrap_or_else(|| format!("\"rev:{}\"", snapshot.snapshot_revision)));
        self.snapshot = Some(snapshot);
        self.last_error = None;
        if self.conflict.is_some() {
            self.status = SyncStatus::ConflictPending;
        } else if self.outbox.is_empty() {
            self.status = SyncStatus::Synced;
        } else {
            self.status = SyncStatus::ReconcilingOutbox;
        }
    }

    pub fn apply_snapshot_304(&mut self) {
        self.last_error = None;
        if self.conflict.is_some() {
            self.status = SyncStatus::ConflictPending;
        } else if self.outbox.is_empty() {
            self.status = SyncStatus::Synced;
        } else {
            self.status = SyncStatus::ReconcilingOutbox;
        }
    }

    pub fn queue_put(&mut self, id: String, write: ProfileWrite, now_unix_ms: u64) {
        let base = self
            .snapshot
            .as_ref()
            .and_then(|s| s.profiles.iter().find(|p| p.id == id))
            .cloned();
        let base_revision = base.as_ref().map_or(0, |p| p.revision);
        self.outbox.push(OutboxOp::Put {
            id,
            base_revision,
            base,
            write,
            queued_unix_ms: now_unix_ms,
        });
        self.outbox = compact_outbox(std::mem::take(&mut self.outbox));
    }

    pub fn queue_delete(&mut self, id: &str, now_unix_ms: u64) {
        let base = self
            .snapshot
            .as_ref()
            .and_then(|s| s.profiles.iter().find(|p| p.id == id))
            .cloned();
        let base_revision = base.as_ref().map_or(0, |p| p.revision);
        self.outbox.push(OutboxOp::Delete {
            id: id.to_string(),
            base_revision,
            base,
            queued_unix_ms: now_unix_ms,
        });
        self.outbox = compact_outbox(std::mem::take(&mut self.outbox));
    }

    /// Pops the next queued op once the server acknowledges it.
    pub fn on_put_succeeded(&mut self, updated: SyncProfile) {
        let id = updated.id.clone();
        self.outbox.retain(|op| op.id() != id);
        if self.conflict.as_ref().is_some_and(|c| c.id == id) {
            self.conflict = None;
        }
        if let Some(ref mut snap) = self.snapshot {
            if let Some(slot) = snap.profiles.iter_mut().find(|p| p.id == id) {
                *slot = updated;
            } else {
                snap.profiles.push(updated);
                snap.profiles.sort_by(|a, b| a.id.cmp(&b.id));
            }
        }
        self.last_error = None;
        self.status = if self.outbox.is_empty() {
            SyncStatus::Synced
        } else {
            SyncStatus::ReconcilingOutbox
        };
    }

    pub fn on_delete_succeeded(&mut self, id: &str, snapshot_revision: u64) {
        self.outbox.retain(|op| op.id() != id);
        if let Some(ref mut snap) = self.snapshot {
            snap.profiles.retain(|p| p.id != id);
            if snapshot_revision > snap.snapshot_revision {
                snap.snapshot_revision = snapshot_revision;
                self.snapshot_etag = Some(format!("\"rev:{snapshot_revision}\""));
            }
        }
        self.last_error = None;
        self.status = if self.outbox.is_empty() {
            SyncStatus::Synced
        } else {
            SyncStatus::ReconcilingOutbox
        };
    }

    /// Reconciles a `409 RevisionConflict` against the freshly fetched `remote` profile.
    /// Returns `Some(retry_op)` if changes auto-merged cleanly, or `None` if user decision needed.
    pub fn reconcile_conflict(
        &mut self,
        id: &str,
        remote: Option<&SyncProfile>,
    ) -> Option<OutboxOp> {
        let idx = self.outbox.iter().position(|op| op.id() == id)?;
        let op = self.outbox[idx].clone();
        match (op, remote) {
            (
                OutboxOp::Put {
                    base,
                    write,
                    queued_unix_ms,
                    ..
                },
                Some(remote_prof),
            ) => match merge_three_way(base.as_ref(), &write, remote_prof) {
                MergeOutcome::Merged(merged) => {
                    let next = OutboxOp::Put {
                        id: id.to_string(),
                        base_revision: remote_prof.revision,
                        base: Some(remote_prof.clone()),
                        write: merged,
                        queued_unix_ms,
                    };
                    self.outbox[idx] = next.clone();
                    self.status = SyncStatus::ReconcilingOutbox;
                    Some(next)
                }
                MergeOutcome::Conflict(conflict) => {
                    self.conflict = Some(*conflict);
                    self.status = SyncStatus::ConflictPending;
                    None
                }
            },
            (
                OutboxOp::Put {
                    base,
                    write,
                    queued_unix_ms,
                    ..
                },
                None,
            ) => {
                // Server profile was deleted remotely; convert create/recreate conflict into prompt.
                let synthetic_remote = SyncProfile {
                    id: id.to_string(),
                    revision: 0,
                    scope: write.scope,
                    owner_device_uuid: write.owner_device_uuid.clone().unwrap_or_default(),
                    name: String::new(),
                    network_matcher: None,
                    stream: StreamProfileSpec::default(),
                    input_deck_id: None,
                    updated_unix_ms: queued_unix_ms,
                    updated_by_device: String::new(),
                };
                self.conflict = Some(ProfileConflict {
                    id: id.to_string(),
                    server_revision: 0,
                    conflicting_fields: vec!["deleted_on_host".into()],
                    base,
                    local: write,
                    remote: synthetic_remote,
                });
                self.status = SyncStatus::ConflictPending;
                None
            }
            (OutboxOp::Delete { .. }, None) => {
                // Already deleted on host.
                self.outbox.remove(idx);
                self.status = if self.outbox.is_empty() {
                    SyncStatus::Synced
                } else {
                    SyncStatus::ReconcilingOutbox
                };
                None
            }
            (OutboxOp::Delete { queued_unix_ms, .. }, Some(remote_prof)) => {
                let next = OutboxOp::Delete {
                    id: id.to_string(),
                    base_revision: remote_prof.revision,
                    base: Some(remote_prof.clone()),
                    queued_unix_ms,
                };
                self.outbox[idx] = next.clone();
                Some(next)
            }
        }
    }

    pub fn resolve_conflict(&mut self, choice: ConflictChoice, now_unix_ms: u64) {
        let Some(conflict) = self.conflict.take() else {
            return;
        };
        let target_id = conflict.id.clone();
        self.outbox.retain(|op| op.id() != target_id);
        match choice {
            ConflictChoice::KeepMine => {
                self.outbox.insert(
                    0,
                    OutboxOp::Put {
                        id: target_id,
                        base_revision: conflict.server_revision,
                        base: Some(conflict.remote),
                        write: conflict.local,
                        queued_unix_ms: now_unix_ms,
                    },
                );
                self.status = SyncStatus::ReconcilingOutbox;
            }
            ConflictChoice::UseHost => {
                if let Some(ref mut snap) = self.snapshot {
                    if conflict.server_revision == 0 {
                        snap.profiles.retain(|p| p.id != target_id);
                    } else if let Some(slot) = snap.profiles.iter_mut().find(|p| p.id == target_id)
                    {
                        *slot = conflict.remote;
                    } else {
                        snap.profiles.push(conflict.remote);
                    }
                }
                self.status = if self.outbox.is_empty() {
                    SyncStatus::Synced
                } else {
                    SyncStatus::ReconcilingOutbox
                };
            }
            ConflictChoice::SaveAsCopy { new_id, new_name } => {
                let mut copy_write = conflict.local;
                copy_write.name = new_name.chars().take(120).collect();
                self.outbox.push(OutboxOp::Put {
                    id: new_id,
                    base_revision: 0,
                    base: None,
                    write: copy_write,
                    queued_unix_ms: now_unix_ms,
                });
                if let Some(ref mut snap) = self.snapshot
                    && conflict.server_revision > 0
                    && let Some(slot) = snap.profiles.iter_mut().find(|p| p.id == target_id)
                {
                    *slot = conflict.remote;
                }
                self.status = SyncStatus::ReconcilingOutbox;
            }
        }
    }

    pub fn on_forbidden(&mut self, message: impl Into<String>) {
        self.status = SyncStatus::PermissionDenied;
        self.last_error = Some(message.into());
    }

    pub fn on_offline(&mut self, message: Option<String>) {
        self.status = SyncStatus::OfflineDegraded;
        self.last_error = message;
    }

    /// Profiles with queued offline edits overlaid cleanly for immediate UI display.
    pub fn effective_profiles(&self) -> Vec<SyncProfile> {
        let mut map: BTreeMap<String, SyncProfile> = self
            .snapshot
            .as_ref()
            .map(|s| {
                s.profiles
                    .iter()
                    .map(|p| (p.id.clone(), p.clone()))
                    .collect()
            })
            .unwrap_or_default();
        for op in &self.outbox {
            match op {
                OutboxOp::Put {
                    id,
                    base_revision,
                    write,
                    queued_unix_ms,
                    ..
                } => {
                    let prev = map.get(id).cloned().unwrap_or_default();
                    map.insert(
                        id.clone(),
                        SyncProfile {
                            id: id.clone(),
                            revision: *base_revision,
                            scope: write.scope,
                            owner_device_uuid: write
                                .owner_device_uuid
                                .clone()
                                .unwrap_or(prev.owner_device_uuid),
                            name: write.name.clone(),
                            network_matcher: write.network_matcher.clone(),
                            stream: write.stream.clone(),
                            input_deck_id: write.input_deck_id.clone(),
                            updated_unix_ms: *queued_unix_ms,
                            updated_by_device: prev.updated_by_device,
                        },
                    );
                }
                OutboxOp::Delete { id, .. } => {
                    map.remove(id);
                }
            }
        }
        map.into_values().collect()
    }

    pub fn host_overrides(&self) -> &[HostOverrideSummary] {
        self.snapshot
            .as_ref()
            .map_or(&[], |s| s.host_overrides_summary.as_slice())
    }

    pub fn input_decks(&self) -> &[InputDeckSummary] {
        self.snapshot
            .as_ref()
            .map_or(&[], |s| s.input_decks.as_slice())
    }
}

#[cfg(test)]
mod tests;
