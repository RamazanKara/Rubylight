//! One UDP socket for everything internet play sends: STUN queries, DHT lookups and the
//! punched paths. Using the one socket matters: the public address STUN reports and the DHT
//! record publishes is the NAT mapping of this socket, the one the peer punches.
//!
//! A thread receives and sorts datagrams by their first bytes (STUN, KRPC, [`crate::wire`]).
//! STUN queries and DHT lookups block the thread that asks; punched paths get their datagrams
//! through a handler.

use crate::{
    Millis,
    dht::{self, Id, Item, Lookup, Message, Newest, Tid},
    stun,
};
use std::{
    collections::HashMap,
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

/// Takes the datagrams of punched paths: (datagram, sender). The datagram is the receive
/// buffer itself, which the handler may decrypt in place.
pub type Handler = Box<dyn Fn(&mut [u8], SocketAddr) + Send + Sync>;

/// How long a lookup may take in all.
pub const LOOKUP_DEADLINE: Millis = 20_000;
/// How long STUN servers get to answer.
pub const STUN_WAIT: Millis = 3_000;
/// How long a put waits for the nodes' confirmations after its lookup.
pub const PUT_WAIT: Millis = 3_000;
/// The responsive DHT nodes kept to start later lookups from.
const KEPT_NODES: usize = 64;
/// The bootstrap names' addresses are kept this long: hostnames of DHT routers change rarely, and
/// a lookup that reached no node resolves them again at once.
const BOOTSTRAP_KEPT: Millis = 30 * 60_000;
/// A resolution that found nothing (no DNS yet, or the name is gone) is tried again after this.
const BOOTSTRAP_RETRY: Millis = 10_000;
/// The receive buffer asked for every internet play socket: a single thread opens and forwards
/// every channel's media, and the OS default (64 KiB on Windows) holds a few milliseconds of
/// video. The OS may grant less; smaller sizes are tried down to [`RECEIVE_BUFFER_MIN`], as
/// Nightjar's session sockets do.
pub const RECEIVE_BUFFER: usize = 8 << 20;
const RECEIVE_BUFFER_MIN: usize = 32767;
const RECEIVE_BUFFER_STEP: usize = 16384;

/// A UDP socket bound at `address`, with as much of [`RECEIVE_BUFFER`] as the OS grants.
pub fn bind_udp(address: SocketAddr) -> io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(
        Domain::for_address(address),
        Type::DGRAM,
        Some(Protocol::UDP),
    )?;
    let mut size = RECEIVE_BUFFER;
    while socket.set_recv_buffer_size(size).is_err() && size > RECEIVE_BUFFER_MIN {
        size = size
            .saturating_sub(RECEIVE_BUFFER_STEP)
            .max(RECEIVE_BUFFER_MIN);
    }
    socket.bind(&address.into())?;
    Ok(socket.into())
}

#[derive(Clone)]
pub struct Node {
    shared: Arc<Shared>,
}

struct Shared {
    socket: UdpSocket,
    clock: Instant,
    stop: AtomicBool,
    id: Id,
    bootstrap: Vec<String>,
    state: Mutex<State>,
    /// Counts [`Node::set_handler`] calls, so the receive thread takes `state`'s lock for the
    /// handler only when it changed.
    handler_version: AtomicU64,
    /// The bootstrap names' addresses and when they were resolved ([`Node::seeds`]).
    resolved: Mutex<Option<(Millis, Vec<SocketAddr>)>>,
    /// How many times the bootstrap names were resolved.
    #[cfg(test)]
    resolutions: AtomicU64,
}

type DhtAnswer = (Tid, SocketAddr, Message);

#[derive(Default)]
struct State {
    stun: HashMap<[u8; 12], mpsc::Sender<([u8; 12], SocketAddr)>>,
    dht: HashMap<Tid, mpsc::Sender<DhtAnswer>>,
    next_tid: u16,
    known: Vec<SocketAddr>,
    handler: Option<Arc<Handler>>,
}

impl Node {
    /// Binds an IPv4 socket at `address` (port 0: any) and starts receiving.
    pub fn bind(address: SocketAddr, bootstrap: Vec<String>) -> io::Result<Self> {
        let socket = bind_udp(address)?;
        socket.set_read_timeout(Some(Duration::from_millis(250)))?;
        let shared = Arc::new(Shared {
            socket,
            clock: Instant::now(),
            stop: AtomicBool::new(false),
            id: crate::crypto::random(),
            bootstrap,
            state: Mutex::new(State::default()),
            handler_version: AtomicU64::new(0),
            resolved: Mutex::new(None),
            #[cfg(test)]
            resolutions: AtomicU64::new(0),
        });
        let receiver = shared.clone();
        thread::Builder::new()
            .name("internet play receive".into())
            .spawn(move || receive(receiver))?;
        Ok(Self { shared })
    }

    /// Milliseconds since the node started.
    pub fn now(&self) -> Millis {
        self.shared.clock.elapsed().as_millis() as Millis
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.shared.socket.local_addr()
    }

    pub fn send(&self, to: SocketAddr, datagram: &[u8]) {
        // A lost datagram is like one lost on the way; the protocols above resend.
        let _ = self.shared.socket.send_to(datagram, to);
    }

    pub fn set_handler(&self, handler: Handler) {
        let mut state = self.shared.state.lock().unwrap();
        state.handler = Some(Arc::new(handler));
        self.shared.handler_version.fetch_add(1, Ordering::Release);
    }

    pub fn is_stopped(&self) -> bool {
        self.shared.stop.load(Ordering::Acquire)
    }

    /// Stops receiving; the socket closes once the last clone is gone.
    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::Release);
    }

    fn resolve(&self, entry: &str) -> Vec<SocketAddr> {
        let v4 = self.local_addr().map_or(true, |local| local.is_ipv4());
        entry
            .to_socket_addrs()
            .map(|addresses| {
                addresses
                    .filter(|address| address.is_ipv4() == v4)
                    .take(2)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The public address each STUN server reports for this socket, in the servers' order;
    /// servers that don't answer are left out.
    pub fn stun(&self, servers: &[String]) -> Vec<SocketAddr> {
        let (sender, answers) = mpsc::channel();
        let mut asked = Vec::new();
        for server in servers {
            let transaction: [u8; 12] = crate::crypto::random();
            let addresses = self.resolve(server);
            if addresses.is_empty() {
                continue;
            }
            self.shared
                .state
                .lock()
                .unwrap()
                .stun
                .insert(transaction, sender.clone());
            asked.push((transaction, addresses));
        }
        drop(sender);
        let deadline = Instant::now() + Duration::from_millis(STUN_WAIT);
        let mut found: HashMap<[u8; 12], SocketAddr> = HashMap::new();
        let mut next_send = Instant::now();
        while Instant::now() < deadline && found.len() < asked.len() && !self.is_stopped() {
            if Instant::now() >= next_send {
                for (transaction, addresses) in &asked {
                    if !found.contains_key(transaction) {
                        for &address in addresses {
                            self.send(address, &stun::binding_request(transaction));
                        }
                    }
                }
                next_send = Instant::now() + Duration::from_millis(500);
            }
            if let Ok((transaction, address)) = answers.recv_timeout(Duration::from_millis(100)) {
                found.insert(transaction, address);
            }
        }
        let mut state = self.shared.state.lock().unwrap();
        for (transaction, _) in &asked {
            state.stun.remove(transaction);
        }
        drop(state);
        asked
            .iter()
            .filter_map(|(transaction, _)| found.get(transaction).copied())
            .collect()
    }

    fn seeds(&self) -> Vec<SocketAddr> {
        self.seeds_at(self.now())
    }

    /// The nodes a lookup starts from: those that answered before, and the bootstrap names'
    /// addresses. The names resolve once and are kept ([`BOOTSTRAP_KEPT`]): resolving them for
    /// every lookup put a DNS query on every poll of every device, which cannot help a lookup
    /// that the known nodes serve.
    fn seeds_at(&self, now: Millis) -> Vec<SocketAddr> {
        let mut seeds = self.shared.state.lock().unwrap().known.clone();
        let kept = {
            let resolved = self.shared.resolved.lock().unwrap();
            resolved.as_ref().and_then(|(at, addresses)| {
                let age = now.saturating_sub(*at);
                let keep = if addresses.is_empty() {
                    BOOTSTRAP_RETRY
                } else {
                    BOOTSTRAP_KEPT
                };
                (age < keep).then(|| addresses.clone())
            })
        };
        let bootstrap = kept.unwrap_or_else(|| {
            // Resolved with no lock held: a slow DNS answer holds no other lookup back.
            #[cfg(test)]
            self.shared.resolutions.fetch_add(1, Ordering::Relaxed);
            let mut addresses = Vec::new();
            for entry in &self.shared.bootstrap {
                for address in self.resolve(entry) {
                    if !addresses.contains(&address) {
                        addresses.push(address);
                    }
                }
            }
            *self.shared.resolved.lock().unwrap() = Some((now, addresses.clone()));
            addresses
        });
        for address in bootstrap {
            if !seeds.contains(&address) {
                seeds.push(address);
            }
        }
        seeds
    }

    /// Runs one lookup towards `target`, sending `make(tid)` to each node, and hands every
    /// reply to `on_reply`.
    fn lookup(
        &self,
        target: Id,
        make: impl Fn(Tid) -> Vec<u8>,
        mut on_reply: impl FnMut(&dht::Reply),
    ) -> Lookup {
        if self.is_stopped() {
            return Lookup::new(target, &[], 0);
        }
        let (sender, answers) = mpsc::channel::<DhtAnswer>();
        let first = {
            let mut state = self.shared.state.lock().unwrap();
            let first = state.next_tid;
            state.next_tid = state.next_tid.wrapping_add(1024);
            first
        };
        let mut lookup = Lookup::new(target, &self.seeds(), first);
        let mut registered: Vec<Tid> = Vec::new();
        let start = self.now();
        loop {
            if self.is_stopped() {
                break;
            }
            let now = self.now();
            let queries = lookup.poll(now);
            if !queries.is_empty() {
                let mut state = self.shared.state.lock().unwrap();
                for (_, tid) in &queries {
                    state.dht.insert(*tid, sender.clone());
                    registered.push(*tid);
                }
            }
            for (to, tid) in queries {
                self.send(to, &make(tid));
            }
            if lookup.done() || now - start > LOOKUP_DEADLINE || self.is_stopped() {
                break;
            }
            match answers.recv_timeout(Duration::from_millis(100)) {
                Ok((tid, from, Message::Reply { reply, .. })) => {
                    if lookup.on_reply(&tid, from, &reply) {
                        on_reply(&reply);
                    }
                }
                Ok((tid, from, Message::Error { .. })) => {
                    lookup.on_error(&tid, from);
                }
                _ => {}
            }
        }
        let mut state = self.shared.state.lock().unwrap();
        for tid in registered {
            state.dht.remove(&tid);
        }
        let responsive = lookup.responsive();
        for &address in &responsive {
            if !state.known.contains(&address) {
                state.known.push(address);
            }
        }
        let excess = state.known.len().saturating_sub(KEPT_NODES);
        state.known.drain(..excess);
        drop(state);
        if responsive.is_empty() {
            // Nothing answered: the kept addresses may be stale, so the next lookup resolves
            // the names again.
            *self.shared.resolved.lock().unwrap() = None;
        }
        lookup
    }

    /// The newest valid item of `key` and `salt` in the DHT, and the nodes a put should go to.
    pub fn get(&self, key: &[u8; 32], salt: &[u8]) -> (Option<Item>, Vec<(SocketAddr, Vec<u8>)>) {
        let target = Item::target(key, salt);
        let id = self.shared.id;
        let mut newest = Newest::default();
        let mut lookup = self.lookup(
            target,
            |tid| dht::get(tid, &id, &target, None),
            |reply| newest.offer(reply, salt, key),
        );
        (newest.item, lookup.writable())
    }

    /// Stores `item` on the nodes closest to it; returns how many confirmed.
    pub fn put(&self, item: &Item) -> usize {
        if self.is_stopped() {
            return 0;
        }
        let (_, writable) = self.get(&item.key, &item.salt);
        if self.is_stopped() || writable.is_empty() {
            return 0;
        }
        let (sender, answers) = mpsc::channel::<DhtAnswer>();
        let mut expected: std::collections::HashMap<Tid, SocketAddr> =
            std::collections::HashMap::new();
        let mut registered = Vec::new();
        {
            let mut state = self.shared.state.lock().unwrap();
            for (address, _) in &writable {
                let tid = state.next_tid.to_be_bytes();
                state.next_tid = state.next_tid.wrapping_add(1);
                state.dht.insert(tid, sender.clone());
                expected.insert(tid, *address);
                registered.push(tid);
            }
        }
        for ((address, token), tid) in writable.iter().zip(&registered) {
            self.send(*address, &dht::put(*tid, &self.shared.id, token, item));
        }
        let deadline = Instant::now() + Duration::from_millis(PUT_WAIT);
        let mut confirmed = 0;
        while confirmed < registered.len() && Instant::now() < deadline && !self.is_stopped() {
            match answers.recv_timeout(Duration::from_millis(100)) {
                Ok((tid, from, Message::Reply { .. })) if expected.get(&tid) == Some(&from) => {
                    expected.remove(&tid);
                    confirmed += 1;
                }
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        let mut state = self.shared.state.lock().unwrap();
        for tid in registered {
            state.dht.remove(&tid);
        }
        confirmed
    }
}

fn receive(shared: Arc<Shared>) {
    let mut buffer = vec![0u8; 4096];
    let mut handler: Option<Arc<Handler>> = None;
    let mut version = 0;
    // Checked after every receive, timeouts included: a replaced handler (and the path it
    // holds) goes within a read timeout, and one set during a wait takes what ended it.
    let mut refresh = |handler: &mut Option<Arc<Handler>>| {
        let current = shared.handler_version.load(Ordering::Acquire);
        if current != version {
            *handler = shared.state.lock().unwrap().handler.clone();
            version = current;
        }
    };
    while !shared.stop.load(Ordering::Acquire) {
        let received = shared.socket.recv_from(&mut buffer);
        refresh(&mut handler);
        let (length, from) = match received {
            Ok(received) => received,
            // Timeouts, and on Windows the ICMP port-unreachable a punch can draw.
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::ConnectionRefused
                ) =>
            {
                continue;
            }
            Err(_) => {
                thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        let datagram = &mut buffer[..length];
        if crate::wire::is_wire(datagram) {
            if let Some(handler) = &handler {
                handler(datagram, from);
            }
        } else if stun::is_stun(datagram) {
            let Some(transaction) = stun::transaction(datagram) else {
                continue;
            };
            let Some(address) = stun::mapped_address(datagram, &transaction) else {
                continue;
            };
            if let Some(sender) = shared.state.lock().unwrap().stun.remove(&transaction) {
                let _ = sender.send((transaction, address));
            }
        } else if dht::is_krpc(datagram) {
            let message = match dht::parse(datagram) {
                Some(message @ (Message::Reply { .. } | Message::Error { .. })) => message,
                _ => continue,
            };
            let tid = match &message {
                Message::Reply { tid, .. } | Message::Error { tid, .. } => tid.clone(),
                Message::Query => continue,
            };
            let Ok(tid) = <Tid>::try_from(tid.as_slice()) else {
                continue;
            };
            let sender = shared.state.lock().unwrap().dht.get(&tid).cloned();
            if let Some(sender) = sender {
                let _ = sender.send((tid, from, message));
            }
        }
    }
}

/// The unspecified IPv4 address and any port: where a node binds by default.
pub fn any_ipv4() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)
}

/// Unix time in seconds.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sockets_get_a_larger_receive_buffer_than_the_default() {
        let plain = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sized = bind_udp("127.0.0.1:0".parse().unwrap()).unwrap();
        let size = |socket: &UdpSocket| socket2::SockRef::from(socket).recv_buffer_size().unwrap();
        assert!(
            size(&sized) > size(&plain),
            "{} < {}",
            size(&sized),
            size(&plain)
        );
        assert!(size(&sized) >= RECEIVE_BUFFER_MIN);
    }

    /// The bootstrap names resolve once, not for every lookup: again after a long time, or
    /// once a lookup reached no node; a name that resolved to nothing is retried sooner.
    #[test]
    fn bootstrap_names_resolve_once_and_again_after_a_failure_or_a_long_time() {
        let resolutions = |node: &Node| node.shared.resolutions.load(Ordering::Relaxed);
        let dead: SocketAddr = "127.0.0.1:9".parse().unwrap();
        let node = Node::bind("127.0.0.1:0".parse().unwrap(), vec![dead.to_string()]).unwrap();
        assert_eq!(node.seeds_at(0), vec![dead]);
        assert_eq!(node.seeds_at(1_000), vec![dead]);
        assert_eq!(node.seeds_at(BOOTSTRAP_KEPT - 1), vec![dead]);
        assert_eq!(resolutions(&node), 1);
        assert_eq!(node.seeds_at(BOOTSTRAP_KEPT), vec![dead]);
        assert_eq!(resolutions(&node), 2);
        // A lookup that reached nobody (nothing listens there) makes the next one resolve again,
        // while one that is served by the kept addresses does not resolve at all.
        let before = resolutions(&node);
        let (item, _) = node.get(&[1; 32], b"salt");
        assert!(item.is_none());
        assert_eq!(resolutions(&node), before);
        node.seeds();
        assert_eq!(resolutions(&node), before + 1);
        node.stop();

        // A name that gave nothing is asked again after a short wait, not for every lookup.
        let node = Node::bind(
            "127.0.0.1:0".parse().unwrap(),
            vec!["no port in this name".into()],
        )
        .unwrap();
        assert!(node.seeds_at(0).is_empty());
        assert!(node.seeds_at(BOOTSTRAP_RETRY - 1).is_empty());
        assert_eq!(resolutions(&node), 1);
        assert!(node.seeds_at(BOOTSTRAP_RETRY).is_empty());
        assert_eq!(resolutions(&node), 2);
        node.stop();
    }

    /// A STUN server on loopback answers the node's query with the address it saw.
    #[test]
    fn stun_reports_the_address_a_server_sees() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let server_address = server.local_addr().unwrap();
        thread::spawn(move || {
            let mut buffer = [0u8; 512];
            for _ in 0..4 {
                let Ok((length, from)) = server.recv_from(&mut buffer) else {
                    return;
                };
                let transaction: [u8; 12] = buffer[8..20].try_into().unwrap();
                let _ = length;
                let mut reply = vec![0x01, 0x01, 0, 12];
                reply.extend_from_slice(&stun::COOKIE.to_be_bytes());
                reply.extend_from_slice(&transaction);
                reply.extend_from_slice(&[0, 0x20, 0, 8, 0, 1]);
                let SocketAddr::V4(from4) = from else { return };
                reply.extend_from_slice(&(from4.port() ^ 0x2112).to_be_bytes());
                let cookie = stun::COOKIE.to_be_bytes();
                reply.extend(from4.ip().octets().iter().zip(cookie).map(|(a, b)| a ^ b));
                server.send_to(&reply, from).unwrap();
            }
        });
        let node = Node::bind("127.0.0.1:0".parse().unwrap(), Vec::new()).unwrap();
        let answers = node.stun(&[server_address.to_string()]);
        assert_eq!(answers, vec![node.local_addr().unwrap()]);
        node.stop();
    }

    #[test]
    fn put_exits_promptly_when_node_is_stopped() {
        let item = Item::sign(&[3; 32], b"salt".to_vec(), 1, b"value".to_vec());
        // Pre-stopped node returns immediately without touching the network.
        let stopped = Node::bind(
            "127.0.0.1:0".parse().unwrap(),
            vec!["127.0.0.1:9".to_owned()],
        )
        .unwrap();
        stopped.stop();
        let started = Instant::now();
        assert_eq!(stopped.put(&item), 0);
        assert!(started.elapsed() < Duration::from_millis(500));

        // Stopping during PUT_WAIT exits promptly instead of waiting the full PUT_WAIT.
        let dht = UdpSocket::bind("127.0.0.1:0").unwrap();
        dht.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let dht_addr = dht.local_addr().unwrap();
        let node = Node::bind("127.0.0.1:0".parse().unwrap(), vec![dht_addr.to_string()]).unwrap();
        let stopper = node.clone();
        let server = thread::spawn(move || {
            let mut buf = [0u8; 2048];
            let mut got_put = false;
            for _ in 0..8 {
                let Ok((len, from)) = dht.recv_from(&mut buf) else {
                    break;
                };
                let Some(msg) = crate::bencode::decode(&buf[..len]) else {
                    continue;
                };
                let Some(tid) = msg.get(b"t").and_then(crate::bencode::Value::as_bytes) else {
                    continue;
                };
                match msg.get(b"q").and_then(crate::bencode::Value::as_bytes) {
                    Some(b"get") => {
                        let reply = crate::bencode::Value::dict([
                            ("t", crate::bencode::Value::bytes(tid)),
                            ("y", crate::bencode::Value::bytes(*b"r")),
                            (
                                "r",
                                crate::bencode::Value::dict([
                                    ("id", crate::bencode::Value::bytes([7u8; 20])),
                                    ("token", crate::bencode::Value::bytes(*b"tok")),
                                ]),
                            ),
                        ])
                        .encode();
                        let _ = dht.send_to(&reply, from);
                    }
                    Some(b"put") => {
                        got_put = true;
                        thread::sleep(Duration::from_millis(50));
                        stopper.stop();
                        break;
                    }
                    _ => {}
                }
            }
            got_put
        });
        let started = Instant::now();
        let confirmed = node.put(&item);
        let elapsed = started.elapsed();
        assert_eq!(confirmed, 0);
        assert!(server.join().unwrap(), "mock DHT node received put");
        assert!(
            elapsed < Duration::from_millis(1_500),
            "put waited {elapsed:?} after stop"
        );
    }
}
