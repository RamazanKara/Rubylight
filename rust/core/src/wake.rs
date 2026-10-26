//! Wake-on-LAN and sleep after idle: the MAC policy, NIC readiness, `IdleSleep` and `WakeGrace` (`docs/wake-and-audio-design.md`).
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr};

pub mod idle;
mod readiness;
pub use readiness::*;

/// What an adapter carries, as far as waking the PC goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Medium {
    /// Ethernet (802.3).
    Wired,
    /// Native 802.11.
    Wifi,
    /// Anything else: tunnels, Bluetooth, mobile broadband, loopback.
    #[default]
    Other,
}

/// One network adapter as Windows reports it (`GetAdaptersAddresses` and
/// `GetIfEntry2`). The fixtures in `wake/fixtures` are lists of these.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Adapter {
    pub index: u32,
    /// `InterfaceGuid`, `{8-4-4-4-12}` in upper case; the key of the NIC script's report.
    pub guid: String,
    /// The friendly name, for logs only: it is localized.
    pub name: String,
    /// The current address; `None` without a 6-byte one (Wintun, tunnels).
    #[serde(with = "mac_text")]
    pub mac: Option<[u8; 6]>,
    /// The burned-in address (`PermanentPhysicalAddress`).
    #[serde(with = "mac_text")]
    pub permanent: Option<[u8; 6]>,
    /// `InterfaceAndOperStatusFlags.HardwareInterface`.
    pub hardware: bool,
    /// `InterfaceAndOperStatusFlags.ConnectorPresent`: false for virtual
    /// adapters that claim hardware, such as Wi-Fi Direct.
    pub connector: bool,
    pub medium: Medium,
    /// `OperStatus` is up.
    pub connected: bool,
    /// The IPv4 interface metric; lower is preferred.
    pub metric: u32,
    /// IPv4 addresses with their prefix lengths.
    pub ipv4: Vec<(Ipv4Addr, u8)>,
    /// Every unicast address, IPv4 and IPv6, to find the adapter a request came in on.
    pub addresses: Vec<IpAddr>,
}

impl Adapter {
    /// A NIC a magic packet can reach: real hardware with a connector, wired
    /// or Wi-Fi, and a 6-byte unicast address that isn't zero. vEthernet,
    /// TAP, Wintun, loopback and tunnel adapters are virtual.
    pub fn physical(&self) -> bool {
        self.hardware
            && self.connector
            && self.medium != Medium::Other
            && self.mac.is_some_and(unicast_mac)
    }
    /// The subnet-directed IPv4 broadcasts of the adapter.
    pub fn broadcasts(&self) -> Vec<Ipv4Addr> {
        let mut broadcasts: Vec<_> = self
            .ipv4
            .iter()
            .filter(|(address, prefix)| {
                *prefix < 31 && !address.is_loopback() && !address.is_link_local()
            })
            .map(|(address, prefix)| {
                Ipv4Addr::from(u32::from(*address) | (u32::MAX >> (*prefix).min(32)))
            })
            .collect();
        broadcasts.sort();
        broadcasts.dedup();
        broadcasts
    }
    fn owns(&self, local: IpAddr) -> bool {
        self.addresses
            .iter()
            .any(|address| address.to_canonical() == local)
    }
    fn shares_subnet(&self, other: &Adapter) -> bool {
        self.ipv4.iter().any(|(a, prefix)| {
            let mask = u32::MAX
                .checked_shl(32 - u32::from((*prefix).min(32)))
                .unwrap_or(0);
            other
                .ipv4
                .iter()
                .any(|(b, _)| u32::from(*a) & mask == u32::from(*b) & mask)
        })
    }
}

/// A unicast address that isn't all zeros.
pub fn unicast_mac(mac: [u8; 6]) -> bool {
    mac != [0; 6] && mac[0] & 1 == 0
}

/// `AA:BB:CC:DD:EE:FF`, the format of `<mac>` in `/serverinfo`.
pub fn format_mac(mac: Option<[u8; 6]>) -> String {
    mac.unwrap_or_default()
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// The adapter `/serverinfo` names for a request that reached the PC at
/// `local`, and how it was chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Primary<'a> {
    pub adapter: &'a Adapter,
    /// The request came in through this adapter, or through a Hyper-V switch
    /// on it.
    pub route: bool,
    /// Wi-Fi: whether it wakes depends on the adapter and the access point.
    pub experimental: bool,
}

/// Which physical adapter's MAC to report for a request that reached the PC
/// at `local`; `None` when no physical adapter is connected, and the caller
/// sends zeros. `armed` is the cached readiness of an adapter, `None` while
/// it hasn't been read.
pub fn primary<'a>(
    adapters: &'a [Adapter],
    local: IpAddr,
    armed: impl Fn(&Adapter) -> Option<bool>,
) -> Option<Primary<'a>> {
    let local = local.to_canonical();
    let route = adapters.iter().find(|adapter| adapter.owns(local));
    let pick = |adapter, route| Primary {
        adapter,
        route,
        experimental: adapter.medium == Medium::Wifi,
    };
    let connected_wired = |route: &Adapter| {
        adapters.iter().find(|other| {
            other.physical()
                && other.connected
                && other.medium == Medium::Wired
                && other.shares_subnet(route)
        })
    };
    if let Some(route) = route.filter(|route| route.physical()) {
        if route.medium == Medium::Wired {
            return Some(pick(route, true));
        }
        // Wi-Fi: wired wake is far more reliable, and the broadcast reaches it.
        return Some(
            connected_wired(route)
                .map(|wired| pick(wired, false))
                .unwrap_or(pick(route, true)),
        );
    }
    // A Hyper-V external switch: its vEthernet takes the NIC's own address.
    if let Some(mac) = route
        .and_then(|route| route.mac)
        .filter(|mac| unicast_mac(*mac))
        && let Some(nic) = adapters
            .iter()
            .find(|nic| nic.physical() && (nic.permanent == Some(mac) || nic.mac == Some(mac)))
    {
        return Some(pick(nic, true));
    }
    adapters
        .iter()
        .filter(|adapter| adapter.physical() && adapter.connected)
        .min_by_key(|adapter| {
            (
                adapter.medium != Medium::Wired,
                armed(adapter) != Some(true),
                adapter.metric,
                adapter.index,
            )
        })
        .map(|adapter| pick(adapter, false))
}

/// The `<mac>` of `/serverinfo` for a request that reached the PC at `local`:
/// the [`primary`] adapter's, zeros for loopback or when no physical adapter
/// is connected.
pub fn reported_mac(
    adapters: &[Adapter],
    local: IpAddr,
    armed: impl Fn(&Adapter) -> Option<bool>,
) -> String {
    if local.to_canonical().is_loopback() {
        return format_mac(None);
    }
    format_mac(primary(adapters, local, armed).and_then(|primary| primary.adapter.mac))
}

/// `Option<[u8; 6]>` as `AA:BB:CC:DD:EE:FF` text, `null` for none.
mod mac_text {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};
    pub fn serialize<S: Serializer>(mac: &Option<[u8; 6]>, s: S) -> Result<S::Ok, S::Error> {
        match mac {
            Some(_) => s.serialize_str(&super::format_mac(*mac)),
            None => s.serialize_none(),
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<[u8; 6]>, D::Error> {
        let Some(text) = Option::<String>::deserialize(d)? else {
            return Ok(None);
        };
        super::parse_mac(&text)
            .map(Some)
            .ok_or_else(|| D::Error::custom(format!("not a MAC address: {text}")))
    }
}

/// `AA:BB:CC:DD:EE:FF`, `aa-bb-cc-dd-ee-ff` or `AABBCCDDEEFF`.
pub fn parse_mac(text: &str) -> Option<[u8; 6]> {
    let hex: String = text.chars().filter(|c| !matches!(c, ':' | '-')).collect();
    if hex.len() != 12 || !hex.is_ascii() {
        return None;
    }
    let mut mac = [0; 6];
    for (i, byte) in mac.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(mac)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<Adapter> {
        let text = match name {
            "this_pc" => include_str!("wake/fixtures/adapters_this_pc.json"),
            "hyperv_external" => include_str!("wake/fixtures/adapters_hyperv_external.json"),
            "tailscale" => include_str!("wake/fixtures/adapters_tailscale.json"),
            "zerotier" => include_str!("wake/fixtures/adapters_zerotier.json"),
            "wifi_laptop" => include_str!("wake/fixtures/adapters_wifi_laptop.json"),
            "wifi_and_dock" => include_str!("wake/fixtures/adapters_wifi_and_dock.json"),
            "two_wired" => include_str!("wake/fixtures/adapters_two_wired.json"),
            "nothing" => include_str!("wake/fixtures/adapters_nothing.json"),
            _ => unreachable!("{name}"),
        };
        serde_json::from_str(text).unwrap()
    }
    fn reported(name: &str, local: &str) -> String {
        reported_mac(&fixture(name), local.parse().unwrap(), |_| None)
    }
    const REALTEK: &str = "02:7C:16:B4:13:4F";

    #[test]
    fn this_pc_reports_the_realtek_for_ethernet_vethernet_and_tailscale_routes() {
        // The Ethernet route.
        assert_eq!(reported("this_pc", "192.168.4.20"), REALTEK);
        // IPv4-mapped, as a dual-stack listener sees it.
        assert_eq!(reported("this_pc", "::ffff:192.168.4.20"), REALTEK);
        // WSL's vEthernet: a Hyper-V MAC no packet wakes (the old bug).
        assert_eq!(reported("this_pc", "172.30.112.1"), REALTEK);
        // A Tailscale address on this PC: Wintun has no MAC (zeros before).
        assert_eq!(reported("tailscale", "100.101.102.103"), REALTEK);
        // An address no adapter owns still gets the physical NIC.
        assert_eq!(reported("this_pc", "10.9.9.9"), REALTEK);
    }

    #[test]
    fn virtual_and_vpn_macs_are_never_reported() {
        for (name, local) in [
            ("this_pc", "172.30.112.1"),
            ("zerotier", "10.147.17.5"),
            ("tailscale", "100.101.102.103"),
        ] {
            let adapters = fixture(name);
            let chosen = primary(&adapters, local.parse().unwrap(), |_| None).unwrap();
            assert!(chosen.adapter.physical(), "{name}");
            assert!(
                !chosen.route,
                "{name}: the client came through a virtual adapter"
            );
            for adapter in adapters.iter().filter(|a| !a.physical()) {
                assert_ne!(chosen.adapter.mac, adapter.mac, "{name}: {}", adapter.name);
            }
        }
        // ZeroTier's TAP and Wi-Fi Direct claim hardware but have no connector.
        let zerotier = fixture("zerotier");
        assert!(zerotier.iter().any(|a| a.hardware && !a.connector));
        assert_eq!(reported("zerotier", "10.147.17.5"), "02:11:22:33:44:55");
    }

    #[test]
    fn a_hyper_v_external_switch_reports_the_nic_it_shares_its_address_with() {
        let adapters = fixture("hyperv_external");
        let chosen = primary(&adapters, "192.168.1.50".parse().unwrap(), |_| None).unwrap();
        assert_eq!(format_mac(chosen.adapter.mac), "02:AA:BB:CC:DD:01");
        assert!(chosen.adapter.physical());
        assert!(chosen.route);
        // The vEthernet itself isn't physical.
        assert!(
            !adapters
                .iter()
                .find(|a| a.owns("192.168.1.50".parse().unwrap()))
                .unwrap()
                .physical()
        );
    }

    #[test]
    fn wifi_is_reported_as_experimental_unless_a_wired_nic_shares_its_subnet() {
        let laptop = fixture("wifi_laptop");
        let chosen = primary(&laptop, "192.168.4.12".parse().unwrap(), |_| None).unwrap();
        assert_eq!(chosen.adapter.medium, Medium::Wifi);
        assert!(chosen.experimental && chosen.route);
        assert_eq!(format_mac(chosen.adapter.mac), "02:E9:AA:CA:6E:C3");
        // Docked: the wired NIC on the same subnet wins over the Wi-Fi route.
        let docked = fixture("wifi_and_dock");
        let chosen = primary(&docked, "192.168.4.12".parse().unwrap(), |_| None).unwrap();
        assert_eq!(chosen.adapter.medium, Medium::Wired);
        assert!(!chosen.experimental && !chosen.route);
        // A wired NIC on another subnet doesn't.
        let mut elsewhere = docked.clone();
        for adapter in &mut elsewhere {
            if adapter.medium == Medium::Wired {
                adapter.ipv4 = vec![("10.0.0.7".parse().unwrap(), 24)];
            }
        }
        let chosen = primary(&elsewhere, "192.168.4.12".parse().unwrap(), |_| None).unwrap();
        assert_eq!(chosen.adapter.medium, Medium::Wifi);
    }

    #[test]
    fn with_two_wired_nics_the_route_wins_then_armed_then_metric_then_index() {
        let adapters = fixture("two_wired");
        // Each route reports its own NIC.
        assert_eq!(reported("two_wired", "192.168.1.10"), "02:00:00:00:00:01");
        assert_eq!(reported("two_wired", "10.0.0.10"), "02:00:00:00:00:02");
        // A VPN route: the lower metric first.
        assert_eq!(reported("two_wired", "100.64.0.1"), "02:00:00:00:00:02");
        // A wake-armed NIC beats a lower metric.
        let armed = |a: &Adapter| Some(a.mac == Some([2, 0, 0, 0, 0, 1]));
        let chosen = primary(&adapters, "100.64.0.1".parse().unwrap(), armed).unwrap();
        assert_eq!(format_mac(chosen.adapter.mac), "02:00:00:00:00:01");
        // Equal metric: the lower index.
        let mut equal = adapters.clone();
        for adapter in &mut equal {
            adapter.metric = 25;
        }
        let chosen = primary(&equal, "100.64.0.1".parse().unwrap(), |_| None).unwrap();
        assert_eq!(chosen.adapter.index, 4);
        // A disconnected NIC isn't picked for a VPN route.
        let mut unplugged = adapters.clone();
        unplugged.retain(|a| a.index != 9);
        for adapter in &mut unplugged {
            if adapter.index == 4 {
                adapter.connected = false;
            }
        }
        assert!(primary(&unplugged, "100.64.0.1".parse().unwrap(), |_| None).is_none());
    }

    #[test]
    fn nothing_connected_or_loopback_reports_zeros() {
        assert_eq!(reported("nothing", "172.30.112.1"), "00:00:00:00:00:00");
        assert_eq!(reported("nothing", "100.101.102.103"), "00:00:00:00:00:00");
        assert_eq!(reported("this_pc", "127.0.0.1"), "00:00:00:00:00:00");
        assert_eq!(reported("this_pc", "::1"), "00:00:00:00:00:00");
        assert_eq!(reported("this_pc", "::ffff:127.0.0.1"), "00:00:00:00:00:00");
    }

    #[test]
    fn broadcasts_follow_each_prefix() {
        let adapter = Adapter {
            ipv4: vec![
                ("192.168.4.20".parse().unwrap(), 24),
                ("10.1.2.3".parse().unwrap(), 16),
                ("169.254.1.1".parse().unwrap(), 16),
                ("10.9.9.9".parse().unwrap(), 32),
            ],
            ..Default::default()
        };
        assert_eq!(
            adapter.broadcasts(),
            [
                "10.1.255.255".parse::<Ipv4Addr>().unwrap(),
                "192.168.4.255".parse().unwrap()
            ]
        );
    }

    #[test]
    fn mac_rules() {
        assert!(!unicast_mac([0; 6]));
        assert!(!unicast_mac([1, 0, 0x5e, 0, 0, 1]));
        assert!(unicast_mac([2, 0, 0, 0, 0, 1]));
        assert_eq!(format_mac(None), "00:00:00:00:00:00");
        assert_eq!(
            format_mac(Some([4, 0x7c, 0x16, 0xb4, 0x13, 0x4f])),
            "04:7C:16:B4:13:4F"
        );
    }
}
