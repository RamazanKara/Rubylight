//! The update check and its notice, ported from Moonlight V+ PC's AutoUpdateChecker
//! (qiin2333/moonlight-qt, `app/backend/autoupdatechecker.cpp`) and pointed at
//! RamazanKara/nightjar: once per launch, when the setting is on and the PC is online, it asks
//! GitHub for the latest release and, when that is a stable version newer than this one, the
//! launcher says so. A copy the setup installed and an AppImage also get Update now, which starts
//! the download only when clicked (self_update.rs does the work); a portable Windows copy keeps the
//! link to the release page. A deb, rpm, Flatpak or archive gets updates from elsewhere, so it
//! doesn't check.
//!
//! The launcher holds the update back for a stream: a stream starting cancels a running download
//! and nothing is handed over until the launcher is back.
//!
//! Failures of the background check stay out of the UI (anonymous GitHub requests are rate limited,
//! as Android's UpdateChecker notes) and go to the log. Failures after Update now show in the notice.
//!
//! Settings → Check for updates now asks once more whenever it is chosen, setting on or off, as
//! Android's `UpdateFlow.checkNow`: its answer is a toast, or the notice when there is an update.
//! Nothing is asked without the setting or that choice.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

use crate::self_update::{self, Asset, Attempt, CopyKind, Failure, Progress, Steps};
use crate::strings::{tr, trf};
use crate::update_net::{Native, Transport};

const LATEST: &str = "https://api.github.com/repos/RamazanKara/nightjar/releases/latest";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub url: String,
    /// What Update now downloads, or why it refuses to.
    pub asset: Result<Asset, Failure>,
}

/// A version's numbers and its pre-release part ("rc.1" in 4.0.0-rc.1), from a tag or a
/// versionName; a leading `v` is allowed.
fn parse(text: &str) -> Option<([u64; 3], Option<&str>)> {
    let text = text.trim().strip_prefix('v').unwrap_or(text.trim());
    let (core, pre) = match text.split_once('-') { Some((core, pre)) => (core, Some(pre)), None => (text, None) };
    let core = core.split('+').next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let numbers = [parts.next()??, parts.next()??, parts.next().unwrap_or(Some(0))?];
    parts.next().is_none().then_some((numbers, pre.filter(|p| !p.is_empty())))
}

/// Whether `tag` is a stable release newer than `current`. A pre-release build is older than its
/// own final version, as in SemVer. A tag is digits and dots only: it becomes a file name and an
/// address, and `v9.9.9+a/b` would read as 9.9.9.
pub fn newer_stable(tag: &str, current: &str) -> bool {
    if !tag.strip_prefix('v').unwrap_or(tag).bytes().all(|b| b.is_ascii_digit() || b == b'.') { return false; }
    let (Some((latest, None)), Some((running, pre))) = (parse(tag), parse(current)) else { return false };
    latest > running || (latest == running && pre.is_some())
}

/// The release GitHub answered, if it is an update for `current`. Drafts and pre-releases never
/// are (GitHub's latest release excludes them; the flags are checked as V+ PC does).
pub fn offer(json: &str, current: &str, copy: &CopyKind) -> Result<Option<Release>, String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("unreadable release: {e}"))?;
    let tag = value["tag_name"].as_str().ok_or("the release has no tag")?;
    if value["draft"].as_bool() == Some(true) || value["prerelease"].as_bool() == Some(true) || !newer_stable(tag, current) {
        return Ok(None);
    }
    let url = value["html_url"].as_str().filter(|u| u.starts_with("https://github.com/")).ok_or("the release has no page")?;
    let version = tag.trim_start_matches('v');
    let asset = copy.asset_name(version).ok_or(Failure::NoInstaller).and_then(|name| self_update::asset(&value, tag, &name));
    Ok(Some(Release { version: version.to_owned(), url: url.to_owned(), asset }))
}

/// A download on its way.
struct Job {
    progress: Arc<Progress>,
    size: u64,
    done: Receiver<Result<std::path::PathBuf, Failure>>,
}

/// What Check for updates now found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// A newer release: the notice shows it.
    Found,
    UpToDate,
    Failed,
}

impl Answer {
    /// Its toast, Android's `update_up_to_date` and `update_check_failed`; none for an update.
    pub fn toast(self) -> Option<&'static str> {
        match self {
            Self::Found => None,
            Self::UpToDate => Some(tr("You have the latest version")),
            Self::Failed => Some(tr("Couldn’t check for updates")),
        }
    }
}

/// What a check needs, kept for Check for updates now.
struct Asker {
    net: Arc<dyn Transport>,
    copy: CopyKind,
    current: &'static str,
    online: fn() -> bool,
}

/// The launch's one check, what it found and where the update stands.
#[derive(Default)]
pub struct Updates {
    pending: Option<Receiver<Release>>,
    /// None for a copy that doesn't check (a package manager updates it).
    asker: Option<Asker>,
    /// Check for updates now, waiting for GitHub.
    asked: Option<Receiver<Result<Option<Release>, String>>>,
    answer: Option<Answer>,
    found: Option<Release>,
    /// None for a copy that keeps the link.
    steps: Option<Steps>,
    wake: Option<crate::hosts::Waker>,
    job: Option<Job>,
    failure: Option<Failure>,
    held: bool,
    /// Held while `held`: tells another Nightjar's update, and the setup, about this stream.
    stream_mark: Option<self_update::Mark>,
}

impl Updates {
    /// Starts the check on its own thread; `wake` redraws the launcher when it has an answer. With
    /// the setting off the thread only clears what an earlier update left.
    pub fn start(enabled: bool, wake: crate::hosts::Waker) -> Self {
        let copy = self_update::current();
        if !copy.checks() { return Self::default(); }
        let agent = format!("Nightjar/{} ({})", crate::VERSION, if cfg!(windows) { "Windows" } else { "Linux" });
        let net: Arc<dyn Transport> = Arc::new(Native::new(agent));
        let steps = Steps::for_copy(copy, net.clone());
        Self::begin(enabled, copy.clone(), net, steps, crate::VERSION, online, wake)
    }

    fn begin(enabled: bool, copy: CopyKind, net: Arc<dyn Transport>, steps: Option<Steps>, current: &'static str, online: fn() -> bool, wake: crate::hosts::Waker) -> Self {
        let (mut found, mut failure) = (None, None);
        if let Some(attempt) = steps.as_ref().filter(|_| enabled).and_then(|s| s.marker.as_deref()).and_then(|marker| failed_attempt(marker, current)) {
            eprintln!("nightjar: update: Nightjar {} did not install", attempt.version);
            found = Some(Release { version: attempt.version, url: attempt.url, asset: Err(Failure::NotInstalled) });
            failure = Some(Failure::NotInstalled);
        }
        let leftovers = steps.as_ref().map(|s| (s.dir.clone(), s.own_dir));
        let asker = Asker { net: net.clone(), copy: copy.clone(), current, online };
        let (send, receive) = channel();
        let waker = wake.clone();
        let spawned = std::thread::Builder::new().name("update-check".into()).spawn(move || {
            if let Some((dir, own_dir)) = leftovers { self_update::remove_stale(&dir, own_dir); }
            if !enabled { return; }
            match check(&*net, &copy, current, online) {
                Ok(Some(release)) => {
                    eprintln!("nightjar: update check: Nightjar {} is available", release.version);
                    if let Err(refused) = &release.asset { eprintln!("nightjar: update check: no Update now: {refused:?}"); }
                    let _ = send.send(release);
                    waker();
                }
                Ok(None) => eprintln!("nightjar: update check: up to date"),
                Err(e) => eprintln!("nightjar: update check: {e}"),
            }
        });
        if let Err(e) = spawned { eprintln!("nightjar: update check: no thread: {e}"); }
        // Off, the copy still answers Check for updates now, and an update it finds installs.
        if !enabled { return Self { asker: Some(asker), steps, wake: Some(wake), ..Self::default() }; }
        Self { pending: Some(receive), asker: Some(asker), found, steps, wake: Some(wake), failure, ..Self::default() }
    }

    /// Check for updates now: one check, asked for by the user. False when this copy doesn't check
    /// or a check or download is already running; its answer comes from [`Self::answer`].
    pub fn check_now(&mut self) -> bool {
        if self.asked.is_some() || self.job.is_some() { return false; }
        let Some(asker) = &self.asker else { return false };
        let (net, copy, current, online) = (asker.net.clone(), asker.copy.clone(), asker.current, asker.online);
        let wake = self.wake.clone();
        let (send, receive) = channel();
        let spawned = std::thread::Builder::new().name("update-check-now".into()).spawn(move || {
            let _ = send.send(check(&*net, &copy, current, online));
            if let Some(wake) = wake { wake(); }
        });
        match spawned {
            Ok(_) => self.asked = Some(receive),
            Err(e) => {
                eprintln!("nightjar: update check: no thread: {e}");
                self.answer = Some(Answer::Failed);
            }
        }
        true
    }

    /// Whether Check for updates now is waiting for GitHub.
    #[cfg(test)]
    pub fn checking(&self) -> bool {
        self.asked.is_some()
    }

    /// Check for updates now's answer, once.
    pub fn answer(&mut self) -> Option<Answer> {
        self.answer.take()
    }

    /// Whether a stream is running or starting. Called on every pass of the main loop, so a stream
    /// never shares the network with a download or loses Nightjar to an installer.
    pub fn hold(&mut self, busy: bool) {
        if busy != self.held {
            self.stream_mark = busy.then(|| self_update::Mark::new(self_update::STREAM_MUTEX)).and_then(|mark| mark.map_err(|e| eprintln!("nightjar: update: {e}")).ok());
        }
        self.held = busy;
        if busy && self.job.is_some() {
            eprintln!("nightjar: update: a stream is starting, the download stops");
            self.cancel();
        }
    }

    /// Update now was clicked.
    fn update_now(&mut self) {
        if self.held || self.job.is_some() { return; }
        let (Some(steps), Some(release)) = (&self.steps, &self.found) else { return };
        if (steps.stream_elsewhere)() {
            eprintln!("nightjar: update: refused: another Nightjar is streaming");
            self.failure = Some(Failure::StreamElsewhere);
            return;
        }
        let asset = match &release.asset {
            Ok(asset) => asset.clone(),
            Err(refused) => {
                eprintln!("nightjar: update: refused: {refused:?}");
                self.failure = Some(refused.clone());
                return;
            }
        };
        let wake = self.wake.clone().unwrap_or_else(|| Arc::new(|| {}));
        let progress = Arc::new(Progress { cancel: AtomicBool::new(false), received: AtomicU64::new(0), wake: wake.clone() });
        let (send, done) = channel();
        let (net, allow, part, shared, size) = (steps.net.clone(), steps.allow, steps.part(), progress.clone(), asset.size);
        let spawned = std::thread::Builder::new().name("update-download".into()).spawn(move || {
            let result = self_update::download(&*net, allow, &asset, &part, &shared).map(|()| part);
            // Nobody waits any more: Cancel arrived after the last check. cancel() sets the flag
            // before it looks in the channel, so one of the two always sees the file.
            let part = result.as_ref().ok().cloned();
            let unwanted = send.send(result).is_err() || shared.cancel.load(Ordering::SeqCst);
            if let (true, Some(part)) = (unwanted, part) { let _ = std::fs::remove_file(part); }
            wake();
        });
        match spawned {
            Ok(_) => {
                self.failure = None;
                self.job = Some(Job { progress, size, done });
            }
            Err(e) => self.failure = Some(Failure::Network(e.to_string())),
        }
    }

    /// Cancel was clicked, or a stream is starting: the thread drops its file at its next chunk.
    fn cancel(&mut self) {
        let Some(job) = self.job.take() else { return };
        job.progress.cancel.store(true, Ordering::SeqCst);
        // The download may have finished just before: its file has no other owner now.
        if let Ok(Ok(part)) = job.done.try_recv() {
            if let Err(e) = std::fs::remove_file(&part) { eprintln!("nightjar: update: leftover {}: {e}", part.display()); }
        }
    }

    /// Takes in what the threads finished, once per pass of the main loop. A verified download that
    /// nothing stands in the way of comes back: the caller saves what the next Nightjar reads, then
    /// gives it to [`Self::hand_over`].
    pub fn step(&mut self) -> Option<PathBuf> {
        if let Some(release) = self.pending.as_ref().and_then(|r| r.try_recv().ok()) {
            self.pending = None;
            self.found = Some(release);
        }
        if let Some(answer) = self.asked.as_ref().and_then(|r| match r.try_recv() {
            Ok(answer) => Some(answer),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("the check stopped".into())),
        }) {
            self.asked = None;
            self.answer = Some(match answer {
                Ok(Some(release)) => {
                    eprintln!("nightjar: update check now: Nightjar {} is available", release.version);
                    // A notice dismissed earlier shows again; a download in progress keeps its own.
                    if self.job.is_none() {
                        self.failure = None;
                        self.found = Some(release);
                    }
                    Answer::Found
                }
                Ok(None) => {
                    eprintln!("nightjar: update check now: up to date");
                    Answer::UpToDate
                }
                Err(e) => {
                    eprintln!("nightjar: update check now: {e}");
                    Answer::Failed
                }
            });
        }
        let job = self.job.as_ref()?;
        let result = match job.done.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err(Failure::Network("the download stopped".into())),
        };
        self.job = None;
        let ready = result.and_then(|part| self.refusal(&part).map(|()| part));
        self.settle(ready)
    }

    /// True when Nightjar should exit now for the update: the installer is running or the new
    /// AppImage has started.
    pub fn hand_over(&mut self, part: &Path) -> bool {
        let result = self.apply(part);
        self.settle(result).is_some()
    }

    fn settle<T>(&mut self, result: Result<T, Failure>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(Failure::Cancelled) => None,
            Err(failure) => {
                eprintln!("nightjar: update: {failure:?}");
                self.failure = Some(failure);
                None
            }
        }
    }

    /// What stops the update now that the download is here. The file goes with it.
    fn refusal(&self, part: &Path) -> Result<(), Failure> {
        let Some(steps) = &self.steps else { return Err(Failure::Cancelled) };
        let refused = if (steps.stream_elsewhere)() {
            Failure::StreamElsewhere
        } else if (steps.setup_running)() {
            // A second setup would stop at once and start Nightjar, on files the first is replacing.
            Failure::Apply("another Nightjar is installing an update".into())
        } else {
            return Ok(());
        };
        if let Err(e) = std::fs::remove_file(part) { eprintln!("nightjar: update: leftover {}: {e}", part.display()); }
        Err(refused)
    }

    fn apply(&self, part: &Path) -> Result<(), Failure> {
        let (Some(steps), Some(release)) = (&self.steps, &self.found) else { return Err(Failure::Cancelled) };
        let Ok(asset) = &release.asset else { return Err(Failure::NoInstaller) };
        let marker = steps.marker.as_deref();
        if let Some(marker) = marker {
            let attempt = Attempt { version: release.version.clone(), url: release.url.clone() };
            if let Err(e) = self_update::write_attempt(marker, &attempt) { eprintln!("nightjar: update: {}: {e}", marker.display()); }
        }
        let result = (steps.apply)(part, asset);
        if result.is_err() {
            if let Some(marker) = marker { let _ = std::fs::remove_file(marker); }
        }
        result
    }

    /// The notice over the launcher until it is opened or dismissed.
    pub fn show(&mut self, ctx: &egui::Context) {
        let Some(release) = &self.found else { return };
        let (mut update, mut stop, mut close) = (false, false, false);
        egui::Area::new(egui::Id::unique("update-notice"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-24.0, -24.0))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgba_unmultiplied(0x28, 0x2A, 0x36, 245))
                    .corner_radius(egui::CornerRadius::same(14))
                    .inner_margin(egui::Margin::symmetric(16, 12))
                    .show(ui, |ui| {
                        ui.set_max_width(360.0);
                        if let Some(job) = &self.job {
                            ui.label(trf("Downloading Nightjar {}", &[&release.version]));
                            let received = job.progress.received.load(Ordering::Relaxed);
                            ui.add(egui::ProgressBar::new(received as f32 / job.size.max(1) as f32).desired_width(280.0).show_percentage());
                            stop = ui.button(tr("Cancel")).clicked();
                            return;
                        }
                        ui.label(trf("Nightjar {} is available.", &[&release.version]));
                        if let Some(failure) = &self.failure {
                            ui.colored_label(ui.visuals().error_fg_color, failure_text(failure, &release.version));
                        }
                        ui.horizontal(|ui| {
                            let refused = self.failure.as_ref().is_some_and(Failure::is_final);
                            if self.steps.is_some() && !refused {
                                update = ui.add_enabled(!self.held, egui::Button::new(tr("Update now"))).clicked();
                            }
                            if ui.button(tr("Open download page")).clicked() {
                                if let Err(e) = crate::sdl::open_url(&release.url) { eprintln!("nightjar: update page: {e}"); }
                                close = true;
                            }
                            close |= ui.button(tr("Not now")).clicked();
                        });
                    });
            });
        if update { self.update_now(); }
        if stop { self.cancel(); }
        if close {
            self.found = None;
            self.failure = None;
        }
    }
}

fn failure_text(failure: &Failure, version: &str) -> String {
    match failure {
        Failure::NoInstaller => tr("This release has no installer for this copy of Nightjar.").to_owned(),
        Failure::NoDigest => tr("This release has no checksum, so it was not installed.").to_owned(),
        Failure::Digest => tr("The download failed its checksum, so it was not installed.").to_owned(),
        Failure::Size => tr("The download has the wrong size, so it was not installed.").to_owned(),
        Failure::Address(_) => tr("The download came from an unexpected address, so it was not installed.").to_owned(),
        Failure::Network(why) => trf("The download failed: {}", &[why]),
        Failure::Apply(why) => trf("The update could not start: {}", &[why]),
        Failure::Restart(why) => trf("The new version is installed, but could not start: {}", &[why]),
        Failure::NotInstalled => trf("Nightjar {} did not install.", &[&version]),
        Failure::StreamElsewhere => tr("Another Nightjar window is streaming, so nothing was installed.").to_owned(),
        Failure::Cancelled => String::new(),
    }
}

/// What the last run's update left, when this run isn't that version or newer.
fn failed_attempt(marker: &Path, current: &str) -> Option<Attempt> {
    self_update::take_attempt(marker).filter(|attempt| newer_stable(&attempt.version, current))
}

fn check(net: &dyn Transport, copy: &CopyKind, current: &str, online: fn() -> bool) -> Result<Option<Release>, String> {
    if !online() { return Err("skipped, offline".into()); }
    let response = self_update::open(net, LATEST, self_update::api_host, true).map_err(|e| format!("{e:?}"))?;
    let status = response.status;
    let mut body = Vec::new();
    response.body.take(4 << 20).read_to_end(&mut body).map_err(|e| format!("read: {e}"))?;
    match status {
        200 => offer(std::str::from_utf8(&body).map_err(|_| "the answer is not UTF-8")?, current, copy),
        403 | 429 => Err(format!("GitHub rate limit (HTTP {status})")),
        _ => Err(format!("GitHub answered HTTP {status}")),
    }
}

/// Windows says the internet is reachable (not only a local network).
#[cfg(windows)]
fn online() -> bool {
    use windows::Networking::Connectivity::{NetworkConnectivityLevel, NetworkInformation};
    NetworkInformation::GetInternetConnectionProfile()
        .and_then(|profile| profile.GetNetworkConnectivityLevel())
        .is_ok_and(|level| level == NetworkConnectivityLevel::InternetAccess)
}

#[cfg(not(windows))]
fn online() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update_net::{Response, Url};
    use sha2::{Digest, Sha256};
    use std::io::Write;
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    #[test]
    fn only_a_newer_stable_release_is_offered() {
        assert!(newer_stable("v4.0.1", "4.0.0"));
        assert!(newer_stable("v4.1.0", "4.0.9"));
        assert!(newer_stable("5.0.0", "4.9.9"));
        assert!(newer_stable("v4.0.0", "4.0.0-rc.3"), "the final release follows its candidates");
        assert!(!newer_stable("v4.0.0", "4.0.0"));
        assert!(!newer_stable("v3.9.9", "4.0.0"));
        assert!(!newer_stable("v4.1.0-rc.1", "4.0.0"), "a pre-release tag is never offered");
        assert!(!newer_stable("nightly", "4.0.0"));
        assert!(!newer_stable("v4.1.0", "a local build"));
        assert!(newer_stable("v4.1", "4.0.0"), "a two-part tag reads as .0");
        assert!(!newer_stable("v4.1.0.2", "4.0.0"));
        for odd in ["v9.9.9+a/b", "9.9.9+build", "v9.9.9/..", " v9.9.9"] {
            assert!(!newer_stable(odd, "4.0.0"), "{odd}");
            assert_eq!(offer(&answer(odd, false, false, ""), "4.0.0", &CopyKind::Installed), Ok(None), "{odd}");
        }
        assert!(newer_stable("v4.1.0", "4.0.0+local"), "only the tag is held to this");
    }

    const SETUP: &str = "nightjar-desktop-4.1.0-windows-x86_64-setup.exe";
    const DIGEST: &str = "sha256:AB12000000000000000000000000000000000000000000000000000000000000";

    fn answer(tag: &str, draft: bool, pre: bool, assets: &str) -> String {
        format!(r#"{{"tag_name":"{tag}","draft":{draft},"prerelease":{pre},"html_url":"https://github.com/RamazanKara/nightjar/releases/tag/{tag}","assets":[{assets}]}}"#)
    }

    fn setup_asset(url: &str, digest: &str) -> String {
        format!(r#"{{"name":"{SETUP}","size":1000,"browser_download_url":"{url}"{digest}}}"#)
    }

    fn github_url() -> String {
        format!("https://github.com/RamazanKara/nightjar/releases/download/v4.1.0/{SETUP}")
    }

    #[test]
    fn the_answer_is_read_like_v_plus_reads_it() {
        let release = |tag: &str, draft: bool, pre: bool| answer(tag, draft, pre, "");
        assert_eq!(offer(&release("v4.1.0", false, false), "4.0.0", &CopyKind::Portable), Ok(Some(Release {
            version: "4.1.0".into(),
            url: "https://github.com/RamazanKara/nightjar/releases/tag/v4.1.0".into(),
            asset: Err(Failure::NoInstaller),
        })));
        assert_eq!(offer(&release("v4.1.0", true, false), "4.0.0", &CopyKind::Portable), Ok(None), "a draft");
        assert_eq!(offer(&release("v4.1.0", false, true), "4.0.0", &CopyKind::Portable), Ok(None), "a pre-release");
        assert_eq!(offer(&release("v4.0.0", false, false), "4.0.0", &CopyKind::Portable), Ok(None), "this version");
        assert!(offer("{\"message\":\"Not Found\"}", "4.0.0", &CopyKind::Portable).is_err());
        assert!(offer("<html>", "4.0.0", &CopyKind::Portable).is_err());
        assert!(offer(r#"{"tag_name":"v9.0.0","html_url":"https://evil.example/"}"#, "4.0.0", &CopyKind::Portable).is_err(), "only GitHub pages open");
    }

    #[test]
    fn the_release_answer_says_what_to_download_and_check() {
        let read = |assets: &str| offer(&answer("v4.1.0", false, false, assets), "4.0.0", &CopyKind::Installed).unwrap().unwrap().asset;
        let digest = format!(r#","digest":"{DIGEST}""#);
        assert_eq!(read(&setup_asset(&github_url(), &digest)), Ok(Asset {
            name: SETUP.into(),
            url: github_url(),
            digest: DIGEST[7..].to_ascii_lowercase(),
            size: 1000,
        }));
        assert_eq!(read(""), Err(Failure::NoInstaller));
    }

    #[test]
    fn a_missing_digest_refuses() {
        let read = |digest: &str| offer(&answer("v4.1.0", false, false, &setup_asset(&github_url(), digest)), "4.0.0", &CopyKind::Installed).unwrap().unwrap().asset;
        assert_eq!(read(""), Err(Failure::NoDigest), "no field");
        assert_eq!(read(r#","digest":null"#), Err(Failure::NoDigest), "null");
        assert_eq!(read(r#","digest":"md5:abcd""#), Err(Failure::NoDigest), "another hash");
        assert_eq!(read(r#","digest":"sha256:abcd""#), Err(Failure::NoDigest), "too short");
    }

    #[test]
    fn an_asset_on_another_host_refuses() {
        let digest = format!(r#","digest":"{DIGEST}""#);
        let url = format!("https://evil.example/RamazanKara/nightjar/releases/download/v4.1.0/{SETUP}");
        let release = offer(&answer("v4.1.0", false, false, &setup_asset(&url, &digest)), "4.0.0", &CopyKind::Installed).unwrap().unwrap();
        assert_eq!(release.asset, Err(Failure::Address(url)));
    }

    /// The real request: `cargo test -p nightjar-desktop asks_github -- --ignored`.
    #[test]
    #[ignore = "asks GitHub over the network"]
    fn asks_github() {
        let net = Native::new("Nightjar/test".into());
        let json = check(&net, &CopyKind::Installed, "0.0.1", || true).unwrap().unwrap();
        assert!(json.url.starts_with("https://github.com/RamazanKara/nightjar/"), "{json:?}");
        assert!(json.asset.as_ref().is_ok_and(|a| a.size > 0 && a.digest.len() == 64), "{json:?}");
        assert_eq!(check(&net, &CopyKind::Installed, "999.0.0", || true).unwrap(), None);
    }

    /// The real download, through GitHub's real redirect, without running anything:
    /// `cargo test -p nightjar-desktop downloads_the_real -- --ignored`.
    #[test]
    #[ignore = "downloads from GitHub over the network"]
    fn downloads_the_real_setup() {
        let net = Native::new("Nightjar/test".into());
        let asset = check(&net, &CopyKind::Installed, "0.0.1", || true).unwrap().unwrap().asset.unwrap();
        let part = folder("real").join("setup.part");
        let progress = Progress { cancel: AtomicBool::new(false), received: AtomicU64::new(0), wake: Arc::new(|| {}) };
        self_update::download(&net, self_update::download_host, &asset, &part, &progress).unwrap();
        assert_eq!(std::fs::metadata(&part).unwrap().len(), asset.size);
        std::fs::remove_file(&part).unwrap();
    }

    #[test]
    fn off_is_off_and_no_request_is_made() {
        let updates = Updates::start(false, Arc::new(|| {}));
        assert!(updates.pending.is_none() && updates.found.is_none());
    }

    #[test]
    fn with_the_check_off_leftovers_still_go_and_nothing_is_asked() {
        let installer = Installer::new("off-leftovers");
        let part = installer.dir.join(".nightjar-update-1-2.part");
        std::fs::write(&part, b"x").unwrap();
        std::fs::File::options().write(true).open(&part).unwrap().set_modified(std::time::SystemTime::now() - Duration::from_secs(7200)).unwrap();
        let count = Arc::new(Counting::default());
        let net: Arc<dyn Transport> = count.clone();
        let updates = Updates::begin(false, CopyKind::Installed, net.clone(), Some(installer.steps(net, local)), "4.0.0", || true, Arc::new(|| {}));
        wait_until("the leftover stays", || !part.exists());
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(count.gets.load(Ordering::Relaxed), 0, "no request");
        assert!(updates.pending.is_none() && updates.found.is_none() && !updates.checking());
    }

    #[test]
    fn check_now_asks_once_when_chosen_with_the_setting_off() {
        let installer = Installer::new("check-now");
        let count = Arc::new(Counting::default());
        let digest = format!(r#","digest":"{DIGEST}""#);
        *count.answer.lock().unwrap() = answer("v4.1.0", false, false, &setup_asset(&github_url(), &digest));
        let net: Arc<dyn Transport> = count.clone();
        let mut updates = Updates::begin(false, CopyKind::Installed, net.clone(), Some(installer.steps(net, local)), "4.0.0", || true, Arc::new(|| {}));
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(count.gets.load(Ordering::Relaxed), 0, "nothing until it is chosen");
        assert!(updates.check_now());
        assert!(!updates.check_now(), "one check at a time");
        wait_until("no answer", || { updates.step(); !updates.checking() });
        assert_eq!(updates.answer(), Some(Answer::Found));
        assert_eq!(updates.answer(), None, "the answer comes once");
        assert!(updates.found.as_ref().unwrap().asset.is_ok(), "the notice offers Update now");
        assert_eq!(count.gets.load(Ordering::Relaxed), 1);
        assert!(installer.ran().is_empty(), "nothing installs without the click");

        // Dismissed, the notice comes back with the next check; this version is up to date.
        updates.found = None;
        assert!(updates.check_now());
        wait_until("no second answer", || { updates.step(); !updates.checking() });
        assert_eq!(updates.answer(), Some(Answer::Found));
        assert!(updates.found.is_some());
        *count.answer.lock().unwrap() = answer("v4.0.0", false, false, "");
        assert!(updates.check_now());
        wait_until("no third answer", || { updates.step(); !updates.checking() });
        assert_eq!(updates.answer(), Some(Answer::UpToDate));
        assert_eq!(Answer::UpToDate.toast(), Some("You have the latest version"));
        *count.answer.lock().unwrap() = "<html>".into();
        assert!(updates.check_now());
        wait_until("no fourth answer", || { updates.step(); !updates.checking() });
        assert_eq!(updates.answer(), Some(Answer::Failed));
        assert_eq!(Answer::Failed.toast(), Some("Couldn’t check for updates"));
        assert_eq!(Answer::Found.toast(), None, "the notice says it");
    }

    #[test]
    fn a_managed_copy_has_no_check_now() {
        let mut updates = Updates::default();
        assert!(!updates.check_now());
        assert!(!updates.checking() && updates.answer().is_none());
    }

    // A local server stands in for GitHub; the real WinHTTP or ureq talks to it.

    #[derive(Clone)]
    enum Reply {
        Body(Vec<u8>),
        /// Says one length and sends fewer bytes.
        Short(u64, Vec<u8>),
        /// No length: the end of the connection ends the body.
        Unsized(Vec<u8>),
        Redirect(String),
        /// Sends the first part, then waits for the release before sending the rest.
        Hold(Vec<u8>, Vec<u8>, Arc<Mutex<Option<Receiver<()>>>>),
    }

    struct Fixture {
        port: u16,
        hits: Arc<Mutex<Vec<String>>>,
    }

    impl Fixture {
        fn start(route: impl Fn(&str, u16) -> Reply + Send + Sync + 'static) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let hits = Arc::new(Mutex::new(Vec::new()));
            let (seen, route) = (hits.clone(), Arc::new(route));
            std::thread::spawn(move || {
                for connection in listener.incoming().flatten() {
                    let (seen, route) = (seen.clone(), route.clone());
                    std::thread::spawn(move || serve(connection, &seen, &*route, port));
                }
            });
            Self { port, hits }
        }

        fn url(&self, path: &str) -> String {
            format!("http://127.0.0.1:{}{path}", self.port)
        }

        fn hits(&self) -> Vec<String> {
            self.hits.lock().unwrap().clone()
        }
    }

    fn serve(mut connection: std::net::TcpStream, seen: &Mutex<Vec<String>>, route: &(dyn Fn(&str, u16) -> Reply + Send + Sync), port: u16) {
        let mut request = Vec::new();
        let mut byte = [0u8];
        while !request.ends_with(b"\r\n\r\n") {
            if connection.read(&mut byte).unwrap_or(0) == 0 { return; }
            request.push(byte[0]);
        }
        let text = String::from_utf8_lossy(&request);
        let path = text.split_whitespace().nth(1).unwrap_or("").to_owned();
        seen.lock().unwrap().push(path.clone());
        let _ = match route(&path, port) {
            Reply::Body(body) => write!(connection, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).and_then(|()| connection.write_all(&body)),
            Reply::Short(declared, body) => write!(connection, "HTTP/1.1 200 OK\r\nContent-Length: {declared}\r\nConnection: close\r\n\r\n").and_then(|()| connection.write_all(&body)),
            Reply::Unsized(body) => write!(connection, "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n").and_then(|()| connection.write_all(&body)),
            Reply::Redirect(to) => write!(connection, "HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
            Reply::Hold(first, rest, release) => {
                let length = first.len() + rest.len();
                let receiver = release.lock().unwrap().take();
                write!(connection, "HTTP/1.1 200 OK\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n")
                    .and_then(|()| connection.write_all(&first))
                    .and_then(|()| connection.flush())
                    .map(|()| { if let Some(r) = receiver { let _ = r.recv_timeout(Duration::from_secs(20)); } })
                    .and_then(|()| connection.write_all(&rest))
            }
        };
    }

    fn local(url: &Url) -> bool {
        !url.secure && url.host == "127.0.0.1"
    }

    fn folder(name: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!("nj-update-check-{}-{}-{name}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        // Windows reuses process ids: an earlier run's folder of the same name may still be there.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sha256(bytes: &[u8]) -> String {
        Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    /// The fake installer: keeps what it was handed instead of running anything.
    struct Installer {
        dir: PathBuf,
        ran: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    impl Installer {
        fn new(name: &str) -> Self {
            Self { dir: folder(name), ran: Arc::default() }
        }

        fn steps(&self, net: Arc<dyn Transport>, allow: fn(&Url) -> bool) -> Steps {
            let ran = self.ran.clone();
            Steps {
                net,
                allow,
                dir: self.dir.clone(),
                own_dir: true,
                marker: Some(self.marker()),
                stream_elsewhere: || false,
                setup_running: || false,
                apply: Box::new(move |part, _| {
                    ran.lock().unwrap().push(std::fs::read(part).unwrap());
                    std::fs::remove_file(part).unwrap();
                    Ok(())
                }),
            }
        }

        fn marker(&self) -> PathBuf {
            self.dir.join("update-pending.json")
        }

        fn ran(&self) -> Vec<Vec<u8>> {
            self.ran.lock().unwrap().clone()
        }

        /// No unfinished file stays behind.
        fn parts(&self) -> usize {
            std::fs::read_dir(&self.dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".part")).count()
        }
    }

    fn native() -> Arc<dyn Transport> {
        Arc::new(Native::new("Nightjar/test".into()))
    }

    fn notice(asset: Result<Asset, Failure>, steps: Option<Steps>) -> Updates {
        Updates {
            found: Some(Release { version: "4.1.0".into(), url: "https://github.com/RamazanKara/nightjar/releases/tag/v4.1.0".into(), asset }),
            steps,
            ..Updates::default()
        }
    }

    fn asset_at(url: String, body: &[u8]) -> Asset {
        Asset { name: SETUP.into(), url, digest: sha256(body), size: body.len() as u64 }
    }

    /// Steps until the download is over. Whether Nightjar should exit.
    fn finish(updates: &mut Updates) -> bool {
        let deadline = Instant::now() + Duration::from_secs(30);
        while updates.job.is_some() {
            assert!(Instant::now() < deadline, "the download never ended");
            if let Some(part) = updates.step() { return updates.hand_over(&part); }
            std::thread::sleep(Duration::from_millis(2));
        }
        false
    }

    fn body() -> Vec<u8> {
        (0..300_000u32).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn update_now_downloads_checks_hands_over_and_exits() {
        let bytes = body();
        let served = bytes.clone();
        let fixture = Fixture::start(move |path, port| match path {
            "/setup" => Reply::Redirect(format!("http://127.0.0.1:{port}/storage?sig=1")),
            _ => Reply::Body(served.clone()),
        });
        let installer = Installer::new("good");
        let mut updates = notice(Ok(asset_at(fixture.url("/setup"), &bytes)), Some(installer.steps(native(), local)));
        updates.update_now();
        assert!(finish(&mut updates), "Nightjar exits for the installer");
        assert_eq!(installer.ran(), [bytes]);
        assert_eq!(fixture.hits(), ["/setup", "/storage?sig=1"]);
        assert_eq!(installer.parts(), 0);
        let attempt = self_update::take_attempt(&installer.marker()).expect("the next launch can tell");
        assert_eq!(attempt.version, "4.1.0");
    }

    fn refused(reply: Reply, asset: impl FnOnce(&Fixture) -> Asset) -> Failure {
        let fixture = Fixture::start(move |_, _| reply.clone());
        let installer = Installer::new("refused");
        let mut updates = notice(Ok(asset(&fixture)), Some(installer.steps(native(), local)));
        updates.update_now();
        assert!(!finish(&mut updates), "Nightjar stays open");
        assert!(installer.ran().is_empty(), "nothing ran");
        assert_eq!(installer.parts(), 0, "nothing stays behind");
        assert!(!installer.marker().exists());
        updates.failure.expect("the notice says why")
    }

    #[test]
    fn a_tampered_download_never_runs() {
        let bytes = body();
        let mut tampered = bytes.clone();
        tampered[1000] ^= 1;
        assert_eq!(refused(Reply::Body(tampered), |f| asset_at(f.url("/setup"), &bytes)), Failure::Digest);
    }

    #[test]
    fn a_truncated_download_never_runs() {
        let bytes = body();
        let half = bytes[..bytes.len() / 2].to_vec();
        let failure = refused(Reply::Short(bytes.len() as u64, half.clone()), |f| asset_at(f.url("/setup"), &bytes));
        assert!(matches!(failure, Failure::Size | Failure::Network(_)), "the connection ended early: {failure:?}");
        assert_eq!(refused(Reply::Unsized(half), |f| asset_at(f.url("/setup"), &bytes)), Failure::Size, "no length to go by");
    }

    #[test]
    fn a_download_of_another_size_never_runs() {
        let bytes = body();
        let mut longer = bytes.clone();
        longer.push(0);
        assert_eq!(refused(Reply::Body(longer.clone()), |f| asset_at(f.url("/setup"), &bytes)), Failure::Size, "the length says so");
        assert_eq!(refused(Reply::Unsized(longer), |f| asset_at(f.url("/setup"), &bytes)), Failure::Size, "the bytes say so");
    }

    #[test]
    fn a_redirect_to_another_host_is_refused_unvisited() {
        let bytes = body();
        for target in ["http://localhost:{port}/evil", "https://evil.example/evil", "https://127.0.0.1:{port}/evil"] {
            let fixture = Fixture::start(move |path, port| match path {
                "/setup" => Reply::Redirect(target.replace("{port}", &port.to_string())),
                _ => Reply::Body(Vec::new()),
            });
            let installer = Installer::new("redirect");
            let mut updates = notice(Ok(asset_at(fixture.url("/setup"), &bytes)), Some(installer.steps(native(), local)));
            updates.update_now();
            assert!(!finish(&mut updates));
            assert!(matches!(updates.failure, Some(Failure::Address(_))), "{target}: {:?}", updates.failure);
            assert_eq!(fixture.hits(), ["/setup"], "{target}");
            assert!(installer.ran().is_empty());
        }
    }

    #[test]
    fn a_wrong_host_is_refused_before_connecting() {
        let bytes = body();
        let fixture = Fixture::start(|_, _| Reply::Body(Vec::new()));
        let installer = Installer::new("host");
        // With the real rule, the fixture's plain local address is itself a wrong host.
        let mut updates = notice(Ok(asset_at(fixture.url("/setup"), &bytes)), Some(installer.steps(native(), self_update::download_host)));
        updates.update_now();
        assert!(!finish(&mut updates));
        assert_eq!(updates.failure, Some(Failure::Address("127.0.0.1".into())));
        assert!(fixture.hits().is_empty());
        assert!(installer.ran().is_empty());
    }

    #[test]
    fn a_missing_digest_refuses_on_click_and_the_link_stays() {
        let installer = Installer::new("no-digest");
        let count = Arc::new(Counting::default());
        let mut updates = notice(Err(Failure::NoDigest), Some(installer.steps(count.clone(), local)));
        updates.update_now();
        assert_eq!(updates.failure, Some(Failure::NoDigest));
        assert!(updates.job.is_none() && updates.step().is_none() && installer.ran().is_empty());
        assert_eq!(count.gets.load(Ordering::Relaxed), 0, "nothing is fetched");
        assert!(updates.found.is_some(), "the release link stays");
        assert!(Failure::NoDigest.is_final(), "and Update now goes");
        assert!(!failure_text(&Failure::NoDigest, "4.1.0").is_empty());
    }

    fn held_reply(bytes: &[u8]) -> (Reply, std::sync::mpsc::Sender<()>) {
        let (release, wait) = channel();
        let (first, rest) = bytes.split_at(bytes.len() / 3);
        (Reply::Hold(first.to_vec(), rest.to_vec(), Arc::new(Mutex::new(Some(wait)))), release)
    }

    fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !done() {
            assert!(Instant::now() < deadline, "{what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn a_server_that_goes_silent_fails_the_download() {
        let bytes = body();
        // Kept until the end: the reply never goes on.
        let (reply, _release) = held_reply(&bytes);
        let fixture = Fixture::start(move |_, _| reply.clone());
        let installer = Installer::new("silent");
        let net: Arc<dyn Transport> = Arc::new(Native { agent: "Nightjar/test".into(), read_timeout: Duration::from_millis(300) });
        let mut updates = notice(Ok(asset_at(fixture.url("/setup"), &bytes)), Some(installer.steps(net, local)));
        updates.update_now();
        assert!(!finish(&mut updates), "Nightjar stays open");
        assert!(matches!(updates.failure, Some(Failure::Network(_))), "{:?}", updates.failure);
        assert_eq!(installer.parts(), 0);
        assert!(installer.ran().is_empty());
    }

    #[test]
    fn cancel_stops_the_download_and_runs_nothing() {
        let bytes = body();
        let (reply, release) = held_reply(&bytes);
        let fixture = Fixture::start(move |_, _| reply.clone());
        let installer = Installer::new("cancel");
        let mut updates = notice(Ok(asset_at(fixture.url("/setup"), &bytes)), Some(installer.steps(native(), local)));
        updates.update_now();
        let progress = updates.job.as_ref().unwrap().progress.clone();
        wait_until("no progress", || progress.received.load(Ordering::Relaxed) > 0);
        updates.cancel();
        assert!(updates.job.is_none() && updates.failure.is_none(), "cancelling isn't a failure");
        let _ = release.send(());
        wait_until("the file stays", || installer.parts() == 0);
        for _ in 0..50 {
            assert!(updates.step().is_none());
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(installer.ran().is_empty());
        assert!(!installer.marker().exists());
        updates.update_now();
        assert!(updates.job.is_some(), "and it can start over");
        updates.cancel();
    }

    #[test]
    fn a_stream_starting_stops_the_download_and_nothing_runs() {
        let bytes = body();
        let (reply, release) = held_reply(&bytes);
        let fixture = Fixture::start(move |_, _| reply.clone());
        let installer = Installer::new("stream");
        let mut updates = notice(Ok(asset_at(fixture.url("/setup"), &bytes)), Some(installer.steps(native(), local)));
        updates.update_now();
        let progress = updates.job.as_ref().unwrap().progress.clone();
        wait_until("no progress", || progress.received.load(Ordering::Relaxed) > 0);
        updates.hold(true);
        assert!(updates.job.is_none(), "no download next to a stream");
        let _ = release.send(());
        wait_until("the file stays", || installer.parts() == 0);
        assert!(updates.step().is_none());
        assert!(installer.ran().is_empty());
    }

    #[test]
    fn update_now_does_nothing_during_a_stream() {
        let bytes = body();
        let count = Arc::new(Counting::default());
        let installer = Installer::new("streaming");
        let mut updates = notice(Ok(asset_at("http://127.0.0.1:1/setup".into(), &bytes)), Some(installer.steps(count.clone(), local)));
        updates.hold(true);
        updates.update_now();
        assert!(updates.job.is_none());
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(count.gets.load(Ordering::Relaxed), 0);
        updates.hold(false);
        updates.update_now();
        assert!(updates.job.is_some(), "back on the launcher it works");
        updates.cancel();
    }

    #[test]
    fn a_finished_download_is_never_handed_over_once_a_stream_started() {
        let bytes = body();
        let served = bytes.clone();
        let fixture = Fixture::start(move |_, _| Reply::Body(served.clone()));
        let installer = Installer::new("finished");
        let mut updates = notice(Ok(asset_at(fixture.url("/setup"), &bytes)), Some(installer.steps(native(), local)));
        updates.update_now();
        // The download may end before the stream's first pass of the main loop; step isn't called
        // until after that pass. The thread is done when only the job and this test hold this.
        let progress = updates.job.as_ref().unwrap().progress.clone();
        wait_until("the download never ended", || Arc::strong_count(&progress) == 2);
        updates.hold(true);
        assert!(updates.step().is_none());
        wait_until("the file stays", || installer.parts() == 0);
        assert!(installer.ran().is_empty());
    }

    #[test]
    fn a_stream_in_another_process_stops_the_update() {
        static STREAMING: AtomicBool = AtomicBool::new(true);
        let bytes = body();
        let served = bytes.clone();
        let fixture = Fixture::start(move |_, _| Reply::Body(served.clone()));
        let installer = Installer::new("elsewhere");
        let mut steps = installer.steps(native(), local);
        steps.stream_elsewhere = || STREAMING.load(Ordering::SeqCst);
        let mut updates = notice(Ok(asset_at(fixture.url("/setup"), &bytes)), Some(steps));
        updates.update_now();
        assert_eq!(updates.failure, Some(Failure::StreamElsewhere), "it says why");
        assert!(updates.job.is_none() && fixture.hits().is_empty(), "nothing is fetched");
        assert!(!Failure::StreamElsewhere.is_final(), "Update now stays for after that stream");
        // It starts during the download: the file goes and nothing runs.
        STREAMING.store(false, Ordering::SeqCst);
        updates.update_now();
        let progress = updates.job.as_ref().unwrap().progress.clone();
        wait_until("the download never ended", || Arc::strong_count(&progress) == 2);
        STREAMING.store(true, Ordering::SeqCst);
        assert!(updates.step().is_none(), "Nightjar stays open");
        assert_eq!(updates.failure, Some(Failure::StreamElsewhere));
        assert!(installer.ran().is_empty() && !installer.marker().exists());
        assert_eq!(installer.parts(), 0);
    }

    #[test]
    fn a_setup_already_running_stops_the_hand_over() {
        let bytes = body();
        let served = bytes.clone();
        let fixture = Fixture::start(move |_, _| Reply::Body(served.clone()));
        let installer = Installer::new("second-setup");
        let mut steps = installer.steps(native(), local);
        steps.setup_running = || true;
        let mut updates = notice(Ok(asset_at(fixture.url("/setup"), &bytes)), Some(steps));
        updates.update_now();
        assert!(!finish(&mut updates), "Nightjar stays open");
        assert!(matches!(updates.failure, Some(Failure::Apply(_))), "it says why: {:?}", updates.failure);
        assert!(!updates.failure.as_ref().unwrap().is_final(), "Update now stays for afterwards");
        assert!(installer.ran().is_empty() && !installer.marker().exists(), "no second setup, no false alarm");
        assert_eq!(installer.parts(), 0);
    }

    /// The next Nightjar can read its files as soon as it starts, while this one is still leaving.
    #[test]
    fn nothing_runs_before_the_caller_has_saved() {
        let bytes = body();
        let served = bytes.clone();
        let fixture = Fixture::start(move |_, _| Reply::Body(served.clone()));
        let installer = Installer::new("saves-first");
        let mut updates = notice(Ok(asset_at(fixture.url("/setup"), &bytes)), Some(installer.steps(native(), local)));
        updates.update_now();
        let deadline = Instant::now() + Duration::from_secs(30);
        let part = loop {
            assert!(Instant::now() < deadline, "the download never ended");
            if let Some(part) = updates.step() { break part; }
            std::thread::sleep(Duration::from_millis(2));
        };
        assert!(installer.ran().is_empty() && !installer.marker().exists(), "the caller's turn to save");
        assert_eq!(std::fs::read(&part).unwrap(), bytes);
        assert!(updates.hand_over(&part));
        assert_eq!(installer.ran(), [bytes]);
        assert!(installer.marker().exists());

        let main = include_str!("main.rs");
        let frame = main.split("self.updates.step()").nth(1).expect("the main loop steps the update");
        let (saved, handed) = (frame.find("self.save_for_next_run()").unwrap(), frame.find("self.updates.hand_over(").unwrap());
        assert!(saved < handed, "the main loop saves first");
    }

    #[test]
    fn a_hand_over_that_fails_after_the_save_stays_open_and_says_why() {
        let bytes = body();
        let served = bytes.clone();
        let fixture = Fixture::start(move |_, _| Reply::Body(served.clone()));
        let installer = Installer::new("late-failure");
        let mut steps = installer.steps(native(), local);
        steps.apply = Box::new(|_, _| Err(Failure::Restart("no".into())));
        let mut updates = notice(Ok(asset_at(fixture.url("/setup"), &bytes)), Some(steps));
        updates.update_now();
        assert!(!finish(&mut updates));
        assert_eq!(updates.failure, Some(Failure::Restart("no".into())));
    }

    #[cfg(windows)]
    #[test]
    fn a_stream_here_is_visible_from_other_processes() {
        let mut updates = Updates::default();
        updates.hold(true);
        assert!(self_update::mutex_exists(self_update::STREAM_MUTEX));
        updates.hold(true);
        assert!(updates.stream_mark.is_some(), "while it runs");
        updates.hold(false);
        assert!(updates.stream_mark.is_none(), "and not after");
    }

    /// Answers the release question and counts what it is asked.
    #[derive(Default)]
    struct Counting {
        gets: AtomicU64,
        answer: Mutex<String>,
    }

    impl Transport for Counting {
        fn get(&self, _: &Url, _: bool) -> Result<Response, String> {
            self.gets.fetch_add(1, Ordering::Relaxed);
            let body = self.answer.lock().unwrap().clone().into_bytes();
            Ok(Response { status: 200, location: None, length: None, body: Box::new(std::io::Cursor::new(body)) })
        }
    }

    fn frame(updates: &mut Updates, ctx: &egui::Context) {
        ctx.run_ui(egui::RawInput::default(), |ui| updates.show(ui.ctx())).textures_delta.clear();
    }

    #[test]
    fn drawing_the_notice_never_takes_the_answer_to_exit() {
        let bytes = body();
        let served = bytes.clone();
        let fixture = Fixture::start(move |_, _| Reply::Body(served.clone()));
        let installer = Installer::new("two-passes");
        let mut updates = notice(Ok(asset_at(fixture.url("/setup"), &bytes)), Some(installer.steps(native(), local)));
        updates.update_now();
        let progress = updates.job.as_ref().unwrap().progress.clone();
        wait_until("the download never ended", || Arc::strong_count(&progress) == 2);
        // egui runs a frame's closure a second time after request_discard.
        let ctx = egui::Context::default();
        frame(&mut updates, &ctx);
        frame(&mut updates, &ctx);
        assert!(installer.ran().is_empty(), "nothing happens while drawing");
        assert!(updates.step().is_some(), "the main loop gets the answer");
    }

    fn checked(copy: CopyKind, steps: impl FnOnce(Arc<dyn Transport>) -> Option<Steps>) -> (Updates, Arc<Counting>) {
        let count = Arc::new(Counting::default());
        let digest = format!(r#","digest":"{DIGEST}""#);
        *count.answer.lock().unwrap() = answer("v4.1.0", false, false, &setup_asset(&github_url(), &digest));
        let net: Arc<dyn Transport> = count.clone();
        let steps = steps(net.clone());
        let mut updates = Updates::begin(true, copy, net, steps, "4.0.0", || true, Arc::new(|| {}));
        wait_until("no answer", || { updates.step(); updates.found.is_some() });
        (updates, count)
    }

    #[test]
    fn the_notice_starts_nothing_without_the_click() {
        let installer = Installer::new("no-click");
        let ctx = egui::Context::default();
        let (mut updates, count) = checked(CopyKind::Installed, |net| Some(installer.steps(net, local)));
        for _ in 0..20 {
            frame(&mut updates, &ctx);
            assert!(updates.step().is_none());
        }
        assert!(updates.job.is_none());
        assert!(updates.found.as_ref().unwrap().asset.is_ok());
        assert_eq!(count.gets.load(Ordering::Relaxed), 1, "only the release question");
        assert!(installer.ran().is_empty());
    }

    #[test]
    fn a_portable_copy_only_has_the_link() {
        let ctx = egui::Context::default();
        let (mut updates, count) = checked(CopyKind::Portable, |_| None);
        assert_eq!(updates.found.as_ref().unwrap().asset, Err(Failure::NoInstaller));
        updates.update_now();
        for _ in 0..5 {
            frame(&mut updates, &ctx);
            assert!(updates.step().is_none());
        }
        assert!(updates.job.is_none() && updates.failure.is_none());
        assert_eq!(count.gets.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn a_setup_that_did_not_install_says_so_on_the_next_launch() {
        let installer = Installer::new("failed-setup");
        let attempt = Attempt { version: "4.1.0".into(), url: "https://github.com/RamazanKara/nightjar/releases/tag/v4.1.0".into() };
        self_update::write_attempt(&installer.marker(), &attempt).unwrap();
        let count: Arc<dyn Transport> = Arc::new(Counting::default());
        let mut updates = Updates::begin(true, CopyKind::Installed, count.clone(), Some(installer.steps(count, local)), "4.0.0", || false, Arc::new(|| {}));
        assert_eq!(updates.failure, Some(Failure::NotInstalled));
        assert_eq!(updates.found.as_ref().unwrap().url, attempt.url, "the link is there");
        assert!(!installer.marker().exists(), "it says so once");
        updates.update_now();
        assert!(updates.job.is_none(), "nothing to download until the check answers");
        frame(&mut updates, &egui::Context::default());
    }

    #[test]
    fn a_setup_that_did_install_says_nothing() {
        for running in ["4.1.0", "4.2.0"] {
            let installer = Installer::new("good-setup");
            let attempt = Attempt { version: "4.1.0".into(), url: "https://github.com/RamazanKara/nightjar/releases/tag/v4.1.0".into() };
            self_update::write_attempt(&installer.marker(), &attempt).unwrap();
            assert_eq!(failed_attempt(&installer.marker(), running), None, "{running}");
            assert!(!installer.marker().exists());
        }
    }

    #[test]
    fn the_appimage_flow_replaces_the_file_with_the_verified_download() {
        let bytes = body();
        let served = bytes.clone();
        let fixture = Fixture::start(move |_, _| Reply::Body(served.clone()));
        let dir = folder("appimage");
        let target = dir.join("Nightjar.AppImage");
        std::fs::write(&target, b"old").unwrap();
        let started = Arc::new(AtomicBool::new(false));
        let (copy, flag) = (target.clone(), started.clone());
        let steps = Steps {
            net: native(),
            allow: local,
            dir: dir.clone(),
            own_dir: false,
            marker: None,
            stream_elsewhere: || false,
            setup_running: || false,
            apply: Box::new(move |part, _| {
                self_update::replace_file(part, &copy).map_err(Failure::Apply)?;
                flag.store(true, Ordering::Relaxed);
                Ok(())
            }),
        };
        let mut updates = notice(Ok(asset_at(fixture.url("/image"), &bytes)), Some(steps));
        updates.update_now();
        assert!(finish(&mut updates));
        assert_eq!(std::fs::read(&target).unwrap(), bytes);
        assert!(started.load(Ordering::Relaxed));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "only the new AppImage");
    }

    #[test]
    fn a_failed_hand_over_stays_open_and_says_why() {
        let bytes = body();
        let served = bytes.clone();
        let fixture = Fixture::start(move |_, _| Reply::Body(served.clone()));
        let installer = Installer::new("no-start");
        let mut steps = installer.steps(native(), local);
        steps.apply = Box::new(|part, _| {
            std::fs::remove_file(part).unwrap();
            Err(Failure::Apply("no".into()))
        });
        let mut updates = notice(Ok(asset_at(fixture.url("/setup"), &bytes)), Some(steps));
        updates.update_now();
        assert!(!finish(&mut updates));
        assert_eq!(updates.failure, Some(Failure::Apply("no".into())));
        assert!(!installer.marker().exists(), "no false alarm on the next launch");
    }

    #[test]
    fn every_failure_has_words() {
        for failure in [Failure::NoInstaller, Failure::NoDigest, Failure::Digest, Failure::Size, Failure::Address("x".into()), Failure::Network("x".into()), Failure::Apply("x".into()), Failure::Restart("x".into()), Failure::NotInstalled, Failure::StreamElsewhere] {
            assert!(!failure_text(&failure, "4.1.0").is_empty(), "{failure:?}");
        }
    }
}
