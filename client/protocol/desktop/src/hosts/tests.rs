//! Unit tests for desktop host workers, polling, discovery, remote paths and native API reads.

use super::*;

mod classic_and_links;
mod covers;
mod power_and_invites;


#[test]
fn the_read_for_a_selected_game_gives_up_at_its_deadline_and_says_why() {
    let started = Instant::now();
    let slow = within(Duration::from_millis(20), || {
        std::thread::sleep(Duration::from_secs(5));
        Ok(1)
    });
    assert!(slow.is_err_and(|e| e.contains("no answer")));
    assert!(started.elapsed() < Duration::from_secs(4), "it does not wait for the read");
    assert_eq!(within(Duration::from_secs(5), || Ok(1)), Ok(1));
    assert_eq!(within(Duration::from_secs(5), || Err::<i32, _>("refused".to_owned())), Err("refused".to_owned()));
}

/// The native-api capabilities fixture with these features added, and the folder grant.
pub(crate) fn native_caps(features: &[&str], shared_folders: bool) -> NativeCapabilities {
    let mut value: serde_json::Value = serde_json::from_str(include_str!("../../../native-api/tests/fixtures/capabilities.json")).unwrap();
    value["features"].as_array_mut().unwrap().extend(features.iter().map(|f| serde_json::Value::from(*f)));
    value["permissions"]["shared_folders"] = shared_folders.into();
    serde_json::from_value(value).unwrap()
}

/// A full native read of `id` with these capabilities lands, as the worker would send it.
pub(crate) fn native_lands(hosts: &mut Hosts, id: u64, capabilities: NativeCapabilities) {
    let epoch = hosts.get(id).unwrap().native_epoch;
    let read = NativeRead { pairing_requests: Vec::new(), capabilities: Some(capabilities), library: Some(library().into()), session: Ok(session("idle")), wake: None, update: None };
    send(hosts, Event::Native { id, epoch, seq: 1, result: Ok(Box::new(read)) });
}

/// The PC answers serverinfo again, as every poll does.
pub(crate) fn info_lands(hosts: &mut Hosts, id: u64, info: ServerInfo) {
    let seq = hosts.get(id).unwrap().info_seq + 1;
    let address = hosts.get(id).unwrap().record.address.clone();
    send(hosts, Event::Info { path: InfoPath::Pinned, id, seq, address, result: Ok(Box::new(info)) });
}

/// Serverinfo of a paired PC with the native API and the clipboard permission.
pub(crate) fn native_info() -> ServerInfo {
    native_info_of("uid", true)
}

/// `native_info` from the PC `unique_id`, which lists this device as paired or not.
/// A PC's requests run one after another on its own worker, which is started once and kept,
/// not a thread for each; a removed PC's worker goes with it.
#[test]
fn a_pcs_requests_share_one_worker_that_goes_with_the_pc() {
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    let (tx, rx) = channel();
    for _ in 0..3 {
        let tx = tx.clone();
        hosts.run_on(id, Lane::Asked, move || {
            let _ = tx.send(std::thread::current().id());
            None
        });
    }
    let threads: Vec<_> = (0..3).map(|_| rx.recv_timeout(Duration::from_secs(5)).expect("the job ran")).collect();
    assert!(threads.iter().all(|t| *t == threads[0]), "one worker: {threads:?}");
    assert_ne!(threads[0], std::thread::current().id());
    assert_eq!(hosts.pc_workers.len(), 1);
    hosts.remove(id);
    assert!(hosts.pc_workers.is_empty());
}

/// A slow background poll (an away PC is tried over the internet too) holds up neither a check
/// the user asked for nor a native read: those run on the PC's other worker and answer while it
/// is still out. A removed PC's two workers go with it.
#[test]
fn a_check_asked_for_answers_while_a_slow_background_poll_is_out() {
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    let (release, held) = channel::<()>();
    let (tx, rx) = channel();
    let background = tx.clone();
    hosts.run_on(id, Lane::Background, move || {
        let _ = held.recv_timeout(Duration::from_secs(30));
        let _ = background.send("background");
        None
    });
    hosts.run_on(id, Lane::Asked, move || {
        let _ = tx.send("asked");
        None
    });
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok("asked"), "not behind the slow poll");
    assert!(rx.try_recv().is_err(), "which is still out");
    release.send(()).unwrap();
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok("background"));
    assert_eq!(hosts.pc_workers.len(), 2);
    hosts.remove(id);
    assert!(hosts.pc_workers.is_empty());
}

/// Try again pressed while an older poll is out: that poll's answer lands first and says Offline
/// with the old error, but the card stays at Checking… until the check the user asked for answers.
#[test]
fn an_older_polls_answer_leaves_a_newer_check_checking() {
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    let host = hosts.get_mut(id).unwrap();
    (host.poll_seq, host.polling, host.checking) = (2, true, true);
    send(&mut hosts, Event::Info { path: InfoPath::Pinned, id, seq: 1, address: "192.0.2.1".into(), result: Err("timed out".into()) });
    let host = hosts.get(id).unwrap();
    assert!(host.checking && host.polling, "the old poll's answer is not the check's");
    assert_eq!(hosts.polling_seq(id), Some(2));
    info_lands(&mut hosts, id, native_info());
    let host = hosts.get(id).unwrap();
    assert!(!host.checking && !host.polling, "the check's own answer ends it");
    assert_eq!(host.reach, Reach::Online);
}

/// Background polls of one PC never queue up, and a poll asked for while another asked one is
/// out waits for it instead of queueing behind it, then starts once it answers.
#[test]
fn polls_of_one_pc_never_queue_up() {
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    let now = Instant::now();
    // A background poll is out; the check the user asked for answered first.
    hosts.get_mut(id).unwrap().background_poll = Some(1);
    assert!(hosts.due(now, true, None).is_empty(), "no second background poll behind the first");
    assert!(!hosts.poll_landed(id, 1) && hosts.get(id).unwrap().background_poll.is_none());
    assert_eq!(hosts.due(now, true, None), vec![id]);
    // An asked poll is out: two more presses reserve one poll, which starts once it answers.
    hosts.set_identity(Arc::new(ClientIdentity::generate().expect("a key")));
    {
        let host = hosts.get_mut(id).unwrap();
        (host.asked_poll, host.poll_seq) = (Some(1), 1);
    }
    hosts.poll_one(id);
    hosts.check_now(id);
    let host = hosts.get(id).unwrap();
    assert!(host.asked_again && host.checking && host.polling);
    assert_eq!((host.asked_poll, host.poll_seq), (Some(1), 3), "nothing more was sent");
    assert!(hosts.pc_workers.is_empty());
    assert!(hosts.poll_landed(id, 1), "the reserved poll goes once the one out answers");
    assert!(!hosts.get(id).unwrap().asked_again);
}

pub(super) fn native_info_of(unique_id: &str, paired: bool) -> ServerInfo {
    let xml = format!("<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\"><hostname>PC</hostname><uniqueid>{unique_id}</uniqueid>\
             <HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>{}</PairStatus><currentgame>0</currentgame>\
             <state>SUNSHINE_SERVER_FREE</state><RustHostVersion>3.0.0</RustHostVersion><NativeApiVersion>1</NativeApiVersion>\
             <Permission>100859904</Permission></root>", u8::from(paired));
    ServerInfo::parse(&xml, 47989).unwrap()
}

#[test]
fn grants_go_when_the_pc_forgets_this_device_or_another_pc_answers() {
    let gated = |hosts: &Hosts| {
        let info = hosts.list[0].info.as_ref().unwrap();
        [Feature::SharedFolders, Feature::Coop, Feature::ImageClipboard, Feature::FileUpload].map(|f| info.capabilities.has(f))
    };
    let extras = ["folders", "coop", "clipboard.image", "files.upload"];
    let mut granted = native_caps(&extras, true);
    granted.files.file_upload = true;
    for (answer, why) in [(native_info_of("uid", false), "the PC no longer lists this device"), (native_info_of("other", true), "another PC answers")] {
        let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
        hosts.list[0].record.claim_notice(crate::config::NOTICE_PYROWAVE_FALLBACK);
        let id = hosts.list[0].id;
        info_lands(&mut hosts, id, native_info());
        native_lands(&mut hosts, id, granted.clone());
        assert_eq!(gated(&hosts), [true; 4]);
        let in_flight = hosts.list[0].native_epoch;
        info_lands(&mut hosts, id, answer);
        assert_eq!(gated(&hosts), [false; 4], "{why}: no old grant opens a row or a gate");
        assert!(hosts.list[0].native.capabilities.is_none() && hosts.list[0].native.library.is_none(), "{why}");
        // A read that started before lands now: dropped.
        let read = NativeRead { pairing_requests: Vec::new(), capabilities: Some(granted.clone()), library: Some(library().into()), session: Ok(session("idle")), wake: None, update: None };
        hosts.list[0].record.server_certificate = Some("pem".into());
        send(&mut hosts, Event::Native { id, epoch: in_flight, seq: 9, result: Ok(Box::new(read)) });
        assert!(hosts.list[0].native.capabilities.is_none(), "{why}: the old read is not taken in");
        // A notice told for the old PC is told again for another one; the same PC keeps it.
        let told = hosts.list[0].record.notices.contains(crate::config::NOTICE_PYROWAVE_FALLBACK);
        assert_eq!(told, why != "another PC answers", "{why}");
    }
}

/// The same PC as a stock Moonlight host sees it: no native API.
pub(crate) fn stock_info() -> ServerInfo {
    let xml = "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\"><hostname>PC</hostname><uniqueid>uid</uniqueid>\
             <HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>1</PairStatus><currentgame>0</currentgame>\
             <state>SUNSHINE_SERVER_FREE</state><Permission>100859904</Permission></root>";
    ServerInfo::parse(xml, 47989).unwrap()
}

pub(crate) const NATIVE_EXTRAS: &[&str] = &["folders", "coop", "clipboard.image"];

#[test]
fn the_native_read_reaches_every_gate_and_stays_across_polls() {
    let gated = |info: &ServerInfo| {
        [Feature::SharedFolders, Feature::Coop, Feature::ImageClipboard].map(|f| info.capabilities.has(f))
    };
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    info_lands(&mut hosts, id, native_info());
    assert_eq!(gated(hosts.list[0].info.as_ref().unwrap()), [false; 3], "serverinfo alone offers none of them");
    native_lands(&mut hosts, id, native_caps(NATIVE_EXTRAS, true));
    assert_eq!(gated(hosts.list[0].info.as_ref().unwrap()), [true; 3], "the native read is merged in");
    info_lands(&mut hosts, id, native_info());
    assert_eq!(gated(hosts.list[0].info.as_ref().unwrap()), [true; 3], "and kept when serverinfo is read again");
    // A resume reads serverinfo again too, and merges the plan's native read the same way.
    let mut fresh = native_info();
    merge_native(&mut fresh, hosts.list[0].native.capabilities.as_ref());
    assert_eq!(gated(&fresh), [true; 3]);
    // Without the folder grant the folders stay hidden, the rest stays.
    native_lands(&mut hosts, id, native_caps(NATIVE_EXTRAS, false));
    assert_eq!(gated(hosts.list[0].info.as_ref().unwrap()), [false, true, true]);
    // A PC that no longer offers the native API offers none of them, whatever was read before.
    info_lands(&mut hosts, id, stock_info());
    assert_eq!(gated(hosts.list[0].info.as_ref().unwrap()), [false; 3]);
    let mut fresh = stock_info();
    merge_native(&mut fresh, Some(&native_caps(NATIVE_EXTRAS, true)));
    assert_eq!(gated(&fresh), [false; 3], "offered by capability, never by an old read");
}

// A PC's remote address: its own ExternalIP, else this device's STUN answer, tried last.
#[test]
fn the_remote_address_is_the_pcs_external_ip_else_stun() {
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    send(&mut hosts, Event::Remote { id, result: Ok(Some(Ipv4Addr::new(93, 184, 216, 34))) });
    assert_eq!(hosts.list[0].record.remote_address.as_deref(), Some("93.184.216.34"));
    let record = &hosts.list[0].record;
    assert_eq!(poll_addresses(record), ["192.0.2.1", "93.184.216.34"], "after the LAN address");

    let external = |ip: &str| {
        let mut info = native_info();
        info.external_ip = Some(ip.into());
        info
    };
    info_lands(&mut hosts, id, external("10.0.0.5"));
    assert_eq!(hosts.list[0].record.remote_address.as_deref(), Some("93.184.216.34"), "a private ExternalIP is no remote address");
    info_lands(&mut hosts, id, external("151.101.1.1"));
    assert_eq!(hosts.list[0].record.remote_address.as_deref(), Some("151.101.1.1"), "the PC's own replaces STUN's");
    send(&mut hosts, Event::Remote { id, result: Ok(Some(Ipv4Addr::new(93, 184, 216, 34))) });
    assert_eq!(hosts.list[0].record.remote_address.as_deref(), Some("151.101.1.1"), "and stays over a later STUN answer");
    send(&mut hosts, Event::Remote { id, result: Err(std::io::Error::other("unreachable")) });
    assert_eq!(hosts.list[0].record.remote_address.as_deref(), Some("151.101.1.1"));
}

// A poll lists each address once; only a public IPv4 ExternalIP becomes the remote address.
#[test]
fn the_remote_address_is_listed_once_and_only_public() {
    let mut record = paired_record("uid");
    record.other_addresses = vec!["fe80::1".into()];
    record.remote_address = Some("fe80::1".into());
    assert_eq!(poll_addresses(&record), ["192.0.2.1", "fe80::1"], "listed once");
    assert_eq!(public_ipv4(" 151.101.1.1 "), Some("151.101.1.1".into()));
    for local in ["192.168.1.2", "10.1.1.1", "172.16.0.1", "127.0.0.1", "169.254.1.1", "0.0.0.0", "198.51.100.1", "x"] {
        assert_eq!(public_ipv4(local), None, "{local}");
    }
}

// Away from home only the remote address answers: requests go there, but the PC keeps its
// LAN address, so a hairpin NAT at home does not turn home streams into internet ones.
#[test]
fn the_remote_address_reaches_the_pc_but_never_becomes_its_own() {
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    send(&mut hosts, Event::Remote { id, result: Ok(Some(Ipv4Addr::new(93, 184, 216, 34))) });
    let answer = |hosts: &mut Hosts, address: &str| {
        let seq = hosts.get(id).unwrap().info_seq + 1;
        send(hosts, Event::Info { path: InfoPath::Pinned, id, seq, address: address.into(), result: Ok(Box::new(native_info())) });
    };
    answer(&mut hosts, "93.184.216.34");
    let host = hosts.get(id).unwrap();
    assert_eq!(host.record.address, "192.0.2.1", "the remote address is not promoted");
    assert_eq!(host.connect_address(), "93.184.216.34", "but requests and streams go there");
    assert_eq!(host.connect_record().address, "93.184.216.34");
    assert_eq!(poll_addresses(&host.record), ["192.0.2.1", "93.184.216.34"], "the LAN is tried first again");

    answer(&mut hosts, "192.0.2.1");
    let host = hosts.get(id).unwrap();
    assert_eq!(host.record.address, "192.0.2.1");
    assert_eq!(host.connect_address(), "192.0.2.1", "back home the LAN is the primary");

    // Pinned, the pinned address is the only one used.
    answer(&mut hosts, "93.184.216.34");
    assert!(hosts.pin_address(id, Some("192.0.2.1")));
    assert_eq!(hosts.get(id).unwrap().connect_address(), "192.0.2.1");
    assert_eq!(poll_addresses(&hosts.get(id).unwrap().record), ["192.0.2.1"]);
}

// A record an older build saved with the remote address as its own: the LAN answers first
// and takes its place back, and an mDNS sighting does so at once.
#[test]
fn the_lan_address_comes_back_from_a_saved_remote_one() {
    let mut record = paired_record("uid");
    record.address = "93.184.216.34".into();
    record.other_addresses = vec!["192.0.2.1".into()];
    record.remote_address = Some("93.184.216.34".into());
    assert_eq!(poll_addresses(&record), ["192.0.2.1", "93.184.216.34"], "the remote address goes last");

    let mut hosts = Hosts::without_discovery(vec![record.clone()], Arc::new(|| {}));
    let id = hosts.list[0].id;
    send(&mut hosts, Event::Info { path: InfoPath::Pinned, id, seq: 1, address: "192.0.2.1".into(), result: Ok(Box::new(native_info())) });
    let host = hosts.get(id).unwrap();
    assert_eq!(host.record.address, "192.0.2.1");
    assert!(host.record.other_addresses.is_empty(), "the remote address stays only the remote one");
    assert_eq!(host.record.remote_address.as_deref(), Some("93.184.216.34"));

    let mut hosts = Hosts::without_discovery(vec![record], Arc::new(|| {}));
    assert!(hosts.apply_discovery("PC".into(), vec!["192.0.2.1".parse().unwrap()], 0));
    assert_eq!(hosts.list[0].record.address, "192.0.2.1");
    assert_eq!(hosts.list[0].connect_address(), "192.0.2.1");
}

pub(super) fn sight(hosts: &mut Hosts, name: &str, address: &str, port: u16) -> bool {
    hosts.apply_discovery(name.into(), vec![address.parse().unwrap()], port)
}

// Nightjar #7: two hosts on one machine share an address, so a sighting is a listed PC only
// at its port too. Either side may write the default port as 0.
#[test]
fn a_sighting_is_a_listed_pc_at_its_address_and_port() {
    let listed = |port, other: &str| {
        let record = HostRecord { address: "192.0.2.1".into(), other_addresses: vec![other.into()], http_port: port, ..Default::default() };
        Hosts::without_discovery(vec![record], Arc::new(|| {}))
    };
    for (saved, sighted) in [(0, 0), (0, DEFAULT_HTTP_PORT), (DEFAULT_HTTP_PORT, 0), (48989, 48989)] {
        for address in ["192.0.2.1", "192.0.2.2"] {
            let mut hosts = listed(saved, "192.0.2.2");
            sight(&mut hosts, "Desk", address, sighted);
            assert_eq!(hosts.list.len(), 1, "{address}, saved {saved}, seen {sighted}");
            assert_eq!((hosts.list[0].record.name.as_str(), hosts.list[0].record.http_port), ("Desk", saved), "its name, and its port as saved");
        }
    }
    // Another port is another PC, listed once however often it is seen.
    for (saved, sighted) in [(48989, 0), (48989, DEFAULT_HTTP_PORT), (0, 48989), (DEFAULT_HTTP_PORT, 48989)] {
        for address in ["192.0.2.1", "192.0.2.2"] {
            let mut hosts = listed(saved, "192.0.2.2");
            for _ in 0..3 {
                sight(&mut hosts, "Desk", address, sighted);
            }
            let pcs: Vec<(&str, &str, u16)> = hosts.list.iter().map(|h| (h.record.name.as_str(), h.record.address.as_str(), h.record.http_port)).collect();
            assert_eq!(pcs, [("", "192.0.2.1", saved), ("Desk", address, http_port(sighted))], "saved {saved}, seen {sighted}");
        }
    }
}

// The unique ID is the PC: a sighting that answers with a listed PC's is that PC, at
// whatever address and port, and stays it when seen again.
#[test]
fn a_sighting_that_answers_with_a_listed_pcs_unique_id_is_that_pc() {
    for address in ["192.0.2.1", "192.0.2.7"] {
        let mut record = paired_record("uid");
        (record.http_port, record.manual) = (48989, true);
        let mut hosts = Hosts::without_discovery(vec![record], Arc::new(|| {}));
        let known = hosts.list[0].id;
        sight(&mut hosts, "Desk", address, 0);
        assert_eq!(hosts.list.len(), 2, "{address}: another port, so another PC until it answers");
        let seen = hosts.list[1].id;
        info_lands(&mut hosts, seen, native_info_of("uid", true));
        assert_eq!(hosts.list.len(), 1, "{address}");
        let host = &hosts.list[0];
        assert_eq!((host.id, host.record.http_port, host.record.manual), (known, 48989, true), "{address}: the PC as it was listed");
        assert!(host.record.server_certificate.is_some(), "{address}: still paired");
        assert!(host.record.address == address || host.record.other_addresses.iter().any(|a| a == address), "{address}: {:?}", host.record);
        for _ in 0..2 {
            sight(&mut hosts, "Desk", address, DEFAULT_HTTP_PORT);
        }
        assert_eq!(hosts.list.len(), 1, "{address}: seen again, it is that PC");

        // Another PC on a third port there is its own, also once it has answered.
        sight(&mut hosts, "Den", address, 50000);
        let den = hosts.list[1].id;
        info_lands(&mut hosts, den, native_info_of("den", false));
        sight(&mut hosts, "Den", address, 50000);
        assert_eq!(hosts.list.iter().map(|h| (h.id, h.record.http_port)).collect::<Vec<_>>(), [(known, 48989), (den, 50000)], "{address}");

        // Forgotten, the PC is found again like any other.
        hosts.remove(known);
        sight(&mut hosts, "Desk", address, 0);
        assert_eq!(hosts.list.iter().map(|h| h.record.http_port).collect::<Vec<_>>(), [50000, DEFAULT_HTTP_PORT], "{address}");
    }
}

// A PC added by hand with its own port and the one discovery finds at the default port of
// the same address are two PCs: neither takes the other's place, neither is listed twice.
#[test]
fn a_pc_added_by_hand_with_a_port_stays_beside_the_one_found_at_the_default_port() {
    let mut hosts = Hosts::without_discovery(Vec::new(), Arc::new(|| {}));
    let own = hosts.add_manual("192.0.2.1", Some(48989));
    for port in [0, DEFAULT_HTTP_PORT, 0] {
        sight(&mut hosts, "Desk", "192.0.2.1", port);
    }
    assert_eq!(hosts.list.len(), 2, "the one at the default port, once");
    let by_hand = &hosts.get(own).unwrap().record;
    assert_eq!((by_hand.name.as_str(), by_hand.http_port, by_hand.manual), ("", 48989, true), "not the found PC's name or port");
    let found = hosts.list[1].id;
    let record = &hosts.list[1].record;
    assert_eq!((record.name.as_str(), record.http_port, record.manual), ("Desk", DEFAULT_HTTP_PORT, false));

    // Each is seen and added again as itself.
    sight(&mut hosts, "Den", "192.0.2.1", 48989);
    assert_eq!(hosts.get(own).unwrap().record.name, "Den");
    assert_eq!(hosts.add_manual("192.0.2.1", Some(48989)), own);
    assert_eq!(hosts.add_manual("192.0.2.1", Some(DEFAULT_HTTP_PORT)), found);
    assert_eq!(hosts.list.len(), 2);

    // By hand at the default port, saved as 0 by an older build: discovery's is that PC.
    let mut hosts = Hosts::without_discovery(vec![HostRecord { address: "192.0.2.1".into(), manual: true, ..Default::default() }], Arc::new(|| {}));
    sight(&mut hosts, "Desk", "192.0.2.1", DEFAULT_HTTP_PORT);
    assert_eq!(hosts.add_manual("192.0.2.1", Some(DEFAULT_HTTP_PORT)), hosts.list[0].id);
    assert_eq!((hosts.list.len(), hosts.list[0].record.name.as_str(), hosts.list[0].record.manual), (1, "Desk", true));
}

// A PC that answered on a second port with its unique ID stays that PC there after Nightjar
// restarts: the port is saved with it, so the next run lists no passing duplicate.
#[test]
fn a_port_that_answered_as_a_listed_pc_is_saved_with_it() {
    let mut record = paired_record("uid");
    record.http_port = 48989;
    let mut hosts = Hosts::without_discovery(vec![record], Arc::new(|| {}));
    let known = hosts.list[0].id;
    sight(&mut hosts, "Desk", "192.0.2.1", 0);
    let seen = hosts.list[1].id;
    info_lands(&mut hosts, seen, native_info_of("uid", true));
    assert_eq!(hosts.list.len(), 1);
    assert_eq!((hosts.list[0].id, hosts.list[0].record.http_port), (known, 48989));
    assert_eq!(hosts.list[0].record.other_http_ports, [DEFAULT_HTTP_PORT]);
    // Seen and merged there again, the port is listed once.
    sight(&mut hosts, "Desk", "192.0.2.1", 0);
    assert_eq!((hosts.list.len(), hosts.list[0].record.other_http_ports.len()), (1, 1));

    let saved = serde_json::to_string(&hosts.records()).unwrap();
    let records: Vec<HostRecord> = serde_json::from_str(&saved).unwrap();
    let mut hosts = Hosts::without_discovery(records, Arc::new(|| {}));
    for port in [0, DEFAULT_HTTP_PORT] {
        sight(&mut hosts, "Desk", "192.0.2.1", port);
    }
    assert_eq!(hosts.list.len(), 1, "the next run knows the port too");
    assert_eq!(hosts.add_manual("192.0.2.1", None), hosts.list[0].id, "added by hand there, it is that PC too");
    assert_eq!(hosts.list.len(), 1);
    // Another port there is still another PC.
    sight(&mut hosts, "Den", "192.0.2.1", 50000);
    assert_eq!(hosts.list.len(), 2);

    // A hosts file from before the field loads with none.
    let old: Vec<HostRecord> = serde_json::from_str(r#"[{"address":"192.0.2.1","http_port":48989,"manual":true}]"#).unwrap();
    assert!(old[0].other_http_ports.is_empty());
}

// The ports go with the PC that answered there: another PC answering at its address has none.
#[test]
fn another_pc_at_the_address_drops_the_saved_ports() {
    let mut record = paired_record("uid");
    record.other_http_ports = vec![DEFAULT_HTTP_PORT];
    record.http_port = 48989;
    let mut hosts = Hosts::without_discovery(vec![record], Arc::new(|| {}));
    let id = hosts.list[0].id;
    info_lands(&mut hosts, id, native_info_of("another", false));
    assert!(hosts.list[0].record.other_http_ports.is_empty());
    sight(&mut hosts, "Desk", "192.0.2.1", DEFAULT_HTTP_PORT);
    assert_eq!(hosts.list.len(), 2);
}

// Added by hand without a port is the PC at the default port, never one listed at that
// address under another port.
#[test]
fn a_pc_added_by_hand_without_a_port_is_the_one_at_the_default_port() {
    let mut hosts = Hosts::without_discovery(Vec::new(), Arc::new(|| {}));
    let other = hosts.add_manual("192.0.2.1", Some(48989));
    let plain = hosts.add_manual("192.0.2.1", None);
    assert_ne!(plain, other);
    assert_eq!(hosts.get(plain).unwrap().record.http_port, DEFAULT_HTTP_PORT);
    assert_eq!(hosts.add_manual("192.0.2.1", None), plain);
    assert_eq!(hosts.add_manual("192.0.2.1", Some(DEFAULT_HTTP_PORT)), plain);
    assert_eq!(hosts.add_manual("192.0.2.1", Some(48989)), other);
    assert_eq!(hosts.list.len(), 2);
    // Discovery at the default port finds the one added without a port.
    sight(&mut hosts, "Desk", "192.0.2.1", 0);
    assert_eq!((hosts.list.len(), hosts.get(plain).unwrap().record.name.as_str()), (2, "Desk"));

    // Saved as 0 by an older build, a PC added without a port is still that one.
    let mut hosts = Hosts::without_discovery(vec![HostRecord { address: "192.0.2.1".into(), manual: true, ..Default::default() }], Arc::new(|| {}));
    assert_eq!(hosts.add_manual("192.0.2.1", None), hosts.list[0].id);
    assert_eq!(hosts.list.len(), 1);
}

// Test speed goes to a Sunshine-family PC only once it said it has Foundation's test; stock
// Sunshine, Apollo and Vibepollo do not. Rubylight keeps its own test without asking.
#[test]
fn the_bandwidth_test_is_offered_only_where_the_pc_has_one() {
    let sunshine = |unique_id: &str| {
        let xml = format!("<root status_code=\"200\"><uniqueid>{unique_id}</uniqueid><state>SUNSHINE_SERVER_FREE</state>\
                <PairStatus>1</PairStatus><appversion>7.1.431.-1</appversion><SunshineVersion>2025.1</SunshineVersion></root>");
        ServerInfo::parse(&xml, 47989).unwrap()
    };
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    info_lands(&mut hosts, id, sunshine("uid"));
    assert!(!hosts.list[0].bandwidth_test(), "not before the PC answered");
    send(&mut hosts, Event::FoundationProbe { id, unique_id: "uid".into(), found: Some(false) });
    assert!(!hosts.list[0].bandwidth_test(), "stock Sunshine");
    hosts.list[0].foundation_probe = None;
    send(&mut hosts, Event::FoundationProbe { id, unique_id: "other".into(), found: Some(true) });
    assert!(!hosts.list[0].bandwidth_test(), "another PC's answer");
    send(&mut hosts, Event::FoundationProbe { id, unique_id: "uid".into(), found: Some(true) });
    assert!(hosts.list[0].bandwidth_test(), "Foundation Sunshine");
    info_lands(&mut hosts, id, sunshine("another-pc-uid"));
    assert_eq!(hosts.list[0].foundation_probe, None, "another PC at this address is asked again");

    let rubylight = "<root status_code=\"200\"><uniqueid>uid</uniqueid><state>SUNSHINE_SERVER_FREE</state><PairStatus>1</PairStatus>\
            <RustHostVersion>3.0.0</RustHostVersion><PyroWaveBandwidthProbeBytes>33554432</PyroWaveBandwidthProbeBytes></root>";
    info_lands(&mut hosts, id, ServerInfo::parse(rubylight, 47989).unwrap());
    assert_eq!(hosts.list[0].foundation_probe, None);
    assert!(hosts.list[0].bandwidth_test(), "Rubylight's PyroWave test, without asking");
}

pub(super) fn paired_record(unique_id: &str) -> HostRecord {
    HostRecord {
        pace: None,
        unique_id: unique_id.into(),
        name: "PC".into(),
        alias: None,
        address: "192.0.2.1".into(),
        http_port: 0,
        https_port: 0,
        other_addresses: Vec::new(),
        other_http_ports: Vec::new(),
        pinned_address: None,
        remote_address: None,
        server_certificate: Some("pem".into()),
        mac: None,
        wake_macs: Vec::new(),
        wake_broadcasts: Vec::new(),
        own_wake_macs: Vec::new(),
        manual: true,
        settings: None,
        app_settings: Default::default(),
        quit_on_leave: Default::default(),
        notices: Default::default(),
        full_chroma: Default::default(),
        internet_play: Default::default(),
        ..Default::default()
    }
}

pub(super) fn library() -> Library {
    serde_json::from_str(include_str!("../../../native-api/tests/fixtures/library.json")).unwrap()
}

pub(super) fn session(state: &str) -> Session {
    let text = include_str!("../../../native-api/tests/fixtures/session.json")
        .replace("\"suspended\"", &format!("\"{state}\""));
    serde_json::from_str(&text).unwrap()
}

pub(super) fn full_read(session: Session) -> Box<NativeRead> {
    Box::new(NativeRead { pairing_requests: Vec::new(), capabilities: None, library: Some(library().into()), session: Ok(session), wake: None, update: None })
}

#[test]
fn wake_targets_follow_the_pc_and_stay_when_they_could_not_be_read() {
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    let mut read = |wake: Option<(Vec<String>, Vec<String>)>| {
        let mut native = full_read(session("idle"));
        native.wake = wake;
        hosts.sender.send(Event::Native { id, epoch: 0, seq: 1, result: Ok(native) }).unwrap();
        let changed = hosts.drain(&egui::Context::default()).1;
        (changed, hosts.list[0].record.wake_macs.clone(), hosts.list[0].record.wake_broadcasts.clone())
    };
    let targets = (vec!["0a:00:27:00:00:05".to_string()], vec!["10.0.0.255".to_string()]);
    assert_eq!(read(Some(targets.clone())), (true, targets.0.clone(), targets.1.clone()), "kept, and saved");
    assert_eq!(read(Some(targets.clone())), (false, targets.0.clone(), targets.1.clone()), "the same ones save nothing");
    assert_eq!(read(None), (false, targets.0.clone(), targets.1.clone()), "a failed read keeps them");
    assert_eq!(read(Some((Vec::new(), Vec::new()))), (true, Vec::new(), Vec::new()), "a PC without wake has none");
}

pub(super) fn send(hosts: &mut Hosts, event: Event) {
    hosts.sender.send(event).unwrap();
    hosts.drain(&egui::Context::default());
}

#[test]
fn wait_quits_blocks_until_inflight_quit_guard_finishes() {
    let mut hosts = Hosts::without_discovery(vec![], Arc::new(|| {}));
    assert!(!hosts.has_pending_quits());
    let pending = Arc::new((Mutex::new(false), Condvar::new()));
    hosts.pending_quits.push(Arc::clone(&pending));
    assert!(hosts.has_pending_quits());

    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done_bg = Arc::clone(&done);
    let guard = QuitGuard(pending);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        done_bg.store(true, std::sync::atomic::Ordering::Release);
        drop(guard);
    });

    hosts.wait_quits(Duration::from_secs(2));
    assert!(done.load(std::sync::atomic::Ordering::Acquire), "shutdown wait must block until QuitGuard drops");
    assert!(!hosts.has_pending_quits());
}

#[test]
fn game_info_keys_removed_clears_cached_facts_and_retries_failed_entries() {
    struct NoopHttp;
    impl crate::game_info::Http for NoopHttp {
        fn send(&self, _: &nightjar_policy::game_info::job::Request) -> crate::game_info::Answer {
            crate::game_info::Answer { status: 0, retry_after: None, body: Vec::new() }
        }
    }

    let mut hosts = Hosts::without_discovery(vec![], Arc::new(|| {}));
    Arc::make_mut(&mut hosts.facts).insert(
        "entry-1".into(),
        nightjar_policy::game_info::cache::Facts {
            genres: vec!["Action".into()],
            ..Default::default()
        },
    );
    Arc::make_mut(&mut hosts.facts_asked).insert("entry-1".into());
    hosts.sender.send(Event::GameInfo(Box::new(nightjar_policy::game_info::cache::Record {
        key: "entry-1".into(),
        info: nightjar_policy::game_info::cache::Outcome::Failed,
        ..Default::default()
    }))).unwrap();
    hosts.drain(&egui::Context::default());
    assert!(!hosts.facts_asked.contains("entry-1"));
    assert!(!hosts.facts.contains_key("entry-1"));

    let keys = nightjar_policy::game_info::job::Keys {
        igdb_client_id: "id".into(),
        igdb_client_secret: "secret".into(),
        ..Default::default()
    };
    hosts.game_info = Some(Arc::new(crate::game_info::GameInfo::new(
        &std::env::temp_dir().join("nightjar-game-info-unused"),
        keys,
        Box::new(NoopHttp),
    )));
    Arc::make_mut(&mut hosts.facts).insert(
        "entry-2".into(),
        nightjar_policy::game_info::cache::Facts {
            genres: vec!["RPG".into()],
            ..Default::default()
        },
    );
    Arc::make_mut(&mut hosts.facts_asked).insert("entry-2".into());
    hosts.set_game_info_keys(nightjar_policy::game_info::job::Keys::default());
    assert!(hosts.facts.is_empty());
    assert!(hosts.facts_asked.is_empty());
}

#[test]
fn pin_move_external_ip_and_stun_keep_typed_endpoints_in_sync() {
    use nightjar_policy::hosts::EndpointSlot;

    let mut record = HostRecord {
        unique_id: "uid-sync".into(),
        name: "SyncEndpointHost".into(),
        address: "10.220.41.10".into(),
        http_port: 47989,
        other_addresses: vec!["10.220.41.20".into()],
        remote_address: Some("8.8.8.8".into()),
        local_endpoint: EndpointSlot::new("10.220.41.10", 47989),
        remote_endpoint: EndpointSlot::new("8.8.8.8", 47989),
        ..HostRecord::default()
    };
    let mut hosts = Hosts::without_discovery(vec![record.clone()], Arc::new(|| {}));
    let id = hosts.list[0].id;

    // Pinning and unpinning updates and clears pinned_endpoint alongside pinned_address.
    assert!(hosts.pin_address(id, Some("10.220.41.20")));
    assert_eq!(hosts.list[0].record.pinned_address.as_deref(), Some("10.220.41.20"));
    assert_eq!(hosts.list[0].record.pinned_endpoint, EndpointSlot::new("10.220.41.20", 47989));
    assert!(hosts.pin_address(id, None));
    assert_eq!(hosts.list[0].record.pinned_address, None);
    assert_eq!(hosts.list[0].record.pinned_endpoint, None);
    // Re-syncing shared fields from slots must keep pinned_address cleared.
    record = hosts.list[0].record.clone();
    record.pinned_address = Some("10.220.41.20".into());
    record.sync_shared_from_slots();
    assert_eq!(record.pinned_address, None);

    // STUN remote update overwrites stale remote_endpoint directly.
    send(&mut hosts, Event::Remote { id, result: Ok(Some(Ipv4Addr::new(1, 1, 1, 1))) });
    assert_eq!(hosts.list[0].record.remote_address.as_deref(), Some("1.1.1.1"));
    assert_eq!(hosts.list[0].record.remote_endpoint, EndpointSlot::new("1.1.1.1", 47989));

    // ServerInfo move_to + external_ip overwrite stale local_endpoint and remote_endpoint.
    let mut info = native_info_of("uid-sync", true);
    info.hostname = "SyncEndpointHost".into();
    info.external_ip = Some("9.9.9.9".into());
    send(
        &mut hosts,
        Event::Info {
            id,
            seq: 1,
            address: "10.220.41.55".into(),
            path: InfoPath::Plain,
            result: Ok(Box::new(info)),
        },
    );
    assert_eq!(hosts.list[0].record.address, "10.220.41.55");
    assert_eq!(hosts.list[0].record.local_endpoint, EndpointSlot::new("10.220.41.55", 47989));
    assert_eq!(hosts.list[0].record.remote_address.as_deref(), Some("9.9.9.9"));
    assert_eq!(hosts.list[0].record.remote_endpoint, EndpointSlot::new("9.9.9.9", 47989));
}
