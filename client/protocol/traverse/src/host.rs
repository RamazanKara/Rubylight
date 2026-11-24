//! The host's side of the rendezvous: it finds its public address with STUN, publishes its
//! record for every device that has a secret, looks for those devices' records, and answers
//! their punches with a tunnel to its own ports.

use crate::{
    crypto::Key,
    node::{unix_now, Node},
    punch,
    record::{self, Keys, Nat, Ports, Record, Role},
    tunnel::{self, Bridge, State, Tunnel},
    wire, Millis,
};
use std::{
    collections::{HashMap, HashSet},
    io,
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, RwLock,
    },
    thread,
    time::Duration,
};

/// STUN again this often: a router's public address can change.
const STUN_EVERY: Millis = 5 * 60_000;
/// Republish a record this often, well inside the two hours DHT nodes keep it.
const PUBLISH_EVERY: Millis = 30 * 60_000;
/// Look for the record of a device heard from in the last [`ACTIVE_FOR`] this often,
const POLL_EVERY: Millis = punch::HOST_POLL;
/// and for every other device's this often. The device's punch budget covers this wait.
const POLL_IDLE: Millis = punch::HOST_POLL_IDLE;
/// How long a device counts as active after its record last answered this host, or while it
/// has a path open.
const ACTIVE_FOR: Millis = 10 * 60_000;
// A record published just after a slow poll began is found within the device's punch budget:
// the interval from that poll's start, the loop's lateness, one lookup (both windows run at
// once), and the punches.
const _: () =
    assert!(POLL_IDLE + punch::HOST_LOOP + crate::node::LOOKUP_DEADLINE < punch::AFTER_PUBLISH);
/// A device's path problem shows this long after the device last tried: one still trying
/// renews it with every punch or record.
const PATH_ERROR_FOR: Millis = 30_000;
/// At most this many devices' records are looked for at once, each on a thread of its own, so
/// one device's slow lookups never hold another's back.
const MAX_POLLS: usize = 4;

#[derive(Clone, Debug)]
pub struct HostConfig {
    pub ports: Ports,
    /// Where the host's own services listen: 127.0.0.1 when they listen everywhere.
    pub service: IpAddr,
    /// Tunnels connect from a loopback address per device (127.77.x.y), so the host tells
    /// devices apart; needs `service` on loopback.
    pub per_device_source: bool,
    /// Where the rendezvous socket binds (IPv4, port 0 for any).
    pub bind: SocketAddr,
    pub stun_servers: Vec<String>,
    pub bootstrap: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// A stable id: the host uses the device certificate's SHA-256.
    pub id: String,
    pub name: String,
    pub secret: Key,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PathStatus {
    pub device: String,
    pub peer: Option<SocketAddr>,
    pub connected: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// The rendezvous socket's public addresses, as STUN servers see them.
    pub public: Vec<SocketAddr>,
    pub nat: Nat,
    /// Why STUN found nothing, if it did not.
    pub stun_error: Option<String>,
    /// Devices whose record the DHT took on the last publish.
    pub published: usize,
    pub devices: usize,
    pub dht_error: Option<String>,
    pub paths: Vec<PathStatus>,
    /// Devices whose path did not start lately, by name, and why.
    pub path_errors: Vec<(String, String)>,
}

struct DeviceState {
    device: Device,
    keys: Keys,
    tag: [u8; 8],
    published: Option<(u64, Millis, Vec<SocketAddr>)>,
    last_poll: Option<Millis>,
    /// A look for its record is under way.
    polling: bool,
    /// When a fresh record of the device last named this host's session.
    heard: Option<Millis>,
    answered: Option<[u8; 16]>,
}

/// How often a device's record is looked for: more often while it is trying to connect, or was
/// lately (its record answered this host, or it has a path in `open`), never while its path is
/// connected and heard from (`healthy`): a record can add nothing to that path, and a device
/// that starts again punches through at once ([`HostService::receive`]) or, once its old path
/// goes quiet, is found here again.
fn poll_interval(
    state: &DeviceState,
    open: &[[u8; 8]],
    healthy: &[[u8; 8]],
    now: Millis,
) -> Option<Millis> {
    if healthy.contains(&state.tag) {
        return None;
    }
    let heard = state
        .heard
        .is_some_and(|at| now.saturating_sub(at) < ACTIVE_FOR);
    Some(if heard || open.contains(&state.tag) {
        POLL_EVERY
    } else {
        POLL_IDLE
    })
}

/// What starting a device's path needs, copied out of its state so no lock is held meanwhile.
#[derive(Clone)]
struct Peer {
    tag: [u8; 8],
    keys: Keys,
    device: Device,
}

impl Peer {
    fn of(state: &DeviceState) -> Self {
        Self {
            tag: state.tag,
            keys: state.keys.clone(),
            device: state.device.clone(),
        }
    }
}

/// Each open tunnel by its path tag, with the device session it was opened for.
type Tunnels = HashMap<[u8; 8], ([u8; 16], Tunnel)>;

/// Why a device's path did not start, by its tag: (device name, words, when).
type PathErrors = HashMap<[u8; 8], (String, String, Millis)>;

struct Shared {
    node: Node,
    config: HostConfig,
    session: [u8; 16],
    devices: Mutex<Vec<DeviceState>>,
    /// Read for every datagram, written when a path starts or ends.
    tunnels: RwLock<Tunnels>,
    status: Mutex<Status>,
    path_errors: Mutex<PathErrors>,
    /// Paths a punch is starting (tag and device session), so repeated punches start one.
    starting: Mutex<HashSet<([u8; 8], [u8; 16])>>,
    /// The records are being published.
    publishing: AtomicBool,
    stop: AtomicBool,
}

#[derive(Clone)]
pub struct HostService {
    shared: Arc<Shared>,
}

impl HostService {
    pub fn start(config: HostConfig) -> io::Result<Self> {
        let node = Node::bind(config.bind, config.bootstrap.clone())?;
        let shared = Arc::new(Shared {
            node: node.clone(),
            config,
            session: crate::crypto::random(),
            devices: Mutex::new(Vec::new()),
            tunnels: RwLock::new(HashMap::new()),
            status: Mutex::new(Status::default()),
            path_errors: Mutex::new(HashMap::new()),
            starting: Mutex::new(HashSet::new()),
            publishing: AtomicBool::new(false),
            stop: AtomicBool::new(false),
        });
        let handling = Arc::downgrade(&shared);
        node.set_handler(Box::new(move |datagram, from| {
            if let Some(shared) = handling.upgrade() {
                HostService { shared }.receive(datagram, from);
            }
        }));
        let service = Self { shared };
        let running = service.clone();
        thread::Builder::new()
            .name("internet play host".into())
            .spawn(move || running.run())?;
        Ok(service)
    }

    /// The host session nonce, which `/internet-play/v1` also hands a device on the LAN.
    pub fn session(&self) -> [u8; 16] {
        self.shared.session
    }

    /// The rendezvous socket's address on this PC.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.shared.node.local_addr()
    }

    /// The devices that have a secret; others' paths close.
    pub fn set_devices(&self, devices: Vec<Device>) {
        let mut states = self.shared.devices.lock().unwrap();
        let mut next = Vec::with_capacity(devices.len());
        for device in devices {
            match states.iter().position(|state| state.device == device) {
                Some(index) => next.push(states.swap_remove(index)),
                None => {
                    let keys = Keys::new(device.secret);
                    let tag = keys.device_tag(&self.shared.session);
                    next.push(DeviceState {
                        device,
                        keys,
                        tag,
                        published: None,
                        last_poll: None,
                        polling: false,
                        heard: None,
                        answered: None,
                    });
                }
            }
        }
        *states = next;
        let tags: Vec<[u8; 8]> = states.iter().map(|state| state.tag).collect();
        drop(states);
        let mut tunnels = self.shared.tunnels.write().unwrap();
        tunnels.retain(|tag, (_, tunnel)| {
            let keep = tags.contains(tag);
            if !keep {
                tunnel.close("the device is no longer paired");
            }
            keep
        });
    }

    pub fn status(&self) -> Status {
        let mut status = self.shared.status.lock().unwrap().clone();
        let devices = self.shared.devices.lock().unwrap();
        status.devices = devices.len();
        let names: HashMap<[u8; 8], String> = devices
            .iter()
            .map(|state| (state.tag, state.device.name.clone()))
            .collect();
        drop(devices);
        status.paths = self
            .shared
            .tunnels
            .read()
            .unwrap()
            .iter()
            .filter(|(_, (_, tunnel))| !tunnel.is_closed())
            .map(|(tag, (_, tunnel))| {
                let state = tunnel.state();
                PathStatus {
                    device: names.get(tag).cloned().unwrap_or_default(),
                    peer: match state {
                        State::Connected(peer) => Some(peer),
                        _ => None,
                    },
                    connected: matches!(state, State::Connected(_)),
                }
            })
            .collect();
        let now = self.shared.node.now();
        status.path_errors = self
            .shared
            .path_errors
            .lock()
            .unwrap()
            .values()
            .filter(|(_, _, at)| now.saturating_sub(*at) < PATH_ERROR_FOR)
            .map(|(name, words, _)| (name.clone(), words.clone()))
            .collect();
        status
    }

    /// Notes why `peer`'s path did not start, for [`Status::path_errors`].
    fn path_error(&self, peer: &Peer, words: String) {
        let entry = (peer.device.name.clone(), words, self.shared.node.now());
        self.shared
            .path_errors
            .lock()
            .unwrap()
            .insert(peer.tag, entry);
    }

    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::Release);
        for (_, (_, tunnel)) in self.shared.tunnels.write().unwrap().drain() {
            tunnel.close("the host stopped");
        }
        self.shared.node.stop();
    }

    fn stopped(&self) -> bool {
        self.shared.stop.load(Ordering::Acquire)
    }

    fn source(&self, device: &Device) -> IpAddr {
        if self.shared.config.per_device_source {
            tunnel::loopback_for(77, &device.id)
        } else {
            self.shared.config.service
        }
    }

    /// Without a source address per device, every tunnel reaches the host from the same
    /// address, and the host keys sessions and launches by address: one device at a time, so
    /// one device can never step into another's session.
    fn busy(&self, tunnels: &Tunnels, tag: &[u8; 8]) -> bool {
        !self.shared.config.per_device_source
            && tunnels
                .iter()
                .any(|(other, (_, tunnel))| other != tag && !tunnel.is_closed())
    }

    fn waits(&self, peer: &Peer) {
        self.path_error(peer, format!(
            "{} waits: another device is playing over the internet, and the host listens on one address",
            peer.device.name
        ));
    }

    /// Starts (or restarts) the path to a device for its session `client`. The tunnel starts
    /// (sockets bound, threads started) with no lock held, so other paths' media and the
    /// console's status never wait on it.
    fn open(
        &self,
        peer: &Peer,
        client: [u8; 16],
        epoch: u64,
        candidates: Vec<SocketAddr>,
    ) -> Option<Tunnel> {
        {
            let tunnels = self.shared.tunnels.read().unwrap();
            if let Some((session, tunnel)) = tunnels.get(&peer.tag)
                && *session == client
                && !tunnel.is_closed()
            {
                tunnel.add_candidates(&candidates);
                return Some(tunnel.clone());
            }
            if self.busy(&tunnels, &peer.tag) {
                drop(tunnels);
                self.waits(peer);
                return None;
            }
        }
        let keys = peer.keys.path(&self.shared.session, &client);
        let bridge = Bridge::Host {
            service: self.shared.config.service,
            source: self.source(&peer.device),
            ports: self.shared.config.ports,
            device: peer.device.id.clone(),
        };
        let tunnel = match Tunnel::start(
            self.shared.node.clone(),
            Role::Host,
            keys,
            client,
            candidates.clone(),
            bridge,
        ) {
            Ok(tunnel) => tunnel,
            Err(error) => {
                self.path_error(
                    peer,
                    format!("a path to {} could not start: {error}", peer.device.name),
                );
                return None;
            }
        };
        tunnel.set_epoch(epoch);
        let mut tunnels = self.shared.tunnels.write().unwrap();
        // Meanwhile a path for this session, or another device's path, may have started.
        if let Some((session, existing)) = tunnels.get(&peer.tag)
            && *session == client
            && !existing.is_closed()
        {
            let existing = existing.clone();
            drop(tunnels);
            tunnel.close("the path started twice");
            existing.add_candidates(&candidates);
            return Some(existing);
        }
        if self.busy(&tunnels, &peer.tag) {
            drop(tunnels);
            tunnel.close("another device is playing over the internet");
            self.waits(peer);
            return None;
        }
        let old = tunnels.insert(peer.tag, (client, tunnel.clone()));
        drop(tunnels);
        if let Some((_, old)) = old {
            old.close("the device connected again");
        }
        self.shared.path_errors.lock().unwrap().remove(&peer.tag);
        Some(tunnel)
    }

    /// Takes a datagram for a path; an open path decrypts it in place.
    fn receive(&self, datagram: &mut [u8], from: SocketAddr) {
        let Some(tag) = wire::tag(datagram) else {
            return;
        };
        let punch_session = wire::punch_session(datagram);
        // Set when this punch would replace a connected path: when that path's session began.
        let mut replacing: Option<u64> = None;
        // The map is read-locked only for the lookup, never through the tunnel's handling: a
        // writer (a path starting or ending) then waits for a hash lookup, not for the slowest
        // datagram in flight, and every datagram behind it does not wait for that writer.
        let existing = self
            .shared
            .tunnels
            .read()
            .unwrap()
            .get(&tag)
            .map(|(session, tunnel)| (*session, tunnel.clone()));
        if let Some((session, tunnel)) = &existing {
            if !tunnel.is_closed() && punch_session.is_none_or(|punched| punched == *session) {
                tunnel.receive(datagram, from);
                return;
            }
            // A new session replaces a connected path only once the old one is gone, or by a
            // punch that proves its device started again (below): an old punch replayed by
            // someone on the way can't break a working stream.
            if !tunnel.is_closed() && matches!(tunnel.state(), State::Connected(_)) {
                replacing = Some(tunnel.epoch());
            }
        }
        let Some(client) = punch_session else { return };
        let peer = {
            let devices = self.shared.devices.lock().unwrap();
            let Some(state) = devices.iter().find(|state| state.tag == tag) else {
                return;
            };
            Peer::of(state)
        };
        // Only a punch that opens with this device's keys starts a path.
        let keys = peer.keys.path(&self.shared.session, &client);
        // On a copy: the path the punch starts opens it again.
        let Some(opened) = wire::Opener::new(&keys.client_to_host, keys.tag).open(datagram) else {
            return;
        };
        // The device's clock, from a punch of a device that sends it.
        let stamp = (opened.kind == wire::Kind::Punch)
            .then(|| wire::punch_time(&opened.payload))
            .flatten();
        if let Some(current) = replacing {
            // A connected path whose device started again is replaced at once, without waiting
            // for the old one to time out. Only a punch sealed with this device's keys, sent
            // lately and after the old session began, counts: a captured older punch cannot
            // break a working stream, and one from the current session is no replay of another.
            let fresh = stamp.is_some_and(|time| {
                time > current && unix_now().abs_diff(time) <= record::CLIENT_FRESH_SECONDS
            });
            if !fresh {
                return;
            }
        }
        // A device that has to wait is told here, without a thread for each of its punches.
        if self.busy(&self.shared.tunnels.read().unwrap(), &tag) {
            self.waits(&peer);
            return;
        }
        // The path starts on a thread of its own: this thread takes every path's media. Punches
        // that come meanwhile are dropped; the device sends more.
        if !self.shared.starting.lock().unwrap().insert((tag, client)) {
            return;
        }
        let opening = self.clone();
        let mut datagram = datagram.to_vec();
        let spawned = thread::Builder::new()
            .name("internet play open".into())
            .spawn(move || {
                let tunnel = opening.open(&peer, client, stamp.unwrap_or(0), vec![from]);
                opening
                    .shared
                    .starting
                    .lock()
                    .unwrap()
                    .remove(&(tag, client));
                if let Some(tunnel) = tunnel {
                    tunnel.receive(&mut datagram, from);
                }
            });
        if spawned.is_err() {
            self.shared.starting.lock().unwrap().remove(&(tag, client));
        }
    }
}

mod worker;

#[cfg(test)]
mod tests;
