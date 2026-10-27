//! Wake-on-LAN and sleep on Windows: adapter facts, NIC wake settings, power capabilities and suspend (`docs/wake-and-audio-design.md`).
use anyhow::{Context, Result};
use butterpollo_core::wake::{Adapter, ApplyReport, FixPlan, Medium, NicReport, SystemFacts};
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr},
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

/// Set in `--idle-seconds`' exit code when it failed; the low bits carry the
/// Win32 error.
pub const IDLE_FAILED: u32 = 0x8000_0000;

/// `--idle-seconds`' exit code: [`idle_seconds`], or [`IDLE_FAILED`] with
/// the Win32 error (1 when there is none).
pub fn idle_exit_code() -> u32 {
    match idle_seconds() {
        Ok(seconds) => seconds,
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
mod tests {
    use super::*;

    #[test]
    fn media_follow_the_interface_type_and_ndis() {
        assert_eq!(medium(6, 14), Medium::Wired);
        assert_eq!(medium(6, 0), Medium::Wired);
        assert_eq!(medium(71, 9), Medium::Wifi);
        // Bluetooth PAN is Ethernet to IP Helper.
        assert_eq!(medium(6, 10), Medium::Other);
        assert_eq!(medium(24, 0), Medium::Other);
        assert_eq!(medium(53, 0), Medium::Other);
    }

    #[test]
    fn this_pcs_adapters_are_read() -> Result<()> {
        let adapters = adapters()?;
        anyhow::ensure!(!adapters.is_empty(), "no adapters");
        // Loopback is always there and never physical.
        let loopback = adapters
            .iter()
            .find(|a| a.addresses.iter().any(IpAddr::is_loopback))
            .expect("loopback");
        assert!(!loopback.physical());
        for adapter in &adapters {
            if !adapter.guid.is_empty() {
                assert_eq!(adapter.guid.len(), 38, "{}", adapter.guid);
            }
        }
        Ok(())
    }

    #[test]
    fn power_facts_and_clocks_are_read() -> Result<()> {
        println!("{:?}", system_facts()?);
        execution_state()?;
        // Within a second of each other unless the PC has slept.
        let slept = slept_ms()?;
        assert!(slept_ms()? + 1000 >= slept);
        Ok(())
    }

    const HOLD: &str = "BUTTERPOLLO_TEST_HOLD_POWER_REQUEST";

    /// H1 step 1: sleep after idle relies on this (design A.5, point 5).
    #[test]
    #[ignore = "run as SYSTEM on an idle PC: rust/release/elevated.ps1"]
    fn another_processs_power_request_shows_in_the_execution_state() -> Result<()> {
        use windows::Win32::System::Power::ES_SYSTEM_REQUIRED;
        const NAME: &str =
            "wake::tests::another_processs_power_request_shows_in_the_execution_state";
        if std::env::var_os(HOLD).is_some() {
            let _request = PowerRequest::system_required("Rubylight wake test")?;
            std::thread::sleep(Duration::from_secs(8));
            return Ok(());
        }
        let required = ES_SYSTEM_REQUIRED.0;
        let before = execution_state()?;
        anyhow::ensure!(
            before & required == 0,
            "inconclusive: ES_SYSTEM_REQUIRED is already set ({before:#x}); see powercfg /requests"
        );
        let mut child = std::process::Command::new(std::env::current_exe()?)
            .args([NAME, "--exact", "--ignored", "--nocapture"])
            .env(HOLD, "1")
            .spawn()?;
        std::thread::sleep(Duration::from_secs(4));
        let held = execution_state()?;
        let status = child.wait()?;
        let after = execution_state()?;
        println!("execution state: before {before:#x}, held {held:#x}, after {after:#x}");
        assert!(status.success(), "the child failed: {status}");
        assert_ne!(held & required, 0, "the child's request is not visible");
        assert_eq!(after & required, 0, "the request outlived the child");
        Ok(())
    }

    #[test]
    #[ignore = "run as SYSTEM on this PC: rust/release/elevated.ps1"]
    fn the_nic_script_reads_this_pcs_realtek_as_ready() -> Result<()> {
        let adapters = adapters()?;
        let report = read_nics()?;
        println!("{report:#?}");
        let realtek = adapters
            .iter()
            .find(|a| a.physical() && a.connected && a.medium == Medium::Wired)
            .expect("a connected wired NIC");
        let nic = report.get(realtek).expect("the script reports it");
        assert!(nic.errors.is_empty(), "{:?}", nic.errors);
        assert!(nic.armed(), "{nic:?}");
        assert!(system_facts()?.s3);
        Ok(())
    }

    #[test]
    #[ignore = "needs a signed-in desktop session"]
    fn idle_seconds_are_plausible() -> Result<()> {
        let idle = idle_seconds()?;
        println!("idle for {idle} s");
        assert!(idle <= 604_800);
        Ok(())
    }

    #[test]
    fn armed_is_cached_by_guid_in_any_case() {
        let adapter = Adapter {
            guid: "{875511C7-0EF5-45F7-876C-3BE50726AC42}".into(),
            ..Default::default()
        };
        remember_armed([("{875511c7-0ef5-45f7-876c-3be50726ac42}".into(), true)]);
        assert_eq!(armed(&adapter), Some(true));
        assert_eq!(
            armed(&Adapter {
                guid: "{00000000-0000-0000-0000-000000000000}".into(),
                ..Default::default()
            }),
            None
        );
    }

    /// `function name` from the NIC script, up to its closing brace.
    fn script_function(name: &str) -> &'static str {
        let start = NICS_SCRIPT
            .find(&format!("function {name}"))
            .unwrap_or_else(|| panic!("{name} is not in the NIC script"));
        let end = start + NICS_SCRIPT[start..].find("\n}\n").unwrap() + 3;
        &NICS_SCRIPT[start..end]
    }
    /// `body` after `functions` from the NIC script, with stand-ins for the
    /// cmdlets that would touch the adapters; its JSON.
    fn run_with_stand_ins(
        functions: &[&str],
        body: &str,
        environment: &[(&str, String)],
    ) -> Result<serde_json::Value> {
        let mut script = String::from("$ErrorActionPreference = 'Stop'\n");
        script += "$log = [Collections.Generic.List[string]]::new()\n";
        for name in functions {
            script += script_function(name);
        }
        script += body;
        let output = run_script(&script, "", environment, Duration::from_secs(60))?;
        Ok(serde_json::from_str(output.trim_start_matches('\u{feff}'))?)
    }

    #[test]
    fn wmi_instances_match_the_whole_pnp_id_and_an_empty_one_none() -> Result<()> {
        let found = run_with_stand_ins(
            &["Find-Wmi"],
            r##"
function Get-CimInstance { param($Namespace, $ClassName)
    'PCI\VEN_10EC&DEV_8125\01_0', 'PCI\VEN_10EC&DEV_8125\012_0', 'PCI\VEN_10EC&DEV_8125[1]_0' |
        ForEach-Object { [pscustomobject]@{ InstanceName = $_ } } }
$empty = try { Find-Wmi 'MSPower_DeviceWakeEnable' ''; 'matched' } catch { "$_" }
ConvertTo-Json -Compress -InputObject @{
    one = @(Find-Wmi 'MSPower_DeviceWakeEnable' 'pci\ven_10ec&dev_8125\01' | ForEach-Object InstanceName)
    bracket = @(Find-Wmi 'MSPower_DeviceWakeEnable' 'PCI\VEN_10EC&DEV_8125[1]' | ForEach-Object InstanceName)
    empty = $empty
}
"##,
            &[],
        )?;
        assert_eq!(
            found["one"],
            serde_json::json!(["PCI\\VEN_10EC&DEV_8125\\01_0"])
        );
        assert_eq!(
            found["bracket"],
            serde_json::json!(["PCI\\VEN_10EC&DEV_8125[1]_0"])
        );
        assert_eq!(found["empty"], "the adapter has no PnP id");
        Ok(())
    }

    #[test]
    fn a_failed_restart_enables_the_adapter_again() -> Result<()> {
        let restarted = run_with_stand_ins(
            &["Restart-Adapter"],
            r##"
function Disable-NetAdapter { param([Parameter(ValueFromPipeline)]$InputObject, [switch]$Confirm)
    process { $log.Add('disable'); throw 'disabling failed halfway' } }
function Enable-NetAdapter { param([Parameter(ValueFromPipeline)]$InputObject, [switch]$Confirm)
    process { $log.Add('enable'); if ($log.Count -eq 2) { throw 'busy' } } }
$failure = try { Restart-Adapter ([pscustomobject]@{ Name = 'Ethernet' }); 'none' } catch { "$_" }
ConvertTo-Json -Compress -InputObject @{ log = @($log); error = $failure }
"##,
            &[],
        )?;
        assert_eq!(
            restarted["log"],
            serde_json::json!(["disable", "enable", "enable"])
        );
        assert_eq!(restarted["error"], "disabling failed halfway");
        Ok(())
    }

    #[test]
    fn a_restart_too_close_to_the_kill_waits_for_the_next_boot() -> Result<()> {
        let plan = FixPlan {
            adapters: vec![butterpollo_core::wake::AdapterFix {
                guid: "{G}".into(),
                pnp: "PCI\\X".into(),
                name: "Ethernet".into(),
                keywords: [("*WakeOnMagicPacket".to_string(), "1".to_string())].into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let apply = |restart_before: Option<&str>| {
            let mut environment = vec![(PLAN_VARIABLE, serde_json::to_string(&plan).unwrap())];
            if let Some(seconds) = restart_before {
                environment.push((RESTART_BEFORE_VARIABLE, seconds.to_string()));
            }
            run_with_stand_ins(
                &["Invoke-Apply"],
                r##"
function Get-NetAdapter { param([switch]$Physical) [pscustomobject]@{ InterfaceGuid = '{G}' } }
function Set-NetAdapterAdvancedProperty { param([Parameter(ValueFromPipeline)]$InputObject,
    $RegistryKeyword, $RegistryValue, [switch]$NoRestart) process { $log.Add("set $RegistryKeyword") } }
function Restart-Adapter($adapter) { $log.Add('restart') }
ConvertTo-Json -Compress -Depth 4 -InputObject @{ report = (Invoke-Apply); log = @($log) }
"##,
                &environment,
            )
        };
        let early = apply(Some("120"))?;
        assert_eq!(
            early["log"],
            serde_json::json!(["set *WakeOnMagicPacket", "restart"])
        );
        assert_eq!(early["report"]["restarted"], serde_json::json!(["{G}"]));
        assert_eq!(early["report"]["after_reboot"], serde_json::json!([]));
        let late = apply(Some("0.000001"))?;
        assert_eq!(late["log"], serde_json::json!(["set *WakeOnMagicPacket"]));
        assert_eq!(late["report"]["restarted"], serde_json::json!([]));
        assert_eq!(late["report"]["after_reboot"], serde_json::json!(["{G}"]));
        // Without its deadline the script changes nothing.
        assert!(apply(None).is_err());
        Ok(())
    }

    #[test]
    fn the_host_kills_apply_only_after_its_last_restart_could_end() {
        assert!(APPLY_TIMEOUT >= RESTART_ALLOWANCE * 2);
        assert!(SETTINGS_TIMEOUT > READ_TIMEOUT + APPLY_TIMEOUT);
    }

    #[test]
    fn a_child_past_its_timeout_is_stopped() {
        let started = Instant::now();
        let error = output_within(
            std::process::Command::new("ping").args(["-n", "30", "127.0.0.1"]),
            b"",
            Duration::from_millis(500),
        )
        .unwrap_err();
        assert!(error.to_string().contains("did not finish"), "{error:#}");
        assert!(started.elapsed() < Duration::from_secs(10));
        let (status, stdout, _) = output_within(
            std::process::Command::new("cmd").args(["/d", "/c", "findstr", "x"]),
            b"x marks it\r\n",
            Duration::from_secs(30),
        )
        .unwrap();
        assert!(status.success());
        assert_eq!(stdout.trim(), "x marks it");
    }
}
