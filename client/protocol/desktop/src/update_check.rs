//! The Windows update check, ported from Moonlight V+ PC's AutoUpdateChecker
//! (qiin2333/moonlight-qt, `app/backend/autoupdatechecker.cpp`) and pointed at
//! RamazanKara/nightjar: once per launch, when the setting is on and the PC is online, it asks
//! GitHub for the latest release and, when that is a stable version newer than this one, the
//! launcher says so with a link to the release page. Nothing is downloaded or installed. Linux
//! gets updates from its package manager, so the check is Windows only.
//!
//! Failures stay out of the UI (anonymous GitHub requests are rate limited, as Android's
//! UpdateChecker notes) and go to the log.

use std::sync::mpsc::{channel, Receiver};

use crate::strings::{tr, trf};

#[cfg_attr(not(windows), allow(dead_code))]
const HOST: &str = "api.github.com";
#[cfg_attr(not(windows), allow(dead_code))]
const PATH: &str = "/repos/RamazanKara/nightjar/releases/latest";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub url: String,
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
/// own final version, as in SemVer.
pub fn newer_stable(tag: &str, current: &str) -> bool {
    let (Some((latest, None)), Some((running, pre))) = (parse(tag), parse(current)) else { return false };
    latest > running || (latest == running && pre.is_some())
}

/// The release GitHub answered, if it is an update for `current`. Drafts and pre-releases never
/// are (GitHub's latest release excludes them; the flags are checked as V+ PC does).
pub fn offer(json: &str, current: &str) -> Result<Option<Release>, String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("unreadable release: {e}"))?;
    let tag = value["tag_name"].as_str().ok_or("the release has no tag")?;
    if value["draft"].as_bool() == Some(true) || value["prerelease"].as_bool() == Some(true) || !newer_stable(tag, current) {
        return Ok(None);
    }
    let url = value["html_url"].as_str().filter(|u| u.starts_with("https://github.com/")).ok_or("the release has no page")?;
    Ok(Some(Release { version: tag.trim_start_matches('v').to_owned(), url: url.to_owned() }))
}

/// The launch's one check and what it found.
#[derive(Default)]
pub struct Updates {
    pending: Option<Receiver<Release>>,
    found: Option<Release>,
}

impl Updates {
    /// Starts the check on its own thread; `wake` redraws the launcher when it has an answer.
    pub fn start(enabled: bool, wake: crate::hosts::Waker) -> Self {
        if !enabled || !cfg!(windows) { return Self::default(); }
        let (send, receive) = channel();
        let spawned = std::thread::Builder::new().name("update-check".into()).spawn(move || {
            match check() {
                Ok(Some(release)) => {
                    eprintln!("nightjar: update check: Nightjar {} is available", release.version);
                    let _ = send.send(release);
                    wake();
                }
                Ok(None) => eprintln!("nightjar: update check: up to date"),
                Err(e) => eprintln!("nightjar: update check: {e}"),
            }
        });
        if let Err(e) = spawned { eprintln!("nightjar: update check: no thread: {e}"); }
        Self { pending: Some(receive), found: None }
    }

    /// The notice over the launcher until it is opened or dismissed.
    pub fn show(&mut self, ctx: &egui::Context) {
        if let Some(release) = self.pending.as_ref().and_then(|r| r.try_recv().ok()) {
            self.pending = None;
            self.found = Some(release);
        }
        let Some(release) = &self.found else { return };
        let mut close = false;
        egui::Area::new(egui::Id::unique("update-notice"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-24.0, -24.0))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgba_unmultiplied(0x28, 0x2A, 0x36, 245))
                    .corner_radius(egui::CornerRadius::same(14))
                    .inner_margin(egui::Margin::symmetric(16, 12))
                    .show(ui, |ui| {
                        ui.label(trf("Nightjar {} is available.", &[&release.version]));
                        ui.horizontal(|ui| {
                            if ui.button(tr("Open download page")).clicked() {
                                if let Err(e) = crate::sdl::open_url(&release.url) { eprintln!("nightjar: update page: {e}"); }
                                close = true;
                            }
                            close |= ui.button(tr("Not now")).clicked();
                        });
                    });
            });
        if close { self.found = None; }
    }
}

#[cfg(windows)]
fn check() -> Result<Option<Release>, String> {
    if !online() { return Err("skipped, offline".into()); }
    let json = winhttp::get(HOST, PATH, &format!("Nightjar/{} (Windows)", crate::VERSION))?;
    offer(&json, crate::VERSION)
}

#[cfg(not(windows))]
fn check() -> Result<Option<Release>, String> {
    Ok(None)
}

/// Windows says the internet is reachable (not only a local network).
#[cfg(windows)]
fn online() -> bool {
    use windows::Networking::Connectivity::{NetworkConnectivityLevel, NetworkInformation};
    NetworkInformation::GetInternetConnectionProfile()
        .and_then(|profile| profile.GetNetworkConnectivityLevel())
        .is_ok_and(|level| level == NetworkConnectivityLevel::InternetAccess)
}

/// One HTTPS GET through WinHTTP: the system's proxy settings and certificate store, nothing
/// bundled.
#[cfg(windows)]
mod winhttp {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Networking::WinHttp::*;

    struct Handle(*mut core::ffi::c_void);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: a handle WinHTTP returned, closed once.
            if !self.0.is_null() { unsafe { let _ = WinHttpCloseHandle(self.0); } }
        }
    }

    fn open(handle: *mut core::ffi::c_void, what: &str) -> Result<Handle, String> {
        if handle.is_null() { Err(format!("{what}: {}", windows::core::Error::from_thread())) } else { Ok(Handle(handle)) }
    }

    pub fn get(host: &str, path: &str, agent: &str) -> Result<String, String> {
        // SAFETY: WinHTTP calls on handles owned here; strings outlive the calls.
        unsafe {
            let session = open(WinHttpOpen(&HSTRING::from(agent), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0), "WinHttpOpen")?;
            WinHttpSetTimeouts(session.0, 10_000, 10_000, 15_000, 15_000).map_err(|e| format!("timeouts: {e}"))?;
            let connection = open(WinHttpConnect(session.0, &HSTRING::from(host), 443, 0), "connect")?;
            let request = open(WinHttpOpenRequest(connection.0, &HSTRING::from("GET"), &HSTRING::from(path), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), WINHTTP_FLAG_SECURE), "request")?;
            let headers: Vec<u16> = "Accept: application/vnd.github+json\r\nX-GitHub-Api-Version: 2022-11-28\r\n".encode_utf16().collect();
            WinHttpSendRequest(request.0, Some(&headers), None, 0, 0, 0).map_err(|e| format!("send: {e}"))?;
            WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(|e| format!("response: {e}"))?;
            let mut status = 0u32;
            let mut size = std::mem::size_of::<u32>() as u32;
            WinHttpQueryHeaders(request.0, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, PCWSTR::null(), Some((&mut status as *mut u32).cast()), &mut size, std::ptr::null_mut())
                .map_err(|e| format!("status: {e}"))?;
            let mut body = Vec::new();
            let mut chunk = [0u8; 16 * 1024];
            loop {
                let mut read = 0u32;
                WinHttpReadData(request.0, chunk.as_mut_ptr().cast(), chunk.len() as u32, &mut read).map_err(|e| format!("read: {e}"))?;
                if read == 0 { break; }
                body.extend_from_slice(&chunk[..read as usize]);
                if body.len() > 4 << 20 { return Err("the answer is too large".into()); }
            }
            match status {
                200 => String::from_utf8(body).map_err(|_| "the answer is not UTF-8".into()),
                403 | 429 => Err(format!("GitHub rate limit (HTTP {status})")),
                _ => Err(format!("GitHub answered HTTP {status}")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }

    #[test]
    fn the_answer_is_read_like_v_plus_reads_it() {
        let release = |tag: &str, draft: bool, pre: bool| format!(r#"{{"tag_name":"{tag}","draft":{draft},"prerelease":{pre},"html_url":"https://github.com/RamazanKara/nightjar/releases/tag/{tag}"}}"#);
        assert_eq!(offer(&release("v4.1.0", false, false), "4.0.0"), Ok(Some(Release {
            version: "4.1.0".into(),
            url: "https://github.com/RamazanKara/nightjar/releases/tag/v4.1.0".into(),
        })));
        assert_eq!(offer(&release("v4.1.0", true, false), "4.0.0"), Ok(None), "a draft");
        assert_eq!(offer(&release("v4.1.0", false, true), "4.0.0"), Ok(None), "a pre-release");
        assert_eq!(offer(&release("v4.0.0", false, false), "4.0.0"), Ok(None), "this version");
        assert!(offer("{\"message\":\"Not Found\"}", "4.0.0").is_err());
        assert!(offer("<html>", "4.0.0").is_err());
        assert!(offer(r#"{"tag_name":"v9.0.0","html_url":"https://evil.example/"}"#, "4.0.0").is_err(), "only GitHub pages open");
    }

    /// The real request: `cargo test -p nightjar-desktop asks_github -- --ignored`.
    #[cfg(windows)]
    #[test]
    #[ignore = "asks GitHub over the network"]
    fn asks_github() {
        let json = winhttp::get(HOST, PATH, "Nightjar/test (Windows)").unwrap();
        assert!(offer(&json, "0.0.1").unwrap().is_some(), "{json}");
        assert_eq!(offer(&json, "999.0.0").unwrap(), None);
    }

    #[test]
    fn off_is_off_and_no_request_is_made() {
        let updates = Updates::start(false, std::sync::Arc::new(|| {}));
        assert!(updates.pending.is_none() && updates.found.is_none());
    }
}
