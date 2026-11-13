//! What every Nightjar app shows of file transfer, shared so that they all behave alike: the
//! transfer list (queued, running, done, failed, cancelled), the host's per-device limits that
//! decide which transfer starts next, what each refusal means for the user, the name an upload
//! is sent under, and a download that never leaves a short file behind.
//!
//! The host serves no ranges, so nothing resumes: retrying a failed or cancelled transfer sends
//! or fetches the whole file again. Progress comes from `upload_file` and `download_file`, which
//! move 64 KiB (upload) and 16 KiB (download) pieces and report at 0, at most every 256 KiB and
//! 100 ms, and at the end.

use super::files::{FileError, FileRefusal, TransferCancel, Uploaded};
use super::{Error, HostClient};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The host runs one upload per device at a time; another is `transfer_busy`.
pub const MAX_RUNNING_UPLOADS: usize = 1;
/// The host runs two downloads per device at a time; another is `transfer_busy`.
pub const MAX_RUNNING_DOWNLOADS: usize = 2;
/// The longest name the host accepts, in UTF-16 units.
pub const MAX_NAME_UNITS: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    /// This device to the PC.
    Upload,
    /// The PC to this device.
    Download,
}

/// Why a transfer failed, as the user needs to hear it. Each platform words these itself, by
/// [`Problem::id`]; the host's own message goes beside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Problem {
    /// 403 `forbidden`: the console has not given this device Upload files or Download files.
    /// Full control does not include them: they are granted per device.
    NoPermission,
    /// 403 `file_transfer_disabled`: the host's File transfer setting is off.
    TransferOff,
    /// 400 `invalid_name`.
    InvalidName,
    /// 400 `incomplete_upload`: the upload ended, stalled or was too slow before its end.
    Interrupted,
    /// 413 `too_large`: over the host's largest file.
    TooLarge,
    /// 507 `insufficient_storage`.
    NoSpace,
    /// 409 `no_console_user`: nobody is signed in to Windows on the PC.
    NoConsoleUser,
    /// 409 `transfer_busy`: the device or the PC runs as many transfers as the host allows, or
    /// another program on the PC holds the file.
    Busy,
    /// 404 `not_found`: the file is no longer in the PC's folder.
    NotFound,
    /// The network failed or timed out.
    Network,
    /// Reading or writing the file on this device failed.
    Local,
    /// Anything else the host answered or did: a status or body the API does not document, a
    /// download cut short, a certificate that is not the paired one.
    Host,
}

impl Problem {
    pub const ALL: [Problem; 12] = [
        Problem::NoPermission,
        Problem::TransferOff,
        Problem::InvalidName,
        Problem::Interrupted,
        Problem::TooLarge,
        Problem::NoSpace,
        Problem::NoConsoleUser,
        Problem::Busy,
        Problem::NotFound,
        Problem::Network,
        Problem::Local,
        Problem::Host,
    ];

    /// The problem behind a failed call; `None` for a cancelled one, which is no problem.
    pub fn of(error: &FileError) -> Option<Self> {
        Some(match error {
            FileError::Refused { refusal, .. } => match refusal {
                FileRefusal::Forbidden => Problem::NoPermission,
                FileRefusal::FileTransferDisabled => Problem::TransferOff,
                FileRefusal::InvalidName => Problem::InvalidName,
                FileRefusal::IncompleteUpload => Problem::Interrupted,
                // The client always sends a length: a host asking for one is the host's fault.
                FileRefusal::LengthRequired => Problem::Host,
                FileRefusal::TooLarge => Problem::TooLarge,
                FileRefusal::InsufficientStorage => Problem::NoSpace,
                FileRefusal::NoConsoleUser => Problem::NoConsoleUser,
                FileRefusal::TransferBusy => Problem::Busy,
                FileRefusal::NotFound => Problem::NotFound,
                // Only the shared folder calls answer this; on a file transfer call it is the host's fault.
                FileRefusal::FoldersDisabled => Problem::Host,
                // Only the shared folder calls answer this, which show it in their own words.
                FileRefusal::FolderUnreadable => Problem::Host,
            },
            FileError::Local(_) => Problem::Local,
            FileError::Host(Error::Cancelled) => return None,
            FileError::Host(Error::Io(_)) => Problem::Network,
            FileError::Host(_) => Problem::Host,
        })
    }

    /// A stable id for the platforms' strings: `no_permission`, `transfer_off`, ...
    pub fn id(self) -> &'static str {
        match self {
            Problem::NoPermission => "no_permission",
            Problem::TransferOff => "transfer_off",
            Problem::InvalidName => "invalid_name",
            Problem::Interrupted => "interrupted",
            Problem::TooLarge => "too_large",
            Problem::NoSpace => "no_space",
            Problem::NoConsoleUser => "no_console_user",
            Problem::Busy => "busy",
            Problem::NotFound => "not_found",
            Problem::Network => "network",
            Problem::Local => "local",
            Problem::Host => "host",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.id() == id)
    }

    /// Whether trying again can succeed: the same name, size or missing file fails the same way.
    pub fn retryable(self) -> bool {
        !matches!(
            self,
            Problem::InvalidName | Problem::TooLarge | Problem::NotFound
        )
    }

    /// Whether the fix is in the host's web console (Devices for the permission, Settings for
    /// File transfer), so the app points there.
    pub fn console_fix(self) -> bool {
        matches!(self, Problem::NoPermission | Problem::TransferOff)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Queued,
    Running {
        done: u64,
        total: u64,
    },
    /// `name` is where the file landed: the host's name for an upload (" (n)" when the name was
    /// taken), the local name for a download.
    Done {
        name: String,
        size: u64,
    },
    Failed {
        problem: Problem,
        message: String,
    },
    Cancelled,
}

impl State {
    pub fn is_finished(&self) -> bool {
        matches!(
            self,
            State::Done { .. } | State::Failed { .. } | State::Cancelled
        )
    }
}

/// One file to move.
pub struct Transfer {
    pub id: u64,
    pub direction: Direction,
    /// The name the host knows: what an upload is sent as ([`upload_name`]), what a download
    /// fetches.
    pub name: String,
    /// The name on this device: the file picked for an upload, the file saved for a download.
    pub local_name: String,
    pub size: u64,
    pub state: State,
    cancel: Arc<TransferCancel>,
    cancelling: bool,
}

impl std::fmt::Debug for Transfer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transfer")
            .field("id", &self.id)
            .field("direction", &self.direction)
            .field("name", &self.name)
            .field("local_name", &self.local_name)
            .field("size", &self.size)
            .field("state", &self.state)
            .field("cancelling", &self.cancelling)
            .finish()
    }
}

impl Transfer {
    /// How far it got, 0 to 1; `None` while queued or failed.
    pub fn fraction(&self) -> Option<f64> {
        match self.state {
            State::Running { done, total } if total > 0 => {
                Some((done as f64 / total as f64).min(1.0))
            }
            State::Running { .. } => Some(0.0),
            State::Done { .. } => Some(1.0),
            _ => None,
        }
    }

    /// Asked to stop, and not stopped yet.
    pub fn is_cancelling(&self) -> bool {
        self.cancelling && !self.state.is_finished()
    }

    /// Whether [`Transfers::retry`] would queue it again.
    pub fn can_retry(&self) -> bool {
        match &self.state {
            State::Failed { problem, .. } => problem.retryable(),
            State::Cancelled => true,
            _ => false,
        }
    }
}

/// A transfer to run now, on a worker: hand `cancel` to `upload_file` or `download_file` and
/// feed the answers back to [`Transfers::progress`] and [`Transfers::finish_upload`] or
/// [`Transfers::finish_download`].
#[derive(Clone)]
pub struct Start {
    pub id: u64,
    pub direction: Direction,
    pub name: String,
    pub local_name: String,
    pub size: u64,
    pub cancel: Arc<TransferCancel>,
}

impl std::fmt::Debug for Start {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Start")
            .field("id", &self.id)
            .field("direction", &self.direction)
            .field("name", &self.name)
            .field("local_name", &self.local_name)
            .field("size", &self.size)
            .finish()
    }
}

/// The transfers of one host, in the order they were asked for. Not thread-safe by itself: the
/// UI owns it, and workers report through the UI.
#[derive(Debug, Default)]
pub struct Transfers {
    items: Vec<Transfer>,
    next_id: u64,
}

impl Transfers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn items(&self) -> &[Transfer] {
        &self.items
    }

    pub fn get(&self, id: u64) -> Option<&Transfer> {
        self.items.iter().find(|t| t.id == id)
    }

    fn get_mut(&mut self, id: u64) -> Option<&mut Transfer> {
        self.items.iter_mut().find(|t| t.id == id)
    }

    /// Queues an upload of a local file named `local_name`, sent as [`upload_name`] of it.
    pub fn queue_upload(&mut self, local_name: &str, size: u64) -> u64 {
        self.queue(
            Direction::Upload,
            upload_name(local_name),
            local_name.into(),
            size,
        )
    }

    /// Queues a download of the host's `name`, saved as `local_name`.
    pub fn queue_download(&mut self, name: &str, local_name: &str, size: u64) -> u64 {
        self.queue(Direction::Download, name.into(), local_name.into(), size)
    }

    fn queue(&mut self, direction: Direction, name: String, local_name: String, size: u64) -> u64 {
        self.next_id += 1;
        self.items.push(Transfer {
            id: self.next_id,
            direction,
            name,
            local_name,
            size,
            state: State::Queued,
            cancel: Arc::new(TransferCancel::new()),
            cancelling: false,
        });
        self.next_id
    }

    fn running(&self, direction: Direction) -> usize {
        self.items
            .iter()
            .filter(|t| t.direction == direction && matches!(t.state, State::Running { .. }))
            .count()
    }

    /// Starts the queued transfers the host's limits leave room for, oldest first: one upload
    /// and two downloads at a time, so this device never causes its own `transfer_busy`.
    pub fn start_next(&mut self) -> Vec<Start> {
        let mut uploads = MAX_RUNNING_UPLOADS.saturating_sub(self.running(Direction::Upload));
        let mut downloads = MAX_RUNNING_DOWNLOADS.saturating_sub(self.running(Direction::Download));
        let mut started = Vec::new();
        for t in self.items.iter_mut().filter(|t| t.state == State::Queued) {
            let room = match t.direction {
                Direction::Upload => &mut uploads,
                Direction::Download => &mut downloads,
            };
            if *room == 0 {
                continue;
            }
            *room -= 1;
            t.state = State::Running {
                done: 0,
                total: t.size,
            };
            started.push(Start {
                id: t.id,
                direction: t.direction,
                name: t.name.clone(),
                local_name: t.local_name.clone(),
                size: t.size,
                cancel: t.cancel.clone(),
            });
        }
        started
    }

    /// A progress report from the worker. Returns whether the shown state changed; reports for a
    /// transfer that is not running, or that go backwards, change nothing.
    pub fn progress(&mut self, id: u64, done: u64, total: u64) -> bool {
        let Some(t) = self.get_mut(id) else {
            return false;
        };
        match &mut t.state {
            State::Running {
                done: shown,
                total: of,
            } if done >= *shown && (done, total) != (*shown, *of) => {
                *shown = done;
                *of = total;
                if t.direction == Direction::Download {
                    // A download learns its length from the host.
                    t.size = total;
                }
                true
            }
            _ => false,
        }
    }

    /// An upload's answer.
    pub fn finish_upload(&mut self, id: u64, result: Result<Uploaded, FileError>) {
        self.finish(id, result.map(|u| (u.name, u.size)));
    }

    /// A download's answer: the length saved as the transfer's `local_name`.
    pub fn finish_download(&mut self, id: u64, result: Result<u64, FileError>) {
        let local_name = self.get(id).map(|t| t.local_name.clone());
        self.finish(
            id,
            result.map(|size| (local_name.unwrap_or_default(), size)),
        );
    }

    fn finish(&mut self, id: u64, result: Result<(String, u64), FileError>) {
        let Some(t) = self.get_mut(id) else {
            return;
        };
        if !matches!(t.state, State::Running { .. }) {
            return;
        }
        t.state = match result {
            // A transfer that completed before the cancel took hold is done all the same.
            Ok((name, size)) => State::Done { name, size },
            Err(_) if t.cancelling => State::Cancelled,
            Err(error) => match Problem::of(&error) {
                None => State::Cancelled,
                Some(problem) => State::Failed {
                    problem,
                    message: error.to_string(),
                },
            },
        };
    }

    /// Cancels a transfer: a queued one at once, a running one as soon as its worker returns.
    /// Returns whether anything was cancelled.
    pub fn cancel(&mut self, id: u64) -> bool {
        let Some(t) = self.get_mut(id) else {
            return false;
        };
        match t.state {
            State::Queued => {
                t.state = State::Cancelled;
                true
            }
            State::Running { .. } if !t.cancelling => {
                t.cancelling = true;
                t.cancel.cancel();
                true
            }
            _ => false,
        }
    }

    /// Cancels every transfer that has not finished, as a screen that closes must.
    pub fn cancel_all(&mut self) {
        let ids: Vec<u64> = self
            .items
            .iter()
            .filter(|t| !t.state.is_finished())
            .map(|t| t.id)
            .collect();
        for id in ids {
            self.cancel(id);
        }
    }

    /// Queues a failed (retryable) or cancelled transfer again, from its first byte. Returns
    /// whether it did.
    pub fn retry(&mut self, id: u64) -> bool {
        let Some(t) = self.get_mut(id) else {
            return false;
        };
        if !t.can_retry() {
            return false;
        }
        t.state = State::Queued;
        t.cancel = Arc::new(TransferCancel::new());
        t.cancelling = false;
        true
    }

    /// Forgets the finished transfers.
    pub fn clear_finished(&mut self) {
        self.items.retain(|t| !t.state.is_finished());
    }

    /// Whether any transfer is queued or running.
    pub fn is_active(&self) -> bool {
        self.items.iter().any(|t| !t.state.is_finished())
    }

    /// Whether a finished upload landed, so the PC's list is worth reading again.
    pub fn uploaded_since(&self, ids: &[u64]) -> bool {
        self.items.iter().any(|t| {
            t.direction == Direction::Upload
                && ids.contains(&t.id)
                && matches!(t.state, State::Done { .. })
        })
    }
}

const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "COM0", "COM1", "COM2", "COM3", "COM4",
    "COM5", "COM6", "COM7", "COM8", "COM9", "COM¹", "COM²", "COM³", "LPT0", "LPT1", "LPT2", "LPT3",
    "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³",
];

/// The host's invisible formatting characters, which it refuses in a name (the zero-width joiner
/// and non-joiner stay).
fn format_character(c: char) -> bool {
    matches!(
        c,
        '\u{ad}'
            | '\u{61c}'
            | '\u{180e}'
            | '\u{200b}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
    )
}

/// The name a local file is sent as: the host refuses a name rather than rewriting it, so the
/// client makes it one the host takes and shows both when they differ. Characters Windows
/// refuses become `_`, invisible formatting characters go, a trailing dot or space goes, a
/// device name such as `CON` or a `.part` name gets a `_`, and a name over 200 UTF-16 units
/// keeps its extension and loses the end of its stem. A name with nothing left is `file`.
pub fn upload_name(local: &str) -> String {
    // A path from a picker that gives one: only its last part.
    let base = local.rsplit(['/', '\\']).next().unwrap_or(local);
    let mut name: String = base
        .chars()
        .filter(|c| !format_character(*c))
        .map(|c| {
            if c.is_control() || matches!(c, ':' | '<' | '>' | '"' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    name.truncate(name.trim_end_matches(['.', ' ']).len());
    if name.is_empty() || name == ".." {
        name = "file".into();
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or(&name)
        .trim_end_matches(' ');
    if RESERVED
        .iter()
        .any(|device| stem.eq_ignore_ascii_case(device))
    {
        name.insert(0, '_');
    }
    if name.encode_utf16().count() > MAX_NAME_UNITS {
        name = shorten(&name);
    }
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".part") {
        name.push('_');
    }
    name
}

/// `name` cut to [`MAX_NAME_UNITS`], keeping a short extension.
fn shorten(name: &str) -> String {
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 && name[dot..].encode_utf16().count() <= 16 => name.split_at(dot),
        _ => (name, ""),
    };
    let room = MAX_NAME_UNITS - extension.encode_utf16().count();
    let mut kept = String::new();
    let mut units = 0;
    for c in stem.chars() {
        units += c.len_utf16();
        if units > room {
            break;
        }
        kept.push(c);
    }
    let kept = kept.trim_end_matches(['.', ' ']);
    format!("{kept}{extension}")
}

/// Where a download is written until all of it arrived: beside `dest`, so the rename stays on
/// one volume.
pub fn partial_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

/// Runs `fetch` into a new file beside `dest` and moves it to `dest` only when `fetch` succeeded
/// and the file is on disk. A `<dest>.part` that is already there is not touched, and a file
/// that took `dest` during the download is not replaced: both fail the download. On any other
/// failure, a cancel included, the partial file is deleted (one that can't be is named in the
/// error) and `dest` is left as it was: a short file never carries the name.
pub fn download_into(
    dest: &Path,
    fetch: impl FnOnce(&mut dyn Write) -> Result<u64, FileError>,
) -> Result<u64, FileError> {
    let part = partial_path(dest);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&part)
        .map_err(|e| {
            FileError::Local(std::io::Error::new(
                e.kind(),
                format!("{}: {e}", part.display()),
            ))
        })?;
    let result = fetch(&mut file).and_then(|size| {
        file.sync_all().map_err(FileError::Local)?;
        Ok(size)
    });
    drop(file);
    result
        .and_then(|size| {
            move_partial(&part, dest).map_err(FileError::Local)?;
            Ok(size)
        })
        .map_err(|error| discard_partial(&part, error))
}

/// Moves the finished download `part` to `dest`, which must still be free: a file that took
/// the name during the download is never replaced, and the move fails naming it.
pub(super) fn move_partial(part: &Path, dest: &Path) -> std::io::Result<()> {
    rename_new(part, dest).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => std::io::Error::new(
            e.kind(),
            format!(
                "{} appeared during the download and was not replaced",
                dest.display()
            ),
        ),
        _ => e,
    })
}

/// Deletes the partial file of a failed download and returns `error`, or, when the file stays,
/// an error that names it beside `error`.
pub(super) fn discard_partial(part: &Path, error: FileError) -> FileError {
    match std::fs::remove_file(part) {
        Ok(()) => error,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => error,
        Err(e) => FileError::Local(std::io::Error::new(
            e.kind(),
            format!(
                "{error}; the partial file {} is left, it could not be deleted: {e}",
                part.display()
            ),
        )),
    }
}

/// `std::fs::rename`, except that an existing `to` fails it with `AlreadyExists` instead of
/// being replaced.
#[cfg(windows)]
fn rename_new(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    let wide = |path: &Path| -> Vec<u16> { path.as_os_str().encode_wide().chain([0]).collect() };
    let (from, to) = (wide(from), wide(to));
    // SAFETY: both are NUL-terminated and outlive the call. Without MOVEFILE_REPLACE_EXISTING
    // an existing target fails the move (ERROR_ALREADY_EXISTS).
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// `std::fs::rename`, except that an existing `to` fails it with `AlreadyExists` instead of
/// being replaced: `renameat2(RENAME_NOREPLACE)`, or a hard link and unlink where the file
/// system does not have it.
#[cfg(target_os = "linux")]
fn rename_new(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c = |path: &Path| {
        std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
    };
    let (from_c, to_c) = (c(from)?, c(to)?);
    // SAFETY: both are NUL-terminated and outlive the call.
    let moved = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from_c.as_ptr(),
            libc::AT_FDCWD,
            to_c.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if moved == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::EINVAL | libc::ENOSYS) => link_and_unlink(from, to),
        _ => Err(error),
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn rename_new(from: &Path, to: &Path) -> std::io::Result<()> {
    link_and_unlink(from, to)
}

/// A link fails on an existing target, so nothing is replaced.
#[cfg(unix)]
fn link_and_unlink(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::hard_link(from, to)?;
    std::fs::remove_file(from)
}

impl HostClient {
    /// Downloads the host's `name` to the file `dest` through [`download_into`], with
    /// `download_file`'s size check, progress and cancel.
    pub fn download_to_path(
        &self,
        name: &str,
        listed: u64,
        dest: &Path,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<u64, FileError> {
        download_into(dest, |sink| {
            self.download_file(name, listed, sink, progress, stop)
        })
    }
}

#[cfg(test)]
mod tests;
