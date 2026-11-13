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
    node::Node,
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
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Condvar, Mutex,
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
/// A path that hears nothing for this long is gone.
const IDLE: Millis = 45_000;
const TICK: Duration = Duration::from_millis(20);

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
    /// address on this PC.
    Host {
        service: IpAddr,
        source: IpAddr,
        ports: Ports,
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
    opener: Opener,
    puncher: Puncher,
    reliable: Reliable,
    peer: Option<SocketAddr>,
    last_heard: Millis,
    last_sent: Millis,
    connections: HashMap<StreamId, Connection>,
    closed: Option<String>,
}

struct Shared {
    node: Node,
    role: Role,
    bridge: Bridge,
    sealer: Sealer,
    inner: Mutex<Inner>,
    capacity: Condvar,
    stopped: AtomicBool,
    /// The device's media sockets, and where its client last sent from on each.
    media: Vec<Option<MediaEnd>>,
    listeners: Mutex<Vec<SocketAddr>>,
}

struct MediaEnd {
    socket: UdpSocket,
    client: Mutex<Option<SocketAddr>>,
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
                            socket: UdpSocket::bind((*local, port))?,
                            client: Mutex::new(None),
                        }),
                        None => None,
                    }
                }
                Bridge::Host { source, .. } => Some(MediaEnd {
                    socket: UdpSocket::bind((*source, 0))?,
                    client: Mutex::new(None),
                }),
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
            inner: Mutex::new(Inner {
                opener: Opener::new(&receive_key, keys.tag),
                puncher: Puncher::new(candidates, now),
                reliable: Reliable::new(role == Role::Client),
                peer: None,
                last_heard: now,
                last_sent: now,
                connections: HashMap::new(),
                closed: None,
            }),
            node,
            capacity: Condvar::new(),
            stopped: AtomicBool::new(false),
            media,
            listeners: Mutex::new(Vec::new()),
        });
        let tunnel = Self { shared };
        if let Bridge::Device { local, ports } = &bridge {
            for target in [TARGET_HTTP, TARGET_HTTPS, TARGET_RTSP] {
                let Some(port) = tcp_port(ports, target).filter(|port| *port != 0) else {
                    continue;
                };
                let listener = TcpListener::bind((*local, port))?;
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

    /// More addresses for the peer, learned after the start.
    pub fn add_candidates(&self, more: &[SocketAddr]) {
        self.shared.inner.lock().unwrap().puncher.add(more);
    }

    /// Ends the path and tells the peer.
    pub fn close(&self, reason: &str) {
        let peer = {
            let mut inner = self.shared.inner.lock().unwrap();
            if inner.closed.is_none() {
                inner.closed = Some(reason.to_owned());
            }
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
        }
    }

    pub fn is_closed(&self) -> bool {
        self.shared.stopped.load(Ordering::Acquire)
    }

    /// Takes a datagram the node received for this path's tag.
    pub fn receive(&self, datagram: &[u8], from: SocketAddr) {
        let now = self.shared.node.now();
        let mut inner = self.shared.inner.lock().unwrap();
        if inner.closed.is_some() {
            return;
        }
        let Some(opened) = inner.opener.open(datagram) else {
            return;
        };
        inner.last_heard = now;
        // Any authenticated datagram moves the path to where it came from: a NAT that changed
        // the peer's port keeps working.
        inner.peer = Some(from);
        match opened.kind {
            Kind::Punch => {
                inner.puncher.on_punch(from);
                drop(inner);
                self.send_to(from, Kind::PunchAck, 0, &[]);
                return;
            }
            Kind::PunchAck => inner.puncher.on_ack(from),
            Kind::Stream => {
                if !matches!(inner.puncher.state(), punch::State::Connected(_)) {
                    inner.puncher.on_ack(from);
                }
                inner.reliable.handle(&opened.payload, now);
            }
            Kind::Media => {
                drop(inner);
                self.media_in(opened.channel, &opened.payload);
                return;
            }
            Kind::Close => {
                inner.closed = Some("the other side ended the connection".into());
                drop(inner);
                self.close("the other side ended the connection");
                return;
            }
            Kind::Keepalive => {}
        }
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
        let now = self.shared.node.now();
        let mut inner = self.shared.inner.lock().unwrap();
        let Some(peer) = inner.peer else { return };
        if !matches!(inner.puncher.state(), punch::State::Connected(_)) {
            return;
        }
        let mut sent = false;
        while let Some(segment) = inner.reliable.poll_transmit(now) {
            self.shared
                .node
                .send(peer, &self.shared.sealer.seal(Kind::Stream, 0, &segment));
            sent = true;
        }
        if sent {
            inner.last_sent = now;
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
        let tunnel = self.clone();
        let spawned = thread::Builder::new()
            .name("internet play connect".into())
            .spawn(move || match connect_from(source, service) {
                Ok(tcp) => tunnel.bridge_connection(stream, tcp, writes),
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

    /// The device side: a local client connected to one of the host's ports.
    fn accept(&self, listener: TcpListener, target: u8) {
        for incoming in listener.incoming() {
            if self.is_closed() {
                return;
            }
            let Ok(tcp) = incoming else { continue };
            if !matches!(self.state(), State::Connected(_)) {
                // Not through yet: the client sees a refused connection, as from a host that
                // is not there, and tries again.
                let _ = tcp.shutdown(Shutdown::Both);
                continue;
            }
            let (writer, writes) = mpsc::channel();
            let stream = {
                let mut inner = self.shared.inner.lock().unwrap();
                let stream = inner.reliable.open(target);
                inner.connections.insert(
                    stream,
                    Connection {
                        writer,
                        finished_in: false,
                        finished_out: false,
                    },
                );
                stream
            };
            self.flush();
            let tunnel = self.clone();
            let _ = thread::Builder::new()
                .name("internet play connection".into())
                .spawn(move || tunnel.bridge_connection(stream, tcp, writes));
        }
    }

    /// Runs a TCP connection's two directions: a thread reads it into the stream, this one
    /// writes what the stream brings.
    fn bridge_connection(
        &self,
        stream: StreamId,
        tcp: TcpStream,
        writes: mpsc::Receiver<Outgoing>,
    ) {
        let _ = tcp.set_nodelay(true);
        if let Ok(reading) = tcp.try_clone() {
            let tunnel = self.clone();
            let _ = thread::Builder::new()
                .name("internet play read".into())
                .spawn(move || tunnel.read_connection(stream, reading));
        } else {
            self.shared.inner.lock().unwrap().reliable.reset(stream);
            self.flush();
            return;
        }
        let mut tcp = tcp;
        for write in writes {
            match write {
                Outgoing::Data(data) => {
                    if tcp.write_all(&data).is_err() {
                        let _ = tcp.shutdown(Shutdown::Both);
                        let mut inner = self.shared.inner.lock().unwrap();
                        inner.reliable.reset(stream);
                        inner.connections.remove(&stream);
                        drop(inner);
                        self.flush();
                        return;
                    }
                }
                Outgoing::Finish => {
                    let _ = tcp.shutdown(Shutdown::Write);
                }
                Outgoing::Reset => {
                    let _ = tcp.shutdown(Shutdown::Both);
                    return;
                }
            }
        }
    }

    fn read_connection(&self, stream: StreamId, mut tcp: TcpStream) {
        let mut buffer = vec![0u8; 16 * 1024];
        loop {
            let length = match tcp.read(&mut buffer) {
                Ok(0) => {
                    let mut inner = self.shared.inner.lock().unwrap();
                    inner.reliable.finish(stream);
                    if let Some(connection) = inner.connections.get_mut(&stream) {
                        connection.finished_out = true;
                        if connection.finished_in {
                            inner.connections.remove(&stream);
                        }
                    }
                    drop(inner);
                    self.flush();
                    return;
                }
                Ok(length) => length,
                Err(_) => {
                    let mut inner = self.shared.inner.lock().unwrap();
                    if inner.connections.remove(&stream).is_some() {
                        inner.reliable.reset(stream);
                    }
                    drop(inner);
                    self.flush();
                    return;
                }
            };
            let mut at = 0;
            while at < length {
                let mut inner = self.shared.inner.lock().unwrap();
                if inner.closed.is_some() || inner.reliable.is_dead() {
                    let _ = tcp.shutdown(Shutdown::Both);
                    return;
                }
                let taken = inner.reliable.write(stream, &buffer[at..length]);
                at += taken;
                if taken == 0 {
                    // The send buffer is full: wait for acknowledgements to free it.
                    let _ = self
                        .shared
                        .capacity
                        .wait_timeout(inner, Duration::from_millis(100));
                } else {
                    drop(inner);
                }
                self.flush();
            }
        }
    }

    /// Media from this side's local sockets goes out to the peer.
    fn forward_media(&self, channel: u8) {
        let Some(end) = &self.shared.media[channel as usize] else {
            return;
        };
        let mut buffer = vec![0u8; 2048];
        while !self.is_closed() {
            let Ok((length, from)) = end.socket.recv_from(&mut buffer) else {
                continue;
            };
            if let Bridge::Device { .. } = self.shared.bridge {
                // Replies go back to wherever the client last sent from.
                *end.client.lock().unwrap() = Some(from);
            }
            let peer = {
                let inner = self.shared.inner.lock().unwrap();
                match inner.puncher.state() {
                    punch::State::Connected(_) => inner.peer,
                    _ => None,
                }
            };
            if let Some(peer) = peer {
                self.shared.node.send(
                    peer,
                    &self
                        .shared
                        .sealer
                        .seal(Kind::Media, channel, &buffer[..length]),
                );
            }
        }
    }

    /// Media from the peer goes to this side's local end.
    fn media_in(&self, channel: u8, payload: &[u8]) {
        let Some(Some(end)) = self.shared.media.get(channel as usize) else {
            return;
        };
        let to = match &self.shared.bridge {
            Bridge::Device { .. } => *end.client.lock().unwrap(),
            Bridge::Host { service, ports, .. } => {
                media_port(ports, channel).map(|port| SocketAddr::new(*service, port))
            }
        };
        if let Some(to) = to {
            let _ = end.socket.send_to(payload, to);
        }
    }

    /// Punches, keepalives, retransmissions and the idle timeout.
    fn run(&self) {
        while !self.is_closed() {
            thread::sleep(TICK);
            let now = self.shared.node.now();
            let mut inner = self.shared.inner.lock().unwrap();
            let punches = inner.puncher.poll(now);
            let failed = inner.puncher.state() == punch::State::Failed;
            let connected = matches!(inner.puncher.state(), punch::State::Connected(_));
            let idle = now.saturating_sub(inner.last_heard) > IDLE;
            let keepalive = connected && now.saturating_sub(inner.last_sent) > KEEPALIVE;
            let peer = inner.peer;
            drop(inner);
            for to in punches {
                self.send_to(to, Kind::Punch, 0, &[]);
            }
            if failed {
                self.close("no punch got through");
                return;
            }
            if connected && idle {
                self.close("the other side stopped answering");
                return;
            }
            if let (true, Some(peer)) = (keepalive, peer) {
                self.send_to(peer, Kind::Keepalive, 0, &[]);
            }
            self.dispatch();
            self.flush();
        }
    }

    pub fn role(&self) -> Role {
        self.shared.role
    }
}

/// A TCP connection to `to` from `source`, any port.
fn connect_from(source: IpAddr, to: SocketAddr) -> io::Result<TcpStream> {
    use socket2::{Domain, Protocol, SockAddr, Socket, Type};
    let socket = Socket::new(Domain::for_address(to), Type::STREAM, Some(Protocol::TCP))?;
    if source.is_ipv4() == to.is_ipv4() && !source.is_unspecified() {
        socket.bind(&SockAddr::from(SocketAddr::new(source, 0)))?;
    }
    socket.connect_timeout(&SockAddr::from(to), Duration::from_secs(3))?;
    Ok(socket.into())
}

/// A loopback address of its own for each name: 127.`base`.x.y from a hash of `name`. Every
/// address in 127.0.0.0/8 is loopback on Windows, Linux and Android; macOS answers only
/// 127.0.0.1, which [`bindable_loopback`] falls back to.
pub fn loopback_for(base: u8, name: &str) -> IpAddr {
    let hash = crate::crypto::sha1(&[name.as_bytes()]);
    // x.y never 0.0 or 255.255, which some stacks treat apart.
    let (x, y) = (hash[0], hash[1].clamp(1, 254));
    IpAddr::V4(std::net::Ipv4Addr::new(127, base, x, y))
}

/// `wanted` if a socket can bind it here, else 127.0.0.1.
pub fn bindable_loopback(wanted: IpAddr) -> IpAddr {
    if UdpSocket::bind((wanted, 0)).is_ok() {
        wanted
    } else {
        IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::Keys;

    /// A device and a host tunnel on loopback, punching each other through real sockets: an HTTP
    /// exchange and a media datagram go through both ways.
    #[test]
    fn a_device_reaches_the_hosts_tcp_and_media_ports_through_the_tunnel() {
        // The "host's" services: an echo server on TCP and a UDP echo for video.
        let service = TcpListener::bind("127.0.0.1:0").unwrap();
        let video = UdpSocket::bind("127.0.0.1:0").unwrap();
        let ports = Ports {
            http: service.local_addr().unwrap().port(),
            video: video.local_addr().unwrap().port(),
            ..Ports::default()
        };
        thread::spawn(move || {
            for connection in service.incoming() {
                let mut connection = connection.unwrap();
                let mut request = [0u8; 4];
                connection.read_exact(&mut request).unwrap();
                connection.write_all(b"pong").unwrap();
            }
        });
        thread::spawn(move || {
            let mut buffer = [0u8; 64];
            let (length, from) = video.recv_from(&mut buffer).unwrap();
            video.send_to(&buffer[..length], from).unwrap();
        });

        let keys = Keys::new([5; 32]).path(&[1; 16], &[2; 16]);
        let host_node = Node::bind("127.0.0.1:0".parse().unwrap(), Vec::new()).unwrap();
        let device_node = Node::bind("127.0.0.1:0".parse().unwrap(), Vec::new()).unwrap();
        let host = Tunnel::start(
            host_node.clone(),
            Role::Host,
            keys,
            [2; 16],
            vec![device_node.local_addr().unwrap()],
            Bridge::Host {
                service: "127.0.0.1".parse().unwrap(),
                source: "127.0.0.1".parse().unwrap(),
                ports,
            },
        )
        .unwrap();
        // The device binds the host's ports on its own loopback address; the test's services
        // hold them on 127.0.0.1, so it takes another.
        let local = bindable_loopback("127.0.0.2".parse().unwrap());
        if local == IpAddr::from([127, 0, 0, 1]) {
            return; // A system without 127/8 (macOS): nothing to test here.
        }
        let device = Tunnel::start(
            device_node.clone(),
            Role::Client,
            keys,
            [2; 16],
            vec![host_node.local_addr().unwrap()],
            Bridge::Device { local, ports },
        )
        .unwrap();
        let (h, d) = (host.clone(), device.clone());
        host_node.set_handler(Box::new(move |datagram, from| h.receive(datagram, from)));
        device_node.set_handler(Box::new(move |datagram, from| d.receive(datagram, from)));
        for _ in 0..100 {
            if matches!(device.state(), State::Connected(_))
                && matches!(host.state(), State::Connected(_))
            {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            matches!(device.state(), State::Connected(_)),
            "{:?}",
            device.state()
        );

        let mut tcp = TcpStream::connect((local, ports.http)).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        tcp.write_all(b"ping").unwrap();
        let mut reply = [0u8; 4];
        tcp.read_exact(&mut reply).unwrap();
        assert_eq!(&reply, b"pong");

        let client = UdpSocket::bind((local, 0)).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        client.send_to(b"frame", (local, ports.video)).unwrap();
        let mut buffer = [0u8; 64];
        let (length, _) = client.recv_from(&mut buffer).unwrap();
        assert_eq!(&buffer[..length], b"frame");

        device.close("test over");
        for _ in 0..50 {
            if host.is_closed() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(host.is_closed());
        host_node.stop();
        device_node.stop();
    }

    #[test]
    fn loopback_addresses_are_stable_and_spread() {
        assert_eq!(loopback_for(78, "host-a"), loopback_for(78, "host-a"));
        assert_ne!(loopback_for(78, "host-a"), loopback_for(78, "host-b"));
        let IpAddr::V4(ip) = loopback_for(77, "x") else {
            panic!()
        };
        assert_eq!(ip.octets()[..2], [127, 77]);
        assert!(ip.is_loopback());
    }
}
