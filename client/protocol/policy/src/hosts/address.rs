//! Typed host address slots (`(address, port)`), `"<address> <port>"` pinned-address
//! formatting/parsing, bidirectional `SharedHostFields` <-> `HostSlots` sync, and site-local
//! address classification shared across Nightjar clients.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr};

use serde::{Deserialize, Serialize};

use super::record::SharedHostFields;

/// Default HTTP port when a legacy entry specifies `0`.
pub const DEFAULT_HTTP_PORT: u16 = 47989;

/// A host address paired with its HTTP port (distinct per slot: local, remote, manual, IPv6,
/// or pinned).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EndpointSlot {
    pub address: String,
    pub port: u16,
}

impl EndpointSlot {
    /// Builds an [`EndpointSlot`] if `address` is non-empty and `port` is in `1..=65535`,
    /// stripping surrounding `[...]` brackets on IPv6 literals.
    pub fn new(address: &str, port: u16) -> Option<Self> {
        if port == 0 {
            return None;
        }
        let trimmed = address.trim();
        if trimmed.is_empty() {
            return None;
        }
        let unbracketed = trimmed
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .unwrap_or(trimmed);
        if unbracketed.is_empty() {
            return None;
        }
        Some(Self {
            address: unbracketed.to_owned(),
            port,
        })
    }
}

impl fmt::Display for EndpointSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.address.contains(':') {
            write!(f, "[{}]:{}", self.address, self.port)
        } else {
            write!(f, "{}:{}", self.address, self.port)
        }
    }
}

/// Typed per-role endpoint slots stored beside [`SharedHostFields`] in `hosts.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostSlots {
    pub local: Option<EndpointSlot>,
    pub manual: Option<EndpointSlot>,
    pub ipv6: Option<EndpointSlot>,
    pub remote: Option<EndpointSlot>,
    pub pinned: Option<EndpointSlot>,
}

impl HostSlots {
    /// Whether a remote/WAN endpoint slot is populated.
    pub fn has_remote(&self) -> bool {
        self.remote.is_some()
    }

    /// Whether any of the four candidate slots (`local`, `manual`, `ipv6`, `remote`) is populated.
    pub fn has_any(&self) -> bool {
        self.local.is_some()
            || self.manual.is_some()
            || self.ipv6.is_some()
            || self.remote.is_some()
    }
}

/// Synchronizes Desktop's flat [`SharedHostFields`] from typed [`HostSlots`].
///
/// `shared.address` picks `local.or(manual).or(ipv6)` and NEVER promotes `remote`.
/// Remaining non-remote slots populate `shared.other_addresses` and `shared.other_http_ports`.
/// `shared.manual` is true only when `manual` is populated and `local` is absent.
pub fn sync_shared_from_slots(shared: &mut SharedHostFields, slots: &HostSlots) {
    let primary = slots
        .local
        .as_ref()
        .or(slots.manual.as_ref())
        .or(slots.ipv6.as_ref());
    if let Some(p) = primary {
        shared.address = p.address.clone();
        shared.http_port = p.port;
    } else {
        shared.address.clear();
        shared.http_port = slots.remote.as_ref().map_or(0, |r| r.port);
    }

    shared.remote_address = slots.remote.as_ref().map(|r| r.address.clone());
    shared.manual = slots.manual.is_some() && slots.local.is_none();

    let mut other_addrs: Vec<String> = Vec::new();
    for existing in &shared.other_addresses {
        if !existing.is_empty()
            && *existing != shared.address
            && shared.remote_address.as_deref() != Some(existing.as_str())
            && !other_addrs.contains(existing)
        {
            other_addrs.push(existing.clone());
        }
    }
    let mut other_ports: Vec<u16> = Vec::new();
    for &port in &shared.other_http_ports {
        if port != 0 && port != shared.http_port && !other_ports.contains(&port) {
            other_ports.push(port);
        }
    }
    for slot in [&slots.local, &slots.manual, &slots.ipv6]
        .into_iter()
        .flatten()
    {
        if slot.address != shared.address && !other_addrs.contains(&slot.address) {
            other_addrs.push(slot.address.clone());
        }
        if slot.port != 0 && slot.port != shared.http_port && !other_ports.contains(&slot.port) {
            other_ports.push(slot.port);
        }
    }
    shared.other_addresses = other_addrs;
    shared.other_http_ports = other_ports;

    shared.pinned_address = slots.pinned.as_ref().map(|p| p.address.clone());
}

/// Populates missing typed [`HostSlots`] from legacy flat [`SharedHostFields`].
pub fn sync_slots_from_shared(shared: &SharedHostFields, slots: &mut HostSlots) {
    let default_port = if shared.http_port == 0 {
        DEFAULT_HTTP_PORT
    } else {
        shared.http_port
    };
    if slots.local.is_none()
        && slots.manual.is_none()
        && slots.ipv6.is_none()
        && !shared.address.is_empty()
    {
        let primary = EndpointSlot::new(&shared.address, default_port);
        if shared.manual {
            slots.manual = primary;
        } else if shared
            .address
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_ipv6())
        {
            slots.ipv6 = primary;
        } else {
            slots.local = primary;
        }
    }
    if slots.remote.is_none()
        && let Some(remote) = shared.remote_address.as_deref()
    {
        slots.remote = EndpointSlot::new(remote, default_port);
    }
    if slots.pinned.is_none()
        && let Some(pinned) = shared.pinned_address.as_deref()
    {
        let port = [&slots.local, &slots.manual, &slots.ipv6, &slots.remote]
            .into_iter()
            .flatten()
            .find(|s| s.address == pinned)
            .map_or(default_port, |s| s.port);
        slots.pinned = EndpointSlot::new(pinned, port);
    }
}

/// Whether an IPv4 address is in RFC 6598 Shared Address Space (`100.64.0.0/10`),
/// used by ISP CGNAT and overlay VPNs such as Tailscale.
pub fn is_cgnat_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    a == 100 && (64..=127).contains(&b)
}

/// Whether an IP address belongs to a private overlay/VPN space (`100.64.0.0/10` CGNAT/Tailscale
/// IPv4 or Tailscale's `fd7a:115c:a1e0::/48` IPv6 range).
pub fn is_overlay_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_cgnat_ipv4(v4),
        IpAddr::V6(v6) => {
            let s = v6.segments();
            s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0
        }
    }
}

/// Whether an IPv4 address is site-local (RFC 1918 private: `10/8`, `172.16/12`, `192.168/16`),
/// matching Java `Inet4Address.isSiteLocalAddress()`.
pub fn is_site_local_ipv4(ip: Ipv4Addr) -> bool {
    ip.is_private()
}

/// Whether an IP address is site-local (RFC 1918 IPv4 or IPv6 ULA `fc00::/7` / site-local
/// `fec0::/10`, excluding Tailscale's `fd7a:115c:a1e0::/48` overlay range).
pub fn is_site_local_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_site_local_ipv4(v4),
        IpAddr::V6(v6) => {
            if is_overlay_ip(IpAddr::V6(v6)) {
                return false;
            }
            let first = v6.segments()[0];
            (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfec0
        }
    }
}

/// Parses `text` as a publicly routable IPv4 address for remote reachability, returning `None`
/// for private, CGNAT/Tailscale (`100.64.0.0/10`), loopback, link-local, unspecified, broadcast,
/// multicast, or documentation IPs.
pub fn public_ipv4(text: &str) -> Option<String> {
    let address: Ipv4Addr = text.trim().parse().ok()?;
    let local = address.is_private()
        || is_cgnat_ipv4(address)
        || address.is_loopback()
        || address.is_link_local()
        || address.is_unspecified()
        || address.is_broadcast()
        || address.is_multicast()
        || address.is_documentation();
    (!local).then(|| address.to_string())
}

/// Pure `"<address> <port>"` codec matching Android's `PinnedAddresses` preference storage.
pub struct PinnedAddress;

impl PinnedAddress {
    pub fn parse(stored: &str) -> Option<EndpointSlot> {
        let split = stored.rfind(' ')?;
        if split == 0 {
            return None;
        }
        let addr = &stored[..split];
        let port: u16 = stored[split + 1..].parse().ok()?;
        EndpointSlot::new(addr, port)
    }

    pub fn format(slot: &EndpointSlot) -> String {
        format!("{} {}", slot.address, slot.port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_shared_from_slots_clears_pinned_address_when_slot_is_none() {
        let mut shared = SharedHostFields {
            address: "192.168.1.10".into(),
            http_port: DEFAULT_HTTP_PORT,
            pinned_address: Some("192.168.1.10".into()),
            ..SharedHostFields::default()
        };
        let mut slots = HostSlots {
            local: EndpointSlot::new("192.168.1.10", DEFAULT_HTTP_PORT),
            pinned: None,
            ..HostSlots::default()
        };
        sync_shared_from_slots(&mut shared, &slots);
        assert_eq!(shared.pinned_address, None, "unpinning slot must clear shared.pinned_address");

        slots.pinned = EndpointSlot::new("10.0.0.5", 48000);
        sync_shared_from_slots(&mut shared, &slots);
        assert_eq!(shared.pinned_address.as_deref(), Some("10.0.0.5"));

        slots.pinned = None;
        sync_shared_from_slots(&mut shared, &slots);
        assert_eq!(shared.pinned_address, None);
    }
}
