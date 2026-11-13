//! An in-process NAT simulator: a host and a device behind NATs of each kind RFC 4787 names,
//! exchanging real sealed datagrams ([`crate::wire`]) through simulated mappings and filters,
//! driven by the same [`Puncher`] and [`Reliable`] the tunnel uses. No socket is opened.

use crate::{
    path::{self, Known, Outcome, Path},
    punch::{self, Puncher},
    record::{Keys, Nat as NatKind},
    stream::{Event, Reliable},
    stun,
    wire::{Kind, Opener, Sealer},
    Millis,
};
use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, Ipv4Addr, SocketAddr},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Behaviour {
    /// No NAT: the address is public, and nothing is filtered.
    Open,
    /// Endpoint-independent mapping and filtering.
    FullCone,
    /// Endpoint-independent mapping; lets in what comes from an address it sent to.
    Restricted,
    /// Endpoint-independent mapping; lets in what comes from an address and port it sent to.
    PortRestricted,
    /// A new port per destination, filtered like port-restricted; `sequential` hands ports out
    /// in order, otherwise at random.
    Symmetric { sequential: bool },
}

const STUN_A: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)), 3478);
const STUN_B: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 2)), 3478);
const LATENCY: Millis = 30;
const STEP: Millis = 10;

struct Nat {
    public: Ipv4Addr,
    behaviour: Behaviour,
    mappings: HashMap<(SocketAddr, Option<SocketAddr>), u16>,
    reverse: HashMap<u16, SocketAddr>,
    sent_to: HashSet<(u16, SocketAddr)>,
    /// Ports forwarded on the router, as a port mapping makes them.
    forwards: HashMap<u16, SocketAddr>,
    next_port: u16,
    random: u32,
}

impl Nat {
    fn new(public: Ipv4Addr, behaviour: Behaviour, seed: u32) -> Self {
        Self {
            public,
            behaviour,
            mappings: HashMap::new(),
            reverse: HashMap::new(),
            sent_to: HashSet::new(),
            forwards: HashMap::new(),
            next_port: 30_000,
            random: seed,
        }
    }

    fn outbound(&mut self, internal: SocketAddr, destination: SocketAddr) -> SocketAddr {
        if self.behaviour == Behaviour::Open {
            return internal;
        }
        let key = (
            internal,
            matches!(self.behaviour, Behaviour::Symmetric { .. }).then_some(destination),
        );
        let port = match self.mappings.get(&key) {
            Some(port) => *port,
            None => {
                let port = match self.behaviour {
                    Behaviour::Symmetric { sequential: false } => loop {
                        self.random = self.random.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                        let port = 20_000 + ((self.random >> 8) % 40_000) as u16;
                        if !self.reverse.contains_key(&port) {
                            break port;
                        }
                    },
                    _ => {
                        self.next_port += 1;
                        self.next_port
                    }
                };
                self.mappings.insert(key, port);
                self.reverse.insert(port, internal);
                port
            }
        };
        self.sent_to.insert((port, destination));
        SocketAddr::new(IpAddr::V4(self.public), port)
    }

    /// The internal address a datagram from `from` to public `port` reaches, if it gets in.
    fn inbound(&self, port: u16, from: SocketAddr) -> Option<SocketAddr> {
        if let Some(internal) = self.forwards.get(&port) {
            return Some(*internal);
        }
        let internal = *self.reverse.get(&port)?;
        let allowed = match self.behaviour {
            Behaviour::Open | Behaviour::FullCone => true,
            Behaviour::Restricted => self
                .sent_to
                .iter()
                .any(|(p, to)| *p == port && to.ip() == from.ip()),
            Behaviour::PortRestricted | Behaviour::Symmetric { .. } => {
                self.sent_to.contains(&(port, from))
            }
        };
        allowed.then_some(internal)
    }
}

struct Peer {
    internal: SocketAddr,
    nat: Nat,
    puncher: Option<Puncher>,
    sealer: Sealer,
    opener: Opener,
    reliable: Reliable,
    events: Vec<Event>,
    /// Where this peer's STUN servers saw it, and what that says about its NAT.
    endpoints: Vec<SocketAddr>,
    kind: NatKind,
}

impl Peer {
    fn new(
        internal: SocketAddr,
        nat: Nat,
        send: &[u8; 32],
        receive: &[u8; 32],
        tag: [u8; 8],
        initiator: bool,
    ) -> Self {
        let mut peer = Self {
            internal,
            nat,
            puncher: None,
            sealer: Sealer::new(send, tag, [2; 16]),
            opener: Opener::new(receive, tag),
            reliable: Reliable::new(initiator),
            events: Vec::new(),
            endpoints: Vec::new(),
            kind: NatKind::Unknown,
        };
        let answers = [
            peer.nat.outbound(internal, STUN_A),
            peer.nat.outbound(internal, STUN_B),
        ];
        peer.kind = NatKind::from_mapping(stun::mapping(internal, &answers));
        peer.endpoints = answers.to_vec();
        peer.endpoints.dedup();
        peer
    }

    fn connected(&self) -> Option<SocketAddr> {
        match self.puncher.as_ref()?.state() {
            punch::State::Connected(peer) => Some(peer),
            _ => None,
        }
    }
}

struct World {
    peers: [Peer; 2],
    in_air: Vec<(Millis, usize, SocketAddr, SocketAddr, Vec<u8>)>,
    now: Millis,
}

impl World {
    fn new(host: Behaviour, device: Behaviour) -> Self {
        let keys = Keys::new([7; 32]).path(&[1; 16], &[2; 16]);
        let host_public = Ipv4Addr::new(198, 51, 100, 1);
        let device_public = Ipv4Addr::new(203, 0, 113, 1);
        let address = |public: Ipv4Addr, private: Ipv4Addr, behaviour: Behaviour, port: u16| {
            SocketAddr::new(
                IpAddr::V4(if behaviour == Behaviour::Open {
                    public
                } else {
                    private
                }),
                port,
            )
        };
        let host_peer = Peer::new(
            address(host_public, Ipv4Addr::new(192, 168, 1, 10), host, 40_000),
            Nat::new(host_public, host, 99),
            &keys.host_to_client,
            &keys.client_to_host,
            keys.tag,
            false,
        );
        let device_peer = Peer::new(
            address(device_public, Ipv4Addr::new(10, 0, 0, 5), device, 50_000),
            Nat::new(device_public, device, 7),
            &keys.client_to_host,
            &keys.host_to_client,
            keys.tag,
            true,
        );
        Self {
            peers: [host_peer, device_peer],
            in_air: Vec::new(),
            now: 0,
        }
    }

    fn send(&mut self, from: usize, to: SocketAddr, datagram: Vec<u8>) {
        let peer = &mut self.peers[from];
        let source = peer.nat.outbound(peer.internal, to);
        self.in_air
            .push((self.now + LATENCY, from, source, to, datagram));
    }

    fn deliver(&mut self) {
        let now = self.now;
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.in_air)
            .into_iter()
            .partition(|d| d.0 <= now);
        self.in_air = later;
        for (_, from, source, to, datagram) in due {
            let target = 1 - from;
            let peer = &self.peers[target];
            let reached = if peer.nat.behaviour == Behaviour::Open {
                (peer.internal == to).then_some(to)
            } else if to.ip() == IpAddr::V4(peer.nat.public) {
                peer.nat.inbound(to.port(), source)
            } else {
                None
            };
            if reached != Some(self.peers[target].internal) {
                continue;
            }
            let peer = &mut self.peers[target];
            let Some(opened) = peer.opener.open(&datagram) else {
                continue;
            };
            match opened.kind {
                Kind::Punch => {
                    if let Some(puncher) = peer.puncher.as_mut() {
                        puncher.on_punch(source);
                    }
                    let ack = peer.sealer.seal(Kind::PunchAck, 0, &[]);
                    self.send(target, source, ack);
                }
                Kind::PunchAck => {
                    if let Some(puncher) = peer.puncher.as_mut() {
                        puncher.on_ack(source);
                    }
                }
                Kind::Stream => peer.reliable.handle(&opened.payload, now),
                _ => {}
            }
        }
    }

    fn step(&mut self) {
        for index in 0..2 {
            let now = self.now;
            let punches = self.peers[index]
                .puncher
                .as_mut()
                .map(|puncher| puncher.poll(now))
                .unwrap_or_default();
            for to in punches {
                let punch = self.peers[index].sealer.seal(Kind::Punch, 0, &[]);
                self.send(index, to, punch);
            }
            if let Some(peer_address) = self.peers[index].connected() {
                while let Some(segment) = self.peers[index].reliable.poll_transmit(now) {
                    let sealed = self.peers[index].sealer.seal(Kind::Stream, 0, &segment);
                    self.send(index, peer_address, sealed);
                }
            }
            while let Some(event) = self.peers[index].reliable.poll_event() {
                self.peers[index].events.push(event);
            }
        }
        self.deliver();
        self.now += STEP;
    }

    /// The device starts punching from the host's record; the host starts when its DHT poll
    /// finds the device's record, `host_delay` later. True when both ends connect.
    fn punch(&mut self, host_delay: Millis) -> bool {
        let host_candidates = punch::candidates(&self.peers[1].endpoints, self.peers[1].kind);
        let device_candidates = punch::candidates(&self.peers[0].endpoints, self.peers[0].kind);
        self.peers[1].puncher = Some(Puncher::new(device_candidates, self.now));
        while self.now < punch::TIMEOUT + host_delay + 1_000 {
            if self.now == host_delay {
                self.peers[0].puncher = Some(Puncher::new(host_candidates.clone(), self.now));
            }
            self.step();
            if self.peers[0].connected().is_some() && self.peers[1].connected().is_some() {
                return true;
            }
        }
        false
    }

    /// After punching: an HTTP-like request and reply through the reliable layer.
    fn exchange(&mut self) -> bool {
        let stream = self.peers[1].reliable.open(1);
        self.peers[1].reliable.write(stream, b"GET /serverinfo");
        let mut replied = false;
        for _ in 0..500 {
            self.step();
            if !replied && data(&self.peers[0].events, stream) == b"GET /serverinfo" {
                self.peers[0].reliable.write(stream, b"<root/>");
                replied = true;
            }
            if data(&self.peers[1].events, stream) == b"<root/>" {
                return true;
            }
        }
        false
    }
}

fn data(events: &[Event], stream: u32) -> Vec<u8> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Data { stream: s, data } if *s == stream => Some(data.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

use Behaviour::*;

#[test]
fn stun_classifies_each_behaviour() {
    for (behaviour, kind) in [
        (Open, NatKind::None),
        (FullCone, NatKind::Cone),
        (Restricted, NatKind::Cone),
        (PortRestricted, NatKind::Cone),
        (Symmetric { sequential: true }, NatKind::Symmetric),
        (Symmetric { sequential: false }, NatKind::Symmetric),
    ] {
        assert_eq!(
            World::new(behaviour, Open).peers[0].kind,
            kind,
            "{behaviour:?}"
        );
    }
}

#[test]
fn punching_works_wherever_one_side_lets_the_other_in() {
    for (host, device) in [
        (Open, Symmetric { sequential: false }),
        (FullCone, FullCone),
        (FullCone, Symmetric { sequential: false }),
        (Restricted, PortRestricted),
        (Restricted, Symmetric { sequential: false }),
        (PortRestricted, PortRestricted),
        (PortRestricted, Restricted),
        (Symmetric { sequential: false }, Restricted),
        // A sequential symmetric NAT's next port is among the predicted ones.
        (PortRestricted, Symmetric { sequential: true }),
        (Symmetric { sequential: true }, PortRestricted),
    ] {
        let mut world = World::new(host, device);
        assert!(world.punch(3_000), "host {host:?}, device {device:?}");
        assert!(
            world.exchange(),
            "no exchange: host {host:?}, device {device:?}"
        );
    }
}

#[test]
fn punching_fails_between_two_networks_that_both_filter_by_port_and_hide_theirs() {
    for (host, device) in [
        (PortRestricted, Symmetric { sequential: false }),
        (Symmetric { sequential: false }, PortRestricted),
        (
            Symmetric { sequential: false },
            Symmetric { sequential: false },
        ),
    ] {
        let mut world = World::new(host, device);
        assert!(!world.punch(3_000), "host {host:?}, device {device:?}");
    }
}

#[test]
fn the_device_falls_back_from_the_lan_to_a_mapped_port_to_a_punched_path() {
    let lan: SocketAddr = "192.168.1.10:47989".parse().unwrap();
    let wan: SocketAddr = "198.51.100.1:47989".parse().unwrap();
    let known = Known {
        lan: vec![lan],
        ipv6: Vec::new(),
        mapped: vec![wan],
        punch: true,
    };

    // The router maps nothing, but the NATs let a punched path through.
    let mut world = World::new(PortRestricted, Restricted);
    let reach =
        |world: &mut World, path: Path, address: Option<SocketAddr>| -> Result<(), Outcome> {
            match (path, address) {
                // The device is away: its LAN is another one.
                (Path::Lan, _) => Err(Outcome::NoAnswer),
                (_, Some(address)) => {
                    let device = world.peers[1]
                        .nat
                        .outbound(world.peers[1].internal, address);
                    world.peers[0]
                        .nat
                        .inbound(address.port(), device)
                        .map(|_| ())
                        .ok_or(Outcome::NoAnswer)
                }
                (Path::Punched, None) => {
                    if world.punch(3_000) {
                        Ok(())
                    } else {
                        Err(Outcome::Failed("no punch got through".into()))
                    }
                }
                _ => Err(Outcome::Unknown),
            }
        };
    let (used, ()) =
        path::first_working(&known, |path, address| reach(&mut world, path, address)).unwrap();
    assert_eq!(used, Path::Punched);

    // With the port forwarded on the router the mapped port wins, and nothing is punched.
    let mut world = World::new(PortRestricted, Restricted);
    let host = world.peers[0].internal;
    world.peers[0]
        .nat
        .forwards
        .insert(47989, SocketAddr::new(host.ip(), 47989));
    let (used, ()) =
        path::first_working(&known, |path, address| reach(&mut world, path, address)).unwrap();
    assert_eq!(used, Path::MappedIpv4);
    assert!(world.peers[1].puncher.is_none());

    // Nothing works: the message names every path.
    let mut world = World::new(
        Symmetric { sequential: false },
        Symmetric { sequential: false },
    );
    let error =
        path::first_working(&known, |path, address| reach(&mut world, path, address)).unwrap_err();
    assert!(error.contains("local network: no answer"), "{error}");
    assert!(error.contains("forwarded port: no answer"), "{error}");
    assert!(
        error.contains("direct internet path: no punch got through"),
        "{error}"
    );
}
