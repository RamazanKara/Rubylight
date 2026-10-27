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

/// Cancels one upload or download from another thread, at once, even while it waits on the
/// host: the transfer's socket is shut down. One per transfer, cheap to share as an `Arc`; it
/// leaves the client's other requests alone, unlike `HostClient::cancel_pending_requests`.
#[derive(Default)]
pub struct TransferCancel(transport::Cancel);

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
    /// 409 `transfer_busy`: another upload from this device is running.
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

/// Reads an upload's answer: 201, and the host stored as many bytes as were sent.
fn uploaded(response: transport::Response, path: &str, length: u64) -> Result<Uploaded, FileError> {
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
fn downloaded(response: transport::Response, path: &str, length: u64) -> Result<u64, FileError> {
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

    /// `error`, unless the local side failed first. A cancel wins: what failed after it does
    /// not matter.
    fn or(self, error: Error) -> FileError {
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
        };
        match transport::upload(&request, &self.cancel, upload, progress, &stop.0) {
            Ok(response) => uploaded(response, &path, length),
            Err(error) => Err(file.or(error)),
        }
    }

    /// Downloads `name` into `sink` and returns its length, reporting progress like
    /// `upload_file`. A body shorter than its Content-Length is an error, as is cancelling `stop`; the
    /// caller then discards what `sink` got. Needs the device's `file_download` permission.
    pub fn download_file(
        &self,
        name: &str,
        sink: &mut dyn Write,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<u64, FileError> {
        let (path, target) = self.files_target(CONTENT_PATH, Some(name));
        let request = self.files_request(&target, MAX_JSON_BYTES, TRANSFER_TIMEOUT)?;
        let mut sink = Local::new(sink);
        let mut length = 0;
        let mut counted = |done, total| {
            length = done;
            progress(done, total);
        };
        match transport::download(&request, &self.cancel, &mut sink, &mut counted, &stop.0) {
            Ok(response) => downloaded(response, &path, length),
            Err(error) => Err(sink.or(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::native_api::error::ErrorCode;

    fn response(status: u16, body: &str) -> transport::Response {
        transport::Response {
            status,
            reason: "Reason".into(),
            content_type: Some("application/json".into()),
            body: body.as_bytes().to_vec(),
        }
    }

    fn error(status: u16, code: &str) -> transport::Response {
        response(
            status,
            &format!(r#"{{"error":{{"code":"{code}","message":"Host says {code}"}}}}"#),
        )
    }

    /// The design's error table, row by row.
    const TABLE: [(u16, &str, FileRefusal); 10] = [
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

    #[test]
    fn every_documented_refusal_is_typed_for_every_call() {
        for (status, code, expected) in TABLE {
            let answers: [Result<(), FileError>; 3] = [
                decode::<FileList>(error(status, code), LIST_PATH, 200).map(drop),
                decode::<Uploaded>(error(status, code), LIST_PATH, 201).map(drop),
                downloaded(error(status, code), CONTENT_PATH, 0).map(drop),
            ];
            for answer in answers {
                match answer {
                    Err(FileError::Refused {
                        refusal,
                        status: got,
                        message,
                    }) => {
                        assert_eq!((refusal, got), (expected, status), "{code}");
                        assert_eq!(message, format!("Host says {code}"));
                    }
                    other => panic!("{status} {code}: {other:?}"),
                }
            }
            assert_eq!(expected.code(), code);
        }
    }

    #[test]
    fn fields_the_spec_does_not_name_are_ignored() {
        // The native error body's optional fields; a busy host gives no retry time.
        let body = r#"{"error":{"code":"transfer_busy","message":"busy","setting":null,"retry_after_ms":1500}}"#;
        match decode::<Uploaded>(response(409, body), LIST_PATH, 201) {
            Err(FileError::Refused {
                refusal: FileRefusal::TransferBusy,
                status: 409,
                message,
            }) => assert_eq!(message, "busy"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_undocumented_answer_is_a_loud_host_error() {
        // A known code under another status, a code from elsewhere in the API, and no JSON.
        let answers = [
            (error(403, "not_found"), Some(ErrorCode::NotFound)),
            (error(400, "too_large"), Some(ErrorCode::Unknown)),
            (error(409, "session_busy"), Some(ErrorCode::SessionBusy)),
            (error(500, "internal"), Some(ErrorCode::Internal)),
            (error(418, "teapot"), Some(ErrorCode::Unknown)),
            (response(500, "<html>oops</html>"), None),
            (response(413, ""), None),
        ];
        for (answer, code) in answers {
            let status = answer.status;
            let error = decode::<Uploaded>(answer, LIST_PATH, 201).unwrap_err();
            match (&error, code) {
                (FileError::Host(Error::Api { status: got, error }), Some(code)) => {
                    assert_eq!((*got, error.code), (status, code));
                }
                (FileError::Host(Error::Status { code, .. }), None) => {
                    assert_eq!(*code, i32::from(status));
                }
                _ => panic!("{status}: {error:?}"),
            }
        }
        // A 404 without the native error body names the path.
        match downloaded(response(404, ""), CONTENT_PATH, 0) {
            Err(FileError::Host(Error::NotFound(path))) => {
                assert_eq!(path, format!("/{CONTENT_PATH}"))
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_upload_needs_201_and_its_answer() {
        let body = r#"{"name":"photo (1).jpg","size":5}"#;
        assert_eq!(
            uploaded(response(201, body), LIST_PATH, 5).unwrap(),
            Uploaded {
                name: "photo (1).jpg".into(),
                size: 5
            }
        );
        // The host stored another size than was sent.
        for length in [0, 4, 6] {
            match uploaded(response(201, body), LIST_PATH, length) {
                Err(FileError::Host(Error::Malformed(m))) => {
                    assert_eq!(
                        m,
                        format!("native API files: the host stored 5 of {length} bytes")
                    )
                }
                other => panic!("{length}: {other:?}"),
            }
        }
        for status in [200, 202, 204, 299] {
            match uploaded(response(status, body), LIST_PATH, 5) {
                Err(FileError::Host(Error::Status { code, .. })) => {
                    assert_eq!(code, i32::from(status))
                }
                other => panic!("{status}: {other:?}"),
            }
        }
        for broken in ["", "{}", r#"{"name":"a"}"#, r#"{"name":"a","size":-1}"#] {
            assert!(
                matches!(
                    uploaded(response(201, broken), LIST_PATH, 1),
                    Err(FileError::Host(Error::Malformed(_)))
                ),
                "{broken}"
            );
        }
    }

    #[test]
    fn a_download_needs_200() {
        assert_eq!(downloaded(response(200, ""), CONTENT_PATH, 7).unwrap(), 7);
        for status in [201, 204, 206, 299] {
            match downloaded(response(status, ""), CONTENT_PATH, 7) {
                Err(FileError::Host(Error::Status { code, .. })) => {
                    assert_eq!(code, i32::from(status))
                }
                other => panic!("{status}: {other:?}"),
            }
        }
    }

    #[test]
    fn a_listing_needs_200_and_its_fields() {
        let body = r#"{"folder":"C:\\Users\\u\\Downloads\\Rubylight","free_bytes":10,
            "max_file_bytes":4294967296,"files":[{"name":"a.txt","size":3,"modified_unix_ms":1}]}"#;
        let list = decode::<FileList>(response(200, body), LIST_PATH, 200).unwrap();
        assert_eq!(list.folder, r"C:\Users\u\Downloads\Rubylight");
        assert_eq!(list.max_file_bytes, 4 << 30);
        assert_eq!(
            list.files,
            [RemoteFile {
                name: "a.txt".into(),
                size: 3,
                modified_unix_ms: 1
            }]
        );
        assert!(matches!(
            decode::<FileList>(response(201, body), LIST_PATH, 200),
            Err(FileError::Host(Error::Status { code: 201, .. }))
        ));
        assert!(matches!(
            decode::<FileList>(response(200, r#"{"folder":"x"}"#), LIST_PATH, 200),
            Err(FileError::Host(Error::Malformed(_)))
        ));
    }

    /// A file the transfer may not write.
    struct Full;

    impl Write for Full {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(ErrorKind::PermissionDenied.into())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_cancel_wins_over_whatever_failed_after_it() {
        let failed_disk = || {
            let mut sink = Local::new(Full);
            assert!(sink.write_all(b"x").is_err());
            sink
        };
        assert!(matches!(
            failed_disk().or(Error::Cancelled),
            FileError::Host(Error::Cancelled)
        ));
        assert!(matches!(
            failed_disk().or(Error::Malformed("cut short".into())),
            FileError::Local(e) if e.kind() == ErrorKind::PermissionDenied
        ));
        assert!(matches!(
            Local::new(std::io::empty()).or(Error::Malformed("cut short".into())),
            FileError::Host(Error::Malformed(_))
        ));
        // One handle per transfer, cancelled from another thread.
        let stop = std::sync::Arc::new(TransferCancel::new());
        let other = stop.clone();
        std::thread::spawn(move || other.cancel()).join().unwrap();
        assert!(stop.is_cancelled());
    }
}
