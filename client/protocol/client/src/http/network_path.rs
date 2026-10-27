//! The route to the host as the host sees it (`/api/network-path`), read as
//! NvHTTP.getNetworkPath and NetworkPathInfo.java do: only the current peer's route category,
//! no peer list or addresses. Gate on `Feature::NetworkPath`.

use super::{Error, Feature, HostApiError, HostCapabilities, HostClient};
use serde_json::Value;
use std::time::Duration;

/// The largest reply read.
pub const MAX_NETWORK_PATH_BYTES: usize = 512;
/// The whole request, connect included, as Android's call timeout.
const CALL_TIMEOUT: Duration = Duration::from_secs(4);

/// How this client reaches the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NetworkPath {
    Direct,
    /// Through a relay server (Tailscale DERP and the like).
    Relay,
    /// Through another peer of the overlay network.
    PeerRelay,
    /// A category this client does not know yet.
    Unknown,
}

fn invalid() -> HostApiError {
    HostApiError::Host(Error::Malformed("Invalid route response".into()))
}

impl NetworkPath {
    /// A reply body. A version other than 1 is [`HostApiError::UnsupportedVersion`].
    pub fn parse(json: &str) -> Result<Self, HostApiError> {
        if json.len() > MAX_NETWORK_PATH_BYTES {
            return Err(invalid());
        }
        let root: Value = serde_json::from_str(json).map_err(|_| invalid())?;
        let root = root.as_object().ok_or_else(invalid)?;
        // Any number equal to 1, as org.json compares it; "1" as a string is not a version.
        let version = root
            .get("version")
            .and_then(Value::as_f64)
            .ok_or_else(invalid)?;
        if version != 1.0 {
            return Err(HostApiError::UnsupportedVersion);
        }
        // org.json reads any present value as a string: only the known names map to a route.
        Ok(match root.get("kind").ok_or_else(invalid)?.as_str() {
            Some("direct") => NetworkPath::Direct,
            Some("relay") => NetworkPath::Relay,
            Some("peer_relay") => NetworkPath::PeerRelay,
            _ => NetworkPath::Unknown,
        })
    }
}

impl HostClient {
    /// The route category to the host. `capabilities` comes from a paired serverinfo; without
    /// `Feature::NetworkPath` nothing is sent and the answer is [`HostApiError::Unavailable`].
    pub fn network_path(
        &self,
        capabilities: &HostCapabilities,
    ) -> Result<NetworkPath, HostApiError> {
        HostApiError::require(capabilities, Feature::NetworkPath)?;
        let response = self.pinned_get(
            "api/network-path",
            MAX_NETWORK_PATH_BYTES,
            CALL_TIMEOUT,
            true,
        )?;
        NetworkPath::parse(std::str::from_utf8(&response.body).map_err(|_| invalid())?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::serverinfo::tests::RUBYLIGHT;
    use crate::http::{Availability, ServerInfo};

    #[test]
    fn accepts_only_known_categories_and_version() {
        // The cases of Android's NetworkPathInfoTest.
        for (json, path) in [
            (r#"{"version":1,"kind":"direct"}"#, NetworkPath::Direct),
            (r#"{"version":1,"kind":"relay"}"#, NetworkPath::Relay),
            (
                r#"{"version":1,"kind":"peer_relay"}"#,
                NetworkPath::PeerRelay,
            ),
            (r#"{"version":1,"kind":"future"}"#, NetworkPath::Unknown),
            (r#"{"version":1.0,"kind":"direct"}"#, NetworkPath::Direct),
            (r#"{"version":1,"kind":5}"#, NetworkPath::Unknown),
        ] {
            assert_eq!(NetworkPath::parse(json).unwrap(), path, "{json}");
        }
        for json in [
            "{}".to_owned(),
            "[]".to_owned(),
            "<html/>".to_owned(),
            r#"{"version":"1","kind":"direct"}"#.to_owned(),
            r#"{"version":1}"#.to_owned(),
            "\0".repeat(513),
            format!(
                r#"{{"version":1,"kind":"direct","pad":"{}"}}"#,
                " ".repeat(512)
            ),
        ] {
            assert!(
                matches!(
                    NetworkPath::parse(&json),
                    Err(HostApiError::Host(Error::Malformed(_)))
                ),
                "{json}"
            );
        }
        assert!(matches!(
            NetworkPath::parse(r#"{"version":2,"kind":"direct"}"#),
            Err(HostApiError::UnsupportedVersion)
        ));
    }

    #[test]
    fn a_host_without_the_feature_is_never_asked() {
        let identity = std::sync::Arc::new(crate::http::identity::tests::fixture());
        let client = HostClient::new("127.0.0.1", 1, identity)
            .unwrap()
            .with_https_port(1);
        client.pin(Some(client.identity().certificate_der().to_vec()));
        let without = ServerInfo::parse(
            &RUBYLIGHT.replace("<NetworkPathVersion>1</NetworkPathVersion>", ""),
            47989,
        )
        .unwrap()
        .capabilities;
        assert!(matches!(
            client.network_path(&without),
            Err(HostApiError::Unavailable {
                feature: Feature::NetworkPath,
                availability: Availability::Unsupported(_),
            })
        ));
        // With the feature the request goes out (and port 1 refuses it).
        let with = ServerInfo::parse(RUBYLIGHT, 47989).unwrap().capabilities;
        assert!(matches!(
            client.network_path(&with),
            Err(HostApiError::Host(Error::Io(_)))
        ));
    }
}
