//! Unit tests for updater release validation, warnings, downloads, and staging.

use super::super::*;
use super::{Fixture, launch};

fn release() -> Value {
    let (repository, name) = ("Butterpollo", "butterpollo-setup-2.0.0-rc.7.exe");
    json!({"tag_name":"2.0.0-rc.7","assets":[{"name":name,"browser_download_url":format!("https://github.com/RamazanKara/{repository}/releases/download/2.0.0-rc.7/{name}"),"size":32,"digest":format!("sha256:{}", "ab".repeat(32))}]})
}
#[test]
fn the_installer_under_the_old_nvidia_name_is_never_taken() {
    // Former releases carried the signed installer under the old NVIDIA build's name too;
    // this host takes only its own names.
    let url = |name: &str| {
        format!("https://github.com/RamazanKara/Rubylight/releases/download/2.0.0-rc.7/{name}")
    };
    let mut release = release();
    release["assets"] = json!([]);
    for name in [
        "rubylight-nvidia-setup-2.0.0-rc.7.exe",
        "rubylight-setup-2.0.0-rc.7.exe",
    ] {
        let mut asset = self::release()["assets"][0].clone();
        asset["name"] = json!(name);
        asset["browser_download_url"] = json!(url(name));
        release["assets"].as_array_mut().unwrap().push(asset);
    }
    assert_eq!(
        installer(&release).unwrap().url,
        url("rubylight-setup-2.0.0-rc.7.exe")
    );
    release["assets"].as_array_mut().unwrap().pop();
    assert!(installer(&release).is_err());
}
#[test]
fn an_unresolved_rollback_cannot_be_overwritten_by_another_update() -> Result<()> {
    let f = Fixture::new();
    let path = f.directory.join("update-result.json");
    for phase in ["installing", "recovery_failed"] {
        let record = json!({"version":"2.0.0-rc.22","phase":phase,
            "backup":f.directory.join("updates/transaction-1-2/previous")});
        butterpollo_session::state::write_json(&path, &record)?;
        assert!(check_recovery(&f.directory).is_err());
        assert_eq!(
            butterpollo_session::state::load_json(&path, Value::Null)?,
            record
        );
    }
    for phase in ["installed", "rolled_back", "failed", "superseded"] {
        butterpollo_session::state::write_json(&path, &json!({"phase":phase}))?;
        assert!(check_recovery(&f.directory).is_ok());
    }
    std::fs::write(&path, b"{")?;
    let error = check_recovery(&f.directory).unwrap_err().to_string();
    assert!(error.contains("Run the Rubylight installer"), "{error}");
    Ok(())
}
#[test]
fn the_failed_update_warning_goes_once_that_release_is_installed() -> Result<()> {
    let f = Fixture::new();
    let path = f.directory.join("update-result.json");
    let shown = |warnings: &butterpollo_session::session::Warnings| {
        warnings
            .snapshot()
            .into_iter()
            .any(|warning| warning.code == UPDATE_FAILED)
    };
    // A release newer than this host failed: the warning stays, also across a restart.
    raise_failed_install(&f.host, "v999.0.0", "boom");
    butterpollo_session::state::write_json(
        &path,
        &json!({"version":"999.0.0","phase":"rolled_back","error":"boom"}),
    )?;
    withdraw_installed_warning(&f.host);
    assert!(shown(&f.host.warnings));
    let restarted = butterpollo_session::session::Warnings::default();
    crate::host_warnings::restore(&f.directory, &restarted);
    assert!(shown(&restarted));
    // Installed from Updates afterwards: gone now and at the next start.
    butterpollo_session::state::write_json(
        &path,
        &json!({"version":"999.0.0","phase":"installed","error":null}),
    )?;
    withdraw_installed_warning(&f.host);
    assert!(!shown(&f.host.warnings));
    let restarted = butterpollo_session::session::Warnings::default();
    crate::host_warnings::restore(&f.directory, &restarted);
    assert!(!shown(&restarted));
    assert!(f.host.aliases.lock().unwrap()["root"][FAILED_VERSION].is_null());
    // This host already runs a release at least as new: gone without any record.
    std::fs::remove_file(&path)?;
    raise_failed_install(&f.host, "v0.0.1", "boom");
    assert!(shown(&f.host.warnings));
    withdraw_installed_warning(&f.host);
    assert!(!shown(&f.host.warnings));
    Ok(())
}
#[test]
fn a_failed_automatic_install_warns_once_also_after_setup_restarted_the_host() -> Result<()> {
    let f = Fixture::new();
    let path = f.directory.join("update-result.json");
    let shown = || {
        f.host
            .warnings
            .snapshot()
            .into_iter()
            .filter(|warning| warning.code == UPDATE_FAILED)
            .map(|warning| warning.message)
            .collect::<Vec<_>>()
    };
    // No automatic install was started: a failed record is the console's to show.
    butterpollo_session::state::write_json(
        &path,
        &json!({"version":"9.9.9","phase":"failed","error":"boom"}),
    )?;
    report_failed_install(&f.host, None);
    assert!(shown().is_empty());
    remember_attempt(&f.host, Some("v9.9.9"));
    // What this host writes before it starts setup: setup still runs.
    butterpollo_session::state::write_json(
        &path,
        &json!({"version":"v9.9.9","phase":"installing","started_at":1}),
    )?;
    report_failed_install(&f.host, None);
    assert!(shown().is_empty());
    // Setup rolled back and started this host again, which reads the outcome.
    butterpollo_session::state::write_json(
        &path,
        &json!({"version":"9.9.9","phase":"rolled_back",
            "error":"Update failed; the previous version was restored. The service did not start."}),
    )?;
    report_failed_install(&f.host, None);
    assert_eq!(
        shown(),
        [
            "The automatic update to Rubylight 9.9.9 could not be installed: Update failed; the previous version was restored. The service did not start. This PC keeps the version it has; install the update from Updates in the console's settings when the PC is free."
        ]
    );
    assert!(f.host.aliases.lock().unwrap()["root"][AUTOMATIC_ATTEMPT].is_null());
    // Once: a later look changes nothing, and the warning stays until it is dismissed.
    crate::host_warnings::dismiss(&f.directory, &f.host.warnings, UPDATE_FAILED)?;
    report_failed_install(&f.host, None);
    assert!(shown().is_empty());
    // An install that succeeded is forgotten quietly.
    remember_attempt(&f.host, Some("v9.9.10"));
    butterpollo_session::state::write_json(
        &path,
        &json!({"version":"9.9.10","phase":"installed","error":null}),
    )?;
    report_failed_install(&f.host, None);
    assert!(shown().is_empty());
    assert!(f.host.aliases.lock().unwrap()["root"][AUTOMATIC_ATTEMPT].is_null());
    Ok(())
}
#[test]
fn a_setup_that_stops_early_reports_its_recorded_reason() -> Result<()> {
    use std::os::windows::process::ExitStatusExt;
    let f = Fixture::new();
    let path = f.directory.join("update-result.json");
    let exited = || Ok(std::process::ExitStatus::from_raw(1));
    // What this host writes before it starts setup.
    butterpollo_session::state::write_json(
        &path,
        &json!({"version":"v2.2.22","phase":"installing","started_at":1}),
    )?;
    let error = setup_failure(&f.directory, exited()).to_string();
    assert_eq!(
        error,
        "Installer exited before replacing this host (exit code 1). See the setup log."
    );
    let reason = "Rubylight cannot be updated in D:\\Games\\Rubylight: D:\\Games must be owned by Administrators, SYSTEM or TrustedInstaller.";
    butterpollo_session::state::write_json(
        &path,
        &json!({"version":"2.2.22","phase":"failed","error":reason}),
    )?;
    assert_eq!(setup_failure(&f.directory, exited()).to_string(), reason);
    Ok(())
}
#[tokio::test]
async fn busy_host_defers_download_before_contacting_the_network() {
    let f = Fixture::new();
    f.host
        .sessions
        .lock()
        .unwrap()
        .queue(launch(butterpollo_session::session::Role::Stream))
        .unwrap();
    let candidate = installer(&release()).unwrap();
    let error = transfer(&f.host, "test", &candidate, &f.directory.join("setup.exe"))
        .await
        .unwrap_err();
    assert!(error.is::<DownloadPaused>());
    assert!(!f.directory.join("setup.exe").exists());
    // A device's Install now pauses beside another device's stream too.
    *f.host.updates.lock().unwrap() = json!({"phase":"downloading","now":true,"now_by":"phone"});
    let error = transfer(&f.host, "test", &candidate, &f.directory.join("setup.exe"))
        .await
        .unwrap_err();
    assert!(error.is::<DownloadPaused>());
    assert!(!f.directory.join("setup.exe").exists());
}
#[test]
fn only_exact_release_assets_with_digests_can_be_installed() {
    assert!(installer(&release()).is_ok());
    for (key, value) in [
        (
            "browser_download_url",
            json!("https://example.org/setup.exe"),
        ),
        ("digest", Value::Null),
        ("size", json!(MAX_INSTALLER + 1)),
        ("name", json!("butterpollo-setup-2.0.0-rc.7-candidate.exe")),
    ] {
        let mut release = release();
        release["assets"][0][key] = value;
        assert!(installer(&release).is_err(), "{key}");
    }
}
#[test]
fn installers_from_before_and_after_the_rename_are_accepted() {
    for (repository, name) in [
        ("Rubylight", "rubylight-setup-2.0.0-rc.7.exe"),
        ("Rubylight", "butterpollo-setup-2.0.0-rc.7.exe"),
        ("Butterpollo", "rubylight-setup-2.0.0-rc.7.exe"),
        ("Butterpollo", "butterpollo-setup-2.0.0-rc.7.exe"),
    ] {
        let url = format!(
            "https://github.com/RamazanKara/{repository}/releases/download/2.0.0-rc.7/{name}"
        );
        let mut release = release();
        release["assets"][0]["name"] = json!(name);
        release["assets"][0]["browser_download_url"] = json!(url);
        assert_eq!(installer(&release).unwrap().url, url);
    }
    // The current name wins when a release carries both.
    let mut both = release();
    let mut current = both["assets"][0].clone();
    current["name"] = json!("rubylight-setup-2.0.0-rc.7.exe");
    current["browser_download_url"] = json!(
        "https://github.com/RamazanKara/Rubylight/releases/download/2.0.0-rc.7/rubylight-setup-2.0.0-rc.7.exe"
    );
    both["assets"].as_array_mut().unwrap().push(current);
    assert!(
        installer(&both)
            .unwrap()
            .url
            .ends_with("/rubylight-setup-2.0.0-rc.7.exe")
    );
    // Another repository's copy is refused.
    let mut foreign = release();
    foreign["assets"][0]["browser_download_url"] = json!(
        "https://github.com/someone/Rubylight/releases/download/2.0.0-rc.7/butterpollo-setup-2.0.0-rc.7.exe"
    );
    assert!(installer(&foreign).is_err());
}
#[test]
fn only_the_two_newest_update_transactions_are_kept() {
    let names: Vec<String> = [
        "transaction-9876-1700000000000000300",
        "transaction-12-1700000000000000100",
        "transaction-4-1700000000000000400",
        "transaction-123456-1700000000000000200",
        "transaction-x-1700000000000000000",
        "transaction-5-",
        "backup",
    ]
    .map(String::from)
    .into();
    let mut stale = stale_transactions(&names);
    stale.sort_unstable();
    assert_eq!(
        stale,
        [
            "transaction-12-1700000000000000100",
            "transaction-123456-1700000000000000200"
        ]
    );
    assert!(stale_transactions(&names[..2]).is_empty());
}
#[test]
fn truncated_or_changed_installer_is_rejected() {
    let candidate = installer(&release()).unwrap();
    assert!(verify(32, &candidate.digest, &candidate).is_ok());
    assert!(verify(31, &candidate.digest, &candidate).is_err());
    assert!(verify(32, &"00".repeat(32), &candidate).is_err());
}
#[test]
fn a_download_that_ends_early_is_retried_not_refused() {
    let candidate = installer(&release()).unwrap();
    assert!(arrived(32, &candidate).is_ok());
    // More than advertised is the transfer's own refusal, not this check's.
    assert!(arrived(33, &candidate).is_ok());
    let short = arrived(31, &candidate).unwrap_err();
    assert!(!short.is::<InstallerRejected>());
    let mut state = json!({"phase":"downloading","attempted_version":candidate.version});
    finish_download(&mut state, Err(short));
    assert_eq!(state["phase"], "failed");
    assert!(state["attempted_version"].is_null());
}
#[test]
fn a_broken_off_download_is_retried_but_a_rejected_installer_is_not() {
    let candidate = installer(&release()).unwrap();
    let downloading = || json!({"phase":"downloading","attempted_version":candidate.version});
    // The network dropped mid-transfer: the next check may queue it again.
    let mut state = downloading();
    finish_download(&mut state, Err(anyhow::anyhow!("connection reset")));
    assert_eq!(state["phase"], "failed");
    assert!(state["attempted_version"].is_null());
    // A wrong size or digest keeps that version from downloading again.
    let mut state = downloading();
    finish_download(
        &mut state,
        verify(31, &candidate.digest, &candidate).map(|()| PathBuf::new()),
    );
    assert_eq!(state["phase"], "failed");
    assert_eq!(state["attempted_version"], candidate.version);
    assert_eq!(
        state["error"],
        "Installer verification failed; no files were installed"
    );
    // A paused download waits and keeps its place.
    let mut state = downloading();
    finish_download(&mut state, Err(DownloadPaused.into()));
    assert_eq!(state["phase"], "waiting");
    assert_eq!(state["attempted_version"], candidate.version);
}
#[test]
fn a_paused_download_resumes_only_the_same_installer_and_only_from_a_matching_range() -> Result<()>
{
    let f = Fixture::new();
    let candidate = installer(&release()).unwrap();
    let path = f.directory.join("download-kept").join("setup.exe");
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, [7u8; 12])?;
    let kept =
        json!({"path":path,"url":candidate.url,"size":candidate.size,"digest":candidate.digest});
    assert_eq!(resumable(&kept, &candidate), Some(path.clone()));
    // Another release, address or digest starts over, and so does nothing kept.
    for (key, value) in [
        (
            "url",
            json!("https://github.com/RamazanKara/Rubylight/releases/download/x/other.exe"),
        ),
        ("size", json!(candidate.size + 1)),
        ("digest", json!("00".repeat(32))),
    ] {
        let mut other = kept.clone();
        other[key] = value;
        assert_eq!(resumable(&other, &candidate), None, "{key}");
    }
    assert_eq!(resumable(&Value::Null, &candidate), None);
    // A file already as long as the installer has nothing left to ask for.
    std::fs::write(&path, vec![7u8; candidate.size as usize])?;
    assert_eq!(resumable(&kept, &candidate), None);
    // Only a 206 with the rest of this installer resumes; a server ignoring the range (200) or
    // answering another range starts over.
    let size = candidate.size;
    let rest = format!("bytes 12-{}/{size}", size - 1);
    assert!(resumes(206, Some(&rest), 12, size));
    assert!(!resumes(200, Some(&rest), 12, size));
    assert!(!resumes(206, None, 12, size));
    assert!(!resumes(
        206,
        Some(&format!("bytes 0-{}/{size}", size - 1)),
        12,
        size
    ));
    assert!(!resumes(
        206,
        Some(&format!("bytes 12-{}/*", size - 1)),
        12,
        size
    ));
    // The kept part hashed, then the rest: the whole file's digest, checked at the end as before.
    let whole: Vec<u8> = (0..64u8).collect();
    std::fs::write(&path, &whole[..20])?;
    let mut hash = hash_prefix(&path, 20)?;
    hash.update(&whole[20..]);
    assert_eq!(
        hex::encode(hash.finalize()),
        hex::encode(Sha256::digest(&whole))
    );
    assert!(hash_prefix(&path, 21).is_err());
    // A paused download keeps its place for the console too.
    let mut state = json!({"phase":"downloading","downloaded_bytes":20});
    finish_download(&mut state, Err(DownloadPaused.into()));
    assert_eq!(state["phase"], "waiting");
    assert_eq!(state["downloaded_bytes"], 20);
    Ok(())
}
/// A temporary folder, removed with what is in it.
struct Temporary(PathBuf);
impl Temporary {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "butterpollo-download-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn elevated() -> bool {
    butterpollo_win_io::process::elevated().unwrap_or(false)
}
/// The host downloaded setup.exe into ProgramData\Butterpollo\config\
/// updates and ran it elevated from there, which Defender scores as
/// malware. It downloads into the installation folder's staging folder,
/// admin-only from the start, and removes earlier downloads first.
#[test]
fn the_installer_is_downloaded_into_the_installation_folder() -> Result<()> {
    if !elevated() {
        return Ok(());
    }
    let temporary = Temporary::new();
    let install = temporary.0.join("Rubylight");
    std::fs::create_dir(&install)?;
    let staging = install.join(".staging");
    let earlier = staging.join(format!("download-{}", uuid::Uuid::new_v4()));
    let backup = staging.join("transaction-1-2").join("previous");
    std::fs::create_dir_all(&earlier)?;
    std::fs::create_dir_all(&backup)?;
    std::fs::write(earlier.join("setup.exe"), b"earlier")?;
    std::fs::write(backup.join("butterpollo.exe"), b"previous")?;
    let id = uuid::Uuid::new_v4().to_string();
    let directory = prepare_download(&install, &id)?;
    assert_eq!(directory, staging.join(format!("download-{id}")));
    assert!(!directory.starts_with(butterpollo_session::paths::installed_profile()));
    assert_eq!(std::fs::read_dir(&directory)?.count(), 0);
    assert!(!earlier.exists());
    assert_eq!(std::fs::read(backup.join("butterpollo.exe"))?, b"previous");
    std::fs::write(directory.join("setup.exe"), b"downloaded")?;
    // A failed or finished download goes; a failed update's backup stays.
    remove_download(&directory);
    assert!(!directory.exists());
    assert!(backup.exists());
    std::fs::remove_dir_all(staging.join("transaction-1-2"))?;
    let directory = prepare_download(&install, &uuid::Uuid::new_v4().to_string())?;
    remove_download(&directory);
    assert!(!staging.exists());
    assert!(install.exists());
    Ok(())
}
#[test]
fn a_link_at_or_in_the_staging_folder_is_refused_not_followed() -> Result<()> {
    if !elevated() {
        return Ok(());
    }
    let temporary = Temporary::new();
    let install = temporary.0.join("Rubylight");
    std::fs::create_dir(&install)?;
    let outside = temporary.0.join("outside");
    let planted = outside.join(format!("download-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&planted)?;
    std::fs::write(planted.join("setup.exe"), b"unchanged")?;
    let staging = install.join(".staging");
    for link in [staging.clone(), staging.join("package")] {
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::os::windows::fs::symlink_dir(&outside, &link)?;
        let error = prepare_download(&install, &uuid::Uuid::new_v4().to_string())
            .err()
            .context("a link was followed")?;
        assert!(format!("{error:#}").contains("reparse point"), "{error:#}");
        assert_eq!(std::fs::read(planted.join("setup.exe"))?, b"unchanged");
        std::fs::remove_dir(&link)?;
    }
    Ok(())
}
#[test]
fn a_download_a_setup_still_runs_from_is_left_for_later() -> Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    let staging = Temporary::new();
    let running = staging.0.join(format!("download-{}", uuid::Uuid::new_v4()));
    let done = staging.0.join(format!("download-{}", uuid::Uuid::new_v4()));
    for folder in [&running, &done] {
        std::fs::create_dir(folder)?;
        std::fs::write(folder.join("setup.exe"), b"setup")?;
    }
    for other in ["package", "transaction-1-2", "download-not-a-uuid"] {
        std::fs::create_dir(staging.0.join(other))?;
    }
    // As Windows holds a running program.
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1)
        .open(running.join("setup.exe"))?;
    assert_eq!(remove_downloads(&staging.0), 1);
    assert!(!done.exists());
    assert!(running.join("setup.exe").exists());
    drop(held);
    assert_eq!(remove_downloads(&staging.0), 0);
    let mut left: Vec<String> = std::fs::read_dir(&staging.0)?
        .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_>>()?;
    left.sort();
    assert_eq!(left, ["download-not-a-uuid", "package", "transaction-1-2"]);
    Ok(())
}
#[test]
fn downloads_left_in_the_profile_are_removed() -> Result<()> {
    let profile = Temporary::new();
    let updates = profile.0.join("updates");
    let download = updates.join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&download)?;
    std::fs::write(download.join("setup.exe"), b"earlier")?;
    let transactions = [
        "transaction-1-100",
        "transaction-2-400",
        "transaction-3-300",
        "transaction-4-200",
    ];
    for name in transactions {
        std::fs::create_dir_all(updates.join(name).join("previous"))?;
    }
    std::fs::create_dir(updates.join("other"))?;
    remove_legacy_downloads(&updates);
    let mut left: Vec<String> = std::fs::read_dir(&updates)?
        .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_>>()?;
    left.sort();
    assert_eq!(left, ["other", "transaction-2-400", "transaction-3-300"]);
    for name in left {
        std::fs::remove_dir_all(updates.join(name))?;
    }
    std::fs::create_dir(&download)?;
    remove_legacy_downloads(&updates);
    assert!(!updates.exists());
    if elevated() {
        // A link there goes itself; what it leads to stays.
        let outside = profile.0.join("outside");
        std::fs::create_dir_all(&updates)?;
        std::fs::create_dir(&outside)?;
        std::fs::write(outside.join("file"), b"unchanged")?;
        std::os::windows::fs::symlink_dir(&outside, &download)?;
        remove_legacy_downloads(&updates);
        assert!(std::fs::symlink_metadata(&download).is_err());
        assert_eq!(std::fs::read(outside.join("file"))?, b"unchanged");
    }
    Ok(())
}

#[test]
fn an_update_the_host_started_but_setup_never_took_over_ends_as_failed() -> Result<()> {
    let f = Fixture::new();
    let install = f.directory.join("Rubylight");
    std::fs::create_dir_all(&install)?;
    let path = f.directory.join("update-result.json");
    // While the spawned setup process is still alive (even before taking update.lock),
    // recovery leaves the record alone instead of marking it failed.
    let me = std::process::id();
    let started = butterpollo_win_io::process::creation_time(me);
    assert_ne!(started, 0);
    let live = installing_record("9.9.9", &install, Some((me, started)));
    assert_eq!(live["setup_pid"], me);
    assert_eq!(live["setup_started"], started);
    butterpollo_session::state::write_json(&path, &live)?;
    assert_eq!(
        butterpollo_session::update_recovery::recover(&f.directory, &install)?,
        None
    );
    assert_eq!(
        butterpollo_session::state::load_json(&path, Value::Null)?["phase"],
        "installing"
    );

    // Once setup is gone (or a legacy record has no setup identity), recovery ends it as failed.
    butterpollo_session::state::write_json(&path, &installing_record("9.9.9", &install, None))?;
    // Another installation's service leaves it alone.
    let other = f.directory.join("other");
    std::fs::create_dir_all(&other)?;
    assert_eq!(
        butterpollo_session::update_recovery::recover(&f.directory, &other)?,
        None
    );
    assert_eq!(
        butterpollo_session::update_recovery::recover(&f.directory, &install)?.as_deref(),
        Some(butterpollo_session::update_recovery::STOPPED_BEFORE_BACKUP)
    );
    let record = butterpollo_session::state::load_json(&path, Value::Null)?;
    assert_eq!(record["phase"], "failed");
    assert_eq!(record["version"], "9.9.9");
    Ok(())
}
