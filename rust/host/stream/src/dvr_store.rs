//! Stream DVR's clips on disk and the host's one running replay buffer.
//!
//! Clips live in `dvr_directory`, or `clips` beside the configuration file: `<id>.mp4` and a
//! `<id>.json` sidecar holding its [`ClipSummary`]. A clip is written as `<id>.mp4.tmp`, synced,
//! its sidecar renamed into place, and then renamed to `<id>.mp4`, so a crash leaves at most a
//! `.tmp` file, which [`recover`] resolves at start. Room is made before each write: the oldest
//! replay clips go first, recordings are never deleted for room, and the drive keeps
//! [`quota::DISK_FLOOR_BYTES`] free.
//!
//! One stream at a time has a replay buffer: the owner's plain stream that armed first
//! ([`DvrStore::register`]); the native API and the console reach it through
//! [`DvrStore::active`].
use butterpollo_config::config::Config;
use butterpollo_media_policy::dvr::quota::{self, StoredClip};
use rubylight_native_api::dvr::{ClipKind, ClipList, ClipSummary};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

mod recover;
pub use recover::{Recovered, recover, recover_before};

/// Clips one page of [`page`] lists unless asked for fewer.
pub const PAGE: usize = 50;
/// The most clips one page lists.
pub const MAX_PAGE: usize = 200;
const GIB: u64 = 1 << 30;

/// The host's one running replay buffer, by session id.
#[derive(Default)]
pub struct DvrStore {
    active: Mutex<Option<(String, Arc<crate::stream::dvr::Shared>)>>,
    /// Held while clips are deleted or made room for, so two writers do not count the same
    /// free space.
    disk: Mutex<()>,
}

impl DvrStore {
    /// Makes `dvr` the running replay buffer of `session`; `false` when another stream has one.
    pub fn register(&self, session: &str, dvr: Arc<crate::stream::dvr::Shared>) -> bool {
        let mut active = self.active.lock().unwrap_or_else(PoisonError::into_inner);
        if active.as_ref().is_some_and(|(id, _)| id != session) {
            return false;
        }
        *active = Some((session.to_owned(), dvr));
        true
    }

    /// Forgets `session`'s replay buffer, when it is the running one.
    pub fn unregister(&self, session: &str) {
        let mut active = self.active.lock().unwrap_or_else(PoisonError::into_inner);
        if active.as_ref().is_some_and(|(id, _)| id == session) {
            *active = None;
        }
    }

    /// The running replay buffer, if a stream has one.
    pub fn active(&self) -> Option<Arc<crate::stream::dvr::Shared>> {
        let active = self.active.lock().unwrap_or_else(PoisonError::into_inner);
        active.as_ref().map(|(_, dvr)| dvr.clone())
    }

    /// `session`'s replay buffer, when it has the running one.
    pub fn active_for(&self, session: &str) -> Option<Arc<crate::stream::dvr::Shared>> {
        let active = self.active.lock().unwrap_or_else(PoisonError::into_inner);
        active
            .as_ref()
            .filter(|(id, _)| id == session)
            .map(|(_, dvr)| dvr.clone())
    }

    /// Deletes what must go so `incoming` more bytes fit `limit` and the drive keeps its floor
    /// after `disk_incoming` more are written (a recording counts all its bytes against the
    /// quota but only its next fragment against the drive, whose free space already shows the
    /// rest); `open` names clips being written, which are never deleted.
    pub fn make_room(
        &self,
        dir: &Path,
        incoming: u64,
        disk_incoming: u64,
        limit: u64,
        open: &[String],
    ) -> Result<Vec<String>, quota::Refusal> {
        let _disk = self.disk.lock().unwrap_or_else(PoisonError::into_inner);
        let stored: Vec<StoredClip> = list(dir)
            .into_iter()
            .map(|clip| StoredClip {
                open: open.contains(&clip.id),
                id: clip.id,
                bytes: clip.size_bytes,
                created_unix_ms: clip.created_unix_ms,
                kind: match clip.kind {
                    ClipKind::Manual => quota::ClipKind::Manual,
                    _ => quota::ClipKind::Replay,
                },
            })
            .collect();
        // `plan` checks the floor against `incoming`; shift the free space so it is checked
        // against `disk_incoming`.
        let free = butterpollo_win_io::background_write::free_bytes(dir)
            .ok()
            .map(|free| free.saturating_add(incoming.saturating_sub(disk_incoming)));
        let evict = quota::plan(&stored, incoming, limit, free, quota::DISK_FLOOR_BYTES)?;
        for id in &evict {
            match delete(dir, id) {
                Ok(_) => {
                    tracing::info!(clip = %id, "stream dvr: oldest replay clip deleted for room")
                }
                Err(error) => {
                    tracing::warn!(clip = %id, %error, "stream dvr: clip could not be deleted for room")
                }
            }
        }
        Ok(evict)
    }

    /// Deletes clip `id`, holding the disk lock; `Ok(false)` when there is no such clip.
    pub fn delete(&self, dir: &Path, id: &str) -> io::Result<bool> {
        let _disk = self.disk.lock().unwrap_or_else(PoisonError::into_inner);
        delete(dir, id)
    }
}

/// Resolves what a crash left in the clips folder ([`recover`]) on a thread of its own, so the
/// host starts at once. A folder that does not exist yet is left alone.
pub fn recover_at_start(h: &crate::state::Shared) {
    let dir = {
        let config = h.config.read().unwrap_or_else(PoisonError::into_inner);
        clips_dir(&h.config_path, &config)
    };
    if !dir.is_dir() {
        return;
    }
    let started = SystemTime::now();
    let spawned = std::thread::Builder::new()
        .name("dvr-recover".into())
        .spawn(move || {
            butterpollo_win_io::background_write::lower_current_thread();
            recover_before(&dir, started);
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "stream dvr: the clips folder was not checked for unfinished clips");
    }
}

/// The clips folder: `dvr_directory`, or `clips` beside the configuration file.
pub fn clips_dir(config_path: &Path, config: &Config) -> PathBuf {
    config.dvr_directory().unwrap_or_else(|| {
        config_path
            .parent()
            .map_or_else(|| PathBuf::from("clips"), |dir| dir.join("clips"))
    })
}

/// The clips quota in bytes (`dvr_quota_gb`).
pub fn quota_bytes(config: &Config) -> u64 {
    u64::from(config.dvr_quota_gb()) * GIB
}

/// `dir` exists (it is made if missing) and takes files.
pub fn writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(format!(".write-check-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// The path of clip `id`'s video.
pub fn video_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.mp4"))
}

/// The path clip `id` is written to before it is complete.
pub fn temp_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.mp4.tmp"))
}

fn sidecar_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

/// The sidecar a recording keeps while it runs, so a recording cut short by a crash keeps its
/// game and size when [`recover`] salvages it.
pub fn draft_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json.tmp"))
}

/// A new clip id, `YYYYMMDD-HHMMSS-<kind>-<4 hex>` in the PC's local time.
pub fn new_id(kind: ClipKind, now: SystemTime) -> String {
    let unix = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let local = butterpollo_win_io::local_time::utc_to_local(unix).unwrap_or(unix as i64);
    let stamp = time::OffsetDateTime::from_unix_timestamp(local).map_or_else(
        |_| local.to_string(),
        |t| {
            format!(
                "{:04}{:02}{:02}-{:02}{:02}{:02}",
                t.year(),
                u8::from(t.month()),
                t.day(),
                t.hour(),
                t.minute(),
                t.second()
            )
        },
    );
    let kind = match kind {
        ClipKind::Manual => "manual",
        _ => "replay",
    };
    format!("{stamp}-{kind}-{:04x}", rand::random::<u16>())
}

/// Writes `summary` as clip `summary.id`'s sidecar: to a `.tmp` name, synced, then renamed.
pub fn write_sidecar(dir: &Path, summary: &ClipSummary) -> io::Result<()> {
    let json = serde_json::to_vec_pretty(summary).map_err(io::Error::other)?;
    let draft = draft_path(dir, &summary.id);
    write_synced(&draft, &json)?;
    std::fs::rename(&draft, sidecar_path(dir, &summary.id))
}

/// Writes a recording's draft sidecar (see [`draft_path`]).
pub fn write_draft(dir: &Path, summary: &ClipSummary) -> io::Result<()> {
    let json = serde_json::to_vec_pretty(summary).map_err(io::Error::other)?;
    write_synced(&draft_path(dir, &summary.id), &json)
}

fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Every saved clip with its sidecar, newest first. Sizes are the files' own.
pub fn list(dir: &Path) -> Vec<ClipSummary> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut clips: Vec<ClipSummary> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let id = name.strip_suffix(".json")?;
            if !quota::valid_clip_id(id) {
                return None;
            }
            let size = std::fs::metadata(video_path(dir, id)).ok()?.len();
            let text = std::fs::read(entry.path()).ok()?;
            let mut clip: ClipSummary = serde_json::from_slice(&text).ok()?;
            clip.id = id.to_owned();
            clip.size_bytes = size;
            Some(clip)
        })
        .collect();
    clips.sort_by(|a, b| {
        b.created_unix_ms
            .cmp(&a.created_unix_ms)
            .then_with(|| b.id.cmp(&a.id))
    });
    clips
}

/// The bytes the clips take.
pub fn used_bytes(clips: &[ClipSummary]) -> u64 {
    clips.iter().map(|clip| clip.size_bytes).sum()
}

/// One page of `clips` (newest first): `limit` (default [`PAGE`], at most [`MAX_PAGE`]) after
/// the clip named by `cursor`.
pub fn page(
    clips: Vec<ClipSummary>,
    limit: Option<usize>,
    cursor: Option<&str>,
    quota_limit_bytes: u64,
) -> ClipList {
    let quota_used_bytes = used_bytes(&clips);
    let start = cursor
        .and_then(|cursor| clips.iter().position(|clip| clip.id == cursor))
        .map_or(0, |at| at + 1);
    let limit = limit.unwrap_or(PAGE).clamp(1, MAX_PAGE);
    let page: Vec<ClipSummary> = clips.into_iter().skip(start).take(limit + 1).collect();
    let next_cursor = (page.len() > limit).then(|| page[limit - 1].id.clone());
    ClipList {
        clips: page.into_iter().take(limit).collect(),
        next_cursor,
        quota_used_bytes,
        quota_limit_bytes,
    }
}

/// Deletes clip `id`'s video and sidecar; `Ok(false)` when it has neither. Refuses an id that
/// is not a clip id, so no path outside the folder can be named.
pub fn delete(dir: &Path, id: &str) -> io::Result<bool> {
    if !quota::valid_clip_id(id) {
        return Ok(false);
    }
    let mut found = false;
    for path in [video_path(dir, id), sidecar_path(dir, id)] {
        match std::fs::remove_file(&path) {
            Ok(()) => found = true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(found)
}

/// Clip `id`'s summary, when it is saved.
pub fn find(dir: &Path, id: &str) -> Option<ClipSummary> {
    if !quota::valid_clip_id(id) {
        return None;
    }
    list(dir).into_iter().find(|clip| clip.id == id)
}

#[cfg(test)]
mod tests;
