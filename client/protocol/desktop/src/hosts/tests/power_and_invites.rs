//! Unit tests for invites, sleep/power token handshakes, host updates and certificate mismatch handling.

use super::classic_and_links::{INFO, PEM};
use super::*;


const TOKEN_A: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const TOKEN_B: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

/// A reply as the PC sends it: supported, allowed and available, then the token.
fn power_reply(status: u16, message: &str, flags: [bool; 3], token: Option<&str>) -> PowerReply {
    PowerReply {
        status,
        message: message.into(),
        version: 1,
        sleep_supported: flags[0],
        sleep_allowed: flags[1],
        sleep_available: flags[2],
        active_session_policy: "idle-only".into(),
        token: token.map(Into::into),
        token_lifetime_seconds: 30,
        accepted: status == 202,
    }
}

/// A PC that hands out `tokens` in turn and answers each sleep request with `sleep`.
struct FakePc {
    tokens: std::cell::RefCell<Vec<&'static str>>,
    sleep: fn() -> Result<PowerReply, PowerError>,
    calls: std::cell::RefCell<Vec<String>>,
}

impl FakePc {
    fn new(tokens: &[&'static str], sleep: fn() -> Result<PowerReply, PowerError>) -> Self {
        Self { tokens: std::cell::RefCell::new(tokens.to_vec()), sleep, calls: Default::default() }
    }
    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }
}

impl PowerHost for FakePc {
    fn power_capabilities(&self) -> Result<PowerReply, PowerError> {
        self.calls.borrow_mut().push("GET".into());
        let mut tokens = self.tokens.borrow_mut();
        let token = (!tokens.is_empty()).then(|| tokens.remove(0));
        Ok(power_reply(200, "", [true, true, token.is_some()], token))
    }
    fn sleep(&self, token: &str) -> Result<PowerReply, PowerError> {
        self.calls.borrow_mut().push(format!("POST {token}"));
        (self.sleep)()
    }
}

fn accepted() -> Result<PowerReply, PowerError> {
    Ok(power_reply(
        202,
        "Sleep request accepted; native completion is not confirmed",
        [true, true, false],
        None,
    ))
}

fn invite_link(name: &str) -> rubylight_protocol::invite_uri::InviteUri {
    rubylight_protocol::invite_uri::InviteUri {
        secret: [3; 32],
        unique_id: "HOST-1".into(),
        cert_sha256: [7; 32],
        name: name.into(),
        expires: 4_000_000_000,
        permission: "controller".into(),
        ports: rubylight_protocol::invite_uri::InvitePorts {
            http: 47989,
            https: 47984,
            ..Default::default()
        },
        session: None,
        endpoints: Vec::new(),
        nat: String::new(),
        ipv6: Vec::new(),
        external_ipv4: None,
        mapped_ports: false,
    }
}

#[test]
fn add_pc_reads_invites_and_refuses_a_broken_one_as_an_invite() {
    let invite = invite_link("Den");
    let pasted = format!(
        "  {}
",
        invite.to_uri()
    );
    assert_eq!(
        parse_entry(&pasted),
        Ok(Entry::Invite(Box::new(invite.clone())))
    );
    let broken = invite.to_uri().replace("&perm=controller", "&perm=admin");
    assert_eq!(parse_entry(&broken), Err(INVITE_INVALID));
    assert_eq!(INVITE_INVALID, nightjar_core::invite::NOT_AN_INVITE);
}

#[test]
fn the_engines_invite_errors_are_the_ones_translated() {
    for known in [
        nightjar_core::invite::NOT_AN_INVITE,
        nightjar_core::invite::EXPIRED,
        nightjar_core::invite::REFUSED,
        nightjar_core::invite::INTERNET_OFF,
    ] {
        crate::strings::with_lang(crate::strings::Lang::De, || {
            assert_ne!(invite_error(known), known, "{known} has a German text");
        });
    }
    assert_eq!(
        invite_error("the PC did not answer"),
        "the PC did not answer"
    );
}

const OTHER_PEM: &str = "-----BEGIN CERTIFICATE-----
BAUG
-----END CERTIFICATE-----
";

fn redeemed_as(unique_id: &str, pem: &str) -> nightjar_core::invite::Redeemed {
    nightjar_core::invite::Redeemed {
        unique_id: unique_id.into(),
        name: "Den".into(),
        address: "203.0.113.9:47989".parse().unwrap(),
        path: nightjar_core::internet::InternetPath::MappedIpv4,
        https_port: 47984,
        server_certificate_pem: pem.into(),
        reply: "the invite's".into(),
    }
}

// A link anyone can write names the ID of a PC the player already paired, with the certificate of
// the PC it really leads to: the paired PC keeps its certificate and its internet details.
#[test]
fn an_invite_never_replaces_the_certificate_of_a_pc_paired_with_another_one() {
    let mut known = paired_record("HOST-1");
    known.server_certificate = Some(PEM.into());
    known.internet_play = Some("its own".into());
    known.https_port = 47984;
    let mut hosts = Hosts::without_discovery(vec![known], Arc::new(|| {}));

    // Before anything is sent: the link's certificate is not the one this PC is paired with.
    hosts.redeem_invite(invite_link("Den"));
    assert!(matches!(
        hosts.events.try_recv(),
        Ok(Event::Invited { result: Err(why), id: None }) if why == INVITE_KNOWN_PC
    ));

    // And once more where the PC is kept, whatever led there.
    assert_eq!(hosts.apply_invite(&redeemed_as("HOST-1", OTHER_PEM)), Err(INVITE_KNOWN_PC.to_owned()));
    assert_eq!(hosts.apply_invite(&redeemed_as("HOST-1", "not a certificate")), Err(INVITE_KNOWN_PC.to_owned()));
    assert_eq!(hosts.list.len(), 1);
    let record = &hosts.list[0].record;
    assert_eq!(record.server_certificate.as_deref(), Some(PEM));
    assert_eq!(record.internet_play.as_deref(), Some("its own"));
}

#[test]
fn an_invite_renews_the_pc_it_proved_and_adds_one_not_listed() {
    let mut known = paired_record("HOST-1");
    known.server_certificate = Some(PEM.into());
    known.internet_play = Some("its own".into());
    let mut hosts = Hosts::without_discovery(vec![known], Arc::new(|| {}));
    let id = hosts.list[0].id;

    // The same certificate: the PC itself, which gets the invite's details.
    assert_eq!(hosts.apply_invite(&redeemed_as("HOST-1", PEM)), Ok(id));
    assert_eq!(hosts.list.len(), 1);
    assert_eq!(hosts.list[0].record.internet_play.as_deref(), Some("the invite's"));

    // A PC not on the list is added with the certificate the invite proved.
    let added = hosts.apply_invite(&redeemed_as("HOST-2", OTHER_PEM)).unwrap();
    assert_ne!(added, id);
    assert_eq!(hosts.list.len(), 2);
    assert_eq!(hosts.get(added).unwrap().record.server_certificate.as_deref(), Some(OTHER_PEM));

    // A PC found on the network and never paired has no certificate to keep.
    let mut found = paired_record("HOST-3");
    found.server_certificate = None;
    let mut hosts = Hosts::without_discovery(vec![found], Arc::new(|| {}));
    let id = hosts.list[0].id;
    assert_eq!(hosts.apply_invite(&redeemed_as("HOST-3", OTHER_PEM)), Ok(id));
    assert_eq!(hosts.list[0].record.server_certificate.as_deref(), Some(OTHER_PEM));
}

#[test]
fn add_pc_reads_addresses_and_pairing_links() {
    assert_eq!(parse_entry(" 192.168.1.20:48123 "), Ok(Entry::Address("192.168.1.20".into(), Some(48123))));
    assert_eq!(parse_entry("[::1]"), Ok(Entry::Address("::1".into(), None)));
    assert_eq!(parse_entry("pc:port"), Err(ADDRESS_INVALID));
    let invitation = PairingUri {
        host: "192.168.4.2".into(),
        port: 48123,
        otp: "4821".into(),
        passphrase: "a&b=c d".into(),
        unique_id: "0123ABCD".into(),
        cert_sha256: [7; 32],
        name: "Wohnzimmer-PC".into(),
    };
    let pasted = format!("  {}\n", invitation.to_uri());
    assert_eq!(parse_entry(&pasted), Ok(Entry::Link(Box::new(invitation.clone()))));
    // As Windows' shell hands a link over; whoever copies it from there pastes this form.
    let from_shell = invitation.to_uri().replacen("pair?", "pair/?", 1);
    assert_eq!(parse_entry(&from_shell), Ok(Entry::Link(Box::new(invitation.clone()))));
    let v6 = PairingUri { host: "[fe80::1%eth0]".into(), ..invitation.clone() };
    let Ok(Entry::Link(link)) = parse_entry(&v6.to_uri()) else { panic!("an IPv6 link") };
    assert_eq!(link.host, "fe80::1%eth0", "kept without brackets, like a typed address");
    // A broken link is refused as a link, never added as an address.
    let short = invitation.to_uri().replace(&"07".repeat(32), &"07".repeat(31));
    assert_eq!(parse_entry(&short), Err(LINK_INVALID));
    assert_eq!(parse_entry("rubylight://pair?v=2"), Err(LINK_INVALID));
    assert_eq!(pair_error(&nightjar_core::http::Error::CertificateMismatch), PAIR_LINK_MISMATCH);
}

#[test]
fn sleep_spends_the_offered_token_and_the_answer_reaches_the_ui() {
    let pc = FakePc::new(&[TOKEN_A], accepted);
    let (token, at) = sleep_token(&pc).unwrap();
    assert_eq!(token, TOKEN_A);
    assert!(matches!(sleep_confirmed(&pc, token, at), Ok(SleepStep::Accepted)));
    assert_eq!(pc.calls(), ["GET".to_owned(), format!("POST {TOKEN_A}")], "one offer, one request");

    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    hosts.sender.send(Event::Sleep { id, ask: 1, name: "PC".into(), confirmed: true, result: Ok(SleepStep::Accepted) }).unwrap();
    let (events, changed) = hosts.drain(&egui::Context::default());
    assert!(!changed);
    assert!(matches!(events[..], [Event::Sleep { confirmed: true, result: Ok(SleepStep::Accepted), .. }]));
}

#[test]
fn a_token_older_than_25_seconds_is_asked_for_again_before_it_is_spent() {
    let ago = |s| {
        let now = Handed::now();
        Handed { at: now.at.checked_sub(Duration::from_secs(s)).unwrap(), wall: now.wall - Duration::from_secs(s) }
    };
    let pc = FakePc::new(&[TOKEN_B], accepted);
    assert!(sleep_confirmed(&pc, TOKEN_A.into(), ago(24)).is_ok());
    assert_eq!(pc.calls(), [format!("POST {TOKEN_A}")], "a young token is spent as it is");

    let pc = FakePc::new(&[TOKEN_B], accepted);
    assert!(sleep_confirmed(&pc, TOKEN_A.into(), ago(26)).is_ok());
    assert_eq!(pc.calls(), ["GET".to_owned(), format!("POST {TOKEN_B}")], "the fresh token is spent");

    // A PC that no longer offers a token (busy by now) says so, and nothing is spent.
    let pc = FakePc::new(&[], accepted);
    assert_eq!(sleep_confirmed(&pc, TOKEN_A.into(), ago(26)).err().as_deref(), Some(SLEEP_BUSY));
    assert_eq!(pc.calls(), ["GET"]);
}

#[test]
fn a_token_is_as_old_as_either_clock_says_even_across_suspend() {
    let now = Handed::now();
    let secs = Duration::from_secs;
    // Suspended for a minute: the monotonic clock missed it, the wall clock did not.
    let suspended = Handed { at: now.at.checked_sub(secs(1)).unwrap(), wall: now.wall - secs(61) };
    let pc = FakePc::new(&[TOKEN_B], accepted);
    assert!(sleep_confirmed(&pc, TOKEN_A.into(), suspended).is_ok());
    assert_eq!(pc.calls(), ["GET".to_owned(), format!("POST {TOKEN_B}")], "the stale token is not spent");
    // The wall clock set back: the monotonic age still counts.
    let set_back = Handed { at: now.at.checked_sub(secs(26)).unwrap(), wall: now.wall + secs(3600) };
    assert!(set_back.age() >= SLEEP_TOKEN_REFRESH);
    let fresh = Handed { at: now.at.checked_sub(secs(1)).unwrap(), wall: now.wall + secs(3600) };
    assert!(fresh.age() < SLEEP_TOKEN_REFRESH);
}

#[test]
fn a_refusal_shows_the_hosts_words_and_a_failed_request_says_what_failed() {
    use nightjar_core::http::{Error, Refusal};
    let busy = || Err(PowerError::Refused {
        refusal: Refusal::Conflict,
        reply: power_reply(409, "End all applications and sessions before requesting sleep", [true, true, false], None),
    });
    let pc = FakePc::new(&[TOKEN_A], busy);
    assert_eq!(
        sleep_confirmed(&pc, TOKEN_A.into(), Handed::now()).err().as_deref(),
        Some("End all applications and sessions before requesting sleep"),
        "exactly as sent"
    );
    let silent = || Err(PowerError::Refused { refusal: Refusal::Unauthorized, reply: power_reply(401, "", [false; 3], None) });
    assert_eq!(
        sleep_confirmed(&FakePc::new(&[], silent), TOKEN_A.into(), Handed::now()).err().as_deref(),
        Some("The PC refused with status 401.")
    );
    let timeout = || Err(PowerError::Host(Error::Io(std::io::ErrorKind::TimedOut.into())));
    assert_eq!(
        sleep_confirmed(&FakePc::new(&[], timeout), TOKEN_A.into(), Handed::now()).err().as_deref(),
        Some("The PC did not answer.")
    );
}

/// A PC offering Restart and Shut down: hands out `tokens` in turn, answers a request with `post`.
struct FakePowerPc {
    tokens: std::cell::RefCell<Vec<&'static str>>,
    reason: Option<&'static str>,
    post: fn() -> Result<(), nightjar_core::http::Error>,
    calls: std::cell::RefCell<Vec<String>>,
}

impl PcPowerHost for FakePowerPc {
    fn pc_power(&self) -> Result<PcPowerOffer, nightjar_core::http::Error> {
        use nightjar_core::http::PowerAvailability;
        self.calls.borrow_mut().push("GET".into());
        let mut tokens = self.tokens.borrow_mut();
        let token = (!tokens.is_empty()).then(|| tokens.remove(0).to_owned());
        let available = PowerAvailability { available: token.is_some(), reason: self.reason.map(Into::into) };
        Ok(PcPowerOffer { restart: available.clone(), shutdown: available, token, token_lifetime_s: 30, ..Default::default() })
    }
    fn power_pc(&self, action: PcPower, token: &str) -> Result<(), nightjar_core::http::Error> {
        self.calls.borrow_mut().push(format!("POST {} {token}", action.path()));
        (self.post)()
    }
}

fn power_pc(tokens: &[&'static str], reason: Option<&'static str>, post: fn() -> Result<(), nightjar_core::http::Error>) -> FakePowerPc {
    FakePowerPc { tokens: std::cell::RefCell::new(tokens.to_vec()), reason, post, calls: Default::default() }
}

#[test]
fn restart_and_shut_down_spend_a_fresh_token_and_show_the_hosts_refusal() {
    use nightjar_core::http::{native_api::error::ApiError, Error};
    let ok = || Ok(());
    let pc = power_pc(&[TOKEN_A], None, ok);
    let Ok(PcPowerStep::Offer { token, at, wake_from_off: false }) = pc_power_token(&pc, PcPower::Restart) else { panic!("an offer") };
    assert!(matches!(pc_power_confirmed(&pc, PcPower::Restart, token, at), Ok(PcPowerStep::Accepted)));
    assert_eq!(*pc.calls.borrow(), ["GET".to_owned(), format!("POST power/restart {TOKEN_A}")]);

    // An old token is replaced before it is spent, as Sleep PC does.
    let now = Handed::now();
    let old = Handed { at: now.at.checked_sub(Duration::from_secs(26)).unwrap(), wall: now.wall - Duration::from_secs(26) };
    let pc = power_pc(&[TOKEN_B], None, ok);
    assert!(pc_power_confirmed(&pc, PcPower::ShutDown, TOKEN_A.into(), old).is_ok());
    assert_eq!(*pc.calls.borrow(), ["GET".to_owned(), format!("POST power/shutdown {TOKEN_B}")]);

    // A busy PC: its reason as it is, or ours when it gives none; nothing is spent.
    let pc = power_pc(&[], Some("TV is streaming from this PC."), ok);
    assert_eq!(pc_power_token(&pc, PcPower::ShutDown).err().as_deref(), Some("TV is streaming from this PC."));
    assert_eq!(pc_power_token(&power_pc(&[], None, ok), PcPower::Restart).err().as_deref(), Some(PC_POWER_BUSY));

    // A refused request shows the host's words.
    let refused = || {
        let error: ApiError = serde_json::from_str(
            r#"{"code":"host_in_use","message":"Desk is streaming from this PC.","setting":null,"retry_after_ms":null}"#,
        )
        .unwrap();
        Err(Error::Api { status: 409, error: Box::new(error) })
    };
    let pc = power_pc(&[TOKEN_A], None, refused);
    assert_eq!(
        pc_power_confirmed(&pc, PcPower::Restart, TOKEN_A.into(), Handed::now()).err().as_deref(),
        Some("Desk is streaming from this PC.")
    );
}

/// A PC that answers `GET /power` with these refusals in turn, then stops answering; each
/// marked unconfirmed by the second field.
struct RefusingPc(std::cell::RefCell<Vec<Option<&'static str>>>, bool);

impl PcPowerHost for RefusingPc {
    fn pc_power(&self) -> Result<PcPowerOffer, nightjar_core::http::Error> {
        let mut answers = self.0.borrow_mut();
        if answers.is_empty() {
            return Err(nightjar_core::http::Error::NotFound("/power".into()));
        }
        Ok(PcPowerOffer { last_refusal: answers.remove(0).map(Into::into), last_refusal_unconfirmed: self.1, ..Default::default() })
    }
    fn power_pc(&self, _: PcPower, _: &str) -> Result<(), nightjar_core::http::Error> {
        panic!("the refusal check never asks again")
    }
}

#[test]
fn a_refusal_after_the_pc_took_the_request_is_shown() {
    let refusing = |answers: &[Option<&'static str>]| RefusingPc(std::cell::RefCell::new(answers.to_vec()), false);
    let windows = "Windows refused to restart the PC: Access is denied. (exit code 5)";
    assert_eq!(pc_power_refusal(&refusing(&[None, None, Some(windows)]), 10, || {}), Some(LateRefusal::Refused(windows.into())));
    // Windows did not confirm it in time: the host says so, and it is no refusal.
    let unanswered = "Windows did not confirm the request in time; the PC may still restart: process timed out";
    let unconfirmed = RefusingPc(std::cell::RefCell::new(vec![None, Some(unanswered)]), true);
    assert_eq!(pc_power_refusal(&unconfirmed, 10, || {}), Some(LateRefusal::Unconfirmed(unanswered.into())));
    // Going down: the PC stops answering, nothing to show.
    assert_eq!(pc_power_refusal(&refusing(&[None]), 10, || {}), None);
    // Never says within the reads.
    assert_eq!(pc_power_refusal(&refusing(&[None, None, None, Some(windows)]), 3, || {}), None);
}

/// A PC during an update: `GET /update` answers in turn (None: not answering), and
/// `/serverinfo` says `running` once the answers run out.
struct UpdatingPc {
    answers: std::cell::RefCell<Vec<Option<HostUpdate>>>,
    running: Option<&'static str>,
}

impl UpdateHost for UpdatingPc {
    fn host_update(&self) -> Result<HostUpdate, nightjar_core::http::Error> {
        let mut answers = self.answers.borrow_mut();
        match (!answers.is_empty()).then(|| answers.remove(0)).flatten() {
            Some(update) => Ok(update),
            None => Err(nightjar_core::http::Error::NotFound("/update".into())),
        }
    }
    fn running_version(&self) -> Option<String> {
        if self.answers.borrow().is_empty() {
            self.running.map(Into::into)
        } else {
            Some("3.3.0".into())
        }
    }
}

#[test]
fn an_update_fails_at_once_without_a_target_and_follows_the_pcs_phase_error_and_wait() {
    let update = |phase: &str, error: Option<&str>, waiting: Option<&str>| HostUpdate {
        current_version: "3.3.0".into(),
        latest_version: Some("v3.4.0".into()),
        phase: phase.into(),
        install_supported: true,
        error: error.map(Into::into),
        waiting_for: waiting.map(Into::into),
        install_now: false,
    };
    // The 202 offers nothing newer: no 15-minute wait for version "".
    let none = HostUpdate { latest_version: None, ..update("waiting", None, None) };
    assert_eq!(update_target(&none), Err(UPDATE_GONE.to_owned()));
    assert_eq!(update_target(&update("waiting", None, None)), Ok("3.4.0".to_owned()));
    let pc = |answers: Vec<Option<HostUpdate>>, running| UpdatingPc { answers: answers.into(), running };
    let follow_most = |pc: &UpdatingPc, polls, most| {
        let mut said = Vec::new();
        let result = follow_update(pc, "3.4.0", polls, most, || {}, |step| said.push(step));
        (result, said)
    };
    let follow = |pc: &UpdatingPc, polls| follow_most(pc, polls, u32::MAX);
    // Back on the new version.
    let (result, said) = follow(&pc(vec![Some(update("downloading", None, None)), None], Some("3.4.0")), 10);
    assert_eq!((result, said), (Ok(UpdateStep::Back("3.4.0".into())), vec![]));
    // The updater failed: its words, at once.
    let failed = update("failed", Some("Installer verification failed; no files were installed"), None);
    let (result, _) = follow(&pc(vec![Some(failed)], None), 10);
    assert_eq!(result, Err("Installer verification failed; no files were installed".into()));
    let (result, _) = follow(&pc(vec![Some(update("failed", None, None))], None), 10);
    assert_eq!(result, Err(UPDATE_FAILED.into()));
    // Gone, then back on the old version with nothing under way: setup gave up.
    let (result, _) = follow(&pc(vec![Some(update("installing", None, None)), None, Some(update("idle", None, None))], None), 10);
    assert!(result.as_ref().is_err_and(|e| e.contains("3.3.0")), "{result:?}");
    // Waiting for another device's stream: said once, and not counted against the wait.
    let tv = "TV is streaming from this PC.";
    let waits = vec![Some(update("ready", None, Some(tv))); 5];
    let (result, said) = follow(&pc([waits, vec![None]].concat(), Some("3.4.0")), 2);
    assert_eq!((result, said), (Ok(UpdateStep::Back("3.4.0".into())), vec![UpdateStep::Waiting(tv.into())]));
    // Nothing ever comes back.
    let (result, _) = follow(&pc(vec![Some(update("installing", None, None)); 3], None), 3);
    assert_eq!(result, Err(trf("The PC did not come back with version {} in time.", &[&"3.4.0"])));
    // Another device's stream holds it for `most` reads at most, then the wait gives up.
    let held = pc(vec![Some(update("ready", None, Some(tv))); 6], Some("3.4.0"));
    let (result, said) = follow_most(&held, 2, 4);
    assert_eq!(result, Err(trf("The PC did not come back with version {} in time.", &[&"3.4.0"])));
    assert_eq!((said, held.answers.borrow().len()), (vec![UpdateStep::Waiting(tv.into())], 2));
    assert_eq!(UPDATE_MAX_WAIT.as_secs(), 3 * 60 * 60);
}

/// The PC's last answer, with the power fields when `power` is (supported, allowed).
fn sleep_info(rubylight: bool, power: Option<(bool, bool)>) -> ServerInfo {
    let mut extra = String::new();
    if rubylight {
        extra.push_str("<RustHostVersion>3.0.0</RustHostVersion>");
    }
    if let Some((supported, allowed)) = power {
        extra.push_str(&format!(
            "<PowerActionVersion>1</PowerActionVersion><SleepSupported>{}</SleepSupported><SleepAllowed>{}</SleepAllowed>",
            u8::from(supported),
            u8::from(allowed)
        ));
    }
    ServerInfo::parse(&INFO.replace("</root>", &format!("{extra}</root>")), 47989).unwrap()
}

#[test]
fn sleep_is_offered_only_by_a_pc_that_says_so() {
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let host = &mut hosts.list[0];
    host.reach = Reach::Online;
    host.info = Some(sleep_info(false, None));
    assert_eq!(host.sleep_option(), None, "a stock host never shows Sleep PC");
    host.info = Some(sleep_info(true, None));
    assert_eq!(host.sleep_option(), None, "nor a Rubylight without the power API");
    host.info = Some(sleep_info(true, Some((false, true))));
    assert_eq!(host.sleep_option(), None, "nor a PC that cannot sleep");
    host.info = Some(sleep_info(false, Some((true, false))));
    assert_eq!(host.sleep_option(), Some(Err(SLEEP_NOT_ALLOWED)), "not for this device: shown with the reason");
    host.info = Some(sleep_info(false, Some((true, true))));
    assert_eq!(host.sleep_option(), Some(Ok(())));
    host.reach = Reach::Offline;
    assert_eq!(host.sleep_option(), None, "a PC that does not answer is not asked");
}

#[test]
fn an_offer_without_a_token_says_why() {
    let reply = |flags| move || -> Result<PowerReply, PowerError> { Ok(power_reply(200, "", flags, None)) };
    struct Offer(Box<dyn Fn() -> Result<PowerReply, PowerError>>);
    impl PowerHost for Offer {
        fn power_capabilities(&self) -> Result<PowerReply, PowerError> { (self.0)() }
        fn sleep(&self, _: &str) -> Result<PowerReply, PowerError> { unreachable!("nothing to spend") }
    }
    for (flags, why) in [
        ([false, true, false], SLEEP_UNSUPPORTED),
        ([true, false, false], SLEEP_NOT_ALLOWED),
        ([true, true, false], SLEEP_BUSY),
        ([true, true, true], SLEEP_BUSY),
    ] {
        assert_eq!(sleep_token(&Offer(Box::new(reply(flags)))).err().as_deref(), Some(why));
    }
}

/// A paired PC comes back at a new address (a new DHCP lease, only new IPv6 addresses after a
/// reboot or a wake): the sighting there is polled over plain HTTP, where every host says "not
/// paired". It merges into the paired PC, which keeps its pairing and takes the new address; only
/// the pinned PC's own word (its refusal of this device) drops the pairing.
#[test]
fn a_paired_pc_at_a_new_address_keeps_its_pairing() {
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    info_lands(&mut hosts, id, native_info());
    assert!(hosts.list[0].paired());

    sight(&mut hosts, "PC", "192.0.2.50", 0);
    assert_eq!(hosts.list.len(), 2, "a sighting at an address not listed is a new entry until it answers");
    let sighted = hosts.list[1].id;
    send(&mut hosts, Event::Info { path: InfoPath::Plain, id: sighted, seq: 1, address: "192.0.2.50".into(), result: Ok(Box::new(native_info_of("uid", false))) });
    assert_eq!(hosts.list.len(), 1, "the same unique ID: merged into the paired PC");
    let host = &hosts.list[0];
    assert_eq!(host.id, id);
    assert_eq!(host.record.server_certificate.as_deref(), Some("pem"), "the pairing survives an HTTP answer");
    assert_eq!(host.record.address, "192.0.2.50", "the PC moves to the new address");
    assert!(host.record.other_addresses.contains(&"192.0.2.1".to_owned()), "the old one is kept among its others");
    assert!(host.paired(), "Nightjar does not ask for a PIN");

    // The poll over pinned HTTPS at the new address confirms it.
    info_lands(&mut hosts, id, native_info());
    assert!(hosts.list[0].paired() && hosts.list[0].reach == Reach::Online);

    // A poll that left over plain HTTP before a pairing ended lands after it: the new pin stays.
    let plain = Event::Info { path: InfoPath::Plain, id, seq: hosts.list[0].info_seq + 1, address: "192.0.2.50".into(), result: Ok(Box::new(native_info_of("uid", false))) };
    send(&mut hosts, plain);
    assert!(hosts.list[0].paired());

    // The pinned PC refusing this device (401 over pinned HTTPS) is the PC forgetting it.
    let refused = Event::Info { path: InfoPath::Refused, id, seq: hosts.list[0].info_seq + 1, address: "192.0.2.50".into(), result: Ok(Box::new(native_info_of("uid", false))) };
    send(&mut hosts, refused);
    assert!(hosts.list[0].record.server_certificate.is_none(), "the PC forgot this device: pair again");
}

/// Another certificate at the PC's address (another PC took its lease, or a second streaming app
/// answers on the same ports): no HTTP answer of that one is taken. The PC keeps its pairing and
/// its unique ID, and reads as away with the reason.
#[test]
fn another_certificate_at_the_address_keeps_the_pairing_and_the_unique_id() {
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    info_lands(&mut hosts, id, native_info());
    let error = poll_error(&nightjar_core::http::Error::CertificateMismatch);
    assert_eq!(error, ANOTHER_PC_ANSWERS);
    let seq = hosts.list[0].info_seq + 1;
    send(&mut hosts, Event::Info { path: InfoPath::Plain, id, seq, address: "192.0.2.1".into(), result: Err(error) });
    let host = &hosts.list[0];
    assert_eq!(host.record.server_certificate.as_deref(), Some("pem"), "the pin stays");
    assert_eq!(host.record.unique_id, "uid", "the unique ID is not the other one's");
    assert_eq!(host.reach, Reach::Offline);
    assert_eq!(host.error.as_deref(), Some(ANOTHER_PC_ANSWERS));
    // Other failures read as before.
    assert_eq!(poll_error(&nightjar_core::http::Error::NotPaired), describe(&nightjar_core::http::Error::NotPaired));
}

/// Another certificate at the PC's first address and a timeout at the next (its IPv6 or remote
/// address): the PC reads as away with the reason the user can act on, not the last timeout.
#[test]
fn another_certificate_is_the_reason_over_a_later_paths_failure() {
    let mut failure = PollFailure::default();
    failure.failed(poll_error(&nightjar_core::http::Error::CertificateMismatch));
    failure.failed(describe(&nightjar_core::http::Error::Io(std::io::ErrorKind::TimedOut.into())));
    assert_eq!(failure.reason(), ANOTHER_PC_ANSWERS);
    let mut failure = PollFailure::default();
    failure.failed("timed out".into());
    assert_eq!(failure.reason(), "timed out", "otherwise the last failure, as before");
    assert_eq!(PollFailure::default().reason(), "No address");
}

/// A plain answer for a paired PC (a poll that left before its pairing ended) is taken in for the
/// address only: the PC stays at Checking… for the pinned check that follows, and the flag that
/// asks for it is spent.
#[test]
fn a_late_plain_answer_leaves_checking_to_the_pinned_check() {
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    info_lands(&mut hosts, id, native_info());
    let host = hosts.get_mut(id).unwrap();
    (host.poll_seq, host.polling, host.checking) = (host.info_seq + 1, true, true);
    let seq = host.poll_seq;
    send(&mut hosts, Event::Info { path: InfoPath::Plain, id, seq, address: "192.0.2.1".into(), result: Ok(Box::new(native_info_of("uid", false))) });
    let host = hosts.get(id).unwrap();
    assert!(host.paired());
    assert!(host.checking && host.polling, "the pinned check, not the plain answer, ends Checking…");
    assert!(!host.pinned_check, "the check was asked for once");
}

#[test]
fn missing_identity_or_host_on_pair_or_invite_emits_error_and_wakes() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    let waker: Waker = Arc::new(move || {
        w.fetch_add(1, Ordering::SeqCst);
    });
    let mut hosts = Hosts::without_discovery(Vec::new(), waker);
    hosts.redeem_invite(invite_link("Den"));
    assert!(matches!(
        hosts.events.try_recv(),
        Ok(Event::Invited { result: Err(_), id: None })
    ));
    assert_eq!(wakes.load(Ordering::SeqCst), 1);

    hosts.pair(999, "1234".into(), None);
    assert!(matches!(
        hosts.events.try_recv(),
        Ok(Event::Paired { id: 999, result: Err(_), .. })
    ));
    assert_eq!(wakes.load(Ordering::SeqCst), 2);
}
