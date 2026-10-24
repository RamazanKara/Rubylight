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
use std::net::{IpAddr, SocketAddr, UdpSocket};

/// The port every TV sends its broadcast to.
pub const PORT: u16 = 8000;

/// A datagram longer than this is not a `discover` message.
const MAX_DATAGRAM: usize = 512;

/// The reply to a datagram, if the peer is on the local network and the
/// datagram is a `discover` message.
pub fn reply(
    peer: IpAddr,
    datagram: &[u8],
    name: &str,
    port: u16,
    mac_address: impl FnOnce() -> Option<String>,
) -> Option<String> {
    if reach(peer) > Reach::Lan || datagram.len() > MAX_DATAGRAM {
        return None;
    }
    if !is_discovery_request(std::str::from_utf8(datagram).ok()?) {
        return None;
    }
    Some(ServerMessage::discovery(name, port, mac_address()).to_json())
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

/// The Wake-on-LAN address of the interface that reaches `peer`. None for a
/// loopback peer, which can't wake anything.
pub fn mac_address_towards(peer: IpAddr) -> Option<String> {
    let probe = UdpSocket::bind(if peer.is_ipv4() {
        SocketAddr::from(([0, 0, 0, 0], 0))
    } else {
        SocketAddr::from(([0u16; 8], 0))
    })
    .ok()?;
    // A UDP connect sends nothing; it only picks the route.
    probe.connect((peer, 9)).ok()?;
    mac_address(probe.local_addr().ok()?.ip())
}

/// The Wake-on-LAN address of the interface that owns `local`.
pub fn mac_address(local: IpAddr) -> Option<String> {
    let mac = butterpollo_windows::net::local_mac(local).ok()?;
    (mac != "00:00:00:00:00:00").then_some(mac)
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
        let name = crate::network::host_name(&h.config.read().unwrap());
        let reply = reply(ip, &buffer[..length], &name, port, || {
            mac_address_towards(ip)
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
