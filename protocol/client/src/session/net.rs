//! Addresses and sockets: name resolution, the local address toward the host, the
//! local/remote decision, the media sockets and their pings.
//!
//! Ported from moonlight-common-c (GPL-3.0, Cameron Gutman and contributors):
//! `PlatformSockets.c` (resolveHostName, getLocalAddressByUdpConnect, bindUdpSocket,
//! recvUdpSocket, isPrivateNetworkAddress with Nightjar's `android_sockets.h` prefix fix,
//! isNat64SynthesizedAddress) and the ping threads of `VideoStream.c` and `AudioStream.c`.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use socket2::{Domain, Protocol, Socket, Type};

use crate::priority::{self, Qos};

/// How long a media receive waits before checking for shutdown and timeouts.
pub const UDP_RECV_POLL_TIMEOUT: Duration = Duration::from_millis(100);
const TEST_PORT_TIMEOUT: Duration = Duration::from_secs(3);
const RCV_BUFFER_SIZE_MIN: usize = 32767;
const RCV_BUFFER_SIZE_STEP: usize = 16384;
const PING_INTERVAL: Duration = Duration::from_millis(500);
const LEGACY_PING: [u8; 4] = *b"PING";

/// Resolves `host` (a name or literal, IPv6 with or without brackets). With several
/// addresses, the first that accepts a TCP connection on one of `test_ports` wins, as the C
/// core does; otherwise the first address.
pub fn resolve(host: &str, test_ports: &[u16]) -> io::Result<SocketAddr> {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let addresses: Vec<SocketAddr> = (host, 0).to_socket_addrs()?.collect();
    let first = *addresses
        .first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no address for host"))?;
    if addresses.len() == 1 {
        return Ok(first);
    }
    for address in &addresses {
        for &port in test_ports {
            let mut candidate = *address;
            candidate.set_port(port);
            if TcpStream::connect_timeout(&candidate, TEST_PORT_TIMEOUT).is_ok() {
                return Ok(*address);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::ConnectionRefused,
        "no working address for host",
    ))
}

/// The local address the OS routes toward `remote`, found with a connected UDP socket.
pub fn local_address_toward(remote: SocketAddr) -> io::Result<IpAddr> {
    let bind: SocketAddr = match remote {
        SocketAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        SocketAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
    };
    let socket = UdpSocket::bind(bind)?;
    socket.connect(remote)?;
    Ok(socket.local_addr()?.ip())
}

fn in_prefix(address: &[u8; 16], prefix: &[u8], bits: usize) -> bool {
    (0..bits).all(|i| {
        let mask = 0x80u8 >> (i % 8);
        address[i / 8] & mask == prefix[i / 8] & mask
    })
}

/// moonlight-common-c's isPrivateNetworkAddress: RFC 1918 and link-local IPv4 (not CGN);
/// link-local, site-local and unique-local IPv6.
pub fn is_private(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => {
            let a = u32::from(v4);
            a & 0xFF00_0000 == 0x0A00_0000
                || a & 0xFFF0_0000 == 0xAC10_0000
                || a & 0xFFFF_0000 == 0xC0A8_0000
                || a & 0xFFFF_0000 == 0xA9FE_0000
        }
        IpAddr::V6(v6) => {
            let o = v6.octets();
            in_prefix(&o, &[0xfe, 0x80], 10)
                || in_prefix(&o, &[0xfe, 0xc0], 10)
                || in_prefix(&o, &[0xfc, 0x00], 7)
        }
    }
}

/// The IPv4 address embedded in an IPv6 address under an RFC 6052 prefix of `bits`.
fn rfc6052_embedded(address: &[u8; 16], bits: usize) -> [u8; 4] {
    // Byte 8 (bits 64-71) is reserved and skipped.
    let positions: Vec<usize> = (bits / 8..16).filter(|&i| i != 8).take(4).collect();
    let mut out = [0u8; 4];
    for (slot, &at) in out.iter_mut().zip(&positions) {
        *slot = address[at];
    }
    out
}

/// Whether `address` is under a NAT64 prefix of `candidates` (the IPv6 answers for
/// `ipv4only.arpa`, RFC 7050).
pub fn is_nat64_with(address: &Ipv6Addr, candidates: &[Ipv6Addr]) -> bool {
    const WELL_KNOWN: [[u8; 4]; 2] = [[192, 0, 0, 170], [192, 0, 0, 171]];
    let target = address.octets();
    candidates.iter().any(|candidate| {
        let c = candidate.octets();
        [32, 40, 48, 56, 64, 96].iter().any(|&bits| {
            WELL_KNOWN.contains(&rfc6052_embedded(&c, bits)) && c[..bits / 8] == target[..bits / 8]
        })
    })
}

/// moonlight-common-c's isNat64SynthesizedAddress: discovers the NAT64 prefix through DNS.
pub fn is_nat64(address: IpAddr) -> bool {
    let IpAddr::V6(v6) = address else {
        return false;
    };
    let Ok(answers) = ("ipv4only.arpa.", 0).to_socket_addrs() else {
        return false;
    };
    let candidates: Vec<Ipv6Addr> = answers
        .filter_map(|a| match a.ip() {
            IpAddr::V6(v6) => Some(v6),
            IpAddr::V4(_) => None,
        })
        .collect();
    is_nat64_with(&v6, &candidates)
}

/// A UDP socket bound to `local` on an ephemeral port, with the receive buffer stepped down
/// from `buffer_size` until the OS accepts it (0 keeps the default). `qos` sets the queue
/// priority as moonlight-common-c's `setSocketQos` does; a kernel that refuses it is not an
/// error, the same as there.
pub fn bind_udp(local: IpAddr, buffer_size: usize, qos: Qos) -> io::Result<UdpSocket> {
    let address = SocketAddr::new(local, 0);
    let socket = Socket::new(
        Domain::for_address(address),
        Type::DGRAM,
        Some(Protocol::UDP),
    )?;
    socket.bind(&address.into())?;
    let _ = priority::prioritize_socket(&socket, qos);
    let mut size = buffer_size;
    while size != 0 {
        if socket.set_recv_buffer_size(size).is_ok() || size <= RCV_BUFFER_SIZE_MIN {
            break;
        }
        size = if size - RCV_BUFFER_SIZE_STEP <= RCV_BUFFER_SIZE_MIN {
            RCV_BUFFER_SIZE_MIN
        } else {
            size - RCV_BUFFER_SIZE_STEP
        };
    }
    let socket: UdpSocket = socket.into();
    socket.set_read_timeout(Some(UDP_RECV_POLL_TIMEOUT))?;
    Ok(socket)
}

/// One receive: `Ok(None)` on the poll timeout. ICMP port-unreachable reports (a ping sent
/// before the host bound its port) are skipped, as in recvUdpSocket.
pub fn receive(socket: &UdpSocket, buffer: &mut [u8]) -> io::Result<Option<usize>> {
    loop {
        match socket.recv(buffer) {
            Ok(n) => return Ok(Some(n)),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                return Ok(None)
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionRefused
                ) => {}
            // WSA_IO_PENDING, which some Windows systems return instead of a timeout.
            Err(e) if cfg!(windows) && e.raw_os_error() == Some(997) => return Ok(None),
            Err(e) => return Err(e),
        }
    }
}

/// A stop flag that sleeping threads wake up for.
#[derive(Default)]
pub struct StopSignal {
    stopped: Mutex<bool>,
    wake: Condvar,
}

impl StopSignal {
    pub fn stop(&self) {
        *self.stopped.lock().unwrap_or_else(|p| p.into_inner()) = true;
        self.wake.notify_all();
    }

    /// Sleeps up to `timeout`; returns whether the signal was stopped.
    pub fn sleep(&self, timeout: Duration) -> bool {
        let guard = self.stopped.lock().unwrap_or_else(|p| p.into_inner());
        let (guard, _) = self
            .wake
            .wait_timeout_while(guard, timeout, |stopped| !*stopped)
            .unwrap_or_else(|p| p.into_inner());
        *guard
    }
}

/// The datagram of ping `count`: the RTSP ping payload and a big-endian sequence number
/// (`SS_PING`), or the legacy "PING" without a payload.
pub fn ping_datagram(payload: Option<[u8; 16]>, count: u32) -> Vec<u8> {
    match payload {
        Some(payload) if payload[0] != 0 => {
            let mut out = payload.to_vec();
            out.extend_from_slice(&count.to_be_bytes());
            out
        }
        _ => LEGACY_PING.to_vec(),
    }
}

/// Pings `target` from `socket` every 500 ms until stopped. Send errors are ignored: the
/// receive side handles socket failures.
pub struct Pinger {
    stop: Arc<StopSignal>,
    thread: Option<JoinHandle<()>>,
}

impl Pinger {
    pub fn start(
        name: &str,
        socket: &UdpSocket,
        target: SocketAddr,
        payload: Option<[u8; 16]>,
    ) -> io::Result<Self> {
        let socket = socket.try_clone()?;
        let stop = Arc::new(StopSignal::default());
        let signal = stop.clone();
        let thread = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let mut count = 0u32;
                loop {
                    count = count.wrapping_add(1);
                    let _ = socket.send_to(&ping_datagram(payload, count), target);
                    if signal.sleep(PING_INTERVAL) {
                        break;
                    }
                }
            })?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }

    pub fn stop(&mut self) {
        self.stop.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Pinger {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_ranges_match_the_c() {
        for private in ["10.1.2.3", "172.31.0.1", "192.168.1.1", "169.254.0.5"] {
            assert!(is_private(private.parse().unwrap()), "{private}");
        }
        for public in ["8.8.8.8", "172.32.0.1", "100.64.0.1", "127.0.0.1"] {
            assert!(!is_private(public.parse().unwrap()), "{public}");
        }
        for private in ["fe80::1", "fec0::1", "fd00::1", "fc12::1"] {
            assert!(is_private(private.parse().unwrap()), "{private}");
        }
        for public in ["2001:db8::1", "::1", "fe00::1"] {
            assert!(!is_private(public.parse().unwrap()), "{public}");
        }
    }

    #[test]
    fn nat64_prefixes_are_recognized() {
        let well_known: Ipv6Addr = "64:ff9b::c000:aa".parse().unwrap();
        assert!(is_nat64_with(
            &"64:ff9b::808:808".parse().unwrap(),
            &[well_known]
        ));
        assert!(!is_nat64_with(
            &"2001:db8::808:808".parse().unwrap(),
            &[well_known]
        ));
        // A /64 prefix: the IPv4 address sits after the reserved byte 8.
        let slash64: Ipv6Addr = "2001:db8:1:2:c0:0:aa00:0".parse().unwrap();
        assert!(is_nat64_with(
            &"2001:db8:1:2:0808:0808::".parse().unwrap(),
            &[slash64]
        ));
        assert!(!is_nat64_with(&"64:ff9b::808:808".parse().unwrap(), &[]));
    }

    #[test]
    fn pings_carry_the_payload_and_a_sequence() {
        let ping = ping_datagram(Some(*b"0123456789abcdef"), 2);
        assert_eq!(&ping[..16], b"0123456789abcdef");
        assert_eq!(&ping[16..], &[0, 0, 0, 2]);
        assert_eq!(ping_datagram(None, 1), b"PING");
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn media_sockets_are_bound_with_their_priority() {
        use std::os::fd::AsRawFd;
        fn read(socket: &UdpSocket) -> i32 {
            let mut value: i32 = -1;
            let mut length = std::mem::size_of::<i32>() as libc::socklen_t;
            let result = unsafe {
                libc::getsockopt(
                    socket.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_PRIORITY,
                    (&mut value as *mut i32).cast(),
                    &mut length,
                )
            };
            assert_eq!(result, 0);
            value
        }
        let video = bind_udp(Ipv4Addr::LOCALHOST.into(), 0, Qos::Video).unwrap();
        assert_eq!(read(&video), 5);
        let audio = bind_udp(Ipv4Addr::LOCALHOST.into(), 0, Qos::Audio).unwrap();
        assert_eq!(read(&audio), 6);
    }

    #[test]
    fn receive_times_out_without_data() {
        let socket = bind_udp(Ipv4Addr::LOCALHOST.into(), 1 << 20, Qos::BestEffort).unwrap();
        let mut buffer = [0u8; 64];
        assert!(receive(&socket, &mut buffer).unwrap().is_none());
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        sender
            .send_to(b"hello", socket.local_addr().unwrap())
            .unwrap();
        assert_eq!(receive(&socket, &mut buffer).unwrap(), Some(5));
    }
}
