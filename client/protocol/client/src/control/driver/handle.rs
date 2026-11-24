//! Control driver configuration, errors, shared thread state and [`ControlHandle`].

use std::io;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rubylight_protocol::roaming::PROBE_FLAG_MEDIA_REBOUND;

use super::waker::Waker;
use super::CONTROL_PEER_TIMEOUT;
use super::super::messages::ClientMessage;
use super::super::session::{ControlSession, SendError};
use crate::input::InputQueue;
use crate::roaming::LaunchSeq;

/// Where the driver reports a problem that does not stop the stream (a socket option the OS
/// refused).
#[derive(Clone)]
pub struct Warn(Arc<dyn Fn(&str) + Send + Sync>);

impl Warn {
    pub fn new(warn: impl Fn(&str) + Send + Sync + 'static) -> Self {
        Self(Arc::new(warn))
    }

    pub(super) fn say(&self, message: &str) {
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
    /// roaming ([`super::HostCapabilities::client_roaming`]).
    pub roaming: bool,
    /// The roaming seqs sent under this launch key; a reconnect under the same key passes the
    /// earlier connection's, since the host rejects seqs it has seen.
    pub roam_seq: LaunchSeq,
    pub warn: Warn,
    /// Frame Warp's shared state: counts input as it leaves and takes the host's messages.
    pub frame_warp: Option<Arc<crate::frame_warp::FrameWarp>>,
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
            roam_seq: LaunchSeq::default(),
            warn: Warn::default(),
            frame_warp: None,
        }
    }

    pub(super) fn peer_timeout(&self) -> Duration {
        self.network_grace.max(CONTROL_PEER_TIMEOUT)
    }

    pub(super) fn grace_enabled(&self) -> bool {
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

/// How [`super::ControlClient::run`] ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// [`ControlHandle::stop`] ended it.
    Stopped,
    /// The host ended it, or the connection failed; the code is also delivered as
    /// [`super::ControlEvent::Terminated`].
    Terminated(i32),
}

pub(super) struct Shared {
    pub(super) session: Mutex<ControlSession>,
    pub(super) input: Mutex<InputQueue>,
    pub(super) stop: AtomicBool,
    /// A producer asked for a service after the last transmit: the control thread continues
    /// instead of waiting, even when its wake datagram was drained with the session's.
    pub(super) wake_pending: AtomicBool,
    pub(super) contact_lost: AtomicBool,
    pub(super) rtt: Mutex<Option<(Duration, Duration)>>,
    /// The connection ended with the host's disconnect ([`ControlHandle::said_goodbye`]).
    pub(super) goodbye: AtomicBool,
    /// Both sides have roaming on.
    pub(super) roaming: bool,
    /// PROBE flags of a [`ControlHandle::network_changed`] the control thread has not taken.
    pub(super) roam_request: Mutex<Option<u32>>,
    /// [`ControlHandle::hold_roaming`]: no attempts.
    pub(super) roam_held: AtomicBool,
    /// A socket from [`ControlHandle::move_to`] the control thread has not moved to.
    pub(super) next_socket: Mutex<Option<UdpSocket>>,
    /// Where the control socket is bound, or moving to.
    pub(super) local: Mutex<SocketAddr>,
    pub(super) waker: Waker,
}

pub(super) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Thread-safe access to a running control stream.
#[derive(Clone)]
pub struct ControlHandle {
    pub(super) shared: Arc<Shared>,
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
        if !self.shared.wake_pending.swap(true, Ordering::AcqRel) {
            self.shared.waker.wake();
        }
        result
    }

    /// Runs `f` on the session without waking the control thread, for state that waits for
    /// its next service (at most `MAX_IDLE_WAIT`), such as per-frame connection quality.
    pub fn with_session_quiet<R>(&self, f: impl FnOnce(&mut ControlSession) -> R) -> R {
        f(&mut lock(&self.shared.session))
    }

    /// Runs `f` on the input queue and wakes the control thread.
    pub fn with_input<R>(&self, f: impl FnOnce(&mut InputQueue) -> R) -> R {
        let result = f(&mut lock(&self.shared.input));
        if !self.shared.wake_pending.swap(true, Ordering::AcqRel) {
            self.shared.waker.wake();
        }
        result
    }

    /// ENet's round-trip time and its variance, while connected.
    pub fn round_trip_time(&self) -> Option<(Duration, Duration)> {
        *lock(&self.shared.rtt)
    }

    /// Whether the connection ended because this session's host disconnected (with or without a
    /// reason), rather than going silent past the timeout.
    pub fn said_goodbye(&self) -> bool {
        self.shared.goodbye.load(Ordering::SeqCst)
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
    /// one without the opt-in): the running attempt ends with [`crate::roaming::RoamEvent::Held`],
    /// not as a failure, none starts, and the network grace period decides. False lets roaming run
    /// again.
    pub fn hold_roaming(&self, held: bool) {
        self.shared.roam_held.store(held, Ordering::Release);
        self.shared.waker.wake();
    }

    /// Where the control socket is bound, or moving to after [`ControlHandle::move_to`].
    pub fn local_addr(&self) -> SocketAddr {
        *lock(&self.shared.local)
    }

    /// Moves the control socket to a new port on `local`, as a network change does; a
    /// [`ControlHandle::network_changed`] should follow. Fails unless both sides have
    /// roaming on.
    pub fn move_to(&self, local: IpAddr) -> io::Result<()> {
        let socket = self.bind_move(local)?;
        self.move_to_socket(socket)
    }

    /// Binds the socket [`ControlHandle::move_to`] moves to, so a caller moving other sockets
    /// too can bind them all before anything moves.
    pub fn bind_move(&self, local: IpAddr) -> io::Result<UdpSocket> {
        if !self.shared.roaming {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "roaming is off for this session",
            ));
        }
        if self.local_addr().is_ipv4() != local.is_ipv4() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the control socket cannot change address family",
            ));
        }
        let socket = UdpSocket::bind(SocketAddr::new(local, 0))?;
        // What rusty_enet's init sets on a UdpSocket.
        socket.set_nonblocking(true)?;
        socket.set_broadcast(true)?;
        Ok(socket)
    }

    /// Moves the control socket to `socket`, from [`ControlHandle::bind_move`].
    pub fn move_to_socket(&self, socket: UdpSocket) -> io::Result<()> {
        let address = socket.local_addr()?;
        *lock(&self.shared.next_socket) = Some(socket);
        *lock(&self.shared.local) = address;
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
