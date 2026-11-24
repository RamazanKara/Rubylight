//! ENet UDP socket wrapper: QoS marking watch, roaming inbox, wake filtering, readiness
//! polling and disconnect intercept.

use std::collections::VecDeque;
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use rubylight_protocol::roaming::is_roaming;
use rusty_enet::{PacketReceived, MTU_MAX};

use super::Warn;
use crate::priority;

/// Control socket buffers: room for the input a stall on this thread leaves queued in the kernel
/// (receive: the host's replies and rumble; send: a burst of input and acknowledgements).
const CONTROL_RECV_BUFFER: usize = 256 * 1024;
const CONTROL_SEND_BUFFER: usize = 128 * 1024;
/// Roaming datagrams set aside between services at most; the host sends one per PROBE.
const MAX_ROAM_INBOX: usize = 8;

/// Codes of sends and receives that only mean a datagram was lost, which ENet treats as
/// packet loss: unreachable networks and hosts during a roam, and ICMP port unreachable.
pub(super) fn datagram_lost(error: &io::Error) -> bool {
    #[cfg(windows)]
    const LOST: &[i32] = &[10050, 10051, 10052, 10064, 10065];
    #[cfg(not(windows))]
    const LOST: &[i32] = &[100, 101, 112, 113];
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::AddrNotAvailable
    ) || error
        .raw_os_error()
        .is_some_and(|code| LOST.contains(&code))
}

/// Whether a datagram addressed to peer 0 is exactly one ENet disconnect command (without a
/// checksum or compression). Returns whether it is unsequenced.
pub(super) fn enet_disconnect(datagram: &[u8]) -> Option<bool> {
    const FLAG_COMPRESSED: u16 = 1 << 14;
    const FLAG_SENT_TIME: u16 = 1 << 15;
    const FLAGS_AND_SESSION: u16 = 0xF000;
    const COMMAND_DISCONNECT: u8 = 4;
    const COMMAND_FLAG_UNSEQUENCED: u8 = 1 << 6;
    const DISCONNECT_BYTES: usize = 8;
    if datagram.len() < 2 {
        return None;
    }
    let peer_id = u16::from_be_bytes([datagram[0], datagram[1]]);
    if peer_id & FLAG_COMPRESSED != 0 || peer_id & !FLAGS_AND_SESSION != 0 {
        return None;
    }
    let header = if peer_id & FLAG_SENT_TIME != 0 { 4 } else { 2 };
    if datagram.len() != header + DISCONNECT_BYTES || datagram[header] & 0x0F != COMMAND_DISCONNECT
    {
        return None;
    }
    Some(datagram[header] & COMMAND_FLAG_UNSEQUENCED != 0)
}

/// The ENet socket: drops wake datagrams, sets roaming datagrams aside, notes when the host
/// was last heard, and holds back a host disconnect so receives queued before it are not
/// lost (Nightjar's disconnect intercept), handing it to ENet once they are processed.
pub(super) struct ControlSocket {
    pub(super) socket: UdpSocket,
    /// Roaming datagrams for [`crate::roaming::Roaming::handle_datagram`]; ENet would drop them.
    pub(super) roam_inbox: VecDeque<Vec<u8>>,
    /// Both sides have roaming on; otherwise roaming datagrams are dropped, not kept.
    pub(super) roaming: bool,
    pub(super) intercept: bool,
    pub(super) held_disconnect: Option<(SocketAddr, Vec<u8>)>,
    pub(super) held_unsequenced: bool,
    pub(super) reinject: Option<(SocketAddr, Vec<u8>)>,
    pub(super) last_receive: Option<Instant>,
    /// When a host disconnect last went to ENet: the [`rusty_enet::Event::Disconnect`] that
    /// follows within [`super::DISCONNECT_PROCESS_WAIT`] is the host's goodbye
    /// ([`super::is_goodbye`]).
    pub(super) disconnect_handed: Option<Instant>,
    /// Set while the connect is in flight and the socket is still marked.
    pub(super) marking: Option<Marking>,
    /// The QoS marking is on, for a socket the control thread moves to.
    pub(super) marked: bool,
    /// Windows' marking: the qWAVE flow each socket joins at its first send. None elsewhere,
    /// once the marking is off, or when qWAVE refused (said once).
    pub(super) flow: Option<priority::ControlFlow>,
    pub(super) send_failures: SendFailures,
    pub(super) warn: Warn,
}

/// Sends that failed with a code ENet treats as packet loss ([`datagram_lost`]): a streak is
/// reported once, and a connect none of whose datagrams left fails with the last error
/// instead of a timeout.
#[derive(Debug, Default)]
pub(super) struct SendFailures {
    pub(super) consecutive: u32,
    pub(super) any_sent: bool,
    last: Option<io::Error>,
}

impl SendFailures {
    /// Failures in a row before the streak is reported.
    const REPORT_AFTER: u32 = 3;

    /// Counts a failed send to `to`; returns the warning on the streak's third failure.
    pub(super) fn failed(&mut self, to: SocketAddr, error: io::Error) -> Option<String> {
        self.consecutive = self.consecutive.saturating_add(1);
        let warning = (self.consecutive == Self::REPORT_AFTER)
            .then(|| format!("Control datagrams to {to} are failing: {error}"));
        self.last = Some(error);
        warning
    }

    pub(super) fn sent(&mut self) {
        self.consecutive = 0;
        self.any_sent = true;
    }

    /// The last error, if no send has succeeded yet and at least one failed.
    pub(super) fn take_if_none_sent(&mut self) -> Option<io::Error> {
        if self.any_sent {
            None
        } else {
            self.last.take()
        }
    }
}

/// The connect-time watch over the control socket's QoS marking. ENet drops the marking when
/// three connect attempts in a row get no answer (`enet_protocol_send_outgoing_commands`):
/// some networks discard marked packets, so the fourth goes out plain.
pub(super) struct Marking {
    pub(super) ipv6: bool,
    pub(super) unanswered: u32,
}

/// Marks the control socket and starts the watch over the marking. A refused option is
/// reported, but the watch runs regardless: if one of the options took, it still has to come
/// off for a route that drops marked packets.
pub(super) fn mark_for_connect<S: priority::PriorityTarget>(
    socket: &S,
    ipv6: bool,
    warn: &Warn,
) -> Marking {
    if let Err(e) = priority::mark_control_socket(socket, ipv6, true) {
        warn.say(&format!("Could not mark the control socket for QoS: {e}"));
    }
    Marking {
        ipv6,
        unanswered: 0,
    }
}

/// A poll timeout in whole milliseconds, rounded up so a sub-millisecond timer does not spin.
pub(super) fn poll_millis(timeout: Duration) -> i32 {
    i32::try_from(timeout.as_nanos().div_ceil(1_000_000)).unwrap_or(i32::MAX)
}

/// Waits until `socket` is readable or `timeout` passes.
#[cfg(windows)]
fn poll_readable(socket: &UdpSocket, timeout: Duration) -> io::Result<()> {
    use std::os::windows::io::AsRawSocket;
    #[repr(C)]
    struct WsaPollFd {
        fd: usize,
        events: i16,
        revents: i16,
    }
    #[link(name = "ws2_32")]
    unsafe extern "system" {
        fn WSAPoll(fds: *mut WsaPollFd, count: u32, timeout: i32) -> i32;
    }
    const POLLRDNORM: i16 = 0x0100;
    let mut fd = WsaPollFd {
        fd: socket.as_raw_socket() as usize,
        events: POLLRDNORM,
        revents: 0,
    };
    // SAFETY: one valid WSAPOLLFD for a socket that outlives the call.
    if unsafe { WSAPoll(&mut fd, 1, poll_millis(timeout)) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Waits until `socket` is readable or `timeout` passes.
// POSIX/Unix (Linux, Android, macOS): poll(2) waits for socket readability.
#[cfg(unix)]
fn poll_readable(socket: &UdpSocket, timeout: Duration) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;
    let mut fd = libc::pollfd {
        fd: socket.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd for a socket that outlives the call.
    if unsafe { libc::poll(&mut fd, 1, poll_millis(timeout)) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

// Non-Windows, non-POSIX platforms fall back to blocking peek_from.
#[cfg(not(any(windows, unix)))]
fn poll_readable(_: &UdpSocket, _: Duration) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}

impl ControlSocket {
    /// Sleeps until a datagram arrives or `timeout` passes. A readiness poll leaves the socket
    /// non-blocking; if the poll fails, a blocking peek waits instead.
    pub(super) fn wait(&self, timeout: Duration) {
        if self.reinject.is_some() || timeout.is_zero() {
            return;
        }
        match poll_readable(&self.socket, timeout) {
            Err(e) if e.kind() != io::ErrorKind::Interrupted => self.wait_blocking(timeout),
            _ => {}
        }
    }

    fn wait_blocking(&self, timeout: Duration) {
        let blocking = self.socket.set_nonblocking(false).is_ok();
        if blocking && self.socket.set_read_timeout(Some(timeout)).is_ok() {
            let _ = self.socket.peek_from(&mut [0; 1]);
        }
        let _ = self.socket.set_nonblocking(true);
    }

    pub(super) fn release_disconnect(&mut self) {
        self.intercept = false;
        self.reinject = self.held_disconnect.take();
    }

    /// Moves to `socket` ([`super::ControlHandle::move_to`]), marked as the old one was. Datagrams
    /// still on their way to the old socket are lost, as on a real network change.
    pub(super) fn replace(&mut self, socket: UdpSocket) {
        if let Some(flow) = &mut self.flow {
            flow.remove();
        }
        if self.marked {
            let ipv6 = socket.local_addr().is_ok_and(|a| a.is_ipv6());
            if let Err(e) = priority::mark_control_socket(&socket, ipv6, true) {
                self.warn
                    .say(&format!("Could not mark the moved control socket for QoS: {e}"));
            }
        }
        self.socket = socket;
    }
}

impl rusty_enet::Socket for ControlSocket {
    type Address = SocketAddr;
    type Error = io::Error;

    fn init(&mut self, options: rusty_enet::SocketOptions) -> io::Result<()> {
        rusty_enet::Socket::init(&mut self.socket, options)?;
        // rusty_enet's UdpSocket::init leaves the buffer sizes at the OS default (its TODO: std
        // cannot set them), so they are set here. A refusal keeps the default.
        let socket = socket2::SockRef::from(&self.socket);
        let _ = socket.set_recv_buffer_size(CONTROL_RECV_BUFFER);
        let _ = socket.set_send_buffer_size(CONTROL_SEND_BUFFER);
        Ok(())
    }

    fn send(&mut self, address: SocketAddr, buffer: &[u8]) -> io::Result<usize> {
        if let Some(marking) = &mut self.marking {
            if priority::should_drop_control_marking(marking.unanswered) {
                let ipv6 = marking.ipv6;
                self.marking = None;
                self.marked = false;
                self.flow = None;
                self.warn
                    .say("No answer to the first control connects; sending without QoS marking");
                if let Err(e) = priority::mark_control_socket(&self.socket, ipv6, false) {
                    self.warn
                        .say(&format!("Could not unmark the control socket: {e}"));
                }
            } else {
                marking.unanswered += 1;
            }
        }
        if let Some(flow) = &mut self.flow
            && let Err(e) = flow.add(&self.socket, address)
        {
            self.warn
                .say(&format!("Could not mark the control socket for QoS: {e}"));
        }
        let result = rusty_enet::Socket::send(&mut self.socket, address, buffer);
        #[cfg(test)]
        let result = super::tests::send_result().map_or(result, |injected| {
            injected.map_err(io::Error::from_raw_os_error)
        });
        match result {
            Err(error) if datagram_lost(&error) => {
                if let Some(warning) = self.send_failures.failed(address, error) {
                    self.warn.say(&warning);
                }
                Ok(0)
            }
            // Ok(0) is a full send buffer (WouldBlock): nothing left.
            Ok(sent) if sent > 0 => {
                self.send_failures.sent();
                Ok(sent)
            }
            result => result,
        }
    }

    fn receive(
        &mut self,
        buffer: &mut [u8; MTU_MAX],
    ) -> io::Result<Option<(SocketAddr, PacketReceived)>> {
        if let Some((address, datagram)) = self.reinject.take() {
            self.disconnect_handed = Some(Instant::now());
            buffer[..datagram.len()].copy_from_slice(&datagram);
            return Ok(Some((address, PacketReceived::Complete(datagram.len()))));
        }
        loop {
            let (address, length) = match rusty_enet::Socket::receive(&mut self.socket, buffer) {
                Ok(Some((address, PacketReceived::Complete(length)))) => (address, length),
                Ok(other) => return Ok(other),
                Err(error) if datagram_lost(&error) => return Ok(None),
                Err(error) => return Err(error),
            };
            // A wake, or anything else shorter than ENet's 2-byte peer ID: not ENet's (ENet
            // drops it), so it does not count as the host. The waker's source address is
            // the OS's choice, so the length tells a wake apart.
            if length < 2 {
                continue;
            }
            // Roaming datagrams are not ENet's, and count as the host only once they open.
            if is_roaming(&buffer[..length]) {
                if self.roaming && self.roam_inbox.len() < MAX_ROAM_INBOX {
                    self.roam_inbox.push_back(buffer[..length].to_vec());
                }
                continue;
            }
            // Anything else on this private socket is the host, from whichever of its
            // addresses it replies (ENet follows the source, as moonlight-common-c's does).
            self.last_receive = Some(Instant::now());
            // The host answered: the marking works, so stop watching it.
            self.marking = None;
            let disconnect = enet_disconnect(&buffer[..length]);
            if self.intercept
                && self.held_disconnect.is_none()
                && let Some(unsequenced) = disconnect
            {
                self.held_disconnect = Some((address, buffer[..length].to_vec()));
                self.held_unsequenced = unsequenced;
                continue;
            }
            if disconnect.is_some() {
                self.disconnect_handed = Some(Instant::now());
            }
            return Ok(Some((address, PacketReceived::Complete(length))));
        }
    }
}
