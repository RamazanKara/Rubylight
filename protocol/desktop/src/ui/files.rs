//! A PC's Files page (Experimental), the desktop counterpart of Nightjar Android's file screen:
//! send files to the PC's transfer folder, list that folder, download from it, with progress,
//! cancel and the reason for each refusal. The transfer list is nightjar-core's, shared with
//! Android: one upload and two downloads at a time, and a retry starts again from the first byte
//! (the host serves no ranges).
//!
//! The page asks the PC's native API what this device may do, then lists the folder. Each
//! transfer runs on its own worker thread and reports through a channel; the UI thread owns the
//! list, and the workers wake the UI loop. Leaving the page (it is neither shown nor on the way
//! back to) cancels every transfer that has not finished: a download then leaves no partial file
//! behind, an upload no file on the PC. Downloads land in the user's Downloads folder under a
//! name no file there has yet.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use egui::{Align, CornerRadius, Frame, Layout, Margin, Response, RichText, Ui};
use nightjar_core::http::transfers::{partial_path, upload_name, Direction, Problem, Start, State, Transfer, Transfers};
use nightjar_core::http::{
    Availability, ClientIdentity, Error, Feature, FileError, FileList, HostCapabilities, HostClient, NativeCapabilities, Reason,
    RemoteFile, TransferCancel, Uploaded,
};

use super::{primary_button, quiet_button, section, spinner, text_button, Command, Launcher, Screen, View};
use crate::hosts::{client_for, Host, Reach, Waker};
use crate::strings::{en, tr, trf};
use crate::theme::*;

/// Whether a PC's options offer Files: a paired PC that answers with the native API. What this
/// device may do with files is known only once the page asked the PC.
pub fn offers_files(host: &Host) -> bool {
    host.paired() && host.reach == Reach::Online && host.info.as_ref().is_some_and(|i| i.capabilities.has(Feature::NativeApi))
}

/// The calls the page makes; `HostClient` in the app, a fake in tests.
pub trait FileHost: Send + Sync + 'static {
    fn capabilities(&self) -> Result<NativeCapabilities, Error>;
    fn list(&self) -> Result<FileList, FileError>;
    fn upload(&self, name: &str, file: &mut dyn Read, length: u64, progress: &mut dyn FnMut(u64, u64), stop: &TransferCancel) -> Result<Uploaded, FileError>;
    fn download(&self, name: &str, listed: u64, dest: &Path, progress: &mut dyn FnMut(u64, u64), stop: &TransferCancel) -> Result<u64, FileError>;
}

impl FileHost for HostClient {
    fn capabilities(&self) -> Result<NativeCapabilities, Error> {
        self.native_capabilities_with_files()
    }

    fn list(&self) -> Result<FileList, FileError> {
        self.list_files()
    }

    fn upload(&self, name: &str, file: &mut dyn Read, length: u64, progress: &mut dyn FnMut(u64, u64), stop: &TransferCancel) -> Result<Uploaded, FileError> {
        self.upload_file(name, file, length, progress, stop)
    }

    fn download(&self, name: &str, listed: u64, dest: &Path, progress: &mut dyn FnMut(u64, u64), stop: &TransferCancel) -> Result<u64, FileError> {
        self.download_to_path(name, listed, dest, progress, stop)
    }
}

/// What a worker or the file dialog reports to the page.
enum Event {
    Access(Result<NativeCapabilities, Error>),
    /// `ask` numbers the read, so an older answer that lands later is dropped.
    Listed { ask: u64, result: Result<FileList, FileError> },
    Progress { id: u64, done: u64, total: u64 },
    Uploaded { id: u64, result: Result<Uploaded, FileError> },
    Downloaded { id: u64, result: Result<u64, FileError> },
    Chosen(Result<Vec<PathBuf>, String>),
}

/// What the PC allows this device.
#[derive(Clone, Debug, PartialEq)]
pub enum Access {
    Checking,
    Known { upload: Availability, download: Availability },
    /// The PC could not be asked; the error as it came.
    Failed(String),
}

/// One PC's Files page while it is open or on the way back to.
pub struct Page {
    pub host: u64,
    api: Arc<dyn FileHost>,
    /// The PC's serverinfo capabilities, which the native answer completes.
    base: HostCapabilities,
    pub access: Access,
    /// The folder's last read: the list, or the problem and the error as it came.
    pub list: Option<Result<FileList, (Problem, String)>>,
    /// The number of the read in flight.
    listing: Option<u64>,
    asks: u64,
    pub transfers: Transfers,
    /// Each upload's file on this device, for as long as it is listed.
    sources: HashMap<u64, PathBuf>,
    /// Where downloads land, or why there is no such folder.
    pub folder: Result<PathBuf, String>,
    /// Chosen files that were not queued, and dialogs that failed, in the words to show.
    pub notices: Vec<String>,
    choosing: bool,
    sender: Sender<Event>,
    events: Receiver<Event>,
    waker: Waker,
}

impl Drop for Page {
    fn drop(&mut self) {
        // The workers end with `Cancelled` and report to a channel nobody reads.
        self.transfers.cancel_all();
    }
}

impl Page {
    /// A page that starts asking the PC at once.
    pub fn new(host: u64, api: Arc<dyn FileHost>, base: HostCapabilities, folder: Result<PathBuf, String>, waker: Waker) -> Self {
        let (sender, events) = channel();
        let mut page = Self {
            host,
            api,
            base,
            access: Access::Checking,
            list: None,
            listing: None,
            asks: 0,
            transfers: Transfers::new(),
            sources: HashMap::new(),
            folder,
            notices: Vec::new(),
            choosing: false,
            sender,
            events,
            waker,
        };
        page.check_again();
        page
    }

    /// Asks the PC again what this device may do, then reads the folder again.
    pub fn check_again(&mut self) {
        if self.access == Access::Checking && self.asks > 0 {
            return;
        }
        self.asks += 1;
        self.access = Access::Checking;
        let api = self.api.clone();
        if let Err(e) = self.spawn("files access", move || Event::Access(api.capabilities())) {
            self.access = Access::Failed(e);
        }
    }

    /// Whether something runs that the UI should animate.
    pub fn busy(&self) -> bool {
        self.access == Access::Checking || self.listing.is_some() || self.choosing || self.transfers.is_active()
    }

    pub fn may(&self, feature: Feature) -> bool {
        match &self.access {
            Access::Known { upload, download } => if feature == Feature::FileUpload { upload } else { download }.is_supported(),
            _ => false,
        }
    }

    /// Runs `work` on a worker that sends its event and wakes the UI. Err when no thread started.
    fn spawn(&self, name: &str, work: impl FnOnce() -> Event + Send + 'static) -> Result<(), String> {
        let (sender, waker) = (self.sender.clone(), self.waker.clone());
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let _ = sender.send(work());
                waker();
            })
            .map(drop)
            .map_err(|e| e.to_string())
    }

    fn read_folder(&mut self) {
        if self.listing.is_some() {
            return;
        }
        self.asks += 1;
        let ask = self.asks;
        let api = self.api.clone();
        match self.spawn("files list", move || Event::Listed { ask, result: api.list() }) {
            Ok(()) => self.listing = Some(ask),
            Err(e) => self.list = Some(Err((Problem::Local, e))),
        }
    }

    /// Queues files of this device to send. One that is not a readable file says so instead.
    pub fn send(&mut self, paths: Vec<PathBuf>) {
        for path in paths {
            let shown = path.display().to_string();
            match std::fs::metadata(&path) {
                Ok(meta) if meta.is_file() => {
                    let local = path.file_name().map_or_else(|| shown.clone(), |n| n.to_string_lossy().into_owned());
                    let id = self.transfers.queue_upload(&local, meta.len());
                    self.sources.insert(id, path);
                }
                Ok(_) => self.notices.push(trf("{} was not sent: it is not a file.", &[&shown])),
                Err(e) => self.notices.push(trf("{} was not sent: {}", &[&shown, &e])),
            }
        }
        self.pump();
    }

    /// Queues a download of the PC's `file` into the downloads folder.
    pub fn download(&mut self, file: &RemoteFile) {
        let Ok(folder) = &self.folder else { return };
        // A cancelled or failed download keeps its name too: trying it again writes to it.
        let taken: Vec<&str> = self
            .transfers
            .items()
            .iter()
            .filter(|t| t.direction == Direction::Download)
            .map(|t| t.local_name.as_str())
            .collect();
        let local = free_name(folder, &upload_name(&file.name), &taken);
        self.transfers.queue_download(&file.name, &local, file.size);
        self.pump();
    }

    pub fn cancel(&mut self, id: u64) {
        self.transfers.cancel(id);
        self.pump();
    }

    pub fn retry(&mut self, id: u64) {
        self.transfers.retry(id);
        self.pump();
    }

    pub fn clear_finished(&mut self) {
        self.transfers.clear_finished();
        let transfers = &self.transfers;
        self.sources.retain(|id, _| transfers.get(*id).is_some());
    }

    /// Starts what the host's limits leave room for.
    fn pump(&mut self) {
        for start in self.transfers.start_next() {
            let id = start.id;
            let started = match start.direction {
                Direction::Upload => {
                    let source = self.sources.get(&id).cloned();
                    self.spawn("file upload", self.uploader(start, source))
                }
                Direction::Download => match &self.folder {
                    Ok(folder) => {
                        let dest = folder.join(&start.local_name);
                        self.spawn("file download", self.downloader(start, dest))
                    }
                    Err(e) => Err(e.clone()),
                },
            };
            if let Err(e) = started {
                let error = FileError::Local(std::io::Error::other(e));
                match self.transfers.get(id).map(|t| t.direction) {
                    Some(Direction::Upload) => self.transfers.finish_upload(id, Err(error)),
                    _ => self.transfers.finish_download(id, Err(error)),
                }
            }
        }
    }

    fn progress(&self, id: u64) -> impl FnMut(u64, u64) + Send + 'static {
        let (sender, waker) = (self.sender.clone(), self.waker.clone());
        move |done, total| {
            let _ = sender.send(Event::Progress { id, done, total });
            waker();
        }
    }

    fn uploader(&self, start: Start, source: Option<PathBuf>) -> impl FnOnce() -> Event + Send + 'static {
        let (api, mut progress) = (self.api.clone(), self.progress(start.id));
        move || {
            let result = source
                .ok_or_else(|| FileError::Local(std::io::Error::new(std::io::ErrorKind::NotFound, "the file to send is no longer listed")))
                .and_then(|path| std::fs::File::open(path).map_err(FileError::Local))
                .and_then(|mut file| api.upload(&start.name, &mut file, start.size, &mut progress, &start.cancel));
            Event::Uploaded { id: start.id, result }
        }
    }

    fn downloader(&self, start: Start, dest: PathBuf) -> impl FnOnce() -> Event + Send + 'static {
        let (api, mut progress) = (self.api.clone(), self.progress(start.id));
        move || {
            // Its name was free when it was queued; a file that took it since is not replaced.
            let result = if dest.exists() {
                Err(FileError::Local(std::io::Error::new(std::io::ErrorKind::AlreadyExists, format!("{} already exists", dest.display()))))
            } else {
                api.download(&start.name, start.size, &dest, &mut progress, &start.cancel)
            };
            Event::Downloaded { id: start.id, result }
        }
    }

    /// Takes in what the workers reported.
    pub fn poll(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            self.handle(event);
        }
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Access(Ok(native)) => {
                let caps = self.base.with_native(&native);
                let (upload, download) = (caps.availability(Feature::FileUpload), caps.availability(Feature::FileDownload));
                self.access = Access::Known { upload, download };
                // A device that may only send still learns the folder, its free space and limit.
                if upload.is_supported() || download.is_supported() {
                    self.read_folder();
                } else {
                    self.list = None;
                }
            }
            Event::Access(Err(e)) => self.access = Access::Failed(e.to_string()),
            Event::Listed { ask, result } => {
                if self.listing != Some(ask) {
                    return;
                }
                self.listing = None;
                self.list = Some(result.map_err(|e| (Problem::of(&e).unwrap_or(Problem::Host), e.to_string())));
            }
            Event::Progress { id, done, total } => {
                self.transfers.progress(id, done, total);
            }
            Event::Uploaded { id, result } => {
                self.transfers.finish_upload(id, result);
                if self.transfers.uploaded_since(&[id]) {
                    self.read_folder();
                }
                self.pump();
            }
            Event::Downloaded { id, result } => {
                self.transfers.finish_download(id, result);
                self.pump();
            }
            Event::Chosen(chosen) => {
                self.choosing = false;
                match chosen {
                    Ok(paths) => self.send(paths),
                    Err(e) => self.notices.push(trf("The file dialog did not open: {}", &[&e])),
                }
            }
        }
    }
}

/// The Files page of the PC it is open for, if any.
#[derive(Default)]
pub struct Files {
    pub page: Option<Page>,
}

impl Files {
    /// Takes in what the workers reported. `open` is the PC whose page is shown or on the way
    /// back to: leaving the page cancels its transfers and forgets it.
    pub fn poll(&mut self, open: Option<u64>) {
        if self.page.as_ref().is_some_and(|p| open != Some(p.host)) {
            self.page = None;
        }
        if let Some(page) = &mut self.page {
            page.poll();
        }
    }

    pub fn busy(&self) -> bool {
        self.page.as_ref().is_some_and(Page::busy)
    }

    /// Whether a transfer is queued or running, which leaving the page cancels.
    pub fn transferring(&self) -> bool {
        self.page.as_ref().is_some_and(|p| p.transfers.is_active())
    }

    /// Command::ChooseFiles: `pick` shows the system's file dialog and calls back with the files
    /// chosen (none when it was closed), on any thread. One dialog at a time.
    pub fn choose(&mut self, pick: impl FnOnce(Box<dyn FnOnce(Result<Vec<PathBuf>, String>) + Send>)) {
        let Some(page) = &mut self.page else { return };
        if page.choosing {
            return;
        }
        page.choosing = true;
        let (sender, waker) = (page.sender.clone(), page.waker.clone());
        pick(Box::new(move |chosen| {
            let _ = sender.send(Event::Chosen(chosen));
            waker();
        }));
    }
}

/// Command::OpenFiles: opens `host`'s Files page, which asks the PC at once. One that cannot
/// start says why.
pub fn open(launcher: &mut Launcher, identity: Option<&Arc<ClientIdentity>>, host: &Host, waker: Waker) {
    // Still on the way back to: the same page, with its transfers.
    if launcher.files.page.as_ref().is_some_and(|p| p.host == host.id) {
        launcher.open(Screen::Files(host.id));
        return;
    }
    let client = match identity {
        Some(identity) => client_for(identity, &host.record, host.connect_address()),
        None => Err(tr("Nightjar has no client identity yet. Restart Nightjar and try again.").to_owned()),
    };
    match client {
        Ok(client) => {
            let base = host.info.as_ref().map_or(HostCapabilities::UNKNOWN, |i| i.capabilities.clone());
            launcher.files.page = Some(Page::new(host.id, Arc::new(client), base, downloads_folder(), waker));
            launcher.open(Screen::Files(host.id));
        }
        Err(e) => launcher.message(tr("Files could not open"), e),
    }
}

/// The user's Downloads folder.
fn downloads_folder() -> Result<PathBuf, String> {
    directories::UserDirs::new()
        .and_then(|dirs| dirs.download_dir().map(Path::to_path_buf))
        .ok_or_else(|| tr("This system names no Downloads folder, so nothing can be downloaded.").to_owned())
}

/// `name`, or `name (2)`, `name (3)`, ... before its extension: the first that no file in
/// `folder` has, as itself or as its `.part` file, and no download in `taken` is going to.
fn free_name(folder: &Path, name: &str, taken: &[&str]) -> String {
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name, ""),
    };
    let free = |candidate: &str| {
        let path = folder.join(candidate);
        !taken.iter().any(|t| t.eq_ignore_ascii_case(candidate)) && !path.exists() && !partial_path(&path).exists()
    };
    if free(name) {
        return name.to_owned();
    }
    (2..)
        .map(|n| format!("{stem} ({n}){extension}"))
        .find(|candidate| free(candidate))
        .expect("some number is free")
}

/// A size in the units file managers show (1000-based, like Android's).
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// What a failed transfer or folder read means for the user.
pub fn problem_text(problem: Problem) -> &'static str {
    match problem {
        Problem::NoPermission => en("This device lacks the permission; allow it in the PC’s web UI, under Devices."),
        Problem::TransferOff => en("File transfer is off on the PC; turn it on in its web UI."),
        Problem::InvalidName => en("The PC refused the file name."),
        Problem::Interrupted => en("The upload stopped before its end."),
        Problem::TooLarge => en("The file is larger than the PC allows."),
        Problem::NoSpace => en("The PC does not have enough free space."),
        Problem::NoConsoleUser => en("Nobody is signed in on the PC."),
        Problem::Busy => en("The PC is busy with other transfers; try again shortly."),
        Problem::NotFound => en("The file is no longer on the PC."),
        Problem::Network => en("The connection to the PC failed."),
        Problem::Local => en("The file on this device could not be read or written."),
        Problem::Host => en("The PC answered with an error."),
    }
}

/// Why this device cannot send (`upload`) or download, or None when it can.
pub fn refusal_text(availability: Availability, upload: bool) -> Option<&'static str> {
    match availability {
        Availability::Supported => None,
        Availability::Unsupported(Some(Reason::FilePermission)) if upload => {
            Some(en("This device may not send files to this PC. On the PC, open the web UI’s Devices page and allow Upload files for it."))
        }
        Availability::Unsupported(Some(Reason::FilePermission)) => {
            Some(en("This device may not download this PC’s files. On the PC, open the web UI’s Devices page and allow Download files for it."))
        }
        Availability::Unsupported(None) if upload => Some(en("This PC takes no files.")),
        Availability::Unsupported(None) => Some(en("This PC offers no files to download.")),
        Availability::Unsupported(Some(_)) => Some(en("This PC’s streaming app has no file transfer.")),
        Availability::Unknown => Some(en("The PC is not online.")),
    }
}

/// Where a transfer stands, as its row says it.
pub fn status(transfer: &Transfer, folder: Option<&Path>) -> String {
    let upload = transfer.direction == Direction::Upload;
    match &transfer.state {
        State::Queued => tr("Waiting").to_owned(),
        State::Running { .. } if transfer.is_cancelling() => tr("Cancelling…").to_owned(),
        State::Running { done, total } if *total > 0 => {
            let percent = (transfer.fraction().unwrap_or(0.0) * 100.0) as u32;
            let template = if upload { en("Sending · {0} % ({1} of {2})") } else { en("Downloading · {0} % ({1} of {2})") };
            trf(template, &[&percent, &size(*done), &size(*total)])
        }
        State::Running { done, .. } => trf(if upload { en("Sending · {}") } else { en("Downloading · {}") }, &[&size(*done)]),
        State::Done { name, .. } if upload && *name == transfer.name => tr("Sent").to_owned(),
        State::Done { name, .. } if upload => trf("Sent as {}", &[name]),
        State::Done { name, .. } => match folder {
            Some(folder) => trf("Saved to {}", &[&folder.join(name).display()]),
            None => tr("Saved").to_owned(),
        },
        State::Failed { problem, .. } => tr(problem_text(*problem)).to_owned(),
        State::Cancelled => tr("Cancelled").to_owned(),
    }
}

/// A transfer's name: both when the name on the PC differs from the one here.
fn title(transfer: &Transfer) -> String {
    match transfer.direction {
        _ if transfer.name == transfer.local_name => transfer.name.clone(),
        // Figtree has no arrow (U+2192 draws as a box); it has the angle quote.
        Direction::Upload => format!("{} › {}", transfer.local_name, transfer.name),
        Direction::Download => format!("{} › {}", transfer.name, transfer.local_name),
    }
}

/// A line of the page: a name and what to say about it on the left, `control` on the right.
fn item(ui: &mut Ui, name: &str, line: &str, color: egui::Color32, control: impl FnOnce(&mut Ui) -> Option<Response>) -> Option<Response> {
    let mut out = None;
    Frame::new()
        .fill(*SURFACE)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(18, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let control_width = 180.0;
                ui.vertical(|ui| {
                    ui.set_width((ui.available_width() - control_width - 16.0).max(200.0));
                    ui.label(RichText::new(name).font(semibold(text::BODY)).color(ON_SURFACE));
                    ui.label(RichText::new(line).font(regular(text::CAPTION)).color(color));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| out = control(ui));
            });
        });
    ui.add_space(6.0);
    out
}

impl Launcher {
    pub(super) fn files_page(&mut self, ui: &mut Ui, view: &View<'_>, id: u64, commands: &mut Vec<Command>) {
        // The page is taken out while it is drawn, so focus can be handed out on `self`.
        let page = self.files.page.take().filter(|p| p.host == id);
        let (Some(mut page), Some(_)) = (page, view.hosts.get(id)) else {
            // The PC was removed: its transfers end with the page.
            let root = self.root();
            self.pop(root);
            return;
        };
        self.draw_files(ui, &mut page, commands);
        self.files.page = Some(page);
    }

    fn draw_files(&mut self, ui: &mut Ui, page: &mut Page, commands: &mut Vec<Command>) {
        let folder = page.folder.as_ref().ok().cloned();
        let may_send = page.may(Feature::FileUpload);
        let may_download = page.may(Feature::FileDownload) && folder.is_some();
        ui.label(RichText::new(tr("Leaving this page cancels the transfers that have not finished. A transfer tried again starts from its first byte.")).color(ON_SURFACE_VARIANT));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let send = ui.add_enabled_ui(may_send && !page.choosing, |ui| primary_button(ui, "Send files…")).inner;
            self.focus_item(&send, "files-send", true);
            if send.clicked() {
                commands.push(Command::ChooseFiles);
            }
            let again = ui.add_enabled_ui(page.access != Access::Checking, |ui| quiet_button(ui, "Check again")).inner;
            self.focus_item(&again, "files-check", !may_send);
            if again.clicked() {
                page.check_again();
            }
        });
        match &page.access {
            Access::Checking => {
                ui.horizontal(|ui| {
                    spinner(ui, 10.0);
                    ui.label(RichText::new(tr("Asking the PC…")).color(ON_SURFACE_VARIANT));
                });
            }
            Access::Failed(e) => {
                ui.label(RichText::new(trf("Could not ask the PC: {}", &[e])).color(ERROR));
            }
            Access::Known { upload, download } => {
                let both_off = upload.is_never() && download.is_never();
                let reasons = if both_off {
                    vec![en("File transfer is off on this PC. Turn it on in the PC’s web UI, under File transfer.")]
                } else {
                    [refusal_text(*upload, true), refusal_text(*download, false)].into_iter().flatten().collect()
                };
                for reason in reasons {
                    ui.label(RichText::new(tr(reason)).color(ERROR));
                }
            }
        }
        if let Err(why) = &page.folder {
            ui.label(RichText::new(why).color(ERROR));
        }
        for notice in &page.notices {
            ui.label(RichText::new(notice).color(ERROR));
        }
        if !page.notices.is_empty() && text_button(ui, "Dismiss").clicked() {
            page.notices.clear();
        }

        if !page.transfers.items().is_empty() {
            section(ui, "Transfers");
            let mut act: Option<(u64, bool)> = None;
            for transfer in page.transfers.items() {
                let failed = matches!(transfer.state, State::Failed { .. });
                let line = status(transfer, folder.as_deref());
                let id = transfer.id;
                let button = item(ui, &title(transfer), &line, if failed { *ERROR } else { *ON_SURFACE_VARIANT }, |ui| {
                    if !transfer.state.is_finished() && !transfer.is_cancelling() {
                        Some(quiet_button(ui, "Cancel"))
                    } else if transfer.can_retry() {
                        Some(quiet_button(ui, "Try again"))
                    } else {
                        None
                    }
                });
                if let State::Failed { message, .. } = &transfer.state {
                    ui.label(RichText::new(message).font(regular(text::CAPTION)).color(OUTLINE));
                }
                if let Some(fraction) = transfer.fraction().filter(|_| matches!(transfer.state, State::Running { .. })) {
                    ui.add(egui::ProgressBar::new(fraction as f32).desired_height(4.0));
                }
                if let Some(button) = button {
                    self.focus_item(&button, &format!("transfer-{id}"), false);
                    if button.clicked() {
                        act = Some((id, transfer.can_retry()));
                    }
                }
            }
            match act {
                Some((id, true)) => page.retry(id),
                Some((id, false)) => page.cancel(id),
                None => {}
            }
            if page.transfers.items().iter().any(|t| t.state.is_finished()) {
                let clear = quiet_button(ui, "Clear finished");
                self.focus_item(&clear, "files-clear", false);
                if clear.clicked() {
                    page.clear_finished();
                }
            }
        }

        section(ui, "On the PC");
        if page.listing.is_some() {
            ui.horizontal(|ui| {
                spinner(ui, 10.0);
                ui.label(RichText::new(tr("Reading the PC’s folder…")).color(ON_SURFACE_VARIANT));
            });
        }
        let mut fetch = None;
        match &page.list {
            None => {}
            Some(Err((problem, message))) => {
                ui.label(RichText::new(tr(problem_text(*problem))).color(ERROR));
                ui.label(RichText::new(message).font(regular(text::CAPTION)).color(OUTLINE));
            }
            Some(Ok(list)) => {
                ui.label(RichText::new(trf("{0} · {1} free", &[&list.folder, &size(list.free_bytes)])).color(ON_SURFACE_VARIANT));
                ui.add_space(4.0);
                if list.files.is_empty() && page.may(Feature::FileDownload) {
                    ui.label(RichText::new(tr("No files in the PC’s folder yet.")).color(ON_SURFACE_VARIANT));
                }
                for file in &list.files {
                    let button = item(ui, &file.name, &size(file.size), *ON_SURFACE_VARIANT, |ui| {
                        Some(ui.add_enabled_ui(may_download, |ui| quiet_button(ui, "Download")).inner)
                    });
                    if let Some(button) = button {
                        self.focus_item(&button, &format!("file-{}", file.name), false);
                        if button.clicked() {
                            fetch = Some(file.clone());
                        }
                    }
                }
            }
        }
        if let Some(file) = fetch {
            page.download(&file);
        }
        ui.add_space(12.0);
    }
}

#[cfg(test)]
pub(crate) mod tests;
