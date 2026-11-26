//! Ending an in-place update that power loss or a crash interrupted, when the
//! service starts: an update whose setup had backed up the files is rolled
//! back, one that stopped before the backup (so no file changed) is recorded
//! as failed. Setup (rust/setup/src/update.rs) writes the backup and the
//! record read here, and rolls back the same way when it starts; keep the two
//! in step. The service (rust/service) compiles this file itself: a change
//! here needs a new service version (rust/release/README.md).
use crate::state;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};

/// What the update record says of an update of this installation that
/// never finished.
#[derive(Debug, PartialEq, Eq)]
enum Interrupted {
    /// "installing" without a backup: setup stopped before it changed a file.
    BeforeBackup,
    /// "installing" or "recovery_failed" with a backup: files may have
    /// changed, and the backup holds the previous ones.
    AfterBackup,
}
/// The error an update that stopped before its backup ends with. The console
/// shows it, and the host reports it for an automatic update.
pub const STOPPED_BEFORE_BACKUP: &str = "Setup stopped before it changed any file (Windows may have restarted or setup was closed), so the installed version still runs. Start the update again from Updates in the console's settings.";

/// If the last update of `install` still says "installing" or
/// "recovery_failed" in the profile's update-result.json and no setup holds
/// update.lock, that setup never finished: put back the files its backup
/// holds or, when it stopped before making one, record the update as failed.
/// Returns what it recorded, if it did anything.
pub fn recover(profile: &Path, install: &Path) -> Result<Option<String>> {
    let path = profile.join("update-result.json");
    if interrupted(&state::load_json(&path, Value::Null)?, install).is_none() {
        return Ok(None);
    }
    let Some(_lock) = lock(&profile.join("update.lock"))? else {
        return Ok(None);
    };
    // Setup may have finished before the lock was free.
    let mut record = state::load_json(&path, Value::Null)?;
    let Some(interrupted) = interrupted(&record, install) else {
        return Ok(None);
    };
    if interrupted == Interrupted::BeforeBackup {
        record["phase"] = json!("failed");
        record["error"] = json!(STOPPED_BEFORE_BACKUP);
        state::write_json(&path, &record)?;
        return Ok(Some(STOPPED_BEFORE_BACKUP.to_owned()));
    }
    let backup = PathBuf::from(record["backup"].as_str().unwrap_or_default());
    let restored = restore(&backup, install);
    let (phase, message) = match &restored {
        Ok(()) => (
            "rolled_back",
            "The update did not finish; the previous version was restored.".to_owned(),
        ),
        Err(error) => (
            "recovery_failed",
            format!(
                "The update did not finish, and restoring the previous version failed: {error:#}. Backup: {}",
                backup.display()
            ),
        ),
    };
    record["phase"] = json!(phase);
    record["error"] = json!(message);
    state::write_json(&path, &record)?;
    restored.map(|()| Some(message))
}
/// Only a record that names this installation counts. The one the host
/// writes before it starts setup names none; setup replaces it with its own,
/// which does, once it holds update.lock and before it stops the service.
fn interrupted(record: &Value, install: &Path) -> Option<Interrupted> {
    let this_install = record["install"].as_str().is_some_and(|recorded| {
        std::fs::canonicalize(recorded)
            .ok()
            .zip(std::fs::canonicalize(install).ok())
            .is_some_and(|(a, b)| a == b)
    });
    match (record["phase"].as_str(), record["backup"].is_string()) {
        _ if !this_install => None,
        (Some("installing" | "recovery_failed"), true) => Some(Interrupted::AfterBackup),
        (Some("installing"), false) => Some(Interrupted::BeforeBackup),
        _ => None,
    }
}
/// update.lock, which keeps a setup from starting meanwhile; None while
/// one holds it.
fn lock(path: &Path) -> Result<Option<std::fs::File>> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(false).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    match options.open(path) {
        Ok(file) => Ok(Some(file)),
        // ERROR_SHARING_VIOLATION
        Err(error) if error.raw_os_error() == Some(32) => Ok(None),
        Err(error) => Err(error.into()),
    }
}
/// Setup's Backup::restore: the files that existed are copied back, the
/// ones the update added are removed. Every saved file is checked first.
fn restore(backup: &Path, install: &Path) -> Result<()> {
    let files: Vec<(String, bool)> =
        serde_json::from_slice(&std::fs::read(backup.join("backup.json"))?)
            .context("reading the update backup")?;
    let relative = |name: &str| -> Result<PathBuf> {
        let path = PathBuf::from(name);
        if path.as_os_str().is_empty()
            || path
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            bail!("unsafe path in the update backup");
        }
        Ok(path)
    };
    for (name, existed) in &files {
        let path = relative(name)?;
        if *existed && !backup.join(path).is_file() {
            bail!("the update backup lacks {name}");
        }
    }
    for (name, existed) in &files {
        let target = install.join(relative(name)?);
        if *existed {
            replace(&backup.join(relative(name)?), &target)?;
        } else if target.exists() {
            std::fs::remove_file(&target)?;
        }
    }
    Ok(())
}
/// Setup's replace_file: a file in use (the running service) is renamed
/// aside, deleted when Windows restarts, and the copy put in its place.
fn replace(source: &Path, target: &Path) -> Result<()> {
    crate::update_files::replace(source, target, delete_at_restart)
}
fn delete_at_restart(path: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        const MOVEFILE_DELAY_UNTIL_REBOOT: u32 = 0x4;
        unsafe extern "system" {
            fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
        }
        let wide: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: `wide` is NUL-terminated and outlives the call; a null target
        // with this flag schedules deletion.
        unsafe { MoveFileExW(wide.as_ptr(), std::ptr::null(), MOVEFILE_DELAY_UNTIL_REBOOT) };
    }
    #[cfg(not(windows))]
    let _ = std::fs::remove_file(path);
}
#[cfg(test)]
mod tests {
    use super::*;
    // update.lock excludes a second opener only through Windows share modes.
    #[cfg(windows)]
    #[test]
    fn an_interrupted_update_is_rolled_back_unless_setup_runs() -> Result<()> {
        let root = tempfile::tempdir()?;
        let profile = root.path().join("config");
        let install = root.path().join("Butterpollo");
        let backup = profile.join("updates/transaction-1-2/previous");
        std::fs::create_dir_all(&backup)?;
        std::fs::create_dir_all(install.join("assets"))?;
        std::fs::write(backup.join("butterpollo.exe"), "previous")?;
        std::fs::create_dir(backup.join("assets"))?;
        std::fs::write(backup.join("assets/web.js"), "previous web")?;
        std::fs::write(
            backup.join("backup.json"),
            r#"[["assets/web.js",true],["butterpollo.exe",true],["added.dll",false]]"#,
        )?;
        let installing = || -> Result<()> {
            std::fs::write(install.join("butterpollo.exe"), "half")?;
            std::fs::write(install.join("assets/web.js"), "half web")?;
            std::fs::write(install.join("added.dll"), "added")?;
            state::write_json(
                &profile.join("update-result.json"),
                &json!({"version":"2.0.1","phase":"installing","error":null,
                        "backup":backup,"install":install}),
            )
        };
        installing()?;
        // A setup that holds the lock is still at work.
        let held = lock(&profile.join("update.lock"))?.unwrap();
        assert_eq!(recover(&profile, &install)?, None);
        assert_eq!(std::fs::read(install.join("butterpollo.exe"))?, b"half");
        drop(held);
        // Another installation is not touched.
        assert_eq!(recover(&profile, &backup)?, None);
        assert!(recover(&profile, &install)?.is_some());
        assert_eq!(std::fs::read(install.join("butterpollo.exe"))?, b"previous");
        assert_eq!(
            std::fs::read(install.join("assets/web.js"))?,
            b"previous web"
        );
        assert!(!install.join("added.dll").exists());
        let record = state::load_json(&profile.join("update-result.json"), Value::Null)?;
        assert_eq!(record["phase"], "rolled_back");
        assert_eq!(record["version"], "2.0.1");
        // Done once: the next start leaves the files alone.
        std::fs::write(install.join("butterpollo.exe"), "later")?;
        assert_eq!(recover(&profile, &install)?, None);
        assert_eq!(std::fs::read(install.join("butterpollo.exe"))?, b"later");
        // An update that ended has nothing to roll back. The record the host
        // writes before it starts setup names no installation: setup puts
        // its own in its place before it changes anything (the next test).
        for record in [
            json!({"version":"2.0.1","phase":"installed","error":null,"backup":backup,"install":install}),
            json!({"version":"2.0.1","phase":"installing","started_at":1}),
        ] {
            state::write_json(&profile.join("update-result.json"), &record)?;
            assert_eq!(recover(&profile, &install)?, None);
            assert_eq!(
                state::load_json(&profile.join("update-result.json"), Value::Null)?,
                record
            );
        }
        // A backup with a file missing restores nothing.
        installing()?;
        std::fs::remove_file(backup.join("assets/web.js"))?;
        assert!(recover(&profile, &install).is_err());
        assert_eq!(std::fs::read(install.join("butterpollo.exe"))?, b"half");
        let record = state::load_json(&profile.join("update-result.json"), Value::Null)?;
        assert_eq!(record["phase"], "recovery_failed");
        assert_eq!(record["backup"], json!(backup));
        assert_eq!(record["install"], json!(install));
        std::fs::write(backup.join("assets/web.js"), "previous web")?;
        assert!(recover(&profile, &install)?.is_some());
        assert_eq!(std::fs::read(install.join("butterpollo.exe"))?, b"previous");
        Ok(())
    }
    #[cfg(windows)]
    #[test]
    fn an_update_that_stopped_before_its_backup_is_recorded_as_failed() -> Result<()> {
        let root = tempfile::tempdir()?;
        let profile = root.path().join("config");
        let install = root.path().join("Butterpollo");
        let other = root.path().join("Other");
        std::fs::create_dir_all(&profile)?;
        std::fs::create_dir(&install)?;
        std::fs::create_dir(&other)?;
        std::fs::write(install.join("butterpollo.exe"), "installed")?;
        let path = profile.join("update-result.json");
        // What setup writes once it holds update.lock, before the backup.
        let started = |install: &Path| json!({"version":"2.0.1","phase":"installing","error":null,"install":install});
        let read = || state::load_json(&path, Value::Null);
        state::write_json(&path, &started(&install))?;
        // A setup that holds the lock is still at work.
        let held = lock(&profile.join("update.lock"))?.unwrap();
        assert_eq!(recover(&profile, &install)?, None);
        assert_eq!(read()?, started(&install));
        drop(held);
        // Another installation's update is not this service's to end.
        assert_eq!(recover(&profile, &other)?, None);
        assert_eq!(read()?, started(&install));
        // Setup stopped (power loss, a crash) before it changed a file.
        assert_eq!(
            recover(&profile, &install)?.as_deref(),
            Some(STOPPED_BEFORE_BACKUP)
        );
        let record = read()?;
        assert_eq!(record["phase"], "failed");
        assert_eq!(record["error"], STOPPED_BEFORE_BACKUP);
        assert_eq!(record["version"], "2.0.1");
        assert_eq!(
            std::fs::read(install.join("butterpollo.exe"))?,
            b"installed"
        );
        // Done once: the next start leaves it alone.
        assert_eq!(recover(&profile, &install)?, None);
        assert_eq!(read()?, record);
        // Records of updates that ended are left as they are, and so is a
        // record of another installation's.
        for record in [
            json!({"version":"2.0.1","phase":"installed","error":null,"install":install}),
            json!({"version":"2.0.1","phase":"failed","error":"Disconnect all streams","install":install}),
            json!({"version":"2.0.1","phase":"rolled_back","error":"restored","install":install}),
            started(&other),
        ] {
            state::write_json(&path, &record)?;
            assert_eq!(recover(&profile, &install)?, None);
            assert_eq!(read()?, record);
        }
        Ok(())
    }
    #[test]
    fn every_backup_path_is_validated_before_any_file_is_restored() -> Result<()> {
        let root = tempfile::tempdir()?;
        let backup = root.path().join("backup");
        let install = root.path().join("install");
        std::fs::create_dir(&backup)?;
        std::fs::create_dir(&install)?;
        std::fs::write(backup.join("host.exe"), b"previous")?;
        std::fs::write(install.join("host.exe"), b"current")?;
        std::fs::write(
            backup.join("backup.json"),
            r#"[["host.exe",true],["../outside",false]]"#,
        )?;
        assert!(restore(&backup, &install).is_err());
        assert_eq!(std::fs::read(install.join("host.exe"))?, b"current");
        Ok(())
    }
}
