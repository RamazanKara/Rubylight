//! Nucleus Co-op on this PC: the pinned release, installed at first use below
//! the host's data folder, or the user's own copy (`nucleus_path`).
//!
//! The host's copy lives in `<data>\nucleus\<version>`, which only SYSTEM and
//! Administrators can write: Nucleus runs elevated, so a folder its user could
//! write without elevation would let any program of that user run elevated.
//! The archive is checked before a byte of it reaches the disk (size and
//! SHA-256 of the whole download, then each entry's CRC while unpacking), and
//! a refused download is dropped.
use anyhow::{Context, Result, bail, ensure};
use butterpollo_core::nucleus::install as pinned;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// How the host downloads. The host passes its own HTTPS client (the
/// updater's), so this crate carries no second HTTP stack.
pub trait Fetch {
    /// The body of `url`. A body longer than `limit` bytes is an error: the
    /// pinned archive has a known size.
    fn fetch(&mut self, url: &str, limit: u64) -> Result<Vec<u8>>;
}

/// The Nucleus folder streams use: `nucleus_path` when set, which must hold
/// NucleusCoop.exe (it is never replaced by the host's copy) and which only
/// SYSTEM and Administrators may change, else the host's copy below `data`
/// once it is installed.
pub fn installed(data: &Path, configured: &str) -> Result<Option<PathBuf>> {
    installed_checked(data, configured, administrators_only)
}

fn installed_checked(
    data: &Path,
    configured: &str,
    trusted: fn(&Path) -> Result<()>,
) -> Result<Option<PathBuf>> {
    let configured = configured.trim();
    if !configured.is_empty() {
        let folder = PathBuf::from(configured);
        ensure!(
            folder.join(pinned::EXECUTABLE).is_file(),
            "nucleus_path is {}, which holds no {}: point it at a Nucleus Co-op folder or clear it to let Rubylight install Nucleus Co-op {}",
            folder.display(),
            pinned::EXECUTABLE,
            pinned::VERSION
        );
        trusted(&folder).with_context(|| {
            format!(
                "nucleus_path is {}, which Rubylight does not run: Nucleus Co-op runs elevated, so only SYSTEM and Administrators may change its folder. Move it under Program Files or clear nucleus_path to let Rubylight install Nucleus Co-op {}",
                folder.display(),
                pinned::VERSION
            )
        })?;
        return Ok(Some(folder));
    }
    let folder = pinned::folder(data);
    Ok(folder.join(pinned::EXECUTABLE).is_file().then_some(folder))
}

/// SYSTEM's, the Administrators group's and TrustedInstaller's (what Program
/// Files holds).
const TRUSTED: [&str; 3] = [
    "S-1-5-18",
    "S-1-5-32-544",
    "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
];

/// Whether an elevated program may run from `folder`: neither it, its
/// NucleusCoop.exe nor its parent is a link, each is owned by [`TRUSTED`],
/// nobody else may write the folder or the executable, and nobody else may
/// move the folder out of its parent (the parent's own new folders are no
/// matter). Otherwise a program of an ordinary user could swap what runs
/// elevated, or point the host's settings writes elsewhere.
fn administrators_only(folder: &Path) -> Result<()> {
    use crate::process::{open_file_child_movers, open_file_owner, open_file_writers};
    let exe = folder.join(pinned::EXECUTABLE);
    let parent = folder.parent().filter(|p| !p.as_os_str().is_empty());
    let checked = [(folder, true), (exe.as_path(), false)]
        .into_iter()
        .chain(parent.map(|p| (p, true)));
    for (index, (path, is_folder)) in checked.enumerate() {
        let file = crate::process::open_unfollowed(path, is_folder)
            .with_context(|| format!("opening {}", path.display()))?;
        let owner = open_file_owner(&file)
            .with_context(|| format!("reading the owner of {}", path.display()))?;
        let others = if index < 2 {
            open_file_writers(&file)
        } else {
            open_file_child_movers(&file)
        }
        .with_context(|| format!("reading the permissions of {}", path.display()))?;
        untrusted(path, &file, &owner, &others)?;
    }
    Ok(())
}

/// [`administrators_only`]'s verdict on one path, from its owner and those
/// who may change it.
fn untrusted(path: &Path, file: &std::fs::File, owner: &str, others: &[String]) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    let attributes = file
        .metadata()
        .with_context(|| format!("reading {}", path.display()))?
        .file_attributes();
    ensure!(
        attributes & 0x400 == 0,
        "{} is a link to elsewhere",
        path.display()
    );
    verdict(path, owner, others)
}

fn verdict(path: &Path, owner: &str, others: &[String]) -> Result<()> {
    ensure!(
        TRUSTED.contains(&owner),
        "{} is owned by {owner}, not by SYSTEM or Administrators",
        path.display()
    );
    if let Some(other) = others.iter().find(|sid| !TRUSTED.contains(&sid.as_str())) {
        bail!("{} can be changed by {other}", path.display());
    }
    Ok(())
}

/// The Nucleus folder, installing the pinned release first when there is
/// none. Concurrent calls install once.
pub fn ensure(data: &Path, configured: &str, fetch: &mut dyn Fetch) -> Result<PathBuf> {
    static INSTALLING: Mutex<()> = Mutex::new(());
    let _one = INSTALLING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(folder) = installed(data, configured)? {
        return Ok(folder);
    }
    install(data, fetch, &Steps::PINNED)
}

/// The checks that differ in tests: the fake archive has another hash, and
/// the test process may not own files as Administrators.
struct Steps {
    verify: fn(&[u8]) -> std::result::Result<(), String>,
    secure: fn(&Path) -> Result<()>,
}
impl Steps {
    const PINNED: Self = Self {
        verify: pinned::verify,
        secure: crate::process::restrict_to_administrators,
    };
}

fn install(data: &Path, fetch: &mut dyn Fetch, steps: &Steps) -> Result<PathBuf> {
    let root = data.join("nucleus");
    std::fs::create_dir_all(&root).with_context(|| format!("creating {}", root.display()))?;
    // Before anything is written: what is created inside inherits it.
    (steps.secure)(&root)
        .with_context(|| format!("limiting {} to Administrators", root.display()))?;
    tracing::info!(
        version = pinned::VERSION,
        url = pinned::URL,
        "Installing Nucleus Co-op"
    );
    let archive = fetch
        .fetch(pinned::URL, pinned::SIZE)
        .with_context(|| format!("downloading Nucleus Co-op {}", pinned::VERSION))?;
    (steps.verify)(&archive).map_err(anyhow::Error::msg)?;
    let staging = root.join(format!("{}.partial", pinned::VERSION));
    remove_folder(&staging)?;
    let unpacked = unpack(&archive, &staging).and_then(|files| {
        ensure!(
            staging.join(pinned::EXECUTABLE).is_file(),
            "the Nucleus Co-op {} archive holds no {}",
            pinned::VERSION,
            pinned::EXECUTABLE
        );
        Ok(files)
    });
    let files = match unpacked {
        Ok(files) => files,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error.context(format!("unpacking Nucleus Co-op {}", pinned::VERSION)));
        }
    };
    let folder = pinned::folder(data);
    // A folder without the executable is a broken copy (`installed` said none).
    remove_folder(&folder)?;
    std::fs::rename(&staging, &folder)
        .with_context(|| format!("moving Nucleus Co-op into {}", folder.display()))?;
    tracing::info!(folder = %folder.display(), files, "Nucleus Co-op installed");
    Ok(folder)
}

fn remove_folder(folder: &Path) -> Result<()> {
    match std::fs::remove_dir_all(folder) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(error).with_context(|| format!("removing {}", folder.display()))
        }
        _ => Ok(()),
    }
}

/// Unpack the release archive into the new folder `to`; the number of files.
fn unpack(archive: &[u8], to: &Path) -> Result<usize> {
    let mut archive = zip::ZipArchive::new(Cursor::new(archive))?;
    std::fs::create_dir(to).with_context(|| format!("creating {}", to.display()))?;
    let mut files = 0;
    for index in 0..archive.len() {
        let mut entry = archive.by_index_decrypt(index, pinned::PASSWORD)?;
        let name = entry.name().to_string();
        let Some(relative) = pinned::entry_path(&name) else {
            bail!("the archive entry {name:?} would leave the Nucleus Co-op folder");
        };
        ensure!(
            !entry.is_symlink(),
            "the archive entry {name:?} is a symbolic link"
        );
        let path = to.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&path)?;
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // A second entry of the same name is refused rather than overwriting the first.
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .with_context(|| format!("creating {}", path.display()))?;
        // Reading to the end checks the entry's CRC (and the password).
        std::io::copy(&mut entry, &mut file).with_context(|| format!("unpacking {name}"))?;
        files += 1;
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::unstable::write::FileOptionsExt;
    use zip::write::SimpleFileOptions;

    struct Fake {
        archive: Result<Vec<u8>, String>,
        calls: Vec<(String, u64)>,
    }
    impl Fake {
        fn new(archive: Vec<u8>) -> Self {
            Self {
                archive: Ok(archive),
                calls: Vec::new(),
            }
        }
    }
    impl Fetch for Fake {
        fn fetch(&mut self, url: &str, limit: u64) -> Result<Vec<u8>> {
            self.calls.push((url.to_string(), limit));
            self.archive.clone().map_err(anyhow::Error::msg)
        }
    }

    fn accept(_: &[u8]) -> std::result::Result<(), String> {
        Ok(())
    }
    fn refuse(_: &[u8]) -> std::result::Result<(), String> {
        Err("The Nucleus Co-op download's SHA-256 is wrong; it was deleted.".into())
    }
    fn secure(folder: &Path) -> Result<()> {
        // Run before the unpack: nothing is staged yet.
        ensure!(
            !folder.join(format!("{}.partial", pinned::VERSION)).exists(),
            "secured too late"
        );
        std::fs::write(folder.join("secured"), b"")?;
        Ok(())
    }
    const TEST: Steps = Steps {
        verify: accept,
        secure,
    };

    /// A release-like archive: ZipCrypto entries with the published password,
    /// deflated and stored ones, a folder, and one unencrypted entry.
    fn archive(password: &[u8], extra: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let deflated = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .with_deprecated_encryption(password);
        let stored = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .with_deprecated_encryption(password);
        writer.start_file("NucleusCoop.exe", deflated).unwrap();
        writer.write_all(b"MZ nucleus").unwrap();
        writer.start_file("Settings.ini", stored).unwrap();
        writer.write_all(b"[Dev]\r\n").unwrap();
        writer
            .add_directory("handlers/", SimpleFileOptions::default())
            .unwrap();
        writer
            .start_file("utils/readme.txt", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"plain").unwrap();
        for (name, data) in extra {
            writer.start_file(*name, deflated).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn the_pinned_release_is_fetched_checked_and_unpacked() {
        let data = tempfile::tempdir().unwrap();
        let mut fake = Fake::new(archive(pinned::PASSWORD, &[]));
        let folder = install(data.path(), &mut fake, &TEST).unwrap();
        assert_eq!(folder, pinned::folder(data.path()));
        assert_eq!(fake.calls, [(pinned::URL.to_string(), pinned::SIZE)]);
        assert_eq!(
            std::fs::read(folder.join("NucleusCoop.exe")).unwrap(),
            b"MZ nucleus"
        );
        assert_eq!(
            std::fs::read(folder.join("Settings.ini")).unwrap(),
            b"[Dev]\r\n"
        );
        assert!(folder.join("handlers").is_dir());
        assert_eq!(
            std::fs::read(folder.join("utils").join("readme.txt")).unwrap(),
            b"plain"
        );
        let root = data.path().join("nucleus");
        assert!(root.join("secured").is_file());
        assert!(!root.join(format!("{}.partial", pinned::VERSION)).exists());
        assert_eq!(installed(data.path(), "").unwrap(), Some(folder));
    }

    #[test]
    fn a_refused_download_installs_nothing() {
        let data = tempfile::tempdir().unwrap();
        let mut fake = Fake::new(archive(pinned::PASSWORD, &[]));
        let steps = Steps {
            verify: refuse,
            secure,
        };
        let error = install(data.path(), &mut fake, &steps).unwrap_err();
        assert!(format!("{error:#}").contains("it was deleted"), "{error:#}");
        let root = data.path().join("nucleus");
        let left: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, ["secured"]);
        assert_eq!(installed(data.path(), "").unwrap(), None);

        let mut offline = Fake {
            archive: Err("no network".into()),
            calls: Vec::new(),
        };
        let error = install(data.path(), &mut offline, &TEST).unwrap_err();
        assert_eq!(
            format!("{error:#}"),
            format!("downloading Nucleus Co-op {}: no network", pinned::VERSION)
        );
    }

    #[test]
    fn an_entry_outside_the_folder_or_a_wrong_password_unpacks_nothing() {
        let outside: &[&str] = &["would leave the Nucleus Co-op folder"];
        // ZipCrypto's check byte lets one wrong password in 256 through; the
        // entry's CRC or its deflate stream then fails.
        let password: &[&str] = &["assword", "checksum", "deflate", "corrupt"];
        for (archive, expected) in [
            (archive(pinned::PASSWORD, &[("../evil.txt", b"x")]), outside),
            (archive(pinned::PASSWORD, &[("C:/evil.txt", b"x")]), outside),
            (archive(b"not nucleus", &[]), password),
        ] {
            let data = tempfile::tempdir().unwrap();
            let error = install(data.path(), &mut Fake::new(archive), &TEST).unwrap_err();
            let text = format!("{error:#}");
            assert!(text.starts_with("unpacking Nucleus Co-op"), "{text}");
            assert!(expected.iter().any(|part| text.contains(part)), "{text}");
            let root = data.path().join("nucleus");
            assert!(!root.join(format!("{}.partial", pinned::VERSION)).exists());
            assert!(!pinned::folder(data.path()).exists());
        }
    }

    #[test]
    fn an_archive_without_nucleus_is_refused() {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("readme.txt", SimpleFileOptions::default())
            .unwrap();
        let empty = writer.finish().unwrap().into_inner();
        let data = tempfile::tempdir().unwrap();
        let error = install(data.path(), &mut Fake::new(empty), &TEST).unwrap_err();
        assert!(
            format!("{error:#}").contains("holds no NucleusCoop.exe"),
            "{error:#}"
        );
        assert!(!pinned::folder(data.path()).exists());
    }

    #[test]
    fn a_broken_copy_is_replaced_and_a_working_one_kept() {
        let data = tempfile::tempdir().unwrap();
        let folder = pinned::folder(data.path());
        std::fs::create_dir_all(folder.join("handlers")).unwrap();
        std::fs::write(folder.join("stale.dll"), b"old").unwrap();
        assert_eq!(installed(data.path(), "").unwrap(), None);
        install(
            data.path(),
            &mut Fake::new(archive(pinned::PASSWORD, &[])),
            &TEST,
        )
        .unwrap();
        assert!(!folder.join("stale.dll").exists());
        assert!(folder.join(pinned::EXECUTABLE).is_file());
    }

    #[test]
    fn the_users_own_copy_must_hold_nucleus() {
        let data = tempfile::tempdir().unwrap();
        let own = tempfile::tempdir().unwrap();
        let path = own.path().to_str().unwrap().to_string();
        let error = installed(data.path(), &path).unwrap_err().to_string();
        assert!(error.starts_with("nucleus_path is "), "{error}");
        assert!(error.contains("holds no NucleusCoop.exe"), "{error}");
        std::fs::write(own.path().join(pinned::EXECUTABLE), b"MZ").unwrap();
        let trusted = |_: &Path| Ok(());
        assert_eq!(
            installed_checked(data.path(), &format!(" {path} "), trusted).unwrap(),
            Some(own.path().to_path_buf())
        );
    }

    #[test]
    fn the_users_own_copy_must_be_one_only_administrators_change() {
        let data = tempfile::tempdir().unwrap();
        // A temporary folder is the user's, as an extracted download is.
        let own = tempfile::tempdir().unwrap();
        std::fs::write(own.path().join(pinned::EXECUTABLE), b"MZ").unwrap();
        let path = own.path().to_str().unwrap().to_string();
        let mut fake = Fake::new(Vec::new());
        let error = format!("{:#}", ensure(data.path(), &path, &mut fake).unwrap_err());
        assert!(error.starts_with("nucleus_path is "), "{error}");
        assert!(
            error.contains("only SYSTEM and Administrators may change its folder"),
            "{error}"
        );
        // Neither used nor replaced by the host's copy: nothing is fetched.
        assert!(fake.calls.is_empty());
        let folder = own.path().join("Nucleus");
        let trusted = &["S-1-5-18".to_string(), "S-1-5-32-544".to_string()];
        assert!(verdict(&folder, "S-1-5-32-544", trusted).is_ok());
        assert!(
            verdict(
                &folder,
                "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
                &[]
            )
            .is_ok()
        );
        let user = verdict(&folder, "S-1-5-21-1-2-3-1001", &[]).unwrap_err();
        assert!(
            user.to_string().contains("is owned by S-1-5-21-1-2-3-1001"),
            "{user}"
        );
        let writer = verdict(&folder, "S-1-5-18", &["S-1-5-11".into()]).unwrap_err();
        assert!(
            writer.to_string().contains("can be changed by S-1-5-11"),
            "{writer}"
        );
        let entry = verdict(&folder, "S-1-5-18", &["ACE type 5".into()]).unwrap_err();
        assert!(entry.to_string().contains("ACE type 5"), "{entry}");
    }
}
