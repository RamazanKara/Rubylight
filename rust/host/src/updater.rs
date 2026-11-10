//! Only release metadata from our fixed GitHub repository can select an installer.
//! Installation is limited to the installed service and serialized with launches.
use crate::state::Shared;
use anyhow::{Context, Result, bail};
use butterpollo_core::version::newer;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::io::AsyncWriteExt;

mod schedule;

/// The repository was RamazanKara/Butterpollo until the product became
/// Rubylight. Before the rename only the old name answers; after it, the old
/// name redirects to an API address `client` refuses, so the new one is first.
const REPOSITORIES: [&str; 2] = ["RamazanKara/Rubylight", "RamazanKara/Butterpollo"];
/// Installer names, current first. Releases also carry the old name so that
/// hosts from before the rename find them.
const INSTALLERS: [&str; 2] = ["rubylight-setup", "butterpollo-setup"];
const MAX_INSTALLER: u64 = 512 * 1024 * 1024;
const IDLE_SECONDS: u64 = 60;

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Clone, Debug)]
pub struct Installer {
    pub version: String,
    pub url: String,
    pub digest: String,
    pub size: u64,
}

/// The release list from the first repository name GitHub knows.
pub async fn releases(client: &reqwest::Client) -> Result<reqwest::Response> {
    let mut repositories = REPOSITORIES.iter().peekable();
    while let Some(repository) = repositories.next() {
        let response = client
            .get(format!(
                "https://api.github.com/repos/{repository}/releases?per_page=30"
            ))
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND && repositories.peek().is_some() {
            continue;
        }
        return Ok(response.error_for_status()?);
    }
    unreachable!("REPOSITORIES is not empty")
}

pub fn installer(release: &Value) -> Result<Installer> {
    let tag = release["tag_name"]
        .as_str()
        .context("release has no version")?;
    let version = tag.trim_start_matches('v');
    if version.is_empty()
        || !version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b))
    {
        bail!("invalid release version");
    }
    let assets = release["assets"]
        .as_array()
        .context("release has no assets")?;
    let (name, asset) = INSTALLERS
        .iter()
        .find_map(|prefix| {
            let name = format!("{prefix}-{version}.exe");
            let asset = assets.iter().find(|asset| asset["name"] == name)?;
            Some((name, asset))
        })
        .context("release has no Windows installer")?;
    let url = REPOSITORIES
        .iter()
        .map(|repository| format!("https://github.com/{repository}/releases/download/{tag}/{name}"))
        .find(|url| asset["browser_download_url"] == *url)
        .context("installer is not from the Rubylight release repository")?;
    let digest = asset["digest"]
        .as_str()
        .and_then(|s| s.strip_prefix("sha256:"))
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .context("GitHub has not supplied a SHA-256 digest for this installer")?;
    let size = asset["size"]
        .as_u64()
        .filter(|s| *s > 0 && *s <= MAX_INSTALLER)
        .context("installer size is invalid")?;
    Ok(Installer {
        version: tag.into(),
        url,
        digest: digest.to_ascii_lowercase(),
        size,
    })
}

pub fn client(timeout: Duration) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(15))
        .timeout(timeout)
        .user_agent("Rubylight")
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let url = attempt.url();
            if attempt.previous().len() < 5
                && url.scheme() == "https"
                && matches!(
                    url.host_str(),
                    Some(
                        "github.com"
                            | "release-assets.githubusercontent.com"
                            | "objects.githubusercontent.com"
                    )
                )
            {
                attempt.follow()
            } else {
                attempt.error("unexpected release download redirect")
            }
        }))
        .build()?)
}

/// The host installs its own updates: an installed Windows service.
pub fn install_supported(h: &Shared) -> bool {
    supported(h)
}

fn supported(h: &Shared) -> bool {
    let profile = butterpollo_core::paths::installed_profile();
    butterpollo_windows::process::is_system()
        && std::fs::canonicalize(&h.directory)
            .ok()
            .zip(std::fs::canonicalize(profile).ok())
            .is_some_and(|(a, b)| a == b)
}

pub fn status(h: &Shared) -> Value {
    let mut state = h.updates.lock().unwrap().clone();
    state["install_supported"] = json!(supported(h));
    state["auto_update"] = json!(h.config.read().unwrap().boolean("auto_update", false));
    state["update_check_interval"] = json!(
        h.config
            .read()
            .unwrap()
            .integer("update_check_interval", 86400)
    );
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
        butterpollo_core::state::load_json(&h.directory.join("update-result.json"), json!(null))
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
            interval: config.integer("update_check_interval", 86400),
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
    butterpollo_windows::local_time::utc_to_local(at).unwrap_or(at as i64)
}

/// The state file's entry for the last successful release check.
const LAST_CHECK: &str = "last_update_check";

/// The last successful release check before this run, as Unix seconds; 0
/// when there was none.
fn remembered_check(h: &Shared) -> u64 {
    h.aliases.lock().unwrap()["root"][LAST_CHECK]
        .as_u64()
        .unwrap_or(0)
}

/// Keep the moment of the release check that just succeeded across restarts,
/// for the daily check at a time of day ([`schedule`]).
pub fn remember_check(h: &Shared) {
    let checked_at = h.updates.lock().unwrap()["checked_at"]
        .as_u64()
        .unwrap_or(0);
    let mut aliases = h.aliases.lock().unwrap();
    let mut next = aliases.clone();
    if !next["root"].is_object() {
        next["root"] = json!({});
    }
    next["root"][LAST_CHECK] = json!(checked_at);
    match butterpollo_core::state::write_json(&h.aliases_path, &next) {
        Ok(()) => *aliases = next,
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "the release check time could not be saved");
        }
    }
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
pub fn held_for_others(h: &Shared) -> Option<butterpollo_core::power_action::Busy> {
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
    Ok(())
}

/// Called by maintenance. All admission checks and the final handoff share the
/// Moonlight launch lock, so a connection cannot slip between the check and setup.
pub fn poll(h: &Shared) {
    tidy_downloads(h);
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
    let auto_enabled =
        config.boolean("auto_update", false) && config.integer("update_check_interval", 86400) > 0;
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
            match result {
                Ok(path) => {
                    state["installer_path"] = json!(path);
                    state["phase"] = json!("ready");
                }
                Err(error) if error.is::<DownloadPaused>() => {
                    state["phase"] = json!("waiting");
                    state["idle_since"] = json!(0);
                    state["downloaded_bytes"] = json!(0);
                }
                Err(error) => failed(&mut state, error),
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
            butterpollo_core::state::write_json(
                &h.directory.join("update-result.json"),
                &json!({"version":candidate.version,"phase":"installing","started_at":now()}),
            )?;
            let download = path.parent().map(Path::to_path_buf);
            let mut command = std::process::Command::new(path);
            use std::os::windows::process::CommandExt;
            // The installer must survive the service stopping its host job.
            command
                .creation_flags(0x01000000 | 0x08000000)
                .args(["--quiet", "--update"])
                .args(install_now.then_some("--end-streams"))
                .arg("--install-dir")
                .arg(install)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            let mut child = command.spawn().context("starting the update installer")?;
            let h = h.clone();
            std::thread::spawn(move || {
                let result = child.wait();
                // If the old host is still here, setup failed before the restart.
                // It has exited, so its folder can go.
                if let Some(download) = &download {
                    remove_download(download);
                }
                let error = setup_failure(&h.directory, result);
                failed(&mut h.updates.lock().unwrap(), error);
            });
            Ok(())
        })();
        match result {
            Ok(()) => state["phase"] = json!("installing"),
            Err(error) => failed(&mut state, error),
        }
    }
}

/// Why setup exited without replacing this host: what it recorded when it
/// refused before changing anything, else only how it exited.
fn setup_failure(
    profile: &std::path::Path,
    exit: std::io::Result<std::process::ExitStatus>,
) -> anyhow::Error {
    let record =
        butterpollo_core::state::load_json(&profile.join("update-result.json"), Value::Null)
            .unwrap_or(Value::Null);
    if record["phase"] == "failed"
        && let Some(error) = record["error"].as_str()
    {
        return anyhow::anyhow!("{error}");
    }
    let exit = match exit {
        Ok(status) => status
            .code()
            .map_or_else(|| status.to_string(), |code| format!("exit code {code}")),
        Err(error) => error.to_string(),
    };
    anyhow::anyhow!("Installer exited before replacing this host ({exit}). See the setup log.")
}

fn failed(state: &mut Value, error: anyhow::Error) {
    tracing::warn!(%error, "update failed");
    state["phase"] = json!("failed");
    state["error"] = json!(format!("{error:#}"));
}

fn verify(size: u64, digest: &str, candidate: &Installer) -> Result<()> {
    if size != candidate.size || digest != candidate.digest {
        bail!("Installer verification failed; no files were installed");
    }
    Ok(())
}

async fn download(h: &Shared, id: &str, candidate: &Installer) -> Result<PathBuf> {
    check_recovery(&h.directory)?;
    // No record names a backup still to be restored.
    remove_legacy_downloads(&h.directory.join("updates"));
    let directory = prepare_download(&install_folder()?, id)?;
    let path = directory.join("setup.exe");
    let result = transfer(h, id, candidate, &path).await;
    if result.is_err() {
        remove_download(&directory);
    }
    result
}

/// The folder this host runs from, which setup installed and secured.
fn install_folder() -> Result<PathBuf> {
    Ok(std::env::current_exe()?
        .parent()
        .context("installation folder missing")?
        .to_path_buf())
}
/// Setup's staging folder in the installation folder (setup's
/// `install::staging_root`). The installer is downloaded there, never to
/// ProgramData: Defender's cloud model scores an unsigned setup that runs
/// elevated from ProgramData as malware.
fn staging_folder(install: &Path) -> PathBuf {
    install.join(".staging")
}
const DOWNLOAD_PREFIX: &str = "download-";
/// A new, empty folder for download `id` in `install`'s staging folder,
/// limited to SYSTEM and Administrators before anything lands in it. A link
/// at or in the staging folder is refused, never followed. Earlier
/// downloads go first; one a cancelled transfer still holds open stays and
/// cleans itself up.
fn prepare_download(install: &Path, id: &str) -> Result<PathBuf> {
    let staging = staging_folder(install);
    match std::fs::symlink_metadata(&staging) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            butterpollo_windows::process::create_folder_administrators_only(&staging)?
        }
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    butterpollo_windows::process::restrict_to_administrators(&staging)
        .with_context(|| format!("securing {}", staging.display()))?;
    remove_downloads(&staging);
    // A UUID folder per download avoids partial downloads and overlapping
    // cancellation races; a new one, so nothing made before is inside it.
    let directory = staging.join(format!("{DOWNLOAD_PREFIX}{id}"));
    butterpollo_windows::process::create_folder_administrators_only(&directory)?;
    Ok(directory)
}
/// Remove the download folders in `staging`. Returns how many are left: one a setup still runs from, or a cancelled transfer
/// still writes to.
fn remove_downloads(staging: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(staging) else {
        return 0;
    };
    let mut left = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name
            .strip_prefix(DOWNLOAD_PREFIX)
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
        {
            continue;
        }
        if let Err(error) = remove_tree(&entry.path()) {
            tracing::debug!(%error, folder = %entry.path().display(), "download folder not removed yet");
            left += 1;
        }
    }
    left
}
/// One download folder, and its staging folder once nothing else is in it.
fn remove_download(directory: &Path) {
    if let Err(error) = remove_tree(directory)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%error, folder = %directory.display(), "the update download could not be removed");
    }
    if let Some(staging) = directory.parent() {
        let _ = std::fs::remove_dir(staging);
    }
}
/// `path` and everything in it; a link is removed itself, never followed.
fn remove_tree(path: &Path) -> std::io::Result<()> {
    use std::os::windows::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_attributes() & 0x400 != 0 {
        std::fs::remove_dir(path).or_else(|_| std::fs::remove_file(path))
    } else if metadata.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}
/// Earlier hosts downloaded into the profile's `updates` folder, where
/// earlier setups also kept their transactions. Its downloads go (a link
/// is removed itself), its transactions but the newest two, and the folder
/// once empty. Only after `check_recovery`: no record may name a backup
/// still to be restored.
fn remove_legacy_downloads(updates: &Path) {
    let Ok(entries) = std::fs::read_dir(updates) else {
        return;
    };
    let mut transactions = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if uuid::Uuid::parse_str(&name).is_ok() {
            if let Err(error) = remove_tree(&entry.path()) {
                tracing::warn!(%error, folder = %entry.path().display(), "an earlier update download could not be removed");
            }
        } else if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            transactions.push(name);
        }
    }
    for name in stale_transactions(&transactions) {
        let _ = remove_tree(&updates.join(name));
    }
    let _ = std::fs::remove_dir(updates);
}
/// When the next `tidy_downloads` runs, in seconds since the epoch.
static NEXT_TIDY: AtomicU64 = AtomicU64::new(0);
/// A setup that updated this host ran from its download folder until after
/// it started this host, so only this host can remove that folder once
/// setup has exited: tried every minute until none is left.
fn tidy_downloads(h: &Shared) {
    let now = now();
    if NEXT_TIDY.load(Ordering::Relaxed) > now {
        return;
    }
    NEXT_TIDY.store(now + 60, Ordering::Relaxed);
    let phase = h.updates.lock().unwrap()["phase"].clone();
    if matches!(phase.as_str(), Some("downloading" | "ready" | "installing")) || !supported(h) {
        return;
    }
    let Ok(install) = install_folder() else {
        return;
    };
    let staging = staging_folder(&install);
    if remove_downloads(&staging) == 0 {
        // Once nothing else is in it (setup keeps a failed update's backup
        // there).
        let _ = std::fs::remove_dir(&staging);
        NEXT_TIDY.store(u64::MAX, Ordering::Relaxed);
    }
}

fn check_recovery(profile: &std::path::Path) -> Result<Value> {
    let record =
        butterpollo_core::state::load_json(&profile.join("update-result.json"), Value::Null)
            .context("The last update's record is unreadable. Run the Rubylight installer to repair this installation")?;
    if record["phase"] == "recovery_failed"
        || (record["phase"] == "installing" && record["backup"].is_string())
    {
        bail!(
            "An earlier update could not be rolled back. Run the Rubylight installer to repair this installation; keep the backup the update record names until it has finished."
        );
    }
    Ok(record)
}

/// Setup's update folders (transaction-<process id>-<nanoseconds>, kept
/// with their backup when an update fails) older than the newest two.
/// Other names are left alone.
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
    dated.into_iter().skip(2).map(|(_, name)| name).collect()
}

async fn transfer(
    h: &Shared,
    id: &str,
    candidate: &Installer,
    path: &std::path::Path,
) -> Result<PathBuf> {
    // A download for "Install now" carries on beside the stream, but not beside
    // another device's when a device asked.
    let paused = || {
        // Its own statement: the updates lock is free again before
        // `held_for_others` takes it.
        let now = h.updates.lock().unwrap()["now"] == true;
        (busy(h) && !now) || held_for_others(h).is_some()
    };
    if paused() {
        return Err(DownloadPaused.into());
    }
    let mut response = client(Duration::from_secs(600))?
        .get(&candidate.url)
        .send()
        .await?
        .error_for_status()?;
    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .await?;
    let mut size = 0;
    let mut hash = Sha256::new();
    while let Some(chunk) = response.chunk().await? {
        if paused() {
            return Err(DownloadPaused.into());
        }
        if h.updates.lock().unwrap()["download_id"] != id {
            bail!("Download cancelled");
        }
        size += chunk.len() as u64;
        if size > candidate.size {
            bail!("Installer exceeds its advertised size");
        }
        file.write_all(&chunk).await?;
        hash.update(&chunk);
        h.updates.lock().unwrap()["downloaded_bytes"] = json!(size);
    }
    file.sync_all().await?;
    verify(size, &hex::encode(hash.finalize()), candidate)?;
    if h.updates.lock().unwrap()["download_id"] != id {
        bail!("Download cancelled");
    }
    Ok(path.to_path_buf())
}

#[derive(Debug)]
struct DownloadPaused;
impl std::fmt::Display for DownloadPaused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("download paused while the host is busy")
    }
}
impl std::error::Error for DownloadPaused {}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        host: Shared,
        directory: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let directory = std::env::temp_dir()
                .join(format!("butterpollo-update-test-{}", uuid::Uuid::new_v4()));
            let host = crate::state::Host::load(directory.clone(), directory.join("assets"), None)
                .unwrap();
            Self { host, directory }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }
    fn launch(role: butterpollo_core::session::Role) -> crate::state::Launch {
        crate::state::Launch {
            id: "fixture".into(),
            client: butterpollo_core::state::Client {
                name: "fixture".into(),
                cert: String::new(),
                uuid: "fixture".into(),
                perm: u32::MAX,
                enabled: true,
                extra: Default::default(),
            },
            peer: "127.0.0.1".parse().unwrap(),
            app_id: 1,
            background: None,
            key: [0; 16],
            key_id: 1,
            ping: "fixture".into(),
            connect_data: 1,
            role,
            created: std::time::Instant::now(),
            rtsp_encrypted: true,
            rtsp_counter: Default::default(),
            rtsp_received: Default::default(),
            preparation: Default::default(),
            vrr_requested: false,
            host_audio: false,
            requested_rate: 0,
            options: Default::default(),
            audio_preparation: Default::default(),
            preparing: Default::default(),
            transport: Default::default(),
            roaming_offered: Default::default(),
            claimed_until: None,
            nucleus: None,
            held: None,
            warnings: Default::default(),
        }
    }
    #[test]
    fn connections_reset_the_idle_wait_including_during_downloads() {
        use butterpollo_core::{rtsp::Negotiated, session::Role};
        let f = Fixture::new();
        let h = &f.host;
        for role in [Role::Stream, Role::RemoteMonitor, Role::InputOnly] {
            for phase in ["waiting", "downloading", "ready"] {
                *h.updates.lock().unwrap() = json!({"phase":phase,"idle_since":now()-120});
                h.sessions.lock().unwrap().queue(launch(role)).unwrap();
                poll(h);
                assert_eq!(h.updates.lock().unwrap()["phase"], phase);
                assert_eq!(h.updates.lock().unwrap()["idle_since"], 0);
                h.sessions
                    .lock()
                    .unwrap()
                    .start(launch(role), Negotiated::default())
                    .unwrap();
                poll(h);
                assert_eq!(h.updates.lock().unwrap()["idle_since"], 0);
                h.sessions.lock().unwrap().active.clear();
                poll(h);
                assert!(h.updates.lock().unwrap()["idle_since"].as_u64().unwrap() > 0);
                assert_eq!(h.updates.lock().unwrap()["phase"], phase);
            }
        }
    }
    #[test]
    fn a_devices_install_now_waits_for_another_devices_stream_but_not_its_own() {
        let f = Fixture::new();
        let h = &f.host;
        // "fixture" streams; the phone asked, so its install waits, even once
        // the download is done and a stream starts later.
        h.sessions
            .lock()
            .unwrap()
            .queue(launch(butterpollo_core::session::Role::Stream))
            .unwrap();
        for phase in ["waiting", "ready"] {
            *h.updates.lock().unwrap() =
                json!({"phase":phase,"idle_since":0,"now":true,"now_by":"phone"});
            poll(h);
            assert_eq!(h.updates.lock().unwrap()["phase"], phase);
            assert_eq!(
                status(h)["waiting_for"],
                "fixture is starting a stream.",
                "{phase}"
            );
        }
        // The asking device's own stream ends: on to the installer, which
        // this test host, no installed service, refuses.
        *h.updates.lock().unwrap() =
            json!({"phase":"ready","idle_since":0,"now":true,"now_by":"fixture"});
        assert_eq!(status(h)["waiting_for"], Value::Null);
        poll(h);
        assert_eq!(h.updates.lock().unwrap()["phase"], "failed");
        // So does the console's, whoever streams.
        *h.updates.lock().unwrap() = json!({"phase":"ready","idle_since":0,"now":true});
        poll(h);
        assert_eq!(h.updates.lock().unwrap()["phase"], "failed");
    }
    #[test]
    fn install_now_again_says_whether_it_changed_anything() {
        let mut state = json!({"phase":"waiting","now":false});
        assert!(install_now_again(&mut state, Some("phone")));
        assert_eq!(
            (&state["now"], &state["now_by"]),
            (&json!(true), &json!("phone"))
        );
        // The same device again: nothing new, so no second update.started.
        assert!(!install_now_again(&mut state, Some("phone")));
        // The console's ends any stream.
        assert!(install_now_again(&mut state, None));
        assert_eq!(state["now_by"], Value::Null);
        assert!(!install_now_again(&mut state, Some("phone")));
        assert_eq!(state["now_by"], Value::Null);
        let mut installing = json!({"phase":"installing","now":false});
        assert!(!install_now_again(&mut installing, Some("phone")));
        assert_eq!(installing["now"], false);
    }
    #[test]
    fn install_now_does_not_wait_for_an_idle_host() {
        let f = Fixture::new();
        let h = &f.host;
        h.sessions
            .lock()
            .unwrap()
            .queue(launch(butterpollo_core::session::Role::Stream))
            .unwrap();
        *h.updates.lock().unwrap() = json!({"phase":"ready","idle_since":0,"now":true});
        poll(h);
        // Past the idle gate, straight to the installer; this test host is
        // no installed service, so that step fails.
        let state = h.updates.lock().unwrap();
        assert_eq!(state["phase"], "failed");
        assert!(
            state["error"]
                .as_str()
                .unwrap()
                .contains("installed Windows service"),
            "{state}"
        );
        drop(state);
        // Without it the busy host keeps the update waiting.
        *h.updates.lock().unwrap() = json!({"phase":"ready","idle_since":0});
        poll(h);
        assert_eq!(h.updates.lock().unwrap()["phase"], "ready");
        assert_eq!(h.updates.lock().unwrap()["idle_since"], 0);
    }
    #[test]
    fn the_installer_is_hashed_with_no_lock_held_and_checked_again_after() {
        let f = Fixture::new();
        let h = &f.host;
        let ready = json!({"phase":"ready","idle_since":0,"now":true,
                           "download_id":"a","installer_path":"C:\\setup.exe"});
        let hashes = std::cell::Cell::new(0);
        // Unchanged meanwhile: on to the installer, which this test host,
        // no installed service, refuses.
        *h.updates.lock().unwrap() = ready.clone();
        poll_with(h, None, &|path| {
            hashes.set(hashes.get() + 1);
            assert_eq!(path, std::path::Path::new("C:\\setup.exe"));
            assert!(h.launch_transition.try_lock().is_ok());
            assert!(h.updates.try_lock().is_ok());
            Ok((1, "00".into()))
        });
        assert_eq!(hashes.get(), 1);
        let state = h.updates.lock().unwrap().clone();
        assert_eq!(state["phase"], "failed");
        assert!(
            state["error"]
                .as_str()
                .unwrap()
                .contains("installed Windows service")
        );
        // Cancelled and downloaded again while it was hashed: nothing starts,
        // and the next poll hashes the new download.
        *h.updates.lock().unwrap() = ready.clone();
        poll_with(h, None, &|_| {
            hashes.set(hashes.get() + 1);
            h.updates.lock().unwrap()["download_id"] = json!("b");
            Ok((1, "00".into()))
        });
        assert_eq!(hashes.get(), 2);
        let state = h.updates.lock().unwrap().clone();
        assert_eq!(state["phase"], "ready");
        assert_eq!(state["download_id"], "b");
        // A busy host by then keeps the update waiting.
        *h.updates.lock().unwrap() = json!({"phase":"ready","idle_since":now()-3600,
                                            "download_id":"a","installer_path":"C:\\setup.exe"});
        poll_with(h, None, &|_| {
            h.sessions
                .lock()
                .unwrap()
                .queue(launch(butterpollo_core::session::Role::Stream))
                .unwrap();
            Ok((1, "00".into()))
        });
        let state = h.updates.lock().unwrap().clone();
        assert_eq!(state["phase"], "ready");
        assert_eq!(state["idle_since"], 0);
    }
    #[test]
    fn cancellation_invalidates_a_download_and_stops_automatic_requeue() {
        let f = Fixture::new();
        let h = &f.host;
        *h.updates.lock().unwrap() =
            json!({"phase":"downloading","download_id":"old","latest_version":"2.1.0"});
        cancel(h).unwrap();
        let state = h.updates.lock().unwrap();
        assert_eq!(state["phase"], "idle");
        assert!(state["download_id"].is_null());
        assert_eq!(state["attempted_version"], "2.1.0");
        drop(state);
        h.updates.lock().unwrap()["phase"] = json!("installing");
        assert!(cancel(h).is_err());
    }
    #[test]
    fn disabling_automatic_updates_cancels_an_automatic_queue_only() {
        let f = Fixture::new();
        let h = &f.host;
        for automatic in [true, false] {
            *h.updates.lock().unwrap() =
                json!({"phase":"waiting","automatic":automatic,"idle_since":0});
            poll(h);
            assert_eq!(
                h.updates.lock().unwrap()["phase"],
                if automatic { "idle" } else { "waiting" }
            );
        }
        assert!(!status(h)["install_supported"].as_bool().unwrap());
        assert_eq!(status(h)["update_check_interval"], 86400);
        h.config
            .write()
            .unwrap()
            .values
            .insert("update_check_interval".into(), "0".into());
        assert_eq!(status(h)["update_check_interval"], 0);
        assert!(queue(h, false, false, None).is_err());
    }
    fn set_interval(h: &Shared, seconds: &str) {
        h.config
            .write()
            .unwrap()
            .values
            .insert("update_check_interval".into(), seconds.into());
    }
    #[test]
    fn the_schedule_follows_the_wall_clock_and_the_current_interval() {
        let day = 86400;
        let t = 1_000_000_000;
        // Off, or never checked.
        assert_eq!(next_check_at(0, t, 0, t), None);
        assert_eq!(next_check_at(-1, t, 0, t), None);
        assert_eq!(next_check_at(day, 0, 0, t), Some(0));
        // Not due until a whole interval after the last check; the console's
        // floor of an hour applies to a smaller value.
        assert_eq!(
            next_check_at(day, t - 100, 0, t),
            Some(t - 100 + day as u64)
        );
        assert_eq!(next_check_at(60, t - 100, 0, t), Some(t - 100 + 3600));
        // A PC that slept for days is overdue the moment it wakes.
        assert!(next_check_at(day, t - 3 * 86400, 0, t).unwrap() <= t);
        // A clock set back must not postpone the check by the difference.
        assert_eq!(next_check_at(day, t + 5000, 0, t), Some(0));
        // A step of a second or two (time sync) is not a clock set back.
        assert_eq!(next_check_at(day, t + 1, 0, t), Some(t + 1 + day as u64));
        assert_eq!(
            next_check_at(day, t + 300, 0, t),
            Some(t + 300 + day as u64)
        );
        assert_eq!(next_check_at(day, t + 301, 0, t), Some(0));
        // A failed check (network not back after sleep) is retried within
        // minutes, not after a whole interval.
        let retry = next_check_at(day, t - 10, 1, t).unwrap();
        assert!(retry > t && retry <= t + 300, "{retry}");
    }
    #[test]
    fn repeated_failures_back_off_up_to_the_interval() {
        let t = 1_000_000_000;
        let day = 86400;
        let wait = |failures, interval| next_check_at(interval, t, failures, t + 1).unwrap() - t;
        assert_eq!(wait(1, day), 120);
        assert_eq!(wait(2, day), 240);
        assert_eq!(wait(3, day), 480);
        // Capped by the interval, and by the hour floor for a smaller one.
        assert_eq!(wait(20, day), day as u64);
        assert_eq!(wait(20, 7200), 7200);
        assert_eq!(wait(20, 60), 3600);
        // A huge count must not overflow the shift.
        assert_eq!(wait(u64::MAX, day), day as u64);
        // A success resets the count.
        assert_eq!(wait(0, day), day as u64);
    }
    #[test]
    fn a_changed_interval_applies_without_a_restart_and_a_failed_check_is_retried() {
        let f = Fixture::new();
        let h = &f.host;
        // Checked two hours ago with the default daily interval: not due.
        let t = now();
        *h.updates.lock().unwrap() =
            json!({"checking":false,"check_failed":false,"checked_at":t - 7200});
        assert!(!check_due(h));
        assert_eq!(status(h)["next_check_at"], t - 7200 + 86400);
        // The user picks the minimum: due now, same running host.
        set_interval(h, "3600");
        assert!(check_due(h));
        // Turned off.
        set_interval(h, "0");
        assert!(!check_due(h));
        assert!(status(h)["next_check_at"].is_null());
        // A check in flight is not started again.
        set_interval(h, "3600");
        h.updates.lock().unwrap()["checking"] = json!(true);
        assert!(!check_due(h));
        // A check that failed a moment ago waits briefly; one that failed
        // minutes ago is retried even with a daily interval.
        set_interval(h, "86400");
        *h.updates.lock().unwrap() =
            json!({"checking":false,"check_failed":true,"check_failures":1,"checked_at":t - 10});
        assert!(!check_due(h));
        h.updates.lock().unwrap()["checked_at"] = json!(t - 600);
        assert!(check_due(h));
    }
    #[test]
    fn a_stream_defers_the_check() {
        let f = Fixture::new();
        let h = &f.host;
        *h.updates.lock().unwrap() = json!({"checking":false,"checked_at":0});
        assert!(check_due(h));
        h.sessions
            .lock()
            .unwrap()
            .queue(launch(butterpollo_core::session::Role::Stream))
            .unwrap();
        assert!(!check_due(h));
        // Due but held back: the console says so.
        assert_eq!(status(h)["deferred"], true);
    }
    #[test]
    fn a_lingering_game_or_monitor_does_not_defer_the_check() {
        let f = Fixture::new();
        let h = &f.host;
        *h.updates.lock().unwrap() = json!({"checking":false,"checked_at":0});
        // (A held virtual display is `busy` the same way, but a `Retained`
        // cannot be built without a display driver.)
        *h.current_app.lock().unwrap() = Some(
            crate::process::RunningApp::with_environment(
                &butterpollo_core::state::App::desktop(),
                Default::default(),
            )
            .unwrap(),
        );
        assert!(busy(h), "the install path still waits for an idle host");
        assert!(check_due(h));
        assert_eq!(status(h)["deferred"], false);
    }
    fn set_time(h: &Shared, time: &str) {
        h.config
            .write()
            .unwrap()
            .values
            .insert("update_check_time".into(), time.into());
    }
    /// The time this PC's clock showed `hours` ago, as the setting takes it.
    fn hours_ago(hours: i64) -> String {
        let local = local_clock(now()) - hours * 3600;
        format!(
            "{:02}:{:02}",
            local.rem_euclid(86400) / 3600,
            local.rem_euclid(3600) / 60
        )
    }
    #[test]
    fn a_time_of_day_decides_the_check_in_place_of_the_interval() {
        let f = Fixture::new();
        let h = &f.host;
        let t = now();
        let checked = |seconds_ago: u64| {
            *h.updates.lock().unwrap() =
                json!({"checking":false,"check_failed":false,"checked_at":t - seconds_ago});
        };
        assert!(status(h)["update_check_time"].is_null());
        // The time was four hours ago and the last check eight hours ago: due,
        // although the daily interval has not passed.
        let time = hours_ago(4);
        set_time(h, &time);
        checked(8 * 3600);
        assert!(check_due(h));
        assert_eq!(status(h)["update_check_time"], time);
        assert!(status(h)["next_check_at"].as_u64().unwrap() <= t);
        // Checked since: not again today, however short the interval.
        set_interval(h, "3600");
        checked(2 * 3600);
        assert!(!check_due(h));
        let next = status(h)["next_check_at"].as_u64().unwrap();
        assert!(next > t + 18 * 3600 && next < t + 22 * 3600, "{next}");
        // Without the time the interval decides again, same running host.
        set_time(h, "");
        assert!(check_due(h));
        assert!(status(h)["update_check_time"].is_null());
        // So does it with a time the host cannot read.
        for unreadable in ["3am", "25:00", "0300"] {
            set_time(h, unreadable);
            assert!(check_due(h), "{unreadable}");
            assert!(status(h)["update_check_time"].is_null(), "{unreadable}");
            assert_eq!(status(h)["next_check_at"], t - 2 * 3600 + 3600);
        }
        // An interval of 0 turns automatic checks off with a time as well.
        set_time(h, &time);
        set_interval(h, "0");
        checked(8 * 3600);
        assert!(!check_due(h));
        assert!(status(h)["next_check_at"].is_null());
        // A stream defers the daily check like any other.
        set_interval(h, "86400");
        assert!(check_due(h));
        h.sessions
            .lock()
            .unwrap()
            .queue(launch(butterpollo_core::session::Role::Stream))
            .unwrap();
        assert!(!check_due(h));
        assert_eq!(status(h)["deferred"], true);
    }
    #[test]
    fn the_daily_check_counts_the_last_check_before_a_restart() {
        let f = Fixture::new();
        let h = &f.host;
        let t = now();
        let time = hours_ago(4);
        // Never checked: at once.
        set_time(h, &time);
        assert!(check_due(h));
        // A check two hours ago succeeded; then the host restarts.
        h.updates.lock().unwrap()["checked_at"] = json!(t - 2 * 3600);
        remember_check(h);
        let saved = butterpollo_core::state::load_json(&h.aliases_path, json!(null)).unwrap();
        assert_eq!(saved["root"][LAST_CHECK], t - 2 * 3600);
        let again = crate::state::Host::load(f.directory.clone(), f.directory.join("assets"), None)
            .unwrap();
        assert_eq!(again.updates.lock().unwrap()["checked_at"], 0);
        // Today's check is done: the restarted host waits for tomorrow's time.
        set_time(&again, &time);
        assert!(!check_due(&again));
        assert!(status(&again)["next_check_at"].as_u64().unwrap() > t);
        // With an interval it checks at every start, as before.
        set_time(&again, "");
        assert!(check_due(&again));
        // A restart after the time passed catches the check up.
        again.aliases.lock().unwrap()["root"][LAST_CHECK] = json!(t - 8 * 3600);
        set_time(&again, &time);
        assert!(check_due(&again));
        // A check that failed in this run is retried by its own time, not
        // held back by the one remembered.
        again.aliases.lock().unwrap()["root"][LAST_CHECK] = json!(t - 2 * 3600);
        *again.updates.lock().unwrap() =
            json!({"checking":false,"check_failed":true,"check_failures":1,"checked_at":t - 600});
        assert!(check_due(&again));
    }
    #[tokio::test]
    #[ignore = "downloads the official public installer; never executes it"]
    async fn official_release_download_verifies_with_github_digest() -> Result<()> {
        let f = Fixture::new();
        let releases: Vec<Value> = super::releases(&client(Duration::from_secs(35))?)
            .await?
            .json()
            .await?;
        let candidate = releases
            .iter()
            .find_map(|r| installer(r).ok())
            .context("no release installer")?;
        f.host.updates.lock().unwrap()["download_id"] = json!("download-test");
        let path = f.directory.join("setup.exe");
        transfer(&f.host, "download-test", &candidate, &path).await?;
        assert_eq!(std::fs::metadata(&path)?.len(), candidate.size);
        eprintln!(
            "Verified {}: {} bytes, SHA256 {}",
            candidate.version, candidate.size, candidate.digest
        );
        Ok(())
    }
    fn release() -> Value {
        json!({"tag_name":"2.0.0-rc.7","assets":[{"name":"butterpollo-setup-2.0.0-rc.7.exe","browser_download_url":"https://github.com/RamazanKara/Butterpollo/releases/download/2.0.0-rc.7/butterpollo-setup-2.0.0-rc.7.exe","size":32,"digest":format!("sha256:{}", "ab".repeat(32))}]})
    }
    #[test]
    fn an_unresolved_rollback_cannot_be_overwritten_by_another_update() -> Result<()> {
        let f = Fixture::new();
        let path = f.directory.join("update-result.json");
        for phase in ["installing", "recovery_failed"] {
            let record = json!({"version":"2.0.0-rc.22","phase":phase,
                "backup":f.directory.join("updates/transaction-1-2/previous")});
            butterpollo_core::state::write_json(&path, &record)?;
            assert!(check_recovery(&f.directory).is_err());
            assert_eq!(
                butterpollo_core::state::load_json(&path, Value::Null)?,
                record
            );
        }
        for phase in ["installed", "rolled_back", "failed", "superseded"] {
            butterpollo_core::state::write_json(&path, &json!({"phase":phase}))?;
            assert!(check_recovery(&f.directory).is_ok());
        }
        std::fs::write(&path, b"{")?;
        let error = check_recovery(&f.directory).unwrap_err().to_string();
        assert!(error.contains("Run the Rubylight installer"), "{error}");
        Ok(())
    }
    #[test]
    fn a_setup_that_stops_early_reports_its_recorded_reason() -> Result<()> {
        use std::os::windows::process::ExitStatusExt;
        let f = Fixture::new();
        let path = f.directory.join("update-result.json");
        let exited = || Ok(std::process::ExitStatus::from_raw(1));
        // What this host writes before it starts setup.
        butterpollo_core::state::write_json(
            &path,
            &json!({"version":"v2.2.22","phase":"installing","started_at":1}),
        )?;
        let error = setup_failure(&f.directory, exited()).to_string();
        assert_eq!(
            error,
            "Installer exited before replacing this host (exit code 1). See the setup log."
        );
        let reason = "Rubylight cannot be updated in D:\\Games\\Rubylight: D:\\Games must be owned by Administrators, SYSTEM or TrustedInstaller.";
        butterpollo_core::state::write_json(
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
            .queue(launch(butterpollo_core::session::Role::Stream))
            .unwrap();
        let candidate = installer(&release()).unwrap();
        let error = transfer(&f.host, "test", &candidate, &f.directory.join("setup.exe"))
            .await
            .unwrap_err();
        assert!(error.is::<DownloadPaused>());
        assert!(!f.directory.join("setup.exe").exists());
        // A device's Install now pauses beside another device's stream too.
        *f.host.updates.lock().unwrap() =
            json!({"phase":"downloading","now":true,"now_by":"phone"});
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
        butterpollo_windows::process::elevated().unwrap_or(false)
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
        assert!(!directory.starts_with(butterpollo_core::paths::installed_profile()));
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
}
