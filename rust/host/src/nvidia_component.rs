//! The NVIDIA component on this PC ([`butterpollo_core::nvidia_component`]):
//! the console's download, the check of every file, the install into a folder
//! only administrators may change, the removal, and the start-up check.
//!
//! The download is the user's choice on the console (`POST
//! /api/nvidia-component/install`); nothing is fetched before it. It comes
//! from this host version's own GitHub release, one archive checked against
//! GitHub's SHA-256 digest as the updater checks its installers, and NVIDIA's
//! files in it against the digests compiled into the host. Staged files
//! become the component only after every check, and the record naming them
//! is written last. Before each load the host checks them all again
//! ([`butterpollo_windows::nvidia_enhance::load`]).
//!
//! A host update finds the earlier version's adapter: it keeps using it (the
//! ABI check refuses one it cannot speak) and fetches its own in the
//! background, keeping the earlier one if that fails. Removal forgets the
//! component at once and deletes its files; a file still loaded in this
//! process goes at the next start, before anything is loaded.

use crate::state::Shared;
use anyhow::{Context, Result, anyhow, bail, ensure};
use butterpollo_core::nvidia_component::{self as pins, ADAPTER, NGX_DATA, Pinned, RECORD, Record};
use butterpollo_windows::nvidia_enhance::{self as loader, Component};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, OnceLock, PoisonError, atomic::Ordering},
    time::Duration,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");
/// No archive of the component is larger.
const MAX_ASSET: u64 = 64 * 1024 * 1024;

/// The component's download or removal in progress, and the last failure.
#[derive(Default)]
pub struct NvidiaComponent {
    job: Mutex<Job>,
    /// A test's GitHub (API and downloads) in place of github.com.
    #[cfg(test)]
    pub origin: Mutex<Option<String>>,
    /// A test's NVIDIA files in place of [`pins::RUNTIME`].
    #[cfg(test)]
    pub runtime: Mutex<Option<Vec<Pinned>>>,
}

#[derive(Clone, Debug, Default)]
struct Job {
    /// `download` or `remove` while one runs.
    phase: Option<&'static str>,
    done: u64,
    total: u64,
    error: Option<String>,
}

impl NvidiaComponent {
    fn lock(&self) -> MutexGuard<'_, Job> {
        self.job.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The component's folder on this PC.
pub fn root(h: &Shared) -> PathBuf {
    pins::root(&h.directory)
}

/// Whether this host runs elevated (the installed service, or a portable
/// host run as administrator): then the folder it loads from must be one
/// nobody but administrators may change, or a process without those rights
/// could swap the adapter between its check and its load. Tests, which run
/// elevated on CI, keep to their own temporary folders.
fn elevated() -> bool {
    if cfg!(test) {
        return false;
    }
    butterpollo_windows::process::elevated().unwrap_or(true)
}

/// Refuse a folder that is not this host's own: a link on the way to it, or
/// for an elevated host ([`elevated`]), a folder anyone but administrators
/// may change, or one above it they don't own.
fn trusted(folder: &Path, elevated: bool) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    for path in folder
        .ancestors()
        .filter(|path| !path.as_os_str().is_empty())
    {
        let meta = std::fs::symlink_metadata(path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        ensure!(
            meta.file_attributes() & 0x400 == 0,
            "{} is a link to elsewhere, and the NVIDIA components are kept behind none",
            path.display()
        );
        if elevated {
            crate::neural::folder::administrators_hold(path, path == folder).map_err(|problem| {
                anyhow!(
                    "the NVIDIA components are kept only where nobody but administrators may change them: {problem}"
                )
            })?;
        }
    }
    Ok(())
}

/// The component folder, readable by users and writable only by
/// administrators.
fn ensure_root(root: &Path) -> Result<()> {
    if root.is_dir() {
        return Ok(());
    }
    if elevated() {
        match butterpollo_windows::process::create_folder_administrators_readable(root) {
            Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => {
                return Err(e).with_context(|| format!("cannot create {}", root.display()));
            }
            _ => {}
        }
    } else {
        std::fs::create_dir_all(root)
            .with_context(|| format!("cannot create {}", root.display()))?;
    }
    Ok(())
}

/// The installed component of the host whose profile is `profile`, after
/// checking its folder; the `--nvidia-probe` process finds it this way too.
pub fn component_for(profile: &Path) -> Result<Component> {
    let root = pins::root(profile);
    let record = Record::read(&root)?.context("the NVIDIA components are not installed")?;
    let elevated = elevated();
    trusted(&root, elevated)?;
    let folder = record.files(&root);
    trusted(&folder, elevated)?;
    Ok(Component {
        folder,
        adapter_sha256: record.adapter_sha256,
        data: root.join(NGX_DATA),
    })
}

/// Whether this PC has an NVIDIA GPU, asked once.
fn nvidia_gpu() -> bool {
    static FOUND: OnceLock<bool> = OnceLock::new();
    *FOUND.get_or_init(loader::has_nvidia_gpu)
}

/// The console's view of the component: the job's state (`missing`,
/// `download`, `check`, `remove`, `ready`, or `failed` with the reason) and
/// progress; apart from it, whether a component is installed and in use, and
/// whether that one is an earlier version's; and whether this PC has an
/// NVIDIA GPU at all.
pub fn summary(h: &Shared) -> Value {
    let job = h.nvidia_component.lock().clone();
    let installed = loader::installed();
    let state = match (&job.phase, &job.error) {
        (Some(phase), _) => phase,
        (None, Some(_)) => "failed",
        (None, None) if installed => "ready",
        (None, None) => "missing",
    };
    let outdated = installed
        && Record::read(&root(h))
            .ok()
            .flatten()
            .is_some_and(|record| record.version != VERSION);
    json!({
        "state": state,
        "done": job.done,
        "total": job.total,
        "error": job.error,
        "installed": installed,
        "outdated": outdated,
        "nvidia_gpu": nvidia_gpu(),
    })
}

/// Starts `phase`, unless another job runs.
fn begin(h: &Shared, phase: &'static str) -> Result<(), String> {
    let mut job = h.nvidia_component.lock();
    if job.phase.is_some() {
        return Err(
            "The NVIDIA components are already being changed; wait until that ends.".into(),
        );
    }
    *job = Job {
        phase: Some(phase),
        ..Job::default()
    };
    Ok(())
}

/// The running job moves on to `phase`.
fn advance(h: &Shared, phase: &'static str) {
    h.nvidia_component.lock().phase = Some(phase);
}

fn end(h: &Shared, error: Option<String>) {
    *h.nvidia_component.lock() = Job {
        error,
        ..Job::default()
    };
}

/// The user's download: refused during a stream, whose bandwidth it would
/// take, and on a PC without an NVIDIA GPU.
pub fn install(h: &Shared) -> Result<(), String> {
    if !nvidia_gpu() {
        return Err("This PC has no NVIDIA graphics card.".into());
    }
    if crate::updater::streaming(h) {
        return Err("End the stream first: the download would take bandwidth from it.".into());
    }
    begin(h, "download")?;
    let h = h.clone();
    tokio::spawn(async move {
        let result = fetch(&h).await;
        let outcome = finish(&h, result).await;
        end(&h, outcome.err());
    });
    Ok(())
}

/// Forgets the component and deletes its files; refused during a stream,
/// which may have it loaded.
pub fn remove(h: &Shared) -> Result<(), String> {
    if crate::updater::streaming(h) {
        return Err("End the stream first: it may be using the NVIDIA components.".into());
    }
    begin(h, "remove")?;
    loader::set_component(None);
    h.nvidia_enhancements.store(0, Ordering::Release);
    h.nvidia_enhancement_errors.lock().unwrap().clear();
    let root = root(h);
    let result = forget(&root).map_err(|error| format!("{error:#}"));
    match &result {
        Ok(()) => tracing::info!("NVIDIA components removed"),
        Err(error) => tracing::error!(%error, "the NVIDIA components were not removed"),
    }
    end(h, result.clone().err());
    result
}

/// Deletes the record and every file of the component, when its folder is
/// this host's own.
fn forget(root: &Path) -> Result<()> {
    if !exists(root)? {
        return Ok(());
    }
    trusted(root, elevated())?;
    match std::fs::remove_file(root.join(RECORD)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("cannot delete {}", root.join(RECORD).display()));
        }
    }
    clean(root, None)
}

/// Whether `path` exists, without following a link at it.
fn exists(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("cannot read {}", path.display())),
    }
}

/// At start: what a crash or a removal left goes, an installed component is
/// checked and asked what the GPU and driver offer, and an earlier version's
/// adapter is kept while this version's is fetched ([`update`]).
pub fn start(h: &Shared) {
    let h = h.clone();
    tokio::spawn(async move {
        // Held from the first step: a download the user starts meanwhile
        // waits, and nothing here clears up under it.
        if begin(&h, "check").is_err() {
            return;
        }
        let root = root(&h);
        let found = tokio::task::spawn_blocking(move || -> Result<Option<Record>> {
            if !exists(&root)? {
                return Ok(None);
            }
            trusted(&root, elevated())?;
            let record = Record::read(&root)?;
            clean(&root, record.as_ref().map(|record| record.folder.as_str()))?;
            Ok(record)
        })
        .await;
        let record = match found {
            Ok(Ok(Some(record))) => record,
            Ok(Ok(None)) | Err(_) => {
                end(&h, None);
                return;
            }
            Ok(Err(error)) => {
                tracing::error!(
                    error = format!("{error:#}"),
                    "the NVIDIA components cannot be used; download them again"
                );
                end(&h, Some(format!("{error:#}")));
                return;
            }
        };
        let activated = activate(&h).await;
        if record.version == VERSION || activated.is_err() {
            end(&h, activated.err());
            return;
        }
        end(&h, None);
        update(&h, &record.version).await;
    });
}

/// While an earlier version's component is installed, fetches this
/// version's, with a growing pause between tries (the release may not carry
/// it yet), until it is installed or the component is removed or replaced.
/// It waits while a stream runs, whose bandwidth it would take. A failed try
/// shows on the console, where the user can try again.
async fn update(h: &Shared, installed: &str) {
    let mut pause = Duration::from_secs(300);
    loop {
        while crate::updater::streaming(h) {
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
        let root = root(h);
        let current = tokio::task::spawn_blocking(move || Record::read(&root)).await;
        if !matches!(current, Ok(Ok(Some(ref record))) if record.version != VERSION) {
            return;
        }
        if begin(h, "download").is_ok() {
            tracing::info!(%installed, "fetching this version's NVIDIA components");
            let fetched = fetch(h).await;
            let done = fetched.is_ok();
            let outcome = finish(h, fetched).await;
            end(h, outcome.err());
            if done {
                return;
            }
        }
        tokio::time::sleep(pause).await;
        pause = (pause * 2).min(Duration::from_secs(3600));
    }
}

/// Names the installed component to the loader and asks the GPU and driver
/// what they offer with it (in a process of its own).
async fn activate(h: &Shared) -> Result<(), String> {
    let profile = h.directory.clone();
    let found = tokio::task::spawn_blocking(move || component_for(&profile))
        .await
        .map_err(|error| error.to_string())?;
    match found {
        Ok(component) => loader::set_component(Some(component)),
        Err(error) => {
            tracing::error!(
                error = format!("{error:#}"),
                "the NVIDIA components cannot be used"
            );
            loader::set_component(None);
            h.nvidia_enhancements.store(0, Ordering::Release);
            return Err(format!("{error:#}"));
        }
    }
    let probe = h.clone();
    tokio::task::spawn_blocking(move || crate::nvidia_enhance::probe(&probe))
        .await
        .map_err(|error| error.to_string())
}

/// After a download: the new component is checked with the GPU and driver
/// (the job shows `check` meanwhile), and what it replaced or what a failed
/// download left goes. The job's end is the caller's.
async fn finish(h: &Shared, result: Result<Record>) -> Result<(), String> {
    let root = root(h);
    match result {
        Ok(record) => {
            tracing::info!(version = %record.version, folder = %record.folder, "NVIDIA components installed");
            advance(h, "check");
            // A running stream keeps the adapter it loaded; the new one is
            // named to the loader once no stream runs, so that one process
            // never runs two copies of NGX.
            while crate::updater::streaming(h) {
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            let activated = activate(h).await;
            let cleaned =
                tokio::task::spawn_blocking(move || clean(&root, Some(&record.folder))).await;
            if let Ok(Err(error)) = cleaned {
                tracing::warn!(
                    error = format!("{error:#}"),
                    "earlier NVIDIA component files stay"
                );
            }
            activated
        }
        Err(error) => {
            tracing::error!(
                error = format!("{error:#}"),
                "the NVIDIA components were not installed"
            );
            let cleaned = tokio::task::spawn_blocking(move || {
                let keep = Record::read(&root)?;
                clean(&root, keep.as_ref().map(|record| record.folder.as_str()))
            })
            .await;
            if let Ok(Err(cleanup)) = cleaned {
                tracing::warn!(
                    error = format!("{cleanup:#}"),
                    "a failed NVIDIA component download left files"
                );
            }
            Err(format!("{error:#}"))
        }
    }
}

/// Deletes what the component folder holds besides `keep` and NGX's data:
/// staged and earlier files. Refuses a folder that is not this host's own
/// ([`trusted`]) and never follows a link in it; a file still loaded stays
/// until the next start.
fn clean(root: &Path, keep: Option<&str>) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    if !exists(root)? {
        return Ok(());
    }
    trusted(root, elevated())?;
    for entry in std::fs::read_dir(root)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if Some(name.as_str()) == keep || name == NGX_DATA || name == RECORD {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let removed = if meta.file_attributes() & 0x400 != 0 {
            // A link: only the link itself goes.
            std::fs::remove_dir(&path).or_else(|_| std::fs::remove_file(&path))
        } else if meta.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if let Err(error) = removed {
            tracing::debug!(path = %path.display(), %error, "an earlier NVIDIA component file stays until the next start");
        }
    }
    Ok(())
}

/// NVIDIA's files this host trusts.
fn runtime_pins(h: &Shared) -> Vec<Pinned> {
    #[cfg(test)]
    if let Some(pins) = h.nvidia_component.runtime.lock().unwrap().clone() {
        return pins;
    }
    let _ = h;
    pins::RUNTIME.to_vec()
}

/// GitHub's API and download origins.
fn origins(h: &Shared) -> (String, String) {
    #[cfg(test)]
    if let Some(base) = h.nvidia_component.origin.lock().unwrap().clone() {
        return (format!("{base}/api"), base);
    }
    let _ = h;
    ("https://api.github.com".into(), "https://github.com".into())
}

fn http(h: &Shared, timeout: Duration) -> Result<reqwest::Client> {
    #[cfg(test)]
    if h.nvidia_component.origin.lock().unwrap().is_some() {
        return Ok(reqwest::Client::builder().timeout(timeout).build()?);
    }
    let _ = h;
    crate::updater::client(timeout)
}

/// A release asset as GitHub describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Asset {
    name: String,
    url: String,
    size: u64,
    sha256: String,
}

/// `name` in `release`: from a Rubylight release repository, with GitHub's
/// SHA-256 digest and a sane size.
fn asset(release: &Value, name: &str, downloads: &str) -> Result<Asset> {
    let tag = release["tag_name"]
        .as_str()
        .context("the release has no tag")?;
    let found = release["assets"]
        .as_array()
        .and_then(|assets| assets.iter().find(|asset| asset["name"] == name))
        .with_context(|| {
            format!("this version's release has no {name} yet; try again once it is published")
        })?;
    let url = crate::updater::REPOSITORIES
        .iter()
        .map(|repository| format!("{downloads}/{repository}/releases/download/{tag}/{name}"))
        .find(|url| found["browser_download_url"] == *url)
        .with_context(|| format!("{name} is not from the Rubylight release repository"))?;
    let sha256 = found["digest"]
        .as_str()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .map(str::to_ascii_lowercase)
        .filter(|digest| pins::is_sha256(digest))
        .with_context(|| format!("GitHub has not supplied a SHA-256 digest for {name}"))?;
    let size = found["size"]
        .as_u64()
        .filter(|size| (1..=MAX_ASSET).contains(size))
        .with_context(|| format!("{name} has an invalid size"))?;
    Ok(Asset {
        name: name.into(),
        url,
        size,
        sha256,
    })
}

/// This host version's release.
async fn release(h: &Shared) -> Result<Value> {
    let (api, _) = origins(h);
    let client = http(h, Duration::from_secs(30))?;
    let mut repositories = crate::updater::REPOSITORIES.iter().peekable();
    while let Some(repository) = repositories.next() {
        let response = client
            .get(format!("{api}/repos/{repository}/releases/tags/v{VERSION}"))
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND && repositories.peek().is_some() {
            continue;
        }
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            bail!(
                "Rubylight {VERSION} has no published release to download the NVIDIA components from"
            );
        }
        return Ok(response.error_for_status()?.json().await?);
    }
    unreachable!("REPOSITORIES is not empty")
}

/// Downloads `asset` to `path`, checking its size and SHA-256 as it comes.
async fn download(h: &Shared, asset: &Asset, path: &Path, before: u64, total: u64) -> Result<()> {
    let _ = std::fs::remove_file(path);
    let mut response = http(h, Duration::from_secs(1800))?
        .get(&asset.url)
        .send()
        .await?
        .error_for_status()?;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await
        .with_context(|| format!("cannot create {}", path.display()))?;
    let mut hash = Sha256::new();
    let mut size = 0;
    while let Some(chunk) = response.chunk().await? {
        size += chunk.len() as u64;
        ensure!(
            size <= asset.size,
            "{} is larger than its release says",
            asset.name
        );
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        hash.update(&chunk);
        let mut job = h.nvidia_component.lock();
        job.done = before + size;
        job.total = total;
    }
    file.sync_all().await?;
    ensure!(
        size == asset.size,
        "{} is {size} bytes, its release says {}",
        asset.name,
        asset.size
    );
    ensure!(
        hex::encode(hash.finalize()) == asset.sha256,
        "{} does not match its release's SHA-256 digest; nothing was installed",
        asset.name
    );
    Ok(())
}

/// Takes the files the archive `archive` (the release asset `asset`) may
/// contribute ([`pins::wanted`]) into `into`: only entries at its top level,
/// none larger than [`MAX_ASSET`], none twice.
fn extract(archive: &Path, asset: &str, into: &Path) -> Result<Vec<String>> {
    use std::io::{Read, Write};
    let file = std::fs::File::open(archive)
        .with_context(|| format!("cannot read {}", archive.display()))?;
    let mut zip =
        zip::ZipArchive::new(file).with_context(|| format!("{asset} is not a zip archive"))?;
    let mut taken = Vec::new();
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let name = entry.name().to_owned();
        if entry.is_dir() || name.contains(['/', '\\']) || !pins::wanted(&name) {
            continue;
        }
        ensure!(entry.size() <= MAX_ASSET, "{name} in {asset} is too large");
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(into.join(&name))
            .with_context(|| format!("{name} appears twice in {asset}"))?;
        let mut limited = (&mut entry).take(MAX_ASSET + 1);
        let copied = std::io::copy(&mut limited, &mut out)?;
        ensure!(copied <= MAX_ASSET, "{name} in {asset} is too large");
        out.flush()?;
        taken.push(name);
    }
    Ok(taken)
}

/// A folder name from this version and the adapter's SHA-256.
fn folder_name(sha256: &str) -> String {
    let version: String = VERSION
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("{version}-{}", &sha256[..16])
}

/// Moves the checked files in `staging` to their folder in `root`, named
/// after the adapter ([`folder_name`]). The same files already there are
/// kept; a folder an earlier install left, perhaps with a file still loaded,
/// goes or is passed over for the next free name. A link is never used.
fn place(root: &Path, staging: &Path, sha256: &str, runtime: &[Pinned]) -> Result<PathBuf> {
    use std::os::windows::fs::MetadataExt;
    let base = folder_name(sha256);
    for n in 0..10 {
        let target = match n {
            0 => root.join(&base),
            n => root.join(format!("{base}-{n}")),
        };
        match std::fs::symlink_metadata(&target) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("cannot read {}", target.display()));
            }
            Ok(meta) if meta.is_dir() && meta.file_attributes() & 0x400 == 0 => {
                if pins::verify_with(&target, sha256, runtime).is_ok() {
                    std::fs::remove_dir_all(staging)?;
                    return Ok(target);
                }
                if std::fs::remove_dir_all(&target).is_err() {
                    continue;
                }
            }
            Ok(_) => continue,
        }
        std::fs::rename(staging, &target).with_context(|| {
            format!("cannot move the NVIDIA components to {}", target.display())
        })?;
        return Ok(target);
    }
    bail!(
        "{} holds no free folder for the NVIDIA components; restart the host and download them again",
        root.display()
    )
}

/// Downloads, checks and installs this version's component; returns its
/// record, already written.
async fn fetch(h: &Shared) -> Result<Record> {
    let root = root(h);
    let runtime = runtime_pins(h);
    {
        let root = root.clone();
        tokio::task::spawn_blocking(move || {
            ensure_root(&root)?;
            trusted(&root, elevated())
        })
        .await??;
    }
    let (_, downloads) = origins(h);
    let release = release(h).await?;
    let asset = asset(&release, &pins::asset(VERSION), &downloads)?;
    let staging = root.join(format!("staging-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir(&staging)
        .with_context(|| format!("cannot create {}", staging.display()))?;
    let archive = root.join(format!("{}.part", asset.name));
    download(h, &asset, &archive, 0, asset.size).await?;
    let (staged, sha256) = {
        let (root, staging) = (root.clone(), staging.clone());
        tokio::task::spawn_blocking(move || -> Result<(PathBuf, String)> {
            let taken = extract(&archive, &asset.name, &staging);
            let _ = std::fs::remove_file(&archive);
            let taken = taken?;
            ensure!(
                taken.iter().any(|name| name == ADAPTER),
                "{} holds no {ADAPTER}",
                asset.name
            );
            for file in &runtime {
                pins::check_pinned(&staging.join(file.name), file)?;
            }
            let (sha256, _) = pins::hash_file(&staging.join(ADAPTER))?;
            pins::verify_with(&staging, &sha256, &runtime)?;
            let target = place(&root, &staging, &sha256, &runtime)?;
            Ok((target, sha256))
        })
        .await??
    };
    let record = Record {
        version: VERSION.into(),
        folder: staged
            .file_name()
            .context("the component folder has no name")?
            .to_string_lossy()
            .into_owned(),
        adapter_sha256: sha256,
    };
    record.write(&root)?;
    Ok(record)
}

#[cfg(test)]
mod tests;
