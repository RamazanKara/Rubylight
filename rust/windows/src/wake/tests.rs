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
    function_in(NICS_SCRIPT, name)
}

/// `function name` in `script` up to and including the line ending after its
/// closing brace. CI's Windows checkout gives the script CRLF endings, so
/// both kinds count.
fn function_in<'a>(script: &'a str, name: &str) -> &'a str {
    let start = script
        .find(&format!("function {name}"))
        .unwrap_or_else(|| panic!("{name} is not in the NIC script"));
    let rest = &script[start..];
    let (close, ending) = ["\n}\n", "\r\n}\r\n"]
        .iter()
        .filter_map(|ending| rest.find(ending).map(|at| (at, ending.len())))
        .min()
        .unwrap_or_else(|| panic!("{name} has no closing brace in the NIC script"));
    &rest[..close + ending]
}

#[test]
fn script_functions_are_found_with_either_line_ending() {
    let lf = "function A {\n  1\n}\nfunction B {\n  2\n}\n";
    let crlf = lf.replace('\n', "\r\n");
    assert_eq!(function_in(lf, "A"), "function A {\n  1\n}\n");
    assert_eq!(function_in(lf, "B"), "function B {\n  2\n}\n");
    assert_eq!(function_in(&crlf, "A"), "function A {\r\n  1\r\n}\r\n");
    assert_eq!(function_in(&crlf, "B"), "function B {\r\n  2\r\n}\r\n");
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
