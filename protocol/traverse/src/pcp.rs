//! PCP (RFC 6887) MAP requests: IPv4 port mappings on a router that speaks PCP, and IPv6
//! firewall pinholes (a MAP for an IPv6 address opens the router's firewall for it). Requests go
//! to the default gateway's UDP port 5351, the same as NAT-PMP's; a NAT-PMP-only router answers
//! a PCP request with version 0, which [`parse_map`] reports as [`Error::Version`].

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub const PORT: u16 = 5351;
pub const VERSION: u8 = 2;
const OPCODE_MAP: u8 = 1;
pub const REQUEST_BYTES: usize = 60;
pub const TCP: u8 = 6;
pub const UDP: u8 = 17;

/// An address in PCP's 16-byte form: IPv4 as an IPv4-mapped IPv6 address.
fn wire(address: IpAddr) -> [u8; 16] {
    match address {
        IpAddr::V4(ip) => ip.to_ipv6_mapped().octets(),
        IpAddr::V6(ip) => ip.octets(),
    }
}

fn from_wire(octets: [u8; 16]) -> IpAddr {
    let ip = Ipv6Addr::from(octets);
    match ip.to_ipv4_mapped() {
        Some(v4) => IpAddr::V4(v4),
        None => IpAddr::V6(ip),
    }
}

/// A MAP request. `client` is this PC's address as the router sees it; `suggested` the external
/// address wanted (None: any, of `client`'s family). Lifetime 0 deletes the mapping.
pub fn map_request(
    client: IpAddr,
    lifetime: u32,
    nonce: &[u8; 12],
    protocol: u8,
    internal_port: u16,
    suggested_port: u16,
    suggested: Option<IpAddr>,
) -> [u8; REQUEST_BYTES] {
    let mut request = [0u8; REQUEST_BYTES];
    request[0] = VERSION;
    request[1] = OPCODE_MAP;
    request[4..8].copy_from_slice(&lifetime.to_be_bytes());
    request[8..24].copy_from_slice(&wire(client));
    request[24..36].copy_from_slice(nonce);
    request[36] = protocol;
    request[40..42].copy_from_slice(&internal_port.to_be_bytes());
    request[42..44].copy_from_slice(&suggested_port.to_be_bytes());
    let suggested = suggested.unwrap_or(match client {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
    });
    request[44..60].copy_from_slice(&wire(suggested));
    request
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapResponse {
    /// 0 is success; others name the refusal ([`describe`]).
    pub result: u8,
    pub lifetime: u32,
    pub epoch: u32,
    pub nonce: [u8; 12],
    pub protocol: u8,
    pub internal_port: u16,
    pub external_port: u16,
    pub external_address: IpAddr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Malformed,
    /// Another version: 0 is a NAT-PMP router.
    Version(u8),
}

pub fn parse_map(datagram: &[u8]) -> Result<MapResponse, Error> {
    if datagram.len() < 4 {
        return Err(Error::Malformed);
    }
    if datagram[0] != VERSION {
        return Err(Error::Version(datagram[0]));
    }
    if datagram[1] != 0x80 | OPCODE_MAP {
        return Err(Error::Malformed);
    }
    let result = datagram[3];
    // A refusal may come back short, without the opcode's fields (RFC 6887 7.2).
    if datagram.len() < REQUEST_BYTES {
        if result != 0 && datagram.len() >= 24 {
            return Ok(MapResponse {
                result,
                lifetime: u32::from_be_bytes(datagram[4..8].try_into().unwrap()),
                epoch: u32::from_be_bytes(datagram[8..12].try_into().unwrap()),
                nonce: [0; 12],
                protocol: 0,
                internal_port: 0,
                external_port: 0,
                external_address: IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            });
        }
        return Err(Error::Malformed);
    }
    Ok(MapResponse {
        result,
        lifetime: u32::from_be_bytes(datagram[4..8].try_into().unwrap()),
        epoch: u32::from_be_bytes(datagram[8..12].try_into().unwrap()),
        nonce: datagram[24..36].try_into().unwrap(),
        protocol: datagram[36],
        internal_port: u16::from_be_bytes([datagram[40], datagram[41]]),
        external_port: u16::from_be_bytes([datagram[42], datagram[43]]),
        external_address: from_wire(datagram[44..60].try_into().unwrap()),
    })
}

/// Whether a PCP refusal means the router speaks NAT-PMP, or no PCP at all, rather than
/// refusing this mapping.
pub fn wants_natpmp(error: Result<&MapResponse, &Error>) -> bool {
    match error {
        Err(Error::Version(0)) => true,
        Ok(response) => response.result == 1,
        _ => false,
    }
}

/// A PCP result code in words, for the console.
pub fn describe(result: u8) -> &'static str {
    match result {
        0 => "mapped",
        1 => "the router does not support this PCP version",
        2 => "the router refused (PCP may be turned off on it)",
        3 | 6 => "the router could not read the request",
        4 | 5 => "the router does not support this request",
        7 => "the router has no internet connection",
        8 => "the router is out of mappings",
        9 => "the router does not support this protocol",
        10 => "the router's limit for this PC is reached",
        11 => "the router cannot give this port or address",
        12 => "the router sees this PC at another address (another NAT is on the way)",
        13 => "the router allows no more peers",
        _ => "the router refused the mapping",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ipv4_map_request_has_the_rfc_layout() {
        let request = map_request(
            "192.168.1.5".parse().unwrap(),
            7200,
            &[7; 12],
            TCP,
            47984,
            47984,
            None,
        );
        assert_eq!(request.len(), 60);
        assert_eq!(&request[..8], &[2, 1, 0, 0, 0, 0, 0x1c, 0x20]);
        assert_eq!(
            &request[8..24],
            &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 192, 168, 1, 5]
        );
        assert_eq!(&request[24..36], &[7; 12]);
        assert_eq!(request[36], 6);
        assert_eq!(&request[40..44], &[0xbb, 0x70, 0xbb, 0x70]);
        // No preference: the IPv4-mapped unspecified address.
        assert_eq!(
            &request[44..60],
            &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0, 0, 0, 0]
        );
    }

    #[test]
    fn an_ipv6_pinhole_names_the_pcs_own_address() {
        let ip: IpAddr = "2001:db8::5".parse().unwrap();
        let request = map_request(ip, 3600, &[1; 12], UDP, 47998, 47998, Some(ip));
        assert_eq!(request[36], 17);
        assert_eq!(from_wire(request[44..60].try_into().unwrap()), ip);
        assert_eq!(from_wire(request[8..24].try_into().unwrap()), ip);
    }

    fn response(result: u8, address: IpAddr) -> Vec<u8> {
        let mut out = vec![2, 0x81, 0, result, 0, 0, 0x0e, 0x10, 0, 0, 0, 5];
        out.extend_from_slice(&[0; 12]);
        out.extend_from_slice(&[7; 12]);
        out.extend_from_slice(&[6, 0, 0, 0, 0xbb, 0x70, 0xc3, 0x50]);
        out.extend_from_slice(&wire(address));
        out
    }

    #[test]
    fn a_map_response_parses() {
        let parsed = parse_map(&response(0, "203.0.113.9".parse().unwrap())).unwrap();
        assert_eq!(parsed.result, 0);
        assert_eq!(parsed.lifetime, 3600);
        assert_eq!(parsed.epoch, 5);
        assert_eq!(parsed.nonce, [7; 12]);
        assert_eq!(parsed.protocol, TCP);
        assert_eq!(parsed.internal_port, 47984);
        assert_eq!(parsed.external_port, 50000);
        assert_eq!(
            parsed.external_address,
            "203.0.113.9".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn refusals_and_natpmp_routers_are_told_apart() {
        let refused = parse_map(&response(2, "0.0.0.0".parse().unwrap())).unwrap();
        assert_eq!(refused.result, 2);
        assert!(!wants_natpmp(Ok(&refused)));
        let short = parse_map(&[
            2, 0x81, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ])
        .unwrap();
        assert!(wants_natpmp(Ok(&short)));
        let natpmp = parse_map(&[0, 129, 0, 1, 0, 0, 0, 0]);
        assert_eq!(natpmp, Err(Error::Version(0)));
        assert!(wants_natpmp(natpmp.as_ref()));
        assert_eq!(parse_map(&[2, 0x81, 0]), Err(Error::Malformed));
        assert_eq!(parse_map(&[2, 0x01, 0, 0]), Err(Error::Malformed));
    }
}
