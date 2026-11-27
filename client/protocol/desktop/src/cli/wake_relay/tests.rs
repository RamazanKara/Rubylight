//! `nightjar wake-relay`: its command line, the interfaces and addresses it uses, and the lines
//! it prints.
use super::*;
use crate::cli::{Parsed, parse};
use std::ffi::OsString;

fn args(list: &[&str]) -> Result<Parsed, String> {
    parse(list.iter().map(OsString::from))
}

#[test]
fn wake_relay_parses_with_its_own_options_anywhere() {
    assert_eq!(
        args(&["wake-relay"]),
        Ok(Parsed::WakeRelay(WakeRelay::default()))
    );
    assert_eq!(
        args(&["wake-relay", "--name", "Living room NUC"]),
        Ok(Parsed::WakeRelay(WakeRelay {
            name: Some("Living room NUC".into()),
            ..WakeRelay::default()
        }))
    );
    assert_eq!(
        args(&["--enroll", "WAKE-RELAY", "--name=NUC"]),
        Ok(Parsed::WakeRelay(WakeRelay {
            name: Some("NUC".into()),
            enroll: true,
            leave: false,
        }))
    );
    assert_eq!(
        args(&["wake-relay", "--leave"]),
        Ok(Parsed::WakeRelay(WakeRelay {
            leave: true,
            ..WakeRelay::default()
        }))
    );
}

#[test]
fn wrong_wake_relay_command_lines_are_errors() {
    for wrong in [
        &["wake-relay", "--leave", "--enroll"][..],
        &["wake-relay", "--leave", "--name", "NUC"],
        &["wake-relay", "extra"],
        &["wake-relay", "--host", "pc"],
        &["wake-relay", "--fps", "60"],
        &["wake-relay", "--verbose"],
        &["wake-relay", "--name"],
        &["wake-relay", "--name", "NUC", "--name", "Other"],
        &["--name", "NUC"],
        &["--enroll"],
        &["list", "pc", "--leave"],
        &["stream", "pc", "app", "--enroll"],
        &["--host", "pc", "--name", "NUC"],
    ] {
        assert!(args(wrong).is_err(), "{wrong:?}");
    }
    assert!(crate::cli::USAGE.contains("wake-relay"));
}

#[test]
fn interfaces_take_their_prefix_from_the_netmask() {
    let ip = Ipv4Addr::new(192, 168, 4, 10);
    let lan = interface_of(ip, Ipv4Addr::new(255, 255, 255, 0)).unwrap();
    assert_eq!((lan.ip, lan.prefix), (ip, 24));
    assert_eq!(lan.broadcast(), Some(Ipv4Addr::new(192, 168, 4, 255)));
    let wide = interface_of(Ipv4Addr::new(10, 1, 2, 3), Ipv4Addr::new(255, 0, 0, 0)).unwrap();
    assert_eq!(wide.prefix, 8);
    assert_eq!(interface_of(ip, Ipv4Addr::BROADCAST).unwrap().prefix, 32);
    assert_eq!(interface_of(ip, Ipv4Addr::UNSPECIFIED).unwrap().prefix, 0);
    // Loopback, no address, and a netmask with holes are left out.
    assert!(interface_of(Ipv4Addr::LOCALHOST, Ipv4Addr::new(255, 0, 0, 0)).is_none());
    assert!(interface_of(Ipv4Addr::UNSPECIFIED, Ipv4Addr::new(255, 255, 255, 0)).is_none());
    assert!(interface_of(ip, Ipv4Addr::new(255, 0, 255, 0)).is_none());
}

#[test]
fn a_pc_is_asked_only_at_its_addresses_on_this_network() {
    for here in [
        "192.168.4.20",
        "10.1.2.3",
        "172.16.0.9",
        "169.254.1.1",
        "desktop.local",
    ] {
        assert!(on_this_network(here), "{here}");
    }
    let away = [
        "8.8.8.8",
        "100.64.0.1",
        "127.78.1.2",
        "127.0.0.1",
        "[fe80::1]",
        "fe80::1%eth0",
        "[fe80::1%eth0]",
        "[fe80::1%25eth0]",
        "fe80::1%12",
        "2001:db8::1",
        "",
        "   ",
    ];
    for away in away {
        assert!(!on_this_network(away), "{away}");
    }
}

#[test]
fn scoped_ipv6_literals_are_rejected_while_names_and_lan_ipv4_pass() {
    assert!(!on_this_network("fe80::1%eth0"));
    assert!(!on_this_network("[fe80::1%eth0]"));
    assert!(!on_this_network("[fe80::1%25eth0]"));
    assert!(!on_this_network("fe80::1%12"));
    assert!(on_this_network("desktop.local"));
    assert!(on_this_network("gaming-pc"));
    assert!(on_this_network("192.168.4.20"));
}

#[test]
fn a_leave_the_pc_no_longer_knows_counts_as_left() {
    let api = |status, code| Error::Api {
        status,
        error: Box::new(nightjar_core::http::native_api::error::ApiError::new(
            code,
            "That wake relay is not enrolled on this PC.",
        )),
    };
    assert!(left_already(&api(404, ErrorCode::NotFound)));
    assert!(left_already(&Error::NotFound("/wake/relay/enroll".into())));
    assert!(!left_already(&api(403, ErrorCode::HostControlRequired)));
    assert!(!left_already(&Error::Io(
        std::io::ErrorKind::TimedOut.into()
    )));
}

#[test]
fn status_lines_say_what_changed_once() {
    let before = RelayStatus::default();
    let mut after = RelayStatus {
        public: vec!["203.0.113.7:40000".parse().unwrap()],
        nat: Nat::Cone,
        targets: 1,
        devices: 2,
        published: 2,
        last_wake: Some(("Desktop".into(), 1_800_000_005, Status::PacketSent)),
        ..RelayStatus::default()
    };
    let lines = status_lines(&before, &after);
    assert_eq!(
        lines,
        [
            "public address 203.0.113.7:40000 (cone NAT)",
            "serving 1 PC(s) for 2 device(s)",
            "published for 2 of 2 device(s)",
            "08:00:05 UTC wake for Desktop: magic packets sent",
        ]
    );
    assert!(status_lines(&after, &after).is_empty());
    let mut later = after.clone();
    later.last_wake = Some(("Desktop".into(), 1_800_000_030, Status::HostAwake));
    later.dht_error = Some("no DHT node answered".into());
    later.stun_error = Some("no STUN server answered".into());
    assert_eq!(
        status_lines(&after, &later),
        [
            "STUN: no STUN server answered",
            "DHT: no DHT node answered",
            "08:00:30 UTC wake for Desktop: the PC is awake",
        ]
    );
    after = later.clone();
    later.dht_error = None;
    assert_eq!(status_lines(&after, &later), ["DHT: publishing again"]);
}

#[test]
fn note_lines_name_the_pc_and_the_way_back() {
    let refused = note_line("Desktop", &Note::Refused("Away wake relay is off.".into()));
    assert!(refused.starts_with("Desktop: refused"), "{refused}");
    assert!(refused.contains("--enroll"), "{refused}");
    assert_eq!(
        note_line("Desktop", &Note::Enrolled { devices: 1 }),
        "Desktop: enrolled; 1 device may wake it"
    );
    assert_eq!(clock(0), "00:00:00 UTC");
}
