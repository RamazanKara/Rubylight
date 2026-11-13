//! Play over the internet (`GET /internet-play/v1`, paired HTTPS): what a device learns from
//! the host while it is on the host's network, kept per host for when it is away. The reply
//! carries the rendezvous secret this device and the host share, the host's public endpoints,
//! its IPv6 addresses and whether its ports are forwarded. Gate on `Feature::InternetPlay`.
//!
//! Devices keep the reply as it came ([`InternetPlay::parse`] reads it again), so a field a
//! newer host adds survives in the store.

use super::{Error, Feature, HostApiError, HostCapabilities, HostClient};
use rubylight_traverse::{
    client::HostInfo,
    path::Known,
    record::{Nat, Ports},
};
use serde_json::Value;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

/// The largest reply read.
pub const MAX_INTERNET_PLAY_BYTES: usize = 4096;
const CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// The host's reply.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InternetPlay {
    /// Play over the internet is on at the host.
    pub enabled: bool,
    /// Automatic port forwarding (`upnp`) is on at the host.
    pub port_mapping: bool,
    /// The router's public IPv4 address, when the host knows it.
    pub external_ipv4: Option<Ipv4Addr>,
    /// Every streaming port is forwarded under its own number.
    pub mapped_ports: bool,
    /// The host's global IPv6 addresses, when it listens on IPv6.
    pub ipv6: Vec<Ipv6Addr>,
    pub ports: Ports,
    /// The rendezvous secret for this device; only while internet play is on.
    pub secret: Option<[u8; 32]>,
    /// The host's rendezvous session and public endpoints when it answered.
    pub session: Option<[u8; 16]>,
    pub endpoints: Vec<SocketAddr>,
    pub nat: Nat,
}

fn invalid() -> HostApiError {
    HostApiError::Host(Error::Malformed("Invalid internet play response".into()))
}

fn hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let bytes = text.as_bytes();
    if bytes.len() != N * 2 {
        return None;
    }
    let mut out = [0; N];
    for (i, pair) in bytes.chunks(2).enumerate() {
        let digit = |c: u8| (c as char).to_digit(16);
        out[i] = (digit(pair[0])? * 16 + digit(pair[1])?) as u8;
    }
    Some(out)
}

fn port(ports: &Value, name: &str) -> Result<u16, HostApiError> {
    ports
        .get(name)
        .and_then(Value::as_u64)
        .and_then(|port| u16::try_from(port).ok())
        .filter(|&port| port != 0)
        .ok_or_else(invalid)
}

impl InternetPlay {
    /// A reply body. A version other than 1 is [`HostApiError::UnsupportedVersion`].
    pub fn parse(json: &str) -> Result<Self, HostApiError> {
        if json.len() > MAX_INTERNET_PLAY_BYTES {
            return Err(invalid());
        }
        let root: Value = serde_json::from_str(json).map_err(|_| invalid())?;
        let root = root.as_object().ok_or_else(invalid)?;
        if root
            .get("version")
            .and_then(Value::as_f64)
            .ok_or_else(invalid)?
            != 1.0
        {
            return Err(HostApiError::UnsupportedVersion);
        }
        let flag = |name: &str| root.get(name).and_then(Value::as_bool).unwrap_or(false);
        let ports = root.get("ports").ok_or_else(invalid)?;
        let ports = Ports {
            http: port(ports, "http")?,
            https: port(ports, "https")?,
            rtsp: port(ports, "rtsp")?,
            video: port(ports, "video")?,
            audio: port(ports, "audio")?,
            control: port(ports, "control")?,
            mic: port(ports, "mic")?,
        };
        let strings = |name: &str| -> Vec<String> {
            root.get(name)
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        let text = |name: &str| root.get(name).and_then(Value::as_str);
        let nat = match text("nat") {
            Some("none") => Nat::None,
            Some("cone") => Nat::Cone,
            Some("symmetric") => Nat::Symmetric,
            _ => Nat::Unknown,
        };
        Ok(Self {
            enabled: flag("enabled"),
            port_mapping: flag("portMapping"),
            external_ipv4: text("externalIpv4").and_then(|ip| ip.parse().ok()),
            mapped_ports: flag("mappedPorts"),
            ipv6: strings("ipv6")
                .iter()
                .filter_map(|ip| ip.parse().ok())
                .take(4)
                .collect(),
            ports,
            secret: text("secret").and_then(hex::<32>),
            session: text("session").and_then(hex::<16>),
            endpoints: strings("endpoints")
                .iter()
                .filter_map(|endpoint| endpoint.parse().ok())
                .take(rubylight_traverse::record::MAX_ENDPOINTS)
                .collect(),
            nat,
        })
    }

    /// The host as the rendezvous starts from it until the DHT has something newer.
    pub fn host_info(&self) -> HostInfo {
        HostInfo {
            session: self.session,
            endpoints: self.endpoints.clone(),
            nat: self.nat,
            ports: self.ports,
        }
    }

    /// Whether a direct internet path may exist without the rendezvous: a forwarded IPv4 port
    /// or an IPv6 address.
    pub fn direct(&self) -> bool {
        !self.ipv6.is_empty() || self.mapped()
    }

    fn mapped(&self) -> bool {
        self.mapped_ports && self.external_ipv4.is_some()
    }

    /// Where the host may be, in [`rubylight_traverse::path::order`]: `lan` (the addresses it
    /// was found at), its IPv6 addresses, its forwarded IPv4 address, then a punched path when
    /// `punch` (this device has internet play on) and the host gave this device a secret.
    pub fn known(&self, lan: &[IpAddr], punch: bool) -> Known {
        let at = |ip: IpAddr| SocketAddr::new(ip, self.ports.http);
        Known {
            lan: lan.iter().copied().map(at).collect(),
            ipv6: self.ipv6.iter().map(|&ip| at(IpAddr::V6(ip))).collect(),
            mapped: self
                .external_ipv4
                .filter(|_| self.mapped())
                .map(|ip| at(IpAddr::V4(ip)))
                .into_iter()
                .collect(),
            punch: punch && self.enabled && self.secret.is_some(),
        }
    }
}

impl HostClient {
    /// The host's internet play reply, as text to keep and as read. `capabilities` comes from a
    /// paired serverinfo; without `Feature::InternetPlay` nothing is sent and the answer is
    /// [`HostApiError::Unavailable`].
    pub fn internet_play(
        &self,
        capabilities: &HostCapabilities,
    ) -> Result<(String, InternetPlay), HostApiError> {
        HostApiError::require(capabilities, Feature::InternetPlay)?;
        let response = self.pinned_get(
            "internet-play/v1",
            MAX_INTERNET_PLAY_BYTES,
            CALL_TIMEOUT,
            true,
        )?;
        let text = String::from_utf8(response.body).map_err(|_| invalid())?;
        let parsed = InternetPlay::parse(&text)?;
        Ok((text, parsed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY: &str = r#"{"version":1,"enabled":true,"portMapping":true,
        "externalIpv4":"203.0.113.5","mappedPorts":true,"ipv6":["2001:db8::5","fe80::bad"],
        "ports":{"http":47989,"https":47984,"rtsp":48010,"video":47998,"audio":48000,"control":47999,"mic":48002},
        "secret":"000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
        "session":"f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff","endpoints":["203.0.113.5:40000"],"nat":"cone"}"#;

    #[test]
    fn the_reply_gives_the_rendezvous_secret_endpoints_and_the_direct_paths_in_order() {
        let info = InternetPlay::parse(REPLY).unwrap();
        assert!(info.enabled && info.port_mapping && info.mapped_ports);
        assert_eq!(info.secret.unwrap()[31], 0x1f);
        assert_eq!(info.session.unwrap()[0], 0xf0);
        assert_eq!(info.nat, Nat::Cone);
        assert_eq!(info.ports.rtsp, 48010);
        // A link-local address is left in: the host sends only global ones, and parsing keeps
        // what it was sent.
        assert_eq!(info.ipv6.len(), 2);
        let known = info.known(&["192.168.1.10".parse().unwrap()], true);
        let order = rubylight_traverse::path::order(&known);
        use rubylight_traverse::path::Path;
        let paths: Vec<Path> = order.iter().map(|(path, _)| *path).collect();
        assert_eq!(
            paths,
            [
                Path::Lan,
                Path::Ipv6,
                Path::Ipv6,
                Path::MappedIpv4,
                Path::Punched
            ]
        );
        assert_eq!(order[0].1.unwrap().to_string(), "192.168.1.10:47989");
        assert_eq!(order[3].1.unwrap().to_string(), "203.0.113.5:47989");
        assert_eq!(info.host_info().endpoints.len(), 1);
    }

    #[test]
    fn without_a_secret_or_with_internet_play_off_on_either_side_no_punching_is_tried() {
        let off = REPLY
            .replace(r#""enabled":true"#, r#""enabled":false"#)
            .replace(
                r#""secret":"000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f","#,
                "",
            );
        let info = InternetPlay::parse(&off).unwrap();
        assert!(info.secret.is_none());
        assert!(!info.known(&[], true).punch);
        assert!(!InternetPlay::parse(REPLY).unwrap().known(&[], false).punch);
        // A forwarded port that changed numbers is no direct path.
        let moved =
            InternetPlay::parse(&REPLY.replace(r#""mappedPorts":true"#, r#""mappedPorts":false"#))
                .unwrap();
        assert!(moved.known(&[], true).mapped.is_empty());
    }

    #[test]
    fn a_malformed_or_newer_reply_is_refused() {
        assert!(matches!(
            InternetPlay::parse(&REPLY.replace(r#""version":1"#, r#""version":2"#)),
            Err(HostApiError::UnsupportedVersion)
        ));
        assert!(InternetPlay::parse(&REPLY.replace("48010", "0")).is_err());
        assert!(InternetPlay::parse("[]").is_err());
        assert!(InternetPlay::parse(&" ".repeat(MAX_INTERNET_PLAY_BYTES + 1)).is_err());
        // A secret of the wrong length is no secret.
        let short = InternetPlay::parse(&REPLY.replace("1e1f\"", "1e\"")).unwrap();
        assert!(short.secret.is_none());
    }
}
