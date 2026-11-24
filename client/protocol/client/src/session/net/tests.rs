//! Unit tests for address resolution, probing, socket binding, pingers and kernel arrival
//! timestamps.

mod batch;

use super::batch::arrival_us;
#[cfg(any(target_os = "linux", target_os = "android"))]
use super::batch::{cmsg_timestamp_ns, SOL_SOCKET, SO_TIMESTAMPNS};
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

fn addr(text: &str) -> SocketAddr {
    text.parse().unwrap()
}

/// A dead first address costs the wait for a preferred answer, not its whole timeout: the
/// probes run at once.
#[test]
fn a_dead_address_does_not_hold_up_one_that_answers() {
    let addresses = [addr("192.0.2.1:0"), addr("127.0.0.1:0")];
    let started = Instant::now();
    let found = probe(&addresses, &[47984, 47989], Duration::from_secs(30), Duration::from_millis(50), |candidate, timeout| {
        if candidate.ip() == addr("192.0.2.1:0").ip() {
            std::thread::sleep(timeout);
            false
        } else {
            true
        }
    });
    assert_eq!(found, Some(addresses[1]));
    assert!(started.elapsed() < Duration::from_secs(20), "{:?}", started.elapsed());
}

/// When several answer, the resolver's order decides: an earlier address still trying is
/// waited for, however fast a later one answered.
#[test]
fn the_earliest_address_that_answers_wins() {
    let addresses = [addr("10.0.0.2:0"), addr("10.0.0.3:0"), addr("10.0.0.4:0")];
    let found = probe(&addresses, &[47984], Duration::from_secs(30), Duration::from_secs(30), move |candidate, _| {
        match candidate.ip() {
            ip if ip == addresses[0].ip() => false,
            ip if ip == addresses[1].ip() => {
                std::thread::sleep(Duration::from_millis(50));
                true
            }
            _ => true,
        }
    });
    assert_eq!(found, Some(addresses[1]));
    // One port of several answering is enough, and the port is not part of the result.
    let found = probe(&addresses[..1], &[47984, 47989, 48010], Duration::from_secs(30), Duration::ZERO, |candidate, _| {
        candidate.port() == 48010
    });
    assert_eq!(found, Some(addresses[0]));
}

#[test]
fn no_answer_is_none_and_only_the_first_addresses_are_tried() {
    let addresses: Vec<SocketAddr> = (1..=20).map(|i| addr(&format!("10.0.1.{i}:0"))).collect();
    let tried = Arc::new(Mutex::new(Vec::new()));
    let seen = tried.clone();
    let found = probe(&addresses, &[47984], Duration::from_secs(30), Duration::ZERO, move |candidate, _| {
        seen.lock().unwrap().push(candidate.ip());
        false
    });
    assert_eq!(found, None);
    let mut tried = tried.lock().unwrap().clone();
    tried.sort();
    let mut first: Vec<IpAddr> = addresses[..PROBED_ADDRESSES].iter().map(SocketAddr::ip).collect();
    first.sort();
    assert_eq!(tried, first);
}

/// Over real sockets: the address with a listener is found though another never answers.
#[test]
fn resolution_finds_the_address_with_a_listener() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let addresses = [addr("192.0.2.1:0"), addr("127.0.0.1:0")];
    let started = Instant::now();
    let found = probe(&addresses, &[port], TEST_PORT_TIMEOUT, PREFERENCE_WAIT, |candidate, timeout| {
        TcpStream::connect_timeout(&candidate, timeout).is_ok()
    });
    assert_eq!(found, Some(addresses[1]));
    assert!(started.elapsed() < TEST_PORT_TIMEOUT + Duration::from_secs(1), "{:?}", started.elapsed());
}

#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn a_timestamp_control_message_is_read_among_others() {
    let word = std::mem::size_of::<std::os::raw::c_long>();
    let mut timespec = Vec::new();
    for value in [12i64, 345_678_901] {
        timespec.extend_from_slice(&value.to_ne_bytes()[..word]);
    }
    let mut control = control_message(SOL_UDP_FOR_TEST, 104, &1400i32.to_ne_bytes());
    control.extend(control_message(SOL_SOCKET, SO_TIMESTAMPNS, &timespec));
    assert_eq!(cmsg_timestamp_ns(&control), Some(12_345_678_901));
    assert_eq!(
        cmsg_timestamp_ns(&control[..24]),
        None,
        "no timestamp message"
    );
    assert_eq!(cmsg_timestamp_ns(&[]), None);
}

#[cfg(any(target_os = "linux", target_os = "android"))]
const SOL_UDP_FOR_TEST: i32 = 17;

#[test]
fn an_arrival_is_the_batch_clock_minus_the_datagrams_age() {
    // Read 500 µs after the stamp: it arrived 500 µs before the local clock's reading.
    assert_eq!(arrival_us(1_000_000_000, 1_000_500_000, 10_000), 9_500);
    // A stamp after the reading (the wall clock stepped back) counts as now.
    assert_eq!(arrival_us(2_000_000_000, 1_000_000_000, 10_000), 10_000);
    // Older than the local clock's origin: 0.
    assert_eq!(arrival_us(0, 1_000_000_000, 10_000), 0);
}

#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn datagrams_of_one_batch_carry_their_own_kernel_arrival() {
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    receiver
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut batch = BatchReceiver::new(&receiver, 1500, false);
    assert!(batch.batched());
    // The first receive sets the socket option, so it must come before the datagrams.
    assert_eq!(batch.receive(&receiver).unwrap(), 0);
    let to = receiver.local_addr().unwrap();
    sender.send_to(&[1; 100], to).unwrap();
    std::thread::sleep(Duration::from_millis(20));
    sender.send_to(&[2; 100], to).unwrap();
    assert_eq!(batch.receive(&receiver).unwrap(), 2);
    let (first, second) = (batch.arrival_us(0), batch.arrival_us(1));
    let (first, second) = (first.expect("a stamp"), second.expect("a stamp"));
    assert!(
        second >= first + 15_000,
        "20 ms apart on the wire: {first} and {second}"
    );
}
