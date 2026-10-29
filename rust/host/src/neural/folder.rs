//! Neural Rendering in one game's folder: find the game's .exe, refuse
//! anti-cheat and other upscaler mods, place a cached package under a free
//! proxy name, check it before a launch, and remove exactly what was placed.
//!
//! Game folders are the user's to write, so a SYSTEM host never touches them
//! itself: it runs `butterpollo.exe --neural-folder` with the signed-in
//! user's token ([`super::run_folder`]), and a junction planted in a game
//! folder gains nothing the user could not do anyway.
use butterpollo_core::neural::{
    self, MANIFEST, Manifest, Now, Occupant, PLACED_BY, PROXY_SOURCE, Placed, SETTINGS, SUMS,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    io::{BufRead, Read, Write},
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
/// Files at least this big are checked by size before a launch; hashing the
/// 760 MB model at every launch would delay the game.
const HASHED_AT_LAUNCH: u64 = 64 << 20;
/// How far the anti-cheat and .exe searches look into an install.
const DEPTH: usize = 6;
const ENTRIES: usize = 50_000;

/// A release in the host's cache: `files/` holds the archive's contents,
/// each checked against the release's SHA256SUMS.txt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    pub tag: String,
    pub asset: String,
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

#[derive(Debug, Serialize, Deserialize)]
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

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
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
}

pub fn handle(request: Request, progress: &dyn Fn(u64, u64)) -> Response {
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
            place(&exe, &package, progress)
        }),
        Request::Prepare {
            exe,
            install,
            package,
        } => prepare(&exe, install.as_deref(), package, progress),
        Request::Remove { exe } => remove(folder_of(&exe)).map_err(Failure::Error),
    };
    result.unwrap_or_else(|failure| match failure {
        Failure::ChooseExe(reason) => Response::ChooseExe { reason },
        Failure::Refused(reason) => Response::Refused { reason },
        Failure::Error(message) => Response::Error { message },
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
            Ok(request) => handle(request, &|done, total| {
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
        return if program && exe.is_absolute() && exe.is_file() {
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
    walk(install, |path, depth, folder| {
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
        } else if depth == 1 && !helper_program(&name) {
            top.push(path.to_path_buf());
        }
        None
    });
    match (shipping.len(), top.len()) {
        (1, _) => Ok(shipping.remove(0)),
        (0, 1) => Ok(top.remove(0)),
        _ => Err(Failure::ChooseExe(format!(
            "Rubylight can't tell which program in {} is the game",
            install.display()
        ))),
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

/// Walk `root` breadth first, `depth` 1 for its own entries, without following
/// links, until `visit` returns something or the walk is too big.
fn walk<T>(root: &Path, mut visit: impl FnMut(&Path, usize, bool) -> Option<T>) -> Option<T> {
    let mut queue = VecDeque::from([(root.to_path_buf(), 1)]);
    let mut seen = 0;
    while let Some((folder, depth)) = queue.pop_front() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > ENTRIES {
                return None;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if let Some(found) = visit(&path, depth, kind.is_dir()) {
                return Some(found);
            }
            if kind.is_dir() && !kind.is_symlink() && depth < DEPTH {
                queue.push_back((path, depth + 1));
            }
        }
    }
    None
}

/// Refuse a game with anti-cheat anywhere in its install, or beside the
/// three folders above its .exe (an Unreal game's `EasyAntiCheat` sits
/// three up), stopping at a library's `common` folder and a drive's root.
fn refuse_anti_cheat(exe: &Path, install: Option<&Path>) -> Outcome<()> {
    let found = |path: &Path| {
        let name = path.file_name()?.to_string_lossy().into_owned();
        neural::anti_cheat(&name).map(|which| (which, path.to_path_buf()))
    };
    let root = install
        .filter(|install| exe.starts_with(install))
        .unwrap_or(folder_of(exe));
    let mut hit = walk(root, |path, _, _| found(path));
    for above in folder_of(exe).ancestors().skip(1).take(3) {
        let library = above.parent().is_none()
            || above
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("common"));
        if hit.is_some() || library || install.is_some_and(|install| !above.starts_with(install)) {
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

fn sha256_file(path: &Path) -> std::io::Result<(String, u64)> {
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

/// A placed file as it is now. `quick` checks a big file by its size.
fn now(folder: &Path, path: &str, placed: &Placed, quick: bool) -> Now {
    let file = folder.join(path);
    match std::fs::symlink_metadata(&file) {
        Err(_) => Now::Missing,
        Ok(meta) if !meta.is_file() || meta.len() != placed.size => Now::Changed,
        Ok(_) if quick && placed.size >= HASHED_AT_LAUNCH => Now::Same,
        Ok(_) => match sha256_file(&file) {
            Ok((hash, _)) if hash == placed.sha256 => Now::Same,
            _ => Now::Changed,
        },
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

fn write_manifest(folder: &Path, manifest: &Manifest) -> Result<(), String> {
    let path = folder.join(MANIFEST);
    let partial = folder.join(format!("{MANIFEST}{PARTIAL}"));
    let text = serde_json::to_string_pretty(manifest).map_err(|e| e.to_string())?;
    std::fs::write(&partial, text)
        .and_then(|()| std::fs::rename(&partial, &path))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Copy `source` to `target` through a partial file, checking it against the
/// package as it goes; a tampered cache never reaches the game.
fn copy_checked(
    source: &Path,
    target: &Path,
    placed: &Placed,
    done: &mut u64,
    progress: &dyn Fn(u64),
) -> Result<(), String> {
    let partial = PathBuf::from(format!("{}{PARTIAL}", target.display()));
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
        std::fs::rename(&partial, target)
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
        && manifest
            .files
            .get(name)
            .is_some_and(|placed| now(folder, name, placed, false) == Now::Same)
    {
        return Occupant::Ours;
    }
    match std::fs::read(&path) {
        Ok(bytes) if bytes.windows(10).any(|w| w == b"OptiScaler") => Occupant::OptiScaler,
        _ => Occupant::Foreign,
    }
}

fn place(exe: &Path, cached: &Cached, progress: &dyn Fn(u64, u64)) -> Outcome<Response> {
    let folder = folder_of(exe);
    let package = &cached.package;
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
    // Each file of the package where it goes, the mod's DLL under the proxy name.
    let mut plan = Vec::new();
    let mut keep_settings = None;
    for (path, placed) in &package.files {
        if path == SUMS {
            continue;
        }
        let target = if path == PROXY_SOURCE {
            proxy.to_owned()
        } else {
            path.clone()
        };
        let before = old.as_ref().and_then(|m| m.files.get(&target));
        match before.map(|placed| now(folder, &target, placed, false)) {
            Some(Now::Changed) if target == SETTINGS => {
                // The user's settings from the mod's menu stay through an update.
                keep_settings = before.cloned();
                continue;
            }
            Some(Now::Changed) => {
                return Err(Failure::Error(format!(
                    "{target} changed since Rubylight placed it in {}: turn Neural Rendering off and on again",
                    folder.display()
                )));
            }
            Some(_) => {}
            None if std::fs::symlink_metadata(folder.join(&target)).is_ok() => {
                if neural::document(&target) {
                    continue;
                }
                return Err(Failure::Refused(format!(
                    "{target} is already in the game's folder and is not Rubylight's"
                )));
            }
            None => {}
        }
        plan.push((path.clone(), target, placed.clone()));
    }
    // Folders the files need that are not there yet, parents first.
    let mut folders = old.as_ref().map(|m| m.folders.clone()).unwrap_or_default();
    for (_, target, _) in &plan {
        let parts: Vec<&str> = target.split('/').collect();
        for end in 1..parts.len() {
            let relative = parts[..end].join("/");
            let path = folder.join(&relative);
            if !path.exists() {
                std::fs::create_dir(&path).map_err(|e| {
                    Failure::Error(format!("cannot create {}: {e}", path.display()))
                })?;
                folders.push(relative);
            } else if !path.is_dir() {
                return Err(Failure::Refused(format!(
                    "{relative} in the game's folder is not a folder"
                )));
            }
        }
    }
    let mut files = old.as_ref().map(|m| m.files.clone()).unwrap_or_default();
    for (_, target, placed) in &plan {
        files.insert(target.clone(), placed.clone());
    }
    if let Some(settings) = keep_settings {
        files.insert(SETTINGS.into(), settings);
    }
    // The manifest names every file before it is placed, so a copy cut short
    // is still removed.
    let mut manifest = Manifest {
        by: PLACED_BY.into(),
        release: old
            .as_ref()
            .map_or_else(|| package.tag.clone(), |m| m.release.clone()),
        proxy: proxy.into(),
        files,
        folders,
    };
    write_manifest(folder, &manifest).map_err(Failure::Error)?;
    let total: u64 = plan.iter().map(|(_, _, placed)| placed.size).sum();
    let mut done = 0;
    for (source, target, placed) in &plan {
        let source = cached.folder.join("files").join(source);
        let copied = copy_checked(&source, &folder.join(target), placed, &mut done, &|done| {
            progress(done, total)
        });
        if let Err(error) = copied {
            // A first install cut short goes again, so the switch stays off
            // with nothing left behind; an update stays for the launch to report.
            if old.is_none() {
                let _ = remove(folder);
            }
            return Err(Failure::Error(error));
        }
    }
    // Files of the release before that this one no longer has.
    let placed: Vec<&String> = plan.iter().map(|(_, target, _)| target).collect();
    let stale: Vec<String> = manifest
        .files
        .keys()
        .filter(|path| !placed.contains(path) && path.as_str() != SETTINGS)
        .cloned()
        .collect();
    for path in stale {
        if now(folder, &path, &manifest.files[&path], false) != Now::Changed {
            let _ = std::fs::remove_file(folder.join(&path));
            manifest.files.remove(&path);
        }
    }
    let updated = old.as_ref().is_some_and(|m| m.release != package.tag);
    manifest.release = package.tag.clone();
    write_manifest(folder, &manifest).map_err(Failure::Error)?;
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
    if let Some(folder) = package {
        let cached = Package::read(&folder).map_err(Failure::Error)?.with(folder);
        if cached.package.tag != manifest.release {
            return place(exe, &cached, progress);
        }
    }
    for (path, placed) in &manifest.files {
        let state = now(folder, path, placed, true);
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
fn remove(folder: &Path) -> Result<Response, String> {
    let Some(manifest) = manifest(folder)? else {
        return Ok(Response::Removed { kept: Vec::new() });
    };
    let removal = neural::removal(&manifest, |path| {
        now(folder, path, &manifest.files[path], false)
    });
    for path in &removal.delete {
        let file = folder.join(path);
        std::fs::remove_file(&file)
            .map_err(|e| format!("cannot remove {}: {e}", file.display()))?;
    }
    for path in manifest.files.keys() {
        let _ = std::fs::remove_file(PathBuf::from(format!(
            "{}{PARTIAL}",
            folder.join(path).display()
        )));
    }
    for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_file())
            && neural::logs(&entry.file_name().to_string_lossy())
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    // Deepest first; a folder that still holds someone's files stays.
    for relative in manifest.folders.iter().rev() {
        let _ = std::fs::remove_dir(folder.join(relative));
    }
    let _ = std::fs::remove_file(folder.join(format!("{MANIFEST}{PARTIAL}")));
    let path = folder.join(MANIFEST);
    std::fs::remove_file(&path).map_err(|e| format!("cannot remove {}: {e}", path.display()))?;
    Ok(Response::Removed { kept: removal.keep })
}
