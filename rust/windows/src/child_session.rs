//! Windows side of background streaming in a child session: the edition, the
//! child-session calls, the registry, firewall and DPAPI behind the seams of
//! [`butterpollo_core::child_session`], the GPU the session renders on, the
//! lock setup and restore take across processes ([`lock`]), and the RDP
//! viewer that keeps the child session attached ([`viewer`]).
//!
//! Ported from Annex (mattxslv/annex, MIT, Copyright (c) 2026 mattxslv; the
//! license text is in `third-party/licenses/annex-LICENSE.txt`): the Windows
//! settings of `src/DesktopSetup.cs` and `scripts/setup-admin.ps1`, the
//! child-session calls of `src/Annex.cs` and the RDP client hosting of
//! `src/LauncherApplication.cs`.

pub mod lock;
pub mod viewer;

use crate::text::{from_wide, to_wide};
use anyhow::{Context, Result, bail};
use butterpollo_core::child_session::{
    ActiveFirewall, Adapter, Hive, Listener, RDP_SERVICE, RDP_TCP, RdpListeners, RdpServer,
    Registry, ServiceListener, SystemSettings, TERMINAL_SERVER,
};
use std::os::windows::io::FromRawHandle;
use windows::{
    Win32::{
        Foundation::{
            ERROR_FILE_NOT_FOUND, ERROR_INSUFFICIENT_BUFFER, ERROR_MORE_DATA, ERROR_PATH_NOT_FOUND,
            ERROR_SUCCESS, GetLastError, HANDLE, HLOCAL, LUID, LocalFree, NO_ERROR, SetLastError,
            VARIANT_TRUE, WIN32_ERROR,
        },
        Graphics::{
            Direct3D::D3D_FEATURE_LEVEL_11_0,
            Direct3D12::{
                D3D12_FEATURE_ARCHITECTURE, D3D12_FEATURE_DATA_ARCHITECTURE, D3D12CreateDevice,
                ID3D12Device,
            },
            Dxgi::{CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIAdapter1, IDXGIFactory1},
        },
        NetworkManagement::{
            IpHelper::{
                GetExtendedTcpTable, GetExtendedUdpTable, GetOwnerModuleFromTcp6Entry,
                GetOwnerModuleFromTcpEntry, MIB_TCP6ROW_OWNER_MODULE, MIB_TCP6ROW_OWNER_PID,
                MIB_TCP6TABLE_OWNER_MODULE, MIB_TCP6TABLE_OWNER_PID, MIB_TCPROW_OWNER_MODULE,
                MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_MODULE, MIB_TCPTABLE_OWNER_PID,
                MIB_UDP6ROW_OWNER_PID, MIB_UDP6TABLE_OWNER_PID, MIB_UDPROW_OWNER_PID,
                MIB_UDPTABLE_OWNER_PID, TCP_TABLE_CLASS, TCP_TABLE_OWNER_MODULE_LISTENER,
                TCP_TABLE_OWNER_PID_LISTENER, TCPIP_OWNER_MODULE_BASIC_INFO,
                TCPIP_OWNER_MODULE_INFO_BASIC, UDP_TABLE_OWNER_PID,
            },
            WindowsFirewall::*,
        },
        Networking::WinSock::{ADDRESS_FAMILY, AF_INET, AF_INET6},
        Security::{
            AdjustTokenPrivileges,
            Cryptography::{
                CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
            },
            LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED,
            TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
        },
        System::{
            Com::*,
            Registry::{
                HKEY, HKEY_LOCAL_MACHINE, HKEY_USERS, KEY_READ, REG_DWORD, RRF_RT_REG_DWORD,
                RRF_RT_REG_SZ, RegCloseKey, RegDeleteKeyValueW, RegGetValueW, RegLoadKeyW,
                RegOpenKeyExW, RegSetKeyValueW, RegUnLoadKeyW,
            },
            RemoteDesktop::{
                WTSEnableChildSessions, WTSGetChildSessionId, WTSIsChildSessionsEnabled,
                WTSLogoffSession,
            },
            Services::{
                CloseServiceHandle, ENUM_SERVICE_STATUS_PROCESSW, EnumServicesStatusExW,
                OpenSCManagerW, OpenServiceW, QueryServiceStatusEx, SC_ENUM_PROCESS_INFO,
                SC_HANDLE, SC_MANAGER_CONNECT, SC_MANAGER_ENUMERATE_SERVICE,
                SC_STATUS_PROCESS_INFO, SERVICE_ACTIVE, SERVICE_QUERY_STATUS, SERVICE_RUNNING,
                SERVICE_STATUS_PROCESS, SERVICE_WIN32,
            },
            Threading::{GetCurrentProcess, OpenProcessToken},
        },
    },
    core::{BOOL, BSTR, PCWSTR, w},
};
use zeroize::Zeroizing;

/// `EditionID` under `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion`,
/// e.g. `Core` (Home) or `Professional`; empty when it can't be read.
pub fn edition_id() -> String {
    let key = to_wide(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
    let name = to_wide("EditionID");
    let mut buffer = [0u16; 128];
    let mut size = size_of_val(&buffer) as u32;
    // SAFETY: `key` and `name` are NUL-terminated and outlive the call, and `buffer` is writable
    // for the `size` bytes passed; RegGetValueW NUL-terminates REG_SZ data it returns.
    let read = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if read.is_ok() {
        from_wide(&buffer)
    } else {
        String::new()
    }
}

/// Whether Windows child sessions are on (`WTSIsChildSessionsEnabled`).
pub fn child_sessions_enabled() -> Result<bool> {
    let mut enabled = BOOL(0);
    // SAFETY: `enabled` is a live local the call writes once.
    unsafe { WTSIsChildSessionsEnabled(&mut enabled) }
        .ok()
        .context("Windows could not report whether child sessions are on")?;
    Ok(enabled.as_bool())
}

/// The child session of this process's session, None when there is none.
pub fn child_session_id() -> Option<u32> {
    let mut session = 0;
    // SAFETY: `session` is a live local the call writes once.
    unsafe { WTSGetChildSessionId(&mut session) }
        .as_bool()
        .then_some(session)
        .filter(|session| *session != 0 && *session != u32::MAX)
}

/// Refuse, with the reason, unless `child` is a session of the account signed
/// in at this PC's screen and the viewer that reported it (process `viewer`)
/// runs at that screen too ([`butterpollo_core::child_session::check_child_owner`]).
/// Needs SYSTEM, as the host runs.
pub fn verify_child_owner(child: u32, viewer: u32) -> Result<()> {
    let console = crate::process::console_session();
    let viewer_session = crate::process::process_session(viewer)
        .with_context(|| format!("reading the session of the child-session viewer {viewer}"))?;
    let console_user = crate::process::session_user_sid(console)
        .context("reading the account signed in at this PC's screen")?;
    let child_user = crate::process::session_user_sid(child)
        .with_context(|| format!("reading the account of session {child}"))?;
    butterpollo_core::child_session::check_child_owner(
        child,
        console,
        viewer_session,
        &child_user,
        &console_user,
    )
}

/// The account signed in at this PC's screen, as a SID string. Needs SYSTEM.
pub fn console_user() -> Result<String> {
    crate::process::session_user_sid(crate::process::console_session())
        .context("reading the account signed in at this PC's screen")
}

/// Sign out a session, closing every app in it. Callers check the session
/// with [`butterpollo_core::child_session::end_confirmed_session`] first.
pub fn sign_out(session: u32) -> Result<()> {
    // SAFETY: no server handle means this machine; the call does not keep any argument.
    unsafe { WTSLogoffSession(None, session, false) }
        .with_context(|| format!("Windows did not sign out session {session}"))
}

/// What answers on the RDP port now, for
/// [`butterpollo_core::child_session::check_rdp_server`]: the Remote Desktop
/// service's process, every IPv4 and IPv6 TCP listener and UDP port, and the RDP registry
/// values. It only reads, so the viewer, an ordinary user, can.
pub fn rdp_server() -> Result<RdpServer> {
    // The service first: one that restarts in between no longer matches its listeners.
    let service_pid = service_pid(RDP_SERVICE)?;
    let mut listeners: Vec<Listener> = tcp_listeners::<MIB_TCPROW_OWNER_PID>(
        AF_INET,
        std::mem::offset_of!(MIB_TCPTABLE_OWNER_PID, table),
        TCP_TABLE_OWNER_PID_LISTENER,
    )?
    .into_iter()
    .map(|row| Listener {
        // Network byte order, as it lies in memory.
        address: std::net::Ipv4Addr::from(row.dwLocalAddr.to_ne_bytes()).into(),
        port: local_port(row.dwLocalPort),
        pid: row.dwOwningPid,
        udp: false,
    })
    .collect();
    listeners.extend(
        tcp_listeners::<MIB_TCP6ROW_OWNER_PID>(
            AF_INET6,
            std::mem::offset_of!(MIB_TCP6TABLE_OWNER_PID, table),
            TCP_TABLE_OWNER_PID_LISTENER,
        )?
        .into_iter()
        .map(|row| Listener {
            address: std::net::Ipv6Addr::from(row.ucLocalAddr).into(),
            port: local_port(row.dwLocalPort),
            pid: row.dwOwningPid,
            udp: false,
        }),
    );
    // RDP's UDP transport: a squatter there would take the session's traffic.
    listeners.extend(
        udp_listeners::<MIB_UDPROW_OWNER_PID>(
            AF_INET,
            std::mem::offset_of!(MIB_UDPTABLE_OWNER_PID, table),
        )?
        .into_iter()
        .map(|row| Listener {
            address: std::net::Ipv4Addr::from(row.dwLocalAddr.to_ne_bytes()).into(),
            port: local_port(row.dwLocalPort),
            pid: row.dwOwningPid,
            udp: true,
        }),
    );
    listeners.extend(
        udp_listeners::<MIB_UDP6ROW_OWNER_PID>(
            AF_INET6,
            std::mem::offset_of!(MIB_UDP6TABLE_OWNER_PID, table),
        )?
        .into_iter()
        .map(|row| Listener {
            address: std::net::Ipv6Addr::from(row.ucLocalAddr).into(),
            port: local_port(row.dwLocalPort),
            pid: row.dwOwningPid,
            udp: true,
        }),
    );
    Ok(RdpServer {
        service_pid,
        listeners,
        port_number: read_dword(&Hive::LocalMachine, RDP_TCP, "PortNumber")?,
        deny_ts_connections: read_dword(
            &Hive::LocalMachine,
            TERMINAL_SERVER,
            "fDenyTSConnections",
        )?,
    })
}
/// The Remote Desktop service's TCP listeners, for
/// [`SystemSettings::rdp_listeners`]; none when it is not running.
pub fn rdp_listeners() -> Result<RdpListeners> {
    match service_pid(RDP_SERVICE)? {
        0 => Ok(RdpListeners::default()),
        pid => service_listeners(pid),
    }
}
/// The TCP listeners of a service's process. When it hosts other services
/// too, which one owns each socket comes from `GetOwnerModuleFromTcpEntry`
/// (the service tag of the thread that opened it); that needs to query the
/// process, which the host (SYSTEM) can, and a socket it can't name fails.
fn service_listeners(pid: u32) -> Result<RdpListeners> {
    let services = services_in_process(pid)?;
    if services == 0 {
        bail!("the service process {pid} ended while its listeners were read");
    }
    if services == 1 {
        let own = |port_pid: (u32, u32)| {
            (port_pid.1 == pid).then(|| ServiceListener {
                port: local_port(port_pid.0),
                owner: None,
            })
        };
        let mut listeners: Vec<ServiceListener> = tcp_listeners::<MIB_TCPROW_OWNER_PID>(
            AF_INET,
            std::mem::offset_of!(MIB_TCPTABLE_OWNER_PID, table),
            TCP_TABLE_OWNER_PID_LISTENER,
        )?
        .into_iter()
        .filter_map(|row| own((row.dwLocalPort, row.dwOwningPid)))
        .collect();
        listeners.extend(
            tcp_listeners::<MIB_TCP6ROW_OWNER_PID>(
                AF_INET6,
                std::mem::offset_of!(MIB_TCP6TABLE_OWNER_PID, table),
                TCP_TABLE_OWNER_PID_LISTENER,
            )?
            .into_iter()
            .filter_map(|row| own((row.dwLocalPort, row.dwOwningPid))),
        );
        return Ok(RdpListeners {
            shared: false,
            listeners,
        });
    }
    let mut listeners = Vec::new();
    for row in tcp_listeners::<MIB_TCPROW_OWNER_MODULE>(
        AF_INET,
        std::mem::offset_of!(MIB_TCPTABLE_OWNER_MODULE, table),
        TCP_TABLE_OWNER_MODULE_LISTENER,
    )? {
        if row.dwOwningPid == pid {
            let port = local_port(row.dwLocalPort);
            // SAFETY: `row` is a live row of the table; the buffer and size come from owner_module.
            let owner = owner_module(port, pid, |buffer, size| unsafe {
                GetOwnerModuleFromTcpEntry(&row, TCPIP_OWNER_MODULE_INFO_BASIC, buffer, size)
            })?;
            listeners.push(ServiceListener {
                port,
                owner: Some(owner),
            });
        }
    }
    for row in tcp_listeners::<MIB_TCP6ROW_OWNER_MODULE>(
        AF_INET6,
        std::mem::offset_of!(MIB_TCP6TABLE_OWNER_MODULE, table),
        TCP_TABLE_OWNER_MODULE_LISTENER,
    )? {
        if row.dwOwningPid == pid {
            let port = local_port(row.dwLocalPort);
            // SAFETY: as above, for the IPv6 row.
            let owner = owner_module(port, pid, |buffer, size| unsafe {
                GetOwnerModuleFromTcp6Entry(&row, TCPIP_OWNER_MODULE_INFO_BASIC, buffer, size)
            })?;
            listeners.push(ServiceListener {
                port,
                owner: Some(owner),
            });
        }
    }
    Ok(RdpListeners {
        shared: true,
        listeners,
    })
}
/// The module name `GetOwnerModuleFrom*Entry` (through `read`, which gets a
/// buffer writable for the size it is passed) reports for a socket: a
/// service's name, or the program's (`svchost.exe` for an untagged socket).
fn owner_module(
    port: u16,
    pid: u32,
    read: impl Fn(*mut std::ffi::c_void, &mut u32) -> u32,
) -> Result<String> {
    let failed = || format!("reading which service of process {pid} listens on TCP port {port}");
    let mut size = 0u32;
    let mut result = read(std::ptr::null_mut(), &mut size);
    // u64s: aligned for the info struct's pointers.
    let mut buffer = Vec::<u64>::new();
    if result == ERROR_INSUFFICIENT_BUFFER.0 {
        buffer = vec![0; (size as usize).div_ceil(8)];
        result = read(buffer.as_mut_ptr().cast(), &mut size);
    }
    if result != NO_ERROR.0 {
        return Err(windows::core::Error::from(WIN32_ERROR(result).to_hresult()))
            .with_context(failed);
    }
    if buffer.len() * 8 < size_of::<TCPIP_OWNER_MODULE_BASIC_INFO>() {
        bail!("{}: Windows returned no module", failed());
    }
    // SAFETY: Windows wrote the info struct at the start of the buffer; its name points into
    // the buffer, which lives until the name is copied.
    let name = unsafe {
        buffer
            .as_ptr()
            .cast::<TCPIP_OWNER_MODULE_BASIC_INFO>()
            .read()
            .pModuleName
    };
    if name.is_null() {
        bail!("{}: Windows returned no module name", failed());
    }
    // SAFETY: as above; the name is NUL-terminated.
    unsafe { name.to_string() }.with_context(failed)
}
/// How many running services the process `pid` hosts (`EnumServicesStatusExW`).
fn services_in_process(pid: u32) -> Result<usize> {
    let manager = ServiceHandle(
        // SAFETY: null names select this PC's service manager.
        unsafe {
            OpenSCManagerW(
                PCWSTR::null(),
                PCWSTR::null(),
                SC_MANAGER_CONNECT | SC_MANAGER_ENUMERATE_SERVICE,
            )
        }
        .context("opening the service manager")?,
    );
    // u64s: aligned for the entries' pointers. A resume handle reads the list in parts.
    let mut buffer = vec![0u64; 8192];
    let mut resume = 0u32;
    let mut count = 0;
    for _ in 0..64 {
        let (mut needed, mut returned) = (0u32, 0u32);
        // SAFETY: the byte view covers the live buffer, which the call fills with entries followed
        // by the strings they point to.
        let listed = unsafe {
            EnumServicesStatusExW(
                manager.0,
                SC_ENUM_PROCESS_INFO,
                SERVICE_WIN32,
                SERVICE_ACTIVE,
                Some(std::slice::from_raw_parts_mut(
                    buffer.as_mut_ptr().cast::<u8>(),
                    buffer.len() * 8,
                )),
                &mut needed,
                &mut returned,
                Some(&mut resume),
                PCWSTR::null(),
            )
        };
        let more = match listed {
            Ok(()) => false,
            Err(error) if error.code() == ERROR_MORE_DATA.to_hresult() => true,
            Err(error) => return Err(error).context("listing the running services"),
        };
        let entries = buffer.as_ptr().cast::<ENUM_SERVICE_STATUS_PROCESSW>();
        if returned as usize * size_of::<ENUM_SERVICE_STATUS_PROCESSW>() > buffer.len() * 8 {
            bail!("Windows listed more services than its buffer holds");
        }
        // SAFETY: the `returned` entries lie at the start of the buffer, checked above.
        count += (0..returned as usize)
            .filter(|&i| unsafe { entries.add(i).read() }.ServiceStatusProcess.dwProcessId == pid)
            .count();
        if !more {
            return Ok(count);
        }
        if returned == 0 {
            bail!("a service entry did not fit in {} bytes", buffer.len() * 8);
        }
    }
    bail!("this PC's services kept changing while they were listed")
}
/// The port in a TCP table row: network byte order in the low 16 bits.
fn local_port(raw: u32) -> u16 {
    u16::from_be(raw as u16)
}
/// A service manager or service handle, closed on drop.
struct ServiceHandle(SC_HANDLE);
impl Drop for ServiceHandle {
    fn drop(&mut self) {
        // SAFETY: the handle came from OpenSCManagerW or OpenServiceW and is closed only here.
        let _ = unsafe { CloseServiceHandle(self.0) };
    }
}
/// A service's process, 0 when it is not running.
fn service_pid(name: &str) -> Result<u32> {
    let wide = to_wide(name);
    // SAFETY: null names select this PC's service manager; the service name is NUL-terminated
    // and outlives the call; the status is the struct whose size is passed.
    unsafe {
        let manager = ServiceHandle(
            OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT)
                .context("opening the service manager")?,
        );
        let service = ServiceHandle(
            OpenServiceW(manager.0, PCWSTR(wide.as_ptr()), SERVICE_QUERY_STATUS)
                .with_context(|| format!("opening the service {name}"))?,
        );
        let mut status = SERVICE_STATUS_PROCESS::default();
        let mut needed = 0;
        QueryServiceStatusEx(
            service.0,
            SC_STATUS_PROCESS_INFO,
            Some(std::slice::from_raw_parts_mut(
                (&mut status as *mut SERVICE_STATUS_PROCESS).cast(),
                size_of::<SERVICE_STATUS_PROCESS>(),
            )),
            &mut needed,
        )
        .with_context(|| format!("reading the state of the service {name}"))?;
        Ok(if status.dwCurrentState == SERVICE_RUNNING {
            status.dwProcessId
        } else {
            0
        })
    }
}
/// The rows of `GetExtendedTcpTable` for one address family and a listener
/// `class` whose rows are `Row`; `offset` is where the rows start in its table.
fn tcp_listeners<Row: Copy>(
    family: ADDRESS_FAMILY,
    offset: usize,
    class: TCP_TABLE_CLASS,
) -> Result<Vec<Row>> {
    // SAFETY: the buffer and size come from owner_table, writable for `size` bytes.
    owner_table("TCP", offset, |buffer, size| unsafe {
        GetExtendedTcpTable(Some(buffer), size, false, u32::from(family.0), class, 0)
    })
}
/// The rows of `GetExtendedUdpTable(UDP_TABLE_OWNER_PID)` for one address
/// family: every bound UDP port, which is all a UDP listener is.
fn udp_listeners<Row: Copy>(family: ADDRESS_FAMILY, offset: usize) -> Result<Vec<Row>> {
    // SAFETY: the buffer and size come from owner_table, writable for `size` bytes.
    owner_table("UDP", offset, |buffer, size| unsafe {
        GetExtendedUdpTable(
            Some(buffer),
            size,
            false,
            u32::from(family.0),
            UDP_TABLE_OWNER_PID,
            0,
        )
    })
}
/// The rows of an IP Helper table `read` fills: a u32 count, then rows from
/// `offset`. `read` gets a buffer writable for the size it is passed.
fn owner_table<Row: Copy>(
    protocol: &str,
    offset: usize,
    read: impl Fn(*mut std::ffi::c_void, &mut u32) -> u32,
) -> Result<Vec<Row>> {
    let mut size = 0u32;
    // The table can grow between the size query and the read.
    for _ in 0..8 {
        // u64s: aligned for the table's u32 fields.
        let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
        let result = read(buffer.as_mut_ptr().cast(), &mut size);
        if result == ERROR_INSUFFICIENT_BUFFER.0 {
            continue;
        }
        if result != NO_ERROR.0 {
            return Err(windows::core::Error::from(WIN32_ERROR(result).to_hresult()))
                .with_context(|| format!("reading this PC's {protocol} listeners"));
        }
        let bytes = (size as usize).min(buffer.len() * 8);
        let base = buffer.as_ptr().cast::<u8>();
        if bytes < 4 {
            bail!("Windows returned a {protocol} table without its length");
        }
        // SAFETY: the count and every row read lie inside the `bytes` Windows filled.
        unsafe {
            let count = base.cast::<u32>().read() as usize;
            if offset + count * size_of::<Row>() > bytes {
                bail!("Windows returned a {protocol} table shorter than its count");
            }
            let rows = base.add(offset).cast::<Row>();
            return Ok((0..count).map(|i| rows.add(i).read_unaligned()).collect());
        }
    }
    bail!("this PC's {protocol} listeners kept changing while they were read")
}

fn root(hive: &Hive, path: &str) -> (HKEY, Vec<u16>) {
    match hive {
        Hive::LocalMachine => (HKEY_LOCAL_MACHINE, to_wide(path)),
        Hive::User(sid) => (HKEY_USERS, to_wide(&format!(r"{sid}\{path}"))),
    }
}
fn absent(error: WIN32_ERROR) -> bool {
    error == ERROR_FILE_NOT_FOUND || error == ERROR_PATH_NOT_FOUND
}
fn hive_name(hive: &Hive) -> String {
    match hive {
        Hive::LocalMachine => "HKLM".into(),
        Hive::User(sid) => format!(r"HKU\{sid}"),
    }
}

/// The real registry, child-session switch and Windows Firewall. Setup and
/// restore run as an administrator (the SYSTEM host or the uninstaller).
pub struct NativeSettings {
    policy: INetFwPolicy2,
}
impl NativeSettings {
    pub fn new() -> Result<Self> {
        // SAFETY: COM is initialised on this thread before the in-process firewall object is made.
        unsafe {
            // Already initialised in another mode is fine for this in-process object.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            Ok(Self {
                policy: CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER)
                    .context("opening Windows Firewall")?,
            })
        }
    }
}

/// Lists and switches group rules by ID; the COM firewall API names rules
/// only by their display name, which several rules can share.
const RULES_SCRIPT: &str = include_str!("child_session_rules.ps1");
const RULES_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
fn run_rules_script(mode: &str, environment: &[(&str, &str)]) -> Result<String> {
    use std::{os::windows::process::CommandExt, process::Command};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let mut command = Command::new(
        std::path::Path::new(&root).join("System32\\WindowsPowerShell\\v1.0\\powershell.exe"),
    );
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &format!("& ([scriptblock]::Create([Console]::In.ReadToEnd())) {mode}"),
        ])
        // A PowerShell 7 parent's module path breaks Windows PowerShell's modules.
        .env_remove("PSModulePath")
        .env_remove("BUTTERPOLLO_FIREWALL_RULE")
        .env_remove("BUTTERPOLLO_FIREWALL_ENABLED")
        .env_remove("BUTTERPOLLO_FIREWALL_BLOCK")
        .envs(environment.iter().copied())
        .creation_flags(CREATE_NO_WINDOW);
    let (status, stdout, stderr) =
        crate::wake::output_within(&mut command, RULES_SCRIPT.as_bytes(), RULES_TIMEOUT)
            .with_context(|| format!("the firewall rule script ({mode})"))?;
    if !status.success() {
        bail!(
            "the firewall rule script ({mode}) failed with {status}: {}",
            stderr.trim()
        );
    }
    Ok(stdout)
}
/// The JSON array of rule IDs the script's `list` mode prints.
fn parse_rule_ids(output: &str) -> Result<Vec<String>> {
    let mut ids: Vec<String> =
        serde_json::from_str(output.trim().trim_start_matches('\u{feff}'))
            .with_context(|| format!("the firewall rule list is not a JSON array: {output:?}"))?;
    ids.sort();
    ids.dedup();
    Ok(ids)
}
const PROTOCOLS: [(NET_FW_IP_PROTOCOL, &str); 2] = [
    (NET_FW_IP_PROTOCOL_TCP, "TCP"),
    (NET_FW_IP_PROTOCOL_UDP, "UDP"),
];
fn read_dword(hive: &Hive, path: &str, name: &str) -> Result<Option<u32>> {
    let (root, key) = root(hive, path);
    let value = to_wide(name);
    let mut data = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: `key` and `value` are NUL-terminated and outlive the call, and `data` is the
    // 4-byte buffer `size` names.
    let read = unsafe {
        RegGetValueW(
            root,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut data as *mut u32).cast()),
            Some(&mut size),
        )
    };
    match read {
        error if error.is_ok() => Ok(Some(data)),
        error if absent(error) => Ok(None),
        error => Err(windows::core::Error::from(error.to_hresult()))
            .with_context(|| format!(r"reading {}\{path}\{name}", hive_name(hive))),
    }
}
fn write_dword(hive: &Hive, path: &str, name: &str, value: u32) -> Result<()> {
    let (root, key) = root(hive, path);
    let wide = to_wide(name);
    // SAFETY: `key` and `wide` are NUL-terminated and outlive the call, and `value` is the
    // 4-byte REG_DWORD the size names.
    unsafe {
        RegSetKeyValueW(
            root,
            PCWSTR(key.as_ptr()),
            PCWSTR(wide.as_ptr()),
            REG_DWORD.0,
            Some((&value as *const u32).cast()),
            size_of::<u32>() as u32,
        )
    }
    .ok()
    .with_context(|| format!(r"writing {}\{path}\{name}", hive_name(hive)))
}
/// The registry alone, without Windows Firewall: what turns RDP off when
/// [`NativeSettings::new`] fails.
pub struct NativeRegistry;
impl Registry for NativeRegistry {
    fn read_dword(&self, hive: &Hive, path: &str, name: &str) -> Result<Option<u32>> {
        read_dword(hive, path, name)
    }
    fn write_dword(&mut self, hive: &Hive, path: &str, name: &str, value: u32) -> Result<()> {
        write_dword(hive, path, name, value)
    }
}
impl Registry for NativeSettings {
    fn read_dword(&self, hive: &Hive, path: &str, name: &str) -> Result<Option<u32>> {
        read_dword(hive, path, name)
    }
    fn write_dword(&mut self, hive: &Hive, path: &str, name: &str, value: u32) -> Result<()> {
        write_dword(hive, path, name, value)
    }
}
impl SystemSettings for NativeSettings {
    fn delete_value(&mut self, hive: &Hive, path: &str, name: &str) -> Result<()> {
        let (root, key) = root(hive, path);
        let wide = to_wide(name);
        // SAFETY: `key` and `wide` are NUL-terminated and outlive the call.
        match unsafe { RegDeleteKeyValueW(root, PCWSTR(key.as_ptr()), PCWSTR(wide.as_ptr())) } {
            error if error.is_ok() || absent(error) => Ok(()),
            error => Err(windows::core::Error::from(error.to_hresult()))
                .with_context(|| format!(r"removing {}\{path}\{name}", hive_name(hive))),
        }
    }
    fn child_sessions_enabled(&self) -> Result<bool> {
        child_sessions_enabled()
    }
    fn set_child_sessions(&mut self, enabled: bool) -> Result<()> {
        // SAFETY: the call takes a plain flag.
        unsafe { WTSEnableChildSessions(enabled) }
            .ok()
            .context("WTSEnableChildSessions failed")
    }
    fn firewall_problem(&self) -> Result<Option<String>> {
        for (profile, label) in [
            (NET_FW_PROFILE2_DOMAIN, "Domain"),
            (NET_FW_PROFILE2_PRIVATE, "Private"),
            (NET_FW_PROFILE2_PUBLIC, "Public"),
        ] {
            // SAFETY: `policy` is a live firewall policy object.
            if !unsafe { self.policy.get_FirewallEnabled(profile) }?.as_bool() {
                return Ok(Some(format!(
                    "Windows Firewall is off for the {label} profile"
                )));
            }
        }
        // SAFETY: `policy` is a live firewall policy object.
        let state = unsafe { self.policy.LocalPolicyModifyState() }
            .context("asking Windows Firewall whether local rules take effect")?;
        Ok(modify_state_problem(state))
    }
    fn active_firewall(&self, group: &str, block_rule: &str) -> Result<ActiveFirewall> {
        let output = run_rules_script(
            "active",
            &[
                ("BUTTERPOLLO_FIREWALL_GROUP", group),
                ("BUTTERPOLLO_FIREWALL_BLOCK", block_rule),
            ],
        )?;
        parse_active(&output)
    }
    fn rdp_listeners(&self) -> Result<RdpListeners> {
        rdp_listeners()
    }
    fn enabled_rules(&self, group: &str) -> Result<Vec<String>> {
        parse_rule_ids(&run_rules_script(
            "list",
            &[("BUTTERPOLLO_FIREWALL_GROUP", group)],
        )?)
        .with_context(|| format!("listing the firewall rules of {group}"))
    }
    fn set_rule_enabled(&mut self, group: &str, id: &str, enabled: bool) -> Result<()> {
        run_rules_script(
            "set",
            &[
                ("BUTTERPOLLO_FIREWALL_GROUP", group),
                ("BUTTERPOLLO_FIREWALL_RULE", id),
                (
                    "BUTTERPOLLO_FIREWALL_ENABLED",
                    if enabled { "1" } else { "0" },
                ),
            ],
        )
        .with_context(|| format!("switching the firewall rule {id}"))?;
        Ok(())
    }
    fn add_block_rule(&mut self, name: &str, port: u16) -> Result<()> {
        // The new rules go in beside the old ones, which stay until then: the
        // port is never left without a block rule.
        let staged = staging_name(name);
        self.remove_named(&staged)?;
        self.add_block_rules(&staged, port)?;
        self.remove_named(name)?;
        for (_, label) in PROTOCOLS {
            let (from, to) = (format!("{staged} ({label})"), format!("{name} ({label})"));
            // SAFETY: `policy` is a live firewall policy object and each BSTR outlives its call.
            unsafe {
                self.policy
                    .Rules()?
                    .Item(&BSTR::from(from.as_str()))?
                    .SetName(&BSTR::from(to.as_str()))
            }
            .with_context(|| format!("renaming the firewall rule {from} to {to}"))?;
        }
        Ok(())
    }
    fn remove_rule(&mut self, name: &str) -> Result<()> {
        self.remove_named(&staging_name(name))?;
        self.remove_named(name)
    }
    fn load_user_hive(&mut self, sid: &str) -> Result<bool> {
        load_user_hive(sid)
    }
    fn unload_user_hive(&mut self, sid: &str) -> Result<()> {
        with_privileges(&[w!("SeRestorePrivilege"), w!("SeBackupPrivilege")], || {
            let key = to_wide(sid);
            // SAFETY: `key` is NUL-terminated and outlives the call.
            unsafe { RegUnLoadKeyW(HKEY_USERS, PCWSTR(key.as_ptr())) }
                .ok()
                .with_context(|| format!(r"unloading HKU\{sid}"))
        })
    }
}

/// Where [`SystemSettings::add_block_rule`] puts the new rules before the old
/// ones go.
fn staging_name(name: &str) -> String {
    format!("{name} [new]")
}

/// Why local firewall rules don't take effect now, None when they do.
fn modify_state_problem(state: NET_FW_MODIFY_STATE) -> Option<String> {
    match state {
        NET_FW_MODIFY_STATE_OK => None,
        NET_FW_MODIFY_STATE_GP_OVERRIDE => Some(
            "group policy overrides the local Windows Firewall rules for the current network profile"
                .into(),
        ),
        NET_FW_MODIFY_STATE_INBOUND_BLOCKED => Some(
            "Windows Firewall ignores local inbound rules for the current network profile".into(),
        ),
        other => Some(format!(
            "Windows Firewall reports local rules in state {} it does not document",
            other.0
        )),
    }
}

/// The JSON object the script's `active` mode prints.
fn parse_active(output: &str) -> Result<ActiveFirewall> {
    serde_json::from_str(output.trim().trim_start_matches('\u{feff}'))
        .with_context(|| format!("the firewall rules in effect are not readable: {output:?}"))
}

/// Run `f` with these privileges of the process enabled, then put them back.
fn with_privileges<T>(names: &[PCWSTR], f: impl FnOnce() -> Result<T>) -> Result<T> {
    let mut token = HANDLE::default();
    // SAFETY: `token` is a live local the call writes once; it is closed below.
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )
    }
    .context("opening this process's token")?;
    // SAFETY: the token was just opened here, and only this guard closes it.
    let _close = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(token.0) };
    let mut previous = Vec::new();
    let mut enabled = || -> Result<()> {
        for name in names {
            let mut luid = LUID::default();
            // SAFETY: `name` is a static NUL-terminated string and `luid` is writable.
            unsafe { LookupPrivilegeValueW(None, *name, &mut luid) }?;
            let privileges = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [LUID_AND_ATTRIBUTES {
                    Luid: luid,
                    Attributes: SE_PRIVILEGE_ENABLED,
                }],
            };
            let mut before = TOKEN_PRIVILEGES::default();
            let mut size = 0;
            // SAFETY: the token is open for adjusting; `before` and `size` are writable and
            // `before` is as large as the length given.
            unsafe {
                SetLastError(ERROR_SUCCESS);
                AdjustTokenPrivileges(
                    token,
                    false,
                    Some(&privileges),
                    size_of::<TOKEN_PRIVILEGES>() as u32,
                    Some(&mut before),
                    Some(&mut size),
                )?;
                previous.push(before);
                // Success without the privilege reports ERROR_NOT_ALL_ASSIGNED.
                if GetLastError() != ERROR_SUCCESS {
                    bail!(
                        "this process does not hold {}; run it as an administrator",
                        name.display()
                    );
                }
            }
        }
        Ok(())
    };
    let result = enabled().and_then(|()| f());
    for before in previous.iter().rev() {
        // SAFETY: as above; `before` is what the matching call returned.
        let _ = unsafe { AdjustTokenPrivileges(token, false, Some(before), 0, None, None) };
    }
    result
}

/// Load a signed-out account's hive (`NTUSER.DAT` in its profile folder) as
/// `HKU\<sid>`; false when it is there already.
fn load_user_hive(sid: &str) -> Result<bool> {
    let key = to_wide(sid);
    let mut open = HKEY::default();
    // SAFETY: `key` is NUL-terminated and outlives the call; `open` is closed below.
    match unsafe { RegOpenKeyExW(HKEY_USERS, PCWSTR(key.as_ptr()), None, KEY_READ, &mut open) } {
        error if error.is_ok() => {
            // SAFETY: `open` is the key the call opened.
            let _ = unsafe { RegCloseKey(open) };
            return Ok(false);
        }
        error if absent(error) => {}
        error => {
            return Err(windows::core::Error::from(error.to_hresult()))
                .with_context(|| format!(r"opening HKU\{sid}"));
        }
    }
    let folder = profile_folder(sid)?;
    let file = std::path::Path::new(&folder).join("NTUSER.DAT");
    let wide = to_wide(&file.to_string_lossy());
    with_privileges(&[w!("SeRestorePrivilege"), w!("SeBackupPrivilege")], || {
        // SAFETY: both strings are NUL-terminated and outlive the call.
        unsafe { RegLoadKeyW(HKEY_USERS, PCWSTR(key.as_ptr()), PCWSTR(wide.as_ptr())) }
            .ok()
            .with_context(|| format!(r"loading {} as HKU\{sid}", file.display()))
    })?;
    Ok(true)
}

/// The account's profile folder, as `ProfileList` names it.
fn profile_folder(sid: &str) -> Result<String> {
    let key = to_wide(&format!(
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\{sid}"
    ));
    let name = to_wide("ProfileImagePath");
    let mut buffer = vec![0u16; 1024];
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: `key` and `name` are NUL-terminated and outlive the call, and `buffer` is
    // writable for the `size` bytes passed; RegGetValueW expands REG_EXPAND_SZ and
    // NUL-terminates what it returns.
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    }
    .ok()
    .with_context(|| format!("reading the profile folder of {sid}"))?;
    let folder = from_wide(&buffer);
    if folder.is_empty() {
        bail!("the account {sid} has no profile folder");
    }
    Ok(folder)
}

impl NativeSettings {
    fn add_block_rules(&mut self, name: &str, port: u16) -> Result<()> {
        for (protocol, label) in PROTOCOLS {
            // SAFETY: COM is initialised (see `new`); the rule is a fresh in-process object and
            // each BSTR outlives its call.
            unsafe {
                let rule: INetFwRule = CoCreateInstance(&NetFwRule, None, CLSCTX_INPROC_SERVER)?;
                rule.SetName(&BSTR::from(format!("{name} ({label})")))?;
                rule.SetDescription(&BSTR::from(
                    "Keeps Remote Desktop, which Rubylight's child session turns on, reachable only from this PC.",
                ))?;
                rule.SetGrouping(&BSTR::from("Rubylight"))?;
                rule.SetProtocol(protocol.0)?;
                rule.SetLocalPorts(&BSTR::from(port.to_string()))?;
                rule.SetDirection(NET_FW_RULE_DIR_IN)?;
                rule.SetAction(NET_FW_ACTION_BLOCK)?;
                rule.SetProfiles(NET_FW_PROFILE2_ALL.0)?;
                rule.SetEnabled(VARIANT_TRUE)?;
                self.policy
                    .Rules()?
                    .Add(&rule)
                    .with_context(|| format!("adding the firewall rule {name} ({label})"))?;
            }
        }
        Ok(())
    }
    fn remove_named(&mut self, name: &str) -> Result<()> {
        for (_, label) in PROTOCOLS {
            let full = format!("{name} ({label})");
            // Several rules can share a name; remove until none is left.
            for _ in 0..16 {
                // SAFETY: `policy` is a live firewall policy object and the BSTR outlives the call.
                unsafe {
                    let rules = self.policy.Rules()?;
                    let name = BSTR::from(full.as_str());
                    if rules.Item(&name).is_err() {
                        break;
                    }
                    rules
                        .Remove(&name)
                        .with_context(|| format!("removing the firewall rule {full}"))?;
                }
            }
        }
        Ok(())
    }
}

/// DPAPI with the current Windows user's key: only that user, signed in on
/// this PC, can decrypt the blob.
pub struct UserDpapi;
const ENTROPY: &[u8] = b"Rubylight child-session password";
fn blob(bytes: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr().cast_mut(),
    }
}
impl butterpollo_core::child_session::Dpapi for UserDpapi {
    fn protect(&self, plain: &[u8]) -> Result<Vec<u8>> {
        let (input, entropy) = (blob(plain), blob(ENTROPY));
        let mut output = CRYPT_INTEGER_BLOB::default();
        // SAFETY: the input blobs point at live slices the call only reads, and the output is
        // copied out before it is freed once with LocalFree.
        unsafe {
            CryptProtectData(
                &input,
                PCWSTR::null(),
                Some(&entropy),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
            .context("DPAPI could not encrypt the password")?;
            let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
            let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
            Ok(bytes)
        }
    }
    fn unprotect(&self, data: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        let (input, entropy) = (blob(data), blob(ENTROPY));
        let mut output = CRYPT_INTEGER_BLOB::default();
        // SAFETY: as in `protect`; the plaintext is copied into a zeroizing buffer and wiped in
        // place before LocalFree.
        unsafe {
            CryptUnprotectData(
                &input,
                None,
                Some(&entropy),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
            .context("DPAPI could not decrypt the password")?;
            let plain = std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize);
            let bytes = Zeroizing::new(plain.to_vec());
            zeroize::Zeroize::zeroize(plain);
            let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
            Ok(bytes)
        }
    }
}

/// The GPUs DXGI reports, and the one whose output this session's desktop
/// is on: in a remote session the display hangs off the adapter that
/// renders it.
pub fn adapters() -> Result<(Vec<Adapter>, Option<Adapter>)> {
    // SAFETY: the factory and adapters are live COM objects returned by these calls, GetDesc1
    // returns an owned struct, and the D3D12 feature query writes the struct whose size it gets.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let (mut all, mut render) = (Vec::new(), None);
        for index in 0..32 {
            let Ok(adapter) = factory.EnumAdapters1(index) else {
                break;
            };
            let desc = adapter.GetDesc1()?;
            let found = Adapter {
                name: from_wide(&desc.Description),
                vendor: desc.VendorId,
                dedicated_memory: desc.DedicatedVideoMemory as u64,
                software: desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0,
                uma: uma(&adapter),
            };
            if render.is_none() && adapter.EnumOutputs(0).is_ok() {
                render = Some(found.clone());
            }
            all.push(found);
        }
        Ok((all, render))
    }
}
fn uma(adapter: &IDXGIAdapter1) -> Option<bool> {
    // SAFETY: `adapter` is live; the feature struct is the size passed and outlives the call.
    unsafe {
        let mut device: Option<ID3D12Device> = None;
        D3D12CreateDevice(adapter, D3D_FEATURE_LEVEL_11_0, &mut device).ok()?;
        let mut data = D3D12_FEATURE_DATA_ARCHITECTURE::default();
        device?
            .CheckFeatureSupport(
                D3D12_FEATURE_ARCHITECTURE,
                (&mut data as *mut D3D12_FEATURE_DATA_ARCHITECTURE).cast(),
                size_of::<D3D12_FEATURE_DATA_ARCHITECTURE>() as u32,
            )
            .ok()?;
        Some(data.UMA.as_bool())
    }
}

#[cfg(test)]
mod tests {
    use butterpollo_core::child_session::{Dpapi, PasswordStore, Registry, Secret, SystemSettings};

    #[test]
    fn the_edition_is_readable() {
        // Every Windows install names its edition; the mode decision depends on it.
        assert!(!super::edition_id().is_empty());
    }

    #[test]
    fn dpapi_roundtrips_for_this_user() {
        let blob = super::UserDpapi.protect(b"not a real password").unwrap();
        assert!(!blob.windows(19).any(|w| w == b"not a real password"));
        assert_eq!(
            super::UserDpapi.unprotect(&blob).unwrap().as_slice(),
            b"not a real password"
        );
        let folder = tempfile::tempdir().unwrap();
        let store = PasswordStore::new(folder.path().join("password.bin"), super::UserDpapi);
        store.save(&Secret::new("also not real".into())).unwrap();
        assert_eq!(store.load().unwrap().unwrap().expose(), "also not real");
        assert!(store.forget().unwrap());
    }

    #[test]
    fn reading_settings_changes_nothing() {
        // Read-only: this PC's registry, RDP and firewall settings stay as they are.
        let settings = super::NativeSettings::new().unwrap();
        let value = settings
            .read_dword(
                &butterpollo_core::child_session::Hive::LocalMachine,
                butterpollo_core::child_session::TERMINAL_SERVER,
                "fDenyTSConnections",
            )
            .unwrap();
        assert!(matches!(value, None | Some(0) | Some(1)));
        assert_eq!(
            settings
                .read_dword(
                    &butterpollo_core::child_session::Hive::LocalMachine,
                    r"SOFTWARE\Rubylight\No\Such\Key",
                    "Missing",
                )
                .unwrap(),
            None
        );
        settings.child_sessions_enabled().unwrap();
        let (adapters, _) = super::adapters().unwrap();
        assert!(!adapters.is_empty());
    }

    #[test]
    fn firewall_rules_are_named_by_id() {
        // Read-only: listing, and a switch that matches no rule.
        let mut settings = super::NativeSettings::new().unwrap();
        let ids = settings
            .enabled_rules(butterpollo_core::child_session::REMOTE_DESKTOP_RULES)
            .unwrap();
        // IDs, not display names such as "Remote Desktop - User Mode (TCP-In)".
        assert!(ids.iter().all(|id| !id.contains(' ')), "{ids:?}");
        let group = "Rubylight test group that does not exist";
        assert!(settings.enabled_rules(group).unwrap().is_empty());
        let error = format!(
            "{:#}",
            settings
                .set_rule_enabled(group, "RemoteDesktop-UserMode-In-TCP", true)
                .unwrap_err()
        );
        assert!(error.contains("is not in the group"), "{error}");
    }

    #[test]
    fn the_firewall_in_effect_is_readable() {
        // Read-only: the active store, and this account's hive, which is loaded.
        let mut settings = super::NativeSettings::new().unwrap();
        let active = settings
            .active_firewall(
                butterpollo_core::child_session::REMOTE_DESKTOP_RULES,
                "Rubylight test rule that does not exist",
            )
            .unwrap();
        assert!(!active.blocked);
        assert!(active.block_ports.is_empty(), "{active:?}");
        assert!(
            active.allowed.iter().all(|id| !id.contains(' ')),
            "{active:?}"
        );
        settings.firewall_problem().unwrap();
        let session = crate::process::current_session().unwrap();
        if let Ok(sid) = crate::process::session_user_sid(session) {
            assert!(!settings.load_user_hive(&sid).unwrap(), "{sid} is loaded");
        }
        assert!(super::profile_folder("S-1-5-21-0-0-0-1").is_err());
    }

    #[test]
    fn the_active_firewall_parses_and_local_rule_states_are_named() {
        use windows::Win32::NetworkManagement::WindowsFirewall::*;
        let active = super::parse_active(
            "\u{feff}{\"allowed\":[\"RemoteDesktop-UserMode-In-TCP\"],\"blocked\":true,\"block_ports\":[\"3390\",\"3390\"],\"unprotected\":[\"Domain\"]}\r\n",
        )
        .unwrap();
        assert_eq!(active.allowed, ["RemoteDesktop-UserMode-In-TCP"]);
        assert!(active.blocked);
        assert_eq!(active.block_ports, ["3390", "3390"]);
        assert_eq!(active.unprotected, ["Domain"]);
        assert!(super::parse_active("{\"blocked\":true}").is_err());
        // Without the ports, a block rule never reads as closing the RDP port.
        assert!(
            super::parse_active("{\"allowed\":[],\"blocked\":true,\"unprotected\":[]}").is_err()
        );
        assert!(super::modify_state_problem(NET_FW_MODIFY_STATE_OK).is_none());
        let problem = super::modify_state_problem(NET_FW_MODIFY_STATE_GP_OVERRIDE).unwrap();
        assert!(problem.contains("group policy"), "{problem}");
        assert!(super::modify_state_problem(NET_FW_MODIFY_STATE_INBOUND_BLOCKED).is_some());
        assert!(super::modify_state_problem(NET_FW_MODIFY_STATE(7)).is_some());
        assert_eq!(super::staging_name("x"), "x [new]");
    }

    #[test]
    fn the_rdp_server_is_readable_and_ports_come_off_the_wire_order() {
        // 3389 is 0x0D3D, stored as the bytes 0D 3D.
        assert_eq!(
            super::local_port(u32::from(u16::from_ne_bytes([0x0D, 0x3D]))),
            3389
        );
        // Read-only: the service state, the TCP tables and two registry values.
        let server = super::rdp_server().unwrap();
        assert!(!server.listeners.is_empty());
        assert!(server.listeners.iter().any(|l| l.address.is_ipv4()));
        // This test's own listener shows up, with this process as its owner.
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        let server = super::rdp_server().unwrap();
        assert!(
            server
                .listeners
                .iter()
                .any(|l| l.port == port && l.pid == std::process::id() && !l.udp),
            "{port}"
        );
        // A squatter on a port is refused, whatever runs on this PC.
        let error = butterpollo_core::child_session::check_rdp_server(port, &server).unwrap_err();
        assert!(
            format!("{error:#}").contains(&format!("process {}", std::process::id())),
            "{error:#}"
        );
        // A bound UDP port shows up too, so a UDP squatter is refused as well.
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        let server = super::rdp_server().unwrap();
        assert!(
            server
                .listeners
                .iter()
                .any(|l| l.port == port && l.pid == std::process::id() && l.udp),
            "{port}"
        );
        let error = butterpollo_core::child_session::check_rdp_server(port, &server).unwrap_err();
        assert!(
            format!("{error:#}").contains(&format!("process {}", std::process::id())),
            "{error:#}"
        );
    }

    #[test]
    fn a_shared_service_process_names_the_service_of_each_listener() {
        // Read-only: the Remote Desktop service's listeners, whether it runs or not.
        super::rdp_listeners().unwrap().ports().unwrap();
        // RPC (RpcSs, TCP 135) runs on every Windows, on this PC in one svchost with
        // RpcEptMapper: its listeners are named by service when the process is shared.
        let pid = super::service_pid("RpcSs").unwrap();
        assert_ne!(pid, 0);
        let services = super::services_in_process(pid).unwrap();
        assert!(services >= 1);
        let rpc = super::service_listeners(pid).unwrap();
        assert_eq!(rpc.shared, services > 1);
        let endpoint_mapper = rpc.listeners.iter().find(|l| l.port == 135);
        assert!(endpoint_mapper.is_some(), "{rpc:?}");
        if rpc.shared {
            assert!(
                rpc.listeners
                    .iter()
                    .all(|l| l.owner.as_deref().is_some_and(|o| !o.is_empty())),
                "{rpc:?}"
            );
            assert!(
                endpoint_mapper
                    .and_then(|l| l.owner.as_deref())
                    .is_some_and(|owner| owner.eq_ignore_ascii_case("RpcSs")),
                "{rpc:?}"
            );
        }
        // No service runs in this test's process.
        assert_eq!(super::services_in_process(std::process::id()).unwrap(), 0);
    }

    #[test]
    fn rule_lists_parse_with_or_without_a_bom() {
        assert_eq!(
            super::parse_rule_ids("\u{feff}[\"b\",\"a\",\"a\"]\r\n").unwrap(),
            ["a", "b"]
        );
        assert!(super::parse_rule_ids("[]").unwrap().is_empty());
        assert!(super::parse_rule_ids("\"one\"").is_err());
    }
}
