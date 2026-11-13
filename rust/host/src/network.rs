use crate::state::Shared;
use anyhow::Result;
use axum::http::{HeaderMap, Uri, header, uri::Authority};
use butterpollo_core::config::{Config, Ports};
use igd_next::{
    PortMappingProtocol as Protocol,
    aio::{Gateway, Provider},
};
use mdns_sd::{IfKind, ServiceDaemon, ServiceInfo};
use socket2::{Domain, Socket, Type};
use std::{
    net::{IpAddr, SocketAddr, UdpSocket},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
#[cfg(test)]
mod fixtures;
mod ipv6;
mod portmap;
mod upnp;

/// Whether a connection or datagram from `peer` came through an internet play tunnel. The tunnel
/// reaches the host's ports from this PC (a loopback address of the device's own, or the address
/// the host listens on), so its peers would otherwise look like programs on this PC. Every trust
/// decision that favours this PC or the local network asks this first: a tunnelled device gets
/// what a device on the internet gets.
pub fn tunnelled(peer: SocketAddr) -> bool {
    rubylight_traverse::peers::is_tunnelled(peer)
}

/// Whether a connection from `peer` is a program on this PC: over loopback, and not through an
/// internet play tunnel.
pub fn from_this_pc(peer: SocketAddr) -> bool {
    peer.ip().to_canonical().is_loopback() && !tunnelled(peer)
}

/// Whether a connection from `peer` to `local` comes from this PC: over loopback, or to one of
/// this PC's own addresses, which it then comes from too. Never through an internet play tunnel.
fn peer_is_this_pc(peer: SocketAddr, local: IpAddr) -> bool {
    if tunnelled(peer) {
        return false;
    }
    let ip = peer.ip().to_canonical();
    ip.is_loopback() || ip == local.to_canonical()
}

/// Whether the browser behind this request runs on this PC. The address can only say it might:
/// nginx, cloudflared, `tailscale serve`, `ssh -L` and `netsh portproxy` on this PC all connect
/// from here for a browser anywhere. So the request's headers can take it back. They only ever
/// restore the stricter behaviour, so a browser that sends them gains nothing.
///
/// What adds no header and shows the browser the same address and port still can't be told apart:
/// `ssh -L 47990:localhost:47990`, or a raw port proxy on the same port, opened at `localhost`.
pub fn browser_on_this_pc(peer: SocketAddr, local: SocketAddr, headers: &HeaderMap) -> bool {
    peer_is_this_pc(peer, local.ip())
        && !request_names_elsewhere(
            headers,
            local,
            butterpollo_windows::net::host_name().as_deref(),
        )
}

/// Whether a request connected to `local` shows it came through something: a proxy's own header, a
/// `Host` at another port than the one it connected to, or a `Host` or `Origin` that isn't this PC.
/// A browser on this PC that opened the page at the PC's network address connects to that address,
/// so it names `local`. A missing header says nothing. A `Host` without a port means the scheme's
/// default, so it only fits a host listening on 80 or 443. The `Origin`'s port isn't compared: a
/// page may signal to another port than it came from.
fn request_names_elsewhere(headers: &HeaderMap, local: SocketAddr, pc_name: Option<&str>) -> bool {
    let forwarded = headers
        .keys()
        .any(|name| name == "forwarded" || name.as_str().starts_with("x-forwarded-"));
    let host_elsewhere = headers.get_all(header::HOST).iter().any(|host| {
        let authority = host.to_str().ok();
        let authority = authority.and_then(|host| host.parse::<Authority>().ok());
        !authority.is_some_and(|authority| {
            authority
                .port_u16()
                .map_or(matches!(local.port(), 80 | 443), |port| {
                    port == local.port()
                })
                && names_this_pc(authority.host(), local.ip(), pc_name)
        })
    });
    let origin_elsewhere = headers.get_all(header::ORIGIN).iter().any(|origin| {
        let origin = origin.to_str().ok();
        let origin = origin.and_then(|origin| origin.parse::<Uri>().ok());
        !origin
            .as_ref()
            .and_then(Uri::host)
            .is_some_and(|host| names_this_pc(host, local.ip(), pc_name))
    });
    forwarded || host_elsewhere || origin_elsewhere
}

/// Whether a request's host names this PC: loopback, the address the connection reached, or the
/// PC's own name. A longer name (`pc.tailnet.ts.net`, a domain's) may be a proxy's, so it doesn't
/// count, and such a stream keeps the configured layout.
fn names_this_pc(host: &str, local: IpAddr, pc_name: Option<&str>) -> bool {
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if let Ok(address) = host.parse::<IpAddr>() {
        let address = address.to_canonical();
        return address.is_loopback() || address == local.to_canonical();
    }
    host == "localhost"
        || host.ends_with(".localhost")
        || pc_name.is_some_and(|name| {
            let name = name.to_ascii_lowercase();
            host == name || host.strip_suffix(".local") == Some(name.as_str())
        })
}

/// The address a WebRTC stream's media socket binds and offers: the one the browser's signalling
/// reached, which is known to work from there. A browser on this PC that opened the page at
/// `localhost` signals over loopback, and not every browser sends media there: Firefox makes no
/// candidate pair with a loopback address, so its connection fails after 5 s without one check.
/// That browser gets `lan`, this PC's own address, which it reaches without leaving the PC. A host
/// bound to loopback stays there, and so does a PC with no network.
pub fn media_address(
    signalled: IpAddr,
    bind: IpAddr,
    lan: impl FnOnce() -> Option<IpAddr>,
) -> IpAddr {
    let signalled = signalled.to_canonical();
    if signalled.is_loopback() && bind.is_unspecified() {
        return lan().unwrap_or(signalled);
    }
    signalled
}

/// This PC's own address on the network, for [`media_address`]; None without a network.
pub fn this_pc_address() -> Option<IpAddr> {
    let addresses = butterpollo_windows::net::lan_addresses().unwrap_or_else(|error| {
        tracing::warn!(error = %format!("{error:#}"), "the LAN addresses for a WebRTC stream could not be read");
        Vec::new()
    });
    addresses
        .iter()
        .find_map(|address| address.parse().ok())
        // `lan_addresses` lists IPv4 only. A UDP connect sends nothing; it only picks the route.
        .or_else(|| {
            let probe = UdpSocket::bind((IpAddr::from([0u16; 8]), 0)).ok()?;
            probe.connect(("2001:4860:4860::8888", 9)).ok()?;
            let address = probe.local_addr().ok()?.ip();
            let usable = !address.is_loopback()
                && !address.is_unspecified()
                && !matches!(address, IpAddr::V6(ip) if ip.is_unicast_link_local());
            usable.then_some(address)
        })
}

/// Where a peer is, by address, as Vibepollo classifies it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reach {
    /// This computer.
    Pc,
    /// Private, link-local and carrier-grade NAT (Tailscale) addresses.
    Lan,
    Wan,
}
/// Internet play tunnels' loopback ranges (127.77.0.0/16, 127.78.0.0/16) are the internet: a
/// registered tunnel peer outside them needs its port as well, so callers that have it ask
/// [`tunnelled`] (or [`peer_reach`]) too.
pub fn reach(address: IpAddr) -> Reach {
    match address.to_canonical() {
        ip if rubylight_traverse::peers::tunnel_range(ip) => Reach::Wan,
        IpAddr::V4(ip) if ip.is_loopback() => Reach::Pc,
        IpAddr::V4(ip)
            if ip.is_private()
                || ip.is_link_local()
                || (ip.octets()[0] == 100 && ip.octets()[1] & 0xc0 == 64) =>
        {
            Reach::Lan
        }
        IpAddr::V6(ip) if ip.is_loopback() => Reach::Pc,
        IpAddr::V6(ip) if ip.is_unique_local() || ip.is_unicast_link_local() => Reach::Lan,
        _ => Reach::Wan,
    }
}
/// The farthest peers allowed to use the web console and API.
/// [`reach`] of a connection's or datagram's peer: the internet when it came through a tunnel.
pub fn peer_reach(peer: SocketAddr) -> Reach {
    if tunnelled(peer) {
        Reach::Wan
    } else {
        reach(peer.ip())
    }
}
pub fn web_reach(config: &Config) -> Reach {
    match config.get("origin_web_ui_allowed", "lan") {
        "wan" => Reach::Wan,
        "lan" => Reach::Lan,
        _ => Reach::Pc,
    }
}
/// Carrier-grade NAT space, 100.64.0.0/10: Tailscale's addresses, and also the WAN side of a
/// router behind an ISP's NAT, whose other customers connect from it.
fn shared_address_space(address: IpAddr) -> bool {
    matches!(address.to_canonical(), IpAddr::V4(ip) if ip.octets()[0] == 100 && ip.octets()[1] & 0xc0 == 64)
}
/// Why `/play` refuses a browser at `peer` that reached this host on `local`, if it does, given
/// the PC's default gateways. Only the TCP peer counts. A forwarded guest port can make an
/// internet peer look local: a CGNAT neighbour's 100.64/10 address arrives on the LAN address
/// (real Tailscale traffic arrives on the Tailscale address), and a router that source-NATs its
/// forwards hands over its own address.
pub fn play_refusal(peer: IpAddr, local: IpAddr, gateways: &[IpAddr]) -> Option<&'static str> {
    let peer = peer.to_canonical();
    if reach(peer) > Reach::Lan {
        return Some("the browser is outside the local network");
    }
    if shared_address_space(peer) && !shared_address_space(local) {
        return Some("a 100.64/10 browser reached a non-Tailscale address of this PC");
    }
    if gateways
        .iter()
        .any(|gateway| gateway.to_canonical() == peer)
    {
        return Some("the browser connects from the default gateway, which forwards the internet");
    }
    None
}
/// [`play_refusal`] with this PC's default gateways, read only for a peer the address checks
/// alone let in. A routing table that cannot be read refuses: the gateway check would be skipped.
pub fn play_refused(peer: SocketAddr, local: SocketAddr) -> Option<String> {
    if tunnelled(peer) {
        return Some("the browser is outside the local network".to_owned());
    }
    if let Some(reason) = play_refusal(peer.ip(), local.ip(), &[]) {
        return Some(reason.to_owned());
    }
    match default_gateways() {
        Ok(gateways) => play_refusal(peer.ip(), local.ip(), &gateways).map(str::to_owned),
        Err(error) => Some(format!("the default gateway could not be read: {error:#}")),
    }
}
#[cfg(not(test))]
fn default_gateways() -> Result<Vec<IpAddr>> {
    butterpollo_windows::net::default_gateways()
}
/// The router of the test fixtures' 192.168.4.0/24, rather than this PC's.
#[cfg(test)]
pub const TEST_GATEWAY: &str = "192.168.4.1";
#[cfg(test)]
thread_local! {
    /// How often this thread read the gateways.
    static GATEWAY_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
#[cfg(test)]
fn default_gateways() -> Result<Vec<IpAddr>> {
    GATEWAY_READS.with(|reads| reads.set(reads.get() + 1));
    Ok(vec![TEST_GATEWAY.parse()?])
}
/// The encryption a client at `peer` needs: `lan_encryption_mode` on this PC and the local
/// network, `wan_encryption_mode` elsewhere, including through an internet play tunnel.
pub fn encryption_mode(config: &Config, peer: SocketAddr) -> u32 {
    let lan = peer_reach(peer) <= Reach::Lan;
    config
        .integer(
            if lan {
                "lan_encryption_mode"
            } else {
                "wan_encryption_mode"
            },
            if lan { 0 } else { 1 },
        )
        .clamp(0, 2) as u32
}
pub fn ping_timeout(config: &Config) -> Duration {
    Duration::from_millis(config.integer("ping_timeout", 10000).clamp(1000, 300000) as u64)
}

/// The name clients show for this host: `sunshine_name`, else the PC's name.
pub fn host_name(config: &Config) -> String {
    let configured = config.get("sunshine_name", "").trim();
    if !configured.is_empty() {
        return configured.to_owned();
    }
    pc_name().unwrap_or_else(|| "Rubylight".into())
}
/// The PC's DNS host name, read from Windows once: a new name takes a restart of
/// Windows. Every `/serverinfo` asked for it. A failed read is tried again.
fn pc_name() -> Option<String> {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    if let Some(name) = NAME.get() {
        return Some(name.clone());
    }
    let name = butterpollo_windows::net::host_name()?;
    Some(NAME.get_or_init(|| name).clone())
}
pub fn bind_address(config: &Config, override_address: Option<IpAddr>) -> Result<IpAddr> {
    if let Some(address) = override_address {
        return Ok(address);
    }
    let configured = config.get("bind_address", "");
    if !configured.is_empty() {
        // A value that is no address at all (an interface name) would keep
        // the host, and the console to fix it, down: this PC only until it is
        // corrected. A valid address that is not up yet, as at boot, is kept:
        // the service retries until the network has it.
        return Ok(
            match configured.trim().trim_matches('"').parse::<IpAddr>() {
                Ok(address) => address,
                Err(_) => {
                    let fallback = IpAddr::from([127, 0, 0, 1]);
                    tracing::warn!(
                        bind_address = configured,
                        %fallback,
                        "bind_address is not an IP address; listening on this PC only"
                    );
                    fallback
                }
            },
        );
    }
    match config.get("address_family", "ipv4") {
        "both" => Ok(IpAddr::from([0u16; 8])),
        other => {
            butterpollo_core::config::fallback("address_family", other, "ipv4");
            Ok(IpAddr::from([0, 0, 0, 0]))
        }
    }
}
fn socket(address: SocketAddr, kind: Type) -> Result<Socket> {
    let socket = Socket::new(Domain::for_address(address), kind, None)?;
    if address.is_ipv6() {
        socket.set_only_v6(false)?;
    }
    socket.bind(&address.into())?;
    socket.set_nonblocking(true)?;
    Ok(socket)
}
pub fn udp(address: SocketAddr) -> Result<UdpSocket> {
    Ok(socket(address, Type::DGRAM)?.into())
}
pub fn tcp(address: SocketAddr) -> Result<tokio::net::TcpListener> {
    let socket = socket(address, Type::STREAM)?;
    socket.listen(128)?;
    Ok(tokio::net::TcpListener::from_std(socket.into())?)
}
/// Accepts the next connection. A failed accept belongs to one connection
/// (a client resetting mid-handshake) or is transient (out of handles), so
/// it never ends the listener, which would shut the host and its game down.
pub async fn accept(listener: &tokio::net::TcpListener) -> (tokio::net::TcpStream, SocketAddr) {
    loop {
        match listener.accept().await {
            Ok((socket, peer)) => {
                return (
                    socket,
                    SocketAddr::new(peer.ip().to_canonical(), peer.port()),
                );
            }
            Err(error) if per_connection(&error) => {
                tracing::debug!(%error, "connection closed before it was accepted");
            }
            Err(error) => {
                tracing::warn!(%error, "accepting a connection failed; retrying");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}
fn per_connection(error: &std::io::Error) -> bool {
    use std::io::ErrorKind::*;
    matches!(
        error.kind(),
        ConnectionReset | ConnectionAborted | ConnectionRefused | Interrupted | WouldBlock
    )
}
pub struct Discovery(ServiceDaemon);
impl Discovery {
    pub fn start(config: &Config, bind: IpAddr) -> Result<Option<Self>> {
        if !config.boolean("enable_discovery", true) || bind.is_loopback() {
            return Ok(None);
        }
        let daemon = ServiceDaemon::new()?;
        let result = (|| -> Result<()> {
            daemon.set_ip_check_interval(30)?;
            daemon.disable_interface(IfKind::LoopbackV4)?;
            daemon.disable_interface(IfKind::LoopbackV6)?;
            if bind.is_ipv4() {
                daemon.disable_interface(IfKind::IPv6)?;
            }
            if !bind.is_unspecified() {
                daemon.disable_interface(IfKind::All)?;
                daemon.enable_interface(IfKind::Addr(bind))?;
            }
            let hostname = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "butterpollo".into());
            let hostname = format!(
                "{}.local.",
                hostname.trim_end_matches('.').to_ascii_lowercase()
            );
            let instance = host_name(config);
            let instance = instance.as_str();
            let addresses = if bind.is_unspecified() {
                String::new()
            } else {
                bind.to_string()
            };
            let service = ServiceInfo::new(
                "_nvstream._tcp.local.",
                instance,
                &hostname,
                addresses.as_str(),
                config.ports()?.http,
                None,
            )?;
            let service = if bind.is_unspecified() {
                service.enable_addr_auto()
            } else {
                service
            };
            daemon.register(service)?;
            tracing::debug!(%instance, "Moonlight discovery registered");
            Ok(())
        })();
        if let Err(error) = result {
            let _ = daemon.shutdown();
            return Err(error);
        }
        Ok(Some(Self(daemon)))
    }
}
impl Drop for Discovery {
    fn drop(&mut self) {
        let _ = self.0.shutdown();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Mapping {
    protocol: Protocol,
    port: u16,
}
fn mappings(config: &Config, ports: Ports) -> Vec<Mapping> {
    let mut mappings = vec![
        Mapping {
            protocol: Protocol::TCP,
            port: ports.http,
        },
        Mapping {
            protocol: Protocol::TCP,
            port: ports.https,
        },
        Mapping {
            protocol: Protocol::TCP,
            port: ports.rtsp,
        },
        Mapping {
            protocol: Protocol::UDP,
            port: ports.video,
        },
        Mapping {
            protocol: Protocol::UDP,
            port: ports.control,
        },
        Mapping {
            protocol: Protocol::UDP,
            port: ports.audio,
        },
    ];
    if config.boolean("stream_mic", true) {
        mappings.push(Mapping {
            protocol: Protocol::UDP,
            port: ports.mic,
        });
    }
    if config.get("origin_web_ui_allowed", "lan") == "wan" {
        mappings.push(Mapping {
            protocol: Protocol::TCP,
            port: ports.web,
        });
    }
    // Browser guests: the guest page on its own port, and one media port per guest seat.
    if config.webrtc_server() {
        mappings.push(Mapping {
            protocol: Protocol::TCP,
            port: ports.guest,
        });
        let media = config.webrtc_media_port();
        mappings.extend(
            (0..crate::webrtc::guests::MAX_GUESTS)
                .filter_map(|slot| media.checked_add(1 + slot))
                .map(|port| Mapping {
                    protocol: Protocol::UDP,
                    port,
                }),
        );
    }
    mappings
}
/// The router's mapping table, and whether it was read to the end. Routers
/// answer the index past the end with 713, but some hide their table (606),
/// answer 714, 402 or 501, or time out: that ends the list without stopping
/// the host from adding its own mappings.
async fn entries<P: Provider>(gateway: &Gateway<P>) -> (Vec<igd_next::PortMappingEntry>, bool) {
    let mut entries = Vec::new();
    for index in 0..4096 {
        match tokio::time::timeout(
            Duration::from_secs(3),
            gateway.get_generic_port_mapping_entry(index),
        )
        .await
        {
            Ok(Ok(entry)) => entries.push(entry),
            Ok(Err(igd_next::GetGenericPortMappingEntryError::SpecifiedArrayIndexInvalid)) => {
                return (entries, true);
            }
            Ok(Err(error)) => {
                tracing::debug!(%error, index, "UPnP router stopped listing its mappings");
                return (entries, false);
            }
            Err(_) => {
                tracing::debug!(index, "UPnP router timed out listing its mappings");
                return (entries, false);
            }
        }
    }
    tracing::debug!("UPnP router mapping table exceeds its limit");
    (entries, false)
}
fn owned(entry: &igd_next::PortMappingEntry, local: IpAddr, description: &str) -> bool {
    entry.internal_client.parse::<IpAddr>().ok() == Some(local)
        && entry.port_mapping_description == description
        && entry.internal_port == entry.external_port
}
/// Whether another mapping already forwards this port to the same port on
/// this PC, as one left by another streaming host does: clients reach the
/// host through it, so it is not a conflict.
fn forwards_here(entry: &igd_next::PortMappingEntry, local: IpAddr) -> bool {
    entry.internal_client.parse::<IpAddr>().ok() == Some(local)
        && entry.internal_port == entry.external_port
}
/// `reported` holds the ports already warned about: the router is asked
/// again every minute, but a continuing conflict is only warned once.
async fn apply<P: Provider>(
    gateway: &Gateway<P>,
    local: IpAddr,
    wanted: &[Mapping],
    description: &str,
    reported: &mut Vec<Mapping>,
) -> Result<Vec<Mapping>> {
    let (entries, _) = entries(gateway).await;
    // The stable host description lets a restarted host reclaim its own
    // permanent leases, including ports removed from a later configuration.
    let mut applied: Vec<_> = entries
        .iter()
        .filter(|entry| owned(entry, local, description))
        .map(|entry| Mapping {
            protocol: entry.protocol,
            port: entry.external_port,
        })
        .collect();
    for mapping in wanted {
        if let Some(entry) = entries.iter().find(|entry| {
            entry.protocol == mapping.protocol
                && entry.external_port == mapping.port
                && !owned(entry, local, description)
        }) {
            if forwards_here(entry, local) {
                tracing::debug!(port=mapping.port, protocol=?mapping.protocol, owner=%entry.port_mapping_description, "UPnP port already forwards to this PC under another mapping");
            } else if reported.contains(mapping) {
                tracing::debug!(port=mapping.port, protocol=?mapping.protocol, "UPnP port is still owned by another mapping");
            } else {
                reported.push(*mapping);
                tracing::warn!(port=mapping.port, protocol=?mapping.protocol, client=%entry.internal_client, owner=%entry.port_mapping_description, "UPnP port is already owned by another mapping; remove it on the router so clients outside your network reach this PC");
            }
            continue;
        }
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            gateway.add_port(
                mapping.protocol,
                mapping.port,
                (local, mapping.port).into(),
                120,
                description,
            ),
        )
        .await;
        let result = if matches!(
            result,
            Ok(Err(igd_next::AddPortError::OnlyPermanentLeasesSupported))
        ) {
            tokio::time::timeout(
                Duration::from_secs(3),
                gateway.add_port(
                    mapping.protocol,
                    mapping.port,
                    (local, mapping.port).into(),
                    0,
                    description,
                ),
            )
            .await
        } else {
            result
        };
        match result {
            Ok(Ok(())) => {
                reported.retain(|reported| reported != mapping);
                if !applied.contains(mapping) {
                    applied.push(*mapping);
                }
            }
            Ok(Err(igd_next::AddPortError::PortInUse)) => {
                if !reported.contains(mapping) {
                    reported.push(*mapping);
                    tracing::warn!(port=mapping.port, protocol=?mapping.protocol, "UPnP port is already owned by another mapping; remove it on the router so clients outside your network reach this PC");
                }
            }
            Ok(Err(error)) => tracing::warn!(%error,port=mapping.port,"UPnP mapping failed"),
            Err(error) => tracing::warn!(%error,port=mapping.port,"UPnP mapping timed out"),
        }
    }
    Ok(applied)
}
async fn remove<P: Provider>(
    gateway: &Gateway<P>,
    local: IpAddr,
    applied: &[Mapping],
    description: &str,
) -> Result<()> {
    let (entries, complete) = entries(gateway).await;
    // A mapping the router would not remove must not keep the later ones open:
    // all are tried, and the first failure is the one reported. A router that
    // stopped answering ends it: the next removal would wait as long, and the
    // host's shutdown gives this task ten seconds.
    let mut failed = None;
    for mapping in applied {
        let entry = entries.iter().find(|entry| {
            entry.protocol == mapping.protocol && entry.external_port == mapping.port
        });
        // A mapping this host added that an unreadable table cannot show is
        // still removed; one the table shows under another owner is not.
        let ours = match entry {
            Some(entry) => owned(entry, local, description),
            None => !complete,
        };
        if ours {
            let removed = tokio::time::timeout(
                Duration::from_secs(3),
                gateway.remove_port(mapping.protocol, mapping.port),
            )
            .await;
            let error = match removed {
                Ok(Ok(())) => continue,
                Ok(Err(error)) => anyhow::Error::from(error),
                Err(error) => {
                    failed.get_or_insert(anyhow::Error::from(error));
                    break;
                }
            };
            if failed.is_none() {
                failed = Some(error);
            }
        }
    }
    failed.map_or(Ok(()), Err)
}
/// The UPnP and IPv6 pinhole task, which sleeps until its next renewal or until
/// it is told that the host stops.
pub struct PortForward {
    task: tokio::task::JoinHandle<()>,
    stop: std::sync::Arc<tokio::sync::Notify>,
}
impl PortForward {
    /// The host stops: after `h.stop` is set, this wakes the task, which takes its
    /// mappings down and ends.
    pub fn stop(&self) {
        self.stop.notify_one();
    }
    /// Waits up to `wait` for the task to take its mappings down, then ends it.
    pub async fn finish(mut self, wait: Duration) {
        if tokio::time::timeout(wait, &mut self.task).await.is_err() {
            self.task.abort();
        }
    }
}
/// Waits until `renewal` or until `stop` is notified, whichever comes first.
async fn until_renewal_or_stop(stop: &tokio::sync::Notify, renewal: Instant) {
    let wait = renewal.saturating_duration_since(Instant::now());
    tokio::select! {
        () = stop.notified() => {}
        () = tokio::time::sleep(wait) => {}
    }
}
fn protocol_name(protocol: Protocol) -> String {
    if protocol == Protocol::TCP {
        "TCP"
    } else {
        "UDP"
    }
    .to_owned()
}
/// The console's view of a mapping attempt: each wanted port mapped or refused, and why.
fn port_states(
    wanted: &[Mapping],
    mapped: &[(Mapping, u16)],
    refused: &[(Mapping, String)],
) -> (
    Vec<crate::internet_play::PortState>,
    Vec<crate::internet_play::PortState>,
) {
    let state = |mapping: &Mapping, external: Option<u16>, reason: Option<String>| {
        crate::internet_play::PortState {
            protocol: protocol_name(mapping.protocol),
            port: mapping.port,
            external,
            reason,
        }
    };
    let ok = wanted
        .iter()
        .filter_map(|mapping| {
            mapped
                .iter()
                .find(|(done, _)| done == mapping)
                .map(|(_, external)| state(mapping, Some(*external), None))
        })
        .collect();
    let failed = wanted
        .iter()
        .filter(|mapping| !mapped.iter().any(|(done, _)| done == *mapping))
        .map(|mapping| {
            let reason = refused
                .iter()
                .find(|(done, _)| done == mapping)
                .map_or_else(
                    || "the router refused the mapping".to_owned(),
                    |(_, reason)| reason.clone(),
                );
            state(mapping, None, Some(reason))
        })
        .collect();
    (ok, failed)
}
/// The router's IPv6 gateways, for PCP pinholes.
fn ipv6_gateways() -> Vec<std::net::Ipv6Addr> {
    default_gateways()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|gateway| match gateway {
            IpAddr::V6(ip) => Some(ip),
            IpAddr::V4(_) => None,
        })
        .collect()
}
pub fn port_forward(h: Shared, bind: IpAddr) -> Option<PortForward> {
    let enabled = h.config.read().unwrap().boolean("upnp", false) && !bind.is_loopback();
    crate::internet_play::update_mapping(|status| {
        *status = crate::internet_play::MappingStatus {
            enabled,
            ..Default::default()
        };
    });
    if !enabled {
        return None;
    }
    let stop = std::sync::Arc::new(tokio::sync::Notify::new());
    let stopped = stop.clone();
    let task = tokio::spawn(async move {
        let config = h.config.read().unwrap().clone();
        let wanted = mappings(&config, config.ports().unwrap());
        let description = format!("Butterpollo Rust {}", h.paired.read().unwrap().unique_id);
        let mut next_search = Instant::now();
        let mut current = None;
        let mut firewall = None;
        // Routers without UPnP IGD: PCP, else NAT-PMP, at the default gateway.
        let mut mapper: Option<portmap::PortMapper> = None;
        // IPv6 routers without an IGDv2 firewall service: PCP pinholes.
        let mut pinholes: Vec<portmap::PortMapper> = Vec::new();
        let mut reported = Vec::new();
        while !h.stop.load(Ordering::Acquire) {
            if Instant::now() >= next_search {
                next_search = Instant::now() + Duration::from_secs(60);
                let mut upnp_error = None;
                if current.is_none() {
                    let options = igd_next::SearchOptions {
                        timeout: Some(Duration::from_secs(3)),
                        bind_addr: if bind.is_ipv4() {
                            (bind, 0).into()
                        } else {
                            (IpAddr::from([0, 0, 0, 0]), 0).into()
                        },
                        ..Default::default()
                    };
                    match upnp::discover(options).await {
                        Ok(gateway) => {
                            let local = (|| -> Result<IpAddr> {
                                let socket = UdpSocket::bind((IpAddr::from([0, 0, 0, 0]), 0))?;
                                socket.connect(gateway.addr)?;
                                Ok(if bind.is_unspecified() || bind.is_ipv6() {
                                    socket.local_addr()?.ip()
                                } else {
                                    bind
                                })
                            })();
                            if let Ok(local) = local {
                                current = Some((gateway, local, Vec::new()));
                            }
                        }
                        Err(error) => {
                            tracing::debug!(%error, "UPnP gateway unavailable; retrying");
                            upnp_error = Some(format!("{error:#}"));
                        }
                    }
                }
                if let Some((gateway, local, applied)) = current.as_mut() {
                    match apply(gateway, *local, &wanted, &description, &mut reported).await {
                        Ok(mappings) => *applied = mappings,
                        Err(error) => tracing::warn!(%error, "UPnP mapping could not be renewed"),
                    }
                    let external =
                        tokio::time::timeout(Duration::from_secs(3), gateway.get_external_ip())
                            .await
                            .ok()
                            .and_then(|result| result.ok());
                    let mapped: Vec<(Mapping, u16)> = applied
                        .iter()
                        .map(|mapping| (*mapping, mapping.port))
                        .collect();
                    let refused: Vec<(Mapping, String)> = reported
                        .iter()
                        .map(|mapping| {
                            (
                                *mapping,
                                "another mapping on the router already uses this port".to_owned(),
                            )
                        })
                        .collect();
                    let (ok, failed) = port_states(&wanted, &mapped, &refused);
                    crate::internet_play::update_mapping(|status| {
                        status.method = Some("UPnP".into());
                        status.external_ipv4 = external;
                        status.mapped = ok;
                        status.failed = failed;
                        status.error = None;
                    });
                } else {
                    if mapper.is_none() {
                        let route = default_gateways()
                            .ok()
                            .and_then(|gateways| portmap::ipv4_route(&gateways, bind));
                        if let Some((gateway, local)) = route {
                            mapper = portmap::PortMapper::ipv4(gateway, local).await.ok();
                        }
                    }
                    let result = match mapper.as_mut() {
                        Some(mapper) => mapper.renew(&wanted).await,
                        None => Err(anyhow::anyhow!("no default gateway")),
                    };
                    match result {
                        Ok(outcome) => {
                            for (mapping, reason) in &outcome.failed {
                                if !reported.contains(mapping) {
                                    reported.push(*mapping);
                                    tracing::warn!(port = mapping.port, protocol = ?mapping.protocol, %reason, "port mapping refused");
                                }
                            }
                            let (ok, failed) =
                                port_states(&wanted, &outcome.mapped, &outcome.failed);
                            let method = mapper
                                .as_ref()
                                .and_then(portmap::PortMapper::method)
                                .map(|method| method.name().to_owned());
                            let external = mapper.as_ref().and_then(|mapper| mapper.external);
                            crate::internet_play::update_mapping(|status| {
                                status.method = method;
                                status.external_ipv4 = external;
                                status.mapped = ok;
                                status.failed = failed;
                                status.error = None;
                            });
                        }
                        Err(error) => {
                            // Look for a router again next time: it may come up later.
                            mapper = None;
                            let message = format!(
                                "the router answers none of UPnP, PCP and NAT-PMP: turn UPnP on in the router, or forward the ports by hand ({})",
                                upnp_error.as_deref().unwrap_or("no UPnP router found")
                            );
                            tracing::debug!(error = %format!("{error:#}"), "PCP and NAT-PMP unavailable; retrying");
                            let (ok, failed) = port_states(&wanted, &[], &[]);
                            crate::internet_play::update_mapping(|status| {
                                status.method = None;
                                status.mapped = ok;
                                status.failed = failed;
                                status.error = Some(message);
                            });
                        }
                    }
                }
                if bind.is_ipv6() {
                    if firewall.is_none() {
                        firewall = ipv6::discover(bind).await.ok();
                    }
                    if let Some(firewall) = firewall.as_mut() {
                        let result = firewall.renew(&wanted).await;
                        if let Err(error) = &result {
                            tracing::debug!(%error, "IPv6 firewall pinholes could not be renewed");
                        }
                        let pinholes = if result.is_ok() { wanted.len() } else { 0 };
                        let error = result.err().map(|error| format!("{error:#}"));
                        crate::internet_play::update_mapping(|status| {
                            status.ipv6_method = Some("UPnP".into());
                            status.ipv6_pinholes = pinholes;
                            status.ipv6_error = error;
                        });
                    } else {
                        if pinholes.is_empty() {
                            let gateways = ipv6_gateways();
                            for (local, index) in portmap::ipv6_locals(bind) {
                                for gateway in &gateways {
                                    if let Ok(mapper) =
                                        portmap::PortMapper::ipv6(*gateway, local, index).await
                                    {
                                        pinholes.push(mapper);
                                    }
                                }
                            }
                        }
                        let mut opened = 0;
                        let mut last_error = None;
                        for mapper in &mut pinholes {
                            match mapper.renew(&wanted).await {
                                Ok(outcome) => opened += outcome.mapped.len(),
                                Err(error) => last_error = Some(format!("{error:#}")),
                            }
                        }
                        if opened == 0 {
                            // Search again next time: the router may answer later.
                            pinholes.clear();
                        }
                        crate::internet_play::update_mapping(|status| {
                            status.ipv6_method = (opened > 0).then(|| "PCP".into());
                            status.ipv6_pinholes = opened;
                            status.ipv6_error = (opened == 0).then(|| {
                                last_error.unwrap_or_else(|| {
                                    "the router opens no IPv6 pinholes (neither UPnP IGDv2 nor PCP)"
                                        .to_owned()
                                })
                            });
                        });
                    }
                }
            }
            until_renewal_or_stop(&stopped, next_search).await;
        }
        if let Some(mut firewall) = firewall
            && let Err(error) = firewall.close().await
        {
            tracing::debug!(%error, "IPv6 pinholes will expire after their finite lease");
        }
        for mut mapper in pinholes {
            mapper.close().await;
        }
        if let Some(mut mapper) = mapper {
            mapper.close().await;
        }
        if let Some((gateway, local, applied)) = current
            && let Err(error) = remove(&gateway, local, &applied, &description).await
        {
            tracing::warn!(%error, "UPnP cleanup failed; permanent mappings remain reclaimable by this host");
        }
    });
    Some(PortForward { task, stop })
}
#[cfg(test)]
mod port_state_tests {
    use super::*;
    #[test]
    fn the_console_sees_every_wanted_port_mapped_or_refused_with_a_reason() {
        let wanted = [
            Mapping {
                protocol: Protocol::TCP,
                port: 47984,
            },
            Mapping {
                protocol: Protocol::UDP,
                port: 47998,
            },
            Mapping {
                protocol: Protocol::UDP,
                port: 48000,
            },
        ];
        let (ok, failed) = port_states(
            &wanted,
            &[(wanted[0], 50000)],
            &[(wanted[1], "the router is out of mappings".into())],
        );
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].external, Some(50000));
        assert_eq!(ok[0].protocol, "TCP");
        assert_eq!(failed.len(), 2);
        assert_eq!(
            failed[0].reason.as_deref(),
            Some("the router is out of mappings")
        );
        assert_eq!(
            failed[1].reason.as_deref(),
            Some("the router refused the mapping")
        );
    }
}
#[cfg(test)]
mod tests {
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
}
