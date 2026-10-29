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

use super::{primary_button, quiet_button, scroll, section, spinner, text_button, Command, Launcher, Screen, View};
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
        Some(identity) => client_for(identity, &host.record, &host.record.address),
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
        Problem::NoPermission => en("This device lacks the permission; allow it in the PC's web UI, under Devices."),
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
            Some(en("This device may not send files to this PC. On the PC, open the web UI's Devices page and allow Upload files for it."))
        }
        Availability::Unsupported(Some(Reason::FilePermission)) => {
            Some(en("This device may not download this PC's files. On the PC, open the web UI's Devices page and allow Download files for it."))
        }
        Availability::Unsupported(None) if upload => Some(en("This PC takes no files.")),
        Availability::Unsupported(None) => Some(en("This PC offers no files to download.")),
        Availability::Unsupported(Some(_)) => Some(en("This PC's host has no file transfer.")),
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
        scroll(ui, "files", |ui| {
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
                        vec![en("File transfer is off on this PC. Turn it on in the PC's web UI, under File transfer.")]
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
                    ui.label(RichText::new(tr("Reading the PC's folder…")).color(ON_SURFACE_VARIANT));
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
                        ui.label(RichText::new(tr("No files in the PC's folder yet.")).color(ON_SURFACE_VARIANT));
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
        });
    }
}

#[cfg(test)]
pub(crate) mod tests {
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
        assert_eq!(refusal_text(download, false), Some("This device may not download this PC's files. On the PC, open the web UI's Devices page and allow Download files for it."));
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
        assert_eq!(refusal_text(Availability::Unsupported(Some(Reason::UpdateHost)), true), Some("This PC's host has no file transfer."));
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
}
