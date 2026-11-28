//! Persistent Host Hub Sync store (`hub-sync.json`), corrupt-file recovery,
//! and lazy SHA-256 cover art hashing keyed by `(mtime, len)`.
use butterpollo_session::{
    catalog,
    hub_sync::{HubError, HubState, PutOutcome, host_overrides_summary, normalized_device_uuid},
    native,
    state::{self, App, Client},
};
use rubylight_native_api::hub::{
    CoverEntry, HostOverrideSummary, InputDeckSummary, ProfileDeleteResult, ProfileWrite,
    SyncProfile,
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::{SystemTime, UNIX_EPOCH},
};

/// Durable store filename inside the host configuration directory.
pub const FILE: &str = "hub-sync.json";
/// Host warning code emitted when `hub-sync.json` was unreadable and backed up.
pub const CORRUPT_WARNING: &str = "hub_sync_corrupt";

#[derive(Clone, Debug)]
struct CachedCover {
    mtime_ns: u128,
    len: u64,
    etag: String,
    width: u32,
    height: u32,
    mime: String,
    updated_unix_ms: u64,
}

/// Snapshot projection computed under lock.
#[derive(Clone, Debug)]
pub struct HubProjection {
    pub snapshot_revision: u64,
    pub generated_unix_ms: u64,
    pub caller_device_uuid: String,
    pub covers: Vec<CoverEntry>,
    pub profiles: Vec<SyncProfile>,
    pub input_decks: Vec<InputDeckSummary>,
    pub host_overrides_summary: Vec<HostOverrideSummary>,
}

/// Thread-safe, crash-resilient host store for synced profiles and cover digests.
#[derive(Debug)]
pub struct HubStore {
    path: PathBuf,
    state: Mutex<HubState>,
    cover_cache: Mutex<BTreeMap<PathBuf, CachedCover>>,
}

impl HubStore {
    /// Loads `hub-sync.json` from `directory`, backing up corrupt JSON and warning visibly.
    pub fn load(directory: &Path, warnings: &butterpollo_session::session::Warnings) -> Arc<Self> {
        let path = directory.join(FILE);
        let state = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<HubState>(&text) {
                Ok(mut parsed) => {
                    if parsed.snapshot_revision == 0 {
                        parsed.snapshot_revision = 1;
                    }
                    parsed
                }
                Err(error) => {
                    tracing::warn!(
                        path = %path.display(),
                        %error,
                        "hub-sync.json could not be parsed; backing up and resetting to revision 1"
                    );
                    backup_corrupt(&path);
                    crate::host_warnings::notice(
                        directory,
                        warnings,
                        CORRUPT_WARNING,
                        "Synced device profiles could not be read and were backed up.",
                    );
                    HubState::default()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => HubState::default(),
            Err(error) => {
                tracing::warn!(
                    path = %path.display(),
                    %error,
                    "hub-sync.json could not be read; starting with clean state"
                );
                HubState::default()
            }
        };
        Arc::new(Self {
            path,
            state: Mutex::new(state),
            cover_cache: Mutex::new(BTreeMap::new()),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Recomputes changed cover digests lazily via `(mtime, len)` and builds a snapshot view.
    pub fn project(
        &self,
        apps: &[App],
        assets: &Path,
        client: &Client,
        all_devices: bool,
        host_control: bool,
    ) -> anyhow::Result<HubProjection> {
        let mut seen_paths = BTreeSet::new();
        let mut covers = Vec::with_capacity(apps.len());
        let mut digests = BTreeMap::new();

        for app in apps {
            let app_id = app.id();
            let path = catalog::artwork(app, assets);
            if path == assets.join("box.png") {
                continue;
            }
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            seen_paths.insert(path.clone());
            let Some(cached) = self.cached_or_hash_cover(&path, &meta) else {
                continue;
            };
            digests.insert(app_id, cached.etag.clone());
            covers.push(CoverEntry {
                app_id,
                app_uuid: native::install_id(app),
                etag: cached.etag,
                bytes: cached.len,
                width: cached.width,
                height: cached.height,
                mime: cached.mime,
                updated_unix_ms: cached.updated_unix_ms,
            });
        }
        self.cover_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|p, _| seen_paths.contains(p));

        let caller_uuid = normalized_device_uuid(client).to_string();
        let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.cover_digests != digests {
            let mut next = guard.clone();
            next.record_cover_digests(digests);
            state::write_json(&self.path, &next)?;
            *guard = next;
        }
        let snapshot_revision = guard.snapshot_revision;
        let profiles = guard.visible_profiles(&caller_uuid, all_devices, host_control);
        let input_decks = guard.visible_input_decks(&caller_uuid, all_devices, host_control);
        drop(guard);

        let overrides = host_overrides_summary(apps, client);
        Ok(HubProjection {
            snapshot_revision,
            generated_unix_ms: now_unix_ms(),
            caller_device_uuid: caller_uuid,
            covers,
            profiles,
            input_decks,
            host_overrides_summary: overrides,
        })
    }

    /// Creates or updates one profile with CAS revision checking and atomic disk persistence.
    pub fn put_profile(
        &self,
        id: &str,
        client: &Client,
        host_control: bool,
        expected_rev: u64,
        write: ProfileWrite,
    ) -> Result<Result<PutOutcome, HubError>, anyhow::Error> {
        let caller_uuid = normalized_device_uuid(client).to_string();
        let now = now_unix_ms();
        let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut next = guard.clone();
        let outcome =
            match next.put_profile(id, &caller_uuid, host_control, expected_rev, write, now) {
                Ok(outcome) => outcome,
                Err(hub_err) => return Ok(Err(hub_err)),
            };
        state::write_json(&self.path, &next)?;
        *guard = next;
        Ok(Ok(outcome))
    }

    /// Deletes one profile with CAS revision checking and atomic disk persistence.
    pub fn delete_profile(
        &self,
        id: &str,
        client: &Client,
        host_control: bool,
        expected_rev: u64,
    ) -> Result<Result<ProfileDeleteResult, HubError>, anyhow::Error> {
        let caller_uuid = normalized_device_uuid(client).to_string();
        let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut next = guard.clone();
        let result = match next.delete_profile(id, &caller_uuid, host_control, expected_rev) {
            Ok(res) => res,
            Err(hub_err) => return Ok(Err(hub_err)),
        };
        state::write_json(&self.path, &next)?;
        *guard = next;
        Ok(Ok(result))
    }

    /// Checks ETag match without reading full bytes when `(mtime, len)` hits cache, or loads bytes.
    pub fn read_cover(
        &self,
        apps: &[App],
        assets: &Path,
        app_id: u32,
        if_none_match: Option<&str>,
    ) -> Option<CoverRead> {
        let app = apps
            .iter()
            .find(|a| a.id() == app_id || a.aliases.contains(&app_id))?;
        let path = catalog::artwork(app, assets);
        if path == assets.join("box.png") {
            return None;
        }
        let meta = std::fs::metadata(&path).ok()?;
        if !meta.is_file() {
            return None;
        }
        let mtime_ns = modified_unix_ns(&meta);
        let len = meta.len();
        if let Some(inm) = if_none_match {
            let guard = self
                .cover_cache
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(cached) = guard.get(&path)
                && cached.mtime_ns == mtime_ns
                && cached.len == len
                && butterpollo_session::hub_sync::etag_matches(inm, &cached.etag)
            {
                return Some(CoverRead::NotModified {
                    etag: cached.etag.clone(),
                });
            }
        }
        let bytes = std::fs::read(&path).ok()?;
        let cached = build_cached_cover(mtime_ns, &bytes);
        let etag = cached.etag.clone();
        let mime = cached.mime.clone();
        self.cover_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(path, cached);
        if let Some(inm) = if_none_match
            && butterpollo_session::hub_sync::etag_matches(inm, &etag)
        {
            return Some(CoverRead::NotModified { etag });
        }
        Some(CoverRead::Body { etag, mime, bytes })
    }

    fn cached_or_hash_cover(&self, path: &Path, meta: &std::fs::Metadata) -> Option<CachedCover> {
        let mtime_ns = modified_unix_ns(meta);
        let len = meta.len();
        {
            let guard = self
                .cover_cache
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(hit) = guard.get(path)
                && hit.mtime_ns == mtime_ns
                && hit.len == len
            {
                return Some(hit.clone());
            }
        }
        let bytes = std::fs::read(path).ok()?;
        let cached = build_cached_cover(mtime_ns, &bytes);
        self.cover_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(path.to_path_buf(), cached.clone());
        Some(cached)
    }
}

/// Result of reading a cover blob for `GET /hub/covers/{app_id}`.
pub enum CoverRead {
    NotModified {
        etag: String,
    },
    Body {
        etag: String,
        mime: String,
        bytes: Vec<u8>,
    },
}

fn build_cached_cover(mtime_ns: u128, bytes: &[u8]) -> CachedCover {
    let digest = Sha256::digest(bytes);
    let etag = format!("sha256:{}", hex::encode(digest));
    let (mime, width, height) = inspect_image(bytes);
    CachedCover {
        mtime_ns,
        len: bytes.len() as u64,
        etag,
        width,
        height,
        mime: mime.to_string(),
        updated_unix_ms: (mtime_ns / 1_000_000) as u64,
    }
}

fn modified_unix_ns(meta: &std::fs::Metadata) -> u128 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos())
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn backup_corrupt(path: &Path) {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("hub-sync.json");
    let mut backup = path.with_file_name(format!("{file_name}.corrupt-{ts}"));
    let mut n = 1u32;
    while backup.exists() {
        backup = path.with_file_name(format!("{file_name}.corrupt-{ts}-{n}"));
        n += 1;
    }
    let _ = std::fs::rename(path, backup);
}

fn inspect_image(bytes: &[u8]) -> (&'static str, u32, u32) {
    if bytes.len() >= 24 && bytes.starts_with(b"\x89PNG\r\n\x1a\n") && &bytes[12..16] == b"IHDR" {
        let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        return ("image/png", w, h);
    }
    if bytes.len() >= 30 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        let (w, h) = parse_webp_dimensions(bytes);
        return ("image/webp", w, h);
    }
    if bytes.len() >= 4 && bytes[0] == 0xFF && bytes[1] == 0xD8 {
        let (w, h) = parse_jpeg_dimensions(bytes);
        return ("image/jpeg", w, h);
    }
    ("application/octet-stream", 0, 0)
}

fn parse_webp_dimensions(bytes: &[u8]) -> (u32, u32) {
    let chunk = &bytes[12..16];
    if chunk == b"VP8 " && bytes.len() >= 30 && bytes[23..26] == [0x9d, 0x01, 0x2a] {
        let w = u32::from(u16::from_le_bytes([bytes[26], bytes[27]]) & 0x3FFF);
        let h = u32::from(u16::from_le_bytes([bytes[28], bytes[29]]) & 0x3FFF);
        return (w, h);
    }
    if chunk == b"VP8L" && bytes.len() >= 25 && bytes[20] == 0x2F {
        let bits = u32::from_le_bytes([bytes[21], bytes[22], bytes[23], bytes[24]]);
        let w = (bits & 0x3FFF) + 1;
        let h = ((bits >> 14) & 0x3FFF) + 1;
        return (w, h);
    }
    if chunk == b"VP8X" && bytes.len() >= 30 {
        let w =
            (u32::from(bytes[24]) | (u32::from(bytes[25]) << 8) | (u32::from(bytes[26]) << 16)) + 1;
        let h =
            (u32::from(bytes[27]) | (u32::from(bytes[28]) << 8) | (u32::from(bytes[29]) << 16)) + 1;
        return (w, h);
    }
    (0, 0)
}

fn parse_jpeg_dimensions(bytes: &[u8]) -> (u32, u32) {
    let mut i = 2usize;
    while i + 3 < bytes.len() {
        if bytes[i] != 0xFF {
            i += 1;
            continue;
        }
        while i < bytes.len() && bytes[i] == 0xFF {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let marker = bytes[i];
        i += 1;
        if marker == 0xD8 || marker == 0xD9 || marker == 0x00 || (0xD0..=0xD7).contains(&marker) {
            continue;
        }
        if i + 1 >= bytes.len() {
            break;
        }
        let seg_len = usize::from(u16::from_be_bytes([bytes[i], bytes[i + 1]]));
        if seg_len < 2 || i + seg_len > bytes.len() {
            break;
        }
        let is_sof = matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF);
        if is_sof && seg_len >= 7 {
            let h = u32::from(u16::from_be_bytes([bytes[i + 3], bytes[i + 4]]));
            let w = u32::from(u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]));
            return (w, h);
        }
        i += seg_len;
    }
    (0, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rubylight_native_api::hub::{ProfileScope, StreamProfileSpec};

    fn dummy_client(uuid: &str) -> Client {
        Client {
            name: "Dev".into(),
            cert: String::new(),
            uuid: uuid.into(),
            perm: 0,
            enabled: true,
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn corrupt_hub_sync_is_backed_up_and_warns_cleanly() {
        let dir =
            std::env::temp_dir().join(format!("butterpollo-hub-corrupt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE), "{not valid json").unwrap();
        let warnings = butterpollo_session::session::Warnings::default();
        let store = HubStore::load(&dir, &warnings);
        let proj = store
            .project(&[], &dir, &dummy_client("dev-1"), false, false)
            .unwrap();
        assert_eq!(proj.snapshot_revision, 1);
        assert!(
            warnings
                .snapshot()
                .iter()
                .any(|w| w.code == CORRUPT_WARNING)
        );
        let backups: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains("corrupt-"))
            .collect();
        assert_eq!(backups.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn persists_across_reload_and_caches_unchanged_covers() {
        let dir =
            std::env::temp_dir().join(format!("butterpollo-hub-persist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let warnings = butterpollo_session::session::Warnings::default();
        let cover_path = dir.join("game.png");
        let mut png = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&600u32.to_be_bytes());
        png.extend_from_slice(&900u32.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        std::fs::write(&cover_path, &png).unwrap();

        let mut extra = BTreeMap::new();
        extra.insert(
            "image-path".into(),
            serde_json::Value::String(cover_path.to_string_lossy().into_owned()),
        );
        let app = App {
            name: "Game".into(),
            cmd: String::new(),
            working_dir: String::new(),
            prep: Vec::new(),
            computed_id: Some(1042),
            aliases: Vec::new(),
            extra,
        };
        let apps = vec![app];
        let client = dummy_client("dev-1");

        let store = HubStore::load(&dir, &warnings);
        let put = store
            .put_profile(
                "couch",
                &client,
                false,
                0,
                ProfileWrite {
                    scope: ProfileScope::Device,
                    owner_device_uuid: None,
                    name: "Couch".into(),
                    network_matcher: None,
                    stream: StreamProfileSpec::default(),
                    input_deck_id: None,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(put.profile.revision, 1);

        let p1 = store.project(&apps, &dir, &client, false, false).unwrap();
        assert_eq!(p1.covers.len(), 1);
        assert_eq!((p1.covers[0].width, p1.covers[0].height), (600, 900));
        assert_eq!(p1.covers[0].mime, "image/png");
        let rev1 = p1.snapshot_revision;

        // Second call hits cover cache and keeps exact snapshot_revision.
        let p2 = store.project(&apps, &dir, &client, false, false).unwrap();
        assert_eq!(p2.snapshot_revision, rev1);

        // Reload from disk preserves profile & snapshot_revision.
        let reloaded = HubStore::load(&dir, &warnings);
        let p3 = reloaded
            .project(&apps, &dir, &client, false, false)
            .unwrap();
        assert_eq!(p3.snapshot_revision, rev1);
        assert_eq!(p3.profiles.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
