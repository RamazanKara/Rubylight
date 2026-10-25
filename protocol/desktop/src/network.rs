use std::net::{IpAddr, ToSocketAddrs};
use std::sync::mpsc::{channel, Receiver};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Mobile, HomeWifi, Wifi, Other }

#[derive(Clone, Default)]
pub struct Network {
    pub wifi: bool,
    pub cellular: bool,
    pub metered: bool,
    pub ssid: Option<String>,
    pub link_mbps: u32,
    pub error: Option<String>,
}

impl Network {
    pub fn kind(&self, home: &[String]) -> Kind {
        if self.cellular || self.metered { Kind::Mobile }
        else if !self.wifi { Kind::Other }
        else if self.ssid.as_ref().is_some_and(|name| home.contains(name)) { Kind::HomeWifi }
        else { Kind::Wifi }
    }

    /// The link speed sent to the host as the pacing hint, for every codec: 0 on Wi-Fi or
    /// cellular, where the adapter speed says nothing about the path.
    pub fn wired_link_mbps(&self) -> u32 {
        if self.wifi || self.cellular { 0 } else { self.link_mbps.min(9999) }
    }

    /// Whether the route to the host is Wi-Fi or cellular, sent as a hint so the host paces for
    /// a wireless link from the first frame. False when wired, unknown or unreadable.
    pub fn wireless(&self) -> bool {
        self.wifi || self.cellular
    }

    pub fn description(&self, home: &[String]) -> String {
        if let Some(error) = &self.error { return format!("Network details unavailable: {error}"); }
        let name = match self.kind(home) {
            Kind::Mobile => "Mobile or metered network",
            Kind::HomeWifi => "Home Wi-Fi",
            Kind::Wifi => "Other Wi-Fi",
            Kind::Other => "Ethernet or other network",
        };
        if self.wifi && self.ssid.is_none() { format!("{name} · Wi-Fi name unavailable") }
        else { name.into() }
    }
}

/// What the interface carrying the traffic to a host is made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Medium { Wired, Wireless, Other }

/// The route to one host: whether it leaves over Wi-Fi or cellular, and the speed of the port when
/// it is wired.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Toward {
    pub wireless: bool,
    pub link_mbps: u32,
}

impl Toward {
    /// Tunnels, loopback and anything else that does not say what carries the traffic are `None`,
    /// so the caller falls back to the internet connection.
    fn of(medium: Medium, link_mbps: u32) -> Option<Toward> {
        match medium {
            Medium::Wired => Some(Toward { wireless: false, link_mbps }),
            Medium::Wireless => Some(Toward { wireless: true, link_mbps: 0 }),
            Medium::Other => None,
        }
    }
}

/// The interface the system routes traffic for `host` over, not the one with the internet
/// connection: a PC on the same Ethernet segment is wired even when Wi-Fi is the default route.
/// Blocks on name lookup. Errs with why the caller must fall back to the internet connection.
pub fn toward(host: &str) -> Result<Toward, Fallback> {
    let ip = (host, 0).to_socket_addrs().ok().and_then(|mut found| found.next()).ok_or(Fallback::Resolve)?.ip();
    let (medium, link_mbps) = interface_toward(ip).ok_or(Fallback::Route)?;
    Toward::of(medium, link_mbps).ok_or(Fallback::NotTelling)
}

/// Why `toward` has no answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// The host name did not resolve to an address.
    Resolve,
    /// The system has no route to the address, or its interface could not be read.
    Route,
    /// The interface is loopback, a tunnel, virtual or of a type that says nothing.
    NotTelling,
}

impl std::fmt::Display for Fallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Fallback::Resolve => "host name did not resolve",
            Fallback::Route => "route lookup failed",
            Fallback::NotTelling => "interface does not say",
        })
    }
}

impl Network {
    /// This connection as it applies to the route to one host: the Wi-Fi flag and the link speed
    /// follow the interface that reaches the host, so a PC on the local Ethernet is wired even
    /// when Wi-Fi carries the internet. Unchanged when the route does not tell.
    ///
    /// On a wireless route the speed stays the Wi-Fi or cellular rate the system reports (what a
    /// stored pace calibration is keyed on); the hint sent to the host stays 0 through
    /// `wired_link_mbps`. With a wired internet connection the Wi-Fi name is unknown, so no
    /// Wi-Fi calibration matches.
    pub fn toward(&self, route: Result<Toward, Fallback>) -> Network {
        let Ok(route) = route else { return self.clone() };
        Network {
            wifi: route.wireless && (self.wifi || !self.cellular),
            cellular: route.wireless && self.cellular,
            link_mbps: match (route.wireless, self.wifi || self.cellular) {
                (false, _) => route.link_mbps,
                (true, true) => self.link_mbps,
                (true, false) => 0,
            },
            ssid: if route.wireless { self.ssid.clone() } else { None },
            error: None,
            ..self.clone()
        }
    }
}

/// Windows interface types (IANA ifType): IEEE 802.11 and the two WWAN types are wireless;
/// Ethernet in its several spellings is wired. Virtual adapters (TAP-Windows, ZeroTier, Hamachi)
/// also report Ethernet with made-up speeds, so only a hardware interface counts.
#[cfg_attr(not(windows), allow(dead_code))]
fn medium_of_if_type(if_type: u32, hardware: bool) -> Medium {
    if !hardware { return Medium::Other; }
    match if_type {
        71 | 243 | 244 => Medium::Wireless,
        6 | 26 | 62 | 69 | 117 => Medium::Wired,
        _ => Medium::Other,
    }
}

/// Linux: a `wireless` directory or a wlan/wwan `DEVTYPE` in sysfs is wireless, an Ethernet-type
/// (ARPHRD_ETHER, 1) interface without them is wired. Interfaces without a `device` link (tap,
/// zerotier, dummy) are virtual, with a made-up speed, and say nothing.
#[cfg_attr(windows, allow(dead_code))]
fn medium_of_sysfs(has_device: bool, has_wireless_dir: bool, devtype: Option<&str>, arphrd: Option<u32>) -> Medium {
    if !has_device {
        Medium::Other
    } else if has_wireless_dir || matches!(devtype, Some("wlan" | "wwan")) {
        Medium::Wireless
    } else if arphrd == Some(1) && !matches!(devtype, Some("bridge" | "vlan" | "veth")) {
        Medium::Wired
    } else {
        Medium::Other
    }
}

/// The interface name from one line of `ip -o route get`: `... dev eth0 src ...`.
#[cfg_attr(windows, allow(dead_code))]
fn parse_route_dev(output: &str) -> Option<&str> {
    let mut words = output.split_whitespace();
    words.find(|word| *word == "dev")?;
    words.next().filter(|name| {
        (1..=15).contains(&name.len()) && name.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
    })
}

#[cfg(windows)]
fn interface_toward(ip: IpAddr) -> Option<(Medium, u32)> {
    use windows::Win32::NetworkManagement::IpHelper::{GetBestInterfaceEx, GetIfEntry2, MIB_IF_ROW2};
    use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6, SOCKADDR, SOCKADDR_IN, SOCKADDR_IN6};
    let mut index = 0u32;
    let status = unsafe {
        match ip {
            IpAddr::V4(v4) => {
                let mut address: SOCKADDR_IN = std::mem::zeroed();
                address.sin_family = AF_INET;
                address.sin_addr.S_un.S_addr = u32::from_ne_bytes(v4.octets());
                GetBestInterfaceEx(&address as *const SOCKADDR_IN as *const SOCKADDR, &mut index)
            }
            IpAddr::V6(v6) => {
                let mut address: SOCKADDR_IN6 = std::mem::zeroed();
                address.sin6_family = AF_INET6;
                address.sin6_addr.u.Byte = v6.octets();
                GetBestInterfaceEx(&address as *const SOCKADDR_IN6 as *const SOCKADDR, &mut index)
            }
        }
    };
    if status != 0 { return None; }
    let mut row: MIB_IF_ROW2 = unsafe { std::mem::zeroed() };
    row.InterfaceIndex = index;
    if unsafe { GetIfEntry2(&mut row) }.0 != 0 { return None; }
    // All ones means the adapter does not report a speed.
    let mbps = if row.ReceiveLinkSpeed == u64::MAX { 0 } else { (row.ReceiveLinkSpeed / 1_000_000).min(u32::MAX as u64) as u32 };
    // Bit 0 of InterfaceAndOperStatusFlags is HardwareInterface.
    let hardware = row.InterfaceAndOperStatusFlags._bitfield & 1 != 0;
    Some((medium_of_if_type(row.Type, hardware), mbps))
}

#[cfg(not(windows))]
fn interface_toward(ip: IpAddr) -> Option<(Medium, u32)> {
    let output = std::process::Command::new("ip").args(["-o", "route", "get", &ip.to_string()]).output().ok()?;
    if !output.status.success() { return None; }
    let text = String::from_utf8_lossy(&output.stdout);
    let name = parse_route_dev(&text)?;
    let base = format!("/sys/class/net/{name}");
    let read = |file: &str| std::fs::read_to_string(format!("{base}/{file}")).ok();
    let devtype = read("uevent").and_then(|uevent| {
        uevent.lines().find_map(|line| line.strip_prefix("DEVTYPE=").map(str::to_owned))
    });
    let arphrd = read("type").and_then(|t| t.trim().parse::<u32>().ok());
    let exists = |entry: &str| std::path::Path::new(&format!("{base}/{entry}")).exists();
    let medium = medium_of_sysfs(exists("device"), exists("wireless"), devtype.as_deref(), arphrd);
    // "-1" is what a port without a speed reports.
    let mbps = read("speed").and_then(|t| t.trim().parse::<i64>().ok()).filter(|n| *n > 0).map_or(0, |n| n.min(u32::MAX as i64) as u32);
    Some((medium, mbps))
}

pub fn query(waker: crate::hosts::Waker) -> Receiver<Network> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let result = read().unwrap_or_else(|error| Network { error: Some(error), ..Default::default() });
        let _ = tx.send(result);
        waker();
    });
    rx
}

#[cfg(windows)]
fn read() -> Result<Network, String> {
    use windows::Networking::Connectivity::{NetworkInformation, NetworkCostType};
    use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};
    // Each query runs on a fresh worker; balance its WinRT apartment on every return path.
    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.map_err(|e| e.to_string())?;
    let result = (|| -> windows::core::Result<Network> {
        let profile = NetworkInformation::GetInternetConnectionProfile().or_else(|_| {
            let profiles = NetworkInformation::GetConnectionProfiles()?;
            for profile in profiles {
                if profile.GetNetworkConnectivityLevel()?.0 > 0 { return Ok(profile); }
            }
            Err(windows::core::Error::empty())
        })?;
        let wifi = profile.IsWlanConnectionProfile()?;
        let cost = profile.GetConnectionCost()?;
        let metered = matches!(cost.NetworkCostType()?, NetworkCostType::Fixed | NetworkCostType::Variable)
            || cost.Roaming()? || cost.OverDataLimit()?;
        Ok(Network {
            wifi,
            cellular: profile.IsWwanConnectionProfile()?,
            metered,
            ssid: wifi.then(|| profile.WlanConnectionProfileDetails().and_then(|w| w.GetConnectedSsid()).ok())
                .flatten().map(|s| s.to_string()).filter(|s| !s.is_empty()),
            link_mbps: profile.NetworkAdapter().and_then(|a| a.InboundMaxBitsPerSecond()).map_or(0, |n| (n / 1_000_000).min(u32::MAX as u64) as u32),
            error: None,
        })
    })();
    unsafe { RoUninitialize(); }
    result.map_err(|e| e.to_string())
}

#[cfg(not(windows))]
fn read() -> Result<Network, String> {
    use zbus::{blocking::{connection::Builder, Proxy}, zvariant::OwnedObjectPath};
    let read = || -> zbus::Result<Network> {
        let connection = Builder::system()?.method_timeout(std::time::Duration::from_secs(2)).build()?;
        let proxy = |path: &str, interface: &str| Proxy::new_owned(connection.clone(), "org.freedesktop.NetworkManager", path.to_owned(), interface.to_owned());
        let manager = proxy("/org/freedesktop/NetworkManager", "org.freedesktop.NetworkManager")?;
        let primary: OwnedObjectPath = manager.get_property("PrimaryConnection")?;
        let active = proxy(primary.as_str(), "org.freedesktop.NetworkManager.Connection.Active")?;
        let kind: String = active.get_property("Type")?;
        let metered: u32 = manager.get_property("Metered")?;
        let mut network = Network { wifi: kind == "802-11-wireless", cellular: matches!(kind.as_str(), "gsm" | "cdma"),
            metered: matches!(metered, 1 | 3), ..Default::default() };
        let devices: Vec<OwnedObjectPath> = active.get_property("Devices")?;
        if let Some(path) = devices.first() {
            if network.wifi {
                let device = proxy(path.as_str(), "org.freedesktop.NetworkManager.Device.Wireless")?;
                network.link_mbps = device.get_property::<u32>("Bitrate").unwrap_or(0) / 1000;
                network.ssid = (|| -> zbus::Result<Option<String>> {
                    let ap: OwnedObjectPath = device.get_property("ActiveAccessPoint")?;
                    let access = proxy(ap.as_str(), "org.freedesktop.NetworkManager.AccessPoint")?;
                    let bytes: Vec<u8> = access.get_property("Ssid")?;
                    Ok(String::from_utf8(bytes).ok().filter(|s| !s.is_empty()))
                })().ok().flatten();
            } else if kind == "802-3-ethernet" {
                network.link_mbps = proxy(path.as_str(), "org.freedesktop.NetworkManager.Device.Wired")?
                    .get_property("Speed").unwrap_or(0);
            }
        }
        Ok(network)
    };
    read().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_hint_is_the_wired_speed_and_zero_on_wireless() {
        let wired = Network { link_mbps: 2500, ..Default::default() };
        assert_eq!(wired.wired_link_mbps(), 2500);
        assert_eq!(Network { link_mbps: 40_000, ..wired.clone() }.wired_link_mbps(), 9999);
        assert_eq!(Network { wifi: true, ..wired.clone() }.wired_link_mbps(), 0);
        assert_eq!(Network { cellular: true, ..wired.clone() }.wired_link_mbps(), 0);
        assert_eq!(Network::default().wired_link_mbps(), 0);
    }

    #[test]
    fn wireless_is_wifi_or_cellular_and_nothing_else() {
        assert!(Network { wifi: true, ..Default::default() }.wireless());
        assert!(Network { cellular: true, ..Default::default() }.wireless());
        assert!(Network { wifi: true, cellular: true, ..Default::default() }.wireless());
        assert!(!Network { link_mbps: 2500, ..Default::default() }.wireless());
        // Metered alone says nothing about the medium; an unreadable network is not wireless.
        assert!(!Network { metered: true, ..Default::default() }.wireless());
        assert!(!Network { error: Some("no adapter".into()), ..Default::default() }.wireless());
    }

    #[test]
    fn windows_interface_types_map_to_a_medium() {
        assert_eq!(medium_of_if_type(71, true), Medium::Wireless); // IEEE 802.11
        assert_eq!(medium_of_if_type(243, true), Medium::Wireless); // WWAN
        assert_eq!(medium_of_if_type(244, true), Medium::Wireless);
        assert_eq!(medium_of_if_type(6, true), Medium::Wired); // Ethernet
        assert_eq!(medium_of_if_type(117, true), Medium::Wired); // Gigabit Ethernet
        // Loopback, tunnel, PPP and unknown types say nothing.
        assert_eq!(medium_of_if_type(24, true), Medium::Other);
        assert_eq!(medium_of_if_type(131, true), Medium::Other);
        assert_eq!(medium_of_if_type(23, true), Medium::Other);
        assert_eq!(medium_of_if_type(0, true), Medium::Other);
    }

    #[test]
    fn windows_virtual_adapters_do_not_count_as_wired() {
        // TAP-Windows, ZeroTier and Hamachi report ifType 6 without the HardwareInterface bit.
        assert_eq!(medium_of_if_type(6, false), Medium::Other);
        assert_eq!(medium_of_if_type(117, false), Medium::Other);
        assert_eq!(medium_of_if_type(71, false), Medium::Other);
    }

    #[test]
    fn sysfs_attributes_map_to_a_medium() {
        assert_eq!(medium_of_sysfs(true, true, None, Some(1)), Medium::Wireless);
        assert_eq!(medium_of_sysfs(true, false, Some("wlan"), Some(1)), Medium::Wireless);
        assert_eq!(medium_of_sysfs(true, false, Some("wwan"), Some(1)), Medium::Wireless);
        assert_eq!(medium_of_sysfs(true, false, None, Some(1)), Medium::Wired);
        assert_eq!(medium_of_sysfs(true, false, Some("bridge"), Some(1)), Medium::Other);
        assert_eq!(medium_of_sysfs(true, false, None, Some(772)), Medium::Other); // loopback
        assert_eq!(medium_of_sysfs(true, false, None, Some(65534)), Medium::Other); // tun
        assert_eq!(medium_of_sysfs(true, false, None, None), Medium::Other);
    }

    #[test]
    fn linux_interfaces_without_a_device_do_not_count_as_wired() {
        // tap and zerotier devices look like Ethernet but have no /sys/class/net/<if>/device.
        assert_eq!(medium_of_sysfs(false, false, None, Some(1)), Medium::Other);
        assert_eq!(medium_of_sysfs(false, false, Some("wlan"), Some(1)), Medium::Other);
    }

    #[test]
    fn network_follows_the_route_and_falls_back_when_it_does_not_tell() {
        let wifi = Network { wifi: true, ssid: Some("home".into()), link_mbps: 866, ..Default::default() };
        let wired = wifi.toward(Ok(Toward { wireless: false, link_mbps: 2500 }));
        assert!(!wired.wifi && !wired.wireless());
        assert_eq!((wired.link_mbps, wired.ssid), (2500, None));
        let ethernet = Network { link_mbps: 1000, ..Default::default() };
        let wireless = ethernet.toward(Ok(Toward { wireless: true, link_mbps: 0 }));
        assert!(wireless.wifi && !wireless.cellular);
        assert_eq!(wireless.link_mbps, 0); // the internet is wired: no Wi-Fi rate to keep
        assert_eq!(wireless.ssid, None);
        // On Wi-Fi the rate and name stay what the system reports; the host hint is still 0.
        let kept = wifi.toward(Ok(Toward { wireless: true, link_mbps: 0 }));
        assert_eq!((kept.link_mbps, kept.ssid.as_deref()), (866, Some("home")));
        assert_eq!(kept.wired_link_mbps(), 0);
        let cellular = Network { cellular: true, ..Default::default() }.toward(Ok(Toward { wireless: true, link_mbps: 0 }));
        assert!(cellular.cellular && !cellular.wifi);
        assert_eq!(wifi.toward(Err(Fallback::NotTelling)).link_mbps, 866);
        assert!(wifi.toward(Err(Fallback::Route)).wifi);
        assert!(wifi.toward(Err(Fallback::Resolve)).wifi);
        // The route is known even when the connection profile could not be read.
        let unreadable = Network { error: Some("no profile".into()), ..Default::default() };
        assert!(unreadable.toward(Ok(Toward { wireless: false, link_mbps: 1000 })).error.is_none());
        assert!(unreadable.toward(Err(Fallback::Route)).error.is_some());
    }

    #[test]
    fn route_lookup_output_names_the_interface() {
        assert_eq!(parse_route_dev("192.168.1.9 dev wlp3s0 src 192.168.1.20 uid 1000"), Some("wlp3s0"));
        assert_eq!(parse_route_dev("10.0.0.1 via 10.0.0.254 dev eth0 src 10.0.0.2\n"), Some("eth0"));
        assert_eq!(parse_route_dev("local 127.0.0.1 dev lo src 127.0.0.1"), Some("lo"));
        assert_eq!(parse_route_dev("RTNETLINK answers: Network is unreachable"), None);
        assert_eq!(parse_route_dev("1.2.3.4 dev"), None);
        assert_eq!(parse_route_dev("1.2.3.4 dev bad/name"), None);
        assert_eq!(parse_route_dev("1.2.3.4 dev 0123456789abcdef"), None);
    }

    #[test]
    fn only_wired_and_wireless_media_decide_the_route() {
        assert_eq!(Toward::of(Medium::Wired, 2500), Some(Toward { wireless: false, link_mbps: 2500 }));
        // A wireless port's speed says nothing about the path.
        assert_eq!(Toward::of(Medium::Wireless, 866), Some(Toward { wireless: true, link_mbps: 0 }));
        assert_eq!(Toward::of(Medium::Other, 1000), None);
    }

    #[test]
    fn fallback_reasons_are_told_apart() {
        let reasons = [Fallback::Resolve, Fallback::Route, Fallback::NotTelling].map(|why| why.to_string());
        assert!(reasons[0] != reasons[1] && reasons[1] != reasons[2] && reasons[0] != reasons[2]);
    }
}
