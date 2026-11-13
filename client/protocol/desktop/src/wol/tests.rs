use super::*;
use nightjar_core::http::native_api::wake::Target;

#[test]
fn packet_repeats_the_mac_sixteen_times() {
    let packet = magic_packet("00:1a:2B:3c:4D:5e").unwrap();
    assert_eq!(&packet[..6], &[255; 6]);
    for chunk in packet[6..].as_chunks::<6>().0 {
        assert_eq!(chunk, &[0, 0x1a, 0x2b, 0x3c, 0x4d, 0x5e]);
    }
    assert_eq!(magic_packet("00-1a-2b-3c-4d-5e").unwrap(), packet);
    for mac in [
        "",
        "00:1a:2b:3c:4d",
        "00:1a-2b:3c:4d:5e",
        "00:1a:2b:3c:4d:zz",
        "é0:1a:2b:3c:4d:5",
    ] {
        assert!(magic_packet(mac).is_err());
    }
}

#[test]
fn custom_ports_keep_fixed_wake_ports_and_do_not_wrap() {
    assert_eq!(ports(0), [7, 9, 47009, 47998, 47999, 48000, 48002, 48010]);
    assert_eq!(
        ports(48989),
        [7, 9, 47009, 48998, 48999, 49000, 49002, 49010]
    );
    assert_eq!(ports(65535), [7, 9, 47009]);
}

fn host(mac: Option<&str>, wake_macs: &[&str], wake_broadcasts: &[&str]) -> HostRecord {
    HostRecord {
        address: "192.168.4.20".into(),
        other_addresses: vec!["10.0.0.20".into()],
        mac: mac.map(Into::into),
        wake_macs: wake_macs.iter().map(|m| m.to_string()).collect(),
        wake_broadcasts: wake_broadcasts.iter().map(|b| b.to_string()).collect(),
        ..Default::default()
    }
}

fn links() -> Links {
    Links {
        broadcasts: BTreeSet::from([Ipv4Addr::new(192, 168, 4, 255)]),
        ipv6: BTreeSet::from([7]),
    }
}

fn v4(address: &str, port: u16) -> SocketAddr {
    SocketAddr::new(address.parse().unwrap(), port)
}

#[test]
fn typed_macs_are_read_in_every_usual_form_once() {
    assert_eq!(
        parse_macs(" 00:11:22:33:44:55\n0A-1B-2C-3D-4E-5F, AABBCCDDEEFF;00-11-22-33-44-55 ").unwrap(),
        ["00:11:22:33:44:55", "0a:1b:2c:3d:4e:5f", "aa:bb:cc:dd:ee:ff"]
    );
    assert_eq!(parse_macs("  \n "), Some(Vec::new()));
    for wrong in ["00:11:22:33:44", "00:11-22:33:44:55", "00:00:00:00:00:00", "the PC", "ÄÄ:11:22:33:44:55"] {
        assert_eq!(parse_macs(wrong), None, "{wrong}");
    }
    let nine: String = (0..=MAX_MACS).map(|i| format!("02:00:00:00:00:0{i}\n")).collect();
    assert_eq!(parse_macs(&nine), None);
}

#[test]
fn the_users_own_macs_wake_the_pc_too() {
    let mut host = host(None, &[], &[]);
    assert!(!host.can_wake());
    assert_eq!(plan(&host, &links()).err().as_deref(), Some(NO_MAC));
    host.own_wake_macs = vec!["0a:00:27:00:00:05".into()];
    assert!(host.can_wake());
    host.mac = Some("02:7c:16:b4:13:4f".into());
    host.wake_macs = vec!["0a:00:27:00:00:05".into()];
    let plan = plan(&host, &links()).unwrap();
    assert_eq!(
        plan.payloads,
        [
            magic_packet("02:7c:16:b4:13:4f").unwrap(),
            magic_packet("0a:00:27:00:00:05").unwrap()
        ]
    );
}

#[test]
fn two_macs_go_to_every_address_broadcast_and_port() {
    let host = host(
        Some("02-7C-16-B4-13-4F"),
        &["02:7c:16:b4:13:4f", "0a:00:27:00:00:05"],
        &["10.0.0.255", "192.168.4.255"],
    );
    let plan = plan(&host, &links()).unwrap();
    assert_eq!(
        plan.payloads,
        [
            magic_packet("02:7c:16:b4:13:4f").unwrap(),
            magic_packet("0a:00:27:00:00:05").unwrap()
        ]
    );
    let addresses: BTreeSet<_> = plan
        .destinations
        .iter()
        .map(|d| d.ip().to_string())
        .collect();
    assert_eq!(
        addresses,
        [
            "10.0.0.20",
            "10.0.0.255",
            "192.168.4.20",
            "192.168.4.255",
            "255.255.255.255"
        ]
        .map(String::from)
        .into()
    );
    for address in &addresses {
        for port in ports(0) {
            assert!(
                plan.destinations.contains(&v4(address, port)),
                "{address}:{port}"
            );
        }
    }
    assert_eq!(plan.destinations.len(), addresses.len() * ports(0).len());
    let all_nodes: Vec<_> = plan.best_effort.iter().copied().collect();
    assert_eq!(all_nodes.len(), ports(0).len());
    for destination in all_nodes {
        let SocketAddr::V6(v6) = destination else {
            panic!("{destination}")
        };
        assert_eq!(
            (v6.ip().to_string(), v6.scope_id()),
            ("ff02::1".to_string(), 7)
        );
    }
}

#[test]
fn a_host_without_wake_targets_gets_serverinfos_mac_only() {
    let plan = plan(
        &host(Some("00:1a:2b:3c:4d:5e"), &[], &[]),
        &Links::default(),
    )
    .unwrap();
    assert_eq!(plan.payloads, [magic_packet("00:1a:2b:3c:4d:5e").unwrap()]);
    assert!(plan.destinations.contains(&v4("255.255.255.255", 9)));
    assert!(plan.best_effort.is_empty());
}

#[test]
fn no_usable_mac_is_no_plan() {
    assert_eq!(
        plan(&host(None, &[], &["10.0.0.255"]), &links())
            .err()
            .as_deref(),
        Some(NO_MAC)
    );
    assert_eq!(
        plan(&host(Some("00:00:00:00:00:00"), &["nope"], &[]), &links())
            .err()
            .as_deref(),
        Some(NO_MAC)
    );
    // A MAC from `/wake` wakes the PC even when serverinfo's is not usable.
    assert_eq!(
        plan(&host(Some("nope"), &["0a:00:27:00:00:05"], &[]), &links())
            .unwrap()
            .payloads
            .len(),
        1
    );
}

#[test]
fn failed_all_nodes_sends_do_not_fail_the_wake_but_they_do_not_carry_it_either() {
    let plan = plan(
        &host(Some("00:1a:2b:3c:4d:5e"), &["0a:00:27:00:00:05"], &[]),
        &links(),
    )
    .unwrap();
    let mut sends = 0;
    let only_v4 = send(&plan, |_, destination| {
        sends += 1;
        if destination.is_ipv4() {
            Ok(())
        } else {
            Err(io::Error::other("no route"))
        }
    });
    assert_eq!(only_v4, Ok(()));
    assert_eq!(
        sends,
        3 * 2 * (plan.destinations.len() + plan.best_effort.len())
    );
    let only_v6 = send(&plan, |_, destination| {
        if destination.is_ipv6() {
            Ok(())
        } else {
            Err(io::Error::other("network is unreachable"))
        }
    });
    assert_eq!(only_v6.err().as_deref(), Some("network is unreachable"));
}

#[test]
fn wake_targets_are_normal_and_bounded() {
    let target = |mac: &str, broadcasts: &[&str]| {
        Target {
        mac: mac.into(),
        broadcasts: broadcasts.iter().map(|b| b.to_string()).collect(),
        ..serde_json::from_str(r#"{"mac":"","kind":"ethernet","connected":true,"route":true,"broadcasts":[],"state":"ready","checks":[]}"#).unwrap()
    }
    };
    let mut wake: Wake = serde_json::from_str(
        r#"{"revision":1,"primary":"02-7C-16-B4-13-4F","targets":[],"system":{"sleep":"s3","modern_standby":false,"hibernate":false,"fast_startup":"inactive","shutdown_wake":"firmware_unknown"},"idle_sleep":{"state":"off","minutes":0,"reason":null}}"#,
    )
    .unwrap();
    wake.targets = vec![
        target(
            "02:7c:16:b4:13:4f",
            &["192.168.4.255", "pc.lan", "192.168.4.255"],
        ),
        target("00:00:00:00:00:00", &["10.0.0.255"]),
        target("bad", &["300.0.0.1"]),
    ];
    wake.targets
        .extend((0..10).map(|i| target(&format!("0a:00:27:00:00:{i:02x}"), &[])));
    let (macs, broadcasts) = targets(&wake);
    assert_eq!(macs.len(), MAX_MACS);
    assert_eq!(&macs[..2], ["02:7c:16:b4:13:4f", "0a:00:27:00:00:00"]);
    assert_eq!(broadcasts, ["192.168.4.255", "10.0.0.255"]);
}

fn ms(at: u64) -> Duration {
    Duration::from_millis(at)
}

#[test]
fn wake_wait_sends_at_once_then_waits_between_resends() {
    let t = Instant::now();
    let mut wait = WakeWait::with(t + ms(1_000), ms(60_000), ms(8_000));
    assert_eq!(wait.next(false, t + ms(1_000)), Step::Send);
    assert_eq!(wait.next(false, t + ms(2_000)), Step::Wait);
    assert_eq!(wait.next(false, t + ms(8_999)), Step::Wait);
    assert_eq!(wait.next(false, t + ms(9_000)), Step::Send);
    assert_eq!(wait.next(false, t + ms(10_000)), Step::Wait);
    assert_eq!(wait.next(false, t + ms(17_000)), Step::Send);
}

#[test]
fn wake_wait_gives_up_when_the_time_is_up() {
    let t = Instant::now();
    let mut wait = WakeWait::with(t, ms(60_000), ms(8_000));
    assert_eq!(wait.next(false, t), Step::Send);
    assert_eq!(wait.next(false, t + ms(7_000)), Step::Wait);
    assert_eq!(wait.next(false, t + ms(60_000)), Step::TimedOut);
}

#[test]
fn wake_wait_an_answering_pc_wins_even_in_the_tick_the_time_ran_out() {
    let t = Instant::now();
    let mut wait = WakeWait::with(t, ms(60_000), ms(8_000));
    assert_eq!(wait.next(true, t), Step::Online);
    assert_eq!(wait.next(true, t + ms(60_000)), Step::Online);
}

#[test]
fn wake_wait_defaults_give_a_resume_from_sleep_ninety_seconds() {
    let t = Instant::now();
    let mut wait = WakeWait::new(t);
    assert_eq!(wait.next(false, t), Step::Send);
    assert_eq!(wait.next(false, t + ms(7_999)), Step::Wait);
    assert_eq!(wait.next(false, t + WakeWait::RESEND), Step::Send);
    let mut at = 2 * WakeWait::RESEND;
    while at < WakeWait::TIMEOUT {
        assert_eq!(wait.next(false, t + at), Step::Send);
        at += WakeWait::RESEND;
    }
    assert_eq!(wait.next(false, t + WakeWait::TIMEOUT - ms(1)), Step::Wait);
    assert_eq!(wait.next(false, t + WakeWait::TIMEOUT), Step::TimedOut);
}

#[test]
fn a_late_error_of_a_cancelled_wake_does_not_end_the_next_wake_of_the_same_pc() {
    let t = Instant::now();
    let mut wakes = Wakes::default();
    assert!(wakes.start(7, t));
    let old = wakes.current().unwrap().run;
    wakes.end();
    assert!(wakes.start(7, t + ms(1_000)));
    let new = wakes.current().unwrap().run;
    assert_ne!(old, new);

    assert!(!wakes.failed(7, old));
    assert_eq!(wakes.host(), Some(7));
    assert!(!wakes.failed(8, new));
    assert_eq!(wakes.host(), Some(7));
    assert!(wakes.failed(7, new));
    assert_eq!(wakes.host(), None);
}

#[test]
fn a_second_wake_waits_for_the_first_to_end() {
    let t = Instant::now();
    let mut wakes = Wakes::default();
    assert!(wakes.start(7, t));
    assert!(!wakes.start(8, t));
    assert_eq!(wakes.host(), Some(7));
}
