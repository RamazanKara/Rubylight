//! A folder the PC shares with devices (`shared_folders`): listed, read and
//! written as the signed-in user, like the file transfer folder
//! ([`crate::user_folder`]), and never left. The root is resolved once when
//! the folder opens. A device's path is a list of names
//! (`butterpollo_core::file_transfer::folder_path`) opened below it, and what
//! Windows opened must be exactly that path below that root: a symbolic link,
//! junction or mount point on the way resolves elsewhere, and an 8.3 short
//! name to its long name, so both are refused. Links are never listed.
//!
//! Every call blocks and impersonates on the calling thread: run them on a
//! blocking thread, not an async worker.
use crate::user_folder::{
    Impersonation, PartFile, UserFolder, UserFolderError, final_path, plain, reopen_for_data,
    same_name, user_token,
};
use butterpollo_core::file_transfer;
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io,
    os::windows::{fs::OpenOptionsExt, io::OwnedHandle},
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
};

/// A file or folder in a shared folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderEntry {
    pub name: String,
    pub dir: bool,
    /// Bytes; 0 for a folder.
    pub size: u64,
    pub modified_unix_ms: u64,
}

/// A shared folder's root and the user it is read and written as.
#[derive(Clone, Debug)]
pub struct SharedFolder {
    /// The root as Windows resolved it, with the `\\?\` prefix.
    root: PathBuf,
    token: Option<Arc<OwnedHandle>>,
}
impl SharedFolder {
    /// The shared folder at `root`, which must be a folder already: it is
    /// never created. Open it for each request, since the signed-in user can
    /// change.
    pub fn open(root: &Path) -> Result<Self, UserFolderError> {
        Self::at(root, user_token()?.map(Arc::new))
    }

    /// The folder at `root`, opened as this process rather than as the
    /// signed-in user. Only for tests of the callers (the `test-folder`
    /// feature), as [`UserFolder::unimpersonated`].
    #[cfg(feature = "test-folder")]
    pub fn unimpersonated(root: &Path) -> Result<Self, UserFolderError> {
        Self::at(root, None)
    }

    fn at(root: &Path, token: Option<Arc<OwnedHandle>>) -> Result<Self, UserFolderError> {
        if !root.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("the shared folder {} is not absolute", root.display()),
            )
            .into());
        }
        // The root itself may be a link the administrator chose: the folder
        // it leads to is the root.
        let handle = as_user(token.as_deref(), || open_dir(root, 0))?;
        if !handle.metadata()?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("the shared folder {} is not a folder", root.display()),
            )
            .into());
        }
        let resolved = PathBuf::from(final_path(&handle)?);
        if resolved.to_str().is_none_or(|path| {
            path.get(..8)
                .is_some_and(|p| p.eq_ignore_ascii_case(r"\\?\UNC\"))
        }) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "the shared folder {} leads to a network share",
                    root.display()
                ),
            )
            .into());
        }
        Ok(Self {
            root: resolved,
            token,
        })
    }

    /// The root as Windows resolved it.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn as_user<T>(&self, work: impl FnOnce() -> io::Result<T>) -> Result<T, UserFolderError> {
        as_user(self.token.as_deref(), work)
    }

    /// `names` below the root, each checked again here.
    fn path(&self, names: &[&str]) -> Result<PathBuf, UserFolderError> {
        let mut path = self.root.clone();
        for name in names {
            file_transfer::check_folder_name(name)?;
            path.push(name);
        }
        Ok(path)
    }

    /// Fails unless Windows opened `file` at exactly `path`, in any case.
    fn confined(&self, file: &File, path: &Path) -> Result<(), UserFolderError> {
        let actual = final_path(file)?;
        if same_name(&actual, path.as_os_str()) {
            Ok(())
        } else {
            tracing::warn!(
                "Shared folders refused {}: Windows opened {}",
                path.display(),
                actual.display()
            );
            Err(UserFolderError::OutsideFolder)
        }
    }

    /// The folder `names` below the root, held open without delete sharing
    /// so it can't be moved or swapped while the handle lives.
    fn dir(&self, names: &[&str]) -> Result<(File, PathBuf), UserFolderError> {
        let path = self.path(names)?;
        let dir = self.as_user(|| open_dir(&path, FILE_FLAG_OPEN_REPARSE_POINT.0))?;
        let metadata = dir.metadata()?;
        if metadata.is_symlink() {
            return Err(UserFolderError::OutsideFolder);
        }
        if !metadata.is_dir() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "not a folder").into());
        }
        self.confined(&dir, &path)?;
        Ok((dir, path))
    }

    /// The folders and plain files in the folder `names` below the root,
    /// folders first, then by name. Links, uploads in progress (`.part`) and
    /// names a device couldn't ask for are left out.
    pub fn entries(&self, names: &[&str]) -> Result<Vec<FolderEntry>, UserFolderError> {
        let (dir, path) = self.dir(names)?;
        let mut entries = self.as_user(|| {
            let mut entries = Vec::new();
            for entry in fs::read_dir(&path)? {
                let entry = entry?;
                // From the directory listing itself: links are not followed.
                let metadata = entry.metadata()?;
                let is_dir = metadata.is_dir();
                if !is_dir && !plain(&metadata) {
                    continue;
                }
                let Ok(name) = entry.file_name().into_string() else {
                    continue;
                };
                if file_transfer::check_folder_name(&name).is_err() {
                    continue;
                }
                entries.push(FolderEntry {
                    name,
                    dir: is_dir,
                    size: if is_dir { 0 } else { metadata.len() },
                    modified_unix_ms: modified(&metadata)?,
                });
            }
            Ok(entries)
        })?;
        drop(dir);
        entries.sort_by(|a, b| {
            b.dir
                .cmp(&a.dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(entries)
    }

    /// Opens the plain file `names` below the root for reading, with its
    /// length.
    pub fn open_file(&self, names: &[&str]) -> Result<(File, u64), UserFolderError> {
        if names.is_empty() {
            return Err(UserFolderError::NotPlainFile);
        }
        let path = self.path(names)?;
        // Backup semantics let a folder open too, so it is refused below
        // like a link rather than failing with access denied.
        let file = self.as_user(|| {
            OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ.0)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0 | FILE_FLAG_BACKUP_SEMANTICS.0)
                .open(&path)
        })?;
        if !plain(&file.metadata()?) {
            return Err(UserFolderError::NotPlainFile);
        }
        self.confined(&file, &path)?;
        reopen_for_data(self.token.as_deref(), &file)
    }

    /// The folder `names` below the root as the folder an upload goes to.
    /// It stays held open while the returned folder, or an upload it
    /// started, lives.
    pub fn upload_folder(&self, names: &[&str]) -> Result<UserFolder, UserFolderError> {
        let (dir, path) = self.dir(names)?;
        UserFolder::pinned(path, self.token.clone(), dir)
    }

    /// Starts an upload of `name` into `folder`, from
    /// [`SharedFolder::upload_folder`], as [`UserFolder::create_part`] does,
    /// and checks that its `.part` file is where it was asked for.
    pub fn create_part(
        &self,
        folder: &UserFolder,
        name: &str,
    ) -> Result<PartFile, UserFolderError> {
        let part = folder.create_part(name)?;
        if let (Some(file), Some(path)) = (part.file(), part.part_path()) {
            self.confined(file, path)?;
        }
        Ok(part)
    }
}

fn as_user<T>(
    token: Option<&OwnedHandle>,
    work: impl FnOnce() -> io::Result<T>,
) -> Result<T, UserFolderError> {
    let _user = Impersonation::begin(token)?;
    Ok(work()?)
}

/// Opens the folder at `path` without delete sharing, with `flags`.
fn open_dir(path: &Path, flags: u32) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | flags)
        .open(path)
}

fn modified(metadata: &Metadata) -> io::Result<u64> {
    Ok(metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use windows::{Win32::Storage::FileSystem::GetShortPathNameW, core::HSTRING};

    /// A shared folder at `root` in a new temporary folder, beside `outside`.
    struct Rig {
        _temp: tempfile::TempDir,
        root: PathBuf,
        outside: PathBuf,
        folder: SharedFolder,
    }
    fn rig() -> Rig {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("shared");
        let outside = temp.path().join("private");
        fs::create_dir_all(root.join("saves")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), b"secret").unwrap();
        fs::write(root.join("saves").join("slot1.sav"), b"slot").unwrap();
        fs::write(root.join("readme.txt"), b"hello").unwrap();
        let folder = SharedFolder::at(&root, None).unwrap();
        Rig {
            _temp: temp,
            root,
            outside,
            folder,
        }
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

    /// A symbolic link, when this process may create one (elevated or
    /// Developer Mode); `None` otherwise.
    fn symlink(target: &Path, link: &Path, dir: bool) -> Option<()> {
        let made = if dir {
            std::os::windows::fs::symlink_dir(target, link)
        } else {
            std::os::windows::fs::symlink_file(target, link)
        };
        match made {
            Ok(()) => Some(()),
            // ERROR_PRIVILEGE_NOT_HELD
            Err(error) if error.raw_os_error() == Some(1314) => None,
            Err(error) => panic!("symlink: {error}"),
        }
    }

    /// The 8.3 short name of the file at `path`, when the volume makes one.
    fn short_name(path: &Path) -> Option<String> {
        let long = HSTRING::from(path);
        let mut short = vec![0u16; 1024];
        // SAFETY: long is NUL-terminated and short a writable buffer.
        let len = unsafe { GetShortPathNameW(&long, Some(&mut short)) } as usize;
        assert!(len > 0 && len < short.len(), "{path:?}");
        let short = String::from_utf16(&short[..len]).unwrap();
        let short = short.rsplit('\\').next().unwrap().to_owned();
        (short != path.file_name().unwrap().to_str().unwrap()).then_some(short)
    }

    fn read(folder: &SharedFolder, names: &[&str]) -> Result<Vec<u8>, UserFolderError> {
        let (mut file, len) = folder.open_file(names)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes.len() as u64, len);
        Ok(bytes)
    }

    fn outside(result: Result<impl std::fmt::Debug, UserFolderError>) {
        assert!(
            matches!(result, Err(UserFolderError::OutsideFolder)),
            "{result:?}"
        );
    }

    #[test]
    fn folders_come_first_and_links_and_uploads_in_progress_are_not_listed() {
        let rig = rig();
        fs::write(rig.root.join("b.txt"), b"bb").unwrap();
        fs::write(rig.root.join("movie.mp4.part"), b"half").unwrap();
        fs::create_dir(rig.root.join("Alpha")).unwrap();
        junction(&rig.outside, &rig.root.join("to private"));
        symlink(
            &rig.outside.join("secret.txt"),
            &rig.root.join("secret.txt"),
            false,
        );
        let entries = rig.folder.entries(&[]).unwrap();
        let listed: Vec<(&str, bool, u64)> = entries
            .iter()
            .map(|e| (e.name.as_str(), e.dir, e.size))
            .collect();
        assert_eq!(
            listed,
            [
                ("Alpha", true, 0),
                ("saves", true, 0),
                ("b.txt", false, 2),
                ("readme.txt", false, 5)
            ]
        );
        assert!(entries.iter().all(|e| e.modified_unix_ms > 0));
        let saves = rig.folder.entries(&["saves"]).unwrap();
        assert_eq!(saves[0].name, "slot1.sav");
        // A name in another case is the same folder.
        assert_eq!(rig.folder.entries(&["SAVES"]).unwrap(), saves);
    }

    #[test]
    fn files_are_read_below_the_root_only() {
        let rig = rig();
        assert_eq!(read(&rig.folder, &["saves", "slot1.sav"]).unwrap(), b"slot");
        assert_eq!(read(&rig.folder, &["README.TXT"]).unwrap(), b"hello");
        assert!(matches!(
            read(&rig.folder, &["saves"]),
            Err(UserFolderError::NotPlainFile)
        ));
        assert!(matches!(
            read(&rig.folder, &[]),
            Err(UserFolderError::NotPlainFile)
        ));
        assert!(matches!(
            read(&rig.folder, &["missing.txt"]),
            Err(UserFolderError::Io(error)) if error.kind() == io::ErrorKind::NotFound
        ));
        // Names that would leave the root are refused before Windows sees them.
        for names in [
            &["..", "private", "secret.txt"][..],
            &["saves", "..", "..", "private", "secret.txt"],
            &["readme.txt:hidden"],
            &["readme.txt::$DATA"],
            &["CON"],
            &["nul.txt"],
            &["saves\\..\\..\\private\\secret.txt"],
            &["."],
        ] {
            assert!(
                matches!(
                    rig.folder.open_file(names),
                    Err(UserFolderError::InvalidName(_))
                ),
                "{names:?}"
            );
            assert!(
                matches!(
                    rig.folder.entries(names),
                    Err(UserFolderError::InvalidName(_))
                ),
                "{names:?}"
            );
        }
    }

    #[test]
    fn a_junction_or_symlink_out_of_the_root_is_refused_every_way() {
        let rig = rig();
        junction(&rig.outside, &rig.root.join("j"));
        outside(rig.folder.entries(&["j"]));
        outside(rig.folder.open_file(&["j", "secret.txt"]));
        outside(rig.folder.upload_folder(&["j"]));
        // One back into the root is a link too.
        junction(&rig.root.join("saves"), &rig.root.join("again"));
        outside(rig.folder.entries(&["again"]));
        outside(rig.folder.open_file(&["again", "slot1.sav"]));
        if symlink(&rig.outside, &rig.root.join("d"), true).is_some() {
            outside(rig.folder.entries(&["d"]));
            outside(rig.folder.open_file(&["d", "secret.txt"]));
        }
        if symlink(
            &rig.outside.join("secret.txt"),
            &rig.root.join("s.txt"),
            false,
        )
        .is_some()
        {
            assert!(matches!(
                rig.folder.open_file(&["s.txt"]),
                Err(UserFolderError::NotPlainFile)
            ));
        }
        assert_eq!(fs::read(rig.outside.join("secret.txt")).unwrap(), b"secret");
    }

    #[test]
    fn a_junction_to_a_name_that_only_lowercases_alike_is_refused() {
        let rig = rig();
        // A folder named with the Kelvin sign, which NTFS keeps apart from
        // "k", and a junction "kelvin" to it.
        let kelvin = rig.root.join("\u{212a}elvin");
        fs::create_dir(&kelvin).unwrap();
        fs::write(kelvin.join("secret.txt"), b"k").unwrap();
        junction(&kelvin, &rig.root.join("kelvin"));
        outside(rig.folder.open_file(&["kelvin", "secret.txt"]));
        outside(rig.folder.entries(&["kelvin"]));
        assert_eq!(
            read(&rig.folder, &["\u{212a}elvin", "secret.txt"]).unwrap(),
            b"k"
        );
    }

    #[test]
    fn a_short_name_is_not_another_way_in() {
        let rig = rig();
        let long = rig.root.join("a long folder name");
        fs::create_dir(&long).unwrap();
        fs::write(long.join("a long file name.txt"), b"x").unwrap();
        // Volumes without 8.3 names have no other name to try.
        if let Some(short) = short_name(&long) {
            outside(rig.folder.entries(&[&short]));
            outside(rig.folder.open_file(&[&short, "a long file name.txt"]));
        }
        if let Some(short) = short_name(&long.join("a long file name.txt")) {
            outside(rig.folder.open_file(&["a long folder name", &short]));
        }
        assert_eq!(
            read(&rig.folder, &["a long folder name", "a long file name.txt"]).unwrap(),
            b"x"
        );
    }

    #[test]
    fn a_root_that_is_a_junction_is_the_folder_it_leads_to() {
        let rig = rig();
        let link = rig._temp.path().join("link to shared");
        junction(&rig.root, &link);
        let folder = SharedFolder::at(&link, None).unwrap();
        assert_eq!(
            folder.root().to_str().unwrap().to_lowercase(),
            rig.folder.root().to_str().unwrap().to_lowercase()
        );
        assert_eq!(read(&folder, &["readme.txt"]).unwrap(), b"hello");
    }

    #[test]
    fn a_missing_root_or_a_file_is_no_folder_and_nothing_is_created() {
        let rig = rig();
        let missing = rig._temp.path().join("gone");
        assert!(matches!(
            SharedFolder::at(&missing, None),
            Err(UserFolderError::Io(error)) if error.kind() == io::ErrorKind::NotFound
        ));
        assert!(!missing.exists());
        assert!(SharedFolder::at(&rig.root.join("readme.txt"), None).is_err());
        assert!(SharedFolder::at(Path::new("shared"), None).is_err());
    }

    #[test]
    fn an_upload_lands_in_its_folder_which_stays_put_until_it_ends() {
        let rig = rig();
        let folder = rig.folder.upload_folder(&["saves"]).unwrap();
        let mut part = rig.folder.create_part(&folder, "slot1.sav").unwrap();
        part.write_all(b"new").unwrap();
        // Held open: the folder can't be renamed away under the upload.
        assert!(fs::rename(rig.root.join("saves"), rig.root.join("moved")).is_err());
        drop(folder);
        assert_eq!(part.finish().unwrap(), "slot1 (1).sav");
        assert_eq!(
            fs::read(rig.root.join("saves").join("slot1 (1).sav")).unwrap(),
            b"new"
        );
        assert_eq!(
            fs::read(rig.root.join("saves").join("slot1.sav")).unwrap(),
            b"slot"
        );
        // Released with the upload.
        fs::rename(rig.root.join("saves"), rig.root.join("moved")).unwrap();
        // Uploads take the names file transfer takes.
        let root = rig.folder.upload_folder(&[]).unwrap();
        assert!(matches!(
            rig.folder.create_part(&root, "CON.txt"),
            Err(UserFolderError::InvalidName(_))
        ));
        assert!(matches!(
            rig.folder.create_part(&root, "x.part"),
            Err(UserFolderError::InvalidName(_))
        ));
        assert!(matches!(
            rig.folder.upload_folder(&["readme.txt"]),
            Err(UserFolderError::Io(error)) if error.kind() == io::ErrorKind::NotFound
        ));
    }
}
