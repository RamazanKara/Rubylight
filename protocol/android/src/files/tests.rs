use super::*;
use nightjar_core::http::{FileRefusal, RemoteFile};
use std::io::{Seek, SeekFrom};

/// Sends and receives in 4-byte steps, failing as told.
#[derive(Default)]
struct Fake {
    refuse: Option<(FileRefusal, u16)>,
    content: Vec<u8>,
    /// Cancels the transfer after this many bytes, as a user would.
    cancel_after: Option<u64>,
    received: Mutex<Vec<u8>>,
}

impl Fake {
    fn refusal(&self) -> Result<(), FileError> {
        match self.refuse {
            Some((refusal, status)) => Err(FileError::Refused {
                refusal,
                status,
                message: "refused".into(),
            }),
            None => Ok(()),
        }
    }
}

impl FileHost for Fake {
    fn list(&self) -> Result<FileList, FileError> {
        self.refusal()?;
        Ok(FileList {
            folder: "C:\\Users\\me\\Downloads\\Rubylight".into(),
            free_bytes: 1000,
            max_file_bytes: 500,
            files: vec![RemoteFile {
                name: "a.txt".into(),
                size: 3,
                modified_unix_ms: 1,
            }],
        })
    }
    fn upload(
        &self,
        name: &str,
        file: &mut dyn Read,
        length: u64,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<Uploaded, FileError> {
        self.refusal()?;
        let mut done = 0;
        progress(0, length);
        let mut chunk = [0u8; 4];
        while done < length {
            if stop.is_cancelled() || self.cancel_after.is_some_and(|n| done >= n) {
                return Err(FileError::Host(Error::Cancelled));
            }
            let n = file.read(&mut chunk).map_err(FileError::Local)?;
            if n == 0 {
                break;
            }
            self.received.lock().unwrap().extend_from_slice(&chunk[..n]);
            done += n as u64;
            progress(done, length);
        }
        Ok(Uploaded {
            name: format!("{name} (1)"),
            size: done,
        })
    }
    fn download(
        &self,
        _name: &str,
        _listed: u64,
        sink: &mut dyn Write,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<u64, FileError> {
        self.refusal()?;
        let total = self.content.len() as u64;
        progress(0, total);
        let mut done = 0;
        for chunk in self.content.chunks(4) {
            if stop.is_cancelled() || self.cancel_after.is_some_and(|n| done >= n) {
                return Err(FileError::Host(Error::Cancelled));
            }
            sink.write_all(chunk).map_err(FileError::Local)?;
            done += chunk.len() as u64;
            progress(done, total);
        }
        Ok(done)
    }
}

fn temp(content: &[u8]) -> File {
    let path = std::env::temp_dir().join(format!(
        "nightjar-files-{}-{}",
        std::process::id(),
        NEXT_HANDLE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    file.write_all(content).unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    // The open file outlives its name, so nothing is left in the temp folder.
    let _ = std::fs::remove_file(&path);
    file
}

fn parse(reply: &str) -> Value {
    serde_json::from_str(reply).unwrap()
}

fn started(handle: i64) -> Vec<u64> {
    parse(&start_next(handle))
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_u64().unwrap())
        .collect()
}

#[test]
fn an_upload_runs_reports_progress_and_lands_under_the_hosts_name() {
    let handle = create();
    let id = queue_upload(handle, "/storage/emulated/0/DCIM/photo?.jpg", 10) as u64;
    let rows = parse(&snapshot(handle));
    assert_eq!(rows[0]["state"], "queued");
    assert_eq!(rows[0]["name"], "photo_.jpg");
    assert_eq!(rows[0]["local_name"], "/storage/emulated/0/DCIM/photo?.jpg");

    let starts = parse(&start_next(handle));
    assert_eq!(starts[0]["direction"], "upload");
    assert_eq!(starts[0]["name"], "photo_.jpg");
    let host = Fake::default();
    let mut reports = Vec::new();
    let reply = parse(&run(
        handle,
        id,
        Ok(&host),
        Ok(temp(b"0123456789")),
        &mut |done, total| reports.push((done, total)),
    ));
    assert_eq!(reports, [(4, 10), (8, 10), (10, 10)]);
    assert_eq!(host.received.lock().unwrap().as_slice(), b"0123456789");
    let row = &reply["ok"];
    assert_eq!(row["state"], "done");
    assert_eq!(row["saved_name"], "photo_.jpg (1)");
    assert_eq!(
        (row["done"].as_u64(), row["total"].as_u64()),
        (Some(10), Some(10))
    );
    // A transfer runs once per start.
    let again = parse(&run(handle, id, Ok(&host), Ok(temp(b"")), &mut |_, _| {}));
    assert_eq!(again["error"]["problem"], "local");
    destroy(handle);
}

#[test]
fn a_download_writes_the_file_and_learns_its_size() {
    let handle = create();
    let id = queue_download(handle, "notes.txt", "notes.txt", 0) as u64;
    assert_eq!(started(handle), [id]);
    let host = Fake {
        content: b"hello world".to_vec(),
        ..Fake::default()
    };
    let mut file = temp(b"");
    let reply = parse(&run(
        handle,
        id,
        Ok(&host),
        file.try_clone(),
        &mut |_, _| {},
    ));
    assert_eq!(reply["ok"]["state"], "done");
    assert_eq!(reply["ok"]["size"], 11);
    let mut written = String::new();
    file.seek(SeekFrom::Start(0)).unwrap();
    file.read_to_string(&mut written).unwrap();
    assert_eq!(written, "hello world");
    destroy(handle);
}

#[test]
fn refusals_become_problems_and_only_some_can_be_retried() {
    for (refusal, status, problem, console, retry) in [
        (FileRefusal::Forbidden, 403, "no_permission", true, true),
        (
            FileRefusal::FileTransferDisabled,
            403,
            "transfer_off",
            true,
            true,
        ),
        (FileRefusal::TooLarge, 413, "too_large", false, false),
        (FileRefusal::TransferBusy, 409, "busy", false, true),
        (FileRefusal::NotFound, 404, "not_found", false, false),
    ] {
        let handle = create();
        let id = queue_download(handle, "a", "a", 0) as u64;
        started(handle);
        let host = Fake {
            refuse: Some((refusal, status)),
            ..Fake::default()
        };
        let row = &parse(&run(handle, id, Ok(&host), Ok(temp(b"")), &mut |_, _| {}))["ok"];
        assert_eq!(row["state"], "failed", "{refusal:?}");
        assert_eq!(row["problem"], problem, "{refusal:?}");
        assert_eq!(row["console_fix"], console, "{refusal:?}");
        assert_eq!(row["can_retry"], retry, "{refusal:?}");
        assert_eq!(retry, super::retry(handle, id), "{refusal:?}");
        destroy(handle);
    }
}

#[test]
fn no_client_fails_the_transfer_instead_of_dropping_it() {
    let handle = create();
    let id = queue_upload(handle, "a.bin", 4) as u64;
    started(handle);
    let reply = parse(&run::<&Fake>(
        handle,
        id,
        Err(Error::InvalidIdentity("bad".into())),
        Ok(temp(b"abcd")),
        &mut |_, _| {},
    ));
    assert_eq!(reply["ok"]["state"], "failed");
    assert_eq!(reply["ok"]["problem"], "host");
    destroy(handle);
}

#[test]
fn a_file_that_would_not_open_fails_the_transfer() {
    let handle = create();
    let id = queue_download(handle, "a.bin", "a.bin", 0) as u64;
    started(handle);
    let reply = parse(&run(
        handle,
        id,
        Ok(&Fake::default()),
        Err(std::io::Error::other("gone")),
        &mut |_, _| {},
    ));
    assert_eq!(reply["ok"]["state"], "failed");
    assert_eq!(reply["ok"]["problem"], "local");
    assert_eq!(reply["ok"]["can_retry"], true);
    destroy(handle);
}

#[test]
fn a_cancel_while_running_ends_cancelled_and_can_be_retried() {
    let handle = create();
    let id = queue_upload(handle, "big.bin", 12) as u64;
    started(handle);
    assert!(cancel(handle, id));
    let rows = parse(&snapshot(handle));
    assert_eq!(rows[0]["cancelling"], true);
    let host = Fake::default();
    let row = &parse(&run(
        handle,
        id,
        Ok(&host),
        Ok(temp(&[0; 12])),
        &mut |_, _| {},
    ))["ok"];
    assert_eq!(row["state"], "cancelled");
    assert_eq!(row["cancelling"], false);
    assert!(host.received.lock().unwrap().is_empty());
    assert!(retry(handle, id));
    assert_eq!(started(handle), [id]);
    let row = &parse(&run(
        handle,
        id,
        Ok(&host),
        Ok(temp(&[7; 12])),
        &mut |_, _| {},
    ))["ok"];
    assert_eq!(row["state"], "done");
    destroy(handle);
}

#[test]
fn one_upload_and_two_downloads_run_at_a_time() {
    let handle = create();
    let up = [queue_upload(handle, "1", 1), queue_upload(handle, "2", 1)];
    let down = [
        queue_download(handle, "a", "a", 1),
        queue_download(handle, "b", "b", 1),
        queue_download(handle, "c", "c", 1),
    ];
    let first: Vec<i64> = started(handle).into_iter().map(|i| i as i64).collect();
    assert_eq!(first, [up[0], down[0], down[1]]);
    assert!(started(handle).is_empty());
    let host = Fake {
        content: vec![1],
        ..Fake::default()
    };
    run(
        handle,
        down[0] as u64,
        Ok(&host),
        Ok(temp(b"")),
        &mut |_, _| {},
    );
    assert_eq!(started(handle), [down[2] as u64]);
    destroy(handle);
}

#[test]
fn a_destroyed_handle_answers_nothing_and_cancels_its_transfers() {
    let handle = create();
    let id = queue_download(handle, "a", "a", 0) as u64;
    started(handle);
    let kept = entry(handle).unwrap();
    destroy(handle);
    assert!(lock(&kept).transfers.get(id).unwrap().is_cancelling());
    assert_eq!(snapshot(handle), "[]");
    assert_eq!(start_next(handle), "[]");
    assert_eq!(queue_upload(handle, "a", 1), -1);
    assert!(!cancel(handle, id));
    let reply = parse(&run(
        handle,
        id,
        Ok(&Fake::default()),
        Ok(temp(b"")),
        &mut |_, _| {},
    ));
    assert_eq!(reply["error"]["problem"], "local");
}

#[test]
fn a_handle_destroyed_while_its_transfer_runs_cancels_it_and_the_run_still_answers() {
    // The screen closes (main thread) while a worker is between two chunks.
    let handle = create();
    let id = queue_upload(handle, "big.bin", 12) as u64;
    started(handle);
    let host = Fake::default();
    let mut destroyed = false;
    let reply = parse(&run(
        handle,
        id,
        Ok(&host),
        Ok(temp(&[0; 12])),
        &mut |done, _| {
            if done > 0 && !destroyed {
                destroyed = true;
                destroy(handle);
            }
        },
    ));
    assert!(destroyed);
    assert_eq!(reply["ok"]["state"], "cancelled");
    assert_eq!(
        host.received.lock().unwrap().len(),
        4,
        "no chunk after the close"
    );
    assert_eq!(snapshot(handle), "[]");
}

#[test]
fn the_list_and_its_errors_keep_the_status_and_code() {
    let reply = parse(&list(Ok(&Fake::default())));
    assert_eq!(reply["ok"]["files"][0]["name"], "a.txt");
    assert_eq!(reply["ok"]["max_file_bytes"], 500);
    let refused = parse(&list(Ok(&Fake {
        refuse: Some((FileRefusal::Forbidden, 403)),
        ..Fake::default()
    })));
    let error = &refused["error"];
    assert_eq!(error["problem"], "no_permission");
    assert_eq!(
        (error["http_status"].as_u64(), error["code"].as_str()),
        (Some(403), Some("forbidden"))
    );
    let offline = parse(&list::<&Fake>(Err(Error::Io(std::io::Error::other(
        "down",
    )))));
    assert_eq!(offline["error"]["problem"], "network");
    assert!(offline["error"].get("http_status").is_none());
    let cancelled = parse(&file_error_envelope(&FileError::Host(Error::Cancelled)));
    assert_eq!(cancelled["error"]["problem"], "cancelled");
}

impl<T: FileHost> FileHost for &T {
    fn list(&self) -> Result<FileList, FileError> {
        (*self).list()
    }
    fn upload(
        &self,
        name: &str,
        file: &mut dyn Read,
        length: u64,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<Uploaded, FileError> {
        (*self).upload(name, file, length, progress, stop)
    }
    fn download(
        &self,
        name: &str,
        listed: u64,
        sink: &mut dyn Write,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<u64, FileError> {
        (*self).download(name, listed, sink, progress, stop)
    }
}
