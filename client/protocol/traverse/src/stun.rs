//! STUN binding requests (RFC 5389) for the public address a NAT gives a socket, over IPv4 and
//! IPv6, and what two answers tell about the NAT's mapping.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

pub const COOKIE: u32 = 0x2112_a442;
pub const HEADER_BYTES: usize = 20;
const BINDING_REQUEST: u16 = 0x0001;
const BINDING_SUCCESS: u16 = 0x0101;
const MAPPED_ADDRESS: u16 = 0x0001;
const XOR_MAPPED_ADDRESS: u16 = 0x0020;

/// The 20-byte binding request for `transaction`.
pub fn binding_request(transaction: &[u8; 12]) -> [u8; HEADER_BYTES] {
    let mut request = [0u8; HEADER_BYTES];
    request[..2].copy_from_slice(&BINDING_REQUEST.to_be_bytes());
    request[4..8].copy_from_slice(&COOKIE.to_be_bytes());
    request[8..].copy_from_slice(transaction);
    request
}

/// Whether a datagram looks like STUN: the two top bits clear and the magic cookie in place.
/// Records and DHT messages never do, so one socket carries all three.
pub fn is_stun(datagram: &[u8]) -> bool {
    datagram.len() >= HEADER_BYTES
        && datagram[0] & 0xc0 == 0
        && datagram[4..8] == COOKIE.to_be_bytes()
}

/// The transaction of a STUN message.
pub fn transaction(datagram: &[u8]) -> Option<[u8; 12]> {
    is_stun(datagram).then(|| datagram[8..20].try_into().unwrap())
}

/// The mapped address of a binding success for `transaction`, IPv4 or IPv6; XOR-MAPPED-ADDRESS
/// wins over MAPPED-ADDRESS. None for anything else.
pub fn mapped_address(response: &[u8], transaction: &[u8; 12]) -> Option<SocketAddr> {
    if !is_stun(response)
        || u16::from_be_bytes([response[0], response[1]]) != BINDING_SUCCESS
        || response[8..20] != transaction[..]
    {
        return None;
    }
    let length = u16::from_be_bytes([response[2], response[3]]) as usize;
    let attributes = response.get(HEADER_BYTES..HEADER_BYTES + length)?;
    let mut plain = None;
    let mut at = 0;
    while at + 4 <= attributes.len() {
        let kind = u16::from_be_bytes([attributes[at], attributes[at + 1]]);
        let size = u16::from_be_bytes([attributes[at + 2], attributes[at + 3]]) as usize;
        let value = attributes.get(at + 4..at + 4 + size)?;
        let xor = kind == XOR_MAPPED_ADDRESS;
        if kind == XOR_MAPPED_ADDRESS || kind == MAPPED_ADDRESS {
            if let Some(address) = address(value, xor, transaction) {
                if xor {
                    return Some(address);
                }
                plain = Some(address);
            }
        }
        // Attributes are padded to four bytes.
        at += 4 + size.div_ceil(4) * 4;
    }
    plain
}

fn address(value: &[u8], xor: bool, transaction: &[u8; 12]) -> Option<SocketAddr> {
    if value.len() < 4 {
        return None;
    }
    let mut port = u16::from_be_bytes([value[2], value[3]]);
    let mut mask = [0u8; 16];
    mask[..4].copy_from_slice(&COOKIE.to_be_bytes());
    mask[4..].copy_from_slice(transaction);
    if xor {
        port ^= (COOKIE >> 16) as u16;
    }
    let ip = match (value[1], value.len()) {
        (1, 8) => {
            let mut octets: [u8; 4] = value[4..8].try_into().unwrap();
            if xor {
                octets
                    .iter_mut()
                    .zip(mask)
                    .for_each(|(byte, mask)| *byte ^= mask);
            }
            IpAddr::V4(Ipv4Addr::from(octets))
        }
        (2, 20) => {
            let mut octets: [u8; 16] = value[4..20].try_into().unwrap();
            if xor {
                octets
                    .iter_mut()
                    .zip(mask)
                    .for_each(|(byte, mask)| *byte ^= mask);
            }
            IpAddr::V6(Ipv6Addr::from(octets))
        }
        _ => return None,
    };
    Some(SocketAddr::new(ip, port))
}

/// How a NAT maps one socket's datagrams to different destinations (RFC 4787).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mapping {
    /// The socket has its public address itself: no NAT on the way.
    None,
    /// The same public address and port for every destination: punchable.
    Independent,
    /// Another public port per destination (a symmetric NAT): punchable only from a peer whose
    /// own NAT doesn't filter by port, or by guessing the next port.
    Dependent,
    Unknown,
}

/// The mapping two or more STUN servers' answers for one socket show; `local` is the socket's
/// own address.
pub fn mapping(local: SocketAddr, answers: &[SocketAddr]) -> Mapping {
    let Some(first) = answers.first() else {
        return Mapping::Unknown;
    };
    if answers.iter().any(|answer| answer != first) {
        return Mapping::Dependent;
    }
    if !local.ip().is_unspecified() && *first == local {
        return Mapping::None;
    }
    if answers.len() < 2 {
        return Mapping::Unknown;
    }
    Mapping::Independent
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(attribute: u16, value: &[u8], transaction: &[u8; 12]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&BINDING_SUCCESS.to_be_bytes());
        out.extend_from_slice(&((4 + value.len().div_ceil(4) * 4) as u16).to_be_bytes());
        out.extend_from_slice(&COOKIE.to_be_bytes());
        out.extend_from_slice(transaction);
        out.extend_from_slice(&attribute.to_be_bytes());
        out.extend_from_slice(&(value.len() as u16).to_be_bytes());
        out.extend_from_slice(value);
        while out.len() % 4 != 0 {
            out.push(0);
        }
        out
    }

    #[test]
    fn the_request_is_a_binding_with_the_cookie() {
        let request = binding_request(&[9; 12]);
        assert_eq!(&request[..4], &[0, 1, 0, 0]);
        assert!(is_stun(&request));
        assert_eq!(transaction(&request), Some([9; 12]));
    }

    #[test]
    fn xor_mapped_ipv4_and_ipv6_decode() {
        let transaction = [3; 12];
        // RFC 5769 2.2: 192.0.2.1:32853 as XOR-MAPPED-ADDRESS.
        let v4 = response(
            XOR_MAPPED_ADDRESS,
            &[0, 1, 0xa1, 0x47, 0xe1, 0x12, 0xa6, 0x43],
            &transaction,
        );
        assert_eq!(
            mapped_address(&v4, &transaction),
            Some("192.0.2.1:32853".parse().unwrap())
        );
        // RFC 5769 2.3's address, masked with the cookie and this transaction.
        let wanted: Ipv6Addr = "2001:db8:1234:5678:11:2233:4455:6677".parse().unwrap();
        let mut value = vec![0, 2, 0xa1, 0x47];
        let mut mask = COOKIE.to_be_bytes().to_vec();
        mask.extend_from_slice(&transaction);
        value.extend(wanted.octets().iter().zip(&mask).map(|(a, b)| a ^ b));
        let v6 = response(XOR_MAPPED_ADDRESS, &value, &transaction);
        assert_eq!(
            mapped_address(&v6, &transaction),
            Some(SocketAddr::new(wanted.into(), 32853))
        );
    }

    #[test]
    fn plain_mapped_address_is_the_fallback_and_other_transactions_are_ignored() {
        let transaction = [4; 12];
        let plain = response(
            MAPPED_ADDRESS,
            &[0, 1, 0x1f, 0x90, 203, 0, 113, 5],
            &transaction,
        );
        assert_eq!(
            mapped_address(&plain, &transaction),
            Some("203.0.113.5:8080".parse().unwrap())
        );
        assert_eq!(mapped_address(&plain, &[5; 12]), None);
        assert_eq!(mapped_address(&plain[..19], &transaction), None);
        // An attribute running past the message is not read.
        let mut cut = plain.clone();
        cut[3] = 64;
        assert_eq!(mapped_address(&cut, &transaction), None);
    }

    #[test]
    fn two_answers_classify_the_mapping() {
        let local: SocketAddr = "192.168.1.5:50000".parse().unwrap();
        let a: SocketAddr = "203.0.113.5:40000".parse().unwrap();
        let b: SocketAddr = "203.0.113.5:40001".parse().unwrap();
        assert_eq!(mapping(local, &[a, a]), Mapping::Independent);
        assert_eq!(mapping(local, &[a, b]), Mapping::Dependent);
        assert_eq!(mapping(local, &[a]), Mapping::Unknown);
        assert_eq!(mapping(a, &[a, a]), Mapping::None);
        assert_eq!(mapping(local, &[]), Mapping::Unknown);
    }
}
