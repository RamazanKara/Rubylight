//! The host's own view of tunnelled connections: every socket the host side of a tunnel opens
//! towards the host's ports is registered here, by the address the host's servers see as the
//! peer, before it sends anything. The host asks [`is_tunnelled`] wherever it would trust a
//! loopback or local peer, so a device on the internet never gets what a program on this PC
//! gets, whatever address the tunnel had to connect from.
//!
//! Addresses in 127.77.0.0/16 (the host side's per-device sources) and 127.78.0.0/16 (the device
//! side's loopback ends) count as tunnelled even without a registration: nothing else uses them.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicU64, Ordering},
        LazyLock, Mutex,
    },
};

struct Entry {
    id: u64,
    device: String,
}

static PEERS: LazyLock<Mutex<HashMap<SocketAddr, Entry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT: AtomicU64 = AtomicU64::new(1);

fn canonical(address: SocketAddr) -> SocketAddr {
    SocketAddr::new(address.ip().to_canonical(), address.port())
}

/// A registration; dropping it removes it.
#[derive(Debug)]
pub struct Mark {
    address: SocketAddr,
    id: u64,
}

impl Mark {
    pub fn address(&self) -> SocketAddr {
        self.address
    }
}

impl Drop for Mark {
    fn drop(&mut self) {
        let mut peers = PEERS.lock().unwrap();
        // Only this mark's own entry: the port may have been registered again since.
        if peers
            .get(&self.address)
            .is_some_and(|entry| entry.id == self.id)
        {
            peers.remove(&self.address);
        }
    }
}

/// Registers `address`, the peer address the host's servers will see, for `device`.
pub fn register(address: SocketAddr, device: &str) -> Mark {
    let address = canonical(address);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    PEERS.lock().unwrap().insert(
        address,
        Entry {
            id,
            device: device.to_owned(),
        },
    );
    Mark { address, id }
}

/// Whether an address is in the ranges only tunnels use (127.77.0.0/16, 127.78.0.0/16).
pub fn tunnel_range(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(ip) => {
            let [a, b, ..] = ip.octets();
            a == 127 && (b == 77 || b == 78)
        }
        IpAddr::V6(_) => false,
    }
}

/// Whether a connection or datagram from `peer` came through an internet play tunnel.
pub fn is_tunnelled(peer: SocketAddr) -> bool {
    let peer = canonical(peer);
    tunnel_range(peer.ip()) || PEERS.lock().unwrap().contains_key(&peer)
}

/// The device a registered peer belongs to.
pub fn device(peer: SocketAddr) -> Option<String> {
    PEERS
        .lock()
        .unwrap()
        .get(&canonical(peer))
        .map(|entry| entry.device.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_registered_peer_is_tunnelled_until_its_mark_goes() {
        let address: SocketAddr = "192.0.2.10:50001".parse().unwrap();
        assert!(!is_tunnelled(address));
        let mark = register(address, "phone");
        assert!(is_tunnelled(address));
        assert!(is_tunnelled("[::ffff:192.0.2.10]:50001".parse().unwrap()));
        assert_eq!(device(address).as_deref(), Some("phone"));
        // Another port of the same address is not.
        assert!(!is_tunnelled("192.0.2.10:50002".parse().unwrap()));
        // A newer registration of the port outlives the older mark.
        let newer = register(address, "tablet");
        drop(mark);
        assert_eq!(device(address).as_deref(), Some("tablet"));
        drop(newer);
        assert!(!is_tunnelled(address));
    }

    #[test]
    fn the_tunnel_ranges_count_without_a_registration() {
        assert!(is_tunnelled("127.77.1.2:1".parse().unwrap()));
        assert!(is_tunnelled("127.78.200.9:1".parse().unwrap()));
        assert!(!is_tunnelled("127.0.0.1:1".parse().unwrap()));
        assert!(!is_tunnelled("127.79.0.1:1".parse().unwrap()));
        assert!(!is_tunnelled("[::1]:1".parse().unwrap()));
    }
}
