//! Sends a paired client's PyroWave UDP probe (`butterpollo_core::pyrowave_probe`)
//! from the address its HTTPS request reached, to that client's own address.
use anyhow::{Context, Result, bail};
use butterpollo_core::pyrowave_probe::{self, DURATION, Request, TOKEN_LEN};
use std::{
    io::ErrorKind,
    net::{IpAddr, SocketAddr, UdpSocket},
    time::{Duration, Instant},
};

/// What the client compares its count with.
#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    pub expected: u32,
    pub sent: u32,
    pub elapsed: Duration,
    /// Sends refused for a full buffer and retried.
    pub send_retries: u32,
    /// The last refusal's OS error, 0 for none.
    pub last_send_error: i32,
}

/// The probe's socket on the address the client's request reached.
pub fn bind(local: IpAddr) -> Result<UdpSocket> {
    let domain = if local.is_ipv6() {
        socket2::Domain::IPV6
    } else {
        socket2::Domain::IPV4
    };
    let socket = socket2::Socket::new(domain, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
    // A frame-shaped burst at a fast pace queues many packets at once.
    let _ = socket.set_send_buffer_size(4 * 1024 * 1024);
    socket.bind(&SocketAddr::new(local, 0).into())?;
    socket.set_nonblocking(true)?;
    Ok(socket.into())
}

/// Wait for the client's token on `socket`, sending it ours every 100 ms, and
/// return the address it came from: the client's own receiving port, through
/// whatever its firewall or NAT made of it.
fn handshake(socket: &UdpSocket, peer: SocketAddr, token: &str) -> Result<SocketAddr> {
    let deadline = Instant::now() + Duration::from_millis(1500);
    let mut warmup = Instant::now();
    let mut buffer = [0u8; 64];
    while Instant::now() < deadline {
        if Instant::now() >= warmup {
            let _ = socket.send_to(token.as_bytes(), peer);
            warmup = Instant::now() + Duration::from_millis(100);
        }
        match socket.recv_from(&mut buffer) {
            Ok((bytes, source))
                if bytes == TOKEN_LEN
                    && source.ip().to_canonical() == peer.ip().to_canonical()
                    && source.port() >= 1024
                    && &buffer[..bytes] == token.as_bytes() =>
            {
                return Ok(source);
            }
            Ok(_) => {}
            // Windows reports an ICMP port unreachable for an earlier warmup
            // as a reset; the client may simply not be listening yet.
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::WouldBlock
                        | ErrorKind::ConnectionReset
                        | ErrorKind::ConnectionRefused
                ) || e.raw_os_error() == Some(10040) => {}
            Err(e) => return Err(e).context("waiting for the client's UDP token"),
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    bail!("the client did not complete the UDP handshake")
}

/// Send the probe to `peer` (the client's IP and the port it named), after the
/// handshake when it asked for one.
pub fn send(socket: &UdpSocket, peer: SocketAddr, request: &Request) -> Result<Outcome> {
    let destination = if request.handshake {
        handshake(socket, peer, &request.token)?
    } else {
        peer
    };
    let timer = butterpollo_windows::timing::Timer::new()?;
    let schedule = pyrowave_probe::schedule(request, destination.is_ipv6());
    let mut packet = pyrowave_probe::packet(&request.token, request.packet_size);
    let mut outcome = Outcome {
        expected: pyrowave_probe::expected(request.kbps, request.packet_size),
        ..Default::default()
    };
    let start = Instant::now();
    // An overloaded sender stops instead of queueing an ever later probe.
    let limit = DURATION + Duration::from_millis(100);
    let mut sequence = 0;
    for (offset, end) in schedule {
        if start.elapsed() > limit {
            break;
        }
        timer.until_precise(start + offset);
        for next in sequence..end {
            pyrowave_probe::set_sequence(&mut packet, next);
            send_one(socket, &packet, destination, &mut outcome);
        }
        sequence = end;
    }
    timer.until(start + DURATION);
    outcome.elapsed = start.elapsed();
    Ok(outcome)
}

/// A momentarily full send buffer is retried for up to 1 ms, as the stream
/// sender does, rather than counted lost.
fn send_one(socket: &UdpSocket, packet: &[u8], to: SocketAddr, outcome: &mut Outcome) {
    let until = Instant::now() + Duration::from_millis(1);
    let mut retried = false;
    loop {
        match socket.send_to(packet, to) {
            Ok(bytes) if bytes == packet.len() => {
                outcome.sent += 1;
                return;
            }
            Ok(_) => return,
            Err(e) => {
                outcome.last_send_error = e.raw_os_error().unwrap_or(-1);
                // WSAENOBUFS: the send buffer is full.
                let full = e.kind() == ErrorKind::WouldBlock || e.raw_os_error() == Some(10055);
                if !full || Instant::now() >= until {
                    return;
                }
                if !retried {
                    retried = true;
                    outcome.send_retries += 1;
                }
                std::thread::yield_now();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    const TOKEN: &str = "fedcba9876543210fedcba9876543210";

    fn request(handshake: bool) -> Request {
        Request {
            kbps: 20_000,
            port: 0,
            packet_size: 1024,
            token: TOKEN.into(),
            handshake,
            burst: None,
        }
    }
    fn receive(client: &UdpSocket, expected: u32) -> HashSet<u32> {
        client
            .set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        let mut seen = HashSet::new();
        let mut buffer = [0u8; 2048];
        while let Ok((bytes, _)) = client.recv_from(&mut buffer) {
            if bytes == 1024 + 48 && &buffer[..TOKEN_LEN] == TOKEN.as_bytes() {
                seen.insert(u32::from_be_bytes(buffer[32..36].try_into().unwrap()));
            }
            if seen.len() as u32 == expected {
                break;
            }
        }
        seen
    }

    #[test]
    fn a_client_receives_every_packet_of_a_loopback_probe() {
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        let peer = client.local_addr().unwrap();
        let socket = bind("127.0.0.1".parse().unwrap()).unwrap();
        let receiver =
            std::thread::spawn(move || receive(&client, pyrowave_probe::expected(20_000, 1024)));
        let outcome = send(&socket, peer, &request(false)).unwrap();
        // 20 Mbps for 2 s at 1158 wire bytes a packet.
        assert_eq!(outcome.expected, 4_317);
        // A busy machine can fall behind and stop at the overload limit.
        assert!(outcome.sent > outcome.expected / 2 && outcome.sent <= outcome.expected);
        assert!(
            outcome.elapsed >= DURATION && outcome.elapsed < DURATION + Duration::from_millis(150)
        );
        let seen = receiver.join().unwrap();
        assert_eq!(seen.len() as u32, outcome.sent);
    }

    #[test]
    fn the_handshake_sends_to_the_port_the_client_answered_from() {
        let socket = bind("127.0.0.1".parse().unwrap()).unwrap();
        let host = socket.local_addr().unwrap();
        // The client named one port in its request but answers from another.
        let named = UdpSocket::bind("127.0.0.1:0").unwrap();
        let answering = UdpSocket::bind("127.0.0.1:0").unwrap();
        let peer = named.local_addr().unwrap();
        let receiver = std::thread::spawn(move || {
            answering.send_to(TOKEN.as_bytes(), host).unwrap();
            receive(&answering, pyrowave_probe::expected(20_000, 1024))
        });
        let outcome = send(&socket, peer, &request(true)).unwrap();
        assert_eq!(receiver.join().unwrap().len() as u32, outcome.sent);
    }

    #[test]
    fn a_probe_without_the_clients_token_fails_the_handshake() {
        let socket = bind("127.0.0.1".parse().unwrap()).unwrap();
        let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
        let error = send(&socket, silent.local_addr().unwrap(), &request(true)).unwrap_err();
        assert!(format!("{error:#}").contains("handshake"), "{error:#}");
    }
}
