//! File transfer (`docs/native-api.md`, "Files"): a paired device sends
//! files to the PC's transfer folder and fetches them back.
//!
//! Every route needs the host's File transfer setting and the device's
//! Upload files or Download files bit, read on each request and again every
//! [`PACE_CHECK`] while a transfer runs. Files are opened as the signed-in
//! Windows user on a blocking thread (`butterpollo_win_io::user_folder`).
//! While a stream runs, uploads and downloads together move at
//! `file_transfer_stream_mbps`. An upload streams to its `.part` file and is
//! renamed only once all of its `Content-Length` arrived.
mod download;
mod upload;

use super::{Failure, Reply, caller, json};
use axum::{
    Extension,
    extract::{Request, State},
    http::StatusCode,
};
use butterpollo_session::{
    native::{FILE_DOWNLOAD, FILE_UPLOAD},
    state::Client,
};
pub use butterpollo_stream::file_transfers::{PACE_BURST, PACE_CHECK, Transfers, room};
use butterpollo_stream::{state::Shared, tls::Connection};
use butterpollo_win_io::user_folder::{UserFolder, UserFolderError};
pub(super) use download::send_file;
use rubylight_native_api::{
    error::ErrorCode,
    files::{FileEntry, FileList},
};
use std::{
    io,
    path::{Path, PathBuf},
    sync::PoisonError,
    time::{Duration, Instant},
};
pub(super) use upload::{accept, closing, drain, upload};

/// Bytes read from disk per download chunk.
const CHUNK: usize = 64 * 1024;
/// Chunks queued between the socket and the disk.
const QUEUE: usize = 8;
/// Downloads one device may run at once.
const DEVICE_DOWNLOADS: usize = 2;
/// Downloads all devices together may run at once.
const ALL_DOWNLOADS: usize = 8;
/// `ERROR_SHARING_VIOLATION`: another program holds the file open without
/// letting others read it.
const SHARING_VIOLATION: i32 = 32;

/// A device's upload slot, given back when the upload ends however it ends.
/// The upload's writer holds it, so the slot frees only once the `.part`
/// file is renamed or gone.
pub(super) struct UploadSlot {
    host: Shared,
    device: String,
}
impl UploadSlot {
    /// Holds room for `length` bytes on the folder's volume, if it has that
    /// much beside the reserve and the bytes other uploads still write.
    pub(super) fn hold_room(
        &self,
        folder: &UserFolder,
        length: u64,
    ) -> Result<(), UserFolderError> {
        let transfers = &self.host.file_transfers;
        // The space is read under the lock, so two uploads can't both
        // count on the same room.
        let mut uploads = transfers
            .uploads
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (free, total) = transfers.space(folder)?;
        if length > room(&uploads, free, total) {
            return Err(io::Error::from(io::ErrorKind::StorageFull).into());
        }
        uploads.insert(self.device.clone(), length);
        Ok(())
    }
    /// Counts `bytes` as written, so they are no longer due.
    fn wrote(&self, bytes: u64) {
        let mut uploads = self
            .host
            .file_transfers
            .uploads
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(left) = uploads.get_mut(&self.device) {
            *left = left.saturating_sub(bytes);
        }
    }
}
impl Drop for UploadSlot {
    fn drop(&mut self) {
        self.host
            .file_transfers
            .uploads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.device);
    }
}
fn begin_upload(h: &Shared, device: &str) -> Result<UploadSlot, Failure> {
    let mut uploads = h
        .file_transfers
        .uploads
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if uploads.contains_key(device) {
        return Err(Failure::new(
            ErrorCode::TransferBusy,
            "This device is already sending a file. Wait for it to finish.",
        ));
    }
    uploads.insert(device.to_owned(), 0);
    Ok(UploadSlot {
        host: h.clone(),
        device: device.to_owned(),
    })
}

/// A device's download slot, given back once the download's file is closed.
pub(super) struct DownloadSlot {
    host: Shared,
    device: String,
}
impl Drop for DownloadSlot {
    fn drop(&mut self) {
        let mut downloads = self
            .host
            .file_transfers
            .downloads
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(running) = downloads.get_mut(&self.device) {
            *running -= 1;
            if *running == 0 {
                downloads.remove(&self.device);
            }
        }
    }
}
pub(super) fn begin_download(h: &Shared, device: &str) -> Result<DownloadSlot, Failure> {
    let mut downloads = h
        .file_transfers
        .downloads
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if downloads
        .get(device)
        .is_some_and(|n| *n >= DEVICE_DOWNLOADS)
    {
        return Err(Failure::new(
            ErrorCode::TransferBusy,
            "This device is already downloading two files. Wait for one to finish.",
        ));
    }
    if downloads.values().sum::<usize>() >= ALL_DOWNLOADS {
        return Err(Failure::new(
            ErrorCode::TransferBusy,
            "The PC is sending as many files as it can at once. Wait for one to finish.",
        ));
    }
    *downloads.entry(device.to_owned()).or_default() += 1;
    Ok(DownloadSlot {
        host: h.clone(),
        device: device.to_owned(),
    })
}

/// The caller, if the host has File transfer on and the device has `bit`.
fn allowed(h: &Shared, c: &Connection, bit: u32, name: &str) -> Result<Client, Failure> {
    let client = caller(h, c)?;
    if !h
        .config
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .file_transfer()
    {
        return Err(Failure::new(
            ErrorCode::FileTransferDisabled,
            "File transfer is off on this PC. Turn it on in the Rubylight console.",
        ));
    }
    if !client.allows(bit) {
        return Err(Failure::new(
            ErrorCode::Forbidden,
            format!("Turn on {name} for this device in the Rubylight console."),
        ));
    }
    Ok(client)
}
/// A transfer's permission check, again every [`PACE_CHECK`] while it runs.
/// For file transfer it is [`allowed`], so turning File transfer off,
/// revoking the bit or unpairing the device ends the transfer.
pub(super) struct Recheck {
    allowed: Box<dyn FnMut() -> Result<(), Failure> + Send>,
    at: Instant,
}
impl Recheck {
    fn new(h: &Shared, c: &Connection, bit: u32, name: &'static str) -> Self {
        let (h, c) = (h.clone(), c.clone());
        Self::with(move || allowed(&h, &c, bit, name).map(drop))
    }
    /// Runs `allowed` again every [`PACE_CHECK`].
    pub(super) fn with(allowed: impl FnMut() -> Result<(), Failure> + Send + 'static) -> Self {
        Self {
            allowed: Box::new(allowed),
            at: Instant::now(),
        }
    }
    fn check(&mut self) -> Result<(), Failure> {
        let now = Instant::now();
        if now.duration_since(self.at) < PACE_CHECK {
            return Ok(());
        }
        self.at = now;
        (self.allowed)()
    }
}
/// Waits out a pacer's `wait`, still checking the device's permission.
async fn paced(wait: Duration, recheck: &mut Recheck) -> Result<(), Failure> {
    let until = Instant::now() + wait;
    loop {
        recheck.check()?;
        let now = Instant::now();
        if now >= until {
            return Ok(());
        }
        tokio::time::sleep((until - now).min(PACE_CHECK)).await;
    }
}
/// [`paced`] on a blocking thread.
fn paced_blocking(wait: Duration, recheck: &mut Recheck) -> Result<(), Failure> {
    let until = Instant::now() + wait;
    loop {
        recheck.check()?;
        let now = Instant::now();
        if now >= until {
            return Ok(());
        }
        std::thread::sleep((until - now).min(PACE_CHECK));
    }
}
fn transfer_dir(h: &Shared) -> Option<PathBuf> {
    h.config
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .file_transfer_dir()
}
/// The transfer folder, as the signed-in user.
#[cfg(not(any(test, feature = "test-support")))]
fn open_folder(dir: Option<&Path>) -> Result<UserFolder, UserFolderError> {
    UserFolder::open(dir)
}
/// Tests open their folder as the developer: an elevated test runner
/// without a desktop shell has no user to act as.
#[cfg(any(test, feature = "test-support"))]
fn open_folder(dir: Option<&Path>) -> Result<UserFolder, UserFolderError> {
    UserFolder::unimpersonated(dir.expect("a test names its transfer folder"))
}

/// The `name` query argument, percent-decoded as RFC 3986 has it: `+` is a
/// plus, not a space. The name itself is checked where the folder opens the
/// file.
fn name_of(query: Option<&str>) -> Result<String, Failure> {
    let invalid = |message: &str| Failure::new(ErrorCode::InvalidName, message);
    let mut found = None;
    for pair in query.unwrap_or_default().split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key != "name" {
            continue;
        }
        if found.is_some() {
            return Err(invalid("Send one name."));
        }
        found =
            Some(decode(value).ok_or_else(|| invalid("The name is not percent-encoded UTF-8."))?);
    }
    found.ok_or_else(|| invalid("Send the file name as ?name=."))
}
pub(super) fn decode(value: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut rest = value.as_bytes();
    while let Some((&byte, tail)) = rest.split_first() {
        rest = tail;
        bytes.push(match byte {
            b'%' => {
                // Two hex digits: `from_str_radix` alone would take a sign, `%+A` as a newline.
                let hex = rest
                    .get(..2)
                    .filter(|h| h.iter().all(u8::is_ascii_hexdigit))?;
                rest = &rest[2..];
                u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?
            }
            byte => byte,
        });
    }
    String::from_utf8(bytes).ok()
}

/// A folder failure as the error the spec gives it. Anything unexpected is
/// logged and answered as `internal`.
pub(super) fn failure(what: &str, error: UserFolderError) -> Failure {
    match error {
        UserFolderError::NoConsoleUser => Failure::new(
            ErrorCode::NoConsoleUser,
            "Nobody is signed in to Windows on this PC. Sign in and try again.",
        ),
        UserFolderError::NoUserShell => {
            tracing::error!(error = %UserFolderError::NoUserShell, "native API: {what}");
            Failure::new(
                ErrorCode::Internal,
                "Rubylight runs as administrator without the Windows desktop, so it can't reach your files. Start it without administrator rights, or use the Windows service.",
            )
            .explained("Rubylight can't reach your files on the PC", None)
        }
        UserFolderError::InvalidName(error) => Failure::new(
            ErrorCode::InvalidName,
            format!("The host refuses this name: {error}."),
        ),
        UserFolderError::Io(error)
            if matches!(
                error.kind(),
                io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded
            ) =>
        {
            no_room()
        }
        UserFolderError::Io(error) if error.raw_os_error() == Some(SHARING_VIOLATION) => {
            Failure::new(
                ErrorCode::TransferBusy,
                "Another program on the PC has this file open. Close it and try again.",
            )
        }
        error => Failure::internal(what, error),
    }
}
fn no_room() -> Failure {
    Failure::new(
        ErrorCode::InsufficientStorage,
        "The PC has no room for this file.",
    )
}
pub(super) async fn blocking<T: Send + 'static>(
    what: &'static str,
    work: impl FnOnce() -> Result<T, UserFolderError> + Send + 'static,
) -> Result<T, Failure> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| Failure::internal(what, error))?
        .map_err(|error| failure(what, error))
}

/// `GET /files`. A device that may only upload gets the free space and the
/// size limit, so it can check a file before it sends it, but no files.
pub(super) async fn list(State(h): State<Shared>, Extension(c): Extension<Connection>) -> Reply {
    let may_list = match allowed(&h, &c, FILE_DOWNLOAD, "Download files") {
        Ok(_) => true,
        Err(refusal) if refusal.code == ErrorCode::Forbidden => {
            allowed(&h, &c, FILE_UPLOAD, "Upload files").map_err(|_| refusal)?;
            false
        }
        Err(refusal) => return Err(refusal.into()),
    };
    let dir = transfer_dir(&h);
    let max_file_bytes = h
        .config
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .file_transfer_max_bytes();
    let list = blocking("list the transfer folder", move || {
        let folder = open_folder(dir.as_deref())?;
        let files = if may_list { folder.list()? } else { vec![] };
        Ok(FileList {
            folder: folder.path().display().to_string(),
            free_bytes: h.file_transfers.room(&folder)?,
            max_file_bytes,
            files: files
                .into_iter()
                .map(|file| FileEntry {
                    name: file.name,
                    size: file.size,
                    modified_unix_ms: file.modified_unix_ms,
                })
                .collect(),
        })
    })
    .await?;
    Ok(json(StatusCode::OK, &list))
}

/// `GET /files/content`: the file with its exact length. A file that changes
/// size while it is read ends the connection before the last byte, so the
/// client never takes a short or grown file for the whole one. So does a
/// client that takes nothing for `idle`, and a device that loses its
/// permission.
pub(super) async fn content(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let client = allowed(&h, &c, FILE_DOWNLOAD, "Download files")?;
    let name = name_of(request.uri().query())?;
    let slot = begin_download(&h, butterpollo_session::state::device_uuid(&client.uuid))?;
    let dir = transfer_dir(&h);
    let opened = tokio::task::spawn_blocking(move || open_folder(dir.as_deref())?.open_file(&name))
        .await
        .map_err(|error| Failure::internal("open the file", error))?;
    let (file, length) = match opened {
        Ok(opened) => opened,
        Err(UserFolderError::NotPlainFile) => return Err(not_found().into()),
        Err(UserFolderError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            return Err(not_found().into());
        }
        Err(error) => return Err(failure("open the file", error).into()),
    };
    let recheck = Recheck::new(&h, &c, FILE_DOWNLOAD, "Download files");
    Ok(send_file(h, slot, file, length, recheck))
}
fn not_found() -> Failure {
    Failure::new(ErrorCode::NotFound, "There is no such file on the PC.")
}

#[cfg(test)]
mod tests;
