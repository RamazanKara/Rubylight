//! Persisted paired host records, pace calibrations, layer source resolution, and address parsing.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{Overrides, Profile, Settings};

pub use nightjar_policy::hosts::{
    ALIAS_MAX_CHARS, IdentityFile as Identity, NOTICE_FRAME_WARP_UNAVAILABLE,
    NOTICE_PACKED_UNAVAILABLE, NOTICE_PYROWAVE_FALLBACK,
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HostRecord {
    pub pace: Option<PaceCalibration>,
    /// The host's `uniqueid`; empty until the host answered once.
    pub unique_id: String,
    pub name: String,
    /// A name chosen on this device. The PC's own `name` keeps following what it reports.
    pub alias: Option<String>,
    /// Last address that answered, then the others it was known by.
    pub address: String,
    pub http_port: u16,
    pub https_port: u16,
    pub other_addresses: Vec<String>,
    /// Other HTTP ports at its addresses that answered with this PC's unique ID, from a listing
    /// of it there that was merged into this one: a sighting at one of them is this PC.
    pub other_http_ports: Vec<u16>,
    /// The known address the user chose to always connect through, or None for the automatic
    /// pick (`address` first, then `other_addresses`), as Moonlight V+ PC's `pinnedAddress`.
    pub pinned_address: Option<String>,
    /// The WAN address that reaches the PC from outside its network, tried after the others
    /// (Moonlight's remote address): the PC's own `ExternalIP`, else this device's STUN answer
    /// when the PC was found on the LAN.
    pub remote_address: Option<String>,
    /// The certificate pinned at pairing (PEM); None while unpaired.
    pub server_certificate: Option<String>,
    pub mac: Option<String>,
    /// The MACs of the PC's other adapters and the subnet broadcasts that reach them, from its
    /// native `GET /wake` (`wol::targets`); kept so a PC that sleeps can be woken through them.
    pub wake_macs: Vec<String>,
    pub wake_broadcasts: Vec<String>,
    /// MACs the user added for the PC: an adapter that wakes it while the one paired over cannot
    /// (`wol::parse_macs`). What the PC lists never replaces them.
    pub own_wake_macs: Vec<String>,
    /// Added by hand rather than found on the network.
    pub manual: bool,
    pub settings: Option<Overrides>,
    pub app_settings: BTreeMap<String, Overrides>,
    /// The games whose stream quits them on the PC when the player leaves it, kept as Android's
    /// `AppQuitOnLeave` does: under `uuid:<uuid>` and `id:<id>`.
    pub quit_on_leave: BTreeSet<String>,
    /// The once-per-PC notices already shown for this PC, by Android's `HostNotices` names.
    pub notices: BTreeSet<String>,
    /// What this PC's answers to full chroma left: its last offered modes and whether it refused
    /// the last stream that asked (Android's `HostPackedModes`).
    pub full_chroma: nightjar_policy::full_chroma::Memory,
    /// The PC's `/internet-play/v1` reply as it came, read on its network while paired: its
    /// rendezvous secret for this device, IPv6 addresses and forwarded ports, for reaching it
    /// from outside (`nightjar_core::internet`). None until a PC that offers it answered.
    pub internet_play: Option<String>,
    /// Typed Android endpoint slots and unknown fields in `hosts.json`, preserved losslessly when
    /// Desktop loads and saves records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_endpoint: Option<nightjar_policy::hosts::EndpointSlot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_endpoint: Option<nightjar_policy::hosts::EndpointSlot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_endpoint: Option<nightjar_policy::hosts::EndpointSlot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipv6_endpoint: Option<nightjar_policy::hosts::EndpointSlot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_endpoint: Option<nightjar_policy::hosts::EndpointSlot>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaceCalibration {
    pub pace_mbps: u32,
    pub client_link_mbps: u32,
    pub host_link_mbps: u32,
    pub wifi: bool,
    pub ssid: Option<String>,
    pub qualified: bool,
}

impl PaceCalibration {
    pub fn for_network(&self, network: &crate::network_types::Network, host_link: u32) -> u32 {
        if network.error.is_none()
            && self.client_link_mbps == network.link_mbps
            && self.host_link_mbps == host_link
            && self.wifi == network.wifi
            && (!self.wifi || (self.ssid.is_some() && self.ssid == network.ssid))
            && (1..=9999).contains(&self.pace_mbps)
        {
            self.pace_mbps
        } else {
            0
        }
    }
}

impl HostRecord {
    /// Whether there is a MAC to wake the PC by: serverinfo's, one its `/wake` listed, or one the
    /// user added. Every wake gate asks this.
    pub fn can_wake(&self) -> bool {
        self.mac.is_some() || !self.wake_macs.is_empty() || !self.own_wake_macs.is_empty()
    }

    /// Keeps `text` as this device's name for the PC. Empty (or blank) restores the PC's own name.
    pub fn set_alias(&mut self, text: &str) {
        self.alias = nightjar_policy::hosts::HostNames::normalize_alias(Some(text), None);
    }

    pub fn settings(&self, global: &Settings) -> Settings {
        self.settings.as_ref().map_or_else(|| global.clone(), |own| own.apply(global))
    }

    /// True the first time `notice` is shown for this PC, and remembered (Android's `HostNotices.claim`).
    pub fn claim_notice(&mut self, notice: &str) -> bool {
        self.notices.insert(notice.to_owned())
    }

    /// The PC has the feature again: if it loses it later, the notice is shown again. True when it was shown.
    pub fn reset_notice(&mut self, notice: &str) -> bool {
        self.notices.remove(notice)
    }

    /// What this PC keeps over the main settings and profiles: whether it has settings of its
    /// own, and for how many games (each saved under `id:`, and `uuid:` when it has one).
    pub fn own_settings(&self) -> (bool, usize) {
        let count = |prefix: &str| {
            self.app_settings
                .iter()
                .filter(|(key, own)| key.starts_with(prefix) && !own.is_empty())
                .count()
        };
        (self.settings.as_ref().is_some_and(|own| !own.is_empty()), count("id:").max(count("uuid:")))
    }

    /// Drops the PC's and its games' own settings, so they follow the profile or main settings.
    pub fn clear_own_settings(&mut self) {
        self.settings = None;
        self.app_settings.clear();
    }

    pub fn app_settings(&self, uuid: &str, id: i32) -> Option<&Overrides> {
        self.app_settings.get(&app_key(uuid, id))
    }

    pub fn save_app_settings(&mut self, uuid: &str, id: i32, settings: Overrides) {
        let mut keys = vec![app_key(uuid, id)];
        if !uuid.is_empty() {
            keys.push(app_key("", id));
        }
        for key in keys {
            if settings.is_empty() {
                self.app_settings.remove(&key);
            } else {
                self.app_settings.insert(key, settings.clone());
            }
        }
    }

    /// Whether leaving this game's stream quits it. An ID can be reused for another game on hosts
    /// with UUIDs, so a known UUID decides alone.
    pub fn quits_on_leave(&self, uuid: &str, id: i32) -> bool {
        if !uuid.is_empty() {
            return self.quit_on_leave.contains(&app_key(uuid, 0));
        }
        id > 0 && self.quit_on_leave.contains(&app_key("", id))
    }

    pub fn set_quit_on_leave(&mut self, uuid: &str, id: i32, enabled: bool) {
        let mut keys = Vec::new();
        if !uuid.is_empty() {
            keys.push(app_key(uuid, id));
        }
        if id > 0 {
            keys.push(app_key("", id));
        }
        for key in keys {
            if enabled {
                self.quit_on_leave.insert(key);
            } else {
                self.quit_on_leave.remove(&key);
            }
        }
    }

    /// Exports the shared flat host fields represented in `hosts.json`.
    pub fn to_shared_fields(&self) -> nightjar_policy::hosts::SharedHostFields {
        nightjar_policy::hosts::SharedHostFields {
            unique_id: self.unique_id.clone(),
            name: self.name.clone(),
            alias: self.alias.clone(),
            address: self.address.clone(),
            http_port: self.http_port,
            https_port: self.https_port,
            mac: self.mac.clone(),
            server_certificate: self.server_certificate.clone(),
            manual: self.manual,
            other_addresses: self.other_addresses.clone(),
            other_http_ports: self.other_http_ports.clone(),
            pinned_address: self.pinned_address.clone(),
            remote_address: self.remote_address.clone(),
            wake_macs: self.wake_macs.clone(),
            wake_broadcasts: self.wake_broadcasts.clone(),
            own_wake_macs: self.own_wake_macs.clone(),
            quit_on_leave: self.quit_on_leave.clone(),
            notices: self.notices.clone(),
            full_chroma: self.full_chroma.clone(),
            internet_play: self.internet_play.clone(),
        }
    }

    /// Applies updated flat [`nightjar_policy::hosts::SharedHostFields`] back onto `self`.
    pub fn apply_shared_fields(&mut self, shared: &nightjar_policy::hosts::SharedHostFields) {
        self.unique_id = shared.unique_id.clone();
        self.name = shared.name.clone();
        self.alias = shared.alias.clone();
        self.address = shared.address.clone();
        self.http_port = shared.http_port;
        self.https_port = shared.https_port;
        self.mac = shared.mac.clone();
        self.server_certificate = shared.server_certificate.clone();
        self.manual = shared.manual;
        self.other_addresses = shared.other_addresses.clone();
        self.other_http_ports = shared.other_http_ports.clone();
        self.pinned_address = shared.pinned_address.clone();
        self.remote_address = shared.remote_address.clone();
        self.wake_macs = shared.wake_macs.clone();
        self.wake_broadcasts = shared.wake_broadcasts.clone();
        self.own_wake_macs = shared.own_wake_macs.clone();
        self.quit_on_leave = shared.quit_on_leave.clone();
        self.notices = shared.notices.clone();
        self.full_chroma = shared.full_chroma.clone();
        self.internet_play = shared.internet_play.clone();
    }

    /// Typed Android endpoint slots preserved in `hosts.json`.
    pub fn slots(&self) -> nightjar_policy::hosts::HostSlots {
        nightjar_policy::hosts::HostSlots {
            local: self.local_endpoint.clone(),
            remote: self.remote_endpoint.clone(),
            manual: self.manual_endpoint.clone(),
            ipv6: self.ipv6_endpoint.clone(),
            pinned: self.pinned_endpoint.clone(),
        }
    }

    /// Writes typed endpoint slots back onto `self`.
    pub fn set_slots(&mut self, slots: &nightjar_policy::hosts::HostSlots) {
        self.local_endpoint = slots.local.clone();
        self.remote_endpoint = slots.remote.clone();
        self.manual_endpoint = slots.manual.clone();
        self.ipv6_endpoint = slots.ipv6.clone();
        self.pinned_endpoint = slots.pinned.clone();
    }

    /// Recomputes flat shared fields from populated Android endpoint slots.
    pub fn sync_shared_from_slots(&mut self) {
        let slots = self.slots();
        if slots.has_any() {
            let mut shared = self.to_shared_fields();
            nightjar_policy::hosts::sync_shared_from_slots(&mut shared, &slots);
            self.apply_shared_fields(&shared);
        }
    }

    /// Populates missing typed slots from legacy flat Desktop fields so cross-device sync is lossless.
    pub fn sync_slots_from_shared(&mut self) {
        let mut slots = self.slots();
        let shared = self.to_shared_fields();
        nightjar_policy::hosts::sync_slots_from_shared(&shared, &mut slots);
        self.set_slots(&slots);
    }

    /// Updates the typed primary endpoint slot (`ipv6`, `manual`, or `local`) directly so DHCP
    /// lease changes or mDNS promotions never leave stale addresses in `hosts.json` or WoL.
    pub fn set_primary_endpoint(&mut self, address: &str, port: u16) {
        let port = if port == 0 {
            nightjar_policy::hosts::DEFAULT_HTTP_PORT
        } else {
            port
        };
        let slot = nightjar_policy::hosts::EndpointSlot::new(address, port);
        if address
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_ipv6())
        {
            self.ipv6_endpoint = slot;
        } else if self.manual && self.local_endpoint.is_none() {
            self.manual_endpoint = slot;
        } else {
            self.local_endpoint = slot;
        }
    }

    /// Updates `remote_address` and `remote_endpoint` together so WAN IP changes stay synced.
    pub fn set_remote_endpoint(&mut self, address: String, port: u16) {
        let port = if port == 0 {
            nightjar_policy::hosts::DEFAULT_HTTP_PORT
        } else {
            port
        };
        self.remote_endpoint = nightjar_policy::hosts::EndpointSlot::new(&address, port);
        self.remote_address = Some(address);
    }

    /// Updates `pinned_address` and `pinned_endpoint` together (`None` clears both).
    pub fn set_pinned_endpoint(&mut self, address: Option<&str>, default_port: u16) {
        match address {
            None => {
                self.pinned_address = None;
                self.pinned_endpoint = None;
            }
            Some(addr) => {
                let default_port = if default_port == 0 {
                    nightjar_policy::hosts::DEFAULT_HTTP_PORT
                } else {
                    default_port
                };
                let port = [
                    &self.local_endpoint,
                    &self.manual_endpoint,
                    &self.ipv6_endpoint,
                    &self.remote_endpoint,
                ]
                .into_iter()
                .flatten()
                .find(|s| s.address == addr)
                .map_or(default_port, |s| s.port);
                self.pinned_address = Some(addr.to_owned());
                self.pinned_endpoint = nightjar_policy::hosts::EndpointSlot::new(addr, port);
            }
        }
    }
}

/// The layer a stream setting's value comes from, from the top: the game, the PC, the profile in
/// use, else the main settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Main,
    Profile(String),
    Pc,
    Game,
}

pub fn source(
    keys: &[&str],
    profile: Option<&Profile>,
    pc: Option<&Overrides>,
    game: Option<&Overrides>,
) -> Source {
    if game.is_some_and(|o| o.has(keys)) {
        Source::Game
    } else if pc.is_some_and(|o| o.has(keys)) {
        Source::Pc
    } else {
        profile
            .filter(|p| p.settings.has(keys))
            .map_or(Source::Main, |p| Source::Profile(p.name.clone()))
    }
}

fn app_key(uuid: &str, id: i32) -> String {
    if uuid.is_empty() { format!("id:{id}") } else { format!("uuid:{uuid}") }
}

/// Splits "host", "host:port", "[v6]:port" or a bare IPv6 address.
pub fn parse_address(text: &str) -> Option<(String, Option<u16>)> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Some(rest) = text.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = match tail.strip_prefix(':') {
            Some(p) => Some(p.parse().ok()?),
            None if tail.is_empty() => None,
            None => return None,
        };
        return Some((host.to_owned(), port));
    }
    match text.matches(':').count() {
        0 => Some((text.to_owned(), None)),
        1 => {
            let (host, port) = text.split_once(':')?;
            if host.is_empty() {
                return None;
            }
            Some((host.to_owned(), Some(port.parse().ok()?)))
        }
        _ => Some((text.to_owned(), None)),
    }
}
