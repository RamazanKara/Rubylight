//! PCP (RFC 6887) and NAT-PMP (RFC 6886) port mappings, for routers that don't speak UPnP IGD,
//! and PCP pinholes in an IPv6 router's firewall. The formats are `rubylight_traverse`'s; this
//! is the exchange with the router and the leases.
use super::{Mapping, Protocol};
use anyhow::{Context, Result, bail};
use rubylight_traverse::{natpmp, pcp};
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6},
    time::{Duration, Instant},
};
use tokio::net::UdpSocket;

/// The lifetime asked for every mapping, in seconds; leases renew at half of what the router
/// grants.
const LIFETIME: u32 = 7200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Method {
    Pcp,
    NatPmp,
}
impl Method {
    pub(super) fn name(self) -> &'static str {
        match self {
            Method::Pcp => "PCP",
            Method::NatPmp => "NAT-PMP",
        }
    }
}

struct Lease {
    mapping: Mapping,
    external: u16,
    renew_at: Instant,
}

/// The mappings asked of one router at one of this PC's addresses.
pub(super) struct PortMapper {
    socket: UdpSocket,
    local: IpAddr,
    method: Option<Method>,
    nonce: [u8; 12],
    leases: Vec<Lease>,
    pub(super) external: Option<IpAddr>,
}

/// What a renewal did: each wanted port mapped (with its external port) or refused, and why.
#[derive(Debug, Default)]
pub(super) struct Outcome {
    pub(super) mapped: Vec<(Mapping, u16)>,
    pub(super) failed: Vec<(Mapping, String)>,
}

fn protocol_number(protocol: Protocol) -> u8 {
    if protocol == Protocol::TCP {
        pcp::TCP
    } else {
        pcp::UDP
    }
}

impl PortMapper {
    /// A mapper for the IPv4 router `gateway`, from this PC's `local` address.
    pub(super) async fn ipv4(gateway: Ipv4Addr, local: Ipv4Addr) -> Result<Self> {
        let socket = UdpSocket::bind((local, 0)).await?;
        socket.connect((gateway, pcp::PORT)).await?;
        Ok(Self::new(socket, IpAddr::V4(local)))
    }

    /// A PCP mapper for pinholes in the IPv6 router `gateway`'s firewall, for this PC's global
    /// address `local` on interface `index` (a link-local router needs it).
    pub(super) async fn ipv6(gateway: Ipv6Addr, local: Ipv6Addr, index: u32) -> Result<Self> {
        let socket = UdpSocket::bind(SocketAddrV6::new(local, 0, 0, index)).await?;
        let scope = if gateway.is_unicast_link_local() {
            index
        } else {
            0
        };
        socket
            .connect(SocketAddrV6::new(gateway, pcp::PORT, 0, scope))
            .await?;
        let mut mapper = Self::new(socket, IpAddr::V6(local));
        // NAT-PMP knows no IPv6.
        mapper.method = Some(Method::Pcp);
        Ok(mapper)
    }

    fn new(socket: UdpSocket, local: IpAddr) -> Self {
        Self {
            socket,
            local,
            method: None,
            nonce: rubylight_traverse::crypto::random(),
            leases: Vec::new(),
            external: None,
        }
    }

    pub(super) fn method(&self) -> Option<Method> {
        self.method
    }

    /// Sends `request` until a datagram `matches` comes back: RFC 6886's schedule from 250 ms,
    /// doubling, four tries.
    async fn exchange(&self, request: &[u8], matches: impl Fn(&[u8]) -> bool) -> Option<Vec<u8>> {
        let mut wait = Duration::from_millis(250);
        let mut buffer = [0u8; 1100];
        for _ in 0..4 {
            if self.socket.send(request).await.is_err() {
                return None;
            }
            let deadline = tokio::time::Instant::now() + wait;
            while let Ok(Ok(length)) =
                tokio::time::timeout_at(deadline, self.socket.recv(&mut buffer)).await
            {
                if matches(&buffer[..length]) {
                    return Some(buffer[..length].to_vec());
                }
            }
            wait *= 2;
        }
        None
    }

    async fn pcp(
        &mut self,
        mapping: Mapping,
        lifetime: u32,
    ) -> Result<std::result::Result<pcp::MapResponse, pcp::Error>> {
        let protocol = protocol_number(mapping.protocol);
        let suggested = self.local.is_ipv6().then_some(self.local);
        let request = pcp::map_request(
            self.local,
            lifetime,
            &self.nonce,
            protocol,
            mapping.port,
            mapping.port,
            suggested,
        );
        let nonce = self.nonce;
        let reply = self
            .exchange(&request, |reply| match pcp::parse_map(reply) {
                Ok(response) => {
                    response.result != 0
                        || (response.nonce == nonce
                            && response.protocol == protocol
                            && response.internal_port == mapping.port)
                }
                // A NAT-PMP router's answer to a PCP request.
                Err(pcp::Error::Version(0)) => true,
                Err(_) => false,
            })
            .await
            .context("the router does not answer PCP or NAT-PMP")?;
        Ok(pcp::parse_map(&reply))
    }

    async fn natpmp(
        &mut self,
        mapping: Mapping,
        lifetime: u32,
    ) -> Result<std::result::Result<natpmp::Response, natpmp::Error>> {
        let protocol = if mapping.protocol == Protocol::TCP {
            natpmp::Protocol::Tcp
        } else {
            natpmp::Protocol::Udp
        };
        let external = if lifetime == 0 { 0 } else { mapping.port };
        let request = natpmp::map_request(protocol, mapping.port, external, lifetime);
        let reply = self
            .exchange(&request, |reply| match natpmp::parse(reply) {
                Ok(natpmp::Response::Mapping {
                    protocol: answered,
                    internal,
                    ..
                }) => answered == protocol && internal == mapping.port,
                Err(natpmp::Error::Refused(_)) => reply.get(1) == Some(&(0x80 | request[1])),
                _ => false,
            })
            .await
            .context("the router does not answer NAT-PMP")?;
        Ok(natpmp::parse(&reply))
    }

    async fn natpmp_address(&mut self) {
        let reply = self
            .exchange(&natpmp::address_request(), |reply| {
                matches!(natpmp::parse(reply), Ok(natpmp::Response::Address { .. }))
            })
            .await;
        if let Some(Ok(natpmp::Response::Address { address, .. })) =
            reply.as_deref().map(natpmp::parse)
        {
            self.external = Some(IpAddr::V4(address));
        }
    }

    /// Maps (or renews) each wanted port whose lease is due, and removes leases no longer wanted.
    pub(super) async fn renew(&mut self, wanted: &[Mapping]) -> Result<Outcome> {
        let now = Instant::now();
        let gone: Vec<Mapping> = self
            .leases
            .iter()
            .map(|lease| lease.mapping)
            .filter(|mapping| !wanted.contains(mapping))
            .collect();
        for mapping in gone {
            self.unmap(mapping).await;
        }
        let mut outcome = Outcome::default();
        for &mapping in wanted {
            if let Some(lease) = self
                .leases
                .iter()
                .find(|lease| lease.mapping == mapping && lease.renew_at > now)
            {
                outcome.mapped.push((mapping, lease.external));
                continue;
            }
            match self.map(mapping).await {
                Ok((external, lifetime)) => {
                    self.leases.retain(|lease| lease.mapping != mapping);
                    self.leases.push(Lease {
                        mapping,
                        external,
                        renew_at: now + Duration::from_secs(u64::from(lifetime.max(120)) / 2),
                    });
                    outcome.mapped.push((mapping, external));
                }
                Err(error) => {
                    // No answer at all is the router's, not this port's: stop asking.
                    if self.method.is_none()
                        || (outcome.mapped.is_empty()
                            && error.to_string().contains("does not answer"))
                    {
                        return Err(error);
                    }
                    outcome.failed.push((mapping, format!("{error:#}")));
                }
            }
        }
        if self.method == Some(Method::NatPmp) && self.external.is_none() {
            self.natpmp_address().await;
        }
        Ok(outcome)
    }

    /// One mapping: its external port and granted lifetime.
    async fn map(&mut self, mapping: Mapping) -> Result<(u16, u32)> {
        if self.method != Some(Method::NatPmp) {
            let response = self.pcp(mapping, LIFETIME).await?;
            if pcp::wants_natpmp(response.as_ref()) && self.local.is_ipv4() {
                self.method = Some(Method::NatPmp);
            } else {
                let response = response.map_err(|error| {
                    anyhow::anyhow!("the router's PCP answer is unreadable: {error:?}")
                })?;
                self.method = Some(Method::Pcp);
                if response.result != 0 {
                    bail!(
                        "{} (PCP {})",
                        pcp::describe(response.result),
                        response.result
                    );
                }
                if self.local.is_ipv4() {
                    self.external = Some(response.external_address);
                }
                return Ok((response.external_port, response.lifetime));
            }
        }
        match self.natpmp(mapping, LIFETIME).await? {
            Ok(natpmp::Response::Mapping {
                external, lifetime, ..
            }) => {
                self.method = Some(Method::NatPmp);
                Ok((external, lifetime))
            }
            Err(natpmp::Error::Refused(code)) => {
                self.method = Some(Method::NatPmp);
                bail!("{} (NAT-PMP {code})", natpmp::describe(code))
            }
            other => bail!("the router's NAT-PMP answer is unreadable: {other:?}"),
        }
    }

    async fn unmap(&mut self, mapping: Mapping) {
        let removed = match self.method {
            Some(Method::Pcp) => self.pcp(mapping, 0).await.is_ok(),
            Some(Method::NatPmp) => self.natpmp(mapping, 0).await.is_ok(),
            None => true,
        };
        if !removed {
            tracing::debug!(
                port = mapping.port,
                "a PCP or NAT-PMP mapping will expire with its lease"
            );
        }
        self.leases.retain(|lease| lease.mapping != mapping);
    }

    /// Removes every mapping this mapper made: the host stops.
    pub(super) async fn close(&mut self) {
        let mappings: Vec<Mapping> = self.leases.iter().map(|lease| lease.mapping).collect();
        for mapping in mappings {
            self.unmap(mapping).await;
        }
    }
}

/// The IPv4 default gateway and this PC's address towards it.
pub(super) fn ipv4_route(gateways: &[IpAddr], bind: IpAddr) -> Option<(Ipv4Addr, Ipv4Addr)> {
    let gateway = gateways.iter().find_map(|gateway| match gateway {
        IpAddr::V4(ip) => Some(*ip),
        IpAddr::V6(_) => None,
    })?;
    let local = match bind {
        IpAddr::V4(ip) if !ip.is_unspecified() => ip,
        _ => {
            let socket = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
            socket
                .connect(SocketAddr::new(IpAddr::V4(gateway), pcp::PORT))
                .ok()?;
            match socket.local_addr().ok()?.ip() {
                IpAddr::V4(ip) => ip,
                IpAddr::V6(_) => return None,
            }
        }
    };
    Some((gateway, local))
}

/// This PC's global IPv6 addresses with their interface index, for pinholes.
pub(super) fn ipv6_locals(bind: IpAddr) -> Vec<(Ipv6Addr, u32)> {
    let Ok(interfaces) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    interfaces
        .into_iter()
        .filter(|interface| interface.oper_status == if_addrs::IfOperStatus::Up)
        .filter_map(|interface| match (interface.ip(), interface.index) {
            (IpAddr::V6(ip), Some(index)) if ip.segments()[0] & 0xe000 == 0x2000 => {
                Some((ip, index))
            }
            _ => None,
        })
        .filter(|(ip, _)| bind.is_unspecified() || bind == IpAddr::V6(*ip))
        .take(4)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A NAT-PMP-only router on loopback: it refuses PCP with version 0, maps with NAT-PMP and
    /// reports its address.
    #[tokio::test]
    async fn a_natpmp_router_is_used_after_its_pcp_refusal() {
        let router = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = router.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buffer = [0u8; 128];
            loop {
                let Ok((length, from)) = router.recv_from(&mut buffer).await else {
                    return;
                };
                let request = &buffer[..length];
                let reply: Vec<u8> = match request[0] {
                    2 => vec![0, 0x81, 0, 1, 0, 0, 0, 0],
                    _ if request[1] == 0 => vec![0, 128, 0, 0, 0, 0, 0, 1, 203, 0, 113, 9],
                    _ => {
                        let mut reply = vec![0, 0x80 | request[1], 0, 0, 0, 0, 0, 1];
                        reply.extend_from_slice(&request[4..6]);
                        reply.extend_from_slice(&request[4..6]);
                        reply.extend_from_slice(&3600u32.to_be_bytes());
                        reply
                    }
                };
                router.send_to(&reply, from).await.unwrap();
            }
        });
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        socket.connect(address).await.unwrap();
        let mut mapper = PortMapper::new(socket, "127.0.0.1".parse().unwrap());
        let wanted = [
            Mapping {
                protocol: Protocol::TCP,
                port: 47984,
            },
            Mapping {
                protocol: Protocol::UDP,
                port: 47998,
            },
        ];
        let outcome = mapper.renew(&wanted).await.unwrap();
        assert_eq!(mapper.method(), Some(Method::NatPmp));
        assert_eq!(outcome.mapped, vec![(wanted[0], 47984), (wanted[1], 47998)]);
        assert!(outcome.failed.is_empty());
        assert_eq!(mapper.external, Some("203.0.113.9".parse().unwrap()));
        // A second renewal within the lease asks nothing.
        let again = mapper.renew(&wanted).await.unwrap();
        assert_eq!(again.mapped.len(), 2);
        mapper.close().await;
        assert!(mapper.leases.is_empty());
    }

    /// A PCP router that refuses one port: the other maps, the refusal says why.
    #[tokio::test]
    async fn a_pcp_refusal_names_its_reason() {
        let router = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = router.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buffer = [0u8; 128];
            loop {
                let Ok((length, from)) = router.recv_from(&mut buffer).await else {
                    return;
                };
                let request = buffer[..length].to_vec();
                let port = u16::from_be_bytes([request[40], request[41]]);
                let mut reply = vec![2, 0x81, 0, if port == 47998 { 8 } else { 0 }];
                reply.extend_from_slice(&7200u32.to_be_bytes());
                reply.extend_from_slice(&[0, 0, 0, 1]);
                reply.extend_from_slice(&[0; 12]);
                reply.extend_from_slice(&request[24..44]);
                reply.extend_from_slice(&Ipv4Addr::new(203, 0, 113, 9).to_ipv6_mapped().octets());
                router.send_to(&reply, from).await.unwrap();
            }
        });
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        socket.connect(address).await.unwrap();
        let mut mapper = PortMapper::new(socket, "127.0.0.1".parse().unwrap());
        let wanted = [
            Mapping {
                protocol: Protocol::TCP,
                port: 47984,
            },
            Mapping {
                protocol: Protocol::UDP,
                port: 47998,
            },
        ];
        let outcome = mapper.renew(&wanted).await.unwrap();
        assert_eq!(mapper.method(), Some(Method::Pcp));
        assert_eq!(outcome.mapped, vec![(wanted[0], 47984)]);
        assert_eq!(outcome.failed.len(), 1);
        assert!(
            outcome.failed[0].1.contains("out of mappings"),
            "{:?}",
            outcome.failed
        );
        assert_eq!(mapper.external, Some("203.0.113.9".parse().unwrap()));
    }
}
