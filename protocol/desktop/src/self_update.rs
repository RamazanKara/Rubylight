//! The steps behind the launcher's Update now: what kind of copy this is, which release asset
//! updates it, the checked download and the hand-over.
//!
//! A copy the Windows setup installed runs the new setup silently and exits; the setup replaces
//! the files in the same folder and starts Nightjar again (/RELAUNCH in nightjar.iss). A copy run
//! as an AppImage has its file replaced atomically and starts the new one. Every other copy has no
//! Update now: a Windows copy the setup didn't install keeps the link to the release page, and a
//! deb, rpm, Flatpak or archive doesn't check at all, as its package manager or its user updates it.
//! Either way the next Nightjar can start while this one is still leaving, so what it reads is saved
//! before the hand-over (save_for_next_run in main.rs).
//!
//! The asset's size and SHA-256 come from GitHub's release answer. Nothing runs without both
//! matching, and only https addresses at GitHub's own hosts are ever connected to, redirects included.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::update_net::{Response, Transport, Url};

const REPOSITORY: &str = "RamazanKara/nightjar";
/// The releases are around 12 MB.
const MAX_SIZE: u64 = 512 << 20;
const MAX_REDIRECTS: usize = 5;
const PART_PREFIX: &str = ".nightjar-update-";
/// A leftover younger than this may belong to another running Nightjar.
const STALE_AFTER: Duration = Duration::from_secs(3600);
/// Exists while any Nightjar streams. The setup's AppMutex names it too (nightjar.iss): Setup stops
/// before it deletes anything, instead of closing that stream with Restart Manager.
pub const STREAM_MUTEX: &str = "Local\\NightjarStream";
/// Exists while the setup runs (SetupMutex in nightjar.iss).
const SETUP_MUTEX: &str = "Local\\NightjarSetup";
/// What the setup starts Nightjar with when it ends, so that isn't taken for a launch during the install.
const AFTER_SETUP: &str = "--after-setup";
/// How long a started program gets to fail at once. After this it runs, and Nightjar can go.
const START_WATCH: Duration = Duration::from_secs(1);
/// The entry the setup writes for its AppId (nightjar.iss): per user, so under HKCU.
#[cfg(windows)]
const UNINSTALL_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{6A3C8F12-5E7B-4D2A-9C41-7B0E8D3F2A65}_is1";

/// Why an update didn't happen. update_check.rs says each of these in the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    NoInstaller,
    NoDigest,
    /// The address, or the one a redirect pointed to, isn't one of GitHub's.
    Address(String),
    Digest,
    Size,
    Network(String),
    /// The setup didn't start or ended at once, or the replacement didn't happen.
    Apply(String),
    /// The AppImage is replaced, but the new one didn't start.
    Restart(String),
    /// Nightjar came back on its old version: the last run's setup failed.
    NotInstalled,
    /// Another Nightjar process is streaming; the setup would close it.
    StreamElsewhere,
    Cancelled,
}

impl Failure {
    /// Trying the same release again can't help.
    pub fn is_final(&self) -> bool {
        matches!(self, Self::NoInstaller | Self::NoDigest | Self::Address(_) | Self::Restart(_))
    }
}

/// The kind of copy that is running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CopyKind {
    /// The setup installed this folder.
    Installed,
    /// Windows, anywhere else: an unpacked zip or a build.
    Portable,
    AppImage(PathBuf),
    /// deb, rpm, Flatpak, an unpacked archive or a build.
    Managed,
}

impl CopyKind {
    /// Whether this copy looks for updates at all.
    pub fn checks(&self) -> bool {
        !matches!(self, Self::Managed)
    }

    /// The release asset that updates this copy, for a version.
    pub fn asset_name(&self, version: &str) -> Option<String> {
        match self {
            Self::Installed => Some(format!("nightjar-desktop-{version}-windows-x86_64-setup.exe")),
            Self::AppImage(_) => Some(format!("nightjar-desktop-{version}-x86_64.AppImage")),
            Self::Portable | Self::Managed => None,
        }
    }
}

/// This process's kind of copy, decided once.
pub fn current() -> &'static CopyKind {
    static COPY: OnceLock<CopyKind> = OnceLock::new();
    COPY.get_or_init(|| {
        let fallback = if cfg!(windows) { CopyKind::Portable } else { CopyKind::Managed };
        let copy = match std::env::current_exe() {
            Ok(exe) => detect(&exe),
            Err(e) => {
                eprintln!("nightjar: update: no path to this program: {e}");
                fallback
            }
        };
        eprintln!("nightjar: update: this copy is {copy:?}");
        copy
    })
}

fn detect(exe: &Path) -> CopyKind {
    if cfg!(windows) { return windows_copy(exe, install_location().as_deref()); }
    let env = |name| std::env::var_os(name).map(PathBuf::from);
    appimage_copy(env("APPIMAGE").as_deref(), env("APPDIR").as_deref(), exe)
}

/// Installed only when the folder of this program is the one the setup's uninstall entry names,
/// the test the setup makes itself for an upgrade (IsUpgradeOfThisFolder). A zip unpacked or a
/// build run on a PC that also has Nightjar installed stays portable.
pub fn windows_copy(exe: &Path, install_location: Option<&Path>) -> CopyKind {
    let folder = |path: &Path| path.canonicalize().ok().map(|p| p.to_string_lossy().to_lowercase());
    let here = exe.parent().and_then(folder);
    match (here, install_location.and_then(folder)) {
        (Some(here), Some(installed)) if here == installed => CopyKind::Installed,
        _ => CopyKind::Portable,
    }
}

/// An AppImage only when its runtime's $APPIMAGE names a file and this program runs from inside
/// its $APPDIR: the variable alone can leak into a child process that is something else.
pub fn appimage_copy(appimage: Option<&Path>, appdir: Option<&Path>, exe: &Path) -> CopyKind {
    let (Some(appimage), Some(appdir)) = (appimage, appdir) else { return CopyKind::Managed };
    let inside = appdir.canonicalize().ok().zip(exe.canonicalize().ok()).is_some_and(|(dir, exe)| exe.starts_with(dir));
    match appimage.canonicalize() {
        Ok(path) if inside && path.is_file() => CopyKind::AppImage(path),
        _ => CopyKind::Managed,
    }
}

#[cfg(windows)]
fn install_location() -> Option<PathBuf> {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};

    let mut text = vec![0u16; 4 * 1024];
    let mut size = (text.len() * 2) as u32;
    // SAFETY: the buffer holds `size` bytes and the strings outlive the call.
    let status = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, &HSTRING::from(UNINSTALL_KEY), &HSTRING::from("InstallLocation"), RRF_RT_REG_SZ, None, Some(text.as_mut_ptr().cast()), Some(&mut size))
    };
    if status.is_err() { return None; }
    let end = (size as usize / 2).saturating_sub(1);
    Some(PathBuf::from(String::from_utf16_lossy(&text[..end])))
}

#[cfg(not(windows))]
fn install_location() -> Option<PathBuf> {
    None
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub digest: String,
    pub size: u64,
}

/// The named asset of a release answer, with what the download is checked against.
pub fn asset(release: &serde_json::Value, tag: &str, name: &str) -> Result<Asset, Failure> {
    let found = release["assets"].as_array().and_then(|list| list.iter().find(|asset| asset["name"] == name)).ok_or(Failure::NoInstaller)?;
    let url = format!("https://github.com/{REPOSITORY}/releases/download/{tag}/{name}");
    if found["browser_download_url"] != url.as_str() {
        return Err(Failure::Address(found["browser_download_url"].as_str().unwrap_or("").to_owned()));
    }
    let digest = found["digest"]
        .as_str()
        .and_then(|d| d.strip_prefix("sha256:"))
        .filter(|d| d.len() == 64 && d.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or(Failure::NoDigest)?;
    let size = found["size"].as_u64().filter(|s| *s > 0 && *s <= MAX_SIZE).ok_or(Failure::Size)?;
    Ok(Asset { name: name.to_owned(), url, digest: digest.to_ascii_lowercase(), size })
}

/// The hosts a release download goes through, GitHub's redirect to its asset storage included.
pub fn download_host(url: &Url) -> bool {
    url.secure && url.port == 443 && matches!(url.host.as_str(), "github.com" | "release-assets.githubusercontent.com" | "objects.githubusercontent.com")
}

/// The host of the release question.
pub fn api_host(url: &Url) -> bool {
    url.secure && url.port == 443 && url.host == "api.github.com"
}

/// A GET that follows redirects itself, judging each address before connecting to it.
pub fn open(net: &dyn Transport, address: &str, allow: fn(&Url) -> bool, api: bool) -> Result<Response, Failure> {
    let mut url = Url::parse(address).ok_or_else(|| Failure::Address(address.to_owned()))?;
    for _ in 0..=MAX_REDIRECTS {
        if !allow(&url) { return Err(Failure::Address(url.host)); }
        let response = net.get(&url, api).map_err(Failure::Network)?;
        if !matches!(response.status, 301 | 302 | 303 | 307 | 308) { return Ok(response); }
        let location = response.location.as_deref().ok_or_else(|| Failure::Network("a redirect to nowhere".into()))?;
        url = url.resolve(location).ok_or_else(|| Failure::Address(location.to_owned()))?;
    }
    Err(Failure::Network("too many redirects".into()))
}

/// What the launcher and the download thread share.
pub struct Progress {
    pub cancel: AtomicBool,
    pub received: AtomicU64,
    pub wake: crate::hosts::Waker,
}

/// Downloads to `part` and checks it. Afterwards `part` is the complete, verified file, or gone.
pub fn download(net: &dyn Transport, allow: fn(&Url) -> bool, asset: &Asset, part: &Path, progress: &Progress) -> Result<(), Failure> {
    let result = download_inner(net, allow, asset, part, progress);
    if result.is_err() {
        if let Err(e) = std::fs::remove_file(part) {
            if e.kind() != std::io::ErrorKind::NotFound { eprintln!("nightjar: update: leftover {}: {e}", part.display()); }
        }
    }
    result
}

fn download_inner(net: &dyn Transport, allow: fn(&Url) -> bool, asset: &Asset, part: &Path, progress: &Progress) -> Result<(), Failure> {
    let cancelled = || progress.cancel.load(Ordering::Relaxed);
    let mut response = open(net, &asset.url, allow, false)?;
    if response.status != 200 { return Err(Failure::Network(format!("HTTP {}", response.status))); }
    if response.length.is_some_and(|length| length != asset.size) { return Err(Failure::Size); }
    let local = |e: std::io::Error| Failure::Apply(format!("{}: {e}", part.display()));
    if let Some(dir) = part.parent() { std::fs::create_dir_all(dir).map_err(local)?; }
    let mut file = File::create(part).map_err(local)?;
    let mut hash = Sha256::new();
    let mut received = 0u64;
    let mut chunk = vec![0u8; 64 * 1024];
    let mut woke = Instant::now();
    loop {
        if cancelled() { return Err(Failure::Cancelled); }
        let read = response.body.read(&mut chunk).map_err(|e| Failure::Network(e.to_string()))?;
        if read == 0 { break; }
        received += read as u64;
        if received > asset.size { return Err(Failure::Size); }
        hash.update(&chunk[..read]);
        file.write_all(&chunk[..read]).map_err(local)?;
        progress.received.store(received, Ordering::Relaxed);
        if woke.elapsed() > Duration::from_millis(100) {
            woke = Instant::now();
            (progress.wake)();
        }
    }
    if cancelled() { return Err(Failure::Cancelled); }
    if received != asset.size { return Err(Failure::Size); }
    let digest: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if digest != asset.digest { return Err(Failure::Digest); }
    file.sync_all().map_err(local)?;
    Ok(())
}

/// How one copy gets updated; what a test replaces.
pub struct Steps {
    pub net: Arc<dyn Transport>,
    pub allow: fn(&Url) -> bool,
    /// Where the download goes while it runs.
    pub dir: PathBuf,
    /// Only Nightjar's updates live in `dir`, not, say, the user's folder beside an AppImage.
    pub own_dir: bool,
    /// Says on the next launch that this attempt didn't work.
    pub marker: Option<PathBuf>,
    /// Whether another Nightjar process is streaming.
    pub stream_elsewhere: fn() -> bool,
    /// Whether a setup is running already, another Nightjar's Update now.
    pub setup_running: fn() -> bool,
    /// Takes the verified download. Ok means Nightjar should exit now.
    pub apply: Apply,
}

pub type Apply = Box<dyn Fn(&Path, &Asset) -> Result<(), Failure> + Send>;

impl Steps {
    /// The real steps for a copy that can update itself.
    pub fn for_copy(copy: &CopyKind, net: Arc<dyn Transport>) -> Option<Self> {
        let marker = marker_path();
        let stream_elsewhere = || mutex_exists(STREAM_MUTEX);
        let setup_running = || mutex_exists(SETUP_MUTEX);
        match copy {
            CopyKind::Installed => {
                let local = directories::BaseDirs::new().map(|dirs| dirs.data_local_dir().to_path_buf());
                let Some(dir) = update_dir(std::env::var_os("NIGHTJAR_CONFIG_DIR").map(PathBuf::from), local) else {
                    eprintln!("nightjar: update: no folder for the download");
                    return None;
                };
                Some(Self { net, allow: download_host, dir, own_dir: true, marker, stream_elsewhere, setup_running, apply: Box::new(run_setup) })
            }
            CopyKind::AppImage(path) => {
                let target = path.clone();
                let apply = move |part: &Path, _: &Asset| {
                    replace_file(part, &target).map_err(Failure::Apply)?;
                    start_watched(&target, &restart_arguments(), START_WATCH).map_err(Failure::Restart)
                };
                // The same folder as the AppImage: a rename across file systems isn't atomic.
                Some(Self { net, allow: download_host, dir: path.parent()?.to_path_buf(), own_dir: false, marker, stream_elsewhere, setup_running, apply: Box::new(apply) })
            }
            CopyKind::Portable | CopyKind::Managed => None,
        }
    }

    /// A name for this download's unfinished file.
    pub fn part(&self) -> PathBuf {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        self.dir.join(format!("{PART_PREFIX}{}-{}.part", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
    }
}

/// Where a setup waits to run: the user's own folder, as TEMP can be a shared one where someone else
/// could put a file in its place. Under NIGHTJAR_CONFIG_DIR when that is set (tests, the harness).
fn update_dir(config_override: Option<PathBuf>, local_data: Option<PathBuf>) -> Option<PathBuf> {
    match config_override {
        Some(config) => Some(config.join("update")),
        None => local_data.map(|local| local.join("Nightjar").join("update")),
    }
}

/// A named mutex that exists as long as this does. Nobody owns it: what counts is that it exists, as
/// for Inno's AppMutex, and several Nightjars can stream at once.
pub struct Mark {
    #[cfg(windows)]
    handle: windows::Win32::Foundation::HANDLE,
}

impl Mark {
    #[cfg(windows)]
    pub fn new(name: &str) -> Result<Self, String> {
        // SAFETY: the name outlives the call; the handle is closed once, in Drop.
        unsafe { windows::Win32::System::Threading::CreateMutexW(None, false, &windows::core::HSTRING::from(name)) }
            .map(|handle| Self { handle })
            .map_err(|e| format!("{name}: {e}"))
    }

    /// Only Windows setups close running programs.
    #[cfg(not(windows))]
    pub fn new(_: &str) -> Result<Self, String> {
        Ok(Self {})
    }
}

#[cfg(windows)]
impl Drop for Mark {
    fn drop(&mut self) {
        // SAFETY: the handle CreateMutexW returned, closed once.
        unsafe { let _ = windows::Win32::Foundation::CloseHandle(self.handle); }
    }
}

/// Whether some process holds a [`Mark`] of this name.
#[cfg(windows)]
pub fn mutex_exists(name: &str) -> bool {
    use windows::Win32::Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND};
    use windows::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
    // SAFETY: the name outlives the call; the handle is closed at once.
    match unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, &windows::core::HSTRING::from(name)) } {
        Ok(handle) => {
            unsafe { let _ = CloseHandle(handle); }
            true
        }
        // Any other answer (denied, for an elevated Nightjar) still means it exists.
        Err(e) => e.code() != ERROR_FILE_NOT_FOUND.to_hresult(),
    }
}

#[cfg(not(windows))]
pub fn mutex_exists(_: &str) -> bool {
    false
}

/// The setup's command line. /SILENT shows its progress and asks nothing; /NOCANCEL because stopping
/// half way would leave new and old files mixed. /RELAUNCH is ours: nightjar.iss starts Nightjar
/// again when it ends. No /DIR: an upgrade stays in the folder of the install it upgrades, and keeps
/// its tasks.
pub fn setup_arguments(log: &Path) -> Vec<String> {
    let mut arguments: Vec<String> = ["/SILENT", "/SUPPRESSMSGBOXES", "/NOCANCEL", "/NORESTART", "/SP-", "/RELAUNCH"].map(str::to_owned).to_vec();
    arguments.push(format!("/LOG={}", log.display()));
    arguments
}

fn run_setup(part: &Path, asset: &Asset) -> Result<(), Failure> {
    // Its asset name: Windows runs only what is called .exe.
    let setup = part.with_file_name(&asset.name);
    std::fs::rename(part, &setup).map_err(|e| {
        let _ = std::fs::remove_file(part);
        Failure::Apply(format!("{}: {e}", setup.display()))
    })?;
    start_watched(&setup, &setup_arguments(&setup.with_extension("log")), START_WATCH).map_err(|e| {
        let _ = std::fs::remove_file(&setup);
        Failure::Apply(e)
    })
}

/// Starts a program that outlives Nightjar, and stays for `watch` to see it isn't gone at once with
/// a failure: Nightjar must not exit for something that already ended.
fn start_watched(program: &Path, arguments: &[String], watch: Duration) -> Result<(), String> {
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{}: {e}", program.display()))?;
    let started = Instant::now();
    while started.elapsed() < watch {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("{}: {status}", program.display())),
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => {
                // It did start; only the watching failed.
                eprintln!("nightjar: update: {}: {e}", program.display());
                return Ok(());
            }
        }
    }
    Ok(())
}

/// An update's restart says so, as the setup's does: it is no launch that starts a game.
fn restart_arguments() -> Vec<String> {
    vec![AFTER_SETUP.to_owned()]
}

/// Takes the setup's word out of the command line, and says whether it was there.
pub fn split_after_setup(mut args: Vec<std::ffi::OsString>) -> (Vec<std::ffi::OsString>, bool) {
    let before = args.len();
    args.retain(|arg| arg != AFTER_SETUP);
    let found = args.len() != before;
    (args, found)
}

/// A launch while this user's Update now is installing: it has to end before it reads the marker,
/// which is the setup's result, or holds on to the files the setup is replacing. The setup's own
/// start at its end isn't one.
pub fn install_running(after_setup: bool) -> bool {
    !after_setup && marker_path().is_some_and(|marker| marker.exists()) && mutex_exists(SETUP_MUTEX)
}

fn marker_path() -> Option<PathBuf> {
    crate::config::config_dir().ok().map(|dir| dir.join("update-pending.json"))
}

/// Puts the verified `part` in `target`'s place in one step, runnable as `target` was. A failure
/// leaves `target` as it was and removes `part`.
pub fn replace_file(part: &Path, target: &Path) -> Result<(), String> {
    let replaced = make_runnable_like(part, target).and_then(|()| std::fs::rename(part, target));
    if let Err(e) = replaced {
        let _ = std::fs::remove_file(part);
        return Err(format!("{}: {e}", target.display()));
    }
    #[cfg(unix)]
    if let Some(dir) = target.parent() {
        // The rename reaches the disk; the new file itself already has (download).
        if let Err(e) = File::open(dir).and_then(|dir| dir.sync_all()) { eprintln!("nightjar: update: {}: {e}", dir.display()); }
    }
    Ok(())
}

#[cfg(unix)]
fn make_runnable_like(part: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(target)?.permissions().mode() | 0o700;
    std::fs::set_permissions(part, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn make_runnable_like(_: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::metadata(target).map(drop)
}

/// Removes what an earlier attempt left in `dir`: an unfinished download, or a setup and its log,
/// which are still in use while Nightjar comes back. In a folder that isn't Nightjar's own
/// (`own_dir`) a setup with that name is the user's, and only the unfinished downloads count.
pub fn remove_stale(dir: &Path, own_dir: bool) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let ours = name.starts_with(PART_PREFIX) || (own_dir && name.starts_with("nightjar-desktop-") && (name.ends_with("-setup.exe") || name.ends_with("-setup.log")));
        let old = entry.metadata().and_then(|m| m.modified()).is_ok_and(|at| at.elapsed().is_ok_and(|age| age > STALE_AFTER));
        if ours && old {
            if let Err(e) = std::fs::remove_file(entry.path()) { eprintln!("nightjar: update: leftover {name}: {e}"); }
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Attempt {
    pub version: String,
    pub url: String,
}

/// Written before Nightjar exits for an update: the setup has no way to say that it failed.
pub fn write_attempt(path: &Path, attempt: &Attempt) -> std::io::Result<()> {
    if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
    std::fs::write(path, serde_json::to_vec(attempt).map_err(std::io::Error::other)?)
}

/// The attempt the last run left, once. A release page only from GitHub, as for the notice.
pub fn take_attempt(path: &Path) -> Option<Attempt> {
    let bytes = std::fs::read(path).ok()?;
    if let Err(e) = std::fs::remove_file(path) { eprintln!("nightjar: update: {}: {e}", path.display()); }
    serde_json::from_slice::<Attempt>(&bytes).ok().filter(|a| a.url.starts_with("https://github.com/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!("nj-self-update-{}-{}-{name}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn only_the_folder_the_setup_installed_is_installed() {
        let installed = folder("installed");
        let zip = folder("zip");
        std::fs::write(installed.join("nightjar.exe"), b"").unwrap();
        std::fs::write(zip.join("nightjar.exe"), b"").unwrap();
        assert_eq!(windows_copy(&installed.join("nightjar.exe"), Some(&installed)), CopyKind::Installed);
        let upper = PathBuf::from(installed.to_string_lossy().to_uppercase());
        if cfg!(windows) { assert_eq!(windows_copy(&installed.join("nightjar.exe"), Some(&upper)), CopyKind::Installed, "Windows paths ignore case"); }
        assert_eq!(windows_copy(&zip.join("nightjar.exe"), Some(&installed)), CopyKind::Portable, "a zip on a PC that also has it installed");
        assert_eq!(windows_copy(&zip.join("nightjar.exe"), None), CopyKind::Portable, "no uninstall entry");
        assert_eq!(windows_copy(&zip.join("nightjar.exe"), Some(&zip.join("gone"))), CopyKind::Portable, "an entry for a folder that's gone");
    }

    #[test]
    fn a_portable_copy_has_no_asset_and_nothing_to_run() {
        let net: Arc<dyn Transport> = Arc::new(crate::update_net::Native::new("test".into()));
        assert_eq!(CopyKind::Portable.asset_name("4.8.0"), None);
        assert!(Steps::for_copy(&CopyKind::Portable, net.clone()).is_none());
        assert!(Steps::for_copy(&CopyKind::Managed, net).is_none());
        assert!(CopyKind::Portable.checks() && !CopyKind::Managed.checks(), "portable keeps its link; a package manager's copy stays out of it");
    }

    #[test]
    fn an_appimage_is_its_runtimes_word_and_where_this_program_runs() {
        let dir = folder("appimage");
        let image = dir.join("Nightjar.AppImage");
        let appdir = dir.join("mount");
        std::fs::create_dir_all(&appdir).unwrap();
        std::fs::write(&image, b"").unwrap();
        std::fs::write(appdir.join("nightjar"), b"").unwrap();
        std::fs::write(dir.join("nightjar"), b"").unwrap();
        let exe = appdir.join("nightjar");
        assert_eq!(appimage_copy(Some(&image), Some(&appdir), &exe), CopyKind::AppImage(image.canonicalize().unwrap()));
        assert_eq!(appimage_copy(None, None, &exe), CopyKind::Managed, "a deb, an rpm or a Flatpak");
        assert_eq!(appimage_copy(Some(&image), None, &exe), CopyKind::Managed, "no runtime");
        assert_eq!(appimage_copy(Some(&image), Some(&appdir), &dir.join("nightjar")), CopyKind::Managed, "a variable leaked into something else");
        assert_eq!(appimage_copy(Some(&dir.join("gone")), Some(&appdir), &exe), CopyKind::Managed);
    }

    #[test]
    fn the_asset_names_are_the_ones_the_release_publishes() {
        assert_eq!(CopyKind::Installed.asset_name("4.7.0").unwrap(), "nightjar-desktop-4.7.0-windows-x86_64-setup.exe");
        assert_eq!(CopyKind::AppImage("x".into()).asset_name("4.7.0").unwrap(), "nightjar-desktop-4.7.0-x86_64.AppImage");
    }

    #[test]
    fn only_githubs_https_hosts_are_allowed() {
        let allowed = |a: &str| download_host(&Url::parse(a).unwrap());
        assert!(allowed("https://github.com/RamazanKara/nightjar/releases/download/v1/x"));
        assert!(allowed("https://release-assets.githubusercontent.com/x?sig=1"));
        assert!(allowed("https://objects.githubusercontent.com/x"));
        assert!(!allowed("http://github.com/x"), "no plain http");
        assert!(!allowed("https://github.com:8443/x"), "no other port");
        assert!(!allowed("https://evil.example/x"));
        assert!(!allowed("https://github.com.evil.example/x"));
        assert!(!allowed("https://evilgithub.com/x"));
        assert!(!allowed("https://api.github.com/x"));
        assert!(api_host(&Url::parse("https://api.github.com/x").unwrap()));
        assert!(!api_host(&Url::parse("https://github.com/x").unwrap()));
    }

    #[test]
    fn the_silent_setup_keeps_its_folder_and_starts_nightjar_again() {
        let arguments = setup_arguments(Path::new("C:\\a b\\setup.log"));
        for wanted in ["/SILENT", "/SUPPRESSMSGBOXES", "/NOCANCEL", "/NORESTART", "/SP-", "/RELAUNCH", "/LOG=C:\\a b\\setup.log"] {
            assert!(arguments.iter().any(|a| a == wanted), "{wanted}");
        }
        assert!(!arguments.iter().any(|a| a.to_uppercase().starts_with("/DIR")), "the firewall asks once per path");
    }

    #[test]
    fn the_setup_keeps_what_the_update_relies_on() {
        let iss = include_str!("../packaging/windows/nightjar.iss");
        assert!(iss.contains("PrivilegesRequired=lowest"), "no UAC prompt");
        assert!(iss.contains("AppId={{6A3C8F12-5E7B-4D2A-9C41-7B0E8D3F2A65}"), "the same install is upgraded");
        assert!(iss.contains("DisableDirPage=yes") && !iss.contains("UsePreviousAppDir=no"), "the same folder");
        assert!(iss.contains("procedure DeinitializeSetup") && iss.contains("'/RELAUNCH'"), "Nightjar comes back");
        assert!(iss.contains(&format!("AppMutex={STREAM_MUTEX}\n")), "the setup stops for a stream");
        assert!(iss.contains(&format!("SetupMutex={SETUP_MUTEX}\n")), "a launch can tell the setup runs");
        assert!(iss.contains(&format!("'{AFTER_SETUP}'")) && iss.contains(&format!("Parameters: \"{AFTER_SETUP}\"")), "and the setup's own start says so");
        let data = iss.split("\n[InstallDelete]\n").nth(1).unwrap().split("\n[Files]\n").next().unwrap();
        assert!(!data.contains("userappdata") && !data.contains("config"), "an upgrade deletes nothing of the user's");
        assert!(!data.lines().any(|line| line.starts_with("Type:") && line.contains(".dll")), "no library goes before [Files]: an abort wouldn't bring it back");
        assert!(iss.contains("InstallFinished := True;\n    if WasUpgrade then\n      RemoveStaleDlls()"), "old libraries go only after success");
        assert!(iss.contains("if CurStep = ssInstall then begin\n    WasUpgrade := IsUpgradeOfThisFolder();\n    if WasUpgrade then\n      BackUpDlls();"), "the libraries [Files] replaces are kept first");
        let closing = iss.split("procedure DeinitializeSetup").nth(1).unwrap();
        assert!(closing.find("RestoreDlls()").unwrap() < closing.find("nightjar.exe").unwrap(), "an aborted install gets them back before Nightjar starts");
        assert!(iss.contains("if not InstallFinished and FindFirst(DllBackup"), "a finished one keeps the new ones");
        let files = iss.split("\n[Files]\n").nth(1).unwrap().split("\n[Icons]\n").next().unwrap();
        let last = files.lines().rfind(|line| line.starts_with("Source:")).unwrap();
        assert!(last.contains(r"\nightjar.exe"), "the program is replaced last: {last}");
        #[cfg(windows)]
        assert!(UNINSTALL_KEY.contains("{6A3C8F12-5E7B-4D2A-9C41-7B0E8D3F2A65}_is1") && iss.contains(UNINSTALL_KEY));
    }

    #[test]
    fn the_installer_smoke_runs_what_update_now_runs() {
        let smoke = include_str!("../packaging/windows/build-installer.ps1");
        let flags = smoke.lines().find_map(|line| line.strip_prefix("$updateFlags = ")).expect("the smoke's flags");
        let listed: Vec<String> = setup_arguments(Path::new("x")).iter().filter(|a| !a.starts_with("/LOG=")).map(|a| format!("'{a}'")).collect();
        assert_eq!(flags, format!("@({})", listed.join(", ")));
        assert!(smoke.contains(&format!("'{STREAM_MUTEX}'")) && smoke.contains(&format!("'{SETUP_MUTEX}'")), "the same mutexes");
        assert!(smoke.contains(&format!("'{AFTER_SETUP}'")));
    }

    #[cfg(windows)]
    #[test]
    fn a_mark_is_seen_from_outside_until_it_goes() {
        let name = format!("Local\\NightjarTest-{}", std::process::id());
        assert!(!mutex_exists(&name));
        let (first, second) = (Mark::new(&name).unwrap(), Mark::new(&name).unwrap());
        assert!(mutex_exists(&name));
        drop(first);
        assert!(mutex_exists(&name), "two streams: the other one still counts");
        drop(second);
        assert!(!mutex_exists(&name));
    }

    /// A command line that runs `script` in the system's shell.
    fn shell(script: &str) -> (PathBuf, Vec<String>) {
        let (program, flag) = if cfg!(windows) { ("cmd", "/c") } else { ("sh", "-c") };
        (program.into(), vec![flag.to_owned(), script.to_owned()])
    }

    #[test]
    fn a_program_that_fails_at_once_is_noticed() {
        let (program, arguments) = shell("exit 3");
        let failure = start_watched(&program, &arguments, Duration::from_secs(10)).unwrap_err();
        assert!(failure.contains('3'), "{failure}");
        let (program, arguments) = shell("exit 0");
        let started = Instant::now();
        assert_eq!(start_watched(&program, &arguments, Duration::from_secs(10)), Ok(()));
        assert!(started.elapsed() < Duration::from_secs(9), "no reason to wait on");
    }

    #[test]
    fn a_program_that_keeps_running_is_left_running() {
        let (program, arguments) = shell(if cfg!(windows) { "ping -n 4 127.0.0.1 >nul" } else { "sleep 3" });
        let started = Instant::now();
        assert_eq!(start_watched(&program, &arguments, Duration::from_millis(200)), Ok(()));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_setup_that_ends_at_once_with_a_failure_keeps_nightjar_open() {
        let dir = folder("early-exit");
        let part = dir.join("download.part");
        // A real program that refuses the setup's arguments.
        #[cfg(windows)]
        std::fs::copy(std::env::var_os("SystemRoot").map(PathBuf::from).unwrap().join("System32\\where.exe"), &part).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(&part, b"#!/bin/sh\nexit 3\n").unwrap();
            std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let name = "nightjar-desktop-9.0.0-windows-x86_64-setup.exe";
        let asset = Asset { name: name.into(), url: String::new(), digest: String::new(), size: 0 };
        assert!(matches!(run_setup(&part, &asset), Err(Failure::Apply(_))));
        assert!(!dir.join(name).exists(), "the setup goes again");
    }

    #[test]
    fn the_setups_word_never_reaches_the_command_line() {
        let args = |list: &[&str]| list.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
        assert_eq!(split_after_setup(restart_arguments().into_iter().map(Into::into).collect()), (args(&[]), true), "an AppImage's restart");
        assert_eq!(split_after_setup(args(&[])), (args(&[]), false));
        assert_eq!(split_after_setup(args(&["--host", "x"])), (args(&["--host", "x"]), false));
        assert_eq!(split_after_setup(args(&[AFTER_SETUP])), (args(&[]), true));
        assert_eq!(split_after_setup(args(&[AFTER_SETUP, "--host", "x"])), (args(&["--host", "x"]), true));
        assert!(!install_running(true), "the setup's own start always goes on");
    }

    #[test]
    fn the_setup_waits_in_the_users_own_folder() {
        let (config, local) = (PathBuf::from("config"), PathBuf::from("local"));
        assert_eq!(update_dir(None, Some(local.clone())), Some(local.join("Nightjar").join("update")));
        assert_eq!(update_dir(Some(config.clone()), Some(local)), Some(config.join("update")), "a sandboxed run stays in its sandbox");
        assert_eq!(update_dir(None, None), None);
        let iss = include_str!("../packaging/windows/nightjar.iss");
        assert!(iss.contains(r#"Name: "{localappdata}\Nightjar\update""#), "and it goes with the uninstall");
    }

    #[test]
    fn a_setup_that_cannot_take_its_name_leaves_no_file() {
        let dir = folder("rename");
        let part = dir.join("download.part");
        std::fs::write(&part, b"setup").unwrap();
        let name = "nightjar-desktop-9.0.0-windows-x86_64-setup.exe";
        std::fs::create_dir(dir.join(name)).unwrap();
        let asset = Asset { name: name.into(), url: String::new(), digest: String::new(), size: 5 };
        assert!(matches!(run_setup(&part, &asset), Err(Failure::Apply(_))));
        assert!(!part.exists());
    }

    #[test]
    fn a_setup_that_cannot_start_keeps_nightjar_open() {
        let dir = folder("setup");
        let part = dir.join("download.part");
        std::fs::write(&part, b"not a program").unwrap();
        let asset = Asset { name: "nightjar-desktop-9.0.0-windows-x86_64-setup.txt".into(), url: String::new(), digest: String::new(), size: 13 };
        assert!(matches!(run_setup(&part, &asset), Err(Failure::Apply(_))));
    }

    #[test]
    fn the_replacement_is_whole_and_runnable_or_not_at_all() {
        let dir = folder("replace");
        let target = dir.join("Nightjar.AppImage");
        std::fs::write(&target, b"old").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o750)).unwrap();
        }
        let part = dir.join(".nightjar-update-1.part");
        assert!(replace_file(&part, &target).is_err(), "nothing to put there");
        assert_eq!(std::fs::read(&target).unwrap(), b"old", "and the old one stays");
        std::fs::write(&part, b"new").unwrap();
        replace_file(&part, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert!(!part.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o750);
        }
        let gone = dir.join("gone.AppImage");
        std::fs::write(&part, b"new").unwrap();
        assert!(replace_file(&part, &gone).is_err(), "the AppImage was removed meanwhile");
        assert!(!part.exists() && !gone.exists());
    }

    #[test]
    fn only_old_leftovers_of_ours_are_cleared() {
        let dir = old_files("stale");
        remove_stale(&dir, true);
        assert_eq!(left_in(&dir), ["mine.txt", "nightjar-desktop-4.8.0-x86_64.AppImage"]);
    }

    #[test]
    fn beside_an_appimage_the_users_setup_stays() {
        let dir = old_files("beside-appimage");
        remove_stale(&dir, false);
        assert_eq!(left_in(&dir), ["mine.txt", "nightjar-desktop-4.8.0-windows-x86_64-setup.exe", "nightjar-desktop-4.8.0-x86_64.AppImage"]);
    }

    /// One young set of files proves nothing goes early; then all of it is two hours old.
    fn old_files(name: &str) -> PathBuf {
        let dir = folder(name);
        for name in [".nightjar-update-1-2.part", "nightjar-desktop-4.8.0-windows-x86_64-setup.exe", "nightjar-desktop-4.8.0-x86_64.AppImage", "mine.txt"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        remove_stale(&dir, true);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 4, "young ones may be another Nightjar's");
        let long_ago = std::time::SystemTime::now() - Duration::from_secs(7200);
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            File::options().write(true).open(entry.path()).unwrap().set_modified(long_ago).unwrap();
        }
        dir
    }

    fn left_in(dir: &Path) -> Vec<String> {
        let mut left: Vec<String> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        left
    }

    #[test]
    fn an_attempt_is_read_once_and_only_from_github() {
        let path = folder("attempt").join("update-pending.json");
        assert_eq!(take_attempt(&path), None);
        let attempt = Attempt { version: "4.8.0".into(), url: "https://github.com/RamazanKara/nightjar/releases/tag/v4.8.0".into() };
        write_attempt(&path, &attempt).unwrap();
        assert_eq!(take_attempt(&path), Some(attempt));
        assert_eq!(take_attempt(&path), None);
        std::fs::write(&path, br#"{"version":"4.8.0","url":"https://evil.example/"}"#).unwrap();
        assert_eq!(take_attempt(&path), None);
        std::fs::write(&path, b"<garbage>").unwrap();
        assert_eq!(take_attempt(&path), None);
        assert!(!path.exists());
    }
}
