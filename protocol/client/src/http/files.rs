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
use serde::Deserialize;
use std::io::{ErrorKind, Read, Write};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

const LIST_PATH: &str = "files";
const CONTENT_PATH: &str = "files/content";
/// A folder of a few thousand files is well under this.
const MAX_LIST_BYTES: usize = 4 * 1024 * 1024;
/// An upload's answer, or a refusal.
const MAX_JSON_BYTES: usize = 1024 * 1024;
/// The host gives up on a transfer idle for 30 s. The client waits a little longer, in either
/// direction, so that the host's answer comes first.
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(35);

/// One file in the host's transfer folder.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct RemoteFile {
    pub name: String,
    pub size: u64,
    pub modified_unix_ms: u64,
}

/// The host's transfer folder.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
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
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
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
    /// 409 `transfer_busy`: another transfer is running; see `retry_after_ms`.
    TransferBusy,
    /// 404 `not_found`: no such file.
    NotFound,
}

impl FileRefusal {
    const ALL: [(u16, &'static str, FileRefusal); 10] = [
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
        retry_after_ms: Option<u64>,
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
    #[serde(default)]
    retry_after_ms: Option<u64>,
}

/// A non-2xx answer: a documented refusal, or the native API's error for anything else.
fn refusal(response: transport::Response, path: &str) -> FileError {
    if let Ok(ErrorBody { error }) = serde_json::from_slice(&response.body) {
        if let Some(refusal) = FileRefusal::from_answer(response.status, &error.code) {
            return FileError::Refused {
                refusal,
                status: response.status,
                message: error.message,
                retry_after_ms: error.retry_after_ms,
            };
        }
    }
    FileError::Host(error_of(response, path))
}

/// Reads an answer to `path`: `success` is the one status that is not a refusal.
fn decode<T: serde::de::DeserializeOwned>(
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

/// The local side of a transfer. Keeps its first error, so that a full disk is not taken for a
/// network failure.
struct Local<T> {
    inner: T,
    error: Option<std::io::Error>,
}

impl<T> Local<T> {
    fn new(inner: T) -> Self {
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

    /// `error`, unless the local side failed first.
    fn or(self, error: Error) -> FileError {
        match self.error {
            Some(local) => FileError::Local(local),
            None => FileError::Host(error),
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
    fn files_request<'a>(
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

    /// Uploads exactly `length` bytes of `file` as `name`, reporting progress after every
    /// chunk. Setting `stop` ends the upload before its next chunk with `Error::Cancelled`; the
    /// host then drops the partial file. Needs the device's `file_upload` permission.
    pub fn upload_file(
        &self,
        name: &str,
        file: &mut dyn Read,
        length: u64,
        progress: &mut dyn FnMut(u64, u64),
        stop: &AtomicBool,
    ) -> Result<Uploaded, FileError> {
        let (path, target) = self.files_target(LIST_PATH, Some(name));
        let request = self.files_request(&target, MAX_JSON_BYTES, TRANSFER_TIMEOUT)?;
        let mut file = Local::new(file);
        let upload = Upload {
            reader: &mut file,
            length,
            content_type: "application/octet-stream",
        };
        match transport::upload(&request, &self.cancel, upload, progress, stop) {
            Ok(response) => decode(response, &path, 201),
            Err(error) => Err(file.or(error)),
        }
    }

    /// Downloads `name` into `sink` and returns its length, reporting progress after every
    /// chunk. A body shorter than its Content-Length is an error, as is setting `stop`; the
    /// caller then discards what `sink` got. Needs the device's `file_download` permission.
    pub fn download_file(
        &self,
        name: &str,
        sink: &mut dyn Write,
        progress: &mut dyn FnMut(u64, u64),
        stop: &AtomicBool,
    ) -> Result<u64, FileError> {
        let (path, target) = self.files_target(CONTENT_PATH, Some(name));
        let request = self.files_request(&target, MAX_JSON_BYTES, TRANSFER_TIMEOUT)?;
        let mut sink = Local::new(sink);
        let mut length = 0;
        let mut counted = |done, total| {
            length = done;
            progress(done, total);
        };
        match transport::download(&request, &self.cancel, &mut sink, &mut counted, stop) {
            Ok(response) if response.status == 200 => Ok(length),
            Ok(response) if (200..=299).contains(&response.status) => Err(Error::Status {
                code: i32::from(response.status),
                message: response.reason,
            }
            .into()),
            Ok(response) => Err(refusal(response, &path)),
            Err(error) => Err(sink.or(error)),
        }
    }
}
