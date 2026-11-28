//! Tests for network interfaces, gateways, link detection, MTU, QoS, and change notifications.
use super::super::batch::Ancillary;
use super::super::interfaces::{interface_link, underlying_link};
use super::super::reply::local_address;
use super::super::*;
use anyhow::Result;
use std::{
    net::{IpAddr, SocketAddr, UdpSocket},
    os::windows::io::AsRawSocket,
    sync::atomic::Ordering,
};
use windows::Win32::{
    NetworkManagement::{IpHelper::*, Ndis::*},
    Networking::WinSock::*,
};

#[test]
fn the_default_gateways_are_the_next_hops_of_the_default_routes() {
    let gateways = default_gateways().unwrap();
    // A PC with a default route has its next hop here, and none is a placeholder.
    assert!(
        gateways
            .iter()
            .all(|gateway| !gateway.is_unspecified() && !gateway.is_loopback()),
        "{gateways:?}"
    );
    let best = std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|socket| socket.connect("192.0.2.1:9").map(|_| socket))
        .is_ok();
    if best {
        assert!(
            !gateways.is_empty(),
            "a route to the internet but no gateway"
        );
    }
}

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
                butterpollo_media_policy::network_pacing::rate_bps(
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
            butterpollo_media_policy::network_pacing::rate_bps(
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
            butterpollo_media_policy::network_pacing::rate_bps(
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
        .map(|a| butterpollo_session::wake::format_mac(a.mac))
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
fn qos_leaves_clients_it_cannot_tag_alone() -> Result<()> {
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    assert!(QosFlow::new(&socket, "127.0.0.1:9".parse()?, false)?.is_none());
    let dual = UdpSocket::bind("[::1]:0")?;
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
fn a_loopback_route_reports_no_mtu_and_a_socket_can_refuse_fragmentation() -> Result<()> {
    for peer in [
        "127.0.0.1:9",
        "127.77.1.2:9",
        "127.78.1.2:9",
        "[::1]:9",
        "[::ffff:127.77.1.2]:9",
    ] {
        let peer: SocketAddr = peer.parse()?;
        // Loopback and Internet Play tunnel addresses must never report a giant pseudo-MTU.
        assert_eq!(routed_mtu(peer), None, "{peer}");
    }
    for (peer, bind) in [
        ("127.0.0.1:9".parse::<SocketAddr>()?, "127.0.0.1:0"),
        ("[::1]:9".parse::<SocketAddr>()?, "[::1]:0"),
    ] {
        assert!(routed_same_link(peer), "{peer}");
        let socket = UdpSocket::bind(bind)?;
        // A jumbo stream enables both families' refusal; a later stock stream
        // must be able to clear it on the same process-wide socket.
        set_dont_fragment(&socket, true)?;
        set_dont_fragment(&socket, false)?;
    }
    // A dual-stack socket carries both families, so both options must apply.
    let dual = socket2::Socket::new(
        socket2::Domain::IPV6,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;
    dual.set_only_v6(false)?;
    dual.bind(&"[::]:0".parse::<SocketAddr>()?.into())?;
    let dual: UdpSocket = dual.into();
    set_dont_fragment(&dual, true)?;
    let read = |level: i32, option: i32| {
        let mut value = 0u32;
        let mut length = std::mem::size_of::<u32>() as i32;
        // SAFETY: `value` and `length` are live locals; the socket is open.
        let read = unsafe {
            windows::Win32::Networking::WinSock::getsockopt(
                SOCKET(dual.as_raw_socket() as usize),
                level,
                option,
                windows::core::PSTR((&mut value as *mut u32).cast()),
                &mut length,
            )
        };
        assert_eq!(read, 0);
        value
    };
    assert_eq!(read(IPPROTO_IP.0, IP_DONTFRAGMENT), 1);
    assert_eq!(read(IPPROTO_IPV6.0, IPV6_DONTFRAG), 1);
    set_dont_fragment(&dual, false)?;
    assert_eq!(read(IPPROTO_IP.0, IP_DONTFRAGMENT), 0);
    assert_eq!(read(IPPROTO_IPV6.0, IPV6_DONTFRAG), 0);
    Ok(())
}

#[test]
fn an_oversized_datagram_is_counted_as_oversized_but_still_dropped() {
    let mut batch = Batch {
        dont_fragment: true,
        ..Batch::default()
    };
    batch.too_large(2, WSAEMSGSIZE);
    assert_eq!(batch.dropped, 2);
    assert_eq!(batch.oversized, 2);
}

#[test]
fn hyper_v_adapters_are_known_by_their_drivers_description() {
    for description in [
        "Hyper-V Virtual Ethernet Adapter",
        "Hyper-V Virtual Ethernet Adapter #2",
    ] {
        assert!(hyper_v_adapter(description), "{description}");
    }
    for description in [
        "TAP-Windows Adapter V9",
        "ZeroTier Virtual Port",
        "Intel(R) Ethernet Connection (17) I219-V",
        "vEthernet (External)",
    ] {
        assert!(!hyper_v_adapter(description), "{description}");
    }
}

#[test]
fn an_uplink_is_physical_or_hyper_vs_adapter_with_a_default_gateway() {
    let interface = |hardware, hyper_v, gateways: &[&str]| LanInterface {
        address: "192.0.2.10".parse().unwrap(),
        hardware,
        hyper_v,
        metric: 25,
        gateways: gateways
            .iter()
            .map(|gateway| gateway.parse().unwrap())
            .collect(),
    };
    assert!(interface(true, false, &[]).uplink());
    // An external switch's adapter, and the Default Switch's or WSL's.
    assert!(interface(false, true, &["192.0.2.1"]).uplink());
    assert!(!interface(false, true, &[]).uplink());
    // A TAP or ZeroTier adapter, with or without a default gateway.
    assert!(!interface(false, false, &["10.8.0.1"]).uplink());
}

/// Every address this PC lists is read with its metric and gateways; one with a default route
/// has its gateway among them.
#[test]
fn lan_interfaces_read_this_pcs_adapters() {
    let interfaces = lan_interfaces().unwrap();
    let gateways = default_gateways().unwrap();
    for interface in &interfaces {
        assert!(!interface.address.is_unspecified());
        for gateway in &interface.gateways {
            assert!(
                gateways.contains(&std::net::IpAddr::V4(*gateway)),
                "{interface:?}"
            );
        }
    }
    assert_eq!(
        lan_addresses().unwrap(),
        interfaces
            .iter()
            .map(|interface| interface.address.to_string())
            .collect::<Vec<_>>()
    );
}

#[test]
fn interface_change_notifications_arrive_and_end_with_the_registration() {
    use std::sync::{Arc, atomic::AtomicUsize};
    let arrived = Arc::new(AtomicUsize::new(0));
    let seen = arrived.clone();
    // The initial notification reports every interface this PC has, so at least one arrives
    // without an interface actually changing.
    let changes = InterfaceChanges::register_initial(
        move || {
            seen.fetch_add(1, Ordering::Relaxed);
        },
        || {},
    )
    .expect("Windows refused NotifyIpInterfaceChange");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while arrived.load(Ordering::Relaxed) == 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        arrived.load(Ordering::Relaxed) >= 1,
        "no interface reported"
    );
    drop(changes);
}

#[test]
fn configure_udp_sets_four_megabyte_send_buffer() -> Result<()> {
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    configure_udp(&socket)?;
    assert!(socket2::SockRef::from(&socket).send_buffer_size()? >= SEND_BUFFER_DEFAULT);
    Ok(())
}
