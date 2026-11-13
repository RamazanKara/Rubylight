//! In-place service updates. The verified package is staged before shutdown;
//! changed files are backed up and restored if copying or startup fails.
use crate::{detect, install, log::line, payload, system, ui::Progress};
use anyhow::{Context, Result, bail};
use serde_json::json;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::Duration,
};

/// `before_start` runs once the new files are in place, while the service
/// is still stopped: the driver scripts stop a running service and start it
/// again, which would drop a client that just reconnected.
pub fn run(
    folder: &Path,
    start: bool,
    end_streams: bool,
    progress: &Progress,
    before_start: &mut dyn FnMut(),
) -> Result<()> {
    let profile = install::profile();
    let result = profile.join("update-result.json");
    let service = detect::service();
    let mut lock = None;
    let Prepared {
        install,
        work,
        staged,
        backup,
        entries,
        previous,
        paths,
    } = match prepare(folder, &profile, end_streams, progress, &mut lock) {
        Ok(prepared) => prepared,
        Err(error) => {
            record_refusal(&profile, lock, &error);
            return Err(error);
        }
    };
    let _lock = lock;
    write_result(&result, "installing", None).inspect_err(|_| remove_work(&work))?;
    progress.set("Stopping Rubylight…");
    stop_for_update(
        &result,
        || system::stop_service(service),
        || {
            if !system::wait_stopped(service, Duration::from_secs(120)) {
                line("the service is still stopping");
            }
            system::start_service(service)
        },
    )
    .inspect_err(|_| remove_work(&work))?;
    // Nothing has been overwritten if the backup fails.
    let saved = Backup::create(&install, &backup, &paths).and_then(|saved| {
        // From here files change. If this setup cannot finish, setup or
        // the service rolls back from this backup the next time it starts.
        write_record(
            &result,
            &json!({"version":env!("CARGO_PKG_VERSION"),"phase":"installing","error":null,
                    "backup":backup,"install":install}),
        )?;
        Ok(saved)
    });
    let saved = match saved {
        Ok(saved) => saved,
        Err(error) => {
            let _ = system::start_service(service);
            // No record names this backup: nothing was replaced.
            remove_work(&work);
            write_result(&result, "failed", Some(&format!("{error:#}")))?;
            return Err(error);
        }
    };
    // Set when the service is disabled, so it was left stopped.
    let left_disabled = std::cell::Cell::new(false);
    let update = attempt_install(
        &saved,
        &install,
        || -> Result<()> {
            progress.set("Installing the update…");
            replace_package(&staged, &install, &entries)?;
            before_start();
            progress.set("Checking that Rubylight starts…");
            if start {
                if system::start_service_unless_disabled(service)? {
                    install::wait_ready(install::probe(&profile), Some(env!("CARGO_PKG_VERSION")))?;
                    // The host made the console's certificate as it started.
                    install::trust_console_or_warn(&profile);
                } else {
                    left_disabled.set(true);
                }
            }
            install::register(&install, &entries)?;
            Ok(())
        },
        || {
            progress.set("Restoring the previous version…");
            system::stop_service(service)
        },
        || {
            // A disabled service was never running; the files are restored.
            if system::start_service_unless_disabled(service)? {
                install::wait_ready(install::probe(&profile), None)?;
            }
            Ok(())
        },
    );
    match update {
        Ok(()) => finish_update(
            &install,
            &work,
            &result,
            &previous,
            &entries,
            left_disabled.get().then_some(DISABLED_WARNING),
        ),
        Err(InstallFailure {
            error,
            recovery: rollback,
        }) => {
            let message = match &rollback {
                Ok(()) => format!("Update failed; the previous version was restored. {error:#}"),
                Err(rollback) => format!(
                    "Update failed: {error:#}. Recovery failed: {rollback:#}. Backup: {}",
                    backup.display()
                ),
            };
            let phase = if rollback.is_ok() {
                "rolled_back"
            } else {
                "recovery_failed"
            };
            write_record(
                &result,
                &json!({"version":env!("CARGO_PKG_VERSION"),"phase":phase,"error":message,
                        "backup":backup,"install":install}),
            )?;
            bail!("{message}")
        }
    }
}

struct Prepared {
    install: PathBuf,
    work: PathBuf,
    staged: PathBuf,
    backup: PathBuf,
    entries: Vec<payload::Entry>,
    previous: Vec<payload::Entry>,
    paths: BTreeSet<String>,
}
/// Everything before the update changes a file or the service. update.lock
/// is held in `lock` from when it is taken.
fn prepare(
    folder: &Path,
    profile: &Path,
    end_streams: bool,
    progress: &Progress,
    lock: &mut Option<std::fs::File>,
) -> Result<Prepared> {
    detect::scan().check_version()?;
    let install = std::fs::canonicalize(folder)?;
    let service =
        system::service_program(detect::service()).context("Rubylight service is not installed")?;
    check_service_folder(&service, &install)?;
    // Both are the installed service's now: repair what earlier installers
    // left, or refuse.
    install::prepare_profile(profile, true)?;
    install::prepare_install(folder)?;
    // A reinstall may find the profile folder deleted; the lock lives in it.
    std::fs::create_dir_all(profile)?;
    // A non-shared handle rejects another updater until this transaction ends.
    *lock = Some(open_lock(profile).context("Another update is already running")?);
    // The new backup must hold one whole version, not what an interrupted
    // update left.
    install::ensure_idle(install::probe(profile), end_streams)?;
    recover_or_supersede(&profile.join("update-result.json"))?;
    if let Err(error) = install::remove_legacy_staging(&system::program_data()) {
        line(format!("warning: {error:#}"));
    }
    // In the installation folder, which only administrators may change,
    // never in ProgramData (see install::staging_root).
    let staging = install::prepare_staging_root(folder)?;
    // No record names a backup still to be restored any more.
    prune_transactions(&staging);
    let work = staging.join(format!(
        "transaction-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    std::fs::create_dir(&work)?;
    let staged = work.join("package");
    let backup = work.join("previous");
    let unpacked = (|| -> Result<_> {
        system::restrict(&work, false)?;
        std::fs::create_dir(&staged)?;
        progress.set("Verifying the update…");
        payload::Payload::open()?
            .context("The update installer has no package")?
            .extract(&staged)?;
        let entries = payload::verify(&staged)?;
        let previous = payload::manifest(&install).context(
            "The installed package has no readable manifest; run the Rubylight installer to repair it",
        )?;
        Ok((entries, previous))
    })();
    let (entries, previous) = match unpacked {
        Ok(unpacked) => unpacked,
        Err(error) => {
            // Nothing is backed up yet: the staged files have no use.
            remove_work(&work);
            return Err(error);
        }
    };
    let paths = previous
        .iter()
        .chain(&entries)
        .map(|e| e.path.clone())
        .chain(["manifest.json".into(), "uninstall.exe".into()])
        .collect();
    if let Err(error) = install::ensure_idle(install::probe(profile), end_streams) {
        remove_work(&work);
        return Err(error);
    }
    Ok(Prepared {
        install,
        work,
        staged,
        backup,
        entries,
        previous,
        paths,
    })
}
fn open_lock(profile: &Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .share_mode(0)
        .open(profile.join("update.lock"))
}
/// The update stopped before it changed anything. The console shows the
/// update record, so it says why there; the host itself only learns that
/// setup exited. Only into a profile folder setup trusts, never while
/// another setup holds update.lock (`lock` is this one's, if taken), and
/// never over a record that names a backup still to be put back.
pub(crate) fn record_refusal(profile: &Path, lock: Option<std::fs::File>, error: &anyhow::Error) {
    let recorded = (|| -> Result<()> {
        system::trusted_ancestors(profile.parent().context("profile has no parent")?)?;
        let _lock = match lock {
            Some(lock) => lock,
            None => open_lock(profile)?,
        };
        refuse(&profile.join("update-result.json"), error)
    })();
    if let Err(problem) = recorded {
        line(format!("the update record was left as it was: {problem:#}"));
    }
}
fn refuse(result: &Path, error: &anyhow::Error) -> Result<()> {
    let record: serde_json::Value = match std::fs::read(result) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("the update record is unreadable")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::Value::Null,
        Err(error) => return Err(error.into()),
    };
    if matches!(
        record["phase"].as_str(),
        Some("installing" | "recovery_failed")
    ) && record["backup"].is_string()
    {
        bail!("it names a backup that is still to be restored");
    }
    write_result(result, "failed", Some(&format!("{error:#}")))
}
/// After setup installed a complete package. The console shows the update
/// record, which may still say why an earlier update failed.
pub fn record_installed(profile: &Path) {
    let recorded = open_lock(profile)
        .map_err(anyhow::Error::from)
        .and_then(|_lock| write_result(&profile.join("update-result.json"), "installed", None));
    if let Err(error) = recorded {
        line(format!("the update record was not written: {error:#}"));
    }
}

fn replace_package(staged: &Path, install: &Path, entries: &[payload::Entry]) -> Result<()> {
    for entry in entries {
        install::replace_file(
            &payload::safe_join(staged, &entry.path)?,
            &payload::safe_join(install, &entry.path)?,
        )?;
    }
    install::replace_file(
        &staged.join("manifest.json"),
        &install.join("manifest.json"),
    )?;
    payload::write_stub(
        &install.join("uninstall.exe"),
        payload::packaged_stub(staged, entries)?.as_deref(),
    )
}

/// Shown in the update record when the service was left disabled.
pub const DISABLED_WARNING: &str = "The Rubylight service is disabled, as it was before, so it was not started. Set its startup type to Manual or Automatic in Services, then start it.";
fn finish_update(
    install: &Path,
    work: &Path,
    result: &Path,
    previous: &[payload::Entry],
    entries: &[payload::Entry],
    warning: Option<&str>,
) -> Result<()> {
    if let Some(warning) = warning {
        line(warning);
    }
    let record = match warning {
        Some(warning) => {
            json!({"version":env!("CARGO_PKG_VERSION"),"phase":"installed","error":null,"warning":warning})
        }
        None => json!({"version":env!("CARGO_PKG_VERSION"),"phase":"installed","error":null}),
    };
    keep_installed_record(result, || write_record(result, &record))?;
    for old in previous {
        if !entries
            .iter()
            .any(|e| e.path.eq_ignore_ascii_case(&old.path))
        {
            // A file still loaded (an old Vulkan layer in a running game)
            // goes at the next restart, as a full installation does.
            system::remove_file_later(&payload::safe_join(install, &old.path)?);
        }
    }
    remove_work(work);
    Ok(())
}
/// Delete an update's transaction folder, and the staging folder holding it
/// once nothing else is in it.
fn remove_work(work: &Path) {
    match std::fs::remove_dir_all(work) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => line(format!(
            "the update folder {} could not be removed: {error}",
            work.display()
        )),
    }
    if let Some(staging) = work.parent() {
        let _ = std::fs::remove_dir(staging);
    }
}
/// A failed update keeps its transaction folder, with the backup, in the
/// staging folder; all but the newest `KEPT_TRANSACTIONS` go, as the host's
/// updater did when they were in the profile. Only once no record names a
/// backup still to be restored. Other names are left alone.
fn prune_transactions(staging: &Path) {
    let Ok(entries) = std::fs::read_dir(staging) else {
        return;
    };
    let names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    for name in stale_transactions(&names) {
        let folder = staging.join(name);
        if let Err(error) = std::fs::remove_dir_all(&folder) {
            line(format!(
                "the earlier update folder {} could not be removed: {error}",
                folder.display()
            ));
        }
    }
}
const KEPT_TRANSACTIONS: usize = 2;
/// Transaction folders (transaction-<process id>-<nanoseconds>) older than
/// the newest `KEPT_TRANSACTIONS`.
fn stale_transactions(names: &[String]) -> Vec<&str> {
    let mut dated: Vec<(u128, &str)> = names
        .iter()
        .filter_map(|name| {
            let (process, time) = name.strip_prefix("transaction-")?.split_once('-')?;
            process.parse::<u32>().ok()?;
            Some((time.parse().ok()?, name.as_str()))
        })
        .collect();
    dated.sort_unstable_by(|a, b| b.cmp(a));
    dated
        .into_iter()
        .skip(KEPT_TRANSACTIONS)
        .map(|(_, name)| name)
        .collect()
}
/// The update works, so the record must stop saying "installing" with a
/// backup: the service would restore the previous version over it at its
/// next start. When the write fails twice (a full disk, a scanner holding
/// the file) the record goes, and setup still reports the error.
fn keep_installed_record(result: &Path, write: impl Fn() -> Result<()>) -> Result<()> {
    let Err(error) = write().or_else(|_| write()) else {
        return Ok(());
    };
    match std::fs::remove_file(result) {
        Ok(()) => Err(error.context(
            "The update is installed, but its record could not be written, so it was removed",
        )),
        Err(removal) if removal.kind() == std::io::ErrorKind::NotFound => Err(error),
        Err(removal) => Err(error.context(format!(
            "The update is installed, but its record could not be written or removed ({removal}); \
             the service may restore the previous version at its next start"
        ))),
    }
}

fn check_service_folder(service: &Path, install: &Path) -> Result<()> {
    // The executable itself can be missing after an interrupted replacement.
    if !service
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("butterpollo-service.exe"))
        || std::fs::canonicalize(service.parent().context("service program has no folder")?)?
            != install
    {
        bail!("The update folder does not belong to the installed Rubylight service");
    }
    Ok(())
}

/// Roll back an update that power loss or a crash interrupted, unless a
/// setup is running (it holds update.lock). See recover_locked.
pub fn recover(profile: &Path) -> Result<()> {
    let result = profile.join("update-result.json");
    if !result.is_file() {
        return Ok(());
    }
    let _lock = open_lock(profile)
        .context("Cannot recover the previous update; another setup may still be running")?;
    recover_or_supersede(&result)
}
/// Setup installs a complete package next, so an update that cannot be
/// rolled back must not block it: that would leave a mix of two versions
/// that neither setup nor the in-app updater could ever replace. The record
/// stops naming a backup to restore, so the service never puts the old
/// backup over the new files; the backup folder itself is kept.
fn recover_or_supersede(result: &Path) -> Result<()> {
    let Err(error) = recover_locked(result) else {
        return Ok(());
    };
    line(format!(
        "warning: the earlier update could not be rolled back; installing the complete package over it: {error:#}"
    ));
    let record: serde_json::Value = std::fs::read(result)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let kept = record["backup"]
        .as_str()
        .map_or_else(String::new, |backup| {
            format!(" Its backup is kept in {backup}.")
        });
    write_record(
        result,
        &json!({"version":record["version"],"phase":"superseded","previous_backup":record["backup"],
                "error":format!("An earlier update could not be rolled back ({error:#}). Setup {} replaces the installed files with its complete package.{kept}", env!("CARGO_PKG_VERSION"))}),
    )
}
/// With update.lock held: if the last update still says "installing" after
/// its backup was made, its setup never finished, so the files the backup
/// holds are put back. The service does the same when it starts
/// (butterpollo_core::update_recovery); keep the two in step.
fn recover_locked(result: &Path) -> Result<()> {
    let bytes = match std::fs::read(result) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let mut record: serde_json::Value = serde_json::from_slice(&bytes).context(
        "The update recovery record cannot be read; keep the update's backup for recovery",
    )?;
    let (Some("installing" | "recovery_failed"), Some(backup), Some(install)) = (
        record["phase"].as_str(),
        record["backup"].as_str(),
        record["install"].as_str(),
    ) else {
        return Ok(());
    };
    line(format!(
        "the update to {} did not finish; restoring {install} from {backup}",
        record["version"]
    ));
    let restored =
        Backup::load(Path::new(backup)).and_then(|saved| saved.restore(Path::new(install)));
    let (phase, message) = match &restored {
        Ok(()) => (
            "rolled_back",
            "The update did not finish; the previous version was restored.".to_owned(),
        ),
        Err(error) => (
            "recovery_failed",
            format!(
                "The update did not finish, and restoring the previous version failed: {error:#}. Backup: {backup}"
            ),
        ),
    };
    record["phase"] = json!(phase);
    record["error"] = json!(message);
    write_record(result, &record)?;
    restored
}
/// Stop the service before any file changes. If it does not stop in time,
/// the update fails before changing anything, and the service is started
/// again once it has stopped, so the host is not left stopped.
fn stop_for_update(
    result: &Path,
    stop: impl FnOnce() -> Result<()>,
    start_again: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let Err(error) = stop() else {
        return Ok(());
    };
    if let Err(start) = start_again() {
        line(format!("the service could not be started again: {start:#}"));
    }
    write_result(result, "failed", Some(&format!("{error:#}")))?;
    Err(error)
}

#[derive(Debug)]
struct InstallFailure {
    error: anyhow::Error,
    recovery: Result<()>,
}

/// The same recovery path is exercised with isolated files and a stand-in
/// service lifecycle in tests; it never overwrites files if stopping fails.
fn attempt_install(
    saved: &Backup,
    install: &Path,
    apply: impl FnOnce() -> Result<()>,
    stop: impl FnOnce() -> Result<()>,
    restart: impl FnOnce() -> Result<()>,
) -> std::result::Result<(), InstallFailure> {
    if let Err(error) = apply() {
        let recovery = (|| {
            stop()?;
            saved.restore(install)?;
            restart()
        })();
        Err(InstallFailure { error, recovery })
    } else {
        Ok(())
    }
}

fn write_result(path: &Path, phase: &str, error: Option<&str>) -> Result<()> {
    write_record(
        path,
        &json!({"version":env!("CARGO_PKG_VERSION"),"phase":phase,"error":error}),
    )
}
fn write_record(path: &Path, value: &serde_json::Value) -> Result<()> {
    let temporary = path.with_extension("tmp");
    let mut file = std::fs::File::create(&temporary)?;
    std::io::Write::write_all(&mut file, &serde_json::to_vec_pretty(value)?)?;
    // A record that names a backup must survive a power loss.
    file.sync_all()?;
    drop(file);
    // The console may read this file during startup. Replace it atomically.
    crate::update_files::publish(&temporary, path)?;
    Ok(())
}

struct Backup {
    directory: PathBuf,
    files: Vec<(String, bool)>,
}
impl Backup {
    fn create(install: &Path, directory: &Path, paths: &BTreeSet<String>) -> Result<Self> {
        // On disk before any file is replaced, so that a power loss cannot
        // leave a rollback with a backup that was never written.
        let flush = |path: &Path| -> Result<()> {
            Ok(std::fs::OpenOptions::new()
                .write(true)
                .open(path)?
                .sync_all()?)
        };
        let mut files = Vec::new();
        for name in paths {
            let source = payload::safe_join(install, name)?;
            let target = payload::safe_join(directory, name)?;
            if source.exists() {
                std::fs::create_dir_all(target.parent().context("backup path has no parent")?)?;
                std::fs::copy(source, &target)?;
                flush(&target)?;
                files.push((name.clone(), true));
            } else {
                files.push((name.clone(), false));
            }
        }
        std::fs::write(directory.join("backup.json"), serde_json::to_vec(&files)?)?;
        flush(&directory.join("backup.json"))?;
        Ok(Self {
            directory: directory.to_path_buf(),
            files,
        })
    }
    fn load(directory: &Path) -> Result<Self> {
        let files: Vec<(String, bool)> =
            serde_json::from_slice(&std::fs::read(directory.join("backup.json"))?)
                .context("reading the update backup")?;
        // Every saved file is checked before any is put back.
        for (name, existed) in &files {
            let path = payload::safe_join(directory, name)?;
            if *existed && !path.is_file() {
                bail!("the update backup lacks {name}");
            }
        }
        Ok(Self {
            directory: directory.to_path_buf(),
            files,
        })
    }
    fn restore(&self, install: &Path) -> Result<()> {
        for (name, existed) in &self.files {
            let target = payload::safe_join(install, name)?;
            if *existed {
                install::replace_file(&payload::safe_join(&self.directory, name)?, &target)?;
            } else if target.exists() {
                std::fs::remove_file(target)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
