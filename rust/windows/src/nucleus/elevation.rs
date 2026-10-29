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
mod tests {
    use super::*;

    /// A copy of a Windows program with its manifest replaced by `level`'s,
    /// or removed.
    fn program(folder: &Path, name: &str, level: Option<&str>) -> PathBuf {
        let system = std::env::var("SystemRoot").unwrap();
        copy(
            &Path::new(&system).join(r"System32\where.exe"),
            folder,
            name,
            level,
        )
    }

    /// A copy of `source` with only a manifest of `level`, or no resources.
    fn copy(source: &Path, folder: &Path, name: &str, level: Option<&str>) -> PathBuf {
        use windows::Win32::System::LibraryLoader::{
            BeginUpdateResourceW, EndUpdateResourceW, UpdateResourceW,
        };
        let path = folder.join(name);
        std::fs::copy(source, &path).unwrap();
        let wide = crate::text::to_wide(&path.to_string_lossy());
        let manifest = level.map(|level| {
            format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security><requestedPrivileges>
      <requestedExecutionLevel level="{level}" uiAccess="false"/>
    </requestedPrivileges></security>
  </trustInfo>
</assembly>"#
            )
        });
        // SAFETY: the path is NUL-terminated, the data outlives the update,
        // and the handle is ended once.
        unsafe {
            // Every resource goes: the copy has one manifest or none.
            let update = BeginUpdateResourceW(PCWSTR(wide.as_ptr()), true).unwrap();
            if let Some(data) = &manifest {
                UpdateResourceW(
                    update,
                    PCWSTR(std::ptr::without_provenance(24)),
                    PCWSTR(std::ptr::without_provenance(1)),
                    0x0409,
                    Some(data.as_ptr().cast()),
                    data.len() as u32,
                )
                .unwrap();
            }
            EndUpdateResourceW(update, false).unwrap();
        }
        path
    }

    #[test]
    fn the_embedded_manifest_level_is_read() {
        let folder = tempfile::tempdir().unwrap();
        let admin = program(folder.path(), "game.exe", Some("requireAdministrator"));
        assert_eq!(
            manifest_need(&admin).unwrap(),
            Some(Need::Administrator(Why::Manifest))
        );
        let invoker = program(folder.path(), "plain.exe", Some("asInvoker"));
        assert_eq!(manifest_need(&invoker).unwrap(), Some(Need::Nothing));
        let highest = program(folder.path(), "most.exe", Some("highestAvailable"));
        assert_eq!(manifest_need(&highest).unwrap(), Some(Need::Highest));
        // This test program's own, which Rust's linker embeds.
        assert_eq!(
            manifest_need(&std::env::current_exe().unwrap()).unwrap(),
            Some(Need::Nothing)
        );
    }

    #[test]
    fn without_an_embedded_manifest_the_one_beside_it_counts() {
        let folder = tempfile::tempdir().unwrap();
        let bare = program(folder.path(), "bare.exe", None);
        assert_eq!(manifest_need(&bare).unwrap(), None);
        std::fs::write(
            folder.path().join("bare.exe.manifest"),
            b"\xEF\xBB\xBF<assembly><requestedExecutionLevel level=\"requireAdministrator\"/></assembly>",
        )
        .unwrap();
        assert_eq!(
            manifest_need(&bare).unwrap(),
            Some(Need::Administrator(Why::Manifest))
        );
        // An embedded one wins over the file.
        let invoker = program(folder.path(), "plain.exe", Some("asInvoker"));
        std::fs::write(
            folder.path().join("plain.exe.manifest"),
            "<requestedExecutionLevel level=\"requireAdministrator\"/>",
        )
        .unwrap();
        assert_eq!(manifest_need(&invoker).unwrap(), Some(Need::Nothing));
    }

    #[test]
    fn a_file_that_is_no_program_is_an_error() {
        let folder = tempfile::tempdir().unwrap();
        let text = folder.path().join("game.exe");
        std::fs::write(&text, b"not a program").unwrap();
        let error = format!("{:#}", manifest_need(&text).unwrap_err());
        assert!(error.contains("game.exe's manifest"), "{error}");
    }

    #[test]
    fn utf16_manifests_are_read() {
        let mut bytes = vec![0xFF, 0xFE];
        for unit in "<requestedExecutionLevel level='asInvoker'/>".encode_utf16() {
            bytes.extend(unit.to_le_bytes());
        }
        assert_eq!(rule::manifest_need(&decode(&bytes)), Some(Need::Nothing));
    }

    #[test]
    fn where_exe_is_64_bit() {
        let system = std::env::var("SystemRoot").unwrap();
        assert!(!thirty_two_bit(&Path::new(&system).join(r"System32\where.exe")).unwrap());
        let wow = Path::new(&system).join(r"SysWOW64\where.exe");
        if wow.is_file() {
            assert!(thirty_two_bit(&wow).unwrap());
        }
    }

    #[test]
    fn a_32_bit_program_named_like_an_installer_needs_admin_without_a_level() {
        let system = std::env::var("SystemRoot").unwrap();
        let wow = Path::new(&system).join(r"SysWOW64\where.exe");
        if !wow.is_file() {
            return;
        }
        let folder = tempfile::tempdir().unwrap();
        let sid = crate::process::own_sid().unwrap();
        // A level turns detection off.
        let invoker = copy(&wow, folder.path(), "setup.exe", Some("asInvoker"));
        assert_eq!(game_need(&invoker, &sid, None).unwrap(), Need::Nothing);
        let setup = copy(&wow, folder.path(), "game_setup.exe", None);
        assert_eq!(manifest_need(&setup).unwrap(), None);
        assert_eq!(
            game_need(&setup, &sid, None).unwrap(),
            Need::Administrator(Why::Installer)
        );
        let game = copy(&wow, folder.path(), "game.exe", None);
        assert_eq!(game_need(&game, &sid, None).unwrap(), Need::Nothing);
        // A 64-bit one is never taken for an installer.
        let system = std::env::var("SystemRoot").unwrap();
        let native = copy(
            &Path::new(&system).join(r"System32\where.exe"),
            folder.path(),
            "update.exe",
            None,
        );
        assert_eq!(game_need(&native, &sid, None).unwrap(), Need::Nothing);
    }

    /// The manifest beside `exe`, asking for administrator rights, then
    /// `padding` spaces.
    fn admin_manifest(exe: &Path, padding: usize) -> PathBuf {
        let path = external_manifest(exe);
        let mut text =
            b"<assembly><requestedExecutionLevel level=\"requireAdministrator\"/></assembly>"
                .to_vec();
        text.resize(text.len() + padding, b' ');
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn a_manifest_beside_the_game_is_read_up_to_its_limit() {
        let folder = tempfile::tempdir().unwrap();
        let bare = program(folder.path(), "bare.exe", None);
        let manifest = admin_manifest(&bare, 0);
        let fits = MANIFEST_LIMIT as usize - std::fs::metadata(&manifest).unwrap().len() as usize;
        admin_manifest(&bare, fits);
        assert_eq!(
            manifest_need(&bare).unwrap(),
            Some(Need::Administrator(Why::Manifest))
        );
        // One byte more and it is refused, never read whole.
        admin_manifest(&bare, fits + 1);
        let error = format!("{:#}", manifest_need(&bare).unwrap_err());
        assert!(
            error.contains("bare.exe.manifest is larger than 64 KiB"),
            "{error}"
        );
    }

    #[test]
    fn a_link_in_place_of_the_manifest_is_not_followed() {
        let folder = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let bare = program(folder.path(), "bare.exe", None);
        junction(elsewhere.path(), &external_manifest(&bare));
        let error = format!("{:#}", manifest_need(&bare).unwrap_err());
        assert!(
            error.contains("bare.exe.manifest is a link or a folder, not a manifest"),
            "{error}"
        );
        // A folder of that name the same.
        let other = program(folder.path(), "other.exe", None);
        std::fs::create_dir(external_manifest(&other)).unwrap();
        let error = format!("{:#}", manifest_need(&other).unwrap_err());
        assert!(error.contains("is a link or a folder"), "{error}");
    }

    #[test]
    fn the_games_files_are_read_as_the_user() {
        let sid = crate::process::own_sid().unwrap();
        let user = crate::user_folder::tests::without_own_sid();
        let root = tempfile::tempdir().unwrap();
        // A game whose folder only this account by name may read: the
        // token without its SID, standing in for the user, is refused it.
        let named = root.path().join("named");
        crate::process::create_folder(&named, &format!("D:P(A;OICI;FA;;;{sid})")).unwrap();
        let game = program(&named, "game_setup.exe", Some("asInvoker"));
        assert_eq!(game_need(&game, &sid, None).unwrap(), Need::Nothing);
        let error = format!("{:#}", game_need(&game, &sid, Some(&user)).unwrap_err());
        assert!(error.contains("game_setup.exe's manifest"), "{error}");
        assert!(error.contains("0x80070005"), "{error}");
        // A game anyone may read, whose manifest only this account may: the
        // manifest is read as the user too.
        let open = root.path().join("open");
        crate::process::create_folder(&open, "D:P(A;OICI;FA;;;WD)").unwrap();
        let bare = program(&open, "bare.exe", None);
        assert_eq!(game_need(&bare, &sid, Some(&user)).unwrap(), Need::Nothing);
        admin_manifest_of(&bare, &sid, &user);
        assert_eq!(
            game_need(&bare, &sid, None).unwrap(),
            Need::Administrator(Why::Manifest)
        );
        let error = format!("{:#}", game_need(&bare, &sid, Some(&user)).unwrap_err());
        assert!(error.contains("reading"), "{error}");
        assert!(error.contains("bare.exe.manifest"), "{error}");
        // The host is itself again once the user's reads are done.
        assert_eq!(
            game_need(&bare, &sid, None).unwrap(),
            Need::Administrator(Why::Manifest)
        );
    }

    /// The manifest beside `exe`, asking for administrator rights, that only
    /// this account by name (`sid`) may read: `user`, standing in for the
    /// user, may not open it.
    fn admin_manifest_of(exe: &Path, sid: &str, user: &OwnedHandle) -> PathBuf {
        let manifest = admin_manifest(exe, 0);
        let status = std::process::Command::new("icacls")
            .arg(&manifest)
            .args(["/inheritance:r", "/grant:r", &format!("*{sid}:F")])
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "icacls failed");
        let opened = {
            let _user = crate::user_folder::Impersonation::begin(Some(user)).unwrap();
            std::fs::File::open(&manifest).map(drop)
        };
        assert!(
            opened.is_err_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied),
            "the stand-in for the user may open {}, so these tests can't tell whose reads it is: {}",
            manifest.display(),
            String::from_utf8_lossy(
                &std::process::Command::new("icacls")
                    .arg(&manifest)
                    .output()
                    .unwrap()
                    .stdout
            )
        );
        manifest
    }

    #[test]
    fn each_read_of_the_games_files_acts_as_the_user_anew() {
        // A read that leaves this thread as the host, as a loader that read
        // the manifest a program embeds could: the one beside it is still
        // read as the user.
        fn then_as_the_host(exe: &Path) -> Result<Option<String>> {
            let text = embedded_manifest(exe);
            // SAFETY: ends this thread's impersonation; the guard ending it
            // again does nothing.
            unsafe { windows::Win32::Security::RevertToSelf() }.unwrap();
            text
        }
        let sid = crate::process::own_sid().unwrap();
        let user = crate::user_folder::tests::without_own_sid();
        let root = tempfile::tempdir().unwrap();
        let open = root.path().join("open");
        crate::process::create_folder(&open, "D:P(A;OICI;FA;;;WD)").unwrap();
        let bare = program(&open, "bare.exe", None);
        admin_manifest_of(&bare, &sid, &user);
        assert_eq!(
            manifest_as(&bare, None, then_as_the_host).unwrap(),
            Some(Need::Administrator(Why::Manifest))
        );
        let error = format!(
            "{:#}",
            manifest_as(&bare, Some(&user), then_as_the_host).unwrap_err()
        );
        assert!(error.contains("reading"), "{error}");
        assert!(error.contains("bare.exe.manifest"), "{error}");
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

    /// A folder anyone may change, whose new files anyone may only read.
    fn read_only_files(path: &Path) {
        crate::process::create_folder(path, "D:P(A;;FA;;;WD)(A;OICIIO;FR;;;WD)").unwrap();
    }

    #[test]
    fn a_game_folder_the_user_may_change_needs_nothing() {
        let game = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(game.path().join("data").join("maps")).unwrap();
        std::fs::write(game.path().join("game.exe"), b"MZ").unwrap();
        std::fs::write(game.path().join("data").join("maps").join("1.map"), b"x").unwrap();
        assert_eq!(folder_need(game.path(), None).unwrap(), Need::Nothing);
    }

    #[test]
    fn a_file_the_user_may_not_change_needs_admin() {
        let game = tempfile::tempdir().unwrap();
        std::fs::write(game.path().join("game.exe"), b"MZ").unwrap();
        let locked = game.path().join("data");
        read_only_files(&locked);
        std::fs::write(locked.join("1.map"), b"x").unwrap();
        assert_eq!(
            folder_need(game.path(), None).unwrap(),
            Need::Administrator(Why::Folder)
        );
    }

    #[test]
    fn links_in_the_game_folder_are_followed_as_nucleus_does() {
        let elsewhere = tempfile::tempdir().unwrap();
        let locked = elsewhere.path().join("locked");
        read_only_files(&locked);
        std::fs::write(locked.join("1.map"), b"x").unwrap();
        let game = tempfile::tempdir().unwrap();
        std::fs::write(game.path().join("game.exe"), b"MZ").unwrap();
        // .NET's walk goes into it and stops on the file it can't change.
        junction(&locked, &game.path().join("data"));
        assert_eq!(
            walk(game.path()).unwrap().need,
            Need::Administrator(Why::Folder)
        );
        // One to nothing is passed over, as .NET does.
        std::fs::remove_dir(game.path().join("data")).unwrap();
        let gone = elsewhere.path().join("gone");
        std::fs::create_dir(&gone).unwrap();
        junction(&gone, &game.path().join("data"));
        std::fs::remove_dir(&gone).unwrap();
        assert_eq!(walk(game.path()).unwrap().need, Need::Nothing);
    }

    #[test]
    fn a_link_back_into_the_game_folder_is_walked_once() {
        let game = tempfile::tempdir().unwrap();
        std::fs::create_dir(game.path().join("data")).unwrap();
        std::fs::write(game.path().join("data").join("1.map"), b"x").unwrap();
        junction(game.path(), &game.path().join("data").join("loop"));
        junction(game.path(), &game.path().join("again"));
        assert_eq!(walk(game.path()).unwrap().need, Need::Nothing);
    }

    #[test]
    fn the_game_folder_is_walked_as_the_user() {
        let sid = crate::process::own_sid().unwrap();
        let game = tempfile::tempdir().unwrap();
        // Only this account by name may list it: the token without its SID,
        // standing in for the user, is refused, as Nucleus run as them is.
        let named = game.path().join("named");
        crate::process::create_folder(&named, &format!("D:P(A;OICI;FA;;;{sid})")).unwrap();
        std::fs::write(named.join("1.map"), b"x").unwrap();
        let user = crate::user_folder::tests::without_own_sid();
        let walked = Mutex::new(Walked::new());
        let now = Instant::now();
        assert_eq!(
            kept(&walked, &named, None, now, &mut walk).unwrap(),
            Need::Nothing
        );
        assert_eq!(
            kept(&walked, &named, Some(&user), now, &mut walk).unwrap(),
            Need::Administrator(Why::Folder)
        );
    }

    /// Give everyone `rights` on `path` (icacls' letters).
    fn grant(path: &Path, rights: &str) {
        let status = std::process::Command::new("icacls")
            .arg(path)
            .args(["/grant", &format!("*S-1-1-0:{rights}")])
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "icacls failed");
    }

    #[test]
    fn the_walk_is_kept_until_what_it_looked_at_changes() {
        let game = tempfile::tempdir().unwrap();
        let data = game.path().join("data");
        std::fs::create_dir_all(data.join("maps")).unwrap();
        std::fs::write(game.path().join("game.exe"), b"MZ").unwrap();
        let walked = Mutex::new(Walked::new());
        let walks = std::cell::Cell::new(0);
        let mut counted = |folder: &Path| {
            walks.set(walks.get() + 1);
            walk(folder)
        };
        let t0 = Instant::now();
        // A clock tick after the change before it, as a launch is: a walk in
        // the tick of a change is not kept.
        let mut look = |folder: &Path, at: Instant| {
            next_tick();
            kept(&walked, folder, None, at, &mut counted).unwrap()
        };
        for _ in 0..3 {
            assert_eq!(look(game.path(), t0), Need::Nothing);
        }
        // A launch after the first adds no walk.
        assert_eq!(walks.get(), 1);
        // A file added, removed or renamed in the folder walks it again.
        std::fs::write(game.path().join("patch.dll"), b"MZ").unwrap();
        look(game.path(), t0);
        assert_eq!(walks.get(), 2);
        std::fs::rename(game.path().join("patch.dll"), game.path().join("old.dll")).unwrap();
        look(game.path(), t0);
        assert_eq!(walks.get(), 3);
        // So does one added in a folder below it.
        std::fs::write(data.join("maps").join("2.map"), b"x").unwrap();
        look(game.path(), t0);
        assert_eq!(walks.get(), 4);
        // And a folder's permissions changed.
        grant(&data, "(RX)");
        look(game.path(), t0);
        assert_eq!(walks.get(), 5);
        look(game.path(), t0);
        assert_eq!(walks.get(), 5);
        // A file added behind a junction, in the folder it leads to.
        let elsewhere = tempfile::tempdir().unwrap();
        junction(elsewhere.path(), &data.join("linked"));
        look(game.path(), t0);
        assert_eq!(walks.get(), 6);
        std::fs::write(elsewhere.path().join("3.map"), b"x").unwrap();
        look(game.path(), t0);
        assert_eq!(walks.get(), 7);
        // A file the user may not change, then may: an administrator fixed
        // what was refused, and the next launch sees it.
        let locked = game.path().join("locked");
        read_only_files(&locked);
        std::fs::write(locked.join("1.map"), b"x").unwrap();
        assert_eq!(look(game.path(), t0), Need::Administrator(Why::Folder));
        assert_eq!(look(game.path(), t0), Need::Administrator(Why::Folder));
        assert_eq!(walks.get(), 8);
        grant(&locked.join("1.map"), "M");
        assert_eq!(look(game.path(), t0), Need::Nothing);
        assert_eq!(walks.get(), 9);
        // No answer is older than KEPT_FOR: a change no change time shows.
        look(game.path(), t0 + KEPT_FOR);
        assert_eq!(walks.get(), 10);
        look(game.path(), t0 + KEPT_FOR + Duration::from_secs(1));
        assert_eq!(walks.get(), 10);
        // Another game's folder has its own answer.
        let other = tempfile::tempdir().unwrap();
        look(other.path(), t0 + KEPT_FOR);
        look(game.path(), t0 + KEPT_FOR);
        assert_eq!(walks.get(), 11);
        // A failed start forgets the game's folder, for every user.
        forget_in(&walked, game.path());
        look(game.path(), t0 + KEPT_FOR);
        assert_eq!(walks.get(), 12);
        look(other.path(), t0 + KEPT_FOR);
        assert_eq!(walks.get(), 12);
        // Answers too old to use go as another is kept.
        let kept_for = |folder: &Path| {
            walked
                .lock()
                .unwrap()
                .keys()
                .any(|(_, kept)| kept == folder)
        };
        let later = tempfile::tempdir().unwrap();
        look(later.path(), t0 + KEPT_FOR * 2 - Duration::from_secs(1));
        assert!(kept_for(game.path()) && kept_for(other.path()));
        let last = tempfile::tempdir().unwrap();
        look(last.path(), t0 + KEPT_FOR * 2);
        assert!(!kept_for(game.path()) && !kept_for(other.path()));
        assert!(kept_for(later.path()) && kept_for(last.path()));
        // A walk that fails is not kept.
        let mut failing = |_: &Path| -> Result<Walk> { anyhow::bail!("listing failed") };
        let third = tempfile::tempdir().unwrap();
        let at = Instant::now();
        assert!(kept(&walked, third.path(), None, at, &mut failing).is_err());
        assert!(!kept_for(third.path()));
    }

    #[test]
    fn a_walk_that_saw_a_change_made_after_it_began_is_not_kept() {
        let game = tempfile::tempdir().unwrap();
        std::fs::write(game.path().join("game.exe"), b"MZ").unwrap();
        let walked = Mutex::new(Walked::new());
        let walks = std::cell::Cell::new(0);
        // A file added as the walk begins: another change in that clock
        // tick would leave the folder's change time as the walk saw it.
        let mut changing = |folder: &Path| {
            walks.set(walks.get() + 1);
            std::fs::write(folder.join(format!("{}.dll", walks.get())), b"MZ").unwrap();
            walk(folder)
        };
        let t0 = Instant::now();
        next_tick();
        kept(&walked, game.path(), None, t0, &mut changing).unwrap();
        kept(&walked, game.path(), None, t0, &mut changing).unwrap();
        assert_eq!(walks.get(), 2);
        assert!(walked.lock().unwrap().is_empty());
        // Nothing changed since the walk began: kept.
        let mut counted = |folder: &Path| {
            walks.set(walks.get() + 1);
            walk(folder)
        };
        next_tick();
        kept(&walked, game.path(), None, t0, &mut counted).unwrap();
        kept(&walked, game.path(), None, t0, &mut counted).unwrap();
        assert_eq!(walks.get(), 3);
    }

    #[test]
    fn a_link_the_walk_passed_keeps_the_answer_while_it_leads_where_it_did() {
        use crate::user_folder::tests::mount;
        let game = tempfile::tempdir().unwrap();
        let data = game.path().join("data");
        std::fs::create_dir(&data).unwrap();
        std::fs::write(game.path().join("game.exe"), b"MZ").unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let locked = elsewhere.path().join("locked");
        read_only_files(&locked);
        std::fs::write(locked.join("1.map"), b"x").unwrap();
        let walked = Mutex::new(Walked::new());
        let walks = std::cell::Cell::new(0);
        let mut counted = |folder: &Path| {
            walks.set(walks.get() + 1);
            walk(folder)
        };
        let t0 = Instant::now();
        let mut look = |at: Instant| {
            next_tick();
            kept(&walked, game.path(), None, at, &mut counted).unwrap()
        };
        // A second link into a folder already walked, then led elsewhere in
        // place: walked again.
        let again = game.path().join("again");
        junction(&data, &again);
        assert_eq!(look(t0), Need::Nothing);
        assert_eq!(look(t0), Need::Nothing);
        assert_eq!(walks.get(), 1);
        mount(&again, &locked).unwrap();
        assert_eq!(look(t0), Need::Administrator(Why::Folder));
        assert_eq!(walks.get(), 2);
        std::fs::remove_dir(&again).unwrap();
        // A link to nothing, until what it names is made.
        let later = elsewhere.path().join("later");
        std::fs::create_dir(&later).unwrap();
        junction(&later, &again);
        std::fs::remove_dir(&later).unwrap();
        assert_eq!(look(t0), Need::Nothing);
        assert_eq!(look(t0), Need::Nothing);
        assert_eq!(walks.get(), 3);
        read_only_files(&later);
        std::fs::write(later.join("1.map"), b"x").unwrap();
        assert_eq!(look(t0), Need::Administrator(Why::Folder));
        assert_eq!(walks.get(), 4);
    }

    #[test]
    fn a_folder_reached_again_is_kept_while_the_way_there_leads_there() {
        use crate::user_folder::tests::mount;
        let game = tempfile::tempdir().unwrap();
        std::fs::create_dir(game.path().join("data")).unwrap();
        // `a` leads to `outer\data`, `outer` back into the game: to its
        // `data`, walked already.
        let elsewhere = tempfile::tempdir().unwrap();
        let outer = elsewhere.path().join("outer");
        junction(game.path(), &outer);
        junction(&outer.join("data"), &game.path().join("a"));
        let walked = Mutex::new(Walked::new());
        let walks = std::cell::Cell::new(0);
        let mut counted = |folder: &Path| {
            walks.set(walks.get() + 1);
            walk(folder)
        };
        let t0 = Instant::now();
        let mut look = |at: Instant| {
            next_tick();
            kept(&walked, game.path(), None, at, &mut counted).unwrap()
        };
        assert_eq!(look(t0), Need::Nothing);
        assert_eq!(look(t0), Need::Nothing);
        assert_eq!(walks.get(), 1);
        // `outer` led elsewhere: `a` now reaches files the user may not
        // change, though neither `a` nor the game's folders changed.
        let other = elsewhere.path().join("other");
        std::fs::create_dir(&other).unwrap();
        read_only_files(&other.join("data"));
        std::fs::write(other.join("data").join("1.map"), b"x").unwrap();
        mount(&outer, &other).unwrap();
        assert_eq!(look(t0), Need::Administrator(Why::Folder));
        assert_eq!(walks.get(), 2);
    }
}
