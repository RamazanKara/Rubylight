//! Sends a paired client's PyroWave UDP probe (`butterpollo_core::pyrowave_probe`)
//! from the address its HTTPS request reached, to that client's own address.
use anyhow::{Context, Result, bail};
use butterpollo_core::pyrowave_probe::{self, DURATION, Request, TOKEN_LEN};
use butterpollo_windows::net::Batch;
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
    /// Bounded waits after a full send buffer refused a batch.
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
    let ipv6 = destination.ip().to_canonical().is_ipv6();
    let schedule = pyrowave_probe::schedule(request, ipv6);
    let packet = pyrowave_probe::packet(&request.token, request.packet_size);
    let packet_len = packet.len();
    let mut packets = vec![packet; 64];
    let mut batch = Batch::default();
    batch.waits = Some(Default::default());
    let mut outcome = Outcome {
        expected: pyrowave_probe::expected(request.kbps, request.packet_size),
        ..Default::default()
    };
    let start = Instant::now();
    let mut pacer = butterpollo_core::network_pacing::Pacer::new(start);
    let pace_bps = u64::from(request.burst.map_or(request.kbps, |burst| burst.pace_kbps)) * 1000;
    let batch_limit =
        butterpollo_core::network_pacing::pyrowave_batch_limit(pace_bps, packet_len, ipv6);
    // An overloaded sender stops instead of queueing an ever later probe.
    let limit = DURATION + Duration::from_millis(100);
    let mut sequence = 0;
    'probe: for (offset, end) in schedule {
        while sequence < end {
            timer.until_precise((start + offset).max(pacer.due()));
            if start.elapsed() > limit {
                break 'probe;
            }
            let available = ((end - sequence) as usize).min(packets.len());
            let count = Batch::count(&packets[..available], batch_limit);
            for (index, packet) in packets[..count].iter_mut().enumerate() {
                pyrowave_probe::set_sequence(packet, sequence + index as u32);
            }
            // Use the stream's segmentation, fallback and shared retry budget,
            // so calibration exercises the same bursts and socket pressure.
            let bytes = batch.send(socket, &packets[..count], destination)?;
            let sent = bytes / packet_len;
            outcome.sent += sent as u32;
            pacer.sent(Instant::now(), bytes, sent, ipv6, pace_bps);
            sequence += count as u32;
        }
    }
    timer.until(start + DURATION);
    outcome.elapsed = start.elapsed();
    if let Some(waits) = batch.waits {
        outcome.send_retries = waits.count.min(u64::from(u32::MAX)) as u32;
        outcome.last_send_error = waits.last_error;
    }
    Ok(outcome)
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
        receives_probe(None);
    }

    #[test]
    fn a_frame_shaped_probe_delivers_batched_packets_without_duplicates() {
        receives_probe(Some(pyrowave_probe::Burst {
            fps: 60,
            pace_kbps: 800_000,
        }));
    }

    fn receives_probe(burst: Option<pyrowave_probe::Burst>) {
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        // Retain the complete probe while a busy test runner deschedules us.
        socket2::SockRef::from(&client)
            .set_recv_buffer_size(8 * 1024 * 1024)
            .unwrap();
        let peer = client.local_addr().unwrap();
        let socket = bind("127.0.0.1".parse().unwrap()).unwrap();
        let receiver =
            std::thread::spawn(move || receive(&client, pyrowave_probe::expected(20_000, 1024)));
        let mut request = request(false);
        request.burst = burst;
        let outcome = send(&socket, peer, &request).unwrap();
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
        // Retain the complete probe while a busy test runner deschedules us.
        socket2::SockRef::from(&answering)
            .set_recv_buffer_size(8 * 1024 * 1024)
            .unwrap();
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
