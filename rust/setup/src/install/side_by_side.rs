//! Side-by-side port allocation, profile creation and host health probing.
use super::profile::profile_empty;
use crate::{detect, host_service, log::line};
use anyhow::{Context, Result, bail};
use std::{
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// The base ports a side-by-side installation tries, in turn. They stay clear
/// of the default 47984-48010 other hosts use and of the Windows ephemeral
/// range above 49151, and of each other's ranges.
pub(super) const SIDE_BY_SIDE_PORTS: [u16; 6] = [48589, 48689, 48789, 48889, 48989, 49089];
/// `webrtc_port`'s offset from the base port; `webrtc_media_port` and the
/// guests' four media ports follow it.
pub(super) const WEBRTC_OFFSET: u16 = 30;
/// Every port a base can use: `Ports::from_base`'s, then WebRTC's.
pub(super) fn ports_of(base: u16) -> std::ops::RangeInclusive<u16> {
    base.saturating_sub(5)..=base.saturating_add(WEBRTC_OFFSET + 5)
}
/// Decodes an IP Helper MIB `dwLocalPort` value (network byte order in the low
/// 16 bits; upper 16 bits may hold uninitialized data).
pub(super) fn mib_port(raw: u32) -> u16 {
    u16::from_be(raw as u16)
}

/// Checks whether a packed Windows MIB table buffer (`dwNumEntries` followed by
/// `table: [R; ...]`) contains a row whose local port equals `port`.
pub(super) fn table_bytes_hold_port<T, R>(
    buf: &[u32],
    rows_offset: usize,
    row_port: impl Fn(&R) -> u32,
    port: u16,
) -> bool {
    const {
        assert!(std::mem::align_of::<T>() <= std::mem::align_of::<u32>());
        assert!(std::mem::align_of::<R>() <= std::mem::align_of::<u32>());
    }
    let Some(&count) = buf.first() else {
        return false;
    };
    let count = count as usize;
    let bytes = std::mem::size_of_val(buf);
    let Some(needed) = count
        .checked_mul(std::mem::size_of::<R>())
        .and_then(|n| rows_offset.checked_add(n))
    else {
        return false;
    };
    if needed > bytes {
        return false;
    }
    // SAFETY: `buf` is 4-byte aligned, `align_of::<R>() <= 4`, and `rows_offset + count * size_of::<R>() <= bytes`.
    let rows = unsafe {
        let base = (buf.as_ptr().cast::<u8>()).add(rows_offset).cast::<R>();
        std::slice::from_raw_parts(base, count)
    };
    rows.iter().any(|row| mib_port(row_port(row)) == port)
}

fn query_mib_table(
    mut fetch: impl FnMut(Option<*mut std::ffi::c_void>, *mut u32) -> u32,
) -> Option<Vec<u32>> {
    use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
    let mut size = 0u32;
    let first = fetch(None, &mut size);
    if first == NO_ERROR.0 {
        return Some(Vec::new());
    }
    if first != ERROR_INSUFFICIENT_BUFFER.0 || size == 0 {
        return None;
    }
    for _ in 0..4 {
        let words = (size as usize).div_ceil(4).saturating_add(16);
        let mut buf = vec![0u32; words];
        let mut capacity = u32::try_from(words.saturating_mul(4)).ok()?;
        let status = fetch(Some(buf.as_mut_ptr().cast()), &mut capacity);
        if status == NO_ERROR.0 {
            return Some(buf);
        }
        if status != ERROR_INSUFFICIENT_BUFFER.0 {
            return None;
        }
        size = capacity;
    }
    None
}

/// Whether Windows's active IPv4/IPv6 TCP or UDP tables list `port` on any local
/// address (wildcard, LAN IP, or loopback) without opening a wildcard listener.
///
/// `iphlpapi.dll` is loaded from System32 at run time: as a static import,
/// Windows would look for it beside setup.exe first (in Downloads) because it
/// is not a KnownDLL.
pub(super) fn table_holds_port(port: u16) -> bool {
    use windows::{
        Win32::{
            Foundation::FreeLibrary,
            NetworkManagement::IpHelper::{
                MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID, MIB_TCPROW_LH, MIB_TCPTABLE,
                MIB_UDP6ROW_OWNER_PID, MIB_UDP6TABLE_OWNER_PID, MIB_UDPROW, MIB_UDPTABLE,
                TCP_TABLE_BASIC_ALL, TCP_TABLE_OWNER_PID_ALL, UDP_TABLE_BASIC, UDP_TABLE_OWNER_PID,
            },
            Networking::WinSock::{AF_INET, AF_INET6},
            System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW},
        },
        core::{BOOL, HSTRING, s},
    };
    type GetExtendedTable =
        unsafe extern "system" fn(*mut std::ffi::c_void, *mut u32, BOOL, u32, i32, u32) -> u32;
    // SAFETY: `iphlpapi.dll` is loaded from System32 only and freed once below
    // after all table queries return; signatures match iphlpapi.h.
    let Ok(library) = (unsafe {
        LoadLibraryExW(
            &HSTRING::from("iphlpapi.dll"),
            None,
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    }) else {
        return false;
    };
    let held = (|| -> bool {
        // SAFETY: `library` is a live handle to System32\iphlpapi.dll until `FreeLibrary` below.
        let (Some(tcp_proc), Some(udp_proc)) = (unsafe {
            (
                GetProcAddress(library, s!("GetExtendedTcpTable")),
                GetProcAddress(library, s!("GetExtendedUdpTable")),
            )
        }) else {
            return false;
        };
        // SAFETY: both symbols are from System32\iphlpapi.dll and match `GetExtendedTable`.
        let get_tcp: GetExtendedTable = unsafe { std::mem::transmute(tcp_proc) };
        // SAFETY: as above.
        let get_udp: GetExtendedTable = unsafe { std::mem::transmute(udp_proc) };
        let v4_tcp = query_mib_table(|ptr, len| {
            // SAFETY: `query_mib_table` passes null for sizing or a writable buffer of `*len` bytes.
            unsafe {
                get_tcp(
                    ptr.unwrap_or(std::ptr::null_mut()),
                    len,
                    BOOL(0),
                    u32::from(AF_INET.0),
                    TCP_TABLE_BASIC_ALL.0,
                    0,
                )
            }
        })
        .is_some_and(|buf| {
            table_bytes_hold_port::<MIB_TCPTABLE, MIB_TCPROW_LH>(
                &buf,
                std::mem::offset_of!(MIB_TCPTABLE, table),
                |row| row.dwLocalPort,
                port,
            )
        });
        if v4_tcp {
            return true;
        }
        let v6_tcp = query_mib_table(|ptr, len| {
            // SAFETY: `query_mib_table` passes null for sizing or a writable buffer of `*len` bytes.
            unsafe {
                get_tcp(
                    ptr.unwrap_or(std::ptr::null_mut()),
                    len,
                    BOOL(0),
                    u32::from(AF_INET6.0),
                    TCP_TABLE_OWNER_PID_ALL.0,
                    0,
                )
            }
        })
        .is_some_and(|buf| {
            table_bytes_hold_port::<MIB_TCP6TABLE_OWNER_PID, MIB_TCP6ROW_OWNER_PID>(
                &buf,
                std::mem::offset_of!(MIB_TCP6TABLE_OWNER_PID, table),
                |row| row.dwLocalPort,
                port,
            )
        });
        if v6_tcp {
            return true;
        }
        let v4_udp = query_mib_table(|ptr, len| {
            // SAFETY: `query_mib_table` passes null for sizing or a writable buffer of `*len` bytes.
            unsafe {
                get_udp(
                    ptr.unwrap_or(std::ptr::null_mut()),
                    len,
                    BOOL(0),
                    u32::from(AF_INET.0),
                    UDP_TABLE_BASIC.0,
                    0,
                )
            }
        })
        .is_some_and(|buf| {
            table_bytes_hold_port::<MIB_UDPTABLE, MIB_UDPROW>(
                &buf,
                std::mem::offset_of!(MIB_UDPTABLE, table),
                |row| row.dwLocalPort,
                port,
            )
        });
        if v4_udp {
            return true;
        }
        query_mib_table(|ptr, len| {
            // SAFETY: `query_mib_table` passes null for sizing or a writable buffer of `*len` bytes.
            unsafe {
                get_udp(
                    ptr.unwrap_or(std::ptr::null_mut()),
                    len,
                    BOOL(0),
                    u32::from(AF_INET6.0),
                    UDP_TABLE_OWNER_PID.0,
                    0,
                )
            }
        })
        .is_some_and(|buf| {
            table_bytes_hold_port::<MIB_UDP6TABLE_OWNER_PID, MIB_UDP6ROW_OWNER_PID>(
                &buf,
                std::mem::offset_of!(MIB_UDP6TABLE_OWNER_PID, table),
                |row| row.dwLocalPort,
                port,
            )
        })
    })();
    // SAFETY: `library` was returned by `LoadLibraryExW` above and is no longer referenced.
    let _ = unsafe { FreeLibrary(library) };
    held
}

/// Whether no program holds `port`, on TCP or UDP. Queries Windows's TCP/UDP
/// tables across all local addresses first, then probes loopback only (avoiding
/// wildcard `0.0.0.0`/`[::]` listens that trigger Windows Firewall prompts) to
/// also catch Windows excluded port ranges and concurrent binds.
pub(super) fn port_free(port: u16) -> bool {
    use std::io::ErrorKind::{AddrInUse, PermissionDenied};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, TcpListener, UdpSocket};
    if table_holds_port(port) {
        return false;
    }
    let v4 = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let free = TcpListener::bind((v4, port)).is_ok() && UdpSocket::bind((v4, port)).is_ok();
    // A PC without IPv6 has nothing there to collide with.
    let v6 = IpAddr::V6(Ipv6Addr::LOCALHOST);
    let held = |error: std::io::Error| matches!(error.kind(), AddrInUse | PermissionDenied);
    free && !TcpListener::bind((v6, port)).err().is_some_and(held)
        && !UdpSocket::bind((v6, port)).err().is_some_and(held)
}
/// The first base port whose whole range is free.
pub(super) fn pick_base_port(free: impl Fn(u16) -> bool) -> Result<u16> {
    SIDE_BY_SIDE_PORTS
        .into_iter()
        .find(|&base| ports_of(base).all(&free))
        .with_context(|| {
            format!(
                "no free ports for Rubylight beside another host: every range from {} to {} is in use. Close what uses them, then run setup again. Nothing has been changed.",
                ports_of(SIDE_BY_SIDE_PORTS[0]).start(),
                ports_of(SIDE_BY_SIDE_PORTS[5]).end(),
            )
        })
}
/// The new profile's settings: its ports, and a name of its own for clients,
/// which would otherwise show two hosts with the PC's name.
pub(super) fn side_by_side_conf(base: u16, pc: Option<&str>) -> String {
    let name = match pc.map(str::trim).filter(|pc| !pc.is_empty()) {
        Some(pc) => format!("{pc} (Rubylight)"),
        None => "Rubylight".to_owned(),
    };
    format!(
        "port = {base}\nwebrtc_port = {}\nwebrtc_media_port = {}\nsunshine_name = {name}\n",
        base + WEBRTC_OFFSET,
        base + WEBRTC_OFFSET + 1
    )
}
/// Marks a profile that side by side made, so its identity was never imported
/// from another host: it is kept when Rubylight is installed side by side again
/// after an uninstall that kept its settings.
pub(crate) const SIDE_BY_SIDE_MARK: &str = "side-by-side";
/// Readies the profile of a side-by-side installation, which never takes
/// settings or an identity from another host. An installed Rubylight's profile
/// and one an earlier side-by-side installation left stay; a profile from
/// before moves aside unchanged. Otherwise the profile is new, with its own
/// base port. Returns whether this run made it; `moved` is where the earlier
/// profile went.
pub(super) fn prepare_side_by_side_profile(
    profile: &Path,
    installed: bool,
    pc: Option<&str>,
    free: impl Fn(u16) -> bool,
    moved: &mut Option<PathBuf>,
) -> Result<bool> {
    let empty = profile_empty(profile)?;
    if !empty && (installed || profile.join(SIDE_BY_SIDE_MARK).is_file()) {
        if !installed {
            // Its port was free when it was chosen; nothing has held it since.
            let base = web_port(profile) - 1;
            if !ports_of(base).all(&free) {
                bail!(
                    "Rubylight's kept settings in {} use base port {base}, and a port of its range is in use now. Close what uses it, or set another port in sunshine.conf there, then run setup again. Nothing has been changed.",
                    profile.display()
                );
            }
        }
        return Ok(false);
    }
    let base = pick_base_port(free)?;
    if !empty {
        *moved = Some(move_profile_aside(profile)?);
    }
    let write = (|| -> Result<()> {
        std::fs::create_dir_all(profile)?;
        std::fs::write(profile.join("sunshine.conf"), side_by_side_conf(base, pc))?;
        std::fs::write(profile.join(SIDE_BY_SIDE_MARK), b"")?;
        Ok(())
    })();
    if let Err(error) = write {
        let _ = std::fs::remove_dir_all(profile);
        if let Some(aside) = moved.as_ref()
            && std::fs::rename(aside, profile).is_ok()
        {
            *moved = None;
        }
        return Err(error);
    }
    Ok(true)
}
pub(super) fn earlier_settings_moved(aside: &Path) -> String {
    format!(
        "Side by side starts with new settings, so setup moved the earlier Rubylight settings to {}.",
        aside.display()
    )
}
/// `profile` under the next free name beside it, unchanged.
pub(super) fn move_profile_aside(profile: &Path) -> Result<PathBuf> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let name = profile.file_name().context("profile has no name")?;
    let aside = (0..100)
        .map(|n| profile.with_file_name(format!("{}.previous-{seconds}-{n}", name.display())))
        .find(|aside| std::fs::symlink_metadata(aside).is_err())
        .context("no free name to move the earlier settings to")?;
    std::fs::rename(profile, &aside)
        .with_context(|| format!("moving {} to {}", profile.display(), aside.display()))?;
    Ok(aside)
}
/// What the finished window adds for a side-by-side installation: clients
/// must be given the port, and may otherwise reach the other host.
pub fn side_by_side_result(web_port: u16, address: &str, hosts: &[String]) -> String {
    let beside = if hosts.is_empty() {
        String::new()
    } else {
        format!(" beside {}", detect::in_words(hosts))
    };
    format!(
        "Rubylight runs{beside} as the {} service, with its own settings. In Moonlight or Nightjar, add this PC as {address}:{}: without the port, a client reaches whatever host uses the default one.",
        host_service::SIDE_BY_SIDE,
        web_port - 1
    )
}
/// This PC's address on its default route, else its name. Connecting a UDP
/// socket sends nothing; it only picks the route.
pub fn pc_address() -> String {
    let routed = std::net::UdpSocket::bind(("0.0.0.0", 0))
        .and_then(|socket| {
            socket.connect(("192.0.2.1", 9))?;
            socket.local_addr()
        })
        .ok()
        .map(|address| address.ip())
        .filter(|ip| !ip.is_unspecified() && !ip.is_loopback());
    match routed {
        Some(ip) => ip.to_string(),
        None => pc_name().unwrap_or_else(|| "this PC's address".to_owned()),
    }
}
pub(super) fn pc_name() -> Option<String> {
    std::env::var("COMPUTERNAME")
        .ok()
        .filter(|name| !name.trim().is_empty())
}
/// The text of a profile's sunshine.conf, empty if it cannot be read. The
/// host also reads one saved with a byte order mark or as UTF-16.
pub(super) fn conf_text(profile: &Path) -> String {
    let bytes = std::fs::read(profile.join("sunshine.conf")).unwrap_or_default();
    let utf16 = |bytes: &[u8], unit: fn([u8; 2]) -> u16| {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| unit(pair))
            .collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(rest) = bytes.strip_prefix(b"\xff\xfe") {
        return utf16(rest, u16::from_le_bytes);
    }
    if let Some(rest) = bytes.strip_prefix(b"\xfe\xff") {
        return utf16(rest, u16::from_be_bytes);
    }
    let text = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
    String::from_utf8_lossy(text).into_owned()
}
/// The configured base port of a profile (47989 unless set).
pub fn web_port(profile: &Path) -> u16 {
    let base = conf_text(profile)
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == "port")
                .then(|| {
                    value
                        .split('#')
                        .next()?
                        .trim()
                        .trim_matches('"')
                        .parse::<u16>()
                        .ok()
                })
                .flatten()
        })
        .filter(|port| (1029..=65514).contains(port))
        .unwrap_or(47989);
    base + 1
}
/// Where the host answers serverinfo: its bind_address, or this PC when
/// that is blank, all interfaces or not an address (the host then listens
/// on all of them or only on this PC). The first value that is not empty
/// counts, as the host reads it.
pub(super) fn probe_address(conf: &str) -> IpAddr {
    conf.lines()
        .find_map(|line| {
            let (key, value) = line.split('#').next()?.split_once('=')?;
            let value = value.trim().trim_matches('"').trim();
            (key.trim() == "bind_address" && !value.is_empty()).then_some(value)
        })
        .and_then(|value| value.parse::<IpAddr>().ok())
        .filter(|address| !address.is_unspecified())
        .unwrap_or(IpAddr::from([127, 0, 0, 1]))
}
/// The host's serverinfo address for a profile.
pub(crate) fn probe(profile: &Path) -> SocketAddr {
    SocketAddr::new(probe_address(&conf_text(profile)), web_port(profile) - 1)
}

/// Whether the host at `profile`'s address reports a stream, a queued
/// launch or a running app.
pub(crate) fn streaming(profile: &Path) -> bool {
    serverinfo(probe(profile)).is_ok_and(|response| check_idle(&response).is_err())
}
/// Refuse to stop a host that reports a stream, a queued launch or a running
/// app, unless the user chose to end them (`end_streams`). A host that cannot
/// say is stopped as before: none is running (Windows refuses a closed
/// loopback port only after about two seconds), it is Vibepollo, Apollo or
/// Sunshine, or it was asked on a LAN bind_address, where a Rust host leaves
/// its counts blank.
pub(crate) fn ensure_idle(address: SocketAddr, end_streams: bool) -> Result<()> {
    match serverinfo(address) {
        Ok(response) => match check_idle(&response) {
            Err(_) if end_streams => {
                line("the host is streaming; ending the stream as asked");
                Ok(())
            }
            result => result,
        },
        Err(error) => {
            line(format!("no host status at {address} ({error}); continuing"));
            Ok(())
        }
    }
}
pub(super) fn serverinfo(address: SocketAddr) -> std::io::Result<String> {
    use std::io::{Read, Write};
    let mut socket = std::net::TcpStream::connect_timeout(&address, Duration::from_millis(500))?;
    socket.set_read_timeout(Some(Duration::from_secs(3)))?;
    socket.set_write_timeout(Some(Duration::from_secs(3)))?;
    write!(
        socket,
        "GET /serverinfo HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    )?;
    // A reset or timeout after the answer arrived still leaves the counts.
    let mut response = Vec::new();
    let read = socket.take(65536).read_to_end(&mut response);
    if response.is_empty() {
        read?;
    }
    Ok(String::from_utf8_lossy(&response).into_owned())
}

pub(super) const IDLE_FIELDS: [&str; 3] = [
    "RustHostSessionCount",
    "RustHostPendingSessionCount",
    "RustHostApplicationActive",
];
pub(super) fn host_count(response: &str, field: &str) -> Option<u32> {
    response
        .split_once(&format!("<{field}>"))
        .and_then(|(_, value)| value.split_once(&format!("</{field}>")))
        .and_then(|(value, _)| value.trim().parse::<u32>().ok())
}
pub(super) fn check_idle(response: &str) -> Result<()> {
    for field in IDLE_FIELDS {
        if host_count(response, field).is_some_and(|count| count != 0) {
            bail!(
                "Disconnect all streams and close games started from Moonlight before installing, then run setup again. Nothing has been changed."
            );
        }
    }
    Ok(())
}
/// The repair's idle check: proceeds only on a positive idle answer, every
/// count present and 0. A host that does not answer, or answers with blank
/// counts (it blanks them for a caller that is not on loopback), may be
/// streaming and counts as busy. Only a host process that is not running at
/// all leaves nothing to interrupt. `host_running` says whether one is.
/// `blank_ok` is for a person who asked (`--now`): a host on a LAN
/// bind_address blanks its counts for every caller, this PC included, and
/// would otherwise never be repairable. Counts that are there still count.
pub(super) fn ensure_idle_positive(
    answer: &std::io::Result<String>,
    host_running: impl FnOnce() -> bool,
    blank_ok: bool,
) -> Result<()> {
    match answer {
        Ok(response) => {
            check_idle(response)?;
            if !blank_ok
                && IDLE_FIELDS
                    .iter()
                    .any(|field| host_count(response, field).is_none())
            {
                bail!("the host does not report whether it is streaming");
            }
            Ok(())
        }
        Err(error) if host_running() => {
            bail!("the host is running but does not answer ({error})")
        }
        Err(_) => Ok(()),
    }
}
/// Wait until the host answers serverinfo at `address` as a Rust host.
pub(crate) fn wait_ready(address: SocketAddr, version: Option<&str>) -> Result<()> {
    use std::io::{Read, Write};
    let host = match address {
        SocketAddr::V4(address) => address.ip().to_string(),
        SocketAddr::V6(address) => format!("[{}]", address.ip()),
    };
    let request = format!("GET /serverinfo HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    let deadline = Instant::now() + Duration::from_secs(90);
    while Instant::now() < deadline {
        if let Ok(mut socket) =
            std::net::TcpStream::connect_timeout(&address, Duration::from_millis(500))
        {
            let _ = socket.set_read_timeout(Some(Duration::from_secs(20)));
            let _ = socket.write_all(request.as_bytes());
            let mut response = String::new();
            let _ = socket.read_to_string(&mut response);
            if version.map_or_else(
                || response.contains("<RustHostVersion>"),
                |version| {
                    response.contains(&format!("<RustHostVersion>{version}</RustHostVersion>"))
                },
            ) {
                line("the host is answering");
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    bail!("Rubylight did not start within 90 seconds; see logs\\service.log in the profile")
}
