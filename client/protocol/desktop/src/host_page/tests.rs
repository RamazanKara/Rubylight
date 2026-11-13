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
    assert_eq!(label("SUNSHINE_SERVER_FREE", "<Permission>1</Permission>"), "Apollo-compatible streaming app");
    assert_eq!(label("MJOLNIR_STATE_FREE", ""), "NVIDIA GameStream");
    assert_eq!(label("SUNSHINE_SERVER_FREE", ""), "Moonlight-compatible streaming app");
    assert_eq!(software_label(&HostCapabilities::UNKNOWN), "Moonlight-compatible streaming app");
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
fn the_check_command_runs_one_check_and_says_why_one_cannot_start() {
    let pc = host(None, Reach::Online, "192.0.2.7");
    let mut remote = RemoteAccess::default();
    // A check is running: a second Test does nothing to it.
    let probe = FakeProbe::answering(verified(Ok(NetworkPath::Direct), &[8.0]));
    remote.start(pc.id, probe.clone(), "uid".into(), Arc::new(|| {}));
    start_check(&mut remote, None, &pc, Arc::new(|| {}));
    assert!(remote.running() && remote.state_for(pc.id) == Some(&CheckState::Running));
    assert_eq!(probe.cancels.load(Ordering::SeqCst), 0);
    probe.release();
    finish(&mut remote, Some(pc.id));

    // No identity yet: the page says so.
    start_check(&mut remote, None, &pc, Arc::new(|| {}));
    let Some(CheckState::Done(r)) = remote.state_for(pc.id) else { panic!("{:?}", remote.state) };
    assert_eq!((r.summary, r.error.as_deref()), (FAILED, Some("Nightjar has no client identity yet. Restart Nightjar and try again.")));

    // A client that cannot be made: its error, word for word.
    let identity = Arc::new(ClientIdentity::generate().expect("an identity"));
    let mut broken = host(None, Reach::Online, "192.0.2.7");
    broken.record.server_certificate = Some("not a certificate".into());
    let Err(why) = client_for(&identity, &broken.record, &broken.record.address) else { panic!("the client was made") };
    start_check(&mut remote, Some(&identity), &broken, Arc::new(|| {}));
    let Some(CheckState::Done(r)) = remote.state_for(broken.id) else { panic!("{:?}", remote.state) };
    assert_eq!((r.summary, r.error.as_deref()), (FAILED, Some(why.as_str())));

    // A client: the real check runs, here against a PC never paired, so no request leaves.
    start_check(&mut remote, Some(&identity), &pc, Arc::new(|| {}));
    assert!(remote.running());
    finish(&mut remote, Some(pc.id));
    let Some(CheckState::Done(r)) = remote.state_for(pc.id) else { panic!("{:?}", remote.state) };
    assert!(r.summary.starts_with("Pair this PC locally first") && !r.verified, "{r:?}");
}

#[test]
fn open_web_ui_opens_the_link_or_says_where_it_is_and_why_not() {
    let url = "https://192.0.2.7:47990";
    let mut opened = None;
    assert_eq!(open_web_ui(url, |u| { opened = Some(u.to_owned()); Ok(()) }), None, "nothing to say");
    assert_eq!(opened.as_deref(), Some(url));
    let (title, body) = open_web_ui(url, |_| Err("No handler for https".into())).expect("a message");
    assert!(title.starts_with("No browser could open the PC’s web UI."), "{title}");
    assert_eq!(body, "https://192.0.2.7:47990\n\nNo handler for https", "the link to copy, then the error");
}

/// How to connect opens the pairing guide Android's help opens, or says where it is.
#[test]
fn open_guide_opens_the_pairing_page_or_says_where_it_is() {
    let mut opened = None;
    assert_eq!(open_guide("pairing.md", |u| { opened = Some(u.to_owned()); Ok(()) }), None, "nothing to say");
    assert_eq!(opened.as_deref(), Some("https://github.com/RamazanKara/nightjar/blob/main/docs/pairing.md"));
    let (title, body) = open_guide("pairing.md", |_| Err("No handler for https".into())).expect("a message");
    assert!(title.starts_with("No browser could open the guide."), "{title}");
    assert_eq!(body, "https://github.com/RamazanKara/nightjar/blob/main/docs/pairing.md\n\nNo handler for https");
}

#[test]
fn a_check_that_could_not_start_says_why() {
    let mut remote = RemoteAccess::default();
    remote.fail(4, "Nightjar has no client identity yet.".into());
    let Some(CheckState::Done(r)) = remote.state_for(4) else { panic!() };
    assert_eq!((r.summary, r.error.as_deref()), (FAILED, Some("Nightjar has no client identity yet.")));
}
