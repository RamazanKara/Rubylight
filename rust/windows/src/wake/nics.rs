//! The wake settings of the physical network adapters
//! (docs/wake-and-audio-design.md, A.3): [`read`] reports them and [`apply`]
//! carries out a [`FixPlan`]. They read and change what `Get-NetAdapter
//! -Physical`, `Get-NetAdapterAdvancedProperty -AllProperties`,
//! `Set-NetAdapterAdvancedProperty -NoRestart`, `Disable-NetAdapter`,
//! `Enable-NetAdapter` and the `root\wmi` wake classes do, through WMI and the
//! driver's registry key, with no script host. The report is keyed by
//! InterfaceGuid, PnP id and registry keyword, never by display names, which
//! are localized; core judges it (butterpollo_core::wake).
//!
//! The logic works on [`Nics`], which tests fake; [`Native`] is Windows.

use crate::{
    net_adapter::{self, PhysicalAdapter},
    text::to_wide,
    wmi::{self, Wmi},
};
use anyhow::{Context, Result, bail};
use butterpollo_core::wake::{
    ApplyReport, FixPlan, MAGIC_PACKET, MODERN_STANDBY_MAGIC_PACKET, NicReport, NicSettings,
    SHUTDOWN_WAKE, WAKE_ON_PATTERN,
};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

/// The advanced properties the report carries.
const KEYWORDS: [&str; 4] = [
    MAGIC_PACKET,
    WAKE_ON_PATTERN,
    MODERN_STANDBY_MAGIC_PACKET,
    SHUTDOWN_WAKE,
];
/// How long a failed enable waits before it tries once more.
const ENABLE_RETRY: Duration = Duration::from_secs(2);

/// A wake setting of the adapter's device in `root\wmi`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WakeSetting {
    /// "Allow this device to wake the computer".
    WakeEnable,
    /// "Only allow a magic packet to wake the computer".
    MagicPacketOnly,
}

impl WakeSetting {
    pub(super) fn class(self) -> &'static str {
        match self {
            Self::WakeEnable => "MSPower_DeviceWakeEnable",
            Self::MagicPacketOnly => "MSNdis_DeviceWakeOnMagicPacketOnly",
        }
    }
    fn property(self) -> &'static str {
        match self {
            Self::WakeEnable => "Enable",
            Self::MagicPacketOnly => "EnableWakeOnMagicPacketOnly",
        }
    }
}

/// One instance of a [`WakeSetting`] class.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct WmiInstance {
    /// The device's PnP id followed by `_0`.
    pub name: String,
    pub on: bool,
    /// Its `__RELPATH`, to change it.
    pub path: String,
}

/// One advanced property of an adapter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct AdvancedProperty {
    /// The adapter's InterfaceGuid.
    pub guid: String,
    pub keyword: String,
    /// The first of its registry values, empty when it has none.
    pub value: String,
}

/// What [`read`] and [`apply`] ask Windows.
pub(super) trait Nics {
    /// `Get-NetAdapter -Physical`.
    fn physical(&mut self) -> Result<Vec<PhysicalAdapter>>;
    /// Every adapter's advanced properties, hidden ones too.
    fn advanced(&mut self) -> Result<Vec<AdvancedProperty>>;
    /// Every instance of the setting's class, of any device.
    fn instances(&mut self, setting: WakeSetting) -> Result<Vec<WmiInstance>>;
    /// Saves one advanced property without restarting the adapter.
    fn set_advanced(&mut self, adapter: &PhysicalAdapter, keyword: &str, value: &str)
    -> Result<()>;
    fn turn_on(&mut self, setting: WakeSetting, instance: &WmiInstance) -> Result<()>;
    fn disable(&mut self, adapter: &PhysicalAdapter) -> Result<()>;
    fn enable(&mut self, adapter: &PhysicalAdapter) -> Result<()>;
    /// `HiberbootEnabled = 0`.
    fn fast_startup_off(&mut self) -> Result<()>;
    fn pause(&mut self, duration: Duration);
}

/// The instances that belong to the device `pnp`: their InstanceName is the
/// PnP id followed by `_` and an index, in any case; a longer id that starts
/// with this one is another device. An empty id would match every instance.
pub(super) fn matching<'a>(
    instances: &'a [WmiInstance],
    pnp: &str,
) -> Result<Vec<&'a WmiInstance>> {
    if pnp.is_empty() {
        bail!("the adapter has no PnP id");
    }
    Ok(instances
        .iter()
        .filter(|instance| {
            instance
                .name
                .get(..pnp.len())
                .is_some_and(|start| start.eq_ignore_ascii_case(pnp))
                && instance.name[pnp.len()..].starts_with('_')
        })
        .collect())
}

/// Every physical adapter's wake settings. Only listing the adapters can
/// fail; what can't be read of one adapter is in its `errors`.
pub(super) fn read(nics: &mut impl Nics) -> Result<NicReport> {
    let adapters = nics.physical()?;
    let advanced = nics.advanced();
    let wake = nics.instances(WakeSetting::WakeEnable);
    let only = nics.instances(WakeSetting::MagicPacketOnly);
    let adapters = adapters
        .iter()
        .map(|adapter| {
            let mut errors = Vec::new();
            let mut keywords = BTreeMap::new();
            match &advanced {
                Ok(properties) => {
                    for property in properties.iter().filter(|p| {
                        p.guid.eq_ignore_ascii_case(&adapter.guid)
                            && KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(&p.keyword))
                    }) {
                        // As core keeps them: keywords in upper case, values trimmed.
                        keywords.insert(
                            property.keyword.to_ascii_uppercase(),
                            property.value.trim().to_owned(),
                        );
                    }
                }
                Err(error) => errors.push(format!("advanced properties: {error:#}")),
            }
            let mut flag = |setting: WakeSetting, instances: &Result<Vec<WmiInstance>>| {
                let found = match instances {
                    Ok(instances) => matching(instances, &adapter.pnp),
                    Err(error) => Err(anyhow::anyhow!("{error:#}")),
                };
                match found {
                    Ok(found) => found.first().map(|instance| instance.on),
                    Err(error) => {
                        errors.push(format!("{}: {error:#}", setting.class()));
                        None
                    }
                }
            };
            let wake_enable = flag(WakeSetting::WakeEnable, &wake);
            let magic_packet_only = flag(WakeSetting::MagicPacketOnly, &only);
            NicSettings {
                guid: adapter.guid.to_ascii_uppercase(),
                pnp: adapter.pnp.clone(),
                name: adapter.name.clone(),
                wake_enable,
                magic_packet_only,
                keywords,
                errors,
            }
        })
        .collect();
    Ok(NicReport { adapters })
}

/// Carries out `plan`. A changed advanced property restarts its adapter,
/// unless `restart_before` has passed since the start: then it takes effect
/// when the PC restarts, so whoever stops a late apply never stops it in the
/// middle of a restart. Only listing the adapters can fail; each failed
/// change is in the report's `errors`.
pub(super) fn apply(
    nics: &mut impl Nics,
    plan: &FixPlan,
    restart_before: Duration,
) -> Result<ApplyReport> {
    let started = Instant::now();
    let adapters = nics.physical()?;
    let mut report = ApplyReport::default();
    for fix in &plan.adapters {
        let Some(adapter) = adapters
            .iter()
            .find(|adapter| adapter.guid.eq_ignore_ascii_case(&fix.guid))
        else {
            report.errors.push(format!("{}: no such adapter", fix.guid));
            continue;
        };
        let mut changed = false;
        for (keyword, value) in &fix.keywords {
            match nics.set_advanced(adapter, keyword, value) {
                Ok(()) => changed = true,
                Err(error) => report
                    .errors
                    .push(format!("{} {keyword}: {error:#}", fix.guid)),
            }
        }
        for (wanted, setting) in [
            (fix.wake_enable, WakeSetting::WakeEnable),
            (fix.magic_packet_only, WakeSetting::MagicPacketOnly),
        ] {
            if wanted && let Err(error) = turn_on(nics, setting, &adapter.pnp) {
                report
                    .errors
                    .push(format!("{} {}: {error:#}", fix.guid, setting.class()));
            }
        }
        if changed && started.elapsed() >= restart_before {
            report.after_reboot.push(fix.guid.clone());
        } else if changed {
            match restart(nics, adapter) {
                Ok(()) => report.restarted.push(fix.guid.clone()),
                Err(error) => report
                    .errors
                    .push(format!("{} restart: {error:#}", fix.guid)),
            }
        }
    }
    if plan.fast_startup_off
        && let Err(error) = nics.fast_startup_off()
    {
        report.errors.push(format!("HiberbootEnabled: {error:#}"));
    }
    Ok(report)
}

/// Turns `setting` on for each instance of the device `pnp`, stopping at the
/// first that fails.
fn turn_on(nics: &mut impl Nics, setting: WakeSetting, pnp: &str) -> Result<()> {
    let instances = nics.instances(setting)?;
    for instance in matching(&instances, pnp)? {
        nics.turn_on(setting, instance)?;
    }
    Ok(())
}

/// Disables, then enables `adapter`, as `Restart-NetAdapter` does. A failure
/// in between would leave it off, so it is enabled whatever happens, once
/// more after a pause if that fails; a failed enable is the error that counts.
fn restart(nics: &mut impl Nics, adapter: &PhysicalAdapter) -> Result<()> {
    let disabled = nics.disable(adapter);
    nics.enable(adapter).or_else(|_| {
        nics.pause(ENABLE_RETRY);
        nics.enable(adapter)
    })?;
    disabled
}

/// Windows: WMI for the adapters and their device's wake settings, the
/// driver's registry key for an advanced property. Runs on a thread with COM
/// initialised.
#[derive(Default)]
pub(super) struct Native {
    adapters: Option<Wmi>,
    devices: Option<Wmi>,
}

impl Native {
    fn adapters(&mut self) -> Result<&Wmi> {
        if self.adapters.is_none() {
            self.adapters = Some(Wmi::connect(net_adapter::NAMESPACE)?);
        }
        Ok(self.adapters.as_ref().unwrap())
    }
    fn devices(&mut self) -> Result<&Wmi> {
        if self.devices.is_none() {
            self.devices = Some(Wmi::connect(r"root\wmi")?);
        }
        Ok(self.devices.as_ref().unwrap())
    }
}

impl Nics for Native {
    fn physical(&mut self) -> Result<Vec<PhysicalAdapter>> {
        net_adapter::physical_adapters(self.adapters()?)
    }
    fn advanced(&mut self) -> Result<Vec<AdvancedProperty>> {
        let all = wmi::context(&[("AllProperties", wmi::Value::Flag(true))])?;
        self.adapters()?
            .query(
                "SELECT * FROM MSFT_NetAdapterAdvancedPropertySettingData",
                Some(&all),
            )?
            .iter()
            .map(|property| {
                // `{guid}::keyword`
                let id = wmi::string(property, "InstanceID")?.unwrap_or_default();
                Ok(AdvancedProperty {
                    guid: id.split("::").next().unwrap_or_default().to_owned(),
                    keyword: wmi::string(property, "RegistryKeyword")?.unwrap_or_default(),
                    value: wmi::strings(property, "RegistryValue")?
                        .into_iter()
                        .next()
                        .unwrap_or_default(),
                })
            })
            .collect()
    }
    fn instances(&mut self, setting: WakeSetting) -> Result<Vec<WmiInstance>> {
        self.devices()?
            .query(&format!("SELECT * FROM {}", setting.class()), None)?
            .iter()
            .map(|instance| {
                Ok(WmiInstance {
                    name: wmi::string(instance, "InstanceName")?.unwrap_or_default(),
                    on: wmi::boolean(instance, setting.property())?.unwrap_or_default(),
                    path: wmi::string(instance, "__RELPATH")?.unwrap_or_default(),
                })
            })
            .collect()
    }
    fn set_advanced(
        &mut self,
        adapter: &PhysicalAdapter,
        keyword: &str,
        value: &str,
    ) -> Result<()> {
        let key = driver_key(&adapter.pnp, true)?;
        write_advanced(&key, keyword, value)
    }
    fn turn_on(&mut self, setting: WakeSetting, instance: &WmiInstance) -> Result<()> {
        let devices = self.devices()?;
        let object = devices.get(&instance.path)?;
        wmi::set_boolean(&object, setting.property(), true)?;
        devices.update(&object)
    }
    fn disable(&mut self, adapter: &PhysicalAdapter) -> Result<()> {
        self.adapters()?.call(&adapter.path, "Disable", None)
    }
    fn enable(&mut self, adapter: &PhysicalAdapter) -> Result<()> {
        self.adapters()?.call(&adapter.path, "Enable", None)
    }
    fn fast_startup_off(&mut self) -> Result<()> {
        use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, REG_DWORD, RegSetKeyValueW};
        let off = 0u32;
        // SAFETY: The strings are static and `off` is the 4-byte REG_DWORD the size names.
        unsafe {
            RegSetKeyValueW(
                HKEY_LOCAL_MACHINE,
                windows::core::w!("SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Power"),
                windows::core::w!("HiberbootEnabled"),
                REG_DWORD.0,
                Some((&raw const off).cast()),
                4,
            )
        }
        .ok()
        .context("writing HiberbootEnabled")
    }
    fn pause(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// An open registry key, closed when dropped.
pub(super) struct Key(windows::Win32::System::Registry::HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: The key was opened by this module and is closed only here.
        let _ = unsafe { windows::Win32::System::Registry::RegCloseKey(self.0) };
    }
}

impl Key {
    /// `path` under this key, `None` when it isn't there.
    fn open(
        &self,
        path: &str,
        access: windows::Win32::System::Registry::REG_SAM_FLAGS,
    ) -> Result<Option<Key>> {
        use windows::Win32::{
            Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND},
            System::Registry::{HKEY, RegOpenKeyExW},
        };
        let wide = to_wide(path);
        let mut key = HKEY::default();
        // SAFETY: `wide` is NUL-terminated and outlives the call; `key` is a live local.
        match unsafe {
            RegOpenKeyExW(
                self.0,
                windows::core::PCWSTR(wide.as_ptr()),
                None,
                access,
                &mut key,
            )
        } {
            error if error.is_ok() => Ok(Some(Key(key))),
            error if error == ERROR_FILE_NOT_FOUND || error == ERROR_PATH_NOT_FOUND => Ok(None),
            error => Err(windows::core::Error::from(error.to_hresult()))
                .with_context(|| format!("opening {path}")),
        }
    }
    /// The type of the value `name` (`REG_SZ`, `REG_DWORD`...), `None` when
    /// there is no such value.
    fn kind(&self, name: &str) -> Result<Option<u32>> {
        use windows::Win32::{
            Foundation::ERROR_FILE_NOT_FOUND,
            System::Registry::{REG_VALUE_TYPE, RegQueryValueExW},
        };
        let wide = to_wide(name);
        let mut kind = REG_VALUE_TYPE::default();
        // SAFETY: `wide` is NUL-terminated and outlives the call; only the type is read.
        match unsafe {
            RegQueryValueExW(
                self.0,
                windows::core::PCWSTR(wide.as_ptr()),
                None,
                Some(&mut kind),
                None,
                None,
            )
        } {
            error if error.is_ok() => Ok(Some(kind.0)),
            error if error == ERROR_FILE_NOT_FOUND => Ok(None),
            error => Err(windows::core::Error::from(error.to_hresult()))
                .with_context(|| format!("reading the value {name}")),
        }
    }
    /// The string value `name`, `None` when there is none or it isn't a
    /// string. Tests compare it with what WMI reports.
    #[cfg(test)]
    pub(super) fn string(&self, name: &str) -> Result<Option<String>> {
        use windows::Win32::{
            Foundation::{ERROR_FILE_NOT_FOUND, ERROR_UNSUPPORTED_TYPE},
            System::Registry::{RRF_RT_REG_SZ, RegGetValueW},
        };
        let wide = to_wide(name);
        let mut buffer = vec![0u16; 1024];
        let mut size = (buffer.len() * 2) as u32;
        // SAFETY: `wide` is NUL-terminated and outlives the call; `buffer` is writable for
        // the `size` bytes passed, and RegGetValueW NUL-terminates what it returns.
        match unsafe {
            RegGetValueW(
                self.0,
                windows::core::PCWSTR::null(),
                windows::core::PCWSTR(wide.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut size),
            )
        } {
            error if error.is_ok() => Ok(Some(crate::text::from_wide(&buffer))),
            error if error == ERROR_FILE_NOT_FOUND || error == ERROR_UNSUPPORTED_TYPE => Ok(None),
            error => Err(windows::core::Error::from(error.to_hresult()))
                .with_context(|| format!("reading the value {name}")),
        }
    }
    fn set_string(&self, name: &str, value: &str) -> Result<()> {
        use windows::Win32::System::Registry::{REG_SZ, RegSetValueExW};
        let wide = to_wide(name);
        let data: Vec<u8> = to_wide(value)
            .iter()
            .flat_map(|c| c.to_le_bytes())
            .collect();
        // SAFETY: `wide` is NUL-terminated and outlives the call; `data` is the value with
        // its terminating NUL.
        unsafe {
            RegSetValueExW(
                self.0,
                windows::core::PCWSTR(wide.as_ptr()),
                None,
                REG_SZ,
                Some(&data),
            )
        }
        .ok()
        .with_context(|| format!("writing the value {name}"))
    }
    fn set_dword(&self, name: &str, value: u32) -> Result<()> {
        use windows::Win32::System::Registry::{REG_DWORD, RegSetValueExW};
        let wide = to_wide(name);
        // SAFETY: `wide` is NUL-terminated and outlives the call.
        unsafe {
            RegSetValueExW(
                self.0,
                windows::core::PCWSTR(wide.as_ptr()),
                None,
                REG_DWORD,
                Some(&value.to_le_bytes()),
            )
        }
        .ok()
        .with_context(|| format!("writing the value {name}"))
    }
}

/// The software (driver) key of the device `pnp`, where its advanced
/// properties live, open to read values, and to write them with `write`.
pub(super) fn driver_key(pnp: &str, write: bool) -> Result<Key> {
    use windows::Win32::{
        Devices::DeviceAndDriverInstallation::{
            CM_LOCATE_DEVNODE_NORMAL, CM_Locate_DevNodeW, CM_MapCrToWin32Err, CM_Open_DevNode_Key,
            CM_REGISTRY_SOFTWARE, CONFIGRET, CR_SUCCESS, RegDisposition_OpenExisting,
        },
        System::Registry::{HKEY, KEY_QUERY_VALUE, KEY_SET_VALUE},
    };
    if pnp.is_empty() {
        bail!("the adapter has no PnP id");
    }
    let failed = |status: CONFIGRET, what: &str| {
        // SAFETY: Maps a status code; no pointers.
        let code = unsafe { CM_MapCrToWin32Err(status, 0) };
        anyhow::Error::from(std::io::Error::from_raw_os_error(code as i32))
            .context(format!("{what} {pnp} (CONFIGRET {})", status.0))
    };
    let id = to_wide(pnp);
    let mut node = 0u32;
    // SAFETY: `id` is NUL-terminated and outlives the call; `node` is a live local.
    let status = unsafe {
        CM_Locate_DevNodeW(
            &mut node,
            windows::core::PCWSTR(id.as_ptr()),
            CM_LOCATE_DEVNODE_NORMAL,
        )
    };
    if status != CR_SUCCESS {
        return Err(failed(status, "finding the device"));
    }
    let access = if write {
        KEY_QUERY_VALUE | KEY_SET_VALUE
    } else {
        KEY_QUERY_VALUE
    };
    let mut key = HKEY::default();
    // SAFETY: `key` is a live local the call fills; it is closed by Key.
    let status = unsafe {
        CM_Open_DevNode_Key(
            node,
            access.0,
            0,
            RegDisposition_OpenExisting,
            &mut key,
            CM_REGISTRY_SOFTWARE,
        )
    };
    if status != CR_SUCCESS {
        return Err(failed(status, "opening the driver key of"));
    }
    Ok(Key(key))
}

/// Sets the advanced property `keyword` in the driver key `key` as
/// `Set-NetAdapterAdvancedProperty` does: only a property the driver
/// defines (`Ndi\Params\<keyword>`), only a value it lists when it lists
/// them (`Enum`), and in the type it already has, else as a string.
pub(super) fn write_advanced(key: &Key, keyword: &str, value: &str) -> Result<()> {
    use windows::Win32::System::Registry::{KEY_QUERY_VALUE, REG_DWORD};
    let Some(params) = key.open(&format!(r"Ndi\Params\{keyword}"), KEY_QUERY_VALUE)? else {
        bail!("the adapter has no advanced property {keyword}");
    };
    if let Some(values) = params.open("Enum", KEY_QUERY_VALUE)?
        && values.kind(value)?.is_none()
    {
        bail!("{value:?} is not one of the values {keyword} takes");
    }
    if key.kind(keyword)? == Some(REG_DWORD.0) {
        let number = value
            .parse()
            .with_context(|| format!("{keyword} is a number, not {value:?}"))?;
        key.set_dword(keyword, number)
    } else {
        key.set_string(keyword, value)
    }
}

#[cfg(test)]
#[path = "nics_tests.rs"]
mod tests;
