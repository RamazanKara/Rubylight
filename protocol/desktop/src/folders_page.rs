//! The folders a PC shares with this device, on its details page (Experimental): the folders,
//! their directories, a file downloaded into a folder chosen here, and a file chosen here
//! uploaded into a directory with a write grant. One request and one transfer at a time, each
//! on a worker thread; leaving the page cancels both.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use nightjar_core::http::{
    child_path, parent_path, ClientIdentity, Error, FileError, FileRefusal, FolderEntries,
    HostClient, SharedFolder, TransferCancel, Uploaded,
};

use crate::hosts::{client_for, Host, Waker};
use crate::strings::{en, tr, trf};

/// The shared folder calls against one PC; `HostClient` in the app, a fake in tests.
pub trait FolderSource: Send + Sync {
    fn folders(&self) -> Result<Vec<SharedFolder>, FileError>;
    fn entries(&self, id: &str, path: &str) -> Result<FolderEntries, FileError>;
    fn save(
        &self,
        id: &str,
        path: &str,
        listed: u64,
        dir: &Path,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<PathBuf, FileError>;
    fn upload(
        &self,
        id: &str,
        path: &str,
        file: &mut dyn std::io::Read,
        length: u64,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<Uploaded, FileError>;
    /// Ends every request in flight, and every later one, with `Error::Cancelled`.
    fn cancel(&self);
}

impl FolderSource for HostClient {
    fn folders(&self) -> Result<Vec<SharedFolder>, FileError> {
        self.shared_folders()
    }

    fn entries(&self, id: &str, path: &str) -> Result<FolderEntries, FileError> {
        self.folder_entries(id, path)
    }

    fn save(
        &self,
        id: &str,
        path: &str,
        listed: u64,
        dir: &Path,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<PathBuf, FileError> {
        self.save_folder_file(id, path, listed, dir, progress, stop)
    }

    fn upload(
        &self,
        id: &str,
        path: &str,
        file: &mut dyn std::io::Read,
        length: u64,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<Uploaded, FileError> {
        self.upload_folder_file(id, path, file, length, progress, stop)
    }

    fn cancel(&self) {
        self.cancel_pending_requests();
    }
}

/// Where the browser is: the list of folders, or a directory of one (`path` empty for the
/// folder itself).
#[derive(Clone, Debug, PartialEq)]
pub enum Place {
    Folders,
    Dir { folder: SharedFolder, path: String },
}

/// Bytes done and in all of a transfer, as the worker reports them.
#[derive(Default)]
pub struct Progress {
    done: AtomicU64,
    total: AtomicU64,
}

impl Progress {
    pub fn get(&self) -> (u64, u64) {
        (
            self.done.load(Ordering::Relaxed),
            self.total.load(Ordering::Relaxed),
        )
    }
}

pub struct Transfer {
    pub upload: bool,
    pub name: String,
    pub progress: Arc<Progress>,
    stop: Arc<TransferCancel>,
}

enum Answer {
    Folders(u64, Result<Vec<SharedFolder>, FileError>),
    Entries(u64, Result<FolderEntries, FileError>),
    /// A finished transfer: what to say, and whether the directory changed.
    Done(Result<String, FileError>, bool),
}

/// The browser of the page whose PC it was opened for.
#[derive(Default)]
pub struct SharedFolders {
    open: Option<Open>,
    /// The PC whose browser could not open, and why.
    failed: Option<(u64, String)>,
}

pub struct Open {
    pub host: u64,
    source: Arc<dyn FolderSource>,
    waker: Waker,
    pub place: Place,
    pub folders: Vec<SharedFolder>,
    /// The listed directory of `place`, once it came.
    pub entries: Option<FolderEntries>,
    /// A listing is on its way.
    pub loading: bool,
    /// The number of the listing being waited for; a later one replaces it.
    ask: u64,
    pub transfer: Option<Transfer>,
    /// Why the listing failed.
    pub error: Option<String>,
    /// Why the last transfer failed or did not start; a listing neither shows nor clears it.
    pub transfer_error: Option<String>,
    /// What the last transfer did.
    pub done: Option<String>,
    answers: Receiver<Answer>,
    sender: Sender<Answer>,
}

impl Drop for Open {
    fn drop(&mut self) {
        if let Some(transfer) = &self.transfer {
            transfer.stop.cancel();
        }
        self.source.cancel();
    }
}

/// Command::SharedFolders: opens the browser of `host` with a client of its own, since closing
/// the browser cancels that client's requests for good.
pub fn start(
    folders: &mut SharedFolders,
    identity: Option<&Arc<ClientIdentity>>,
    host: &Host,
    waker: Waker,
) {
    let client = match identity {
        Some(identity) => client_for(identity, &host.record, host.connect_address()),
        None => Err(
            tr("Nightjar has no client identity yet. Restart Nightjar and try again.").to_owned(),
        ),
    };
    match client {
        Ok(client) => folders.open(host.id, Arc::new(client), waker),
        Err(error) => folders.fail(host.id, error),
    }
}

impl SharedFolders {
    /// Opens the browser for `host` on its list of folders.
    pub fn open(&mut self, host: u64, source: Arc<dyn FolderSource>, waker: Waker) {
        let (sender, answers) = channel();
        self.failed = None;
        self.open = Some(Open {
            host,
            source,
            waker,
            place: Place::Folders,
            folders: Vec::new(),
            entries: None,
            loading: false,
            ask: 0,
            transfer: None,
            error: None,
            transfer_error: None,
            done: None,
            answers,
            sender,
        });
        self.list();
    }

    /// The browser could not open: says why on the page.
    pub fn fail(&mut self, host: u64, error: String) {
        self.open = None;
        self.failed = Some((host, error));
    }

    pub fn close(&mut self) {
        self.open = None;
        self.failed = None;
    }

    /// Why the browser of `host` could not open.
    pub fn failure_for(&self, host: u64) -> Option<&str> {
        self.failed
            .as_ref()
            .filter(|(h, _)| *h == host)
            .map(|(_, e)| e.as_str())
    }

    pub fn state_for(&self, host: u64) -> Option<&Open> {
        self.open.as_ref().filter(|open| open.host == host)
    }

    fn for_host(&mut self, host: u64) -> Option<&mut Open> {
        self.open.as_mut().filter(|open| open.host == host)
    }

    /// Lists the place the browser is at again.
    pub fn list(&mut self) {
        let Some(open) = &mut self.open else { return };
        open.ask += 1;
        open.loading = true;
        open.error = None;
        let (ask, source, sender, waker) = (
            open.ask,
            open.source.clone(),
            open.sender.clone(),
            open.waker.clone(),
        );
        let place = open.place.clone();
        let started = spawn("shared folders", move || {
            let answer = match place {
                Place::Folders => Answer::Folders(ask, source.folders()),
                Place::Dir { folder, path } => {
                    Answer::Entries(ask, source.entries(&folder.id, &path))
                }
            };
            let _ = sender.send(answer);
            waker();
        });
        if let Err(e) = started {
            open.loading = false;
            open.error = Some(e);
        }
    }

    fn go(&mut self, place: Place) {
        if let Some(open) = &mut self.open {
            open.place = place;
            open.entries = None;
            open.done = None;
            open.transfer_error = None;
            self.list();
        }
    }

    /// Says on the page of `host` why a transfer did not start.
    pub fn transfer_refused(&mut self, host: u64, why: String) {
        if let Some(open) = self.for_host(host) {
            open.done = None;
            open.transfer_error = Some(why);
        }
    }

    /// Opens `folder` at its top.
    pub fn enter_folder(&mut self, host: u64, folder: SharedFolder) {
        if self.for_host(host).is_some() {
            self.go(Place::Dir {
                folder,
                path: String::new(),
            });
        }
    }

    /// Opens the directory `name` of the listed one.
    pub fn enter_dir(&mut self, host: u64, name: &str) {
        let Some(Place::Dir { folder, path }) = self.for_host(host).map(|open| open.place.clone())
        else {
            return;
        };
        self.go(Place::Dir {
            folder,
            path: child_path(&path, name),
        });
    }

    /// One directory up; from a folder's top, back to the folders.
    pub fn up(&mut self, host: u64) {
        let Some(place) = self.for_host(host).map(|open| open.place.clone()) else {
            return;
        };
        match place {
            Place::Folders => {}
            Place::Dir { path, .. } if path.is_empty() => self.go(Place::Folders),
            Place::Dir { folder, path } => self.go(Place::Dir {
                path: parent_path(&path).to_owned(),
                folder,
            }),
        }
    }

    /// Downloads the file `name` of the directory `at` into `dir`. `at` is where the browser was
    /// when the file was picked; it may have moved on while the folder here was chosen. False
    /// when the browser of `host` is closed, so the page that chose it is gone.
    pub fn download(&mut self, host: u64, at: &Place, name: &str, size: u64, dir: PathBuf) -> bool {
        let Some(open) = self.for_host(host) else {
            return false;
        };
        // Only a directory lists files to pick.
        let Place::Dir { folder, path } = at.clone() else {
            return true;
        };
        if refuse_second_transfer(open) {
            return true;
        }
        let remote = child_path(&path, name);
        start_transfer(
            open,
            false,
            name.to_owned(),
            Box::new(
                move |source: &dyn FolderSource,
                      progress: &mut dyn FnMut(u64, u64),
                      stop: &TransferCancel| {
                    source
                        .save(&folder.id, &remote, size, &dir, progress, stop)
                        .map(|saved| (trf("Saved to {}", &[&saved.display()]), false))
                },
            ),
        );
        true
    }

    /// Uploads the file at `local` into the directory `at`, under its own name; `at` and the
    /// answer as for [`Self::download`].
    pub fn upload(&mut self, host: u64, at: &Place, local: PathBuf) -> bool {
        let Some(open) = self.for_host(host) else {
            return false;
        };
        // Only a directory with a write grant offers an upload.
        let Place::Dir { folder, path } = at.clone() else {
            return true;
        };
        if refuse_second_transfer(open) {
            return true;
        }
        let Some(name) = local
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_owned)
        else {
            open.done = None;
            open.transfer_error = Some(trf(
                "The PC takes only file names in Unicode: {}",
                &[&local.display()],
            ));
            return true;
        };
        let remote = child_path(&path, &name);
        start_transfer(
            open,
            true,
            name,
            Box::new(
                move |source: &dyn FolderSource,
                      progress: &mut dyn FnMut(u64, u64),
                      stop: &TransferCancel| {
                    let mut file = std::fs::File::open(&local).map_err(FileError::Local)?;
                    let length = file.metadata().map_err(FileError::Local)?.len();
                    source
                        .upload(&folder.id, &remote, &mut file, length, progress, stop)
                        .map(|up| (trf("Uploaded as {}", &[&up.name]), true))
                },
            ),
        );
        true
    }

    pub fn cancel_transfer(&mut self, host: u64) {
        if let Some(transfer) = self.for_host(host).and_then(|open| open.transfer.as_ref()) {
            transfer.stop.cancel();
        }
    }

    /// Whether a file goes to or from a shared folder, which leaving the PC's page cancels.
    pub fn transferring(&self) -> bool {
        self.open.as_ref().is_some_and(|open| open.transfer.is_some())
    }

    /// Takes in what the workers found. `page` is the PC whose details page is open or on the
    /// way back: leaving it closes the browser, which cancels what is running.
    pub fn poll(&mut self, page: Option<u64>) {
        if self
            .failed
            .as_ref()
            .is_some_and(|(host, _)| page != Some(*host))
        {
            self.failed = None;
        }
        if self
            .open
            .as_ref()
            .is_some_and(|open| page != Some(open.host))
        {
            self.open = None;
            return;
        }
        let mut relist = false;
        let Some(open) = &mut self.open else { return };
        while let Ok(answer) = open.answers.try_recv() {
            match answer {
                Answer::Folders(ask, _) | Answer::Entries(ask, _) if ask != open.ask => {}
                Answer::Folders(_, result) => {
                    open.loading = false;
                    match result {
                        Ok(folders) => open.folders = folders,
                        Err(e) => open.error = Some(folder_error(&e)),
                    }
                }
                Answer::Entries(_, result) => {
                    open.loading = false;
                    match result {
                        Ok(entries) => open.entries = Some(entries),
                        Err(e) => open.error = Some(folder_error(&e)),
                    }
                }
                Answer::Done(result, changed) => {
                    open.transfer = None;
                    match result {
                        Ok(said) => {
                            open.done = Some(said);
                            open.transfer_error = None;
                            relist |= changed;
                        }
                        Err(e) => open.transfer_error = Some(folder_error(&e)),
                    }
                }
            }
        }
        if relist {
            self.list();
        }
    }
}

type Job = dyn FnOnce(
        &dyn FolderSource,
        &mut dyn FnMut(u64, u64),
        &TransferCancel,
    ) -> Result<(String, bool), FileError>
    + Send;

fn start_transfer(open: &mut Open, upload: bool, name: String, job: Box<Job>) {
    let transfer = Transfer {
        upload,
        name,
        progress: Arc::default(),
        stop: Arc::new(TransferCancel::new()),
    };
    let (source, sender, waker, progress, stop) = (
        open.source.clone(),
        open.sender.clone(),
        open.waker.clone(),
        transfer.progress.clone(),
        transfer.stop.clone(),
    );
    open.transfer_error = None;
    open.done = None;
    open.transfer = Some(transfer);
    let started = spawn("shared folder transfer", move || {
        let mut report = |done, total| {
            progress.done.store(done, Ordering::Relaxed);
            progress.total.store(total, Ordering::Relaxed);
            waker();
        };
        let result = job(source.as_ref(), &mut report, &stop);
        let (said, changed) = match result {
            Ok((said, changed)) => (Ok(said), changed),
            Err(e) => (Err(e), false),
        };
        let _ = sender.send(Answer::Done(said, changed));
        waker();
    });
    if let Err(e) = started {
        open.transfer = None;
        open.transfer_error = Some(e);
    }
}

/// One transfer at a time: a second one says so on the page instead of starting.
fn refuse_second_transfer(open: &mut Open) -> bool {
    if open.transfer.is_none() {
        return false;
    }
    open.done = None;
    open.transfer_error = Some(tr("Another transfer is still running.").to_owned());
    true
}

/// Runs `work` on its own thread; why it could not start, as the page says it.
fn spawn(name: &str, work: impl FnOnce() + Send + 'static) -> Result<(), String> {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(work)
        .map(drop)
        .map_err(|e| trf("Could not start the request: {}", &[&e]))
}

/// What failed, in the page's words, then the error as it came.
pub fn folder_error(error: &FileError) -> String {
    let why = match error {
        FileError::Refused { refusal, .. } => match refusal {
            FileRefusal::Forbidden => Some(en(
                "This device has no grant for this folder, or may only read it.",
            )),
            FileRefusal::FoldersDisabled => Some(en("The PC shares no folders.")),
            FileRefusal::FolderUnreadable => Some(en(
                "The PC can’t read this folder: Windows denies its signed-in user access. Check the folder’s permissions on the PC.",
            )),
            FileRefusal::NoConsoleUser => Some(en(
                "Nobody is signed in on the PC. Shared folders open as the signed-in Windows user.",
            )),
            FileRefusal::NotFound => Some(en("The file or folder is no longer there.")),
            FileRefusal::TooLarge => Some(en("The file is larger than the PC takes.")),
            FileRefusal::InsufficientStorage => Some(en("The PC’s drive is full.")),
            FileRefusal::TransferBusy => {
                Some(en("Another upload from this device is still running."))
            }
            FileRefusal::InvalidName => Some(en("The PC does not take this name.")),
            _ => None,
        },
        FileError::Local(_) => Some(en("The file on this device could not be read or written.")),
        FileError::Host(Error::Cancelled) => return tr("Transfer cancelled.").to_owned(),
        FileError::Host(_) => None,
    };
    match why {
        Some(why) => format!("{}\n{error}", tr(why)),
        None => error.to_string(),
    }
}

/// A size as the page shows it.
pub fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

#[cfg(test)]
pub(crate) mod tests;
