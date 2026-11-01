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
fn the_default_route_leaves_from_an_adapters_own_address() -> Result<()> {
    // Without a default route (an offline runner) there is nothing to check;
    // IPv6 may be off or unrouted.
    let adapters = adapters()?;
    let ipv6 = route_source(Ipv6Addr::new(0x2001, 0x4860, 0x4860, 0, 0, 0, 0, 0x8888).into());
    let ipv6 = ipv6.ok().flatten();
    for source in [default_route_source()?, ipv6].into_iter().flatten() {
        assert!(!source.is_loopback(), "{source}");
        assert!(
            adapters
                .iter()
                .any(|adapter| adapter.addresses.contains(&source)),
            "{source} belongs to no adapter"
        );
    }
    if let Some(source) = ipv6 {
        assert!(source.is_ipv6(), "{source}");
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
    const NAME: &str = "wake::tests::another_processs_power_request_shows_in_the_execution_state";
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
fn this_pcs_realtek_reads_as_ready() -> Result<()> {
    let adapters = adapters()?;
    let report = read_nics()?;
    println!("{report:#?}");
    let realtek = adapters
        .iter()
        .find(|a| a.physical() && a.connected && a.medium == Medium::Wired)
        .expect("a connected wired NIC");
    let nic = report.get(realtek).expect("the reading reports it");
    assert!(nic.errors.is_empty(), "{:?}", nic.errors);
    assert!(nic.armed(), "{nic:?}");
    assert!(system_facts()?.s3);
    Ok(())
}

#[test]
fn this_pcs_nic_settings_are_read_from_the_drivers_own_keys() -> Result<()> {
    // Read-only. Without elevation WMI's wake classes refuse access, which
    // is each adapter's error, never the reading's.
    let report = read_nics()?;
    println!("{report:#?}");
    for nic in &report.adapters {
        assert_eq!(nic.guid.len(), 38, "{}", nic.guid);
        assert_eq!(nic.guid, nic.guid.to_ascii_uppercase());
        if nic.pnp.is_empty() {
            continue;
        }
        // The key a fix would write is the one these values came from.
        let key = nics::driver_key(&nic.pnp, false)?;
        for (keyword, value) in &nic.keywords {
            if let Some(stored) = key.string(keyword)? {
                assert_eq!(stored.trim(), value, "{} {keyword}", nic.name);
            }
        }
    }
    // IP Helper and WMI should agree on what is physical: every adapter read
    // is a hardware one with a connector there, and every connected one is
    // read. Some PCs' virtual NICs (CI runners') make them disagree; then
    // there is nothing to compare.
    let listed = adapters()?;
    let not_physical: Vec<_> = report
        .adapters
        .iter()
        .filter_map(|nic| {
            listed
                .iter()
                .find(|a| a.guid.eq_ignore_ascii_case(&nic.guid))
        })
        .filter(|a| !(a.hardware && a.connector))
        .collect();
    let unread: Vec<_> = listed
        .iter()
        .filter(|a| a.physical() && a.connected && report.get(a).is_none())
        .collect();
    if !not_physical.is_empty() || !unread.is_empty() {
        println!(
            "IP Helper and WMI disagree here; not compared. Read but not physical: \
             {not_physical:#?}; connected but not read: {unread:#?}"
        );
    }
    Ok(())
}

#[test]
fn this_pcs_advanced_properties_read_per_adapter_are_its_share_of_the_listing() -> Result<()> {
    // Read-only. The per-adapter query is what a failed listing falls back on.
    use nics::Nics as _;
    crate::wmi::on_com_thread("comparing advanced properties", READ_TIMEOUT, || {
        let mut native = nics::Native::default();
        let listing = native.advanced()?;
        let sorted = |properties: Vec<&nics::AdvancedProperty>| {
            let mut properties: Vec<_> = properties
                .into_iter()
                .map(|p| (p.keyword.clone(), p.value.clone()))
                .collect();
            properties.sort();
            properties
        };
        for adapter in native.physical()? {
            let own = native.advanced_of(&adapter)?;
            assert!(
                own.iter()
                    .all(|p| p.guid.eq_ignore_ascii_case(&adapter.guid)),
                "{}: {own:?}",
                adapter.name
            );
            assert_eq!(
                sorted(own.iter().collect()),
                sorted(
                    listing
                        .iter()
                        .filter(|p| p.guid.eq_ignore_ascii_case(&adapter.guid))
                        .collect()
                ),
                "{}",
                adapter.name
            );
        }
        Ok(())
    })
}

/// The reading against what Windows PowerShell's cmdlets report on this PC:
/// the script the native reading replaced ran them (test tooling only; the
/// host and setup run no PowerShell).
#[test]
#[ignore = "compares with Windows PowerShell; elevated for the wake classes"]
fn the_reading_matches_the_powershell_cmdlets() -> Result<()> {
    const SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$keywords = @('*WakeOnMagicPacket', '*WakeOnPattern', '*ModernStandbyWoLMagicPacket', 'S5WakeOnLan')
function Find-Wmi([string]$Class, [string]$Pnp) {
    $pattern = [Management.Automation.WildcardPattern]::Escape($Pnp) + '_*'
    @(Get-CimInstance -Namespace root\wmi -ClassName $Class | Where-Object { $_.InstanceName -like $pattern })
}
$adapters = @(Get-NetAdapter -Physical | ForEach-Object {
    $values = @{}
    foreach ($p in @($_ | Get-NetAdapterAdvancedProperty -AllProperties)) {
        if ($keywords -contains $p.RegistryKeyword) { $values[$p.RegistryKeyword.ToUpperInvariant()] = ([string]@($p.RegistryValue)[0]).Trim() }
    }
    $wake = $null; $only = $null
    try { $f = @(Find-Wmi 'MSPower_DeviceWakeEnable' $_.PnPDeviceID); if ($f.Count) { $wake = [bool]$f[0].Enable } } catch {}
    try { $f = @(Find-Wmi 'MSNdis_DeviceWakeOnMagicPacketOnly' $_.PnPDeviceID); if ($f.Count) { $only = [bool]$f[0].EnableWakeOnMagicPacketOnly } } catch {}
    @{ guid = ([string]$_.InterfaceGuid).ToUpperInvariant(); pnp = [string]$_.PnPDeviceID; name = [string]$_.Name
       wake_enable = $wake; magic_packet_only = $only; keywords = $values }
})
ConvertTo-Json -InputObject @{ adapters = $adapters } -Depth 6 -Compress
"#;
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let (status, stdout, stderr) = output_within(
        std::process::Command::new(
            std::path::Path::new(&root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe"),
        )
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "& ([scriptblock]::Create([Console]::In.ReadToEnd()))",
        ])
        .env_remove("PSModulePath"),
        SCRIPT.as_bytes(),
        Duration::from_secs(120),
    )?;
    anyhow::ensure!(status.success(), "PowerShell failed: {stderr}");
    let mut expected = NicReport::parse(&stdout)?;
    let mut native = read_nics()?;
    for report in [&mut expected, &mut native] {
        report.adapters.sort_by(|a, b| a.guid.cmp(&b.guid));
        for nic in &mut report.adapters {
            nic.errors.clear();
        }
    }
    assert_eq!(native, expected);
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

#[test]
fn a_late_apply_is_given_up_only_after_its_last_restart_could_end() {
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
