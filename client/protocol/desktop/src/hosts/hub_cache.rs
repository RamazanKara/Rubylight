//! Offline disk cache for Host Hub Sync (`<store_dir>/hub/<host_id>/{snapshot,outbox,cover_etags}.json`)
//! and ETag-backed cover art writes into `Store::art_path`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use nightjar_core::http::native_api::hub::HubSnapshot;
use nightjar_policy::hub_sync::OutboxOp;

use crate::config::Store;

/// Loaded per-host offline hub state from disk.
#[derive(Clone, Debug, Default)]
pub struct CachedHub {
    pub snapshot: Option<HubSnapshot>,
    pub outbox: Vec<OutboxOp>,
    pub cover_etags: BTreeMap<u32, String>,
}

fn host_dir(store_dir: &Path, unique_id: &str) -> PathBuf {
    let safe: String = unique_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let folder = if safe.is_empty() { "unknown" } else { &safe };
    store_dir.join("hub").join(folder)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(_) => {
            let _ = std::fs::remove_file(path);
            let res = std::fs::rename(&tmp, path);
            if res.is_err() {
                let _ = std::fs::remove_file(&tmp);
            }
            res
        }
    }
}

fn write_json<T: serde::Serialize + ?Sized>(path: &Path, value: &T) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    atomic_write(path, &bytes)
}

pub fn load_host_cache(store_dir: &Path, unique_id: &str) -> CachedHub {
    let dir = host_dir(store_dir, unique_id);
    let snapshot = std::fs::read(dir.join("snapshot.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<HubSnapshot>(&b).ok());
    let outbox = std::fs::read(dir.join("outbox.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Vec<OutboxOp>>(&b).ok())
        .unwrap_or_default();
    let cover_etags = std::fs::read(dir.join("cover_etags.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<BTreeMap<u32, String>>(&b).ok())
        .unwrap_or_default();
    CachedHub {
        snapshot,
        outbox,
        cover_etags,
    }
}

pub fn save_snapshot(
    store_dir: &Path,
    unique_id: &str,
    snapshot: Option<&HubSnapshot>,
) -> std::io::Result<()> {
    let path = host_dir(store_dir, unique_id).join("snapshot.json");
    match snapshot {
        Some(snap) => write_json(&path, snap),
        None => match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        },
    }
}

pub fn save_outbox(store_dir: &Path, unique_id: &str, outbox: &[OutboxOp]) -> std::io::Result<()> {
    let path = host_dir(store_dir, unique_id).join("outbox.json");
    if outbox.is_empty() {
        return match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        };
    }
    write_json(&path, outbox)
}

pub fn save_cover_etags(
    store_dir: &Path,
    unique_id: &str,
    etags: &BTreeMap<u32, String>,
) -> std::io::Result<()> {
    let path = host_dir(store_dir, unique_id).join("cover_etags.json");
    write_json(&path, etags)
}

/// Validates and writes downloaded cover bytes atomically to `store.art_path` for `covers.rs`.
pub fn save_cover_to_art(
    store_dir: &Path,
    unique_id: &str,
    app_id: u32,
    cover_version: &str,
    bytes: &[u8],
) -> std::io::Result<bool> {
    if bytes.is_empty() || nightjar_core::http::is_placeholder_box_art(bytes) {
        return Ok(false);
    }
    let store = Store::at(store_dir.to_path_buf())?;
    let app = app_id as i32;
    let target = store.art_path(unique_id, app, cover_version);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = target.with_extension("img.tmp");
    std::fs::write(&tmp, bytes)?;
    if std::fs::rename(&tmp, &target).is_err() {
        let _ = std::fs::remove_file(&target);
        std::fs::rename(&tmp, &target)?;
    }
    if !cover_version.is_empty() {
        let bare = store.art_path(unique_id, app, "");
        if !bare.exists() {
            let _ = std::fs::write(bare, bytes);
        }
    }
    Ok(true)
}

/// Removes persisted Hub Sync state when a PC is removed from the client.
pub fn clear_host_cache(store_dir: &Path, unique_id: &str) {
    if unique_id.is_empty() {
        return;
    }
    let _ = std::fs::remove_dir_all(host_dir(store_dir, unique_id));
}

#[cfg(test)]
mod tests {
    use super::*;
    use nightjar_core::http::native_api::hub::{ProfileScope, ProfileWrite};

    #[test]
    fn round_trips_snapshot_outbox_cover_etags_and_art_file() {
        let dir = std::env::temp_dir().join(format!("nightjar-hub-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let snap = HubSnapshot {
            schema_version: 1,
            snapshot_revision: 9,
            host_unique_id: "uid-1".into(),
            ..Default::default()
        };
        let outbox = vec![OutboxOp::Put {
            id: "couch".into(),
            base_revision: 0,
            base: None,
            write: ProfileWrite {
                scope: ProfileScope::Device,
                name: "Couch".into(),
                ..Default::default()
            },
            queued_unix_ms: 42,
        }];
        let etags = BTreeMap::from([(1042u32, "sha256:abc".to_string())]);
        save_snapshot(&dir, "uid-1", Some(&snap)).unwrap();
        save_outbox(&dir, "uid-1", &outbox).unwrap();
        save_cover_etags(&dir, "uid-1", &etags).unwrap();

        let loaded = load_host_cache(&dir, "uid-1");
        assert_eq!(loaded.snapshot.unwrap().snapshot_revision, 9);
        assert_eq!(loaded.outbox.len(), 1);
        assert_eq!(
            loaded.cover_etags.get(&1042).map(String::as_str),
            Some("sha256:abc")
        );

        // Fake minimal PNG bytes write to Store::art_path and survive corrupt cleanup test.
        let mut png = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
        png.extend_from_slice(&[0; 20]);
        assert!(save_cover_to_art(&dir, "uid-1", 1042, "v9", &png).unwrap());
        let store = Store::at(dir.clone()).unwrap();
        assert_eq!(
            std::fs::read(store.art_path("uid-1", 1042, "v9")).unwrap(),
            png
        );

        clear_host_cache(&dir, "uid-1");
        assert!(load_host_cache(&dir, "uid-1").snapshot.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
