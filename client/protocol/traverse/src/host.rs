//! The host's side of the rendezvous: it finds its public address with STUN, publishes its
//! record for every device that has a secret, looks for those devices' records, and answers
//! their punches with a tunnel to its own ports.

use crate::{
    crypto::Key,
    node::{unix_now, Node},
    punch,
    record::{self, Keys, Nat, Ports, Record, Role},
    stun,
    tunnel::{self, Bridge, State, Tunnel},
    wire, Millis,
};
use std::{
    collections::HashMap,
    io,
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

/// STUN again this often: a router's public address can change.
const STUN_EVERY: Millis = 5 * 60_000;
/// Republish a record this often, well inside the two hours DHT nodes keep it.
const PUBLISH_EVERY: Millis = 30 * 60_000;
/// Look for each device's record this often.
const POLL_EVERY: Millis = 10_000;

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
}

struct DeviceState {
    device: Device,
    keys: Keys,
    tag: [u8; 8],
    published: Option<(u64, Millis, Vec<SocketAddr>)>,
    last_poll: Option<Millis>,
    answered: Option<[u8; 16]>,
}

struct Shared {
    node: Node,
    config: HostConfig,
    session: [u8; 16],
    devices: Mutex<Vec<DeviceState>>,
    tunnels: Mutex<HashMap<[u8; 8], ([u8; 16], Tunnel)>>,
    status: Mutex<Status>,
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
            tunnels: Mutex::new(HashMap::new()),
            status: Mutex::new(Status::default()),
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
                        answered: None,
                    });
                }
            }
        }
        *states = next;
        let tags: Vec<[u8; 8]> = states.iter().map(|state| state.tag).collect();
        drop(states);
        let mut tunnels = self.shared.tunnels.lock().unwrap();
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
            .lock()
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
        status
    }

    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::Release);
        for (_, (_, tunnel)) in self.shared.tunnels.lock().unwrap().drain() {
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

    /// Starts (or restarts) the path to a device for its session `client`.
    fn open(
        &self,
        state: &DeviceState,
        client: [u8; 16],
        candidates: Vec<SocketAddr>,
    ) -> Option<Tunnel> {
        let mut tunnels = self.shared.tunnels.lock().unwrap();
        if let Some((session, tunnel)) = tunnels.get(&state.tag) {
            if *session == client && !tunnel.is_closed() {
                tunnel.add_candidates(&candidates);
                return Some(tunnel.clone());
            }
        }
        let keys = state.keys.path(&self.shared.session, &client);
        let bridge = Bridge::Host {
            service: self.shared.config.service,
            source: self.source(&state.device),
            ports: self.shared.config.ports,
        };
        let tunnel = match Tunnel::start(
            self.shared.node.clone(),
            Role::Host,
            keys,
            client,
            candidates,
            bridge,
        ) {
            Ok(tunnel) => tunnel,
            Err(error) => {
                self.shared.status.lock().unwrap().dht_error = Some(format!(
                    "a path to {} could not start: {error}",
                    state.device.name
                ));
                return None;
            }
        };
        if let Some((_, old)) = tunnels.insert(state.tag, (client, tunnel.clone())) {
            old.close("the device connected again");
        }
        Some(tunnel)
    }

    fn receive(&self, datagram: &[u8], from: SocketAddr) {
        let Some(tag) = wire::tag(datagram) else {
            return;
        };
        let existing = self.shared.tunnels.lock().unwrap().get(&tag).cloned();
        let punch_session = wire::punch_session(datagram);
        if let Some((session, tunnel)) = &existing {
            if !tunnel.is_closed() && punch_session.map_or(true, |punched| punched == *session) {
                tunnel.receive(datagram, from);
                return;
            }
            // A new session replaces a connected path only once the old one is gone, so an old
            // punch replayed by someone on the way can't break a working stream.
            if !tunnel.is_closed() && matches!(tunnel.state(), State::Connected(_)) {
                return;
            }
        }
        let Some(client) = punch_session else { return };
        let devices = self.shared.devices.lock().unwrap();
        let Some(state) = devices.iter().find(|state| state.tag == tag) else {
            return;
        };
        // Only a punch that opens with this device's keys starts a path.
        let keys = state.keys.path(&self.shared.session, &client);
        if wire::Opener::new(&keys.client_to_host, keys.tag)
            .open(datagram)
            .is_none()
        {
            return;
        }
        let tunnel = self.open(state, client, vec![from]);
        drop(devices);
        if let Some(tunnel) = tunnel {
            tunnel.receive(datagram, from);
        }
    }

    fn run(&self) {
        let mut next_stun: Millis = 0;
        while !self.stopped() {
            let now = self.shared.node.now();
            if now >= next_stun {
                next_stun = now + STUN_EVERY;
                self.discover();
            }
            self.serve_devices();
            self.shared
                .tunnels
                .lock()
                .unwrap()
                .retain(|_, (_, tunnel)| !tunnel.is_closed());
            for _ in 0..10 {
                if self.stopped() {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
    }

    fn discover(&self) {
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
        let changed = {
            let mut status = self.shared.status.lock().unwrap();
            status.stun_error = answers.is_empty().then(|| {
                "no STUN server answered: the PC may have no internet connection, or a firewall blocks UDP".to_owned()
            });
            let changed = status.public != public;
            status.public = public;
            status.nat = nat;
            changed
        };
        // A changed address is republished right away.
        if changed {
            for state in self.shared.devices.lock().unwrap().iter_mut() {
                state.published = None;
            }
        }
    }

    fn serve_devices(&self) {
        let ids: Vec<String> = self
            .shared
            .devices
            .lock()
            .unwrap()
            .iter()
            .map(|state| state.device.id.clone())
            .collect();
        let (public, nat) = {
            let status = self.shared.status.lock().unwrap();
            (status.public.clone(), status.nat)
        };
        let mut published = 0;
        for id in ids {
            if self.stopped() {
                return;
            }
            let unix = unix_now();
            let window = record::window(unix);
            let now = self.shared.node.now();
            // Copy what the slow DHT work needs, so the lock isn't held through it.
            let (keys, publish, poll) = {
                let devices = self.shared.devices.lock().unwrap();
                let Some(state) = devices.iter().find(|state| state.device.id == id) else {
                    continue;
                };
                let publish = !public.is_empty()
                    && match &state.published {
                        None => true,
                        Some((at_window, at, endpoints)) => {
                            *at_window != window || now - at > PUBLISH_EVERY || *endpoints != public
                        }
                    };
                let poll = state.last_poll.map_or(true, |at| now - at >= POLL_EVERY);
                (state.keys.clone(), publish, poll)
            };
            if publish {
                let record = Record {
                    role: Role::Host,
                    time: unix,
                    session: self.shared.session,
                    peer_session: [0; 16],
                    nat,
                    endpoints: public.clone(),
                    ports: Some(self.shared.config.ports),
                };
                let confirmed =
                    self.shared
                        .node
                        .put(&record.item(&keys, window, crate::crypto::random()));
                let mut devices = self.shared.devices.lock().unwrap();
                if let Some(state) = devices.iter_mut().find(|state| state.device.id == id) {
                    if confirmed > 0 {
                        state.published = Some((window, now, public.clone()));
                    }
                }
                drop(devices);
                self.shared.status.lock().unwrap().dht_error = (confirmed == 0).then(|| {
                    "no DHT node stored the PC's record: UDP to the internet may be blocked"
                        .to_owned()
                });
            }
            if poll {
                self.poll_device(&id, &keys, unix);
            }
            let devices = self.shared.devices.lock().unwrap();
            if devices
                .iter()
                .any(|state| state.device.id == id && state.published.is_some())
            {
                published += 1;
            }
        }
        self.shared.status.lock().unwrap().published = published;
    }

    fn poll_device(&self, id: &str, keys: &Keys, unix: u64) {
        let window = record::window(unix);
        let mut found = None;
        for window in [window, window.saturating_sub(1)] {
            let (item, _) = self
                .shared
                .node
                .get(&keys.public(Role::Client), &keys.salt(Role::Client, window));
            if let Some(record) =
                item.and_then(|item| Record::from_item(keys, Role::Client, window, &item))
            {
                found = Some(record);
                break;
            }
        }
        let now = self.shared.node.now();
        let mut devices = self.shared.devices.lock().unwrap();
        let Some(state) = devices.iter_mut().find(|state| state.device.id == id) else {
            return;
        };
        state.last_poll = Some(now);
        let Some(record) = found else { return };
        let fresh = unix.abs_diff(record.time) <= record::CLIENT_FRESH_SECONDS;
        if !fresh
            || record.peer_session != self.shared.session
            || state.answered == Some(record.session)
        {
            return;
        }
        state.answered = Some(record.session);
        let candidates = punch::candidates(&record.endpoints, record.nat);
        self.open(state, record.session, candidates);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> HostConfig {
        HostConfig {
            ports: Ports::default(),
            service: "127.0.0.1".parse().unwrap(),
            per_device_source: false,
            bind: "127.0.0.1:0".parse().unwrap(),
            stun_servers: Vec::new(),
            bootstrap: Vec::new(),
        }
    }

    #[test]
    fn only_a_paired_devices_punch_opens_a_path() {
        let host = HostService::start(config()).unwrap();
        let device = Device {
            id: "a".into(),
            name: "Phone".into(),
            secret: [3; 32],
        };
        host.set_devices(vec![device.clone()]);
        let keys = Keys::new(device.secret);
        let path = keys.path(&host.session(), &[9; 16]);
        let probe = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let from = probe.local_addr().unwrap();

        // A stranger's punch with the right tag but another secret opens nothing.
        let stranger = Keys::new([4; 32]).path(&host.session(), &[9; 16]);
        let forged = wire::Sealer::new(&stranger.client_to_host, path.tag, [9; 16]).seal(
            wire::Kind::Punch,
            0,
            &[],
        );
        host.receive(&forged, from);
        assert!(host.status().paths.is_empty());

        let punch = wire::Sealer::new(&path.client_to_host, path.tag, [9; 16]).seal(
            wire::Kind::Punch,
            0,
            &[],
        );
        host.receive(&punch, from);
        let status = host.status();
        assert_eq!(status.paths.len(), 1);
        assert_eq!(status.paths[0].device, "Phone");
        assert!(status.paths[0].connected);

        // Unpairing closes it.
        host.set_devices(Vec::new());
        assert!(host.status().paths.is_empty());
        host.stop();
    }
}
