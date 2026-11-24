//! Loopback TCP/UDP bridging, tunnel run loop, and per-device loopback address selection.

use super::*;

impl Tunnel {
    /// The device side: a local client connected to one of the host's ports.
    pub(super) fn accept(&self, listener: TcpListener, target: u8) {
        for incoming in listener.incoming() {
            if self.is_closed() {
                return;
            }
            let Ok(tcp) = incoming else { continue };
            self.shared
                .last_used
                .fetch_max(self.shared.node.now(), Ordering::Relaxed);
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
                .spawn(move || tunnel.bridge_connection(stream, tcp, writes, None));
        }
    }

    /// Runs a TCP connection's two directions: a thread reads it into the stream, this one
    /// writes what the stream brings.
    /// `mark`, on the host side, registers the connection as tunnelled; both directions hold
    /// it, so it stays registered while either still uses the socket. The device side has
    /// nothing to register: its connections come from the device's own programs.
    pub(super) fn bridge_connection(
        &self,
        stream: StreamId,
        tcp: TcpStream,
        writes: mpsc::Receiver<Outgoing>,
        mark: Option<Arc<peers::Mark>>,
    ) {
        let _ = tcp.set_nodelay(true);
        if let Ok(reading) = tcp.try_clone() {
            let tunnel = self.clone();
            let held = mark.clone();
            let _ = thread::Builder::new()
                .name("internet play read".into())
                .spawn(move || {
                    tunnel.read_connection(stream, reading);
                    drop(held);
                });
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
    pub(super) fn forward_media(&self, channel: u8) {
        let Some(end) = &self.shared.media[channel as usize] else {
            return;
        };
        let mut buffer = vec![0u8; 4096];
        // Kept for every datagram, so sealing allocates nothing.
        let mut sealed = Vec::with_capacity(buffer.len() + wire::OVERHEAD);
        while !self.is_closed() {
            let Ok((length, from)) = end.socket.recv_from(&mut buffer) else {
                continue;
            };
            if let Bridge::Device { .. } = self.shared.bridge {
                // Replies go back to wherever the client last sent from.
                if *end.client.read().unwrap() != Some(from) {
                    *end.client.write().unwrap() = Some(from);
                }
                self.shared
                    .last_used
                    .fetch_max(self.shared.node.now(), Ordering::Relaxed);
            }
            let peer = *self.shared.connected.read().unwrap();
            if let Some(peer) = peer {
                self.shared
                    .sealer
                    .seal_into(Kind::Media, channel, &buffer[..length], &mut sealed);
                self.shared.node.send(peer, &sealed);
            }
        }
    }

    /// Media from the peer goes to this side's local end.
    pub(super) fn media_in(&self, channel: u8, payload: &[u8]) {
        let Some(Some(end)) = self.shared.media.get(channel as usize) else {
            return;
        };
        let to = match &self.shared.bridge {
            Bridge::Device { .. } => *end.client.read().unwrap(),
            Bridge::Host { service, ports, .. } => {
                media_port(ports, channel).map(|port| SocketAddr::new(*service, port))
            }
        };
        if let Some(to) = to {
            let _ = end.socket.send_to(payload, to);
        }
    }

    /// Punches, keepalives, retransmissions and the timeouts. Sleeps until the next of them is
    /// due, so an idle path costs a wake every few seconds, not 50 a second.
    pub(super) fn run(&self) {
        while !self.is_closed() {
            #[cfg(test)]
            self.shared.rounds.fetch_add(1, Ordering::Relaxed);
            let now = self.shared.node.now();
            let mut inner = self.shared.inner.lock().unwrap();
            let punches = inner.puncher.poll(now);
            self.publish(&inner);
            let failed = inner.puncher.state() == punch::State::Failed;
            let connected = matches!(inner.puncher.state(), punch::State::Connected(_));
            let idle = now.saturating_sub(self.shared.last_heard.load(Ordering::Relaxed)) > IDLE;
            let keepalive = connected && now.saturating_sub(inner.last_sent) > KEEPALIVE;
            // An open connection is use: the unused time counts from its end (give or take the
            // longest sleep).
            if !inner.connections.is_empty() {
                self.shared.last_used.fetch_max(now, Ordering::Relaxed);
            }
            let unused = self
                .unused_since(&inner, connected)
                .is_some_and(|since| now.saturating_sub(since) > UNUSED);
            let peer = inner.peer;
            drop(inner);
            // A device's punch says when it was sent, so the host can tell a device that started
            // again from an old punch replayed on the way ([`wire::punch_time`]).
            let stamp =
                (self.shared.role == Role::Client).then(|| wire::punch_stamp(node::unix_now()));
            for to in punches {
                self.send_to(
                    to,
                    Kind::Punch,
                    0,
                    stamp.as_ref().map_or(&[][..], |s| &s[..]),
                );
            }
            if failed {
                self.close("no punch got through");
                return;
            }
            if connected && idle {
                self.close("the other side stopped answering");
                return;
            }
            if unused {
                self.close("the device no longer used the connection");
                return;
            }
            if let (true, Some(peer)) = (keepalive, peer) {
                self.send_to(peer, Kind::Keepalive, 0, &[]);
            }
            self.dispatch();
            if self.transmit() == Some(0) {
                // The stream layer died: its event closes the path.
                self.dispatch();
            }
            self.sleep();
        }
    }

    /// Since when a device's connected path has carried nothing, while no connection is open
    /// on it: the device closes it [`UNUSED`] later. None on the host, whose device closes it.
    fn unused_since(&self, inner: &Inner, connected: bool) -> Option<Millis> {
        let device = matches!(self.shared.bridge, Bridge::Device { .. });
        (device && connected && inner.connections.is_empty())
            .then(|| self.shared.last_used.load(Ordering::Relaxed))
    }

    /// Sleeps until the next deadline, or until [`Tunnel::wake_by`] wakes it.
    fn sleep(&self) {
        let now = self.shared.node.now();
        let next = {
            let inner = self.shared.inner.lock().unwrap();
            let connected = matches!(inner.puncher.state(), punch::State::Connected(_));
            let mut next = now + LONGEST_SLEEP;
            if let Some(at) = inner.puncher.next_wakeup() {
                next = next.min(at);
            }
            if let Some(at) = inner.reliable.next_timeout() {
                next = next.min(at);
            }
            if connected {
                next = next.min(inner.last_sent + KEEPALIVE + 1);
                next = next.min(self.shared.last_heard.load(Ordering::Relaxed) + IDLE + 1);
            }
            if let Some(since) = self.unused_since(&inner, connected) {
                next = next.min(since + UNUSED + 1);
            }
            next
        };
        let mut woken = self.shared.woken.lock().unwrap();
        if !*woken && next > now && !self.is_closed() {
            self.shared.sleeping_until.store(next, Ordering::SeqCst);
            woken = self
                .shared
                .wake
                .wait_timeout(woken, Duration::from_millis(next - now))
                .unwrap()
                .0;
        }
        self.shared.sleeping_until.store(u64::MAX, Ordering::SeqCst);
        *woken = false;
    }

    pub fn role(&self) -> Role {
        self.shared.role
    }
}
/// A TCP connection to `to` from `source`, any port, registered as `device`'s tunnelled peer
/// before it connects, so the host's server finds the registration from its first byte. A source
/// that can't be bound exactly (unspecified, or another family) is refused: the host would see a
/// peer address nobody registered, and might trust it.
pub(super) fn connect_from(
    source: IpAddr,
    to: SocketAddr,
    device: &str,
) -> io::Result<(TcpStream, peers::Mark)> {
    use socket2::{Domain, Protocol, SockAddr, Socket, Type};
    if source.is_ipv4() != to.is_ipv4() || source.is_unspecified() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a tunnelled connection needs a source address of its own",
        ));
    }
    let socket = Socket::new(Domain::for_address(to), Type::STREAM, Some(Protocol::TCP))?;
    socket.bind(&SockAddr::from(SocketAddr::new(source, 0)))?;
    let bound = socket
        .local_addr()?
        .as_socket()
        .ok_or_else(|| io::Error::other("no local address"))?;
    let mark = peers::register(bound, device);
    socket.connect_timeout(&SockAddr::from(to), Duration::from_secs(3))?;
    Ok((socket.into(), mark))
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
