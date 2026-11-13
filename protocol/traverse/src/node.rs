//! One UDP socket for everything internet play sends: STUN queries, DHT lookups and the
//! punched paths. Using the one socket matters: the public address STUN reports and the DHT
//! record publishes is the NAT mapping of this socket, the one the peer punches.
//!
//! A thread receives and sorts datagrams by their first bytes (STUN, KRPC, [`crate::wire`]).
//! STUN queries and DHT lookups block the thread that asks; punched paths get their datagrams
//! through a handler.

use crate::{
    dht::{self, Id, Item, Lookup, Message, Newest, Tid},
    stun, Millis,
};
use std::{
    collections::HashMap,
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

/// Takes the datagrams of punched paths: (datagram, sender).
pub type Handler = Box<dyn Fn(&[u8], SocketAddr) + Send + Sync>;

/// How long a lookup may take in all.
const LOOKUP_DEADLINE: Millis = 20_000;
const STUN_WAIT: Duration = Duration::from_secs(3);
/// The responsive DHT nodes kept to start later lookups from.
const KEPT_NODES: usize = 64;

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
        let socket = UdpSocket::bind(address)?;
        socket.set_read_timeout(Some(Duration::from_millis(250)))?;
        let shared = Arc::new(Shared {
            socket,
            clock: Instant::now(),
            stop: AtomicBool::new(false),
            id: crate::crypto::random(),
            bootstrap,
            state: Mutex::new(State::default()),
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
        self.shared.state.lock().unwrap().handler = Some(Arc::new(handler));
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
        let deadline = Instant::now() + STUN_WAIT;
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
        let mut seeds = self.shared.state.lock().unwrap().known.clone();
        for entry in &self.shared.bootstrap {
            seeds.extend(self.resolve(entry));
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
        for address in lookup.responsive() {
            if !state.known.contains(&address) {
                state.known.push(address);
            }
        }
        let excess = state.known.len().saturating_sub(KEPT_NODES);
        state.known.drain(..excess);
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
        let (_, writable) = self.get(&item.key, &item.salt);
        let (sender, answers) = mpsc::channel::<DhtAnswer>();
        let mut registered = Vec::new();
        {
            let mut state = self.shared.state.lock().unwrap();
            for _ in &writable {
                let tid = state.next_tid.to_be_bytes();
                state.next_tid = state.next_tid.wrapping_add(1);
                state.dht.insert(tid, sender.clone());
                registered.push(tid);
            }
        }
        for ((address, token), tid) in writable.iter().zip(&registered) {
            self.send(*address, &dht::put(*tid, &self.shared.id, token, item));
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut confirmed = 0;
        while confirmed < registered.len() && Instant::now() < deadline {
            match answers.recv_timeout(Duration::from_millis(100)) {
                Ok((_, _, Message::Reply { .. })) => confirmed += 1,
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
    let mut buffer = vec![0u8; 2048];
    while !shared.stop.load(Ordering::Acquire) {
        let (length, from) = match shared.socket.recv_from(&mut buffer) {
            Ok(received) => received,
            // Timeouts, and on Windows the ICMP port-unreachable a punch can draw.
            Err(_) => continue,
        };
        let datagram = &buffer[..length];
        if crate::wire::is_wire(datagram) {
            let handler = shared.state.lock().unwrap().handler.clone();
            if let Some(handler) = handler {
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
}
