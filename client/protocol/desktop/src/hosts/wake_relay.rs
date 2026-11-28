//! Away wake relay for desktop Nightjar (`docs/designs/15-away-wake-relay.md`): per-host
//! away wake bundles (`wake-relay-keys.json`, owner-only and redacted from logs), local subnet
//! detection (`on_pc_network`), and the `rubylight-traverse` requester runner when a PC is offline
//! away from home.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use nightjar_core::hosts::io::{file_lock, write_atomic};
use nightjar_core::http::native_api::wake_relay::{
    RelayCredential, WakeRelayBundle, WakeRelayInfo,
};
use nightjar_core::wake_relay::request_config;
use nightjar_desktop_config::config::HostRecord;
use nightjar_desktop_config::strings::{tr, trf};
use rubylight_traverse::wake_relay::{Status, WakeStatus};
use rubylight_traverse::wake_relay_node::requester::{self, Failure, Progress, RequestConfig};
use rubylight_traverse::{DHT_BOOTSTRAP, STUN_SERVERS};
use serde::{Deserialize, Serialize};

pub const KEYS_FILE_NAME: &str = "wake-relay-keys.json";
const BIND: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0);

/// Result of querying `GET /wake/relay` during a full native read.
#[derive(Clone, PartialEq, Eq)]
pub enum WakeRelaySync {
    /// Host offers away wake relay; `None` when no relay is enrolled for this device.
    Offered(Option<WakeRelayBundle>),
    /// Host does not offer away wake relay (forget cached keys).
    NotOffered,
}

/// Cached entry for one paired PC that offers away wake relay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeptEntry {
    /// Host offers away wake relay and gave a bundle with at least one relay.
    Relays(WakeRelayBundle),
    /// Host offers away wake relay, but has no enrolled relay for this device.
    NoRelay,
}

#[derive(Default, Serialize, Deserialize)]
struct KeysFile {
    #[serde(default)]
    hosts: BTreeMap<String, Option<WakeRelayBundle>>,
}

/// Registers away-wake bundle secrets so log scrubbing redacts every device/relay hex key.
pub(crate) fn register_bundle_secrets(bundle: &WakeRelayBundle) {
    nightjar_core::report::redact::add_secret(&bundle.device);
    for relay in &bundle.relays {
        nightjar_core::report::redact::add_secret(&relay.secret);
    }
}

/// Reads `GET /wake/relay` when `offered`, or marks [`WakeRelaySync::NotOffered`] when unsupported.
/// Returns `None` on transient transport/HTTP failure so previously stored keys remain intact.
pub(super) fn fetch_sync(
    client: &nightjar_core::http::HostClient,
    offered: bool,
) -> Option<WakeRelaySync> {
    if !offered {
        return Some(WakeRelaySync::NotOffered);
    }
    match client.wake_relay() {
        Ok(WakeRelayInfo {
            enabled: true,
            bundle,
            ..
        }) => {
            let valid = bundle.filter(|b| !b.relays.is_empty());
            if let Some(bundle) = &valid {
                register_bundle_secrets(bundle);
            }
            Some(WakeRelaySync::Offered(valid))
        }
        Ok(WakeRelayInfo { enabled: false, .. }) => Some(WakeRelaySync::NotOffered),
        Err(e) => {
            eprintln!(
                "nightjar: wake relay keys not read: {}",
                super::describe(&e)
            );
            None
        }
    }
}

/// Persistent per-PC away-wake bundle store in `wake-relay-keys.json`.
#[derive(Default)]
pub struct KeysStore {
    path: Option<PathBuf>,
    hosts: BTreeMap<String, Option<WakeRelayBundle>>,
    unreadable: bool,
}

impl KeysStore {
    pub fn open(dir: &Path) -> Self {
        let path = dir.join(KEYS_FILE_NAME);
        let lock = file_lock(&path);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<KeysFile>(&bytes) {
                Ok(parsed) => {
                    for bundle in parsed.hosts.values().flatten() {
                        register_bundle_secrets(bundle);
                    }
                    Self {
                        path: Some(path),
                        hosts: parsed.hosts,
                        unreadable: false,
                    }
                }
                Err(error) => {
                    // Never quote parsed JSON values in case a secret sits near the syntax error.
                    eprintln!(
                        "nightjar: {} cannot be read (line {}, column {}); keeping existing file",
                        path.display(),
                        error.line(),
                        error.column()
                    );
                    Self {
                        path: Some(path),
                        hosts: BTreeMap::new(),
                        unreadable: true,
                    }
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => Self {
                path: Some(path),
                hosts: BTreeMap::new(),
                unreadable: false,
            },
            Err(error) => {
                eprintln!(
                    "nightjar: {} cannot be read ({}); keeping existing file",
                    path.display(),
                    error.kind()
                );
                Self {
                    path: Some(path),
                    hosts: BTreeMap::new(),
                    unreadable: true,
                }
            }
        }
    }

    pub fn get(&self, unique_id: &str) -> Option<KeptEntry> {
        if unique_id.is_empty() {
            return None;
        }
        self.hosts.get(unique_id).map(|entry| match entry {
            Some(bundle) if !bundle.relays.is_empty() => KeptEntry::Relays(bundle.clone()),
            _ => KeptEntry::NoRelay,
        })
    }

    pub fn apply(&mut self, unique_id: &str, sync: WakeRelaySync) {
        if unique_id.is_empty() {
            return;
        }
        match sync {
            WakeRelaySync::NotOffered => self.forget(unique_id),
            WakeRelaySync::Offered(bundle) => {
                let normalized = bundle.filter(|b| !b.relays.is_empty());
                if let Some(bundle) = &normalized {
                    register_bundle_secrets(bundle);
                }
                if self.hosts.get(unique_id) == Some(&normalized) {
                    return;
                }
                self.hosts.insert(unique_id.to_owned(), normalized);
                self.save();
            }
        }
    }

    pub fn forget(&mut self, unique_id: &str) {
        if unique_id.is_empty() || self.hosts.remove(unique_id).is_none() {
            return;
        }
        self.save();
    }

    fn save(&self) {
        let Some(path) = self.path.as_ref().filter(|_| !self.unreadable) else {
            return;
        };
        let lock = file_lock(path);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let payload = KeysFile {
            hosts: self.hosts.clone(),
        };
        match serde_json::to_vec_pretty(&payload) {
            Ok(bytes) => {
                if let Err(error) = write_atomic(path, &bytes, true) {
                    eprintln!("nightjar: saving {} failed: {error}", path.display());
                }
            }
            Err(error) => eprintln!("nightjar: serializing {} failed: {error}", path.display()),
        }
    }
}

/// One local IPv4 interface subnet (`address / prefix`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Subnet {
    pub address: u32,
    pub prefix: u8,
}

impl Subnet {
    pub fn contains(self, other: u32) -> bool {
        if self.prefix < 1 || self.prefix > 32 {
            return false;
        }
        let mask = if self.prefix == 32 {
            u32::MAX
        } else {
            !((1u32 << (32 - self.prefix)) - 1)
        };
        (self.address & mask) == (other & mask)
    }

    pub fn broadcast(self) -> u32 {
        if self.prefix >= 32 {
            self.address
        } else {
            self.address | ((1u32 << (32 - self.prefix)) - 1)
        }
    }
}

fn prefix_from_netmask(netmask: Ipv4Addr) -> Option<u8> {
    let mask = u32::from(netmask);
    let prefix = mask.leading_ones();
    let remainder = if prefix >= 32 { 0 } else { mask << prefix };
    if remainder != 0 {
        return None;
    }
    u8::try_from(prefix).ok()
}

/// Lists active non-loopback IPv4 interface subnets on this device.
pub fn own_subnets() -> Vec<Subnet> {
    let Ok(list) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    list.into_iter()
        .filter(|iface| iface.is_oper_up() && !iface.is_loopback())
        .filter_map(|iface| match iface.addr {
            if_addrs::IfAddr::V4(v4) => {
                let prefix = prefix_from_netmask(v4.netmask)?;
                Some(Subnet {
                    address: u32::from(v4.ip),
                    prefix,
                })
            }
            if_addrs::IfAddr::V6(_) => None,
        })
        .collect()
}

/// Local (non-away) address strings saved for the PC.
pub fn pc_local_addresses(record: &HostRecord) -> Vec<String> {
    let shared = record.to_shared_fields();
    let reply = nightjar_core::hosts::poll::parse_internet_reply(record.internet_play.as_deref());
    super::poll_addresses(record)
        .into_iter()
        .filter(|addr| !nightjar_core::hosts::poll::away_address(&shared, reply.as_ref(), addr))
        .collect()
}

/// Whether any of `own` subnets holds one of `addresses` or matches one of `broadcasts` (`/8`..=`/30`).
pub fn on_pc_network_in(own: &[Subnet], addresses: &[String], broadcasts: &[String]) -> bool {
    for &subnet in own {
        for addr in addresses {
            if let Ok(ip) = addr.trim().parse::<Ipv4Addr>()
                && subnet.contains(u32::from(ip))
            {
                return true;
            }
        }
        for bcast in broadcasts {
            if let Ok(ip) = bcast.trim().parse::<Ipv4Addr>()
                && (8..=30).contains(&subnet.prefix)
                && subnet.broadcast() == u32::from(ip)
            {
                return true;
            }
        }
    }
    false
}

/// Whether this device appears to be on the PC's local network right now.
pub fn on_pc_network(record: &HostRecord) -> bool {
    on_pc_network_in(
        &own_subnets(),
        &pc_local_addresses(record),
        &record.wake_broadcasts,
    )
}

/// Wake path chosen for a PC when the user selects Wake.
#[derive(Clone, PartialEq, Eq)]
pub enum WakePath {
    Lan,
    Relay(Box<WakeRelayBundle>),
    NoRelay,
}

impl fmt::Debug for WakePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lan => f.write_str("Lan"),
            Self::Relay(b) => f.debug_tuple("Relay").field(&b.relays.len()).finish(),
            Self::NoRelay => f.write_str("NoRelay"),
        }
    }
}

/// Only an offline PC that offers away wake relay, seen from outside its network, leaves LAN WoL.
pub fn choose_wake_path(offline: bool, on_network: bool, kept: Option<&KeptEntry>) -> WakePath {
    if !offline || on_network {
        return WakePath::Lan;
    }
    match kept {
        None => WakePath::Lan,
        Some(KeptEntry::Relays(bundle)) if !bundle.relays.is_empty() => {
            WakePath::Relay(Box::new(bundle.clone()))
        }
        Some(_) => WakePath::NoRelay,
    }
}

/// How an away wake request ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Awake,
    SentNoAnswer,
    TimedOut,
    RateLimited(u16),
    Refused,
    NotOnPcNetwork,
    RelayOffline,
    RelayUnreachable,
    Cancelled,
    Network(String),
    NoRelay,
    Invalid,
}

impl Outcome {
    fn of_status(status: &WakeStatus) -> Option<Self> {
        Some(match status.status {
            Status::HostAwake => Self::Awake,
            Status::TimedOut => Self::TimedOut,
            Status::RateLimited => Self::RateLimited(status.retry_after),
            Status::RejectedAuth | Status::RejectedReplay => Self::Refused,
            Status::PacketSent | Status::WaitingForBoot => Self::SentNoAnswer,
            Status::TargetNotOnLan => return None,
        })
    }

    /// Whether offering a Retry button makes sense for this outcome.
    pub fn offers_retry(&self) -> bool {
        matches!(
            self,
            Self::SentNoAnswer
                | Self::TimedOut
                | Self::RelayOffline
                | Self::RelayUnreachable
                | Self::Network(_)
        )
    }
}

pub fn starting_message(bundle: &WakeRelayBundle) -> String {
    let relay = bundle.relays.first().map_or("", |r| r.name.as_str());
    trf("Contacting your wake relay ({})…", &[&relay])
}

pub fn progress_message(relay: &str, pc: &str, progress: &Progress) -> Option<String> {
    match progress {
        Progress::LookingUp | Progress::Punching => {
            Some(trf("Contacting your wake relay ({})…", &[&relay]))
        }
        Progress::Requested => Some(trf(
            "Asking your wake relay ({}) to wake {}…",
            &[&relay, &pc],
        )),
        Progress::Status(status)
            if matches!(status.status, Status::PacketSent | Status::WaitingForBoot) =>
        {
            Some(trf("Wake signal sent. Waiting for {} to start…", &[&pc]))
        }
        Progress::Status(_) => None,
    }
}

pub fn no_relay_message(pc: &str) -> String {
    trf(
        "{} can’t be woken from outside your home network yet. To wake it from away, turn on “Act as a LAN wake relay” in Nightjar on an always-on computer at home.",
        &[&pc],
    )
}

pub fn outcome_message(outcome: &Outcome, pc: &str, relays: &[String]) -> Option<String> {
    let relay_names = if relays.is_empty() {
        String::new()
    } else {
        relays.join(", ")
    };
    Some(match outcome {
        Outcome::Cancelled => return None,
        Outcome::Awake => trf("{} is awake.", &[&pc]),
        Outcome::SentNoAnswer => trf(
            "The wake signal reached your home network, but your wake relay stopped answering. {} may still be starting, so check again in a minute.",
            &[&pc],
        ),
        Outcome::TimedOut => trf(
            "The wake signal reached your home network, but {} did not answer within 45 seconds. Check Wake-on-LAN setup in the PC menu.",
            &[&pc],
        ),
        Outcome::RateLimited(secs) => trf(
            "Too many wake requests for {} lately. Try again in {} s.",
            &[&pc, &(*secs).max(1)],
        ),
        Outcome::Refused | Outcome::Invalid => tr(
            "Your wake relay turned this request down. Check that this device’s clock is right, or connect once on your home network to refresh wake access.",
        )
        .to_owned(),
        Outcome::NotOnPcNetwork => trf(
            "Your wake relay ({}) is not on the same network as {}, so it could not wake it.",
            &[&relay_names, &pc],
        ),
        Outcome::RelayOffline => trf(
            "Your wake relay ({0}) is offline, so {1} could not be woken from away. Turn on the relay or wake {1} from your home network.",
            &[&relay_names, &pc],
        ),
        Outcome::RelayUnreachable => trf(
            "Nightjar could not reach your wake relay ({}). Check that it is on and online, then try again.",
            &[&relay_names],
        ),
        Outcome::Network(_) => tr(
            "Nightjar could not open a connection on this network. Check your internet connection and try again.",
        )
        .to_owned(),
        Outcome::NoRelay => no_relay_message(pc),
    })
}

pub type AskRelayFn<'a> = dyn FnMut(RequestConfig, &AtomicBool, &mut dyn FnMut(Progress)) -> Result<WakeStatus, Failure>
    + 'a;

fn relay_configs(
    bundle: &WakeRelayBundle,
    bind: SocketAddr,
    stun: &[String],
    bootstrap: &[String],
) -> Result<Vec<(String, RequestConfig)>, Outcome> {
    if bundle.relays.is_empty() {
        return Err(Outcome::NoRelay);
    }
    let configs: Vec<(String, RequestConfig)> = bundle
        .relays
        .iter()
        .filter_map(|relay: &RelayCredential| {
            let cfg = request_config(bundle, relay, bind, stun.to_vec(), bootstrap.to_vec())?;
            Some((relay.name.clone(), cfg))
        })
        .collect();
    if configs.is_empty() {
        Err(Outcome::Invalid)
    } else {
        Ok(configs)
    }
}

/// Runs away wake across `bundle`'s relays in order, returning the final outcome and named relays.
pub fn request_through_relays(
    bundle: &WakeRelayBundle,
    pc: &str,
    bind: SocketAddr,
    stun: &[String],
    bootstrap: &[String],
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(String),
    ask: &mut AskRelayFn<'_>,
) -> (Outcome, Vec<String>) {
    let relays = match relay_configs(bundle, bind, stun, bootstrap) {
        Ok(relays) => relays,
        Err(outcome) => return (outcome, Vec::new()),
    };
    let (mut offline, mut unreachable, mut elsewhere) = (Vec::new(), Vec::new(), Vec::new());
    for (name, cfg) in relays {
        if cancel.load(Ordering::Acquire) {
            return (Outcome::Cancelled, Vec::new());
        }
        let answer = ask(cfg, cancel, &mut |step| {
            if let Some(line) = progress_message(&name, pc, &step) {
                on_progress(line);
            }
        });
        match answer {
            Ok(status) => match Outcome::of_status(&status) {
                Some(outcome) => return (outcome, vec![name]),
                None => elsewhere.push(name),
            },
            Err(Failure::RelayOffline) => offline.push(name),
            Err(Failure::RelayUnreachable) => unreachable.push(name),
            Err(Failure::Cancelled) => return (Outcome::Cancelled, Vec::new()),
            Err(Failure::Network(detail)) => return (Outcome::Network(detail), vec![name]),
        }
    }
    if !elsewhere.is_empty() {
        (Outcome::NotOnPcNetwork, elsewhere)
    } else if !unreachable.is_empty() {
        (Outcome::RelayUnreachable, unreachable)
    } else {
        (Outcome::RelayOffline, offline)
    }
}

/// Runs away wake against `rubylight-traverse`'s live requester on the calling thread.
pub fn run_away_wake(
    bundle: &WakeRelayBundle,
    pc: &str,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(String),
) -> (Outcome, Vec<String>) {
    let stun: Vec<String> = STUN_SERVERS.iter().map(|s| (*s).to_owned()).collect();
    let bootstrap: Vec<String> = DHT_BOOTSTRAP.iter().map(|s| (*s).to_owned()).collect();
    request_through_relays(
        bundle,
        pc,
        BIND,
        &stun,
        &bootstrap,
        cancel,
        on_progress,
        &mut |cfg, cancel, progress| requester::request_wake(cfg, cancel, progress),
    )
}

#[cfg(test)]
mod tests;
