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
        dir: &Path,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<PathBuf, FileError> {
        self.save_folder_file(id, path, dir, progress, stop)
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
    pub error: Option<String>,
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
        Some(identity) => client_for(identity, &host.record, &host.record.address),
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
        spawn(
            "shared folders",
            move || {
                let answer = match place {
                    Place::Folders => Answer::Folders(ask, source.folders()),
                    Place::Dir { folder, path } => {
                        Answer::Entries(ask, source.entries(&folder.id, &path))
                    }
                };
                let _ = sender.send(answer);
                waker();
            },
            open,
        );
    }

    fn go(&mut self, place: Place) {
        if let Some(open) = &mut self.open {
            open.place = place;
            open.entries = None;
            open.done = None;
            self.list();
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
    /// when the file was picked; it may have moved on while the folder here was chosen.
    pub fn download(&mut self, host: u64, at: &Place, name: &str, dir: PathBuf) {
        let Some(open) = self.for_host(host) else {
            return;
        };
        let Place::Dir { folder, path } = at.clone() else {
            return;
        };
        if open.transfer.is_some() {
            return;
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
                        .save(&folder.id, &remote, &dir, progress, stop)
                        .map(|saved| (trf("Saved to {}", &[&saved.display()]), false))
                },
            ),
        );
    }

    /// Uploads the file at `local` into the directory `at`, under its own name; `at` as for
    /// [`Self::download`].
    pub fn upload(&mut self, host: u64, at: &Place, local: PathBuf) {
        let Some(open) = self.for_host(host) else {
            return;
        };
        let Place::Dir { folder, path } = at.clone() else {
            return;
        };
        if open.transfer.is_some() {
            return;
        }
        let Some(name) = local
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_owned)
        else {
            open.error = Some(trf(
                "The PC takes only file names in Unicode: {}",
                &[&local.display()],
            ));
            return;
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
    }

    pub fn cancel_transfer(&mut self, host: u64) {
        if let Some(transfer) = self.for_host(host).and_then(|open| open.transfer.as_ref()) {
            transfer.stop.cancel();
        }
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
                            relist |= changed;
                        }
                        Err(e) => open.error = Some(folder_error(&e)),
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
    open.error = None;
    open.done = None;
    open.transfer = Some(transfer);
    spawn(
        "shared folder transfer",
        move || {
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
        },
        open,
    );
}

/// Runs `work` on its own thread; one that cannot start says why on the page.
fn spawn(name: &str, work: impl FnOnce() + Send + 'static, open: &mut Open) {
    if let Err(e) = std::thread::Builder::new().name(name.into()).spawn(work) {
        open.loading = false;
        open.transfer = None;
        open.error = Some(trf("Could not start the request: {}", &[&e]));
    }
}

/// What failed, in the page's words, then the error as it came.
pub fn folder_error(error: &FileError) -> String {
    let why = match error {
        FileError::Refused { refusal, .. } => match refusal {
            FileRefusal::Forbidden => Some(en(
                "This device has no grant for this folder, or may only read it.",
            )),
            FileRefusal::FoldersDisabled => Some(en("The PC shares no folders.")),
            FileRefusal::NoConsoleUser => Some(en(
                "Nobody is signed in on the PC. Shared folders open as the signed-in Windows user.",
            )),
            FileRefusal::NotFound => Some(en("The file or folder is no longer there.")),
            FileRefusal::TooLarge => Some(en("The file is larger than the PC takes.")),
            FileRefusal::InsufficientStorage => Some(en("The PC's drive is full.")),
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
mod tests {
    use super::*;
    use nightjar_core::http::{EntryKind, FolderAccess, FolderEntry};
    use std::sync::Mutex;

    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<String>>,
        /// Uploads read the whole file here.
        uploaded: Mutex<Vec<u8>>,
        cancelled: std::sync::atomic::AtomicBool,
        refuse: Option<(FileRefusal, u16)>,
    }

    impl Fake {
        fn refused(&self) -> Result<(), FileError> {
            match self.refuse {
                Some((refusal, status)) => Err(FileError::Refused {
                    refusal,
                    status,
                    message: "nope".into(),
                }),
                None => Ok(()),
            }
        }
    }

    impl FolderSource for Fake {
        fn folders(&self) -> Result<Vec<SharedFolder>, FileError> {
            self.calls.lock().unwrap().push("folders".into());
            self.refused()?;
            Ok(vec![SharedFolder {
                id: "saves".into(),
                name: "Saves".into(),
                access: FolderAccess::Write,
            }])
        }

        fn entries(&self, id: &str, path: &str) -> Result<FolderEntries, FileError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("entries {id} {path:?}"));
            self.refused()?;
            let file = FolderEntry {
                name: "a.txt".into(),
                kind: EntryKind::File,
                size: 3,
                modified_unix_ms: 0,
            };
            Ok(FolderEntries {
                path: path.into(),
                entries: vec![file],
            })
        }

        fn save(
            &self,
            id: &str,
            path: &str,
            dir: &Path,
            progress: &mut dyn FnMut(u64, u64),
            _: &TransferCancel,
        ) -> Result<PathBuf, FileError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("save {id} {path:?}"));
            self.refused()?;
            progress(3, 3);
            Ok(dir.join(nightjar_core::http::file_name(path)))
        }

        fn upload(
            &self,
            id: &str,
            path: &str,
            file: &mut dyn std::io::Read,
            length: u64,
            progress: &mut dyn FnMut(u64, u64),
            _: &TransferCancel,
        ) -> Result<Uploaded, FileError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("upload {id} {path:?} {length}"));
            self.refused()?;
            file.read_to_end(&mut self.uploaded.lock().unwrap())
                .map_err(FileError::Local)?;
            progress(length, length);
            Ok(Uploaded {
                name: "b (1).txt".into(),
                size: length,
            })
        }

        fn cancel(&self) {
            self.cancelled.store(true, Ordering::SeqCst);
        }
    }

    fn waker() -> Waker {
        Arc::new(|| {})
    }

    /// Polls until nothing is running.
    fn settle(folders: &mut SharedFolders, host: u64) {
        for _ in 0..500 {
            folders.poll(Some(host));
            if folders
                .state_for(host)
                .is_some_and(|open| !open.loading && open.transfer.is_none())
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the browser did not settle");
    }

    fn calls(fake: &Fake) -> Vec<String> {
        fake.calls.lock().unwrap().clone()
    }

    #[test]
    fn the_browser_lists_folders_walks_directories_and_goes_back_up() {
        let fake = Arc::new(Fake::default());
        let mut folders = SharedFolders::default();
        folders.open(7, fake.clone(), waker());
        settle(&mut folders, 7);
        let folder = folders.state_for(7).unwrap().folders[0].clone();
        assert!(folder.writable());
        folders.enter_folder(7, folder.clone());
        settle(&mut folders, 7);
        folders.enter_dir(7, "Slot 1");
        settle(&mut folders, 7);
        assert_eq!(
            folders.state_for(7).unwrap().place,
            Place::Dir {
                folder: folder.clone(),
                path: "Slot 1".into()
            }
        );
        assert_eq!(
            folders.state_for(7).unwrap().entries.as_ref().unwrap().path,
            "Slot 1"
        );
        folders.up(7);
        settle(&mut folders, 7);
        folders.up(7);
        settle(&mut folders, 7);
        assert_eq!(folders.state_for(7).unwrap().place, Place::Folders);
        assert_eq!(
            calls(&fake),
            [
                "folders",
                "entries saves \"\"",
                "entries saves \"Slot 1\"",
                "entries saves \"\"",
                "folders"
            ]
        );
        // Another PC's page sees nothing of it, and does nothing to it.
        assert!(folders.state_for(8).is_none());
        folders.enter_dir(8, "x");
        assert_eq!(calls(&fake).len(), 5);
    }

    #[test]
    fn a_download_says_where_the_file_went_and_an_upload_its_name_then_lists_again() {
        let fake = Arc::new(Fake::default());
        let mut folders = SharedFolders::default();
        folders.open(7, fake.clone(), waker());
        settle(&mut folders, 7);
        folders.enter_folder(7, folders.state_for(7).unwrap().folders[0].clone());
        settle(&mut folders, 7);
        // The file is picked at the folder's top, and the browser moves on while the folder
        // here is chosen: the download is still the picked file.
        let picked = folders.state_for(7).unwrap().place.clone();
        folders.enter_dir(7, "Slot 1");
        settle(&mut folders, 7);
        let dir = std::env::temp_dir();
        folders.download(7, &picked, "a.txt", dir.clone());
        assert!(folders
            .state_for(7)
            .unwrap()
            .transfer
            .as_ref()
            .is_some_and(|t| !t.upload && t.name == "a.txt"));
        settle(&mut folders, 7);
        let said = folders.state_for(7).unwrap().done.clone().unwrap();
        assert!(
            said.contains(&dir.join("a.txt").display().to_string()),
            "{said}"
        );

        let local = std::env::temp_dir().join(format!(
            "nightjar-folders-upload-{}.txt",
            std::process::id()
        ));
        std::fs::write(&local, b"abc").unwrap();
        folders.upload(7, &picked, local.clone());
        settle(&mut folders, 7);
        std::fs::remove_file(&local).unwrap();
        let open = folders.state_for(7).unwrap();
        assert_eq!(open.error, None);
        assert!(open.done.as_deref().unwrap().contains("b (1).txt"));
        assert_eq!(*fake.uploaded.lock().unwrap(), b"abc");
        let name = local.file_name().unwrap().to_str().unwrap();
        // The upload lists the browser's directory again, so a new file there shows.
        assert_eq!(
            calls(&fake)[3..],
            [
                "save saves \"a.txt\"".to_owned(),
                format!("upload saves {name:?} 3"),
                "entries saves \"Slot 1\"".into()
            ]
        );
    }

    #[test]
    fn refusals_and_local_failures_are_shown_by_name() {
        let fake = Arc::new(Fake {
            refuse: Some((FileRefusal::NoConsoleUser, 409)),
            ..Default::default()
        });
        let mut folders = SharedFolders::default();
        folders.open(7, fake, waker());
        settle(&mut folders, 7);
        let error = folders.state_for(7).unwrap().error.clone().unwrap();
        assert!(
            error.contains("Nobody is signed in")
                && error.contains("409")
                && error.contains("nope"),
            "{error}"
        );

        let fake = Arc::new(Fake::default());
        folders.open(7, fake.clone(), waker());
        settle(&mut folders, 7);
        folders.enter_folder(7, folders.state_for(7).unwrap().folders[0].clone());
        settle(&mut folders, 7);
        let at = folders.state_for(7).unwrap().place.clone();
        folders.upload(
            7,
            &at,
            std::env::temp_dir().join("nightjar-no-such-file-to-upload.bin"),
        );
        settle(&mut folders, 7);
        let error = folders.state_for(7).unwrap().error.clone().unwrap();
        assert!(
            error.starts_with("The file on this device could not be read"),
            "{error}"
        );
        assert!(!calls(&fake).iter().any(|c| c.starts_with("upload")));
        assert_eq!(
            folder_error(&FileError::Host(Error::Cancelled)),
            "Transfer cancelled."
        );
    }

    #[test]
    fn leaving_the_page_cancels_and_a_late_listing_is_dropped() {
        let fake = Arc::new(Fake::default());
        let mut folders = SharedFolders::default();
        folders.open(7, fake.clone(), waker());
        folders.poll(None);
        assert!(folders.state_for(7).is_none());
        assert!(fake.cancelled.load(Ordering::SeqCst));

        // Of two listings, only the later one counts.
        let fake = Arc::new(Fake::default());
        folders.open(7, fake.clone(), waker());
        settle(&mut folders, 7);
        let folder = folders.state_for(7).unwrap().folders[0].clone();
        folders.enter_folder(7, folder.clone());
        folders.up(7);
        settle(&mut folders, 7);
        let open = folders.state_for(7).unwrap();
        assert_eq!(open.place, Place::Folders);
        assert!(open.entries.is_none());
    }

    #[test]
    fn sizes_read_like_a_file_manager() {
        assert_eq!(size_text(999), "999 B");
        assert_eq!(size_text(1500), "1.5 KB");
        assert_eq!(size_text(2_400_000), "2.4 MB");
        assert_eq!(size_text(5_000_000_000_000_000), "5000.0 TB");
    }
}
