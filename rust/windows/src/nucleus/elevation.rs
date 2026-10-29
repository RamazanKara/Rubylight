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
use std::os::windows::io::OwnedHandle;
use std::path::{Path, PathBuf};
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
    let _user =
        crate::user_folder::Impersonation::begin(token).context("acting as the signed-in user")?;
    let manifest = manifest_need(exe)?;
    let installer = manifest.is_none()
        && exe
            .file_name()
            .is_some_and(|name| rule::installer_name(&name.to_string_lossy()))
        && thirty_two_bit(exe)?;
    Ok(Program {
        manifest,
        installer,
    })
}

/// The level the manifest of `exe` asks for: the one it embeds, else the
/// `<name>.exe.manifest` beside it, as Windows reads them. None when it
/// names none. Read as this thread is: [`game_need`] reads it as the user.
pub fn manifest_need(exe: &Path) -> Result<Option<Need>> {
    let text = match embedded_manifest(exe)? {
        Some(text) => text,
        None => match external_manifest_text(exe)? {
            Some(text) => text,
            None => return Ok(None),
        },
    };
    Ok(rule::manifest_need(&text))
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

/// [`Need::Administrator`] ([`Why::Folder`]) when the user of `token` may
/// not change the attributes of a file in `folder` or list one of its
/// folders: Nucleus Co-op clears every file's attributes there before it
/// starts the game (`StartGameUtil.UnlockGameFiles`) and stops on the first
/// it can't. Read as that user (None: as the host); links are not followed,
/// and the walk stops at the first such file.
pub fn folder_need(folder: &Path, token: Option<&OwnedHandle>) -> Result<Need> {
    let _user =
        crate::user_folder::Impersonation::begin(token).context("acting as the signed-in user")?;
    let mut pending = vec![folder.to_path_buf()];
    while let Some(path) = pending.pop() {
        let entries = match std::fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                return Ok(locked(&path));
            }
            Err(error) => {
                return Err(error).with_context(|| format!("listing {}", path.display()));
            }
        };
        for entry in entries {
            let entry = entry.with_context(|| format!("listing {}", path.display()))?;
            let kind = entry
                .file_type()
                .with_context(|| format!("reading {}", entry.path().display()))?;
            if kind.is_symlink() || is_link(&entry)? {
                continue;
            }
            if kind.is_dir() {
                pending.push(entry.path());
            } else if !attributes_writable(&entry.path())? {
                return Ok(locked(&entry.path()));
            }
        }
    }
    Ok(Need::Nothing)
}

fn locked(path: &Path) -> Need {
    tracing::info!(path = %path.display(), "Nucleus Co-op: the signed-in user may not change this file of the game, so Nucleus Co-op runs elevated");
    Need::Administrator(Why::Folder)
}

/// A junction or other reparse point, which `file_type` calls a folder.
fn is_link(entry: &std::fs::DirEntry) -> Result<bool> {
    use std::os::windows::fs::MetadataExt;
    let metadata = entry
        .metadata()
        .with_context(|| format!("reading {}", entry.path().display()))?;
    Ok(metadata.file_attributes() & 0x400 != 0)
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
        let manifest = admin_manifest(&bare, 0);
        let status = std::process::Command::new("icacls")
            .arg(&manifest)
            .args(["/inheritance:r", "/grant:r", &format!("*{sid}:F")])
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "icacls failed");
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
    fn links_in_the_game_folder_are_not_followed() {
        let elsewhere = tempfile::tempdir().unwrap();
        let locked = elsewhere.path().join("locked");
        read_only_files(&locked);
        std::fs::write(locked.join("1.map"), b"x").unwrap();
        let game = tempfile::tempdir().unwrap();
        std::fs::write(game.path().join("game.exe"), b"MZ").unwrap();
        junction(&locked, &game.path().join("data"));
        assert_eq!(folder_need(game.path(), None).unwrap(), Need::Nothing);
    }
}
