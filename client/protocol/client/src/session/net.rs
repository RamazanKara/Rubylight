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
    lengths: [usize; RECEIVE_BATCH],
    /// Where the last receive's first datagram starts in `buffer`, and the distance from each
    /// datagram to the next: the slot, or a coalesced read's datagram size.
    base: usize,
    stride: usize,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    batch: Option<mmsg::Batch>,
    #[cfg(windows)]
    coalesce: Option<uro::Coalesce>,
    /// The datagrams of the last coalesced read that the receives so far have not taken.
    #[cfg(windows)]
    pending: Segments,
    /// Why batching stopped, until the caller takes it.
    fallback: Option<io::Error>,
}

impl BatchReceiver {
    /// A receiver for `socket` and the sockets that replace it. With `recvmmsg` batches, a
    /// datagram longer than `max_datagram` (the reader's own maximum) is cut to it; its length
    /// still counts in full. Otherwise, coalesced or not, datagrams are read whole.
    pub fn new(socket: &UdpSocket, max_datagram: usize) -> Self {
        Self::with_batches(socket, max_datagram, true)
    }

    /// One datagram per receive, as where the kernel refuses batches.
    #[cfg(test)]
    pub fn unbatched(socket: &UdpSocket, max_datagram: usize) -> Self {
        Self::with_batches(socket, max_datagram, false)
    }

    fn with_batches(socket: &UdpSocket, max_datagram: usize, batches: bool) -> Self {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            let slot = max_datagram.max(1).next_multiple_of(64);
            let mut buffer = vec![0u8; slot * RECEIVE_BATCH];
            // Points into `buffer`'s allocation, which is never resized and outlives it here.
            let batch = mmsg::Batch::new(&mut buffer, slot);
            let (batch, fallback) = match mmsg::probe(socket) {
                Ok(()) => (batches.then_some(batch), None),
                Err(e) => (None, Some(e)),
            };
            Self {
                slot,
                buffer,
                lengths: [0; RECEIVE_BATCH],
                base: 0,
                stride: slot,
                batch,
                fallback,
            }
        }
        #[cfg(windows)]
        {
            let (coalesce, fallback) = if batches {
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
                fallback,
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "android", windows)))]
        {
            let _ = (socket, max_datagram, batches);
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
        return self.coalesce.is_some();
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
        self.base = 0;
        self.stride = self.slot;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        while let Some(batch) = self.batch.as_mut() {
            match batch.receive(socket) {
                Ok(count) => {
                    for (i, length) in self.lengths[..count].iter_mut().enumerate() {
                        *length = batch.length(i);
                    }
                    return Ok(count);
                }
                Err(e) if mmsg::unsupported(&e) => {
                    self.batch = None;
                    self.fallback = Some(e);
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
                    self.lengths[0] = length;
                    return Ok(1);
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
        let start = self.base + index * self.stride;
        &self.buffer[start..start + self.lengths[index].min(self.stride)]
    }

    /// The length on the wire of datagram `index` of the last receive.
    pub fn length(&self, index: usize) -> usize {
        self.lengths[index]
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod mmsg {
    use std::io;
    use std::net::UdpSocket;
    use std::os::fd::AsRawFd;

    use super::RECEIVE_BATCH;

    /// `recvmmsg` headers, one per slot of a buffer they point into.
    pub struct Batch {
        headers: Box<[libc::mmsghdr]>,
        _iovecs: Box<[libc::iovec]>,
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
            let headers = iovecs
                .iter_mut()
                .map(|iovec| {
                    // SAFETY: mmsghdr is plain data; all zero is an empty header.
                    let mut header: libc::mmsghdr = unsafe { std::mem::zeroed() };
                    header.msg_hdr.msg_iov = iovec;
                    header.msg_hdr.msg_iovlen = 1;
                    header
                })
                .collect();
            Self {
                headers,
                _iovecs: iovecs,
            }
        }

        /// Up to a batch: blocks for the first datagram only (`MSG_WAITFORONE`). `MSG_TRUNC`
        /// reports a cut datagram's length on the wire.
        pub fn receive(&mut self, socket: &UdpSocket) -> io::Result<usize> {
            #[cfg(test)]
            refused()?;
            // SAFETY: the headers point at live iovecs and slots of their lengths.
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

/// The datagrams of one coalesced read still to take: `left` datagrams of `segment` bytes back
/// to back from `next`, the last one of what remains up to `end` when the read did not divide
/// evenly.
#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Segments {
    next: usize,
    end: usize,
    segment: usize,
    left: usize,
}

#[cfg(any(windows, test))]
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

/// The datagram size of a coalesced read: the `UDP_COALESCED_INFO` message among the
/// `WSARecvMsg` control messages in `control` (a `WSACMSGHDR` of `cmsg_len`, `cmsg_level` and
/// `cmsg_type`, then the data at the next natural alignment). None without one, or when the
/// messages run past the buffer.
#[cfg(any(windows, test))]
fn coalesced_segment(control: &[u8]) -> Option<usize> {
    const WORD: usize = std::mem::size_of::<usize>();
    // WSACMSGHDR, and WSA_CMSGDATA_ALIGN of it.
    const HEADER: usize = WORD + 8;
    const DATA: usize = HEADER.next_multiple_of(WORD);
    const IPPROTO_UDP: i32 = 17;
    const UDP_COALESCED_INFO: i32 = 3;
    let word = |at: usize| usize::from_ne_bytes(control[at..at + WORD].try_into().unwrap());
    let int = |at: usize| i32::from_ne_bytes(control[at..at + 4].try_into().unwrap());
    let mut at = 0;
    while at + HEADER <= control.len() {
        let length = word(at);
        if length < HEADER || at + length > control.len() {
            return None;
        }
        if int(at + WORD) == IPPROTO_UDP
            && int(at + WORD + 4) == UDP_COALESCED_INFO
            && length >= DATA + 4
        {
            let size = &control[at + DATA..at + DATA + 4];
            return Some(u32::from_ne_bytes(size.try_into().unwrap()) as usize);
        }
        // WSA_CMSG_NXTHDR: the next header at the header alignment.
        at += length.next_multiple_of(WORD);
    }
    None
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
        /// The socket coalescing is on for.
        socket: SOCKET,
        /// The option's value: a call's worth of the reader's datagrams.
        size: u32,
        /// The control messages of a read: one `UDP_COALESCED_INFO` takes 24 bytes.
        control: [u64; 16],
    }

    impl Coalesce {
        /// Coalescing on `socket` up to [`RECEIVE_BATCH`] datagrams of `max_datagram` bytes
        /// (more of smaller ones, which the receiver takes over several receives), or why the
        /// system refused it.
        pub fn new(socket: &UdpSocket, max_datagram: usize) -> io::Result<Self> {
            let size = (max_datagram.max(1) * RECEIVE_BATCH).min(MAX_COALESCED) as u32;
            let recv_msg = enable(raw(socket), size)?;
            Ok(Self {
                recv_msg,
                socket: raw(socket),
                size,
                control: [0; 16],
            })
        }

        /// Turns coalescing on for `socket` when it is not the one it is on for (a roam's new
        /// socket), or says why the system refused it there.
        pub fn follow(&mut self, socket: &UdpSocket) -> io::Result<()> {
            if raw(socket) != self.socket {
                self.recv_msg = enable(raw(socket), self.size)?;
                self.socket = raw(socket);
            }
            Ok(())
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

    /// `socket`'s `WSARecvMsg`, then coalescing up to `size` bytes on it: in this order, so a
    /// socket without the function never coalesces.
    fn enable(socket: SOCKET, size: u32) -> io::Result<LPFN_WSARECVMSG> {
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
        Ok(recv_msg)
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
        use crate::priority::tests::read_priority;
        let bind = |qos, network_priority| {
            bind_udp(
                (Ipv4Addr::LOCALHOST, 0).into(),
                0,
                qos,
                network_priority,
                |_| {},
            )
            .unwrap()
        };
        for network_priority in [false, true] {
            assert_eq!(read_priority(&bind(Qos::Video, network_priority)), 5);
            // The audio class is the microphone's too.
            assert_eq!(read_priority(&bind(Qos::Audio, network_priority)), 6);
        }
        assert_eq!(read_priority(&bind(Qos::BestEffort, false)), 0);
        assert_eq!(read_priority(&bind(Qos::BestEffort, true)), 6);
    }

    // Moonlight V+'s DSCP marking, and the priority it must not reset (IP_TOS does on Linux).
    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn media_sockets_are_marked_and_keep_their_priority() {
        use crate::priority::tests::{read_option, read_priority};
        let bind = |qos| bind_udp((Ipv4Addr::LOCALHOST, 0).into(), 0, qos, false, |_| {}).unwrap();
        let audio = bind(Qos::Audio);
        assert_eq!(read_option(&audio, libc::IPPROTO_IP, libc::IP_TOS), 0xB8);
        assert_eq!(read_priority(&audio), 6);
        let video = bind(Qos::Video);
        assert_eq!(read_option(&video, libc::IPPROTO_IP, libc::IP_TOS), 0x88);
        assert_eq!(read_priority(&video), 5);
        let plain = bind(Qos::BestEffort);
        assert_eq!(read_option(&plain, libc::IPPROTO_IP, libc::IP_TOS), 0);
    }

    #[test]
    fn a_pinger_survives_a_closed_port_and_stops_without_waiting_out_the_interval() {
        // Nothing listens: on loopback the sends draw ICMP port unreachable.
        let target = UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut pinger = Pinger::start("TestPing", &socket, target, None).unwrap();
        std::thread::sleep(PING_INTERVAL + Duration::from_millis(200));
        assert!(!pinger.thread.as_ref().unwrap().is_finished());
        let stopping = std::time::Instant::now();
        pinger.stop();
        assert!(stopping.elapsed() < PING_INTERVAL, "stop waited for the next ping");
        assert!(pinger.thread.is_none());
    }

    #[test]
    fn a_rebound_pinger_pings_from_the_new_socket_at_once_and_counts_on() {
        let host = UdpSocket::bind("127.0.0.1:0").unwrap();
        host.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let old = UdpSocket::bind("127.0.0.1:0").unwrap();
        let new = UdpSocket::bind("127.0.0.1:0").unwrap();
        let payload = Some(*b"0123456789abcdef");
        let mut pinger =
            Pinger::start("TestPing", &old, host.local_addr().unwrap(), payload).unwrap();
        let mut buffer = [0u8; 64];
        let (n, from) = host.recv_from(&mut buffer).unwrap();
        assert_eq!(from, old.local_addr().unwrap());
        assert_eq!(&buffer[..n], ping_datagram(payload, 1));
        let rebinding = std::time::Instant::now();
        pinger.handle().rebind(new.try_clone().unwrap());
        let (n, from) = host.recv_from(&mut buffer).unwrap();
        assert!(
            rebinding.elapsed() < PING_INTERVAL,
            "the rebound pinger waited for its interval"
        );
        assert_eq!(from, new.local_addr().unwrap());
        assert_eq!(&buffer[..n], ping_datagram(payload, 2));
        pinger.stop();
    }

    #[test]
    fn a_failed_receive_waits_a_while_for_the_next_socket_then_gives_up() {
        let stop = AtomicBool::new(false);
        let next = Arc::new(NextSocket::default());
        // The rebind comes after the failure: the wait gets its socket when it is set.
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = socket.local_addr().unwrap();
        let setter = {
            let next = next.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                next.set(socket);
            })
        };
        let waiting = Instant::now();
        let moved = next.wait(REBIND_WAIT, &stop).expect("the socket set meanwhile");
        assert_eq!(moved.local_addr().unwrap(), address);
        assert!(waiting.elapsed() < REBIND_WAIT, "waited out the timeout");
        setter.join().unwrap();
        // No rebind: about REBIND_WAIT, not forever.
        let waiting = Instant::now();
        assert!(next.wait(REBIND_WAIT, &stop).is_none());
        let waited = waiting.elapsed();
        assert!(waited >= REBIND_WAIT && waited < 2 * REBIND_WAIT, "{waited:?}");
        // A stopping session does not wait.
        stop.store(true, Ordering::Release);
        let waiting = Instant::now();
        assert!(next.wait(REBIND_WAIT, &stop).is_none());
        assert!(waiting.elapsed() < UDP_RECV_POLL_TIMEOUT * 2);
    }

    #[test]
    fn a_port_rewrite_keeps_the_ipv6_scope() {
        let scoped = SocketAddr::V6(std::net::SocketAddrV6::new(
            "fe80::1".parse().unwrap(),
            48010,
            0,
            7,
        ));
        let SocketAddr::V6(rewritten) = with_port(scoped, 47999) else {
            panic!("an IPv6 address stays IPv6");
        };
        assert_eq!(rewritten.port(), 47999);
        assert_eq!(rewritten.scope_id(), 7);
    }

    #[test]
    fn the_local_address_toward_ipv6_loopback_is_ipv6() {
        let Ok(probe) = UdpSocket::bind("[::1]:0") else {
            eprintln!("skipped: no IPv6 loopback");
            return;
        };
        let local = local_address_toward(probe.local_addr().unwrap()).unwrap();
        assert_eq!(local, "[::1]:0".parse().unwrap());
        bind_udp(local, 0, Qos::BestEffort, false, |_| {}).unwrap();
    }

    /// nightjar#38: a link-local host only works with its scope, through the connect to it
    /// and the binds after.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_link_local_host_keeps_its_scope() {
        let Some(local) = link_local_address() else {
            eprintln!("skipped: no link-local IPv6 interface");
            return;
        };
        // Our own address stands in for the host on the same link.
        let host = UdpSocket::bind(local).unwrap();
        let toward = local_address_toward(host.local_addr().unwrap()).unwrap();
        let SocketAddr::V6(v6) = toward else {
            panic!("{toward} is not IPv6");
        };
        assert_eq!(v6.scope_id(), local.scope_id());
        let socket = bind_udp(toward, 0, Qos::BestEffort, false, |_| {}).unwrap();
        socket
            .send_to(b"hello", host.local_addr().unwrap())
            .unwrap();
    }

    /// A usable link-local IPv6 address of this machine with its scope, from
    /// /proc/net/if_inet6 (address, interface index, prefix, scope, flags, name).
    #[cfg(target_os = "linux")]
    fn link_local_address() -> Option<std::net::SocketAddrV6> {
        // IFA_F_DADFAILED and IFA_F_TENTATIVE: not bindable.
        const UNUSABLE: u32 = 0x08 | 0x40;
        let table = std::fs::read_to_string("/proc/net/if_inet6").ok()?;
        table.lines().find_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let address = Ipv6Addr::from(u128::from_str_radix(fields.first()?, 16).ok()?);
            let index = u32::from_str_radix(fields.get(1)?, 16).ok()?;
            let flags = u32::from_str_radix(fields.get(4)?, 16).ok()?;
            (address.segments()[0] & 0xffc0 == 0xfe80 && flags & UNUSABLE == 0)
                .then(|| std::net::SocketAddrV6::new(address, 0, 0, index))
        })
    }

    #[test]
    fn the_granted_receive_buffer_is_read_back() {
        let asked = 1 << 20;
        let bind = |size| {
            bind_udp_sized(
                (Ipv4Addr::LOCALHOST, 0).into(),
                size,
                Qos::BestEffort,
                false,
                |m: &str| panic!("{m}"),
            )
            .unwrap()
            .1
        };
        let buffer = bind(asked).expect("a size was asked for");
        assert_eq!(buffer.asked, asked);
        // Linux grants up to net.core.rmem_max, silently; Windows what was asked.
        #[cfg(target_os = "linux")]
        let expected = std::fs::read_to_string("/proc/sys/net/core/rmem_max")
            .unwrap()
            .trim()
            .parse::<usize>()
            .unwrap()
            .min(asked);
        #[cfg(not(target_os = "linux"))]
        let expected = asked;
        assert_eq!(buffer.granted, expected);
        assert_eq!(bind(0), None, "the default is not read");
    }

    #[test]
    fn a_short_receive_buffer_says_how_much_and_how_to_raise_it() {
        let full = ReceiveBuffer {
            asked: 4 << 20,
            granted: 4 << 20,
        };
        assert!(!full.short());
        assert_eq!(full.describe("Video"), "Video receive buffer: 4096 KiB, as asked");
        let capped = ReceiveBuffer {
            asked: 4 << 20,
            granted: 212_992,
        };
        assert!(capped.short());
        let text = capped.describe("Video");
        assert!(
            text.starts_with("Video receive buffer: the system granted 208 KiB of the 4096 KiB asked"),
            "{text}"
        );
        assert_eq!(
            text.contains("sudo sysctl -w net.core.rmem_max=4194304"),
            cfg!(any(target_os = "linux", target_os = "android")),
            "{text}"
        );
    }

    #[test]
    fn receive_times_out_without_data() {
        let socket = bind_udp(
            (Ipv4Addr::LOCALHOST, 0).into(),
            1 << 20,
            Qos::BestEffort,
            false,
            |_| {},
        )
        .unwrap();
        let mut buffer = [0u8; 64];
        assert!(receive(&socket, &mut buffer).unwrap().is_none());
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        sender
            .send_to(b"hello", socket.local_addr().unwrap())
            .unwrap();
        assert_eq!(receive(&socket, &mut buffer).unwrap(), Some(5));
    }

    /// A loopback media socket with a receive buffer that holds every test datagram.
    fn media_socket() -> UdpSocket {
        bind_udp((Ipv4Addr::LOCALHOST, 0).into(), 1 << 20, Qos::Video, false, |_| {}).unwrap()
    }

    /// `count` datagrams of `size` bytes, the last one `last` bytes, each starting with its
    /// sequence number, sent to `to`.
    fn send_numbered(to: &UdpSocket, count: u16, size: usize, last: usize) -> Vec<Vec<u8>> {
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let datagrams: Vec<Vec<u8>> = (0..count)
            .map(|n| {
                let length = if n + 1 == count { last } else { size };
                let mut datagram: Vec<u8> = (0..length).map(|i| (i as u8) ^ (n as u8)).collect();
                datagram[..2].copy_from_slice(&n.to_be_bytes());
                datagram
            })
            .collect();
        for datagram in &datagrams {
            sender.send_to(datagram, to.local_addr().unwrap()).unwrap();
        }
        datagrams
    }

    /// Receives until `expected` datagrams came: them and the count of each receive.
    fn receive_all(
        batch: &mut BatchReceiver,
        socket: &UdpSocket,
        expected: usize,
    ) -> (Vec<Vec<u8>>, Vec<usize>) {
        let mut received = Vec::new();
        let mut counts = Vec::new();
        while received.len() < expected {
            let count = batch.receive(socket).unwrap();
            assert_ne!(count, 0, "timed out after {} datagrams", received.len());
            for i in 0..count {
                assert_eq!(batch.length(i), batch.datagram(i).len());
                received.push(batch.datagram(i).to_vec());
            }
            counts.push(count);
        }
        (received, counts)
    }

    #[test]
    fn a_batch_delivers_every_queued_datagram_in_order() {
        let socket = media_socket();
        let mut batch = BatchReceiver::new(&socket, 1500);
        assert!(batch.take_fallback().is_none());
        // More than a batch, all queued before the first receive, the last one short.
        let sent = send_numbered(&socket, RECEIVE_BATCH as u16 * 2 + 5, 1392, 77);
        let (received, counts) = receive_all(&mut batch, &socket, sent.len());
        assert_eq!(received, sent);
        assert!(counts.iter().all(|&count| count <= RECEIVE_BATCH), "{counts:?}");
        if cfg!(any(target_os = "linux", target_os = "android")) {
            assert!(batch.batched());
            // Not necessarily all in the first call: under load the kernel may queue them late.
            assert!(counts.iter().any(|&count| count > 1), "{counts:?}");
        } else if !batch.batched() {
            assert!(counts.iter().all(|&count| count == 1), "{counts:?}");
        }
        // Coalescing on Windows: whether loopback coalesces depends on the Windows version.
        assert_eq!(batch.receive(&socket).unwrap(), 0, "then the poll timeout");
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn a_kernel_without_batches_takes_one_datagram_per_call() {
        let socket = media_socket();
        mmsg::REFUSED.set(true);
        let mut batch = BatchReceiver::new(&socket, 1500);
        assert!(!batch.batched());
        let refusal = batch.take_fallback().expect("the refusal");
        assert_eq!(refusal.raw_os_error(), Some(libc::ENOSYS));
        let sent = send_numbered(&socket, 5, 1392, 10);
        let (received, counts) = receive_all(&mut batch, &socket, sent.len());
        mmsg::REFUSED.set(false);
        assert_eq!(received, sent);
        assert_eq!(counts, [1; 5]);
        assert!(batch.take_fallback().is_none(), "reported once");
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn a_refusal_mid_stream_is_reported_once_and_receives_go_on_one_at_a_time() {
        let socket = media_socket();
        let mut batch = BatchReceiver::new(&socket, 1500);
        assert!(batch.batched());
        let sent = send_numbered(&socket, 3, 1392, 10);
        assert_eq!(receive_all(&mut batch, &socket, sent.len()).0, sent);
        assert!(batch.take_fallback().is_none());
        mmsg::REFUSED.set(true);
        // Reported after the receive that met it, even when that one then timed out.
        assert_eq!(batch.receive(&socket).unwrap(), 0);
        let refusal = batch.take_fallback().expect("the refusal");
        assert_eq!(refusal.raw_os_error(), Some(libc::ENOSYS));
        assert!(!batch.batched());
        let sent = send_numbered(&socket, 4, 1392, 10);
        let (received, counts) = receive_all(&mut batch, &socket, sent.len());
        mmsg::REFUSED.set(false);
        assert_eq!(received, sent);
        assert_eq!(counts, [1; 4]);
        assert!(batch.take_fallback().is_none(), "reported once");
    }

    #[test]
    fn without_batches_every_datagram_comes_in_a_call_of_its_own() {
        let socket = media_socket();
        let mut batch = BatchReceiver::unbatched(&socket, 1500);
        assert!(!batch.batched());
        let sent = send_numbered(&socket, 10, 1392, 3);
        let (received, counts) = receive_all(&mut batch, &socket, sent.len());
        assert_eq!(received, sent);
        assert_eq!(counts, [1; 10]);
        assert_eq!(batch.receive(&socket).unwrap(), 0);
    }

    #[test]
    fn a_datagram_is_delivered_without_waiting_for_more() {
        for batched in [true, false] {
            let socket = media_socket();
            // A receive that waited to fill its batch would wait this long.
            socket.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
            let address = socket.local_addr().unwrap();
            let (tx, rx) = std::sync::mpsc::channel();
            let receiver = std::thread::spawn(move || {
                let mut batch = if batched {
                    BatchReceiver::new(&socket, 1500)
                } else {
                    BatchReceiver::unbatched(&socket, 1500)
                };
                tx.send(()).unwrap();
                let count = batch.receive(&socket).unwrap();
                (count, batch.datagram(0).to_vec())
            });
            rx.recv().unwrap();
            // The receiver is about to block, or already blocked: one datagram, then nothing.
            std::thread::sleep(Duration::from_millis(50));
            UdpSocket::bind("127.0.0.1:0")
                .unwrap()
                .send_to(b"alone", address)
                .unwrap();
            let waiting = Instant::now();
            while !receiver.is_finished() {
                assert!(waiting.elapsed() < Duration::from_secs(10), "the receive waited for more");
                std::thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(receiver.join().unwrap(), (1, b"alone".to_vec()));
        }
    }

    #[test]
    fn a_long_datagram_counts_in_full() {
        let socket = media_socket();
        let mut batch = BatchReceiver::new(&socket, 100);
        send_numbered(&socket, 2, 1000, 50);
        let mut seen = Vec::new();
        while seen.len() < 2 {
            let count = batch.receive(&socket).unwrap();
            assert_ne!(count, 0);
            seen.extend((0..count).map(|i| (batch.length(i), batch.datagram(i).len())));
        }
        // recvmmsg batches cut it to the reader's maximum (rounded up to the slot); elsewhere,
        // coalesced reads included, it is read whole.
        let mmsg = cfg!(any(target_os = "linux", target_os = "android"));
        let cut = if mmsg && batch.batched() { 128 } else { 1000 };
        assert_eq!(seen, [(1000, cut), (50, 50)]);
        let mut fallback = BatchReceiver::unbatched(&socket, 100);
        send_numbered(&socket, 1, 1000, 1000);
        assert_eq!(fallback.receive(&socket).unwrap(), 1);
        assert_eq!(fallback.length(0), 1000);
    }

    #[test]
    fn a_batch_skips_icmp_reports() {
        // Connected to a closed port, a send draws an ICMP port unreachable, which the next
        // receive reports as an error.
        let refused = || {
            let socket = media_socket();
            let closed = UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
            socket.connect(closed).unwrap();
            socket
        };
        let control = refused();
        control.send(b"ping").unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let error = control.recv(&mut [0; 16]).expect_err("an ICMP report");
        assert!(matches!(failure(error), Failure::Retry));
        for batched in [true, false] {
            let socket = refused();
            let mut batch = if batched {
                BatchReceiver::new(&socket, 1500)
            } else {
                BatchReceiver::unbatched(&socket, 1500)
            };
            socket.send(b"ping").unwrap();
            std::thread::sleep(Duration::from_millis(50));
            assert_eq!(batch.receive(&socket).unwrap(), 0, "batched {batched}");
        }
    }

    /// Takes every datagram of `segments`, a receive's worth at a time: the (start, length)
    /// of each and the count of each take.
    fn take_all(mut segments: Segments) -> (Vec<(usize, usize)>, Vec<usize>) {
        let mut lengths = [0; RECEIVE_BATCH];
        let (mut datagrams, mut counts) = (Vec::new(), Vec::new());
        while segments.left() > 0 {
            let (base, count) = segments.take(&mut lengths);
            let stride = segments.segment();
            datagrams.extend((0..count).map(|i| (base + i * stride, lengths[i])));
            counts.push(count);
        }
        (datagrams, counts)
    }

    #[test]
    fn a_coalesced_read_splits_into_its_datagrams() {
        // Equal datagrams.
        let (datagrams, counts) = take_all(Segments::new(3 * 1408, Some(1408)));
        assert_eq!(datagrams, [(0, 1408), (1408, 1408), (2816, 1408)]);
        assert_eq!(counts, [3]);
        // A short last one.
        let (datagrams, _) = take_all(Segments::new(2 * 1408 + 77, Some(1408)));
        assert_eq!(datagrams, [(0, 1408), (1408, 1408), (2816, 77)]);
        // No control message: the read is one datagram, as is an empty one.
        assert_eq!(take_all(Segments::new(1408, None)).0, [(0, 1408)]);
        assert_eq!(take_all(Segments::new(0, None)).0, [(0, 0)]);
        // A size that cannot split the read leaves it whole.
        for size in [Some(0), Some(1408), Some(5000)] {
            assert_eq!(take_all(Segments::new(1408, size)).0, [(0, 1408)], "{size:?}");
        }
        // More datagrams than a receive takes: the rest come with the next receives.
        let (datagrams, counts) = take_all(Segments::new(70 * 100 + 30, Some(100)));
        assert_eq!(counts, [RECEIVE_BATCH, RECEIVE_BATCH, 7]);
        assert_eq!(datagrams.len(), 71);
        assert!(datagrams.windows(2).all(|w| w[1].0 == w[0].0 + 100), "{datagrams:?}");
        assert_eq!(datagrams[70], (7000, 30));
        assert_eq!(datagrams.iter().map(|d| d.1).sum::<usize>(), 7030);
    }

    /// A control message as `WSARecvMsg` lays it out: `WSACMSGHDR`, then the data at the
    /// natural alignment, then padding to the header alignment.
    fn control_message(level: i32, kind: i32, data: &[u8]) -> Vec<u8> {
        let word = std::mem::size_of::<usize>();
        let start = (word + 8).next_multiple_of(word);
        let length = start + data.len();
        let mut message = Vec::new();
        message.extend_from_slice(&length.to_ne_bytes());
        message.extend_from_slice(&level.to_ne_bytes());
        message.extend_from_slice(&kind.to_ne_bytes());
        message.resize(start, 0);
        message.extend_from_slice(data);
        message.resize(length.next_multiple_of(word), 0);
        message
    }

    #[test]
    fn the_datagram_size_is_read_from_the_coalesced_info() {
        let info = control_message(17, 3, &1408u32.to_ne_bytes());
        assert_eq!(coalesced_segment(&info), Some(1408));
        // Behind another message (IP_PKTINFO's level and type, say).
        let mut both = control_message(0, 19, &[1; 20]);
        both.extend_from_slice(&info);
        assert_eq!(coalesced_segment(&both), Some(1408));
        // None without it, or with a message that runs past the buffer.
        assert_eq!(coalesced_segment(&[]), None);
        assert_eq!(coalesced_segment(&control_message(0, 19, &[1; 20])), None);
        assert_eq!(coalesced_segment(&info[..info.len() - 8]), None);
        let mut short = info.clone();
        short[0] = 4;
        assert_eq!(coalesced_segment(&short), None, "a length shorter than its header");
        // A zero size is passed on, and leaves the read whole.
        let zero = control_message(17, 3, &0u32.to_ne_bytes());
        assert_eq!(coalesced_segment(&zero), Some(0));
        assert_eq!(Segments::new(2816, coalesced_segment(&zero)).left(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn a_system_without_coalescing_takes_one_datagram_per_call() {
        let socket = media_socket();
        uro::REFUSED.set(true);
        let mut batch = BatchReceiver::new(&socket, 1500);
        uro::REFUSED.set(false);
        assert!(!batch.batched());
        let refusal = batch.take_fallback().expect("the refusal");
        assert_eq!(refusal.raw_os_error(), Some(10042), "WSAENOPROTOOPT");
        let sent = send_numbered(&socket, 5, 1392, 10);
        let (received, counts) = receive_all(&mut batch, &socket, sent.len());
        assert_eq!(received, sent);
        assert_eq!(counts, [1; 5]);
        assert!(batch.take_fallback().is_none(), "reported once");
    }

    #[cfg(windows)]
    #[test]
    fn a_roams_new_socket_coalesces_too_or_falls_back_once() {
        let first = media_socket();
        let mut batch = BatchReceiver::new(&first, 1500);
        if batch.take_fallback().is_some() {
            return; // This Windows has no coalescing.
        }
        use std::os::windows::io::AsRawSocket;
        let second = media_socket();
        let sent = send_numbered(&second, 3, 1392, 10);
        assert_eq!(receive_all(&mut batch, &second, sent.len()).0, sent);
        let coalesce = batch.coalesce.as_ref().expect("still coalescing");
        assert_eq!(uro::socket_of(coalesce), second.as_raw_socket() as usize);
        // A third that refuses it: reported once, then one datagram per call.
        let third = media_socket();
        uro::REFUSED.set(true);
        let sent = send_numbered(&third, 3, 1392, 10);
        let (received, counts) = receive_all(&mut batch, &third, sent.len());
        uro::REFUSED.set(false);
        assert_eq!(received, sent);
        assert_eq!(counts, [1; 3]);
        assert!(!batch.batched());
        assert_eq!(batch.take_fallback().unwrap().raw_os_error(), Some(10042));
    }

    /// Whether this Windows coalesces loopback datagrams, and that a coalesced stream comes
    /// back as it was sent. Loopback coalesces on some Windows versions only, so it reports
    /// what it saw rather than requiring it:
    ///
    /// `cargo test -p nightjar-core --lib coalescing_on_loopback -- --ignored --nocapture`
    #[cfg(windows)]
    #[test]
    #[ignore = "reports whether loopback coalesces on this Windows"]
    fn coalescing_on_loopback_splits_back_into_the_sent_datagrams() {
        let local = (Ipv4Addr::LOCALHOST, 0).into();
        let socket = bind_udp(local, 8 << 20, Qos::Video, false, |_| {}).unwrap();
        let mut batch = BatchReceiver::new(&socket, 1408);
        if let Some(e) = batch.take_fallback() {
            println!("coalescing refused: {e}");
            return;
        }
        let mut coalesced = 0;
        let mut calls = 0;
        // Several rounds of bursts, each with a short last datagram, as frames end.
        for round in 0..20 {
            let sent = send_numbered(&socket, 200, 1408, 100 + round);
            let (received, counts) = receive_all(&mut batch, &socket, sent.len());
            assert_eq!(received, sent, "round {round}");
            assert!(counts.iter().all(|&count| count <= RECEIVE_BATCH), "{counts:?}");
            coalesced += counts.iter().filter(|&&count| count > 1).count();
            calls += counts.len();
        }
        assert_eq!(batch.receive(&socket).unwrap(), 0, "then the poll timeout");
        println!(
            "coalescing on; {coalesced} of {calls} receives took more than one datagram ({})",
            if coalesced > 0 {
                "loopback coalesces"
            } else {
                "loopback does not coalesce here"
            }
        );
    }
}
