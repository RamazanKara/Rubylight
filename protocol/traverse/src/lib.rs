//! Internet play: reaching a paired host from outside its network without forwarded ports and
//! without a server of our own. `docs/internet-play.md` describes the design; this crate holds
//! what the Rubylight host and Nightjar share.
//!
//! The paths, in the order a client tries them ([`path`]): the local network, the host's IPv6
//! address (opened by an IGDv2 or PCP pinhole), an IPv4 port the host mapped on its router
//! (UPnP IGD, PCP or NAT-PMP; [`pcp`], [`natpmp`]), and last a UDP path both sides punch through
//! their NATs ([`punch`]). For that last one they meet without a server: each side publishes a
//! signed, encrypted record of its public endpoints ([`record`]) as a BEP 44 mutable item in the
//! BitTorrent mainline DHT ([`dht`], [`bencode`]), under keys derived from a secret the host gave
//! the device over its paired HTTPS connection. Only paired devices can read or write them.
//!
//! Over a punched path the host's TCP channels (HTTP, HTTPS, RTSP) run through a small reliable
//! stream layer ([`stream`]) and the media datagrams go straight through, every datagram sealed
//! with ChaCha20-Poly1305 ([`wire`]). The client sees the host at a loopback address with the
//! host's own port numbers ([`tunnel`]), so Moonlight's protocol runs on top unchanged.
//!
//! Everything that decides is sans-IO and tested without a network, including an in-process
//! NAT simulator (`sim`, tests only) with full-cone, restricted, port-restricted and symmetric
//! NATs. [`node`], [`host`] and [`client`] drive it over real sockets.

pub mod bencode;
pub mod client;
pub mod crypto;
pub mod dht;
pub mod host;
pub mod natpmp;
pub mod node;
pub mod path;
pub mod pcp;
pub mod peers;
pub mod punch;
pub mod record;
pub mod stream;
pub mod stun;
pub mod tunnel;
pub mod wire;

#[cfg(test)]
mod sim;

/// Milliseconds on a monotonic clock, as the sans-IO parts take time.
pub type Millis = u64;

/// The DHT nodes a lookup starts from when it knows no others: the well-known routers of the
/// BitTorrent mainline DHT. Documented in `docs/privacy.md` and `client/docs/PRIVACY.md`.
pub const DHT_BOOTSTRAP: &[&str] = &[
    "router.bittorrent.com:6881",
    "dht.transmissionbt.com:6881",
    "router.utorrent.com:6881",
    "dht.libtorrent.org:25401",
];

/// The public STUN servers asked for this device's public address, first to last. Two servers
/// tell an endpoint-independent NAT (the same public port for both) from a symmetric one.
/// Moonlight's own server comes first. Documented in `docs/privacy.md`.
pub const STUN_SERVERS: &[&str] = &["stun.moonlight-stream.org:3478", "stun.cloudflare.com:3478"];

/// Parses a comma or whitespace separated list of `host:port` entries, as the settings hold
/// them; an empty list means the defaults.
pub fn server_list(text: &str, defaults: &[&str]) -> Vec<String> {
    let listed: Vec<String> = text
        .split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|entry| !entry.is_empty() && entry.contains(':'))
        .map(str::to_owned)
        .collect();
    if listed.is_empty() {
        defaults.iter().map(|entry| (*entry).to_owned()).collect()
    } else {
        listed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_server_list_means_the_defaults() {
        assert_eq!(server_list("", STUN_SERVERS).len(), STUN_SERVERS.len());
        assert_eq!(server_list("  , ", STUN_SERVERS).len(), STUN_SERVERS.len());
        assert_eq!(
            server_list("stun.example.net:3478, other.example:19302", STUN_SERVERS),
            vec![
                "stun.example.net:3478".to_owned(),
                "other.example:19302".to_owned()
            ]
        );
        // An entry without a port is no server.
        assert_eq!(
            server_list("nonsense", STUN_SERVERS).len(),
            STUN_SERVERS.len()
        );
    }
}
