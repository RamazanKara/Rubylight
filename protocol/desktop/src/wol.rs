//! Wake-on-LAN: the magic packets that wake a PC, and [`WakeWait`], the clock that resends them
//! until the PC answers.

use std::collections::BTreeSet;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

use nightjar_core::http::native_api::wake::Wake;

use crate::config::HostRecord;

/// Why a PC without a usable MAC cannot be woken.
pub const NO_MAC: &str = "The PC has no valid Wake-on-LAN MAC address.";
/// A host lists one target per physical adapter; more than this is not a PC's worth.
const MAX_MACS: usize = 8;
const MAX_BROADCASTS: usize = 16;

fn magic_packet(mac: &str) -> Result<[u8; 102], String> {
    let bytes = mac.as_bytes();
    if bytes.len() != 17 || !matches!(bytes[2], b':' | b'-') {
        return Err(NO_MAC.into());
    }
    let mut address = [0; 6];
    for (i, octet) in address.iter_mut().enumerate() {
        let offset = i * 3;
        if !bytes[offset..offset + 2].iter().all(u8::is_ascii_hexdigit)
            || (i < 5 && bytes[offset + 2] != bytes[2])
        {
            return Err(NO_MAC.into());
        }
        *octet = u8::from_str_radix(&mac[offset..offset + 2], 16).unwrap();
    }
    let mut packet = [0xff; 102];
    for chunk in packet[6..].chunks_exact_mut(6) {
        chunk.copy_from_slice(&address);
    }
    Ok(packet)
}

/// Echo and discard, where magic packets classically go, the port Moonlight hosts listen on, and
/// the Moonlight ports that follow `http_port`.
fn ports(http_port: u16) -> Vec<u16> {
    let http_port = if http_port == 0 { 47989 } else { http_port };
    let mut ports = vec![7, 9, 47009];
    ports.extend(
        [9, 10, 11, 13, 21]
            .into_iter()
            .filter_map(|offset| http_port.checked_add(offset)),
    );
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// `mac` lowercase and colon-separated, or None when it is not a MAC or is the zero MAC a host
/// sends when it could not tell (it wakes nothing).
fn normal_mac(mac: &str) -> Option<String> {
    magic_packet(mac).ok()?;
    let normal = mac.replace('-', ":").to_ascii_lowercase();
    (normal != "00:00:00:00:00:00").then_some(normal)
}

/// What a host's `GET /wake` adds to serverinfo's MAC, for [`HostRecord::wake_macs`] and
/// [`HostRecord::wake_broadcasts`]: its adapters' MACs and their subnet broadcasts, well formed
/// and without repeats.
pub fn targets(wake: &Wake) -> (Vec<String>, Vec<String>) {
    let mut macs = Vec::new();
    let mut broadcasts = Vec::new();
    let listed = wake
        .primary
        .iter()
        .chain(wake.targets.iter().map(|t| &t.mac));
    for mac in listed.filter_map(|mac| normal_mac(mac)) {
        if macs.len() < MAX_MACS && !macs.contains(&mac) {
            macs.push(mac);
        }
    }
    for broadcast in wake.targets.iter().flat_map(|t| &t.broadcasts) {
        // A literal only: a name here would need a DNS lookup while the PC sleeps.
        let Ok(address) = broadcast.parse::<Ipv4Addr>() else {
            continue;
        };
        let address = address.to_string();
        if broadcasts.len() < MAX_BROADCASTS && !broadcasts.contains(&address) {
            broadcasts.push(address);
        }
    }
    (macs, broadcasts)
}

/// This device's side of a wake: the subnet broadcast of each IPv4 adapter, and the interface
/// index of each IPv6 one, for `ff02::1` on its link.
#[derive(Default)]
struct Links {
    broadcasts: BTreeSet<Ipv4Addr>,
    ipv6: BTreeSet<u32>,
}

impl Links {
    fn read() -> Self {
        let mut links = Self::default();
        match if_addrs::get_if_addrs() {
            Ok(interfaces) => {
                for interface in interfaces
                    .iter()
                    .filter(|i| i.is_oper_up() && !i.is_loopback())
                {
                    match &interface.addr {
                        if_addrs::IfAddr::V4(v4) => links.broadcasts.extend(v4.broadcast),
                        if_addrs::IfAddr::V6(_) => links.ipv6.extend(interface.index),
                    }
                }
            }
            // Without the list, the PC's addresses and 255.255.255.255 still get their packets.
            Err(e) => eprintln!("nightjar: Wake-on-LAN cannot list the network interfaces: {e}"),
        }
        links
    }
}

/// What one wake sends: each payload (one per MAC) to every destination, and to the best-effort ones.
struct Plan {
    payloads: Vec<[u8; 102]>,
    destinations: BTreeSet<SocketAddr>,
    /// `ff02::1` on each IPv6 link; sends there may all fail without failing the wake.
    best_effort: BTreeSet<SocketAddr>,
}

/// The packets for the PC: one payload for serverinfo's MAC and each MAC its `/wake` listed, to
/// the PC's addresses, to 255.255.255.255, to the subnet broadcasts of this device and of the PC,
/// and to `ff02::1`. Resolving the PC's addresses may block on DNS.
fn plan(host: &HostRecord, links: &Links) -> Result<Plan, String> {
    let mut macs: Vec<String> = Vec::new();
    for mac in host
        .mac
        .iter()
        .chain(&host.wake_macs)
        .filter_map(|mac| normal_mac(mac))
    {
        if !macs.contains(&mac) {
            macs.push(mac);
        }
    }
    let payloads: Vec<_> = macs
        .iter()
        .filter_map(|mac| magic_packet(mac).ok())
        .collect();
    if payloads.is_empty() {
        return Err(NO_MAC.into());
    }
    let ports = ports(host.http_port);
    let mut addresses = BTreeSet::new();
    for address in std::iter::once(&host.address).chain(&host.other_addresses) {
        match (address.as_str(), 9).to_socket_addrs() {
            Ok(resolved) => addresses.extend(resolved.map(|address| address.ip())),
            Err(e) => eprintln!("nightjar: Wake-on-LAN skips {address}: {e}"),
        }
    }
    // Directed broadcasts reach each subnet and avoid stale ARP entries for sleeping PCs.
    addresses.insert(IpAddr::V4(Ipv4Addr::BROADCAST));
    addresses.extend(links.broadcasts.iter().copied().map(IpAddr::V4));
    addresses.extend(
        host.wake_broadcasts
            .iter()
            .filter_map(|b| b.parse::<Ipv4Addr>().ok())
            .map(IpAddr::V4),
    );
    let destinations = addresses
        .iter()
        .flat_map(|address| ports.iter().map(|port| SocketAddr::new(*address, *port)))
        .collect();
    let best_effort = links
        .ipv6
        .iter()
        .flat_map(|scope| {
            ports.iter().map(|port| {
                SocketAddr::V6(SocketAddrV6::new(
                    Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1),
                    *port,
                    0,
                    *scope,
                ))
            })
        })
        .collect();
    Ok(Plan {
        payloads,
        destinations,
        best_effort,
    })
}

/// Sends the plan through `send`; fails only when no packet reached a destination that is not
/// best effort.
fn send(
    plan: &Plan,
    mut send: impl FnMut(&[u8], SocketAddr) -> io::Result<()>,
) -> Result<(), String> {
    let mut sent = false;
    let mut error = String::from("No Wake-on-LAN destination could be reached.");
    let mut best_effort_failures = 0;
    // A magic packet has no acknowledgement; a short burst tolerates an isolated UDP loss.
    for _ in 0..3 {
        for payload in &plan.payloads {
            for destination in &plan.destinations {
                match send(payload, *destination) {
                    Ok(()) => sent = true,
                    Err(e) => error = e.to_string(),
                }
            }
            for destination in &plan.best_effort {
                if send(payload, *destination).is_err() {
                    best_effort_failures += 1;
                }
            }
        }
    }
    if best_effort_failures > 0 {
        eprintln!("nightjar: Wake-on-LAN: {best_effort_failures} IPv6 all-nodes sends failed");
    }
    if sent {
        Ok(())
    } else {
        Err(error)
    }
}

/// Sends the magic packets for the PC (blocks): see [`plan`]. Fails when the PC has no usable
/// MAC, or when no packet could be sent at all.
pub fn wake(host: &HostRecord) -> Result<(), String> {
    let plan = plan(host, &Links::read())?;
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).map_err(|e| e.to_string())?;
    socket.set_broadcast(true).map_err(|e| e.to_string())?;
    let ipv6 = UdpSocket::bind("[::]:0").ok();
    send(&plan, |payload, destination| {
        let socket = if destination.is_ipv4() {
            &socket
        } else {
            ipv6.as_ref()
                .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "no IPv6 socket"))?
        };
        socket.send_to(payload, destination).map(|_| ())
    })
}

/// What [`WakeWait::next`] asks for on this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing yet: the packet went out lately.
    Wait,
    /// Send a wake packet now.
    Send,
    /// The PC answers.
    Online,
    /// The PC did not wake in time.
    TimedOut,
}

/// Waking a PC as a clock-driven decision: send a wake packet at once, send it again every
/// [`WakeWait::RESEND`] while the PC stays away, and give up after [`WakeWait::TIMEOUT`]. It holds
/// no thread and reads no clock, so the timing is tested with plain instants.
pub struct WakeWait {
    start: Instant,
    timeout: Duration,
    resend: Duration,
    last_sent: Option<Instant>,
}

impl WakeWait {
    /// A resume from sleep takes well under this on the hardware we know of; longer counts as a failed wake.
    pub const TIMEOUT: Duration = Duration::from_secs(90);
    pub const RESEND: Duration = Duration::from_secs(8);

    pub fn new(start: Instant) -> Self {
        Self::with(start, Self::TIMEOUT, Self::RESEND)
    }

    fn with(start: Instant, timeout: Duration, resend: Duration) -> Self {
        Self {
            start,
            timeout,
            resend,
            last_sent: None,
        }
    }

    /// An answering PC wins over everything, including the time that ran out in the same tick.
    pub fn next(&mut self, online: bool, now: Instant) -> Step {
        if online {
            return Step::Online;
        }
        if now.saturating_duration_since(self.start) >= self.timeout {
            return Step::TimedOut;
        }
        if self
            .last_sent
            .is_none_or(|sent| now.saturating_duration_since(sent) >= self.resend)
        {
            self.last_sent = Some(now);
            return Step::Send;
        }
        Step::Wait
    }
}

/// The PC being woken, if any. Each wake is its own run and its sends carry the run, so a send of
/// a cancelled wake that fails late cannot end a new wake of the same PC.
#[derive(Default)]
pub struct Wakes {
    current: Option<Waking>,
    runs: u64,
}

pub struct Waking {
    pub host: u64,
    pub run: u64,
    pub wait: WakeWait,
}

impl Wakes {
    /// Starts waking `host` unless a wake is already running; says whether it started.
    pub fn start(&mut self, host: u64, now: Instant) -> bool {
        if self.current.is_some() {
            return false;
        }
        self.runs += 1;
        self.current = Some(Waking {
            host,
            run: self.runs,
            wait: WakeWait::new(now),
        });
        true
    }

    pub fn current(&mut self) -> Option<&mut Waking> {
        self.current.as_mut()
    }

    pub fn host(&self) -> Option<u64> {
        self.current.as_ref().map(|w| w.host)
    }

    /// Cancel, an answering PC, the time running out: the wake is over.
    pub fn end(&mut self) {
        self.current = None;
    }

    /// A send of `run` to `host` failed. Only the running wake ends on it; says whether it did.
    pub fn failed(&mut self, host: u64, run: u64) -> bool {
        let ends = self
            .current
            .as_ref()
            .is_some_and(|w| w.host == host && w.run == run);
        if ends {
            self.current = None;
        }
        ends
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nightjar_core::http::native_api::wake::Target;

    #[test]
    fn packet_repeats_the_mac_sixteen_times() {
        let packet = magic_packet("00:1a:2B:3c:4D:5e").unwrap();
        assert_eq!(&packet[..6], &[255; 6]);
        for chunk in packet[6..].chunks_exact(6) {
            assert_eq!(chunk, &[0, 0x1a, 0x2b, 0x3c, 0x4d, 0x5e]);
        }
        assert_eq!(magic_packet("00-1a-2b-3c-4d-5e").unwrap(), packet);
        for mac in [
            "",
            "00:1a:2b:3c:4d",
            "00:1a-2b:3c:4d:5e",
            "00:1a:2b:3c:4d:zz",
            "é0:1a:2b:3c:4d:5",
        ] {
            assert!(magic_packet(mac).is_err());
        }
    }

    #[test]
    fn custom_ports_keep_fixed_wake_ports_and_do_not_wrap() {
        assert_eq!(ports(0), [7, 9, 47009, 47998, 47999, 48000, 48002, 48010]);
        assert_eq!(
            ports(48989),
            [7, 9, 47009, 48998, 48999, 49000, 49002, 49010]
        );
        assert_eq!(ports(65535), [7, 9, 47009]);
    }

    fn host(mac: Option<&str>, wake_macs: &[&str], wake_broadcasts: &[&str]) -> HostRecord {
        HostRecord {
            address: "192.168.4.20".into(),
            other_addresses: vec!["10.0.0.20".into()],
            mac: mac.map(Into::into),
            wake_macs: wake_macs.iter().map(|m| m.to_string()).collect(),
            wake_broadcasts: wake_broadcasts.iter().map(|b| b.to_string()).collect(),
            ..Default::default()
        }
    }

    fn links() -> Links {
        Links {
            broadcasts: BTreeSet::from([Ipv4Addr::new(192, 168, 4, 255)]),
            ipv6: BTreeSet::from([7]),
        }
    }

    fn v4(address: &str, port: u16) -> SocketAddr {
        SocketAddr::new(address.parse().unwrap(), port)
    }

    #[test]
    fn two_macs_go_to_every_address_broadcast_and_port() {
        let host = host(
            Some("02-7C-16-B4-13-4F"),
            &["02:7c:16:b4:13:4f", "0a:00:27:00:00:05"],
            &["10.0.0.255", "192.168.4.255"],
        );
        let plan = plan(&host, &links()).unwrap();
        assert_eq!(
            plan.payloads,
            [
                magic_packet("02:7c:16:b4:13:4f").unwrap(),
                magic_packet("0a:00:27:00:00:05").unwrap()
            ]
        );
        let addresses: BTreeSet<_> = plan
            .destinations
            .iter()
            .map(|d| d.ip().to_string())
            .collect();
        assert_eq!(
            addresses,
            [
                "10.0.0.20",
                "10.0.0.255",
                "192.168.4.20",
                "192.168.4.255",
                "255.255.255.255"
            ]
            .map(String::from)
            .into()
        );
        for address in &addresses {
            for port in ports(0) {
                assert!(
                    plan.destinations.contains(&v4(address, port)),
                    "{address}:{port}"
                );
            }
        }
        assert_eq!(plan.destinations.len(), addresses.len() * ports(0).len());
        let all_nodes: Vec<_> = plan.best_effort.iter().copied().collect();
        assert_eq!(all_nodes.len(), ports(0).len());
        for destination in all_nodes {
            let SocketAddr::V6(v6) = destination else {
                panic!("{destination}")
            };
            assert_eq!(
                (v6.ip().to_string(), v6.scope_id()),
                ("ff02::1".to_string(), 7)
            );
        }
    }

    #[test]
    fn a_host_without_wake_targets_gets_serverinfos_mac_only() {
        let plan = plan(
            &host(Some("00:1a:2b:3c:4d:5e"), &[], &[]),
            &Links::default(),
        )
        .unwrap();
        assert_eq!(plan.payloads, [magic_packet("00:1a:2b:3c:4d:5e").unwrap()]);
        assert!(plan.destinations.contains(&v4("255.255.255.255", 9)));
        assert!(plan.best_effort.is_empty());
    }

    #[test]
    fn no_usable_mac_is_no_plan() {
        assert_eq!(
            plan(&host(None, &[], &["10.0.0.255"]), &links())
                .err()
                .as_deref(),
            Some(NO_MAC)
        );
        assert_eq!(
            plan(&host(Some("00:00:00:00:00:00"), &["nope"], &[]), &links())
                .err()
                .as_deref(),
            Some(NO_MAC)
        );
        // A MAC from `/wake` wakes the PC even when serverinfo's is not usable.
        assert_eq!(
            plan(&host(Some("nope"), &["0a:00:27:00:00:05"], &[]), &links())
                .unwrap()
                .payloads
                .len(),
            1
        );
    }

    #[test]
    fn failed_all_nodes_sends_do_not_fail_the_wake_but_they_do_not_carry_it_either() {
        let plan = plan(
            &host(Some("00:1a:2b:3c:4d:5e"), &["0a:00:27:00:00:05"], &[]),
            &links(),
        )
        .unwrap();
        let mut sends = 0;
        let only_v4 = send(&plan, |_, destination| {
            sends += 1;
            if destination.is_ipv4() {
                Ok(())
            } else {
                Err(io::Error::other("no route"))
            }
        });
        assert_eq!(only_v4, Ok(()));
        assert_eq!(
            sends,
            3 * 2 * (plan.destinations.len() + plan.best_effort.len())
        );
        let only_v6 = send(&plan, |_, destination| {
            if destination.is_ipv6() {
                Ok(())
            } else {
                Err(io::Error::other("network is unreachable"))
            }
        });
        assert_eq!(only_v6.err().as_deref(), Some("network is unreachable"));
    }

    #[test]
    fn wake_targets_are_normal_and_bounded() {
        let target = |mac: &str, broadcasts: &[&str]| {
            Target {
            mac: mac.into(),
            broadcasts: broadcasts.iter().map(|b| b.to_string()).collect(),
            ..serde_json::from_str(r#"{"mac":"","kind":"ethernet","connected":true,"route":true,"broadcasts":[],"state":"ready","checks":[]}"#).unwrap()
        }
        };
        let mut wake: Wake = serde_json::from_str(
            r#"{"revision":1,"primary":"02-7C-16-B4-13-4F","targets":[],"system":{"sleep":"s3","modern_standby":false,"hibernate":false,"fast_startup":"inactive","shutdown_wake":"firmware_unknown"},"idle_sleep":{"state":"off","minutes":0,"reason":null}}"#,
        )
        .unwrap();
        wake.targets = vec![
            target(
                "02:7c:16:b4:13:4f",
                &["192.168.4.255", "pc.lan", "192.168.4.255"],
            ),
            target("00:00:00:00:00:00", &["10.0.0.255"]),
            target("bad", &["300.0.0.1"]),
        ];
        wake.targets
            .extend((0..10).map(|i| target(&format!("0a:00:27:00:00:{i:02x}"), &[])));
        let (macs, broadcasts) = targets(&wake);
        assert_eq!(macs.len(), MAX_MACS);
        assert_eq!(&macs[..2], ["02:7c:16:b4:13:4f", "0a:00:27:00:00:00"]);
        assert_eq!(broadcasts, ["192.168.4.255", "10.0.0.255"]);
    }

    fn ms(at: u64) -> Duration {
        Duration::from_millis(at)
    }

    #[test]
    fn wake_wait_sends_at_once_then_waits_between_resends() {
        let t = Instant::now();
        let mut wait = WakeWait::with(t + ms(1_000), ms(60_000), ms(8_000));
        assert_eq!(wait.next(false, t + ms(1_000)), Step::Send);
        assert_eq!(wait.next(false, t + ms(2_000)), Step::Wait);
        assert_eq!(wait.next(false, t + ms(8_999)), Step::Wait);
        assert_eq!(wait.next(false, t + ms(9_000)), Step::Send);
        assert_eq!(wait.next(false, t + ms(10_000)), Step::Wait);
        assert_eq!(wait.next(false, t + ms(17_000)), Step::Send);
    }

    #[test]
    fn wake_wait_gives_up_when_the_time_is_up() {
        let t = Instant::now();
        let mut wait = WakeWait::with(t, ms(60_000), ms(8_000));
        assert_eq!(wait.next(false, t), Step::Send);
        assert_eq!(wait.next(false, t + ms(7_000)), Step::Wait);
        assert_eq!(wait.next(false, t + ms(60_000)), Step::TimedOut);
    }

    #[test]
    fn wake_wait_an_answering_pc_wins_even_in_the_tick_the_time_ran_out() {
        let t = Instant::now();
        let mut wait = WakeWait::with(t, ms(60_000), ms(8_000));
        assert_eq!(wait.next(true, t), Step::Online);
        assert_eq!(wait.next(true, t + ms(60_000)), Step::Online);
    }

    #[test]
    fn wake_wait_defaults_give_a_resume_from_sleep_ninety_seconds() {
        let t = Instant::now();
        let mut wait = WakeWait::new(t);
        assert_eq!(wait.next(false, t), Step::Send);
        assert_eq!(wait.next(false, t + ms(7_999)), Step::Wait);
        assert_eq!(wait.next(false, t + WakeWait::RESEND), Step::Send);
        let mut at = 2 * WakeWait::RESEND;
        while at < WakeWait::TIMEOUT {
            assert_eq!(wait.next(false, t + at), Step::Send);
            at += WakeWait::RESEND;
        }
        assert_eq!(wait.next(false, t + WakeWait::TIMEOUT - ms(1)), Step::Wait);
        assert_eq!(wait.next(false, t + WakeWait::TIMEOUT), Step::TimedOut);
    }

    #[test]
    fn a_late_error_of_a_cancelled_wake_does_not_end_the_next_wake_of_the_same_pc() {
        let t = Instant::now();
        let mut wakes = Wakes::default();
        assert!(wakes.start(7, t));
        let old = wakes.current().unwrap().run;
        wakes.end();
        assert!(wakes.start(7, t + ms(1_000)));
        let new = wakes.current().unwrap().run;
        assert_ne!(old, new);

        assert!(!wakes.failed(7, old));
        assert_eq!(wakes.host(), Some(7));
        assert!(!wakes.failed(8, new));
        assert_eq!(wakes.host(), Some(7));
        assert!(wakes.failed(7, new));
        assert_eq!(wakes.host(), None);
    }

    #[test]
    fn a_second_wake_waits_for_the_first_to_end() {
        let t = Instant::now();
        let mut wakes = Wakes::default();
        assert!(wakes.start(7, t));
        assert!(!wakes.start(8, t));
        assert_eq!(wakes.host(), Some(7));
    }
}
