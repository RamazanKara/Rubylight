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
/// How long a restarted AppImage gets to fail at once. After this it runs, and Nightjar can go.
const START_WATCH: Duration = Duration::from_secs(1);
/// How long a started setup can take to hold SETUP_MUTEX. Until it does it may yet fail: a freshly
/// copied program is scanned first, which took 0.7 s here on an idle machine and over a second
/// under load.
const SETUP_STARTING: Duration = Duration::from_secs(10);
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
    #[cfg(test)]
    pub fn asset_name(&self, version: &str) -> Option<String> {
        self.asset_names(version).into_iter().next()
    }

    /// The release assets that update this copy, best first: an install takes the setup of the
    /// PC's own architecture, and this build's when a release has none for the PC.
    pub fn asset_names(&self, version: &str) -> Vec<String> {
        match self {
            Self::Installed => windows_setup_assets(version, windows_machine(), BUILD_ARCH),
            Self::AppImage(_) => vec![appimage_asset(version, std::env::consts::ARCH)],
            Self::Portable | Self::Managed => Vec::new(),
        }
    }
}

/// This build's architecture in the Windows package names.
const BUILD_ARCH: &str = if cfg!(target_arch = "aarch64") { "arm64" } else { "x86_64" };

/// The setups an install on a `machine` PC running a `build` build takes, best first.
fn windows_setup_assets(version: &str, machine: &str, build: &str) -> Vec<String> {
    let mut names = vec![windows_setup_asset(version, machine)];
    if build != machine {
        names.push(windows_setup_asset(version, build));
    }
    names
}

/// The AppImage of one architecture (the names of `std::env::consts::ARCH`): an update stays on it.
fn appimage_asset(version: &str, arch: &str) -> String {
    format!("nightjar-desktop-{version}-{arch}.AppImage")
}

/// The Windows setup of one architecture, `x86_64` or `arm64`.
fn windows_setup_asset(version: &str, arch: &str) -> String {
    format!("nightjar-desktop-{version}-windows-{arch}-setup.exe")
}

/// The architecture of the PC's own Windows rather than of this program: the x86_64 build that
/// an ARM64 PC runs under emulation updates to the ARM64 setup, which installs into the same
/// folder (same AppId; the setup removes the DLLs it does not ship) and runs natively from then on.
fn windows_machine() -> &'static str {
    #[cfg(windows)]
    if native_machine() == Some(windows::Win32::System::SystemInformation::IMAGE_FILE_MACHINE_ARM64.0) {
        return "arm64";
    }
    BUILD_ARCH
}

/// IsWow64Process2's native machine (IMAGE_FILE_MACHINE_*). It is looked up at run time: Windows 10
/// before 1709 lacks it, and a static import would stop the exe from starting there. An x86_64
/// process under emulation is not WOW64, and only this call names the ARM64 machine underneath;
/// GetNativeSystemInfo says AMD64 to it.
#[cfg(windows)]
fn native_machine() -> Option<u16> {
    use windows::core::{s, w, BOOL};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows::Win32::System::Threading::GetCurrentProcess;

    type IsWow64Process2 = unsafe extern "system" fn(HANDLE, *mut u16, *mut u16) -> BOOL;
    // SAFETY: kernel32 is loaded in every process; the pointer is IsWow64Process2, whose signature
    // the type above spells out (USHORT for IMAGE_FILE_MACHINE), and both out-pointers are valid.
    unsafe {
        let kernel = GetModuleHandleW(w!("kernel32.dll")).ok()?;
        let function = std::mem::transmute::<unsafe extern "system" fn() -> isize, IsWow64Process2>(GetProcAddress(kernel, s!("IsWow64Process2"))?);
        let (mut process, mut native) = (0u16, 0u16);
        function(GetCurrentProcess(), &mut process, &mut native).as_bool().then_some(native)
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
                    start_watched(&target, &restart_arguments(), START_WATCH, || false).map_err(Failure::Restart)
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
    start_watched(&setup, &setup_arguments(&setup.with_extension("log")), SETUP_STARTING, || mutex_exists(SETUP_MUTEX)).map_err(|e| {
        let _ = std::fs::remove_file(&setup);
        Failure::Apply(e)
    })
}

/// Starts a program that outlives Nightjar, and stays for `watch`, or until it has `settled`, to see
/// it isn't gone at once with a failure: Nightjar must not exit for something that already ended.
fn start_watched(program: &Path, arguments: &[String], watch: Duration, settled: impl Fn() -> bool) -> Result<(), String> {
    let mut child = crate::amd_decode::for_child(&mut Command::new(program))
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
            Ok(None) if settled() => return Ok(()),
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
    !after_setup && installing(marker_path().as_deref().and_then(marker_age), mutex_exists(SETUP_MUTEX))
}

/// Windows' setup takes its mutex a moment after it starts: until then a young marker says the same.
fn installing(marker_age: Option<Duration>, setup_mutex: bool) -> bool {
    marker_age.is_some_and(|age| setup_mutex || (cfg!(windows) && age < SETUP_STARTING))
}

/// A marker with a clock that can't tell counts as an old one.
fn marker_age(marker: &Path) -> Option<Duration> {
    let written = std::fs::metadata(marker).ok()?.modified().ok();
    Some(written.and_then(|at| at.elapsed().ok()).unwrap_or(Duration::MAX))
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
mod tests;
