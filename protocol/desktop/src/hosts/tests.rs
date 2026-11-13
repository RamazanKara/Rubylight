use super::*;

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
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("../../../client/tests/fixtures/native-api/capabilities.json")).unwrap();
    let listed = value["features"].as_array_mut().unwrap();
    listed.extend(features.iter().map(|f| serde_json::Value::from(*f)));
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
    send(hosts, Event::Info { id, seq, address, result: Ok(Box::new(info)) });
}

/// Serverinfo of a paired PC with the native API and the clipboard permission.
pub(crate) fn native_info() -> ServerInfo {
    native_info_of("uid", true)
}

/// `native_info` from the PC `unique_id`, which lists this device as paired or not.
fn native_info_of(unique_id: &str, paired: bool) -> ServerInfo {
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
        send(hosts, Event::Info { id, seq, address: address.into(), result: Ok(Box::new(native_info())) });
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
    send(&mut hosts, Event::Info { id, seq: 1, address: "192.0.2.1".into(), result: Ok(Box::new(native_info())) });
    let host = hosts.get(id).unwrap();
    assert_eq!(host.record.address, "192.0.2.1");
    assert!(host.record.other_addresses.is_empty(), "the remote address stays only the remote one");
    assert_eq!(host.record.remote_address.as_deref(), Some("93.184.216.34"));

    let mut hosts = Hosts::without_discovery(vec![record], Arc::new(|| {}));
    assert!(hosts.apply_discovery("PC".into(), vec!["192.0.2.1".parse().unwrap()], 0));
    assert_eq!(hosts.list[0].record.address, "192.0.2.1");
    assert_eq!(hosts.list[0].connect_address(), "192.0.2.1");
}

fn sight(hosts: &mut Hosts, name: &str, address: &str, port: u16) -> bool {
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

fn paired_record(unique_id: &str) -> HostRecord {
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
    }
}

fn library() -> Library {
    serde_json::from_str(include_str!("../../../client/tests/fixtures/native-api/library.json")).unwrap()
}

fn session(state: &str) -> Session {
    let text = include_str!("../../../client/tests/fixtures/native-api/session.json")
        .replace("\"suspended\"", &format!("\"{state}\""));
    serde_json::from_str(&text).unwrap()
}

fn full_read(session: Session) -> Box<NativeRead> {
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

fn send(hosts: &mut Hosts, event: Event) {
    hosts.sender.send(event).unwrap();
    hosts.drain(&egui::Context::default());
}

#[test]
fn the_hosts_placeholder_box_art_shows_the_title() {
    let placeholder = include_bytes!("../../../client/tests/fixtures/box-placeholder.png").to_vec();
    assert!(decode_image(&placeholder).is_some(), "the placeholder is a real PNG");
    assert!(art_image(Some(placeholder)).is_none(), "and no cover: the card shows the title");
    let mut cover = Vec::new();
    image::RgbaImage::new(30, 40).write_to(&mut std::io::Cursor::new(&mut cover), image::ImageFormat::Png).unwrap();
    assert!(art_image(Some(cover)).is_some());
    assert!(art_image(None).is_none());
}

#[test]
fn a_pinned_address_wins_until_the_pin_is_removed() {
    let record = HostRecord { other_addresses: vec!["10.0.0.5".into(), "fd00::5".into()], ..paired_record("uid") };
    let mut hosts = Hosts::without_discovery(vec![record], Arc::new(|| {}));
    let id = hosts.list[0].id;
    assert_eq!(poll_addresses(&hosts.list[0].record), ["192.0.2.1", "10.0.0.5", "fd00::5"], "automatic: last that answered first");
    assert!(!hosts.pin_address(id, Some("198.51.100.9")), "only a known address is pinned");
    assert!(hosts.pin_address(id, Some("fd00::5")));
    let pinned = &hosts.list[0].record;
    assert_eq!((pinned.address.as_str(), pinned.pinned_address.as_deref()), ("fd00::5", Some("fd00::5")), "streams go to it");
    assert_eq!(poll_addresses(pinned), ["fd00::5"], "the only address polled");
    // An answer from the old address, still in flight when the user pinned, keeps the pin.
    let seq = hosts.get(id).unwrap().info_seq + 1;
    send(&mut hosts, Event::Info { id, seq, address: "192.0.2.1".into(), result: Ok(Box::new(native_info())) });
    assert_eq!(hosts.list[0].record.address, "fd00::5");
    assert!(hosts.pin_address(id, None));
    let automatic = &hosts.list[0].record;
    assert_eq!(automatic.pinned_address, None);
    assert_eq!(poll_addresses(automatic), ["fd00::5", "10.0.0.5", "192.0.2.1"], "back to the automatic pick");
}

fn classic_app(id: i32, name: &str, uuid: &str, control: AppControl) -> App {
    App { name: name.into(), id, hdr_supported: false, uuid: uuid.into(), host_index: None, art_version: format!("v{id}"), control, monitor_resume: false }
}

#[test]
fn a_classic_pcs_app_list_becomes_its_home_library_and_is_saved() {
    let apps = vec![
        classic_app(1, "Desktop", "D-1", AppControl::None),
        classic_app(2, "Hades", "", AppControl::None),
        classic_app(3, "Terminate", "E16CBE1B-295D-4632-9A76-EC4180C857D3", AppControl::Terminate),
    ];
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    info_lands(&mut hosts, id, stock_info());
    assert!(hosts.list[0].library().is_none(), "nothing before the list is read");
    send(&mut hosts, Event::Apps { id, result: Ok(apps.clone()) });
    let pc = &hosts.list[0];
    let games: Vec<_> = pc.library().unwrap().games.iter().map(|g| (g.app_id, g.title.as_str(), g.running)).collect();
    assert_eq!(games, [(1, "Desktop", false), (2, "Hades", false)], "the games, not the control tiles");
    assert_eq!(pc.cover_version(2), "v2", "covers at the list's box art version");
    let source = crate::console::Source::of(pc);
    assert!(source.classic && source.library.is_some());
    let saves = hosts.take_classic_saves();
    assert_eq!(saves.len(), 1);
    assert_eq!((saves[0].0.as_str(), saves[0].1.as_ref().map(|a| a.apps.len())), ("uid", Some(2)));
    send(&mut hosts, Event::Apps { id, result: Ok(apps.clone()) });
    assert!(hosts.take_classic_saves().is_empty(), "the same list is not saved again");

    // currentgame marks the running app while the PC answers; away, nothing runs.
    let mut playing = stock_info();
    playing.current_game = 2;
    info_lands(&mut hosts, id, playing);
    assert!(hosts.list[0].library().unwrap().games[1].running);
    let seq = hosts.list[0].info_seq + 1;
    send(&mut hosts, Event::Info { id, seq, address: "192.0.2.1".into(), result: Err("timed out".into()) });
    assert!(hosts.list[0].library().unwrap().games.iter().all(|g| !g.running));
    assert!(hosts.list[0].library().is_some(), "the list stays for Home while the PC is away");

    // Unpaired, the list goes with the pairing.
    send(&mut hosts, Event::Unpaired { id, result: Ok(()) });
    assert!(hosts.list[0].library().is_none() && hosts.list[0].classic_apps.is_none());

    // A PC with the native library keeps no app list for Home, and one kept is forgotten.
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    hosts.list[0].classic_apps = Some(crate::config::ClassicApps::of(&apps));
    let id = hosts.list[0].id;
    info_lands(&mut hosts, id, native_info());
    send(&mut hosts, Event::Apps { id, result: Ok(apps) });
    assert!(hosts.list[0].classic.is_none() && !crate::console::Source::of(&hosts.list[0]).classic);
    let saves = hosts.take_classic_saves();
    assert_eq!((saves.len(), saves[0].1.is_none()), (1, true), "the saved list is forgotten");
}

#[test]
fn a_saved_app_list_shows_while_the_pc_is_away_unless_a_native_library_is_saved() {
    let (store, dir) = art_store("classic-saved");
    let apps = crate::config::ClassicApps::of(&[classic_app(4, "Celeste", "C-4", AppControl::None)]);
    store.save_classic_apps("uid", &apps).unwrap();
    store.save_classic_apps("native", &apps).unwrap();
    store.save_library("native", &library().into()).unwrap();
    let mut unpaired = paired_record("gone");
    unpaired.server_certificate = None;
    store.save_classic_apps("gone", &apps).unwrap();
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid"), paired_record("native"), unpaired], Arc::new(|| {}));
    hosts.load_cached_libraries(&store);
    let titles = |pc: &Host| pc.library().map(|l| l.games.iter().map(|g| g.title.clone()).collect::<Vec<_>>());
    assert_eq!(titles(&hosts.list[0]), Some(vec!["Celeste".to_owned()]));
    assert!(crate::console::Source::of(&hosts.list[0]).classic);
    assert_ne!(titles(&hosts.list[1]), Some(vec!["Celeste".to_owned()]), "the native library wins");
    assert!(!crate::console::Source::of(&hosts.list[1]).classic);
    assert!(hosts.list[2].library().is_none(), "nothing for a PC not paired");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_native_read_that_lands_after_unpairing_is_dropped() {
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    // The read started while the PC was paired (epoch 0).
    send(&mut hosts, Event::Unpaired { id, result: Ok(()) });
    send(&mut hosts, Event::Native { id, epoch: 0, seq: 1, result: Ok(full_read(session("suspended"))) });
    assert!(hosts.list[0].native.library.is_none(), "the library of an unpaired PC is not taken in");
    assert!(hosts.take_library_saves().is_empty(), "and nothing is saved for it");

    // Even when it was queued for saving before the unpair, an unpaired PC is not saved.
    hosts.list[0].record.server_certificate = Some("pem".into());
    let epoch = hosts.list[0].native_epoch;
    send(&mut hosts, Event::Native { id, epoch, seq: 2, result: Ok(full_read(session("suspended"))) });
    assert!(hosts.list[0].native.library.is_some());
    hosts.list[0].record.server_certificate = None;
    assert!(hosts.take_library_saves().is_empty());
}

#[test]
fn the_certificate_stays_until_the_pc_has_unpaired_the_device() {
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    // No identity: the request cannot be made, and the user is told.
    assert!(hosts.unpair(id).is_err());
    assert!(hosts.list[0].record.server_certificate.is_some());
    // The request failed or never got through.
    send(&mut hosts, Event::Unpaired { id, result: Err(UNPAIR_FAILED.into()) });
    assert!(hosts.list[0].record.server_certificate.is_some(), "a failed unpair keeps the pairing");
    // The PC no longer lists this device, or never did.
    send(&mut hosts, Event::Unpaired { id, result: Err(UNPAIR_NOT_PAIRED.into()) });
    assert!(hosts.list[0].record.server_certificate.is_none());
    hosts.list[0].record.server_certificate = Some("pem".into());
    send(&mut hosts, Event::Unpaired { id, result: Ok(()) });
    assert!(hosts.list[0].record.server_certificate.is_none());
}

#[test]
fn a_session_read_from_before_an_action_does_not_overwrite_its_answer() {
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    // A full read starts (seq 1), then Close game lands and says the session is gone.
    hosts.list[0].native_seq = 1;
    send(&mut hosts, Event::Session { id, action: ActionId::End, result: Ok(Box::new(session("idle"))) });
    assert_eq!(hosts.list[0].native.session.as_ref().map(|s| s.state), Some(session("idle").state));
    // The older read lands afterwards: its library counts, its session does not.
    send(&mut hosts, Event::Native { id, epoch: 0, seq: 1, result: Ok(full_read(session("suspended"))) });
    assert!(hosts.list[0].native.library.is_some());
    assert_eq!(hosts.list[0].native.session.as_ref().map(|s| s.state), Some(session("idle").state));
    // A read that started after the action is newer and replaces it.
    let seq = hosts.list[0].native_seq + 1;
    send(&mut hosts, Event::Native { id, epoch: 0, seq, result: Ok(full_read(session("suspended"))) });
    assert_eq!(hosts.list[0].native.session.as_ref().map(|s| s.state), Some(session("suspended").state));
}

const INFO: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\">        <hostname>Renamed on the PC</hostname><uniqueid>uid</uniqueid><HttpsPort>47984</HttpsPort>        <ExternalPort>47989</ExternalPort><PairStatus>1</PairStatus><currentgame>0</currentgame>        <state>SUNSHINE_SERVER_FREE</state></root>";

const PEM: &str = "-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n";

fn link_naming(cert_sha256: [u8; 32]) -> PairingUri {
    PairingUri {
        host: "203.0.113.9".into(),
        port: 47989,
        otp: "4821".into(),
        passphrase: "hunter2".into(),
        unique_id: String::new(),
        cert_sha256,
        name: "Claimed".into(),
    }
}

fn known_pc_with_link(cert_sha256: [u8; 32]) -> (Hosts, u64) {
    let mut known = paired_record("uid");
    known.server_certificate = Some(PEM.into());
    let mut hosts = Hosts::without_discovery(vec![known], Arc::new(|| {}));
    let id = hosts.add_manual("203.0.113.9", Some(47989));
    hosts.pair_link(id, link_naming(cert_sha256));
    (hosts, id)
}

#[test]
fn a_link_address_claiming_a_known_pc_cannot_move_it() {
    let (mut hosts, id) = known_pc_with_link([9; 32]);
    let info = ServerInfo::parse(INFO, 47989).unwrap();
    assert!(hosts.apply_info(id, "203.0.113.9".into(), Ok(Box::new(info))));
    assert_eq!(hosts.list.len(), 1, "the record the link added is gone");
    let known = &hosts.list[0].record;
    assert_eq!(known.unique_id, "uid");
    assert_eq!(known.address, "192.0.2.1", "the known PC stays where it was");
    assert!(!known.other_addresses.contains(&"203.0.113.9".to_owned()));
    assert_eq!(known.name, "PC");
    assert_eq!(known.server_certificate.as_deref(), Some(PEM));
}

#[test]
fn the_known_pcs_own_link_may_still_find_it_at_a_new_address() {
    let (mut hosts, id) = known_pc_with_link(nightjar_core::http::certificate_sha256(PEM).unwrap());
    let info = ServerInfo::parse(INFO, 47989).unwrap();
    assert!(hosts.apply_info(id, "203.0.113.9".into(), Ok(Box::new(info))));
    assert_eq!(hosts.list.len(), 1, "one PC, as before");
    assert_eq!(hosts.list[0].record.address, "203.0.113.9");
    assert_eq!(hosts.list[0].record.server_certificate.as_deref(), Some(PEM));
}

/// A small PNG of one colour, as a host's box art arrives.
fn png(color: [u8; 3]) -> Vec<u8> {
    png_of([30, 40], color)
}

/// A PNG of one colour and `size` pixels: 600 by 900 is the cover a store hands out.
fn png_of(size: [u32; 2], color: [u8; 3]) -> Vec<u8> {
    let image = image::RgbImage::from_pixel(size[0], size[1], image::Rgb(color));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).expect("a PNG");
    bytes.into_inner()
}

/// A waker that tells the test of each wake, to wait for a cover loaded on the art threads.
fn telling_waker() -> (Waker, Receiver<()>) {
    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let waker: Waker = Arc::new(move || {
        let _ = tx.lock().unwrap().send(());
    });
    (waker, rx)
}

/// An empty settings folder of this test's own.
fn art_store(name: &str) -> (Store, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("nightjar-covers-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    (Store::at(dir.clone()).expect("a settings folder"), dir)
}

fn cover_file(store: &Store, app: i32, bytes: &[u8]) -> std::path::PathBuf {
    let path = store.art_path("uid", app, "v1");
    std::fs::create_dir_all(path.parent().expect("the PC's folder")).expect("the art cache");
    std::fs::write(&path, bytes).expect("the cover's file");
    path
}

fn cover_pixels(pc: &Host, app: i32) -> Option<[usize; 2]> {
    match pc.art.get(&app) {
        Some(Art::Ready(texture)) => Some(texture.size()),
        _ => None,
    }
}

/// Frees the textures a test loaded, as the renderer would.
fn end_pass(ctx: &egui::Context) {
    let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
    output.textures_delta.clear();
}

#[test]
fn a_cover_arrives_with_its_ambient_already_baked() {
    let (image, ambient, sized) = art_and_ambient(Some(png([200, 20, 20])), 1.0);
    assert_eq!(image.expect("the cover").size, [30, 40]);
    assert_eq!(sized, None, "a small cover is never made larger");
    let ambient = ambient.expect("baked with the cover, off the UI thread");
    assert_eq!(ambient, crate::ambient::bake(&decode_image(&png([200, 20, 20])).expect("the cover")));
    assert_eq!(art_and_ambient(None, 1.0), (None, None, None), "no cover, no ambient");
    assert_eq!(art_and_ambient(Some(b"not an image".to_vec()), 1.0), (None, None, None));
}

/// The game page draws a cover 420 by 630 points and nothing draws one larger, so that many
/// points at the window's pixels per point, and half as many again to stay sharp where it
/// is drawn smaller, is all of a cover that is kept.
#[test]
fn a_cover_is_kept_no_larger_than_the_window_needs() {
    assert_eq!(cover_size([1000, 1500], 1.0), Some([630, 945]), "a large cover on a plain screen");
    assert_eq!(cover_size([900, 1350], 1.0), Some([630, 945]));
    assert_eq!(cover_size([900, 1350], 1.25), Some([788, 1182]));
    assert_eq!(cover_size([2000, 3000], 2.0), Some([1260, 1890]), "twice the pixels on a dense screen");
    assert_eq!(cover_size([4000, 6000], 4.5), Some([2835, 4253]), "150 % text on a 300 % screen");
    // Nothing to win, or too little to pay for another resampling of the picture.
    assert_eq!(cover_size([600, 900], 1.0), None, "a store's cover stays as its file has it");
    assert_eq!(cover_size([900, 1350], 1.5), None, "every pixel of it is kept");
    assert_eq!(cover_size([680, 1020], 1.0), None, "nine tenths of its side and more are kept whole");
    assert_eq!(cover_size([30, 40], 1.0), None, "never made larger");
    // A cover fills its place and is cut to it: the side that needs the most decides.
    assert_eq!(cover_size([920, 430], 1.0), None, "a wide banner is short of the place's height as it is");
    assert_eq!(cover_size([3840, 2160], 1.0), Some([1680, 945]), "a screenshot keeps the full height");
    assert_eq!(cover_size([1000, 6000], 1.0), Some([630, 3780]), "and a strip the full width");
    assert_eq!(cover_size([0, 900], 1.0), None);
    assert_eq!(cover_size([600, 0], 1.0), None);
    // The sizes the pages draw: the cards and the PC tile are all below the game page's.
    assert_eq!(COVER_POINTS, [420.0, 630.0]);
}

#[test]
fn a_cover_made_smaller_keeps_its_picture_and_its_ambient() {
    let bytes = png_of([900, 1350], [200, 20, 20]);
    let full = decode_image(&bytes).expect("the cover");
    let (image, ambient, sized) = art_and_ambient(Some(bytes), 1.0);
    let image = image.expect("the cover");
    assert_eq!(image.size, [630, 945]);
    assert_eq!(sized, Some(1.0), "the pixels per point it is good for");
    assert!(image.pixels.iter().all(|&pixel| pixel == egui::Color32::from_rgb(200, 20, 20)), "one colour stays that colour");
    assert_eq!(ambient.expect("the ambient"), crate::ambient::bake(&full), "baked from the cover as its file has it");
    // On a screen that shows every pixel it is handed over as decoded.
    let (same, sized) = cover_sized(full.clone(), 2.0);
    assert_eq!((same, sized), (full, None));
}

/// The pixels are multiplied by their alpha before they are resampled, so the colour a clear
/// pixel happens to have does not show as a fringe around the picture.
#[test]
fn a_clear_part_of_a_cover_does_not_bleed_into_the_picture_when_it_is_made_smaller() {
    let picture = image::RgbaImage::from_fn(900, 1350, |x, _| {
        if x < 450 { image::Rgba([200, 0, 0, 255]) } else { image::Rgba([0, 255, 0, 0]) }
    });
    let (small, sized) = cover_sized(egui::ColorImage::from_rgba_unmultiplied([900, 1350], picture.as_raw()), 1.0);
    assert_eq!((small.size, sized), ([630, 945], Some(1.0)));
    assert!(small.pixels.iter().all(|pixel| pixel.g() == 0), "no green from the clear half");
    assert_eq!(small.pixels[0], egui::Color32::from_rgb(200, 0, 0));
    assert_eq!(small.pixels[629], egui::Color32::TRANSPARENT);
}

#[test]
fn the_ambient_lands_with_its_cover_and_goes_with_it() {
    let ctx = egui::Context::default();
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    hosts.list[0].art_versions.insert(7, "v1".into());
    let art = |version: &str, color: Option<[u8; 3]>| {
        let (image, ambient, sized) = art_and_ambient(color.map(png), 1.0);
        Event::Art { id, app: 7, version: version.into(), image, ambient, sized }
    };
    hosts.sender.send(art("v1", Some([20, 200, 20]))).expect("the channel");
    hosts.drain(&ctx);
    assert!(hosts.list[0].ambient.contains_key(&7));
    assert!(hosts.list[0].ambient_glow[&7].a() > 0, "its glow lands with it");
    assert!(matches!(hosts.list[0].art.get(&7), Some(Art::Ready(_))));

    hosts.sender.send(art("old", None)).expect("the channel");
    hosts.drain(&ctx);
    assert!(hosts.list[0].ambient.contains_key(&7), "an answer for an older cover changes nothing");

    hosts.sender.send(art("v1", None)).expect("the channel");
    hosts.drain(&ctx);
    assert!(!hosts.list[0].ambient.contains_key(&7), "no cover any more, no ambient of it");
    assert!(!hosts.list[0].ambient_glow.contains_key(&7), "nor its glow");
    let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
    output.textures_delta.clear();
}

/// Linux's switch to the GPU renderer: covers on the old renderer are dropped, so they load
/// again from the art cache; no copy of them is kept.
#[test]
fn a_new_renderer_drops_the_covers_so_they_load_again() {
    let ctx = egui::Context::default();
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    hosts.list[0].art_versions.insert(7, "v1".into());
    let (image, ambient, sized) = art_and_ambient(Some(png([20, 200, 20])), 1.0);
    hosts.sender.send(Event::Art { id, app: 7, version: "v1".into(), image, ambient, sized }).expect("the channel");
    hosts.drain(&ctx);
    hosts.list[0].art.insert(8, Art::Missing);
    hosts.list[0].art.insert(9, Art::Loading);
    hosts.list[0].art.insert(10, Art::Dropped);
    hosts.renderer_changed();
    let pc = &hosts.list[0];
    assert!(!pc.art.contains_key(&7) && !pc.ambient.contains_key(&7), "asked for again, from the cache");
    assert!(pc.ambient_glow.contains_key(&7) && pc.art_versions.contains_key(&7), "no texture: kept");
    assert!(matches!(pc.art.get(&8), Some(Art::Missing)), "no cover is still no cover");
    assert!(matches!(pc.art.get(&9), Some(Art::Loading)), "one on its way lands as it is");
    assert!(pc.art_wanted(10, ""), "one given up for room is asked for again as before");
    assert!(hosts.art_used.is_empty(), "the old renderer's textures are gone");
    end_pass(&ctx);
}

/// Four covers of 30 by 40 pixels on one PC, landed and then shown for as many frames as
/// `shown` lists, each frame drawing the apps it names.
fn covers_shown_in(name: &str, shown: &[&[i32]]) -> (Hosts, egui::Context, Store, std::path::PathBuf) {
    let ctx = egui::Context::default();
    let (store, dir) = art_store(name);
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    for app in 1..=4 {
        hosts.list[0].art_versions.insert(app, "v1".into());
        let (image, ambient, sized) = art_and_ambient(Some(png([20, 200, 20])), 1.0);
        hosts.sender.send(Event::Art { id, app, version: "v1".into(), image, ambient, sized }).expect("the channel");
    }
    hosts.drain(&ctx);
    for frame in shown {
        for app in *frame {
            let Some(Art::Ready(texture)) = hosts.list[0].art.get(app) else { panic!("cover {app} is not there to draw") };
            cover_used(&ctx, texture);
        }
        hosts.covers_shown(&ctx, &store);
    }
    (hosts, ctx, store, dir)
}

const COVER_BYTES: usize = 30 * 40 * 4;

#[test]
fn covers_past_their_budget_are_given_up_longest_off_screen_first() {
    let (mut hosts, ctx, _store, dir) = covers_shown_in("budget", &[&[1, 2, 3, 4], &[2, 3, 4], &[3, 4]]);
    assert!((1..=4).all(|app| cover_pixels(&hosts.list[0], app).is_some()), "all within the real budget");

    // Room for two and a bit: the two longest off screen go, and no more than needed.
    hosts.drop_covers(2 * COVER_BYTES + 100);
    let pc = &hosts.list[0];
    assert!(pc.art_dropped(1) && pc.art_dropped(2), "last shown two frames and one frame ago");
    assert!(cover_pixels(pc, 3).is_some() && cover_pixels(pc, 4).is_some());
    assert_eq!(hosts.art_used.len(), 2, "what is gone is no longer kept count of");
    // What it leaves: the page's backdrop, and the version its file is found by.
    assert!(pc.ambient.contains_key(&1) && pc.ambient_glow.contains_key(&1));
    assert_eq!(pc.art_versions[&1], "v1");

    // A cover that is given up is asked for again where it is drawn, as it was.
    let pc = &mut hosts.list[0];
    assert!(pc.art_wanted(1, "") && pc.art_wanted(1, "v1"));
    assert!(!pc.art_wanted(3, "") && !pc.art_wanted(3, "v1"));
    assert_eq!(pc.begin_art(1, "").as_deref(), Some("v1"));
    assert!(matches!(pc.art.get(&1), Some(Art::Loading)) && !pc.art_wanted(1, "v1"), "asked for once");
    end_pass(&ctx);
    let _ = std::fs::remove_dir_all(dir);
}

/// Dropping what is on screen would have it loaded again at once and something else dropped
/// for it: a page with more covers than the budget holds would load and drop in turns.
#[test]
fn a_cover_on_screen_is_never_given_up() {
    let (mut hosts, ctx, store, dir) = covers_shown_in("on-screen", &[&[1, 2, 3, 4], &[3, 4]]);
    hosts.drop_covers(0);
    let pc = &hosts.list[0];
    assert!(pc.art_dropped(1) && pc.art_dropped(2));
    assert!(cover_pixels(pc, 3).is_some() && cover_pixels(pc, 4).is_some(), "drawn in the frame just shown");
    // A frame that draws neither: no cover landed, so nothing is given up, and the textures
    // stay for the page that is gone back to. The next cover to land makes room with them.
    hosts.covers_shown(&ctx, &store);
    assert!(cover_pixels(&hosts.list[0], 3).is_some() && cover_pixels(&hosts.list[0], 4).is_some());
    hosts.drop_covers(0);
    assert!(hosts.list[0].art_dropped(3) && hosts.list[0].art_dropped(4));
    end_pass(&ctx);
    let _ = std::fs::remove_dir_all(dir);
}

/// A cover lands before the frame it was asked for, and by then its card may have scrolled
/// away. It counts as shown when it lands: without that it would be the first to go, and be
/// asked for and dropped in turns by a page that asks for a cover it does not draw.
#[test]
fn a_cover_that_just_landed_is_not_the_first_to_go() {
    let (mut hosts, ctx, store, dir) = covers_shown_in("landed", &[&[1, 2, 3, 4], &[3, 4]]);
    let id = hosts.list[0].id;
    hosts.list[0].art_versions.insert(5, "v1".into());
    let (image, ambient, sized) = art_and_ambient(Some(png([20, 20, 200])), 1.0);
    hosts.sender.send(Event::Art { id, app: 5, version: "v1".into(), image, ambient, sized }).expect("the channel");
    hosts.drain(&ctx);
    // The frame after draws nothing at all.
    hosts.covers_shown(&ctx, &store);
    hosts.drop_covers(3 * COVER_BYTES);
    let pc = &hosts.list[0];
    assert!(pc.art_dropped(1) && pc.art_dropped(2), "the two that were off screen before it came");
    assert!((3..=5).all(|app| cover_pixels(pc, app).is_some()));
    end_pass(&ctx);
    let _ = std::fs::remove_dir_all(dir);
}

/// Its PC may be off by the time a cover scrolls back in. The file it was saved to is all
/// that is read: this PC's address answers nobody.
#[test]
fn a_cover_given_up_comes_back_from_its_file_without_its_pc() {
    let ctx = egui::Context::default();
    let (store, dir) = art_store("back");
    let (waker, woken) = telling_waker();
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], waker);
    hosts.set_identity(Arc::new(ClientIdentity::generate().expect("a key")));
    let id = hosts.list[0].id;
    cover_file(&store, 7, &png_of([900, 1350], [20, 200, 20]));
    hosts.list[0].art_versions.insert(7, "v1".into());
    hosts.list[0].art.insert(7, Art::Dropped);
    assert!(hosts.list[0].reach != Reach::Online, "away");

    hosts.request_art(id, 7, "", &store);
    assert!(matches!(hosts.list[0].art.get(&7), Some(Art::Loading)));
    hosts.request_art(id, 7, "", &store);
    woken.recv_timeout(Duration::from_secs(20)).expect("the cover is read from its file");
    hosts.drain(&ctx);
    assert_eq!(cover_pixels(&hosts.list[0], 7), Some([630, 945]), "and sized for the window as before");
    assert_eq!(hosts.list[0].art_sized.get(&7), Some(&1.0));
    assert!(woken.try_recv().is_err(), "asked for twice, loaded once");
    end_pass(&ctx);
    let _ = std::fs::remove_dir_all(dir);
}

/// The file can be gone (the art folder cleared, or a disk too full to save it). An away PC
/// is not asked, which would hold an art thread until the connection gives up, and the cover
/// is not taken as one the PC has none of: it is asked for again once the PC is up.
#[test]
fn a_cover_given_up_whose_file_is_gone_waits_for_its_pc() {
    let ctx = egui::Context::default();
    let (store, dir) = art_store("gone");
    let (waker, woken) = telling_waker();
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], waker);
    hosts.set_identity(Arc::new(ClientIdentity::generate().expect("a key")));
    let id = hosts.list[0].id;
    hosts.list[0].art_versions.insert(7, "v1".into());
    hosts.list[0].art.insert(7, Art::Dropped);
    hosts.list[0].ambient_glow.insert(7, egui::Color32::RED);
    hosts.request_art(id, 7, "", &store);
    woken.recv_timeout(Duration::from_secs(20)).expect("its file is looked for");
    hosts.drain(&ctx);
    let pc = &hosts.list[0];
    assert!(!pc.art.contains_key(&7), "neither a cover nor known to have none");
    assert!(!pc.art_dropped(7), "so the pages do not ask for it while the PC is away");
    assert!(pc.art_wanted(7, "v1") && pc.ambient_glow.contains_key(&7), "and do when it is back");
    end_pass(&ctx);
    let _ = std::fs::remove_dir_all(dir);
}

/// A larger text size, or the window moved to a denser screen: the covers made smaller for
/// the old one are read again from their files and swapped in where they are, with no tile
/// in between.
#[test]
fn a_cover_made_smaller_is_loaded_sharper_when_the_window_shows_more_pixels() {
    let ctx = egui::Context::default();
    let (store, dir) = art_store("sharper");
    let (waker, woken) = telling_waker();
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], waker);
    let id = hosts.list[0].id;
    let bytes = png_of([900, 1350], [20, 200, 20]);
    let file = cover_file(&store, 7, &bytes);
    hosts.list[0].art_versions.insert(7, "v1".into());
    hosts.covers_shown(&ctx, &store);
    assert_eq!(hosts.art_scale, 1.0);
    let (image, ambient, sized) = art_and_ambient(Some(bytes), hosts.art_scale);
    hosts.sender.send(Event::Art { id, app: 7, version: "v1".into(), image, ambient, sized }).expect("the channel");
    hosts.drain(&ctx);
    hosts.covers_shown(&ctx, &store);
    assert_eq!(cover_pixels(&hosts.list[0], 7), Some([630, 945]));
    assert_eq!(hosts.list[0].art_sized.get(&7), Some(&1.0));

    // 125 %: still smaller than its file, and sharper than it was.
    ctx.set_pixels_per_point(1.25);
    end_pass(&ctx);
    assert_eq!(ctx.pixels_per_point(), 1.25);
    hosts.covers_shown(&ctx, &store);
    assert_eq!(cover_pixels(&hosts.list[0], 7), Some([630, 945]), "it shows as it is until the sharper one lands");
    hosts.covers_shown(&ctx, &store);
    woken.recv_timeout(Duration::from_secs(20)).expect("the cover is read again from its file");
    hosts.drain(&ctx);
    assert_eq!(cover_pixels(&hosts.list[0], 7), Some([788, 1182]));
    assert_eq!(hosts.list[0].art_sized.get(&7), Some(&1.25));
    assert!(woken.try_recv().is_err(), "asked for once, though two frames passed before it landed");

    // 200 %: all of the file is kept, and there is nothing left to load after it.
    ctx.set_pixels_per_point(2.0);
    end_pass(&ctx);
    hosts.covers_shown(&ctx, &store);
    woken.recv_timeout(Duration::from_secs(20)).expect("read again");
    hosts.drain(&ctx);
    assert_eq!(cover_pixels(&hosts.list[0], 7), Some([900, 1350]));
    assert!(!hosts.list[0].art_sized.contains_key(&7));

    // Back to 100 % it stays: sharper than needed, and the budget is what bounds it.
    ctx.set_pixels_per_point(1.0);
    end_pass(&ctx);
    hosts.covers_shown(&ctx, &store);
    assert_eq!(cover_pixels(&hosts.list[0], 7), Some([900, 1350]));
    let _ = std::fs::remove_file(file);
    end_pass(&ctx);
    let _ = std::fs::remove_dir_all(dir);
}

/// The sharper cover was read for the cover that was there when it was asked for.
#[test]
fn a_sharper_cover_lands_only_on_the_cover_it_was_read_for() {
    let ctx = egui::Context::default();
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    hosts.list[0].art_versions.insert(7, "v1".into());
    let sharper = |version: &str| {
        let image = decode_image(&png_of([60, 80], [20, 20, 200])).expect("the cover");
        Event::ArtSharper { id, app: 7, version: version.into(), image, sized: None }
    };
    let (image, ambient, _) = art_and_ambient(Some(png([20, 200, 20])), 1.0);
    hosts.sender.send(Event::Art { id, app: 7, version: "v1".into(), image, ambient, sized: Some(1.0) }).expect("the channel");
    hosts.sender.send(sharper("v0")).expect("the channel");
    hosts.drain(&ctx);
    assert_eq!(cover_pixels(&hosts.list[0], 7), Some([30, 40]), "the PC has another cover by now");
    assert_eq!(hosts.list[0].art_sized.get(&7), Some(&1.0));

    hosts.sender.send(sharper("v1")).expect("the channel");
    hosts.drain(&ctx);
    assert_eq!(cover_pixels(&hosts.list[0], 7), Some([60, 80]));
    assert!(!hosts.list[0].art_sized.contains_key(&7), "all of its file now");
    assert!(hosts.list[0].ambient.contains_key(&7), "the ambient is the cover's own and stays");

    // Given up for room in between, it is not brought back by a read that is late.
    hosts.list[0].art.insert(7, Art::Dropped);
    hosts.sender.send(sharper("v1")).expect("the channel");
    hosts.drain(&ctx);
    assert!(hosts.list[0].art_dropped(7));
    hosts.list[0].art.insert(7, Art::Missing);
    hosts.sender.send(sharper("v1")).expect("the channel");
    hosts.drain(&ctx);
    assert!(matches!(hosts.list[0].art.get(&7), Some(Art::Missing)));
    end_pass(&ctx);
}

/// The Apps page asked for every cover without a version each pass, and the pages that knew
/// one asked with it: each dropped the other's cover and fetched it again.
#[test]
fn a_cover_asked_for_without_a_version_keeps_the_known_one() {
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let pc = &mut hosts.list[0];
    assert!(pc.art_wanted(7, "") && pc.art_wanted(7, "v1"), "not asked for yet");
    assert_eq!(pc.begin_art(7, "v1").as_deref(), Some("v1"));
    pc.ambient_glow.insert(7, egui::Color32::RED);

    assert!(!pc.art_wanted(7, "") && !pc.art_wanted(7, "v1"));
    assert_eq!(pc.begin_art(7, ""), None, "no version says nothing of the cover");
    assert_eq!(pc.begin_art(7, "v1"), None);
    assert_eq!(pc.art_versions[&7], "v1");
    assert!(matches!(pc.art.get(&7), Some(Art::Loading)) && pc.ambient_glow.contains_key(&7), "kept");

    // Dropped with its renderer, it loads as the cover it was: from the file of that version.
    pc.art.remove(&7);
    assert!(pc.art_wanted(7, ""));
    assert_eq!(pc.begin_art(7, "").as_deref(), Some("v1"));

    assert!(pc.art_wanted(7, "v2"), "a new version is another cover");
    assert_eq!(pc.begin_art(7, "v2").as_deref(), Some("v2"));
    assert_eq!(pc.art_versions[&7], "v2");
    assert!(!pc.ambient_glow.contains_key(&7), "the old cover's glow goes with it");

    // A PC that gives its covers no version: asked for once.
    assert_eq!(pc.begin_art(8, "").as_deref(), Some(""));
    assert!(!pc.art_wanted(8, ""));
    assert_eq!(pc.begin_art(8, ""), None);
}

#[test]
fn every_page_asks_for_a_cover_at_one_version() {
    let app = |id: i32, art_version: &str| App {
        name: "Game".into(),
        id,
        hdr_supported: false,
        uuid: String::new(),
        host_index: None,
        art_version: art_version.into(),
        control: AppControl::None,
        monitor_resume: false,
    };
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let pc = &mut hosts.list[0];
    assert_eq!(pc.cover_version(7), "", "nothing read yet");
    pc.apps = Apps::Loaded(vec![app(7, "7"), app(8, "8"), app(9, "")]);
    assert_eq!((pc.cover_version(7), pc.cover_version(8), pc.cover_version(9)), ("7", "8", ""));
    assert_eq!(pc.cover_version(10), "", "not an app of this PC");

    let mut library = library();
    let listed = library.games[0].app_id as i32;
    library.games[0].artwork.version = "a41c09e2".into();
    pc.apps = Apps::Loaded(vec![app(listed, "7"), app(listed + 1000, "8")]);
    pc.native.library = Some(library);
    assert_eq!(pc.cover_version(listed), "a41c09e2", "the library's, whatever the app list says");
    assert_eq!(pc.cover_version(listed + 1000), "8", "the app list's for an app the library leaves out");
    pc.apps = Apps::Loading;
    assert_eq!(pc.cover_version(listed), "a41c09e2");
}

/// A long app list asked for all its covers at once, a thread and a TLS handshake each.
#[test]
fn covers_load_a_few_at_a_time_and_all_of_them() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Condvar;

    let wait_for = |what: &str, met: &dyn Fn() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !met() {
            assert!(Instant::now() < deadline, "{what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    };
    let pool = ArtPool::default();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (started, done) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let asked = ART_WORKERS * 3;
    for cover in 0..asked {
        let (gate, started, done) = (gate.clone(), started.clone(), done.clone());
        pool.run(move || {
            started.fetch_add(1, Ordering::SeqCst);
            let (open, opened) = &*gate;
            drop(opened.wait_while(open.lock().unwrap(), |open| !*open).unwrap());
            done.fetch_add(1, Ordering::SeqCst);
            assert!(cover != 1, "a cover that panics takes no other with it");
        });
    }
    wait_for("the first covers start", &|| started.load(Ordering::SeqCst) == ART_WORKERS);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(started.load(Ordering::SeqCst), ART_WORKERS, "the rest wait their turn");
    let (workers, waiting) = {
        let queue = pool.queue();
        (queue.workers, queue.waiting.len())
    };
    assert_eq!((workers, waiting), (ART_WORKERS, asked - ART_WORKERS));

    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    wait_for("every cover loads", &|| done.load(Ordering::SeqCst) == asked);
    wait_for("the workers end with the last cover", &|| pool.queue().workers == 0);

    // An idle pool starts again for the next cover.
    let (again, landed) = (done.clone(), done.clone());
    pool.run(move || {
        again.fetch_add(1, Ordering::SeqCst);
    });
    wait_for("a cover asked for later loads", &|| landed.load(Ordering::SeqCst) == asked + 1);
}

#[test]
fn a_link_to_a_known_address_with_another_certificate_is_refused_at_once() {
    let mut known = paired_record("uid");
    known.server_certificate = Some(PEM.into());
    known.http_port = 47989;
    let mut hosts = Hosts::without_discovery(vec![known], Arc::new(|| {}));
    let id = hosts.list[0].id;
    hosts.pair_link(id, link_naming([9; 32]));
    assert!(matches!(
        hosts.events.try_recv(),
        Ok(Event::Paired { id: refused, result: Err(why), .. }) if refused == id && why == PAIR_LINK_MISMATCH
    ));
    assert!(hosts.link_claims.is_empty());
    assert_eq!(hosts.list[0].record.server_certificate.as_deref(), Some(PEM));
}

#[test]
fn a_pc_paired_nowhere_takes_any_link_and_a_typed_address_is_as_before() {
    assert!(!pins_another(&HostRecord::default(), &[9; 32]));
    let mut record = paired_record("uid");
    record.server_certificate = Some("not a certificate".into());
    assert!(pins_another(&record, &[9; 32]), "unreadable counts as another");
    let mut known = paired_record("uid");
    known.server_certificate = Some(PEM.into());
    let mut hosts = Hosts::without_discovery(vec![known], Arc::new(|| {}));
    let id = hosts.add_manual("203.0.113.9", Some(47989));
    let info = ServerInfo::parse(INFO, 47989).unwrap();
    hosts.apply_info(id, "203.0.113.9".into(), Ok(Box::new(info)));
    assert_eq!(hosts.list[0].record.address, "203.0.113.9", "no link, no claim");
}

#[test]
fn a_name_chosen_here_survives_the_pc_reporting_another() {
    let mut record = paired_record("uid");
    record.set_alias("Living room");
    let mut hosts = Hosts::without_discovery(vec![record], Arc::new(|| {}));
    let id = hosts.list[0].id;
    let info = ServerInfo::parse(INFO, 47989).unwrap();
    assert!(hosts.apply_info(id, "192.0.2.1".into(), Ok(Box::new(info))));
    let host = &hosts.list[0];
    assert_eq!(host.record.name, "Renamed on the PC", "the PC's own name keeps following the PC");
    assert_eq!(host.display_name(), "Living room");
    assert_eq!(hosts.records()[0].alias.as_deref(), Some("Living room"), "and the alias is saved");
    hosts.list[0].record.set_alias("");
    assert_eq!(hosts.list[0].display_name(), "Renamed on the PC");
}

#[test]
fn names_a_pc_reports_or_is_given_later_are_kept_out_of_the_log() {
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    let info = INFO.replace("Renamed on the PC", "Secret name from serverinfo").replace("<uniqueid>uid", "<uniqueid>secret-uid-from-serverinfo");
    assert!(hosts.apply_info(id, "192.0.2.1".into(), Ok(Box::new(ServerInfo::parse(&info, 47989).unwrap()))));
    assert!(hosts.rename(id, "Secret alias given here"));
    hosts.apply_discovery("Secret name from mDNS".into(), vec!["192.0.2.99".parse().unwrap()], 0);
    let line = nightjar_core::report::redact::scrub("Secret name from serverinfo, secret-uid-from-serverinfo, Secret alias given here, Secret name from mDNS");
    // Other tests save a PC named "PC": it does not mark the marks again.
    assert_eq!(line, "[PC], [PC], [PC], [PC]");
}

#[test]
fn pairing_texts_follow_the_android_wording() {
    let one_time = pair_outcome(PairResult::PinWrong, true).unwrap_err();
    let plain = pair_outcome(PairResult::PinWrong, false).unwrap_err();
    assert_eq!(one_time, PAIR_OTP_PIN_WRONG);
    assert_eq!(plain, PAIR_PIN_WRONG);
    assert_eq!(pair_outcome(PairResult::AlreadyInProgress, false).unwrap_err(), PAIR_IN_PROGRESS);
    let io = nightjar_core::http::Error::Io(std::io::ErrorKind::TimedOut.into());
    assert_eq!(pair_error(&io), PAIR_NETWORK);
    assert_eq!(unpair_error(&nightjar_core::http::Error::NotPaired), UNPAIR_NOT_PAIRED);
    assert_eq!(unpair_error(&io), UNPAIR_FAILED);

    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    assert_eq!(hosts.list[0].pairing_blocked(), None);
    hosts.list[0].reach = Reach::Offline;
    assert_eq!(hosts.list[0].pairing_blocked(), Some(PAIR_OFFLINE));
    hosts.list[0].reach = Reach::Online;
    let busy = INFO.replace("<currentgame>0", "<currentgame>7").replace("SERVER_FREE", "SERVER_BUSY");
    hosts.list[0].info = Some(ServerInfo::parse(&busy, 47989).unwrap());
    assert_eq!(hosts.list[0].pairing_blocked(), Some(PAIR_IN_GAME));
}

#[test]
fn merging_two_records_of_one_pc_keeps_the_name_chosen_here() {
    let mut named = paired_record("uid");
    named.set_alias("Living room");
    named.address = "192.0.2.20".into();
    let mut plain = paired_record("uid");
    plain.server_certificate = None;
    plain.address = "192.0.2.21".into();
    for (first, second) in [(named.clone(), plain.clone()), (plain.clone(), named.clone())] {
        let mut hosts = Hosts::without_discovery(vec![first, second], Arc::new(|| {}));
        let id = hosts.list[0].id;
        let info = ServerInfo::parse(INFO, 47989).unwrap();
        assert!(hosts.apply_info(id, hosts.list[0].record.address.clone(), Ok(Box::new(info))));
        assert_eq!(hosts.list.len(), 1, "the two records are one PC");
        assert_eq!(hosts.list[0].display_name(), "Living room");
    }
}

fn info_event(id: u64, seq: u64, game: i32) -> Event {
    let text = INFO
        .replace("<currentgame>0", &format!("<currentgame>{game}"))
        .replace("SERVER_FREE", if game == 0 { "SERVER_FREE" } else { "SERVER_BUSY" });
    Event::Info { id, seq, address: "192.0.2.1".into(), result: Ok(Box::new(ServerInfo::parse(&text, 47989).unwrap())) }
}

#[test]
fn an_older_serverinfo_answer_does_not_bring_back_a_finished_game() {
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    send(&mut hosts, info_event(id, 1, 7));
    assert_eq!(hosts.list[0].running_app(), Some(7));
    // The poll after the quit answers first; the one from before it lands late.
    send(&mut hosts, info_event(id, 3, 0));
    send(&mut hosts, info_event(id, 2, 7));
    assert_eq!(hosts.list[0].running_app(), None, "the app list must not show the quit game as running");
}

#[test]
fn the_failure_reason_comes_only_from_the_poll_after_the_stream() {
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    let failed = |seq| {
        let text = INFO.replace("<state>", "<RustHostLastFailure>HDR is on, but the display is not in HDR</RustHostLastFailure><state>");
        Event::Info { id, seq, address: "192.0.2.1".into(), result: Ok(Box::new(ServerInfo::parse(&text, 47989).unwrap())) }
    };
    // An answer from before the stream says nothing about it.
    send(&mut hosts, failed(1));
    assert_eq!(hosts.failure_after(id, 2), None, "still waiting for the poll after the stream");
    send(&mut hosts, failed(2));
    assert_eq!(hosts.failure_after(id, 2), Some(Some("HDR is on, but the display is not in HDR".into())));
    // A PC that gives no reason, or could not be asked, ends the wait without one.
    send(&mut hosts, info_event(id, 3, 0));
    assert_eq!(hosts.failure_after(id, 3), Some(None));
    send(&mut hosts, Event::Info { id, seq: 4, address: "192.0.2.1".into(), result: Err("timed out".into()) });
    assert_eq!(hosts.failure_after(id, 4), Some(None));
}

#[test]
fn a_quit_a_closed_game_and_a_stream_end_ask_the_pc_again_for_a_while() {
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let id = hosts.list[0].id;
    let now = Instant::now();
    assert_eq!(hosts.list[0].poll_interval(now), POLL_INTERVAL);
    for event in [
        Event::Quit { id, result: Ok(()) },
        Event::Quit { id, result: Err("declined".into()) },
        Event::Session { id, action: ActionId::CloseGame, result: Ok(Box::new(session("idle"))) },
        Event::Action { id, result: Ok(()) },
    ] {
        hosts.list[0].recheck_until = None;
        send(&mut hosts, event);
        assert_eq!(hosts.list[0].poll_interval(Instant::now()), RECHECK_INTERVAL);
    }
    // The same entry point serves the end of a stream.
    hosts.list[0].recheck_until = None;
    hosts.refresh_after_exit(id);
    let until = hosts.list[0].recheck_until.expect("a stream's end arms the recheck");
    assert_eq!(hosts.list[0].poll_interval(until), POLL_INTERVAL, "and it ends");
}

#[test]
fn the_library_is_read_again_every_few_seconds_while_rechecking_only() {
    let mut hosts = Hosts::without_discovery(vec![paired_record("uid")], Arc::new(|| {}));
    let now = Instant::now();
    let host = &mut hosts.list[0];
    host.native.full_at = Some(now);
    assert!(!host.library_recheck_due(now), "outside the window the 30 s timer rules");
    host.recheck_until = Some(now + RECHECK_WINDOW);
    assert!(!host.library_recheck_due(now + RECHECK_LIBRARY_INTERVAL / 2), "just read");
    assert!(host.library_recheck_due(now + RECHECK_LIBRARY_INTERVAL), "read again after a few seconds");
    host.native.full_at = None;
    assert!(host.library_recheck_due(now), "never read: read now");
    assert!(!host.library_recheck_due(now + RECHECK_WINDOW), "the window ends");
}

#[test]
fn pins_are_four_digits() {
    for _ in 0..20 {
        let pin = random_pin();
        assert_eq!(pin.len(), 4);
        assert!(pin.bytes().all(|b| b.is_ascii_digit()));
    }
}

#[test]
fn action_completion_requires_a_known_message_for_the_selected_action() {
    use nightjar_core::http::Error;
    let terminated = Error::Status { code: 410, message: "Application terminated.".into() };
    assert!(completed_action(AppControl::Terminate, &terminated));
    assert!(!completed_action(AppControl::DisconnectInput, &terminated));
    assert!(!completed_action(AppControl::Terminate, &Error::Status { code: 410, message: "Confirm termination".into() }));
    assert!(!completed_action(AppControl::Terminate, &Error::Status { code: 500, message: "Application terminated".into() }));
}

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
    Ok(power_reply(202, "Sleep request accepted; native completion is not confirmed", [true, true, false], None))
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
        Err(Error::Api { status: 409, error })
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
