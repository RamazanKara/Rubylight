//! UDP discovery for WebRTC TV clients: a `discover` broadcast on port 8000
//! gets a `discovery` reply with the host's name, the signalling port and the
//! Wake-on-LAN address. Only peers on the local network are answered.

use crate::{
    network::{Reach, reach},
    state::Shared,
};
use anyhow::{Context, Result};
use butterpollo_core::webrtc::protocol::{ServerMessage, is_discovery_request};
use socket2::{Domain, Socket, Type};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr, UdpSocket},
    time::{Duration, Instant},
};

/// The port every TV sends its broadcast to.
pub const PORT: u16 = 8000;

/// A datagram longer than this is not a `discover` message.
const MAX_DATAGRAM: usize = 512;

/// Whether `datagram` from `peer` is a `discover` message from the local
/// network, which gets a reply.
fn is_request(peer: IpAddr, datagram: &[u8]) -> bool {
    reach(peer) <= Reach::Lan
        && datagram.len() <= MAX_DATAGRAM
        && std::str::from_utf8(datagram).is_ok_and(is_discovery_request)
}

/// The reply to a datagram, if the peer is on the local network and the
/// datagram is a `discover` message.
#[cfg(test)]
pub fn reply(
    peer: IpAddr,
    datagram: &[u8],
    name: &str,
    port: u16,
    mac_address: impl FnOnce() -> Option<String>,
) -> Option<String> {
    is_request(peer, datagram)
        .then(|| ServerMessage::discovery(name, port, mac_address()).to_json())
}

/// The reply to a `discover` message (see [`is_request`]), with the name and Wake-on-LAN address taken from `answers`. The address is
/// cached by the local address that routes to the peer (`local`), never by
/// the sender, which anyone on the network can forge.
fn cached_reply(
    port: u16,
    answers: &mut Answers,
    now: Instant,
    name: impl FnOnce() -> String,
    local: impl FnOnce() -> Option<IpAddr>,
    mac: impl FnOnce(IpAddr) -> Option<String>,
) -> String {
    let name = answers.name(now, name);
    let mac = local().and_then(|local| answers.mac(now, local, || mac(local)));
    ServerMessage::discovery(&name, port, mac).to_json()
}

/// The UDP socket on `address`, shared with any other listener on the port
/// (`SO_REUSEADDR`), so a second program that wants the TV's broadcasts still
/// gets them.
pub fn bind(address: SocketAddr) -> Result<UdpSocket> {
    let socket = Socket::new(Domain::for_address(address), Type::DGRAM, None)?;
    if address.is_ipv6() {
        socket.set_only_v6(false)?;
    }
    socket.set_reuse_address(true)?;
    socket
        .bind(&address.into())
        .with_context(|| format!("UDP {address}"))?;
    socket.set_nonblocking(true)?;
    Ok(socket.into())
}

/// The local address of the interface that reaches `peer`.
fn local_address_towards(peer: IpAddr) -> Option<IpAddr> {
    let probe = UdpSocket::bind(if peer.is_ipv4() {
        SocketAddr::from(([0, 0, 0, 0], 0))
    } else {
        SocketAddr::from(([0u16; 8], 0))
    })
    .ok()?;
    // A UDP connect sends nothing; it only picks the route.
    probe.connect((peer, 9)).ok()?;
    Some(probe.local_addr().ok()?.ip())
}

/// The Wake-on-LAN address of the interface that owns `local`.
pub fn mac_address(local: IpAddr) -> Option<String> {
    let mac = butterpollo_windows::net::local_mac(local).ok()?;
    (mac != "00:00:00:00:00:00").then_some(mac)
}

/// How long a looked-up host name or Wake-on-LAN address is reused. A
/// datagram flood must not cost a configuration read and an adapter lookup each.
const CACHE_FOR: Duration = Duration::from_secs(5);
/// Peers whose route is remembered at once.
const CACHE_PEERS: usize = 256;

/// The host name and the Wake-on-LAN address of each local address answered
/// lately.
#[derive(Default)]
struct Answers {
    name: Option<(Instant, String)>,
    macs: HashMap<IpAddr, (Instant, Option<String>)>,
}

impl Answers {
    fn name(&mut self, now: Instant, lookup: impl FnOnce() -> String) -> String {
        match &self.name {
            Some((at, name)) if now.duration_since(*at) < CACHE_FOR => name.clone(),
            _ => {
                let name = lookup();
                self.name = Some((now, name.clone()));
                name
            }
        }
    }

    fn mac(
        &mut self,
        now: Instant,
        local: IpAddr,
        lookup: impl FnOnce() -> Option<String>,
    ) -> Option<String> {
        if let Some((at, mac)) = self.macs.get(&local)
            && now.duration_since(*at) < CACHE_FOR
        {
            return mac.clone();
        }
        self.macs
            .retain(|_, (at, _)| now.duration_since(*at) < CACHE_FOR);
        let mac = lookup();
        if self.macs.len() < CACHE_PEERS {
            self.macs.insert(local, (now, mac.clone()));
        }
        mac
    }
}

pub async fn serve(h: Shared, socket: UdpSocket, port: u16) {
    let socket = match tokio::net::UdpSocket::from_std(socket) {
        Ok(socket) => socket,
        Err(error) => {
            tracing::error!(%error, "WebRTC discovery could not start");
            return;
        }
    };
    let mut buffer = [0u8; MAX_DATAGRAM + 1];
    let mut answers = Answers::default();
    loop {
        let (length, peer) = match socket.recv_from(&mut buffer).await {
            Ok(received) => received,
            Err(error) => {
                // A reply to an unreachable peer comes back as a reset on Windows.
                tracing::debug!(%error, "WebRTC discovery read failed");
                continue;
            }
        };
        let ip = peer.ip().to_canonical();
        // Only a datagram that gets an answer costs a lookup, and only once in a while.
        let reply = is_request(ip, &buffer[..length]).then(|| {
            cached_reply(
                port,
                &mut answers,
                Instant::now(),
                || crate::network::host_name(&h.config.read().unwrap()),
                || local_address_towards(ip),
                mac_address,
            )
        });
        match reply {
            Some(reply) => {
                if let Err(error) = socket.send_to(reply.as_bytes(), peer).await {
                    tracing::debug!(%peer, %error, "WebRTC discovery reply failed");
                }
            }
            None if reach(ip) > Reach::Lan => {
                tracing::debug!(%peer, "WebRTC discovery ignored a peer outside the local network");
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DISCOVER: &[u8] = br#"{"version":2,"type":"discover"}"#;

    fn answer(peer: &str, datagram: &[u8]) -> Option<ServerMessage> {
        reply(peer.parse().unwrap(), datagram, "Den PC", 8000, || {
            Some("2C:F0:5D:7B:E6:D0".into())
        })
        .map(|text| ServerMessage::from_json(&text).unwrap())
    }

    #[test]
    fn local_network_peers_get_the_name_port_and_mac() {
        for peer in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.9",
            "192.168.4.20",
            "169.254.3.4",
            "100.64.0.1",
            "100.127.255.254",
            "fd00::1",
            "fe80::1",
            "::1",
            "::ffff:192.168.1.5",
        ] {
            assert_eq!(
                answer(peer, DISCOVER),
                Some(ServerMessage::discovery(
                    "Den PC",
                    8000,
                    Some("2C:F0:5D:7B:E6:D0".into())
                )),
                "{peer}"
            );
        }
    }

    #[test]
    fn public_peers_get_no_reply() {
        for peer in [
            "8.8.8.8",
            "172.32.0.1",
            "100.128.0.1",
            "100.63.255.255",
            "192.169.0.1",
            "2001:db8::1",
            "::ffff:8.8.8.8",
        ] {
            assert_eq!(answer(peer, DISCOVER), None, "{peer}");
        }
    }

    #[test]
    fn only_a_version_2_discover_is_answered() {
        for datagram in [
            &br#"{"version":1,"type":"discover"}"#[..],
            br#"{"type":"discover"}"#,
            br#"{"version":2,"type":"authenticate"}"#,
            b"discover",
            b"\xff\xfe",
            b"",
        ] {
            assert_eq!(answer("192.168.1.2", datagram), None);
        }
        let long = format!(
            r#"{{"version":2,"type":"discover","pad":"{}"}}"#,
            "x".repeat(600)
        );
        assert_eq!(answer("192.168.1.2", long.as_bytes()), None);
    }

    #[test]
    fn the_mac_address_is_left_out_when_there_is_none() {
        let text = reply("192.168.1.2".parse().unwrap(), DISCOVER, "PC", 8000, || {
            None
        })
        .unwrap();
        assert!(!text.contains("macAddress"));
    }

    /// Counts of (name, local address, MAC) lookups.
    type Counts = std::cell::Cell<(u32, u32, u32)>;

    /// The reply to `datagram` from `peer`, which reaches this host at
    /// `local`, at `at` after `start`.
    fn ask(
        answers: &mut Answers,
        counts: &Counts,
        start: Instant,
        peer: &str,
        local: &str,
        datagram: &[u8],
        at: Duration,
    ) -> Option<String> {
        let peer = peer.parse().unwrap();
        is_request(peer, datagram).then(|| {
            cached_reply(
                8000,
                answers,
                start + at,
                || {
                    counts.set((counts.get().0 + 1, counts.get().1, counts.get().2));
                    "Den PC".into()
                },
                || {
                    counts.set((counts.get().0, counts.get().1 + 1, counts.get().2));
                    local.parse().ok()
                },
                |_| {
                    counts.set((counts.get().0, counts.get().1, counts.get().2 + 1));
                    Some("2C:F0:5D:7B:E6:D0".into())
                },
            )
        })
    }

    #[test]
    fn the_name_and_mac_are_looked_up_once_in_five_seconds() {
        let start = Instant::now();
        let mut answers = Answers::default();
        let counts = Counts::default();
        let mut ask = |peer: &str, local: &str, datagram: &[u8], at: Duration| {
            ask(&mut answers, &counts, start, peer, local, datagram, at)
        };
        for n in 0..100 {
            let at = Duration::from_millis(n);
            assert!(ask("192.168.1.2", "192.168.1.10", DISCOVER, at).is_some());
        }
        assert_eq!(counts.get().0, 1);
        assert_eq!(counts.get().2, 1);
        // Another route has its own address, but the same name.
        let second = Duration::from_secs(1);
        assert!(ask("192.168.2.2", "192.168.2.10", DISCOVER, second).is_some());
        assert_eq!((counts.get().0, counts.get().2), (1, 2));
        // Datagrams that get no answer cost nothing.
        let before = counts.get();
        assert!(ask("8.8.8.8", "192.168.1.10", DISCOVER, second).is_none());
        assert!(ask("192.168.1.2", "192.168.1.10", b"hello", second).is_none());
        assert_eq!(counts.get(), before);
        // After five seconds both are looked up again.
        let later = Duration::from_secs(6);
        assert!(ask("192.168.1.2", "192.168.1.10", DISCOVER, later).is_some());
        assert_eq!((counts.get().0, counts.get().2), (2, 3));
    }

    #[test]
    fn forged_sender_addresses_on_one_route_share_one_lookup() {
        let start = Instant::now();
        let mut answers = Answers::default();
        let counts = Counts::default();
        for n in 0..1000u32 {
            let peer = format!("10.{}.{}.{}", n % 4, (n / 4) % 250, n % 200 + 1);
            let reply = ask(
                &mut answers,
                &counts,
                start,
                &peer,
                "10.0.0.5",
                DISCOVER,
                Duration::ZERO,
            );
            assert!(reply.is_some(), "{peer}");
        }
        // The route is found for each datagram, but the adapter is asked once
        // and the cache holds one entry, whoever the sender claims to be.
        assert_eq!(
            (counts.get().0, counts.get().1, counts.get().2),
            (1, 1000, 1)
        );
        assert_eq!(answers.macs.len(), 1);
    }

    #[test]
    fn a_peer_with_no_route_gets_a_reply_without_a_mac() {
        let start = Instant::now();
        let mut answers = Answers::default();
        let counts = Counts::default();
        let text = ask(
            &mut answers,
            &counts,
            start,
            "192.168.1.2",
            "none",
            DISCOVER,
            Duration::ZERO,
        )
        .unwrap();
        assert!(!text.contains("macAddress"));
        assert_eq!(counts.get().2, 0);
        assert!(answers.macs.is_empty());
    }

    #[test]
    fn the_route_cache_stays_bounded() {
        let start = Instant::now();
        let mut answers = Answers::default();
        for n in 0..(CACHE_PEERS as u32 * 2) {
            let peer = IpAddr::from(std::net::Ipv4Addr::from(0xc0a8_0000 + n));
            answers.mac(start, peer, || None);
        }
        assert_eq!(answers.macs.len(), CACHE_PEERS);
    }

    #[tokio::test]
    async fn the_socket_answers_a_broadcast_and_shares_its_port() {
        let fixture = crate::state::test_support::Fixture::new();
        let h = fixture.host.clone();
        let first = bind(([127, 0, 0, 1], 0).into()).unwrap();
        let address = first.local_addr().unwrap();
        // A second listener on the same port is allowed.
        let second = bind(address).unwrap();
        drop(second);
        let task = tokio::spawn(serve(h, first, 4242));
        let client = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        client.send_to(DISCOVER, address).await.unwrap();
        let mut buffer = [0u8; 1024];
        let (length, _) = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.recv_from(&mut buffer),
        )
        .await
        .expect("a discovery reply")
        .unwrap();
        match ServerMessage::from_json(std::str::from_utf8(&buffer[..length]).unwrap()).unwrap() {
            ServerMessage::Discovery { port, name, .. } => {
                assert_eq!(port, 4242);
                assert!(!name.is_empty());
            }
            other => panic!("{other:?}"),
        }
        task.abort();
    }
}
