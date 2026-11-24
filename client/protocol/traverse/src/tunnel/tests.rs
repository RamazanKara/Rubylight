//! Loopback tunnel transport, idle wake, and address allocation tests.

use super::*;
use crate::record::Keys;

/// A device and a host tunnel on loopback, punching each other through real sockets: an HTTP
/// exchange and a media datagram go through both ways.
#[test]
fn a_device_reaches_the_hosts_tcp_and_media_ports_through_the_tunnel() {
    // The "host's" services: an echo server on TCP and a UDP echo for video.
    let service = TcpListener::bind("127.0.0.1:0").unwrap();
    let video = UdpSocket::bind("127.0.0.1:0").unwrap();
    let ports = Ports {
        http: service.local_addr().unwrap().port(),
        video: video.local_addr().unwrap().port(),
        ..Ports::default()
    };
    // The services answer only a peer registered as tunnelled: the tunnel connects from
    // 127.0.0.1 here, as a host bound to one address connects from that address, and must
    // still never look like a program on this PC.
    thread::spawn(move || {
        for connection in service.incoming() {
            let mut connection = connection.unwrap();
            let tunnelled = crate::peers::is_tunnelled(connection.peer_addr().unwrap());
            let mut request = [0u8; 4];
            connection.read_exact(&mut request).unwrap();
            connection
                .write_all(if tunnelled { b"pong" } else { b"locl" })
                .unwrap();
        }
    });
    thread::spawn(move || {
        let mut buffer = [0u8; 64];
        let (length, from) = video.recv_from(&mut buffer).unwrap();
        if crate::peers::is_tunnelled(from) {
            video.send_to(&buffer[..length], from).unwrap();
        } else {
            video.send_to(b"local", from).unwrap();
        }
    });

    let keys = Keys::new([5; 32]).path(&[1; 16], &[2; 16]);
    let host_node = Node::bind("127.0.0.1:0".parse().unwrap(), Vec::new()).unwrap();
    let device_node = Node::bind("127.0.0.1:0".parse().unwrap(), Vec::new()).unwrap();
    let host = Tunnel::start(
        host_node.clone(),
        Role::Host,
        keys,
        [2; 16],
        vec![device_node.local_addr().unwrap()],
        Bridge::Host {
            service: "127.0.0.1".parse().unwrap(),
            source: "127.0.0.1".parse().unwrap(),
            ports,
            device: "test device".into(),
        },
    )
    .unwrap();
    // The device binds the host's ports on its own loopback address; the test's services
    // hold them on 127.0.0.1, so it takes another.
    let local = bindable_loopback("127.0.0.2".parse().unwrap());
    if local == IpAddr::from([127, 0, 0, 1]) {
        return; // A system without 127/8 (macOS): nothing to test here.
    }
    let device = Tunnel::start(
        device_node.clone(),
        Role::Client,
        keys,
        [2; 16],
        vec![host_node.local_addr().unwrap()],
        Bridge::Device { local, ports },
    )
    .unwrap();
    let (h, d) = (host.clone(), device.clone());
    host_node.set_handler(Box::new(move |datagram, from| h.receive(datagram, from)));
    device_node.set_handler(Box::new(move |datagram, from| d.receive(datagram, from)));
    for _ in 0..100 {
        if matches!(device.state(), State::Connected(_))
            && matches!(host.state(), State::Connected(_))
        {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        matches!(device.state(), State::Connected(_)),
        "{:?}",
        device.state()
    );

    let mut tcp = TcpStream::connect((local, ports.http)).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    tcp.write_all(b"ping").unwrap();
    let mut reply = [0u8; 4];
    tcp.read_exact(&mut reply).unwrap();
    assert_eq!(&reply, b"pong");

    let client = UdpSocket::bind((local, 0)).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    client.send_to(b"frame", (local, ports.video)).unwrap();
    let mut buffer = [0u8; 64];
    let (length, _) = client.recv_from(&mut buffer).unwrap();
    assert_eq!(&buffer[..length], b"frame");

    device.close("test over");
    for _ in 0..50 {
        if host.is_closed() {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(host.is_closed());
    host_node.stop();
    device_node.stop();
}

/// A connected path with nothing to send sleeps until its keepalive, instead of going
/// round 50 times a second.
#[test]
fn an_idle_path_sleeps_until_its_next_deadline() {
    let keys = Keys::new([6; 32]).path(&[1; 16], &[2; 16]);
    let host_node = Node::bind("127.0.0.1:0".parse().unwrap(), Vec::new()).unwrap();
    let device_node = Node::bind("127.0.0.1:0".parse().unwrap(), Vec::new()).unwrap();
    let start = |node: &Node, role, peer: &Node, bridge| {
        let peer = vec![peer.local_addr().unwrap()];
        Tunnel::start(node.clone(), role, keys, [2; 16], peer, bridge).unwrap()
    };
    let host = start(
        &host_node,
        Role::Host,
        &device_node,
        Bridge::Host {
            service: "127.0.0.1".parse().unwrap(),
            source: "127.0.0.1".parse().unwrap(),
            ports: Ports::default(),
            device: "idle test".into(),
        },
    );
    // No ports: the device side binds nothing.
    let device = start(
        &device_node,
        Role::Client,
        &host_node,
        Bridge::Device {
            local: "127.0.0.1".parse().unwrap(),
            ports: Ports::default(),
        },
    );
    let (h, d) = (host.clone(), device.clone());
    host_node.set_handler(Box::new(move |datagram, from| h.receive(datagram, from)));
    device_node.set_handler(Box::new(move |datagram, from| d.receive(datagram, from)));
    for _ in 0..100 {
        if matches!(device.state(), State::Connected(_))
            && matches!(host.state(), State::Connected(_))
        {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(matches!(device.state(), State::Connected(_)));
    // Let the last punches and answers settle, then count a second of rounds.
    thread::sleep(Duration::from_millis(300));
    let before = device.shared.rounds.load(Ordering::Relaxed);
    thread::sleep(Duration::from_secs(1));
    let rounds = device.shared.rounds.load(Ordering::Relaxed) - before;
    assert!(rounds <= 3, "{rounds} rounds in a second");
    assert!(!device.is_closed());

    // A stream segment waiting for its acknowledgement (the host hears nothing for now)
    // makes it sleep until the retransmission is due, not spin.
    host_node.set_handler(Box::new(|_, _| {}));
    let before = device.shared.rounds.load(Ordering::Relaxed);
    device
        .shared
        .inner
        .lock()
        .unwrap()
        .reliable
        .open(TARGET_HTTP);
    device.flush();
    thread::sleep(Duration::from_millis(500));
    let rounds = device.shared.rounds.load(Ordering::Relaxed) - before;
    assert!(
        rounds <= 5,
        "{rounds} rounds in half a second with a segment in flight"
    );
    let h = host.clone();
    host_node.set_handler(Box::new(move |datagram, from| h.receive(datagram, from)));

    // A close wakes it at once.
    device.close("test over");
    for _ in 0..50 {
        if host.is_closed() {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(host.is_closed());
    host_node.stop();
    device_node.stop();
}

/// A device's punches say when they were sent, the host's say nothing.
#[test]
fn only_a_devices_punches_carry_its_clock() {
    let keys = Keys::new([7; 32]).path(&[1; 16], &[2; 16]);
    let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let stamp_of = |role: Role, bridge: Bridge| {
        let node = Node::bind("127.0.0.1:0".parse().unwrap(), Vec::new()).unwrap();
        let tunnel = Tunnel::start(
            node.clone(),
            role,
            keys,
            [2; 16],
            vec![peer.local_addr().unwrap()],
            bridge,
        )
        .unwrap();
        let mut buffer = [0u8; 256];
        let (length, _) = peer.recv_from(&mut buffer).unwrap();
        let opener = match role {
            Role::Client => &keys.client_to_host,
            Role::Host => &keys.host_to_client,
        };
        let opened = Opener::new(opener, keys.tag)
            .open(&buffer[..length])
            .unwrap();
        assert_eq!(opened.kind, Kind::Punch);
        tunnel.close("test over");
        node.stop();
        wire::punch_time(&opened.payload)
    };
    let device = stamp_of(
        Role::Client,
        Bridge::Device {
            local: "127.0.0.1".parse().unwrap(),
            ports: Ports::default(),
        },
    );
    let sent = device.expect("a device's punch carries its clock");
    assert!(node::unix_now().abs_diff(sent) <= 5);
    let host = stamp_of(
        Role::Host,
        Bridge::Host {
            service: "127.0.0.1".parse().unwrap(),
            source: "127.0.0.1".parse().unwrap(),
            ports: Ports::default(),
            device: "stamp test".into(),
        },
    );
    assert_eq!(host, None);
}

#[test]
fn loopback_addresses_are_stable_and_spread() {
    assert_eq!(loopback_for(78, "host-a"), loopback_for(78, "host-a"));
    assert_ne!(loopback_for(78, "host-a"), loopback_for(78, "host-b"));
    let IpAddr::V4(ip) = loopback_for(77, "x") else {
        panic!()
    };
    assert_eq!(ip.octets()[..2], [127, 77]);
    assert!(ip.is_loopback());
}
