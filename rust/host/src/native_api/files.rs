//! File transfer (`docs/native-api.md`, "Files"): a paired device sends
//! files to the PC's transfer folder and fetches them back.
//!
//! Every route needs the host's File transfer setting and the device's
//! Upload files or Download files bit, read on each request and again every
//! [`PACE_CHECK`] while a transfer runs. Files are opened as the signed-in
//! Windows user on a blocking thread (`butterpollo_windows::user_folder`).
//! While a stream runs, uploads and downloads together move at
//! `file_transfer_stream_mbps`. An upload streams to its `.part` file and is
//! renamed only once all of its `Content-Length` arrived.
use super::{Failure, Reply, caller, json};
use crate::{state::Shared, tls::Connection};
use axum::{
    Extension,
    body::{Body, Bytes, HttpBody},
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
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
    collections::HashMap,
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

/// Bytes read from disk per download chunk.
const CHUNK: usize = 64 * 1024;
/// Chunks queued between the socket and the disk.
const QUEUE: usize = 8;
/// What a paced transfer may move at once after a pause.
const PACE_BURST: u64 = 256 * 1024;
/// How often a running transfer looks whether a stream started or ended,
/// whether the pace changed and whether the device may still transfer.
const PACE_CHECK: Duration = Duration::from_millis(250);
/// Downloads one device may run at once.
const DEVICE_DOWNLOADS: usize = 2;
/// Downloads all devices together may run at once.
const ALL_DOWNLOADS: usize = 8;
/// Free space an upload leaves on its volume: 1/50 (2 %) of the volume, at
/// least 1 GiB and at most 4 GiB.
const MIN_RESERVE: u64 = 1 << 30;
const MAX_RESERVE: u64 = 4 << 30;
const RESERVE_SHARE: u64 = 50;
/// `ERROR_SHARING_VIOLATION`: another program holds the file open without
/// letting others read it.
const SHARING_VIOLATION: i32 = 32;

/// How long a transfer may stall, and how much of a refused upload is read.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    /// A transfer that moves nothing for this long ends.
    pub(crate) idle: Duration,
    /// After `grace`, an upload must have arrived at `min_rate` bytes a
    /// second on average (not 0). Time it waited for the pacer or the disk
    /// doesn't count.
    pub(crate) min_rate: u64,
    pub(crate) grace: Duration,
    /// The rest of a refused upload is read and dropped for at most this
    /// many bytes or this long, then the connection closes.
    pub(crate) drain_bytes: u64,
    pub(crate) drain_time: Duration,
}
const LIMITS: Limits = Limits {
    idle: Duration::from_secs(30),
    min_rate: 64 * 1024,
    grace: Duration::from_secs(30),
    drain_bytes: 8 << 20,
    drain_time: Duration::from_secs(5),
};

/// The transfers running, all paced together.
#[derive(Debug)]
pub struct Transfers {
    /// Uploads by device, one each, with the bytes each still has to write.
    uploads: Mutex<HashMap<String, u64>>,
    /// Downloads by device.
    downloads: Mutex<HashMap<String, usize>>,
    pacing: Mutex<Pacing>,
    /// Shorter limits for a test.
    #[cfg(test)]
    limits: Mutex<Limits>,
    /// Free and total bytes of the volume, in place of the disk's.
    #[cfg(test)]
    space: Mutex<Option<(u64, u64)>>,
    /// How long a refused upload's writer lingers after its answer.
    #[cfg(test)]
    refusal_hold: Mutex<Duration>,
}
impl Default for Transfers {
    fn default() -> Self {
        Self {
            uploads: Mutex::default(),
            downloads: Mutex::default(),
            pacing: Mutex::new(Pacing {
                pacer: Pacer::new(None, PACE_BURST, Instant::now()),
                checked: None,
            }),
            #[cfg(test)]
            limits: Mutex::new(LIMITS),
            #[cfg(test)]
            space: Mutex::default(),
            #[cfg(test)]
            refusal_hold: Mutex::default(),
        }
    }
}
/// One pacer for every transfer, so together they stay within the stream
/// rate.
#[derive(Debug)]
struct Pacing {
    pacer: Pacer,
    /// When the pacer last looked whether a stream runs.
    checked: Option<Instant>,
}
impl Transfers {
    /// How long to wait before moving `bytes`: unpaced unless a stream runs.
    /// The rate is read from the settings every [`PACE_CHECK`], so a change
    /// applies to the transfers already running.
    fn pace(&self, h: &Shared, bytes: u64) -> Duration {
        let now = Instant::now();
        let mut pacing = self.pacing.lock().unwrap_or_else(PoisonError::into_inner);
        if pacing
            .checked
            .is_none_or(|at| now.duration_since(at) >= PACE_CHECK)
        {
            pacing.checked = Some(now);
            let want = if streaming(h) {
                h.config
                    .read()
                    .unwrap_or_else(PoisonError::into_inner)
                    .file_transfer_stream_rate()
            } else {
                None
            };
            if pacing.pacer.rate() != want {
                pacing.pacer.set_rate(want, now);
            }
        }
        pacing.pacer.take(bytes, now)
    }

    #[cfg(not(test))]
    fn limits(&self) -> Limits {
        LIMITS
    }
    #[cfg(test)]
    fn limits(&self) -> Limits {
        *self.limits.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The free and total bytes of the folder's volume.
    #[cfg(not(test))]
    fn space(&self, folder: &UserFolder) -> Result<(u64, u64), UserFolderError> {
        folder.volume_bytes()
    }
    #[cfg(test)]
    fn space(&self, folder: &UserFolder) -> Result<(u64, u64), UserFolderError> {
        match *self.space.lock().unwrap_or_else(PoisonError::into_inner) {
            Some(space) => Ok(space),
            None => folder.volume_bytes(),
        }
    }

    /// What an upload may still use of the folder's volume: its free bytes
    /// less the reserve and what the uploads running still write.
    fn room(&self, folder: &UserFolder) -> Result<u64, UserFolderError> {
        let uploads = self.uploads.lock().unwrap_or_else(PoisonError::into_inner);
        let (free, total) = self.space(folder)?;
        Ok(room(&uploads, free, total))
    }
}
#[cfg(test)]
impl Transfers {
    /// Shortens the limits for a test.
    pub(crate) fn limit(&self, change: impl FnOnce(&mut Limits)) {
        change(&mut self.limits.lock().unwrap());
    }
    /// Reports `free` of `total` bytes on the transfer folder's volume.
    pub(crate) fn fake_space(&self, free: u64, total: u64) {
        *self.space.lock().unwrap() = Some((free, total));
    }
    /// Keeps a refused upload's writer running `hold` after its answer.
    pub(crate) fn hold_refusals(&self, hold: Duration) {
        *self.refusal_hold.lock().unwrap() = hold;
    }
    /// The downloads running.
    pub(crate) fn downloads(&self) -> usize {
        self.downloads.lock().unwrap().values().sum()
    }
}
/// What is left of `free` bytes on a volume of `total` for another upload,
/// beside the reserve and the bytes `uploads` still write.
fn room(uploads: &HashMap<String, u64>, free: u64, total: u64) -> u64 {
    let reserve = (total / RESERVE_SHARE).clamp(MIN_RESERVE, MAX_RESERVE);
    let due = uploads
        .values()
        .fold(0u64, |due, left| due.saturating_add(*left));
    free.saturating_sub(reserve).saturating_sub(due)
}

/// A device's upload slot, given back when the upload ends however it ends.
/// The upload's writer holds it, so the slot frees only once the `.part`
/// file is renamed or gone.
struct UploadSlot {
    host: Shared,
    device: String,
}
impl UploadSlot {
    /// Holds room for `length` bytes on the folder's volume, if it has that
    /// much beside the reserve and the bytes other uploads still write.
    fn hold_room(&self, folder: &UserFolder, length: u64) -> Result<(), UserFolderError> {
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
struct DownloadSlot {
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
fn begin_download(h: &Shared, device: &str) -> Result<DownloadSlot, Failure> {
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
/// [`allowed`] again every [`PACE_CHECK`] while a transfer runs, so turning
/// File transfer off, revoking the bit or unpairing the device ends it.
struct Recheck {
    host: Shared,
    connection: Connection,
    bit: u32,
    name: &'static str,
    at: Instant,
}
impl Recheck {
    fn new(h: &Shared, c: &Connection, bit: u32, name: &'static str) -> Self {
        Self {
            host: h.clone(),
            connection: c.clone(),
            bit,
            name,
            at: Instant::now(),
        }
    }
    fn check(&mut self) -> Result<(), Failure> {
        let now = Instant::now();
        if now.duration_since(self.at) < PACE_CHECK {
            return Ok(());
        }
        self.at = now;
        allowed(&self.host, &self.connection, self.bit, self.name).map(drop)
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
#[cfg(not(test))]
fn open_folder(dir: Option<&Path>) -> Result<UserFolder, UserFolderError> {
    UserFolder::open(dir)
}
/// Tests open their folder as the developer: an elevated test runner
/// without a desktop shell has no user to act as.
#[cfg(test)]
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
async fn blocking<T: Send + 'static>(
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
        Err(refusal) => return Err(refusal),
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
    let slot = begin_download(&h, &client.uuid)?;
    let dir = transfer_dir(&h);
    let opened = tokio::task::spawn_blocking(move || open_folder(dir.as_deref())?.open_file(&name))
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
    let (fail, failed) = oneshot::channel();
    let runtime = tokio::runtime::Handle::current();
    let idle = h.file_transfers.limits().idle;
    let mut recheck = Recheck::new(&h, &c, FILE_DOWNLOAD, "Download files");
    tokio::task::spawn_blocking(move || {
        // Dropped after the file, so the slot frees once it is closed.
        let _slot = slot;
        let mut file = file;
        let refused =
            |refusal: Failure| io::Error::new(io::ErrorKind::PermissionDenied, refusal.message);
        let sent = read_exactly(&mut file, length, |chunk| {
            let wait = h.file_transfers.pace(&h, chunk.len() as u64);
            paced_blocking(wait, &mut recheck).map_err(refused)?;
            let stalled = Instant::now() + idle;
            let permit = loop {
                match runtime.block_on(tokio::time::timeout(PACE_CHECK, tx.reserve())) {
                    Ok(Ok(permit)) => break permit,
                    // The client went away.
                    Ok(Err(_)) => return Ok(false),
                    Err(_) if Instant::now() >= stalled => {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!("the device took nothing for {} s", idle.as_secs()),
                        ));
                    }
                    Err(_) => recheck.check().map_err(refused)?,
                }
            };
            permit.send(chunk);
            Ok(true)
        });
        if let Err(error) = sent {
            tracing::warn!("File transfer stopped a download: {error}");
            // Sent before the queue closes, so the body ends with it after
            // the chunks queued.
            let _ = fail.send(error);
        }
    });
    let body = Body::from_stream(tokio_util::io::ReaderStream::with_capacity(
        Chunks {
            rx,
            failed,
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
/// Stops quietly when `send` reports that the client went away (`false`),
/// and with `send`'s error.
fn read_exactly(
    file: &mut impl Read,
    length: u64,
    mut send: impl FnMut(Bytes) -> io::Result<bool>,
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
        if !send(Bytes::copy_from_slice(&buffer[..read]))? || left == 0 {
            return Ok(());
        }
    }
}
/// The download's chunks as a reader. A download that failed ends the body
/// with its error once the chunks queued are read.
struct Chunks {
    rx: mpsc::Receiver<Bytes>,
    failed: oneshot::Receiver<io::Error>,
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
                Some(chunk) => this.current = chunk,
                None => {
                    return Poll::Ready(match this.failed.try_recv() {
                        Ok(error) => Err(error),
                        Err(_) => Ok(()),
                    });
                }
            }
        }
        let n = buf.remaining().min(this.current.len());
        buf.put_slice(&this.current.split_to(n));
        Poll::Ready(Ok(()))
    }
}

/// `POST /files?name=`. A caller that isn't a paired device is answered and
/// the connection closed at once. Any other refusal answers at once too;
/// the rest of the body is then read and dropped in the background for a
/// while ([`drain`]), so the client, which sends all of it before it reads
/// the answer, gets the answer rather than a reset.
pub(super) async fn upload(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Response {
    let (parts, mut body) = request.into_parts();
    if let Err(refusal) = caller(&h, &c) {
        // Reading on would let anyone send the host data.
        refused_upload(&c, &refusal);
        return closing(refusal.into_response());
    }
    let mut response = match receive(&h, &c, parts.uri.query(), &parts.headers, &mut body).await {
        Ok(response) => response,
        Err(failure) => {
            refused_upload(&c, &failure);
            failure.into_response()
        }
    };
    if !body.is_end_stream() {
        tokio::spawn(drain(h, body));
        response = closing(response);
    }
    response
}
fn refused_upload(c: &Connection, failure: &Failure) {
    tracing::warn!(
        "File transfer refused an upload from {}: {}",
        c.peer.ip(),
        failure.message
    );
}
/// The connection closes after this answer.
fn closing(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CONNECTION, HeaderValue::from_static("close"));
    response
}
/// Reads and drops the rest of a refused upload, through the pacer, for at
/// most `drain_bytes` or `drain_time`. Dropping the body then closes the
/// connection, as its answer said.
async fn drain(h: Shared, mut body: Body) {
    let limits = h.file_transfers.limits();
    let until = tokio::time::Instant::now() + limits.drain_time;
    let mut drained = 0u64;
    while drained < limits.drain_bytes {
        let Ok(Some(Ok(chunk))) = tokio::time::timeout_at(until, next_chunk(&mut body)).await
        else {
            break;
        };
        drained += chunk.len() as u64;
        let wait = h.file_transfers.pace(&h, chunk.len() as u64);
        if tokio::time::Instant::now() + wait >= until {
            break;
        }
        tokio::time::sleep(wait).await;
    }
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
/// Whether a stream runs or starts, so transfers are paced.
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
    let (max, dir) = {
        let config = h.config.read().unwrap_or_else(PoisonError::into_inner);
        (config.file_transfer_max_bytes(), config.file_transfer_dir())
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

    let limits = h.file_transfers.limits();
    let mut recheck = Recheck::new(h, c, FILE_UPLOAD, "Upload files");
    let started = Instant::now();
    // Time spent on the pacer and the disk, which the device isn't slow for.
    let mut held = Duration::ZERO;
    let mut last = started;
    let mut received = 0u64;
    let read: Result<(), Failure> = loop {
        if let Err(refusal) = recheck.check() {
            break Err(refusal);
        }
        let stalled = last + limits.idle;
        // When the average falls below `min_rate`, once `grace` is over.
        let slow = started
            + held
            + limits.grace.max(Duration::from_millis(
                received.saturating_mul(1000) / limits.min_rate,
            ));
        let until = stalled.min(slow).min(Instant::now() + PACE_CHECK);
        let chunk = match tokio::time::timeout_at(until.into(), next_chunk(body)).await {
            Err(_) => {
                let now = Instant::now();
                if now >= stalled {
                    break Err(incomplete(format!(
                        "The upload stalled after {received} of {length} bytes."
                    )));
                }
                if now >= slow {
                    break Err(incomplete(format!(
                        "The upload arrived slower than {} KiB/s; {received} of {length} bytes came.",
                        limits.min_rate / 1024
                    )));
                }
                continue;
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
        let holding = Instant::now();
        let wait = h.file_transfers.pace(h, chunk.len() as u64);
        if let Err(refusal) = paced(wait, &mut recheck).await {
            break Err(refusal);
        }
        if tx.send(ToDisk::Chunk(chunk)).await.is_err() {
            // The writer failed; its error is the answer.
            break Ok(());
        }
        last = Instant::now();
        held += last - holding;
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
/// Holds room for the upload and creates its `.part` file, says whether
/// that worked on `ready`, then writes the queued chunks. The `.part` file
/// lives and dies on this blocking thread, as it is opened as the user:
/// [`ToDisk::Finish`] renames it and returns the name, while a queue closed
/// early (a refused or cancelled upload) or a failed write deletes it. The
/// device's slot and the room it held free only then.
fn write_part(
    slot: UploadSlot,
    dir: Option<PathBuf>,
    name: String,
    length: u64,
    ready: oneshot::Sender<Result<(), UserFolderError>>,
    mut rx: mpsc::Receiver<ToDisk>,
) -> Result<Option<String>, UserFolderError> {
    let created = open_folder(dir.as_deref()).and_then(|folder| {
        slot.hold_room(&folder, length)?;
        folder.create_part(&name)
    });
    let mut part = match created {
        Ok(part) => part,
        Err(error) => {
            #[cfg(test)]
            let hold = *slot
                .host
                .file_transfers
                .refusal_hold
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            // Free the device's slot first: once the refusal is answered,
            // its next upload must not find the slot still taken.
            drop(slot);
            let _ = ready.send(Err(error));
            #[cfg(test)]
            std::thread::sleep(hold);
            return Ok(None);
        }
    };
    if ready.send(Ok(())).is_err() {
        return discard(part);
    }
    loop {
        match rx.blocking_recv() {
            Some(ToDisk::Chunk(chunk)) => {
                part.write_all(&chunk)?;
                slot.wrote(chunk.len() as u64);
            }
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
            Ok(true)
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
    fn a_client_gone_stops_the_download_quietly_and_a_refusal_with_its_error() {
        let file = vec![0; CHUNK * 3];
        let mut calls = 0;
        let result = read_exactly(&mut io::Cursor::new(&file), file.len() as u64, |_| {
            calls += 1;
            Ok(false)
        });
        assert!(result.is_ok());
        assert_eq!(calls, 1);
        let result = read_exactly(&mut io::Cursor::new(&file), file.len() as u64, |_| {
            Err(io::Error::from(io::ErrorKind::TimedOut))
        });
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
    }

    fn set(h: &Shared, key: &str, value: &str) {
        h.config
            .write()
            .unwrap()
            .values
            .insert(key.into(), value.into());
    }
    /// Makes the pacer look at the stream and the settings again at once,
    /// as it would after [`PACE_CHECK`].
    fn look_again(h: &Shared) {
        h.file_transfers.pacing.lock().unwrap().checked.take();
    }

    #[test]
    fn transfers_share_one_pace_and_only_while_a_stream_runs() {
        let f = crate::state::test_support::Fixture::new();
        let h = &f.host;
        set(h, "file_transfer_stream_mbps", "1");
        let pace = |bytes| h.file_transfers.pace(h, bytes);
        assert!(pace(PACE_BURST * 4).is_zero(), "no stream: unpaced");
        f.session(f.client(0), butterpollo_core::session::Role::Stream);
        look_again(h);
        // Two transfers, one budget: the second waits for what the first took.
        let first = pace(PACE_BURST);
        let second = pace(125_000);
        assert!(first < Duration::from_millis(100), "{first:?}");
        assert!(second >= Duration::from_millis(900), "{second:?}");
    }

    #[test]
    fn a_new_stream_rate_applies_to_the_transfers_running() {
        let f = crate::state::test_support::Fixture::new();
        let h = &f.host;
        f.session(f.client(0), butterpollo_core::session::Role::Stream);
        let rate = || h.file_transfers.pacing.lock().unwrap().pacer.rate();
        h.file_transfers.pace(h, 1);
        assert_eq!(rate(), Some(50 * 125_000), "the default 50 Mbit/s");
        set(h, "file_transfer_stream_mbps", "8");
        // Not before the next look, which comes every PACE_CHECK.
        h.file_transfers.pace(h, 1);
        assert_eq!(rate(), Some(50 * 125_000));
        look_again(h);
        h.file_transfers.pace(h, 1);
        assert_eq!(rate(), Some(1_000_000));
        set(h, "file_transfer_stream_mbps", "0");
        look_again(h);
        assert!(h.file_transfers.pace(h, 100 << 20).is_zero());
        assert_eq!(rate(), None);
    }

    #[test]
    fn an_upload_leaves_the_reserve_and_the_bytes_running_uploads_still_write() {
        const GIB: u64 = 1 << 30;
        let none = HashMap::new();
        // 1 GiB on a small volume, 2 % on a mid-sized one, 4 GiB on a big one.
        assert_eq!(room(&none, 10 * GIB, 20 * GIB), 9 * GIB);
        assert_eq!(room(&none, 30 * GIB, 100 * GIB), 28 * GIB);
        assert_eq!(room(&none, 30 * GIB, 1000 * GIB), 26 * GIB);
        // A 930 GB drive with 15 GB free, where 2 % would leave nothing.
        assert_eq!(
            room(&none, 15_000_000_000, 930_000_000_000),
            15_000_000_000 - 4 * GIB
        );
        assert_eq!(room(&none, 3 * GIB, 4000 * GIB), 0);
        assert_eq!(room(&none, GIB / 2, 20 * GIB), 0);
        let running = HashMap::from([("a".to_owned(), 3 * GIB), ("b".to_owned(), GIB)]);
        assert_eq!(room(&running, 10 * GIB, 20 * GIB), 5 * GIB);
        assert_eq!(room(&running, 4 * GIB, 20 * GIB), 0);
        let huge = HashMap::from([("a".to_owned(), u64::MAX), ("b".to_owned(), u64::MAX)]);
        assert_eq!(room(&huge, u64::MAX, u64::MAX), 0);
    }

    #[test]
    fn names_are_percent_decoded_and_must_be_utf8_and_single() {
        assert_eq!(name_of(Some("name=a%20b%2Bc.txt")).unwrap(), "a b+c.txt");
        assert_eq!(name_of(Some("x=1&name=%C3%A4.png")).unwrap(), "ä.png");
        // RFC 3986: a plus is a plus, not a space.
        assert_eq!(name_of(Some("name=a+b")).unwrap(), "a+b");
        assert_eq!(name_of(Some("name=C%2B%2B+x.txt")).unwrap(), "C+++x.txt");
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
