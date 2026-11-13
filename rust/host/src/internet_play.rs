//! Internet play on the host (`docs/internet-play.md`): what the port mapping did, the public
//! address STUN finds, the DHT rendezvous and hole-punched tunnels for paired devices, the
//! per-device secret that keys them, and `/internet-play/v1`, which hands a paired device on the
//! LAN what it needs to come back from outside.
//!
//! "Play over the internet" (`internet_play`) turns on STUN, the rendezvous and the tunnels.
//! Port mapping keeps following the `upnp` setting, and also asks PCP and NAT-PMP routers.

use crate::state::Shared;
use anyhow::{Context, Result};
use butterpollo_core::config::Config;
use rubylight_traverse::{
    self as traverse,
    host::{Device, HostConfig, HostService},
    record::{Nat, Ports},
};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::Digest;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{LazyLock, Mutex},
    time::Duration,
};

/// The paired-device record field holding the device's rendezvous secret (64 hex digits).
pub const SECRET_FIELD: &str = "internet_play_secret";
/// `RustHostInternetPlayVersion` in serverinfo, and the version of `/internet-play/v1`.
pub const VERSION: u32 = 1;
pub const SERVERINFO_FIELD: &str = "RustHostInternetPlayVersion";
pub const ROUTE: &str = "/internet-play/v1";

pub fn enabled(config: &Config) -> bool {
    config.boolean("internet_play", false)
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PortState {
    pub protocol: String,
    pub port: u16,
    pub external: Option<u16>,
    pub reason: Option<String>,
}

/// What the port mapping task last did, for the console.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MappingStatus {
    pub enabled: bool,
    /// UPnP, PCP or NAT-PMP: whichever the router answered.
    pub method: Option<String>,
    pub external_ipv4: Option<IpAddr>,
    pub mapped: Vec<PortState>,
    pub failed: Vec<PortState>,
    /// Why nothing could be mapped.
    pub error: Option<String>,
    pub ipv6_method: Option<String>,
    pub ipv6_pinholes: usize,
    pub ipv6_error: Option<String>,
}

static MAPPING: LazyLock<Mutex<MappingStatus>> =
    LazyLock::new(|| Mutex::new(MappingStatus::default()));
static SERVICE: Mutex<Option<HostService>> = Mutex::new(None);

pub fn update_mapping(change: impl FnOnce(&mut MappingStatus)) {
    change(&mut MAPPING.lock().unwrap());
}

fn mapping() -> MappingStatus {
    MAPPING.lock().unwrap().clone()
}

fn service() -> Option<HostService> {
    SERVICE.lock().unwrap().clone()
}

/// Private, link-local and carrier-grade NAT (100.64/10) addresses: none of them is reachable
/// from the internet.
fn public_ipv4(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => {
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || (ip.octets()[0] == 100 && ip.octets()[1] & 0xc0 == 64))
        }
        IpAddr::V6(_) => false,
    }
}

/// The public IPv4 address Moonlight stores as the PC's remote address (`ExternalIP`): the
/// router's own address when it is public, else the address STUN servers see.
pub fn external_ipv4() -> Option<IpAddr> {
    let mapped = mapping()
        .external_ipv4
        .filter(|address| public_ipv4(*address));
    mapped.or_else(|| {
        service()?
            .status()
            .public
            .iter()
            .map(SocketAddr::ip)
            .find(|address| public_ipv4(*address))
    })
}

fn nat_name(nat: Nat) -> &'static str {
    match nat {
        Nat::Unknown => "unknown",
        Nat::None => "none",
        Nat::Cone => "cone",
        Nat::Symmetric => "symmetric",
    }
}

/// Plain-English findings across the mapping and STUN: a router behind carrier-grade NAT, or a
/// second NAT in front of the router.
fn notes(mapping: &MappingStatus, stun: &[SocketAddr], nat: Nat) -> Vec<String> {
    let mut notes = Vec::new();
    if let Some(external) = mapping.external_ipv4 {
        if !public_ipv4(external) {
            notes.push(format!(
                "The router's own internet address ({external}) is not public: the internet provider shares one address among customers (carrier-grade NAT), so forwarded ports can't be reached from outside. Devices use a direct internet path instead."
            ));
        } else if let Some(seen) = stun.iter().map(SocketAddr::ip).find(|ip| ip.is_ipv4())
            && seen != external
        {
            notes.push(format!(
                "STUN servers see this PC at {seen}, but the router reports {external}: another router or NAT is in front of it, and forwarded ports may not be reachable."
            ));
        }
    }
    if nat == Nat::Symmetric {
        notes.push(
            "This network gives each destination another port (symmetric NAT): a direct internet path opens only to devices on more open networks."
                .to_owned(),
        );
    }
    notes
}

/// `GET /api/internet-play`, the console's status card.
pub fn console_status(h: &Shared) -> Result<Value> {
    let config = h.config_snapshot();
    let mapping = mapping();
    let rendezvous = service().map(|service| service.status());
    let (public, nat) = rendezvous
        .as_ref()
        .map_or((Vec::new(), Nat::Unknown), |status| {
            (status.public.clone(), status.nat)
        });
    let notes = notes(&mapping, &public, nat);
    Ok(json!({
        "enabled": enabled(&config),
        "portMapping": config.boolean("upnp", false),
        "mapping": mapping,
        "externalIp": external_ipv4(),
        "rendezvous": rendezvous.map(|status| json!({
            "public": status.public,
            "nat": nat_name(status.nat),
            "stunError": status.stun_error,
            "devices": status.devices,
            "published": status.published,
            "dhtError": status.dht_error,
            "paths": status.paths.iter().map(|path| json!({
                "device": path.device,
                "peer": path.peer,
                "connected": path.connected,
            })).collect::<Vec<_>>(),
        })),
        "notes": notes,
    }))
}

fn ports(config: &Config) -> Result<Ports> {
    let ports = config.ports()?;
    Ok(Ports {
        http: ports.http,
        https: ports.https,
        rtsp: ports.rtsp,
        video: ports.video,
        audio: ports.audio,
        control: ports.control,
        mic: if config.boolean("stream_mic", true) {
            ports.mic
        } else {
            0
        },
    })
}

/// A stable id for a paired device: the SHA-256 of its certificate.
fn device_id(certificate: &str) -> String {
    hex::encode(sha2::Sha256::digest(certificate.trim().as_bytes()))
}

fn parse_secret(value: &Value) -> Option<[u8; 32]> {
    let bytes = hex::decode(value.as_str()?).ok()?;
    bytes.try_into().ok()
}

/// The devices that have a secret, for the rendezvous.
fn devices(h: &Shared) -> Vec<Device> {
    let paired = h.paired.read().unwrap();
    let mut devices: Vec<Device> = Vec::new();
    for client in paired.clients.iter().filter(|client| client.enabled) {
        let Some(secret) = client.extra.get(SECRET_FIELD).and_then(parse_secret) else {
            continue;
        };
        let id = device_id(&client.cert);
        if !devices.iter().any(|device| device.id == id) {
            devices.push(Device {
                id,
                name: client.name.clone(),
                secret,
            });
        }
    }
    devices
}

/// The device's secret, made and stored with its pairing the first time it asks.
fn secret(h: &Shared, certificate: &str) -> Result<[u8; 32]> {
    let mut paired = h.paired.write().unwrap();
    let matching =
        |client: &&mut butterpollo_core::state::Client| client.cert.trim() == certificate.trim();
    if let Some(secret) = paired
        .clients
        .iter_mut()
        .filter(matching)
        .find_map(|client| client.extra.get(SECRET_FIELD).and_then(parse_secret))
    {
        return Ok(secret);
    }
    let secret: [u8; 32] = traverse::crypto::random();
    let mut stored = false;
    for client in paired.clients.iter_mut().filter(matching) {
        client
            .extra
            .insert(SECRET_FIELD.to_owned(), Value::String(hex::encode(secret)));
        stored = true;
    }
    anyhow::ensure!(stored, "the device is not paired");
    paired
        .save(&h.paired_path)
        .context("the internet play secret could not be saved")?;
    drop(paired);
    if let Some(service) = service() {
        service.set_devices(devices(h));
    }
    Ok(secret)
}

/// This PC's global IPv6 addresses, when the host listens on IPv6.
fn ipv6_addresses(config: &Config) -> Vec<String> {
    if config.get("address_family", "ipv4") != "both" {
        return Vec::new();
    }
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|interface| interface.oper_status == if_addrs::IfOperStatus::Up)
        .filter_map(|interface| match interface.ip() {
            IpAddr::V6(ip) if ip.segments()[0] & 0xe000 == 0x2000 => Some(ip.to_string()),
            _ => None,
        })
        .take(4)
        .collect()
}

/// `GET /internet-play/v1` for a paired device (`certificate`, over HTTPS): the secret, the
/// host session, the public endpoints and ports. The device keeps it per host.
pub fn describe(h: &Shared, certificate: &str) -> Result<Value> {
    let config = h.config_snapshot();
    let ports = ports(&config)?;
    let mapping = mapping();
    let mut answer = json!({
        "version": VERSION,
        "enabled": enabled(&config),
        "portMapping": config.boolean("upnp", false),
        "externalIpv4": external_ipv4(),
        // A client derives every port from the base port: a mapping is usable only when the
        // router kept the port numbers.
        "mappedPorts": mapping.failed.is_empty()
            && !mapping.mapped.is_empty()
            && mapping.mapped.iter().all(|port| port.external == Some(port.port)),
        "ipv6": ipv6_addresses(&config),
        "ports": {
            "http": ports.http,
            "https": ports.https,
            "rtsp": ports.rtsp,
            "video": ports.video,
            "audio": ports.audio,
            "control": ports.control,
            "mic": ports.mic,
        },
    });
    if enabled(&config) {
        answer["secret"] = Value::String(hex::encode(secret(h, certificate)?));
        if let Some(service) = service() {
            let status = service.status();
            answer["session"] = Value::String(hex::encode(service.session()));
            answer["endpoints"] = json!(status.public);
            answer["nat"] = Value::String(nat_name(status.nat).to_owned());
        }
    }
    Ok(answer)
}

/// The running rendezvous; stops it when dropped through [`InternetPlay::stop`].
pub struct InternetPlay {
    refresh: tokio::task::JoinHandle<()>,
}

impl InternetPlay {
    pub fn stop(self) {
        self.refresh.abort();
        if let Some(service) = SERVICE.lock().unwrap().take() {
            service.stop();
        }
    }
}

/// Starts STUN, the rendezvous and the tunnels when Play over the internet is on.
pub fn start(h: Shared, bind: IpAddr) -> Option<InternetPlay> {
    let config = h.config_snapshot();
    if !enabled(&config) || bind.is_loopback() {
        return None;
    }
    let ports = match ports(&config) {
        Ok(ports) => ports,
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "Play over the internet could not read the ports");
            return None;
        }
    };
    // Tunnels connect to the host's own ports on loopback, from an address per device; a host
    // bound to one address is reached there instead.
    let (service_address, per_device_source) = if bind.is_unspecified() {
        (IpAddr::V4(Ipv4Addr::LOCALHOST), true)
    } else {
        (bind, false)
    };
    let node_bind = match bind {
        IpAddr::V4(ip) if !ip.is_unspecified() => SocketAddr::new(bind, 0),
        _ => traverse::node::any_ipv4(),
    };
    let host = match HostService::start(HostConfig {
        ports,
        service: service_address,
        per_device_source,
        bind: node_bind,
        stun_servers: traverse::server_list(
            config.get("internet_play_stun_servers", ""),
            traverse::STUN_SERVERS,
        ),
        bootstrap: traverse::DHT_BOOTSTRAP
            .iter()
            .map(|entry| (*entry).to_owned())
            .collect(),
    }) {
        Ok(host) => host,
        Err(error) => {
            tracing::warn!(%error, "Play over the internet could not open its UDP socket");
            return None;
        }
    };
    host.set_devices(devices(&h));
    tracing::info!(local = ?host.local_addr().ok(), "Play over the internet started");
    *SERVICE.lock().unwrap() = Some(host);
    let refresh = tokio::spawn(async move {
        let mut reported: Option<String> = None;
        loop {
            tokio::time::sleep(Duration::from_secs(15)).await;
            let Some(service) = service() else { return };
            service.set_devices(devices(&h));
            // Log a changed state once, not every 15 s.
            let status = service.status();
            let summary = format!(
                "{:?} {} {:?} {:?}",
                status.public,
                nat_name(status.nat),
                status.stun_error,
                status.dht_error
            );
            if reported.as_ref() != Some(&summary) {
                if let Some(error) = status.stun_error.as_ref().or(status.dht_error.as_ref()) {
                    tracing::warn!(%error, "Play over the internet: devices outside this network may not reach this PC");
                } else {
                    tracing::info!(public = ?status.public, nat = nat_name(status.nat), published = status.published, "Play over the internet is reachable");
                }
                reported = Some(summary);
            }
        }
    });
    Some(InternetPlay { refresh })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_public_ipv4_counts_as_external() {
        assert!(public_ipv4("203.0.113.5".parse().unwrap()));
        assert!(!public_ipv4("192.168.1.1".parse().unwrap()));
        assert!(!public_ipv4("100.72.0.1".parse().unwrap()));
        assert!(!public_ipv4("2001:db8::1".parse().unwrap()));
    }

    #[test]
    fn notes_name_carrier_grade_and_double_nat() {
        let cgnat = MappingStatus {
            external_ipv4: Some("100.72.0.1".parse().unwrap()),
            ..Default::default()
        };
        let found = notes(&cgnat, &[], Nat::Cone);
        assert_eq!(found.len(), 1);
        assert!(found[0].contains("carrier-grade NAT"));

        let double = MappingStatus {
            external_ipv4: Some("203.0.113.5".parse().unwrap()),
            ..Default::default()
        };
        let found = notes(
            &double,
            &["198.51.100.7:40000".parse().unwrap()],
            Nat::Symmetric,
        );
        assert_eq!(found.len(), 2);
        assert!(found[0].contains("another router or NAT"));
        assert!(found[1].contains("symmetric NAT"));

        assert!(notes(&double, &["203.0.113.5:40000".parse().unwrap()], Nat::Cone).is_empty());
    }

    #[test]
    fn device_ids_ignore_surrounding_whitespace() {
        assert_eq!(device_id("cert\n"), device_id("cert"));
        assert_ne!(device_id("a"), device_id("b"));
        assert_eq!(parse_secret(&Value::String("00".repeat(32))), Some([0; 32]));
        assert_eq!(parse_secret(&Value::String("00".repeat(31))), None);
    }
}
