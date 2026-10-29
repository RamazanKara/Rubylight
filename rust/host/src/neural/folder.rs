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
/// How far the anti-cheat and .exe searches look into an install, and how
/// much they see before they give up (less in tests, which fill a folder).
const DEPTH: usize = 6;
const ENTRIES: usize = if cfg!(test) { 1_000 } else { 50_000 };

/// A release in the host's cache: `files/` holds the package's contents and
/// the runtime from the release's component, each checked against the
/// release's SHA256SUMS.txt or the archive's own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    pub tag: String,
    pub asset: String,
    /// The runtime component the runtime came from, when not the package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
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
        Request::Remove { exe } => remove(folder_of(&exe)),
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
impl Failure {
    fn message(&self) -> String {
        match self {
            Self::ChooseExe(text) | Self::Refused(text) | Self::Error(text) => text.clone(),
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
        } else if depth == 1 && !helper_program(&name) {
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
fn inside(path: &Path, root: &Path) -> bool {
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

/// A folder that holds other programs: a drive's root, a library's `common`
/// folder or a Program Files folder.
fn holds_others(folder: &Path) -> bool {
    let Some(name) = folder.file_name() else {
        return true;
    };
    let name = name.to_string_lossy();
    ["common", "Program Files", "Program Files (x86)"]
        .iter()
        .any(|other| name.eq_ignore_ascii_case(other))
        || ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"]
            .into_iter()
            .filter_map(std::env::var_os)
            .any(|root| same(folder, Path::new(&root)))
}

/// Refuse a game with anti-cheat anywhere in its install or below its .exe,
/// or beside the three folders above the .exe (an Unreal game's
/// `EasyAntiCheat` sits three up). That last search stays in the install
/// and stops at a folder that holds other programs ([`holds_others`]). An
/// install too big to search through is refused too.
fn refuse_anti_cheat(exe: &Path, install: Option<&Path>) -> Outcome<()> {
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
            || holds_others(above)
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

fn write_manifest(folder: &Path, manifest: &Manifest) -> Outcome<()> {
    let path = folder.join(MANIFEST);
    let partial = partial(&path);
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
    write_manifest(folder, &manifest)?;
    let placing = (|| {
        for relative in &make {
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
            let source = cached.folder.join("files").join(source);
            copy_checked(&source, &folder.join(target), placed, &mut done, &|done| {
                progress(done, total)
            })
            .map_err(Failure::Error)?;
        }
        let (dll, others): (Vec<_>, Vec<_>) =
            plan.iter().partition(|(_, target, _, _)| target == proxy);
        for (_, target, _, _) in others.into_iter().chain(dll) {
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
        write_manifest(folder, &manifest)
    })();
    if let Err(failure) = placing {
        for (_, target, _, _) in &plan {
            let _ = std::fs::remove_file(partial(&folder.join(target)));
        }
        // A first install cut short goes again, so the switch stays off with
        // nothing left behind. An update cut short leaves each file it
        // replaces in one of its two forms: the next launch finishes it, and
        // turning the switch off removes either.
        if first && let Err(left) = remove(folder) {
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
            return place(exe, &cached, progress);
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
fn remove(folder: &Path) -> Outcome<Response> {
    // A folder that is gone with its game holds nothing; one on a drive that
    // is not connected may, and is not called removed.
    if !folder.is_dir() && folder.ancestors().last().is_none_or(|root| !root.is_dir()) {
        return Ok(Response::Unreachable {
            folder: folder.display().to_string(),
        });
    }
    let Some(manifest) = manifest(folder).map_err(Failure::Error)? else {
        return Ok(Response::Removed { kept: Vec::new() });
    };
    let removal = neural::removal(&manifest, |path| state(folder, &manifest, path, false));
    for path in &removal.delete {
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
    leftovers.push(format!("{MANIFEST}{PARTIAL}"));
    for path in leftovers {
        let _ = std::fs::remove_file(folder.join(path));
    }
    // Deepest first; a folder that still holds someone's files stays.
    for relative in manifest.folders.iter().rev() {
        let _ = std::fs::remove_dir(folder.join(relative));
    }
    let path = folder.join(MANIFEST);
    std::fs::remove_file(&path)
        .map_err(|e| Failure::Error(format!("cannot remove {}: {e}", path.display())))?;
    Ok(Response::Removed { kept: removal.keep })
}
