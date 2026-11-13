#[test]
fn a_pc_on_this_machine_always_has_a_route() {
    assert!(super::has_route("127.0.0.1"));
}

use super::*;

#[test]
fn link_hint_is_the_wired_speed_and_zero_on_wireless() {
    let wired = Network { link_mbps: 2500, ..Default::default() };
    assert_eq!(wired.wired_link_mbps(), 2500);
    assert_eq!(Network { link_mbps: 40_000, ..wired.clone() }.wired_link_mbps(), 9999);
    assert_eq!(Network { wifi: true, ..wired.clone() }.wired_link_mbps(), 0);
    assert_eq!(Network { cellular: true, ..wired.clone() }.wired_link_mbps(), 0);
    assert_eq!(Network::default().wired_link_mbps(), 0);
}

#[test]
fn wireless_is_wifi_or_cellular_and_nothing_else() {
    assert!(Network { wifi: true, ..Default::default() }.wireless());
    assert!(Network { cellular: true, ..Default::default() }.wireless());
    assert!(Network { wifi: true, cellular: true, ..Default::default() }.wireless());
    assert!(!Network { link_mbps: 2500, ..Default::default() }.wireless());
    // Metered alone says nothing about the medium; an unreadable network is not wireless.
    assert!(!Network { metered: true, ..Default::default() }.wireless());
    assert!(!Network { error: Some("no adapter".into()), ..Default::default() }.wireless());
}

#[test]
fn windows_interface_types_map_to_a_medium() {
    assert_eq!(medium_of_if_type(71, true), Medium::Wireless); // IEEE 802.11
    assert_eq!(medium_of_if_type(243, true), Medium::Wireless); // WWAN
    assert_eq!(medium_of_if_type(244, true), Medium::Wireless);
    assert_eq!(medium_of_if_type(6, true), Medium::Wired); // Ethernet
    assert_eq!(medium_of_if_type(117, true), Medium::Wired); // Gigabit Ethernet
    // Loopback, tunnel, PPP and unknown types say nothing.
    assert_eq!(medium_of_if_type(24, true), Medium::Other);
    assert_eq!(medium_of_if_type(131, true), Medium::Other);
    assert_eq!(medium_of_if_type(23, true), Medium::Other);
    assert_eq!(medium_of_if_type(0, true), Medium::Other);
}

#[test]
fn windows_virtual_adapters_do_not_count_as_wired() {
    // TAP-Windows, ZeroTier and Hamachi report ifType 6 without the HardwareInterface bit.
    assert_eq!(medium_of_if_type(6, false), Medium::Other);
    assert_eq!(medium_of_if_type(117, false), Medium::Other);
    assert_eq!(medium_of_if_type(71, false), Medium::Other);
}

#[test]
fn sysfs_attributes_map_to_a_medium() {
    assert_eq!(medium_of_sysfs(true, true, None, Some(1)), Medium::Wireless);
    assert_eq!(medium_of_sysfs(true, false, Some("wlan"), Some(1)), Medium::Wireless);
    assert_eq!(medium_of_sysfs(true, false, Some("wwan"), Some(1)), Medium::Wireless);
    assert_eq!(medium_of_sysfs(true, false, None, Some(1)), Medium::Wired);
    assert_eq!(medium_of_sysfs(true, false, Some("bridge"), Some(1)), Medium::Other);
    assert_eq!(medium_of_sysfs(true, false, None, Some(772)), Medium::Other); // loopback
    assert_eq!(medium_of_sysfs(true, false, None, Some(65534)), Medium::Other); // tun
    assert_eq!(medium_of_sysfs(true, false, None, None), Medium::Other);
}

#[test]
fn linux_interfaces_without_a_device_do_not_count_as_wired() {
    // tap and zerotier devices look like Ethernet but have no /sys/class/net/<if>/device.
    assert_eq!(medium_of_sysfs(false, false, None, Some(1)), Medium::Other);
    assert_eq!(medium_of_sysfs(false, false, Some("wlan"), Some(1)), Medium::Other);
}

#[test]
fn network_follows_the_route_and_falls_back_when_it_does_not_tell() {
    let wifi = Network { wifi: true, ssid: Some("home".into()), link_mbps: 866, ..Default::default() };
    let wired = wifi.toward(Ok(Toward { wireless: false, link_mbps: 2500 }));
    assert!(!wired.wifi && !wired.wireless());
    assert_eq!((wired.link_mbps, wired.ssid), (2500, None));
    let ethernet = Network { link_mbps: 1000, ..Default::default() };
    let wireless = ethernet.toward(Ok(Toward { wireless: true, link_mbps: 0 }));
    assert!(wireless.wifi && !wireless.cellular);
    assert_eq!(wireless.link_mbps, 0); // the internet is wired: no Wi-Fi rate to keep
    assert_eq!(wireless.ssid, None);
    // On Wi-Fi the rate and name stay what the system reports; the host hint is still 0.
    let kept = wifi.toward(Ok(Toward { wireless: true, link_mbps: 0 }));
    assert_eq!((kept.link_mbps, kept.ssid.as_deref()), (866, Some("home")));
    assert_eq!(kept.wired_link_mbps(), 0);
    let cellular = Network { cellular: true, ..Default::default() }.toward(Ok(Toward { wireless: true, link_mbps: 0 }));
    assert!(cellular.cellular && !cellular.wifi);
    assert_eq!(wifi.toward(Err(Fallback::NotTelling)).link_mbps, 866);
    assert!(wifi.toward(Err(Fallback::Route)).wifi);
    assert!(wifi.toward(Err(Fallback::Resolve)).wifi);
    // The route is known even when the connection profile could not be read.
    let unreadable = Network { error: Some("no profile".into()), ..Default::default() };
    assert!(unreadable.toward(Ok(Toward { wireless: false, link_mbps: 1000 })).error.is_none());
    assert!(unreadable.toward(Err(Fallback::Route)).error.is_some());
}

#[test]
fn windows_vpn_interfaces_are_told_apart() {
    let nic = [0x2c, 0xf0, 0x5d, 1, 2, 3];
    // WireGuardNT and Wintun (Tailscale) are IF_TYPE_PROP_VIRTUAL; tunnels and PPP count too.
    assert!(vpn_of_if_row(53, 1420, &[], "WireGuard Tunnel"));
    assert!(vpn_of_if_row(53, 1500, &[], "Tailscale"));
    assert!(vpn_of_if_row(131, 1500, &[], "Teredo"));
    assert!(vpn_of_if_row(23, 1500, &[], "Dial-up"));
    // A smaller MTU on another kind of adapter, OpenVPN's TAP address, ZeroTier and a VPN by name.
    assert!(vpn_of_if_row(1, 1400, &[], "Contoso Secure Client"));
    assert!(vpn_of_if_row(6, 1500, &[0x00, 0xFF, 1, 2, 3, 4], "Ethernet 4"));
    assert!(vpn_of_if_row(6, 1500, &nic, "ZeroTier One [abcdef]"));
    assert!(vpn_of_if_row(6, 1500, &nic, "Contoso VPN"));
    // Ethernet, Wi-Fi and Hyper-V's virtual switch are not; nor is loopback's all-ones MTU.
    assert!(!vpn_of_if_row(6, 1500, &nic, "Ethernet"));
    assert!(!vpn_of_if_row(71, 1500, &nic, "Wi-Fi"));
    assert!(!vpn_of_if_row(6, 1500, &[0x00, 0x15, 0x5d, 1, 2, 3], "vEthernet (Default Switch)"));
    assert!(!vpn_of_if_row(24, u32::MAX, &[], "Loopback Pseudo-Interface 1"));
    assert!(!vpn_of_if_row(6, 9000, &nic, "Ethernet"), "jumbo frames");
    assert!(!vpn_of_if_row(6, 0, &nic, "Ethernet"), "an unknown MTU");
    // The review's regression: a PPPoE LAN's 1492 MTU on Ethernet or Wi-Fi is no VPN.
    assert!(!vpn_of_if_row(6, 1492, &nic, "Ethernet"));
    assert!(!vpn_of_if_row(71, 1492, &nic, "Wi-Fi"));
}

#[test]
fn linux_vpn_interfaces_are_told_apart() {
    // wg0 and tailscale0 are ARPHRD_NONE with a small MTU; tun0 of OpenVPN is point to point.
    assert!(vpn_of_sysfs(Some(65534), Some(1420), Some(0x1091), None, "wg0"));
    assert!(vpn_of_sysfs(Some(65534), Some(1500), Some(0x1091), None, "tun0"));
    assert!(vpn_of_sysfs(Some(512), Some(1500), None, None, "ppp0"));
    assert!(vpn_of_sysfs(Some(65535), Some(1400), Some(0x1003), None, "vti0"));
    assert!(vpn_of_sysfs(Some(1), Some(1500), Some(0x1013), Some("2c:f0:5d:01:02:03"), "eth1"), "IFF_POINTOPOINT");
    assert!(vpn_of_sysfs(Some(1), Some(1500), Some(0x1003), Some("00:FF:12:34:56:78"), "tap0"));
    // Ethernet, Wi-Fi and loopback are not.
    assert!(!vpn_of_sysfs(Some(1), Some(1500), Some(0x1003), Some("2c:f0:5d:01:02:03"), "enp3s0"));
    assert!(!vpn_of_sysfs(Some(1), Some(1500), Some(0x1003), Some("2c:f0:5d:01:02:04"), "wlp2s0"));
    assert!(!vpn_of_sysfs(Some(772), Some(65536), Some(0x9), Some("00:00:00:00:00:00"), "lo"));
    assert!(!vpn_of_sysfs(None, None, None, None, "eth0"), "nothing readable");
    // The review's regression: a PPPoE LAN's 1492 MTU on Ethernet or Wi-Fi is no VPN.
    assert!(!vpn_of_sysfs(Some(1), Some(1492), Some(0x1003), Some("2c:f0:5d:01:02:03"), "enp3s0"));
    assert!(!vpn_of_sysfs(Some(1), Some(1492), Some(0x1003), Some("2c:f0:5d:01:02:04"), "wlp2s0"));
}

#[test]
fn route_lookup_output_names_the_interface() {
    assert_eq!(parse_route_dev("192.168.1.9 dev wlp3s0 src 192.168.1.20 uid 1000"), Some("wlp3s0"));
    assert_eq!(parse_route_dev("10.0.0.1 via 10.0.0.254 dev eth0 src 10.0.0.2\n"), Some("eth0"));
    assert_eq!(parse_route_dev("local 127.0.0.1 dev lo src 127.0.0.1"), Some("lo"));
    assert_eq!(parse_route_dev("RTNETLINK answers: Network is unreachable"), None);
    assert_eq!(parse_route_dev("1.2.3.4 dev"), None);
    assert_eq!(parse_route_dev("1.2.3.4 dev bad/name"), None);
    assert_eq!(parse_route_dev("1.2.3.4 dev 0123456789abcdef"), None);
}

// Regression: Flatpak has no `ip`, so every route read as "wired or unknown" (nightjar#80).
#[test]
fn the_local_address_finds_its_interface() {
    let addresses = [
        ("lo", IpAddr::from([127, 0, 0, 1])),
        ("lo", "::1".parse().unwrap()),
        ("eth0", IpAddr::from([192, 168, 1, 20])),
        ("wlan0", IpAddr::from([10, 0, 0, 7])),
        ("wlan0", "fe80::1234".parse().unwrap()),
        ("wg0", "fd00::2".parse().unwrap()),
    ];
    let find = |local: &str| interface_of_address(local.parse().unwrap(), addresses);
    assert_eq!(find("192.168.1.20"), Some("eth0"));
    assert_eq!(find("10.0.0.7"), Some("wlan0"));
    assert_eq!(find("fe80::1234"), Some("wlan0"));
    assert_eq!(find("fd00::2"), Some("wg0"));
    assert_eq!(find("::1"), Some("lo"));
    // A dual-stack socket's v4-mapped address is the IPv4 one, and the other way round.
    assert_eq!(find("::ffff:192.168.1.20"), Some("eth0"));
    let mapped: IpAddr = "::ffff:192.168.1.20".parse().unwrap();
    assert_eq!(interface_of_address(IpAddr::from([192, 168, 1, 20]), [("eth0", mapped)]), Some("eth0"));
    assert_eq!(find("192.168.1.21"), None);
    assert_eq!(find("::"), None);
}

#[cfg(target_os = "linux")]
#[test]
fn a_pc_on_this_machine_is_reached_over_loopback_which_does_not_tell() {
    assert_eq!(interface_name_toward(IpAddr::from([127, 0, 0, 1])).as_deref(), Ok("lo"));
    let lo = interface_toward(IpAddr::from([127, 0, 0, 1])).expect("loopback is readable");
    assert_eq!(lo.medium, Medium::Other);
    assert!(!lo.vpn);
}

#[test]
fn only_wired_and_wireless_media_decide_the_route() {
    assert_eq!(Toward::of(Medium::Wired, 2500), Some(Toward { wireless: false, link_mbps: 2500 }));
    // A wireless port's speed says nothing about the path.
    assert_eq!(Toward::of(Medium::Wireless, 866), Some(Toward { wireless: true, link_mbps: 0 }));
    assert_eq!(Toward::of(Medium::Other, 1000), None);
}

fn link(local: [u8; 4], ssid: Option<&str>, metered: bool) -> Link {
    Link { local: IpAddr::from(local), wifi: ssid.is_some(), cellular: false, ssid: ssid.map(Into::into), metered }
}

#[test]
fn the_start_network_and_its_metered_flag_alone_are_no_change() {
    let home = link([192, 168, 1, 20], Some("home"), false);
    let mut watch = LinkWatch::new(home.clone());
    assert_eq!(watch.observe(Some(home.clone())), None);
    // Marking the same connection metered starts no roam.
    assert_eq!(watch.observe(Some(Link { metered: true, ..home.clone() })), None);
    assert_eq!(watch.observe(Some(home)), None);
}

#[test]
fn a_new_route_or_wifi_is_one_hint_with_its_metered_flag() {
    let mut watch = LinkWatch::new(link([192, 168, 1, 20], None, false));
    // Unplugged: the route moves to Wi-Fi, reported once.
    let wifi = link([192, 168, 1, 21], Some("home"), false);
    assert_eq!(watch.observe(Some(wifi.clone())), Some(false));
    assert_eq!(watch.observe(Some(wifi)), None);
    // Another Wi-Fi with an address in the same range is still a new network.
    let hotspot = link([192, 168, 1, 21], Some("phone"), true);
    assert_eq!(watch.observe(Some(hotspot.clone())), Some(true));
    assert_eq!(watch.observe(Some(hotspot)), None);
    let cellular = Link { cellular: true, wifi: false, ssid: None, ..link([10, 64, 0, 9], None, true) };
    assert_eq!(watch.observe(Some(cellular)), Some(true));
}

#[test]
fn the_same_network_back_after_the_route_was_gone_is_a_change() {
    let home = link([192, 168, 1, 20], Some("home"), false);
    let mut watch = LinkWatch::new(home.clone());
    assert_eq!(watch.observe(None), None);
    assert_eq!(watch.observe(None), None);
    assert_eq!(watch.observe(Some(home.clone())), Some(false));
    assert_eq!(watch.observe(Some(home)), None);
}

// A Wi-Fi blip: the route goes (said once), comes back on the same network (a return the
// stream may not roam onto), then a new network without a loss first is no return.
#[test]
fn a_lost_route_is_said_once_and_its_return_is_marked() {
    let home = link([192, 168, 1, 20], Some("home"), false);
    let mut watch = LinkWatch::new(home.clone());
    assert_eq!(watch.event(None), Some(LinkEvent::Lost));
    assert_eq!(watch.event(None), None);
    assert_eq!(watch.event(Some(home.clone())), Some(LinkEvent::Changed { metered: false, after_loss: true }));
    assert_eq!(watch.event(Some(home)), None);
    let hotspot = link([172, 20, 10, 2], Some("phone"), true);
    assert_eq!(watch.event(Some(hotspot)), Some(LinkEvent::Changed { metered: true, after_loss: false }));
}

#[test]
fn the_local_address_toward_loopback_is_loopback() {
    assert_eq!(local_toward(SocketAddr::from(([127, 0, 0, 1], 47989))), Some(IpAddr::from([127, 0, 0, 1])));
}

#[test]
fn fallback_reasons_are_told_apart() {
    let reasons = [Fallback::Resolve, Fallback::Route, Fallback::NotTelling].map(|why| why.to_string());
    assert!(reasons[0] != reasons[1] && reasons[1] != reasons[2] && reasons[0] != reasons[2]);
}

fn on_wifi(link_mbps: u32) -> Network {
    Network { wifi: true, ssid: Some("Home".into()), link_mbps, ..Default::default() }
}

#[test]
fn the_watcher_reads_at_once_and_then_as_often_as_the_open_page_needs() {
    let start = std::time::Instant::now();
    let mut plan = Plan::new(true, start);
    assert_eq!(plan.due, Some(start), "the launcher opens with the network known");
    plan.read(&on_wifi(866), start);
    assert_eq!(plan.due, Some(start + IDLE_EVERY), "the system tells of changes, so the reads in between are few");
    // A page that shows the link's speed opens: read now, then as often as it always was.
    let opened = start + Duration::from_secs(7);
    plan.ask(&Ask::Pace(Pace::Live), opened);
    assert_eq!(plan.due, Some(opened));
    plan.read(&on_wifi(866), opened);
    assert_eq!(plan.due, Some(opened + LIVE_EVERY));
    // Asked for the pace it has, nothing moves.
    plan.ask(&Ask::Pace(Pace::Live), opened + Duration::from_secs(1));
    assert_eq!(plan.due, Some(opened + LIVE_EVERY));
    // The page closes: the read that was due still comes, and the long wait follows it.
    plan.ask(&Ask::Pace(Pace::Idle), opened + Duration::from_secs(2));
    assert_eq!(plan.due, Some(opened + LIVE_EVERY));
    plan.read(&on_wifi(866), opened + LIVE_EVERY);
    assert_eq!(plan.due, Some(opened + LIVE_EVERY + IDLE_EVERY));
}

#[test]
fn a_system_that_tells_of_no_changes_is_read_as_often_as_ever() {
    let start = std::time::Instant::now();
    let mut plan = Plan::new(false, start);
    plan.read(&on_wifi(866), start);
    assert_eq!(plan.due, Some(start + LIVE_EVERY), "nothing else would tell of another network for a minute");
    assert_eq!(LIVE_EVERY, Duration::from_secs(5), "the launcher's pace before there was a watcher");
}

#[test]
fn a_notice_from_the_system_is_read_soon_and_once_more_when_it_has_settled() {
    let start = std::time::Instant::now();
    let mut plan = Plan::new(true, start);
    plan.read(&on_wifi(866), start);
    // A change comes as a burst of notices, read as one.
    let told = start + Duration::from_secs(20);
    plan.ask(&Ask::Changed, told);
    assert_eq!(plan.due, Some(told + GATHER));
    plan.ask(&Ask::Changed, told + Duration::from_millis(50));
    plan.ask(&Ask::Changed, told + Duration::from_millis(120));
    assert_eq!(plan.due, Some(told + GATHER), "a later notice of the burst does not put the read off");
    let wired = Network { link_mbps: 2500, ..Default::default() };
    assert!(plan.read(&wired, told + GATHER));
    assert_eq!(plan.due, Some(told + GATHER + SETTLE), "the new connection may not have had its name and cost yet");
    assert!(!plan.read(&wired, told + GATHER + SETTLE));
    assert_eq!(plan.due, Some(told + GATHER + SETTLE + IDLE_EVERY), "and then the long wait again");
    // A read that is due sooner than the notice asks for stays where it is.
    let mut plan = Plan::new(true, start);
    plan.ask(&Ask::Changed, start);
    assert_eq!(plan.due, Some(start));
}

#[test]
fn the_watcher_reads_nothing_during_a_stream_and_at_once_after_it() {
    let start = std::time::Instant::now();
    let mut plan = Plan::new(true, start);
    plan.read(&on_wifi(866), start);
    plan.ask(&Ask::Pace(Pace::Paused), start + Duration::from_secs(1));
    assert_eq!(plan.due, None);
    // The stream has its own watcher for a network that changes under it.
    plan.ask(&Ask::Changed, start + Duration::from_secs(30));
    assert_eq!(plan.due, None);
    let ended = start + Duration::from_secs(3600);
    plan.ask(&Ask::Pace(Pace::Idle), ended);
    assert_eq!(plan.due, Some(ended), "the network may be another one after an hour's stream");
    // Back to a page that shows the speed is no different.
    plan.ask(&Ask::Pace(Pace::Paused), ended);
    plan.ask(&Ask::Pace(Pace::Live), ended + Duration::from_secs(9));
    assert_eq!(plan.due, Some(ended + Duration::from_secs(9)));
}

#[test]
fn only_an_answer_that_differs_from_the_last_is_news() {
    let start = std::time::Instant::now();
    let mut plan = Plan::new(true, start);
    assert!(plan.read(&on_wifi(866), start), "the first answer");
    assert!(!plan.read(&on_wifi(866), start));
    assert!(plan.read(&on_wifi(720), start), "the link's speed");
    assert!(plan.read(&Network { ssid: Some("Guest".into()), ..on_wifi(720) }, start), "the network's name");
    assert!(plan.read(&Network { metered: true, ssid: Some("Guest".into()), ..on_wifi(720) }, start), "its cost");
    assert!(plan.read(&Network { cellular: true, ..Default::default() }, start), "its kind");
    let failed = Network { error: Some("no answer".into()), ..Default::default() };
    assert!(plan.read(&failed, start), "a read that failed");
    assert!(!plan.read(&failed, start));
    assert!(plan.read(&on_wifi(866), start), "and the first answer again");
}

#[test]
fn the_watcher_hands_over_every_answer_and_wakes_the_launcher_only_for_news() {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};
    let long = Duration::from_secs(10);
    let answer = Arc::new(Mutex::new(Ok(on_wifi(866))));
    let (read_tx, reads) = channel();
    let (notices_tx, notices) = channel();
    let (woken_tx, woken) = channel();
    let wakes = Arc::new(AtomicU32::new(0));
    let reader = {
        let answer = answer.clone();
        move || {
            let _ = read_tx.send(());
            answer.lock().unwrap().clone()
        }
    };
    let waker: crate::hosts::Waker = {
        let wakes = wakes.clone();
        let woken_tx = Mutex::new(woken_tx);
        Arc::new(move || {
            wakes.fetch_add(1, Ordering::SeqCst);
            let _ = woken_tx.lock().unwrap().send(());
        })
    };
    let mut watch = Watch::spawn(
        reader,
        move |asks| {
            let _ = notices_tx.send(asks);
            Some(Box::new(()) as Box<dyn std::any::Any>)
        },
        waker,
    );
    // The system's notices would come through this, and it keeps the thread's channel open.
    let notice = notices.recv_timeout(long).expect("the watcher asks for the system's notices");

    // At the start, with nobody asking.
    woken.recv_timeout(long).expect("the first answer wakes the launcher");
    assert_eq!(watch.latest(), Some(on_wifi(866)));
    assert_eq!(watch.latest(), None, "an answer is handed over once");

    // A page that shows the speed reads at once. The same answer wakes nobody, and is there
    // all the same for the frame that comes next.
    watch.pace(Pace::Live);
    assert_eq!(watch.news.recv_timeout(long), Ok(on_wifi(866)));
    assert_eq!(wakes.load(Ordering::SeqCst), 1);

    // A stream: nothing is read. After it the network is another one, read at once.
    watch.pace(Pace::Paused);
    let mobile = Network { cellular: true, metered: true, ..Default::default() };
    *answer.lock().unwrap() = Ok(mobile.clone());
    watch.pace(Pace::Idle);
    woken.recv_timeout(long).expect("another network wakes the launcher");
    assert_eq!(watch.latest(), Some(mobile));
    assert_eq!(wakes.load(Ordering::SeqCst), 2, "the answer that was no news woke nobody");

    // The system tells of a change, and the read that follows fails.
    *answer.lock().unwrap() = Err("no answer".to_owned());
    notice.send(Ask::Changed).unwrap();
    woken.recv_timeout(long).expect("a notice is read within moments, not at the next minute");
    assert_eq!(watch.latest(), Some(Network { error: Some("no answer".into()), ..Default::default() }));

    // Closed with the launcher, though the system's notices still hold the channel open.
    drop(watch);
    loop {
        match reads.recv_timeout(long) {
            Ok(()) => {}
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => panic!("the watcher's thread outlived it"),
        }
    }
    drop(notice);
}

#[test]
fn the_pages_that_show_the_link_speed_are_the_ones_read_often() {
    use crate::ui::Screen;
    // The Profiles page prints "Link speed", and a PC's settings start the network test that
    // keeps the speed with its result.
    assert_eq!(Screen::Profiles.network_pace(), Pace::Live);
    assert_eq!(Screen::HostSettings(1).network_pace(), Pace::Live);
    for screen in [Screen::Home, Screen::AllGames, Screen::Game { host: 1, app: 2 }, Screen::Hosts, Screen::Apps(1), Screen::Settings] {
        assert_eq!(screen.network_pace(), Pace::Idle, "{screen:?}");
    }
}
