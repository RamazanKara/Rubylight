use super::*;
use windows::Win32::NetworkManagement::{IpHelper::*, Ndis::*};

#[test]
fn a_loopback_connection_belongs_to_the_process_that_opened_it() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (_accepted, _) = listener.accept().unwrap();
    let v4 = |address: SocketAddr| match address {
        SocketAddr::V4(v4) => v4,
        SocketAddr::V6(v6) => panic!("{v6} is not IPv4"),
    };
    let (local, remote) = (
        v4(client.local_addr().unwrap()),
        v4(listener.local_addr().unwrap()),
    );
    assert_eq!(tcp_owner(local, remote).unwrap(), Some(std::process::id()));
    // The reverse direction is the accepted end's, also ours; a made-up one is no one's.
    assert_eq!(tcp_owner(remote, local).unwrap(), Some(std::process::id()));
    let elsewhere = std::net::SocketAddrV4::new(*local.ip(), 1);
    assert_eq!(tcp_owner(elsewhere, remote).unwrap(), None);
}

#[test]
fn a_refused_datagram_or_segmented_batch_retries_once_after_buffer_pressure() {
    // Both the audio/datagram path and WSASendMsg use this operation. A
    // refused batch has not been submitted, so retrying it cannot duplicate
    // any successful datagrams.
    for bytes in [128, 44 * 1456] {
        for error in [WSAEWOULDBLOCK, WSAENOBUFS] {
            let mut sends = 0;
            let mut waits = 0;
            let result = send_with_retry(
                || {
                    sends += 1;
                    if sends == 1 { Err(error) } else { Ok(bytes) }
                },
                |reported| {
                    assert_eq!(reported, error);
                    waits += 1;
                    true
                },
            );
            assert_eq!(result, Ok(bytes));
            assert_eq!((sends, waits), (2, 1));
        }
    }
    assert_eq!(
        send_with_retry(|| Ok(128), |_| panic!("a successful send must not wait")),
        Ok(128)
    );
}

#[test]
fn persistent_pressure_and_an_expired_budget_do_not_loop_or_hide_errors() {
    for can_wait in [true, false] {
        let mut sends = 0;
        let mut waits = 0;
        let result: std::result::Result<usize, _> = send_with_retry(
            || {
                sends += 1;
                Err(WSAENOBUFS)
            },
            |_| {
                waits += 1;
                can_wait
            },
        );
        assert_eq!(result, Err(WSAENOBUFS));
        assert_eq!(sends, if can_wait { 2 } else { 1 });
        assert_eq!(waits, 1);
    }
    for error in [WSAEHOSTUNREACH, WSAECONNRESET, WSAEINVAL, WSAENOTSOCK] {
        let result: std::result::Result<usize, _> = send_with_retry(
            || Err(error),
            |_| panic!("only buffer pressure permits a retry"),
        );
        assert_eq!(result, Err(error));
    }
}

#[test]
fn buffer_backoff_consumes_the_same_batch_budget_as_writable_waits() {
    let start = std::time::Instant::now();
    let mut now = start;
    let mut budget = WaitBudget::default();
    let mut retries = 0;
    for _ in 0..64 {
        if let Some(deadline) = budget.backoff_at(now) {
            assert!(deadline > now);
            now = deadline;
            retries += 1;
        }
    }
    assert_eq!(retries, 3);
    assert_eq!(now - start, std::time::Duration::from_millis(3));
    assert_eq!(budget.take(now), 1);
    assert_eq!(budget.take(start + WRITABLE_WAIT), 0);
    assert!(budget.backoff_at(start + WRITABLE_WAIT).is_none());

    // Earlier socket polling also spends the backoff budget. A late timer
    // wake leaves no new retry allowance for later packets in that batch.
    let mut budget = WaitBudget::default();
    assert_eq!(budget.take(start), 4);
    assert!(
        budget
            .backoff_at(start + WRITABLE_WAIT - BUFFER_RETRY_DELAY)
            .is_none()
    );
    assert!(budget.backoff_at(start + WRITABLE_WAIT * 2).is_none());
}

#[test]
fn no_buffer_space_waits_even_when_the_socket_is_already_writable() -> Result<()> {
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    assert!(writable(&socket, &mut WaitBudget::default(), None));
    let mut budget = WaitBudget::default();
    let mut waits = WritableWaits::default();
    let retried = buffer_available(&socket, WSAENOBUFS, &mut budget, Some(&mut waits));
    assert_eq!(waits.count, 1);
    assert!(waits.elapsed >= BUFFER_RETRY_DELAY);
    // A busy test machine may wake after the shared deadline; it must then
    // refuse the retry rather than restart the budget.
    assert_eq!(retried, waits.elapsed < WRITABLE_WAIT);
    let mut spent = WaitBudget {
        deadline: Some(std::time::Instant::now()),
        ..Default::default()
    };
    assert!(!buffer_available(
        &socket,
        WSAENOBUFS,
        &mut spent,
        Some(&mut waits)
    ));
    assert_eq!(waits.count, 1);
    assert!(spent.backoff_timer.is_none());
    Ok(())
}

#[test]
fn writable_waits_are_counted_only_when_traced() -> Result<()> {
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    assert!(Batch::default().waits.is_none());
    assert!(writable(&socket, &mut WaitBudget::default(), None));
    let mut waits = WritableWaits::default();
    let started = std::time::Instant::now();
    assert!(writable(
        &socket,
        &mut WaitBudget::default(),
        Some(&mut waits)
    ));
    assert!(writable(
        &socket,
        &mut WaitBudget::default(),
        Some(&mut waits)
    ));
    assert_eq!(waits.count, 2);
    assert!(waits.elapsed <= started.elapsed());
    Ok(())
}
#[test]
fn a_batch_shares_one_writable_wait_instead_of_one_per_datagram() -> Result<()> {
    let start = std::time::Instant::now();
    let ms = std::time::Duration::from_micros;
    let mut budget = WaitBudget::default();
    assert_eq!(budget.take(start), 4);
    assert_eq!(budget.take(start + ms(2_500)), 2);
    assert_eq!(budget.take(start + ms(3_999)), 1);
    assert_eq!(budget.take(start + ms(4_000)), 0);
    assert_eq!(budget.take(start + ms(250_000)), 0);
    // A spent budget refuses at once, without polling the socket.
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    let mut spent = WaitBudget {
        deadline: Some(std::time::Instant::now()),
        ..Default::default()
    };
    let mut waits = WritableWaits::default();
    assert!(!writable(&socket, &mut spent, Some(&mut waits)));
    assert_eq!(waits.count, 0);
    Ok(())
}

fn interface(kind: u32, hardware: bool) -> MIB_IF_ROW2 {
    let mut row = MIB_IF_ROW2 {
        Type: kind,
        OperStatus: IfOperStatusUp,
        TransmitLinkSpeed: 1_000_000_000,
        ..Default::default()
    };
    row.InterfaceAndOperStatusFlags._bitfield = u8::from(hardware);
    row
}

#[test]
fn interface_detection_keeps_physical_link_limits_separate_from_pacing_demand() {
    for kind in [
        IF_TYPE_IEEE80211,
        IF_TYPE_IEEE80216_WMAN,
        IF_TYPE_WWANPP,
        IF_TYPE_WWANPP2,
    ] {
        for hardware in [false, true] {
            let link = interface_link(&interface(kind, hardware));
            assert!(link.wireless);
            assert_eq!(link.bps, 0);
            assert_eq!(
                butterpollo_core::network_pacing::rate_bps(
                    0,
                    50_000,
                    link.bps,
                    link.wireless,
                    65_000_000
                ),
                100_000_000
            );
        }
    }
    // vEthernet/TAP, VPN/PPP, Tailscale/tunnel, loopback and unknown.
    for kind in [
        IF_TYPE_ETHERNET_CSMACD,
        IF_TYPE_PPP,
        IF_TYPE_TUNNEL,
        IF_TYPE_SOFTWARE_LOOPBACK,
        IF_TYPE_OTHER,
        0,
    ] {
        let link = interface_link(&interface(kind, false));
        assert_eq!(link, Link::default());
        assert_eq!(
            butterpollo_core::network_pacing::rate_bps(
                0,
                50_000,
                link.bps,
                link.wireless,
                65_000_000
            ),
            800_000_000
        );
    }
    let mut wired = interface(IF_TYPE_ETHERNET_CSMACD, true);
    wired.TransmitLinkSpeed = 100_000_000;
    wired.InterfaceLuid.Value = 0x0006_0000_0000_0001;
    assert_eq!(
        interface_link(&wired),
        Link {
            bps: 100_000_000,
            wireless: false,
            interface: 0x0006_0000_0000_0001,
        }
    );
    wired.OperStatus = IfOperStatusDown;
    assert_eq!(interface_link(&wired), Link::default());
    let mut wifi = interface(IF_TYPE_IEEE80211, true);
    wifi.OperStatus = IfOperStatusDown;
    assert_eq!(interface_link(&wifi), Link::default());
}

#[test]
fn virtual_switch_uses_only_an_unambiguous_physical_binding() {
    let stack = [
        MIB_IFSTACK_ROW {
            HigherLayerInterfaceIndex: 7,
            LowerLayerInterfaceIndex: 8,
        },
        MIB_IFSTACK_ROW {
            HigherLayerInterfaceIndex: 8,
            LowerLayerInterfaceIndex: 9,
        },
    ];
    for kind in [IF_TYPE_ETHERNET_CSMACD, IF_TYPE_IEEE80211] {
        let physical = interface(kind, true);
        assert_eq!(
            underlying_link(7, &stack, |index| Some(if index == 9 {
                physical
            } else {
                interface(IF_TYPE_ETHERNET_CSMACD, false)
            })),
            interface_link(&physical)
        );
    }
    assert_eq!(
        underlying_link(7, &[], |_| panic!("no binding")),
        Link::default()
    );
    assert_eq!(underlying_link(7, &stack, |_| None), Link::default());
    let ambiguous = [
        stack[0],
        MIB_IFSTACK_ROW {
            HigherLayerInterfaceIndex: 7,
            LowerLayerInterfaceIndex: 9,
        },
    ];
    assert_eq!(
        underlying_link(7, &ambiguous, |_| panic!("ambiguous binding")),
        Link::default()
    );
    let cycle = [
        stack[0],
        MIB_IFSTACK_ROW {
            HigherLayerInterfaceIndex: 8,
            LowerLayerInterfaceIndex: 7,
        },
    ];
    assert_eq!(
        underlying_link(7, &cycle, |_| Some(interface(
            IF_TYPE_ETHERNET_CSMACD,
            false
        ))),
        Link::default()
    );
    let mut down = interface(IF_TYPE_IEEE80211, true);
    down.OperStatus = IfOperStatusDown;
    assert_eq!(underlying_link(7, &stack, |_| Some(down)), Link::default());
}

#[test]
fn loopback_including_mapped_ipv4_keeps_fast_pacing_without_a_link_cap() {
    for peer in ["127.0.0.1:9", "[::1]:9", "[::ffff:127.0.0.1]:9"] {
        let link = routed_link(peer.parse().unwrap());
        assert_eq!(link, Link::default());
        assert_eq!(
            butterpollo_core::network_pacing::rate_bps(
                0,
                50_000,
                link.bps,
                link.wireless,
                65_000_000
            ),
            800_000_000
        );
    }
}

#[test]
#[ignore = "requires a configured native network adapter and route"]
fn native_local_mac_matches_mapped_ipv6_and_preserves_loopback() -> Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    // UDP connect selects an interface without sending any packet.
    socket.connect("192.0.2.1:9")?;
    let address = socket.local_addr()?.ip();
    let mac = local_mac(address)?;
    anyhow::ensure!(
        mac != "00:00:00:00:00:00",
        "selected interface has no Ethernet MAC"
    );
    if let std::net::IpAddr::V4(ip) = address {
        assert_eq!(local_mac(ip.to_ipv6_mapped().into())?, mac);
    }
    assert_eq!(local_mac("127.0.0.1".parse()?)?, "00:00:00:00:00:00");
    println!("read-only network identity: {address} / {mac}");
    Ok(())
}
#[test]
fn local_mac_never_reports_a_virtual_adapter() -> Result<()> {
    // Before, a request on WSL's or Hyper-V's vEthernet reported its MAC,
    // which no magic packet wakes, and a Tailscale one reported zeros.
    let adapters = crate::wake::adapters()?;
    let physical: Vec<_> = adapters
        .iter()
        .filter(|a| a.physical())
        .map(|a| butterpollo_core::wake::format_mac(a.mac))
        .collect();
    for address in adapters.iter().flat_map(|a| a.addresses.clone()) {
        let mac = local_mac(address)?;
        assert!(
            mac == "00:00:00:00:00:00" || physical.contains(&mac),
            "{address} reported {mac}, not a physical adapter's"
        );
        if !address.is_loopback() && adapters.iter().any(|a| a.physical() && a.connected) {
            assert_ne!(mac, "00:00:00:00:00:00", "{address}");
        }
    }
    Ok(())
}
#[test]
fn waiting_for_input_wakes_when_a_datagram_arrives() -> Result<()> {
    use std::os::windows::io::AsRawSocket;
    let receiver = UdpSocket::bind("127.0.0.1:0")?;
    let sender = UdpSocket::bind("127.0.0.1:0")?;
    assert!(!wait_readable(receiver.as_raw_socket(), 0)?);
    sender.send_to(b"input", receiver.local_addr()?)?;
    let waited = std::time::Instant::now();
    assert!(wait_readable(receiver.as_raw_socket(), 1000)?);
    assert!(waited.elapsed() < std::time::Duration::from_millis(500));
    Ok(())
}
#[test]
fn control_wait_wakes_for_feedback_without_client_input() -> Result<()> {
    let control = UdpSocket::bind("127.0.0.1:0")?;
    let feedback = UdpSocket::bind("127.0.0.1:0")?;
    let sender = UdpSocket::bind("127.0.0.1:0")?;
    let wait =
        |timeout| wait_readable_or_wake(control.as_raw_socket(), feedback.as_raw_socket(), timeout);
    assert!(!wait(0)?);
    let address = feedback.local_addr()?;
    let thread = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(20));
        sender.send_to(&[1], address)
    });
    let started = std::time::Instant::now();
    assert!(wait(1000)?);
    assert!(started.elapsed() < std::time::Duration::from_millis(500));
    thread.join().unwrap()?;
    feedback.recv(&mut [0])?;
    assert!(!wait(0)?);
    let sender = UdpSocket::bind("127.0.0.1:0")?;
    sender.send_to(&[1], control.local_addr()?)?;
    assert!(wait(1000)?);
    Ok(())
}
#[test]
fn native_udp_batches_preserve_datagrams_destinations_and_short_tail() -> Result<()> {
    for host in ["127.0.0.1:0", "[::1]:0"] {
        let sender = ReplySocket::bind(host)?;
        let receiver = UdpSocket::bind(host)?;
        receiver.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
        let mut packets: Vec<_> = (0..7).map(|i| vec![i; 512]).collect();
        packets.push(vec![91; 13]);
        assert_eq!(Batch::count(&packets, 16384), 7);
        let mut batch = Batch::default();
        assert_eq!(
            batch.send(&sender, &packets[..7], receiver.local_addr()?)?,
            7 * 512
        );
        batch.send(&sender, &packets[7..], receiver.local_addr()?)?;
        for expected in &packets {
            let mut buffer = [0u8; 2048];
            let (size, peer) = receiver.recv_from(&mut buffer)?;
            assert_eq!(&buffer[..size], expected);
            assert_eq!(peer, sender.local_addr()?);
        }
        println!(
            "{host}: {} datagrams, {} send calls",
            packets.len(),
            batch.system_calls
        );
        // Verify the compatibility path explicitly on the same socket.
        batch.offload = false;
        batch.send(&sender, &packets[..2], receiver.local_addr()?)?;
        for expected in &packets[..2] {
            let mut buffer = [0u8; 512];
            let n = receiver.recv(&mut buffer)?;
            assert_eq!(&buffer[..n], expected);
        }
    }
    Ok(())
}
#[test]
fn a_segment_run_is_equal_sizes_then_at_most_one_shorter() {
    let run = |sizes: &[usize]| {
        let packets: Vec<Vec<u8>> = sizes.iter().map(|&n| vec![0; n]).collect();
        Segments::run(&packets)
    };
    assert_eq!(run(&[]), 0);
    assert_eq!(run(&[300]), 1);
    assert_eq!(run(&[1200, 1200, 1200]), 3);
    // A shorter datagram ends the run; a longer one starts the next.
    assert_eq!(run(&[1200, 1200, 700, 1200]), 3);
    assert_eq!(run(&[100, 1200, 1200]), 1);
    assert_eq!(run(&[1200, 1300]), 1);
    // At most 64 datagrams and 65507 bytes.
    assert_eq!(run(&[100; 70]), 64);
    assert_eq!(run(&[1200; 64]), 54);
    assert_eq!(run(&[65507, 1]), 1);
    // Segmentation cannot send an empty datagram.
    assert_eq!(run(&[0, 0]), 1);
    assert_eq!(run(&[1200, 1200, 0]), 2);
}
#[test]
fn segments_keep_mixed_datagrams_in_order_in_one_write_per_run() -> Result<()> {
    for host in ["127.0.0.1:0", "[::1]:0"] {
        let sender = UdpSocket::bind(host)?;
        let receiver = UdpSocket::bind(host)?;
        receiver.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
        let sizes = [90, 1200, 1200, 1200, 1200, 350, 1100, 1100, 160, 1];
        let packets: Vec<Vec<u8>> = sizes
            .iter()
            .enumerate()
            .map(|(i, &n)| (0..n).map(|b| (i * 31 + b) as u8).collect())
            .collect();
        let peer = receiver.local_addr()?;
        let mut segments = Segments::default();
        let sent = segments.send(&sender, &packets, peer);
        assert_eq!((sent.datagrams, sent.failed), (sizes.len(), 0), "{host}");
        assert_eq!(sent.bytes, sizes.iter().sum::<usize>());
        for expected in &packets {
            let mut buffer = [0u8; 2048];
            let (size, from) = receiver.recv_from(&mut buffer)?;
            assert_eq!(&buffer[..size], expected, "{host}");
            assert_eq!(from, sender.local_addr()?);
        }
        // 90; the four 1200s and their 350; the two 1100s and their 160; 1.
        assert!(segments.offload, "{host}: segmentation stays on");
        assert_eq!(segments.system_calls, 4, "{host}");
        // Without segmentation, one datagram per write, still in order.
        segments.offload = false;
        let sent = segments.send(&sender, &packets[1..6], peer);
        assert_eq!(sent.datagrams, 5);
        assert_eq!(segments.system_calls, 9, "{host}");
        for expected in &packets[1..6] {
            let mut buffer = [0u8; 2048];
            let size = receiver.recv(&mut buffer)?;
            assert_eq!(&buffer[..size], expected, "{host}");
        }
    }
    Ok(())
}
#[test]
#[ignore = "timing: cargo test -p butterpollo-windows --lib segment_burst_timing -- --ignored --nocapture"]
fn segment_burst_timing() -> Result<()> {
    // A frame's burst over loopback, segmented against one send_to per datagram, alternating:
    // how long the writes take and when the burst's last datagram reaches the receiver.
    use std::os::windows::io::AsRawSocket;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    use windows::Win32::Networking::WinSock::{SO_RCVBUF, SOCKET, SOL_SOCKET, setsockopt};
    let sender = UdpSocket::bind("127.0.0.1:0")?;
    let receiver = UdpSocket::bind("127.0.0.1:0")?;
    let buffer = (16i32 << 20).to_ne_bytes();
    let socket = SOCKET(receiver.as_raw_socket() as usize);
    // SAFETY: the socket is open for the call and the option value is a valid i32 buffer.
    let set = unsafe { setsockopt(socket, SOL_SOCKET, SO_RCVBUF, Some(&buffer)) };
    assert_eq!(set, 0);
    let peer = receiver.local_addr()?;
    let (arrived, last) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = [0u8; 2048];
        let mut count = 0usize;
        while let Ok(size) = receiver.recv(&mut buffer) {
            count += 1;
            if size > 0 && buffer[0] == 1 {
                let _ = arrived.send((Instant::now(), count));
                count = 0;
            }
        }
    });
    let percentile = |values: &mut Vec<f64>, p: f64| {
        values.sort_by(f64::total_cmp);
        values[((values.len() - 1) as f64 * p).round() as usize]
    };
    // A 4K60 P frame at 23 Mbps (48 KB) and a 4K key frame (300 KB), as 1200-byte RTP packets.
    for (name, count) in [("p-frame", 41usize), ("key-frame", 251)] {
        let mut packets = vec![vec![0u8; 1200]; count];
        packets[count - 1] = vec![1u8; 700];
        let mut results: [(Vec<f64>, Vec<f64>); 2] = Default::default();
        let mut segments = Segments::default();
        for trial in 0..4000 {
            let offload = trial % 2 == 0;
            segments.offload = offload;
            let start = Instant::now();
            let sent = segments.send(&sender, &packets, peer);
            let written = start.elapsed();
            let (at, received) = last.recv_timeout(Duration::from_secs(2))?;
            assert_eq!((sent.datagrams, received), (count, count), "{name}");
            assert_eq!(segments.offload, offload, "segmentation stays on");
            let result = &mut results[usize::from(offload)];
            result.0.push(written.as_secs_f64() * 1e6);
            result.1.push(at.duration_since(start).as_secs_f64() * 1e6);
            std::thread::sleep(Duration::from_millis(1));
        }
        for (mode, (written, arrived)) in ["send_to", "segmented"].iter().zip(&mut results) {
            println!(
                "{name:9} {mode:9} write us p50 {:7.1} p99 {:7.1}  last arrival us p50 {:7.1} p90 {:7.1} p99 {:7.1}",
                percentile(written, 0.5),
                percentile(written, 0.99),
                percentile(arrived, 0.5),
                percentile(arrived, 0.9),
                percentile(arrived, 0.99),
            );
        }
    }
    Ok(())
}
#[test]
fn segments_count_a_failed_run_as_lost_and_go_on() -> Result<()> {
    // An IPv4 socket cannot reach an IPv6 peer: every datagram fails, none is fatal, and an
    // address error does not turn segmentation off. The failed run is tried again one datagram
    // at a time, so a passing error loses only the datagrams it hits.
    let sender = UdpSocket::bind("127.0.0.1:0")?;
    let packets = vec![vec![7u8; 500]; 5];
    let mut segments = Segments::default();
    let sent = segments.send(&sender, &packets, "[::1]:9".parse()?);
    assert_eq!((sent.datagrams, sent.failed), (0, 5));
    assert!(sent.error.is_some());
    assert!(segments.offload);
    assert_eq!(segments.system_calls, 6);
    Ok(())
}
#[test]
fn qos_leaves_clients_it_cannot_tag_alone() -> Result<()> {
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    assert!(QosFlow::new(&socket, "127.0.0.1:9".parse()?, false)?.is_none());
    let dual = UdpSocket::bind("[::]:0")?;
    assert!(QosFlow::new(&dual, "[::ffff:192.0.2.10]:9".parse()?, true)?.is_none());
    Ok(())
}
#[test]
fn local_address_reads_pktinfo_after_other_control_messages() {
    let mut control = Ancillary::default();
    control.push(IPPROTO_UDP, UDP_SEND_MSG_SIZE, 1200u32);
    control.source([10, 1, 2, 3].into());
    assert_eq!(control.len, 48);
    assert_eq!(
        local_address(&control.bytes[..control.len]),
        Some([10, 1, 2, 3].into())
    );
    let mut control = Ancillary::default();
    let address: IpAddr = "fd7a:115c:a1e0::1".parse().unwrap();
    control.source(address);
    assert_eq!(control.len, 40);
    assert_eq!(local_address(&control.bytes[..control.len]), Some(address));
    // A v4-mapped source is sent, and read back, as IPv4.
    let mut control = Ancillary::default();
    control.source("::ffff:100.64.0.1".parse().unwrap());
    assert_eq!(
        local_address(&control.bytes[..control.len]),
        Some([100, 64, 0, 1].into())
    );
    // Truncated or malformed control data has no address.
    assert_eq!(local_address(&control.bytes[..20]), None);
    assert_eq!(local_address(&[0u8; 32]), None);
}
#[test]
#[ignore = "sends to a documentation address; run under a packet capture to see the marks"]
fn qos_flows_tag_a_network_client() -> Result<()> {
    let socket = ReplySocket::bind("0.0.0.0:0")?;
    for (port, voice) in [(9, false), (10, true)] {
        let peer: SocketAddr = format!("192.0.2.10:{port}").parse()?;
        let flow = QosFlow::new(&socket, peer, voice)?;
        assert!(flow.is_some());
        for _ in 0..5 {
            send_datagram(&socket, b"qos", peer)?;
        }
    }
    Ok(())
}
#[test]
fn transient_send_errors_drop_packets_instead_of_ending_the_stream() -> Result<()> {
    for error in [WSAEWOULDBLOCK, WSAENOBUFS, WSAEHOSTUNREACH, WSAECONNRESET] {
        assert!(transient(error), "{}", error.0);
    }
    for error in [WSAEINVAL, WSAENOTSOCK, WSAEFAULT, WSAEMSGSIZE] {
        assert!(!transient(error), "{}", error.0);
    }
    for code in [WSAENETUNREACH, WSAENETRESET, WSAEMSGSIZE, WSAENOBUFS] {
        assert!(datagram_lost(&std::io::Error::from_raw_os_error(code.0)));
    }
    assert!(!datagram_lost(&std::io::Error::from_raw_os_error(
        WSAENOTSOCK.0
    )));
    // Without SIO_UDP_CONNRESET disabled, the ICMP reply from a closed
    // port turns later sends into WSAECONNRESET on Windows.
    let sender = ReplySocket::bind("127.0.0.1:0")?;
    let closed = UdpSocket::bind("127.0.0.1:0")?.local_addr()?;
    let packets = vec![vec![7u8; 256]; 4];
    let mut batch = Batch::default();
    for _ in 0..20 {
        send_datagram(&sender, &packets[0], closed)?;
        batch.send(&sender, &packets, closed)?;
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    configure_udp(&sender)?;
    assert!(socket2::SockRef::from(&sender).send_buffer_size()? >= 1 << 20);
    Ok(())
}
/// The send path's own cost on loopback, per call: a 64-datagram video
/// batch and one audio datagram, to a peer never heard from ("routed"),
/// one Windows routes from the address it reached ("matched", a LAN
/// client) and one it does not ("pinned"). Run with --release.
#[test]
#[ignore = "timing; run with --release --ignored --nocapture"]
fn send_path_cost() -> Result<()> {
    let receiver = UdpSocket::bind("127.0.0.1:0")?;
    socket2::SockRef::from(&receiver).set_recv_buffer_size(64 << 20)?;
    receiver.set_read_timeout(Some(std::time::Duration::from_millis(200)))?;
    let peer = receiver.local_addr()?;
    let drain = std::thread::spawn(move || {
        let mut buffer = [0u8; 2048];
        while receiver.recv(&mut buffer).is_ok() {}
    });
    let video = vec![vec![7u8; 1200]; 64];
    let audio = vec![9u8; 400];
    // "before" makes the calls 848095f08 made, without a ReplySocket
    // lookup.
    let modes = ["before", "routed", "matched", "pinned"];
    let mut senders = Vec::new();
    for mode in modes {
        let mut sender = ReplySocket::bind("0.0.0.0:0")?;
        configure_udp(&sender)?;
        if mode == "pinned" {
            sender.route = |_, _| Ok(IpAddr::from([192, 0, 2, 9]));
        }
        if mode == "matched" || mode == "pinned" {
            sender.answer_from(peer, Some(peer.ip()));
            assert_eq!(sender.sources_from(peer.ip()), [(peer, peer.ip())]);
        }
        senders.push((mode, sender, Batch::default(), Vec::new(), Vec::new()));
    }
    // Short turns in a rotating order: this PC's send cost changes over
    // seconds, so each mode sees the same mix.
    for round in 0..40 {
        for i in 0..modes.len() {
            let (mode, sender, batch, batches, datagrams) = &mut senders[(i + round) % modes.len()];
            let started = std::time::Instant::now();
            for _ in 0..200 {
                if *mode == "before" {
                    let mut budget = WaitBudget::default();
                    batch
                        .send_segmented(sender, &video, peer, None, &mut budget)?
                        .unwrap();
                } else {
                    batch.send(sender, &video, peer)?;
                }
            }
            batches.push(started.elapsed().as_nanos() / 200);
            let started = std::time::Instant::now();
            for _ in 0..2000 {
                if *mode == "before" {
                    send_one(sender, &audio, peer, None, &mut WaitBudget::default(), None)?
                        .unwrap();
                } else {
                    send_datagram(sender, &audio, peer)?;
                }
            }
            datagrams.push(started.elapsed().as_nanos() / 2000);
        }
    }
    for (mode, sender, batch, batches, datagrams) in &mut senders {
        // A refused pin would have measured the unpinned path.
        assert_eq!(
            sender.source(peer),
            (*mode == "pinned").then_some(peer.ip()),
            "{mode}"
        );
        assert_eq!(batch.dropped, 0, "{mode}");
        assert!(batch.offload, "{mode}");
        let mean = |values: &[u128]| values.iter().sum::<u128>() / values.len() as u128;
        batches.sort();
        datagrams.sort();
        println!(
            "{mode}: batch mean {} ns, median {} ns; datagram mean {} ns, median {} ns",
            mean(batches),
            batches[batches.len() / 2],
            mean(datagrams),
            datagrams[datagrams.len() / 2]
        );
    }
    drop(senders);
    drain.join().unwrap();
    Ok(())
}
/// The receive path's own cost on loopback, per datagram, as the control
/// thread pays it before input is applied: "before" is recv_from,
/// "settled" WSARecvMsg and answer_from for a peer checked within
/// ROUTE_CHECK, "checked" the same with Windows' route asked every time,
/// as happens once a second per peer. Run with --release.
#[test]
#[ignore = "timing; run with --release --ignored --nocapture"]
fn receive_path_cost() -> Result<()> {
    let modes = ["before", "settled", "checked"];
    let sender = UdpSocket::bind("127.0.0.1:0")?;
    let datagram = [5u8; 64];
    let mut receivers = Vec::new();
    for mode in modes {
        let mut receiver = ReplySocket::bind("0.0.0.0:0")?;
        configure_udp(&receiver)?;
        if mode == "checked" {
            // Every lookup is a route check: the clock jumps past ROUTE_CHECK.
            static TICKS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            static BASE: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
            receiver.now = || {
                *BASE.get_or_init(std::time::Instant::now)
                    + ROUTE_CHECK * TICKS.fetch_add(2, Ordering::Relaxed) as u32
            };
        }
        let port = receiver.local_addr()?.port();
        let target = SocketAddr::from(([127, 0, 0, 1], port));
        receivers.push((mode, receiver, target, Vec::new()));
    }
    let mut buffer = [0u8; 2048];
    for round in 0..40 {
        for i in 0..modes.len() {
            let (mode, receiver, target, costs) = &mut receivers[(i + round) % modes.len()];
            let mut elapsed = std::time::Duration::ZERO;
            for _ in 0..500 {
                sender.send_to(&datagram, *target)?;
                while !wait_readable(receiver.as_raw_socket(), 100)? {}
                let started = std::time::Instant::now();
                if *mode == "before" {
                    receiver.socket.recv_from(&mut buffer)?;
                } else {
                    let (_, peer, local) = receiver.receive(&mut buffer)?;
                    assert!(local.is_some(), "{mode}");
                    receiver.answer_from(peer, local);
                }
                elapsed += started.elapsed();
            }
            costs.push(elapsed.as_nanos() / 500);
        }
    }
    for (mode, _, _, costs) in &mut receivers {
        costs.sort();
        println!(
            "{mode}: datagram mean {} ns, median {} ns, max turn {} ns",
            costs.iter().sum::<u128>() / costs.len() as u128,
            costs[costs.len() / 2],
            costs[costs.len() - 1]
        );
    }
    Ok(())
}
fn dual_stack() -> std::io::Result<UdpSocket> {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV6,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;
    socket.set_only_v6(false)?;
    socket.bind(&"[::]:0".parse::<SocketAddr>().unwrap().into())?;
    Ok(socket.into())
}
/// Sends `host`'s replies to `peer` on every path, each freshly pinned to
/// `pin`: control, audio, a segmented video batch and video datagrams,
/// calling `sent` after each. Returns the senders they came from at
/// `client`.
fn reply_sources(
    host: &ReplySocket,
    client: &UdpSocket,
    peer: SocketAddr,
    pin: Option<IpAddr>,
    sent: &dyn Fn(&str),
) -> Result<Vec<IpAddr>> {
    use anyhow::Context;
    let video: Vec<_> = (0..3).map(|i| vec![i; 256]).collect();
    let mut expected = Vec::new();
    let pin = || {
        host.forget(peer);
        host.answer_from(peer, pin);
    };
    pin();
    host.send_to(b"control", peer).context("control")?;
    sent("control");
    expected.push(b"control".to_vec());
    pin();
    assert!(send_datagram(host, b"audio", peer).context("audio")?);
    sent("audio");
    expected.push(b"audio".to_vec());
    let mut batch = Batch::default();
    pin();
    assert_eq!(
        batch.send(host, &video, peer).context("segmented")?,
        3 * 256
    );
    sent("segmented");
    assert!(batch.offload, "segmentation stays on");
    expected.extend(video.iter().cloned());
    batch.offload = false;
    pin();
    assert_eq!(
        batch.send(host, &video, peer).context("datagrams")?,
        3 * 256
    );
    sent("datagrams");
    expected.extend(video.iter().cloned());
    let mut sources = Vec::new();
    let mut buffer = [0u8; 2048];
    for expected in expected {
        let (size, from) = client.recv_from(&mut buffer)?;
        assert_eq!(&buffer[..size], expected);
        sources.push(from.ip().to_canonical());
    }
    Ok(sources)
}
/// Issue #54: a host on every address answers each client from the
/// address the client reached, not the one Windows routes by. To one of
/// its own addresses, Windows replies from that address, so a client on
/// one address of this PC that reached another sees the wrong source
/// without pinning. Skipped without two such addresses of one family.
#[test]
fn replies_leave_from_the_address_the_client_reached() -> Result<()> {
    let addresses = unicast_addresses()?;
    let mut tested = Vec::new();
    type Bind = fn() -> std::io::Result<UdpSocket>;
    let hosts: [(&str, Bind, bool); 3] = [
        ("IPv4", || UdpSocket::bind("0.0.0.0:0"), false),
        ("dual-stack IPv4", dual_stack, false),
        ("IPv6", || UdpSocket::bind("[::]:0"), true),
    ];
    for (name, bind, v6) in hosts {
        let same: Vec<_> = addresses.iter().filter(|a| a.is_ipv6() == v6).collect();
        'pairs: for client_ip in &same {
            for reached in &same {
                if client_ip == reached {
                    continue;
                }
                let host = ReplySocket::new(bind()?)?;
                let Ok(client) = UdpSocket::bind((**client_ip, 0)) else {
                    continue;
                };
                host.set_read_timeout(Some(std::time::Duration::from_millis(300)))?;
                client.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
                let target = SocketAddr::new(**reached, host.local_addr()?.port());
                if client.send_to(b"ping", target).is_err() {
                    continue;
                }
                let mut buffer = [0u8; 64];
                let Ok((size, peer, local)) = host.receive(&mut buffer) else {
                    continue;
                };
                assert_eq!(&buffer[..size], b"ping", "{name}");
                assert_eq!(local, Some(**reached), "{name}");
                host.answer_from(peer, local);
                assert_eq!(host.sources_from(peer.ip()), [(peer, **reached)], "{name}");
                let sources = reply_sources(&host, &client, peer, local, &|_| {})?;
                assert!(
                    sources.iter().all(|source| source == *reached),
                    "{name}: {client_ip} reached {reached}; replies came from {sources:?}"
                );
                tested.push(name);
                break 'pairs;
            }
        }
    }
    println!("tested {tested:?} among {addresses:?}");
    if tested.len() < hosts.len() {
        println!(
            "skipped {:?}: no two addresses of one family reach each other",
            hosts
                .iter()
                .map(|(name, ..)| *name)
                .filter(|name| !tested.contains(name))
                .collect::<Vec<_>>()
        );
    }
    Ok(())
}
/// A source Windows refuses, such as an address a VPN took away, is
/// dropped: the reply leaves from the routed address, segmentation stays
/// on, and the client's next datagram does not pin it again within
/// REFUSAL. The pin is a documentation address this PC does not have,
/// which Windows always refuses, so this also shows that every path
/// sends from the pinned address. Not 127.0.0.2: for minutes after any
/// process sent from it, such as udp_shaper, Windows's path cache lets
/// every socket send from it.
#[test]
fn a_refused_source_is_forgotten_and_the_reply_still_sent() -> Result<()> {
    let gone: IpAddr = [192, 0, 2, 2].into();
    assert!(!unicast_addresses()?.contains(&gone), "this PC has {gone}");
    let gone = Some(gone);
    type Bind = fn() -> std::io::Result<UdpSocket>;
    let hosts: [(&str, Bind); 2] = [
        ("IPv4", || UdpSocket::bind("0.0.0.0:0")),
        ("dual-stack IPv4", dual_stack),
    ];
    for (name, bind) in hosts {
        let mut host = ReplySocket::new(bind()?)?;
        fake_clock(&mut host);
        let client = UdpSocket::bind("127.0.0.1:0")?;
        host.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
        client.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
        let loopback: IpAddr = [127, 0, 0, 1].into();
        client.send_to(b"ping", (loopback, host.local_addr()?.port()))?;
        let mut buffer = [0u8; 64];
        let (_, peer, local) = host.receive(&mut buffer)?;
        assert_eq!(local, Some(loopback), "{name}");
        host.answer_from(peer, gone);
        assert_eq!(host.source(peer), gone, "{name}: pinned");
        // Each path tried the pin, and forgot it when refused.
        let sources = reply_sources(&host, &client, peer, gone, &|path| {
            assert_eq!(
                host.source(peer),
                None,
                "{name}: {path} never tried the pin"
            )
        })?;
        assert_eq!(sources, vec![loopback; sources.len()], "{name}");
        host.answer_from(peer, gone);
        assert_eq!(host.source(peer), None, "{name}: pinned a refused source");
        host.forget(peer);
        host.answer_from(peer, gone);
        assert_eq!(host.source(peer), gone, "{name}: forgetting the peer");
    }
    Ok(())
}
/// The one loopback source Windows sends from stays pinned on every
/// path, so Windows accepted each path's pktinfo message: IPv4, a
/// dual-stack socket answering a v4-mapped peer, and IPv6. The route is
/// faked to leave from elsewhere, or loopback would not be pinned.
#[test]
fn every_path_keeps_an_accepted_pin() -> Result<()> {
    loopback_replies(Some(|_, _| Ok(IpAddr::from([192, 0, 2, 9]))))
}
/// A client Windows already routes replies to from the address it
/// reached, as on a plain LAN, is answered as before the fix: unpinned,
/// on every path, with Windows's real route lookup.
#[test]
fn a_client_routed_from_the_address_it_reached_is_not_pinned() -> Result<()> {
    loopback_replies(None)
}
/// Replies on every path to loopback clients, with `route` faked or
/// Windows's own; pinned exactly when the route is faked.
fn loopback_replies(
    route: Option<fn(SOCKET, SocketAddr) -> std::io::Result<IpAddr>>,
) -> Result<()> {
    type Bind = fn() -> std::io::Result<UdpSocket>;
    let hosts: [(&str, Bind, &str); 3] = [
        ("IPv4", || UdpSocket::bind("0.0.0.0:0"), "127.0.0.1:0"),
        ("dual-stack IPv4", dual_stack, "127.0.0.1:0"),
        ("IPv6", || UdpSocket::bind("[::]:0"), "[::1]:0"),
    ];
    for (name, bind, client) in hosts {
        let mut host = ReplySocket::new(bind()?)?;
        if let Some(route) = route {
            host.route = route;
        }
        let expected = |local: Option<IpAddr>| local.filter(|_| route.is_some());
        let client = UdpSocket::bind(client)?;
        host.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
        client.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
        let reached = client.local_addr()?.ip();
        client.send_to(b"ping", (reached, host.local_addr()?.port()))?;
        let mut buffer = [0u8; 64];
        let (_, peer, local) = host.receive(&mut buffer)?;
        assert_eq!(local, Some(reached), "{name}");
        if name == "dual-stack IPv4" {
            assert!(matches!(peer.ip(), IpAddr::V6(v6) if v6.to_ipv4_mapped().is_some()));
        }
        let sources = reply_sources(&host, &client, peer, local, &|path| {
            assert_eq!(host.source(peer), expected(local), "{name}: {path}");
            assert_eq!(host.sources_from(peer.ip()), [(peer, reached)], "{name}");
        })?;
        assert_eq!(sources, vec![reached; sources.len()], "{name}");
    }
    Ok(())
}
/// Only an unpinned send that succeeds blames the pin: an interface
/// without a route back is refused once and stays refused for REFUSAL,
/// while a NIC
/// without UDP segmentation or a network that is down keeps the pin.
#[test]
fn only_a_send_the_pin_broke_drops_it() -> Result<()> {
    let mut host = ReplySocket::bind("127.0.0.1:0")?;
    fake_clock(&mut host);
    host.route = |_, _| Ok(IpAddr::from([192, 0, 2, 9]));
    let peer: SocketAddr = "192.0.2.1:9".parse().unwrap();
    let pin: IpAddr = [192, 0, 2, 2].into();
    let pinned = |first: WSA_ERROR, then: Result<(), WSA_ERROR>| {
        host.forget(peer);
        host.answer_from(peer, Some(pin));
        let mut tried = Vec::new();
        let (result, source) = host
            .send_pinned(peer, Some(pin), |source| {
                tried.push(source);
                Ok::<_, std::convert::Infallible>(if source.is_some() { Err(first) } else { then })
            })
            .unwrap();
        (result, source, tried)
    };
    for unreachable in [WSAENETUNREACH, WSAEHOSTUNREACH, WSAEADDRNOTAVAIL, WSAEINVAL] {
        assert_eq!(
            pinned(unreachable, Ok(())),
            (Ok(()), None, vec![Some(pin), None]),
            "{unreachable:?}"
        );
        assert_eq!(host.source(peer), None);
        host.answer_from(peer, Some(pin));
        assert_eq!(host.source(peer), None, "{unreachable:?} pinned again");
    }
    // No segmentation fails both sends with WSAEINVAL; a network that is
    // down fails both too.
    for error in [WSAEINVAL, WSAENETUNREACH] {
        assert_eq!(
            pinned(error, Err(error)),
            (Err(error), Some(pin), vec![Some(pin), None])
        );
        assert_eq!(host.source(peer), Some(pin), "{error:?}");
    }
    // Any other error is the peer's, not the pin's.
    assert_eq!(
        pinned(WSAECONNRESET, Ok(())),
        (Err(WSAECONNRESET), Some(pin), vec![Some(pin)])
    );
    assert_eq!(host.source(peer), Some(pin));
    Ok(())
}
thread_local! {
    /// The time `fake_clock` sockets on this test's thread read.
    static NOW: std::cell::Cell<std::time::Instant> =
        std::cell::Cell::new(std::time::Instant::now());
}
/// Makes `host` read the time from NOW, which only `advance` moves.
fn fake_clock(host: &mut ReplySocket) {
    host.now = || NOW.with(std::cell::Cell::get);
}
fn advance(by: std::time::Duration) {
    NOW.with(|now| now.set(now.get() + by));
}
/// A refused pin lapses after REFUSAL, so a VPN that went down and came
/// back mid-stream is pinned again.
#[test]
fn a_refused_pin_is_taken_again_after_the_refusal_lapses() -> Result<()> {
    let mut host = ReplySocket::bind("127.0.0.1:0")?;
    fake_clock(&mut host);
    host.route = |_, _| Ok(IpAddr::from([192, 0, 2, 9]));
    let peer: SocketAddr = "192.0.2.1:9".parse().unwrap();
    let pin: IpAddr = [192, 0, 2, 2].into();
    let refused = |host: &ReplySocket| {
        let (result, source) = host
            .send_pinned(peer, Some(pin), |source| {
                Ok::<_, std::convert::Infallible>(match source {
                    Some(_) => Err(WSAEADDRNOTAVAIL),
                    None => Ok(()),
                })
            })
            .unwrap();
        assert_eq!((result, source), (Ok(()), None));
    };
    host.answer_from(peer, Some(pin));
    assert_eq!(host.source(peer), Some(pin));
    refused(&host);
    advance(REFUSAL - std::time::Duration::from_millis(1));
    host.answer_from(peer, Some(pin));
    assert_eq!(host.source(peer), None, "still refused");
    advance(std::time::Duration::from_millis(1));
    host.answer_from(peer, Some(pin));
    assert_eq!(host.source(peer), Some(pin), "pinned again");
    assert!(host.sources.read().unwrap().refused.is_empty());
    // Refused again, it lapses anew.
    refused(&host);
    advance(REFUSAL - std::time::Duration::from_millis(1));
    host.answer_from(peer, Some(pin));
    assert_eq!(host.source(peer), None);
    Ok(())
}
/// A full table evicts the oldest pin, not every pin.
#[test]
fn a_full_table_evicts_the_oldest_pin() -> Result<()> {
    let mut host = ReplySocket::bind("127.0.0.1:0")?;
    host.route = |_, _| Ok(IpAddr::from([192, 0, 2, 9]));
    let local = Some(IpAddr::from([192, 0, 2, 2]));
    let peers: Vec<SocketAddr> = (0..=REPLY_SOURCES as u16)
        .map(|port| SocketAddr::from(([192, 0, 2, 1], 1000 + port)))
        .collect();
    for peer in &peers[..REPLY_SOURCES] {
        host.answer_from(*peer, local);
    }
    // The first peer's newer datagram makes the second the oldest.
    host.answer_from(peers[0], Some(IpAddr::from([192, 0, 2, 3])));
    host.answer_from(peers[REPLY_SOURCES], local);
    assert_eq!(host.source(peers[1]), None);
    assert_eq!(host.source(peers[0]), Some(IpAddr::from([192, 0, 2, 3])));
    for peer in &peers[2..] {
        assert_eq!(host.source(*peer), local, "{peer}");
    }
    host.forget(peers[2]);
    assert_eq!(host.source(peers[2]), None);
    Ok(())
}
/// The address the fake route below answers with.
static ROUTED: std::sync::Mutex<Option<IpAddr>> = std::sync::Mutex::new(None);
/// A peer whose route leaves from the address it reached is answered
/// unpinned and one whose route leaves from elsewhere pinned. A route
/// that changes is noticed on the first datagram a second after the last
/// check, or RECHECK after it once a send failed as when a VPN came up.
#[test]
fn the_route_decides_whether_replies_are_pinned() -> Result<()> {
    let mut host = ReplySocket::bind("127.0.0.1:0")?;
    fake_clock(&mut host);
    host.route = |_, _| Ok(ROUTED.lock().unwrap().unwrap());
    let route = |address: [u8; 4]| *ROUTED.lock().unwrap() = Some(address.into());
    let peer: SocketAddr = "192.0.2.1:9".parse().unwrap();
    let local = IpAddr::from([192, 0, 2, 2]);
    route([192, 0, 2, 2]);
    host.answer_from(peer, Some(local));
    assert_eq!(host.source(peer), None, "routed from the address reached");
    route([192, 0, 2, 9]);
    host.answer_from(peer, Some(local));
    assert_eq!(host.source(peer), None, "checked within a second");
    advance(ROUTE_CHECK);
    host.answer_from(peer, Some(local));
    assert_eq!(host.source(peer), Some(local), "routed from elsewhere");
    route([192, 0, 2, 2]);
    advance(ROUTE_CHECK);
    host.answer_from(peer, Some(local));
    assert_eq!(host.source(peer), None, "routed back");
    assert!(
        !host.any_pinned.load(Ordering::Acquire),
        "sends skip the lock"
    );
    // A route that changed, found by a failed unpinned send: the next
    // datagram RECHECK after the last check pins the peer.
    route([192, 0, 2, 9]);
    let (result, source) = host.send_pinned(peer, None, |source| {
        assert_eq!(source, None);
        Ok::<_, std::convert::Infallible>(Err::<(), _>(WSAENETUNREACH))
    })?;
    assert_eq!((result, source), (Err(WSAENETUNREACH), None));
    host.answer_from(peer, Some(local));
    assert_eq!(host.source(peer), None, "checked within RECHECK");
    advance(RECHECK);
    host.answer_from(peer, Some(local));
    assert_eq!(host.source(peer), Some(local), "rechecked after a failure");
    // A route Windows cannot name keeps what was decided.
    host.route = |_, _| Err(std::io::Error::from_raw_os_error(WSAENETUNREACH.0));
    advance(ROUTE_CHECK);
    host.answer_from(peer, Some(local));
    assert_eq!(host.source(peer), Some(local));
    host.forget(peer);
    assert!(!host.any_pinned.load(Ordering::Acquire));
    Ok(())
}
/// Route lookups by the fake route below.
static LOOKUPS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// Failed sends never ask Windows for the route themselves, and however
/// many fail, the receiving thread asks at most once per RECHECK.
#[test]
fn failed_sends_check_the_route_at_most_once_per_recheck() -> Result<()> {
    let mut host = ReplySocket::bind("127.0.0.1:0")?;
    fake_clock(&mut host);
    host.route = |_, _| {
        LOOKUPS.fetch_add(1, Ordering::Relaxed);
        Ok(IpAddr::from([192, 0, 2, 2]))
    };
    let peer: SocketAddr = "192.0.2.1:9".parse().unwrap();
    let local = IpAddr::from([192, 0, 2, 2]);
    host.answer_from(peer, Some(local));
    assert_eq!(LOOKUPS.load(Ordering::Relaxed), 1);
    let fail = |host: &ReplySocket| {
        let (result, _) = host
            .send_pinned(peer, None, |_| {
                Ok::<_, std::convert::Infallible>(Err::<(), _>(WSAENETUNREACH))
            })
            .unwrap();
        assert_eq!(result, Err(WSAENETUNREACH));
    };
    for _ in 0..1000 {
        fail(&host);
    }
    assert_eq!(
        LOOKUPS.load(Ordering::Relaxed),
        1,
        "not on the sending thread"
    );
    advance(RECHECK);
    for _ in 0..1000 {
        fail(&host);
        host.answer_from(peer, Some(local));
    }
    assert_eq!(LOOKUPS.load(Ordering::Relaxed), 2, "once per RECHECK");
    Ok(())
}
