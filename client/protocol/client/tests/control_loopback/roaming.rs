//! Control-port roaming, silence probing, seq continuity, and non-roaming loopback tests.

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use nightjar_core::control::driver::{connect, DriverConfig, Outcome};
use nightjar_core::control::{ClientMessage, ControlEvent, HostCapabilities, SessionOptions};
use nightjar_core::roaming::{LaunchSeq, RoamEvent, RoamTrigger};
use rubylight_protocol::control::Reconfigure;
use rubylight_protocol::roaming::{is_roaming, Datagram, Header, Kind, Payload};
use rusty_enet::{PacketReceived, MTU_MAX};

use crate::host::{caps, TestHost, CONNECT_DATA, KEY};

/// A host socket that sets roaming datagrams aside, as the Rubylight host does, and hands
/// the rest to ENet.
struct Roamable {
    socket: UdpSocket,
    roaming: Vec<(SocketAddr, Vec<u8>)>,
    /// When ENet datagrams carrying a PING arrived.
    pings: Vec<Instant>,
}

/// Whether an ENet datagram carries a PING command.
fn has_enet_ping(data: &[u8]) -> bool {
    const SENT_TIME: u16 = 0x8000;
    const PING: u8 = 5;
    let Some(peer) = data.get(..2) else {
        return false;
    };
    let sent_time = u16::from_be_bytes([peer[0], peer[1]]) & SENT_TIME != 0;
    let mut at = if sent_time { 4 } else { 2 };
    let length = |offset: usize| {
        data.get(offset..offset + 2)
            .map(|bytes| usize::from(u16::from_be_bytes([bytes[0], bytes[1]])))
    };
    while let Some(&header) = data.get(at) {
        let size = match header & 0x0F {
            1 | 4 => 8,
            2 => 48,
            3 => 44,
            PING => return true,
            6 => 6 + length(at + 4).unwrap_or(usize::MAX / 2),
            7 | 9 => 8 + length(at + 6).unwrap_or(usize::MAX / 2),
            8 | 12 => 24 + length(at + 6).unwrap_or(usize::MAX / 2),
            10 => 12,
            11 => 16,
            _ => return false,
        };
        at += size;
    }
    false
}

impl rusty_enet::Socket for Roamable {
    type Address = SocketAddr;
    type Error = io::Error;

    fn init(&mut self, _: rusty_enet::SocketOptions) -> io::Result<()> {
        self.socket.set_nonblocking(true)
    }

    fn send(&mut self, address: SocketAddr, buffer: &[u8]) -> io::Result<usize> {
        self.socket.send_to(buffer, address)
    }

    fn receive(
        &mut self,
        buffer: &mut [u8; MTU_MAX],
    ) -> io::Result<Option<(SocketAddr, PacketReceived)>> {
        loop {
            match self.socket.recv_from(buffer) {
                Ok((length, from)) if is_roaming(&buffer[..length]) => {
                    self.roaming.push((from, buffer[..length].to_vec()));
                }
                Ok((length, from)) => {
                    if has_enet_ping(&buffer[..length]) {
                        self.pings.push(Instant::now());
                    }
                    return Ok(Some((from, PacketReceived::Complete(length))));
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::ConnectionReset
                    ) =>
                {
                    return Ok(None)
                }
                Err(e) => return Err(e),
            }
        }
    }
}

impl TestHost<Roamable> {
    fn roamable() -> Self {
        Self::on(Roamable {
            socket: UdpSocket::bind("127.0.0.1:0").unwrap(),
            roaming: Vec::new(),
            pings: Vec::new(),
        })
    }

    /// The roaming payloads received so far, with their senders, opened under the
    /// session's key.
    fn roam_payloads(&self, gcm: &aes_gcm::Aes128Gcm) -> Vec<(SocketAddr, Payload)> {
        self.host
            .socket()
            .roaming
            .iter()
            .map(|(from, data)| {
                let datagram = Datagram::decode(data).unwrap();
                (*from, nightjar_core::roaming::open(gcm, &datagram).unwrap())
            })
            .collect()
    }

    /// The seqs of the roaming datagrams received so far.
    fn roam_seqs(&self) -> Vec<u32> {
        self.host
            .socket()
            .roaming
            .iter()
            .map(|(_, data)| Datagram::decode(data).unwrap().header.seq)
            .collect()
    }

    fn roam_reply(&self, to: SocketAddr, gcm: &aes_gcm::Aes128Gcm, seq: u32, payload: Payload) {
        let (_, session_tag) = nightjar_core::roaming::derive(&KEY);
        let header = Header {
            kind: payload.kind(),
            session_tag,
            seq,
        };
        let datagram = nightjar_core::roaming::seal(gcm, header, payload);
        self.host.socket().socket.send_to(&datagram, to).unwrap();
    }
}

fn roam_gcm() -> aes_gcm::Aes128Gcm {
    use aes_gcm::KeyInit;
    let (key, _) = nightjar_core::roaming::derive(&KEY);
    aes_gcm::Aes128Gcm::new(&key.into())
}

/// Connects to `host` and runs the client, sending its events to the returned receiver.
fn run_roamable(
    host: &mut TestHost<Roamable>,
    caps: HostCapabilities,
) -> (
    thread::JoinHandle<Outcome>,
    nightjar_core::control::driver::ControlHandle,
    mpsc::Receiver<ControlEvent>,
) {
    run_roamable_after(host, caps, LaunchSeq::default())
}

/// [`run_roamable`] for a reconnect under the same key: `roam_seq` holds the seqs the
/// earlier connections sent.
fn run_roamable_after(
    host: &mut TestHost<Roamable>,
    caps: HostCapabilities,
    roam_seq: LaunchSeq,
) -> (
    thread::JoinHandle<Outcome>,
    nightjar_core::control::driver::ControlHandle,
    mpsc::Receiver<ControlEvent>,
) {
    let config = DriverConfig {
        connect_data: CONNECT_DATA,
        roaming: true,
        network_grace: Duration::from_secs(20),
        roam_seq,
        ..DriverConfig::new(host.host.socket().socket.local_addr().unwrap())
    };
    let connecting =
        thread::spawn(move || connect(config, caps, &KEY, SessionOptions::default()).unwrap());
    while !connecting.is_finished() {
        host.pump();
        thread::sleep(Duration::from_millis(1));
    }
    let (client, handle) = connecting.join().unwrap();
    let (events_tx, events) = mpsc::channel();
    let running = thread::spawn(move || client.run(move |event| events_tx.send(event).unwrap()));
    host.until("start", |h| h.has(0x0307));
    (running, handle, events)
}

/// Waits, servicing the host, for the client's roaming events up to one matching `last`.
fn roam_events(
    host: &mut TestHost<Roamable>,
    events: &mpsc::Receiver<ControlEvent>,
    last: impl Fn(&RoamEvent) -> bool,
) -> Vec<RoamEvent> {
    let mut seen = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(Instant::now() < deadline, "roaming events: {seen:?}");
        host.pump();
        if let Ok(ControlEvent::Roam(event)) = events.recv_timeout(Duration::from_millis(5)) {
            let done = last(&event);
            seen.push(event);
            if done {
                return seen;
            }
        }
    }
}

#[test]
fn a_network_change_roams_through_the_control_port() {
    let mut host = TestHost::roamable();
    let caps = HostCapabilities {
        client_roaming: true,
        ..caps()
    };
    let (running, handle, events) = run_roamable(&mut host, caps);
    let gcm = roam_gcm();

    assert!(handle.network_changed(true));
    host.until("probe", |h| !h.host.socket().roaming.is_empty());
    let (from, probe) = host.roam_payloads(&gcm)[0];
    let Payload::Probe { nonce_c, flags } = probe else {
        panic!("{probe:?}");
    };
    assert_eq!(flags, 1, "the media rebound flag rides along");

    host.roam_reply(
        from,
        &gcm,
        1,
        Payload::Challenge {
            nonce_c,
            challenge: 77,
        },
    );
    host.until("confirm", |h| {
        h.roam_payloads(&gcm)
            .iter()
            .any(|(_, p)| p.kind() == Kind::Confirm)
    });
    let confirm = host
        .roam_payloads(&gcm)
        .into_iter()
        .find(|(_, p)| p.kind() == Kind::Confirm)
        .unwrap();
    assert_eq!(confirm, (from, Payload::Confirm { challenge: 77 }));
    // A host message first: ENet's own ping is then not due for half a second.
    let mut seen = Vec::new();
    host.send(0x010b, &[0, 0, 0, 0, 2, 0, 0x00, 0x80, 0xff, 0xff]);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(Instant::now() < deadline, "no rumble: {seen:?}");
        host.pump();
        match events.recv_timeout(Duration::from_millis(5)) {
            Ok(ControlEvent::Roam(event)) => seen.push(event),
            Ok(ControlEvent::Rumble { .. }) => break,
            _ => {}
        }
    }
    host.host.socket_mut().pings.clear();
    let migrated_at = Instant::now();
    host.roam_reply(from, &gcm, 2, Payload::Migrated { challenge: 77 });

    seen.extend(roam_events(&mut host, &events, |e| {
        matches!(e, RoamEvent::Roamed { .. })
    }));
    assert_eq!(
        seen[0],
        RoamEvent::Started {
            trigger: RoamTrigger::NetworkChanged
        }
    );
    assert_eq!(seen.len(), 2, "{seen:?}");
    // The client pings ENet at once so its backed-off resend timers recover.
    host.until("an ENet ping", |h| !h.host.socket().pings.is_empty());
    let ping = host.host.socket().pings[0] - migrated_at;
    assert!(
        ping < Duration::from_millis(250),
        "ENet pinged {ping:?} after MIGRATED"
    );

    // ENet runs on over the same port.
    handle.stop();
    host.until("disconnect", |h| h.disconnected);
    assert_eq!(running.join().unwrap(), Outcome::Stopped);
}

#[test]
fn a_silent_host_is_probed_until_heard_again() {
    let mut host = TestHost::roamable();
    let caps = HostCapabilities {
        client_roaming: true,
        ..caps()
    };
    let (running, handle, events) = run_roamable(&mut host, caps);
    let gcm = roam_gcm();

    // The host stops servicing: the client hears nothing past INPUT_CONTACT_LOST_AFTER.
    thread::sleep(Duration::from_millis(1600));
    let seen = roam_events(&mut host, &events, |e| {
        matches!(e, RoamEvent::Unneeded { .. })
    });
    assert_eq!(
        seen[0],
        RoamEvent::Started {
            trigger: RoamTrigger::HostSilent
        }
    );
    assert_eq!(seen.len(), 2, "{seen:?}");
    let (_, probe) = host.roam_payloads(&gcm)[0];
    assert!(
        matches!(probe, Payload::Probe { flags: 0, .. }),
        "{probe:?}"
    );

    handle.stop();
    host.until("disconnect", |h| h.disconnected);
    assert_eq!(running.join().unwrap(), Outcome::Stopped);
}

/// Waits for the client's next roaming event while the host stays silent.
fn next_roam_event_in_silence(events: &mpsc::Receiver<ControlEvent>) -> RoamEvent {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(left) {
            Ok(ControlEvent::Roam(event)) => return event,
            Ok(_) => {}
            Err(e) => panic!("no roaming event: {e}"),
        }
    }
}

/// The newest PROBE the host received: its sender and `nonce_c`.
fn newest_probe(host: &TestHost<Roamable>, gcm: &aes_gcm::Aes128Gcm) -> (SocketAddr, u64) {
    host.roam_payloads(gcm)
        .into_iter()
        .rev()
        .find_map(|(from, payload)| match payload {
            Payload::Probe { nonce_c, .. } => Some((from, nonce_c)),
            _ => None,
        })
        .expect("a PROBE")
}

#[test]
fn a_network_change_during_a_silence_is_not_cancelled_by_a_straggler() {
    let mut host = TestHost::roamable();
    let caps = HostCapabilities {
        client_roaming: true,
        ..caps()
    };
    let (running, handle, events) = run_roamable(&mut host, caps);
    let gcm = roam_gcm();

    // The host stops servicing: a silence starts an attempt.
    assert_eq!(
        next_roam_event_in_silence(&events),
        RoamEvent::Started {
            trigger: RoamTrigger::HostSilent
        }
    );
    // Then the network changes, still in silence.
    assert!(handle.network_changed(false));
    assert_eq!(
        next_roam_event_in_silence(&events),
        RoamEvent::Started {
            trigger: RoamTrigger::NetworkChanged
        }
    );
    // A straggler on the old path (the host's ENet acknowledgements) does not end it.
    let until = Instant::now() + Duration::from_millis(600);
    while Instant::now() < until {
        host.pump();
        thread::sleep(Duration::from_millis(1));
    }
    let early: Vec<ControlEvent> = events.try_iter().collect();
    assert!(
        early.iter().all(|e| !matches!(e, ControlEvent::Roam(_))),
        "{early:?}"
    );

    let (from, nonce_c) = newest_probe(&host, &gcm);
    host.roam_reply(
        from,
        &gcm,
        1,
        Payload::Challenge {
            nonce_c,
            challenge: 31,
        },
    );
    host.until("confirm", |h| {
        h.roam_payloads(&gcm)
            .contains(&(from, Payload::Confirm { challenge: 31 }))
    });
    host.roam_reply(from, &gcm, 2, Payload::Migrated { challenge: 31 });
    let seen = roam_events(&mut host, &events, |e| {
        !matches!(e, RoamEvent::Started { .. })
    });
    assert!(matches!(seen[..], [RoamEvent::Roamed { .. }]), "{seen:?}");

    handle.stop();
    host.until("disconnect", |h| h.disconnected);
    assert_eq!(running.join().unwrap(), Outcome::Stopped);
}

/// Roams on a network change, answering like the Rubylight host only a PROBE whose seq is
/// above `seen`, the last it accepted under the key, with replies at `host_seq` and the next.
fn roam_above(
    host: &mut TestHost<Roamable>,
    handle: &nightjar_core::control::driver::ControlHandle,
    events: &mpsc::Receiver<ControlEvent>,
    seen: u32,
    host_seq: u32,
) {
    let gcm = roam_gcm();
    assert!(handle.network_changed(true));
    host.until("a PROBE above the seqs seen", |h| {
        h.roam_seqs().iter().any(|&seq| seq > seen)
    });
    let (from, nonce_c) = host
        .roam_seqs()
        .into_iter()
        .zip(host.roam_payloads(&gcm))
        .find_map(|(seq, (from, payload))| match payload {
            Payload::Probe { nonce_c, .. } if seq > seen => Some((from, nonce_c)),
            _ => None,
        })
        .unwrap();
    host.roam_reply(
        from,
        &gcm,
        host_seq,
        Payload::Challenge {
            nonce_c,
            challenge: 55,
        },
    );
    host.until("confirm", |h| {
        h.roam_payloads(&gcm)
            .contains(&(from, Payload::Confirm { challenge: 55 }))
    });
    host.roam_reply(
        from,
        &gcm,
        host_seq + 1,
        Payload::Migrated { challenge: 55 },
    );
    let seen = roam_events(host, events, |e| !matches!(e, RoamEvent::Started { .. }));
    assert!(
        matches!(
            seen[..],
            [RoamEvent::Started { .. }, RoamEvent::Roamed { .. }]
        ),
        "{seen:?}"
    );
}

/// Strictly increasing.
fn increasing(seqs: &[u32]) -> bool {
    seqs.windows(2).all(|pair| pair[0] < pair[1])
}

#[test]
fn a_reconnect_under_the_same_key_roams_with_higher_seqs() {
    let caps = HostCapabilities {
        client_roaming: true,
        ..caps()
    };
    let roam_seq = LaunchSeq::default();
    let mut first = TestHost::roamable();
    let (running, handle, events) = run_roamable_after(&mut first, caps.clone(), roam_seq.clone());
    roam_above(&mut first, &handle, &events, 0, 1);
    handle.stop();
    first.until("disconnect", |h| h.disconnected);
    assert_eq!(running.join().unwrap(), Outcome::Stopped);
    let before = first.roam_seqs();
    assert!(increasing(&before), "{before:?}");
    let last = *before.last().unwrap();

    // The same launch again: the host still holds the seqs it saw under the key.
    let mut second = TestHost::roamable();
    let (running, handle, events) = run_roamable_after(&mut second, caps, roam_seq);
    roam_above(&mut second, &handle, &events, last, 3);
    handle.stop();
    second.until("disconnect", |h| h.disconnected);
    assert_eq!(running.join().unwrap(), Outcome::Stopped);
    let after = second.roam_seqs();
    assert!(increasing(&after), "{after:?}");
    assert!(after[0] > last, "{before:?} then {after:?}");
}

#[test]
fn without_roaming_magic_datagrams_leave_enet_alone() {
    let mut host = TestHost::roamable();
    let (running, handle, events) = run_roamable(&mut host, caps());
    let client = host
        .host
        .peer(host.peer.unwrap())
        .address()
        .expect("connected");
    // A sealed CHALLENGE and junk behind the magic reach the client's ENet socket.
    host.roam_reply(
        client,
        &roam_gcm(),
        1,
        Payload::Challenge {
            nonce_c: 1,
            challenge: 2,
        },
    );
    let junk = [0x0F, 0xFE, 0x52, 0x4D, 1, 2, 3];
    host.host.socket().socket.send_to(&junk, client).unwrap();

    // ENet carries on both ways.
    host.send(0x010b, &[0, 0, 0, 0, 2, 0, 0x00, 0x80, 0xff, 0xff]);
    handle
        .send(ClientMessage::Reconfigure(Reconfigure {
            width: 1280,
            height: 720,
            fps_millihz: 60_000,
        }))
        .unwrap();
    host.until("reconfigure", |h| h.has(0x5532));
    let mut seen = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !seen
        .iter()
        .any(|e| matches!(e, ControlEvent::Rumble { .. }))
    {
        assert!(Instant::now() < deadline, "no rumble: {seen:?}");
        host.pump();
        seen.extend(events.recv_timeout(Duration::from_millis(5)));
    }
    seen.extend(events.try_iter());
    assert!(
        !seen.iter().any(|e| matches!(e, ControlEvent::Roam(_))),
        "{seen:?}"
    );
    assert!(host.host.socket().roaming.is_empty());

    handle.stop();
    host.until("disconnect", |h| h.disconnected);
    assert_eq!(running.join().unwrap(), Outcome::Stopped);
}

#[test]
fn without_the_advertisement_nothing_is_probed() {
    let mut host = TestHost::roamable();
    let (running, handle, events) = run_roamable(&mut host, caps());

    assert!(!handle.network_changed(true));
    // A silence past INPUT_CONTACT_LOST_AFTER, then time for fast probes to arrive.
    thread::sleep(Duration::from_millis(1600));
    let until = Instant::now() + Duration::from_millis(600);
    while Instant::now() < until {
        host.pump();
        thread::sleep(Duration::from_millis(1));
    }
    assert!(host.host.socket().roaming.is_empty());
    assert!(!events
        .try_iter()
        .any(|e| matches!(e, ControlEvent::Roam(_))));

    handle.stop();
    host.until("disconnect", |h| h.disconnected);
    assert_eq!(running.join().unwrap(), Outcome::Stopped);
}
