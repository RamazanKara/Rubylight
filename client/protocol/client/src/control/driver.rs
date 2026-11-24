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
//! unless the host already sent a disconnect or a termination message, or has been silent too
//! long to send one.
//!
//! When the host advertised roaming and [`DriverConfig::roaming`] is on, the driver also runs
//! [`Roaming`] beside ENet on the same socket: it sets roaming datagrams aside before ENet
//! sees them, and starts an attempt on [`ControlHandle::network_changed`] or after a second
//! without a word from the host.

mod handle;
mod socket;
mod waker;

pub use handle::{ConnectError, ControlHandle, DriverConfig, Outcome, Warn};

use std::collections::VecDeque;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use handle::{lock, Shared};
#[cfg(test)]
use socket::{enet_disconnect, poll_millis, Marking};
use socket::{datagram_lost, mark_for_connect, ControlSocket, SendFailures};
#[cfg(test)]
use waker::wake_bind;
use waker::Waker;

use rand_core::{OsRng, RngCore};
#[cfg(test)]
use rubylight_protocol::roaming::PROBE_FLAG_MEDIA_REBOUND;
#[cfg(test)]
use rusty_enet::{PacketReceived, MTU_MAX};
use rusty_enet::{Event, Host, HostSettings, Packet, PacketKind, PeerID, PeerState};

use super::events::ControlEvent;
use super::negotiation::HostCapabilities;
use super::session::{ControlSession, SessionOptions};
use super::wire::{channel, Delivery};
use crate::crypto::Key;
use crate::input::{InputPacket, InputQueue};
use crate::priority;
use crate::roaming::{RoamEvent, RoamTrigger, Roaming};

/// The ENet peer timeout without a network grace period.
pub const CONTROL_PEER_TIMEOUT: Duration = Duration::from_secs(10);
/// With a grace period, input is held back once nothing has come from the host this long.
pub const INPUT_CONTACT_LOST_AFTER: Duration = Duration::from_millis(1000);
/// While ENet's reliable window is full, input stays in the [`InputQueue`] (where a controller
/// state updates in place and motion merges) until it holds this many entries.
const INPUT_HOLD_LIMIT: usize = crate::input::queue::MAX_QUEUED / 2;
/// Receives that may still follow a host's disconnect are awaited this long.
const DISCONNECT_RECEIVE_WAIT: Duration = Duration::from_millis(100);
/// A disconnect handed back to ENet that ENet ignored (another session's) is forgotten after.
const DISCONNECT_PROCESS_WAIT: Duration = Duration::from_secs(1);
/// A host silent this long at the stop gets no linger: it will not acknowledge the disconnect. A
/// short blip toward a live host stays below it, so a clean stop still reaches it.
const SILENT_STOP_AFTER: Duration = Duration::from_secs(2);
/// Longest sleep between control-thread services. A datagram ends the wait at once and ENet's own
/// resend and ping deadlines end it when sooner; this cap only bounds what neither reports, such
/// as the lost-block report's early deadline or a timer that moved after the wait began.
const MAX_IDLE_WAIT: Duration = Duration::from_millis(10);
/// After a failed roaming attempt, the next silence starts another this long later at the
/// earliest.
const ROAM_RETRY_AFTER: Duration = Duration::from_secs(10);

/// How long the control thread may sleep before its next service: the earliest of ENet's next
/// resend or ping (`enet`) and the session and roaming timers, capped at `MAX_IDLE_WAIT`. A timer
/// already due gives zero, so a due timer is served at once.
fn service_wait(now: Instant, enet: Option<Duration>, timers: [Option<Instant>; 2]) -> Duration {
    let mut wait = MAX_IDLE_WAIT;
    if let Some(enet) = enet {
        wait = wait.min(enet);
    }
    for timer in timers.into_iter().flatten() {
        wait = wait.min(timer.saturating_duration_since(now));
    }
    wait
}

/// Whether the stop's linger would wait for nothing: a host silent past [`SILENT_STOP_AFTER`]
/// (ENet's pings go out several times a second) will not acknowledge the disconnect either.
fn lingers_for_nothing(last_receive: Option<Instant>, now: Instant) -> bool {
    last_receive.is_some_and(|at| now.saturating_duration_since(at) >= SILENT_STOP_AFTER)
}

/// Whether an [`Event::Disconnect`] at `now` is the host's goodbye: a host disconnect went to ENet
/// just before, and ENet took it. A disconnect ENet ignored (another session's) is forgotten after
/// [`DISCONNECT_PROCESS_WAIT`], so a timeout later is no goodbye.
fn is_goodbye(disconnect_handed: Option<Instant>, now: Instant) -> bool {
    disconnect_handed.is_some_and(|at| now.saturating_duration_since(at) < DISCONNECT_PROCESS_WAIT)
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
    roam_triggers: RoamTriggers,
    roam_events: Vec<RoamEvent>,
    /// Taken input, kept so that each pass allocates nothing.
    input: Vec<InputPacket>,
    /// Queued control datagrams, kept for the same reason.
    outgoing: Vec<super::session::Outgoing>,
    /// Reusable plaintext scratch so input packets seal in place into their existing `Vec<u8>`.
    seal_buf: Vec<u8>,
    /// Input was left queued because ENet's reliable window was full.
    input_waits_for_window: bool,
}

/// When the driver starts, restarts and ends roaming attempts.
#[derive(Debug, Default)]
struct RoamTriggers {
    /// When the host had last been heard as the latest attempt started: a silence starts at
    /// most one attempt, and a host heard again ends one it started.
    contact: Option<Instant>,
    /// After a failed attempt, no silence starts another before this.
    retry_at: Option<Instant>,
    /// Roaming is held ([`ControlHandle::hold_roaming`]).
    held: bool,
}

impl RoamTriggers {
    /// Acts on a network change (`request`, its PROBE flags), the host last heard at
    /// `contact`, and the hold on roaming.
    fn update(
        &mut self,
        roaming: &mut Roaming,
        now: Instant,
        contact: Option<Instant>,
        request: Option<u32>,
        held: bool,
    ) {
        let hold_starts = held && !self.held;
        self.held = held;
        if held {
            // No probes from a network roaming may not use: the grace period decides. Not a
            // failure, so no backoff after it.
            if hold_starts {
                roaming.hold();
                // Once lifted, a silence that lasts starts an attempt again.
                self.contact = None;
            }
        } else if let Some(flags) = request {
            self.contact = contact;
            roaming.start(now, RoamTrigger::NetworkChanged, flags, OsRng.next_u64());
        } else if roaming.trigger() == Some(RoamTrigger::HostSilent) {
            if contact != self.contact {
                roaming.cancel(now);
            }
        } else if roaming.trigger().is_none()
            && contact != self.contact
            && contact.is_some_and(|at| now.duration_since(at) >= INPUT_CONTACT_LOST_AFTER)
            && !self.retry_at.is_some_and(|at| now < at)
        {
            self.contact = contact;
            roaming.start(now, RoamTrigger::HostSilent, 0, OsRng.next_u64());
        }
    }

    /// After a failed attempt the peer may live on through a long grace period: the next
    /// silence may start another attempt, [`ROAM_RETRY_AFTER`] later at the earliest.
    fn ended(&mut self, now: Instant, events: &[RoamEvent]) {
        if events
            .iter()
            .any(|event| matches!(event, RoamEvent::Failed { .. }))
        {
            self.contact = None;
            self.retry_at = Some(now + ROAM_RETRY_AFTER);
        }
    }
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
    // Marked before connecting, as the former C engine did: the connect retransmissions below take
    // the marking off again for routes that drop it.
    let marking = mark_for_connect(&socket, local.is_ipv6(), &config.warn);
    let bound = socket.local_addr()?;
    let waker = Waker::new(bound)?;
    let roaming = config.roaming && caps.client_roaming;
    let control_socket = ControlSocket {
        socket,
        roam_inbox: VecDeque::new(),
        roaming,
        intercept: true,
        held_disconnect: None,
        held_unsequenced: false,
        reinject: None,
        last_receive: None,
        disconnect_handed: None,
        marking: Some(marking),
        marked: true,
        flow: priority::open_control_flow(|m| config.warn.say(m)),
        send_failures: SendFailures::default(),
        warn: config.warn.clone(),
    };
    let settings = HostSettings {
        peer_limit: 1,
        channel_limit: channel::COUNT,
        round_up_srtt: true,
        lenient_peer_address: true,
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
    input.set_frame_warp(config.frame_warp.clone());
    if config.input_disabled {
        input.disable();
    }
    let shared = Arc::new(Shared {
        input: Mutex::new(input),
        session: Mutex::new({
            let mut session = ControlSession::new(caps, key, options);
            session.set_frame_warp(config.frame_warp.clone());
            session
        }),
        stop: AtomicBool::new(false),
        wake_pending: AtomicBool::new(false),
        contact_lost: AtomicBool::new(false),
        rtt: Mutex::new(None),
        goodbye: AtomicBool::new(false),
        roaming,
        roam_request: Mutex::new(None),
        roam_held: AtomicBool::new(false),
        next_socket: Mutex::new(None),
        local: Mutex::new(bound),
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
                let wait = service_wait(now, host.next_service_in(peer), [Some(deadline), None]);
                host.socket().wait(wait);
            }
        }
    }
    // Send the verify acknowledgement now.
    host.flush();
    let timeout = config.peer_timeout().as_millis().min(u128::from(u32::MAX)) as u32;
    // Back off at most to twice the round trip, and give up after the (grace) timeout.
    host.peer_mut(peer).set_timeout(2, timeout, timeout);
    // ENet's packet throttle shrinks the reliable window toward one MTU (and drops unreliable
    // packets) for every acknowledgement slower than the last round trip plus twice its
    // variance: jitter on a mobile or VPN link, not congestion. Control traffic is a few kB/s,
    // so it keeps the full window and input leaves at once instead of after the next
    // acknowledgement (nightjar#97).
    host.peer_mut(peer).set_throttle(
        rusty_enet::consts::PEER_PACKET_THROTTLE_INTERVAL,
        rusty_enet::consts::PEER_PACKET_THROTTLE_ACCELERATION,
        0,
    );
    lock(&shared.session).start(Instant::now());
    let client = ControlClient {
        host,
        peer,
        shared,
        roaming: roaming
            .then(|| Roaming::with_seq(key, config.peer_timeout(), config.roam_seq.clone())),
        config,
        disconnect_held_since: None,
        disconnect_released_at: None,
        roam_triggers: RoamTriggers::default(),
        roam_events: Vec::new(),
        input: Vec::new(),
        outgoing: Vec::new(),
        seal_buf: Vec::new(),
        input_waits_for_window: false,
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
        self.input.clear();
        // ENet sends reliable data only while what is in transit fits the throttled window; past
        // it, every packet handed over waits there unchanged and leaves in a burst at the next
        // acknowledgement. On a jittery 60 ms link that held about 20 stale controller states
        // and sent them all late (nightjar#97). Leave input in the queue instead, where a newer
        // controller state replaces the queued one, as moonlight-common-c waits for its packet to
        // leave and merges the newer state meanwhile.
        let window_full = self.host.peer(self.peer).has_unsent_reliable();
        let input_wake = {
            let mut queue = lock(&self.shared.input);
            self.input_waits_for_window = false;
            if drain_input {
                queue.drain(now, contact_lost, &mut self.input);
                None
            } else if window_full && !contact_lost && queue.len() < INPUT_HOLD_LIMIT {
                // The acknowledgement that opens the window wakes the loop (see `run`).
                self.input_waits_for_window = !queue.is_empty();
                None
            } else {
                queue.poll(now, contact_lost, &mut self.input)
            }
        };
        self.outgoing.clear();
        let session_wake = {
            let mut session = lock(&self.shared.session);
            for InputPacket {
                data,
                channel,
                delivery,
            } in self.input.drain(..)
            {
                self.seal_buf.clear();
                self.seal_buf.extend_from_slice(&data);
                let _ = session.send_input_into(&self.seal_buf, data, channel, delivery);
            }
            let wake = session.poll(now);
            self.outgoing
                .extend(std::iter::from_fn(|| session.poll_transmit()));
            wake
        };
        let peer = self.host.peer_mut(self.peer);
        let channels = peer.channel_count();
        for out in self.outgoing.drain(..) {
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
                Event::Disconnect { .. } => {
                    // Before the termination it brings, so whoever reports it can tell.
                    let goodbye = is_goodbye(self.host.socket().disconnect_handed, now);
                    self.shared.goodbye.store(goodbye, Ordering::SeqCst);
                    session.disconnected();
                }
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
        // With roaming off no socket moves, no roaming datagram is kept and none goes out.
        let roaming = self.roaming.as_mut()?;
        if let Some(socket) = lock(&self.shared.next_socket).take() {
            self.host.socket_mut().replace(socket);
        }
        let inbox = std::mem::take(&mut self.host.socket_mut().roam_inbox);
        for datagram in inbox {
            // A dropped datagram is never answered; an attempt left without answers fails.
            let _ = roaming.handle_datagram(now, &datagram);
        }
        let contact = self.host.socket().last_receive;
        let request = lock(&self.shared.roam_request).take();
        let held = self.shared.roam_held.load(Ordering::Acquire);
        self.roam_triggers
            .update(roaming, now, contact, request, held);
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
        self.roam_triggers.ended(now, &events);
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
        } else if let Some(at) = self.disconnect_released_at
            && now.duration_since(at) >= DISCONNECT_PROCESS_WAIT
        {
            // ENet ignored it (another session's disconnect); the peer lives on.
            self.disconnect_released_at = None;
            socket.intercept = true;
            socket.disconnect_handed = None;
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
            self.shared.wake_pending.store(false, Ordering::Release);
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
            if self.input_waits_for_window && !self.host.peer(self.peer).has_unsent_reliable() {
                // An acknowledgement opened the window: send the input that waited for it.
                continue;
            }
            if self.shared.wake_pending.swap(false, Ordering::AcqRel) {
                continue;
            }
            let wait = service_wait(
                now,
                self.host.next_service_in(self.peer),
                [timer, roam_timer],
            );
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
        let silent = lingers_for_nothing(self.host.socket().last_receive, now);
        let peer = self.host.peer_mut(self.peer);
        if held || silent {
            // The host already disconnected, or has been silent too long to acknowledge ours (a PC
            // that is gone): waiting out the linger would only hold the stop up.
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
                let wait = service_wait(
                    now,
                    self.host.next_service_in(self.peer),
                    [Some(deadline), None],
                );
                self.host.socket().wait(wait);
            }
        }
        *lock(&self.shared.rtt) = None;
        let _ = self.deliver(on_event);
    }
}

#[cfg(test)]
mod tests;
