//! Addresses and sockets: name resolution, the local address toward the host, the
//! local/remote decision, the media sockets and their pings.
//!
//! Ported from moonlight-common-c (GPL-3.0, Cameron Gutman and contributors):
//! `PlatformSockets.c` (resolveHostName, getLocalAddressByUdpConnect, bindUdpSocket,
//! recvUdpSocket, isPrivateNetworkAddress with Nightjar's `android_sockets.h` prefix fix,
//! isNat64SynthesizedAddress) and the ping threads of `VideoStream.c` and `AudioStream.c`.

mod batch;
#[cfg(any(target_os = "linux", target_os = "android"))]
mod mmsg;
#[cfg(windows)]
mod uro;

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, Socket, Type};

pub use batch::{BatchReceiver, RECEIVE_BATCH};

use crate::priority::{self, Qos};

/// How long a media receive waits before checking for shutdown and timeouts.
pub const UDP_RECV_POLL_TIMEOUT: Duration = Duration::from_millis(100);
/// How long a media receive that failed waits for the roam's new socket before it ends the
/// session: the rebind follows the route poll, which runs every second.
pub const REBIND_WAIT: Duration = Duration::from_secs(1);
const TEST_PORT_TIMEOUT: Duration = Duration::from_secs(3);
/// Addresses of a name tested at most, the first in the resolver's order (`probeAddressPorts` in
/// the C core's resolve-probe patch).
const PROBED_ADDRESSES: usize = 8;
/// Once an address answers, how much longer an address the resolver put before it may take.
const PREFERENCE_WAIT: Duration = Duration::from_millis(250);
const RCV_BUFFER_SIZE_MIN: usize = 32767;
const RCV_BUFFER_SIZE_STEP: usize = 16384;
const PING_INTERVAL: Duration = Duration::from_millis(500);
const LEGACY_PING: [u8; 4] = *b"PING";

/// Resolves `host` (a name or literal, IPv6 with or without brackets). With several
/// addresses, the most preferred one that accepts a TCP connection on one of `test_ports`
/// wins ([`probe`]), as the C core does; a single address is taken untested.
pub fn resolve(host: &str, test_ports: &[u16]) -> io::Result<SocketAddr> {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let addresses: Vec<SocketAddr> = (host, 0).to_socket_addrs()?.collect();
    let first = *addresses
        .first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no address for host"))?;
    if addresses.len() == 1 {
        return Ok(first);
    }
    probe(
        &addresses,
        test_ports,
        TEST_PORT_TIMEOUT,
        PREFERENCE_WAIT,
        |candidate, timeout| TcpStream::connect_timeout(&candidate, timeout).is_ok(),
    )
    .ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "no working address for host",
        )
    })
}

/// Tests every port of the first [`PROBED_ADDRESSES`] addresses at once, each pair with
/// `connect` on a thread of its own, so dead addresses cost one `timeout` in all rather than
/// one each. Returns the most preferred address that answered on one of its ports, the
/// earliest in `addresses` (the resolver's order): once one answers, an earlier address still
/// trying gets `preference` more. `None` when none answers within `timeout`. Attempts still
/// running then end at their own timeout, and their answers go nowhere.
fn probe(
    addresses: &[SocketAddr],
    ports: &[u16],
    timeout: Duration,
    preference: Duration,
    connect: impl Fn(SocketAddr, Duration) -> bool + Clone + Send + 'static,
) -> Option<SocketAddr> {
    let addresses = &addresses[..addresses.len().min(PROBED_ADDRESSES)];
    let (tx, rx) = std::sync::mpsc::channel();
    // Attempts still running for each address.
    let mut trying = vec![0usize; addresses.len()];
    for (index, address) in addresses.iter().enumerate() {
        for &port in ports {
            let (tx, connect, candidate) = (tx.clone(), connect.clone(), with_port(*address, port));
            let spawned = std::thread::Builder::new()
                .name("ResolveProbe".into())
                .spawn(move || {
                    let _ = tx.send((index, connect(candidate, timeout)));
                });
            if spawned.is_ok() {
                trying[index] += 1;
            }
        }
    }
    drop(tx);
    let mut answered = vec![false; addresses.len()];
    let mut until = Instant::now() + timeout;
    let mut waiting_for_earlier = false;
    loop {
        let best = answered.iter().position(|&a| a);
        if !trying[..best.unwrap_or(addresses.len())].iter().any(|&t| t > 0) {
            return best.map(|i| addresses[i]);
        }
        if best.is_some() && !waiting_for_earlier {
            waiting_for_earlier = true;
            until = until.min(Instant::now() + preference);
        }
        let Some(left) = until.checked_duration_since(Instant::now()) else {
            return best.map(|i| addresses[i]);
        };
        match rx.recv_timeout(left) {
            Ok((index, ok)) => {
                trying[index] -= 1;
                answered[index] |= ok;
            }
            Err(_) => return best.map(|i| addresses[i]),
        }
    }
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

/// One receive (`Ok(None)` on timeout); skips ICMP port-unreachable as in `recvUdpSocket`.
#[cfg(test)]
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
