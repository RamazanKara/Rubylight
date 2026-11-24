//! In-process rusty_enet test host and multi-homed socket harness for control loopback tests.

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::thread;
use std::time::{Duration, Instant};

use nightjar_core::control::driver::{connect, DriverConfig};
use nightjar_core::control::negotiation::encryption;
use nightjar_core::control::{HostCapabilities, SessionOptions};
use rubylight_wire::packet::{decrypt_control, encrypted_control};
use rusty_enet::{Event, Host, HostSettings, Packet, PacketReceived, PeerID, MTU_MAX};

pub(crate) const KEY: [u8; 16] = *b"loopback-key-016";
pub(crate) const CONNECT_DATA: u32 = 0x1234_5678;

pub(crate) fn caps() -> HostCapabilities {
    HostCapabilities {
        app_version: [7, 1, 431, -1],
        feature_flags: 3,
        encryption_enabled: encryption::CONTROL_V2,
        control_messages: vec![0x5532, 0x5537],
        client_roaming: false,
    }
}

/// A wildcard-bound host socket on a multi-homed machine (nightjar#38): datagrams arrive on
/// `reached`, the address the client connects to (or on `routed` once the client follows
/// the replies), and every reply leaves from `routed`, as when the OS picks the source by
/// route (Tailscale routing the LAN, IPv6 temporary addresses, a second IP).
pub(crate) struct MultiHomed {
    pub(crate) reached: UdpSocket,
    pub(crate) routed: UdpSocket,
}

impl rusty_enet::Socket for MultiHomed {
    type Address = SocketAddr;
    type Error = io::Error;

    fn init(&mut self, _: rusty_enet::SocketOptions) -> io::Result<()> {
        self.reached.set_nonblocking(true)?;
        self.routed.set_nonblocking(true)
    }

    fn send(&mut self, address: SocketAddr, buffer: &[u8]) -> io::Result<usize> {
        self.routed.send_to(buffer, address)
    }

    fn receive(
        &mut self,
        buffer: &mut [u8; MTU_MAX],
    ) -> io::Result<Option<(SocketAddr, PacketReceived)>> {
        for socket in [&self.reached, &self.routed] {
            match socket.recv_from(buffer) {
                Ok((length, from)) => return Ok(Some((from, PacketReceived::Complete(length)))),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::ConnectionReset
                    ) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(None)
    }
}

pub(crate) struct TestHost<S: rusty_enet::Socket = UdpSocket> {
    pub(crate) host: Host<S>,
    pub(crate) peer: Option<PeerID>,
    pub(crate) sequence: u32,
    /// (channel, message type, payload) of each client message.
    pub(crate) received: Vec<(u8, u16, Vec<u8>)>,
    pub(crate) disconnected: bool,
}

impl TestHost {
    pub(crate) fn new() -> Self {
        Self::on(UdpSocket::bind("127.0.0.1:0").unwrap())
    }

    pub(crate) fn address(&self) -> SocketAddr {
        self.host.socket().local_addr().unwrap()
    }

    /// A host whose ENet grants the client its smallest reliable window (4096 bytes).
    pub(crate) fn with_small_window() -> Self {
        let mut host = Self::new();
        host.host = Host::new(
            UdpSocket::bind("127.0.0.1:0").unwrap(),
            HostSettings {
                peer_limit: 4,
                channel_limit: 255,
                incoming_bandwidth_limit: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
        host
    }
}

impl TestHost<MultiHomed> {
    /// None when this machine cannot bind one of the addresses.
    pub(crate) fn multi_homed(reached: &str, routed: &str) -> Option<Self> {
        Some(Self::on(MultiHomed {
            reached: UdpSocket::bind(reached).ok()?,
            routed: UdpSocket::bind(routed).ok()?,
        }))
    }

    pub(crate) fn address(&self) -> SocketAddr {
        self.host.socket().reached.local_addr().unwrap()
    }
}

impl<S: rusty_enet::Socket<Address = SocketAddr, Error = io::Error>> TestHost<S> {
    pub(crate) fn on(socket: S) -> Self {
        let settings = HostSettings {
            peer_limit: 4,
            channel_limit: 255,
            ..Default::default()
        };
        Self {
            host: Host::new(socket, settings).unwrap(),
            peer: None,
            sequence: 0,
            received: Vec::new(),
            disconnected: false,
        }
    }

    pub(crate) fn pump(&mut self) {
        loop {
            let event = match self.host.service() {
                Ok(Some(event)) => event,
                Ok(None) => break,
                // Windows hands a UDP socket the ICMP reset of a client port that closed.
                Err(e) if e.kind() == io::ErrorKind::ConnectionReset => break,
                Err(e) => panic!("host service: {e}"),
            };
            match event {
                Event::Connect { peer, data } => {
                    assert_eq!(data, CONNECT_DATA);
                    assert_eq!(peer.channel_count(), 0x30);
                    self.peer = Some(peer.id());
                }
                Event::Receive {
                    channel_id, packet, ..
                } => {
                    let (_, kind, payload) = decrypt_control(&KEY, packet.data(), true).unwrap();
                    self.received.push((channel_id, kind, payload));
                }
                Event::Disconnect { .. } => self.disconnected = true,
            }
        }
    }

    pub(crate) fn until(&mut self, what: &str, mut done: impl FnMut(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done(self) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            self.pump();
            thread::sleep(Duration::from_millis(1));
        }
    }

    pub(crate) fn send(&mut self, kind: u16, payload: &[u8]) {
        self.sequence += 1;
        let datagram = encrypted_control(&KEY, self.sequence, kind, payload, true).unwrap();
        let peer = self.peer.expect("connected");
        self.host
            .peer_mut(peer)
            .send(0, &Packet::reliable(datagram))
            .unwrap();
        self.host.flush();
    }

    pub(crate) fn has(&self, kind: u16) -> bool {
        self.received.iter().any(|m| m.1 == kind)
    }
}

pub(crate) fn config(host: &TestHost) -> DriverConfig {
    DriverConfig {
        connect_data: CONNECT_DATA,
        ..DriverConfig::new(host.address())
    }
}

/// Connects while the test host services, returning the client and handle.
pub(crate) fn connect_to(
    host: &mut TestHost,
) -> (
    nightjar_core::control::driver::ControlClient,
    nightjar_core::control::driver::ControlHandle,
) {
    let config = config(host);
    let connecting =
        thread::spawn(move || connect(config, caps(), &KEY, SessionOptions::default()).unwrap());
    host.until("connect", |h| h.peer.is_some());
    while !connecting.is_finished() {
        host.pump();
        thread::sleep(Duration::from_millis(1));
    }
    connecting.join().unwrap()
}
