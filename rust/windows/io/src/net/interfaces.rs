//! Network host identity, adapter enumeration, route classification, and TCP owner lookup.
use anyhow::Result;
use std::net::{IpAddr, SocketAddr};
use windows::Win32::Networking::WinSock::*;

/// The PC's DNS host name, which Vibepollo shows to clients by default.
pub fn host_name() -> Option<String> {
    use windows::Win32::System::SystemInformation::{ComputerNameDnsHostname, GetComputerNameExW};
    let mut size = 0u32;
    // SAFETY: A null buffer only queries the required length into the local `size`.
    unsafe {
        let _ = GetComputerNameExW(ComputerNameDnsHostname, None, &mut size);
    }
    if size == 0 || size > 1024 {
        return None;
    }
    let mut name = vec![0u16; size as usize];
    // SAFETY: `name` holds `size` UTF-16 units, the length the previous call reported, and outlives
    // the call.
    unsafe {
        GetComputerNameExW(
            ComputerNameDnsHostname,
            Some(windows::core::PWSTR(name.as_mut_ptr())),
            &mut size,
        )
        .ok()?;
    }
    name.truncate(size as usize);
    Some(String::from_utf16_lossy(&name)).filter(|name| !name.trim().is_empty())
}

/// The MAC a magic packet wakes this PC with, for a request that reached it
/// at `address` (IPv4-mapped IPv6 included): the physical NIC carrying it, or
/// the one under a virtual or VPN adapter (`wake::primary`). Zeros for
/// loopback or when no physical adapter is connected.
pub fn local_mac(address: std::net::IpAddr) -> Result<String> {
    if address.to_canonical().is_loopback() {
        return Ok(butterpollo_session::wake::format_mac(None));
    }
    Ok(butterpollo_session::wake::reported_mac(
        &crate::wake::adapters()?,
        address,
        crate::wake::armed,
    ))
}

/// The unicast addresses of interfaces that are up, without loopback and
/// link-local ones.
pub fn unicast_addresses() -> Result<Vec<IpAddr>> {
    use windows::Win32::{
        Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS},
        NetworkManagement::IpHelper::*,
    };
    let mut size = 15000u32;
    for _ in 0..3 {
        anyhow::ensure!(
            size <= 1024 * 1024,
            "network interface table exceeds its limit"
        );
        let mut storage = vec![0u64; (size as usize).div_ceil(8)];
        let first = storage.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        // SAFETY: `storage` is 8-byte aligned, at least `size` bytes long, and outlives the call.
        let status = unsafe {
            GetAdaptersAddresses(
                AF_UNSPEC.0 as u32,
                GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER,
                None,
                Some(first),
                &mut size,
            )
        };
        if status == ERROR_BUFFER_OVERFLOW.0 {
            continue;
        }
        if status != ERROR_SUCCESS.0 {
            return Err(std::io::Error::from_raw_os_error(status as i32).into());
        }
        let mut addresses = Vec::new();
        // SAFETY: As in wake::adapters, the linked records live in `storage`, and each sockaddr is
        // converted only within its reported length.
        unsafe {
            let mut adapter = first;
            while let Some(row) = adapter.as_ref() {
                let mut unicast = if row.OperStatus.0 == 1 {
                    row.FirstUnicastAddress
                } else {
                    std::ptr::null_mut()
                };
                while let Some(ip) = unicast.as_ref() {
                    let address = socket2::SockAddr::try_init(|storage, length| {
                        let size =
                            (ip.Address.iSockaddrLength.max(0) as usize).min(*length as usize);
                        if !ip.Address.lpSockaddr.is_null() {
                            std::ptr::copy_nonoverlapping(
                                ip.Address.lpSockaddr.cast::<u8>(),
                                storage.cast::<u8>(),
                                size,
                            );
                        }
                        *length = size as i32;
                        Ok(())
                    })
                    .ok()
                    .and_then(|(_, address)| address.as_socket());
                    if let Some(address) = address.map(|a| a.ip().to_canonical())
                        && !address.is_loopback()
                        && !address.is_unspecified()
                        && !matches!(address, IpAddr::V4(v4) if v4.is_link_local())
                        && !matches!(address, IpAddr::V6(v6) if v6.is_unicast_link_local())
                    {
                        addresses.push(address);
                    }
                    unicast = ip.Next;
                }
                adapter = row.Next;
            }
        }
        addresses.sort();
        addresses.dedup();
        return Ok(addresses);
    }
    anyhow::bail!("network interfaces changed during enumeration")
}

/// The next hops of every default route (0.0.0.0/0 and ::/0): the routers this PC sends the
/// internet through. One read of the routing table, no network traffic.
pub fn default_gateways() -> Result<Vec<IpAddr>> {
    use windows::Win32::NetworkManagement::IpHelper::*;
    let mut table = std::ptr::null_mut();
    // SAFETY: `table` is valid on success, read only within its NumEntries rows and freed once.
    unsafe {
        let status = GetIpForwardTable2(AF_UNSPEC, &mut table);
        if status.0 != 0 {
            return Err(std::io::Error::from_raw_os_error(status.0 as i32).into());
        }
        let rows = std::slice::from_raw_parts(
            (&raw const (*table).Table).cast::<MIB_IPFORWARD_ROW2>(),
            (*table).NumEntries as usize,
        );
        let mut gateways = Vec::new();
        for row in rows {
            if row.DestinationPrefix.PrefixLength != 0 {
                continue;
            }
            let hop = row.NextHop;
            // The family field is shared by every view; a view is read only when it names it.
            let address = match hop.si_family {
                AF_INET => Some(IpAddr::from(hop.Ipv4.sin_addr.S_un.S_addr.to_ne_bytes())),
                AF_INET6 => Some(IpAddr::from(hop.Ipv6.sin6_addr.u.Byte).to_canonical()),
                _ => None,
            };
            if let Some(address) = address.filter(|address| !address.is_unspecified()) {
                gateways.push(address);
            }
        }
        FreeMibTable(table.cast());
        gateways.sort();
        gateways.dedup();
        Ok(gateways)
    }
}

/// The process that owns the IPv4 TCP connection from `local` to `remote`; None when there is
/// no such connection. Kernel drivers' connections belong to System, PID 4.
pub fn tcp_owner(
    local: std::net::SocketAddrV4,
    remote: std::net::SocketAddrV4,
) -> Result<Option<u32>> {
    use windows::Win32::{
        Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR},
        NetworkManagement::IpHelper::*,
    };
    let address = |raw: u32| std::net::Ipv4Addr::from(raw.to_ne_bytes());
    // The low 16 bits hold the port in network order.
    let port = |raw: u32| {
        let bytes = raw.to_ne_bytes();
        u16::from_be_bytes([bytes[0], bytes[1]])
    };
    let mut size = 0u32;
    let mut storage = Vec::<u64>::new();
    for _ in 0..4 {
        // SAFETY: `storage` is 8-byte aligned, at least `size` bytes long, and outlives the call;
        // an empty one only queries the size.
        let status = unsafe {
            GetExtendedTcpTable(
                (!storage.is_empty()).then(|| storage.as_mut_ptr().cast()),
                &mut size,
                false,
                AF_INET.0 as u32,
                TCP_TABLE_OWNER_PID_CONNECTIONS,
                0,
            )
        };
        if status == ERROR_INSUFFICIENT_BUFFER.0 {
            anyhow::ensure!(size <= 64 << 20, "the TCP table exceeds its limit");
            storage = vec![0u64; (size as usize).div_ceil(8)];
            continue;
        }
        if status != NO_ERROR.0 {
            return Err(std::io::Error::from_raw_os_error(status as i32).into());
        }
        if storage.is_empty() {
            return Ok(None);
        }
        // SAFETY: The call filled `storage` with a MIB_TCPTABLE_OWNER_PID whose `dwNumEntries`
        // rows follow its count inside the `size` bytes it reported.
        let rows = unsafe {
            let table = &*storage.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>();
            std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize)
        };
        return Ok(rows
            .iter()
            .find(|row| {
                address(row.dwLocalAddr) == *local.ip()
                    && port(row.dwLocalPort) == local.port()
                    && address(row.dwRemoteAddr) == *remote.ip()
                    && port(row.dwRemotePort) == remote.port()
            })
            .map(|row| row.dwOwningPid));
    }
    anyhow::bail!("TCP connections changed during enumeration")
}

/// Active Ethernet/Wi-Fi IPv4 addresses for Moonlight's manual Add PC flow.
pub fn lan_addresses() -> Result<Vec<String>> {
    Ok(lan_interfaces()?
        .into_iter()
        .map(|interface| interface.address.to_string())
        .collect())
}

/// An active Ethernet or Wi-Fi interface's IPv4 address, as [`lan_interfaces`] lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanInterface {
    pub address: std::net::Ipv4Addr,
    /// A physical adapter: NDIS calls the interface hardware.
    pub hardware: bool,
    /// Hyper-V's virtual Ethernet adapter, by [`hyper_v_adapter`].
    pub hyper_v: bool,
    /// The IPv4 interface metric: of two routes alike, Windows takes the lower.
    pub metric: u32,
    /// The interface's IPv4 default gateways.
    pub gateways: Vec<std::net::Ipv4Addr>,
}
impl LanInterface {
    /// Whether this PC reaches its router through the interface: a physical adapter, or
    /// Hyper-V's adapter on an external switch, which carries the address of the physical card
    /// it shares and its default gateway. Hyper-V's Default Switch and WSL's adapter have no
    /// default gateway (this PC is theirs), and VPN adapters that call themselves Ethernet (TAP,
    /// ZeroTier) are neither.
    pub fn uplink(&self) -> bool {
        self.hardware || (self.hyper_v && !self.gateways.is_empty())
    }
}

/// Whether an adapter's description, which its driver gives and the user cannot rename (unlike
/// its name, "vEthernet (...)"), is Hyper-V's virtual Ethernet adapter: "Hyper-V Virtual
/// Ethernet Adapter", with " #2" and on for more of them.
pub fn hyper_v_adapter(description: &str) -> bool {
    description.starts_with("Hyper-V Virtual Ethernet Adapter")
}

/// Active Ethernet/Wi-Fi IPv4 addresses with what tells their interfaces apart: Hyper-V, WSL and
/// some VPN adapters (TAP, ZeroTier) also identify as Ethernet. Physical first, then private
/// addresses first.
pub fn lan_interfaces() -> Result<Vec<LanInterface>> {
    use windows::Win32::{
        Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS},
        NetworkManagement::IpHelper::*,
    };
    let mut size = 15000u32;
    for _ in 0..3 {
        anyhow::ensure!(
            size <= 1024 * 1024,
            "network interface table exceeds its limit"
        );
        let mut storage = vec![0u64; (size as usize).div_ceil(8)];
        let first = storage.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        // SAFETY: `storage` is 8-byte aligned, at least `size` bytes long, and outlives the call.
        let status = unsafe {
            GetAdaptersAddresses(
                AF_INET.0 as u32,
                GAA_FLAG_SKIP_ANYCAST
                    | GAA_FLAG_SKIP_MULTICAST
                    | GAA_FLAG_SKIP_DNS_SERVER
                    | GAA_FLAG_INCLUDE_GATEWAYS,
                None,
                Some(first),
                &mut size,
            )
        };
        if status == ERROR_BUFFER_OVERFLOW.0 {
            continue;
        }
        if status != ERROR_SUCCESS.0 {
            return Err(std::io::Error::from_raw_os_error(status as i32).into());
        }
        let mut addresses: Vec<LanInterface> = Vec::new();
        // SAFETY: As in wake::adapters, the linked records live in `storage`, and each sockaddr is read
        // as SOCKADDR_IN only when non-null and long enough; the gateway list is read the same way,
        // the description is a NUL-terminated string in the same buffer, and `interface` is a local.
        unsafe {
            let mut adapter = first;
            while let Some(row) = adapter.as_ref() {
                if row.OperStatus.0 == 1 && matches!(row.IfType, 6 | 71) {
                    let mut interface = MIB_IF_ROW2 {
                        InterfaceLuid: row.Luid,
                        ..Default::default()
                    };
                    let hardware = GetIfEntry2(&mut interface).0 == 0
                        && interface.InterfaceAndOperStatusFlags._bitfield & 1 != 0;
                    let hyper_v = !row.Description.is_null()
                        && row
                            .Description
                            .to_string()
                            .is_ok_and(|description| hyper_v_adapter(&description));
                    let mut gateways = Vec::new();
                    let mut gateway = row.FirstGatewayAddress;
                    while let Some(hop) = gateway.as_ref() {
                        if !hop.Address.lpSockaddr.is_null()
                            && hop.Address.iSockaddrLength as usize >= size_of::<SOCKADDR_IN>()
                        {
                            let socket = &*hop.Address.lpSockaddr.cast::<SOCKADDR_IN>();
                            let address =
                                std::net::Ipv4Addr::from(socket.sin_addr.S_un.S_addr.to_ne_bytes());
                            if socket.sin_family == AF_INET && !address.is_unspecified() {
                                gateways.push(address);
                            }
                        }
                        gateway = hop.Next;
                    }
                    let mut unicast = row.FirstUnicastAddress;
                    while let Some(ip) = unicast.as_ref() {
                        if !ip.Address.lpSockaddr.is_null()
                            && ip.Address.iSockaddrLength as usize >= size_of::<SOCKADDR_IN>()
                        {
                            let socket = &*ip.Address.lpSockaddr.cast::<SOCKADDR_IN>();
                            if socket.sin_family == AF_INET {
                                let address = std::net::Ipv4Addr::from(
                                    socket.sin_addr.S_un.S_addr.to_ne_bytes(),
                                );
                                if !address.is_loopback()
                                    && !address.is_link_local()
                                    && !address.is_unspecified()
                                {
                                    addresses.push(LanInterface {
                                        address,
                                        hardware,
                                        hyper_v,
                                        metric: row.Ipv4Metric,
                                        gateways: gateways.clone(),
                                    });
                                }
                            }
                        }
                        unicast = ip.Next;
                    }
                }
                adapter = row.Next;
            }
        }
        // Hyper-V/WSL adapters also identify as Ethernet. Prefer a physical
        // interface so first-run instructions point at a reachable LAN address.
        addresses.sort_by_key(|interface| {
            (
                !interface.hardware,
                !interface.address.is_private(),
                interface.address,
            )
        });
        addresses.dedup_by_key(|interface| interface.address);
        return Ok(addresses);
    }
    anyhow::bail!("network interfaces changed during enumeration")
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Link {
    /// Physical Ethernet speed; zero means unknown.
    pub bps: u64,
    pub wireless: bool,
    /// The NDIS locally unique identifier (LUID) of the interface the route
    /// leaves by; zero means unknown. A host that changes network adapter
    /// has a different path to the same client. It names the adapter, not the
    /// network it is joined to.
    pub interface: u64,
}

pub(super) fn interface_link(
    row: &windows::Win32::NetworkManagement::IpHelper::MIB_IF_ROW2,
) -> Link {
    use windows::Win32::NetworkManagement::{IpHelper::*, Ndis::IfOperStatusUp};
    if row.OperStatus != IfOperStatusUp {
        return Link::default();
    }
    Link {
        // SAFETY: `Value` and the bitfield view of the LUID union are the same eight bytes.
        interface: unsafe { row.InterfaceLuid.Value },
        bps: if row.Type == IF_TYPE_ETHERNET_CSMACD
            && row.InterfaceAndOperStatusFlags._bitfield & 1 != 0
        {
            row.TransmitLinkSpeed
        } else {
            0
        },
        wireless: matches!(
            row.Type,
            IF_TYPE_IEEE80211 | IF_TYPE_IEEE80216_WMAN | IF_TYPE_WWANPP | IF_TYPE_WWANPP2
        ),
    }
}

pub(super) fn underlying_link(
    mut index: u32,
    stack: &[windows::Win32::NetworkManagement::IpHelper::MIB_IFSTACK_ROW],
    mut entry: impl FnMut(u32) -> Option<windows::Win32::NetworkManagement::IpHelper::MIB_IF_ROW2>,
) -> Link {
    let mut seen = std::collections::BTreeSet::new();
    while seen.insert(index) {
        let mut lower = stack.iter().filter(|row| {
            row.HigherLayerInterfaceIndex == index && row.LowerLayerInterfaceIndex != 0
        });
        let Some(row) = lower.next() else { break };
        // A team or ambiguous binding cannot establish the route's medium.
        if lower.next().is_some() {
            break;
        }
        index = row.LowerLayerInterfaceIndex;
        let Some(row) = entry(index) else { break };
        if row.OperStatus != windows::Win32::NetworkManagement::Ndis::IfOperStatusUp {
            break;
        }
        if row.InterfaceAndOperStatusFlags._bitfield & 1 != 0 {
            return interface_link(&row);
        }
    }
    Link::default()
}

/// Classify the route, using a virtual Ethernet adapter's physical binding
/// when Windows exposes an unambiguous interface stack.
pub fn routed_link(peer: SocketAddr) -> Link {
    use windows::Win32::NetworkManagement::{IpHelper::*, Ndis::IfOperStatusUp};
    let peer = SocketAddr::new(peer.ip().to_canonical(), peer.port());
    if peer.ip().is_loopback() {
        return Link::default();
    }
    let address = socket2::SockAddr::from(peer);
    let mut index = 0;
    // SAFETY: `address` and `row` outlive the calls that read and fill them; `table` is valid on
    // success and freed only after `stack`, its NumEntries rows, is last used.
    unsafe {
        if GetBestInterfaceEx(address.as_ptr().cast(), &mut index) != 0 {
            return Link::default();
        }
        let mut row = MIB_IF_ROW2 {
            InterfaceIndex: index,
            ..Default::default()
        };
        // Without statistics: the counters cost the driver a query, and
        // only the interface's type, state and speed are read.
        if GetIfEntry2Ex(MibIfEntryNormalWithoutStatistics, &mut row).0 != 0
            || row.OperStatus != IfOperStatusUp
        {
            return Link::default();
        }
        // The interface the route leaves by, whatever the medium under it.
        let interface = row.InterfaceLuid.Value;
        if row.Type != IF_TYPE_ETHERNET_CSMACD || row.InterfaceAndOperStatusFlags._bitfield & 1 != 0
        {
            return interface_link(&row);
        }
        let mut table = std::ptr::null_mut();
        if GetIfStackTable(&mut table).0 != 0 {
            return Link::default();
        }
        let stack = std::slice::from_raw_parts(
            (&raw const (*table).Table).cast(),
            (*table).NumEntries as usize,
        );
        let link = underlying_link(index, stack, |index| {
            let mut row = MIB_IF_ROW2 {
                InterfaceIndex: index,
                ..Default::default()
            };
            (GetIfEntry2Ex(MibIfEntryNormalWithoutStatistics, &mut row).0 == 0).then_some(row)
        });
        FreeMibTable(table.cast());
        Link { interface, ..link }
    }
}

/// Physical Ethernet speed on the route to this client; zero means unknown.
pub fn routed_link_bps(peer: SocketAddr) -> u64 {
    routed_link(peer).bps
}

/// Whether Windows reaches `peer` without a gateway, so it is on this host's
/// link. A route with a next hop crosses a router that may carry a smaller MTU,
/// which the host cannot see, so jumbo packets are offered only on-link.
pub fn routed_same_link(peer: SocketAddr) -> bool {
    use windows::Win32::NetworkManagement::IpHelper::{GetBestRoute2, MIB_IPFORWARD_ROW2};
    use windows::Win32::Networking::WinSock::SOCKADDR_INET;
    let peer = SocketAddr::new(peer.ip().to_canonical(), peer.port());
    let address = socket2::SockAddr::from(peer);
    let mut route = MIB_IPFORWARD_ROW2::default();
    let mut source = SOCKADDR_INET::default();
    // SAFETY: `address` outlives the call; `route` and `source` are live locals
    // the call fills before they are read on success.
    unsafe {
        GetBestRoute2(
            None,
            0,
            None,
            address.as_ptr().cast(),
            0,
            &mut route,
            &mut source,
        )
        .ok()
        .is_ok_and(|()| next_hop_unspecified(&route.NextHop))
    }
}

/// Whether a route's next hop is unspecified: the destination is on-link.
fn next_hop_unspecified(next: &windows::Win32::Networking::WinSock::SOCKADDR_INET) -> bool {
    use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6};
    // SAFETY: `si_family` is the union's tag; matching it is the contract for
    // reading the selected member, which Windows initialised.
    unsafe {
        if next.si_family == AF_INET {
            next.Ipv4.sin_addr.S_un.S_addr == 0
        } else if next.si_family == AF_INET6 {
            next.Ipv6.sin6_addr.u.Byte.iter().all(|byte| *byte == 0)
        } else {
            // An unknown family is not proven on-link; offer nothing.
            false
        }
    }
}

/// The payload MTU of the interface Windows routes `peer` through, IPv4 or IPv6:
/// the size a datagram may have before the network fragments it. None for loopback
/// (including Internet Play tunnels on `127.77.x.y`/`127.78.x.y`), tunnel interfaces,
/// or when the lookup fails or there is no route.
pub fn routed_mtu(peer: SocketAddr) -> Option<u32> {
    use windows::Win32::NetworkManagement::IpHelper::{
        GetBestInterfaceEx, GetIfEntry2Ex, GetIpInterfaceEntry, IF_TYPE_SOFTWARE_LOOPBACK,
        IF_TYPE_TUNNEL, MIB_IF_ROW2, MIB_IPINTERFACE_ROW, MibIfEntryNormalWithoutStatistics,
    };
    let peer = SocketAddr::new(peer.ip().to_canonical(), peer.port());
    if peer.ip().is_loopback() {
        return None;
    }
    let address = socket2::SockAddr::from(peer);
    let mut index = 0;
    // SAFETY: `address` outlives the call that only reads it, and `if_row`/`row` are
    // filled by the calls before their fields are read on success.
    unsafe {
        if GetBestInterfaceEx(address.as_ptr().cast(), &mut index) != 0 {
            return None;
        }
        let mut if_row = MIB_IF_ROW2 {
            InterfaceIndex: index,
            ..Default::default()
        };
        if GetIfEntry2Ex(MibIfEntryNormalWithoutStatistics, &mut if_row).0 == 0
            && matches!(if_row.Type, IF_TYPE_SOFTWARE_LOOPBACK | IF_TYPE_TUNNEL)
        {
            return None;
        }
        let mut row = MIB_IPINTERFACE_ROW {
            Family: if peer.is_ipv4() { AF_INET } else { AF_INET6 },
            InterfaceIndex: index,
            ..Default::default()
        };
        if GetIpInterfaceEntry(&mut row).0 != 0 {
            return None;
        }
        Some(row.NlMtu)
    }
}
