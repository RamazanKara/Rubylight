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

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

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
use crate::priority::{self, Qos};

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

struct Waker {
    socket: UdpSocket,
    target: SocketAddr,
}

impl Waker {
    fn new(enet_local: SocketAddr) -> io::Result<Self> {
        let ip = enet_local.ip();
        let (bind, target_ip): (IpAddr, IpAddr) = if ip.is_unspecified() {
            match ip {
                IpAddr::V4(_) => (Ipv4Addr::LOCALHOST.into(), Ipv4Addr::LOCALHOST.into()),
                IpAddr::V6(_) => (Ipv6Addr::LOCALHOST.into(), Ipv6Addr::LOCALHOST.into()),
            }
        } else {
            (ip, ip)
        };
        let socket = UdpSocket::bind(SocketAddr::new(bind, 0))?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            target: SocketAddr::new(target_ip, enet_local.port()),
        })
    }

    fn wake(&self) {
        let _ = self.socket.send_to(&[0], self.target);
    }
}

struct Shared {
    session: Mutex<ControlSession>,
    input: Mutex<InputQueue>,
    stop: AtomicBool,
    contact_lost: AtomicBool,
    rtt: Mutex<Option<(Duration, Duration)>>,
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

/// The ENet socket: drops wake datagrams, notes when the host was last heard, and holds back
/// a host disconnect so receives queued before it are not lost (Nightjar's disconnect
/// intercept), handing it to ENet once they are processed.
struct ControlSocket {
    socket: UdpSocket,
    remote: SocketAddr,
    wake_from: SocketAddr,
    intercept: bool,
    held_disconnect: Option<(SocketAddr, Vec<u8>)>,
    held_unsequenced: bool,
    reinject: Option<(SocketAddr, Vec<u8>)>,
    last_receive: Option<Instant>,
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
}

impl rusty_enet::Socket for ControlSocket {
    type Address = SocketAddr;
    type Error = io::Error;

    fn init(&mut self, options: rusty_enet::SocketOptions) -> io::Result<()> {
        rusty_enet::Socket::init(&mut self.socket, options)
    }

    fn send(&mut self, address: SocketAddr, buffer: &[u8]) -> io::Result<usize> {
        match rusty_enet::Socket::send(&mut self.socket, address, buffer) {
            Err(error) if datagram_lost(&error) => Ok(0),
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
            if address == self.remote {
                self.last_receive = Some(Instant::now());
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
    // The control socket has no QoS class of its own; "prioritize stream traffic" raises it.
    let _ = priority::prioritize_socket(&socket, Qos::BestEffort);
    let waker = Waker::new(socket.local_addr()?)?;
    let control_socket = ControlSocket {
        socket,
        remote: config.remote,
        wake_from: waker.socket.local_addr()?,
        intercept: true,
        held_disconnect: None,
        held_unsequenced: false,
        reinject: None,
        last_receive: None,
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
    let shared = Arc::new(Shared {
        input: Mutex::new(input),
        session: Mutex::new(ControlSession::new(caps, key, options)),
        stop: AtomicBool::new(false),
        contact_lost: AtomicBool::new(false),
        rtt: Mutex::new(None),
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
        config,
        disconnect_held_since: None,
        disconnect_released_at: None,
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

    fn deliver(&self, on_event: &mut impl FnMut(ControlEvent)) -> Option<i32> {
        let (events, terminated) = {
            let mut session = lock(&self.shared.session);
            (session.take_events(), session.terminated())
        };
        for event in events {
            on_event(event);
        }
        terminated
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
            if let Some(timer) = timer {
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
