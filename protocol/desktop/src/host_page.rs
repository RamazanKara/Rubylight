//! A PC's details page: the host software, its web UI, and the remote access check of Android's
//! RemoteSetupActivity. The check runs on a worker thread and ends early when it is cancelled or
//! its page is left.

use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

use nightjar_core::http::{
    ClientIdentity, Error, Family, Feature, HostApiError, HostCapabilities, HostClient, NetworkPath, RemoteCheck,
    RemoteOutcome, DEFAULT_HTTP_PORT,
};

use crate::hosts::{client_for, Host, Reach, Waker};
use crate::strings::{en, tr, trf};

/// The host software as Android's HostCapabilities.familyLabel names it.
pub fn software_label(caps: &HostCapabilities) -> String {
    let label = match caps.family() {
        Family::Rubylight => return trf("Rubylight {}", &[&caps.host_version().unwrap_or_default()]),
        Family::Vibepollo => en("Vibepollo-compatible streaming app"),
        Family::Apollo => en("Apollo-compatible streaming app"),
        Family::Sunshine => en("Sunshine-compatible streaming app"),
        Family::GameStream => en("NVIDIA GameStream"),
        Family::Unknown => en("Moonlight-compatible streaming app"),
    };
    tr(label).to_owned()
}

/// Where the PC's web UI is, when the PC answers and offers one; `Err` says why its saved
/// address makes no link a browser opens.
pub fn web_ui_url(host: &Host) -> Option<Result<String, String>> {
    let info = host.info.as_ref().filter(|i| host.reach == Reach::Online && i.capabilities.has(Feature::WebUi))?;
    let https_port = if info.https_port != 0 { info.https_port } else { host.record.https_port };
    let http_port = if host.record.http_port == 0 { DEFAULT_HTTP_PORT } else { host.record.http_port };
    web_ui_at(host.connect_address(), http_port, https_port)
}

/// Android's host details: the HTTPS port + 6 (47990 by default), else the HTTP port + 1.
fn web_ui_at(address: &str, http_port: u16, https_port: u16) -> Option<Result<String, String>> {
    let port = if https_port != 0 { https_port.checked_add(6)? } else { http_port.checked_add(1)? };
    Some(url_host(address).map(|host| format!("https://{host}:{port}")))
}

/// The saved address as a URL's host: an IP address or a host name, nothing else.
fn url_host(address: &str) -> Result<String, String> {
    let (bare, bracketed) = match address.strip_prefix('[').and_then(|a| a.strip_suffix(']')) {
        Some(inner) => (inner, true),
        None => (address, false),
    };
    if bare.split_once('%').is_some_and(|(ip, _)| ip.parse::<Ipv6Addr>().is_ok()) {
        return Err(trf("Browsers cannot open an address with a zone ID ({}). Open the web UI from another address of this PC.", &[&address]));
    }
    if let Ok(ip) = bare.parse::<Ipv6Addr>() {
        // A mapped IPv4 address is the IPv4 address.
        return Ok(ip.to_ipv4_mapped().map_or_else(|| format!("[{ip}]"), |v4| v4.to_string()));
    }
    if !bracketed {
        if let Ok(ip) = bare.parse::<Ipv4Addr>() {
            return Ok(ip.to_string());
        }
        if is_host_name(bare) {
            return Ok(bare.to_owned());
        }
    }
    Err(trf("The saved address {} is not an IP address or host name, so it makes no web UI link.", &[&address]))
}

/// A DNS name a browser takes as it is: ASCII labels of letters, digits, '-' and '_'. A last
/// label of digits only would be read as a broken IPv4 address.
fn is_host_name(name: &str) -> bool {
    let name = name.strip_suffix('.').unwrap_or(name);
    let label = |l: &str| {
        (1..=63).contains(&l.len())
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    };
    name.len() <= 253
        && name.split('.').all(label)
        && !name.rsplit('.').next().is_some_and(|last| last.bytes().all(|b| b.is_ascii_digit()))
}

/// Command::RemoteCheck: checks `host` unless a check is running. One that cannot start says
/// why on the page.
pub fn start_check(remote: &mut RemoteAccess, identity: Option<&Arc<ClientIdentity>>, host: &Host, waker: Waker) {
    if remote.running() {
        return;
    }
    let client = match identity {
        Some(identity) => client_for(identity, &host.record, host.connect_address()),
        None => Err(tr("Nightjar has no client identity yet. Restart Nightjar and try again.").to_owned()),
    };
    match client {
        Ok(client) => remote.start(host.id, Arc::new(client), host.record.unique_id.clone(), waker),
        Err(error) => remote.fail(host.id, error),
    }
}

/// Command::OpenWebUi: opens `url` with `open`. When it could not, the message to show: the
/// address, so it can be opened elsewhere, and the error.
pub fn open_web_ui(url: &str, open: impl FnOnce(&str) -> Result<(), String>) -> Option<(String, String)> {
    open(url).err().map(|e| {
        (tr("No browser could open the PC’s web UI. Copy its address and open it on another device.").to_owned(), format!("{url}\n\n{e}"))
    })
}

/// Command::OpenGuide: opens the guide `page` with `open`. When it could not, the message to show:
/// the guide's address, so it can be opened elsewhere, and the error.
pub fn open_guide(page: &str, open: impl FnOnce(&str) -> Result<(), String>) -> Option<(String, String)> {
    let url = format!("{}{page}", crate::ui::GUIDES);
    open(&url).err().map(|e| (tr("No browser could open the guide. Its address is below, to open on another device.").to_owned(), format!("{url}

{e}")))
}

/// The remote check against one PC; `HostClient` in the app, a fake in tests.
pub trait RemoteProbe: Send + Sync {
    /// Blocks until the check is done or cancelled.
    fn check(&self, unique_id: &str) -> Result<RemoteCheck, Error>;
    /// Ends a running `check` with `Error::Cancelled`.
    fn cancel(&self);
}

impl RemoteProbe for HostClient {
    fn check(&self, unique_id: &str) -> Result<RemoteCheck, Error> {
        self.remote_check(unique_id)
    }

    fn cancel(&self) {
        self.cancel_pending_requests();
    }
}

/// A check on its worker thread. Dropping it cancels the requests in flight.
struct Check {
    host: u64,
    result: Receiver<Result<RemoteCheck, Error>>,
    probe: Arc<dyn RemoteProbe>,
}

impl Drop for Check {
    fn drop(&mut self) {
        self.probe.cancel();
    }
}

/// What the check found, in the words the page shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub summary: &'static str,
    /// What the route means for streaming; only for a PC that was verified.
    pub route: Option<&'static str>,
    /// The route's name; only for a PC that was verified.
    pub path: Option<&'static str>,
    /// Round trips, jitter and loss; only for a PC that was verified.
    pub latency_ms: Option<f64>,
    pub jitter_ms: Option<f64>,
    pub loss_percent: Option<f64>,
    /// The error behind the result, as the client or the host gave it.
    pub error: Option<String>,
    pub verified: bool,
}

impl Report {
    fn failed(summary: &'static str, error: Option<String>) -> Self {
        Self {
            summary,
            route: None,
            path: None,
            latency_ms: None,
            jitter_ms: None,
            loss_percent: None,
            error,
            verified: false,
        }
    }
}

const FAILED: &str = en("The paired PC could not be verified. Check that it is awake, its streaming app and VPN are running, and the saved address and streaming ports are correct. If pairing changed, pair again at home. This test cannot distinguish a sleeping PC from a blocked route.");

/// RemoteSetupActivity's result texts for one check.
pub fn report(result: Result<RemoteCheck, Error>) -> Report {
    let check = match result {
        Ok(check) => check,
        Err(e @ (Error::NotPaired | Error::InvalidArgument(_))) => {
            return Report::failed(
                en("Pair this PC locally first. Its saved certificate and HTTPS port are required to verify that the remote address reaches your PC."),
                Some(e.to_string()),
            );
        }
        Err(e) => return Report::failed(FAILED, Some(e.to_string())),
    };
    let path = match check.outcome {
        RemoteOutcome::Verified { path, .. } => path,
        RemoteOutcome::WrongHost => return Report::failed(FAILED, Some(tr("A different PC answered at this address.").to_owned())),
        RemoteOutcome::PairRevoked => {
            return Report::failed(en("The PC responded, but this device is no longer paired. Pair again at home before testing remote play."), None);
        }
        RemoteOutcome::PermissionDenied => {
            return Report::failed(
                en("The paired PC responded, but this device cannot view streams. Enable viewing or launching applications for this device in the PC’s settings or web UI."),
                None,
            );
        }
        RemoteOutcome::Unreachable(e) => return Report::failed(FAILED, Some(e.to_string())),
    };
    let unknown = (
        en("Direct or relayed path: unavailable. This PC, VPN or connection does not expose a current route classification. Check your VPN’s status; low ping alone does not prove a direct path."),
        en("Unavailable"),
    );
    let ((route, name), error) = match path {
        Ok(NetworkPath::Direct) => (
            (en("Tailscale reports a direct path for this connection. Direct paths usually provide the best throughput and latency."), en("Direct")),
            None,
        ),
        Ok(NetworkPath::Relay) => (
            (
                en("Tailscale reports a DERP relay. It provides connectivity, but streaming speed may be limited. Check Tailscale’s connectivity guidance or configure a suitable peer relay."),
                en("Relay (DERP)"),
            ),
            None,
        ),
        Ok(NetworkPath::PeerRelay) => (
            (en("Tailscale reports a peer relay. Performance depends on that relay and both network paths; compare a real stream."), en("Peer relay")),
            None,
        ),
        // A host without the route reports nothing to show; any other failure is said as it came.
        Ok(NetworkPath::Unknown) | Err(HostApiError::Unavailable { .. }) => (unknown, None),
        Err(e) => (unknown, Some(e.to_string())),
    };
    Report {
        summary: en("The paired PC responded on this network. Next, try a normal stream at a modest bitrate; HTTPS reachability does not verify video/audio UDP ports or sustained streaming performance."),
        route: Some(route),
        path: Some(name),
        latency_ms: check.stats.latency_ms(),
        jitter_ms: check.stats.jitter_ms(),
        loss_percent: check.stats.loss_percent(),
        error,
        verified: true,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum CheckState {
    Running,
    Done(Report),
    Cancelled,
}

/// The remote check of the details page: at most one at a time, for the PC whose page is open.
#[derive(Default)]
pub struct RemoteAccess {
    check: Option<Check>,
    /// The PC the last check was for and where it stands; cleared when its page is left (not
    /// when Settings opens over it).
    pub state: Option<(u64, CheckState)>,
}

impl RemoteAccess {
    /// Starts checking that `unique_id` answers through `probe`; `waker` runs when it is done.
    pub fn start(&mut self, host: u64, probe: Arc<dyn RemoteProbe>, unique_id: String, waker: Waker) {
        self.check = None;
        let (tx, result) = channel();
        let worker = probe.clone();
        let spawned = std::thread::Builder::new().name("remote check".into()).spawn(move || {
            let _ = tx.send(worker.check(&unique_id));
            waker();
        });
        match spawned {
            Ok(_) => {
                self.check = Some(Check { host, result, probe });
                self.state = Some((host, CheckState::Running));
            }
            Err(e) => self.fail(host, e.to_string()),
        }
    }

    /// The check could not start: says why, as a failed check.
    pub fn fail(&mut self, host: u64, error: String) {
        self.check = None;
        self.state = Some((host, CheckState::Done(Report::failed(FAILED, Some(error)))));
    }

    pub fn cancel(&mut self) {
        if let Some(check) = self.check.take() {
            self.state = Some((check.host, CheckState::Cancelled));
        }
    }

    pub fn running(&self) -> bool {
        self.check.is_some()
    }

    /// The state for `host`'s page.
    pub fn state_for(&self, host: u64) -> Option<&CheckState> {
        self.state.as_ref().filter(|(h, _)| *h == host).map(|(_, s)| s)
    }

    /// Takes in a finished check. `page` is the PC whose details page is open or on the way
    /// back: leaving it cancels a running check and forgets the last result.
    pub fn poll(&mut self, page: Option<u64>) {
        if self.state.as_ref().is_some_and(|(host, _)| page != Some(*host)) {
            self.check = None;
            self.state = None;
            return;
        }
        let Some(check) = &self.check else { return };
        let host = check.host;
        let state = match check.result.try_recv() {
            Err(TryRecvError::Empty) => return,
            Ok(Err(Error::Cancelled)) => CheckState::Cancelled,
            Ok(result) => CheckState::Done(report(result)),
            Err(TryRecvError::Disconnected) => {
                CheckState::Done(Report::failed(FAILED, Some(tr("The check stopped without a result.").to_owned())))
            }
        };
        self.check = None;
        self.state = Some((host, state));
    }
}

#[cfg(test)]
pub(crate) mod tests;
