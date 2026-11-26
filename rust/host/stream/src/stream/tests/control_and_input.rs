//! Unit tests for ENet/WebRTC control watchdog, pad feedback/haptics, roaming floods, and reply pinning.

use super::super::*;

#[test]
fn webrtc_sessions_are_not_ended_for_lacking_an_enet_control_peer() {
    let f = crate::state::test_support::Fixture::new();
    let client = f.client(u32::MAX);
    let moonlight = f.session(client.clone(), Role::Stream);
    assert!(super::super::expects_enet_control(&moonlight));
    let mut launch = f.launch(client, Role::Stream);
    launch.transport = butterpollo_session::session::Transport::WebRtc;
    let webrtc = Session::new(launch, Default::default());
    assert!(!super::super::expects_enet_control(&webrtc));
}
#[test]
fn the_control_watchdog_ends_a_moonlight_session_without_a_peer_but_not_a_webrtc_one() {
    let f = crate::state::test_support::Fixture::new();
    let client = f.client(u32::MAX);
    let moonlight = f.session(client.clone(), Role::Stream);
    let mut launch = f.launch(client, Role::Stream);
    launch.transport = butterpollo_session::session::Transport::WebRtc;
    let webrtc = Session::new(launch, Default::default());
    assert_ne!(moonlight.launch.id, webrtc.launch.id);
    let timeout = Duration::from_millis(20);
    let mut unattended = HashMap::new();
    let control = ReplySocket::bind("127.0.0.1:0").unwrap();
    let sweep = |unattended: &mut HashMap<String, Instant>| {
        let mut sessions = vec![moonlight.clone(), webrtc.clone()];
        super::super::end_unattended_sessions(
            &mut sessions,
            |_| false,
            unattended,
            timeout,
            &control,
        );
    };
    // The clock starts at the first sweep, so nothing ends yet.
    sweep(&mut unattended);
    assert!(!moonlight.stopping() && !webrtc.stopping());
    std::thread::sleep(timeout * 3);
    sweep(&mut unattended);
    assert!(
        moonlight.stopping(),
        "a GameStream session with no control peer must end"
    );
    assert!(
        !webrtc.stopping(),
        "a WebRTC session carries control on its DataChannels and has no ENet peer"
    );
    assert!(!unattended.contains_key(&webrtc.launch.id));
}
#[test]
fn waveform_haptics_require_negotiation_and_encrypted_multichannel_control() {
    let mut config = butterpollo_media_policy::rtsp::Negotiated::from_sdp(
        b"a=x-ml-general.featureFlags:7\na=x-ss-general.encryptionEnabled:1\n",
    )
    .unwrap();
    assert!(haptics_negotiated(&config, 9));
    assert!(!haptics_negotiated(&config, 8));
    config.encryption = 0;
    assert!(!haptics_negotiated(&config, 9));
    config.encryption = 1;
    config.moonlight_features = 3;
    assert!(!haptics_negotiated(&config, 9));
    assert!(!haptics_negotiated(&Default::default(), 9));
}
#[test]
fn with_rumble_off_only_the_first_stop_after_a_buzz_goes_out() {
    // An Xbox report: main and trigger motors, as VHF reports kind 4.
    let packets = |data: [u8; 8]| feedback_packets(3, 4, &data);
    let (on, off) = (packets([0, 9, 0, 7, 1, 0, 2, 0]), packets([0; 8]));
    const NONE: [u16; 0] = [];
    let mut rumble = RumbleForwarded::default();
    let mut forward = |allowed: bool, packets: &[(u16, Vec<u8>)]| {
        packets
            .iter()
            .filter(|(kind, payload)| rumble.forward(allowed, *kind, payload))
            .map(|(kind, _)| *kind)
            .collect::<Vec<_>>()
    };
    // Nothing ran: rumble off sends no stops, however often VHF repeats
    // its report for a lightbar or trigger effect change.
    assert_eq!(forward(false, &off), NONE);
    assert_eq!(forward(false, &off), NONE);
    // A buzz while rumble is on, then the client turns rumble off.
    assert_eq!(forward(true, &on), [RUMBLE_DATA, RUMBLE_TRIGGER_DATA]);
    assert_eq!(forward(false, &on), NONE);
    // Both motors' stops go out once, then no more.
    assert_eq!(forward(false, &off), [RUMBLE_DATA, RUMBLE_TRIGGER_DATA]);
    assert_eq!(forward(false, &off), NONE);
    // Rumble on forwards everything, stops included.
    assert_eq!(forward(true, &off), [RUMBLE_DATA, RUMBLE_TRIGGER_DATA]);
    // Other controllers and kinds follow the injector alone.
    let other = feedback_packets(4, RUMBLE_DATA, &[0; 4]);
    assert_eq!(forward(false, &other), NONE);
    assert!(!rumble.forward(false, 0x5502, &[3, 0, 1, 2, 3]));
    assert!(rumble.forward(true, 0x5502, &[3, 0, 1, 2, 3]));
}
#[test]
fn the_control_pass_sends_feedback_through_the_rumble_filter() {
    // The pass the control loop runs, so rumble off still stops a buzz once.
    let report = |data: [u8; 8]| PadReport::Feedback {
        id: 3,
        kind: 4,
        data: data.to_vec(),
    };
    let mut pcm = Default::default();
    let mut arbiter = Default::default();
    let mut rumble = RumbleForwarded::default();
    let mut pass = |allowed: bool, data: [u8; 8]| {
        pad_reports(
            [report(data)],
            |_| allowed,
            true,
            &[0; 16],
            &mut Default::default(),
            &mut pcm,
            &mut arbiter,
            &mut rumble,
        )
        .messages
        .into_iter()
        .map(|(kind, _)| kind)
        .collect::<Vec<_>>()
    };
    let both = [RUMBLE_DATA, RUMBLE_TRIGGER_DATA];
    assert_eq!(pass(true, [0, 9, 0, 7, 1, 0, 2, 0]), both);
    assert_eq!(pass(false, [0; 8]), both);
    assert_eq!(pass(false, [0; 8]), [0u16; 0]);
}
#[test]
fn composite_feedback_preserves_partial_trigger_flags_and_rumble_layout() {
    let mut effect = vec![0; 23];
    effect[..3].copy_from_slice(&[4, 1, 0]);
    let packet = feedback_packets(2, 0x5503, &effect).remove(0);
    assert_eq!(&packet.1[..5], &[2, 0, 4, 1, 0]);
    let rumble = feedback_packets(2, RUMBLE_DATA, &[0, 9, 0, 7]).remove(0);
    assert_eq!(&rumble.1, &[0xee, 0xff, 0xc0, 0, 2, 0, 0, 9, 0, 7]);
}
#[test]
fn steam_controller_feedback_goes_out_as_it_came() {
    let haptic = [vec![3, 0, 4, 0x82, 0, 2, 0xfa], vec![0; 8]].concat();
    assert_eq!(
        feedback_packets(3, 0x5504, &haptic),
        [(0x5504, haptic.clone())]
    );
    let settings = [vec![3, 0, 0x87, 1, 0x30, 1, 0], vec![0; 24]].concat();
    assert_eq!(
        feedback_packets(3, 0x5505, &settings),
        [(0x5505, settings.clone())]
    );
    assert!(feedback_packets(3, 0x5504, &haptic[..14]).is_empty());
}
#[test]
fn a_media_ping_finds_its_session_and_sees_a_teardown_or_roam_that_came_meanwhile() {
    let f = crate::state::test_support::Fixture::new();
    let client = f.client(u32::MAX);
    let mut launch = f.launch(client.clone(), Role::Stream);
    launch.ping = "0123456789abcdef".into();
    let s = Session::new(launch, Default::default());
    let sessions = &f.host.sessions;
    sessions
        .lock()
        .unwrap()
        .active
        .insert(s.launch.id.clone(), s.clone());
    let peers = Mutex::new(Peers::default());
    let peer: SocketAddr = "127.0.0.1:50000".parse().unwrap();
    let find = |ping: &[u8], peer: SocketAddr| {
        super::super::ping_session(&sessions.lock().unwrap(), &peers, ping, peer, false)
    };
    // Its own ping payload, from its client's IP.
    let (found, roams) = find(b"0123456789abcdef", peer).unwrap();
    assert!(Arc::ptr_eq(&found, &s));
    assert_eq!(roams, 0);
    assert!(find(b"0123456789abcdeX", peer).is_none());
    let elsewhere: SocketAddr = "127.0.0.2:50000".parse().unwrap();
    assert!(find(b"0123456789abcdef", elsewhere).is_none());
    // A bare PING only while one session plays to that IP.
    assert!(find(b"PING", peer).is_some());
    let other = f.session(client, Role::Stream);
    assert!(find(b"PING", peer).is_none());
    assert!(find(b"0123456789abcdef", peer).is_some());
    sessions.lock().unwrap().active.remove(&other.launch.id);
    let current =
        |roams: u32| super::super::still_current(&sessions.lock().unwrap(), &found, roams);
    assert!(current(roams));
    // A roam while the ping was answered: decided again. Once roamed, a bare PING
    // counts only from the address the session already uses.
    s.roamed(peer.ip(), Instant::now());
    assert!(!current(roams));
    assert!(find(b"PING", peer).is_none());
    peers.lock().unwrap().insert(&s.launch.id, false, peer);
    assert_eq!(find(b"PING", peer).map(|(_, roams)| roams), Some(1));
    assert!(current(1));
    let other_port: SocketAddr = "127.0.0.1:50001".parse().unwrap();
    assert!(peers.lock().unwrap().contains(peer));
    assert!(!peers.lock().unwrap().contains(other_port));
    // A teardown meanwhile: the ping is for no session now.
    sessions.lock().unwrap().begin_teardown(&s);
    assert!(!current(1));
    assert!(find(b"0123456789abcdef", peer).is_none());
}
#[test]
fn the_unattended_sweep_runs_when_a_peer_comes_or_goes_and_on_its_timer() {
    let start = Instant::now();
    let mut sweep = super::super::UnattendedSweep::new(start);
    assert!(sweep.due(start, false), "the first pass sweeps");
    assert_eq!(sweep.next(), start + super::super::UNATTENDED_SWEEP);
    // Input passes in between leave it alone.
    for ms in [1, 8, 100, 249] {
        assert!(!sweep.due(start + Duration::from_millis(ms), false));
    }
    assert_eq!(sweep.next(), start + super::super::UNATTENDED_SWEEP);
    // A peer that comes or goes sweeps at once, and the timer starts over.
    let changed = start + Duration::from_millis(120);
    assert!(sweep.due(changed, true));
    assert_eq!(sweep.next(), changed + super::super::UNATTENDED_SWEEP);
    assert!(!sweep.due(start + super::super::UNATTENDED_SWEEP, false));
    assert!(sweep.due(changed + super::super::UNATTENDED_SWEEP, false));
    // Still well inside the shortest ping timeout the watchdog enforces.
    assert!(super::super::UNATTENDED_SWEEP * 4 <= Duration::from_secs(1));
}
#[test]
fn control_backlog_is_serviced_without_the_feedback_delay() {
    for queued in [600, 1100] {
        let mut remaining = queued;
        while remaining >= super::super::CONTROL_EVENT_LIMIT {
            assert_eq!(
                super::super::control_wait_timeout(
                    Duration::from_millis(8),
                    super::super::CONTROL_EVENT_LIMIT
                ),
                0
            );
            remaining -= super::super::CONTROL_EVENT_LIMIT;
        }
        assert_eq!(
            super::super::control_wait_timeout(Duration::from_millis(8), remaining),
            8
        );
    }
    assert_eq!(super::super::control_wait_timeout(Duration::ZERO, 0), 0);
    assert_eq!(
        super::super::control_wait_timeout(Duration::from_micros(100), 0),
        1
    );
}
#[test]
fn pad_feedback_forwards_rumble_and_each_family_extra() {
    let rumble = (
        RUMBLE_DATA,
        vec![0xee, 0xff, 0xc0, 0, 2, 0, 255, 255, 128, 128],
    );
    // Xbox pads add trigger rumble.
    let xbox = feedback_packets(2, 4, &[255, 255, 128, 128, 1, 2, 3, 4]);
    assert_eq!(
        xbox,
        vec![
            rumble.clone(),
            (RUMBLE_TRIGGER_DATA, vec![2, 0, 1, 2, 3, 4])
        ]
    );
    // PlayStation pads add the lightbar only when the report sets it.
    let ds4 = feedback_packets(2, 5, &[255, 255, 128, 128, 12, 34, 56, 1]);
    assert_eq!(ds4, vec![rumble.clone(), (0x5502, vec![2, 0, 12, 34, 56])]);
    assert_eq!(
        feedback_packets(2, 5, &[255, 255, 128, 128, 12, 34, 56, 0]),
        vec![rumble]
    );
    // Reports of no known kind send nothing.
    assert!(feedback_packets(2, 3, &[255; 8]).is_empty());
}
#[test]
fn trigger_rumble_alone_still_reaches_the_client() {
    // The reporter's test (nightjar#124): impulse triggers on, ordinary
    // vibration off, so an Xbox pad's report has zero body magnitudes and only
    // its two trigger magnets running.
    let packets = feedback_packets(1, 4, &[0, 0, 0, 0, 0x40, 0x00, 0x20, 0x00]);
    assert_eq!(packets.len(), 2);
    assert_eq!(packets[0].0, RUMBLE_DATA);
    assert_eq!(
        &packets[0].1[4..],
        &[1, 0, 0, 0, 0, 0],
        "the body motors' stop"
    );
    assert_eq!(
        packets[1],
        (RUMBLE_TRIGGER_DATA, vec![1, 0, 0x40, 0, 0x20, 0]),
        "left trigger 0x0040, right 0x0020"
    );
    // Both go out while rumble is on.
    let mut rumble = RumbleForwarded::default();
    assert!(
        packets
            .iter()
            .all(|(kind, payload)| rumble.forward(true, *kind, payload))
    );
}
#[test]
fn vhf_rumble_and_stop_keep_the_strengths_in_moonlight_packets() {
    for (kind, size) in [(4, 8), (5, 32)] {
        for strengths in [[0x00, 0x80, 0x00, 0x40], [0; 4]] {
            let mut data = vec![0; size];
            data[..4].copy_from_slice(&strengths);
            let packets = feedback_packets(0, kind, &data);
            assert_eq!(packets[0].0, RUMBLE_DATA);
            assert_eq!(RUMBLE_DATA, 0x010b);
            assert_eq!(&packets[0].1[..4], &RUMBLE_MARKER.to_le_bytes());
            assert_eq!(&packets[0].1[4..], &[&[0, 0][..], &strengths].concat());
        }
    }
}
#[test]
fn rumble_reaches_a_single_channel_moonlight_peer() {
    let settings = || HostSettings {
        peer_limit: 1,
        channel_limit: 1,
        ..Default::default()
    };
    let socket = Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap());
    let address = socket.local_addr().unwrap();
    let mut host = Host::new(ControlSocket::new(socket), settings()).unwrap();
    let mut client = Host::new(UdpSocket::bind("127.0.0.1:0").unwrap(), settings()).unwrap();
    client.connect(address, 1, 0).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !host.peer(PeerID(0)).connected() || !client.peer(PeerID(0)).connected() {
        host.service().unwrap();
        client.service().unwrap();
        assert!(Instant::now() < deadline, "ENet connect timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(host.peer(PeerID(0)).channel_count(), 1);
    // The previous channel silently discarded every feedback message.
    assert_eq!(
        host.peer_mut(PeerID(0))
            .send(1, &Packet::new(&[][..], PacketKind::Reliable)),
        Err(rusty_enet::error::PeerSendError::InvalidChannel)
    );
    let key = [0x42; 16];
    for v2 in [false, true] {
        for strengths in [[0x00, 0x80, 0x00, 0x40], [0; 4]] {
            let mut data = vec![0; 8];
            data[..4].copy_from_slice(&strengths);
            let (kind, payload) = feedback_packets(2, 4, &data).remove(0);
            let message =
                butterpollo_media_policy::packet::encrypted_control(&key, 7, kind, &payload, v2)
                    .unwrap();
            host.socket_mut().hold();
            host.peer_mut(PeerID(0))
                .send(
                    FEEDBACK_CHANNEL,
                    &Packet::new(message, PacketKind::Reliable),
                )
                .unwrap();
            host.flush();
            host.socket_mut().release().unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            let received = loop {
                host.service().unwrap();
                if let Some(Event::Receive {
                    channel_id, packet, ..
                }) = client.service().unwrap()
                {
                    assert_eq!(channel_id, 0);
                    break packet;
                }
                assert!(Instant::now() < deadline, "rumble delivery timed out");
                std::thread::sleep(Duration::from_millis(1));
            };
            let bytes = received.data();
            let mut iv = vec![0; if v2 { 12 } else { 16 }];
            iv[0] = 7;
            if v2 {
                iv[10..].copy_from_slice(b"HC");
            }
            let plain =
                butterpollo_crypto::crypto::gcm_open(&key, &iv, &bytes[8..24], &bytes[24..])
                    .unwrap();
            // Moonlight's decrypted V2 header, marker, controller ID,
            // then the low- and high-frequency strengths, all LE.
            assert_eq!(
                &plain[..10],
                &[0x0b, 0x01, 10, 0, 0xee, 0xff, 0xc0, 0, 2, 0]
            );
            assert_eq!(&plain[10..], &strengths);
        }
    }
}
#[test]
fn dualsense_trigger_effects_use_the_moonlight_layout() {
    let mut data = vec![0u8; 32];
    data[..4].copy_from_slice(&[1, 2, 3, 4]);
    data[7] = 2;
    data[10] = 0x21;
    for (i, byte) in data[11..21].iter_mut().enumerate() {
        *byte = i as u8 + 1;
    }
    data[21] = 0x26;
    for (i, byte) in data[22..32].iter_mut().enumerate() {
        *byte = i as u8 + 11;
    }
    let packets = feedback_packets(1, 5, &data);
    let (_, trigger) = packets.iter().find(|(kind, _)| *kind == 0x5503).unwrap();
    let mut expected = vec![1, 0, 0x0c, 0x21, 0x26];
    expected.extend(1..=20);
    assert_eq!(trigger, &expected);
    assert!(packets.iter().any(|(kind, _)| *kind == RUMBLE_DATA));
}
/// A flood of roaming datagrams ends one receive with none; the ENet datagram
/// behind it arrives on a later call.
#[test]
fn a_roaming_flood_does_not_hold_the_control_receive() {
    let mut socket = ControlSocket::new(Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap()));
    socket.socket.set_nonblocking(true).unwrap();
    socket.roaming.set_enabled(true);
    let host = socket.socket.local_addr().unwrap();
    let client = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut magic = [0u8; butterpollo_session::roaming::DATAGRAM_BYTES];
    magic[..4].copy_from_slice(&[0x0F, 0xFE, 0x52, 0x4D]);
    for _ in 0..SWALLOWED_PER_RECEIVE + 10 {
        client.send_to(&magic, host).unwrap();
    }
    client.send_to(b"enet", host).unwrap();
    let mut buffer = [0; rusty_enet::MTU_MAX];
    let before = Instant::now();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut empty = 0;
    let received = loop {
        assert!(Instant::now() < deadline, "the ENet datagram never arrived");
        match rusty_enet::Socket::receive(&mut socket, &mut buffer).unwrap() {
            Some((from, rusty_enet::PacketReceived::Complete(size))) => break (from, size),
            Some(_) => unreachable!(),
            None => {
                empty += 1;
                // Roaming datagrams are no input: they start no latency stamp.
                assert!(socket.first_received.is_none());
            }
        }
    };
    assert_eq!(received, (client.local_addr().unwrap(), 4));
    assert!(empty >= 1, "the flood ended a receive");
    // The pass's first datagram for ENet is stamped for the input latency; hold()
    // starts the next pass.
    assert!(socket.first_received.is_some_and(|at| at >= before));
    socket.hold();
    assert!(socket.first_received.is_none());
}
#[test]
fn held_control_datagrams_go_out_in_order_once_released() {
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    let address = receiver.local_addr().unwrap();
    let mut socket = ControlSocket::new(Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap()));
    let mut buffer = [0u8; 64];
    socket.hold();
    // The middle one is too large for UDP: it is lost, the rest go out.
    for datagram in [&b"one"[..], &[0; 70_000], b"two"] {
        assert_eq!(
            rusty_enet::Socket::send(&mut socket, address, datagram).unwrap(),
            datagram.len()
        );
    }
    receiver.set_nonblocking(true).unwrap();
    assert!(receiver.recv_from(&mut buffer).is_err());
    socket.release().unwrap();
    receiver.set_nonblocking(false).unwrap();
    receiver
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut received = Vec::new();
    for _ in 0..2 {
        let (length, _) = receiver.recv_from(&mut buffer).unwrap();
        received.push(buffer[..length].to_vec());
    }
    assert_eq!(received, [b"one".to_vec(), b"two".to_vec()]);
    // Released: sent at once again.
    rusty_enet::Socket::send(&mut socket, address, b"three").unwrap();
    let (length, _) = receiver.recv_from(&mut buffer).unwrap();
    assert_eq!(&buffer[..length], b"three");
}
/// Issue #54: Moonlight's ENet, like rusty_enet, drops replies from any
/// address but the one it connected to. To one of its own addresses,
/// Windows replies from that address, so a client on one address of this
/// PC that reached another only connects when the host pins the reply
/// source. Skipped without two such addresses of one family.
#[test]
fn enet_control_answers_from_the_address_the_client_reached() {
    let settings = || HostSettings {
        peer_limit: 1,
        channel_limit: 1,
        ..Default::default()
    };
    let addresses = butterpollo_win_io::net::unicast_addresses().unwrap();
    // A client address that reaches another address of this PC.
    let pair = addresses.iter().find_map(|&client_ip| {
        addresses.iter().find_map(|&reached| {
            let host = UdpSocket::bind((reached, 0)).ok()?;
            host.set_read_timeout(Some(Duration::from_millis(300)))
                .ok()?;
            let client = UdpSocket::bind((client_ip, 0)).ok()?;
            (client_ip != reached && client_ip.is_ipv6() == reached.is_ipv6()).then_some(())?;
            client.send_to(b"ping", host.local_addr().ok()?).ok()?;
            host.recv_from(&mut [0u8; 16]).ok()?;
            Some((client_ip, reached))
        })
    });
    let Some((client_ip, reached)) = pair else {
        println!("skipped: no two addresses of one family among {addresses:?}");
        return;
    };
    let wildcard = if reached.is_ipv6() {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    };
    let socket = Arc::new(ReplySocket::bind(wildcard).unwrap());
    let reached = SocketAddr::new(reached, socket.local_addr().unwrap().port());
    let mut host = Host::new(ControlSocket::new(socket.clone()), settings()).unwrap();
    let mut client = Host::new(UdpSocket::bind((client_ip, 0)).unwrap(), settings()).unwrap();
    let client_address = client.socket().local_addr().unwrap();
    client.connect(reached, 1, 0).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !host.peer(PeerID(0)).connected() || !client.peer(PeerID(0)).connected() {
        host.service().unwrap();
        client.service().unwrap();
        assert!(
            Instant::now() < deadline,
            "ENet connect from {client_address} to {reached} timed out"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(socket.source(client_address), Some(reached.ip()));
    // Held datagrams, sent on release, leave from the same address.
    let observer = UdpSocket::bind((client_ip, 0)).unwrap();
    observer
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    observer.send_to(b"hello", reached).unwrap();
    let mut buffer = [0u8; rusty_enet::MTU_MAX];
    let mut control = ControlSocket::new(socket);
    let deadline = Instant::now() + Duration::from_secs(2);
    let peer = loop {
        match rusty_enet::Socket::receive(&mut control, &mut buffer).unwrap() {
            Some((peer, _)) if peer == observer.local_addr().unwrap() => break peer,
            _ => assert!(Instant::now() < deadline, "the datagram did not arrive"),
        }
    };
    control.hold();
    rusty_enet::Socket::send(&mut control, peer, b"held").unwrap();
    control.release().unwrap();
    let (length, from) = observer.recv_from(&mut buffer).unwrap();
    assert_eq!((&buffer[..length], from), (&b"held"[..], reached));
}

#[test]
fn session_ping_feature_rejects_bare_ping() {
    let f = crate::state::test_support::Fixture::new();
    let client = f.client(u32::MAX);
    let mut launch = f.launch(client, Role::Stream);
    launch.ping = "fedcba9876543210".into();
    let negotiated = butterpollo_media_policy::rtsp::Negotiated {
        moonlight_features: 0x02,
        ..Default::default()
    };
    let s = Session::new(launch, negotiated);
    let sessions = &f.host.sessions;
    sessions
        .lock()
        .unwrap()
        .active
        .insert(s.launch.id.clone(), s.clone());
    let peers = Mutex::new(Peers::default());
    let peer: SocketAddr = "127.0.0.1:50000".parse().unwrap();
    let find = |ping: &[u8]| {
        super::super::ping_session(&sessions.lock().unwrap(), &peers, ping, peer, false)
    };
    // Even with a single session on this IP, bare "PING" is refused when session ping is negotiated.
    assert!(find(b"PING").is_none());
    let (found, _) = find(b"fedcba9876543210").unwrap();
    assert!(Arc::ptr_eq(&found, &s));
    peers.lock().unwrap().insert(&s.launch.id, false, peer);
    s.roamed(peer.ip(), Instant::now());
    assert!(find(b"PING").is_none());
    assert!(find(b"fedcba9876543210").is_some());
}

#[test]
fn control_seq_memory_preserves_sequence_and_replay_window_across_restarts() {
    let memory = super::super::ControlSeqMemory::default();
    let key_a = [0x11; 16];
    let key_b = [0x22; 16];

    {
        let state = memory.for_launch("launch-a", key_a);
        let mut guard = state.lock().unwrap();
        assert_eq!(guard.sequence, 0);
        guard.sequence = 2;
        assert!(guard.received.accept(0));
        assert!(guard.received.accept(2));
        assert!(!guard.received.accept(2));
    }

    // Re-acquiring for the same launch id and key keeps both sequence and replay window.
    {
        let restarted = memory.for_launch("launch-a", key_a);
        let mut guard = restarted.lock().unwrap();
        assert_eq!(guard.sequence, 2);
        assert!(!guard.received.accept(0));
        assert!(!guard.received.accept(2));
        assert!(guard.received.accept(1));
    }

    // A new launch starting with the same key starts cleanly rather than inheriting replay state.
    {
        let second_launch = memory.for_launch("launch-a-2", key_a);
        let mut guard = second_launch.lock().unwrap();
        assert_eq!(guard.sequence, 0);
        assert!(guard.received.accept(0));
    }

    // Ended launches are pruned on retain_launches.
    let b = memory.for_launch("launch-b", key_b);
    b.lock().unwrap().sequence = 5;
    memory.retain_launches([("launch-a", &key_a)]);
    let reset_b = memory.for_launch("launch-b", key_b);
    assert_eq!(reset_b.lock().unwrap().sequence, 0);
}
