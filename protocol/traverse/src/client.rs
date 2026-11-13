//! The device's side: find the host's record in the DHT, publish its own, punch, and bind the
//! host's ports on a loopback address once the path is through.

use crate::{
    crypto::Key,
    node::{any_ipv4, unix_now, Node},
    punch,
    record::{self, Keys, Nat, Ports, Record, Role},
    stun,
    tunnel::{self, Bridge, State, Tunnel},
};
use std::{
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
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

/// How long the punching may take, from the record's publication: the host looks for it every
/// ten seconds, and the DHT takes a few more.
const PUNCH_WAIT: Duration = Duration::from_secs(45);

struct Shared {
    status: Mutex<Status>,
    tunnel: Mutex<Option<Tunnel>>,
    node: Mutex<Option<Node>>,
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
            });
        if let Err(error) = spawned {
            tunnel.set(Status::Failed(format!("could not start: {error}")));
        }
        tunnel
    }

    pub fn status(&self) -> Status {
        let status = self.shared.status.lock().unwrap().clone();
        if let Status::Connected { .. } = status {
            // A path that ended since is reported as such.
            if let Some(tunnel) = self.shared.tunnel.lock().unwrap().as_ref() {
                if let State::Closed(reason) = tunnel.state() {
                    return Status::Failed(format!(
                        "the connection over the internet ended: {reason}"
                    ));
                }
            }
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
        if let Some(tunnel) = self.shared.tunnel.lock().unwrap().take() {
            tunnel.close("the device disconnected");
        }
        if let Some(node) = self.shared.node.lock().unwrap().take() {
            node.stop();
        }
        self.set(Status::Stopped);
    }

    fn set(&self, status: Status) {
        let mut current = self.shared.status.lock().unwrap();
        if *current != Status::Stopped {
            *current = status;
        }
    }

    fn stopped(&self) -> bool {
        self.shared.stop.load(Ordering::Acquire)
    }

    fn connect(&self, config: ClientConfig) -> Result<(), String> {
        let node = Node::bind(any_ipv4(), config.bootstrap.clone())
            .map_err(|error| format!("no UDP socket for the internet path: {error}"))?;
        *self.shared.node.lock().unwrap() = Some(node.clone());
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

        let keys = Keys::new(config.secret);
        let unix = unix_now();
        let window = record::window(unix);
        let mut host = None;
        for window in [window, window.saturating_sub(1)] {
            let (item, _) = node.get(&keys.public(Role::Host), &keys.salt(Role::Host, window));
            if let Some(record) =
                item.and_then(|item| Record::from_item(&keys, Role::Host, window, &item))
            {
                host = Some(record);
                break;
            }
            if self.stopped() {
                return Ok(());
            }
        }
        let (session, endpoints, host_nat, ports) = match (host, &config.known) {
            (Some(record), _) => {
                let ports = record.ports.unwrap_or_default();
                (record.session, record.endpoints, record.nat, ports)
            }
            (
                None,
                Some(HostInfo {
                    session: Some(session),
                    endpoints,
                    nat,
                    ports,
                }),
            ) if !endpoints.is_empty() => (*session, endpoints.clone(), *nat, *ports),
            _ => {
                return Err(if answers.is_empty() {
                    "this device's network blocks UDP to the internet".to_owned()
                } else {
                    "the PC's record was not found: the PC may be off or asleep, or Play over the internet is off on it"
                        .to_owned()
                })
            }
        };
        if ports.http == 0 || ports.https == 0 {
            return Err("the PC did not say which ports it uses".to_owned());
        }

        let local = tunnel::bindable_loopback(tunnel::loopback_for(78, &config.host_id));
        let client_session: [u8; 16] = crate::crypto::random();
        let path = keys.path(&session, &client_session);
        let candidates = punch::candidates(&endpoints, host_nat);
        let tunnel = Tunnel::start(
            node.clone(),
            Role::Client,
            path,
            client_session,
            candidates,
            Bridge::Device { local, ports },
        )
        .map_err(|error| {
            format!("the PC's ports could not be opened on this device ({local}): {error}")
        })?;
        let receiving = tunnel.clone();
        node.set_handler(Box::new(move |datagram, from| {
            receiving.receive(datagram, from)
        }));
        *self.shared.tunnel.lock().unwrap() = Some(tunnel.clone());
        self.set(Status::Punching);

        // The host learns this device's endpoints and session from its record.
        let record = Record {
            role: Role::Client,
            time: unix_now(),
            session: client_session,
            peer_session: session,
            nat,
            endpoints: own,
            ports: None,
        };
        let stored =
            node.put(&record.item(&keys, record::window(record.time), crate::crypto::random()));

        let deadline = Instant::now() + PUNCH_WAIT;
        while Instant::now() < deadline && !self.stopped() {
            match tunnel.state() {
                State::Connected(peer) => {
                    self.set(Status::Connected {
                        address: local,
                        peer,
                    });
                    return Ok(());
                }
                State::Closed(_) => break,
                State::Punching => thread::sleep(Duration::from_millis(50)),
            }
        }
        if self.stopped() {
            return Ok(());
        }
        tunnel.close("no path");
        Err(if nat == Nat::Symmetric && host_nat == Nat::Symmetric {
            "both networks give each destination another port (symmetric NAT), so no direct path opens; forward the ports on the PC's router, or use IPv6".to_owned()
        } else if stored == 0 {
            "this device's record could not be stored in the DHT, so the PC could not answer"
                .to_owned()
        } else {
            "the PC did not answer: it may be off or asleep, or its network blocks the path"
                .to_owned()
        })
    }
}
