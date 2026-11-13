use super::*;
use crate::profile_tests::{assert_usable, package, rc23_profile, snapshot};
#[test]
fn a_missing_service_executable_does_not_prevent_recovery() -> Result<()> {
    let root = tempfile::tempdir()?;
    let install = root.path().join("installed");
    std::fs::create_dir(&install)?;
    let install = install.canonicalize()?;
    let service = install.join("butterpollo-service.exe");
    std::fs::write(&service, b"previous service")?;
    let backup = root.path().join("backup");
    Backup::create(
        &install,
        &backup,
        &["butterpollo-service.exe".into()].into_iter().collect(),
    )?;
    let record = root.path().join("update-result.json");
    write_record(
        &record,
        &json!({"phase":"installing","backup":backup,"install":install}),
    )?;
    std::fs::remove_file(&service)?;
    check_service_folder(&service, &install)?;
    recover_locked(&record)?;
    assert_eq!(std::fs::read(service)?, b"previous service");
    assert!(check_service_folder(&install.join("another-service.exe"), &install).is_err());
    assert!(check_service_folder(&backup.join("butterpollo-service.exe"), &install).is_err());
    Ok(())
}
#[test]
fn rc23_upgrade_keeps_the_entire_profile_and_only_replaces_package_files() -> Result<()> {
    let root = tempfile::tempdir()?;
    let install = root.path().join("Çağrı Müller/installed");
    let profile = root.path().join("Çağrı Müller/config");
    rc23_profile(&profile)?;
    let mut expected_profile = snapshot(&profile)?;
    let previous = package(&install, "2.0.0-rc.23", "obsolete.dll")?;
    std::fs::write(install.join("uninstall.exe"), b"rc.23 setup")?;
    std::fs::write(install.join("user-owned.txt"), b"outside the manifest")?;
    let work = profile.join("updates/transaction-test");
    let staged = work.join("package");
    let entries = package(&staged, env!("CARGO_PKG_VERSION"), "new.dll")?;
    let mut expected_install = snapshot(&staged)?;
    expected_install.insert("user-owned.txt".into(), b"outside the manifest".to_vec());
    expected_install.insert(
        "uninstall.exe".into(),
        std::fs::read(std::env::current_exe()?)?,
    );
    let paths = previous
        .iter()
        .chain(&entries)
        .map(|entry| entry.path.clone())
        .chain(["manifest.json".into(), "uninstall.exe".into()])
        .collect();
    let saved = Backup::create(&install, &work.join("previous"), &paths)?;
    let result = profile.join("update-result.json");
    write_result(&result, "installing", None)?;
    attempt_install(
        &saved,
        &install,
        || replace_package(&staged, &install, &entries),
        || panic!("successful update must not roll back"),
        || panic!("successful update must not restart the previous host"),
    )
    .map_err(|failure| failure.error)?;
    assert!(
        work.canonicalize()?
            .starts_with(root.path().canonicalize()?)
    );
    finish_update(&install, &work, &result, &previous, &entries, None)?;
    expected_profile.insert(
        "update-result.json".into(),
        serde_json::to_vec_pretty(
            &json!({"version":env!("CARGO_PKG_VERSION"),"phase":"installed","error":null}),
        )?,
    );
    assert_eq!(snapshot(&install)?, expected_install);
    assert_eq!(snapshot(&profile)?, expected_profile);
    assert!(!work.exists());
    assert_usable(&profile)?;
    // A completed update must never restore its old binaries later.
    recover(&profile)?;
    assert_eq!(snapshot(&install)?, expected_install);
    assert_eq!(snapshot(&profile)?, expected_profile);
    Ok(())
}
#[test]
fn rc23_profile_survives_rollback_after_each_interrupted_package_file() -> Result<()> {
    let root = tempfile::tempdir()?;
    let install = root.path().join("Çağrı Müller/installed");
    let profile = root.path().join("Çağrı Müller/config");
    rc23_profile(&profile)?;
    let previous = package(&install, "2.0.0-rc.23", "obsolete.dll")?;
    std::fs::write(install.join("uninstall.exe"), b"rc.23 setup")?;
    std::fs::write(install.join("user-owned.txt"), b"outside the manifest")?;
    let expected_install = snapshot(&install)?;
    let work = profile.join("updates/transaction-interrupted");
    let staged = work.join("package");
    let entries = package(&staged, env!("CARGO_PKG_VERSION"), "new.dll")?;
    std::fs::write(staged.join("uninstall.exe"), b"new setup stub")?;
    let paths = previous
        .iter()
        .chain(&entries)
        .map(|entry| entry.path.clone())
        .chain(["manifest.json".into(), "uninstall.exe".into()])
        .collect();
    let backup = work.join("previous");
    Backup::create(&install, &backup, &paths)?;
    let changed: Vec<_> = entries
        .iter()
        .map(|entry| entry.path.as_str())
        .chain(["manifest.json", "uninstall.exe"])
        .collect();
    let result = profile.join("update-result.json");
    let mut expected_profile = snapshot(&profile)?;
    expected_profile.remove(Path::new("update-result.json"));
    for copied in 0..=changed.len() {
        write_record(
            &result,
            &json!({"version":env!("CARGO_PKG_VERSION"),"phase":"installing",
            "error":null,"backup":backup,"install":install}),
        )?;
        for name in changed.iter().take(copied) {
            install::replace_file(&staged.join(name), &install.join(name))?;
        }
        recover(&profile)?;
        assert_eq!(
            snapshot(&install)?,
            expected_install,
            "interrupted after {copied} files"
        );
        let mut recovered = snapshot(&profile)?;
        let record: serde_json::Value =
            serde_json::from_slice(&recovered.remove(Path::new("update-result.json")).unwrap())?;
        assert_eq!(record["phase"], "rolled_back");
        assert_eq!(record["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(record["backup"], json!(backup));
        assert_eq!(record["install"], json!(install));
        assert_eq!(
            recovered, expected_profile,
            "interrupted after {copied} files"
        );
        let complete = snapshot(&profile)?;
        recover(&profile)?;
        assert_eq!(snapshot(&profile)?, complete);
        assert_eq!(snapshot(&install)?, expected_install);
    }
    assert_usable(&profile)?;
    Ok(())
}
#[test]
fn startup_failure_restores_files_before_restarting_the_previous_host() -> Result<()> {
    let root = tempfile::tempdir()?;
    let installed = root.path().join("installed");
    std::fs::create_dir(&installed)?;
    std::fs::write(installed.join("host.exe"), b"previous")?;
    let backup = Backup::create(
        &installed,
        &root.path().join("backup"),
        &["host.exe".into()].into_iter().collect(),
    )?;
    let stopped = std::cell::Cell::new(false);
    let failure = attempt_install(
        &backup,
        &installed,
        || {
            std::fs::write(installed.join("host.exe"), b"update")?;
            bail!("new host failed its startup health check")
        },
        || {
            stopped.set(true);
            Ok(())
        },
        || {
            assert!(stopped.get());
            assert_eq!(std::fs::read(installed.join("host.exe"))?, b"previous");
            Ok(())
        },
    )
    .unwrap_err();
    assert!(failure.recovery.is_ok());
    assert!(failure.error.to_string().contains("startup health check"));
    Ok(())
}
#[test]
fn recovery_does_not_replace_files_if_the_service_cannot_be_stopped() -> Result<()> {
    let root = tempfile::tempdir()?;
    let installed = root.path().join("installed");
    std::fs::create_dir(&installed)?;
    std::fs::write(installed.join("host.exe"), b"previous")?;
    let backup = Backup::create(
        &installed,
        &root.path().join("backup"),
        &["host.exe".into()].into_iter().collect(),
    )?;
    let failure = attempt_install(
        &backup,
        &installed,
        || {
            std::fs::write(installed.join("host.exe"), b"update")?;
            bail!("startup failed")
        },
        || bail!("stop failed"),
        || panic!("must not start after a failed stop"),
    )
    .unwrap_err();
    assert!(failure.recovery.is_err());
    assert_eq!(std::fs::read(installed.join("host.exe"))?, b"update");
    assert_eq!(
        std::fs::read(root.path().join("backup/host.exe"))?,
        b"previous"
    );
    Ok(())
}
#[test]
fn a_service_that_does_not_stop_is_started_again_and_the_update_fails() -> Result<()> {
    let root = tempfile::tempdir()?;
    let result = root.path().join("update-result.json");
    write_result(&result, "installing", None)?;
    let started = std::cell::Cell::new(false);
    let error = stop_for_update(
        &result,
        || bail!("the ApolloService service did not stop"),
        || {
            started.set(true);
            bail!("still stopping")
        },
    )
    .unwrap_err();
    assert!(started.get());
    assert!(error.to_string().contains("did not stop"));
    let written: serde_json::Value = serde_json::from_slice(&std::fs::read(&result)?)?;
    assert_eq!(written["phase"], "failed");
    assert!(written["error"].as_str().unwrap().contains("did not stop"));
    stop_for_update(
        &result,
        || Ok(()),
        || panic!("a stopped service is not started"),
    )?;
    Ok(())
}
#[test]
fn an_interrupted_update_is_rolled_back_when_setup_starts() -> Result<()> {
    let root = tempfile::tempdir()?;
    let profile = root.path().join("config");
    let installed = root.path().join("installed");
    let backup = profile.join("updates/transaction-1-2/previous");
    std::fs::create_dir_all(&backup)?;
    std::fs::create_dir(&installed)?;
    std::fs::write(installed.join("host.exe"), b"previous")?;
    Backup::create(
        &installed,
        &backup,
        &["host.exe".into(), "new.dll".into()].into_iter().collect(),
    )?;
    std::fs::write(installed.join("host.exe"), b"half")?;
    std::fs::write(installed.join("new.dll"), b"new")?;
    let result = profile.join("update-result.json");
    write_record(
        &result,
        &json!({"version":"2.0.1","phase":"installing","error":null,
                "backup":backup,"install":installed}),
    )?;
    {
        // A setup that holds the lock is still at work.
        use std::os::windows::fs::OpenOptionsExt;
        let _held = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .share_mode(0)
            .open(profile.join("update.lock"))?;
        assert!(recover(&profile).is_err());
        assert_eq!(std::fs::read(installed.join("host.exe"))?, b"half");
    }
    recover(&profile)?;
    assert_eq!(std::fs::read(installed.join("host.exe"))?, b"previous");
    assert!(!installed.join("new.dll").exists());
    let written: serde_json::Value = serde_json::from_slice(&std::fs::read(&result)?)?;
    assert_eq!(written["phase"], "rolled_back");
    assert_eq!(written["version"], "2.0.1");
    // Before the backup was made, no file had changed.
    std::fs::write(installed.join("host.exe"), b"later")?;
    write_result(&result, "installing", None)?;
    recover(&profile)?;
    assert_eq!(std::fs::read(installed.join("host.exe"))?, b"later");
    Ok(())
}
#[test]
fn failed_recovery_keeps_the_backup_and_retries() -> Result<()> {
    let root = tempfile::tempdir()?;
    let install = root.path().join("install");
    let backup = root.path().join("backup");
    std::fs::create_dir(&install)?;
    std::fs::create_dir(&backup)?;
    std::fs::write(install.join("host.exe"), b"half")?;
    std::fs::write(backup.join("backup.json"), r#"[["host.exe",true]]"#)?;
    let result = root.path().join("update-result.json");
    write_record(
        &result,
        &json!({"version":"2.0.0-rc.22","phase":"installing",
        "backup":backup,"install":install}),
    )?;
    assert!(recover_locked(&result).is_err());
    let record: serde_json::Value = serde_json::from_slice(&std::fs::read(&result)?)?;
    assert_eq!(record["phase"], "recovery_failed");
    assert_eq!(record["backup"], json!(backup));
    assert_eq!(record["install"], json!(install));
    std::fs::write(backup.join("host.exe"), b"previous")?;
    recover_locked(&result)?;
    assert_eq!(std::fs::read(install.join("host.exe"))?, b"previous");
    std::fs::write(&result, b"broken record")?;
    assert!(recover_locked(&result).is_err());
    assert_eq!(std::fs::read(&result)?, b"broken record");
    Ok(())
}
#[test]
fn a_backup_that_cannot_be_restored_does_not_block_a_complete_reinstall() -> Result<()> {
    let root = tempfile::tempdir()?;
    let profile = root.path().join("config");
    let install = root.path().join("install");
    let backup = profile.join("updates/transaction-1-2/previous");
    std::fs::create_dir_all(&backup)?;
    std::fs::create_dir(&install)?;
    std::fs::write(install.join("host.exe"), b"half")?;
    // The backup lost a file, so every restore fails before changing any.
    std::fs::write(backup.join("backup.json"), r#"[["host.exe",true]]"#)?;
    let result = profile.join("update-result.json");
    write_record(
        &result,
        &json!({"version":"2.0.0-rc.22","phase":"installing","backup":backup,"install":install}),
    )?;
    for _ in 0..2 {
        assert!(recover_locked(&result).is_err());
    }
    // Setup goes on to install a complete package.
    recover(&profile)?;
    assert_eq!(std::fs::read(install.join("host.exe"))?, b"half");
    assert!(backup.join("backup.json").is_file());
    let record: serde_json::Value = serde_json::from_slice(&std::fs::read(&result)?)?;
    assert_eq!(record["phase"], "superseded");
    assert_eq!(record["previous_backup"], json!(backup));
    assert!(record["backup"].is_null());
    // Neither setup nor the service puts that backup back later.
    std::fs::write(backup.join("host.exe"), b"older")?;
    recover(&profile)?;
    assert_eq!(std::fs::read(install.join("host.exe"))?, b"half");
    // An unreadable record is replaced too.
    std::fs::write(&result, b"broken record")?;
    recover(&profile)?;
    let record: serde_json::Value = serde_json::from_slice(&std::fs::read(&result)?)?;
    assert_eq!(record["phase"], "superseded");
    Ok(())
}
#[test]
fn an_update_that_stops_early_records_why_unless_a_backup_is_pending() -> Result<()> {
    let root = tempfile::tempdir()?;
    let result = root.path().join("update-result.json");
    let error = anyhow::anyhow!("D:\\Games must be owned by Administrators")
        .context("Rubylight cannot be updated in D:\\Games\\Rubylight");
    let read =
        || -> Result<serde_json::Value> { Ok(serde_json::from_slice(&std::fs::read(&result)?)?) };
    let failed = json!({"version":env!("CARGO_PKG_VERSION"),"phase":"failed",
        "error":"Rubylight cannot be updated in D:\\Games\\Rubylight: D:\\Games must be owned by Administrators"});
    // No record yet, and the one the host writes before it starts setup.
    refuse(&result, &error)?;
    assert_eq!(read()?, failed);
    write_record(
        &result,
        &json!({"version":"v2.2.22","phase":"installing","started_at":1}),
    )?;
    refuse(&result, &error)?;
    assert_eq!(read()?, failed);
    // A record that names a backup still to be put back stays for the
    // rollback, and so does one that cannot be read.
    for phase in ["installing", "recovery_failed"] {
        let pending = json!({"version":"2.2.21","phase":phase,"error":null,
            "backup":root.path().join("previous"),"install":root.path().join("installed")});
        write_record(&result, &pending)?;
        assert!(refuse(&result, &error).is_err());
        assert_eq!(read()?, pending);
    }
    std::fs::write(&result, b"broken record")?;
    assert!(refuse(&result, &error).is_err());
    assert_eq!(std::fs::read(&result)?, b"broken record");
    Ok(())
}
/// `Butterpollo/config` under `root`, owned by Administrators whatever
/// owner this process's token gives the folders it creates (a user, not
/// Administrators, in some elevated shells).
fn trusted_profile(root: &Path) -> Result<PathBuf> {
    system::restrict(root, true)?;
    let profile = root.join("Butterpollo/config");
    std::fs::create_dir_all(&profile)?;
    system::restrict(profile.parent().unwrap(), true)?;
    system::restrict(&profile, true)?;
    Ok(profile)
}
#[test]
fn early_failures_are_recorded_only_in_a_trusted_profile_no_other_setup_holds() -> Result<()> {
    // The profile must be owned by Administrators, as setup leaves it.
    if !system::elevated() {
        return Ok(());
    }
    let temp = tempfile::tempdir_in(system::program_data())?;
    let profile = trusted_profile(temp.path())?;
    let result = profile.join("update-result.json");
    let host = json!({"version":"v2.2.22","phase":"installing","started_at":1});
    write_record(&result, &host)?;
    let error = anyhow::anyhow!("Disconnect all streams");
    let phase = || -> Result<serde_json::Value> {
        Ok(serde_json::from_slice::<serde_json::Value>(&std::fs::read(&result)?)?["phase"].clone())
    };
    {
        let _other = open_lock(&profile)?;
        record_refusal(&profile, None, &error);
        assert_eq!(phase()?, "installing");
    }
    // With update.lock this setup's own, or free.
    record_refusal(&profile, Some(open_lock(&profile)?), &error);
    assert_eq!(phase()?, "failed");
    write_record(&result, &host)?;
    record_refusal(&profile, None, &error);
    assert_eq!(phase()?, "failed");
    let written: serde_json::Value = serde_json::from_slice(&std::fs::read(&result)?)?;
    assert_eq!(written["error"], "Disconnect all streams");
    // A profile another user could change is not written as SYSTEM.
    write_record(&result, &host)?;
    let (_, identity) = system::run(
        &system::system32("whoami.exe"),
        &["/user", "/fo", "csv", "/nh"],
        Duration::from_secs(10),
    )?;
    let sid = identity
        .trim()
        .split(',')
        .nth(1)
        .context("whoami omitted SID")?
        .trim_matches('"');
    let (code, output) = system::run(
        &system::system32("icacls.exe"),
        &[
            &profile.parent().unwrap().display().to_string(),
            "/setowner",
            &format!("*{sid}"),
            "/Q",
        ],
        Duration::from_secs(10),
    )?;
    assert_eq!(code, 0, "{output}");
    record_refusal(&profile, None, &error);
    assert_eq!(phase()?, "installing");
    Ok(())
}
#[test]
fn a_pc_without_an_encoding_gpu_is_recorded_as_a_failed_update() -> Result<()> {
    // The profile must be owned by Administrators, as setup leaves it.
    if !system::elevated() {
        return Ok(());
    }
    let temp = tempfile::tempdir_in(system::program_data())?;
    let profile = trusted_profile(temp.path())?;
    let refusal = crate::encoder_gpu::Refusal::NoGpu;
    let text = refusal.to_string();
    // What main does for `--update` on a PC the check refuses.
    record_refusal(&profile, None, &anyhow::Error::new(refusal));
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(profile.join("update-result.json"))?)?;
    assert_eq!(written["phase"], "failed");
    assert_eq!(written["error"], text);
    assert!(!text.is_empty());
    Ok(())
}
#[test]
fn a_complete_installation_replaces_the_record_of_a_failed_update() -> Result<()> {
    let root = tempfile::tempdir()?;
    let result = root.path().join("update-result.json");
    let failed = json!({"version":"2.2.22","phase":"failed","error":"Rubylight cannot be updated"});
    write_record(&result, &failed)?;
    {
        let _other = open_lock(root.path())?;
        record_installed(root.path());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(&result)?)?,
            failed
        );
    }
    record_installed(root.path());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(&result)?)?,
        json!({"version":env!("CARGO_PKG_VERSION"),"phase":"installed","error":null})
    );
    Ok(())
}
#[test]
fn rollback_restores_removed_and_overwritten_files_and_removes_new_files() -> Result<()> {
    let root = tempfile::tempdir()?;
    let installed = root.path().join("installed");
    std::fs::create_dir(&installed)?;
    std::fs::write(installed.join("host.exe"), b"old executable")?;
    std::fs::write(installed.join("old.dll"), b"old dll")?;
    std::fs::write(installed.join("settings.conf"), b"user settings")?;
    let paths = ["host.exe", "old.dll", "new.dll"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let backup = Backup::create(&installed, &root.path().join("backup"), &paths)?;
    std::fs::write(installed.join("host.exe"), b"broken new executable")?;
    std::fs::remove_file(installed.join("old.dll"))?;
    std::fs::write(installed.join("new.dll"), b"new dll")?;
    backup.restore(&installed)?;
    assert_eq!(
        std::fs::read(installed.join("host.exe"))?,
        b"old executable"
    );
    assert_eq!(std::fs::read(installed.join("old.dll"))?, b"old dll");
    assert!(!installed.join("new.dll").exists());
    assert_eq!(
        std::fs::read(installed.join("settings.conf"))?,
        b"user settings"
    );
    Ok(())
}

#[test]
fn a_failed_installed_record_does_not_leave_a_rollback_behind() -> Result<()> {
    let root = tempfile::tempdir()?;
    let install = root.path().join("install");
    std::fs::create_dir(&install)?;
    let result = root.path().join("update-result.json");
    let installing = json!({"phase":"installing","backup":root.path().join("backup"),
                            "install":install});
    write_record(&result, &installing)?;
    // One failure is retried.
    let tries = std::cell::Cell::new(0);
    keep_installed_record(&result, || {
        tries.set(tries.get() + 1);
        if tries.get() == 1 {
            bail!("disk full");
        }
        write_result(&result, "installed", None)
    })?;
    assert_eq!(tries.get(), 2);
    // Two fail: the record that names the backup goes, and setup still fails.
    write_record(&result, &installing)?;
    let error = keep_installed_record(&result, || bail!("disk full")).unwrap_err();
    assert!(format!("{error:#}").contains("disk full"));
    assert!(!result.exists());
    assert_eq!(
        butterpollo_core::update_recovery::recover(root.path(), &install)?,
        None
    );
    Ok(())
}
#[test]
fn a_service_left_disabled_is_a_warning_on_the_installed_record() -> Result<()> {
    let root = tempfile::tempdir()?;
    let result = root.path().join("update-result.json");
    for (warning, expected) in [
        (Some(DISABLED_WARNING), json!(DISABLED_WARNING)),
        (None, serde_json::Value::Null),
    ] {
        finish_update(
            root.path(),
            &root.path().join("work"),
            &result,
            &[],
            &[],
            warning,
        )?;
        let record: serde_json::Value = serde_json::from_slice(&std::fs::read(&result)?)?;
        assert_eq!(record["phase"], "installed");
        assert!(record["error"].is_null());
        assert_eq!(record["warning"], expected);
    }
    Ok(())
}
/// Failed updates keep their backup in the installation folder's
/// staging folder now, where the host's updater no longer prunes them.
#[test]
fn only_the_two_newest_failed_update_transactions_are_kept() -> Result<()> {
    let staging = tempfile::tempdir()?;
    let names = [
        "transaction-9876-1700000000000000300",
        "transaction-12-1700000000000000100",
        "transaction-555-1700000000000000400",
        "transaction-7-1700000000000000200",
        "package",
        "transaction-x-1",
    ];
    for name in names {
        std::fs::create_dir_all(staging.path().join(name).join("previous"))?;
    }
    prune_transactions(staging.path());
    let mut left: Vec<String> = std::fs::read_dir(staging.path())?
        .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_>>()?;
    left.sort();
    assert_eq!(
        left,
        [
            "package",
            "transaction-555-1700000000000000400",
            "transaction-9876-1700000000000000300",
            "transaction-x-1",
        ]
    );
    Ok(())
}
#[test]
fn a_finished_transaction_takes_the_empty_staging_folder_with_it() -> Result<()> {
    let install = tempfile::tempdir()?;
    let staging = install::staging_root(install.path());
    let (kept, work) = (
        staging.join("transaction-1-1"),
        staging.join("transaction-1-2"),
    );
    std::fs::create_dir_all(kept.join("previous"))?;
    std::fs::create_dir_all(work.join("package"))?;
    remove_work(&work);
    assert!(!work.exists());
    assert!(kept.exists());
    std::fs::remove_dir_all(&kept)?;
    std::fs::create_dir_all(work.join("package"))?;
    remove_work(&work);
    assert!(!staging.exists());
    assert!(install.path().exists());
    Ok(())
}
