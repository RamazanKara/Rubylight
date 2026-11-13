use super::*;
use nightjar_core::http::{EntryKind, FolderAccess, FolderEntry};
use std::sync::Mutex;

#[derive(Default)]
pub(crate) struct Fake {
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
        listed: u64,
        dir: &Path,
        progress: &mut dyn FnMut(u64, u64),
        _: &TransferCancel,
    ) -> Result<PathBuf, FileError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("save {id} {path:?} {listed}"));
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

pub(crate) fn waker() -> Waker {
    Arc::new(|| {})
}

/// Polls until nothing is running.
pub(crate) fn settle(folders: &mut SharedFolders, host: u64) {
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

pub(crate) fn calls(fake: &Fake) -> Vec<String> {
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
    folders.download(7, &picked, "a.txt", 3, dir.clone());
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
            "save saves \"a.txt\" 3".to_owned(),
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
    let error = folders
        .state_for(7)
        .unwrap()
        .transfer_error
        .clone()
        .unwrap();
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

fn saves() -> Place {
    Place::Dir {
        folder: SharedFolder {
            id: "saves".into(),
            name: "Saves".into(),
            access: FolderAccess::Write,
        },
        path: String::new(),
    }
}

#[test]
fn transfer_errors_stay_apart_from_listing_errors() {
    // A failed upload is the transfer's: listing again neither hides nor clears it.
    let fake = Arc::new(Fake::default());
    let mut folders = SharedFolders::default();
    folders.open(7, fake, waker());
    settle(&mut folders, 7);
    folders.enter_folder(7, folders.state_for(7).unwrap().folders[0].clone());
    settle(&mut folders, 7);
    let missing = std::env::temp_dir().join("nightjar-no-such-file-to-upload.bin");
    assert!(folders.upload(7, &saves(), missing));
    settle(&mut folders, 7);
    folders.list();
    settle(&mut folders, 7);
    let open = folders.state_for(7).unwrap();
    assert!(open.transfer_error.is_some(), "{:?}", open.error);
    assert_eq!(open.error, None, "the listing worked");
    assert!(open.entries.is_some());

    // A failed listing stays when a transfer starts and fails on its own.
    let refusing = Arc::new(Fake {
        refuse: Some((FileRefusal::NotFound, 404)),
        ..Default::default()
    });
    folders.open(7, refusing, waker());
    settle(&mut folders, 7);
    let listing = folders.state_for(7).unwrap().error.clone().unwrap();
    assert!(folders.download(7, &saves(), "a.txt", 3, std::env::temp_dir()));
    settle(&mut folders, 7);
    let open = folders.state_for(7).unwrap();
    assert_eq!(open.error.as_ref(), Some(&listing));
    let transfer = open.transfer_error.as_deref().unwrap();
    assert!(transfer.contains("no longer there"), "{transfer}");
    // A transfer that works clears only its own error.
    let fake = Arc::new(Fake::default());
    folders.open(7, fake, waker());
    settle(&mut folders, 7);
    folders.transfer_refused(7, "earlier".into());
    assert!(folders.download(7, &saves(), "a.txt", 3, std::env::temp_dir()));
    settle(&mut folders, 7);
    let open = folders.state_for(7).unwrap();
    assert_eq!(
        (open.error.as_deref(), open.transfer_error.as_deref()),
        (None, None)
    );
    assert!(open.done.is_some());
}

#[test]
fn a_second_transfer_says_one_is_running_and_a_closed_browser_takes_none() {
    let fake = Arc::new(Fake::default());
    let mut folders = SharedFolders::default();
    folders.open(7, fake.clone(), waker());
    settle(&mut folders, 7);
    assert!(folders.download(7, &saves(), "a.txt", 3, std::env::temp_dir()));
    assert!(folders.transferring(), "leaving the page now would cancel the download");
    let local = std::env::temp_dir().join("nightjar-second-transfer.txt");
    assert!(folders.upload(7, &saves(), local.clone()));
    let said = folders.state_for(7).unwrap().transfer_error.clone();
    assert_eq!(said.as_deref(), Some("Another transfer is still running."));
    settle(&mut folders, 7);
    assert!(!folders.transferring());
    assert!(!calls(&fake).iter().any(|c| c.starts_with("upload")));

    // Leaving the page closes the browser: what was chosen for it is not started.
    folders.poll(None);
    assert!(!folders.download(7, &saves(), "a.txt", 3, std::env::temp_dir()));
    assert!(!folders.upload(7, &saves(), local));
    assert_eq!(calls(&fake).len(), 2, "{:?}", calls(&fake));
}

#[test]
fn sizes_read_like_a_file_manager() {
    assert_eq!(size_text(999), "999 B");
    assert_eq!(size_text(1500), "1.5 KB");
    assert_eq!(size_text(2_400_000), "2.4 MB");
    assert_eq!(size_text(5_000_000_000_000_000), "5000.0 TB");
}

#[test]
fn a_folder_windows_denies_says_the_pc_cannot_read_it() {
    let error = folder_error(&FileError::Refused {
        refusal: FileRefusal::FolderUnreadable,
        status: 403,
        message: "denied".into(),
    });
    assert!(
        error.starts_with("The PC can’t read this folder: Windows denies"),
        "{error}"
    );
}
