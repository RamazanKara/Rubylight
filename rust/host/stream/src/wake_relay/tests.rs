//! The PC's side of the away wake relay: who may enroll, what a relay and a device get, and
//! that a removed relay or a rotated key stays gone.
use super::*;
use crate::{state::test_support::Fixture, wake::fake::Fake};
use butterpollo_crypto::crypto::Identity;
use butterpollo_session::state::{PLAY_DEVICE_PERMISSION, VIEW_STREAMS};
use rubylight_traverse::wake_relay::{Enrollment, parse_hex};
use std::{
    collections::HashMap,
    net::Ipv4Addr,
    sync::{Mutex, OnceLock},
};

/// The fake PC's Ethernet: 192.168.4.20/24, MAC 02:7C:16:B4:13:4F (`wake::fake`).
const LAN_PEER: &str = "192.168.4.50";

fn host() -> Fixture {
    let f = Fixture::new();
    Fake::new().on(&f.host);
    set(
        &f,
        &[("internet_play", "true"), ("away_wake_relay", "true")],
    );
    f
}

fn set(f: &Fixture, values: &[(&str, &str)]) {
    let mut config = f.host.config.write().unwrap();
    for (key, value) in values {
        config.values.insert((*key).into(), (*value).into());
    }
}

/// A certificate per name, made once for all tests.
fn cert(name: &str) -> String {
    static CERTS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    let mut certs = CERTS.get_or_init(Default::default).lock().unwrap();
    certs
        .entry(name.to_owned())
        .or_insert_with(|| Identity::generate().unwrap().certificate)
        .clone()
}

/// A paired device named after `name`, with its own certificate.
fn device(f: &Fixture, name: &str, perm: u32, host_control: bool) -> Client {
    paired(f, name, perm, host_control, true)
}

fn paired(f: &Fixture, name: &str, perm: u32, host_control: bool, enabled: bool) -> Client {
    let mut extra = std::collections::BTreeMap::new();
    if host_control {
        extra.insert("host_control".into(), true.into());
    }
    let client = Client {
        name: format!("Device {name}"),
        cert: cert(name),
        uuid: name.into(),
        perm,
        enabled,
        extra,
    };
    f.host
        .paired
        .write()
        .unwrap()
        .add(&f.host.paired_path, client.clone())
        .unwrap();
    client
}

fn ip(text: &str) -> SocketAddr {
    SocketAddr::new(text.parse().unwrap(), 50_000)
}

fn enroll_as(f: &Fixture, relay: &Client) -> RelayAssignment {
    enroll(
        &f.host,
        relay,
        ip(LAN_PEER),
        RelayEnroll::Enroll {
            name: Some("Living room NUC".into()),
        },
    )
    .unwrap()
    .assignment
    .unwrap()
}

#[test]
fn offered_only_with_internet_play_and_the_setting() {
    for (internet, relay, expected) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (true, true, true),
    ] {
        let mut config = Config::default();
        config
            .values
            .insert("internet_play".into(), internet.to_string());
        config
            .values
            .insert("away_wake_relay".into(), relay.to_string());
        assert_eq!(offered(&config), expected, "{internet} {relay}");
    }
}

#[test]
fn only_a_device_with_host_control_on_the_pcs_own_subnet_enrolls() {
    let f = host();
    let player = device(&f, "player", PLAY_DEVICE_PERMISSION, false);
    let relay = device(&f, "relay", PLAY_DEVICE_PERMISSION, true);
    let enroll_from = |client: &Client, address: &str| {
        enroll(
            &f.host,
            client,
            ip(address),
            RelayEnroll::Enroll { name: None },
        )
    };
    assert_eq!(
        enroll_from(&player, LAN_PEER).unwrap_err(),
        Refused::Permission
    );
    // A tunnelled connection (loopback), the internet, another private network, a VPN on a
    // subnet the PC has no adapter on, and the PC's virtual switch are not its network.
    for address in [
        "127.0.0.1",
        "::1",
        "8.8.8.8",
        "10.0.0.5",
        "192.168.5.50",
        "172.30.112.9",
    ] {
        assert_eq!(
            enroll_from(&relay, address).unwrap_err(),
            Refused::Relay(Refusal::NotOnLan),
            "{address}"
        );
    }
    assert!(load(&f.host).unwrap().relays.is_empty());
    // IPv4-mapped IPv6 is the same address.
    assert!(enroll_from(&relay, "::ffff:192.168.4.50").is_ok());
    assert_eq!(load(&f.host).unwrap().relays.len(), 1);
}

#[test]
fn nothing_is_enrolled_or_handed_out_with_the_setting_off() {
    let f = host();
    set(&f, &[("away_wake_relay", "false")]);
    let relay = device(&f, "relay", PLAY_DEVICE_PERMISSION, true);
    let enrolled = enroll(
        &f.host,
        &relay,
        ip(LAN_PEER),
        RelayEnroll::Enroll { name: None },
    );
    assert_eq!(enrolled.unwrap_err(), Refused::Off);
    let read = info(&f.host, &relay).unwrap();
    assert!(!read.enabled);
    assert!(read.bundle.is_none() && read.relays.is_empty());
    // Play over the internet off turns it off too.
    set(
        &f,
        &[("away_wake_relay", "true"), ("internet_play", "false")],
    );
    assert!(!info(&f.host, &relay).unwrap().enabled);
}

#[test]
fn a_relay_gets_only_the_pcs_physical_macs_its_own_network_and_the_devices_that_may_launch() {
    let f = host();
    let relay = device(&f, "relay", PLAY_DEVICE_PERMISSION, true);
    let player = device(&f, "player", PLAY_DEVICE_PERMISSION, false);
    let viewer = device(&f, "viewer", VIEW_STREAMS, false);
    paired(&f, "off", PLAY_DEVICE_PERMISSION, false, false);
    let assignment = enroll_as(&f, &relay);
    // The connected Ethernet first, then the WLAN; never the virtual switch.
    assert_eq!(assignment.macs, ["02:7C:16:B4:13:4F", "02:E0:4C:68:00:01"]);
    assert_eq!(assignment.broadcasts, ["192.168.4.255"]);
    assert_eq!(assignment.address.as_deref(), Some("192.168.4.20"));
    assert_eq!(assignment.refresh_seconds, relay::REFRESH_SECONDS);
    assert_eq!(assignment.target_id, target_id(&f.host));
    assert!(assignment.devices.contains(&device_id(&player.cert)));
    assert!(!assignment.devices.contains(&device_id(&viewer.cert)));
    assert!(!assignment.devices.contains(&device_id(&cert("off"))));
    // The secret is 32 random bytes, the same on every refresh.
    assert!(parse_hex::<32>(&assignment.secret).is_some());
    let refreshed = enroll(&f.host, &relay, ip(LAN_PEER), RelayEnroll::Refresh)
        .unwrap()
        .assignment
        .unwrap();
    assert_eq!(refreshed.secret, assignment.secret);
    // The file never holds anything but the relays and the epoch.
    let saved = std::fs::read_to_string(path(&f.host)).unwrap();
    assert!(!saved.contains("02:7C") && !saved.contains("player"));
}

#[test]
fn a_device_gets_keys_only_with_launch_and_they_match_what_the_relay_derives() {
    let f = host();
    let relay = device(&f, "relay", PLAY_DEVICE_PERMISSION, true);
    let player = device(&f, "player", PLAY_DEVICE_PERMISSION, false);
    let viewer = device(&f, "viewer", VIEW_STREAMS, false);
    let assignment = enroll_as(&f, &relay);

    assert_eq!(info(&f.host, &viewer).unwrap_err(), Refused::Permission);
    let read = info(&f.host, &player).unwrap();
    assert!(read.enabled);
    assert_eq!(read.relays.len(), 1);
    assert_eq!(read.relays[0].name, "Living room NUC");
    let bundle = read.bundle.unwrap();
    assert_eq!(bundle.device, device_id(&player.cert));
    assert_eq!(bundle.relays.len(), 1);
    assert_ne!(bundle.relays[0].secret, assignment.secret);

    // The relay derives the same key from its assignment, so it accepts the device's request.
    let enrollment = Enrollment {
        target_id: assignment.target_id.clone(),
        target_name: assignment.target_name.clone(),
        epoch: assignment.epoch,
        secret: parse_hex::<32>(&assignment.secret).unwrap(),
        macs: Vec::new(),
        broadcasts: vec![Ipv4Addr::new(192, 168, 4, 255)],
        address: None,
        http_port: assignment.http_port,
        devices: Vec::new(),
    };
    let device = parse_hex::<32>(&bundle.device).unwrap();
    assert_eq!(
        hex::encode(enrollment.device_secret(&device)),
        bundle.relays[0].secret
    );
}

#[test]
fn a_removed_relay_never_comes_back_by_refreshing() {
    let f = host();
    let relay = device(&f, "relay", PLAY_DEVICE_PERMISSION, true);
    let owner = device(&f, "owner", PLAY_DEVICE_PERMISSION, true);
    enroll_as(&f, &relay);
    let revoked = enroll(
        &f.host,
        &owner,
        ip("8.8.8.8"),
        RelayEnroll::Revoke {
            relay: device_id(&relay.cert),
        },
    )
    .unwrap();
    assert!(revoked.relays.is_empty() && revoked.assignment.is_none());
    assert_eq!(
        enroll(&f.host, &relay, ip(LAN_PEER), RelayEnroll::Refresh).unwrap_err(),
        Refused::Relay(Refusal::UnknownRelay)
    );
    assert!(load(&f.host).unwrap().relays.is_empty());
    // Leaving works the same way, and leaving twice says so.
    enroll_as(&f, &relay);
    enroll(&f.host, &relay, ip(LAN_PEER), RelayEnroll::Leave).unwrap();
    assert_eq!(
        enroll(&f.host, &relay, ip(LAN_PEER), RelayEnroll::Leave).unwrap_err(),
        Refused::Relay(Refusal::UnknownRelay)
    );
    // The console removes one the same way.
    enroll_as(&f, &relay);
    console_change(&f.host, Some(&device_id(&relay.cert))).unwrap();
    assert_eq!(
        console_status(&f.host).unwrap()["relays"],
        serde_json::json!([])
    );
}

#[test]
fn rotating_the_keys_ends_every_device_key_and_relay_secret() {
    let f = host();
    let relay = device(&f, "relay", PLAY_DEVICE_PERMISSION, true);
    let player = device(&f, "player", PLAY_DEVICE_PERMISSION, false);
    let before = enroll_as(&f, &relay);
    let old = info(&f.host, &player).unwrap().bundle.unwrap();
    console_change(&f.host, None).unwrap();
    let after = enroll(&f.host, &relay, ip(LAN_PEER), RelayEnroll::Refresh)
        .unwrap()
        .assignment
        .unwrap();
    let new = info(&f.host, &player).unwrap().bundle.unwrap();
    assert_eq!(after.epoch, before.epoch + 1);
    assert_ne!(after.secret, before.secret);
    assert_eq!(new.epoch, old.epoch + 1);
    assert_ne!(new.relays[0].secret, old.relays[0].secret);
    // A device with Host control rotates them over the native API too.
    let rotated = enroll(&f.host, &relay, ip("8.8.8.8"), RelayEnroll::RotateEpoch).unwrap();
    assert!(rotated.assignment.is_none());
    assert_eq!(load(&f.host).unwrap().epoch(), after.epoch + 1);
}

#[test]
fn a_damaged_file_is_an_error_and_never_new_keys() {
    let f = host();
    let relay = device(&f, "relay", PLAY_DEVICE_PERMISSION, true);
    let player = device(&f, "player", PLAY_DEVICE_PERMISSION, false);
    enroll_as(&f, &relay);
    std::fs::write(path(&f.host), b"{ not json").unwrap();
    assert!(matches!(
        info(&f.host, &player).unwrap_err(),
        Refused::Failed(_)
    ));
    assert!(matches!(
        enroll(&f.host, &relay, ip(LAN_PEER), RelayEnroll::Refresh).unwrap_err(),
        Refused::Failed(_)
    ));
    assert_eq!(std::fs::read(path(&f.host)).unwrap(), b"{ not json");
}

#[test]
fn the_console_lists_without_secrets_and_says_why_it_refuses() {
    let f = host();
    let relay = device(&f, "relay", PLAY_DEVICE_PERMISSION, true);
    let assignment = enroll_as(&f, &relay);
    let none = serde_json::Value::Null;
    let status = console(&f.host, "GET", "/api/wake-relay", &none).unwrap();
    assert_eq!(status["enabled"], true);
    assert_eq!(status["relays"][0]["name"], "Living room NUC");
    assert!(!status.to_string().contains(&assignment.secret));
    let revoke =
        |body: serde_json::Value| console(&f.host, "POST", "/api/wake-relay/revoke", &body);
    assert!(revoke(serde_json::json!({})).is_err());
    let unknown = revoke(serde_json::json!({"id": "nobody"})).unwrap_err();
    assert!(unknown.to_string().contains("not enrolled"), "{unknown}");
    let after = revoke(serde_json::json!({"id": device_id(&relay.cert)})).unwrap();
    assert_eq!(after["relays"], serde_json::json!([]));
    let rotated = console(&f.host, "POST", "/api/wake-relay/rotate", &none).unwrap();
    assert_eq!(rotated["epoch"], assignment.epoch + 1);
}

#[test]
fn wake_relay_stores_secrets_under_credentials_and_migrates_legacy_root_file() {
    let f = host();
    let relay = device(&f, "relay", PLAY_DEVICE_PERMISSION, true);
    assert_eq!(
        path(&f.host),
        f.host.directory.join("credentials").join(relay::FILE)
    );
    // Remove credentials/ to verify save creates it cleanly before writing.
    let _ = std::fs::remove_dir_all(f.host.directory.join("credentials"));
    let assignment = enroll_as(&f, &relay);
    assert!(path(&f.host).is_file());
    assert!(!legacy_path(&f.host.directory).exists());

    // Startup migration moves a legacy root wake-relay.json into credentials/ and removes the old file.
    let legacy_dir = crate::state::test_support::Fixture::profile();
    std::fs::create_dir_all(&legacy_dir).unwrap();
    let legacy_json = std::fs::read_to_string(path(&f.host)).unwrap();
    std::fs::write(legacy_dir.join(relay::FILE), &legacy_json).unwrap();
    let loaded = crate::state::test_support::Fixture::load(legacy_dir.clone());
    assert!(!legacy_dir.join(relay::FILE).exists());
    assert!(legacy_dir.join("credentials").join(relay::FILE).is_file());
    let migrated = load(&loaded.host).unwrap();
    assert_eq!(migrated.relays.len(), 1);
    assert_eq!(migrated.relays[0].secret, assignment.secret);

    // When credentials/wake-relay.json already exists, migration keeps it and deletes the stale root copy.
    std::fs::write(legacy_dir.join(relay::FILE), br#"{"epoch":99,"relays":[]}"#).unwrap();
    assert!(migrate(&legacy_dir).unwrap());
    assert!(!legacy_dir.join(relay::FILE).exists());
    assert_eq!(
        load(&loaded.host).unwrap().relays[0].secret,
        assignment.secret
    );
}

#[test]
fn a_tunnelled_lan_peer_is_refused_for_enroll_and_refresh_but_may_revoke() {
    let f = host();
    let relay = device(&f, "relay", PLAY_DEVICE_PERMISSION, true);
    // When bind_address is a LAN IPv4, a Traverse bridge arrives from that LAN IPv4
    // registered as a tunnelled peer. It must never enroll or refresh as a LAN relay.
    let tunnel_peer: SocketAddr = "192.168.4.20:50941".parse().unwrap();
    let _mark = rubylight_traverse::peers::register(tunnel_peer, "phone");
    assert_eq!(
        enroll(
            &f.host,
            &relay,
            tunnel_peer,
            RelayEnroll::Enroll { name: None },
        )
        .unwrap_err(),
        Refused::Relay(Refusal::NotOnLan)
    );
    assert!(load(&f.host).unwrap().relays.is_empty());

    // Enroll directly on the LAN, then confirm Refresh over the tunnel is also refused.
    enroll_as(&f, &relay);
    assert_eq!(
        enroll(&f.host, &relay, tunnel_peer, RelayEnroll::Refresh).unwrap_err(),
        Refused::Relay(Refusal::NotOnLan)
    );
    // Management actions (RotateEpoch / Revoke) remain permitted from away.
    enroll(&f.host, &relay, tunnel_peer, RelayEnroll::RotateEpoch).unwrap();
    enroll(
        &f.host,
        &relay,
        tunnel_peer,
        RelayEnroll::Revoke {
            relay: device_id(&relay.cert),
        },
    )
    .unwrap();
    assert!(load(&f.host).unwrap().relays.is_empty());
}
