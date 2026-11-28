//! The relay and a requester on real sockets, all on 127.0.0.1 (no wildcard bind, no DHT, no
//! STUN): the requester knows the relay's session and address, as a DHT record would tell it.

use super::*;
use crate::wake_relay::{WakeRequest, device_id, device_secret};
use std::net::{IpAddr, Ipv4Addr};

const TARGET: &str = "GAMING-PC";
const MAC: [u8; 6] = [0x10, 0x7b, 0x44, 0x0a, 0x0b, 0x0c];

#[derive(Default)]
struct FakeLan {
    bursts: Mutex<Vec<Burst>>,
    up: AtomicBool,
    probes: std::sync::atomic::AtomicUsize,
}

impl Lan for FakeLan {
    fn interfaces(&self) -> Vec<Interface> {
        vec![Interface {
            ip: Ipv4Addr::new(192, 168, 1, 10),
            prefix: 24,
        }]
    }
    fn send(&self, burst: &Burst) -> usize {
        self.bursts.lock().unwrap().push(burst.clone());
        burst.packets.len()
    }
    fn is_up(&self, _: SocketAddrV4) -> bool {
        self.probes.fetch_add(1, Ordering::SeqCst);
        self.up.load(Ordering::Acquire)
    }
}

fn loopback() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)
}

fn device() -> DeviceId {
    device_id("phone certificate")
}

fn enrollment() -> Enrollment {
    Enrollment {
        target_id: TARGET.into(),
        target_name: "Gaming PC".into(),
        epoch: 2,
        secret: [21; 32],
        macs: vec![MAC],
        broadcasts: vec![Ipv4Addr::new(192, 168, 1, 255)],
        address: Some(Ipv4Addr::new(192, 168, 1, 30)),
        http_port: 47989,
        devices: vec![device()],
    }
}

fn relay(lan: Arc<FakeLan>) -> WakeRelayPeer {
    let config = RelayConfig {
        bind: loopback(),
        stun_servers: Vec::new(),
        bootstrap: Vec::new(),
    };
    let peer = WakeRelayPeer::start(config, lan).unwrap();
    peer.set_enrollments(vec![enrollment()]);
    peer
}

fn request(peer: &WakeRelayPeer, epoch: u64) -> RequestConfig {
    RequestConfig {
        target_id: TARGET.into(),
        device: device(),
        epoch,
        secret: device_secret(&[21; 32], TARGET, &device(), epoch),
        known: Some(RelayInfo {
            session: peer.session(),
            endpoints: vec![peer.local_addr().unwrap()],
            nat: Nat::None,
        }),
        bind: loopback(),
        stun_servers: Vec::new(),
        bootstrap: Vec::new(),
    }
}

#[test]
fn a_paired_device_wakes_the_pc_through_the_relay_and_hears_it_came_up() {
    let lan = Arc::new(FakeLan::default());
    let peer = relay(lan.clone());
    lan.up.store(true, Ordering::Release);
    let mut seen = Vec::new();
    let result = request_wake(request(&peer, 2), &AtomicBool::new(false), &mut |p| {
        seen.push(p)
    });
    peer.stop();
    assert_eq!(result.map(|status| status.status), Ok(Status::HostAwake));
    assert!(seen.contains(&Progress::Requested));
    assert!(
        seen.iter()
            .any(|p| matches!(p, Progress::Status(s) if s.status == Status::PacketSent))
    );
    // One request, one burst.
    let bursts = lan.bursts.lock().unwrap();
    assert_eq!(bursts.len(), 1);
    assert!(bursts[0].packets.len() <= crate::wake_relay::MAX_BURST_PACKETS);
    assert_eq!(
        peer.status().last_wake.map(|w| w.2),
        Some(Status::HostAwake)
    );
}

#[test]
fn a_second_wake_right_after_is_rate_limited_without_a_burst() {
    let lan = Arc::new(FakeLan::default());
    let peer = relay(lan.clone());
    lan.up.store(true, Ordering::Release);
    let cancel = AtomicBool::new(false);
    let first = request_wake(request(&peer, 2), &cancel, &mut |_| {});
    let second = request_wake(request(&peer, 2), &cancel, &mut |_| {});
    peer.stop();
    assert_eq!(first.map(|s| s.status), Ok(Status::HostAwake));
    let second = second.unwrap();
    assert_eq!(second.status, Status::RateLimited);
    assert!(second.retry_after > 0);
    assert_eq!(lan.bursts.lock().unwrap().len(), 1);
}

/// Whatever arrives without a listed device's keys gets not one byte back, and sends nothing on
/// the LAN: no amplification, no reflection, nothing for a scanner to see.
#[test]
fn unauthenticated_datagrams_get_no_answer_at_all() {
    let lan = Arc::new(FakeLan::default());
    let peer = relay(lan.clone());
    let relay_address = peer.local_addr().unwrap();
    let probe = UdpSocket::bind(loopback()).unwrap();
    probe
        .set_read_timeout(Some(Duration::from_millis(600)))
        .unwrap();
    let mut datagrams: Vec<Vec<u8>> = vec![
        b"hello".to_vec(),
        vec![0; 200],
        // A DHT query and a STUN request: other protocols on the same socket.
        b"d1:ad2:id20:aaaaaaaaaaaaaaaaaaaae1:q4:ping1:t2:aa1:y1:qe".to_vec(),
        [0x00, 0x01, 0x00, 0x00, 0x21, 0x12, 0xa4, 0x42].repeat(3),
    ];
    // Well-formed punches and requests under keys the relay does not hold: an unknown device,
    // a device of an older epoch (revoked), and the right tag with the wrong secret.
    let stamp = wire::punch_stamp(unix_now());
    let session = [9; 16];
    let stranger = Keys::new([1; 32]).path(&peer.session(), &session);
    let revoked =
        Keys::new(device_secret(&[21; 32], TARGET, &device(), 1)).path(&peer.session(), &session);
    let listed = Keys::new(enrollment().device_secret(&device())).path(&peer.session(), &session);
    for keys in [stranger, revoked] {
        let sealer = Sealer::new(&keys.client_to_host, keys.tag, session);
        datagrams.push(sealer.seal(Kind::Punch, 0, &stamp));
        datagrams.push(sealer.seal(Kind::WakeRequest, 0, &[0; 148]));
    }
    let wrong_key = Sealer::new(&stranger.client_to_host, listed.tag, session);
    datagrams.push(wrong_key.seal(Kind::Punch, 0, &stamp));
    // The listed device's own punch, but stale: a capture replayed later.
    let stale = Sealer::new(&listed.client_to_host, listed.tag, session);
    datagrams.push(stale.seal(Kind::Punch, 0, &wire::punch_stamp(unix_now() - 600)));
    for datagram in &datagrams {
        probe.send_to(datagram, relay_address).unwrap();
    }
    let mut buffer = [0u8; 2048];
    let answer = probe.recv_from(&mut buffer);
    peer.stop();
    assert!(
        answer.is_err(),
        "the relay answered {:?}",
        answer.map(|a| a.0)
    );
    assert!(lan.bursts.lock().unwrap().is_empty());
}

#[test]
fn a_device_the_pc_dropped_is_refused_at_once_on_its_open_path() {
    let lan = Arc::new(FakeLan::default());
    let peer = relay(lan.clone());
    let relay_address = peer.local_addr().unwrap();
    let probe = UdpSocket::bind(loopback()).unwrap();
    probe
        .set_read_timeout(Some(Duration::from_millis(600)))
        .unwrap();
    let session = [7; 16];
    let keys = Keys::new(enrollment().device_secret(&device())).path(&peer.session(), &session);
    let sealer = Sealer::new(&keys.client_to_host, keys.tag, session);
    probe
        .send_to(
            &sealer.seal(Kind::Punch, 0, &wire::punch_stamp(unix_now())),
            relay_address,
        )
        .unwrap();
    let mut buffer = [0u8; 2048];
    let (length, _) = probe.recv_from(&mut buffer).unwrap();
    let mut opener = Opener::new(&keys.host_to_client, keys.tag);
    assert_eq!(opener.open(&buffer[..length]).unwrap().kind, Kind::PunchAck);
    // The PC unpairs the device: its path closes, and its request gets nothing.
    let mut dropped = enrollment();
    dropped.devices.clear();
    peer.set_enrollments(vec![dropped]);
    let secret = enrollment().device_secret(&device());
    let request = WakeRequest::new(TARGET, device(), 2, unix_now()).encode(&secret);
    probe
        .send_to(&sealer.seal(Kind::WakeRequest, 0, &request), relay_address)
        .unwrap();
    let answer = probe.recv_from(&mut buffer);
    peer.stop();
    assert!(answer.is_err());
    assert!(lan.bursts.lock().unwrap().is_empty());
}

#[test]
fn a_relay_on_several_networks_sends_each_datagram_from_one_address_only() {
    let interfaces = [
        Interface {
            ip: Ipv4Addr::new(192, 168, 1, 10),
            prefix: 24,
        },
        Interface {
            ip: Ipv4Addr::new(10, 0, 0, 5),
            prefix: 24,
        },
        // A second address on the first network: still one datagram, not two.
        Interface {
            ip: Ipv4Addr::new(192, 168, 1, 11),
            prefix: 24,
        },
    ];
    let to = |ip: [u8; 4]| SocketAddrV4::new(ip.into(), crate::wake_relay::WOL_PORT);
    let destinations = [
        to([192, 168, 1, 255]),
        to([10, 0, 0, 255]),
        to([172, 16, 0, 255]),
        to([255, 255, 255, 255]),
    ];
    let burst = crate::wake_relay::burst(&[MAC], &destinations);
    let from = sources(&burst, &interfaces);
    assert_eq!(from.len(), burst.packets.len());
    assert!(from.len() <= crate::wake_relay::MAX_BURST_PACKETS);
    for ((destination, _), source) in burst.packets.iter().zip(&from) {
        let expected = match destination.ip().octets() {
            [192, 168, 1, 255] | [255, 255, 255, 255] => Some(Ipv4Addr::new(192, 168, 1, 10)),
            [10, 0, 0, 255] => Some(Ipv4Addr::new(10, 0, 0, 5)),
            _ => None,
        };
        assert_eq!(*source, expected, "{destination}");
    }
}

/// A path's key depends only on the relay's session and the device's, so a path built again for
/// the same device session must not count from where the last one started.
#[test]
fn a_path_built_again_for_a_device_session_never_repeats_a_sequence_number() {
    let lan = Arc::new(FakeLan::default());
    let peer = relay(lan.clone());
    let relay_address = peer.local_addr().unwrap();
    let probe = UdpSocket::bind(loopback()).unwrap();
    probe
        .set_read_timeout(Some(Duration::from_millis(600)))
        .unwrap();
    let session = [5; 16];
    let keys = Keys::new(enrollment().device_secret(&device())).path(&peer.session(), &session);
    let sealer = Sealer::new(&keys.client_to_host, keys.tag, session);
    let mut buffer = [0u8; 2048];
    let mut answers = Vec::new();
    for _ in 0..3 {
        probe
            .send_to(
                &sealer.seal(Kind::Punch, 0, &wire::punch_stamp(unix_now())),
                relay_address,
            )
            .unwrap();
        // Everything the relay sends on the path (its own punches too) is under the same key;
        // a fresh opener per datagram, as a listener that kept none would.
        loop {
            let (length, _) = probe.recv_from(&mut buffer).unwrap();
            let mut opener = Opener::new(&keys.host_to_client, keys.tag);
            let kind = opener.open(&buffer[..length]).unwrap().kind;
            answers.push(u64::from_be_bytes(buffer[16..24].try_into().unwrap()));
            if kind == Kind::PunchAck {
                break;
            }
        }
        // The path goes, as when its puncher fails or the device closes it; once the ended-session
        // hold elapses, a path built again for the same session still takes a fresh sequence range.
        probe
            .send_to(&sealer.seal(Kind::Close, 0, &[]), relay_address)
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while peer
            .shared
            .paths
            .lock()
            .unwrap()
            .active
            .contains_key(&keys.tag)
            && std::time::Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(5));
        }
        peer.shared.paths.lock().unwrap().ended.clear();
    }
    peer.stop();
    let mut distinct = answers.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        answers.len(),
        "sequence numbers {answers:?}"
    );
}

fn wait_until(mut cond: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !cond() {
        assert!(std::time::Instant::now() < deadline, "timed out waiting");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn ended_or_superseded_session_cannot_reopen_or_clobber_newer_session() {
    let lan = Arc::new(FakeLan::default());
    let peer = relay(lan);
    let relay_address = peer.local_addr().unwrap();
    let probe = UdpSocket::bind(loopback()).unwrap();
    probe
        .set_read_timeout(Some(Duration::from_millis(250)))
        .unwrap();
    let base_keys = Keys::new(enrollment().device_secret(&device()));
    let mut buffer = [0u8; 2048];
    let now = unix_now();

    // Open session 1, close it, and verify a replayed punch cannot reopen it within 2 * CLIENT_FRESH_SECONDS.
    let session1 = [11; 16];
    let k1 = base_keys.path(&peer.session(), &session1);
    let s1 = Sealer::new(&k1.client_to_host, k1.tag, session1);
    probe
        .send_to(
            &s1.seal(Kind::Punch, 0, &wire::punch_stamp(now)),
            relay_address,
        )
        .unwrap();
    let (len, _) = probe.recv_from(&mut buffer).unwrap();
    assert_eq!(
        Opener::new(&k1.host_to_client, k1.tag)
            .open(&buffer[..len])
            .unwrap()
            .kind,
        Kind::PunchAck
    );
    probe
        .send_to(&s1.seal(Kind::Close, 0, &[]), relay_address)
        .unwrap();
    wait_until(|| {
        !peer
            .shared
            .paths
            .lock()
            .unwrap()
            .active
            .contains_key(&k1.tag)
    });
    probe
        .send_to(
            &s1.seal(Kind::Punch, 0, &wire::punch_stamp(now)),
            relay_address,
        )
        .unwrap();
    assert!(
        probe.recv_from(&mut buffer).is_err(),
        "closed session stayed rejected"
    );

    // Open newer session 2; an unseen older session 3 must not clobber it.
    let session2 = [12; 16];
    let k2 = base_keys.path(&peer.session(), &session2);
    let s2 = Sealer::new(&k2.client_to_host, k2.tag, session2);
    let t2 = now + 5;
    probe
        .send_to(
            &s2.seal(Kind::Punch, 0, &wire::punch_stamp(t2)),
            relay_address,
        )
        .unwrap();
    let (len, _) = probe.recv_from(&mut buffer).unwrap();
    assert_eq!(
        Opener::new(&k2.host_to_client, k2.tag)
            .open(&buffer[..len])
            .unwrap()
            .kind,
        Kind::PunchAck
    );

    let session3 = [13; 16];
    let k3 = base_keys.path(&peer.session(), &session3);
    let s3 = Sealer::new(&k3.client_to_host, k3.tag, session3);
    probe
        .send_to(
            &s3.seal(Kind::Punch, 0, &wire::punch_stamp(t2 - 2)),
            relay_address,
        )
        .unwrap();
    assert!(probe.recv_from(&mut buffer).is_err(), "older punch ignored");
    assert_eq!(
        peer.shared.paths.lock().unwrap().active[&k2.tag].session,
        session2
    );

    // Session 4 supersedes session 2; replaying session 2 must not clobber session 4.
    let session4 = [14; 16];
    let k4 = base_keys.path(&peer.session(), &session4);
    let s4 = Sealer::new(&k4.client_to_host, k4.tag, session4);
    let t4 = t2 + 1;
    probe
        .send_to(
            &s4.seal(Kind::Punch, 0, &wire::punch_stamp(t4)),
            relay_address,
        )
        .unwrap();
    let (len, _) = probe.recv_from(&mut buffer).unwrap();
    assert_eq!(
        Opener::new(&k4.host_to_client, k4.tag)
            .open(&buffer[..len])
            .unwrap()
            .kind,
        Kind::PunchAck
    );
    probe
        .send_to(
            &s2.seal(Kind::Punch, 0, &wire::punch_stamp(t4 + 1)),
            relay_address,
        )
        .unwrap();
    assert!(
        probe.recv_from(&mut buffer).is_err(),
        "superseded session rejected"
    );
    assert_eq!(
        peer.shared.paths.lock().unwrap().active[&k4.tag].session,
        session4
    );

    // poll_device must not clobber a connected path, reopen an ended session, or clobber a newer punching path.
    let make_record = |session: [u8; 16], time: u64| Record {
        role: Role::WakeRequester,
        time,
        session,
        peer_session: peer.session(),
        nat: Nat::None,
        endpoints: vec![probe.local_addr().unwrap()],
        ports: None,
    };
    peer.apply_polled_record(k4.tag, &make_record([15; 16], t4 + 2));
    assert_eq!(
        peer.shared.paths.lock().unwrap().active[&k4.tag].session,
        session4,
        "connected path not clobbered by DHT poll"
    );

    probe
        .send_to(&s4.seal(Kind::Close, 0, &[]), relay_address)
        .unwrap();
    wait_until(|| {
        !peer
            .shared
            .paths
            .lock()
            .unwrap()
            .active
            .contains_key(&k4.tag)
    });
    peer.apply_polled_record(k4.tag, &make_record(session4, t4 + 2));
    assert!(
        !peer
            .shared
            .paths
            .lock()
            .unwrap()
            .active
            .contains_key(&k4.tag),
        "ended session not reopened by DHT poll"
    );

    let session6 = [16; 16];
    peer.apply_polled_record(k4.tag, &make_record(session6, t4 + 5));
    assert_eq!(
        peer.shared.paths.lock().unwrap().active[&k4.tag].session,
        session6
    );
    peer.apply_polled_record(k4.tag, &make_record([17; 16], t4 + 4));
    assert_eq!(
        peer.shared.paths.lock().unwrap().active[&k4.tag].session,
        session6,
        "older DHT record did not clobber newer punching path"
    );
    peer.stop();
}

#[test]
fn request_wake_returns_cancelled_before_and_during_lookup() {
    let config = RequestConfig {
        target_id: TARGET.into(),
        device: device(),
        epoch: 2,
        secret: device_secret(&[21; 32], TARGET, &device(), 2),
        known: None,
        bind: loopback(),
        stun_servers: Vec::new(),
        bootstrap: vec!["127.0.0.1:9".into()],
    };
    // Pre-cancelled returns Cancelled immediately rather than RelayOffline.
    let started = std::time::Instant::now();
    let result = request_wake(config.clone(), &AtomicBool::new(true), &mut |_| {});
    assert_eq!(result, Err(Failure::Cancelled));
    assert!(started.elapsed() < Duration::from_millis(500));

    // Cancelling while look_up is waiting on an unresponsive bootstrap returns Cancelled promptly.
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_thread = {
        let cancel = cancel.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(60));
            cancel.store(true, Ordering::Release);
        })
    };
    let started = std::time::Instant::now();
    let result = request_wake(config, &cancel, &mut |_| {});
    cancel_thread.join().unwrap();
    assert_eq!(result, Err(Failure::Cancelled));
    assert!(
        started.elapsed() < Duration::from_millis(1_500),
        "cancel during lookup took {:?}",
        started.elapsed()
    );
}

#[test]
fn watch_skips_missing_target_and_stops_when_session_leaves_paths() {
    // Enrollment with no target address: watch returns early without probing.
    let lan = Arc::new(FakeLan::default());
    let peer = WakeRelayPeer::start(
        RelayConfig {
            bind: loopback(),
            stun_servers: Vec::new(),
            bootstrap: Vec::new(),
        },
        lan.clone(),
    )
    .unwrap();
    let mut no_addr = enrollment();
    no_addr.address = None;
    peer.set_enrollments(vec![no_addr]);
    let relay_address = peer.local_addr().unwrap();
    let probe = UdpSocket::bind(loopback()).unwrap();
    probe
        .set_read_timeout(Some(Duration::from_millis(600)))
        .unwrap();
    let session = [21; 16];
    let keys = Keys::new(enrollment().device_secret(&device())).path(&peer.session(), &session);
    let sealer = Sealer::new(&keys.client_to_host, keys.tag, session);
    let mut opener = Opener::new(&keys.host_to_client, keys.tag);
    let mut buffer = [0u8; 2048];
    probe
        .send_to(
            &sealer.seal(Kind::Punch, 0, &wire::punch_stamp(unix_now())),
            relay_address,
        )
        .unwrap();
    let (len, _) = probe.recv_from(&mut buffer).unwrap();
    assert_eq!(opener.open(&buffer[..len]).unwrap().kind, Kind::PunchAck);
    let secret = enrollment().device_secret(&device());
    let req = WakeRequest::new(TARGET, device(), 2, unix_now()).encode(&secret);
    probe
        .send_to(&sealer.seal(Kind::WakeRequest, 0, &req), relay_address)
        .unwrap();
    let (len, _) = probe.recv_from(&mut buffer).unwrap();
    let opened = opener.open(&buffer[..len]).unwrap();
    assert_eq!(opened.kind, Kind::WakeStatus);
    assert_eq!(
        WakeStatus::decode(&opened.payload).unwrap().status,
        Status::PacketSent
    );
    thread::sleep(PROBE_EVERY * 3);
    assert_eq!(lan.probes.load(Ordering::SeqCst), 0);
    peer.stop();

    // Enrollment with target address: closing the session stops watch probing promptly.
    let lan = Arc::new(FakeLan::default());
    let peer = relay(lan.clone());
    let relay_address = peer.local_addr().unwrap();
    let session = [22; 16];
    let keys = Keys::new(enrollment().device_secret(&device())).path(&peer.session(), &session);
    let sealer = Sealer::new(&keys.client_to_host, keys.tag, session);
    let mut opener = Opener::new(&keys.host_to_client, keys.tag);
    probe
        .send_to(
            &sealer.seal(Kind::Punch, 0, &wire::punch_stamp(unix_now())),
            relay_address,
        )
        .unwrap();
    let (len, _) = probe.recv_from(&mut buffer).unwrap();
    assert_eq!(opener.open(&buffer[..len]).unwrap().kind, Kind::PunchAck);
    let req = WakeRequest::new(TARGET, device(), 2, unix_now()).encode(&secret);
    probe
        .send_to(&sealer.seal(Kind::WakeRequest, 0, &req), relay_address)
        .unwrap();
    let (len, _) = probe.recv_from(&mut buffer).unwrap();
    assert_eq!(
        WakeStatus::decode(&opener.open(&buffer[..len]).unwrap().payload)
            .unwrap()
            .status,
        Status::PacketSent
    );
    wait_until(|| lan.probes.load(Ordering::SeqCst) >= 1);
    probe
        .send_to(&sealer.seal(Kind::Close, 0, &[]), relay_address)
        .unwrap();
    wait_until(|| {
        !peer
            .shared
            .paths
            .lock()
            .unwrap()
            .active
            .contains_key(&keys.tag)
    });
    thread::sleep(PROBE_EVERY * 2);
    let after_close = lan.probes.load(Ordering::SeqCst);
    thread::sleep(PROBE_EVERY * 3);
    assert_eq!(
        lan.probes.load(Ordering::SeqCst),
        after_close,
        "watch thread kept probing after session left paths"
    );
    peer.stop();
}

/// While waiting for the target PC to boot, the relay sends periodic `WaitingForBoot` updates
/// and the requester sends periodic keepalives that refresh the relay path before `HostAwake`.
#[test]
fn boot_wait_sends_waiting_for_boot_and_keepalives_until_host_wakes() {
    let lan = Arc::new(FakeLan::default());
    let peer = relay(lan.clone());
    let lan_up = lan.clone();
    let boot_trigger = thread::spawn(move || {
        thread::sleep(WAIT_KEEPALIVE * 3);
        lan_up.up.store(true, Ordering::Release);
    });
    let mut seen = Vec::new();
    let result = request_wake(request(&peer, 2), &AtomicBool::new(false), &mut |p| {
        seen.push(p)
    });
    boot_trigger.join().unwrap();
    peer.stop();
    assert_eq!(result.map(|s| s.status), Ok(Status::HostAwake));
    assert!(
        seen.iter()
            .any(|p| matches!(p, Progress::Status(s) if s.status == Status::PacketSent)),
        "missing PacketSent: {seen:?}"
    );
    assert!(
        seen.iter()
            .any(|p| matches!(p, Progress::Status(s) if s.status == Status::WaitingForBoot)),
        "expected periodic WaitingForBoot keepalive during boot wait: {seen:?}"
    );
}
