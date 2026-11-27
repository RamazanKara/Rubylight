//! Only release metadata from our fixed GitHub repository can select an installer.
//! Installation is limited to the installed service and serialized with launches.
use crate::state::Shared;
use anyhow::{Context, Result, bail};
use butterpollo_session::version::newer;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::atomic::Ordering,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

mod download;
mod release;
mod schedule;
mod warnings;

#[cfg(test)]
use release::MAX_INSTALLER;
pub(crate) use release::REPOSITORIES;
use release::client_with;
pub use release::{Installer, client, installer, releases};

#[cfg(test)]
use warnings::{AUTOMATIC_ATTEMPT, FAILED_VERSION, LAST_CHECK};
use warnings::{
    NEXT_REPORT, raise_failed_install, remember_attempt, remembered_check, report_failed_install,
    setup_failure, withdraw_installed_warning,
};
pub use warnings::{UPDATE_FAILED, remember_check};

#[cfg(test)]
use download::{
    DownloadPaused, InstallerRejected, arrived, hash_prefix, prepare_download, remove_downloads,
    remove_legacy_downloads, resumable, resumes, stale_transactions, transfer,
};
use download::{
    NEXT_TIDY, check_recovery, download, failed, finish_download, remove_download, tidy_downloads,
    tidy_soon, verify,
};

const IDLE_SECONDS: u64 = 60;

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// The host installs its own updates: an installed Windows service.
pub fn install_supported(h: &Shared) -> bool {
    supported(h)
}

fn supported(h: &Shared) -> bool {
    let profile = butterpollo_session::paths::installed_profile();
    butterpollo_win_io::process::is_system()
        && std::fs::canonicalize(&h.directory)
            .ok()
            .zip(std::fs::canonicalize(profile).ok())
            .is_some_and(|(a, b)| a == b)
}

pub fn status(h: &Shared) -> Value {
    let mut state = h.updates.lock().unwrap().clone();
    state["install_supported"] = json!(supported(h));
    state["updates"] = json!(match h.config.read().unwrap().updates() {
        butterpollo_config::config::UpdatesMode::Off => "off",
        butterpollo_config::config::UpdatesMode::TellMe => "tell_me",
        butterpollo_config::config::UpdatesMode::Install => "install",
    });
    state["auto_update"] = json!(
        h.config.read().unwrap().updates() == butterpollo_config::config::UpdatesMode::Install
    );
    // 0 when automatic checks are off.
    state["update_check_interval"] = json!(h.config.read().unwrap().update_check_interval());
    // "HH:MM" when the check runs once a day at that time; null when the
    // interval decides.
    state["update_check_time"] = json!(
        h.config
            .read()
            .unwrap()
            .update_check_time()
            .map(|time| format!("{:02}:{:02}", time / 3600, time % 3600 / 60))
    );
    // Until this run has checked, the console shows when the last one did.
    if state["checked_at"].as_u64().unwrap_or(0) == 0 {
        state["previous_check_at"] = json!(Some(remembered_check(h)).filter(|at| *at > 0));
    }
    // Null when automatic checks are off.
    state["next_check_at"] = json!(next_check(h));
    // Due, but held back by a stream; the console says "After the stream".
    state["deferred"] = json!(streaming(h) && next_check(h).is_some_and(|at| now() >= at));
    // A device's Install now held back by another device's stream.
    state["waiting_for"] = json!(held_for_others(h).map(|busy| busy.message()));
    state["last_install"] =
        butterpollo_session::state::load_json(&h.directory.join("update-result.json"), json!(null))
            .unwrap_or(Value::Null);
    state
}

/// The floor for the update check interval, as the console enforces it.
const MIN_CHECK_INTERVAL: i64 = 3600;
/// How far the clock may step back (time sync) before it counts as set back.
const CLOCK_SLACK: u64 = 300;
/// After a failed check (typically the network is not back yet after the PC
/// woke from sleep) try again soon, not after a whole interval; repeated
/// failures back off (2 min, 4, 8, ...) up to the interval.
const RETRY_AFTER_FAILURE: u64 = 120;

/// Seconds to wait after `failures` consecutive failed checks (at least 1).
fn retry_delay(failures: u64, interval: u64) -> u64 {
    let shift = failures.saturating_sub(1).min(32) as u32;
    (RETRY_AFTER_FAILURE << shift).min(interval)
}

/// When the next automatic check is due, as Unix seconds of the wall clock,
/// so sleep counts as elapsed time and a changed interval applies at once.
/// `None` when automatic checks are off; 0 when one is due right away.
pub fn next_check_at(interval: i64, checked_at: u64, failures: u64, now: u64) -> Option<u64> {
    if interval <= 0 {
        return None;
    }
    // Never checked, or the clock was set back well past the last check; a
    // step of a few minutes (time sync) must not trigger a check.
    if checked_at == 0 || checked_at > now + CLOCK_SLACK {
        return Some(0);
    }
    let interval = interval.max(MIN_CHECK_INTERVAL) as u64;
    let wait = if failures > 0 {
        retry_delay(failures, interval)
    } else {
        interval
    };
    Some(checked_at + wait)
}

fn next_check(h: &Shared) -> Option<u64> {
    let schedule = {
        let config = h.config.read().unwrap();
        schedule::Schedule {
            interval: config.update_check_interval(),
            time: config.update_check_time(),
        }
    };
    let (mut checked_at, failures) = {
        let state = h.updates.lock().unwrap();
        (
            state["checked_at"].as_u64().unwrap_or(0),
            state["check_failures"].as_u64().unwrap_or(0),
        )
    };
    // With an interval the host checks whenever it starts. With a time of day
    // it must not, or a PC switched on in the evening would check and install
    // then: the last check from before the restart counts.
    if checked_at == 0 && schedule.time.is_some() {
        checked_at = remembered_check(h);
    }
    schedule::next_at(schedule, checked_at, failures, now(), &local_clock)
}

/// What this PC's clock shows at a moment, as Unix seconds as if it were UTC.
fn local_clock(at: u64) -> i64 {
    butterpollo_win_io::local_time::utc_to_local(at).unwrap_or(at as i64)
}

/// A client is streaming or about to; a game left running does not count.
pub fn streaming(h: &Shared) -> bool {
    let sessions = h.sessions.lock().unwrap();
    !sessions.active.is_empty() || !sessions.pending.is_empty()
}

/// Whether the maintenance loop should look for a release now. A stream
/// defers it (the loop asks again in a second); a game left running with no
/// client connected does not.
pub fn check_due(h: &Shared) -> bool {
    if h.updates.lock().unwrap()["checking"] == true {
        return false;
    }
    !streaming(h) && next_check(h).is_some_and(|at| now() >= at)
}

/// How long the loop sleeps before it looks for a release again: a check
/// already due (a stream or one still running defers it) a second, as its
/// poll asked; the remaining interval otherwise, capped at a day so an
/// interval longer than that is looked at again rather than slept past.
fn next_wait(due: u64, now: u64) -> Duration {
    if due <= now {
        return Duration::from_secs(1);
    }
    Duration::from_secs((due - now).min(86_400))
}

/// When the maintenance loop next looks for a release, as its own deadline
/// instead of a poll: `None` when no automatic check is scheduled. The loop
/// re-reads it on every wake, so a PC that slept checks on the first wake
/// after, and a save's wake catches a changed interval at once.
pub fn next_deadline(h: &Shared) -> Option<Instant> {
    let due = next_check(h)?;
    Some(Instant::now() + next_wait(due, now()))
}

/// Whether `poll` has work beyond an update in flight: the download tidy or
/// the install report, each on its own cadence. The maintenance loop starts
/// a task for it only then, so a resting host spawns none.
pub fn housekeeping_due() -> bool {
    let now = now();
    NEXT_TIDY.load(Ordering::Relaxed) <= now || NEXT_REPORT.load(Ordering::Relaxed) <= now
}

/// When [`housekeeping_due`] next turns true, for the idle loop's sleep.
pub fn housekeeping_at() -> Instant {
    let now = now();
    let due = NEXT_TIDY
        .load(Ordering::Relaxed)
        .min(NEXT_REPORT.load(Ordering::Relaxed));
    Instant::now() + Duration::from_secs(due.saturating_sub(now).min(86_400))
}

pub fn installing(h: &Shared) -> bool {
    h.updates.lock().unwrap()["phase"] == "installing"
}

pub fn busy(h: &Shared) -> bool {
    let sessions = h.sessions.lock().unwrap();
    let busy = !sessions.active.is_empty() || !sessions.pending.is_empty();
    drop(sessions);
    busy || h.current_app.lock().unwrap().is_some() || !h.monitors.lock().unwrap().is_empty()
}

/// Queue the latest release. `immediately` is the user's "Install now": it does not
/// wait for the host to be idle and ends a running stream, which the client
/// can start again once Rubylight is back. Automatic updates always wait.
/// `by` is the device whose Install now this is (`None` for the console):
/// it ends only that device's stream and waits while another device streams
/// ([`held_for_others`]). Whether this queued the update or made it install
/// now; false when it already did.
pub fn queue(h: &Shared, automatic: bool, immediately: bool, by: Option<&str>) -> Result<bool> {
    if !supported(h) {
        bail!("Install Rubylight as a Windows service to use in-app updates");
    }
    let config = h.config.read().unwrap();
    let prereleases = config.boolean("notify_pre_releases", false);
    drop(config);
    let mut state = h.updates.lock().unwrap();
    if matches!(
        state["phase"].as_str(),
        Some("waiting" | "downloading" | "ready" | "installing")
    ) {
        return Ok(immediately && install_now_again(&mut state, by));
    }
    if state["check_failed"] == true {
        bail!("Check for updates successfully before installing");
    }
    let version = state["latest_version"]
        .as_str()
        .context("No newer release is available")?;
    let release = state["releases"]
        .as_array()
        .and_then(|r| r.iter().find(|r| r["tag_name"] == version))
        .context("Check for updates again")?;
    if release["prerelease"] == true && !prereleases {
        bail!("Pre-release updates are disabled");
    }
    if !newer(version, env!("CARGO_PKG_VERSION")) {
        bail!("This version is already installed");
    }
    let candidate = installer(release)?;
    let last = check_recovery(&h.directory)?;
    if automatic {
        // Never loop on a bad release after a rollback or interrupted installer.
        if last["version"]
            .as_str()
            .is_some_and(|v| v.trim_start_matches('v') == candidate.version.trim_start_matches('v'))
        {
            return Ok(false);
        }
        if state["attempted_version"] == candidate.version {
            return Ok(false);
        }
    }
    state["phase"] = json!("waiting");
    state["queued_version"] = json!(candidate.version);
    state["automatic"] = json!(automatic);
    let now = immediately && !automatic;
    state["now"] = json!(now);
    state["now_by"] = json!(by.filter(|_| now));
    state["idle_since"] = json!(0);
    state["error"] = Value::Null;
    drop(state);
    // The maintenance loop takes a queued update on from here.
    h.wake_maintenance();
    Ok(true)
}

/// "Install now" on an update already queued: whether it changed anything.
/// The console's own (`by` None) ends any stream and stays so; a device's
/// ends only its own.
fn install_now_again(state: &mut Value, by: Option<&str>) -> bool {
    if state["phase"] == "installing" {
        return false;
    }
    let already = state["now"] == true;
    if already && (state["now_by"].is_null() || state["now_by"].as_str() == by) {
        return false;
    }
    state["now"] = json!(true);
    state["now_by"] = json!(by);
    state["automatic"] = json!(false);
    true
}

/// What a device's Install now waits for: another device that streams or
/// starts a stream, whose stream it never ends. `None` for the console's
/// Install now and for an update that waits for an idle host anyway.
pub fn held_for_others(h: &Shared) -> Option<butterpollo_session::power_action::Busy> {
    let state = h.updates.lock().unwrap();
    if state["now"] != true
        || !matches!(
            state["phase"].as_str(),
            Some("waiting" | "downloading" | "ready")
        )
    {
        return None;
    }
    let by = state["now_by"].as_str()?.to_owned();
    drop(state);
    crate::power_action::streaming(h, Some(&by))
}

pub fn cancel(h: &Shared) -> Result<()> {
    let mut state = h.updates.lock().unwrap();
    if state["phase"] == "installing" {
        bail!("Installation has already started");
    }
    state["phase"] = json!("idle");
    state["now"] = json!(false);
    state["now_by"] = Value::Null;
    // A generation token prevents a cancelled download from publishing its result.
    state["download_id"] = Value::Null;
    state["queued_version"] = Value::Null;
    state["attempted_version"] = state["latest_version"].clone();
    // A cancelled download is not resumed: its folder goes at the next tidy.
    state["partial"] = Value::Null;
    tidy_soon();
    Ok(())
}

/// Called by maintenance. All admission checks and the final handoff share the
/// Moonlight launch lock, so a connection cannot slip between the check and setup.
pub fn poll(h: &Shared) {
    tidy_downloads(h);
    let now = now();
    if NEXT_REPORT.load(Ordering::Relaxed) <= now {
        NEXT_REPORT.store(now + 30, Ordering::Relaxed);
        withdraw_installed_warning(h);
        report_failed_install(h, None);
    }
    poll_with(h, None, &hash_installer)
}

type InstallerHash = std::io::Result<(u64, String)>;
/// The ready installer, hashed with no lock held.
struct Hashed {
    download_id: Value,
    path: PathBuf,
    digest: InstallerHash,
}
fn hash_installer(path: &std::path::Path) -> InstallerHash {
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let size = std::io::copy(&mut file, &mut hash)?;
    Ok((size, hex::encode(hash.finalize())))
}

fn poll_with(h: &Shared, hashed: Option<Hashed>, hash: &dyn Fn(&std::path::Path) -> InstallerHash) {
    let Ok(transition) = h.launch_transition.try_lock() else {
        return;
    };
    let phase = h.updates.lock().unwrap()["phase"]
        .as_str()
        .unwrap_or("idle")
        .to_owned();
    if !matches!(phase.as_str(), "waiting" | "ready" | "downloading") {
        return;
    }
    let config = h.config.read().unwrap();
    let auto_enabled = config.updates() == butterpollo_config::config::UpdatesMode::Install
        && config.update_check_interval() > 0;
    let prereleases = config.boolean("notify_pre_releases", false);
    drop(config);
    let mut state = h.updates.lock().unwrap();
    if state["phase"] != phase {
        return;
    }
    let release = state["releases"]
        .as_array()
        .and_then(|r| r.iter().find(|r| r["tag_name"] == state["queued_version"]))
        .cloned();
    if (state["automatic"] == true && !auto_enabled)
        || release
            .as_ref()
            .is_some_and(|r| r["prerelease"] == true && !prereleases)
    {
        state["phase"] = json!("idle");
        state["download_id"] = Value::Null;
        // A half-finished download is not resumed: its folder goes at the next
        // tidy, which skips a download it believes is still running.
        state["partial"] = Value::Null;
        tidy_soon();
        return;
    }
    drop(state);
    let is_busy = busy(h);
    let held = held_for_others(h);
    let mut state = h.updates.lock().unwrap();
    if state["phase"] != phase {
        return;
    }
    let install_now = state["now"] == true;
    if let Some(busy) = held {
        tracing::debug!(
            busy = busy.message(),
            "update: a device's Install now waits for another device's stream"
        );
        return;
    }
    if !install_now {
        if is_busy {
            state["idle_since"] = json!(0);
            return;
        }
        let idle_since = state["idle_since"].as_u64().unwrap_or(0);
        if idle_since == 0 {
            state["idle_since"] = json!(now());
            return;
        }
        if now().saturating_sub(idle_since) < IDLE_SECONDS {
            return;
        }
    }
    if phase == "downloading" {
        return;
    }
    if phase == "waiting" {
        let candidate = release
            .as_ref()
            .context("release metadata is unavailable")
            .and_then(installer);
        let candidate = match candidate {
            Ok(c) => c,
            Err(error) => {
                failed(&mut state, error);
                return;
            }
        };
        let id = uuid::Uuid::new_v4().to_string();
        state["phase"] = json!("downloading");
        state["download_id"] = json!(id);
        state["downloaded_bytes"] = json!(0);
        state["download_size"] = json!(candidate.size);
        state["attempted_version"] = json!(candidate.version);
        drop(state);
        let h = h.clone();
        tokio::spawn(async move {
            let result = download(&h, &id, &candidate).await;
            let mut state = h.updates.lock().unwrap();
            if state["download_id"] != id {
                drop(state);
                if let Ok(path) = result {
                    remove_download(path.parent().unwrap());
                }
                return;
            }
            finish_download(&mut state, result);
            // A failure that keeps its version attempted (an installer that does not match its
            // release) is not downloaded again automatically: the console says so.
            if state["phase"] == "failed"
                && state["automatic"] == true
                && let Some(version) = state["attempted_version"].as_str()
            {
                let (version, reason) = (
                    version.to_owned(),
                    state["error"].as_str().unwrap_or_default().to_owned(),
                );
                drop(state);
                raise_failed_install(&h, &version, &reason);
            }
        });
    } else {
        // Hashing up to MAX_INSTALLER bytes holds neither lock, so status
        // reads and Moonlight launches don't wait on the disk. Every check
        // above runs again after it.
        let path = state["installer_path"].as_str().map(PathBuf::from);
        let hashed = match (hashed, path) {
            (Some(hashed), Some(path))
                if hashed.download_id == state["download_id"] && hashed.path == path =>
            {
                Some(hashed.digest)
            }
            // Replaced or cancelled meanwhile: the next poll hashes again.
            (Some(_), _) => return,
            (None, Some(path)) => {
                let download_id = state["download_id"].clone();
                drop(state);
                drop(transition);
                let digest = hash(&path);
                let hashed = Hashed {
                    download_id,
                    path,
                    digest,
                };
                return poll_with(h, Some(hashed), hash);
            }
            (None, None) => None,
        };
        if state["automatic"] == true
            && let Some(version) = state["queued_version"].as_str()
        {
            remember_attempt(h, Some(version));
        }
        let result = (|| -> Result<()> {
            if !supported(h) {
                bail!("Updates require the installed Windows service");
            }
            let path = PathBuf::from(
                state["installer_path"]
                    .as_str()
                    .context("installer missing")?,
            );
            let candidate = installer(release.as_ref().context("release missing")?)?;
            let (size, digest) = hashed.context("installer missing")??;
            verify(size, &digest, &candidate)?;
            let install = std::env::current_exe()?
                .parent()
                .context("installation folder missing")?
                .to_path_buf();
            // A persistent attempt record also prevents automatic retry loops.
            check_recovery(&h.directory)?;
            let update_lock = open_update_lock(&h.directory)?;
            let download = path.parent().map(Path::to_path_buf);
            let mut command = std::process::Command::new(path);
            use std::os::windows::process::CommandExt;
            // The installer must survive the service stopping its host job.
            command
                .creation_flags(0x01000000 | 0x08000000)
                .args(["--quiet", "--update"])
                .args(install_now.then_some("--end-streams"))
                .arg("--install-dir")
                .arg(&install)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            let mut child = command.spawn().context("starting the update installer")?;
            let setup_pid = child.id();
            let setup_started = butterpollo_win_io::process::creation_time(setup_pid);
            let setup = (setup_started != 0).then_some((setup_pid, setup_started));
            butterpollo_session::state::write_json(
                &h.directory.join("update-result.json"),
                &installing_record(&candidate.version, &install, setup),
            )?;
            drop(update_lock);
            let h = h.clone();
            std::thread::spawn(move || {
                let result = child.wait();
                // If the old host is still here, setup failed before the restart.
                // It has exited, so its folder can go.
                if let Some(download) = &download {
                    remove_download(download);
                }
                let error = setup_failure(&h.directory, result);
                let reason = format!("{error:#}");
                failed(&mut h.updates.lock().unwrap(), error);
                report_failed_install(&h, Some(reason));
            });
            Ok(())
        })();
        match result {
            Ok(()) => state["phase"] = json!("installing"),
            Err(error) => {
                let reason = format!("{error:#}");
                failed(&mut state, error);
                drop(state);
                report_failed_install(h, Some(reason));
            }
        }
    }
}

#[cfg(test)]
mod tests;

/// Hold `update.lock` across spawning setup and recording its identity in
/// `update-result.json` so setup cannot race ahead of the write and service
/// recovery cannot mark the update failed while setup is still starting. Setup's
/// `open_lock` retries `ERROR_SHARING_VIOLATION` briefly until this handle drops.
fn open_update_lock(profile: &Path) -> Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .share_mode(0)
        .open(profile.join("update.lock"))
        .context("Another update is already running")
}

/// The update record a host writes when it starts setup. It names the
/// installation and the spawned setup process (`setup_pid`, `setup_started`),
/// so the service leaves a starting setup alone and ends the update as failed
/// only when setup stopped without taking over (`update_recovery`).
fn installing_record(version: &str, install: &Path, setup: Option<(u32, u64)>) -> Value {
    let mut record = json!({
        "version": version,
        "phase": "installing",
        "started_at": now(),
        "install": install,
    });
    if let Some((pid, started)) = setup {
        record["setup_pid"] = json!(pid);
        record["setup_started"] = json!(started);
    }
    record
}
