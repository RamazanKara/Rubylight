//! What a game's .exe asks of Windows to start, read from the PC, for
//! [`butterpollo_core::nucleus::elevation::choose`]: its manifest's level
//! (or Windows' installer detection without one), the compatibility
//! settings of the machine and of the signed-in user, and whether that user
//! may change the files Nucleus Co-op changes before it starts the game.
//!
//! The game's folder is usually one its user may change, so the host never
//! opens a file there as itself: a link planted there would let that user
//! have SYSTEM read any file. Every read of it is done as the user.
use super::layers::{self, Layers};
use anyhow::{Context, Result, bail};
use butterpollo_core::nucleus::elevation::{self as rule, Need, Why};
use std::collections::BTreeMap;
use std::os::windows::io::OwnedHandle;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::FreeLibrary;
use windows::Win32::System::LibraryLoader::{
    FindResourceW, LOAD_LIBRARY_AS_DATAFILE, LOAD_LIBRARY_AS_IMAGE_RESOURCE, LoadLibraryExW,
    LoadResource, LockResource, SizeofResource,
};
use windows::core::PCWSTR;

/// The most of a `<name>.exe.manifest` that is read: Windows' own are a few
/// KiB.
const MANIFEST_LIMIT: u64 = 64 * 1024;

/// What the game at `exe` needs, for the user `sid` of `token` (None: as the
/// host, in tests), who its files are read as: the most of its manifest,
/// Windows' installer detection and both compatibility settings.
pub fn game_need(exe: &Path, sid: &str, token: Option<&OwnedHandle>) -> Result<Need> {
    let Program {
        manifest,
        installer,
    } = program(exe, token)?;
    let path = exe.to_string_lossy();
    let machine = layers::machine_values()?;
    let user = layers::UserHive::of(sid.to_string()).values()?;
    let need = rule::most(
        [
            manifest.unwrap_or(Need::Nothing),
            if installer {
                Need::Administrator(Why::Installer)
            } else {
                Need::Nothing
            },
        ]
        .into_iter()
        .chain(
            [machine, user]
                .iter()
                .filter_map(|values| layers::program_layers(values, &path))
                .map(|value| rule::layers_need(&value)),
        ),
    );
    tracing::debug!(exe = %exe.display(), ?manifest, installer, ?need, "Nucleus Co-op: what the game asks of Windows");
    Ok(need)
}

/// What the game's own files ask of Windows.
struct Program {
    /// Its manifest's level ([`manifest_need`]).
    manifest: Option<Need>,
    /// Whether Windows' installer detection takes it for an installer.
    installer: bool,
}

/// Every read of `exe` and its manifest, done here as the user of `token`
/// (None: as the host, in tests).
fn program(exe: &Path, token: Option<&OwnedHandle>) -> Result<Program> {
    let manifest = manifest_as(exe, token, embedded_manifest)?;
    let installer = manifest.is_none()
        && exe
            .file_name()
            .is_some_and(|name| rule::installer_name(&name.to_string_lossy()))
        && read_as(token, exe, thirty_two_bit)?;
    Ok(Program {
        manifest,
        installer,
    })
}

/// The level the manifest of `exe` asks for: the one it embeds, else the
/// `<name>.exe.manifest` beside it, as Windows reads them. None when it
/// names none. Read as this thread is: [`game_need`] reads it as the user.
pub fn manifest_need(exe: &Path) -> Result<Option<Need>> {
    manifest_as(exe, None, embedded_manifest)
}

/// [`manifest_need`] read as the user of `token`, with `embedded` reading
/// the manifest `exe` embeds.
fn manifest_as(
    exe: &Path,
    token: Option<&OwnedHandle>,
    embedded: fn(&Path) -> Result<Option<String>>,
) -> Result<Option<Need>> {
    let text = match read_as(token, exe, embedded)? {
        Some(text) => Some(text),
        None => read_as(token, exe, external_manifest_text)?,
    };
    Ok(text.and_then(|text| rule::manifest_need(&text)))
}

/// `read` of `exe`, as the user of `token` from its start. Each read acts as
/// the user anew: should one end this thread's impersonation (Windows'
/// loader reads the manifest a program embeds), the next is still not done
/// as the host.
fn read_as<T>(
    token: Option<&OwnedHandle>,
    exe: &Path,
    read: impl FnOnce(&Path) -> Result<T>,
) -> Result<T> {
    let _user =
        crate::user_folder::Impersonation::begin(token).context("acting as the signed-in user")?;
    read(exe)
}

fn external_manifest(exe: &Path) -> PathBuf {
    let mut name = exe.as_os_str().to_owned();
    name.push(".manifest");
    PathBuf::from(name)
}

/// The text of the `<name>.exe.manifest` beside `exe`, None without one:
/// only a file on a disk, never through a link in its place, and at most
/// [`MANIFEST_LIMIT`] bytes.
fn external_manifest_text(exe: &Path) -> Result<Option<String>> {
    use std::io::Read;
    use std::os::windows::fs::MetadataExt;
    let path = external_manifest(exe);
    let not_a_file = || {
        anyhow::anyhow!(
            "{} is a link or a folder, not a manifest, so Rubylight does not read it: replace it with the file or remove it",
            path.display()
        )
    };
    let mut file = match crate::process::open_unfollowed(&path, false) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            // A junction or a folder opens only as a folder.
            if std::fs::symlink_metadata(&path)
                .is_ok_and(|metadata| metadata.file_attributes() & (0x400 | 0x10) != 0)
            {
                return Err(not_a_file());
            }
            return Err(error).with_context(|| format!("reading {}", path.display()));
        }
    };
    let attributes = file
        .metadata()
        .with_context(|| format!("reading {}", path.display()))?
        .file_attributes();
    if attributes & (0x400 | 0x10) != 0 || !on_disk(&file) {
        return Err(not_a_file());
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MANIFEST_LIMIT + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {}", path.display()))?;
    if bytes.len() as u64 > MANIFEST_LIMIT {
        bail!(
            "{} is larger than {} KiB, which no manifest is, so Rubylight does not read it: remove it",
            path.display(),
            MANIFEST_LIMIT / 1024
        );
    }
    Ok(Some(decode(&bytes)))
}

/// Whether `file` is a file on a disk, not a pipe or a device a path
/// through the object manager could name.
fn on_disk(file: &std::fs::File) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{FILE_TYPE_DISK, GetFileType};
    // SAFETY: the handle is open for as long as `file` is.
    unsafe { GetFileType(HANDLE(file.as_raw_handle())) == FILE_TYPE_DISK }
}

/// The manifest resource (`RT_MANIFEST` 1) `exe` embeds, read as data: no
/// code of it runs.
fn embedded_manifest(exe: &Path) -> Result<Option<String>> {
    const RT_MANIFEST: usize = 24;
    const CREATEPROCESS_MANIFEST: usize = 1;
    let name = crate::text::to_wide(&exe.to_string_lossy());
    // SAFETY: `name` is NUL-terminated and outlives the call; a data file
    // mapping runs nothing, and the module is freed once below.
    let module = unsafe {
        LoadLibraryExW(
            PCWSTR(name.as_ptr()),
            None,
            LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE,
        )
    }
    .with_context(|| format!("reading {}'s manifest", exe.display()))?;
    // SAFETY: the module is loaded until FreeLibrary; the resource's bytes
    // are copied out before it is freed, and its size bounds the slice.
    let text = unsafe {
        let resource = FindResourceW(
            Some(module),
            PCWSTR(std::ptr::without_provenance(CREATEPROCESS_MANIFEST)),
            PCWSTR(std::ptr::without_provenance(RT_MANIFEST)),
        );
        let text = if resource.is_invalid() {
            Ok(None)
        } else {
            let size = SizeofResource(Some(module), resource) as usize;
            LoadResource(Some(module), resource).map(|data| {
                let bytes = LockResource(data).cast::<u8>();
                (!bytes.is_null() && size > 0)
                    .then(|| decode(std::slice::from_raw_parts(bytes, size)))
            })
        };
        let _ = FreeLibrary(module);
        text
    };
    text.with_context(|| format!("reading {}'s manifest", exe.display()))
}

/// A manifest's text: UTF-8, or UTF-16 with its byte order mark.
fn decode(bytes: &[u8]) -> String {
    match bytes {
        [0xFF, 0xFE, rest @ ..] => {
            let units: Vec<u16> = rest
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&pair| u16::from_le_bytes(pair))
                .collect();
            String::from_utf16_lossy(&units)
        }
        _ => String::from_utf8_lossy(bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes))
            .into_owned(),
    }
}

/// Whether `exe` is a 32-bit Windows program, which installer detection
/// applies to. Read as this thread is.
fn thirty_two_bit(exe: &Path) -> Result<bool> {
    use windows::Win32::Storage::FileSystem::GetBinaryTypeW;
    const SCS_32BIT_BINARY: u32 = 0;
    let name = crate::text::to_wide(&exe.to_string_lossy());
    let mut kind = 0u32;
    // SAFETY: `name` is NUL-terminated and `kind` is writable, both live for the call.
    unsafe { GetBinaryTypeW(PCWSTR(name.as_ptr()), &mut kind) }
        .with_context(|| format!("reading what kind of program {} is", exe.display()))?;
    Ok(kind == SCS_32BIT_BINARY)
}

/// The longest a [`folder_need`] answer is kept: a change no change time it
/// checks shows (the permissions of a file the walk passed) counts within
/// it.
const KEPT_FOR: Duration = Duration::from_secs(60);

/// A [`walk`]'s answer and the change time of everything it looked at.
struct Walk {
    need: Need,
    /// Every folder it went into (as the path it reached it by: a link's
    /// target's time), each link to a folder itself, one to nothing, and
    /// the file it stopped at. None when one of them has no change time to
    /// read: the answer is not kept.
    seen: Option<Vec<Seen>>,
}

/// A path a [`walk`] looked at, whether a link there was followed, and its
/// [`change_time`] then: None when it led nowhere.
type Seen = (PathBuf, bool, Option<i64>);

/// A kept [`Walk`], from `at`.
struct Kept {
    need: Need,
    seen: Vec<Seen>,
    at: Instant,
}

/// Each user's (by SID) [`folder_need`] answer for a game folder.
type Walked = BTreeMap<(String, PathBuf), Kept>;

static WALKED: Mutex<Walked> = Mutex::new(BTreeMap::new());

/// [`Need::Administrator`] ([`Why::Folder`]) when the user of `token` may
/// not change the attributes of a file in `folder` or list one of its
/// folders: Nucleus Co-op clears every file's attributes there before it
/// starts the game (`StartGameUtil.UnlockGameFiles`) and stops on the first
/// it can't. Read as that user (None: as the host, in tests), the walk
/// stopping at the first such file. The walk takes as long as the folder is
/// big, so its answer is kept for up to [`KEPT_FOR`] while no folder it
/// went into, nor the file it stopped at, changes (their change times: a
/// file or folder added, removed or renamed, permissions or attributes),
/// and until [`forget`]. Older answers go as new ones are kept. One that saw
/// a change made after the walk began is not kept: a change time moves
/// once per clock tick (15.6 ms on most PCs), so another change in that
/// tick would leave it as it was.
pub fn folder_need(folder: &Path, token: Option<&OwnedHandle>) -> Result<Need> {
    kept(&WALKED, folder, token, Instant::now(), &mut walk)
}

/// Walk `folder` again next time, for every user: a session that ran
/// Nucleus Co-op for its game failed to start, maybe for a file the kept
/// answer missed.
pub fn forget(folder: &Path) {
    forget_in(&WALKED, folder);
}

/// Whether an answer for `folder` is kept, for any user.
#[cfg(test)]
pub(crate) fn is_kept(folder: &Path) -> bool {
    WALKED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .keys()
        .any(|(_, kept)| kept == folder)
}

/// Wait for the system clock's next tick: whatever changed before has an
/// older change time than a walk begun after ([`settled`]).
#[cfg(test)]
pub(crate) fn next_tick() {
    let now = crate::process::now();
    while crate::process::now() <= now {
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn forget_in(walked: &Mutex<Walked>, folder: &Path) {
    walked
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|(_, kept), _| kept != folder);
}

/// [`folder_need`] at `now` with the answers kept in `walked`, from `walk`.
fn kept(
    walked: &Mutex<Walked>,
    folder: &Path,
    token: Option<&OwnedHandle>,
    now: Instant,
    walk: &mut dyn FnMut(&Path) -> Result<Walk>,
) -> Result<Need> {
    let user = token
        .map(crate::process::token_user_sid)
        .transpose()?
        .unwrap_or_default();
    let _user =
        crate::user_folder::Impersonation::begin(token).context("acting as the signed-in user")?;
    let key = (user, folder.to_path_buf());
    let lock = || walked.lock().unwrap_or_else(PoisonError::into_inner);
    // Checked outside the lock: one open per folder.
    let found = lock().remove(&key);
    if let Some(found) = found.filter(|found| fresh(found, now)) {
        let need = found.need;
        lock().insert(key, found);
        return Ok(need);
    }
    let began = crate::process::now();
    let Walk { need, seen } = walk(folder)?;
    if let Some(seen) = seen.filter(|seen| settled(seen, began)) {
        let mut walked = lock();
        walked.retain(|_, kept| now.saturating_duration_since(kept.at) < KEPT_FOR);
        walked.insert(
            key,
            Kept {
                need,
                seen,
                at: now,
            },
        );
    }
    Ok(need)
}

/// Whether nothing in `seen` changed at or after `began` (the system time,
/// which change times are stamped from: [`crate::process::now`]).
fn settled(seen: &[Seen], began: u64) -> bool {
    seen.iter()
        .all(|(_, _, changed)| changed.is_none_or(|time| (time as u64) < began))
}

/// Whether `kept` still holds at `now`: younger than [`KEPT_FOR`], and
/// nothing it looked at changed since.
fn fresh(kept: &Kept, now: Instant) -> bool {
    now.saturating_duration_since(kept.at) < KEPT_FOR
        && kept
            .seen
            .iter()
            .all(|(path, follow, changed)| match change_time(path, *follow) {
                Ok(time) => *changed == Some(time),
                Err(error) => changed.is_none() && error.kind() == std::io::ErrorKind::NotFound,
            })
}

/// When `path` (a link to one followed when `follow`, else the link itself)
/// last changed: for a folder, a file or folder added, removed or renamed
/// in it; its own attributes or permissions; a link's target.
fn change_time(path: &Path, follow: bool) -> std::io::Result<i64> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        FILE_BASIC_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileBasicInfo,
        GetFileInformationByHandleEx,
    };
    let mut flags = FILE_FLAG_BACKUP_SEMANTICS;
    if !follow {
        flags |= FILE_FLAG_OPEN_REPARSE_POINT;
    }
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES.0)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .custom_flags(flags.0)
        .open(path)?;
    let mut info = FILE_BASIC_INFO::default();
    // SAFETY: the handle is open and `info` is a writable FILE_BASIC_INFO of
    // the size passed.
    unsafe {
        GetFileInformationByHandleEx(
            HANDLE(file.as_raw_handle()),
            FileBasicInfo,
            (&raw mut info).cast(),
            size_of::<FILE_BASIC_INFO>() as u32,
        )
    }
    .map_err(crate::user_folder::os_error)?;
    Ok(info.ChangeTime)
}

/// The walk [`folder_need`] keeps the answer of, as Nucleus' own goes:
/// `Directory.GetFileSystemEntries(.., AllDirectories)` of .NET Framework
/// goes into every folder, a junction or a link to one too (one to nothing
/// is passed over), and `File.SetAttributes` changes a file link itself. A
/// folder reached twice through links is walked once. Each folder's change
/// time is read before it is listed: a change while it is walked shows on
/// the next look. So is each link's to a folder, which changes with what it
/// leads to, and one to nothing is kept while it still leads nowhere.
fn walk(folder: &Path) -> Result<Walk> {
    use std::os::windows::fs::MetadataExt;
    const DIRECTORY: u32 = 0x10;
    const REPARSE_POINT: u32 = 0x400;
    let mut pending = vec![folder.to_path_buf()];
    let mut walked = std::collections::BTreeSet::new();
    let mut seen = Vec::new();
    // A link passed (the game's folder too, should it be one), itself.
    let link = |path: &Path, seen: &mut Vec<Seen>| -> Result<()> {
        match change_time(path, false) {
            Ok(time) => seen.push((path.to_path_buf(), false, Some(time))),
            // Gone meanwhile: its folder's change time shows it.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        }
        Ok(())
    };
    // Stopped at `path`: kept while it and the folders before it stay.
    let stop = |path: &Path, mut seen: Vec<Seen>| Walk {
        need: locked(path),
        seen: change_time(path, true).ok().map(|time| {
            seen.push((path.to_path_buf(), true, Some(time)));
            seen
        }),
    };
    match std::fs::symlink_metadata(folder) {
        Ok(metadata) if metadata.file_attributes() & REPARSE_POINT != 0 => {
            link(folder, &mut seen)?;
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            return Ok(stop(folder, seen));
        }
        Err(error) => return Err(error).with_context(|| format!("reading {}", folder.display())),
    }
    while let Some(path) = pending.pop() {
        let below = path != folder;
        match std::fs::canonicalize(&path) {
            Ok(real) => {
                if !walked.insert(real) {
                    // Kept while this path still leads there (a link above
                    // it may be led elsewhere).
                    match change_time(&path, true) {
                        Ok(time) => seen.push((path, true, Some(time))),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            seen.push((path, true, None));
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                            return Ok(stop(&path, seen));
                        }
                        Err(error) => {
                            return Err(error)
                                .with_context(|| format!("reading {}", path.display()));
                        }
                    }
                    continue;
                }
            }
            Err(error) if below && error.kind() == std::io::ErrorKind::NotFound => {
                seen.push((path, true, None));
                continue;
            }
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                return Ok(stop(&path, seen));
            }
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        }
        match change_time(&path, true) {
            Ok(time) => seen.push((path.clone(), true, Some(time))),
            Err(error) if below && error.kind() == std::io::ErrorKind::NotFound => {
                seen.push((path, true, None));
                continue;
            }
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                return Ok(stop(&path, seen));
            }
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        }
        let entries = match std::fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                return Ok(stop(&path, seen));
            }
            Err(error) if below && error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("listing {}", path.display()));
            }
        };
        for entry in entries {
            let entry = entry.with_context(|| format!("listing {}", path.display()))?;
            // The entry itself, as listed: a link is not followed here.
            let attributes = entry
                .metadata()
                .with_context(|| format!("reading {}", entry.path().display()))?
                .file_attributes();
            if attributes & DIRECTORY != 0 {
                if attributes & REPARSE_POINT != 0 {
                    link(&entry.path(), &mut seen)?;
                }
                pending.push(entry.path());
            } else if !attributes_writable(&entry.path())? {
                return Ok(stop(&entry.path(), seen));
            }
        }
    }
    Ok(Walk {
        need: Need::Nothing,
        seen: Some(seen),
    })
}

fn locked(path: &Path) -> Need {
    tracing::info!(path = %path.display(), "Nucleus Co-op: the signed-in user may not change this file of the game, so Nucleus Co-op runs elevated");
    Need::Administrator(Why::Folder)
}

/// Whether this thread may change the file's attributes, as
/// `File.SetAttributes` does. Opening for that alone conflicts with no one
/// else's open.
fn attributes_writable(path: &Path) -> Result<bool> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        FILE_WRITE_ATTRIBUTES,
    };
    let opened = std::fs::OpenOptions::new()
        .access_mode(FILE_WRITE_ATTRIBUTES.0)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path);
    match opened {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => Ok(false),
        // Gone meanwhile: Nucleus won't find it either.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error).with_context(|| format!("opening {}", path.display())),
    }
}

#[cfg(test)]
mod tests;
