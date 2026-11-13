//! File transfer under the native API (`/api/native/v1/files`): list the host's transfer folder,
//! upload into it and download from it. Same pinned HTTPS and client certificate as the rest of
//! the native API; gate the calls on `Feature::FileUpload` and `Feature::FileDownload`.
//!
//! Bodies stream: no call holds a whole file. Every refusal the host documents becomes
//! [`FileError::Refused`], by HTTP status and error code together; anything else is an
//! [`Error`], never a quiet success.

use super::native::error_of;
use super::native_api::PREFIX;
use super::transport::{self, encode_query, Request, Timeouts, Upload};
use super::{Error, HostClient, LONG_CONNECT_TIMEOUT, READ_TIMEOUT};
use serde::{Deserialize, Serialize};
use std::io::{ErrorKind, Read, Write};
use std::time::Duration;

const LIST_PATH: &str = "files";
const CONTENT_PATH: &str = "files/content";
/// A folder of a few thousand files is well under this.
pub(super) const MAX_LIST_BYTES: usize = 4 * 1024 * 1024;
/// An upload's answer, or a refusal.
pub(super) const MAX_JSON_BYTES: usize = 1024 * 1024;
/// The host gives up on a transfer idle for 30 s. The client waits a little longer, in either
/// direction, so that the host's answer comes first.
pub(super) const TRANSFER_TIMEOUT: Duration = Duration::from_secs(35);

/// Cancels one upload or download from another thread, at once, even while it waits on the
/// host: the transfer's socket is shut down. One per transfer, cheap to share as an `Arc`; it
/// leaves the client's other requests alone, unlike `HostClient::cancel_pending_requests`.
#[derive(Default)]
pub struct TransferCancel(pub(super) transport::Cancel);

impl TransferCancel {
    pub fn new() -> Self {
        Self::default()
    }

    /// The transfer fails with `Error::Cancelled`, and so does any transfer started with this
    /// handle later.
    pub fn cancel(&self) {
        self.0.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
}

/// One file in the host's transfer folder.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RemoteFile {
    pub name: String,
    pub size: u64,
    pub modified_unix_ms: u64,
}

/// The host's transfer folder.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct FileList {
    /// Where the files land on the host, for display.
    pub folder: String,
    pub free_bytes: u64,
    /// The largest upload the host takes.
    pub max_file_bytes: u64,
    pub files: Vec<RemoteFile>,
}

/// A finished upload: the name the host stored it under, which gains " (n)" when the name was
/// taken, and its size.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Uploaded {
    pub name: String,
    pub size: u64,
}

/// A refusal the host documents, by HTTP status and error code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileRefusal {
    /// 403 `forbidden`: this device lacks the permission.
    Forbidden,
    /// 403 `file_transfer_disabled`: file transfer is off on the host.
    FileTransferDisabled,
    /// 400 `invalid_name`.
    InvalidName,
    /// 400 `incomplete_upload`: the body ended before its Content-Length.
    IncompleteUpload,
    /// 411 `length_required`.
    LengthRequired,
    /// 413 `too_large`: over `max_file_bytes`.
    TooLarge,
    /// 507 `insufficient_storage`.
    InsufficientStorage,
    /// 409 `no_console_user`: nobody is signed in on the host.
    NoConsoleUser,
    /// 409 `transfer_busy`: another upload from this device is running.
    TransferBusy,
    /// 404 `not_found`: no such file.
    NotFound,
    /// 403 `folders_disabled`: the host shares no folders.
    FoldersDisabled,
    /// 403 `folder_unreadable`: Windows denies the host's signed-in user reading the shared
    /// folder, or a folder or file in it.
    FolderUnreadable,
}

impl FileRefusal {
    const ALL: [(u16, &'static str, FileRefusal); 12] = [
        (403, "forbidden", FileRefusal::Forbidden),
        (
            403,
            "file_transfer_disabled",
            FileRefusal::FileTransferDisabled,
        ),
        (400, "invalid_name", FileRefusal::InvalidName),
        (400, "incomplete_upload", FileRefusal::IncompleteUpload),
        (411, "length_required", FileRefusal::LengthRequired),
        (413, "too_large", FileRefusal::TooLarge),
        (
            507,
            "insufficient_storage",
            FileRefusal::InsufficientStorage,
        ),
        (409, "no_console_user", FileRefusal::NoConsoleUser),
        (409, "transfer_busy", FileRefusal::TransferBusy),
        (404, "not_found", FileRefusal::NotFound),
        (403, "folders_disabled", FileRefusal::FoldersDisabled),
        (403, "folder_unreadable", FileRefusal::FolderUnreadable),
    ];

    fn from_answer(status: u16, code: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .find(|(s, c, _)| *s == status && *c == code)
            .map(|(_, _, refusal)| *refusal)
    }

    /// The host's error code.
    pub fn code(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(_, _, refusal)| *refusal == self)
            .map_or("", |(_, code, _)| code)
    }
}

/// Why a file call gave no success.
#[derive(Debug)]
pub enum FileError {
    /// The host answered with a documented refusal.
    Refused {
        refusal: FileRefusal,
        status: u16,
        message: String,
    },
    /// Reading the file to upload, or writing the download, failed on this device.
    Local(std::io::Error),
    Host(Error),
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileError::Refused {
                status, message, ..
            } => write!(f, "host error {status}: {message}"),
            FileError::Local(e) => write!(f, "local file: {e}"),
            FileError::Host(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for FileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FileError::Local(e) => Some(e),
            FileError::Host(e) => Some(e),
            FileError::Refused { .. } => None,
        }
    }
}

impl From<Error> for FileError {
    fn from(e: Error) -> Self {
        FileError::Host(e)
    }
}

/// The error body with the code as the host sent it: the pinned `ErrorCode` has no file codes.
#[derive(Deserialize)]
struct ErrorBody {
    error: RawError,
}

#[derive(Deserialize)]
struct RawError {
    code: String,
    #[serde(default)]
    message: String,
}

/// A non-2xx answer: a documented refusal, or the native API's error for anything else.
fn refusal(response: transport::Response, path: &str) -> FileError {
    if let Ok(ErrorBody { error }) = serde_json::from_slice(&response.body) {
        if let Some(refusal) = FileRefusal::from_answer(response.status, &error.code) {
            return FileError::Refused {
                refusal,
                status: response.status,
                message: error.message,
            };
        }
    }
    FileError::Host(error_of(response, path))
}

/// Reads an answer to `path`: `success` is the one status that is not a refusal.
pub(super) fn decode<T: serde::de::DeserializeOwned>(
    response: transport::Response,
    path: &str,
    success: u16,
) -> Result<T, FileError> {
    if response.status == success {
        return serde_json::from_slice(&response.body)
            .map_err(|e| Error::Malformed(format!("native API {path}: {e}")).into());
    }
    if (200..=299).contains(&response.status) {
        return Err(Error::Status {
            code: i32::from(response.status),
            message: response.reason,
        }
        .into());
    }
    Err(refusal(response, path))
}

/// Reads an upload's answer: 201, and the host stored as many bytes as were sent.
pub(super) fn uploaded(response: transport::Response, path: &str, length: u64) -> Result<Uploaded, FileError> {
    let uploaded: Uploaded = decode(response, path, 201)?;
    if uploaded.size != length {
        return Err(Error::Malformed(format!(
            "native API {path}: the host stored {} of {length} bytes",
            uploaded.size
        ))
        .into());
    }
    Ok(uploaded)
}

/// Reads a download's answer: only a 200 delivered the `length` bytes in the sink.
pub(super) fn downloaded(response: transport::Response, path: &str, length: u64) -> Result<u64, FileError> {
    match response.status {
        200 => Ok(length),
        201..=299 => Err(Error::Status {
            code: i32::from(response.status),
            message: response.reason,
        }
        .into()),
        _ => Err(refusal(response, path)),
    }
}

/// The local side of a transfer. Keeps its first error, so that a full disk is not taken for a
/// network failure.
pub(super) struct Local<T> {
    inner: T,
    error: Option<std::io::Error>,
}

impl<T> Local<T> {
    pub(super) fn new(inner: T) -> Self {
        Self { inner, error: None }
    }

    fn keep(&mut self, error: std::io::Error) -> std::io::Error {
        if error.kind() == ErrorKind::Interrupted {
            return error;
        }
        let copy = std::io::Error::new(error.kind(), error.to_string());
        self.error.get_or_insert(error);
        copy
    }

    /// `error`, unless the local side failed first. A cancel wins: what failed after it does
    /// not matter.
    pub(super) fn or(self, error: Error) -> FileError {
        match (self.error, error) {
            (_, Error::Cancelled) => FileError::Host(Error::Cancelled),
            (Some(local), _) => FileError::Local(local),
            (None, error) => FileError::Host(error),
        }
    }
}

impl<R: Read> Read for Local<R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(bytes).map_err(|e| self.keep(e))
    }
}

impl<W: Write> Write for Local<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.inner.write(bytes).map_err(|e| self.keep(e))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush().map_err(|e| self.keep(e))
    }
}

impl HostClient {
    pub(super) fn files_request<'a>(
        &'a self,
        target: &'a str,
        limit: usize,
        read: Duration,
    ) -> Result<Request<'a>, Error> {
        Ok(Request {
            host: &self.host,
            port: self.https_port(true)?,
            target,
            tls: Some(self.tls()?),
            timeouts: Timeouts {
                connect: LONG_CONNECT_TIMEOUT,
                read: Some(read),
            },
            body_limit: limit,
        })
    }

    fn files_target(&self, path: &str, name: Option<&str>) -> (String, String) {
        let path = format!("{}/{path}", PREFIX.trim_start_matches('/'));
        let query = name.map(|name| format!("name={}", encode_query(name)));
        let target = self.url_target(&path, query.as_deref());
        (path, target)
    }

    /// The host's transfer folder. Needs the device's `file_download` permission.
    pub fn list_files(&self) -> Result<FileList, FileError> {
        let (path, target) = self.files_target(LIST_PATH, None);
        let request = self.files_request(&target, MAX_LIST_BYTES, READ_TIMEOUT)?;
        decode(transport::get(&request, &self.cancel)?, &path, 200)
    }

    /// Uploads exactly `length` bytes of `file` as `name`, reporting progress at 0, at most
    /// every 256 KiB and 100 ms, and at `length`. `stop` ends the upload with `Error::Cancelled`;
    /// the host then drops the partial file. Needs the device's `file_upload` permission.
    pub fn upload_file(
        &self,
        name: &str,
        file: &mut dyn Read,
        length: u64,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<Uploaded, FileError> {
        let (path, target) = self.files_target(LIST_PATH, Some(name));
        let request = self.files_request(&target, MAX_JSON_BYTES, TRANSFER_TIMEOUT)?;
        let mut file = Local::new(file);
        let upload = Upload {
            reader: &mut file,
            length,
            content_type: "application/octet-stream",
            method: "POST",
        };
        match transport::upload(&request, &self.cancel, upload, progress, &stop.0) {
            Ok(response) => uploaded(response, &path, length),
            Err(error) => Err(file.or(error)),
        }
    }

    /// Downloads `name`, which the list gave as `listed` bytes, into `sink` and returns its
    /// length, reporting progress like `upload_file`. A body shorter than its Content-Length is
    /// an error, as is one longer than `listed` or cancelling `stop`; the caller then discards
    /// what `sink` got. Needs the device's `file_download` permission.
    pub fn download_file(
        &self,
        name: &str,
        listed: u64,
        sink: &mut dyn Write,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<u64, FileError> {
        let (path, target) = self.files_target(CONTENT_PATH, Some(name));
        let request = self.files_request(&target, MAX_JSON_BYTES, TRANSFER_TIMEOUT)?;
        download_listed(&request, &self.cancel, &path, listed, sink, progress, stop)
    }
}

/// A download's sink that takes no more than the size the listing gave, so a host that sends
/// more stops at that size instead of filling the disk.
struct Listed<'a> {
    inner: &'a mut dyn Write,
    left: u64,
    over: bool,
}

impl Write for Listed<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > self.left {
            self.over = true;
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                "more than the listed size",
            ));
        }
        let written = self.inner.write(bytes)?;
        self.left -= written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Runs the download `request` of `path`, listed at `listed` bytes, into `sink`: what
/// [`HostClient::download_file`] and `download_folder_file` share.
pub(super) fn download_listed(
    request: &Request<'_>,
    cancel: &transport::Cancel,
    path: &str,
    listed: u64,
    sink: &mut dyn Write,
    progress: &mut dyn FnMut(u64, u64),
    stop: &TransferCancel,
) -> Result<u64, FileError> {
    let mut sink = Local::new(Listed {
        inner: sink,
        left: listed,
        over: false,
    });
    let mut length = 0;
    let mut counted = |done, total| {
        length = done;
        progress(done, total);
    };
    match transport::download(request, cancel, &mut sink, &mut counted, &stop.0) {
        Ok(response) => downloaded(response, path, length),
        Err(_) if sink.inner.over => Err(Error::Malformed(format!(
            "native API {path}: the host sent more than the {listed} bytes it listed"
        ))
        .into()),
        Err(error) => Err(sink.or(error)),
    }
}

#[cfg(test)]
mod tests;
