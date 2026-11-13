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
        local_root(root, &resolved)?;
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
        Ok(UserFolder::pinned(path, self.token.clone(), dir))
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

/// Fails unless `resolved`, where Windows resolved the shared folder `root`,
/// is a UTF-8 path on this PC rather than a network share.
fn local_root(root: &Path, resolved: &Path) -> io::Result<()> {
    let Some(path) = resolved.to_str() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "the shared folder {} resolves to {}, which is not valid Unicode",
                root.display(),
                resolved.display()
            ),
        ));
    };
    if path
        .get(..8)
        .is_some_and(|p| p.eq_ignore_ascii_case(r"\\?\UNC\"))
    {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "the shared folder {} leads to the network share {path}",
                root.display()
            ),
        ));
    }
    Ok(())
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
mod tests;
