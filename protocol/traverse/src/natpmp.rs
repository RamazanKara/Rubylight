//! NAT-PMP (RFC 6886): the router's public IPv4 address and port mappings, for routers that
//! speak neither UPnP IGD nor PCP. Requests go to the default gateway's UDP port [`PORT`].

use std::net::Ipv4Addr;

pub const PORT: u16 = 5351;
/// The lifetime RFC 6886 recommends for a mapping, in seconds; the host renews at half of it.
pub const LIFETIME: u32 = 7200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    Udp,
    Tcp,
}

impl Protocol {
    fn opcode(self) -> u8 {
        match self {
            Protocol::Udp => 1,
            Protocol::Tcp => 2,
        }
    }
}

/// The request for the router's public address.
pub fn address_request() -> [u8; 2] {
    [0, 0]
}

/// A mapping request; lifetime 0 with external port 0 deletes the mapping.
pub fn map_request(protocol: Protocol, internal: u16, external: u16, lifetime: u32) -> [u8; 12] {
    let mut request = [0u8; 12];
    request[1] = protocol.opcode();
    request[4..6].copy_from_slice(&internal.to_be_bytes());
    request[6..8].copy_from_slice(&external.to_be_bytes());
    request[8..12].copy_from_slice(&lifetime.to_be_bytes());
    request
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Response {
    Address {
        epoch: u32,
        address: Ipv4Addr,
    },
    Mapping {
        protocol: Protocol,
        epoch: u32,
        internal: u16,
        external: u16,
        lifetime: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Too short, or not a response.
    Malformed,
    /// Another protocol version: a PCP-only router answers NAT-PMP so.
    Version(u8),
    /// The router refused with this result code ([`describe`]).
    Refused(u16),
}

pub fn parse(datagram: &[u8]) -> Result<Response, Error> {
    if datagram.len() < 4 {
        return Err(Error::Malformed);
    }
    if datagram[0] != 0 {
        return Err(Error::Version(datagram[0]));
    }
    let opcode = datagram[1];
    if opcode & 0x80 == 0 {
        return Err(Error::Malformed);
    }
    let result = u16::from_be_bytes([datagram[2], datagram[3]]);
    if result != 0 {
        return Err(Error::Refused(result));
    }
    let word = |at: usize| -> Result<u32, Error> {
        datagram
            .get(at..at + 4)
            .map(|bytes| u32::from_be_bytes(bytes.try_into().unwrap()))
            .ok_or(Error::Malformed)
    };
    let half = |at: usize| -> Result<u16, Error> {
        datagram
            .get(at..at + 2)
            .map(|bytes| u16::from_be_bytes(bytes.try_into().unwrap()))
            .ok_or(Error::Malformed)
    };
    match opcode & 0x7f {
        0 => Ok(Response::Address {
            epoch: word(4)?,
            address: Ipv4Addr::from(word(8)?),
        }),
        op @ (1 | 2) => Ok(Response::Mapping {
            protocol: if op == 1 {
                Protocol::Udp
            } else {
                Protocol::Tcp
            },
            epoch: word(4)?,
            internal: half(8)?,
            external: half(10)?,
            lifetime: word(12)?,
        }),
        _ => Err(Error::Malformed),
    }
}

/// A NAT-PMP result code in words, for the console.
pub fn describe(code: u16) -> &'static str {
    match code {
        1 => "the router does not support this NAT-PMP version",
        2 => "the router refused (NAT-PMP may be turned off on it)",
        3 => "the router has no internet connection",
        4 => "the router is out of mappings",
        5 => "the router does not support this request",
        _ => "the router refused the mapping",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_have_the_rfc_layout() {
        assert_eq!(address_request(), [0, 0]);
        assert_eq!(
            map_request(Protocol::Tcp, 47984, 47984, 7200),
            [0, 2, 0, 0, 0xbb, 0x70, 0xbb, 0x70, 0, 0, 0x1c, 0x20]
        );
        assert_eq!(map_request(Protocol::Udp, 47998, 0, 0)[1], 1);
    }

    #[test]
    fn responses_parse() {
        let address = [0, 128, 0, 0, 0, 0, 0, 9, 203, 0, 113, 7];
        assert_eq!(
            parse(&address),
            Ok(Response::Address {
                epoch: 9,
                address: Ipv4Addr::new(203, 0, 113, 7)
            })
        );
        let mapping = [
            0, 130, 0, 0, 0, 0, 0, 1, 0xbb, 0x70, 0xbb, 0x71, 0, 0, 0x1c, 0x20,
        ];
        assert_eq!(
            parse(&mapping),
            Ok(Response::Mapping {
                protocol: Protocol::Tcp,
                epoch: 1,
                internal: 47984,
                external: 47985,
                lifetime: 7200
            })
        );
    }

    #[test]
    fn refusals_versions_and_garbage_are_errors() {
        assert_eq!(parse(&[0, 129, 0, 2, 0, 0, 0, 0]), Err(Error::Refused(2)));
        assert_eq!(
            describe(2),
            "the router refused (NAT-PMP may be turned off on it)"
        );
        // A PCP router answers with its own version.
        assert_eq!(parse(&[2, 0x81, 0, 1]), Err(Error::Version(2)));
        assert_eq!(parse(&[0, 1, 0, 0]), Err(Error::Malformed));
        assert_eq!(parse(&[0, 128, 0, 0, 0, 0]), Err(Error::Malformed));
        assert_eq!(parse(&[0]), Err(Error::Malformed));
    }
}
