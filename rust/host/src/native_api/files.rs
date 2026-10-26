//! File transfer (`docs/native-api.md`, "Files"): a paired device sends
//! files to the PC's transfer folder and fetches them back.
//!
//! Every route needs the host's File transfer setting and the device's
//! Upload files or Download files bit, read on each request. Files are opened
//! as the signed-in Windows user on a blocking thread
//! (`butterpollo_windows::user_folder`). An upload streams to its `.part`
//! file, written at `file_transfer_stream_mbps` while a stream runs, and is
//! renamed only once all of its `Content-Length` arrived.
use super::{Failure, Reply, caller, json};
use crate::{state::Shared, tls::Connection};
use axum::{
    Extension,
    body::{Body, Bytes, HttpBody},
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use butterpollo_core::{
    file_transfer::Pacer,
    native::{FILE_DOWNLOAD, FILE_UPLOAD},
    native_api::{
        error::ErrorCode,
        files::{FileEntry, FileList, Uploaded},
    },
    state::Client,
};
use butterpollo_windows::user_folder::{PartFile, UserFolder, UserFolderError};
use std::{
    collections::HashSet,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Mutex, PoisonError},
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::{
    io::ReadBuf,
    sync::{mpsc, oneshot},
};

/// An upload whose body sends nothing for this long is dropped.
const IDLE: Duration = Duration::from_secs(30);
/// Bytes read from disk per download chunk.
const CHUNK: usize = 64 * 1024;
/// Chunks queued between the socket and the disk.
const QUEUE: usize = 8;
/// What a paced upload may write at once after a pause.
const PACE_BURST: u64 = 256 * 1024;
/// How often a running upload looks whether a stream started or ended.
const PACE_CHECK: Duration = Duration::from_millis(250);

/// The uploads running: one per device, all paced together.
#[derive(Debug)]
pub struct Uploads {
    running: Mutex<HashSet<String>>,
    pacing: Mutex<Pacing>,
}
impl Default for Uploads {
    fn default() -> Self {
        Self {
            running: Mutex::default(),
            pacing: Mutex::new(Pacing {
                pacer: Pacer::new(None, PACE_BURST, Instant::now()),
                checked: None,
            }),
        }
    }
}
/// One pacer for every upload, so together they stay within the stream rate.
#[derive(Debug)]
struct Pacing {
    pacer: Pacer,
    /// When the pacer last looked whether a stream runs.
    checked: Option<Instant>,
}
impl Uploads {
    /// How long to wait before writing `bytes`: unpaced unless a stream runs.
    fn pace(&self, h: &Shared, rate: Option<u64>, bytes: u64) -> Duration {
        let now = Instant::now();
        let mut pacing = self.pacing.lock().unwrap_or_else(PoisonError::into_inner);
        if pacing
            .checked
            .is_none_or(|at| now.duration_since(at) >= PACE_CHECK)
        {
            pacing.checked = Some(now);
            let want = if streaming(h) { rate } else { None };
            if pacing.pacer.rate() != want {
                pacing.pacer.set_rate(want, now);
            }
        }
        pacing.pacer.take(bytes, now)
    }
}
/// A device's upload slot, given back when the upload ends however it ends.
/// The upload's writer holds it, so the slot frees only once the `.part`
/// file is renamed or gone.
struct Slot {
    host: Shared,
    device: String,
}
impl Drop for Slot {
    fn drop(&mut self) {
        self.host
            .file_uploads
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.device);
    }
}
fn begin_upload(h: &Shared, device: &str) -> Result<Slot, Failure> {
    let mut running = h
        .file_uploads
        .running
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if !running.insert(device.to_owned()) {
        return Err(Failure::new(
            ErrorCode::TransferBusy,
            "This device is already sending a file. Wait for it to finish.",
        ));
    }
    Ok(Slot {
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
fn transfer_dir(h: &Shared) -> Option<PathBuf> {
    h.config
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .file_transfer_dir()
}
/// The transfer folder, as the signed-in user. Tests open theirs as the
/// developer: an elevated test runner without a desktop shell has no user
/// to act as.
fn open_folder(dir: Option<&Path>) -> Result<UserFolder, UserFolderError> {
    #[cfg(not(test))]
    return UserFolder::open(dir);
    #[cfg(test)]
    UserFolder::unimpersonated(dir.expect("a test names its transfer folder"))
}

/// The `name` query argument, percent-decoded. The name itself is checked
/// where the folder opens the file.
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
fn decode(value: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut rest = value.as_bytes();
    while let Some((&byte, tail)) = rest.split_first() {
        rest = tail;
        bytes.push(match byte {
            b'%' => {
                let hex = std::str::from_utf8(rest.get(..2)?).ok()?;
                rest = &rest[2..];
                u8::from_str_radix(hex, 16).ok()?
            }
            b'+' => b' ',
            byte => byte,
        });
    }
    String::from_utf8(bytes).ok()
}

/// A folder failure as the error the spec gives it. Anything unexpected is
/// logged and answered as `internal`.
fn failure(what: &str, error: UserFolderError) -> Failure {
    match error {
        UserFolderError::NoConsoleUser => Failure::new(
            ErrorCode::NoConsoleUser,
            "Nobody is signed in to Windows on this PC. Sign in and try again.",
        ),
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
        error => Failure::internal(what, error),
    }
}
fn no_room() -> Failure {
    Failure::new(
        ErrorCode::InsufficientStorage,
        "The PC has no room for this file.",
    )
}
async fn blocking<T: Send + 'static>(
    what: &'static str,
    work: impl FnOnce() -> Result<T, UserFolderError> + Send + 'static,
) -> Result<T, Failure> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| Failure::internal(what, error))?
        .map_err(|error| failure(what, error))
}

/// `GET /files`.
pub(super) async fn list(State(h): State<Shared>, Extension(c): Extension<Connection>) -> Reply {
    allowed(&h, &c, FILE_DOWNLOAD, "Download files")?;
    let dir = transfer_dir(&h);
    let max_file_bytes = h
        .config
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .file_transfer_max_bytes();
    let list = blocking("list the transfer folder", move || {
        let folder = open_folder(dir.as_deref())?;
        Ok(FileList {
            folder: folder.path().display().to_string(),
            free_bytes: folder.free_bytes()?,
            max_file_bytes,
            files: folder
                .list()?
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
/// client never takes a short or grown file for the whole one.
pub(super) async fn content(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    allowed(&h, &c, FILE_DOWNLOAD, "Download files")?;
    let name = name_of(request.uri().query())?;
    let dir = transfer_dir(&h);
    let opened =
        tokio::task::spawn_blocking(move || open_folder(dir.as_deref())?.open_file(&name))
            .await
            .map_err(|error| Failure::internal("open the file", error))?;
    let (file, length) = match opened {
        Ok(opened) => opened,
        Err(UserFolderError::NotPlainFile) => return Err(not_found()),
        Err(UserFolderError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            return Err(not_found());
        }
        Err(error) => return Err(failure("open the file", error)),
    };
    let (tx, rx) = mpsc::channel(QUEUE);
    tokio::task::spawn_blocking(move || {
        let mut file = file;
        let sent = read_exactly(&mut file, length, |chunk| {
            tx.blocking_send(Ok(chunk)).is_ok()
        });
        if let Err(error) = sent {
            tracing::warn!("File transfer stopped a download: {error}");
            let _ = tx.blocking_send(Err(error));
        }
    });
    let body = Body::from_stream(tokio_util::io::ReaderStream::with_capacity(
        Chunks {
            rx,
            current: Bytes::new(),
        },
        CHUNK,
    ));
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_owned()),
            (header::CONTENT_LENGTH, length.to_string()),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        body,
    )
        .into_response())
}
fn not_found() -> Failure {
    Failure::new(ErrorCode::NotFound, "There is no such file on the PC.")
}

/// Hands `length` bytes of `file` to `send`, and fails if the file holds
/// fewer or more. The last chunk waits until the end of the file is seen.
/// Stops quietly when `send` reports that the client went away.
fn read_exactly(
    file: &mut impl Read,
    length: u64,
    mut send: impl FnMut(Bytes) -> bool,
) -> io::Result<()> {
    let changed = |how: &str| io::Error::other(format!("the file {how} while it was sent"));
    let mut left = length;
    let mut buffer = vec![0; CHUNK];
    loop {
        let read = match file.read(&mut buffer) {
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if read == 0 {
            return if left == 0 {
                Ok(())
            } else {
                Err(changed("shrank"))
            };
        }
        if read as u64 > left {
            return Err(changed("grew"));
        }
        left -= read as u64;
        if left == 0 {
            let mut probe = [0; 1];
            let more = loop {
                match file.read(&mut probe) {
                    Ok(more) => break more,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error),
                }
            };
            if more != 0 {
                return Err(changed("grew"));
            }
        }
        if !send(Bytes::copy_from_slice(&buffer[..read])) || left == 0 {
            return Ok(());
        }
    }
}
/// The download's chunks as a reader; an error ends the body with it.
struct Chunks {
    rx: mpsc::Receiver<io::Result<Bytes>>,
    current: Bytes,
}
impl tokio::io::AsyncRead for Chunks {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        while this.current.is_empty() {
            match std::task::ready!(this.rx.poll_recv(cx)) {
                Some(Ok(chunk)) => this.current = chunk,
                Some(Err(error)) => return Poll::Ready(Err(error)),
                None => return Poll::Ready(Ok(())),
            }
        }
        let n = buf.remaining().min(this.current.len());
        buf.put_slice(&this.current.split_to(n));
        Poll::Ready(Ok(()))
    }
}

/// `POST /files?name=`. A refusal answers at once; the rest of the body is
/// then read and dropped in the background, so the client, which sends all
/// of it before it reads the answer, gets the answer rather than a reset.
pub(super) async fn upload(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Response {
    let (parts, mut body) = request.into_parts();
    let reply = receive(&h, &c, parts.uri.query(), &parts.headers, &mut body).await;
    if !body.is_end_stream() {
        tokio::spawn(drain(body));
    }
    match reply {
        Ok(response) => response,
        Err(failure) => {
            tracing::warn!(
                "File transfer refused an upload from {}: {}",
                c.peer.ip(),
                failure.message
            );
            failure.into_response()
        }
    }
}
async fn drain(mut body: Body) {
    while let Ok(Some(Ok(_))) = tokio::time::timeout(IDLE, next_chunk(&mut body)).await {}
}
async fn next_chunk(body: &mut Body) -> Option<Result<Bytes, axum::Error>> {
    loop {
        match std::future::poll_fn(|cx| Pin::new(&mut *body).poll_frame(cx)).await? {
            Ok(frame) => {
                if let Ok(data) = frame.into_data()
                    && !data.is_empty()
                {
                    return Some(Ok(data));
                }
            }
            Err(error) => return Some(Err(error)),
        }
    }
}
fn content_length(headers: &HeaderMap) -> Result<u64, Failure> {
    let value = headers.get(header::CONTENT_LENGTH).ok_or_else(|| {
        Failure::new(
            ErrorCode::LengthRequired,
            "Send the file with a Content-Length.",
        )
    })?;
    value
        .to_str()
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .ok_or_else(|| {
            Failure::new(
                ErrorCode::InvalidRequest,
                "The Content-Length is not a number.",
            )
        })
}
fn incomplete(message: impl Into<String>) -> Failure {
    Failure::new(ErrorCode::IncompleteUpload, message)
}
/// Whether a stream runs or starts, so uploads leave it the link and disk.
fn streaming(h: &Shared) -> bool {
    let sessions = h.sessions.lock().unwrap_or_else(PoisonError::into_inner);
    !sessions.active.is_empty() || !sessions.pending.is_empty()
}

async fn receive(
    h: &Shared,
    c: &Connection,
    query: Option<&str>,
    headers: &HeaderMap,
    body: &mut Body,
) -> Reply {
    let client = allowed(h, c, FILE_UPLOAD, "Upload files")?;
    let name = name_of(query)?;
    butterpollo_core::file_transfer::check_name(&name)
        .map_err(|error| failure("check the name", error.into()))?;
    let length = content_length(headers)?;
    let (max, rate, dir) = {
        let config = h.config.read().unwrap_or_else(PoisonError::into_inner);
        (
            config.file_transfer_max_bytes(),
            config.file_transfer_stream_rate(),
            config.file_transfer_dir(),
        )
    };
    if length > max {
        return Err(Failure::new(
            ErrorCode::TooLarge,
            format!(
                "The file is larger than the {} MiB this PC accepts.",
                max >> 20
            ),
        ));
    }
    let slot = begin_upload(h, &client.uuid)?;
    let (ready_tx, ready) = oneshot::channel();
    let (tx, rx) = mpsc::channel(QUEUE);
    let writer =
        tokio::task::spawn_blocking(move || write_part(slot, dir, name, length, ready_tx, rx));
    match ready.await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(failure("start the upload", error)),
        Err(_) => {
            return Err(match writer.await {
                Err(error) => Failure::internal("start the upload", error),
                Ok(_) => Failure::internal("start the upload", "the writer stopped"),
            });
        }
    }

    let mut received = 0u64;
    let read: Result<(), Failure> = loop {
        let chunk = match tokio::time::timeout(IDLE, next_chunk(body)).await {
            Err(_) => {
                break Err(incomplete(format!(
                    "The upload stalled after {received} of {length} bytes."
                )));
            }
            Ok(None) if received == length => break Ok(()),
            Ok(None) => {
                break Err(incomplete(format!(
                    "The upload ended after {received} of {length} bytes."
                )));
            }
            Ok(Some(Err(error))) => {
                break Err(incomplete(format!(
                    "The upload broke off after {received} of {length} bytes: {error}."
                )));
            }
            Ok(Some(Ok(chunk))) => chunk,
        };
        received += chunk.len() as u64;
        if received > length {
            break Err(Failure::new(
                ErrorCode::InvalidRequest,
                "The body is longer than its Content-Length.",
            ));
        }
        let wait = h.file_uploads.pace(h, rate, chunk.len() as u64);
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        if tx.send(ToDisk::Chunk(chunk)).await.is_err() {
            // The writer failed; its error is the answer.
            break Ok(());
        }
    };
    if read.is_ok() {
        // A writer gone already has its error.
        let _ = tx.send(ToDisk::Finish).await;
    }
    drop(tx);
    let written = writer
        .await
        .map_err(|error| Failure::internal("write the upload", error))?;
    let name = match (read, written) {
        (_, Err(error)) => return Err(failure("write the upload", error)),
        (Err(refusal), Ok(_)) => return Err(refusal),
        (Ok(()), Ok(Some(name))) => name,
        (Ok(()), Ok(None)) => {
            return Err(Failure::internal(
                "write the upload",
                "the writer stopped before the end",
            ));
        }
    };
    tracing::info!(
        "File transfer received {name} ({received} bytes) from {}",
        client.name
    );
    Ok(json(
        StatusCode::CREATED,
        &Uploaded {
            name,
            size: received,
        },
    ))
}
enum ToDisk {
    Chunk(Bytes),
    /// The whole body arrived: rename the `.part` file.
    Finish,
}
/// Creates the `.part` file, says whether that worked on `ready`, then writes
/// the queued chunks. The `.part` file lives and dies on this blocking
/// thread, as it is opened as the user: [`ToDisk::Finish`] renames it and
/// returns the name, while a queue closed early (a refused or cancelled
/// upload) or a failed write deletes it. The device's slot frees only then.
fn write_part(
    slot: Slot,
    dir: Option<PathBuf>,
    name: String,
    length: u64,
    ready: oneshot::Sender<Result<(), UserFolderError>>,
    mut rx: mpsc::Receiver<ToDisk>,
) -> Result<Option<String>, UserFolderError> {
    let _slot = slot;
    let created = open_folder(dir.as_deref()).and_then(|folder| {
        if length > folder.free_bytes()? {
            return Err(io::Error::from(io::ErrorKind::StorageFull).into());
        }
        folder.create_part(&name)
    });
    let mut part = match created {
        Ok(part) => part,
        Err(error) => {
            let _ = ready.send(Err(error));
            return Ok(None);
        }
    };
    if ready.send(Ok(())).is_err() {
        return discard(part);
    }
    loop {
        match rx.blocking_recv() {
            Some(ToDisk::Chunk(chunk)) => part.write_all(&chunk)?,
            Some(ToDisk::Finish) => return part.finish().map(Some),
            None => return discard(part),
        }
    }
}
fn discard(part: PartFile) -> Result<Option<String>, UserFolderError> {
    if let Err(error) = part.discard() {
        tracing::warn!("File transfer could not delete an incomplete upload: {error}");
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send(file: &[u8], length: u64) -> (io::Result<()>, Vec<u8>) {
        let mut sent = Vec::new();
        let result = read_exactly(&mut io::Cursor::new(file), length, |chunk| {
            sent.extend_from_slice(&chunk);
            true
        });
        (result, sent)
    }

    #[test]
    fn a_download_sends_exactly_its_length_or_fails_before_the_last_byte() {
        let file: Vec<u8> = (0..CHUNK * 2 + 7).map(|i| i as u8).collect();
        let (result, sent) = send(&file, file.len() as u64);
        result.unwrap();
        assert_eq!(sent, file);
        assert!(send(&[], 0).0.is_ok());
        // Grown since it was opened: the last chunk is held back.
        let (result, sent) = send(&file, file.len() as u64 - 1);
        assert!(result.unwrap_err().to_string().contains("grew"));
        assert!(sent.len() < file.len() - 1);
        assert!(send(&[1], 0).0.is_err());
        // Shrunk: everything there is goes, then the error ends the body.
        let (result, sent) = send(&file, file.len() as u64 + 1);
        assert!(result.unwrap_err().to_string().contains("shrank"));
        assert_eq!(sent, file);
    }

    #[test]
    fn a_client_gone_stops_the_download_quietly() {
        let file = vec![0; CHUNK * 3];
        let mut calls = 0;
        let result = read_exactly(&mut io::Cursor::new(&file), file.len() as u64, |_| {
            calls += 1;
            false
        });
        assert!(result.is_ok());
        assert_eq!(calls, 1);
    }

    #[test]
    fn uploads_share_one_pace_and_only_while_a_stream_runs() {
        let f = crate::state::test_support::Fixture::new();
        let h = &f.host;
        let rate = Some(125_000); // 1 Mbit/s
        let pace = |bytes| h.file_uploads.pace(h, rate, bytes);
        assert!(pace(PACE_BURST * 4).is_zero(), "no stream: unpaced");
        f.session(f.client(0), butterpollo_core::session::Role::Stream);
        // Look again at once, as the pacer would after PACE_CHECK.
        h.file_uploads.pacing.lock().unwrap().checked.take();
        // Two uploads, one budget: the second waits for what the first took.
        let first = pace(PACE_BURST);
        let second = pace(125_000);
        assert!(first < Duration::from_millis(100), "{first:?}");
        assert!(second >= Duration::from_millis(900), "{second:?}");
    }

    #[test]
    fn names_are_percent_decoded_and_must_be_utf8_and_single() {
        assert_eq!(name_of(Some("name=a%20b%2Bc.txt")).unwrap(), "a b+c.txt");
        assert_eq!(name_of(Some("x=1&name=%C3%A4.png")).unwrap(), "ä.png");
        assert_eq!(name_of(Some("name=a+b")).unwrap(), "a b");
        for query in [
            None,
            Some(""),
            Some("name=%FF"),
            Some("name=%2"),
            Some("name=a&name=b"),
        ] {
            let failure = name_of(query).unwrap_err();
            assert_eq!(failure.code, ErrorCode::InvalidName, "{query:?}");
        }
    }
}
