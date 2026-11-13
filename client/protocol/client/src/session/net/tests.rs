use super::*;

#[test]
fn private_ranges_match_the_c() {
    for private in ["10.1.2.3", "172.31.0.1", "192.168.1.1", "169.254.0.5"] {
        assert!(is_private(private.parse().unwrap()), "{private}");
    }
    for public in ["8.8.8.8", "172.32.0.1", "100.64.0.1", "127.0.0.1"] {
        assert!(!is_private(public.parse().unwrap()), "{public}");
    }
    for private in ["fe80::1", "fec0::1", "fd00::1", "fc12::1"] {
        assert!(is_private(private.parse().unwrap()), "{private}");
    }
    for public in ["2001:db8::1", "::1", "fe00::1"] {
        assert!(!is_private(public.parse().unwrap()), "{public}");
    }
}

#[test]
fn nat64_prefixes_are_recognized() {
    let well_known: Ipv6Addr = "64:ff9b::c000:aa".parse().unwrap();
    assert!(is_nat64_with(
        &"64:ff9b::808:808".parse().unwrap(),
        &[well_known]
    ));
    assert!(!is_nat64_with(
        &"2001:db8::808:808".parse().unwrap(),
        &[well_known]
    ));
    // A /64 prefix: the IPv4 address sits after the reserved byte 8.
    let slash64: Ipv6Addr = "2001:db8:1:2:c0:0:aa00:0".parse().unwrap();
    assert!(is_nat64_with(
        &"2001:db8:1:2:0808:0808::".parse().unwrap(),
        &[slash64]
    ));
    assert!(!is_nat64_with(&"64:ff9b::808:808".parse().unwrap(), &[]));
}

#[test]
fn pings_carry_the_payload_and_a_sequence() {
    let ping = ping_datagram(Some(*b"0123456789abcdef"), 2);
    assert_eq!(&ping[..16], b"0123456789abcdef");
    assert_eq!(&ping[16..], &[0, 0, 0, 2]);
    assert_eq!(ping_datagram(None, 1), b"PING");
}

#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn media_sockets_are_bound_with_their_priority() {
    use crate::priority::tests::read_priority;
    let bind = |qos, network_priority| {
        bind_udp(
            (Ipv4Addr::LOCALHOST, 0).into(),
            0,
            qos,
            network_priority,
            |_| {},
        )
        .unwrap()
    };
    for network_priority in [false, true] {
        assert_eq!(read_priority(&bind(Qos::Video, network_priority)), 5);
        // The audio class is the microphone's too.
        assert_eq!(read_priority(&bind(Qos::Audio, network_priority)), 6);
    }
    assert_eq!(read_priority(&bind(Qos::BestEffort, false)), 0);
    assert_eq!(read_priority(&bind(Qos::BestEffort, true)), 6);
}

// Moonlight V+'s DSCP marking, and the priority it must not reset (IP_TOS does on Linux).
#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn media_sockets_are_marked_and_keep_their_priority() {
    use crate::priority::tests::{read_option, read_priority};
    let bind = |qos| bind_udp((Ipv4Addr::LOCALHOST, 0).into(), 0, qos, false, |_| {}).unwrap();
    let audio = bind(Qos::Audio);
    assert_eq!(read_option(&audio, libc::IPPROTO_IP, libc::IP_TOS), 0xB8);
    assert_eq!(read_priority(&audio), 6);
    let video = bind(Qos::Video);
    assert_eq!(read_option(&video, libc::IPPROTO_IP, libc::IP_TOS), 0x88);
    assert_eq!(read_priority(&video), 5);
    let plain = bind(Qos::BestEffort);
    assert_eq!(read_option(&plain, libc::IPPROTO_IP, libc::IP_TOS), 0);
}

#[test]
fn a_pinger_survives_a_closed_port_and_stops_without_waiting_out_the_interval() {
    // Nothing listens: on loopback the sends draw ICMP port unreachable.
    let target = UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut pinger = Pinger::start("TestPing", &socket, target, None).unwrap();
    std::thread::sleep(PING_INTERVAL + Duration::from_millis(200));
    assert!(!pinger.thread.as_ref().unwrap().is_finished());
    let stopping = std::time::Instant::now();
    pinger.stop();
    assert!(stopping.elapsed() < PING_INTERVAL, "stop waited for the next ping");
    assert!(pinger.thread.is_none());
}

#[test]
fn a_rebound_pinger_pings_from_the_new_socket_at_once_and_counts_on() {
    let host = UdpSocket::bind("127.0.0.1:0").unwrap();
    host.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let old = UdpSocket::bind("127.0.0.1:0").unwrap();
    let new = UdpSocket::bind("127.0.0.1:0").unwrap();
    let payload = Some(*b"0123456789abcdef");
    let mut pinger =
        Pinger::start("TestPing", &old, host.local_addr().unwrap(), payload).unwrap();
    let mut buffer = [0u8; 64];
    let (n, from) = host.recv_from(&mut buffer).unwrap();
    assert_eq!(from, old.local_addr().unwrap());
    assert_eq!(&buffer[..n], ping_datagram(payload, 1));
    let rebinding = std::time::Instant::now();
    pinger.handle().rebind(new.try_clone().unwrap());
    let (n, from) = host.recv_from(&mut buffer).unwrap();
    assert!(
        rebinding.elapsed() < PING_INTERVAL,
        "the rebound pinger waited for its interval"
    );
    assert_eq!(from, new.local_addr().unwrap());
    assert_eq!(&buffer[..n], ping_datagram(payload, 2));
    pinger.stop();
}

#[test]
fn a_failed_receive_waits_a_while_for_the_next_socket_then_gives_up() {
    let stop = AtomicBool::new(false);
    let next = Arc::new(NextSocket::default());
    // The rebind comes after the failure: the wait gets its socket when it is set.
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap();
    let setter = {
        let next = next.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            next.set(socket);
        })
    };
    let waiting = Instant::now();
    let moved = next.wait(REBIND_WAIT, &stop).expect("the socket set meanwhile");
    assert_eq!(moved.local_addr().unwrap(), address);
    assert!(waiting.elapsed() < REBIND_WAIT, "waited out the timeout");
    setter.join().unwrap();
    // No rebind: about REBIND_WAIT, not forever.
    let waiting = Instant::now();
    assert!(next.wait(REBIND_WAIT, &stop).is_none());
    let waited = waiting.elapsed();
    assert!(waited >= REBIND_WAIT && waited < 2 * REBIND_WAIT, "{waited:?}");
    // A stopping session does not wait.
    stop.store(true, Ordering::Release);
    let waiting = Instant::now();
    assert!(next.wait(REBIND_WAIT, &stop).is_none());
    assert!(waiting.elapsed() < UDP_RECV_POLL_TIMEOUT * 2);
}

#[test]
fn a_port_rewrite_keeps_the_ipv6_scope() {
    let scoped = SocketAddr::V6(std::net::SocketAddrV6::new(
        "fe80::1".parse().unwrap(),
        48010,
        0,
        7,
    ));
    let SocketAddr::V6(rewritten) = with_port(scoped, 47999) else {
        panic!("an IPv6 address stays IPv6");
    };
    assert_eq!(rewritten.port(), 47999);
    assert_eq!(rewritten.scope_id(), 7);
}

#[test]
fn the_local_address_toward_ipv6_loopback_is_ipv6() {
    let Ok(probe) = UdpSocket::bind("[::1]:0") else {
        eprintln!("skipped: no IPv6 loopback");
        return;
    };
    let local = local_address_toward(probe.local_addr().unwrap()).unwrap();
    assert_eq!(local, "[::1]:0".parse().unwrap());
    bind_udp(local, 0, Qos::BestEffort, false, |_| {}).unwrap();
}

/// nightjar#38: a link-local host only works with its scope, through the connect to it
/// and the binds after.
#[cfg(target_os = "linux")]
#[test]
fn a_link_local_host_keeps_its_scope() {
    let Some(local) = link_local_address() else {
        eprintln!("skipped: no link-local IPv6 interface");
        return;
    };
    // Our own address stands in for the host on the same link.
    let host = UdpSocket::bind(local).unwrap();
    let toward = local_address_toward(host.local_addr().unwrap()).unwrap();
    let SocketAddr::V6(v6) = toward else {
        panic!("{toward} is not IPv6");
    };
    assert_eq!(v6.scope_id(), local.scope_id());
    let socket = bind_udp(toward, 0, Qos::BestEffort, false, |_| {}).unwrap();
    socket
        .send_to(b"hello", host.local_addr().unwrap())
        .unwrap();
}

/// A usable link-local IPv6 address of this machine with its scope, from
/// /proc/net/if_inet6 (address, interface index, prefix, scope, flags, name).
#[cfg(target_os = "linux")]
fn link_local_address() -> Option<std::net::SocketAddrV6> {
    // IFA_F_DADFAILED and IFA_F_TENTATIVE: not bindable.
    const UNUSABLE: u32 = 0x08 | 0x40;
    let table = std::fs::read_to_string("/proc/net/if_inet6").ok()?;
    table.lines().find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let address = Ipv6Addr::from(u128::from_str_radix(fields.first()?, 16).ok()?);
        let index = u32::from_str_radix(fields.get(1)?, 16).ok()?;
        let flags = u32::from_str_radix(fields.get(4)?, 16).ok()?;
        (address.segments()[0] & 0xffc0 == 0xfe80 && flags & UNUSABLE == 0)
            .then(|| std::net::SocketAddrV6::new(address, 0, 0, index))
    })
}

#[test]
fn the_granted_receive_buffer_is_read_back() {
    let asked = 1 << 20;
    let bind = |size| {
        bind_udp_sized(
            (Ipv4Addr::LOCALHOST, 0).into(),
            size,
            Qos::BestEffort,
            false,
            |m: &str| panic!("{m}"),
        )
        .unwrap()
        .1
    };
    let buffer = bind(asked).expect("a size was asked for");
    assert_eq!(buffer.asked, asked);
    // Linux grants up to net.core.rmem_max, silently; Windows what was asked.
    #[cfg(target_os = "linux")]
    let expected = std::fs::read_to_string("/proc/sys/net/core/rmem_max")
        .unwrap()
        .trim()
        .parse::<usize>()
        .unwrap()
        .min(asked);
    #[cfg(not(target_os = "linux"))]
    let expected = asked;
    assert_eq!(buffer.granted, expected);
    assert_eq!(bind(0), None, "the default is not read");
}

#[test]
fn a_short_receive_buffer_says_how_much_and_how_to_raise_it() {
    let full = ReceiveBuffer {
        asked: 4 << 20,
        granted: 4 << 20,
    };
    assert!(!full.short());
    assert_eq!(full.describe("Video"), "Video receive buffer: 4096 KiB, as asked");
    let capped = ReceiveBuffer {
        asked: 4 << 20,
        granted: 212_992,
    };
    assert!(capped.short());
    let text = capped.describe("Video");
    assert!(
        text.starts_with("Video receive buffer: the system granted 208 KiB of the 4096 KiB asked"),
        "{text}"
    );
    assert_eq!(
        text.contains("sudo sysctl -w net.core.rmem_max=4194304"),
        cfg!(any(target_os = "linux", target_os = "android")),
        "{text}"
    );
}

#[test]
fn receive_times_out_without_data() {
    let socket = bind_udp(
        (Ipv4Addr::LOCALHOST, 0).into(),
        1 << 20,
        Qos::BestEffort,
        false,
        |_| {},
    )
    .unwrap();
    let mut buffer = [0u8; 64];
    assert!(receive(&socket, &mut buffer).unwrap().is_none());
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    sender
        .send_to(b"hello", socket.local_addr().unwrap())
        .unwrap();
    assert_eq!(receive(&socket, &mut buffer).unwrap(), Some(5));
}

/// A loopback media socket with a receive buffer that holds every test datagram.
fn media_socket() -> UdpSocket {
    bind_udp((Ipv4Addr::LOCALHOST, 0).into(), 1 << 20, Qos::Video, false, |_| {}).unwrap()
}

/// `count` datagrams of `size` bytes, the last one `last` bytes, each starting with its
/// sequence number, sent to `to`.
fn send_numbered(to: &UdpSocket, count: u16, size: usize, last: usize) -> Vec<Vec<u8>> {
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let datagrams: Vec<Vec<u8>> = (0..count)
        .map(|n| {
            let length = if n + 1 == count { last } else { size };
            let mut datagram: Vec<u8> = (0..length).map(|i| (i as u8) ^ (n as u8)).collect();
            datagram[..2].copy_from_slice(&n.to_be_bytes());
            datagram
        })
        .collect();
    for datagram in &datagrams {
        sender.send_to(datagram, to.local_addr().unwrap()).unwrap();
    }
    datagrams
}

/// Receives until `expected` datagrams came: them and the count of each receive.
fn receive_all(
    batch: &mut BatchReceiver,
    socket: &UdpSocket,
    expected: usize,
) -> (Vec<Vec<u8>>, Vec<usize>) {
    let mut received = Vec::new();
    let mut counts = Vec::new();
    while received.len() < expected {
        let count = batch.receive(socket).unwrap();
        assert_ne!(count, 0, "timed out after {} datagrams", received.len());
        for i in 0..count {
            assert_eq!(batch.length(i), batch.datagram(i).len());
            received.push(batch.datagram(i).to_vec());
        }
        counts.push(count);
    }
    (received, counts)
}

#[test]
fn a_batch_delivers_every_queued_datagram_in_order() {
    let socket = media_socket();
    let mut batch = BatchReceiver::new(&socket, 1500, true);
    assert!(batch.take_fallback().is_none());
    // More than a batch, all queued before the first receive, the last one short.
    let sent = send_numbered(&socket, RECEIVE_BATCH as u16 * 2 + 5, 1392, 77);
    let (received, counts) = receive_all(&mut batch, &socket, sent.len());
    assert_eq!(received, sent);
    // With UDP_GRO a call's count is its datagrams, which can exceed the batch.
    if !batch.gro() {
        assert!(counts.iter().all(|&count| count <= RECEIVE_BATCH), "{counts:?}");
    }
    if cfg!(any(target_os = "linux", target_os = "android")) {
        assert!(batch.batched());
        // Not necessarily all in the first call: under load the kernel may queue them late.
        assert!(counts.iter().any(|&count| count > 1), "{counts:?}");
    } else if !batch.batched() {
        assert!(counts.iter().all(|&count| count == 1), "{counts:?}");
    }
    // Coalescing on Windows: whether loopback coalesces depends on the Windows version.
    assert_eq!(batch.receive(&socket).unwrap(), 0, "then the poll timeout");
}

#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn a_kernel_without_batches_takes_one_datagram_per_call() {
    let socket = media_socket();
    mmsg::REFUSED.set(true);
    let mut batch = BatchReceiver::new(&socket, 1500, true);
    assert!(!batch.batched());
    let refusal = batch.take_fallback().expect("the refusal");
    assert_eq!(refusal.raw_os_error(), Some(libc::ENOSYS));
    let sent = send_numbered(&socket, 5, 1392, 10);
    let (received, counts) = receive_all(&mut batch, &socket, sent.len());
    mmsg::REFUSED.set(false);
    assert_eq!(received, sent);
    assert_eq!(counts, [1; 5]);
    assert!(batch.take_fallback().is_none(), "reported once");
}

#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn a_refusal_mid_stream_is_reported_once_and_receives_go_on_one_at_a_time() {
    let socket = media_socket();
    let mut batch = BatchReceiver::new(&socket, 1500, true);
    assert!(batch.batched());
    let sent = send_numbered(&socket, 3, 1392, 10);
    assert_eq!(receive_all(&mut batch, &socket, sent.len()).0, sent);
    assert!(batch.take_fallback().is_none());
    mmsg::REFUSED.set(true);
    // Reported after the receive that met it, even when that one then timed out.
    assert_eq!(batch.receive(&socket).unwrap(), 0);
    let refusal = batch.take_fallback().expect("the refusal");
    assert_eq!(refusal.raw_os_error(), Some(libc::ENOSYS));
    assert!(!batch.batched());
    let sent = send_numbered(&socket, 4, 1392, 10);
    let (received, counts) = receive_all(&mut batch, &socket, sent.len());
    mmsg::REFUSED.set(false);
    assert_eq!(received, sent);
    assert_eq!(counts, [1; 4]);
    assert!(batch.take_fallback().is_none(), "reported once");
}

#[test]
fn without_batches_every_datagram_comes_in_a_call_of_its_own() {
    let socket = media_socket();
    let mut batch = BatchReceiver::unbatched(&socket, 1500);
    assert!(!batch.batched());
    let sent = send_numbered(&socket, 10, 1392, 3);
    let (received, counts) = receive_all(&mut batch, &socket, sent.len());
    assert_eq!(received, sent);
    assert_eq!(counts, [1; 10]);
    assert_eq!(batch.receive(&socket).unwrap(), 0);
}

#[test]
fn a_datagram_is_delivered_without_waiting_for_more() {
    for batched in [true, false] {
        let socket = media_socket();
        // A receive that waited to fill its batch would wait this long.
        socket.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
        let address = socket.local_addr().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let receiver = std::thread::spawn(move || {
            let mut batch = if batched {
                BatchReceiver::new(&socket, 1500, true)
            } else {
                BatchReceiver::unbatched(&socket, 1500)
            };
            tx.send(()).unwrap();
            let count = batch.receive(&socket).unwrap();
            (count, batch.datagram(0).to_vec())
        });
        rx.recv().unwrap();
        // The receiver is about to block, or already blocked: one datagram, then nothing.
        std::thread::sleep(Duration::from_millis(50));
        UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .send_to(b"alone", address)
            .unwrap();
        let waiting = Instant::now();
        while !receiver.is_finished() {
            assert!(waiting.elapsed() < Duration::from_secs(10), "the receive waited for more");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(receiver.join().unwrap(), (1, b"alone".to_vec()));
    }
}

#[test]
fn a_long_datagram_counts_in_full() {
    let socket = media_socket();
    let mut batch = BatchReceiver::new(&socket, 100, true);
    send_numbered(&socket, 2, 1000, 50);
    let mut seen = Vec::new();
    while seen.len() < 2 {
        let count = batch.receive(&socket).unwrap();
        assert_ne!(count, 0);
        seen.extend((0..count).map(|i| (batch.length(i), batch.datagram(i).len())));
    }
    // recvmmsg batches cut it to the reader's maximum (the slot's rounding without UDP_GRO,
    // the maximum itself with it); elsewhere, coalesced reads included, it is read whole.
    let cut = if !cfg!(any(target_os = "linux", target_os = "android")) || !batch.batched() {
        1000
    } else if batch.gro() {
        100
    } else {
        128
    };
    assert_eq!(seen, [(1000, cut), (50, 50)]);
    let mut fallback = BatchReceiver::unbatched(&socket, 100);
    send_numbered(&socket, 1, 1000, 1000);
    assert_eq!(fallback.receive(&socket).unwrap(), 1);
    assert_eq!(fallback.length(0), 1000);
}

#[test]
fn a_batch_skips_icmp_reports() {
    // Connected to a closed port, a send draws an ICMP port unreachable, which the next
    // receive reports as an error.
    let refused = || {
        let socket = media_socket();
        let closed = UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        socket.connect(closed).unwrap();
        socket
    };
    let control = refused();
    control.send(b"ping").unwrap();
    std::thread::sleep(Duration::from_millis(50));
    let error = control.recv(&mut [0; 16]).expect_err("an ICMP report");
    assert!(matches!(failure(error), Failure::Retry));
    for batched in [true, false] {
        let socket = refused();
        let mut batch = if batched {
            BatchReceiver::new(&socket, 1500, true)
        } else {
            BatchReceiver::unbatched(&socket, 1500)
        };
        socket.send(b"ping").unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(batch.receive(&socket).unwrap(), 0, "batched {batched}");
    }
}

/// Takes every datagram of `segments`, a receive's worth at a time: the (start, length)
/// of each and the count of each take.
fn take_all(mut segments: Segments) -> (Vec<(usize, usize)>, Vec<usize>) {
    let mut lengths = [0; RECEIVE_BATCH];
    let (mut datagrams, mut counts) = (Vec::new(), Vec::new());
    while segments.left() > 0 {
        let (base, count) = segments.take(&mut lengths);
        let stride = segments.segment();
        datagrams.extend((0..count).map(|i| (base + i * stride, lengths[i])));
        counts.push(count);
    }
    (datagrams, counts)
}

#[test]
fn a_coalesced_read_splits_into_its_datagrams() {
    // Equal datagrams.
    let (datagrams, counts) = take_all(Segments::new(3 * 1408, Some(1408)));
    assert_eq!(datagrams, [(0, 1408), (1408, 1408), (2816, 1408)]);
    assert_eq!(counts, [3]);
    // A short last one.
    let (datagrams, _) = take_all(Segments::new(2 * 1408 + 77, Some(1408)));
    assert_eq!(datagrams, [(0, 1408), (1408, 1408), (2816, 77)]);
    // No control message: the read is one datagram, as is an empty one.
    assert_eq!(take_all(Segments::new(1408, None)).0, [(0, 1408)]);
    assert_eq!(take_all(Segments::new(0, None)).0, [(0, 0)]);
    // A size that cannot split the read leaves it whole.
    for size in [Some(0), Some(1408), Some(5000)] {
        assert_eq!(take_all(Segments::new(1408, size)).0, [(0, 1408)], "{size:?}");
    }
    // More datagrams than a receive takes: the rest come with the next receives.
    let (datagrams, counts) = take_all(Segments::new(70 * 100 + 30, Some(100)));
    assert_eq!(counts, [RECEIVE_BATCH, RECEIVE_BATCH, 7]);
    assert_eq!(datagrams.len(), 71);
    assert!(datagrams.windows(2).all(|w| w[1].0 == w[0].0 + 100), "{datagrams:?}");
    assert_eq!(datagrams[70], (7000, 30));
    assert_eq!(datagrams.iter().map(|d| d.1).sum::<usize>(), 7030);
}

/// A control message as `WSARecvMsg` lays it out: `WSACMSGHDR`, then the data at the
/// natural alignment, then padding to the header alignment.
fn control_message(level: i32, kind: i32, data: &[u8]) -> Vec<u8> {
    let word = std::mem::size_of::<usize>();
    let start = (word + 8).next_multiple_of(word);
    let length = start + data.len();
    let mut message = Vec::new();
    message.extend_from_slice(&length.to_ne_bytes());
    message.extend_from_slice(&level.to_ne_bytes());
    message.extend_from_slice(&kind.to_ne_bytes());
    message.resize(start, 0);
    message.extend_from_slice(data);
    message.resize(length.next_multiple_of(word), 0);
    message
}

#[test]
fn the_datagram_size_is_read_from_the_coalesced_info() {
    let info = control_message(17, 3, &1408u32.to_ne_bytes());
    assert_eq!(coalesced_segment(&info), Some(1408));
    // Behind another message (IP_PKTINFO's level and type, say).
    let mut both = control_message(0, 19, &[1; 20]);
    both.extend_from_slice(&info);
    assert_eq!(coalesced_segment(&both), Some(1408));
    // None without it, or with a message that runs past the buffer.
    assert_eq!(coalesced_segment(&[]), None);
    assert_eq!(coalesced_segment(&control_message(0, 19, &[1; 20])), None);
    assert_eq!(coalesced_segment(&info[..info.len() - 8]), None);
    let mut short = info.clone();
    short[0] = 4;
    assert_eq!(coalesced_segment(&short), None, "a length shorter than its header");
    // A zero size is passed on, and leaves the read whole.
    let zero = control_message(17, 3, &0u32.to_ne_bytes());
    assert_eq!(coalesced_segment(&zero), Some(0));
    assert_eq!(Segments::new(2816, coalesced_segment(&zero)).left(), 1);
}

#[test]
fn a_gro_size_is_read_from_its_control_message_among_others() {
    let gro = control_message(17, 104, &1400u32.to_ne_bytes());
    assert_eq!(cmsg_int(&gro, 17, 104), Some(1400));
    // Behind another message, with the level and kind checked.
    let mut both = control_message(0, 19, &[1; 20]);
    both.extend_from_slice(&gro);
    assert_eq!(cmsg_int(&both, 17, 104), Some(1400));
    assert_eq!(cmsg_int(&both, 17, 3), None);
    // None without it, or when it runs past the buffer.
    assert_eq!(cmsg_int(&[], 17, 104), None);
    assert_eq!(cmsg_int(&control_message(0, 19, &[1; 20]), 17, 104), None);
    assert_eq!(cmsg_int(&gro[..gro.len() - 8], 17, 104), None);
}

#[test]
fn a_gro_message_splits_into_its_segments() {
    let mut spans = Vec::new();
    push_segments(&mut spans, 0, 65536, 1500, 3000, Some(1400));
    assert_eq!(
        spans,
        [
            Span { start: 0, wire: 1400, kept: 1400 },
            Span { start: 1400, wire: 1400, kept: 1400 },
            Span { start: 2800, wire: 200, kept: 200 },
        ]
    );
    // Each keeps at most the cut.
    spans.clear();
    push_segments(&mut spans, 0, 65536, 100, 3000, Some(1400));
    assert_eq!(spans.iter().map(|s| s.kept).collect::<Vec<_>>(), [100, 100, 100]);
    assert_eq!(spans.iter().map(|s| s.wire).collect::<Vec<_>>(), [1400, 1400, 200]);
    // No bytes past the slot: the message starts in its own slot.
    spans.clear();
    push_segments(&mut spans, 65536, 65536, 65536, 3000, Some(1400));
    assert_eq!(spans[0].start, 65536);
    assert!(spans.iter().all(|s| s.start + s.kept <= 2 * 65536));
    spans.clear();
    push_segments(&mut spans, 0, 1000, 1500, 3000, Some(1400));
    assert_eq!(spans.iter().map(|s| s.kept).collect::<Vec<_>>(), [1000, 0, 0]);
    // More segments than one take: all 71 of them, in order, the last one short.
    spans.clear();
    push_segments(&mut spans, 0, 65536, 100, 70 * 100 + 30, Some(100));
    assert_eq!(spans.len(), 71);
    assert!(spans.windows(2).all(|w| w[1].start == w[0].start + w[0].wire));
    assert_eq!(spans[70], Span { start: 7000, wire: 30, kept: 30 });
    // Without a size, one datagram of the whole message.
    spans.clear();
    push_segments(&mut spans, 65536, 65536, 65536, 1392, None);
    assert_eq!(spans, [Span { start: 65536, wire: 1392, kept: 1392 }]);
}

#[cfg(windows)]
#[test]
fn a_system_without_coalescing_takes_one_datagram_per_call() {
    let socket = media_socket();
    uro::REFUSED.set(true);
    let mut batch = BatchReceiver::new(&socket, 1500, true);
    uro::REFUSED.set(false);
    assert!(!batch.batched());
    let refusal = batch.take_fallback().expect("the refusal");
    assert_eq!(refusal.raw_os_error(), Some(10042), "WSAENOPROTOOPT");
    let sent = send_numbered(&socket, 5, 1392, 10);
    let (received, counts) = receive_all(&mut batch, &socket, sent.len());
    assert_eq!(received, sent);
    assert_eq!(counts, [1; 5]);
    assert!(batch.take_fallback().is_none(), "reported once");
}

#[cfg(windows)]
#[test]
fn a_roams_new_socket_coalesces_too_or_falls_back_once() {
    let first = media_socket();
    let mut batch = BatchReceiver::new(&first, 1500, true);
    if batch.take_fallback().is_some() {
        return; // This Windows has no coalescing.
    }
    use std::os::windows::io::AsRawSocket;
    let second = media_socket();
    let sent = send_numbered(&second, 3, 1392, 10);
    assert_eq!(receive_all(&mut batch, &second, sent.len()).0, sent);
    let coalesce = batch.coalesce.as_ref().expect("still coalescing");
    assert_eq!(uro::socket_of(coalesce), second.as_raw_socket() as usize);
    // A third that refuses it: reported once, then one datagram per call.
    let third = media_socket();
    uro::REFUSED.set(true);
    let sent = send_numbered(&third, 3, 1392, 10);
    let (received, counts) = receive_all(&mut batch, &third, sent.len());
    uro::REFUSED.set(false);
    assert_eq!(received, sent);
    assert_eq!(counts, [1; 3]);
    assert!(!batch.batched());
    assert_eq!(batch.take_fallback().unwrap().raw_os_error(), Some(10042));
}

#[test]
fn with_the_setting_off_nothing_is_coalesced() {
    let socket = media_socket();
    let mut batch = BatchReceiver::new(&socket, 1500, false);
    assert!(batch.take_fallback().is_none(), "the setting is no refusal");
    assert!(!batch.gro());
    assert!(batch.take_gro_refusal().is_none());
    // Linux and Android still take recvmmsg batches; Windows' batches are coalesced reads.
    assert_eq!(batch.batched(), cfg!(any(target_os = "linux", target_os = "android")));
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawSocket;
        assert_eq!(uro::size_on(socket.as_raw_socket() as usize).unwrap(), 0);
    }
    let sent = send_numbered(&socket, RECEIVE_BATCH as u16 + 3, 1392, 77);
    let (received, counts) = receive_all(&mut batch, &socket, sent.len());
    assert_eq!(received, sent);
    assert!(counts.iter().all(|&count| count <= RECEIVE_BATCH), "{counts:?}");
    if cfg!(windows) {
        assert!(counts.iter().all(|&count| count == 1), "{counts:?}");
    }
}

/// What a network adapter that loses the size of coalesced reads hands over (quinn-rs/quinn
/// #2041): a read longer than any datagram of the stream, without its size. A datagram
/// longer than the reader's maximum is read the same way.
#[cfg(windows)]
#[test]
fn a_read_without_its_size_turns_coalescing_off_for_the_stream() {
    use std::os::windows::io::AsRawSocket;
    let socket = media_socket();
    let raw = socket.as_raw_socket() as usize;
    let mut batch = BatchReceiver::new(&socket, 100, true);
    if batch.take_fallback().is_some() {
        return; // This Windows has no coalescing.
    }
    assert!(batch.batched());
    assert_eq!(uro::size_on(raw).unwrap(), 100 * RECEIVE_BATCH as u32);
    // Within the maximum, coalescing stays on.
    let sent = send_numbered(&socket, 3, 100, 60);
    assert_eq!(receive_all(&mut batch, &socket, sent.len()).0, sent);
    assert!(batch.take_fallback().is_none());
    // Past it: passed on whole, as before, and coalescing goes off on the socket.
    let sent = send_numbered(&socket, 1, 1000, 1000);
    assert_eq!(receive_all(&mut batch, &socket, 1).0, sent);
    let reason = batch.take_fallback().expect("coalescing went off");
    assert!(reason.to_string().contains("off for this stream"), "{reason}");
    assert!(reason.to_string().contains("a 1000-byte read"), "{reason}");
    assert!(!batch.batched());
    assert_eq!(uro::size_on(raw).unwrap(), 0);
    // The stream goes on one datagram per read, and another long read is not reported.
    let sent = send_numbered(&socket, 5, 90, 1000);
    let (received, counts) = receive_all(&mut batch, &socket, sent.len());
    assert_eq!(received, sent);
    assert_eq!(counts, [1; 5]);
    assert!(batch.take_fallback().is_none(), "reported once");
    // A roam's new socket stays without it.
    let second = media_socket();
    let sent = send_numbered(&second, 3, 90, 90);
    assert_eq!(receive_all(&mut batch, &second, sent.len()).0, sent);
    assert_eq!(uro::size_on(second.as_raw_socket() as usize).unwrap(), 0);
    assert!(!batch.batched());
    assert!(batch.take_fallback().is_none());
}

#[test]
fn a_lost_size_says_what_the_adapter_did_and_whether_coalescing_went_off() {
    let off = unsized_read(Ok(()), 2848, None, 1424);
    assert_eq!(
        off.to_string(),
        "receive coalescing is off for this stream, a 2848-byte read of datagrams of at \
             most 1424 bytes came with no datagram size: this network adapter loses the size of \
             coalesced reads"
    );
    let refused = io::Error::from(io::ErrorKind::PermissionDenied);
    let failed = unsized_read(Err(refused), 2848, Some(2848), 1424);
    assert_eq!(failed.kind(), io::ErrorKind::PermissionDenied);
    assert!(failed.to_string().starts_with("turning receive coalescing off failed ("));
    assert!(failed.to_string().contains("came with a datagram size of 2848"));
}

/// Whether this Windows coalesces loopback datagrams, and that a coalesced stream comes
/// back as it was sent. Loopback coalesces on some Windows versions only, so it reports
/// what it saw rather than requiring it:
///
/// `cargo test -p nightjar-core --lib coalescing_on_loopback -- --ignored --nocapture`
#[cfg(windows)]
#[test]
#[ignore = "reports whether loopback coalesces on this Windows"]
fn coalescing_on_loopback_splits_back_into_the_sent_datagrams() {
    let local = (Ipv4Addr::LOCALHOST, 0).into();
    let socket = bind_udp(local, 8 << 20, Qos::Video, false, |_| {}).unwrap();
    let mut batch = BatchReceiver::new(&socket, 1408, true);
    if let Some(e) = batch.take_fallback() {
        println!("coalescing refused: {e}");
        return;
    }
    let mut coalesced = 0;
    let mut calls = 0;
    // Several rounds of bursts, each with a short last datagram, as frames end.
    for round in 0..20 {
        let sent = send_numbered(&socket, 200, 1408, 100 + round);
        let (received, counts) = receive_all(&mut batch, &socket, sent.len());
        assert_eq!(received, sent, "round {round}");
        assert!(counts.iter().all(|&count| count <= RECEIVE_BATCH), "{counts:?}");
        coalesced += counts.iter().filter(|&&count| count > 1).count();
        calls += counts.len();
    }
    assert_eq!(batch.receive(&socket).unwrap(), 0, "then the poll timeout");
    println!(
        "coalescing on; {coalesced} of {calls} receives took more than one datagram ({})",
        if coalesced > 0 {
            "loopback coalesces"
        } else {
            "loopback does not coalesce here"
        }
    );
}
