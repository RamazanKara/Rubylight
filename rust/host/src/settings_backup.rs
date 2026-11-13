//! Settings backup and restore from the console's Troubleshoot page and the
//! tray (docs/configuration.md, "Settings backup and restore"; the archive
//! and its checks are `butterpollo_core::settings_backup`).
//!
//! A restore runs in three steps: `inspect` checks an uploaded archive, keeps
//! it as `backups/pending-<id>.zip` and answers what it changes; `restore`
//! saves the current state, with its secrets, as `backups/before-restore-*`,
//! stages the pending archive and restarts the host; the next start applies
//! it before anything is loaded (`Host::load`).
use crate::state::Shared;
use anyhow::{Context, Result, bail};
use butterpollo_core::settings_backup as core;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const PENDING_PREFIX: &str = "pending-";
/// Before-restore archives kept; older ones are removed.
const KEEP_BEFORE_RESTORE: usize = 5;

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn fresh_secret() -> String {
    hex::encode(butterpollo_core::crypto::random::<32>())
}

/// Applies a staged restore at start; the message for the console when one
/// failed.
pub fn apply_staged(directory: &Path) -> std::result::Result<(), String> {
    match core::apply_staged(directory, fresh_secret) {
        Ok(true) => {
            tracing::info!("settings restored from a backup");
            Ok(())
        }
        Ok(false) => Ok(()),
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "the staged settings restore failed");
            Err(format!(
                "The settings restore failed and the previous settings were kept: {error:#}. The archive was kept as backups\\restore-failed.zip."
            ))
        }
    }
}

/// The archive's file name, with the date and time (UTC).
pub fn file_name(secrets: bool) -> String {
    let now = time::OffsetDateTime::now_utc();
    let stamp = format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    );
    if secrets {
        format!("rubylight-settings-{stamp}-with-secrets.zip")
    } else {
        format!("rubylight-settings-{stamp}.zip")
    }
}

/// The current settings as an archive in memory.
pub fn export(directory: &Path, secrets: bool) -> Result<Vec<u8>> {
    let mut out = std::io::Cursor::new(Vec::new());
    core::export(
        directory,
        secrets,
        env!("CARGO_PKG_VERSION"),
        now_unix(),
        &mut out,
    )?;
    Ok(out.into_inner())
}

fn backups(directory: &Path) -> PathBuf {
    directory.join(core::BACKUPS)
}

/// The pending archive an `inspect` answered with `token`.
fn pending(directory: &Path, token: &str) -> Result<PathBuf> {
    if token.len() != 32 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("the restore token is not valid; upload the backup again");
    }
    let path = backups(directory).join(format!("{PENDING_PREFIX}{token}.zip"));
    if !path.is_file() {
        bail!("the uploaded backup is gone; upload it again");
    }
    Ok(path)
}

fn remove_pending(directory: &Path, except: Option<&Path>) {
    let Ok(entries) = std::fs::read_dir(backups(directory)) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(PENDING_PREFIX) && Some(path.as_path()) != except {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Checks an uploaded archive and keeps it for `restore`: the token and what
/// a restore changes.
pub fn inspect(directory: &Path, bytes: &[u8]) -> Result<Value> {
    if bytes.len() as u64 > core::MAX_ARCHIVE {
        bail!("the archive is too large");
    }
    let archive = core::read(std::io::Cursor::new(bytes))?;
    let summary = core::summarize(&archive, directory);
    let token = hex::encode(butterpollo_core::crypto::random::<16>());
    let path = backups(directory).join(format!("{PENDING_PREFIX}{token}.zip"));
    butterpollo_core::state::atomic_write(&path, bytes)?;
    remove_pending(directory, Some(&path));
    Ok(json!({"status": true, "token": token, "summary": summary}))
}

/// Saves the current state, stages the pending archive and asks for a
/// restart. Refused while a device streams or connects.
pub fn restore(h: &Shared, token: &str) -> Result<Value> {
    if crate::auto_lock::devices_connected(h) {
        bail!(Busy);
    }
    let directory = &h.directory;
    let path = pending(directory, token)?;
    // The pending file is checked again: it sat on disk since `inspect`.
    core::read(std::io::BufReader::new(std::fs::File::open(&path)?))
        .context("the uploaded backup can no longer be read")?;
    let before = backups(directory).join(format!("before-restore-{}.zip", now_unix()));
    butterpollo_core::state::atomic_write(&before, &export(directory, true)?)
        .context("the current settings could not be saved first, so nothing was restored")?;
    prune_before_restore(directory);
    std::fs::rename(&path, backups(directory).join(core::STAGED))
        .context("the backup could not be staged")?;
    remove_pending(directory, None);
    tracing::info!(saved = %before.display(), "settings restore staged; restarting");
    h.restart.store(true, std::sync::atomic::Ordering::Release);
    h.stop.store(true, std::sync::atomic::Ordering::Release);
    h.wake_maintenance();
    Ok(
        json!({"status": true, "saved": before.file_name().map(|n| n.to_string_lossy().into_owned())}),
    )
}

fn prune_before_restore(directory: &Path) {
    let Ok(entries) = std::fs::read_dir(backups(directory)) else {
        return;
    };
    let mut saved: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("before-restore-") && n.ends_with(".zip"))
        })
        .collect();
    saved.sort();
    let excess = saved.len().saturating_sub(KEEP_BEFORE_RESTORE);
    for path in saved.into_iter().take(excess) {
        let _ = std::fs::remove_file(path);
    }
}

/// A restore refused because a device is connected.
#[derive(Debug)]
pub struct Busy;
impl std::fmt::Display for Busy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a device is connected; restore the settings when no device streams")
    }
}
impl std::error::Error for Busy {}

/// The tray's "Back up settings": an archive without secrets in the signed-in
/// user's Documents\Rubylight folder.
pub fn export_from_tray(h: &Shared) -> Result<PathBuf> {
    let environment = butterpollo_windows::process::user_environment()?;
    let profile = environment
        .get("USERPROFILE")
        .context("the signed-in user's profile folder is unknown")?;
    let path = Path::new(profile)
        .join("Documents")
        .join("Rubylight")
        .join(file_name(false));
    let bytes = export(&h.directory, false)?;
    butterpollo_core::state::atomic_write(&path, &bytes)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);
    impl Temp {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn tempdir() -> Temp {
        let path = std::env::temp_dir().join(format!("rubylight-backup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Temp(path)
    }

    #[test]
    fn settings_backup_inspect_keeps_one_pending_archive_and_restore_needs_its_token() {
        let source = tempdir();
        std::fs::write(source.path().join("sunshine.conf"), "sunshine_name = Den\n").unwrap();
        let bytes = export(source.path(), false).unwrap();

        let target = tempdir();
        let first = inspect(target.path(), &bytes).unwrap();
        let second = inspect(target.path(), &bytes).unwrap();
        let token = second["token"].as_str().unwrap();
        assert_ne!(first["token"], second["token"]);
        assert!(pending(target.path(), first["token"].as_str().unwrap()).is_err());
        assert!(pending(target.path(), token).is_ok());
        assert!(pending(target.path(), "../../sunshine").is_err());
        let changes = second["summary"]["changes"].as_array().unwrap();
        assert!(
            changes.iter().any(|c| c["item"] == "settings"
                && c["detail"].as_str().unwrap().contains("sunshine_name"))
        );
        assert!(inspect(target.path(), b"not a zip").is_err());
    }

    #[test]
    fn settings_backup_keeps_the_newest_before_restore_archives() {
        let dir = tempdir();
        let folder = backups(dir.path());
        std::fs::create_dir_all(&folder).unwrap();
        for stamp in 1..=7 {
            std::fs::write(folder.join(format!("before-restore-{stamp}.zip")), "x").unwrap();
        }
        std::fs::write(folder.join(core::STAGED), "x").unwrap();
        prune_before_restore(dir.path());
        let mut left: Vec<String> = std::fs::read_dir(&folder)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "before-restore-3.zip",
                "before-restore-4.zip",
                "before-restore-5.zip",
                "before-restore-6.zip",
                "before-restore-7.zip",
                core::STAGED,
            ]
        );
        assert!(file_name(true).ends_with("-with-secrets.zip"));
        assert!(file_name(false).starts_with("rubylight-settings-"));
    }
}
