use super::*;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A PC with the native API and Rubylight's file transfer, as serverinfo says it.
pub(crate) fn native_answer() -> nightjar_core::http::ServerInfo {
    crate::host_page::tests::info("SUNSHINE_SERVER_FREE", "<RustHostVersion>3.0.0</RustHostVersion><NativeApiVersion>1</NativeApiVersion>")
}

/// The native answer with the file features listed and these permissions.
pub(crate) fn native(features: &str, upload: bool, download: bool) -> NativeCapabilities {
    serde_json::from_str(&format!(
        "{{\"api_version\":1,\"host\":{{\"name\":\"PC\",\"version\":\"3.0.0\",\"unique_id\":\"uid\"}},\"features\":[{features}],\
             \"permissions\":{{\"list\":true,\"view\":true,\"launch\":true,\"host_control\":false,\"file_upload\":{upload},\"file_download\":{download}}}}}"
    ))
    .expect("the answer parses")
}

pub(crate) const BOTH: &str = "\"files.upload\",\"files.download\"";

#[test]
fn a_renamed_transfer_is_titled_in_glyphs_the_font_has() {
    let mut transfers = Transfers::new();
    let down = transfers.queue_download("report.pdf", "report (1).pdf", 1);
    // The PC takes no colon in a name, so the upload is sent under another one.
    let up = transfers.queue_upload("a:b.txt", 1);
    let (down, up) = (title(transfers.get(down).unwrap()), title(transfers.get(up).unwrap()));
    assert_eq!(down, "report.pdf › report (1).pdf");
    assert_eq!(up, "a:b.txt › a_b.txt");
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    // The fonts load with the first frame; its textures are let go like the other tests do.
    ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
    for font in [crate::theme::regular(12.0), crate::theme::semibold(12.0)] {
        assert!(ctx.fonts_mut(|f| f.has_glyphs(&font, &down) && f.has_glyphs(&font, &up)), "{down:?} / {up:?} in {font:?}");
    }
}

/// The PC's side: its folder, what it was sent, and the answers to give.
#[derive(Default)]
pub(crate) struct Fake {
    pub(crate) native: Mutex<Option<NativeCapabilities>>,
    pub(crate) files: Mutex<Vec<(String, Vec<u8>)>>,
    pub(crate) received: Mutex<Vec<(String, Vec<u8>)>>,
    pub(crate) lists: Mutex<usize>,
    /// Transfers then run until they are cancelled.
    pub(crate) hold: Mutex<bool>,
    /// The next transfer is refused with this.
    pub(crate) refuse: Mutex<Option<nightjar_core::http::FileRefusal>>,
}

fn refused(refusal: nightjar_core::http::FileRefusal) -> FileError {
    FileError::Refused { refusal, status: 507, message: format!("host says {}", refusal.code()) }
}

impl Fake {
    fn held(&self, stop: &TransferCancel) -> Result<(), FileError> {
        let until = Instant::now() + Duration::from_secs(10);
        while *self.hold.lock().unwrap() && Instant::now() < until {
            if stop.is_cancelled() {
                return Err(FileError::Host(Error::Cancelled));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    }
}

impl FileHost for Fake {
    fn capabilities(&self) -> Result<NativeCapabilities, Error> {
        self.native.lock().unwrap().clone().ok_or_else(|| Error::Malformed("no answer".into()))
    }

    fn list(&self) -> Result<FileList, FileError> {
        *self.lists.lock().unwrap() += 1;
        let files = self.files.lock().unwrap();
        let files = files.iter().map(|(name, data)| RemoteFile { name: name.clone(), size: data.len() as u64, modified_unix_ms: 0 }).collect();
        Ok(FileList { folder: "C:\\Users\\Me\\Downloads\\Rubylight".into(), free_bytes: 5_000_000, max_file_bytes: 1 << 30, files })
    }

    fn upload(&self, name: &str, file: &mut dyn Read, length: u64, progress: &mut dyn FnMut(u64, u64), stop: &TransferCancel) -> Result<Uploaded, FileError> {
        progress(0, length);
        self.held(stop)?;
        if let Some(refusal) = self.refuse.lock().unwrap().take() {
            return Err(refused(refusal));
        }
        let mut data = Vec::new();
        file.read_to_end(&mut data).map_err(FileError::Local)?;
        progress(data.len() as u64, length);
        let mut files = self.files.lock().unwrap();
        let stored = if files.iter().any(|(n, _)| n == name) { format!("{name} (1)") } else { name.to_owned() };
        files.push((stored.clone(), data.clone()));
        self.received.lock().unwrap().push((name.to_owned(), data));
        Ok(Uploaded { name: stored, size: length })
    }

    fn download(&self, name: &str, listed: u64, dest: &Path, progress: &mut dyn FnMut(u64, u64), stop: &TransferCancel) -> Result<u64, FileError> {
        let data = self.files.lock().unwrap().iter().find(|(n, _)| n == name).map(|(_, d)| d.clone());
        if data.as_ref().is_some_and(|d| d.len() as u64 != listed) {
            return Err(FileError::Host(Error::Malformed(format!("the page asked for {listed} bytes, not the listed size"))));
        }
        nightjar_core::http::transfers::download_into(dest, |sink| {
            let data = data.ok_or_else(|| refused(nightjar_core::http::FileRefusal::NotFound))?;
            let half = data.len() / 2;
            sink.write_all(&data[..half]).map_err(FileError::Local)?;
            progress(half as u64, data.len() as u64);
            self.held(stop)?;
            if let Some(refusal) = self.refuse.lock().unwrap().take() {
                return Err(refused(refusal));
            }
            sink.write_all(&data[half..]).map_err(FileError::Local)?;
            progress(data.len() as u64, data.len() as u64);
            Ok(data.len() as u64)
        })
    }
}

pub(crate) fn fake(upload: bool, download: bool) -> Arc<Fake> {
    let fake = Arc::new(Fake::default());
    *fake.native.lock().unwrap() = Some(native(BOTH, upload, download));
    fake.files.lock().unwrap().push(("report.txt".into(), b"0123456789".to_vec()));
    fake
}

/// A folder of its own under the system's temp folder, removed afterwards.
pub(crate) struct Folder(pub(crate) PathBuf);

impl Folder {
    pub(crate) fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("nightjar-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn file(&self, name: &str, data: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, data).unwrap();
        path
    }

    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.0).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn page(api: Arc<Fake>, folder: &Folder) -> Page {
    Page::new(7, api, native_answer().capabilities, Ok(folder.0.clone()), Arc::new(|| {}))
}

/// A Files page for PC `host` whose upload is held running, for the launcher's tests.
pub(crate) fn sending(host: u64, downloads: &Folder, here: &Folder) -> Page {
    let api = fake(true, true);
    let mut p = Page::new(host, api.clone(), native_answer().capabilities, Ok(downloads.0.clone()), Arc::new(|| {}));
    until(&mut p, "the list", |p| p.list.is_some());
    *api.hold.lock().unwrap() = true;
    p.send(vec![here.file("a.txt", b"a")]);
    assert!(p.transfers.is_active());
    p
}

/// Waits for one report and takes it in: the workers run on threads.
fn wait(page: &mut Page) -> bool {
    match page.events.recv_timeout(Duration::from_secs(10)) {
        Ok(event) => {
            page.handle(event);
            true
        }
        Err(_) => false,
    }
}

/// Takes in reports until `done` holds.
fn until(page: &mut Page, what: &str, done: impl Fn(&Page) -> bool) {
    while !done(page) {
        assert!(wait(page), "nothing more came while waiting for {what}");
    }
}

fn state(page: &Page, id: u64) -> State {
    page.transfers.get(id).expect("listed").state.clone()
}

#[test]
fn the_page_asks_the_pc_then_lists_its_folder() {
    let downloads = Folder::new("asks");
    let api = fake(true, true);
    let mut p = page(api.clone(), &downloads);
    assert_eq!(p.access, Access::Checking);
    assert!(p.busy());
    until(&mut p, "the list", |p| p.list.is_some());
    assert_eq!(p.access, Access::Known { upload: Availability::Supported, download: Availability::Supported });
    let list = p.list.as_ref().unwrap().as_ref().unwrap();
    assert_eq!(list.files[0].name, "report.txt");
    assert!(!p.busy());
    assert!(p.may(Feature::FileUpload) && p.may(Feature::FileDownload));
}

#[test]
fn each_refusal_of_access_says_what_to_change() {
    let downloads = Folder::new("access");
    let known = |native: Option<NativeCapabilities>| {
        let api = Arc::new(Fake::default());
        *api.native.lock().unwrap() = native;
        let mut p = page(api.clone(), &downloads);
        until(&mut p, "the access", |p| p.access != Access::Checking);
        let lists = *api.lists.lock().unwrap();
        (p.access.clone(), lists, p.listing.is_some())
    };
    // Upload only: the folder is read for its free space; downloading is refused.
    let (access, _, listing) = known(Some(native(BOTH, true, false)));
    let Access::Known { upload, download } = access else { panic!("{access:?}") };
    assert!(upload.is_supported() && listing);
    assert_eq!(refusal_text(download, false), Some("This device may not download this PC’s files. On the PC, open the web UI’s Devices page and allow Download files for it."));
    assert_eq!(refusal_text(upload, true), None);
    // Neither permission: nothing is read.
    let (access, lists, listing) = known(Some(native(BOTH, false, false)));
    let Access::Known { upload, .. } = access else { panic!("{access:?}") };
    assert!(refusal_text(upload, true).unwrap().contains("Upload files"));
    assert_eq!((lists, listing), (0, false));
    // File transfer off on the PC: both never.
    let (access, ..) = known(Some(native("", true, true)));
    let Access::Known { upload, download } = access else { panic!("{access:?}") };
    assert!(upload.is_never() && download.is_never());
    // The PC could not be asked: the error as it came.
    let (access, ..) = known(None);
    assert!(matches!(&access, Access::Failed(e) if e.contains("no answer")), "{access:?}");
    assert_eq!(refusal_text(Availability::Unknown, true), Some("The PC is not online."));
    assert_eq!(refusal_text(Availability::Unsupported(Some(Reason::UpdateHost)), true), Some("This PC’s streaming app has no file transfer."));
}

#[test]
fn sent_files_go_one_at_a_time_and_the_folder_is_read_again() {
    let downloads = Folder::new("send");
    let here = Folder::new("send-here");
    let api = fake(true, true);
    let mut p = page(api.clone(), &downloads);
    until(&mut p, "the list", |p| p.list.is_some());
    let a = here.file("a.part", b"aaa");
    let b = here.file("b.txt", b"bbbb");
    p.send(vec![a, b, here.0.join("missing.txt"), here.0.clone()]);
    let ids: Vec<u64> = p.transfers.items().iter().map(|t| t.id).collect();
    assert_eq!(ids.len(), 2, "what is not a file is not queued");
    assert_eq!(p.notices.len(), 2, "{:?}", p.notices);
    assert!(p.notices[1].contains("it is not a file"), "{:?}", p.notices);
    assert!(matches!(state(&p, ids[0]), State::Running { .. }));
    assert_eq!(state(&p, ids[1]), State::Queued, "the host runs one upload per device");
    assert_eq!(p.transfers.get(ids[0]).unwrap().name, "a.part_", "sent under a name the host takes");
    until(&mut p, "both sent", |p| ids.iter().all(|id| state(p, *id).is_finished()) && p.listing.is_none());
    assert_eq!(*api.received.lock().unwrap(), [("a.part_".to_owned(), b"aaa".to_vec()), ("b.txt".to_owned(), b"bbbb".to_vec())]);
    assert_eq!(status(p.transfers.get(ids[1]).unwrap(), None), "Sent");
    let list = p.list.as_ref().unwrap().as_ref().unwrap();
    assert!(list.files.iter().any(|f| f.name == "b.txt"), "the list was read again: {list:?}");
    assert!(*api.lists.lock().unwrap() >= 2);
    p.clear_finished();
    assert!(p.transfers.items().is_empty() && p.sources.is_empty());
}

#[test]
fn a_download_lands_under_a_free_name_and_never_replaces_a_file() {
    let downloads = Folder::new("download");
    std::fs::write(downloads.0.join("report.txt"), b"mine").unwrap();
    let api = fake(true, true);
    let mut p = page(api.clone(), &downloads);
    until(&mut p, "the list", |p| p.list.is_some());
    let file = p.list.as_ref().unwrap().as_ref().unwrap().files[0].clone();
    p.download(&file);
    p.download(&file);
    let ids: Vec<u64> = p.transfers.items().iter().map(|t| t.id).collect();
    let locals: Vec<&str> = p.transfers.items().iter().map(|t| t.local_name.as_str()).collect();
    assert_eq!(locals, ["report (2).txt", "report (3).txt"], "two downloads at a time, each its own name");
    until(&mut p, "both saved", |p| ids.iter().all(|id| state(p, *id).is_finished()));
    assert_eq!(std::fs::read(downloads.0.join("report.txt")).unwrap(), b"mine", "the file already there is kept");
    assert_eq!(std::fs::read(downloads.0.join("report (2).txt")).unwrap(), b"0123456789");
    let saved = status(p.transfers.get(ids[0]).unwrap(), Some(&downloads.0));
    assert!(saved.starts_with("Saved to ") && saved.ends_with("report (2).txt"), "{saved}");
    assert_eq!(downloads.names(), ["report (2).txt", "report (3).txt", "report.txt"]);
}

#[test]
fn a_name_from_the_pc_never_leaves_the_downloads_folder() {
    let downloads = Folder::new("escape");
    let api = fake(true, true);
    api.files.lock().unwrap().push(("..\\..\\evil.bat".into(), b"x".to_vec()));
    let mut p = page(api.clone(), &downloads);
    until(&mut p, "the list", |p| p.list.is_some());
    let file = RemoteFile { name: "..\\..\\evil.bat".into(), size: 1, modified_unix_ms: 0 };
    p.download(&file);
    let id = p.transfers.items()[0].id;
    assert_eq!(p.transfers.items()[0].local_name, "evil.bat");
    until(&mut p, "saved", |p| state(p, id).is_finished());
    assert_eq!(downloads.names(), ["evil.bat"]);
}

#[test]
fn a_cancelled_download_leaves_no_part_file_and_retries_from_the_start() {
    let downloads = Folder::new("cancel");
    let api = fake(true, true);
    let mut p = page(api.clone(), &downloads);
    until(&mut p, "the list", |p| p.list.is_some());
    *api.hold.lock().unwrap() = true;
    let file = p.list.as_ref().unwrap().as_ref().unwrap().files[0].clone();
    p.download(&file);
    let id = p.transfers.items()[0].id;
    until(&mut p, "half of it", |p| matches!(state(p, id), State::Running { done: 5, .. }));
    assert!(status(p.transfers.get(id).unwrap(), None).starts_with("Downloading · 50 %"));
    p.cancel(id);
    assert_eq!(status(p.transfers.get(id).unwrap(), None), "Cancelling…");
    until(&mut p, "cancelled", |p| state(p, id).is_finished());
    assert_eq!(state(&p, id), State::Cancelled);
    assert!(downloads.names().is_empty(), "no partial file: {:?}", downloads.names());
    *api.hold.lock().unwrap() = false;
    p.retry(id);
    until(&mut p, "saved", |p| state(p, id).is_finished());
    assert!(matches!(state(&p, id), State::Done { .. }));
    assert_eq!(std::fs::read(downloads.0.join("report.txt")).unwrap(), b"0123456789");
}

#[test]
fn a_cancelled_download_keeps_its_name_for_its_retry() {
    let downloads = Folder::new("keeps");
    let api = fake(true, true);
    let mut p = page(api.clone(), &downloads);
    until(&mut p, "the list", |p| p.list.is_some());
    *api.hold.lock().unwrap() = true;
    let file = p.list.as_ref().unwrap().as_ref().unwrap().files[0].clone();
    p.download(&file);
    let first = p.transfers.items()[0].id;
    p.cancel(first);
    until(&mut p, "cancelled", |p| state(p, first).is_finished());
    p.download(&file);
    let locals: Vec<&str> = p.transfers.items().iter().map(|t| t.local_name.as_str()).collect();
    assert_eq!(locals, ["report.txt", "report (2).txt"], "a retry of the first never writes beside the second");
    drop(p);
    let until = Instant::now() + Duration::from_secs(10);
    while Arc::strong_count(&api) > 1 && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(downloads.names().is_empty(), "{:?}", downloads.names());
}

#[test]
fn files_is_offered_for_a_paired_online_pc_with_the_native_api() {
    let record = crate::config::HostRecord {
        unique_id: "uid".into(),
        name: "PC".into(),
        address: "192.0.2.7".into(),
        server_certificate: Some("pem".into()),
        ..Default::default()
    };
    let mut host = crate::hosts::Hosts::without_discovery(vec![record], Arc::new(|| {})).list.remove(0);
    host.reach = Reach::Online;
    host.info = Some(native_answer());
    assert!(offers_files(&host));
    host.reach = Reach::Offline;
    assert!(!offers_files(&host), "offline");
    host.reach = Reach::Online;
    host.info = Some(crate::host_page::tests::info("SUNSHINE_SERVER_FREE", ""));
    assert!(!offers_files(&host), "a host without the native API");
    host.info = Some(native_answer());
    host.record.server_certificate = None;
    assert!(!offers_files(&host), "not paired");
}

#[test]
fn a_refused_transfer_says_why_and_a_file_that_wont_open_fails_here() {
    let downloads = Folder::new("refused");
    let here = Folder::new("refused-here");
    let api = fake(true, true);
    let mut p = page(api.clone(), &downloads);
    until(&mut p, "the list", |p| p.list.is_some());
    *api.refuse.lock().unwrap() = Some(nightjar_core::http::FileRefusal::InsufficientStorage);
    p.send(vec![here.file("big.bin", b"big")]);
    let id = p.transfers.items()[0].id;
    until(&mut p, "refused", |p| state(p, id).is_finished());
    let transfer = p.transfers.get(id).unwrap();
    assert_eq!(status(transfer, None), "The PC does not have enough free space.");
    assert!(matches!(&transfer.state, State::Failed { message, .. } if message.contains("507")), "{transfer:?}");
    assert!(transfer.can_retry());
    // The file is gone by the time it is tried again.
    std::fs::remove_file(here.0.join("big.bin")).unwrap();
    p.retry(id);
    until(&mut p, "failed here", |p| state(p, id).is_finished());
    assert!(matches!(state(&p, id), State::Failed { problem: Problem::Local, .. }), "{:?}", state(&p, id));
}

#[test]
fn leaving_the_page_cancels_what_runs() {
    let downloads = Folder::new("leave");
    let here = Folder::new("leave-here");
    let api = fake(true, true);
    let mut files = Files { page: Some(page(api.clone(), &downloads)) };
    let p = files.page.as_mut().unwrap();
    until(p, "the list", |p| p.list.is_some());
    *api.hold.lock().unwrap() = true;
    p.send(vec![here.file("a.txt", b"a")]);
    let cancel = p.transfers.start_next();
    assert!(cancel.is_empty(), "already running");
    files.poll(Some(7));
    assert!(files.page.is_some() && files.busy(), "shown or on the way back: kept");
    files.poll(None);
    assert!(files.page.is_none() && !files.busy());
    // The upload's worker saw the cancel and gave up: nothing reached the PC.
    let until = Instant::now() + Duration::from_secs(10);
    while Arc::strong_count(&api) > 1 && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(Arc::strong_count(&api), 1, "the workers ended");
    assert!(api.received.lock().unwrap().is_empty());
}

#[test]
fn the_file_dialog_queues_what_was_chosen_and_says_when_it_failed() {
    let downloads = Folder::new("dialog");
    let here = Folder::new("dialog-here");
    let mut files = Files { page: Some(page(fake(true, true), &downloads)) };
    let a = here.file("a.txt", b"a");
    let mut opened = 0;
    files.choose(|done| {
        opened += 1;
        done(Ok(vec![a.clone()]));
    });
    files.choose(|_| opened += 1);
    assert_eq!(opened, 1, "one dialog at a time");
    let p = files.page.as_mut().unwrap();
    until(p, "the choice", |p| !p.choosing);
    assert_eq!(p.transfers.items().len(), 1);
    files.choose(|done| done(Err("no portal".into())));
    let p = files.page.as_mut().unwrap();
    until(p, "the failure", |p| !p.choosing);
    assert_eq!(p.notices, ["The file dialog did not open: no portal"]);
    files.choose(|done| done(Ok(Vec::new())));
    let p = files.page.as_mut().unwrap();
    until(p, "nothing chosen", |p| !p.choosing);
    assert_eq!(p.transfers.items().len(), 1, "a closed dialog sends nothing");
}

#[test]
fn names_and_sizes_read_as_file_managers_show_them() {
    let downloads = Folder::new("names");
    std::fs::write(downloads.0.join("a.txt"), b"").unwrap();
    std::fs::write(downloads.0.join("a (2).txt"), b"").unwrap();
    assert_eq!(free_name(&downloads.0, "a.txt", &[]), "a (3).txt");
    assert_eq!(free_name(&downloads.0, "b.txt", &["B.TXT"]), "b (2).txt", "a download on its way takes its name");
    assert_eq!(free_name(&downloads.0, ".profile", &[]), ".profile");
    assert_eq!(free_name(&downloads.0, "notes", &["notes"]), "notes (2)");
    // Another program's download in progress keeps its name too.
    std::fs::write(downloads.0.join("movie.mkv.part"), b"a browser's").unwrap();
    assert_eq!(free_name(&downloads.0, "movie.mkv", &[]), "movie (2).mkv");
    assert_eq!(size(0), "0 B");
    assert_eq!(size(999), "999 B");
    assert_eq!(size(1_500), "1.5 kB");
    assert_eq!(size(5_000_000), "5.0 MB");
    assert_eq!(size(u64::MAX), "18446744.1 TB");
}

#[test]
fn every_problem_has_its_own_words() {
    let problems = [
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
    let texts: std::collections::HashSet<&str> = problems.iter().map(|p| problem_text(*p)).collect();
    assert_eq!(texts.len(), problems.len());
    crate::strings::tests::with_lang(crate::strings::Lang::De, || {
        assert_eq!(tr(problem_text(Problem::NoSpace)), "Auf dem PC ist nicht genug Platz frei.");
    });
}
