//! Runs a [`ControlSession`] and an [`InputQueue`] over ENet (rusty_enet, which the Rubylight
//! host also uses).
//!
//! [`connect`] opens the ENet peer with the control stream's 48 channels and the RTSP
//! connect data, and returns a [`ControlClient`] to run on a thread of the caller's choosing
//! and a [`ControlHandle`] for every other thread. The handle wakes the client through a
//! loopback datagram, so it sleeps on its socket between ENet timers.
//!
//! Teardown follows Nightjar's patched `stopControlStream`: queued input is drained, then the
//! peer disconnects gracefully and lingers up to two seconds for the host's acknowledgement,
//! unless the host already sent a disconnect or a termination message.
//!
//! When the host advertised roaming and [`DriverConfig::roaming`] is on, the driver also runs
//! [`Roaming`] beside ENet on the same socket: it sets roaming datagrams aside before ENet
//! sees them, and starts an attempt on [`ControlHandle::network_changed`] or after a second
//! without a word from the host.

use std::collections::VecDeque;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rand_core::{OsRng, RngCore};
use rubylight_protocol::roaming::{is_roaming, PROBE_FLAG_MEDIA_REBOUND};
use rusty_enet::{
    Event, Host, HostSettings, Packet, PacketKind, PacketReceived, PeerID, PeerState, MTU_MAX,
};

use super::events::ControlEvent;
use super::messages::ClientMessage;
use super::negotiation::HostCapabilities;
use super::session::{ControlSession, SendError, SessionOptions};
use super::wire::{channel, Delivery};
use crate::crypto::Key;
use crate::input::{InputPacket, InputQueue};
use crate::priority;
use crate::roaming::{RoamEvent, RoamTrigger, Roaming};

/// The ENet peer timeout without a network grace period.
pub const CONTROL_PEER_TIMEOUT: Duration = Duration::from_secs(10);
/// With a grace period, input is held back once nothing has come from the host this long.
pub const INPUT_CONTACT_LOST_AFTER: Duration = Duration::from_millis(1000);
/// Receives that may still follow a host's disconnect are awaited this long.
const DISCONNECT_RECEIVE_WAIT: Duration = Duration::from_millis(100);
/// A disconnect handed back to ENet that ENet ignored (another session's) is forgotten after.
const DISCONNECT_PROCESS_WAIT: Duration = Duration::from_secs(1);
/// Longest sleep between ENet services, for its resend and ping timers.
const MAX_SERVICE_INTERVAL: Duration = Duration::from_millis(5);
/// Roaming datagrams set aside between services at most; the host sends one per PROBE.
const MAX_ROAM_INBOX: usize = 8;

/// Where the driver reports a problem that does not stop the stream (a socket option the OS
/// refused).
#[derive(Clone)]
pub struct Warn(Arc<dyn Fn(&str) + Send + Sync>);

impl Warn {
    pub fn new(warn: impl Fn(&str) + Send + Sync + 'static) -> Self {
        Self(Arc::new(warn))
    }

    fn say(&self, message: &str) {
        (self.0)(message);
    }
}

impl Default for Warn {
    fn default() -> Self {
        Self::new(|_| {})
    }
}

impl std::fmt::Debug for Warn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Warn")
    }
}

#[derive(Clone, Debug)]
pub struct DriverConfig {
    /// The host's control port (RTSP SETUP of `streamid=control`).
    pub remote: SocketAddr,
    /// Local address to bind; the unspecified address of the remote's family by default.
    pub local: Option<SocketAddr>,
    /// `X-SS-Connect-Data` from RTSP, passed with the ENet connect.
    pub connect_data: u32,
    /// Nightjar's network grace period: a longer peer timeout, and input held back while the
    /// host is silent. Up to [`CONTROL_PEER_TIMEOUT`] changes nothing.
    pub network_grace: Duration,
    pub connect_timeout: Duration,
    pub linger: Duration,
    /// Send mouse and pen motion at once ([`InputQueue::set_unbatched`]).
    pub unbatched_input: bool,
    /// Never send input ([`InputQueue::disable`]), for remote monitor sessions.
    pub input_disabled: bool,
    /// The client's roaming setting. Probes go out only when the host also advertised
    /// roaming ([`HostCapabilities::client_roaming`]).
    pub roaming: bool,
    pub warn: Warn,
}

impl DriverConfig {
    pub fn new(remote: SocketAddr) -> Self {
        Self {
            remote,
            local: None,
            connect_data: 0,
            network_grace: Duration::ZERO,
            connect_timeout: Duration::from_secs(10),
            linger: Duration::from_secs(2),
            unbatched_input: false,
            input_disabled: false,
            roaming: false,
            warn: Warn::default(),
        }
    }

    fn peer_timeout(&self) -> Duration {
        self.network_grace.max(CONTROL_PEER_TIMEOUT)
    }

    fn grace_enabled(&self) -> bool {
        self.network_grace > CONTROL_PEER_TIMEOUT
    }
}

#[derive(Debug)]
pub enum ConnectError {
    Io(io::Error),
    /// No answer within the connect timeout.
    TimedOut,
    /// [`ControlHandle::stop`] was called while connecting.
    Interrupted,
    /// The host refused or dropped the connection attempt.
    Refused,
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "control stream socket: {error}"),
            Self::TimedOut => f.write_str("control stream connection timed out"),
            Self::Interrupted => f.write_str("control stream connection interrupted"),
            Self::Refused => f.write_str("the host refused the control stream connection"),
        }
    }
}

impl std::error::Error for ConnectError {}

impl From<io::Error> for ConnectError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// How [`ControlClient::run`] ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// [`ControlHandle::stop`] ended it.
    Stopped,
    /// The host ended it, or the connection failed; the code is also delivered as
    /// [`ControlEvent::Terminated`].
    Terminated(i32),
}

/// Wakes the control thread with a datagram to the control socket; connected to it, so a
/// moved control socket ([`ControlHandle::move_to`]) is a reconnect away.
struct Waker {
    socket: UdpSocket,
}

/// Where wakes for the control socket at `enet_local` go. A specific local address keeps its
/// IPv6 scope, which a link-local one needs.
fn wake_target(enet_local: SocketAddr) -> SocketAddr {
    let mut target = enet_local;
    if target.ip().is_unspecified() {
        target.set_ip(match target.ip() {
            IpAddr::V4(_) => Ipv4Addr::LOCALHOST.into(),
            IpAddr::V6(_) => Ipv6Addr::LOCALHOST.into(),
        });
    }
    target
}

impl Waker {
    fn new(enet_local: SocketAddr) -> io::Result<Self> {
        let target = wake_target(enet_local);
        let mut bind = target;
        bind.set_port(0);
        let socket = UdpSocket::bind(bind)?;
        socket.set_nonblocking(true)?;
        socket.connect(target)?;
        Ok(Self { socket })
    }

    fn retarget(&self, enet_local: SocketAddr) -> io::Result<()> {
        self.socket.connect(wake_target(enet_local))
    }

    fn wake(&self) {
        let _ = self.socket.send(&[0]);
    }
}

struct Shared {
    session: Mutex<ControlSession>,
    input: Mutex<InputQueue>,
    stop: AtomicBool,
    contact_lost: AtomicBool,
    rtt: Mutex<Option<(Duration, Duration)>>,
    /// Both sides have roaming on.
    roaming: bool,
    /// PROBE flags of a [`ControlHandle::network_changed`] the control thread has not taken.
    roam_request: Mutex<Option<u32>>,
    /// [`ControlHandle::hold_roaming`]: no attempts.
    roam_held: AtomicBool,
    /// A socket from [`ControlHandle::move_to`] the control thread has not moved to.
    next_socket: Mutex<Option<UdpSocket>>,
    waker: Waker,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Thread-safe access to a running control stream.
#[derive(Clone)]
pub struct ControlHandle {
    shared: Arc<Shared>,
}

impl ControlHandle {
    /// Sends a control message; the error says why it was refused.
    pub fn send(&self, message: ClientMessage) -> Result<(), SendError> {
        self.with_session(|session| session.send(message))
    }

    /// Runs `f` on the session (video feedback, clock sync probes, HDR state) and wakes the
    /// control thread to send what it queued.
    pub fn with_session<R>(&self, f: impl FnOnce(&mut ControlSession) -> R) -> R {
        let result = f(&mut lock(&self.shared.session));
        self.shared.waker.wake();
        result
    }

    /// Runs `f` on the session without waking the control thread, for state that waits for
    /// its next service (at most 5 ms), such as per-frame connection quality.
    pub fn with_session_quiet<R>(&self, f: impl FnOnce(&mut ControlSession) -> R) -> R {
        f(&mut lock(&self.shared.session))
    }

    /// Runs `f` on the input queue and wakes the control thread.
    pub fn with_input<R>(&self, f: impl FnOnce(&mut InputQueue) -> R) -> R {
        let result = f(&mut lock(&self.shared.input));
        self.shared.waker.wake();
        result
    }

    /// ENet's round-trip time and its variance, while connected.
    pub fn round_trip_time(&self) -> Option<(Duration, Duration)> {
        *lock(&self.shared.rtt)
    }

    /// Whether input is held back because the host has been silent through a grace period.
    pub fn contact_lost(&self) -> bool {
        self.shared.contact_lost.load(Ordering::Relaxed)
    }

    /// The network toward the host changed: starts a roaming attempt, or restarts the
    /// running one. `media_rebound` says the media sockets moved to new ports. Returns false,
    /// and sends nothing, unless both the host and the client have roaming on.
    pub fn network_changed(&self, media_rebound: bool) -> bool {
        if !self.shared.roaming || self.shared.roam_held.load(Ordering::Acquire) {
            return false;
        }
        let flags = if media_rebound {
            PROBE_FLAG_MEDIA_REBOUND
        } else {
            0
        };
        let mut request = lock(&self.shared.roam_request);
        *request = Some(request.unwrap_or(0) | flags);
        drop(request);
        self.shared.waker.wake();
        true
    }

    /// Whether both the host and the client have roaming on.
    pub fn roaming(&self) -> bool {
        self.shared.roaming
    }

    /// Holds roaming off while the client is on a network it may not roam onto (a metered
    /// one without the opt-in): the running attempt fails, none starts, and the network
    /// grace period decides. False lets roaming run again.
    pub fn hold_roaming(&self, held: bool) {
        self.shared.roam_held.store(held, Ordering::Release);
        self.shared.waker.wake();
    }

    /// Moves the control socket to a new port on `local`, as a network change would; a
    /// [`ControlHandle::network_changed`] should follow. For nightjar-cli's simulated roams:
    /// a real one needs no move, the socket is bound to the unspecified address and follows
    /// the route. Fails unless both sides have roaming on.
    pub fn move_to(&self, local: IpAddr) -> io::Result<()> {
        if !self.shared.roaming {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "roaming is off for this session",
            ));
        }
        if self.shared.waker.socket.peer_addr()?.is_ipv4() != local.is_ipv4() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the control socket cannot change address family",
            ));
        }
        let socket = UdpSocket::bind(SocketAddr::new(local, 0))?;
        // What rusty_enet's init sets on a UdpSocket.
        socket.set_nonblocking(true)?;
        socket.set_broadcast(true)?;
        let address = socket.local_addr()?;
        *lock(&self.shared.next_socket) = Some(socket);
        // Woken on the old socket, the control thread moves; later wakes go to the new one.
        self.shared.waker.wake();
        self.shared.waker.retarget(address)
    }

    /// Ends the stream: queued input is sent, then the peer disconnects.
    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::Release);
        lock(&self.shared.input).stop();
        self.shared.waker.wake();
    }
}

/// Codes of sends and receives that only mean a datagram was lost, which ENet treats as
/// packet loss: unreachable networks and hosts during a roam, and ICMP port unreachable.
fn datagram_lost(error: &io::Error) -> bool {
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
fn enet_disconnect(datagram: &[u8]) -> Option<bool> {
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
struct ControlSocket {
    socket: UdpSocket,
    wake_from: SocketAddr,
    /// Roaming datagrams for [`Roaming::handle_datagram`]; ENet would drop them.
    roam_inbox: VecDeque<Vec<u8>>,
    intercept: bool,
    held_disconnect: Option<(SocketAddr, Vec<u8>)>,
    held_unsequenced: bool,
    reinject: Option<(SocketAddr, Vec<u8>)>,
    last_receive: Option<Instant>,
    /// Set while the connect is in flight and the socket is still marked.
    marking: Option<Marking>,
    /// The QoS marking is on, for a socket the control thread moves to.
    marked: bool,
    send_failures: SendFailures,
    warn: Warn,
}

/// Sends that failed with a code ENet treats as packet loss ([`datagram_lost`]): a streak is
/// reported once, and a connect none of whose datagrams left fails with the last error
/// instead of a timeout.
#[derive(Debug, Default)]
struct SendFailures {
    consecutive: u32,
    any_sent: bool,
    last: Option<io::Error>,
}

impl SendFailures {
    /// Failures in a row before the streak is reported.
    const REPORT_AFTER: u32 = 3;

    /// Counts a failed send to `to`; returns the warning on the streak's third failure.
    fn failed(&mut self, to: SocketAddr, error: io::Error) -> Option<String> {
        self.consecutive = self.consecutive.saturating_add(1);
        let warning = (self.consecutive == Self::REPORT_AFTER)
            .then(|| format!("Control datagrams to {to} are failing: {error}"));
        self.last = Some(error);
        warning
    }

    fn sent(&mut self) {
        self.consecutive = 0;
        self.any_sent = true;
    }

    /// The last error, if no send has succeeded yet and at least one failed.
    fn take_if_none_sent(&mut self) -> Option<io::Error> {
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
struct Marking {
    ipv6: bool,
    unanswered: u32,
}

/// Marks the control socket and starts the watch over the marking. A refused option is
/// reported, but the watch runs regardless: if one of the options took, it still has to come
/// off for a route that drops marked packets.
fn mark_for_connect<S: priority::PriorityTarget>(socket: &S, ipv6: bool, warn: &Warn) -> Marking {
    if let Err(e) = priority::mark_control_socket(socket, ipv6, true) {
        warn.say(&format!("Could not mark the control socket for QoS: {e}"));
    }
    Marking {
        ipv6,
        unanswered: 0,
    }
}

impl ControlSocket {
    /// Sleeps until a datagram arrives or `timeout` passes.
    fn wait(&self, timeout: Duration) {
        if self.reinject.is_some() || timeout.is_zero() {
            return;
        }
        let blocking = self.socket.set_nonblocking(false).is_ok();
        if blocking && self.socket.set_read_timeout(Some(timeout)).is_ok() {
            let _ = self.socket.peek_from(&mut [0; 1]);
        }
        let _ = self.socket.set_nonblocking(true);
    }

    fn release_disconnect(&mut self) {
        self.intercept = false;
        self.reinject = self.held_disconnect.take();
    }

    /// Moves to `socket` ([`ControlHandle::move_to`]), marked as the old one was. Datagrams
    /// still on their way to the old socket are lost, as on a real network change.
    fn replace(&mut self, socket: UdpSocket) {
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
        rusty_enet::Socket::init(&mut self.socket, options)
    }

    fn send(&mut self, address: SocketAddr, buffer: &[u8]) -> io::Result<usize> {
        if let Some(marking) = &mut self.marking {
            if priority::should_drop_control_marking(marking.unanswered) {
                let ipv6 = marking.ipv6;
                self.marking = None;
                self.marked = false;
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
        let result = rusty_enet::Socket::send(&mut self.socket, address, buffer);
        #[cfg(test)]
        let result = tests::send_result().map_or(result, |injected| {
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
            if address == self.wake_from {
                continue;
            }
            // Roaming datagrams are not ENet's, and count as the host only once they open.
            if is_roaming(&buffer[..length]) {
                if self.roam_inbox.len() < MAX_ROAM_INBOX {
                    self.roam_inbox.push_back(buffer[..length].to_vec());
                }
                continue;
            }
            // Anything but a wake on this private socket is the host, from whichever of its
            // addresses it replies (ENet follows the source, as moonlight-common-c's does).
            // Shorter than ENet's 2-byte peer ID it is not ENet's (ENet drops it), so a stray
            // wake-sized datagram does not count.
            if length >= 2 {
                self.last_receive = Some(Instant::now());
                // The host answered: the marking works, so stop watching it.
                self.marking = None;
            }
            if self.intercept && self.held_disconnect.is_none() {
                if let Some(unsequenced) = enet_disconnect(&buffer[..length]) {
                    self.held_disconnect = Some((address, buffer[..length].to_vec()));
                    self.held_unsequenced = unsequenced;
                    continue;
                }
            }
            return Ok(Some((address, PacketReceived::Complete(length))));
        }
    }
}

/// The control thread's side: owns the ENet host.
pub struct ControlClient {
    host: Host<ControlSocket>,
    peer: PeerID,
    shared: Arc<Shared>,
    config: DriverConfig,
    disconnect_held_since: Option<Instant>,
    disconnect_released_at: Option<Instant>,
    /// Present when both sides have roaming on.
    roaming: Option<Roaming>,
    /// When the host had last been heard as the latest attempt started: a silence starts at
    /// most one attempt, and a host heard again ends one it started.
    roam_contact: Option<Instant>,
    roam_events: Vec<RoamEvent>,
}

/// Connects the control stream and queues Start A and B. Blocks up to the connect timeout.
pub fn connect(
    config: DriverConfig,
    caps: HostCapabilities,
    key: &Key,
    options: SessionOptions,
) -> Result<(ControlClient, ControlHandle), ConnectError> {
    let local = config.local.unwrap_or_else(|| match config.remote {
        SocketAddr::V4(_) => SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0),
        SocketAddr::V6(_) => SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), 0),
    });
    let socket = UdpSocket::bind(local)?;
    // Marked before connecting, as the C engine does: the connect retransmissions below take
    // the marking off again for routes that drop it.
    let marking = mark_for_connect(&socket, local.is_ipv6(), &config.warn);
    let waker = Waker::new(socket.local_addr()?)?;
    let control_socket = ControlSocket {
        socket,
        wake_from: waker.socket.local_addr()?,
        roam_inbox: VecDeque::new(),
        intercept: true,
        held_disconnect: None,
        held_unsequenced: false,
        reinject: None,
        last_receive: None,
        marking: Some(marking),
        marked: true,
        send_failures: SendFailures::default(),
        warn: config.warn.clone(),
    };
    let settings = HostSettings {
        peer_limit: 1,
        channel_limit: channel::COUNT,
        ..Default::default()
    };
    let mut host = Host::new(control_socket, settings).map_err(|error| match error {
        rusty_enet::error::HostNewError::FailedToInitializeSocket(error) => ConnectError::Io(error),
        rusty_enet::error::HostNewError::BadParameter(_) => {
            ConnectError::Io(io::ErrorKind::InvalidInput.into())
        }
    })?;
    let peer = host
        .connect(config.remote, channel::COUNT, config.connect_data)
        .map_err(|_| ConnectError::Io(io::ErrorKind::AddrNotAvailable.into()))?
        .id();
    let mut input = InputQueue::new(&caps);
    input.set_unbatched(config.unbatched_input);
    if config.input_disabled {
        input.disable();
    }
    let roaming = config.roaming && caps.client_roaming;
    let shared = Arc::new(Shared {
        input: Mutex::new(input),
        session: Mutex::new(ControlSession::new(caps, key, options)),
        stop: AtomicBool::new(false),
        contact_lost: AtomicBool::new(false),
        rtt: Mutex::new(None),
        roaming,
        roam_request: Mutex::new(None),
        roam_held: AtomicBool::new(false),
        next_socket: Mutex::new(None),
        waker,
    });
    let handle = ControlHandle {
        shared: shared.clone(),
    };
    let deadline = Instant::now() + config.connect_timeout;
    loop {
        match host.service()? {
            Some(Event::Connect { .. }) => break,
            Some(Event::Disconnect { .. }) => return Err(ConnectError::Refused),
            Some(Event::Receive { .. }) => {}
            None => {
                let now = Instant::now();
                if shared.stop.load(Ordering::Acquire) {
                    host.peer_mut(peer).reset();
                    return Err(ConnectError::Interrupted);
                }
                if now >= deadline {
                    host.peer_mut(peer).reset();
                    // No datagram ever left: say why rather than that the host was silent.
                    if let Some(error) = host.socket_mut().send_failures.take_if_none_sent() {
                        return Err(ConnectError::Io(error));
                    }
                    return Err(ConnectError::TimedOut);
                }
                host.socket()
                    .wait((deadline - now).min(MAX_SERVICE_INTERVAL));
            }
        }
    }
    // Send the verify acknowledgement now.
    host.flush();
    let timeout = config.peer_timeout().as_millis().min(u128::from(u32::MAX)) as u32;
    // Back off at most to twice the round trip, and give up after the (grace) timeout.
    host.peer_mut(peer).set_timeout(2, timeout, timeout);
    lock(&shared.session).start(Instant::now());
    let client = ControlClient {
        host,
        peer,
        shared,
        roaming: roaming.then(|| Roaming::new(key, config.peer_timeout())),
        config,
        disconnect_held_since: None,
        disconnect_released_at: None,
        roam_contact: None,
        roam_events: Vec::new(),
    };
    Ok((client, handle))
}

fn packet(data: Vec<u8>, delivery: Delivery) -> Packet {
    let kind = match delivery {
        Delivery::Reliable => PacketKind::Reliable,
        Delivery::Unreliable => PacketKind::Unreliable { sequenced: true },
        Delivery::Unsequenced => PacketKind::Unreliable { sequenced: false },
    };
    Packet::new(data, kind)
}

impl ControlClient {
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.host.socket().socket.local_addr()
    }

    fn contact_lost(&self, now: Instant) -> bool {
        if !self.config.grace_enabled() {
            return false;
        }
        if self.host.peer(self.peer).state() != PeerState::Connected {
            return true;
        }
        self.host
            .socket()
            .last_receive
            .is_some_and(|at| now.duration_since(at) >= INPUT_CONTACT_LOST_AFTER)
    }

    /// Moves queued input and control messages into ENet. Returns the next timer.
    fn transmit(&mut self, now: Instant, drain_input: bool) -> Option<Instant> {
        let contact_lost = self.contact_lost(now);
        self.shared
            .contact_lost
            .store(contact_lost, Ordering::Relaxed);
        let mut input = Vec::new();
        let input_wake = {
            let mut queue = lock(&self.shared.input);
            if drain_input {
                queue.drain(now, contact_lost, &mut input);
                None
            } else {
                queue.poll(now, contact_lost, &mut input)
            }
        };
        let mut outgoing = Vec::new();
        let session_wake = {
            let mut session = lock(&self.shared.session);
            for InputPacket {
                data,
                channel,
                delivery,
            } in input
            {
                let _ = session.send_input(&data, channel, delivery);
            }
            let wake = session.poll(now);
            outgoing.extend(std::iter::from_fn(|| session.poll_transmit()));
            wake
        };
        let peer = self.host.peer_mut(self.peer);
        let channels = peer.channel_count();
        for out in outgoing {
            let channel = if usize::from(out.channel) < channels {
                out.channel
            } else {
                channel::GENERIC
            };
            // A peer that is no longer connected reports a disconnect from service().
            let _ = peer.send(channel, &packet(out.datagram, out.delivery));
        }
        match (input_wake, session_wake) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Services ENet until it has no event. Returns whether any arrived, or an error.
    fn service(&mut self) -> io::Result<bool> {
        let mut any = false;
        while let Some(event) = self.host.service()? {
            any = true;
            let now = Instant::now();
            let mut session = lock(&self.shared.session);
            match event {
                Event::Receive { packet, .. } => {
                    let _ = session.handle_datagram(now, packet.data());
                }
                Event::Disconnect { .. } => session.disconnected(),
                Event::Connect { .. } => {}
            }
            if session.terminated().is_some() {
                break;
            }
        }
        Ok(any)
    }

    fn deliver(&mut self, on_event: &mut impl FnMut(ControlEvent)) -> Option<i32> {
        let (events, terminated) = {
            let mut session = lock(&self.shared.session);
            (session.take_events(), session.terminated())
        };
        for event in events {
            on_event(event);
        }
        for event in self.roam_events.drain(..) {
            on_event(ControlEvent::Roam(event));
        }
        terminated
    }

    /// Runs roaming: the datagrams set aside, a network change or a host silence, the
    /// timers, and the datagrams to send. Returns the next timer.
    fn roam(&mut self, now: Instant) -> Option<Instant> {
        if let Some(socket) = lock(&self.shared.next_socket).take() {
            self.host.socket_mut().replace(socket);
        }
        let inbox = std::mem::take(&mut self.host.socket_mut().roam_inbox);
        let roaming = self.roaming.as_mut()?;
        for datagram in inbox {
            // A dropped datagram is never answered; an attempt left without answers fails.
            let _ = roaming.handle_datagram(now, &datagram);
        }
        let contact = self.host.socket().last_receive;
        let request = lock(&self.shared.roam_request).take();
        if self.shared.roam_held.load(Ordering::Acquire) {
            // No probes from a network roaming may not use: the grace period decides.
            if roaming.trigger().is_some() {
                roaming.give_up(now);
            }
        } else if let Some(flags) = request {
            self.roam_contact = contact;
            roaming.start(now, RoamTrigger::NetworkChanged, flags, OsRng.next_u64());
        } else if roaming.trigger() == Some(RoamTrigger::HostSilent) {
            if contact != self.roam_contact {
                roaming.cancel(now);
            }
        } else if roaming.trigger().is_none()
            && contact != self.roam_contact
            && contact.is_some_and(|at| now.duration_since(at) >= INPUT_CONTACT_LOST_AFTER)
        {
            self.roam_contact = contact;
            roaming.start(now, RoamTrigger::HostSilent, 0, OsRng.next_u64());
        }
        let timer = roaming.poll(now);
        // The host's control port, from whichever address it answers.
        let to = self
            .host
            .peer(self.peer)
            .address()
            .unwrap_or(self.config.remote);
        while let Some(datagram) = roaming.poll_transmit() {
            match self.host.socket().socket.send_to(&datagram, to) {
                Err(error)
                    if !datagram_lost(&error) && error.kind() != io::ErrorKind::WouldBlock =>
                {
                    self.config.warn.say(&format!(
                        "Could not send a roaming datagram to {to}: {error}"
                    ));
                }
                // A lost datagram is retried with the next PROBE.
                _ => {}
            }
        }
        let events = roaming.take_events();
        if events
            .iter()
            .any(|event| matches!(event, RoamEvent::Roamed { .. }))
        {
            // ENet's resend timers backed off during the outage; a ping gets traffic going.
            self.host.peer_mut(self.peer).ping();
        }
        self.roam_events.extend(events);
        timer
    }

    fn update_rtt(&self) {
        let peer = self.host.peer(self.peer);
        let rtt = (peer.state() == PeerState::Connected)
            .then(|| (peer.round_trip_time(), peer.round_trip_time_variance()));
        *lock(&self.shared.rtt) = rtt;
    }

    /// Hands a held host disconnect to ENet once receives have stopped for a moment.
    fn handle_held_disconnect(&mut self, now: Instant, received: bool) {
        let socket = self.host.socket_mut();
        if socket.held_disconnect.is_some() {
            let since = self.disconnect_held_since.get_or_insert(now);
            if received {
                *since = now;
            } else if now.duration_since(*since) >= DISCONNECT_RECEIVE_WAIT {
                socket.release_disconnect();
                self.disconnect_held_since = None;
                self.disconnect_released_at = Some(now);
            }
        } else if let Some(at) = self.disconnect_released_at {
            if now.duration_since(at) >= DISCONNECT_PROCESS_WAIT {
                // ENet ignored it (another session's disconnect); the peer lives on.
                self.disconnect_released_at = None;
                socket.intercept = true;
            }
        }
    }

    /// Runs the stream until [`ControlHandle::stop`] or the host ends it. `on_event` is called
    /// on this thread, outside every lock.
    pub fn run(mut self, mut on_event: impl FnMut(ControlEvent)) -> Outcome {
        loop {
            let now = Instant::now();
            if self.shared.stop.load(Ordering::Acquire) {
                self.teardown(&mut on_event);
                return Outcome::Stopped;
            }
            let timer = self.transmit(now, false);
            let received = match self.service() {
                Ok(received) => received,
                Err(_) => {
                    lock(&self.shared.session).disconnected();
                    true
                }
            };
            let roam_timer = self.roam(Instant::now());
            if let Some(error) = self.deliver(&mut on_event) {
                // The host will not acknowledge a graceful disconnect after terminating.
                self.host.peer_mut(self.peer).disconnect_now(0);
                self.host.flush();
                *lock(&self.shared.rtt) = None;
                return Outcome::Terminated(error);
            }
            self.update_rtt();
            let now = Instant::now();
            self.handle_held_disconnect(now, received);
            if received {
                continue;
            }
            let mut wait = MAX_SERVICE_INTERVAL;
            for timer in [timer, roam_timer].into_iter().flatten() {
                wait = wait.min(timer.saturating_duration_since(now));
            }
            if self.disconnect_held_since.is_some() {
                wait = wait.min(DISCONNECT_RECEIVE_WAIT);
            }
            self.host.socket().wait(wait);
        }
    }

    fn teardown(&mut self, on_event: &mut impl FnMut(ControlEvent)) {
        let now = Instant::now();
        lock(&self.shared.input).stop();
        self.transmit(now, true);
        lock(&self.shared.session).stop();
        let held =
            self.host.socket().held_disconnect.is_some() || self.disconnect_released_at.is_some();
        let peer = self.host.peer_mut(self.peer);
        if held {
            // The host already disconnected; it will not acknowledge ours.
            peer.disconnect_now(0);
            self.host.flush();
        } else if peer.state() == PeerState::Connected {
            // Linger so the host gets the final input (key releases) and our disconnect; a
            // host disconnect during the linger ends it.
            self.host.socket_mut().intercept = false;
            self.host.peer_mut(self.peer).disconnect_later(0);
            let deadline = now + self.config.linger;
            loop {
                match self.host.service() {
                    Ok(Some(Event::Disconnect { .. })) | Err(_) => break,
                    Ok(Some(_)) => continue,
                    Ok(None) => {}
                }
                let now = Instant::now();
                if now >= deadline {
                    self.host.peer_mut(self.peer).reset();
                    break;
                }
                self.host
                    .socket()
                    .wait((deadline - now).min(MAX_SERVICE_INTERVAL));
            }
        }
        *lock(&self.shared.rtt) = None;
        let _ = self.deliver(on_event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A code [`datagram_lost`] takes for packet loss: network unreachable.
    const UNREACHABLE: i32 = if cfg!(windows) { 10051 } else { 101 };

    thread_local! {
        /// Replaces the result of this thread's control socket sends: a byte count, or a raw
        /// OS error.
        static SEND_RESULT: Cell<Option<Result<usize, i32>>> = const { Cell::new(None) };
    }

    pub(super) fn send_result() -> Option<Result<usize, i32>> {
        SEND_RESULT.with(Cell::get)
    }

    fn inject_sends(result: Option<Result<usize, i32>>) {
        SEND_RESULT.with(|injected| injected.set(result));
    }

    /// A control socket on `socket`, with the warnings it gives.
    fn control_socket(
        socket: UdpSocket,
        marking: Option<Marking>,
    ) -> (ControlSocket, Arc<Mutex<Vec<String>>>) {
        let warnings = Arc::new(Mutex::new(Vec::new()));
        let control = ControlSocket {
            socket,
            wake_from: "127.0.0.1:1".parse().unwrap(),
            roam_inbox: VecDeque::new(),
            intercept: false,
            held_disconnect: None,
            held_unsequenced: false,
            reinject: None,
            last_receive: None,
            marked: marking.is_some(),
            marking,
            send_failures: SendFailures::default(),
            warn: Warn::new({
                let warnings = warnings.clone();
                move |m| warnings.lock().unwrap().push(m.to_string())
            }),
        };
        (control, warnings)
    }

    /// A control socket marked for QoS and watching its first connects, aimed at a sink.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn marked_socket() -> (ControlSocket, UdpSocket, Arc<Mutex<Vec<String>>>) {
        let sink = UdpSocket::bind("127.0.0.1:0").unwrap();
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        priority::mark_control_socket(&socket, false, true).unwrap();
        let (control, warnings) = control_socket(
            socket,
            Some(Marking {
                ipv6: false,
                unanswered: 0,
            }),
        );
        (control, sink, warnings)
    }

    /// A socket that takes the type of service but refuses the priority.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    struct RefusesPriority(UdpSocket);

    #[cfg(any(target_os = "linux", target_os = "android"))]
    impl priority::PriorityTarget for RefusesPriority {
        fn set_priority(&self, _value: i32) -> io::Result<()> {
            Err(io::ErrorKind::PermissionDenied.into())
        }

        fn set_tos(&self, ipv6: bool, value: i32) -> io::Result<()> {
            priority::PriorityTarget::set_tos(&self.0, ipv6, value)
        }
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn the_marking_is_watched_even_when_the_priority_was_refused() {
        let socket = RefusesPriority(UdpSocket::bind("127.0.0.1:0").unwrap());
        let warnings = Arc::new(Mutex::new(Vec::new()));
        let warn = Warn::new({
            let warnings = warnings.clone();
            move |m| warnings.lock().unwrap().push(m.to_string())
        });
        let marking = mark_for_connect(&socket, false, &warn);
        assert_eq!(marking.unanswered, 0);
        assert_eq!(warnings.lock().unwrap().len(), 1);
        // The type of service took, so the unmarking below has something to undo.
        assert_eq!(tos(&socket.0), priority::CONTROL_TOS);
        let _ = priority::mark_control_socket(&socket, false, false);
        assert_eq!(tos(&socket.0), 0);
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn tos(socket: &UdpSocket) -> i32 {
        priority::tests::read_option(socket, libc::IPPROTO_IP, libc::IP_TOS)
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn the_marking_comes_off_after_three_unanswered_connects() {
        use rusty_enet::Socket as _;
        let (mut control, sink, warnings) = marked_socket();
        let remote = sink.local_addr().unwrap();
        for _ in 0..3 {
            control.send(remote, b"connect").unwrap();
            assert_eq!(tos(&control.socket), priority::CONTROL_TOS);
        }
        control.send(remote, b"connect").unwrap();
        assert_eq!(tos(&control.socket), 0);
        assert_eq!(
            priority::tests::read_priority(&control.socket),
            0,
            "the priority goes with the marking"
        );
        assert_eq!(warnings.lock().unwrap().len(), 1);
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn an_answer_keeps_the_marking() {
        use rusty_enet::Socket as _;
        let (mut control, sink, warnings) = marked_socket();
        let remote = sink.local_addr().unwrap();
        control.send(remote, b"connect").unwrap();
        // The host answers from its address.
        let ours = control.socket.local_addr().unwrap();
        sink.send_to(b"verify", ours).unwrap();
        let mut buffer = [0u8; MTU_MAX];
        let mut received = false;
        for _ in 0..50 {
            if control.receive(&mut buffer).unwrap().is_some() {
                received = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(received);
        for _ in 0..6 {
            control.send(remote, b"data").unwrap();
        }
        assert_eq!(tos(&control.socket), priority::CONTROL_TOS);
        assert!(warnings.lock().unwrap().is_empty());
    }

    #[test]
    fn a_streak_of_failed_sends_is_reported_once() {
        let to: SocketAddr = "192.0.2.1:47999".parse().unwrap();
        let unreachable = || io::Error::from_raw_os_error(UNREACHABLE);
        assert!(datagram_lost(&unreachable()));
        let mut failures = SendFailures::default();
        assert!(failures.failed(to, unreachable()).is_none());
        assert!(failures.failed(to, unreachable()).is_none());
        let warning = failures
            .failed(to, unreachable())
            .expect("the third failure warns");
        assert!(
            warning.starts_with("Control datagrams to 192.0.2.1:47999 are failing: "),
            "{warning}"
        );
        assert!(failures.failed(to, unreachable()).is_none());
        // A send that gets out ends the streak; the next one is reported again.
        failures.sent();
        assert!(failures.failed(to, unreachable()).is_none());
        assert!(failures.failed(to, unreachable()).is_none());
        assert!(failures.failed(to, unreachable()).is_some());
    }

    #[test]
    fn a_connect_whose_sends_all_failed_reports_the_error() {
        let to: SocketAddr = "192.0.2.1:47999".parse().unwrap();
        let mut failures = SendFailures::default();
        assert!(failures.take_if_none_sent().is_none(), "nothing failed");
        failures.failed(to, io::ErrorKind::AddrNotAvailable.into());
        let error = failures.take_if_none_sent().expect("every send failed");
        assert_eq!(error.kind(), io::ErrorKind::AddrNotAvailable);

        let mut failures = SendFailures::default();
        failures.sent();
        failures.failed(to, io::ErrorKind::AddrNotAvailable.into());
        assert!(
            failures.take_if_none_sent().is_none(),
            "a datagram left, so the host was silent"
        );
    }

    #[test]
    fn control_socket_sends_are_counted() {
        use rusty_enet::Socket as _;
        let sink = UdpSocket::bind("127.0.0.1:0").unwrap();
        let to = sink.local_addr().unwrap();
        let (mut control, warnings) = control_socket(UdpSocket::bind("127.0.0.1:0").unwrap(), None);
        inject_sends(Some(Err(UNREACHABLE)));
        for _ in 0..3 {
            assert_eq!(
                control.send(to, b"connect").unwrap(),
                0,
                "a loss, not an error"
            );
        }
        assert_eq!(
            *warnings.lock().unwrap(),
            [format!(
                "Control datagrams to {to} are failing: {}",
                io::Error::from_raw_os_error(UNREACHABLE)
            )]
        );
        // A full send buffer (WouldBlock) is not a datagram that left.
        inject_sends(Some(Ok(0)));
        control.send(to, b"connect").unwrap();
        assert!(!control.send_failures.any_sent);
        inject_sends(None);
        assert!(control.send(to, b"connect").unwrap() > 0);
        assert!(control.send_failures.any_sent);
        assert_eq!(control.send_failures.consecutive, 0);
    }

    #[test]
    fn a_datagram_too_short_for_enet_is_not_the_host() {
        use rusty_enet::Socket as _;
        let (mut control, _) = control_socket(UdpSocket::bind("127.0.0.1:0").unwrap(), None);
        control.socket.set_nonblocking(true).unwrap();
        let ours = control.socket.local_addr().unwrap();
        let stray = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut buffer = [0u8; MTU_MAX];
        let mut receive = |control: &mut ControlSocket| {
            for _ in 0..50 {
                if control.receive(&mut buffer).unwrap().is_some() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("nothing arrived");
        };
        stray.send_to(&[0], ours).unwrap();
        receive(&mut control);
        assert!(control.last_receive.is_none());
        stray.send_to(&[0, 0], ours).unwrap();
        receive(&mut control);
        assert!(control.last_receive.is_some());
    }

    #[test]
    fn a_connect_none_of_whose_datagrams_left_fails_with_the_error() {
        let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
        let config = DriverConfig {
            connect_timeout: Duration::from_millis(300),
            ..DriverConfig::new(silent.local_addr().unwrap())
        };
        let caps = HostCapabilities {
            app_version: [7, 1, 431, -1],
            feature_flags: 3,
            encryption_enabled: super::super::negotiation::encryption::CONTROL_V2,
            control_messages: Vec::new(),
            client_roaming: false,
        };
        inject_sends(Some(Err(UNREACHABLE)));
        let result = connect(config, caps, &[0; 16], SessionOptions::default());
        inject_sends(None);
        match result {
            Err(ConnectError::Io(error)) => assert_eq!(error.raw_os_error(), Some(UNREACHABLE)),
            Err(other) => panic!("{other}"),
            Ok(_) => panic!("connected without sending"),
        }
    }

    #[test]
    fn the_waker_aims_at_the_control_socket() {
        let target = |waker: &Waker| waker.socket.peer_addr().unwrap();
        let waker = Waker::new("127.0.0.1:47000".parse().unwrap()).unwrap();
        assert_eq!(target(&waker), "127.0.0.1:47000".parse().unwrap());
        let waker = Waker::new("0.0.0.0:47000".parse().unwrap()).unwrap();
        assert_eq!(target(&waker), "127.0.0.1:47000".parse().unwrap());
        // A moved control socket.
        waker.retarget("0.0.0.0:47001".parse().unwrap()).unwrap();
        assert_eq!(target(&waker), "127.0.0.1:47001".parse().unwrap());
        let Ok(waker) = Waker::new("[::]:47000".parse().unwrap()) else {
            eprintln!("skipped: no IPv6 loopback");
            return;
        };
        assert_eq!(target(&waker), "[::1]:47000".parse().unwrap());
    }

    #[test]
    fn disconnect_datagrams_are_recognized() {
        // Reliable disconnect with a sent time: header 4, command 0x84 (acknowledge flag).
        assert_eq!(
            enet_disconnect(&[0x80, 0x00, 0, 1, 0x84, 0xff, 0, 1, 0, 0, 0, 0]),
            Some(false)
        );
        // enet_peer_disconnect_now: unsequenced, no sent time.
        assert_eq!(
            enet_disconnect(&[0x10, 0x00, 0x44, 0xff, 0, 0, 0, 0, 0, 0]),
            Some(true)
        );
        // Addressed to another peer, compressed, another command or another length.
        assert_eq!(
            enet_disconnect(&[0x00, 0x01, 0x44, 0xff, 0, 0, 0, 0, 0, 0]),
            None
        );
        assert_eq!(
            enet_disconnect(&[0x40, 0x00, 0x44, 0xff, 0, 0, 0, 0, 0, 0]),
            None
        );
        assert_eq!(
            enet_disconnect(&[0x00, 0x00, 0x46, 0xff, 0, 0, 0, 0, 0, 0]),
            None
        );
        assert_eq!(
            enet_disconnect(&[0x00, 0x00, 0x44, 0xff, 0, 0, 0, 0, 0]),
            None
        );
    }
}
