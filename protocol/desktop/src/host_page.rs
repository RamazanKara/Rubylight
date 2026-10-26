//! A PC's details page: the host software, its web UI, and the remote access check of Android's
//! RemoteSetupActivity. The check runs on a worker thread and ends early when it is cancelled or
//! its page is left.

use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

use nightjar_core::http::{
    Error, Family, Feature, HostApiError, HostCapabilities, HostClient, NetworkPath, RemoteCheck, RemoteOutcome,
    DEFAULT_HTTP_PORT,
};

use crate::hosts::{Host, Reach, Waker};
use crate::strings::{en, tr, trf};

/// The host software as Android's HostCapabilities.familyLabel names it.
pub fn software_label(caps: &HostCapabilities) -> String {
    let label = match caps.family() {
        Family::Rubylight => return trf("Rubylight {}", &[&caps.host_version().unwrap_or_default()]),
        Family::Vibepollo => en("Vibepollo-compatible host"),
        Family::Apollo => en("Apollo-compatible host"),
        Family::Sunshine => en("Sunshine-compatible host"),
        Family::GameStream => en("NVIDIA GameStream"),
        Family::Unknown => en("Moonlight-compatible host"),
    };
    tr(label).to_owned()
}

/// Where the PC's web UI is, when the PC answers and offers one; `Err` says why its saved
/// address makes no link a browser opens.
pub fn web_ui_url(host: &Host) -> Option<Result<String, String>> {
    let info = host.info.as_ref().filter(|i| host.reach == Reach::Online && i.capabilities.has(Feature::WebUi))?;
    let https_port = if info.https_port != 0 { info.https_port } else { host.record.https_port };
    let http_port = if host.record.http_port == 0 { DEFAULT_HTTP_PORT } else { host.record.http_port };
    web_ui_at(&host.record.address, http_port, https_port)
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

const FAILED: &str = en("The paired PC could not be verified. Check that it is awake, the host and VPN are running, and the saved address and streaming ports are correct. If pairing changed, pair again at home. This test cannot distinguish a sleeping PC from a blocked route.");

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
                en("The paired PC responded, but this device cannot view streams. Enable viewing or launching applications for this device in your host's settings or web UI."),
                None,
            );
        }
        RemoteOutcome::Unreachable(e) => return Report::failed(FAILED, Some(e.to_string())),
    };
    let unknown = (
        en("Direct or relayed path: unavailable. This host, VPN or connection does not expose a current route classification. Check your VPN's status; low ping alone does not prove a direct path."),
        en("Unavailable"),
    );
    let ((route, name), error) = match path {
        Ok(NetworkPath::Direct) => (
            (en("Tailscale reports a direct path for this connection. Direct paths usually provide the best throughput and latency."), en("Direct")),
            None,
        ),
        Ok(NetworkPath::Relay) => (
            (
                en("Tailscale reports a DERP relay. It provides connectivity, but streaming speed may be limited. Check Tailscale's connectivity guidance or configure a suitable peer relay."),
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
    /// The PC the last check was for and where it stands; cleared when its page is left.
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

    /// Takes in a finished check. `page` is the PC whose details page is open: leaving it
    /// cancels a running check and forgets the last result.
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
pub(crate) mod tests {
    use super::*;
    use crate::config::HostRecord;
    use crate::hosts::Hosts;
    use nightjar_core::http::{ConnectionTestStats, ServerInfo};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Condvar, Mutex};
    use std::time::Duration;

    pub(crate) fn info(state: &str, extra: &str) -> ServerInfo {
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\"><hostname>PC</hostname><uniqueid>uid</uniqueid>\
             <HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>1</PairStatus><currentgame>0</currentgame>\
             <state>{state}</state>{extra}</root>"
        );
        ServerInfo::parse(&xml, 47989).expect("the answer parses")
    }

    fn host(info: Option<ServerInfo>, reach: Reach, address: &str) -> Host {
        let record = HostRecord { unique_id: "uid".into(), name: "PC".into(), address: address.into(), ..Default::default() };
        let mut hosts = Hosts::without_discovery(vec![record], Arc::new(|| {}));
        let mut host = hosts.list.remove(0);
        host.reach = reach;
        host.info = info;
        host
    }

    #[test]
    fn the_software_label_names_the_family_and_rubylights_version() {
        let label = |state: &str, extra: &str| software_label(&info(state, extra).capabilities);
        assert_eq!(label("SUNSHINE_SERVER_FREE", "<RustHostVersion>3.0.0</RustHostVersion>"), "Rubylight 3.0.0");
        assert_eq!(label("SUNSHINE_SERVER_FREE", "<Permission>1</Permission>"), "Apollo-compatible host");
        assert_eq!(label("MJOLNIR_STATE_FREE", ""), "NVIDIA GameStream");
        assert_eq!(label("SUNSHINE_SERVER_FREE", ""), "Moonlight-compatible host");
        assert_eq!(software_label(&HostCapabilities::UNKNOWN), "Moonlight-compatible host");
    }

    #[test]
    fn the_web_ui_is_offered_only_by_a_pc_that_answers_and_has_one() {
        let rubylight = || Some(info("SUNSHINE_SERVER_FREE", "<RustHostVersion>3.0.0</RustHostVersion>"));
        assert_eq!(web_ui_url(&host(rubylight(), Reach::Online, "192.0.2.7")), Some(Ok("https://192.0.2.7:47990".into())));
        assert_eq!(web_ui_url(&host(rubylight(), Reach::Online, "fd7a::1")), Some(Ok("https://[fd7a::1]:47990".into())));
        assert_eq!(web_ui_url(&host(rubylight(), Reach::Offline, "192.0.2.7")), None, "an offline PC's web UI is not there");
        assert_eq!(web_ui_url(&host(Some(info("MJOLNIR_STATE_FREE", "")), Reach::Online, "192.0.2.7")), None, "GameStream has none");
        assert_eq!(web_ui_url(&host(Some(info("SUNSHINE_SERVER_FREE", "")), Reach::Online, "192.0.2.7")), None, "unknown host: not offered");
        assert_eq!(web_ui_url(&host(None, Reach::Online, "192.0.2.7")), None);
        assert_eq!(web_ui_at("pc", 47989, 0), Some(Ok("https://pc:47990".into())), "no HTTPS port: the HTTP port + 1");
        assert_eq!(web_ui_at("pc", 47989, 65533), None, "past the last port");
    }

    #[test]
    fn the_web_ui_link_is_made_only_from_an_ip_address_or_a_host_name() {
        let url = |address: &str| web_ui_at(address, 47989, 47984).expect("a port");
        assert_eq!(url("2001:db8::1"), Ok("https://[2001:db8::1]:47990".into()));
        assert_eq!(url("[2001:db8::1]"), Ok("https://[2001:db8::1]:47990".into()), "a bracketed IPv6 address");
        assert_eq!(url("::ffff:192.168.1.5"), Ok("https://192.168.1.5:47990".into()), "a mapped IPv4 address is that address");
        assert_eq!(url("gaming-pc.tail1234.ts.net"), Ok("https://gaming-pc.tail1234.ts.net:47990".into()));
        assert_eq!(url("GAMING_PC"), Ok("https://GAMING_PC:47990".into()), "a NetBIOS-style name");
        // A zone ID is not something a browser opens: no button, and the page says why.
        for scoped in ["fe80::1%12", "[fe80::1%12]", "fe80::1%eth0"] {
            let reason = url(scoped).expect_err(scoped);
            assert!(reason.contains("zone ID") && reason.contains(scoped), "{reason}");
        }
        // Anything else is refused, naming the address, never pasted into a link.
        for bad in ["pc#frag", "pc/path", "user@pc", "pc:47989", "[pc]", "-pc", "pc-", "a..b", "192.168.1.300", "pc name", "", "Büro"] {
            let reason = url(bad).expect_err(bad);
            assert!(reason.contains("not an IP address or host name") && reason.contains(bad), "{bad}: {reason}");
        }
        assert!(url(&"a".repeat(64)).is_err(), "a label is at most 63 characters");
    }

    fn stats(samples: &[f64]) -> ConnectionTestStats {
        let mut stats = ConnectionTestStats::default();
        samples.iter().for_each(|ms| stats.add(*ms));
        stats
    }

    fn verified(path: Result<NetworkPath, HostApiError>, samples: &[f64]) -> Result<RemoteCheck, Error> {
        let server_info = Box::new(info("SUNSHINE_SERVER_FREE", ""));
        Ok(RemoteCheck { outcome: RemoteOutcome::Verified { server_info, path }, stats: stats(samples) })
    }

    #[test]
    fn a_verified_pc_reports_its_route_and_measurements() {
        let r = report(verified(Ok(NetworkPath::Relay), &[10.0, 14.0, f64::NAN, 12.0, 12.0]));
        assert!(r.verified && r.summary.starts_with("The paired PC responded"));
        assert_eq!(r.path, Some("Relay (DERP)"));
        assert!(r.route.unwrap().contains("DERP relay"));
        assert_eq!((r.latency_ms, r.jitter_ms, r.loss_percent), (Some(12.0), Some(2.0), Some(20.0)));
        assert_eq!(r.error, None);
        // A host without the route says nothing more; another failure is shown as it came.
        let unavailable = HostApiError::Unavailable { feature: Feature::NetworkPath, availability: nightjar_core::http::Availability::Unknown };
        let r = report(verified(Err(unavailable), &[5.0]));
        assert_eq!((r.path, r.error), (Some("Unavailable"), None));
        let r = report(verified(Err(HostApiError::Host(Error::Malformed("Invalid route response".into()))), &[5.0]));
        assert_eq!(r.path, Some("Unavailable"));
        assert!(r.error.unwrap().contains("Invalid route response"));
    }

    #[test]
    fn failures_keep_the_error_as_it_came() {
        let r = report(Err(Error::NotPaired));
        assert!(r.summary.starts_with("Pair this PC locally first") && !r.verified);
        assert_eq!(r.error, Some(Error::NotPaired.to_string()));
        let gone = Error::Io(std::io::Error::new(std::io::ErrorKind::TimedOut, "connection timed out"));
        let r = report(Ok(RemoteCheck { outcome: RemoteOutcome::Unreachable(gone), stats: stats(&[f64::NAN; 5]) }));
        assert_eq!((r.summary, r.error.as_deref(), r.latency_ms), (FAILED, Some("connection timed out"), None));
        let r = report(Ok(RemoteCheck { outcome: RemoteOutcome::WrongHost, stats: stats(&[3.0]) }));
        assert_eq!((r.summary, r.error.as_deref()), (FAILED, Some("A different PC answered at this address.")));
        let r = report(Ok(RemoteCheck { outcome: RemoteOutcome::PairRevoked, stats: stats(&[3.0]) }));
        assert!(r.summary.contains("no longer paired"));
        let r = report(Err(Error::Status { code: 503, message: "Service Unavailable".into() }));
        assert_eq!(r.summary, FAILED);
        assert!(r.error.unwrap().contains("Service Unavailable"));
    }

    /// A check that answers once it is let go, or ends with `Cancelled` when cancelled first.
    #[derive(Default)]
    pub(crate) struct FakeProbe {
        pub answer: Mutex<Option<Result<RemoteCheck, Error>>>,
        pub cancels: AtomicUsize,
        pub asked: Mutex<Option<String>>,
        released: Mutex<bool>,
        gate: Condvar,
    }

    impl FakeProbe {
        pub fn answering(answer: Result<RemoteCheck, Error>) -> Arc<Self> {
            Arc::new(Self { answer: Mutex::new(Some(answer)), ..Default::default() })
        }

        pub fn release(&self) {
            *self.released.lock().unwrap() = true;
            self.gate.notify_all();
        }
    }

    impl RemoteProbe for FakeProbe {
        fn check(&self, unique_id: &str) -> Result<RemoteCheck, Error> {
            *self.asked.lock().unwrap() = Some(unique_id.to_owned());
            let released = self.gate.wait_while(self.released.lock().unwrap(), |r| !*r).unwrap();
            drop(released);
            if self.cancels.load(Ordering::SeqCst) > 0 {
                return Err(Error::Cancelled);
            }
            self.answer.lock().unwrap().take().expect("one answer")
        }

        fn cancel(&self) {
            self.cancels.fetch_add(1, Ordering::SeqCst);
            self.release();
        }
    }

    /// Polls until the check is no longer running.
    pub(crate) fn finish(remote: &mut RemoteAccess, page: Option<u64>) {
        for _ in 0..500 {
            remote.poll(page);
            if !remote.running() {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("the check did not finish");
    }

    #[test]
    fn a_check_runs_off_the_ui_thread_and_reports_back() {
        let probe = FakeProbe::answering(verified(Ok(NetworkPath::Direct), &[8.0, 9.0]));
        let woken = Arc::new(AtomicUsize::new(0));
        let wake = woken.clone();
        let mut remote = RemoteAccess::default();
        remote.start(4, probe.clone(), "uid".into(), Arc::new(move || {
            wake.fetch_add(1, Ordering::SeqCst);
        }));
        remote.poll(Some(4));
        assert_eq!(remote.state_for(4), Some(&CheckState::Running), "the UI thread did not wait for it");
        probe.release();
        finish(&mut remote, Some(4));
        assert_eq!(woken.load(Ordering::SeqCst), 1, "the UI is woken once");
        assert_eq!(probe.asked.lock().unwrap().as_deref(), Some("uid"));
        let Some(CheckState::Done(r)) = remote.state_for(4) else { panic!("{:?}", remote.state) };
        assert_eq!((r.path, r.latency_ms), (Some("Direct"), Some(8.5)));
        assert_eq!(remote.state_for(5), None, "another PC's page shows nothing");
    }

    #[test]
    fn cancel_ends_the_requests_and_a_late_answer_is_dropped() {
        let probe = FakeProbe::answering(verified(Ok(NetworkPath::Direct), &[8.0]));
        let mut remote = RemoteAccess::default();
        remote.start(4, probe.clone(), "uid".into(), Arc::new(|| {}));
        remote.cancel();
        assert_eq!(probe.cancels.load(Ordering::SeqCst), 1, "the client's requests were cancelled");
        assert!(!remote.running());
        assert_eq!(remote.state_for(4), Some(&CheckState::Cancelled));
        std::thread::sleep(Duration::from_millis(20));
        remote.poll(Some(4));
        assert_eq!(remote.state_for(4), Some(&CheckState::Cancelled), "nothing replaces it");
    }

    #[test]
    fn a_cancelled_client_reads_as_cancelled_and_leaving_the_page_cancels() {
        let probe = FakeProbe::answering(Err(Error::Cancelled));
        let mut remote = RemoteAccess::default();
        remote.start(4, probe.clone(), "uid".into(), Arc::new(|| {}));
        probe.release();
        finish(&mut remote, Some(4));
        assert_eq!(remote.state_for(4), Some(&CheckState::Cancelled));

        let probe = FakeProbe::answering(verified(Ok(NetworkPath::Direct), &[8.0]));
        remote.start(4, probe.clone(), "uid".into(), Arc::new(|| {}));
        remote.poll(None);
        assert!(!remote.running() && remote.state.is_none(), "the page was left");
        assert_eq!(probe.cancels.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_check_that_could_not_start_says_why() {
        let mut remote = RemoteAccess::default();
        remote.fail(4, "Nightjar has no client identity yet.".into());
        let Some(CheckState::Done(r)) = remote.state_for(4) else { panic!() };
        assert_eq!((r.summary, r.error.as_deref()), (FAILED, Some("Nightjar has no client identity yet.")));
    }
}
