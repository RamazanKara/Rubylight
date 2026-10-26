//! The folder file transfers land in, opened as the signed-in user: a host
//! running as SYSTEM must not create files the user can't touch, nor reach
//! files the user can't. Handles are opened while impersonating the user's
//! token, so Windows checks the user's rights, and then read and written
//! without it. Any other host already runs as the user.
//!
//! Every call blocks and impersonates on the calling thread: run them on a
//! blocking thread, not an async worker.
use butterpollo_core::file_transfer::{self, NameError};
use std::{
    ffi::OsString,
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Write},
    marker::PhantomData,
    os::windows::{
        ffi::OsStringExt,
        fs::{MetadataExt, OpenOptionsExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};
use windows::{
    Win32::{
        Foundation::{ERROR_NO_TOKEN, HANDLE},
        Security::{ImpersonateLoggedOnUser, RevertToSelf},
        Storage::FileSystem::{
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_SHARE_READ, GetDiskFreeSpaceExW, MOVE_FILE_FLAGS, MoveFileExW,
        },
        System::{Com::CoTaskMemFree, RemoteDesktop::WTSQueryUserToken},
        UI::Shell::{FOLDERID_Downloads, KF_FLAG_DEFAULT, SHGetKnownFolderPath},
    },
    core::{HRESULT, HSTRING},
};

/// The subfolder of the user's Downloads folder transfers use by default.
pub const DEFAULT_SUBFOLDER: &str = "Rubylight";
/// How many numbered names an upload tries before it gives up.
const MAX_COLLISIONS: u32 = 9999;

#[derive(Debug)]
pub enum UserFolderError {
    /// Nobody is signed in to the host's Windows session, so there is no
    /// user to write as.
    NoConsoleUser,
    InvalidName(NameError),
    /// The name is a folder, a link or another reparse point.
    NotPlainFile,
    Io(io::Error),
}
impl std::fmt::Display for UserFolderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoConsoleUser => f.write_str("nobody is signed in to Windows"),
            Self::InvalidName(error) => error.fmt(f),
            Self::NotPlainFile => f.write_str("not a plain file"),
            Self::Io(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for UserFolderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidName(error) => Some(error),
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}
impl From<io::Error> for UserFolderError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<NameError> for UserFolderError {
    fn from(error: NameError) -> Self {
        Self::InvalidName(error)
    }
}

/// A Win32 failure as the `io::Error` std would report, so callers can
/// match its kind (`StorageFull`, `AlreadyExists`, ...).
fn os_error(error: windows::core::Error) -> io::Error {
    let code = error.code().0 as u32;
    if code & 0xFFFF_0000 == 0x8007_0000 {
        io::Error::from_raw_os_error((code & 0xFFFF) as i32)
    } else {
        io::Error::other(error)
    }
}

/// This thread runs as the user until the guard drops, also when the work
/// fails or panics. Not `Send`: the impersonation belongs to this thread.
struct Impersonation(PhantomData<*const ()>);
impl Impersonation {
    fn begin(token: Option<&OwnedHandle>) -> io::Result<Option<Self>> {
        let Some(token) = token else { return Ok(None) };
        // SAFETY: token is an open primary token of the signed-in user; the
        // returned guard reverts the impersonation on this thread.
        unsafe { ImpersonateLoggedOnUser(HANDLE(token.as_raw_handle())) }.map_err(os_error)?;
        Ok(Some(Self(PhantomData)))
    }
}
impl Drop for Impersonation {
    fn drop(&mut self) {
        // SAFETY: ends the impersonation `begin` started on this thread.
        if let Err(error) = unsafe { RevertToSelf() } {
            tracing::error!("File transfer could not stop acting as the Windows user: {error}");
        }
    }
}

/// The user signed in to the host's session, when the host runs as SYSTEM.
fn user_token() -> Result<Option<OwnedHandle>, UserFolderError> {
    if !crate::process::is_system() {
        return Ok(None);
    }
    let session = crate::process::current_session()
        .map_err(|error| io::Error::other(format!("{error:#}")))?;
    let mut token = HANDLE::default();
    // SAFETY: token is a writable handle slot.
    let queried = unsafe { WTSQueryUserToken(session, &mut token) };
    match queried {
        // SAFETY: WTS returned an open token that only this OwnedHandle closes.
        Ok(()) => Ok(Some(unsafe { OwnedHandle::from_raw_handle(token.0) })),
        Err(error) if error.code() == HRESULT::from_win32(ERROR_NO_TOKEN.0) => {
            Err(UserFolderError::NoConsoleUser)
        }
        Err(error) => Err(os_error(error).into()),
    }
}

/// The user's Downloads folder, wherever they moved it.
fn downloads(token: Option<&OwnedHandle>) -> Result<PathBuf, UserFolderError> {
    let token = token.map(|token| HANDLE(token.as_raw_handle()));
    // SAFETY: the token stays open for the call; the string Windows returns
    // is copied, then freed once.
    unsafe {
        let path =
            SHGetKnownFolderPath(&FOLDERID_Downloads, KF_FLAG_DEFAULT, token).map_err(os_error)?;
        let copy = OsString::from_wide(path.as_wide());
        CoTaskMemFree(Some(path.0 as *const _));
        Ok(PathBuf::from(copy))
    }
}

/// A plain file: not a folder, a link or any other reparse point.
fn plain(metadata: &Metadata) -> bool {
    metadata.is_file() && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0
}

/// A file in the transfer folder.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct FileEntry {
    pub name: String,
    pub size: u64,
    pub modified_unix_ms: u64,
}

/// The transfer folder and the user it belongs to.
#[derive(Clone, Debug)]
pub struct UserFolder {
    path: PathBuf,
    token: Option<Arc<OwnedHandle>>,
}
impl UserFolder {
    /// The transfer folder, created if missing: `custom` when the host's
    /// settings name one, otherwise the user's `Downloads\Rubylight`. Open
    /// it for each transfer, since the signed-in user can change.
    pub fn open(custom: Option<&Path>) -> Result<Self, UserFolderError> {
        let token = user_token()?.map(Arc::new);
        let path = match custom {
            Some(path) if path.is_absolute() => path.to_path_buf(),
            Some(path) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "the file transfer folder {} is not absolute",
                        path.display()
                    ),
                )
                .into());
            }
            None => downloads(token.as_deref())?.join(DEFAULT_SUBFOLDER),
        };
        let folder = Self { path, token };
        folder.as_user(|| fs::create_dir_all(&folder.path))?;
        Ok(folder)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn as_user<T>(&self, work: impl FnOnce() -> io::Result<T>) -> Result<T, UserFolderError> {
        let _user = Impersonation::begin(self.token.as_deref())?;
        Ok(work()?)
    }

    /// Starts an upload of `name`: a new `.part` file under the first
    /// numbered name whose `.part` is free, so neither an upload in flight
    /// nor a leftover `.part` is overwritten.
    pub fn create_part(&self, name: &str) -> Result<PartFile, UserFolderError> {
        file_transfer::check_name(name)?;
        let (file, part) = self.as_user(|| {
            for n in 0..=MAX_COLLISIONS {
                let part =
                    self.path
                        .join(file_transfer::part_name(&file_transfer::collision_name(
                            name, n,
                        )));
                // CREATE_NEW never follows a link at that name: it fails.
                match OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .share_mode(FILE_SHARE_READ.0)
                    .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
                    .open(&part)
                {
                    Ok(file) => return Ok((file, part)),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("every numbered name of {name} is taken"),
            ))
        })?;
        Ok(PartFile {
            file: Some(file),
            part: Some(part),
            name: name.to_owned(),
            folder: self.clone(),
        })
    }

    /// Opens a plain file in the folder for reading, with its length.
    pub fn open_file(&self, name: &str) -> Result<(File, u64), UserFolderError> {
        file_transfer::check_name(name)?;
        let path = self.path.join(name);
        // Backup semantics let a folder open too, so it is refused below
        // like a link rather than failing with access denied.
        let file = self.as_user(|| {
            OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ.0)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0 | FILE_FLAG_BACKUP_SEMANTICS.0)
                .open(&path)
        })?;
        let metadata = file.metadata()?;
        if !plain(&metadata) {
            return Err(UserFolderError::NotPlainFile);
        }
        Ok((file, metadata.len()))
    }

    /// The plain files in the folder, by name. Subfolders, links, uploads
    /// in progress (`.part`) and names a client couldn't ask for are left
    /// out.
    pub fn list(&self) -> Result<Vec<FileEntry>, UserFolderError> {
        self.as_user(|| {
            let mut files = Vec::new();
            for entry in fs::read_dir(&self.path)? {
                let entry = entry?;
                // From the directory listing itself: links are not followed.
                let metadata = entry.metadata()?;
                if !plain(&metadata) {
                    continue;
                }
                let Ok(name) = entry.file_name().into_string() else {
                    continue;
                };
                if name.ends_with(file_transfer::PART_SUFFIX)
                    || file_transfer::check_name(&name).is_err()
                {
                    continue;
                }
                let modified_unix_ms = metadata
                    .modified()?
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |since| {
                        u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
                    });
                files.push(FileEntry {
                    name,
                    size: metadata.len(),
                    modified_unix_ms,
                });
            }
            files.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(files)
        })
    }

    /// The bytes the user may still write to the folder's volume, after
    /// any disk quota.
    pub fn free_bytes(&self) -> Result<u64, UserFolderError> {
        let path = HSTRING::from(self.path.as_path());
        self.as_user(|| {
            let mut free = 0u64;
            // SAFETY: path is NUL-terminated and outlives the call; free is a
            // writable u64.
            unsafe { GetDiskFreeSpaceExW(&path, Some(&mut free), None, None) }.map_err(os_error)?;
            Ok(free)
        })
    }
}

/// An upload in progress, written to its `.part` file through a handle
/// opened as the user. Dropping it before [`PartFile::finish`] deletes the
/// `.part` file; [`PartFile::discard`] does the same and reports a failure.
#[derive(Debug)]
pub struct PartFile {
    file: Option<File>,
    part: Option<PathBuf>,
    name: String,
    folder: UserFolder,
}
impl PartFile {
    pub fn part_path(&self) -> Option<&Path> {
        self.part.as_deref()
    }

    /// Closes the upload and gives it the first free numbered name, which
    /// is returned. If the rename fails the `.part` file is deleted.
    pub fn finish(mut self) -> Result<String, UserFolderError> {
        let Some(mut file) = self.file.take() else {
            return Err(io::Error::other("the upload is already closed").into());
        };
        file.flush()?;
        drop(file);
        let part = HSTRING::from(self.part.as_deref().unwrap_or(Path::new("")));
        let name = self.folder.as_user(|| {
            for n in 0..=MAX_COLLISIONS {
                let candidate = file_transfer::collision_name(&self.name, n);
                let target = HSTRING::from(self.folder.path.join(&candidate).as_path());
                // SAFETY: both paths are NUL-terminated and outlive the call.
                // Without MOVEFILE_REPLACE_EXISTING an existing file stays.
                match unsafe { MoveFileExW(&part, &target, MOVE_FILE_FLAGS(0)) }.map_err(os_error) {
                    Ok(()) => return Ok(candidate),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("every numbered name of {} is taken", self.name),
            ))
        })?;
        self.part = None;
        Ok(name)
    }

    /// Ends an incomplete upload by deleting its `.part` file.
    pub fn discard(mut self) -> Result<(), UserFolderError> {
        self.remove()
    }

    fn remove(&mut self) -> Result<(), UserFolderError> {
        drop(self.file.take());
        let Some(part) = self.part.take() else {
            return Ok(());
        };
        self.folder.as_user(|| fs::remove_file(&part))
    }
}
impl Write for PartFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match &mut self.file {
            Some(file) => file.write(bytes),
            None => Err(io::Error::other("the upload is closed")),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match &mut self.file {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}
impl Drop for PartFile {
    fn drop(&mut self) {
        if let Err(error) = self.remove() {
            tracing::warn!(
                "File transfer could not delete the incomplete upload of {}: {error}",
                self.name
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::{
        Foundation::CloseHandle,
        Security::{TOKEN_DUPLICATE, TOKEN_IMPERSONATE, TOKEN_QUERY},
        System::Threading::{
            GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken,
        },
    };

    fn folder() -> (tempfile::TempDir, UserFolder) {
        let dir = tempfile::tempdir().unwrap();
        let folder = UserFolder::open(Some(dir.path())).unwrap();
        (dir, folder)
    }

    fn upload(folder: &UserFolder, name: &str, bytes: &[u8]) -> String {
        let mut part = folder.create_part(name).unwrap();
        part.write_all(bytes).unwrap();
        part.finish().unwrap()
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    /// A file symlink, when this process may create one (elevated or
    /// Developer Mode); `None` otherwise.
    fn file_symlink(target: &Path, link: &Path) -> Option<()> {
        match std::os::windows::fs::symlink_file(target, link) {
            Ok(()) => Some(()),
            // ERROR_PRIVILEGE_NOT_HELD
            Err(error) if error.raw_os_error() == Some(1314) => None,
            Err(error) => panic!("symlink: {error}"),
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

    #[test]
    fn an_upload_is_written_to_part_then_renamed() {
        let (dir, folder) = folder();
        let mut part = folder.create_part("photo.jpg").unwrap();
        assert_eq!(
            part.part_path(),
            Some(dir.path().join("photo.jpg.part").as_path())
        );
        part.write_all(b"jpeg bytes").unwrap();
        assert_eq!(names(dir.path()), ["photo.jpg.part"]);
        assert_eq!(part.finish().unwrap(), "photo.jpg");
        assert_eq!(names(dir.path()), ["photo.jpg"]);
        assert_eq!(
            fs::read(dir.path().join("photo.jpg")).unwrap(),
            b"jpeg bytes"
        );
    }

    #[test]
    fn a_taken_name_gets_the_next_number() {
        let (dir, folder) = folder();
        fs::write(dir.path().join("a.txt"), "old").unwrap();
        fs::write(dir.path().join("a (1).txt"), "older").unwrap();
        assert_eq!(upload(&folder, "a.txt", b"new"), "a (2).txt");
        assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"old");
        assert_eq!(fs::read(dir.path().join("a (1).txt")).unwrap(), b"older");
        assert_eq!(fs::read(dir.path().join("a (2).txt")).unwrap(), b"new");
        // A folder takes a name too.
        fs::create_dir(dir.path().join("b")).unwrap();
        assert_eq!(upload(&folder, "b", b"x"), "b (1)");
    }

    #[test]
    fn a_name_taken_during_the_upload_is_not_overwritten() {
        let (dir, folder) = folder();
        let mut part = folder.create_part("b.txt").unwrap();
        part.write_all(b"upload").unwrap();
        fs::write(dir.path().join("b.txt"), "meanwhile").unwrap();
        assert_eq!(part.finish().unwrap(), "b (1).txt");
        assert_eq!(fs::read(dir.path().join("b.txt")).unwrap(), b"meanwhile");
        assert_eq!(fs::read(dir.path().join("b (1).txt")).unwrap(), b"upload");
    }

    #[test]
    fn uploads_of_one_name_in_flight_get_their_own_part_files() {
        let (dir, folder) = folder();
        // A leftover from a host that stopped mid-upload stays untouched.
        fs::write(dir.path().join("c.txt.part"), "stale").unwrap();
        let mut first = folder.create_part("c.txt").unwrap();
        let mut second = folder.create_part("c.txt").unwrap();
        assert_eq!(
            names(dir.path()),
            ["c (1).txt.part", "c (2).txt.part", "c.txt.part"]
        );
        first.write_all(b"first").unwrap();
        second.write_all(b"second").unwrap();
        assert_eq!(second.finish().unwrap(), "c.txt");
        assert_eq!(first.finish().unwrap(), "c (1).txt");
        assert_eq!(fs::read(dir.path().join("c.txt")).unwrap(), b"second");
        assert_eq!(fs::read(dir.path().join("c (1).txt")).unwrap(), b"first");
        assert_eq!(fs::read(dir.path().join("c.txt.part")).unwrap(), b"stale");
    }

    #[test]
    fn an_incomplete_upload_leaves_nothing_behind() {
        let (dir, folder) = folder();
        let mut part = folder.create_part("d.bin").unwrap();
        part.write_all(&[7; 4096]).unwrap();
        part.discard().unwrap();
        assert!(names(dir.path()).is_empty());
        // Dropped without finishing, as when the request fails part way.
        let mut part = folder.create_part("d.bin").unwrap();
        part.write_all(&[7; 4096]).unwrap();
        drop(part);
        assert!(names(dir.path()).is_empty());
    }

    #[test]
    fn invalid_names_create_nothing_and_open_nothing() {
        let (dir, folder) = folder();
        fs::write(dir.path().join("secret"), "x").unwrap();
        for name in ["..", "..\\secret", "x:y", "CON.txt", "a.", ""] {
            assert!(
                matches!(
                    folder.create_part(name),
                    Err(UserFolderError::InvalidName(_))
                ),
                "{name:?}"
            );
            assert!(
                matches!(folder.open_file(name), Err(UserFolderError::InvalidName(_))),
                "{name:?}"
            );
        }
        assert_eq!(names(dir.path()), ["secret"]);
    }

    #[test]
    fn only_plain_files_are_listed_and_opened() {
        let (dir, folder) = folder();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("elsewhere.txt"), "outside").unwrap();
        fs::write(dir.path().join("b.txt"), "bb").unwrap();
        fs::write(dir.path().join("a.txt"), "a").unwrap();
        fs::write(dir.path().join("e.txt.part"), "partial").unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        junction(outside.path(), &dir.path().join("junction"));
        let linked = file_symlink(
            &outside.path().join("elsewhere.txt"),
            &dir.path().join("link.txt"),
        );

        let files = folder.list().unwrap();
        let listed: Vec<_> = files.iter().map(|f| (f.name.as_str(), f.size)).collect();
        assert_eq!(listed, [("a.txt", 1), ("b.txt", 2)]);
        let now_ms = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        for file in &files {
            assert!(file.modified_unix_ms > now_ms - 600_000, "{file:?}");
            assert!(file.modified_unix_ms <= now_ms + 1000, "{file:?}");
        }

        let (mut file, len) = folder.open_file("b.txt").unwrap();
        let mut bytes = Vec::new();
        io::Read::read_to_end(&mut file, &mut bytes).unwrap();
        assert_eq!((bytes.as_slice(), len), (b"bb".as_slice(), 2));
        for name in ["sub", "junction"] {
            assert!(
                matches!(folder.open_file(name), Err(UserFolderError::NotPlainFile)),
                "{name}"
            );
        }
        if linked.is_some() {
            assert!(matches!(
                folder.open_file("link.txt"),
                Err(UserFolderError::NotPlainFile)
            ));
        }
        assert!(matches!(
            folder.open_file("missing.txt"),
            Err(UserFolderError::Io(error)) if error.kind() == io::ErrorKind::NotFound
        ));
    }

    #[test]
    fn free_space_is_reported() {
        let (_dir, folder) = folder();
        assert!(folder.free_bytes().unwrap() > 0);
    }

    #[test]
    fn a_relative_custom_folder_is_refused() {
        assert!(matches!(
            UserFolder::open(Some(Path::new("Downloads"))),
            Err(UserFolderError::Io(error)) if error.kind() == io::ErrorKind::InvalidInput
        ));
    }

    fn impersonating() -> bool {
        let mut token = HANDLE::default();
        // SAFETY: token is a writable slot, closed below when opened.
        unsafe {
            let opened = OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &mut token).is_ok();
            if opened {
                let _ = CloseHandle(token);
            }
            opened
        }
    }

    #[test]
    fn impersonation_ends_after_success_failure_and_panic() {
        let dir = tempfile::tempdir().unwrap();
        let mut token = HANDLE::default();
        // SAFETY: token is a writable slot whose handle the OwnedHandle owns.
        let token = unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_IMPERSONATE,
                &mut token,
            )
            .unwrap();
            OwnedHandle::from_raw_handle(token.0)
        };
        let folder = UserFolder {
            path: dir.path().to_path_buf(),
            token: Some(Arc::new(token)),
        };
        assert!(!impersonating());
        assert!(folder.as_user(|| Ok(impersonating())).unwrap());
        assert!(!impersonating());
        let failed: Result<(), _> = folder.as_user(|| Err(io::Error::other("failed")));
        assert!(failed.is_err());
        assert!(!impersonating());
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            folder.as_user(|| -> io::Result<()> { panic!("work panicked") })
        }));
        assert!(panicked.is_err());
        assert!(!impersonating());
        // The handles opened as the user write after it ends.
        let mut part = folder.create_part("f.txt").unwrap();
        assert!(!impersonating());
        part.write_all(b"written").unwrap();
        assert_eq!(part.finish().unwrap(), "f.txt");
        assert!(!impersonating());
    }

    #[test]
    #[ignore = "creates Downloads\\Rubylight in this user's real profile"]
    fn the_default_folder_is_downloads_rubylight() {
        let folder = UserFolder::open(None).unwrap();
        assert!(
            folder.path().ends_with("Downloads\\Rubylight"),
            "{:?}",
            folder.path()
        );
        assert!(folder.path().is_dir());
    }

    #[test]
    #[ignore = "needs SYSTEM in a session nobody is signed in to (session 0 via rust/release/elevated.ps1)"]
    fn without_a_signed_in_user_there_is_no_folder() {
        assert!(crate::process::is_system());
        assert!(matches!(
            UserFolder::open(None),
            Err(UserFolderError::NoConsoleUser)
        ));
    }
}
