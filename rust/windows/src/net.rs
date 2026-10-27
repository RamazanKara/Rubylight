//! Avoid stale UDP ICMP errors aborting a subsequent streaming client.

use anyhow::Result;
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket},
    os::windows::io::{AsRawSocket, AsSocket, BorrowedSocket, RawSocket},
    sync::{
        RwLock,
        atomic::{AtomicBool, Ordering},
    },
};
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
/// Use the interface carrying this connection, including IPv4-mapped IPv6.
/// Loopback and interfaces without an Ethernet address cannot support WOL.
pub fn local_mac(address: std::net::IpAddr) -> Result<String> {
    use windows::Win32::{
        Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS},
        NetworkManagement::IpHelper::*,
    };
    let address = address.to_canonical();
    if address.is_loopback() {
        return Ok("00:00:00:00:00:00".into());
    }
    let mut size = 15000u32;
    for _ in 0..3 {
        if size > 1024 * 1024 {
            anyhow::bail!("network interface table exceeds its limit");
        }
        // The Windows records contain u64 members and pointers.
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
        // SAFETY: GetAdaptersAddresses filled `storage`, still alive, with linked records; each
        // sockaddr is read as IPv4 or IPv6 only after its family and length are checked.
        unsafe {
            let mut adapter = first;
            while let Some(row) = adapter.as_ref() {
                let mut unicast = row.FirstUnicastAddress;
                while let Some(ip) = unicast.as_ref() {
                    let socket = ip.Address;
                    let family = socket.lpSockaddr.as_ref().map(|a| a.sa_family);
                    let candidate = match family {
                        Some(AF_INET)
                            if socket.iSockaddrLength as usize
                                >= std::mem::size_of::<SOCKADDR_IN>() =>
                        {
                            let value = &*socket.lpSockaddr.cast::<SOCKADDR_IN>();
                            Some(std::net::IpAddr::from(
                                value.sin_addr.S_un.S_addr.to_ne_bytes(),
                            ))
                        }
                        Some(AF_INET6)
                            if socket.iSockaddrLength as usize
                                >= std::mem::size_of::<SOCKADDR_IN6>() =>
                        {
                            let value = &*socket.lpSockaddr.cast::<SOCKADDR_IN6>();
                            Some(std::net::IpAddr::from(value.sin6_addr.u.Byte).to_canonical())
                        }
                        _ => None,
                    };
                    if candidate == Some(address) && row.PhysicalAddressLength == 6 {
                        return Ok(row.PhysicalAddress[..6]
                            .iter()
                            .map(|n| format!("{n:02X}"))
                            .collect::<Vec<_>>()
                            .join(":"));
                    }
                    unicast = ip.Next;
                }
                adapter = row.Next;
            }
        }
        break;
    }
    Ok("00:00:00:00:00:00".into())
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
        // SAFETY: As in local_mac, the linked records live in `storage`, and each sockaddr is
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
/// Active Ethernet/Wi-Fi IPv4 addresses for Moonlight's manual Add PC flow.
pub fn lan_addresses() -> Result<Vec<String>> {
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
        // SAFETY: As in local_mac, the linked records live in `storage`, and each sockaddr is read
        // as SOCKADDR_IN only when non-null and long enough; `interface` is a local.
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
                                    addresses.push((hardware, address));
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
        addresses.sort_by_key(|(hardware, address)| (!hardware, !address.is_private(), *address));
        addresses.dedup_by_key(|(_, address)| *address);
        return Ok(addresses
            .into_iter()
            .map(|(_, address)| address.to_string())
            .collect());
    }
    anyhow::bail!("network interfaces changed during enumeration")
}
pub fn configure_udp(socket: &UdpSocket) -> Result<()> {
    let disabled = 0u32;
    let mut returned = 0;
    // SAFETY: The borrowed socket stays open, `disabled` is the 4-byte input buffer, and the call
    // is synchronous with no output buffer.
    if unsafe {
        WSAIoctl(
            SOCKET(socket.as_raw_socket() as usize),
            SIO_UDP_CONNRESET,
            Some((&disabled as *const u32).cast()),
            4,
            None,
            0,
            &mut returned,
            None,
            None,
        )
    } == SOCKET_ERROR
    {
        // SAFETY: WSAGetLastError only reads this thread's last Winsock error.
        return Err(std::io::Error::from_raw_os_error(unsafe { WSAGetLastError() }.0).into());
    }
    // Vibepollo's video socket buffer: a key frame's burst fits without the
    // non-blocking socket refusing it.
    socket2::SockRef::from(socket).set_send_buffer_size(1 << 20)?;
    Ok(())
}
/// Send errors after which the socket still works and only these datagrams
/// are lost: a full send buffer, no buffer space, or a route that is down for
/// the moment. Vibepollo drops the packets and keeps streaming; so do we.
fn transient(error: WSA_ERROR) -> bool {
    [
        WSAEWOULDBLOCK,
        WSAENOBUFS,
        WSAENETDOWN,
        WSAENETUNREACH,
        WSAENETRESET,
        WSAEHOSTUNREACH,
        WSAEHOSTDOWN,
        WSAECONNRESET,
        WSAEADDRNOTAVAIL,
    ]
    .contains(&error)
}
/// A send or receive error that loses only this datagram; the socket keeps
/// working. A receive also reports a datagram too large for its buffer this
/// way (WSAEMSGSIZE) after discarding it.
pub fn datagram_lost(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        || error.raw_os_error().is_some_and(|code| {
            let code = WSA_ERROR(code);
            transient(code) || code == WSAEMSGSIZE
        })
}
/// How long one send, or all of one batch's sends together, wait for room in
/// a full socket buffer before dropping. Per datagram, 64 refused datagrams
/// would hold the sender for 256 ms.
const WRITABLE_WAIT: std::time::Duration = std::time::Duration::from_millis(4);
const BUFFER_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(1);
/// The writable wait left to one send call, started at its first full buffer.
#[derive(Default)]
struct WaitBudget {
    deadline: Option<std::time::Instant>,
    backoff_timer: Option<crate::timing::Timer>,
}
impl WaitBudget {
    fn deadline_at(&mut self, now: std::time::Instant) -> std::time::Instant {
        *self.deadline.get_or_insert(now + WRITABLE_WAIT)
    }
    /// Milliseconds to wait for a full buffer now; 0 once the budget is spent.
    fn take(&mut self, now: std::time::Instant) -> i32 {
        let left = self.deadline_at(now).saturating_duration_since(now);
        left.as_micros().div_ceil(1000) as i32
    }
    /// Reserve a short pause only when it leaves time for the retry. All
    /// datagrams in a batch share this deadline, including time already spent
    /// polling a full socket or sending earlier datagrams.
    fn backoff_at(&mut self, now: std::time::Instant) -> Option<std::time::Instant> {
        let retry = now + BUFFER_RETRY_DELAY;
        (retry < self.deadline_at(now)).then_some(retry)
    }
}
/// Wait until `socket` has a datagram to read, or `timeout_ms` passes;
/// returns whether one is there. Unlike a fixed sleep, input waiting on the
/// socket is handled the moment it arrives.
pub fn wait_readable(
    socket: std::os::windows::io::RawSocket,
    timeout_ms: i32,
) -> std::io::Result<bool> {
    wait_readable_sockets([socket], timeout_ms)
}
/// Include host feedback in the same wait as incoming control traffic.
pub fn wait_readable_or_wake(
    socket: std::os::windows::io::RawSocket,
    wake: std::os::windows::io::RawSocket,
    timeout_ms: i32,
) -> std::io::Result<bool> {
    wait_readable_sockets([socket, wake], timeout_ms)
}
fn wait_readable_sockets<const N: usize>(
    sockets: [std::os::windows::io::RawSocket; N],
    timeout_ms: i32,
) -> std::io::Result<bool> {
    let mut poll = sockets.map(|socket| WSAPOLLFD {
        fd: SOCKET(socket as usize),
        events: POLLRDNORM,
        revents: WSAPOLL_EVENT_FLAGS(0),
    });
    // SAFETY: `poll` holds N entries and outlives the call; a stale socket only makes
    // WSAPoll fail.
    match unsafe { WSAPoll(poll.as_mut_ptr(), N as u32, timeout_ms) } {
        ready if ready >= 0 => Ok(ready > 0),
        _ => Err(std::io::Error::from_raw_os_error(
            // SAFETY: WSAGetLastError only reads this thread's last Winsock error.
            unsafe { WSAGetLastError() }.0,
        )),
    }
}
#[derive(Clone, Copy, Default)]
pub struct WritableWaits {
    pub count: u64,
    pub elapsed: std::time::Duration,
    /// Last transient socket refusal, including one followed by a successful retry.
    pub last_error: i32,
}
/// ENOBUFS can come from transport/driver allocation pressure while the socket
/// still polls writable. Pause briefly before one retry rather than hot-polling
/// that readiness bit or dropping an entire segmented batch immediately.
fn buffer_available(
    socket: &UdpSocket,
    error: WSA_ERROR,
    budget: &mut WaitBudget,
    mut trace: Option<&mut WritableWaits>,
) -> bool {
    if let Some(trace) = trace.as_mut() {
        trace.last_error = error.0;
    }
    if error == WSAEWOULDBLOCK {
        return writable(socket, budget, trace);
    }
    debug_assert_eq!(error, WSAENOBUFS);
    let started = std::time::Instant::now();
    let Some(retry_at) = budget.backoff_at(started) else {
        return false;
    };
    if budget.backoff_timer.is_none() {
        budget.backoff_timer = crate::timing::Timer::new().ok();
    }
    let Some(timer) = budget.backoff_timer.as_ref() else {
        return false;
    };
    timer.until(retry_at);
    let finished = std::time::Instant::now();
    if let Some(trace) = trace {
        trace.count += 1;
        trace.elapsed += finished.saturating_duration_since(started);
    }
    // A delayed timer wake must not revive an expired batch budget.
    finished < budget.deadline_at(started)
}

/// A successful UDP send must never be repeated. Retry only buffer pressure,
/// once, after the caller's shared wait budget permits it; all other errors
/// retain their existing drop/fatal handling.
fn send_with_retry<T>(
    mut send: impl FnMut() -> std::result::Result<T, WSA_ERROR>,
    wait: impl FnOnce(WSA_ERROR) -> bool,
) -> std::result::Result<T, WSA_ERROR> {
    match send() {
        Err(error) if [WSAEWOULDBLOCK, WSAENOBUFS].contains(&error) && wait(error) => send(),
        result => result,
    }
}
fn writable(
    socket: &UdpSocket,
    budget: &mut WaitBudget,
    trace: Option<&mut WritableWaits>,
) -> bool {
    let timeout_ms = budget.take(std::time::Instant::now());
    if timeout_ms == 0 {
        return false;
    }
    let started = trace.as_ref().map(|_| std::time::Instant::now());
    let mut poll = [WSAPOLLFD {
        fd: SOCKET(socket.as_raw_socket() as usize),
        events: POLLWRNORM,
        revents: WSAPOLL_EVENT_FLAGS(0),
    }];
    // SAFETY: `poll` is a one-element array that outlives the call, for a socket the borrow keeps
    // open.
    let ready = (unsafe { WSAPoll(poll.as_mut_ptr(), 1, timeout_ms) }) > 0
        && poll[0].revents.0 & POLLWRNORM.0 != 0;
    if let (Some(trace), Some(started)) = (trace, started) {
        trace.count += 1;
        trace.elapsed += started.elapsed();
    }
    ready
}
/// One datagram from `source`, retried once when the socket buffer is full
/// and frees up within `budget`. The inner error is Winsock's.
fn send_one(
    socket: &UdpSocket,
    packet: &[u8],
    peer: SocketAddr,
    source: Option<IpAddr>,
    budget: &mut WaitBudget,
    trace: Option<&mut WritableWaits>,
) -> Result<std::result::Result<(), WSA_ERROR>> {
    let send = || send_from(socket, packet, peer, source);
    let result = send_with_retry(send, |error| buffer_available(socket, error, budget, trace));
    Ok(match result {
        Ok(size) => {
            anyhow::ensure!(
                size == packet.len(),
                "Winsock returned an incomplete UDP datagram"
            );
            Ok(())
        }
        Err(error) => Err(error),
    })
}
/// One datagram, from the address `peer` reached; `Ok(false)` when a
/// transient error dropped it.
pub fn send_datagram(socket: &ReplySocket, packet: &[u8], peer: SocketAddr) -> Result<bool> {
    let mut budget = WaitBudget::default();
    let (result, _) = socket.send_pinned(peer, socket.source(peer), |source| {
        send_one(socket, packet, peer, source, &mut budget, None)
    })?;
    match result {
        Ok(()) => Ok(true),
        Err(error) if transient(error) => Ok(false),
        Err(error) => Err(std::io::Error::from_raw_os_error(error.0).into()),
    }
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
fn interface_link(row: &windows::Win32::NetworkManagement::IpHelper::MIB_IF_ROW2) -> Link {
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
fn underlying_link(
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
        if GetIfEntry2(&mut row).0 != 0 || row.OperStatus != IfOperStatusUp {
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
            (GetIfEntry2(&mut row).0 == 0).then_some(row)
        });
        FreeMibTable(table.cast());
        Link { interface, ..link }
    }
}
/// Physical Ethernet speed on the route to this client; zero means unknown.
pub fn routed_link_bps(peer: SocketAddr) -> u64 {
    routed_link(peer).bps
}
/// qWAVE, loaded at run time as Vibepollo does: Windows Server has
/// qwave.dll only with its Quality Windows Audio Video Experience feature.
struct Qwave {
    handle: usize,
    add: QosAdd,
    remove: QosRemove,
}
type QosAdd = unsafe extern "system" fn(
    windows::Win32::Foundation::HANDLE,
    SOCKET,
    *const SOCKADDR,
    i32,
    u32,
    *mut u32,
) -> windows::core::BOOL;
type QosRemove = unsafe extern "system" fn(
    windows::Win32::Foundation::HANDLE,
    SOCKET,
    u32,
    u32,
) -> windows::core::BOOL;
fn qwave() -> Option<&'static Qwave> {
    use windows::Win32::{Foundation::HANDLE, System::LibraryLoader::*};
    #[repr(C)]
    struct Version {
        major: u16,
        minor: u16,
    }
    type Create = unsafe extern "system" fn(*const Version, *mut HANDLE) -> windows::core::BOOL;
    static QWAVE: std::sync::OnceLock<Option<Qwave>> = std::sync::OnceLock::new();
    // SAFETY: qwave.dll comes from System32 and is never freed, so its pointers, transmuted to
    // their documented signatures, stay valid; `Version` and `handle` outlive the call.
    QWAVE
        .get_or_init(|| unsafe {
            let module = LoadLibraryExW(
                windows::core::w!("qwave.dll"),
                None,
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
            .ok()?;
            let create: Create =
                std::mem::transmute(GetProcAddress(module, windows::core::s!("QOSCreateHandle"))?);
            let add: QosAdd = std::mem::transmute(GetProcAddress(
                module,
                windows::core::s!("QOSAddSocketToFlow"),
            )?);
            let remove: QosRemove = std::mem::transmute(GetProcAddress(
                module,
                windows::core::s!("QOSRemoveSocketFromFlow"),
            )?);
            let mut handle = HANDLE::default();
            if !create(&Version { major: 1, minor: 0 }, &mut handle).as_bool() {
                tracing::warn!(error = %windows::core::Error::from_thread(), "QoS tagging is unavailable");
                return None;
            }
            Some(Qwave {
                handle: handle.0 as usize,
                add,
                remove,
            })
        })
        .as_ref()
}
/// Tags a socket's datagrams to one client for priority, as Vibepollo does
/// when Moonlight asks (it does on a local network): video as audio-video
/// traffic (DSCP 40, 802.1p 5), audio as voice (DSCP 56, 802.1p 7). Wi-Fi
/// sends those from WMM's video and voice queues, ahead of other traffic,
/// and switches that honour the marks do the same. The tag lasts until the
/// flow is dropped.
pub struct QosFlow(u32);
impl QosFlow {
    /// `None` where there is nothing to tag: no qWAVE, a client on this PC,
    /// or an IPv4 client of a dual-stack socket, which qWAVE cannot tag
    /// without connecting the socket that other sessions share.
    pub fn new(socket: &UdpSocket, peer: SocketAddr, voice: bool) -> Result<Option<Self>> {
        const AUDIO_VIDEO: i32 = 3;
        const VOICE: i32 = 4;
        const NON_ADAPTIVE_FLOW: u32 = 2;
        let mapped = matches!(peer.ip(), std::net::IpAddr::V6(ip) if ip.to_ipv4_mapped().is_some());
        if peer.ip().to_canonical().is_loopback() || mapped {
            return Ok(None);
        }
        let Some(qwave) = qwave() else {
            return Ok(None);
        };
        let address = socket2::SockAddr::from(peer);
        let mut flow = 0;
        // SAFETY: `add` stays valid because qwave.dll is never freed, the handle is from
        // QOSCreateHandle, and `address` and `flow` outlive the call.
        let added = unsafe {
            (qwave.add)(
                windows::Win32::Foundation::HANDLE(qwave.handle as *mut _),
                SOCKET(socket.as_raw_socket() as usize),
                address.as_ptr().cast(),
                if voice { VOICE } else { AUDIO_VIDEO },
                NON_ADAPTIVE_FLOW,
                &mut flow,
            )
        };
        if !added.as_bool() {
            return Err(windows::core::Error::from_thread().into());
        }
        Ok(Some(Self(flow)))
    }
}
impl Drop for QosFlow {
    fn drop(&mut self) {
        if let Some(qwave) = qwave() {
            // SAFETY: The function and handle come from the never-freed Qwave, and `self.0` is the
            // flow QOSAddSocketToFlow returned; it is removed only here.
            unsafe {
                let _ = (qwave.remove)(
                    windows::Win32::Foundation::HANDLE(qwave.handle as *mut _),
                    SOCKET(0),
                    self.0,
                    0,
                );
            }
        }
    }
}
/// Per-message segmentation leaves the shared socket's options untouched.
pub struct Batch {
    offload: bool,
    pub system_calls: u64,
    /// Datagrams lost to transient send errors.
    pub dropped: u64,
    /// Enabled by the sender only for pacing traces.
    pub waits: Option<WritableWaits>,
    unreported: u64,
    reported: Option<std::time::Instant>,
}
impl Default for Batch {
    fn default() -> Self {
        Self {
            offload: true,
            system_calls: 0,
            dropped: 0,
            waits: None,
            unreported: 0,
            reported: None,
        }
    }
}
impl Batch {
    pub fn count(packets: &[Vec<u8>], limit: usize) -> usize {
        let Some(first) = packets.first() else {
            return 0;
        };
        let size = first.len().max(1);
        let limit = limit.min(65507);
        packets
            .iter()
            .take((limit / size).clamp(1, 64))
            .take_while(|p| p.len() == first.len())
            .count()
            .max(1)
    }
    /// Count dropped datagrams and report them at most every 5 seconds.
    fn dropped(&mut self, count: usize, error: WSA_ERROR) {
        if let Some(trace) = self.waits.as_mut() {
            trace.last_error = error.0;
        }
        self.dropped += count as u64;
        self.unreported += count as u64;
        if self
            .reported
            .is_none_or(|at| at.elapsed() >= std::time::Duration::from_secs(5))
        {
            tracing::warn!(
                code = error.0,
                dropped = self.unreported,
                "UDP send failed; packets dropped and the client may stutter. Lower bitrate and check the network adapter"
            );
            self.unreported = 0;
            self.reported = Some(std::time::Instant::now());
        }
    }
    /// Count datagrams as refused by a full socket without sending them,
    /// for `BUTTERPOLLO_TEST_SEND_OUTAGE`.
    pub fn refuse(&mut self, count: usize) {
        self.dropped(count, WSAEWOULDBLOCK);
    }
    /// Bytes handed to Winsock. Datagrams that hit a transient error are
    /// dropped, as Vibepollo does, rather than ending the stream. The batch
    /// shares one writable wait: under congestion it drops rather than
    /// holding the frame for a wait per datagram. They leave from the
    /// address `peer` reached.
    pub fn send(
        &mut self,
        socket: &ReplySocket,
        packets: &[Vec<u8>],
        peer: SocketAddr,
    ) -> Result<usize> {
        if packets.is_empty() {
            return Ok(0);
        }
        let mut source = socket.source(peer);
        let mut budget = WaitBudget::default();
        if self.offload
            && packets.len() > 1
            && packets.len() <= 64
            && packets.iter().all(|p| p.len() == packets[0].len())
        {
            let result;
            (result, source) = socket.send_pinned(peer, source, |source| {
                self.send_segmented(socket, packets, peer, source, &mut budget)
            })?;
            match result {
                Ok(sent) => return Ok(sent),
                Err(error)
                    if [WSAEINVAL, WSAENOPROTOOPT, WSAEOPNOTSUPP, WSAEMSGSIZE].contains(&error) =>
                {
                    self.offload = false;
                    tracing::warn!(
                        code = error.0,
                        "UDP segmentation unavailable; using individual datagrams with more CPU overhead. Update the network driver or lower bitrate if sending stalls"
                    );
                }
                Err(error) if transient(error) => {
                    self.dropped(packets.len(), error);
                    return Ok(0);
                }
                Err(error) => return Err(std::io::Error::from_raw_os_error(error.0).into()),
            }
        }
        let mut sent = 0;
        for packet in packets {
            let result;
            (result, source) = socket.send_pinned(peer, source, |source| {
                self.system_calls += 1;
                send_one(
                    socket,
                    packet,
                    peer,
                    source,
                    &mut budget,
                    self.waits.as_mut(),
                )
            })?;
            match result {
                Ok(()) => sent += packet.len(),
                Err(error) if transient(error) => self.dropped(1, error),
                Err(error) => return Err(std::io::Error::from_raw_os_error(error.0).into()),
            }
        }
        Ok(sent)
    }
    /// One WSASendMsg with UDP segmentation from `source`, retried once when
    /// the socket buffer is full and frees up within `budget`. The inner error
    /// is Winsock's.
    fn send_segmented(
        &mut self,
        socket: &UdpSocket,
        packets: &[Vec<u8>],
        peer: SocketAddr,
        source: Option<IpAddr>,
        budget: &mut WaitBudget,
    ) -> Result<std::result::Result<usize, WSA_ERROR>> {
        let mut control = Ancillary::default();
        control.push(
            IPPROTO_UDP,
            UDP_SEND_MSG_SIZE,
            u32::try_from(packets[0].len())?,
        );
        if let Some(source) = source {
            control.source(source);
        }
        let address = socket2::SockAddr::from(peer);
        let mut buffers = [WSABUF {
            len: 0,
            buf: windows::core::PSTR::null(),
        }; 64];
        for (buffer, packet) in buffers.iter_mut().zip(packets) {
            buffer.len = packet.len() as u32;
            buffer.buf = windows::core::PSTR(packet.as_ptr().cast_mut());
        }
        // The only caller limits packets to 64.
        let buffers = &buffers[..packets.len()];
        let expected: usize = packets.iter().map(Vec::len).sum();
        let system_calls = &mut self.system_calls;
        let result = send_with_retry(
            || {
                *system_calls += 1;
                send_message(socket, buffers, &address, &control)
            },
            |error| buffer_available(socket, error, budget, self.waits.as_mut()),
        );
        Ok(match result {
            Ok(sent) => {
                anyhow::ensure!(sent == expected, "Winsock returned an incomplete UDP batch");
                Ok(sent)
            }
            Err(error) => Err(error),
        })
    }
}
/// WSASendMsg control data: whole messages, each padded to 8 bytes as
/// WSA_CMSG_SPACE pads them.
#[repr(C, align(8))]
struct Ancillary {
    bytes: [u8; 64],
    len: usize,
}
impl Default for Ancillary {
    fn default() -> Self {
        Self {
            bytes: [0; 64],
            len: 0,
        }
    }
}
impl Ancillary {
    fn push<T: Copy>(&mut self, level: IPPROTO, kind: i32, data: T) {
        let header = std::mem::size_of::<CMSGHDR>();
        let size = header + std::mem::size_of::<T>();
        let end = self.len + size.next_multiple_of(8);
        assert!(end <= self.bytes.len(), "control data too large");
        // SAFETY: the header and `data` end within `bytes` (asserted above), and unaligned writes
        // need no alignment.
        unsafe {
            let at = self.bytes.as_mut_ptr().add(self.len);
            at.cast::<CMSGHDR>().write_unaligned(CMSGHDR {
                cmsg_len: size,
                cmsg_level: level.0,
                cmsg_type: kind,
            });
            at.add(header).cast::<T>().write_unaligned(data);
        }
        self.len = end;
    }
    /// Send from `source`, as Moonlight's ENet does: IP_PKTINFO for IPv4, also
    /// on a dual-stack socket, IPV6_PKTINFO for IPv6.
    fn source(&mut self, source: IpAddr) {
        match source.to_canonical() {
            IpAddr::V4(source) => self.push(
                IPPROTO_IP,
                IP_PKTINFO,
                IN_PKTINFO {
                    ipi_addr: IN_ADDR {
                        S_un: IN_ADDR_0 {
                            S_addr: u32::from_ne_bytes(source.octets()),
                        },
                    },
                    ipi_ifindex: 0,
                },
            ),
            IpAddr::V6(source) => self.push(
                IPPROTO_IPV6,
                IPV6_PKTINFO,
                IN6_PKTINFO {
                    ipi6_addr: IN6_ADDR {
                        u: IN6_ADDR_0 {
                            Byte: source.octets(),
                        },
                    },
                    ipi6_ifindex: 0,
                },
            ),
        }
    }
}
/// One WSASendMsg of `buffers` to `peer` with `control`. The error is Winsock's.
fn send_message(
    socket: &UdpSocket,
    buffers: &[WSABUF],
    peer: &socket2::SockAddr,
    control: &Ancillary,
) -> std::result::Result<usize, WSA_ERROR> {
    let message = WSAMSG {
        name: peer.as_ptr().cast_mut().cast(),
        namelen: peer.len(),
        // WSASendMsg only reads the buffers.
        lpBuffers: buffers.as_ptr().cast_mut(),
        dwBufferCount: buffers.len() as u32,
        Control: WSABUF {
            len: control.len as u32,
            buf: windows::core::PSTR(control.bytes.as_ptr().cast_mut()),
        },
        dwFlags: 0,
    };
    let mut sent = 0;
    // SAFETY: `message` and all it points to outlive this synchronous call, and dwBufferCount and
    // the control length stay within `buffers` and `control.bytes`.
    let result = unsafe {
        WSASendMsg(
            SOCKET(socket.as_raw_socket() as usize),
            &message,
            0,
            Some(&mut sent),
            None,
            None,
        )
    };
    if result == 0 {
        Ok(sent as usize)
    } else {
        // SAFETY: WSAGetLastError only reads this thread's last Winsock error.
        Err(unsafe { WSAGetLastError() })
    }
}
/// One datagram to `peer`, from `source` when it is known. The error is Winsock's.
fn send_from(
    socket: &UdpSocket,
    packet: &[u8],
    peer: SocketAddr,
    source: Option<IpAddr>,
) -> std::result::Result<usize, WSA_ERROR> {
    let Some(source) = source else {
        return socket
            .send_to(packet, peer)
            .map_err(|e| WSA_ERROR(e.raw_os_error().unwrap_or(WSAEINVAL.0)));
    };
    let mut control = Ancillary::default();
    control.source(source);
    let buffer = WSABUF {
        len: packet.len() as u32,
        buf: windows::core::PSTR(packet.as_ptr().cast_mut()),
    };
    send_message(socket, &[buffer], &socket2::SockAddr::from(peer), &control)
}
/// Spoofed senders cannot grow a ReplySocket's table without bound; a client
/// still streaming is learned again from its next datagram.
const REPLY_SOURCES: usize = 256;
/// A UDP socket that answers each client from the local address the client's
/// datagrams arrived on, as moonlight-common-c's ENet does. Bound to every
/// address, Windows otherwise picks the source by route: on a PC where a VPN
/// such as Tailscale also routes the LAN, replies to a LAN client could leave
/// from the VPN's address, and the client drops them (issue #54).
///
/// Every send through it (`send_to`, `Batch::send`, `send_datagram`) uses
/// what `answer_from` learned for that peer. A peer Windows already routes
/// from the address it reached, as on a plain LAN, is answered as before,
/// with plain sendto and without taking the table's lock while no peer is
/// pinned.
pub struct ReplySocket {
    socket: UdpSocket,
    receive_message: unsafe extern "system" fn(
        SOCKET,
        *mut WSAMSG,
        *mut u32,
        *mut windows::Win32::System::IO::OVERLAPPED,
        LPWSAOVERLAPPED_COMPLETION_ROUTINE,
    ) -> i32,
    /// The address Windows sends to a peer from; `routed_source` but in tests.
    route: fn(SOCKET, SocketAddr) -> std::io::Result<IpAddr>,
    /// The time; `Instant::now` but in tests.
    now: fn() -> std::time::Instant,
    sources: RwLock<Sources>,
    /// Whether any peer in `sources` is pinned, so sends skip the lock
    /// while none is.
    any_pinned: AtomicBool,
    /// Whether a datagram without its local address was logged.
    missing_reported: AtomicBool,
    /// Whether a failed route lookup was logged.
    route_reported: AtomicBool,
}
/// How often a peer's route is checked again while its datagrams arrive, so
/// a VPN coming up or going down mid-stream is noticed.
const ROUTE_CHECK: std::time::Duration = std::time::Duration::from_secs(1);
/// How soon after the last check a failed send has the route checked again.
const RECHECK: std::time::Duration = std::time::Duration::from_millis(100);
/// How long an address Windows refused to send to a peer from is not pinned
/// again; a refusal that lasts costs a failed send and its retry this often.
const REFUSAL: std::time::Duration = std::time::Duration::from_secs(5);
/// What a ReplySocket learned about one peer.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Reached {
    /// The local address the peer's datagrams arrive on.
    local: IpAddr,
    /// Whether Windows routes replies from another address, so they must be
    /// sent from `local`.
    pinned: bool,
    /// When the route was last checked.
    checked: std::time::Instant,
    /// Whether a send to the peer failed as unreachable since.
    recheck: bool,
}
/// What a ReplySocket learned per peer, each table holding at most
/// REPLY_SOURCES entries and evicting the oldest.
#[derive(Default)]
struct Sources {
    /// What each peer last reached, and when it was learned.
    pins: HashMap<SocketAddr, (Reached, u64)>,
    /// The local addresses Windows refused to send to a peer from, and when;
    /// not pinned again for REFUSAL, as a VPN that went down may come back.
    refused: HashMap<(SocketAddr, IpAddr), (std::time::Instant, u64)>,
    /// Orders both tables' entries.
    clock: u64,
}
impl Sources {
    /// Inserts into `table`, evicting its oldest entry when full; whether
    /// `key` is new.
    fn remember<K: Copy + Eq + std::hash::Hash, V>(
        table: &mut HashMap<K, (V, u64)>,
        clock: &mut u64,
        key: K,
        value: V,
    ) -> bool {
        if table.len() >= REPLY_SOURCES
            && !table.contains_key(&key)
            && let Some(oldest) = table.iter().min_by_key(|(_, (_, at))| *at).map(|(k, _)| *k)
        {
            table.remove(&oldest);
        }
        *clock += 1;
        table.insert(key, (value, *clock)).is_none()
    }
    /// Whether Windows refused to send to `peer` from `local` less than
    /// REFUSAL before `now`.
    fn refusing(&self, peer: SocketAddr, local: IpAddr, now: std::time::Instant) -> bool {
        self.refused
            .get(&(peer, local))
            .is_some_and(|(at, _)| now.duration_since(*at) < REFUSAL)
    }
    /// Whether a datagram from `peer` to `local` at `now` changes nothing:
    /// the same address with a fresh route, or one refused for it.
    fn settled(&self, peer: SocketAddr, local: Option<IpAddr>, now: std::time::Instant) -> bool {
        let reached = self.pins.get(&peer).map(|(reached, _)| reached);
        match local {
            Some(local) if self.refusing(peer, local, now) => reached.is_none(),
            Some(local) => reached.is_some_and(|reached| {
                let age = now.duration_since(reached.checked);
                reached.local == local && age < ROUTE_CHECK && !(reached.recheck && age >= RECHECK)
            }),
            None => reached.is_none(),
        }
    }
}
/// The errors with which Windows refuses to send from a pinned address: one
/// it no longer has (WSAEADDRNOTAVAIL, or WSAEINVAL for one it never had),
/// or one whose interface has no route to the peer (WSAENETUNREACH,
/// WSAEHOSTUNREACH), as on a Hyper-V switch or a Tailscale subnet router.
/// WSAEINVAL is also how a NIC without UDP segmentation fails.
const REFUSED: [WSA_ERROR; 4] = [WSAEINVAL, WSAEADDRNOTAVAIL, WSAENETUNREACH, WSAEHOSTUNREACH];
impl ReplySocket {
    /// Ask Windows for each datagram's local address on `socket`.
    pub fn new(socket: UdpSocket) -> std::io::Result<Self> {
        let raw = SOCKET(socket.as_raw_socket() as usize);
        let ipv6 = socket.local_addr()?.is_ipv6();
        let mut options = Vec::with_capacity(2);
        // An IPv6-only socket receives no IPv4 datagrams.
        if !ipv6 || !socket2::SockRef::from(&socket).only_v6()? {
            options.push((IPPROTO_IP, IP_PKTINFO));
        }
        if ipv6 {
            options.push((IPPROTO_IPV6, IPV6_PKTINFO));
        }
        for (level, option) in options {
            // SAFETY: the borrowed socket stays open, and the option takes this 4-byte BOOL.
            if unsafe { setsockopt(raw, level.0, option, Some(&1u32.to_ne_bytes())) }
                == SOCKET_ERROR
            {
                return Err(last_error());
            }
        }
        let mut receive_message: LPFN_WSARECVMSG = None;
        let mut returned = 0;
        // SAFETY: the input is the GUID and the output the function pointer's own storage, both
        // outliving this synchronous call on the open socket.
        if unsafe {
            WSAIoctl(
                raw,
                SIO_GET_EXTENSION_FUNCTION_POINTER,
                Some((&WSAID_WSARECVMSG as *const windows::core::GUID).cast()),
                std::mem::size_of::<windows::core::GUID>() as u32,
                Some((&mut receive_message as *mut LPFN_WSARECVMSG).cast()),
                std::mem::size_of::<LPFN_WSARECVMSG>() as u32,
                &mut returned,
                None,
                None,
            )
        } == SOCKET_ERROR
        {
            return Err(last_error());
        }
        Ok(Self {
            socket,
            receive_message: receive_message
                .ok_or_else(|| std::io::Error::other("Winsock returned no WSARecvMsg"))?,
            sources: RwLock::default(),
            route: routed_source,
            now: std::time::Instant::now,
            any_pinned: AtomicBool::new(false),
            missing_reported: AtomicBool::new(false),
            route_reported: AtomicBool::new(false),
        })
    }
    pub fn bind(address: impl ToSocketAddrs) -> std::io::Result<Self> {
        Self::new(UdpSocket::bind(address)?)
    }
    /// One datagram: its size, its sender, and the local address it arrived
    /// on. A datagram larger than `buffer` is lost with WSAEMSGSIZE, as
    /// recv_from loses it.
    pub fn receive(
        &self,
        buffer: &mut [u8],
    ) -> std::io::Result<(usize, SocketAddr, Option<IpAddr>)> {
        #[repr(C, align(8))]
        struct Control([u8; 128]);
        let mut control = Control([0; 128]);
        let mut data = WSABUF {
            len: buffer.len().try_into().unwrap_or(u32::MAX),
            buf: windows::core::PSTR(buffer.as_mut_ptr()),
        };
        // SAFETY: WSARecvMsg writes at most `length` bytes of sender address into the storage
        // try_init provides and the length it wrote; `data`, `control` and `message` outlive this
        // synchronous call on the open socket.
        let ((size, flags, control_len), peer) = unsafe {
            socket2::SockAddr::try_init(|storage, length| {
                let mut message = WSAMSG {
                    name: storage.cast(),
                    namelen: *length,
                    lpBuffers: &mut data,
                    dwBufferCount: 1,
                    Control: WSABUF {
                        len: control.0.len() as u32,
                        buf: windows::core::PSTR(control.0.as_mut_ptr()),
                    },
                    dwFlags: 0,
                };
                let mut size = 0;
                if (self.receive_message)(
                    SOCKET(self.socket.as_raw_socket() as usize),
                    &mut message,
                    &mut size,
                    std::ptr::null_mut(),
                    None,
                ) == SOCKET_ERROR
                {
                    return Err(last_error());
                }
                *length = message.namelen;
                Ok((size as usize, message.dwFlags, message.Control.len as usize))
            })?
        };
        if flags & MSG_PARTIAL != 0 {
            return Err(std::io::Error::from_raw_os_error(WSAEMSGSIZE.0));
        }
        let peer = peer
            .as_socket()
            .ok_or_else(|| std::io::Error::other("a datagram from a non-IP sender"))?;
        let local = local_address(&control.0[..control_len.min(control.0.len())]);
        if local.is_none() && !self.missing_reported.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                %peer,
                "Windows did not report which address a datagram arrived on; replies to it leave from the address Windows routes by"
            );
        }
        Ok((size, peer, local))
    }
    /// Answer `peer` from `local`, the address its datagram arrived on,
    /// unless Windows refused that within REFUSAL. Asks which address Windows routes
    /// to `peer` from when `local` is new, and again at most every
    /// ROUTE_CHECK, here on the receiving thread rather than on a send.
    pub fn answer_from(&self, peer: SocketAddr, local: Option<IpAddr>) {
        let now = (self.now)();
        if self.sources.read().unwrap().settled(peer, local, now) {
            return;
        }
        let local = local.filter(|local| !self.sources.read().unwrap().refusing(peer, *local, now));
        let Some(local) = local else {
            let mut sources = self.sources.write().unwrap();
            sources.pins.remove(&peer);
            self.note_pins(&sources);
            return;
        };
        let previous = self
            .sources
            .read()
            .unwrap()
            .pins
            .get(&peer)
            .filter(|(reached, _)| reached.local == local)
            .map(|(reached, _)| reached.pinned);
        let pinned = self.routes_elsewhere(peer, local, previous);
        let mut sources = self.sources.write().unwrap();
        let Sources {
            pins,
            refused,
            clock,
        } = &mut *sources;
        refused.remove(&(peer, local));
        Sources::remember(
            pins,
            clock,
            peer,
            Reached {
                local,
                pinned,
                checked: now,
                recheck: false,
            },
        );
        self.note_pins(&sources);
    }
    /// Forget what `peer` reached, once its connection or session ended.
    pub fn forget(&self, peer: SocketAddr) {
        let mut sources = self.sources.write().unwrap();
        sources.pins.remove(&peer);
        sources.refused.retain(|(refused, _), _| *refused != peer);
        self.note_pins(&sources);
    }
    /// Records whether any peer is pinned, after `sources` changed under
    /// its write lock.
    fn note_pins(&self, sources: &Sources) {
        let any = sources.pins.values().any(|(reached, _)| reached.pinned);
        self.any_pinned.store(any, Ordering::Release);
    }
    /// The address replies to `peer` must leave from; None leaves it to
    /// Windows, which routes them from the address the peer reached or
    /// knows nothing better.
    pub fn source(&self, peer: SocketAddr) -> Option<IpAddr> {
        if !self.any_pinned.load(Ordering::Acquire) {
            return None;
        }
        self.sources
            .read()
            .unwrap()
            .pins
            .get(&peer)
            .filter(|(reached, _)| reached.pinned)
            .map(|(reached, _)| reached.local)
    }
    /// The senders at `ip` and the address each reached, for logs.
    pub fn sources_from(&self, ip: IpAddr) -> Vec<(SocketAddr, IpAddr)> {
        let ip = ip.to_canonical();
        let mut sources: Vec<_> = self
            .sources
            .read()
            .unwrap()
            .pins
            .iter()
            .filter(|(peer, _)| peer.ip().to_canonical() == ip)
            .map(|(peer, (reached, _))| (*peer, reached.local))
            .collect();
        sources.sort();
        sources
    }
    /// One datagram to `peer`, from the address it reached.
    pub fn send_to(&self, buffer: &[u8], peer: SocketAddr) -> std::io::Result<usize> {
        let Ok((result, _)) = self.send_pinned(peer, self.source(peer), |source| {
            Ok::<_, std::convert::Infallible>(send_from(&self.socket, buffer, peer, source))
        });
        result.map_err(|error| std::io::Error::from_raw_os_error(error.0))
    }
    /// Sends with `send` from `source`, the address `peer` reached, and once
    /// more from the address Windows routes by when Windows refuses it. Only
    /// that second send succeeding shows the pin was at fault: a network
    /// that is down, or a NIC without UDP segmentation, keeps it, and the
    /// second send's error is the one the caller handles. Any other refusal
    /// has the route checked again soon (`reroute`). Returns the result and
    /// the source later sends to `peer` use.
    fn send_pinned<T, E>(
        &self,
        peer: SocketAddr,
        source: Option<IpAddr>,
        mut send: impl FnMut(Option<IpAddr>) -> Result<Result<T, WSA_ERROR>, E>,
    ) -> Result<(Result<T, WSA_ERROR>, Option<IpAddr>), E> {
        let result = send(source)?;
        let Err(error) = &result else {
            return Ok((result, source));
        };
        let error = *error;
        if !REFUSED.contains(&error) {
            return Ok((result, source));
        }
        let Some(pinned) = source else {
            self.reroute(peer);
            return Ok((result, source));
        };
        match send(None)? {
            Ok(sent) => {
                self.refuse(peer, pinned, error);
                Ok((Ok(sent), None))
            }
            Err(retry) => {
                self.reroute(peer);
                Ok((Err(retry), source))
            }
        }
    }
    /// Whether Windows routes replies to `peer` from an address other than
    /// `local`. When Windows cannot say, `previous` stands, and a new peer is
    /// answered from `local`, the address it expects.
    fn routes_elsewhere(&self, peer: SocketAddr, local: IpAddr, previous: Option<bool>) -> bool {
        match (self.route)(SOCKET(self.socket.as_raw_socket() as usize), peer) {
            Ok(routed) => routed != local,
            Err(error) => {
                if !self.route_reported.swap(true, Ordering::Relaxed) {
                    tracing::warn!(
                        %peer,
                        %local,
                        %error,
                        "Windows did not say which address it routes this client by; replying from the address the client reached"
                    );
                }
                previous.unwrap_or(true)
            }
        }
    }
    /// Asks the receiving thread to check `peer`'s route again with its next
    /// datagram, at most every RECHECK, after a send to it failed, as when a
    /// VPN came up or went down. The sending thread asks Windows nothing.
    fn reroute(&self, peer: SocketAddr) {
        let flagged = |sources: &Sources| {
            sources
                .pins
                .get(&peer)
                .is_none_or(|(reached, _)| reached.recheck)
        };
        if flagged(&self.sources.read().unwrap()) {
            return;
        }
        if let Some((reached, _)) = self.sources.write().unwrap().pins.get_mut(&peer) {
            reached.recheck = true;
        }
    }
    /// Stop answering `peer` from `source`, which Windows refused, as after a
    /// VPN disconnects or when its interface has no route back; for REFUSAL,
    /// replies leave from the address Windows routes by.
    fn refuse(&self, peer: SocketAddr, source: IpAddr, error: WSA_ERROR) {
        let now = (self.now)();
        let new = {
            let mut sources = self.sources.write().unwrap();
            let new = !sources.refusing(peer, source, now);
            let Sources {
                pins,
                refused,
                clock,
            } = &mut *sources;
            if pins
                .get(&peer)
                .is_some_and(|(reached, _)| reached.local == source)
            {
                pins.remove(&peer);
            }
            Sources::remember(refused, clock, (peer, source), now);
            self.note_pins(&sources);
            new
        };
        if new {
            tracing::warn!(
                %peer,
                %source,
                code = error.0,
                "Windows refused to send from the address the client reached; replying to this client from the address Windows routes by"
            );
        }
    }
}
/// The local address Windows sends to `peer` from, by route.
fn routed_source(socket: SOCKET, peer: SocketAddr) -> std::io::Result<IpAddr> {
    let peer = socket2::SockAddr::from(peer);
    // SAFETY: Winsock writes at most the storage's length into it, and the
    // input is the peer's own sockaddr, both outliving this synchronous call
    // on the open socket.
    let ((), routed) = unsafe {
        socket2::SockAddr::try_init(|storage, length| {
            let mut returned = 0;
            if WSAIoctl(
                socket,
                SIO_ROUTING_INTERFACE_QUERY,
                Some(peer.as_ptr().cast()),
                peer.len() as u32,
                Some(storage.cast()),
                *length as u32,
                &mut returned,
                None,
                None,
            ) == SOCKET_ERROR
            {
                return Err(last_error());
            }
            *length = returned as _;
            Ok(())
        })
    }?;
    routed
        .as_socket()
        .map(|routed| routed.ip().to_canonical())
        .ok_or_else(|| std::io::Error::other("Windows returned no IP address for the route"))
}
impl std::ops::Deref for ReplySocket {
    type Target = UdpSocket;
    fn deref(&self) -> &UdpSocket {
        &self.socket
    }
}
impl AsRawSocket for ReplySocket {
    fn as_raw_socket(&self) -> RawSocket {
        self.socket.as_raw_socket()
    }
}
impl AsSocket for ReplySocket {
    fn as_socket(&self) -> BorrowedSocket<'_> {
        self.socket.as_socket()
    }
}
/// The local address in WSARecvMsg's control data: IP_PKTINFO or IPV6_PKTINFO.
fn local_address(control: &[u8]) -> Option<IpAddr> {
    let header = std::mem::size_of::<CMSGHDR>();
    let mut offset = 0;
    while offset + header <= control.len() {
        // SAFETY: the header lies within `control`, and an unaligned read needs no alignment.
        let message = unsafe {
            control
                .as_ptr()
                .add(offset)
                .cast::<CMSGHDR>()
                .read_unaligned()
        };
        if message.cmsg_len < header || offset + message.cmsg_len > control.len() {
            return None;
        }
        let data = &control[offset + header..offset + message.cmsg_len];
        if message.cmsg_level == IPPROTO_IP.0
            && message.cmsg_type == IP_PKTINFO
            && data.len() >= std::mem::size_of::<IN_PKTINFO>()
        {
            // SAFETY: `data` holds an IN_PKTINFO (checked above), read unaligned; every bit
            // pattern is a valid address.
            let address = unsafe {
                data.as_ptr()
                    .cast::<IN_PKTINFO>()
                    .read_unaligned()
                    .ipi_addr
                    .S_un
                    .S_addr
            };
            return Some(std::net::Ipv4Addr::from(address.to_ne_bytes()).into());
        }
        if message.cmsg_level == IPPROTO_IPV6.0
            && message.cmsg_type == IPV6_PKTINFO
            && data.len() >= std::mem::size_of::<IN6_PKTINFO>()
        {
            // SAFETY: `data` holds an IN6_PKTINFO (checked above), read unaligned; every bit
            // pattern is a valid address.
            let address = unsafe {
                data.as_ptr()
                    .cast::<IN6_PKTINFO>()
                    .read_unaligned()
                    .ipi6_addr
                    .u
                    .Byte
            };
            return Some(IpAddr::from(address).to_canonical());
        }
        offset += message.cmsg_len.next_multiple_of(8);
    }
    None
}
fn last_error() -> std::io::Error {
    // SAFETY: WSAGetLastError only reads this thread's last Winsock error.
    std::io::Error::from_raw_os_error(unsafe { WSAGetLastError() }.0)
}
#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::NetworkManagement::{IpHelper::*, Ndis::*};

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
        let wait = |timeout| {
            wait_readable_or_wake(control.as_raw_socket(), feedback.as_raw_socket(), timeout)
        };
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
                let (mode, sender, batch, batches, datagrams) =
                    &mut senders[(i + round) % modes.len()];
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
                    Ok::<_, std::convert::Infallible>(if source.is_some() {
                        Err(first)
                    } else {
                        then
                    })
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
}
