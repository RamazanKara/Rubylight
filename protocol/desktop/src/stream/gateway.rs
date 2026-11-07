//! This device's gateway, while the PC is on the same link, and a ping toward the PC: what tells a
//! PC that is gone from a dropout or a restarting host ([`nightjar_core::host_loss`]). A PC behind a
//! router, a tunnel or over IPv6 gets no gateway, and then only the connection's timeout can say.

use std::net::{IpAddr, Ipv4Addr};

pub fn toward(host: IpAddr) -> Option<IpAddr> {
    let IpAddr::V4(host) = host else { return None };
    let interfaces = if_addrs::get_if_addrs().ok()?;
    let on_link = interfaces.iter().filter(|i| i.is_oper_up() && !i.is_loopback()).any(|i| match &i.addr {
        if_addrs::IfAddr::V4(v4) => same_subnet(v4.ip, v4.netmask, host),
        if_addrs::IfAddr::V6(_) => false,
    });
    if !on_link {
        return None;
    }
    default_gateway().filter(|gateway| *gateway != host).map(IpAddr::V4)
}

fn same_subnet(ip: Ipv4Addr, netmask: Ipv4Addr, host: Ipv4Addr) -> bool {
    let mask = u32::from(netmask);
    mask != 0 && u32::from(ip) & mask == u32::from(host) & mask
}

#[cfg(windows)]
fn default_gateway() -> Option<Ipv4Addr> {
    use windows::Win32::NetworkManagement::IpHelper::{GetBestRoute, MIB_IPFORWARDROW};
    let mut row = MIB_IPFORWARDROW::default();
    // The next hop toward any address off the link. Nothing is sent.
    // SAFETY: `row` outlives the call.
    let result = unsafe { GetBestRoute(u32::from_ne_bytes([192, 0, 2, 1]), None, &mut row) };
    if result != 0 || row.dwForwardNextHop == 0 {
        return None;
    }
    Some(Ipv4Addr::from(row.dwForwardNextHop.to_ne_bytes()))
}

/// This platform's ping toward the PC ([`nightjar_core::host_loss::Pinger`]).
#[cfg(windows)]
pub fn pinger() -> nightjar_core::host_loss::Pinger {
    std::sync::Arc::new(|ip, timeout| match ip {
        IpAddr::V4(ip) => icmp_echo(ip, timeout),
        IpAddr::V6(_) => None,
    })
}

#[cfg(not(windows))]
pub fn pinger() -> nightjar_core::host_loss::Pinger {
    nightjar_core::host_loss::system_pinger()
}

/// One ICMP echo through `IcmpSendEcho`, which needs no privileges: whether `ip` answered, None
/// when the ping could not go out.
#[cfg(windows)]
fn icmp_echo(ip: Ipv4Addr, timeout: std::time::Duration) -> Option<bool> {
    use windows::Win32::NetworkManagement::IpHelper::{IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho, ICMP_ECHO_REPLY};
    const PAYLOAD: &[u8] = b"nightjar";
    // Room for one reply, its payload and the ICMP error an unanswered echo may bring.
    let mut reply = [0u8; std::mem::size_of::<ICMP_ECHO_REPLY>() + PAYLOAD.len() + 8 + 64];
    // SAFETY: the handle is closed below; the buffers outlive the call and their sizes are theirs.
    unsafe {
        let handle = IcmpCreateFile().ok()?;
        let replies = IcmpSendEcho(
            handle,
            u32::from_ne_bytes(ip.octets()),
            PAYLOAD.as_ptr().cast(),
            PAYLOAD.len() as u16,
            None,
            reply.as_mut_ptr().cast(),
            reply.len() as u32,
            timeout.as_millis().clamp(1, u128::from(u32::MAX)) as u32,
        );
        let _ = IcmpCloseHandle(handle);
        if replies == 0 {
            // A timeout or an unreachable address is no answer; a failed call proves nothing.
            let error = windows::Win32::Foundation::GetLastError();
            // IP_REQ_TIMED_OUT, IP_DEST_HOST_UNREACHABLE, IP_DEST_NET_UNREACHABLE.
            return matches!(error.0, 11010 | 11003 | 11002).then_some(false);
        }
        // IP_SUCCESS: an echo reply, not an ICMP error from a router on the way.
        let first = std::ptr::read_unaligned(reply.as_ptr().cast::<ICMP_ECHO_REPLY>());
        Some(first.Status == 0)
    }
}

#[cfg(target_os = "linux")]
fn default_gateway() -> Option<Ipv4Addr> {
    parse_proc_route(&std::fs::read_to_string("/proc/net/route").ok()?)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn default_gateway() -> Option<Ipv4Addr> {
    None
}

/// The default route's gateway in `/proc/net/route`, whose addresses are little-endian hex.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn parse_proc_route(table: &str) -> Option<Ipv4Addr> {
    table.lines().skip(1).find_map(|line| {
        let mut fields = line.split_whitespace().skip(1);
        let destination = fields.next()?;
        let gateway = u32::from_str_radix(fields.next()?, 16).ok()?;
        (destination == "00000000" && gateway != 0).then(|| Ipv4Addr::from(gateway.to_le_bytes()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_on_the_subnet_is_on_the_link() {
        let ip = Ipv4Addr::new(192, 168, 4, 12);
        let mask = Ipv4Addr::new(255, 255, 255, 0);
        assert!(same_subnet(ip, mask, Ipv4Addr::new(192, 168, 4, 69)));
        assert!(!same_subnet(ip, mask, Ipv4Addr::new(192, 168, 5, 69)));
        assert!(!same_subnet(ip, Ipv4Addr::UNSPECIFIED, Ipv4Addr::new(192, 168, 4, 69)));
    }

    #[test]
    fn the_default_route_gives_the_gateway() {
        let table = "Iface\tDestination\tGateway \tFlags\n\
            wlan0\t0004A8C0\t00000000\t0001\n\
            wlan0\t00000000\t0104A8C0\t0003\n";
        assert_eq!(parse_proc_route(table), Some(Ipv4Addr::new(192, 168, 4, 1)));
        assert_eq!(parse_proc_route("Iface\tDestination\tGateway\nwlan0\t0004A8C0\t00000000\n"), None);
    }

    #[test]
    fn loopback_answers_a_ping_and_ipv6_is_not_asked() {
        let ping = pinger();
        let timeout = std::time::Duration::from_secs(1);
        if cfg!(windows) {
            assert_eq!(ping(IpAddr::V4(Ipv4Addr::LOCALHOST), timeout), Some(true));
            assert_eq!(ping("::1".parse().unwrap(), timeout), None);
        } else if let Some(answered) = ping(IpAddr::V4(Ipv4Addr::LOCALHOST), timeout) {
            // Linux pings only within net.ipv4.ping_group_range.
            assert!(answered);
        }
    }

    #[test]
    fn nothing_for_ipv6_or_a_host_off_every_link() {
        assert_eq!(toward("fe80::1".parse().unwrap()), None);
        assert_eq!(toward(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 200))), None);
    }
}
