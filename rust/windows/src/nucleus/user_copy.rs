//! The signed-in user's own copy of Nucleus Co-op, which Rubylight's own
//! one-player handler runs with the user's standard rights.
//!
//! It is never a folder an elevated Nucleus runs from, and never shares one
//! with the host's copy: `<data>\nucleus-user\<SID>\<key>`, outside
//! `C:\Users` (Nucleus refuses to run from there) and removed with the
//! host's data. It is copied from the host's Nucleus folder (the release it
//! installs, where alone Rubylight's own handler runs), program files only,
//! and `<key>` changes with them.
//! SYSTEM makes `nucleus-user` (only SYSTEM and Administrators may open
//! it), `<SID>` (its user may list it, nothing more) and `<key>` (its user
//! may add to it and change what is in it, never its permissions, nor
//! rename or delete it), each with its permissions from the start, and
//! checks them when they exist, never following a link: one owned or
//! permitted otherwise is refused, never rebuilt. Each is held open shared
//! for reading only while SYSTEM works below it, so the user can't make it a
//! link meanwhile. SYSTEM also removes an emptied folder from `<SID>`, after
//! checking `<SID>` is no link, renames a copy with a file the user can't
//! change any more aside within `<SID>` (a link in its place moves itself,
//! #56), and opens the copy to start Nucleus from it
//! ([`hold`]): `<key>` and its `NucleusCoop.exe`, checked by handle, the
//! program by the path Windows opened it at, and both held so neither can
//! be swapped for a link until it runs.
//! Everything else below `<SID>` is done as that user: writing the copy,
//! checking it, emptying an older one, the handlers, its game list, the
//! profile and settings. What the user can change there runs with their
//! rights only.
use super::{files, install};
use anyhow::{Context, Result, bail, ensure};
use butterpollo_core::nucleus::generic::{self, Setup};
use butterpollo_core::nucleus::handler::Handler;
use butterpollo_core::nucleus::install::{self as pinned, Part};
use butterpollo_core::nucleus::profile;
use std::collections::BTreeSet;
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::OwnedHandle;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The folder holding every user's copy: SYSTEM and Administrators own and
/// change it, and nobody else may open it. A user reaches their own folder
/// below it with the bypass traverse checking every account has, never
/// learning whose others are there.
const ROOT: &str = "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";

/// The file a whole copy holds: the listing of the program it was copied
/// from ([`Listing::text`]), written last. Its name is the host's, so it is
/// no part of the listing itself.
const MARKER: &str = "Rubylight-copy.txt";

const REPARSE_POINT: u32 = 0x400;
const DIRECTORY: u32 = 0x10;

/// A user's folder (`<SID>`): SYSTEM and Administrators own and change it;
/// the user may list it and pass through it, nothing more. Adding to a
/// folder is writing its data, which is enough to make an empty one a link.
fn sid_sddl(sid: &str) -> String {
    format!("O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;;0x1200a9;;;{sid})")
}

/// A user's copy (`<key>`): SYSTEM and Administrators own and change it;
/// the user may list it and add to it, and change and delete what is in it,
/// never its permissions, its owner or the folder itself (deleting or
/// renaming it). Emptied, they could make it a link: [`hold`] keeps them
/// from that while Nucleus starts.
fn user_sddl(sid: &str) -> String {
    format!(
        "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;;0x1200af;;;{sid})(A;OICIIO;0x1301bf;;;{sid})"
    )
}

/// The signed-in user's token, which their copy is written and run as. An
/// error without one: the host never writes their copy as itself.
pub fn user() -> Result<OwnedHandle> {
    match crate::user_folder::user_token() {
        Ok(Some(token)) => Ok(token),
        Ok(None) => bail!(
            "Nucleus Co-op's copy for the signed-in user needs the host to run as the Rubylight Windows service"
        ),
        Err(error) => Err(anyhow::anyhow!("{error}"))
            .context("finding the signed-in user, whose own Nucleus Co-op copy runs"),
    }
}

/// The SID of the user of `token`.
pub fn sid(token: &OwnedHandle) -> Result<String> {
    crate::process::token_user_sid(token)
}

/// Runs `work` as the user of `token` (None: as the host, in tests).
pub fn as_user<T>(token: Option<&OwnedHandle>, work: impl FnOnce() -> T) -> Result<T> {
    let _user =
        crate::user_folder::Impersonation::begin(token).context("acting as the signed-in user")?;
    Ok(work())
}

/// What differs in tests: a test process can't give a folder to the
/// Administrators group.
struct Steps {
    create: fn(&Path, &str) -> std::io::Result<()>,
    check: fn(&Path, &[&str]) -> Result<std::fs::File>,
}
impl Steps {
    const PINNED: Self = Self {
        create: crate::process::create_folder,
        check: secured,
    };
}

/// The user `sid`'s own copy of the host's Nucleus folder `source` (read as
/// the host: only SYSTEM and Administrators may change it), made again when
/// it is missing or differs from `source`'s program files by name, size or
/// time, or holds a file Rubylight changes that the user can't ([`stuck`]),
/// with every other copy and leftover in their folder removed.
/// `token` is theirs.
pub fn ensure(data: &Path, source: &Path, sid: &str, token: &OwnedHandle) -> Result<PathBuf> {
    ensure_with(data, source, sid, Some(token), &Steps::PINNED)
}

fn ensure_with(
    data: &Path,
    source: &Path,
    sid: &str,
    token: Option<&OwnedHandle>,
    steps: &Steps,
) -> Result<PathBuf> {
    static MAKING: Mutex<()> = Mutex::new(());
    let _one = MAKING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let folder = pinned::user_folder(data, sid)
        .with_context(|| format!("{sid:?} is not a Windows account's SID"))?;
    let listing = Listing::of(source).with_context(|| {
        format!(
            "reading the Nucleus Co-op folder {} for the signed-in user's own copy",
            source.display()
        )
    })?;
    ensure!(
        listing.files.iter().any(|(name, _)| name
            .to_str()
            .is_some_and(|name| name.eq_ignore_ascii_case(pinned::EXECUTABLE))),
        "{} holds no {}",
        source.display(),
        pinned::EXECUTABLE
    );
    let root = pinned::user_root(data);
    // Each held open ([`open_held`]): none can be moved, replaced or made a
    // link while the host works below it.
    let _root = made(&root, ROOT, &[], steps)?;
    let mut held_folder = made(&folder, &sid_sddl(sid), &[], steps)?;
    let key = pinned::copy_key(&listing.text);
    let copy = folder.join(&key);
    let mut held = made(&copy, &user_sddl(sid), &[sid], steps)?;
    // One file the user can't change any more would refuse every launch
    // (#56): a copy that holds one, or can't be emptied to be made again,
    // goes aside, and a new one is made.
    let differs = as_user(token, || differs(&copy, &listing.text))?;
    match &differs {
        None => {}
        Some(Differs::Missing) => {
            tracing::info!(copy = %copy.display(), source = %source.display(), "Nucleus Co-op: copying it for the signed-in user");
        }
        Some(Differs::Changed(why)) => {
            tracing::warn!(copy = %copy.display(), source = %source.display(), why, "Nucleus Co-op: the signed-in user's copy is not the host's; copying it again");
        }
    }
    let stuck = match &differs {
        None => as_user(token, || stuck(&copy))?,
        Some(_) => as_user(token, || empty(&copy))?
            .err()
            .map(|why| format!("{why:#}")),
    };
    if let Some(why) = &stuck {
        tracing::warn!(copy = %copy.display(), why, "Nucleus Co-op: the signed-in user can't change a file in their copy any more; it is set aside and copied again");
        // A rename writes the folder it is in: both are let go for it, and
        // checked again. The user can't add to `<SID>`.
        drop((held, held_folder));
        set_aside(&folder, &key)?;
        held_folder = made(&folder, &sid_sddl(sid), &[], steps)?;
        held = made(&copy, &user_sddl(sid), &[sid], steps)?;
    }
    if stuck.is_some() || differs.is_some() {
        make(source, &listing, &copy, token)?;
    }
    as_user(token, || seed_settings(&copy))??;
    remove_others(&folder, &key, token)?;
    drop((held, held_folder));
    Ok(copy)
}

/// Why the user can't change what in their copy at `copy` Rubylight
/// changes or removes as them before Nucleus Co-op starts: the copy and its
/// `handlers`, `game profiles` and `content` folders, which it lists and
/// adds to, and the handlers, the profiles of its own handlers' games, the
/// game list and the settings, each with the file it is written through.
/// A read-only, hidden or system attribute on one of those files, which
/// Rubylight's writes and removals refuse, is cleared: the user may. Done
/// as the user, never through a link. None when they can change them all.
fn stuck(copy: &Path) -> Option<String> {
    let list = files::list(copy);
    // The folders Rubylight adds to, then the files it changes.
    let mut folders = vec![
        copy.to_path_buf(),
        copy.join("handlers"),
        copy.join("game profiles"),
    ];
    folders.extend(list.parent().map(Path::to_path_buf));
    let settings = copy.join("Settings.ini");
    let mut files = vec![
        list.with_extension("json.rubylight"),
        list,
        settings.with_extension("rubylight.tmp"),
        settings,
    ];
    let listed = |folder: &Path| -> Vec<(PathBuf, std::fs::FileType)> {
        std::fs::read_dir(folder)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| Some((entry.path(), entry.file_type().ok()?)))
            .collect()
    };
    for (path, kind) in listed(&copy.join("handlers")) {
        if kind.is_file()
            && path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("js"))
        {
            files.push(path);
        }
    }
    for (game, kind) in listed(&copy.join("game profiles")) {
        let ours = game
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.get(..generic::PREFIX.len()))
            .is_some_and(|start| start.eq_ignore_ascii_case(generic::PREFIX));
        if kind.is_dir() && ours {
            files.extend(
                listed(&game)
                    .into_iter()
                    .filter(|(_, kind)| kind.is_file())
                    .map(|(path, _)| path),
            );
            folders.push(game);
        }
    }
    let folders = folders.iter().map(|path| (path, true));
    let files = files.iter().map(|path| (path, false));
    folders
        .chain(files)
        .find_map(|(path, folder)| unchangeable(path, folder))
}

/// The read-only, hidden and system attributes: Windows refuses to write a
/// read-only file or delete it, and to replace a hidden or system one
/// (`CREATE_ALWAYS`).
const KEPT: u32 = 0x1 | 0x2 | 0x4;

/// Why the user can't list and add to the `folder` at `path`, or write and
/// delete the file there, if it is there. A file's [`KEPT`] attributes are
/// cleared first, through a handle that never follows a link.
fn unchangeable(path: &Path, folder: bool) -> Option<String> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::{
        DELETE, FILE_APPEND_DATA, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA,
    };
    let open = |access: u32, flags: u32| {
        std::fs::OpenOptions::new()
            .access_mode(access)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
            .custom_flags(flags | FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(path)
    };
    let refused = |error: std::io::Error| match error.kind() {
        std::io::ErrorKind::PermissionDenied => Some(format!("{}: {error}", path.display())),
        // Gone, or in use for now: neither stays.
        _ => None,
    };
    if folder {
        // A folder's FILE_ADD_FILE and FILE_ADD_SUBDIRECTORY are a file's
        // FILE_WRITE_DATA and FILE_APPEND_DATA.
        let access = FILE_LIST_DIRECTORY.0 | FILE_WRITE_DATA.0 | FILE_APPEND_DATA.0;
        return open(access, FILE_FLAG_BACKUP_SEMANTICS.0)
            .err()
            .and_then(refused);
    }
    let attributes = match open(FILE_READ_ATTRIBUTES.0 | FILE_WRITE_ATTRIBUTES.0, 0) {
        Ok(file) => file.metadata().map(|metadata| metadata.file_attributes()),
        Err(error) => return refused(error),
    };
    match attributes {
        Ok(attributes) if attributes & KEPT != 0 => {
            if let Err(error) = clear_kept(path, attributes) {
                return Some(format!(
                    "{} is read-only, hidden or a system file (attributes 0x{attributes:x}), which the user could not change: {error}",
                    path.display()
                ));
            }
            tracing::info!(file = %path.display(), attributes = format!("0x{attributes:x}"), "Nucleus Co-op: a read-only, hidden or system file in the signed-in user's copy made plain");
        }
        Ok(_) => {}
        Err(error) => return refused(error),
    }
    open(FILE_WRITE_DATA.0 | DELETE.0, 0)
        .err()
        .and_then(refused)
}

/// Clear the [`KEPT`] attributes of the file at `path`, which has
/// `attributes`, by a handle that never follows a link.
fn clear_kept(path: &Path, attributes: u32) -> std::io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_NORMAL, FILE_BASIC_INFO, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES, FileBasicInfo,
        SetFileInformationByHandle,
    };
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES.0 | FILE_WRITE_ATTRIBUTES.0)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)?;
    let left = attributes & !KEPT;
    // Zero times leave them as they are; no attributes at all is NORMAL.
    let info = FILE_BASIC_INFO {
        FileAttributes: if left == 0 {
            FILE_ATTRIBUTE_NORMAL.0
        } else {
            left
        },
        ..Default::default()
    };
    // SAFETY: the handle is open for as long as `file` is, and `info` is a
    // FILE_BASIC_INFO of its size.
    unsafe {
        SetFileInformationByHandle(
            HANDLE(file.as_raw_handle()),
            FileBasicInfo,
            std::ptr::from_ref(&info).cast(),
            std::mem::size_of::<FILE_BASIC_INFO>() as u32,
        )
    }
    .map_err(crate::user_folder::os_error)
}

/// Move the copy `key` in the user's folder `folder` aside, as the host:
/// the user may not rename it themselves. A link in its place moves
/// itself; `folder` was checked by the caller, and the user can neither add
/// to it nor move it, so no name in it leads elsewhere. [`remove_others`]
/// removes the copy like any other, and warns while a file in it can't be
/// removed.
fn set_aside(folder: &Path, key: &str) -> Result<()> {
    let from = folder.join(key);
    let to = (1..=64)
        .map(|n| folder.join(format!("{key}.aside-{n}")))
        .find(|to| std::fs::symlink_metadata(to).is_err())
        .with_context(|| {
            format!(
                "{} holds too many copies set aside: an administrator of this PC can delete them",
                folder.display()
            )
        })?;
    std::fs::rename(&from, &to).with_context(|| {
        format!(
            "setting {} aside, as the signed-in user can't change a file in it: an administrator of this PC can delete it, then start the stream again",
            from.display()
        )
    })?;
    tracing::info!(from = %from.display(), to = %to.display(), "Nucleus Co-op: the signed-in user's copy set aside");
    Ok(())
}

/// Give the copy at `copy` Nucleus Co-op's own settings when it has none
/// ([`profile::default_settings_ini`]): never the host's, which hold its
/// user's nicknames, Steam IDs and helper accounts' password. What the
/// user's Nucleus changed in them stays.
fn seed_settings(copy: &Path) -> Result<()> {
    use std::io::Write;
    let path = copy.join("Settings.ini");
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("creating {}", path.display())),
    };
    let written = file.write_all(&profile::default_settings_ini());
    if let Err(error) = written {
        drop(file);
        // Not left part-written: Nucleus refuses to start on a missing key.
        let _ = std::fs::remove_file(&path);
        return Err(error).with_context(|| format!("writing {}", path.display()));
    }
    Ok(())
}

/// The user's copy at `copy`, open while Nucleus Co-op starts from it:
/// SYSTEM opens the program and the working folder by their paths, so
/// neither may be a link. The folders above `<key>` are SYSTEM's and
/// Administrators', and the user can't rename or delete `<key>` itself.
/// Held, `<key>` is shared for reading only, so nobody can open it to make
/// it a link, rename or delete it; `NucleusCoop.exe` can't be written,
/// renamed or deleted. Both until this drops.
pub struct Held {
    _folder: std::fs::File,
    _program: std::fs::File,
}

/// [`Held`] for the user `sid`'s copy at `copy`: an error when `<key>` is a
/// link or not the host's ([`secured`]), or its `NucleusCoop.exe` is a
/// link, a folder, a hard link to another file or not in the folder held.
pub fn hold(copy: &Path, sid: &str) -> Result<Held> {
    hold_with(copy, sid, secured)
}

fn hold_with(
    copy: &Path,
    sid: &str,
    check: fn(&Path, &[&str]) -> Result<std::fs::File>,
) -> Result<Held> {
    let folder = check(copy, &[sid])?;
    let program = copy.join(pinned::EXECUTABLE);
    let file = open_program(&program)
        .and_then(|file| {
            in_folder(&folder, &file)?;
            Ok(file)
        })
        .with_context(|| {
            format!(
                "{} is not a program file Rubylight starts Nucleus Co-op from: start the stream again, which copies it again",
                program.display()
            )
        })?;
    Ok(Held {
        _folder: folder,
        _program: file,
    })
}

/// An error unless the file `program` has open is `NucleusCoop.exe` (in any
/// case) right in the folder `folder` has open, by the paths Windows
/// resolved both to.
fn in_folder(folder: &std::fs::File, program: &std::fs::File) -> Result<()> {
    let folder = crate::user_folder::final_path(folder).context("reading the copy's path")?;
    let found = crate::user_folder::final_path(program).context("reading the program's path")?;
    let found = PathBuf::from(found);
    let named = found.file_name().is_some_and(|name| {
        crate::user_folder::same_name(name, std::ffi::OsStr::new(pinned::EXECUTABLE))
    });
    ensure!(
        named && found.parent() == Some(Path::new(&folder)),
        "it opened as {}, not in {}",
        found.display(),
        Path::new(&folder).display()
    );
    Ok(())
}

/// The folder `path` opened to check and hold: a link is opened itself,
/// never followed, and it is shared for reading only. Nobody can open it to
/// write (which can make an empty folder a link), rename or delete it while
/// it is open; files and folders can still be added to it and removed.
fn open_held(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY,
        FILE_READ_ATTRIBUTES, FILE_SHARE_READ, READ_CONTROL,
    };
    std::fs::OpenOptions::new()
        .access_mode(FILE_LIST_DIRECTORY.0 | READ_CONTROL.0 | FILE_READ_ATTRIBUTES.0)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
        .open(path)
}

/// The file `path`, never through a link, open for reading only and shared
/// only for reading: it can't be written, renamed or deleted while it is
/// open. Refused when it is a link, a folder or one of several names of a
/// file.
fn open_program(path: &Path) -> Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_TYPE_DISK,
        GetFileInformationByHandle, GetFileType,
    };
    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    let handle = HANDLE(file.as_raw_handle());
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the handle is open for as long as `file` is, and `info` is
    // writable.
    unsafe { GetFileInformationByHandle(handle, &mut info) }
        .with_context(|| format!("reading {}", path.display()))?;
    // SAFETY: as above.
    let disk = unsafe { GetFileType(handle) } == FILE_TYPE_DISK;
    ensure!(
        info.dwFileAttributes & (REPARSE_POINT | DIRECTORY) == 0 && disk,
        "{} is a link or a folder",
        path.display()
    );
    ensure!(
        info.nNumberOfLinks == 1,
        "{} has {} names",
        path.display(),
        info.nNumberOfLinks
    );
    Ok(file)
}

/// What of a Nucleus Co-op folder its user's copy holds ([`pinned::part`]).
#[derive(Debug, Default)]
struct Listing {
    /// Program folders, below the folder.
    folders: Vec<PathBuf>,
    /// Program files and their (size, last write time).
    files: Vec<(PathBuf, (u64, u64))>,
    /// Files copied into a new copy only.
    seeded: Vec<PathBuf>,
    /// State folders, made empty in a new copy.
    state: Vec<PathBuf>,
    /// One sorted line per program folder (`name\`) and file
    /// (`name<TAB>size<TAB>time`): the copy's key and marker.
    text: String,
}

impl Listing {
    /// `folder`'s listing, read as whoever runs it. A link anywhere it
    /// looks is an error: no link is copied or followed. State folders are
    /// not looked into (Nucleus links game files in `content`).
    fn of(folder: &Path) -> Result<Self> {
        let mut listing = Self::default();
        let mut lines = Vec::new();
        let mut pending = vec![PathBuf::new()];
        while let Some(below) = pending.pop() {
            let path = folder.join(&below);
            let entries =
                std::fs::read_dir(&path).with_context(|| format!("listing {}", path.display()))?;
            for entry in entries {
                let entry = entry.with_context(|| format!("listing {}", path.display()))?;
                // From the listing itself: never follows a link.
                let metadata = entry
                    .metadata()
                    .with_context(|| format!("reading {}", entry.path().display()))?;
                let attributes = metadata.file_attributes();
                if attributes & REPARSE_POINT != 0 {
                    bail!(
                        "{} is a link, which Rubylight does not copy into a user's own Nucleus Co-op: remove it",
                        entry.path().display()
                    );
                }
                let name = below.join(entry.file_name());
                let Some(text) = name.to_str() else {
                    bail!("{} has a name that is not Unicode", entry.path().display());
                };
                let is_folder = attributes & DIRECTORY != 0;
                match (pinned::part(text, is_folder), is_folder) {
                    (Part::Program, true) => {
                        lines.push(format!("{text}\\"));
                        listing.folders.push(name.clone());
                        pending.push(name);
                    }
                    (Part::Program, false) => {
                        let stamp = (metadata.file_size(), metadata.last_write_time());
                        lines.push(format!("{text}\t{}\t{}", stamp.0, stamp.1));
                        listing.files.push((name, stamp));
                    }
                    (Part::Seeded, _) => listing.seeded.push(name),
                    (Part::State, true) => listing.state.push(name),
                    (Part::State, false) => {}
                }
            }
        }
        lines.sort();
        listing.text = lines.iter().map(|line| format!("{line}\n")).collect();
        Ok(listing)
    }
}

/// Why the copy at `copy` is not a whole copy of the program `text` lists.
enum Differs {
    Missing,
    Changed(String),
}

/// None when the copy at `copy` is a whole copy of the program `text`
/// lists: its marker says so and its program files are those, with the
/// same sizes and times, and its `NucleusCoop.exe` has no other name.
fn differs(copy: &Path, text: &str) -> Option<Differs> {
    // No more than it should hold: the copy is the user's to fill.
    let read = std::fs::File::open(copy.join(MARKER)).and_then(|file| {
        let mut marker = String::new();
        std::io::Read::read_to_string(
            &mut std::io::Read::take(file, text.len() as u64 + 1),
            &mut marker,
        )?;
        Ok(marker)
    });
    let marker = match read {
        Ok(marker) => marker,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Some(Differs::Missing);
        }
        Err(error) => return Some(Differs::Changed(format!("reading its {MARKER}: {error}"))),
    };
    if marker != text {
        return Some(Differs::Changed(format!(
            "its {MARKER} lists another program"
        )));
    }
    match Listing::of(copy) {
        Ok(found) if found.text == text => {}
        Ok(found) => return Some(Differs::Changed(difference(text, &found.text))),
        Err(error) => return Some(Differs::Changed(format!("{error:#}"))),
    }
    // [`hold`] refuses a program with another name: copied again, it has one.
    match names_of(&copy.join(pinned::EXECUTABLE)) {
        Ok(1) => None,
        Ok(names) => Some(Differs::Changed(format!(
            "its {} has {names} names",
            pinned::EXECUTABLE
        ))),
        Err(error) => Some(Differs::Changed(format!(
            "reading its {}: {error}",
            pinned::EXECUTABLE
        ))),
    }
}

/// How many names (hard links) the file at `path` has, never following a
/// link.
fn names_of(path: &Path) -> std::io::Result<u32> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle,
    };
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES.0)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the handle is open for as long as `file` is, and `info` is
    // writable.
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }
        .map_err(crate::user_folder::os_error)?;
    Ok(info.nNumberOfLinks)
}

/// The first line of two listings that differs, for the log.
fn difference(expected: &str, found: &str) -> String {
    let expected: BTreeSet<&str> = expected.lines().collect();
    let found: BTreeSet<&str> = found.lines().collect();
    let name = |line: &str| line.split('\t').next().unwrap_or_default().to_string();
    if let Some(line) = expected.difference(&found).next() {
        format!("{} is missing or changed", name(line))
    } else if let Some(line) = found.difference(&expected).next() {
        format!("{} is not the host's", name(line))
    } else {
        "its listing is in another order".into()
    }
}

/// Copy `source`'s `listing` into the copy's folder `copy`, emptied first:
/// written as the user of `token`, each file read as the host. Its marker
/// is written last, so a copy is whole or [`differs`] says so.
fn make(source: &Path, listing: &Listing, copy: &Path, token: Option<&OwnedHandle>) -> Result<()> {
    // What is there differs: it is not used.
    as_user(token, || empty(copy))??;
    let files = match fill(source, listing, copy, token) {
        Ok(files) => files,
        Err(error) => {
            if let Err(left) = as_user(token, || empty(copy)).and_then(|emptied| emptied) {
                tracing::warn!(copy = %copy.display(), error = format!("{left:#}"), "Nucleus Co-op: a partial copy is left; the next start empties it");
            }
            return Err(error.context("copying Nucleus Co-op for the signed-in user"));
        }
    };
    tracing::info!(copy = %copy.display(), files, "Nucleus Co-op: the signed-in user's own copy is made");
    Ok(())
}

/// Fill the empty folder `staging` with `listing`; the number of files.
fn fill(
    source: &Path,
    listing: &Listing,
    staging: &Path,
    token: Option<&OwnedHandle>,
) -> Result<usize> {
    as_user(token, || -> Result<()> {
        for folder in listing.folders.iter().chain(&listing.state) {
            let path = staging.join(folder);
            std::fs::create_dir_all(&path)
                .with_context(|| format!("creating {}", path.display()))?;
        }
        Ok(())
    })??;
    let program = listing
        .files
        .iter()
        .map(|(name, stamp)| (name, Some(*stamp)));
    let seeded = listing.seeded.iter().map(|name| (name, None));
    let mut files = 0;
    let mut buffer = vec![0; COPY_CHUNK];
    for (name, stamp) in program.chain(seeded) {
        copy_file(
            &source.join(name),
            &staging.join(name),
            stamp,
            token,
            &mut buffer,
        )?;
        files += 1;
    }
    as_user(token, || {
        std::fs::write(staging.join(MARKER), &listing.text)
    })?
    .with_context(|| format!("writing {}", staging.join(MARKER).display()))?;
    Ok(files)
}

/// How much of a file [`copy_file`] reads and writes at a time:
/// `std::io::copy` moves 8 KiB per call, which took about twice as long for
/// a folder of 127 MB in 464 files.
const COPY_CHUNK: usize = 1 << 20;

/// Copy the file `from`, opened as the host without following a link, to
/// the new file `to`, written as the user of `token` through `buffer`, with
/// `from`'s last write time. A program file must still have its listed
/// `stamp`.
fn copy_file(
    from: &Path,
    to: &Path,
    stamp: Option<(u64, u64)>,
    token: Option<&OwnedHandle>,
    buffer: &mut [u8],
) -> Result<()> {
    // Shares no writing while it is open.
    let mut source = crate::process::open_unfollowed(from, false)
        .with_context(|| format!("opening {}", from.display()))?;
    let metadata = source
        .metadata()
        .with_context(|| format!("reading {}", from.display()))?;
    ensure!(
        metadata.file_attributes() & (REPARSE_POINT | DIRECTORY) == 0,
        "{} is a link or a folder, not a file, so Rubylight does not copy it",
        from.display()
    );
    if let Some(stamp) = stamp {
        ensure!(
            (metadata.file_size(), metadata.last_write_time()) == stamp,
            "{} changed while Rubylight copied it: start the stream again",
            from.display()
        );
    }
    let modified = metadata
        .modified()
        .with_context(|| format!("reading {}", from.display()))?;
    as_user(token, || -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(to)
            .with_context(|| format!("creating {}", to.display()))?;
        copy_through(&mut source, &mut file, buffer)
            .with_context(|| format!("copying {}", from.display()))?;
        // Last: a write after it would move it again.
        file.set_modified(modified)
            .with_context(|| format!("writing {}", to.display()))
    })?
}

/// Copy what is left of `from` to `to`, `buffer` at a time.
fn copy_through(
    from: &mut impl std::io::Read,
    to: &mut impl std::io::Write,
    buffer: &mut [u8],
) -> std::io::Result<()> {
    loop {
        match from.read(buffer) {
            Ok(0) => return Ok(()),
            Ok(read) => to.write_all(&buffer[..read])?,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

/// The folder `path` with `sddl`'s permissions, made now or checked
/// ([`secured`]) when it is there, and open: it can't be renamed while the
/// returned handle is.
fn made(path: &Path, sddl: &str, user: &[&str], steps: &Steps) -> Result<std::fs::File> {
    match (steps.create)(path, sddl) {
        Ok(()) => {
            tracing::info!(folder = %path.display(), "Nucleus Co-op: folder for the users' own copies made")
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error).with_context(|| format!("creating {}", path.display())),
    }
    (steps.check)(path, user)
}

/// The folder at `path`, held open ([`open_held`]), when it is no link,
/// SYSTEM or Administrators own it, nobody but them may move, delete or
/// re-permission it, and nobody but them and `user` may change it.
/// Otherwise someone else made it: the host never writes through it.
fn secured(path: &Path, user: &[&str]) -> Result<std::fs::File> {
    let file = open_held(path).with_context(|| format!("opening {}", path.display()))?;
    let attributes = file
        .metadata()
        .with_context(|| format!("reading {}", path.display()))?
        .file_attributes();
    let owner = crate::process::open_file_owner(&file)
        .with_context(|| format!("reading the owner of {}", path.display()))?;
    let writers = crate::process::open_file_writers(&file)
        .with_context(|| format!("reading the permissions of {}", path.display()))?;
    let movers = crate::process::open_file_movers(&file)
        .with_context(|| format!("reading the permissions of {}", path.display()))?;
    verdict(path, attributes, &owner, (&writers, &movers), user)?;
    Ok(file)
}

/// Whether the folder at `path` may be used, from its `attributes`, its
/// `owner`, and who may change it and who may move it (`writers`,
/// `movers`): see [`secured`].
fn verdict(
    path: &Path,
    attributes: u32,
    owner: &str,
    (writers, movers): (&[String], &[String]),
    user: &[&str],
) -> Result<()> {
    let refused = |why: String| {
        anyhow::anyhow!(
            "{why}, so Rubylight doesn't use it for Nucleus Co-op: an administrator of this PC can delete {}, then start the stream again",
            path.display()
        )
    };
    if attributes & REPARSE_POINT != 0 {
        return Err(refused(format!(
            "{} is a link to elsewhere",
            path.display()
        )));
    }
    if !install::TRUSTED.contains(&owner) {
        return Err(refused(format!(
            "{} is owned by {owner}, not by SYSTEM or Administrators",
            path.display()
        )));
    }
    if let Some(other) = writers
        .iter()
        .find(|sid| !install::TRUSTED.contains(&sid.as_str()) && !user.contains(&sid.as_str()))
    {
        return Err(refused(format!(
            "{} can be changed by {other}",
            path.display()
        )));
    }
    if let Some(other) = movers
        .iter()
        .find(|sid| !install::TRUSTED.contains(&sid.as_str()))
    {
        return Err(refused(format!(
            "{} can be moved or deleted by {other}",
            path.display()
        )));
    }
    Ok(())
}

/// Remove what else is in the user's folder: a copy of another program
/// (`key` is this one's). What is in it goes as the user of `token`; the
/// folder itself then as the host, which may remove one it made (the user
/// may not). The user's folder is held by the caller and checked again to
/// be no link here; removing a link in it removes the link. One still in
/// use stays until the next time, with a warning.
fn remove_others(folder: &Path, key: &str, token: Option<&OwnedHandle>) -> Result<()> {
    let attributes = std::fs::symlink_metadata(folder)
        .with_context(|| format!("reading {}", folder.display()))?
        .file_attributes();
    ensure!(
        attributes & REPARSE_POINT == 0 && attributes & DIRECTORY != 0,
        "{} is a link to elsewhere, so Rubylight removes nothing in it: an administrator of this PC can delete it, then start the stream again",
        folder.display()
    );
    let entries = as_user(token, || -> Result<Vec<(PathBuf, bool)>> {
        let mut entries = Vec::new();
        for entry in
            std::fs::read_dir(folder).with_context(|| format!("listing {}", folder.display()))?
        {
            let entry = entry.with_context(|| format!("listing {}", folder.display()))?;
            if entry.file_name() != key {
                // A link is no folder here: it goes itself.
                entries.push((
                    entry.path(),
                    entry.file_type().is_ok_and(|kind| kind.is_dir()),
                ));
            }
        }
        Ok(entries)
    })??;
    for (path, is_folder) in entries {
        let removed = if is_folder {
            as_user(token, || empty(&path))
                .and_then(|emptied| emptied)
                .and_then(|()| {
                    std::fs::remove_dir(&path)
                        .with_context(|| format!("removing {}", path.display()))
                })
        } else {
            as_user(token, || remove_entry(&path)).and_then(|removed| removed)
        };
        match removed {
            Ok(()) => {
                tracing::info!(path = %path.display(), "Nucleus Co-op: another copy of the signed-in user's removed")
            }
            Err(error) => {
                tracing::warn!(path = %path.display(), error = format!("{error:#}"), "Nucleus Co-op: another copy of the signed-in user's was not fully removed; it is tried again next time, and an administrator of this PC can delete it")
            }
        }
    }
    Ok(())
}

/// Remove everything in `folder`, never following a link: a link itself
/// goes. What can't be removed stays, and the first such is the error:
/// the rest goes all the same (a copy set aside for one stuck file, #56).
fn empty(folder: &Path) -> Result<()> {
    let mut first = None;
    for entry in
        std::fs::read_dir(folder).with_context(|| format!("listing {}", folder.display()))?
    {
        let entry = entry.with_context(|| format!("listing {}", folder.display()))?;
        if let Err(error) = remove_entry(&entry.path()) {
            first.get_or_insert(error);
        }
    }
    first.map_or(Ok(()), Err)
}

/// Remove the file, folder or link at `path`, if there: a link itself, never
/// what it leads to.
fn remove_entry(path: &Path) -> Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let removed = if metadata.is_dir() {
        std::fs::remove_dir_all(path)
    } else if metadata.file_attributes() & DIRECTORY != 0 {
        // A link to a folder goes as a folder; a file's refusal is its own
        // error, not "The directory name is invalid" (#56).
        std::fs::remove_dir(path)
    } else {
        std::fs::remove_file(path)
    };
    match removed {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(error).with_context(|| format!("removing {}", path.display()))
        }
        _ => Ok(()),
    }
}

/// Write Rubylight's handler for `setup` into the host's Nucleus at
/// `folder` with its game in that one's list ([`files::write_own`]), then
/// give the user's copy at `copy` the host's handlers and add the game to
/// the copy's own list ([`files::list`], the one its Nucleus reads), as the
/// user of `token`. A copy's list Rubylight could not add to refuses the
/// launch before anything is written. The host's sweep removes the
/// handler with its game ([`files::remove_own`], then [`remove_game`]),
/// and the next mirror the copy's handler.
pub fn write_own(
    folder: &Path,
    copy: &Path,
    setup: &Setup,
    token: Option<&OwnedHandle>,
) -> Result<Handler> {
    as_user(token, || files::check_list(copy, Some(setup), &[]))??;
    let handler = files::write_own(folder, setup)?;
    let handlers = mirror_handlers(folder, copy, &handler.file, token)?;
    tracing::debug!(handlers, copy = %copy.display(), "Nucleus Co-op: the user's own copy has the host's handlers");
    as_user(token, || files::add_game(&files::list(copy), setup))??;
    Ok(handler)
}

/// Remove the game `guid` of a handler of Rubylight's the host's sweep
/// removed from the list of the user's copy at `copy`, as the user of
/// `token`.
pub fn remove_game(copy: &Path, guid: &str, token: Option<&OwnedHandle>) -> Result<()> {
    as_user(token, || files::remove_game(&files::list(copy), guid))?
}

/// Make the handlers of the user's copy at `copy` the host's hub handlers
/// at `folder` and Rubylight's own handler `own` for the game launched:
/// Nucleus drops from its list every game whose handler it doesn't have,
/// and a hub handler added in the host's Nucleus works in the copy too.
/// Rubylight's handlers for other games (another account's among them, with
/// their games' paths) stay out. The host's are read as the host from its
/// own folder and written as the user of `token`; a handler only the copy
/// had goes (Rubylight's own once the host's sweep removed it, or another
/// game was launched), so the copy runs only handlers the host's has.
/// Returns how many there are.
fn mirror_handlers(
    folder: &Path,
    copy: &Path,
    own: &str,
    token: Option<&OwnedHandle>,
) -> Result<usize> {
    let source = folder.join("handlers");
    let mut handlers = Vec::new();
    let entries = match std::fs::read_dir(&source) {
        Ok(entries) => entries.collect::<std::io::Result<Vec<_>>>()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error).with_context(|| format!("reading {}", source.display())),
    };
    for entry in entries {
        if !handler_file(&entry) {
            continue;
        }
        let bytes = std::fs::read(entry.path())
            .with_context(|| format!("reading {}", entry.path().display()))?;
        let launched = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.eq_ignore_ascii_case(own));
        if !launched && generic::owns(&String::from_utf8_lossy(&bytes)) {
            continue;
        }
        handlers.push((entry.file_name(), bytes));
    }
    let count = handlers.len();
    as_user(token, || write_handlers(&copy.join("handlers"), &handlers))??;
    Ok(count)
}

/// A handler file (`*.js`, not a link) in a handlers folder.
fn handler_file(entry: &std::fs::DirEntry) -> bool {
    entry.file_type().is_ok_and(|kind| kind.is_file())
        && Path::new(&entry.file_name())
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("js"))
}

fn write_handlers(target: &Path, handlers: &[(std::ffi::OsString, Vec<u8>)]) -> Result<()> {
    std::fs::create_dir_all(target).with_context(|| format!("creating {}", target.display()))?;
    for entry in
        std::fs::read_dir(target).with_context(|| format!("reading {}", target.display()))?
    {
        let entry = entry.with_context(|| format!("reading {}", target.display()))?;
        let name = entry.file_name();
        let js = Path::new(&name)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("js"));
        let kept = handlers
            .iter()
            .any(|(kept, _)| kept.eq_ignore_ascii_case(&name));
        if js && !kept && !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            std::fs::remove_file(entry.path())
                .with_context(|| format!("removing {}", entry.path().display()))?;
        }
    }
    for (name, bytes) in handlers {
        let path = target.join(name);
        // No more of the user's file than the host's handler holds.
        let mut old = Vec::new();
        let same = std::fs::File::open(&path)
            .and_then(|file| {
                std::io::Read::read_to_end(
                    &mut std::io::Read::take(file, bytes.len() as u64 + 1),
                    &mut old,
                )
            })
            .is_ok_and(|_| old == *bytes);
        if same {
            continue;
        }
        std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
