//! Host Hub Sync policy & state model (`docs/designs/09-host-hub-sync.md`):
//! ID/quota/schema validation, optimistic CAS revision checks (`If-Match`),
//! `#screen2` normalization via [`crate::state::device_uuid`], per-device scope
//! enforcement, and read-only `device_overrides` summary projection.
use crate::{
    device_overrides,
    state::{self, App, Client},
};
use butterpollo_media_policy::radeon_profile;
use regex::Regex;
use rubylight_native_api::{
    error::ErrorCode,
    hub::{
        HostOverrideSummary, InputDeckSummary, ProfileDeleteResult, ProfileScope, ProfileWrite,
        SyncProfile,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

/// Durable `hub-sync.json` schema version.
pub const STATE_VERSION: u32 = 1;
/// Maximum synced profiles owned by one device.
pub const MAX_PROFILES_PER_DEVICE: usize = 64;
/// Maximum synced profiles stored on one host across all devices.
pub const MAX_PROFILES_HOST: usize = 256;
/// Valid upscaling wire strings accepted on write (`"sgsr2"` maps to `"sgsr1"` on read).
pub const ALLOWED_UPSCALING: [&str; 5] = ["off", "bilinear", "fsr1", "sgsr1", "metalfx"];

fn default_version() -> u32 {
    STATE_VERSION
}

fn default_revision() -> u64 {
    1
}

fn id_regex() -> &'static Regex {
    static ID: OnceLock<Regex> = OnceLock::new();
    ID.get_or_init(|| Regex::new("^[a-z0-9][a-z0-9_-]{0,63}$").expect("valid hub id regex"))
}

/// Pure host state persisted in `hub-sync.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubState {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default = "default_revision")]
    pub snapshot_revision: u64,
    #[serde(default)]
    pub profiles: BTreeMap<String, SyncProfile>,
    #[serde(default)]
    pub input_decks: BTreeMap<String, InputDeckSummary>,
    /// Last observed cover digest per `app_id`, used so unchanged restarts do not bump revisions.
    #[serde(default)]
    pub cover_digests: BTreeMap<u32, String>,
}

impl Default for HubState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            snapshot_revision: 1,
            profiles: BTreeMap::new(),
            input_decks: BTreeMap::new(),
            cover_digests: BTreeMap::new(),
        }
    }
}

/// Outcome of a `PUT /hub/profiles/{id}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PutOutcome {
    pub created: bool,
    pub profile: SyncProfile,
    pub snapshot_revision: u64,
}

/// Errors produced by pure Hub Sync policy checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HubError {
    InvalidRequest(String),
    InvalidValue {
        setting: Option<String>,
        message: String,
    },
    HostControlRequired(String),
    NotFound(String),
    RevisionConflict {
        profile_id: String,
        current_revision: u64,
    },
}

impl HubError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::InvalidRequest(_) => ErrorCode::InvalidRequest,
            Self::InvalidValue { .. } => ErrorCode::InvalidValue,
            Self::HostControlRequired(_) => ErrorCode::HostControlRequired,
            Self::NotFound(_) => ErrorCode::NotFound,
            Self::RevisionConflict { .. } => ErrorCode::RevisionConflict,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::InvalidRequest(m)
            | Self::InvalidValue { message: m, .. }
            | Self::HostControlRequired(m)
            | Self::NotFound(m) => m,
            Self::RevisionConflict { .. } => {
                "This profile changed on another device. Refresh from the PC before saving."
            }
        }
    }

    pub fn setting(&self) -> Option<&str> {
        match self {
            Self::InvalidValue { setting, .. } => setting.as_deref(),
            Self::RevisionConflict { profile_id, .. } => Some(profile_id.as_str()),
            _ => None,
        }
    }

    pub fn current_revision(&self) -> Option<u64> {
        match self {
            Self::RevisionConflict {
                current_revision, ..
            } => Some(*current_revision),
            _ => None,
        }
    }
}

/// Validates a profile or deck identifier (`^[a-z0-9][a-z0-9_-]{0,63}$`).
pub fn validate_id(id: &str) -> Result<(), HubError> {
    if id_regex().is_match(id) {
        Ok(())
    } else {
        Err(HubError::InvalidRequest(
            "Profile IDs must be 1–64 lowercase ASCII letters, digits, '-' or '_', starting with a letter or digit."
                .into(),
        ))
    }
}

/// Validates a [`ProfileWrite`] payload prior to CAS mutation.
pub fn validate_profile_write(id: &str, write: &ProfileWrite) -> Result<(), HubError> {
    validate_id(id)?;
    let trimmed = write.name.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 120 {
        return Err(HubError::InvalidValue {
            setting: Some("name".into()),
            message: "Profile name must be between 1 and 120 characters.".into(),
        });
    }
    if write.scope == ProfileScope::Unknown {
        return Err(HubError::InvalidValue {
            setting: Some("scope".into()),
            message: "Profile scope must be 'device' or 'shared'.".into(),
        });
    }
    if let Some(ref upscaling) = write.stream.upscaling
        && !ALLOWED_UPSCALING.contains(&upscaling.as_str())
    {
        return Err(HubError::InvalidValue {
            setting: Some("upscaling".into()),
            message: format!("Unsupported upscaling value '{upscaling}'."),
        });
    }
    if let Some(scale) = write.stream.resolution_scale_pct
        && !(50..=100).contains(&scale)
    {
        return Err(HubError::InvalidValue {
            setting: Some("resolution_scale_pct".into()),
            message: "Resolution scale must be between 50% and 100%.".into(),
        });
    }
    if let Some(ref deck_id) = write.input_deck_id {
        validate_id(deck_id)?;
    }
    Ok(())
}

/// Parses a numeric `If-Match` revision header (`"7"`, `7`, or `W/"7"`).
pub fn parse_if_match_revision(raw: &str) -> Result<u64, HubError> {
    let trimmed = raw.trim();
    let trimmed = trimmed.strip_prefix("W/").unwrap_or(trimmed).trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(trimmed)
        .trim();
    unquoted.parse::<u64>().map_err(|_| {
        HubError::InvalidRequest(
            "Send a quoted numeric If-Match revision header (use \"0\" to create a profile)."
                .into(),
        )
    })
}

fn normalize_etag(token: &str) -> &str {
    let t = token.trim();
    let t = t.strip_prefix("W/").unwrap_or(t).trim();
    t.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(t)
}

/// Returns true when an `If-None-Match` header matches `current_etag`.
pub fn etag_matches(if_none_match: &str, current_etag: &str) -> bool {
    let want = normalize_etag(current_etag);
    if_none_match.split(',').any(|candidate| {
        let cand = candidate.trim();
        cand == "*" || normalize_etag(cand) == want
    })
}

/// Normalized device identity for Hub Sync (`#screen2` shares its primary device's bucket).
pub fn normalized_device_uuid(client: &Client) -> &str {
    state::device_uuid(&client.uuid)
}

impl HubState {
    /// Lists profiles visible to `caller_uuid`. Only `host_control` callers may see all devices.
    pub fn visible_profiles(
        &self,
        caller_uuid: &str,
        all_devices: bool,
        host_control: bool,
    ) -> Vec<SyncProfile> {
        self.profiles
            .values()
            .filter(|profile| {
                (all_devices && host_control)
                    || profile.scope == ProfileScope::Shared
                    || (profile.scope == ProfileScope::Device
                        && profile.owner_device_uuid == caller_uuid)
            })
            .cloned()
            .collect()
    }

    /// Lists input deck summaries visible to `caller_uuid`.
    pub fn visible_input_decks(
        &self,
        caller_uuid: &str,
        all_devices: bool,
        host_control: bool,
    ) -> Vec<InputDeckSummary> {
        self.input_decks
            .values()
            .filter(|deck| {
                (all_devices && host_control)
                    || deck.scope == ProfileScope::Shared
                    || (deck.scope == ProfileScope::Device && deck.owner_device_uuid == caller_uuid)
            })
            .cloned()
            .collect()
    }

    /// Creates or updates one profile with ownership + quota + `If-Match` CAS checks.
    pub fn put_profile(
        &mut self,
        id: &str,
        caller_uuid: &str,
        host_control: bool,
        expected_rev: u64,
        write: ProfileWrite,
        now_unix_ms: u64,
    ) -> Result<PutOutcome, HubError> {
        validate_profile_write(id, &write)?;

        if let Some(existing) = self.profiles.get(id) {
            let owns_existing =
                existing.scope == ProfileScope::Device && existing.owner_device_uuid == caller_uuid;
            if !owns_existing && !host_control {
                return Err(HubError::HostControlRequired(
                    "Only this device's profiles can be changed without Host control.".into(),
                ));
            }
            if write.scope != existing.scope && !host_control {
                return Err(HubError::HostControlRequired(
                    "Changing a profile's sharing scope requires Host control.".into(),
                ));
            }
            let target_owner = resolve_target_owner(
                caller_uuid,
                &write,
                Some((existing.scope, existing.owner_device_uuid.as_str())),
                host_control,
            )?;
            if write.scope == ProfileScope::Device
                && (existing.scope != ProfileScope::Device
                    || target_owner != existing.owner_device_uuid)
            {
                self.check_device_quota(&target_owner, Some(id))?;
            }
            if expected_rev != existing.revision {
                return Err(HubError::RevisionConflict {
                    profile_id: id.to_string(),
                    current_revision: existing.revision,
                });
            }
            let next_rev = existing.revision.saturating_add(1);
            let mut updated =
                build_profile(id, next_rev, &write, target_owner, caller_uuid, now_unix_ms);
            updated.name = write.name.trim().to_string();
            self.snapshot_revision = self.snapshot_revision.saturating_add(1);
            self.profiles.insert(id.to_string(), updated.clone());
            return Ok(PutOutcome {
                created: false,
                profile: updated,
                snapshot_revision: self.snapshot_revision,
            });
        }

        let target_owner = resolve_target_owner(caller_uuid, &write, None, host_control)?;
        if expected_rev != 0 {
            return Err(HubError::RevisionConflict {
                profile_id: id.to_string(),
                current_revision: 0,
            });
        }
        if self.profiles.len() >= MAX_PROFILES_HOST {
            return Err(HubError::InvalidValue {
                setting: None,
                message: format!(
                    "This PC already holds the maximum of {MAX_PROFILES_HOST} synced profiles."
                ),
            });
        }
        if write.scope == ProfileScope::Device {
            self.check_device_quota(&target_owner, None)?;
        }
        let mut created = build_profile(id, 1, &write, target_owner, caller_uuid, now_unix_ms);
        created.name = write.name.trim().to_string();
        self.snapshot_revision = self.snapshot_revision.saturating_add(1);
        self.profiles.insert(id.to_string(), created.clone());
        Ok(PutOutcome {
            created: true,
            profile: created,
            snapshot_revision: self.snapshot_revision,
        })
    }

    /// Deletes one profile with ownership and `If-Match` CAS checks.
    pub fn delete_profile(
        &mut self,
        id: &str,
        caller_uuid: &str,
        host_control: bool,
        expected_rev: u64,
    ) -> Result<ProfileDeleteResult, HubError> {
        validate_id(id)?;
        let Some(existing) = self.profiles.get(id) else {
            return Err(HubError::NotFound(
                "No synced profile with that ID exists on this PC.".into(),
            ));
        };
        let owns_existing =
            existing.scope == ProfileScope::Device && existing.owner_device_uuid == caller_uuid;
        if !owns_existing && !host_control {
            return Err(HubError::HostControlRequired(
                "Only this device's profiles can be deleted without Host control.".into(),
            ));
        }
        if expected_rev != existing.revision {
            return Err(HubError::RevisionConflict {
                profile_id: id.to_string(),
                current_revision: existing.revision,
            });
        }
        self.profiles.remove(id);
        self.snapshot_revision = self.snapshot_revision.saturating_add(1);
        Ok(ProfileDeleteResult {
            deleted: true,
            snapshot_revision: self.snapshot_revision,
        })
    }

    /// Updates persisted cover digest index and bumps `snapshot_revision` only when digests change.
    pub fn record_cover_digests(&mut self, digests: BTreeMap<u32, String>) -> bool {
        if self.cover_digests == digests {
            return false;
        }
        self.cover_digests = digests;
        self.snapshot_revision = self.snapshot_revision.saturating_add(1);
        true
    }

    fn check_device_quota(&self, owner: &str, excluding_id: Option<&str>) -> Result<(), HubError> {
        let device_count = self
            .profiles
            .iter()
            .filter(|(pid, p)| {
                excluding_id != Some(pid.as_str())
                    && p.scope == ProfileScope::Device
                    && p.owner_device_uuid == owner
            })
            .count();
        if device_count >= MAX_PROFILES_PER_DEVICE {
            return Err(HubError::InvalidValue {
                setting: None,
                message: format!(
                    "This device already holds the maximum of {MAX_PROFILES_PER_DEVICE} synced profiles."
                ),
            });
        }
        Ok(())
    }
}

fn resolve_target_owner(
    caller_uuid: &str,
    write: &ProfileWrite,
    existing: Option<(ProfileScope, &str)>,
    host_control: bool,
) -> Result<String, HubError> {
    if write.scope == ProfileScope::Shared {
        if !host_control {
            return Err(HubError::HostControlRequired(
                "Creating or editing shared profiles requires Host control.".into(),
            ));
        }
        let explicit = write
            .owner_device_uuid
            .as_deref()
            .map(state::device_uuid)
            .filter(|s| !s.is_empty())
            .unwrap_or("shared");
        return Ok(explicit.to_string());
    }
    let default_owner = match existing {
        Some((ProfileScope::Device, owner)) if !owner.is_empty() => owner,
        _ => caller_uuid,
    };
    let requested = write
        .owner_device_uuid
        .as_deref()
        .map(state::device_uuid)
        .filter(|s| !s.is_empty())
        .unwrap_or(default_owner);
    if requested != caller_uuid && !host_control {
        return Err(HubError::HostControlRequired(
            "Managing another device's profiles requires Host control.".into(),
        ));
    }
    Ok(requested.to_string())
}

fn build_profile(
    id: &str,
    revision: u64,
    write: &ProfileWrite,
    owner_device_uuid: String,
    caller_uuid: &str,
    now_unix_ms: u64,
) -> SyncProfile {
    SyncProfile {
        id: id.to_string(),
        revision,
        scope: write.scope,
        owner_device_uuid,
        name: write.name.clone(),
        network_matcher: write.network_matcher.clone(),
        stream: write.stream.clone(),
        input_deck_id: write.input_deck_id.clone(),
        updated_unix_ms: now_unix_ms,
        updated_by_device: caller_uuid.to_string(),
    }
}

fn collect_override_keys(block: &serde_json::Map<String, Value>, out: &mut BTreeSet<String>) {
    if let Some(config_map) = block
        .get(device_overrides::CONFIG)
        .and_then(Value::as_object)
    {
        for (key, value) in config_map {
            if !value.is_null() && value.as_str() != Some("") {
                out.insert(key.clone());
            }
        }
    }
    if block
        .get(radeon_profile::PROFILE)
        .is_some_and(|v| !v.is_null() && v.as_str() != Some(""))
    {
        out.insert("radeon_profile".into());
    }
    if block
        .get(radeon_profile::SETTINGS)
        .and_then(Value::as_object)
        .is_some_and(|m| !m.is_empty())
    {
        out.insert("radeon_settings".into());
    }
    if block
        .get(crate::game_files::SETTING)
        .is_some_and(|v| match v {
            Value::Array(items) => !items.is_empty(),
            Value::Object(map) => !map.is_empty(),
            _ => false,
        })
    {
        out.insert("game_files".into());
    }
}

/// Projects effective host/app/device overrides for `client` without exposing raw host configs.
pub fn host_overrides_summary(apps: &[App], client: &Client) -> Vec<HostOverrideSummary> {
    let dev_uuid = state::device_uuid(&client.uuid);
    let mut out = Vec::new();
    for app in apps {
        let dev_block = app
            .extra
            .get(device_overrides::SETTING)
            .and_then(|v| v.get(dev_uuid))
            .and_then(Value::as_object);
        let mut dev_keys = BTreeSet::new();
        if let Some(block) = dev_block {
            collect_override_keys(block, &mut dev_keys);
        }
        if !dev_keys.is_empty() {
            out.push(HostOverrideSummary {
                app_id: app.id(),
                source: "app_device".into(),
                locked_keys: dev_keys.iter().cloned().collect(),
            });
        }
        let mut app_keys = BTreeSet::new();
        let app_map: serde_json::Map<String, Value> = app
            .extra
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        collect_override_keys(&app_map, &mut app_keys);
        let clears_radeon = dev_keys.contains("radeon_profile")
            || dev_block
                .and_then(|b| b.get(radeon_profile::PROFILE))
                .is_some_and(|v| v.as_str() == Some(""));
        if clears_radeon {
            app_keys.remove("radeon_profile");
            app_keys.remove("radeon_settings");
        }
        app_keys.retain(|k| !dev_keys.contains(k));
        if !app_keys.is_empty() {
            out.push(HostOverrideSummary {
                app_id: app.id(),
                source: "app".into(),
                locked_keys: app_keys.into_iter().collect(),
            });
        }
    }
    if let Some(device_map) = client
        .extra
        .get("config_overrides")
        .and_then(Value::as_object)
    {
        let keys: BTreeSet<String> = device_map
            .iter()
            .filter(|(_, v)| !v.is_null() && v.as_str() != Some(""))
            .map(|(k, _)| k.clone())
            .collect();
        if !keys.is_empty() {
            out.push(HostOverrideSummary {
                app_id: 0,
                source: "device".into(),
                locked_keys: keys.into_iter().collect(),
            });
        }
    }
    out
}

#[cfg(test)]
#[path = "hub_sync/tests.rs"]
mod tests;
