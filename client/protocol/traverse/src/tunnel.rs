//! One punched path between a device and its host, and what runs over it.
//!
//! On the device ([`Bridge::Device`]) the tunnel binds the host's own TCP ports (HTTP, HTTPS,
//! RTSP) and media ports (video, audio, control, microphone) on a loopback address. The client
//! talks to that address as if it were the host: each TCP connection becomes a stream of the
//! reliable layer, each media datagram goes through sealed as it came.
//!
//! On the host ([`Bridge::Host`]) each stream becomes a TCP connection to the host's own port,
//! and each media channel a UDP socket that sends to the host's media port. Both connect from a
//! loopback address of the device's own, so the host tells tunnelled devices apart the way it
//! tells devices apart on the network: by address.

use crate::{
    node::{self, Node},
    peers,
    punch::{self, Puncher},
    record::{PathKeys, Ports, Role},
    stream::{Event, Reliable, StreamId},
    wire::{self, Kind, Opener, Sealer, CHANNELS},
    Millis,
};
use std::{
    collections::HashMap,
    io::{self, Read, Write},
    net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream, UdpSocket},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Condvar, Mutex, RwLock,
    },
    thread,
    time::Duration,
};

/// The stream targets: which TCP port of the host a stream reaches.
pub const TARGET_HTTP: u8 = 0;
pub const TARGET_HTTPS: u8 = 1;
pub const TARGET_RTSP: u8 = 2;

/// A connected path sends a keepalive when it has sent nothing for this long: NATs drop idle
/// UDP mappings after 30 s or more.
const KEEPALIVE: Millis = 10_000;
/// A connected path that heard something within this long is [`Tunnel::healthy`]: the peer's
/// keepalive (every [`KEEPALIVE`]) and a margin for a lost one.
pub const HEALTHY_FOR: Millis = KEEPALIVE + 5_000;
/// A path that hears nothing for this long is gone.
const IDLE: Millis = 45_000;
/// The device closes a path that carried no connection and no media for this long: the host
/// list's polls and streams keep it up, and a later connect punches again.
const UNUSED: Millis = 120_000;
/// The path thread sleeps until its next deadline (a punch, a retransmission, a keepalive, a
/// timeout), or until something earlier is due, but never longer than this.
const LONGEST_SLEEP: Millis = KEEPALIVE;

fn tcp_port(ports: &Ports, target: u8) -> Option<u16> {
    match target {
        TARGET_HTTP => Some(ports.http),
        TARGET_HTTPS => Some(ports.https),
        TARGET_RTSP => Some(ports.rtsp),
        _ => None,
    }
}

fn media_port(ports: &Ports, channel: u8) -> Option<u16> {
    match channel {
        wire::CHANNEL_VIDEO => Some(ports.video),
        wire::CHANNEL_AUDIO => Some(ports.audio),
        wire::CHANNEL_CONTROL => Some(ports.control),
        wire::CHANNEL_MIC => Some(ports.mic),
        _ => None,
    }
}

/// What the tunnel connects to on its side.
#[derive(Clone, Debug)]
pub enum Bridge {
    /// The device: bind the host's ports at `local`, a loopback address.
    Device { local: IpAddr, ports: Ports },
    /// The host: reach the host's ports at `service` from `source`, the device's loopback
    /// address on this PC. Every socket it opens is registered in [`crate::peers`] under
    /// `device` before it sends, so the host never trusts it as a local peer.
    Host {
        service: IpAddr,
        source: IpAddr,
        ports: Ports,
        device: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Punching,
    Connected(SocketAddr),
    /// Gone, and why.
    Closed(String),
}

enum Outgoing {
    Data(Vec<u8>),
    Finish,
    Reset,
}

struct Connection {
    writer: mpsc::Sender<Outgoing>,
    /// The peer finished its side, and so did ours.
    finished_in: bool,
    finished_out: bool,
}

struct Inner {
    puncher: Puncher,
    reliable: Reliable,
    peer: Option<SocketAddr>,
    last_sent: Millis,
    connections: HashMap<StreamId, Connection>,
    closed: Option<String>,
}

struct Shared {
    node: Node,
    role: Role,
    bridge: Bridge,
    sealer: Sealer,
    /// Apart from `inner`, so decrypting a datagram never waits on the reliable layer.
    opener: Mutex<Opener>,
    inner: Mutex<Inner>,
    /// The peer while the path is connected and open, else None: `inner`'s state, kept here so
    /// media needs no `inner` lock. Written only with `inner` held ([`Tunnel::publish`]).
    connected: RwLock<Option<SocketAddr>>,
    /// When an authenticated datagram last came in.
    last_heard: AtomicU64,
    /// When the path last carried media or a stream segment, or a connection opened.
    last_used: AtomicU64,
    capacity: Condvar,
    /// Wakes the path thread ([`Tunnel::wake_by`]); `woken` says it was woken.
    wake: Condvar,
    woken: Mutex<bool>,
    /// What the path thread sleeps until, u64::MAX while it is awake.
    sleeping_until: AtomicU64,
    /// When the device's session began on its clock (unix seconds, 0 unknown): from the stamp
    /// of the punch that started the path, or the device's record. The host replaces a connected
    /// path only for a session that began later ([`Tunnel::set_epoch`]).
    epoch: AtomicU64,
    /// How often the path thread went round.
    #[cfg(test)]
    rounds: AtomicU64,
    stopped: AtomicBool,
    /// The device's media sockets, and where its client last sent from on each.
    media: Vec<Option<MediaEnd>>,
    listeners: Mutex<Vec<SocketAddr>>,
}

struct MediaEnd {
    socket: UdpSocket,
    client: RwLock<Option<SocketAddr>>,
    /// The host side's registration of this socket as a tunnelled peer.
    _mark: Option<peers::Mark>,
}

#[derive(Clone)]
pub struct Tunnel {
    shared: Arc<Shared>,
}

impl Tunnel {
    /// Starts a path: binds the bridge's sockets, then punches `candidates` while the node's
    /// handler feeds it datagrams ([`Tunnel::receive`]). `session` is the device's session nonce.
    pub fn start(
        node: Node,
        role: Role,
        keys: PathKeys,
        session: [u8; 16],
        candidates: Vec<SocketAddr>,
        bridge: Bridge,
    ) -> io::Result<Self> {
        let (send_key, receive_key) = match role {
            Role::Host => (keys.host_to_client, keys.client_to_host),
            Role::Client => (keys.client_to_host, keys.host_to_client),
        };
        let now = node.now();
        let mut media = Vec::with_capacity(CHANNELS);
        for channel in 0..CHANNELS as u8 {
            media.push(match &bridge {
                Bridge::Device { local, ports } => {
                    match media_port(ports, channel).filter(|port| *port != 0) {
                        Some(port) => Some(MediaEnd {
                            socket: node::bind_udp(SocketAddr::new(*local, port))?,
                            client: RwLock::new(None),
                            _mark: None,
                        }),
                        None => None,
                    }
                }
                Bridge::Host { source, device, .. } => {
                    let socket = node::bind_udp(SocketAddr::new(*source, 0))?;
                    // Registered before the first datagram reaches the host's media ports.
                    let mark = peers::register(socket.local_addr()?, device);
                    Some(MediaEnd {
                        socket,
                        client: RwLock::new(None),
                        _mark: Some(mark),
                    })
                }
            });
        }
        for end in media.iter().flatten() {
            end.socket
                .set_read_timeout(Some(Duration::from_millis(500)))?;
        }
        let shared = Arc::new(Shared {
            role,
            bridge: bridge.clone(),
            sealer: Sealer::new(&send_key, keys.tag, session),
            opener: Mutex::new(Opener::new(&receive_key, keys.tag)),
            inner: Mutex::new(Inner {
                puncher: Puncher::new(candidates, now),
                reliable: Reliable::new(role == Role::Client),
                peer: None,
                last_sent: now,
                connections: HashMap::new(),
                closed: None,
            }),
            connected: RwLock::new(None),
            last_heard: AtomicU64::new(now),
            last_used: AtomicU64::new(now),
            node,
            capacity: Condvar::new(),
            wake: Condvar::new(),
            woken: Mutex::new(false),
            sleeping_until: AtomicU64::new(u64::MAX),
            epoch: AtomicU64::new(0),
            #[cfg(test)]
            rounds: AtomicU64::new(0),
            stopped: AtomicBool::new(false),
            media,
            listeners: Mutex::new(Vec::new()),
        });
        let tunnel = Self { shared };
        if let Bridge::Device { local, ports } = &bridge {
            // Every port is bound before an accept thread starts, so a port in use leaves no
            // thread behind holding the others.
            let mut listeners = Vec::new();
            for target in [TARGET_HTTP, TARGET_HTTPS, TARGET_RTSP] {
                let Some(port) = tcp_port(ports, target).filter(|port| *port != 0) else {
                    continue;
                };
                listeners.push((TcpListener::bind((*local, port))?, target));
            }
            for (listener, target) in listeners {
                tunnel
                    .shared
                    .listeners
                    .lock()
                    .unwrap()
                    .push(listener.local_addr()?);
                let accepting = tunnel.clone();
                thread::Builder::new()
                    .name("internet play accept".into())
                    .spawn(move || accepting.accept(listener, target))?;
            }
        }
        for channel in 0..CHANNELS as u8 {
            if tunnel.shared.media[channel as usize].is_some() {
                let forwarding = tunnel.clone();
                thread::Builder::new()
                    .name("internet play media".into())
                    .spawn(move || forwarding.forward_media(channel))?;
            }
        }
        let ticking = tunnel.clone();
        thread::Builder::new()
            .name("internet play path".into())
            .spawn(move || ticking.run())?;
        Ok(tunnel)
    }

    pub fn state(&self) -> State {
        let inner = self.shared.inner.lock().unwrap();
        if let Some(reason) = &inner.closed {
            return State::Closed(reason.clone());
        }
        match inner.puncher.state() {
            punch::State::Connected(_) => {
                State::Connected(inner.peer.expect("a connected path has a peer"))
            }
            _ => State::Punching,
        }
    }

    /// Notes when the device's session began, in unix seconds.
    pub fn set_epoch(&self, unix: u64) {
        self.shared.epoch.store(unix, Ordering::Relaxed);
    }

    /// When the device's session began (unix seconds), 0 when unknown.
    pub fn epoch(&self) -> u64 {
        self.shared.epoch.load(Ordering::Relaxed)
    }

    /// The path is connected and its peer was heard from lately ([`HEALTHY_FOR`]): there is
    /// nothing to look for in the DHT on its behalf.
    pub fn healthy(&self) -> bool {
        let silent = self
            .shared
            .node
            .now()
            .saturating_sub(self.shared.last_heard.load(Ordering::Relaxed));
        !self.is_closed() && silent <= HEALTHY_FOR && matches!(self.state(), State::Connected(_))
    }

    /// More addresses for the peer, learned after the start.
    pub fn add_candidates(&self, more: &[SocketAddr]) {
        let mut inner = self.shared.inner.lock().unwrap();
        inner.puncher.add(more);
        self.publish(&inner);
        drop(inner);
        self.wake_by(0);
    }

    /// Blocks until the path is closed.
    pub fn wait_closed(&self) {
        let mut inner = self.shared.inner.lock().unwrap();
        while inner.closed.is_none() {
            inner = self.shared.capacity.wait(inner).unwrap();
        }
    }

    /// Wakes the path thread when `at` (the node's clock) comes before what it sleeps until.
    /// Called after the change that makes `at` due, so a thread about to sleep sees either the
    /// change or the wake.
    fn wake_by(&self, at: Millis) {
        if at < self.shared.sleeping_until.load(Ordering::SeqCst) {
            *self.shared.woken.lock().unwrap() = true;
            self.shared.wake.notify_one();
        }
    }

    /// Punches on at least until `deadline` (the node's clock): the peer finds this side's
    /// record later.
    pub fn hold_punching_until(&self, deadline: Millis) {
        self.shared
            .inner
            .lock()
            .unwrap()
            .puncher
            .hold_until(deadline);
    }

    /// Mirrors `inner`'s connected peer into `connected`; called with `inner` held after any
    /// change to its peer, punch state or closing, so the two never disagree for long.
    fn publish(&self, inner: &Inner) {
        let peer = match inner.puncher.state() {
            punch::State::Connected(_) if inner.closed.is_none() => inner.peer,
            _ => None,
        };
        // Written only when it changes: the media threads read it for every datagram.
        if *self.shared.connected.read().unwrap() != peer {
            *self.shared.connected.write().unwrap() = peer;
        }
    }

    /// Ends the path and tells the peer.
    pub fn close(&self, reason: &str) {
        let peer = {
            let mut inner = self.shared.inner.lock().unwrap();
            if inner.closed.is_none() {
                inner.closed = Some(reason.to_owned());
            }
            self.publish(&inner);
            for (_, connection) in inner.connections.drain() {
                let _ = connection.writer.send(Outgoing::Reset);
            }
            inner.peer
        };
        if let Some(peer) = peer {
            self.shared
                .node
                .send(peer, &self.shared.sealer.seal(Kind::Close, 0, &[]));
        }
        if !self.shared.stopped.swap(true, Ordering::AcqRel) {
            // Wake the accept threads, which block in accept.
            for address in self.shared.listeners.lock().unwrap().iter() {
                let _ = TcpStream::connect_timeout(address, Duration::from_millis(200));
            }
            self.shared.capacity.notify_all();
            self.wake_by(0);
        }
    }

    pub fn is_closed(&self) -> bool {
        self.shared.stopped.load(Ordering::Acquire)
    }

    /// Takes a datagram the node received for this path's tag, and decrypts it in place.
    pub fn receive(&self, datagram: &mut [u8], from: SocketAddr) {
        let now = self.shared.node.now();
        if self.is_closed() {
            return;
        }
        let opened = self.shared.opener.lock().unwrap().open_in_place(datagram);
        let Some((kind, channel, payload)) = opened else {
            return;
        };
        let payload = &datagram[payload];
        self.shared.last_heard.fetch_max(now, Ordering::Relaxed);
        if matches!(kind, Kind::Media | Kind::Stream) {
            self.shared.last_used.fetch_max(now, Ordering::Relaxed);
        }
        // Media from the connected peer's address changes nothing in `inner`: it skips the
        // reliable layer's lock entirely.
        if kind == Kind::Media && *self.shared.connected.read().unwrap() == Some(from) {
            self.media_in(channel, payload);
            return;
        }
        let mut inner = self.shared.inner.lock().unwrap();
        if inner.closed.is_some() {
            return;
        }
        // Any authenticated datagram moves the path to where it came from: a NAT that changed
        // the peer's port keeps working.
        inner.peer = Some(from);
        match kind {
            Kind::Punch => {
                inner.puncher.on_punch(from);
                self.publish(&inner);
                drop(inner);
                self.send_to(from, Kind::PunchAck, 0, &[]);
                return;
            }
            Kind::PunchAck => inner.puncher.on_ack(from),
            Kind::Stream => {
                if !matches!(inner.puncher.state(), punch::State::Connected(_)) {
                    inner.puncher.on_ack(from);
                }
                inner.reliable.handle(payload, now);
            }
            Kind::Media => {
                self.publish(&inner);
                drop(inner);
                self.media_in(channel, payload);
                return;
            }
            Kind::Close => {
                inner.closed = Some("the other side ended the connection".into());
                self.publish(&inner);
                drop(inner);
                self.close("the other side ended the connection");
                return;
            }
            Kind::Keepalive => {}
        }
        self.publish(&inner);
        drop(inner);
        self.dispatch();
        self.flush();
    }

    fn send_to(&self, to: SocketAddr, kind: Kind, channel: u8, payload: &[u8]) {
        self.shared
            .node
            .send(to, &self.shared.sealer.seal(kind, channel, payload));
        self.shared.inner.lock().unwrap().last_sent = self.shared.node.now();
    }

    /// Sends what the reliable layer has ready.
    fn flush(&self) {
        // A retransmission due earlier than the path thread planned, or a dead stream layer,
        // is the path thread's.
        if let Some(due) = self.transmit() {
            self.wake_by(due);
        }
    }

    /// [`Tunnel::flush`] without waking the path thread, for the path thread itself (its next
    /// sleep reckons with the stream layer anyway): when the stream layer next needs it, 0 when
    /// it died.
    fn transmit(&self) -> Option<Millis> {
        let now = self.shared.node.now();
        let mut inner = self.shared.inner.lock().unwrap();
        let peer = inner.peer?;
        if !matches!(inner.puncher.state(), punch::State::Connected(_)) {
            return None;
        }
        let mut sent = false;
        let mut sealed = Vec::new();
        while let Some(segment) = inner.reliable.poll_transmit(now) {
            self.shared
                .sealer
                .seal_into(Kind::Stream, 0, &segment, &mut sealed);
            self.shared.node.send(peer, &sealed);
            sent = true;
        }
        if sent {
            inner.last_sent = now;
        }
        if inner.reliable.is_dead() {
            Some(0)
        } else {
            inner.reliable.next_timeout()
        }
    }

    /// Hands the reliable layer's events to the connections.
    fn dispatch(&self) {
        loop {
            let event = {
                let mut inner = self.shared.inner.lock().unwrap();
                match inner.reliable.poll_event() {
                    Some(event) => event,
                    None => break,
                }
            };
            match event {
                Event::Opened { stream, target } => self.opened(stream, target),
                Event::Data { stream, data } => self.to_connection(stream, Outgoing::Data(data)),
                Event::Finished { stream } => {
                    self.to_connection(stream, Outgoing::Finish);
                    let mut inner = self.shared.inner.lock().unwrap();
                    if let Some(connection) = inner.connections.get_mut(&stream) {
                        connection.finished_in = true;
                        if connection.finished_out {
                            inner.connections.remove(&stream);
                        }
                    }
                }
                Event::Reset { stream } => {
                    self.to_connection(stream, Outgoing::Reset);
                    self.shared
                        .inner
                        .lock()
                        .unwrap()
                        .connections
                        .remove(&stream);
                }
                Event::Dead => {
                    self.close("the other side stopped answering");
                }
            }
        }
        self.shared.capacity.notify_all();
    }

    fn to_connection(&self, stream: StreamId, write: Outgoing) {
        let inner = self.shared.inner.lock().unwrap();
        if let Some(connection) = inner.connections.get(&stream) {
            let _ = connection.writer.send(write);
        }
    }

    /// The host side: a stream opened, so connect to the host's port it names.
    fn opened(&self, stream: StreamId, target: u8) {
        let Bridge::Host {
            service,
            source,
            ports,
            device,
        } = &self.shared.bridge
        else {
            // A host never opens streams towards a device.
            self.shared.inner.lock().unwrap().reliable.reset(stream);
            return;
        };
        let Some(port) = tcp_port(ports, target) else {
            self.shared.inner.lock().unwrap().reliable.reset(stream);
            return;
        };
        let (writer, writes) = mpsc::channel();
        self.shared.inner.lock().unwrap().connections.insert(
            stream,
            Connection {
                writer,
                finished_in: false,
                finished_out: false,
            },
        );
        let (service, source) = (SocketAddr::new(*service, port), *source);
        let device = device.clone();
        let tunnel = self.clone();
        let spawned = thread::Builder::new()
            .name("internet play connect".into())
            .spawn(move || match connect_from(source, service, &device) {
                Ok((tcp, mark)) => {
                    tunnel.bridge_connection(stream, tcp, writes, Some(Arc::new(mark)))
                }
                Err(_) => {
                    let mut inner = tunnel.shared.inner.lock().unwrap();
                    inner.connections.remove(&stream);
                    inner.reliable.reset(stream);
                    drop(inner);
                    tunnel.flush();
                }
            });
        if spawned.is_err() {
            self.shared.inner.lock().unwrap().reliable.reset(stream);
        }
    }
}

mod bridge;
use bridge::connect_from;
pub use bridge::{bindable_loopback, loopback_for};

#[cfg(test)]
mod tests;
