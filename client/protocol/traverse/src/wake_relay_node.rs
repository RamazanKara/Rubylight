//! The away wake relay over sockets; [`crate::wake_relay`] decides, this drives it.
//!
//! [`WakeRelayPeer`] runs on an always-on device on the PC's network that the PC enrolled: per
//! PC and per device the PC lists, it publishes a [`Role::WakeRelay`] record (its public
//! endpoints and session) under that device's wake secret, looks for the device's
//! [`Role::WakeRequester`] record, punches back, and answers a sealed request with one burst of
//! magic packets on its own network ([`Lan`]) and the statuses that follow. A datagram that does
//! not open with a listed device's keys gets no answer at all. [`request_wake`]
//! ([`requester`]) is the device's side.
//!
//! It shares nothing with the internet play host's service ([`crate::host`]): no tunnel, no
//! port of the PC's, one socket of its own.

use crate::{
    Millis,
    node::{Node, unix_now},
    punch::{self, Puncher},
    record::{self, Keys, Nat, PathKeys, Record, Role},
    stun,
    wake_relay::{Burst, Decision, DeviceId, Enrollment, Interface, Relay, Status, WakeStatus},
    wire::{self, Kind, Opened, Opener, Sealer},
};
use std::{
    collections::HashMap,
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream, UdpSocket},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

pub mod requester;
pub use requester::{Failure, Progress, RelayInfo, RequestConfig, request_wake};

/// STUN again this often, as the internet play host does.
const STUN_EVERY: Millis = 5 * 60_000;
const STUN_RETRY: Millis = 30_000;
/// Republish a relay record this often, well inside the two hours DHT nodes keep it.
pub const PUBLISH_EVERY: Millis = 30 * 60_000;
const PUBLISH_RETRY: Millis = 60_000;
/// Look for each device's request this often: within a requester's punch budget.
const POLL_EVERY: Millis = punch::HOST_POLL_IDLE;
const MAX_POLLS: usize = 4;
/// A path nobody speaks on for this long is dropped.
const PATH_IDLE: Millis = (crate::wake_relay::BOOT_WAIT_SECONDS + 30) * 1000;
/// Ended or superseded `(tag, session)` pairs stay rejected for twice a punch/record's freshness
/// window so a delayed or replayed packet or DHT record can neither reopen them nor clobber a
/// newer session.
const ENDED_SESSION_HOLD: Millis = 2 * record::CLIENT_FRESH_SECONDS * 1000;
/// How often the relay looks whether the PC came up after a burst.
#[cfg(not(test))]
const PROBE_EVERY: Duration = Duration::from_secs(2);
#[cfg(test)]
const PROBE_EVERY: Duration = Duration::from_millis(50);
/// How often the relay sends `Status::WaitingForBoot` while waiting for the PC to boot, keeping
/// NAT pinholes alive across [`crate::wake_relay::BOOT_WAIT_SECONDS`].
#[cfg(not(test))]
const WAIT_KEEPALIVE: Duration = Duration::from_secs(10);
#[cfg(test)]
const WAIT_KEEPALIVE: Duration = Duration::from_millis(120);
const PROBE_CONNECT: Duration = Duration::from_millis(800);
/// The sequence numbers each path may send: far more than a path lives to use.
const PATH_SEQUENCES: u64 = 1 << 32;
/// How often, and how far apart, the final status goes.
const FINAL_COPIES: usize = 2;
const FINAL_GAP: Duration = Duration::from_millis(300);

/// What the relay needs of the network it is on.
pub trait Lan: Send + Sync {
    /// This device's IPv4 interfaces that are up.
    fn interfaces(&self) -> Vec<Interface>;
    /// Sends the burst once; how many datagrams went.
    fn send(&self, burst: &Burst) -> usize;
    /// Whether the PC answers at `address` (its HTTP port takes a connection).
    fn is_up(&self, address: SocketAddrV4) -> bool;
}

/// A [`Lan`] on real sockets. Listing interfaces takes a crate each caller already has.
pub struct UdpLan {
    list: Box<dyn Fn() -> Vec<Interface> + Send + Sync>,
}

impl UdpLan {
    pub fn new(list: impl Fn() -> Vec<Interface> + Send + Sync + 'static) -> Self {
        Self {
            list: Box::new(list),
        }
    }
}

impl Lan for UdpLan {
    fn interfaces(&self) -> Vec<Interface> {
        (self.list)()
    }

    /// Each datagram leaves once, from the address [`sources`] picks (never a wildcard).
    fn send(&self, burst: &Burst) -> usize {
        let sources = sources(burst, &self.interfaces());
        let mut sockets: HashMap<Ipv4Addr, UdpSocket> = HashMap::new();
        for ip in sources.iter().flatten() {
            if !sockets.contains_key(ip)
                && let Ok(socket) = UdpSocket::bind(SocketAddrV4::new(*ip, 0))
                && socket.set_broadcast(true).is_ok()
            {
                sockets.insert(*ip, socket);
            }
        }
        let per_repeat = burst
            .packets
            .len()
            .div_ceil(crate::wake_relay::BURST_REPEATS)
            .max(1);
        let mut sent = 0;
        for (index, ((destination, packet), source)) in
            burst.packets.iter().zip(&sources).enumerate()
        {
            if index > 0 && index % per_repeat == 0 {
                thread::sleep(Duration::from_millis(10));
            }
            if let Some(socket) = source.and_then(|ip| sockets.get(&ip))
                && socket.send_to(packet, destination).is_ok()
            {
                sent += 1;
            }
        }
        sent
    }

    fn is_up(&self, address: SocketAddrV4) -> bool {
        TcpStream::connect_timeout(&address.into(), PROBE_CONNECT).is_ok()
    }
}

/// The address each of a burst's datagrams leaves from, or none: a directed broadcast from the
/// first interface on its subnet, the limited broadcast from the first of those only. So a
/// relay on several networks still sends each datagram at most once, never more than the burst.
fn sources(burst: &Burst, interfaces: &[Interface]) -> Vec<Option<Ipv4Addr>> {
    let on_subnet = |destination: Ipv4Addr| {
        interfaces
            .iter()
            .find(|interface| interface.broadcast() == Some(destination))
            .map(|interface| interface.ip)
    };
    let first = burst
        .packets
        .iter()
        .find_map(|(destination, _)| on_subnet(*destination.ip()));
    burst
        .packets
        .iter()
        .map(|(destination, _)| {
            if destination.ip().is_broadcast() {
                first
            } else {
                on_subnet(*destination.ip())
            }
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct RelayConfig {
    /// Where the relay's socket binds (IPv4, port 0 for any).
    pub bind: SocketAddr,
    pub stun_servers: Vec<String>,
    pub bootstrap: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RelayStatus {
    /// The socket's public addresses, as STUN servers see them.
    pub public: Vec<SocketAddr>,
    pub nat: Nat,
    pub stun_error: Option<String>,
    pub dht_error: Option<String>,
    /// PCs that enrolled this relay, and the devices they let wake them.
    pub targets: usize,
    pub devices: usize,
    /// Devices whose relay record the DHT took.
    pub published: usize,
    /// The last request's PC, unix time and outcome.
    pub last_wake: Option<(String, u64, Status)>,
}

struct DeviceState {
    target: String,
    device: DeviceId,
    keys: Keys,
    tag: [u8; 8],
    published: Option<(u64, Millis, Vec<SocketAddr>)>,
    publish_retry_at: Option<Millis>,
    last_poll: Option<Millis>,
    polling: bool,
    answered: Option<[u8; 16]>,
}

/// A wake request a path took, handled once the paths are unlocked.
struct Pending {
    tag: [u8; 8],
    session: [u8; 16],
    target: String,
    device: DeviceId,
    payload: Vec<u8>,
    from: SocketAddr,
}

/// A punched path to one device's session.
struct Path {
    target: String,
    device: DeviceId,
    session: [u8; 16],
    /// Unix timestamp of the punch or DHT record that opened this path.
    time: u64,
    puncher: Puncher,
    opener: Opener,
    sealer: Sealer,
    peer: Option<SocketAddr>,
    heard: Millis,
}

impl Path {
    /// `first` is the first sequence number its sealer sends ([`Shared::next_seq`]).
    fn new(
        state: &DeviceState,
        keys: PathKeys,
        session: [u8; 16],
        time: u64,
        first: u64,
        now: Millis,
    ) -> Self {
        Self {
            target: state.target.clone(),
            device: state.device,
            session,
            time,
            heard: now,
            puncher: Puncher::new(Vec::new(), now),
            opener: Opener::new(&keys.client_to_host, keys.tag),
            sealer: Sealer::starting_at(&keys.host_to_client, keys.tag, session, first),
            peer: None,
        }
    }
}

/// Active paths alongside recently ended or superseded `(tag, session)` pairs.
#[derive(Default)]
struct Paths {
    active: HashMap<[u8; 8], Path>,
    ended: HashMap<([u8; 8], [u8; 16]), Millis>,
}

impl Paths {
    fn prune_ended(&mut self, now: Millis) {
        self.ended
            .retain(|_, ended_at| now.saturating_sub(*ended_at) < ENDED_SESSION_HOLD);
    }

    fn is_ended(&mut self, tag: [u8; 8], session: [u8; 16], now: Millis) -> bool {
        self.prune_ended(now);
        self.ended.contains_key(&(tag, session))
    }

    fn mark_ended(&mut self, tag: [u8; 8], session: [u8; 16], now: Millis) {
        self.prune_ended(now);
        self.ended.insert((tag, session), now);
    }

    /// Installs `path` for `tag`, recording any replaced session as superseded.
    fn replace(&mut self, tag: [u8; 8], path: Path, now: Millis) {
        let session = path.session;
        if let Some(old) = self.active.insert(tag, path)
            && old.session != session
        {
            self.mark_ended(tag, old.session, now);
        }
    }

    /// Ends `tag`'s active path and remembers its session for [`ENDED_SESSION_HOLD`].
    fn end(&mut self, tag: [u8; 8], now: Millis) {
        if let Some(old) = self.active.remove(&tag) {
            self.mark_ended(tag, old.session, now);
        }
    }
}

struct Shared {
    node: Node,
    config: RelayConfig,
    session: [u8; 16],
    lan: Arc<dyn Lan>,
    enrollments: Mutex<Vec<Enrollment>>,
    devices: Mutex<Vec<DeviceState>>,
    /// Locked before `devices` when a thread needs both.
    paths: Mutex<Paths>,
    relay: Mutex<Relay>,
    status: Mutex<RelayStatus>,
    publishing: AtomicBool,
    /// Where the next path's sequence numbers start. A path's key depends only on this relay's
    /// session and the device's, and a path for the same device session can be built again (a
    /// punch arriving after the last one failed or closed): each path takes its own
    /// [`PATH_SEQUENCES`], so no nonce repeats under a key.
    next_seq: AtomicU64,
    stop: AtomicBool,
}

#[derive(Clone)]
pub struct WakeRelayPeer {
    shared: Arc<Shared>,
}

impl WakeRelayPeer {
    pub fn start(config: RelayConfig, lan: Arc<dyn Lan>) -> io::Result<Self> {
        let node = Node::bind(config.bind, config.bootstrap.clone())?;
        let shared = Arc::new(Shared {
            node: node.clone(),
            config,
            session: crate::crypto::random(),
            lan,
            enrollments: Mutex::new(Vec::new()),
            devices: Mutex::new(Vec::new()),
            paths: Mutex::new(Paths::default()),
            relay: Mutex::new(Relay::new()),
            status: Mutex::new(RelayStatus::default()),
            publishing: AtomicBool::new(false),
            next_seq: AtomicU64::new(1),
            stop: AtomicBool::new(false),
        });
        let handling = Arc::downgrade(&shared);
        node.set_handler(Box::new(move |datagram, from| {
            if let Some(shared) = handling.upgrade() {
                WakeRelayPeer { shared }.receive(datagram, from);
            }
        }));
        let peer = Self { shared };
        let running = peer.clone();
        thread::Builder::new()
            .name("wake relay".into())
            .spawn(move || running.run())?;
        Ok(peer)
    }

    /// This relay's session nonce, which its records carry.
    pub fn session(&self) -> [u8; 16] {
        self.shared.session
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.shared.node.local_addr()
    }

    /// The PCs that enrolled this relay, as they are now: a device a PC no longer lists, or a
    /// whole PC, loses its records' answers and its open path at once.
    pub fn set_enrollments(&self, enrollments: Vec<Enrollment>) {
        let enrollments: Vec<Enrollment> =
            enrollments.into_iter().map(Enrollment::sanitized).collect();
        let mut paths = self.shared.paths.lock().unwrap();
        let mut states = self.shared.devices.lock().unwrap();
        let mut next = Vec::new();
        for enrollment in &enrollments {
            for device in &enrollment.devices {
                let keys = Keys::new(enrollment.device_secret(device));
                let tag = keys.device_tag(&self.shared.session);
                match states.iter().position(|state| state.tag == tag) {
                    Some(index) => next.push(states.swap_remove(index)),
                    None => next.push(DeviceState {
                        target: enrollment.target_id.clone(),
                        device: *device,
                        keys,
                        tag,
                        published: None,
                        publish_retry_at: None,
                        last_poll: None,
                        polling: false,
                        answered: None,
                    }),
                }
            }
        }
        let now = self.shared.node.now();
        let dropped: Vec<([u8; 8], [u8; 16])> = paths
            .active
            .iter()
            .filter(|(tag, _)| !next.iter().any(|state| state.tag == **tag))
            .map(|(&tag, path)| (tag, path.session))
            .collect();
        for (tag, session) in dropped {
            paths.active.remove(&tag);
            paths.mark_ended(tag, session, now);
        }
        paths
            .ended
            .retain(|(tag, _), _| next.iter().any(|state| state.tag == *tag));
        let mut status = self.shared.status.lock().unwrap();
        status.targets = enrollments.len();
        status.devices = next.len();
        *states = next;
        *self.shared.enrollments.lock().unwrap() = enrollments;
    }

    pub fn status(&self) -> RelayStatus {
        self.shared.status.lock().unwrap().clone()
    }

    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::Release);
        self.shared.node.stop();
    }

    /// The first sequence number of a new path's range.
    fn first_seq(&self) -> u64 {
        self.shared
            .next_seq
            .fetch_add(PATH_SEQUENCES, Ordering::Relaxed)
    }

    fn stopped(&self) -> bool {
        self.shared.stop.load(Ordering::Acquire)
    }

    fn send(&self, path: &Path, to: SocketAddr, kind: Kind, payload: &[u8]) {
        self.shared
            .node
            .send(to, &path.sealer.seal(kind, 0, payload));
    }

    /// Takes a datagram from the socket. Only one that opens with the keys of a device some PC
    /// lists gets anything back.
    fn receive(&self, datagram: &mut [u8], from: SocketAddr) {
        let Some(tag) = wire::tag(datagram) else {
            return;
        };
        let punched = wire::punch_session(datagram);
        let now = self.shared.node.now();
        let mut paths = self.shared.paths.lock().unwrap();
        if let Some(path) = paths.active.get_mut(&tag)
            && punched.is_none_or(|session| session == path.session)
        {
            if let Some(opened) = path.opener.open(datagram)
                && let Some(request) = self.on_datagram(&mut paths, tag, opened, from)
            {
                drop(paths);
                self.on_request(request);
            }
            return;
        }
        // A punch for a session of its own: a device starting a new request.
        let Some(session) = punched else { return };
        if paths.is_ended(tag, session, now) {
            return;
        }
        let devices = self.shared.devices.lock().unwrap();
        let Some(state) = devices.iter().find(|state| state.tag == tag) else {
            return;
        };
        let keys = state.keys.path(&self.shared.session, &session);
        let mut opener = Opener::new(&keys.client_to_host, keys.tag);
        let Some(opened) = opener.open(datagram) else {
            return;
        };
        // Only a fresh punch opens a path: a captured one replayed later starts nothing.
        let Some(punch_time) = (opened.kind == Kind::Punch)
            .then(|| wire::punch_time(&opened.payload))
            .flatten()
        else {
            return;
        };
        if unix_now().abs_diff(punch_time) > record::CLIENT_FRESH_SECONDS {
            return;
        }
        if paths
            .active
            .get(&tag)
            .is_some_and(|existing| punch_time < existing.time)
        {
            return;
        }
        let mut path = Path::new(state, keys, session, punch_time, self.first_seq(), now);
        drop(devices);
        path.opener = opener;
        paths.replace(tag, path, now);
        if let Some(request) = self.on_datagram(&mut paths, tag, opened, from) {
            drop(paths);
            self.on_request(request);
        }
    }

    /// Answers what a path needs at once; a wake request comes back, for [`Self::on_request`]
    /// to handle once the paths are unlocked (a burst and a look at the interfaces take time).
    fn on_datagram(
        &self,
        paths: &mut Paths,
        tag: [u8; 8],
        opened: Opened,
        from: SocketAddr,
    ) -> Option<Pending> {
        let now = self.shared.node.now();
        let path = paths.active.get_mut(&tag)?;
        path.heard = now;
        path.peer = Some(from);
        match opened.kind {
            Kind::Punch => {
                path.puncher.on_punch(from);
                self.send(path, from, Kind::PunchAck, &[]);
            }
            Kind::PunchAck => path.puncher.on_ack(from),
            Kind::WakeRequest => {
                return Some(Pending {
                    tag,
                    session: path.session,
                    target: path.target.clone(),
                    device: path.device,
                    payload: opened.payload,
                    from,
                });
            }
            Kind::Close => {
                paths.end(tag, now);
            }
            _ => {}
        }
        None
    }

    fn on_request(&self, request: Pending) {
        let Some(enrollment) = self
            .shared
            .enrollments
            .lock()
            .unwrap()
            .iter()
            .find(|enrollment| enrollment.target_id == request.target)
            .cloned()
        else {
            return;
        };
        let now = unix_now();
        let interfaces = self.shared.lan.interfaces();
        let decision = self.shared.relay.lock().unwrap().handle(
            &enrollment,
            &request.device,
            request.from,
            &request.payload,
            now,
            &interfaces,
        );
        let reply = match decision {
            Decision::Silent => return,
            Decision::Reply(reply) => reply,
            Decision::Wake { reply, burst } => {
                self.shared.lan.send(&burst);
                self.watch(&enrollment, request.tag, request.session, reply.nonce);
                reply
            }
        };
        self.shared.status.lock().unwrap().last_wake =
            Some((enrollment.target_name.clone(), now, reply.status));
        let paths = self.shared.paths.lock().unwrap();
        if let Some(path) = paths
            .active
            .get(&request.tag)
            .filter(|path| path.session == request.session)
        {
            self.send(path, request.from, Kind::WakeStatus, &reply.encode());
        }
    }

    /// After a burst: looks whether the PC answers on its network, and tells the device when it
    /// does or when [`crate::wake_relay::BOOT_WAIT_SECONDS`] ran out ([`FINAL_COPIES`] times, as
    /// one lost datagram must not hide the outcome).
    fn watch(&self, enrollment: &Enrollment, tag: [u8; 8], session: [u8; 16], nonce: [u8; 16]) {
        let Some(target) = enrollment
            .address
            .map(|address| SocketAddrV4::new(address, enrollment.http_port))
        else {
            return;
        };
        let name = enrollment.target_name.clone();
        let watching = self.clone();
        let _ = thread::Builder::new()
            .name("wake relay watch".into())
            .spawn(move || {
                let started = std::time::Instant::now();
                let mut last_keepalive = started;
                let wait = Duration::from_secs(crate::wake_relay::BOOT_WAIT_SECONDS);
                let status = loop {
                    thread::sleep(PROBE_EVERY);
                    if watching.stopped() {
                        return;
                    }
                    let peer_alive = {
                        let paths = watching.shared.paths.lock().unwrap();
                        paths
                            .active
                            .get(&tag)
                            .filter(|path| path.session == session)
                            .map(|path| path.peer)
                    };
                    let Some(_) = peer_alive else {
                        return;
                    };
                    if watching.shared.lan.is_up(target) {
                        break Status::HostAwake;
                    }
                    if started.elapsed() >= wait {
                        break Status::TimedOut;
                    }
                    if last_keepalive.elapsed() >= WAIT_KEEPALIVE {
                        let paths = watching.shared.paths.lock().unwrap();
                        if let Some(path) = paths
                            .active
                            .get(&tag)
                            .filter(|path| path.session == session)
                            && let Some(peer) = path.peer
                        {
                            let mut waiting = WakeStatus::new(Status::WaitingForBoot, nonce);
                            waiting.elapsed_ms =
                                u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);
                            watching.send(path, peer, Kind::WakeStatus, &waiting.encode());
                        }
                        last_keepalive = std::time::Instant::now();
                    }
                };
                let mut answer = WakeStatus::new(status, nonce);
                answer.elapsed_ms =
                    u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);
                watching.shared.status.lock().unwrap().last_wake = Some((name, unix_now(), status));
                for copy in 0..FINAL_COPIES {
                    if copy > 0 {
                        thread::sleep(FINAL_GAP);
                    }
                    let paths = watching.shared.paths.lock().unwrap();
                    let Some(path) = paths
                        .active
                        .get(&tag)
                        .filter(|path| path.session == session)
                    else {
                        return;
                    };
                    if let Some(peer) = path.peer {
                        watching.send(path, peer, Kind::WakeStatus, &answer.encode());
                    }
                }
            });
    }

    fn run(&self) {
        let mut next_stun: Millis = 0;
        while !self.stopped() {
            let now = self.shared.node.now();
            if now >= next_stun && !self.shared.config.stun_servers.is_empty() {
                next_stun = now
                    + if self.discover() {
                        STUN_EVERY
                    } else {
                        STUN_RETRY
                    };
            }
            self.serve_devices();
            self.drive_paths();
            thread::sleep(Duration::from_millis(100));
        }
    }

    fn discover(&self) -> bool {
        let answers = self.shared.node.stun(&self.shared.config.stun_servers);
        let local = self.shared.node.local_addr().ok();
        let mut public: Vec<SocketAddr> = Vec::new();
        for answer in &answers {
            if !public.contains(answer) {
                public.push(*answer);
            }
        }
        let nat = local.map_or(Nat::Unknown, |local| {
            Nat::from_mapping(stun::mapping(local, &answers))
        });
        let mut status = self.shared.status.lock().unwrap();
        status.stun_error = answers.is_empty().then(|| {
            "no STUN server answered: this device may have no internet connection, or a firewall blocks UDP".to_owned()
        });
        status.public = public;
        status.nat = nat;
        !answers.is_empty()
    }

    /// Punches toward devices whose request record named this relay, and drops idle paths.
    fn drive_paths(&self) {
        let now = self.shared.node.now();
        let stamp = wire::punch_stamp(unix_now());
        let mut paths = self.shared.paths.lock().unwrap();
        paths.prune_ended(now);
        let expired: Vec<([u8; 8], [u8; 16])> = paths
            .active
            .iter()
            .filter(|(_, path)| {
                path.puncher.state() == punch::State::Failed
                    || now.saturating_sub(path.heard) >= PATH_IDLE
            })
            .map(|(&tag, path)| (tag, path.session))
            .collect();
        for (tag, session) in expired {
            paths.active.remove(&tag);
            paths.mark_ended(tag, session, now);
        }
        for path in paths.active.values_mut() {
            for to in path.puncher.poll(now) {
                self.send(path, to, Kind::Punch, &stamp);
            }
        }
    }
}

mod worker;

#[cfg(test)]
mod tests;
