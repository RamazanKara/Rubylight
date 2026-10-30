//! IPv4 STUN binding discovery: the WAN address a NAT gives this device, so a PC found on the LAN
//! can be reached from outside it at that address (Moonlight's `LiFindExternalAddressIP4`, which
//! Moonlight V+ PC runs for a PC found by mDNS or added by a private IPv4 address; Android's copy
//! is `android_stun.c`). RFC 5389 binding request; XOR-MAPPED-ADDRESS, else MAPPED-ADDRESS.

use std::io;
use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

use ring::rand::{SecureRandom, SystemRandom};

/// The STUN server Moonlight clients ask.
pub const MOONLIGHT_STUN_SERVER: &str = "stun.moonlight-stream.org";
pub const MOONLIGHT_STUN_PORT: u16 = 3478;
/// How long a query waits in all, as `android_stun.c`'s `STUN_RECV_TIMEOUT_SEC`.
pub const STUN_TIMEOUT: Duration = Duration::from_secs(3);

const COOKIE: u32 = 0x2112_a442;
const HEADER_BYTES: usize = 20;
const BINDING_REQUEST: u16 = 0x0001;
const BINDING_SUCCESS: u16 = 0x0101;
const MAPPED_ADDRESS: u16 = 0x0001;
const XOR_MAPPED_ADDRESS: u16 = 0x0020;
const RESEND: Duration = Duration::from_secs(1);

/// The 20-byte binding request for `transaction`.
pub fn binding_request(transaction: &[u8; 12]) -> [u8; HEADER_BYTES] {
    let mut request = [0u8; HEADER_BYTES];
    request[..2].copy_from_slice(&BINDING_REQUEST.to_be_bytes());
    request[4..8].copy_from_slice(&COOKIE.to_be_bytes());
    request[8..].copy_from_slice(transaction);
    request
}

/// The mapped IPv4 address of a binding success for `transaction`; None for anything else.
pub fn mapped_address(response: &[u8], transaction: &[u8; 12]) -> Option<Ipv4Addr> {
    if response.len() < HEADER_BYTES
        || u16::from_be_bytes([response[0], response[1]]) != BINDING_SUCCESS
        || response[4..8] != COOKIE.to_be_bytes()
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
        // Family 1 is IPv4: reserved byte, family, port, address.
        if size == 8 && value[1] == 1 {
            let address = u32::from_be_bytes([value[4], value[5], value[6], value[7]]);
            match kind {
                XOR_MAPPED_ADDRESS => return Some(Ipv4Addr::from(address ^ COOKIE)),
                MAPPED_ADDRESS => plain = Some(Ipv4Addr::from(address)),
                _ => {}
            }
        }
        at += 4 + size.div_ceil(4) * 4;
    }
    plain
}

/// Asks `server` for this device's WAN IPv4 address: up to three of its IPv4 addresses, again
/// every second, until one answers or `timeout` ends (`Ok(None)`).
pub fn find_external_ipv4(server: &str, port: u16, timeout: Duration) -> io::Result<Option<Ipv4Addr>> {
    let servers: Vec<SocketAddr> = (server, port).to_socket_addrs()?.filter(SocketAddr::is_ipv4).take(3).collect();
    if servers.is_empty() {
        return Ok(None);
    }
    let mut transaction = [0u8; 12];
    SystemRandom::new()
        .fill(&mut transaction)
        .map_err(|_| io::Error::other("no random STUN transaction ID"))?;
    let request = binding_request(&transaction);
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    let deadline = Instant::now() + timeout;
    let mut next_send = Instant::now();
    let mut buffer = [0u8; 1024];
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        if now >= next_send {
            for address in &servers {
                socket.send_to(&request, address)?;
            }
            next_send = now + RESEND;
        }
        socket.set_read_timeout(Some((deadline - now).min(next_send - now).max(Duration::from_millis(1))))?;
        match socket.recv_from(&mut buffer) {
            Ok((read, from)) if servers.contains(&from) => {
                if let Some(address) = mapped_address(&buffer[..read], &transaction) {
                    return Ok(Some(address));
                }
            }
            Ok(_) => {}
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            // Windows reports an ICMP port unreachable from an earlier send on the next receive.
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => {}
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn success(transaction: &[u8; 12], attributes: &[u8]) -> Vec<u8> {
        let mut response = vec![0x01, 0x01];
        response.extend((attributes.len() as u16).to_be_bytes());
        response.extend(COOKIE.to_be_bytes());
        response.extend(transaction);
        response.extend(attributes);
        response
    }

    // 203.0.113.7:4500 as XOR-MAPPED-ADDRESS (port ^ 0x2112, address ^ cookie), after an unknown
    // attribute whose 3 bytes are padded to 4.
    const XOR_ATTRIBUTES: [u8; 20] = [
        0x80, 0x22, 0, 3, b'r', b'l', b'!', 0, //
        0x00, 0x20, 0, 8, 0, 1, 0x30, 0x86, 0xea, 0x12, 0xd5, 0x45,
    ];

    #[test]
    fn the_binding_request_is_the_rfc_5389_header() {
        let transaction = [7u8; 12];
        let request = binding_request(&transaction);
        assert_eq!(&request[..8], &[0x00, 0x01, 0x00, 0x00, 0x21, 0x12, 0xa4, 0x42]);
        assert_eq!(&request[8..], &transaction);
    }

    #[test]
    fn the_mapped_address_comes_back_only_for_this_transaction() {
        let transaction = [9u8; 12];
        let ok = success(&transaction, &XOR_ATTRIBUTES);
        assert_eq!(mapped_address(&ok, &transaction), Some(Ipv4Addr::new(203, 0, 113, 7)));
        assert_eq!(mapped_address(&ok, &[1u8; 12]), None, "another transaction");
        let mut error = ok.clone();
        error[1] = 0x11;
        assert_eq!(mapped_address(&error, &transaction), None, "a binding error");
        assert_eq!(mapped_address(&ok[..30], &transaction), None, "truncated");
        // An old server's plain MAPPED-ADDRESS, as android_stun.c also takes.
        let plain = success(&transaction, &[0x00, 0x01, 0, 8, 0, 1, 0x11, 0x94, 198, 51, 100, 2]);
        assert_eq!(mapped_address(&plain, &transaction), Some(Ipv4Addr::new(198, 51, 100, 2)));
    }

    #[test]
    fn a_query_reads_the_answer_of_the_server_it_asked() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = server.local_addr().unwrap().port();
        let answer = std::thread::spawn(move || {
            let mut request = [0u8; 64];
            let (read, from) = server.recv_from(&mut request).unwrap();
            assert_eq!(read, HEADER_BYTES);
            let transaction: [u8; 12] = request[8..20].try_into().unwrap();
            assert_eq!(binding_request(&transaction), request[..HEADER_BYTES]);
            server.send_to(&success(&transaction, &XOR_ATTRIBUTES), from).unwrap();
        });
        let found = find_external_ipv4("127.0.0.1", port, Duration::from_secs(3)).unwrap();
        answer.join().unwrap();
        assert_eq!(found, Some(Ipv4Addr::new(203, 0, 113, 7)));
    }

    #[test]
    fn no_answer_is_no_address() {
        let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = silent.local_addr().unwrap().port();
        assert_eq!(find_external_ipv4("127.0.0.1", port, Duration::from_millis(300)).unwrap(), None);
        assert_eq!(find_external_ipv4("::1", port, Duration::from_millis(300)).unwrap(), None, "no IPv4 address");
    }
}
