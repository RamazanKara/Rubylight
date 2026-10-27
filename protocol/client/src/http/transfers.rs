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
/// and the file is on disk, replacing what `dest` held. On any failure, a cancel included, the
/// partial file is deleted and `dest` is left as it was: a short file never carries the name.
pub fn download_into(
    dest: &Path,
    fetch: impl FnOnce(&mut dyn Write) -> Result<u64, FileError>,
) -> Result<u64, FileError> {
    let part = partial_path(dest);
    let mut file = std::fs::File::create(&part).map_err(FileError::Local)?;
    let result = fetch(&mut file).and_then(|size| {
        file.sync_all().map_err(FileError::Local)?;
        Ok(size)
    });
    drop(file);
    let result = result.and_then(|size| {
        std::fs::rename(&part, dest).map_err(FileError::Local)?;
        Ok(size)
    });
    if result.is_err() {
        // Deleting what is left is best effort; the transfer's own error is the one to show.
        let _ = std::fs::remove_file(&part);
    }
    result
}

impl HostClient {
    /// Downloads the host's `name` to the file `dest` through [`download_into`], with
    /// `download_file`'s progress and cancel.
    pub fn download_to_path(
        &self,
        name: &str,
        dest: &Path,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<u64, FileError> {
        download_into(dest, |sink| self.download_file(name, sink, progress, stop))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(refusal: FileRefusal) -> FileError {
        FileError::Refused {
            refusal,
            status: 403,
            message: format!("Host says {}", refusal.code()),
        }
    }

    #[test]
    fn every_refusal_has_its_problem() {
        let table = [
            (FileRefusal::Forbidden, Problem::NoPermission),
            (FileRefusal::FileTransferDisabled, Problem::TransferOff),
            (FileRefusal::InvalidName, Problem::InvalidName),
            (FileRefusal::IncompleteUpload, Problem::Interrupted),
            (FileRefusal::LengthRequired, Problem::Host),
            (FileRefusal::TooLarge, Problem::TooLarge),
            (FileRefusal::InsufficientStorage, Problem::NoSpace),
            (FileRefusal::NoConsoleUser, Problem::NoConsoleUser),
            (FileRefusal::TransferBusy, Problem::Busy),
            (FileRefusal::NotFound, Problem::NotFound),
        ];
        for (refusal, problem) in table {
            assert_eq!(Problem::of(&refused(refusal)), Some(problem), "{refusal:?}");
        }
        let io = || std::io::Error::other("reset");
        assert_eq!(Problem::of(&FileError::Local(io())), Some(Problem::Local));
        assert_eq!(
            Problem::of(&FileError::Host(Error::Io(io()))),
            Some(Problem::Network)
        );
        assert_eq!(Problem::of(&FileError::Host(Error::Cancelled)), None);
        for other in [
            Error::CertificateMismatch,
            Error::Malformed("the download ended after 5 of 9 bytes".into()),
            Error::Status {
                code: 500,
                message: "x".into(),
            },
            Error::NotFound("/files".into()),
        ] {
            assert_eq!(Problem::of(&FileError::Host(other)), Some(Problem::Host));
        }
    }

    #[test]
    fn problem_ids_are_stable_and_unique() {
        let ids: Vec<&str> = Problem::ALL.iter().map(|p| p.id()).collect();
        assert_eq!(
            ids,
            [
                "no_permission",
                "transfer_off",
                "invalid_name",
                "interrupted",
                "too_large",
                "no_space",
                "no_console_user",
                "busy",
                "not_found",
                "network",
                "local",
                "host"
            ]
        );
        for problem in Problem::ALL {
            assert_eq!(Problem::from_id(problem.id()), Some(problem));
        }
        assert_eq!(Problem::from_id("forbidden"), None);
        // Only the permission and the setting are fixed in the console.
        let console: Vec<Problem> = Problem::ALL
            .into_iter()
            .filter(|p| p.console_fix())
            .collect();
        assert_eq!(console, [Problem::NoPermission, Problem::TransferOff]);
        let final_: Vec<Problem> = Problem::ALL
            .into_iter()
            .filter(|p| !p.retryable())
            .collect();
        assert_eq!(
            final_,
            [Problem::InvalidName, Problem::TooLarge, Problem::NotFound]
        );
    }

    #[test]
    fn the_host_limits_decide_what_starts() {
        let mut list = Transfers::new();
        let up1 = list.queue_upload("a.txt", 10);
        let up2 = list.queue_upload("b.txt", 10);
        let down1 = list.queue_download("c.txt", "c.txt", 10);
        let down2 = list.queue_download("d.txt", "d.txt", 10);
        let down3 = list.queue_download("e.txt", "e.txt", 10);
        let started: Vec<u64> = list.start_next().iter().map(|s| s.id).collect();
        assert_eq!(started, [up1, down1, down2]);
        // Nothing more until one finishes.
        assert!(list.start_next().is_empty());
        list.finish_download(down1, Ok(10));
        assert_eq!(
            list.start_next().iter().map(|s| s.id).collect::<Vec<_>>(),
            [down3]
        );
        list.finish_upload(
            up1,
            Ok(Uploaded {
                name: "a (1).txt".into(),
                size: 10,
            }),
        );
        let next = list.start_next();
        assert_eq!(
            (next.len(), next[0].id, next[0].name.as_str()),
            (1, up2, "b.txt")
        );
        assert_eq!(
            list.get(up1).unwrap().state,
            State::Done {
                name: "a (1).txt".into(),
                size: 10
            }
        );
        assert!(list.uploaded_since(&[up1]));
        assert!(!list.uploaded_since(&[up2, down1]));
    }

    #[test]
    fn progress_only_moves_forward_while_running() {
        let mut list = Transfers::new();
        let id = list.queue_download("big.bin", "big.bin", 0);
        assert!(!list.progress(id, 5, 100), "queued");
        list.start_next();
        assert_eq!(list.get(id).unwrap().fraction(), Some(0.0));
        assert!(list.progress(id, 0, 100));
        assert_eq!(
            list.get(id).unwrap().size,
            100,
            "a download learns its size"
        );
        assert!(list.progress(id, 64 * 1024, 100 * 1024));
        assert!(!list.progress(id, 64 * 1024, 100 * 1024), "the same report");
        assert!(!list.progress(id, 16 * 1024, 100 * 1024), "backwards");
        assert_eq!(list.get(id).unwrap().fraction(), Some(0.64));
        list.finish_download(id, Ok(100 * 1024));
        assert!(!list.progress(id, 100 * 1024, 100 * 1024), "finished");
        assert_eq!(list.get(id).unwrap().fraction(), Some(1.0));
        assert!(!list.progress(99, 1, 1), "unknown");
    }

    #[test]
    fn failures_keep_their_problem_and_message() {
        let mut list = Transfers::new();
        let id = list.queue_upload("a.txt", 10);
        list.start_next();
        list.finish_upload(id, Err(refused(FileRefusal::Forbidden)));
        let t = list.get(id).unwrap();
        assert_eq!(
            t.state,
            State::Failed {
                problem: Problem::NoPermission,
                message: "host error 403: Host says forbidden".into()
            }
        );
        assert!(t.can_retry());
        assert!(list.retry(id));
        assert_eq!(list.get(id).unwrap().state, State::Queued);
        list.start_next();
        list.finish_upload(id, Err(refused(FileRefusal::TooLarge)));
        assert!(!list.get(id).unwrap().can_retry());
        assert!(!list.retry(id), "too large stays too large");
        // An answer for a transfer that is not running changes nothing.
        list.finish_upload(
            id,
            Ok(Uploaded {
                name: "a.txt".into(),
                size: 10,
            }),
        );
        assert!(matches!(
            list.get(id).unwrap().state,
            State::Failed {
                problem: Problem::TooLarge,
                ..
            }
        ));
    }

    #[test]
    fn a_cancel_stops_queued_at_once_and_running_through_the_worker() {
        let mut list = Transfers::new();
        let running = list.queue_upload("a.txt", 10);
        let queued = list.queue_upload("b.txt", 10);
        let start = list.start_next().remove(0);
        assert!(list.cancel(queued));
        assert_eq!(list.get(queued).unwrap().state, State::Cancelled);
        assert!(list.cancel(running));
        assert!(start.cancel.is_cancelled(), "the worker's handle");
        assert!(list.get(running).unwrap().is_cancelling());
        assert!(!list.cancel(running), "once");
        assert!(list.start_next().is_empty(), "the slot is still taken");
        // Whatever the worker failed with after the cancel, it was cancelled.
        list.finish_upload(
            running,
            Err(FileError::Host(Error::Io(std::io::Error::other(
                "shut down",
            )))),
        );
        assert_eq!(list.get(running).unwrap().state, State::Cancelled);
        assert!(!list.get(running).unwrap().is_cancelling());
        // A retry gets a fresh handle: the old one cancels every transfer started with it.
        assert!(list.retry(running));
        let again = list.start_next().remove(0);
        assert_eq!(again.id, running);
        assert!(!again.cancel.is_cancelled());
        // One that finished before the cancel took hold is done.
        list.cancel(running);
        list.finish_upload(
            running,
            Ok(Uploaded {
                name: "a.txt".into(),
                size: 10,
            }),
        );
        assert!(matches!(
            list.get(running).unwrap().state,
            State::Done { .. }
        ));
    }

    #[test]
    fn cancel_all_and_clear_finished() {
        let mut list = Transfers::new();
        let a = list.queue_download("a", "a", 1);
        let b = list.queue_download("b", "b", 1);
        let c = list.queue_download("c", "c", 1);
        list.start_next();
        list.finish_download(a, Ok(1));
        assert!(list.is_active());
        list.cancel_all();
        assert_eq!(list.get(c).unwrap().state, State::Cancelled);
        assert!(list.get(b).unwrap().is_cancelling());
        list.finish_download(b, Err(FileError::Host(Error::Cancelled)));
        assert!(!list.is_active());
        list.clear_finished();
        assert!(list.items().is_empty());
        // Ids are never reused.
        assert_eq!(list.queue_upload("d", 1), c + 1);
    }

    #[test]
    fn a_cancel_error_without_a_cancel_request_is_still_cancelled() {
        let mut list = Transfers::new();
        let id = list.queue_upload("a", 1);
        list.start_next();
        list.finish_upload(id, Err(FileError::Host(Error::Cancelled)));
        assert_eq!(list.get(id).unwrap().state, State::Cancelled);
    }

    #[test]
    fn upload_names_are_ones_the_host_takes() {
        // The host's own check of these names runs in its interop tests, which build this crate.
        for (local, sent) in [
            ("photo.jpg", "photo.jpg"),
            (
                "Screenshot 2026-10-27 at 10:15:00.png",
                "Screenshot 2026-10-27 at 10_15_00.png",
            ),
            ("a<b>c\"d|e?f*g.txt", "a_b_c_d_e_f_g.txt"),
            ("tab\there.txt", "tab_here.txt"),
            ("\u{202e}txt.exe", "txt.exe"),
            ("trailing. . ", "trailing"),
            ("CON.txt", "_CON.txt"),
            ("nul", "_nul"),
            ("lpt¹.log", "_lpt¹.log"),
            ("movie.mp4.part", "movie.mp4.part_"),
            ("MOVIE.PART", "MOVIE.PART_"),
            ("/storage/emulated/0/DCIM/clip.mp4", "clip.mp4"),
            ("C:\\Users\\me\\clip.mp4", "clip.mp4"),
            ("", "file"),
            ("...", "file"),
            ("..", "file"),
            ("emoji 👨‍👩‍👧.png", "emoji 👨‍👩‍👧.png"),
        ] {
            assert_eq!(upload_name(local), sent, "{local:?}");
        }
    }

    #[test]
    fn long_names_keep_their_extension() {
        let long = format!("{}.mkv", "x".repeat(300));
        let name = upload_name(&long);
        assert_eq!(name.encode_utf16().count(), MAX_NAME_UNITS);
        assert!(name.ends_with("xx.mkv"));
        // Characters outside the BMP count two units and are never split.
        let wide = format!("{}.txt", "😀".repeat(150));
        let name = upload_name(&wide);
        assert!(name.encode_utf16().count() <= MAX_NAME_UNITS);
        assert!(name.ends_with("😀.txt"));
        // A long "extension" is part of the name.
        let dotted = format!("a.{}", "y".repeat(300));
        assert_eq!(upload_name(&dotted).encode_utf16().count(), MAX_NAME_UNITS);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("nightjar-transfers-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("saved.bin")
    }

    #[test]
    fn a_download_lands_whole_or_not_at_all() {
        let dest = scratch("whole");
        std::fs::write(&dest, b"old").unwrap();
        // Cut short: the old file stays and the partial one is gone.
        let result = download_into(&dest, |sink| {
            sink.write_all(b"new but sh").unwrap();
            Err(FileError::Host(Error::Malformed(
                "the download ended after 10 of 20 bytes".into(),
            )))
        });
        assert!(matches!(result, Err(FileError::Host(Error::Malformed(_)))));
        assert_eq!(std::fs::read(&dest).unwrap(), b"old");
        assert!(!partial_path(&dest).exists());
        // Cancelled: the same.
        let result = download_into(&dest, |sink| {
            sink.write_all(b"x").unwrap();
            Err(FileError::Host(Error::Cancelled))
        });
        assert!(matches!(result, Err(FileError::Host(Error::Cancelled))));
        assert!(!partial_path(&dest).exists());
        // Whole: it replaces the old file.
        let result = download_into(&dest, |sink| {
            sink.write_all(b"new and whole").unwrap();
            Ok(13)
        });
        assert_eq!(result.unwrap(), 13);
        assert_eq!(std::fs::read(&dest).unwrap(), b"new and whole");
        assert!(!partial_path(&dest).exists());
        std::fs::remove_dir_all(dest.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_download_into_a_missing_folder_is_a_local_failure() {
        let dest = std::env::temp_dir()
            .join("nightjar-no-such-folder-7f3a")
            .join("x.bin");
        let mut called = false;
        let result = download_into(&dest, |_| {
            called = true;
            Ok(0)
        });
        assert!(matches!(result, Err(FileError::Local(_))));
        assert!(!called, "nothing is fetched without a place to put it");
    }

    #[test]
    fn the_partial_file_sits_beside_the_destination() {
        assert_eq!(
            partial_path(Path::new("/home/u/Downloads/clip (1).mp4")),
            Path::new("/home/u/Downloads/clip (1).mp4.part")
        );
    }
}
