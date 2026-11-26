//! Stream DVR's recovery when the host starts: what a crash left in the clips folder.
//!
//! A `<id>.mp4.tmp` whose sidecar was already renamed into place was written and synced in
//! full, so it becomes `<id>.mp4`. Any other `.mp4.tmp` is a clip or recording cut short: its
//! complete fragments are kept as `<id>-recovered.mp4` (the fragmented MP4 is flushed every
//! fragment), with the draft sidecar's game and the salvaged length, and a file without one
//! complete fragment is deleted so torn bytes never count against the quota.
use super::{draft_path, sidecar_path, temp_path, video_path, write_sidecar};
use butterpollo_media_policy::dvr::{quota, recover::salvage};
use rubylight_native_api::dvr::{ClipKind, ClipSummary};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::time::SystemTime;

/// What [`recover`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Recovered {
    /// Complete clips renamed into place.
    pub completed: u32,
    /// Cut-short files kept as `-recovered` clips.
    pub salvaged: u32,
    /// Torn files deleted.
    pub deleted: u32,
}

/// Resolves every `.tmp` file in `dir`; see the module documentation.
pub fn recover(dir: &Path) -> Recovered {
    recover_before(dir, SystemTime::now())
}

/// [`recover`] for the files last written before `cutoff`: a stream that started since writes
/// its own.
pub fn recover_before(dir: &Path, cutoff: SystemTime) -> Recovered {
    let old = |path: &Path| {
        std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .is_ok_and(|modified| modified < cutoff)
    };
    let mut done = Recovered::default();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return done;
    };
    let names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    for name in &names {
        if name.starts_with(".write-check-") {
            let _ = std::fs::remove_file(dir.join(name));
        }
        let Some(id) = name.strip_suffix(".mp4.tmp") else {
            continue;
        };
        if !quota::valid_clip_id(id) || !old(&temp_path(dir, id)) {
            continue;
        }
        if sidecar_path(dir, id).exists() {
            match std::fs::rename(temp_path(dir, id), video_path(dir, id)) {
                Ok(()) => done.completed += 1,
                Err(error) => {
                    tracing::warn!(clip = id, %error, "stream dvr: a complete clip could not be renamed")
                }
            }
        } else if salvage_one(dir, id) {
            done.salvaged += 1;
        } else {
            let _ = std::fs::remove_file(temp_path(dir, id));
            done.deleted += 1;
        }
        let _ = std::fs::remove_file(draft_path(dir, id));
    }
    // Drafts without a video left (their recording never wrote a byte).
    for name in &names {
        if let Some(id) = name.strip_suffix(".json.tmp")
            && quota::valid_clip_id(id)
            && !temp_path(dir, id).exists()
            && old(&draft_path(dir, id))
        {
            let _ = std::fs::remove_file(draft_path(dir, id));
        }
    }
    if done != Recovered::default() {
        tracing::info!(
            completed = done.completed,
            salvaged = done.salvaged,
            deleted = done.deleted,
            "stream dvr: clips left by an unexpected stop resolved"
        );
    }
    done
}

/// Keeps the complete fragments of `<id>.mp4.tmp` as `<id>-recovered.mp4`; `false` when none.
fn salvage_one(dir: &Path, id: &str) -> bool {
    let recovered = format!("{id}-recovered");
    if !quota::valid_clip_id(&recovered) {
        return false;
    }
    let path = temp_path(dir, id);
    let result = (|| -> std::io::Result<Option<u64>> {
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)?;
        let found = salvage(&mut file)?;
        if found.fragments == 0 {
            return Ok(None);
        }
        file.set_len(found.valid_len)?;
        if let Some((offset, bytes)) = &found.mehd_patch {
            file.seek(SeekFrom::Start(*offset))?;
            file.write_all(bytes)?;
        }
        file.sync_all()?;
        Ok(Some(found.duration_us))
    })();
    let duration_us = match result {
        Ok(Some(duration_us)) => duration_us,
        Ok(None) => return false,
        Err(error) => {
            tracing::warn!(clip = id, %error, "stream dvr: a cut-short clip could not be read");
            return false;
        }
    };
    let mut summary = std::fs::read(draft_path(dir, id))
        .ok()
        .and_then(|text| serde_json::from_slice::<ClipSummary>(&text).ok())
        .unwrap_or_else(|| ClipSummary {
            app_title: "Unknown".into(),
            container: "mp4".into(),
            kind: ClipKind::Manual,
            ..Default::default()
        });
    summary.id = recovered.clone();
    summary.recovered = true;
    summary.duration_ms = duration_us / 1000;
    summary.size_bytes = std::fs::metadata(&path).map_or(0, |m| m.len());
    if let Err(error) = std::fs::rename(&path, video_path(dir, &recovered))
        .and_then(|()| write_sidecar(dir, &summary))
    {
        tracing::warn!(clip = id, %error, "stream dvr: a cut-short clip could not be kept");
        return false;
    }
    tracing::info!(clip = %recovered, duration_ms = summary.duration_ms, "stream dvr: kept the complete part of a cut-short clip");
    true
}
