//! The order a device tries the ways to a host, and what it tells the user when none works.
//! The local network first (no added latency at home), then the host's IPv6 address, then a
//! forwarded IPv4 port, and only then a punched path, which needs the DHT and takes seconds.

use std::net::SocketAddr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Path {
    Lan,
    Ipv6,
    MappedIpv4,
    Punched,
}

impl Path {
    /// The path's name as the client shows it.
    pub fn label(self) -> &'static str {
        match self {
            Path::Lan => "Local network",
            Path::Ipv6 => "Internet (IPv6)",
            Path::MappedIpv4 => "Internet (forwarded port)",
            Path::Punched => "Internet (direct path)",
        }
    }
    /// A stable key for settings, logs and the JNI bridge.
    pub fn key(self) -> &'static str {
        match self {
            Path::Lan => "lan",
            Path::Ipv6 => "ipv6",
            Path::MappedIpv4 => "mapped",
            Path::Punched => "punched",
        }
    }
    pub fn from_key(key: &str) -> Option<Self> {
        [Path::Lan, Path::Ipv6, Path::MappedIpv4, Path::Punched]
            .into_iter()
            .find(|path| path.key() == key)
    }
}

/// What a device knows about where a host may be.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Known {
    pub lan: Vec<SocketAddr>,
    pub ipv6: Vec<SocketAddr>,
    pub mapped: Vec<SocketAddr>,
    /// The host gave this device a rendezvous secret, and internet play is on.
    pub punch: bool,
}

/// The attempts in order: each direct address under its path, then the punched path.
pub fn order(known: &Known) -> Vec<(Path, Option<SocketAddr>)> {
    let mut out = Vec::new();
    let mut seen = Vec::new();
    for (path, addresses) in [
        (Path::Lan, &known.lan),
        (Path::Ipv6, &known.ipv6),
        (Path::MappedIpv4, &known.mapped),
    ] {
        for &address in addresses {
            if !seen.contains(&address) {
                seen.push(address);
                out.push((path, Some(address)));
            }
        }
    }
    if known.punch {
        out.push((Path::Punched, None));
    }
    out
}

/// Why one path did not work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The path had no address to try.
    Unknown,
    /// Tried; no answer.
    NoAnswer,
    /// The punched path failed for this reason, in words.
    Failed(String),
}

/// What a device reports after every path failed.
pub fn failure_message(tried: &[(Path, Outcome)]) -> String {
    let mut parts = Vec::new();
    for path in [Path::Lan, Path::Ipv6, Path::MappedIpv4, Path::Punched] {
        let outcome = tried
            .iter()
            .rev()
            .find(|(tried, _)| *tried == path)
            .map(|(_, outcome)| outcome);
        let words = match (path, outcome) {
            (Path::Lan, Some(Outcome::NoAnswer)) => "the local network: no answer".to_owned(),
            (Path::Lan, _) => "the local network: no address known".to_owned(),
            (Path::Ipv6, Some(Outcome::NoAnswer)) => "IPv6: no answer".to_owned(),
            (Path::Ipv6, _) => "IPv6: the PC has no public IPv6 address known to this device".to_owned(),
            (Path::MappedIpv4, Some(Outcome::NoAnswer)) => "a forwarded port: no answer".to_owned(),
            (Path::MappedIpv4, _) => "a forwarded port: the PC reported none".to_owned(),
            (Path::Punched, Some(Outcome::Failed(reason))) => format!("a direct internet path: {reason}"),
            (Path::Punched, Some(Outcome::NoAnswer)) => "a direct internet path: the PC did not answer".to_owned(),
            (Path::Punched, _) => {
                "a direct internet path: connect once on the PC's network with Play over the internet on".to_owned()
            }
        };
        parts.push(words);
    }
    format!("The PC could not be reached. Tried {}.", parts.join("; "))
}

/// Tries the paths in [`order`] until `attempt` reaches the host on one; the error is the
/// [`failure_message`] for every path tried.
pub fn first_working<T>(
    known: &Known,
    mut attempt: impl FnMut(Path, Option<SocketAddr>) -> Result<T, Outcome>,
) -> Result<(Path, T), String> {
    let mut tried = Vec::new();
    for (path, address) in order(known) {
        match attempt(path, address) {
            Ok(value) => return Ok((path, value)),
            Err(outcome) => tried.push((path, outcome)),
        }
    }
    Err(failure_message(&tried))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lan_comes_first_and_punching_last() {
        let lan: SocketAddr = "192.168.1.5:47989".parse().unwrap();
        let v6: SocketAddr = "[2001:db8::5]:47989".parse().unwrap();
        let wan: SocketAddr = "203.0.113.5:47989".parse().unwrap();
        let known = Known {
            lan: vec![lan],
            ipv6: vec![v6],
            mapped: vec![wan, lan],
            punch: true,
        };
        assert_eq!(
            order(&known),
            vec![
                (Path::Lan, Some(lan)),
                (Path::Ipv6, Some(v6)),
                (Path::MappedIpv4, Some(wan)),
                (Path::Punched, None)
            ]
        );
        assert_eq!(order(&Known::default()), vec![]);
    }

    #[test]
    fn the_failure_names_every_path() {
        let message = failure_message(&[
            (Path::Lan, Outcome::NoAnswer),
            (
                Path::Punched,
                Outcome::Failed("both networks change ports per destination".into()),
            ),
        ]);
        assert!(message.contains("local network: no answer"));
        assert!(message.contains("IPv6: the PC has no public IPv6 address"));
        assert!(message.contains("forwarded port: the PC reported none"));
        assert!(message.contains("direct internet path: both networks"));
        assert_eq!(Path::from_key("punched"), Some(Path::Punched));
        assert_eq!(Path::from_key(Path::Ipv6.key()), Some(Path::Ipv6));
        assert_eq!(Path::from_key("relay"), None);
    }
}
