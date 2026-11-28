//! The device's side: find the host's record in the DHT, publish its own, punch, and bind the
//! host's ports on a loopback address once the path is through.

use crate::{
    crypto::Key,
    node::{any_ipv4, unix_now, Node, LOOKUP_DEADLINE, STUN_WAIT},
    punch,
    record::{self, Keys, Nat, Ports, Record, Role},
    stun,
    tunnel::{self, Bridge, State, Tunnel},
    Millis,
};
use std::{
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

/// What a device learned about the host on the LAN (`/internet-play/v1`): used while the DHT
/// has nothing newer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostInfo {
    pub session: Option<[u8; 16]>,
    pub endpoints: Vec<SocketAddr>,
    pub nat: Nat,
    pub ports: Ports,
}

#[derive(Clone, Debug)]
pub struct ClientConfig {
    /// The host's unique id: names the loopback address the tunnel binds.
    pub host_id: String,
    pub secret: Key,
    pub known: Option<HostInfo>,
    pub stun_servers: Vec<String>,
    pub bootstrap: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Finding this device's public address and the host's record.
    LookingUp,
    /// Both sides are punching.
    Punching,
    /// Through: the client connects to `address`, the host's ports at a loopback address.
    Connected {
        address: IpAddr,
        peer: SocketAddr,
    },
    Failed(String),
    Stopped,
}

/// How long an old path's sockets take to close: its threads wake at least twice a second.
const REPLACE_WAIT: Millis = 2_000;

const fn max(a: Millis, b: Millis) -> Millis {
    if a > b {
        a
    } else {
        b
    }
}

/// The longest [`ClientTunnel`] takes to connect or fail: STUN beside the PC's record in two
/// windows, a path replaced when that record names a newer session, this device's record, then
/// [`punch::AFTER_PUBLISH`]. A caller that waits for it waits this long.
pub const CONNECT_WAIT: Duration = Duration::from_millis(
    max(STUN_WAIT, 2 * LOOKUP_DEADLINE) + REPLACE_WAIT + punch::PUBLISH + punch::AFTER_PUBLISH,
);

/// Where the PC is: from its record in the DHT, or from what the LAN told.
#[derive(Clone, Debug)]
struct Target {
    session: [u8; 16],
    endpoints: Vec<SocketAddr>,
    nat: Nat,
    ports: Ports,
}

impl Target {
    fn from_record(record: Record) -> Self {
        Self {
            session: record.session,
            ports: record.ports.unwrap_or_default(),
            endpoints: record.endpoints,
            nat: record.nat,
        }
    }

    fn known(info: &HostInfo) -> Option<Self> {
        let session = info.session?;
        let usable = !info.endpoints.is_empty() && info.ports.http != 0 && info.ports.https != 0;
        usable.then(|| Self {
            session,
            endpoints: info.endpoints.clone(),
            nat: info.nat,
            ports: info.ports,
        })
    }
}

/// A path being punched towards a [`Target`].
struct Path {
    target: Target,
    tunnel: Tunnel,
}

/// This device's record on its way into the DHT, and how long punching may go on.
struct Publication {
    stored: mpsc::Receiver<usize>,
    /// How many DHT nodes took the record, once the put returned.
    confirmed: Option<usize>,
    /// The record's time, also its DHT sequence number.
    time: u64,
    /// Until the put returns: its longest run, then [`punch::AFTER_PUBLISH`]. After: that long
    /// from its return.
    deadline: Instant,
}

impl Publication {
    fn poll(&mut self, node: &Node, tunnel: &Tunnel) {
        if self.confirmed.is_some() {
            return;
        }
        let confirmed = match self.stored.try_recv() {
            Ok(confirmed) => confirmed,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => 0,
        };
        self.confirmed = Some(confirmed);
        self.deadline = Instant::now() + Duration::from_millis(punch::AFTER_PUBLISH);
        tunnel.hold_punching_until(node.now() + punch::AFTER_PUBLISH);
    }
}

/// Looks the PC's record up on a thread of its own, in this window and the previous one. The
/// answer is None when there is none, or when the thread could not start.
fn look_up(node: &Node, keys: &Keys) -> mpsc::Receiver<Option<Record>> {
    let (sender, receiver) = mpsc::channel();
    let (node, keys) = (node.clone(), keys.clone());
    let _ = thread::Builder::new()
        .name("internet play lookup".into())
        .spawn(move || {
            let window = record::window(unix_now());
            let mut found = None;
            for window in [window, window.saturating_sub(1)] {
                if node.is_stopped() {
                    break;
                }
                let (item, _) = node.get(&keys.public(Role::Host), &keys.salt(Role::Host, window));
                found = item.and_then(|item| Record::from_item(&keys, Role::Host, window, &item));
                if found.is_some() {
                    break;
                }
            }
            let _ = sender.send(found);
        });
    receiver
}

struct Shared {
    status: Mutex<Status>,
    tunnel: Mutex<Option<Tunnel>>,
    node: Mutex<Option<Node>>,
    ended_at: Mutex<Option<Instant>>,
    stop: AtomicBool,
}

#[derive(Clone)]
pub struct ClientTunnel {
    shared: Arc<Shared>,
}

impl ClientTunnel {
    /// Starts connecting in the background; [`ClientTunnel::status`] tells how it goes.
    pub fn start(config: ClientConfig) -> Self {
        let shared = Arc::new(Shared {
            status: Mutex::new(Status::LookingUp),
            tunnel: Mutex::new(None),
            node: Mutex::new(None),
            ended_at: Mutex::new(None),
            stop: AtomicBool::new(false),
        });
        let tunnel = Self { shared };
        let working = tunnel.clone();
        let spawned = thread::Builder::new()
            .name("internet play connect".into())
            .spawn(move || {
                let outcome = working.connect(config);
                if let Err(reason) = outcome {
                    working.set(Status::Failed(reason));
                }
                // The path is gone or never came: nothing listens on the node any more.
                if let Some(node) = working.shared.node.lock().unwrap().take() {
                    node.stop();
                }
            });
        if let Err(error) = spawned {
            tunnel.set(Status::Failed(format!("could not start: {error}")));
        }
        tunnel
    }

    pub fn status(&self) -> Status {
        let status = self.shared.status.lock().unwrap().clone();
        if let Status::Connected { .. } = status
            // A path that ended since is reported as such.
            && let Some(tunnel) = self.shared.tunnel.lock().unwrap().as_ref()
            && let State::Closed(reason) = tunnel.state()
        {
            return Status::Failed(format!(
                "the connection over the internet ended: {reason}"
            ));
        }
        status
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self.status(),
            Status::LookingUp | Status::Punching | Status::Connected { .. }
        )
    }

    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::Release);
        {
            let mut ended = self.shared.ended_at.lock().unwrap();
            ended.get_or_insert_with(Instant::now);
        }
        if let Some(tunnel) = self.shared.tunnel.lock().unwrap().take() {
            tunnel.close("the device disconnected");
        }
        if let Some(node) = self.shared.node.lock().unwrap().take() {
            node.set_handler(Box::new(|_, _| {}));
            node.stop();
        }
        self.set(Status::Stopped);
    }

    pub fn ended_at(&self) -> Option<Instant> {
        *self.shared.ended_at.lock().unwrap()
    }

    fn set(&self, status: Status) {
        let is_terminal = matches!(status, Status::Failed(_) | Status::Stopped);
        let mut current = self.shared.status.lock().unwrap();
        if *current != Status::Stopped {
            *current = status;
        }
        drop(current);
        if is_terminal {
            let mut ended = self.shared.ended_at.lock().unwrap();
            ended.get_or_insert_with(Instant::now);
        }
    }

    fn stopped(&self) -> bool {
        self.shared.stop.load(Ordering::Acquire)
    }

    fn connect(&self, config: ClientConfig) -> Result<(), String> {
        let node = Node::bind(any_ipv4(), config.bootstrap.clone())
            .map_err(|error| format!("no UDP socket for the internet path: {error}"))?;
        *self.shared.node.lock().unwrap() = Some(node.clone());
        let keys = Keys::new(config.secret);
        let local = tunnel::bindable_loopback(tunnel::loopback_for(78, &config.host_id));
        let client_session: [u8; 16] = crate::crypto::random();
        let mut lookup = Some(look_up(&node, &keys));
        // What the LAN told is punched at once, while STUN and the DHT are asked: the punches
        // open this device's NAT towards the PC early. The PC's record, when it names another
        // session, replaces it.
        let known = match config.known.as_ref().and_then(Target::known) {
            Some(target) => {
                Some(self.open_path_retrying(&node, &keys, client_session, local, target)?)
            }
            None => None,
        };

        let answers = node.stun(&config.stun_servers);
        let mut own: Vec<SocketAddr> = Vec::new();
        for answer in &answers {
            if !own.contains(answer) {
                own.push(*answer);
            }
        }
        let nat = node.local_addr().map_or(Nat::Unknown, |local| {
            Nat::from_mapping(stun::mapping(local, &answers))
        });
        if self.stopped() {
            return Ok(());
        }

        let mut path = match known {
            Some(path) => path,
            None => {
                // Nothing to punch yet: the PC's record decides.
                let mut found = None;
                if let Some(receiver) = lookup.take() {
                    while !self.stopped() {
                        match receiver.recv_timeout(Duration::from_millis(100)) {
                            Ok(record) => {
                                found = record;
                                break;
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    }
                }
                if self.stopped() {
                    return Ok(());
                }
                let Some(record) = found else {
                    return Err(if answers.is_empty() {
                        "this device's network blocks UDP to the internet".to_owned()
                    } else {
                        "the PC's record was not found: the PC may be off or asleep, or Play over the internet is off on it"
                            .to_owned()
                    });
                };
                self.open_path_retrying(
                    &node,
                    &keys,
                    client_session,
                    local,
                    Target::from_record(record),
                )?
            }
        };
        let mut publication = self.publish(&node, &keys, &path, client_session, nat, &own, 0);

        let mut closed_reason = None;
        while !self.stopped() {
            match path.tunnel.state() {
                State::Connected(peer) => {
                    self.set(Status::Connected {
                        address: local,
                        peer,
                    });
                    // Until the path closes ([`ClientTunnel::stop`], the PC, or no more use).
                    path.tunnel.wait_closed();
                    return Ok(());
                }
                State::Closed(reason) => {
                    closed_reason = Some(reason);
                    break;
                }
                State::Punching => {}
            }
            let found = match lookup.as_ref().map(mpsc::Receiver::try_recv) {
                Some(Ok(found)) => {
                    lookup = None;
                    found
                }
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    lookup = None;
                    None
                }
                Some(Err(mpsc::TryRecvError::Empty)) | None => None,
            };
            if let Some(target) = found.map(Target::from_record) {
                if target.session == path.target.session {
                    path.tunnel
                        .add_candidates(&punch::candidates(&target.endpoints, target.nat));
                } else {
                    // The PC started again since the LAN told: a new path, and a record for it.
                    path = self.replace_path(&node, &keys, client_session, local, path, target)?;
                    let after = publication.time + 1;
                    publication =
                        self.publish(&node, &keys, &path, client_session, nat, &own, after);
                }
            }
            publication.poll(&node, &path.tunnel);
            if Instant::now() >= publication.deadline {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        if self.stopped() {
            return Ok(());
        }
        path.tunnel.close("no path");
        Err(match closed_reason.as_deref() {
            Some(reason) if reason != "no punch got through" => reason.to_owned(),
            _ if nat == Nat::Symmetric && path.target.nat == Nat::Symmetric => {
                "both networks give each destination another port (symmetric NAT), so no direct path opens; forward the ports on the PC's router, or use IPv6".to_owned()
            }
            _ if publication.confirmed == Some(0) => {
                "this device's record could not be stored in the DHT, so the PC could not answer"
                    .to_owned()
            }
            _ => {
                "the PC did not answer: it may be off or asleep, or its network blocks the path"
                    .to_owned()
            }
        })
    }

    /// Starts punching `target`, retrying briefly on `AddrInUse` while a previous tunnel's
    /// local TCP/UDP sockets finish closing.
    fn open_path_retrying(
        &self,
        node: &Node,
        keys: &Keys,
        client_session: [u8; 16],
        local: IpAddr,
        target: Target,
    ) -> Result<Path, String> {
        let deadline = Instant::now() + Duration::from_millis(REPLACE_WAIT);
        loop {
            match self.open_path(node, keys, client_session, local, target.clone()) {
                Err((true, _)) if Instant::now() < deadline && !self.stopped() => {
                    thread::sleep(Duration::from_millis(25))
                }
                Err((_, message)) => return Err(message),
                Ok(path) => return Ok(path),
            }
        }
    }

    /// Starts punching `target` and makes it the path the node's datagrams go to.
    /// Returns `(addr_in_use, message)` on error so callers can retry transient socket teardown.
    fn open_path(
        &self,
        node: &Node,
        keys: &Keys,
        client_session: [u8; 16],
        local: IpAddr,
        target: Target,
    ) -> Result<Path, (bool, String)> {
        if target.ports.http == 0 || target.ports.https == 0 {
            return Err((false, "the PC did not say which ports it uses".to_owned()));
        }
        let tunnel = Tunnel::start(
            node.clone(),
            Role::Client,
            keys.path(&target.session, &client_session),
            client_session,
            punch::candidates(&target.endpoints, target.nat),
            Bridge::Device {
                local,
                ports: target.ports,
            },
        )
        .map_err(|error| {
            (
                error.kind() == std::io::ErrorKind::AddrInUse,
                format!("the PC's ports could not be opened on this device ({local}): {error}"),
            )
        })?;
        let receiving = tunnel.clone();
        node.set_handler(Box::new(move |datagram, from| {
            receiving.receive(datagram, from)
        }));
        *self.shared.tunnel.lock().unwrap() = Some(tunnel.clone());
        if self.stopped() {
            // `stop` ran meanwhile and closed the path before this one, or none.
            tunnel.close("the device disconnected");
        }
        self.set(Status::Punching);
        Ok(Path { target, tunnel })
    }

    /// Closes `old` and opens a path to `target` on the same ports once `old` let them go.
    fn replace_path(
        &self,
        node: &Node,
        keys: &Keys,
        client_session: [u8; 16],
        local: IpAddr,
        old: Path,
        target: Target,
    ) -> Result<Path, String> {
        old.tunnel.close("the PC started again");
        node.set_handler(Box::new(|_, _| {}));
        self.shared.tunnel.lock().unwrap().take();
        drop(old);
        self.open_path_retrying(node, keys, client_session, local, target)
    }

    /// Stores this device's record for `path` on a thread of its own, so punching goes on
    /// meanwhile: the host learns this device's endpoints and session from it. The record's
    /// time is at least `after`, which a record it replaces needs.
    #[allow(clippy::too_many_arguments)]
    fn publish(
        &self,
        node: &Node,
        keys: &Keys,
        path: &Path,
        client_session: [u8; 16],
        nat: Nat,
        own: &[SocketAddr],
        after: u64,
    ) -> Publication {
        let record = Record {
            role: Role::Client,
            time: unix_now().max(after),
            session: client_session,
            peer_session: path.target.session,
            nat,
            endpoints: own.to_vec(),
            ports: None,
        };
        let time = record.time;
        path.tunnel
            .hold_punching_until(punch::device_deadline(node.now()));
        let deadline =
            Instant::now() + Duration::from_millis(punch::PUBLISH + punch::AFTER_PUBLISH);
        let (sender, stored) = mpsc::channel();
        let (node, keys) = (node.clone(), keys.clone());
        let _ = thread::Builder::new()
            .name("internet play publish".into())
            .spawn(move || {
                let item = record.item(&keys, record::window(record.time), crate::crypto::random());
                let _ = sender.send(node.put(&item));
            });
        Publication {
            stored,
            confirmed: None,
            time,
            deadline,
        }
    }
}
