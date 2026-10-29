//! What a game's .exe asks of Windows to start, read from the PC, for
//! [`butterpollo_core::nucleus::elevation::choose`]: its manifest's level
//! (or Windows' installer detection without one), the compatibility
//! settings of the machine and of the signed-in user, and whether that user
//! may change the files Nucleus Co-op changes before it starts the game.
use super::layers::{self, Layers};
use anyhow::{Context, Result};
use butterpollo_core::nucleus::elevation::{self as rule, Need, Why};
use std::os::windows::io::OwnedHandle;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::FreeLibrary;
use windows::Win32::System::LibraryLoader::{
    FindResourceW, LOAD_LIBRARY_AS_DATAFILE, LOAD_LIBRARY_AS_IMAGE_RESOURCE, LoadLibraryExW,
    LoadResource, LockResource, SizeofResource,
};
use windows::core::PCWSTR;

/// What the game at `exe` needs, for the user `sid`: the most of its
/// manifest, Windows' installer detection and both compatibility settings.
pub fn game_need(exe: &Path, sid: &str) -> Result<Need> {
    let manifest = manifest_need(exe)?;
    let installer = manifest.is_none()
        && thirty_two_bit(exe)?
        && exe
            .file_name()
            .is_some_and(|name| rule::installer_name(&name.to_string_lossy()));
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

/// The level the manifest of `exe` asks for: the one it embeds, else the
/// `<name>.exe.manifest` beside it, as Windows reads them. None when it
/// names none.
pub fn manifest_need(exe: &Path) -> Result<Option<Need>> {
    let text = match embedded_manifest(exe)? {
        Some(text) => text,
        None => match std::fs::read(external_manifest(exe)) {
            Ok(bytes) => decode(&bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("reading {}", external_manifest(exe).display()));
            }
        },
    };
    Ok(rule::manifest_need(&text))
}

fn external_manifest(exe: &Path) -> PathBuf {
    let mut name = exe.as_os_str().to_owned();
    name.push(".manifest");
    PathBuf::from(name)
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
/// applies to.
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
        assert_eq!(game_need(&invoker, &sid).unwrap(), Need::Nothing);
        let setup = copy(&wow, folder.path(), "game_setup.exe", None);
        assert_eq!(manifest_need(&setup).unwrap(), None);
        assert_eq!(
            game_need(&setup, &sid).unwrap(),
            Need::Administrator(Why::Installer)
        );
        let game = copy(&wow, folder.path(), "game.exe", None);
        assert_eq!(game_need(&game, &sid).unwrap(), Need::Nothing);
        // A 64-bit one is never taken for an installer.
        let system = std::env::var("SystemRoot").unwrap();
        let native = copy(
            &Path::new(&system).join(r"System32\where.exe"),
            folder.path(),
            "update.exe",
            None,
        );
        assert_eq!(game_need(&native, &sid).unwrap(), Need::Nothing);
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
