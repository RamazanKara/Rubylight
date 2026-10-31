//! Wake-on-LAN and sleep on Windows: adapter facts, NIC wake settings, power capabilities and suspend (`docs/wake-and-audio-design.md`).
use anyhow::{Context, Result};
use butterpollo_core::wake::{Adapter, ApplyReport, FixPlan, Medium, NicReport, SystemFacts};
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    sync::RwLock,
    time::{Duration, Instant},
};

/// Every adapter Windows knows, with the facts the MAC policy and the NIC
/// report need.
pub fn adapters() -> Result<Vec<Adapter>> {
    use windows::Win32::{
        Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS},
        NetworkManagement::{IpHelper::*, Ndis::IfOperStatusUp},
        Networking::WinSock::AF_UNSPEC,
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
        let mut adapters = Vec::new();
        // SAFETY: GetAdaptersAddresses filled `storage`, still alive, with linked records; each
        // sockaddr is converted only within its reported length; `interface` is a local.
        unsafe {
            let mut adapter = first;
            while let Some(row) = adapter.as_ref() {
                let mut interface = MIB_IF_ROW2 {
                    InterfaceLuid: row.Luid,
                    ..Default::default()
                };
                let entry = GetIfEntry2(&mut interface).0 == 0;
                let flags = if entry {
                    interface.InterfaceAndOperStatusFlags._bitfield
                } else {
                    0
                };
                let mut ipv4 = Vec::new();
                let mut addresses = Vec::new();
                let mut unicast = row.FirstUnicastAddress;
                while let Some(ip) = unicast.as_ref() {
                    if let Some(address) = socket_address(&ip.Address) {
                        if let IpAddr::V4(v4) = address {
                            ipv4.push((v4, ip.OnLinkPrefixLength));
                        }
                        addresses.push(address);
                    }
                    unicast = ip.Next;
                }
                let guid = interface.InterfaceGuid;
                adapters.push(Adapter {
                    index: row.Anonymous1.Anonymous.IfIndex,
                    guid: if entry {
                        format!("{{{guid:?}}}")
                    } else {
                        String::new()
                    },
                    name: row.FriendlyName.to_string().unwrap_or_default(),
                    mac: six(&row.PhysicalAddress, row.PhysicalAddressLength),
                    permanent: entry
                        .then(|| {
                            six(
                                &interface.PermanentPhysicalAddress,
                                interface.PhysicalAddressLength,
                            )
                        })
                        .flatten(),
                    hardware: flags & 1 != 0,
                    connector: flags & 4 != 0,
                    medium: medium(row.IfType, interface.PhysicalMediumType.0),
                    connected: row.OperStatus == IfOperStatusUp,
                    metric: row.Ipv4Metric,
                    ipv4,
                    addresses,
                });
                adapter = row.Next;
            }
        }
        return Ok(adapters);
    }
    anyhow::bail!("network interfaces changed during enumeration")
}

/// Ethernet (`IF_TYPE_ETHERNET_CSMACD`) unless NDIS names another medium,
/// as Bluetooth PAN does; Wi-Fi is `IF_TYPE_IEEE80211`.
fn medium(if_type: u32, physical: i32) -> Medium {
    const UNSPECIFIED: i32 = 0;
    const ETHERNET: i32 = 14;
    match (if_type, physical) {
        (71, _) => Medium::Wifi,
        (6, UNSPECIFIED | ETHERNET) => Medium::Wired,
        _ => Medium::Other,
    }
}

fn six(bytes: &[u8], length: u32) -> Option<[u8; 6]> {
    (length == 6).then(|| bytes[..6].try_into().unwrap())
}

/// An IPv4 or IPv6 address, IPv4-mapped ones canonical.
///
/// # Safety
/// `address.lpSockaddr` is null or points to `iSockaddrLength` readable bytes.
unsafe fn socket_address(
    address: &windows::Win32::Networking::WinSock::SOCKET_ADDRESS,
) -> Option<IpAddr> {
    use windows::Win32::Networking::WinSock::*;
    // SAFETY: The caller's pointer is null or readable.
    let family = unsafe { address.lpSockaddr.as_ref() }?.sa_family;
    let length = address.iSockaddrLength.max(0) as usize;
    match family {
        AF_INET if length >= size_of::<SOCKADDR_IN>() => {
            // SAFETY: The caller's length covers a SOCKADDR_IN.
            let value = unsafe { &*address.lpSockaddr.cast::<SOCKADDR_IN>() };
            // SAFETY: Every view of the IN_ADDR union is the same four bytes.
            Some(Ipv4Addr::from(unsafe { value.sin_addr.S_un.S_addr }.to_ne_bytes()).into())
        }
        AF_INET6 if length >= size_of::<SOCKADDR_IN6>() => {
            // SAFETY: The caller's length covers a SOCKADDR_IN6.
            let value = unsafe { &*address.lpSockaddr.cast::<SOCKADDR_IN6>() };
            // SAFETY: Every view of the IN6_ADDR union is the same sixteen bytes.
            Some(IpAddr::from(unsafe { value.sin6_addr.u.Byte }).to_canonical())
        }
        _ => None,
    }
}

/// The PC's own address on its default route: the source Windows picks toward
/// a public IPv4 address, or toward a public IPv6 one on an IPv6-only
/// network. Nothing is sent. `None` without a default route.
pub fn default_route_source() -> Result<Option<IpAddr>> {
    // Any public addresses; the route lookup only reads the routing table.
    const PUBLIC_V4: Ipv4Addr = Ipv4Addr::new(8, 8, 8, 8);
    const PUBLIC_V6: Ipv6Addr = Ipv6Addr::new(0x2001, 0x4860, 0x4860, 0, 0, 0, 0, 0x8888);
    match route_source(PUBLIC_V4.into()) {
        Ok(None) => route_source(PUBLIC_V6.into()),
        Ok(source) => Ok(source),
        // An IPv6 route still names the adapter; otherwise the IPv4 error stands.
        Err(error) => match route_source(PUBLIC_V6.into()) {
            Ok(Some(source)) => Ok(Some(source)),
            _ => Err(error),
        },
    }
}

/// The source address of the best route toward `destination`; `None` when
/// none leads there.
fn route_source(destination: IpAddr) -> Result<Option<IpAddr>> {
    use windows::Win32::{
        Foundation::{ERROR_NETWORK_UNREACHABLE, ERROR_NOT_FOUND, ERROR_SUCCESS},
        NetworkManagement::IpHelper::{GetBestRoute2, MIB_IPFORWARD_ROW2},
        Networking::WinSock::{AF_INET, AF_INET6, SOCKADDR_INET},
    };
    let mut target = SOCKADDR_INET::default();
    match destination {
        IpAddr::V4(v4) => {
            target.si_family = AF_INET;
            // The IPv4 view of a zeroed union whose family says IPv4.
            target.Ipv4.sin_addr.S_un.S_addr = u32::from_ne_bytes(v4.octets());
        }
        IpAddr::V6(v6) => {
            target.si_family = AF_INET6;
            target.Ipv6.sin6_addr.u.Byte = v6.octets();
        }
    }
    let mut route = MIB_IPFORWARD_ROW2::default();
    let mut source = SOCKADDR_INET::default();
    // SAFETY: Every pointer is to a local that outlives the call.
    let status = unsafe { GetBestRoute2(None, 0, None, &target, 0, &mut route, &mut source) };
    if status == ERROR_NETWORK_UNREACHABLE || status == ERROR_NOT_FOUND {
        return Ok(None);
    }
    if status != ERROR_SUCCESS {
        return Err(std::io::Error::from_raw_os_error(status.0 as i32))
            .with_context(|| format!("GetBestRoute2 toward {destination}"));
    }
    // SAFETY: The family field is shared by every view; a view is read only
    // when the family names it.
    let address = unsafe {
        match source.si_family {
            AF_INET => Some(IpAddr::from(source.Ipv4.sin_addr.S_un.S_addr.to_ne_bytes())),
            AF_INET6 => Some(IpAddr::from(source.Ipv6.sin6_addr.u.Byte).to_canonical()),
            _ => None,
        }
    };
    Ok(address.filter(|address| !address.is_unspecified()))
}

/// Whether each adapter, by GUID, was last read with wake on magic packet
/// armed; the MAC policy prefers an armed NIC.
static ARMED: RwLock<Option<HashMap<String, bool>>> = RwLock::new(None);

/// The cached readiness of `adapter`; `None` until the NIC script has read it.
pub fn armed(adapter: &Adapter) -> Option<bool> {
    ARMED
        .read()
        .unwrap()
        .as_ref()?
        .get(&adapter.guid.to_ascii_uppercase())
        .copied()
}

/// Remember which adapters the NIC script last read as armed.
pub fn remember_armed(armed: impl IntoIterator<Item = (String, bool)>) {
    *ARMED.write().unwrap() = Some(
        armed
            .into_iter()
            .map(|(guid, armed)| (guid.to_ascii_uppercase(), armed))
            .collect(),
    );
}

/// Reads and applies the NIC wake settings; it takes its own text on stdin.
const NICS_SCRIPT: &str = include_str!("wake_nics.ps1");
/// The `apply` mode's FixPlan, as JSON.
const PLAN_VARIABLE: &str = "BUTTERPOLLO_WAKE_PLAN";
/// The seconds after which `apply` starts no adapter restart.
const RESTART_BEFORE_VARIABLE: &str = "BUTTERPOLLO_WAKE_RESTART_BEFORE";
/// How long `wake_nics.ps1 read` may take.
pub const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// How long `wake_nics.ps1 apply` may take before it is killed.
pub const APPLY_TIMEOUT: Duration = Duration::from_secs(180);
/// The time left for an adapter restart, which takes a few seconds: `apply`
/// starts none later than this before its kill, so the kill never lands
/// between disabling and enabling an adapter. A later one waits for the next
/// restart of the PC.
const RESTART_ALLOWANCE: Duration = Duration::from_secs(60);
/// How long `butterpollo.exe --apply-wake-settings` may take: a read and an
/// apply, each bounded above, and some slack. Whoever starts it waits at
/// least this long, so it is never killed before its script's own deadline.
pub const SETTINGS_TIMEOUT: Duration =
    Duration::from_secs(READ_TIMEOUT.as_secs() + APPLY_TIMEOUT.as_secs() + 20);

/// `wake_nics.ps1 read`, also remembering which adapters are armed. The
/// error carries PowerShell's own message, as for a blocked script
/// (Constrained Language Mode, AppLocker).
pub fn read_nics() -> Result<NicReport> {
    let report = NicReport::parse(&run_script(NICS_SCRIPT, "read", &[], READ_TIMEOUT)?)?;
    remember_armed(
        report
            .adapters
            .iter()
            .map(|nic| (nic.guid.clone(), nic.armed())),
    );
    Ok(report)
}

/// `wake_nics.ps1 apply`: changed adapters restart, which drops the network
/// for a few seconds.
pub fn apply_nics(plan: &FixPlan) -> Result<ApplyReport> {
    let restart_before = APPLY_TIMEOUT.saturating_sub(RESTART_ALLOWANCE);
    let environment = [
        (PLAN_VARIABLE, serde_json::to_string(plan)?),
        (
            RESTART_BEFORE_VARIABLE,
            restart_before.as_secs_f64().to_string(),
        ),
    ];
    let output = run_script(NICS_SCRIPT, "apply", &environment, APPLY_TIMEOUT)?;
    Ok(serde_json::from_str(output.trim_start_matches('\u{feff}'))?)
}

/// `script` on stdin with `mode`, killed after `timeout`; its stdout.
fn run_script(
    script: &str,
    mode: &str,
    environment: &[(&str, String)],
    timeout: Duration,
) -> Result<String> {
    use std::{os::windows::process::CommandExt, process::Command};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let program =
        std::path::Path::new(&root).join("System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    let mut command = Command::new(&program);
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
        .env_remove(PLAN_VARIABLE)
        .env_remove(RESTART_BEFORE_VARIABLE)
        .envs(environment.iter().map(|(name, value)| (name, value)))
        .creation_flags(CREATE_NO_WINDOW);
    let (status, stdout, stderr) = output_within(&mut command, script.as_bytes(), timeout)
        .with_context(|| format!("the NIC wake script ({mode})"))?;
    anyhow::ensure!(
        status.success(),
        "the NIC wake script ({mode}) failed with {status}: {}",
        stderr.trim()
    );
    Ok(stdout)
}

/// Run `command` with `input` on its stdin; its exit status, stdout and
/// stderr. It is killed if it hasn't finished within `timeout`; its own
/// children are not.
pub fn output_within(
    command: &mut std::process::Command,
    input: &[u8],
    timeout: Duration,
) -> Result<(std::process::ExitStatus, String, String)> {
    use std::{
        io::{Read, Write},
        process::Stdio,
    };
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("starting {}", command.get_program().to_string_lossy()))?;
    let read = |stream: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut stream) = stream {
                let _ = stream.read_to_end(&mut bytes);
            }
            String::from_utf8_lossy(&bytes).into_owned()
        })
    };
    let stdout = read(child.stdout.take().map(|s| Box::new(s) as _));
    let stderr = read(child.stderr.take().map(|s| Box::new(s) as _));
    // Dropped after writing, so the child sees the end of its input.
    let written = child
        .stdin
        .take()
        .context("the child has no stdin")?
        .write_all(input);
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("did not finish within {timeout:?} and was stopped");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    written.context("writing the child's input")?;
    Ok((status, stdout, stderr))
}

/// `GetPwrCapabilities` and `HiberbootEnabled`.
pub fn system_facts() -> Result<SystemFacts> {
    use windows::Win32::System::{
        Power::{GetPwrCapabilities, SYSTEM_POWER_CAPABILITIES},
        Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW},
    };
    let mut capabilities = SYSTEM_POWER_CAPABILITIES::default();
    // SAFETY: `capabilities` is a local the call fills.
    if !unsafe { GetPwrCapabilities(&mut capabilities) } {
        return Err(anyhow::Error::from(windows::core::Error::from_thread())
            .context("reading the power capabilities"));
    }
    let mut value = 0u32;
    let mut size = 4u32;
    // SAFETY: `value` and `size` are locals; RRF_RT_REG_DWORD writes at most 4 bytes.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            windows::core::w!("SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Power"),
            windows::core::w!("HiberbootEnabled"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&mut size),
        )
    };
    let hiberboot = match status {
        windows::Win32::Foundation::ERROR_SUCCESS => Some(value != 0),
        windows::Win32::Foundation::ERROR_FILE_NOT_FOUND => None,
        error => {
            return Err(std::io::Error::from_raw_os_error(error.0 as i32))
                .context("reading HiberbootEnabled");
        }
    };
    Ok(SystemFacts {
        s3: capabilities.SystemS3,
        modern_standby: capabilities.AoAc,
        hibernate: capabilities.HiberFilePresent,
        hiberboot,
    })
}

/// `CallNtPowerInformation(SystemExecutionState)`: the `ES_*` flags every
/// process's `SetThreadExecutionState` and power requests add up to.
pub fn execution_state() -> Result<u32> {
    use windows::Win32::System::Power::{CallNtPowerInformation, SystemExecutionState};
    let mut state = 0u32;
    // SAFETY: The output buffer is the local `state`, four bytes long.
    unsafe {
        CallNtPowerInformation(
            SystemExecutionState,
            None,
            0,
            Some((&raw mut state).cast()),
            4,
        )
    }
    .ok()
    .context("reading the system execution state")?;
    Ok(state)
}

/// Milliseconds the PC has slept since it started: `GetTickCount64` counts
/// sleep, `QueryUnbiasedInterruptTime` doesn't.
pub fn slept_ms() -> Result<u64> {
    use windows::Win32::System::{
        SystemInformation::GetTickCount64, WindowsProgramming::QueryUnbiasedInterruptTime,
    };
    let mut unbiased = 0u64;
    // SAFETY: Both read clocks; `unbiased` is a local.
    let (ticks, read) = unsafe {
        let ticks = GetTickCount64();
        (ticks, QueryUnbiasedInterruptTime(&mut unbiased).as_bool())
    };
    anyhow::ensure!(read, "QueryUnbiasedInterruptTime failed");
    Ok(ticks.saturating_sub(unbiased / 10_000))
}

/// Seconds since the last input in this session, for `--idle-seconds`:
/// `GetTickCount() - LASTINPUTINFO.dwTime`, wrapping, capped at a week.
pub fn idle_seconds() -> Result<u32> {
    use windows::Win32::{
        System::SystemInformation::GetTickCount,
        UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO},
    };
    let mut input = LASTINPUTINFO {
        cbSize: size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // SAFETY: `input` is a local with its size set.
    if !unsafe { GetLastInputInfo(&mut input) }.as_bool() {
        return Err(windows::core::Error::from_thread()).context("reading the last input time");
    }
    // SAFETY: GetTickCount only reads a clock.
    let now = unsafe { GetTickCount() };
    Ok((now.wrapping_sub(input.dwTime) / 1000).min(604_800))
}

/// Set in `--idle-seconds`' exit code when it failed; the low 16 bits carry
/// the Win32 error.
pub const IDLE_FAILED: u32 = 0x8000_0000;
/// Set in `--idle-seconds`' exit code when it read the time; the low 20 bits
/// carry the seconds. Any other code (a panic's 101, a crash) is a failure,
/// never idle time.
pub const IDLE_READ: u32 = 0x4000_0000;

/// `--idle-seconds`' exit code: [`IDLE_READ`] with [`idle_seconds`], or
/// [`IDLE_FAILED`] with the Win32 error (1 when there is none).
pub fn idle_exit_code() -> u32 {
    match idle_seconds() {
        Ok(seconds) => IDLE_READ | seconds,
        Err(error) => {
            IDLE_FAILED
                | error
                    .chain()
                    .find_map(|cause| cause.downcast_ref::<windows::core::Error>())
                    .map(|error| error.code().0 as u32 & 0xFFFF)
                    .filter(|&code| code != 0)
                    .unwrap_or(1)
        }
    }
}

/// Enables `SeShutdownPrivilege` on this thread, as Sleep PC does
/// (`power::suspend`), and puts the PC to sleep (S3). Returns after it
/// resumes. The caller has checked for S3 itself.
pub fn suspend() -> Result<()> {
    use windows::Win32::System::Power::SetSuspendState;
    let _privilege = crate::power::shutdown_privilege()?;
    // SAFETY: No pointers; it blocks until the PC resumes.
    if !unsafe { SetSuspendState(false, false, false) } {
        return Err(windows::core::Error::from_thread()).context("SetSuspendState");
    }
    Ok(())
}

/// A `PowerCreateRequest` power request, held until dropped; `powercfg
/// /requests` shows its reason.
pub struct PowerRequest(windows::Win32::Foundation::HANDLE);

// SAFETY: The handle is a kernel object handle, valid on any thread; only
// Drop uses it after creation.
unsafe impl Send for PowerRequest {}

impl PowerRequest {
    pub fn system_required(reason: &str) -> Result<Self> {
        use windows::Win32::System::{
            Power::{PowerCreateRequest, PowerRequestSystemRequired, PowerSetRequest},
            Threading::{POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0},
        };
        let mut text: Vec<u16> = reason.encode_utf16().chain([0]).collect();
        let context = REASON_CONTEXT {
            Version: 0,
            Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
            Reason: REASON_CONTEXT_0 {
                SimpleReasonString: windows::core::PWSTR(text.as_mut_ptr()),
            },
        };
        // SAFETY: `context` and the `text` it points to outlive the call, which copies them.
        let handle = unsafe { PowerCreateRequest(&context) }.context("PowerCreateRequest")?;
        let request = Self(handle);
        // SAFETY: `handle` is the request just created, closed only by Drop.
        unsafe { PowerSetRequest(handle, PowerRequestSystemRequired) }
            .context("PowerSetRequest")?;
        Ok(request)
    }
}

impl Drop for PowerRequest {
    fn drop(&mut self) {
        // SAFETY: The handle came from PowerCreateRequest and is closed only here, which also
        // ends the request.
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(self.0) };
    }
}

#[cfg(test)]
mod tests;
