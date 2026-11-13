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
    for chunk in packet[6..].as_chunks_mut::<6>().0 {
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

/// The MACs typed into a PC's own wake addresses, separated by commas, spaces or lines, as
/// `00:11:22:33:44:55`, `00-11-22-33-44-55` or `001122334455`; each once. None when one of them is
/// not a MAC address, is all zeros, or there are more than [`MAX_MACS`].
pub fn parse_macs(text: &str) -> Option<Vec<String>> {
    let mut macs = Vec::new();
    for token in text.split(|c: char| c.is_whitespace() || c == ',' || c == ';').filter(|t| !t.is_empty()) {
        let spaced;
        let token = if token.len() == 12 && token.bytes().all(|b| b.is_ascii_hexdigit()) {
            spaced = (0..6).map(|i| &token[i * 2..i * 2 + 2]).collect::<Vec<_>>().join(":");
            spaced.as_str()
        } else {
            token
        };
        let mac = normal_mac(token)?;
        if macs.contains(&mac) {
            continue;
        }
        if macs.len() >= MAX_MACS {
            return None;
        }
        macs.push(mac);
    }
    Some(macs)
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
        .chain(&host.own_wake_macs)
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
mod tests;
