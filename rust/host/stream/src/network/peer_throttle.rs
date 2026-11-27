//! Peer rate throttling and IP subnet grouping for network listeners.
use super::{Reach, reach};
use std::{
    net::IpAddr,
    time::{Duration, Instant},
};

/// Keeps a repeating warning, such as refused peers of a scan, to one line in ten seconds.
pub struct Throttle {
    last: Option<Instant>,
    skipped: u32,
}

impl Default for Throttle {
    fn default() -> Self {
        Self::new()
    }
}

impl Throttle {
    pub const fn new() -> Self {
        Self {
            last: None,
            skipped: 0,
        }
    }

    /// Whether to log now, at most once in ten seconds; with how many were skipped since the last
    /// one logged.
    pub fn allow(&mut self) -> Option<u32> {
        let now = Instant::now();
        if self
            .last
            .is_some_and(|at| now.duration_since(at) < Duration::from_secs(10))
        {
            self.skipped += 1;
            return None;
        }
        self.last = Some(now);
        Some(std::mem::take(&mut self.skipped))
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn rewind_for_test(&mut self, ago: Duration) {
        self.last = Some(Instant::now() - ago);
    }
}

/// Loopback, private, link-local, ULA and carrier-grade NAT addresses; nothing the internet can reach.
pub fn local_network(peer: IpAddr) -> bool {
    reach(peer.to_canonical()) <= Reach::Lan
}

/// What counts as one peer: an IPv4 address, or an IPv6 /64. One PC rotates
/// through many privacy addresses of its /64.
pub fn peer_group(peer: IpAddr) -> IpAddr {
    match peer.to_canonical() {
        IpAddr::V6(v6) => IpAddr::V6(std::net::Ipv6Addr::from(u128::from(v6) & (u128::MAX << 64))),
        v4 => v4,
    }
}

#[cfg(test)]
mod port_state_tests {
    use super::super::{Mapping, Protocol, mapped_ports, port_states};

    #[test]
    fn a_port_another_mapping_forwards_here_is_reported_mapped_at_its_own_number() {
        let forwarded = [Mapping {
            protocol: Protocol::TCP,
            port: 47991,
        }];
        let (ok, failed) = port_states(&forwarded, &mapped_ports(&[], &forwarded), &[]);
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].port, 47991);
        assert_eq!(ok[0].external, Some(47991));
        assert!(ok[0].reason.is_none());
        assert!(failed.is_empty());
    }

    #[test]
    fn the_console_sees_every_wanted_port_mapped_or_refused_with_a_reason() {
        let wanted = [
            Mapping {
                protocol: Protocol::TCP,
                port: 47984,
            },
            Mapping {
                protocol: Protocol::UDP,
                port: 47998,
            },
            Mapping {
                protocol: Protocol::UDP,
                port: 48000,
            },
        ];
        let (ok, failed) = port_states(
            &wanted,
            &[(wanted[0], 50000)],
            &[(wanted[1], "the router is out of mappings".into())],
        );
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].external, Some(50000));
        assert_eq!(ok[0].protocol, "TCP");
        assert_eq!(failed.len(), 2);
        assert_eq!(
            failed[0].reason.as_deref(),
            Some("the router is out of mappings")
        );
        assert_eq!(
            failed[1].reason.as_deref(),
            Some("the router refused the mapping")
        );
    }

    #[test]
    fn peer_group_unmaps_ipv4_mapped_ipv6_addresses() {
        use super::peer_group;
        use std::net::IpAddr;

        let v4: IpAddr = "1.2.3.4".parse().unwrap();
        let mapped: IpAddr = "::ffff:1.2.3.4".parse().unwrap();
        let other_mapped: IpAddr = "::ffff:5.6.7.8".parse().unwrap();
        assert_eq!(peer_group(mapped), v4);
        assert_ne!(peer_group(mapped), peer_group(other_mapped));
        assert_eq!(
            peer_group("2001:db8:1:2::a".parse().unwrap()),
            peer_group("2001:db8:1:2::b".parse().unwrap())
        );
        assert_ne!(
            peer_group("2001:db8:1:2::a".parse().unwrap()),
            peer_group("2001:db8:1:3::a".parse().unwrap())
        );
    }
}
