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

/// Runs `work` on this thread as the user (see [`user_token`]), so Windows
/// checks what it opens, follows and replaces against the user's rights, not
/// the host's: a link the user put in a folder of theirs leads nowhere the
/// user could not reach anyway. With nobody to act as, `work` does not run.
pub fn as_user<T>(work: impl FnOnce() -> T) -> Result<T, UserFolderError> {
    as_token(user_token()?.as_ref(), work)
}

/// [`as_user`] with `token`, or as the host without one. The impersonation
/// ends when `work` returns or panics.
pub(crate) fn as_token<T>(
    token: Option<&OwnedHandle>,
    work: impl FnOnce() -> T,
) -> Result<T, UserFolderError> {
    let _user = Impersonation::begin(token)?;
    Ok(work())
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
pub(crate) fn no_signed_in_user(code: HRESULT) -> bool {
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
pub(crate) fn known_folder(
    id: &GUID,
    token: Option<&OwnedHandle>,
) -> Result<PathBuf, UserFolderError> {
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
pub(crate) fn verbatim(path: &Path) -> io::Result<Vec<u16>> {
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
pub(crate) fn delete_on_close(file: &File) -> io::Result<()> {
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
pub(crate) mod tests;
