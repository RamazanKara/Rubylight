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
//! checking `<SID>` is no link, and opens the copy to start Nucleus from it
//! ([`hold`]): `<key>` and its `NucleusCoop.exe`, checked by handle, the
//! program by the path Windows opened it at, and both held so neither can
//! be swapped for a link until it runs.
//! Everything else below `<SID>` is done as that user: writing the copy,
//! checking it, emptying an older one, the handlers, its game list, the
//! profile and settings. What the user can change there runs with their
//! rights only.
use super::{files, install};
use anyhow::{Context, Result, bail, ensure};
use butterpollo_core::nucleus::generic::Setup;
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
/// time, with every other copy and leftover in their folder removed.
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
    let _folder = made(&folder, &sid_sddl(sid), &[], steps)?;
    let key = pinned::copy_key(&listing.text);
    let copy = folder.join(&key);
    let _copy = made(&copy, &user_sddl(sid), &[sid], steps)?;
    match as_user(token, || differs(&copy, &listing.text))? {
        None => {}
        Some(Differs::Missing) => {
            tracing::info!(copy = %copy.display(), source = %source.display(), "Nucleus Co-op: copying it for the signed-in user");
            make(source, &listing, &copy, token)?;
        }
        Some(Differs::Changed(why)) => {
            tracing::warn!(copy = %copy.display(), source = %source.display(), why, "Nucleus Co-op: the signed-in user's copy is not the host's; copying it again");
            make(source, &listing, &copy, token)?;
        }
    }
    as_user(token, || seed_settings(&copy))??;
    remove_others(&folder, &key, token)?;
    Ok(copy)
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
    for (name, stamp) in program.chain(seeded) {
        copy_file(&source.join(name), &staging.join(name), stamp, token)?;
        files += 1;
    }
    as_user(token, || {
        std::fs::write(staging.join(MARKER), &listing.text)
    })?
    .with_context(|| format!("writing {}", staging.join(MARKER).display()))?;
    Ok(files)
}

/// Copy the file `from`, opened as the host without following a link, to
/// the new file `to`, written as the user of `token`, with `from`'s last
/// write time. A program file must still have its listed `stamp`.
fn copy_file(
    from: &Path,
    to: &Path,
    stamp: Option<(u64, u64)>,
    token: Option<&OwnedHandle>,
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
        std::io::copy(&mut source, &mut file)
            .with_context(|| format!("copying {}", from.display()))?;
        // Last: a write after it would move it again.
        file.set_modified(modified)
            .with_context(|| format!("writing {}", to.display()))
    })?
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
    use std::os::windows::fs::MetadataExt;
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
    if attributes & 0x400 != 0 {
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
    use std::os::windows::fs::MetadataExt;
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
                tracing::warn!(path = %path.display(), error = format!("{error:#}"), "Nucleus Co-op: another copy of the signed-in user's was not removed; it is tried again next time")
            }
        }
    }
    Ok(())
}

/// Remove everything in `folder`, never following a link: a link itself
/// goes.
fn empty(folder: &Path) -> Result<()> {
    for entry in
        std::fs::read_dir(folder).with_context(|| format!("listing {}", folder.display()))?
    {
        let entry = entry.with_context(|| format!("listing {}", folder.display()))?;
        remove_entry(&entry.path())?;
    }
    Ok(())
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
    } else {
        std::fs::remove_file(path).or_else(|_| std::fs::remove_dir(path))
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
    let handlers = mirror_handlers(folder, copy, token)?;
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

/// Make the handlers of the user's copy at `copy` those of the host's copy
/// at `folder`: Nucleus drops from its list every game whose handler it
/// doesn't have, and a hub handler added in the host's Nucleus works in the
/// copy too. The host's are read as the host from its own folder and
/// written as the user of `token`; a handler only the copy had goes
/// (Rubylight's own once the host's sweep removed it), so the copy runs only
/// handlers the host's has. Returns how many there are.
fn mirror_handlers(folder: &Path, copy: &Path, token: Option<&OwnedHandle>) -> Result<usize> {
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
mod tests {
    use super::*;

    const SID: &str = "S-1-5-21-1-2-3-1001";

    fn create(path: &Path, _: &str) -> std::io::Result<()> {
        std::fs::create_dir(path)
    }
    /// [`secured`]'s open without its checks: a test process can't make
    /// Administrators the owner.
    fn open(path: &Path, _: &[&str]) -> Result<std::fs::File> {
        Ok(open_held(path)?)
    }
    const TEST: Steps = Steps {
        create,
        check: open,
    };

    /// The host's Nucleus settings, with its user's own values.
    const HOST_SETTINGS: &str = "[Misc]\r\nNucleusAccountPassword=hunter2\r\n[ControllerMapping]\r\nPlayer_1=Alice\r\n[SteamIDs]\r\nPlayer_1=76561190000000001\r\n";

    /// A host's Nucleus folder after it ran: its program and settings, and
    /// what it and the host wrote as they ran.
    fn host_folder() -> tempfile::TempDir {
        let folder = tempfile::tempdir().unwrap();
        for (name, data) in [
            ("NucleusCoop.exe", &b"MZ nucleus"[..]),
            ("Settings.ini", HOST_SETTINGS.as_bytes()),
            (r"gui\theme\theme.ini", b"theme"),
            (r"webview\darkreader.js", b"dark"),
            (r"handlers\Puzzle.js", b"Game.GUID = 'Puzzle';"),
            (r"content\Puzzle\Instance0\game.exe", b"MZ game"),
            (r"content\userprofile.json", b"{\"Games\":[\"host\"]}"),
            (r"game profiles\Puzzle\Profile[1].json", b"{}"),
            (r"webview\cache\data_0", b"cache"),
            ("debug-log.txt", b"log"),
            ("Rubylight-layers-S-1-5-21-1-2-3-1001.json", b"{}"),
            // Settings.ini as the host wrote it, left by a crash.
            ("Settings.rubylight.tmp", HOST_SETTINGS.as_bytes()),
        ] {
            let path = folder.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, data).unwrap();
        }
        std::fs::create_dir_all(folder.path().join(r"gui\covers")).unwrap();
        folder
    }

    fn names(folder: &Path) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    fn junction(target: &Path, link: &Path) {
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "mklink /J failed");
    }

    #[test]
    fn the_copy_is_the_hosts_program_without_what_it_wrote() {
        let data = tempfile::tempdir().unwrap();
        let host = host_folder();
        let source = host.path();
        let copy = ensure_with(data.path(), source, SID, None, &TEST).unwrap();
        let folder = pinned::user_folder(data.path(), SID).unwrap();
        let key = copy.file_name().unwrap().to_str().unwrap().to_string();
        assert_eq!(copy, pinned::user_copy(data.path(), SID, &key).unwrap());
        assert_eq!(key.len(), 16);
        // Not the host's copy, nor below it.
        assert!(!copy.starts_with(data.path().join("nucleus")));
        for name in [
            "NucleusCoop.exe",
            r"gui\theme\theme.ini",
            r"webview\darkreader.js",
        ] {
            assert_eq!(
                std::fs::read(copy.join(name)).unwrap(),
                std::fs::read(source.join(name)).unwrap(),
                "{name}"
            );
        }
        // With the host's times, which the next check compares.
        let modified = |path: &Path| std::fs::metadata(path).unwrap().modified().unwrap();
        assert_eq!(
            modified(&copy.join("NucleusCoop.exe")),
            modified(&source.join("NucleusCoop.exe"))
        );
        // What Nucleus and the host wrote as they ran is not copied; the
        // folders it is in are there, empty.
        for name in [
            "debug-log.txt",
            "Rubylight-layers-S-1-5-21-1-2-3-1001.json",
            "Settings.rubylight.tmp",
        ] {
            assert!(!copy.join(name).exists(), "{name}");
        }
        for name in [
            "content",
            "handlers",
            "game profiles",
            r"gui\covers",
            r"webview\cache",
        ] {
            assert!(names(&copy.join(name)).is_empty(), "{name}");
        }
        assert!(copy.join(MARKER).is_file());
        assert_eq!(names(&folder), [key.as_str()]);
        // Nucleus' own settings, with every key it ships and ours, and none
        // of the host's user's.
        let settings = std::fs::read_to_string(copy.join("Settings.ini")).unwrap();
        assert_eq!(settings.as_bytes(), profile::default_settings_ini());
        for line in profile::DEFAULT_SETTINGS.lines() {
            let key = line.split('=').next().unwrap();
            assert!(settings.contains(key), "{key}");
        }
        for private in ["hunter2", "Alice", "76561190000000001"] {
            assert!(!settings.contains(private), "{private}");
        }

        // What the host's Nucleus or the user's writes as they run is no
        // change: the same copy is kept, with what the user's wrote.
        std::fs::write(copy.join("content").join("Puzzle"), b"theirs").unwrap();
        // Its game list, where Nucleus 2.4.2 reads it, and the host's.
        let list = copy.join("content").join("userprofile.json");
        std::fs::write(&list, b"{\"Games\":[\"theirs\"]}").unwrap();
        std::fs::write(
            source.join(r"content\userprofile.json"),
            b"{\"Games\":[\"host\",\"hub\"]}",
        )
        .unwrap();
        std::fs::write(copy.join("Settings.ini"), b"[Dev]\r\ntheirs").unwrap();
        std::fs::write(source.join("debug-log.txt"), b"more").unwrap();
        std::fs::write(source.join("crashlog.txt"), b"crash").unwrap();
        std::fs::write(source.join("Settings.ini"), b"[Dev]\r\nhost").unwrap();
        std::fs::write(source.join(r"handlers\Rubylight-x.js"), b"//").unwrap();
        std::fs::create_dir_all(source.join(r"Temp\download")).unwrap();
        assert_eq!(
            ensure_with(data.path(), source, SID, None, &TEST).unwrap(),
            copy
        );
        assert!(copy.join("content").join("Puzzle").is_file());
        assert_eq!(std::fs::read(&list).unwrap(), b"{\"Games\":[\"theirs\"]}");
        assert!(
            !std::fs::read_to_string(copy.join(MARKER))
                .unwrap()
                .contains("userprofile")
        );
        assert_eq!(
            std::fs::read(copy.join("Settings.ini")).unwrap(),
            b"[Dev]\r\ntheirs"
        );
        // Gone, Nucleus' own are back.
        std::fs::remove_file(copy.join("Settings.ini")).unwrap();
        ensure_with(data.path(), source, SID, None, &TEST).unwrap();
        assert_eq!(
            std::fs::read(copy.join("Settings.ini")).unwrap(),
            profile::default_settings_ini()
        );
    }

    #[test]
    fn a_copy_that_differs_from_the_hosts_is_made_again() {
        let data = tempfile::tempdir().unwrap();
        let host = host_folder();
        let copy = ensure_with(data.path(), host.path(), SID, None, &TEST).unwrap();
        type Change = (&'static str, fn(&Path));
        let changes: [Change; 8] = [
            ("a file removed", |copy| {
                std::fs::remove_file(copy.join(r"gui\theme\theme.ini")).unwrap()
            }),
            ("a folder removed", |copy| {
                std::fs::remove_dir_all(copy.join("webview")).unwrap()
            }),
            // Its size is the same, its time not.
            ("a file changed", |copy| {
                std::fs::write(copy.join("NucleusCoop.exe"), b"MZ NUCLEUS").unwrap()
            }),
            ("a file shortened", |copy| {
                std::fs::write(copy.join("NucleusCoop.exe"), b"MZ").unwrap()
            }),
            ("a file added", |copy| {
                std::fs::write(copy.join("version.dll"), b"MZ").unwrap()
            }),
            ("its marker removed", |copy| {
                std::fs::remove_file(copy.join(MARKER)).unwrap()
            }),
            ("its marker changed", |copy| {
                std::fs::write(copy.join(MARKER), b"NucleusCoop.exe\t2\t1\n").unwrap()
            }),
            // Read no further than the listing it should be.
            ("its marker grown", |copy| {
                let mut marker = std::fs::read(copy.join(MARKER)).unwrap();
                marker.extend(vec![b'x'; 1 << 20]);
                std::fs::write(copy.join(MARKER), marker).unwrap()
            }),
        ];
        for (what, change) in changes {
            change(&copy);
            assert_eq!(
                ensure_with(data.path(), host.path(), SID, None, &TEST).unwrap(),
                copy,
                "{what}"
            );
            assert_eq!(
                std::fs::read(copy.join("NucleusCoop.exe")).unwrap(),
                b"MZ nucleus",
                "{what}"
            );
            assert!(copy.join(r"gui\theme\theme.ini").is_file(), "{what}");
            assert!(copy.join(r"webview\darkreader.js").is_file(), "{what}");
            assert!(!copy.join("version.dll").exists(), "{what}");
            assert_eq!(
                differs(&copy, &Listing::of(host.path()).unwrap().text).map(|_| ()),
                None,
                "{what}"
            );
        }

        // A link the user put in their copy goes with it, and what it
        // points to stays.
        let elsewhere = tempfile::tempdir().unwrap();
        std::fs::write(elsewhere.path().join("kept.txt"), b"x").unwrap();
        junction(elsewhere.path(), &copy.join(r"gui\linked"));
        ensure_with(data.path(), host.path(), SID, None, &TEST).unwrap();
        assert!(!copy.join(r"gui\linked").exists());
        assert_eq!(names(elsewhere.path()), ["kept.txt"]);
    }

    #[test]
    fn a_changed_program_makes_a_new_copy_and_drops_the_old() {
        let data = tempfile::tempdir().unwrap();
        let host = host_folder();
        let folder = pinned::user_folder(data.path(), SID).unwrap();
        // A copy named by its version (an earlier build's), a partial one.
        std::fs::create_dir_all(folder.join("2.4.2").join("handlers")).unwrap();
        std::fs::create_dir_all(folder.join("0123456789abcdef.partial")).unwrap();
        let first = ensure_with(data.path(), host.path(), SID, None, &TEST).unwrap();
        let name = |copy: &Path| copy.file_name().unwrap().to_str().unwrap().to_string();
        assert_eq!(names(&folder), [name(&first)]);
        // Nucleus updated (or nucleus_path pointed elsewhere).
        std::fs::write(host.path().join("NucleusCoop.exe"), b"MZ nucleus 2.5").unwrap();
        let second = ensure_with(data.path(), host.path(), SID, None, &TEST).unwrap();
        assert_ne!(first, second);
        assert_eq!(
            std::fs::read(second.join("NucleusCoop.exe")).unwrap(),
            b"MZ nucleus 2.5"
        );
        assert_eq!(names(&folder), [name(&second)]);
        // Nothing is removed through a link in the user's folder's place.
        let elsewhere = tempfile::tempdir().unwrap();
        std::fs::create_dir(elsewhere.path().join("other")).unwrap();
        let linked = data.path().join("linked");
        junction(elsewhere.path(), &linked);
        let error = remove_others(&linked, &name(&second), None).unwrap_err();
        assert!(
            format!("{error:#}").contains("is a link to elsewhere"),
            "{error:#}"
        );
        assert_eq!(names(elsewhere.path()), ["other"]);
    }

    #[test]
    fn a_link_in_the_hosts_folder_is_never_copied() {
        let data = tempfile::tempdir().unwrap();
        let host = host_folder();
        let elsewhere = tempfile::tempdir().unwrap();
        std::fs::write(elsewhere.path().join("secret.txt"), b"x").unwrap();
        // Nucleus links game files into its instances: never looked into.
        junction(
            elsewhere.path(),
            &host.path().join(r"content\Puzzle\Instance1"),
        );
        let copy = ensure_with(data.path(), host.path(), SID, None, &TEST).unwrap();
        assert!(names(&copy.join("content")).is_empty());
        // In its program: refused, and nothing is copied.
        std::fs::write(host.path().join("NucleusCoop.exe"), b"MZ nucleus 2.5").unwrap();
        junction(elsewhere.path(), &host.path().join(r"gui\linked"));
        let error = ensure_with(data.path(), host.path(), SID, None, &TEST).unwrap_err();
        let error = format!("{error:#}");
        assert!(error.contains(r"gui\linked is a link"), "{error}");
        let folder = pinned::user_folder(data.path(), SID).unwrap();
        assert_eq!(
            names(&folder),
            [copy.file_name().unwrap().to_str().unwrap()]
        );
        assert_eq!(names(elsewhere.path()), ["secret.txt"]);
    }

    #[test]
    fn the_hosts_folder_is_read_as_the_host_and_the_copy_written_as_the_user() {
        let sid = crate::process::own_sid().unwrap();
        let user = crate::user_folder::tests::without_own_sid();
        let root = tempfile::tempdir().unwrap();
        // A Nucleus folder only this account by name may read: the token
        // standing in for the user is refused it, the host is not.
        let host = root.path().join("host");
        crate::process::create_folder(&host, &format!("D:P(A;OICI;FA;;;{sid})")).unwrap();
        std::fs::write(host.join("NucleusCoop.exe"), b"MZ nucleus").unwrap();
        let open = root.path().join("open");
        crate::process::create_folder(&open, "D:P(A;OICI;FA;;;WD)").unwrap();
        let copy = ensure_with(&open, &host, SID, Some(&user), &TEST).unwrap();
        assert_eq!(
            std::fs::read(copy.join("NucleusCoop.exe")).unwrap(),
            b"MZ nucleus"
        );
        // A data folder only this account by name may change: the user's
        // copy is not written there.
        let named = root.path().join("named");
        crate::process::create_folder(&named, &format!("D:P(A;OICI;FA;;;{sid})")).unwrap();
        let error = ensure_with(&named, &host, SID, Some(&user), &TEST).unwrap_err();
        let error = format!("{error:#}");
        assert!(error.contains("(os error 5)"), "{error}");
        // Only the folders the host makes are there.
        let folder = pinned::user_folder(&named, SID).unwrap();
        for copy in names(&folder) {
            assert!(names(&folder.join(copy)).is_empty());
        }
    }

    #[test]
    fn a_folder_without_nucleus_or_a_bad_sid_makes_no_copy() {
        let data = tempfile::tempdir().unwrap();
        let host = host_folder();
        std::fs::remove_file(host.path().join("NucleusCoop.exe")).unwrap();
        let error = ensure_with(data.path(), host.path(), SID, None, &TEST).unwrap_err();
        assert!(
            error.to_string().ends_with("holds no NucleusCoop.exe"),
            "{error}"
        );
        assert!(!pinned::user_root(data.path()).exists());
        // Its name in another case is the same file to Windows.
        std::fs::write(host.path().join("nucleuscoop.EXE"), b"MZ nucleus").unwrap();
        let copy = ensure_with(data.path(), host.path(), SID, None, &TEST).unwrap();
        assert!(copy.join("nucleuscoop.EXE").is_file());

        let host = host_folder();
        let error =
            ensure_with(data.path(), host.path(), r"S-1-5-21-1\..\..", None, &TEST).unwrap_err();
        assert!(
            error.to_string().contains("is not a Windows account's SID"),
            "{error}"
        );
    }

    #[test]
    fn a_link_in_place_of_a_folder_is_refused() {
        let data = tempfile::tempdir().unwrap();
        let host = host_folder();
        let elsewhere = tempfile::tempdir().unwrap();
        junction(elsewhere.path(), &pinned::user_root(data.path()));
        let steps = Steps {
            create,
            check: secured,
        };
        let error = ensure_with(data.path(), host.path(), SID, None, &steps).unwrap_err();
        assert!(
            format!("{error:#}").contains("is a link to elsewhere"),
            "{error:#}"
        );
        assert!(
            std::fs::read_dir(elsewhere.path())
                .unwrap()
                .next()
                .is_none()
        );

        // The user's folder too: a junction there is never written through.
        std::fs::remove_dir(pinned::user_root(data.path())).unwrap();
        std::fs::create_dir(pinned::user_root(data.path())).unwrap();
        junction(
            elsewhere.path(),
            &pinned::user_folder(data.path(), SID).unwrap(),
        );
        let mut steps_root_ok = Steps {
            create,
            check: secured,
        };
        fn root_open_user_checked(path: &Path, user: &[&str]) -> Result<std::fs::File> {
            if path.ends_with(SID) {
                secured(path, user)
            } else {
                open(path, user)
            }
        }
        steps_root_ok.check = root_open_user_checked;
        let error = ensure_with(data.path(), host.path(), SID, None, &steps_root_ok).unwrap_err();
        assert!(
            format!("{error:#}").contains("is a link to elsewhere"),
            "{error:#}"
        );
        assert!(
            std::fs::read_dir(elsewhere.path())
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[test]
    fn only_the_hosts_and_the_users_own_folders_are_used() {
        let path = Path::new(r"C:\ProgramData\Butterpollo\nucleus-user");
        let trusted = ["S-1-5-18".to_string(), "S-1-5-32-544".to_string()];
        let none: [String; 0] = [];
        assert!(verdict(path, 0x10, "S-1-5-32-544", (&trusted, &trusted), &[]).is_ok());
        let user = [trusted[0].clone(), SID.to_string()];
        assert!(verdict(path, 0x10, "S-1-5-18", (&user, &trusted), &[SID]).is_ok());
        // Nor this user, who may change it, may move or delete it.
        let moved = verdict(path, 0x10, "S-1-5-18", (&user, &user), &[SID]).unwrap_err();
        assert!(
            moved
                .to_string()
                .contains(&format!("can be moved or deleted by {SID}")),
            "{moved}"
        );
        // Another user may not change it, nor this one where only the host may.
        let another = ["S-1-5-21-9".to_string()];
        let other = verdict(path, 0x10, "S-1-5-18", (&another, &none), &[SID]).unwrap_err();
        assert!(
            other.to_string().contains("can be changed by S-1-5-21-9"),
            "{other}"
        );
        assert!(
            other.to_string().ends_with(
                r"an administrator of this PC can delete C:\ProgramData\Butterpollo\nucleus-user, then start the stream again"
            ),
            "{other}"
        );
        let root = verdict(path, 0x10, "S-1-5-18", (&user, &none), &[]).unwrap_err();
        assert!(
            root.to_string()
                .contains(&format!("can be changed by {SID}")),
            "{root}"
        );
        // The user made it, or it's a link.
        let owned = verdict(path, 0x10, SID, (&none, &none), &[SID]).unwrap_err();
        assert!(
            owned.to_string().contains(&format!("is owned by {SID}")),
            "{owned}"
        );
        let link = verdict(path, 0x410, "S-1-5-18", (&none, &none), &[]).unwrap_err();
        assert!(
            link.to_string().contains("is a link to elsewhere"),
            "{link}"
        );
        let typed = ["ACE type 5".to_string()];
        let entry = verdict(path, 0x10, "S-1-5-18", (&typed, &none), &[SID]).unwrap_err();
        assert!(entry.to_string().contains("ACE type 5"), "{entry}");
    }

    #[test]
    fn the_folders_permissions_keep_the_user_to_what_is_inside() {
        // Owned by Administrators: the user can't change the permissions.
        assert!(ROOT.starts_with("O:BAD:P("));
        assert!(user_sddl(SID).starts_with("O:BAD:P("));
        // Only SYSTEM and Administrators may open the root: a user reaches
        // their own folder by traverse, and never lists whose are there.
        assert_eq!(ROOT, "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)");
        // The user's folder itself: list, add files and folders, read; what
        // is inside: modify (no permissions, owner or delete-child).
        let sddl = user_sddl(SID);
        assert!(sddl.contains(&format!("(A;;0x1200af;;;{SID})")), "{sddl}");
        assert!(
            sddl.contains(&format!("(A;OICIIO;0x1301bf;;;{SID})")),
            "{sddl}"
        );
        const WRITE_DAC: u32 = 0x4_0000;
        const WRITE_OWNER: u32 = 0x8_0000;
        const DELETE: u32 = 0x1_0000;
        const DELETE_CHILD: u32 = 0x40;
        assert_eq!(
            0x1200af & (WRITE_DAC | WRITE_OWNER | DELETE | DELETE_CHILD),
            0
        );
        assert_eq!(0x1301bf & (WRITE_DAC | WRITE_OWNER | DELETE_CHILD), 0);
        // Their own folder above the copy: list and pass through only. Adding
        // a file is writing its data, which could make it a link.
        const ADD: u32 = 0x2 | 0x4;
        const WRITE_EA: u32 = 0x10;
        const WRITE_ATTRIBUTES: u32 = 0x100;
        let theirs = sid_sddl(SID);
        assert_eq!(
            theirs,
            format!("O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;;0x1200a9;;;{SID})")
        );
        assert_eq!(
            0x1200a9
                & (ADD
                    | WRITE_EA
                    | WRITE_ATTRIBUTES
                    | WRITE_DAC
                    | WRITE_OWNER
                    | DELETE
                    | DELETE_CHILD),
            0
        );
        // Each makes a valid descriptor.
        for sddl in [ROOT.to_string(), sddl, theirs] {
            let folder = tempfile::tempdir().unwrap();
            let path = folder.path().join("x");
            match crate::process::create_folder(&path, &sddl) {
                Ok(()) => {
                    let file = crate::process::open_unfollowed(&path, true).unwrap();
                    assert_eq!(
                        crate::process::open_file_owner(&file).unwrap(),
                        "S-1-5-32-544"
                    );
                }
                // Not elevated: Administrators can't be made the owner.
                Err(error) => assert_eq!(error.raw_os_error(), Some(1307), "{error}"),
            }
        }
    }

    #[test]
    fn a_user_reaches_their_folder_through_a_root_they_cannot_open() {
        let sid = crate::process::own_sid().unwrap();
        // Standing in for the user: no rights by this account's name, and
        // only the privileges every account has (bypass traverse checking).
        let user = crate::user_folder::tests::without_own_sid();
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("nucleus-user");
        // As it is made, with this account in place of Administrators.
        crate::process::create_folder(&root, &ROOT.replace("BA", &sid)).unwrap();
        let theirs = root.join(SID);
        crate::process::create_folder(&theirs, "D:P(A;OICI;FA;;;WD)").unwrap();
        std::fs::write(theirs.join("NucleusCoop.exe"), b"MZ").unwrap();
        let (listed, reached) = as_user(Some(&user), || {
            (
                std::fs::read_dir(&root).map(|_| ()),
                std::fs::read(theirs.join("NucleusCoop.exe")),
            )
        })
        .unwrap();
        assert_eq!(
            listed.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(reached.unwrap(), b"MZ");
    }

    thread_local! {
        /// The folders [`RECORDED`] made, with their permissions.
        static MADE: std::cell::RefCell<Vec<(PathBuf, String)>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }
    fn create_recorded(path: &Path, sddl: &str) -> std::io::Result<()> {
        std::fs::create_dir(path)?;
        MADE.with_borrow_mut(|made| made.push((path.to_path_buf(), sddl.to_string())));
        Ok(())
    }
    /// As [`secured`] does, refuses a folder the host did not make.
    fn check_recorded(path: &Path, user: &[&str]) -> Result<std::fs::File> {
        ensure!(
            MADE.with_borrow(|made| made.iter().any(|(made, _)| made == path)),
            "{} is owned by {SID}",
            path.display()
        );
        open(path, user)
    }
    const RECORDED: Steps = Steps {
        create: create_recorded,
        check: check_recorded,
    };

    #[test]
    fn the_copys_folder_is_the_hosts_and_any_other_is_refused() {
        let data = tempfile::tempdir().unwrap();
        let host = host_folder();
        let copy = ensure_with(data.path(), host.path(), SID, None, &RECORDED).unwrap();
        // Made by the host: the user's folder they can only list, the copy
        // they fill and can't move.
        let folder = pinned::user_folder(data.path(), SID).unwrap();
        let made = MADE.with_borrow(|made| made.clone());
        assert!(made.contains(&(folder.clone(), sid_sddl(SID))), "{made:?}");
        assert!(made.contains(&(copy.clone(), user_sddl(SID))), "{made:?}");
        MADE.with_borrow_mut(|made| made.retain(|(path, _)| *path != copy));

        // One the host did not make is refused, and left as it is.
        std::fs::remove_dir_all(&copy).unwrap();
        std::fs::create_dir(&copy).unwrap();
        std::fs::write(copy.join("old.txt"), b"x").unwrap();
        let error = ensure_with(data.path(), host.path(), SID, None, &RECORDED).unwrap_err();
        assert!(format!("{error:#}").contains("is owned by"), "{error:#}");
        assert_eq!(names(&copy), ["old.txt"]);

        // So is a link in its place, and nothing is written through it.
        let elsewhere = tempfile::tempdir().unwrap();
        std::fs::remove_dir_all(&copy).unwrap();
        junction(elsewhere.path(), &copy);
        assert!(ensure_with(data.path(), host.path(), SID, None, &RECORDED).is_err());
        assert!(names(elsewhere.path()).is_empty());
    }

    /// Open `path` for writing its data, as a user setting a link on it
    /// does.
    fn open_to_write(path: &Path) -> std::io::Result<std::fs::File> {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
            FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_DATA,
        };
        std::fs::OpenOptions::new()
            .access_mode(FILE_WRITE_DATA.0)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
            .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
            .open(path)
    }

    #[test]
    fn the_user_never_makes_their_folder_or_a_held_copy_a_link() {
        use crate::user_folder::tests::mount;
        let sid = crate::process::own_sid().unwrap();
        // Standing in for the user: no rights by this account's name.
        let user = crate::user_folder::tests::without_own_sid();
        let root = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        // As the host makes them, with this account in place of
        // Administrators and Everyone in place of the user.
        let ours = |sddl: String| sddl.replace("BA", &sid);
        let theirs = root.path().join(SID);
        crate::process::create_folder(&theirs, &ours(sid_sddl("WD"))).unwrap();
        // An empty folder the user may add to, they can make a link: why
        // their own folder doesn't let them add to it.
        let open = root.path().join("open");
        crate::process::create_folder(&open, &ours(user_sddl("WD"))).unwrap();
        let (writable, linked) = as_user(Some(&user), || {
            (
                open_to_write(&open).map(drop),
                mount(&open, elsewhere.path()),
            )
        })
        .unwrap();
        writable.unwrap();
        linked.unwrap();
        assert!(
            std::fs::symlink_metadata(&open)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        // Their own folder: neither, nor adding to it.
        let (writable, linked, added) = as_user(Some(&user), || {
            (
                open_to_write(&theirs).map(drop),
                mount(&theirs, elsewhere.path()),
                std::fs::write(theirs.join("x"), b"x"),
            )
        })
        .unwrap();
        for refused in [writable, linked, added] {
            assert_eq!(
                refused.unwrap_err().kind(),
                std::io::ErrorKind::PermissionDenied
            );
        }
        assert!(std::fs::symlink_metadata(&theirs).unwrap().is_dir());
        // An empty copy, held as the host holds it: not while it is held.
        let copy = theirs.join("0123456789abcdef");
        crate::process::create_folder(&copy, &ours(user_sddl("WD"))).unwrap();
        let held = open_held(&copy).unwrap();
        let (writable, linked) = as_user(Some(&user), || {
            (
                open_to_write(&copy).map(drop),
                mount(&copy, elsewhere.path()),
            )
        })
        .unwrap();
        for refused in [writable, linked] {
            // A sharing violation.
            assert_eq!(refused.unwrap_err().raw_os_error(), Some(32));
        }
        // Files can still be added and removed meanwhile.
        let filled = as_user(Some(&user), || {
            std::fs::write(copy.join("Settings.ini"), b"x")
                .and_then(|()| std::fs::remove_file(copy.join("Settings.ini")))
        })
        .unwrap();
        filled.unwrap();
        drop(held);
        let linked = as_user(Some(&user), || mount(&copy, elsewhere.path())).unwrap();
        linked.unwrap();
        assert!(names(elsewhere.path()).is_empty());
    }

    #[test]
    fn the_user_changes_what_is_in_their_copy_but_never_moves_it() {
        let sid = crate::process::own_sid().unwrap();
        // Standing in for the user: no rights by this account's name.
        let user = crate::user_folder::tests::without_own_sid();
        let root = tempfile::tempdir().unwrap();
        // As the host makes both, with this account in place of
        // Administrators and Everyone in place of the user.
        let theirs = root.path().join(SID);
        crate::process::create_folder(&theirs, &sid_sddl("WD").replace("BA", &sid)).unwrap();
        let copy = theirs.join("0123456789abcdef");
        crate::process::create_folder(&copy, &user_sddl("WD").replace("BA", &sid)).unwrap();
        let (filled, emptied, renamed, removed) = as_user(Some(&user), || {
            let filled = (|| -> std::io::Result<()> {
                std::fs::write(copy.join("NucleusCoop.exe"), b"MZ")?;
                std::fs::create_dir(copy.join("gui"))?;
                std::fs::write(copy.join("gui").join("theme.ini"), b"x")
            })();
            (
                filled,
                empty(&copy),
                std::fs::rename(&copy, theirs.join("moved")),
                std::fs::remove_dir(&copy),
            )
        })
        .unwrap();
        filled.unwrap();
        emptied.unwrap();
        assert!(names(&copy).is_empty());
        for refused in [renamed, removed] {
            assert_eq!(
                refused.unwrap_err().kind(),
                std::io::ErrorKind::PermissionDenied
            );
        }
        // The host removes an emptied one.
        std::fs::remove_dir(&copy).unwrap();
    }

    /// A folder `key` in `root` holding a program as NucleusCoop.exe.
    fn program_copy(root: &Path) -> (PathBuf, PathBuf) {
        let key = root.join("0123456789abcdef");
        std::fs::create_dir(&key).unwrap();
        let program = key.join(pinned::EXECUTABLE);
        let system = std::env::var("SystemRoot").unwrap();
        std::fs::copy(Path::new(&system).join(r"System32\where.exe"), &program).unwrap();
        (key, program)
    }

    #[test]
    fn the_copy_and_its_program_are_held_while_nucleus_starts() {
        let root = tempfile::tempdir().unwrap();
        let (key, program) = program_copy(root.path());
        let moved = root.path().join("moved");
        let held = hold_with(&key, SID, open).unwrap();
        // Nobody can open the folder to write (to make it a link).
        assert_eq!(open_to_write(&key).unwrap_err().raw_os_error(), Some(32));
        // Nobody can move or delete either, nor write the program.
        assert!(std::fs::rename(&key, &moved).is_err());
        assert!(std::fs::remove_file(&program).is_err());
        assert!(std::fs::rename(&program, key.join("other.exe")).is_err());
        assert!(
            std::fs::OpenOptions::new()
                .write(true)
                .open(&program)
                .is_err()
        );
        // Windows starts it all the same, in that folder.
        let status = std::process::Command::new(&program)
            .arg("/?")
            .current_dir(&key)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "{status}");
        drop(held);
        std::fs::rename(&key, &moved).unwrap();
    }

    #[test]
    fn a_program_given_another_name_is_copied_again_and_then_starts() {
        let data = tempfile::tempdir().unwrap();
        let host = host_folder();
        let copy = ensure_with(data.path(), host.path(), SID, None, &TEST).unwrap();
        // A hard link the user (or a dedupe tool) made: refused to start.
        let elsewhere = tempfile::tempdir().unwrap();
        let other = elsewhere.path().join("other.exe");
        std::fs::hard_link(copy.join(pinned::EXECUTABLE), &other).unwrap();
        let error = format!("{:#}", hold_with(&copy, SID, open).err().unwrap());
        assert!(error.contains("has 2 names"), "{error}");
        // The next stream copies it again, with one name.
        assert_eq!(
            ensure_with(data.path(), host.path(), SID, None, &TEST).unwrap(),
            copy
        );
        assert_eq!(names_of(&copy.join(pinned::EXECUTABLE)).unwrap(), 1);
        hold_with(&copy, SID, open).unwrap();
        // The other name is left as it was.
        assert_eq!(std::fs::read(&other).unwrap(), b"MZ nucleus");
    }

    #[test]
    fn a_link_in_place_of_the_copy_or_its_program_is_never_started() {
        let root = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let (real, program) = program_copy(elsewhere.path());
        // The copy's folder a junction to another.
        let key = root.path().join("0123456789abcdef");
        junction(&real, &key);
        let error = format!("{:#}", hold_with(&key, SID, secured).err().unwrap());
        assert!(error.contains("is a link to elsewhere"), "{error}");
        std::fs::remove_dir(&key).unwrap();
        // Its program a link (a file link needs a privilege or developer
        // mode), a junction, a folder or a hard link.
        std::fs::create_dir(&key).unwrap();
        let link = key.join(pinned::EXECUTABLE);
        if std::os::windows::fs::symlink_file(&program, &link).is_ok() {
            let error = format!("{:#}", hold_with(&key, SID, open).err().unwrap());
            assert!(error.contains("is a link or a folder"), "{error}");
            std::fs::remove_file(&link).unwrap();
        }
        junction(&real, &link);
        let error = format!("{:#}", hold_with(&key, SID, open).err().unwrap());
        assert!(error.contains("is not a program file"), "{error}");
        std::fs::remove_dir(&link).unwrap();
        std::fs::create_dir(&link).unwrap();
        assert!(hold_with(&key, SID, open).is_err());
        std::fs::remove_dir(&link).unwrap();
        std::fs::hard_link(&program, &link).unwrap();
        let error = format!("{:#}", hold_with(&key, SID, open).err().unwrap());
        assert!(error.contains("has 2 names"), "{error}");
        std::fs::remove_file(&link).unwrap();
        std::fs::copy(&program, &link).unwrap();
        hold_with(&key, SID, open).unwrap();
        // A program that opened elsewhere than in the folder held.
        let held = open_held(&key).unwrap();
        let error = in_folder(&held, &open_program(&program).unwrap()).unwrap_err();
        assert!(format!("{error:#}").contains("not in"), "{error:#}");
        in_folder(&held, &open_program(&link).unwrap()).unwrap();
    }

    #[test]
    fn the_copys_handlers_are_the_hosts() {
        let host = tempfile::tempdir().unwrap();
        let copy = tempfile::tempdir().unwrap();
        let handlers = host.path().join("handlers");
        std::fs::create_dir_all(handlers.join("Puzzle")).unwrap();
        std::fs::write(handlers.join("Puzzle.js"), b"Game.GUID = 'Puzzle';").unwrap();
        std::fs::write(handlers.join("Rubylight-game.js"), b"// ours").unwrap();
        std::fs::write(handlers.join("Puzzle").join("x.dll"), b"x").unwrap();
        std::fs::write(handlers.join("notes.txt"), b"x").unwrap();
        let theirs = copy.path().join("handlers");
        std::fs::create_dir_all(&theirs).unwrap();
        // Gone from the host's: removed. Changed: replaced.
        std::fs::write(theirs.join("Removed.JS"), b"old").unwrap();
        std::fs::write(theirs.join("Puzzle.js"), b"old").unwrap();
        std::fs::write(theirs.join("readme.txt"), b"kept").unwrap();
        assert_eq!(mirror_handlers(host.path(), copy.path(), None).unwrap(), 2);
        let mut names: Vec<_> = std::fs::read_dir(&theirs)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["Puzzle.js", "Rubylight-game.js", "readme.txt"]);
        assert_eq!(
            std::fs::read(theirs.join("Puzzle.js")).unwrap(),
            b"Game.GUID = 'Puzzle';"
        );
        // A host's copy without handlers leaves the copy none.
        std::fs::remove_dir_all(&handlers).unwrap();
        assert_eq!(mirror_handlers(host.path(), copy.path(), None).unwrap(), 0);
        assert!(!theirs.join("Puzzle.js").exists());
    }

    #[test]
    fn ours_is_in_the_hosts_nucleus_and_leaves_the_copy_after_the_hosts_sweep() {
        let host = tempfile::tempdir().unwrap();
        let copy = tempfile::tempdir().unwrap();
        let puzzle = Setup::new(r"D:\Puzzle\Puzzle.exe", "Puzzle", None);
        let ice = Setup::new(r"D:\Ice\Ice.exe", "Ice", None);
        let first = write_own(host.path(), copy.path(), &puzzle, None).unwrap();
        // The host's Nucleus has it too, so its sweep knows the game.
        let hosts = files::catalog(host.path(), true, None).unwrap();
        assert!(hosts.handlers.iter().any(|h| h.own && h.guid == first.guid));
        assert!(hosts.games.iter().any(|game| game.guid == first.guid));
        // Another game's launch leaves the first game's in the copy.
        write_own(host.path(), copy.path(), &ice, None).unwrap();
        let theirs = copy.path().join("handlers");
        let mut both = [puzzle.file(), ice.file()];
        both.sort();
        assert_eq!(names(&theirs), both);
        // The host's sweep removes it and its game; the next launch, the
        // copy's.
        files::remove_own(host.path(), &first).unwrap();
        remove_game(copy.path(), &first.guid, None).unwrap();
        write_own(host.path(), copy.path(), &ice, None).unwrap();
        assert_eq!(names(&theirs), [ice.file()]);
        let copys = files::catalog(copy.path(), true, None).unwrap();
        assert!(copys.games.iter().all(|game| game.guid != first.guid));
    }

    #[test]
    fn the_copys_nucleus_lists_our_game_from_its_own_list() {
        let host = tempfile::tempdir().unwrap();
        let copy = tempfile::tempdir().unwrap();
        // A game whose .exe is there, as Nucleus 2.4.2 lists only those.
        let game = tempfile::tempdir().unwrap();
        let exe = game.path().join("Ice.exe");
        std::fs::write(&exe, b"MZ").unwrap();
        let ice = Setup::new(exe.to_str().unwrap(), "Ice", None);
        let own = write_own(host.path(), copy.path(), &ice, None).unwrap();
        // The copy's Nucleus reads its own content\userprofile.json, never
        // the host's or %APPDATA%'s: the game is there with its handler.
        assert_eq!(
            files::listed_by_nucleus(copy.path()).unwrap(),
            std::slice::from_ref(&own.guid)
        );
        assert!(files::list(copy.path()).is_file());
        // A list the copy's Nucleus wrote in a way Rubylight can't edit
        // refuses the launch before anything is written.
        let other = Setup::new(r"D:\Puzzle\Puzzle.exe", "Puzzle", None);
        std::fs::write(files::list(copy.path()), b"[not a list").unwrap();
        assert!(write_own(host.path(), copy.path(), &other, None).is_err());
        assert!(!host.path().join("handlers").join(other.file()).exists());
    }

    #[test]
    fn a_linked_handler_is_not_read() {
        let host = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(elsewhere.path().join("x.js")).unwrap();
        let handlers = host.path().join("handlers");
        std::fs::create_dir_all(&handlers).unwrap();
        junction(&elsewhere.path().join("x.js"), &handlers.join("linked.js"));
        let copy = tempfile::tempdir().unwrap();
        assert_eq!(mirror_handlers(host.path(), copy.path(), None).unwrap(), 0);
    }
}
