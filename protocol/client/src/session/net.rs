//! Addresses and sockets: name resolution, the local address toward the host, the
//! local/remote decision, the media sockets and their pings.
//!
//! Ported from moonlight-common-c (GPL-3.0, Cameron Gutman and contributors):
//! `PlatformSockets.c` (resolveHostName, getLocalAddressByUdpConnect, bindUdpSocket,
//! recvUdpSocket, isPrivateNetworkAddress with Nightjar's `android_sockets.h` prefix fix,
//! isNat64SynthesizedAddress) and the ping threads of `VideoStream.c` and `AudioStream.c`.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, Socket, Type};

use crate::priority::{self, Qos};

/// How long a media receive waits before checking for shutdown and timeouts.
pub const UDP_RECV_POLL_TIMEOUT: Duration = Duration::from_millis(100);
/// How long a media receive that failed waits for the roam's new socket before it ends the
/// session: the rebind follows the route poll, which runs every second.
pub const REBIND_WAIT: Duration = Duration::from_secs(1);
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

/// The local address the OS routes toward `remote`, found with a connected UDP socket. The
/// port is 0; an IPv6 scope (of `remote`, kept by the caller) stays, which link-local
/// addresses need to connect and bind.
pub fn local_address_toward(remote: SocketAddr) -> io::Result<SocketAddr> {
    let bind: SocketAddr = match remote {
        SocketAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        SocketAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
    };
    let socket = UdpSocket::bind(bind)?;
    socket.connect(remote)?;
    Ok(with_port(socket.local_addr()?, 0))
}

/// `address` with another port, keeping an IPv6 scope.
pub fn with_port(mut address: SocketAddr, port: u16) -> SocketAddr {
    address.set_port(port);
    address
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

/// A media socket's receive buffer: the size asked for and the size the OS granted. Linux caps
/// it at `net.core.rmem_max` (often 208 KiB) without an error, so only reading it back tells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReceiveBuffer {
    pub asked: usize,
    pub granted: usize,
}

impl ReceiveBuffer {
    /// The OS granted less than asked: a burst beyond it is lost before it is read.
    pub fn short(&self) -> bool {
        self.granted < self.asked
    }

    /// The log line for a socket that carries `what`; a short one says how to raise the cap.
    pub fn describe(&self, what: &str) -> String {
        let kib = |bytes: usize| bytes.div_ceil(1024);
        if !self.short() {
            return format!("{what} receive buffer: {} KiB, as asked", kib(self.granted));
        }
        let raise = if cfg!(any(target_os = "linux", target_os = "android")) {
            format!(" (the system's cap: raise it with `sudo sysctl -w net.core.rmem_max={}`)", self.asked)
        } else {
            String::new()
        };
        format!(
            "{what} receive buffer: the system granted {} KiB of the {} KiB asked{raise}",
            kib(self.granted),
            kib(self.asked)
        )
    }
}

/// The usable size of `socket`'s receive buffer. Linux reports twice the size asked for (the
/// kernel doubles it for its bookkeeping, socket(7)), so it is halved there.
fn granted_receive_buffer(socket: &Socket) -> io::Result<usize> {
    let size = socket.recv_buffer_size()?;
    Ok(if cfg!(any(target_os = "linux", target_os = "android")) {
        size / 2
    } else {
        size
    })
}

/// A UDP socket bound to `local` (with its IPv6 scope) on an ephemeral port, with the
/// receive buffer stepped down from `buffer_size` until the OS accepts it (0 keeps the
/// default). `qos` sets the queue priority as moonlight-common-c's `setSocketQos` does
/// (`network_priority` is "prioritize stream traffic"). A kernel that refuses it is not an
/// error, the same as there, but `warn` hears about it, and about a receive buffer smaller than
/// asked ([`ReceiveBuffer::short`]).
pub fn bind_udp(
    local: SocketAddr,
    buffer_size: usize,
    qos: Qos,
    network_priority: bool,
    warn: impl Fn(&str),
) -> io::Result<UdpSocket> {
    let (socket, buffer) = bind_udp_sized(local, buffer_size, qos, network_priority, &warn)?;
    if let Some(buffer) = buffer.filter(ReceiveBuffer::short) {
        warn(&buffer.describe(&format!("{qos:?}")));
    }
    Ok(socket)
}

static TOS_REFUSED: AtomicBool = AtomicBool::new(false);

/// [`bind_udp`], returning the receive buffer the OS granted instead of warning about it (None
/// with `buffer_size` 0, or when it cannot be read back, which `warn` hears).
pub fn bind_udp_sized(
    local: SocketAddr,
    buffer_size: usize,
    qos: Qos,
    network_priority: bool,
    warn: impl Fn(&str),
) -> io::Result<(UdpSocket, Option<ReceiveBuffer>)> {
    let address = with_port(local, 0);
    let socket = Socket::new(
        Domain::for_address(address),
        Type::DGRAM,
        Some(Protocol::UDP),
    )?;
    socket.bind(&address.into())?;
    // Before the priority: on Linux, IP_TOS resets SO_PRIORITY.
    if let Err(e) = priority::mark_media_socket(&socket, qos, address.is_ipv6()) {
        // Once per process: a network or OS that refuses it refuses it for every socket.
        if !TOS_REFUSED.swap(true, Ordering::Relaxed) {
            warn(&format!(
                "Could not mark the {qos:?} socket's DSCP, sending unmarked: {e}"
            ));
        }
    }
    if let Err(e) = priority::prioritize_socket(&socket, qos, network_priority) {
        warn(&format!("Could not set the {qos:?} socket priority: {e}"));
    }
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
    let buffer = match buffer_size {
        0 => None,
        asked => match granted_receive_buffer(&socket) {
            Ok(granted) => Some(ReceiveBuffer { asked, granted }),
            Err(e) => {
                warn(&format!("Could not read the {qos:?} receive buffer size back: {e}"));
                None
            }
        },
    };
    let socket: UdpSocket = socket.into();
    socket.set_read_timeout(Some(UDP_RECV_POLL_TIMEOUT))?;
    Ok((socket, buffer))
}

/// What a failed receive means.
enum Failure {
    /// The poll timeout.
    Timeout,
    /// An ICMP port-unreachable report (a ping sent before the host bound its port): receive
    /// again, as recvUdpSocket does.
    Retry,
    Fatal(io::Error),
}

fn failure(e: io::Error) -> Failure {
    match e.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted => {
            Failure::Timeout
        }
        io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionRefused => Failure::Retry,
        // WSA_IO_PENDING, which some Windows systems return instead of a timeout.
        _ if cfg!(windows) && e.raw_os_error() == Some(997) => Failure::Timeout,
        _ => Failure::Fatal(e),
    }
}

/// One receive: `Ok(None)` on the poll timeout. ICMP port-unreachable reports (a ping sent
/// before the host bound its port) are skipped, as in recvUdpSocket.
pub fn receive(socket: &UdpSocket, buffer: &mut [u8]) -> io::Result<Option<usize>> {
    loop {
        match socket.recv(buffer).map_err(failure) {
            Ok(n) => return Ok(Some(n)),
            Err(Failure::Timeout) => return Ok(None),
            Err(Failure::Retry) => {}
            Err(Failure::Fatal(e)) => return Err(e),
        }
    }
}

/// Datagrams one [`BatchReceiver::receive`] takes at most.
pub const RECEIVE_BATCH: usize = 32;
/// Large enough that no datagram is truncated: Windows fails a truncating receive. It holds a
/// coalesced read too, which is at most `uro::MAX_COALESCED` bytes.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
const WHOLE_DATAGRAM: usize = 65536;

/// Receives every datagram already queued on a media socket with one system call, so a high
/// packet rate does not pay a call per datagram: `recvmmsg` with `MSG_WAITFORONE` on Linux and
/// Android, which waits (up to the socket's poll timeout) for the first datagram only and
/// returns at once with whatever else is queued. On Windows, UDP receive coalescing
/// (`UDP_RECV_MAX_COALESCED_SIZE`): the stack hands over consecutive datagrams of one sender
/// in one buffer, which `WSARecvMsg` reads with their size (`UDP_COALESCED_INFO`), and the
/// receiver splits it again. Where the kernel refuses `recvmmsg` or the system refuses
/// coalescing (older Windows versions), one datagram per receive as [`receive`]; where the
/// adapter does not coalesce, each read is one datagram all the same. (Windows' Registered I/O takes batches too, but its sockets
/// refuse the plain sends and the clone the video socket needs.)
pub struct BatchReceiver {
    /// Bytes per datagram slot of `buffer`.
    slot: usize,
    buffer: Vec<u8>,
    /// The length of each datagram of the last receive on the wire, which may exceed `slot`.
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    lengths: [usize; RECEIVE_BATCH],
    /// Where the last receive's first datagram starts in `buffer`, and the distance from each
    /// datagram to the next: the slot, or a coalesced read's datagram size.
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    base: usize,
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    stride: usize,
    /// Linux and Android: the datagrams of the last receive, each a span of `buffer` (see
    /// [`push_segments`]). A message of UDP_GRO segments is several of them.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    spans: Vec<Span>,
    /// Linux and Android: the most bytes kept of each datagram, the reader's maximum with
    /// UDP_GRO and the slot otherwise.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    cut: usize,
    /// Linux and Android: whether UDP_GRO is on for the socket.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    gro: bool,
    /// Linux and Android: why UDP_GRO was refused, until the caller takes it.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    gro_refused: Option<io::Error>,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    batch: Option<mmsg::Batch>,
    #[cfg(windows)]
    coalesce: Option<uro::Coalesce>,
    /// The datagrams of the last coalesced read that the receives so far have not taken.
    #[cfg(windows)]
    pending: Segments,
    /// Windows: the reader's own maximum. A coalesced read that its size leaves with a
    /// datagram longer than this came from an adapter that lost the size (see
    /// [`BatchReceiver::receive_coalesced`]).
    #[cfg(windows)]
    max_datagram: usize,
    /// Why batching stopped, until the caller takes it.
    fallback: Option<io::Error>,
}

impl BatchReceiver {
    /// A receiver for `socket` and the sockets that replace it. With `recvmmsg` batches, a
    /// datagram longer than `max_datagram` (the reader's own maximum) is cut to it; its length
    /// still counts in full. Otherwise, coalesced or not, datagrams are read whole.
    /// `coalescing` is the setting for the stack's coalescing (UDP_GRO on Linux and Android,
    /// receive coalescing on Windows); off, Linux and Android still take `recvmmsg` batches.
    pub fn new(socket: &UdpSocket, max_datagram: usize, coalescing: bool) -> Self {
        Self::with_batches(socket, max_datagram, true, coalescing)
    }

    /// One datagram per receive, as where the kernel refuses batches.
    #[cfg(test)]
    pub fn unbatched(socket: &UdpSocket, max_datagram: usize) -> Self {
        Self::with_batches(socket, max_datagram, false, false)
    }

    fn with_batches(
        socket: &UdpSocket,
        max_datagram: usize,
        batches: bool,
        coalescing: bool,
    ) -> Self {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            let (probed, fallback) = match mmsg::probe(socket) {
                Ok(()) => (true, None),
                Err(e) => (false, Some(e)),
            };
            // A coalesced message is split only by batches, so UDP_GRO is asked for with them.
            let (coalescing, refused) = if probed && batches && coalescing {
                match gro::set(socket, true) {
                    Ok(()) => (true, None),
                    Err(e) => (false, Some(e)),
                }
            } else {
                (false, None)
            };
            let slot = if coalescing {
                gro::SLOT
            } else {
                max_datagram.max(1).next_multiple_of(64)
            };
            let mut buffer = vec![0u8; slot * RECEIVE_BATCH];
            // Points into `buffer`'s allocation, which is never resized and outlives it here.
            let batch = mmsg::Batch::new(&mut buffer, slot);
            Self {
                slot,
                buffer,
                spans: Vec::with_capacity(RECEIVE_BATCH),
                cut: if coalescing {
                    max_datagram.max(1)
                } else {
                    slot
                },
                gro: coalescing,
                gro_refused: refused,
                batch: (probed && batches).then_some(batch),
                fallback,
            }
        }
        #[cfg(windows)]
        {
            // Batches on Windows are coalesced reads, so they come only with the setting.
            let (coalesce, fallback) = if batches && coalescing {
                match uro::Coalesce::new(socket, max_datagram) {
                    Ok(coalesce) => (Some(coalesce), None),
                    Err(e) => (None, Some(e)),
                }
            } else {
                (None, None)
            };
            Self {
                slot: WHOLE_DATAGRAM,
                buffer: vec![0u8; WHOLE_DATAGRAM],
                lengths: [0; RECEIVE_BATCH],
                base: 0,
                stride: WHOLE_DATAGRAM,
                coalesce,
                pending: Segments::default(),
                max_datagram: max_datagram.max(1),
                fallback,
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "android", windows)))]
        {
            let _ = (socket, max_datagram, batches, coalescing);
            Self {
                slot: WHOLE_DATAGRAM,
                buffer: vec![0u8; WHOLE_DATAGRAM],
                lengths: [0; RECEIVE_BATCH],
                base: 0,
                stride: WHOLE_DATAGRAM,
                fallback: None,
            }
        }
    }

    /// Whether receives take batches: `recvmmsg`, or coalesced reads on Windows.
    pub fn batched(&self) -> bool {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        return self.batch.is_some();
        #[cfg(windows)]
        return self.coalesce.as_ref().is_some_and(uro::Coalesce::on);
        #[cfg(not(any(target_os = "linux", target_os = "android", windows)))]
        false
    }

    /// Why receives stopped taking batches, once: the kernel refused `recvmmsg`, or the system
    /// refused coalescing on the socket or the roam's new one.
    pub fn take_fallback(&mut self) -> Option<io::Error> {
        self.fallback.take()
    }

    /// Waits up to the socket's poll timeout for a datagram and takes it with every other one
    /// already queued, up to [`RECEIVE_BATCH`]: their count, 0 on the timeout. ICMP
    /// port-unreachable reports are skipped as in [`receive`].
    pub fn receive(&mut self, socket: &UdpSocket) -> io::Result<usize> {
        #[cfg(windows)]
        if self.coalesce.is_some() || self.pending.left() > 0 {
            if let Some(count) = self.receive_coalesced(socket)? {
                return Ok(count);
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        {
            self.base = 0;
            self.stride = self.slot;
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        while let Some(batch) = self.batch.as_mut() {
            match batch.receive(socket) {
                Ok(count) => {
                    self.spans.clear();
                    for i in 0..count {
                        let segment = batch.gro(i);
                        let total = batch.length(i);
                        push_segments(
                            &mut self.spans,
                            i * self.slot,
                            self.slot,
                            self.cut,
                            total,
                            segment,
                        );
                    }
                    return Ok(self.spans.len());
                }
                Err(e) if mmsg::unsupported(&e) => {
                    self.batch = None;
                    self.fallback = Some(e);
                    if self.gro {
                        // One-datagram reads cannot split a coalesced read, so UDP_GRO goes off.
                        let _ = gro::set(socket, false);
                        self.gro = false;
                        self.cut = self.slot;
                    }
                }
                Err(e) => match failure(e) {
                    Failure::Timeout => return Ok(0),
                    Failure::Retry => {}
                    Failure::Fatal(e) => return Err(e),
                },
            }
        }
        loop {
            match self.receive_one(socket).map_err(failure) {
                Ok(length) => {
                    #[cfg(any(target_os = "linux", target_os = "android"))]
                    {
                        self.spans.clear();
                        push_segments(&mut self.spans, 0, self.slot, self.cut, length, None);
                        return Ok(self.spans.len());
                    }
                    #[cfg(not(any(target_os = "linux", target_os = "android")))]
                    {
                        self.lengths[0] = length;
                        return Ok(1);
                    }
                }
                Err(Failure::Timeout) => return Ok(0),
                Err(Failure::Retry) => {}
                Err(Failure::Fatal(e)) => return Err(e),
            }
        }
    }

    /// One datagram into the first slot: its length on the wire.
    fn receive_one(&mut self, socket: &UdpSocket) -> io::Result<usize> {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        return mmsg::receive_one(socket, &mut self.buffer[..self.slot]);
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        socket.recv(&mut self.buffer[..self.slot])
    }

    /// The next datagrams of a coalesced read: those the last receive left, or a new read's.
    /// None when the roam's new socket refused coalescing, which leaves the receive to the
    /// one-datagram path from now on.
    ///
    /// A read that its size (`UDP_COALESCED_INFO`) leaves with a datagram longer than the
    /// reader's maximum held several datagrams whose size the stack did not report: some
    /// network adapters' receive segment coalescing (RSC) loses it (quinn-rs/quinn#2041).
    /// Split by guesswork, a read could come apart wrongly, so it is passed on as before (the
    /// video receiver keeps its first datagram's worth), and coalescing goes off on the socket
    /// for the rest of the stream; the reads already queued are still read with their size.
    #[cfg(windows)]
    fn receive_coalesced(&mut self, socket: &UdpSocket) -> io::Result<Option<usize>> {
        if self.pending.left() == 0 {
            let Some(coalesce) = self.coalesce.as_mut() else {
                return Ok(None);
            };
            if let Err(e) = coalesce.follow(socket) {
                self.coalesce = None;
                self.fallback = Some(e);
                return Ok(None);
            }
            loop {
                match coalesce.receive(socket, &mut self.buffer).map_err(failure) {
                    Ok((total, segment)) => {
                        self.pending = Segments::new(total, segment);
                        if coalesce.on() && self.pending.segment() > self.max_datagram {
                            self.fallback =
                                Some(unsized_read(coalesce.turn_off(), total, segment, self.max_datagram));
                        }
                        break;
                    }
                    Err(Failure::Timeout) => return Ok(Some(0)),
                    Err(Failure::Retry) => {}
                    Err(Failure::Fatal(e)) => return Err(e),
                }
            }
        }
        let (base, count) = self.pending.take(&mut self.lengths);
        self.base = base;
        self.stride = self.pending.segment();
        Ok(Some(count))
    }

    /// Datagram `index` of the last receive, cut to the slot.
    pub fn datagram(&self, index: usize) -> &[u8] {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        return {
            let span = self.spans[index];
            &self.buffer[span.start..span.start + span.kept]
        };
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        {
            let start = self.base + index * self.stride;
            &self.buffer[start..start + self.lengths[index].min(self.stride)]
        }
    }

    /// The length on the wire of datagram `index` of the last receive.
    pub fn length(&self, index: usize) -> usize {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        return self.spans[index].wire;
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        self.lengths[index]
    }

    /// Whether receives take UDP_GRO coalesced reads: Linux and Android, with batches.
    pub fn gro(&self) -> bool {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        return self.gro;
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        false
    }

    /// Why UDP_GRO was refused for the socket, once: receives then take one datagram per read
    /// as before.
    pub fn take_gro_refusal(&mut self) -> Option<io::Error> {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        return self.gro_refused.take();
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        None
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod mmsg {
    use std::io;
    use std::net::UdpSocket;
    use std::os::fd::AsRawFd;

    use super::RECEIVE_BATCH;

    /// Control bytes per header: room for a UDP_GRO message (a `cmsghdr` and an int, 24 bytes on
    /// 64-bit Linux).
    const CONTROL: usize = 64;

    /// `recvmmsg` headers, one per slot of a buffer they point into, each with its control
    /// messages.
    pub struct Batch {
        headers: Box<[libc::mmsghdr]>,
        _iovecs: Box<[libc::iovec]>,
        controls: Box<[u8]>,
    }

    #[cfg(test)]
    thread_local! {
        /// Makes this thread's `recvmmsg` calls fail with `ENOSYS`, as a kernel without it.
        pub static REFUSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    #[cfg(test)]
    fn refused() -> io::Result<()> {
        if REFUSED.get() {
            return Err(io::Error::from_raw_os_error(libc::ENOSYS));
        }
        Ok(())
    }

    impl Batch {
        /// The headers for `buffer`'s slots of `slot` bytes. They hold pointers into it: the
        /// caller keeps it alive and its allocation in place while they are used.
        pub fn new(buffer: &mut [u8], slot: usize) -> Self {
            let mut iovecs: Box<[libc::iovec]> = buffer
                .chunks_exact_mut(slot)
                .take(RECEIVE_BATCH)
                .map(|slot| libc::iovec {
                    iov_base: slot.as_mut_ptr().cast(),
                    iov_len: slot.len(),
                })
                .collect();
            let mut controls = vec![0u8; CONTROL * iovecs.len()].into_boxed_slice();
            let control_base = controls.as_mut_ptr();
            let headers = iovecs
                .iter_mut()
                .enumerate()
                .map(|(index, iovec)| {
                    // SAFETY: mmsghdr is plain data; all zero is an empty header.
                    let mut header: libc::mmsghdr = unsafe { std::mem::zeroed() };
                    header.msg_hdr.msg_iov = iovec;
                    header.msg_hdr.msg_iovlen = 1;
                    header.msg_hdr.msg_control = control_base.wrapping_add(index * CONTROL).cast();
                    header.msg_hdr.msg_controllen = CONTROL as _;
                    header
                })
                .collect();
            Self {
                headers,
                _iovecs: iovecs,
                controls,
            }
        }

        /// Up to a batch: blocks for the first datagram only (`MSG_WAITFORONE`). `MSG_TRUNC`
        /// reports a cut datagram's length on the wire.
        pub fn receive(&mut self, socket: &UdpSocket) -> io::Result<usize> {
            #[cfg(test)]
            refused()?;
            for header in self.headers.iter_mut() {
                // The kernel leaves the length of the control messages it wrote; each call
                // needs the whole control buffer again.
                header.msg_hdr.msg_controllen = CONTROL as _;
            }
            // SAFETY: the headers point at live iovecs, slots and control buffers of their lengths.
            let count = unsafe {
                libc::recvmmsg(
                    socket.as_raw_fd(),
                    self.headers.as_mut_ptr(),
                    self.headers.len() as libc::c_uint,
                    libc::MSG_WAITFORONE | libc::MSG_TRUNC,
                    std::ptr::null_mut(),
                )
            };
            if count < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(count as usize)
        }

        pub fn length(&self, index: usize) -> usize {
            self.headers[index].msg_len as usize
        }

        /// The segment size of datagram `index` when UDP_GRO coalesced it, from its control
        /// messages.
        pub fn gro(&self, index: usize) -> Option<usize> {
            // `msg_controllen` is `usize` on glibc and Android but `socklen_t` elsewhere.
            #[allow(clippy::unnecessary_cast)]
            let used = (self.headers[index].msg_hdr.msg_controllen as usize).min(CONTROL);
            let start = index * CONTROL;
            super::cmsg_int(
                &self.controls[start..start + used],
                super::gro::SOL_UDP,
                super::gro::UDP_GRO,
            )
        }
    }

    /// Whether the kernel can take batches: `recvmmsg` of no datagrams returns at once, or
    /// fails where it is missing (before Linux 2.6.33) or filtered out.
    pub fn probe(socket: &UdpSocket) -> io::Result<()> {
        #[cfg(test)]
        refused()?;
        // SAFETY: no headers are read or written with a length of 0.
        let result = unsafe {
            libc::recvmmsg(
                socket.as_raw_fd(),
                std::ptr::null_mut(),
                0,
                libc::MSG_DONTWAIT,
                std::ptr::null_mut(),
            )
        };
        if result < 0 {
            let e = io::Error::last_os_error();
            // Any other error is a pending socket error (an ICMP report): batches work.
            if unsupported(&e) {
                return Err(e);
            }
        }
        Ok(())
    }

    /// An error that means `recvmmsg` itself does not work here.
    pub fn unsupported(e: &io::Error) -> bool {
        matches!(
            e.raw_os_error(),
            Some(libc::ENOSYS | libc::EPERM | libc::EOPNOTSUPP | libc::EINVAL)
        )
    }

    /// One datagram; its length on the wire, even when `buffer` cut it (`MSG_TRUNC`).
    pub fn receive_one(socket: &UdpSocket, buffer: &mut [u8]) -> io::Result<usize> {
        // SAFETY: `buffer` is valid for its length.
        let length = unsafe {
            libc::recv(
                socket.as_raw_fd(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                libc::MSG_TRUNC,
            )
        };
        if length < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(length as usize)
    }
}

/// UDP_GRO, which has the kernel coalesce consecutive datagrams of one sender into one read, with
/// their segment size in a control message, so one batch takes more datagrams per call.
#[cfg(any(target_os = "linux", target_os = "android"))]
mod gro {
    use std::io;
    use std::net::UdpSocket;
    use std::os::fd::AsRawFd;

    /// `SOL_UDP` and `UDP_GRO`, as the kernel defines them.
    pub const SOL_UDP: libc::c_int = 17;
    pub const UDP_GRO: libc::c_int = 104;
    /// Bytes per slot while UDP_GRO is on: the largest read a coalesced message can be.
    pub const SLOT: usize = 65536;

    /// Turns UDP_GRO on or off for `socket`.
    pub fn set(socket: &UdpSocket, on: bool) -> io::Result<()> {
        let value = libc::c_int::from(on);
        // SAFETY: the option is one int, read for its size.
        let result = unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                SOL_UDP,
                UDP_GRO,
                (&value as *const libc::c_int).cast(),
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

/// The datagrams of one coalesced read still to take: `left` datagrams of `segment` bytes back
/// to back from `next`, the last one of what remains up to `end` when the read did not divide
/// evenly.
#[cfg(any(windows, target_os = "linux", target_os = "android", test))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Segments {
    next: usize,
    end: usize,
    segment: usize,
    left: usize,
}

#[cfg(any(windows, target_os = "linux", target_os = "android", test))]
impl Segments {
    /// A read of `total` bytes from the start of the buffer, with the datagram size its
    /// `UDP_COALESCED_INFO` gave (None without one). Without one, or with a size that does not
    /// split the read (0, or the whole read or more), the read is one datagram: what Windows
    /// hands over when it did not coalesce.
    fn new(total: usize, segment: Option<usize>) -> Self {
        let (segment, left) = match segment {
            Some(segment) if segment > 0 && segment < total => (segment, total.div_ceil(segment)),
            _ => (total, 1),
        };
        Self {
            next: 0,
            end: total,
            segment,
            left,
        }
    }

    fn left(&self) -> usize {
        self.left
    }

    // Windows' coalesced reads and the tests use it; Linux reads the GRO size per datagram.
    #[cfg_attr(not(windows), allow(dead_code))]
    fn segment(&self) -> usize {
        self.segment
    }

    /// Takes up to `lengths.len()` datagrams, writing their lengths: where the first starts,
    /// and how many.
    fn take(&mut self, lengths: &mut [usize]) -> (usize, usize) {
        let base = self.next;
        let count = self.left.min(lengths.len());
        for length in &mut lengths[..count] {
            *length = self.segment.min(self.end - self.next);
            self.next += *length;
        }
        self.left -= count;
        (base, count)
    }
}

/// The first int of the control message at `level` and `kind` among the control messages in
/// `control`: a `cmsghdr` (`WSACMSGHDR` on Windows, the same layout) of `cmsg_len`, `cmsg_level`
/// and `cmsg_type`, then the data at the next natural alignment, then padding to the header
/// alignment. None without one, or when the messages run past the buffer.
#[cfg(any(windows, target_os = "linux", target_os = "android", test))]
fn cmsg_int(control: &[u8], level: i32, kind: i32) -> Option<usize> {
    const WORD: usize = std::mem::size_of::<usize>();
    // The header, and the data's alignment after it.
    const HEADER: usize = WORD + 8;
    const DATA: usize = HEADER.next_multiple_of(WORD);
    let word = |at: usize| usize::from_ne_bytes(control[at..at + WORD].try_into().unwrap());
    let int = |at: usize| i32::from_ne_bytes(control[at..at + 4].try_into().unwrap());
    let mut at = 0;
    while at + HEADER <= control.len() {
        let length = word(at);
        if length < HEADER || length > control.len() - at {
            return None;
        }
        if int(at + WORD) == level && int(at + WORD + 4) == kind && length >= DATA + 4 {
            let value = &control[at + DATA..at + DATA + 4];
            return Some(u32::from_ne_bytes(value.try_into().unwrap()) as usize);
        }
        // The next header, at the header alignment.
        at += length.next_multiple_of(WORD);
    }
    None
}

/// The datagram size of a coalesced read: the `UDP_COALESCED_INFO` message among the `WSARecvMsg`
/// control messages in `control` (see [`cmsg_int`]).
#[cfg(any(windows, test))]
fn coalesced_segment(control: &[u8]) -> Option<usize> {
    const IPPROTO_UDP: i32 = 17;
    const UDP_COALESCED_INFO: i32 = 3;
    cmsg_int(control, IPPROTO_UDP, UDP_COALESCED_INFO)
}

/// One datagram of a Linux or Android receive: where it starts in the buffer, its length on the
/// wire, and the bytes of it kept there.
#[cfg(any(target_os = "linux", target_os = "android", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Span {
    start: usize,
    wire: usize,
    kept: usize,
}

/// Appends the datagrams of one received message to `spans`. The message starts at `slot_start`
/// in a slot of `slot` bytes, is `total` bytes on the wire, and UDP_GRO coalesced it into
/// `segment`-byte datagrams (the last may be shorter) when that is Some; otherwise it is one
/// datagram. Each keeps up to `cut` bytes, and no bytes past the slot.
#[cfg(any(target_os = "linux", target_os = "android", test))]
fn push_segments(
    spans: &mut Vec<Span>,
    slot_start: usize,
    slot: usize,
    cut: usize,
    total: usize,
    segment: Option<usize>,
) {
    let slot_end = slot_start + slot;
    let mut segments = Segments::new(total, segment);
    let mut lengths = [0; 64];
    while segments.left() > 0 {
        let (base, count) = segments.take(&mut lengths);
        let mut start = slot_start + base;
        for &wire in &lengths[..count] {
            let kept = wire.min(cut).min(slot_end.saturating_sub(start));
            spans.push(Span { start, wire, kept });
            start += wire;
        }
    }
}

/// Why receive coalescing went off for the stream: a read of `total` bytes whose size
/// (`segment`, from `UDP_COALESCED_INFO`) left a datagram longer than `max_datagram`, and
/// whether turning it off worked (`turned_off`).
#[cfg(any(windows, test))]
fn unsized_read(
    turned_off: io::Result<()>,
    total: usize,
    segment: Option<usize>,
    max_datagram: usize,
) -> io::Error {
    let size = match segment {
        Some(segment) => format!("a datagram size of {segment}"),
        None => "no datagram size".to_owned(),
    };
    let read = format!(
        "a {total}-byte read of datagrams of at most {max_datagram} bytes came with {size}: \
         this network adapter loses the size of coalesced reads"
    );
    match turned_off {
        Ok(()) => io::Error::other(format!("receive coalescing is off for this stream, {read}")),
        Err(e) => io::Error::new(
            e.kind(),
            format!("turning receive coalescing off failed ({e}), {read}"),
        ),
    }
}

/// UDP receive coalescing on Windows.
#[cfg(windows)]
mod uro {
    use std::io;
    use std::mem::{size_of, size_of_val};
    use std::net::UdpSocket;
    use std::os::windows::io::AsRawSocket;
    use std::ptr::null_mut;

    use windows_sys::core::GUID;
    use windows_sys::Win32::Networking::WinSock::{
        setsockopt, WSAGetLastError, WSAIoctl, CMSGHDR, IPPROTO_UDP, LPFN_WSARECVMSG,
        SIO_GET_EXTENSION_FUNCTION_POINTER, SOCKET, SOCKET_ERROR, UDP_COALESCED_INFO,
        UDP_RECV_MAX_COALESCED_SIZE, WSABUF, WSAID_WSARECVMSG, WSAMSG,
    };

    use super::RECEIVE_BATCH;

    /// The most a coalesced read holds: the largest UDP payload over IPv4, as msquic sets it.
    pub const MAX_COALESCED: usize = 65527;

    // The layout `coalesced_segment` reads.
    const _: () = assert!(
        IPPROTO_UDP == 17
            && UDP_COALESCED_INFO == 3
            && size_of::<CMSGHDR>() == size_of::<usize>() + 8
    );

    #[cfg(test)]
    thread_local! {
        /// Makes this thread's sockets refuse coalescing, as a system without it.
        pub static REFUSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    fn last_error() -> io::Error {
        // SAFETY: no preconditions.
        io::Error::from_raw_os_error(unsafe { WSAGetLastError() })
    }

    fn raw(socket: &UdpSocket) -> SOCKET {
        socket.as_raw_socket() as SOCKET
    }

    /// A socket's `WSARecvMsg` and the coalescing turned on, for the receiver's socket and
    /// the roam's replacements.
    pub struct Coalesce {
        recv_msg: LPFN_WSARECVMSG,
        /// The socket the reads are on.
        socket: SOCKET,
        /// The option's value: a call's worth of the reader's datagrams.
        size: u32,
        /// Whether coalescing is on: until [`Coalesce::turn_off`], which also keeps it off on a
        /// roam's new socket.
        on: bool,
        /// The control messages of a read: one `UDP_COALESCED_INFO` takes 24 bytes.
        control: [u64; 16],
    }

    impl Coalesce {
        /// Coalescing on `socket` up to [`RECEIVE_BATCH`] datagrams of `max_datagram` bytes
        /// (more of smaller ones, which the receiver takes over several receives), or why the
        /// system refused it.
        pub fn new(socket: &UdpSocket, max_datagram: usize) -> io::Result<Self> {
            let size = (max_datagram.max(1) * RECEIVE_BATCH).min(MAX_COALESCED) as u32;
            let recv_msg = enable(raw(socket), Some(size))?;
            Ok(Self {
                recv_msg,
                socket: raw(socket),
                size,
                on: true,
                control: [0; 16],
            })
        }

        /// Whether coalescing is on.
        pub fn on(&self) -> bool {
            self.on
        }

        /// Moves the reads to `socket` when it is not the one they are on (a roam's new
        /// socket), with coalescing on there while it is on, or says why the system refused it.
        pub fn follow(&mut self, socket: &UdpSocket) -> io::Result<()> {
            if raw(socket) != self.socket {
                self.recv_msg = enable(raw(socket), self.on.then_some(self.size))?;
                self.socket = raw(socket);
            }
            Ok(())
        }

        /// Turns coalescing off on the socket and for the roam's next ones. The reads go on
        /// through `WSARecvMsg`: those coalesced before still come with their size.
        pub fn turn_off(&mut self) -> io::Result<()> {
            self.on = false;
            set_size(self.socket, 0)
        }

        /// One read into `buffer`: its bytes and the datagram size of a coalesced read. The
        /// socket's poll timeout applies as to `recv`. The sender's address is not asked for:
        /// as with `recv` and `recvmmsg`, the unconnected socket takes any.
        pub fn receive(
            &mut self,
            socket: &UdpSocket,
            buffer: &mut [u8],
        ) -> io::Result<(usize, Option<usize>)> {
            let Some(recv_msg) = self.recv_msg else {
                return Err(io::ErrorKind::Unsupported.into());
            };
            let mut data = WSABUF {
                len: buffer.len().min(u32::MAX as usize) as u32,
                buf: buffer.as_mut_ptr(),
            };
            let mut message = WSAMSG {
                name: null_mut(),
                namelen: 0,
                lpBuffers: &mut data,
                dwBufferCount: 1,
                Control: WSABUF {
                    len: size_of_val(&self.control) as u32,
                    buf: self.control.as_mut_ptr().cast(),
                },
                dwFlags: 0,
            };
            let mut received = 0u32;
            // SAFETY: the message points at `buffer` and `control` for their lengths, both
            // live across the call; no overlapped I/O.
            let result =
                unsafe { recv_msg(raw(socket), &mut message, &mut received, null_mut(), None) };
            if result == SOCKET_ERROR {
                return Err(last_error());
            }
            // On return Control.len is the length of the control messages.
            let length = (message.Control.len as usize).min(size_of_val(&self.control));
            // SAFETY: plain integers seen as their bytes, within the array.
            let control =
                unsafe { std::slice::from_raw_parts(self.control.as_ptr().cast::<u8>(), length) };
            Ok((received as usize, super::coalesced_segment(control)))
        }
    }

    /// `socket`'s `WSARecvMsg`, then coalescing up to `size` bytes on it when that is Some: in
    /// this order, so a socket without the function never coalesces.
    fn enable(socket: SOCKET, size: Option<u32>) -> io::Result<LPFN_WSARECVMSG> {
        #[cfg(test)]
        if REFUSED.get() {
            return Err(io::Error::from_raw_os_error(
                windows_sys::Win32::Networking::WinSock::WSAENOPROTOOPT,
            ));
        }
        let mut recv_msg: LPFN_WSARECVMSG = None;
        let mut returned = 0u32;
        // SAFETY: the in and out buffers are valid for the sizes given; no overlapped I/O.
        let result = unsafe {
            WSAIoctl(
                socket,
                SIO_GET_EXTENSION_FUNCTION_POINTER,
                (&WSAID_WSARECVMSG as *const GUID).cast(),
                size_of::<GUID>() as u32,
                (&mut recv_msg as *mut LPFN_WSARECVMSG).cast(),
                size_of::<LPFN_WSARECVMSG>() as u32,
                &mut returned,
                null_mut(),
                None,
            )
        };
        if result == SOCKET_ERROR {
            return Err(last_error());
        }
        if recv_msg.is_none() {
            return Err(io::Error::new(io::ErrorKind::Unsupported, "no WSARecvMsg"));
        }
        if let Some(size) = size {
            set_size(socket, size)?;
        }
        Ok(recv_msg)
    }

    /// The coalesced reads' maximum on `socket`: 0 turns coalescing off.
    fn set_size(socket: SOCKET, size: u32) -> io::Result<()> {
        // SAFETY: a DWORD option of its size.
        let result = unsafe {
            setsockopt(
                socket,
                IPPROTO_UDP,
                UDP_RECV_MAX_COALESCED_SIZE,
                (&size as *const u32).cast(),
                size_of::<u32>() as i32,
            )
        };
        if result == SOCKET_ERROR {
            return Err(last_error());
        }
        Ok(())
    }

    /// The socket's coalesced reads' maximum, 0 while coalescing is off.
    #[cfg(test)]
    pub fn size_on(socket: SOCKET) -> io::Result<u32> {
        use windows_sys::Win32::Networking::WinSock::getsockopt;
        let mut size = 0u32;
        let mut length = size_of::<u32>() as i32;
        // SAFETY: a DWORD option into a DWORD of its size.
        let result = unsafe {
            getsockopt(
                socket,
                IPPROTO_UDP,
                UDP_RECV_MAX_COALESCED_SIZE,
                (&mut size as *mut u32).cast(),
                &mut length,
            )
        };
        if result == SOCKET_ERROR {
            return Err(last_error());
        }
        Ok(size)
    }

    #[cfg(test)]
    pub fn socket_of(coalesce: &Coalesce) -> SOCKET {
        coalesce.socket
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

/// The socket a receive thread moves to when the session rebinds its media for a roam. The
/// thread looks only when a receive times out or fails, so the packet path is unchanged: it
/// reads the old socket until the host stops sending there.
#[derive(Default)]
pub struct NextSocket {
    socket: Mutex<Option<UdpSocket>>,
    set: Condvar,
}

impl NextSocket {
    pub fn set(&self, socket: UdpSocket) {
        *self.socket.lock().unwrap_or_else(|p| p.into_inner()) = Some(socket);
        self.set.notify_all();
    }

    pub fn take(&self) -> Option<UdpSocket> {
        self.socket.lock().unwrap_or_else(|p| p.into_inner()).take()
    }

    /// The next socket once it is set, waiting up to `timeout` for it: a receive that failed
    /// with the network may be ahead of the rebind. None after `timeout`, or soon after `stop`.
    pub fn wait(&self, timeout: Duration, stop: &AtomicBool) -> Option<UdpSocket> {
        let deadline = Instant::now() + timeout;
        let mut socket = self.socket.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if let Some(next) = socket.take() {
                return Some(next);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || stop.load(Ordering::Acquire) {
                return None;
            }
            socket = self
                .set
                .wait_timeout(socket, left.min(UDP_RECV_POLL_TIMEOUT))
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
    }
}

#[derive(Default)]
struct PingState {
    stopped: bool,
    /// A socket to ping from now on ([`PingHandle::rebind`]).
    next: Option<UdpSocket>,
    /// Ping without waiting out the interval ([`PingHandle::ping_now`]).
    now: bool,
}

#[derive(Default)]
struct PingControl {
    state: Mutex<PingState>,
    wake: Condvar,
}

/// Pings `target` from `socket` every 500 ms until stopped. Send errors are ignored: the
/// receive side handles socket failures.
pub struct Pinger {
    control: Arc<PingControl>,
    thread: Option<JoinHandle<()>>,
}

/// Moves a running [`Pinger`] to another socket.
#[derive(Clone)]
pub struct PingHandle(Arc<PingControl>);

impl PingHandle {
    /// Pings from `socket` at once and from then on; the sequence carries on.
    pub fn rebind(&self, socket: UdpSocket) {
        self.0.state.lock().unwrap_or_else(|p| p.into_inner()).next = Some(socket);
        self.0.wake.notify_all();
    }

    /// Pings at once, then every interval from there; the sequence carries on.
    pub fn ping_now(&self) {
        self.0.state.lock().unwrap_or_else(|p| p.into_inner()).now = true;
        self.0.wake.notify_all();
    }
}

impl Pinger {
    pub fn start(
        name: &str,
        socket: &UdpSocket,
        target: SocketAddr,
        payload: Option<[u8; 16]>,
    ) -> io::Result<Self> {
        let mut socket = socket.try_clone()?;
        let control = Arc::new(PingControl::default());
        let signal = control.clone();
        let thread = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let mut count = 0u32;
                loop {
                    count = count.wrapping_add(1);
                    let _ = socket.send_to(&ping_datagram(payload, count), target);
                    let state = signal.state.lock().unwrap_or_else(|p| p.into_inner());
                    let (mut state, _) = signal
                        .wake
                        .wait_timeout_while(state, PING_INTERVAL, |s| {
                            !s.stopped && s.next.is_none() && !s.now
                        })
                        .unwrap_or_else(|p| p.into_inner());
                    if state.stopped {
                        break;
                    }
                    state.now = false;
                    if let Some(next) = state.next.take() {
                        socket = next;
                    }
                }
            })?;
        Ok(Self {
            control,
            thread: Some(thread),
        })
    }

    pub fn handle(&self) -> PingHandle {
        PingHandle(self.control.clone())
    }

    pub fn stop(&mut self) {
        self.control
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .stopped = true;
        self.control.wake.notify_all();
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

#[cfg(all(test, any(target_os = "linux", target_os = "android", windows)))]
mod receive_bench;

#[cfg(test)]
mod tests;
