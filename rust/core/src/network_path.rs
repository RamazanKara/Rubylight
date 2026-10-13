//! Privacy-preserving interpretation of a local Tailscale status snapshot.
//! Never return another peer's names, addresses, keys, routes or relay location.
use serde::Serialize;
use serde_json::Value;
use std::net::IpAddr;

pub const MAX_STATUS_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NetworkPath {
    pub version: u8,
    pub kind: &'static str,
}

impl NetworkPath {
    pub fn unknown() -> Self {
        Self {
            version: 1,
            kind: "unknown",
        }
    }
}

pub fn from_tailscale_status(bytes: &[u8], remote: IpAddr) -> NetworkPath {
    let unknown = NetworkPath::unknown();
    if bytes.len() > MAX_STATUS_BYTES {
        return unknown;
    }
    let Ok(status) = serde_json::from_slice::<Value>(bytes) else {
        return unknown;
    };
    if status["BackendState"].as_str() != Some("Running") {
        return unknown;
    }
    let Some(peers) = status["Peer"].as_object() else {
        return unknown;
    };
    let mut matches = peers.values().filter(|peer| {
        peer["TailscaleIPs"].as_array().is_some_and(|ips| {
            ips.iter().any(|ip| {
                ip.as_str()
                    .and_then(|s| s.parse::<IpAddr>().ok())
                    .is_some_and(|ip| ip.to_canonical() == remote.to_canonical())
            })
        })
    });
    let Some(peer) = matches.next() else {
        return unknown;
    };
    // An idle snapshot only describes a previous route. A DERP region alone is
    // not evidence that the current connection uses a relay.
    if matches.next().is_some() || peer["Active"].as_bool() != Some(true) {
        return unknown;
    }
    let nonempty = |key: &str| peer[key].as_str().is_some_and(|s| !s.is_empty());
    let kind = if nonempty("PeerRelay") {
        "peer_relay"
    } else if nonempty("CurAddr") {
        "direct"
    } else if nonempty("Relay") {
        "relay"
    } else {
        "unknown"
    };
    NetworkPath { version: 1, kind }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn sample(peer: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({"BackendState":"Running", "Peer":{"key":peer}})).unwrap()
    }
    fn ip() -> IpAddr {
        "100.64.0.4".parse().unwrap()
    }
    #[test]
    fn only_active_exact_peer_is_reported_and_private_fields_are_absent() {
        let bytes = sample(json!({"TailscaleIPs":["100.64.0.4"], "Active":true,
            "CurAddr":"192.0.2.4:41641", "Relay":"fra", "DNSName":"private-name"}));
        let report = from_tailscale_status(&bytes, ip());
        assert_eq!(
            serde_json::to_string(&report).unwrap(),
            r#"{"version":1,"kind":"direct"}"#
        );
        assert_eq!(
            from_tailscale_status(&bytes, "100.64.0.5".parse().unwrap()),
            NetworkPath::unknown()
        );
        assert_eq!(
            from_tailscale_status(&bytes, "::ffff:100.64.0.4".parse().unwrap()),
            report
        );
    }
    #[test]
    fn inactive_missing_and_malformed_data_never_infer_direct_or_relay() {
        for peer in [
            json!({"TailscaleIPs":["100.64.0.4"],"Active":false,"Relay":"fra"}),
            json!({"TailscaleIPs":["100.64.0.4"],"Active":true}),
            json!({"TailscaleIPs":["100.64.0.4"],"Active":"true","CurAddr":"x"}),
        ] {
            assert_eq!(
                from_tailscale_status(&sample(peer), ip()),
                NetworkPath::unknown()
            );
        }
        assert_eq!(
            from_tailscale_status(b"not json", ip()),
            NetworkPath::unknown()
        );
        assert_eq!(
            from_tailscale_status(&vec![b' '; MAX_STATUS_BYTES + 1], ip()),
            NetworkPath::unknown()
        );
    }
    #[test]
    fn relay_types_and_ambiguous_status() {
        for (field, kind) in [("Relay", "relay"), ("PeerRelay", "peer_relay")] {
            let mut peer = json!({"TailscaleIPs":["100.64.0.4"],"Active":true});
            peer[field] = json!("location");
            assert_eq!(
                from_tailscale_status(&sample(peer.clone()), ip()).kind,
                kind
            );
            let duplicate =
                serde_json::to_vec(&json!({"BackendState":"Running", "Peer":{"a":peer,"b":peer}}))
                    .unwrap();
            assert_eq!(
                from_tailscale_status(&duplicate, ip()),
                NetworkPath::unknown()
            );
        }
    }
}
