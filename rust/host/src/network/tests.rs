use super::*;
#[tokio::test]
async fn port_forwarding_sleeps_until_its_renewal_or_the_hosts_stop() {
    let stop = tokio::sync::Notify::new();
    // Without a stop it sleeps until the renewal, not in short polls.
    let start = Instant::now();
    until_renewal_or_stop(&stop, start + Duration::from_millis(150)).await;
    assert!(start.elapsed() >= Duration::from_millis(150));
    // A stop wakes it at once, also one told while it was renewing.
    let start = Instant::now();
    stop.notify_one();
    until_renewal_or_stop(&stop, start + Duration::from_secs(60)).await;
    assert!(start.elapsed() < Duration::from_secs(30));
    let stopping = async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        stop.notify_one();
    };
    let start = Instant::now();
    tokio::join!(
        until_renewal_or_stop(&stop, start + Duration::from_secs(60)),
        stopping
    );
    assert!(start.elapsed() < Duration::from_secs(30));
    // A renewal that is due already does not wait.
    let start = Instant::now();
    until_renewal_or_stop(&stop, start).await;
    assert!(start.elapsed() < Duration::from_secs(30));
}
#[test]
fn peers_are_classified_like_vibepollo() {
    for (address, expected) in [
        ("127.0.0.1", Reach::Pc),
        ("::1", Reach::Pc),
        ("192.168.4.20", Reach::Lan),
        ("10.1.2.3", Reach::Lan),
        ("172.20.0.1", Reach::Lan),
        ("169.254.10.1", Reach::Lan),
        ("100.101.102.103", Reach::Lan),
        ("100.128.0.1", Reach::Wan),
        ("fd12::1", Reach::Lan),
        ("fe80::1", Reach::Lan),
        ("::ffff:192.168.1.5", Reach::Lan),
        ("8.8.8.8", Reach::Wan),
        ("2001:db8::1", Reach::Wan),
    ] {
        assert_eq!(reach(address.parse().unwrap()), expected, "{address}");
    }
    assert_eq!(web_reach(&Config::default()), Reach::Lan);
    assert_eq!(
        web_reach(&Config::parse("origin_web_ui_allowed = pc\n").unwrap()),
        Reach::Pc
    );
}
#[test]
fn play_takes_tailscale_only_on_its_address_and_never_the_gateway() {
    let ip = |text: &str| text.parse::<IpAddr>().unwrap();
    let gateways = [ip("192.168.4.1"), ip("fe80::1")];
    let lan = ip("192.168.4.69");
    let tailnet = ip("100.90.1.2");
    for (peer, local, allowed) in [
        ("192.168.4.20", lan, true),
        ("127.0.0.1", ip("127.0.0.1"), true),
        ("fd00::5", ip("fd00::69"), true),
        // Tailscale traffic arrives on the PC's Tailscale address.
        ("100.101.102.103", tailnet, true),
        ("::ffff:100.101.102.103", ip("::ffff:100.90.1.2"), true),
        // A CGNAT neighbour through a forwarded port arrives on the LAN address.
        ("100.101.102.103", lan, false),
        ("::ffff:100.64.0.9", lan, false),
        // A router that source-NATs the forward: its own address, on either family.
        ("192.168.4.1", lan, false),
        ("::ffff:192.168.4.1", lan, false),
        ("fe80::1", ip("fe80::69"), false),
        ("203.0.113.7", lan, false),
        ("100.128.0.1", tailnet, false),
    ] {
        assert_eq!(
            play_refusal(ip(peer), local, &gateways).is_none(),
            allowed,
            "{peer} on {local}"
        );
    }
    // With no gateway known, the LAN router's address is a LAN peer like any other.
    assert!(play_refusal(ip("192.168.4.1"), lan, &[]).is_none());
}
#[test]
fn play_reads_the_routing_table_only_for_peers_the_address_checks_let_in() {
    let reads = || GATEWAY_READS.with(std::cell::Cell::get);
    let at = |text: &str| text.parse::<SocketAddr>().unwrap();
    let lan = at("192.168.4.69:47991");
    let before = reads();
    assert!(play_refused(at("203.0.113.7:50000"), lan).is_some());
    assert!(play_refused(at("100.101.102.103:50000"), lan).is_some());
    assert_eq!(reads(), before);
    assert!(play_refused(at("192.168.4.20:50000"), lan).is_none());
    assert!(play_refused(at(&format!("{TEST_GATEWAY}:50000")), lan).is_some());
    assert_eq!(reads(), before + 2);
}
#[test]
fn a_browser_on_this_pc_gets_media_at_an_address_every_browser_sends_to() {
    let lan: IpAddr = "192.168.4.69".parse().unwrap();
    let any_v4 = IpAddr::from([0, 0, 0, 0]);
    let any = IpAddr::from([0u16; 8]);
    // Opened at 127.0.0.1 or localhost, on a host listening everywhere (issue 67).
    for signalled in ["127.0.0.1", "::1", "::ffff:127.0.0.1"] {
        let signalled = signalled.parse().unwrap();
        assert_eq!(media_address(signalled, any_v4, || Some(lan)), lan);
        assert_eq!(media_address(signalled, any, || Some(lan)), lan);
    }
    // No network: nothing better than loopback.
    let loopback = IpAddr::from([127, 0, 0, 1]);
    assert_eq!(media_address(loopback, any_v4, || None), loopback);
    // A host bound to loopback never opens a port on the network.
    assert_eq!(
        media_address(loopback, loopback, || panic!("no LAN lookup")),
        loopback
    );
    // Every other browser keeps the address its signalling reached.
    for signalled in ["192.168.4.69", "100.101.102.103", "fd43::1", "2001:db8::1"] {
        let signalled: IpAddr = signalled.parse().unwrap();
        assert_eq!(
            media_address(signalled, any_v4, || panic!("no LAN lookup")),
            signalled
        );
    }
}
#[test]
fn a_peer_on_this_pc_comes_over_loopback_or_from_the_address_it_reached() {
    let lan: IpAddr = "192.168.4.69".parse().unwrap();
    let loopback = IpAddr::from([127, 0, 0, 1]);
    assert!(peer_is_this_pc(
        "127.0.0.1:50000".parse().unwrap(),
        loopback
    ));
    assert!(peer_is_this_pc(
        "[::1]:50000".parse().unwrap(),
        "::1".parse().unwrap()
    ));
    assert!(peer_is_this_pc("192.168.4.69:50000".parse().unwrap(), lan));
    assert!(peer_is_this_pc(
        "[::ffff:192.168.4.69]:50000".parse().unwrap(),
        lan
    ));
    assert!(!peer_is_this_pc("192.168.4.10:50000".parse().unwrap(), lan));
}

/// An internet play tunnel reaches the host's ports from this PC: from a loopback address of
/// the device's own, or, when the host listens on one address, from that address. Neither
/// may pass for a program on this PC or the local network.
#[test]
fn a_tunnelled_peer_is_never_this_pc_or_the_local_network() {
    let loopback = IpAddr::from([127, 0, 0, 1]);
    let lan: IpAddr = "192.168.4.69".parse().unwrap();
    let per_device: SocketAddr = "127.77.4.2:50000".parse().unwrap();
    assert_eq!(reach(per_device.ip()), Reach::Wan);
    assert_eq!(reach("127.78.1.1".parse().unwrap()), Reach::Wan);
    assert_eq!(peer_reach(per_device), Reach::Wan);
    assert!(tunnelled(per_device));
    assert!(!from_this_pc(per_device));
    assert!(!peer_is_this_pc(per_device, loopback));
    assert!(!browser_on_this_pc(
        per_device,
        "127.0.0.1:47990".parse().unwrap(),
        &HeaderMap::new()
    ));
    assert!(play_refused(per_device, "127.0.0.1:47990".parse().unwrap()).is_some());
    assert_eq!(
        crate::state::pairing_source(per_device.ip()),
        "a device over the internet"
    );

    // The address the host listens on, registered by the tunnel for this connection only.
    let shared: SocketAddr = "192.168.4.69:50011".parse().unwrap();
    let loopback_shared: SocketAddr = "127.0.0.1:50012".parse().unwrap();
    assert_eq!(peer_reach(shared), Reach::Lan);
    assert!(peer_is_this_pc(shared, lan));
    assert!(from_this_pc(loopback_shared));
    let marks = [
        rubylight_traverse::peers::register(shared, "phone"),
        rubylight_traverse::peers::register(loopback_shared, "phone"),
    ];
    assert_eq!(peer_reach(shared), Reach::Wan);
    assert_eq!(peer_reach(loopback_shared), Reach::Wan);
    assert!(!peer_is_this_pc(shared, lan));
    assert!(!peer_is_this_pc(loopback_shared, loopback));
    assert!(!from_this_pc(loopback_shared));
    assert!(!browser_on_this_pc(
        loopback_shared,
        "127.0.0.1:47990".parse().unwrap(),
        &HeaderMap::new()
    ));
    assert!(play_refused(shared, "192.168.4.69:47990".parse().unwrap()).is_some());
    // Another connection from the same address is still this PC.
    assert!(from_this_pc("127.0.0.1:50013".parse().unwrap()));
    drop(marks);
    assert!(from_this_pc(loopback_shared));
}

/// A tunnelled client gets the internet's encryption, never the local network's.
#[test]
fn a_tunnelled_peer_needs_the_internets_encryption() {
    let config = Config::parse("lan_encryption_mode = 0\nwan_encryption_mode = 2\n").unwrap();
    assert_eq!(
        encryption_mode(&config, "127.0.0.1:50020".parse().unwrap()),
        0
    );
    assert_eq!(
        encryption_mode(&config, "127.77.9.9:50020".parse().unwrap()),
        2
    );
    let shared: SocketAddr = "192.168.4.69:50021".parse().unwrap();
    assert_eq!(encryption_mode(&config, shared), 0);
    let mark = rubylight_traverse::peers::register(shared, "phone");
    assert_eq!(encryption_mode(&config, shared), 2);
    drop(mark);
    // Defaults: the internet asks for encryption where the local network doesn't.
    let defaults = Config::default();
    assert_eq!(
        encryption_mode(&defaults, "127.77.9.9:50020".parse().unwrap()),
        1
    );
}
fn request(pairs: &[(&'static str, &str)]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in pairs {
        headers.append(*name, value.parse().unwrap());
    }
    headers
}
#[test]
fn a_browser_on_this_pc_names_this_pc_however_it_opened_the_console() {
    let loopback: SocketAddr = "127.0.0.1:47990".parse().unwrap();
    let v6: SocketAddr = "[::1]:47990".parse().unwrap();
    let lan: SocketAddr = "192.168.4.69:47990".parse().unwrap();
    let tailnet: SocketAddr = "100.90.1.2:47990".parse().unwrap();
    for (local, host, origin) in [
        (loopback, "127.0.0.1:47990", "https://127.0.0.1:47990"),
        (loopback, "localhost:47990", "https://localhost:47990"),
        (loopback, "LOCALHOST:47990", "https://Localhost:47990"),
        (
            loopback,
            "console.localhost:47990",
            "https://console.localhost:47990",
        ),
        // A console on the scheme's default port.
        (
            "127.0.0.1:443".parse().unwrap(),
            "127.0.0.1",
            "https://127.0.0.1",
        ),
        (v6, "[::1]:47990", "https://[::1]:47990"),
        // At the PC's network address the connection reaches that address.
        (lan, "192.168.4.69:47990", "https://192.168.4.69:47990"),
        (tailnet, "100.90.1.2:47990", "https://100.90.1.2:47990"),
        (lan, "gaming-pc:47990", "https://Gaming-PC:47990"),
        (
            lan,
            "gaming-pc.local:47990",
            "https://gaming-pc.local:47990",
        ),
        // A page may signal to another port than its own.
        (loopback, "127.0.0.1:47990", "https://127.0.0.1:47991"),
    ] {
        let headers = request(&[("host", host), ("origin", origin)]);
        assert!(
            !request_names_elsewhere(&headers, local, Some("GAMING-PC")),
            "{host} {origin} on {local}"
        );
    }
    assert!(!request_names_elsewhere(
        &HeaderMap::new(),
        loopback,
        Some("GAMING-PC")
    ));
}
#[test]
fn a_proxy_on_this_pc_is_not_a_browser_on_this_pc() {
    let loopback: SocketAddr = "127.0.0.1:47990".parse().unwrap();
    let lan: SocketAddr = "192.168.4.69:47990".parse().unwrap();
    let this_pc = [
        ("host", "127.0.0.1:47990"),
        ("origin", "https://127.0.0.1:47990"),
    ];
    for (name, value) in [
        // What nginx, cloudflared and `tailscale serve` add, whatever else they keep.
        ("x-forwarded-for", "203.0.113.7"),
        ("x-forwarded-proto", "https"),
        ("x-forwarded-host", "console.example.com"),
        ("forwarded", "for=203.0.113.7"),
    ] {
        let mut pairs = this_pc.to_vec();
        pairs.push((name, value));
        assert!(
            request_names_elsewhere(&request(&pairs), loopback, Some("GAMING-PC")),
            "{name}"
        );
    }
    for (local, host, origin) in [
        // nginx passes the name on, or the browser's Origin at least.
        (
            loopback,
            "console.example.com",
            "https://console.example.com",
        ),
        (loopback, "127.0.0.1:47990", "https://console.example.com"),
        (loopback, "127.0.0.1:47990", "https://pc.tailnet.ts.net"),
        // A longer name of this PC may be a proxy's.
        (
            loopback,
            "gaming-pc.example.com",
            "https://gaming-pc.example.com",
        ),
        // `ssh -L 9000:localhost:47990`, opened on the far side as localhost.
        (loopback, "localhost:9000", "https://localhost:9000"),
        // `netsh portproxy` from the network address to loopback, or to another port.
        (loopback, "192.168.4.69:47990", "https://192.168.4.69:47990"),
        (lan, "192.168.4.69:8443", "https://192.168.4.69:8443"),
        // `netsh portproxy` from 443, so the browser sends no port.
        (lan, "192.168.4.69", "https://192.168.4.69"),
        (lan, "gaming-pc", "https://gaming-pc"),
        (loopback, "127.0.0.1", "https://127.0.0.1"),
        // Another address than the connection reached.
        (lan, "192.168.4.70:47990", "https://192.168.4.70:47990"),
        // No address at all.
        (loopback, "127.0.0.1:47990", "null"),
    ] {
        let headers = request(&[("host", host), ("origin", origin)]);
        assert!(
            request_names_elsewhere(&headers, local, Some("GAMING-PC")),
            "{host} {origin} on {local}"
        );
    }
    // docs/webrtc.md: nothing in this request differs from a browser's on this PC.
    let same_port = request(&[
        ("host", "localhost:47990"),
        ("origin", "https://localhost:47990"),
    ]);
    assert!(!request_names_elsewhere(
        &same_port,
        loopback,
        Some("GAMING-PC")
    ));
}
#[test]
fn the_address_and_the_headers_both_have_to_say_this_pc() {
    let peer: SocketAddr = "127.0.0.1:50000".parse().unwrap();
    let loopback: SocketAddr = "127.0.0.1:47990".parse().unwrap();
    let lan: SocketAddr = "192.168.4.69:47990".parse().unwrap();
    let clean = request(&[("host", "127.0.0.1:47990")]);
    assert!(browser_on_this_pc(peer, loopback, &clean));
    let proxied = request(&[
        ("host", "127.0.0.1:47990"),
        ("x-forwarded-for", "203.0.113.7"),
    ]);
    assert!(!browser_on_this_pc(peer, loopback, &proxied));
    // Clean headers grant nothing to a browser elsewhere.
    let elsewhere: SocketAddr = "192.168.4.10:50000".parse().unwrap();
    let named = request(&[("host", "192.168.4.69:47990")]);
    assert!(!browser_on_this_pc(elsewhere, lan, &named));
    assert!(browser_on_this_pc(
        "192.168.4.69:50000".parse().unwrap(),
        lan,
        &named
    ));
}
#[test]
fn lan_defaults_and_explicit_binding_keep_legacy_semantics() {
    assert_eq!(
        bind_address(&Config::default(), None).unwrap(),
        "0.0.0.0".parse::<IpAddr>().unwrap()
    );
    let config = Config::parse("address_family=both\nbind_address=192.0.2.10\n").unwrap();
    assert_eq!(
        bind_address(&config, None).unwrap(),
        "192.0.2.10".parse::<IpAddr>().unwrap()
    );
    assert_eq!(
        bind_address(&config, Some("127.0.0.1".parse().unwrap())).unwrap(),
        "127.0.0.1".parse::<IpAddr>().unwrap()
    );
    // An interface name instead of an address: this PC only, not down.
    let named = Config::parse("bind_address=Ethernet\n").unwrap();
    assert_eq!(
        bind_address(&named, None).unwrap(),
        "127.0.0.1".parse::<IpAddr>().unwrap()
    );
    // TCP 48118, 48123, 48144; UDP 48132, 48133, 48134 and the microphone's 48135.
    assert_eq!(
        mappings(&Config::default(), Ports::from_base(48123)).len(),
        7
    );
    let no_mic = Config::parse("stream_mic=false\n").unwrap();
    assert_eq!(mappings(&no_mic, Ports::from_base(48123)).len(), 6);
}
#[test]
fn browser_guest_ports_are_mapped_only_with_the_webrtc_server_on() {
    let guest_ports = |config: &Config| {
        mappings(config, Ports::from_base(48123))
            .into_iter()
            .filter(|m| m.port == 48125 || (9000..9010).contains(&m.port))
            .collect::<Vec<_>>()
    };
    let off = Config::parse("webrtc_media_port=9001\n").unwrap();
    assert!(guest_ports(&off).is_empty());
    let on = Config::parse("webrtc_server=true\nwebrtc_media_port=9001\n").unwrap();
    let mut expected = vec![Mapping {
        protocol: Protocol::TCP,
        port: 48125,
    }];
    expected.extend((9002..=9005).map(|port| Mapping {
        protocol: Protocol::UDP,
        port,
    }));
    assert_eq!(guest_ports(&on), expected);
    // The signalling and own-stream media ports stay closed.
    assert!(
        !mappings(&on, Ports::from_base(48123))
            .iter()
            .any(|m| m.port == 9001 || m.port == 8000)
    );
}
#[test]
fn a_mapping_left_by_another_host_on_this_pc_is_not_a_conflict() {
    let local: IpAddr = "192.168.0.20".parse().unwrap();
    let entry = |client: &str, internal_port| igd_next::PortMappingEntry {
        remote_host: String::new(),
        external_port: 47989,
        protocol: Protocol::TCP,
        internal_port,
        internal_client: client.into(),
        enabled: true,
        port_mapping_description: "Sunshine".into(),
        lease_duration: 0,
    };
    assert!(forwards_here(&entry("192.168.0.20", 47989), local));
    assert!(!forwards_here(&entry("192.168.0.30", 47989), local));
    assert!(!forwards_here(&entry("192.168.0.20", 47990), local));
    assert!(!owned(
        &entry("192.168.0.20", 47989),
        local,
        "Butterpollo Rust id"
    ));
}
#[tokio::test]
#[ignore = "listens on every interface; Windows Firewall asks again for every new test binary"]
async fn dual_stack_listener_accepts_ipv4_and_ipv6_loopback() {
    let listener = tcp("[::]:0".parse().unwrap()).unwrap();
    let port = listener.local_addr().unwrap().port();
    for address in [format!("127.0.0.1:{port}"), format!("[::1]:{port}")] {
        let client = tokio::net::TcpStream::connect(address).await.unwrap();
        let (_, peer) = listener.accept().await.unwrap();
        assert!(peer.ip().to_canonical().is_loopback());
        drop(client);
    }
}
