use super::*;
use butterpollo_core::wake::AdapterFix;

const GUID: &str = "{875511C7-0EF5-45F7-876C-3BE50726AC42}";
const PNP: &str = r"PCI\VEN_10EC&DEV_8125\01";

/// Windows as the tests set it up; every call is logged, and the calls in
/// `fail` fail once each with their message.
#[derive(Default)]
struct Fake {
    adapters: Vec<PhysicalAdapter>,
    advanced: Vec<AdvancedProperty>,
    wake: Vec<WmiInstance>,
    only: Vec<WmiInstance>,
    fail: Vec<(&'static str, &'static str)>,
    log: Vec<String>,
}

impl Fake {
    fn call(&mut self, call: &str, detail: String) -> Result<()> {
        self.log.push(if detail.is_empty() {
            call.to_owned()
        } else {
            format!("{call} {detail}")
        });
        match self.fail.iter().position(|(failing, _)| *failing == call) {
            Some(at) => bail!(self.fail.remove(at).1),
            None => Ok(()),
        }
    }
}

impl Nics for Fake {
    fn physical(&mut self) -> Result<Vec<PhysicalAdapter>> {
        self.call("physical", String::new())?;
        Ok(self.adapters.clone())
    }
    fn advanced(&mut self) -> Result<Vec<AdvancedProperty>> {
        self.call("advanced", String::new())?;
        Ok(self.advanced.clone())
    }
    fn instances(&mut self, setting: WakeSetting) -> Result<Vec<WmiInstance>> {
        self.call(setting.class(), String::new())?;
        Ok(match setting {
            WakeSetting::WakeEnable => self.wake.clone(),
            WakeSetting::MagicPacketOnly => self.only.clone(),
        })
    }
    fn set_advanced(
        &mut self,
        adapter: &PhysicalAdapter,
        keyword: &str,
        value: &str,
    ) -> Result<()> {
        self.call("set", format!("{} {keyword}={value}", adapter.name))
    }
    fn turn_on(&mut self, setting: WakeSetting, instance: &WmiInstance) -> Result<()> {
        self.call(
            "turn on",
            format!("{} {}", setting.property(), instance.name),
        )
    }
    fn disable(&mut self, adapter: &PhysicalAdapter) -> Result<()> {
        self.call("disable", adapter.name.clone())
    }
    fn enable(&mut self, adapter: &PhysicalAdapter) -> Result<()> {
        self.call("enable", adapter.name.clone())
    }
    fn fast_startup_off(&mut self) -> Result<()> {
        self.call("fast startup off", String::new())
    }
    fn pause(&mut self, duration: Duration) {
        self.log.push(format!("pause {duration:?}"));
    }
}

fn ethernet() -> PhysicalAdapter {
    PhysicalAdapter {
        guid: GUID.into(),
        pnp: PNP.into(),
        name: "Ethernet".into(),
        description: "Realtek PCIe 2.5GbE Family Controller".into(),
        medium: 14,
        path: "MSFT_NetAdapter.DeviceID=\"x\"".into(),
    }
}

fn instance(name: &str, on: bool) -> WmiInstance {
    WmiInstance {
        name: name.into(),
        on,
        path: format!("path of {name}"),
    }
}

fn property(guid: &str, keyword: &str, value: &str) -> AdvancedProperty {
    AdvancedProperty {
        guid: guid.into(),
        keyword: keyword.into(),
        value: value.into(),
    }
}

fn fix(keywords: &[(&str, &str)]) -> AdapterFix {
    AdapterFix {
        guid: GUID.into(),
        pnp: PNP.into(),
        name: "Ethernet".into(),
        keywords: keywords
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        ..Default::default()
    }
}

#[test]
fn wmi_instances_match_the_whole_pnp_id_and_an_empty_one_none() {
    let instances = [
        instance(r"PCI\VEN_10EC&DEV_8125\01_0", true),
        instance(r"PCI\VEN_10EC&DEV_8125\012_0", true),
        instance(r"PCI\VEN_10EC&DEV_8125[1]_0", true),
        instance(r"PCI\VEN_10EC&DEV_8125\01", true),
    ];
    let names = |pnp: &str| -> Vec<String> {
        matching(&instances, pnp)
            .unwrap()
            .iter()
            .map(|i| i.name.clone())
            .collect()
    };
    assert_eq!(
        names(r"pci\ven_10ec&dev_8125\01"),
        [r"PCI\VEN_10EC&DEV_8125\01_0"]
    );
    assert_eq!(
        names(r"PCI\VEN_10EC&DEV_8125[1]"),
        [r"PCI\VEN_10EC&DEV_8125[1]_0"]
    );
    assert!(names(r"PCI\VEN_10EC&DEV_8125\0123").is_empty());
    assert_eq!(
        matching(&instances, "").unwrap_err().to_string(),
        "the adapter has no PnP id"
    );
}

#[test]
fn a_single_wmi_instance_is_read() -> Result<()> {
    // The script this replaced read a lone instance as unreadable on every PC.
    let mut fake = Fake {
        adapters: vec![ethernet()],
        advanced: vec![
            property(GUID, "*WakeOnMagicPacket", " 1 "),
            property(GUID, "*SpeedDuplex", "0"),
        ],
        wake: vec![instance(&format!("{PNP}_0"), true)],
        only: vec![instance(&format!("{PNP}_0"), false)],
        ..Default::default()
    };
    let report = read(&mut fake)?;
    let [nic] = report.adapters.as_slice() else {
        panic!("{report:?}");
    };
    assert_eq!(nic.errors, Vec::<String>::new());
    assert_eq!(nic.guid, GUID);
    assert_eq!(nic.pnp, PNP);
    assert_eq!(nic.name, "Ethernet");
    assert_eq!(nic.wake_enable, Some(true));
    assert_eq!(nic.magic_packet_only, Some(false));
    // Only the wake keywords, in upper case and trimmed, as core keeps them.
    assert_eq!(
        nic.keywords,
        BTreeMap::from([("*WAKEONMAGICPACKET".to_string(), "1".to_string())])
    );
    assert!(nic.armed());
    Ok(())
}

#[test]
fn each_adapter_gets_its_own_keywords_and_every_failure_of_its_own() -> Result<()> {
    let wifi = PhysicalAdapter {
        guid: "{A5234352-A9A6-4D45-8B0B-2A1CA4957AF2}".into(),
        pnp: String::new(),
        name: "WLAN".into(),
        medium: 9,
        ..Default::default()
    };
    let mut fake = Fake {
        adapters: vec![ethernet(), wifi.clone()],
        advanced: vec![
            property(&GUID.to_ascii_lowercase(), "s5wakeonlan", "1"),
            property(&wifi.guid, "*WakeOnPattern", "0"),
            property(&wifi.guid, "*ModernStandbyWoLMagicPacket", ""),
        ],
        wake: vec![instance(&format!("{PNP}_0"), false)],
        fail: vec![("MSNdis_DeviceWakeOnMagicPacketOnly", "Access denied")],
        ..Default::default()
    };
    let report = read(&mut fake)?;
    let (ethernet, wifi) = (&report.adapters[0], &report.adapters[1]);
    assert_eq!(
        ethernet.keywords,
        BTreeMap::from([("S5WAKEONLAN".to_string(), "1".to_string())])
    );
    assert_eq!(ethernet.wake_enable, Some(false));
    assert_eq!(ethernet.magic_packet_only, None);
    assert_eq!(
        ethernet.errors,
        ["MSNdis_DeviceWakeOnMagicPacketOnly: Access denied"]
    );
    assert_eq!(wifi.keywords.len(), 2);
    assert_eq!(wifi.keyword("*ModernStandbyWoLMagicPacket"), Some(""));
    // Without a PnP id nothing of WMI can be its own.
    assert_eq!(
        wifi.errors,
        [
            "MSPower_DeviceWakeEnable: the adapter has no PnP id",
            "MSNdis_DeviceWakeOnMagicPacketOnly: Access denied"
        ]
    );
    // Unreadable advanced properties are each adapter's error too.
    fake.fail = vec![("advanced", "Not supported")];
    let report = read(&mut fake)?;
    assert!(report.adapters[0].keywords.is_empty());
    assert_eq!(
        report.adapters[0].errors,
        ["advanced properties: Not supported"]
    );
    // Without the adapters there is nothing to report.
    fake.fail = vec![("physical", "Access denied")];
    assert_eq!(read(&mut fake).unwrap_err().to_string(), "Access denied");
    Ok(())
}

#[test]
fn a_failed_restart_enables_the_adapter_again() -> Result<()> {
    let plan = FixPlan {
        adapters: vec![fix(&[("*WakeOnMagicPacket", "1")])],
        ..Default::default()
    };
    let mut fake = Fake {
        adapters: vec![ethernet()],
        fail: vec![("disable", "disabling failed halfway"), ("enable", "busy")],
        ..Default::default()
    };
    let report = apply(&mut fake, &plan, Duration::from_secs(120))?;
    assert_eq!(
        fake.log,
        [
            "physical",
            "set Ethernet *WakeOnMagicPacket=1",
            "disable Ethernet",
            "enable Ethernet",
            "pause 2s",
            "enable Ethernet"
        ]
    );
    assert_eq!(
        report.errors,
        [format!("{GUID} restart: disabling failed halfway")]
    );
    assert!(report.restarted.is_empty());
    // Enabling that fails twice is the error that counts: the adapter is off.
    fake.log.clear();
    fake.fail = vec![("enable", "busy"), ("enable", "still busy")];
    let report = apply(&mut fake, &plan, Duration::from_secs(120))?;
    assert_eq!(report.errors, [format!("{GUID} restart: still busy")]);
    Ok(())
}

#[test]
fn a_restart_too_close_to_the_deadline_waits_for_the_next_boot() -> Result<()> {
    let plan = FixPlan {
        adapters: vec![fix(&[("*WakeOnMagicPacket", "1")])],
        ..Default::default()
    };
    let mut fake = Fake {
        adapters: vec![ethernet()],
        ..Default::default()
    };
    let early = apply(&mut fake, &plan, Duration::from_secs(120))?;
    assert_eq!(
        fake.log,
        [
            "physical",
            "set Ethernet *WakeOnMagicPacket=1",
            "disable Ethernet",
            "enable Ethernet"
        ]
    );
    assert_eq!(early.restarted, [GUID]);
    assert!(early.after_reboot.is_empty());
    fake.log.clear();
    let late = apply(&mut fake, &plan, Duration::ZERO)?;
    assert_eq!(fake.log, ["physical", "set Ethernet *WakeOnMagicPacket=1"]);
    assert!(late.restarted.is_empty());
    assert_eq!(late.after_reboot, [GUID]);
    assert!(late.errors.is_empty());
    Ok(())
}

#[test]
fn apply_turns_wake_on_for_the_adapters_device_only_and_reports_each_failure() -> Result<()> {
    let mut wake_only = fix(&[]);
    wake_only.wake_enable = true;
    wake_only.magic_packet_only = true;
    let mut failing = fix(&[("*ModernStandbyWoLMagicPacket", "1")]);
    failing.guid = GUID.to_ascii_lowercase();
    let plan = FixPlan {
        adapters: vec![
            wake_only,
            failing,
            AdapterFix {
                guid: "{00000000-0000-0000-0000-000000000000}".into(),
                ..Default::default()
            },
        ],
        fast_startup_off: true,
        skipped: vec![],
    };
    let mut fake = Fake {
        adapters: vec![ethernet()],
        wake: vec![
            instance(&format!("{PNP}_0"), false),
            instance(&format!("{PNP}2_0"), false),
        ],
        only: vec![instance(&format!("{PNP}_0"), false)],
        fail: vec![
            (
                "set",
                "the adapter has no advanced property *ModernStandbyWoLMagicPacket",
            ),
            ("fast startup off", "Access is denied."),
        ],
        ..Default::default()
    };
    let report = apply(&mut fake, &plan, Duration::from_secs(120))?;
    assert_eq!(
        fake.log,
        [
            "physical".to_string(),
            "MSPower_DeviceWakeEnable".into(),
            format!("turn on Enable {PNP}_0"),
            "MSNdis_DeviceWakeOnMagicPacketOnly".into(),
            format!("turn on EnableWakeOnMagicPacketOnly {PNP}_0"),
            "set Ethernet *ModernStandbyWoLMagicPacket=1".into(),
            "fast startup off".into(),
        ]
    );
    // WMI changes take effect at once and a failed property changes nothing:
    // no restart.
    assert!(report.restarted.is_empty() && report.after_reboot.is_empty());
    assert_eq!(
        report.errors,
        [
            format!(
                "{} *ModernStandbyWoLMagicPacket: the adapter has no advanced property *ModernStandbyWoLMagicPacket",
                GUID.to_ascii_lowercase()
            ),
            "{00000000-0000-0000-0000-000000000000}: no such adapter".into(),
            "HiberbootEnabled: Access is denied.".into(),
        ]
    );
    // A failed WMI change is the adapter's error; the rest goes on.
    let mut wake = fix(&[]);
    wake.wake_enable = true;
    let plan = FixPlan {
        adapters: vec![wake],
        ..Default::default()
    };
    fake.fail = vec![("turn on", "Generic failure")];
    let report = apply(&mut fake, &plan, Duration::from_secs(120))?;
    assert_eq!(
        report.errors,
        [format!("{GUID} MSPower_DeviceWakeEnable: Generic failure")]
    );
    Ok(())
}

/// A scratch key under HKCU\Software\Rubylight\Tests, deleted when dropped.
struct Scratch(Key, Vec<u16>);

impl Drop for Scratch {
    fn drop(&mut self) {
        use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteTreeW};
        // SAFETY: The path is NUL-terminated and lives in self.
        let _ =
            unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, windows::core::PCWSTR(self.1.as_ptr())) };
    }
}

fn scratch() -> Scratch {
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_ALL_ACCESS, REG_OPTION_VOLATILE, RegCreateKeyExW,
    };
    let path = to_wide(&format!(
        r"Software\Rubylight\Tests\wake-{}",
        std::process::id()
    ));
    let mut key = HKEY::default();
    // SAFETY: The path is NUL-terminated and outlives the call; `key` is a live local.
    unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            windows::core::PCWSTR(path.as_ptr()),
            None,
            None,
            REG_OPTION_VOLATILE,
            KEY_ALL_ACCESS,
            None,
            &mut key,
            None,
        )
    }
    .ok()
    .unwrap();
    Scratch(Key(key), path)
}

fn create(key: &Key, path: &str) -> Key {
    use windows::Win32::System::Registry::{
        HKEY, KEY_ALL_ACCESS, REG_OPTION_VOLATILE, RegCreateKeyExW,
    };
    let wide = to_wide(path);
    let mut created = HKEY::default();
    // SAFETY: As in `scratch`.
    unsafe {
        RegCreateKeyExW(
            key.0,
            windows::core::PCWSTR(wide.as_ptr()),
            None,
            None,
            REG_OPTION_VOLATILE,
            KEY_ALL_ACCESS,
            None,
            &mut created,
            None,
        )
    }
    .ok()
    .unwrap();
    Key(created)
}

#[test]
fn an_advanced_property_is_written_only_as_the_driver_defines_it() -> Result<()> {
    // A driver key of its own under HKCU: never a real adapter's.
    let scratch = scratch();
    let driver = &scratch.0;
    let magic = create(driver, r"Ndi\Params\*WakeOnMagicPacket\Enum");
    magic.set_string("0", "Disabled")?;
    magic.set_string("1", "Enabled")?;
    create(driver, r"Ndi\Params\*ModernStandbyWoLMagicPacket");
    driver.set_string("*WakeOnMagicPacket", "0")?;
    driver.set_dword("*ModernStandbyWoLMagicPacket", 0)?;

    write_advanced(driver, "*WakeOnMagicPacket", "1")?;
    assert_eq!(driver.string("*WakeOnMagicPacket")?.as_deref(), Some("1"));
    // A value the driver doesn't list, or a property it doesn't define, is
    // refused and changes nothing.
    let error = write_advanced(driver, "*WakeOnMagicPacket", "2").unwrap_err();
    assert_eq!(
        error.to_string(),
        "\"2\" is not one of the values *WakeOnMagicPacket takes"
    );
    assert_eq!(driver.string("*WakeOnMagicPacket")?.as_deref(), Some("1"));
    let error = write_advanced(driver, "*WakeOnPattern", "1").unwrap_err();
    assert_eq!(
        error.to_string(),
        "the adapter has no advanced property *WakeOnPattern"
    );
    assert_eq!(driver.kind("*WakeOnPattern")?, None);
    // A number stays a number.
    write_advanced(driver, "*ModernStandbyWoLMagicPacket", "1")?;
    assert_eq!(
        driver.kind("*ModernStandbyWoLMagicPacket")?,
        Some(windows::Win32::System::Registry::REG_DWORD.0)
    );
    assert!(write_advanced(driver, "*ModernStandbyWoLMagicPacket", "on").is_err());
    // Without a value yet it is written as a string.
    create(driver, r"Ndi\Params\S5WakeOnLan");
    write_advanced(driver, "S5WakeOnLan", "1")?;
    assert_eq!(driver.string("S5WakeOnLan")?.as_deref(), Some("1"));
    Ok(())
}
