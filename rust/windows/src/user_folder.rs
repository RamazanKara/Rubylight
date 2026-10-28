//! The folder file transfers land in, opened as the signed-in user: a host
//! running as SYSTEM must not create files the user can't touch, nor reach
//! files the user can't. Handles are opened while impersonating the user's
//! token, so Windows checks the user's rights, and then read and written
//! without it. An elevated host acts as the same user without administrator
//! rights; a host without them already is the user.
//!
//! Every call blocks and impersonates on the calling thread: run them on a
//! blocking thread, not an async worker.
use butterpollo_core::file_transfer::{self, NameError};
use std::{
    ffi::{OsStr, OsString},
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Write},
    marker::PhantomData,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::{MetadataExt, OpenOptionsExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};
use windows::{
    Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NO_TOKEN, ERROR_SHARING_VIOLATION, HANDLE},
        Globalization::{CSTR_EQUAL, CompareStringOrdinal},
        Security::{ImpersonateLoggedOnUser, RevertToSelf},
        Storage::FileSystem::{
            DELETE, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL, FILE_BASIC_INFO,
            FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_FLAGS_AND_ATTRIBUTES, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_NAME_NORMALIZED,
            FILE_RENAME_INFO, FILE_SHARE_READ, FileBasicInfo, FileDispositionInfo, FileRenameInfo,
            GetDiskFreeSpaceExW, GetFinalPathNameByHandleW, ReOpenFile, SetFileInformationByHandle,
        },
        System::{Com::CoTaskMemFree, RemoteDesktop::WTSQueryUserToken},
        UI::Shell::{
            FOLDERID_Downloads, FOLDERID_RoamingAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
        },
    },
    core::{GUID, HRESULT, HSTRING},
};

/// The subfolder of the user's Downloads folder transfers use by default.
pub const DEFAULT_SUBFOLDER: &str = "Rubylight";
/// How many numbered names an upload tries before it gives up.
const MAX_COLLISIONS: u32 = 9999;
/// Marks this host's `.part` files until they are renamed, so a later open
/// of the file transfer folder sweeps those a stopped host left behind and
/// nobody else's (shared folders are never swept). Unlike most
/// attributes, a new file doesn't inherit it from its folder.
const PART_MARKER: FILE_FLAGS_AND_ATTRIBUTES = FILE_ATTRIBUTE_HIDDEN;
/// The Mark of the Web: Windows treats an upload like a download from the
/// internet (SmartScreen, Office Protected View).
const ZONE_IDENTIFIER: &[u8] = b"[ZoneTransfer]\r\nZoneId=3\r\n";

#[derive(Debug)]
pub enum UserFolderError {
    /// Nobody is signed in to the host's Windows session, so there is no
    /// user to write as.
    NoConsoleUser,
    /// The host runs elevated and the Windows desktop shell isn't running,
    /// so there is no token of the user without administrator rights. The
    /// folder is refused rather than written with administrator rights.
    NoUserShell,
    InvalidName(NameError),
    /// The name is a folder or a link (a symbolic link or a junction).
    NotPlainFile,
    /// What Windows opened is not the path asked for below a shared folder's
    /// root: a link or mount point on the way leads elsewhere, or a name was
    /// another name (8.3) of the file.
    OutsideFolder,
    Io(io::Error),
}
impl std::fmt::Display for UserFolderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoConsoleUser => f.write_str("nobody is signed in to Windows"),
            Self::NoUserShell => f.write_str(
                "file transfer cannot act as the Windows user: the host runs with administrator rights and the Windows desktop shell is not running. Start Rubylight without administrator rights, or use the installed Windows service",
            ),
            Self::InvalidName(error) => error.fmt(f),
            Self::NotPlainFile => f.write_str("not a plain file"),
            Self::OutsideFolder => f.write_str(
                "the path leaves the shared folder through a link, or names a file by another name",
            ),
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
pub(crate) fn os_error(error: windows::core::Error) -> io::Error {
    let code = error.code().0 as u32;
    if code & 0xFFFF_0000 == 0x8007_0000 {
        io::Error::from_raw_os_error((code & 0xFFFF) as i32)
    } else {
        io::Error::other(error)
    }
}

/// This thread runs as the user until the guard drops, also when the work
/// fails or panics. Not `Send`: the impersonation belongs to this thread.
pub(crate) struct Impersonation(PhantomData<*const ()>);
impl Impersonation {
    pub(crate) fn begin(token: Option<&OwnedHandle>) -> io::Result<Option<Self>> {
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
            tracing::error!("The host could not stop acting as the Windows user: {error}");
            // This thread would go on with the user's rights in place of the
            // host's, or with the host's where the user's were checked.
            std::process::abort();
        }
    }
}

/// The token files are opened with: the user signed in to the host's
/// session when the host runs as SYSTEM, the same user without
/// administrator rights when it runs elevated, and none when the host
/// already is that user.
pub(crate) fn user_token() -> Result<Option<OwnedHandle>, UserFolderError> {
    if !crate::process::is_system() {
        return standard_user(crate::process::standard_user_token());
    }
    let session = crate::process::current_session()
        .map_err(|error| io::Error::other(format!("{error:#}")))?;
    let mut token = HANDLE::default();
    // SAFETY: token is a writable handle slot.
    let queried = unsafe { WTSQueryUserToken(session, &mut token) };
    match queried {
        // SAFETY: WTS returned an open token that only this OwnedHandle closes.
        Ok(()) => Ok(Some(unsafe { OwnedHandle::from_raw_handle(token.0) })),
        Err(error) if no_signed_in_user(error.code()) => Err(UserFolderError::NoConsoleUser),
        Err(error) => Err(os_error(error).into()),
    }
}

/// The elevated host's token of its user without administrator rights, with
/// a missing desktop shell as [`UserFolderError::NoUserShell`].
fn standard_user(
    token: anyhow::Result<Option<OwnedHandle>>,
) -> Result<Option<OwnedHandle>, UserFolderError> {
    token.map_err(|error| {
        if error.downcast_ref::<crate::process::NoShell>().is_some() {
            return UserFolderError::NoUserShell;
        }
        io::Error::other(format!(
            "file transfer cannot act as the Windows user without administrator rights: {error:#}"
        ))
        .into()
    })
}

/// Runs `work` on this thread as the user signed in to the host's session
/// when the host runs as SYSTEM, for what Windows keeps per user. Otherwise,
/// or with nobody signed in, it runs as the host.
pub(crate) fn as_session_user<T>(work: impl FnOnce() -> T) -> io::Result<T> {
    let token = if crate::process::is_system() {
        match user_token() {
            Ok(token) => token,
            Err(UserFolderError::NoConsoleUser) => None,
            Err(UserFolderError::Io(error)) => return Err(error),
            Err(error) => return Err(io::Error::other(error.to_string())),
        }
    } else {
        None
    };
    let _user = Impersonation::begin(token.as_ref())?;
    Ok(work())
}

/// How `WTSQueryUserToken` says the session has no user: ERROR_NO_TOKEN
/// at the sign-in screen, ERROR_FILE_NOT_FOUND when the session is gone.
fn no_signed_in_user(code: HRESULT) -> bool {
    code == HRESULT::from_win32(ERROR_NO_TOKEN.0)
        || code == HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0)
}

/// The user's Downloads folder, wherever they moved it.
fn downloads(token: Option<&OwnedHandle>) -> Result<PathBuf, UserFolderError> {
    known_folder(&FOLDERID_Downloads, token)
}

/// The roaming AppData folder of the user signed in to the host's session
/// (see [`user_token`]), where Nucleus Co-op keeps the user's game list.
pub fn roaming_app_data() -> Result<PathBuf, UserFolderError> {
    let token = user_token()?;
    known_folder(&FOLDERID_RoamingAppData, token.as_ref())
}

/// The known folder `id` of the user `token` names, or of the host's user.
fn known_folder(id: &GUID, token: Option<&OwnedHandle>) -> Result<PathBuf, UserFolderError> {
    let token = token.map(|token| HANDLE(token.as_raw_handle()));
    // SAFETY: the token stays open for the call; the string Windows returns
    // is copied, then freed once.
    unsafe {
        let path = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, token).map_err(os_error)?;
        let copy = OsString::from_wide(path.as_wide());
        CoTaskMemFree(Some(path.0 as *const _));
        Ok(PathBuf::from(copy))
    }
}

/// A plain file: not a folder or a link. Other reparse points, such as
/// compressed (WOF), deduplicated or cloud files, hold the file's own data.
pub(crate) fn plain(metadata: &Metadata) -> bool {
    metadata.is_file()
}

/// `path` made absolute with the `\\?\` prefix, so Win32 calls take it
/// beyond MAX_PATH (260 characters). Not NUL-terminated.
fn verbatim(path: &Path) -> io::Result<Vec<u16>> {
    let path: Vec<u16> = std::path::absolute(path)?
        .as_os_str()
        .encode_wide()
        .collect();
    let prefix = |prefix: &str| -> Vec<u16> { OsStr::new(prefix).encode_wide().collect() };
    let (keep, from) = if path.starts_with(&prefix(r"\\?\")) {
        return Ok(path);
    } else if path.starts_with(&prefix(r"\\.\")) {
        (prefix(r"\\?\"), 4)
    } else if path.starts_with(&prefix(r"\\")) {
        (prefix(r"\\?\UNC\"), 2)
    } else {
        (prefix(r"\\?\"), 0)
    };
    Ok([keep.as_slice(), &path[from..]].concat())
}

pub(crate) fn raw(file: &File) -> HANDLE {
    HANDLE(file.as_raw_handle())
}

/// Renames the open file to `target`, never over a file already there.
fn rename_open(file: &File, target: &Path) -> io::Result<()> {
    let target = verbatim(target)?;
    let name_bytes = target.len() * size_of::<u16>();
    let bytes = std::mem::offset_of!(FILE_RENAME_INFO, FileName) + name_bytes + size_of::<u16>();
    // u64 keeps the structure's alignment; zeroed is ReplaceIfExists false,
    // no root folder and a NUL after the name.
    let mut buffer = vec![0u64; bytes.div_ceil(size_of::<u64>())];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // SAFETY: the buffer is aligned for FILE_RENAME_INFO and holds the
    // name it is followed by; Windows reads it during the call only.
    unsafe {
        (*info).FileNameLength = name_bytes as u32;
        std::ptr::copy_nonoverlapping(
            target.as_ptr(),
            (&raw mut (*info).FileName).cast::<u16>(),
            target.len(),
        );
        SetFileInformationByHandle(
            raw(file),
            FileRenameInfo,
            info.cast_const().cast(),
            (buffer.len() * size_of::<u64>()) as u32,
        )
    }
    .map_err(os_error)
}

/// Deletes the open file once its last handle closes.
fn delete_on_close(file: &File) -> io::Result<()> {
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: info is a live FILE_DISPOSITION_INFO of the size passed.
    unsafe {
        SetFileInformationByHandle(
            raw(file),
            FileDispositionInfo,
            (&raw const info).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    }
    .map_err(os_error)
}

/// Clears [`PART_MARKER`] from the open file.
fn unmark(file: &File) -> io::Result<()> {
    let attributes = file.metadata()?.file_attributes() & !PART_MARKER.0;
    let info = FILE_BASIC_INFO {
        // Times of zero stay as they are; no attributes at all is NORMAL.
        FileAttributes: if attributes == 0 {
            FILE_ATTRIBUTE_NORMAL.0
        } else {
            attributes
        },
        ..Default::default()
    };
    // SAFETY: info is a live FILE_BASIC_INFO of the size passed.
    unsafe {
        SetFileInformationByHandle(
            raw(file),
            FileBasicInfo,
            (&raw const info).cast(),
            size_of::<FILE_BASIC_INFO>() as u32,
        )
    }
    .map_err(os_error)
}

/// The open plain file opened again, as the user, without
/// FILE_FLAG_OPEN_REPARSE_POINT, so a compressed, deduplicated or cloud file
/// reads as its data; with its length.
pub(crate) fn reopen_for_data(
    token: Option<&OwnedHandle>,
    file: &File,
) -> Result<(File, u64), UserFolderError> {
    let reopened = {
        let _user = Impersonation::begin(token)?;
        // SAFETY: the handle is open for the call.
        unsafe {
            ReOpenFile(
                raw(file),
                FILE_GENERIC_READ.0,
                FILE_SHARE_READ,
                FILE_FLAGS_AND_ATTRIBUTES(0),
            )
        }
        .map_err(os_error)?
    };
    // SAFETY: ReOpenFile returned a new open handle only this File closes.
    let file = unsafe { File::from_raw_handle(reopened.0) };
    let len = file.metadata()?.len();
    Ok((file, len))
}

/// The name the open file has in its folder, as stored on disk: the long
/// name even when it was opened by its 8.3 short name.
fn final_name(file: &File) -> io::Result<OsString> {
    let path = final_path(file)?;
    let path: Vec<u16> = path.encode_wide().collect();
    let start = path
        .iter()
        .rposition(|&unit| unit == u16::from(b'\\'))
        .map_or(0, |separator| separator + 1);
    Ok(OsString::from_wide(&path[start..]))
}

/// The whole path of the open file as Windows resolved it, with the `\\?\`
/// prefix: links followed and 8.3 short names replaced by the long ones.
pub(crate) fn final_path(file: &File) -> io::Result<OsString> {
    let mut path = vec![0u16; 512];
    loop {
        // SAFETY: the handle is open and path is a writable buffer.
        let len = unsafe { GetFinalPathNameByHandleW(raw(file), &mut path, FILE_NAME_NORMALIZED) }
            as usize;
        if len == 0 {
            return Err(io::Error::last_os_error());
        }
        if len < path.len() {
            path.truncate(len);
            break;
        }
        // Too small: len is the size needed, with the NUL.
        path.resize(len, 0);
    }
    Ok(OsString::from_wide(&path))
}

/// Whether `a` and `b` are one name or path to NTFS: equal but for case as
/// Windows maps it, code unit by code unit. Unlike Unicode lowercasing, the
/// Kelvin sign is not a `k` there, nor the Ohm sign an omega.
pub(crate) fn same_name(a: &OsStr, b: &OsStr) -> bool {
    let a: Vec<u16> = a.encode_wide().collect();
    let b: Vec<u16> = b.encode_wide().collect();
    // SAFETY: both are live slices, passed with their lengths.
    a == b || unsafe { CompareStringOrdinal(&a, &b, true) } == CSTR_EQUAL
}

/// Writes the Mark of the Web next to the data of the file at `path`.
fn mark_of_the_web(path: &Path) -> io::Result<()> {
    let mut stream = path.as_os_str().to_owned();
    stream.push(":Zone.Identifier");
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(stream)?
        .write_all(ZONE_IDENTIFIER)
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
    /// A shared folder's directory, held open without delete sharing so it
    /// can't be moved or swapped for a link while an upload uses it.
    _pin: Option<Arc<File>>,
}
impl UserFolder {
    /// The transfer folder, created if missing: `custom` when the host's
    /// settings name one, otherwise the user's `Downloads\Rubylight`. Open
    /// it for each transfer, since the signed-in user can change.
    pub fn open(custom: Option<&Path>) -> Result<Self, UserFolderError> {
        Self::open_as(custom, user_token)
    }

    /// [`UserFolder::open`] with the user's token from `token`. The folder
    /// setting is checked first, so a bad one is named whoever runs the host.
    fn open_as(
        custom: Option<&Path>,
        token: impl FnOnce() -> Result<Option<OwnedHandle>, UserFolderError>,
    ) -> Result<Self, UserFolderError> {
        if let Some(path) = custom.filter(|path| !path.is_absolute()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "the file transfer folder {} is not absolute",
                    path.display()
                ),
            )
            .into());
        }
        let token = token()?.map(Arc::new);
        let path = match custom {
            Some(path) => path.to_path_buf(),
            None => downloads(token.as_deref())?.join(DEFAULT_SUBFOLDER),
        };
        Self::at(path, token)
    }

    /// The folder at `path`, opened as this process rather than as the
    /// signed-in user. Only for tests of the callers (the `test-folder`
    /// feature): a test host may run elevated without a desktop shell,
    /// where [`UserFolder::open`] refuses.
    #[cfg(feature = "test-folder")]
    pub fn unimpersonated(path: &Path) -> Result<Self, UserFolderError> {
        Self::at(path.to_path_buf(), None)
    }

    fn at(path: PathBuf, token: Option<Arc<OwnedHandle>>) -> Result<Self, UserFolderError> {
        let folder = Self {
            path,
            token,
            _pin: None,
        };
        folder.as_user(|| {
            fs::create_dir_all(&folder.path)?;
            folder.sweep()
        })?;
        Ok(folder)
    }

    /// A directory of a shared folder that `pin` holds open, as the folder
    /// uploads go to. Nothing is created and nothing swept: the folder is
    /// the user's, so a hidden `.part` file there can be anyone's. One a
    /// stopped host left behind stays, hidden and never listed.
    pub(crate) fn pinned(path: PathBuf, token: Option<Arc<OwnedHandle>>, pin: File) -> Self {
        Self {
            path,
            token,
            _pin: Some(Arc::new(pin)),
        }
    }

    /// Deletes the `.part` files of uploads a stopped host left behind.
    /// One still being written is open without delete sharing, so it fails
    /// with a sharing violation and stays.
    fn sweep(&self) -> io::Result<()> {
        for entry in fs::read_dir(&self.path)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            let name = entry.file_name();
            if !plain(&metadata)
                || metadata.file_attributes() & PART_MARKER.0 == 0
                || !name.to_str().is_some_and(file_transfer::has_part_suffix)
            {
                continue;
            }
            match fs::remove_file(entry.path()) {
                Ok(()) => tracing::info!(
                    "File transfer deleted the incomplete upload {}",
                    name.display()
                ),
                Err(error) if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION.0 as i32) => {}
                Err(error) => tracing::warn!(
                    "File transfer could not delete the incomplete upload {}: {error}",
                    name.display()
                ),
            }
        }
        Ok(())
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
                // Delete access renames or deletes the part through this
                // handle, without delete sharing nobody else can.
                match OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .access_mode(FILE_GENERIC_WRITE.0 | DELETE.0)
                    .share_mode(FILE_SHARE_READ.0)
                    .attributes(PART_MARKER.0)
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

    /// Opens a plain file in the folder for reading, with its length. Only
    /// by its own name: not by its 8.3 short name, which could reach an
    /// upload in progress.
    pub fn open_file(&self, name: &str) -> Result<(File, u64), UserFolderError> {
        file_transfer::check_stored_name(name)?;
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
        if !plain(&file.metadata()?) {
            return Err(UserFolderError::NotPlainFile);
        }
        let actual = final_name(&file)?;
        if !same_name(&actual, OsStr::new(name)) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{name} is another name of {}", actual.display()),
            )
            .into());
        }
        reopen_for_data(self.token.as_deref(), &file)
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
                // Refuses `.part` names too.
                if file_transfer::check_stored_name(&name).is_err() {
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
        let mut path = verbatim(&self.path)?;
        if path.last() != Some(&u16::from(b'\\')) {
            path.push(u16::from(b'\\'));
        }
        let path = HSTRING::from_wide(&path);
        self.as_user(|| {
            let mut free = 0u64;
            // SAFETY: path is NUL-terminated and outlives the call; free is a
            // writable u64.
            unsafe { GetDiskFreeSpaceExW(&path, Some(&mut free), None, None) }.map_err(os_error)?;
            Ok(free)
        })
    }

    /// The bytes the user may still write to the folder's volume and the
    /// volume's size as the user sees it, both after any disk quota.
    pub fn volume_bytes(&self) -> Result<(u64, u64), UserFolderError> {
        let mut path = verbatim(&self.path)?;
        if path.last() != Some(&u16::from(b'\\')) {
            path.push(u16::from(b'\\'));
        }
        let path = HSTRING::from_wide(&path);
        self.as_user(|| {
            let (mut free, mut total) = (0u64, 0u64);
            // SAFETY: path is NUL-terminated and outlives the call; free and
            // total are writable u64s.
            unsafe { GetDiskFreeSpaceExW(&path, Some(&mut free), Some(&mut total), None) }
                .map_err(os_error)?;
            Ok((free, total))
        })
    }
}

/// An upload in progress, written to its `.part` file through a handle
/// opened as the user, then renamed and deleted through that handle, so a
/// link put at its path later can't redirect either. Dropping it before
/// [`PartFile::finish`] deletes the `.part` file; [`PartFile::discard`]
/// does the same and reports a failure. A `.part` file a failed deletion
/// leaves behind is swept by the next [`UserFolder::open`].
///
/// Dropping it, like every call here, does blocking I/O: drop it on a
/// blocking thread.
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
    /// The open `.part` file, until it is finished or discarded.
    pub(crate) fn file(&self) -> Option<&File> {
        self.file.as_ref()
    }

    /// Closes the upload and gives it the first free numbered name, which
    /// is returned. If the rename fails the `.part` file is deleted.
    pub fn finish(mut self) -> Result<String, UserFolderError> {
        let (Some(file), Some(part)) = (self.file.as_mut(), self.part.as_deref()) else {
            return Err(io::Error::other("the upload is already closed").into());
        };
        file.flush()?;
        // Not synced to disk before the rename: that would hold up the
        // reply for seconds on a large upload, and a power cut right after
        // could leave the named file short, as with a browser download.
        if let Err(error) = self.folder.as_user(|| mark_of_the_web(part)) {
            // As on FAT or exFAT, which can't hold the mark.
            tracing::warn!(
                "File transfer could not mark {} as downloaded from the internet: {error}",
                self.name
            );
        }
        unmark(file)?;
        let file = &*file;
        let name = self.folder.as_user(|| {
            for n in 0..=MAX_COLLISIONS {
                let candidate = file_transfer::collision_name(&self.name, n);
                match rename_open(file, &self.folder.path.join(&candidate)) {
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
        drop(self.file.take());
        Ok(name)
    }

    /// Ends an incomplete upload by deleting its `.part` file.
    pub fn discard(mut self) -> Result<(), UserFolderError> {
        self.remove()
    }

    fn remove(&mut self) -> Result<(), UserFolderError> {
        self.part = None;
        let Some(file) = self.file.take() else {
            return Ok(());
        };
        // Through the handle, so only this upload's file goes; deleted
        // when the handle closes right after.
        delete_on_close(&file)?;
        Ok(())
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
        Security::{
            CreateRestrictedToken, DISABLE_MAX_PRIVILEGE, GetTokenInformation, SID_AND_ATTRIBUTES,
            TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_IMPERSONATE, TOKEN_QUERY, TOKEN_USER,
            TokenUser,
        },
        Storage::FileSystem::GetShortPathNameW,
        System::Threading::{
            GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken,
        },
    };

    /// A folder in a new temporary folder, used as the host itself.
    fn folder() -> (tempfile::TempDir, UserFolder) {
        let dir = tempfile::tempdir().unwrap();
        let folder = UserFolder::at(dir.path().to_path_buf(), None).unwrap();
        (dir, folder)
    }

    fn icacls(path: &Path, args: &[&str]) {
        let status = std::process::Command::new("icacls")
            .arg(path)
            .args(args)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "icacls {args:?} failed");
    }

    fn own_token() -> OwnedHandle {
        let mut token = HANDLE::default();
        // SAFETY: token is a writable slot whose handle the OwnedHandle owns.
        unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_IMPERSONATE | TOKEN_ASSIGN_PRIMARY,
                &mut token,
            )
            .unwrap();
            OwnedHandle::from_raw_handle(token.0)
        }
    }

    /// This process's token with its user's SID only able to deny: it is
    /// refused what is granted to the user by name, like another user.
    fn without_own_sid() -> OwnedHandle {
        let own = own_token();
        let own = HANDLE(own.as_raw_handle());
        let mut bytes = 0;
        // SAFETY: the buffer is aligned and as large as Windows asked for;
        // the user SID it holds is copied by CreateRestrictedToken.
        unsafe {
            let _ = GetTokenInformation(own, TokenUser, None, 0, &mut bytes);
            let mut buffer = vec![0u64; (bytes as usize).div_ceil(size_of::<u64>())];
            GetTokenInformation(
                own,
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                bytes,
                &mut bytes,
            )
            .unwrap();
            let user = (*buffer.as_ptr().cast::<TOKEN_USER>()).User;
            let mut restricted = HANDLE::default();
            CreateRestrictedToken(
                own,
                DISABLE_MAX_PRIVILEGE,
                Some(&[SID_AND_ATTRIBUTES {
                    Sid: user.Sid,
                    Attributes: 0,
                }]),
                None,
                None,
                &mut restricted,
            )
            .unwrap();
            OwnedHandle::from_raw_handle(restricted.0)
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
    fn an_upload_is_hidden_until_it_is_finished_and_then_marked_from_the_internet() {
        let (dir, folder) = folder();
        let mut part = folder.create_part("setup.exe").unwrap();
        part.write_all(b"MZ").unwrap();
        let attributes = |name: &str| {
            fs::metadata(dir.path().join(name))
                .unwrap()
                .file_attributes()
        };
        assert_ne!(attributes("setup.exe.part") & PART_MARKER.0, 0);
        assert_eq!(part.finish().unwrap(), "setup.exe");
        assert_eq!(attributes("setup.exe") & PART_MARKER.0, 0);
        assert_eq!(
            fs::read(dir.path().join("setup.exe:Zone.Identifier")).unwrap(),
            ZONE_IDENTIFIER
        );
        assert_eq!(fs::read(dir.path().join("setup.exe")).unwrap(), b"MZ");
    }

    #[test]
    fn an_upload_past_max_path_is_finished() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f".repeat(80));
        let folder = UserFolder::at(path.clone(), None).unwrap();
        let name = format!("{}.txt", "n".repeat(196));
        assert!(path.join(file_transfer::part_name(&name)).as_os_str().len() > 260);
        assert_eq!(upload(&folder, &name, b"long"), name);
        let numbered = file_transfer::collision_name(&name, 1);
        assert_eq!(upload(&folder, &name, b"longer"), numbered);
        assert_eq!(names(&path), [numbered.clone(), name.clone()]);
        let (mut file, len) = folder.open_file(&name).unwrap();
        let mut bytes = Vec::new();
        io::Read::read_to_end(&mut file, &mut bytes).unwrap();
        assert_eq!((bytes.as_slice(), len), (b"long".as_slice(), 4));
        // The numbered copy is longer than any name an upload may have.
        assert_eq!(folder.list().unwrap().len(), 2);
        assert_eq!(folder.open_file(&numbered).unwrap().1, 6);
        assert!(folder.free_bytes().unwrap() > 0);
        let (free, total) = folder.volume_bytes().unwrap();
        assert!(0 < free && free <= total, "{free} of {total}");
        drop(file);
        // An incomplete one is deleted there too.
        let mut part = folder.create_part(&name).unwrap();
        part.write_all(b"partial").unwrap();
        drop(part);
        assert_eq!(names(&path), [numbered, name]);
    }

    #[test]
    fn opening_the_folder_sweeps_only_its_own_stale_uploads() {
        let (dir, folder) = folder();
        // Left by a host that stopped mid-upload.
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .attributes(PART_MARKER.0)
            .open(dir.path().join("stale.txt.part"))
            .unwrap();
        // A browser's download in progress, not marked.
        fs::write(dir.path().join("browser.zip.part"), "zip").unwrap();
        let mut live = folder.create_part("live.txt").unwrap();
        live.write_all(b"live").unwrap();
        UserFolder::at(dir.path().to_path_buf(), None).unwrap();
        assert_eq!(names(dir.path()), ["browser.zip.part", "live.txt.part"]);
        assert_eq!(live.finish().unwrap(), "live.txt");
        assert_eq!(fs::read(dir.path().join("live.txt")).unwrap(), b"live");
    }

    #[test]
    fn names_are_the_same_exactly_when_ntfs_finds_one_by_the_other() {
        let dir = tempfile::tempdir().unwrap();
        // The Kelvin, long s, Angstrom and Ohm signs lowercase to letters
        // NTFS keeps apart from them.
        for (stored, asked) in [
            ("K", "k"),
            ("\u{e4}", "\u{c4}"),
            ("\u{212a}", "k"),
            ("\u{17f}", "s"),
            ("\u{212b}", "\u{e5}"),
            ("\u{2126}", "\u{3c9}"),
        ] {
            let stored = format!("{stored}.txt");
            let asked = format!("{asked}.txt");
            fs::write(dir.path().join(&stored), "x").unwrap();
            let found = fs::metadata(dir.path().join(&asked)).is_ok();
            assert_eq!(
                same_name(OsStr::new(&stored), OsStr::new(&asked)),
                found,
                "{stored} {asked}"
            );
            fs::remove_file(dir.path().join(&stored)).unwrap();
        }
        assert!(!same_name(OsStr::new("\u{212a}"), OsStr::new("k")));
        assert!(same_name(OsStr::new("Photo.JPG"), OsStr::new("photo.jpg")));
    }

    #[test]
    fn a_file_opens_only_by_its_own_name() {
        let (dir, folder) = folder();
        let mut part = folder.create_part("holiday photo.jpg").unwrap();
        part.write_all(b"jpeg").unwrap();
        assert_eq!(
            upload(&folder, "long document name.txt", b"done"),
            "long document name.txt"
        );
        // Its name in another case is the same name.
        let (_file, len) = folder.open_file("LONG DOCUMENT NAME.TXT").unwrap();
        assert_eq!(len, 4);
        assert!(matches!(
            folder.open_file("holiday photo.jpg.part"),
            Err(UserFolderError::InvalidName(NameError::PartSuffix))
        ));
        // Volumes without 8.3 names have no other name to try.
        if let Some(short) = short_name(&dir.path().join("long document name.txt")) {
            assert!(
                matches!(
                    folder.open_file(&short),
                    Err(UserFolderError::Io(error)) if error.kind() == io::ErrorKind::NotFound
                ),
                "{short}"
            );
        }
        if let Some(short) = short_name(&dir.path().join("holiday photo.jpg.part")) {
            assert!(folder.open_file(&short).is_err(), "{short}");
        }
        part.discard().unwrap();
    }

    #[test]
    fn a_compressed_file_is_listed_and_reads_as_its_data() {
        let (dir, folder) = folder();
        let bytes: Vec<u8> = b"rubylight "
            .iter()
            .copied()
            .cycle()
            .take(64 * 1024)
            .collect();
        let path = dir.path().join("notes.txt");
        fs::write(&path, &bytes).unwrap();
        let status = std::process::Command::new("compact")
            .args(["/c", "/exe:xpress4k"])
            .arg(&path)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "compact failed");
        // Compressed by the Windows Overlay Filter into a reparse point,
        // which the filter usually hides; where it shows, the file must
        // still list and read as its data.
        let listed: Vec<_> = folder
            .list()
            .unwrap()
            .into_iter()
            .map(|file| (file.name, file.size))
            .collect();
        assert_eq!(listed, [("notes.txt".to_owned(), bytes.len() as u64)]);
        let (mut file, len) = folder.open_file("notes.txt").unwrap();
        let mut read = Vec::new();
        io::Read::read_to_end(&mut file, &mut read).unwrap();
        assert_eq!(len, bytes.len() as u64);
        assert!(read == bytes, "the compressed file read as other bytes");
    }

    #[test]
    fn a_folder_swapped_for_a_junction_keeps_the_users_rights() {
        let base = tempfile::tempdir().unwrap();
        // Authenticated Users may use everything here, junctions included,
        // except `forbidden`: only this user's SID may, which the
        // restricted token can't.
        icacls(base.path(), &["/grant", "*S-1-5-11:(OI)(CI)F"]);
        let allowed = base.path().join("allowed");
        let forbidden = base.path().join("forbidden");
        fs::create_dir(&allowed).unwrap();
        fs::create_dir(&forbidden).unwrap();
        let sid = crate::process::own_sid().unwrap();
        icacls(
            &forbidden,
            &["/inheritance:r", "/grant:r", &format!("*{sid}:(OI)(CI)F")],
        );
        fs::write(forbidden.join("secret.txt"), "secret").unwrap();
        let link = base.path().join("transfers");
        junction(&allowed, &link);
        let folder = UserFolder::at(link.clone(), Some(Arc::new(without_own_sid()))).unwrap();
        assert_eq!(upload(&folder, "a.txt", b"a"), "a.txt");
        assert_eq!(fs::read(allowed.join("a.txt")).unwrap(), b"a");

        // Swapped during an upload: the rename is checked with the user's
        // rights at the new place, and the part is still deleted.
        let mut part = folder.create_part("b.txt").unwrap();
        part.write_all(b"b").unwrap();
        fs::remove_dir(&link).unwrap();
        junction(&forbidden, &link);
        fn denied<T>(result: Result<T, UserFolderError>) -> bool {
            matches!(result, Err(UserFolderError::Io(error)) if error.kind() == io::ErrorKind::PermissionDenied)
        }
        assert!(denied(part.finish()));
        assert!(denied(folder.create_part("c.txt")));
        assert!(denied(folder.open_file("secret.txt")));
        assert!(denied(folder.list()));
        assert_eq!(names(&allowed), ["a.txt"]);
        assert_eq!(names(&forbidden), ["secret.txt"]);
    }

    #[test]
    fn an_elevated_host_acts_as_the_user_without_administrator_rights() {
        if crate::process::is_system() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        match crate::process::standard_user_token() {
            Ok(Some(_)) => {
                let folder = UserFolder::open(Some(dir.path())).unwrap();
                assert!(folder.token.is_some());
                let elevated = folder
                    .as_user(|| {
                        let mut token = HANDLE::default();
                        // SAFETY: token is a writable slot whose handle the
                        // OwnedHandle owns.
                        let token = unsafe {
                            OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &mut token)
                                .map_err(os_error)?;
                            OwnedHandle::from_raw_handle(token.0)
                        };
                        crate::process::token_elevated(&token)
                            .map_err(|error| io::Error::other(format!("{error:#}")))
                    })
                    .unwrap();
                assert!(!elevated);
            }
            // Not elevated, or User Account Control is off: already the user.
            Ok(None) => assert!(UserFolder::open(Some(dir.path())).unwrap().token.is_none()),
            // Elevated without the user's desktop shell: refused, not done
            // with administrator rights.
            Err(error) if error.downcast_ref::<crate::process::NoShell>().is_some() => {
                assert!(matches!(
                    UserFolder::open(Some(dir.path())),
                    Err(UserFolderError::NoUserShell)
                ))
            }
            Err(_) => assert!(UserFolder::open(Some(dir.path())).is_err()),
        }
    }

    #[test]
    fn an_elevated_host_without_a_shell_opens_no_folder() {
        let dir = tempfile::tempdir().unwrap();
        let wanted = dir.path().join("transfers");
        let refused = UserFolder::open_as(Some(&wanted), || {
            standard_user(Err(crate::process::NoShell.into()))
        });
        assert!(matches!(refused, Err(UserFolderError::NoUserShell)));
        // Not created with the host's administrator rights instead.
        assert!(!wanted.exists());
        let message = UserFolderError::NoUserShell.to_string();
        assert!(
            message.contains("desktop shell is not running"),
            "{message}"
        );
        assert!(
            message.contains("without administrator rights"),
            "{message}"
        );
    }

    #[test]
    fn other_token_failures_stay_io_errors_with_their_cause() {
        let failed = standard_user(Err(anyhow::anyhow!(
            "the Windows desktop shell belongs to another user"
        )));
        match failed {
            Err(UserFolderError::Io(error)) => {
                assert!(
                    error.to_string().contains("belongs to another user"),
                    "{error}"
                )
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(standard_user(Ok(None)), Ok(None)));
    }

    #[test]
    fn a_relative_custom_folder_is_refused_before_the_user_is_looked_up() {
        let refused = UserFolder::open_as(Some(Path::new("Downloads")), || {
            panic!("the user's token was looked up for a refused folder")
        });
        assert!(matches!(
            refused,
            Err(UserFolderError::Io(error)) if error.kind() == io::ErrorKind::InvalidInput
        ));
    }

    #[test]
    fn paths_get_the_verbatim_prefix_once() {
        let verbatim =
            |path: &str| String::from_utf16(&verbatim(Path::new(path)).unwrap()).unwrap();
        assert_eq!(verbatim(r"C:\a\..\b"), r"\\?\C:\b");
        assert_eq!(verbatim(r"\\server\share\x"), r"\\?\UNC\server\share\x");
        assert_eq!(verbatim(r"\\.\C:\x"), r"\\?\C:\x");
        assert_eq!(verbatim(r"\\?\C:\x"), r"\\?\C:\x");
    }

    #[test]
    fn a_session_without_a_user_is_no_console_user() {
        assert!(no_signed_in_user(HRESULT::from_win32(ERROR_NO_TOKEN.0)));
        assert!(no_signed_in_user(HRESULT::from_win32(
            ERROR_FILE_NOT_FOUND.0
        )));
        assert!(!no_signed_in_user(HRESULT::from_win32(5)));
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
        // A `.part` file this host didn't write stays untouched.
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
        let folder = UserFolder {
            path: dir.path().to_path_buf(),
            token: Some(Arc::new(own_token())),
            _pin: None,
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
