//! Host Hub Sync endpoints (`/hub/snapshot`, `/hub/profiles/{id}`, `/hub/covers/{app_id}`),
//! while the host lists [`crate::feature::HUB_SYNC`]: cover SHA-256 digests, per-device and
//! shared stream profiles with optimistic revision concurrency (`If-Match`), input deck
//! metadata summaries, and effective host-override summaries.
use crate::capabilities::Permissions;
use serde::{Deserialize, Deserializer, Serialize};

/// Current schema version of [`HubSnapshot`].
pub const SNAPSHOT_SCHEMA_VERSION: u32 = 1;
/// Maximum accepted payload size for `PUT /hub/profiles/{id}`.
pub const MAX_PROFILE_BODY_BYTES: usize = 64 * 1024;
/// Response header returned on `409 Conflict` (`revision_conflict`) naming the server revision.
pub const HEADER_CURRENT_REVISION: &str = "X-Hub-Current-Revision";

/// Ownership scope of a synced profile or input deck.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileScope {
    #[default]
    Device,
    Shared,
    /// A scope from a newer host. Clients leave it read-only.
    #[serde(other)]
    Unknown,
}

/// Persistent monitor/display matcher carried in [`StreamProfileSpec`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayMatcher {
    pub id: String,
    pub name: String,
}

/// Network auto-selection matcher for a synced stream profile (`any`, `lan`, `wifi`, `cellular`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkMatcher {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssid: Option<String>,
}

fn deserialize_upscaling<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    Ok(value.map(|s| if s == "sgsr2" { "sgsr1".to_string() } else { s }))
}

/// Stream parameters carried by [`SyncProfile`] and [`ProfileWrite`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StreamProfileSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bitrate_kbps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hdr: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_layout: Option<String>,
    /// Allowed values on write: `"off"`, `"bilinear"`, `"fsr1"`, `"sgsr1"`, `"metalfx"`, or null.
    /// Legacy `"sgsr2"` reads as `"sgsr1"`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_upscaling"
    )]
    pub upscaling: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_scaling: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orientation: Option<String>,
    /// Per-display scale percentage (`50..=100`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_scale_pct: Option<u8>,
    /// Persistent monitor/display matcher for per-display recall.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_matcher: Option<DisplayMatcher>,
}

/// Capabilities summary embedded inside [`HubSnapshot`] for offline capability awareness.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HubCapabilitiesSnapshot {
    pub api_version: u32,
    pub features: Vec<String>,
    pub permissions: Permissions,
    pub codecs: Vec<String>,
    pub virtual_display: bool,
    pub hdr: bool,
}

/// Content-hashed cover entry in [`HubSnapshot::covers`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CoverEntry {
    pub app_id: u32,
    pub app_uuid: String,
    /// `sha256:<hex>` digest matching `ETag` of `GET /hub/covers/{app_id}`.
    pub etag: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    pub mime: String,
    pub updated_unix_ms: u64,
}

/// One synced stream profile stored on the host.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncProfile {
    pub id: String,
    pub revision: u64,
    pub scope: ProfileScope,
    pub owner_device_uuid: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_matcher: Option<NetworkMatcher>,
    pub stream: StreamProfileSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_deck_id: Option<String>,
    pub updated_unix_ms: u64,
    pub updated_by_device: String,
}

/// Metadata summary of a layered input deck stored on the host.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InputDeckSummary {
    pub id: String,
    pub layer_scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<u32>,
    pub revision: u64,
    pub scope: ProfileScope,
    pub owner_device_uuid: String,
    pub name: String,
    pub content_sha256: String,
    pub updated_unix_ms: u64,
}

/// Read-only summary of host/app/device overrides effective for the caller.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HostOverrideSummary {
    pub app_id: u32,
    /// Precedence tier: `"app_device"`, `"app"`, `"device"`, or `"host"`.
    pub source: String,
    pub locked_keys: Vec<String>,
}

/// `GET /api/native/v1/hub/snapshot`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HubSnapshot {
    pub schema_version: u32,
    pub snapshot_revision: u64,
    pub host_unique_id: String,
    pub generated_unix_ms: u64,
    pub caller_device_uuid: String,
    pub capabilities_snapshot: HubCapabilitiesSnapshot,
    pub covers: Vec<CoverEntry>,
    pub profiles: Vec<SyncProfile>,
    pub input_decks: Vec<InputDeckSummary>,
    pub host_overrides_summary: Vec<HostOverrideSummary>,
}

/// Request body of `PUT /api/native/v1/hub/profiles/{id}`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileWrite {
    pub scope: ProfileScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_device_uuid: Option<String>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_matcher: Option<NetworkMatcher>,
    #[serde(default)]
    pub stream: StreamProfileSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_deck_id: Option<String>,
}

/// Response body of `DELETE /api/native/v1/hub/profiles/{id}`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileDeleteResult {
    pub deleted: bool,
    pub snapshot_revision: u64,
}
