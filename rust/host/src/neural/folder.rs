//! Neural Rendering in one game's folder: find the game's .exe, refuse
//! anti-cheat and other upscaler mods, place a cached package under a free
//! proxy name, check it before a launch, and remove exactly what was placed.
//!
//! Game folders are the user's to write, so a SYSTEM host runs
//! `butterpollo.exe --neural-folder` with the signed-in user's token
//! ([`super::run_folder`]). It writes a folder itself only when the user may
//! not and nobody but administrators may change it ([`Writer::System`]).
//! Neither changes a folder through a link: one planted in a library that
//! other accounts share would send a copy or a delete elsewhere.
use butterpollo_core::neural::{
    self, MANIFEST, Manifest, Now, Occupant, PLACED_BY, PROXY_SOURCE, Placed, SETTINGS, SUMS,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    io::{BufRead, Read, Write},
    os::windows::fs::MetadataExt,
    path::{Path, PathBuf},
};

/// The hidden flag that makes the host exe do one folder request.
pub const FLAG: &str = "--neural-folder";
/// Marks the helper's answer and progress lines on its output.
pub const ANSWER: &str = "NEURAL-FOLDER-RESPONSE ";
pub const PROGRESS: &str = "NEURAL-FOLDER-PROGRESS ";
/// A cached package's description, beside its `files` folder.
pub const PACKAGE: &str = "package.json";
/// What a copy is called until it is complete.
const PARTIAL: &str = ".rubylight-partial";
/// What the user's helper creates and deletes to learn whether it may write
/// the game's folder.
const PROBE: &str = "rubylight-neural.probe";
/// Files at least this big are checked by size before a launch; hashing the
/// 760 MB model at every launch would delay the game.
const HASHED_AT_LAUNCH: u64 = 64 << 20;
/// How far the anti-cheat and .exe searches look into an install, and how
/// much they see before they give up (less in tests, which fill a folder).
const DEPTH: usize = 6;
const ENTRIES: usize = if cfg!(test) { 1_000 } else { 50_000 };
/// SYSTEM's, the Administrators group's and TrustedInstaller's (what Program
/// Files holds).
const ADMINISTRATORS: [&str; 3] = [
    "S-1-5-18",
    "S-1-5-32-544",
    "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
];
/// A junction, symbolic link or other reparse point.
const LINK: u32 = 0x400;

/// Who writes the game's folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Writer {
    /// The signed-in user's helper, or a host that is not SYSTEM.
    User,
    /// A SYSTEM host, in a folder the user may not write: only when it and
    /// every folder above it are no links and nobody but administrators may
    /// change them ([`administrators_only`]).
    System,
}

/// A release in the host's cache: `files/` holds the package's contents and
/// the runtime from the release's component, each checked against the
/// release's SHA256SUMS.txt or the archive's own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    pub tag: String,
    pub asset: String,
    /// GitHub's SHA-256 of `asset`, and of `runtime`: the cache serves the
    /// release only while its archives are these.
    #[serde(default)]
    pub digest: String,
    /// The runtime component the runtime came from, when not the package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_digest: Option<String>,
    pub files: BTreeMap<String, Placed>,
}
impl Package {
    pub fn read(folder: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(folder.join(PACKAGE)).map_err(|e| {
            format!(
                "the cached release in {} is unreadable: {e}",
                folder.display()
            )
        })?;
        let package: Self = serde_json::from_str(&text)
            .map_err(|e| format!("the cached release in {} is damaged: {e}", folder.display()))?;
        if !package
            .files
            .keys()
            .all(|path| neural::entry_path(path).as_ref() == Some(path))
        {
            return Err(format!(
                "the cached release in {} names a file outside it",
                folder.display()
            ));
        }
        Ok(package)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Place `package` beside `exe`, or beside the game's .exe found in `install`.
    Install {
        exe: Option<PathBuf>,
        install: Option<PathBuf>,
        package: PathBuf,
    },
    /// Before a launch: refuse anti-cheat, apply `package` when it is
    /// another release, and check that the placed files are still as placed.
    Prepare {
        exe: PathBuf,
        install: Option<PathBuf>,
        package: Option<PathBuf>,
    },
    Remove {
        exe: PathBuf,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Placed {
        exe: PathBuf,
        release: String,
        proxy: String,
        updated: bool,
    },
    Removed {
        /// Placed files that changed since and stayed.
        kept: Vec<String>,
    },
    /// The folder's drive or share is not there now; the mod may still be in it.
    Unreachable {
        folder: String,
    },
    /// No single .exe in the install is the game's: the user chooses it.
    ChooseExe {
        reason: String,
    },
    /// The game cannot have the mod (anti-cheat, another mod, a name taken).
    Refused {
        reason: String,
    },
    Error {
        message: String,
    },
    /// The signed-in user may not write the game's folder (a game in
    /// Program Files): a SYSTEM host may write it itself ([`Writer::System`]).
    NotWritable {
        folder: PathBuf,
    },
}

pub fn handle(request: Request, writer: Writer, progress: &dyn Fn(u64, u64)) -> Response {
    let result = match request {
        Request::Install {
            exe,
            install,
            package,
        } => find_exe(exe, install.as_deref()).and_then(|exe| {
            let package = Package::read(&package)
                .map_err(Failure::Error)?
                .with(package);
            refuse_anti_cheat(&exe, install.as_deref())?;
            place(&exe, &package, writer, progress)
        }),
        Request::Prepare {
            exe,
            install,
            package,
        } => prepare(&exe, install.as_deref(), package, writer, progress),
        Request::Remove { exe } => remove(folder_of(&exe), writer),
    };
    result.unwrap_or_else(|failure| match failure {
        Failure::ChooseExe(reason) => Response::ChooseExe { reason },
        Failure::Refused(reason) => Response::Refused { reason },
        Failure::Error(message) => Response::Error { message },
        Failure::NotWritable(folder) => Response::NotWritable { folder },
    })
}

/// The helper's entry point (`butterpollo.exe --neural-folder`): one request
/// line on stdin, progress and one answer line on stdout.
pub fn helper() -> i32 {
    let mut line = String::new();
    let response = if butterpollo_windows::process::is_system() {
        Response::Error {
            message: "the Neural Rendering helper must run as the signed-in user".into(),
        }
    } else if let Err(error) = std::io::stdin().lock().read_line(&mut line) {
        Response::Error {
            message: format!("unreadable helper request: {error}"),
        }
    } else {
        match serde_json::from_str(&line) {
            Ok(request) => handle(request, Writer::User, &|done, total| {
                let mut out = std::io::stdout().lock();
                let _ = writeln!(out, "{PROGRESS}{done} {total}");
                let _ = out.flush();
            }),
            Err(error) => Response::Error {
                message: format!("unreadable helper request: {error}"),
            },
        }
    };
    let answer = serde_json::to_string(&response).unwrap_or_default();
    let mut out = std::io::stdout().lock();
    if writeln!(out, "{ANSWER}{answer}")
        .and_then(|()| out.flush())
        .is_err()
    {
        return 1;
    }
    0
}

enum Failure {
    ChooseExe(String),
    Refused(String),
    Error(String),
    NotWritable(PathBuf),
}
impl Failure {
    fn message(&self) -> String {
        match self {
            Self::ChooseExe(text) | Self::Refused(text) | Self::Error(text) => text.clone(),
            Self::NotWritable(folder) => format!("cannot write {}", folder.display()),
        }
    }
}
type Outcome<T> = Result<T, Failure>;

struct Cached {
    folder: PathBuf,
    package: Package,
}
impl Package {
    fn with(self, folder: PathBuf) -> Cached {
        Cached {
            folder,
            package: self,
        }
    }
}

fn folder_of(exe: &Path) -> &Path {
    exe.parent().unwrap_or(exe)
}

/// The chosen .exe, or the game's .exe in its install: the one Unreal Engine
/// shipping build, else the one program in the install's top folder that is
/// no crash reporter, launcher or installer.
fn find_exe(exe: Option<PathBuf>, install: Option<&Path>) -> Outcome<PathBuf> {
    if let Some(exe) = exe {
        let program = exe
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"));
        return if let Some(what) = not_the_game(&exe) {
            Err(Failure::ChooseExe(format!(
                "{} is {what}, not the game: choose the game's own program",
                exe.display()
            )))
        } else if let Some(install) = install.filter(|install| !inside(&exe, install)) {
            Err(Failure::ChooseExe(format!(
                "{} is not in the game's folder {}: choose the game's own program",
                exe.display(),
                install.display()
            )))
        } else if program && exe.is_absolute() && exe.is_file() {
            Ok(exe)
        } else {
            Err(Failure::Error(format!(
                "{} is not a program on this PC",
                exe.display()
            )))
        };
    }
    let Some(install) = install else {
        return Err(Failure::ChooseExe(
            "Rubylight doesn't know where this game is installed".into(),
        ));
    };
    if !install.is_dir() {
        return Err(Failure::ChooseExe(format!(
            "{} is not a folder on this PC",
            install.display()
        )));
    }
    let mut shipping = Vec::new();
    let mut top = Vec::new();
    let walked = walk(install, |path, depth, folder| {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        if folder || !name.ends_with(".exe") {
            return None::<()>;
        }
        if name.ends_with("-shipping.exe") {
            shipping.push(path.to_path_buf());
        } else if depth == 1 && !helper_program(&name) && not_the_game(path).is_none() {
            top.push(path.to_path_buf());
        }
        None
    });
    if let Err(TooBig) = walked {
        return Err(Failure::ChooseExe(format!(
            "{} has too many files for Rubylight to tell which program is the game",
            install.display()
        )));
    }
    match (shipping.len(), top.len()) {
        (1, _) => Ok(shipping.remove(0)),
        (0, 1) => Ok(top.remove(0)),
        _ => Err(Failure::ChooseExe(format!(
            "Rubylight can't tell which program in {} is the game",
            install.display()
        ))),
    }
}

/// What `exe` is when it is not a game's own program: a store's or
/// library's launcher, a script host or interpreter, or a program of
/// Windows. The mod beside it would load into that program and what it
/// starts, never into the game, and in Program Files it would be there for
/// every account.
pub(super) fn not_the_game(exe: &Path) -> Option<&'static str> {
    let name = exe
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    let stem = name.strip_suffix(".exe").unwrap_or(&name);
    let launcher = [
        "steam",
        "upc",
        "ubisoftconnect",
        "battle.net",
        "eadesktop",
        "origin",
        "galaxyclient",
        "amazon games",
        "amazon games ui",
        "itch",
        "heroic",
        "xboxpcapp",
        "gamelaunchhelper",
    ]
    .contains(&stem)
        || stem.contains("launcher")
        || stem.starts_with("playnite.");
    let interpreter = [
        "cmd",
        "powershell",
        "powershell_ise",
        "pwsh",
        "wscript",
        "cscript",
        "mshta",
        "rundll32",
        "conhost",
        "explorer",
        "py",
        "pyw",
        "node",
        "java",
        "javaw",
        "ruby",
        "rubyw",
        "perl",
        "php",
        "bash",
        "sh",
        "wsl",
        "dotnet",
    ]
    .contains(&stem)
        || stem.starts_with("python");
    let windows = ["SystemRoot", "windir"]
        .into_iter()
        .filter_map(std::env::var_os)
        .filter(|root| !root.is_empty())
        .any(|root| inside(exe, Path::new(&root)));
    if launcher {
        Some("a launcher")
    } else if interpreter {
        Some("a script host or interpreter")
    } else if windows {
        Some("part of Windows")
    } else {
        None
    }
}

fn helper_program(name: &str) -> bool {
    [
        "crash",
        "report",
        "launcher",
        "unins",
        "setup",
        "install",
        "redist",
        "vcredist",
        "vc_redist",
        "dxsetup",
        "dotnet",
        "helper",
        "updater",
        "anticheat",
        "_be.exe",
        "beservice",
        "start_protected",
        "cefprocess",
        "webhelper",
        "touchup",
        "cleanup",
    ]
    .iter()
    .any(|part| name.contains(part))
}

/// [`walk`] saw more than [`ENTRIES`] and stopped.
struct TooBig;

/// Walk `root` breadth first, `depth` 1 for its own entries, without following
/// links, until `visit` returns something; [`TooBig`] when the walk is too big.
fn walk<T>(
    root: &Path,
    mut visit: impl FnMut(&Path, usize, bool) -> Option<T>,
) -> Result<Option<T>, TooBig> {
    let mut queue = VecDeque::from([(root.to_path_buf(), 1)]);
    let mut seen = 0;
    while let Some((folder, depth)) = queue.pop_front() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > ENTRIES {
                return Err(TooBig);
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if let Some(found) = visit(&path, depth, kind.is_dir()) {
                return Ok(Some(found));
            }
            if kind.is_dir() && !kind.is_symlink() && depth < DEPTH {
                queue.push_back((path, depth + 1));
            }
        }
    }
    Ok(None)
}

/// Whether `path` is `root` or inside it, comparing names as Windows does,
/// whatever their case.
pub(super) fn inside(path: &Path, root: &Path) -> bool {
    let mut parts = path.components();
    root.components().all(|part| {
        parts.next().is_some_and(|other| {
            part.as_os_str().to_string_lossy().to_lowercase()
                == other.as_os_str().to_string_lossy().to_lowercase()
        })
    })
}
fn same(a: &Path, b: &Path) -> bool {
    a.components().count() == b.components().count() && inside(a, b)
}

/// A folder that holds other programs or their data: a drive's root, a
/// library's `common` folder, or one of the `shared` folders
/// ([`shared_folders`]).
fn holds_others(folder: &Path, shared: &[PathBuf]) -> bool {
    let Some(name) = folder.file_name() else {
        return true;
    };
    let name = name.to_string_lossy();
    ["common", "Program Files", "Program Files (x86)"]
        .iter()
        .any(|other| name.eq_ignore_ascii_case(other))
        || shared.iter().any(|root| same(folder, root))
}

/// The folders known by place to hold other programs or their data: the
/// Program Files folders and the user's profile and `AppData` folders
/// (`%LOCALAPPDATA%\BattlEye` is no game's own).
pub(super) fn shared_folders() -> Vec<PathBuf> {
    [
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
        "USERPROFILE",
        "LOCALAPPDATA",
        "APPDATA",
    ]
    .into_iter()
    .filter_map(std::env::var_os)
    .filter(|root| !root.is_empty())
    .map(PathBuf::from)
    .collect()
}

/// [`refuse_anti_cheat_in`] with this account's [`shared_folders`].
fn refuse_anti_cheat(exe: &Path, install: Option<&Path>) -> Outcome<()> {
    refuse_anti_cheat_in(exe, install, &shared_folders())
}
/// [`refuse_anti_cheat_in`] for a test's own shared folders, and why it refused.
#[cfg(test)]
pub(super) fn anti_cheat_refusal(
    exe: &Path,
    install: Option<&Path>,
    shared: &[PathBuf],
) -> Result<(), String> {
    refuse_anti_cheat_in(exe, install, shared).map_err(|failure| failure.message())
}

/// Refuse a game with anti-cheat anywhere in its install or below its .exe,
/// or beside the three folders above the .exe (an Unreal game's
/// `EasyAntiCheat` sits three up): only each folder's own entries, and the
/// search stays in the install and stops at a folder that holds other
/// programs ([`holds_others`]). An install too big to search through is
/// refused too.
fn refuse_anti_cheat_in(exe: &Path, install: Option<&Path>, shared: &[PathBuf]) -> Outcome<()> {
    let found = |path: &Path| {
        let name = path.file_name()?.to_string_lossy().into_owned();
        neural::anti_cheat(&name).map(|which| (which, path.to_path_buf()))
    };
    let own = folder_of(exe);
    let mut roots = Vec::from_iter(install.filter(|install| !same(install, own)));
    roots.push(own);
    let mut hit = None;
    for root in roots {
        hit = walk(root, |path, _, _| found(path)).map_err(|TooBig| {
            Failure::Refused(format!(
                "{} has too many files for Rubylight to check it for anti-cheat",
                root.display()
            ))
        })?;
        if hit.is_some() {
            break;
        }
    }
    let within = install.filter(|install| inside(own, install));
    for above in own.ancestors().skip(1).take(3) {
        if hit.is_some()
            || holds_others(above, shared)
            || within.is_some_and(|install| !inside(above, install))
        {
            break;
        }
        hit = std::fs::read_dir(above)
            .into_iter()
            .flatten()
            .flatten()
            .find_map(|entry| found(&entry.path()));
    }
    match hit {
        Some((which, path)) => Err(Failure::Refused(format!(
            "the game uses {which} ({}), and an anti-cheat treats Neural Rendering's DLL as a cheat",
            path.display()
        ))),
        None => Ok(()),
    }
}

pub(super) fn sha256_file(path: &Path) -> std::io::Result<(String, u64)> {
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 1 << 20];
    let mut size = 0;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok((hex::encode(hash.finalize()), size));
        }
        size += read as u64;
        hash.update(&buffer[..read]);
    }
}

/// Which of `forms` the file at `path` is now, [`Now::Missing`] or
/// [`Now::Changed`] when it is none. `quick` checks a big file by its size.
fn matched<'a>(
    folder: &Path,
    path: &str,
    forms: &[&'a Placed],
    quick: bool,
) -> Result<&'a Placed, Now> {
    let file = folder.join(path);
    let meta = std::fs::symlink_metadata(&file).map_err(|_| Now::Missing)?;
    let sized: Vec<&Placed> = forms
        .iter()
        .copied()
        .filter(|placed| meta.is_file() && meta.len() == placed.size)
        .collect();
    match sized.first() {
        None => Err(Now::Changed),
        Some(placed) if quick && placed.size >= HASHED_AT_LAUNCH => Ok(placed),
        Some(_) => {
            let (hash, _) = sha256_file(&file).map_err(|_| Now::Changed)?;
            sized
                .into_iter()
                .find(|placed| placed.sha256 == hash)
                .ok_or(Now::Changed)
        }
    }
}

/// A file `manifest` names as it is now: [`Now::Same`] as placed, or during
/// an update, as the release before placed it.
fn state(folder: &Path, manifest: &Manifest, path: &str, quick: bool) -> Now {
    let forms: Vec<&Placed> = manifest
        .files
        .get(path)
        .into_iter()
        .chain(manifest.previous.get(path))
        .collect();
    match matched(folder, path, &forms, quick) {
        Ok(_) => Now::Same,
        Err(now) => now,
    }
}

/// Refuse a link on the way from the game's folder to `relative` in it, the
/// two included: a junction or symbolic link planted there would send a
/// copy or a delete elsewhere. A SYSTEM host also refuses a folder on the
/// way that anyone but administrators may change.
fn guard(folder: &Path, relative: &str, writer: Writer) -> Outcome<()> {
    let parts: Vec<&str> = relative
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let mut path = folder.to_path_buf();
    for depth in 0..=parts.len() {
        if depth > 0 {
            path.push(parts[depth - 1]);
        }
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                return Err(Failure::Error(format!(
                    "cannot read {}: {e}",
                    path.display()
                )));
            }
        };
        if meta.file_attributes() & LINK != 0 {
            return Err(Failure::Refused(format!(
                "{} is a link to elsewhere, and Rubylight changes nothing through one",
                path.display()
            )));
        }
        if writer == Writer::System && depth > 0 && meta.is_dir() {
            security(&path, false)
                .map_err(|e| {
                    Failure::Error(format!(
                        "cannot check who may change {}: {e:#}",
                        path.display()
                    ))
                })
                .and_then(|security| verdict(&path, &security).map_err(Failure::Refused))?;
        }
    }
    Ok(())
}

/// Who may change a folder, as [`verdict`] judges it.
#[derive(Debug)]
pub(super) struct Security {
    pub link: bool,
    pub owner: String,
    /// Those its permissions let change it, or for a drive's root, move or
    /// delete what it holds: a root lets users add folders, which moves nothing.
    pub writers: Vec<String>,
}

fn security(path: &Path, root: bool) -> anyhow::Result<Security> {
    use butterpollo_windows::process::{
        open_file_child_movers, open_file_owner, open_file_writers, open_unfollowed,
    };
    let file = open_unfollowed(path, true)?;
    Ok(Security {
        link: file.metadata()?.file_attributes() & LINK != 0,
        owner: open_file_owner(&file)?,
        writers: if root {
            open_file_child_movers(&file)
        } else {
            open_file_writers(&file)
        }?,
    })
}

/// Whether only administrators may change `path`.
pub(super) fn verdict(path: &Path, security: &Security) -> Result<(), String> {
    if security.link {
        return Err(format!("{} is a link to elsewhere", path.display()));
    }
    if !ADMINISTRATORS.contains(&security.owner.as_str()) {
        return Err(format!(
            "{} is owned by {}, not by SYSTEM or Administrators",
            path.display(),
            security.owner
        ));
    }
    match security
        .writers
        .iter()
        .find(|sid| !ADMINISTRATORS.contains(&sid.as_str()))
    {
        Some(other) => Err(format!("{} can be changed by {other}", path.display())),
        None => Ok(()),
    }
}

/// Whether administrators own `path`, and with `whole` alone may change it
/// ([`verdict`]); the cache's check ([`super::trusted_root`]).
pub(super) fn administrators_hold(path: &Path, whole: bool) -> Result<(), String> {
    let security = security(path, false)
        .map_err(|e| format!("cannot check who may change {}: {e:#}", path.display()))?;
    if whole {
        return verdict(path, &security);
    }
    verdict(
        path,
        &Security {
            writers: Vec::new(),
            ..security
        },
    )
}

/// A SYSTEM host's check before it writes `folder` for the user: it and
/// every folder above it, the drive's root included, are no links and
/// nobody but administrators may change them. Otherwise an ordinary account
/// could swap one for a link and send SYSTEM's copies anywhere.
pub(super) fn administrators_only(folder: &Path) -> Result<(), String> {
    for path in folder.ancestors().filter(|p| !p.as_os_str().is_empty()) {
        let security = security(path, path.parent().is_none())
            .map_err(|e| format!("cannot check who may change {}: {e:#}", path.display()))?;
        verdict(path, &security)?;
    }
    Ok(())
}

/// Whether this process may change the game's folder. The user's helper
/// tries, and answers [`Failure::NotWritable`] for a folder only
/// administrators may write; a SYSTEM host writes only such a folder, and
/// only when nobody else may change it ([`administrators_only`]).
fn writable(folder: &Path, writer: Writer) -> Outcome<()> {
    match writer {
        Writer::User => {
            guard(folder, PROBE, writer)?;
            let probe = folder.join(PROBE);
            let tried = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&probe)
                .and_then(|file| {
                    drop(file);
                    std::fs::remove_file(&probe)
                });
            match tried {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    Err(Failure::NotWritable(folder.to_path_buf()))
                }
                Err(e) => Err(Failure::Error(format!(
                    "cannot write {}: {e}",
                    folder.display()
                ))),
            }
        }
        Writer::System => administrators_only(folder).map_err(|problem| {
            Failure::Refused(format!(
                "you may not change the game's folder {}, and Rubylight changes it for you only when nobody but administrators may: {problem}",
                folder.display()
            ))
        }),
    }
}

/// The manifest in `folder`, None when there is none.
fn manifest(folder: &Path) -> Result<Option<Manifest>, String> {
    match std::fs::read_to_string(folder.join(MANIFEST)) {
        Ok(text) => Manifest::parse(&text)
            .map(Some)
            .map_err(|e| format!("{e} ({})", folder.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!(
            "cannot read {}: {e}",
            folder.join(MANIFEST).display()
        )),
    }
}

fn write_manifest(folder: &Path, manifest: &Manifest, writer: Writer) -> Outcome<()> {
    let path = folder.join(MANIFEST);
    let partial = partial(&path);
    guard(folder, MANIFEST, writer)?;
    guard(folder, &format!("{MANIFEST}{PARTIAL}"), writer)?;
    let text = serde_json::to_string_pretty(manifest).map_err(|e| Failure::Error(e.to_string()))?;
    std::fs::write(&partial, text)
        .and_then(|()| std::fs::rename(&partial, &path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&partial);
            Failure::Error(format!("cannot write {}: {e}", path.display()))
        })
}

/// What `path` is called until it is complete.
fn partial(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(PARTIAL);
    name.into()
}

/// Copy `source` to `target`'s partial file, checking it against the package
/// as it goes; a tampered cache never reaches the game.
fn copy_checked(
    source: &Path,
    target: &Path,
    placed: &Placed,
    done: &mut u64,
    progress: &dyn Fn(u64),
) -> Result<(), String> {
    let partial = partial(target);
    let _ = std::fs::remove_file(&partial);
    let result = (|| {
        let mut input = std::fs::File::open(source)?;
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)?;
        let mut hash = Sha256::new();
        let mut buffer = vec![0; 1 << 20];
        let mut size = 0;
        loop {
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            size += read as u64;
            if size > placed.size {
                break;
            }
            hash.update(&buffer[..read]);
            output.write_all(&buffer[..read])?;
            *done += read as u64;
            progress(*done);
        }
        output.sync_all()?;
        drop(output);
        if size != placed.size || hex::encode(hash.finalize()) != placed.sha256 {
            return Err(std::io::Error::other(format!(
                "the cached {} does not match its release",
                source.display()
            )));
        }
        Ok(())
    })();
    result.map_err(|e| {
        let _ = std::fs::remove_file(&partial);
        format!("cannot place {}: {e}", target.display())
    })
}

/// What has `name` beside the game's .exe.
fn occupant(folder: &Path, name: &str, manifest: Option<&Manifest>) -> Occupant {
    let path = folder.join(name);
    if !path.exists() {
        return Occupant::Free;
    }
    if let Some(manifest) = manifest
        && manifest.proxy == name
        && manifest.files.contains_key(name)
        && state(folder, manifest, name, false) == Now::Same
    {
        return Occupant::Ours;
    }
    match std::fs::read(&path) {
        Ok(bytes) if bytes.windows(10).any(|w| w == b"OptiScaler") => Occupant::OptiScaler,
        _ => Occupant::Foreign,
    }
}

fn place(
    exe: &Path,
    cached: &Cached,
    writer: Writer,
    progress: &dyn Fn(u64, u64),
) -> Outcome<Response> {
    let folder = folder_of(exe);
    let package = &cached.package;
    guard(folder, MANIFEST, writer)?;
    let old = manifest(folder).map_err(Failure::Error)?;
    let ours = |name: &str| {
        old.as_ref().is_some_and(|m| {
            m.files.contains_key(name) || m.folders.iter().any(|f| f == name) || (m.proxy == name)
        })
    };
    for entry in std::fs::read_dir(folder)
        .map_err(|e| Failure::Error(format!("cannot read {}: {e}", folder.display())))?
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !ours(&name)
            && let Some(other) = neural::other_mod(&name)
        {
            return Err(Failure::Refused(format!(
                "{other} is installed in the game's folder ({name}): remove it first"
            )));
        }
    }
    let proxy =
        neural::proxy(|name| occupant(folder, name, old.as_ref())).map_err(Failure::Refused)?;
    writable(folder, writer)?;
    // Each file of the package where it goes, the mod's DLL under the proxy
    // name. A file already as this release has it stays; one to copy keeps
    // the form the release before left, if any.
    let mut wanted = Vec::new();
    let mut plan = Vec::new();
    for (path, placed) in &package.files {
        if path == SUMS {
            continue;
        }
        let target = if path == PROXY_SOURCE {
            proxy.to_owned()
        } else {
            path.clone()
        };
        guard(folder, &target, writer)?;
        let forms: Vec<&Placed> = old
            .iter()
            .flat_map(|m| [m.files.get(&target), m.previous.get(&target)])
            .flatten()
            .collect();
        if forms.is_empty() && std::fs::symlink_metadata(folder.join(&target)).is_ok() {
            if neural::document(&target) {
                continue;
            }
            return Err(Failure::Refused(format!(
                "{target} is already in the game's folder and is not Rubylight's"
            )));
        }
        let before = match matched(folder, &target, &forms, false) {
            Ok(form) if form == placed => {
                wanted.push((target, placed.clone()));
                continue;
            }
            Ok(form) => Some(form.clone()),
            Err(Now::Changed) if target == SETTINGS => {
                // The user's settings from the mod's menu stay through an update.
                continue;
            }
            Err(Now::Changed) => {
                return Err(Failure::Error(format!(
                    "{target} changed since Rubylight placed it in {}: turn Neural Rendering off and on again",
                    folder.display()
                )));
            }
            Err(_) => None,
        };
        plan.push((path.clone(), target.clone(), placed.clone(), before));
        wanted.push((target, placed.clone()));
    }
    // Folders the files need that are not there yet, parents first.
    let mut folders = old.as_ref().map(|m| m.folders.clone()).unwrap_or_default();
    let mut make = Vec::new();
    for (_, target, _, _) in &plan {
        let parts: Vec<&str> = target.split('/').collect();
        for end in 1..parts.len() {
            let relative = parts[..end].join("/");
            let path = folder.join(&relative);
            if make.contains(&relative) {
                continue;
            }
            if !path.exists() {
                if !folders.contains(&relative) {
                    folders.push(relative.clone());
                }
                make.push(relative);
            } else if !path.is_dir() {
                return Err(Failure::Refused(format!(
                    "{relative} in the game's folder is not a folder"
                )));
            }
        }
    }
    let mut files = old.as_ref().map(|m| m.files.clone()).unwrap_or_default();
    let mut previous = old.as_ref().map(|m| m.previous.clone()).unwrap_or_default();
    for (target, placed) in &wanted {
        files.insert(target.clone(), placed.clone());
        previous.remove(target);
    }
    for (_, target, _, before) in &plan {
        if let Some(before) = before {
            previous.insert(target.clone(), before.clone());
        }
    }
    let updated = old.as_ref().is_some_and(|m| m.release != package.tag);
    let first = old.is_none();
    // The manifest names every file and folder before one is made, each
    // file it replaces in both forms; whatever a failure or a kill leaves
    // is the host's, and a removal finds it.
    let mut manifest = Manifest {
        by: PLACED_BY.into(),
        release: old.map_or_else(|| package.tag.clone(), |m| m.release),
        proxy: proxy.into(),
        files,
        previous,
        folders,
    };
    write_manifest(folder, &manifest, writer)?;
    let placing = (|| {
        for relative in &make {
            guard(folder, relative, writer)?;
            let path = folder.join(relative);
            std::fs::create_dir(&path)
                .map_err(|e| Failure::Error(format!("cannot create {}: {e}", path.display())))?;
        }
        // Each file whole and checked beside its place first, then all in
        // place at once, the mod's DLL last: the game loads the mod only
        // beside the files it needs.
        let total: u64 = plan.iter().map(|(_, _, placed, _)| placed.size).sum();
        let mut done = 0;
        for (source, target, placed, _) in &plan {
            guard(folder, target, writer)?;
            guard(folder, &format!("{target}{PARTIAL}"), writer)?;
            let source = cached.folder.join("files").join(source);
            copy_checked(&source, &folder.join(target), placed, &mut done, &|done| {
                progress(done, total)
            })
            .map_err(Failure::Error)?;
        }
        let (dll, others): (Vec<_>, Vec<_>) =
            plan.iter().partition(|(_, target, _, _)| target == proxy);
        for (_, target, _, _) in others.into_iter().chain(dll) {
            guard(folder, target, writer)?;
            let path = folder.join(target);
            std::fs::rename(partial(&path), &path)
                .map_err(|e| Failure::Error(format!("cannot place {}: {e}", path.display())))?;
        }
        // Files of the release before that this one no longer has.
        let stale: Vec<String> = manifest
            .files
            .keys()
            .filter(|path| {
                path.as_str() != SETTINGS && !wanted.iter().any(|(target, _)| target == *path)
            })
            .cloned()
            .collect();
        for path in stale {
            let forms: Vec<&Placed> = manifest
                .files
                .get(&path)
                .into_iter()
                .chain(manifest.previous.get(&path))
                .collect();
            let form = match matched(folder, &path, &forms, false) {
                Ok(form) => form.clone(),
                Err(Now::Missing) => {
                    manifest.files.remove(&path);
                    continue;
                }
                // Someone's since: it stays, for a removal to keep too.
                Err(_) => continue,
            };
            guard(folder, &path, writer)?;
            match std::fs::remove_file(folder.join(&path)) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    // Still the host's, as it is now: a removal takes it.
                    manifest.files.insert(path, form);
                }
                _ => {
                    manifest.files.remove(&path);
                }
            }
        }
        manifest.release = package.tag.clone();
        manifest.previous.clear();
        write_manifest(folder, &manifest, writer)
    })();
    if let Err(failure) = placing {
        for (_, target, _, _) in &plan {
            if guard(folder, &format!("{target}{PARTIAL}"), writer).is_ok() {
                let _ = std::fs::remove_file(partial(&folder.join(target)));
            }
        }
        // A first install cut short goes again, so the switch stays off with
        // nothing left behind. An update cut short leaves each file it
        // replaces in one of its two forms: the next launch finishes it, and
        // turning the switch off removes either.
        if first && let Err(left) = remove(folder, writer) {
            return Err(Failure::Error(format!(
                "{}, and what was placed could not be removed again: {}",
                failure.message(),
                left.message()
            )));
        }
        return Err(failure);
    }
    Ok(Response::Placed {
        exe: exe.to_path_buf(),
        release: package.tag.clone(),
        proxy: proxy.into(),
        updated,
    })
}

fn prepare(
    exe: &Path,
    install: Option<&Path>,
    package: Option<PathBuf>,
    writer: Writer,
    progress: &dyn Fn(u64, u64),
) -> Outcome<Response> {
    let folder = folder_of(exe);
    refuse_anti_cheat(exe, install)?;
    let Some(manifest) = manifest(folder).map_err(Failure::Error)? else {
        return Err(Failure::Error(format!(
            "Neural Rendering's files are gone from {}: turn it off and on again",
            folder.display()
        )));
    };
    // Another release, or the rest of an update cut short.
    if let Some(folder) = package {
        let cached = Package::read(&folder).map_err(Failure::Error)?.with(folder);
        if cached.package.tag != manifest.release || !manifest.previous.is_empty() {
            return place(exe, &cached, writer, progress);
        }
    }
    if !manifest.previous.is_empty() {
        return Err(Failure::Error(format!(
            "an update of Neural Rendering in {} was cut short: turn it off and on again",
            folder.display()
        )));
    }
    for path in manifest.files.keys() {
        let state = state(folder, &manifest, path, true);
        if state != Now::Same && !(path == SETTINGS && state == Now::Changed) {
            let how = if state == Now::Missing {
                "is missing"
            } else {
                "changed"
            };
            return Err(Failure::Error(format!(
                "Neural Rendering's {path} {how} in {}: turn it off and on again",
                folder.display()
            )));
        }
    }
    Ok(Response::Placed {
        exe: exe.to_path_buf(),
        release: manifest.release,
        proxy: manifest.proxy,
        updated: false,
    })
}

/// Delete what the manifest names and is still as placed, the mod's logs and
/// the folders it created once empty; a file someone changed stays.
fn remove(folder: &Path, writer: Writer) -> Outcome<Response> {
    // A folder that is gone with its game holds nothing; one on a drive that
    // is not connected may, and is not called removed.
    if !folder.is_dir() && folder.ancestors().last().is_none_or(|root| !root.is_dir()) {
        return Ok(Response::Unreachable {
            folder: folder.display().to_string(),
        });
    }
    guard(folder, MANIFEST, writer)?;
    let Some(manifest) = manifest(folder).map_err(Failure::Error)? else {
        return Ok(Response::Removed { kept: Vec::new() });
    };
    writable(folder, writer)?;
    // A link anywhere refuses the removal before anything goes.
    for path in manifest.files.keys().chain(&manifest.folders) {
        guard(folder, path, writer)?;
    }
    let removal = neural::removal(&manifest, |path| state(folder, &manifest, path, false));
    for path in &removal.delete {
        guard(folder, path, writer)?;
        let file = folder.join(path);
        std::fs::remove_file(&file)
            .map_err(|e| Failure::Error(format!("cannot remove {}: {e}", file.display())))?;
    }
    let mut leftovers: Vec<String> = manifest
        .files
        .keys()
        .map(|path| format!("{path}{PARTIAL}"))
        .collect();
    leftovers.extend(
        std::fs::read_dir(folder)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| neural::logs(name)),
    );
    leftovers.extend([format!("{MANIFEST}{PARTIAL}"), PROBE.into()]);
    for path in leftovers {
        guard(folder, &path, writer)?;
        let _ = std::fs::remove_file(folder.join(path));
    }
    // Deepest first; a folder that still holds someone's files stays.
    for relative in manifest.folders.iter().rev() {
        guard(folder, relative, writer)?;
        let _ = std::fs::remove_dir(folder.join(relative));
    }
    guard(folder, MANIFEST, writer)?;
    let path = folder.join(MANIFEST);
    std::fs::remove_file(&path)
        .map_err(|e| Failure::Error(format!("cannot remove {}: {e}", path.display())))?;
    Ok(Response::Removed { kept: removal.keep })
}
