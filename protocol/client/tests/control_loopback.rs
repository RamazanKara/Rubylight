//! The ENet driver against an in-process rusty_enet host that seals and opens messages with
//! the Rubylight host's own helpers.

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use butterpollo_core::input::{decode, Input};
use butterpollo_core::packet::{decrypt_control, encrypted_control};
use nightjar_core::control::driver::{connect, ConnectError, DriverConfig, Outcome, Warn};
use nightjar_core::control::negotiation::encryption;
use nightjar_core::control::{ClientMessage, ControlEvent, HostCapabilities, SessionOptions};
use rubylight_protocol::control::{ClockSyncReply, Reconfigure};
use rusty_enet::{Event, Host, HostSettings, Packet, PacketReceived, PeerID, MTU_MAX};

const KEY: [u8; 16] = *b"loopback-key-016";
const CONNECT_DATA: u32 = 0x1234_5678;

fn caps() -> HostCapabilities {
    HostCapabilities {
        app_version: [7, 1, 431, -1],
        feature_flags: 3,
        encryption_enabled: encryption::CONTROL_V2,
        control_messages: vec![0x5532, 0x5537],
    }
}

/// A wildcard-bound host socket on a multi-homed machine (nightjar#38): datagrams arrive on
/// `reached`, the address the client connects to (or on `routed` once the client follows
/// the replies), and every reply leaves from `routed`, as when the OS picks the source by
/// route (Tailscale routing the LAN, IPv6 temporary addresses, a second IP).
struct MultiHomed {
    reached: UdpSocket,
    routed: UdpSocket,
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

struct TestHost<S: rusty_enet::Socket = UdpSocket> {
    host: Host<S>,
    peer: Option<PeerID>,
    sequence: u32,
    /// (channel, message type, payload) of each client message.
    received: Vec<(u8, u16, Vec<u8>)>,
    disconnected: bool,
}

impl TestHost {
    fn new() -> Self {
        Self::on(UdpSocket::bind("127.0.0.1:0").unwrap())
    }

    fn address(&self) -> SocketAddr {
        self.host.socket().local_addr().unwrap()
    }
}

impl TestHost<MultiHomed> {
    /// None when this machine cannot bind one of the addresses.
    fn multi_homed(reached: &str, routed: &str) -> Option<Self> {
        Some(Self::on(MultiHomed {
            reached: UdpSocket::bind(reached).ok()?,
            routed: UdpSocket::bind(routed).ok()?,
        }))
    }

    fn address(&self) -> SocketAddr {
        self.host.socket().reached.local_addr().unwrap()
    }
}

impl<S: rusty_enet::Socket<Address = SocketAddr, Error = io::Error>> TestHost<S> {
    fn on(socket: S) -> Self {
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

    fn pump(&mut self) {
        while let Some(event) = self.host.service().unwrap() {
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

    fn until(&mut self, what: &str, mut done: impl FnMut(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done(self) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            self.pump();
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn send(&mut self, kind: u16, payload: &[u8]) {
        self.sequence += 1;
        let datagram = encrypted_control(&KEY, self.sequence, kind, payload, true).unwrap();
        let peer = self.peer.expect("connected");
        self.host
            .peer_mut(peer)
            .send(0, &Packet::reliable(datagram))
            .unwrap();
        self.host.flush();
    }

    fn has(&self, kind: u16) -> bool {
        self.received.iter().any(|m| m.1 == kind)
    }
}

fn config(host: &TestHost) -> DriverConfig {
    DriverConfig {
        connect_data: CONNECT_DATA,
        ..DriverConfig::new(host.address())
    }
}

/// Connects while the test host services, returning the client and handle.
fn connect_to(
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

#[test]
fn messages_flow_both_ways_and_stop_disconnects_gracefully() {
    let mut host = TestHost::new();
    let (client, handle) = connect_to(&mut host);
    let (events_tx, events) = mpsc::channel();
    let running = thread::spawn(move || client.run(move |event| events_tx.send(event).unwrap()));

    host.until("start and ping", |h| {
        h.has(0x0302) && h.has(0x0307) && h.has(0x0200)
    });
    let first: Vec<u16> = host.received.iter().map(|m| m.1).take(2).collect();
    assert_eq!(first, [0x0302, 0x0307]);

    handle
        .with_input(|input| input.send_keyboard(0x41, true, 0, 0))
        .unwrap();
    handle
        .send(ClientMessage::Reconfigure(Reconfigure {
            width: 1920,
            height: 1080,
            fps_millihz: 120_000,
        }))
        .unwrap();
    host.until("input and reconfigure", |h| h.has(0x0206) && h.has(0x5532));
    let inputs: Vec<Input> = host
        .received
        .iter()
        .filter(|m| m.1 == 0x0206)
        .map(|m| decode(&m.2).unwrap())
        .collect();
    assert_eq!(inputs[0], Input::Haptics(true));
    assert!(inputs.contains(&Input::Keyboard {
        key: 0x41,
        modifiers: 0,
        flags: 0,
        down: true
    }));
    let key_channel = host
        .received
        .iter()
        .find(|m| m.1 == 0x0206 && m.2.len() == 14)
        .unwrap()
        .0;
    assert_eq!(key_channel, 0x02);
    let reconfigure = host.received.iter().find(|m| m.1 == 0x5532).unwrap();
    assert_eq!(Reconfigure::decode(&reconfigure.2).unwrap().width, 1920);

    host.send(0x010b, &[0, 0, 0, 0, 2, 0, 0x00, 0x80, 0xff, 0xff]);
    let reply = ClockSyncReply {
        sequence: 1,
        client_us: 5,
        host_receive_us: 10,
        host_send_us: 12,
    };
    host.send(0x5537, &reply.encode());
    let mut seen = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while seen.len() < 2 {
        assert!(Instant::now() < deadline, "events: {seen:?}");
        host.pump();
        if let Ok(event) = events.recv_timeout(Duration::from_millis(5)) {
            seen.push(event);
        }
    }
    assert_eq!(
        seen[0],
        ControlEvent::Rumble {
            controller: 2,
            low_frequency: 0x8000,
            high_frequency: 0xffff
        }
    );
    assert!(matches!(seen[1], ControlEvent::ClockSync { reply: r, .. } if r == reply));
    assert!(handle.round_trip_time().is_some());

    // A key release queued right before stop still reaches the host.
    handle
        .with_input(|input| input.send_keyboard(0x41, false, 0, 0))
        .unwrap();
    handle.stop();
    host.until("disconnect", |h| h.disconnected);
    assert_eq!(running.join().unwrap(), Outcome::Stopped);
    let released = host.received.iter().filter(|m| m.1 == 0x0206).any(|m| {
        decode(&m.2).unwrap()
            == Input::Keyboard {
                key: 0x41,
                modifiers: 0,
                flags: 0,
                down: false,
            }
    });
    assert!(released, "the release was drained before the disconnect");
    assert!(handle.send(ClientMessage::RequestIdrFrame).is_err());
}

#[test]
fn a_remote_monitor_connection_sends_no_input_at_all() {
    let mut host = TestHost::new();
    let config = DriverConfig {
        input_disabled: true,
        ..config(&host)
    };
    let connecting =
        thread::spawn(move || connect(config, caps(), &KEY, SessionOptions::default()).unwrap());
    while !connecting.is_finished() {
        host.pump();
        thread::sleep(Duration::from_millis(1));
    }
    let (client, handle) = connecting.join().unwrap();
    let running = thread::spawn(move || client.run(|_| {}));
    host.until("start and ping", |h| h.has(0x0302) && h.has(0x0200));

    assert!(handle
        .with_input(|input| input.send_keyboard(0x41, true, 0, 0))
        .is_err());
    assert!(handle
        .with_input(|input| input.send_mouse_move(1, 1))
        .is_err());
    // Control traffic still flows (start and ping arrived); only input is refused.
    handle.stop();
    host.until("disconnect", |h| h.disconnected);
    assert_eq!(running.join().unwrap(), Outcome::Stopped);
    assert!(
        !host.has(0x0206),
        "no input packet, not even the haptics enable"
    );
}

#[test]
fn termination_followed_by_disconnect_now_keeps_the_reason() {
    let mut host = TestHost::new();
    let (client, handle) = connect_to(&mut host);
    handle.with_session(|session| session.frame_seen(1, Instant::now()));
    let (events_tx, events) = mpsc::channel();
    let running = thread::spawn(move || client.run(move |event| events_tx.send(event).unwrap()));
    host.until("start", |h| h.has(0x0307));

    // Sunshine forks send their end message and disconnect at once.
    host.send(0x0109, &0x8003_0023u32.to_be_bytes());
    let peer = host.peer.unwrap();
    host.host.peer_mut(peer).disconnect_now(0);
    host.host.flush();

    let deadline = Instant::now() + Duration::from_secs(5);
    while !running.is_finished() {
        assert!(Instant::now() < deadline, "client did not end");
        host.pump();
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(running.join().unwrap(), Outcome::Terminated(0));
    let all: Vec<ControlEvent> = events.try_iter().collect();
    assert_eq!(all, [ControlEvent::Terminated { error: 0 }]);
}

#[test]
fn host_disconnect_without_reason_terminates() {
    let mut host = TestHost::new();
    let (client, _handle) = connect_to(&mut host);
    let running = thread::spawn(move || client.run(|_| {}));
    host.until("start", |h| h.has(0x0307));
    let peer = host.peer.unwrap();
    host.host.peer_mut(peer).disconnect(0);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !running.is_finished() {
        assert!(Instant::now() < deadline, "client did not end");
        host.pump();
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(running.join().unwrap(), Outcome::Terminated(-1));
}

#[test]
fn input_is_held_while_the_host_is_silent_during_a_grace_period() {
    let mut host = TestHost::new();
    let config = DriverConfig {
        network_grace: Duration::from_secs(20),
        ..config(&host)
    };
    let connecting =
        thread::spawn(move || connect(config, caps(), &KEY, SessionOptions::default()).unwrap());
    while !connecting.is_finished() {
        host.pump();
        thread::sleep(Duration::from_millis(1));
    }
    let (client, handle) = connecting.join().unwrap();
    let running = thread::spawn(move || client.run(|_| {}));
    host.until("start", |h| h.has(0x0200));

    // The host goes quiet: no acknowledgements, so the client stops hearing it.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !handle.contact_lost() {
        assert!(Instant::now() < deadline, "contact never lost");
        thread::sleep(Duration::from_millis(20));
    }
    handle.with_input(|input| {
        input.send_keyboard(0x42, true, 0, 0).unwrap();
        input.send_keyboard(0x42, false, 0, 0).unwrap();
        input.send_keyboard(0x43, true, 0, 0).unwrap();
        input.send_mouse_move(5, 5).unwrap();
    });
    thread::sleep(Duration::from_millis(50));
    assert!(handle.with_input(|input| input.is_holding()));

    let keys = |h: &TestHost| -> Vec<Input> {
        h.received
            .iter()
            .filter(|m| m.1 == 0x0206)
            .map(|m| decode(&m.2).unwrap())
            .filter(|i| matches!(i, Input::Keyboard { .. } | Input::Relative { .. }))
            .collect()
    };
    host.until("held input", |h| keys(h).len() >= 2);
    assert_eq!(
        keys(&host),
        [
            Input::Keyboard {
                key: 0x42,
                modifiers: 0,
                flags: 0,
                down: false
            },
            Input::Keyboard {
                key: 0x43,
                modifiers: 0,
                flags: 0,
                down: true
            },
        ]
    );
    assert!(!handle.contact_lost());
    handle.stop();
    host.until("disconnect", |h| h.disconnected);
    assert_eq!(running.join().unwrap(), Outcome::Stopped);
}

#[test]
fn connect_times_out_without_a_host() {
    let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
    let config = DriverConfig {
        connect_timeout: Duration::from_millis(300),
        ..DriverConfig::new(silent.local_addr().unwrap())
    };
    let started = Instant::now();
    let result = connect(config, caps(), &KEY, SessionOptions::default());
    assert!(matches!(result, Err(ConnectError::TimedOut)));
    assert!(started.elapsed() < Duration::from_secs(3));
}

/// nightjar#38: connects to a host that replies from `routed` although the client sent to
/// `reached`, and keeps hearing it.
fn connect_to_a_multi_homed_host(reached: &str, routed: &str) {
    let Some(mut host) = TestHost::multi_homed(reached, routed) else {
        eprintln!("skipped: cannot bind {reached} and {routed}");
        return;
    };
    let warnings = Arc::new(Mutex::new(Vec::<String>::new()));
    let config = DriverConfig {
        connect_data: CONNECT_DATA,
        connect_timeout: Duration::from_secs(4),
        // With a grace period, contact_lost() says whether the replies count as the host's.
        network_grace: Duration::from_secs(20),
        warn: Warn::new({
            let warnings = warnings.clone();
            move |m| warnings.lock().unwrap().push(m.to_string())
        }),
        ..DriverConfig::new(host.address())
    };
    let connecting =
        thread::spawn(move || connect(config, caps(), &KEY, SessionOptions::default()));
    while !connecting.is_finished() {
        host.pump();
        thread::sleep(Duration::from_millis(1));
    }
    let (client, handle) = match connecting.join().unwrap() {
        Ok(connected) => connected,
        Err(e) => panic!("the control stream connects to a multi-homed host: {e:?}"),
    };
    host.until("connect", |h| h.peer.is_some());
    let running = thread::spawn(move || client.run(|_| {}));
    host.until("start and ping", |h| h.has(0x0302) && h.has(0x0200));
    // The host keeps acknowledging for longer than INPUT_CONTACT_LOST_AFTER.
    let until = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < until {
        host.pump();
        thread::sleep(Duration::from_millis(1));
    }
    assert!(
        !handle.contact_lost(),
        "input held back although the host answers"
    );
    handle.stop();
    host.until("disconnect", |h| h.disconnected);
    assert_eq!(running.join().unwrap(), Outcome::Stopped);
    let warnings = warnings.lock().unwrap();
    assert!(
        !warnings
            .iter()
            .any(|w| w.starts_with("No answer to the first control connects")),
        "{warnings:?}"
    );
}

#[test]
fn connects_to_a_host_that_replies_from_another_port() {
    connect_to_a_multi_homed_host("127.0.0.1:0", "127.0.0.1:0");
}

#[test]
fn connects_to_a_host_that_replies_from_another_ip() {
    connect_to_a_multi_homed_host("127.0.0.1:0", "127.0.0.2:0");
}
