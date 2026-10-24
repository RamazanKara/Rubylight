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
}
