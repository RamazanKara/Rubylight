//! Away wake relay on the PC that is woken (`docs/designs/15-away-wake-relay.md`): the relays
//! it enrolled, its key epoch, who may enroll, and what a relay and a device get. The keys
//! themselves are `rubylight-traverse`'s (`wake_relay::device_secret`); the host's stream crate
//! derives them. Kept in `credentials/wake-relay.json`.
//!
//! A relay is a paired device with Host control on one of this PC's own private subnets. It
//! enrolls itself over its paired connection and enrolls again every [`REFRESH_SECONDS`], which
//! brings it the devices the PC lets wake it now: a device unpaired, turned off or without
//! Launch drops out of the next refresh, and the relay then refuses it.
use super::Adapter;
use crate::state::Client;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr};

/// How often a relay enrolls again to fetch the devices of now.
pub const REFRESH_SECONDS: u32 = 300;
/// Most relays one PC keeps.
pub const MAX_RELAYS: usize = 8;
/// Most MACs and broadcasts an assignment carries, as the relay acts on.
pub const MAX_MACS: usize = 4;
pub const MAX_BROADCASTS: usize = 4;
pub const FILE: &str = "wake-relay.json";
/// Path of the wake relay secret file relative to the host profile directory.
pub const CREDENTIALS_FILE: &str = "credentials/wake-relay.json";

/// One enrolled relay.
/// Its `Debug` leaves the secret out, so a log line never carries it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relay {
    /// The relay's device id: the SHA-256 of its certificate, hex.
    pub id: String,
    pub name: String,
    /// The relay's secret for this PC, hex. Never shown, never logged.
    pub secret: String,
    pub enrolled_unix: u64,
    #[serde(default)]
    pub last_seen_unix: u64,
}

impl std::fmt::Debug for Relay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Relay")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("enrolled_unix", &self.enrolled_unix)
            .field("last_seen_unix", &self.last_seen_unix)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Relays {
    /// Starts at 1; [`Relays::rotate`] raises it, which ends every device's and relay's access
    /// until each fetches the new one.
    pub epoch: u64,
    pub relays: Vec<Relay>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The caller does not reach the PC from one of its own private subnets.
    NotOnLan,
    TooManyRelays,
    UnknownRelay,
}

impl Relays {
    pub fn epoch(&self) -> u64 {
        self.epoch.max(1)
    }

    /// Enrolls the relay `id`, or refreshes it: its secret stays, its name and last seen change.
    /// `secret` makes a new relay's secret (32 random bytes, hex).
    pub fn enroll(
        &mut self,
        id: &str,
        name: &str,
        now: u64,
        secret: impl FnOnce() -> String,
    ) -> Result<&Relay, Refusal> {
        self.epoch = self.epoch();
        let name = clean_name(name);
        if let Some(index) = self.relays.iter().position(|relay| relay.id == id) {
            let relay = &mut self.relays[index];
            if !name.is_empty() {
                relay.name = name;
            }
            relay.last_seen_unix = now;
            return Ok(&self.relays[index]);
        }
        if self.relays.len() >= MAX_RELAYS {
            return Err(Refusal::TooManyRelays);
        }
        self.relays.push(Relay {
            id: id.to_owned(),
            name: if name.is_empty() {
                "Wake relay".to_owned()
            } else {
                name
            },
            secret: secret(),
            enrolled_unix: now,
            last_seen_unix: now,
        });
        Ok(self.relays.last().expect("just pushed"))
    }

    pub fn remove(&mut self, id: &str) -> Result<(), Refusal> {
        let before = self.relays.len();
        self.relays.retain(|relay| relay.id != id);
        if self.relays.len() == before {
            return Err(Refusal::UnknownRelay);
        }
        Ok(())
    }

    /// New keys: a new epoch, and a new secret for every relay.
    pub fn rotate(&mut self, mut secret: impl FnMut() -> String) {
        self.epoch = self.epoch() + 1;
        for relay in &mut self.relays {
            relay.secret = secret();
        }
    }
}

/// A relay's name as the PC lists it: one line, at most 64 characters.
fn clean_name(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control())
        .take(64)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// A private or link-local IPv4 address, also one written as IPv4-mapped IPv6.
fn private_ipv4(peer: IpAddr) -> Option<Ipv4Addr> {
    match peer.to_canonical() {
        IpAddr::V4(ip) if ip.is_private() || ip.is_link_local() => Some(ip),
        _ => None,
    }
}

fn in_subnet(a: Ipv4Addr, b: Ipv4Addr, prefix: u8) -> bool {
    let mask = u32::MAX
        .checked_shl(32 - u32::from(prefix.min(32)))
        .unwrap_or(0);
    u32::from(a) & mask == u32::from(b) & mask
}

/// The PC's own address on the subnet `peer` reached it from, when `peer` is a private IPv4
/// address on a subnet of one of the PC's physical, connected adapters. A relay must be there:
/// its magic packets reach the PC only on its own network.
pub fn lan_address(peer: IpAddr, adapters: &[Adapter]) -> Option<Ipv4Addr> {
    let peer = private_ipv4(peer)?;
    adapters
        .iter()
        .filter(|adapter| adapter.physical() && adapter.connected)
        .flat_map(|adapter| adapter.ipv4.iter())
        .find(|(own, prefix)| (8..=30).contains(prefix) && in_subnet(*own, peer, *prefix))
        .map(|(own, _)| *own)
}

/// The PC's wake MACs (`AA:BB:CC:DD:EE:FF`) and subnet broadcasts, from its physical adapters,
/// connected ones first, each list bounded.
pub fn wake_targets(adapters: &[Adapter]) -> (Vec<String>, Vec<String>) {
    let mut physical: Vec<&Adapter> = adapters.iter().filter(|a| a.physical()).collect();
    physical.sort_by_key(|adapter| !adapter.connected);
    let mut macs: Vec<String> = Vec::new();
    let mut broadcasts: Vec<String> = Vec::new();
    for adapter in physical {
        let mac = super::format_mac(adapter.mac);
        if !macs.contains(&mac) && macs.len() < MAX_MACS {
            macs.push(mac);
        }
        for broadcast in adapter.broadcasts() {
            let private = broadcast.is_private() || broadcast.is_link_local();
            let text = broadcast.to_string();
            if private && !broadcasts.contains(&text) && broadcasts.len() < MAX_BROADCASTS {
                broadcasts.push(text);
            }
        }
    }
    (macs, broadcasts)
}

/// The devices that may wake the PC through a relay: paired, turned on, with Launch
/// applications. Their ids are the SHA-256 of their certificate, hex, as `id` makes them.
pub fn wakers(clients: &[Client], id: impl Fn(&str) -> String) -> Vec<String> {
    let mut ids: Vec<String> = clients
        .iter()
        .filter(|client| client.allows(crate::state::LAUNCH_APPS))
        .map(|client| id(&client.cert))
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wake::Medium;

    fn adapter(ip: [u8; 4], prefix: u8, connected: bool) -> Adapter {
        Adapter {
            index: 1,
            mac: Some([0x10, 0x7b, 0x44, ip[3], 0, 1]),
            hardware: true,
            connector: true,
            medium: Medium::Wired,
            connected,
            ipv4: vec![(Ipv4Addr::from(ip), prefix)],
            addresses: vec![IpAddr::V4(Ipv4Addr::from(ip))],
            ..Adapter::default()
        }
    }

    #[test]
    fn only_a_caller_on_one_of_the_pcs_private_subnets_may_enroll() {
        let adapters = [adapter([192, 168, 1, 30], 24, true)];
        let here = |ip: &str| lan_address(ip.parse().unwrap(), &adapters);
        assert_eq!(here("192.168.1.10"), Some(Ipv4Addr::new(192, 168, 1, 30)));
        assert_eq!(
            here("::ffff:192.168.1.10"),
            Some(Ipv4Addr::new(192, 168, 1, 30))
        );
        // Another subnet, the internet, loopback (a Traverse tunnel), IPv6: no.
        for away in [
            "192.168.2.10",
            "203.0.113.5",
            "127.0.0.1",
            "127.77.1.2",
            "fd00::5",
        ] {
            assert_eq!(here(away), None, "{away}");
        }
        // A subnet of a virtual or disconnected adapter does not count.
        let mut virtual_adapter = adapter([10, 0, 0, 2], 24, true);
        virtual_adapter.hardware = false;
        let unplugged = adapter([172, 16, 0, 2], 16, false);
        let adapters = [virtual_adapter, unplugged];
        assert_eq!(lan_address("10.0.0.9".parse().unwrap(), &adapters), None);
        assert_eq!(lan_address("172.16.4.4".parse().unwrap(), &adapters), None);
    }

    #[test]
    fn enrolling_again_keeps_the_secret_and_rotating_replaces_every_one() {
        let mut relays = Relays::default();
        let made = relays
            .enroll("a", "NUC", 10, || "s1".into())
            .unwrap()
            .clone();
        assert_eq!((made.secret.as_str(), relays.epoch), ("s1", 1));
        let again = relays.enroll("a", "", 20, || "s2".into()).unwrap();
        assert_eq!((again.secret.as_str(), again.name.as_str()), ("s1", "NUC"));
        assert_eq!(again.last_seen_unix, 20);
        relays
            .enroll("b", "Living\u{7}room", 30, || "s3".into())
            .unwrap();
        assert_eq!(relays.relays[1].name, "Livingroom");
        let mut fresh = ["x", "y"].into_iter();
        relays.rotate(|| fresh.next().unwrap().into());
        assert_eq!(relays.epoch, 2);
        assert_eq!(relays.relays[0].secret, "x");
        assert_eq!(relays.relays[1].secret, "y");
        assert_eq!(relays.remove("a"), Ok(()));
        assert_eq!(relays.remove("a"), Err(Refusal::UnknownRelay));
        for n in 0..MAX_RELAYS {
            let _ = relays.enroll(&format!("r{n}"), "x", 1, String::new);
        }
        assert_eq!(
            relays.enroll("one too many", "x", 1, String::new).err(),
            Some(Refusal::TooManyRelays)
        );
    }

    #[test]
    fn a_relay_gets_the_pcs_physical_macs_and_private_broadcasts_only() {
        let mut wifi = adapter([10, 0, 0, 31], 24, false);
        wifi.medium = Medium::Wifi;
        let mut vpn = adapter([100, 64, 0, 2], 10, true);
        vpn.hardware = false;
        let public = adapter([203, 0, 113, 7], 24, true);
        let (macs, broadcasts) =
            wake_targets(&[wifi, adapter([192, 168, 1, 30], 24, true), vpn, public]);
        // The connected wired NIC first; the VPN's MAC never.
        assert_eq!(macs.len(), 3);
        assert_eq!(macs[0], "10:7B:44:1E:00:01");
        assert!(!macs.contains(&"10:7B:44:02:00:01".to_owned()));
        assert_eq!(broadcasts, vec!["192.168.1.255", "10.0.0.255"]);
    }

    #[test]
    fn only_enabled_devices_with_launch_may_wake() {
        let client = |cert: &str, perm: u32, enabled: bool| Client {
            name: cert.into(),
            cert: cert.into(),
            uuid: cert.into(),
            perm,
            enabled,
            extra: Default::default(),
        };
        let launch = crate::state::LAUNCH_APPS;
        let clients = [
            client("phone", launch, true),
            client("viewer", crate::state::VIEW_STREAMS, true),
            client("off", launch, false),
            client("phone", launch, true),
        ];
        assert_eq!(
            wakers(&clients, |cert| format!("id-{cert}")),
            vec!["id-phone"]
        );
    }

    #[test]
    fn a_relay_prints_without_its_secret() {
        let mut relays = Relays::default();
        let relay = relays
            .enroll("a", "NUC", 10, || "0123456789abcdef".into())
            .unwrap();
        let printed = format!("{relay:?}");
        assert!(printed.contains("NUC"));
        assert!(!printed.contains("0123456789abcdef"), "{printed}");
    }
}
